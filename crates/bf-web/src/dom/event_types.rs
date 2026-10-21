//! Las interfaces de evento cuyos datos son solo miembros de su diccionario (HTML §8.1.8,
//! §7.4, §9.3, §12.2; UI Events §5.4, §5.7): `HashChangeEvent`, `PopStateEvent`,
//! `PageTransitionEvent`, `MessageEvent`, `StorageEvent`, `CompositionEvent`, `WheelEvent`,
//! `TouchEvent` (sin `Touch`: las listas son arrays), `AnimationEvent` y `TransitionEvent`.
//! Los miembros propios van en los slots del wrapper desde el índice 2, en el orden
//! lexicográfico en que WebIDL lee el diccionario (§3.2.17).

use bf_dom::events::{Event, EventKind};
use bf_js::host::{HostClass, HostClassId};
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use crate::dom::classes;
use crate::dom::events::{
    EventIface, construct_event_vec, ctor_type, dict_get, event_init, mouse_parts, required, slot,
    ui_init, with_event, with_event_mut,
};

/// El tipo WebIDL de un miembro y su valor por defecto.
#[derive(Clone, Copy)]
enum Ty {
    /// `DOMString`/`USVString` con `""`.
    Str,
    /// `DOMString?` con `null`.
    NullStr,
    /// `any` (o un objeto nullable) con `null`.
    Any,
    Bool,
    Double,
    ULong,
    /// `sequence<…>` con `[]` (un array nuevo).
    Seq,
}

/// Un miembro: nombre en el diccionario y en el atributo, y tipo.
type Member = (&'static str, Ty);

const HASH_CHANGE: &[Member] = &[("newURL", Ty::Str), ("oldURL", Ty::Str)];
const POP_STATE: &[Member] = &[("hasUAVisualTransition", Ty::Bool), ("state", Ty::Any)];
const PAGE_TRANSITION: &[Member] = &[("persisted", Ty::Bool)];
const MESSAGE: &[Member] = &[
    ("data", Ty::Any),
    ("lastEventId", Ty::Str),
    ("origin", Ty::Str),
    ("ports", Ty::Seq),
    ("source", Ty::Any),
];
const STORAGE: &[Member] = &[
    ("key", Ty::NullStr),
    ("newValue", Ty::NullStr),
    ("oldValue", Ty::NullStr),
    ("storageArea", Ty::Any),
    ("url", Ty::Str),
];
const COMPOSITION: &[Member] = &[("data", Ty::Str)];
const WHEEL: &[Member] = &[
    ("deltaMode", Ty::ULong),
    ("deltaX", Ty::Double),
    ("deltaY", Ty::Double),
    ("deltaZ", Ty::Double),
];

const ANIMATION: &[Member] = &[
    ("animationName", Ty::Str),
    ("elapsedTime", Ty::Double),
    ("pseudoElement", Ty::Str),
];
const TRANSITION: &[Member] = &[
    ("elapsedTime", Ty::Double),
    ("propertyName", Ty::Str),
    ("pseudoElement", Ty::Str),
];
/// `TouchEventInit`: los de `EventModifierInit` y luego los propios (WebIDL lee primero el
/// diccionario heredado).
const TOUCH: &[Member] = &[
    ("altKey", Ty::Bool),
    ("ctrlKey", Ty::Bool),
    ("metaKey", Ty::Bool),
    ("shiftKey", Ty::Bool),
    ("changedTouches", Ty::Seq),
    ("targetTouches", Ty::Seq),
    ("touches", Ty::Seq),
];

/// Los getters de los slots 2 a 8.
const SLOT_GETTERS: [bf_js::vm::function::NativeFn; 7] = [s2, s3, s4, s5, s6, s7, s8];

fn s2(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    slot(agent, a, 2)
}
fn s3(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    slot(agent, a, 3)
}
fn s4(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    slot(agent, a, 4)
}
fn s5(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    slot(agent, a, 5)
}
fn s6(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    slot(agent, a, 6)
}
fn s7(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    slot(agent, a, 7)
}
fn s8(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    slot(agent, a, 8)
}

/// Una clase con los atributos de `members`.
fn class(name: &'static str, parent: HostClassId, members: &[Member]) -> HostClass {
    let mut c = HostClass::new(name).parent(parent);
    for (i, (m, _)) in members.iter().enumerate() {
        c = c.attribute(m, Some(SLOT_GETTERS[i]), None);
    }
    c
}

pub fn hash_change_event_class(event: HostClassId) -> HostClass {
    class("HashChangeEvent", event, HASH_CHANGE).constructor(hash_change_ctor, 1)
}

pub fn pop_state_event_class(event: HostClassId) -> HostClass {
    class("PopStateEvent", event, POP_STATE).constructor(pop_state_ctor, 1)
}

pub fn page_transition_event_class(event: HostClassId) -> HostClass {
    class("PageTransitionEvent", event, PAGE_TRANSITION).constructor(page_transition_ctor, 1)
}

pub fn message_event_class(event: HostClassId) -> HostClass {
    class("MessageEvent", event, MESSAGE)
        .constructor(message_ctor, 1)
        .method("initMessageEvent", 1, init_message_event)
}

pub fn storage_event_class(event: HostClassId) -> HostClass {
    class("StorageEvent", event, STORAGE)
        .constructor(storage_ctor, 1)
        .method("initStorageEvent", 1, init_storage_event)
}

pub fn composition_event_class(ui: HostClassId) -> HostClass {
    class("CompositionEvent", ui, COMPOSITION)
        .constructor(composition_ctor, 1)
        .method("initCompositionEvent", 1, init_composition_event)
}

pub fn wheel_event_class(mouse: HostClassId) -> HostClass {
    class("WheelEvent", mouse, WHEEL)
        .constructor(wheel_ctor, 1)
        .constant("DOM_DELTA_PIXEL", 0.0)
        .constant("DOM_DELTA_LINE", 1.0)
        .constant("DOM_DELTA_PAGE", 2.0)
}

pub fn animation_event_class(event: HostClassId) -> HostClass {
    class("AnimationEvent", event, ANIMATION).constructor(animation_ctor, 1)
}

pub fn transition_event_class(event: HostClassId) -> HostClass {
    class("TransitionEvent", event, TRANSITION).constructor(transition_ctor, 1)
}

pub fn touch_event_class(ui: HostClassId) -> HostClass {
    class("TouchEvent", ui, TOUCH).constructor(touch_ctor, 1)
}

/// Convierte `v` según `ty` (con el valor por defecto si es `undefined`).
fn convert(agent: &mut Agent, v: Value, ty: Ty) -> JsResult<Value> {
    if v.is_undefined() {
        return Ok(match ty {
            Ty::Str => Value::str(""),
            Ty::NullStr | Ty::Any => Value::Null,
            Ty::Bool => Value::Boolean(false),
            Ty::Double | Ty::ULong => Value::Number(0.0),
            Ty::Seq => Value::Object(agent.create_array_from_list(&[])?),
        });
    }
    Ok(match ty {
        Ty::Str => Value::String(agent.to_string(&v)?),
        Ty::NullStr if v.is_null() => Value::Null,
        Ty::NullStr => Value::String(agent.to_string(&v)?),
        Ty::Any => v,
        Ty::Bool => Value::Boolean(bf_js::host::webidl::to_boolean(agent, &v)),
        Ty::Double => Value::Number(bf_js::host::webidl::to_double(agent, &v)?),
        Ty::ULong => Value::Number(f64::from(bf_js::host::webidl::to_unsigned_long(agent, &v)?)),
        Ty::Seq => {
            let items = bf_js::host::webidl::to_sequence(agent, &v, |_, x| Ok(x.clone()))?;
            Value::Object(agent.create_array_from_list(&items)?)
        }
    })
}

/// Los miembros propios del diccionario `dict`.
fn read_members(agent: &mut Agent, dict: &Value, members: &[Member]) -> JsResult<Vec<Value>> {
    let mut out = Vec::with_capacity(members.len());
    for (name, ty) in members {
        let v = dict_get(agent, dict, name)?;
        out.push(convert(agent, v, *ty)?);
    }
    Ok(out)
}

/// El constructor de una interfaz de `Event` con solo miembros de diccionario.
fn plain_ctor(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    name: &str,
    class: HostClassId,
    members: &[Member],
) -> JsResult<Value> {
    let type_ = ctor_type(agent, a, name)?;
    let dict = a.arg(1);
    let init = event_init(agent, &dict)?;
    let mut slots = vec![Value::Null, Value::Undefined];
    slots.extend(read_members(agent, &dict, members)?);
    let ev = Event::new(type_, init);
    construct_event_vec(agent, a, EventIface::Class(class), ev, slots)
}

fn hash_change_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let c = classes(agent).hash_change_event;
    plain_ctor(agent, a, "HashChangeEvent", c, HASH_CHANGE)
}

fn pop_state_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let c = classes(agent).pop_state_event;
    plain_ctor(agent, a, "PopStateEvent", c, POP_STATE)
}

fn page_transition_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let c = classes(agent).page_transition_event;
    plain_ctor(agent, a, "PageTransitionEvent", c, PAGE_TRANSITION)
}

fn message_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let c = classes(agent).message_event;
    plain_ctor(agent, a, "MessageEvent", c, MESSAGE)
}

fn storage_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let c = classes(agent).storage_event;
    plain_ctor(agent, a, "StorageEvent", c, STORAGE)
}

fn animation_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let c = classes(agent).animation_event;
    plain_ctor(agent, a, "AnimationEvent", c, ANIMATION)
}

fn transition_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let c = classes(agent).transition_event;
    plain_ctor(agent, a, "TransitionEvent", c, TRANSITION)
}

/// `new TouchEvent(type, dict)`: `UIEventInit`, las teclas y las listas de toques.
fn touch_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let type_ = ctor_type(agent, a, "TouchEvent")?;
    let dict = a.arg(1);
    let init = event_init(agent, &dict)?;
    let (view, detail) = ui_init(agent, &dict)?;
    let mut slots = vec![view, Value::Undefined];
    slots.extend(read_members(agent, &dict, TOUCH)?);
    let ev = Event::with_kind(type_, init, EventKind::Ui { detail });
    let c = classes(agent).touch_event;
    construct_event_vec(agent, a, EventIface::Class(c), ev, slots)
}

/// `new CompositionEvent(type, dict)`: `UIEventInit` y `data`.
fn composition_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let type_ = ctor_type(agent, a, "CompositionEvent")?;
    let dict = a.arg(1);
    let init = event_init(agent, &dict)?;
    let (view, detail) = ui_init(agent, &dict)?;
    let mut slots = vec![view, Value::Undefined];
    slots.extend(read_members(agent, &dict, COMPOSITION)?);
    let ev = Event::with_kind(type_, init, EventKind::Ui { detail });
    let c = classes(agent).composition_event;
    construct_event_vec(agent, a, EventIface::Class(c), ev, slots)
}

/// `new WheelEvent(type, dict)`: `MouseEventInit` y los deltas.
fn wheel_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let type_ = ctor_type(agent, a, "WheelEvent")?;
    let dict = a.arg(1);
    let (init, view, data) = mouse_parts(agent, &dict)?;
    let mut slots = vec![view, Value::Undefined];
    slots.extend(read_members(agent, &dict, WHEEL)?);
    let ev = Event::with_kind(type_, init, EventKind::Mouse(data));
    let c = classes(agent).wheel_event;
    construct_event_vec(agent, a, EventIface::Class(c), ev, slots)
}

/// Reinicia el evento (los `init*Event` heredados) y pone los slots desde el 2; no hace nada
/// durante el dispatch.
fn legacy_init(agent: &mut Agent, a: &CallArgs<'_>, values: Vec<Value>) -> JsResult<Value> {
    let type_ = crate::dom::to_dom_string(agent, &a.arg(0))?;
    let bubbles = bf_js::host::webidl::to_boolean(agent, &a.arg(1));
    let cancelable = bf_js::host::webidl::to_boolean(agent, &a.arg(2));
    if with_event(agent, a, Event::is_dispatching)? {
        return Ok(Value::Undefined);
    }
    with_event_mut(agent, a, |e| e.init_event(type_, bubbles, cancelable))?;
    if let Value::Object(o) = a.this
        && let Some(s) = agent.host_slots_mut(o)
    {
        s.truncate(2);
        s.resize(2, Value::Undefined);
        s.extend(values);
    }
    Ok(Value::Undefined)
}

/// `initMessageEvent(type, bubbles, cancelable, data, origin, lastEventId, source, ports)`
/// (HTML §9.3.1): los slots siguen el orden de [`MESSAGE`].
fn init_message_event(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    required(agent, a, 1, "initMessageEvent")?;
    let data = convert(agent, a.arg(3), Ty::Any)?;
    let origin = convert(agent, a.arg(4), Ty::Str)?;
    let last = convert(agent, a.arg(5), Ty::Str)?;
    let source = convert(agent, a.arg(6), Ty::Any)?;
    let ports = convert(agent, a.arg(7), Ty::Seq)?;
    legacy_init(agent, a, vec![data, last, origin, ports, source])
}

/// `initStorageEvent(type, bubbles, cancelable, key, oldValue, newValue, url, storageArea)`.
fn init_storage_event(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    required(agent, a, 1, "initStorageEvent")?;
    let key = convert(agent, a.arg(3), Ty::NullStr)?;
    let old = convert(agent, a.arg(4), Ty::NullStr)?;
    let new = convert(agent, a.arg(5), Ty::NullStr)?;
    let url = convert(agent, a.arg(6), Ty::Str)?;
    let area = convert(agent, a.arg(7), Ty::Any)?;
    legacy_init(agent, a, vec![key, new, old, area, url])
}

/// `initCompositionEvent(type, bubbles, cancelable, view, data)`.
fn init_composition_event(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    required(agent, a, 1, "initCompositionEvent")?;
    let data = convert(agent, a.arg(4), Ty::Str)?;
    legacy_init(agent, a, vec![data])?;
    if let Value::Object(o) = a.this
        && let Some(s) = agent.host_slots_mut(o)
    {
        s[0] = if a.arg(3).is_undefined() {
            Value::Null
        } else {
            a.arg(3)
        };
    }
    Ok(Value::Undefined)
}
