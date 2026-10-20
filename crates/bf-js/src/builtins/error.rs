//! Error (§20.5), los NativeError (§20.5.5) y AggregateError (§20.5.7).

use crate::builtins::{constructor, method, value_prop};
use crate::heap::Gc;
use crate::object::{Object, ObjectKind, PropertyDescriptor};
use crate::value::Value;
use crate::vm::function::CallArgs;
use crate::vm::realm::Intrinsics;
use crate::vm::{Agent, JsResult, RealmId};

pub fn init(agent: &mut Agent, realm: RealmId) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    let proto = agent.new_object(Some(op));
    let ctor = constructor(agent, "Error", 1, error_ctor, proto);
    value_prop(agent, proto, "name", Value::str("Error"));
    value_prop(agent, proto, "message", Value::str(""));
    method(agent, proto, "toString", 0, error_to_string);
    method(agent, ctor, "isError", 1, is_error);
    {
        let i = &mut agent.realms[realm].intrinsics;
        i.error_ctor = ctor;
        i.error_prototype = proto;
    }
    macro_rules! native {
        ($name:literal, $f:ident, $c:ident, $p:ident) => {{
            let p = agent.new_object(Some(proto));
            let c = constructor(agent, $name, 1, $f, p);
            agent.heap[c].proto = Some(ctor);
            value_prop(agent, p, "name", Value::str($name));
            value_prop(agent, p, "message", Value::str(""));
            let i = &mut agent.realms[realm].intrinsics;
            i.$c = c;
            i.$p = p;
        }};
    }
    native!(
        "EvalError",
        eval_error,
        eval_error_ctor,
        eval_error_prototype
    );
    native!(
        "RangeError",
        range_error,
        range_error_ctor,
        range_error_prototype
    );
    native!(
        "ReferenceError",
        reference_error,
        reference_error_ctor,
        reference_error_prototype
    );
    native!(
        "SyntaxError",
        syntax_error,
        syntax_error_ctor,
        syntax_error_prototype
    );
    native!(
        "TypeError",
        type_error,
        type_error_ctor,
        type_error_prototype
    );
    native!("URIError", uri_error, uri_error_ctor, uri_error_prototype);
    let p = agent.new_object(Some(proto));
    let c = constructor(agent, "AggregateError", 2, aggregate_error, p);
    agent.heap[c].proto = Some(ctor);
    value_prop(agent, p, "name", Value::str("AggregateError"));
    value_prop(agent, p, "message", Value::str(""));
    let i = &mut agent.realms[realm].intrinsics;
    i.aggregate_error_ctor = c;
    i.aggregate_error_prototype = p;
}

/// El cuerpo común de Error y los NativeError (§20.5.1.1, §20.5.6.1.1).
fn make(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    default: fn(&Intrinsics) -> Gc<Object>,
    msg_index: usize,
) -> JsResult<Gc<Object>> {
    let nt = a.new_target.unwrap_or(a.callee);
    let o = agent.ordinary_create_from_constructor(nt, default, ObjectKind::Error)?;
    let message = a.arg(msg_index);
    if !message.is_undefined() {
        let m = agent.to_string(&message)?;
        let k = agent.keys.message.clone();
        agent.create_non_enumerable_data_property_or_throw(o, k, Value::String(m))?;
    }
    // InstallErrorCause (§20.5.8.1).
    if let Value::Object(opts) = a.arg(msg_index + 1) {
        let k = agent.keys.cause.clone();
        if agent.has_property(opts, &k)? {
            let cause = agent.get(opts, &k, Value::Object(opts))?;
            agent.create_non_enumerable_data_property_or_throw(o, k, cause)?;
        }
    }
    agent.attach_stack(o);
    Ok(o)
}

fn error_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Object(make(agent, a, |i| i.error_prototype, 0)?))
}

macro_rules! native_fn {
    ($f:ident, $p:ident) => {
        fn $f(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
            Ok(Value::Object(make(agent, a, |i| i.$p, 0)?))
        }
    };
}

native_fn!(eval_error, eval_error_prototype);
native_fn!(range_error, range_error_prototype);
native_fn!(reference_error, reference_error_prototype);
native_fn!(syntax_error, syntax_error_prototype);
native_fn!(type_error, type_error_prototype);
native_fn!(uri_error, uri_error_prototype);

/// AggregateError ( errors, message [ , options ] ) (§20.5.7.1.1).
fn aggregate_error(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = make(agent, a, |i| i.aggregate_error_prototype, 1)?;
    let list = agent.iterable_to_list(&a.arg(0))?;
    let arr = agent.create_array_from_list(&list)?;
    agent.define_property_or_throw(
        o,
        crate::value::PropertyKey::from("errors"),
        PropertyDescriptor::data(Value::Object(arr), true, false, true),
    )?;
    Ok(Value::Object(o))
}

/// Error.isError ( arg ) (§20.5.2.1): ¿tiene `[[ErrorData]]`?
fn is_error(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(
        matches!(a.arg(0), Value::Object(o) if agent.is_error_object(o)),
    ))
}

/// Error.prototype.toString (§20.5.3.4).
fn error_to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Value::Object(o) = a.this else {
        return agent.type_error("Error.prototype.toString called on non-object");
    };
    let nk = agent.keys.name.clone();
    let name = agent.get(o, &nk, a.this.clone())?;
    let name = if name.is_undefined() {
        crate::string::JsString::from("Error")
    } else {
        agent.to_string(&name)?
    };
    let mk = agent.keys.message.clone();
    let msg = agent.get(o, &mk, a.this.clone())?;
    let msg = if msg.is_undefined() {
        crate::string::JsString::default()
    } else {
        agent.to_string(&msg)?
    };
    if name.is_empty() {
        return Ok(Value::String(msg));
    }
    if msg.is_empty() {
        return Ok(Value::String(name));
    }
    let mut u = name.units().to_vec();
    u.extend(": ".encode_utf16());
    u.extend_from_slice(msg.units());
    Ok(Value::String(crate::string::JsString::from_units(u)))
}
