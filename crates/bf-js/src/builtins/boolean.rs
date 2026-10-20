//! Boolean (§20.3).

use crate::abstract_ops::conversion::to_boolean;
use crate::builtins::{FnEntry, constructor, methods};
use crate::object::{Object, ObjectKind};
use crate::value::Value;
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

pub fn init(agent: &mut Agent, realm: RealmId) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    let proto = agent.alloc(Object::new(Some(op), ObjectKind::Boolean(false)));
    agent.realms[realm].intrinsics.boolean_prototype = proto;
    let ctor = constructor(agent, "Boolean", 1, boolean_ctor, proto);
    agent.realms[realm].intrinsics.boolean_ctor = ctor;
    const PROTO: &[FnEntry] = &[("toString", 0, to_string), ("valueOf", 0, value_of)];
    methods(agent, proto, PROTO);
}

/// Boolean ( value ) (§20.3.1.1).
fn boolean_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let b = to_boolean(agent, &a.arg(0));
    let Some(nt) = a.new_target else {
        return Ok(Value::Boolean(b));
    };
    let o = agent.ordinary_create_from_constructor(
        nt,
        |i| i.boolean_prototype,
        ObjectKind::Boolean(b),
    )?;
    Ok(Value::Object(o))
}

/// ThisBooleanValue (§20.3.3.3.1).
fn this_boolean(agent: &mut Agent, v: &Value) -> JsResult<bool> {
    match v {
        Value::Boolean(b) => Ok(*b),
        Value::Object(o) => match agent.heap[*o].kind {
            ObjectKind::Boolean(b) => Ok(b),
            _ => agent.type_error("Boolean.prototype method called on incompatible receiver"),
        },
        _ => agent.type_error("Boolean.prototype method called on incompatible receiver"),
    }
}

fn to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let b = this_boolean(agent, &a.this)?;
    Ok(Value::str(if b { "true" } else { "false" }))
}

fn value_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(this_boolean(agent, &a.this)?))
}
