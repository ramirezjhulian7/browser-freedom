//! Date (ECMA-262 §21.4): los algoritmos de tiempo de §21.4.1 (Day, YearFromTime,
//! MakeTime, MakeDay, MakeDate, TimeClip, LocalTime, UTC), el constructor, `Date.now`,
//! `Date.parse` (el formato de §21.4.1.32 y, por compatibilidad, los de `toString` y
//! `toUTCString`), `Date.UTC`, Date.prototype con todos sus getters y setters locales y UTC, y
//! los de Annex B (`getYear`, `setYear`, `toGMTString`).
//!
//! La zona horaria local sale del sistema sin `unsafe` ([`TimeZone::system`]): la variable
//! `TZ` (un nombre IANA, una ruta a un archivo TZif o una regla POSIX) o, en Unix,
//! `/etc/localtime` (TZif, RFC 8536, con la regla POSIX del final para las fechas futuras). En
//! Windows sin `TZ` la zona es UTC: `std` no expone la zona del sistema y leerla exigiría FFI.

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::abstract_ops::conversion::Hint;
use crate::builtins::{FnEntry, constructor, methods};
use crate::heap::Gc;
use crate::object::{Object, ObjectKind, Prop};
use crate::string::JsString;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

const MS_PER_SECOND: f64 = 1000.0;
const MS_PER_MINUTE: f64 = 60_000.0;
const MS_PER_HOUR: f64 = 3_600_000.0;
const MS_PER_DAY: f64 = 86_400_000.0;

pub fn init(agent: &mut Agent, realm: RealmId) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    // %Date.prototype% es un objeto ordinario (§21.4.4).
    let proto = agent.new_object(Some(op));
    agent.realms[realm].intrinsics.date_prototype = proto;
    let ctor = constructor(agent, "Date", 7, date_ctor, proto);
    agent.realms[realm].intrinsics.date_ctor = ctor;
    const STATICS: &[FnEntry] = &[("now", 0, now), ("parse", 1, parse), ("UTC", 7, utc)];
    methods(agent, ctor, STATICS);
    const PROTO: &[FnEntry] = &[
        ("getDate", 0, get_date),
        ("getDay", 0, get_day),
        ("getFullYear", 0, get_full_year),
        ("getHours", 0, get_hours),
        ("getMilliseconds", 0, get_milliseconds),
        ("getMinutes", 0, get_minutes),
        ("getMonth", 0, get_month),
        ("getSeconds", 0, get_seconds),
        ("getTime", 0, get_time),
        ("getTimezoneOffset", 0, get_timezone_offset),
        ("getUTCDate", 0, get_utc_date),
        ("getUTCDay", 0, get_utc_day),
        ("getUTCFullYear", 0, get_utc_full_year),
        ("getUTCHours", 0, get_utc_hours),
        ("getUTCMilliseconds", 0, get_utc_milliseconds),
        ("getUTCMinutes", 0, get_utc_minutes),
        ("getUTCMonth", 0, get_utc_month),
        ("getUTCSeconds", 0, get_utc_seconds),
        ("getYear", 0, get_year),
        ("setDate", 1, set_date),
        ("setFullYear", 3, set_full_year),
        ("setHours", 4, set_hours),
        ("setMilliseconds", 1, set_milliseconds),
        ("setMinutes", 3, set_minutes),
        ("setMonth", 2, set_month),
        ("setSeconds", 2, set_seconds),
        ("setTime", 1, set_time),
        ("setUTCDate", 1, set_utc_date),
        ("setUTCFullYear", 3, set_utc_full_year),
        ("setUTCHours", 4, set_utc_hours),
        ("setUTCMilliseconds", 1, set_utc_milliseconds),
        ("setUTCMinutes", 3, set_utc_minutes),
        ("setUTCMonth", 2, set_utc_month),
        ("setUTCSeconds", 2, set_utc_seconds),
        ("setYear", 1, set_year),
        ("toDateString", 0, to_date_string),
        ("toISOString", 0, to_iso_string),
        ("toJSON", 1, to_json),
        ("toLocaleDateString", 0, to_locale_date_string),
        ("toLocaleString", 0, to_locale_string),
        ("toLocaleTimeString", 0, to_locale_time_string),
        ("toString", 0, to_string),
        ("toTimeString", 0, to_time_string),
        ("valueOf", 0, value_of),
    ];
    methods(agent, proto, PROTO);
    // toUTCString y toGMTString son la misma función (B.2.4.3).
    let f = crate::builtins::method(agent, proto, "toUTCString", 0, to_utc_string);
    crate::builtins::value_prop(agent, proto, "toGMTString", Value::Object(f));
    // Date.prototype [ @@toPrimitive ] ( hint ) (§21.4.4.45): no escribible.
    let tp = agent.create_builtin_function(to_primitive, 1, "[Symbol.toPrimitive]", false);
    let key = PropertyKey::Symbol(agent.well_known.to_primitive);
    agent.heap[proto]
        .props
        .insert(key, Prop::data(Value::Object(tp), false, false, true));
}

// --- Algoritmos de tiempo (§21.4.1) ---

/// Day ( t ).
fn day(t: f64) -> f64 {
    (t / MS_PER_DAY).floor()
}

/// TimeWithinDay ( t ).
fn time_within_day(t: f64) -> f64 {
    t.rem_euclid(MS_PER_DAY)
}

/// DaysInYear ( y ).
fn days_in_year(y: f64) -> f64 {
    if y % 4.0 != 0.0 {
        365.0
    } else if y % 100.0 != 0.0 {
        366.0
    } else if y % 400.0 != 0.0 {
        365.0
    } else {
        366.0
    }
}

/// DayFromYear ( y ).
fn day_from_year(y: f64) -> f64 {
    365.0 * (y - 1970.0) + ((y - 1969.0) / 4.0).floor() - ((y - 1901.0) / 100.0).floor()
        + ((y - 1601.0) / 400.0).floor()
}

/// TimeFromYear ( y ).
fn time_from_year(y: f64) -> f64 {
    MS_PER_DAY * day_from_year(y)
}

/// YearFromTime ( t ): el mayor `y` con TimeFromYear(y) ≤ t.
fn year_from_time(t: f64) -> f64 {
    let mut y = (t / (MS_PER_DAY * 365.2425)).floor() + 1970.0;
    while time_from_year(y) > t {
        y -= 1.0;
    }
    while time_from_year(y + 1.0) <= t {
        y += 1.0;
    }
    y
}

/// InLeapYear ( t ).
fn in_leap_year(t: f64) -> bool {
    days_in_year(year_from_time(t)) == 366.0
}

/// DayWithinYear ( t ).
fn day_within_year(t: f64) -> f64 {
    day(t) - day_from_year(year_from_time(t))
}

/// Los días acumulados antes de cada mes (año no bisiesto).
const CUMULATIVE_DAYS: [f64; 13] = [
    0.0, 31.0, 59.0, 90.0, 120.0, 151.0, 181.0, 212.0, 243.0, 273.0, 304.0, 334.0, 365.0,
];

fn month_start(m: usize, leap: bool) -> f64 {
    CUMULATIVE_DAYS[m] + if leap && m >= 2 { 1.0 } else { 0.0 }
}

/// MonthFromTime ( t ): 0 a 11.
fn month_from_time(t: f64) -> f64 {
    let d = day_within_year(t);
    let leap = in_leap_year(t);
    for m in 0..12 {
        if d < month_start(m + 1, leap) {
            return m as f64;
        }
    }
    11.0
}

/// DateFromTime ( t ): 1 a 31.
fn date_from_time(t: f64) -> f64 {
    let m = month_from_time(t) as usize;
    day_within_year(t) - month_start(m, in_leap_year(t)) + 1.0
}

/// WeekDay ( t ): 0 (domingo) a 6.
fn week_day(t: f64) -> f64 {
    (day(t) + 4.0).rem_euclid(7.0)
}

fn hour_from_time(t: f64) -> f64 {
    (t / MS_PER_HOUR).floor().rem_euclid(24.0)
}

fn min_from_time(t: f64) -> f64 {
    (t / MS_PER_MINUTE).floor().rem_euclid(60.0)
}

fn sec_from_time(t: f64) -> f64 {
    (t / MS_PER_SECOND).floor().rem_euclid(60.0)
}

fn ms_from_time(t: f64) -> f64 {
    t.rem_euclid(MS_PER_SECOND)
}

/// ToIntegerOrInfinity sobre un Number.
fn to_integer(n: f64) -> f64 {
    crate::abstract_ops::conversion::integer_or_infinity(n)
}

/// MakeTime ( hour, min, sec, ms ) (§21.4.1.27).
fn make_time(hour: f64, min: f64, sec: f64, ms: f64) -> f64 {
    if !hour.is_finite() || !min.is_finite() || !sec.is_finite() || !ms.is_finite() {
        return f64::NAN;
    }
    let (h, m, s, milli) = (
        to_integer(hour),
        to_integer(min),
        to_integer(sec),
        to_integer(ms),
    );
    ((h * MS_PER_HOUR + m * MS_PER_MINUTE) + s * MS_PER_SECOND) + milli
}

/// MakeDay ( year, month, date ) (§21.4.1.28).
fn make_day(year: f64, month: f64, date: f64) -> f64 {
    if !year.is_finite() || !month.is_finite() || !date.is_finite() {
        return f64::NAN;
    }
    let (y, m, dt) = (to_integer(year), to_integer(month), to_integer(date));
    let ym = y + (m / 12.0).floor();
    if !ym.is_finite() || ym.abs() > 400_000_000.0 {
        return f64::NAN;
    }
    let mn = m.rem_euclid(12.0) as usize;
    let leap = days_in_year(ym) == 366.0;
    let t_day = day_from_year(ym) + month_start(mn, leap);
    t_day + dt - 1.0
}

/// MakeDate ( day, time ) (§21.4.1.29).
fn make_date(day: f64, time: f64) -> f64 {
    if !day.is_finite() || !time.is_finite() {
        return f64::NAN;
    }
    let tv = day * MS_PER_DAY + time;
    if !tv.is_finite() {
        return f64::NAN;
    }
    tv
}

/// MakeFullYear ( year ) (§21.4.1.30).
fn make_full_year(year: f64) -> f64 {
    if year.is_nan() {
        return f64::NAN;
    }
    let t = to_integer(year);
    if (0.0..=99.0).contains(&t) {
        1900.0 + t
    } else {
        t
    }
}

/// TimeClip ( time ) (§21.4.1.31).
fn time_clip(time: f64) -> f64 {
    if !time.is_finite() || time.abs() > 8.64e15 {
        return f64::NAN;
    }
    to_integer(time) + 0.0
}

/// LocalTime ( t ) (§21.4.1.25).
fn local_time(t: f64) -> f64 {
    t + TimeZone::system().offset_ms(t)
}

/// UTC ( t ) (§21.4.1.26): de hora local a UTC, con la desambiguación del spec (en un
/// solapamiento, el instante anterior; en un hueco, el offset de antes de la transición).
fn utc_of(t: f64) -> f64 {
    if !t.is_finite() {
        return f64::NAN;
    }
    t - TimeZone::system().offset_for_local_ms(t)
}

fn now_ms() -> f64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as f64,
        Err(e) => -(e.duration().as_millis() as f64),
    }
}

// --- Zona horaria ---

/// Una regla POSIX (`TZ=EST5EDT,M3.2.0,M11.1.0`): offsets en segundos al este de UTC.
#[derive(Debug, Clone)]
struct PosixTz {
    std_abbr: String,
    std_offset: i64,
    dst: Option<PosixDst>,
}

#[derive(Debug, Clone)]
struct PosixDst {
    abbr: String,
    offset: i64,
    start: (RuleDate, i64),
    end: (RuleDate, i64),
}

#[derive(Debug, Clone, Copy)]
enum RuleDate {
    /// `Jn`: día 1–365 sin contar el 29 de febrero.
    Julian(u32),
    /// `n`: día 0–365 contando el 29 de febrero.
    Zero(u32),
    /// `Mm.w.d`.
    Month(u32, u32, u32),
}

/// Una zona horaria: transiciones (TZif) y la regla POSIX para después de la última.
#[derive(Debug, Clone)]
pub struct TimeZone {
    name: String,
    /// (instante UTC en segundos, índice del tipo).
    transitions: Vec<(i64, usize)>,
    /// (offset en segundos, abreviatura).
    types: Vec<(i64, String)>,
    rule: Option<PosixTz>,
}

impl TimeZone {
    fn utc() -> TimeZone {
        TimeZone {
            name: "Coordinated Universal Time".into(),
            transitions: Vec::new(),
            types: vec![(0, "UTC".into())],
            rule: None,
        }
    }

    /// La zona del sistema (calculada una vez).
    pub fn system() -> &'static TimeZone {
        static TZ: OnceLock<TimeZone> = OnceLock::new();
        TZ.get_or_init(|| TimeZone::detect().unwrap_or_else(TimeZone::utc))
    }

    fn detect() -> Option<TimeZone> {
        if let Ok(tz) = std::env::var("TZ") {
            let tz = tz.strip_prefix(':').unwrap_or(&tz).to_owned();
            if tz.is_empty() || tz == "UTC" || tz == "UTC0" || tz == "Etc/UTC" {
                return None;
            }
            if let Some(z) = TimeZone::from_zoneinfo_name(&tz) {
                return Some(z);
            }
            return TimeZone::from_posix(&tz);
        }
        if cfg!(unix) {
            let data = std::fs::read("/etc/localtime").ok()?;
            let name = std::fs::read_link("/etc/localtime")
                .ok()
                .and_then(|p| {
                    let s = p.to_string_lossy().into_owned();
                    s.split("zoneinfo/").nth(1).map(str::to_owned)
                })
                .unwrap_or_default();
            return TimeZone::from_tzif(&data, name);
        }
        None
    }

    fn from_zoneinfo_name(name: &str) -> Option<TimeZone> {
        let path = std::path::Path::new(name);
        if path.is_absolute() {
            let data = std::fs::read(path).ok()?;
            return TimeZone::from_tzif(&data, name.to_owned());
        }
        if name.contains("..") {
            return None;
        }
        for dir in [
            "/usr/share/zoneinfo",
            "/usr/lib/zoneinfo",
            "/usr/share/lib/zoneinfo",
            "/etc/zoneinfo",
        ] {
            if let Ok(data) = std::fs::read(std::path::Path::new(dir).join(name))
                && let Some(z) = TimeZone::from_tzif(&data, name.to_owned())
            {
                return Some(z);
            }
        }
        None
    }

    fn from_posix(s: &str) -> Option<TimeZone> {
        let rule = parse_posix_tz(s)?;
        Some(TimeZone {
            name: rule.std_abbr.clone(),
            transitions: Vec::new(),
            types: vec![(rule.std_offset, rule.std_abbr.clone())],
            rule: Some(rule),
        })
    }

    /// Un archivo TZif (RFC 8536), versiones 1 a 4.
    fn from_tzif(data: &[u8], name: String) -> Option<TimeZone> {
        fn be32(d: &[u8], at: usize) -> Option<i64> {
            let b = d.get(at..at + 4)?;
            Some(i64::from(i32::from_be_bytes([b[0], b[1], b[2], b[3]])))
        }
        fn be64(d: &[u8], at: usize) -> Option<i64> {
            let b = d.get(at..at + 8)?;
            let mut a = [0u8; 8];
            a.copy_from_slice(b);
            Some(i64::from_be_bytes(a))
        }
        if data.get(0..4)? != b"TZif" {
            return None;
        }
        let version = *data.get(4)?;
        let counts = |at: usize| -> Option<[usize; 6]> {
            let mut c = [0usize; 6];
            for (k, slot) in c.iter_mut().enumerate() {
                *slot = usize::try_from(be32(data, at + 20 + 4 * k)?).ok()?;
            }
            Some(c)
        };
        // [isutcnt, isstdcnt, leapcnt, timecnt, typecnt, charcnt]
        let c1 = counts(0)?;
        let v1_len = c1[3] * 5 + c1[4] * 6 + c1[5] + c1[2] * 8 + c1[1] + c1[0];
        let (base, c, time_size) = if version >= b'2' {
            let second = 44 + v1_len;
            if data.get(second..second + 4)? != b"TZif" {
                return None;
            }
            (second, counts(second)?, 8)
        } else {
            (0, c1, 4)
        };
        let mut p = base + 44;
        let mut transitions = Vec::with_capacity(c[3]);
        let mut times = Vec::with_capacity(c[3]);
        for _ in 0..c[3] {
            times.push(if time_size == 8 {
                be64(data, p)?
            } else {
                be32(data, p)?
            });
            p += time_size;
        }
        for &t in &times {
            transitions.push((t, usize::from(*data.get(p)?)));
            p += 1;
        }
        let mut raw_types = Vec::with_capacity(c[4]);
        for _ in 0..c[4] {
            let off = be32(data, p)?;
            let idx = usize::from(*data.get(p + 5)?);
            raw_types.push((off, idx));
            p += 6;
        }
        let chars = data.get(p..p + c[5])?;
        p += c[5];
        let types: Vec<(i64, String)> = raw_types
            .iter()
            .map(|&(off, idx)| {
                let abbr: String = chars
                    .get(idx..)
                    .unwrap_or(&[])
                    .iter()
                    .take_while(|&&b| b != 0)
                    .map(|&b| b as char)
                    .collect();
                (off, abbr)
            })
            .collect();
        if types.is_empty() || transitions.iter().any(|&(_, i)| i >= types.len()) {
            return None;
        }
        p += c[2] * (time_size + 4) + c[1] + c[0];
        let rule = if version >= b'2' {
            data.get(p..).and_then(|footer| {
                let s = std::str::from_utf8(footer).ok()?;
                let s = s.trim_matches('\n');
                let s = s.split('\n').next()?;
                if s.is_empty() {
                    None
                } else {
                    parse_posix_tz(s)
                }
            })
        } else {
            None
        };
        Some(TimeZone {
            name,
            transitions,
            types,
            rule,
        })
    }

    /// El offset (segundos) y la abreviatura en el instante UTC `t` (segundos).
    fn offset_at(&self, t: i64) -> (i64, &str) {
        if let Some(&(last, _)) = self.transitions.last()
            && t >= last
            && let Some(rule) = &self.rule
        {
            return rule.offset_at(t);
        }
        if self.transitions.is_empty()
            && let Some(rule) = &self.rule
        {
            return rule.offset_at(t);
        }
        let i = self.transitions.partition_point(|&(tt, _)| tt <= t);
        let ty = if i == 0 { 0 } else { self.transitions[i - 1].1 };
        let (off, abbr) = &self.types[ty];
        (*off, abbr.as_str())
    }

    /// El offset en milisegundos en el instante UTC `t` (ms).
    fn offset_ms(&self, t: f64) -> f64 {
        if self.transitions.is_empty() && self.rule.is_none() {
            return self.types[0].0 as f64 * 1000.0;
        }
        let secs = (t / 1000.0).floor() as i64;
        self.offset_at(secs).0 as f64 * 1000.0
    }

    /// El offset (ms) que corresponde a la hora local `t`: el del primer instante posible
    /// (GetNamedTimeZoneEpochNanoseconds) o, en un hueco, el de antes de la transición.
    fn offset_for_local_ms(&self, t: f64) -> f64 {
        if self.transitions.is_empty() && self.rule.is_none() {
            return self.types[0].0 as f64 * 1000.0;
        }
        let possible = |t: f64| -> Vec<f64> {
            let mut offs = [
                self.offset_ms(t - MS_PER_DAY),
                self.offset_ms(t + MS_PER_DAY),
            ];
            offs.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
            let mut out = Vec::new();
            for o in offs {
                let u = t - o;
                if self.offset_ms(u) == o && !out.contains(&u) {
                    out.push(u);
                }
            }
            out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            out
        };
        let instants = possible(t);
        let instant = match instants.first() {
            Some(&u) => u,
            None => match possible(t - MS_PER_DAY).last() {
                Some(&u) => u,
                None => t - self.offset_ms(t),
            },
        };
        self.offset_ms(instant)
    }

    /// El nombre para TimeZoneString: el de la zona si es UTC, si no la abreviatura vigente.
    fn display_name(&self, t: f64) -> String {
        if self.transitions.is_empty() && self.rule.is_none() {
            return self.name.clone();
        }
        let secs = (t / 1000.0).floor() as i64;
        let abbr = self.offset_at(secs).1;
        if abbr.is_empty() || abbr.starts_with(['+', '-']) {
            self.name.clone()
        } else {
            abbr.to_owned()
        }
    }
}

impl PosixTz {
    fn offset_at(&self, t: i64) -> (i64, &str) {
        let Some(dst) = &self.dst else {
            return (self.std_offset, &self.std_abbr);
        };
        // El año en hora estándar local; las transiciones de ese año en UTC.
        let local = (t + self.std_offset) as f64 * 1000.0;
        let year = year_from_time(local);
        let start = rule_instant(year, dst.start.0) + dst.start.1 - self.std_offset;
        let end = rule_instant(year, dst.end.0) + dst.end.1 - dst.offset;
        let in_dst = if start < end {
            t >= start && t < end
        } else {
            !(t >= end && t < start)
        };
        if in_dst {
            (dst.offset, &dst.abbr)
        } else {
            (self.std_offset, &self.std_abbr)
        }
    }
}

/// La medianoche (segundos desde el epoch, como si fuera UTC) del día de la regla en `year`.
fn rule_instant(year: f64, r: RuleDate) -> i64 {
    let leap = days_in_year(year) == 366.0;
    let d = day_from_year(year);
    let day = match r {
        RuleDate::Julian(n) => {
            let n = f64::from(n) - 1.0;
            d + n + if leap && n >= 59.0 { 1.0 } else { 0.0 }
        }
        RuleDate::Zero(n) => d + f64::from(n),
        RuleDate::Month(m, w, wd) => {
            let m = (m.clamp(1, 12) - 1) as usize;
            let first = d + month_start(m, leap);
            let first_wd = week_day(first * MS_PER_DAY);
            let mut dd = (f64::from(wd) - first_wd).rem_euclid(7.0) + 7.0 * (f64::from(w) - 1.0);
            let len = month_start(m + 1, leap) - month_start(m, leap);
            while dd >= len {
                dd -= 7.0;
            }
            first + dd
        }
    };
    (day * 86400.0) as i64
}

/// Analiza una regla POSIX de TZ.
fn parse_posix_tz(s: &str) -> Option<PosixTz> {
    let b = s.as_bytes();
    let mut i = 0;
    let name = |i: &mut usize| -> Option<String> {
        if b.get(*i) == Some(&b'<') {
            let end = s[*i..].find('>')? + *i;
            let n = s[*i + 1..end].to_owned();
            *i = end + 1;
            return Some(n);
        }
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_alphabetic() {
            *i += 1;
        }
        (*i - start >= 3).then(|| s[start..*i].to_owned())
    };
    // [+|-]hh[:mm[:ss]] en segundos.
    let time = |i: &mut usize| -> Option<i64> {
        let mut sign = 1;
        if b.get(*i) == Some(&b'+') {
            *i += 1;
        } else if b.get(*i) == Some(&b'-') {
            sign = -1;
            *i += 1;
        }
        let mut parts = [0i64; 3];
        for (k, part) in parts.iter_mut().enumerate() {
            if k > 0 {
                if b.get(*i) != Some(&b':') {
                    break;
                }
                *i += 1;
            }
            let start = *i;
            while *i < b.len() && b[*i].is_ascii_digit() {
                *i += 1;
            }
            if *i == start {
                return None;
            }
            *part = s[start..*i].parse().ok()?;
        }
        Some(sign * (parts[0] * 3600 + parts[1] * 60 + parts[2]))
    };
    let std_abbr = name(&mut i)?;
    // POSIX: el offset es al oeste de UTC.
    let std_offset = -time(&mut i)?;
    if i >= b.len() {
        return Some(PosixTz {
            std_abbr,
            std_offset,
            dst: None,
        });
    }
    let dst_abbr = name(&mut i)?;
    let dst_offset = if i < b.len() && b[i] != b',' {
        -time(&mut i)?
    } else {
        std_offset + 3600
    };
    let rule = |i: &mut usize| -> Option<(RuleDate, i64)> {
        let num = |i: &mut usize| -> Option<u32> {
            let start = *i;
            while *i < b.len() && b[*i].is_ascii_digit() {
                *i += 1;
            }
            s.get(start..*i)?.parse().ok()
        };
        let d = match b.get(*i)? {
            b'J' => {
                *i += 1;
                RuleDate::Julian(num(i)?)
            }
            b'M' => {
                *i += 1;
                let m = num(i)?;
                if b.get(*i) != Some(&b'.') {
                    return None;
                }
                *i += 1;
                let w = num(i)?;
                if b.get(*i) != Some(&b'.') {
                    return None;
                }
                *i += 1;
                RuleDate::Month(m, w, num(i)?)
            }
            _ => RuleDate::Zero(num(i)?),
        };
        let t = if b.get(*i) == Some(&b'/') {
            *i += 1;
            time(i)?
        } else {
            7200
        };
        Some((d, t))
    };
    let (start, end) = if b.get(i) == Some(&b',') {
        i += 1;
        let start = rule(&mut i)?;
        if b.get(i) != Some(&b',') {
            return None;
        }
        i += 1;
        (start, rule(&mut i)?)
    } else {
        (
            (RuleDate::Month(3, 2, 0), 7200),
            (RuleDate::Month(11, 1, 0), 7200),
        )
    };
    Some(PosixTz {
        std_abbr,
        std_offset,
        dst: Some(PosixDst {
            abbr: dst_abbr,
            offset: dst_offset,
            start,
            end,
        }),
    })
}

// --- Formatos de texto ---

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn year_string(y: f64) -> String {
    let sign = if y >= 0.0 { "" } else { "-" };
    format!("{sign}{:04}", y.abs() as i64)
}

/// DateString ( tv ) (§21.4.4.41.2).
fn date_string(tv: f64) -> String {
    format!(
        "{} {} {:02} {}",
        WEEKDAYS[week_day(tv) as usize],
        MONTHS[month_from_time(tv) as usize],
        date_from_time(tv) as i64,
        year_string(year_from_time(tv))
    )
}

/// TimeString ( tv ) (§21.4.4.41.1).
fn time_string(tv: f64) -> String {
    format!(
        "{:02}:{:02}:{:02} GMT",
        hour_from_time(tv) as i64,
        min_from_time(tv) as i64,
        sec_from_time(tv) as i64
    )
}

/// TimeZoneString ( tv ) (§21.4.4.41.3).
fn time_zone_string(tv: f64) -> String {
    let tz = TimeZone::system();
    let offset = tz.offset_ms(tv);
    let sign = if offset >= 0.0 { '+' } else { '-' };
    let abs = (offset.abs() / MS_PER_MINUTE).round() as i64;
    format!(
        "{sign}{:02}{:02} ({})",
        abs / 60,
        abs % 60,
        tz.display_name(tv)
    )
}

/// ToDateString ( tv ) (§21.4.4.41.4).
fn to_date_string_of(tv: f64) -> String {
    if tv.is_nan() {
        return "Invalid Date".into();
    }
    let t = local_time(tv);
    format!(
        "{} {}{}",
        date_string(t),
        time_string(t),
        time_zone_string(tv)
    )
}

fn iso_string(tv: f64) -> String {
    let y = year_from_time(tv);
    let year = if (0.0..=9999.0).contains(&y) {
        format!("{:04}", y as i64)
    } else {
        let sign = if y < 0.0 { '-' } else { '+' };
        format!("{sign}{:06}", y.abs() as i64)
    };
    format!(
        "{year}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        month_from_time(tv) as i64 + 1,
        date_from_time(tv) as i64,
        hour_from_time(tv) as i64,
        min_from_time(tv) as i64,
        sec_from_time(tv) as i64,
        ms_from_time(tv) as i64
    )
}

// --- Date.parse ---

/// El formato Date Time String de §21.4.1.32: `None` si no lo es.
fn parse_iso(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let mut i = 0;
    let digits = |i: &mut usize, n: usize| -> Option<f64> {
        let part = b.get(*i..*i + n)?;
        if !part.iter().all(u8::is_ascii_digit) {
            return None;
        }
        *i += n;
        Some(
            part.iter()
                .fold(0.0, |a, &d| a * 10.0 + f64::from(d - b'0')),
        )
    };
    let year = match b.first()? {
        b'+' | b'-' => {
            let neg = b[0] == b'-';
            i = 1;
            let y = digits(&mut i, 6)?;
            if neg && y == 0.0 {
                return None;
            }
            if neg { -y } else { y }
        }
        _ => digits(&mut i, 4)?,
    };
    let mut month = 1.0;
    let mut dayv = 1.0;
    if b.get(i) == Some(&b'-') {
        i += 1;
        month = digits(&mut i, 2)?;
        if b.get(i) == Some(&b'-') {
            i += 1;
            dayv = digits(&mut i, 2)?;
        }
    }
    if !(1.0..=12.0).contains(&month) {
        return None;
    }
    let leap = days_in_year(year) == 366.0;
    let mlen = month_start(month as usize, leap) - month_start(month as usize - 1, leap);
    if dayv < 1.0 || dayv > mlen {
        return None;
    }
    let (mut h, mut m, mut sec, mut ms) = (0.0, 0.0, 0.0, 0.0);
    let mut offset: Option<f64> = None;
    let mut has_time = false;
    if b.get(i) == Some(&b'T') || b.get(i) == Some(&b't') {
        has_time = true;
        i += 1;
        h = digits(&mut i, 2)?;
        if b.get(i) != Some(&b':') {
            return None;
        }
        i += 1;
        m = digits(&mut i, 2)?;
        if b.get(i) == Some(&b':') {
            i += 1;
            sec = digits(&mut i, 2)?;
            if b.get(i) == Some(&b'.') {
                i += 1;
                let start = i;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                if i == start {
                    return None;
                }
                let frac = &s[start..i];
                let first3: String = frac.chars().chain("000".chars()).take(3).collect();
                ms = first3.parse::<f64>().ok()?;
            }
        }
        if h > 24.0 || m > 59.0 || sec > 59.0 || (h == 24.0 && (m > 0.0 || sec > 0.0 || ms > 0.0)) {
            return None;
        }
        match b.get(i) {
            Some(b'Z') | Some(b'z') => {
                offset = Some(0.0);
                i += 1;
            }
            Some(&c @ (b'+' | b'-')) => {
                i += 1;
                let oh = digits(&mut i, 2)?;
                if b.get(i) != Some(&b':') {
                    return None;
                }
                i += 1;
                let om = digits(&mut i, 2)?;
                if oh > 23.0 || om > 59.0 {
                    return None;
                }
                let o = (oh * 60.0 + om) * MS_PER_MINUTE;
                offset = Some(if c == b'-' { -o } else { o });
            }
            _ => {}
        }
    }
    if i != b.len() {
        return None;
    }
    let date = make_date(make_day(year, month - 1.0, dayv), make_time(h, m, sec, ms));
    // Sin offset: las formas de solo fecha son UTC y las de fecha y hora, locales.
    let tv = match offset {
        Some(o) => date - o,
        None if has_time => utc_of(date),
        None => date,
    };
    Some(time_clip(tv))
}

/// Los formatos de `toString`, `toUTCString` y variantes habituales ("Tue Feb 01 2022
/// 10:00:00 GMT+0100 (CET)", "Tue, 01 Feb 2022 10:00:00 GMT", "Feb 1, 2022").
fn parse_legacy(s: &str) -> Option<f64> {
    #[derive(Debug, Clone, PartialEq)]
    enum Tok {
        Num(f64, usize),
        Word(String),
        Sign(i8),
        Colon,
        Dot,
    }
    let mut toks = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut depth = 0;
    while i < chars.len() {
        let c = chars[i];
        if depth > 0 {
            if c == '(' {
                depth += 1;
            } else if c == ')' {
                depth -= 1;
            }
            i += 1;
            continue;
        }
        match c {
            '(' => {
                depth = 1;
                i += 1;
            }
            '0'..='9' => {
                let start = i;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                let t: String = chars[start..i].iter().collect();
                toks.push(Tok::Num(t.parse().ok()?, i - start));
            }
            'a'..='z' | 'A'..='Z' => {
                let start = i;
                while i < chars.len() && chars[i].is_ascii_alphabetic() {
                    i += 1;
                }
                toks.push(Tok::Word(
                    chars[start..i]
                        .iter()
                        .collect::<String>()
                        .to_ascii_lowercase(),
                ));
            }
            '+' => {
                toks.push(Tok::Sign(1));
                i += 1;
            }
            '-' => {
                toks.push(Tok::Sign(-1));
                i += 1;
            }
            ':' => {
                toks.push(Tok::Colon);
                i += 1;
            }
            '.' => {
                toks.push(Tok::Dot);
                i += 1;
            }
            ' ' | ',' | '/' | '\t' => i += 1,
            _ => return None,
        }
    }
    let mut year: Option<f64> = None;
    let mut month: Option<f64> = None;
    let mut dayv: Option<f64> = None;
    let mut time: Option<(f64, f64, f64, f64)> = None;
    let mut offset: Option<f64> = None;
    let mut pm: Option<bool> = None;
    let mut numbers: Vec<f64> = Vec::new();
    let mut k = 0;
    while k < toks.len() {
        match &toks[k] {
            Tok::Word(w) => {
                if let Some(m) = MONTHS
                    .iter()
                    .position(|m| w.starts_with(&m.to_ascii_lowercase()))
                    && w.len() >= 3
                {
                    month = Some(m as f64);
                } else if WEEKDAYS
                    .iter()
                    .any(|d| w.len() >= 3 && w.starts_with(&d.to_ascii_lowercase()))
                {
                } else if w == "gmt" || w == "utc" || w == "ut" || w == "z" {
                    offset = Some(0.0);
                } else if w == "am" || w == "pm" {
                    pm = Some(w == "pm");
                } else if w == "t" {
                } else {
                    return None;
                }
                k += 1;
            }
            Tok::Num(n, len) => {
                if matches!(toks.get(k + 1), Some(Tok::Colon)) && time.is_none() {
                    // hh:mm[:ss[.sss]]
                    let h = *n;
                    let Some(Tok::Num(m, _)) = toks.get(k + 2) else {
                        return None;
                    };
                    let mut sec = 0.0;
                    let mut ms = 0.0;
                    k += 3;
                    if matches!(toks.get(k), Some(Tok::Colon)) {
                        let Some(Tok::Num(sv, _)) = toks.get(k + 1) else {
                            return None;
                        };
                        sec = *sv;
                        k += 2;
                        if matches!(toks.get(k), Some(Tok::Dot))
                            && let Some(Tok::Num(f, flen)) = toks.get(k + 1)
                        {
                            ms = (f / 10f64.powi(*flen as i32 - 3)).floor();
                            k += 2;
                        }
                    }
                    if h > 24.0 || *m > 59.0 || sec > 59.0 {
                        return None;
                    }
                    time = Some((h, *m, sec, ms));
                    continue;
                }
                if *len >= 3 || *n > 31.0 {
                    if year.is_some() {
                        return None;
                    }
                    year = Some(*n);
                } else {
                    numbers.push(*n);
                }
                k += 1;
            }
            Tok::Sign(sg) => {
                // Un offset después de GMT/UTC o de la hora (+hhmm, +hh:mm), o un año con
                // signo.
                let Some(Tok::Num(n, len)) = toks.get(k + 1) else {
                    return None;
                };
                if offset.is_some() || time.is_some() && year.is_some() {
                    let (oh, om, used) = if matches!(toks.get(k + 2), Some(Tok::Colon)) {
                        let Some(Tok::Num(m, _)) = toks.get(k + 3) else {
                            return None;
                        };
                        (*n, *m, 4)
                    } else if *len == 4 {
                        ((n / 100.0).floor(), n % 100.0, 2)
                    } else if *len <= 2 {
                        (*n, 0.0, 2)
                    } else {
                        return None;
                    };
                    let o = (oh * 60.0 + om) * MS_PER_MINUTE;
                    offset = Some(f64::from(*sg) * o);
                    k += used;
                } else {
                    if year.is_some() {
                        return None;
                    }
                    year = Some(f64::from(*sg) * n);
                    k += 2;
                }
            }
            Tok::Colon | Tok::Dot => return None,
        }
    }
    // Los números sueltos: día y, si falta, año (y mes si no hubo nombre).
    let mut nums = numbers.into_iter();
    if month.is_none() {
        // Formato numérico m/d/y.
        month = nums.next().map(|m| m - 1.0);
    }
    if dayv.is_none() {
        dayv = nums.next();
    }
    if year.is_none() {
        year = nums.next().map(|y| {
            if y < 50.0 {
                2000.0 + y
            } else if y < 100.0 {
                1900.0 + y
            } else {
                y
            }
        });
    }
    if nums.next().is_some() {
        return None;
    }
    let (year, month) = (year?, month?);
    let dayv = dayv.unwrap_or(1.0);
    if !(0.0..=11.0).contains(&month) || !(1.0..=31.0).contains(&dayv) {
        return None;
    }
    let (mut h, m, sec, ms) = time.unwrap_or((0.0, 0.0, 0.0, 0.0));
    if let Some(pm) = pm {
        if h > 12.0 {
            return None;
        }
        if h == 12.0 {
            h = 0.0;
        }
        if pm {
            h += 12.0;
        }
    }
    let date = make_date(make_day(year, month, dayv), make_time(h, m, sec, ms));
    let tv = match offset {
        Some(o) => date - o,
        None => utc_of(date),
    };
    Some(time_clip(tv))
}

/// El parser de Date.parse (§21.4.3.2): el formato del spec y, si no lo es, los formatos
/// de compatibilidad.
pub fn parse_date(s: &str) -> f64 {
    let s = s.trim();
    parse_iso(s).or_else(|| parse_legacy(s)).unwrap_or(f64::NAN)
}

// --- El constructor y las funciones estáticas ---

fn this_time_value(agent: &mut Agent, this: &Value, method: &str) -> JsResult<f64> {
    if let Value::Object(o) = this
        && let ObjectKind::Date(t) = agent.heap[*o].kind
    {
        return Ok(t);
    }
    agent.type_error(&format!(
        "Date.prototype.{method} requires that 'this' be a Date object"
    ))
}

fn set_this_time(agent: &mut Agent, this: &Value, t: f64) {
    if let Value::Object(o) = this
        && let ObjectKind::Date(v) = &mut agent.heap[*o].kind
    {
        *v = t;
    }
}

/// Date ( ...values ) (§21.4.2.1).
fn date_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return Ok(Value::str(&to_date_string_of(now_ms())));
    };
    let dv = match a.len() {
        0 => now_ms(),
        1 => {
            let value = a.arg(0);
            let tv = if let Value::Object(o) = value
                && let ObjectKind::Date(t) = agent.heap[o].kind
            {
                t
            } else {
                let v = agent.to_primitive(&value, Hint::Default)?;
                if let Value::String(s) = &v {
                    parse_date(&s.to_string_lossy())
                } else {
                    agent.to_number(&v)?
                }
            };
            time_clip(tv)
        }
        n => {
            let mut nums = [0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
            for (k, slot) in nums.iter_mut().enumerate().take(n.min(7)) {
                *slot = agent.to_number(&a.arg(k))?;
            }
            let yr = make_full_year(nums[0]);
            let final_date = make_date(
                make_day(yr, nums[1], nums[2]),
                make_time(nums[3], nums[4], nums[5], nums[6]),
            );
            time_clip(utc_of(final_date))
        }
    };
    let o =
        agent.ordinary_create_from_constructor(nt, |i| i.date_prototype, ObjectKind::Date(dv))?;
    Ok(Value::Object(o))
}

/// Date.now ( ) (§21.4.3.1).
fn now(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(now_ms()))
}

/// Date.parse ( string ) (§21.4.3.2).
fn parse(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = agent.to_string(&a.arg(0))?;
    Ok(Value::Number(parse_date(&s.to_string_lossy())))
}

/// Date.UTC ( year [ , month [ , date [ , hours [ , minutes [ , seconds [ , ms ] ] ] ] ] ] )
/// (§21.4.3.4).
fn utc(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let y = agent.to_number(&a.arg(0))?;
    let mut nums = [0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
    for (k, slot) in nums.iter_mut().enumerate() {
        if k + 1 < a.len() {
            *slot = agent.to_number(&a.arg(k + 1))?;
        }
    }
    let yr = make_full_year(y);
    Ok(Value::Number(time_clip(make_date(
        make_day(yr, nums[0], nums[1]),
        make_time(nums[2], nums[3], nums[4], nums[5]),
    ))))
}

// --- Getters ---

macro_rules! getter {
    ($name:ident, $js:literal, local, $f:expr) => {
        fn $name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
            let t = this_time_value(agent, &a.this, $js)?;
            if t.is_nan() {
                return Ok(Value::Number(f64::NAN));
            }
            let f: fn(f64) -> f64 = $f;
            Ok(Value::Number(f(local_time(t))))
        }
    };
    ($name:ident, $js:literal, utc, $f:expr) => {
        fn $name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
            let t = this_time_value(agent, &a.this, $js)?;
            if t.is_nan() {
                return Ok(Value::Number(f64::NAN));
            }
            let f: fn(f64) -> f64 = $f;
            Ok(Value::Number(f(t)))
        }
    };
}

getter!(get_date, "getDate", local, date_from_time);
getter!(get_day, "getDay", local, week_day);
getter!(get_full_year, "getFullYear", local, year_from_time);
getter!(get_hours, "getHours", local, hour_from_time);
getter!(get_milliseconds, "getMilliseconds", local, ms_from_time);
getter!(get_minutes, "getMinutes", local, min_from_time);
getter!(get_month, "getMonth", local, month_from_time);
getter!(get_seconds, "getSeconds", local, sec_from_time);
getter!(get_year, "getYear", local, |t| year_from_time(t) - 1900.0);
getter!(get_utc_date, "getUTCDate", utc, date_from_time);
getter!(get_utc_day, "getUTCDay", utc, week_day);
getter!(get_utc_full_year, "getUTCFullYear", utc, year_from_time);
getter!(get_utc_hours, "getUTCHours", utc, hour_from_time);
getter!(
    get_utc_milliseconds,
    "getUTCMilliseconds",
    utc,
    ms_from_time
);
getter!(get_utc_minutes, "getUTCMinutes", utc, min_from_time);
getter!(get_utc_month, "getUTCMonth", utc, month_from_time);
getter!(get_utc_seconds, "getUTCSeconds", utc, sec_from_time);

/// Date.prototype.getTime ( ) (§21.4.4.10).
fn get_time(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(this_time_value(agent, &a.this, "getTime")?))
}

/// Date.prototype.valueOf ( ) (§21.4.4.44).
fn value_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(this_time_value(agent, &a.this, "valueOf")?))
}

/// Date.prototype.getTimezoneOffset ( ) (§21.4.4.11).
fn get_timezone_offset(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = this_time_value(agent, &a.this, "getTimezoneOffset")?;
    if t.is_nan() {
        return Ok(Value::Number(f64::NAN));
    }
    Ok(Value::Number((t - local_time(t)) / MS_PER_MINUTE))
}

// --- Setters ---

/// Los argumentos opcionales convertidos con ToNumber (en orden, antes de mirar el valor).
fn numbers(agent: &mut Agent, a: &CallArgs<'_>, n: usize) -> JsResult<Vec<Option<f64>>> {
    let mut out = Vec::with_capacity(n);
    for k in 0..n {
        out.push(if k < a.len() {
            Some(agent.to_number(&a.arg(k))?)
        } else {
            None
        });
    }
    Ok(out)
}

/// Guarda TimeClip(v) (pasado por UTC si `local`) y lo devuelve.
fn store(agent: &mut Agent, this: &Value, date: f64, local: bool) -> JsResult<Value> {
    let u = time_clip(if local { utc_of(date) } else { date });
    set_this_time(agent, this, u);
    Ok(Value::Number(u))
}

/// Date.prototype.setTime ( time ) (§21.4.4.27).
fn set_time(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_time_value(agent, &a.this, "setTime")?;
    let t = agent.to_number(&a.arg(0))?;
    let v = time_clip(t);
    set_this_time(agent, &a.this, v);
    Ok(Value::Number(v))
}

/// setMilliseconds / setUTCMilliseconds (§21.4.4.23, §21.4.4.31).
fn set_ms_impl(agent: &mut Agent, a: &CallArgs<'_>, local: bool, name: &str) -> JsResult<Value> {
    let t = this_time_value(agent, &a.this, name)?;
    let ms = agent.to_number(&a.arg(0))?;
    if t.is_nan() {
        return Ok(Value::Number(f64::NAN));
    }
    let t = if local { local_time(t) } else { t };
    let time = make_time(hour_from_time(t), min_from_time(t), sec_from_time(t), ms);
    store(agent, &a.this, make_date(day(t), time), local)
}

/// setSeconds / setUTCSeconds (§21.4.4.26, §21.4.4.34).
fn set_seconds_impl(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    local: bool,
    name: &str,
) -> JsResult<Value> {
    let t = this_time_value(agent, &a.this, name)?;
    let n = numbers(agent, a, 2)?;
    let s = n[0].unwrap_or(f64::NAN);
    if t.is_nan() {
        return Ok(Value::Number(f64::NAN));
    }
    let t = if local { local_time(t) } else { t };
    let milli = n[1].unwrap_or_else(|| ms_from_time(t));
    let date = make_date(
        day(t),
        make_time(hour_from_time(t), min_from_time(t), s, milli),
    );
    store(agent, &a.this, date, local)
}

/// setMinutes / setUTCMinutes (§21.4.4.24, §21.4.4.32).
fn set_minutes_impl(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    local: bool,
    name: &str,
) -> JsResult<Value> {
    let t = this_time_value(agent, &a.this, name)?;
    let n = numbers(agent, a, 3)?;
    let m = n[0].unwrap_or(f64::NAN);
    if t.is_nan() {
        return Ok(Value::Number(f64::NAN));
    }
    let t = if local { local_time(t) } else { t };
    let s = n[1].unwrap_or_else(|| sec_from_time(t));
    let milli = n[2].unwrap_or_else(|| ms_from_time(t));
    let date = make_date(day(t), make_time(hour_from_time(t), m, s, milli));
    store(agent, &a.this, date, local)
}

/// setHours / setUTCHours (§21.4.4.22, §21.4.4.30).
fn set_hours_impl(agent: &mut Agent, a: &CallArgs<'_>, local: bool, name: &str) -> JsResult<Value> {
    let t = this_time_value(agent, &a.this, name)?;
    let n = numbers(agent, a, 4)?;
    let h = n[0].unwrap_or(f64::NAN);
    if t.is_nan() {
        return Ok(Value::Number(f64::NAN));
    }
    let t = if local { local_time(t) } else { t };
    let m = n[1].unwrap_or_else(|| min_from_time(t));
    let s = n[2].unwrap_or_else(|| sec_from_time(t));
    let milli = n[3].unwrap_or_else(|| ms_from_time(t));
    let date = make_date(day(t), make_time(h, m, s, milli));
    store(agent, &a.this, date, local)
}

/// setDate / setUTCDate (§21.4.4.20, §21.4.4.28).
fn set_date_impl(agent: &mut Agent, a: &CallArgs<'_>, local: bool, name: &str) -> JsResult<Value> {
    let t = this_time_value(agent, &a.this, name)?;
    let dt = agent.to_number(&a.arg(0))?;
    if t.is_nan() {
        return Ok(Value::Number(f64::NAN));
    }
    let t = if local { local_time(t) } else { t };
    let new_date = make_date(
        make_day(year_from_time(t), month_from_time(t), dt),
        time_within_day(t),
    );
    store(agent, &a.this, new_date, local)
}

/// setMonth / setUTCMonth (§21.4.4.25, §21.4.4.33).
fn set_month_impl(agent: &mut Agent, a: &CallArgs<'_>, local: bool, name: &str) -> JsResult<Value> {
    let t = this_time_value(agent, &a.this, name)?;
    let n = numbers(agent, a, 2)?;
    let m = n[0].unwrap_or(f64::NAN);
    if t.is_nan() {
        return Ok(Value::Number(f64::NAN));
    }
    let t = if local { local_time(t) } else { t };
    let dt = n[1].unwrap_or_else(|| date_from_time(t));
    let new_date = make_date(make_day(year_from_time(t), m, dt), time_within_day(t));
    store(agent, &a.this, new_date, local)
}

/// setFullYear / setUTCFullYear (§21.4.4.21, §21.4.4.29).
fn set_full_year_impl(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    local: bool,
    name: &str,
) -> JsResult<Value> {
    let t = this_time_value(agent, &a.this, name)?;
    let y = agent.to_number(&a.arg(0))?;
    let t = if t.is_nan() {
        0.0
    } else if local {
        local_time(t)
    } else {
        t
    };
    let m = if a.len() > 1 {
        agent.to_number(&a.arg(1))?
    } else {
        month_from_time(t)
    };
    let dt = if a.len() > 2 {
        agent.to_number(&a.arg(2))?
    } else {
        date_from_time(t)
    };
    let new_date = make_date(make_day(y, m, dt), time_within_day(t));
    store(agent, &a.this, new_date, local)
}

/// Date.prototype.setYear ( year ) (B.2.3.2).
fn set_year(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = this_time_value(agent, &a.this, "setYear")?;
    let y = agent.to_number(&a.arg(0))?;
    let t = if t.is_nan() { 0.0 } else { local_time(t) };
    let yyyy = make_full_year(y);
    let d = make_day(yyyy, month_from_time(t), date_from_time(t));
    let date = make_date(d, time_within_day(t));
    store(agent, &a.this, date, true)
}

macro_rules! setter {
    ($name:ident, $imp:ident, $local:expr, $js:literal) => {
        fn $name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
            $imp(agent, a, $local, $js)
        }
    };
}

setter!(set_milliseconds, set_ms_impl, true, "setMilliseconds");
setter!(
    set_utc_milliseconds,
    set_ms_impl,
    false,
    "setUTCMilliseconds"
);
setter!(set_seconds, set_seconds_impl, true, "setSeconds");
setter!(set_utc_seconds, set_seconds_impl, false, "setUTCSeconds");
setter!(set_minutes, set_minutes_impl, true, "setMinutes");
setter!(set_utc_minutes, set_minutes_impl, false, "setUTCMinutes");
setter!(set_hours, set_hours_impl, true, "setHours");
setter!(set_utc_hours, set_hours_impl, false, "setUTCHours");
setter!(set_date, set_date_impl, true, "setDate");
setter!(set_utc_date, set_date_impl, false, "setUTCDate");
setter!(set_month, set_month_impl, true, "setMonth");
setter!(set_utc_month, set_month_impl, false, "setUTCMonth");
setter!(set_full_year, set_full_year_impl, true, "setFullYear");
setter!(
    set_utc_full_year,
    set_full_year_impl,
    false,
    "setUTCFullYear"
);

// --- Conversiones a string ---

/// Date.prototype.toString ( ) (§21.4.4.41).
fn to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let tv = this_time_value(agent, &a.this, "toString")?;
    Ok(Value::str(&to_date_string_of(tv)))
}

/// Date.prototype.toDateString ( ) (§21.4.4.35).
fn to_date_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let tv = this_time_value(agent, &a.this, "toDateString")?;
    if tv.is_nan() {
        return Ok(Value::str("Invalid Date"));
    }
    Ok(Value::str(&date_string(local_time(tv))))
}

/// Date.prototype.toTimeString ( ) (§21.4.4.42).
fn to_time_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let tv = this_time_value(agent, &a.this, "toTimeString")?;
    if tv.is_nan() {
        return Ok(Value::str("Invalid Date"));
    }
    let t = local_time(tv);
    Ok(Value::str(&format!(
        "{}{}",
        time_string(t),
        time_zone_string(tv)
    )))
}

/// Date.prototype.toUTCString ( ) (§21.4.4.43).
fn to_utc_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let tv = this_time_value(agent, &a.this, "toUTCString")?;
    if tv.is_nan() {
        return Ok(Value::str("Invalid Date"));
    }
    Ok(Value::str(&format!(
        "{}, {:02} {} {} {}",
        WEEKDAYS[week_day(tv) as usize],
        date_from_time(tv) as i64,
        MONTHS[month_from_time(tv) as usize],
        year_string(year_from_time(tv)),
        time_string(tv)
    )))
}

/// Date.prototype.toISOString ( ) (§21.4.4.36).
fn to_iso_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let tv = this_time_value(agent, &a.this, "toISOString")?;
    if !tv.is_finite() {
        return agent.range_error("Invalid time value");
    }
    Ok(Value::str(&iso_string(tv)))
}

/// Date.prototype.toJSON ( key ) (§21.4.4.37).
fn to_json(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = agent.to_object(&a.this)?;
    let tv = agent.to_primitive(&Value::Object(o), Hint::Number)?;
    if let Value::Number(n) = tv
        && !n.is_finite()
    {
        return Ok(Value::Null);
    }
    agent.invoke(&Value::Object(o), &PropertyKey::from("toISOString"), &[])
}

/// toLocaleString y compañía sin ECMA-402: los mismos formatos que toString.
fn to_locale_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let tv = this_time_value(agent, &a.this, "toLocaleString")?;
    Ok(Value::str(&to_date_string_of(tv)))
}

fn to_locale_date_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let tv = this_time_value(agent, &a.this, "toLocaleDateString")?;
    if tv.is_nan() {
        return Ok(Value::str("Invalid Date"));
    }
    Ok(Value::str(&date_string(local_time(tv))))
}

fn to_locale_time_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let tv = this_time_value(agent, &a.this, "toLocaleTimeString")?;
    if tv.is_nan() {
        return Ok(Value::str("Invalid Date"));
    }
    let t = local_time(tv);
    Ok(Value::str(&format!(
        "{}{}",
        time_string(t),
        time_zone_string(tv)
    )))
}

/// Date.prototype [ @@toPrimitive ] ( hint ) (§21.4.4.45).
fn to_primitive(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Value::Object(o) = a.this else {
        return agent.type_error("Date.prototype[Symbol.toPrimitive] called on non-object");
    };
    let hint = match a.arg(0) {
        Value::String(s) if s == JsString::from("string") || s == JsString::from("default") => {
            Hint::String
        }
        Value::String(s) if s == JsString::from("number") => Hint::Number,
        _ => return agent.type_error("Invalid hint"),
    };
    agent.ordinary_to_primitive(o, hint)
}

/// Un objeto Date con el valor `t` (para otros built-ins).
pub fn create_date(agent: &mut Agent, t: f64) -> Gc<Object> {
    let proto = agent.intrinsics().date_prototype;
    agent.alloc(Object::new(Some(proto), ObjectKind::Date(time_clip(t))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar() {
        let t = make_date(
            make_day(2024.0, 1.0, 29.0),
            make_time(13.0, 5.0, 7.0, 123.0),
        );
        assert_eq!(year_from_time(t), 2024.0);
        assert_eq!(month_from_time(t), 1.0);
        assert_eq!(date_from_time(t), 29.0);
        assert_eq!(week_day(t), 4.0);
        assert_eq!(hour_from_time(t), 13.0);
        assert_eq!(ms_from_time(t), 123.0);
        assert_eq!(iso_string(t), "2024-02-29T13:05:07.123Z");
        let neg = make_date(make_day(-1.0, 11.0, 31.0), 0.0);
        assert_eq!(year_from_time(neg), -1.0);
        assert_eq!(iso_string(neg), "-000001-12-31T00:00:00.000Z");
        assert!(time_clip(8.64e15 + 1.0).is_nan());
        assert_eq!(year_from_time(-8.64e15), -271821.0);
        assert_eq!(year_from_time(8.64e15), 275760.0);
    }

    #[test]
    fn iso_parsing() {
        assert_eq!(parse_iso("1970-01-01T00:00:00.000Z"), Some(0.0));
        assert_eq!(parse_iso("1970"), Some(0.0));
        assert_eq!(parse_iso("1970-01-01T00:00Z"), Some(0.0));
        assert_eq!(parse_iso("1970-01-01T01:00+01:00"), Some(0.0));
        assert_eq!(parse_iso("1970-01-02T24:00:00Z"), Some(2.0 * MS_PER_DAY));
        assert_eq!(parse_iso("-000000-01-01T00:00:00Z"), None);
        assert_eq!(parse_iso("1970-13-01"), None);
        assert_eq!(parse_iso("1970-01-01T24:00:01Z"), None);
        assert_eq!(parse_iso("+275760-09-13T00:00:00.000Z"), Some(8.64e15));
        assert!(parse_iso("+275760-09-13T00:00:00.001Z").unwrap().is_nan());
    }

    #[test]
    fn legacy_parsing() {
        assert_eq!(parse_legacy("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0.0));
        assert_eq!(
            parse_legacy("Thu Jan 01 1970 00:00:00 GMT+0000 (Coordinated Universal Time)"),
            Some(0.0)
        );
        assert_eq!(
            parse_legacy("Thu Jan 01 1970 01:00:00 GMT+0100 (CET)"),
            Some(0.0)
        );
        assert_eq!(
            parse_legacy("Sat, 01 Jan -0001 00:00:00 GMT").map(year_from_time),
            Some(-1.0)
        );
        assert_eq!(parse_legacy("foo"), None);
    }

    #[test]
    fn posix_rules() {
        let z = TimeZone::from_posix("EST5EDT,M3.2.0,M11.1.0").unwrap();
        // 2024-07-01T12:00Z: EDT (−4 h).
        let summer = make_date(make_day(2024.0, 6.0, 1.0), make_time(12.0, 0.0, 0.0, 0.0));
        assert_eq!(z.offset_ms(summer), -4.0 * MS_PER_HOUR);
        let winter = make_date(make_day(2024.0, 0.0, 15.0), 0.0);
        assert_eq!(z.offset_ms(winter), -5.0 * MS_PER_HOUR);
        // 2024-03-10T02:30 local no existe: se usa el offset de antes (EST).
        let gap = make_date(make_day(2024.0, 2.0, 10.0), make_time(2.0, 30.0, 0.0, 0.0));
        assert_eq!(z.offset_for_local_ms(gap), -5.0 * MS_PER_HOUR);
        // 2024-11-03T01:30 local ocurre dos veces: el primer instante (EDT).
        let overlap = make_date(make_day(2024.0, 10.0, 3.0), make_time(1.0, 30.0, 0.0, 0.0));
        assert_eq!(z.offset_for_local_ms(overlap), -4.0 * MS_PER_HOUR);
    }
}
