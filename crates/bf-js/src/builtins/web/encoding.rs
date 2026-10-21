//! `TextEncoder` y `TextDecoder` (Encoding Standard §8) con los decodificadores de UTF-8
//! (§9.1.1) y UTF-16 (§14.2.1) en streaming y con modo `fatal`; los single-byte y
//! `x-user-defined` usan las tablas de `bf_infra::encoding`. Los multibyte legacy (gb18030,
//! Big5, EUC-JP, ISO-2022-JP, Shift_JIS, EUC-KR) se decodifican como windows-1252, como en
//! `bf_infra` (TODO(roadmap) fase 3.5).

use bf_infra::encoding::Encoding;

use crate::builtins::array_buffer::{ArrayBufferData, ElementType};
use crate::builtins::typed_array::TypedArrayData;
use crate::builtins::web::{
    WebData, attr, create_from, data_of, data_of_mut, interface, op, require_args, require_new,
    str_value, usv_string,
};
use crate::heap::Gc;
use crate::object::{Object, ObjectKind};
use crate::string::JsString;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

pub fn init(agent: &mut Agent, realm: RealmId) {
    let (_, p) = interface(
        agent,
        realm,
        "TextEncoder",
        "TextEncoder.prototype",
        0,
        encoder_ctor,
        None,
    );
    attr(agent, p, "encoding", encoder_encoding, None);
    op(agent, p, "encode", 0, encode);
    op(agent, p, "encodeInto", 2, encode_into);
    let (_, p) = interface(
        agent,
        realm,
        "TextDecoder",
        "TextDecoder.prototype",
        0,
        decoder_ctor,
        None,
    );
    attr(agent, p, "encoding", decoder_encoding, None);
    attr(agent, p, "fatal", decoder_fatal, None);
    attr(agent, p, "ignoreBOM", decoder_ignore_bom, None);
    op(agent, p, "decode", 0, decode);
}

// --- Bytes ---

/// Los bytes de un `AllowSharedBufferSource` (ArrayBuffer, SharedArrayBuffer o una vista):
/// una copia (`None` si no es uno).
pub fn buffer_source_bytes(agent: &Agent, v: &Value) -> Option<Vec<u8>> {
    let Value::Object(o) = v else { return None };
    match &agent.heap[*o].kind {
        ObjectKind::ArrayBuffer(b) => Some(b.data.clone().unwrap_or_default()),
        ObjectKind::TypedArray(t) => {
            let len = crate::builtins::typed_array::ta_length(agent, *o).unwrap_or(0);
            let bytes = len * t.kind.size();
            Some(slice_of(agent, t.buffer, t.byte_offset, bytes))
        }
        ObjectKind::DataView(d) => {
            let total = crate::builtins::array_buffer::byte_length(agent, d.buffer);
            let len = d.byte_length.unwrap_or(total.saturating_sub(d.byte_offset));
            Some(slice_of(agent, d.buffer, d.byte_offset, len))
        }
        _ => None,
    }
}

fn slice_of(agent: &Agent, buffer: Gc<Object>, offset: usize, len: usize) -> Vec<u8> {
    match &agent.heap[buffer].kind {
        ObjectKind::ArrayBuffer(b) => b
            .data
            .as_ref()
            .and_then(|d| d.get(offset..offset + len))
            .map(<[u8]>::to_vec)
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Un Uint8Array nuevo (del realm actual) con `bytes`.
pub fn new_uint8_array(agent: &mut Agent, bytes: Vec<u8>) -> Gc<Object> {
    let len = bytes.len();
    let bp = agent.intrinsics().array_buffer_prototype;
    let buffer = agent.alloc(Object::new(
        Some(bp),
        ObjectKind::ArrayBuffer(Box::new(ArrayBufferData {
            data: Some(bytes),
            max_byte_length: None,
            shared: false,
        })),
    ));
    let proto = agent.intrinsics().uint8_array_prototype;
    agent.alloc(Object::new(
        Some(proto),
        ObjectKind::TypedArray(Box::new(TypedArrayData {
            buffer,
            kind: ElementType::Uint8,
            byte_offset: 0,
            array_length: Some(len),
        })),
    ))
}

// --- TextEncoder (§8.2) ---

fn encoder_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let nt = require_new(agent, a, "TextEncoder")?;
    let o = create_from(agent, nt, "TextEncoder.prototype", WebData::TextEncoder)?;
    Ok(Value::Object(o))
}

fn check_encoder(agent: &mut Agent, this: &Value) -> JsResult<()> {
    match data_of(agent, this) {
        Some(WebData::TextEncoder) => Ok(()),
        _ => agent.type_error("Illegal invocation"),
    }
}

fn encoder_encoding(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    check_encoder(agent, &a.this)?;
    Ok(str_value("utf-8"))
}

/// `encode(input = "")`: UTF-8 del USVString.
fn encode(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    check_encoder(agent, &a.this)?;
    let s = if a.arg(0).is_undefined() {
        String::new()
    } else {
        usv_string(agent, &a.arg(0))?
    };
    Ok(Value::Object(new_uint8_array(agent, s.into_bytes())))
}

/// `encodeInto(source, destination)`: escribe lo que quepa; `read` en code units UTF-16.
fn encode_into(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    check_encoder(agent, &a.this)?;
    require_args(
        agent,
        a,
        2,
        "Failed to execute 'encodeInto' on 'TextEncoder'",
    )?;
    let src = usv_string(agent, &a.arg(0))?;
    let dest = match a.arg(1) {
        Value::Object(o) => match &agent.heap[o].kind {
            ObjectKind::TypedArray(t) if t.kind == ElementType::Uint8 => {
                Some((o, t.buffer, t.byte_offset))
            }
            _ => None,
        },
        _ => None,
    };
    let Some((dest, buffer, offset)) = dest else {
        return agent.type_error(
            "Failed to execute 'encodeInto' on 'TextEncoder': parameter 2 is not of type 'Uint8Array'.",
        );
    };
    let capacity = crate::builtins::typed_array::ta_length(agent, dest).unwrap_or(0);
    let mut read = 0usize;
    let mut out: Vec<u8> = Vec::new();
    for c in src.chars() {
        let n = c.len_utf8();
        if out.len() + n > capacity {
            break;
        }
        let mut buf = [0u8; 4];
        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        read += c.len_utf16();
    }
    if let ObjectKind::ArrayBuffer(b) = &mut agent.heap[buffer].kind
        && let Some(d) = &mut b.data
        && let Some(slot) = d.get_mut(offset..offset + out.len())
    {
        slot.copy_from_slice(&out);
    }
    let r = agent.new_plain_object();
    agent.create_data_property_or_throw(
        r,
        PropertyKey::from("read"),
        Value::Number(read as f64),
    )?;
    agent.create_data_property_or_throw(
        r,
        PropertyKey::from("written"),
        Value::Number(out.len() as f64),
    )?;
    Ok(Value::Object(r))
}

// --- Decodificadores ---

/// El estado de un decodificador.
#[derive(Debug, Clone)]
enum Decoder {
    /// UTF-8 decoder (§9.1.1).
    Utf8 {
        code_point: u32,
        seen: u8,
        needed: u8,
        lower: u8,
        upper: u8,
    },
    /// Shared UTF-16 decoder (§14.2.1).
    Utf16 {
        little: bool,
        lead_byte: Option<u8>,
        lead_surrogate: Option<u16>,
    },
    /// Single-byte, x-user-defined y los multibyte sin decodificador propio.
    Table(Encoding),
}

impl Decoder {
    fn new(e: Encoding) -> Decoder {
        match e {
            Encoding::Utf8 => Decoder::Utf8 {
                code_point: 0,
                seen: 0,
                needed: 0,
                lower: 0x80,
                upper: 0xBF,
            },
            Encoding::Utf16Le | Encoding::Utf16Be => Decoder::Utf16 {
                little: e == Encoding::Utf16Le,
                lead_byte: None,
                lead_surrogate: None,
            },
            other => Decoder::Table(other),
        }
    }

    /// Procesa la cola: los code points decodificados en `out`. Con `flush`, el
    /// end-of-queue. `Err` en un error con `fatal`.
    fn run(
        &mut self,
        input: &[u8],
        flush: bool,
        fatal: bool,
        out: &mut Vec<u32>,
    ) -> Result<(), ()> {
        let mut queue: std::collections::VecDeque<u8> = input.iter().copied().collect();
        let error = |out: &mut Vec<u32>| -> Result<(), ()> {
            if fatal {
                Err(())
            } else {
                out.push(0xFFFD);
                Ok(())
            }
        };
        loop {
            let Some(byte) = queue.pop_front() else {
                if !flush {
                    return Ok(());
                }
                // end-of-queue.
                match self {
                    Decoder::Utf8 { needed, .. } if *needed != 0 => {
                        *self = Decoder::new(Encoding::Utf8);
                        return error(out);
                    }
                    Decoder::Utf16 {
                        lead_byte,
                        lead_surrogate,
                        ..
                    } if lead_byte.is_some() || lead_surrogate.is_some() => {
                        *lead_byte = None;
                        *lead_surrogate = None;
                        return error(out);
                    }
                    _ => return Ok(()),
                }
            };
            match self {
                Decoder::Utf8 {
                    code_point,
                    seen,
                    needed,
                    lower,
                    upper,
                } => {
                    if *needed == 0 {
                        match byte {
                            0x00..=0x7F => out.push(u32::from(byte)),
                            0xC2..=0xDF => {
                                *needed = 1;
                                *code_point = u32::from(byte & 0x1F);
                            }
                            0xE0..=0xEF => {
                                if byte == 0xE0 {
                                    *lower = 0xA0;
                                }
                                if byte == 0xED {
                                    *upper = 0x9F;
                                }
                                *needed = 2;
                                *code_point = u32::from(byte & 0x0F);
                            }
                            0xF0..=0xF4 => {
                                if byte == 0xF0 {
                                    *lower = 0x90;
                                }
                                if byte == 0xF4 {
                                    *upper = 0x8F;
                                }
                                *needed = 3;
                                *code_point = u32::from(byte & 0x07);
                            }
                            _ => error(out)?,
                        }
                        continue;
                    }
                    if !(*lower..=*upper).contains(&byte) {
                        *code_point = 0;
                        *needed = 0;
                        *seen = 0;
                        *lower = 0x80;
                        *upper = 0xBF;
                        queue.push_front(byte);
                        error(out)?;
                        continue;
                    }
                    *lower = 0x80;
                    *upper = 0xBF;
                    *code_point = (*code_point << 6) | u32::from(byte & 0x3F);
                    *seen += 1;
                    if *seen != *needed {
                        continue;
                    }
                    out.push(*code_point);
                    *code_point = 0;
                    *seen = 0;
                    *needed = 0;
                }
                Decoder::Utf16 {
                    little,
                    lead_byte,
                    lead_surrogate,
                } => {
                    let Some(lead) = lead_byte.take() else {
                        *lead_byte = Some(byte);
                        continue;
                    };
                    let cu = if *little {
                        u16::from(byte) << 8 | u16::from(lead)
                    } else {
                        u16::from(lead) << 8 | u16::from(byte)
                    };
                    if let Some(ls) = lead_surrogate.take() {
                        if (0xDC00..0xE000).contains(&cu) {
                            out.push(
                                0x10000
                                    + ((u32::from(ls) - 0xD800) << 10)
                                    + (u32::from(cu) - 0xDC00),
                            );
                            continue;
                        }
                        let (b1, b2) = ((cu >> 8) as u8, (cu & 0xFF) as u8);
                        if *little {
                            queue.push_front(b1);
                            queue.push_front(b2);
                        } else {
                            queue.push_front(b2);
                            queue.push_front(b1);
                        }
                        error(out)?;
                        continue;
                    }
                    if (0xD800..0xDC00).contains(&cu) {
                        *lead_surrogate = Some(cu);
                        continue;
                    }
                    if (0xDC00..0xE000).contains(&cu) {
                        error(out)?;
                        continue;
                    }
                    out.push(u32::from(cu));
                }
                Decoder::Table(e) => {
                    if byte < 0x80 && !matches!(e, Encoding::Replacement) {
                        out.push(u32::from(byte));
                        continue;
                    }
                    let s = e.decode_without_bom(&[byte]);
                    match s.chars().next() {
                        Some('\u{FFFD}') | None => error(out)?,
                        Some(c) => out.push(c as u32),
                    }
                }
            }
        }
    }
}

/// Los slots de un TextDecoder.
#[derive(Debug, Clone)]
pub struct DecoderData {
    encoding: Encoding,
    fatal: bool,
    ignore_bom: bool,
    bom_seen: bool,
    do_not_flush: bool,
    decoder: Decoder,
}

/// `new TextDecoder(label = "utf-8", options)` (§8.1.3).
fn decoder_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let nt = require_new(agent, a, "TextDecoder")?;
    let label = if a.arg(0).is_undefined() {
        "utf-8".to_owned()
    } else {
        agent.to_string(&a.arg(0))?.to_string_lossy()
    };
    let (fatal, ignore_bom) =
        dictionary_flags(agent, &a.arg(1), &["fatal", "ignoreBOM"]).map(|v| (v[0], v[1]))?;
    let encoding = match Encoding::for_label(&label) {
        Some(e) if e != Encoding::Replacement => e,
        _ => {
            return agent.range_error(&format!(
                "Failed to construct 'TextDecoder': The encoding label provided ('{label}') is invalid."
            ));
        }
    };
    let o = create_from(
        agent,
        nt,
        "TextDecoder.prototype",
        WebData::TextDecoder(Box::new(DecoderData {
            encoding,
            fatal,
            ignore_bom,
            bom_seen: false,
            do_not_flush: false,
            decoder: Decoder::new(encoding),
        })),
    )?;
    Ok(Value::Object(o))
}

/// Los miembros booleanos de un diccionario de WebIDL (§3.2.18), en orden.
fn dictionary_flags(agent: &mut Agent, v: &Value, names: &[&str]) -> JsResult<Vec<bool>> {
    match v {
        Value::Undefined | Value::Null => Ok(vec![false; names.len()]),
        Value::Object(o) => {
            let mut out = Vec::with_capacity(names.len());
            for n in names {
                let x = agent.get_str(*o, n)?;
                out.push(crate::abstract_ops::conversion::to_boolean(agent, &x));
            }
            Ok(out)
        }
        _ => agent.type_error("The provided value is not of type 'dictionary'"),
    }
}

fn decoder_data(agent: &mut Agent, this: &Value) -> JsResult<DecoderData> {
    match data_of(agent, this) {
        Some(WebData::TextDecoder(d)) => Ok((**d).clone()),
        _ => agent.type_error("Illegal invocation"),
    }
}

fn decoder_encoding(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = decoder_data(agent, &a.this)?;
    Ok(str_value(&d.encoding.name().to_ascii_lowercase()))
}

fn decoder_fatal(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(decoder_data(agent, &a.this)?.fatal))
}

fn decoder_ignore_bom(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(decoder_data(agent, &a.this)?.ignore_bom))
}

/// `decode(input, options)` (§8.1.4).
fn decode(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let mut d = decoder_data(agent, &a.this)?;
    let input = a.arg(0);
    if !input.is_undefined() && buffer_source_bytes(agent, &input).is_none() {
        return agent.type_error(
            "Failed to execute 'decode' on 'TextDecoder': The provided value is not of type '(ArrayBuffer or ArrayBufferView)'",
        );
    }
    // WebIDL convierte `options` antes de que el algoritmo copie los bytes (un getter puede
    // haber detached el buffer).
    let stream = dictionary_flags(agent, &a.arg(1), &["stream"])?[0];
    let bytes = if input.is_undefined() {
        Vec::new()
    } else {
        match buffer_source_bytes(agent, &input) {
            Some(b) => b,
            None => {
                return agent.type_error(
                    "Failed to execute 'decode' on 'TextDecoder': The provided value is not of type '(ArrayBuffer or ArrayBufferView)'",
                );
            }
        }
    };
    if !d.do_not_flush {
        d.decoder = Decoder::new(d.encoding);
        d.bom_seen = false;
    }
    d.do_not_flush = stream;
    let mut cps = Vec::new();
    let r = d.decoder.run(&bytes, !stream, d.fatal, &mut cps);
    // Serialize I/O queue (§8.1.4): el BOM inicial de UTF-8 y UTF-16 se quita.
    let mut units: Vec<u16> = Vec::with_capacity(cps.len());
    let bom_encoding = matches!(
        d.encoding,
        Encoding::Utf8 | Encoding::Utf16Le | Encoding::Utf16Be
    );
    for cp in cps {
        if bom_encoding && !d.ignore_bom && !d.bom_seen {
            d.bom_seen = true;
            if cp == 0xFEFF {
                continue;
            }
        }
        match char::from_u32(cp) {
            Some(c) => {
                let mut b = [0u16; 2];
                units.extend_from_slice(c.encode_utf16(&mut b));
            }
            None => units.push(cp as u16),
        }
    }
    if r.is_err() {
        // Un error fatal deja el decodificador para un decode nuevo.
        d.do_not_flush = false;
    }
    if let Value::Object(o) = a.this
        && let Some(WebData::TextDecoder(slot)) = data_of_mut(agent, o)
    {
        **slot = d;
    }
    if r.is_err() {
        return agent.type_error(
            "Failed to execute 'decode' on 'TextDecoder': The encoded data was not valid.",
        );
    }
    Ok(Value::String(JsString::from_units(units)))
}
