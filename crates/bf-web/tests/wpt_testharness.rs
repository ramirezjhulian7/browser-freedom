//! El runner de testharness.js (`bf_web::html::testharness`).
//!
//! Los tests rápidos usan un WPT sintético (un `testharness.js` mínimo que llama a los
//! completion callbacks desde `done()`) para probar todo el camino: el parser, los scripts con
//! `src` desde la raíz montada, el `testharnessreport.js` del runner y los envoltorios de
//! `.any.js`. El `#[ignore]` corre los directorios reales de `tests/wpt` y exige que ningún
//! archivo termine en pánico.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bf_web::html::testharness::{
    Harness, SubStatus, TestDoc, is_test_file, meta_lines, run_doc, test_docs,
};

/// Un `testharness.js` mínimo: `test()` corre en el acto, `done()` llama a los callbacks.
const FAKE_HARNESS: &str = r#"
var __cbs = [], __tests = [];
function setup() {}
function add_completion_callback(f) { __cbs.push(f); }
function test(f, name) {
  var t = { name: name, status: 0, message: null };
  try { f(); } catch (e) { t.status = 1; t.message = String(e); }
  __tests.push(t);
}
function done() {
  for (var i = 0; i < __cbs.length; i++) __cbs[i](__tests, { status: 0, message: null });
}
"#;

fn fake_root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bf-web-wpt-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("resources")).unwrap();
    std::fs::create_dir_all(dir.join("common")).unwrap();
    std::fs::create_dir_all(dir.join("t")).unwrap();
    std::fs::write(dir.join("resources/testharness.js"), FAKE_HARNESS).unwrap();
    // El de WPT no se usa: el runner sirve el suyo.
    std::fs::write(dir.join("resources/testharnessreport.js"), "throw 1;").unwrap();
    std::fs::write(dir.join("common/helper.js"), "var helperValue = 42;").unwrap();
    dir
}

fn run(root: &Path, rel: &str) -> Vec<bf_web::html::testharness::DocResult> {
    test_docs(root, rel)
        .iter()
        .map(|d| run_doc(root, d, Duration::from_secs(10)))
        .collect()
}

#[test]
fn html_test_with_scripts() {
    let root = fake_root("html");
    std::fs::write(
        root.join("t/a.html"),
        "<!doctype html><title>t</title>\
         <script src=/resources/testharness.js></script>\
         <script src=/resources/testharnessreport.js></script>\
         <script src=../common/helper.js></script>\
         <script>\
           test(function () { if (helperValue !== 42) throw new Error('helper'); }, 'pasa');\
           test(function () { throw new Error('mal\\tformado'); }, 'falla');\
           done();\
         </script>",
    )
    .unwrap();
    let r = run(&root, "t/a.html");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].harness, Harness::Ok);
    let st: Vec<(&str, SubStatus)> = r[0]
        .subtests
        .iter()
        .map(|(n, s, _)| (n.as_str(), *s))
        .collect();
    assert_eq!(st, [("pasa", SubStatus::Pass), ("falla", SubStatus::Fail)]);
    assert!(r[0].subtests[1].2.contains("mal formado"));
}

#[test]
fn any_js_wrapper_and_meta() {
    let root = fake_root("any");
    std::fs::write(
        root.join("t/b.any.js"),
        "// META: title=Un título\n// META: script=/common/helper.js\n\n\
         test(function () { if (!GLOBAL.isWindow()) throw 1; }, 'global');\n\
         test(function () { if (helperValue !== 42) throw 1; }, 'script');\n\
         done();\n",
    )
    .unwrap();
    std::fs::write(
        root.join("t/w.any.js"),
        "// META: global=dedicatedworker\ntest(function () {}, 'x');\n",
    )
    .unwrap();
    assert!(test_docs(&root, "t/w.any.js").is_empty(), "solo worker");
    let docs: Vec<TestDoc> = test_docs(&root, "t/b.any.js");
    assert_eq!(docs[0].path, "t/b.any.html");
    assert!(docs[0].html.contains("<title>Un título</title>"));
    let r = run(&root, "t/b.any.js");
    assert_eq!(r[0].harness, Harness::Ok, "{:?}", r[0]);
    assert!(r[0].subtests.iter().all(|(_, s, _)| *s == SubStatus::Pass));
    assert_eq!(r[0].subtests.len(), 2);
}

#[test]
fn variants_and_no_result() {
    let root = fake_root("var");
    std::fs::write(
        root.join("t/v.window.js"),
        "// META: variant=?a\n// META: variant=?b\ntest(function () {}, location.search);\ndone();\n",
    )
    .unwrap();
    let docs = test_docs(&root, "t/v.window.js");
    let paths: Vec<&str> = docs.iter().map(|d| d.path.as_str()).collect();
    assert_eq!(paths, ["t/v.window.html?a", "t/v.window.html?b"]);
    // Sin completion callback: el bucle queda sin nada que hacer.
    std::fs::write(
        root.join("t/n.html"),
        "<script src=/resources/testharness.js></script><script>test(function(){}, 'x');</script>",
    )
    .unwrap();
    let r = run(&root, "t/n.html");
    assert!(
        matches!(r[0].harness, Harness::NoResult(_)),
        "{:?}",
        r[0].harness
    );
}

#[test]
fn file_selection() {
    assert!(is_test_file("dom/nodes/Node-cloneNode.html"));
    assert!(is_test_file("url/url-constructor.any.js"));
    assert!(is_test_file("dom/x.window.js"));
    assert!(!is_test_file("dom/nodes/resources/helper.html"));
    assert!(!is_test_file("dom/nodes/x-ref.html"));
    assert!(!is_test_file("url/a.sub.html"));
    assert!(!is_test_file("url/a.js"));
    let m =
        meta_lines("// META: global=window,worker\n// META: timeout=long\ncode();\n// META: x=y");
    assert_eq!(m.len(), 2);
    assert_eq!(m[0], ("global".to_owned(), "window,worker".to_owned()));
}

/// Los directorios reales de WPT: ningún archivo puede terminar en pánico.
#[test]
#[ignore]
fn wpt_testharness_no_crashes() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/wpt");
    assert!(
        root.join("resources/testharness.js").exists(),
        "falta tests/wpt: tools/fetch-wpt"
    );
    let mut files = Vec::new();
    fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect(root, &p, out);
            } else if let Ok(rel) = p.strip_prefix(root) {
                let rel = rel.to_string_lossy().replace('\\', "/");
                if is_test_file(&rel) {
                    out.push(rel);
                }
            }
        }
    }
    for d in ["dom/nodes", "html/dom", "url", "encoding"] {
        collect(&root, &root.join(d), &mut files);
    }
    files.sort();
    let mut crashes = Vec::new();
    let (mut docs, mut pass, mut total) = (0, 0, 0);
    for rel in &files {
        for doc in test_docs(&root, rel) {
            let r = run_doc(&root, &doc, Duration::from_secs(10));
            docs += 1;
            total += r.subtests.len();
            pass += r
                .subtests
                .iter()
                .filter(|(_, s, _)| *s == SubStatus::Pass)
                .count();
            if let Harness::Crash(m) = &r.harness {
                crashes.push(format!("{}: {m}", r.path));
            }
        }
    }
    println!("{docs} documentos; subtests: {pass} pass / {total}");
    assert!(crashes.is_empty(), "pánicos:\n{}", crashes.join("\n"));
}
