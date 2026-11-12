//! Temporal.PlainDateTime: una fecha y hora sin zona.

use super::iso::{self, IsoDate, IsoDateTime, Time};
use super::options::{RoundingMode, Unit, UnitGroup, UnitOption};
use super::parse::{self, Goal};
use super::plain_date::calendar_field;
use super::plain_time::time_field;
use super::{
    Cal, DATE_FIELDS, Fields, ISO8601, Required, TIME_FIELDS, TemporalData, calendar_annotation,
    class, create, data_of, getters, js_str,
};
use crate::builtins::FnEntry;
use crate::heap::Gc;
use crate::object::Object;
use crate::value::Value;
use crate::vm::function::{CallArgs, NativeFn};
use crate::vm::{Agent, JsResult};

fn this_dt(agent: &mut Agent, this: &Value, method: &str) -> JsResult<(IsoDateTime, Cal)> {
    match super::this_data(agent, this, "PlainDateTime", method, |d| {
        matches!(d, TemporalData::PlainDateTime(..))
    })? {
        TemporalData::PlainDateTime(dt, c) => Ok((dt, c)),
        _ => unreachable!(),
    }
}

fn this_date_part(agent: &mut Agent, this: &Value, method: &str) -> JsResult<(IsoDate, Cal)> {
    let (dt, cal) = this_dt(agent, this, method)?;
    Ok((dt.date, cal))
}

macro_rules! date_getters {
    ($($name:literal => $f:ident),* $(,)?) => {
        $(
            fn $f(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
                let (d, cal) = this_date_part(agent, &a.this, $name)?;
                Ok(calendar_field(d, cal, $name))
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
                let (dt, _) = this_dt(agent, &a.this, $name)?;
                Ok(time_field(dt.time, $name))
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
    const STATICS: &[FnEntry] = &[("from", 1, dt_from), ("compare", 2, dt_compare)];
    let (_, proto) = class(agent, temporal, "PlainDateTime", 3, dt_ctor, STATICS);
    getters(agent, proto, DATE_G);
    getters(agent, proto, TIME_G);
    const METHODS: &[FnEntry] = &[
        ("with", 1, dt_with),
        ("withPlainTime", 0, dt_with_plain_time),
        ("withCalendar", 1, dt_with_calendar),
        ("add", 1, dt_add),
        ("subtract", 1, dt_subtract),
        ("until", 1, dt_until),
        ("since", 1, dt_since),
        ("round", 1, dt_round),
        ("equals", 1, dt_equals),
        ("toString", 0, dt_to_string),
        ("toLocaleString", 0, dt_to_locale_string),
        ("toJSON", 0, dt_to_json),
        ("valueOf", 0, super::value_of),
        ("toZonedDateTime", 1, dt_to_zoned_date_time),
        ("toPlainDate", 0, dt_to_plain_date),
        ("toPlainTime", 0, dt_to_plain_time),
    ];
    crate::builtins::methods(agent, proto, METHODS);
    proto
}

impl Agent {
    /// CreateTemporalDateTime.
    pub fn temporal_create_date_time(
        &mut self,
        dt: IsoDateTime,
        cal: Cal,
        new_target: Option<Gc<Object>>,
    ) -> JsResult<Value> {
        self.temporal_check_date_time(dt)?;
        create(self, TemporalData::PlainDateTime(dt, cal), new_target)
    }

    /// ToTemporalDateTime.
    pub fn to_temporal_date_time(
        &mut self,
        item: &Value,
        options: &Value,
    ) -> JsResult<(IsoDateTime, Cal)> {
        if let Value::Object(o) = item {
            match data_of(self, item) {
                Some(TemporalData::PlainDateTime(dt, c)) => {
                    self.temporal_overflow_of(options)?;
                    return Ok((dt, c));
                }
                Some(TemporalData::ZonedDateTime { ns, tz, cal }) => {
                    let dt = super::date_time_for(&tz, ns);
                    self.temporal_overflow_of(options)?;
                    return Ok((dt, cal));
                }
                Some(TemporalData::PlainDate(d, c)) => {
                    self.temporal_overflow_of(options)?;
                    let dt = IsoDateTime {
                        date: d,
                        time: Time::MIDNIGHT,
                    };
                    self.temporal_check_date_time(dt)?;
                    return Ok((dt, c));
                }
                _ => {}
            }
            let o = *o;
            let cal = self.temporal_calendar_with_default(o)?;
            let mut names = DATE_FIELDS.to_vec();
            names.extend_from_slice(TIME_FIELDS);
            let f = self.temporal_prepare_fields(cal, o, &names, Required::List(&[]))?;
            let reject = self.temporal_overflow_of(options)?;
            let dt = self.temporal_date_time_from_fields(cal, f, reject)?;
            self.temporal_check_date_time(dt)?;
            return Ok((dt, cal));
        }
        let Value::String(s) = item else {
            return self.type_error("date-time must be an object or a string");
        };
        let s = s.to_string_lossy();
        let Some(r) = parse::parse_iso_date_time(&s, &[Goal::DateTime { zoned: false }]) else {
            return self.range_error(&format!("Invalid date-time: {s}"));
        };
        let cal = self.temporal_canonicalize_calendar(r.calendar.as_deref().unwrap_or(ISO8601))?;
        self.temporal_overflow_of(options)?;
        let dt = IsoDateTime {
            date: IsoDate::new(r.year.unwrap_or(0), r.month, r.day.unwrap_or(1)),
            time: r.time.unwrap_or_default(),
        };
        self.temporal_check_date_time(dt)?;
        Ok((dt, cal))
    }
}

/// Temporal.PlainDateTime ( isoYear, isoMonth, isoDay [ , hour … , calendar ] ).
fn dt_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor Temporal.PlainDateTime requires 'new'");
    };
    let y = agent.to_integer_with_truncation(&a.arg(0))?;
    let m = agent.to_integer_with_truncation(&a.arg(1))?;
    let d = agent.to_integer_with_truncation(&a.arg(2))?;
    let mut t = [0f64; 6];
    for (k, v) in t.iter_mut().enumerate() {
        let x = a.arg(3 + k);
        if !x.is_undefined() {
            *v = agent.to_integer_with_truncation(&x)?;
        }
    }
    let cal = agent.temporal_calendar_arg(&a.arg(9))?;
    if !iso::within_i32(y as i64) || !iso::is_valid_date(y as i64, m as i64, d as i64) {
        return agent.range_error("invalid date");
    }
    if !iso::is_valid_time(t[0], t[1], t[2], t[3], t[4], t[5]) {
        return agent.range_error("invalid time");
    }
    let time = agent.temporal_regulate_time(t, true)?;
    let dt = IsoDateTime {
        date: IsoDate::new(y as i64, m as i64, d as i64),
        time,
    };
    agent.temporal_create_date_time(dt, cal, Some(nt))
}

fn dt_from(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (dt, cal) = agent.to_temporal_date_time(&a.arg(0), &a.arg(1))?;
    agent.temporal_create_date_time(dt, cal, None)
}

fn dt_compare(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (one, _) = agent.to_temporal_date_time(&a.arg(0), &Value::Undefined)?;
    let (two, _) = agent.to_temporal_date_time(&a.arg(1), &Value::Undefined)?;
    Ok(Value::Number(f64::from(iso::compare_date_time(one, two))))
}

/// Temporal.PlainDateTime.prototype.with ( temporalDateTimeLike [ , options ] ).
fn dt_with(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (dt, cal) = this_dt(agent, &a.this, "with")?;
    let like = a.arg(0);
    if !agent.temporal_is_partial(&like)? {
        return agent.type_error("with() requires a partial date-time object");
    }
    let Value::Object(o) = like else {
        unreachable!()
    };
    let mut fields = Fields::from_date(cal, dt.date, true, true);
    fields.set_time(dt.time);
    let mut names = DATE_FIELDS.to_vec();
    names.extend_from_slice(TIME_FIELDS);
    let partial = agent.temporal_prepare_fields(cal, o, &names, Required::Partial)?;
    let merged = fields.merge(cal, partial);
    let reject = agent.temporal_overflow_of(&a.arg(1))?;
    let r = agent.temporal_date_time_from_fields(cal, merged, reject)?;
    agent.temporal_create_date_time(r, cal, None)
}

fn dt_with_plain_time(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (dt, cal) = this_dt(agent, &a.this, "withPlainTime")?;
    let time = agent.temporal_time_or_midnight(&a.arg(0))?;
    agent.temporal_create_date_time(
        IsoDateTime {
            date: dt.date,
            time,
        },
        cal,
        None,
    )
}

fn dt_with_calendar(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (dt, _) = this_dt(agent, &a.this, "withCalendar")?;
    let cal = agent.to_temporal_calendar(&a.arg(0))?;
    agent.temporal_create_date_time(dt, cal, None)
}

/// AddDurationToDateTime.
fn add(agent: &mut Agent, a: &CallArgs<'_>, subtract: bool) -> JsResult<Value> {
    let (dt, cal) = this_dt(agent, &a.this, if subtract { "subtract" } else { "add" })?;
    let mut d = agent.to_temporal_duration(&a.arg(0))?;
    if subtract {
        d = d.negated();
    }
    let reject = agent.temporal_overflow_of(&a.arg(1))?;
    let internal = d.internal_24h();
    let (days, time) = iso::balance_time(dt.time.nanos() + internal.time);
    let dd = agent.temporal_adjust_date_duration(internal.date, days, None, None)?;
    let date = agent.temporal_date_add(cal, dt.date, dd, reject)?;
    agent.temporal_create_date_time(IsoDateTime { date, time }, cal, None)
}

fn dt_add(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    add(agent, a, false)
}

fn dt_subtract(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    add(agent, a, true)
}

/// DifferenceTemporalPlainDateTime.
fn difference(agent: &mut Agent, a: &CallArgs<'_>, since: bool) -> JsResult<Value> {
    let (dt, cal) = this_dt(agent, &a.this, if since { "since" } else { "until" })?;
    let (other, other_cal) = agent.to_temporal_date_time(&a.arg(0), &Value::Undefined)?;
    if cal != other_cal {
        return agent
            .range_error("cannot compute the difference between dates of different calendars");
    }
    let o = agent.temporal_options_object(&a.arg(1))?;
    let (largest, smallest, increment, mode) = agent.temporal_difference_settings(
        since,
        o,
        UnitGroup::DateTime,
        &[],
        Unit::Nanosecond,
        Unit::Day,
    )?;
    if dt == other {
        return agent.temporal_create_duration([0.0; 10], None);
    }
    let d = agent
        .temporal_diff_date_time_rounding(cal, dt, other, largest, increment, smallest, mode)?;
    let mut r = agent.temporal_from_internal(d, largest)?;
    if since {
        r = r.negated();
    }
    agent.temporal_create_duration(r.0, None)
}

fn dt_until(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    difference(agent, a, false)
}

fn dt_since(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    difference(agent, a, true)
}

/// Temporal.PlainDateTime.prototype.round ( roundTo ).
fn dt_round(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (dt, cal) = this_dt(agent, &a.this, "round")?;
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
        return agent.temporal_create_date_time(dt, cal, None);
    }
    let r = super::round_date_time(dt, increment, unit, mode);
    agent.temporal_create_date_time(r, cal, None)
}

fn dt_equals(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let this = this_dt(agent, &a.this, "equals")?;
    let other = agent.to_temporal_date_time(&a.arg(0), &Value::Undefined)?;
    Ok(Value::Boolean(this == other))
}

/// Temporal.PlainDateTime.prototype.toString ( [ options ] ).
fn dt_to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (dt, cal) = this_dt(agent, &a.this, "toString")?;
    let o = agent.temporal_options_object(&a.arg(0))?;
    let show = agent.temporal_show_calendar(o)?;
    let digits = agent.temporal_fractional_digits(o)?;
    let mode = agent.temporal_rounding_mode(o, RoundingMode::Trunc)?;
    let smallest = agent.temporal_unit_option(o, "smallestUnit", false)?;
    agent.temporal_validate_unit(smallest, UnitGroup::Time, false, false)?;
    if smallest == UnitOption::Unit(Unit::Hour) {
        return agent.range_error("smallestUnit must be minutes or smaller");
    }
    let (precision, unit, increment) = super::seconds_precision(smallest, digits);
    let r = super::round_date_time(dt, increment, unit, mode);
    agent.temporal_check_date_time(r)?;
    Ok(js_str(&format!(
        "{}{}",
        iso::date_time_string(r, precision),
        calendar_annotation(cal, &show)
    )))
}

fn dt_to_locale_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_dt(agent, &a.this, "toLocaleString")?;
    crate::builtins::intl::date_time_format::temporal_to_locale_string(
        agent,
        &a.this,
        &a.arg(0),
        &a.arg(1),
        crate::builtins::intl::date_time_format::Required::Any,
        crate::builtins::intl::date_time_format::Defaults::All,
        None,
    )
}

fn dt_to_json(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (dt, cal) = this_dt(agent, &a.this, "toJSON")?;
    Ok(js_str(&format!(
        "{}{}",
        iso::date_time_string(dt, iso::Precision::Auto),
        calendar_annotation(cal, "auto")
    )))
}

/// Temporal.PlainDateTime.prototype.toZonedDateTime ( temporalTimeZoneLike [ , options ] ).
fn dt_to_zoned_date_time(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (dt, cal) = this_dt(agent, &a.this, "toZonedDateTime")?;
    let tz = agent.to_temporal_time_zone(&a.arg(0))?;
    let o = agent.temporal_options_object(&a.arg(1))?;
    agent.temporal_disambiguation(o)?;
    let ns = agent.temporal_epoch_ns_for(&tz, dt)?;
    create(agent, TemporalData::ZonedDateTime { ns, tz, cal }, None)
}

fn dt_to_plain_date(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (dt, cal) = this_dt(agent, &a.this, "toPlainDate")?;
    agent.temporal_create_date(dt.date, cal, None)
}

fn dt_to_plain_time(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (dt, _) = this_dt(agent, &a.this, "toPlainTime")?;
    create(agent, TemporalData::PlainTime(dt.time), None)
}

impl Agent {
    /// GetTemporalDisambiguationOption.
    pub fn temporal_disambiguation(&mut self, o: Gc<Object>) -> JsResult<String> {
        Ok(self
            .temporal_string_option(
                o,
                "disambiguation",
                &["compatible", "earlier", "later", "reject"],
            )?
            .unwrap_or_else(|| "compatible".to_owned()))
    }

    /// GetTemporalOffsetOption.
    pub fn temporal_offset_option(&mut self, o: Gc<Object>, fallback: &str) -> JsResult<String> {
        Ok(self
            .temporal_string_option(o, "offset", &["prefer", "use", "ignore", "reject"])?
            .unwrap_or_else(|| fallback.to_owned()))
    }
}
