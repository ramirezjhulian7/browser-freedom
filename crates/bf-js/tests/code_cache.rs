//! E3: la caché de código compilado (`bf_js::code_cache`): precompilar en otro hilo, copiar
//! el código entre agentes del mismo hilo sin compartir sus inline caches ni sus templates, y
//! los errores de sintaxis informados al ejecutar.

use bf_js::Runtime;
use bf_js::code_cache;
use bf_js::host::DefaultHooks;

/// Un script de más de `MIN_SOURCE_LEN` bytes con propiedades (inline caches), un template
/// con tag y una función anidada; su valor es `"<suma>|<mismo template>"`.
fn script(tag: &str) -> String {
    let mut s = format!("// {tag}\n");
    while s.len() < code_cache::MIN_SOURCE_LEN {
        s.push_str("var o = { a: 1, b: 2 }; o.a = o.a + o.b;\n");
    }
    s.push_str(
        "function t(strings) { return strings; }\n\
         function site() { return t`x${1}y`; }\n\
         (function (n) { var acc = 0; for (var i = 0; i < n; i++) acc += o.a; return acc; })(10)\n\
           + '|' + (site() === site());",
    );
    s
}

fn run(src: &str, url: &str) -> String {
    let mut rt = Runtime::new(Box::new(DefaultHooks));
    let realm = rt.main_realm();
    let agent = &mut rt.agent;
    agent.current_realm = realm;
    let v = agent.eval_script_cached(src, url).expect("script");
    rt.to_display(&v)
}

#[test]
fn warm_from_another_thread_is_used_without_compiling() {
    let url = "https://example.test/warm.js";
    let src = script("warm");
    let (u, s) = (url.to_owned(), src.clone());
    std::thread::spawn(move || code_cache::warm(&u, Some("\"v1\""), &s))
        .join()
        .unwrap();
    assert!(code_cache::is_warm(url, &src));
    let before = code_cache::stats();
    assert_eq!(run(&src, url), "30|true");
    let after = code_cache::stats();
    assert_eq!(after.compiled, before.compiled, "no compila");
    assert_eq!(after.process_hits, before.process_hits + 1);
}

#[test]
fn second_agent_of_the_thread_copies_the_code() {
    let url = "https://example.test/copy.js";
    let src = script("copy");
    let before = code_cache::stats();
    // El primero compila; el segundo (otro agente, con su heap) copia, y sus inline caches
    // empiezan vacías: el resultado es el mismo.
    assert_eq!(run(&src, url), "30|true");
    assert_eq!(run(&src, url), "30|true");
    let after = code_cache::stats();
    assert_eq!(after.compiled, before.compiled + 1);
    assert_eq!(after.thread_hits, before.thread_hits + 1);
}

/// Cada ejecución es otro Parse Node: en el mismo realm, el mismo texto evaluado dos veces da
/// template objects distintos (ECMA-262 §13.2.8.4 GetTemplateObject).
#[test]
fn copies_get_their_own_template_sites() {
    let mut src = script("templates");
    src.push_str(";\nvar first = site();");
    let url = "https://example.test/templates.js";
    let mut rt = Runtime::new(Box::new(DefaultHooks));
    let realm = rt.main_realm();
    rt.agent.current_realm = realm;
    rt.agent.eval_script_cached(&src, url).unwrap();
    rt.agent
        .eval_script_cached("var keep = first;", "https://example.test/keep.js")
        .unwrap();
    rt.agent.eval_script_cached(&src, url).unwrap();
    let v = rt
        .agent
        .eval_script_cached("keep === first", "https://example.test/check.js")
        .unwrap();
    assert_eq!(rt.to_display(&v), "false");
}

#[test]
fn syntax_error_is_reported_when_running() {
    let url = "https://example.test/broken.js";
    let mut src = script("broken");
    src.push_str("\nfunction (");
    // Precompilar no informa nada.
    code_cache::warm(url, None, &src);
    assert!(code_cache::is_warm(url, &src));
    let mut rt = Runtime::new(Box::new(DefaultHooks));
    let realm = rt.main_realm();
    rt.agent.current_realm = realm;
    let err = rt.agent.eval_script_cached(&src, url);
    assert!(err.is_err(), "SyntaxError al ejecutar");
    let e = rt.eval_script(realm, "1", "x.js");
    assert!(e.is_ok());
}

#[test]
fn a_new_validator_replaces_the_old_entry() {
    let url = "https://example.test/versioned.js";
    let old = script("v1");
    let new = script("v2");
    code_cache::warm(url, Some("\"1\""), &old);
    code_cache::warm(url, Some("\"2\""), &new);
    assert!(!code_cache::is_warm(url, &old));
    assert!(code_cache::is_warm(url, &new));
}
