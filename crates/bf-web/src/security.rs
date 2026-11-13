//! E6: las políticas de seguridad del documento de una ventana (Mixed Content y Content
//! Security Policy, [`bf_net::security::DocumentPolicy`]) en los puntos donde el motor web
//! carga o ejecuta: los scripts del parser y los que inserta el DOM, `fetch`/XHR y los
//! documentos de los marcos. Cada bloqueo se anota en la consola con la URL.
//!
//! La política la pone el navegador ([`install`]) con las cabeceras de la respuesta (sin
//! navegador, se crea una al primer chequeo con la URL del documento); las de
//! `<meta http-equiv="Content-Security-Policy">` se agregan cuando el parser las insertó
//! (CSP 3 §3.3: valen desde ese momento, y solo en el `<head>`), revisando el documento antes
//! de cada decisión ([`refresh_meta`]).
//!
//! Cada violación de CSP dispara además `securitypolicyviolation` (CSP 3 §5.3, "Report a
//! violation"), en una tarea: en el elemento que hizo la petición si sigue conectado, si no en
//! el documento (burbujea hasta la ventana). Sin `report-uri` ni `report-to`.

use std::collections::HashSet;
use std::sync::Arc;

use bf_dom::{Document, NodeId};
use bf_infra::{Destination, Url};
use bf_js::host::ConsoleLevel;
use bf_js::vm::{Agent, RealmId};
use bf_net::csp::{Disposition, Inline};
use bf_net::security::{DocumentPolicy, Report};

use crate::{NodeRef, WebContext};

/// La política de la ventana y los `<meta>` ya leídos (dato del realm).
pub struct WindowPolicy {
    pub policy: Arc<DocumentPolicy>,
    seen_meta: HashSet<NodeId>,
}

/// Pone `policy` como la de la ventana `realm`, también para `eval`/`new Function`
/// (HostEnsureCanCompileStrings de `bf-js`).
pub fn install(agent: &mut Agent, realm: RealmId, policy: Arc<DocumentPolicy>) {
    let check = policy.clone();
    agent.set_realm_data(
        realm,
        bf_js::vm::eval::StringCompilationCheck(std::rc::Rc::new(move |agent: &mut Agent| {
            let r = check.check_eval();
            flush(agent, realm);
            r
        })),
    );
    agent.set_realm_data(
        realm,
        WindowPolicy {
            policy,
            seen_meta: HashSet::new(),
        },
    );
}

/// La política de la ventana `realm`, si tiene.
pub fn policy(agent: &Agent, realm: RealmId) -> Option<Arc<DocumentPolicy>> {
    agent
        .realm_data::<WindowPolicy>(realm)
        .map(|w| w.policy.clone())
}

/// La política de la ventana; si todavía no tiene (un motor sin navegador: el runner de
/// testharness o el de reftests), una sin CSP de cabeceras para la URL del documento, para que
/// valgan Mixed Content y los `<meta>` de CSP. `None` sin `WebContext`.
fn ensure(
    agent: &mut Agent,
    realm: RealmId,
    doc: Option<&Document>,
) -> Option<Arc<DocumentPolicy>> {
    if let Some(p) = policy(agent, realm) {
        return Some(p);
    }
    let url = match doc {
        Some(d) => d.url.clone(),
        None => agent
            .realm_data::<WebContext>(realm)?
            .document()
            .url
            .clone(),
    };
    let p = Arc::new(DocumentPolicy::new(url));
    install(agent, realm, p.clone());
    Some(p)
}

/// Los `<meta http-equiv="Content-Security-Policy">` del `<head>` de `doc` (en orden de
/// documento).
pub fn meta_policies(doc: &Document) -> Vec<(NodeId, String)> {
    let Some(head) = doc.head() else {
        return Vec::new();
    };
    doc.inclusive_descendants(head)
        .into_iter()
        .filter_map(|n| {
            let e = doc.element(n)?;
            let csp = e.is_html("meta")
                && e.attr("http-equiv")
                    .is_some_and(|v| v.trim().eq_ignore_ascii_case("content-security-policy"));
            if !csp {
                return None;
            }
            Some((n, e.attr("content")?.to_owned()))
        })
        .collect()
}

/// Agrega a la política las de los `<meta>` que el parser insertó desde la última vez. `doc`
/// es el documento si no está en el contexto (el parser lo tiene mientras prepara un script).
pub fn refresh_meta(agent: &mut Agent, realm: RealmId, doc: Option<&Document>) {
    if agent.realm_data::<WindowPolicy>(realm).is_none() {
        return;
    }
    let metas = match doc {
        Some(d) => meta_policies(d),
        None => match agent.realm_data::<WebContext>(realm) {
            Some(ctx) => meta_policies(ctx.document()),
            None => return,
        },
    };
    let Some(w) = agent.realm_data_mut::<WindowPolicy>(realm) else {
        return;
    };
    for (node, content) in metas {
        if w.seen_meta.insert(node) {
            w.policy.add_meta(&content);
        }
    }
}

/// Los mensajes pendientes de la política, a la consola de la ventana, y sus violaciones
/// como `securitypolicyviolation` en el documento.
pub fn flush(agent: &mut Agent, realm: RealmId) {
    flush_at(agent, realm, None);
}

/// Como [`flush`]; las violaciones van a `element` si sigue conectado al disparar.
fn flush_at(agent: &mut Agent, realm: RealmId, element: Option<NodeRef>) {
    let Some(p) = policy(agent, realm) else {
        return;
    };
    for m in p.take_messages() {
        agent.host.hooks.console(ConsoleLevel::Error, &m);
    }
    for report in p.take_reports() {
        // CSP 3 §5.3 paso 3: "queue a task to run the following steps" (la fuente de tareas
        // de DOM manipulation, como en los navegadores).
        agent.queue_task(
            bf_js::host::TaskSource::DomManipulation,
            Box::new(move |agent: &mut Agent| {
                fire_violation(agent, realm, element, &report);
            }),
        );
    }
}

/// CSP 3 §5.3 pasos 3.1 a 3.3: el destino (el elemento si está conectado; si no, el
/// documento de la ventana) y "fire an event named `securitypolicyviolation` that uses
/// `SecurityPolicyViolationEvent`", que burbujea y es *composed*.
fn fire_violation(agent: &mut Agent, realm: RealmId, element: Option<NodeRef>, r: &Report) {
    use bf_dom::events::{Event, EventInit, EventTarget};
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let Some(ctx) = agent.realm_data::<WebContext>(realm) else {
        agent.current_realm = saved;
        return;
    };
    let document = bf_net::security::strip_for_report(&ctx.url);
    let connected = element
        .filter(|e| (e.doc as usize) < ctx.documents.len() && ctx.doc(e.doc).is_connected(e.node));
    let (doc, node) = match connected {
        Some(e) => (e.doc, e.node),
        None => (0, ctx.doc(0).root()),
    };
    let disposition = match r.disposition {
        Disposition::Enforce => "enforce",
        Disposition::Report => "report",
    };
    let slots = crate::dom::event_types::security_policy_violation_slots(
        &r.blocked,
        disposition,
        &document,
        &r.effective,
        &r.policy,
    );
    let class = crate::dom::classes(agent).security_policy_violation_event;
    let init = EventInit {
        bubbles: true,
        cancelable: false,
        composed: true,
    };
    let mut event = Event::new("securitypolicyviolation", init);
    event.is_trusted = true;
    let o = crate::dom::events::create_event_object(
        agent,
        crate::dom::events::EventIface::Class(class),
        std::rc::Rc::new(std::cell::RefCell::new(event)),
        doc,
    );
    if let Some(s) = agent.host_slots_mut(o) {
        s.extend(slots);
    }
    let pin = agent.host_pin(bf_js::value::Value::Object(o));
    let r = crate::dom::events::dispatch_js_event(agent, doc, EventTarget::Node(node), o, false);
    agent.host_unpin(pin);
    if let Err(bf_js::vm::Throw::Value(e)) = r {
        agent.report_exception(&e);
    }
    agent.current_realm = saved;
}

/// ¿Se puede pedir `url` para `destination` desde la ventana? `Some` con la URL que se pide
/// (quizá pasada a `https:`), `None` si se bloquea (y queda anotado en la consola). Sin
/// política, la misma URL.
pub fn check_request(
    agent: &mut Agent,
    realm: RealmId,
    doc: Option<&Document>,
    url: &Url,
    destination: Destination,
    nonce: Option<&str>,
    element: Option<NodeRef>,
) -> Option<Url> {
    let Some(p) = ensure(agent, realm, doc) else {
        return Some(url.clone());
    };
    refresh_meta(agent, realm, doc);
    let r = p.check_request(url, destination, nonce);
    flush_at(agent, realm, element);
    r
}

/// Como [`check_request`] para un `<script src>` que insertó otro script (no el parser): con
/// `'strict-dynamic'` (CSP 3 §8.2) se permite aunque no tenga nonce.
pub fn check_inserted_script(
    agent: &mut Agent,
    realm: RealmId,
    url: &Url,
    nonce: Option<&str>,
    element: Option<NodeRef>,
) -> Option<Url> {
    let Some(p) = ensure(agent, realm, None) else {
        return Some(url.clone());
    };
    refresh_meta(agent, realm, None);
    let r = p.check_request_by(url, Destination::Script, nonce, false);
    flush_at(agent, realm, element);
    r
}

/// ¿Se puede ejecutar el script en línea `source` con el `nonce` de su elemento?
pub fn allow_inline_script(
    agent: &mut Agent,
    realm: RealmId,
    doc: Option<&Document>,
    nonce: Option<&str>,
    source: &str,
    element: Option<NodeRef>,
) -> bool {
    let Some(p) = ensure(agent, realm, doc) else {
        return true;
    };
    refresh_meta(agent, realm, doc);
    let ok = p.allow_inline(Inline::Script, nonce, source);
    flush_at(agent, realm, element);
    ok
}

/// Como [`allow_inline_script`] para un `<script>` en línea que insertó otro script (no el
/// parser): con `'strict-dynamic'` (CSP 3 §8.2) se permite aunque no tenga nonce.
pub fn allow_inserted_inline_script(
    agent: &mut Agent,
    realm: RealmId,
    nonce: Option<&str>,
    source: &str,
    element: Option<NodeRef>,
) -> bool {
    let Some(p) = ensure(agent, realm, None) else {
        return true;
    };
    refresh_meta(agent, realm, None);
    let ok = p.allow_inline_by(Inline::Script, nonce, source, false);
    flush_at(agent, realm, element);
    ok
}
