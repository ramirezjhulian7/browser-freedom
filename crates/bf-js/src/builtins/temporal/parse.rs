//! La gramática de los strings de Temporal (ISO 8601 / RFC 9557: TemporalDateTimeString,
//! TemporalInstantString, TemporalTimeString, TemporalYearMonthString,
//! TemporalMonthDayString, TimeZoneIdentifier, TemporalDurationString) y ParseISODateTime.
//! Un parser a mano con retroceso por posiciones; los errores son `None` (RangeError para
//! quien llama).

use super::iso::{Time, is_valid_date};

/// Los objetivos de ParseISODateTime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Goal {
    DateTime { zoned: bool },
    Instant,
    Time,
    YearMonth,
    MonthDay,
}

/// ISO Date-Time Parse Record.
#[derive(Debug, Clone, Default)]
pub struct Parsed {
    /// None: un MonthDay sin año.
    pub year: Option<i64>,
    pub month: i64,
    /// None: un YearMonth sin día.
    pub day: Option<i64>,
    /// None: start-of-day (sin hora).
    pub time: Option<Time>,
    pub z: bool,
    /// El UTCOffset de DateTimeUTCOffset y si tiene precisión de segundos.
    pub offset: Option<String>,
    pub offset_sub_minute: bool,
    pub tz_annotation: Option<String>,
    pub calendar: Option<String>,
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> P<'a> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn eat_any(&mut self, cs: &[u8]) -> Option<u8> {
        let c = self.peek()?;
        if cs.contains(&c) {
            self.i += 1;
            Some(c)
        } else {
            None
        }
    }

    fn digits(&mut self, n: usize) -> Option<i64> {
        if self.i + n > self.s.len() {
            return None;
        }
        let mut v = 0i64;
        for k in 0..n {
            let c = self.s[self.i + k];
            if !c.is_ascii_digit() {
                return None;
            }
            v = v * 10 + i64::from(c - b'0');
        }
        self.i += n;
        Some(v)
    }

    fn at_end(&self) -> bool {
        self.i == self.s.len()
    }
}

/// Un Date-Time sin anotaciones.
#[derive(Debug, Clone, Default)]
struct Core {
    year: Option<i64>,
    month: i64,
    day: Option<i64>,
    time: Option<Time>,
    z: bool,
    offset: Option<String>,
    offset_sub_minute: bool,
}

/// DateYear.
fn date_year(p: &mut P) -> Option<i64> {
    let start = p.i;
    if let Some(sign) = p.eat_any(b"+-") {
        let Some(v) = p.digits(6) else {
            p.i = start;
            return None;
        };
        if sign == b'-' && v == 0 {
            p.i = start;
            return None;
        }
        return Some(if sign == b'-' { -v } else { v });
    }
    p.digits(4)
}

fn date_month(p: &mut P) -> Option<i64> {
    let start = p.i;
    let m = p.digits(2)?;
    if (1..=12).contains(&m) {
        Some(m)
    } else {
        p.i = start;
        None
    }
}

fn date_day(p: &mut P) -> Option<i64> {
    let start = p.i;
    let d = p.digits(2)?;
    if (1..=31).contains(&d) {
        Some(d)
    } else {
        p.i = start;
        None
    }
}

/// Date: año, mes y día, extendido o básico (y válido).
fn date(p: &mut P) -> Option<(i64, i64, i64)> {
    let start = p.i;
    let y = date_year(p)?;
    let r = (|| {
        if p.eat(b'-') {
            let m = date_month(p)?;
            if !p.eat(b'-') {
                return None;
            }
            let d = date_day(p)?;
            Some((m, d))
        } else {
            let m = date_month(p)?;
            let d = date_day(p)?;
            Some((m, d))
        }
    })();
    match r {
        Some((m, d)) if is_valid_date(y, m, d) => Some((y, m, d)),
        _ => {
            p.i = start;
            None
        }
    }
}

/// TemporalDecimalFraction: (dígitos, cantidad).
fn fraction(p: &mut P) -> Option<(u32, usize)> {
    let start = p.i;
    p.eat_any(b".,")?;
    let mut v = 0u32;
    let mut n = 0;
    while let Some(c) = p.peek() {
        if !c.is_ascii_digit() || n == 9 {
            break;
        }
        v = v * 10 + u32::from(c - b'0');
        n += 1;
        p.i += 1;
    }
    if n == 0 || p.peek().is_some_and(|c| c.is_ascii_digit()) {
        p.i = start;
        return None;
    }
    Some((v * 10u32.pow((9 - n) as u32), n))
}

fn hour(p: &mut P) -> Option<i64> {
    let start = p.i;
    let h = p.digits(2)?;
    if h <= 23 {
        Some(h)
    } else {
        p.i = start;
        None
    }
}

fn minute_second(p: &mut P, allow_60: bool) -> Option<i64> {
    let start = p.i;
    let m = p.digits(2)?;
    if m <= 59 || (allow_60 && m == 60) {
        Some(m)
    } else {
        p.i = start;
        None
    }
}

/// Time (extendido o básico, sin mezclar).
fn time(p: &mut P) -> Option<Time> {
    let start = p.i;
    let h = hour(p)?;
    let (mut mi, mut s, mut frac) = (0, 0, 0u32);
    if p.eat(b':') {
        match minute_second(p, false) {
            Some(m) => mi = m,
            None => {
                p.i = start;
                return None;
            }
        }
        if p.eat(b':') {
            match minute_second(p, true) {
                Some(x) => s = x,
                None => {
                    p.i = start;
                    return None;
                }
            }
            if let Some((f, _)) = fraction(p) {
                frac = f;
            }
        }
    } else if let Some(m) = minute_second(p, false) {
        mi = m;
        if let Some(x) = minute_second(p, true) {
            s = x;
            if let Some((f, _)) = fraction(p) {
                frac = f;
            }
        }
    }
    if s == 60 {
        s = 59;
    }
    Some(Time {
        hour: h as u8,
        minute: mi as u8,
        second: s as u8,
        millisecond: (frac / 1_000_000) as u16,
        microsecond: (frac / 1000 % 1000) as u16,
        nanosecond: (frac % 1000) as u16,
    })
}

/// UTCOffset: devuelve si tiene segundos (más de un MinuteSecond).
fn utc_offset(p: &mut P, sub_minute: bool) -> Option<bool> {
    let start = p.i;
    p.eat_any(b"+-")?;
    if hour(p).is_none() {
        p.i = start;
        return None;
    }
    let mut seconds = false;
    if p.eat(b':') {
        if minute_second(p, false).is_none() {
            p.i = start;
            return None;
        }
        if sub_minute && p.eat(b':') {
            if minute_second(p, false).is_none() {
                p.i = start;
                return None;
            }
            seconds = true;
            let _ = fraction(p);
        }
    } else if minute_second(p, false).is_some() && sub_minute {
        let save = p.i;
        if minute_second(p, false).is_some() {
            seconds = true;
            let _ = fraction(p);
        } else {
            p.i = save;
        }
    }
    Some(seconds)
}

/// DateTimeUTCOffset[Z].
fn date_time_offset(p: &mut P, allow_z: bool, core: &mut Core) {
    if allow_z && p.eat_any(b"Zz").is_some() {
        core.z = true;
        return;
    }
    let start = p.i;
    if let Some(sec) = utc_offset(p, true) {
        core.offset = Some(String::from_utf8_lossy(&p.s[start..p.i]).into_owned());
        core.offset_sub_minute = sec;
    }
}

/// DateTime[Z, TimeRequired].
fn date_time(p: &mut P, allow_z: bool, time_required: bool) -> Option<Core> {
    let start = p.i;
    let (y, m, d) = date(p)?;
    let mut core = Core {
        year: Some(y),
        month: m,
        day: Some(d),
        ..Core::default()
    };
    let save = p.i;
    if p.eat_any(b"Tt ").is_some()
        && let Some(t) = time(p)
    {
        core.time = Some(t);
        date_time_offset(p, allow_z, &mut core);
        return Some(core);
    }
    p.i = save;
    if time_required {
        p.i = start;
        return None;
    }
    Some(core)
}

/// TimeZoneIdentifier: el texto.
fn tz_identifier(p: &mut P) -> Option<()> {
    let start = p.i;
    if utc_offset(p, false).is_some() {
        return Some(());
    }
    p.i = start;
    loop {
        let comp = p.i;
        let c = p.peek()?;
        if !(c.is_ascii_alphabetic() || c == b'.' || c == b'_') {
            p.i = start;
            return None;
        }
        p.i += 1;
        while let Some(c) = p.peek() {
            if c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-' | b'+') {
                p.i += 1;
            } else {
                break;
            }
        }
        let text = &p.s[comp..p.i];
        if text == b"." || text == b".." {
            p.i = start;
            return None;
        }
        if !p.eat(b'/') {
            return Some(());
        }
    }
}

/// TimeZoneAnnotation? Annotations?: el identificador y el calendario. `None`: sintaxis
/// inválida o una anotación crítica desconocida / calendarios en conflicto.
fn annotations(p: &mut P) -> Option<(Option<String>, Option<String>)> {
    let mut tz = None;
    // TimeZoneAnnotation (sin '=').
    if p.peek() == Some(b'[') {
        let start = p.i;
        p.i += 1;
        p.eat(b'!');
        let id_start = p.i;
        if tz_identifier(p).is_some() && p.peek() == Some(b']') {
            tz = Some(String::from_utf8_lossy(&p.s[id_start..p.i]).into_owned());
            p.i += 1;
        } else {
            p.i = start;
        }
    }
    let mut calendar: Option<String> = None;
    let mut calendar_critical = false;
    while p.eat(b'[') {
        let critical = p.eat(b'!');
        // AnnotationKey.
        let ks = p.i;
        let c = p.peek()?;
        if !(c.is_ascii_lowercase() || c == b'_') {
            return None;
        }
        p.i += 1;
        while let Some(c) = p.peek() {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-' {
                p.i += 1;
            } else {
                break;
            }
        }
        let key = String::from_utf8_lossy(&p.s[ks..p.i]).into_owned();
        if !p.eat(b'=') {
            return None;
        }
        // AnnotationValue.
        let vs = p.i;
        loop {
            let cs = p.i;
            while p.peek().is_some_and(|c| c.is_ascii_alphanumeric()) {
                p.i += 1;
            }
            if p.i == cs {
                return None;
            }
            if !p.eat(b'-') {
                break;
            }
        }
        let value = String::from_utf8_lossy(&p.s[vs..p.i]).into_owned();
        if !p.eat(b']') {
            return None;
        }
        if key == "u-ca" {
            if calendar.is_none() {
                calendar = Some(value);
                calendar_critical = critical;
            } else if critical || calendar_critical {
                return None;
            }
        } else if critical {
            return None;
        }
    }
    Some((tz, calendar))
}

fn finish(core: Core, tz: Option<String>, calendar: Option<String>) -> Parsed {
    Parsed {
        year: core.year,
        month: core.month,
        day: core.day,
        time: core.time,
        z: core.z,
        offset: core.offset,
        offset_sub_minute: core.offset_sub_minute,
        tz_annotation: tz,
        calendar,
    }
}

/// AnnotatedDateTime[Zoned, TimeRequired].
fn annotated_date_time(s: &[u8], zoned: bool, time_required: bool) -> Option<Parsed> {
    let mut p = P { s, i: 0 };
    let core = date_time(&mut p, zoned, time_required)?;
    let (tz, cal) = annotations(&mut p)?;
    if !p.at_end() || (zoned && tz.is_none()) {
        return None;
    }
    Some(finish(core, tz, cal))
}

/// DateSpecYearMonth.
fn year_month(p: &mut P) -> Option<(i64, i64)> {
    let start = p.i;
    let y = date_year(p)?;
    p.eat(b'-');
    match date_month(p) {
        Some(m) => Some((y, m)),
        None => {
            p.i = start;
            None
        }
    }
}

/// DateSpecMonthDay.
fn month_day(p: &mut P) -> Option<(i64, i64)> {
    let start = p.i;
    if p.eat(b'-') && !p.eat(b'-') {
        p.i = start;
        return None;
    }
    let r = (|| {
        let m = date_month(p)?;
        p.eat(b'-');
        let d = date_day(p)?;
        Some((m, d))
    })();
    match r {
        Some((m, d)) if is_valid_date(1972, m, d) => Some((m, d)),
        _ => {
            p.i = start;
            None
        }
    }
}

fn parse_goal(s: &[u8], goal: Goal) -> Option<Parsed> {
    match goal {
        Goal::DateTime { zoned } => annotated_date_time(s, zoned, false),
        Goal::Instant => {
            let mut p = P { s, i: 0 };
            let core = date_time(&mut p, true, true)?;
            if !core.z && core.offset.is_none() {
                return None;
            }
            let (tz, cal) = annotations(&mut p)?;
            if !p.at_end() {
                return None;
            }
            Some(finish(core, tz, cal))
        }
        Goal::YearMonth => {
            let mut p = P { s, i: 0 };
            if let Some((y, m)) = year_month(&mut p)
                && let Some((tz, cal)) = annotations(&mut p)
                && p.at_end()
            {
                let core = Core {
                    year: Some(y),
                    month: m,
                    ..Core::default()
                };
                return Some(finish(core, tz, cal));
            }
            annotated_date_time(s, false, false)
        }
        Goal::MonthDay => {
            let mut p = P { s, i: 0 };
            if let Some((m, d)) = month_day(&mut p)
                && let Some((tz, cal)) = annotations(&mut p)
                && p.at_end()
            {
                let core = Core {
                    year: None,
                    month: m,
                    day: Some(d),
                    ..Core::default()
                };
                return Some(finish(core, tz, cal));
            }
            annotated_date_time(s, false, false)
        }
        Goal::Time => {
            let mut p = P { s, i: 0 };
            let designator = p.eat_any(b"Tt").is_some();
            if let Some(t) = time(&mut p) {
                let mut core = Core {
                    time: Some(t),
                    ..Core::default()
                };
                date_time_offset(&mut p, false, &mut core);
                let body_end = p.i;
                if let Some((tz, cal)) = annotations(&mut p)
                    && p.at_end()
                {
                    // AnnotatedTime sin designador: no puede ser también un MonthDay o un
                    // YearMonth (con o sin las anotaciones).
                    let ambiguous = !designator && {
                        let body = &s[..body_end];
                        let mut q = P { s: body, i: 0 };
                        let md = month_day(&mut q).is_some() && q.at_end();
                        let mut q = P { s: body, i: 0 };
                        let ym = year_month(&mut q).is_some() && q.at_end();
                        md || ym
                    };
                    if !ambiguous {
                        return Some(finish(core, tz, cal));
                    }
                }
            }
            if designator {
                return None;
            }
            annotated_date_time(s, false, true)
        }
    }
}

/// ParseISODateTime: el primer objetivo que reconoce el string. `None` es RangeError.
pub fn parse_iso_date_time(s: &str, goals: &[Goal]) -> Option<Parsed> {
    if !s.is_ascii() {
        return None;
    }
    for &g in goals {
        if let Some(r) = parse_goal(s.as_bytes(), g) {
            if matches!(g, Goal::YearMonth | Goal::MonthDay)
                && let Some(c) = &r.calendar
                && !c.eq_ignore_ascii_case("iso8601")
                && (r.day.is_none() || r.year.is_none())
            {
                return None;
            }
            return Some(r);
        }
    }
    None
}

/// Un TimeZoneIdentifier: offset en minutos o nombre.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TzId {
    Offset(i64),
    Name(String),
}

/// ParseTimeZoneIdentifier.
pub fn parse_tz_identifier(s: &str) -> Option<TzId> {
    if !s.is_ascii() {
        return None;
    }
    let mut p = P {
        s: s.as_bytes(),
        i: 0,
    };
    tz_identifier(&mut p)?;
    if !p.at_end() {
        return None;
    }
    if matches!(s.as_bytes()[0], b'+' | b'-') {
        let b = s.as_bytes();
        let sign = if b[0] == b'-' { -1 } else { 1 };
        let h = i64::from(b[1] - b'0') * 10 + i64::from(b[2] - b'0');
        let rest: Vec<u8> = b[3..].iter().copied().filter(|c| *c != b':').collect();
        let m = if rest.len() >= 2 {
            i64::from(rest[0] - b'0') * 10 + i64::from(rest[1] - b'0')
        } else {
            0
        };
        return Some(TzId::Offset(sign * (h * 60 + m)));
    }
    Some(TzId::Name(s.to_owned()))
}

/// ParseDateTimeUTCOffset: el offset en nanosegundos (`None`: no es un UTCOffset válido).
pub fn parse_offset_ns(s: &str) -> Option<i64> {
    if !s.is_ascii() {
        return None;
    }
    let mut p = P {
        s: s.as_bytes(),
        i: 0,
    };
    utc_offset(&mut p, true)?;
    if !p.at_end() {
        return None;
    }
    let b = s.as_bytes();
    let sign = if b[0] == b'-' { -1 } else { 1 };
    let mut q = P { s: b, i: 1 };
    let h = q.digits(2)?;
    q.eat(b':');
    let m = q.digits(2).unwrap_or(0);
    q.eat(b':');
    let sec = q.digits(2).unwrap_or(0);
    let frac = fraction(&mut q).map_or(0, |(f, _)| i64::from(f));
    Some(sign * (((h * 60 + m) * 60 + sec) * 1_000_000_000 + frac))
}

/// Un Duration Record parseado (los componentes como Number).
pub type DurationFields = [f64; 10];

/// ParseTemporalDurationString: None es RangeError por sintaxis.
pub fn parse_duration(s: &str) -> Option<DurationFields> {
    if !s.is_ascii() {
        return None;
    }
    let b = s.as_bytes();
    let mut i = 0;
    let mut negative = false;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        negative = b[i] == b'-';
        i += 1;
    }
    if i >= b.len() || !matches!(b[i], b'P' | b'p') {
        return None;
    }
    i += 1;
    let mut out = [0f64; 10];
    // Las partes: (dígitos, fracción, designador).
    let read_number = |i: &mut usize| -> Option<(String, Option<(u32, usize)>)> {
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        if *i == start {
            return None;
        }
        let whole = String::from_utf8_lossy(&b[start..*i]).into_owned();
        let mut frac = None;
        if *i < b.len() && (b[*i] == b'.' || b[*i] == b',') {
            let mut p = P { s: b, i: *i };
            frac = Some(fraction(&mut p)?);
            *i = p.i;
        }
        Some((whole, frac))
    };
    let to_f64 = |s: &str| -> f64 { s.parse::<f64>().unwrap_or(f64::INFINITY) };
    // Parte de fecha: Y, M, W, D en orden, sin fracciones.
    let mut any = false;
    let mut last_date = 0usize; // 1..=4
    while i < b.len() && b[i].is_ascii_digit() {
        let (whole, frac) = read_number(&mut i)?;
        if frac.is_some() || i >= b.len() {
            return None;
        }
        let idx = match b[i] {
            b'Y' | b'y' => 1,
            b'M' | b'm' => 2,
            b'W' | b'w' => 3,
            b'D' | b'd' => 4,
            _ => return None,
        };
        if idx <= last_date {
            return None;
        }
        last_date = idx;
        out[idx - 1] = to_f64(&whole);
        i += 1;
        any = true;
    }
    if i < b.len() {
        if !matches!(b[i], b'T' | b't') {
            return None;
        }
        i += 1;
        let mut last_time = 0usize; // 1 H, 2 M, 3 S
        let mut any_time = false;
        let mut fraction_seen = false;
        let mut sub_ns: i128 = 0;
        while i < b.len() {
            if fraction_seen {
                return None;
            }
            let (whole, frac) = read_number(&mut i)?;
            if i >= b.len() {
                return None;
            }
            let idx = match b[i] {
                b'H' | b'h' => 1,
                b'M' | b'm' => 2,
                b'S' | b's' => 3,
                _ => return None,
            };
            if idx <= last_time {
                return None;
            }
            last_time = idx;
            out[3 + idx] = to_f64(&whole);
            if let Some((f, _)) = frac {
                fraction_seen = true;
                // La fracción en nanosegundos de la unidad (f está en 10⁻⁹).
                let unit_ns: i128 = match idx {
                    1 => 3_600_000_000_000,
                    2 => 60_000_000_000,
                    _ => 1_000_000_000,
                };
                sub_ns = i128::from(f) * unit_ns / 1_000_000_000;
                let _ = idx;
            }
            i += 1;
            any_time = true;
        }
        if !any_time {
            return None;
        }
        if sub_ns != 0 {
            // Reparte la fracción en las unidades menores.
            let parts = [
                (5usize, 60_000_000_000i128),
                (6, 1_000_000_000),
                (7, 1_000_000),
                (8, 1000),
                (9, 1),
            ];
            let mut rest = sub_ns;
            for (k, len) in parts {
                if len >= unit_len_for(last_time) {
                    continue;
                }
                out[k] += (rest / len) as f64;
                rest %= len;
            }
        }
        any = true;
    }
    if !any {
        return None;
    }
    if negative {
        for v in &mut out {
            if *v != 0.0 {
                *v = -*v;
            }
        }
    }
    Some(out)
}

fn unit_len_for(idx: usize) -> i128 {
    match idx {
        1 => 3_600_000_000_000,
        2 => 60_000_000_000,
        _ => 1_000_000_000,
    }
}
