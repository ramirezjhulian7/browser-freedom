//! Valores matemáticos exactos para Intl.NumberFormat (ECMA-402 §16.5): el valor exacto de un
//! Number (ℝ(x), no su representación más corta), los strings de Intl mathematical value
//! (§16.5.16 ToIntlMathematicalValue) y los BigInt, con ToRawPrecision (§16.5.8), ToRawFixed
//! (§16.5.9) y los modos de redondeo (§16.5.20–§16.5.21).

use std::cmp::Ordering;

use crate::bigint::BigInt;

/// Un decimal finito: `digits × 10^exp` (dígitos sin ceros a la izquierda; vacío es cero).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decimal {
    pub negative: bool,
    pub digits: Vec<u8>,
    pub exp: i64,
}

/// Un Intl mathematical value: un decimal, ±∞ o NaN (y −0 en `negative`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MathValue {
    Finite(Decimal),
    Infinity(bool),
    NaN,
}

/// Los modos de redondeo (Table 15).
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

impl RoundingMode {
    pub fn parse(s: &str) -> RoundingMode {
        match s {
            "ceil" => RoundingMode::Ceil,
            "floor" => RoundingMode::Floor,
            "expand" => RoundingMode::Expand,
            "trunc" => RoundingMode::Trunc,
            "halfCeil" => RoundingMode::HalfCeil,
            "halfFloor" => RoundingMode::HalfFloor,
            "halfTrunc" => RoundingMode::HalfTrunc,
            "halfEven" => RoundingMode::HalfEven,
            _ => RoundingMode::HalfExpand,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            RoundingMode::Ceil => "ceil",
            RoundingMode::Floor => "floor",
            RoundingMode::Expand => "expand",
            RoundingMode::Trunc => "trunc",
            RoundingMode::HalfCeil => "halfCeil",
            RoundingMode::HalfFloor => "halfFloor",
            RoundingMode::HalfExpand => "halfExpand",
            RoundingMode::HalfTrunc => "halfTrunc",
            RoundingMode::HalfEven => "halfEven",
        }
    }
}

/// GetUnsignedRoundingMode (§16.5.20).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unsigned {
    Infinity,
    Zero,
    HalfInfinity,
    HalfZero,
    HalfEven,
}

fn unsigned(mode: RoundingMode, negative: bool) -> Unsigned {
    use RoundingMode as R;
    match (mode, negative) {
        (R::Ceil, false) | (R::Floor, true) | (R::Expand, _) => Unsigned::Infinity,
        (R::Ceil, true) | (R::Floor, false) | (R::Trunc, _) => Unsigned::Zero,
        (R::HalfCeil, false) | (R::HalfFloor, true) | (R::HalfExpand, _) => Unsigned::HalfInfinity,
        (R::HalfCeil, true) | (R::HalfFloor, false) | (R::HalfTrunc, _) => Unsigned::HalfZero,
        (R::HalfEven, _) => Unsigned::HalfEven,
    }
}

impl Decimal {
    pub fn zero(negative: bool) -> Decimal {
        Decimal {
            negative,
            digits: Vec::new(),
            exp: 0,
        }
    }

    pub fn is_zero(&self) -> bool {
        self.digits.is_empty()
    }

    fn normalize(mut self) -> Decimal {
        let lead = self.digits.iter().take_while(|&&d| d == 0).count();
        self.digits.drain(..lead);
        while self.digits.last() == Some(&0) {
            self.digits.pop();
            self.exp += 1;
        }
        if self.digits.is_empty() {
            self.exp = 0;
        }
        self
    }

    fn from_digit_str(negative: bool, s: &str, exp: i64) -> Decimal {
        Decimal {
            negative,
            digits: s.bytes().map(|b| b - b'0').collect(),
            exp,
        }
        .normalize()
    }

    /// El valor exacto de un Number finito.
    pub fn from_f64(x: f64) -> Decimal {
        let negative = x.is_sign_negative();
        let a = x.abs();
        if a == 0.0 {
            return Decimal::zero(negative);
        }
        let bits = a.to_bits();
        let raw_exp = ((bits >> 52) & 0x7FF) as i64;
        let frac = bits & ((1u64 << 52) - 1);
        let (m, e) = if raw_exp == 0 {
            (frac, -1074)
        } else {
            (frac | (1 << 52), raw_exp - 1075)
        };
        let mb = BigInt::from_u64(m);
        if e >= 0 {
            let v = mb.mul(
                &BigInt::from_u64(2)
                    .pow(&BigInt::from_u64(e as u64))
                    .unwrap_or_default(),
            );
            return Decimal::from_digit_str(negative, &v.to_string_radix(10), 0);
        }
        // m × 2^e = m × 5^(−e) × 10^e.
        let five = BigInt::from_u64(5)
            .pow(&BigInt::from_u64((-e) as u64))
            .unwrap_or_default();
        let v = mb.mul(&five);
        Decimal::from_digit_str(negative, &v.to_string_radix(10), e)
    }

    pub fn from_bigint(b: &BigInt) -> Decimal {
        let s = b.to_string_radix(10);
        let (neg, digits) = match s.strip_prefix('-') {
            Some(d) => (true, d.to_owned()),
            None => (false, s),
        };
        Decimal::from_digit_str(neg, &digits, 0)
    }

    /// El exponente del dígito más significativo (⌊log10 |x|⌋); 0 para cero.
    pub fn magnitude(&self) -> i64 {
        if self.is_zero() {
            0
        } else {
            self.digits.len() as i64 - 1 + self.exp
        }
    }

    /// `x × 10^n`.
    pub fn scale(&self, n: i64) -> Decimal {
        let mut d = self.clone();
        if !d.is_zero() {
            d.exp += n;
        }
        d
    }

    /// `x × k` para un entero pequeño.
    pub fn mul_small(&self, k: u64) -> Decimal {
        let mut out: Vec<u8> = Vec::with_capacity(self.digits.len() + 20);
        let mut carry: u64 = 0;
        for &d in self.digits.iter().rev() {
            let v = u64::from(d) * k + carry;
            out.push((v % 10) as u8);
            carry = v / 10;
        }
        while carry > 0 {
            out.push((carry % 10) as u8);
            carry /= 10;
        }
        out.reverse();
        Decimal {
            negative: self.negative,
            digits: out,
            exp: self.exp,
        }
        .normalize()
    }

    /// Compara los valores absolutos.
    pub fn cmp_abs(&self, other: &Decimal) -> Ordering {
        match (self.is_zero(), other.is_zero()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            _ => {}
        }
        let (ma, mb) = (self.magnitude(), other.magnitude());
        if ma != mb {
            return ma.cmp(&mb);
        }
        let n = self.digits.len().max(other.digits.len());
        for i in 0..n {
            let a = self.digits.get(i).copied().unwrap_or(0);
            let b = other.digits.get(i).copied().unwrap_or(0);
            if a != b {
                return a.cmp(&b);
            }
        }
        Ordering::Equal
    }

    /// Compara con signo (−0 = 0).
    pub fn cmp_signed(&self, other: &Decimal) -> Ordering {
        let an = self.negative && !self.is_zero();
        let bn = other.negative && !other.is_zero();
        match (an, bn) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => self.cmp_abs(other),
            (true, true) => other.cmp_abs(self),
        }
    }

    /// ⌊|x|⌋ y si hay parte fraccionaria, más la comparación de la fracción con ½.
    fn split_integer(&self) -> (Vec<u8>, Ordering, bool) {
        if self.exp >= 0 {
            let mut d = self.digits.clone();
            d.extend(std::iter::repeat_n(0, self.exp as usize));
            return (d, Ordering::Less, false);
        }
        let k = (-self.exp) as usize;
        let len = self.digits.len();
        let (int, frac): (Vec<u8>, Vec<u8>) = if k >= len {
            let mut f = vec![0; k - len];
            f.extend_from_slice(&self.digits);
            (Vec::new(), f)
        } else {
            (
                self.digits[..len - k].to_vec(),
                self.digits[len - k..].to_vec(),
            )
        };
        let nonzero = frac.iter().any(|&d| d != 0);
        let half = match frac.first() {
            None => Ordering::Less,
            Some(&f) if f > 5 => Ordering::Greater,
            Some(&f) if f < 5 => Ordering::Less,
            Some(_) => {
                if frac[1..].iter().any(|&d| d != 0) {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            }
        };
        (int, half, nonzero)
    }

    /// Redondea `|x|` a un entero con el modo (ApplyUnsignedRoundingMode, §16.5.21; el signo
    /// de `x` elige el modo sin signo).
    pub fn round_integer(&self, mode: RoundingMode) -> Vec<u8> {
        let (mut int, half, inexact) = self.split_integer();
        if !inexact {
            return trim_leading(int);
        }
        let up = match unsigned(mode, self.negative) {
            Unsigned::Zero => false,
            Unsigned::Infinity => true,
            Unsigned::HalfZero => half == Ordering::Greater,
            Unsigned::HalfInfinity => half != Ordering::Less,
            Unsigned::HalfEven => match half {
                Ordering::Greater => true,
                Ordering::Less => false,
                Ordering::Equal => int.last().is_some_and(|d| d % 2 == 1),
            },
        };
        if up {
            increment(&mut int);
        }
        trim_leading(int)
    }
}

fn trim_leading(mut v: Vec<u8>) -> Vec<u8> {
    let lead = v.iter().take_while(|&&d| d == 0).count();
    v.drain(..lead);
    v
}

fn increment(v: &mut Vec<u8>) {
    let mut i = v.len();
    loop {
        if i == 0 {
            v.insert(0, 1);
            return;
        }
        i -= 1;
        if v[i] == 9 {
            v[i] = 0;
        } else {
            v[i] += 1;
            return;
        }
    }
}

fn digits_to_string(v: &[u8]) -> String {
    if v.is_empty() {
        return "0".into();
    }
    v.iter().map(|d| (b'0' + d) as char).collect()
}

/// El resultado de ToRawPrecision / ToRawFixed (Record con [[FormattedString]],
/// [[RoundedNumber]], [[IntegerDigitsCount]] y [[RoundingMagnitude]]).
#[derive(Debug, Clone)]
pub struct RawFormat {
    pub string: String,
    pub rounded: Decimal,
    pub integer_digits: i64,
    pub rounding_magnitude: i64,
}

/// ToRawPrecision ( x, minPrecision, maxPrecision, unsignedRoundingMode ) (§16.5.8).
pub fn to_raw_precision(x: &Decimal, min_p: u32, max_p: u32, mode: RoundingMode) -> RawFormat {
    let p = i64::from(max_p);
    let (mut m, e) = if x.is_zero() {
        ("0".repeat(p as usize), 0)
    } else {
        let e0 = x.magnitude();
        // n = round(x / 10^(e − p + 1)).
        let scaled = x.scale(-(e0 - p + 1));
        let mut n = scaled.round_integer(mode);
        let mut e = e0;
        if n.len() as i64 > p {
            // n = 10^p: e + 1 y n = 10^(p−1).
            e += 1;
            n.truncate(p as usize);
        }
        while (n.len() as i64) < p {
            n.insert(0, 0);
        }
        (digits_to_string(&n), e)
    };
    let rounding_magnitude = e - p + 1;
    let rounded = Decimal::from_digit_str(x.negative, &m, rounding_magnitude);
    let int_digits;
    if e >= p - 1 {
        m.push_str(&"0".repeat((e - p + 1) as usize));
        int_digits = e + 1;
    } else if e >= 0 {
        m.insert((e + 1) as usize, '.');
        int_digits = e + 1;
    } else {
        m = format!("0.{}{}", "0".repeat((-e - 1) as usize), m);
        int_digits = 1;
    }
    if m.contains('.') && max_p > min_p {
        let mut cut = max_p - min_p;
        while cut > 0 && m.ends_with('0') {
            m.pop();
            cut -= 1;
        }
        if m.ends_with('.') {
            m.pop();
        }
    }
    RawFormat {
        string: m,
        rounded,
        integer_digits: int_digits,
        rounding_magnitude,
    }
}

/// ToRawFixed ( x, minFraction, maxFraction, roundingIncrement, unsignedRoundingMode )
/// (§16.5.9).
pub fn to_raw_fixed(
    x: &Decimal,
    min_f: u32,
    max_f: u32,
    increment_by: u32,
    mode: RoundingMode,
) -> RawFormat {
    let f = i64::from(max_f);
    // n = round(x × 10^f / increment) × increment. El incremento es 2^a·5^b: dividir es
    // multiplicar por 5^a·2^b y desplazar a + b lugares.
    let mut inc = increment_by.max(1);
    let (mut a, mut b) = (0i64, 0i64);
    while inc.is_multiple_of(2) {
        inc /= 2;
        a += 1;
    }
    while inc.is_multiple_of(5) {
        inc /= 5;
        b += 1;
    }
    let inv = 5u64.pow(a as u32) * 2u64.pow(b as u32);
    let q = x.scale(f).mul_small(inv).scale(-(a + b));
    let q = Decimal {
        negative: x.negative,
        ..q
    };
    let n_digits = q.round_integer(mode);
    let n = Decimal::from_digit_str(false, &digits_to_string(&n_digits), 0)
        .mul_small(u64::from(increment_by.max(1)));
    let mut m = if n.is_zero() {
        "0".to_owned()
    } else {
        let mut s = digits_to_string(&n.digits);
        s.push_str(&"0".repeat(n.exp.max(0) as usize));
        s
    };
    let rounded = Decimal::from_digit_str(x.negative, &m, -f);
    let int_digits;
    if f != 0 {
        let mut k = m.len() as i64;
        if k <= f {
            m = format!("{}{}", "0".repeat((f + 1 - k) as usize), m);
            k = f + 1;
        }
        let split = (k - f) as usize;
        m = format!("{}.{}", &m[..split], &m[split..]);
        int_digits = split as i64;
    } else {
        int_digits = m.len() as i64;
    }
    let mut cut = max_f.saturating_sub(min_f);
    while cut > 0 && m.ends_with('0') && m.contains('.') {
        m.pop();
        cut -= 1;
    }
    if m.ends_with('.') {
        m.pop();
    }
    RawFormat {
        string: m,
        rounded,
        integer_digits: int_digits,
        rounding_magnitude: -f,
    }
}

/// StringToNumber sobre el string de un Intl mathematical value (§16.5.16): el valor exacto.
pub fn parse_str(s: &str) -> MathValue {
    let t = crate::abstract_ops::conversion::trim_js_whitespace(s);
    if t.is_empty() {
        return MathValue::Finite(Decimal::zero(false));
    }
    let lower = t.to_ascii_lowercase();
    for (p, radix) in [("0x", 16), ("0o", 8), ("0b", 2)] {
        if let Some(rest) = lower.strip_prefix(p) {
            return match BigInt::from_digits(rest, radix) {
                Some(b) => MathValue::Finite(Decimal::from_bigint(&b)),
                None => MathValue::NaN,
            };
        }
    }
    let (negative, body) = match t.as_bytes()[0] {
        b'-' => (true, &t[1..]),
        b'+' => (false, &t[1..]),
        _ => (false, t),
    };
    if body == "Infinity" {
        return MathValue::Infinity(negative);
    }
    let (mant, exp) = match body.find(['e', 'E']) {
        Some(i) => {
            let e = &body[i + 1..];
            let ok = !e.is_empty()
                && e.trim_start_matches(['+', '-'])
                    .bytes()
                    .all(|b| b.is_ascii_digit())
                && !e.trim_start_matches(['+', '-']).is_empty()
                && e.matches(['+', '-']).count() <= 1
                && !e[1..].contains(['+', '-']);
            if !ok {
                return MathValue::NaN;
            }
            let ev: i64 = e.parse().unwrap_or(if e.starts_with('-') {
                i64::MIN / 4
            } else {
                i64::MAX / 4
            });
            (&body[..i], ev)
        }
        None => (body, 0),
    };
    let (int, frac) = match mant.split_once('.') {
        Some((a, b)) => (a, b),
        None => (mant, ""),
    };
    if (int.is_empty() && frac.is_empty())
        || !int.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
    {
        return MathValue::NaN;
    }
    let digits = format!("{int}{frac}");
    MathValue::Finite(Decimal::from_digit_str(
        negative,
        &digits,
        exp.saturating_sub(frac.len() as i64),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_values() {
        let d = Decimal::from_f64(0.1);
        assert_eq!(d.magnitude(), -1);
        let r = to_raw_fixed(&d, 0, 20, 1, RoundingMode::HalfExpand);
        assert_eq!(r.string, "0.10000000000000000555");
        let r = to_raw_fixed(&Decimal::from_f64(1.005), 2, 2, 1, RoundingMode::HalfExpand);
        assert_eq!(r.string, "1.00");
        let r = to_raw_precision(&Decimal::from_f64(123456.0), 1, 3, RoundingMode::HalfExpand);
        assert_eq!(r.string, "123000");
        let r = to_raw_precision(&Decimal::from_f64(0.000123), 1, 2, RoundingMode::HalfExpand);
        assert_eq!(r.string, "0.00012");
        let r = to_raw_precision(&Decimal::from_f64(9.99), 1, 2, RoundingMode::HalfExpand);
        assert_eq!(r.string, "10");
        let r = to_raw_fixed(&Decimal::from_f64(2.5), 0, 0, 1, RoundingMode::HalfEven);
        assert_eq!(r.string, "2");
        let r = to_raw_fixed(&Decimal::from_f64(-2.5), 0, 0, 1, RoundingMode::HalfCeil);
        assert_eq!(r.string, "2");
        let r = to_raw_fixed(&Decimal::from_f64(1.23), 2, 2, 5, RoundingMode::HalfExpand);
        assert_eq!(r.string, "1.25");
        let MathValue::Finite(x) = parse_str("1e-3") else {
            panic!()
        };
        assert_eq!(
            to_raw_fixed(&x, 0, 3, 1, RoundingMode::HalfExpand).string,
            "0.001"
        );
        assert_eq!(parse_str("-Infinity"), MathValue::Infinity(true));
        assert_eq!(parse_str("1e"), MathValue::NaN);
    }
}
