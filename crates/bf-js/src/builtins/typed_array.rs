//! TypedArrays: los objetos exóticos (§10.4.5), %TypedArray% (§23.2.1–3) y los doce
//! constructores (§23.2.5–7), incluido Float16Array.
//!
//! Los métodos internos de `crate::object::internal` delegan aquí las claves numéricas
//! (CanonicalNumericIndexString) de los TypedArray. La longitud se calcula con
//! MakeTypedArrayWithBufferWitnessRecord en cada acceso, así que los arrays sobre buffers
//! redimensionables siguen al buffer (length-tracking) y los que quedan fuera de rango o
//! sobre un buffer detached se ven vacíos.

use std::cmp::Ordering;

use crate::builtins::array_buffer::{
    self as ab, ElementType, allocate_default, byte_length, get_value_from_buffer, is_detached,
    is_fixed_length, is_shared, relative_index, set_value_in_buffer, to_element_value,
};
use crate::builtins::{FnEntry, accessor, constructor, methods, slots};
use crate::heap::Gc;
use crate::object::{IterKind, Object, ObjectKind, PropertyDescriptor};
use crate::string::JsString;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::realm::Intrinsics;
use crate::vm::{Agent, JsResult, RealmId};

/// Los internal slots de un TypedArray.
#[derive(Debug)]
pub struct TypedArrayData {
    /// `[[ViewedArrayBuffer]]`.
    pub buffer: Gc<Object>,
    /// `[[TypedArrayName]]` y `[[ContentType]]` salen del tipo de elemento.
    pub kind: ElementType,
    pub byte_offset: usize,
    /// `[[ArrayLength]]`; `None` es auto (sigue al buffer).
    pub array_length: Option<usize>,
}

/// Table 71 en orden.
const KINDS: [ElementType; 12] = [
    ElementType::Int8,
    ElementType::Uint8,
    ElementType::Uint8Clamped,
    ElementType::Int16,
    ElementType::Uint16,
    ElementType::Int32,
    ElementType::Uint32,
    ElementType::BigInt64,
    ElementType::BigUint64,
    ElementType::Float16,
    ElementType::Float32,
    ElementType::Float64,
];

fn kind_name(k: ElementType) -> &'static str {
    match k {
        ElementType::Int8 => "Int8Array",
        ElementType::Uint8 => "Uint8Array",
        ElementType::Uint8Clamped => "Uint8ClampedArray",
        ElementType::Int16 => "Int16Array",
        ElementType::Uint16 => "Uint16Array",
        ElementType::Int32 => "Int32Array",
        ElementType::Uint32 => "Uint32Array",
        ElementType::BigInt64 => "BigInt64Array",
        ElementType::BigUint64 => "BigUint64Array",
        ElementType::Float16 => "Float16Array",
        ElementType::Float32 => "Float32Array",
        ElementType::Float64 => "Float64Array",
    }
}

fn default_proto(k: ElementType) -> fn(&Intrinsics) -> Gc<Object> {
    match k {
        ElementType::Int8 => |i| i.int8_array_prototype,
        ElementType::Uint8 => |i| i.uint8_array_prototype,
        ElementType::Uint8Clamped => |i| i.uint8_clamped_array_prototype,
        ElementType::Int16 => |i| i.int16_array_prototype,
        ElementType::Uint16 => |i| i.uint16_array_prototype,
        ElementType::Int32 => |i| i.int32_array_prototype,
        ElementType::Uint32 => |i| i.uint32_array_prototype,
        ElementType::BigInt64 => |i| i.bigint64_array_prototype,
        ElementType::BigUint64 => |i| i.biguint64_array_prototype,
        ElementType::Float16 => |i| i.float16_array_prototype,
        ElementType::Float32 => |i| i.float32_array_prototype,
        ElementType::Float64 => |i| i.float64_array_prototype,
    }
}

fn intrinsic_ctor(i: &Intrinsics, k: ElementType) -> Gc<Object> {
    match k {
        ElementType::Int8 => i.int8_array_ctor,
        ElementType::Uint8 => i.uint8_array_ctor,
        ElementType::Uint8Clamped => i.uint8_clamped_array_ctor,
        ElementType::Int16 => i.int16_array_ctor,
        ElementType::Uint16 => i.uint16_array_ctor,
        ElementType::Int32 => i.int32_array_ctor,
        ElementType::Uint32 => i.uint32_array_ctor,
        ElementType::BigInt64 => i.bigint64_array_ctor,
        ElementType::BigUint64 => i.biguint64_array_ctor,
        ElementType::Float16 => i.float16_array_ctor,
        ElementType::Float32 => i.float32_array_ctor,
        ElementType::Float64 => i.float64_array_ctor,
    }
}

// --- Operaciones abstractas (§10.4.5.9–19) ---

/// Los slots de `o` si es un TypedArray.
pub fn ta_data(agent: &Agent, o: Gc<Object>) -> Option<&TypedArrayData> {
    match &agent.heap[o].kind {
        ObjectKind::TypedArray(d) => Some(d),
        _ => None,
    }
}

pub(crate) fn kind_of(agent: &Agent, o: Gc<Object>) -> ElementType {
    ta_data(agent, o).map_or(ElementType::Uint8, |d| d.kind)
}

/// TypedArrayLength (§10.4.5.13) sobre MakeTypedArrayWithBufferWitnessRecord: `None` si
/// IsTypedArrayOutOfBounds (§10.4.5.14).
pub fn ta_length(agent: &Agent, o: Gc<Object>) -> Option<usize> {
    let d = ta_data(agent, o)?;
    if is_detached(agent, d.buffer) {
        return None;
    }
    let buf_len = byte_length(agent, d.buffer);
    let size = d.kind.size();
    match d.array_length {
        None => {
            if d.byte_offset > buf_len {
                None
            } else {
                Some((buf_len - d.byte_offset) / size)
            }
        }
        Some(n) => {
            if d.byte_offset > buf_len || d.byte_offset + n * size > buf_len {
                None
            } else {
                Some(n)
            }
        }
    }
}

/// TypedArrayByteLength (§10.4.5.12).
fn ta_byte_length(agent: &Agent, o: Gc<Object>) -> usize {
    ta_length(agent, o).map_or(0, |n| n * kind_of(agent, o).size())
}

/// IsTypedArrayFixedLength (§10.4.5.15).
fn is_fixed(agent: &Agent, o: Gc<Object>) -> bool {
    match ta_data(agent, o) {
        Some(d) => {
            d.array_length.is_some()
                && (is_fixed_length(agent, d.buffer) || is_shared(agent, d.buffer))
        }
        None => true,
    }
}

/// TypedArray [[PreventExtensions]] (§10.4.5.1): `false` si no es de longitud fija.
pub fn can_prevent_extensions(agent: &Agent, o: Gc<Object>) -> bool {
    ta_data(agent, o).is_none() || is_fixed(agent, o)
}

/// IsValidIntegerIndex (§10.4.5.16).
pub fn is_valid_integer_index(agent: &Agent, o: Gc<Object>, index: f64) -> bool {
    if !index.is_finite() || index.fract() != 0.0 || (index == 0.0 && index.is_sign_negative()) {
        return false;
    }
    match ta_length(agent, o) {
        Some(len) => index >= 0.0 && index < len as f64,
        None => false,
    }
}

/// TypedArrayGetElement (§10.4.5.17).
pub fn get_element(agent: &mut Agent, o: Gc<Object>, index: f64) -> Value {
    if !is_valid_integer_index(agent, o, index) {
        return Value::Undefined;
    }
    let d = ta_data(agent, o).expect("typed array");
    let (buffer, kind, offset) = (d.buffer, d.kind, d.byte_offset);
    let byte_index = index as usize * kind.size() + offset;
    get_value_from_buffer(agent, buffer, byte_index, kind, true)
}

/// TypedArraySetElement (§10.4.5.18).
pub fn set_element(agent: &mut Agent, o: Gc<Object>, index: f64, v: &Value) -> JsResult<()> {
    let kind = kind_of(agent, o);
    let num = to_element_value(agent, kind, v)?;
    write_element(agent, o, index, &num);
    Ok(())
}

/// Escribe un valor ya convertido si el índice es válido.
fn write_element(agent: &mut Agent, o: Gc<Object>, index: f64, num: &Value) {
    if is_valid_integer_index(agent, o, index) {
        let d = ta_data(agent, o).expect("typed array");
        let (buffer, kind, offset) = (d.buffer, d.kind, d.byte_offset);
        let byte_index = index as usize * kind.size() + offset;
        set_value_in_buffer(agent, buffer, byte_index, kind, num, true);
    }
}

impl Agent {
    /// La clave numérica (CanonicalNumericIndexString) si `o` es un TypedArray.
    pub(crate) fn ta_numeric_key(&self, o: Gc<Object>, key: &PropertyKey) -> Option<f64> {
        if !matches!(self.heap[o].kind, ObjectKind::TypedArray(_)) {
            return None;
        }
        match key {
            PropertyKey::Index(i) => Some(f64::from(*i)),
            PropertyKey::String(s) => self.canonical_numeric_index_string(s),
            PropertyKey::Symbol(_) => None,
        }
    }
}

// --- Métodos internos (§10.4.5.2–8) ---

/// [[GetOwnProperty]] con una clave numérica (§10.4.5.2).
pub fn get_own_element(agent: &mut Agent, o: Gc<Object>, index: f64) -> Option<PropertyDescriptor> {
    if !is_valid_integer_index(agent, o, index) {
        return None;
    }
    let v = get_element(agent, o, index);
    Some(PropertyDescriptor::data(v, true, true, true))
}

/// [[DefineOwnProperty]] (§10.4.5.4).
pub fn define_own_property(
    agent: &mut Agent,
    o: Gc<Object>,
    key: PropertyKey,
    desc: PropertyDescriptor,
) -> JsResult<bool> {
    let Some(index) = agent.ta_numeric_key(o, &key) else {
        return Ok(agent.ordinary_define_own_property(o, key, desc));
    };
    if !is_valid_integer_index(agent, o, index) {
        return Ok(false);
    }
    if desc.configurable == Some(false)
        || desc.enumerable == Some(false)
        || desc.is_accessor()
        || desc.writable == Some(false)
    {
        return Ok(false);
    }
    if let Some(v) = &desc.value {
        set_element(agent, o, index, v)?;
    }
    Ok(true)
}

/// [[Set]] con una clave numérica (§10.4.5.6).
pub fn set(
    agent: &mut Agent,
    o: Gc<Object>,
    index: f64,
    key: PropertyKey,
    v: Value,
    receiver: Value,
) -> JsResult<bool> {
    if matches!(&receiver, Value::Object(r) if *r == o) {
        set_element(agent, o, index, &v)?;
        return Ok(true);
    }
    if !is_valid_integer_index(agent, o, index) {
        return Ok(true);
    }
    let own = get_own_element(agent, o, index);
    agent.ordinary_set_with_own_descriptor(o, key, v, receiver, own)
}

/// [[OwnPropertyKeys]] (§10.4.5.8).
pub fn own_keys(agent: &Agent, o: Gc<Object>) -> Vec<PropertyKey> {
    let mut out: Vec<PropertyKey> = Vec::new();
    if let Some(len) = ta_length(agent, o) {
        out.extend((0..len as u32).map(PropertyKey::Index));
    }
    let obj = &agent.heap[o];
    obj.props.push_named_keys(&mut out);
    out
}

/// La longitud para %ArrayIteratorPrototype%.next (§23.1.5.1 paso 2.b.i–ii) si `o` es un
/// TypedArray: TypeError si está fuera de rango.
pub fn iteration_length(agent: &mut Agent, o: Gc<Object>) -> Option<JsResult<f64>> {
    ta_data(agent, o)?;
    Some(match ta_length(agent, o) {
        Some(n) => Ok(n as f64),
        None => agent.type_error("Cannot perform %ArrayIteratorPrototype%.next on a detached or out-of-bounds TypedArray"),
    })
}

// --- Creación (§23.2.5.1.1–7, §23.2.4) ---

fn alloc_ta(
    agent: &mut Agent,
    proto: Gc<Object>,
    kind: ElementType,
    buffer: Gc<Object>,
    byte_offset: usize,
    array_length: Option<usize>,
) -> Gc<Object> {
    agent.alloc(Object::new(
        Some(proto),
        ObjectKind::TypedArray(Box::new(TypedArrayData {
            buffer,
            kind,
            byte_offset,
            array_length,
        })),
    ))
}

/// AllocateTypedArray con longitud (§23.2.5.1.1 + AllocateTypedArrayBuffer §23.2.5.1.6).
fn allocate_with_length(
    agent: &mut Agent,
    proto: Gc<Object>,
    kind: ElementType,
    len: u64,
) -> JsResult<Gc<Object>> {
    let byte_len = len.saturating_mul(kind.size() as u64);
    let buffer = allocate_default(agent, byte_len)?;
    Ok(alloc_ta(agent, proto, kind, buffer, 0, Some(len as usize)))
}

/// TypedArray ( ...args ) (§23.2.5.1).
fn ta_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let idx = slots(agent, a.callee)[0].as_number().unwrap_or(0.0) as usize;
    let kind = KINDS[idx];
    let Some(nt) = a.new_target else {
        return agent.type_error(&format!("Constructor {} requires 'new'", kind_name(kind)));
    };
    let first = a.arg(0);
    let Value::Object(fo) = first else {
        let len = agent.to_index(&first)?;
        let proto = agent.get_prototype_from_constructor(nt, default_proto(kind))?;
        return Ok(Value::Object(allocate_with_length(
            agent, proto, kind, len,
        )?));
    };
    let proto = agent.get_prototype_from_constructor(nt, default_proto(kind))?;
    let o = match &agent.heap[fo].kind {
        ObjectKind::TypedArray(_) => init_from_typed_array(agent, proto, kind, fo)?,
        ObjectKind::ArrayBuffer(_) => {
            init_from_array_buffer(agent, proto, kind, fo, &a.arg(1), &a.arg(2))?
        }
        _ => {
            let key = PropertyKey::Symbol(agent.well_known.iterator);
            match agent.get_method_of(&first, &key)? {
                Some(m) => {
                    let (it, next) = agent.get_iterator_from_method(&first, &m)?;
                    let mut values = Vec::new();
                    while let Some(v) = agent.iterator_step_value(&it, &next)? {
                        values.push(v);
                    }
                    let o = allocate_with_length(agent, proto, kind, values.len() as u64)?;
                    for (k, v) in values.iter().enumerate() {
                        set_element(agent, o, k as f64, v)?;
                    }
                    o
                }
                None => {
                    // InitializeTypedArrayFromArrayLike (§23.2.5.1.5).
                    let len = agent.length_of_array_like(fo)?;
                    let o = allocate_with_length(agent, proto, kind, len as u64)?;
                    for k in 0..len as u64 {
                        let v =
                            agent.get(fo, &PropertyKey::from_number(k as f64), first.clone())?;
                        set_element(agent, o, k as f64, &v)?;
                    }
                    o
                }
            }
        }
    };
    Ok(Value::Object(o))
}

/// InitializeTypedArrayFromTypedArray (§23.2.5.1.2).
fn init_from_typed_array(
    agent: &mut Agent,
    proto: Gc<Object>,
    kind: ElementType,
    src: Gc<Object>,
) -> JsResult<Gc<Object>> {
    let sd = ta_data(agent, src).expect("typed array");
    let (src_buffer, src_kind, src_offset) = (sd.buffer, sd.kind, sd.byte_offset);
    let Some(len) = ta_length(agent, src) else {
        return agent.type_error("Source TypedArray is detached or out of bounds");
    };
    let byte_len = kind.size() * len;
    let data = if kind == src_kind {
        ab::clone_array_buffer(agent, src_buffer, src_offset, byte_len)?
    } else {
        let data = allocate_default(agent, byte_len as u64)?;
        if src_kind.is_bigint() != kind.is_bigint() {
            return agent.type_error("Content types of source and target TypedArrays differ");
        }
        for i in 0..len {
            let v = get_value_from_buffer(
                agent,
                src_buffer,
                src_offset + i * src_kind.size(),
                src_kind,
                true,
            );
            set_value_in_buffer(agent, data, i * kind.size(), kind, &v, true);
        }
        data
    };
    Ok(alloc_ta(agent, proto, kind, data, 0, Some(len)))
}

/// InitializeTypedArrayFromArrayBuffer (§23.2.5.1.3).
fn init_from_array_buffer(
    agent: &mut Agent,
    proto: Gc<Object>,
    kind: ElementType,
    buffer: Gc<Object>,
    byte_offset: &Value,
    length: &Value,
) -> JsResult<Gc<Object>> {
    let size = kind.size();
    let offset = agent.to_index(byte_offset)?;
    if !offset.is_multiple_of(size as u64) {
        return agent.range_error(&format!(
            "start offset of {} should be a multiple of {size}",
            kind_name(kind)
        ));
    }
    let fixed = is_fixed_length(agent, buffer);
    let new_length = if length.is_undefined() {
        None
    } else {
        Some(agent.to_index(length)?)
    };
    if is_detached(agent, buffer) {
        return agent.type_error("Cannot construct a TypedArray on a detached ArrayBuffer");
    }
    let buf_len = byte_length(agent, buffer) as u64;
    if new_length.is_none() && !fixed {
        if offset > buf_len {
            return agent.range_error("Start offset is outside the bounds of the buffer");
        }
        return Ok(alloc_ta(agent, proto, kind, buffer, offset as usize, None));
    }
    let new_byte_len = match new_length {
        None => {
            if !buf_len.is_multiple_of(size as u64) {
                return agent.range_error(&format!(
                    "byte length of {} should be a multiple of {size}",
                    kind_name(kind)
                ));
            }
            if offset > buf_len {
                return agent.range_error("Start offset is outside the bounds of the buffer");
            }
            buf_len - offset
        }
        Some(n) => {
            let b = n.saturating_mul(size as u64);
            if offset.saturating_add(b) > buf_len {
                return agent.range_error("Invalid typed array length");
            }
            b
        }
    };
    let len = (new_byte_len / size as u64) as usize;
    Ok(alloc_ta(
        agent,
        proto,
        kind,
        buffer,
        offset as usize,
        Some(len),
    ))
}

/// ValidateTypedArray (§23.2.4.4): el TypedArray y su longitud.
pub(crate) fn validate(agent: &mut Agent, v: &Value) -> JsResult<(Gc<Object>, usize)> {
    let o = require_ta(agent, v)?;
    match ta_length(agent, o) {
        Some(n) => Ok((o, n)),
        None => agent.type_error("TypedArray is detached or out of bounds"),
    }
}

/// RequireInternalSlot(O, [[TypedArrayName]]).
fn require_ta(agent: &mut Agent, v: &Value) -> JsResult<Gc<Object>> {
    match v {
        Value::Object(o) if matches!(agent.heap[*o].kind, ObjectKind::TypedArray(_)) => Ok(*o),
        _ => agent.type_error("this is not a typed array."),
    }
}

/// TypedArrayCreateFromConstructor (§23.2.4.2).
fn create_from_constructor(
    agent: &mut Agent,
    ctor: Gc<Object>,
    args: &[Value],
) -> JsResult<Gc<Object>> {
    let new = agent.construct(ctor, args, None)?;
    let (o, len) = validate(agent, &new)?;
    if let [Value::Number(n)] = args
        && (len as f64) < *n
    {
        return agent.type_error("TypedArray species constructor returned a too short array");
    }
    Ok(o)
}

/// TypedArraySpeciesCreate (§23.2.4.1).
fn species_create(agent: &mut Agent, exemplar: Gc<Object>, args: &[Value]) -> JsResult<Gc<Object>> {
    let kind = kind_of(agent, exemplar);
    let default = intrinsic_ctor(agent.intrinsics(), kind);
    let c = agent.species_constructor(exemplar, default)?;
    let r = create_from_constructor(agent, c, args)?;
    if kind_of(agent, r).is_bigint() != kind.is_bigint() {
        return agent
            .type_error("TypedArray species constructor returned a different content type");
    }
    Ok(r)
}

/// TypedArrayCreateSameType (§23.2.4.3).
fn create_same_type(agent: &mut Agent, exemplar: Gc<Object>, len: usize) -> JsResult<Gc<Object>> {
    let kind = kind_of(agent, exemplar);
    let c = intrinsic_ctor(agent.intrinsics(), kind);
    create_from_constructor(agent, c, &[Value::Number(len as f64)])
}

// --- Intrínsecos ---

pub fn init(agent: &mut Agent, realm: RealmId) {
    let i = agent.realms[realm].intrinsics.clone();
    // %TypedArray.prototype% (§23.2.3).
    let tap = agent.new_object(Some(i.object_prototype));
    const PROTO: &[FnEntry] = &[
        ("at", 1, at),
        ("copyWithin", 2, copy_within),
        ("entries", 0, entries),
        ("every", 1, every),
        ("fill", 1, fill),
        ("filter", 1, filter),
        ("find", 1, find),
        ("findIndex", 1, find_index),
        ("findLast", 1, find_last),
        ("findLastIndex", 1, find_last_index),
        ("forEach", 1, for_each),
        ("includes", 1, includes),
        ("indexOf", 1, index_of),
        ("join", 1, join),
        ("keys", 0, keys),
        ("lastIndexOf", 1, last_index_of),
        ("map", 1, map),
        ("reduce", 1, reduce),
        ("reduceRight", 1, reduce_right),
        ("reverse", 0, reverse),
        ("set", 1, set_method),
        ("slice", 2, slice),
        ("some", 1, some),
        ("sort", 1, sort),
        ("subarray", 2, subarray),
        ("toLocaleString", 0, to_locale_string),
        ("toReversed", 0, to_reversed),
        ("toSorted", 1, to_sorted),
        ("with", 2, with),
    ];
    methods(agent, tap, PROTO);
    let values = crate::builtins::method(agent, tap, "values", 0, values);
    let it = PropertyKey::Symbol(agent.well_known.iterator);
    agent.put_data(tap, it, Value::Object(values), true, false, true);
    // `toString` es el mismo objeto que Array.prototype.toString (§23.2.3.32).
    let ts = array_to_string_function(agent, i.array_prototype);
    agent.put_data(
        tap,
        PropertyKey::from("toString"),
        Value::Object(ts),
        true,
        false,
        true,
    );
    let getters: &[(&str, crate::vm::function::NativeFn)] = &[
        ("buffer", get_buffer),
        ("byteLength", get_byte_length),
        ("byteOffset", get_byte_offset),
        ("length", get_length),
    ];
    for &(name, f) in getters {
        accessor(agent, tap, PropertyKey::from(name), name, Some(f), None);
    }
    let tag = PropertyKey::Symbol(agent.well_known.to_string_tag);
    accessor(
        agent,
        tap,
        tag,
        "[Symbol.toStringTag]",
        Some(get_to_string_tag),
        None,
    );
    // %TypedArray% (§23.2.1–2).
    let ta = constructor(agent, "TypedArray", 0, typed_array_abstract, tap);
    methods(agent, ta, &[("from", 1, from), ("of", 0, of)]);
    let species = PropertyKey::Symbol(agent.well_known.species);
    accessor(
        agent,
        ta,
        species,
        "[Symbol.species]",
        Some(species_getter),
        None,
    );
    agent.realms[realm].intrinsics.typed_array_ctor = ta;
    agent.realms[realm].intrinsics.typed_array_prototype = tap;
    // Los constructores concretos (§23.2.6–7).
    for (idx, &kind) in KINDS.iter().enumerate() {
        let p = agent.new_object(Some(tap));
        let c = constructor(agent, kind_name(kind), 3, ta_ctor, p);
        if let ObjectKind::Builtin(b) = &mut agent.heap[c].kind {
            b.slots = vec![Value::Number(idx as f64)];
        }
        agent.heap[c].proto = Some(ta);
        let bpe = Value::Number(kind.size() as f64);
        crate::builtins::frozen_prop(agent, c, "BYTES_PER_ELEMENT", bpe.clone());
        crate::builtins::frozen_prop(agent, p, "BYTES_PER_ELEMENT", bpe);
        let ints = &mut agent.realms[realm].intrinsics;
        let (cs, ps) = match kind {
            ElementType::Int8 => (&mut ints.int8_array_ctor, &mut ints.int8_array_prototype),
            ElementType::Uint8 => (&mut ints.uint8_array_ctor, &mut ints.uint8_array_prototype),
            ElementType::Uint8Clamped => (
                &mut ints.uint8_clamped_array_ctor,
                &mut ints.uint8_clamped_array_prototype,
            ),
            ElementType::Int16 => (&mut ints.int16_array_ctor, &mut ints.int16_array_prototype),
            ElementType::Uint16 => (
                &mut ints.uint16_array_ctor,
                &mut ints.uint16_array_prototype,
            ),
            ElementType::Int32 => (&mut ints.int32_array_ctor, &mut ints.int32_array_prototype),
            ElementType::Uint32 => (
                &mut ints.uint32_array_ctor,
                &mut ints.uint32_array_prototype,
            ),
            ElementType::BigInt64 => (
                &mut ints.bigint64_array_ctor,
                &mut ints.bigint64_array_prototype,
            ),
            ElementType::BigUint64 => (
                &mut ints.biguint64_array_ctor,
                &mut ints.biguint64_array_prototype,
            ),
            ElementType::Float16 => (
                &mut ints.float16_array_ctor,
                &mut ints.float16_array_prototype,
            ),
            ElementType::Float32 => (
                &mut ints.float32_array_ctor,
                &mut ints.float32_array_prototype,
            ),
            ElementType::Float64 => (
                &mut ints.float64_array_ctor,
                &mut ints.float64_array_prototype,
            ),
        };
        *cs = c;
        *ps = p;
    }
}

/// Array.prototype.toString; si `array.rs` todavía no lo define, se crea aquí y se instala
/// también en Array.prototype para que ambos sean el mismo objeto.
fn array_to_string_function(agent: &mut Agent, array_prototype: Gc<Object>) -> Gc<Object> {
    let k = PropertyKey::from("toString");
    if let Some(Value::Object(f)) = agent.heap[array_prototype]
        .props
        .get(&k)
        .and_then(|p| p.to_descriptor().value)
    {
        return f;
    }
    crate::builtins::method(agent, array_prototype, "toString", 0, array_to_string)
}

/// Array.prototype.toString (§23.1.3.36).
fn array_to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let array = agent.to_object(&a.this)?;
    let mut func = agent.get(array, &PropertyKey::from("join"), Value::Object(array))?;
    if !agent.is_callable(&func) {
        let op = agent.intrinsics().object_prototype;
        func = agent.get(op, &PropertyKey::from("toString"), Value::Object(op))?;
    }
    agent.call(&func, Value::Object(array), &[])
}

fn species_getter(_: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(a.this.clone())
}

/// %TypedArray% ( ) (§23.2.1.1).
fn typed_array_abstract(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    agent.type_error("Abstract class TypedArray not directly constructable")
}

/// %TypedArray%.from (§23.2.2.1).
fn from(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let c = a.this.clone();
    if !agent.is_constructor(&c) {
        return agent.type_error("TypedArray.from: this is not a constructor");
    }
    let c = c.as_object().expect("constructor");
    let mapfn = a.arg(1);
    let mapping = !mapfn.is_undefined();
    if mapping && !agent.is_callable(&mapfn) {
        return agent.type_error("TypedArray.from: mapfn is not callable");
    }
    let this_arg = a.arg(2);
    let source = a.arg(0);
    let key = PropertyKey::Symbol(agent.well_known.iterator);
    if let Some(m) = agent.get_method_of(&source, &key)? {
        let (it, next) = agent.get_iterator_from_method(&source, &m)?;
        let mut values = Vec::new();
        while let Some(v) = agent.iterator_step_value(&it, &next)? {
            values.push(v);
        }
        let target = create_from_constructor(agent, c, &[Value::Number(values.len() as f64)])?;
        for (k, v) in values.into_iter().enumerate() {
            let mapped = if mapping {
                agent.call(&mapfn, this_arg.clone(), &[v, Value::Number(k as f64)])?
            } else {
                v
            };
            agent.set_prop(target, PropertyKey::from_number(k as f64), mapped)?;
        }
        return Ok(Value::Object(target));
    }
    let array_like = agent.to_object(&source)?;
    let len = agent.length_of_array_like(array_like)?;
    let target = create_from_constructor(agent, c, &[Value::Number(len)])?;
    for k in 0..len as u64 {
        let pk = PropertyKey::from_number(k as f64);
        let v = agent.get(array_like, &pk, Value::Object(array_like))?;
        let mapped = if mapping {
            agent.call(&mapfn, this_arg.clone(), &[v, Value::Number(k as f64)])?
        } else {
            v
        };
        agent.set_prop(target, pk, mapped)?;
    }
    Ok(Value::Object(target))
}

/// %TypedArray%.of (§23.2.2.2).
fn of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let c = a.this.clone();
    if !agent.is_constructor(&c) {
        return agent.type_error("TypedArray.of: this is not a constructor");
    }
    let c = c.as_object().expect("constructor");
    let target = create_from_constructor(agent, c, &[Value::Number(a.len() as f64)])?;
    for (k, v) in a.args.iter().enumerate() {
        agent.set_prop(target, PropertyKey::from_number(k as f64), v.clone())?;
    }
    Ok(Value::Object(target))
}

// --- Getters ---

fn get_buffer(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = require_ta(agent, &a.this)?;
    Ok(Value::Object(ta_data(agent, o).expect("ta").buffer))
}

fn get_byte_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = require_ta(agent, &a.this)?;
    Ok(Value::Number(ta_byte_length(agent, o) as f64))
}

fn get_byte_offset(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = require_ta(agent, &a.this)?;
    if ta_length(agent, o).is_none() {
        return Ok(Value::Number(0.0));
    }
    Ok(Value::Number(
        ta_data(agent, o).expect("ta").byte_offset as f64,
    ))
}

fn get_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = require_ta(agent, &a.this)?;
    Ok(Value::Number(ta_length(agent, o).unwrap_or(0) as f64))
}

/// get %TypedArray%.prototype [ @@toStringTag ] (§23.2.3.38).
fn get_to_string_tag(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    match &a.this {
        Value::Object(o) => match &agent.heap[*o].kind {
            ObjectKind::TypedArray(d) => Ok(Value::str(kind_name(d.kind))),
            _ => Ok(Value::Undefined),
        },
        _ => Ok(Value::Undefined),
    }
}

// --- Métodos (§23.2.3) ---

fn callback(agent: &mut Agent, v: &Value) -> JsResult<Value> {
    if !agent.is_callable(v) {
        return agent.type_error(&format!("{} is not a function", agent.describe(v)));
    }
    Ok(v.clone())
}

/// %TypedArray%.prototype.at (§23.2.3.1).
fn at(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = validate(agent, &a.this)?;
    let rel = agent.to_integer_or_infinity(&a.arg(0))?;
    let k = if rel >= 0.0 { rel } else { len as f64 + rel };
    if k < 0.0 || k >= len as f64 {
        return Ok(Value::Undefined);
    }
    Ok(get_element(agent, o, k))
}

/// %TypedArray%.prototype.copyWithin (§23.2.3.6).
fn copy_within(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = validate(agent, &a.this)?;
    let lenf = len as f64;
    let target = relative_index(agent, &a.arg(0), lenf, 0.0)?;
    let start = relative_index(agent, &a.arg(1), lenf, 0.0)?;
    let end = relative_index(agent, &a.arg(2), lenf, lenf)?;
    let count = (end - start).min(lenf - target);
    if count > 0.0 {
        let Some(len) = ta_length(agent, o) else {
            return agent.type_error("TypedArray is detached or out of bounds");
        };
        let d = ta_data(agent, o).expect("ta");
        let (buffer, size, offset) = (d.buffer, d.kind.size(), d.byte_offset);
        let limit = len * size + offset;
        let mut to = target as usize * size + offset;
        let mut from = start as usize * size + offset;
        let mut count_bytes = count as usize * size;
        let backwards = from < to && to < from + count_bytes;
        if backwards {
            from += count_bytes - 1;
            to += count_bytes - 1;
        }
        while count_bytes > 0 {
            if from < limit && to < limit {
                let b = ab::read_raw(agent, buffer, from, 1, true);
                ab::write_raw(agent, buffer, to, 1, b, true);
                if backwards {
                    from = from.wrapping_sub(1);
                    to = to.wrapping_sub(1);
                } else {
                    from += 1;
                    to += 1;
                }
                count_bytes -= 1;
            } else {
                count_bytes = 0;
            }
        }
    }
    Ok(Value::Object(o))
}

fn iterator(agent: &mut Agent, a: &CallArgs<'_>, kind: IterKind) -> JsResult<Value> {
    let (o, _) = validate(agent, &a.this)?;
    Ok(crate::builtins::iterator::create_array_iterator(
        agent, o, kind,
    ))
}

fn entries(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    iterator(agent, a, IterKind::Entries)
}

fn keys(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    iterator(agent, a, IterKind::Keys)
}

fn values(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    iterator(agent, a, IterKind::Values)
}

/// every/some/forEach (§23.2.3.7, §23.2.3.27, §23.2.3.15): `stop` es el resultado que corta.
fn iterate(agent: &mut Agent, a: &CallArgs<'_>, stop: Option<bool>) -> JsResult<Value> {
    let (o, len) = validate(agent, &a.this)?;
    let f = callback(agent, &a.arg(0))?;
    let this_arg = a.arg(1);
    for k in 0..len {
        let v = get_element(agent, o, k as f64);
        let r = agent.call(
            &f,
            this_arg.clone(),
            &[v, Value::Number(k as f64), Value::Object(o)],
        )?;
        if let Some(s) = stop
            && crate::abstract_ops::conversion::to_boolean(agent, &r) == s
        {
            return Ok(Value::Boolean(s));
        }
    }
    Ok(match stop {
        Some(s) => Value::Boolean(!s),
        None => Value::Undefined,
    })
}

fn every(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    iterate(agent, a, Some(false))
}

fn some(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    iterate(agent, a, Some(true))
}

fn for_each(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    iterate(agent, a, None)
}

/// %TypedArray%.prototype.fill (§23.2.3.9).
fn fill(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = validate(agent, &a.this)?;
    let kind = kind_of(agent, o);
    let value = to_element_value(agent, kind, &a.arg(0))?;
    let lenf = len as f64;
    let start = relative_index(agent, &a.arg(1), lenf, 0.0)?;
    let end = relative_index(agent, &a.arg(2), lenf, lenf)?;
    let Some(len) = ta_length(agent, o) else {
        return agent.type_error("TypedArray is detached or out of bounds");
    };
    let end = end.min(len as f64);
    let mut k = start;
    while k < end {
        write_element(agent, o, k, &value);
        k += 1.0;
    }
    Ok(Value::Object(o))
}

/// %TypedArray%.prototype.filter (§23.2.3.10).
fn filter(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = validate(agent, &a.this)?;
    let f = callback(agent, &a.arg(0))?;
    let this_arg = a.arg(1);
    let mut kept = Vec::new();
    for k in 0..len {
        let v = get_element(agent, o, k as f64);
        let r = agent.call(
            &f,
            this_arg.clone(),
            &[v.clone(), Value::Number(k as f64), Value::Object(o)],
        )?;
        if crate::abstract_ops::conversion::to_boolean(agent, &r) {
            kept.push(v);
        }
    }
    let new = species_create(agent, o, &[Value::Number(kept.len() as f64)])?;
    for (n, v) in kept.iter().enumerate() {
        set_element(agent, new, n as f64, v)?;
    }
    Ok(Value::Object(new))
}

/// FindViaPredicate (§23.1.3.12.1) sobre un TypedArray.
fn find_via(agent: &mut Agent, a: &CallArgs<'_>, ascending: bool, index: bool) -> JsResult<Value> {
    let (o, len) = validate(agent, &a.this)?;
    let f = callback(agent, &a.arg(0))?;
    let this_arg = a.arg(1);
    for i in 0..len {
        let k = if ascending { i } else { len - 1 - i };
        let v = get_element(agent, o, k as f64);
        let r = agent.call(
            &f,
            this_arg.clone(),
            &[v.clone(), Value::Number(k as f64), Value::Object(o)],
        )?;
        if crate::abstract_ops::conversion::to_boolean(agent, &r) {
            return Ok(if index { Value::Number(k as f64) } else { v });
        }
    }
    Ok(if index {
        Value::Number(-1.0)
    } else {
        Value::Undefined
    })
}

fn find(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    find_via(agent, a, true, false)
}

fn find_index(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    find_via(agent, a, true, true)
}

fn find_last(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    find_via(agent, a, false, false)
}

fn find_last_index(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    find_via(agent, a, false, true)
}

/// %TypedArray%.prototype.includes (§23.2.3.16).
fn includes(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = validate(agent, &a.this)?;
    if len == 0 {
        return Ok(Value::Boolean(false));
    }
    let n = agent.to_integer_or_infinity(&a.arg(1))?;
    if n == f64::INFINITY {
        return Ok(Value::Boolean(false));
    }
    let lenf = len as f64;
    let mut k = if n >= 0.0 { n } else { (lenf + n).max(0.0) };
    let search = a.arg(0);
    while k < lenf {
        let e = get_element(agent, o, k);
        if agent.same_value_zero(&search, &e) {
            return Ok(Value::Boolean(true));
        }
        k += 1.0;
    }
    Ok(Value::Boolean(false))
}

/// %TypedArray%.prototype.indexOf (§23.2.3.17).
fn index_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = validate(agent, &a.this)?;
    if len == 0 {
        return Ok(Value::Number(-1.0));
    }
    let n = agent.to_integer_or_infinity(&a.arg(1))?;
    if n == f64::INFINITY {
        return Ok(Value::Number(-1.0));
    }
    let lenf = len as f64;
    let mut k = if n >= 0.0 { n } else { (lenf + n).max(0.0) };
    let search = a.arg(0);
    while k < lenf {
        if is_valid_integer_index(agent, o, k) {
            let e = get_element(agent, o, k);
            if agent.is_strictly_equal(&search, &e) {
                return Ok(Value::Number(k));
            }
        }
        k += 1.0;
    }
    Ok(Value::Number(-1.0))
}

/// %TypedArray%.prototype.lastIndexOf (§23.2.3.20).
fn last_index_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = validate(agent, &a.this)?;
    if len == 0 {
        return Ok(Value::Number(-1.0));
    }
    let lenf = len as f64;
    let n = if a.len() > 1 {
        agent.to_integer_or_infinity(&a.arg(1))?
    } else {
        lenf - 1.0
    };
    if n == f64::NEG_INFINITY {
        return Ok(Value::Number(-1.0));
    }
    let mut k = if n >= 0.0 {
        n.min(lenf - 1.0)
    } else {
        lenf + n
    };
    let search = a.arg(0);
    while k >= 0.0 {
        if is_valid_integer_index(agent, o, k) {
            let e = get_element(agent, o, k);
            if agent.is_strictly_equal(&search, &e) {
                return Ok(Value::Number(k));
            }
        }
        k -= 1.0;
    }
    Ok(Value::Number(-1.0))
}

/// %TypedArray%.prototype.join (§23.2.3.18).
fn join(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = validate(agent, &a.this)?;
    let sep: Vec<u16> = if a.arg(0).is_undefined() {
        vec![u16::from(b',')]
    } else {
        agent.to_string(&a.arg(0))?.units().to_vec()
    };
    let mut r: Vec<u16> = Vec::new();
    for k in 0..len {
        if k > 0 {
            r.extend_from_slice(&sep);
        }
        let e = get_element(agent, o, k as f64);
        if !e.is_undefined() {
            r.extend_from_slice(agent.to_string(&e)?.units());
        }
    }
    Ok(Value::String(JsString::from_units(r)))
}

/// %TypedArray%.prototype.map (§23.2.3.22).
fn map(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = validate(agent, &a.this)?;
    let f = callback(agent, &a.arg(0))?;
    let this_arg = a.arg(1);
    let new = species_create(agent, o, &[Value::Number(len as f64)])?;
    for k in 0..len {
        let v = get_element(agent, o, k as f64);
        let m = agent.call(
            &f,
            this_arg.clone(),
            &[v, Value::Number(k as f64), Value::Object(o)],
        )?;
        set_element(agent, new, k as f64, &m)?;
    }
    Ok(Value::Object(new))
}

/// reduce y reduceRight (§23.2.3.23–24).
fn reduce_impl(agent: &mut Agent, a: &CallArgs<'_>, right: bool) -> JsResult<Value> {
    let (o, len) = validate(agent, &a.this)?;
    let f = callback(agent, &a.arg(0))?;
    if len == 0 && a.len() < 2 {
        return agent.type_error("Reduce of empty array with no initial value");
    }
    let order: Vec<usize> = if right {
        (0..len).rev().collect()
    } else {
        (0..len).collect()
    };
    let mut iter = order.into_iter();
    let mut acc = if a.len() >= 2 {
        a.arg(1)
    } else {
        let k = iter.next().expect("primer elemento");
        get_element(agent, o, k as f64)
    };
    for k in iter {
        let v = get_element(agent, o, k as f64);
        acc = agent.call(
            &f,
            Value::Undefined,
            &[acc, v, Value::Number(k as f64), Value::Object(o)],
        )?;
    }
    Ok(acc)
}

fn reduce(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    reduce_impl(agent, a, false)
}

fn reduce_right(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    reduce_impl(agent, a, true)
}

/// %TypedArray%.prototype.reverse (§23.2.3.25).
fn reverse(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = validate(agent, &a.this)?;
    let middle = len / 2;
    for lower in 0..middle {
        let upper = len - lower - 1;
        let lv = get_element(agent, o, lower as f64);
        let uv = get_element(agent, o, upper as f64);
        write_element(agent, o, lower as f64, &uv);
        write_element(agent, o, upper as f64, &lv);
    }
    Ok(Value::Object(o))
}

/// %TypedArray%.prototype.set (§23.2.3.26).
fn set_method(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let target = require_ta(agent, &a.this)?;
    let offset = agent.to_integer_or_infinity(&a.arg(1))?;
    if offset < 0.0 {
        return agent.range_error("offset is out of bounds");
    }
    let source = a.arg(0);
    match &source {
        Value::Object(s) if matches!(agent.heap[*s].kind, ObjectKind::TypedArray(_)) => {
            set_from_typed_array(agent, target, offset, *s)?;
        }
        _ => set_from_array_like(agent, target, offset, &source)?,
    }
    Ok(Value::Undefined)
}

/// SetTypedArrayFromTypedArray (§23.2.3.26.1).
fn set_from_typed_array(
    agent: &mut Agent,
    target: Gc<Object>,
    offset: f64,
    source: Gc<Object>,
) -> JsResult<()> {
    let Some(target_len) = ta_length(agent, target) else {
        return agent.type_error("Target TypedArray is detached or out of bounds");
    };
    let Some(src_len) = ta_length(agent, source) else {
        return agent.type_error("Source TypedArray is detached or out of bounds");
    };
    if offset == f64::INFINITY || src_len as f64 + offset > target_len as f64 {
        return agent.range_error("offset is out of bounds");
    }
    let tk = kind_of(agent, target);
    let sd = ta_data(agent, source).expect("ta");
    let (src_buffer, sk, src_offset) = (sd.buffer, sd.kind, sd.byte_offset);
    if tk.is_bigint() != sk.is_bigint() {
        return agent.type_error("Content types of source and target TypedArrays differ");
    }
    let td = ta_data(agent, target).expect("ta");
    let (target_buffer, target_offset) = (td.buffer, td.byte_offset);
    let mut target_index = offset as usize * tk.size() + target_offset;
    let limit = target_index + tk.size() * src_len;
    if sk == tk {
        // Copia byte a byte (con un buffer intermedio si es el mismo).
        let bytes: Vec<u64> = (0..src_len * sk.size())
            .map(|i| ab::read_raw(agent, src_buffer, src_offset + i, 1, true))
            .collect();
        for b in bytes {
            if target_index >= limit {
                break;
            }
            ab::write_raw(agent, target_buffer, target_index, 1, b, true);
            target_index += 1;
        }
    } else {
        let values: Vec<Value> = (0..src_len)
            .map(|i| get_value_from_buffer(agent, src_buffer, src_offset + i * sk.size(), sk, true))
            .collect();
        for v in values {
            set_value_in_buffer(agent, target_buffer, target_index, tk, &v, true);
            target_index += tk.size();
        }
    }
    Ok(())
}

/// SetTypedArrayFromArrayLike (§23.2.3.26.2).
fn set_from_array_like(
    agent: &mut Agent,
    target: Gc<Object>,
    offset: f64,
    source: &Value,
) -> JsResult<()> {
    let Some(target_len) = ta_length(agent, target) else {
        return agent.type_error("Target TypedArray is detached or out of bounds");
    };
    let src = agent.to_object(source)?;
    let src_len = agent.length_of_array_like(src)?;
    if offset == f64::INFINITY || src_len + offset > target_len as f64 {
        return agent.range_error("offset is out of bounds");
    }
    for k in 0..src_len as u64 {
        let v = agent.get(src, &PropertyKey::from_number(k as f64), Value::Object(src))?;
        set_element(agent, target, offset + k as f64, &v)?;
    }
    Ok(())
}

/// %TypedArray%.prototype.slice (§23.2.3.27).
fn slice(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = validate(agent, &a.this)?;
    let lenf = len as f64;
    let start = relative_index(agent, &a.arg(0), lenf, 0.0)?;
    let mut end = relative_index(agent, &a.arg(1), lenf, lenf)?;
    let mut count = (end - start).max(0.0);
    let new = species_create(agent, o, &[Value::Number(count)])?;
    if count > 0.0 {
        let Some(len) = ta_length(agent, o) else {
            return agent.type_error("TypedArray is detached or out of bounds");
        };
        end = end.min(len as f64);
        count = (end - start).max(0.0);
        let sk = kind_of(agent, o);
        let tk = kind_of(agent, new);
        if sk == tk {
            let sd = ta_data(agent, o).expect("ta");
            let (src_buffer, src_offset) = (sd.buffer, sd.byte_offset);
            let td = ta_data(agent, new).expect("ta");
            let (target_buffer, target_offset) = (td.buffer, td.byte_offset);
            let size = sk.size();
            let mut src_index = start as usize * size + src_offset;
            let mut target_index = target_offset;
            let end_index = target_index + count as usize * size;
            let src_limit = byte_length(agent, src_buffer);
            let target_limit = byte_length(agent, target_buffer);
            while target_index < end_index {
                if src_index >= src_limit || target_index >= target_limit {
                    break;
                }
                let b = ab::read_raw(agent, src_buffer, src_index, 1, true);
                ab::write_raw(agent, target_buffer, target_index, 1, b, true);
                src_index += 1;
                target_index += 1;
            }
        } else {
            let mut n = 0.0;
            let mut k = start;
            while k < end {
                let v = get_element(agent, o, k);
                set_element(agent, new, n, &v)?;
                k += 1.0;
                n += 1.0;
            }
        }
    }
    Ok(Value::Object(new))
}

/// CompareTypedArrayElements (§23.2.4.7).
fn compare_elements(agent: &mut Agent, x: &Value, y: &Value, cmp: &Value) -> JsResult<Ordering> {
    if !cmp.is_undefined() {
        let r = agent.call(cmp, Value::Undefined, &[x.clone(), y.clone()])?;
        let v = agent.to_number(&r)?;
        return Ok(if v < 0.0 {
            Ordering::Less
        } else if v > 0.0 {
            Ordering::Greater
        } else {
            Ordering::Equal
        });
    }
    Ok(match (x, y) {
        (Value::Number(a), Value::Number(b)) => match (a.is_nan(), b.is_nan()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            _ => {
                if a < b {
                    Ordering::Less
                } else if a > b {
                    Ordering::Greater
                } else if a.is_sign_negative() && !b.is_sign_negative() {
                    Ordering::Less
                } else if !a.is_sign_negative() && b.is_sign_negative() {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            }
        },
        (Value::BigInt(a), Value::BigInt(b)) => agent.heap[*a].compare(&agent.heap[*b]),
        _ => Ordering::Equal,
    })
}

/// Ordenamiento estable (merge sort) con un comparador que puede lanzar (SortIndexedProperties
/// §23.1.3.30.1, paso 4).
fn merge_sort(agent: &mut Agent, items: Vec<Value>, cmp: &Value) -> JsResult<Vec<Value>> {
    if items.len() <= 1 {
        return Ok(items);
    }
    let mut v = items;
    let right = v.split_off(v.len() / 2);
    let left = merge_sort(agent, v, cmp)?;
    let right = merge_sort(agent, right, cmp)?;
    let mut out = Vec::with_capacity(left.len() + right.len());
    let (mut i, mut j) = (0, 0);
    while i < left.len() && j < right.len() {
        if compare_elements(agent, &right[j], &left[i], cmp)? == Ordering::Less {
            out.push(right[j].clone());
            j += 1;
        } else {
            out.push(left[i].clone());
            i += 1;
        }
    }
    out.extend_from_slice(&left[i..]);
    out.extend_from_slice(&right[j..]);
    Ok(out)
}

/// %TypedArray%.prototype.sort (§23.2.3.29).
fn sort(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let cmp = a.arg(0);
    if !cmp.is_undefined() && !agent.is_callable(&cmp) {
        return agent.type_error("The comparison function must be either a function or undefined");
    }
    let (o, len) = validate(agent, &a.this)?;
    let items: Vec<Value> = (0..len).map(|k| get_element(agent, o, k as f64)).collect();
    let sorted = merge_sort(agent, items, &cmp)?;
    for (j, v) in sorted.iter().enumerate() {
        set_element(agent, o, j as f64, v)?;
    }
    Ok(Value::Object(o))
}

/// %TypedArray%.prototype.subarray (§23.2.3.30).
fn subarray(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = require_ta(agent, &a.this)?;
    let d = ta_data(agent, o).expect("ta");
    let (buffer, kind, src_offset, auto) =
        (d.buffer, d.kind, d.byte_offset, d.array_length.is_none());
    let src_len = ta_length(agent, o).unwrap_or(0) as f64;
    let start = relative_index(agent, &a.arg(0), src_len, 0.0)?;
    let begin = src_offset as f64 + start * kind.size() as f64;
    let args = if auto && a.arg(1).is_undefined() {
        vec![Value::Object(buffer), Value::Number(begin)]
    } else {
        let end = relative_index(agent, &a.arg(1), src_len, src_len)?;
        let new_len = (end - start).max(0.0);
        vec![
            Value::Object(buffer),
            Value::Number(begin),
            Value::Number(new_len),
        ]
    };
    Ok(Value::Object(species_create(agent, o, &args)?))
}

/// %TypedArray%.prototype.toLocaleString (§23.2.3.31 con Array.prototype.toLocaleString).
fn to_locale_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = validate(agent, &a.this)?;
    let mut r: Vec<u16> = Vec::new();
    let key = PropertyKey::from("toLocaleString");
    for k in 0..len {
        if k > 0 {
            r.push(u16::from(b','));
        }
        let e = get_element(agent, o, k as f64);
        if !e.is_nullish() {
            let s = agent.invoke(&e, &key, &[])?;
            r.extend_from_slice(agent.to_string(&s)?.units());
        }
    }
    Ok(Value::String(JsString::from_units(r)))
}

/// %TypedArray%.prototype.toReversed (§23.2.3.33).
fn to_reversed(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = validate(agent, &a.this)?;
    let new = create_same_type(agent, o, len)?;
    for k in 0..len {
        let v = get_element(agent, o, (len - k - 1) as f64);
        set_element(agent, new, k as f64, &v)?;
    }
    Ok(Value::Object(new))
}

/// %TypedArray%.prototype.toSorted (§23.2.3.34).
fn to_sorted(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let cmp = a.arg(0);
    if !cmp.is_undefined() && !agent.is_callable(&cmp) {
        return agent.type_error("The comparison function must be either a function or undefined");
    }
    let (o, len) = validate(agent, &a.this)?;
    let new = create_same_type(agent, o, len)?;
    let items: Vec<Value> = (0..len).map(|k| get_element(agent, o, k as f64)).collect();
    let sorted = merge_sort(agent, items, &cmp)?;
    for (j, v) in sorted.iter().enumerate() {
        set_element(agent, new, j as f64, v)?;
    }
    Ok(Value::Object(new))
}

/// %TypedArray%.prototype.with (§23.2.3.36).
fn with(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = validate(agent, &a.this)?;
    let rel = agent.to_integer_or_infinity(&a.arg(0))?;
    let actual = if rel >= 0.0 { rel } else { len as f64 + rel };
    let kind = kind_of(agent, o);
    let value = to_element_value(agent, kind, &a.arg(1))?;
    if !is_valid_integer_index(agent, o, actual) {
        return agent.range_error("Invalid typed array index");
    }
    let new = create_same_type(agent, o, len)?;
    for k in 0..len {
        let v = if k as f64 == actual {
            value.clone()
        } else {
            get_element(agent, o, k as f64)
        };
        set_element(agent, new, k as f64, &v)?;
    }
    Ok(Value::Object(new))
}
