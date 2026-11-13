//! E3: instalar una ventana con el GC recolectando muy seguido. Los instaladores de cada área
//! crean objetos y después compilan y llaman a sus fábricas de JavaScript (que recolectan);
//! `bf_web::register_all` los corre dentro de un ámbito de enraizado
//! (`Agent::rooting_scope`) para que nada se libere antes de quedar enlazado. Antes, un
//! objeto sin fijar daba un pánico `Gc liberado` (`bf-js/src/heap.rs`) al crear marcos con el
//! heap ya grande.

use bf_js::host::DefaultHooks;
use bf_js::value::Value;
use bf_web::{DocumentOwner, Runtime, Url, WebContext};

fn window_with_threshold(threshold: usize) -> Runtime {
    let mut rt = Runtime::new(Box::new(DefaultHooks));
    rt.agent.heap.set_threshold(threshold);
    let (doc, _) = bf_html::parse_document(
        "<!doctype html><p id=p data-x=1>hola</p>",
        &bf_html::ParseOptions::default(),
    );
    let realm = rt.main_realm();
    let url = Url::parse("https://example.com/", None).expect("url");
    bf_web::install(
        &mut rt.agent,
        realm,
        WebContext::new(DocumentOwner::Document(Box::new(doc)), url),
    );
    rt
}

#[test]
fn installing_a_window_survives_frequent_collections() {
    let mut rt = window_with_threshold(1);
    let collections = rt.agent.gc_stats().collections;
    assert!(collections > 0, "el GC tiene que haber corrido al instalar");
    let realm = rt.main_realm();
    // Lo que instalaron las fábricas (dataset, IndexedDB, streams, storage) funciona.
    let v = rt
        .eval_script(
            realm,
            "[document.getElementById('p').dataset.x, typeof indexedDB.open, \
             typeof new ReadableStream().getReader, typeof localStorage.getItem].join()",
            "uso.js",
        )
        .expect("eval");
    match v {
        Value::String(s) => assert_eq!(s.to_string_lossy(), "1,function,function,function"),
        other => panic!("{other:?}"),
    }
}
