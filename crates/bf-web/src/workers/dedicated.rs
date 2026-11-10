//! `Worker` (HTML §10.2.6): el objeto del dueño de un worker dedicado. El constructor
//! resuelve la URL, crea el objeto y arranca el hilo ("run a worker"); `postMessage` serializa
//! y deja el mensaje en la bandeja del worker (que lo encola aunque el script todavía no haya
//! cargado: la cola del puerto implícito); `terminate()` corta el hilo. Los eventos del worker
//! (`message`, `messageerror`, `error`) llegan a la bandeja del dueño y se disparan aquí.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use bf_dom::events::{Event, EventInit, EventTarget};
use bf_infra::Url;
use bf_js::builtins::web::serialize;
use bf_js::host::agents::AgentSignal;
use bf_js::host::{HostClass, HostClassId};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use super::scope::{ScopeKind, ScriptType, WorkerSpec};
use super::{AgentInbox, Envelope, WorkerControl, WorkerEvent, WorkerRecord, host};
use crate::WebContextHandle;
use crate::dom::events::{
    EventIface, TargetRef, create_event_object, dispatch_js_event, with_event_handlers,
};

pub fn worker_class(event_target: HostClassId) -> HostClass {
    with_event_handlers(
        HostClass::new("Worker")
            .parent(event_target)
            .constructor(worker_ctor, 1)
            .method("postMessage", 1, worker_post_message)
            .method("terminate", 0, worker_terminate),
        &["message", "messageerror", "error"],
    )
}

/// Las opciones de `new Worker` y `new SharedWorker` (`WorkerOptions`, §10.2.6.3).
pub(crate) struct WorkerOptions {
    pub script_type: ScriptType,
    pub name: String,
    /// `credentials` (por defecto `same-origin`).
    pub credentials: String,
}

/// Lee un `WorkerOptions` (o, para `SharedWorker`, también un nombre suelto).
pub(crate) fn worker_options(
    agent: &mut Agent,
    v: &Value,
    allow_name: bool,
) -> JsResult<WorkerOptions> {
    let mut out = WorkerOptions {
        script_type: ScriptType::Classic,
        name: String::new(),
        credentials: "same-origin".into(),
    };
    match v {
        Value::Undefined | Value::Null => {}
        Value::Object(o) => {
            let o = *o;
            // Los miembros del diccionario en orden alfabético: credentials, name, type.
            let c = agent.get(o, &PropertyKey::from("credentials"), v.clone())?;
            if !c.is_undefined() {
                let c = agent.to_string(&c)?.to_string_lossy();
                if !matches!(c.as_str(), "omit" | "same-origin" | "include") {
                    return agent.type_error(&format!(
                        "Failed to read the 'credentials' property from 'WorkerOptions': The provided value '{c}' is not a valid enum value of type RequestCredentials."
                    ));
                }
                out.credentials = c;
            }
            let n = agent.get(o, &PropertyKey::from("name"), v.clone())?;
            if !n.is_undefined() {
                out.name = agent.to_string(&n)?.to_string_lossy();
            }
            let t = agent.get(o, &PropertyKey::from("type"), v.clone())?;
            if !t.is_undefined() {
                out.script_type = match agent.to_string(&t)?.to_string_lossy().as_str() {
                    "classic" => ScriptType::Classic,
                    "module" => ScriptType::Module,
                    other => {
                        return agent.type_error(&format!(
                            "Failed to read the 'type' property from 'WorkerOptions': The provided value '{other}' is not a valid enum value of type WorkerType."
                        ));
                    }
                };
            }
        }
        other if allow_name => out.name = agent.to_string(other)?.to_string_lossy(),
        _ => return agent.type_error("The provided value is not of type 'WorkerOptions'."),
    }
    Ok(out)
}

/// La URL de un script de worker, relativa a la URL base del dueño (SyntaxError si no
/// parsea).
pub(crate) fn script_url(agent: &mut Agent, v: &Value, what: &str) -> JsResult<Url> {
    let s = agent.to_string(v)?.to_string_lossy();
    let base = WebContextHandle::current(agent).get(agent).base.clone();
    match Url::parse(&s, Some(&base)) {
        Ok(u) => Ok(u),
        Err(_) => crate::net::dom_error(
            agent,
            "SyntaxError",
            &format!("Failed to construct '{what}': The URL '{s}' is invalid."),
        ),
    }
}

/// ¿Está aislado el agent cluster del dueño (`crossOriginIsolated`)? Los `SharedArrayBuffer`
/// solo cruzan entre agentes aislados.
pub(crate) fn cross_origin_isolated(agent: &Agent) -> bool {
    match agent
        .embedder_data::<super::WorkerHost>()
        .and_then(|h| h.scope.as_ref())
    {
        Some(s) => s.cross_origin_isolated,
        // Un marco está en el grupo de contextos de navegación de la ventana de arriba.
        None => {
            let realm = WebContextHandle::current(agent).realm;
            let top = crate::bom::frames::top_of(agent, realm);
            agent
                .realm_data::<super::Isolation>(realm)
                .or_else(|| agent.realm_data::<super::Isolation>(top))
                .is_some_and(|i| i.0)
        }
    }
}

/// La política de inserción (COEP) del global en curso: la del worker, o la del documento
/// (`workers::isolation::set_document_policies`; sin ella, `unsafe-none`).
pub(crate) fn embedder_policy(agent: &Agent) -> super::isolation::EmbedderPolicy {
    match agent
        .embedder_data::<super::WorkerHost>()
        .and_then(|h| h.scope.as_ref())
    {
        Some(s) => s.coep,
        None => agent
            .realm_data::<super::isolation::DocumentEmbedderPolicy>(
                WebContextHandle::current(agent).realm,
            )
            .map(|p| p.0)
            .unwrap_or_default(),
    }
}

/// `new Worker(scriptURL, options)`.
fn worker_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Failed to construct 'Worker': Please use the 'new' operator.");
    };
    if a.args.is_empty() {
        return agent
            .type_error("Failed to construct 'Worker': 1 argument required, but only 0 present.");
    }
    let url = script_url(agent, &a.arg(0), "Worker")?;
    let options = worker_options(agent, &a.arg(1), false)?;
    let class = super::classes(agent).worker;
    let (t, o) = crate::net::xhr::new_target(agent, class, Some(nt))?;
    let id = match t.target {
        EventTarget::Object(id) => id,
        _ => 0,
    };
    let realm = WebContextHandle::current(agent).realm;
    let pin = agent.host_pin(Value::Object(o));
    let worker_inbox = Arc::new(AgentInbox::new(Arc::new(AgentSignal::default())));
    let control = Arc::new(WorkerControl::default());
    let parent = super::inbox(agent);
    host(agent).workers.insert(
        id,
        WorkerRecord {
            realm,
            inbox: worker_inbox.clone(),
            control: control.clone(),
            pin: Some(pin),
        },
    );
    super::ensure_polling(agent, realm);
    let spec = WorkerSpec {
        url,
        script_type: options.script_type,
        name: options.name,
        kind: ScopeKind::Dedicated,
        backend: crate::net::current_backend(agent),
        parent: Some(parent.clone()),
        worker_id: id,
        inbox: worker_inbox,
        control,
        cross_origin_isolated: cross_origin_isolated(agent),
        owner_coep: embedder_policy(agent),
        user_agent: agent.host.hooks.user_agent(),
        partition: super::partition(agent),
    };
    if super::scope::spawn(spec).is_err() {
        parent.push(Envelope::FromWorker {
            worker: id,
            event: WorkerEvent::LoadError,
        });
    }
    Ok(Value::Object(o))
}

fn this_worker(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<u64> {
    let class = super::classes(agent).worker;
    match agent.host_unwrap::<TargetRef>(&a.this).copied() {
        Some(TargetRef {
            target: EventTarget::Object(id),
            ..
        }) if agent.host_is_instance(&a.this, class) => Ok(id),
        _ => agent.type_error("Illegal invocation"),
    }
}

/// `postMessage(message, transfer)` (§10.2.6.3): al worker, aunque todavía no haya cargado.
fn worker_post_message(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_worker(agent, a)?;
    let transfer = super::ports::transfer_argument(agent, &a.arg(1))?;
    let isolated = cross_origin_isolated(agent);
    let s = super::ports::serialize(agent, &a.arg(0), &transfer, isolated)?;
    if let Some(w) = host(agent).workers.get(&id)
        && !w.control.terminated.load(Ordering::SeqCst)
    {
        w.inbox.push(Envelope::ToScope(s));
    }
    Ok(Value::Undefined)
}

/// `terminate()` (§10.2.6.3): el worker termina en cuanto puede; no llegan más eventos.
fn worker_terminate(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_worker(agent, a)?;
    if let Some(w) = host(agent).workers.remove(&id) {
        terminate(&w);
        if let Some(p) = w.pin {
            agent.host_unpin(p);
        }
    }
    Ok(Value::Undefined)
}

pub(crate) fn terminate(w: &WorkerRecord) {
    w.control.terminated.store(true, Ordering::SeqCst);
    w.control.interrupt.store(true, Ordering::SeqCst);
    w.inbox.signal().signal();
}

/// Un evento del worker `id` (en una tarea *posted message* del dueño).
pub(crate) fn deliver_event(agent: &mut Agent, id: u64, event: WorkerEvent) {
    // Un `SharedWorker` cuyo script no cargó (o que no se pudo conectar).
    if matches!(event, WorkerEvent::LoadError) && !host(agent).workers.contains_key(&id) {
        let realm = host(agent).shared_errors.remove(&id);
        let known = realm.is_some() || host(agent).shared.contains_key(&id);
        if known {
            let saved = agent.current_realm;
            if let Some(r) = realm {
                agent.current_realm = r;
            }
            let t = TargetRef {
                doc: 0,
                target: EventTarget::Object(id),
            };
            fire_simple(agent, t, "error", false);
            agent.current_realm = saved;
        }
        return;
    }
    let Some(realm) = host(agent).workers.get(&id).map(|w| w.realm) else {
        // Terminado: solo la consola sigue llegando.
        if let WorkerEvent::Console(level, msg) = event {
            agent.host.hooks.console(level, &msg);
        }
        return;
    };
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let t = TargetRef {
        doc: 0,
        target: EventTarget::Object(id),
    };
    match event {
        WorkerEvent::Message(s) => match serialize::deserialize_with_transfer(agent, s) {
            Ok((data, transferred)) => {
                let ports = super::ports::ports_of(agent, &transferred);
                crate::net::channels::fire_message(agent, t, "message", data, "", "", ports);
            }
            Err(_) => fire_simple_message_error(agent, t),
        },
        WorkerEvent::MessageError => fire_simple_message_error(agent, t),
        WorkerEvent::Error {
            message,
            filename,
            lineno,
            colno,
        } => {
            let not_canceled =
                fire_error_event(agent, t, &message, &filename, lineno, colno, Value::Null);
            // Sin cancelar: "report the error" en el global del dueño (`onerror` de la
            // ventana, o del worker que lo creó, que a su vez lo pasa a su dueño).
            if not_canceled {
                let g = TargetRef {
                    doc: 0,
                    target: EventTarget::Window,
                };
                if fire_error_event(agent, g, &message, &filename, lineno, colno, Value::Null) {
                    agent.host.hooks.report_exception(&message, "");
                }
            }
        }
        WorkerEvent::LoadError => {
            fire_simple(agent, t, "error", true);
        }
        WorkerEvent::Console(level, msg) => agent.host.hooks.console(level, &msg),
        WorkerEvent::Closed => {
            if let Some(w) = host(agent).workers.remove(&id)
                && let Some(p) = w.pin
            {
                agent.host_unpin(p);
            }
        }
    }
    agent.current_realm = saved;
}

fn fire_simple_message_error(agent: &mut Agent, t: TargetRef) {
    crate::net::channels::fire_message(agent, t, "messageerror", Value::Null, "", "", Vec::new());
}

/// Dispara un `Event` de confianza `type_` en `t`.
pub(crate) fn fire_simple(agent: &mut Agent, t: TargetRef, type_: &str, cancelable: bool) -> bool {
    let cell = Rc::new(RefCell::new(Event::new(
        type_,
        EventInit {
            bubbles: false,
            cancelable,
            composed: false,
        },
    )));
    cell.borrow_mut().is_trusted = true;
    let o = create_event_object(agent, EventIface::Event, cell, t.doc);
    let pin = agent.host_pin(Value::Object(o));
    let r = dispatch_js_event(agent, t.doc, t.target, o, false);
    agent.host_unpin(pin);
    matches!(r, Ok(Ok(true)))
}

/// Dispara un `ErrorEvent` `error` cancelable en `t`; `true` si nadie lo canceló.
pub(crate) fn fire_error_event(
    agent: &mut Agent,
    t: TargetRef,
    message: &str,
    filename: &str,
    lineno: u32,
    colno: u32,
    error: Value,
) -> bool {
    let cell = Rc::new(RefCell::new(Event::new(
        "error",
        EventInit {
            bubbles: false,
            cancelable: true,
            composed: false,
        },
    )));
    cell.borrow_mut().is_trusted = true;
    let iface = EventIface::Error {
        message: message.into(),
        filename: filename.into(),
        lineno,
        colno,
    };
    let o = create_event_object(agent, iface, cell, t.doc);
    if let Some(s) = agent.host_slots_mut(o) {
        s[0] = error;
    }
    let pin = agent.host_pin(Value::Object(o));
    let r = dispatch_js_event(agent, t.doc, t.target, o, false);
    agent.host_unpin(pin);
    matches!(r, Ok(Ok(true)))
}
