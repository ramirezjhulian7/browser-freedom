//! Ejecución de los scripts de un documento (HTML §4.12.1 y §8.1): el motor de JavaScript de
//! cada documento ([`Engine`]: un `bf_js::Runtime` con su realm y los hooks del navegador) y la
//! implementación de [`bf_html::ScriptHost`] que usa el parser ([`Scripts`]): scripts clásicos
//! inline y externos, módulos (con su grafo cargado por la red e *import maps* básicos) y el
//! reporte de errores a la consola.
//!
//! Los scripts externos se buscan en el momento con el [`CachedLoader`] del documento (el hilo
//! del documento puede bloquearse), así que el parser nunca queda esperando: los bloqueantes
//! corren en orden, los `defer` al terminar el parseo y los `async` como tareas.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use bf_dom::script::ScriptType;
use bf_dom::{Document, NodeId};
use bf_html::{ParseOptions, Parser, ScriptDisposition};
use bf_infra::encoding::{self, Encoding};
use bf_infra::{ResourceLoader, Url};
use bf_js::host::{ConsoleLevel, HostHooks, RejectionOperation};
use bf_js::vm::module::ModuleLoader;
use bf_js::vm::{AgentOptions, RealmId};
use bf_js::{JsError, Runtime};

use crate::load::CachedLoader;

/// Cuánto puede correr un script (o una tarea) antes de que el motor lo corte.
const SCRIPT_DEADLINE: Duration = Duration::from_secs(10);

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

/// Los hooks del navegador para el motor de un documento.
struct ShellHooks {
    console: Rc<RefCell<ConsoleSink>>,
    /// Rechazos de promesas sin manejar (`promise_rejection`).
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

/// Un *import map* (HTML §8.1.5.3): especificador (o prefijo terminado en `/`) → URL.
#[derive(Debug, Clone, Default)]
pub struct ImportMap {
    pub imports: Vec<(String, Url)>,
}

impl ImportMap {
    /// "Resolve an imports match" (HTML §8.1.5.3.6): la entrada exacta o el prefijo más largo.
    fn resolve(&self, specifier: &str) -> Option<Url> {
        if let Some((_, url)) = self.imports.iter().find(|(k, _)| k == specifier) {
            return Some(url.clone());
        }
        let (key, url) = self
            .imports
            .iter()
            .filter(|(k, _)| k.ends_with('/') && specifier.starts_with(k.as_str()))
            .max_by_key(|(k, _)| k.len())?;
        Url::parse(&specifier[key.len()..], Some(url)).ok()
    }
}

/// El cargador de módulos del documento: resuelve con el import map y la URL del que importa
/// (HTML §8.1.5.5 "resolve a module specifier") y lee por la red.
struct NetModuleLoader {
    loader: Rc<RefCell<Option<std::sync::Arc<CachedLoader>>>>,
    base: Rc<RefCell<Url>>,
    import_map: Rc<RefCell<ImportMap>>,
}

impl ModuleLoader for NetModuleLoader {
    fn resolve(&self, specifier: &str, referrer: Option<&str>) -> Result<String, String> {
        let base = referrer
            .and_then(|r| Url::parse(r, None).ok())
            .unwrap_or_else(|| self.base.borrow().clone());
        if let Some(url) = self.import_map.borrow().resolve(specifier) {
            return Ok(url.href());
        }
        // "Resolve a URL-like module specifier".
        let url_like = specifier.starts_with('/')
            || specifier.starts_with("./")
            || specifier.starts_with("../");
        let url = if url_like {
            Url::parse(specifier, Some(&base)).ok()
        } else {
            Url::parse(specifier, None).ok()
        };
        url.map(|u| u.href()).ok_or_else(|| {
            format!("Failed to resolve module specifier \"{specifier}\": relative references must start with \"/\", \"./\" or \"../\"")
        })
    }

    fn load(&self, url: &str) -> Result<String, String> {
        let parsed = Url::parse(url, None).map_err(|_| format!("URL inválida: {url}"))?;
        let loader = self.loader.borrow().clone().ok_or("sin red")?;
        let res = loader
            .load(&parsed)
            .ok_or_else(|| format!("no se pudo cargar {url}"))?;
        Ok(encoding::decode(&res.bytes, Encoding::Utf8).0)
    }
}

/// El motor de JavaScript de un documento.
pub struct Engine {
    pub rt: Runtime,
    pub realm: RealmId,
    console: Rc<RefCell<ConsoleSink>>,
    rejections: Rc<RefCell<usize>>,
    loader: Rc<RefCell<Option<std::sync::Arc<CachedLoader>>>>,
    base: Rc<RefCell<Url>>,
    import_map: Rc<RefCell<ImportMap>>,
    /// Cuántos scripts se ejecutaron y cuántos terminaron con una excepción.
    pub executed: usize,
    pub failed: usize,
    /// Módulos inline numerados (cada uno es un módulo distinto en el module map).
    inline_modules: usize,
    /// Rechazos sin manejar ya reportados a la consola.
    reported_rejections: usize,
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
        let mut rt = Runtime::with_options(
            Box::new(hooks),
            AgentOptions {
                max_stack: JS_STACK,
                ..AgentOptions::default()
            },
        );
        let loader = Rc::new(RefCell::new(None));
        let base = Rc::new(RefCell::new(
            Url::parse("about:blank", None).expect("about:blank"),
        ));
        let import_map = Rc::new(RefCell::new(ImportMap::default()));
        rt.agent.set_module_loader(Rc::new(NetModuleLoader {
            loader: loader.clone(),
            base: base.clone(),
            import_map: import_map.clone(),
        }));
        let realm = rt.main_realm();
        Engine {
            rt,
            realm,
            console,
            rejections,
            loader,
            base,
            import_map,
            executed: 0,
            failed: 0,
            inline_modules: 0,
            reported_rejections: 0,
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
            eprintln!("[{tag}] {}", e.text);
        }))
    }

    /// Para el documento en `base`, que carga sus recursos con `loader`.
    pub fn set_document(&mut self, base: &Url, loader: std::sync::Arc<CachedLoader>) {
        *self.base.borrow_mut() = base.clone();
        *self.loader.borrow_mut() = Some(loader);
    }

    /// Rechazos de promesas sin manejar.
    pub fn unhandled_rejections(&self) -> usize {
        *self.rejections.borrow()
    }

    /// Tras correr script: los rechazos nuevos que siguen sin manejar, a la consola (como
    /// "Uncaught (in promise)" de los navegadores).
    fn flush_rejections(&mut self) {
        let n = self.unhandled_rejections();
        if n > self.reported_rejections {
            let new = n - self.reported_rejections;
            self.report(
                ConsoleLevel::Error,
                format!("Uncaught (in promise): {new} promesa(s) rechazada(s) sin manejar"),
            );
        }
        self.reported_rejections = n;
    }

    fn report(&mut self, level: ConsoleLevel, text: String) {
        (self.console.borrow_mut())(ConsoleEntry { level, text });
    }

    fn report_error(&mut self, e: &JsError) {
        self.failed += 1;
        let text = if e.stack.is_empty() {
            format!("Uncaught {}", e.message)
        } else {
            format!("Uncaught {}\n{}", e.message, e.stack)
        };
        self.report(ConsoleLevel::Error, text);
    }

    /// Corre `f` con el límite de tiempo de un script.
    fn with_deadline<T>(&mut self, f: impl FnOnce(&mut Engine) -> T) -> T {
        let saved = self.rt.agent.options.deadline;
        self.rt.agent.options.deadline = Some(Instant::now() + SCRIPT_DEADLINE);
        let r = f(self);
        self.rt.agent.options.deadline = saved;
        r
    }

    /// Ejecuta un script clásico; un error va a la consola.
    pub fn run_classic(&mut self, source: &str, url: &str) {
        self.executed += 1;
        let realm = self.realm;
        let r = self.with_deadline(|e| e.rt.eval_script(realm, source, url));
        if let Err(e) = r {
            self.report_error(&e);
        }
        self.flush_rejections();
    }

    /// Ejecuta un módulo (su grafo se carga por la red); un error va a la consola.
    pub fn run_module(&mut self, source: &str, url: &str) {
        self.executed += 1;
        let r = self.with_deadline(|e| {
            let agent = &mut e.rt.agent;
            let saved = agent.current_realm;
            agent.current_realm = e.realm;
            let r = agent
                .parse_module_record(url, source)
                .and_then(|m| {
                    agent.load_requested_modules(m)?;
                    agent.link_module(m)?;
                    agent.evaluate_module(m)
                })
                .and_then(|p| {
                    agent.run_jobs()?;
                    match agent.promise_outcome(p) {
                        Some(Err(v)) => Err(bf_js::vm::Throw::Value(v)),
                        _ => Ok(()),
                    }
                });
            agent.current_realm = saved;
            r.map_err(|t| JsError::from_throw(agent, t))
        });
        if let Err(e) = r {
            self.report_error(&e);
        }
        self.flush_rejections();
    }

    /// Un *import map* inline (HTML §8.1.5.3.2 "parse an import map string"): solo `imports`.
    pub fn register_import_map(&mut self, text: &str, base: &Url) {
        let quoted = json_string(text);
        let probe = format!(
            "(function(){{var m=JSON.parse({quoted});var i=(m&&m.imports)||{{}};\
             return Object.keys(i).map(function(k){{return k+'\\u0000'+i[k];}}).join('\\u0001');}})()"
        );
        let realm = self.realm;
        match self.rt.eval_script(realm, &probe, "importmap") {
            Ok(v) => {
                let s = self.rt.to_display(&v);
                let mut map = self.import_map.borrow_mut();
                for entry in s.split('\u{1}').filter(|e| !e.is_empty()) {
                    if let Some((k, v)) = entry.split_once('\u{0}')
                        && let Ok(url) = Url::parse(v, Some(base))
                    {
                        map.imports.push((k.to_owned(), url));
                    }
                }
            }
            Err(e) => self.report_error(&e),
        }
    }

    /// Corre las tareas que vencieron (temporizadores, async, ...). Devuelve cuántas corrió.
    pub fn run_due_tasks(&mut self) -> usize {
        let mut n = 0;
        loop {
            let r = self.with_deadline(|e| e.rt.run_one_task());
            match r {
                Ok(true) => n += 1,
                Ok(false) => {
                    self.flush_rejections();
                    return n;
                }
                Err(e) => {
                    n += 1;
                    self.report_error(&e);
                }
            }
            if n > 10_000 {
                return n;
            }
        }
    }

    /// Cuándo hay que despertar para el próximo temporizador: `None` si no hay ninguno.
    pub fn next_wake(&mut self) -> Option<Duration> {
        let at = self.rt.next_wake()?;
        let now = self.rt.agent.host.hooks.now();
        Some(Duration::from_secs_f64(((at - now) / 1000.0).max(0.0)))
    }
}

/// Un string de JavaScript con comillas dobles.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Un script externo ya buscado.
struct Fetched {
    source: String,
    url: String,
}

/// El [`bf_html::ScriptHost`] del navegador: busca y ejecuta los scripts del documento con su
/// [`Engine`].
pub struct Scripts<'a> {
    pub engine: &'a mut Engine,
    loader: std::sync::Arc<CachedLoader>,
    fetched: HashMap<NodeId, Fetched>,
    /// ¿Hay módulos? (`nomodule` se ignora entonces: HTML §4.12.1.1 paso 18.)
    modules: bool,
}

impl<'a> Scripts<'a> {
    pub fn new(engine: &'a mut Engine, loader: std::sync::Arc<CachedLoader>) -> Scripts<'a> {
        Scripts {
            engine,
            loader,
            fetched: HashMap::new(),
            modules: true,
        }
    }

    fn fetch(&mut self, url: &Url, charset: &str) -> Option<Fetched> {
        let res = self.loader.load(url)?;
        let fallback = Encoding::for_label(charset).unwrap_or(Encoding::Utf8);
        let (source, _) = encoding::decode(&res.bytes, fallback);
        Some(Fetched {
            source,
            url: res.url.href(),
        })
    }
}

impl bf_dom::events::ScriptHost for Scripts<'_> {
    fn call_listener(
        &mut self,
        _doc: &mut Document,
        _handle: u64,
        _event: &mut bf_dom::events::Event,
    ) -> bool {
        false
    }
}

impl bf_html::ScriptHost for Scripts<'_> {
    fn prepare(&mut self, doc: &mut Parser, script: NodeId) -> ScriptDisposition {
        let Some(info) = doc.document().script_info(script).cloned() else {
            return ScriptDisposition::Failed;
        };
        let Some(url) = info.url.clone() else {
            // Inline: el resultado ya está.
            return ScriptDisposition::Ready;
        };
        match self.fetch(&url, info.encoding) {
            Some(f) => {
                self.fetched.insert(script, f);
                ScriptDisposition::Ready
            }
            None => {
                self.engine.report(
                    ConsoleLevel::Error,
                    format!("Failed to load resource: {}", url.href()),
                );
                ScriptDisposition::Failed
            }
        }
    }

    fn execute(&mut self, doc: &mut Parser, script: NodeId) {
        let Some(info) = doc.document().script_info(script).cloned() else {
            return;
        };
        let _ = self.modules;
        let page = doc.document().url.href();
        match info.type_ {
            ScriptType::Classic => match self.fetched.remove(&script) {
                Some(f) => self.engine.run_classic(&f.source, &f.url),
                None => self
                    .engine
                    .run_classic(&info.source_text.as_lossy_str(), &page),
            },
            ScriptType::Module => match self.fetched.remove(&script) {
                Some(f) => self.engine.run_module(&f.source, &f.url),
                None => {
                    self.engine.inline_modules += 1;
                    let url = format!("{page}#module-{}", self.engine.inline_modules);
                    self.engine
                        .run_module(&info.source_text.as_lossy_str(), &url);
                }
            },
            ScriptType::ImportMap => {
                let base = info.base_url.clone();
                self.engine
                    .register_import_map(&info.source_text.as_lossy_str(), &base);
            }
        }
    }
}

/// Parsea `text` (el HTML ya decodificado de `url`) ejecutando sus scripts con `engine`.
pub fn parse_with_scripts(
    text: &str,
    url: &Url,
    engine: &mut Engine,
    loader: std::sync::Arc<CachedLoader>,
) -> Document {
    let opts = ParseOptions {
        scripting: true,
        iframe_srcdoc: false,
    };
    let mut parser = Parser::new(&opts);
    parser.document_mut().url = url.clone();
    engine.set_document(url, loader.clone());
    let mut host = Scripts::new(engine, loader);
    parser.feed(text, &mut host);
    parser.finish(&mut host);
    // Lo que quede pendiente (scripts async ya listos, "the end").
    parser.run_tasks(&mut host);
    parser.into_document().0
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

    fn loader() -> std::sync::Arc<CachedLoader> {
        std::sync::Arc::new(CachedLoader::new(std::sync::Arc::new(
            bf_net::NetLoader::new(),
        )))
    }

    fn texts(log: &Rc<RefCell<Vec<ConsoleEntry>>>) -> Vec<String> {
        log.borrow().iter().map(|e| e.text.clone()).collect()
    }

    #[test]
    fn inline_scripts_run_in_order() {
        let (mut engine, log) = engine_with_log();
        let url = Url::parse("https://example.test/", None).unwrap();
        parse_with_scripts(
            "<script>console.log('uno')</script><p>x<script>console.log('dos', 1 + 1)</script>",
            &url,
            &mut engine,
            loader(),
        );
        assert_eq!(texts(&log), ["uno", "dos 2"]);
        assert_eq!(engine.executed, 2);
    }

    #[test]
    fn errors_go_to_console() {
        let (mut engine, log) = engine_with_log();
        let url = Url::parse("https://example.test/", None).unwrap();
        parse_with_scripts(
            "<script>noExiste()</script><script>console.log('sigue')</script>",
            &url,
            &mut engine,
            loader(),
        );
        let t = texts(&log);
        assert!(t[0].starts_with("Uncaught ReferenceError"), "{t:?}");
        assert_eq!(t[1], "sigue");
        assert_eq!(engine.failed, 1);
    }

    #[test]
    fn inline_module_and_data_url_external_script() {
        let (mut engine, log) = engine_with_log();
        let url = Url::parse("https://example.test/", None).unwrap();
        parse_with_scripts(
            "<script type=module>console.log('m', import.meta.url.startsWith('https://example.test/'))</script>\
             <script src=\"data:text/javascript,console.log('externo')\"></script>\
             <script type=text/plain>console.log('no')</script>",
            &url,
            &mut engine,
            loader(),
        );
        assert_eq!(texts(&log), ["externo", "m true"]);
    }

    #[test]
    fn module_imports_through_import_map() {
        let (mut engine, log) = engine_with_log();
        let url = Url::parse("https://example.test/", None).unwrap();
        parse_with_scripts(
            "<script type=importmap>{\"imports\": {\"lib\": \"data:text/javascript,export const x = 42;\"}}</script>\
             <script type=module>import { x } from 'lib'; console.log('x', x)</script>",
            &url,
            &mut engine,
            loader(),
        );
        assert_eq!(texts(&log), ["x 42"]);
    }

    #[test]
    fn timers_run_as_tasks() {
        let (mut engine, log) = engine_with_log();
        let url = Url::parse("https://example.test/", None).unwrap();
        parse_with_scripts(
            "<script>setTimeout(function(){ console.log('timer') }, 0); console.log('sync')</script>",
            &url,
            &mut engine,
            loader(),
        );
        assert_eq!(texts(&log), ["sync"]);
        assert!(engine.next_wake().is_some());
        engine.run_due_tasks();
        assert_eq!(texts(&log), ["sync", "timer"]);
    }

    #[test]
    fn import_map_prefix() {
        let base = Url::parse("https://a.test/", None).unwrap();
        let map = ImportMap {
            imports: vec![(
                "lib/".to_owned(),
                Url::parse("https://cdn.test/lib/", Some(&base)).unwrap(),
            )],
        };
        assert_eq!(
            map.resolve("lib/x.js").map(|u| u.href()).as_deref(),
            Some("https://cdn.test/lib/x.js")
        );
        assert!(map.resolve("otro").is_none());
    }
}
