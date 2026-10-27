//! Temporal.PlainMonthDay: un día de un mes (con un año de referencia ISO).

use super::iso::{self, IsoDate};
use super::parse::{self, Goal};
use super::{
    DATE_FIELDS, Field, Fields, ISO8601, Required, TemporalData, calendar_annotation, class,
    create, data_of, getters, js_str,
};
use crate::builtins::FnEntry;
use crate::heap::Gc;
use crate::object::Object;
use crate::value::Value;
use crate::vm::function::{CallArgs, NativeFn};
use crate::vm::{Agent, JsResult};

fn this_md(agent: &mut Agent, this: &Value, method: &str) -> JsResult<IsoDate> {
    match super::this_data(agent, this, "PlainMonthDay", method, |d| {
        matches!(d, TemporalData::PlainMonthDay(_))
    })? {
        TemporalData::PlainMonthDay(d) => Ok(d),
        _ => unreachable!(),
    }
}

macro_rules! md_getters {
    ($($name:literal => $f:ident),* $(,)?) => {
        $(
            fn $f(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
                let d = this_md(agent, &a.this, $name)?;
                Ok(super::plain_date::calendar_field(d, $name))
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
        new_target: Option<Gc<Object>>,
    ) -> JsResult<Value> {
        self.temporal_check_date(d)?;
        create(self, TemporalData::PlainMonthDay(d), new_target)
    }

    /// ToTemporalMonthDay.
    pub fn to_temporal_month_day(&mut self, item: &Value, options: &Value) -> JsResult<IsoDate> {
        if let Value::Object(o) = item {
            if let Some(TemporalData::PlainMonthDay(d)) = data_of(self, item) {
                self.temporal_overflow_of(options)?;
                return Ok(d);
            }
            let o = *o;
            self.temporal_calendar_with_default(o)?;
            let f = self.temporal_prepare_fields(o, DATE_FIELDS, Required::List(&[]))?;
            let reject = self.temporal_overflow_of(options)?;
            return self.temporal_month_day_from_fields(f, reject);
        }
        let Value::String(s) = item else {
            return self.type_error("month-day must be an object or a string");
        };
        let s = s.to_string_lossy();
        let Some(r) = parse::parse_iso_date_time(&s, &[Goal::MonthDay]) else {
            return self.range_error(&format!("Invalid month-day: {s}"));
        };
        self.temporal_canonicalize_calendar(r.calendar.as_deref().unwrap_or(ISO8601))?;
        self.temporal_overflow_of(options)?;
        Ok(IsoDate::new(1972, r.month, r.day.unwrap_or(1)))
    }
}

/// Temporal.PlainMonthDay ( isoMonth, isoDay [ , calendar [ , referenceISOYear ] ] ).
fn md_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor Temporal.PlainMonthDay requires 'new'");
    };
    let m = agent.to_integer_with_truncation(&a.arg(0))?;
    let d = agent.to_integer_with_truncation(&a.arg(1))?;
    agent.temporal_calendar_arg(&a.arg(2))?;
    let y = if a.arg(3).is_undefined() {
        1972.0
    } else {
        agent.to_integer_with_truncation(&a.arg(3))?
    };
    if !iso::within_i32(y as i64) || !iso::is_valid_date(y as i64, m as i64, d as i64) {
        return agent.range_error("invalid month-day");
    }
    agent.temporal_create_month_day(IsoDate::new(y as i64, m as i64, d as i64), Some(nt))
}

fn md_from(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = agent.to_temporal_month_day(&a.arg(0), &a.arg(1))?;
    agent.temporal_create_month_day(d, None)
}

/// Temporal.PlainMonthDay.prototype.with ( temporalMonthDayLike [ , options ] ).
fn md_with(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_md(agent, &a.this, "with")?;
    let like = a.arg(0);
    if !agent.temporal_is_partial(&like)? {
        return agent.type_error("with() requires a partial month-day object");
    }
    let Value::Object(o) = like else {
        unreachable!()
    };
    let fields = Fields::from_date(d, false, true);
    let partial = agent.temporal_prepare_fields(o, DATE_FIELDS, Required::Partial)?;
    let merged = fields.merge(partial);
    let reject = agent.temporal_overflow_of(&a.arg(1))?;
    let r = agent.temporal_month_day_from_fields(merged, reject)?;
    agent.temporal_create_month_day(r, None)
}

fn md_equals(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_md(agent, &a.this, "equals")?;
    let other = agent.to_temporal_month_day(&a.arg(0), &Value::Undefined)?;
    Ok(Value::Boolean(d == other))
}

/// TemporalMonthDayToString.
fn md_string(d: IsoDate, show: &str) -> String {
    let mut s = String::new();
    if show == "always" || show == "critical" {
        s.push_str(&format!("{}-", iso::pad_year(d.year)));
    }
    s.push_str(&format!("{:02}-{:02}", d.month, d.day));
    s.push_str(calendar_annotation(show));
    s
}

fn md_to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_md(agent, &a.this, "toString")?;
    let o = agent.temporal_options_object(&a.arg(0))?;
    let show = agent.temporal_show_calendar(o)?;
    Ok(js_str(&md_string(d, &show)))
}

fn md_to_locale_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_md(agent, &a.this, "toLocaleString")?;
    Ok(js_str(&md_string(d, "auto")))
}

fn md_to_json(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_md(agent, &a.this, "toJSON")?;
    Ok(js_str(&md_string(d, "auto")))
}

/// Temporal.PlainMonthDay.prototype.toPlainDate ( item ).
fn md_to_plain_date(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_md(agent, &a.this, "toPlainDate")?;
    let Value::Object(o) = a.arg(0) else {
        return agent.type_error("toPlainDate() requires an object with a year");
    };
    let fields = Fields::from_date(d, false, true);
    let input = agent.temporal_prepare_fields(o, &[Field::Year], Required::List(&[]))?;
    let merged = fields.merge(input);
    let r = agent.temporal_date_from_fields(merged, false)?;
    agent.temporal_create_date(r, None)
}
