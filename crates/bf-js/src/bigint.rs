//! BigInt (§6.1.6.2): enteros de precisión arbitraria propios (signo y magnitud en base 2³²)
//! con las operaciones del tipo: suma, resta, multiplicación (escolar y Karatsuba), división
//! y resto truncados (algoritmo D de Knuth), potencia, desplazamientos, operaciones bit a bit
//! con la semántica de complemento a dos infinito, comparación, conversión a y desde texto
//! en cualquier base y conversión exacta a y desde Number (redondeo al par).

use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BigInt {
    pub negative: bool,
    /// Dígitos en base 2³², del menos significativo al más; sin ceros al final.
    pub mag: Vec<u32>,
}

/// Tamaño máximo de un BigInt en bits (como otros motores: lo que lo supere es RangeError).
pub const MAX_BITS: u64 = 1 << 30;

/// Umbral (en dígitos) para multiplicar con Karatsuba.
const KARATSUBA_THRESHOLD: usize = 48;

impl BigInt {
    pub fn zero() -> BigInt {
        BigInt::default()
    }

    pub fn one() -> BigInt {
        BigInt::from_u64(1)
    }

    pub fn is_zero(&self) -> bool {
        self.mag.is_empty()
    }

    pub fn from_u64(v: u64) -> BigInt {
        BigInt {
            negative: false,
            mag: vec![v as u32, (v >> 32) as u32],
        }
        .normalize()
    }

    pub fn from_i64(v: i64) -> BigInt {
        let mut b = BigInt::from_u64(v.unsigned_abs());
        b.negative = v < 0 && !b.is_zero();
        b
    }

    fn from_mag(negative: bool, mag: Vec<u32>) -> BigInt {
        BigInt { negative, mag }.normalize()
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

    /// La mayor potencia de `radix` que cabe en un u32 y su exponente.
    fn chunk(radix: u32) -> (u32, usize) {
        let mut chunk = 1u32;
        let mut per = 0;
        while let Some(c) = chunk.checked_mul(radix) {
            chunk = c;
            per += 1;
        }
        (chunk, per)
    }

    /// Dígitos en `radix` (sin prefijo ni signo).
    pub fn from_digits(digits: &str, radix: u32) -> Option<BigInt> {
        if digits.is_empty() {
            return None;
        }
        let (_, per) = BigInt::chunk(radix);
        let values: Vec<u32> = digits
            .chars()
            .map(|c| c.to_digit(radix))
            .collect::<Option<Vec<u32>>>()?;
        let mut b = BigInt::zero();
        for part in values.chunks(per) {
            let mut v = 0u32;
            let mut m = 1u32;
            for &d in part {
                v = v * radix + d;
                m *= radix;
            }
            b.mul_small_add(m, v);
        }
        Some(b.normalize())
    }

    /// El valor de un literal BigInt tal como lo guarda el AST (`0x1f`, `123`).
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

    /// BigInt::toString ( x, radix ) (§6.1.6.2.23): minúsculas, `-` si es negativo.
    pub fn to_string_radix(&self, radix: u32) -> String {
        if self.is_zero() {
            return "0".to_owned();
        }
        let (chunk, per) = BigInt::chunk(radix);
        let mut tmp = self.clone();
        tmp.negative = false;
        let mut out: Vec<char> = Vec::new();
        while !tmp.mag.is_empty() {
            let mut r = tmp.div_small(chunk);
            // Cada trozo da `per` dígitos (el último, sin ceros a la izquierda).
            for _ in 0..per {
                if tmp.mag.is_empty() && r == 0 {
                    break;
                }
                out.push(std::char::from_digit(r % radix, radix).unwrap_or('0'));
                r /= radix;
            }
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
        } else if bits > 1100 {
            f64::INFINITY
        } else {
            // Los 64 bits altos con el resto como sticky: el redondeo de u64 → f64 (al par)
            // ve el bit sticky por debajo del bit de redondeo.
            let shift = bits - 64;
            let top = self.bits_at(shift, 64);
            let sticky = self.any_bit_below(shift);
            let top = if sticky { top | 1 } else { top };
            (top as f64) * 2f64.powi(shift as i32)
        };
        if self.negative { -v } else { v }
    }

    /// Los `n ≤ 64` bits de la magnitud desde el bit `from`.
    fn bits_at(&self, from: u64, n: u32) -> u64 {
        let mut v: u64 = 0;
        for i in 0..u64::from(n) {
            if self.bit(from + i) {
                v |= 1 << i;
            }
        }
        v
    }

    fn any_bit_below(&self, n: u64) -> bool {
        let full = (n / 32) as usize;
        if self.mag[..full.min(self.mag.len())].iter().any(|&d| d != 0) {
            return true;
        }
        let rem = n % 32;
        rem != 0 && full < self.mag.len() && self.mag[full] & ((1u32 << rem) - 1) != 0
    }

    /// Cantidad de bits de la magnitud.
    pub fn bit_len(&self) -> u64 {
        match self.mag.last() {
            None => 0,
            Some(&top) => (self.mag.len() as u64 - 1) * 32 + u64::from(32 - top.leading_zeros()),
        }
    }

    fn bit(&self, i: u64) -> bool {
        let w = (i / 32) as usize;
        w < self.mag.len() && self.mag[w] >> (i % 32) & 1 == 1
    }

    /// El BigInt de un Number entero (exacto; NumberToBigInt, §21.2.1.1.1).
    pub fn from_integer_f64(f: f64) -> BigInt {
        debug_assert!(f.is_finite() && f.fract() == 0.0);
        let negative = f < 0.0;
        let a = f.abs();
        if a < 18446744073709551616.0 {
            let mut b = BigInt::from_u64(a as u64);
            b.negative = negative && !b.is_zero();
            return b;
        }
        // a = m · 2^e con m entero de 53 bits.
        let bits = a.to_bits();
        let exp = ((bits >> 52) & 0x7FF) as i64 - 1075;
        let m = (bits & ((1 << 52) - 1)) | (1 << 52);
        let mut b = BigInt::from_u64(m).shl_bits(exp as u64);
        b.negative = negative;
        b
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

    // --- Aritmética ---

    fn add_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
        let (a, b) = if a.len() >= b.len() { (a, b) } else { (b, a) };
        let mut out = Vec::with_capacity(a.len() + 1);
        let mut carry = 0u64;
        for (i, &x) in a.iter().enumerate() {
            let v = u64::from(x) + u64::from(*b.get(i).unwrap_or(&0)) + carry;
            out.push(v as u32);
            carry = v >> 32;
        }
        if carry != 0 {
            out.push(carry as u32);
        }
        out
    }

    /// `a − b` con `|a| ≥ |b|`.
    fn sub_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
        let mut out = Vec::with_capacity(a.len());
        let mut borrow = 0i64;
        for (i, &x) in a.iter().enumerate() {
            let mut v = i64::from(x) - i64::from(*b.get(i).unwrap_or(&0)) - borrow;
            if v < 0 {
                v += 1 << 32;
                borrow = 1;
            } else {
                borrow = 0;
            }
            out.push(v as u32);
        }
        out
    }

    /// BigInt::add (§6.1.6.2.7).
    pub fn add(&self, o: &BigInt) -> BigInt {
        if self.negative == o.negative {
            return BigInt::from_mag(self.negative, BigInt::add_mag(&self.mag, &o.mag));
        }
        match BigInt::cmp_mag(&self.mag, &o.mag) {
            Ordering::Equal => BigInt::zero(),
            Ordering::Greater => {
                BigInt::from_mag(self.negative, BigInt::sub_mag(&self.mag, &o.mag))
            }
            Ordering::Less => BigInt::from_mag(o.negative, BigInt::sub_mag(&o.mag, &self.mag)),
        }
    }

    /// BigInt::subtract (§6.1.6.2.8).
    pub fn sub(&self, o: &BigInt) -> BigInt {
        self.add(&o.neg())
    }

    /// BigInt::unaryMinus (§6.1.6.2.1).
    pub fn neg(&self) -> BigInt {
        let mut b = self.clone();
        if !b.is_zero() {
            b.negative = !b.negative;
        }
        b
    }

    fn mul_school(a: &[u32], b: &[u32]) -> Vec<u32> {
        let mut out = vec![0u32; a.len() + b.len()];
        for (i, &x) in a.iter().enumerate() {
            if x == 0 {
                continue;
            }
            let mut carry = 0u64;
            for (j, &y) in b.iter().enumerate() {
                let v = u64::from(x) * u64::from(y) + u64::from(out[i + j]) + carry;
                out[i + j] = v as u32;
                carry = v >> 32;
            }
            let mut k = i + b.len();
            while carry != 0 {
                let v = u64::from(out[k]) + carry;
                out[k] = v as u32;
                carry = v >> 32;
                k += 1;
            }
        }
        out
    }

    fn trim(mut v: Vec<u32>) -> Vec<u32> {
        while v.last() == Some(&0) {
            v.pop();
        }
        v
    }

    /// Karatsuba: (a1·B + a0)(b1·B + b0) = z2·B² + (z1 − z2 − z0)·B + z0.
    fn mul_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
        if a.is_empty() || b.is_empty() {
            return Vec::new();
        }
        if a.len() < KARATSUBA_THRESHOLD || b.len() < KARATSUBA_THRESHOLD {
            return BigInt::trim(BigInt::mul_school(a, b));
        }
        let half = a.len().max(b.len()) / 2;
        let split = |x: &[u32]| -> (Vec<u32>, Vec<u32>) {
            if x.len() <= half {
                (BigInt::trim(x.to_vec()), Vec::new())
            } else {
                (
                    BigInt::trim(x[..half].to_vec()),
                    BigInt::trim(x[half..].to_vec()),
                )
            }
        };
        let (a0, a1) = split(a);
        let (b0, b1) = split(b);
        let z0 = BigInt::mul_mag(&a0, &b0);
        let z2 = BigInt::mul_mag(&a1, &b1);
        let sa = BigInt::trim(BigInt::add_mag(&a0, &a1));
        let sb = BigInt::trim(BigInt::add_mag(&b0, &b1));
        let z1 = BigInt::trim(BigInt::mul_mag(&sa, &sb));
        let z1 = BigInt::trim(BigInt::sub_mag(&z1, &z2));
        let z1 = BigInt::trim(BigInt::sub_mag(&z1, &z0));
        let mut out = vec![0u32; a.len() + b.len() + 1];
        let add_at = |out: &mut Vec<u32>, v: &[u32], at: usize| {
            let mut carry = 0u64;
            let mut i = 0;
            while i < v.len() || carry != 0 {
                let s = u64::from(out[at + i]) + u64::from(*v.get(i).unwrap_or(&0)) + carry;
                out[at + i] = s as u32;
                carry = s >> 32;
                i += 1;
            }
        };
        add_at(&mut out, &z0, 0);
        add_at(&mut out, &z1, half);
        add_at(&mut out, &z2, 2 * half);
        BigInt::trim(out)
    }

    /// BigInt::multiply (§6.1.6.2.4).
    pub fn mul(&self, o: &BigInt) -> BigInt {
        BigInt::from_mag(
            self.negative != o.negative,
            BigInt::mul_mag(&self.mag, &o.mag),
        )
    }

    /// División de magnitudes (algoritmo D de Knuth, TAOCP vol. 2 §4.3.1): (cociente, resto).
    fn divmod_mag(u: &[u32], v: &[u32]) -> (Vec<u32>, Vec<u32>) {
        if BigInt::cmp_mag(u, v) == Ordering::Less {
            return (Vec::new(), u.to_vec());
        }
        if v.len() == 1 {
            let mut q = BigInt::from_mag(false, u.to_vec());
            let r = q.div_small(v[0]);
            return (q.mag, if r == 0 { Vec::new() } else { vec![r] });
        }
        // D1: normalizar para que el dígito alto del divisor tenga el bit alto.
        let s = v[v.len() - 1].leading_zeros();
        let shl = |x: &[u32], extra: bool| -> Vec<u32> {
            let mut out = Vec::with_capacity(x.len() + 1);
            let mut carry = 0u32;
            for &d in x {
                if s == 0 {
                    out.push(d);
                } else {
                    out.push(d << s | carry);
                    carry = d >> (32 - s);
                }
            }
            if extra {
                out.push(carry);
            }
            out
        };
        let vn = shl(v, false);
        let mut un = shl(u, true);
        let n = vn.len();
        let m = un.len() - n - 1;
        let mut q = vec![0u32; m + 1];
        let b: u64 = 1 << 32;
        for j in (0..=m).rev() {
            // D3: estimar q̂.
            let num = u64::from(un[j + n]) << 32 | u64::from(un[j + n - 1]);
            let mut qhat = num / u64::from(vn[n - 1]);
            let mut rhat = num % u64::from(vn[n - 1]);
            while qhat >= b || qhat * u64::from(vn[n - 2]) > (rhat << 32 | u64::from(un[j + n - 2]))
            {
                qhat -= 1;
                rhat += u64::from(vn[n - 1]);
                if rhat >= b {
                    break;
                }
            }
            // D4: multiplicar y restar.
            let mut borrow: i64 = 0;
            let mut carry: u64 = 0;
            for i in 0..n {
                let p = qhat * u64::from(vn[i]) + carry;
                carry = p >> 32;
                let t = i64::from(un[i + j]) - borrow - (p & 0xFFFF_FFFF) as i64;
                un[i + j] = t as u32;
                borrow = i64::from(t < 0);
            }
            let t = i64::from(un[j + n]) - borrow - carry as i64;
            un[j + n] = t as u32;
            if t < 0 {
                // D6: sumar de vuelta.
                qhat -= 1;
                let mut c: u64 = 0;
                for i in 0..n {
                    let s2 = u64::from(un[i + j]) + u64::from(vn[i]) + c;
                    un[i + j] = s2 as u32;
                    c = s2 >> 32;
                }
                un[j + n] = un[j + n].wrapping_add(c as u32);
            }
            q[j] = qhat as u32;
        }
        // D8: desnormalizar el resto.
        let r: Vec<u32> = (0..n)
            .map(|i| {
                if s == 0 {
                    un[i]
                } else {
                    un[i] >> s | un[i + 1] << (32 - s)
                }
            })
            .collect();
        (BigInt::trim(q), BigInt::trim(r))
    }

    /// BigInt::divide (§6.1.6.2.5): truncada; `None` si `o` es 0.
    pub fn div(&self, o: &BigInt) -> Option<BigInt> {
        if o.is_zero() {
            return None;
        }
        let (q, _) = BigInt::divmod_mag(&self.mag, &o.mag);
        Some(BigInt::from_mag(self.negative != o.negative, q))
    }

    /// BigInt::remainder (§6.1.6.2.6): con el signo del dividendo; `None` si `o` es 0.
    pub fn rem(&self, o: &BigInt) -> Option<BigInt> {
        if o.is_zero() {
            return None;
        }
        let (_, r) = BigInt::divmod_mag(&self.mag, &o.mag);
        Some(BigInt::from_mag(self.negative, r))
    }

    /// BigInt::exponentiate (§6.1.6.2.3) con exponente no negativo; `None` si el resultado
    /// superaría [`MAX_BITS`].
    pub fn pow(&self, exp: &BigInt) -> Option<BigInt> {
        debug_assert!(!exp.negative);
        if exp.is_zero() {
            return Some(BigInt::one());
        }
        if self.is_zero() {
            return Some(BigInt::zero());
        }
        if self.mag == [1] {
            let odd = exp.mag[0] & 1 == 1;
            return Some(if self.negative && odd {
                self.clone()
            } else {
                BigInt::one()
            });
        }
        if exp.mag.len() > 1 {
            return None;
        }
        let e = u64::from(exp.mag[0]);
        if (self.bit_len() - 1).saturating_mul(e) > MAX_BITS {
            return None;
        }
        let mut result = BigInt::one();
        let mut base = self.clone();
        let mut k = e;
        while k > 0 {
            if k & 1 == 1 {
                result = result.mul(&base);
            }
            k >>= 1;
            if k > 0 {
                base = base.mul(&base);
            }
        }
        Some(result)
    }

    fn shl_bits(&self, n: u64) -> BigInt {
        if self.is_zero() || n == 0 {
            return self.clone();
        }
        let words = (n / 32) as usize;
        let bits = (n % 32) as u32;
        let mut mag = vec![0u32; words];
        let mut carry = 0u32;
        for &d in &self.mag {
            if bits == 0 {
                mag.push(d);
            } else {
                mag.push(d << bits | carry);
                carry = d >> (32 - bits);
            }
        }
        if carry != 0 {
            mag.push(carry);
        }
        BigInt::from_mag(self.negative, mag)
    }

    /// Desplazamiento aritmético a la derecha: ⌊self / 2ⁿ⌋.
    fn shr_bits(&self, n: u64) -> BigInt {
        if self.is_zero() {
            return self.clone();
        }
        if n >= self.bit_len() {
            return if self.negative {
                BigInt::from_i64(-1)
            } else {
                BigInt::zero()
            };
        }
        let words = (n / 32) as usize;
        let bits = (n % 32) as u32;
        let src = &self.mag[words..];
        let mag: Vec<u32> = (0..src.len())
            .map(|i| {
                let lo = src[i];
                let hi = *src.get(i + 1).unwrap_or(&0);
                if bits == 0 {
                    lo
                } else {
                    lo >> bits | hi << (32 - bits)
                }
            })
            .collect();
        let r = BigInt::from_mag(self.negative, mag);
        // Para negativos, el piso: si se perdió algún bit, restar 1.
        if self.negative && self.any_bit_below(n) {
            r.sub(&BigInt::one())
        } else {
            r
        }
    }

    /// El valor de un desplazamiento (saturado).
    fn shift_amount(n: &BigInt) -> u64 {
        if n.mag.len() > 2 {
            return u64::MAX;
        }
        u64::from(n.mag.first().copied().unwrap_or(0))
            | u64::from(n.mag.get(1).copied().unwrap_or(0)) << 32
    }

    /// BigInt::leftShift (§6.1.6.2.9): `self · 2^n` (n negativo desplaza a la derecha);
    /// `None` si el resultado superaría [`MAX_BITS`].
    pub fn shl(&self, n: &BigInt) -> Option<BigInt> {
        let amount = BigInt::shift_amount(n);
        if n.negative {
            return Some(self.shr_bits(amount));
        }
        if self.is_zero() {
            return Some(BigInt::zero());
        }
        if self.bit_len().saturating_add(amount) > MAX_BITS {
            return None;
        }
        Some(self.shl_bits(amount))
    }

    /// BigInt::signedRightShift (§6.1.6.2.10).
    pub fn shr(&self, n: &BigInt) -> Option<BigInt> {
        self.shl(&n.neg())
    }

    /// La representación en complemento a dos con `len` dígitos (suficientes para el signo).
    fn twos(&self, len: usize) -> Vec<u32> {
        let mut v = self.mag.clone();
        v.resize(len, 0);
        if self.negative {
            let mut carry = 1u64;
            for d in &mut v {
                let x = u64::from(!*d) + carry;
                *d = x as u32;
                carry = x >> 32;
            }
        }
        v
    }

    fn from_twos(mut v: Vec<u32>) -> BigInt {
        let negative = v.last().is_some_and(|&d| d >> 31 == 1);
        if !negative {
            return BigInt::from_mag(false, v);
        }
        let mut carry = 1u64;
        for d in &mut v {
            let x = u64::from(!*d) + carry;
            *d = x as u32;
            carry = x >> 32;
        }
        BigInt::from_mag(true, v)
    }

    fn bitwise(&self, o: &BigInt, f: impl Fn(u32, u32) -> u32) -> BigInt {
        let len = self.mag.len().max(o.mag.len()) + 1;
        let a = self.twos(len);
        let b = o.twos(len);
        BigInt::from_twos(a.iter().zip(&b).map(|(&x, &y)| f(x, y)).collect())
    }

    /// BigInt::bitwiseAND (§6.1.6.2.18).
    pub fn bitand(&self, o: &BigInt) -> BigInt {
        self.bitwise(o, |x, y| x & y)
    }

    /// BigInt::bitwiseOR (§6.1.6.2.20).
    pub fn bitor(&self, o: &BigInt) -> BigInt {
        self.bitwise(o, |x, y| x | y)
    }

    /// BigInt::bitwiseXOR (§6.1.6.2.19).
    pub fn bitxor(&self, o: &BigInt) -> BigInt {
        self.bitwise(o, |x, y| x ^ y)
    }

    /// BigInt::bitwiseNOT (§6.1.6.2.2): −x − 1.
    pub fn bitnot(&self) -> BigInt {
        self.neg().sub(&BigInt::one())
    }

    /// BigInt.asUintN: `self` módulo 2^bits; `None` si el resultado superaría [`MAX_BITS`].
    pub fn as_uint_n(&self, bits: u64) -> Option<BigInt> {
        if bits == 0 {
            return Some(BigInt::zero());
        }
        if !self.negative {
            if self.bit_len() <= bits {
                return Some(self.clone());
            }
            return Some(self.low_bits(bits));
        }
        if bits > MAX_BITS {
            return None;
        }
        // El complemento a dos con al menos `bits` bits, truncado.
        let words = bits.div_ceil(32) as usize;
        let v = self.twos(words.max(self.mag.len() + 1));
        Some(BigInt::from_mag(false, v).low_bits(bits))
    }

    /// Los `bits` bits bajos de la magnitud (de un no negativo).
    fn low_bits(&self, bits: u64) -> BigInt {
        let words = bits.div_ceil(32) as usize;
        let mut mag: Vec<u32> = self.mag.iter().take(words).copied().collect();
        let rem = (bits % 32) as u32;
        if rem != 0 && mag.len() == words {
            let last = mag.len() - 1;
            mag[last] &= (1u32 << rem) - 1;
        }
        BigInt::from_mag(false, mag)
    }

    /// BigInt.asIntN: `self` módulo 2^bits interpretado con signo.
    pub fn as_int_n(&self, bits: u64) -> Option<BigInt> {
        if bits == 0 {
            return Some(BigInt::zero());
        }
        // Cabe sin cambios si −2^(bits−1) ≤ self < 2^(bits−1).
        if self.bit_len() < bits {
            return Some(self.clone());
        }
        let m = self.as_uint_n(bits)?;
        if m.bit(bits - 1) {
            Some(m.sub(&BigInt::one().shl_bits(bits)))
        } else {
            Some(m)
        }
    }

    /// Los 64 bits bajos en complemento a dos (ToBigInt64 / ToBigUint64).
    pub fn to_u64_wrapping(&self) -> u64 {
        let lo = u64::from(self.mag.first().copied().unwrap_or(0))
            | u64::from(self.mag.get(1).copied().unwrap_or(0)) << 32;
        if self.negative { lo.wrapping_neg() } else { lo }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(s: &str) -> BigInt {
        BigInt::from_string(s).unwrap()
    }

    fn s(x: &BigInt) -> String {
        x.to_string_radix(10)
    }

    #[test]
    fn literals_and_strings() {
        assert_eq!(s(&BigInt::from_literal("0x10").unwrap()), "16");
        assert_eq!(
            s(&BigInt::from_literal("123456789012345678901234567890").unwrap()),
            "123456789012345678901234567890"
        );
        assert_eq!(s(&BigInt::from_literal("0").unwrap()), "0");
        assert_eq!(s(&b(" -12 ")), "-12");
        assert!(BigInt::from_string("1.5").is_none());
        assert!(BigInt::from_string("-0x10").is_none());
        assert!(BigInt::from_string("0x").is_none());
        assert_eq!(
            BigInt::from_literal("9007199254740993").unwrap().to_f64(),
            9007199254740992.0
        );
        assert_eq!(b("3").compare_f64(2.5), Ordering::Greater);
        assert_eq!(b("2").compare_f64(2.5), Ordering::Less);
        assert_eq!(b("255").to_string_radix(16), "ff");
        assert_eq!(b("-255").to_string_radix(2), "-11111111");
        assert_eq!(b("1000000000000").to_string_radix(36), "cre66i9s");
        assert_eq!(b("1000000000").to_string_radix(10), "1000000000");
    }

    #[test]
    fn arithmetic() {
        let x = b("123456789012345678901234567890");
        let y = b("-987654321098765432109876543210");
        assert_eq!(s(&x.add(&y)), "-864197532086419753208641975320");
        assert_eq!(s(&x.sub(&y)), "1111111110111111111011111111100");
        assert_eq!(
            s(&x.mul(&y)),
            "-121932631137021795226185032733622923332237463801111263526900"
        );
        assert_eq!(s(&y.div(&x).unwrap()), "-8");
        assert_eq!(s(&y.rem(&x).unwrap()), "-9000000000900000000090");
        assert!(x.div(&BigInt::zero()).is_none());
        assert_eq!(
            s(&b("2").pow(&b("100")).unwrap()),
            "1267650600228229401496703205376"
        );
        assert_eq!(s(&b("-3").pow(&b("3")).unwrap()), "-27");
        // División larga con varios dígitos de 32 bits.
        let big = b("2").pow(&b("200")).unwrap().add(&b("12345"));
        let d = b("2").pow(&b("100")).unwrap().add(&b("7"));
        let q = big.div(&d).unwrap();
        let r = big.rem(&d).unwrap();
        assert_eq!(q.mul(&d).add(&r), big);
        assert_eq!(r.compare(&d), Ordering::Less);
    }

    #[test]
    fn karatsuba_matches_schoolbook() {
        let x = b("3").pow(&b("2000")).unwrap();
        let y = b("7").pow(&b("1500")).unwrap().neg();
        let k = x.mul(&y);
        let school = BigInt::from_mag(true, BigInt::trim(BigInt::mul_school(&x.mag, &y.mag)));
        assert_eq!(k, school);
        assert_eq!(k.div(&y).unwrap(), x.neg().neg());
        assert_eq!(k.rem(&y).unwrap(), BigInt::zero());
    }

    #[test]
    fn shifts_and_bitwise() {
        assert_eq!(s(&b("1").shl(&b("70")).unwrap()), "1180591620717411303424");
        assert_eq!(s(&b("-5").shr(&b("1")).unwrap()), "-3");
        assert_eq!(s(&b("-1").shr(&b("1000")).unwrap()), "-1");
        assert_eq!(s(&b("5").shl(&b("-1")).unwrap()), "2");
        assert_eq!(s(&b("-6").bitand(&b("13"))), "8");
        assert_eq!(s(&b("-6").bitor(&b("13"))), "-1");
        assert_eq!(s(&b("-6").bitxor(&b("13"))), "-9");
        assert_eq!(s(&b("5").bitnot()), "-6");
        assert_eq!(s(&b("-1").as_uint_n(64).unwrap()), "18446744073709551615");
        assert_eq!(s(&b("255").as_int_n(8).unwrap()), "-1");
        assert_eq!(s(&b("128").as_int_n(8).unwrap()), "-128");
        assert_eq!(s(&b("-129").as_int_n(8).unwrap()), "127");
        assert_eq!(s(&b("257").as_uint_n(8).unwrap()), "1");
    }

    #[test]
    fn f64_conversions() {
        for f in [0.0, 1.0, -1.0, 2f64.powi(53), 1e21, -1.5e300, 2f64.powi(64)] {
            let x = BigInt::from_integer_f64(f);
            assert_eq!(x.to_f64(), f);
        }
        // Redondeo al par en el medio: 2^53 + 1 → 2^53, 2^53 + 3 → 2^53 + 4.
        assert_eq!(b("9007199254740993").to_f64(), 9007199254740992.0);
        assert_eq!(b("9007199254740995").to_f64(), 9007199254740996.0);
        // Con sticky por encima de 64 bits.
        let x = b("2").pow(&b("80")).unwrap().add(&b("1"));
        assert_eq!(x.to_f64(), 2f64.powi(80));
        assert_eq!(s(&BigInt::from_integer_f64(1e21)), "1000000000000000000000");
        assert_eq!(b("2").pow(&b("1024")).unwrap().to_f64(), f64::INFINITY);
    }
}
