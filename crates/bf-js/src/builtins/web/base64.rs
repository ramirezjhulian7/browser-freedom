//! `atob` y `btoa` (HTML §8.3) con forgiving-base64 (Infra §4.6, "forgiving-base64 encode" y
//! "forgiving-base64 decode").

use crate::builtins::web::{require_args, throw_dom};
use crate::string::JsString;
use crate::value::Value;
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn init(agent: &mut Agent, realm: RealmId) {
    let f = agent.create_builtin_function(atob, 1, "atob", false);
    crate::builtins::register_global(agent, realm, "atob", f);
    let f = agent.create_builtin_function(btoa, 1, "btoa", false);
    crate::builtins::register_global(agent, realm, "btoa", f);
}

/// Forgiving-base64 encode.
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for k in 0..4 {
            if k <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * k) & 0x3F) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Forgiving-base64 decode: `None` es failure.
pub fn decode(input: &[u16]) -> Option<Vec<u8>> {
    // 1. Quitar el whitespace ASCII.
    let mut data: Vec<u16> = input
        .iter()
        .copied()
        .filter(|&c| !matches!(c, 0x09 | 0x0A | 0x0C | 0x0D | 0x20))
        .collect();
    // 2. Con longitud múltiplo de 4, quitar uno o dos `=` finales.
    if data.len().is_multiple_of(4) {
        for _ in 0..2 {
            if data.last() == Some(&u16::from(b'=')) {
                data.pop();
            }
        }
    }
    // 3. Resto 1: failure.
    if data.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(data.len() * 3 / 4);
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for &c in &data {
        let v = ALPHABET.iter().position(|&a| u16::from(a) == c)? as u32;
        buffer = buffer << 6 | v;
        bits += 6;
        if bits == 24 {
            out.extend_from_slice(&[(buffer >> 16) as u8, (buffer >> 8) as u8, buffer as u8]);
            buffer = 0;
            bits = 0;
        }
    }
    match bits {
        12 => out.push((buffer >> 4) as u8),
        18 => out.extend_from_slice(&[(buffer >> 10) as u8, (buffer >> 2) as u8]),
        _ => {}
    }
    Some(out)
}

/// `btoa(data)`: cada code unit debe ser ≤ U+00FF.
fn btoa(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    require_args(agent, a, 1, "Failed to execute 'btoa'")?;
    let s = agent.to_string(&a.arg(0))?;
    let mut bytes = Vec::with_capacity(s.len());
    for &u in s.units() {
        if u > 0xFF {
            return throw_dom(
                agent,
                "InvalidCharacterError",
                "Failed to execute 'btoa': The string to be encoded contains characters outside of the Latin1 range.",
            );
        }
        bytes.push(u as u8);
    }
    Ok(Value::String(JsString::from(encode(&bytes).as_str())))
}

/// `atob(data)`: el resultado como un string de bytes (cada byte un code unit).
fn atob(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    require_args(agent, a, 1, "Failed to execute 'atob'")?;
    let s = agent.to_string(&a.arg(0))?;
    match decode(s.units()) {
        Some(bytes) => Ok(Value::String(JsString::from_units(
            bytes.into_iter().map(u16::from).collect(),
        ))),
        None => throw_dom(
            agent,
            "InvalidCharacterError",
            "Failed to execute 'atob': The string to be decoded is not correctly encoded.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        assert_eq!(encode(b""), "");
        assert_eq!(encode(b"f"), "Zg==");
        assert_eq!(encode(b"fo"), "Zm8=");
        assert_eq!(encode(b"foo"), "Zm9v");
        let u = |s: &str| s.encode_utf16().collect::<Vec<_>>();
        assert_eq!(decode(&u(" Zm 9v ")).unwrap(), b"foo");
        assert_eq!(decode(&u("Zg")).unwrap(), b"f");
        assert!(decode(&u("Z")).is_none());
        assert!(decode(&u("Zg=")).is_none());
        assert!(decode(&u("Zm9v=")).is_none());
    }
}
