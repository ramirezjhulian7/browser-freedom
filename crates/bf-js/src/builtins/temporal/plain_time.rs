//! Temporal.PlainTime: una hora del día sin fecha ni zona.

use super::duration::{DateDuration, Internal, difference_time};
use super::iso::{self, Time};
use super::options::{RoundingMode, Unit, UnitGroup, UnitOption};
use super::parse::{self, Goal};
use super::{TemporalData, class, create, data_of, getters, js_str};
use crate::builtins::FnEntry;
use crate::heap::Gc;
use crate::object::Object;
use crate::value::Value;
use crate::vm::function::{CallArgs, NativeFn};
use crate::vm::{Agent, JsResult};

pub fn init(agent: &mut Agent, temporal: Gc<Object>) -> Gc<Object> {
    const STATICS: &[FnEntry] = &[("from", 1, time_from), ("compare", 2, time_compare)];
    let (_, proto) = class(agent, temporal, "PlainTime", 0, time_ctor, STATICS);
    const G: &[(&str, NativeFn)] = &[
        ("hour", g_hour),
        ("minute", g_minute),
        ("second", g_second),
        ("millisecond", g_millisecond),
        ("microsecond", g_microsecond),
        ("nanosecond", g_nanosecond),
    ];
    getters(agent, proto, G);
    const METHODS: &[FnEntry] = &[
        ("add", 1, time_add),
        ("subtract", 1, time_subtract),
        ("with", 1, time_with),
        ("until", 1, time_until),
        ("since", 1, time_since),
        ("round", 1, time_round),
        ("equals", 1, time_equals),
        ("toString", 0, time_to_string),
        ("toLocaleString", 0, time_to_locale_string),
        ("toJSON", 0, time_to_json),
        ("valueOf", 0, super::value_of),
    ];
    crate::builtins::methods(agent, proto, METHODS);
    proto
}

fn this_time(agent: &mut Agent, this: &Value, method: &str) -> JsResult<Time> {
    match super::this_data(agent, this, "PlainTime", method, |d| {
        matches!(d, TemporalData::PlainTime(_))
    })? {
        TemporalData::PlainTime(t) => Ok(t),
        _ => unreachable!(),
    }
}

/// Un campo de hora por nombre.
pub fn time_field(t: Time, name: &str) -> Value {
    Value::Number(match name {
        "hour" => f64::from(t.hour),
        "minute" => f64::from(t.minute),
        "second" => f64::from(t.second),
        "millisecond" => f64::from(t.millisecond),
        "microsecond" => f64::from(t.microsecond),
        _ => f64::from(t.nanosecond),
    })
}

macro_rules! tg {
    ($f:ident, $name:literal) => {
        fn $f(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
            let t = this_time(agent, &a.this, $name)?;
            Ok(time_field(t, $name))
        }
    };
}

tg!(g_hour, "hour");
tg!(g_minute, "minute");
tg!(g_second, "second");
tg!(g_millisecond, "millisecond");
tg!(g_microsecond, "microsecond");
tg!(g_nanosecond, "nanosecond");

impl Agent {
    /// ToTemporalTime.
    pub fn to_temporal_time(&mut self, item: &Value, options: &Value) -> JsResult<Time> {
        if let Value::Object(o) = item {
            match data_of(self, item) {
                Some(TemporalData::PlainTime(t)) => {
                    self.temporal_overflow_of(options)?;
                    return Ok(t);
                }
                Some(TemporalData::PlainDateTime(dt, _)) => {
                    self.temporal_overflow_of(options)?;
                    return Ok(dt.time);
                }
                Some(TemporalData::ZonedDateTime { ns, tz, .. }) => {
                    let dt = super::date_time_for(&tz, ns);
                    self.temporal_overflow_of(options)?;
                    return Ok(dt.time);
                }
                _ => {}
            }
            let r = self.temporal_time_record(*o, false)?;
            let reject = self.temporal_overflow_of(options)?;
            return self.temporal_regulate_time(r.map(|v| v.unwrap_or(0.0)), reject);
        }
        let Value::String(s) = item else {
            return self.type_error("time must be an object or a string");
        };
        let s = s.to_string_lossy();
        // ParseTemporalTimeString.
        let Some(r) = parse::parse_iso_date_time(&s, &[Goal::Time]) else {
            return self.range_error(&format!("Invalid time: {s}"));
        };
        if r.z {
            return self.range_error("a time string cannot have a UTC designator");
        }
        self.temporal_overflow_of(options)?;
        Ok(r.time.unwrap_or_default())
    }
}

/// Temporal.PlainTime ( [ hour … nanosecond ] ).
fn time_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor Temporal.PlainTime requires 'new'");
    };
    let mut f = [0f64; 6];
    for (k, v) in f.iter_mut().enumerate() {
        let x = a.arg(k);
        if !x.is_undefined() {
            *v = agent.to_integer_with_truncation(&x)?;
        }
    }
    if !iso::is_valid_time(f[0], f[1], f[2], f[3], f[4], f[5]) {
        return agent.range_error("invalid time");
    }
    let t = agent.temporal_regulate_time(f, true)?;
    create(agent, TemporalData::PlainTime(t), Some(nt))
}

fn time_from(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = agent.to_temporal_time(&a.arg(0), &a.arg(1))?;
    create(agent, TemporalData::PlainTime(t), None)
}

fn time_compare(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let one = agent.to_temporal_time(&a.arg(0), &Value::Undefined)?;
    let two = agent.to_temporal_time(&a.arg(1), &Value::Undefined)?;
    Ok(Value::Number(f64::from(iso::sign_i128(
        one.nanos() - two.nanos(),
    ))))
}

/// AddDurationToTime.
fn add(agent: &mut Agent, a: &CallArgs<'_>, subtract: bool) -> JsResult<Value> {
    let t = this_time(agent, &a.this, if subtract { "subtract" } else { "add" })?;
    let mut d = agent.to_temporal_duration(&a.arg(0))?;
    if subtract {
        d = d.negated();
    }
    let (_, r) = iso::balance_time(t.nanos() + d.internal().time);
    create(agent, TemporalData::PlainTime(r), None)
}

fn time_add(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    add(agent, a, false)
}

fn time_subtract(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    add(agent, a, true)
}

/// Temporal.PlainTime.prototype.with ( temporalTimeLike [ , options ] ).
fn time_with(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = this_time(agent, &a.this, "with")?;
    let like = a.arg(0);
    if !agent.temporal_is_partial(&like)? {
        return agent.type_error("with() requires a partial time object");
    }
    let Value::Object(o) = like else {
        unreachable!()
    };
    let p = agent.temporal_time_record(o, true)?;
    let cur = [
        f64::from(t.hour),
        f64::from(t.minute),
        f64::from(t.second),
        f64::from(t.millisecond),
        f64::from(t.microsecond),
        f64::from(t.nanosecond),
    ];
    let mut f = cur;
    for k in 0..6 {
        if let Some(v) = p[k] {
            f[k] = v;
        }
    }
    let reject = agent.temporal_overflow_of(&a.arg(1))?;
    let r = agent.temporal_regulate_time(f, reject)?;
    create(agent, TemporalData::PlainTime(r), None)
}

/// DifferenceTemporalPlainTime.
fn difference(agent: &mut Agent, a: &CallArgs<'_>, since: bool) -> JsResult<Value> {
    let t = this_time(agent, &a.this, if since { "since" } else { "until" })?;
    let other = agent.to_temporal_time(&a.arg(0), &Value::Undefined)?;
    let o = agent.temporal_options_object(&a.arg(1))?;
    let (largest, smallest, increment, mode) = agent.temporal_difference_settings(
        since,
        o,
        UnitGroup::Time,
        &[],
        Unit::Nanosecond,
        Unit::Hour,
    )?;
    let diff = difference_time(t, other);
    let rounded = agent.temporal_round_time_duration(diff, increment, smallest, mode)?;
    let mut r = agent.temporal_from_internal(
        Internal {
            date: DateDuration::default(),
            time: rounded,
        },
        largest,
    )?;
    if since {
        r = r.negated();
    }
    agent.temporal_create_duration(r.0, None)
}

fn time_until(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    difference(agent, a, false)
}

fn time_since(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    difference(agent, a, true)
}

/// Temporal.PlainTime.prototype.round ( roundTo ).
fn time_round(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = this_time(agent, &a.this, "round")?;
    let round_to = super::duration::string_or_options(agent, &a.arg(0), "smallestUnit")?;
    let increment = agent.temporal_rounding_increment(round_to)?;
    let mode = agent.temporal_rounding_mode(round_to, RoundingMode::HalfExpand)?;
    let smallest = agent.temporal_unit_option(round_to, "smallestUnit", true)?;
    agent.temporal_validate_unit(smallest, UnitGroup::Time, false, false)?;
    let UnitOption::Unit(unit) = smallest else {
        return agent.range_error("smallestUnit is required");
    };
    let max = unit.max_increment().expect("unidad de hora");
    agent.temporal_validate_increment(increment, max, false)?;
    let (_, r) = super::round_time(t, increment, unit, mode);
    create(agent, TemporalData::PlainTime(r), None)
}

fn time_equals(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = this_time(agent, &a.this, "equals")?;
    let other = agent.to_temporal_time(&a.arg(0), &Value::Undefined)?;
    Ok(Value::Boolean(t == other))
}

fn time_to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = this_time(agent, &a.this, "toString")?;
    let o = agent.temporal_options_object(&a.arg(0))?;
    let digits = agent.temporal_fractional_digits(o)?;
    let mode = agent.temporal_rounding_mode(o, RoundingMode::Trunc)?;
    let smallest = agent.temporal_unit_option(o, "smallestUnit", false)?;
    agent.temporal_validate_unit(smallest, UnitGroup::Time, false, false)?;
    if smallest == UnitOption::Unit(Unit::Hour) {
        return agent.range_error("smallestUnit must be minutes or smaller");
    }
    let (precision, unit, increment) = super::seconds_precision(smallest, digits);
    let (_, r) = super::round_time(t, increment, unit, mode);
    Ok(js_str(&iso::time_string(r, precision)))
}

fn time_to_locale_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_time(agent, &a.this, "toLocaleString")?;
    crate::builtins::intl::date_time_format::temporal_to_locale_string(
        agent,
        &a.this,
        &a.arg(0),
        &a.arg(1),
        crate::builtins::intl::date_time_format::Required::Time,
        crate::builtins::intl::date_time_format::Defaults::Time,
        None,
    )
}

fn time_to_json(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = this_time(agent, &a.this, "toJSON")?;
    Ok(js_str(&iso::time_string(t, iso::Precision::Auto)))
}
