//! La aritmética del calendario ISO 8601 y de las horas (Temporal, ECMA-262 2026 §§ ISO Date
//! Records, Time Records, ISO Date-Time Records): validez, balanceo, días desde la época,
//! límites y comparación. Todo en enteros (`i64`/`i128`) para que sea exacto.

use std::cmp::Ordering;

pub const NS_PER_DAY: i128 = 86_400_000_000_000;
/// nsMaxInstant (10⁸ días).
pub const NS_MAX_INSTANT: i128 = 8_640_000_000_000_000_000_000;
pub const NS_MIN_INSTANT: i128 = -NS_MAX_INSTANT;
/// maxTimeDuration = 2⁵³ × 10⁹ − 1.
pub const MAX_TIME_DURATION: i128 = 9_007_199_254_740_992 * 1_000_000_000 - 1;

/// ISO Date Record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct IsoDate {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

/// Time Record (sin `[[Days]]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Time {
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub millisecond: u16,
    pub microsecond: u16,
    pub nanosecond: u16,
}

/// ISO Date-Time Record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct IsoDateTime {
    pub date: IsoDate,
    pub time: Time,
}

pub fn is_leap_year(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// ISODaysInMonth.
pub fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if is_leap_year(y) {
                29
            } else {
                28
            }
        }
    }
}

pub fn days_in_year(y: i64) -> i64 {
    if is_leap_year(y) { 366 } else { 365 }
}

/// IsValidISODate.
pub fn is_valid_date(y: i64, m: i64, d: i64) -> bool {
    (1..=12).contains(&m) && d >= 1 && d <= days_in_month(y, m)
}

/// ISODateToEpochDays (con el mes 1–12).
pub fn epoch_days(y: i64, m: i64, d: i64) -> i64 {
    // days_from_civil (Howard Hinnant).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// EpochDaysToISODate (el año en `i64`).
pub fn from_epoch_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

impl IsoDate {
    pub fn new(year: i64, month: i64, day: i64) -> IsoDate {
        IsoDate {
            year: year as i32,
            month: month as u8,
            day: day as u8,
        }
    }

    pub fn epoch_days(self) -> i64 {
        epoch_days(self.year.into(), self.month.into(), self.day.into())
    }

    pub fn from_epoch_days(days: i64) -> IsoDate {
        let (y, m, d) = from_epoch_days(days);
        IsoDate::new(y, m, d)
    }

    /// AddDaysToISODate.
    pub fn add_days(self, days: i64) -> IsoDate {
        IsoDate::from_epoch_days(self.epoch_days() + days)
    }

    /// ISODayOfWeek (lunes = 1).
    pub fn day_of_week(self) -> i64 {
        (self.epoch_days() + 3).rem_euclid(7) + 1
    }

    /// ISODayOfYear.
    pub fn day_of_year(self) -> i64 {
        self.epoch_days() - epoch_days(self.year.into(), 1, 1) + 1
    }

    /// ISOWeekOfYear: (semana, año de la semana).
    pub fn week_of_year(self) -> (i64, i64) {
        let year = i64::from(self.year);
        let doy = self.day_of_year();
        let dow = self.day_of_week();
        let week = (doy - dow + 10) / 7;
        if week < 1 {
            // La última semana del año anterior.
            let prev = IsoDate::new(year - 1, 12, 31);
            return (prev.week_of_year().0, year - 1);
        }
        let max_week = {
            let jan1 = IsoDate::new(year, 1, 1).day_of_week();
            if jan1 == 4 || (jan1 == 3 && is_leap_year(year)) {
                53
            } else {
                52
            }
        };
        if week > max_week {
            return (1, year + 1);
        }
        (week, year)
    }
}

/// BalanceISOYearMonth.
pub fn balance_year_month(y: i64, m: i64) -> (i64, i64) {
    (y + (m - 1).div_euclid(12), (m - 1).rem_euclid(12) + 1)
}

/// BalanceISODate (año, mes y día en `i64`; el resultado puede quedar fuera de los
/// límites: quien llama lo comprueba).
pub fn balance_date(y: i64, m: i64, d: i64) -> (i64, i64, i64) {
    let (y, m) = balance_year_month(y, m);
    from_epoch_days(epoch_days(y, m, 1) + d - 1)
}

/// Un año, mes y día en `i64` que ya están en el rango de IsoDate.
pub fn within_i32(y: i64) -> bool {
    (-300_000..=300_000).contains(&y)
}

impl Time {
    pub const MIDNIGHT: Time = Time {
        hour: 0,
        minute: 0,
        second: 0,
        millisecond: 0,
        microsecond: 0,
        nanosecond: 0,
    };
    pub const NOON: Time = Time {
        hour: 12,
        minute: 0,
        second: 0,
        millisecond: 0,
        microsecond: 0,
        nanosecond: 0,
    };

    /// Los nanosegundos desde la medianoche.
    pub fn nanos(self) -> i128 {
        ((((i128::from(self.hour) * 60 + i128::from(self.minute)) * 60 + i128::from(self.second))
            * 1000
            + i128::from(self.millisecond))
            * 1000
            + i128::from(self.microsecond))
            * 1000
            + i128::from(self.nanosecond)
    }

    /// Desde nanosegundos en [0, nsPerDay).
    pub fn from_nanos(ns: i128) -> Time {
        let ns = ns as i64;
        Time {
            hour: (ns / 3_600_000_000_000) as u8,
            minute: (ns / 60_000_000_000 % 60) as u8,
            second: (ns / 1_000_000_000 % 60) as u8,
            millisecond: (ns / 1_000_000 % 1000) as u16,
            microsecond: (ns / 1000 % 1000) as u16,
            nanosecond: (ns % 1000) as u16,
        }
    }
}

/// IsValidTime.
pub fn is_valid_time(h: f64, mi: f64, s: f64, ms: f64, us: f64, ns: f64) -> bool {
    (0.0..=23.0).contains(&h)
        && (0.0..=59.0).contains(&mi)
        && (0.0..=59.0).contains(&s)
        && (0.0..=999.0).contains(&ms)
        && (0.0..=999.0).contains(&us)
        && (0.0..=999.0).contains(&ns)
}

/// ConstrainTime: cada componente al rango.
pub fn constrain_time(h: f64, mi: f64, s: f64, ms: f64, us: f64, ns: f64) -> Time {
    Time {
        hour: h.clamp(0.0, 23.0) as u8,
        minute: mi.clamp(0.0, 59.0) as u8,
        second: s.clamp(0.0, 59.0) as u8,
        millisecond: ms.clamp(0.0, 999.0) as u16,
        microsecond: us.clamp(0.0, 999.0) as u16,
        nanosecond: ns.clamp(0.0, 999.0) as u16,
    }
}

/// BalanceTime con los componentes ya combinados en nanosegundos: (días, hora).
pub fn balance_time(ns: i128) -> (i64, Time) {
    let days = ns.div_euclid(NS_PER_DAY);
    (days as i64, Time::from_nanos(ns.rem_euclid(NS_PER_DAY)))
}

/// Los nanosegundos de componentes de hora arbitrarios (para BalanceTime).
pub fn time_components_nanos(h: f64, mi: f64, s: f64, ms: f64, us: f64, ns: f64) -> i128 {
    (h as i128) * 3_600_000_000_000
        + (mi as i128) * 60_000_000_000
        + (s as i128) * 1_000_000_000
        + (ms as i128) * 1_000_000
        + (us as i128) * 1000
        + (ns as i128)
}

impl IsoDateTime {
    /// GetUTCEpochNanoseconds.
    pub fn epoch_ns(self) -> i128 {
        i128::from(self.date.epoch_days()) * NS_PER_DAY + self.time.nanos()
    }

    /// El ISO Date-Time Record de unos nanosegundos desde la época (BalanceISODateTime
    /// desde 1970-01-01T00:00).
    pub fn from_epoch_ns(ns: i128) -> IsoDateTime {
        let days = ns.div_euclid(NS_PER_DAY);
        IsoDateTime {
            date: IsoDate::from_epoch_days(days as i64),
            time: Time::from_nanos(ns.rem_euclid(NS_PER_DAY)),
        }
    }
}

/// ISODateTimeWithinLimits.
pub fn date_time_within_limits(dt: IsoDateTime) -> bool {
    if dt.date.epoch_days().abs() > 100_000_001 {
        return false;
    }
    let ns = dt.epoch_ns();
    ns > NS_MIN_INSTANT - NS_PER_DAY && ns < NS_MAX_INSTANT + NS_PER_DAY
}

/// ISODateWithinLimits: con la hora del mediodía.
pub fn date_within_limits(d: IsoDate) -> bool {
    date_time_within_limits(IsoDateTime {
        date: d,
        time: Time::NOON,
    })
}

/// ISODateWithinLimits para fechas en `i64` (pueden no caber en IsoDate).
pub fn date_within_limits_i64(y: i64, m: i64, d: i64) -> bool {
    within_i32(y) && date_within_limits(IsoDate::new(y, m, d))
}

/// ISOYearMonthWithinLimits.
pub fn year_month_within_limits(y: i64, m: i64) -> bool {
    !(y < -271_821 || y > 275_760 || (y == -271_821 && m < 4) || (y == 275_760 && m > 9))
}

/// IsValidEpochNanoseconds.
pub fn is_valid_epoch_ns(ns: i128) -> bool {
    (NS_MIN_INSTANT..=NS_MAX_INSTANT).contains(&ns)
}

/// CompareISODate.
pub fn compare_date(a: IsoDate, b: IsoDate) -> i32 {
    match a.cmp(&b) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// CompareISODateTime.
pub fn compare_date_time(a: IsoDateTime, b: IsoDateTime) -> i32 {
    match a.cmp(&b) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

pub fn sign_i128(x: i128) -> i32 {
    match x.cmp(&0) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// ISODateSurpasses.
pub fn date_surpasses(sign: i64, y1: i64, m1: i64, d1: i64, two: IsoDate) -> bool {
    let (y2, m2, d2) = (
        i64::from(two.year),
        i64::from(two.month),
        i64::from(two.day),
    );
    if y1 != y2 {
        sign * (y1 - y2) > 0
    } else if m1 != m2 {
        sign * (m1 - m2) > 0
    } else if d1 != d2 {
        sign * (d1 - d2) > 0
    } else {
        false
    }
}

/// PadISOYear.
pub fn pad_year(y: i32) -> String {
    if (0..=9999).contains(&y) {
        format!("{y:04}")
    } else {
        let sign = if y < 0 { '-' } else { '+' };
        format!("{sign}{:06}", y.unsigned_abs())
    }
}

/// TemporalDateToString sin la anotación del calendario.
pub fn date_string(d: IsoDate) -> String {
    format!("{}-{:02}-{:02}", pad_year(d.year), d.month, d.day)
}

/// FormatFractionalSeconds.
pub fn fractional_seconds(sub_second_ns: u32, precision: Option<u8>) -> String {
    match precision {
        None => {
            if sub_second_ns == 0 {
                return String::new();
            }
            let s = format!("{sub_second_ns:09}");
            format!(".{}", s.trim_end_matches('0'))
        }
        Some(0) => String::new(),
        Some(p) => {
            let s = format!("{sub_second_ns:09}");
            format!(".{}", &s[..p as usize])
        }
    }
}

/// La precisión de TimeRecordToString/ISODateTimeToString: minuto, auto o 0–9 dígitos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precision {
    Minute,
    Auto,
    Digits(u8),
}

/// FormatTimeString / TimeRecordToString.
pub fn time_string(t: Time, precision: Precision) -> String {
    let hm = format!("{:02}:{:02}", t.hour, t.minute);
    match precision {
        Precision::Minute => hm,
        Precision::Auto | Precision::Digits(_) => {
            let sub = u32::from(t.millisecond) * 1_000_000
                + u32::from(t.microsecond) * 1000
                + u32::from(t.nanosecond);
            let p = match precision {
                Precision::Digits(d) => Some(d),
                _ => None,
            };
            format!("{hm}:{:02}{}", t.second, fractional_seconds(sub, p))
        }
    }
}

/// ISODateTimeToString sin la anotación del calendario.
pub fn date_time_string(dt: IsoDateTime, precision: Precision) -> String {
    format!(
        "{}T{}",
        date_string(dt.date),
        time_string(dt.time, precision)
    )
}

/// FormatOffsetTimeZoneIdentifier (minutos).
pub fn format_offset_minutes(minutes: i64) -> String {
    let sign = if minutes < 0 { '-' } else { '+' };
    let a = minutes.abs();
    format!("{sign}{:02}:{:02}", a / 60, a % 60)
}

/// FormatUTCOffsetNanoseconds.
pub fn format_offset_ns(ns: i64) -> String {
    let sign = if ns < 0 { '-' } else { '+' };
    let a = ns.unsigned_abs();
    let h = a / 3_600_000_000_000;
    let m = a / 60_000_000_000 % 60;
    let s = a / 1_000_000_000 % 60;
    let sub = (a % 1_000_000_000) as u32;
    if s == 0 && sub == 0 {
        format!("{sign}{h:02}:{m:02}")
    } else {
        format!(
            "{sign}{h:02}:{m:02}:{s:02}{}",
            fractional_seconds(sub, None)
        )
    }
}
