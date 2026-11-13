//! Los scripts de los documentos XML (XHTML, SVG) durante el parseo.
//!
//! HTML §13.4 (*Parsing XHTML documents*): cuando el parser XML crea un `script`, lo marca
//! *parser-inserted*; al parsear su etiqueta de cierre hace un punto de control de microtasks
//! y lo prepara ("prepare the script element", §4.12.1.1), y si queda un *pending
//! parsing-blocking script* el parser espera a que se ejecute. Los scripts ven el documento
//! parseado hasta su `</script>` y lo que escriben en el DOM queda antes de lo que sigue.
//!
//! El parser XML de `bf-html` no cede el control, así que el documento ya parseado se
//! reconstruye nodo a nodo en la ventana, en orden de documento, y cada `script` se prepara y
//! ejecuta al cerrarse, con lo siguiente todavía sin insertar. Es equivalente porque en XML no
//! hay nada que el script pueda reinyectar en el parser: `document.write` lanza en un
//! documento XML (HTML §8.4.3 paso 1). Después vienen los scripts `defer`/`async`,
//! `DOMContentLoaded` y, con [`fire_load`], `load` ("the end", §13.2.7, que también aplica al
//! parser XML).
//!
//! La carga y la ejecución son las de los scripts de la ventana (`bf_web::bom::script`), con
//! `document.currentScript` y `load`/`error` en el elemento de los externos.

use bf_dom::events::{Event, EventInit, EventTarget};
use bf_dom::script::{DocumentReadiness, ScriptType};
use bf_dom::{Document, Namespace, NodeId};
use bf_infra::Url;
use bf_js::vm::{Agent, RealmId};
use bf_web::WebContext;

fn with_doc<R>(agent: &mut Agent, realm: RealmId, f: impl FnOnce(&mut Document) -> R) -> R {
    let ctx = agent
        .realm_data_mut::<WebContext>(realm)
        .expect("la ventana tiene WebContext");
    f(ctx.document_mut())
}

fn simple_event(
    agent: &mut Agent,
    realm: RealmId,
    target: EventTarget,
    type_: &str,
    bubbles: bool,
) {
    let mut ev = Event::new(
        type_,
        EventInit {
            bubbles,
            cancelable: false,
            composed: false,
        },
    );
    bf_web::bom::script::dispatch_trusted(agent, realm, target, &mut ev);
}

/// "Update the current document readiness" (§3.1.3): el valor y `readystatechange`.
fn set_readiness(agent: &mut Agent, realm: RealmId, readiness: DocumentReadiness) {
    let root = with_doc(agent, realm, |d| {
        d.scripts_mut().readiness = readiness;
        d.root()
    });
    simple_event(
        agent,
        realm,
        EventTarget::Node(root),
        "readystatechange",
        false,
    );
}

/// Un script externo o en línea listo para ejecutarse.
struct Ready {
    el: NodeId,
    source: String,
    url: String,
    /// `load` en el elemento después (los externos).
    external: bool,
}

/// "Execute the script element" (§4.12.1.3): `currentScript` es el elemento mientras corre.
fn execute(agent: &mut Agent, realm: RealmId, s: &Ready) {
    let old = with_doc(agent, realm, |d| {
        d.scripts_mut().current_script.replace(s.el)
    });
    bf_web::bom::script::run_script(agent, realm, &s.source, &s.url, false);
    with_doc(agent, realm, |d| d.scripts_mut().current_script = old);
    if s.external {
        simple_event(agent, realm, EventTarget::Node(s.el), "load", false);
    }
}

/// "Prepare the script element" para un `script` que insertó el parser XML. `Some` con lo
/// que hay que ejecutar y si espera al final del parseo (`defer`/`async`).
fn prepare(agent: &mut Agent, realm: RealmId, el: NodeId) -> Option<(Ready, bool)> {
    let (info, svg_href) = with_doc(agent, realm, |d| {
        let e = d.element(el)?;
        // SVG 2 §15.3: el `script` de SVG toma su URL de `href` (o `xlink:href`).
        let svg_href = (e.ns == Namespace::Svg)
            .then(|| {
                e.attrs
                    .iter()
                    .find(|a| a.name.as_lossy_str() == "href")
                    .map(|a| a.value.as_lossy_str().to_owned())
            })
            .flatten();
        d.script_state_mut(el).parser_inserted = true;
        let info = d.begin_prepare_script(el, true);
        Some((info, svg_href))
    })?;
    let doc_url = with_doc(agent, realm, |d| d.url.serialize(false));
    let Some(info) = info else {
        // Un `script` de SVG solo con `href` (sin `src` ni texto): se busca por `href`.
        let href = svg_href?;
        let base = with_doc(agent, realm, |d| {
            d.script_state_mut(el).already_started = true;
            d.base_url()
        });
        let url = Url::parse(href.trim(), Some(&base)).ok()?;
        return fetch(agent, realm, el, &url, "utf-8").map(|r| (r, false));
    };
    // Los módulos y los import maps todavía no se cargan desde un documento XML.
    if info.type_ != ScriptType::Classic {
        return None;
    }
    let later = info.defer || info.is_async;
    if info.src.is_some() {
        let Some(url) = info.url.clone() else {
            simple_event(agent, realm, EventTarget::Node(el), "error", false);
            return None;
        };
        return fetch(agent, realm, el, &url, info.encoding).map(|r| (r, later));
    }
    Some((
        Ready {
            el,
            source: info.source_text.to_string(),
            url: doc_url,
            external: false,
        },
        false,
    ))
}

/// Busca un script externo; si falla, `error` en el elemento.
fn fetch(agent: &mut Agent, realm: RealmId, el: NodeId, url: &Url, charset: &str) -> Option<Ready> {
    match bf_web::bom::script::fetch_script(agent, realm, url, charset) {
        Some((source, url)) => Some(Ready {
            el,
            source,
            url,
            external: true,
        }),
        None => {
            simple_event(agent, realm, EventTarget::Node(el), "error", false);
            None
        }
    }
}

/// ¿`n` es un `script` de HTML o de SVG (los que ejecuta el parser XML)?
fn is_script(doc: &Document, n: NodeId) -> bool {
    doc.element(n).is_some_and(|e| {
        matches!(e.ns, Namespace::Html | Namespace::Svg) && e.name.as_lossy_str() == "script"
    })
}

/// Inserta los hijos de `from` (en `parsed`) bajo `to` (en el documento de la ventana), en
/// orden de documento, ejecutando cada `script` al cerrarse.
fn build(
    agent: &mut Agent,
    realm: RealmId,
    parsed: &Document,
    from: NodeId,
    to: NodeId,
    later: &mut Vec<Ready>,
) {
    for c in parsed.children(from).collect::<Vec<_>>() {
        let data = parsed.get(c).data.clone();
        // El parser inserta en el nodo actual de su pila aunque un script lo haya movido.
        let n = with_doc(agent, realm, |d| {
            let n = d.create(data);
            d.append(to, n);
            n
        });
        build(agent, realm, parsed, c, n, later);
        if is_script(parsed, c) {
            // "Perform a microtask checkpoint" antes de preparar el script.
            let _ = agent.perform_microtask_checkpoint();
            match prepare(agent, realm, n) {
                Some((ready, true)) => later.push(ready),
                Some((ready, false)) => execute(agent, realm, &ready),
                None => {}
            }
        }
    }
}

/// Pone `parsed` (un documento XML ya parseado) como documento de la ventana `realm`,
/// ejecutando sus scripts durante la construcción, y termina con los `defer`/`async` y
/// `DOMContentLoaded`. El `load` lo dispara [`fire_load`] cuando el navegador cargó los
/// recursos del documento.
pub fn parse(agent: &mut Agent, realm: RealmId, parsed: &Document, url: &Url) {
    let mut live = Document::new_xml();
    live.url = url.clone();
    live.content_type = parsed.content_type.clone();
    live.character_set = parsed.character_set.clone();
    live.mode = parsed.mode;
    live.scripts_mut().readiness = DocumentReadiness::Loading;
    let root = live.root();
    with_doc(agent, realm, |d| *d = live);
    let mut later = Vec::new();
    build(agent, realm, parsed, parsed.root(), root, &mut later);
    // "The end" (§13.2.7): `interactive`, los scripts que esperaban al final del parseo y
    // `DOMContentLoaded`.
    set_readiness(agent, realm, DocumentReadiness::Interactive);
    for s in &later {
        execute(agent, realm, s);
    }
    simple_event(
        agent,
        realm,
        EventTarget::Node(root),
        "DOMContentLoaded",
        true,
    );
}

/// "The end" (§13.2.7) pasos 8 y siguientes para un documento XML: `complete` y `load` en
/// la ventana.
pub fn fire_load(agent: &mut Agent, realm: RealmId) {
    set_readiness(agent, realm, DocumentReadiness::Complete);
    simple_event(agent, realm, EventTarget::Window, "load", false);
}
