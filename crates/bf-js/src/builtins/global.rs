//! El objeto global (§19): sus propiedades de valor, sus funciones (`eval`, `isFinite`,
//! `isNaN`, `parseFloat`, `parseInt`) y los constructores de esta etapa.

use crate::abstract_ops::conversion::trim_units;
use crate::heap::Gc;
use crate::object::Object;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

pub fn init_functions(agent: &mut Agent, realm: RealmId) {
    let e = agent.create_builtin_function(eval, 1, "eval", false);
    agent.realms[realm].intrinsics.eval = e;
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
    agent.put_data(
        global,
        PropertyKey::from("Infinity"),
        Value::Number(f64::INFINITY),
        false,
        false,
        false,
    );
    agent.put_data(
        global,
        PropertyKey::from("NaN"),
        Value::Number(f64::NAN),
        false,
        false,
        false,
    );
    agent.put_data(
        global,
        PropertyKey::from("undefined"),
        Value::Undefined,
        false,
        false,
        false,
    );
    agent.put_data(
        global,
        PropertyKey::from("eval"),
        Value::Object(i.eval),
        true,
        false,
        true,
    );
    crate::builtins::method(agent, global, "isFinite", 1, is_finite);
    crate::builtins::method(agent, global, "isNaN", 1, is_nan);
    crate::builtins::method(agent, global, "parseFloat", 1, parse_float);
    crate::builtins::method(agent, global, "parseInt", 2, parse_int);
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
        // E2
        ("RegExp", i.regexp_ctor),
        ("BigInt", i.bigint_ctor),
        ("Date", i.date_ctor),
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

/// parseFloat ( string ) (§19.2.4): el prefijo más largo que sea un StrDecimalLiteral.
fn parse_float(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = agent.to_string(&a.arg(0))?;
    let u = s.units();
    let start = u.len()
        - trim_units(u).len()
        - (u.len() - u.iter().rposition(|_| true).map_or(0, |e| e + 1));
    let t: Vec<u16> = {
        // Solo se quita el espacio del principio.
        let first = u
            .iter()
            .position(|&c| {
                !char::from_u32(u32::from(c)).is_some_and(|ch| {
                    crate::lexer::is_whitespace(ch) || crate::lexer::is_line_terminator(ch)
                })
            })
            .unwrap_or(u.len());
        let _ = start;
        u[first..].to_vec()
    };
    let text: String = t
        .iter()
        .map_while(|&c| char::from_u32(u32::from(c)).filter(|ch| ch.is_ascii()))
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
    let mantissa_end = i;
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
    let _ = mantissa_end;
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
    let u = s.units();
    let first = u
        .iter()
        .position(|&c| {
            !char::from_u32(u32::from(c)).is_some_and(|ch| {
                crate::lexer::is_whitespace(ch) || crate::lexer::is_line_terminator(ch)
            })
        })
        .unwrap_or(u.len());
    let mut t = &u[first..];
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
        2 | 8 | 16 => crate::lexer::number::radix_to_f64(&digits, radix as u32),
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
