//! E3: el JavaScript propio de las ventanas (streams, IndexedDB, Service Workers, Cache, CSS
//! Font Loading) no se recompila en cada ventana: la segunda del hilo lo copia de la caché de
//! código, y `precompile_window_scripts` lo deja listo desde otro hilo.

use bf_js::code_cache;
use bf_js::host::DefaultHooks;
use bf_web::{DocumentOwner, Runtime, Url, WebContext};

fn window() -> Runtime {
    let mut rt = Runtime::new(Box::new(DefaultHooks));
    let realm = rt.main_realm();
    let url = Url::parse("https://example.test/", None).unwrap();
    bf_web::install(
        &mut rt.agent,
        realm,
        WebContext::new(DocumentOwner::Document(Box::default()), url),
    );
    rt
}

fn check(rt: &mut Runtime) {
    let realm = rt.main_realm();
    let v = rt
        .eval_script(
            realm,
            "[typeof ReadableStream, typeof indexedDB, typeof caches, typeof FontFace].join()",
            "t.js",
        )
        .unwrap();
    assert_eq!(rt.to_display(&v), "function,object,object,function");
}

#[test]
fn second_window_of_the_thread_does_not_recompile() {
    std::thread::spawn(|| {
        let mut first = window();
        check(&mut first);
        let before = code_cache::stats();
        let mut second = window();
        check(&mut second);
        let after = code_cache::stats();
        assert_eq!(
            after.compiled, before.compiled,
            "la segunda ventana no compila"
        );
        assert!(
            after.thread_hits - before.thread_hits >= 5,
            "{before:?} {after:?}"
        );
    })
    .join()
    .unwrap();
}

#[test]
fn precompiled_in_another_thread() {
    std::thread::spawn(bf_web::precompile_window_scripts)
        .join()
        .unwrap();
    // Un hilo nuevo, sin nada compilado: todo sale de la caché del proceso.
    std::thread::spawn(|| {
        let mut rt = window();
        check(&mut rt);
        let s = code_cache::stats();
        assert_eq!(s.compiled, 0, "{s:?}");
        assert!(s.process_hits >= 5, "{s:?}");
    })
    .join()
    .unwrap();
}
