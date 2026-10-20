//! BigInt (§6.1.6.2): enteros de precisión arbitraria (signo y magnitud en base 2³²). En
//! esta etapa solo lo que necesitan los literales, `typeof`, las comparaciones y `ToString`;
//! la aritmética llega con los built-ins de BigInt.

use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BigInt {
    pub negative: bool,
    /// Dígitos en base 2³², del menos significativo al más; sin ceros al final.
    pub mag: Vec<u32>,
}

impl BigInt {
    pub fn zero() -> BigInt {
        BigInt::default()
    }

    pub fn is_zero(&self) -> bool {
        self.mag.is_empty()
    }

    fn normalize(mut self) -> BigInt {
        while self.mag.last() == Some(&0) {
            self.mag.pop();
        }
        if self.mag.is_empty() {
            self.negative = false;
        }
        self
    }

    fn mul_small_add(&mut self, m: u32, a: u32) {
        let mut carry = u64::from(a);
        for d in &mut self.mag {
            let v = u64::from(*d) * u64::from(m) + carry;
            *d = v as u32;
            carry = v >> 32;
        }
        if carry != 0 {
            self.mag.push(carry as u32);
        }
    }

    /// Divide la magnitud por `d` y devuelve el resto.
    fn div_small(&mut self, d: u32) -> u32 {
        let mut rem: u64 = 0;
        for x in self.mag.iter_mut().rev() {
            let cur = rem << 32 | u64::from(*x);
            *x = (cur / u64::from(d)) as u32;
            rem = cur % u64::from(d);
        }
        while self.mag.last() == Some(&0) {
            self.mag.pop();
        }
        rem as u32
    }

    /// Dígitos en `radix` (sin prefijo ni signo).
    pub fn from_digits(digits: &str, radix: u32) -> Option<BigInt> {
        let mut b = BigInt::zero();
        if digits.is_empty() {
            return None;
        }
        for c in digits.chars() {
            let d = c.to_digit(radix)?;
            b.mul_small_add(radix, d);
        }
        Some(b.normalize())
    }

    /// El valor de un literal BigInt tal como lo guarda el AST (`0x1f`, `017` no, `123`).
    pub fn from_literal(raw: &str) -> Option<BigInt> {
        let lower = raw.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("0x") {
            BigInt::from_digits(rest, 16)
        } else if let Some(rest) = lower.strip_prefix("0o") {
            BigInt::from_digits(rest, 8)
        } else if let Some(rest) = lower.strip_prefix("0b") {
            BigInt::from_digits(rest, 2)
        } else {
            BigInt::from_digits(&lower, 10)
        }
    }

    pub fn to_string_radix(&self, radix: u32) -> String {
        if self.is_zero() {
            return "0".to_owned();
        }
        let mut tmp = self.clone();
        let mut out = Vec::new();
        while !tmp.mag.is_empty() {
            let r = tmp.div_small(radix);
            out.push(std::char::from_digit(r, radix).unwrap_or('0'));
        }
        if self.negative {
            out.push('-');
        }
        out.iter().rev().collect()
    }

    /// El Number más cercano (§21.2.1.1.1 Number(bigint): redondeo al par).
    pub fn to_f64(&self) -> f64 {
        let bits = self.bit_len();
        if bits == 0 {
            return 0.0;
        }
        let v = if bits <= 64 {
            let mut x: u64 = 0;
            for (i, &d) in self.mag.iter().enumerate() {
                x |= u64::from(d) << (32 * i);
            }
            x as f64
        } else {
            // Los 64 bits altos con el resto como sticky, y se escala.
            let shift = bits - 64;
            let mut top: u64 = 0;
            for i in 0..64 {
                if self.bit(shift + i) {
                    top |= 1 << i;
                }
            }
            let sticky = (0..shift).any(|i| self.bit(i));
            // Redondeo en dos pasos sin doble redondeo: se fuerza el bit bajo si hay sticky.
            let top = if sticky { top | 1 } else { top };
            let f = top as f64;
            if shift > 2000 {
                f64::INFINITY
            } else {
                f * 2f64.powi(shift as i32)
            }
        };
        if self.negative { -v } else { v }
    }

    fn bit_len(&self) -> u64 {
        match self.mag.last() {
            None => 0,
            Some(&top) => (self.mag.len() as u64 - 1) * 32 + u64::from(32 - top.leading_zeros()),
        }
    }

    fn bit(&self, i: u64) -> bool {
        let w = (i / 32) as usize;
        w < self.mag.len() && self.mag[w] >> (i % 32) & 1 == 1
    }

    /// El BigInt de un Number entero (exacto).
    pub fn from_integer_f64(f: f64) -> BigInt {
        debug_assert!(f.is_finite() && f.fract() == 0.0);
        let negative = f < 0.0;
        let mut a = f.abs();
        let mut mag = Vec::new();
        while a >= 1.0 {
            let d = a % 4294967296.0;
            mag.push(d as u32);
            a = ((a - d) / 4294967296.0).floor();
        }
        BigInt { negative, mag }.normalize()
    }

    fn cmp_mag(a: &[u32], b: &[u32]) -> Ordering {
        a.len()
            .cmp(&b.len())
            .then_with(|| a.iter().rev().cmp(b.iter().rev()))
    }

    pub fn compare(&self, other: &BigInt) -> Ordering {
        match (self.negative, other.negative) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => BigInt::cmp_mag(&self.mag, &other.mag),
            (true, true) => BigInt::cmp_mag(&other.mag, &self.mag),
        }
    }

    /// Compara con un Number finito o infinito (no NaN): ℝ(self) frente a ℝ(n).
    pub fn compare_f64(&self, n: f64) -> Ordering {
        if n == f64::INFINITY {
            return Ordering::Less;
        }
        if n == f64::NEG_INFINITY {
            return Ordering::Greater;
        }
        let floor = n.floor();
        let fb = BigInt::from_integer_f64(floor);
        match self.compare(&fb) {
            Ordering::Equal if floor != n => Ordering::Less,
            o => o,
        }
    }

    /// StringToBigInt (§7.1.14): `None` si no es un StringIntegerLiteral.
    pub fn from_string(s: &str) -> Option<BigInt> {
        let t = crate::abstract_ops::conversion::trim_js_whitespace(s);
        if t.is_empty() {
            return Some(BigInt::zero());
        }
        let lower = t.to_ascii_lowercase();
        for (prefix, radix) in [("0x", 16), ("0o", 8), ("0b", 2)] {
            if let Some(rest) = lower.strip_prefix(prefix) {
                return BigInt::from_digits(rest, radix);
            }
        }
        let (negative, digits) = match t.as_bytes()[0] {
            b'-' => (true, &t[1..]),
            b'+' => (false, &t[1..]),
            _ => (false, t),
        };
        if !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let mut b = BigInt::from_digits(digits, 10)?;
        b.negative = negative && !b.is_zero();
        Some(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_and_strings() {
        assert_eq!(
            BigInt::from_literal("0x10").unwrap().to_string_radix(10),
            "16"
        );
        assert_eq!(
            BigInt::from_literal("123456789012345678901234567890")
                .unwrap()
                .to_string_radix(10),
            "123456789012345678901234567890"
        );
        assert_eq!(BigInt::from_literal("0").unwrap().to_string_radix(10), "0");
        assert_eq!(
            BigInt::from_string(" -12 ").unwrap().to_string_radix(10),
            "-12"
        );
        assert!(BigInt::from_string("1.5").is_none());
        assert_eq!(
            BigInt::from_literal("9007199254740993").unwrap().to_f64(),
            9007199254740992.0
        );
        assert_eq!(
            BigInt::from_literal("3").unwrap().compare_f64(2.5),
            Ordering::Greater
        );
        assert_eq!(
            BigInt::from_literal("2").unwrap().compare_f64(2.5),
            Ordering::Less
        );
    }
}
