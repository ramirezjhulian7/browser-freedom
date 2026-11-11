//! Las opciones de Temporal (GetOptionsObject, GetOption, GetTemporalUnitValuedOption,
//! GetRoundingIncrementOption, GetRoundingModeOption…), las unidades (Table 21), los modos
//! de redondeo y el redondeo exacto en enteros (RoundNumberToIncrement,
//! ApplyUnsignedRoundingMode).

use crate::abstract_ops::conversion::Hint;
use crate::heap::Gc;
use crate::object::Object;
use crate::value::{PropertyKey, Value};
use crate::vm::{Agent, JsResult};

/// Las unidades de Table 21, de la mayor a la menor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Unit {
    Year,
    Month,
    Week,
    Day,
    Hour,
    Minute,
    Second,
    Millisecond,
    Microsecond,
    Nanosecond,
}

pub const UNITS: [Unit; 10] = [
    Unit::Year,
    Unit::Month,
    Unit::Week,
    Unit::Day,
    Unit::Hour,
    Unit::Minute,
    Unit::Second,
    Unit::Millisecond,
    Unit::Microsecond,
    Unit::Nanosecond,
];

impl Unit {
    pub fn singular(self) -> &'static str {
        match self {
            Unit::Year => "year",
            Unit::Month => "month",
            Unit::Week => "week",
            Unit::Day => "day",
            Unit::Hour => "hour",
            Unit::Minute => "minute",
            Unit::Second => "second",
            Unit::Millisecond => "millisecond",
            Unit::Microsecond => "microsecond",
            Unit::Nanosecond => "nanosecond",
        }
    }

    pub fn plural(self) -> &'static str {
        match self {
            Unit::Year => "years",
            Unit::Month => "months",
            Unit::Week => "weeks",
            Unit::Day => "days",
            Unit::Hour => "hours",
            Unit::Minute => "minutes",
            Unit::Second => "seconds",
            Unit::Millisecond => "milliseconds",
            Unit::Microsecond => "microseconds",
            Unit::Nanosecond => "nanoseconds",
        }
    }

    /// "Length in Nanoseconds" (el día cuenta 24 horas).
    pub fn nanos(self) -> i128 {
        match self {
            Unit::Day => 86_400_000_000_000,
            Unit::Hour => 3_600_000_000_000,
            Unit::Minute => 60_000_000_000,
            Unit::Second => 1_000_000_000,
            Unit::Millisecond => 1_000_000,
            Unit::Microsecond => 1000,
            Unit::Nanosecond => 1,
            _ => 0,
        }
    }

    /// IsCalendarUnit.
    pub fn is_calendar(self) -> bool {
        matches!(self, Unit::Year | Unit::Month | Unit::Week)
    }

    /// TemporalUnitCategory es date.
    pub fn is_date(self) -> bool {
        self <= Unit::Day
    }

    /// LargerOfTwoTemporalUnits.
    pub fn larger(self, other: Unit) -> Unit {
        self.min(other)
    }

    /// MaximumTemporalDurationRoundingIncrement (None: unset).
    pub fn max_increment(self) -> Option<i128> {
        match self {
            Unit::Year | Unit::Month | Unit::Week | Unit::Day => None,
            Unit::Hour => Some(24),
            Unit::Minute | Unit::Second => Some(60),
            _ => Some(1000),
        }
    }
}

/// Un valor de GetTemporalUnitValuedOption.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitOption {
    Unset,
    Auto,
    Unit(Unit),
}

/// Los grupos de ValidateTemporalUnitValue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitGroup {
    Date,
    Time,
    DateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundingMode {
    Ceil,
    Floor,
    Expand,
    Trunc,
    HalfCeil,
    HalfFloor,
    HalfExpand,
    HalfTrunc,
    HalfEven,
}

const ROUNDING_MODES: [(&str, RoundingMode); 9] = [
    ("ceil", RoundingMode::Ceil),
    ("floor", RoundingMode::Floor),
    ("expand", RoundingMode::Expand),
    ("trunc", RoundingMode::Trunc),
    ("halfCeil", RoundingMode::HalfCeil),
    ("halfFloor", RoundingMode::HalfFloor),
    ("halfExpand", RoundingMode::HalfExpand),
    ("halfTrunc", RoundingMode::HalfTrunc),
    ("halfEven", RoundingMode::HalfEven),
];

impl RoundingMode {
    /// NegateRoundingMode.
    pub fn negate(self) -> RoundingMode {
        match self {
            RoundingMode::Ceil => RoundingMode::Floor,
            RoundingMode::Floor => RoundingMode::Ceil,
            RoundingMode::HalfCeil => RoundingMode::HalfFloor,
            RoundingMode::HalfFloor => RoundingMode::HalfCeil,
            m => m,
        }
    }
}

/// Unsigned Rounding Mode (Table 22).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsigned {
    Infinity,
    Zero,
    HalfInfinity,
    HalfZero,
    HalfEven,
}

/// GetUnsignedRoundingMode.
pub fn unsigned_mode(mode: RoundingMode, negative: bool) -> Unsigned {
    use RoundingMode as R;
    match (mode, negative) {
        (R::Ceil, false) | (R::Floor, true) | (R::Expand, _) => Unsigned::Infinity,
        (R::Ceil, true) | (R::Floor, false) | (R::Trunc, _) => Unsigned::Zero,
        (R::HalfCeil, false) | (R::HalfFloor, true) | (R::HalfExpand, _) => Unsigned::HalfInfinity,
        (R::HalfCeil, true) | (R::HalfFloor, false) | (R::HalfTrunc, _) => Unsigned::HalfZero,
        (R::HalfEven, _) => Unsigned::HalfEven,
    }
}

/// ApplyUnsignedRoundingMode para x = r1 + num/den (0 ≤ num ≤ den, den > 0) entre r1 y
/// r2 = r1 + 1 (en unidades de incremento): ¿se redondea hacia r2? `r1_even`: si r1 es par
/// (half-even).
pub fn rounds_up(num: i128, den: i128, mode: Unsigned, r1_even: bool) -> bool {
    if num == 0 {
        return false;
    }
    if num == den {
        return true;
    }
    match mode {
        Unsigned::Zero => false,
        Unsigned::Infinity => true,
        _ => {
            let twice = 2 * num;
            if twice < den {
                false
            } else if twice > den {
                true
            } else {
                match mode {
                    Unsigned::HalfZero => false,
                    Unsigned::HalfInfinity => true,
                    _ => !r1_even,
                }
            }
        }
    }
}

/// RoundNumberToIncrement para enteros exactos.
pub fn round_to_increment(x: i128, increment: i128, mode: RoundingMode) -> i128 {
    let q = x.div_euclid(increment);
    let r = x.rem_euclid(increment);
    // x = q·inc + r, con 0 ≤ r < inc: entre q·inc y (q+1)·inc.
    if r == 0 {
        return x;
    }
    let negative = x < 0;
    let um = unsigned_mode(mode, negative);
    // En valor absoluto: |x| está entre |r1| y |r2|.
    let (r1_abs, frac_num) = if negative {
        // x = -(|q+1|·inc) + r → |x| = |q+1|·inc - r: entre (|q|-1) y |q|.
        (-(q + 1), increment - r)
    } else {
        (q, r)
    };
    let up = rounds_up(frac_num, increment, um, r1_abs % 2 == 0);
    let mag = if up { r1_abs + 1 } else { r1_abs };
    let mag = mag * increment;
    if negative { -mag } else { mag }
}

/// RoundNumberToIncrementAsIfPositive (RoundTemporalInstant).
pub fn round_as_if_positive(x: i128, increment: i128, mode: RoundingMode) -> i128 {
    let q = x.div_euclid(increment);
    let r = x.rem_euclid(increment);
    if r == 0 {
        return x;
    }
    let up = rounds_up(
        r,
        increment,
        unsigned_mode(mode, false),
        q.rem_euclid(2) == 0,
    );
    (if up { q + 1 } else { q }) * increment
}

/// Un cociente exacto como Number con redondeo correcto (TotalTimeDuration y los totales).
pub fn div_to_f64(n: i128, d: i128) -> f64 {
    if n == 0 {
        return 0.0;
    }
    let negative = (n < 0) != (d < 0);
    let a = n.unsigned_abs();
    let d = d.unsigned_abs();
    let q = a / d;
    let mut r = a % d;
    let bits = |x: u128| 128 - x.leading_zeros();
    let v = if bits(q) >= 66 {
        (q | u128::from(r != 0)) as f64
    } else {
        let mut m = q;
        let mut e = 0i32;
        while bits(m) < 66 {
            m <<= 1;
            // r < d: r·2 cabe si d < 2¹²⁷.
            r <<= 1;
            if r >= d {
                m |= 1;
                r -= d;
            }
            e += 1;
        }
        let m = m | u128::from(r != 0);
        (m as f64) * 2f64.powi(-e)
    };
    if negative { -v } else { v }
}

/// Un entero de f64 (exacto) como i128.
pub fn f64_to_i128(x: f64) -> i128 {
    x as i128
}

/// El texto decimal de un Number entero (sin exponente).
pub fn integer_string(x: f64) -> String {
    format!("{}", x.abs() as i128)
}

// --- Lectura de opciones ---

impl Agent {
    /// GetOptionsObject.
    pub fn temporal_options_object(&mut self, options: &Value) -> JsResult<Gc<Object>> {
        match options {
            Value::Undefined => Ok(self.new_object(None)),
            Value::Object(o) => Ok(*o),
            _ => self.type_error("options must be an object or undefined"),
        }
    }

    /// GetOption con tipo string: None si es undefined.
    pub fn temporal_string_option(
        &mut self,
        options: Gc<Object>,
        name: &str,
        allowed: &[&str],
    ) -> JsResult<Option<String>> {
        let v = self.get(options, &PropertyKey::from(name), Value::Object(options))?;
        if v.is_undefined() {
            return Ok(None);
        }
        let s = self.to_string(&v)?.to_string_lossy();
        if !allowed.is_empty() && !allowed.contains(&s.as_str()) {
            return self.range_error(&format!("{s} is not a valid value for option {name}"));
        }
        Ok(Some(s))
    }

    /// GetTemporalUnitValuedOption (`required`: sin valor es RangeError).
    pub fn temporal_unit_option(
        &mut self,
        options: Gc<Object>,
        name: &str,
        required: bool,
    ) -> JsResult<UnitOption> {
        let v = self.get(options, &PropertyKey::from(name), Value::Object(options))?;
        if v.is_undefined() {
            if required {
                return self.range_error(&format!("{name} is required"));
            }
            return Ok(UnitOption::Unset);
        }
        let s = self.to_string(&v)?.to_string_lossy();
        if s == "auto" {
            return Ok(UnitOption::Auto);
        }
        for u in UNITS {
            if s == u.singular() || s == u.plural() {
                return Ok(UnitOption::Unit(u));
            }
        }
        self.range_error(&format!("{s} is not a valid value for {name}"))
    }

    /// ValidateTemporalUnitValue (`auto_ok`: "auto" está entre los valores extra;
    /// `extra_day`: "day" también vale).
    pub fn temporal_validate_unit(
        &mut self,
        value: UnitOption,
        group: UnitGroup,
        auto_ok: bool,
        extra_day: bool,
    ) -> JsResult<()> {
        match value {
            UnitOption::Unset => Ok(()),
            UnitOption::Auto => {
                if auto_ok {
                    Ok(())
                } else {
                    self.range_error("auto is not a valid unit here")
                }
            }
            UnitOption::Unit(u) => {
                if extra_day && u == Unit::Day {
                    return Ok(());
                }
                let ok = if u.is_date() {
                    matches!(group, UnitGroup::Date | UnitGroup::DateTime)
                } else {
                    matches!(group, UnitGroup::Time | UnitGroup::DateTime)
                };
                if ok {
                    Ok(())
                } else {
                    self.range_error(&format!("{} is not a valid unit here", u.singular()))
                }
            }
        }
    }

    /// GetRoundingIncrementOption.
    pub fn temporal_rounding_increment(&mut self, options: Gc<Object>) -> JsResult<i128> {
        let v = self.get(
            options,
            &PropertyKey::from("roundingIncrement"),
            Value::Object(options),
        )?;
        if v.is_undefined() {
            return Ok(1);
        }
        let n = self.to_integer_with_truncation(&v)?;
        if !(1.0..=1e9).contains(&n) {
            return self.range_error("roundingIncrement must be between 1 and 1e9");
        }
        Ok(n as i128)
    }

    /// ValidateTemporalRoundingIncrement.
    pub fn temporal_validate_increment(
        &mut self,
        increment: i128,
        dividend: i128,
        inclusive: bool,
    ) -> JsResult<()> {
        let max = if inclusive { dividend } else { dividend - 1 };
        if increment > max {
            return self.range_error("roundingIncrement is out of range");
        }
        if dividend % increment != 0 {
            return self.range_error("roundingIncrement does not divide evenly");
        }
        Ok(())
    }

    /// GetRoundingModeOption.
    pub fn temporal_rounding_mode(
        &mut self,
        options: Gc<Object>,
        fallback: RoundingMode,
    ) -> JsResult<RoundingMode> {
        let names: Vec<&str> = ROUNDING_MODES.iter().map(|(n, _)| *n).collect();
        let s = self.temporal_string_option(options, "roundingMode", &names)?;
        Ok(match s {
            None => fallback,
            Some(s) => {
                ROUNDING_MODES
                    .iter()
                    .find(|(n, _)| *n == s)
                    .expect("validado")
                    .1
            }
        })
    }

    /// GetTemporalOverflowOption: `true` si es reject.
    pub fn temporal_overflow(&mut self, options: Gc<Object>) -> JsResult<bool> {
        let s = self.temporal_string_option(options, "overflow", &["constrain", "reject"])?;
        Ok(s.as_deref() == Some("reject"))
    }

    /// GetTemporalOverflowOption sobre el argumento `options` crudo.
    pub fn temporal_overflow_of(&mut self, options: &Value) -> JsResult<bool> {
        let o = self.temporal_options_object(options)?;
        self.temporal_overflow(o)
    }

    /// GetTemporalFractionalSecondDigitsOption: None es auto.
    pub fn temporal_fractional_digits(&mut self, options: Gc<Object>) -> JsResult<Option<u8>> {
        let v = self.get(
            options,
            &PropertyKey::from("fractionalSecondDigits"),
            Value::Object(options),
        )?;
        if v.is_undefined() {
            return Ok(None);
        }
        let Value::Number(n) = v else {
            let s = self.to_string(&v)?.to_string_lossy();
            if s != "auto" {
                return self.range_error("fractionalSecondDigits must be auto or 0-9");
            }
            return Ok(None);
        };
        if !n.is_finite() {
            return self.range_error("fractionalSecondDigits must be auto or 0-9");
        }
        let d = n.floor();
        if !(0.0..=9.0).contains(&d) {
            return self.range_error("fractionalSecondDigits must be auto or 0-9");
        }
        Ok(Some(d as u8))
    }

    /// ToIntegerWithTruncation.
    pub fn to_integer_with_truncation(&mut self, v: &Value) -> JsResult<f64> {
        let n = self.to_number(v)?;
        if !n.is_finite() {
            return self.range_error("value must be a finite number");
        }
        let t = n.trunc();
        Ok(if t == 0.0 { 0.0 } else { t })
    }

    /// ToPositiveIntegerWithTruncation.
    pub fn to_positive_integer_with_truncation(&mut self, v: &Value) -> JsResult<f64> {
        let n = self.to_integer_with_truncation(v)?;
        if n <= 0.0 {
            return self.range_error("value must be a positive integer");
        }
        Ok(n)
    }

    /// ToIntegerIfIntegral.
    pub fn to_integer_if_integral(&mut self, v: &Value) -> JsResult<f64> {
        let n = self.to_number(v)?;
        if !n.is_finite() || n.trunc() != n {
            return self.range_error("value must be an integer");
        }
        Ok(if n == 0.0 { 0.0 } else { n })
    }

    /// ToPrimitive(v, string) que tiene que dar un String (ToMonthCode, ToOffsetString…).
    pub fn temporal_primitive_string(&mut self, v: &Value, what: &str) -> JsResult<String> {
        let p = self.to_primitive(v, Hint::String)?;
        match p {
            Value::String(s) => Ok(s.to_string_lossy()),
            _ => self.type_error(&format!("{what} must be a string")),
        }
    }
}
