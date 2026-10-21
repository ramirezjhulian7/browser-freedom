//! `AbortController` y `AbortSignal` (DOM §3) sobre `bf-dom`: el signal es un
//! `EventTarget` no-nodo y "signal abort" dispara `abort` con los listeners de script.

use bf_dom::DomException;
use bf_dom::abort::AbortReason;
use bf_dom::events::EventTarget;
use bf_js::host::{HostClass, HostClassId, TimerHandler};
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, Throw};

use crate::dom::events::{TargetRef, with_script_host, wrap_object_target};
use crate::dom::state::state;
use crate::dom::{classes, exception};
use crate::{DocIndex, with_doc, with_doc_mut};

/// El payload de un `AbortController`.
#[derive(Debug, Clone, Copy)]
pub struct ControllerData {
    pub doc: DocIndex,
    pub signal: EventTarget,
}

pub fn abort_controller_class() -> HostClass {
    HostClass::new("AbortController")
        .constructor(controller_ctor, 0)
        .attribute("signal", Some(controller_signal), None)
        .method("abort", 0, controller_abort)
}

pub fn abort_signal_class(event_target: HostClassId) -> HostClass {
    let c = HostClass::new("AbortSignal")
        .parent(event_target)
        .attribute("aborted", Some(signal_aborted), None)
        .attribute("reason", Some(signal_reason), None)
        .method("throwIfAborted", 0, signal_throw_if_aborted)
        .static_method("abort", 0, static_abort)
        .static_method("timeout", 1, static_timeout)
        .static_method("any", 1, static_any);
    crate::dom::events::with_event_handlers(c, &["abort"])
}

/// Guarda un valor de script como handle opaco para `bf-dom`.
pub fn store_value(agent: &mut Agent, v: Value) -> u64 {
    let h = agent.host_pin(v);
    let st = state(agent);
    st.next_value += 1;
    let id = st.next_value;
    st.values.insert(id, h);
    id
}

/// Un valor guardado con [`store_value`].
pub fn load_value(agent: &mut Agent, id: u64) -> Value {
    let h = state(agent).values.get(&id).copied();
    h.and_then(|h| agent.host_pinned(h)).unwrap_or_default()
}

/// El wrapper del signal `signal` (siempre el mismo).
pub fn wrap_signal(agent: &mut Agent, doc: DocIndex, signal: EventTarget) -> Value {
    let class = classes(agent).abort_signal;
    Value::Object(wrap_object_target(
        agent,
        doc,
        signal,
        class,
        Box::new(TargetRef {
            doc,
            target: signal,
        }),
    ))
}

fn reason_arg(agent: &mut Agent, v: &Value) -> Option<AbortReason> {
    if v.is_undefined() {
        return None;
    }
    Some(AbortReason::Script(store_value(agent, v.clone())))
}

/// `new AbortController()`.
fn controller_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let c = with_doc_mut(agent, 0, |d| d.create_abort_controller());
    let class = classes(agent).abort_controller;
    let nt = a.new_target.expect("new.target");
    let o = agent.host_create_from_new_target(
        nt,
        class,
        None,
        Box::new(ControllerData {
            doc: 0,
            signal: c.signal,
        }),
    )?;
    Ok(Value::Object(o))
}

fn controller(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<ControllerData> {
    match agent.host_unwrap::<ControllerData>(&a.this) {
        Some(c) => Ok(*c),
        None => agent.type_error("Illegal invocation"),
    }
}

fn controller_signal(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let c = controller(agent, a)?;
    Ok(wrap_signal(agent, c.doc, c.signal))
}

/// `abort(reason)`.
fn controller_abort(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let c = controller(agent, a)?;
    // El wrapper del signal existe antes de disparar `abort` (es el target del evento).
    let _ = wrap_signal(agent, c.doc, c.signal);
    let reason = reason_arg(agent, &a.arg(0));
    with_script_host(agent, c.doc, |d, h| d.signal_abort(c.signal, reason, h));
    Ok(Value::Undefined)
}

fn this_signal(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<TargetRef> {
    match agent.host_unwrap::<TargetRef>(&a.this) {
        Some(t) => Ok(*t),
        None => agent.type_error("Illegal invocation"),
    }
}

fn signal_aborted(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_signal(agent, a)?;
    Ok(Value::Boolean(with_doc(agent, s.doc, |d| {
        d.signal_aborted(s.target)
    })))
}

/// La razón como valor (los `DOMException` se crean una vez y se reutilizan).
fn reason_value(agent: &mut Agent, s: TargetRef) -> Value {
    let r = with_doc(agent, s.doc, |d| d.signal_reason(s.target));
    match r {
        None => Value::Undefined,
        Some(AbortReason::Script(id)) => load_value(agent, id),
        Some(AbortReason::Exception(e)) => {
            let EventTarget::Object(id) = s.target else {
                return Value::Undefined;
            };
            let key = (1 << 63) | (u64::from(s.doc) << 32) | (id & 0xffff_ffff);
            if let Some(h) = state(agent).values.get(&key).copied() {
                return agent.host_pinned(h).unwrap_or_default();
            }
            let v = exception::create(agent, e.name(), e.message());
            let h = agent.host_pin(v.clone());
            state(agent).values.insert(key, h);
            v
        }
    }
}

fn signal_reason(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_signal(agent, a)?;
    Ok(reason_value(agent, s))
}

fn signal_throw_if_aborted(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_signal(agent, a)?;
    if with_doc(agent, s.doc, |d| d.signal_aborted(s.target)) {
        let r = reason_value(agent, s);
        return Err(Throw::Value(r));
    }
    Ok(Value::Undefined)
}

/// `AbortSignal.abort(reason)`.
fn static_abort(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let reason = reason_arg(agent, &a.arg(0));
    let s = with_doc_mut(agent, 0, |d| d.aborted_signal(reason));
    Ok(wrap_signal(agent, 0, s))
}

/// `AbortSignal.timeout(milliseconds)`: aborta con `TimeoutError` (tarea de timers).
fn static_timeout(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let ms = agent.to_number(&a.arg(0))?;
    if !ms.is_finite() || !(0.0..=9007199254740991.0).contains(&ms.trunc()) {
        return agent.type_error("Value is outside the 'unsigned long long' value range.");
    }
    let c = with_doc_mut(agent, 0, |d| d.create_abort_controller());
    let signal = wrap_signal(agent, 0, c.signal);
    let EventTarget::Object(id) = c.signal else {
        unreachable!("signal")
    };
    let f = agent.create_builtin_with_slots(timeout_fired, 0, "", vec![Value::Number(id as f64)]);
    let realm = agent.current_realm;
    agent.set_timer(
        realm,
        TimerHandler::Callback(Value::Object(f)),
        ms.trunc(),
        Vec::new(),
        false,
    );
    Ok(signal)
}

fn timeout_fired(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = bf_js::builtins::slots(agent, a.callee)[0]
        .as_number()
        .unwrap_or(0.0) as u64;
    let signal = EventTarget::Object(id);
    let _ = wrap_signal(agent, 0, signal);
    with_script_host(agent, 0, |d, h| {
        d.signal_abort(
            signal,
            Some(AbortReason::Exception(DomException::TimeoutError)),
            h,
        );
    });
    Ok(Value::Undefined)
}

/// `AbortSignal.any(signals)`: aborta en cuanto aborte cualquiera (los listeners de script del
/// signal dependiente corren cuando aborta desde script; desde un abort algorithm nativo solo
/// cambia su estado).
fn static_any(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let signals = bf_js::host::webidl::to_sequence(agent, &a.arg(0), |agent, v| {
        match agent.host_unwrap::<TargetRef>(v).copied() {
            Some(t) if agent.host_is_instance(v, classes(agent).abort_signal) => Ok(t),
            _ => agent.type_error("Failed to execute 'any' on 'AbortSignal': not an AbortSignal"),
        }
    })?;
    let c = with_doc_mut(agent, 0, |d| d.create_abort_controller());
    let result = wrap_signal(agent, 0, c.signal);
    for s in &signals {
        if let Some(r) = with_doc(agent, s.doc, |d| d.signal_reason(s.target)) {
            with_script_host(agent, 0, |d, h| d.signal_abort(c.signal, Some(r), h));
            return Ok(result);
        }
    }
    for s in signals {
        if s.doc != 0 {
            continue;
        }
        let dependent = c.signal;
        let source = s.target;
        with_doc_mut(agent, 0, |d| {
            d.add_abort_algorithm(
                source,
                std::sync::Arc::new(move |d: &mut bf_dom::Document| {
                    let reason = d.signal_reason(source);
                    d.signal_abort(dependent, reason, &mut bf_dom::events::NoScript);
                }),
            );
        });
    }
    Ok(result)
}
