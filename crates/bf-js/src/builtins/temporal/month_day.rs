//! Temporal.PlainMonthDay: un día de un mes (con un año de referencia ISO).

use super::iso::{self, IsoDate};
use super::parse::{self, Goal};
use super::{
    Cal, DATE_FIELDS, Field, Fields, ISO8601, Required, TemporalData, calendar_annotation, class,
    create, data_of, getters, js_str,
};
use crate::builtins::FnEntry;
use crate::heap::Gc;
use crate::object::Object;
use crate::value::Value;
use crate::vm::function::{CallArgs, NativeFn};
use crate::vm::{Agent, JsResult};

fn this_md(agent: &mut Agent, this: &Value, method: &str) -> JsResult<(IsoDate, Cal)> {
    match super::this_data(agent, this, "PlainMonthDay", method, |d| {
        matches!(d, TemporalData::PlainMonthDay(..))
    })? {
        TemporalData::PlainMonthDay(d, c) => Ok((d, c)),
        _ => unreachable!(),
    }
}

macro_rules! md_getters {
    ($($name:literal => $f:ident),* $(,)?) => {
        $(
            fn $f(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
                let (d, cal) = this_md(agent, &a.this, $name)?;
                Ok(super::plain_date::calendar_field(d, cal, $name))
            }
        )*
        const G: &[(&str, NativeFn)] = &[$(($name, $f)),*];
    };
}

md_getters!("calendarId" => g_calendar_id, "monthCode" => g_month_code, "day" => g_day);

pub fn init(agent: &mut Agent, temporal: Gc<Object>) -> Gc<Object> {
    const STATICS: &[FnEntry] = &[("from", 1, md_from)];
    let (_, proto) = class(agent, temporal, "PlainMonthDay", 2, md_ctor, STATICS);
    getters(agent, proto, G);
    const METHODS: &[FnEntry] = &[
        ("with", 1, md_with),
        ("equals", 1, md_equals),
        ("toString", 0, md_to_string),
        ("toLocaleString", 0, md_to_locale_string),
        ("toJSON", 0, md_to_json),
        ("valueOf", 0, super::value_of),
        ("toPlainDate", 1, md_to_plain_date),
    ];
    crate::builtins::methods(agent, proto, METHODS);
    proto
}

impl Agent {
    /// CreateTemporalMonthDay.
    fn temporal_create_month_day(
        &mut self,
        d: IsoDate,
        cal: Cal,
        new_target: Option<Gc<Object>>,
    ) -> JsResult<Value> {
        self.temporal_check_date(d)?;
        create(self, TemporalData::PlainMonthDay(d, cal), new_target)
    }

    /// ToTemporalMonthDay.
    pub fn to_temporal_month_day(
        &mut self,
        item: &Value,
        options: &Value,
    ) -> JsResult<(IsoDate, Cal)> {
        if let Value::Object(o) = item {
            if let Some(TemporalData::PlainMonthDay(d, c)) = data_of(self, item) {
                self.temporal_overflow_of(options)?;
                return Ok((d, c));
            }
            let o = *o;
            let cal = self.temporal_calendar_with_default(o)?;
            let f = self.temporal_prepare_fields(cal, o, DATE_FIELDS, Required::List(&[]))?;
            let reject = self.temporal_overflow_of(options)?;
            return Ok((self.temporal_month_day_from_fields(cal, f, reject)?, cal));
        }
        let Value::String(s) = item else {
            return self.type_error("month-day must be an object or a string");
        };
        let s = s.to_string_lossy();
        let Some(r) = parse::parse_iso_date_time(&s, &[Goal::MonthDay]) else {
            return self.range_error(&format!("Invalid month-day: {s}"));
        };
        let cal = self.temporal_canonicalize_calendar(r.calendar.as_deref().unwrap_or(ISO8601))?;
        self.temporal_overflow_of(options)?;
        if cal.is_iso() {
            return Ok((IsoDate::new(1972, r.month, r.day.unwrap_or(1)), cal));
        }
        // Fuera de ISO, el string tiene la fecha completa (YYYY-MM-DD[u-ca=…]).
        let Some(y) = r.year else {
            return self.range_error(&format!(
                "A month-day string needs a year with the {} calendar",
                cal.id()
            ));
        };
        let d = IsoDate::new(y, r.month, r.day.unwrap_or(1));
        self.temporal_check_date(d)?;
        let f = Fields::from_date(cal, d, false, true);
        Ok((self.temporal_month_day_from_fields(cal, f, false)?, cal))
    }
}

/// Temporal.PlainMonthDay ( isoMonth, isoDay [ , calendar [ , referenceISOYear ] ] ).
fn md_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor Temporal.PlainMonthDay requires 'new'");
    };
    let m = agent.to_integer_with_truncation(&a.arg(0))?;
    let d = agent.to_integer_with_truncation(&a.arg(1))?;
    let cal = agent.temporal_calendar_arg(&a.arg(2))?;
    let y = if a.arg(3).is_undefined() {
        1972.0
    } else {
        agent.to_integer_with_truncation(&a.arg(3))?
    };
    if !iso::within_i32(y as i64) || !iso::is_valid_date(y as i64, m as i64, d as i64) {
        return agent.range_error("invalid month-day");
    }
    agent.temporal_create_month_day(IsoDate::new(y as i64, m as i64, d as i64), cal, Some(nt))
}

fn md_from(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (d, cal) = agent.to_temporal_month_day(&a.arg(0), &a.arg(1))?;
    agent.temporal_create_month_day(d, cal, None)
}

/// Temporal.PlainMonthDay.prototype.with ( temporalMonthDayLike [ , options ] ).
fn md_with(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (d, cal) = this_md(agent, &a.this, "with")?;
    let like = a.arg(0);
    if !agent.temporal_is_partial(&like)? {
        return agent.type_error("with() requires a partial month-day object");
    }
    let Value::Object(o) = like else {
        unreachable!()
    };
    let fields = Fields::from_date(cal, d, false, true);
    let partial = agent.temporal_prepare_fields(cal, o, DATE_FIELDS, Required::Partial)?;
    let merged = fields.merge(cal, partial);
    let reject = agent.temporal_overflow_of(&a.arg(1))?;
    let r = agent.temporal_month_day_from_fields(cal, merged, reject)?;
    agent.temporal_create_month_day(r, cal, None)
}

fn md_equals(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let this = this_md(agent, &a.this, "equals")?;
    let other = agent.to_temporal_month_day(&a.arg(0), &Value::Undefined)?;
    Ok(Value::Boolean(this == other))
}

/// TemporalMonthDayToString.
fn md_string(d: IsoDate, cal: Cal, show: &str) -> String {
    let mut s = String::new();
    if show == "always" || show == "critical" || !cal.is_iso() {
        s.push_str(&format!("{}-", iso::pad_year(d.year)));
    }
    s.push_str(&format!("{:02}-{:02}", d.month, d.day));
    s.push_str(&calendar_annotation(cal, show));
    s
}

fn md_to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (d, cal) = this_md(agent, &a.this, "toString")?;
    let o = agent.temporal_options_object(&a.arg(0))?;
    let show = agent.temporal_show_calendar(o)?;
    Ok(js_str(&md_string(d, cal, &show)))
}

fn md_to_locale_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (d, cal) = this_md(agent, &a.this, "toLocaleString")?;
    Ok(js_str(&md_string(d, cal, "auto")))
}

fn md_to_json(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (d, cal) = this_md(agent, &a.this, "toJSON")?;
    Ok(js_str(&md_string(d, cal, "auto")))
}

/// Temporal.PlainMonthDay.prototype.toPlainDate ( item ).
fn md_to_plain_date(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (d, cal) = this_md(agent, &a.this, "toPlainDate")?;
    let Value::Object(o) = a.arg(0) else {
        return agent.type_error("toPlainDate() requires an object with a year");
    };
    let fields = Fields::from_date(cal, d, false, true);
    let input = agent.temporal_prepare_fields(cal, o, &[Field::Year], Required::List(&[]))?;
    let merged = fields.merge(cal, input);
    let r = agent.temporal_date_from_fields(cal, merged, false)?;
    agent.temporal_create_date(r, cal, None)
}
