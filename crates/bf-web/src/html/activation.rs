//! Las acciones de *activation behavior* que `bf-dom` deja en cola (DOM §2.9 paso 11,
//! [`bf_dom::Document::take_activations`]): seguir enlaces (HTML §4.6.4), enviar y resetear
//! formularios (§4.10.21), el `label` que reenvía el click a su control (§4.10.4) y el
//! `summary` que abre o cierra su `details` (§4.11.2), con el evento `toggle` (§4.11.1).

use std::collections::HashMap;

use bf_dom::events::{Activation, EventInit};
use bf_infra::DomString;
use bf_js::value::Value;
use bf_js::vm::Agent;

use super::elements::{attr_value, html_local_name, parse_url, remove_attr, set_attr};
use super::forms::{self, form_owner_of};
use super::submission;
use crate::{DocIndex, NodeRef, WebContextHandle, with_doc, with_doc_mut};

/// Procesa las activaciones pendientes del documento `doc` (después de despachar un `click`
/// desde script o desde `click()`).
pub fn run_activations(agent: &mut Agent, doc: DocIndex) {
    let pending = with_doc_mut(agent, doc, |d| d.take_activations());
    for a in pending {
        run(agent, doc, a);
    }
}

/// Una activación.
pub fn run(agent: &mut Agent, doc: DocIndex, a: Activation) {
    match a {
        Activation::FollowHyperlink(n) => follow_hyperlink(agent, NodeRef::new(doc, n)),
        Activation::Submit(s) => {
            let s = NodeRef::new(doc, s);
            if let Some(form) = form_owner_of(agent, s) {
                submission::submit(agent, form, s, false);
            }
        }
        Activation::Reset(s) => {
            if let Some(form) = form_owner_of(agent, NodeRef::new(doc, s)) {
                forms::reset_form(agent, form);
            }
        }
        Activation::Label(l) => {
            if let Some(c) = forms::labeled_control(agent, NodeRef::new(doc, l)) {
                super::misc::click_element(agent, c);
            }
        }
        Activation::Summary(s) => toggle_details(agent, NodeRef::new(doc, s)),
    }
}

/// "Follow the hyperlink" (§4.6.4) de un `a` o `area`.
fn follow_hyperlink(agent: &mut Agent, n: NodeRef) {
    // "Cannot navigate": el documento no es el de la ventana, o un `area` desconectado.
    if n.doc != 0 {
        return;
    }
    let is_a = html_local_name(agent, n).as_deref() == Some("a");
    if !is_a && !with_doc(agent, n.doc, |d| d.is_connected(n.node)) {
        return;
    }
    // Las descargas (`download`) no navegan.
    if attr_value(agent, n, "download").is_some() {
        return;
    }
    let Some(href) = attr_value(agent, n, "href") else {
        return;
    };
    let Some(url) = parse_url(agent, n, &href.to_string_lossy()) else {
        return;
    };
    let target = attr_value(agent, n, "target")
        .map(|t| t.to_string_lossy().into_owned())
        .or_else(|| submission::base_target(agent, n))
        .unwrap_or_default();
    submission::navigate(agent, url, None, &target);
}

/// La activation behavior de `summary`: alterna el atributo `open` de su `details`.
fn toggle_details(agent: &mut Agent, summary: NodeRef) {
    let ok = with_doc(agent, summary.doc, |d| {
        d.is_summary_for_parent_details(summary.node)
    });
    let parent = with_doc(agent, summary.doc, |d| d.parent(summary.node));
    let (true, Some(p)) = (ok, parent) else {
        return;
    };
    let details = NodeRef::new(summary.doc, p);
    if attr_value(agent, details, "open").is_some() {
        remove_attr(agent, details, "open");
    } else {
        let _ = set_attr(agent, details, "open", DomString::new());
    }
}

// --- `toggle` de `details` (§4.11.1) ---

/// El "details toggle task tracker" de cada `details`: el estado viejo (`true` si estaba
/// abierto) y el nuevo de la tarea pendiente.
#[derive(Debug, Default)]
struct ToggleTasks(HashMap<NodeRef, (bool, bool)>);

/// Encola (o actualiza) la tarea del evento `toggle` de los `details` de `doc` cuyo `open`
/// cambió ("queue a details toggle event task").
pub fn queue_details_toggles(agent: &mut Agent, doc: DocIndex) {
    let changes = with_doc_mut(agent, doc, |d| d.take_details_toggles());
    if changes.is_empty() {
        return;
    }
    let realm = WebContextHandle::current(agent).realm;
    if agent.realm_data::<ToggleTasks>(realm).is_none() {
        agent.set_realm_data(realm, ToggleTasks::default());
    }
    for (el, open) in changes {
        let key = NodeRef::new(doc, el);
        let tasks = agent
            .realm_data_mut::<ToggleTasks>(realm)
            .expect("ToggleTasks");
        if let Some(t) = tasks.0.get_mut(&key) {
            t.1 = open;
            continue;
        }
        tasks.0.insert(key, (!open, open));
        agent.queue_task(
            bf_js::host::TaskSource::DomManipulation,
            Box::new(move |agent: &mut Agent| fire_toggle(agent, realm, key)),
        );
    }
}

fn fire_toggle(agent: &mut Agent, realm: bf_js::vm::RealmId, key: NodeRef) {
    let Some((old, new)) = agent
        .realm_data_mut::<ToggleTasks>(realm)
        .and_then(|t| t.0.remove(&key))
    else {
        return;
    };
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let state = |open: bool| Value::str(if open { "open" } else { "closed" });
    let class = submission::event_classes(agent).toggle;
    submission::fire_class_event(
        agent,
        key,
        class,
        "toggle",
        EventInit::default(),
        vec![state(new), state(old)],
    );
    agent.current_realm = saved;
}
