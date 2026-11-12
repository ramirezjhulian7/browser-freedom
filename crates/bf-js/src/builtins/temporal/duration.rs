//! Temporal.Duration y la aritmética de duraciones de Temporal: Date Duration Records,
//! Internal Duration Records (la parte de hora en nanosegundos exactos), CalendarDateAdd y
//! CalendarDateUntil del calendario ISO 8601, las diferencias entre fechas
//! (DifferenceISODateTime, DifferenceZonedDateTime) y el redondeo relativo
//! (RoundRelativeDuration: NudgeToCalendarUnit, NudgeToZonedTime, NudgeToDayOrTime,
//! BubbleRelativeDuration; TotalRelativeDuration).

use super::calendar::Cal;
use super::iso::{self, IsoDate, IsoDateTime, MAX_TIME_DURATION, NS_PER_DAY, Time};
use super::options::{
    RoundingMode, Unit, UnitGroup, UnitOption, div_to_f64, integer_string, round_to_increment,
    rounds_up, unsigned_mode,
};
use super::{Field, Required, TemporalData, class, create, data_of, getters, js_str};
use crate::builtins::FnEntry;
use crate::heap::Gc;
use crate::object::Object;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult};

/// Los diez campos de una Temporal.Duration (Numbers enteros).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Duration(pub [f64; 10]);

pub const FIELD_NAMES: [&str; 10] = [
    "years",
    "months",
    "weeks",
    "days",
    "hours",
    "minutes",
    "seconds",
    "milliseconds",
    "microseconds",
    "nanoseconds",
];

impl Duration {
    /// DurationSign.
    pub fn sign(&self) -> i32 {
        for v in self.0 {
            if v < 0.0 {
                return -1;
            }
            if v > 0.0 {
                return 1;
            }
        }
        0
    }

    /// DefaultTemporalLargestUnit.
    pub fn default_largest_unit(&self) -> Unit {
        for (k, v) in self.0.iter().enumerate() {
            if *v != 0.0 {
                return super::options::UNITS[k];
            }
        }
        Unit::Nanosecond
    }

    pub fn negated(&self) -> Duration {
        let mut d = *self;
        for v in &mut d.0 {
            *v = if *v == 0.0 { 0.0 } else { -*v };
        }
        d
    }

    /// TimeDurationFromComponents de la parte de hora.
    pub fn time_nanos(&self) -> i128 {
        let f = |x: f64| x as i128;
        f(self.0[4]) * 3_600_000_000_000
            + f(self.0[5]) * 60_000_000_000
            + f(self.0[6]) * 1_000_000_000
            + f(self.0[7]) * 1_000_000
            + f(self.0[8]) * 1000
            + f(self.0[9])
    }

    /// ToInternalDurationRecord.
    pub fn internal(&self) -> Internal {
        Internal {
            date: DateDuration {
                years: self.0[0] as i64,
                months: self.0[1] as i64,
                weeks: self.0[2] as i64,
                days: self.0[3] as i64,
            },
            time: self.time_nanos(),
        }
    }

    /// ToInternalDurationRecordWith24HourDays.
    pub fn internal_24h(&self) -> Internal {
        let mut i = self.internal();
        i.time += i128::from(i.date.days) * NS_PER_DAY;
        i.date.days = 0;
        i
    }
}

/// IsValidDuration.
pub fn is_valid(d: &[f64; 10]) -> bool {
    let mut sign = 0;
    for v in d {
        if !v.is_finite() {
            return false;
        }
        let s = if *v < 0.0 {
            -1
        } else if *v > 0.0 {
            1
        } else {
            0
        };
        if s != 0 {
            if sign != 0 && s != sign {
                return false;
            }
            sign = s;
        }
    }
    const MAX: f64 = 4_294_967_296.0;
    if d[0].abs() >= MAX || d[1].abs() >= MAX || d[2].abs() >= MAX {
        return false;
    }
    // normalizedSeconds < 2⁵³, exacto (los campos son enteros).
    let big = |x: f64| x.abs() as u128;
    let ns: u128 = big(d[3]) * 86_400_000_000_000
        + big(d[4]) * 3_600_000_000_000
        + big(d[5]) * 60_000_000_000
        + big(d[6]) * 1_000_000_000
        + big(d[7]) * 1_000_000
        + big(d[8]) * 1000
        + big(d[9]);
    ns < (1u128 << 53) * 1_000_000_000
}

/// Date Duration Record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DateDuration {
    pub years: i64,
    pub months: i64,
    pub weeks: i64,
    pub days: i64,
}

impl DateDuration {
    /// DateDurationSign.
    pub fn sign(&self) -> i32 {
        for v in [self.years, self.months, self.weeks, self.days] {
            if v != 0 {
                return v.signum() as i32;
            }
        }
        0
    }
}

/// Internal Duration Record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Internal {
    pub date: DateDuration,
    pub time: i128,
}

impl Internal {
    /// InternalDurationSign.
    pub fn sign(&self) -> i32 {
        let s = self.date.sign();
        if s != 0 { s } else { iso::sign_i128(self.time) }
    }
}

impl Agent {
    /// CreateDateDurationRecord (valida con IsValidDuration).
    pub fn temporal_date_duration(
        &mut self,
        years: i64,
        months: i64,
        weeks: i64,
        days: i64,
    ) -> JsResult<DateDuration> {
        let f = [
            years as f64,
            months as f64,
            weeks as f64,
            days as f64,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
        ];
        if !is_valid(&f) {
            return self.range_error("duration is out of range");
        }
        Ok(DateDuration {
            years,
            months,
            weeks,
            days,
        })
    }

    /// Una time duration validada (abs ≤ maxTimeDuration).
    pub fn temporal_check_time_duration(&mut self, t: i128) -> JsResult<i128> {
        if t.abs() > MAX_TIME_DURATION {
            return self.range_error("time duration is out of range");
        }
        Ok(t)
    }

    /// CreateTemporalDuration.
    pub fn temporal_create_duration(
        &mut self,
        f: [f64; 10],
        new_target: Option<Gc<Object>>,
    ) -> JsResult<Value> {
        if !is_valid(&f) {
            return self.range_error("duration is out of range");
        }
        let mut f = f;
        for v in &mut f {
            if *v == 0.0 {
                *v = 0.0;
            }
        }
        create(self, TemporalData::Duration(Duration(f)), new_target)
    }

    /// TemporalDurationFromInternal.
    pub fn temporal_from_internal(&mut self, d: Internal, largest: Unit) -> JsResult<Duration> {
        let sign = iso::sign_i128(d.time);
        let mut ns = d.time.unsigned_abs();
        let (mut days, mut hours, mut minutes, mut seconds, mut ms, mut us) =
            (0u128, 0, 0, 0, 0, 0);
        match largest {
            Unit::Year | Unit::Month | Unit::Week | Unit::Day => {
                us = ns / 1000;
                ns %= 1000;
                ms = us / 1000;
                us %= 1000;
                seconds = ms / 1000;
                ms %= 1000;
                minutes = seconds / 60;
                seconds %= 60;
                hours = minutes / 60;
                minutes %= 60;
                days = hours / 24;
                hours %= 24;
            }
            Unit::Hour => {
                us = ns / 1000;
                ns %= 1000;
                ms = us / 1000;
                us %= 1000;
                seconds = ms / 1000;
                ms %= 1000;
                minutes = seconds / 60;
                seconds %= 60;
                hours = minutes / 60;
                minutes %= 60;
            }
            Unit::Minute => {
                us = ns / 1000;
                ns %= 1000;
                ms = us / 1000;
                us %= 1000;
                seconds = ms / 1000;
                ms %= 1000;
                minutes = seconds / 60;
                seconds %= 60;
            }
            Unit::Second => {
                us = ns / 1000;
                ns %= 1000;
                ms = us / 1000;
                us %= 1000;
                seconds = ms / 1000;
                ms %= 1000;
            }
            Unit::Millisecond => {
                us = ns / 1000;
                ns %= 1000;
                ms = us / 1000;
                us %= 1000;
            }
            Unit::Microsecond => {
                us = ns / 1000;
                ns %= 1000;
            }
            Unit::Nanosecond => {}
        }
        let s = f64::from(sign);
        let v = |x: u128| x as f64 * s;
        let f = [
            d.date.years as f64,
            d.date.months as f64,
            d.date.weeks as f64,
            d.date.days as f64 + v(days),
            v(hours),
            v(minutes),
            v(seconds),
            v(ms),
            v(us),
            v(ns),
        ];
        if !is_valid(&f) {
            return self.range_error("duration is out of range");
        }
        let mut f = f;
        for x in &mut f {
            if *x == 0.0 {
                *x = 0.0;
            }
        }
        Ok(Duration(f))
    }

    /// ToTemporalDuration.
    pub fn to_temporal_duration(&mut self, item: &Value) -> JsResult<Duration> {
        if let Some(TemporalData::Duration(d)) = data_of(self, item) {
            return Ok(d);
        }
        let Value::Object(o) = item else {
            let Value::String(s) = item else {
                return self.type_error("duration must be an object or a string");
            };
            let s = s.to_string_lossy();
            let Some(f) = super::parse::parse_duration(&s) else {
                return self.range_error(&format!("Invalid duration: {s}"));
            };
            if !is_valid(&f) {
                return self.range_error("duration is out of range");
            }
            return Ok(Duration(f));
        };
        let partial = self.temporal_partial_duration(*o)?;
        let f = partial.map(|v| v.unwrap_or(0.0));
        if !is_valid(&f) {
            return self.range_error("duration is out of range");
        }
        Ok(Duration(f))
    }

    /// ToTemporalPartialDurationRecord (los campos en orden alfabético).
    pub fn temporal_partial_duration(&mut self, o: Gc<Object>) -> JsResult<[Option<f64>; 10]> {
        const ORDER: [usize; 10] = [3, 4, 8, 7, 5, 1, 9, 6, 2, 0];
        let mut out = [None; 10];
        let mut any = false;
        for idx in ORDER {
            let v = self.get(o, &PropertyKey::from(FIELD_NAMES[idx]), Value::Object(o))?;
            if !v.is_undefined() {
                any = true;
                out[idx] = Some(self.to_integer_if_integral(&v)?);
            }
        }
        if !any {
            return self.type_error("duration-like object must have at least one field");
        }
        Ok(out)
    }

    // --- Calendario ISO 8601 ---

    /// CalendarDateAdd (NonISODateAdd fuera de ISO 8601).
    pub fn temporal_date_add(
        &mut self,
        cal: Cal,
        date: IsoDate,
        d: DateDuration,
        reject: bool,
    ) -> JsResult<IsoDate> {
        if !cal.is_iso() {
            return match cal.add(date, d, reject) {
                Some(r) => Ok(r),
                None => self.range_error("the date does not exist or is out of range"),
            };
        }
        let (y, m) = iso::balance_year_month(
            i64::from(date.year) + d.years,
            i64::from(date.month) + d.months,
        );
        let (y, m, day) =
            self.temporal_regulate_date(y as f64, m as f64, f64::from(date.day), reject)?;
        if !iso::within_i32(y) {
            return self.range_error("date is outside the representable range");
        }
        let (y, m, day) = iso::balance_date(y, m, day + d.days + 7 * d.weeks);
        if !iso::date_within_limits_i64(y, m, day) {
            return self.range_error("date is outside the representable range");
        }
        Ok(IsoDate::new(y, m, day))
    }

    /// AdjustDateDurationRecord.
    pub fn temporal_adjust_date_duration(
        &mut self,
        d: DateDuration,
        days: i64,
        weeks: Option<i64>,
        months: Option<i64>,
    ) -> JsResult<DateDuration> {
        self.temporal_date_duration(
            d.years,
            months.unwrap_or(d.months),
            weeks.unwrap_or(d.weeks),
            days,
        )
    }
}

/// CalendarDateUntil (NonISODateUntil fuera de ISO 8601).
pub fn date_until(cal: Cal, one: IsoDate, two: IsoDate, largest: Unit) -> DateDuration {
    if !cal.is_iso() {
        return cal.until(one, two, largest);
    }
    let sign = -i64::from(iso::compare_date(one, two));
    if sign == 0 {
        return DateDuration::default();
    }
    let (y1, m1, d1) = (
        i64::from(one.year),
        i64::from(one.month),
        i64::from(one.day),
    );
    let mut years = 0;
    let mut months = 0;
    if matches!(largest, Unit::Year | Unit::Month) {
        let mut candidate_years = i64::from(two.year) - y1;
        if candidate_years != 0 {
            candidate_years -= sign;
        }
        while !iso::date_surpasses(sign, y1 + candidate_years, m1, d1, two) {
            years = candidate_years;
            candidate_years += sign;
        }
        let mut candidate_months = sign;
        let (mut iy, mut im) = iso::balance_year_month(y1 + years, m1 + candidate_months);
        while !iso::date_surpasses(sign, iy, im, d1, two) {
            months = candidate_months;
            candidate_months += sign;
            (iy, im) = iso::balance_year_month(iy, im + sign);
        }
        if largest == Unit::Month {
            months += years * 12;
            years = 0;
        }
    }
    let (iy, im) = iso::balance_year_month(y1 + years, m1 + months);
    let cd = d1.min(iso::days_in_month(iy, im));
    let constrained = iso::epoch_days(iy, im, cd);
    let total_days = two.epoch_days() - constrained;
    let mut weeks = 0;
    let mut days = total_days;
    if largest == Unit::Week {
        weeks = total_days / 7;
        days = total_days % 7;
    }
    DateDuration {
        years,
        months,
        weeks,
        days,
    }
}

/// DifferenceTime.
pub fn difference_time(a: Time, b: Time) -> i128 {
    b.nanos() - a.nanos()
}

/// El epoch de un ISO Date-Time en la zona (None: UTC sin zona).
fn epoch_for(agent: &mut Agent, tz: Option<&str>, dt: IsoDateTime) -> JsResult<i128> {
    match tz {
        None => Ok(dt.epoch_ns()),
        Some(tz) => agent.temporal_epoch_ns_for(tz, dt),
    }
}

/// DifferenceISODateTime.
pub fn difference_iso_date_time(
    cal: Cal,
    a: IsoDateTime,
    b: IsoDateTime,
    largest: Unit,
) -> Internal {
    let mut time = difference_time(a.time, b.time);
    let time_sign = iso::sign_i128(time);
    let date_sign = iso::compare_date(b.date, a.date);
    let mut adjusted = b.date;
    if time_sign == -date_sign {
        adjusted = adjusted.add_days(i64::from(time_sign));
        time -= i128::from(time_sign) * NS_PER_DAY;
    }
    let date_largest = Unit::Day.larger(largest);
    let mut dd = date_until(cal, a.date, adjusted, date_largest);
    if largest != date_largest {
        time += i128::from(dd.days) * NS_PER_DAY;
        dd.days = 0;
    }
    Internal { date: dd, time }
}

/// Una Duration Nudge Result.
struct Nudge {
    duration: Internal,
    nudged_ns: i128,
    expanded: bool,
}

/// Un total exacto: entero + num/den.
pub struct Total {
    pub whole: i128,
    pub num: i128,
    pub den: i128,
}

impl Total {
    pub fn to_f64(&self) -> f64 {
        div_to_f64(self.whole * self.den + self.num, self.den)
    }
}

impl Agent {
    /// NudgeToCalendarUnit: el resultado y el total.
    #[allow(clippy::too_many_arguments)]
    fn nudge_to_calendar_unit(
        &mut self,
        cal: Cal,
        sign: i64,
        d: Internal,
        origin_ns: i128,
        dest_ns: i128,
        dt: IsoDateTime,
        tz: Option<&str>,
        increment: i128,
        unit: Unit,
        mode: RoundingMode,
    ) -> JsResult<(Nudge, Total)> {
        let inc = increment as i64;
        let trunc =
            |x: i64| round_to_increment(i128::from(x), increment, RoundingMode::Trunc) as i64;
        let mut r1 = match unit {
            Unit::Year => trunc(d.date.years),
            Unit::Month => trunc(d.date.months),
            Unit::Week => {
                let ym = self.temporal_adjust_date_duration(d.date, 0, Some(0), None)?;
                let weeks_start = self.temporal_date_add(cal, dt.date, ym, false)?;
                let weeks_end = weeks_start.add_days(d.date.days);
                let until = date_until(cal, weeks_start, weeks_end, Unit::Week);
                trunc(d.date.weeks + until.weeks)
            }
            _ => trunc(d.date.days),
        };
        // La duración con `r` unidades de `unit` (y las mayores de `d`).
        let with_units = |agent: &mut Agent, r: i64| -> JsResult<DateDuration> {
            match unit {
                Unit::Year => agent.temporal_date_duration(r, 0, 0, 0),
                Unit::Month => agent.temporal_adjust_date_duration(d.date, 0, Some(0), Some(r)),
                Unit::Week => agent.temporal_adjust_date_duration(d.date, 0, Some(r), None),
                _ => agent.temporal_adjust_date_duration(d.date, r, None, None),
            }
        };
        // El epoch del origen más una duración (el origen mismo si la duración es cero).
        let epoch_of = |agent: &mut Agent, dd: DateDuration| -> JsResult<i128> {
            if dd.sign() == 0 {
                return Ok(origin_ns);
            }
            let date = agent.temporal_date_add(cal, dt.date, dd, false)?;
            epoch_for(
                agent,
                tz,
                IsoDateTime {
                    date,
                    time: dt.time,
                },
            )
        };
        let mut start_d = with_units(self, r1)?;
        let mut end_d = with_units(self, r1 + inc * sign)?;
        let mut start_ns = epoch_of(self, start_d)?;
        let mut end_ns = epoch_of(self, end_d)?;
        // Si el destino quedó más allá del final (un día que no existe en el mes o el año
        // del final), la ventana es la siguiente.
        let mut tries = 0;
        while i128::from(sign) * (dest_ns - end_ns) > 0 && tries < 2 {
            r1 += inc * sign;
            start_d = end_d;
            start_ns = end_ns;
            end_d = with_units(self, r1 + inc * sign)?;
            end_ns = epoch_of(self, end_d)?;
            tries += 1;
        }
        let in_range = if sign == 1 {
            start_ns <= dest_ns && dest_ns <= end_ns
        } else {
            end_ns <= dest_ns && dest_ns <= start_ns
        };
        if !in_range || start_ns == end_ns {
            return self.range_error("rounding is not possible with this relative date");
        }
        let num = dest_ns - start_ns;
        let den = end_ns - start_ns;
        // total = r1 + progress × increment × sign.
        let total = Total {
            whole: i128::from(r1),
            num: num * increment * i128::from(sign),
            den,
        };
        let (num_abs, den_abs) = (num.abs(), den.abs());
        let um = unsigned_mode(mode, sign < 0);
        let r1_index = (i128::from(r1.abs())) / increment;
        let up = if num_abs == den_abs {
            true
        } else {
            rounds_up(num_abs, den_abs, um, r1_index % 2 == 0)
        };
        let nudge = if up {
            Nudge {
                duration: Internal {
                    date: end_d,
                    time: 0,
                },
                nudged_ns: end_ns,
                expanded: true,
            }
        } else {
            Nudge {
                duration: Internal {
                    date: start_d,
                    time: 0,
                },
                nudged_ns: start_ns,
                expanded: false,
            }
        };
        Ok((nudge, total))
    }

    /// NudgeToZonedTime.
    #[allow(clippy::too_many_arguments)]
    fn nudge_to_zoned_time(
        &mut self,
        cal: Cal,
        sign: i64,
        d: Internal,
        dt: IsoDateTime,
        tz: &str,
        increment: i128,
        unit: Unit,
        mode: RoundingMode,
    ) -> JsResult<Nudge> {
        let start = self.temporal_date_add(cal, dt.date, d.date, false)?;
        let end_date = start.add_days(sign);
        let start_ns = self.temporal_epoch_ns_for(
            tz,
            IsoDateTime {
                date: start,
                time: dt.time,
            },
        )?;
        let end_ns = self.temporal_epoch_ns_for(
            tz,
            IsoDateTime {
                date: end_date,
                time: dt.time,
            },
        )?;
        let day_span = end_ns - start_ns;
        let step = increment * unit.nanos();
        let mut rounded = round_to_increment(d.time, step, mode);
        self.temporal_check_time_duration(rounded)?;
        let beyond = rounded - day_span;
        let (expanded, day_delta, nudged) = if iso::sign_i128(beyond) != -(sign as i32) {
            rounded = round_to_increment(beyond, step, mode);
            self.temporal_check_time_duration(rounded)?;
            (true, sign, end_ns + rounded)
        } else {
            (false, 0, start_ns + rounded)
        };
        let date =
            self.temporal_adjust_date_duration(d.date, d.date.days + day_delta, None, None)?;
        Ok(Nudge {
            duration: Internal {
                date,
                time: rounded,
            },
            nudged_ns: nudged,
            expanded,
        })
    }

    /// NudgeToDayOrTime.
    fn nudge_to_day_or_time(
        &mut self,
        d: Internal,
        dest_ns: i128,
        largest: Unit,
        increment: i128,
        smallest: Unit,
        mode: RoundingMode,
    ) -> JsResult<Nudge> {
        let time =
            self.temporal_check_time_duration(d.time + i128::from(d.date.days) * NS_PER_DAY)?;
        let rounded = round_to_increment(time, smallest.nanos() * increment, mode);
        self.temporal_check_time_duration(rounded)?;
        let diff = rounded - time;
        let whole_days = time / NS_PER_DAY;
        let rounded_whole_days = rounded / NS_PER_DAY;
        let day_delta = rounded_whole_days - whole_days;
        let expanded = iso::sign_i128(day_delta) == iso::sign_i128(time);
        let nudged = dest_ns + diff;
        let (days, remainder) = if largest.is_date() {
            (
                rounded_whole_days as i64,
                rounded - rounded_whole_days * NS_PER_DAY,
            )
        } else {
            (0, rounded)
        };
        let date = self.temporal_adjust_date_duration(d.date, days, None, None)?;
        Ok(Nudge {
            duration: Internal {
                date,
                time: remainder,
            },
            nudged_ns: nudged,
            expanded,
        })
    }

    /// BubbleRelativeDuration.
    #[allow(clippy::too_many_arguments)]
    fn bubble_relative_duration(
        &mut self,
        cal: Cal,
        sign: i64,
        mut d: Internal,
        nudged_ns: i128,
        dt: IsoDateTime,
        tz: Option<&str>,
        largest: Unit,
        smallest: Unit,
    ) -> JsResult<Internal> {
        if smallest == largest {
            return Ok(d);
        }
        let units = super::options::UNITS;
        let largest_index = units.iter().position(|u| *u == largest).expect("unidad");
        let smallest_index = units.iter().position(|u| *u == smallest).expect("unidad");
        let mut index = smallest_index as i64 - 1;
        while index >= largest_index as i64 {
            let unit = units[index as usize];
            index -= 1;
            if unit == Unit::Week && largest != Unit::Week {
                continue;
            }
            let end_d = match unit {
                Unit::Year => self.temporal_date_duration(d.date.years + sign, 0, 0, 0)?,
                Unit::Month => self.temporal_adjust_date_duration(
                    d.date,
                    0,
                    Some(0),
                    Some(d.date.months + sign),
                )?,
                _ => {
                    self.temporal_adjust_date_duration(d.date, 0, Some(d.date.weeks + sign), None)?
                }
            };
            let end = self.temporal_date_add(cal, dt.date, end_d, false)?;
            let end_ns = epoch_for(
                self,
                tz,
                IsoDateTime {
                    date: end,
                    time: dt.time,
                },
            )?;
            let beyond = nudged_ns - end_ns;
            if iso::sign_i128(beyond) != -(sign as i32) {
                d = Internal {
                    date: end_d,
                    time: 0,
                };
            } else {
                break;
            }
        }
        Ok(d)
    }

    /// RoundRelativeDuration.
    #[allow(clippy::too_many_arguments)]
    pub fn temporal_round_relative(
        &mut self,
        cal: Cal,
        d: Internal,
        origin_ns: i128,
        dest_ns: i128,
        dt: IsoDateTime,
        tz: Option<&str>,
        largest: Unit,
        increment: i128,
        smallest: Unit,
        mode: RoundingMode,
    ) -> JsResult<Internal> {
        let irregular = smallest.is_calendar() || (tz.is_some() && smallest == Unit::Day);
        let sign = if d.sign() < 0 { -1 } else { 1 };
        let nudge = if irregular {
            self.nudge_to_calendar_unit(
                cal, sign, d, origin_ns, dest_ns, dt, tz, increment, smallest, mode,
            )?
            .0
        } else if let Some(tz) = tz {
            self.nudge_to_zoned_time(cal, sign, d, dt, tz, increment, smallest, mode)?
        } else {
            self.nudge_to_day_or_time(d, dest_ns, largest, increment, smallest, mode)?
        };
        let mut result = nudge.duration;
        if nudge.expanded && smallest != Unit::Week {
            let start_unit = smallest.larger(Unit::Day);
            result = self.bubble_relative_duration(
                cal,
                sign,
                result,
                nudge.nudged_ns,
                dt,
                tz,
                largest,
                start_unit,
            )?;
        }
        Ok(result)
    }

    /// TotalRelativeDuration.
    #[allow(clippy::too_many_arguments)]
    pub fn temporal_total_relative(
        &mut self,
        cal: Cal,
        d: Internal,
        origin_ns: i128,
        dest_ns: i128,
        dt: IsoDateTime,
        tz: Option<&str>,
        unit: Unit,
    ) -> JsResult<f64> {
        if unit.is_calendar() || (tz.is_some() && unit == Unit::Day) {
            let sign = if d.sign() < 0 { -1 } else { 1 };
            let (_, total) = self.nudge_to_calendar_unit(
                cal,
                sign,
                d,
                origin_ns,
                dest_ns,
                dt,
                tz,
                1,
                unit,
                RoundingMode::Trunc,
            )?;
            return Ok(total.to_f64());
        }
        let time =
            self.temporal_check_time_duration(d.time + i128::from(d.date.days) * NS_PER_DAY)?;
        Ok(div_to_f64(time, unit.nanos()))
    }

    /// DifferencePlainDateTimeWithRounding.
    #[allow(clippy::too_many_arguments)]
    pub fn temporal_diff_date_time_rounding(
        &mut self,
        cal: Cal,
        a: IsoDateTime,
        b: IsoDateTime,
        largest: Unit,
        increment: i128,
        smallest: Unit,
        mode: RoundingMode,
    ) -> JsResult<Internal> {
        if a == b {
            return Ok(Internal::default());
        }
        if !iso::date_time_within_limits(a) || !iso::date_time_within_limits(b) {
            return self.range_error("date-time is outside the representable range");
        }
        let diff = difference_iso_date_time(cal, a, b, largest);
        if smallest == Unit::Nanosecond && increment == 1 {
            return Ok(diff);
        }
        self.temporal_round_relative(
            cal,
            diff,
            a.epoch_ns(),
            b.epoch_ns(),
            a,
            None,
            largest,
            increment,
            smallest,
            mode,
        )
    }

    /// DifferencePlainDateTimeWithTotal.
    pub fn temporal_diff_date_time_total(
        &mut self,
        cal: Cal,
        a: IsoDateTime,
        b: IsoDateTime,
        unit: Unit,
    ) -> JsResult<f64> {
        if a == b {
            return Ok(0.0);
        }
        if !iso::date_time_within_limits(a) || !iso::date_time_within_limits(b) {
            return self.range_error("date-time is outside the representable range");
        }
        let diff = difference_iso_date_time(cal, a, b, unit);
        if unit == Unit::Nanosecond {
            return Ok(diff.time as f64);
        }
        self.temporal_total_relative(cal, diff, a.epoch_ns(), b.epoch_ns(), a, None, unit)
    }

    /// DifferenceZonedDateTime.
    pub fn temporal_diff_zoned(
        &mut self,
        cal: Cal,
        ns1: i128,
        ns2: i128,
        tz: &str,
        largest: Unit,
    ) -> JsResult<Internal> {
        if ns1 == ns2 {
            return Ok(Internal::default());
        }
        let start = super::date_time_for(tz, ns1);
        let end = super::date_time_for(tz, ns2);
        if start.date == end.date {
            return Ok(Internal {
                date: DateDuration::default(),
                time: ns2 - ns1,
            });
        }
        let sign: i64 = if ns2 < ns1 { -1 } else { 1 };
        let max_correction = if sign == 1 { 2 } else { 1 };
        let mut correction = 0;
        let time = difference_time(start.time, end.time);
        if i64::from(iso::sign_i128(time)) == -sign {
            correction += 1;
        }
        let mut found = None;
        while correction <= max_correction {
            let intermediate_date = end.date.add_days(-correction * sign);
            let intermediate = IsoDateTime {
                date: intermediate_date,
                time: start.time,
            };
            let ins = self.temporal_epoch_ns_for(tz, intermediate)?;
            let t = ns2 - ins;
            correction += 1;
            if sign != -i64::from(iso::sign_i128(t)) {
                found = Some((intermediate, t));
                break;
            }
        }
        let Some((intermediate, time)) = found else {
            return self.range_error("could not compute the difference");
        };
        let dd = date_until(
            cal,
            start.date,
            intermediate.date,
            largest.larger(Unit::Day),
        );
        Ok(Internal { date: dd, time })
    }

    /// DifferenceZonedDateTimeWithRounding.
    #[allow(clippy::too_many_arguments)]
    pub fn temporal_diff_zoned_rounding(
        &mut self,
        cal: Cal,
        ns1: i128,
        ns2: i128,
        tz: &str,
        largest: Unit,
        increment: i128,
        smallest: Unit,
        mode: RoundingMode,
    ) -> JsResult<Internal> {
        if !largest.is_date() {
            return self.temporal_diff_instant(ns1, ns2, increment, smallest, mode);
        }
        let diff = self.temporal_diff_zoned(cal, ns1, ns2, tz, largest)?;
        if smallest == Unit::Nanosecond && increment == 1 {
            return Ok(diff);
        }
        let dt = super::date_time_for(tz, ns1);
        self.temporal_round_relative(
            cal,
            diff,
            ns1,
            ns2,
            dt,
            Some(tz),
            largest,
            increment,
            smallest,
            mode,
        )
    }

    /// DifferenceZonedDateTimeWithTotal.
    pub fn temporal_diff_zoned_total(
        &mut self,
        cal: Cal,
        ns1: i128,
        ns2: i128,
        tz: &str,
        unit: Unit,
    ) -> JsResult<f64> {
        if !unit.is_date() {
            return Ok(div_to_f64(ns2 - ns1, unit.nanos()));
        }
        let diff = self.temporal_diff_zoned(cal, ns1, ns2, tz, unit)?;
        let dt = super::date_time_for(tz, ns1);
        self.temporal_total_relative(cal, diff, ns1, ns2, dt, Some(tz), unit)
    }

    /// DifferenceInstant.
    pub fn temporal_diff_instant(
        &mut self,
        ns1: i128,
        ns2: i128,
        increment: i128,
        smallest: Unit,
        mode: RoundingMode,
    ) -> JsResult<Internal> {
        let t = self.temporal_round_time_duration(ns2 - ns1, increment, smallest, mode)?;
        Ok(Internal {
            date: DateDuration::default(),
            time: t,
        })
    }

    /// RoundTimeDuration.
    pub fn temporal_round_time_duration(
        &mut self,
        t: i128,
        increment: i128,
        unit: Unit,
        mode: RoundingMode,
    ) -> JsResult<i128> {
        let r = round_to_increment(t, increment * unit.nanos(), mode);
        self.temporal_check_time_duration(r)
    }

    /// AddZonedDateTime.
    pub fn temporal_add_zoned(
        &mut self,
        cal: Cal,
        ns: i128,
        tz: &str,
        d: Internal,
        reject: bool,
    ) -> JsResult<i128> {
        if d.date.sign() == 0 {
            return self.temporal_add_instant(ns, d.time);
        }
        let dt = super::date_time_for(tz, ns);
        let added = self.temporal_date_add(cal, dt.date, d.date, reject)?;
        let inter = IsoDateTime {
            date: added,
            time: dt.time,
        };
        if !iso::date_time_within_limits(inter) {
            return self.range_error("date-time is outside the representable range");
        }
        let ins = self.temporal_epoch_ns_for(tz, inter)?;
        self.temporal_add_instant(ins, d.time)
    }

    /// AddInstant.
    pub fn temporal_add_instant(&mut self, ns: i128, t: i128) -> JsResult<i128> {
        let r = ns + t;
        if !iso::is_valid_epoch_ns(r) {
            return self.range_error("instant is outside the representable range");
        }
        Ok(r)
    }

    /// GetDifferenceSettings: (largest, smallest, increment, mode).
    #[allow(clippy::too_many_arguments)]
    pub fn temporal_difference_settings(
        &mut self,
        since: bool,
        options: Gc<Object>,
        group: UnitGroup,
        disallowed: &[Unit],
        fallback_smallest: Unit,
        smallest_largest_default: Unit,
    ) -> JsResult<(Unit, Unit, i128, RoundingMode)> {
        let largest = self.temporal_unit_option(options, "largestUnit", false)?;
        let increment = self.temporal_rounding_increment(options)?;
        let mut mode = self.temporal_rounding_mode(options, RoundingMode::Trunc)?;
        let smallest = self.temporal_unit_option(options, "smallestUnit", false)?;
        self.temporal_validate_unit(largest, group, true, false)?;
        let largest = match largest {
            UnitOption::Unset | UnitOption::Auto => None,
            UnitOption::Unit(u) => Some(u),
        };
        if let Some(u) = largest
            && disallowed.contains(&u)
        {
            return self.range_error(&format!("{} is not allowed here", u.singular()));
        }
        if since {
            mode = mode.negate();
        }
        self.temporal_validate_unit(smallest, group, false, false)?;
        let smallest = match smallest {
            UnitOption::Unit(u) => u,
            _ => fallback_smallest,
        };
        if disallowed.contains(&smallest) {
            return self.range_error(&format!("{} is not allowed here", smallest.singular()));
        }
        let default_largest = smallest_largest_default.larger(smallest);
        let largest = largest.unwrap_or(default_largest);
        if largest.larger(smallest) != largest {
            return self.range_error("largestUnit must be larger than smallestUnit");
        }
        if let Some(max) = smallest.max_increment() {
            self.temporal_validate_increment(increment, max, false)?;
        }
        Ok((largest, smallest, increment, mode))
    }
}

// --- relativeTo ---

/// El relativeTo de Duration.round/total/compare.
pub enum RelativeTo {
    None,
    Plain(IsoDate, Cal),
    Zoned { ns: i128, tz: String, cal: Cal },
}

impl Agent {
    /// GetTemporalRelativeToOption.
    pub fn temporal_relative_to(&mut self, options: Gc<Object>) -> JsResult<RelativeTo> {
        let v = self.get(
            options,
            &PropertyKey::from("relativeTo"),
            Value::Object(options),
        )?;
        if v.is_undefined() {
            return Ok(RelativeTo::None);
        }
        let mut exact = false;
        let mut match_minutes = false;
        let (date, time, tz, offset, cal) = if let Value::Object(o) = v {
            match data_of(self, &v) {
                Some(TemporalData::ZonedDateTime { ns, tz, cal }) => {
                    return Ok(RelativeTo::Zoned { ns, tz, cal });
                }
                Some(TemporalData::PlainDate(d, c)) => return Ok(RelativeTo::Plain(d, c)),
                Some(TemporalData::PlainDateTime(dt, c)) => {
                    return Ok(RelativeTo::Plain(dt.date, c));
                }
                _ => {}
            }
            let cal = self.temporal_calendar_with_default(o)?;
            let mut names = super::DATE_FIELDS.to_vec();
            names.extend_from_slice(super::TIME_FIELDS);
            names.push(Field::Offset);
            names.push(Field::TimeZone);
            let f = self.temporal_prepare_fields(cal, o, &names, Required::List(&[]))?;
            let tz = f.time_zone.clone();
            let offset = f.offset.clone();
            let dt = self.temporal_date_time_from_fields(cal, f, false)?;
            (dt.date, Some(dt.time), tz, offset, cal)
        } else {
            let Value::String(s) = &v else {
                return self.type_error("relativeTo must be an object or a string");
            };
            let s = s.to_string_lossy();
            use super::parse::Goal;
            let Some(r) = super::parse::parse_iso_date_time(
                &s,
                &[
                    Goal::DateTime { zoned: true },
                    Goal::DateTime { zoned: false },
                ],
            ) else {
                return self.range_error(&format!("Invalid relativeTo: {s}"));
            };
            let tz = match &r.tz_annotation {
                None => None,
                Some(a) => {
                    let tz = self.to_temporal_time_zone(&js_str(a))?;
                    if r.z {
                        exact = true;
                    }
                    match_minutes = !r.offset_sub_minute;
                    Some(tz)
                }
            };
            let cal = match &r.calendar {
                Some(c) => self.temporal_canonicalize_calendar(c)?,
                None => Cal::Iso,
            };
            let date = IsoDate::new(r.year.unwrap_or(0), r.month, r.day.unwrap_or(1));
            let offset = if tz.is_some() { r.offset.clone() } else { None };
            (date, r.time, tz, offset, cal)
        };
        let Some(tz) = tz else {
            self.temporal_check_date(date)?;
            return Ok(RelativeTo::Plain(date, cal));
        };
        let offset_ns = offset.map(|o| i128::from(super::parse::parse_offset_ns(&o).unwrap_or(0)));
        let ns = self.temporal_interpret_offset(
            date,
            time,
            exact,
            offset_ns,
            &tz,
            false,
            "reject",
            match_minutes,
        )?;
        Ok(RelativeTo::Zoned { ns, tz, cal })
    }
}

// --- Temporal.Duration ---

pub fn init(agent: &mut Agent, temporal: Gc<Object>) -> Gc<Object> {
    const STATICS: &[FnEntry] = &[("from", 1, duration_from), ("compare", 2, duration_compare)];
    let (_, proto) = class(agent, temporal, "Duration", 0, duration_ctor, STATICS);
    getters(
        agent,
        proto,
        &[
            ("years", get_years),
            ("months", get_months),
            ("weeks", get_weeks),
            ("days", get_days),
            ("hours", get_hours),
            ("minutes", get_minutes),
            ("seconds", get_seconds),
            ("milliseconds", get_milliseconds),
            ("microseconds", get_microseconds),
            ("nanoseconds", get_nanoseconds),
            ("sign", get_sign),
            ("blank", get_blank),
        ],
    );
    const METHODS: &[FnEntry] = &[
        ("with", 1, duration_with),
        ("negated", 0, duration_negated),
        ("abs", 0, duration_abs),
        ("add", 1, duration_add),
        ("subtract", 1, duration_subtract),
        ("round", 1, duration_round),
        ("total", 1, duration_total),
        ("toString", 0, duration_to_string),
        ("toJSON", 0, duration_to_json),
        ("toLocaleString", 0, duration_to_locale_string),
        ("valueOf", 0, super::value_of),
    ];
    crate::builtins::methods(agent, proto, METHODS);
    proto
}

fn this_duration(agent: &mut Agent, this: &Value, method: &str) -> JsResult<Duration> {
    match super::this_data(agent, this, "Duration", method, |d| {
        matches!(d, TemporalData::Duration(_))
    })? {
        TemporalData::Duration(d) => Ok(d),
        _ => unreachable!(),
    }
}

/// Temporal.Duration ( … ).
fn duration_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor Temporal.Duration requires 'new'");
    };
    let mut f = [0f64; 10];
    for (k, v) in f.iter_mut().enumerate() {
        let x = a.arg(k);
        if !x.is_undefined() {
            *v = agent.to_integer_if_integral(&x)?;
        }
    }
    agent.temporal_create_duration(f, Some(nt))
}

fn duration_from(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = agent.to_temporal_duration(&a.arg(0))?;
    agent.temporal_create_duration(d.0, None)
}

/// Temporal.Duration.compare ( one, two [ , options ] ).
fn duration_compare(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let one = agent.to_temporal_duration(&a.arg(0))?;
    let two = agent.to_temporal_duration(&a.arg(1))?;
    let options = agent.temporal_options_object(&a.arg(2))?;
    let rel = agent.temporal_relative_to(options)?;
    if one == two {
        return Ok(Value::Number(0.0));
    }
    let (lu1, lu2) = (one.default_largest_unit(), two.default_largest_unit());
    let (d1, d2) = (one.internal(), two.internal());
    if let RelativeTo::Zoned { ns, tz, cal } = &rel
        && (lu1.is_date() || lu2.is_date())
    {
        let after1 = agent.temporal_add_zoned(*cal, *ns, tz, d1, false)?;
        let after2 = agent.temporal_add_zoned(*cal, *ns, tz, d2, false)?;
        return Ok(Value::Number(f64::from(iso::sign_i128(after1 - after2))));
    }
    let (days1, days2) = if lu1.is_calendar() || lu2.is_calendar() {
        let RelativeTo::Plain(date, cal) = rel else {
            return agent.range_error("relativeTo is required for comparing calendar units");
        };
        (
            agent.temporal_date_duration_days(cal, d1.date, date)?,
            agent.temporal_date_duration_days(cal, d2.date, date)?,
        )
    } else {
        (one.0[3] as i64, two.0[3] as i64)
    };
    let t1 = agent.temporal_check_time_duration(d1.time + i128::from(days1) * NS_PER_DAY)?;
    let t2 = agent.temporal_check_time_duration(d2.time + i128::from(days2) * NS_PER_DAY)?;
    Ok(Value::Number(f64::from(iso::sign_i128(t1 - t2))))
}

impl Agent {
    /// DateDurationDays.
    fn temporal_date_duration_days(
        &mut self,
        cal: Cal,
        d: DateDuration,
        rel: IsoDate,
    ) -> JsResult<i64> {
        let ymw = self.temporal_adjust_date_duration(d, 0, None, None)?;
        if ymw.sign() == 0 {
            return Ok(d.days);
        }
        let later = self.temporal_date_add(cal, rel, ymw, false)?;
        Ok(d.days + later.epoch_days() - rel.epoch_days())
    }
}

macro_rules! getter {
    ($name:ident, $idx:expr, $js:literal) => {
        fn $name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
            let d = this_duration(agent, &a.this, $js)?;
            Ok(Value::Number(d.0[$idx]))
        }
    };
}

getter!(get_years, 0, "years");
getter!(get_months, 1, "months");
getter!(get_weeks, 2, "weeks");
getter!(get_days, 3, "days");
getter!(get_hours, 4, "hours");
getter!(get_minutes, 5, "minutes");
getter!(get_seconds, 6, "seconds");
getter!(get_milliseconds, 7, "milliseconds");
getter!(get_microseconds, 8, "microseconds");
getter!(get_nanoseconds, 9, "nanoseconds");

fn get_sign(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_duration(agent, &a.this, "sign")?;
    Ok(Value::Number(f64::from(d.sign())))
}

fn get_blank(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_duration(agent, &a.this, "blank")?;
    Ok(Value::Boolean(d.sign() == 0))
}

/// Temporal.Duration.prototype.with ( temporalDurationLike ).
fn duration_with(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_duration(agent, &a.this, "with")?;
    let Value::Object(o) = a.arg(0) else {
        return agent.type_error("duration-like must be an object");
    };
    let p = agent.temporal_partial_duration(o)?;
    let mut f = d.0;
    for k in 0..10 {
        if let Some(v) = p[k] {
            f[k] = v;
        }
    }
    agent.temporal_create_duration(f, None)
}

fn duration_negated(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_duration(agent, &a.this, "negated")?;
    agent.temporal_create_duration(d.negated().0, None)
}

fn duration_abs(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_duration(agent, &a.this, "abs")?;
    agent.temporal_create_duration(d.0.map(f64::abs), None)
}

/// AddDurations.
fn add_durations(agent: &mut Agent, a: &CallArgs<'_>, subtract: bool) -> JsResult<Value> {
    let d = this_duration(agent, &a.this, if subtract { "subtract" } else { "add" })?;
    let mut other = agent.to_temporal_duration(&a.arg(0))?;
    if subtract {
        other = other.negated();
    }
    let largest = d
        .default_largest_unit()
        .larger(other.default_largest_unit());
    if largest.is_calendar() {
        return agent.range_error("cannot add durations with calendar units without relativeTo");
    }
    let t = d.internal_24h().time + other.internal_24h().time;
    let t = agent.temporal_check_time_duration(t)?;
    let r = agent.temporal_from_internal(
        Internal {
            date: DateDuration::default(),
            time: t,
        },
        largest,
    )?;
    agent.temporal_create_duration(r.0, None)
}

fn duration_add(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    add_durations(agent, a, false)
}

fn duration_subtract(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    add_durations(agent, a, true)
}

/// Un argumento string → `{ [name]: s }` sin prototipo; si no, GetOptionsObject.
pub(crate) fn string_or_options(agent: &mut Agent, v: &Value, name: &str) -> JsResult<Gc<Object>> {
    match v {
        Value::Undefined => agent.type_error("options argument is required"),
        Value::String(_) => {
            let o = agent.new_object(None);
            agent.create_data_property_or_throw(o, PropertyKey::from(name), v.clone())?;
            Ok(o)
        }
        _ => agent.temporal_options_object(v),
    }
}

/// Temporal.Duration.prototype.round ( roundTo ).
fn duration_round(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_duration(agent, &a.this, "round")?;
    let round_to = string_or_options(agent, &a.arg(0), "smallestUnit")?;
    let largest_opt = agent.temporal_unit_option(round_to, "largestUnit", false)?;
    let rel = agent.temporal_relative_to(round_to)?;
    let increment = agent.temporal_rounding_increment(round_to)?;
    let mode = agent.temporal_rounding_mode(round_to, RoundingMode::HalfExpand)?;
    let smallest_opt = agent.temporal_unit_option(round_to, "smallestUnit", false)?;
    agent.temporal_validate_unit(smallest_opt, UnitGroup::DateTime, false, false)?;
    let smallest_present = !matches!(smallest_opt, UnitOption::Unset);
    let smallest = match smallest_opt {
        UnitOption::Unit(u) => u,
        _ => Unit::Nanosecond,
    };
    let existing_largest = d.default_largest_unit();
    let default_largest = existing_largest.larger(smallest);
    let (largest, largest_present) = match largest_opt {
        UnitOption::Unset => (default_largest, false),
        UnitOption::Auto => (default_largest, true),
        UnitOption::Unit(u) => (u, true),
    };
    agent.temporal_validate_unit(largest_opt, UnitGroup::DateTime, true, false)?;
    if !smallest_present && !largest_present {
        return agent.range_error("at least one of smallestUnit or largestUnit is required");
    }
    if largest.larger(smallest) != largest {
        return agent.range_error("largestUnit must be larger than smallestUnit");
    }
    if let Some(max) = smallest.max_increment() {
        agent.temporal_validate_increment(increment, max, false)?;
    }
    if increment > 1 && largest != smallest && smallest.is_date() {
        return agent.range_error("roundingIncrement must be 1 for this rounding");
    }
    match rel {
        RelativeTo::Zoned { ns, tz, cal } => {
            let internal = d.internal();
            let target = agent.temporal_add_zoned(cal, ns, &tz, internal, false)?;
            let r = agent.temporal_diff_zoned_rounding(
                cal, ns, target, &tz, largest, increment, smallest, mode,
            )?;
            let lu = if largest.is_date() {
                Unit::Hour
            } else {
                largest
            };
            let r = agent.temporal_from_internal(r, lu)?;
            agent.temporal_create_duration(r.0, None)
        }
        RelativeTo::Plain(date, cal) => {
            let internal = d.internal_24h();
            let (days, target_time) = iso::balance_time(internal.time);
            let dd = agent.temporal_adjust_date_duration(internal.date, days, None, None)?;
            let target_date = agent.temporal_date_add(cal, date, dd, false)?;
            let start = IsoDateTime {
                date,
                time: Time::MIDNIGHT,
            };
            let target = IsoDateTime {
                date: target_date,
                time: target_time,
            };
            let r = agent.temporal_diff_date_time_rounding(
                cal, start, target, largest, increment, smallest, mode,
            )?;
            let r = agent.temporal_from_internal(r, largest)?;
            agent.temporal_create_duration(r.0, None)
        }
        RelativeTo::None => {
            if existing_largest.is_calendar() || largest.is_calendar() {
                return agent.range_error("relativeTo is required for calendar units");
            }
            let internal = d.internal_24h();
            let r = if smallest == Unit::Day {
                let days =
                    round_to_increment(internal.time, NS_PER_DAY * increment, mode) / NS_PER_DAY;
                let dd = agent.temporal_date_duration(0, 0, 0, days as i64)?;
                Internal { date: dd, time: 0 }
            } else {
                let t =
                    agent.temporal_round_time_duration(internal.time, increment, smallest, mode)?;
                Internal {
                    date: DateDuration::default(),
                    time: t,
                }
            };
            let r = agent.temporal_from_internal(r, largest)?;
            agent.temporal_create_duration(r.0, None)
        }
    }
}

/// Temporal.Duration.prototype.total ( totalOf ).
fn duration_total(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_duration(agent, &a.this, "total")?;
    let total_of = string_or_options(agent, &a.arg(0), "unit")?;
    let rel = agent.temporal_relative_to(total_of)?;
    let unit = agent.temporal_unit_option(total_of, "unit", true)?;
    agent.temporal_validate_unit(unit, UnitGroup::DateTime, false, false)?;
    let UnitOption::Unit(unit) = unit else {
        return agent.range_error("unit is required");
    };
    let total = match rel {
        RelativeTo::Zoned { ns, tz, cal } => {
            let target = agent.temporal_add_zoned(cal, ns, &tz, d.internal(), false)?;
            agent.temporal_diff_zoned_total(cal, ns, target, &tz, unit)?
        }
        RelativeTo::Plain(date, cal) => {
            let internal = d.internal_24h();
            let (days, target_time) = iso::balance_time(internal.time);
            let dd = agent.temporal_adjust_date_duration(internal.date, days, None, None)?;
            let target_date = agent.temporal_date_add(cal, date, dd, false)?;
            agent.temporal_diff_date_time_total(
                cal,
                IsoDateTime {
                    date,
                    time: Time::MIDNIGHT,
                },
                IsoDateTime {
                    date: target_date,
                    time: target_time,
                },
                unit,
            )?
        }
        RelativeTo::None => {
            if d.default_largest_unit().is_calendar() || unit.is_calendar() {
                return agent.range_error("relativeTo is required for calendar units");
            }
            div_to_f64(d.internal_24h().time, unit.nanos())
        }
    };
    Ok(Value::Number(total))
}

/// TemporalDurationToString.
pub fn duration_string(d: &Duration, precision: Option<u8>) -> String {
    let f = &d.0;
    let mut date = String::new();
    for (k, c) in [(0, 'Y'), (1, 'M'), (2, 'W'), (3, 'D')] {
        if f[k] != 0.0 {
            date.push_str(&integer_string(f[k]));
            date.push(c);
        }
    }
    let mut time = String::new();
    for (k, c) in [(4, 'H'), (5, 'M')] {
        if f[k] != 0.0 {
            time.push_str(&integer_string(f[k]));
            time.push(c);
        }
    }
    let zero_minutes_and_higher = d.default_largest_unit() >= Unit::Second;
    let secs: i128 = (f[6].abs() as i128) * 1_000_000_000
        + (f[7].abs() as i128) * 1_000_000
        + (f[8].abs() as i128) * 1000
        + (f[9].abs() as i128);
    if secs != 0 || zero_minutes_and_higher || precision.is_some() {
        let whole = secs / 1_000_000_000;
        let sub = (secs % 1_000_000_000) as u32;
        time.push_str(&format!(
            "{whole}{}",
            iso::fractional_seconds(sub, precision)
        ));
        time.push('S');
    }
    let mut out = String::new();
    if d.sign() < 0 {
        out.push('-');
    }
    out.push('P');
    out.push_str(&date);
    if !time.is_empty() {
        out.push('T');
        out.push_str(&time);
    }
    out
}

/// Temporal.Duration.prototype.toString ( [ options ] ).
fn duration_to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_duration(agent, &a.this, "toString")?;
    let o = agent.temporal_options_object(&a.arg(0))?;
    let digits = agent.temporal_fractional_digits(o)?;
    let mode = agent.temporal_rounding_mode(o, RoundingMode::Trunc)?;
    let smallest = agent.temporal_unit_option(o, "smallestUnit", false)?;
    agent.temporal_validate_unit(smallest, UnitGroup::Time, false, false)?;
    if matches!(smallest, UnitOption::Unit(Unit::Hour | Unit::Minute)) {
        return agent.range_error("smallestUnit must be seconds or smaller");
    }
    let (precision, unit, increment) = super::seconds_precision(smallest, digits);
    let precision = match precision {
        iso::Precision::Digits(p) => Some(p),
        _ => None,
    };
    if unit == Unit::Nanosecond && increment == 1 {
        return Ok(js_str(&duration_string(&d, precision)));
    }
    let largest = d.default_largest_unit();
    let internal = d.internal();
    let t = agent.temporal_round_time_duration(internal.time, increment, unit, mode)?;
    let rounded = agent.temporal_from_internal(
        Internal {
            date: internal.date,
            time: t,
        },
        largest.larger(Unit::Second),
    )?;
    Ok(js_str(&duration_string(&rounded, precision)))
}

fn duration_to_json(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_duration(agent, &a.this, "toJSON")?;
    Ok(js_str(&duration_string(&d, None)))
}

fn duration_to_locale_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_duration(agent, &a.this, "toLocaleString")?;
    Ok(js_str(&duration_string(&d, None)))
}
