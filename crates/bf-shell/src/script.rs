//! El motor de JavaScript de cada documento ([`Engine`]): un `bf_js::Runtime` con la ventana de
//! `bf-web` (DOM, HTML, BOM, CSSOM) y los hooks del navegador. El documento se parsea con
//! ejecución de scripts (`bf_web::bom::script`) y vive en el `WebContext` del realm; el hilo
//! del documento toma de ahí una copia para maquetar cada vez que los scripts lo cambian.
//!
//! Los hooks ([`ShellHooks`], [`ShellBom`], [`ShellWeb`]) dan la consola, la jarra de cookies
//! de `bf-net`, el `localStorage` persistido en `%APPDATA%\freedom\storage` y, en la ventana,
//! las peticiones a la interfaz ([`PageRequest`]: navegar, diálogos, pestañas nuevas).

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use bf_dom::Document;
use bf_infra::Url;
use bf_js::host::{ConsoleLevel, HostHooks, RejectionOperation};
use bf_js::vm::{AgentOptions, RealmId};
use bf_js::{JsError, Runtime};
use bf_net::NetLoader;
use bf_web::bom::{BomHooks, ScreenInfo};
use bf_web::{DocumentOwner, WebContext, WebHooks};

use crate::load::CachedLoader;

/// Cuánto puede correr una tarea antes de que el motor la corte.
const TASK_DEADLINE: Duration = Duration::from_secs(10);

/// Pila de JavaScript de cada documento (el hilo del documento tiene más).
pub const JS_STACK: usize = 64 << 20;

/// Pila del hilo que parsea y ejecuta scripts.
pub const THREAD_STACK: usize = 128 << 20;

/// Una entrada de la consola de un documento.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleEntry {
    pub level: ConsoleLevel,
    pub text: String,
}

/// A dónde van los mensajes de la consola y los errores de un documento.
pub type ConsoleSink = Box<dyn FnMut(ConsoleEntry)>;

/// Un diálogo modal (HTML §8.8.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogKind {
    Alert,
    Confirm,
    /// Con el valor por defecto.
    Prompt(String),
}

/// Lo que un documento le pide a la interfaz.
pub enum PageRequest {
    /// Navegar la pestaña (`replace`: sin entrada nueva en el historial).
    Navigate {
        url: Url,
        replace: bool,
    },
    /// La URL cambió sin cargar otro documento (`pushState`, fragmento).
    SameDocument {
        url: Url,
        replace: bool,
    },
    Reload,
    /// `history.go(delta)` fuera del documento.
    Traverse(i32),
    /// `window.open`: una pestaña nueva.
    Open(String),
    /// `window.close()`.
    Close,
    /// `window.scrollTo` y compañía: desplazar la página a (x, y) px CSS.
    ScrollTo(f32, f32),
    /// Un diálogo: la interfaz responde por `reply` (`None`: cancelado; para `alert` y
    /// `confirm`, `Some("")` es aceptar).
    Dialog {
        kind: DialogKind,
        message: String,
        reply: Sender<Option<String>>,
    },
}

/// A dónde van las peticiones de un documento (en la línea de comandos, a ningún lado).
pub type RequestSink = Rc<dyn Fn(PageRequest)>;

/// Lo que el navegador sabe del documento al cargarlo.
#[derive(Clone)]
pub struct PageEnv {
    pub net: Arc<NetLoader>,
    pub requests: Option<RequestSink>,
    /// Entradas del historial hasta la actual inclusive y después (`history.length`).
    pub session_history: (u32, u32),
    pub referrer: String,
    pub screen: ScreenInfo,
    /// Las fuentes del sistema, para maquetar cuando JavaScript pide geometría con el layout
    /// desactualizado (sin ellas, el CSSOM usa métricas de prueba).
    pub fonts: Option<Arc<bf_font::FontDb>>,
}

impl PageEnv {
    pub fn cli(net: Arc<NetLoader>) -> PageEnv {
        PageEnv {
            net,
            requests: None,
            session_history: (1, 0),
            referrer: String::new(),
            screen: ScreenInfo::default(),
            fonts: None,
        }
    }
}

/// El [`bf_web::cssom::LayoutProvider`] del navegador: maqueta con las fuentes reales y los
/// tamaños de las imágenes del último layout.
struct ShellLayout {
    fonts: bf_font::Fonts,
    images: Rc<RefCell<bf_layout::ImageSizes>>,
}

impl bf_web::cssom::LayoutProvider for ShellLayout {
    fn layout(
        &mut self,
        doc: &Document,
        styles: &bf_style::StyleMap,
        viewport: bf_layout::Size,
    ) -> bf_layout::FragmentTree {
        bf_layout::layout_document_with_images(
            doc,
            styles,
            viewport,
            &self.fonts,
            &self.images.borrow(),
        )
    }
}

/// Los hooks del motor: consola, rechazos sin manejar y `User-Agent`.
struct ShellHooks {
    console: Rc<RefCell<ConsoleSink>>,
    rejections: Rc<RefCell<usize>>,
}

impl ShellHooks {
    fn emit(&mut self, level: ConsoleLevel, text: String) {
        (self.console.borrow_mut())(ConsoleEntry { level, text });
    }
}

impl HostHooks for ShellHooks {
    fn console(&mut self, level: ConsoleLevel, message: &str) {
        self.emit(level, message.to_owned());
    }

    fn report_exception(&mut self, message: &str, stack: &str) {
        let text = if stack.is_empty() {
            format!("Uncaught {message}")
        } else {
            format!("Uncaught {message}\n{stack}")
        };
        self.emit(ConsoleLevel::Error, text);
    }

    fn promise_rejection(
        &mut self,
        _promise: bf_js::heap::Gc<bf_js::object::Object>,
        operation: RejectionOperation,
    ) {
        let mut n = self.rejections.borrow_mut();
        match operation {
            RejectionOperation::Reject => *n += 1,
            RejectionOperation::Handle => *n = n.saturating_sub(1),
        }
    }

    fn user_agent(&self) -> String {
        bf_net::USER_AGENT.to_owned()
    }
}

/// Los hooks de la ventana de `bf-web`: diálogos y `window.open`.
struct ShellWeb {
    requests: Option<RequestSink>,
}

impl ShellWeb {
    fn dialog(&self, kind: DialogKind, message: &str) -> Option<String> {
        let requests = self.requests.as_ref()?;
        let (reply, answer) = channel();
        requests(PageRequest::Dialog {
            kind,
            message: message.to_owned(),
            reply,
        });
        answer.recv().ok().flatten()
    }
}

impl WebHooks for ShellWeb {
    fn alert(&mut self, message: &str) {
        self.dialog(DialogKind::Alert, message);
    }

    fn confirm(&mut self, message: &str) -> bool {
        self.dialog(DialogKind::Confirm, message).is_some()
    }

    fn prompt(&mut self, message: &str, default: &str) -> Option<String> {
        self.dialog(DialogKind::Prompt(default.to_owned()), message)
    }

    fn open(&mut self, url: &str, _target: &str, _features: &str) -> bool {
        match &self.requests {
            Some(r) => {
                r(PageRequest::Open(url.to_owned()));
                true
            }
            None => false,
        }
    }
}

/// Los hooks del BOM: cookies de `bf-net`, `localStorage` en disco y la interfaz.
struct ShellBom {
    env: PageEnv,
}

impl ShellBom {
    fn request(&self, r: PageRequest) {
        if let Some(sink) = &self.env.requests {
            sink(r);
        }
    }
}

impl BomHooks for ShellBom {
    fn navigate(&mut self, url: &Url, replace: bool) {
        self.request(PageRequest::Navigate {
            url: url.clone(),
            replace,
        });
    }

    fn same_document_navigation(&mut self, url: &Url, replace: bool) {
        self.request(PageRequest::SameDocument {
            url: url.clone(),
            replace,
        });
    }

    fn reload(&mut self) {
        self.request(PageRequest::Reload);
    }

    fn traverse(&mut self, delta: i32) {
        self.request(PageRequest::Traverse(delta));
    }

    fn session_history(&self) -> (u32, u32) {
        self.env.session_history
    }

    fn cookies(&mut self, url: &Url) -> String {
        self.env.net.fetcher.cookies.script_cookies(url)
    }

    fn set_cookie(&mut self, url: &Url, value: &str) {
        self.env
            .net
            .fetcher
            .cookies
            .set_cookie_from_script(url, value);
    }

    fn load_storage(&mut self, origin: &str) -> Vec<(String, String)> {
        storage_path(origin)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|s| parse_storage(&s))
            .unwrap_or_default()
    }

    fn save_storage(&mut self, origin: &str, items: &[(String, String)]) {
        let Some(path) = storage_path(origin) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&path, serialize_storage(items)) {
            eprintln!("aviso: no se pudo guardar {}: {e}", path.display());
        }
    }

    fn screen(&self) -> ScreenInfo {
        self.env.screen
    }

    fn languages(&self) -> Vec<String> {
        bf_net::languages()
    }

    fn hardware_concurrency(&self) -> u32 {
        std::thread::available_parallelism().map_or(4, |n| n.get() as u32)
    }

    fn close(&mut self) {
        self.request(PageRequest::Close);
    }

    fn referrer(&self) -> String {
        self.env.referrer.clone()
    }
}

/// El archivo del `localStorage` de `origin`: `%APPDATA%\freedom\storage\<origen>.json`
/// (`$HOME/.freedom/storage` fuera de Windows). Los orígenes opacos no se guardan.
fn storage_path(origin: &str) -> Option<PathBuf> {
    if origin == "null" {
        return None;
    }
    let base = std::env::var_os("APPDATA")
        .map(|d| PathBuf::from(d).join("freedom"))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".freedom")))?;
    let name: String = origin
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Some(base.join("storage").join(format!("{name}.json")))
}

/// Un string JSON.
fn json_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// `[["clave","valor"],…]` (conserva el orden de inserción).
pub fn serialize_storage(items: &[(String, String)]) -> String {
    let mut out = String::from("[");
    for (i, (k, v)) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('[');
        json_string(k, &mut out);
        out.push(',');
        json_string(v, &mut out);
        out.push(']');
    }
    out.push(']');
    out
}

/// Lee lo que escribió [`serialize_storage`] (lo que no se entiende se descarta).
pub fn parse_storage(s: &str) -> Vec<(String, String)> {
    let mut strings = Vec::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut cur = String::new();
        loop {
            match chars.next() {
                None => return Vec::new(),
                Some('"') => break,
                Some('\\') => match chars.next() {
                    Some('n') => cur.push('\n'),
                    Some('r') => cur.push('\r'),
                    Some('t') => cur.push('\t'),
                    Some('u') => {
                        let hex: String = chars.by_ref().take(4).collect();
                        if let Some(c) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)
                        {
                            cur.push(c);
                        }
                    }
                    Some(c) => cur.push(c),
                    None => return Vec::new(),
                },
                Some(c) => cur.push(c),
            }
        }
        strings.push(cur);
    }
    strings
        .chunks(2)
        .filter(|p| p.len() == 2)
        .map(|p| (p[0].clone(), p[1].clone()))
        .collect()
}

/// El motor de JavaScript de un documento.
pub struct Engine {
    pub rt: Runtime,
    pub realm: RealmId,
    console: Rc<RefCell<ConsoleSink>>,
    rejections: Rc<RefCell<usize>>,
    /// Rechazos sin manejar ya reportados a la consola.
    reported_rejections: usize,
    /// Los tamaños de las imágenes del último layout (para el [`ShellLayout`]).
    images: Rc<RefCell<bf_layout::ImageSizes>>,
}

impl Engine {
    /// Un motor nuevo; `console` recibe los mensajes y errores.
    pub fn new(console: ConsoleSink) -> Engine {
        let console = Rc::new(RefCell::new(console));
        let rejections = Rc::new(RefCell::new(0));
        let hooks = ShellHooks {
            console: console.clone(),
            rejections: rejections.clone(),
        };
        let rt = Runtime::with_options(
            Box::new(hooks),
            AgentOptions {
                max_stack: JS_STACK,
                ..AgentOptions::default()
            },
        );
        let realm = rt.main_realm();
        Engine {
            rt,
            realm,
            console,
            rejections,
            reported_rejections: 0,
            images: Rc::new(RefCell::new(bf_layout::ImageSizes::new())),
        }
    }

    /// Un motor que escribe la consola en stderr (línea de comandos).
    pub fn stderr() -> Engine {
        Engine::new(Box::new(|e: ConsoleEntry| {
            let tag = match e.level {
                ConsoleLevel::Error => "error",
                ConsoleLevel::Warn => "aviso",
                _ => "consola",
            };
            let first = e.text.lines().next().unwrap_or_default();
            let short: String = first.chars().take(300).collect();
            eprintln!("[{tag}] {short}");
        }))
    }

    fn report(&mut self, level: ConsoleLevel, text: String) {
        (self.console.borrow_mut())(ConsoleEntry { level, text });
    }

    /// Tras correr script: los rechazos nuevos que siguen sin manejar, a la consola (como el
    /// "Uncaught (in promise)" de los navegadores).
    fn flush_rejections(&mut self) {
        let n = *self.rejections.borrow();
        if n > self.reported_rejections {
            let new = n - self.reported_rejections;
            self.report(
                ConsoleLevel::Error,
                format!("Uncaught (in promise): {new} promesa(s) rechazada(s) sin manejar"),
            );
        }
        self.reported_rejections = n;
    }

    fn report_error(&mut self, e: &JsError) {
        let text = if e.stack.is_empty() {
            format!("Uncaught {}", e.message)
        } else {
            format!("Uncaught {}\n{}", e.message, e.stack)
        };
        self.report(ConsoleLevel::Error, text);
    }

    /// ¿Ya tiene una ventana (`WebContext`)?
    fn has_window(&self) -> bool {
        self.rt.agent.realm_data::<WebContext>(self.realm).is_some()
    }

    /// Instala la ventana para el documento en `url`, con los hooks de `env`.
    fn install_window(&mut self, url: &Url, loader: Arc<CachedLoader>, env: &PageEnv) {
        let realm = self.realm;
        let mut ctx = WebContext::new(DocumentOwner::Document(Box::default()), url.clone());
        ctx.hooks = Box::new(ShellWeb {
            requests: env.requests.clone(),
        });
        bf_web::install(&mut self.rt.agent, realm, ctx);
        bf_web::bom::set_hooks(
            &mut self.rt.agent,
            realm,
            Box::new(ShellBom { env: env.clone() }),
        );
        bf_web::bom::script::install_scripting(&mut self.rt.agent, realm, url, loader);
        if let Some(db) = &env.fonts {
            let provider = ShellLayout {
                fonts: bf_font::Fonts::new(db.clone()),
                images: self.images.clone(),
            };
            bf_web::cssom::set_layout_provider(&mut self.rt.agent, realm, Box::new(provider));
        }
    }

    /// El layout que acaba de hacer el navegador (para el CSSOM View: `getBoundingClientRect`,
    /// `innerWidth`, `getComputedStyle`…).
    pub fn set_layout(
        &mut self,
        page: &bf_paint::Page,
        version: u64,
        scroll: (f32, f32),
        device_pixel_ratio: f32,
    ) {
        let realm = self.realm;
        let generation = bf_web::cssom::sheets_generation(&self.rt.agent, realm);
        let mut snapshot =
            bf_web::LayoutSnapshot::new(page.styles.clone(), Arc::new(page.tree.clone()), version);
        snapshot.sheets_generation = generation;
        snapshot.scroll = scroll;
        snapshot.device_pixel_ratio = device_pixel_ratio;
        *self.images.borrow_mut() = page.images.sizes.clone();
        if let Some(ctx) = self.rt.agent.realm_data_mut::<WebContext>(realm) {
            ctx.layout = Some(snapshot);
        }
        self.update_view(scroll, device_pixel_ratio);
    }

    /// El desplazamiento o la escala de la página cambiaron (sin volver a maquetar).
    pub fn update_view(&mut self, scroll: (f32, f32), device_pixel_ratio: f32) {
        let realm = self.realm;
        if let Some(ctx) = self.rt.agent.realm_data_mut::<WebContext>(realm)
            && let Some(l) = &mut ctx.layout
        {
            l.scroll = scroll;
            l.device_pixel_ratio = device_pixel_ratio;
        }
        if let Some(s) = self
            .rt
            .agent
            .realm_data_mut::<bf_web::cssom::CssomState>(realm)
        {
            s.scroll = scroll;
            s.device_pixel_ratio = device_pixel_ratio;
        }
    }

    /// Un desplazamiento pedido desde JavaScript (`scrollTo`…) que la ventana debe aplicar.
    pub fn take_scroll_request(&mut self) -> Option<(f32, f32)> {
        self.rt
            .agent
            .realm_data_mut::<bf_web::cssom::CssomState>(self.realm)
            .and_then(|s| s.scroll_request.take())
    }

    /// Parsea `text` (el HTML ya decodificado de `url`) ejecutando sus scripts; devuelve una
    /// copia del documento resultante.
    pub fn parse(
        &mut self,
        text: &str,
        url: &Url,
        loader: Arc<CachedLoader>,
        env: &PageEnv,
    ) -> Document {
        if !self.has_window() {
            self.install_window(url, loader, env);
        }
        let realm = self.realm;
        let agent = &mut self.rt.agent;
        bf_web::bom::script::begin_parse(agent, realm, url);
        bf_web::bom::script::feed(agent, realm, text);
        bf_web::bom::script::finish(agent, realm);
        self.flush_rejections();
        self.take_dirty();
        self.document().cloned().unwrap_or_default()
    }

    /// El documento de la ventana.
    pub fn document(&self) -> Option<&Document> {
        self.rt
            .agent
            .realm_data::<WebContext>(self.realm)
            .map(WebContext::document)
    }

    /// Reemplaza el documento de la ventana (la interfaz editó un campo).
    pub fn set_document(&mut self, doc: Document) {
        if let Some(ctx) = self.rt.agent.realm_data_mut::<WebContext>(self.realm) {
            *ctx.document_mut() = doc;
        }
    }

    /// ¿Los scripts cambiaron el DOM desde la última vez? (Y se limpia la marca.)
    pub fn take_dirty(&mut self) -> bool {
        self.rt
            .agent
            .realm_data_mut::<WebContext>(self.realm)
            .is_some_and(WebContext::take_dirty) // E3: también los cambios sin `mutated()`.
    }

    /// Scripts ejecutados y con error.
    pub fn counts(&self) -> (usize, usize) {
        bf_web::bom::script::script_env(&self.rt.agent, self.realm)
            .map_or((0, 0), |e| (e.executed, e.failed))
    }

    /// Corre las tareas que vencieron (temporizadores, async, …). Devuelve cuántas corrió.
    pub fn run_due_tasks(&mut self) -> usize {
        let mut n = 0;
        loop {
            self.rt.agent.options.deadline = Some(Instant::now() + TASK_DEADLINE);
            let r = self.rt.run_one_task();
            self.rt.agent.options.deadline = None;
            match r {
                Ok(true) => n += 1,
                Ok(false) => break,
                Err(e) => {
                    n += 1;
                    self.report_error(&e);
                }
            }
            if n > 10_000 {
                break;
            }
        }
        if n > 0 {
            self.flush_rejections();
        }
        n
    }

    /// Cuánto falta para el próximo temporizador: `None` si no hay ninguno.
    pub fn next_wake(&mut self) -> Option<Duration> {
        let at = self.rt.next_wake()?;
        let now = self.rt.agent.host.hooks.now();
        Some(Duration::from_secs_f64(((at - now) / 1000.0).max(0.0)))
    }
}

/// Parsea `text` con scripts, con un motor nuevo y los hooks de la línea de comandos.
pub fn parse_with_scripts(
    text: &str,
    url: &Url,
    engine: &mut Engine,
    loader: Arc<CachedLoader>,
    env: &PageEnv,
) -> Document {
    engine.parse(text, url, loader, env)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine_with_log() -> (Engine, Rc<RefCell<Vec<ConsoleEntry>>>) {
        let log = Rc::new(RefCell::new(Vec::new()));
        let sink = log.clone();
        let engine = Engine::new(Box::new(move |e| sink.borrow_mut().push(e)));
        (engine, log)
    }

    fn env() -> PageEnv {
        PageEnv::cli(Arc::new(NetLoader::new()))
    }

    fn loader(env: &PageEnv) -> Arc<CachedLoader> {
        Arc::new(CachedLoader::new(env.net.clone()))
    }

    fn texts(log: &Rc<RefCell<Vec<ConsoleEntry>>>) -> Vec<String> {
        log.borrow().iter().map(|e| e.text.clone()).collect()
    }

    fn parse(html: &str) -> (Engine, Rc<RefCell<Vec<ConsoleEntry>>>, Document) {
        let (mut engine, log) = engine_with_log();
        let url = Url::parse("https://example.test/", None).unwrap();
        let env = env();
        let doc = engine.parse(html, &url, loader(&env), &env);
        (engine, log, doc)
    }

    #[test]
    fn inline_scripts_see_the_document() {
        let (engine, log, _) = parse(
            "<p id=a>x</p><script>console.log('uno', document.getElementById('a').textContent)</script>\
             <script>console.log('dos', 1 + 1)</script>",
        );
        assert_eq!(texts(&log), ["uno x", "dos 2"]);
        assert_eq!(engine.counts(), (2, 0));
    }

    #[test]
    fn errors_go_to_console() {
        let (engine, log, _) =
            parse("<script>noExiste()</script><script>console.log('sigue')</script>");
        let t = texts(&log);
        assert!(t[0].starts_with("Uncaught ReferenceError"), "{t:?}");
        assert_eq!(t[1], "sigue");
        assert_eq!(engine.counts(), (2, 1));
    }

    #[test]
    fn document_write_shapes_the_document() {
        let (_, _, doc) = parse("<body><script>document.write('<p id=w>escrito</p>')</script>");
        assert!(doc.get_element_by_id(&"w".into()).is_some());
    }

    #[test]
    fn inline_module_and_data_url_external_script() {
        let (_, log, _) = parse(
            "<script type=module>console.log('m', import.meta.url.startsWith('https://example.test/'))</script>\
             <script src=\"data:text/javascript,console.log('externo')\"></script>\
             <script type=text/plain>console.log('no')</script>",
        );
        assert_eq!(texts(&log), ["externo", "m true"]);
    }

    #[test]
    fn module_imports_through_import_map() {
        let (_, log, _) = parse(
            "<script type=importmap>{\"imports\": {\"lib\": \"data:text/javascript,export const x = 42;\"}}</script>\
             <script type=module>import { x } from 'lib'; console.log('x', x)</script>",
        );
        assert_eq!(texts(&log), ["x 42"]);
    }

    #[test]
    fn timers_run_as_tasks_and_mark_dirty() {
        let (mut engine, log, _) = parse(
            "<body><script>setTimeout(function(){ console.log('timer'); \
             document.body.appendChild(document.createElement('p')); }, 0); console.log('sync')</script>",
        );
        assert_eq!(texts(&log), ["sync"]);
        assert!(engine.next_wake().is_some());
        assert!(!engine.take_dirty());
        engine.run_due_tasks();
        assert_eq!(texts(&log), ["sync", "timer"]);
        assert!(engine.take_dirty());
    }

    #[test]
    fn navigator_is_freedom() {
        let (_, log, _) = parse(
            "<script>console.log(navigator.userAgent.includes('Freedom/'), \
             navigator.userAgent.includes('Chrome'), JSON.stringify(navigator.vendor), \
             navigator.userAgentData.brands[0].brand, navigator.cookieEnabled)</script>",
        );
        assert_eq!(texts(&log), ["true false \"\" Freedom true"]);
    }

    #[test]
    fn layout_snapshot_feeds_cssom_view() {
        let (mut engine, log, doc) = parse(
            "<body style='margin:0'><div id=caja style='width:120px;height:30px;margin-top:50px'></div>",
        );
        let fonts = bf_font::FontDb::system();
        let base = Url::parse("https://example.test/", None).unwrap();
        let env = env();
        let ctx = bf_paint::PageContext {
            base: &base,
            encoding: bf_infra::encoding::Encoding::Utf8,
            loader: loader(&env),
            fonts,
            extra: &[],
        };
        let viewport = bf_layout::Size {
            width: 640.0,
            height: 480.0,
        };
        let page = bf_paint::prepare_page(&doc, &ctx, viewport);
        engine.set_layout(&page, doc.version(), (0.0, 20.0), 1.25);
        let realm = engine.realm;
        engine
            .rt
            .eval_script(
                realm,
                "var r = document.getElementById('caja').getBoundingClientRect();                  console.log(innerWidth, innerHeight, scrollY, devicePixelRatio, r.width, r.top)",
                "t.js",
            )
            .unwrap();
        assert_eq!(texts(&log).last().unwrap(), "640 480 20 1.25 120 30");
        engine.update_view((0.0, 0.0), 1.0);
        engine
            .rt
            .eval_script(realm, "console.log(scrollY, devicePixelRatio)", "t.js")
            .unwrap();
        assert_eq!(texts(&log).last().unwrap(), "0 1");
    }

    #[test]
    fn storage_file_round_trip() {
        let items = vec![
            ("a".to_owned(), "1".to_owned()),
            ("clave \"rara\"\n".to_owned(), "ñ\\u{1}".to_owned()),
        ];
        assert_eq!(parse_storage(&serialize_storage(&items)), items);
        assert_eq!(parse_storage("basura"), Vec::<(String, String)>::new());
        let p = storage_path("https://a.test:8080").unwrap();
        assert!(
            p.ends_with("storage/https___a.test_8080.json")
                || p.ends_with("storage\\https___a.test_8080.json")
        );
        assert!(storage_path("null").is_none());
    }

    #[test]
    fn user_agent_is_freedom() {
        assert!(bf_net::USER_AGENT.starts_with("Mozilla/5.0 ("));
        assert!(
            bf_net::USER_AGENT.ends_with(&format!(") Freedom/{}", bf_infra::identity::VERSION))
        );
        assert!(!bf_net::USER_AGENT.contains("Chrome"));
    }
}
