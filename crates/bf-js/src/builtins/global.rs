//! El objeto global (§19): sus propiedades de valor, sus funciones (`eval`, `isFinite`,
//! `isNaN`, `parseFloat`, `parseInt`, `encodeURI`, `encodeURIComponent`, `decodeURI`,
//! `decodeURIComponent` y `escape`/`unescape` de B.2.1) y los constructores que registra cada
//! módulo de built-ins con [`crate::builtins::register_global`].

use crate::heap::Gc;
use crate::object::Object;
use crate::string::JsString;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, ErrorType, JsResult, RealmId};

/// Las funciones globales que otros objetos comparten (`Number.parseFloat` es el mismo objeto
/// que `parseFloat`): se crean antes que los constructores.
pub fn init_functions(agent: &mut Agent, realm: RealmId) {
    let e = agent.create_builtin_function(eval, 1, "eval", false);
    let pf = agent.create_builtin_function(parse_float, 1, "parseFloat", false);
    let pi = agent.create_builtin_function(parse_int, 2, "parseInt", false);
    let i = &mut agent.realms[realm].intrinsics;
    i.eval = e;
    i.parse_float = pf;
    i.parse_int = pi;
}

/// SetDefaultGlobalBindings (§9.3.4).
pub fn init_global_object(agent: &mut Agent, global: Gc<Object>) {
    let i = agent.intrinsics().clone();
    agent.put_data(
        global,
        PropertyKey::from("globalThis"),
        Value::Object(global),
        true,
        false,
        true,
    );
    for (name, v) in [
        ("Infinity", Value::Number(f64::INFINITY)),
        ("NaN", Value::Number(f64::NAN)),
        ("undefined", Value::Undefined),
    ] {
        agent.put_data(global, PropertyKey::from(name), v, false, false, false);
    }
    for (name, f) in [
        ("eval", i.eval),
        ("parseFloat", i.parse_float),
        ("parseInt", i.parse_int),
    ] {
        agent.put_data(
            global,
            PropertyKey::from(name),
            Value::Object(f),
            true,
            false,
            true,
        );
    }
    crate::builtins::methods(
        agent,
        global,
        &[
            ("decodeURI", 1, decode_uri),
            ("decodeURIComponent", 1, decode_uri_component),
            ("encodeURI", 1, encode_uri),
            ("encodeURIComponent", 1, encode_uri_component),
            ("escape", 1, escape),
            ("isFinite", 1, is_finite),
            ("isNaN", 1, is_nan),
            ("unescape", 1, unescape),
        ],
    );
    let ctors = [
        ("AggregateError", i.aggregate_error_ctor),
        ("Array", i.array_ctor),
        ("Boolean", i.boolean_ctor),
        ("Error", i.error_ctor),
        ("EvalError", i.eval_error_ctor),
        ("Function", i.function_ctor),
        ("Number", i.number_ctor),
        ("Object", i.object_ctor),
        ("RangeError", i.range_error_ctor),
        ("ReferenceError", i.reference_error_ctor),
        ("String", i.string_ctor),
        ("Symbol", i.symbol_ctor),
        ("SyntaxError", i.syntax_error_ctor),
        ("TypeError", i.type_error_ctor),
        ("URIError", i.uri_error_ctor),
        ("Math", i.math),
        ("Reflect", i.reflect),
    ];
    for (name, o) in ctors {
        agent.put_data(
            global,
            PropertyKey::from(name),
            Value::Object(o),
            true,
            false,
            true,
        );
    }
    let realm = agent.current_realm;
    let extra = agent.realms[realm].globals.clone();
    for (name, o) in extra {
        agent.put_data(
            global,
            PropertyKey::from(name),
            Value::Object(o),
            true,
            false,
            true,
        );
    }
}

/// eval ( x ) (§19.2.1): eval indirecto.
fn eval(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    agent.perform_eval(&a.arg(0), false, crate::compiler::EvalFlags::default())
}

/// isFinite ( number ) (§19.2.2).
fn is_finite(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = agent.to_number(&a.arg(0))?;
    Ok(Value::Boolean(n.is_finite()))
}

/// isNaN ( number ) (§19.2.3).
fn is_nan(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = agent.to_number(&a.arg(0))?;
    Ok(Value::Boolean(n.is_nan()))
}

fn is_str_whitespace(c: u16) -> bool {
    char::from_u32(u32::from(c))
        .is_some_and(|ch| crate::lexer::is_whitespace(ch) || crate::lexer::is_line_terminator(ch))
}

/// Los code units sin el StrWhiteSpaceChar del principio (TrimString(…, start)).
fn trim_start(u: &[u16]) -> &[u16] {
    let first = u
        .iter()
        .position(|&c| !is_str_whitespace(c))
        .unwrap_or(u.len());
    &u[first..]
}

/// parseFloat ( string ) (§19.2.4): el prefijo más largo que sea un StrDecimalLiteral.
fn parse_float(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = agent.to_string(&a.arg(0))?;
    let text: String = trim_start(s.units())
        .iter()
        .map_while(|&c| char::from_u32(u32::from(c)).filter(char::is_ascii))
        .collect();
    let b = text.as_bytes();
    let mut i = 0;
    let mut sign = 1.0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        if b[i] == b'-' {
            sign = -1.0;
        }
        i += 1;
    }
    if text[i..].starts_with("Infinity") {
        return Ok(Value::Number(sign * f64::INFINITY));
    }
    let digits_start = i;
    let mut int_digits = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        int_digits += 1;
    }
    let mut frac_digits = 0;
    if i < b.len() && b[i] == b'.' {
        let save = i;
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            frac_digits += 1;
        }
        if frac_digits == 0 && int_digits == 0 {
            i = save;
        }
    }
    if int_digits == 0 && frac_digits == 0 {
        return Ok(Value::Number(f64::NAN));
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let exp_start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > exp_start {
            i = j;
        }
    }
    let mut lit = text[digits_start..i].to_owned();
    if lit.starts_with('.') {
        lit.insert(0, '0');
    }
    let lit = lit.replace(".e", ".0e").replace(".E", ".0E");
    let lit = if lit.ends_with('.') {
        format!("{lit}0")
    } else {
        lit
    };
    let v: f64 = lit.parse().unwrap_or(f64::NAN);
    Ok(Value::Number(sign * v))
}

/// parseInt ( string, radix ) (§19.2.5).
fn parse_int(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = agent.to_string(&a.arg(0))?;
    let r = agent.to_int32(&a.arg(1))?;
    let mut t = trim_start(s.units());
    let mut sign = 1.0;
    if let Some(&c) = t.first()
        && (c == u16::from(b'+') || c == u16::from(b'-'))
    {
        if c == u16::from(b'-') {
            sign = -1.0;
        }
        t = &t[1..];
    }
    let mut radix = r;
    let mut strip_prefix = true;
    if radix != 0 {
        if !(2..=36).contains(&radix) {
            return Ok(Value::Number(f64::NAN));
        }
        if radix != 16 {
            strip_prefix = false;
        }
    } else {
        radix = 10;
    }
    if strip_prefix
        && t.len() >= 2
        && t[0] == u16::from(b'0')
        && (t[1] == u16::from(b'x') || t[1] == u16::from(b'X'))
    {
        t = &t[2..];
        radix = 16;
    }
    let digits: String = t
        .iter()
        .map_while(|&c| char::from_u32(u32::from(c)).filter(|ch| ch.is_digit(radix as u32)))
        .collect();
    if digits.is_empty() {
        return Ok(Value::Number(f64::NAN));
    }
    let v = match radix {
        10 => digits.parse::<f64>().unwrap_or(f64::NAN),
        // Las bases potencia de dos son exactas (redondeo al par).
        2 | 4 | 8 | 16 | 32 => pow2_radix_to_f64(&digits, radix.trailing_zeros()),
        _ => {
            let mut v = 0.0;
            for c in digits.chars() {
                v = v * f64::from(radix) + f64::from(c.to_digit(radix as u32).unwrap_or(0));
            }
            v
        }
    };
    Ok(Value::Number(sign * v))
}

/// Un entero en base 2^`bits` redondeado al Number más cercano (empates al par).
fn pow2_radix_to_f64(digits: &str, bits: u32) -> f64 {
    let mut v: Vec<bool> = Vec::new();
    for c in digits.chars() {
        let d = c.to_digit(1 << bits).unwrap_or(0);
        for i in (0..bits).rev() {
            v.push((d >> i) & 1 == 1);
        }
    }
    let Some(first) = v.iter().position(|&b| b) else {
        return 0.0;
    };
    let v = &v[first..];
    if v.len() <= 64 {
        return v.iter().fold(0u64, |acc, &b| acc << 1 | u64::from(b)) as f64;
    }
    let mut m = v[..53].iter().fold(0u64, |acc, &b| acc << 1 | u64::from(b));
    let round = v[53];
    let sticky = v[54..].iter().any(|&b| b);
    let mut exp = (v.len() - 53) as i32;
    if round && (sticky || m & 1 == 1) {
        m += 1;
        if m == 1 << 53 {
            m >>= 1;
            exp += 1;
        }
    }
    if exp > 1023 - 52 {
        return f64::INFINITY;
    }
    m as f64 * 2f64.powi(exp)
}

// --- URI (§19.2.6) ---

const URI_RESERVED: &str = ";/?:@&=+$,";
const URI_MARK: &str = "-_.!~*'()";

fn uri_unescaped(c: u16, extra: &str) -> bool {
    let Some(ch) = char::from_u32(u32::from(c)) else {
        return false;
    };
    ch.is_ascii_alphanumeric() || URI_MARK.contains(ch) || extra.contains(ch)
}

/// Encode (§19.2.6.5).
fn encode(agent: &mut Agent, s: &JsString, extra: &str) -> JsResult<Value> {
    let u = s.units();
    let mut out: Vec<u16> = Vec::with_capacity(u.len());
    let mut k = 0;
    while k < u.len() {
        let c = u[k];
        if uri_unescaped(c, extra) {
            out.push(c);
            k += 1;
            continue;
        }
        let (cp, n) = crate::builtins::string::code_point_at_units(u, k);
        if (0xD800..0xE000).contains(&cp) {
            return Err(agent.error(ErrorType::UriError, "URI malformed"));
        }
        let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
        let mut buf = [0u8; 4];
        for b in ch.encode_utf8(&mut buf).bytes() {
            for x in format!("%{b:02X}").encode_utf16() {
                out.push(x);
            }
        }
        k += n;
    }
    Ok(Value::String(JsString::from_units(out)))
}

fn hex_byte(u: &[u16], k: usize) -> Option<u8> {
    let h = |c: u16| char::from_u32(u32::from(c)).and_then(|ch| ch.to_digit(16));
    let a = h(*u.get(k)?)?;
    let b = h(*u.get(k + 1)?)?;
    Some((a * 16 + b) as u8)
}

/// Decode (§19.2.6.6).
fn decode(agent: &mut Agent, s: &JsString, preserve: &str) -> JsResult<Value> {
    let u = s.units();
    let mut out: Vec<u16> = Vec::with_capacity(u.len());
    let mut k = 0;
    let malformed = |agent: &mut Agent| Err(agent.error(ErrorType::UriError, "URI malformed"));
    while k < u.len() {
        if u[k] != u16::from(b'%') {
            out.push(u[k]);
            k += 1;
            continue;
        }
        let start = k;
        let Some(b) = hex_byte(u, k + 1) else {
            return malformed(agent);
        };
        k += 3;
        if b < 0x80 {
            let ch = char::from(b);
            if preserve.contains(ch) {
                out.extend_from_slice(&u[start..k]);
            } else {
                out.push(u16::from(b));
            }
            continue;
        }
        let n = b.leading_ones() as usize;
        if n == 1 || n > 4 {
            return malformed(agent);
        }
        let mut bytes = vec![b];
        for _ in 1..n {
            if u.get(k) != Some(&u16::from(b'%')) {
                return malformed(agent);
            }
            let Some(c) = hex_byte(u, k + 1) else {
                return malformed(agent);
            };
            if c & 0xC0 != 0x80 {
                return malformed(agent);
            }
            bytes.push(c);
            k += 3;
        }
        // `from_utf8` rechaza las formas largas, los surrogates y > U+10FFFF.
        match std::str::from_utf8(&bytes) {
            Ok(st) => out.extend(st.encode_utf16()),
            Err(_) => return malformed(agent),
        }
    }
    Ok(Value::String(JsString::from_units(out)))
}

fn decode_uri(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = agent.to_string(&a.arg(0))?;
    decode(agent, &s, ";/?:@&=+$,#")
}

fn decode_uri_component(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = agent.to_string(&a.arg(0))?;
    decode(agent, &s, "")
}

fn encode_uri(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = agent.to_string(&a.arg(0))?;
    let extra = format!("{URI_RESERVED}#");
    encode(agent, &s, &extra)
}

fn encode_uri_component(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = agent.to_string(&a.arg(0))?;
    encode(agent, &s, "")
}

// --- B.2.1 ---

/// escape ( string ) (B.2.1.1).
fn escape(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = agent.to_string(&a.arg(0))?;
    let mut out: Vec<u16> = Vec::with_capacity(s.len());
    for &c in s.units() {
        let plain = char::from_u32(u32::from(c))
            .is_some_and(|ch| ch.is_ascii_alphanumeric() || "@*_+-./".contains(ch));
        if plain {
            out.push(c);
        } else if c < 256 {
            out.extend(format!("%{c:02X}").encode_utf16());
        } else {
            out.extend(format!("%u{c:04X}").encode_utf16());
        }
    }
    Ok(Value::String(JsString::from_units(out)))
}

/// unescape ( string ) (B.2.1.2).
fn unescape(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = agent.to_string(&a.arg(0))?;
    let u = s.units();
    let mut out: Vec<u16> = Vec::with_capacity(u.len());
    let hex = |c: u16| char::from_u32(u32::from(c)).and_then(|ch| ch.to_digit(16));
    let mut k = 0;
    while k < u.len() {
        let c = u[k];
        if c == u16::from(b'%') {
            if u.get(k + 1) == Some(&u16::from(b'u'))
                && k + 6 <= u.len()
                && u[k + 2..k + 6].iter().all(|&d| hex(d).is_some())
            {
                let v = u[k + 2..k + 6]
                    .iter()
                    .fold(0u32, |acc, &d| acc * 16 + hex(d).unwrap_or(0));
                out.push(v as u16);
                k += 6;
                continue;
            }
            if k + 3 <= u.len() && u[k + 1..k + 3].iter().all(|&d| hex(d).is_some()) {
                let v = hex(u[k + 1]).unwrap_or(0) * 16 + hex(u[k + 2]).unwrap_or(0);
                out.push(v as u16);
                k += 3;
                continue;
            }
        }
        out.push(c);
        k += 1;
    }
    Ok(Value::String(JsString::from_units(out)))
}
