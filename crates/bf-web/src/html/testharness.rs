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
use bf_infra::resource::FileLoader;
use bf_js::host::{ConsoleLevel, HostHooks, Runtime};
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, AgentOptions, JsResult, Throw};

use super::scripting::{PageResources, with_parser};
use crate::{DocumentOwner, WebContext};

/// El origen desde el que se sirven los tests (`tests/wpt` montado).
pub const ORIGIN: &str = "http://web-platform.test:8000";

/// Un documento de test: la ruta de WPT (con la variante), su URL y su HTML.
#[derive(Debug, Clone)]
pub struct TestDoc {
    pub path: String,
    pub url: String,
    pub html: String,
    /// `<meta name=timeout content=long>` o `META: timeout=long`: el límite es 6 veces
    /// mayor (como en WPT: 60 s en vez de 10 s).
    pub long: bool,
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
    let server = name.contains(".sub.") || name.contains(".https.") || name.contains(".h2.");
    if support || server {
        return false;
    }
    if name.ends_with(".any.js") || name.ends_with(".window.js") {
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
    if !(is_any || rel.ends_with(".window.js")) {
        if !uses_testharness(&src) {
            return Vec::new();
        }
        return vec![TestDoc {
            path: rel.to_owned(),
            url: format!("{ORIGIN}/{rel}"),
            long: has_long_timeout(&src),
            html: src,
        }];
    }
    let metas = meta_lines(&src);
    if is_any && !includes_window(&metas) {
        return Vec::new();
    }
    let long = metas.iter().any(|(k, v)| k == "timeout" && v == "long");
    let base = rel.trim_end_matches(".js");
    let script_name = rel.rsplit('/').next().unwrap_or(rel);
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
    let mut variants: Vec<String> = metas
        .iter()
        .filter(|(k, _)| k == "variant")
        .map(|(_, v)| v.clone())
        .collect();
    if variants.is_empty() {
        variants.push(String::new());
    }
    variants
        .into_iter()
        .map(|v| TestDoc {
            path: format!("{base}.html{v}"),
            url: format!("{ORIGIN}/{base}.html{v}"),
            html: html.clone(),
            long,
        })
        .collect()
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
    });
    parser.document_mut().url = url.clone();
    crate::install(
        &mut rt.agent,
        realm,
        WebContext::new(DocumentOwner::Parser(Box::new(parser)), url),
    );
    let g = rt.agent.realms[realm].global_object;
    bf_js::builtins::method(&mut rt.agent, g, "__freedom_report", 1, freedom_report);
    let origin = Url::parse(&format!("{ORIGIN}/"), None).expect("origen");
    let loader = FileLoader::new().mount(&origin, root.to_path_buf());
    let mut resources = PageResources::new(Arc::new(loader));
    resources.overrides.insert(
        format!("{ORIGIN}/resources/testharnessreport.js"),
        TESTHARNESS_REPORT.to_owned(),
    );
    rt.agent.set_realm_data(realm, resources);

    with_parser(&mut rt.agent, realm, |p, host| {
        p.feed(&doc.html, host);
        p.finish(host);
    });
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
        match rt.agent.run_until_idle() {
            Ok(_) => {}
            Err(Throw::Terminate(_)) => {
                return result(Harness::NoResult("tiempo agotado".into()), vec![]);
            }
            Err(Throw::Value(_)) => {}
        }
        with_parser(&mut rt.agent, realm, |p, host| p.run_tasks(host));
        if rt
            .agent
            .embedder_data::<Report>()
            .is_some_and(|r| r.0.is_some())
        {
            break;
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
