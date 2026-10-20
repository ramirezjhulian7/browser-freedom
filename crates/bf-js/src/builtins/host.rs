//! Funciones del host para el shell y el runner de test262: `print` y `$262`
//! (test262 INTERPRETING.md: `createRealm`, `evalScript`, `gc`, `global`,
//! `detachArrayBuffer`).

use crate::builtins::{method, value_prop};
use crate::heap::Gc;
use crate::object::Object;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

/// Instala `print` en el objeto global del realm.
pub fn install_print(agent: &mut Agent, realm: RealmId) {
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let g = agent.realms[realm].global_object;
    method(agent, g, "print", 1, print);
    agent.current_realm = saved;
}

fn print(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let mut parts = Vec::new();
    for v in a.args {
        let s = agent.to_string(v)?;
        parts.push(s.to_string_lossy());
    }
    let line = parts.join(" ");
    agent.print(&line);
    Ok(Value::Undefined)
}

/// Instala `$262` (y `print`) en el realm; devuelve el objeto `$262`.
pub fn install_test262(agent: &mut Agent, realm: RealmId) -> Gc<Object> {
    install_print(agent, realm);
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let g = agent.realms[realm].global_object;
    let o = agent.new_plain_object();
    method(agent, o, "createRealm", 0, create_realm);
    method(agent, o, "evalScript", 1, eval_script);
    method(agent, o, "gc", 0, gc);
    method(agent, o, "detachArrayBuffer", 1, detach_array_buffer);
    value_prop(agent, o, "global", Value::Object(g));
    let op = agent.intrinsics().object_prototype;
    let dda = agent.alloc(Object::new(Some(op), crate::object::ObjectKind::HtmlDda));
    value_prop(agent, o, "IsHTMLDDA", Value::Object(dda));
    agent.put_data(
        g,
        PropertyKey::from("$262"),
        Value::Object(o),
        true,
        false,
        true,
    );
    agent.current_realm = saved;
    o
}

fn create_realm(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let r = agent.create_realm();
    let o = install_test262(agent, r);
    Ok(Value::Object(o))
}

fn eval_script(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = agent.to_string(&a.arg(0))?;
    // El script se evalúa en el realm de esta función ($262 de ese realm).
    agent.eval_script(&s.to_string_lossy(), "<evalScript>")
}

fn gc(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    agent.collect();
    Ok(Value::Undefined)
}

fn detach_array_buffer(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    agent.internal_error("ArrayBuffer (paso 4)")
}
