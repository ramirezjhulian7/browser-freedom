//! Symbol (§20.4).

use crate::builtins::{FnEntry, accessor, constructor, frozen_prop, methods, to_string_tag};
use crate::heap::Gc;
use crate::object::ObjectKind;
use crate::string::JsString;
use crate::value::{PropertyKey, Symbol, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

pub fn init(agent: &mut Agent, realm: RealmId) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    let proto = agent.new_object(Some(op));
    let ctor = constructor(agent, "Symbol", 0, symbol_ctor, proto);
    {
        let i = &mut agent.realms[realm].intrinsics;
        i.symbol_ctor = ctor;
        i.symbol_prototype = proto;
    }
    const STATICS: &[FnEntry] = &[("for", 1, symbol_for), ("keyFor", 1, key_for)];
    methods(agent, ctor, STATICS);
    for (name, sym) in agent.well_known.all() {
        frozen_prop(agent, ctor, name, Value::Symbol(sym));
    }
    const PROTO: &[FnEntry] = &[("toString", 0, to_string), ("valueOf", 0, value_of)];
    methods(agent, proto, PROTO);
    accessor(
        agent,
        proto,
        PropertyKey::from("description"),
        "description",
        Some(description),
        None,
    );
    let tp = agent.well_known.to_primitive;
    let f = agent.create_builtin_function(to_primitive, 1, "[Symbol.toPrimitive]", false);
    agent.put_data(
        proto,
        PropertyKey::Symbol(tp),
        Value::Object(f),
        false,
        false,
        true,
    );
    to_string_tag(agent, proto, "Symbol");
}

/// Symbol ( [ description ] ) (§20.4.1.1).
fn symbol_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    if a.new_target.is_some() {
        return agent.type_error("Symbol is not a constructor");
    }
    let d = a.arg(0);
    let desc = if d.is_undefined() {
        None
    } else {
        Some(agent.to_string(&d)?)
    };
    Ok(Value::Symbol(agent.new_symbol(desc)))
}

/// Symbol.for (§20.4.2.2).
fn symbol_for(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let key = agent.to_string(&a.arg(0))?;
    Ok(Value::Symbol(agent.symbol_for(key)))
}

/// Symbol.keyFor (§20.4.2.6).
fn key_for(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Value::Symbol(s) = a.arg(0) else {
        return agent.type_error("Symbol.keyFor requires a symbol");
    };
    Ok(agent
        .symbol_key_for(s)
        .map_or(Value::Undefined, Value::String))
}

/// ThisSymbolValue (§20.4.3).
fn this_symbol(agent: &mut Agent, v: &Value) -> JsResult<Gc<Symbol>> {
    match v {
        Value::Symbol(s) => Ok(*s),
        Value::Object(o) => match agent.heap[*o].kind {
            ObjectKind::Symbol(s) => Ok(s),
            _ => agent.type_error("not a Symbol"),
        },
        _ => agent.type_error("not a Symbol"),
    }
}

/// SymbolDescriptiveString (§20.4.3.3.1).
pub fn descriptive_string(agent: &Agent, s: Gc<Symbol>) -> JsString {
    let mut u: Vec<u16> = "Symbol(".encode_utf16().collect();
    if let Some(d) = &agent.heap[s].description {
        u.extend_from_slice(d.units());
    }
    u.push(u16::from(b')'));
    JsString::from_units(u)
}

fn to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_symbol(agent, &a.this)?;
    Ok(Value::String(descriptive_string(agent, s)))
}

fn value_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Symbol(this_symbol(agent, &a.this)?))
}

fn to_primitive(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Symbol(this_symbol(agent, &a.this)?))
}

fn description(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_symbol(agent, &a.this)?;
    Ok(agent.heap[s]
        .description
        .clone()
        .map_or(Value::Undefined, Value::String))
}
