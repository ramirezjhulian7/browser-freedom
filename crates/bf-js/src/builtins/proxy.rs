//! El constructor Proxy (§28.2) y `Proxy.revocable`; el objeto exótico está en
//! [`crate::object::proxy`].

use crate::builtins::method;
use crate::object::ObjectKind;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

pub fn init(agent: &mut Agent, realm: RealmId) {
    // Proxy no tiene `prototype` (§28.2.2).
    let ctor = agent.create_builtin_function(proxy_ctor, 2, "Proxy", true);
    method(agent, ctor, "revocable", 2, revocable);
    crate::builtins::register_global(agent, realm, "Proxy", ctor);
}

/// Proxy ( target, handler ) (§28.2.1.1).
fn proxy_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    if a.new_target.is_none() {
        return agent.type_error("Constructor Proxy requires 'new'");
    }
    Ok(Value::Object(agent.proxy_create(&a.arg(0), &a.arg(1))?))
}

/// Proxy.revocable ( target, handler ) (§28.2.2.1).
fn revocable(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let p = agent.proxy_create(&a.arg(0), &a.arg(1))?;
    let revoker = agent.create_builtin_with_slots(revoke, 0, "", vec![Value::Object(p)]);
    let result = agent.new_plain_object();
    agent.create_data_property_or_throw(result, PropertyKey::from("proxy"), Value::Object(p))?;
    agent.create_data_property_or_throw(
        result,
        PropertyKey::from("revoke"),
        Value::Object(revoker),
    )?;
    Ok(Value::Object(result))
}

/// Las funciones de revocación (§28.2.2.1.1).
fn revoke(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let p = match &mut agent.heap[a.callee].kind {
        ObjectKind::Builtin(b) => {
            let v = b.slots.first().cloned();
            if let Some(slot) = b.slots.first_mut() {
                *slot = Value::Null;
            }
            v
        }
        _ => None,
    };
    if let Some(Value::Object(p)) = p
        && let ObjectKind::Proxy(d) = &mut agent.heap[p].kind
    {
        d.target = None;
        d.handler = None;
    }
    Ok(Value::Undefined)
}
