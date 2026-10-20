//! DataView (§25.3): vistas con endianness explícita sobre ArrayBuffer y
//! SharedArrayBuffer, incluidos los buffers redimensionables (`[[ByteLength]]` auto).

use crate::builtins::array_buffer::{
    ElementType, buffer_data, byte_length, get_value_from_buffer, is_detached, is_fixed_length,
    set_value_in_buffer, to_element_value,
};
use crate::builtins::{FnEntry, accessor, constructor, methods, to_string_tag};
use crate::heap::Gc;
use crate::object::{Object, ObjectKind};
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

/// Los internal slots de un DataView.
#[derive(Debug)]
pub struct DataViewData {
    /// `[[ViewedArrayBuffer]]`.
    pub buffer: Gc<Object>,
    pub byte_offset: usize,
    /// `[[ByteLength]]`; `None` es auto.
    pub byte_length: Option<usize>,
}

pub fn init(agent: &mut Agent, realm: RealmId) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    let p = agent.new_object(Some(op));
    const PROTO: &[FnEntry] = &[
        ("getBigInt64", 1, get_big_int64),
        ("getBigUint64", 1, get_big_uint64),
        ("getFloat16", 1, get_float16),
        ("getFloat32", 1, get_float32),
        ("getFloat64", 1, get_float64),
        ("getInt8", 1, get_int8),
        ("getInt16", 1, get_int16),
        ("getInt32", 1, get_int32),
        ("getUint8", 1, get_uint8),
        ("getUint16", 1, get_uint16),
        ("getUint32", 1, get_uint32),
        ("setBigInt64", 2, set_big_int64),
        ("setBigUint64", 2, set_big_uint64),
        ("setFloat16", 2, set_float16),
        ("setFloat32", 2, set_float32),
        ("setFloat64", 2, set_float64),
        ("setInt8", 2, set_int8),
        ("setInt16", 2, set_int16),
        ("setInt32", 2, set_int32),
        ("setUint8", 2, set_uint8),
        ("setUint16", 2, set_uint16),
        ("setUint32", 2, set_uint32),
    ];
    methods(agent, p, PROTO);
    let getters: &[(&str, crate::vm::function::NativeFn)] = &[
        ("buffer", get_buffer),
        ("byteLength", get_byte_length),
        ("byteOffset", get_byte_offset),
    ];
    for &(name, f) in getters {
        accessor(agent, p, PropertyKey::from(name), name, Some(f), None);
    }
    to_string_tag(agent, p, "DataView");
    constructor(agent, "DataView", 1, data_view_ctor, p);
    agent.realms[realm].intrinsics.data_view_prototype = p;
}

/// DataView ( buffer [ , byteOffset [ , byteLength ] ] ) (§25.3.2.1).
fn data_view_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor DataView requires 'new'");
    };
    let buffer = match a.arg(0) {
        Value::Object(b) if buffer_data(agent, b).is_some() => b,
        _ => {
            return agent
                .type_error("First argument to DataView constructor must be an ArrayBuffer");
        }
    };
    let offset = agent.to_index(&a.arg(1))?;
    if is_detached(agent, buffer) {
        return agent.type_error("Cannot construct a DataView on a detached ArrayBuffer");
    }
    let buf_len = byte_length(agent, buffer) as u64;
    if offset > buf_len {
        return agent.range_error("Start offset is outside the bounds of the buffer");
    }
    let fixed = is_fixed_length(agent, buffer);
    let length_arg = a.arg(2);
    let view_len = if length_arg.is_undefined() {
        if fixed { Some(buf_len - offset) } else { None }
    } else {
        let l = agent.to_index(&length_arg)?;
        if offset.saturating_add(l) > buf_len {
            return agent.range_error("Invalid DataView length");
        }
        Some(l)
    };
    let proto = agent.get_prototype_from_constructor(nt, |i| i.data_view_prototype)?;
    // Pasos 11–14: GetPrototypeFromConstructor pudo haber cambiado el buffer.
    if is_detached(agent, buffer) {
        return agent.type_error("Cannot construct a DataView on a detached ArrayBuffer");
    }
    let buf_len = byte_length(agent, buffer) as u64;
    if offset > buf_len {
        return agent.range_error("Start offset is outside the bounds of the buffer");
    }
    if !length_arg.is_undefined()
        && let Some(l) = view_len
        && offset + l > buf_len
    {
        return agent.range_error("Invalid DataView length");
    }
    Ok(Value::Object(agent.alloc(Object::new(
        Some(proto),
        ObjectKind::DataView(Box::new(DataViewData {
            buffer,
            byte_offset: offset as usize,
            byte_length: view_len.map(|l| l as usize),
        })),
    ))))
}

fn this_view(agent: &mut Agent, this: &Value) -> JsResult<Gc<Object>> {
    match this {
        Value::Object(o) if matches!(agent.heap[*o].kind, ObjectKind::DataView(_)) => Ok(*o),
        _ => agent.type_error("Receiver is not a DataView"),
    }
}

fn view_data(agent: &Agent, o: Gc<Object>) -> &DataViewData {
    match &agent.heap[o].kind {
        ObjectKind::DataView(d) => d,
        _ => unreachable!("DataView"),
    }
}

/// GetViewByteLength (§25.3.1.3) si no IsViewOutOfBounds (§25.3.1.4).
fn view_byte_length(agent: &Agent, o: Gc<Object>) -> Option<usize> {
    let d = view_data(agent, o);
    if is_detached(agent, d.buffer) {
        return None;
    }
    let buf_len = byte_length(agent, d.buffer);
    let end = match d.byte_length {
        None => buf_len,
        Some(l) => d.byte_offset + l,
    };
    if d.byte_offset > buf_len || end > buf_len {
        return None;
    }
    Some(end - d.byte_offset)
}

fn get_buffer(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_view(agent, &a.this)?;
    Ok(Value::Object(view_data(agent, o).buffer))
}

fn get_byte_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_view(agent, &a.this)?;
    match view_byte_length(agent, o) {
        Some(l) => Ok(Value::Number(l as f64)),
        None => agent.type_error("DataView is detached or out of bounds"),
    }
}

fn get_byte_offset(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_view(agent, &a.this)?;
    if view_byte_length(agent, o).is_none() {
        return agent.type_error("DataView is detached or out of bounds");
    }
    Ok(Value::Number(view_data(agent, o).byte_offset as f64))
}

/// GetViewValue (§25.3.1.5).
fn get_view_value(agent: &mut Agent, a: &CallArgs<'_>, ty: ElementType) -> JsResult<Value> {
    let o = this_view(agent, &a.this)?;
    let index = agent.to_index(&a.arg(0))?;
    let little = crate::abstract_ops::conversion::to_boolean(agent, &a.arg(1));
    let Some(size) = view_byte_length(agent, o) else {
        return agent.type_error("DataView is detached or out of bounds");
    };
    if index + ty.size() as u64 > size as u64 {
        return agent.range_error("Offset is outside the bounds of the DataView");
    }
    let d = view_data(agent, o);
    let (buffer, offset) = (d.buffer, d.byte_offset);
    Ok(get_value_from_buffer(
        agent,
        buffer,
        index as usize + offset,
        ty,
        little,
    ))
}

/// SetViewValue (§25.3.1.6).
fn set_view_value(agent: &mut Agent, a: &CallArgs<'_>, ty: ElementType) -> JsResult<Value> {
    let o = this_view(agent, &a.this)?;
    let index = agent.to_index(&a.arg(0))?;
    let v = to_element_value(agent, ty, &a.arg(1))?;
    let little = crate::abstract_ops::conversion::to_boolean(agent, &a.arg(2));
    let Some(size) = view_byte_length(agent, o) else {
        return agent.type_error("DataView is detached or out of bounds");
    };
    if index + ty.size() as u64 > size as u64 {
        return agent.range_error("Offset is outside the bounds of the DataView");
    }
    let d = view_data(agent, o);
    let (buffer, offset) = (d.buffer, d.byte_offset);
    set_value_in_buffer(agent, buffer, index as usize + offset, ty, &v, little);
    Ok(Value::Undefined)
}

macro_rules! view_methods {
    ($($get:ident, $set:ident, $ty:ident;)*) => {
        $(
            fn $get(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
                get_view_value(agent, a, ElementType::$ty)
            }
            fn $set(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
                set_view_value(agent, a, ElementType::$ty)
            }
        )*
    };
}

view_methods! {
    get_big_int64, set_big_int64, BigInt64;
    get_big_uint64, set_big_uint64, BigUint64;
    get_float16, set_float16, Float16;
    get_float32, set_float32, Float32;
    get_float64, set_float64, Float64;
    get_int8, set_int8, Int8;
    get_int16, set_int16, Int16;
    get_int32, set_int32, Int32;
    get_uint8, set_uint8, Uint8;
    get_uint16, set_uint16, Uint16;
    get_uint32, set_uint32, Uint32;
}
