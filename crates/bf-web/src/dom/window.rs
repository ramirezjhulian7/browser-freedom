//! `Window` (HTML §7.2–7.3) como objeto global del realm: la cadena
//! `Window.prototype → WindowProperties → EventTarget.prototype → Object.prototype`, los
//! diálogos y `open` delegados a [`crate::WebHooks`], `window.event`, los event handlers y
//! los rechazos de promesas sin manejar (`unhandledrejection`, HTML §8.1.7.3).

use std::cell::RefCell;
use std::rc::Rc;

use bf_dom::events::{Event, EventInit, EventTarget};
use bf_js::heap::Gc;
use bf_js::host::{HostClass, HostClassId, TaskSource};
use bf_js::object::Object;
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, RealmId};

use crate::dom::events::{
    EventIface, GLOBAL_EVENT_HANDLERS, WINDOW_EVENT_HANDLERS, create_event_object,
    dispatch_js_event, with_event_handlers,
};
use crate::dom::state::state;
use crate::dom::{classes, to_dom_string, wrap_node};
use crate::{NodeRef, WebContextHandle};

/// El payload del `Window`.
#[derive(Debug, Clone, Copy)]
pub struct WindowData {
    pub realm: RealmId,
}

/// Los realms con ventana (para los rechazos de promesas, que no saben de realm).
#[derive(Debug, Default)]
struct WindowRealms(Vec<RealmId>);

pub fn window_class(event_target: HostClassId) -> HostClass {
    let c = HostClass::new("Window")
        .parent(event_target)
        .unforgeable("window", Some(window_self), None)
        .unforgeable("document", Some(document), None)
        .attribute("self", Some(window_self), Some(replaceable_self))
        .attribute("frames", Some(window_self), Some(replaceable_frames))
        .attribute("parent", Some(window_self), Some(replaceable_parent))
        .unforgeable("top", Some(window_self), None)
        .attribute("opener", Some(null), Some(noop_setter))
        .attribute("frameElement", Some(null), None)
        .attribute("length", Some(zero), Some(replaceable_length))
        .attribute("closed", Some(closed), None)
        .attribute("name", Some(get_name), Some(set_name))
        .attribute("event", Some(current_event), Some(replaceable_event))
        .method("alert", 0, alert)
        .method("confirm", 0, confirm)
        .method("prompt", 0, prompt)
        .method("open", 0, open)
        .method("close", 0, noop)
        .method("stop", 0, noop)
        .method("focus", 0, noop)
        .method("blur", 0, noop)
        .method("print", 0, noop)
        .method("postMessage", 1, post_message);
    let mut names: Vec<&'static str> = GLOBAL_EVENT_HANDLERS.to_vec();
    names.extend_from_slice(WINDOW_EVENT_HANDLERS);
    with_event_handlers(c, &names)
}

/// Convierte el global de `realm` en el `Window` e inserta `WindowProperties`.
pub fn install(agent: &mut Agent, realm: RealmId, _ctx: &WebContextHandle) {
    let class = classes(agent).window;
    let g = agent.host_make_global(realm, class, Box::new(WindowData { realm }));
    // WindowProperties (HTML §7.2.3.1): entre Window.prototype y EventTarget.prototype.
    let wp = agent.heap[g].proto.expect("Window.prototype");
    let et = agent.heap[wp].proto;
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let props = agent.new_object(et);
    let tag = PropertyKey::Symbol(agent.well_known.to_string_tag);
    agent.put_data(
        props,
        tag,
        Value::str("WindowProperties"),
        false,
        false,
        true,
    );
    agent.heap[wp].proto = Some(props);
    agent.current_realm = saved;
    // Los rechazos sin manejar van a `unhandledrejection` en la ventana.
    if agent.embedder_data::<WindowRealms>().is_none() {
        agent.set_embedder_data(WindowRealms::default());
    }
    if let Some(w) = agent.embedder_data_mut::<WindowRealms>() {
        w.0.push(realm);
    }
    agent.host.rejection_notifier = Some(notify_rejected_promises);
}

/// `window`, `self`, `frames`, `parent`, `top`: el `WindowProxy` (sin iframes, el propio
/// global).
fn window_self(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let realm = WebContextHandle::current(agent).realm;
    Ok(Value::Object(agent.realms[realm].global_object))
}

/// `[Replaceable]` (WebIDL §3.7.6): el setter crea una propiedad de datos propia.
fn replace(agent: &mut Agent, a: &CallArgs<'_>, name: &str) -> JsResult<Value> {
    if let Value::Object(o) = a.this {
        let v = a.arg(0);
        agent.create_data_property_or_throw(o, PropertyKey::from(name), v)?;
    }
    Ok(Value::Undefined)
}

fn replaceable_self(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    replace(agent, a, "self")
}

fn replaceable_frames(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    replace(agent, a, "frames")
}

fn replaceable_parent(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    replace(agent, a, "parent")
}

fn replaceable_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    replace(agent, a, "length")
}

fn replaceable_event(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    replace(agent, a, "event")
}

fn noop_setter(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Undefined)
}

/// `window.document`.
fn document(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let root = WebContextHandle::current(agent)
        .get(agent)
        .document()
        .root();
    Ok(wrap_node(agent, NodeRef::new(0, root)))
}

fn null(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Null)
}

fn zero(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(0.0))
}

fn closed(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(false))
}

fn noop(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Undefined)
}

/// `window.name` (HTML §7.2.2.2): el nombre del navigable (sin navegación, vive en el
/// estado de la ventana).
fn get_name(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let n = state(agent).window_name.clone();
    Ok(Value::str(&n))
}

fn set_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = to_dom_string(agent, &a.arg(0))?.to_string();
    state(agent).window_name = v;
    Ok(Value::Undefined)
}

/// `window.event` (HTML §7.2.4): el *current event* o `undefined`.
fn current_event(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let h = state(agent).current_event.last().copied();
    Ok(h.and_then(|h| agent.host_pinned(h)).unwrap_or_default())
}

/// Un argumento opcional de los diálogos como `String` (`""` si falta).
fn opt_string(agent: &mut Agent, v: &Value) -> JsResult<String> {
    if v.is_undefined() {
        return Ok(String::new());
    }
    Ok(to_dom_string(agent, v)?.to_string())
}

/// `alert(message)` (HTML §8.8.1).
fn alert(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let m = opt_string(agent, &a.arg(0))?;
    WebContextHandle::current(agent)
        .get_mut(agent)
        .hooks
        .alert(&m);
    Ok(Value::Undefined)
}

fn confirm(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let m = opt_string(agent, &a.arg(0))?;
    let r = WebContextHandle::current(agent)
        .get_mut(agent)
        .hooks
        .confirm(&m);
    Ok(Value::Boolean(r))
}

fn prompt(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let m = opt_string(agent, &a.arg(0))?;
    let d = opt_string(agent, &a.arg(1))?;
    let r = WebContextHandle::current(agent)
        .get_mut(agent)
        .hooks
        .prompt(&m, &d);
    Ok(r.map_or(Value::Null, |s| Value::str(&s)))
}

/// `open(url, target, features)` (HTML §7.2.2.1): lo decide el navegador; sin otro
/// `WindowProxy` que devolver, el resultado es `null`.
fn open(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let url = opt_string(agent, &a.arg(0))?;
    let target = if a.arg(1).is_undefined() {
        "_blank".to_owned()
    } else {
        opt_string(agent, &a.arg(1))?
    };
    let features = opt_string(agent, &a.arg(2))?;
    WebContextHandle::current(agent)
        .get_mut(agent)
        .hooks
        .open(&url, &target, &features);
    Ok(Value::Null)
}

/// `postMessage(message, targetOrigin)` (HTML §9.3.3) a la propia ventana: una tarea de
/// *posted message* que dispara `message` (como `Event` con `data` en la propiedad `data`).
fn post_message(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let data = agent.host_pin(a.arg(0));
    let realm = WebContextHandle::current(agent).realm;
    agent.queue_task(
        TaskSource::PostedMessage,
        Box::new(move |agent: &mut Agent| {
            let saved = agent.current_realm;
            agent.current_realm = realm;
            let data = agent.host_unpin(data).unwrap_or_default();
            let cell = Rc::new(RefCell::new(Event::new("message", EventInit::default())));
            cell.borrow_mut().is_trusted = true;
            let o = create_event_object(agent, EventIface::Event, cell, 0);
            let _ = agent.create_data_property_or_throw(o, PropertyKey::from("data"), data);
            let pin = agent.host_pin(Value::Object(o));
            let _ = dispatch_js_event(agent, 0, EventTarget::Window, o, false);
            agent.host_unpin(pin);
            agent.current_realm = saved;
        }),
    );
    Ok(Value::Undefined)
}

/// HTML "notify about rejected promises" (§8.1.7.3): una tarea que dispara
/// `unhandledrejection` por cada promesa que siga sin handler; si nadie la cancela, va al
/// reporte del motor.
fn notify_rejected_promises(agent: &mut Agent, promises: Vec<Gc<Object>>) {
    let Some(&realm) = agent
        .embedder_data::<WindowRealms>()
        .and_then(|w| w.0.first())
    else {
        return;
    };
    let pins: Vec<_> = promises
        .into_iter()
        .map(|p| agent.host_pin(Value::Object(p)))
        .collect();
    agent.queue_task(
        TaskSource::DomManipulation,
        Box::new(move |agent: &mut Agent| {
            let saved = agent.current_realm;
            agent.current_realm = realm;
            for h in pins {
                let Some(Value::Object(p)) = agent.host_unpin(h) else {
                    continue;
                };
                let Some((handled, reason)) = agent.promise_handled_and_result(p) else {
                    continue;
                };
                if handled {
                    continue;
                }
                let init = EventInit {
                    bubbles: false,
                    cancelable: true,
                    composed: false,
                };
                let cell = Rc::new(RefCell::new(Event::new("unhandledrejection", init)));
                cell.borrow_mut().is_trusted = true;
                let o = create_event_object(agent, EventIface::PromiseRejection, cell, 0);
                if let Some(s) = agent.host_slots_mut(o) {
                    *s = vec![Value::Object(p), reason.clone()];
                }
                let pin = agent.host_pin(Value::Object(o));
                let not_canceled = matches!(
                    dispatch_js_event(agent, 0, EventTarget::Window, o, false),
                    Ok(Ok(true))
                );
                agent.host_unpin(pin);
                if not_canceled {
                    let err = bf_js::host::JsError::from_value(agent, reason);
                    let msg = format!("Uncaught (in promise) {}", err.message);
                    agent.host.hooks.report_exception(&msg, &err.stack);
                }
            }
            agent.current_realm = saved;
        }),
    );
}
