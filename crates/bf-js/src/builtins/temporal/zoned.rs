//! Temporal.ZonedDateTime: un instante exacto en una zona horaria y un calendario.

use super::iso::{self, IsoDate, IsoDateTime, Precision};
use super::options::{RoundingMode, Unit, UnitGroup, UnitOption, div_to_f64, round_to_increment};
use super::parse::{self, Goal};
use super::plain_time::time_field;
use super::{
    DATE_FIELDS, Field, Fields, ISO8601, Required, TIME_FIELDS, TemporalData, bigint_to_i128,
    bigint_value, calendar_annotation, class, create, data_of, date_time_for, getters, js_str,
};
use crate::builtins::FnEntry;
use crate::heap::Gc;
use crate::object::Object;
use crate::value::Value;
use crate::vm::function::{CallArgs, NativeFn};
use crate::vm::{Agent, JsResult};

fn this_zdt(agent: &mut Agent, this: &Value, method: &str) -> JsResult<(i128, String)> {
    match super::this_data(agent, this, "ZonedDateTime", method, |d| {
        matches!(d, TemporalData::ZonedDateTime { .. })
    })? {
        TemporalData::ZonedDateTime { ns, tz } => Ok((ns, tz)),
        _ => unreachable!(),
    }
}

fn this_date_part(agent: &mut Agent, this: &Value, method: &str) -> JsResult<IsoDate> {
    let (ns, tz) = this_zdt(agent, this, method)?;
    Ok(date_time_for(&tz, ns).date)
}

macro_rules! date_getters {
    ($($name:literal => $f:ident),* $(,)?) => {
        $(
            fn $f(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
                let d = this_date_part(agent, &a.this, $name)?;
                Ok(super::plain_date::calendar_field(d, $name))
            }
        )*
        const DATE_G: &[(&str, NativeFn)] = &[$(($name, $f)),*];
    };
}

date_getters!(
    "calendarId" => g_calendar_id, "era" => g_era, "eraYear" => g_era_year, "year" => g_year,
    "month" => g_month, "monthCode" => g_month_code, "day" => g_day,
    "dayOfWeek" => g_day_of_week, "dayOfYear" => g_day_of_year, "weekOfYear" => g_week_of_year,
    "yearOfWeek" => g_year_of_week, "daysInWeek" => g_days_in_week,
    "daysInMonth" => g_days_in_month, "daysInYear" => g_days_in_year,
    "monthsInYear" => g_months_in_year, "inLeapYear" => g_in_leap_year,
);

macro_rules! time_getters {
    ($($name:literal => $f:ident),* $(,)?) => {
        $(
            fn $f(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
                let (ns, tz) = this_zdt(agent, &a.this, $name)?;
                Ok(time_field(date_time_for(&tz, ns).time, $name))
            }
        )*
        const TIME_G: &[(&str, NativeFn)] = &[$(($name, $f)),*];
    };
}

time_getters!(
    "hour" => g_hour, "minute" => g_minute, "second" => g_second,
    "millisecond" => g_millisecond, "microsecond" => g_microsecond,
    "nanosecond" => g_nanosecond,
);

pub fn init(agent: &mut Agent, temporal: Gc<Object>) -> Gc<Object> {
    const STATICS: &[FnEntry] = &[("from", 1, zdt_from), ("compare", 2, zdt_compare)];
    let (_, proto) = class(agent, temporal, "ZonedDateTime", 2, zdt_ctor, STATICS);
    getters(agent, proto, DATE_G);
    getters(agent, proto, TIME_G);
    const G: &[(&str, NativeFn)] = &[
        ("timeZoneId", g_time_zone_id),
        ("epochMilliseconds", g_epoch_ms),
        ("epochNanoseconds", g_epoch_ns),
        ("hoursInDay", g_hours_in_day),
        ("offsetNanoseconds", g_offset_ns),
        ("offset", g_offset),
    ];
    getters(agent, proto, G);
    const METHODS: &[FnEntry] = &[
        ("with", 1, zdt_with),
        ("withPlainTime", 0, zdt_with_plain_time),
        ("withTimeZone", 1, zdt_with_time_zone),
        ("withCalendar", 1, zdt_with_calendar),
        ("add", 1, zdt_add),
        ("subtract", 1, zdt_subtract),
        ("until", 1, zdt_until),
        ("since", 1, zdt_since),
        ("round", 1, zdt_round),
        ("equals", 1, zdt_equals),
        ("toString", 0, zdt_to_string),
        ("toLocaleString", 0, zdt_to_locale_string),
        ("toJSON", 0, zdt_to_json),
        ("valueOf", 0, super::value_of),
        ("startOfDay", 0, zdt_start_of_day),
        ("getTimeZoneTransition", 1, zdt_get_time_zone_transition),
        ("toInstant", 0, zdt_to_instant),
        ("toPlainDate", 0, zdt_to_plain_date),
        ("toPlainTime", 0, zdt_to_plain_time),
        ("toPlainDateTime", 0, zdt_to_plain_date_time),
    ];
    crate::builtins::methods(agent, proto, METHODS);
    proto
}

fn zdt_value(agent: &mut Agent, ns: i128, tz: String) -> JsResult<Value> {
    create(agent, TemporalData::ZonedDateTime { ns, tz }, None)
}

impl Agent {
    /// ToTemporalZonedDateTime.
    pub fn to_temporal_zoned(&mut self, item: &Value, options: &Value) -> JsResult<(i128, String)> {
        let mut exact = false;
        let mut match_minutes = false;
        let (date, time, tz, offset, o) = if let Value::Object(item_o) = item {
            if let Some(TemporalData::ZonedDateTime { ns, tz }) = data_of(self, item) {
                let o = self.temporal_options_object(options)?;
                self.temporal_disambiguation(o)?;
                self.temporal_offset_option(o, "reject")?;
                self.temporal_overflow(o)?;
                return Ok((ns, tz));
            }
            let item_o = *item_o;
            self.temporal_calendar_with_default(item_o)?;
            let mut names = DATE_FIELDS.to_vec();
            names.extend_from_slice(TIME_FIELDS);
            names.push(Field::Offset);
            names.push(Field::TimeZone);
            let f =
                self.temporal_prepare_fields(item_o, &names, Required::List(&[Field::TimeZone]))?;
            let tz = f.time_zone.clone().expect("timeZone requerido");
            let offset = f.offset.clone();
            let o = self.temporal_options_object(options)?;
            self.temporal_disambiguation(o)?;
            let offset_option = self.temporal_offset_option(o, "reject")?;
            let reject = self.temporal_overflow(o)?;
            let dt = self.temporal_date_time_from_fields(f, reject)?;
            (dt.date, Some(dt.time), tz, offset, offset_option)
        } else {
            let Value::String(s) = item else {
                return self.type_error("zoned date-time must be an object or a string");
            };
            let s = s.to_string_lossy();
            let Some(r) = parse::parse_iso_date_time(&s, &[Goal::DateTime { zoned: true }]) else {
                return self.range_error(&format!("Invalid zoned date-time: {s}"));
            };
            let ann = r.tz_annotation.clone().expect("anotación de zona");
            let tz = self.to_temporal_time_zone(&js_str(&ann))?;
            if r.z {
                exact = true;
            }
            self.temporal_canonicalize_calendar(r.calendar.as_deref().unwrap_or(ISO8601))?;
            match_minutes = !r.offset_sub_minute;
            let o = self.temporal_options_object(options)?;
            self.temporal_disambiguation(o)?;
            let offset_option = self.temporal_offset_option(o, "reject")?;
            self.temporal_overflow(o)?;
            let date = IsoDate::new(r.year.unwrap_or(0), r.month, r.day.unwrap_or(1));
            (date, r.time, tz, r.offset.clone(), offset_option)
        };
        let offset_ns = if exact {
            None
        } else {
            offset.map(|o| i128::from(parse::parse_offset_ns(&o).unwrap_or(0)))
        };
        let ns = self.temporal_interpret_offset(
            date,
            time,
            exact,
            offset_ns,
            &tz,
            false,
            &o,
            match_minutes,
        )?;
        Ok((ns, tz))
    }
}

/// Temporal.ZonedDateTime ( epochNanoseconds, timeZone [ , calendar ] ).
fn zdt_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor Temporal.ZonedDateTime requires 'new'");
    };
    let b = agent.to_bigint(&a.arg(0))?;
    let ns = bigint_to_i128(&agent.heap[b]).unwrap_or(i128::MAX);
    if !iso::is_valid_epoch_ns(ns) {
        return agent.range_error("instant is outside the representable range");
    }
    let Value::String(s) = a.arg(1) else {
        return agent.type_error("time zone must be a string");
    };
    let s = s.to_string_lossy();
    let Some(id) = parse::parse_tz_identifier(&s) else {
        return agent.range_error(&format!("Invalid time zone: {s}"));
    };
    let tz = match id {
        parse::TzId::Offset(m) => iso::format_offset_minutes(m),
        parse::TzId::Name(_) => agent.to_temporal_time_zone(&js_str(&s))?,
    };
    agent.temporal_calendar_arg(&a.arg(2))?;
    create(agent, TemporalData::ZonedDateTime { ns, tz }, Some(nt))
}

fn zdt_from(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (ns, tz) = agent.to_temporal_zoned(&a.arg(0), &a.arg(1))?;
    zdt_value(agent, ns, tz)
}

fn zdt_compare(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (one, _) = agent.to_temporal_zoned(&a.arg(0), &Value::Undefined)?;
    let (two, _) = agent.to_temporal_zoned(&a.arg(1), &Value::Undefined)?;
    Ok(Value::Number(f64::from(iso::sign_i128(one - two))))
}

fn g_time_zone_id(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (_, tz) = this_zdt(agent, &a.this, "timeZoneId")?;
    Ok(js_str(&tz))
}

fn g_epoch_ms(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (ns, _) = this_zdt(agent, &a.this, "epochMilliseconds")?;
    Ok(Value::Number(ns.div_euclid(1_000_000) as f64))
}

fn g_epoch_ns(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (ns, _) = this_zdt(agent, &a.this, "epochNanoseconds")?;
    Ok(bigint_value(agent, ns))
}

fn g_hours_in_day(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (ns, tz) = this_zdt(agent, &a.this, "hoursInDay")?;
    let today = date_time_for(&tz, ns).date;
    let tomorrow = today.add_days(1);
    let t0 = agent.temporal_start_of_day(&tz, today)?;
    let t1 = agent.temporal_start_of_day(&tz, tomorrow)?;
    Ok(Value::Number(div_to_f64(t1 - t0, Unit::Hour.nanos())))
}

fn g_offset_ns(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (_, tz) = this_zdt(agent, &a.this, "offsetNanoseconds")?;
    Ok(Value::Number(super::offset_ns(&tz) as f64))
}

fn g_offset(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (_, tz) = this_zdt(agent, &a.this, "offset")?;
    Ok(js_str(&iso::format_offset_ns(super::offset_ns(&tz) as i64)))
}

/// Temporal.ZonedDateTime.prototype.with ( temporalZonedDateTimeLike [ , options ] ).
fn zdt_with(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (ns, tz) = this_zdt(agent, &a.this, "with")?;
    let like = a.arg(0);
    if !agent.temporal_is_partial(&like)? {
        return agent.type_error("with() requires a partial zoned date-time object");
    }
    let Value::Object(o) = like else {
        unreachable!()
    };
    let offset = super::offset_ns(&tz);
    let dt = date_time_for(&tz, ns);
    let mut fields = Fields::from_date(dt.date, true, true);
    fields.set_time(dt.time);
    fields.offset = Some(iso::format_offset_ns(offset as i64));
    let mut names = DATE_FIELDS.to_vec();
    names.extend_from_slice(TIME_FIELDS);
    names.push(Field::Offset);
    let partial = agent.temporal_prepare_fields(o, &names, Required::Partial)?;
    let merged = fields.merge(partial);
    let opts = agent.temporal_options_object(&a.arg(1))?;
    agent.temporal_disambiguation(opts)?;
    let offset_option = agent.temporal_offset_option(opts, "prefer")?;
    let reject = agent.temporal_overflow(opts)?;
    let off = merged.offset.clone();
    let r = agent.temporal_date_time_from_fields(merged, reject)?;
    let new_offset =
        i128::from(parse::parse_offset_ns(off.as_deref().unwrap_or("+00:00")).unwrap_or(0));
    let ns = agent.temporal_interpret_offset(
        r.date,
        Some(r.time),
        false,
        Some(new_offset),
        &tz,
        false,
        &offset_option,
        false,
    )?;
    zdt_value(agent, ns, tz)
}

fn zdt_with_plain_time(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (ns, tz) = this_zdt(agent, &a.this, "withPlainTime")?;
    let dt = date_time_for(&tz, ns);
    let r = if a.arg(0).is_undefined() {
        agent.temporal_start_of_day(&tz, dt.date)?
    } else {
        let t = agent.to_temporal_time(&a.arg(0), &Value::Undefined)?;
        agent.temporal_epoch_ns_for(
            &tz,
            IsoDateTime {
                date: dt.date,
                time: t,
            },
        )?
    };
    zdt_value(agent, r, tz)
}

fn zdt_with_time_zone(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (ns, _) = this_zdt(agent, &a.this, "withTimeZone")?;
    let tz = agent.to_temporal_time_zone(&a.arg(0))?;
    zdt_value(agent, ns, tz)
}

fn zdt_with_calendar(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (ns, tz) = this_zdt(agent, &a.this, "withCalendar")?;
    agent.to_temporal_calendar(&a.arg(0))?;
    zdt_value(agent, ns, tz)
}

/// AddDurationToZonedDateTime.
fn add(agent: &mut Agent, a: &CallArgs<'_>, subtract: bool) -> JsResult<Value> {
    let (ns, tz) = this_zdt(agent, &a.this, if subtract { "subtract" } else { "add" })?;
    let mut d = agent.to_temporal_duration(&a.arg(0))?;
    if subtract {
        d = d.negated();
    }
    let reject = agent.temporal_overflow_of(&a.arg(1))?;
    let r = agent.temporal_add_zoned(ns, &tz, d.internal(), reject)?;
    zdt_value(agent, r, tz)
}

fn zdt_add(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    add(agent, a, false)
}

fn zdt_subtract(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    add(agent, a, true)
}

/// DifferenceTemporalZonedDateTime.
fn difference(agent: &mut Agent, a: &CallArgs<'_>, since: bool) -> JsResult<Value> {
    let (ns, tz) = this_zdt(agent, &a.this, if since { "since" } else { "until" })?;
    let (other, other_tz) = agent.to_temporal_zoned(&a.arg(0), &Value::Undefined)?;
    let o = agent.temporal_options_object(&a.arg(1))?;
    let (largest, smallest, increment, mode) = agent.temporal_difference_settings(
        since,
        o,
        UnitGroup::DateTime,
        &[],
        Unit::Nanosecond,
        Unit::Hour,
    )?;
    let finish = |agent: &mut Agent, d, lu| -> JsResult<Value> {
        let mut r = agent.temporal_from_internal(d, lu)?;
        if since {
            r = r.negated();
        }
        agent.temporal_create_duration(r.0, None)
    };
    if !largest.is_date() {
        let d = agent.temporal_diff_instant(ns, other, increment, smallest, mode)?;
        return finish(agent, d, largest);
    }
    if !super::time_zone_equals(&tz, &other_tz) {
        return agent.range_error("time zones must be equal to compute calendar differences");
    }
    if ns == other {
        return agent.temporal_create_duration([0.0; 10], None);
    }
    let d =
        agent.temporal_diff_zoned_rounding(ns, other, &tz, largest, increment, smallest, mode)?;
    finish(agent, d, Unit::Hour)
}

fn zdt_until(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    difference(agent, a, false)
}

fn zdt_since(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    difference(agent, a, true)
}

/// Temporal.ZonedDateTime.prototype.round ( roundTo ).
fn zdt_round(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (ns, tz) = this_zdt(agent, &a.this, "round")?;
    let round_to = super::duration::string_or_options(agent, &a.arg(0), "smallestUnit")?;
    let increment = agent.temporal_rounding_increment(round_to)?;
    let mode = agent.temporal_rounding_mode(round_to, RoundingMode::HalfExpand)?;
    let smallest = agent.temporal_unit_option(round_to, "smallestUnit", true)?;
    agent.temporal_validate_unit(smallest, UnitGroup::Time, false, true)?;
    let UnitOption::Unit(unit) = smallest else {
        return agent.range_error("smallestUnit is required");
    };
    let (max, inclusive) = match unit.max_increment() {
        None => (1, true),
        Some(m) => (m, false),
    };
    agent.temporal_validate_increment(increment, max, inclusive)?;
    if unit == Unit::Nanosecond && increment == 1 {
        return zdt_value(agent, ns, tz);
    }
    let dt = date_time_for(&tz, ns);
    let r = if unit == Unit::Day {
        let start = agent.temporal_start_of_day(&tz, dt.date)?;
        let end = agent.temporal_start_of_day(&tz, dt.date.add_days(1))?;
        let rounded = round_to_increment(ns - start, end - start, mode);
        start + rounded
    } else {
        let rd = super::round_date_time(dt, increment, unit, mode);
        let offset = super::offset_ns(&tz);
        agent.temporal_interpret_offset(
            rd.date,
            Some(rd.time),
            false,
            Some(offset),
            &tz,
            false,
            "prefer",
            false,
        )?
    };
    zdt_value(agent, r, tz)
}

fn zdt_equals(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (ns, tz) = this_zdt(agent, &a.this, "equals")?;
    let (other, other_tz) = agent.to_temporal_zoned(&a.arg(0), &Value::Undefined)?;
    Ok(Value::Boolean(
        ns == other && super::time_zone_equals(&tz, &other_tz),
    ))
}

/// TemporalZonedDateTimeToString.
#[allow(clippy::too_many_arguments)]
fn zdt_string(
    ns: i128,
    tz: &str,
    precision: Precision,
    show_calendar: &str,
    show_tz: &str,
    show_offset: &str,
    increment: i128,
    unit: Unit,
    mode: RoundingMode,
) -> String {
    let ns = super::options::round_as_if_positive(ns, increment * unit.nanos(), mode);
    let offset = super::offset_ns(tz);
    let dt = date_time_for(tz, ns);
    let mut s = iso::date_time_string(dt, precision);
    if show_offset != "never" {
        let off = round_to_increment(offset, 60_000_000_000, RoundingMode::HalfExpand);
        s.push_str(&iso::format_offset_minutes((off / 60_000_000_000) as i64));
    }
    if show_tz != "never" {
        let flag = if show_tz == "critical" { "!" } else { "" };
        s.push_str(&format!("[{flag}{tz}]"));
    }
    s.push_str(calendar_annotation(show_calendar));
    s
}

/// Temporal.ZonedDateTime.prototype.toString ( [ options ] ).
fn zdt_to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (ns, tz) = this_zdt(agent, &a.this, "toString")?;
    let o = agent.temporal_options_object(&a.arg(0))?;
    let show_calendar = agent.temporal_show_calendar(o)?;
    let digits = agent.temporal_fractional_digits(o)?;
    let show_offset = agent
        .temporal_string_option(o, "offset", &["auto", "never"])?
        .unwrap_or_else(|| "auto".to_owned());
    let mode = agent.temporal_rounding_mode(o, RoundingMode::Trunc)?;
    let smallest = agent.temporal_unit_option(o, "smallestUnit", false)?;
    let show_tz = agent
        .temporal_string_option(o, "timeZoneName", &["auto", "never", "critical"])?
        .unwrap_or_else(|| "auto".to_owned());
    agent.temporal_validate_unit(smallest, UnitGroup::Time, false, false)?;
    if smallest == UnitOption::Unit(Unit::Hour) {
        return agent.range_error("smallestUnit must be minutes or smaller");
    }
    let (precision, unit, increment) = super::seconds_precision(smallest, digits);
    let rounded = super::options::round_as_if_positive(ns, increment * unit.nanos(), mode);
    if !iso::is_valid_epoch_ns(rounded) {
        return agent.range_error("instant is outside the representable range");
    }
    Ok(js_str(&zdt_string(
        ns,
        &tz,
        precision,
        &show_calendar,
        &show_tz,
        &show_offset,
        increment,
        unit,
        mode,
    )))
}

fn default_string(ns: i128, tz: &str) -> String {
    zdt_string(
        ns,
        tz,
        Precision::Auto,
        "auto",
        "auto",
        "auto",
        1,
        Unit::Nanosecond,
        RoundingMode::Trunc,
    )
}

fn zdt_to_locale_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (ns, tz) = this_zdt(agent, &a.this, "toLocaleString")?;
    Ok(js_str(&default_string(ns, &tz)))
}

fn zdt_to_json(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (ns, tz) = this_zdt(agent, &a.this, "toJSON")?;
    Ok(js_str(&default_string(ns, &tz)))
}

fn zdt_start_of_day(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (ns, tz) = this_zdt(agent, &a.this, "startOfDay")?;
    let d = date_time_for(&tz, ns).date;
    let r = agent.temporal_start_of_day(&tz, d)?;
    zdt_value(agent, r, tz)
}

/// Temporal.ZonedDateTime.prototype.getTimeZoneTransition ( directionParam ).
fn zdt_get_time_zone_transition(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let _ = this_zdt(agent, &a.this, "getTimeZoneTransition")?;
    let o = super::duration::string_or_options(agent, &a.arg(0), "direction")?;
    let dir = agent.temporal_string_option(o, "direction", &["next", "previous"])?;
    if dir.is_none() {
        return agent.range_error("direction is required");
    }
    // Las zonas disponibles (UTC y offsets) no tienen transiciones.
    Ok(Value::Null)
}

fn zdt_to_instant(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (ns, _) = this_zdt(agent, &a.this, "toInstant")?;
    agent.temporal_create_instant(ns, None)
}

fn zdt_to_plain_date(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (ns, tz) = this_zdt(agent, &a.this, "toPlainDate")?;
    agent.temporal_create_date(date_time_for(&tz, ns).date, None)
}

fn zdt_to_plain_time(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (ns, tz) = this_zdt(agent, &a.this, "toPlainTime")?;
    create(
        agent,
        TemporalData::PlainTime(date_time_for(&tz, ns).time),
        None,
    )
}

fn zdt_to_plain_date_time(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (ns, tz) = this_zdt(agent, &a.this, "toPlainDateTime")?;
    agent.temporal_create_date_time(date_time_for(&tz, ns), None)
}
