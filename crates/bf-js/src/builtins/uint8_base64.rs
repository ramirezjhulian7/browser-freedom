//! Uint8Array y base64/hex (ECMA-262 2026, §23.3.3: `Uint8Array.fromBase64`,
//! `Uint8Array.fromHex`, `Uint8Array.prototype.{toBase64, toHex, setFromBase64,
//! setFromHex}`): FromBase64, FromHex, DecodeBase64Chunk y la codificación.

use crate::builtins::array_buffer::{ElementType, read_bytes, write_bytes};
use crate::builtins::typed_array::{ta_data, ta_length};
use crate::builtins::{FnEntry, methods};
use crate::heap::Gc;
use crate::object::Object;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

pub fn init(agent: &mut Agent, realm: RealmId) {
    let c = agent.realms[realm].intrinsics.uint8_array_ctor;
    let p = agent.realms[realm].intrinsics.uint8_array_prototype;
    const STATICS: &[FnEntry] = &[("fromBase64", 1, from_base64), ("fromHex", 1, from_hex)];
    methods(agent, c, STATICS);
    const PROTO: &[FnEntry] = &[
        ("setFromBase64", 1, set_from_base64),
        ("setFromHex", 1, set_from_hex),
        ("toBase64", 0, to_base64),
        ("toHex", 0, to_hex),
    ];
    methods(agent, p, PROTO);
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// El resultado de FromBase64/FromHex: lo leído, los bytes y si hubo un error.
struct Decoded {
    read: usize,
    bytes: Vec<u8>,
    error: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LastChunk {
    Loose,
    Strict,
    StopBeforePartial,
}

fn is_ascii_whitespace(c: u16) -> bool {
    matches!(c, 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}

/// DecodeBase64Chunk: None si sobran bits con `throw_on_extra_bits`.
fn decode_chunk(chunk: &[u8], throw_on_extra_bits: bool) -> Option<Vec<u8>> {
    let mut vals = [0u32; 4];
    for (k, c) in chunk.iter().enumerate() {
        vals[k] = ALPHABET.iter().position(|a| a == c).unwrap_or(0) as u32;
    }
    let n = (vals[0] << 18) | (vals[1] << 12) | (vals[2] << 6) | vals[3];
    let b = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
    match chunk.len() {
        2 => {
            if throw_on_extra_bits && b[1] != 0 {
                return None;
            }
            Some(vec![b[0]])
        }
        3 => {
            if throw_on_extra_bits && b[2] != 0 {
                return None;
            }
            Some(vec![b[0], b[1]])
        }
        _ => Some(b.to_vec()),
    }
}

/// FromBase64 ( string, alphabet, lastChunkHandling [ , maxLength ] ).
fn decode_base64(s: &[u16], url: bool, last: LastChunk, max_length: usize) -> Decoded {
    let mut out = Decoded {
        read: 0,
        bytes: Vec::new(),
        error: false,
    };
    if max_length == 0 {
        return out;
    }
    let mut chunk: Vec<u8> = Vec::with_capacity(4);
    let len = s.len();
    let mut index = 0;
    let skip = |mut i: usize| {
        while i < len && is_ascii_whitespace(s[i]) {
            i += 1;
        }
        i
    };
    loop {
        index = skip(index);
        if index == len {
            if !chunk.is_empty() {
                match last {
                    LastChunk::StopBeforePartial => return out,
                    LastChunk::Loose => {
                        if chunk.len() == 1 {
                            out.error = true;
                            return out;
                        }
                        out.bytes
                            .extend(decode_chunk(&chunk, false).expect("sin bits extra"));
                    }
                    LastChunk::Strict => {
                        out.error = true;
                        return out;
                    }
                }
            }
            out.read = len;
            return out;
        }
        let mut c = s[index];
        index += 1;
        if c == u16::from(b'=') {
            if chunk.len() < 2 {
                out.error = true;
                return out;
            }
            index = skip(index);
            if chunk.len() == 2 {
                if index == len {
                    if last == LastChunk::StopBeforePartial {
                        return out;
                    }
                    out.error = true;
                    return out;
                }
                if s[index] == u16::from(b'=') {
                    index = skip(index + 1);
                }
            }
            if index < len {
                out.error = true;
                return out;
            }
            match decode_chunk(&chunk, last == LastChunk::Strict) {
                Some(b) => out.bytes.extend(b),
                None => {
                    out.error = true;
                    return out;
                }
            }
            out.read = len;
            return out;
        }
        if url {
            if c == u16::from(b'+') || c == u16::from(b'/') {
                out.error = true;
                return out;
            }
            if c == u16::from(b'-') {
                c = u16::from(b'+');
            } else if c == u16::from(b'_') {
                c = u16::from(b'/');
            }
        }
        if c > 0x7F || !ALPHABET.contains(&(c as u8)) {
            out.error = true;
            return out;
        }
        let remaining = max_length - out.bytes.len();
        if (remaining == 1 && chunk.len() == 2) || (remaining == 2 && chunk.len() == 3) {
            return out;
        }
        chunk.push(c as u8);
        if chunk.len() == 4 {
            out.bytes
                .extend(decode_chunk(&chunk, false).expect("sin bits extra"));
            chunk.clear();
            out.read = index;
            if out.bytes.len() == max_length {
                return out;
            }
        }
    }
}

/// FromHex ( string [ , maxLength ] ).
fn decode_hex(s: &[u16], max_length: usize) -> Decoded {
    let mut out = Decoded {
        read: 0,
        bytes: Vec::new(),
        error: false,
    };
    if !s.len().is_multiple_of(2) {
        out.error = true;
        return out;
    }
    let hex =
        |c: u16| -> Option<u8> { char::from_u32(u32::from(c))?.to_digit(16).map(|d| d as u8) };
    while out.read < s.len() && out.bytes.len() < max_length {
        let (Some(h), Some(l)) = (hex(s[out.read]), hex(s[out.read + 1])) else {
            out.error = true;
            return out;
        };
        out.read += 2;
        out.bytes.push((h << 4) | l);
    }
    out
}

/// ValidateUint8Array.
fn validate_uint8(agent: &mut Agent, v: &Value) -> JsResult<Gc<Object>> {
    if let Value::Object(o) = v
        && let Some(d) = ta_data(agent, *o)
        && d.kind == ElementType::Uint8
    {
        return Ok(*o);
    }
    agent.type_error("this is not a Uint8Array")
}

/// Las opciones `alphabet` y `lastChunkHandling` (sin ToString: tienen que ser strings).
fn base64_options(
    agent: &mut Agent,
    options: &Value,
    read_last: bool,
) -> JsResult<(bool, LastChunk, Gc<Object>)> {
    let o = agent.temporal_options_object(options)?;
    let a = agent.get(o, &PropertyKey::from("alphabet"), Value::Object(o))?;
    let url = match &a {
        Value::Undefined => false,
        Value::String(s) if s.to_string_lossy() == "base64" => false,
        Value::String(s) if s.to_string_lossy() == "base64url" => true,
        _ => return agent.type_error("alphabet must be \"base64\" or \"base64url\""),
    };
    let mut last = LastChunk::Loose;
    if read_last {
        let l = agent.get(o, &PropertyKey::from("lastChunkHandling"), Value::Object(o))?;
        last = match &l {
            Value::Undefined => LastChunk::Loose,
            Value::String(s) => match s.to_string_lossy().as_str() {
                "loose" => LastChunk::Loose,
                "strict" => LastChunk::Strict,
                "stop-before-partial" => LastChunk::StopBeforePartial,
                _ => {
                    return agent.type_error(
                        "lastChunkHandling must be \"loose\", \"strict\" or \"stop-before-partial\"",
                    );
                }
            },
            _ => {
                return agent.type_error(
                    "lastChunkHandling must be \"loose\", \"strict\" or \"stop-before-partial\"",
                );
            }
        };
    }
    Ok((url, last, o))
}

fn string_arg(agent: &mut Agent, v: &Value) -> JsResult<Vec<u16>> {
    match v {
        Value::String(s) => Ok(s.units().to_vec()),
        _ => agent.type_error("argument must be a string"),
    }
}

/// Un Uint8Array nuevo con los bytes.
fn new_uint8(agent: &mut Agent, bytes: &[u8]) -> JsResult<Value> {
    let c = agent.intrinsics().uint8_array_ctor;
    let ta = agent.construct(c, &[Value::Number(bytes.len() as f64)], None)?;
    if let Value::Object(o) = ta
        && let Some(d) = ta_data(agent, o)
    {
        let (b, off) = (d.buffer, d.byte_offset);
        write_bytes(agent, b, off, bytes);
    }
    Ok(ta)
}

fn syntax_error(agent: &mut Agent, what: &str) -> crate::vm::Throw {
    agent.error(crate::vm::ErrorType::SyntaxError, what)
}

/// Uint8Array.fromBase64 ( string [ , options ] ).
fn from_base64(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = string_arg(agent, &a.arg(0))?;
    let (url, last, _) = base64_options(agent, &a.arg(1), true)?;
    let r = decode_base64(&s, url, last, usize::MAX);
    if r.error {
        return Err(syntax_error(agent, "invalid base64 string"));
    }
    new_uint8(agent, &r.bytes)
}

/// Uint8Array.fromHex ( string ).
fn from_hex(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = string_arg(agent, &a.arg(0))?;
    let r = decode_hex(&s, usize::MAX);
    if r.error {
        return Err(syntax_error(agent, "invalid hex string"));
    }
    new_uint8(agent, &r.bytes)
}

/// La parte común de setFromBase64/setFromHex: escribe los bytes y arma `{ read, written }`.
fn finish_set(agent: &mut Agent, into: Gc<Object>, r: Decoded) -> JsResult<Value> {
    if let Some(d) = ta_data(agent, into) {
        let (b, off) = (d.buffer, d.byte_offset);
        write_bytes(agent, b, off, &r.bytes);
    }
    if r.error {
        return Err(syntax_error(agent, "invalid encoded string"));
    }
    let op = agent.intrinsics().object_prototype;
    let o = agent.new_object(Some(op));
    agent.create_data_property_or_throw(
        o,
        PropertyKey::from("read"),
        Value::Number(r.read as f64),
    )?;
    agent.create_data_property_or_throw(
        o,
        PropertyKey::from("written"),
        Value::Number(r.bytes.len() as f64),
    )?;
    Ok(Value::Object(o))
}

/// Uint8Array.prototype.setFromBase64 ( string [ , options ] ).
fn set_from_base64(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let into = validate_uint8(agent, &a.this)?;
    let s = string_arg(agent, &a.arg(0))?;
    let (url, last, _) = base64_options(agent, &a.arg(1), true)?;
    let Some(len) = ta_length(agent, into) else {
        return agent.type_error("TypedArray is detached or out of bounds");
    };
    let r = decode_base64(&s, url, last, len);
    finish_set(agent, into, r)
}

/// Uint8Array.prototype.setFromHex ( string ).
fn set_from_hex(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let into = validate_uint8(agent, &a.this)?;
    let s = string_arg(agent, &a.arg(0))?;
    let Some(len) = ta_length(agent, into) else {
        return agent.type_error("TypedArray is detached or out of bounds");
    };
    let r = decode_hex(&s, len);
    finish_set(agent, into, r)
}

/// GetUint8ArrayBytes.
fn uint8_bytes(agent: &mut Agent, o: Gc<Object>) -> JsResult<Vec<u8>> {
    let Some(len) = ta_length(agent, o) else {
        return agent.type_error("TypedArray is detached or out of bounds");
    };
    let d = ta_data(agent, o).expect("TypedArray");
    let (b, off) = (d.buffer, d.byte_offset);
    Ok(read_bytes(agent, b, off, len))
}

/// Uint8Array.prototype.toBase64 ( [ options ] ).
fn to_base64(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = validate_uint8(agent, &a.this)?;
    let (url, _, opts) = base64_options(agent, &a.arg(0), false)?;
    let omit = agent.get(opts, &PropertyKey::from("omitPadding"), Value::Object(opts))?;
    let omit = crate::abstract_ops::conversion::to_boolean(agent, &omit);
    let bytes = uint8_bytes(agent, o)?;
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        let digits = c.len() + 1;
        for k in 0..4 {
            if k < digits {
                let mut ch = ALPHABET[((n >> (18 - 6 * k)) & 63) as usize];
                if url {
                    ch = match ch {
                        b'+' => b'-',
                        b'/' => b'_',
                        x => x,
                    };
                }
                out.push(ch as char);
            } else if !omit {
                out.push('=');
            }
        }
    }
    Ok(Value::str(&out))
}

/// Uint8Array.prototype.toHex ( ).
fn to_hex(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = validate_uint8(agent, &a.this)?;
    let bytes = uint8_bytes(agent, o)?;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    Ok(Value::str(&out))
}
