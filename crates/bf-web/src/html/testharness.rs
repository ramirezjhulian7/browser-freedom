//! El runner de testharness.js (WPT `resources/testharness.js`): qué archivos son tests, los
//! envoltorios de `*.any.js` / `*.window.js` (como los genera `wptserve`, con sus `// META:`),
//! la carga de cada documento con el parser y el [`PageHost`], el bucle de eventos con un
//! reloj virtual y los resultados de cada subtest.
//!
//! Los resultados llegan por el `testharnessreport.js` del runner (WPT deja que cada
//! navegador ponga el suyo, [`TESTHARNESS_REPORT`]): un `add_completion_callback` que llama a
//! la función global `__freedom_report`.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bf_html::{ParseOptions, Parser};
use bf_infra::Url;
use bf_infra::encoding::Encoding;
use bf_infra::resource::FileLoader;
use bf_js::host::{ConsoleLevel, HostHooks, Runtime};
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, AgentOptions, JsResult, Throw};

use super::scripting::{PageResources, with_parser};
use crate::{DocumentOwner, WebContext};

/// El origen desde el que se sirven los tests (`tests/wpt` montado).
pub const ORIGIN: &str = "http://web-platform.test:8000";

/// E3: el origen de los tests `.https.` (los mismos archivos; sin TLS de verdad: el runner
/// carga del disco). Solo con `FREEDOM_WPT_HTTPS=1` (ver [`https_enabled`]).
pub const HTTPS_ORIGIN: &str = "https://web-platform.test:8443";

/// E3: ¿corre el runner los tests `.https.` (en [`HTTPS_ORIGIN`])? Opcional
/// (`FREEDOM_WPT_HTTPS=1`) para no cambiar la cuenta de los demás directorios hasta que se
/// decida activarlo para todos.
pub fn https_enabled() -> bool {
    std::env::var("FREEDOM_WPT_HTTPS").is_ok_and(|v| v == "1")
}

/// El origen desde el que se sirve el test `rel`.
fn origin_for(rel: &str) -> &'static str {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    if name.contains(".https.") {
        HTTPS_ORIGIN
    } else {
        ORIGIN
    }
}

/// Un documento de test: la ruta de WPT (con la variante), su URL y su HTML.
#[derive(Debug, Clone)]
pub struct TestDoc {
    pub path: String,
    pub url: String,
    pub html: String,
    /// `<meta name=timeout content=long>` o `META: timeout=long`: el límite es 6 veces
    /// mayor (como en WPT: 60 s en vez de 10 s).
    pub long: bool,
    /// El encoding del documento (E10): el de BOM, `Content-Type` del `.headers` o
    /// `<meta charset>`; UTF-8 si no declara ninguno.
    pub encoding: Encoding,
}

/// El límite de un documento: `limit`, o 6 veces `limit` si pide el timeout largo.
pub fn doc_limit(doc: &TestDoc, limit: std::time::Duration) -> std::time::Duration {
    if doc.long { limit * 6 } else { limit }
}

/// ¿El HTML tiene `<meta name="timeout" content="long">`?
fn has_long_timeout(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.match_indices("<meta").any(|(i, _)| {
        let tag = &lower[i..lower[i..].find('>').map_or(lower.len(), |e| i + e)];
        let norm: String = tag.chars().filter(|c| *c != '"' && *c != '\'').collect();
        norm.contains("name=timeout") && norm.contains("content=long")
    })
}

/// El estado de un subtest (`Test.statuses` de testharness.js).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubStatus {
    Pass,
    Fail,
    Timeout,
    NotRun,
    PreconditionFailed,
}

impl SubStatus {
    fn from_code(c: &str) -> SubStatus {
        match c {
            "0" => SubStatus::Pass,
            "1" => SubStatus::Fail,
            "2" => SubStatus::Timeout,
            "4" => SubStatus::PreconditionFailed,
            _ => SubStatus::NotRun,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SubStatus::Pass => "PASS",
            SubStatus::Fail => "FAIL",
            SubStatus::Timeout => "TIMEOUT",
            SubStatus::NotRun => "NOTRUN",
            SubStatus::PreconditionFailed => "PRECONDITION_FAILED",
        }
    }
}

/// Cómo terminó el archivo (el *harness status*, o lo que impidió obtenerlo).
#[derive(Debug, Clone, PartialEq)]
pub enum Harness {
    Ok,
    /// `ERROR` del harness (una excepción fuera de los tests, p. ej.).
    Error(String),
    /// `TIMEOUT` del harness.
    Timeout,
    PreconditionFailed(String),
    /// Nunca se llamó al completion callback (el límite de tiempo del archivo, o el bucle
    /// quedó sin nada que hacer).
    NoResult(String),
    /// Pánico del motor.
    Crash(String),
}

impl Harness {
    pub fn label(&self) -> &'static str {
        match self {
            Harness::Ok => "OK",
            Harness::Error(_) => "ERROR",
            Harness::Timeout => "TIMEOUT",
            Harness::PreconditionFailed(_) => "PRECONDITION_FAILED",
            Harness::NoResult(_) => "NORESULT",
            Harness::Crash(_) => "CRASH",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Harness::Error(m)
            | Harness::PreconditionFailed(m)
            | Harness::NoResult(m)
            | Harness::Crash(m) => m,
            _ => "",
        }
    }
}

/// El resultado de un documento de test.
#[derive(Debug, Clone)]
pub struct DocResult {
    pub path: String,
    pub harness: Harness,
    pub subtests: Vec<(String, SubStatus, String)>,
}

// --- Convenciones de WPT ---

/// ¿`rel` es un test de testharness.js que el runner sabe correr?
pub fn is_test_file(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let support = rel.starts_with("resources/")
        || rel.contains("/resources/")
        || rel.contains("/support/")
        || name.starts_with('_')
        || name.contains("-ref.")
        || name.contains("-notref.")
        || name.contains("-expected.");
    // `.sub.` (sustituciones del servidor), HTTPS y HTTP/2 necesitan wptserve.
    let server = name.contains(".sub.")
        || (name.contains(".https.") && !https_enabled())
        || name.contains(".h2.");
    if support || server {
        return false;
    }
    if name.ends_with(".any.js") || name.ends_with(".window.js") {
        return true;
    }
    // E3: un test que corre en un worker dedicado (wptserve le genera la página).
    if name.ends_with(".worker.js") {
        return true;
    }
    name.ends_with(".html") || name.ends_with(".htm")
}

/// ¿Un HTML usa testharness.js? (Los demás son reftests, crashtests o manuales.)
fn uses_testharness(html: &str) -> bool {
    html.contains("/resources/testharness.js")
}

/// Las líneas `// META: clave=valor` del principio de un `.any.js`.
pub fn meta_lines(src: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in src.lines() {
        let t = line.trim();
        match t.strip_prefix("// META:") {
            Some(rest) => {
                if let Some((k, v)) = rest.trim().split_once('=') {
                    out.push((k.trim().to_owned(), v.trim().to_owned()));
                }
            }
            None if t.is_empty() => {}
            None => break,
        }
    }
    out
}

/// ¿Las globals de `META: global=` incluyen `window`? (Sin la línea: `window` y
/// `dedicatedworker`.)
fn includes_window(metas: &[(String, String)]) -> bool {
    let globals: Vec<&str> = metas
        .iter()
        .filter(|(k, _)| k == "global")
        .flat_map(|(_, v)| v.split(','))
        .map(str::trim)
        .collect();
    globals.is_empty() || globals.iter().any(|g| matches!(*g, "window" | "default"))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
}

/// Los documentos de un test: un HTML tal cual, o el envoltorio de un `.any.js` /
/// `.window.js` por cada `META: variant=` (ninguno si no corre en `window`).
pub fn test_docs(root: &Path, rel: &str) -> Vec<TestDoc> {
    let Ok(bytes) = std::fs::read(root.join(rel)) else {
        return Vec::new();
    };
    let src = String::from_utf8_lossy(&bytes).into_owned();
    let is_any = rel.ends_with(".any.js");
    // E3: `x.worker.js` corre en un worker dedicado desde `x.worker.html`.
    if rel.ends_with(".worker.js") && !is_any {
        let base = rel.trim_end_matches(".js");
        let name = rel.rsplit('/').next().unwrap_or(rel);
        return vec![TestDoc {
            path: format!("{base}.html"),
            url: format!("{}/{base}.html", origin_for(rel)),
            html: worker_page(&format!("new Worker({:?})", name)),
            long: meta_lines(&src)
                .iter()
                .any(|(k, v)| k == "timeout" && v == "long"),
            encoding: Encoding::Utf8,
        }];
    }
    if !(is_any || rel.ends_with(".window.js")) {
        if !uses_testharness(&src) {
            return Vec::new();
        }
        // E10: el encoding como lo vería un navegador servido por wptserve (BOM, cabecera
        // `Content-Type` del `.headers`, prescan de `<meta>`); sin declaración, UTF-8.
        let (encoding, src) = match document_encoding(root, rel, &bytes) {
            Some(e) => (e, bf_infra::encoding::decode(&bytes, e).0),
            None => (Encoding::Utf8, src),
        };
        return vec![TestDoc {
            path: rel.to_owned(),
            url: format!("{}/{rel}", origin_for(rel)),
            long: has_long_timeout(&src),
            html: src,
            encoding,
        }];
    }
    let metas = meta_lines(&src);
    let long = metas.iter().any(|(k, v)| k == "timeout" && v == "long");
    let base = rel.trim_end_matches(".js");
    let script_name = rel.rsplit('/').next().unwrap_or(rel);
    let mut variants: Vec<String> = metas
        .iter()
        .filter(|(k, _)| k == "variant")
        .map(|(_, v)| v.clone())
        .collect();
    if variants.is_empty() {
        variants.push(String::new());
    }
    // E3: las variantes en workers (`.any.worker.html`, `.any.sharedworker.html`).
    let mut docs = Vec::new();
    if is_any {
        let worker_js = format!("{}.worker.js", script_name.trim_end_matches(".js"));
        for (global, page, ctor) in [
            ("dedicatedworker", "worker", "Worker"),
            ("sharedworker", "sharedworker", "SharedWorker"),
        ] {
            if !includes_global(&metas, global) {
                continue;
            }
            for v in &variants {
                docs.push(TestDoc {
                    path: format!("{base}.{page}.html{v}"),
                    url: format!("{}/{base}.{page}.html{v}", origin_for(rel)),
                    html: worker_page(&format!("new {ctor}({:?})", format!("{worker_js}{v}"))),
                    long,
                    encoding: Encoding::Utf8,
                });
            }
        }
    }
    if is_any && !includes_window(&metas) {
        return docs;
    }
    let mut html = String::from("<!doctype html>\n<meta charset=utf-8>\n");
    if let Some((_, t)) = metas.iter().find(|(k, _)| k == "title") {
        html.push_str(&format!("<title>{}</title>\n", html_escape(t)));
    }
    if is_any {
        html.push_str(
            "<script>\nself.GLOBAL = {\n  isWindow: function() { return true; },\n  \
             isWorker: function() { return false; },\n  \
             isShadowRealm: function() { return false; },\n};\n</script>\n",
        );
    }
    html.push_str("<script src=\"/resources/testharness.js\"></script>\n");
    html.push_str("<script src=\"/resources/testharnessreport.js\"></script>\n");
    for (k, v) in &metas {
        if k == "script" {
            html.push_str(&format!("<script src=\"{}\"></script>\n", html_escape(v)));
        }
    }
    html.push_str("<div id=log></div>\n");
    html.push_str(&format!(
        "<script src=\"{}\"></script>\n",
        html_escape(script_name)
    ));
    docs.extend(variants.into_iter().map(|v| TestDoc {
        path: format!("{base}.html{v}"),
        url: format!("{}/{base}.html{v}", origin_for(rel)),
        html: html.clone(),
        long,
        encoding: Encoding::Utf8,
    }));
    docs
}

/// E10: el encoding declarado de un `.html` de WPT (HTML §13.2.3.1 sin el default del
/// locale): BOM, `charset` del `Content-Type` de `<archivo>.headers` o `<meta>` (prescan).
fn document_encoding(root: &Path, rel: &str, bytes: &[u8]) -> Option<Encoding> {
    let headers = std::fs::read_to_string(root.join(format!("{rel}.headers"))).ok();
    let content_type = headers.as_deref().and_then(|h| {
        h.lines().find_map(|l| {
            let (name, value) = l.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-type")
                .then(|| bf_net::MimeType::parse(value.trim()))
                .flatten()
        })
    });
    match bf_net::html_encoding_with_source(bytes, content_type.as_ref()) {
        (_, bf_net::EncodingSource::Default) => None,
        (e, _) => Some(e),
    }
}

/// E3: ¿Las globals de `META: global=` incluyen `global` (`dedicatedworker` o
/// `sharedworker`)? `worker` las incluye a todas; sin la línea, `window` y
/// `dedicatedworker`.
fn includes_global(metas: &[(String, String)], global: &str) -> bool {
    let globals: Vec<&str> = metas
        .iter()
        .filter(|(k, _)| k == "global")
        .flat_map(|(_, v)| v.split(','))
        .map(str::trim)
        .collect();
    if globals.is_empty() {
        return global == "dedicatedworker";
    }
    globals.iter().any(|g| *g == global || *g == "worker")
}

/// E3: la página de un test que corre en un worker (como la genera wptserve).
fn worker_page(worker: &str) -> String {
    format!(
        "<!doctype html>\n<meta charset=utf-8>\n\
         <script src=\"/resources/testharness.js\"></script>\n\
         <script src=\"/resources/testharnessreport.js\"></script>\n\
         <div id=log></div>\n<script>\nfetch_tests_from_worker({worker});\n</script>\n"
    )
}

/// E3: el `x.any.worker.js` que genera wptserve para `x.any.js` (`any_path`, la ruta del
/// `.any.js` en el servidor; `src`, su texto).
pub fn any_worker_script(src: &str, any_path: &str) -> String {
    let mut out = String::from(
        "self.GLOBAL = {\n  isWindow: function() { return false; },\n  \
         isWorker: function() { return true; },\n  \
         isShadowRealm: function() { return false; },\n};\n\
         importScripts(\"/resources/testharness.js\");\n",
    );
    for (k, v) in meta_lines(src) {
        if k == "script" {
            out.push_str(&format!("importScripts({v:?});\n"));
        }
    }
    out.push_str(&format!("importScripts({any_path:?});\ndone();\n"));
    out
}

/// El `testharnessreport.js` del runner.
pub const TESTHARNESS_REPORT: &str = r#"// testharnessreport.js de Freedom: los resultados van al runner.
(function () {
  function clean(s) {
    return String(s === undefined || s === null ? "" : s).replace(/[\t\r\n]+/g, " ");
  }
  setup({ output: false });
  add_completion_callback(function (tests, status) {
    var out = ["HARNESS\t" + status.status + "\t" + clean(status.message)];
    for (var i = 0; i < tests.length; i++) {
      var t = tests[i];
      out.push("TEST\t" + t.status + "\t" + clean(t.name) + "\t" + clean(t.message));
    }
    __freedom_report(out.join("\n"));
  });
})();
"#;

// --- Ejecución ---

/// Los hooks del runner: un reloj virtual (el bucle salta al próximo temporizador cuando no
/// hay nada que hacer) y las excepciones sin atrapar.
struct TestHooks {
    clock: Rc<Cell<f64>>,
    errors: Rc<RefCell<Vec<String>>>,
}

impl HostHooks for TestHooks {
    fn now(&mut self) -> f64 {
        self.clock.get()
    }

    fn console(&mut self, _level: ConsoleLevel, _message: &str) {}

    fn report_exception(&mut self, message: &str, _stack: &str) {
        self.errors.borrow_mut().push(message.to_owned());
    }
}

/// Lo que llegó por `__freedom_report`.
#[derive(Default)]
struct Report(Option<String>);

fn freedom_report(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = agent.to_string(&a.arg(0))?.to_string_lossy();
    agent.set_embedder_data(Report(Some(s)));
    Ok(Value::Undefined)
}

/// Corre un documento de test con `root` (la raíz de WPT) montado en [`ORIGIN`].
pub fn run_doc(root: &Path, doc: &TestDoc, limit: Duration) -> DocResult {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_doc_inner(root, doc, limit)
    }));
    match r {
        Ok(r) => r,
        Err(p) => {
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_default();
            DocResult {
                path: doc.path.clone(),
                harness: Harness::Crash(msg),
                subtests: Vec::new(),
            }
        }
    }
}

/// E4: duración de un cuadro del reloj virtual, en ms.
const FRAME_MS: f64 = 16.0;

fn run_doc_inner(root: &Path, doc: &TestDoc, limit: Duration) -> DocResult {
    let deadline = Instant::now() + limit;
    let clock = Rc::new(Cell::new(0.0));
    let errors = Rc::new(RefCell::new(Vec::new()));
    let hooks = TestHooks {
        clock: clock.clone(),
        errors: errors.clone(),
    };
    let mut rt = Runtime::with_options(
        Box::new(hooks),
        AgentOptions {
            max_stack: 64 << 20,
            max_frames: 10_000,
            instruction_budget: None,
            deadline: Some(deadline),
        },
    );
    let realm = rt.main_realm();
    let result = |harness: Harness, subtests| DocResult {
        path: doc.path.clone(),
        harness,
        subtests,
    };
    let Ok(url) = Url::parse(&doc.url, None) else {
        return result(Harness::Crash(format!("URL inválida: {}", doc.url)), vec![]);
    };
    let mut parser = Parser::new(&ParseOptions {
        scripting: true,
        iframe_srcdoc: false,
        allow_declarative_shadow_roots: true,
    });
    parser.document_mut().url = url.clone();
    parser.document_mut().character_set = doc.encoding.name().into();
    // E4: los `<iframe>` del documento demoran su `load` (HTML §4.8.5, "iframe load event
    // steps"; §8.4 "the end"): se suelta después de cargar los marcos.
    parser.delay_load_event();
    crate::install(
        &mut rt.agent,
        realm,
        WebContext::new(DocumentOwner::Parser(Box::new(parser)), url),
    );
    let g = rt.agent.realms[realm].global_object;
    bf_js::builtins::method(&mut rt.agent, g, "__freedom_report", 1, freedom_report);
    let origin = Url::parse(&format!("{ORIGIN}/"), None).expect("origen");
    let https = Url::parse(&format!("{HTTPS_ORIGIN}/"), None).expect("origen");
    let loader = crate::net::wptserve::SubLoader(
        FileLoader::new()
            .mount(&origin, root.to_path_buf())
            .mount(&https, root.to_path_buf()),
    );
    let mut resources = PageResources::new(Arc::new(loader));
    for o in [ORIGIN, HTTPS_ORIGIN] {
        resources.overrides.insert(
            format!("{o}/resources/testharnessreport.js"),
            TESTHARNESS_REPORT.to_owned(),
        );
    }
    // E1: wptserve sirve `/resources/WebIDLParser.js` como alias de la copia de webidl2 de WPT
    // (`resources/webidl2/lib/webidl2.js`; en el repo no hay un archivo con ese nombre). Sin él,
    // los `idlharness` terminan en `WebIDL2 is not defined`.
    if let Ok(webidl2) = std::fs::read_to_string(root.join("resources/webidl2/lib/webidl2.js")) {
        for o in [ORIGIN, HTTPS_ORIGIN] {
            resources
                .overrides
                .insert(format!("{o}/resources/WebIDLParser.js"), webidl2.clone());
        }
    }
    rt.agent.set_realm_data(realm, resources);
    // E3: el runner vacía la bandeja de los workers y los espera en tiempo real.
    crate::workers::set_manual_pump(&mut rt.agent);
    // Los documentos corren en paralelo con el mismo origen: los `BroadcastChannel` de cada
    // uno (y de sus workers) no deben oír a los de los otros.
    crate::workers::set_partition(&mut rt.agent, crate::workers::new_partition());

    with_parser(&mut rt.agent, realm, |p, host| {
        p.feed(&doc.html, host);
        p.finish(host);
    });
    // E4: los `<iframe>` (HTML §4.8.5), como en el hilo de la pestaña: tras el parseo y cada
    // vez que el bucle queda sin trabajo (srcdoc, src servido por wptserve, about:blank, el
    // `load` del elemento).
    crate::bom::frames::sync_frames(&mut rt.agent, realm);
    with_parser(&mut rt.agent, realm, |p, host| p.release_load_event(host));
    // El bucle de eventos hasta el completion callback.
    loop {
        if rt
            .agent
            .embedder_data::<Report>()
            .is_some_and(|r| r.0.is_some())
        {
            break;
        }
        let terminated = rt
            .agent
            .realm_data::<PageResources>(realm)
            .is_some_and(|r| r.terminated);
        if terminated || Instant::now() > deadline {
            return result(Harness::NoResult("tiempo agotado".into()), vec![]);
        }
        crate::workers::pump(&mut rt.agent); // E3
        match rt.agent.run_until_idle() {
            Ok(_) => {}
            Err(Throw::Terminate(_)) => {
                return result(Harness::NoResult("tiempo agotado".into()), vec![]);
            }
            Err(Throw::Value(_)) => {}
        }
        with_parser(&mut rt.agent, realm, |p, host| p.run_tasks(host));
        if crate::bom::frames::sync_frames(&mut rt.agent, realm) {
            continue;
        }
        if rt
            .agent
            .embedder_data::<Report>()
            .is_some_and(|r| r.0.is_some())
        {
            break;
        }
        // E3: mientras haya workers trabajando, se esperan sus mensajes en tiempo real y el
        // reloj virtual avanza lo que pasó de verdad (sin pasar del próximo temporizador).
        if crate::workers::busy(&rt.agent) {
            let start = Instant::now();
            crate::workers::wait_for_events(
                &rt.agent,
                (start + Duration::from_millis(20)).min(deadline),
            );
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            let next = rt.agent.next_wake().unwrap_or(f64::INFINITY);
            clock.set((clock.get() + elapsed).min(next.max(clock.get())));
            continue;
        }
        // E4: "update the rendering" (HTML §8.1.7.3): con callbacks de `requestAnimationFrame`,
        // el reloj virtual salta al próximo cuadro (cada 16 ms) si llega antes que el próximo
        // temporizador, y se corren con esa hora.
        // También hay cuadro con animaciones o transiciones CSS en curso, eventos de animación
        // pendientes o un documento cambiado ("update animations and send events").
        if rt.agent.has_animation_frame_callbacks()
            || crate::bom::animation::wants_frame(&rt.agent, realm, true)
        {
            let frame = (clock.get() / FRAME_MS).floor() * FRAME_MS + FRAME_MS;
            if rt.agent.next_wake().is_none_or(|t| frame <= t) {
                clock.set(frame);
                crate::bom::animation::update_and_send_events(&mut rt.agent, realm);
                if let Err(Throw::Terminate(_)) = rt.agent.run_animation_frames(frame) {
                    return result(Harness::NoResult("tiempo agotado".into()), vec![]);
                }
                continue;
            }
        }
        match rt.agent.next_wake() {
            Some(t) if t > clock.get() => clock.set(t),
            Some(_) => {}
            None => {
                let msg = errors.borrow().first().cloned().unwrap_or_default();
                return result(
                    Harness::NoResult(format!("sin completion callback: {msg}")),
                    vec![],
                );
            }
        }
    }
    let text = rt
        .agent
        .embedder_data::<Report>()
        .and_then(|r| r.0.clone())
        .unwrap_or_default();
    parse_report(&doc.path, &text)
}

/// Las líneas de [`TESTHARNESS_REPORT`].
fn parse_report(path: &str, text: &str) -> DocResult {
    let mut harness = Harness::NoResult("sin estado".into());
    let mut subtests = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        match f.as_slice() {
            ["HARNESS", code, msg @ ..] => {
                let msg = msg.first().copied().unwrap_or("").to_owned();
                harness = match *code {
                    "0" => Harness::Ok,
                    "2" => Harness::Timeout,
                    "3" => Harness::PreconditionFailed(msg),
                    _ => Harness::Error(msg),
                };
            }
            ["TEST", code, name, rest @ ..] => subtests.push((
                (*name).to_owned(),
                SubStatus::from_code(code),
                rest.first().copied().unwrap_or("").to_owned(),
            )),
            _ => {}
        }
    }
    DocResult {
        path: path.to_owned(),
        harness,
        subtests,
    }
}
