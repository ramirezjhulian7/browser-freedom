//! Mensajería entre contextos (HTML §9.3–§9.5): `MessageEvent`, `MessageChannel`,
//! `MessagePort` y `BroadcastChannel`, dentro del mismo agente. Los mensajes se copian con
//! `structuredClone` y se entregan en tareas de la fuente *posted message*.
//!
//! `BroadcastChannel` entre pestañas necesita un canal entre los hilos de las pestañas (el
//! shell): por ahora llega a los `BroadcastChannel` del mismo agente.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use bf_dom::events::{Event, EventInit, EventTarget};
use bf_js::host::{HostClass, HostClassId, HostHandle, TaskSource};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use super::xhr::new_target;
use super::{classes, state};
use crate::dom::events::{EventData, EventIface, TargetRef, dispatch_js_event, with_event_handlers};

type Obj = bf_js::heap::Gc<bf_js::object::Object>;

// --- MessageEvent (HTML §9.3.1) ---

pub fn message_event_class(event: HostClassId) -> HostClass {
    HostClass::new("MessageEvent")
        .parent(event)
        .constructor(message_event_ctor, 1)
        .attribute("data", Some(me_data), None)
        .attribute("origin", Some(me_origin), None)
        .attribute("lastEventId", Some(me_last_event_id), None)
        .attribute("source", Some(me_source), None)
        .attribute("ports", Some(me_ports), None)
        .method("initMessageEvent", 1, me_init)
}

/// Un `MessageEvent` nuevo (`trusted` si lo dispara el navegador).
pub fn create_message_event(
    agent: &mut Agent,
    type_: &str,
    init: EventInit,
    trusted: bool,
    fields: [Value; 5],
) -> JsResult<Obj> {
    let mut ev = Event::new(type_, init);
    ev.is_trusted = trusted;
    let class = classes(agent).message_event;
    let o = agent.host_create(
        class,
        Box::new(EventData {
            cell: Rc::new(RefCell::new(ev)),
            iface: EventIface::Event,
            doc: Cell::new(0),
        }),
    );
    if let Some(s) = agent.host_slots_mut(o) {
        *s = fields.to_vec();
    }
    Ok(o)
}

/// Despacha un evento en `t` (fijado mientras dura).
pub fn dispatch(agent: &mut Agent, t: TargetRef, ev: Obj) {
    let pin = agent.host_pin(Value::Object(ev));
    let _ = dispatch_js_event(agent, t.doc, t.target, ev, false);
    agent.host_unpin(pin);
}

/// Dispara un `MessageEvent` de confianza `type_` en `t`.
pub fn fire_message(
    agent: &mut Agent,
    t: TargetRef,
    type_: &str,
    data: Value,
    origin: &str,
    last_event_id: &str,
    ports: Vec<Value>,
) {
    let ports = match agent.create_array_from_list(&ports) {
        Ok(a) => {
            let _ = agent.set_integrity_level(a, true);
            Value::Object(a)
        }
        Err(_) => Value::Null,
    };
    if let Ok(ev) = create_message_event(
        agent,
        type_,
        EventInit::default(),
        true,
        [data, Value::str(origin), Value::str(last_event_id), Value::Null, ports],
    ) {
        dispatch(agent, t, ev);
    }
}

fn dict_member(agent: &mut Agent, d: &Value, name: &str) -> JsResult<Value> {
    match d {
        Value::Object(o) => agent.get(*o, &PropertyKey::from(name), d.clone()),
        Value::Undefined | Value::Null => Ok(Value::Undefined),
        _ => agent.type_error("The provided value is not of type 'MessageEventInit'"),
    }
}

fn message_event_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Illegal constructor");
    };
    if a.args.is_empty() {
        return agent.type_error("Failed to construct 'MessageEvent': 1 argument required, but only 0 present.");
    }
    let type_ = crate::dom::to_dom_string(agent, &a.arg(0))?;
    let d = a.arg(1);
    let flag = |agent: &mut Agent, n: &str| -> JsResult<bool> {
        let v = dict_member(agent, &d, n)?;
        Ok(bf_js::host::webidl::to_boolean(agent, &v))
    };
    let bubbles = flag(agent, "bubbles")?;
    let cancelable = flag(agent, "cancelable")?;
    let composed = flag(agent, "composed")?;
    let data = dict_member(agent, &d, "data")?;
    let data = if data.is_undefined() { Value::Null } else { data };
    let str_member = |agent: &mut Agent, n: &str| -> JsResult<Value> {
        let v = dict_member(agent, &d, n)?;
        if v.is_undefined() {
            Ok(Value::str(""))
        } else {
            Ok(Value::String(agent.to_string(&v)?))
        }
    };
    let last_event_id = str_member(agent, "lastEventId")?;
    let origin = str_member(agent, "origin")?;
    let ports_v = dict_member(agent, &d, "ports")?;
    let ports = if ports_v.is_undefined() {
        Vec::new()
    } else {
        let m = match &ports_v {
            Value::Object(o) => {
                let k = PropertyKey::Symbol(agent.well_known.iterator);
                agent.get(*o, &k, ports_v.clone())?
            }
            _ => Value::Undefined,
        };
        if !agent.is_callable(&m) {
            return agent.type_error("Failed to construct 'MessageEvent': The 'ports' member is not iterable.");
        }
        let items = super::headers::collect_iterable(agent, &ports_v, &m)?;
        let port_class = classes(agent).message_port;
        for p in &items {
            if !agent.host_is_instance(p, port_class) {
                return agent.type_error("Failed to construct 'MessageEvent': Failed to read the 'ports' member: element is not of type 'MessagePort'.");
            }
        }
        items
    };
    let source = dict_member(agent, &d, "source")?;
    let source = if source.is_undefined() { Value::Null } else { source };
    let ports_arr = agent.create_array_from_list(&ports)?;
    agent.set_integrity_level(ports_arr, true)?;
    let class = classes(agent).message_event;
    let o = agent.host_create_from_new_target(
        nt,
        class,
        None,
        Box::new(EventData {
            cell: Rc::new(RefCell::new(Event::new(
                type_,
                EventInit {
                    bubbles,
                    cancelable,
                    composed,
                },
            ))),
            iface: EventIface::Event,
            doc: Cell::new(0),
        }),
    )?;
    if let Some(s) = agent.host_slots_mut(o) {
        *s = vec![data, origin, last_event_id, source, Value::Object(ports_arr)];
    }
    Ok(Value::Object(o))
}

fn me_slot(agent: &mut Agent, a: &CallArgs<'_>, i: usize) -> JsResult<Value> {
    let class = classes(agent).message_event;
    if !agent.host_is_instance(&a.this, class) {
        return agent.type_error("Illegal invocation");
    }
    let Value::Object(o) = &a.this else {
        return agent.type_error("Illegal invocation");
    };
    Ok(agent
        .host_slots_mut(*o)
        .and_then(|s| s.get(i).cloned())
        .unwrap_or(Value::Null))
}

fn me_data(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    me_slot(agent, a, 0)
}

fn me_origin(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    me_slot(agent, a, 1)
}

fn me_last_event_id(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    me_slot(agent, a, 2)
}

fn me_source(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    me_slot(agent, a, 3)
}

fn me_ports(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    me_slot(agent, a, 4)
}

/// `initMessageEvent(type, bubbles, cancelable, data, origin, lastEventId, source, ports)`.
fn me_init(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let class = classes(agent).message_event;
    if !agent.host_is_instance(&a.this, class) {
        return agent.type_error("Illegal invocation");
    }
    let Some(d) = agent.host_unwrap::<EventData>(&a.this) else {
        return agent.type_error("Illegal invocation");
    };
    let cell = d.cell.clone();
    if cell.borrow().event_phase != bf_dom::events::EventPhase::None {
        return Ok(Value::Undefined);
    }
    let type_ = crate::dom::to_dom_string(agent, &a.arg(0))?;
    let bubbles = bf_js::host::webidl::to_boolean(agent, &a.arg(1));
    let cancelable = bf_js::host::webidl::to_boolean(agent, &a.arg(2));
    {
        let mut ev = cell.borrow_mut();
        let trusted = ev.is_trusted;
        *ev = Event::new(
            type_,
            EventInit {
                bubbles,
                cancelable,
                composed: false,
            },
        );
        ev.is_trusted = trusted;
    }
    let origin = Value::String(agent.to_string(&a.arg(4))?);
    let last = Value::String(agent.to_string(&a.arg(5))?);
    let ports = if a.arg(7).is_undefined() {
        Vec::new()
    } else {
        let pv = a.arg(7);
        let m = match &pv {
            Value::Object(o) => {
                let k = PropertyKey::Symbol(agent.well_known.iterator);
                agent.get(*o, &k, pv.clone())?
            }
            _ => Value::Undefined,
        };
        if agent.is_callable(&m) {
            super::headers::collect_iterable(agent, &pv, &m)?
        } else {
            Vec::new()
        }
    };
    let arr = agent.create_array_from_list(&ports)?;
    agent.set_integrity_level(arr, true)?;
    if let Value::Object(o) = &a.this
        && let Some(s) = agent.host_slots_mut(*o)
    {
        *s = vec![a.arg(3), origin, last, a.arg(6), Value::Object(arr)];
    }
    Ok(Value::Undefined)
}

// --- MessageChannel / MessagePort (HTML §9.4) ---

/// El estado de los puertos y canales del agente.
#[derive(Default)]
pub struct Ports {
    /// Por id de target: el puerto entrelazado, la cola y si empezó.
    ports: HashMap<u64, PortState>,
    /// Los `BroadcastChannel` abiertos: id de target → (origen, nombre).
    broadcast: Vec<(u64, String, String)>,
}

#[derive(Default)]
struct PortState {
    entangled: Option<u64>,
    started: bool,
    closed: bool,
    /// Los mensajes que esperan a `start()` (fijados).
    queue: Vec<(HostHandle, Vec<HostHandle>)>,
    /// El wrapper (fijado mientras está entrelazado, para que reciba mensajes).
    pin: Option<HostHandle>,
}

fn target_id(t: TargetRef) -> u64 {
    match t.target {
        EventTarget::Object(id) => id,
        _ => 0,
    }
}

fn target_of(id: u64) -> TargetRef {
    TargetRef {
        doc: 0,
        target: EventTarget::Object(id),
    }
}

pub fn message_port_class(event_target: HostClassId) -> HostClass {
    with_event_handlers(
        HostClass::new("MessagePort")
            .parent(event_target)
            .method("postMessage", 1, port_post_message)
            .method("start", 0, port_start)
            .method("close", 0, port_close),
        &["message", "messageerror"],
    )
}

pub fn message_channel_class() -> HostClass {
    HostClass::new("MessageChannel")
        .constructor(channel_ctor, 0)
        .attribute("port1", Some(channel_port1), None)
        .attribute("port2", Some(channel_port2), None)
}

/// Un `MessagePort` nuevo (sin entrelazar).
fn new_port(agent: &mut Agent) -> JsResult<(u64, Obj)> {
    let class = classes(agent).message_port;
    let (t, o) = new_target(agent, class, None)?;
    let id = target_id(t);
    let pin = agent.host_pin(Value::Object(o));
    state(agent).ports.ports.insert(
        id,
        PortState {
            pin: Some(pin),
            ..PortState::default()
        },
    );
    Ok((id, o))
}

/// `new MessageChannel()`: dos puertos entrelazados.
fn channel_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Illegal constructor");
    };
    let (id1, p1) = new_port(agent)?;
    let (id2, p2) = new_port(agent)?;
    let ports = &mut state(agent).ports.ports;
    if let Some(p) = ports.get_mut(&id1) {
        p.entangled = Some(id2);
    }
    if let Some(p) = ports.get_mut(&id2) {
        p.entangled = Some(id1);
    }
    let class = classes(agent).message_channel;
    let o = agent.host_create_from_new_target(nt, class, None, Box::new(ChannelData))?;
    if let Some(s) = agent.host_slots_mut(o) {
        *s = vec![Value::Object(p1), Value::Object(p2)];
    }
    Ok(Value::Object(o))
}

#[derive(Debug, Clone, Copy)]
struct ChannelData;

fn channel_port(agent: &mut Agent, a: &CallArgs<'_>, i: usize) -> JsResult<Value> {
    if agent.host_unwrap::<ChannelData>(&a.this).is_none() {
        return agent.type_error("Illegal invocation");
    }
    let Value::Object(o) = &a.this else {
        return agent.type_error("Illegal invocation");
    };
    Ok(agent
        .host_slots_mut(*o)
        .and_then(|s| s.get(i).cloned())
        .unwrap_or(Value::Null))
}

fn channel_port1(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    channel_port(agent, a, 0)
}

fn channel_port2(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    channel_port(agent, a, 1)
}

fn this_port(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<(TargetRef, u64)> {
    let class = classes(agent).message_port;
    match agent.host_unwrap::<TargetRef>(&a.this).copied() {
        Some(t) if agent.host_is_instance(&a.this, class) => Ok((t, target_id(t))),
        _ => agent.type_error("Illegal invocation"),
    }
}

/// StructuredSerializeWithTransfer + deserialize (en el mismo agente): `structuredClone`
/// sin los `MessagePort` de `transfer`, que se pasan aparte.
fn clone_message(agent: &mut Agent, message: &Value, transfer: &Value) -> JsResult<(Value, Vec<Value>)> {
    let port_class = classes(agent).message_port;
    let mut ports = Vec::new();
    let mut others = Vec::new();
    // `transfer` es una secuencia o `StructuredSerializeOptions { transfer }`.
    let list = match transfer {
        Value::Undefined | Value::Null => Value::Undefined,
        Value::Object(o) => {
            let k = PropertyKey::Symbol(agent.well_known.iterator);
            let m = agent.get(*o, &k, transfer.clone())?;
            if agent.is_callable(&m) {
                transfer.clone()
            } else {
                agent.get(*o, &PropertyKey::from("transfer"), transfer.clone())?
            }
        }
        _ => return agent.type_error("Failed to execute 'postMessage': The provided value cannot be converted to a sequence."),
    };
    if let Value::Object(o) = &list {
        let k = PropertyKey::Symbol(agent.well_known.iterator);
        let m = agent.get(*o, &k, list.clone())?;
        if !agent.is_callable(&m) {
            return agent.type_error("Failed to execute 'postMessage': The provided value cannot be converted to a sequence.");
        }
        for v in super::headers::collect_iterable(agent, &list, &m)? {
            if agent.host_is_instance(&v, port_class) {
                ports.push(v);
            } else {
                others.push(v);
            }
        }
    }
    let realm = agent.current_realm;
    let g = agent.realms[realm].global_object;
    let sc = agent.get(g, &PropertyKey::from("structuredClone"), Value::Object(g))?;
    let opts = agent.new_plain_object();
    let arr = agent.create_array_from_list(&others)?;
    agent.create_data_property_or_throw(opts, PropertyKey::from("transfer"), Value::Object(arr))?;
    let data = agent.call(&sc, Value::Undefined, &[message.clone(), Value::Object(opts)])?;
    Ok((data, ports))
}

/// Entrega (o encola) un mensaje en el puerto `to`.
fn enqueue_port_message(agent: &mut Agent, to: u64, data: Value, ports: Vec<Value>) {
    let data = agent.host_pin(data);
    let ports: Vec<HostHandle> = ports.into_iter().map(|p| agent.host_pin(p)).collect();
    let Some(p) = state(agent).ports.ports.get_mut(&to) else {
        return;
    };
    p.queue.push((data, ports));
    let started = p.started || port_has_onmessage(agent, to);
    if started {
        flush_port(agent, to);
    }
}

/// ¿Tiene el puerto un `onmessage`? (Asignarlo hace `start()` implícito, HTML §9.4.4.)
fn port_has_onmessage(agent: &mut Agent, id: u64) -> bool {
    let realm = agent.current_realm;
    let Some(o) = agent.host_lookup(realm, crate::dom::events::object_key(0, id)) else {
        return false;
    };
    agent
        .get(o, &PropertyKey::from("onmessage"), Value::Object(o))
        .is_ok_and(|v| agent.is_callable(&v))
}

/// Pasa la cola del puerto a tareas *posted message*.
fn flush_port(agent: &mut Agent, id: u64) {
    let queue = match state(agent).ports.ports.get_mut(&id) {
        Some(p) => std::mem::take(&mut p.queue),
        None => return,
    };
    for (data, ports) in queue {
        agent.queue_task(
            TaskSource::PostedMessage,
            Box::new(move |agent: &mut Agent| {
                let data = agent.host_unpin(data).unwrap_or(Value::Undefined);
                let ports: Vec<Value> = ports
                    .into_iter()
                    .filter_map(|p| agent.host_unpin(p))
                    .collect();
                let closed = state(agent).ports.ports.get(&id).is_none_or(|p| p.closed);
                if !closed {
                    fire_message(agent, target_of(id), "message", data, "", "", ports);
                }
            }),
        );
    }
}

/// `postMessage(message, transfer)` (HTML §9.4.4).
fn port_post_message(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (_, id) = this_port(agent, a)?;
    let (data, ports) = clone_message(agent, &a.arg(0), &a.arg(1))?;
    for p in &ports {
        if agent.host_unwrap::<TargetRef>(p).map(|t| target_id(*t)) == Some(id) {
            return crate::net::dom_error(agent, "DataCloneError", "A MessagePort cannot transfer itself.");
        }
    }
    let to = state(agent).ports.ports.get(&id).and_then(|p| if p.closed { None } else { p.entangled });
    if let Some(to) = to {
        enqueue_port_message(agent, to, data, ports);
    }
    Ok(Value::Undefined)
}

/// `start()`: empieza a entregar los mensajes encolados.
fn port_start(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (_, id) = this_port(agent, a)?;
    if let Some(p) = state(agent).ports.ports.get_mut(&id) {
        p.started = true;
    }
    flush_port(agent, id);
    Ok(Value::Undefined)
}

/// `close()`: desentrelaza los dos puertos.
fn port_close(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (_, id) = this_port(agent, a)?;
    let other = state(agent).ports.ports.get_mut(&id).and_then(|p| {
        p.closed = true;
        p.entangled.take()
    });
    let pins: Vec<HostHandle> = {
        let ports = &mut state(agent).ports.ports;
        let mut v = Vec::new();
        if let Some(p) = ports.get_mut(&id) {
            v.extend(p.pin.take());
        }
        if let Some(o) = other
            && let Some(p) = ports.get_mut(&o)
        {
            p.entangled = None;
            v.extend(p.pin.take());
        }
        v
    };
    for p in pins {
        agent.host_unpin(p);
    }
    Ok(Value::Undefined)
}

// --- BroadcastChannel (HTML §9.5) ---

pub fn broadcast_channel_class(event_target: HostClassId) -> HostClass {
    with_event_handlers(
        HostClass::new("BroadcastChannel")
            .parent(event_target)
            .constructor(bc_ctor, 1)
            .attribute("name", Some(bc_name), None)
            .method("postMessage", 1, bc_post_message)
            .method("close", 0, bc_close),
        &["message", "messageerror"],
    )
}

fn bc_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Illegal constructor");
    };
    if a.args.is_empty() {
        return agent.type_error("Failed to construct 'BroadcastChannel': 1 argument required, but only 0 present.");
    }
    let name = agent.to_string(&a.arg(0))?.to_string();
    let class = classes(agent).broadcast_channel;
    let (t, o) = new_target(agent, class, Some(nt))?;
    let origin = super::document_origin(agent);
    if let Some(s) = agent.host_slots_mut(o) {
        *s = vec![Value::str(&name), Value::Boolean(false)];
    }
    let id = target_id(t);
    let pin = agent.host_pin(Value::Object(o));
    let st = &mut state(agent).ports;
    st.broadcast.push((id, origin, name));
    st.ports.insert(
        id,
        PortState {
            pin: Some(pin),
            started: true,
            ..PortState::default()
        },
    );
    Ok(Value::Object(o))
}

fn this_bc(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<(u64, Obj)> {
    let class = classes(agent).broadcast_channel;
    match (agent.host_unwrap::<TargetRef>(&a.this).copied(), &a.this) {
        (Some(t), Value::Object(o)) if agent.host_is_instance(&a.this, class) => Ok((target_id(t), *o)),
        _ => agent.type_error("Illegal invocation"),
    }
}

fn bc_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (_, o) = this_bc(agent, a)?;
    Ok(agent
        .host_slots_mut(o)
        .and_then(|s| s.first().cloned())
        .unwrap_or(Value::str("")))
}

/// `postMessage(message)` (HTML §9.5): a todos los demás canales abiertos con el mismo
/// nombre y origen, en orden de creación.
fn bc_post_message(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (id, _) = this_bc(agent, a)?;
    let closed = state(agent).ports.ports.get(&id).is_none_or(|p| p.closed);
    if closed {
        return crate::net::dom_error(agent, "InvalidStateError", "The BroadcastChannel is closed.");
    }
    let (origin, name) = match state(agent).ports.broadcast.iter().find(|b| b.0 == id) {
        Some(b) => (b.1.clone(), b.2.clone()),
        None => return Ok(Value::Undefined),
    };
    let targets: Vec<u64> = state(agent)
        .ports
        .broadcast
        .iter()
        .filter(|b| b.0 != id && b.1 == origin && b.2 == name)
        .map(|b| b.0)
        .collect();
    for to in targets {
        let (data, _) = clone_message(agent, &a.arg(0), &Value::Undefined)?;
        let origin = origin.clone();
        let data = agent.host_pin(data);
        agent.queue_task(
            TaskSource::PostedMessage,
            Box::new(move |agent: &mut Agent| {
                let data = agent.host_unpin(data).unwrap_or(Value::Undefined);
                let closed = state(agent).ports.ports.get(&to).is_none_or(|p| p.closed);
                if !closed {
                    fire_message(agent, target_of(to), "message", data, &origin, "", Vec::new());
                }
            }),
        );
    }
    Ok(Value::Undefined)
}

fn bc_close(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (id, _) = this_bc(agent, a)?;
    let st = &mut state(agent).ports;
    st.broadcast.retain(|b| b.0 != id);
    let pin = st.ports.get_mut(&id).and_then(|p| {
        p.closed = true;
        p.pin.take()
    });
    if let Some(p) = pin {
        agent.host_unpin(p);
    }
    Ok(Value::Undefined)
}
