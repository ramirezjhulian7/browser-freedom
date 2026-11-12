//! Temporal (ECMA-262 2026, §§ Temporal): el objeto `Temporal`, los internal slots de sus
//! tipos y las operaciones compartidas: zonas horarias (UTC y offsets fijos: sin base de
//! datos IANA, como un motor sin ECMA-402 completo), los calendarios (ISO 8601 y los de
//! [`calendar`]), los property bags (PrepareCalendarFields, CalendarResolveFields,
//! CalendarMergeFields) y la conversión de los tipos.

pub mod calendar;
pub mod duration;
pub mod instant;
pub mod iso;
pub mod month_day;
pub mod now;
pub mod options;
pub mod parse;
pub mod plain_date;
pub mod plain_date_time;
pub mod plain_time;
pub mod year_month;
pub mod zoned;

use crate::builtins::{FnEntry, accessor, methods, to_string_tag};
use crate::heap::Gc;
use crate::object::{Object, ObjectKind};
use crate::string::JsString;
use crate::value::{PropertyKey, Value};
use crate::vm::function::NativeFn;
use crate::vm::realm::Intrinsics;
use crate::vm::{Agent, JsResult, RealmId};

use duration::Duration;
use iso::{IsoDate, IsoDateTime, NS_PER_DAY, Time};
use parse::TzId;

pub use calendar::Cal;

/// Los internal slots de un objeto de Temporal (`[[Calendar]]` en los que lo tienen).
#[derive(Debug, Clone, PartialEq)]
pub enum TemporalData {
    /// `[[EpochNanoseconds]]`.
    Instant(i128),
    /// `[[ISODate]]`.
    PlainDate(IsoDate, Cal),
    /// `[[Time]]`.
    PlainTime(Time),
    /// `[[ISODateTime]]`.
    PlainDateTime(IsoDateTime, Cal),
    /// `[[ISODate]]` con el día de referencia.
    PlainYearMonth(IsoDate, Cal),
    /// `[[ISODate]]` con el año de referencia.
    PlainMonthDay(IsoDate, Cal),
    /// `[[EpochNanoseconds]]`, `[[TimeZone]]` y `[[Calendar]]`.
    ZonedDateTime {
        ns: i128,
        tz: String,
        cal: Cal,
    },
    Duration(Duration),
}

impl TemporalData {
    /// El `[[Calendar]]` del objeto, si tiene.
    pub fn calendar(&self) -> Option<Cal> {
        match self {
            TemporalData::PlainDate(_, c)
            | TemporalData::PlainDateTime(_, c)
            | TemporalData::PlainYearMonth(_, c)
            | TemporalData::PlainMonthDay(_, c)
            | TemporalData::ZonedDateTime { cal: c, .. } => Some(*c),
            _ => None,
        }
    }
}

/// El identificador del calendario ISO 8601.
pub const ISO8601: &str = "iso8601";

/// El TemporalData de un valor, si es un objeto de Temporal.
pub fn data_of(agent: &Agent, v: &Value) -> Option<TemporalData> {
    match v {
        Value::Object(o) => match &agent.heap[*o].kind {
            ObjectKind::Temporal(d) => Some((**d).clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Crea un objeto de Temporal con el prototipo de `new_target` (o el intrínseco).
pub fn create(
    agent: &mut Agent,
    data: TemporalData,
    new_target: Option<Gc<Object>>,
) -> JsResult<Value> {
    let proto: fn(&Intrinsics) -> Gc<Object> = match &data {
        TemporalData::Instant(_) => |i| i.temporal_instant_prototype,
        TemporalData::PlainDate(..) => |i| i.temporal_plain_date_prototype,
        TemporalData::PlainTime(_) => |i| i.temporal_plain_time_prototype,
        TemporalData::PlainDateTime(..) => |i| i.temporal_plain_date_time_prototype,
        TemporalData::PlainYearMonth(..) => |i| i.temporal_plain_year_month_prototype,
        TemporalData::PlainMonthDay(..) => |i| i.temporal_plain_month_day_prototype,
        TemporalData::ZonedDateTime { .. } => |i| i.temporal_zoned_date_time_prototype,
        TemporalData::Duration(_) => |i| i.temporal_duration_prototype,
    };
    let kind = ObjectKind::Temporal(Box::new(data));
    let o = match new_target {
        Some(nt) => agent.ordinary_create_from_constructor(nt, proto, kind)?,
        None => {
            let p = proto(agent.intrinsics());
            agent.alloc(Object::new(Some(p), kind))
        }
    };
    Ok(Value::Object(o))
}

pub fn i128_to_bigint(n: i128) -> crate::bigint::BigInt {
    let negative = n < 0;
    let mut a = n.unsigned_abs();
    let mut mag = Vec::new();
    while a != 0 {
        mag.push(a as u32);
        a >>= 32;
    }
    crate::bigint::BigInt { negative, mag }
}

/// Un BigInt que cabe en i128 (los de Temporal están acotados por nsMaxInstant).
pub fn bigint_to_i128(b: &crate::bigint::BigInt) -> Option<i128> {
    if b.mag.len() > 4 {
        return None;
    }
    let mut a: u128 = 0;
    for (k, d) in b.mag.iter().enumerate() {
        a |= u128::from(*d) << (32 * k);
    }
    if a > i128::MAX as u128 {
        return None;
    }
    Some(if b.negative { -(a as i128) } else { a as i128 })
}

pub fn bigint_value(agent: &mut Agent, n: i128) -> Value {
    Value::BigInt(agent.heap.alloc(i128_to_bigint(n)))
}

// --- Zonas horarias ---

/// GetAvailableNamedTimeZoneIdentifier: sin la base de datos IANA, UTC y sus alias (con el
/// identificador tal como lo da la base de datos).
fn available_named_zone(name: &str) -> Option<&'static str> {
    const ZONES: &[&str] = &["UTC", "Etc/UTC", "Etc/GMT", "GMT"];
    ZONES.iter().copied().find(|z| z.eq_ignore_ascii_case(name))
}

/// ¿Es un identificador de offset (`±HH:MM`)?
pub fn is_offset_zone(tz: &str) -> bool {
    tz.starts_with('+') || tz.starts_with('-')
}

/// El offset fijo de la zona en nanosegundos (GetOffsetNanosecondsFor: todas las zonas
/// disponibles tienen offset constante).
pub fn offset_ns(tz: &str) -> i128 {
    match parse::parse_tz_identifier(tz) {
        Some(TzId::Offset(m)) => i128::from(m) * 60_000_000_000,
        _ => 0,
    }
}

/// GetISODateTimeFor.
pub fn date_time_for(tz: &str, ns: i128) -> IsoDateTime {
    IsoDateTime::from_epoch_ns(ns + offset_ns(tz))
}

/// TimeZoneEquals.
pub fn time_zone_equals(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    match (parse::parse_tz_identifier(a), parse::parse_tz_identifier(b)) {
        (Some(TzId::Offset(x)), Some(TzId::Offset(y))) => x == y,
        (Some(TzId::Name(_)), Some(TzId::Name(_))) => {
            // Todos los nombres disponibles son UTC.
            available_named_zone(a).is_some() && available_named_zone(b).is_some()
        }
        _ => false,
    }
}

impl Agent {
    /// El identificador de zona de un string ya parseado como TimeZoneIdentifier.
    fn temporal_zone_from_id(&mut self, id: TzId) -> JsResult<String> {
        match id {
            TzId::Offset(m) => Ok(iso::format_offset_minutes(m)),
            TzId::Name(n) => match available_named_zone(&n) {
                Some(z) => Ok(z.to_owned()),
                None => self.range_error(&format!("Unknown time zone: {n}")),
            },
        }
    }

    /// ToTemporalTimeZoneIdentifier.
    pub fn to_temporal_time_zone(&mut self, v: &Value) -> JsResult<String> {
        if let Some(TemporalData::ZonedDateTime { tz, .. }) = data_of(self, v) {
            return Ok(tz);
        }
        let Value::String(s) = v else {
            return self.type_error("time zone must be a string");
        };
        let s = s.to_string_lossy();
        // ParseTemporalTimeZoneString.
        if let Some(id) = parse::parse_tz_identifier(&s) {
            return self.temporal_zone_from_id(id);
        }
        use parse::Goal;
        let Some(r) = parse::parse_iso_date_time(
            &s,
            &[
                Goal::DateTime { zoned: true },
                Goal::DateTime { zoned: false },
                Goal::Instant,
                Goal::Time,
                Goal::MonthDay,
                Goal::YearMonth,
            ],
        ) else {
            return self.range_error(&format!("Invalid time zone: {s}"));
        };
        if let Some(a) = r.tz_annotation {
            let id = parse::parse_tz_identifier(&a).expect("anotación válida");
            return self.temporal_zone_from_id(id);
        }
        if r.z {
            return Ok("UTC".to_owned());
        }
        if let Some(o) = r.offset {
            // Un offset con segundos no es un identificador.
            match parse::parse_tz_identifier(&o) {
                Some(id) => return self.temporal_zone_from_id(id),
                None => return self.range_error(&format!("Invalid time zone: {s}")),
            }
        }
        self.range_error(&format!("Invalid time zone: {s}"))
    }

    /// GetPossibleEpochNanoseconds + DisambiguatePossibleEpochNanoseconds (todas las zonas
    /// tienen offset constante: siempre hay exactamente uno).
    pub fn temporal_epoch_ns_for(&mut self, tz: &str, dt: IsoDateTime) -> JsResult<i128> {
        let ns = dt.epoch_ns() - offset_ns(tz);
        // Con un offset fijo se comprueba la fecha balanceada en UTC; con un nombre, la
        // fecha local.
        if is_offset_zone(tz) {
            self.temporal_check_days_range(IsoDateTime::from_epoch_ns(ns).date)?;
        } else {
            self.temporal_check_days_range(dt.date)?;
        }
        if !iso::is_valid_epoch_ns(ns) {
            return self.range_error("date-time is outside the representable range");
        }
        Ok(ns)
    }

    /// CheckISODaysRange.
    pub fn temporal_check_days_range(&mut self, d: IsoDate) -> JsResult<()> {
        if d.epoch_days().abs() > 100_000_000 {
            return self.range_error("date is outside the representable range");
        }
        Ok(())
    }

    /// GetStartOfDay.
    pub fn temporal_start_of_day(&mut self, tz: &str, d: IsoDate) -> JsResult<i128> {
        self.temporal_epoch_ns_for(
            tz,
            IsoDateTime {
                date: d,
                time: Time::MIDNIGHT,
            },
        )
    }

    /// InterpretISODateTimeOffset. `offset`: Some(ns) con offsetBehaviour option; `exact`:
    /// offsetBehaviour exact (Z); `time` None es start-of-day.
    #[allow(clippy::too_many_arguments)]
    pub fn temporal_interpret_offset(
        &mut self,
        date: IsoDate,
        time: Option<Time>,
        exact: bool,
        offset: Option<i128>,
        tz: &str,
        reject_disambiguation: bool,
        offset_option: &str,
        match_minutes: bool,
    ) -> JsResult<i128> {
        let _ = reject_disambiguation;
        let Some(time) = time else {
            return self.temporal_start_of_day(tz, date);
        };
        let dt = IsoDateTime { date, time };
        if (offset.is_none() && !exact) || (offset.is_some() && offset_option == "ignore") {
            return self.temporal_epoch_ns_for(tz, dt);
        }
        let offset_ns_value = offset.unwrap_or(0);
        if exact || offset_option == "use" {
            // BalanceISODateTime con el offset restado + CheckISODaysRange.
            let ns = dt.epoch_ns() - offset_ns_value;
            let balanced = IsoDateTime::from_epoch_ns(ns);
            self.temporal_check_days_range(balanced.date)?;
            if !iso::is_valid_epoch_ns(ns) {
                return self.range_error("date-time is outside the representable range");
            }
            return Ok(ns);
        }
        self.temporal_check_days_range(date)?;
        let utc = dt.epoch_ns();
        let candidate = self.temporal_epoch_ns_for(tz, dt)?;
        let candidate_offset = utc - candidate;
        if candidate_offset == offset_ns_value {
            return Ok(candidate);
        }
        if match_minutes {
            let rounded = options::round_to_increment(
                candidate_offset,
                60_000_000_000,
                options::RoundingMode::HalfExpand,
            );
            if rounded == offset_ns_value {
                return Ok(candidate);
            }
        }
        if offset_option == "reject" {
            return self.range_error("offset does not match the time zone");
        }
        Ok(candidate)
    }

    // --- Calendario ---

    /// CanonicalizeCalendar.
    pub fn temporal_canonicalize_calendar(&mut self, id: &str) -> JsResult<Cal> {
        match Cal::from_id(id) {
            Some(c) => Ok(c),
            None => self.range_error(&format!("Unsupported calendar: {id}")),
        }
    }

    /// ToTemporalCalendarIdentifier.
    pub fn to_temporal_calendar(&mut self, v: &Value) -> JsResult<Cal> {
        if let Some(c) = data_of(self, v).and_then(|d| d.calendar()) {
            return Ok(c);
        }
        let Value::String(s) = v else {
            return self.type_error("calendar must be a string");
        };
        let s = s.to_string_lossy();
        // ParseTemporalCalendarString.
        use parse::Goal;
        let id = match parse::parse_iso_date_time(
            &s,
            &[
                Goal::DateTime { zoned: true },
                Goal::DateTime { zoned: false },
                Goal::Instant,
                Goal::Time,
                Goal::MonthDay,
                Goal::YearMonth,
            ],
        ) {
            Some(r) => r.calendar.unwrap_or_else(|| ISO8601.to_owned()),
            None => {
                let ok = !s.is_empty()
                    && s.split('-').all(|c| {
                        (3..=8).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_alphanumeric())
                    });
                if !ok {
                    return self.range_error(&format!("Invalid calendar: {s}"));
                }
                s
            }
        };
        self.temporal_canonicalize_calendar(&id)
    }

    /// GetTemporalCalendarIdentifierWithISODefault.
    pub fn temporal_calendar_with_default(&mut self, item: Gc<Object>) -> JsResult<Cal> {
        if let Some(c) = data_of(self, &Value::Object(item)).and_then(|d| d.calendar()) {
            return Ok(c);
        }
        let c = self.get(item, &PropertyKey::from("calendar"), Value::Object(item))?;
        if c.is_undefined() {
            return Ok(Cal::Iso);
        }
        self.to_temporal_calendar(&c)
    }

    /// La validación de un argumento `calendar` de un constructor.
    pub fn temporal_calendar_arg(&mut self, v: &Value) -> JsResult<Cal> {
        match v {
            Value::Undefined => Ok(Cal::Iso),
            Value::String(s) => {
                let s = s.to_string_lossy();
                self.temporal_canonicalize_calendar(&s)
            }
            _ => self.type_error("calendar must be a string"),
        }
    }

    /// IsPartialTemporalObject.
    pub fn temporal_is_partial(&mut self, v: &Value) -> JsResult<bool> {
        let Value::Object(o) = v else {
            return Ok(false);
        };
        if data_of(self, v).is_some_and(|d| !matches!(d, TemporalData::Duration(_))) {
            return Ok(false);
        }
        let o = *o;
        let c = self.get(o, &PropertyKey::from("calendar"), Value::Object(o))?;
        if !c.is_undefined() {
            return Ok(false);
        }
        let t = self.get(o, &PropertyKey::from("timeZone"), Value::Object(o))?;
        Ok(t.is_undefined())
    }
}

// --- Property bags ---

/// Los campos de Table 19 (en orden alfabético de nombre de propiedad).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Day,
    Era,
    EraYear,
    Hour,
    Microsecond,
    Millisecond,
    Minute,
    Month,
    MonthCode,
    Nanosecond,
    Offset,
    Second,
    TimeZone,
    Year,
}

const FIELD_ORDER: [(Field, &str); 14] = [
    (Field::Day, "day"),
    (Field::Era, "era"),
    (Field::EraYear, "eraYear"),
    (Field::Hour, "hour"),
    (Field::Microsecond, "microsecond"),
    (Field::Millisecond, "millisecond"),
    (Field::Minute, "minute"),
    (Field::Month, "month"),
    (Field::MonthCode, "monthCode"),
    (Field::Nanosecond, "nanosecond"),
    (Field::Offset, "offset"),
    (Field::Second, "second"),
    (Field::TimeZone, "timeZone"),
    (Field::Year, "year"),
];

pub const DATE_FIELDS: &[Field] = &[Field::Year, Field::Month, Field::MonthCode, Field::Day];
pub const TIME_FIELDS: &[Field] = &[
    Field::Hour,
    Field::Minute,
    Field::Second,
    Field::Millisecond,
    Field::Microsecond,
    Field::Nanosecond,
];

/// Calendar Fields Record (None: unset).
#[derive(Debug, Clone, Default)]
pub struct Fields {
    pub era: Option<String>,
    pub era_year: Option<f64>,
    pub year: Option<f64>,
    pub month: Option<f64>,
    pub month_code: Option<String>,
    pub day: Option<f64>,
    pub hour: Option<f64>,
    pub minute: Option<f64>,
    pub second: Option<f64>,
    pub millisecond: Option<f64>,
    pub microsecond: Option<f64>,
    pub nanosecond: Option<f64>,
    pub offset: Option<String>,
    pub time_zone: Option<String>,
}

/// requiredFieldNames de PrepareCalendarFields.
pub enum Required<'a> {
    Partial,
    List(&'a [Field]),
}

impl Fields {
    /// ISODateToFields (con CalendarISOToDate del calendario). Fuera de ISO 8601 va solo el
    /// código de mes: el mes ordinal depende del año (y se recalcula si `with` lo cambia).
    pub fn from_date(cal: Cal, d: IsoDate, year: bool, day: bool) -> Fields {
        let c = cal.to_cal(d);
        Fields {
            year: year.then_some(c.year as f64),
            month: (year && cal.is_iso()).then_some(f64::from(c.month)),
            month_code: Some(c.code.text()),
            day: day.then_some(f64::from(c.day)),
            ..Fields::default()
        }
    }

    pub fn set_time(&mut self, t: Time) {
        self.hour = Some(f64::from(t.hour));
        self.minute = Some(f64::from(t.minute));
        self.second = Some(f64::from(t.second));
        self.millisecond = Some(f64::from(t.millisecond));
        self.microsecond = Some(f64::from(t.microsecond));
        self.nanosecond = Some(f64::from(t.nanosecond));
    }

    /// CalendarMergeFields con CalendarFieldKeysToIgnore: `month` y `monthCode` se reemplazan
    /// juntos; con eras, `era`, `eraYear` y `year` también; en un calendario con eras a mitad
    /// de año (japonés), `day`, `month` y `monthCode` invalidan además la era.
    pub fn merge(mut self, cal: Cal, add: Fields) -> Fields {
        let month_keys = add.month.is_some() || add.month_code.is_some();
        let year_keys = add.era.is_some() || add.era_year.is_some() || add.year.is_some();
        // Las claves de `self` que se ignoran (y quedan como en `add`, presentes o no).
        let ignore_era = (cal.has_eras() && year_keys)
            || (cal.has_mid_year_eras() && (month_keys || add.day.is_some()));
        if month_keys {
            self.month = add.month;
            self.month_code = add.month_code;
        }
        if cal.has_eras() && year_keys {
            self.year = add.year;
        }
        if ignore_era {
            self.era = add.era;
            self.era_year = add.era_year;
        }
        macro_rules! m {
            ($($f:ident),*) => { $( if add.$f.is_some() { self.$f = add.$f; } )* };
        }
        m!(
            year,
            day,
            hour,
            minute,
            second,
            millisecond,
            microsecond,
            nanosecond,
            offset,
            time_zone
        );
        self
    }

    /// Los componentes de la hora (unset → 0).
    pub fn time_values(&self) -> [f64; 6] {
        [
            self.hour.unwrap_or(0.0),
            self.minute.unwrap_or(0.0),
            self.second.unwrap_or(0.0),
            self.millisecond.unwrap_or(0.0),
            self.microsecond.unwrap_or(0.0),
            self.nanosecond.unwrap_or(0.0),
        ]
    }
}

impl Agent {
    /// PrepareCalendarFields: con CalendarExtraFields (`era` y `eraYear` si se piden campos
    /// de año y el calendario tiene eras).
    pub fn temporal_prepare_fields(
        &mut self,
        cal: Cal,
        item: Gc<Object>,
        names: &[Field],
        required: Required<'_>,
    ) -> JsResult<Fields> {
        let mut f = Fields::default();
        let mut any = false;
        let extra = cal.has_eras() && names.contains(&Field::Year);
        for (field, prop) in FIELD_ORDER {
            let wanted =
                names.contains(&field) || (extra && matches!(field, Field::Era | Field::EraYear));
            if !wanted {
                continue;
            }
            let v = self.get(item, &PropertyKey::from(prop), Value::Object(item))?;
            if v.is_undefined() {
                if let Required::List(req) = required
                    && req.contains(&field)
                {
                    return self.type_error(&format!("{prop} is required"));
                }
                continue;
            }
            any = true;
            match field {
                Field::Era => f.era = Some(self.to_string(&v)?.to_string_lossy()),
                Field::Year
                | Field::EraYear
                | Field::Hour
                | Field::Minute
                | Field::Second
                | Field::Millisecond
                | Field::Microsecond
                | Field::Nanosecond => {
                    let n = self.to_integer_with_truncation(&v)?;
                    match field {
                        Field::Year => f.year = Some(n),
                        Field::EraYear => f.era_year = Some(n),
                        Field::Hour => f.hour = Some(n),
                        Field::Minute => f.minute = Some(n),
                        Field::Second => f.second = Some(n),
                        Field::Millisecond => f.millisecond = Some(n),
                        Field::Microsecond => f.microsecond = Some(n),
                        _ => f.nanosecond = Some(n),
                    }
                }
                Field::Month => f.month = Some(self.to_positive_integer_with_truncation(&v)?),
                Field::Day => f.day = Some(self.to_positive_integer_with_truncation(&v)?),
                Field::MonthCode => {
                    // ToMonthCode.
                    let s = self.temporal_primitive_string(&v, "monthCode")?;
                    let b = s.as_bytes();
                    let ok = (b.len() == 3 || (b.len() == 4 && b[3] == b'L'))
                        && b[0] == b'M'
                        && b[1].is_ascii_digit()
                        && b[2].is_ascii_digit()
                        && !(b.len() == 3 && b[1] == b'0' && b[2] == b'0');
                    if !ok {
                        return self.range_error(&format!("Invalid monthCode: {s}"));
                    }
                    f.month_code = Some(s);
                }
                Field::Offset => {
                    // ToOffsetString.
                    let s = self.temporal_primitive_string(&v, "offset")?;
                    if parse::parse_offset_ns(&s).is_none() {
                        return self.range_error(&format!("Invalid offset: {s}"));
                    }
                    f.offset = Some(s);
                }
                Field::TimeZone => f.time_zone = Some(self.to_temporal_time_zone(&v)?),
            }
        }
        if matches!(required, Required::Partial) && !any {
            return self.type_error("at least one field is required");
        }
        Ok(f)
    }

    /// CalendarResolveFields. `kind`: 0 date, 1 year-month, 2 month-day.
    pub fn temporal_resolve_fields(&mut self, cal: Cal, f: &mut Fields, kind: u8) -> JsResult<()> {
        if !cal.is_iso() {
            return self.temporal_resolve_fields_non_iso(cal, f, kind);
        }
        if kind != 2 && f.year.is_none() {
            return self.type_error("year is required");
        }
        if kind != 1 && f.day.is_none() {
            return self.type_error("day is required");
        }
        let Some(code) = f.month_code.clone() else {
            if f.month.is_none() {
                return self.type_error("month or monthCode is required");
            }
            return Ok(());
        };
        let b = code.as_bytes();
        if b.len() != 3 {
            return self.range_error(&format!(
                "Invalid monthCode for the ISO 8601 calendar: {code}"
            ));
        }
        let n = f64::from((b[1] - b'0') * 10 + (b[2] - b'0'));
        if !(1.0..=12.0).contains(&n) {
            return self.range_error(&format!(
                "Invalid monthCode for the ISO 8601 calendar: {code}"
            ));
        }
        if let Some(m) = f.month
            && m != n
        {
            return self.range_error("month and monthCode do not agree");
        }
        f.month = Some(n);
        Ok(())
    }

    /// RegulateISODate.
    pub fn temporal_regulate_date(
        &mut self,
        y: f64,
        m: f64,
        d: f64,
        reject: bool,
    ) -> JsResult<(i64, i64, i64)> {
        if reject {
            if !iso::is_valid_date(y as i64, m as i64, d as i64) {
                return self.range_error("invalid date");
            }
            return Ok((y as i64, m as i64, d as i64));
        }
        let y = y as i64;
        let m = m.clamp(1.0, 12.0) as i64;
        let d = (d.clamp(1.0, 31.0) as i64).min(iso::days_in_month(y, m));
        Ok((y, m, d))
    }

    /// NonISOResolveFields.
    fn temporal_resolve_fields_non_iso(
        &mut self,
        cal: Cal,
        f: &mut Fields,
        kind: u8,
    ) -> JsResult<()> {
        let needs_year = kind != 2 || f.month_code.is_none() || f.month.is_some();
        let needs_day = kind != 1;
        if needs_year && f.year.is_none() && (f.era.is_none() || f.era_year.is_none()) {
            return self.type_error("year (or era and eraYear) is required");
        }
        if f.era.is_some() != f.era_year.is_some() {
            return self.type_error("era and eraYear must be given together");
        }
        if needs_day && f.day.is_none() {
            return self.type_error("day is required");
        }
        if f.month.is_none() && f.month_code.is_none() {
            return self.type_error("month or monthCode is required");
        }
        if let (Some(era), Some(era_year)) = (f.era.take(), f.era_year.take()) {
            let Some(era) = cal.canonical_era(&era) else {
                return self
                    .range_error(&format!("Invalid era for the {} calendar: {era}", cal.id()));
            };
            let y = cal.year_for_era(era, era_year as i64) as f64;
            if let Some(year) = f.year
                && year != y
            {
                return self.range_error("year and era/eraYear do not agree");
            }
            f.year = Some(y);
        }
        if let Some(code) = f.month_code.clone() {
            let parsed = calendar::MonthCode::parse(&code).filter(|c| cal.valid_code(*c));
            let Some(c) = parsed else {
                return self.range_error(&format!(
                    "Invalid monthCode for the {} calendar: {code}",
                    cal.id()
                ));
            };
            if let Some(y) = f.year {
                let y = y as i64;
                let c = cal.constrain_code(y, c, false).unwrap_or(c);
                let m = f64::from(cal.ordinal_of(y, c).unwrap_or(c.num));
                if let Some(month) = f.month
                    && month != m
                {
                    return self.range_error("month and monthCode do not agree");
                }
                f.month = Some(m);
            }
        }
        Ok(())
    }

    /// NonISOCalendarDateToISO.
    fn temporal_non_iso_to_iso(&mut self, cal: Cal, f: &Fields, reject: bool) -> JsResult<IsoDate> {
        let y = f.year.expect("year");
        if y.abs() > 400_000.0 {
            return self.range_error("date is outside the representable range");
        }
        let y = y as i64;
        if let Some(code) = &f.month_code {
            let c = calendar::MonthCode::parse(code).expect("monthCode validado");
            if cal.constrain_code(y, c, reject).is_none() {
                return self.range_error(&format!("{code} does not exist in year {y}"));
            }
        }
        let miy = f64::from(cal.months_in_year(y));
        let mut m = f.month.expect("month");
        if m > miy {
            if reject {
                return self.range_error("month is out of range");
            }
            m = miy;
        }
        let m = m as u8;
        let dim = f64::from(cal.days_in_month(y, m));
        let mut d = f.day.unwrap_or(1.0);
        if d > dim {
            if reject {
                return self.range_error("day is out of range");
            }
            d = dim;
        }
        let r = cal.from_cal(y, m, d as u8);
        if !iso::date_within_limits(r) {
            return self.range_error("date is outside the representable range");
        }
        Ok(r)
    }

    /// CalendarDateFromFields.
    pub fn temporal_date_from_fields(
        &mut self,
        cal: Cal,
        mut f: Fields,
        reject: bool,
    ) -> JsResult<IsoDate> {
        self.temporal_resolve_fields(cal, &mut f, 0)?;
        if !cal.is_iso() {
            return self.temporal_non_iso_to_iso(cal, &f, reject);
        }
        let (y, m, d) = self.temporal_regulate_date(
            f.year.expect("year"),
            f.month.expect("month"),
            f.day.expect("day"),
            reject,
        )?;
        if !iso::date_within_limits_i64(y, m, d) {
            return self.range_error("date is outside the representable range");
        }
        Ok(IsoDate::new(y, m, d))
    }

    /// CalendarYearMonthFromFields.
    pub fn temporal_year_month_from_fields(
        &mut self,
        cal: Cal,
        mut f: Fields,
        reject: bool,
    ) -> JsResult<IsoDate> {
        self.temporal_resolve_fields(cal, &mut f, 1)?;
        if !cal.is_iso() {
            f.day = Some(1.0);
            let r = self.temporal_non_iso_to_iso(cal, &f, reject)?;
            if !iso::year_month_within_limits(r.year.into(), r.month.into()) {
                return self.range_error("year-month is outside the representable range");
            }
            return Ok(r);
        }
        let (y, m, _) = self.temporal_regulate_date(
            f.year.expect("year"),
            f.month.expect("month"),
            1.0,
            reject,
        )?;
        if !iso::year_month_within_limits(y, m) {
            return self.range_error("year-month is outside the representable range");
        }
        Ok(IsoDate::new(y, m, 1))
    }

    /// CalendarMonthDayFromFields.
    pub fn temporal_month_day_from_fields(
        &mut self,
        cal: Cal,
        mut f: Fields,
        reject: bool,
    ) -> JsResult<IsoDate> {
        self.temporal_resolve_fields(cal, &mut f, 2)?;
        if !cal.is_iso() {
            return self.temporal_non_iso_month_day(cal, f, reject);
        }
        let year = f.year.unwrap_or(1972.0);
        let (_, m, d) = self.temporal_regulate_date(
            year,
            f.month.expect("month"),
            f.day.expect("day"),
            reject,
        )?;
        Ok(IsoDate::new(1972, m, d))
    }

    /// NonISOMonthDayToISOReferenceDate.
    fn temporal_non_iso_month_day(
        &mut self,
        cal: Cal,
        f: Fields,
        reject: bool,
    ) -> JsResult<IsoDate> {
        let mut day = f.day.expect("day");
        let (code, dim) = if let Some(y) = f.year {
            if y.abs() > 400_000.0 {
                return self.range_error("date is outside the representable range");
            }
            let y = y as i64;
            let miy = cal.months_in_year(y);
            let mut m = f.month.expect("month");
            if m > f64::from(miy) {
                if reject {
                    return self.range_error("month is out of range");
                }
                m = f64::from(miy);
            }
            let m = m as u8;
            let code = match &f.month_code {
                None => cal.code_of(y, m),
                Some(s) => {
                    let c = calendar::MonthCode::parse(s).expect("monthCode validado");
                    match cal.constrain_code(y, c, reject) {
                        Some(c) => c,
                        None => {
                            return self.range_error(&format!("{s} does not exist in year {y}"));
                        }
                    }
                }
            };
            (code, cal.days_in_month(y, m))
        } else {
            let c = calendar::MonthCode::parse(f.month_code.as_deref().expect("monthCode"))
                .expect("monthCode validado");
            (c, cal.max_days_in_code(c))
        };
        if day > f64::from(dim) {
            if reject {
                return self.range_error("day is out of range");
            }
            day = f64::from(dim);
        }
        match cal.month_day_reference(code, day as u8) {
            Some(d) => Ok(d),
            None => self.range_error("no reference date for this month and day"),
        }
    }

    /// RegulateTime.
    pub fn temporal_regulate_time(&mut self, t: [f64; 6], reject: bool) -> JsResult<Time> {
        if reject {
            if !iso::is_valid_time(t[0], t[1], t[2], t[3], t[4], t[5]) {
                return self.range_error("invalid time");
            }
            return Ok(Time {
                hour: t[0] as u8,
                minute: t[1] as u8,
                second: t[2] as u8,
                millisecond: t[3] as u16,
                microsecond: t[4] as u16,
                nanosecond: t[5] as u16,
            });
        }
        Ok(iso::constrain_time(t[0], t[1], t[2], t[3], t[4], t[5]))
    }

    /// InterpretTemporalDateTimeFields.
    pub fn temporal_date_time_from_fields(
        &mut self,
        cal: Cal,
        f: Fields,
        reject: bool,
    ) -> JsResult<IsoDateTime> {
        let tv = f.time_values();
        let date = self.temporal_date_from_fields(cal, f, reject)?;
        let time = self.temporal_regulate_time(tv, reject)?;
        Ok(IsoDateTime { date, time })
    }

    /// ToTemporalTimeRecord: `partial` deja los ausentes en None.
    pub fn temporal_time_record(
        &mut self,
        item: Gc<Object>,
        partial: bool,
    ) -> JsResult<[Option<f64>; 6]> {
        // hour, microsecond, millisecond, minute, nanosecond, second.
        const ORDER: [(&str, usize); 6] = [
            ("hour", 0),
            ("microsecond", 4),
            ("millisecond", 3),
            ("minute", 1),
            ("nanosecond", 5),
            ("second", 2),
        ];
        let mut out = if partial { [None; 6] } else { [Some(0.0); 6] };
        let mut any = false;
        for (name, idx) in ORDER {
            let v = self.get(item, &PropertyKey::from(name), Value::Object(item))?;
            if !v.is_undefined() {
                any = true;
                out[idx] = Some(self.to_integer_with_truncation(&v)?);
            }
        }
        if !any {
            return self.type_error("at least one time field is required");
        }
        Ok(out)
    }

    /// CreateTemporalDate: comprueba los límites.
    pub fn temporal_check_date(&mut self, d: IsoDate) -> JsResult<()> {
        if !iso::date_within_limits(d) {
            return self.range_error("date is outside the representable range");
        }
        Ok(())
    }

    /// CreateTemporalDateTime: comprueba los límites.
    pub fn temporal_check_date_time(&mut self, dt: IsoDateTime) -> JsResult<()> {
        if !iso::date_time_within_limits(dt) {
            return self.range_error("date-time is outside the representable range");
        }
        Ok(())
    }
}

/// FormatCalendarAnnotation.
pub fn calendar_annotation(cal: Cal, show: &str) -> String {
    match show {
        "never" => String::new(),
        "auto" if cal.is_iso() => String::new(),
        "critical" => format!("[!u-ca={}]", cal.id()),
        _ => format!("[u-ca={}]", cal.id()),
    }
}

impl Agent {
    /// GetTemporalShowCalendarNameOption.
    pub fn temporal_show_calendar(&mut self, o: Gc<Object>) -> JsResult<String> {
        Ok(self
            .temporal_string_option(o, "calendarName", &["auto", "always", "never", "critical"])?
            .unwrap_or_else(|| "auto".to_owned()))
    }
}

// --- Registro ---

/// Define los getters de una tabla.
pub(crate) fn getters(agent: &mut Agent, proto: Gc<Object>, table: &[(&str, NativeFn)]) {
    for &(name, f) in table {
        accessor(agent, proto, PropertyKey::from(name), name, Some(f), None);
    }
}

/// Un constructor de Temporal con su prototipo (los métodos los agrega cada tipo).
pub(crate) fn class(
    agent: &mut Agent,
    temporal: Gc<Object>,
    name: &str,
    length: u32,
    ctor: NativeFn,
    statics: &[FnEntry],
) -> (Gc<Object>, Gc<Object>) {
    let op = agent.intrinsics().object_prototype;
    let proto = agent.new_object(Some(op));
    to_string_tag(agent, proto, &format!("Temporal.{name}"));
    let c = crate::builtins::constructor(agent, name, length, ctor, proto);
    methods(agent, c, statics);
    agent.put_data(
        temporal,
        PropertyKey::from(name),
        Value::Object(c),
        true,
        false,
        true,
    );
    (c, proto)
}

pub fn init(agent: &mut Agent, realm: RealmId) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    let temporal = agent.new_object(Some(op));
    to_string_tag(agent, temporal, "Temporal");
    let p = duration::init(agent, temporal);
    agent.realms[realm].intrinsics.temporal_duration_prototype = p;
    let p = instant::init(agent, temporal);
    agent.realms[realm].intrinsics.temporal_instant_prototype = p;
    let p = plain_date::init(agent, temporal);
    agent.realms[realm].intrinsics.temporal_plain_date_prototype = p;
    let p = plain_time::init(agent, temporal);
    agent.realms[realm].intrinsics.temporal_plain_time_prototype = p;
    let p = plain_date_time::init(agent, temporal);
    agent.realms[realm]
        .intrinsics
        .temporal_plain_date_time_prototype = p;
    let p = year_month::init(agent, temporal);
    agent.realms[realm]
        .intrinsics
        .temporal_plain_year_month_prototype = p;
    let p = month_day::init(agent, temporal);
    agent.realms[realm]
        .intrinsics
        .temporal_plain_month_day_prototype = p;
    let p = zoned::init(agent, temporal);
    agent.realms[realm]
        .intrinsics
        .temporal_zoned_date_time_prototype = p;
    now::init(agent, temporal);
    crate::builtins::register_global(agent, realm, "Temporal", temporal);
    let dp = agent.realms[realm].intrinsics.date_prototype;
    crate::builtins::method(agent, dp, "toTemporalInstant", 0, date_to_temporal_instant);
}

/// Date.prototype.toTemporalInstant ( ).
fn date_to_temporal_instant(
    agent: &mut Agent,
    a: &crate::vm::function::CallArgs<'_>,
) -> JsResult<Value> {
    let t = match &a.this {
        Value::Object(o) => match agent.heap[*o].kind {
            ObjectKind::Date(t) => t,
            _ => return agent.type_error("Date.prototype.toTemporalInstant requires a Date"),
        },
        _ => return agent.type_error("Date.prototype.toTemporalInstant requires a Date"),
    };
    if t.is_nan() {
        return agent.range_error("Invalid Date");
    }
    agent.temporal_create_instant((t as i128) * 1_000_000, None)
}

/// `this` como un objeto de Temporal de cierto tipo (RequireInternalSlot).
pub(crate) fn this_data(
    agent: &mut Agent,
    this: &Value,
    class: &str,
    method: &str,
    ok: fn(&TemporalData) -> bool,
) -> JsResult<TemporalData> {
    match data_of(agent, this) {
        Some(d) if ok(&d) => Ok(d),
        _ => agent.type_error(&format!(
            "Temporal.{class}.prototype.{method} called on incompatible receiver"
        )),
    }
}

pub(crate) fn js_str(s: &str) -> Value {
    Value::String(JsString::from(s))
}

/// `nsPerDay` como i128 (atajo).
pub const DAY: i128 = NS_PER_DAY;

/// `valueOf` de los tipos de Temporal: siempre TypeError.
pub(crate) fn value_of(
    agent: &mut Agent,
    _: &crate::vm::function::CallArgs<'_>,
) -> JsResult<Value> {
    agent
        .type_error("Temporal objects cannot be converted to primitives; use compare() or equals()")
}

/// ToSecondsStringPrecisionRecord: (precisión, unidad, incremento).
pub(crate) fn seconds_precision(
    smallest: options::UnitOption,
    digits: Option<u8>,
) -> (iso::Precision, options::Unit, i128) {
    use iso::Precision as P;
    use options::Unit as U;
    match smallest {
        options::UnitOption::Unit(U::Minute) => (P::Minute, U::Minute, 1),
        options::UnitOption::Unit(U::Second) => (P::Digits(0), U::Second, 1),
        options::UnitOption::Unit(U::Millisecond) => (P::Digits(3), U::Millisecond, 1),
        options::UnitOption::Unit(U::Microsecond) => (P::Digits(6), U::Microsecond, 1),
        options::UnitOption::Unit(U::Nanosecond) => (P::Digits(9), U::Nanosecond, 1),
        _ => match digits {
            None => (P::Auto, U::Nanosecond, 1),
            Some(0) => (P::Digits(0), U::Second, 1),
            Some(d @ 1..=3) => (P::Digits(d), U::Millisecond, 10i128.pow(3 - u32::from(d))),
            Some(d @ 4..=6) => (P::Digits(d), U::Microsecond, 10i128.pow(6 - u32::from(d))),
            Some(d) => (P::Digits(d), U::Nanosecond, 10i128.pow(9 - u32::from(d))),
        },
    }
}

/// RoundTime: (días, hora).
pub(crate) fn round_time(
    t: Time,
    increment: i128,
    unit: options::Unit,
    mode: options::RoundingMode,
) -> (i64, Time) {
    use options::Unit as U;
    let ns = t.nanos();
    let quantity = match unit {
        U::Day | U::Hour => ns,
        U::Minute => ns - i128::from(t.hour) * 3_600_000_000_000,
        U::Second => ns % 60_000_000_000,
        U::Millisecond => ns % 1_000_000_000,
        U::Microsecond => ns % 1_000_000,
        _ => i128::from(t.nanosecond),
    };
    let len = if unit == U::Day {
        NS_PER_DAY
    } else {
        unit.nanos()
    };
    let rounded = options::round_to_increment(quantity, increment * len, mode);
    if unit == U::Day {
        return ((rounded / NS_PER_DAY) as i64, Time::MIDNIGHT);
    }
    // BalanceTime con los componentes mayores intactos.
    let base = ns - quantity;
    iso::balance_time(base + rounded)
}

/// RoundISODateTime.
pub(crate) fn round_date_time(
    dt: IsoDateTime,
    increment: i128,
    unit: options::Unit,
    mode: options::RoundingMode,
) -> IsoDateTime {
    let (days, time) = round_time(dt.time, increment, unit, mode);
    IsoDateTime {
        date: dt.date.add_days(days),
        time,
    }
}
