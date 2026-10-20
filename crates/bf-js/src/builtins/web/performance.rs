//! `performance.now()` y `performance.timeOrigin` (High Resolution Time §4–§5): tiempo
//! monotónico desde la creación del realm, en milisegundos y redondeado a 0,1 ms
//! (coarsen time, §4.5.2, para contextos sin aislamiento cross-origin).

use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::builtins::web::crypto::illegal_constructor;
use crate::builtins::web::{WebData, interface};
use crate::object::{Object, ObjectKind, Prop, PropValue};
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

/// La resolución de coarsen time (100 µs).
const RESOLUTION_MS: f64 = 0.1;

fn process_start() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

fn monotonic_ms() -> f64 {
    process_start().elapsed().as_secs_f64() * 1000.0
}

fn coarsen(ms: f64) -> f64 {
    (ms / RESOLUTION_MS).floor() * RESOLUTION_MS
}

pub fn init(agent: &mut Agent, realm: RealmId) {
    let (_, p) = interface(
        agent,
        realm,
        "Performance",
        "Performance.prototype",
        0,
        illegal_constructor,
        None,
    );
    // El time origin del realm: el instante monotónico y la hora del reloj.
    let origin_mono = monotonic_ms();
    let origin_wall = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
    let slots = vec![Value::Number(origin_mono), Value::Number(origin_wall)];
    let now_fn = agent.create_builtin_with_slots(now, 0, "now", slots.clone());
    agent.put_data(
        p,
        PropertyKey::from("now"),
        Value::Object(now_fn),
        true,
        true,
        true,
    );
    let to_json_fn = agent.create_builtin_with_slots(to_json, 0, "toJSON", slots.clone());
    agent.put_data(
        p,
        PropertyKey::from("toJSON"),
        Value::Object(to_json_fn),
        true,
        true,
        true,
    );
    let getter = agent.create_builtin_with_slots(time_origin, 0, "get timeOrigin", slots);
    agent.heap[p].props.insert(
        PropertyKey::from("timeOrigin"),
        Prop {
            value: PropValue::Accessor {
                get: Some(getter),
                set: None,
            },
            enumerable: true,
            configurable: true,
        },
    );
    let perf = agent.alloc(Object::new(
        Some(p),
        ObjectKind::Web(Box::new(WebData::Registry(Box::default()))),
    ));
    crate::builtins::register_global(agent, realm, "performance", perf);
}

fn slot(agent: &Agent, a: &CallArgs<'_>, i: usize) -> f64 {
    crate::builtins::slots(agent, a.callee)
        .get(i)
        .and_then(Value::as_number)
        .unwrap_or(0.0)
}

/// `performance.now()` (§5.1).
fn now(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(coarsen(monotonic_ms() - slot(agent, a, 0))))
}

/// `performance.timeOrigin` (§5.2).
fn time_origin(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(coarsen(slot(agent, a, 1))))
}

/// `performance.toJSON()` (§5.3): `{ timeOrigin }`.
fn to_json(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = agent.new_plain_object();
    let t = coarsen(slot(agent, a, 1));
    agent.create_data_property_or_throw(o, PropertyKey::from("timeOrigin"), Value::Number(t))?;
    Ok(Value::Object(o))
}
