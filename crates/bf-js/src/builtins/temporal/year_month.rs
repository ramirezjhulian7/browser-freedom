//! Temporal.PlainYearMonth: un mes de un año (con un día de referencia ISO).

use super::duration::Internal;
use super::iso::{self, IsoDate, IsoDateTime, Time};
use super::options::{Unit, UnitGroup};
use super::parse::{self, Goal};
use super::{
    Cal, Field, Fields, ISO8601, Required, TemporalData, calendar_annotation, class, create,
    data_of, getters, js_str,
};
use crate::builtins::FnEntry;
use crate::heap::Gc;
use crate::object::Object;
use crate::value::Value;
use crate::vm::function::{CallArgs, NativeFn};
use crate::vm::{Agent, JsResult};

const YM_FIELDS: &[Field] = &[Field::Year, Field::Month, Field::MonthCode];

fn this_ym(agent: &mut Agent, this: &Value, method: &str) -> JsResult<(IsoDate, Cal)> {
    match super::this_data(agent, this, "PlainYearMonth", method, |d| {
        matches!(d, TemporalData::PlainYearMonth(..))
    })? {
        TemporalData::PlainYearMonth(d, c) => Ok((d, c)),
        _ => unreachable!(),
    }
}

macro_rules! ym_getters {
    ($($name:literal => $f:ident),* $(,)?) => {
        $(
            fn $f(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
                let (d, cal) = this_ym(agent, &a.this, $name)?;
                Ok(super::plain_date::calendar_field(d, cal, $name))
            }
        )*
        const G: &[(&str, NativeFn)] = &[$(($name, $f)),*];
    };
}

ym_getters!(
    "calendarId" => g_calendar_id, "era" => g_era, "eraYear" => g_era_year, "year" => g_year,
    "month" => g_month, "monthCode" => g_month_code, "daysInYear" => g_days_in_year,
    "daysInMonth" => g_days_in_month, "monthsInYear" => g_months_in_year,
    "inLeapYear" => g_in_leap_year,
);

pub fn init(agent: &mut Agent, temporal: Gc<Object>) -> Gc<Object> {
    const STATICS: &[FnEntry] = &[("from", 1, ym_from), ("compare", 2, ym_compare)];
    let (_, proto) = class(agent, temporal, "PlainYearMonth", 2, ym_ctor, STATICS);
    getters(agent, proto, G);
    const METHODS: &[FnEntry] = &[
        ("with", 1, ym_with),
        ("add", 1, ym_add),
        ("subtract", 1, ym_subtract),
        ("until", 1, ym_until),
        ("since", 1, ym_since),
        ("equals", 1, ym_equals),
        ("toString", 0, ym_to_string),
        ("toLocaleString", 0, ym_to_locale_string),
        ("toJSON", 0, ym_to_json),
        ("valueOf", 0, super::value_of),
        ("toPlainDate", 1, ym_to_plain_date),
    ];
    crate::builtins::methods(agent, proto, METHODS);
    proto
}

impl Agent {
    /// CreateTemporalYearMonth.
    fn temporal_create_year_month(
        &mut self,
        d: IsoDate,
        cal: Cal,
        new_target: Option<Gc<Object>>,
    ) -> JsResult<Value> {
        if !iso::year_month_within_limits(d.year.into(), d.month.into()) {
            return self.range_error("year-month is outside the representable range");
        }
        create(self, TemporalData::PlainYearMonth(d, cal), new_target)
    }

    /// ToTemporalYearMonth.
    pub fn to_temporal_year_month(
        &mut self,
        item: &Value,
        options: &Value,
    ) -> JsResult<(IsoDate, Cal)> {
        if let Value::Object(o) = item {
            if let Some(TemporalData::PlainYearMonth(d, c)) = data_of(self, item) {
                self.temporal_overflow_of(options)?;
                return Ok((d, c));
            }
            let o = *o;
            let cal = self.temporal_calendar_with_default(o)?;
            let f = self.temporal_prepare_fields(cal, o, YM_FIELDS, Required::List(&[]))?;
            let reject = self.temporal_overflow_of(options)?;
            return Ok((self.temporal_year_month_from_fields(cal, f, reject)?, cal));
        }
        let Value::String(s) = item else {
            return self.type_error("year-month must be an object or a string");
        };
        let s = s.to_string_lossy();
        let Some(r) = parse::parse_iso_date_time(&s, &[Goal::YearMonth]) else {
            return self.range_error(&format!("Invalid year-month: {s}"));
        };
        let cal = self.temporal_canonicalize_calendar(r.calendar.as_deref().unwrap_or(ISO8601))?;
        self.temporal_overflow_of(options)?;
        let y = r.year.unwrap_or(0);
        if !iso::year_month_within_limits(y, r.month) {
            return self.range_error("year-month is outside the representable range");
        }
        if cal.is_iso() {
            return Ok((IsoDate::new(y, r.month, 1), cal));
        }
        // Fuera de ISO, el string tiene la fecha completa (YYYY-MM-DD[u-ca=…]).
        let Some(day) = r.day else {
            return self.range_error(&format!(
                "A year-month string needs a day with the {} calendar",
                cal.id()
            ));
        };
        let d = IsoDate::new(y, r.month, day);
        let f = Fields::from_date(cal, d, true, false);
        Ok((self.temporal_year_month_from_fields(cal, f, false)?, cal))
    }
}

/// Temporal.PlainYearMonth ( isoYear, isoMonth [ , calendar [ , referenceISODay ] ] ).
fn ym_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor Temporal.PlainYearMonth requires 'new'");
    };
    let y = agent.to_integer_with_truncation(&a.arg(0))?;
    let m = agent.to_integer_with_truncation(&a.arg(1))?;
    let cal = agent.temporal_calendar_arg(&a.arg(2))?;
    let ref_day = if a.arg(3).is_undefined() {
        1.0
    } else {
        agent.to_integer_with_truncation(&a.arg(3))?
    };
    if !iso::within_i32(y as i64) || !iso::is_valid_date(y as i64, m as i64, ref_day as i64) {
        return agent.range_error("invalid year-month");
    }
    let d = IsoDate::new(y as i64, m as i64, ref_day as i64);
    agent.temporal_create_year_month(d, cal, Some(nt))
}

fn ym_from(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (d, cal) = agent.to_temporal_year_month(&a.arg(0), &a.arg(1))?;
    agent.temporal_create_year_month(d, cal, None)
}

fn ym_compare(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (one, _) = agent.to_temporal_year_month(&a.arg(0), &Value::Undefined)?;
    let (two, _) = agent.to_temporal_year_month(&a.arg(1), &Value::Undefined)?;
    Ok(Value::Number(f64::from(iso::compare_date(one, two))))
}

/// Temporal.PlainYearMonth.prototype.with ( temporalYearMonthLike [ , options ] ).
fn ym_with(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (d, cal) = this_ym(agent, &a.this, "with")?;
    let like = a.arg(0);
    if !agent.temporal_is_partial(&like)? {
        return agent.type_error("with() requires a partial year-month object");
    }
    let Value::Object(o) = like else {
        unreachable!()
    };
    let fields = Fields::from_date(cal, d, true, false);
    let partial = agent.temporal_prepare_fields(cal, o, YM_FIELDS, Required::Partial)?;
    let merged = fields.merge(cal, partial);
    let reject = agent.temporal_overflow_of(&a.arg(1))?;
    let r = agent.temporal_year_month_from_fields(cal, merged, reject)?;
    agent.temporal_create_year_month(r, cal, None)
}

/// AddDurationToYearMonth.
fn add(agent: &mut Agent, a: &CallArgs<'_>, subtract: bool) -> JsResult<Value> {
    let (ym, cal) = this_ym(agent, &a.this, if subtract { "subtract" } else { "add" })?;
    let mut d = agent.to_temporal_duration(&a.arg(0))?;
    if subtract {
        d = d.negated();
    }
    let reject = agent.temporal_overflow_of(&a.arg(1))?;
    if d.0[2..].iter().any(|v| *v != 0.0) {
        return agent.range_error("only years and months can be added to a PlainYearMonth");
    }
    let mut fields = Fields::from_date(cal, ym, true, false);
    fields.day = Some(1.0);
    let start = agent.temporal_date_from_fields(cal, fields, false)?;
    let dd = agent.temporal_date_duration_without_time(&d)?;
    let added = agent.temporal_date_add(cal, start, dd, reject)?;
    let f = Fields::from_date(cal, added, true, false);
    let r = agent.temporal_year_month_from_fields(cal, f, reject)?;
    agent.temporal_create_year_month(r, cal, None)
}

fn ym_add(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    add(agent, a, false)
}

fn ym_subtract(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    add(agent, a, true)
}

/// DifferenceTemporalPlainYearMonth.
fn difference(agent: &mut Agent, a: &CallArgs<'_>, since: bool) -> JsResult<Value> {
    let (ym, cal) = this_ym(agent, &a.this, if since { "since" } else { "until" })?;
    let (other, other_cal) = agent.to_temporal_year_month(&a.arg(0), &Value::Undefined)?;
    if cal != other_cal {
        return agent
            .range_error("cannot compute the difference between dates of different calendars");
    }
    let o = agent.temporal_options_object(&a.arg(1))?;
    let (largest, smallest, increment, mode) = agent.temporal_difference_settings(
        since,
        o,
        UnitGroup::Date,
        &[Unit::Week, Unit::Day],
        Unit::Month,
        Unit::Year,
    )?;
    if ym == other {
        return agent.temporal_create_duration([0.0; 10], None);
    }
    let mut f1 = Fields::from_date(cal, ym, true, false);
    f1.day = Some(1.0);
    let this_date = agent.temporal_date_from_fields(cal, f1, false)?;
    let mut f2 = Fields::from_date(cal, other, true, false);
    f2.day = Some(1.0);
    let other_date = agent.temporal_date_from_fields(cal, f2, false)?;
    let mut dd = super::duration::date_until(cal, this_date, other_date, largest);
    dd.weeks = 0;
    dd.days = 0;
    let mut d = Internal { date: dd, time: 0 };
    if smallest != Unit::Month || increment != 1 {
        let start = IsoDateTime {
            date: this_date,
            time: Time::MIDNIGHT,
        };
        let end = IsoDateTime {
            date: other_date,
            time: Time::MIDNIGHT,
        };
        d = agent.temporal_round_relative(
            cal,
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

fn ym_until(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    difference(agent, a, false)
}

fn ym_since(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    difference(agent, a, true)
}

fn ym_equals(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let this = this_ym(agent, &a.this, "equals")?;
    let other = agent.to_temporal_year_month(&a.arg(0), &Value::Undefined)?;
    Ok(Value::Boolean(this == other))
}

/// TemporalYearMonthToString.
fn ym_string(d: IsoDate, cal: Cal, show: &str) -> String {
    let mut s = format!("{}-{:02}", iso::pad_year(d.year), d.month);
    if show == "always" || show == "critical" || !cal.is_iso() {
        s.push_str(&format!("-{:02}", d.day));
    }
    s.push_str(&calendar_annotation(cal, show));
    s
}

fn ym_to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (d, cal) = this_ym(agent, &a.this, "toString")?;
    let o = agent.temporal_options_object(&a.arg(0))?;
    let show = agent.temporal_show_calendar(o)?;
    Ok(js_str(&ym_string(d, cal, &show)))
}

fn ym_to_locale_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_ym(agent, &a.this, "toLocaleString")?;
    crate::builtins::intl::date_time_format::temporal_to_locale_string(
        agent,
        &a.this,
        &a.arg(0),
        &a.arg(1),
        crate::builtins::intl::date_time_format::Required::Date,
        crate::builtins::intl::date_time_format::Defaults::Date,
        None,
    )
}

fn ym_to_json(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (d, cal) = this_ym(agent, &a.this, "toJSON")?;
    Ok(js_str(&ym_string(d, cal, "auto")))
}

/// Temporal.PlainYearMonth.prototype.toPlainDate ( item ).
fn ym_to_plain_date(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (d, cal) = this_ym(agent, &a.this, "toPlainDate")?;
    let Value::Object(o) = a.arg(0) else {
        return agent.type_error("toPlainDate() requires an object with a day");
    };
    let fields = Fields::from_date(cal, d, true, false);
    let input = agent.temporal_prepare_fields(cal, o, &[Field::Day], Required::List(&[]))?;
    let merged = fields.merge(cal, input);
    let r = agent.temporal_date_from_fields(cal, merged, false)?;
    agent.temporal_create_date(r, cal, None)
}
