//! E1: el diagnóstico de una caída de la pestaña ("Esta página falló").
//!
//! El hilo de la pestaña captura los pánicos (`window/worker.rs`, `catch_unwind`) y antes solo
//! mostraba el mensaje (p. ej. "el realm no tiene WebContext"), sin saber dónde ni con qué
//! script. Ahora el detalle de la página de error y del log dice:
//! - **dónde** cayó Rust: archivo, línea y columna del pánico. Un hook de pánico del proceso
//!   ([`install_hook`]) los guarda para el hilo que hace el pánico;
//! - **qué corría el motor**: el tipo de realm en curso (`bf_web::realm_kind`) y la pila de
//!   JavaScript (`Agent::stack_trace`). Se leen *después* del pánico: el desenrollado de Rust no
//!   saca los frames del intérprete ni restaura `current_realm`, así que el `Agent` sigue
//!   diciendo qué script y qué realm estaban activos.
//!
//! Sin datos del usuario: de las URL de los scripts se quitan la consulta y el fragmento (pueden
//! llevar tokens). La URL de la página ya la muestra la página de error.

use std::any::Any;
use std::cell::RefCell;
use std::sync::Once;

use bf_js::vm::Agent;

/// Cuántos frames de JavaScript se muestran.
const MAX_FRAMES: usize = 8;

thread_local! {
    /// Archivo, línea y columna del último pánico de este hilo.
    static LAST: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Instala (una vez por proceso) el hook de pánico que guarda la ubicación del pánico para el
/// hilo que lo provoca. Encadena el hook anterior, que sigue imprimiendo en stderr.
pub fn install_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if let Some(l) = info.location() {
                let here = format!("{}:{}:{}", l.file(), l.line(), l.column());
                // Sin entrar en pánico dentro del hook (el hilo puede estar terminando).
                let _ = LAST.try_with(|c| {
                    if let Ok(mut c) = c.try_borrow_mut() {
                        *c = Some(here);
                    }
                });
            }
            previous(info);
        }));
    });
}

/// La ubicación del último pánico de este hilo (y se olvida), si su mensaje es `message`.
///
/// El hook de pánico es del proceso y otro lo puede reemplazar: el silencioso de los reftests
/// (`bf_paint::reftest::install_panic_hook`, que usa `freedom wpt-reftests-js` y sus tests en
/// este mismo binario). Ese también guarda, por hilo, "mensaje (archivo:línea)": si el nuestro
/// no corrió, se toma de ahí.
fn take_location(message: &str) -> Option<String> {
    let ours = LAST
        .try_with(|c| c.try_borrow_mut().ok().and_then(|mut c| c.take()))
        .ok()
        .flatten();
    ours.or_else(|| {
        let theirs = bf_paint::reftest::last_panic();
        let rest = theirs.strip_prefix(message)?.strip_prefix(" (")?;
        rest.strip_suffix(')').map(str::to_owned)
    })
}

/// El mensaje de un pánico capturado.
pub fn panic_message(e: &(dyn Any + Send)) -> String {
    e.downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| e.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "error interno".to_owned())
}

/// El detalle de una caída: el mensaje, dónde fue en Rust y qué corría el motor (`agent`, si
/// la pestaña tenía uno). Va a la página "Esta página falló" y al log.
pub fn report(e: &(dyn Any + Send), agent: Option<&Agent>) -> String {
    let mut out = panic_message(e);
    if let Some(at) = take_location(&out) {
        out.push_str("\nen ");
        out.push_str(&at);
    }
    out.push('\n');
    match agent {
        Some(a) => out.push_str(&engine_context(a)),
        None => out.push_str("Sin motor de JavaScript en la pestaña."),
    }
    out
}

/// Qué ejecutaba el motor: el realm en curso y la pila de JavaScript.
pub fn engine_context(agent: &Agent) -> String {
    let realm = agent.current_realm;
    let mut out = format!("Realm {realm}: {}", bf_web::realm_kind(agent, realm));
    let stack = agent.stack_trace(MAX_FRAMES);
    if stack.is_empty() {
        out.push_str("\nSin JavaScript en curso.");
    } else {
        out.push_str("\nJavaScript en curso:");
        for line in stack.lines() {
            out.push('\n');
            out.push_str(&without_query(line));
        }
    }
    out
}

/// Una línea `    at f (url:línea:columna)` sin la consulta ni el fragmento de la URL.
fn without_query(line: &str) -> String {
    let (Some(open), true) = (line.rfind('('), line.ends_with(')')) else {
        return line.to_owned();
    };
    let inner = &line[open + 1..line.len() - 1];
    // `url:línea:columna`: lo que queda tras los dos últimos `:` es la posición.
    let mut parts = inner.rsplitn(3, ':');
    let (Some(col), Some(row), Some(url)) = (parts.next(), parts.next(), parts.next()) else {
        return line.to_owned();
    };
    let url = url.split(['?', '#']).next().unwrap_or(url);
    format!("{}({url}:{row}:{col})", &line[..open])
}

/// Para los tests: un pánico dentro de JavaScript, en una función nativa que llama el script de
/// la página (como el de "el realm no tiene WebContext", que salía del código del host).
#[cfg(test)]
pub mod test_panic {
    use bf_js::value::{PropertyKey, Value};
    use bf_js::vm::function::CallArgs;
    use bf_js::vm::{Agent, JsResult};

    /// La URL que provoca el pánico en JavaScript al cargarse en una pestaña.
    pub const URL: &str = "about:blank#panico-en-javascript";

    fn panico(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<bf_js::value::Value> {
        panic!("pánico de prueba en una función nativa");
    }

    /// Con [`URL`], corre en `realm` un script que llama a una función nativa que hace pánico.
    pub fn maybe_panic(agent: &mut Agent, realm: bf_js::vm::RealmId, url: &bf_infra::Url) {
        if url.href() != URL {
            return;
        }
        run(agent, realm);
    }

    /// El script de la prueba: `pagina_de_prueba` (en `https://example.test/prueba.js?token=x`)
    /// llama a la función nativa `__panico`.
    pub fn run(agent: &mut Agent, realm: bf_js::vm::RealmId) {
        let saved = agent.current_realm;
        agent.current_realm = realm;
        let f = agent.create_builtin_function(panico, 0, "__panico", false);
        let g = agent.realms[realm].global_object;
        agent.put_data(
            g,
            PropertyKey::from("__panico"),
            Value::Object(f),
            true,
            false,
            true,
        );
        agent.current_realm = saved;
        let src = "function pagina_de_prueba() {\n  __panico();\n}\npagina_de_prueba();\n";
        let code = agent
            .compile_script_text(src, "https://example.test/prueba.js?token=x#f")
            .expect("compila");
        agent.current_realm = realm;
        let _ = agent.script_evaluation(code);
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use super::*;

    #[test]
    fn query_and_fragment_are_dropped() {
        assert_eq!(
            without_query("    at f (https://a.test/x.js?token=1#y:3:7)"),
            "    at f (https://a.test/x.js:3:7)"
        );
        assert_eq!(
            without_query("    at <anonymous> (inline:1:2)"),
            "    at <anonymous> (inline:1:2)"
        );
    }

    /// Un pánico dentro de una función nativa llamada por JavaScript: el informe tiene el
    /// mensaje, el archivo y la línea de Rust, el realm y la pila de JavaScript sin la consulta.
    #[test]
    fn report_has_rust_location_and_javascript_context() {
        install_hook();
        let mut rt = bf_js::host::Runtime::new(Box::new(bf_js::host::DefaultHooks));
        let realm = rt.main_realm();
        let url = bf_infra::Url::parse("https://example.test/", None).unwrap();
        let (doc, _) = bf_html::parse_document("", &bf_html::ParseOptions::default());
        bf_web::install(
            &mut rt.agent,
            realm,
            bf_web::WebContext::new(bf_web::DocumentOwner::Document(Box::new(doc)), url),
        );
        let r = catch_unwind(AssertUnwindSafe(|| test_panic::run(&mut rt.agent, realm)));
        let e = r.expect_err("tenía que hacer pánico");
        let text = report(&*e, Some(&rt.agent));
        for needle in [
            "pánico de prueba en una función nativa",
            "en crates/bf-shell/src/crash.rs:",
            "Realm 0: ventana",
            "JavaScript en curso:",
            "at pagina_de_prueba (https://example.test/prueba.js:2:",
        ] {
            assert!(text.contains(needle), "falta {needle:?} en {text}");
        }
        assert!(!text.contains("token"), "{text}");
    }

    #[test]
    fn report_without_engine() {
        install_hook();
        let r = catch_unwind(|| panic!("sin motor"));
        let text = report(&*r.unwrap_err(), None);
        assert!(
            text.starts_with("sin motor\nen crates/bf-shell/src/crash.rs:"),
            "{text}"
        );
        assert!(
            text.ends_with("Sin motor de JavaScript en la pestaña."),
            "{text}"
        );
    }
}
