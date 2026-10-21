//! Number (§21.1): el constructor, sus constantes y funciones, y Number.prototype con
//! `toExponential`, `toFixed` y `toPrecision` sobre la expansión decimal exacta del valor.

use crate::abstract_ops::number::{number_to_std_string, number_to_string, number_to_string_radix};
use crate::builtins::{FnEntry, constructor, frozen_prop, methods, value_prop};
use crate::object::{Object, ObjectKind};
use crate::value::Value;
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

pub fn init(agent: &mut Agent, realm: RealmId) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    let proto = agent.alloc(Object::new(Some(op), ObjectKind::Number(0.0)));
    agent.realms[realm].intrinsics.number_prototype = proto;
    let ctor = constructor(agent, "Number", 1, number_ctor, proto);
    agent.realms[realm].intrinsics.number_ctor = ctor;
    for (name, v) in [
        ("EPSILON", f64::EPSILON),
        ("MAX_SAFE_INTEGER", 9007199254740991.0),
        ("MAX_VALUE", f64::MAX),
        ("MIN_SAFE_INTEGER", -9007199254740991.0),
        ("MIN_VALUE", 5e-324),
        ("NaN", f64::NAN),
        ("NEGATIVE_INFINITY", f64::NEG_INFINITY),
        ("POSITIVE_INFINITY", f64::INFINITY),
    ] {
        frozen_prop(agent, ctor, name, Value::Number(v));
    }
    const STATICS: &[FnEntry] = &[
        ("isFinite", 1, is_finite),
        ("isInteger", 1, is_integer),
        ("isNaN", 1, is_nan),
        ("isSafeInteger", 1, is_safe_integer),
    ];
    methods(agent, ctor, STATICS);
    // Number.parseFloat y Number.parseInt son los mismos objetos que los globales
    // (§21.1.2.12–13).
    let (pf, pi) = {
        let i = &agent.realms[realm].intrinsics;
        (i.parse_float, i.parse_int)
    };
    value_prop(agent, ctor, "parseFloat", Value::Object(pf));
    value_prop(agent, ctor, "parseInt", Value::Object(pi));
    const PROTO: &[FnEntry] = &[
        ("toExponential", 1, to_exponential),
        ("toFixed", 1, to_fixed),
        ("toLocaleString", 0, to_locale_string),
        ("toPrecision", 1, to_precision),
        ("toString", 1, to_string),
        ("valueOf", 0, value_of),
    ];
    methods(agent, proto, PROTO);
}

/// Number ( value ) (§21.1.1.1).
fn number_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = if a.is_empty() {
        0.0
    } else {
        match agent.to_numeric(&a.arg(0))? {
            Value::BigInt(b) => agent.heap[b].to_f64(),
            Value::Number(n) => n,
            _ => unreachable!("ToNumeric"),
        }
    };
    let Some(nt) = a.new_target else {
        return Ok(Value::Number(n));
    };
    let o = agent.ordinary_create_from_constructor(
        nt,
        |i| i.number_prototype,
        ObjectKind::Number(n),
    )?;
    Ok(Value::Object(o))
}

/// Number.isFinite (§21.1.2.2).
fn is_finite(_: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(
        matches!(a.arg(0), Value::Number(n) if n.is_finite()),
    ))
}

/// Number.isInteger (§21.1.2.3).
fn is_integer(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(agent.is_integral_number(&a.arg(0))))
}

/// Number.isNaN (§21.1.2.4).
fn is_nan(_: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(
        matches!(a.arg(0), Value::Number(n) if n.is_nan()),
    ))
}

/// Number.isSafeInteger (§21.1.2.5).
fn is_safe_integer(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = a.arg(0);
    Ok(Value::Boolean(
        agent.is_integral_number(&v)
            && matches!(v, Value::Number(n) if n.abs() <= 9007199254740991.0),
    ))
}

/// ThisNumberValue (§21.1.3.7.1).
fn this_number(agent: &mut Agent, v: &Value) -> JsResult<f64> {
    match v {
        Value::Number(n) => Ok(*n),
        Value::Object(o) => match agent.heap[*o].kind {
            ObjectKind::Number(n) => Ok(n),
            _ => agent.type_error("Number.prototype method called on incompatible receiver"),
        },
        _ => agent.type_error("Number.prototype method called on incompatible receiver"),
    }
}

/// Number.prototype.toString ( [ radix ] ) (§21.1.3.6).
fn to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let x = this_number(agent, &a.this)?;
    let radix = if a.arg(0).is_undefined() {
        10.0
    } else {
        agent.to_integer_or_infinity(&a.arg(0))?
    };
    if !(2.0..=36.0).contains(&radix) {
        return agent.range_error("toString() radix must be between 2 and 36");
    }
    if radix == 10.0 {
        return Ok(Value::String(number_to_string(x)));
    }
    Ok(Value::str(&number_to_string_radix(x, radix as u32)))
}

/// Number.prototype.toLocaleString (ECMA-402 §19.2.1). // E2: sobre Intl.NumberFormat.
fn to_locale_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let x = this_number(agent, &a.this)?;
    crate::builtins::intl::number_to_locale_string(
        agent,
        crate::builtins::intl::number_format_value::Value::Number(x),
        &a.arg(0),
        &a.arg(1),
    )
}

fn value_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(this_number(agent, &a.this)?))
}

// --- Expansión decimal exacta ---

/// Los dígitos decimales exactos de un Number finito positivo: `x = D × 10^k` (D sin ceros
/// a la izquierda). Un double es m × 2^e: con e ≥ 0, D = m·2^e; con e < 0, D = m·5^−e y
/// k = e.
pub fn exact_decimal(x: f64) -> (Vec<u8>, i32) {
    let bits = x.to_bits();
    let biased = ((bits >> 52) & 0x7FF) as i32;
    let frac = bits & ((1u64 << 52) - 1);
    let (m, e) = if biased == 0 {
        (frac, -1074)
    } else {
        (frac | (1 << 52), biased - 1075)
    };
    // Entero grande en base 10^9 (del menos significativo al más).
    let mut limbs: Vec<u64> = vec![m % 1_000_000_000, m / 1_000_000_000];
    let (factor, times, k) = if e >= 0 { (2, e, 0) } else { (5, -e, e) };
    for _ in 0..times {
        let mut carry = 0u64;
        for l in &mut limbs {
            let v = *l * factor + carry;
            *l = v % 1_000_000_000;
            carry = v / 1_000_000_000;
        }
        if carry > 0 {
            limbs.push(carry);
        }
    }
    while limbs.len() > 1 && limbs.last() == Some(&0) {
        limbs.pop();
    }
    let mut s = limbs.last().copied().unwrap_or(0).to_string();
    for l in limbs.iter().rev().skip(1) {
        s.push_str(&format!("{l:09}"));
    }
    let mut d: Vec<u8> = s.bytes().map(|b| b - b'0').collect();
    let mut k = k;
    while d.len() > 1 && d.last() == Some(&0) {
        d.pop();
        k += 1;
    }
    (d, k)
}

/// Redondea (la mitad hacia arriba) los dígitos a `n` dígitos significativos: (dígitos, e)
/// con el primero en la posición 10^e.
fn round_significant(digits: &[u8], k: i32, n: usize) -> (Vec<u8>, i32) {
    let len = digits.len();
    let mut e = len as i32 - 1 + k;
    if len <= n {
        let mut d = digits.to_vec();
        d.resize(n, 0);
        return (d, e);
    }
    let mut d = digits[..n].to_vec();
    if digits[n] >= 5 {
        let mut i = n;
        loop {
            if i == 0 {
                d.insert(0, 1);
                d.pop();
                e += 1;
                break;
            }
            i -= 1;
            if d[i] == 9 {
                d[i] = 0;
            } else {
                d[i] += 1;
                break;
            }
        }
    }
    (d, e)
}

fn digits_str(d: &[u8]) -> String {
    d.iter().map(|&x| char::from(b'0' + x)).collect()
}

/// Number.prototype.toExponential (§21.1.3.2).
fn to_exponential(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let x = this_number(agent, &a.this)?;
    let fd = a.arg(0);
    let f = agent.to_integer_or_infinity(&fd)?;
    if !x.is_finite() {
        return Ok(Value::String(number_to_string(x)));
    }
    if !(0.0..=100.0).contains(&f) {
        return agent.range_error("toExponential() argument must be between 0 and 100");
    }
    let mut s = String::new();
    let x = if x < 0.0 {
        s.push('-');
        -x
    } else {
        x
    };
    let (digits, e) = if x == 0.0 {
        (vec![0; f as usize + 1], 0)
    } else if fd.is_undefined() {
        // Tantos dígitos como hagan falta para identificar el valor.
        shortest_digits(x)
    } else {
        let (d, k) = exact_decimal(x);
        round_significant(&d, k, f as usize + 1)
    };
    let ds = digits_str(&digits);
    s.push_str(&ds[..1]);
    if ds.len() > 1 {
        s.push('.');
        s.push_str(&ds[1..]);
    }
    s.push('e');
    s.push(if e >= 0 { '+' } else { '-' });
    s.push_str(&e.abs().to_string());
    Ok(Value::str(&s))
}

/// Los dígitos más cortos de Number::toString con el exponente del primero.
fn shortest_digits(x: f64) -> (Vec<u8>, i32) {
    let e = format!("{x:e}");
    let (mantissa, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let digits: Vec<u8> = mantissa
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(|b| b - b'0')
        .collect();
    (digits, exp.parse().unwrap_or(0))
}

/// Number.prototype.toFixed (§21.1.3.3).
fn to_fixed(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let x = this_number(agent, &a.this)?;
    let f = agent.to_integer_or_infinity(&a.arg(0))?;
    if !f.is_finite() || !(0.0..=100.0).contains(&f) {
        return agent.range_error("toFixed() digits argument must be between 0 and 100");
    }
    let f = f as usize;
    if !x.is_finite() {
        return Ok(Value::String(number_to_string(x)));
    }
    if x.abs() >= 1e21 {
        return Ok(Value::str(&number_to_std_string(x)));
    }
    let mut s = String::new();
    let x = if x < 0.0 {
        s.push('-');
        -x
    } else {
        x
    };
    // n = round(x × 10^f) con empates hacia arriba.
    let n: Vec<u8> = if x == 0.0 {
        vec![0]
    } else {
        let (d, k) = exact_decimal(x);
        let shift = k + f as i32;
        if shift >= 0 {
            let mut v = d.clone();
            v.extend(std::iter::repeat_n(0, shift as usize));
            v
        } else {
            let cut = (-shift) as usize;
            if cut > d.len() {
                vec![0]
            } else {
                let keep = d.len() - cut;
                let mut v: Vec<u8> = d[..keep].to_vec();
                if d[keep] >= 5 {
                    let mut i = v.len();
                    loop {
                        if i == 0 {
                            v.insert(0, 1);
                            break;
                        }
                        i -= 1;
                        if v[i] == 9 {
                            v[i] = 0;
                        } else {
                            v[i] += 1;
                            break;
                        }
                    }
                }
                if v.is_empty() { vec![0] } else { v }
            }
        }
    };
    let mut m = digits_str(&n);
    if f > 0 {
        if m.len() <= f {
            m = format!("{}{m}", "0".repeat(f + 1 - m.len()));
        }
        let split = m.len() - f;
        m = format!("{}.{}", &m[..split], &m[split..]);
    }
    s.push_str(&m);
    Ok(Value::str(&s))
}

/// Number.prototype.toPrecision (§21.1.3.5).
fn to_precision(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let x = this_number(agent, &a.this)?;
    if a.arg(0).is_undefined() {
        return Ok(Value::String(number_to_string(x)));
    }
    let p = agent.to_integer_or_infinity(&a.arg(0))?;
    if !x.is_finite() {
        return Ok(Value::String(number_to_string(x)));
    }
    if !(1.0..=100.0).contains(&p) {
        return agent.range_error("toPrecision() argument must be between 1 and 100");
    }
    let p = p as usize;
    let mut s = String::new();
    let x = if x < 0.0 {
        s.push('-');
        -x
    } else {
        x
    };
    let (digits, e) = if x == 0.0 {
        (vec![0; p], 0)
    } else {
        let (d, k) = exact_decimal(x);
        round_significant(&d, k, p)
    };
    let m = digits_str(&digits);
    if x != 0.0 && (e < -6 || e >= p as i32) {
        s.push_str(&m[..1]);
        if p > 1 {
            s.push('.');
            s.push_str(&m[1..]);
        }
        s.push('e');
        s.push(if e >= 0 { '+' } else { '-' });
        s.push_str(&e.abs().to_string());
        return Ok(Value::str(&s));
    }
    if e == p as i32 - 1 {
        s.push_str(&m);
    } else if e >= 0 {
        let i = e as usize + 1;
        s.push_str(&m[..i]);
        s.push('.');
        s.push_str(&m[i..]);
    } else {
        s.push_str("0.");
        s.push_str(&"0".repeat((-(e + 1)) as usize));
        s.push_str(&m);
    }
    Ok(Value::str(&s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_expansion() {
        assert_eq!(exact_decimal(0.5), (vec![5], -1));
        let (d, k) = exact_decimal(0.1);
        assert_eq!(k, -55);
        assert_eq!(
            digits_str(&d),
            "1000000000000000055511151231257827021181583404541015625"
        );
        assert_eq!(exact_decimal(1024.0), (vec![1, 0, 2, 4], 0));
        assert_eq!(exact_decimal(1e21), (vec![1], 21));
        assert_eq!(round_significant(&[9, 9, 9], 0, 2), (vec![1, 0], 3));
    }
}
