//! Object (§20.1) y Object.prototype, con los métodos de B.2.2.

use crate::builtins::{FnEntry, accessor, constructor, methods};
use crate::object::{IterKind, ObjectKind, PropertyDescriptor};
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

pub fn init(agent: &mut Agent, realm: RealmId) {
    let proto = agent.realms[realm].intrinsics.object_prototype;
    let ctor = constructor(agent, "Object", 1, object_ctor, proto);
    agent.realms[realm].intrinsics.object_ctor = ctor;
    const STATICS: &[FnEntry] = &[
        ("assign", 2, assign),
        ("create", 2, create),
        ("defineProperties", 2, define_properties),
        ("defineProperty", 3, define_property),
        ("entries", 1, entries),
        ("freeze", 1, freeze),
        ("fromEntries", 1, from_entries),
        ("getOwnPropertyDescriptor", 2, get_own_property_descriptor),
        ("getOwnPropertyDescriptors", 1, get_own_property_descriptors),
        ("getOwnPropertyNames", 1, get_own_property_names),
        ("getOwnPropertySymbols", 1, get_own_property_symbols),
        ("getPrototypeOf", 1, get_prototype_of),
        ("hasOwn", 2, has_own),
        ("is", 2, is),
        ("isExtensible", 1, is_extensible),
        ("isFrozen", 1, is_frozen),
        ("isSealed", 1, is_sealed),
        ("keys", 1, keys),
        ("preventExtensions", 1, prevent_extensions),
        ("seal", 1, seal),
        ("setPrototypeOf", 2, set_prototype_of),
        ("values", 1, values),
    ];
    methods(agent, ctor, STATICS);
    const PROTO: &[FnEntry] = &[
        ("hasOwnProperty", 1, has_own_property),
        ("isPrototypeOf", 1, is_prototype_of),
        ("propertyIsEnumerable", 1, property_is_enumerable),
        ("toLocaleString", 0, to_locale_string),
        ("toString", 0, to_string),
        ("valueOf", 0, value_of),
        ("__defineGetter__", 2, define_getter),
        ("__defineSetter__", 2, define_setter),
        ("__lookupGetter__", 1, lookup_getter),
        ("__lookupSetter__", 1, lookup_setter),
    ];
    methods(agent, proto, PROTO);
    let k = agent.keys.proto.clone();
    accessor(
        agent,
        proto,
        k,
        "__proto__",
        Some(proto_get),
        Some(proto_set),
    );
}

/// Object ( [ value ] ) (§20.1.1.1).
fn object_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    if let Some(nt) = a.new_target
        && nt != a.callee
    {
        let o = agent.ordinary_create_from_constructor(
            nt,
            |i| i.object_prototype,
            ObjectKind::Ordinary,
        )?;
        return Ok(Value::Object(o));
    }
    let v = a.arg(0);
    if v.is_nullish() {
        return Ok(Value::Object(agent.new_plain_object()));
    }
    Ok(Value::Object(agent.to_object(&v)?))
}

/// Object.assign (§20.1.2.1).
fn assign(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let to = agent.to_object(&a.arg(0))?;
    for src in a.args.iter().skip(1) {
        if src.is_nullish() {
            continue;
        }
        let from = agent.to_object(src)?;
        let keys = agent.own_property_keys(from)?;
        for k in keys {
            if let Some(d) = agent.get_own_property(from, &k)?
                && d.enumerable == Some(true)
            {
                let v = agent.get(from, &k, Value::Object(from))?;
                agent.set_or_throw(to, k, v, true)?;
            }
        }
    }
    Ok(Value::Object(to))
}

/// Object.create (§20.1.2.2).
fn create(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let proto = match a.arg(0) {
        Value::Object(o) => Some(o),
        Value::Null => None,
        _ => return agent.type_error("Object prototype may only be an Object or null"),
    };
    let o = agent.new_object(proto);
    let props = a.arg(1);
    if !props.is_undefined() {
        object_define_properties(agent, o, &props)?;
    }
    Ok(Value::Object(o))
}

/// ObjectDefineProperties (§20.1.2.3.1).
fn object_define_properties(
    agent: &mut Agent,
    o: crate::heap::Gc<crate::object::Object>,
    props: &Value,
) -> JsResult<()> {
    let p = agent.to_object(props)?;
    let keys = agent.own_property_keys(p)?;
    let mut descs = Vec::new();
    for k in keys {
        if let Some(d) = agent.get_own_property(p, &k)?
            && d.enumerable == Some(true)
        {
            let dobj = agent.get(p, &k, Value::Object(p))?;
            let desc = agent.to_property_descriptor(&dobj)?;
            descs.push((k, desc));
        }
    }
    for (k, d) in descs {
        agent.define_property_or_throw(o, k, d)?;
    }
    Ok(())
}

fn define_properties(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Value::Object(o) = a.arg(0) else {
        return agent.type_error("Object.defineProperties called on non-object");
    };
    object_define_properties(agent, o, &a.arg(1))?;
    Ok(Value::Object(o))
}

/// Object.defineProperty (§20.1.2.4).
fn define_property(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Value::Object(o) = a.arg(0) else {
        return agent.type_error("Object.defineProperty called on non-object");
    };
    let key = agent.to_property_key(&a.arg(1))?;
    let desc = agent.to_property_descriptor(&a.arg(2))?;
    agent.define_property_or_throw(o, key, desc)?;
    Ok(Value::Object(o))
}

fn enumerable(agent: &mut Agent, a: &CallArgs<'_>, kind: IterKind) -> JsResult<Value> {
    let o = agent.to_object(&a.arg(0))?;
    let list = agent.enumerable_own_properties(o, kind)?;
    Ok(Value::Object(agent.create_array_from_list(&list)?))
}

fn entries(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    enumerable(agent, a, IterKind::Entries)
}

fn keys(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    enumerable(agent, a, IterKind::Keys)
}

fn values(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    enumerable(agent, a, IterKind::Values)
}

fn freeze(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = a.arg(0);
    if let Value::Object(o) = v
        && !agent.set_integrity_level(o, true)?
    {
        return agent.type_error("Cannot freeze object");
    }
    Ok(v)
}

fn seal(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = a.arg(0);
    if let Value::Object(o) = v
        && !agent.set_integrity_level(o, false)?
    {
        return agent.type_error("Cannot seal object");
    }
    Ok(v)
}

fn prevent_extensions(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = a.arg(0);
    if let Value::Object(o) = v
        && !agent.prevent_extensions(o)?
    {
        return agent.type_error("Cannot prevent extensions");
    }
    Ok(v)
}

fn is_frozen(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    match a.arg(0) {
        Value::Object(o) => Ok(Value::Boolean(agent.test_integrity_level(o, true)?)),
        _ => Ok(Value::Boolean(true)),
    }
}

fn is_sealed(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    match a.arg(0) {
        Value::Object(o) => Ok(Value::Boolean(agent.test_integrity_level(o, false)?)),
        _ => Ok(Value::Boolean(true)),
    }
}

fn is_extensible(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    match a.arg(0) {
        Value::Object(o) => Ok(Value::Boolean(agent.is_extensible(o)?)),
        _ => Ok(Value::Boolean(false)),
    }
}

/// Object.fromEntries (§20.1.2.7).
fn from_entries(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let iterable = a.arg(0);
    agent.require_object_coercible(&iterable)?;
    let obj = agent.new_plain_object();
    let (it, next) = agent.get_iterator(&iterable)?;
    loop {
        let Some(entry) = agent.iterator_step_value(&it, &next)? else {
            return Ok(Value::Object(obj));
        };
        let r = (|| -> JsResult<()> {
            let Value::Object(e) = entry else {
                return agent.type_error("Iterator value is not an entry object");
            };
            let k = agent.get(e, &PropertyKey::Index(0), Value::Object(e))?;
            let v = agent.get(e, &PropertyKey::Index(1), Value::Object(e))?;
            let key = agent.to_property_key(&k)?;
            agent.create_data_property_or_throw(obj, key, v)
        })();
        if let Err(e) = r {
            agent.iterator_close_on_throw(&it);
            return Err(e);
        }
    }
}

/// Object.getOwnPropertyDescriptor (§20.1.2.8).
fn get_own_property_descriptor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = agent.to_object(&a.arg(0))?;
    let key = agent.to_property_key(&a.arg(1))?;
    let d = agent.get_own_property(o, &key)?;
    agent.from_property_descriptor(d.as_ref())
}

/// Object.getOwnPropertyDescriptors (§20.1.2.9).
fn get_own_property_descriptors(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = agent.to_object(&a.arg(0))?;
    let keys = agent.own_property_keys(o)?;
    let out = agent.new_plain_object();
    for k in keys {
        let d = agent.get_own_property(o, &k)?;
        let dv = agent.from_property_descriptor(d.as_ref())?;
        if !dv.is_undefined() {
            agent.create_data_property_or_throw(out, k, dv)?;
        }
    }
    Ok(Value::Object(out))
}

/// GetOwnPropertyKeys (§20.1.2.11.1).
fn own_keys(agent: &mut Agent, v: &Value, symbols: bool) -> JsResult<Value> {
    let o = agent.to_object(v)?;
    let keys = agent.own_property_keys(o)?;
    let list: Vec<Value> = keys
        .into_iter()
        .filter(|k| k.is_symbol() == symbols)
        .map(|k| k.to_value())
        .collect();
    Ok(Value::Object(agent.create_array_from_list(&list)?))
}

fn get_own_property_names(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    own_keys(agent, &a.arg(0), false)
}

fn get_own_property_symbols(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    own_keys(agent, &a.arg(0), true)
}

fn get_prototype_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = agent.to_object(&a.arg(0))?;
    Ok(agent
        .get_prototype_of(o)?
        .map_or(Value::Null, Value::Object))
}

/// Object.setPrototypeOf (§20.1.2.23).
fn set_prototype_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = a.arg(0);
    agent.require_object_coercible(&o)?;
    let proto = match a.arg(1) {
        Value::Object(p) => Some(p),
        Value::Null => None,
        _ => return agent.type_error("Object prototype may only be an Object or null"),
    };
    let Value::Object(obj) = o else { return Ok(o) };
    if !agent.set_prototype_of(obj, proto)? {
        return agent.type_error("Cannot set prototype");
    }
    Ok(o)
}

fn has_own(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = agent.to_object(&a.arg(0))?;
    let key = agent.to_property_key(&a.arg(1))?;
    Ok(Value::Boolean(agent.has_own_property(o, &key)?))
}

fn is(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(agent.same_value(&a.arg(0), &a.arg(1))))
}

// --- Object.prototype ---

/// Object.prototype.hasOwnProperty (§20.1.3.2): ToPropertyKey antes de ToObject.
fn has_own_property(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let key = agent.to_property_key(&a.arg(0))?;
    let o = agent.to_object(&a.this)?;
    Ok(Value::Boolean(agent.has_own_property(o, &key)?))
}

/// Object.prototype.isPrototypeOf (§20.1.3.3).
fn is_prototype_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Value::Object(mut v) = a.arg(0) else {
        return Ok(Value::Boolean(false));
    };
    let o = agent.to_object(&a.this)?;
    loop {
        match agent.get_prototype_of(v)? {
            None => return Ok(Value::Boolean(false)),
            Some(p) if p == o => return Ok(Value::Boolean(true)),
            Some(p) => v = p,
        }
    }
}

/// Object.prototype.propertyIsEnumerable (§20.1.3.4).
fn property_is_enumerable(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let key = agent.to_property_key(&a.arg(0))?;
    let o = agent.to_object(&a.this)?;
    let d = agent.get_own_property(o, &key)?;
    Ok(Value::Boolean(
        d.is_some_and(|d| d.enumerable == Some(true)),
    ))
}

fn to_locale_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let k = agent.keys.to_string.clone();
    agent.invoke(&a.this, &k, &[])
}

/// Object.prototype.toString (§20.1.3.6).
pub fn to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    match a.this {
        Value::Undefined => return Ok(Value::str("[object Undefined]")),
        Value::Null => return Ok(Value::str("[object Null]")),
        _ => {}
    }
    let o = agent.to_object(&a.this)?;
    let builtin = {
        let obj = &agent.heap[o];
        match &obj.kind {
            ObjectKind::Array(_) => "Array",
            ObjectKind::Arguments(_) => "Arguments",
            _ if obj.is_callable() => "Function",
            ObjectKind::Error => "Error",
            ObjectKind::Boolean(_) => "Boolean",
            ObjectKind::Number(_) => "Number",
            ObjectKind::String(_) => "String",
            _ => "Object",
        }
    };
    let k = PropertyKey::Symbol(agent.well_known.to_string_tag);
    let tag = agent.get(o, &k, Value::Object(o))?;
    let t = match tag {
        Value::String(s) => s.to_string_lossy(),
        _ => builtin.to_owned(),
    };
    Ok(Value::str(&format!("[object {t}]")))
}

fn value_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Object(agent.to_object(&a.this)?))
}

/// `get Object.prototype.__proto__` (B.2.2.1.1).
fn proto_get(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = agent.to_object(&a.this)?;
    Ok(agent
        .get_prototype_of(o)?
        .map_or(Value::Null, Value::Object))
}

/// `set Object.prototype.__proto__` (B.2.2.1.2).
fn proto_set(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    agent.require_object_coercible(&a.this)?;
    let proto = match a.arg(0) {
        Value::Object(p) => Some(p),
        Value::Null => None,
        _ => return Ok(Value::Undefined),
    };
    let Value::Object(o) = a.this else {
        return Ok(Value::Undefined);
    };
    if !agent.set_prototype_of(o, proto)? {
        return agent.type_error("Object.prototype.__proto__ setter failed");
    }
    Ok(Value::Undefined)
}

/// `__defineGetter__` / `__defineSetter__` (B.2.2.2–3).
fn define_accessor(agent: &mut Agent, a: &CallArgs<'_>, setter: bool) -> JsResult<Value> {
    let o = agent.to_object(&a.this)?;
    let f = a.arg(1);
    if !agent.is_callable(&f) {
        return agent.type_error("Object.prototype.__defineGetter__: Expecting function");
    }
    let key = agent.to_property_key(&a.arg(0))?;
    let desc = if setter {
        PropertyDescriptor {
            set: Some(f.as_object()),
            enumerable: Some(true),
            configurable: Some(true),
            ..PropertyDescriptor::default()
        }
    } else {
        PropertyDescriptor {
            get: Some(f.as_object()),
            enumerable: Some(true),
            configurable: Some(true),
            ..PropertyDescriptor::default()
        }
    };
    agent.define_property_or_throw(o, key, desc)?;
    Ok(Value::Undefined)
}

fn define_getter(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    define_accessor(agent, a, false)
}

fn define_setter(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    define_accessor(agent, a, true)
}

/// `__lookupGetter__` / `__lookupSetter__` (B.2.2.4–5).
fn lookup_accessor(agent: &mut Agent, a: &CallArgs<'_>, setter: bool) -> JsResult<Value> {
    let mut o = agent.to_object(&a.this)?;
    let key = agent.to_property_key(&a.arg(0))?;
    loop {
        if let Some(d) = agent.get_own_property(o, &key)? {
            if d.is_accessor() {
                let f = if setter {
                    d.set.flatten()
                } else {
                    d.get.flatten()
                };
                return Ok(f.into());
            }
            return Ok(Value::Undefined);
        }
        match agent.get_prototype_of(o)? {
            Some(p) => o = p,
            None => return Ok(Value::Undefined),
        }
    }
}

fn lookup_getter(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    lookup_accessor(agent, a, false)
}

fn lookup_setter(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    lookup_accessor(agent, a, true)
}
