//! Math (§21.3): las constantes y todas las funciones, con los casos especiales de ±0, NaN e
//! ±∞ de cada una.

use crate::abstract_ops::conversion::{to_int32, to_uint32};
use crate::abstract_ops::number::exponentiate;
use crate::builtins::{FnEntry, frozen_prop, methods, to_string_tag};
use crate::value::Value;
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

pub fn init(agent: &mut Agent, realm: RealmId) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    let m = agent.new_object(Some(op));
    for (name, v) in [
        ("E", std::f64::consts::E),
        ("LN10", std::f64::consts::LN_10),
        ("LN2", std::f64::consts::LN_2),
        ("LOG10E", std::f64::consts::LOG10_E),
        ("LOG2E", std::f64::consts::LOG2_E),
        ("PI", std::f64::consts::PI),
        ("SQRT1_2", std::f64::consts::FRAC_1_SQRT_2),
        ("SQRT2", std::f64::consts::SQRT_2),
    ] {
        frozen_prop(agent, m, name, Value::Number(v));
    }
    const FNS: &[FnEntry] = &[
        ("abs", 1, abs),
        ("acos", 1, acos),
        ("acosh", 1, acosh),
        ("asin", 1, asin),
        ("asinh", 1, asinh),
        ("atan", 1, atan),
        ("atanh", 1, atanh),
        ("atan2", 2, atan2),
        ("cbrt", 1, cbrt),
        ("ceil", 1, ceil),
        ("clz32", 1, clz32),
        ("cos", 1, cos),
        ("cosh", 1, cosh),
        ("exp", 1, exp),
        ("expm1", 1, expm1),
        ("floor", 1, floor),
        ("fround", 1, fround),
        ("f16round", 1, f16round),
        ("hypot", 2, hypot),
        ("imul", 2, imul),
        ("log", 1, log),
        ("log1p", 1, log1p),
        ("log10", 1, log10),
        ("log2", 1, log2),
        ("max", 2, max),
        ("min", 2, min),
        ("pow", 2, pow),
        ("random", 0, random),
        ("round", 1, round),
        ("sign", 1, sign),
        ("sin", 1, sin),
        ("sinh", 1, sinh),
        ("sqrt", 1, sqrt),
        ("sumPrecise", 1, sum_precise),
        ("tan", 1, tan),
        ("tanh", 1, tanh),
        ("trunc", 1, trunc),
    ];
    methods(agent, m, FNS);
    to_string_tag(agent, m, "Math");
    agent.realms[realm].intrinsics.math = m;
}

fn num(agent: &mut Agent, a: &CallArgs<'_>, i: usize) -> JsResult<f64> {
    agent.to_number(&a.arg(i))
}

macro_rules! unary {
    ($name:ident, $doc:literal, $f:expr) => {
        #[doc = $doc]
        fn $name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
            let x = num(agent, a, 0)?;
            let f: fn(f64) -> f64 = $f;
            Ok(Value::Number(f(x)))
        }
    };
}

unary!(abs, "Math.abs (§21.3.2.1).", f64::abs);
unary!(acos, "Math.acos (§21.3.2.2).", f64::acos);
unary!(acosh, "Math.acosh (§21.3.2.3).", |x| if x < 1.0 {
    f64::NAN
} else {
    x.acosh()
});
unary!(asin, "Math.asin (§21.3.2.4).", f64::asin);
unary!(asinh, "Math.asinh (§21.3.2.5).", |x| if x == 0.0 {
    x
} else {
    x.asinh()
});
unary!(atan, "Math.atan (§21.3.2.6).", f64::atan);
unary!(atanh, "Math.atanh (§21.3.2.7).", |x| if x == 0.0 {
    x
} else {
    x.atanh()
});
unary!(cbrt, "Math.cbrt (§21.3.2.9).", f64::cbrt);
unary!(ceil, "Math.ceil (§21.3.2.10).", f64::ceil);
unary!(cos, "Math.cos (§21.3.2.12).", f64::cos);
unary!(cosh, "Math.cosh (§21.3.2.13).", f64::cosh);
unary!(exp, "Math.exp (§21.3.2.14).", f64::exp);
unary!(expm1, "Math.expm1 (§21.3.2.15).", |x| if x == 0.0 {
    x
} else {
    x.exp_m1()
});
unary!(floor, "Math.floor (§21.3.2.16).", f64::floor);
unary!(fround, "Math.fround (§21.3.2.17).", |x| f64::from(
    x as f32
));
unary!(f16round, "Math.f16round (§21.3.2.18).", round_to_f16);
unary!(log, "Math.log (§21.3.2.20).", f64::ln);
unary!(log1p, "Math.log1p (§21.3.2.21).", |x| if x == 0.0 {
    x
} else {
    x.ln_1p()
});
unary!(log10, "Math.log10 (§21.3.2.22).", f64::log10);
unary!(log2, "Math.log2 (§21.3.2.23).", f64::log2);
unary!(sin, "Math.sin (§21.3.2.30).", f64::sin);
unary!(sinh, "Math.sinh (§21.3.2.31).", |x| if x == 0.0 {
    x
} else {
    x.sinh()
});
unary!(sqrt, "Math.sqrt (§21.3.2.32).", f64::sqrt);
unary!(tan, "Math.tan (§21.3.2.34).", f64::tan);
unary!(tanh, "Math.tanh (§21.3.2.35).", |x| if x == 0.0 {
    x
} else {
    x.tanh()
});
unary!(trunc, "Math.trunc (§21.3.2.36).", f64::trunc);
unary!(sign, "Math.sign (§21.3.2.29).", |x| {
    if x.is_nan() || x == 0.0 {
        x
    } else if x > 0.0 {
        1.0
    } else {
        -1.0
    }
});
unary!(
    round,
    "Math.round (§21.3.2.28): la mitad hacia +∞, con −0 en [−0.5, −0].",
    |x| {
        if !x.is_finite() || x == 0.0 {
            return x;
        }
        if x > 0.0 && x < 0.5 {
            return 0.0;
        }
        if (-0.5..0.0).contains(&x) {
            return -0.0;
        }
        let f = x.floor();
        if x - f >= 0.5 { f + 1.0 } else { f }
    }
);

/// Math.atan2 (§21.3.2.8).
fn atan2(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let y = num(agent, a, 0)?;
    let x = num(agent, a, 1)?;
    Ok(Value::Number(y.atan2(x)))
}

/// Math.clz32 (§21.3.2.11).
fn clz32(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = to_uint32(num(agent, a, 0)?);
    Ok(Value::Number(f64::from(n.leading_zeros())))
}

/// Math.imul (§21.3.2.19).
fn imul(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let x = to_int32(num(agent, a, 0)?);
    let y = to_int32(num(agent, a, 1)?);
    Ok(Value::Number(f64::from(x.wrapping_mul(y))))
}

/// Math.hypot (§21.3.2.18): convierte todos los argumentos antes de mirar los casos.
fn hypot(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let mut coerced = Vec::with_capacity(a.len());
    for v in a.args {
        coerced.push(agent.to_number(v)?);
    }
    if coerced.iter().any(|x| x.is_infinite()) {
        return Ok(Value::Number(f64::INFINITY));
    }
    if coerced.iter().any(|x| x.is_nan()) {
        return Ok(Value::Number(f64::NAN));
    }
    let max = coerced.iter().fold(0.0f64, |m, x| m.max(x.abs()));
    if max == 0.0 {
        return Ok(Value::Number(0.0));
    }
    // Escalado para no desbordar, con suma compensada (Kahan).
    let (mut sum, mut comp) = (0.0f64, 0.0f64);
    for x in &coerced {
        let r = x / max;
        let y = r * r - comp;
        let t = sum + y;
        comp = (t - sum) - y;
        sum = t;
    }
    Ok(Value::Number(sum.sqrt() * max))
}

/// Math.max / Math.min (§21.3.2.24–25): convierte todo antes de comparar; +0 > −0.
fn extremum(agent: &mut Agent, a: &CallArgs<'_>, is_max: bool) -> JsResult<Value> {
    let mut coerced = Vec::with_capacity(a.len());
    for v in a.args {
        coerced.push(agent.to_number(v)?);
    }
    let mut best = if is_max {
        f64::NEG_INFINITY
    } else {
        f64::INFINITY
    };
    for x in coerced {
        if x.is_nan() {
            return Ok(Value::Number(f64::NAN));
        }
        let better = if is_max {
            x > best || (x == 0.0 && best == 0.0 && !x.is_sign_negative())
        } else {
            x < best || (x == 0.0 && best == 0.0 && x.is_sign_negative())
        };
        if better {
            best = x;
        }
    }
    Ok(Value::Number(best))
}

fn max(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    extremum(agent, a, true)
}

fn min(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    extremum(agent, a, false)
}

/// Math.pow (§21.3.2.26).
fn pow(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let b = num(agent, a, 0)?;
    let e = num(agent, a, 1)?;
    Ok(Value::Number(exponentiate(b, e)))
}

thread_local! {
    static SEED: std::cell::Cell<u64> = std::cell::Cell::new({
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x9E37_79B9_7F4A_7C15, |d| d.as_nanos() as u64);
        t | 1
    });
}

/// Math.random (§21.3.2.27): xorshift64*, uniforme en [0, 1).
fn random(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let r = SEED.with(|s| {
        let mut x = s.get();
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        s.set(x);
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    });
    Ok(Value::Number((r >> 11) as f64 / (1u64 << 53) as f64))
}

/// El binary16 más cercano (empates al par), sin doble redondeo (f16round).
pub fn round_to_f16(x: f64) -> f64 {
    if x.is_nan() || x.is_infinite() || x == 0.0 {
        return x;
    }
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let a = x.abs();
    // El máximo binary16 es 65504; desde 65520 redondea a infinito.
    if a >= 65520.0 {
        return sign * f64::INFINITY;
    }
    // La unidad de la última posición: 2^-24 en los subnormales, 2^(e-10) en los normales.
    let exp = a.log2().floor() as i32;
    let exp = if 2f64.powi(exp) > a { exp - 1 } else { exp };
    let ulp_exp = if exp < -14 { -24 } else { exp - 10 };
    let ulp = 2f64.powi(ulp_exp);
    let q = a / ulp;
    let fl = q.floor();
    let rem = q - fl;
    let n = if rem > 0.5 || (rem == 0.5 && fl % 2.0 == 1.0) {
        fl + 1.0
    } else {
        fl
    };
    sign * n * ulp
}

/// Math.sumPrecise (§21.3.2.33): la suma exacta de los valores del iterable, redondeada una
/// sola vez.
fn sum_precise(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let items = a.arg(0);
    agent.require_object_coercible(&items)?;
    let (it, next) = agent.get_iterator(&items)?;
    let mut acc = ExactSum::default();
    let mut count: u64 = 0;
    while let Some(v) = agent.iterator_step_value(&it, &next)? {
        count += 1;
        if count >= 1 << 53 {
            agent.iterator_close_on_throw(&it);
            return agent.range_error("Too many values for Math.sumPrecise");
        }
        let Value::Number(n) = v else {
            agent.iterator_close_on_throw(&it);
            return agent.type_error("Math.sumPrecise: value is not a Number");
        };
        acc.add(n);
    }
    Ok(Value::Number(acc.result()))
}

/// Acumulador exacto: un entero en unidades de 2^-1074 en limbs de 32 bits con signo.
#[derive(Default)]
struct ExactSum {
    limbs: Vec<i64>,
    pending: u32,
    nan: bool,
    pos_inf: bool,
    neg_inf: bool,
    all_negative_zero: bool,
    any: bool,
}

const LIMBS: usize = 2100 / 32 + 2;

impl ExactSum {
    fn add(&mut self, x: f64) {
        if !self.any {
            self.any = true;
            self.all_negative_zero = true;
            self.limbs = vec![0; LIMBS];
        }
        if x.is_nan() {
            self.nan = true;
            return;
        }
        if x == f64::INFINITY {
            self.pos_inf = true;
            return;
        }
        if x == f64::NEG_INFINITY {
            self.neg_inf = true;
            return;
        }
        if !(x == 0.0 && x.is_sign_negative()) {
            self.all_negative_zero = false;
        }
        if x == 0.0 {
            return;
        }
        let bits = x.to_bits();
        let biased = ((bits >> 52) & 0x7FF) as i64;
        let frac = bits & ((1 << 52) - 1);
        let (m, shift) = if biased == 0 {
            (frac, 0)
        } else {
            (frac | (1 << 52), biased - 1)
        };
        let sign: i64 = if x < 0.0 { -1 } else { 1 };
        let word = (shift / 32) as usize;
        let off = (shift % 32) as u32;
        let wide = u128::from(m) << off;
        for k in 0..3 {
            let part = ((wide >> (32 * k)) & 0xFFFF_FFFF) as i64;
            if part != 0 {
                self.limbs[word + k] += sign * part;
            }
        }
        self.pending += 1;
        if self.pending >= 1 << 20 {
            self.normalize();
        }
    }

    /// Propaga los acarreos: los limbs quedan en [0, 2^32) salvo un −1 final si es negativo.
    fn normalize(&mut self) {
        let mut carry: i64 = 0;
        for l in &mut self.limbs {
            let v = *l + carry;
            *l = v.rem_euclid(1 << 32);
            carry = v.div_euclid(1 << 32);
        }
        if carry != 0 {
            self.limbs.push(carry);
        }
        self.pending = 0;
    }

    fn result(mut self) -> f64 {
        if !self.any || (self.all_negative_zero && !self.nan && !self.pos_inf && !self.neg_inf) {
            return -0.0;
        }
        if self.nan || (self.pos_inf && self.neg_inf) {
            return f64::NAN;
        }
        if self.pos_inf {
            return f64::INFINITY;
        }
        if self.neg_inf {
            return f64::NEG_INFINITY;
        }
        self.normalize();
        let limbs = std::mem::take(&mut self.limbs);
        let negative = limbs.last().is_some_and(|&l| l < 0);
        let mut mag: Vec<u32> = Vec::with_capacity(limbs.len());
        if negative {
            // Complemento a dos sobre base 2^32 (sin el limb de signo).
            let mut carry = 1i64;
            for l in &limbs[..limbs.len() - 1] {
                let v = (0xFFFF_FFFF - *l) + carry;
                mag.push((v & 0xFFFF_FFFF) as u32);
                carry = v >> 32;
            }
        } else {
            mag.extend(limbs.iter().map(|&l| l as u32));
        }
        while mag.last() == Some(&0) {
            mag.pop();
        }
        let v = magnitude_to_f64(&mag);
        if negative { -v } else { v }
    }
}

/// Redondea al par más cercano un entero (en unidades de 2^-1074) a un Number.
fn magnitude_to_f64(mag: &[u32]) -> f64 {
    let Some(&top) = mag.last() else {
        return 0.0;
    };
    let bit_len = (mag.len() as i64 - 1) * 32 + i64::from(32 - top.leading_zeros());
    let bit = |i: i64| -> bool {
        let w = (i / 32) as usize;
        w < mag.len() && (mag[w] >> (i % 32)) & 1 == 1
    };
    if bit_len <= 53 {
        let mut v: u64 = 0;
        for i in (0..bit_len).rev() {
            v = v << 1 | u64::from(bit(i));
        }
        return v as f64 * f64::from_bits(1);
    }
    let mut shift = bit_len - 53;
    let mut m: u64 = 0;
    for i in (shift..bit_len).rev() {
        m = m << 1 | u64::from(bit(i));
    }
    let round = bit(shift - 1);
    let sticky = (0..shift - 1).any(bit);
    if round && (sticky || m & 1 == 1) {
        m += 1;
        if m == 1 << 53 {
            m >>= 1;
            shift += 1;
        }
    }
    let e = shift - 1074;
    if e > 971 {
        return f64::INFINITY;
    }
    m as f64 * 2f64.powi(e as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16_rounding() {
        assert_eq!(round_to_f16(1.0), 1.0);
        assert_eq!(round_to_f16(65504.0), 65504.0);
        assert_eq!(round_to_f16(65519.99), 65504.0);
        assert_eq!(round_to_f16(65520.0), f64::INFINITY);
        assert_eq!(round_to_f16(5.960464477539063e-8), 5.960464477539063e-8);
        assert_eq!(round_to_f16(2.980232238769531e-8), 0.0);
        assert_eq!(round_to_f16(1.0009765625), 1.0009765625);
        assert_eq!(round_to_f16(1.00048828125), 1.0);
    }

    #[test]
    fn exact_sum() {
        let sum = |xs: &[f64]| {
            let mut s = ExactSum::default();
            for &x in xs {
                s.add(x);
            }
            s.result()
        };
        assert_eq!(sum(&[1e308, 1e308, -1e308, 0.1, -0.1]), 1e308);
        assert_eq!(sum(&[0.1, 0.2, 0.3]), 0.6);
        assert!(sum(&[-0.0]).is_sign_negative());
        assert!(sum(&[]).is_sign_negative());
        assert_eq!(sum(&[1.0, 1e100, 1.0, -1e100]), 2.0);
        assert_eq!(sum(&[-5e-324, -5e-324]), -1e-323);
        assert_eq!(
            sum(&[1.7976931348623157e308, 1.7976931348623157e308]),
            f64::INFINITY
        );
        assert_eq!(sum(&[-3.0, 1.0]), -2.0);
    }
}
