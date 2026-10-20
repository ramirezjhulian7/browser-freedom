//! String (§22.1) y los métodos de B.2.2: el constructor, `fromCharCode`, `fromCodePoint`,
//! `raw` y String.prototype completo. `match`, `matchAll`, `replace`, `replaceAll`, `search` y
//! `split` siguen el protocolo de `@@match`/`@@matchAll`/`@@replace`/`@@search`/`@@split`
//! (la parte de RegExp está en el built-in RegExp); la ruta de strings está aquí.

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
    const STATICS: &[FnEntry] = &[
        ("fromCharCode", 1, from_char_code),
        ("fromCodePoint", 1, from_code_point),
        ("raw", 1, raw),
    ];
    methods(agent, ctor, STATICS);
    const PROTO: &[FnEntry] = &[
        ("at", 1, at),
        ("charAt", 1, char_at),
        ("charCodeAt", 1, char_code_at),
        ("codePointAt", 1, code_point_at),
        ("concat", 1, concat),
        ("endsWith", 1, ends_with),
        ("includes", 1, includes),
        ("indexOf", 1, index_of),
        ("isWellFormed", 0, is_well_formed),
        ("lastIndexOf", 1, last_index_of),
        ("localeCompare", 1, locale_compare),
        ("match", 1, match_),
        ("matchAll", 1, match_all),
        ("normalize", 0, normalize),
        ("padEnd", 1, pad_end),
        ("padStart", 1, pad_start),
        ("repeat", 1, repeat),
        ("replace", 2, replace),
        ("replaceAll", 2, replace_all),
        ("search", 1, search),
        ("slice", 2, slice),
        ("split", 2, split),
        ("startsWith", 1, starts_with),
        ("substring", 2, substring),
        ("toLocaleLowerCase", 0, to_lower_case),
        ("toLocaleUpperCase", 0, to_upper_case),
        ("toLowerCase", 0, to_lower_case),
        ("toString", 0, to_string),
        ("toUpperCase", 0, to_upper_case),
        ("toWellFormed", 0, to_well_formed),
        ("trim", 0, trim),
        ("valueOf", 0, value_of),
        // B.2.2
        ("substr", 2, substr),
        ("anchor", 1, anchor),
        ("big", 0, big),
        ("blink", 0, blink),
        ("bold", 0, bold),
        ("fixed", 0, fixed),
        ("fontcolor", 1, fontcolor),
        ("fontsize", 1, fontsize),
        ("italics", 0, italics),
        ("link", 1, link),
        ("small", 0, small),
        ("strike", 0, strike),
        ("sub", 0, sub),
        ("sup", 0, sup),
    ];
    methods(agent, proto, PROTO);
    // trimStart/trimEnd y sus alias de B.2.2.15–16 (el mismo objeto función).
    let ts = crate::builtins::method(agent, proto, "trimStart", 0, trim_start);
    let te = crate::builtins::method(agent, proto, "trimEnd", 0, trim_end);
    crate::builtins::value_prop(agent, proto, "trimLeft", Value::Object(ts));
    crate::builtins::value_prop(agent, proto, "trimRight", Value::Object(te));
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

/// UTF16EncodeCodePoint (§11.1.1).
pub fn push_code_point(out: &mut Vec<u16>, cp: u32) {
    if cp < 0x10000 {
        out.push(cp as u16);
    } else {
        let c = cp - 0x10000;
        out.push((0xD800 + (c >> 10)) as u16);
        out.push((0xDC00 + (c & 0x3FF)) as u16);
    }
}

/// String.fromCodePoint (§22.1.2.2).
fn from_code_point(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let mut u = Vec::with_capacity(a.len());
    for v in a.args {
        let n = agent.to_number(v)?;
        if !(n.is_finite() && n.trunc() == n) || !(0.0..=1114111.0).contains(&n) {
            return agent.range_error(&format!(
                "Invalid code point {}",
                crate::abstract_ops::number::number_to_std_string(n)
            ));
        }
        push_code_point(&mut u, n as u32);
    }
    Ok(Value::String(JsString::from_units(u)))
}

/// String.raw (§22.1.2.4).
fn raw(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let cooked = agent.to_object(&a.arg(0))?;
    let raw_v = agent.get_str(cooked, "raw")?;
    let literals = agent.to_object(&raw_v)?;
    let count = agent.length_of_array_like(literals)?;
    if count <= 0.0 {
        return Ok(Value::str(""));
    }
    let mut out: Vec<u16> = Vec::new();
    let mut i = 0.0;
    loop {
        let seg = agent.get(
            literals,
            &PropertyKey::from_number(i),
            Value::Object(literals),
        )?;
        let s = agent.to_string(&seg)?;
        out.extend_from_slice(s.units());
        if i + 1.0 == count {
            return Ok(Value::String(JsString::from_units(out)));
        }
        if let Some(sub) = a.args.get(i as usize + 1) {
            let s = agent.to_string(sub)?;
            out.extend_from_slice(s.units());
        }
        i += 1.0;
    }
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

fn sub_string(s: &JsString, from: usize, to: usize) -> Value {
    Value::String(JsString::from_units(s.units()[from..to].to_vec()))
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

/// CodePointAt (§11.1.4) a partir de `pos`: (code point, code units).
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

/// String.prototype.concat (§22.1.3.5).
fn concat(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    let mut out = s.units().to_vec();
    for v in a.args {
        let n = agent.to_string(v)?;
        out.extend_from_slice(n.units());
    }
    Ok(Value::String(JsString::from_units(out)))
}

/// StringIndexOf (§6.1.4.1): la posición de `needle` en `hay` desde `from`.
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

/// String.prototype.isWellFormed (§22.1.3.10).
fn is_well_formed(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    Ok(Value::Boolean(s.is_well_formed()))
}

/// String.prototype.lastIndexOf (§22.1.3.11).
fn last_index_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    let search = agent.to_string(&a.arg(0))?;
    let num_pos = agent.to_number(&a.arg(1))?;
    let pos = if num_pos.is_nan() {
        f64::INFINITY
    } else {
        crate::abstract_ops::conversion::integer_or_infinity(num_pos)
    };
    let len = s.len();
    let sl = search.len();
    if sl > len {
        return Ok(Value::Number(-1.0));
    }
    let start = pos.clamp(0.0, (len - sl) as f64) as usize;
    let (h, n) = (s.units(), search.units());
    for i in (0..=start).rev() {
        if &h[i..i + sl] == n {
            return Ok(Value::Number(i as f64));
        }
    }
    Ok(Value::Number(-1.0))
}

/// La forma normalizada (code point a code point; los surrogates sueltos quedan igual).
fn normalize_units(s: &[u16], form: &str) -> Vec<u16> {
    use unicode_normalization::UnicodeNormalization;
    let mut out = Vec::with_capacity(s.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut Vec<u16>| {
        if run.is_empty() {
            return;
        }
        let n: String = match form {
            "NFC" => run.nfc().collect(),
            "NFD" => run.nfd().collect(),
            "NFKC" => run.nfkc().collect(),
            _ => run.nfkd().collect(),
        };
        out.extend(n.encode_utf16());
        run.clear();
    };
    for r in char::decode_utf16(s.iter().copied()) {
        match r {
            Ok(c) => run.push(c),
            Err(e) => {
                flush(&mut run, &mut out);
                out.push(e.unpaired_surrogate());
            }
        }
    }
    flush(&mut run, &mut out);
    out
}

/// String.prototype.localeCompare (§22.1.3.12) sin Intl: compara las formas NFC code
/// point a code point (las cadenas canónicamente equivalentes dan 0).
fn locale_compare(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    let that = agent.to_string(&a.arg(0))?;
    let x = normalize_units(s.units(), "NFC");
    let y = normalize_units(that.units(), "NFC");
    let cps = |u: &[u16]| -> Vec<u32> {
        char::decode_utf16(u.iter().copied())
            .map(|r| r.map_or_else(|e| u32::from(e.unpaired_surrogate()), u32::from))
            .collect()
    };
    let r = match cps(&x).cmp(&cps(&y)) {
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Equal => 0.0,
        std::cmp::Ordering::Greater => 1.0,
    };
    Ok(Value::Number(r))
}

/// La función `RegExp` del realm (la registra el built-in RegExp como global).
fn regexp_create(agent: &mut Agent, pattern: Value, flags: Option<&str>) -> JsResult<Value> {
    let realm = agent.current_realm;
    let ctor = agent.realms[realm]
        .globals
        .iter()
        .find(|(n, _)| *n == "RegExp")
        .map(|(_, o)| *o);
    let Some(ctor) = ctor else {
        return agent.internal_error("RegExp (built-in de E2)");
    };
    let mut args = vec![pattern];
    if let Some(f) = flags {
        args.push(Value::str(f));
    }
    agent.construct(ctor, &args, None)
}

/// match / matchAll / search: el método del símbolo o RegExpCreate + Invoke (§22.1.3.13,
/// §22.1.3.14, §22.1.3.22).
fn regexp_dispatch(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    sym: crate::heap::Gc<crate::value::Symbol>,
    all: bool,
) -> JsResult<Value> {
    agent.require_object_coercible(&a.this)?;
    let regexp = a.arg(0);
    // Solo los objetos: los primitivos no buscan los símbolos (ES2025).
    if regexp.is_object() {
        if all && agent.is_regexp(&regexp)? {
            let flags = agent.get_value_of(&regexp, &PropertyKey::from("flags"))?;
            agent.require_object_coercible(&flags)?;
            let f = agent.to_string(&flags)?;
            if !f.units().contains(&u16::from(b'g')) {
                return agent.type_error(
                    "String.prototype.matchAll called with a non-global RegExp argument",
                );
            }
        }
        if let Some(m) = agent.get_method_of(&regexp, &PropertyKey::Symbol(sym))? {
            return agent.call(&m, regexp, std::slice::from_ref(&a.this));
        }
    }
    let s = agent.to_string(&a.this)?;
    let rx = regexp_create(agent, regexp, if all { Some("g") } else { None })?;
    agent.invoke(&rx, &PropertyKey::Symbol(sym), &[Value::String(s)])
}

fn match_(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let sym = agent.well_known.match_;
    regexp_dispatch(agent, a, sym, false)
}

fn match_all(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let sym = agent.well_known.match_all;
    regexp_dispatch(agent, a, sym, true)
}

fn search(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let sym = agent.well_known.search;
    regexp_dispatch(agent, a, sym, false)
}

/// String.prototype.normalize (§22.1.3.15).
fn normalize(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    let form = if a.arg(0).is_undefined() {
        "NFC".to_owned()
    } else {
        agent.to_string(&a.arg(0))?.to_string_lossy()
    };
    if !matches!(form.as_str(), "NFC" | "NFD" | "NFKC" | "NFKD") {
        return agent.range_error("The normalization form should be one of NFC, NFD, NFKC, NFKD.");
    }
    Ok(Value::String(JsString::from_units(normalize_units(
        s.units(),
        &form,
    ))))
}

/// StringPaddingBuiltinsImpl (§22.1.3.17.1).
fn pad(agent: &mut Agent, a: &CallArgs<'_>, at_start: bool) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    let max = agent.to_length(&a.arg(0))?;
    let len = s.len() as f64;
    if max <= len {
        return Ok(Value::String(s));
    }
    let filler = if a.arg(1).is_undefined() {
        JsString::from(" ")
    } else {
        agent.to_string(&a.arg(1))?
    };
    if filler.is_empty() {
        return Ok(Value::String(s));
    }
    let fill_len = max - len;
    if max > (1u64 << 30) as f64 {
        return agent.range_error("Invalid string length");
    }
    let f = filler.units();
    let fill: Vec<u16> = f.iter().copied().cycle().take(fill_len as usize).collect();
    let mut out = Vec::with_capacity(max as usize);
    if at_start {
        out.extend(fill);
        out.extend_from_slice(s.units());
    } else {
        out.extend_from_slice(s.units());
        out.extend(fill);
    }
    Ok(Value::String(JsString::from_units(out)))
}

fn pad_end(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    pad(agent, a, false)
}

fn pad_start(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    pad(agent, a, true)
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

/// GetSubstitution (§22.1.3.19.1). `captures` son los grupos (undefined o String) y
/// `named` el objeto de grupos con nombre (o undefined).
pub fn get_substitution(
    agent: &mut Agent,
    matched: &JsString,
    s: &JsString,
    position: usize,
    captures: &[Value],
    named: &Value,
    template: &JsString,
) -> JsResult<JsString> {
    let t = template.units();
    let su = s.units();
    let m = captures.len();
    let tail_pos = (position + matched.len()).min(su.len());
    let mut out: Vec<u16> = Vec::with_capacity(t.len());
    let mut i = 0;
    let dollar = u16::from(b'$');
    let is_digit = |c: u16| (u16::from(b'0')..=u16::from(b'9')).contains(&c);
    while i < t.len() {
        let c = t[i];
        if c != dollar || i + 1 == t.len() {
            out.push(c);
            i += 1;
            continue;
        }
        let n = t[i + 1];
        if n == dollar {
            out.push(dollar);
            i += 2;
        } else if n == u16::from(b'&') {
            out.extend_from_slice(matched.units());
            i += 2;
        } else if n == u16::from(b'`') {
            out.extend_from_slice(&su[..position.min(su.len())]);
            i += 2;
        } else if n == u16::from(b'\'') {
            out.extend_from_slice(&su[tail_pos..]);
            i += 2;
        } else if is_digit(n) {
            // $n o $nn: el más largo que sea un índice válido (1..=m).
            let d1 = usize::from(n - u16::from(b'0'));
            let two = t.get(i + 2).copied().filter(|&c| is_digit(c));
            let (index, consumed) = match two {
                Some(d2) => {
                    let nn = d1 * 10 + usize::from(d2 - u16::from(b'0'));
                    if (1..=m).contains(&nn) {
                        (nn, 3)
                    } else {
                        (d1, 2)
                    }
                }
                None => (d1, 2),
            };
            if (1..=m).contains(&index) {
                if let Value::String(cap) = &captures[index - 1] {
                    out.extend_from_slice(cap.units());
                } else if !captures[index - 1].is_undefined() {
                    let cs = agent.to_string(&captures[index - 1])?;
                    out.extend_from_slice(cs.units());
                }
                i += consumed;
            } else {
                out.push(dollar);
                i += 1;
            }
        } else if n == u16::from(b'<') {
            if named.is_undefined() {
                out.push(dollar);
                i += 1;
            } else {
                let close = t[i + 2..].iter().position(|&c| c == u16::from(b'>'));
                match close {
                    None => {
                        out.push(dollar);
                        i += 1;
                    }
                    Some(e) => {
                        let name = JsString::from_units(t[i + 2..i + 2 + e].to_vec());
                        let v = agent.get_value_of(named, &PropertyKey::from_js_string(name))?;
                        if !v.is_undefined() {
                            let vs = agent.to_string(&v)?;
                            out.extend_from_slice(vs.units());
                        }
                        i += e + 3;
                    }
                }
            }
        } else {
            out.push(dollar);
            i += 1;
        }
    }
    Ok(JsString::from_units(out))
}

/// String.prototype.replace (§22.1.3.19).
fn replace(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    agent.require_object_coercible(&a.this)?;
    let search_value = a.arg(0);
    let mut replace_value = a.arg(1);
    if search_value.is_object() {
        let k = PropertyKey::Symbol(agent.well_known.replace);
        if let Some(r) = agent.get_method_of(&search_value, &k)? {
            return agent.call(&r, search_value, &[a.this.clone(), replace_value]);
        }
    }
    let s = agent.to_string(&a.this)?;
    let search = agent.to_string(&search_value)?;
    let functional = agent.is_callable(&replace_value);
    if !functional {
        replace_value = Value::String(agent.to_string(&replace_value)?);
    }
    let Some(pos) = string_index_of(s.units(), search.units(), 0) else {
        return Ok(Value::String(s));
    };
    let replacement = if functional {
        let r = agent.call(
            &replace_value,
            Value::Undefined,
            &[
                Value::String(search.clone()),
                Value::Number(pos as f64),
                Value::String(s.clone()),
            ],
        )?;
        agent.to_string(&r)?
    } else {
        let Value::String(t) = &replace_value else {
            unreachable!("string")
        };
        get_substitution(agent, &search, &s, pos, &[], &Value::Undefined, t)?
    };
    let u = s.units();
    let mut out = u[..pos].to_vec();
    out.extend_from_slice(replacement.units());
    out.extend_from_slice(&u[pos + search.len()..]);
    Ok(Value::String(JsString::from_units(out)))
}

/// String.prototype.replaceAll (§22.1.3.20).
fn replace_all(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    agent.require_object_coercible(&a.this)?;
    let search_value = a.arg(0);
    let mut replace_value = a.arg(1);
    if search_value.is_object() {
        if agent.is_regexp(&search_value)? {
            let flags = agent.get_value_of(&search_value, &PropertyKey::from("flags"))?;
            agent.require_object_coercible(&flags)?;
            let f = agent.to_string(&flags)?;
            if !f.units().contains(&u16::from(b'g')) {
                return agent.type_error("replaceAll must be called with a global RegExp");
            }
        }
        let k = PropertyKey::Symbol(agent.well_known.replace);
        if let Some(r) = agent.get_method_of(&search_value, &k)? {
            return agent.call(&r, search_value, &[a.this.clone(), replace_value]);
        }
    }
    let s = agent.to_string(&a.this)?;
    let search = agent.to_string(&search_value)?;
    let functional = agent.is_callable(&replace_value);
    if !functional {
        replace_value = Value::String(agent.to_string(&replace_value)?);
    }
    let sl = search.len();
    let advance = sl.max(1);
    let mut positions = Vec::new();
    let mut p = string_index_of(s.units(), search.units(), 0);
    while let Some(pos) = p {
        positions.push(pos);
        p = string_index_of(s.units(), search.units(), pos + advance);
    }
    let u = s.units();
    let mut end = 0;
    let mut out: Vec<u16> = Vec::new();
    for pos in positions {
        out.extend_from_slice(&u[end..pos]);
        let replacement = if functional {
            let r = agent.call(
                &replace_value,
                Value::Undefined,
                &[
                    Value::String(search.clone()),
                    Value::Number(pos as f64),
                    Value::String(s.clone()),
                ],
            )?;
            agent.to_string(&r)?
        } else {
            let Value::String(t) = &replace_value else {
                unreachable!("string")
            };
            get_substitution(agent, &search, &s, pos, &[], &Value::Undefined, t)?
        };
        out.extend_from_slice(replacement.units());
        end = pos + sl;
    }
    if end < u.len() {
        out.extend_from_slice(&u[end..]);
    }
    Ok(Value::String(JsString::from_units(out)))
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
    Ok(sub_string(&s, from as usize, to as usize))
}

/// String.prototype.split (§22.1.3.23).
fn split(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    agent.require_object_coercible(&a.this)?;
    let separator = a.arg(0);
    let limit = a.arg(1);
    if separator.is_object() {
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
    Ok(sub_string(&s, from, to))
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

/// String.prototype.toWellFormed (§22.1.3.31).
fn to_well_formed(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    let out: Vec<u16> = char::decode_utf16(s.units().iter().copied())
        .flat_map(|r| {
            let c = r.unwrap_or(char::REPLACEMENT_CHARACTER);
            let mut buf = [0u16; 2];
            c.encode_utf16(&mut buf).to_vec()
        })
        .collect();
    Ok(Value::String(JsString::from_units(out)))
}

/// String.prototype.trim (§22.1.3.32).
fn trim(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    Ok(Value::String(JsString::from_units(
        trim_units(s.units()).to_vec(),
    )))
}

fn is_ws(c: u16) -> bool {
    char::from_u32(u32::from(c))
        .is_some_and(|ch| crate::lexer::is_whitespace(ch) || crate::lexer::is_line_terminator(ch))
}

/// String.prototype.trimStart (§22.1.3.34).
fn trim_start(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    let u = s.units();
    let first = u.iter().position(|&c| !is_ws(c)).unwrap_or(u.len());
    Ok(Value::String(JsString::from_units(u[first..].to_vec())))
}

/// String.prototype.trimEnd (§22.1.3.33).
fn trim_end(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    let u = s.units();
    let end = u.iter().rposition(|&c| !is_ws(c)).map_or(0, |e| e + 1);
    Ok(Value::String(JsString::from_units(u[..end].to_vec())))
}

/// String.prototype [ @@iterator ] (§22.1.3.36).
fn iterator(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    Ok(create_string_iterator(agent, s))
}

// --- B.2.2 ---

/// String.prototype.substr (B.2.2.1).
fn substr(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    let size = s.len() as f64;
    let start = agent.to_integer_or_infinity(&a.arg(0))?;
    let start = if start == f64::NEG_INFINITY {
        0.0
    } else if start < 0.0 {
        (size + start).max(0.0)
    } else {
        start.min(size)
    };
    let len = if a.arg(1).is_undefined() {
        size
    } else {
        agent.to_integer_or_infinity(&a.arg(1))?
    };
    let end = (start + len).clamp(start, size);
    Ok(sub_string(&s, start as usize, end as usize))
}

/// CreateHTML (B.2.2.2.1).
fn create_html(agent: &mut Agent, a: &CallArgs<'_>, tag: &str, attribute: &str) -> JsResult<Value> {
    let s = this_str(agent, a)?;
    let mut out: Vec<u16> = format!("<{tag}").encode_utf16().collect();
    if !attribute.is_empty() {
        let v = agent.to_string(&a.arg(0))?;
        out.extend(format!(" {attribute}=\"").encode_utf16());
        for &c in v.units() {
            if c == u16::from(b'"') {
                out.extend("&quot;".encode_utf16());
            } else {
                out.push(c);
            }
        }
        out.push(u16::from(b'"'));
    }
    out.push(u16::from(b'>'));
    out.extend_from_slice(s.units());
    out.extend(format!("</{tag}>").encode_utf16());
    Ok(Value::String(JsString::from_units(out)))
}

macro_rules! html {
    ($name:ident, $tag:literal, $attr:literal) => {
        fn $name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
            create_html(agent, a, $tag, $attr)
        }
    };
}

html!(anchor, "a", "name");
html!(big, "big", "");
html!(blink, "blink", "");
html!(bold, "b", "");
html!(fixed, "tt", "");
html!(fontcolor, "font", "color");
html!(fontsize, "font", "size");
html!(italics, "i", "");
html!(link, "a", "href");
html!(small, "small", "");
html!(strike, "strike", "");
html!(sub, "sub", "");
html!(sup, "sup", "");
