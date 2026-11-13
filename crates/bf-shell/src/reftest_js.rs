//! Reftests de WPT que necesitan JavaScript (roadmap 2.3).
//!
//! El harness de `bf-paint` ([`bf_paint::reftest`]) salta los reftests con `<script>` o con
//! atributos `on*` (en el test o en una referencia), porque `bf-paint` no puede depender del
//! motor de JavaScript. Este runner los ejecuta: carga cada documento con el motor web
//! (`bf-web` sobre `bf-js`), sirviendo `tests/wpt` como `wptserve` (igual que el runner de
//! testharness, [`bf_web::html::testharness::ORIGIN`]), corre su bucle de eventos con un reloj
//! virtual, espera a que la raíz pierda `class="reftest-wait"` y pinta con la misma tubería que
//! la ventana (`bf_paint::prepare_page_animated`). La comparación y la lectura de metadatos son
//! las de `bf-paint` ([`bf_paint::reftest::judge`], [`bf_paint::reftest::meta`]).
//!
//! Protocolo de WPT para los reftests (`docs/writing-tests/reftests.md`, *Controlling When
//! Comparison Occurs*): la captura se toma después de `load`; si la raíz tiene la clase
//! `reftest-wait`, se dispara `TestRendered` en ella tras el primer pintado y se espera a que
//! un script quite la clase. Un pánico o un tiempo agotado cuentan como `error`.
//!
//! Los documentos XML (`.xht`, `.svg`) ejecutan sus scripts durante el parseo, en orden de
//! documento, por el mismo camino que la ventana ([`crate::xml_scripts`]).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bf_dom::Document;
use bf_dom::events::{Event, EventInit, EventTarget};
use bf_infra::encoding::Encoding;
use bf_infra::resource::FileLoader;
use bf_infra::{ResourceLoader, Url};
use bf_js::host::{ConsoleLevel, HostHooks, Runtime};
use bf_js::vm::{Agent, AgentOptions, RealmId, Throw};
use bf_paint::reftest::{self, Harness, Meta, RefError, Status, VIEWPORT};
use bf_paint::{FrameDoc, PageContext, Pixmap};
use bf_style::values::color::Rgba;
use bf_web::html::testharness::{HTTPS_ORIGIN, ORIGIN};
use bf_web::{DocumentOwner, WebContext};

/// Límite de tiempo real por documento (el de un reftest de WPT es 10 s).
pub const LIMIT: Duration = Duration::from_secs(10);

/// Hasta dónde avanza el reloj virtual esperando a que se quite `reftest-wait`.
const VIRTUAL_LIMIT_MS: f64 = 60_000.0;

/// Un cuadro del reloj virtual (como el runner de testharness).
const FRAME_MS: f64 = 16.0;

/// Los 13 directorios de la fase 1 (tabla "Punto de partida" del roadmap).
pub const PHASE1: &[&str] = &[
    "css/CSS2/box-display",
    "css/CSS2/margin-padding-clear",
    "css/CSS2/normal-flow",
    "css/CSS2/floats",
    "css/CSS2/floats-clear",
    "css/CSS2/positioning",
    "css/CSS2/linebox",
    "css/CSS2/text",
    "css/CSS2/colors",
    "css/CSS2/borders",
    "css/css-backgrounds",
    "css/css-flexbox",
    "css/selectors",
];

/// Qué es un archivo de test para este runner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Sin `rel=match`/`mismatch`: testharness.js o manual (no es de este runner).
    NoRef,
    /// Reftest sin script en el test ni en las referencias: lo corre el harness de `bf-paint`.
    Static,
    /// Reftest con script (en el test o en una referencia): lo corre este runner.
    Js,
}

/// El resultado de un archivo.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub test: String,
    pub group: String,
    pub class: Class,
    /// Solo para [`Class::Js`]: `Pass`, `Fail` o `Error`.
    pub status: Option<Status>,
    pub detail: String,
}

/// Hooks del motor con el reloj virtual del documento.
struct Hooks {
    clock: Rc<Cell<f64>>,
}

impl HostHooks for Hooks {
    fn now(&mut self) -> f64 {
        self.clock.get()
    }

    fn console(&mut self, _level: ConsoleLevel, _message: &str) {}

    fn report_exception(&mut self, _message: &str, _stack: &str) {}
}

/// El layout síncrono del CSSOM (`offsetWidth`, `getBoundingClientRect`…) con las fuentes
/// del harness y los tamaños de las imágenes del primer layout.
struct Layout {
    fonts: bf_font::Fonts,
    images: Rc<RefCell<bf_layout::ImageSizes>>,
}

impl bf_web::cssom::LayoutProvider for Layout {
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

fn is_xml(path: &Path) -> bool {
    path.extension().is_some_and(|e| {
        ["xht", "xhtml", "svg"]
            .iter()
            .any(|x| e.eq_ignore_ascii_case(x))
    })
}

/// ¿La raíz del documento tiene la clase `reftest-wait`?
fn waiting(doc: &Document) -> bool {
    doc.document_element()
        .and_then(|r| doc.element(r))
        .and_then(|e| e.attr("class"))
        .is_some_and(|c| c.split_ascii_whitespace().any(|t| t == "reftest-wait"))
}

fn document(agent: &Agent, realm: RealmId) -> Document {
    agent
        .realm_data::<WebContext>(realm)
        .map(|c| c.document().clone())
        .unwrap_or_default()
}

/// El documento que se pinta: con los valores que los scripts pusieron en los controles (el
/// *dirty value*, HTML §4.10.5.4) en el atributo `value` o el texto del `textarea`, como hace
/// el hilo de la pestaña.
fn render_copy(agent: &Agent, realm: RealmId) -> Document {
    let mut doc = document(agent, realm);
    let Some(states) = agent.realm_data::<bf_web::html::forms::ControlStates>(realm) else {
        return doc;
    };
    for (n, st) in &states.0 {
        let Some(value) = &st.value else { continue };
        if n.doc != 0 || !doc.is_connected(n.node) {
            continue;
        }
        let Some(el) = doc.element(n.node) else {
            continue;
        };
        if el.is_html("textarea") {
            let t = doc.create_text_node(value.as_str().into());
            doc.replace_all(Some(t), n.node);
        } else if el.is_html("input") {
            let _ = doc.set_attribute(n.node, &"value".into(), value.as_str().into());
        }
    }
    doc
}

/// Los `<iframe>` de la ventana para pintarlos dentro de su caja.
fn frames(agent: &Agent, realm: RealmId) -> Vec<FrameDoc> {
    fn convert(views: Vec<bf_web::bom::frames::FrameView>) -> Vec<FrameDoc> {
        views
            .into_iter()
            .map(|v| FrameDoc {
                node: v.element,
                doc: v.doc,
                base: v.base,
                encoding: Encoding::Utf8,
                scroll: (0.0, 0.0),
                frames: convert(v.children),
            })
            .collect()
    }
    convert(bf_web::bom::frames::frame_views(agent, realm))
}

/// El runner: el harness de `bf-paint` (fuentes con Ahem, rutas, imágenes de los fallos) y
/// el loader de `tests/wpt` con las sustituciones de `wptserve`.
pub struct Runner {
    harness: Harness,
    loader: Arc<dyn ResourceLoader>,
    /// Límite de tiempo real por documento.
    pub limit: Duration,
    /// Referencias usadas por varios tests: se pintan una sola vez.
    cache: Mutex<HashMap<PathBuf, Arc<Pixmap>>>,
    shared_refs: HashMap<PathBuf, usize>,
}

impl Runner {
    pub fn new(wpt: PathBuf, out_dir: Option<PathBuf>) -> Runner {
        let harness = Harness::new(wpt, out_dir);
        let root = harness.wpt.clone();
        let origin = |o: &str| Url::parse(&format!("{o}/"), None).expect("origen");
        let loader = bf_web::net::wptserve::SubLoader(
            FileLoader::new()
                .mount(&origin(ORIGIN), root.clone())
                .mount(&origin(HTTPS_ORIGIN), root.clone())
                .mount(&reftest::wpt_origin(), root),
        );
        Runner {
            harness,
            loader: Arc::new(loader),
            limit: LIMIT,
            cache: Mutex::default(),
            shared_refs: HashMap::new(),
        }
    }

    /// La URL de un archivo de WPT, como la sirve `wptserve` (`.https.` en el origen HTTPS).
    fn url(&self, path: &Path) -> Url {
        let rel = self.harness.test_name(path);
        let name = rel.rsplit('/').next().unwrap_or(&rel);
        let origin = if name.contains(".https.") {
            HTTPS_ORIGIN
        } else {
            ORIGIN
        };
        Url::parse(&format!("{origin}/{rel}"), None)
            .unwrap_or_else(|_| Url::parse(&format!("{origin}/"), None).expect("origen"))
    }

    /// Carga `path` con sus scripts, espera a que esté listo para la captura y lo pinta.
    pub fn render(&self, path: &Path) -> Result<Pixmap, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("no se pudo leer: {e}"))?;
        let text = bf_infra::decode_text(&bytes);
        let url = self.url(path);
        let deadline = Instant::now() + self.limit;
        let clock = Rc::new(Cell::new(0.0));
        let mut rt = Runtime::with_options(
            Box::new(Hooks {
                clock: clock.clone(),
            }),
            AgentOptions {
                max_stack: crate::script::JS_STACK,
                max_frames: 10_000,
                instruction_budget: None,
                deadline: Some(deadline),
            },
        );
        let realm = rt.main_realm();
        let xml = is_xml(path);
        let agent = &mut rt.agent;
        bf_web::install(
            agent,
            realm,
            WebContext::new(DocumentOwner::Document(Box::default()), url.clone()),
        );
        bf_web::bom::script::install_scripting(agent, realm, &url, self.loader.clone());
        bf_web::cssom::set_viewport(agent, realm, VIEWPORT.width, VIEWPORT.height, 1.0);
        let images = Rc::new(RefCell::new(bf_layout::ImageSizes::default()));
        let fonts = self.harness.fonts();
        bf_web::cssom::set_layout_provider(
            agent,
            realm,
            Box::new(Layout {
                fonts: bf_font::Fonts::new(fonts.clone()),
                images: images.clone(),
            }),
        );
        if xml {
            // Como la ventana: los scripts corren durante la construcción del documento.
            let parsed = reftest::parse_text(path, &text);
            crate::xml_scripts::parse(agent, realm, &parsed, &url);
        } else {
            bf_web::bom::script::begin_parse(agent, realm, &url);
            bf_web::bom::script::feed(agent, realm, &text);
            bf_web::bom::script::finish(agent, realm);
        }
        bf_web::bom::frames::sync_frames(agent, realm);
        let base = url.clone();
        let ctx = PageContext {
            base: &base,
            encoding: Encoding::Utf8,
            loader: self.loader.clone(),
            fonts,
            extra: &[],
            text_zoom: 1.0,
            // Los reftests de WPT se capturan con DPR 1.
            device_pixel_ratio: 1.0,
        };
        // Como el hilo de la pestaña: el primer layout carga imágenes y hojas, y después sale
        // `load` (HTML §13.2.7 paso 8); los scripts ven el tamaño de las imágenes.
        let first = bf_paint::prepare_page(&document(agent, realm), &ctx, VIEWPORT);
        *images.borrow_mut() = first.images.sizes.clone();
        if xml {
            crate::xml_scripts::fire_load(agent, realm);
        } else {
            bf_web::bom::script::resources_loaded(agent, realm);
        }
        self.settle(&mut rt, realm, &clock, deadline)?;
        let agent = &mut rt.agent;
        let doc = render_copy(agent, realm);
        let frames = frames(agent, realm);
        let canvases: Vec<_> = bf_web::html::canvas::snapshot(agent, realm)
            .into_iter()
            .map(|(n, _, p)| (n, p))
            .collect();
        let page = bf_paint::prepare_page_animated(
            &doc,
            &ctx,
            VIEWPORT,
            &canvases,
            &frames,
            &mut |styles| bf_web::bom::animation::animate_in(agent, realm, styles.clone()),
        );
        Ok(bf_paint::render(
            &page.display_list,
            VIEWPORT,
            1.0,
            Rgba::WHITE,
        ))
    }

    /// El bucle de eventos hasta el momento de la captura: tareas y microtasks, marcos,
    /// `TestRendered` y, con `reftest-wait`, el reloj virtual avanzando de temporizador en
    /// temporizador (y de cuadro en cuadro con `requestAnimationFrame`) hasta que se quita la
    /// clase.
    fn settle(
        &self,
        rt: &mut Runtime,
        realm: RealmId,
        clock: &Rc<Cell<f64>>,
        deadline: Instant,
    ) -> Result<(), String> {
        let timeout = || Err("tiempo agotado".to_owned());
        let mut rendered = false;
        let mut idle_frames = 0;
        loop {
            if Instant::now() > deadline {
                return timeout();
            }
            for _ in 0..2 {
                if let Err(Throw::Terminate(_)) = rt.agent.run_until_idle() {
                    return timeout();
                }
                bf_web::html::scripting::with_parser(&mut rt.agent, realm, |p, host| {
                    p.run_tasks(host);
                });
            }
            if rt
                .agent
                .realm_data::<bf_web::html::scripting::PageResources>(realm)
                .is_some_and(|r| r.terminated)
            {
                return timeout();
            }
            if bf_web::bom::frames::sync_frames(&mut rt.agent, realm) {
                continue;
            }
            let doc = document(&rt.agent, realm);
            let raf = rt.agent.has_animation_frame_callbacks();
            if !waiting(&doc) {
                // Sin `reftest-wait` la captura sigue a `load`; los callbacks de
                // `requestAnimationFrame` pendientes corren antes (como mucho dos cuadros).
                if raf && idle_frames < 2 {
                    idle_frames += 1;
                    if self.frame(rt, realm, clock).is_err() {
                        return timeout();
                    }
                    continue;
                }
                return Ok(());
            }
            if !rendered {
                // WPT: `TestRendered` en la raíz después del primer pintado.
                rendered = true;
                if let Some(root) = doc.document_element() {
                    let mut ev = Event::new(
                        "TestRendered",
                        EventInit {
                            bubbles: true,
                            cancelable: false,
                            composed: false,
                        },
                    );
                    bf_web::bom::script::dispatch_trusted(
                        &mut rt.agent,
                        realm,
                        EventTarget::Node(root),
                        &mut ev,
                    );
                }
                continue;
            }
            if clock.get() > VIRTUAL_LIMIT_MS {
                return Err("reftest-wait sigue puesto tras 60 s virtuales".into());
            }
            // E4: con callbacks de `requestAnimationFrame` o animaciones CSS, el reloj salta al
            // próximo cuadro si llega antes que el próximo temporizador.
            if raf || bf_web::bom::animation::wants_frame(&rt.agent, realm, true) {
                let frame = (clock.get() / FRAME_MS).floor() * FRAME_MS + FRAME_MS;
                if rt.agent.next_wake().is_none_or(|t| frame <= t) {
                    clock.set(frame);
                    if self.frame(rt, realm, clock).is_err() {
                        return timeout();
                    }
                    continue;
                }
            }
            match rt.agent.next_wake() {
                Some(t) if t > clock.get() => clock.set(t),
                Some(_) => {}
                None => return Err("reftest-wait sigue puesto y no queda nada pendiente".into()),
            }
        }
    }

    /// Un cuadro: animaciones CSS y callbacks de `requestAnimationFrame` con la hora actual.
    fn frame(&self, rt: &mut Runtime, realm: RealmId, clock: &Rc<Cell<f64>>) -> Result<(), ()> {
        bf_web::bom::animation::update_and_send_events(&mut rt.agent, realm);
        match rt.agent.run_animation_frames(clock.get()) {
            Err(Throw::Terminate(_)) => Err(()),
            _ => Ok(()),
        }
    }

    /// Pinta una referencia (cacheada si la usan varios tests).
    fn reference(&self, path: &Path) -> Result<Arc<Pixmap>, RefError> {
        if let Some(p) = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(path)
        {
            return Ok(p.clone());
        }
        if !path.is_file() {
            return Err(RefError::Other(format!(
                "falta la referencia {}",
                path.display()
            )));
        }
        let pixmap = Arc::new(self.render(path).map_err(RefError::Other)?);
        if self.shared_refs.get(path).copied().unwrap_or(0) >= 3 {
            self.cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(path.to_path_buf(), pixmap.clone());
        }
        Ok(pixmap)
    }

    /// La clase de un test y sus metadatos.
    pub fn classify(&self, path: &Path) -> (Class, Option<Meta>) {
        let Some((doc, _)) = reftest::parse_file(path) else {
            return (Class::NoRef, None);
        };
        let m = reftest::meta(&doc);
        if !m.is_reftest() {
            return (Class::NoRef, Some(m));
        }
        let ref_script = m.matches.iter().chain(&m.mismatches).any(|h| {
            self.harness
                .resolve(h, path)
                .and_then(|p| reftest::parse_file(&p))
                .is_some_and(|(d, _)| reftest::meta(&d).has_script)
        });
        let class = if m.has_script || ref_script {
            Class::Js
        } else {
            Class::Static
        };
        (class, Some(m))
    }

    /// Corre un test (un pánico cuenta como `error`).
    pub fn run_one(&self, path: &Path, group: &str) -> Outcome {
        let test = self.harness.test_name(path);
        let (class, m) =
            catch_unwind(AssertUnwindSafe(|| self.classify(path))).unwrap_or((Class::NoRef, None));
        let mut outcome = Outcome {
            test: test.clone(),
            group: group.to_owned(),
            class,
            status: None,
            detail: String::new(),
        };
        let (Class::Js, Some(m)) = (class, m) else {
            return outcome;
        };
        let result = catch_unwind(AssertUnwindSafe(|| -> Result<(Status, String), String> {
            let actual = self.render(path)?;
            let v = reftest::judge(&m, &actual, &mut |href| {
                let p = self
                    .harness
                    .resolve(href, path)
                    .ok_or_else(|| RefError::Other(format!("referencia no resoluble {href}")))?;
                self.reference(&p)
            })?;
            if let Some(expected) = &v.failed_match {
                self.harness.save_failure(&test, &actual, expected);
            }
            Ok((v.status, v.detail))
        }));
        let (status, detail) = match result {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => (Status::Error, e),
            Err(_) => (Status::Error, format!("pánico: {}", reftest::last_panic())),
        };
        outcome.status = Some(status);
        outcome.detail = detail;
        outcome
    }

    /// Corre los tests de `groups` (filtrados por `filter`) con `threads` hilos.
    pub fn run(
        &mut self,
        groups: &[&str],
        filter: Option<&str>,
        threads: usize,
        progress: &(dyn Fn(usize, usize) + Sync),
    ) -> Vec<Outcome> {
        reftest::install_panic_hook();
        let mut tests: Vec<(PathBuf, String)> = Vec::new();
        for g in groups {
            let mut files = Vec::new();
            reftest::collect_files(&self.harness.wpt.join(g), &mut files);
            files.sort();
            for f in files {
                let rel = f.to_string_lossy().replace('\\', "/");
                if filter.is_none_or(|flt| rel.contains(flt)) {
                    tests.push((f, (*g).to_owned()));
                }
            }
        }
        let mut counts: HashMap<PathBuf, usize> = HashMap::new();
        for (t, _) in &tests {
            if let Ok(Some((doc, _))) = catch_unwind(|| reftest::parse_file(t)) {
                let m = reftest::meta(&doc);
                for h in m.matches.iter().chain(&m.mismatches) {
                    if let Some(p) = self.harness.resolve(h, t) {
                        *counts.entry(p).or_default() += 1;
                    }
                }
            }
        }
        self.shared_refs = counts;
        let next = AtomicUsize::new(0);
        let done = AtomicUsize::new(0);
        let results: Mutex<Vec<Outcome>> = Mutex::new(Vec::with_capacity(tests.len()));
        let this: &Runner = self;
        std::thread::scope(|scope| {
            for _ in 0..threads.max(1) {
                std::thread::Builder::new()
                    .stack_size(crate::script::THREAD_STACK * 2)
                    .spawn_scoped(scope, || {
                        loop {
                            let i = next.fetch_add(1, Ordering::SeqCst);
                            let Some((path, group)) = tests.get(i) else {
                                break;
                            };
                            let o = this.run_one(path, group);
                            results.lock().unwrap_or_else(|e| e.into_inner()).push(o);
                            let d = done.fetch_add(1, Ordering::SeqCst) + 1;
                            if d.is_multiple_of(100) || d == tests.len() {
                                progress(d, tests.len());
                            }
                        }
                    })
                    .expect("hilo del runner");
            }
        });
        let mut results = results.into_inner().unwrap_or_else(|e| e.into_inner());
        results.sort_by(|a, b| a.test.cmp(&b.test));
        results
    }
}

/// Las cuentas de un grupo: `needs-js` del harness de `bf-paint` (sin referencia + reftests
/// con script) y lo que hizo este runner con los reftests con script.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub no_ref: usize,
    pub js: usize,
    pub pass: usize,
    pub fail: usize,
    pub error: usize,
}

impl Counts {
    pub fn add(&mut self, o: &Outcome) {
        match o.class {
            Class::NoRef => self.no_ref += 1,
            Class::Static => {}
            Class::Js => {
                self.js += 1;
                match o.status {
                    Some(Status::Pass) => self.pass += 1,
                    Some(Status::Fail) => self.fail += 1,
                    _ => self.error += 1,
                }
            }
        }
    }

    /// Los `needs-js` del harness de `bf-paint`.
    pub fn needs_js(&self) -> usize {
        self.no_ref + self.js
    }

    /// Los que se ejecutan de punta a punta (pasan o fallan; los errores no cuentan).
    pub fn executed(&self) -> usize {
        self.pass + self.fail
    }
}

fn pct(a: usize, b: usize) -> String {
    if b == 0 {
        "–".into()
    } else {
        format!("{:.1} %", 100.0 * a as f64 / b as f64)
    }
}

/// Tabla Markdown por directorio.
pub fn summary_table(results: &[Outcome], groups: &[&str]) -> String {
    let mut out = String::from(
        "| Directorio | needs-js | sin referencia | reftests con JS | pass | fail | error | \
         ejecutados / needs-js | ejecutados / reftests con JS | pass / ejecutados |\n\
         |---|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n",
    );
    let row = |name: &str, c: &Counts| {
        format!(
            "| {name} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            c.needs_js(),
            c.no_ref,
            c.js,
            c.pass,
            c.fail,
            c.error,
            pct(c.executed(), c.needs_js()),
            pct(c.executed(), c.js),
            pct(c.pass, c.executed()),
        )
    };
    let mut total = Counts::default();
    for g in groups {
        let mut c = Counts::default();
        for r in results.iter().filter(|r| r.group == *g) {
            c.add(r);
            total.add(r);
        }
        out.push_str(&row(g, &c));
    }
    out.push_str(&row("**Total**", &total));
    out
}

/// `freedom wpt-reftests-js [--wpt <dir>] [--out <dir>]`: corre el runner sobre los grupos de
/// `REFTEST_GROUPS` (separados por comas; `fase1` son los 13 de la fase 1; sin la variable,
/// todos los de `bf_paint::reftest::GROUPS`), con `REFTEST_FILTER` y `REFTEST_THREADS` (3 por
/// defecto). Imprime la tabla y escribe `<out>/results.tsv` con las imágenes de los fallos.
pub fn command(args: &[String]) -> Result<(), String> {
    let opt = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .map(PathBuf::from)
    };
    let wpt = opt("--wpt").unwrap_or_else(|| PathBuf::from("tests/wpt"));
    let out = opt("--out").unwrap_or_else(|| PathBuf::from("target/reftests-js"));
    if !wpt.join("css").is_dir() {
        return Err(format!("no está WPT en {}", wpt.display()));
    }
    let groups_var = std::env::var("REFTEST_GROUPS").ok();
    let groups: Vec<&str> = match groups_var.as_deref().map(str::trim) {
        None | Some("") | Some("todos") => reftest::GROUPS.to_vec(),
        Some("fase1") => PHASE1.to_vec(),
        Some(list) => list.split(',').map(str::trim).collect(),
    };
    let filter = std::env::var("REFTEST_FILTER")
        .ok()
        .filter(|f| !f.is_empty());
    let threads = std::env::var("REFTEST_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let _ = std::fs::remove_dir_all(&out);
    let _ = std::fs::create_dir_all(&out);
    let mut runner = Runner::new(wpt, Some(out.clone()));
    let start = Instant::now();
    let results = runner.run(&groups, filter.as_deref(), threads, &|done, total| {
        eprintln!("reftests con JS: {done}/{total} ({:.0?})", start.elapsed());
    });
    println!("{}", summary_table(&results, &groups));
    let mut tsv = String::from("test\tclase\tstatus\tdetail\n");
    for r in &results {
        let class = match r.class {
            Class::NoRef => "sin-referencia",
            Class::Static => "sin-js",
            Class::Js => "js",
        };
        let status = r.status.map_or("-", Status::name);
        tsv.push_str(&format!("{}\t{class}\t{status}\t{}\n", r.test, r.detail));
    }
    std::fs::write(out.join("results.tsv"), tsv)
        .map_err(|e| format!("no se pudo escribir results.tsv: {e}"))?;
    eprintln!("tiempo total: {:.0?}", start.elapsed());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un WPT sintético con los tests de `files` (ruta relativa, contenido).
    fn wpt(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("freedom-reftest-js-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (rel, text) in files {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        dir
    }

    const GREEN_REF: &str =
        "<!doctype html><div style='width:100px;height:100px;background:green'></div>";

    fn status(root: &Path, rel: &str) -> (Class, Option<Status>, String) {
        let mut runner = Runner::new(root.to_path_buf(), None);
        runner.limit = Duration::from_secs(3);
        let o = runner.run_one(&root.join(rel), "t");
        (o.class, o.status, o.detail)
    }

    /// Solo pasa si el script corre: pinta de verde un cuadro rojo.
    #[test]
    fn script_runs_before_capture() {
        let root = wpt(
            "script",
            &[
                ("t/ref.html", GREEN_REF),
                (
                    "t/a.html",
                    "<!doctype html><link rel=match href=ref.html>\
                     <div id=d style='width:100px;height:100px;background:red'></div>\
                     <script>document.getElementById('d').style.background = 'green';</script>",
                ),
                (
                    "t/b.html",
                    "<!doctype html><link rel=mismatch href=ref.html>\
                     <div id=d style='width:100px;height:100px;background:green'></div>\
                     <script>document.getElementById('d').style.background = 'red';</script>",
                ),
                (
                    "t/onload.html",
                    "<!doctype html><link rel=match href=ref.html>\
                     <body onload=\"document.getElementById('d').style.background = 'green'\">\
                     <div id=d style='width:100px;height:100px;background:red'></div>",
                ),
            ],
        );
        for t in ["t/a.html", "t/b.html", "t/onload.html"] {
            let (class, st, detail) = status(&root, t);
            assert_eq!(
                (class, st),
                (Class::Js, Some(Status::Pass)),
                "{t}: {detail}"
            );
        }
        // El harness de `bf-paint` los cuenta como `needs-js`.
        let mut h = Harness::new(root.clone(), None);
        let r = h.run(&["t"], None, &|_, _| {});
        assert!(
            r.iter()
                .all(|o| o.status == Status::NeedsJs || o.test.ends_with("ref.html"))
        );
    }

    /// `reftest-wait`: la captura espera a que el script (en un temporizador lanzado desde
    /// `TestRendered`) quite la clase.
    #[test]
    fn reftest_wait_and_test_rendered() {
        let root = wpt(
            "wait",
            &[
                ("t/ref.html", GREEN_REF),
                (
                    "t/wait.html",
                    "<!doctype html><html class=reftest-wait><link rel=match href=ref.html>\
                     <div id=d style='width:100px;height:100px;background:red'></div>\
                     <script>\
                       document.documentElement.addEventListener('TestRendered', function () {\
                         setTimeout(function () {\
                           requestAnimationFrame(function () {\
                             document.getElementById('d').style.background = 'green';\
                             document.documentElement.classList.remove('reftest-wait');\
                           });\
                         }, 2000);\
                       });\
                     </script>",
                ),
                (
                    "t/stuck.html",
                    "<!doctype html><html class=reftest-wait><link rel=match href=ref.html>\
                     <div style='width:100px;height:100px;background:green'></div>",
                ),
            ],
        );
        let (class, st, detail) = status(&root, "t/wait.html");
        assert_eq!((class, st), (Class::Js, Some(Status::Pass)), "{detail}");
        // Sin script que quite la clase: error, no un pase.
        let (_, st, detail) = status(&root, "t/stuck.html");
        assert_eq!(st, None, "sin script es un reftest estático: {detail}");
    }

    /// Un script que no termina agota el tiempo y cuenta como `error`; la corrida sigue.
    #[test]
    fn endless_script_is_an_error() {
        let root = wpt(
            "loop",
            &[
                ("t/ref.html", GREEN_REF),
                (
                    "t/loop.html",
                    "<!doctype html><link rel=match href=ref.html><script>for (;;) {}</script>",
                ),
                (
                    "t/stuck.html",
                    "<!doctype html><html class=reftest-wait><link rel=match href=ref.html>\
                     <script>setInterval(function () {}, 1000);</script>",
                ),
            ],
        );
        let (_, st, detail) = status(&root, "t/loop.html");
        assert_eq!(st, Some(Status::Error), "{detail}");
        assert!(detail.contains("tiempo agotado"), "{detail}");
        let (_, st, detail) = status(&root, "t/stuck.html");
        assert_eq!(st, Some(Status::Error), "{detail}");
        assert!(detail.contains("reftest-wait"), "{detail}");
    }

    /// Un `.xht` con script: se ejecuta después del parseo XML.
    #[test]
    fn xhtml_scripts_run() {
        let root = wpt(
            "xht",
            &[
                ("t/ref.html", GREEN_REF),
                (
                    "t/x.xht",
                    "<html xmlns='http://www.w3.org/1999/xhtml'><head>\
                     <link rel='match' href='ref.html'/></head><body>\
                     <div id='d' style='width:100px;height:100px;background:red'></div>\
                     <script>document.getElementById('d').style.background = 'green';</script>\
                     </body></html>",
                ),
            ],
        );
        let (class, st, detail) = status(&root, "t/x.xht");
        assert_eq!((class, st), (Class::Js, Some(Status::Pass)), "{detail}");
    }
}
