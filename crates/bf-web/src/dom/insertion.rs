//! Lo que pasa cuando JavaScript conecta nodos al documento: los `script` se preparan y los
//! inline se ejecutan (HTML §4.12.1.1, "prepare the script element", con el *parser document*
//! nulo), y los `img`/`link` piden su recurso al navegador ([`crate::WebHooks::resource_needed`]).
//! Los `script` externos se buscan y se ejecutan con el entorno de scripts de la ventana
//! (`bom::script`, E4) en una tarea; sin ese entorno se piden al shell por el mismo hook.

use bf_dom::events::{EventInit, EventKind, EventTarget};
use bf_dom::script::ScriptType;
use bf_dom::{Namespace, NodeData, NodeId};
use bf_js::vm::{Agent, Throw};

use crate::dom::events::{fire_event, report_exception};
use crate::{DocIndex, ResourceKind, WebContextHandle, with_doc, with_doc_mut};

/// Las raíces que inserta `node`: sus hijos si es un `DocumentFragment` (se vacía al
/// insertarlo), si no él mismo. Se calcula antes de insertar.
pub fn roots_of(agent: &Agent, doc: DocIndex, node: NodeId) -> Vec<NodeId> {
    with_doc(agent, doc, |d| match d.get(node).data {
        NodeData::DocumentFragment => d.child_nodes(node),
        _ => vec![node],
    })
}

/// Después de insertar `roots` en `doc`: scripts, imágenes y hojas de estilo conectados.
pub fn inserted(agent: &mut Agent, doc: DocIndex, roots: &[NodeId]) {
    process(agent, doc, roots, true);
}

/// Después de insertar markup parseado (`innerHTML`…): sus `script` no se ejecutan (el
/// parser de fragmentos los marca *already started*), solo se piden los recursos.
pub fn inserted_markup(agent: &mut Agent, doc: DocIndex, roots: &[NodeId]) {
    process(agent, doc, roots, false);
}

fn process(agent: &mut Agent, doc: DocIndex, roots: &[NodeId], scripts: bool) {
    let mut work = Vec::new();
    with_doc(agent, doc, |d| {
        for &r in roots {
            if !d.is_connected(r) {
                continue;
            }
            // Texto insertado en un `script`: "children changed steps" del script.
            if let Some(p) = d.parent(r)
                && d.element(r).is_none()
                && d.element(p).is_some_and(|e| e.is_html("script"))
            {
                work.push(p);
            }
            for n in d.inclusive_descendants(r) {
                if let Some(e) = d.element(n)
                    && e.ns == Namespace::Html
                    && matches!(&*e.name.to_string(), "script" | "img" | "link")
                {
                    work.push(n);
                }
            }
        }
    });
    for el in work {
        let name = with_doc(agent, doc, |d| {
            d.element(el)
                .map(|e| e.name.to_string())
                .unwrap_or_default()
        });
        match name.as_str() {
            // Los documentos sin browsing context (todos menos el de la ventana) tienen el
            // scripting deshabilitado (HTML §8.1.3.4).
            "script" if scripts && doc == 0 => prepare_script(agent, doc, el),
            "img" => resource(agent, doc, el, "src", ResourceKind::Image),
            "link" => {
                let stylesheet = with_doc(agent, doc, |d| {
                    d.element(el).and_then(|e| e.attr("rel")).is_some_and(|r| {
                        r.split_ascii_whitespace()
                            .any(|t| t.eq_ignore_ascii_case("stylesheet"))
                    })
                });
                if stylesheet {
                    resource(agent, doc, el, "href", ResourceKind::Stylesheet);
                }
            }
            _ => {}
        }
    }
}

/// Pide el recurso del atributo `attr` resuelto contra la URL base del documento.
fn resource(agent: &mut Agent, doc: DocIndex, el: NodeId, attr: &str, kind: ResourceKind) {
    let url = with_doc(agent, doc, |d| {
        let value = d.element(el)?.attr(attr)?.to_owned();
        if value.is_empty() {
            return None;
        }
        bf_infra::Url::parse(&value, Some(&d.base_url())).ok()
    });
    if let Some(url) = url {
        let ctx = WebContextHandle::current(agent);
        ctx.get_mut(agent)
            .hooks
            .resource_needed(&url.serialize(false), kind);
    }
}

/// "Prepare the script element" para un `script` que conectó el DOM (no el parser), y la
/// ejecución de los inline clásicos (§4.12.1.1 paso 34.2 y "execute the script element").
fn prepare_script(agent: &mut Agent, doc: DocIndex, el: NodeId) {
    let Some(info) = with_doc_mut(agent, doc, |d| d.begin_prepare_script(el, true)) else {
        return;
    };
    let module = match info.type_ {
        ScriptType::Classic => false,
        ScriptType::Module => true,
        ScriptType::ImportMap => return,
    };
    let realm = WebContextHandle::current(agent).realm;
    // Con el entorno de scripts de la ventana (E4) se carga y se ejecuta como el parser; sin
    // él (un contexto sin navegador) el recurso se le pide al shell.
    let has_env = crate::bom::script::script_env(agent, realm).is_some();
    if info.src.is_some() {
        let Some(url) = info.url.clone() else {
            // Paso 31.5: `src` vacío o inválido → `error` en el elemento (en una tarea).
            queue_element_event(agent, doc, el, "error");
            return;
        };
        if !has_env {
            WebContextHandle::current(agent)
                .get_mut(agent)
                .hooks
                .resource_needed(&url.serialize(false), ResourceKind::Script);
            return;
        }
        // Los scripts que inserta el DOM son async (*force async*): se buscan y se ejecutan
        // en una tarea, en orden de inserción.
        let charset = info.encoding;
        agent.queue_task(
            bf_js::host::TaskSource::Networking,
            Box::new(move |agent: &mut Agent| {
                let saved = agent.current_realm;
                agent.current_realm = realm;
                match crate::bom::script::fetch_script(agent, realm, &url, charset) {
                    Some((source, final_url)) => {
                        execute(agent, doc, el, &source, &final_url, module, true);
                        fire_simple(agent, doc, el, "load");
                    }
                    None => fire_simple(agent, doc, el, "error"),
                }
                agent.current_realm = saved;
            }),
        );
        return;
    }
    let name = WebContextHandle::current(agent)
        .get(agent)
        .url
        .serialize(false);
    let source = info.source_text.to_string();
    if module {
        if has_env {
            // Un módulo inline está listo cuando carga su grafo: se ejecuta en una tarea.
            agent.queue_task(
                bf_js::host::TaskSource::DomManipulation,
                Box::new(move |agent: &mut Agent| {
                    let saved = agent.current_realm;
                    agent.current_realm = realm;
                    execute(agent, doc, el, &source, &name, true, true);
                    agent.current_realm = saved;
                }),
            );
        }
        return;
    }
    execute(agent, doc, el, &source, &name, false, has_env);
}

/// "Execute the script element" (§4.12.1.3): `currentScript` es el elemento (solo para los
/// clásicos) mientras corre.
fn execute(
    agent: &mut Agent,
    doc: DocIndex,
    el: NodeId,
    source: &str,
    url: &str,
    module: bool,
    with_env: bool,
) {
    let current = (!module).then_some(el);
    let old = with_doc_mut(agent, doc, |d| {
        std::mem::replace(&mut d.scripts_mut().current_script, current)
    });
    if with_env {
        let realm = WebContextHandle::current(agent).realm;
        crate::bom::script::run_script(agent, realm, source, url, module);
    } else {
        let r = agent
            .compile_script_text(source, url)
            .and_then(|code| agent.script_evaluation(code));
        if let Err(Throw::Value(e)) = r {
            report_exception(agent, &e);
        }
    }
    with_doc_mut(agent, doc, |d| d.scripts_mut().current_script = old);
}

/// Dispara un evento simple (sin burbujeo ni cancelable) en el elemento.
fn fire_simple(agent: &mut Agent, doc: DocIndex, el: NodeId, type_: &str) {
    fire_event(
        agent,
        doc,
        EventTarget::Node(el),
        type_,
        EventInit::default(),
        EventKind::Event,
    );
}

/// "Queue an element task" que dispara `type_` en el elemento.
fn queue_element_event(agent: &mut Agent, doc: DocIndex, el: NodeId, type_: &'static str) {
    let realm = WebContextHandle::current(agent).realm;
    agent.queue_task(
        bf_js::host::TaskSource::DomManipulation,
        Box::new(move |agent: &mut Agent| {
            let saved = agent.current_realm;
            agent.current_realm = realm;
            fire_simple(agent, doc, el, type_);
            agent.current_realm = saved;
        }),
    );
}

/// Copia *already started* de los `script` de `old` (en `from`) a sus copias `new` (en `to`)
/// ("cloning steps" del `script`, HTML §4.12.1): las copias entre arenas no lo traen.
pub fn copy_script_state(
    agent: &mut Agent,
    (from, old): (DocIndex, &[NodeId]),
    (to, new): (DocIndex, &[NodeId]),
) {
    let started: Vec<NodeId> = with_doc(agent, from, |d| {
        old.iter()
            .zip(new)
            .filter(|(o, _)| d.script_state(**o).already_started)
            .map(|(_, n)| *n)
            .collect()
    });
    with_doc_mut(agent, to, |d| {
        for n in started {
            d.script_state_mut(n).already_started = true;
        }
    });
}
