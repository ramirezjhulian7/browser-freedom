//! `window.matchMedia()`, `MediaQueryList` y `MediaQueryListEvent` (CSSOM View §4.2).
//!
//! `matches` se evalúa en cada lectura contra el viewport actual. Cada `MediaQueryList` es un
//! `EventTarget` (con `onchange`, `addListener`/`removeListener` como alias de
//! `addEventListener("change")`); [`report_changes`] ("evaluate media queries and report
//! changes", §4.2) dispara `change` en las que cambiaron. Corre después de cada cambio del
//! viewport hecho con [`super::set_viewport`]; el shell la llama al redimensionar.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use bf_css::media::parse_media_query_list;
use bf_dom::events::{Event, EventInit, EventTarget};
use bf_js::host::{HostClass, HostClassId, TaskSource};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, RealmId};

use super::{classes, media_context};
use crate::dom::events::{
    EventData, EventIface, TargetRef, dispatch_js_event, object_key, with_event_handlers,
};

/// La clase de `MediaQueryListEvent` (dato del agente).
struct EventClass(HostClassId);

/// Las `MediaQueryList` de una ventana: su target, su consulta y el último `matches`
/// reportado (dato del realm).
#[derive(Default)]
struct Lists(Vec<(u64, String, bool)>);

pub fn register_class(agent: &mut Agent) -> HostClassId {
    let dom = crate::dom::classes(agent).clone();
    let ev = agent.register_host_class(
        HostClass::new("MediaQueryListEvent")
            .parent(dom.event)
            .constructor(event_ctor, 1)
            .attribute("media", Some(ev_media), None)
            .attribute("matches", Some(ev_matches), None),
    );
    agent.set_embedder_data(EventClass(ev));
    agent.register_host_class(with_event_handlers(
        HostClass::new("MediaQueryList")
            .parent(dom.event_target)
            .attribute("media", Some(media), None)
            .attribute("matches", Some(matches), None)
            .method("addListener", 1, add_listener)
            .method("removeListener", 1, remove_listener),
        &["change"],
    ))
}

/// `partial interface Window { [NewObject] MediaQueryList matchMedia(CSSOMString query); }`.
pub fn window_partial() -> HostClass {
    HostClass::new("Window").method("matchMedia", 1, match_media)
}

fn evaluate(agent: &mut Agent, q: &str) -> bool {
    let ctx = media_context(agent);
    parse_media_query_list(q).evaluate(&ctx)
}

fn match_media(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let q = agent.to_string(&a.arg(0))?.to_string();
    // "Parse a media query list" y serializarla (CSSOM §4.1).
    let text = if q.trim().is_empty() {
        String::new()
    } else {
        super::value::media_list_text(&q)
    };
    let class = classes(agent).media_query_list;
    let (t, o) = crate::net::xhr::new_target(agent, class, None)?;
    let EventTarget::Object(id) = t.target else {
        return agent.type_error("matchMedia");
    };
    let now = evaluate(agent, &text);
    let realm = crate::WebContextHandle::current(agent).realm;
    lists(agent, realm).0.push((id, text, now));
    Ok(Value::Object(o))
}

fn lists(agent: &mut Agent, realm: RealmId) -> &mut Lists {
    if agent.realm_data::<Lists>(realm).is_none() {
        agent.set_realm_data(realm, Lists::default());
    }
    agent.realm_data_mut::<Lists>(realm).expect("Lists")
}

fn this_query(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<String> {
    let class = classes(agent).media_query_list;
    let id = match agent.host_unwrap::<TargetRef>(&a.this).copied() {
        Some(TargetRef {
            target: EventTarget::Object(id),
            ..
        }) if agent.host_is_instance(&a.this, class) => id,
        _ => return agent.type_error("Illegal invocation"),
    };
    let realm = crate::WebContextHandle::current(agent).realm;
    Ok(lists(agent, realm)
        .0
        .iter()
        .find(|(i, _, _)| *i == id)
        .map(|(_, q, _)| q.clone())
        .unwrap_or_default())
}

fn media(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let q = this_query(agent, a)?;
    Ok(Value::str(&q))
}

fn matches(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let q = this_query(agent, a)?;
    Ok(Value::Boolean(evaluate(agent, &q)))
}

/// `addListener(callback)`: alias de `addEventListener("change", callback)`.
fn add_listener(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_query(agent, a)?;
    forward(agent, a, "addEventListener")
}

fn remove_listener(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_query(agent, a)?;
    forward(agent, a, "removeEventListener")
}

fn forward(agent: &mut Agent, a: &CallArgs<'_>, method: &str) -> JsResult<Value> {
    let cb = a.arg(0);
    if cb.is_nullish() {
        return Ok(Value::Undefined);
    }
    let Value::Object(o) = a.this.clone() else {
        return Ok(Value::Undefined);
    };
    let f = agent.get(o, &method.into(), a.this.clone())?;
    if agent.is_callable(&f) {
        agent.call(&f, a.this.clone(), &[Value::str("change"), cb])?;
    }
    Ok(Value::Undefined)
}

/// "Evaluate media queries and report changes" (CSSOM View §4.2): dispara `change` (un
/// `MediaQueryListEvent`) en cada `MediaQueryList` de `realm` cuyo `matches` cambió, en el
/// orden en que se crearon.
pub fn report_changes(agent: &mut Agent, realm: RealmId) {
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let entries = lists(agent, realm).0.clone();
    let mut changed = Vec::new();
    for (i, (id, q, last)) in entries.iter().enumerate() {
        let now = evaluate(agent, q);
        if now != *last {
            changed.push((*id, q.clone(), now));
            lists(agent, realm).0[i].2 = now;
        }
    }
    let list_class = classes(agent).media_query_list;
    for (id, q, now) in changed {
        // El wrapper (uno nuevo si el anterior ya no existe: sus listeners siguen en el target).
        let key = object_key(0, id);
        if agent.host_lookup(realm, key).is_none() {
            let tr = TargetRef {
                doc: 0,
                target: EventTarget::Object(id),
            };
            agent.host_wrap(realm, key, list_class, Box::new(tr));
        }
        let ev = new_event(agent, None, "change", EventInit::default(), &q, now, true);
        if let Ok(Value::Object(o)) = ev {
            let pin = agent.host_pin(Value::Object(o));
            let _ = dispatch_js_event(agent, 0, EventTarget::Object(id), o, false);
            agent.host_unpin(pin);
        }
    }
    agent.current_realm = saved;
}

/// Encola [`report_changes`] en una tarea (después de un cambio del viewport).
pub fn queue_report(agent: &mut Agent, realm: RealmId) {
    if agent
        .realm_data::<Lists>(realm)
        .is_none_or(|l| l.0.is_empty())
    {
        return;
    }
    agent.queue_task(
        TaskSource::Other,
        Box::new(move |agent: &mut Agent| report_changes(agent, realm)),
    );
}

// --- MediaQueryListEvent (CSSOM View §4.2) ---

fn new_event(
    agent: &mut Agent,
    nt: Option<bf_js::heap::Gc<bf_js::object::Object>>,
    type_: impl Into<bf_dom::DomString>,
    init: EventInit,
    media: &str,
    matches: bool,
    trusted: bool,
) -> JsResult<Value> {
    let class = agent
        .embedder_data::<EventClass>()
        .expect("MediaQueryListEvent")
        .0;
    let mut ev = Event::new(type_, init);
    ev.is_trusted = trusted;
    let data = Box::new(EventData {
        cell: Rc::new(RefCell::new(ev)),
        iface: EventIface::Event,
        doc: Cell::new(0),
    });
    let o = match nt {
        Some(nt) => agent.host_create_from_new_target(nt, class, None, data)?,
        None => agent.host_create(class, data),
    };
    if let Some(s) = agent.host_slots_mut(o) {
        *s = vec![Value::str(media), Value::Boolean(matches)];
    }
    Ok(Value::Object(o))
}

fn event_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Illegal constructor");
    };
    if a.args.is_empty() {
        return agent.type_error(
            "Failed to construct 'MediaQueryListEvent': 1 argument required, but only 0 present.",
        );
    }
    let type_ = crate::dom::to_dom_string(agent, &a.arg(0))?;
    let d = a.arg(1);
    let get = |agent: &mut Agent, n: &str| -> JsResult<Value> {
        match &d {
            Value::Object(o) => agent.get(*o, &PropertyKey::from(n), d.clone()),
            Value::Undefined | Value::Null => Ok(Value::Undefined),
            _ => agent.type_error("The provided value is not of type 'MediaQueryListEventInit'"),
        }
    };
    // Los miembros del diccionario en orden (EventInit, luego MediaQueryListEventInit).
    let bubbles = get(agent, "bubbles")?;
    let bubbles = bf_js::host::webidl::to_boolean(agent, &bubbles);
    let cancelable = get(agent, "cancelable")?;
    let cancelable = bf_js::host::webidl::to_boolean(agent, &cancelable);
    let composed = get(agent, "composed")?;
    let composed = bf_js::host::webidl::to_boolean(agent, &composed);
    let m = get(agent, "matches")?;
    let matches = bf_js::host::webidl::to_boolean(agent, &m);
    let media = get(agent, "media")?;
    let media = if media.is_undefined() {
        String::new()
    } else {
        agent.to_string(&media)?.to_string()
    };
    new_event(
        agent,
        Some(nt),
        type_,
        EventInit {
            bubbles,
            cancelable,
            composed,
        },
        &media,
        matches,
        false,
    )
}

fn ev_slot(agent: &mut Agent, a: &CallArgs<'_>, i: usize) -> JsResult<Value> {
    let class = agent
        .embedder_data::<EventClass>()
        .expect("MediaQueryListEvent")
        .0;
    if !agent.host_is_instance(&a.this, class) {
        return agent.type_error("Illegal invocation");
    }
    let Value::Object(o) = &a.this else {
        return agent.type_error("Illegal invocation");
    };
    let o = *o;
    Ok(agent
        .host_slots_mut(o)
        .and_then(|s| s.get(i).cloned())
        .unwrap_or(Value::Undefined))
}

fn ev_media(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    ev_slot(agent, a, 0)
}

fn ev_matches(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    ev_slot(agent, a, 1)
}
