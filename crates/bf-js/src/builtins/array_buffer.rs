//! ArrayBuffer (§25.1) y SharedArrayBuffer (§25.2), con los buffers redimensionables y
//! crecientes de ES2024 y `transfer` de ES2025; los Data Blocks y las conversiones entre
//! valores y bytes (GetValueFromBuffer, SetValueInBuffer, NumericToRawBytes y
//! RawBytesToNumeric, §25.1.3), incluida la de binary16 (Float16Array, ES2025).
//!
//! Sin hilos, un Shared Data Block es un bloque más del agente: las operaciones con orden
//! `seq-cst` y `unordered` son la misma lectura o escritura.

use crate::bigint::BigInt;
use crate::builtins::{FnEntry, accessor, constructor, methods, to_string_tag};
use crate::heap::Gc;
use crate::object::{Object, ObjectKind};
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

/// El tamaño máximo de un Data Block (CreateByteDataBlock lanza RangeError si no se puede
/// reservar; aquí, además, por encima de este límite).
pub const MAX_BYTE_LENGTH: u64 = 1 << 30;

/// Los internal slots de un ArrayBuffer o SharedArrayBuffer.
#[derive(Debug, Default)]
pub struct ArrayBufferData {
    /// `[[ArrayBufferData]]`; `None` si está detached.
    pub data: Option<Vec<u8>>,
    /// `[[ArrayBufferMaxByteLength]]` (redimensionable o creciente).
    pub max_byte_length: Option<usize>,
    /// ¿Es un SharedArrayBuffer?
    pub shared: bool,
}

/// Los tipos de elemento (Table 71, §23.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementType {
    Int8,
    Uint8,
    Uint8Clamped,
    Int16,
    Uint16,
    Int32,
    Uint32,
    BigInt64,
    BigUint64,
    Float16,
    Float32,
    Float64,
}

impl ElementType {
    pub fn size(self) -> usize {
        match self {
            ElementType::Int8 | ElementType::Uint8 | ElementType::Uint8Clamped => 1,
            ElementType::Int16 | ElementType::Uint16 | ElementType::Float16 => 2,
            ElementType::Int32 | ElementType::Uint32 | ElementType::Float32 => 4,
            ElementType::BigInt64 | ElementType::BigUint64 | ElementType::Float64 => 8,
        }
    }

    /// IsBigIntElementType.
    pub fn is_bigint(self) -> bool {
        matches!(self, ElementType::BigInt64 | ElementType::BigUint64)
    }
}

// --- Acceso a los slots ---

pub fn buffer_data(agent: &Agent, b: Gc<Object>) -> Option<&ArrayBufferData> {
    match &agent.heap[b].kind {
        ObjectKind::ArrayBuffer(d) => Some(d),
        _ => None,
    }
}

fn buffer_data_mut(agent: &mut Agent, b: Gc<Object>) -> Option<&mut ArrayBufferData> {
    match &mut agent.heap[b].kind {
        ObjectKind::ArrayBuffer(d) => Some(d),
        _ => None,
    }
}

/// IsDetachedBuffer (§25.1.3.2).
pub fn is_detached(agent: &Agent, b: Gc<Object>) -> bool {
    buffer_data(agent, b).is_none_or(|d| d.data.is_none())
}

/// IsSharedArrayBuffer (§25.2.2.2).
pub fn is_shared(agent: &Agent, b: Gc<Object>) -> bool {
    buffer_data(agent, b).is_some_and(|d| d.shared)
}

/// IsFixedLengthArrayBuffer (§25.1.3.9).
pub fn is_fixed_length(agent: &Agent, b: Gc<Object>) -> bool {
    buffer_data(agent, b).is_none_or(|d| d.max_byte_length.is_none())
}

/// ArrayBufferByteLength (§25.1.3.8): 0 si está detached.
pub fn byte_length(agent: &Agent, b: Gc<Object>) -> usize {
    buffer_data(agent, b)
        .and_then(|d| d.data.as_ref())
        .map_or(0, Vec::len)
}

/// CreateByteDataBlock (§6.2.9.1).
fn create_byte_data_block(agent: &mut Agent, size: u64) -> JsResult<Vec<u8>> {
    if size > MAX_BYTE_LENGTH {
        return agent.range_error("Array buffer allocation failed");
    }
    let mut v = Vec::new();
    if v.try_reserve_exact(size as usize).is_err() {
        return agent.range_error("Array buffer allocation failed");
    }
    v.resize(size as usize, 0);
    Ok(v)
}

/// AllocateArrayBuffer (§25.1.3.1); `max` es `[[ArrayBufferMaxByteLength]]` si es
/// redimensionable.
pub fn allocate_array_buffer(
    agent: &mut Agent,
    ctor: Gc<Object>,
    byte_length: u64,
    max: Option<u64>,
) -> JsResult<Gc<Object>> {
    if let Some(m) = max
        && byte_length > m
    {
        return agent.range_error("byteLength exceeds maxByteLength");
    }
    let proto = agent.get_prototype_from_constructor(ctor, |i| i.array_buffer_prototype)?;
    let block = create_byte_data_block(agent, byte_length)?;
    if let Some(m) = max
        && m > MAX_BYTE_LENGTH
    {
        return agent.range_error("Array buffer allocation failed");
    }
    Ok(agent.alloc(Object::new(
        Some(proto),
        ObjectKind::ArrayBuffer(Box::new(ArrayBufferData {
            data: Some(block),
            max_byte_length: max.map(|m| m as usize),
            shared: false,
        })),
    )))
}

/// AllocateArrayBuffer(%ArrayBuffer%, byteLength).
pub fn allocate_default(agent: &mut Agent, byte_length: u64) -> JsResult<Gc<Object>> {
    let c = agent.intrinsics().array_buffer_ctor;
    allocate_array_buffer(agent, c, byte_length, None)
}

/// CloneArrayBuffer (§25.1.3.6).
pub fn clone_array_buffer(
    agent: &mut Agent,
    src: Gc<Object>,
    offset: usize,
    len: usize,
) -> JsResult<Gc<Object>> {
    let target = allocate_default(agent, len as u64)?;
    let bytes: Vec<u8> = buffer_data(agent, src)
        .and_then(|d| d.data.as_ref())
        .map(|d| d[offset..offset + len].to_vec())
        .unwrap_or_default();
    if let Some(d) = buffer_data_mut(agent, target).and_then(|d| d.data.as_mut()) {
        d[..bytes.len()].copy_from_slice(&bytes);
    }
    Ok(target)
}

/// DetachArrayBuffer (§25.1.3.5) sin clave.
pub fn detach_array_buffer(agent: &mut Agent, b: &Value) -> JsResult<()> {
    match b {
        Value::Object(o) => match buffer_data_mut(agent, *o) {
            Some(d) if !d.shared => {
                d.data = None;
                Ok(())
            }
            _ => agent.type_error("Cannot detach a SharedArrayBuffer or a non-ArrayBuffer"),
        },
        _ => agent.type_error("Cannot detach a non-ArrayBuffer"),
    }
}

/// Copia `len` bytes dentro del mismo buffer o entre buffers (CopyDataBlockBytes).
pub fn copy_bytes(
    agent: &mut Agent,
    to: Gc<Object>,
    to_index: usize,
    from: Gc<Object>,
    from_index: usize,
    len: usize,
) {
    let bytes: Vec<u8> = match buffer_data(agent, from).and_then(|d| d.data.as_ref()) {
        Some(d) => d[from_index..from_index + len].to_vec(),
        None => return,
    };
    if let Some(d) = buffer_data_mut(agent, to).and_then(|d| d.data.as_mut()) {
        d[to_index..to_index + len].copy_from_slice(&bytes);
    }
}

// --- Conversiones (§25.1.3.15–17) ---

/// binary16 → Number.
pub fn f16_to_f64(h: u16) -> f64 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = i32::from((h >> 10) & 0x1f);
    let f = f64::from(h & 0x3ff);
    match e {
        0 => sign * f * 2f64.powi(-24),
        31 => {
            if f == 0.0 {
                sign * f64::INFINITY
            } else {
                f64::NAN
            }
        }
        _ => sign * (1.0 + f / 1024.0) * 2f64.powi(e - 15),
    }
}

/// Number → binary16 con roundTiesToEven (§6.1.6.1 y NumericToRawBytes paso 1.a), sin el
/// doble redondeo de pasar por binary32.
pub fn f64_to_f16(x: f64) -> u16 {
    let bits = x.to_bits();
    let sign = ((bits >> 48) & 0x8000) as u16;
    if x.is_nan() {
        return 0x7e00;
    }
    if x.is_infinite() {
        return sign | 0x7c00;
    }
    let e = ((bits >> 52) & 0x7ff) as i32 - 1023;
    if x == 0.0 || e < -25 {
        return sign;
    }
    let mant = (bits & 0x000f_ffff_ffff_ffff) | (1u64 << 52);
    let shift: u32 = if e >= -14 { 42 } else { (28 - e) as u32 };
    let mut q = mant >> shift;
    let rem = mant & ((1u64 << shift) - 1);
    let half = 1u64 << (shift - 1);
    if rem > half || (rem == half && q & 1 == 1) {
        q += 1;
    }
    if e >= -14 {
        let mut he = e + 15;
        if q == 2048 {
            q = 1024;
            he += 1;
        }
        if he >= 31 {
            return sign | 0x7c00;
        }
        sign | ((he as u16) << 10) | ((q - 1024) as u16)
    } else {
        sign | q as u16
    }
}

/// Los 64 bits bajos de un BigInt en complemento a dos (ToBigInt64/ToBigUint64, §7.1.15–16).
pub fn bigint_to_u64(b: &BigInt) -> u64 {
    let lo = u64::from(b.mag.first().copied().unwrap_or(0))
        | u64::from(b.mag.get(1).copied().unwrap_or(0)) << 32;
    if b.negative { lo.wrapping_neg() } else { lo }
}

/// Un BigInt a partir de un entero de 64 bits con o sin signo.
pub fn bigint_from_i128(v: i128) -> BigInt {
    let negative = v < 0;
    let m = v.unsigned_abs();
    let mut mag = vec![
        m as u32,
        (m >> 32) as u32,
        (m >> 64) as u32,
        (m >> 96) as u32,
    ];
    while mag.last() == Some(&0) {
        mag.pop();
    }
    BigInt {
        negative: negative && !mag.is_empty(),
        mag,
    }
}

/// ToBigInt (§7.1.13).
pub fn to_big_int(agent: &mut Agent, v: &Value) -> JsResult<Value> {
    let prim = agent.to_primitive(v, crate::abstract_ops::conversion::Hint::Number)?;
    match &prim {
        Value::BigInt(_) => Ok(prim),
        Value::Boolean(b) => {
            let n = bigint_from_i128(i128::from(*b));
            Ok(Value::BigInt(agent.heap.alloc(n)))
        }
        Value::String(s) => match BigInt::from_string(&s.to_string_lossy()) {
            Some(b) => Ok(Value::BigInt(agent.heap.alloc(b))),
            None => agent.syntax_error(&format!("Cannot convert {} to a BigInt", s)),
        },
        Value::Undefined | Value::Null => {
            agent.type_error(&format!("Cannot convert {} to a BigInt", prim.type_name()))
        }
        Value::Number(_) => agent.type_error("Cannot convert a Number to a BigInt"),
        Value::Symbol(_) => agent.type_error("Cannot convert a Symbol to a BigInt"),
        Value::Object(_) => unreachable!("ToPrimitive devolvió un objeto"),
    }
}

/// Convierte para escribir en un elemento: ToBigInt o ToNumber según el tipo.
pub fn to_element_value(agent: &mut Agent, ty: ElementType, v: &Value) -> JsResult<Value> {
    if ty.is_bigint() {
        to_big_int(agent, v)
    } else {
        Ok(Value::Number(agent.to_number(v)?))
    }
}

/// NumericToRawBytes (§25.1.3.16) como entero de 64 bits en orden little-endian.
pub fn numeric_to_raw(agent: &Agent, ty: ElementType, v: &Value) -> u64 {
    use crate::abstract_ops::conversion as c;
    let n = match v {
        Value::Number(n) => *n,
        Value::BigInt(b) => return bigint_to_u64(&agent.heap[*b]),
        _ => 0.0,
    };
    match ty {
        ElementType::Int8 => u64::from(c::to_int8(n) as u8),
        ElementType::Uint8 => u64::from(c::to_uint8(n)),
        ElementType::Uint8Clamped => u64::from(c::to_uint8_clamp(n)),
        ElementType::Int16 => u64::from(c::to_int16(n) as u16),
        ElementType::Uint16 => u64::from(c::to_uint16(n)),
        ElementType::Int32 => u64::from(c::to_int32(n) as u32),
        ElementType::Uint32 => u64::from(c::to_uint32(n)),
        ElementType::Float16 => u64::from(f64_to_f16(n)),
        ElementType::Float32 => u64::from((n as f32).to_bits()),
        ElementType::Float64 => n.to_bits(),
        ElementType::BigInt64 | ElementType::BigUint64 => 0,
    }
}

/// RawBytesToNumeric (§25.1.3.15).
pub fn raw_to_numeric(agent: &mut Agent, ty: ElementType, raw: u64) -> Value {
    let n = match ty {
        ElementType::Int8 => f64::from(raw as u8 as i8),
        ElementType::Uint8 | ElementType::Uint8Clamped => f64::from(raw as u8),
        ElementType::Int16 => f64::from(raw as u16 as i16),
        ElementType::Uint16 => f64::from(raw as u16),
        ElementType::Int32 => f64::from(raw as u32 as i32),
        ElementType::Uint32 => f64::from(raw as u32),
        ElementType::Float16 => f16_to_f64(raw as u16),
        ElementType::Float32 => f64::from(f32::from_bits(raw as u32)),
        ElementType::Float64 => f64::from_bits(raw),
        ElementType::BigInt64 => {
            let b = bigint_from_i128(i128::from(raw as i64));
            return Value::BigInt(agent.heap.alloc(b));
        }
        ElementType::BigUint64 => {
            let b = bigint_from_i128(i128::from(raw));
            return Value::BigInt(agent.heap.alloc(b));
        }
    };
    Value::Number(n)
}

/// Lee `size` bytes en `index` como entero little-endian (o big-endian si `!little`).
pub fn read_raw(agent: &Agent, b: Gc<Object>, index: usize, size: usize, little: bool) -> u64 {
    let Some(d) = buffer_data(agent, b).and_then(|d| d.data.as_ref()) else {
        return 0;
    };
    let mut raw = 0u64;
    for i in 0..size {
        let byte = u64::from(d.get(index + i).copied().unwrap_or(0));
        if little {
            raw |= byte << (8 * i);
        } else {
            raw = raw << 8 | byte;
        }
    }
    raw
}

/// Escribe `size` bytes de `raw` en `index`.
pub fn write_raw(
    agent: &mut Agent,
    b: Gc<Object>,
    index: usize,
    size: usize,
    raw: u64,
    little: bool,
) {
    let Some(d) = buffer_data_mut(agent, b).and_then(|d| d.data.as_mut()) else {
        return;
    };
    for i in 0..size {
        let shift = if little { 8 * i } else { 8 * (size - 1 - i) };
        if let Some(slot) = d.get_mut(index + i) {
            *slot = (raw >> shift) as u8;
        }
    }
}

/// GetValueFromBuffer (§25.1.3.15).
pub fn get_value_from_buffer(
    agent: &mut Agent,
    b: Gc<Object>,
    index: usize,
    ty: ElementType,
    little: bool,
) -> Value {
    let raw = read_raw(agent, b, index, ty.size(), little);
    raw_to_numeric(agent, ty, raw)
}

/// SetValueInBuffer (§25.1.3.17): `v` ya es un Number o un BigInt según el tipo.
pub fn set_value_in_buffer(
    agent: &mut Agent,
    b: Gc<Object>,
    index: usize,
    ty: ElementType,
    v: &Value,
    little: bool,
) {
    let raw = numeric_to_raw(agent, ty, v);
    write_raw(agent, b, index, ty.size(), raw, little);
}

// --- Intrínsecos ---

pub fn init(agent: &mut Agent, realm: RealmId) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    let species = PropertyKey::Symbol(agent.well_known.species);
    // ArrayBuffer (§25.1.4–6).
    let abp = agent.new_object(Some(op));
    const AB_PROTO: &[FnEntry] = &[
        ("resize", 1, ab_resize),
        ("slice", 2, ab_slice),
        ("transfer", 0, ab_transfer),
        ("transferToFixedLength", 0, ab_transfer_fixed),
    ];
    methods(agent, abp, AB_PROTO);
    let getters: &[(&str, crate::vm::function::NativeFn)] = &[
        ("byteLength", ab_byte_length),
        ("detached", ab_detached),
        ("maxByteLength", ab_max_byte_length),
        ("resizable", ab_resizable),
    ];
    for &(name, f) in getters {
        accessor(agent, abp, PropertyKey::from(name), name, Some(f), None);
    }
    to_string_tag(agent, abp, "ArrayBuffer");
    let abc = constructor(agent, "ArrayBuffer", 1, ab_ctor, abp);
    methods(agent, abc, &[("isView", 1, ab_is_view)]);
    accessor(
        agent,
        abc,
        species.clone(),
        "[Symbol.species]",
        Some(species_getter),
        None,
    );
    // SharedArrayBuffer (§25.2.3–5).
    let sabp = agent.new_object(Some(op));
    methods(
        agent,
        sabp,
        &[("grow", 1, sab_grow), ("slice", 2, sab_slice)],
    );
    let getters: &[(&str, crate::vm::function::NativeFn)] = &[
        ("byteLength", sab_byte_length),
        ("growable", sab_growable),
        ("maxByteLength", sab_max_byte_length),
    ];
    for &(name, f) in getters {
        accessor(agent, sabp, PropertyKey::from(name), name, Some(f), None);
    }
    to_string_tag(agent, sabp, "SharedArrayBuffer");
    let sabc = constructor(agent, "SharedArrayBuffer", 1, sab_ctor, sabp);
    accessor(
        agent,
        sabc,
        species,
        "[Symbol.species]",
        Some(species_getter),
        None,
    );
    let ints = &mut agent.realms[realm].intrinsics;
    ints.array_buffer_ctor = abc;
    ints.array_buffer_prototype = abp;
    ints.shared_array_buffer_ctor = sabc;
    ints.shared_array_buffer_prototype = sabp;
}

fn species_getter(_: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(a.this.clone())
}

/// GetArrayBufferMaxByteLengthOption (§25.1.3.7).
fn max_byte_length_option(agent: &mut Agent, options: &Value) -> JsResult<Option<u64>> {
    let Value::Object(o) = options else {
        return Ok(None);
    };
    let m = agent.get(*o, &PropertyKey::from("maxByteLength"), options.clone())?;
    if m.is_undefined() {
        return Ok(None);
    }
    Ok(Some(agent.to_index(&m)?))
}

/// ArrayBuffer ( length [ , options ] ) (§25.1.4.1).
fn ab_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor ArrayBuffer requires 'new'");
    };
    let len = agent.to_index(&a.arg(0))?;
    let max = max_byte_length_option(agent, &a.arg(1))?;
    Ok(Value::Object(allocate_array_buffer(agent, nt, len, max)?))
}

/// ArrayBuffer.isView (§25.1.5.1).
fn ab_is_view(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(matches!(
        a.arg(0),
        Value::Object(o) if matches!(
            agent.heap[o].kind,
            ObjectKind::TypedArray(_) | ObjectKind::DataView(_)
        )
    )))
}

/// RequireInternalSlot(O, [[ArrayBufferData]]) + IsSharedArrayBuffer(O) = `shared`.
fn this_buffer(agent: &mut Agent, this: &Value, shared: bool, name: &str) -> JsResult<Gc<Object>> {
    if let Value::Object(o) = this
        && let Some(d) = buffer_data(agent, *o)
        && d.shared == shared
    {
        return Ok(*o);
    }
    let class = if shared {
        "SharedArrayBuffer"
    } else {
        "ArrayBuffer"
    };
    agent.type_error(&format!(
        "Method {class}.prototype.{name} called on incompatible receiver"
    ))
}

fn ab_byte_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let b = this_buffer(agent, &a.this, false, "byteLength")?;
    Ok(Value::Number(byte_length(agent, b) as f64))
}

fn ab_detached(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let b = this_buffer(agent, &a.this, false, "detached")?;
    Ok(Value::Boolean(is_detached(agent, b)))
}

fn ab_max_byte_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let b = this_buffer(agent, &a.this, false, "maxByteLength")?;
    if is_detached(agent, b) {
        return Ok(Value::Number(0.0));
    }
    let d = buffer_data(agent, b).expect("buffer");
    let n = d.max_byte_length.unwrap_or_else(|| byte_length(agent, b));
    Ok(Value::Number(n as f64))
}

fn ab_resizable(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let b = this_buffer(agent, &a.this, false, "resizable")?;
    Ok(Value::Boolean(!is_fixed_length(agent, b)))
}

/// ArrayBuffer.prototype.resize (§25.1.6.6).
fn ab_resize(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let b = this_buffer(agent, &a.this, false, "resize")?;
    let Some(max) = buffer_data(agent, b).and_then(|d| d.max_byte_length) else {
        return agent
            .type_error("Method ArrayBuffer.prototype.resize called on a non-resizable buffer");
    };
    let new_len = agent.to_index(&a.arg(0))?;
    if is_detached(agent, b) {
        return agent.type_error("Cannot resize a detached ArrayBuffer");
    }
    if new_len > max as u64 {
        return agent.range_error("ArrayBuffer.prototype.resize: invalid length");
    }
    if let Some(d) = buffer_data_mut(agent, b).and_then(|d| d.data.as_mut()) {
        d.resize(new_len as usize, 0);
    }
    Ok(Value::Undefined)
}

/// Normaliza un índice relativo (pasos de `slice`).
pub fn relative_index(agent: &mut Agent, v: &Value, len: f64, default: f64) -> JsResult<f64> {
    if v.is_undefined() {
        return Ok(default);
    }
    let r = agent.to_integer_or_infinity(v)?;
    Ok(if r < 0.0 {
        (len + r).max(0.0)
    } else {
        r.min(len)
    })
}

/// ArrayBuffer.prototype.slice (§25.1.6.7) y SharedArrayBuffer.prototype.slice (§25.2.5.6).
fn buffer_slice(agent: &mut Agent, a: &CallArgs<'_>, shared: bool) -> JsResult<Value> {
    let o = this_buffer(agent, &a.this, shared, "slice")?;
    if is_detached(agent, o) {
        return agent.type_error("Cannot slice a detached ArrayBuffer");
    }
    let len = byte_length(agent, o) as f64;
    let first = relative_index(agent, &a.arg(0), len, 0.0)?;
    let fin = relative_index(agent, &a.arg(1), len, len)?;
    let new_len = (fin - first).max(0.0);
    let default = if shared {
        agent.intrinsics().shared_array_buffer_ctor
    } else {
        agent.intrinsics().array_buffer_ctor
    };
    let ctor = agent.species_constructor(o, default)?;
    let new = agent.construct(ctor, &[Value::Number(new_len)], None)?;
    let new = match &new {
        Value::Object(n) if buffer_data(agent, *n).is_some() => *n,
        _ => return agent.type_error("Species constructor did not return an ArrayBuffer"),
    };
    if is_shared(agent, new) != shared {
        return agent.type_error("Species constructor returned the wrong kind of buffer");
    }
    if !shared && is_detached(agent, new) {
        return agent.type_error("Species constructor returned a detached ArrayBuffer");
    }
    if new == o {
        return agent.type_error("Species constructor returned the same ArrayBuffer");
    }
    if (byte_length(agent, new) as f64) < new_len {
        return agent.type_error("Species constructor returned a too small ArrayBuffer");
    }
    if is_detached(agent, o) {
        return agent.type_error("ArrayBuffer was detached during slice");
    }
    let current = byte_length(agent, o) as f64;
    if first < current {
        let count = new_len.min(current - first) as usize;
        copy_bytes(agent, new, 0, o, first as usize, count);
    }
    Ok(Value::Object(new))
}

fn ab_slice(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    buffer_slice(agent, a, false)
}

/// ArrayBufferCopyAndDetach (§25.1.3.3).
fn copy_and_detach(agent: &mut Agent, a: &CallArgs<'_>, preserve: bool) -> JsResult<Value> {
    let name = if preserve {
        "transfer"
    } else {
        "transferToFixedLength"
    };
    let b = this_buffer(agent, &a.this, false, name)?;
    let new_len = if a.arg(0).is_undefined() {
        byte_length(agent, b) as u64
    } else {
        agent.to_index(&a.arg(0))?
    };
    if is_detached(agent, b) {
        return agent.type_error("Cannot transfer a detached ArrayBuffer");
    }
    let max = if preserve {
        buffer_data(agent, b).and_then(|d| d.max_byte_length)
    } else {
        None
    };
    if let Some(m) = max
        && new_len > m as u64
    {
        return agent.range_error("transfer: new length exceeds maxByteLength");
    }
    let ctor = agent.intrinsics().array_buffer_ctor;
    let new = allocate_array_buffer(agent, ctor, new_len, max.map(|m| m as u64))?;
    let copy = (new_len as usize).min(byte_length(agent, b));
    copy_bytes(agent, new, 0, b, 0, copy);
    detach_array_buffer(agent, &Value::Object(b))?;
    Ok(Value::Object(new))
}

/// ArrayBuffer.prototype.transfer (§25.1.6.8).
fn ab_transfer(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    copy_and_detach(agent, a, true)
}

/// ArrayBuffer.prototype.transferToFixedLength (§25.1.6.9).
fn ab_transfer_fixed(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    copy_and_detach(agent, a, false)
}

// --- SharedArrayBuffer ---

/// SharedArrayBuffer ( length [ , options ] ) (§25.2.3.1) con AllocateSharedArrayBuffer.
fn sab_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor SharedArrayBuffer requires 'new'");
    };
    let len = agent.to_index(&a.arg(0))?;
    let max = max_byte_length_option(agent, &a.arg(1))?;
    if let Some(m) = max
        && len > m
    {
        return agent.range_error("byteLength exceeds maxByteLength");
    }
    let proto = agent.get_prototype_from_constructor(nt, |i| i.shared_array_buffer_prototype)?;
    let block = create_byte_data_block(agent, len)?;
    if let Some(m) = max
        && m > MAX_BYTE_LENGTH
    {
        return agent.range_error("SharedArrayBuffer allocation failed");
    }
    Ok(Value::Object(agent.alloc(Object::new(
        Some(proto),
        ObjectKind::ArrayBuffer(Box::new(ArrayBufferData {
            data: Some(block),
            max_byte_length: max.map(|m| m as usize),
            shared: true,
        })),
    ))))
}

fn sab_byte_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let b = this_buffer(agent, &a.this, true, "byteLength")?;
    Ok(Value::Number(byte_length(agent, b) as f64))
}

fn sab_growable(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let b = this_buffer(agent, &a.this, true, "growable")?;
    Ok(Value::Boolean(!is_fixed_length(agent, b)))
}

fn sab_max_byte_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let b = this_buffer(agent, &a.this, true, "maxByteLength")?;
    let d = buffer_data(agent, b).expect("buffer");
    let n = d.max_byte_length.unwrap_or_else(|| byte_length(agent, b));
    Ok(Value::Number(n as f64))
}

/// SharedArrayBuffer.prototype.grow (§25.2.5.3).
fn sab_grow(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let b = this_buffer(agent, &a.this, true, "grow")?;
    let Some(max) = buffer_data(agent, b).and_then(|d| d.max_byte_length) else {
        return agent
            .type_error("Method SharedArrayBuffer.prototype.grow called on a non-growable buffer");
    };
    let new_len = agent.to_index(&a.arg(0))?;
    let current = byte_length(agent, b) as u64;
    if new_len < current || new_len > max as u64 {
        return agent.range_error("SharedArrayBuffer.prototype.grow: invalid length");
    }
    if let Some(d) = buffer_data_mut(agent, b).and_then(|d| d.data.as_mut()) {
        d.resize(new_len as usize, 0);
    }
    Ok(Value::Undefined)
}

fn sab_slice(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    buffer_slice(agent, a, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary16() {
        // Exactos.
        for (x, h) in [
            (0.0, 0x0000u16),
            (1.0, 0x3c00),
            (-2.0, 0xc000),
            (65504.0, 0x7bff),
            (2f64.powi(-14), 0x0400),
            (2f64.powi(-24), 0x0001),
        ] {
            assert_eq!(f64_to_f16(x), h, "{x}");
            assert_eq!(f16_to_f64(h), x);
        }
        assert_eq!(f64_to_f16(-0.0), 0x8000);
        assert_eq!(f64_to_f16(f64::INFINITY), 0x7c00);
        assert!(f16_to_f64(f64_to_f16(f64::NAN)).is_nan());
        // roundTiesToEven: 65520 está a mitad entre 65504 (impar) e ∞.
        assert_eq!(f64_to_f16(65520.0), 0x7c00);
        assert_eq!(f64_to_f16(65519.99), 0x7bff);
        // 1 + 2⁻¹¹ es un empate: queda en 1 (par); 1 + 3·2⁻¹¹ sube.
        assert_eq!(f64_to_f16(1.0 + 2f64.powi(-11)), 0x3c00);
        assert_eq!(f64_to_f16(1.0 + 3.0 * 2f64.powi(-11)), 0x3c02);
        // Subnormales: 2⁻²⁵ es un empate con 0; un poco más redondea al mínimo.
        assert_eq!(f64_to_f16(2f64.powi(-25)), 0x0000);
        assert_eq!(f64_to_f16(2f64.powi(-25) * 1.0001), 0x0001);
        // Sin doble redondeo vía binary32.
        let x = 1.0 + 2f64.powi(-11) + 2f64.powi(-40);
        assert_eq!(f64_to_f16(x), 0x3c01);
    }

    #[test]
    fn bigint64() {
        assert_eq!(bigint_to_u64(&bigint_from_i128(-1)), u64::MAX);
        assert_eq!(bigint_to_u64(&bigint_from_i128(1 << 64)), 0);
        assert_eq!(
            bigint_to_u64(&bigint_from_i128(-(1 << 63))) as i64,
            i64::MIN
        );
        assert_eq!(bigint_from_i128(0), BigInt::zero());
    }
}
