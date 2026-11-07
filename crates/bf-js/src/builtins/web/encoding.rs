//! `TextEncoder` y `TextDecoder` (Encoding Standard §8). Los decodificadores de todos los
//! encodings (UTF-8, UTF-16, single-byte, multibyte legacy y `x-user-defined`), en streaming
//! y con modo `fatal`, son los de `bf_infra::encoding`.

use bf_infra::encoding::{Decoder, Encoding};

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
        ObjectKind::ArrayBuffer(_) => Some(crate::builtins::array_buffer::read_bytes(
            agent,
            *o,
            0,
            crate::builtins::array_buffer::byte_length(agent, *o),
        )),
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
    // E3: también sobre la memoria de un SharedArrayBuffer.
    let total = crate::builtins::array_buffer::byte_length(agent, buffer);
    if offset + len > total {
        return Vec::new();
    }
    crate::builtins::array_buffer::read_bytes(agent, buffer, offset, len)
}

/// Un ArrayBuffer nuevo (del realm actual) con `bytes`.
pub fn new_array_buffer(agent: &mut Agent, bytes: Vec<u8>) -> Gc<Object> {
    let bp = agent.intrinsics().array_buffer_prototype;
    agent.alloc(Object::new(
        Some(bp),
        ObjectKind::ArrayBuffer(Box::new(ArrayBufferData {
            data: Some(bytes),
            max_byte_length: None,
            shared: false,
            block: None,
        })),
    ))
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
            block: None,
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
    if offset + out.len() <= crate::builtins::array_buffer::byte_length(agent, buffer) {
        crate::builtins::array_buffer::write_bytes(agent, buffer, offset, &out);
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
    let mut text = String::new();
    let r = d.decoder.decode(&bytes, !stream, d.fatal, &mut text);
    // Serialize I/O queue (§8.1.4): el BOM inicial de UTF-8 y UTF-16 se quita.
    let mut units: Vec<u16> = Vec::with_capacity(text.len());
    let bom_encoding = matches!(
        d.encoding,
        Encoding::Utf8 | Encoding::Utf16Le | Encoding::Utf16Be
    );
    for c in text.chars() {
        if bom_encoding && !d.ignore_bom && !d.bom_seen {
            d.bom_seen = true;
            if c as u32 == 0xFEFF {
                continue;
            }
        }
        let mut b = [0u16; 2];
        units.extend_from_slice(c.encode_utf16(&mut b));
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
