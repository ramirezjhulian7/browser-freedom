//! Temporal.Now: el instante y la zona horaria del sistema (SystemUTCEpochNanoseconds y
//! SystemTimeZoneIdentifier; sin base de datos de zonas, la del sistema es "UTC").

use super::{TemporalData, create, date_time_for};
use crate::builtins::{FnEntry, methods, to_string_tag};
use crate::heap::Gc;
use crate::object::Object;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult};

pub fn init(agent: &mut Agent, temporal: Gc<Object>) {
    let op = agent.intrinsics().object_prototype;
    let now = agent.new_object(Some(op));
    to_string_tag(agent, now, "Temporal.Now");
    const M: &[FnEntry] = &[
        ("instant", 0, now_instant),
        ("timeZoneId", 0, now_time_zone_id),
        ("zonedDateTimeISO", 0, now_zoned_date_time_iso),
        ("plainDateTimeISO", 0, now_plain_date_time_iso),
        ("plainDateISO", 0, now_plain_date_iso),
        ("plainTimeISO", 0, now_plain_time_iso),
    ];
    methods(agent, now, M);
    agent.put_data(
        temporal,
        PropertyKey::from("Now"),
        Value::Object(now),
        true,
        false,
        true,
    );
}

/// SystemUTCEpochNanoseconds.
fn system_ns() -> i128 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i128,
        Err(e) => -(e.duration().as_nanos() as i128),
    }
}

/// SystemTimeZoneIdentifier.
const SYSTEM_TZ: &str = "UTC";

fn now_instant(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    agent.temporal_create_instant(system_ns(), None)
}

fn now_time_zone_id(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(super::js_str(SYSTEM_TZ))
}

/// La zona del argumento opcional (undefined: la del sistema).
fn zone_arg(agent: &mut Agent, v: &Value) -> JsResult<String> {
    if v.is_undefined() {
        Ok(SYSTEM_TZ.to_owned())
    } else {
        agent.to_temporal_time_zone(v)
    }
}

fn now_zoned_date_time_iso(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let tz = zone_arg(agent, &a.arg(0))?;
    create(
        agent,
        TemporalData::ZonedDateTime {
            ns: system_ns(),
            tz,
        },
        None,
    )
}

fn now_plain_date_time_iso(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let tz = zone_arg(agent, &a.arg(0))?;
    agent.temporal_create_date_time(date_time_for(&tz, system_ns()), None)
}

fn now_plain_date_iso(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let tz = zone_arg(agent, &a.arg(0))?;
    agent.temporal_create_date(date_time_for(&tz, system_ns()).date, None)
}

fn now_plain_time_iso(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let tz = zone_arg(agent, &a.arg(0))?;
    create(
        agent,
        TemporalData::PlainTime(date_time_for(&tz, system_ns()).time),
        None,
    )
}
