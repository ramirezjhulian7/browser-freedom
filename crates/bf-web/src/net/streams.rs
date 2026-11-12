//! `ReadableStream` (Streams Standard §4, streams por defecto) y su uso como cuerpo de
//! `Request`/`Response` (Fetch §5.1). La implementación está en `streams.js` (evaluado una vez
//! por realm, con los slots internos en WeakMaps de su clausura); aquí están los helpers que
//! usa el body mixin.

use bf_js::builtins::web::serialize::HostRecord;
use bf_js::host::HostHandle;
use bf_js::string::JsString;
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

/// Un stream que llena la red: `(stream, ops)`. `on_cancel` se llama si el autor lo cancela;
/// `ops` se usa con [`push_chunk`], [`push_close`] y [`push_error`].
pub fn pushed(agent: &mut Agent, on_cancel: Value) -> JsResult<(Value, Value)> {
    let arr = call(agent, "pushed", &[on_cancel])?;
    let Value::Object(a) = arr else {
        return agent.type_error("pushed");
    };
    let s = agent.get(a, &PropertyKey::from(0u32), arr.clone())?;
    let ops = agent.get(a, &PropertyKey::from(1u32), arr.clone())?;
    Ok((s, ops))
}

fn op(agent: &mut Agent, ops: &Value, name: &str, args: &[Value]) {
    let Value::Object(o) = ops else {
        return;
    };
    if let Ok(f) = agent.get(*o, &PropertyKey::from(name), ops.clone()) {
        let _ = agent.call(&f, Value::Undefined, args);
    }
}

/// Encola `bytes` (un `Uint8Array`) en un stream de [`pushed`].
pub fn push_chunk(agent: &mut Agent, ops: &Value, bytes: Vec<u8>) {
    let u8 = Value::Object(new_uint8_array(agent, bytes));
    op(agent, ops, "enqueue", &[u8]);
}

/// Cierra un stream de [`pushed`].
pub fn push_close(agent: &mut Agent, ops: &Value) {
    op(agent, ops, "close", &[]);
}

/// Da error a un stream de [`pushed`].
pub fn push_error(agent: &mut Agent, ops: &Value, e: Value) {
    op(agent, ops, "error", &[e]);
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

/// ¿Es `v` un `ReadableStream`, `WritableStream` o `TransformStream` (objetos transferibles,
/// Streams §8)?
pub fn is_transferable(agent: &mut Agent, v: &Value) -> bool {
    matches!(v, Value::Object(_)) && truthy(agent, "isTransferable", v)
}

/// Un stream transferido (Streams §4.2.6, §5.2.6, §6.2.6): su tipo y los puertos por los que
/// habla con el original (uno; dos para un `TransformStream`).
pub struct StreamRecord {
    kind: String,
    ports: Vec<crate::workers::ports::PortRecord>,
}

impl HostRecord for StreamRecord {
    fn deserialize(self: Box<Self>, agent: &mut Agent) -> JsResult<Value> {
        let mut args = vec![Value::String(JsString::from(self.kind.as_str()))];
        for p in self.ports {
            args.push(crate::workers::ports::adopt_port(agent, p));
        }
        call(agent, "receiveStream", &args)
    }
}

/// Los pasos de transferencia de un stream: lo une por un pipe a un `MessagePort` local
/// entrelazado con el que viaja (`DataCloneError` si está bloqueado).
pub fn transfer(agent: &mut Agent, v: &Value) -> JsResult<StreamRecord> {
    let arr = call(agent, "transferStream", std::slice::from_ref(v))?;
    let Value::Object(a) = arr else {
        return agent.type_error("transferStream");
    };
    let kind = match agent.get(a, &PropertyKey::from(0u32), arr.clone())? {
        Value::String(s) => s.to_string_lossy(),
        _ => return agent.type_error("transferStream"),
    };
    let n = if kind == "transform" { 2 } else { 1 };
    let mut ports = Vec::with_capacity(n);
    for i in 1..=n as u32 {
        let port = agent.get(a, &PropertyKey::from(i), arr.clone())?;
        let Some(local) = crate::workers::ports::port_id(agent, &port) else {
            return agent.type_error("transferStream");
        };
        ports.push(crate::workers::ports::transfer_port(agent, local)?);
    }
    Ok(StreamRecord { kind, ports })
}
