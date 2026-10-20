//! Number (§21.1): el constructor y Number.prototype.{valueOf, toString}; el resto llega con
//! los built-ins del paso 3.

use crate::abstract_ops::number::{number_to_string, number_to_string_radix};
use crate::builtins::{FnEntry, constructor, methods};
use crate::object::{Object, ObjectKind};
use crate::value::Value;
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

pub fn init(agent: &mut Agent, realm: RealmId) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    let proto = agent.alloc(Object::new(Some(op), ObjectKind::Number(0.0)));
    agent.realms[realm].intrinsics.number_prototype = proto;
    let ctor = constructor(agent, "Number", 1, number_ctor, proto);
    agent.realms[realm].intrinsics.number_ctor = ctor;
    const PROTO: &[FnEntry] = &[("toString", 1, to_string), ("valueOf", 0, value_of)];
    methods(agent, proto, PROTO);
}

/// Number ( value ) (§21.1.1.1).
fn number_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = if a.is_empty() {
        0.0
    } else {
        match agent.to_numeric(&a.arg(0))? {
            Value::BigInt(b) => agent.heap[b].to_f64(),
            Value::Number(n) => n,
            _ => unreachable!("ToNumeric"),
        }
    };
    let Some(nt) = a.new_target else {
        return Ok(Value::Number(n));
    };
    let o = agent.ordinary_create_from_constructor(
        nt,
        |i| i.number_prototype,
        ObjectKind::Number(n),
    )?;
    Ok(Value::Object(o))
}

/// ThisNumberValue (§21.1.3.7.1).
fn this_number(agent: &mut Agent, v: &Value) -> JsResult<f64> {
    match v {
        Value::Number(n) => Ok(*n),
        Value::Object(o) => match agent.heap[*o].kind {
            ObjectKind::Number(n) => Ok(n),
            _ => agent.type_error("Number.prototype method called on incompatible receiver"),
        },
        _ => agent.type_error("Number.prototype method called on incompatible receiver"),
    }
}

/// Number.prototype.toString ( [ radix ] ) (§21.1.3.6).
fn to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let x = this_number(agent, &a.this)?;
    let radix = if a.arg(0).is_undefined() {
        10.0
    } else {
        agent.to_integer_or_infinity(&a.arg(0))?
    };
    if !(2.0..=36.0).contains(&radix) {
        return agent.range_error("toString() radix must be between 2 and 36");
    }
    if radix == 10.0 {
        return Ok(Value::String(number_to_string(x)));
    }
    Ok(Value::str(&number_to_string_radix(x, radix as u32)))
}

fn value_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(this_number(agent, &a.this)?))
}
