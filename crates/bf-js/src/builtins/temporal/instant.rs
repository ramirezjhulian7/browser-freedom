//! Temporal.Instant: un punto exacto en el tiempo (`[[EpochNanoseconds]]`).

use super::iso::{self, IsoDateTime, Precision};
use super::options::{RoundingMode, Unit, UnitGroup, UnitOption, round_to_increment};
use super::parse::{self, Goal};
use super::{TemporalData, bigint_to_i128, bigint_value, class, create, data_of, getters, js_str};
use crate::abstract_ops::conversion::Hint;
use crate::builtins::FnEntry;
use crate::heap::Gc;
use crate::object::Object;
use crate::value::Value;
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult};

pub fn init(agent: &mut Agent, temporal: Gc<Object>) -> Gc<Object> {
    const STATICS: &[FnEntry] = &[
        ("from", 1, instant_from),
        ("fromEpochMilliseconds", 1, from_epoch_ms),
        ("fromEpochNanoseconds", 1, from_epoch_ns),
        ("compare", 2, instant_compare),
    ];
    let (_, proto) = class(agent, temporal, "Instant", 1, instant_ctor, STATICS);
    getters(
        agent,
        proto,
        &[
            ("epochMilliseconds", get_epoch_ms),
            ("epochNanoseconds", get_epoch_ns),
        ],
    );
    const METHODS: &[FnEntry] = &[
        ("add", 1, instant_add),
        ("subtract", 1, instant_subtract),
        ("until", 1, instant_until),
        ("since", 1, instant_since),
        ("round", 1, instant_round),
        ("equals", 1, instant_equals),
        ("toString", 0, instant_to_string),
        ("toLocaleString", 0, instant_to_locale_string),
        ("toJSON", 0, instant_to_json),
        ("valueOf", 0, super::value_of),
        ("toZonedDateTimeISO", 1, to_zoned_date_time_iso),
    ];
    crate::builtins::methods(agent, proto, METHODS);
    proto
}

fn this_instant(agent: &mut Agent, this: &Value, method: &str) -> JsResult<i128> {
    match super::this_data(agent, this, "Instant", method, |d| {
        matches!(d, TemporalData::Instant(_))
    })? {
        TemporalData::Instant(ns) => Ok(ns),
        _ => unreachable!(),
    }
}

impl Agent {
    /// CreateTemporalInstant con la validación de IsValidEpochNanoseconds.
    pub fn temporal_create_instant(
        &mut self,
        ns: i128,
        new_target: Option<Gc<Object>>,
    ) -> JsResult<Value> {
        if !iso::is_valid_epoch_ns(ns) {
            return self.range_error("instant is outside the representable range");
        }
        create(self, TemporalData::Instant(ns), new_target)
    }

    /// ToTemporalInstant: los nanosegundos.
    pub fn to_temporal_instant(&mut self, item: &Value) -> JsResult<i128> {
        match data_of(self, item) {
            Some(TemporalData::Instant(ns)) | Some(TemporalData::ZonedDateTime { ns, .. }) => {
                return Ok(ns);
            }
            _ => {}
        }
        let p = self.to_primitive(item, Hint::String)?;
        let Value::String(s) = p else {
            return self.type_error("instant must be a string");
        };
        let s = s.to_string_lossy();
        // ParseTemporalInstantString.
        let Some(r) = parse::parse_iso_date_time(&s, &[Goal::Instant]) else {
            return self.range_error(&format!("Invalid instant: {s}"));
        };
        let offset = if r.z {
            0
        } else {
            parse::parse_offset_ns(r.offset.as_deref().unwrap_or("")).unwrap_or(0)
        };
        let date = iso::IsoDate::new(r.year.unwrap_or(0), r.month, r.day.unwrap_or(1));
        let dt = IsoDateTime {
            date,
            time: r.time.unwrap_or_default(),
        };
        let ns = dt.epoch_ns() - i128::from(offset);
        let balanced = IsoDateTime::from_epoch_ns(ns);
        self.temporal_check_days_range(balanced.date)?;
        if !iso::is_valid_epoch_ns(ns) {
            return self.range_error("instant is outside the representable range");
        }
        Ok(ns)
    }
}

/// Temporal.Instant ( epochNanoseconds ).
fn instant_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor Temporal.Instant requires 'new'");
    };
    let b = agent.to_bigint(&a.arg(0))?;
    let ns = bigint_to_i128(&agent.heap[b]).unwrap_or(i128::MAX);
    agent.temporal_create_instant(ns, Some(nt))
}

fn instant_from(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let ns = agent.to_temporal_instant(&a.arg(0))?;
    agent.temporal_create_instant(ns, None)
}

/// Temporal.Instant.fromEpochMilliseconds ( epochMilliseconds ).
fn from_epoch_ms(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = agent.to_number(&a.arg(0))?;
    // NumberToBigInt.
    if !n.is_finite() || n.trunc() != n {
        return agent.range_error("epochMilliseconds must be an integer");
    }
    if n.abs() > 8.64e15 {
        return agent.range_error("instant is outside the representable range");
    }
    agent.temporal_create_instant((n as i128) * 1_000_000, None)
}

/// Temporal.Instant.fromEpochNanoseconds ( epochNanoseconds ).
fn from_epoch_ns(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let b = agent.to_bigint(&a.arg(0))?;
    let ns = bigint_to_i128(&agent.heap[b]).unwrap_or(i128::MAX);
    agent.temporal_create_instant(ns, None)
}

fn instant_compare(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let one = agent.to_temporal_instant(&a.arg(0))?;
    let two = agent.to_temporal_instant(&a.arg(1))?;
    Ok(Value::Number(f64::from(iso::sign_i128(one - two))))
}

fn get_epoch_ms(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let ns = this_instant(agent, &a.this, "epochMilliseconds")?;
    Ok(Value::Number(ns.div_euclid(1_000_000) as f64))
}

fn get_epoch_ns(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let ns = this_instant(agent, &a.this, "epochNanoseconds")?;
    Ok(bigint_value(agent, ns))
}

/// AddDurationToInstant.
fn add(agent: &mut Agent, a: &CallArgs<'_>, subtract: bool) -> JsResult<Value> {
    let ns = this_instant(agent, &a.this, if subtract { "subtract" } else { "add" })?;
    let mut d = agent.to_temporal_duration(&a.arg(0))?;
    if subtract {
        d = d.negated();
    }
    if d.default_largest_unit().is_date() {
        return agent.range_error("cannot add calendar units or days to an Instant");
    }
    let t = d.internal_24h().time;
    let r = agent.temporal_add_instant(ns, t)?;
    agent.temporal_create_instant(r, None)
}

fn instant_add(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    add(agent, a, false)
}

fn instant_subtract(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    add(agent, a, true)
}

/// DifferenceTemporalInstant.
fn difference(agent: &mut Agent, a: &CallArgs<'_>, since: bool) -> JsResult<Value> {
    let ns = this_instant(agent, &a.this, if since { "since" } else { "until" })?;
    let other = agent.to_temporal_instant(&a.arg(0))?;
    let o = agent.temporal_options_object(&a.arg(1))?;
    let (largest, smallest, increment, mode) = agent.temporal_difference_settings(
        since,
        o,
        UnitGroup::Time,
        &[],
        Unit::Nanosecond,
        Unit::Second,
    )?;
    let d = agent.temporal_diff_instant(ns, other, increment, smallest, mode)?;
    let mut r = agent.temporal_from_internal(d, largest)?;
    if since {
        r = r.negated();
    }
    agent.temporal_create_duration(r.0, None)
}

fn instant_until(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    difference(agent, a, false)
}

fn instant_since(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    difference(agent, a, true)
}

/// El máximo de Temporal.Instant.prototype.round paso 11 (divisiones de un día).
pub(crate) fn day_divisor(unit: Unit) -> i128 {
    iso::NS_PER_DAY / unit.nanos()
}

/// Temporal.Instant.prototype.round ( roundTo ).
fn instant_round(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let ns = this_instant(agent, &a.this, "round")?;
    let round_to = super::duration::string_or_options(agent, &a.arg(0), "smallestUnit")?;
    let increment = agent.temporal_rounding_increment(round_to)?;
    let mode = agent.temporal_rounding_mode(round_to, RoundingMode::HalfExpand)?;
    let smallest = agent.temporal_unit_option(round_to, "smallestUnit", true)?;
    agent.temporal_validate_unit(smallest, UnitGroup::Time, false, false)?;
    let UnitOption::Unit(unit) = smallest else {
        return agent.range_error("smallestUnit is required");
    };
    agent.temporal_validate_increment(increment, day_divisor(unit), true)?;
    let r = super::options::round_as_if_positive(ns, increment * unit.nanos(), mode);
    agent.temporal_create_instant(r, None)
}

fn instant_equals(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let ns = this_instant(agent, &a.this, "equals")?;
    let other = agent.to_temporal_instant(&a.arg(0))?;
    Ok(Value::Boolean(ns == other))
}

/// TemporalInstantToString.
pub fn instant_string(ns: i128, tz: Option<&str>, precision: Precision) -> String {
    let zone = tz.unwrap_or("UTC");
    let dt = super::date_time_for(zone, ns);
    let s = iso::date_time_string(dt, precision);
    match tz {
        None => format!("{s}Z"),
        Some(z) => {
            let off = round_to_increment(
                super::offset_ns(z),
                60_000_000_000,
                RoundingMode::HalfExpand,
            );
            format!(
                "{s}{}",
                iso::format_offset_minutes((off / 60_000_000_000) as i64)
            )
        }
    }
}

/// Temporal.Instant.prototype.toString ( [ options ] ).
fn instant_to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let ns = this_instant(agent, &a.this, "toString")?;
    let o = agent.temporal_options_object(&a.arg(0))?;
    let digits = agent.temporal_fractional_digits(o)?;
    let mode = agent.temporal_rounding_mode(o, RoundingMode::Trunc)?;
    let smallest = agent.temporal_unit_option(o, "smallestUnit", false)?;
    let tz = agent.get(
        o,
        &crate::value::PropertyKey::from("timeZone"),
        Value::Object(o),
    )?;
    agent.temporal_validate_unit(smallest, UnitGroup::Time, false, false)?;
    if smallest == UnitOption::Unit(Unit::Hour) {
        return agent.range_error("smallestUnit must be minutes or smaller");
    }
    let tz = if tz.is_undefined() {
        None
    } else {
        Some(agent.to_temporal_time_zone(&tz)?)
    };
    let (precision, unit, increment) = super::seconds_precision(smallest, digits);
    let r = super::options::round_as_if_positive(ns, increment * unit.nanos(), mode);
    if !iso::is_valid_epoch_ns(r) {
        return agent.range_error("instant is outside the representable range");
    }
    Ok(js_str(&instant_string(r, tz.as_deref(), precision)))
}

fn instant_to_locale_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let ns = this_instant(agent, &a.this, "toLocaleString")?;
    Ok(js_str(&instant_string(ns, None, Precision::Auto)))
}

fn instant_to_json(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let ns = this_instant(agent, &a.this, "toJSON")?;
    Ok(js_str(&instant_string(ns, None, Precision::Auto)))
}

/// Temporal.Instant.prototype.toZonedDateTimeISO ( timeZone ).
fn to_zoned_date_time_iso(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let ns = this_instant(agent, &a.this, "toZonedDateTimeISO")?;
    let tz = agent.to_temporal_time_zone(&a.arg(0))?;
    create(agent, TemporalData::ZonedDateTime { ns, tz }, None)
}
