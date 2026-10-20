//! `structuredClone(value, { transfer })` (HTML §2.7.10): StructuredSerializeWithTransfer
//! seguido de StructuredDeserializeWithTransfer en el mismo realm, hecho en un solo recorrido
//! con el mapa de memoria del spec (identidad y ciclos). Tipos: primitivos (salvo Symbol),
//! los wrappers Boolean/Number/BigInt/String, Date, RegExp, ArrayBuffer, las vistas
//! (TypedArray, DataView), Map, Set, los Error nativos, Array y los objetos ordinarios. Todo lo
//! demás (funciones, Proxy, objetos con otros internal slots) es un `DataCloneError`.

use std::collections::HashMap;

use crate::builtins::array_buffer::ArrayBufferData;
use crate::builtins::map_set::{MapData, MapKey};
use crate::builtins::web::{require_args, throw_dom};
use crate::heap::Gc;
use crate::object::{Object, ObjectKind};
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

pub fn init(agent: &mut Agent, realm: RealmId) {
    let f = agent.create_builtin_function(structured_clone, 1, "structuredClone", false);
    crate::builtins::register_global(agent, realm, "structuredClone", f);
}

fn data_clone_error<T>(agent: &mut Agent, what: &str) -> JsResult<T> {
    throw_dom(
        agent,
        "DataCloneError",
        &format!("Failed to execute 'structuredClone': {what} could not be cloned."),
    )
}

/// `structuredClone(value, options)`.
fn structured_clone(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    require_args(agent, a, 1, "Failed to execute 'structuredClone'")?;
    let transfer = transfer_list(agent, &a.arg(1))?;
    let mut c = Cloner {
        memory: HashMap::new(),
        transferred: Vec::new(),
    };
    // Los ArrayBuffer transferidos: el clon se crea vacío y recibe los bytes al final (el
    // original queda detached).
    for &b in &transfer {
        let ok = matches!(&agent.heap[b].kind, ObjectKind::ArrayBuffer(d) if !d.shared && d.data.is_some());
        if !ok {
            return data_clone_error(agent, "An ArrayBuffer is detached or not transferable and");
        }
        if c.memory.contains_key(&b) {
            return data_clone_error(
                agent,
                "An ArrayBuffer is duplicated in the transfer list and",
            );
        }
        // El clon recibe los bytes ya (las vistas transferidas se crean sobre él); el
        // original se detacha al final.
        let copy = match &agent.heap[b].kind {
            ObjectKind::ArrayBuffer(d) => ArrayBufferData {
                data: d.data.clone(),
                max_byte_length: d.max_byte_length,
                shared: false,
            },
            _ => ArrayBufferData::default(),
        };
        let proto = agent.intrinsics().array_buffer_prototype;
        let nb = agent.alloc(Object::new(
            Some(proto),
            ObjectKind::ArrayBuffer(Box::new(copy)),
        ));
        c.memory.insert(b, nb);
        c.transferred.push((b, nb));
    }
    let out = c.clone_value(agent, &a.arg(0))?;
    for (src, _) in c.transferred {
        if let ObjectKind::ArrayBuffer(d) = &mut agent.heap[src].kind {
            d.data = None;
        }
    }
    Ok(out)
}

/// `options.transfer` (StructuredSerializeOptions: `sequence<object> transfer = []`).
fn transfer_list(agent: &mut Agent, options: &Value) -> JsResult<Vec<Gc<Object>>> {
    let o = match options {
        Value::Undefined | Value::Null => return Ok(Vec::new()),
        Value::Object(o) => *o,
        _ => {
            return agent
                .type_error("The provided value is not of type 'StructuredSerializeOptions'");
        }
    };
    let t = agent.get_str(o, "transfer")?;
    if t.is_undefined() {
        return Ok(Vec::new());
    }
    let items = agent.iterable_to_list(&t)?;
    let mut out = Vec::new();
    for v in items {
        match v {
            Value::Object(o) => out.push(o),
            _ => return agent.type_error("Value in the transfer list is not an object"),
        }
    }
    Ok(out)
}

struct Cloner {
    memory: HashMap<Gc<Object>, Gc<Object>>,
    transferred: Vec<(Gc<Object>, Gc<Object>)>,
}

impl Cloner {
    fn clone_value(&mut self, agent: &mut Agent, v: &Value) -> JsResult<Value> {
        let o = match v {
            Value::Symbol(_) => return data_clone_error(agent, "Symbol"),
            Value::Object(o) => *o,
            other => return Ok(other.clone()),
        };
        if let Some(&c) = self.memory.get(&o) {
            return Ok(Value::Object(c));
        }
        enum Kind {
            Primitive(ObjectKind),
            Date(f64),
            RegExp(crate::string::JsString, crate::string::JsString),
            Buffer,
            View,
            Map(bool),
            Error,
            Array,
            Plain,
        }
        let kind = {
            let obj = &agent.heap[o];
            if obj.is_callable() {
                return data_clone_error(agent, "A function");
            }
            match &obj.kind {
                ObjectKind::Boolean(b) => Kind::Primitive(ObjectKind::Boolean(*b)),
                ObjectKind::Number(n) => Kind::Primitive(ObjectKind::Number(*n)),
                ObjectKind::String(s) => Kind::Primitive(ObjectKind::String(s.clone())),
                ObjectKind::BigInt(b) => Kind::Primitive(ObjectKind::BigInt(*b)),
                ObjectKind::Date(t) => Kind::Date(*t),
                ObjectKind::RegExp(r) => Kind::RegExp(r.source.clone(), r.flags.clone()),
                ObjectKind::ArrayBuffer(_) => Kind::Buffer,
                ObjectKind::TypedArray(_) | ObjectKind::DataView(_) => Kind::View,
                ObjectKind::Map(_) => Kind::Map(false),
                ObjectKind::Set(_) => Kind::Map(true),
                ObjectKind::Error => Kind::Error,
                ObjectKind::Array(_) => Kind::Array,
                ObjectKind::Ordinary | ObjectKind::Arguments(None) => Kind::Plain,
                _ => return data_clone_error(agent, "An object of this type"),
            }
        };
        let out = match kind {
            Kind::Primitive(k) => {
                let i = agent.intrinsics();
                let proto = match &k {
                    ObjectKind::Boolean(_) => i.boolean_prototype,
                    ObjectKind::Number(_) => i.number_prototype,
                    ObjectKind::BigInt(_) => i.bigint_prototype,
                    _ => i.string_prototype,
                };
                let len = match &k {
                    ObjectKind::String(s) => Some(s.len()),
                    _ => None,
                };
                let c = agent.alloc(Object::new(Some(proto), k));
                if let Some(len) = len {
                    let key = agent.keys.length.clone();
                    agent.put_data(c, key, Value::Number(len as f64), false, false, false);
                }
                c
            }
            Kind::Date(t) => crate::builtins::date::create_date(agent, t),
            Kind::RegExp(source, flags) => {
                let Value::Object(r) =
                    crate::builtins::regexp::regexp_create_literal(agent, &source, &flags)?
                else {
                    unreachable!("RegExpCreate")
                };
                r
            }
            Kind::Buffer => return self.clone_buffer(agent, o).map(Value::Object),
            Kind::View => return self.clone_view(agent, o).map(Value::Object),
            Kind::Map(is_set) => return self.clone_map(agent, o, is_set).map(Value::Object),
            Kind::Error => return self.clone_error(agent, o).map(Value::Object),
            Kind::Array => {
                let len = agent.get_str(o, "length")?;
                let len = agent.to_length(&len)?;
                let c = agent.array_create(len as u64, None)?;
                self.memory.insert(o, c);
                self.copy_properties(agent, o, c)?;
                return Ok(Value::Object(c));
            }
            Kind::Plain => {
                let c = agent.new_plain_object();
                self.memory.insert(o, c);
                self.copy_properties(agent, o, c)?;
                return Ok(Value::Object(c));
            }
        };
        self.memory.insert(o, out);
        Ok(Value::Object(out))
    }

    /// Las propiedades propias enumerables con clave string (pasos de Object/Array de
    /// StructuredSerializeInternal: claves primero, cada valor si sigue siendo propio).
    fn copy_properties(
        &mut self,
        agent: &mut Agent,
        src: Gc<Object>,
        dst: Gc<Object>,
    ) -> JsResult<()> {
        let keys = agent.own_property_keys(src)?;
        let mut enumerable = Vec::new();
        for k in keys {
            if k.is_symbol() {
                continue;
            }
            if let Some(d) = agent.get_own_property(src, &k)?
                && d.enumerable == Some(true)
            {
                enumerable.push(k);
            }
        }
        for k in enumerable {
            if !agent.has_own_property(src, &k)? {
                continue;
            }
            let v = agent.get(src, &k, Value::Object(src))?;
            let cv = self.clone_value(agent, &v)?;
            agent.create_data_property(dst, k, cv)?;
        }
        Ok(())
    }

    fn clone_buffer(&mut self, agent: &mut Agent, b: Gc<Object>) -> JsResult<Gc<Object>> {
        let (data, max, shared) = match &agent.heap[b].kind {
            ObjectKind::ArrayBuffer(d) => (d.data.clone(), d.max_byte_length, d.shared),
            _ => unreachable!("ArrayBuffer"),
        };
        if shared {
            return data_clone_error(agent, "A SharedArrayBuffer (sin aislamiento cross-origin)");
        }
        let Some(data) = data else {
            return data_clone_error(agent, "A detached ArrayBuffer");
        };
        let proto = agent.intrinsics().array_buffer_prototype;
        let c = agent.alloc(Object::new(
            Some(proto),
            ObjectKind::ArrayBuffer(Box::new(ArrayBufferData {
                data: Some(data),
                max_byte_length: max,
                shared: false,
            })),
        ));
        self.memory.insert(b, c);
        Ok(c)
    }

    fn clone_view(&mut self, agent: &mut Agent, v: Gc<Object>) -> JsResult<Gc<Object>> {
        // Una vista fuera de los límites de su buffer (o con el buffer detached) no se
        // serializa.
        let oob = match &agent.heap[v].kind {
            ObjectKind::TypedArray(_) => {
                crate::builtins::typed_array::ta_length(agent, v).is_none()
            }
            ObjectKind::DataView(d) => {
                let len = crate::builtins::array_buffer::byte_length(agent, d.buffer);
                crate::builtins::array_buffer::is_detached(agent, d.buffer)
                    || d.byte_offset > len
                    || d.byte_length.is_some_and(|l| d.byte_offset + l > len)
            }
            _ => false,
        };
        if oob {
            return data_clone_error(agent, "An out-of-bounds ArrayBufferView");
        }
        let (buffer, offset, length, ctor) = match &agent.heap[v].kind {
            ObjectKind::TypedArray(t) => {
                let i = agent.intrinsics();
                use crate::builtins::array_buffer::ElementType as E;
                let ctor = match t.kind {
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
                };
                (t.buffer, t.byte_offset, t.array_length, ctor)
            }
            ObjectKind::DataView(d) => {
                // %DataView%: el `constructor` propio de %DataView.prototype%.
                let p = agent.intrinsics().data_view_prototype;
                let ctor = match agent.heap[p]
                    .props
                    .get(&agent.keys.constructor)
                    .map(|p| &p.value)
                {
                    Some(crate::object::PropValue::Data {
                        value: Value::Object(c),
                        ..
                    }) => *c,
                    _ => return data_clone_error(agent, "A DataView"),
                };
                (d.buffer, d.byte_offset, d.byte_length, ctor)
            }
            _ => unreachable!("vista"),
        };
        let nb = self.clone_value(agent, &Value::Object(buffer))?;
        let mut args = vec![nb, Value::Number(offset as f64)];
        if let Some(l) = length {
            args.push(Value::Number(l as f64));
        }
        let Value::Object(c) = agent.construct(ctor, &args, None)? else {
            unreachable!("vista")
        };
        self.memory.insert(v, c);
        Ok(c)
    }

    fn clone_map(
        &mut self,
        agent: &mut Agent,
        m: Gc<Object>,
        is_set: bool,
    ) -> JsResult<Gc<Object>> {
        let entries: Vec<(Value, Value)> = match &agent.heap[m].kind {
            ObjectKind::Map(d) | ObjectKind::Set(d) => {
                d.entries.iter().flatten().cloned().collect()
            }
            _ => unreachable!("Map/Set"),
        };
        let i = agent.intrinsics();
        let (proto, kind) = if is_set {
            (i.set_prototype, ObjectKind::Set(Box::default()))
        } else {
            (i.map_prototype, ObjectKind::Map(Box::default()))
        };
        let c = agent.alloc(Object::new(Some(proto), kind));
        self.memory.insert(m, c);
        for (k, v) in entries {
            let ck = self.clone_value(agent, &k)?;
            let cv = if is_set {
                Value::Undefined
            } else {
                self.clone_value(agent, &v)?
            };
            let key = MapKey::of(agent, &ck);
            if let ObjectKind::Map(d) | ObjectKind::Set(d) = &mut agent.heap[c].kind {
                let d: &mut MapData = d;
                d.set(key, ck, cv);
            }
        }
        Ok(c)
    }

    fn clone_error(&mut self, agent: &mut Agent, e: Gc<Object>) -> JsResult<Gc<Object>> {
        let name = agent.get_str(e, "name")?;
        let name = match &name {
            Value::String(s) => s.to_string_lossy(),
            _ => String::new(),
        };
        let i = agent.intrinsics();
        let proto = match name.as_str() {
            "EvalError" => i.eval_error_prototype,
            "RangeError" => i.range_error_prototype,
            "ReferenceError" => i.reference_error_prototype,
            "SyntaxError" => i.syntax_error_prototype,
            "TypeError" => i.type_error_prototype,
            "URIError" => i.uri_error_prototype,
            _ => i.error_prototype,
        };
        let message = match agent.get_own_property(e, &PropertyKey::from("message"))? {
            Some(d) if d.value.is_some() => Some(agent.to_string(&d.value.unwrap_or_default())?),
            _ => None,
        };
        let c = agent.alloc(Object::new(Some(proto), ObjectKind::Error));
        self.memory.insert(e, c);
        if let Some(m) = message {
            agent.put_data(
                c,
                PropertyKey::from("message"),
                Value::String(m),
                true,
                false,
                true,
            );
        }
        if let Some(d) = agent.get_own_property(e, &PropertyKey::from("cause"))?
            && let Some(v) = d.value
        {
            let cv = self.clone_value(agent, &v)?;
            agent.put_data(c, PropertyKey::from("cause"), cv, true, false, true);
        }
        agent.attach_stack(c);
        Ok(c)
    }
}
