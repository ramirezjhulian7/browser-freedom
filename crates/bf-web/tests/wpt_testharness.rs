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
    Harness, SubStatus, TestDoc, doc_limit, is_test_file, meta_lines, run_doc, test_docs,
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
        .map(|d| run_doc(root, d, doc_limit(d, Duration::from_secs(10))))
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

/// E4: el runner corre los callbacks de `requestAnimationFrame` ("update the rendering", HTML
/// §8.1.7.3) avanzando el reloj virtual de cuadro en cuadro.
#[test]
fn animation_frames_run_on_the_virtual_clock() {
    let root = fake_root("raf");
    std::fs::write(
        root.join("t/raf.html"),
        "<!doctype html><script src=/resources/testharness.js></script>\
         <script src=/resources/testharnessreport.js></script>\
         <script>\
           var times = [];\
           function frame(t) {\
             times.push(t);\
             if (times.length < 3) { requestAnimationFrame(frame); return; }\
             test(function () {\
               if (!(times[0] > 0 && times[1] > times[0] && times[2] > times[1]))\
                 throw new Error(times.join());\
             }, 'raf');\
             done();\
           }\
           requestAnimationFrame(frame);\
         </script>",
    )
    .unwrap();
    let r = run(&root, "t/raf.html");
    assert_eq!(r[0].harness, Harness::Ok);
    assert_eq!(r[0].subtests.len(), 1);
    assert_eq!(
        r[0].subtests[0].1,
        SubStatus::Pass,
        "{}",
        r[0].subtests[0].2
    );
}

/// E4: el runner crea los marcos de los `<iframe>` (HTML §4.8.5) como el hilo de la pestaña:
/// `srcdoc`, `src` servido desde la raíz y `about:blank`, con su `load`.
#[test]
fn iframes_get_their_documents() {
    let root = fake_root("frames");
    std::fs::write(
        root.join("t/sub.html"),
        "<title>sub</title><script>var marca = 42;</script>",
    )
    .unwrap();
    std::fs::write(
        root.join("t/frames.html"),
        "<!doctype html><script src=/resources/testharness.js></script>         <script src=/resources/testharnessreport.js></script>         <iframe id=f srcdoc='<p id=x>hola</p>'></iframe>         <iframe id=g src=sub.html></iframe>         <script>           var b = document.createElement('iframe');           document.body.appendChild(b);           test(function () {             if (b.contentDocument.URL !== 'about:blank') throw new Error(b.contentDocument.URL);           }, 'blank');           var n = 0;           function check() {             if (++n < 2) return;             test(function () {               var d = document.getElementById('f').contentDocument;               if (d.getElementById('x').textContent !== 'hola') throw new Error('srcdoc');               var w = document.getElementById('g').contentWindow;               if (w.marca !== 42 || w.document.title !== 'sub') throw new Error('src');             }, 'load');             done();           }           document.getElementById('f').addEventListener('load', check);           document.getElementById('g').addEventListener('load', check);         </script>",
    )
    .unwrap();
    let r = run(&root, "t/frames.html");
    assert_eq!(r[0].harness, Harness::Ok, "{:?}", r[0].subtests);
    let st: Vec<(&str, SubStatus)> = r[0]
        .subtests
        .iter()
        .map(|(n, s, _)| (n.as_str(), *s))
        .collect();
    assert_eq!(
        st,
        [("blank", SubStatus::Pass), ("load", SubStatus::Pass)],
        "{:?}",
        r[0].subtests
    );
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
    // E3: solo la variante en worker dedicado (`.any.worker.html`), sin la de ventana.
    let w: Vec<String> = test_docs(&root, "t/w.any.js")
        .into_iter()
        .map(|d| d.path)
        .collect();
    assert_eq!(w, ["t/w.any.worker.html"], "solo worker");
    // Sin `META: global`: ventana y worker dedicado; aquí se mira la de ventana.
    let docs: Vec<TestDoc> = test_docs(&root, "t/b.any.js");
    let paths: Vec<&str> = docs.iter().map(|d| d.path.as_str()).collect();
    assert_eq!(paths, ["t/b.any.worker.html", "t/b.any.html"]);
    let i = 1;
    assert!(docs[i].html.contains("<title>Un título</title>"));
    let r = run(&root, "t/b.any.js");
    assert_eq!(r[i].harness, Harness::Ok, "{:?}", r[i]);
    assert!(r[i].subtests.iter().all(|(_, s, _)| *s == SubStatus::Pass));
    assert_eq!(r[i].subtests.len(), 2);
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
fn long_timeout() {
    let root = fake_root("long");
    std::fs::write(
        root.join("t/l.html"),
        "<meta name=\"timeout\" content='long'><script src=/resources/testharness.js></script>",
    )
    .unwrap();
    std::fs::write(
        root.join("t/s.html"),
        "<meta name=viewport content=long><script src=/resources/testharness.js></script>",
    )
    .unwrap();
    std::fs::write(
        root.join("t/l.any.js"),
        "// META: timeout=long
done();
",
    )
    .unwrap();
    assert!(test_docs(&root, "t/l.html")[0].long);
    assert!(!test_docs(&root, "t/s.html")[0].long);
    let d = &test_docs(&root, "t/l.any.js")[0];
    assert!(d.long);
    assert_eq!(
        doc_limit(d, Duration::from_secs(10)),
        Duration::from_secs(60)
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
            let r = run_doc(&root, &doc, doc_limit(&doc, Duration::from_secs(10)));
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
