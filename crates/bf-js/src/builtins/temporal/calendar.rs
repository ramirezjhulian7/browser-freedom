//! Los calendarios de Temporal además de ISO 8601 (ECMA-402 con la propuesta Intl era y
//! monthCode, §4.1; Temporal §12.2): `gregory`, `buddhist`, `japanese` y `roc` (el año y la
//! era cambian; los meses y los días son los de ISO), `coptic`, `ethiopic` y `ethioaa` (13
//! meses), `indian` (Saka), `islamic-civil` e `islamic-tbla` (tabulares), `persian` (la
//! regla aritmética de 33 años de ICU) y `hebrew` (el calendario aritmético con el molad).
//!
//! Las conversiones pasan por el número de día fijo (R.D., *Calendrical Calculations* de
//! Reingold y Dershowitz): el R.D. 1 es el 1 de enero del año 1 del calendario gregoriano
//! proléptico. `chinese`, `dangi` e `islamic-umalqura` necesitan tablas astronómicas y no
//! están (CanonicalizeCalendar da RangeError).
//!
//! Los algoritmos genéricos (NonISODateAdd, NonISODateUntil, BalanceNonISODate,
//! ConstrainMonthCode, MonthCodeToOrdinal, NonISOResolveFields,
//! NonISOMonthDayToISOReferenceDate) siguen el texto de la propuesta.

use super::duration::DateDuration;
use super::iso::{self, IsoDate};
use super::options::Unit;

/// El R.D. del 1970-01-01 (día 0 de `IsoDate::epoch_days`).
const RD_UNIX: i64 = 719_163;

fn rd_of(d: IsoDate) -> i64 {
    d.epoch_days() + RD_UNIX
}

fn iso_of_rd(rd: i64) -> IsoDate {
    IsoDate::from_epoch_days(rd - RD_UNIX)
}

fn fdiv(a: i64, b: i64) -> i64 {
    a.div_euclid(b)
}

/// Un calendario disponible.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub enum Cal {
    #[default]
    Iso,
    Buddhist,
    Coptic,
    Ethioaa,
    Ethiopic,
    Gregory,
    Hebrew,
    Indian,
    IslamicCivil,
    IslamicTbla,
    Japanese,
    Persian,
    Roc,
}

const IDS: [(Cal, &str); 13] = [
    (Cal::Iso, "iso8601"),
    (Cal::Buddhist, "buddhist"),
    (Cal::Coptic, "coptic"),
    (Cal::Ethioaa, "ethioaa"),
    (Cal::Ethiopic, "ethiopic"),
    (Cal::Gregory, "gregory"),
    (Cal::Hebrew, "hebrew"),
    (Cal::Indian, "indian"),
    (Cal::IslamicCivil, "islamic-civil"),
    (Cal::IslamicTbla, "islamic-tbla"),
    (Cal::Japanese, "japanese"),
    (Cal::Persian, "persian"),
    (Cal::Roc, "roc"),
];

/// Un código de mes (`M01`…`M13`, `M05L`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonthCode {
    pub num: u8,
    pub leap: bool,
}

impl MonthCode {
    pub const fn common(num: u8) -> MonthCode {
        MonthCode { num, leap: false }
    }

    /// ParseMonthCode (la sintaxis ya la comprobó ToMonthCode).
    pub fn parse(s: &str) -> Option<MonthCode> {
        let b = s.as_bytes();
        if !(b.len() == 3 || (b.len() == 4 && b[3] == b'L')) || b[0] != b'M' {
            return None;
        }
        if !b[1].is_ascii_digit() || !b[2].is_ascii_digit() {
            return None;
        }
        Some(MonthCode {
            num: (b[1] - b'0') * 10 + (b[2] - b'0'),
            leap: b.len() == 4,
        })
    }

    pub fn text(self) -> String {
        format!("M{:02}{}", self.num, if self.leap { "L" } else { "" })
    }
}

/// CalendarISOToDate: la fecha en el calendario (año aritmético y mes ordinal).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CalDate {
    pub year: i64,
    pub month: u8,
    pub code: MonthCode,
    pub day: u8,
}

// --- Copto y etíope (13 meses de 30 días y uno de 5 o 6) ---

const COPTIC_EPOCH: i64 = 103_605;
const ETHIOPIC_EPOCH: i64 = 2_796;

fn coptic_fixed(epoch: i64, y: i64, m: i64, d: i64) -> i64 {
    epoch - 1 + 365 * (y - 1) + fdiv(y, 4) + 30 * (m - 1) + d
}

fn coptic_from_fixed(epoch: i64, rd: i64) -> (i64, i64, i64) {
    let y = fdiv(4 * (rd - epoch) + 1463, 1461);
    let m = fdiv(rd - coptic_fixed(epoch, y, 1, 1), 30) + 1;
    let d = rd + 1 - coptic_fixed(epoch, y, m, 1);
    (y, m, d)
}

fn coptic_leap(y: i64) -> bool {
    y.rem_euclid(4) == 3
}

// --- Islámico tabular (civil: época del viernes; tbla: del jueves) ---

const ISLAMIC_CIVIL_EPOCH: i64 = 227_015;
const ISLAMIC_TBLA_EPOCH: i64 = 227_014;

fn islamic_fixed(epoch: i64, y: i64, m: i64, d: i64) -> i64 {
    d + 29 * (m - 1) + fdiv(6 * m - 1, 11) + (y - 1) * 354 + fdiv(3 + 11 * y, 30) + epoch - 1
}

fn islamic_from_fixed(epoch: i64, rd: i64) -> (i64, i64, i64) {
    let y = fdiv(30 * (rd - epoch) + 10646, 10631);
    let prior = rd - islamic_fixed(epoch, y, 1, 1);
    let m = fdiv(11 * prior + 330, 325);
    let d = rd - islamic_fixed(epoch, y, m, 1) + 1;
    (y, m, d)
}

fn islamic_leap(y: i64) -> bool {
    (14 + 11 * y).rem_euclid(30) < 11
}

// --- Persa (aritmético de ICU: 8 años bisiestos cada 33) ---

/// 1 Farvardin del año 1: el día juliano 1948320 de ICU (R.D. 226 895).
const PERSIAN_EPOCH: i64 = 226_895;
const PERSIAN_CUM: [i64; 12] = [0, 31, 62, 93, 124, 155, 186, 216, 246, 276, 306, 336];

fn persian_fixed(y: i64, m: i64, d: i64) -> i64 {
    PERSIAN_EPOCH - 1 + 365 * (y - 1) + fdiv(8 * y + 21, 33) + PERSIAN_CUM[(m - 1) as usize] + d
}

fn persian_from_fixed(rd: i64) -> (i64, i64, i64) {
    let since = rd - PERSIAN_EPOCH;
    let y = 1 + fdiv(33 * since + 3, 12053);
    let doy = rd - persian_fixed(y, 1, 1);
    let m0 = if doy < 216 { doy / 31 } else { (doy - 6) / 30 };
    let d = doy - PERSIAN_CUM[m0 as usize] + 1;
    (y, m0 + 1, d)
}

fn persian_leap(y: i64) -> bool {
    (25 * y + 11).rem_euclid(33) < 8
}

// --- Indio nacional (Saka): año gregoriano − 78, Chaitra 1 el 22 (21) de marzo ---

fn indian_start(gy: i64) -> IsoDate {
    IsoDate::new(gy, 3, if iso::is_leap_year(gy) { 21 } else { 22 })
}

// --- Hebreo (Calendrical Calculations, cap. 8) ---

const HEBREW_EPOCH: i64 = -1_373_427;

fn hebrew_leap(y: i64) -> bool {
    (7 * y + 1).rem_euclid(19) < 7
}

fn hebrew_elapsed(y: i64) -> i64 {
    let months = fdiv(235 * y - 234, 19);
    let parts = 12084 + 13753 * months;
    let days = 29 * months + fdiv(parts, 25920);
    if (3 * (days + 1)).rem_euclid(7) < 3 {
        days + 1
    } else {
        days
    }
}

fn hebrew_new_year(y: i64) -> i64 {
    let (ny0, ny1, ny2) = (
        hebrew_elapsed(y - 1),
        hebrew_elapsed(y),
        hebrew_elapsed(y + 1),
    );
    let correction = if ny2 - ny1 == 356 {
        2
    } else if ny1 - ny0 == 382 {
        1
    } else {
        0
    };
    HEBREW_EPOCH + ny1 + correction
}

fn hebrew_year_days(y: i64) -> i64 {
    hebrew_new_year(y + 1) - hebrew_new_year(y)
}

/// Los días del mes ordinal `m` (Tishrei = 1) del año `y`.
fn hebrew_days_in_month(y: i64, m: u8) -> u8 {
    let leap = hebrew_leap(y);
    // El mes "civil" sin el mes intercalado: 1 Tishrei … 5 Shevat, 6 Adar (I), 7 Adar II
    // (solo bisiesto), 8 Nisan … 13 Elul.
    let k = if leap || m < 6 { m } else { m + 1 };
    let len = hebrew_year_days(y);
    match k {
        1 | 5 | 6 | 8 | 10 | 12 => 30,
        2 => {
            if len == 355 || len == 385 {
                30
            } else {
                29
            }
        }
        3 => {
            if len == 353 || len == 383 {
                29
            } else {
                30
            }
        }
        _ => 29,
    }
}

impl Cal {
    pub fn id(self) -> &'static str {
        IDS.iter()
            .find(|(c, _)| *c == self)
            .map_or("iso8601", |(_, n)| n)
    }

    /// CanonicalizeCalendar: sin distinguir mayúsculas ASCII, con los alias de CLDR.
    pub fn from_id(id: &str) -> Option<Cal> {
        let lower = id.to_ascii_lowercase();
        let name = match lower.as_str() {
            "ethiopic-amete-alem" => "ethioaa",
            "islamicc" => "islamic-civil",
            s => s,
        };
        IDS.iter().find(|(_, n)| *n == name).map(|(c, _)| *c)
    }

    pub fn is_iso(self) -> bool {
        self == Cal::Iso
    }

    /// Los calendarios que son ISO con otro año (y otras eras).
    fn gregorian_offset(self) -> Option<i64> {
        match self {
            Cal::Iso | Cal::Gregory | Cal::Japanese => Some(0),
            Cal::Buddhist => Some(543),
            Cal::Roc => Some(-1911),
            _ => None,
        }
    }

    pub fn months_in_year(self, y: i64) -> u8 {
        match self {
            Cal::Coptic | Cal::Ethiopic | Cal::Ethioaa => 13,
            Cal::Hebrew if hebrew_leap(y) => 13,
            _ => 12,
        }
    }

    /// El año etíope (Amete Mihret) de un año aritmético.
    fn coptic_year(self, y: i64) -> (i64, i64) {
        match self {
            Cal::Coptic => (COPTIC_EPOCH, y),
            Cal::Ethiopic => (ETHIOPIC_EPOCH, y),
            _ => (ETHIOPIC_EPOCH, y - 5500),
        }
    }

    pub fn days_in_month(self, y: i64, m: u8) -> u8 {
        if let Some(off) = self.gregorian_offset() {
            return iso::days_in_month(y - off, i64::from(m)) as u8;
        }
        match self {
            Cal::Coptic | Cal::Ethiopic | Cal::Ethioaa => {
                if m < 13 {
                    30
                } else if coptic_leap(self.coptic_year(y).1) {
                    6
                } else {
                    5
                }
            }
            Cal::IslamicCivil | Cal::IslamicTbla => {
                if m % 2 == 1 || (m == 12 && islamic_leap(y)) {
                    30
                } else {
                    29
                }
            }
            Cal::Persian => match m {
                1..=6 => 31,
                7..=11 => 30,
                _ => {
                    if persian_leap(y) {
                        30
                    } else {
                        29
                    }
                }
            },
            Cal::Indian => match m {
                1 => {
                    if iso::is_leap_year(y + 78) {
                        31
                    } else {
                        30
                    }
                }
                2..=6 => 31,
                _ => 30,
            },
            Cal::Hebrew => hebrew_days_in_month(y, m),
            _ => unreachable!("calendario gregoriano"),
        }
    }

    pub fn days_in_year(self, y: i64) -> i64 {
        match self {
            Cal::Hebrew => hebrew_year_days(y),
            _ => (1..=self.months_in_year(y))
                .map(|m| i64::from(self.days_in_month(y, m)))
                .sum(),
        }
    }

    /// CalendarISOToDate.[[InLeapYear]].
    pub fn in_leap_year(self, y: i64) -> bool {
        match self {
            Cal::Hebrew => hebrew_leap(y),
            _ => self.days_in_year(y) > if self.is_islamic() { 354 } else { 365 },
        }
    }

    fn is_islamic(self) -> bool {
        matches!(self, Cal::IslamicCivil | Cal::IslamicTbla)
    }

    /// El código del mes ordinal `m` del año `y`.
    pub fn code_of(self, y: i64, m: u8) -> MonthCode {
        if self == Cal::Hebrew && hebrew_leap(y) && m >= 6 {
            if m == 6 {
                return MonthCode { num: 5, leap: true };
            }
            return MonthCode::common(m - 1);
        }
        MonthCode::common(m)
    }

    /// IsValidMonthCodeForCalendar.
    pub fn valid_code(self, c: MonthCode) -> bool {
        if !c.leap && (1..=12).contains(&c.num) {
            return true;
        }
        match self {
            Cal::Coptic | Cal::Ethiopic | Cal::Ethioaa => !c.leap && c.num == 13,
            Cal::Hebrew => c.leap && c.num == 5,
            _ => false,
        }
    }

    /// YearContainsMonthCode + MonthCodeToOrdinal: el mes ordinal, o None si el código no
    /// está en ese año.
    pub fn ordinal_of(self, y: i64, c: MonthCode) -> Option<u8> {
        if !self.valid_code(c) {
            return None;
        }
        if self == Cal::Hebrew {
            let leap = hebrew_leap(y);
            return match (c.leap, leap) {
                (true, true) => Some(6),
                (true, false) => None,
                (false, true) if c.num >= 6 => Some(c.num + 1),
                _ => Some(c.num),
            };
        }
        Some(c.num)
    }

    /// ConstrainMonthCode: `Err` si el código no está en el año y `reject`.
    pub fn constrain_code(self, y: i64, c: MonthCode, reject: bool) -> Result<MonthCode, ()> {
        if self.ordinal_of(y, c).is_some() {
            return Ok(c);
        }
        if reject {
            return Err(());
        }
        // Hebreo (skip-forward): M05L → M06.
        Ok(MonthCode::common(6))
    }

    /// CalendarISOToDate.
    pub fn to_cal(self, d: IsoDate) -> CalDate {
        if let Some(off) = self.gregorian_offset() {
            return CalDate {
                year: i64::from(d.year) + off,
                month: d.month,
                code: MonthCode::common(d.month),
                day: d.day,
            };
        }
        let rd = rd_of(d);
        let (y, m, day) = match self {
            Cal::Coptic => coptic_from_fixed(COPTIC_EPOCH, rd),
            Cal::Ethiopic => coptic_from_fixed(ETHIOPIC_EPOCH, rd),
            Cal::Ethioaa => {
                let (y, m, d) = coptic_from_fixed(ETHIOPIC_EPOCH, rd);
                (y + 5500, m, d)
            }
            Cal::IslamicCivil => islamic_from_fixed(ISLAMIC_CIVIL_EPOCH, rd),
            Cal::IslamicTbla => islamic_from_fixed(ISLAMIC_TBLA_EPOCH, rd),
            Cal::Persian => persian_from_fixed(rd),
            Cal::Indian => {
                let gy = i64::from(d.year);
                let (y, start) = if iso::compare_date(d, indian_start(gy)) < 0 {
                    (gy - 79, indian_start(gy - 1))
                } else {
                    (gy - 78, indian_start(gy))
                };
                self.month_day_from_offset(y, d.epoch_days() - start.epoch_days())
            }
            Cal::Hebrew => {
                // El mayor año desde `approx - 1` cuyo año nuevo no es posterior (CC 8.4).
                let approx = fdiv((rd - HEBREW_EPOCH) * 98_496, 35_975_351) + 1;
                let y = (approx - 1..=approx + 1)
                    .rev()
                    .find(|y| hebrew_new_year(*y) <= rd)
                    .unwrap_or(approx - 1);
                self.month_day_from_offset(y, rd - hebrew_new_year(y))
            }
            _ => unreachable!("calendario gregoriano"),
        };
        let month = m as u8;
        CalDate {
            year: y,
            month,
            code: self.code_of(y, month),
            day: day as u8,
        }
    }

    /// (año, mes, día) del día `offset` (desde 0) del año `y`.
    fn month_day_from_offset(self, y: i64, mut offset: i64) -> (i64, i64, i64) {
        let mut m = 1u8;
        loop {
            let dim = i64::from(self.days_in_month(y, m));
            if offset < dim || m == self.months_in_year(y) {
                return (y, i64::from(m), offset + 1);
            }
            offset -= dim;
            m += 1;
        }
    }

    /// CalendarIntegersToISO (la fecha ya es válida en el calendario).
    pub fn from_cal(self, y: i64, m: u8, d: u8) -> IsoDate {
        if let Some(off) = self.gregorian_offset() {
            return IsoDate::new(y - off, i64::from(m), i64::from(d));
        }
        let (m64, d64) = (i64::from(m), i64::from(d));
        let rd = match self {
            Cal::Coptic | Cal::Ethiopic | Cal::Ethioaa => {
                let (epoch, y) = self.coptic_year(y);
                coptic_fixed(epoch, y, m64, d64)
            }
            Cal::IslamicCivil => islamic_fixed(ISLAMIC_CIVIL_EPOCH, y, m64, d64),
            Cal::IslamicTbla => islamic_fixed(ISLAMIC_TBLA_EPOCH, y, m64, d64),
            Cal::Persian => persian_fixed(y, m64, d64),
            Cal::Indian => {
                let before: i64 = (1..m).map(|k| i64::from(self.days_in_month(y, k))).sum();
                rd_of(indian_start(y + 78)) + before + d64 - 1
            }
            Cal::Hebrew => {
                let before: i64 = (1..m).map(|k| i64::from(self.days_in_month(y, k))).sum();
                hebrew_new_year(y) + before + d64 - 1
            }
            _ => unreachable!("calendario gregoriano"),
        };
        iso_of_rd(rd)
    }

    /// El día del año (desde 1) de una fecha del calendario.
    pub fn day_of_year(self, c: &CalDate) -> i64 {
        (1..c.month)
            .map(|k| i64::from(self.days_in_month(c.year, k)))
            .sum::<i64>()
            + i64::from(c.day)
    }

    // --- Eras (Table 2) ---

    /// CalendarSupportsEra.
    pub fn has_eras(self) -> bool {
        !self.is_iso()
    }

    /// CalendarHasMidYearEras.
    pub fn has_mid_year_eras(self) -> bool {
        self == Cal::Japanese
    }

    /// CanonicalizeEraInCalendar.
    pub fn canonical_era(self, era: &str) -> Option<&'static str> {
        let eras: &[(&'static str, &[&str])] = match self {
            Cal::Buddhist => &[("be", &[])],
            Cal::Coptic | Cal::Hebrew => &[("am", &[])],
            Cal::Ethioaa => &[("aa", &[])],
            Cal::Ethiopic => &[("am", &[]), ("aa", &[])],
            Cal::Gregory => &[("ce", &["ad"]), ("bce", &["bc"])],
            Cal::Indian => &[("shaka", &[])],
            Cal::IslamicCivil | Cal::IslamicTbla => &[("ah", &[]), ("bh", &[])],
            Cal::Japanese => &[
                ("reiwa", &[]),
                ("heisei", &[]),
                ("showa", &[]),
                ("taisho", &[]),
                ("meiji", &[]),
                ("ce", &["ad"]),
                ("bce", &["bc"]),
            ],
            Cal::Persian => &[("ap", &[])],
            Cal::Roc => &[("roc", &[]), ("broc", &[])],
            Cal::Iso => &[],
        };
        eras.iter()
            .find(|(e, aliases)| *e == era || aliases.contains(&era))
            .map(|(e, _)| *e)
    }

    /// CalendarDateArithmeticYearForEraYear (la era ya es canónica).
    pub fn year_for_era(self, era: &str, era_year: i64) -> i64 {
        match (self, era) {
            (_, "bce" | "bh" | "broc") => 1 - era_year,
            (Cal::Ethiopic, "aa") => era_year - 5500,
            (Cal::Japanese, "reiwa") => 2018 + era_year,
            (Cal::Japanese, "heisei") => 1988 + era_year,
            (Cal::Japanese, "showa") => 1925 + era_year,
            (Cal::Japanese, "taisho") => 1911 + era_year,
            (Cal::Japanese, "meiji") => 1867 + era_year,
            _ => era_year,
        }
    }

    /// CalendarDateEra y CalendarDateEraYear.
    pub fn era(self, iso_date: IsoDate, c: &CalDate) -> Option<(&'static str, i64)> {
        let y = c.year;
        Some(match self {
            Cal::Iso => return None,
            Cal::Buddhist => ("be", y),
            Cal::Coptic | Cal::Hebrew => ("am", y),
            Cal::Ethioaa => ("aa", y),
            Cal::Ethiopic if y >= 1 => ("am", y),
            Cal::Ethiopic => ("aa", y + 5500),
            Cal::Indian => ("shaka", y),
            Cal::Persian => ("ap", y),
            Cal::IslamicCivil | Cal::IslamicTbla if y >= 1 => ("ah", y),
            Cal::IslamicCivil | Cal::IslamicTbla => ("bh", 1 - y),
            Cal::Roc if y >= 1 => ("roc", y),
            Cal::Roc => ("broc", 1 - y),
            Cal::Japanese => {
                // Los comienzos de las eras desde que Japón adoptó el calendario gregoriano
                // (Meiji 6 = 1873); antes, las eras del gregoriano.
                const ERAS: [(&str, (i32, u8, u8), i64); 5] = [
                    ("reiwa", (2019, 5, 1), 2018),
                    ("heisei", (1989, 1, 8), 1988),
                    ("showa", (1926, 12, 25), 1925),
                    ("taisho", (1912, 7, 30), 1911),
                    ("meiji", (1873, 1, 1), 1867),
                ];
                let key = (iso_date.year, iso_date.month, iso_date.day);
                match ERAS.iter().find(|(_, start, _)| key >= *start) {
                    Some((name, _, before)) => (*name, y - before),
                    None if y >= 1 => ("ce", y),
                    None => ("bce", 1 - y),
                }
            }
            Cal::Gregory if y >= 1 => ("ce", y),
            Cal::Gregory => ("bce", 1 - y),
        })
    }

    // --- Aritmética (propuesta Intl era/monthCode §4.1) ---

    /// BalanceNonISODate.
    pub fn balance(self, year: i64, month: i64, day: i64) -> (i64, u8, u8) {
        let (mut y, mut m) = (year, month);
        let mut miy = i64::from(self.months_in_year(y));
        while m <= 0 {
            y -= 1;
            miy = i64::from(self.months_in_year(y));
            m += miy;
        }
        while m > miy {
            m -= miy;
            y += 1;
            miy = i64::from(self.months_in_year(y));
        }
        let mut d = day;
        // Saltos grandes de días: años enteros primero (el resultado es el mismo).
        if d > 400 || d < -400 {
            let date = self.from_cal(y, m as u8, 1).add_days(d - 1);
            let c = self.to_cal(date);
            return (c.year, c.month, c.day);
        }
        let mut dim = i64::from(self.days_in_month(y, m as u8));
        while d <= 0 {
            m -= 1;
            if m == 0 {
                y -= 1;
                miy = i64::from(self.months_in_year(y));
                m = miy;
            }
            dim = i64::from(self.days_in_month(y, m as u8));
            d += dim;
        }
        while d > dim {
            d -= dim;
            m += 1;
            if m > miy {
                y += 1;
                miy = i64::from(self.months_in_year(y));
                m = 1;
            }
            dim = i64::from(self.days_in_month(y, m as u8));
        }
        (y, m as u8, d as u8)
    }

    /// NonISODateAdd. `Err(())`: RangeError (reject o fuera de rango).
    pub fn add(self, date: IsoDate, d: DateDuration, reject: bool) -> Result<IsoDate, ()> {
        let parts = self.to_cal(date);
        let y0 = parts.year + d.years;
        if !(-300_000..=300_000).contains(&y0) {
            return Err(());
        }
        let code = self.constrain_code(y0, parts.code, reject)?;
        let m0 = i64::from(self.ordinal_of(y0, code).expect("código constreñido"));
        let months = m0 + d.months + 1;
        if !(-4_000_000..=4_000_000).contains(&months) {
            return Err(());
        }
        let (ey, em, eday) = self.balance(y0, months, 0);
        let regulated = if parts.day <= eday {
            parts.day
        } else if reject {
            return Err(());
        } else {
            eday
        };
        let days = i64::from(regulated) + 7 * d.weeks + d.days;
        if days.abs() > 200_000_000 {
            return Err(());
        }
        let start = self.from_cal(ey, em, 1);
        let result = start.add_days(days - 1);
        if !iso::date_within_limits(result) {
            return Err(());
        }
        Ok(result)
    }

    /// NonISODateSurpasses con las partes ya calculadas.
    fn surpasses(self, sign: i64, parts: &CalDate, two: &CalDate, dd: DateDuration) -> bool {
        let y0 = parts.year + dd.years;
        if compare_surpasses_code(sign, y0, parts.code, parts.day, two) {
            return true;
        }
        let code = self
            .constrain_code(y0, parts.code, false)
            .expect("constrain");
        let m0 = i64::from(self.ordinal_of(y0, code).expect("código constreñido"));
        let (my, mm, _) = self.balance(y0, m0 + dd.months, 1);
        if compare_surpasses(sign, my, i64::from(mm), i64::from(parts.day), two) {
            return true;
        }
        if dd.weeks == 0 && dd.days == 0 {
            return false;
        }
        let (ey, em, eday) = self.balance(my, i64::from(mm) + 1, 0);
        let regulated = parts.day.min(eday);
        let target = self
            .from_cal(ey, em, 1)
            .add_days(i64::from(regulated) - 1 + 7 * dd.weeks + dd.days);
        let b = self.to_cal(target);
        compare_surpasses(sign, b.year, i64::from(b.month), i64::from(b.day), two)
    }

    /// NonISODateUntil. Cada componente es el mayor (en la dirección de `sign`) que no
    /// sobrepasa la otra fecha; como la condición es monótona, se busca por saltos
    /// exponenciales y bisección en vez de contar de a uno.
    pub fn until(self, one: IsoDate, two: IsoDate, largest: Unit) -> DateDuration {
        let sign = -i64::from(iso::compare_date(one, two));
        if sign == 0 {
            return DateDuration::default();
        }
        let parts = self.to_cal(one);
        let target = self.to_cal(two);
        let mut dd = DateDuration::default();
        let search = |dd: DateDuration, set: &dyn Fn(DateDuration, i64) -> DateDuration| {
            // El mayor k ≥ 0 con !surpasses(k · sign).
            let ok = |k: i64| !self.surpasses(sign, &parts, &target, set(dd, k * sign));
            if !ok(1) {
                return 0;
            }
            let (mut lo, mut hi) = (1i64, 2i64);
            while ok(hi) {
                lo = hi;
                hi *= 2;
            }
            while hi - lo > 1 {
                let mid = lo + (hi - lo) / 2;
                if ok(mid) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            lo * sign
        };
        if largest == Unit::Year {
            dd.years = search(dd, &|d, k| DateDuration { years: k, ..d });
        }
        if matches!(largest, Unit::Year | Unit::Month) {
            dd.months = search(dd, &|d, k| DateDuration { months: k, ..d });
        }
        if largest == Unit::Week {
            dd.weeks = search(dd, &|d, k| DateDuration { weeks: k, ..d });
        }
        dd.days = search(dd, &|d, k| DateDuration { days: k, ..d });
        dd
    }

    /// El máximo de días de un código de mes en cualquier año (sin año en un MonthDay).
    pub fn max_days_in_code(self, c: MonthCode) -> u8 {
        if self.gregorian_offset().is_some() {
            return iso::days_in_month(1972, i64::from(c.num)) as u8;
        }
        (1990..2030)
            .filter_map(|cy| {
                let y = self.to_cal(IsoDate::new(cy, 6, 1)).year;
                self.ordinal_of(y, c).map(|m| self.days_in_month(y, m))
            })
            .max()
            .unwrap_or(30)
    }

    /// El paso 7–8 de NonISOMonthDayToISOReferenceDate: la última fecha ISO de 1900–1972
    /// (o la primera de 1973–2035) con ese código de mes y ese día.
    pub fn month_day_reference(self, c: MonthCode, day: u8) -> Option<IsoDate> {
        if let Some(off) = self.gregorian_offset() {
            let _ = off;
            return Some(IsoDate::new(1972, i64::from(c.num), i64::from(day)));
        }
        let in_year = |iy: i64| -> Option<IsoDate> {
            let first = self.to_cal(IsoDate::new(iy, 1, 1)).year;
            let last = self.to_cal(IsoDate::new(iy, 12, 31)).year;
            (first..=last)
                .rev()
                .filter_map(|cy| {
                    let m = self.ordinal_of(cy, c)?;
                    if day > self.days_in_month(cy, m) {
                        return None;
                    }
                    let d = self.from_cal(cy, m, day);
                    (i64::from(d.year) == iy).then_some(d)
                })
                .max_by_key(|d| d.epoch_days())
        };
        (1900..=1972)
            .rev()
            .find_map(in_year)
            .or_else(|| (1973..=2035).find_map(in_year))
    }
}

/// CompareSurpasses con un código de mes.
fn compare_surpasses_code(sign: i64, year: i64, code: MonthCode, day: u8, two: &CalDate) -> bool {
    if year != two.year {
        return sign * (year - two.year) > 0;
    }
    if code != two.code {
        let key = |c: MonthCode| (c.num, c.leap);
        return sign * if key(code) > key(two.code) { 1 } else { -1 } > 0;
    }
    if day != two.day {
        return sign * (i64::from(day) - i64::from(two.day)) > 0;
    }
    false
}

/// CompareSurpasses con un mes ordinal.
fn compare_surpasses(sign: i64, year: i64, month: i64, day: i64, two: &CalDate) -> bool {
    if year != two.year {
        return sign * (year - two.year) > 0;
    }
    if month != i64::from(two.month) {
        return sign * (month - i64::from(two.month)) > 0;
    }
    if day != i64::from(two.day) {
        return sign * (day - i64::from(two.day)) > 0;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(cal: Cal) {
        // ~4000 años desde el 1000 a. C. (años negativos en todos los calendarios).
        let mut d = IsoDate::new(-1000, 1, 1);
        for _ in 0..15_000 {
            let c = cal.to_cal(d);
            assert!(
                c.month >= 1 && c.month <= cal.months_in_year(c.year),
                "{cal:?} {d:?}"
            );
            assert!(
                c.day >= 1 && c.day <= cal.days_in_month(c.year, c.month),
                "{cal:?} {d:?}"
            );
            assert_eq!(cal.from_cal(c.year, c.month, c.day), d, "{cal:?} {c:?}");
            d = d.add_days(97);
        }
    }

    #[test]
    fn calendars_roundtrip() {
        for (cal, _) in IDS {
            roundtrip(cal);
        }
    }

    #[test]
    fn known_dates() {
        let d = IsoDate::new(2024, 9, 11);
        // 1 Meskerem 2017 (etíope), 1 Thout 1741 (copto).
        let e = Cal::Ethiopic.to_cal(d);
        assert_eq!((e.year, e.month, e.day), (2017, 1, 1));
        let c = Cal::Coptic.to_cal(d);
        assert_eq!((c.year, c.month, c.day), (1741, 1, 1));
        // 1 Tishrei 5785 = 2024-10-03.
        let h = Cal::Hebrew.to_cal(IsoDate::new(2024, 10, 3));
        assert_eq!((h.year, h.month, h.day), (5785, 1, 1));
        // 1 Farvardin 1403 = 2024-03-20.
        let p = Cal::Persian.to_cal(IsoDate::new(2024, 3, 20));
        assert_eq!((p.year, p.month, p.day), (1403, 1, 1));
        // 1 Chaitra 1946 = 2024-03-21 (año bisiesto).
        let i = Cal::Indian.to_cal(IsoDate::new(2024, 3, 21));
        assert_eq!((i.year, i.month, i.day), (1946, 1, 1));
        // 1 Muharram 1446 tabular civil = 2024-07-08 (el astronómico es el 7).
        let m = Cal::IslamicCivil.to_cal(IsoDate::new(2024, 7, 8));
        assert_eq!((m.year, m.month, m.day), (1446, 1, 1));
        let t = Cal::IslamicTbla.to_cal(IsoDate::new(2024, 7, 7));
        assert_eq!((t.year, t.month, t.day), (1446, 1, 1));
        // Adar I de 5784 (bisiesto) es M05L.
        let a = Cal::Hebrew.to_cal(IsoDate::new(2024, 2, 15));
        assert_eq!(a.code, MonthCode { num: 5, leap: true });
    }
}
