//! `performance` (High Resolution Time §5: `now()`, `timeOrigin`), los `performance.timing` y
//! `performance.navigation` de Navigation Timing nivel 1 (en desuso, pero los sitios los leen)
//! y `requestIdleCallback` (Cooperative Scheduling of Background Tasks §4).

use bf_js::host::{HostClass, HostClassId, TimerHandler};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use super::{bom_classes, state};
use crate::WebContextHandle;

/// Cuánto tiempo de inactividad se le ofrece a un callback de `requestIdleCallback`.
const IDLE_PERIOD: f64 = 50.0;

/// Milisegundos desde la época Unix.
pub fn epoch_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

/// Los instantes de la carga (Navigation Timing nivel 1 §4.1), en ms desde la época Unix.
/// Los que todavía no ocurrieron valen 0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timing {
    pub navigation_start: f64,
    pub fetch_start: f64,
    pub response_end: f64,
    pub dom_loading: f64,
    pub dom_interactive: f64,
    pub dom_content_loaded: f64,
    pub dom_complete: f64,
    pub load_event: f64,
}

impl Timing {
    pub fn starting_at(t: f64) -> Timing {
        Timing {
            navigation_start: t,
            fetch_start: t,
            response_end: t,
            dom_loading: t,
            dom_interactive: 0.0,
            dom_content_loaded: 0.0,
            dom_complete: 0.0,
            load_event: 0.0,
        }
    }
}

/// El parseo del documento de `realm` terminó (`domInteractive` … `domComplete` pasan a
/// ahora).
pub fn document_loaded(agent: &mut Agent, realm: bf_js::vm::RealmId) {
    let now = epoch_ms();
    if let Some(s) = agent.realm_data_mut::<super::BomState>(realm) {
        let t = &mut s.timing;
        for f in [
            &mut t.dom_interactive,
            &mut t.dom_content_loaded,
            &mut t.dom_complete,
        ] {
            if *f == 0.0 {
                *f = now;
            }
        }
    }
}

/// El evento `load` del documento de `realm` (`loadEventStart`/`End`).
pub fn load_finished(agent: &mut Agent, realm: bf_js::vm::RealmId) {
    let now = epoch_ms();
    if let Some(s) = agent.realm_data_mut::<super::BomState>(realm) {
        for f in [&mut s.timing.load_event] {
            if *f == 0.0 {
                *f = now;
            }
        }
    }
}

pub fn performance_class(event_target: HostClassId) -> HostClass {
    HostClass::new("Performance")
        .parent(event_target)
        .attribute("timeOrigin", Some(time_origin), None)
        .attribute("timing", Some(timing), None)
        .attribute("navigation", Some(navigation), None)
        .attribute("memory", Some(memory), None)
        .method("now", 0, now)
        .method("getEntries", 0, empty_list)
        .method("getEntriesByType", 1, empty_list)
        .method("getEntriesByName", 1, empty_list)
        .method("mark", 1, undefined)
        .method("measure", 1, undefined)
        .method("clearMarks", 0, undefined)
        .method("clearMeasures", 0, undefined)
        .method("clearResourceTimings", 0, undefined)
        .method("setResourceTimingBufferSize", 1, undefined)
        .method("toJSON", 0, performance_to_json)
}

pub fn timing_class() -> HostClass {
    let fields: [(&'static str, bf_js::vm::function::NativeFn); 21] = [
        ("navigationStart", t_navigation_start),
        ("unloadEventStart", t_zero),
        ("unloadEventEnd", t_zero),
        ("redirectStart", t_zero),
        ("redirectEnd", t_zero),
        ("fetchStart", t_fetch_start),
        ("domainLookupStart", t_fetch_start),
        ("domainLookupEnd", t_fetch_start),
        ("connectStart", t_fetch_start),
        ("connectEnd", t_fetch_start),
        ("secureConnectionStart", t_fetch_start),
        ("requestStart", t_fetch_start),
        ("responseStart", t_response_end),
        ("responseEnd", t_response_end),
        ("domLoading", t_dom_loading),
        ("domInteractive", t_dom_interactive),
        ("domContentLoadedEventStart", t_dom_content_loaded),
        ("domContentLoadedEventEnd", t_dom_content_loaded),
        ("domComplete", t_dom_complete),
        ("loadEventStart", t_load_event),
        ("loadEventEnd", t_load_event),
    ];
    let mut c = HostClass::new("PerformanceTiming");
    for (name, f) in fields {
        c = c.attribute(name, Some(f), None);
    }
    c.method("toJSON", 0, timing_to_json)
}

pub fn navigation_class() -> HostClass {
    HostClass::new("PerformanceNavigation")
        .constant("TYPE_NAVIGATE", 0.0)
        .constant("TYPE_RELOAD", 1.0)
        .constant("TYPE_BACK_FORWARD", 2.0)
        .constant("TYPE_RESERVED", 255.0)
        .attribute("type", Some(t_zero), None)
        .attribute("redirectCount", Some(t_zero), None)
}

pub fn idle_deadline_class() -> HostClass {
    HostClass::new("IdleDeadline")
        .attribute("didTimeout", Some(did_timeout), None)
        .method("timeRemaining", 0, time_remaining)
}

/// `performance.now()`: ms desde el `timeOrigin`, con resolución de 0.1 ms.
fn now(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let t = agent.host.hooks.now() - state(agent).now_origin;
    Ok(Value::Number((t * 10.0).floor() / 10.0))
}

fn time_origin(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(state(agent).time_origin))
}

/// Un objeto de la ventana guardado en el slot `i` de `this` (o creado la primera vez).
fn slot_object(agent: &mut Agent, this: &Value, i: usize, class: HostClassId) -> Value {
    let Value::Object(o) = this else {
        return Value::Null;
    };
    if let Some(v) = agent
        .host_slots_mut(*o)
        .and_then(|s| s.get(i).cloned())
        .filter(|v| !v.is_undefined())
    {
        return v;
    }
    let v = Value::Object(agent.host_create(class, Box::new(())));
    if let Some(slots) = agent.host_slots_mut(*o) {
        while slots.len() <= i {
            slots.push(Value::Undefined);
        }
        slots[i] = v.clone();
    }
    v
}

fn timing(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let class = bom_classes(agent).performance_timing;
    Ok(slot_object(agent, &a.this, 0, class))
}

fn navigation(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let class = bom_classes(agent).performance_navigation;
    Ok(slot_object(agent, &a.this, 1, class))
}

/// `performance.memory` (no estándar, de Chrome): valores fijos razonables.
fn memory(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let o = agent.new_plain_object();
    for (k, v) in [
        ("jsHeapSizeLimit", 4_294_705_152.0),
        ("totalJSHeapSize", 10_000_000.0),
        ("usedJSHeapSize", 8_000_000.0),
    ] {
        agent.put_data(o, PropertyKey::from(k), Value::Number(v), true, true, true);
    }
    Ok(Value::Object(o))
}

fn empty_list(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Object(agent.create_array_from_list(&[])?))
}

fn undefined(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Undefined)
}

fn performance_to_json(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = agent.new_plain_object();
    let origin = state(agent).time_origin;
    agent.put_data(
        o,
        PropertyKey::from("timeOrigin"),
        Value::Number(origin),
        true,
        true,
        true,
    );
    let t = timing(agent, a)?;
    let t = timing_to_json(
        agent,
        &CallArgs {
            this: t,
            args: &[],
            new_target: None,
            callee: a.callee,
        },
    )?;
    agent.put_data(o, PropertyKey::from("timing"), t, true, true, true);
    Ok(Value::Object(o))
}

fn timing_value(agent: &Agent, f: impl Fn(&Timing) -> f64) -> Value {
    Value::Number(f(&state(agent).timing).floor())
}

fn t_zero(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(0.0))
}

fn t_navigation_start(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(timing_value(agent, |t| t.navigation_start))
}

fn t_fetch_start(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(timing_value(agent, |t| t.fetch_start))
}

fn t_response_end(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(timing_value(agent, |t| t.response_end))
}

fn t_dom_loading(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(timing_value(agent, |t| t.dom_loading))
}

fn t_dom_interactive(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(timing_value(agent, |t| t.dom_interactive))
}

fn t_dom_content_loaded(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(timing_value(agent, |t| t.dom_content_loaded))
}

fn t_dom_complete(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(timing_value(agent, |t| t.dom_complete))
}

fn t_load_event(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(timing_value(agent, |t| t.load_event))
}

fn timing_to_json(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let t = state(agent).timing;
    let o = agent.new_plain_object();
    for (k, v) in [
        ("navigationStart", t.navigation_start),
        ("fetchStart", t.fetch_start),
        ("responseEnd", t.response_end),
        ("domLoading", t.dom_loading),
        ("domInteractive", t.dom_interactive),
        ("domContentLoadedEventStart", t.dom_content_loaded),
        ("domContentLoadedEventEnd", t.dom_content_loaded),
        ("domComplete", t.dom_complete),
        ("loadEventStart", t.load_event),
        ("loadEventEnd", t.load_event),
    ] {
        agent.put_data(
            o,
            PropertyKey::from(k),
            Value::Number(v.floor()),
            true,
            true,
            true,
        );
    }
    Ok(Value::Object(o))
}

/// El payload de un `IdleDeadline`: cuándo empezó el periodo y si venció el `timeout`.
#[derive(Debug, Clone, Copy)]
struct Deadline {
    start: f64,
    did_timeout: bool,
}

fn did_timeout(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = agent.host_unwrap::<Deadline>(&a.this).copied();
    Ok(Value::Boolean(d.is_some_and(|d| d.did_timeout)))
}

fn time_remaining(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(d) = agent.host_unwrap::<Deadline>(&a.this).copied() else {
        return Ok(Value::Number(0.0));
    };
    if d.did_timeout {
        return Ok(Value::Number(0.0));
    }
    let elapsed = agent.host.hooks.now() - d.start;
    Ok(Value::Number((IDLE_PERIOD - elapsed).max(0.0)))
}

/// La función que corre el temporizador de `requestIdleCallback`: llama al callback (slot 0)
/// con un `IdleDeadline`.
fn idle_trampoline(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let slots = bf_js::builtins::slots(agent, a.callee);
    let callback = slots.first().cloned().unwrap_or_default();
    let class = bom_classes(agent).idle_deadline;
    let start = agent.host.hooks.now();
    let deadline = agent.host_create(
        class,
        Box::new(Deadline {
            start,
            did_timeout: false,
        }),
    );
    agent.call(&callback, Value::Undefined, &[Value::Object(deadline)])
}

/// `requestIdleCallback(callback, options)`: el callback corre como tarea cuando el bucle
/// queda libre (aquí, en la próxima vuelta de los temporizadores).
pub fn request_idle_callback(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let callback = a.arg(0);
    if !agent.is_callable(&callback) {
        return agent.type_error(
            "Failed to execute 'requestIdleCallback' on 'Window': parameter 1 is not of type 'Function'.",
        );
    }
    let f = agent.create_builtin_with_slots(idle_trampoline, 1, "", vec![callback]);
    let realm = WebContextHandle::current(agent).realm;
    let id = agent.set_timer(
        realm,
        TimerHandler::Callback(Value::Object(f)),
        1.0,
        Vec::new(),
        false,
    );
    Ok(Value::Number(f64::from(id)))
}

pub fn cancel_idle_callback(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = agent.to_number(&a.arg(0))?;
    if id.is_finite() && id > 0.0 {
        agent.clear_timer(id as u32);
    }
    Ok(Value::Undefined)
}

/// El origen de `performance.now()` en el reloj del motor (lo fija [`super::register`]).
pub(crate) fn set_now_origin(agent: &mut Agent, realm: bf_js::vm::RealmId) {
    let now = agent.host.hooks.now();
    if let Some(s) = agent.realm_data_mut::<super::BomState>(realm) {
        s.now_origin = now;
    }
}
