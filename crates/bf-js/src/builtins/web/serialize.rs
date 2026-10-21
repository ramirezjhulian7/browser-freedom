//! StructuredSerializeWithTransfer y StructuredDeserializeWithTransfer (HTML §2.7.6–2.7.7)
//! entre agentes (E3): el resultado de serializar es un [`Serialized`], datos de Rust que se
//! pueden mandar a otro hilo (`Send`), y se deserializa en el realm en curso del agente que lo
//! recibe. Lo usan `postMessage` de los workers y de los `MessagePort` entre hilos.
//!
//! El mapa de memoria del spec es una numeración de los objetos en el orden en que se visitan:
//! un objeto ya visto se serializa como [`Record::Ref`] (identidad y ciclos se conservan).
//!
//! Tipos: los primitivos (salvo Symbol), los wrappers Boolean/Number/BigInt/String, Date,
//! RegExp, ArrayBuffer (copia, o transferencia), SharedArrayBuffer (el mismo Shared Data Block,
//! si el embebedor lo permite), las vistas, Map, Set, los Error nativos, Array y los objetos
//! ordinarios. Los objetos host (Blob, File, MessagePort…) los serializa y transfiere el
//! embebedor con un [`HostCloner`]; todo lo demás es un `DataCloneError`.

use std::collections::HashMap;

use crate::bigint::BigInt;
use crate::builtins::array_buffer::{self as ab, ArrayBufferData, ElementType};
use crate::builtins::map_set::{MapData, MapKey};
use crate::builtins::web::throw_dom;
use crate::heap::Gc;
use crate::host::agents::SharedBlock;
use crate::object::{Object, ObjectKind};
use crate::string::JsString;
use crate::value::{PropertyKey, Value};
use crate::vm::{Agent, JsResult};

/// Un objeto host serializado (o transferido) por el embebedor.
pub trait HostRecord: Send {
    /// El objeto nuevo en el agente que lo recibe.
    fn deserialize(self: Box<Self>, agent: &mut Agent) -> JsResult<Value>;
}

/// Lo que el embebedor sabe de sus objetos host.
pub trait HostCloner {
    /// Serializa un objeto host (no transferido); `None` si no es serializable.
    fn serialize(
        &mut self,
        agent: &mut Agent,
        o: Gc<Object>,
    ) -> Option<JsResult<Box<dyn HostRecord>>>;
    /// ¿Es un objeto host transferible (`MessagePort`…)?
    fn is_transferable(&self, agent: &Agent, o: Gc<Object>) -> bool;
    /// Lo transfiere: el registro, y el original queda inutilizado (pasos de transferencia).
    fn transfer(&mut self, agent: &mut Agent, o: Gc<Object>) -> JsResult<Box<dyn HostRecord>>;
}

/// Un [`HostCloner`] sin objetos host.
pub struct NoHostObjects;

impl HostCloner for NoHostObjects {
    fn serialize(&mut self, _: &mut Agent, _: Gc<Object>) -> Option<JsResult<Box<dyn HostRecord>>> {
        None
    }
    fn is_transferable(&self, _: &Agent, _: Gc<Object>) -> bool {
        false
    }
    fn transfer(&mut self, agent: &mut Agent, _: Gc<Object>) -> JsResult<Box<dyn HostRecord>> {
        agent.type_error("not transferable")
    }
}

/// Un valor serializado.
pub enum Record {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    BigInt(BigInt),
    String(Vec<u16>),
    /// Un objeto ya serializado (su número en el mapa de memoria).
    Ref(u32),
    /// El objeto número `.0` del mapa de memoria.
    Object(u32, Box<ObjRecord>),
    /// El objeto transferido número `.0`.
    Transferred(u32),
}

/// El constructor de una vista.
#[derive(Debug, Clone, Copy)]
pub enum ViewKind {
    DataView,
    Typed(ElementType),
}

/// Un objeto serializado.
pub enum ObjRecord {
    Boolean(bool),
    Number(f64),
    BigInt(BigInt),
    String(Vec<u16>),
    Date(f64),
    RegExp(Vec<u16>, Vec<u16>),
    ArrayBuffer {
        bytes: Vec<u8>,
        max: Option<usize>,
    },
    SharedArrayBuffer {
        block: SharedBlock,
        max: Option<usize>,
    },
    View {
        kind: ViewKind,
        buffer: Record,
        offset: usize,
        /// `None`: sigue el largo del buffer.
        length: Option<usize>,
    },
    Map(Vec<(Record, Record)>),
    Set(Vec<Record>),
    Error {
        name: String,
        message: Option<Vec<u16>>,
        cause: Option<Record>,
        stack: Option<Vec<u16>>,
    },
    Array {
        length: u64,
        props: Vec<(Vec<u16>, Record)>,
    },
    Object(Vec<(Vec<u16>, Record)>),
    Host(Box<dyn HostRecord>),
}

/// Un objeto transferido.
pub enum Transferred {
    ArrayBuffer { bytes: Vec<u8>, max: Option<usize> },
    Host(Box<dyn HostRecord>),
}

/// El resultado de StructuredSerializeWithTransfer.
pub struct Serialized {
    pub value: Record,
    pub transfers: Vec<Transferred>,
}

/// Opciones de la serialización.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// ¿Se pueden serializar los SharedArrayBuffer? (El agent cluster aislado.)
    pub allow_shared: bool,
}

fn data_clone_error<T>(agent: &mut Agent, what: &str) -> JsResult<T> {
    throw_dom(
        agent,
        "DataCloneError",
        &format!("{what} could not be cloned."),
    )
}

struct Serializer<'h> {
    memory: HashMap<Gc<Object>, u32>,
    next: u32,
    transfer: HashMap<Gc<Object>, u32>,
    host: &'h mut dyn HostCloner,
    options: Options,
}

/// StructuredSerializeWithTransfer (§2.7.6).
pub fn serialize_with_transfer(
    agent: &mut Agent,
    value: &Value,
    transfer: &[Gc<Object>],
    host: &mut dyn HostCloner,
    options: Options,
) -> JsResult<Serialized> {
    let mut s = Serializer {
        memory: HashMap::new(),
        next: 0,
        transfer: HashMap::new(),
        host,
        options,
    };
    // Pasos 2–4: la lista de transferencia.
    for (i, &t) in transfer.iter().enumerate() {
        let ok = match &agent.heap[t].kind {
            ObjectKind::ArrayBuffer(d) => {
                if d.shared {
                    return data_clone_error(agent, "A SharedArrayBuffer in the transfer list");
                }
                true
            }
            _ => s.host.is_transferable(agent, t),
        };
        if !ok {
            return data_clone_error(agent, "A value in the transfer list");
        }
        if s.transfer.insert(t, i as u32).is_some() {
            return data_clone_error(agent, "A duplicated value in the transfer list");
        }
    }
    let value = s.serialize(agent, value)?;
    // Pasos 6–7: los transferidos (los ArrayBuffer se detachan).
    let mut transfers = Vec::with_capacity(transfer.len());
    for &t in transfer {
        if let ObjectKind::ArrayBuffer(_) = &agent.heap[t].kind {
            if ab::is_detached(agent, t) {
                return data_clone_error(agent, "A detached ArrayBuffer in the transfer list");
            }
            let len = ab::byte_length(agent, t);
            let bytes = ab::read_bytes(agent, t, 0, len);
            let max = ab::buffer_data(agent, t).and_then(|d| d.max_byte_length);
            ab::detach_array_buffer(agent, &Value::Object(t))?;
            transfers.push(Transferred::ArrayBuffer { bytes, max });
        } else {
            transfers.push(Transferred::Host(s.host.transfer(agent, t)?));
        }
    }
    Ok(Serialized { value, transfers })
}

/// Las claves propias enumerables con clave string, en orden.
fn enumerable_string_keys(agent: &mut Agent, o: Gc<Object>) -> JsResult<Vec<PropertyKey>> {
    let mut out = Vec::new();
    for k in agent.own_property_keys(o)? {
        if k.is_symbol() {
            continue;
        }
        if let Some(d) = agent.get_own_property(o, &k)?
            && d.enumerable == Some(true)
        {
            out.push(k);
        }
    }
    Ok(out)
}

fn key_units(k: &PropertyKey) -> Vec<u16> {
    k.to_js_string()
        .map(|s| s.units().to_vec())
        .unwrap_or_default()
}

impl Serializer<'_> {
    fn serialize(&mut self, agent: &mut Agent, v: &Value) -> JsResult<Record> {
        let o = match v {
            Value::Undefined => return Ok(Record::Undefined),
            Value::Null => return Ok(Record::Null),
            Value::Boolean(b) => return Ok(Record::Bool(*b)),
            Value::Number(n) => return Ok(Record::Number(*n)),
            Value::BigInt(b) => return Ok(Record::BigInt(agent.heap[*b].clone())),
            Value::String(s) => return Ok(Record::String(s.units().to_vec())),
            Value::Symbol(_) => return data_clone_error(agent, "A Symbol"),
            Value::Object(o) => *o,
        };
        if let Some(&i) = self.transfer.get(&o) {
            return Ok(Record::Transferred(i));
        }
        if let Some(&i) = self.memory.get(&o) {
            return Ok(Record::Ref(i));
        }
        let id = self.next;
        self.next += 1;
        self.memory.insert(o, id);
        let rec = self.serialize_object(agent, o)?;
        Ok(Record::Object(id, Box::new(rec)))
    }

    fn serialize_object(&mut self, agent: &mut Agent, o: Gc<Object>) -> JsResult<ObjRecord> {
        if matches!(agent.heap[o].kind, ObjectKind::Host(_)) {
            return match self.host.serialize(agent, o) {
                Some(r) => Ok(ObjRecord::Host(r?)),
                None => data_clone_error(agent, "A platform object"),
            };
        }
        if agent.heap[o].is_callable() {
            return data_clone_error(agent, "A function");
        }
        let rec = match &agent.heap[o].kind {
            ObjectKind::Boolean(b) => ObjRecord::Boolean(*b),
            ObjectKind::Number(n) => ObjRecord::Number(*n),
            ObjectKind::String(s) => ObjRecord::String(s.units().to_vec()),
            ObjectKind::BigInt(b) => ObjRecord::BigInt(agent.heap[*b].clone()),
            ObjectKind::Date(t) => ObjRecord::Date(*t),
            ObjectKind::RegExp(r) => {
                ObjRecord::RegExp(r.source.units().to_vec(), r.flags.units().to_vec())
            }
            ObjectKind::ArrayBuffer(d) => {
                if d.shared {
                    if !self.options.allow_shared {
                        return data_clone_error(agent, "A SharedArrayBuffer");
                    }
                    let block = d.block.clone().expect("Shared Data Block");
                    return Ok(ObjRecord::SharedArrayBuffer {
                        block,
                        max: d.max_byte_length,
                    });
                }
                if d.data.is_none() {
                    return data_clone_error(agent, "A detached ArrayBuffer");
                }
                let max = d.max_byte_length;
                let len = ab::byte_length(agent, o);
                return Ok(ObjRecord::ArrayBuffer {
                    bytes: ab::read_bytes(agent, o, 0, len),
                    max,
                });
            }
            ObjectKind::TypedArray(_) | ObjectKind::DataView(_) => {
                return self.serialize_view(agent, o);
            }
            ObjectKind::Map(_) | ObjectKind::Set(_) => return self.serialize_map(agent, o),
            ObjectKind::Error => return self.serialize_error(agent, o),
            ObjectKind::Array(_) => {
                let len = agent.get_str(o, "length")?;
                let length = agent.to_length(&len)? as u64;
                let props = self.properties(agent, o)?;
                return Ok(ObjRecord::Array { length, props });
            }
            ObjectKind::Ordinary | ObjectKind::Arguments(None) => {
                return Ok(ObjRecord::Object(self.properties(agent, o)?));
            }
            _ => return data_clone_error(agent, "An object of this type"),
        };
        Ok(rec)
    }

    /// Las propiedades propias enumerables con clave string (las claves primero; cada valor
    /// solo si sigue siendo propio).
    fn properties(
        &mut self,
        agent: &mut Agent,
        o: Gc<Object>,
    ) -> JsResult<Vec<(Vec<u16>, Record)>> {
        let keys = enumerable_string_keys(agent, o)?;
        let mut out = Vec::with_capacity(keys.len());
        for k in keys {
            if !agent.has_own_property(o, &k)? {
                continue;
            }
            let v = agent.get(o, &k, Value::Object(o))?;
            out.push((key_units(&k), self.serialize(agent, &v)?));
        }
        Ok(out)
    }

    fn serialize_view(&mut self, agent: &mut Agent, v: Gc<Object>) -> JsResult<ObjRecord> {
        let (buffer, kind, offset, length, oob) = match &agent.heap[v].kind {
            ObjectKind::TypedArray(t) => (
                t.buffer,
                ViewKind::Typed(t.kind),
                t.byte_offset,
                t.array_length,
                crate::builtins::typed_array::ta_length(agent, v).is_none(),
            ),
            ObjectKind::DataView(d) => {
                let len = ab::byte_length(agent, d.buffer);
                let oob = ab::is_detached(agent, d.buffer)
                    || d.byte_offset > len
                    || d.byte_length.is_some_and(|l| d.byte_offset + l > len);
                (
                    d.buffer,
                    ViewKind::DataView,
                    d.byte_offset,
                    d.byte_length,
                    oob,
                )
            }
            _ => unreachable!("vista"),
        };
        if oob {
            return data_clone_error(agent, "An out-of-bounds ArrayBufferView");
        }
        let buffer = self.serialize(agent, &Value::Object(buffer))?;
        Ok(ObjRecord::View {
            kind,
            buffer,
            offset,
            length,
        })
    }

    fn serialize_map(&mut self, agent: &mut Agent, m: Gc<Object>) -> JsResult<ObjRecord> {
        let (entries, is_set): (Vec<(Value, Value)>, bool) = match &agent.heap[m].kind {
            ObjectKind::Map(d) => (d.entries.iter().flatten().cloned().collect(), false),
            ObjectKind::Set(d) => (d.entries.iter().flatten().cloned().collect(), true),
            _ => unreachable!("Map/Set"),
        };
        if is_set {
            let mut out = Vec::with_capacity(entries.len());
            for (k, _) in entries {
                out.push(self.serialize(agent, &k)?);
            }
            return Ok(ObjRecord::Set(out));
        }
        let mut out = Vec::with_capacity(entries.len());
        for (k, v) in entries {
            let k = self.serialize(agent, &k)?;
            let v = self.serialize(agent, &v)?;
            out.push((k, v));
        }
        Ok(ObjRecord::Map(out))
    }

    fn serialize_error(&mut self, agent: &mut Agent, e: Gc<Object>) -> JsResult<ObjRecord> {
        let name = agent.get_str(e, "name")?;
        let name = match &name {
            Value::String(s) => s.to_string_lossy(),
            _ => String::new(),
        };
        let name = match name.as_str() {
            "EvalError" | "RangeError" | "ReferenceError" | "SyntaxError" | "TypeError"
            | "URIError" => name,
            _ => "Error".to_owned(),
        };
        let message = match agent.get_own_property(e, &PropertyKey::from("message"))? {
            Some(d) if d.value.is_some() => Some(
                agent
                    .to_string(&d.value.unwrap_or_default())?
                    .units()
                    .to_vec(),
            ),
            _ => None,
        };
        let cause = match agent.get_own_property(e, &PropertyKey::from("cause"))? {
            Some(d) if d.value.is_some() => {
                Some(self.serialize(agent, &d.value.unwrap_or_default())?)
            }
            _ => None,
        };
        let stack = match agent.get_own_property(e, &PropertyKey::from("stack"))? {
            Some(d) => match d.value {
                Some(Value::String(s)) => Some(s.units().to_vec()),
                _ => None,
            },
            None => None,
        };
        Ok(ObjRecord::Error {
            name,
            message,
            cause,
            stack,
        })
    }
}

// --- StructuredDeserializeWithTransfer (§2.7.7) ---

struct Deserializer {
    memory: Vec<Option<Value>>,
    transfers: Vec<Value>,
}

/// StructuredDeserializeWithTransfer: el valor y los objetos transferidos (en el orden de la
/// lista de transferencia), creados en el realm en curso.
pub fn deserialize_with_transfer(
    agent: &mut Agent,
    s: Serialized,
) -> JsResult<(Value, Vec<Value>)> {
    let mut d = Deserializer {
        memory: Vec::new(),
        transfers: Vec::with_capacity(s.transfers.len()),
    };
    for t in s.transfers {
        let v = match t {
            Transferred::ArrayBuffer { bytes, max } => {
                Value::Object(new_array_buffer(agent, bytes, max))
            }
            Transferred::Host(h) => h.deserialize(agent)?,
        };
        d.transfers.push(v);
    }
    let v = d.deserialize(agent, s.value)?;
    Ok((v, d.transfers))
}

fn new_array_buffer(agent: &mut Agent, bytes: Vec<u8>, max: Option<usize>) -> Gc<Object> {
    let proto = agent.intrinsics().array_buffer_prototype;
    agent.alloc(Object::new(
        Some(proto),
        ObjectKind::ArrayBuffer(Box::new(ArrayBufferData {
            data: Some(bytes),
            max_byte_length: max,
            shared: false,
            block: None,
        })),
    ))
}

impl Deserializer {
    fn remember(&mut self, id: u32, v: Value) {
        let i = id as usize;
        if self.memory.len() <= i {
            self.memory.resize(i + 1, None);
        }
        self.memory[i] = Some(v);
    }

    fn deserialize(&mut self, agent: &mut Agent, r: Record) -> JsResult<Value> {
        Ok(match r {
            Record::Undefined => Value::Undefined,
            Record::Null => Value::Null,
            Record::Bool(b) => Value::Boolean(b),
            Record::Number(n) => Value::Number(n),
            Record::BigInt(b) => Value::BigInt(agent.heap.alloc(b)),
            Record::String(s) => Value::String(JsString::from_units(s)),
            Record::Ref(i) => self
                .memory
                .get(i as usize)
                .cloned()
                .flatten()
                .unwrap_or_default(),
            Record::Transferred(i) => self.transfers.get(i as usize).cloned().unwrap_or_default(),
            Record::Object(id, rec) => self.deserialize_object(agent, id, *rec)?,
        })
    }

    fn deserialize_object(
        &mut self,
        agent: &mut Agent,
        id: u32,
        rec: ObjRecord,
    ) -> JsResult<Value> {
        let i = agent.intrinsics().clone();
        let wrapper = |agent: &mut Agent, proto, kind| agent.alloc(Object::new(Some(proto), kind));
        let o = match rec {
            ObjRecord::Boolean(b) => wrapper(agent, i.boolean_prototype, ObjectKind::Boolean(b)),
            ObjRecord::Number(n) => wrapper(agent, i.number_prototype, ObjectKind::Number(n)),
            ObjRecord::BigInt(b) => {
                let b = agent.heap.alloc(b);
                wrapper(agent, i.bigint_prototype, ObjectKind::BigInt(b))
            }
            ObjRecord::String(s) => {
                let len = s.len();
                let c = wrapper(
                    agent,
                    i.string_prototype,
                    ObjectKind::String(JsString::from_units(s)),
                );
                let key = agent.keys.length.clone();
                agent.put_data(c, key, Value::Number(len as f64), false, false, false);
                c
            }
            ObjRecord::Date(t) => crate::builtins::date::create_date(agent, t),
            ObjRecord::RegExp(source, flags) => {
                let v = crate::builtins::regexp::regexp_create_literal(
                    agent,
                    &JsString::from_units(source),
                    &JsString::from_units(flags),
                )?;
                let Value::Object(r) = v else {
                    unreachable!("RegExpCreate")
                };
                r
            }
            ObjRecord::ArrayBuffer { bytes, max } => new_array_buffer(agent, bytes, max),
            ObjRecord::SharedArrayBuffer { block, max } => {
                crate::host::agents::shared_array_buffer(agent, block, max)
            }
            ObjRecord::View {
                kind,
                buffer,
                offset,
                length,
            } => {
                let b = self.deserialize(agent, buffer)?;
                let ctor = view_ctor(agent, kind)?;
                let mut args = vec![b, Value::Number(offset as f64)];
                if let Some(l) = length {
                    args.push(Value::Number(l as f64));
                }
                let Value::Object(c) = agent.construct(ctor, &args, None)? else {
                    unreachable!("vista")
                };
                c
            }
            ObjRecord::Map(entries) => {
                let c = wrapper(agent, i.map_prototype, ObjectKind::Map(Box::default()));
                self.remember(id, Value::Object(c));
                for (k, v) in entries {
                    let k = self.deserialize(agent, k)?;
                    let v = self.deserialize(agent, v)?;
                    let key = MapKey::of(agent, &k);
                    if let ObjectKind::Map(d) = &mut agent.heap[c].kind {
                        let d: &mut MapData = d;
                        d.set(key, k, v);
                    }
                }
                return Ok(Value::Object(c));
            }
            ObjRecord::Set(items) => {
                let c = wrapper(agent, i.set_prototype, ObjectKind::Set(Box::default()));
                self.remember(id, Value::Object(c));
                for k in items {
                    let k = self.deserialize(agent, k)?;
                    let key = MapKey::of(agent, &k);
                    if let ObjectKind::Set(d) = &mut agent.heap[c].kind {
                        let d: &mut MapData = d;
                        d.set(key, k, Value::Undefined);
                    }
                }
                return Ok(Value::Object(c));
            }
            ObjRecord::Error {
                name,
                message,
                cause,
                stack,
            } => {
                let proto = match name.as_str() {
                    "EvalError" => i.eval_error_prototype,
                    "RangeError" => i.range_error_prototype,
                    "ReferenceError" => i.reference_error_prototype,
                    "SyntaxError" => i.syntax_error_prototype,
                    "TypeError" => i.type_error_prototype,
                    "URIError" => i.uri_error_prototype,
                    _ => i.error_prototype,
                };
                let c = wrapper(agent, proto, ObjectKind::Error);
                self.remember(id, Value::Object(c));
                if let Some(m) = message {
                    let m = Value::String(JsString::from_units(m));
                    agent.put_data(c, PropertyKey::from("message"), m, true, false, true);
                }
                if let Some(cause) = cause {
                    let v = self.deserialize(agent, cause)?;
                    agent.put_data(c, PropertyKey::from("cause"), v, true, false, true);
                }
                match stack {
                    Some(s) => {
                        let s = Value::String(JsString::from_units(s));
                        agent.put_data(c, PropertyKey::from("stack"), s, true, false, true);
                    }
                    None => agent.attach_stack(c),
                }
                return Ok(Value::Object(c));
            }
            ObjRecord::Array { length, props } => {
                let c = agent.array_create(length, None)?;
                self.remember(id, Value::Object(c));
                self.fill(agent, c, props)?;
                return Ok(Value::Object(c));
            }
            ObjRecord::Object(props) => {
                let c = agent.new_plain_object();
                self.remember(id, Value::Object(c));
                self.fill(agent, c, props)?;
                return Ok(Value::Object(c));
            }
            ObjRecord::Host(h) => {
                let v = h.deserialize(agent)?;
                self.remember(id, v.clone());
                return Ok(v);
            }
        };
        self.remember(id, Value::Object(o));
        Ok(Value::Object(o))
    }

    fn fill(
        &mut self,
        agent: &mut Agent,
        o: Gc<Object>,
        props: Vec<(Vec<u16>, Record)>,
    ) -> JsResult<()> {
        for (k, r) in props {
            let v = self.deserialize(agent, r)?;
            let k = PropertyKey::from_js_string(JsString::from_units(k));
            agent.create_data_property_or_throw(o, k, v)?;
        }
        Ok(())
    }
}

/// El constructor intrínseco de una vista.
fn view_ctor(agent: &mut Agent, kind: ViewKind) -> JsResult<Gc<Object>> {
    let i = agent.intrinsics();
    use ElementType as E;
    Ok(match kind {
        ViewKind::Typed(t) => match t {
            E::Int8 => i.int8_array_ctor,
            E::Uint8 => i.uint8_array_ctor,
            E::Uint8Clamped => i.uint8_clamped_array_ctor,
            E::Int16 => i.int16_array_ctor,
            E::Uint16 => i.uint16_array_ctor,
            E::Int32 => i.int32_array_ctor,
            E::Uint32 => i.uint32_array_ctor,
            E::Float16 => i.float16_array_ctor,
            E::Float32 => i.float32_array_ctor,
            E::Float64 => i.float64_array_ctor,
            E::BigInt64 => i.bigint64_array_ctor,
            E::BigUint64 => i.biguint64_array_ctor,
        },
        ViewKind::DataView => {
            // %DataView%: el `constructor` propio de %DataView.prototype%.
            let p = i.data_view_prototype;
            match agent.heap[p]
                .props
                .get(&agent.keys.constructor)
                .map(|p| &p.value)
            {
                Some(crate::object::PropValue::Data {
                    value: Value::Object(c),
                    ..
                }) => *c,
                _ => return data_clone_error(agent, "A DataView"),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(src: &str, check: &str) {
        let mut a = Agent::new(crate::vm::AgentOptions::default());
        let v = a.eval_script(src, "src.js").expect("eval");
        let s = serialize_with_transfer(&mut a, &v, &[], &mut NoHostObjects, Options::default())
            .expect("serializar");
        // A otro agente, en otro hilo.
        let check = check.to_owned();
        std::thread::spawn(move || {
            let mut b = Agent::new(crate::vm::AgentOptions::default());
            let (v, _) = deserialize_with_transfer(&mut b, s).expect("deserializar");
            let g = b.realms[b.current_realm].global_object;
            b.put_data(g, PropertyKey::from("v"), v, true, true, true);
            let r = b.eval_script(&check, "check.js").expect("check");
            assert!(matches!(r, Value::Boolean(true)), "{check}");
        })
        .join()
        .expect("hilo");
    }

    #[test]
    fn values_cross_agents() {
        roundtrip(
            "var o = {a: 1, s: 'x', n: null, arr: [1, , 3], d: new Date(5), re: /a/gi, \
             m: new Map([[1, 'uno']]), st: new Set(['a']), e: new RangeError('mal'), \
             big: 10n, w: Object(true)}; o.self = o; o",
            "v.a === 1 && v.s === 'x' && v.n === null && v.arr.length === 3 && !(1 in v.arr) \
             && v.d.getTime() === 5 && v.re.flags === 'gi' && v.m.get(1) === 'uno' \
             && v.st.has('a') && v.e instanceof RangeError && v.e.message === 'mal' \
             && v.big === 10n && typeof v.w === 'object' && v.w.valueOf() === true \
             && v.self === v",
        );
    }

    #[test]
    fn buffers_and_views() {
        roundtrip(
            "var b = new ArrayBuffer(8); var u = new Uint8Array(b); u[0] = 7; \
             [u, new DataView(b, 2), b]",
            "v[0][0] === 7 && v[1].byteOffset === 2 && v[0].buffer === v[2] \
             && v[1].buffer === v[2]",
        );
    }

    #[test]
    fn functions_and_symbols_fail() {
        let mut a = Agent::new(crate::vm::AgentOptions::default());
        for src in ["(function () {})", "Symbol()", "({f: function () {}})"] {
            let v = a.eval_script(src, "src.js").expect("eval");
            let r =
                serialize_with_transfer(&mut a, &v, &[], &mut NoHostObjects, Options::default());
            assert!(r.is_err(), "{src}");
        }
    }

    #[test]
    fn shared_memory_needs_permission() {
        let mut a = Agent::new(crate::vm::AgentOptions::default());
        let v = a
            .eval_script(
                "var s = new SharedArrayBuffer(4); new Int32Array(s)",
                "src.js",
            )
            .expect("eval");
        assert!(
            serialize_with_transfer(&mut a, &v, &[], &mut NoHostObjects, Options::default())
                .is_err()
        );
        let s = serialize_with_transfer(
            &mut a,
            &v,
            &[],
            &mut NoHostObjects,
            Options { allow_shared: true },
        )
        .expect("compartido");
        let h = std::thread::spawn(move || {
            let mut b = Agent::new(crate::vm::AgentOptions::default());
            let (v, _) = deserialize_with_transfer(&mut b, s).expect("deserializar");
            let g = b.realms[b.current_realm].global_object;
            b.put_data(g, PropertyKey::from("v"), v, true, true, true);
            b.eval_script("Atomics.store(v, 0, 42)", "w.js")
                .expect("store");
        });
        h.join().expect("hilo");
        let r = a
            .eval_script("Atomics.load(new Int32Array(s), 0)", "r.js")
            .expect("load");
        assert!(matches!(r, Value::Number(n) if n == 42.0));
    }

    #[test]
    fn transfer_detaches() {
        let mut a = Agent::new(crate::vm::AgentOptions::default());
        let v = a
            .eval_script(
                "var t = new ArrayBuffer(4); new Uint8Array(t)[1] = 9; t",
                "src.js",
            )
            .expect("eval");
        let Value::Object(t) = v else { panic!() };
        let s = serialize_with_transfer(&mut a, &v, &[t], &mut NoHostObjects, Options::default())
            .expect("transferir");
        assert!(ab::is_detached(&a, t));
        let mut b = Agent::new(crate::vm::AgentOptions::default());
        let (v, list) = deserialize_with_transfer(&mut b, s).expect("deserializar");
        let Value::Object(nb) = v else { panic!() };
        assert!(matches!(list.first(), Some(Value::Object(o)) if *o == nb));
        assert_eq!(ab::read_bytes(&b, nb, 0, 4), vec![0, 9, 0, 0]);
    }
}
