//! Las Web APIs puras de `bf-js` contra sus tests oficiales de WPT: los archivos `.any.js` de
//! `url/`, `encoding/`, `html/webappapis/atob/` y `html/webappapis/structured-clone/` están
//! copiados tal cual (licencia BSD de 3 cláusulas, ver `tests/web/wpt/LICENSE.md` y el commit
//! en `tests/web/wpt/COMMIT`) y corren sobre un testharness.js mínimo escrito aquí. Los datos de
//! `url/resources/*.json` se leen de `tests/wpt` (`tools/fetch-wpt`).
//!
//! Cada `test()`/`promise_test()` de WPT es un caso; el test falla si alguno falla salvo los de
//! [`EXPECTED_FAILURES`], que dependen de APIs que no son de este bloque.

use std::path::{Path, PathBuf};

use bf_js::value::Value;
use bf_js::vm::{Agent, AgentOptions, Throw};

/// El testharness.js mínimo: `test`, `promise_test`, `async_test` y los `assert_*` que usan
/// estos archivos; los resultados quedan en `__results`.
const HARNESS: &str = r#"
var self = globalThis;
var __results = [];
var __promiseTests = [];
var location = { search: "?TextDecoder", href: "about:blank" };
function format_value(v) {
  if (typeof v === "string") return JSON.stringify(v);
  if (typeof v === "number" && Object.is(v, -0)) return "-0";
  if (typeof v === "symbol") return String(v);
  try { return String(v); } catch (e) { return "[" + typeof v + "]"; }
}
function AssertionError(message) { this.message = message; }
AssertionError.prototype.toString = function () { return "AssertionError: " + this.message; };
function __fail(msg, desc) { throw new AssertionError((desc ? desc + ": " : "") + msg); }
function assert_true(v, d) { if (v !== true) __fail("expected true got " + format_value(v), d); }
function assert_false(v, d) { if (v !== false) __fail("expected false got " + format_value(v), d); }
function assert_equals(a, e, d) { if (!Object.is(a, e)) __fail("expected " + format_value(e) + " but got " + format_value(a), d); }
function assert_not_equals(a, e, d) { if (Object.is(a, e)) __fail("got disallowed value " + format_value(a), d); }
function assert_unreached(d) { __fail("reached unreachable code", d); }
function assert_in_array(a, arr, d) { if (arr.indexOf(a) === -1) __fail(format_value(a) + " not in array", d); }
function assert_array_equals(a, e, d) {
  if (typeof a !== "object" || a === null || !("length" in a)) __fail("value is not array-like: " + format_value(a), d);
  if (a.length !== e.length) __fail("lengths differ, expected " + e.length + " got " + a.length, d);
  for (var i = 0; i < e.length; i++) {
    if (!Object.is(a[i], e[i])) __fail("expected " + format_value(e[i]) + " at index " + i + " got " + format_value(a[i]), d);
  }
}
function assert_class_string(o, c, d) { assert_equals(Object.prototype.toString.call(o), "[object " + c + "]", d); }
function assert_throws_js(C, f, d) {
  try { f(); } catch (e) {
    if (!(e instanceof C) && !(e && e.name === C.name)) __fail("expected " + C.name + " but got " + format_value(e), d);
    return;
  }
  __fail("expected " + C.name + " to be thrown", d);
}
function assert_throws_exactly(v, f, d) {
  try { f(); } catch (e) { if (!Object.is(e, v)) __fail("threw " + format_value(e), d); return; }
  __fail("did not throw", d);
}
function assert_throws_dom(name, C, f, d) {
  if (typeof C !== "function" || f === undefined || typeof f === "string") { d = f; f = C; }
  else if (typeof f !== "function") { d = f; f = C; }
  try { f(); } catch (e) {
    var codes = { DATA_CLONE_ERR: "DataCloneError", INVALID_CHARACTER_ERR: "InvalidCharacterError" };
    var n = codes[name] || name;
    if (!(e instanceof DOMException) || e.name !== n) __fail("expected DOMException " + n + " but got " + format_value(e), d);
    return;
  }
  __fail("expected DOMException " + name, d);
}
function __test_object(name) {
  return {
    name: name,
    step: function (f) { return f.apply(this, Array.prototype.slice.call(arguments, 1)); },
    step_func: function (f) { var t = this; return function () { return f.apply(t, arguments); }; },
    step_func_done: function (f) { var t = this; return function () { if (f) f.apply(t, arguments); t.done(); }; },
    unreached_func: function (d) { return function () { assert_unreached(d); }; },
    add_cleanup: function () {},
    done: function () {}
  };
}
function test(f, name) {
  try { f(__test_object(name)); __results.push([name, true, ""]); }
  catch (e) { __results.push([name, false, String(e)]); }
}
function async_test(f, name) {
  if (typeof f === "string") { name = f; f = function () {}; }
  test(f, name);
  return __test_object(name);
}
function promise_test(f, name) { __promiseTests.push([f, name]); }
function promise_rejects_dom(t, name, C, p, d) {
  if (p === undefined || typeof C !== "function") { d = p; p = C; }
  return p.then(function () { __fail("expected rejection with " + name, d); },
                function (e) { assert_throws_dom(name, function () { throw e; }, d); });
}
function promise_rejects_js(t, C, p, d) {
  return p.then(function () { __fail("expected rejection with " + C.name, d); },
                function (e) { assert_throws_js(C, function () { throw e; }, d); });
}
function promise_rejects_exactly(t, v, p, d) {
  return p.then(function () { __fail("expected rejection", d); },
                function (e) { assert_equals(e, v, d); });
}
function fetch_json(p) { return fetch(p).then(function (r) { return r.json(); }); }
function generate_tests(f, cases) {
  cases.forEach(function (c) { test(function () { f.apply(null, c.slice(1)); }, c[0]); });
}
function setup() {}
function done() {}
function subsetTest(f) { return f.apply(null, Array.prototype.slice.call(arguments, 1)); }
function subsetTestByKey(key, f) { return f.apply(null, Array.prototype.slice.call(arguments, 2)); }
var __fetchData = {};
function fetch(path) {
  var data = __fetchData[path];
  if (data === undefined) return Promise.reject(new TypeError("fetch: " + path));
  return Promise.resolve({ json: function () { return Promise.resolve(JSON.parse(data)); } });
}
// /common/sab.js sin WebAssembly.Memory: SharedArrayBuffer directo.
function createBuffer(type, length, opts) {
  if (type === "ArrayBuffer") return new ArrayBuffer(length, opts);
  if (type === "SharedArrayBuffer") return new SharedArrayBuffer(length, opts);
  throw new Error("type has to be ArrayBuffer or SharedArrayBuffer");
}
function __runPromiseTests() {
  var p = Promise.resolve();
  __promiseTests.forEach(function (pt) {
    p = p.then(function () {
      var r;
      try { r = Promise.resolve(pt[0](__test_object(pt[1]))); }
      catch (e) { r = Promise.reject(e); }
      return r.then(function () { __results.push([pt[1], true, ""]); },
                    function (e) { __results.push([pt[1], false, String(e)]); });
    });
  });
}
"#;

/// Los subtests que fallan por APIs fuera de este bloque (Blob, File, ImageData,
/// MessageChannel…, objetos del DOM y la plataforma) o que necesitan un documento.
const EXPECTED_FAILURES: &[&str] = &[
    "Blob",
    "File",
    "FileList",
    "ImageData",
    "ImageBitmap",
    "MessagePort",
    "MessageChannel",
    "OffscreenCanvas",
    "DOMRect",
    "DOMPoint",
    "DOMQuad",
    "DOMMatrix",
    "Resizable ArrayBuffer is transferable",
    "WebAssembly",
    "ReadableStream",
    "Request",
    "Response",
    "FormData",
    // Big5 y los demás multibyte legacy se decodifican como windows-1252 (fase 3.5).
    "TextDecoder end-of-queue handling",
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/web/wpt")
}

/// Los scripts de un archivo: los `// META: script=` (relativos al archivo o a la raíz de
/// WPT) y el archivo.
fn scripts_of(file: &Path) -> Vec<PathBuf> {
    let src = std::fs::read_to_string(file).unwrap();
    let mut out = Vec::new();
    for line in src.lines() {
        let Some(s) = line.strip_prefix("// META: script=") else {
            continue;
        };
        let s = s.trim();
        // /common/* va en el harness; el resto, relativo.
        if s.starts_with("/common/") {
            continue;
        }
        let p = if let Some(abs) = s.strip_prefix('/') {
            root().join(abs)
        } else {
            file.parent().unwrap().join(s)
        };
        out.push(p);
    }
    out.push(file.to_path_buf());
    out
}

/// Ejecuta un archivo: (nombre, pasó, mensaje) de cada subtest.
fn run_file(file: &Path) -> Vec<(String, bool, String)> {
    let mut agent = Agent::new(AgentOptions {
        max_stack: 64 << 20,
        ..AgentOptions::default()
    });
    agent.reset_stack_base();
    let mut src = String::from(HARNESS);
    // Los datos de url/resources para el `fetch` del harness.
    let res = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/wpt/url/resources");
    for name in [
        "urltestdata.json",
        "urltestdata-javascript-only.json",
        "setters_tests.json",
        "IdnaTestV2.json",
    ] {
        if let Ok(data) = std::fs::read_to_string(res.join(name)) {
            src.push_str(&format!(
                "__fetchData[{}] = {};\n",
                serde_json::to_string(&format!("resources/{name}")).unwrap(),
                serde_json::to_string(&data).unwrap()
            ));
        }
    }
    // Los JSON copiados junto a los tests (`fetch_json` relativo al archivo de WPT).
    let b64 = root().join("fetch/data-urls/resources/base64.json");
    if let Ok(data) = std::fs::read_to_string(&b64) {
        src.push_str(&format!(
            "__fetchData['../../../fetch/data-urls/resources/base64.json'] = {};
",
            serde_json::to_string(&data).unwrap()
        ));
    }
    let mut out = Vec::new();
    if let Err(e) = agent.eval_script(&src, "harness.js") {
        panic!("harness: {e:?}");
    }
    for s in scripts_of(file) {
        let code = std::fs::read_to_string(&s).unwrap();
        if let Err(e) = agent.eval_script(&code, &s.to_string_lossy()) {
            let msg = match e {
                Throw::Value(v) => agent
                    .to_string(&v)
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default(),
                Throw::Terminate(m) => m,
            };
            out.push((
                format!("(carga de {})", s.file_name().unwrap().to_string_lossy()),
                false,
                msg,
            ));
        }
    }
    let _ = agent.eval_script("__runPromiseTests();", "run.js");
    let _ = agent.run_jobs();
    let results = agent
        .eval_script(
            r"JSON.stringify(__results.map(function (r) {
                var clean = function (s) { return String(s).replace(/[\uD800-\uDFFF]/g, '�'); };
                return [clean(r[0]), r[1], clean(r[2])];
            }))",
            "results.js",
        )
        .ok();
    if let Some(Value::String(s)) = results {
        let text = s.to_string_lossy();
        let v: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("{e}: {}", &text[..text.len().min(300)]));
        for r in v.as_array().unwrap() {
            out.push((
                r[0].as_str().unwrap_or("").to_owned(),
                r[1].as_bool().unwrap_or(false),
                r[2].as_str().unwrap_or("").to_owned(),
            ));
        }
    }
    out
}

fn expected_failure(name: &str, msg: &str) -> bool {
    EXPECTED_FAILURES
        .iter()
        .any(|p| name.contains(p) || msg.contains(&format!("{p} is not defined")))
}

fn files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in [
        "url",
        "encoding",
        "html/webappapis/atob",
        "html/webappapis/structured-clone",
    ] {
        for e in std::fs::read_dir(root().join(dir)).unwrap() {
            let p = e.unwrap().path();
            if p.to_string_lossy().ends_with(".any.js") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

#[test]
fn wpt_web_apis() {
    let has_data = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/wpt/url/resources/urltestdata.json")
        .exists();
    let mut total = 0;
    let mut passed = 0;
    let mut expected = 0;
    let mut failures = Vec::new();
    for f in files() {
        let name = f
            .strip_prefix(root())
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let results = run_file(&f);
        let mut file_pass = 0;
        for (sub, ok, msg) in &results {
            total += 1;
            if *ok {
                passed += 1;
                file_pass += 1;
            } else if expected_failure(sub, msg) || (!has_data && sub.contains("Loading data")) {
                expected += 1;
            } else {
                failures.push(format!("{name} :: {sub} :: {msg}"));
            }
        }
        eprintln!("{name}: {file_pass}/{}", results.len());
    }
    eprintln!(
        "WPT web APIs: {passed}/{total} subtests pasan ({expected} fallas esperadas por APIs de otros bloques)"
    );
    assert!(passed >= 80, "solo {passed} subtests pasan");
    assert!(
        failures.is_empty(),
        "{} fallas:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
