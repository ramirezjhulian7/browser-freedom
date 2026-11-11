//! Temporal.PlainDate y los getters de calendario comunes a los tipos con fecha.

use super::duration::{DateDuration, Internal};
use super::iso::{self, IsoDate, IsoDateTime, NS_PER_DAY, Time};
use super::options::{Unit, UnitGroup};
use super::parse::{self, Goal};
use super::{
    DATE_FIELDS, Fields, ISO8601, Required, TemporalData, calendar_annotation, class, create,
    data_of, getters, js_str,
};
use crate::builtins::FnEntry;
use crate::heap::Gc;
use crate::object::Object;
use crate::value::{PropertyKey, Value};
use crate::vm::function::{CallArgs, NativeFn};
use crate::vm::{Agent, JsResult};

/// Un campo de calendario de una fecha ISO (los getters de Table 1).
pub fn calendar_field(d: IsoDate, name: &str) -> Value {
    let y = i64::from(d.year);
    let n = |x: i64| Value::Number(x as f64);
    match name {
        "calendarId" => js_str(ISO8601),
        "era" | "eraYear" => Value::Undefined,
        "year" => n(y),
        "month" => n(d.month.into()),
        "monthCode" => js_str(&format!("M{:02}", d.month)),
        "day" => n(d.day.into()),
        "dayOfWeek" => n(d.day_of_week()),
        "dayOfYear" => n(d.day_of_year()),
        "weekOfYear" => n(d.week_of_year().0),
        "yearOfWeek" => n(d.week_of_year().1),
        "daysInWeek" => n(7),
        "daysInMonth" => n(iso::days_in_month(y, d.month.into())),
        "daysInYear" => n(iso::days_in_year(y)),
        "monthsInYear" => n(12),
        "inLeapYear" => Value::Boolean(iso::is_leap_year(y)),
        _ => Value::Undefined,
    }
}

/// Los nombres de los getters de calendario de PlainDate, PlainDateTime y ZonedDateTime.
pub const DATE_GETTERS: &[&str] = &[
    "calendarId",
    "era",
    "eraYear",
    "year",
    "month",
    "monthCode",
    "day",
    "dayOfWeek",
    "dayOfYear",
    "weekOfYear",
    "yearOfWeek",
    "daysInWeek",
    "daysInMonth",
    "daysInYear",
    "monthsInYear",
    "inLeapYear",
];

macro_rules! temporal_date_getters {
    ($table:ident, $date:path, $($name:literal => $f:ident),* $(,)?) => {
        $(
            fn $f(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
                let d = $date(agent, &a.this, $name)?;
                Ok(calendar_field(d, $name))
            }
        )*
        const $table: &[(&str, NativeFn)] = &[$(($name, $f)),*];
    };
}

temporal_date_getters!(
    GETTERS, this_date,
    "calendarId" => g_calendar_id, "era" => g_era, "eraYear" => g_era_year, "year" => g_year,
    "month" => g_month, "monthCode" => g_month_code, "day" => g_day,
    "dayOfWeek" => g_day_of_week, "dayOfYear" => g_day_of_year, "weekOfYear" => g_week_of_year,
    "yearOfWeek" => g_year_of_week, "daysInWeek" => g_days_in_week,
    "daysInMonth" => g_days_in_month, "daysInYear" => g_days_in_year,
    "monthsInYear" => g_months_in_year, "inLeapYear" => g_in_leap_year,
);

pub fn init(agent: &mut Agent, temporal: Gc<Object>) -> Gc<Object> {
    const STATICS: &[FnEntry] = &[("from", 1, date_from), ("compare", 2, date_compare)];
    let (_, proto) = class(agent, temporal, "PlainDate", 3, date_ctor, STATICS);
    getters(agent, proto, GETTERS);
    const METHODS: &[FnEntry] = &[
        ("toPlainYearMonth", 0, to_plain_year_month),
        ("toPlainMonthDay", 0, to_plain_month_day),
        ("add", 1, date_add),
        ("subtract", 1, date_subtract),
        ("with", 1, date_with),
        ("withCalendar", 1, date_with_calendar),
        ("until", 1, date_until),
        ("since", 1, date_since),
        ("equals", 1, date_equals),
        ("toPlainDateTime", 0, to_plain_date_time),
        ("toZonedDateTime", 1, to_zoned_date_time),
        ("toString", 0, date_to_string),
        ("toLocaleString", 0, date_to_locale_string),
        ("toJSON", 0, date_to_json),
        ("valueOf", 0, super::value_of),
    ];
    crate::builtins::methods(agent, proto, METHODS);
    proto
}

fn this_date(agent: &mut Agent, this: &Value, method: &str) -> JsResult<IsoDate> {
    match super::this_data(agent, this, "PlainDate", method, |d| {
        matches!(d, TemporalData::PlainDate(_))
    })? {
        TemporalData::PlainDate(d) => Ok(d),
        _ => unreachable!(),
    }
}

impl Agent {
    /// CreateTemporalDate.
    pub fn temporal_create_date(
        &mut self,
        d: IsoDate,
        new_target: Option<Gc<Object>>,
    ) -> JsResult<Value> {
        self.temporal_check_date(d)?;
        create(self, TemporalData::PlainDate(d), new_target)
    }

    /// ToTemporalDate.
    pub fn to_temporal_date(&mut self, item: &Value, options: &Value) -> JsResult<IsoDate> {
        if let Value::Object(o) = item {
            match data_of(self, item) {
                Some(TemporalData::PlainDate(d)) => {
                    self.temporal_overflow_of(options)?;
                    return Ok(d);
                }
                Some(TemporalData::ZonedDateTime { ns, tz }) => {
                    let dt = super::date_time_for(&tz, ns);
                    self.temporal_overflow_of(options)?;
                    return Ok(dt.date);
                }
                Some(TemporalData::PlainDateTime(dt)) => {
                    self.temporal_overflow_of(options)?;
                    return Ok(dt.date);
                }
                _ => {}
            }
            let o = *o;
            self.temporal_calendar_with_default(o)?;
            let f = self.temporal_prepare_fields(o, DATE_FIELDS, Required::List(&[]))?;
            let reject = self.temporal_overflow_of(options)?;
            return self.temporal_date_from_fields(f, reject);
        }
        let Value::String(s) = item else {
            return self.type_error("date must be an object or a string");
        };
        let s = s.to_string_lossy();
        let Some(r) = parse::parse_iso_date_time(&s, &[Goal::DateTime { zoned: false }]) else {
            return self.range_error(&format!("Invalid date: {s}"));
        };
        self.temporal_canonicalize_calendar(r.calendar.as_deref().unwrap_or(ISO8601))?;
        self.temporal_overflow_of(options)?;
        let d = IsoDate::new(r.year.unwrap_or(0), r.month, r.day.unwrap_or(1));
        self.temporal_check_date(d)?;
        Ok(d)
    }

    /// ToTimeRecordOrMidnight.
    pub fn temporal_time_or_midnight(&mut self, v: &Value) -> JsResult<Time> {
        if v.is_undefined() {
            return Ok(Time::MIDNIGHT);
        }
        self.to_temporal_time(v, &Value::Undefined)
    }
}

/// Temporal.PlainDate ( isoYear, isoMonth, isoDay [ , calendar ] ).
fn date_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor Temporal.PlainDate requires 'new'");
    };
    let y = agent.to_integer_with_truncation(&a.arg(0))?;
    let m = agent.to_integer_with_truncation(&a.arg(1))?;
    let d = agent.to_integer_with_truncation(&a.arg(2))?;
    agent.temporal_calendar_arg(&a.arg(3))?;
    if !iso::is_valid_date(y as i64, m as i64, d as i64) || !iso::within_i32(y as i64) {
        return agent.range_error("invalid date");
    }
    agent.temporal_create_date(IsoDate::new(y as i64, m as i64, d as i64), Some(nt))
}

fn date_from(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = agent.to_temporal_date(&a.arg(0), &a.arg(1))?;
    agent.temporal_create_date(d, None)
}

fn date_compare(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let one = agent.to_temporal_date(&a.arg(0), &Value::Undefined)?;
    let two = agent.to_temporal_date(&a.arg(1), &Value::Undefined)?;
    Ok(Value::Number(f64::from(iso::compare_date(one, two))))
}

fn to_plain_year_month(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_date(agent, &a.this, "toPlainYearMonth")?;
    let r = agent.temporal_year_month_from_fields(Fields::from_date(d, true, true), false)?;
    create(agent, TemporalData::PlainYearMonth(r), None)
}

fn to_plain_month_day(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_date(agent, &a.this, "toPlainMonthDay")?;
    let r = agent.temporal_month_day_from_fields(Fields::from_date(d, true, true), false)?;
    create(agent, TemporalData::PlainMonthDay(r), None)
}

impl Agent {
    /// ToDateDurationRecordWithoutTime.
    pub fn temporal_date_duration_without_time(
        &mut self,
        d: &super::duration::Duration,
    ) -> JsResult<DateDuration> {
        let i = d.internal_24h();
        let days = i.time / NS_PER_DAY;
        self.temporal_date_duration(i.date.years, i.date.months, i.date.weeks, days as i64)
    }
}

/// AddDurationToDate.
fn add(agent: &mut Agent, a: &CallArgs<'_>, subtract: bool) -> JsResult<Value> {
    let date = this_date(agent, &a.this, if subtract { "subtract" } else { "add" })?;
    let mut d = agent.to_temporal_duration(&a.arg(0))?;
    if subtract {
        d = d.negated();
    }
    let dd = agent.temporal_date_duration_without_time(&d)?;
    let reject = agent.temporal_overflow_of(&a.arg(1))?;
    let r = agent.temporal_date_add(date, dd, reject)?;
    agent.temporal_create_date(r, None)
}

fn date_add(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    add(agent, a, false)
}

fn date_subtract(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    add(agent, a, true)
}

/// Temporal.PlainDate.prototype.with ( temporalDateLike [ , options ] ).
fn date_with(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let date = this_date(agent, &a.this, "with")?;
    let like = a.arg(0);
    if !agent.temporal_is_partial(&like)? {
        return agent.type_error("with() requires a partial date object");
    }
    let Value::Object(o) = like else {
        unreachable!()
    };
    let fields = Fields::from_date(date, true, true);
    let partial = agent.temporal_prepare_fields(o, DATE_FIELDS, Required::Partial)?;
    let merged = fields.merge(partial);
    let reject = agent.temporal_overflow_of(&a.arg(1))?;
    let r = agent.temporal_date_from_fields(merged, reject)?;
    agent.temporal_create_date(r, None)
}

fn date_with_calendar(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let date = this_date(agent, &a.this, "withCalendar")?;
    agent.to_temporal_calendar(&a.arg(0))?;
    agent.temporal_create_date(date, None)
}

/// DifferenceTemporalPlainDate.
fn difference(agent: &mut Agent, a: &CallArgs<'_>, since: bool) -> JsResult<Value> {
    let date = this_date(agent, &a.this, if since { "since" } else { "until" })?;
    let other = agent.to_temporal_date(&a.arg(0), &Value::Undefined)?;
    let o = agent.temporal_options_object(&a.arg(1))?;
    let (largest, smallest, increment, mode) =
        agent.temporal_difference_settings(since, o, UnitGroup::Date, &[], Unit::Day, Unit::Day)?;
    if date == other {
        return agent.temporal_create_duration([0.0; 10], None);
    }
    let dd = super::duration::date_until(date, other, largest);
    let mut d = Internal { date: dd, time: 0 };
    if smallest != Unit::Day || increment != 1 {
        let start = IsoDateTime {
            date,
            time: Time::MIDNIGHT,
        };
        let end = IsoDateTime {
            date: other,
            time: Time::MIDNIGHT,
        };
        d = agent.temporal_round_relative(
            d,
            start.epoch_ns(),
            end.epoch_ns(),
            start,
            None,
            largest,
            increment,
            smallest,
            mode,
        )?;
    }
    let mut r = agent.temporal_from_internal(d, Unit::Day)?;
    if since {
        r = r.negated();
    }
    agent.temporal_create_duration(r.0, None)
}

fn date_until(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    difference(agent, a, false)
}

fn date_since(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    difference(agent, a, true)
}

fn date_equals(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let date = this_date(agent, &a.this, "equals")?;
    let other = agent.to_temporal_date(&a.arg(0), &Value::Undefined)?;
    Ok(Value::Boolean(date == other))
}

fn to_plain_date_time(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let date = this_date(agent, &a.this, "toPlainDateTime")?;
    let time = agent.temporal_time_or_midnight(&a.arg(0))?;
    agent.temporal_create_date_time(IsoDateTime { date, time }, None)
}

/// Temporal.PlainDate.prototype.toZonedDateTime ( item ).
fn to_zoned_date_time(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let date = this_date(agent, &a.this, "toZonedDateTime")?;
    let item = a.arg(0);
    let (tz, time) = if let Value::Object(o) = item {
        let tzl = agent.get(o, &PropertyKey::from("timeZone"), item.clone())?;
        if tzl.is_undefined() {
            (agent.to_temporal_time_zone(&item)?, Value::Undefined)
        } else {
            let tz = agent.to_temporal_time_zone(&tzl)?;
            let t = agent.get(o, &PropertyKey::from("plainTime"), item.clone())?;
            (tz, t)
        }
    } else {
        (agent.to_temporal_time_zone(&item)?, Value::Undefined)
    };
    let ns = if time.is_undefined() {
        agent.temporal_start_of_day(&tz, date)?
    } else {
        let t = agent.to_temporal_time(&time, &Value::Undefined)?;
        let dt = IsoDateTime { date, time: t };
        agent.temporal_check_date_time(dt)?;
        agent.temporal_epoch_ns_for(&tz, dt)?
    };
    create(agent, TemporalData::ZonedDateTime { ns, tz }, None)
}

fn date_to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let date = this_date(agent, &a.this, "toString")?;
    let o = agent.temporal_options_object(&a.arg(0))?;
    let show = agent.temporal_show_calendar(o)?;
    Ok(js_str(&format!(
        "{}{}",
        iso::date_string(date),
        calendar_annotation(&show)
    )))
}

fn date_to_locale_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let date = this_date(agent, &a.this, "toLocaleString")?;
    Ok(js_str(&iso::date_string(date)))
}

fn date_to_json(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let date = this_date(agent, &a.this, "toJSON")?;
    Ok(js_str(&iso::date_string(date)))
}
