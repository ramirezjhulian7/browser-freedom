//! String (§22.1): el constructor, `fromCharCode` y los métodos de String.prototype de esta
//! etapa (sin expresiones regulares).

use crate::abstract_ops::conversion::trim_units;
use crate::builtins::iterator::create_string_iterator;
use crate::builtins::{FnEntry, constructor, method_sym, methods, this_string};
use crate::object::{Object, ObjectKind};
use crate::string::JsString;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

pub fn init(agent: &mut Agent, realm: RealmId) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    let proto = agent.alloc(Object::new(
        Some(op),
        ObjectKind::String(JsString::default()),
    ));
    let k = agent.keys.length.clone();
    agent.put_data(proto, k, Value::Number(0.0), false, false, false);
    agent.realms[realm].intrinsics.string_prototype = proto;
    let ctor = constructor(agent, "String", 1, string_ctor, proto);
    agent.realms[realm].intrinsics.string_ctor = ctor;
    const STATICS: &[FnEntry] = &[("fromCharCode", 1, from_char_code)];
    methods(agent, ctor, STATICS);
    const PROTO: &[FnEntry] = &[
        ("at", 1, at),
        ("charAt", 1, char_at),
        ("charCodeAt", 1, char_code_at),
        ("codePointAt", 1, code_point_at),
        ("endsWith", 1, ends_with),
        ("includes", 1, includes),
        ("indexOf", 1, index_of),
        ("repeat", 1, repeat),
        ("slice", 2, slice),
        ("split", 2, split),
        ("startsWith", 1, starts_with),
        ("substring", 2, substring),
        ("toLowerCase", 0, to_lower_case),
        ("toString", 0, to_string),
        ("toUpperCase", 0, to_upper_case),
        ("trim", 0, trim),
        ("valueOf", 0, value_of),
    ];
    methods(agent, proto, PROTO);
    let it = agent.well_known.iterator;
    method_sym(agent, proto, it, "[Symbol.iterator]", 0, iterator);
}

/// String ( value ) (§22.1.1.1).
fn string_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = if a.is_empty() {
        JsString::default()
    } else {
        let v = a.arg(0);
        if a.new_target.is_none()
            && let Value::Symbol(sym) = v
        {
            return Ok(Value::String(crate::builtins::symbol::descriptive_string(
                agent, sym,
            )));
        }
        agent.to_string(&v)?
    };
    let Some(nt) = a.new_target else {
        return Ok(Value::String(s));
    };
    let proto = agent.get_prototype_from_constructor(nt, |i| i.string_prototype)?;
    // StringCreate (§10.4.3.4).
    let len = s.len();
    let o = agent.alloc(Object::new(Some(proto), ObjectKind::String(s)));
    let k = agent.keys.length.clone();
    agent.put_data(o, k, Value::Number(len as f64), false, false, false);
    Ok(Value::Object(o))
}

/// String.fromCharCode (§22.1.2.1).
fn from_char_code(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let mut u = Vec::with_capacity(a.len());
    for v in a.args {
        let n = agent.to_number(v)?;
        u.push(crate::abstract_ops::conversion::to_uint16(n));
    }
    Ok(Value::String(JsString::from_units(u)))
}

/// `this` convertido a String (RequireObjectCoercible + ToString).
fn this_str(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<JsString> {
    agent.require_object_coercible(&a.this)?;
    agent.to_string(&a.this)
}

fn to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::String(this_string(
        agent,
        &a.this,
        "String.prototype.toString",
    )?))
}

fn value_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::String(this_string(
        agent,
        &a.this,
        "String.prototype.valueOf",
    )?))
}

fn unit_string(u: u16) -> Value {
    Value::String(JsString::from_units(vec![u]))
}

/// String.prototype.at (§22.1.3.1).
fn at(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    let len = s.len() as f64;
    let rel = agent.to_integer_or_infinity(&a.arg(0))?;
    let k = if rel >= 0.0 { rel } else { len + rel };
    if k < 0.0 || k >= len {
        return Ok(Value::Undefined);
    }
    Ok(unit_string(s.units()[k as usize]))
}

fn char_at(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    let p = agent.to_integer_or_infinity(&a.arg(0))?;
    if p < 0.0 || p >= s.len() as f64 {
        return Ok(Value::str(""));
    }
    Ok(unit_string(s.units()[p as usize]))
}

fn char_code_at(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    let p = agent.to_integer_or_infinity(&a.arg(0))?;
    if p < 0.0 || p >= s.len() as f64 {
        return Ok(Value::Number(f64::NAN));
    }
    Ok(Value::Number(f64::from(s.units()[p as usize])))
}

/// CodePointAt (§11.1.4) a partir de `pos`.
pub fn code_point_at_units(u: &[u16], pos: usize) -> (u32, usize) {
    let first = u[pos];
    if !(0xD800..0xDC00).contains(&first) || pos + 1 == u.len() {
        return (u32::from(first), 1);
    }
    let second = u[pos + 1];
    if !(0xDC00..0xE000).contains(&second) {
        return (u32::from(first), 1);
    }
    (
        ((u32::from(first) - 0xD800) << 10) + (u32::from(second) - 0xDC00) + 0x10000,
        2,
    )
}

fn code_point_at(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    let p = agent.to_integer_or_infinity(&a.arg(0))?;
    if p < 0.0 || p >= s.len() as f64 {
        return Ok(Value::Undefined);
    }
    let (cp, _) = code_point_at_units(s.units(), p as usize);
    Ok(Value::Number(f64::from(cp)))
}

/// La posición de `needle` en `hay` desde `from` (StringIndexOf, §6.1.4.1).
pub fn string_index_of(hay: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if needle.is_empty() && from <= hay.len() {
        return Some(from);
    }
    if needle.len() > hay.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

fn not_regexp(agent: &mut Agent, v: &Value, name: &str) -> JsResult<()> {
    if agent.is_regexp(v)? {
        return agent.type_error(&format!(
            "First argument to String.prototype.{name} must not be a regular expression"
        ));
    }
    Ok(())
}

/// String.prototype.endsWith (§22.1.3.7).
fn ends_with(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    not_regexp(agent, &a.arg(0), "endsWith")?;
    let search = agent.to_string(&a.arg(0))?;
    let len = s.len() as f64;
    let end = if a.arg(1).is_undefined() {
        len
    } else {
        agent.to_integer_or_infinity(&a.arg(1))?.clamp(0.0, len)
    } as usize;
    let sl = search.len();
    if sl > end {
        return Ok(Value::Boolean(false));
    }
    Ok(Value::Boolean(&s.units()[end - sl..end] == search.units()))
}

/// String.prototype.includes (§22.1.3.8).
fn includes(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    not_regexp(agent, &a.arg(0), "includes")?;
    let search = agent.to_string(&a.arg(0))?;
    let pos = agent
        .to_integer_or_infinity(&a.arg(1))?
        .clamp(0.0, s.len() as f64) as usize;
    Ok(Value::Boolean(
        string_index_of(s.units(), search.units(), pos).is_some(),
    ))
}

/// String.prototype.indexOf (§22.1.3.9).
fn index_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    let search = agent.to_string(&a.arg(0))?;
    let pos = agent
        .to_integer_or_infinity(&a.arg(1))?
        .clamp(0.0, s.len() as f64) as usize;
    Ok(Value::Number(
        string_index_of(s.units(), search.units(), pos).map_or(-1.0, |i| i as f64),
    ))
}

/// String.prototype.repeat (§22.1.3.18).
fn repeat(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    let n = agent.to_integer_or_infinity(&a.arg(0))?;
    if n < 0.0 || n == f64::INFINITY {
        return agent.range_error("Invalid count value");
    }
    if n == 0.0 || s.is_empty() {
        return Ok(Value::str(""));
    }
    if s.len() as f64 * n > (1u64 << 30) as f64 {
        return agent.range_error("Invalid string length");
    }
    Ok(Value::String(JsString::from_units(
        s.units().repeat(n as usize),
    )))
}

fn relative_index(v: f64, len: f64) -> f64 {
    if v == f64::NEG_INFINITY {
        0.0
    } else if v < 0.0 {
        (len + v).max(0.0)
    } else {
        v.min(len)
    }
}

/// String.prototype.slice (§22.1.3.21).
fn slice(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    let len = s.len() as f64;
    let from = relative_index(agent.to_integer_or_infinity(&a.arg(0))?, len);
    let to = if a.arg(1).is_undefined() {
        len
    } else {
        relative_index(agent.to_integer_or_infinity(&a.arg(1))?, len)
    };
    if from >= to {
        return Ok(Value::str(""));
    }
    Ok(Value::String(JsString::from_units(
        s.units()[from as usize..to as usize].to_vec(),
    )))
}

/// String.prototype.split (§22.1.3.23), sin @@split de RegExp todavía.
fn split(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    agent.require_object_coercible(&a.this)?;
    let separator = a.arg(0);
    let limit = a.arg(1);
    if !separator.is_nullish() {
        let k = PropertyKey::Symbol(agent.well_known.split);
        if let Some(splitter) = agent.get_method_of(&separator, &k)? {
            return agent.call(&splitter, separator, &[a.this.clone(), limit]);
        }
    }
    let s = agent.to_string(&a.this)?;
    let lim = if limit.is_undefined() {
        u32::MAX
    } else {
        agent.to_uint32(&limit)?
    };
    let r = agent.to_string(&separator)?;
    if lim == 0 {
        return Ok(Value::Object(agent.create_array_from_list(&[])?));
    }
    if separator.is_undefined() {
        return Ok(Value::Object(
            agent.create_array_from_list(&[Value::String(s)])?,
        ));
    }
    let su = s.units();
    let ru = r.units();
    if su.is_empty() {
        let list = if ru.is_empty() {
            vec![]
        } else {
            vec![Value::String(s.clone())]
        };
        return Ok(Value::Object(agent.create_array_from_list(&list)?));
    }
    let mut out = Vec::new();
    if ru.is_empty() {
        for &u in su.iter().take(lim as usize) {
            out.push(unit_string(u));
        }
        return Ok(Value::Object(agent.create_array_from_list(&out)?));
    }
    let mut p = 0;
    let mut q = 0;
    while let Some(e) = string_index_of(su, ru, q) {
        out.push(Value::String(JsString::from_units(su[p..e].to_vec())));
        if out.len() as u32 == lim {
            return Ok(Value::Object(agent.create_array_from_list(&out)?));
        }
        p = e + ru.len();
        q = p;
    }
    out.push(Value::String(JsString::from_units(su[p..].to_vec())));
    Ok(Value::Object(agent.create_array_from_list(&out)?))
}

/// String.prototype.startsWith (§22.1.3.24).
fn starts_with(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    not_regexp(agent, &a.arg(0), "startsWith")?;
    let search = agent.to_string(&a.arg(0))?;
    let start = agent
        .to_integer_or_infinity(&a.arg(1))?
        .clamp(0.0, s.len() as f64) as usize;
    let sl = search.len();
    if start + sl > s.len() {
        return Ok(Value::Boolean(false));
    }
    Ok(Value::Boolean(
        &s.units()[start..start + sl] == search.units(),
    ))
}

/// String.prototype.substring (§22.1.3.25).
fn substring(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    let len = s.len() as f64;
    let start = agent.to_integer_or_infinity(&a.arg(0))?.clamp(0.0, len);
    let end = if a.arg(1).is_undefined() {
        len
    } else {
        agent.to_integer_or_infinity(&a.arg(1))?.clamp(0.0, len)
    };
    let (from, to) = (start.min(end) as usize, start.max(end) as usize);
    Ok(Value::String(JsString::from_units(
        s.units()[from..to].to_vec(),
    )))
}

/// Convierte mayúsculas/minúsculas (Default Case Conversion) conservando los surrogates
/// sueltos.
fn convert_case(s: &JsString, upper: bool) -> JsString {
    let mut out: Vec<u16> = Vec::with_capacity(s.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut Vec<u16>| {
        if !run.is_empty() {
            let c = if upper {
                run.to_uppercase()
            } else {
                run.to_lowercase()
            };
            out.extend(c.encode_utf16());
            run.clear();
        }
    };
    for r in char::decode_utf16(s.units().iter().copied()) {
        match r {
            Ok(c) => run.push(c),
            Err(e) => {
                flush(&mut run, &mut out);
                out.push(e.unpaired_surrogate());
            }
        }
    }
    flush(&mut run, &mut out);
    JsString::from_units(out)
}

fn to_lower_case(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    Ok(Value::String(convert_case(&s, false)))
}

fn to_upper_case(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    Ok(Value::String(convert_case(&s, true)))
}

/// String.prototype.trim (§22.1.3.32).
fn trim(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    Ok(Value::String(JsString::from_units(
        trim_units(s.units()).to_vec(),
    )))
}

/// String.prototype [ @@iterator ] (§22.1.3.36).
fn iterator(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    Ok(create_string_iterator(agent, s))
}
