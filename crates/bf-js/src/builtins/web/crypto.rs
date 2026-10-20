//! `crypto.getRandomValues` y `crypto.randomUUID` (Web Cryptography API §10.1). Los bytes
//! salen de un generador ChaCha20 (RFC 8439 §2.3) por hilo, sembrado con entropía del sistema
//! operativo: `/dev/urandom` en Unix; en el resto, las claves aleatorias que `std` obtiene del
//! SO para `RandomState` (en Windows, `ProcessPrng`/`BCryptGenRandom`), combinadas con la hora
//! y direcciones de memoria. Sin FFI ni crates externos.

use std::cell::RefCell;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};

use crate::builtins::array_buffer::ElementType;
use crate::builtins::web::{WebData, interface, op, require_args, throw_dom};
use crate::object::{Object, ObjectKind};
use crate::string::JsString;
use crate::value::Value;
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

pub fn init(agent: &mut Agent, realm: RealmId) {
    let (_, p) = interface(
        agent,
        realm,
        "Crypto",
        "Crypto.prototype",
        0,
        illegal_constructor,
        None,
    );
    op(agent, p, "getRandomValues", 1, get_random_values);
    op(agent, p, "randomUUID", 0, random_uuid);
    // `crypto` es un objeto Crypto (los métodos no usan slots).
    let c = agent.alloc(Object::new(
        Some(p),
        ObjectKind::Web(Box::new(WebData::Registry(Box::default()))),
    ));
    crate::builtins::register_global(agent, realm, "crypto", c);
}

pub(crate) fn illegal_constructor(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    agent.type_error("Illegal constructor")
}

// --- ChaCha20 ---

struct ChaCha {
    key: [u32; 8],
    nonce: [u32; 3],
    counter: u32,
    buf: [u8; 64],
    pos: usize,
}

fn quarter(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(7);
}

/// El bloque de ChaCha20 (RFC 8439 §2.3).
fn chacha_block(key: &[u32; 8], counter: u32, nonce: &[u32; 3]) -> [u8; 64] {
    let mut init = [0u32; 16];
    init[..4].copy_from_slice(&[0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574]);
    init[4..12].copy_from_slice(key);
    init[12] = counter;
    init[13..].copy_from_slice(nonce);
    let mut s = init;
    for _ in 0..10 {
        quarter(&mut s, 0, 4, 8, 12);
        quarter(&mut s, 1, 5, 9, 13);
        quarter(&mut s, 2, 6, 10, 14);
        quarter(&mut s, 3, 7, 11, 15);
        quarter(&mut s, 0, 5, 10, 15);
        quarter(&mut s, 1, 6, 11, 12);
        quarter(&mut s, 2, 7, 8, 13);
        quarter(&mut s, 3, 4, 9, 14);
    }
    let mut out = [0u8; 64];
    for i in 0..16 {
        out[4 * i..4 * i + 4].copy_from_slice(&s[i].wrapping_add(init[i]).to_le_bytes());
    }
    out
}

/// 44 bytes de semilla (clave y nonce) del sistema operativo.
fn os_seed() -> [u8; 44] {
    let mut seed = [0u8; 44];
    if cfg!(unix)
        && let Ok(mut f) = std::fs::File::open("/dev/urandom")
    {
        use std::io::Read;
        if f.read_exact(&mut seed).is_ok() {
            return seed;
        }
    }
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let marker = 0u8;
    let addr = std::ptr::addr_of!(marker) as usize;
    for (i, chunk) in seed.chunks_mut(8).enumerate() {
        let mut h = RandomState::new().build_hasher();
        h.write_usize(i);
        h.write_u128(time);
        h.write_usize(addr);
        let v = h.finish().to_le_bytes();
        chunk.copy_from_slice(&v[..chunk.len()]);
    }
    seed
}

impl ChaCha {
    fn new() -> ChaCha {
        let seed = os_seed();
        let word = |i: usize| u32::from_le_bytes([seed[i], seed[i + 1], seed[i + 2], seed[i + 3]]);
        let mut key = [0u32; 8];
        for (k, slot) in key.iter_mut().enumerate() {
            *slot = word(4 * k);
        }
        ChaCha {
            key,
            nonce: [word(32), word(36), word(40)],
            counter: 0,
            buf: [0; 64],
            pos: 64,
        }
    }

    fn fill(&mut self, out: &mut [u8]) {
        for b in out {
            if self.pos == 64 {
                self.buf = chacha_block(&self.key, self.counter, &self.nonce);
                self.counter = self.counter.wrapping_add(1);
                if self.counter == 0 {
                    // Agotado el contador: otra semilla.
                    *self = ChaCha::new();
                    self.buf = chacha_block(&self.key, 0, &self.nonce);
                    self.counter = 1;
                }
                self.pos = 0;
            }
            *b = self.buf[self.pos];
            self.pos += 1;
        }
    }
}

thread_local! {
    static RNG: RefCell<Option<ChaCha>> = const { RefCell::new(None) };
}

/// Bytes aleatorios criptográficamente seguros.
pub fn random_bytes(out: &mut [u8]) {
    RNG.with(|r| {
        let mut r = r.borrow_mut();
        r.get_or_insert_with(ChaCha::new).fill(out);
    });
}

/// `crypto.getRandomValues(array)`: solo TypedArrays enteros, hasta 65 536 bytes.
fn get_random_values(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    require_args(
        agent,
        a,
        1,
        "Failed to execute 'getRandomValues' on 'Crypto'",
    )?;
    let arr = a.arg(0);
    let view = match &arr {
        Value::Object(o) => match &agent.heap[*o].kind {
            ObjectKind::TypedArray(t) => Some((*o, t.kind, t.buffer, t.byte_offset)),
            ObjectKind::DataView(_) => None,
            _ => {
                return agent.type_error(
                    "Failed to execute 'getRandomValues' on 'Crypto': parameter 1 is not of type 'ArrayBufferView'.",
                );
            }
        },
        _ => {
            return agent.type_error(
                "Failed to execute 'getRandomValues' on 'Crypto': parameter 1 is not of type 'ArrayBufferView'.",
            );
        }
    };
    let Some((o, kind, buffer, offset)) = view.filter(|v| {
        !matches!(
            v.1,
            ElementType::Float16 | ElementType::Float32 | ElementType::Float64
        )
    }) else {
        return throw_dom(
            agent,
            "TypeMismatchError",
            "Failed to execute 'getRandomValues' on 'Crypto': The provided ArrayBufferView is of type 'Float', which is not an integer array type.",
        );
    };
    let len = crate::builtins::typed_array::ta_length(agent, o).unwrap_or(0) * kind.size();
    if len > 65536 {
        return throw_dom(
            agent,
            "QuotaExceededError",
            &format!(
                "Failed to execute 'getRandomValues' on 'Crypto': The ArrayBufferView's byte length ({len}) exceeds the number of bytes of entropy available via this API (65536)."
            ),
        );
    }
    let mut bytes = vec![0u8; len];
    random_bytes(&mut bytes);
    if let ObjectKind::ArrayBuffer(b) = &mut agent.heap[buffer].kind
        && let Some(d) = &mut b.data
        && let Some(slot) = d.get_mut(offset..offset + len)
    {
        slot.copy_from_slice(&bytes);
    }
    Ok(arr)
}

/// `crypto.randomUUID()`: un UUID versión 4 (RFC 9562 §5.4).
fn random_uuid(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let mut b = [0u8; 16];
    random_bytes(&mut b);
    b[6] = (b[6] & 0x0F) | 0x40;
    b[8] = (b[8] & 0x3F) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    let s = format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    );
    Ok(Value::String(JsString::from(s.as_str())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chacha20_rfc8439_vector() {
        // RFC 8439 §2.3.2.
        let key: [u32; 8] = std::array::from_fn(|i| {
            u32::from_le_bytes([
                4 * i as u8,
                4 * i as u8 + 1,
                4 * i as u8 + 2,
                4 * i as u8 + 3,
            ])
        });
        let nonce = [0x0900_0000, 0x4a00_0000, 0];
        let out = chacha_block(&key, 1, &nonce);
        assert_eq!(&out[..8], &[0x10, 0xf1, 0xe7, 0xe4, 0xd1, 0x3b, 0x59, 0x15]);
        assert_eq!(
            &out[56..],
            &[0xcb, 0xd0, 0x83, 0xe8, 0xa2, 0x50, 0x3c, 0x4e]
        );
    }

    #[test]
    fn random_differs() {
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        random_bytes(&mut a);
        random_bytes(&mut b);
        assert_ne!(a, b);
    }
}
