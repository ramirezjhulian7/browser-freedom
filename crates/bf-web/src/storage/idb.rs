//! IndexedDB 3.0: la implementación está en `idb1.js` + `idb2.js` (evaluados una vez por
//! realm); aquí están las funciones nativas que usa: eventos de confianza con el event path de
//! IndexedDB (request → transaction → database), tareas, Blobs y el log en disco
//! ([`super::backend`]).

use std::sync::Arc;

use bf_dom::events::EventTarget;
use bf_js::host::TaskSource;
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::{CallArgs, NativeFn};
use bf_js::vm::{Agent, JsResult, RealmId};

use super::{backend_mut, quota};
use crate::dom::events::{EventData, dispatch_js_event, event_target_of};
use crate::net::blob::{self, BlobData};

const IDB1: &str = include_str!("idb1.js");
const IDB2: &str = include_str!("idb2.js");

/// Evalúa IndexedDB en `realm` con sus funciones nativas.
pub fn install(agent: &mut Agent, realm: RealmId) {
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let native = agent.new_plain_object();
    let fns: [(&str, u32, NativeFn); 14] = [
        ("fire", 2, fire),
        ("setParent", 2, set_parent),
        ("queueTask", 1, queue_task),
        ("isBlob", 1, is_blob),
        ("blobParts", 1, blob_parts),
        ("makeBlob", 4, make_blob),
        ("isHost", 1, is_host),
        ("load", 1, load),
        ("append", 2, append),
        ("compact", 2, compact),
        ("remove", 1, remove),
        ("names", 0, names),
        ("estimate", 0, estimate),
        ("origin", 0, origin),
    ];
    for (name, len, f) in fns {
        bf_js::builtins::method(agent, native, name, len, f);
    }
    let src = format!("{IDB1}\n{IDB2}");
    let g = Value::Object(agent.realms[realm].global_object);
    if let Ok(f) = agent.eval_script(&src, "bf-web:idb.js") {
        let _ = agent.call(&f, Value::Undefined, &[g, Value::Object(native)]);
    }
    agent.current_realm = saved;
}

fn str_arg(agent: &mut Agent, v: &Value) -> JsResult<String> {
    Ok(agent.to_string(v)?.to_string())
}

/// `fire(target, event)`: despacha `event` como evento de confianza en `target`; devuelve
/// `1` si se canceló y `2` si algún listener lanzó (y luego corren los microtasks, como al
/// volver de un callback).
fn fire(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let target = a.arg(0);
    let ev = a.arg(1);
    let Some((doc, t)) = event_target_of(agent, &target) else {
        return agent.type_error("fire: not an EventTarget");
    };
    let Value::Object(evo) = ev else {
        return agent.type_error("fire: not an Event");
    };
    let Some(cell) = agent
        .host_unwrap::<EventData>(&a.arg(1))
        .map(|d| d.cell.clone())
    else {
        return agent.type_error("fire: not an Event");
    };
    {
        let mut e = cell.borrow_mut();
        e.is_trusted = true;
        e.listeners_threw = false;
    }
    let pin = agent.host_pin(Value::Object(evo));
    let r = dispatch_js_event(agent, doc, t, evo, false);
    agent.host_unpin(pin);
    let not_canceled = matches!(r, Ok(Ok(true)));
    let threw = cell.borrow().listeners_threw;
    let _ = agent.perform_microtask_checkpoint();
    Ok(Value::Number(
        f64::from(u8::from(!not_canceled)) + if threw { 2.0 } else { 0.0 },
    ))
}

/// `setParent(child, parent)`: el padre de `child` en el event path (`null` lo quita).
fn set_parent(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some((doc, child)) = event_target_of(agent, &a.arg(0)) else {
        return Ok(Value::Undefined);
    };
    let parent: Option<EventTarget> = event_target_of(agent, &a.arg(1)).map(|(_, t)| t);
    crate::with_doc_mut(agent, doc, |d| d.set_object_parent(child, parent));
    Ok(Value::Undefined)
}

/// `queueTask(f)`: una tarea (la "database access task source").
fn queue_task(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    agent.queue_callback_task(TaskSource::Other, a.arg(0), Value::Undefined, Vec::new());
    Ok(Value::Undefined)
}

fn is_blob(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(blob::data(agent, &a.arg(0)).is_some()))
}

/// `[bytes, type, name | null, lastModified]` de un `Blob`/`File`.
fn blob_parts(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(d) = blob::data(agent, &a.arg(0)) else {
        return Ok(Value::Null);
    };
    let bytes = Value::Object(bf_js::builtins::web::encoding::new_uint8_array(
        agent,
        d.bytes.to_vec(),
    ));
    let (name, last) = match &d.file {
        Some((n, l)) => (Value::str(n), Value::Number(*l)),
        None => (Value::Null, Value::Number(0.0)),
    };
    let arr = agent.create_array_from_list(&[bytes, Value::str(&d.mime), name, last])?;
    Ok(Value::Object(arr))
}

/// `makeBlob(bytes, type, name, lastModified)`: un `Blob` (o un `File` si hay nombre).
fn make_blob(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let bytes =
        bf_js::builtins::web::encoding::buffer_source_bytes(agent, &a.arg(0)).unwrap_or_default();
    let mime = str_arg(agent, &a.arg(1))?;
    let file = if a.arg(2).is_null() {
        None
    } else {
        let n = str_arg(agent, &a.arg(2))?;
        let l = agent.to_number(&a.arg(3))?;
        Some((n, l))
    };
    Ok(blob::create(
        agent,
        BlobData {
            bytes: Arc::new(bytes),
            mime,
            file,
        },
    ))
}

/// ¿Es un objeto de plataforma (que no se puede serializar)?
fn is_host(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(agent.host_class_of(&a.arg(0)).is_some()))
}

fn load(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = str_arg(agent, &a.arg(0))?;
    Ok(backend_mut(agent)
        .load(&name)
        .map_or(Value::Null, |s| Value::str(&s)))
}

fn append(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = str_arg(agent, &a.arg(0))?;
    let text = str_arg(agent, &a.arg(1))?;
    Ok(Value::Boolean(
        backend_mut(agent).append(&name, &text).is_ok(),
    ))
}

fn compact(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = str_arg(agent, &a.arg(0))?;
    let text = str_arg(agent, &a.arg(1))?;
    Ok(Value::Boolean(
        backend_mut(agent).compact(&name, &text).is_ok(),
    ))
}

fn remove(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = str_arg(agent, &a.arg(0))?;
    backend_mut(agent).remove(&name);
    Ok(Value::Undefined)
}

fn names(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let v: Vec<Value> = backend_mut(agent)
        .names()
        .iter()
        .map(|n| Value::str(n))
        .collect();
    Ok(Value::Object(agent.create_array_from_list(&v)?))
}

/// `[usage, quota]` del origen (Storage Standard §4.4).
fn estimate(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let usage = backend_mut(agent).usage() as f64;
    let arr = agent.create_array_from_list(&[Value::Number(usage), Value::Number(quota())])?;
    Ok(Value::Object(arr))
}

fn origin(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&crate::net::document_origin(agent)))
}

/// Para los tests: `indexedDB` de `realm` (evita warnings de import sin usar).
pub fn has_indexed_db(agent: &mut Agent, realm: RealmId) -> bool {
    let g = agent.realms[realm].global_object;
    agent
        .get(g, &PropertyKey::from("indexedDB"), Value::Object(g))
        .is_ok_and(|v| matches!(v, Value::Object(_)))
}
