//! `ReadableStream` (Streams Standard §4, streams por defecto) y su uso como cuerpo de
//! `Request`/`Response` (Fetch §5.1). La implementación está en `streams.js` (evaluado una vez
//! por realm, con los slots internos en WeakMaps de su clausura); aquí están los helpers que
//! usa el body mixin.

use bf_js::host::HostHandle;
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::{Agent, JsResult, RealmId};

use bf_js::builtins::web::encoding::new_uint8_array;

const SOURCE: &str = include_str!("streams.js");

/// Los helpers de `streams.js` de un realm (fijados).
pub struct StreamHelpers(pub HostHandle);

/// Instala `ReadableStream`, `ReadableStreamDefaultReader` y
/// `ReadableStreamDefaultController` en `realm`.
pub fn install(agent: &mut Agent, realm: RealmId) {
    if agent.realm_data::<StreamHelpers>(realm).is_some() {
        return;
    }
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let f = agent.eval_script(SOURCE, "bf-web:streams.js");
    let g = Value::Object(agent.realms[realm].global_object);
    let helpers = match f {
        Ok(f) => agent.call(&f, Value::Undefined, &[g]).ok(),
        Err(_) => None,
    };
    agent.current_realm = saved;
    if let Some(h) = helpers {
        let pin = agent.host_pin(h);
        agent.set_realm_data(realm, StreamHelpers(pin));
    }
}

fn helper(agent: &mut Agent, name: &str) -> JsResult<Value> {
    let realm = crate::WebContextHandle::current(agent).realm;
    let Some(h) = agent.realm_data::<StreamHelpers>(realm).map(|h| h.0) else {
        return agent.type_error("ReadableStream no está disponible");
    };
    let obj = agent.host_pinned(h).unwrap_or(Value::Undefined);
    let Value::Object(o) = obj else {
        return agent.type_error("ReadableStream no está disponible");
    };
    agent.get(o, &PropertyKey::from(name), obj.clone())
}

fn call(agent: &mut Agent, name: &str, args: &[Value]) -> JsResult<Value> {
    let f = helper(agent, name)?;
    agent.call(&f, Value::Undefined, args)
}

fn truthy(agent: &mut Agent, name: &str, v: &Value) -> bool {
    call(agent, name, std::slice::from_ref(v)).is_ok_and(|r| matches!(r, Value::Boolean(true)))
}

/// ¿Es `v` un `ReadableStream`?
pub fn is_stream(agent: &mut Agent, v: &Value) -> bool {
    matches!(v, Value::Object(_)) && truthy(agent, "isStream", v)
}

/// "Locked" (§4.9.2, IsReadableStreamLocked).
pub fn is_locked(agent: &mut Agent, s: &Value) -> bool {
    truthy(agent, "isLocked", s)
}

/// `[[disturbed]]`.
pub fn is_disturbed(agent: &mut Agent, s: &Value) -> bool {
    truthy(agent, "isDisturbed", s)
}

/// Un stream con `bytes` como único chunk (un `Uint8Array`).
pub fn from_bytes(agent: &mut Agent, bytes: Vec<u8>) -> JsResult<Value> {
    let u8 = Value::Object(new_uint8_array(agent, bytes));
    call(agent, "fromBytes", &[u8])
}

/// ReadableStreamTee (§4.9.1): las dos ramas.
pub fn tee(agent: &mut Agent, s: &Value) -> JsResult<(Value, Value)> {
    let arr = call(agent, "tee", std::slice::from_ref(s))?;
    let Value::Object(a) = arr else {
        return agent.type_error("tee");
    };
    let b1 = agent.get(a, &PropertyKey::from(0u32), arr.clone())?;
    let b2 = agent.get(a, &PropertyKey::from(1u32), arr.clone())?;
    Ok((b1, b2))
}

/// "Fully read" un stream: una promesa con un `Uint8Array`.
pub fn read_all(agent: &mut Agent, s: &Value) -> JsResult<Value> {
    call(agent, "readAll", std::slice::from_ref(s))
}

/// ReadableStreamCancel.
pub fn cancel(agent: &mut Agent, s: &Value, reason: Value) -> JsResult<Value> {
    call(agent, "cancel", &[s.clone(), reason])
}
