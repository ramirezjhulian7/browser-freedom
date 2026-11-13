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

use std::collections::HashSet;
use std::sync::Arc;

use bf_dom::{Document, NodeId};
use bf_infra::{Destination, Url};
use bf_js::host::ConsoleLevel;
use bf_js::vm::{Agent, RealmId};
use bf_net::csp::Inline;
use bf_net::security::DocumentPolicy;

use crate::WebContext;

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
        bf_js::vm::eval::StringCompilationCheck(Box::new(move || check.check_eval())),
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

/// Los mensajes pendientes de la política, a la consola de la ventana.
pub fn flush(agent: &mut Agent, realm: RealmId) {
    let Some(p) = policy(agent, realm) else {
        return;
    };
    for m in p.take_messages() {
        agent.host.hooks.console(ConsoleLevel::Error, &m);
    }
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
) -> Option<Url> {
    let Some(p) = ensure(agent, realm, doc) else {
        return Some(url.clone());
    };
    refresh_meta(agent, realm, doc);
    let r = p.check_request(url, destination, nonce);
    flush(agent, realm);
    r
}

/// Como [`check_request`] para un `<script src>` que insertó otro script (no el parser): con
/// `'strict-dynamic'` (CSP 3 §8.2) se permite aunque no tenga nonce.
pub fn check_inserted_script(
    agent: &mut Agent,
    realm: RealmId,
    url: &Url,
    nonce: Option<&str>,
) -> Option<Url> {
    let Some(p) = ensure(agent, realm, None) else {
        return Some(url.clone());
    };
    refresh_meta(agent, realm, None);
    let r = p.check_request_by(url, Destination::Script, nonce, false);
    flush(agent, realm);
    r
}

/// ¿Se puede ejecutar el script en línea `source` con el `nonce` de su elemento?
pub fn allow_inline_script(
    agent: &mut Agent,
    realm: RealmId,
    doc: Option<&Document>,
    nonce: Option<&str>,
    source: &str,
) -> bool {
    let Some(p) = ensure(agent, realm, doc) else {
        return true;
    };
    refresh_meta(agent, realm, doc);
    let ok = p.allow_inline(Inline::Script, nonce, source);
    flush(agent, realm);
    ok
}
