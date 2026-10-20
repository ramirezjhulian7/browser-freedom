//! Reflect (§28.1).

use crate::builtins::{FnEntry, methods, to_string_tag};
use crate::value::Value;
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

pub fn init(agent: &mut Agent, realm: RealmId) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    let r = agent.new_object(Some(op));
    const FNS: &[FnEntry] = &[
        ("apply", 3, apply),
        ("construct", 2, construct),
        ("defineProperty", 3, define_property),
        ("deleteProperty", 2, delete_property),
        ("get", 2, get),
        ("getOwnPropertyDescriptor", 2, get_own_property_descriptor),
        ("getPrototypeOf", 1, get_prototype_of),
        ("has", 2, has),
        ("isExtensible", 1, is_extensible),
        ("ownKeys", 1, own_keys),
        ("preventExtensions", 1, prevent_extensions),
        ("set", 3, set),
        ("setPrototypeOf", 2, set_prototype_of),
    ];
    methods(agent, r, FNS);
    to_string_tag(agent, r, "Reflect");
    agent.realms[realm].intrinsics.reflect = r;
}

fn target(agent: &mut Agent, v: &Value) -> JsResult<crate::heap::Gc<crate::object::Object>> {
    match v {
        Value::Object(o) => Ok(*o),
        _ => agent.type_error("Reflect method called on non-object"),
    }
}

/// Reflect.apply (§28.1.1).
fn apply(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let f = a.arg(0);
    if !agent.is_callable(&f) {
        return agent.type_error("Reflect.apply target is not callable");
    }
    let args = agent.create_list_from_array_like(&a.arg(2), false)?;
    agent.call_internal(&f, a.arg(1), &args)
}

/// Reflect.construct (§28.1.2).
fn construct(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = a.arg(0);
    if !agent.is_constructor(&t) {
        return agent.type_error("Reflect.construct target is not a constructor");
    }
    let nt = if a.len() < 3 { t.clone() } else { a.arg(2) };
    if !agent.is_constructor(&nt) {
        return agent.type_error("Reflect.construct newTarget is not a constructor");
    }
    let args = agent.create_list_from_array_like(&a.arg(1), false)?;
    let (t, nt) = (
        t.as_object().expect("constructor"),
        nt.as_object().expect("constructor"),
    );
    agent.construct_internal(t, &args, nt)
}

fn define_property(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = target(agent, &a.arg(0))?;
    let key = agent.to_property_key(&a.arg(1))?;
    let desc = agent.to_property_descriptor(&a.arg(2))?;
    Ok(Value::Boolean(agent.define_own_property(o, key, desc)?))
}

fn delete_property(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = target(agent, &a.arg(0))?;
    let key = agent.to_property_key(&a.arg(1))?;
    Ok(Value::Boolean(agent.delete(o, &key)?))
}

fn get(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = target(agent, &a.arg(0))?;
    let key = agent.to_property_key(&a.arg(1))?;
    let receiver = if a.len() < 3 { a.arg(0) } else { a.arg(2) };
    agent.get(o, &key, receiver)
}

fn get_own_property_descriptor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = target(agent, &a.arg(0))?;
    let key = agent.to_property_key(&a.arg(1))?;
    let d = agent.get_own_property(o, &key)?;
    agent.from_property_descriptor(d.as_ref())
}

fn get_prototype_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = target(agent, &a.arg(0))?;
    Ok(agent
        .get_prototype_of(o)?
        .map_or(Value::Null, Value::Object))
}

fn has(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = target(agent, &a.arg(0))?;
    let key = agent.to_property_key(&a.arg(1))?;
    Ok(Value::Boolean(agent.has_property(o, &key)?))
}

fn is_extensible(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = target(agent, &a.arg(0))?;
    Ok(Value::Boolean(agent.is_extensible(o)?))
}

fn own_keys(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = target(agent, &a.arg(0))?;
    let keys: Vec<Value> = agent
        .own_property_keys(o)?
        .iter()
        .map(|k| k.to_value())
        .collect();
    Ok(Value::Object(agent.create_array_from_list(&keys)?))
}

fn prevent_extensions(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = target(agent, &a.arg(0))?;
    Ok(Value::Boolean(agent.prevent_extensions(o)?))
}

fn set(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = target(agent, &a.arg(0))?;
    let key = agent.to_property_key(&a.arg(1))?;
    let receiver = if a.len() < 4 { a.arg(0) } else { a.arg(3) };
    Ok(Value::Boolean(agent.set(o, key, a.arg(2), receiver)?))
}

fn set_prototype_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = target(agent, &a.arg(0))?;
    let proto = match a.arg(1) {
        Value::Object(p) => Some(p),
        Value::Null => None,
        _ => return agent.type_error("Object prototype may only be an Object or null"),
    };
    Ok(Value::Boolean(agent.set_prototype_of(o, proto)?))
}
