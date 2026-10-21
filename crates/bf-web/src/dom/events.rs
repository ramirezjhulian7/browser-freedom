//! Eventos (DOM §2, UI Events, HTML §8.1.8): `EventTarget`, `Event` y sus subclases
//! (`CustomEvent`, `UIEvent`, `FocusEvent`, `MouseEvent`, `KeyboardEvent`, `InputEvent`,
//! `ErrorEvent`, `PromiseRejectionEvent`), el puente con el dispatch de `bf-dom` y los event
//! handlers (`onclick`…), incluidos los atributos de contenido compilados perezosamente.
//!
//! El dispatch lo hace `bf-dom` (§2.9). Para llamar a JavaScript desde un listener, el
//! documento se saca del [`crate::WebContext`] durante el dispatch y se devuelve mientras
//! corre cada listener; el `Event` de Rust se intercambia del mismo modo con la celda de su
//! wrapper, así el script ve el estado vigente (`currentTarget`, `eventPhase`…) y sus
//! `preventDefault()` llegan al dispatch.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use bf_dom::Document;
use bf_dom::events::{
    Event, EventInit, EventKind, EventTarget, InputEventData, KeyboardEventData, Listener,
    ListenerOptions, MouseEventData, ScriptHost,
};
use bf_infra::DomString;
use bf_js::heap::Gc;
use bf_js::host::{HostClass, HostClassId, HostHandle, HostKey};
use bf_js::object::{Object, PropValue};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, Throw};

use crate::dom::state::{HandlerState, ListenerRec, state};
use crate::dom::window::WindowData;
use crate::dom::{
    classes, from_dom_string, from_opt_dom_string, throw_dom, to_dom_string, unwrap_node, wrap_node,
};
use crate::{DocIndex, NodeRef, WebContextHandle};

/// La interfaz de un evento creado por script (o del wrapper de uno nativo).
#[derive(Debug, Clone, PartialEq)]
pub enum EventIface {
    Event,
    Custom,
    Ui,
    Focus,
    Mouse,
    Keyboard,
    Input,
    Error {
        message: DomString,
        filename: DomString,
        lineno: u32,
        colno: u32,
    },
    PromiseRejection,
}

/// El payload de un wrapper de `Event`. Los valores de script (`detail`, `view`, `error`,
/// `promise`, `reason`) van en los `slots` del objeto.
pub struct EventData {
    pub cell: Rc<RefCell<Event>>,
    pub iface: EventIface,
    /// El documento en el que se despachó (para traducir los targets).
    pub doc: Cell<DocIndex>,
}

/// El payload de un `EventTarget` que no es nodo ni ventana (`new EventTarget()`,
/// `AbortSignal`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetRef {
    pub doc: DocIndex,
    pub target: EventTarget,
}

/// La clave de identidad del wrapper de un `EventTarget::Object`.
pub fn object_key(doc: DocIndex, id: u64) -> HostKey {
    HostKey((1 << 63) | (u64::from(doc) << 32) | (id & 0xffff_ffff))
}

// --- Clases ---

pub fn event_target_class() -> HostClass {
    HostClass::new("EventTarget")
        .constructor(event_target_ctor, 0)
        .method("addEventListener", 2, add_event_listener)
        .method("removeEventListener", 2, remove_event_listener)
        .method("dispatchEvent", 1, dispatch_event)
}

pub fn event_class() -> HostClass {
    HostClass::new("Event")
        .constructor(event_ctor, 1)
        .attribute("type", Some(ev_type), None)
        .attribute("target", Some(ev_target), None)
        .attribute("srcElement", Some(ev_target), None)
        .attribute("currentTarget", Some(ev_current_target), None)
        .attribute("eventPhase", Some(ev_phase), None)
        .attribute(
            "cancelBubble",
            Some(ev_cancel_bubble),
            Some(ev_set_cancel_bubble),
        )
        .attribute("bubbles", Some(ev_bubbles), None)
        .attribute("cancelable", Some(ev_cancelable), None)
        .attribute(
            "returnValue",
            Some(ev_return_value),
            Some(ev_set_return_value),
        )
        .attribute("defaultPrevented", Some(ev_default_prevented), None)
        .attribute("composed", Some(ev_composed), None)
        .unforgeable("isTrusted", Some(ev_is_trusted), None)
        .attribute("timeStamp", Some(ev_time_stamp), None)
        .method("composedPath", 0, ev_composed_path)
        .method("stopPropagation", 0, ev_stop_propagation)
        .method("stopImmediatePropagation", 0, ev_stop_immediate_propagation)
        .method("preventDefault", 0, ev_prevent_default)
        .method("initEvent", 1, ev_init_event)
        .constant("NONE", 0.0)
        .constant("CAPTURING_PHASE", 1.0)
        .constant("AT_TARGET", 2.0)
        .constant("BUBBLING_PHASE", 3.0)
}

pub fn custom_event_class(event: HostClassId) -> HostClass {
    HostClass::new("CustomEvent")
        .parent(event)
        .constructor(custom_event_ctor, 1)
        .attribute("detail", Some(slot0), None)
        .method("initCustomEvent", 1, init_custom_event)
}

pub fn ui_event_class(event: HostClassId) -> HostClass {
    HostClass::new("UIEvent")
        .parent(event)
        .constructor(ui_event_ctor, 1)
        .attribute("view", Some(slot0), None)
        .attribute("detail", Some(ui_detail), None)
        .attribute("which", Some(ui_which), None)
        .method("initUIEvent", 1, init_ui_event)
}

pub fn focus_event_class(ui: HostClassId) -> HostClass {
    HostClass::new("FocusEvent")
        .parent(ui)
        .constructor(focus_event_ctor, 1)
        .attribute("relatedTarget", Some(focus_related), None)
}

pub fn mouse_event_class(ui: HostClassId) -> HostClass {
    HostClass::new("MouseEvent")
        .parent(ui)
        .constructor(mouse_event_ctor, 1)
        .attribute("screenX", Some(mouse_screen_x), None)
        .attribute("screenY", Some(mouse_screen_y), None)
        .attribute("clientX", Some(mouse_client_x), None)
        .attribute("clientY", Some(mouse_client_y), None)
        .attribute("x", Some(mouse_client_x), None)
        .attribute("y", Some(mouse_client_y), None)
        .attribute("pageX", Some(mouse_client_x), None)
        .attribute("pageY", Some(mouse_client_y), None)
        .attribute("offsetX", Some(mouse_client_x), None)
        .attribute("offsetY", Some(mouse_client_y), None)
        .attribute("ctrlKey", Some(mod_ctrl), None)
        .attribute("shiftKey", Some(mod_shift), None)
        .attribute("altKey", Some(mod_alt), None)
        .attribute("metaKey", Some(mod_meta), None)
        .attribute("button", Some(mouse_button), None)
        .attribute("buttons", Some(mouse_buttons), None)
        .attribute("relatedTarget", Some(mouse_related), None)
        .method("getModifierState", 1, get_modifier_state)
}

pub fn keyboard_event_class(ui: HostClassId) -> HostClass {
    HostClass::new("KeyboardEvent")
        .parent(ui)
        .constructor(keyboard_event_ctor, 1)
        .attribute("key", Some(kb_key), None)
        .attribute("code", Some(kb_code), None)
        .attribute("location", Some(kb_location), None)
        .attribute("ctrlKey", Some(mod_ctrl), None)
        .attribute("shiftKey", Some(mod_shift), None)
        .attribute("altKey", Some(mod_alt), None)
        .attribute("metaKey", Some(mod_meta), None)
        .attribute("repeat", Some(kb_repeat), None)
        .attribute("isComposing", Some(kb_is_composing), None)
        .attribute("charCode", Some(kb_char_code), None)
        .attribute("keyCode", Some(kb_key_code), None)
        .method("getModifierState", 1, get_modifier_state)
        .constant("DOM_KEY_LOCATION_STANDARD", 0.0)
        .constant("DOM_KEY_LOCATION_LEFT", 1.0)
        .constant("DOM_KEY_LOCATION_RIGHT", 2.0)
        .constant("DOM_KEY_LOCATION_NUMPAD", 3.0)
}

pub fn input_event_class(ui: HostClassId) -> HostClass {
    HostClass::new("InputEvent")
        .parent(ui)
        .constructor(input_event_ctor, 1)
        .attribute("data", Some(input_data), None)
        .attribute("isComposing", Some(input_is_composing), None)
        .attribute("inputType", Some(input_type), None)
}

pub fn error_event_class(event: HostClassId) -> HostClass {
    HostClass::new("ErrorEvent")
        .parent(event)
        .constructor(error_event_ctor, 1)
        .attribute("message", Some(err_message), None)
        .attribute("filename", Some(err_filename), None)
        .attribute("lineno", Some(err_lineno), None)
        .attribute("colno", Some(err_colno), None)
        .attribute("error", Some(slot0), None)
}

pub fn promise_rejection_event_class(event: HostClassId) -> HostClass {
    HostClass::new("PromiseRejectionEvent")
        .parent(event)
        .constructor(promise_rejection_event_ctor, 2)
        .attribute("promise", Some(slot0), None)
        .attribute("reason", Some(slot1), None)
}

// --- EventTarget ---

/// El target de un wrapper: nodos, la ventana y los demás `EventTarget`.
pub fn event_target_of(agent: &Agent, v: &Value) -> Option<(DocIndex, EventTarget)> {
    if let Some(n) = unwrap_node(agent, v) {
        return Some((n.doc, EventTarget::Node(n.node)));
    }
    if agent.host_unwrap::<WindowData>(v).is_some() {
        return Some((0, EventTarget::Window));
    }
    agent.host_unwrap::<TargetRef>(v).map(|t| (t.doc, t.target))
}

/// El wrapper de un target (`null` si el objeto ya no existe).
pub fn wrap_target(agent: &mut Agent, doc: DocIndex, t: EventTarget) -> Value {
    match t {
        EventTarget::Node(n) => wrap_node(agent, NodeRef::new(doc, n)),
        EventTarget::Window => {
            let realm = WebContextHandle::current(agent).realm;
            Value::Object(agent.realms[realm].global_object)
        }
        EventTarget::Object(id) => {
            let realm = agent.current_realm;
            agent
                .host_lookup(realm, object_key(doc, id))
                .map_or(Value::Null, Value::Object)
        }
    }
}

/// Un wrapper de `EventTarget` para un target no-nodo de `bf-dom` (con identidad).
pub fn wrap_object_target(
    agent: &mut Agent,
    doc: DocIndex,
    target: EventTarget,
    class: HostClassId,
    payload: Box<dyn std::any::Any>,
) -> Gc<Object> {
    let EventTarget::Object(id) = target else {
        unreachable!("solo targets no-nodo")
    };
    let realm = agent.current_realm;
    agent.host_wrap(realm, object_key(doc, id), class, payload)
}

/// `new EventTarget()` (DOM §2.7).
fn event_target_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = crate::with_doc_mut(agent, 0, Document::create_event_target);
    let EventTarget::Object(id) = t else {
        unreachable!("target no-nodo")
    };
    let class = classes(agent).event_target;
    let nt = a.new_target.expect("new.target");
    let o = agent.host_create_from_new_target(
        nt,
        class,
        Some(object_key(0, id)),
        Box::new(TargetRef { doc: 0, target: t }),
    )?;
    Ok(Value::Object(o))
}

fn this_target(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<(DocIndex, EventTarget)> {
    match event_target_of(agent, &a.this) {
        Some(t) => Ok(t),
        None => agent.type_error("Illegal invocation"),
    }
}

/// El id de listener de un callback (siempre el mismo para el mismo objeto).
fn listener_id(agent: &mut Agent, cb: Gc<Object>) -> u64 {
    if let Some(id) = state(agent).listener_ids.get(&cb) {
        return *id;
    }
    let h = agent.host_pin(Value::Object(cb));
    let st = state(agent);
    st.next_listener += 1;
    let id = st.next_listener;
    st.listener_ids.insert(cb, id);
    st.listeners.insert(id, ListenerRec::Callback(h));
    id
}

/// Las opciones de `addEventListener`/`removeEventListener` (DOM §2.7 "flatten more").
fn listener_options(
    agent: &mut Agent,
    v: &Value,
) -> JsResult<(ListenerOptions, Option<(DocIndex, EventTarget)>)> {
    let mut opts = ListenerOptions::default();
    match v {
        Value::Object(o) => {
            let o = *o;
            let get = |agent: &mut Agent, name: &str| {
                agent.get(o, &PropertyKey::from(name), Value::Object(o))
            };
            let capture = get(agent, "capture")?;
            opts.capture = bf_js::host::webidl::to_boolean(agent, &capture);
            let once = get(agent, "once")?;
            opts.once = bf_js::host::webidl::to_boolean(agent, &once);
            let passive = get(agent, "passive")?;
            if !passive.is_undefined() {
                opts.passive = Some(bf_js::host::webidl::to_boolean(agent, &passive));
            }
            let signal = get(agent, "signal")?;
            if signal.is_undefined() {
                return Ok((opts, None));
            }
            let s = agent.host_unwrap::<TargetRef>(&signal).copied();
            match s {
                Some(t) if agent.host_is_instance(&signal, classes(agent).abort_signal) => {
                    Ok((opts, Some((t.doc, t.target))))
                }
                _ => agent.type_error(
                    "Failed to read the 'signal' property from 'AddEventListenerOptions': \
                     The provided value is not of type 'AbortSignal'",
                ),
            }
        }
        other => {
            opts.capture = bf_js::host::webidl::to_boolean(agent, other);
            Ok((opts, None))
        }
    }
}

/// `addEventListener(type, callback, options)` (DOM §2.7).
fn add_event_listener(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, target) = this_target(agent, a)?;
    let type_ = to_dom_string(agent, &a.arg(0))?;
    let cb = a.arg(1);
    let (opts, signal) = listener_options(agent, &a.arg(2))?;
    let Value::Object(cbo) = cb else {
        if !cb.is_nullish() {
            return agent.type_error(
                "Failed to execute 'addEventListener' on 'EventTarget': parameter 2 is not of \
                 type 'Object'",
            );
        }
        return Ok(Value::Undefined);
    };
    if let Some((sdoc, s)) = signal
        && crate::with_doc(agent, sdoc, |d| d.signal_aborted(s))
    {
        return Ok(Value::Undefined);
    }
    let id = listener_id(agent, cbo);
    let capture = opts.capture;
    crate::with_doc_mut(agent, doc, |d| {
        d.add_event_listener(target, type_.clone(), Listener::Script(id), opts);
    });
    if let Some((sdoc, s)) = signal
        && sdoc == doc
    {
        let t = type_.clone();
        crate::with_doc_mut(agent, doc, |d| {
            d.add_abort_algorithm(
                s,
                std::sync::Arc::new(move |d: &mut Document| {
                    d.remove_event_listener(target, &t, &Listener::Script(id), capture);
                }),
            );
        });
    }
    Ok(Value::Undefined)
}

/// `removeEventListener(type, callback, options)`.
fn remove_event_listener(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, target) = this_target(agent, a)?;
    let type_ = to_dom_string(agent, &a.arg(0))?;
    let (opts, _) = listener_options(agent, &a.arg(2))?;
    let Value::Object(cbo) = a.arg(1) else {
        return Ok(Value::Undefined);
    };
    let Some(&id) = state(agent).listener_ids.get(&cbo) else {
        return Ok(Value::Undefined);
    };
    crate::with_doc_mut(agent, doc, |d| {
        d.remove_event_listener(target, &type_, &Listener::Script(id), opts.capture);
    });
    Ok(Value::Undefined)
}

/// `dispatchEvent(event)` (DOM §2.7).
fn dispatch_event(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, target) = this_target(agent, a)?;
    let ev = a.arg(0);
    let Some(Value::Object(evo)) = event_data(agent, &ev).map(|_| ev.clone()) else {
        return agent.type_error(
            "Failed to execute 'dispatchEvent' on 'EventTarget': parameter 1 is not of type \
             'Event'",
        );
    };
    let r = dispatch_js_event(agent, doc, target, evo, true)?;
    match r {
        Ok(not_canceled) => Ok(Value::Boolean(not_canceled)),
        Err(e) => throw_dom(agent, e),
    }
}

// --- El puente con el dispatch de bf-dom ---

/// El payload de un wrapper de evento.
fn event_data<'a>(agent: &'a Agent, v: &Value) -> Option<&'a EventData> {
    agent.host_unwrap::<EventData>(v)
}

/// El wrapper de un evento en dispatch: el objeto, su celda y el pin si se creó aquí.
pub(crate) type EventWrapper = (Gc<Object>, Rc<RefCell<Event>>, Option<HostHandle>);

/// Llama a los listeners de script desde el dispatch de `bf-dom`.
pub(crate) struct JsHost<'a> {
    pub agent: &'a mut Agent,
    pub ctx: WebContextHandle,
    pub doc: DocIndex,
    /// El wrapper del evento (creado en el primer listener si el evento es nativo).
    pub wrapper: Option<EventWrapper>,
}

impl JsHost<'_> {
    fn ensure_wrapper(&mut self, event: &Event) -> (Gc<Object>, Rc<RefCell<Event>>) {
        if let Some((o, c, _)) = &self.wrapper {
            return (*o, c.clone());
        }
        let iface = match &event.kind {
            EventKind::Event => EventIface::Event,
            EventKind::Custom { .. } => EventIface::Custom,
            EventKind::Ui { .. } => EventIface::Ui,
            EventKind::Mouse(_) => EventIface::Mouse,
            EventKind::Keyboard(_) => EventIface::Keyboard,
            EventKind::Focus { .. } => EventIface::Focus,
            EventKind::Input(_) => EventIface::Input,
        };
        let cell = Rc::new(RefCell::new(Event::new("", EventInit::default())));
        let o = create_event_object(self.agent, iface, cell.clone(), self.doc);
        let h = self.agent.host_pin(Value::Object(o));
        self.wrapper = Some((o, cell.clone(), Some(h)));
        (o, cell)
    }
}

impl Drop for JsHost<'_> {
    fn drop(&mut self) {
        if let Some((_, _, Some(h))) = self.wrapper.take() {
            self.agent.host_unpin(h);
        }
    }
}

impl ScriptHost for JsHost<'_> {
    fn call_listener(&mut self, doc: &mut Document, handle: u64, event: &mut Event) -> bool {
        let (wrapper, cell) = self.ensure_wrapper(event);
        // El evento vigente pasa a la celda del wrapper y el documento vuelve al contexto.
        std::mem::swap(&mut *cell.borrow_mut(), event);
        let ctx = self.ctx;
        std::mem::swap(ctx.get_mut(self.agent).doc_mut(self.doc), doc);
        let threw = call_script_listener(self.agent, self.doc, handle, wrapper);
        std::mem::swap(ctx.get_mut(self.agent).doc_mut(self.doc), doc);
        std::mem::swap(&mut *cell.borrow_mut(), event);
        threw
    }
}

/// Despacha `event` en `target` del documento `doc` con los listeners de script (saca el
/// documento del contexto mientras dura). `Err` si el evento ya se estaba despachando o no
/// estaba inicializado (con `checked`).
pub fn dispatch_js_event(
    agent: &mut Agent,
    doc: DocIndex,
    target: EventTarget,
    event: Gc<Object>,
    checked: bool,
) -> JsResult<Result<bool, bf_dom::DomException>> {
    let ev_value = Value::Object(event);
    let cell = event_data(agent, &ev_value).expect("evento").cell.clone();
    if let Some(d) = event_data(agent, &ev_value) {
        d.doc.set(doc);
    }
    let ctx = WebContextHandle::current(agent);
    {
        let ev = cell.borrow();
        sync_content_handlers(agent, doc, target, &ev.type_);
    }
    let mut ev = std::mem::replace(
        &mut *cell.borrow_mut(),
        Event::new("", EventInit::default()),
    );
    let mut document = std::mem::take(ctx.get_mut(agent).doc_mut(doc));
    let r = {
        let mut host = JsHost {
            agent,
            ctx,
            doc,
            wrapper: Some((event, cell.clone(), None)),
        };
        if checked {
            document.dispatch_event_checked(target, &mut ev, &mut host)
        } else {
            Ok(document.dispatch(target, &mut ev, &mut host))
        }
    };
    *ctx.get_mut(agent).doc_mut(doc) = document;
    *cell.borrow_mut() = ev;
    crate::dom::after_dom_mutation(agent, doc);
    Ok(r)
}

/// Corre `f` sobre el documento `doc` con un `ScriptHost` que llama a los listeners de
/// script (el documento sale del contexto mientras tanto): para las operaciones de `bf-dom`
/// que disparan eventos (`signal_abort`, `fire_event`, `fire_window_load`…).
pub fn with_script_host<R>(
    agent: &mut Agent,
    doc: DocIndex,
    f: impl FnOnce(&mut Document, &mut dyn ScriptHost) -> R,
) -> R {
    let ctx = WebContextHandle::current(agent);
    let mut document = std::mem::take(ctx.get_mut(agent).doc_mut(doc));
    let r = {
        let mut host = JsHost {
            agent,
            ctx,
            doc,
            wrapper: None,
        };
        f(&mut document, &mut host)
    };
    *ctx.get_mut(agent).doc_mut(doc) = document;
    crate::dom::after_dom_mutation(agent, doc);
    r
}

/// Dispara un evento de confianza en `target` ("fire an event"): para el navegador.
pub fn fire_event(
    agent: &mut Agent,
    doc: DocIndex,
    target: EventTarget,
    type_: &str,
    init: EventInit,
    kind: EventKind,
) -> bool {
    sync_content_handlers(agent, doc, target, &DomString::from(type_));
    with_script_host(agent, doc, |d, host| {
        d.fire_event(target, type_, init, kind, host)
    })
}

/// Llama a un listener de script; `true` si lanzó.
fn call_script_listener(agent: &mut Agent, doc: DocIndex, handle: u64, event: Gc<Object>) -> bool {
    let Some(rec) = state(agent).listeners.get(&handle).cloned() else {
        return false;
    };
    let ev = Value::Object(event);
    let current = {
        let d = event_data(agent, &ev).expect("evento");
        d.cell.borrow().current_target
    };
    let this = current.map_or(Value::Undefined, |t| wrap_target(agent, doc, t));
    // `window.event` mientras corre el listener (HTML §8.1.8.4).
    let pin = agent.host_pin(ev.clone());
    state(agent).current_event.push(pin);
    let r = match rec {
        ListenerRec::Callback(h) => {
            let cb = agent.host_pinned(h).unwrap_or_default();
            if agent.is_callable(&cb) {
                agent.call(&cb, this, std::slice::from_ref(&ev))
            } else if let Value::Object(o) = cb {
                match agent.get(o, &PropertyKey::from("handleEvent"), Value::Object(o)) {
                    Ok(f) if agent.is_callable(&f) => agent.call(&f, Value::Object(o), &[ev]),
                    Ok(_) => agent.type_error("handleEvent is not a function"),
                    Err(e) => Err(e),
                }
            } else {
                Ok(Value::Undefined)
            }
        }
        ListenerRec::Handler {
            doc: hdoc,
            target,
            type_,
        } => run_event_handler(agent, hdoc, target, &type_, &ev, this),
    };
    if let Some(p) = state(agent).current_event.pop() {
        agent.host_unpin(p);
    }
    match r {
        Ok(_) => false,
        Err(Throw::Value(e)) => {
            report_exception(agent, &e);
            true
        }
        Err(Throw::Terminate(_)) => true,
    }
}

/// HTML "report an exception" (§8.1.4.6): un `ErrorEvent` `error` en la ventana; si nadie lo
/// cancela, va a los hooks del motor.
pub fn report_exception(agent: &mut Agent, e: &Value) {
    let err = bf_js::host::JsError::from_value(agent, e.clone());
    if state(agent).reporting {
        agent.host.hooks.report_exception(&err.message, &err.stack);
        return;
    }
    state(agent).reporting = true;
    let ctx = WebContextHandle::current(agent);
    let filename = ctx.get(agent).url.serialize(false);
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
        message: format!("Uncaught {}", err.message).into(),
        filename: filename.into(),
        lineno: 0,
        colno: 0,
    };
    let o = create_event_object(agent, iface, cell, 0);
    if let Some(s) = agent.host_slots_mut(o) {
        s[0] = e.clone();
    }
    let pin = agent.host_pin(Value::Object(o));
    let not_canceled = matches!(
        dispatch_js_event(agent, 0, EventTarget::Window, o, false),
        Ok(Ok(true))
    );
    agent.host_unpin(pin);
    state(agent).reporting = false;
    if not_canceled {
        agent.host.hooks.report_exception(&err.message, &err.stack);
    }
}

// --- Creación de wrappers de eventos ---

fn class_for_iface(agent: &Agent, iface: &EventIface) -> HostClassId {
    let c = classes(agent);
    match iface {
        EventIface::Event => c.event,
        EventIface::Custom => c.custom_event,
        EventIface::Ui => c.ui_event,
        EventIface::Focus => c.focus_event,
        EventIface::Mouse => c.mouse_event,
        EventIface::Keyboard => c.keyboard_event,
        EventIface::Input => c.input_event,
        EventIface::Error { .. } => c.error_event,
        EventIface::PromiseRejection => c.promise_rejection_event,
    }
}

/// Un wrapper de evento nuevo (sin `new.target`).
pub fn create_event_object(
    agent: &mut Agent,
    iface: EventIface,
    cell: Rc<RefCell<Event>>,
    doc: DocIndex,
) -> Gc<Object> {
    let class = class_for_iface(agent, &iface);
    let o = agent.host_create(
        class,
        Box::new(EventData {
            cell,
            iface,
            doc: Cell::new(doc),
        }),
    );
    if let Some(s) = agent.host_slots_mut(o) {
        *s = vec![Value::Null, Value::Undefined];
    }
    o
}

/// Lee un miembro de un diccionario (`undefined` si no hay diccionario).
fn dict_get(agent: &mut Agent, dict: &Value, name: &str) -> JsResult<Value> {
    match dict {
        Value::Object(o) => agent.get(*o, &PropertyKey::from(name), dict.clone()),
        Value::Undefined | Value::Null => Ok(Value::Undefined),
        _ => agent.type_error("The provided value is not of type 'EventInit'"),
    }
}

fn dict_bool(agent: &mut Agent, dict: &Value, name: &str) -> JsResult<bool> {
    let v = dict_get(agent, dict, name)?;
    Ok(bf_js::host::webidl::to_boolean(agent, &v))
}

/// `EventInit` (DOM §2.2): `bubbles`, `cancelable`, `composed`.
fn event_init(agent: &mut Agent, dict: &Value) -> JsResult<EventInit> {
    Ok(EventInit {
        bubbles: dict_bool(agent, dict, "bubbles")?,
        cancelable: dict_bool(agent, dict, "cancelable")?,
        composed: dict_bool(agent, dict, "composed")?,
    })
}

/// El tipo (`DOMString type`, obligatorio) de un constructor de evento.
fn ctor_type(agent: &mut Agent, a: &CallArgs<'_>, class: &str) -> JsResult<DomString> {
    if a.is_empty() {
        return agent.type_error(&format!(
            "Failed to construct '{class}': 1 argument required, but only 0 present."
        ));
    }
    to_dom_string(agent, &a.arg(0))
}

/// Construye el wrapper de un evento de script (con `new.target`).
fn construct_event(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    iface: EventIface,
    event: Event,
    slots: [Value; 2],
) -> JsResult<Value> {
    let class = class_for_iface(agent, &iface);
    let nt = a.new_target.expect("new.target");
    let o = agent.host_create_from_new_target(
        nt,
        class,
        None,
        Box::new(EventData {
            cell: Rc::new(RefCell::new(event)),
            iface,
            doc: Cell::new(0),
        }),
    )?;
    if let Some(s) = agent.host_slots_mut(o) {
        *s = slots.to_vec();
    }
    Ok(Value::Object(o))
}

/// `new Event(type, eventInitDict)`.
fn event_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let type_ = ctor_type(agent, a, "Event")?;
    let init = event_init(agent, &a.arg(1))?;
    construct_event(
        agent,
        a,
        EventIface::Event,
        Event::new(type_, init),
        [Value::Null, Value::Undefined],
    )
}

/// `new CustomEvent(type, {detail})`.
fn custom_event_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let type_ = ctor_type(agent, a, "CustomEvent")?;
    let dict = a.arg(1);
    let init = event_init(agent, &dict)?;
    let detail = dict_get(agent, &dict, "detail")?;
    let detail = if detail.is_undefined() {
        Value::Null
    } else {
        detail
    };
    let ev = Event::with_kind(type_, init, EventKind::Custom { detail: None });
    construct_event(agent, a, EventIface::Custom, ev, [detail, Value::Undefined])
}

/// `UIEventInit`: `view` (Window?) y `detail` (long).
fn ui_init(agent: &mut Agent, dict: &Value) -> JsResult<(Value, i32)> {
    let detail = dict_get(agent, dict, "detail")?;
    let detail = bf_js::host::webidl::to_long(agent, &detail)?;
    let view = dict_get(agent, dict, "view")?;
    let view = if view.is_nullish() {
        Value::Null
    } else if agent.host_unwrap::<WindowData>(&view).is_some() {
        view
    } else {
        return agent.type_error(
            "Failed to read the 'view' property from 'UIEventInit': The provided value is not \
             of type 'Window'",
        );
    };
    Ok((view, detail))
}

fn ui_event_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let type_ = ctor_type(agent, a, "UIEvent")?;
    let dict = a.arg(1);
    let init = event_init(agent, &dict)?;
    let (view, detail) = ui_init(agent, &dict)?;
    let ev = Event::with_kind(type_, init, EventKind::Ui { detail });
    construct_event(agent, a, EventIface::Ui, ev, [view, Value::Undefined])
}

/// Un `EventTarget?` de un diccionario (`relatedTarget`).
fn dict_target(agent: &mut Agent, dict: &Value, name: &str) -> JsResult<Option<EventTarget>> {
    let v = dict_get(agent, dict, name)?;
    if v.is_nullish() {
        return Ok(None);
    }
    match event_target_of(agent, &v) {
        Some((_, t)) => Ok(Some(t)),
        None => agent.type_error(&format!(
            "Failed to read the '{name}' property: The provided value is not of type \
             'EventTarget'"
        )),
    }
}

fn focus_event_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let type_ = ctor_type(agent, a, "FocusEvent")?;
    let dict = a.arg(1);
    let init = event_init(agent, &dict)?;
    let (view, _) = ui_init(agent, &dict)?;
    let related_target = dict_target(agent, &dict, "relatedTarget")?;
    let ev = Event::with_kind(type_, init, EventKind::Focus { related_target });
    construct_event(agent, a, EventIface::Focus, ev, [view, Value::Undefined])
}

/// `EventModifierInit` (UI Events §3.6): las teclas modificadoras.
fn modifiers(agent: &mut Agent, dict: &Value) -> JsResult<[bool; 4]> {
    Ok([
        dict_bool(agent, dict, "ctrlKey")?,
        dict_bool(agent, dict, "shiftKey")?,
        dict_bool(agent, dict, "altKey")?,
        dict_bool(agent, dict, "metaKey")?,
    ])
}

fn dict_double(agent: &mut Agent, dict: &Value, name: &str) -> JsResult<f64> {
    let v = dict_get(agent, dict, name)?;
    if v.is_undefined() {
        return Ok(0.0);
    }
    bf_js::host::webidl::to_double(agent, &v)
}

fn mouse_event_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let type_ = ctor_type(agent, a, "MouseEvent")?;
    let dict = a.arg(1);
    let init = event_init(agent, &dict)?;
    let (view, detail) = ui_init(agent, &dict)?;
    let [ctrl_key, shift_key, alt_key, meta_key] = modifiers(agent, &dict)?;
    let screen_x = dict_double(agent, &dict, "screenX")?;
    let screen_y = dict_double(agent, &dict, "screenY")?;
    let client_x = dict_double(agent, &dict, "clientX")?;
    let client_y = dict_double(agent, &dict, "clientY")?;
    let button = dict_get(agent, &dict, "button")?;
    let button = bf_js::host::webidl::to_long(agent, &button)? as i16;
    let buttons = dict_get(agent, &dict, "buttons")?;
    let buttons = bf_js::host::webidl::to_unsigned_short(agent, &buttons)?;
    let related_target = dict_target(agent, &dict, "relatedTarget")?;
    let data = MouseEventData {
        screen_x,
        screen_y,
        client_x,
        client_y,
        ctrl_key,
        shift_key,
        alt_key,
        meta_key,
        button,
        buttons,
        related_target,
        detail,
    };
    let ev = Event::with_kind(type_, init, EventKind::Mouse(data));
    construct_event(agent, a, EventIface::Mouse, ev, [view, Value::Undefined])
}

fn keyboard_event_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let type_ = ctor_type(agent, a, "KeyboardEvent")?;
    let dict = a.arg(1);
    let init = event_init(agent, &dict)?;
    let (view, _) = ui_init(agent, &dict)?;
    let [ctrl_key, shift_key, alt_key, meta_key] = modifiers(agent, &dict)?;
    let key = dict_get(agent, &dict, "key")?;
    let key = if key.is_undefined() {
        DomString::new()
    } else {
        to_dom_string(agent, &key)?
    };
    let code = dict_get(agent, &dict, "code")?;
    let code = if code.is_undefined() {
        DomString::new()
    } else {
        to_dom_string(agent, &code)?
    };
    let location = dict_get(agent, &dict, "location")?;
    let location = bf_js::host::webidl::to_unsigned_long(agent, &location)?;
    let repeat = dict_bool(agent, &dict, "repeat")?;
    let is_composing = dict_bool(agent, &dict, "isComposing")?;
    let data = KeyboardEventData {
        key,
        code,
        location,
        ctrl_key,
        shift_key,
        alt_key,
        meta_key,
        repeat,
        is_composing,
    };
    let ev = Event::with_kind(type_, init, EventKind::Keyboard(data));
    construct_event(agent, a, EventIface::Keyboard, ev, [view, Value::Undefined])
}

fn input_event_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let type_ = ctor_type(agent, a, "InputEvent")?;
    let dict = a.arg(1);
    let init = event_init(agent, &dict)?;
    let (view, _) = ui_init(agent, &dict)?;
    let data = dict_get(agent, &dict, "data")?;
    let data = crate::dom::to_opt_dom_string(agent, &data)?;
    let is_composing = dict_bool(agent, &dict, "isComposing")?;
    let input_type = dict_get(agent, &dict, "inputType")?;
    let input_type = if input_type.is_undefined() {
        DomString::new()
    } else {
        to_dom_string(agent, &input_type)?
    };
    let ev = Event::with_kind(
        type_,
        init,
        EventKind::Input(InputEventData {
            data,
            is_composing,
            input_type,
        }),
    );
    construct_event(agent, a, EventIface::Input, ev, [view, Value::Undefined])
}

fn error_event_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let type_ = ctor_type(agent, a, "ErrorEvent")?;
    let dict = a.arg(1);
    let init = event_init(agent, &dict)?;
    let colno = dict_get(agent, &dict, "colno")?;
    let colno = bf_js::host::webidl::to_unsigned_long(agent, &colno)?;
    let error = dict_get(agent, &dict, "error")?;
    let error = if error.is_undefined() {
        Value::Null
    } else {
        error
    };
    let filename = dict_get(agent, &dict, "filename")?;
    let filename = if filename.is_undefined() {
        DomString::new()
    } else {
        to_dom_string(agent, &filename)?
    };
    let lineno = dict_get(agent, &dict, "lineno")?;
    let lineno = bf_js::host::webidl::to_unsigned_long(agent, &lineno)?;
    let message = dict_get(agent, &dict, "message")?;
    let message = if message.is_undefined() {
        DomString::new()
    } else {
        to_dom_string(agent, &message)?
    };
    construct_event(
        agent,
        a,
        EventIface::Error {
            message,
            filename,
            lineno,
            colno,
        },
        Event::new(type_, init),
        [error, Value::Undefined],
    )
}

fn promise_rejection_event_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let type_ = ctor_type(agent, a, "PromiseRejectionEvent")?;
    let dict = a.arg(1);
    let init = event_init(agent, &dict)?;
    let promise = dict_get(agent, &dict, "promise")?;
    if !promise.is_object() {
        return agent.type_error(
            "Failed to construct 'PromiseRejectionEvent': required member promise is undefined.",
        );
    }
    let reason = dict_get(agent, &dict, "reason")?;
    construct_event(
        agent,
        a,
        EventIface::PromiseRejection,
        Event::new(type_, init),
        [promise, reason],
    )
}

// --- Atributos de Event ---

fn with_event<R>(agent: &mut Agent, a: &CallArgs<'_>, f: impl FnOnce(&Event) -> R) -> JsResult<R> {
    match event_data(agent, &a.this) {
        Some(d) => Ok(f(&d.cell.borrow())),
        None => agent.type_error("Illegal invocation"),
    }
}

fn with_event_mut<R>(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    f: impl FnOnce(&mut Event) -> R,
) -> JsResult<R> {
    match event_data(agent, &a.this) {
        Some(d) => Ok(f(&mut d.cell.borrow_mut())),
        None => agent.type_error("Illegal invocation"),
    }
}

fn event_doc(agent: &Agent, a: &CallArgs<'_>) -> DocIndex {
    event_data(agent, &a.this).map_or(0, |d| d.doc.get())
}

fn ev_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = with_event(agent, a, |e| e.type_.clone())?;
    Ok(from_dom_string(&t))
}

fn ev_target(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = with_event(agent, a, |e| e.target)?;
    let doc = event_doc(agent, a);
    Ok(t.map_or(Value::Null, |t| wrap_target(agent, doc, t)))
}

fn ev_current_target(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = with_event(agent, a, |e| e.current_target)?;
    let doc = event_doc(agent, a);
    Ok(t.map_or(Value::Null, |t| wrap_target(agent, doc, t)))
}

fn ev_phase(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let p = with_event(agent, a, |e| e.event_phase as u16)?;
    Ok(Value::Number(f64::from(p)))
}

fn ev_cancel_bubble(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(with_event(agent, a, Event::cancel_bubble)?))
}

fn ev_set_cancel_bubble(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = bf_js::host::webidl::to_boolean(agent, &a.arg(0));
    with_event_mut(agent, a, |e| e.set_cancel_bubble(v))?;
    Ok(Value::Undefined)
}

fn ev_bubbles(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(with_event(agent, a, |e| e.bubbles)?))
}

fn ev_cancelable(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(with_event(agent, a, |e| e.cancelable)?))
}

fn ev_return_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(with_event(agent, a, Event::return_value)?))
}

fn ev_set_return_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = bf_js::host::webidl::to_boolean(agent, &a.arg(0));
    with_event_mut(agent, a, |e| e.set_return_value(v))?;
    Ok(Value::Undefined)
}

fn ev_default_prevented(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(with_event(
        agent,
        a,
        Event::default_prevented,
    )?))
}

fn ev_composed(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(with_event(agent, a, |e| e.composed)?))
}

fn ev_is_trusted(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(with_event(agent, a, |e| e.is_trusted)?))
}

fn ev_time_stamp(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(with_event(agent, a, |e| e.time_stamp)?))
}

fn ev_composed_path(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let path = with_event(agent, a, Event::composed_path)?;
    let doc = event_doc(agent, a);
    let values: Vec<Value> = path
        .into_iter()
        .map(|t| wrap_target(agent, doc, t))
        .collect();
    Ok(Value::Object(agent.create_array_from_list(&values)?))
}

fn ev_stop_propagation(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    with_event_mut(agent, a, Event::stop_propagation)?;
    Ok(Value::Undefined)
}

fn ev_stop_immediate_propagation(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    with_event_mut(agent, a, Event::stop_immediate_propagation)?;
    Ok(Value::Undefined)
}

fn ev_prevent_default(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    with_event_mut(agent, a, Event::prevent_default)?;
    Ok(Value::Undefined)
}

/// `initEvent(type, bubbles = false, cancelable = false)` (DOM §2.2, legacy).
fn ev_init_event(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let type_ = to_dom_string(agent, &a.arg(0))?;
    let bubbles = bf_js::host::webidl::to_boolean(agent, &a.arg(1));
    let cancelable = bf_js::host::webidl::to_boolean(agent, &a.arg(2));
    with_event_mut(agent, a, |e| e.init_event(type_, bubbles, cancelable))?;
    Ok(Value::Undefined)
}

fn slot(agent: &mut Agent, a: &CallArgs<'_>, i: usize) -> JsResult<Value> {
    match &a.this {
        Value::Object(o) => Ok(agent
            .host_slots_mut(*o)
            .and_then(|s| s.get(i).cloned())
            .unwrap_or_default()),
        _ => agent.type_error("Illegal invocation"),
    }
}

fn slot0(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    slot(agent, a, 0)
}

fn slot1(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    slot(agent, a, 1)
}

/// `initCustomEvent(type, bubbles, cancelable, detail)`.
fn init_custom_event(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let type_ = to_dom_string(agent, &a.arg(0))?;
    let bubbles = bf_js::host::webidl::to_boolean(agent, &a.arg(1));
    let cancelable = bf_js::host::webidl::to_boolean(agent, &a.arg(2));
    let dispatching = with_event(agent, a, Event::is_dispatching)?;
    if dispatching {
        return Ok(Value::Undefined);
    }
    with_event_mut(agent, a, |e| e.init_event(type_, bubbles, cancelable))?;
    if let Value::Object(o) = a.this
        && let Some(s) = agent.host_slots_mut(o)
    {
        s[0] = if a.arg(3).is_undefined() {
            Value::Null
        } else {
            a.arg(3)
        };
    }
    Ok(Value::Undefined)
}

/// `initUIEvent(type, bubbles, cancelable, view, detail)`.
fn init_ui_event(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let type_ = to_dom_string(agent, &a.arg(0))?;
    let bubbles = bf_js::host::webidl::to_boolean(agent, &a.arg(1));
    let cancelable = bf_js::host::webidl::to_boolean(agent, &a.arg(2));
    let detail = bf_js::host::webidl::to_long(agent, &a.arg(4))?;
    let dispatching = with_event(agent, a, Event::is_dispatching)?;
    if dispatching {
        return Ok(Value::Undefined);
    }
    with_event_mut(agent, a, |e| {
        e.init_event(type_, bubbles, cancelable);
        if let EventKind::Ui { detail: d } = &mut e.kind {
            *d = detail;
        }
    })?;
    if let Value::Object(o) = a.this
        && let Some(s) = agent.host_slots_mut(o)
    {
        s[0] = if a.arg(3).is_undefined() {
            Value::Null
        } else {
            a.arg(3)
        };
    }
    Ok(Value::Undefined)
}

fn ui_detail(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = with_event(agent, a, |e| match &e.kind {
        EventKind::Ui { detail } => *detail,
        EventKind::Mouse(m) => m.detail,
        _ => 0,
    })?;
    Ok(Value::Number(f64::from(d)))
}

fn ui_which(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let w = with_event(agent, a, |e| match &e.kind {
        EventKind::Mouse(m) => i32::from(m.button) + 1,
        _ => 0,
    })?;
    Ok(Value::Number(f64::from(w)))
}

fn focus_related(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = with_event(agent, a, |e| match &e.kind {
        EventKind::Focus { related_target } => *related_target,
        _ => None,
    })?;
    let doc = event_doc(agent, a);
    Ok(t.map_or(Value::Null, |t| wrap_target(agent, doc, t)))
}

fn mouse<R: Default>(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    f: impl FnOnce(&MouseEventData) -> R,
) -> JsResult<R> {
    with_event(agent, a, |e| match &e.kind {
        EventKind::Mouse(m) => f(m),
        _ => R::default(),
    })
}

fn mouse_screen_x(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(mouse(agent, a, |m| m.screen_x)?))
}

fn mouse_screen_y(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(mouse(agent, a, |m| m.screen_y)?))
}

fn mouse_client_x(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(mouse(agent, a, |m| m.client_x)?))
}

fn mouse_client_y(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(mouse(agent, a, |m| m.client_y)?))
}

fn mouse_button(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(f64::from(mouse(agent, a, |m| m.button)?)))
}

fn mouse_buttons(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(f64::from(mouse(agent, a, |m| m.buttons)?)))
}

fn mouse_related(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = mouse(agent, a, |m| m.related_target)?;
    let doc = event_doc(agent, a);
    Ok(t.map_or(Value::Null, |t| wrap_target(agent, doc, t)))
}

/// Las modificadoras de un evento de mouse o teclado: (ctrl, shift, alt, meta).
fn mods(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<[bool; 4]> {
    with_event(agent, a, |e| match &e.kind {
        EventKind::Mouse(m) => [m.ctrl_key, m.shift_key, m.alt_key, m.meta_key],
        EventKind::Keyboard(k) => [k.ctrl_key, k.shift_key, k.alt_key, k.meta_key],
        _ => [false; 4],
    })
}

fn mod_ctrl(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(mods(agent, a)?[0]))
}

fn mod_shift(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(mods(agent, a)?[1]))
}

fn mod_alt(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(mods(agent, a)?[2]))
}

fn mod_meta(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(mods(agent, a)?[3]))
}

/// `getModifierState(keyArg)` (UI Events §5.6.2).
fn get_modifier_state(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let key = to_dom_string(agent, &a.arg(0))?;
    let m = mods(agent, a)?;
    let r = match key.as_lossy_str() {
        "Control" => m[0],
        "Shift" => m[1],
        "Alt" => m[2],
        "Meta" => m[3],
        _ => false,
    };
    Ok(Value::Boolean(r))
}

fn keyboard<R: Default>(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    f: impl FnOnce(&KeyboardEventData) -> R,
) -> JsResult<R> {
    with_event(agent, a, |e| match &e.kind {
        EventKind::Keyboard(k) => f(k),
        _ => R::default(),
    })
}

fn kb_key(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(from_dom_string(&keyboard(agent, a, |k| k.key.clone())?))
}

fn kb_code(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(from_dom_string(&keyboard(agent, a, |k| k.code.clone())?))
}

fn kb_location(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(f64::from(keyboard(agent, a, |k| {
        k.location
    })?)))
}

fn kb_repeat(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(keyboard(agent, a, |k| k.repeat)?))
}

fn kb_is_composing(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(keyboard(agent, a, |k| k.is_composing)?))
}

fn kb_char_code(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let c = keyboard(agent, a, |k| {
        let mut chars = k.key.as_lossy_str().chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => c as u32,
            _ => 0,
        }
    })?;
    let t = with_event(agent, a, |e| e.type_ == "keypress")?;
    Ok(Value::Number(if t { f64::from(c) } else { 0.0 }))
}

fn kb_key_code(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let c = keyboard(agent, a, |k| {
        let mut chars = k.key.as_lossy_str().chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if c.is_ascii_alphanumeric() => c.to_ascii_uppercase() as u32,
            _ => match k.key.as_lossy_str() {
                "Enter" => 13,
                "Escape" => 27,
                "Backspace" => 8,
                "Tab" => 9,
                " " => 32,
                _ => 0,
            },
        }
    })?;
    Ok(Value::Number(f64::from(c)))
}

fn input<R: Default>(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    f: impl FnOnce(&InputEventData) -> R,
) -> JsResult<R> {
    with_event(agent, a, |e| match &e.kind {
        EventKind::Input(i) => f(i),
        _ => R::default(),
    })
}

fn input_data(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = input(agent, a, |i| i.data.clone())?;
    Ok(from_opt_dom_string(d.as_ref()))
}

fn input_is_composing(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(input(agent, a, |i| i.is_composing)?))
}

fn input_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(from_dom_string(&input(agent, a, |i| i.input_type.clone())?))
}

fn error_field(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<(DomString, DomString, u32, u32)> {
    match event_data(agent, &a.this).map(|d| d.iface.clone()) {
        Some(EventIface::Error {
            message,
            filename,
            lineno,
            colno,
        }) => Ok((message, filename, lineno, colno)),
        Some(_) => Ok((DomString::new(), DomString::new(), 0, 0)),
        None => agent.type_error("Illegal invocation"),
    }
}

fn err_message(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(from_dom_string(&error_field(agent, a)?.0))
}

fn err_filename(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(from_dom_string(&error_field(agent, a)?.1))
}

fn err_lineno(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(f64::from(error_field(agent, a)?.2)))
}

fn err_colno(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(f64::from(error_field(agent, a)?.3)))
}

// --- Event handlers (HTML §8.1.8) ---

/// GlobalEventHandlers (HTML §8.1.8.2).
pub const GLOBAL_EVENT_HANDLERS: &[&str] = &[
    "abort",
    "auxclick",
    "beforeinput",
    "beforematch",
    "beforetoggle",
    "blur",
    "cancel",
    "canplay",
    "canplaythrough",
    "change",
    "click",
    "close",
    "contextlost",
    "contextmenu",
    "contextrestored",
    "copy",
    "cuechange",
    "cut",
    "dblclick",
    "drag",
    "dragend",
    "dragenter",
    "dragleave",
    "dragover",
    "dragstart",
    "drop",
    "durationchange",
    "emptied",
    "ended",
    "error",
    "focus",
    "formdata",
    "input",
    "invalid",
    "keydown",
    "keypress",
    "keyup",
    "load",
    "loadeddata",
    "loadedmetadata",
    "loadstart",
    "mousedown",
    "mouseenter",
    "mouseleave",
    "mousemove",
    "mouseout",
    "mouseover",
    "mouseup",
    "paste",
    "pause",
    "play",
    "playing",
    "progress",
    "ratechange",
    "reset",
    "resize",
    "scroll",
    "scrollend",
    "securitypolicyviolation",
    "seeked",
    "seeking",
    "select",
    "slotchange",
    "stalled",
    "submit",
    "suspend",
    "timeupdate",
    "toggle",
    "volumechange",
    "waiting",
    "webkitanimationend",
    "webkitanimationiteration",
    "webkitanimationstart",
    "webkittransitionend",
    "wheel",
    "animationstart",
    "animationiteration",
    "animationend",
    "animationcancel",
    "transitionrun",
    "transitionstart",
    "transitionend",
    "transitioncancel",
    "pointerover",
    "pointerenter",
    "pointerdown",
    "pointermove",
    "pointerup",
    "pointercancel",
    "pointerout",
    "pointerleave",
    "gotpointercapture",
    "lostpointercapture",
    "selectstart",
    "selectionchange",
];

/// WindowEventHandlers (HTML §8.1.8.2).
pub const WINDOW_EVENT_HANDLERS: &[&str] = &[
    "afterprint",
    "beforeprint",
    "beforeunload",
    "hashchange",
    "languagechange",
    "message",
    "messageerror",
    "offline",
    "online",
    "pagehide",
    "pagereveal",
    "pageshow",
    "pageswap",
    "popstate",
    "rejectionhandled",
    "storage",
    "unhandledrejection",
    "unload",
];

/// Los handlers de `body`/`frameset` que son de la ventana (HTML §8.1.8.2).
const BODY_WINDOW_HANDLERS: &[&str] = &[
    "blur",
    "error",
    "focus",
    "load",
    "resize",
    "scroll",
    "afterprint",
    "beforeprint",
    "beforeunload",
    "hashchange",
    "languagechange",
    "message",
    "messageerror",
    "offline",
    "online",
    "pagehide",
    "pagereveal",
    "pageshow",
    "pageswap",
    "popstate",
    "rejectionhandled",
    "storage",
    "unhandledrejection",
    "unload",
];

/// Agrega los atributos `on…` de `names` a una clase (para `HTMLElement` de E1, `Document` y
/// `Window`).
pub fn with_event_handlers(mut class: HostClass, names: &[&'static str]) -> HostClass {
    for name in names {
        let attr: &'static str = Box::leak(format!("on{name}").into_boxed_str());
        class = class.attribute(attr, Some(handler_get), Some(handler_set));
    }
    class
}

/// El tipo de evento de un getter/setter `get onclick`/`set onclick` (por su `name`).
fn handler_type(agent: &mut Agent, f: Gc<Object>) -> DomString {
    let name = agent.heap[f]
        .props
        .get(&PropertyKey::from("name"))
        .and_then(|p| match &p.value {
            PropValue::Data { value, .. } => value.as_string().map(|s| s.to_string_lossy()),
            PropValue::Accessor { .. } => None,
        })
        .unwrap_or_default();
    name.strip_prefix("get on")
        .or_else(|| name.strip_prefix("set on"))
        .unwrap_or("")
        .into()
}

/// El target de un handler: los de ventana puestos en `body` van a la ventana.
fn handler_target(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    type_: &DomString,
) -> JsResult<(DocIndex, EventTarget)> {
    let (doc, t) = this_target(agent, a)?;
    if let EventTarget::Node(n) = t {
        let is_body = crate::with_doc(agent, doc, |d| {
            d.element(n)
                .is_some_and(|e| e.is_html("body") || e.is_html("frameset"))
        });
        if is_body && BODY_WINDOW_HANDLERS.contains(&type_.as_lossy_str()) {
            return Ok((0, EventTarget::Window));
        }
    }
    Ok((doc, t))
}

/// El elemento que tiene el atributo de contenido de un handler (para la ventana, `body`).
fn content_element(agent: &Agent, doc: DocIndex, target: EventTarget) -> Option<bf_dom::NodeId> {
    let ctx = WebContextHandle::current(agent).get(agent);
    let d = ctx.doc(doc);
    match target {
        EventTarget::Node(n) => d.element(n).is_some().then_some(n),
        EventTarget::Window => d.body(),
        EventTarget::Object(_) => None,
    }
}

/// "Get the current value of the event handler" (HTML §8.1.8.1): compila el atributo de
/// contenido si cambió.
fn handler_value(
    agent: &mut Agent,
    doc: DocIndex,
    target: EventTarget,
    type_: &DomString,
) -> Value {
    let key = (doc, target, type_.clone());
    let st = state(agent).handlers.get(&key).cloned().unwrap_or_default();
    let attr_name: DomString = format!("on{type_}").into();
    let el = content_element(agent, doc, target);
    let attr = el.and_then(|e| crate::with_doc(agent, doc, |d| d.get_attribute(e, &attr_name)));
    // Un atributo de contenido nuevo o cambiado reemplaza al valor IDL.
    let changed = match (&attr, &st.compiled) {
        (Some(a), Some((src, _))) => a != src,
        (Some(_), None) => st.idl.is_none(),
        (None, _) => false,
    };
    if !changed {
        if let Some(idl) = st.idl {
            return idl
                .and_then(|h| agent.host_pinned(h))
                .unwrap_or(Value::Null);
        }
        if let (Some(_), Some((_, f))) = (&attr, &st.compiled) {
            return f.and_then(|h| agent.host_pinned(h)).unwrap_or(Value::Null);
        }
        return Value::Null;
    }
    let src = attr.expect("atributo");
    let f = compile_handler(agent, doc, target, el, type_, &src);
    let h = f.map(|f| agent.host_pin(f));
    let entry = state(agent).handlers.entry(key).or_default();
    entry.idl = None;
    entry.compiled = Some((src, h));
    h.and_then(|h| agent.host_pinned(h)).unwrap_or(Value::Null)
}

/// Compila el cuerpo de un atributo de contenido `on…` (HTML §8.1.8.1 "getting the current
/// value", paso 3): una función con el parámetro `event` (o los cinco de `onerror` de la
/// ventana) y el documento y el elemento en la cadena de scopes.
fn compile_handler(
    agent: &mut Agent,
    doc: DocIndex,
    target: EventTarget,
    el: Option<bf_dom::NodeId>,
    type_: &DomString,
    body: &DomString,
) -> Option<Value> {
    let params = if type_ == "error" && target == EventTarget::Window {
        "event, source, lineno, colno, error"
    } else {
        "event"
    };
    let text = format!(
        "(function (__d, __e) {{ with (__d) {{ with (__e) {{ return function on{type_}({params}) {{\n{body}\n}}; }} }} }})"
    );
    let r = agent
        .compile_script_text(&text, "<event handler>")
        .and_then(|code| agent.script_evaluation(code));
    let outer = match r {
        Ok(f) => f,
        Err(Throw::Value(e)) => {
            report_exception(agent, &e);
            return None;
        }
        Err(Throw::Terminate(_)) => return None,
    };
    let root = crate::with_doc(agent, doc, Document::root);
    let d = wrap_node(agent, NodeRef::new(doc, root));
    let e = match (target, el) {
        (EventTarget::Node(_), Some(el)) => wrap_node(agent, NodeRef::new(doc, el)),
        _ => agent.new_plain_object().into(),
    };
    match agent.call(&outer, Value::Undefined, &[d, e]) {
        Ok(f) => Some(f),
        Err(Throw::Value(e)) => {
            report_exception(agent, &e);
            None
        }
        Err(_) => None,
    }
}

/// Registra (una vez) el listener de un event handler en `bf-dom`.
fn ensure_handler_listener(
    agent: &mut Agent,
    doc: DocIndex,
    target: EventTarget,
    type_: &DomString,
) {
    let key = (doc, target, type_.clone());
    if state(agent).handler_listeners.contains(&key) {
        return;
    }
    let st = state(agent);
    st.handler_listeners.insert(key);
    st.next_listener += 1;
    let id = st.next_listener;
    st.listeners.insert(
        id,
        ListenerRec::Handler {
            doc,
            target,
            type_: type_.clone(),
        },
    );
    crate::with_doc_mut(agent, doc, |d| {
        d.add_event_listener(
            target,
            type_.clone(),
            Listener::Script(id),
            ListenerOptions::default(),
        );
    });
}

/// Antes de un dispatch: registra los listeners de los atributos de contenido `on<type>` de
/// los nodos del event path (los que nunca pasaron por el atributo IDL).
pub fn sync_content_handlers(
    agent: &mut Agent,
    doc: DocIndex,
    target: EventTarget,
    type_: &DomString,
) {
    let attr_name: DomString = format!("on{type_}").into();
    let found: Vec<EventTarget> = crate::with_doc(agent, doc, |d| {
        let mut out = Vec::new();
        let mut cur = match target {
            EventTarget::Node(n) => Some(n),
            _ => None,
        };
        while let Some(n) = cur {
            if d.element(n).is_some() && d.has_attribute(n, &attr_name) {
                out.push(EventTarget::Node(n));
            }
            cur = d.parent(n);
        }
        if (matches!(target, EventTarget::Window) || matches!(target, EventTarget::Node(_)))
            && BODY_WINDOW_HANDLERS.contains(&type_.as_lossy_str())
            && let Some(b) = d.body()
            && d.has_attribute(b, &attr_name)
        {
            out.push(EventTarget::Window);
        }
        out
    });
    for t in found {
        if let EventTarget::Node(n) = t
            && BODY_WINDOW_HANDLERS.contains(&type_.as_lossy_str())
            && crate::with_doc(agent, doc, |d| {
                d.element(n)
                    .is_some_and(|e| e.is_html("body") || e.is_html("frameset"))
            })
        {
            ensure_handler_listener(agent, 0, EventTarget::Window, type_);
            continue;
        }
        let tdoc = if t == EventTarget::Window { 0 } else { doc };
        ensure_handler_listener(agent, tdoc, t, type_);
    }
}

/// El getter `on…`.
fn handler_get(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let type_ = handler_type(agent, a.callee);
    let (doc, target) = handler_target(agent, a, &type_)?;
    Ok(handler_value(agent, doc, target, &type_))
}

/// El setter `on…` (`[LegacyTreatNonObjectAsNull]`: lo que no es objeto es `null`).
fn handler_set(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let type_ = handler_type(agent, a.callee);
    let (doc, target) = handler_target(agent, a, &type_)?;
    let v = bf_js::host::webidl::to_event_handler(&a.arg(0));
    let h = v.map(|v| agent.host_pin(v));
    let key = (doc, target, type_.clone());
    let attr = content_element(agent, doc, target).and_then(|e| {
        let name: DomString = format!("on{type_}").into();
        crate::with_doc(agent, doc, |d| d.get_attribute(e, &name))
    });
    let entry: &mut HandlerState = state(agent).handlers.entry(key).or_default();
    entry.idl = Some(h);
    // El atributo de contenido actual queda como ya visto.
    entry.compiled = attr.map(|a| (a, None));
    if h.is_some() {
        ensure_handler_listener(agent, doc, target, &type_);
    }
    Ok(Value::Undefined)
}

/// "The event handler processing algorithm" (HTML §8.1.8.1).
fn run_event_handler(
    agent: &mut Agent,
    doc: DocIndex,
    target: EventTarget,
    type_: &DomString,
    event: &Value,
    this: Value,
) -> JsResult<Value> {
    let callback = handler_value(agent, doc, target, type_);
    if !agent.is_callable(&callback) {
        return Ok(Value::Undefined);
    }
    let error_special = type_ == "error"
        && target == EventTarget::Window
        && matches!(
            event_data(agent, event).map(|d| &d.iface),
            Some(EventIface::Error { .. })
        );
    let result = if error_special {
        let (message, filename, lineno, colno) =
            match event_data(agent, event).map(|d| d.iface.clone()) {
                Some(EventIface::Error {
                    message,
                    filename,
                    lineno,
                    colno,
                }) => (message, filename, lineno, colno),
                _ => unreachable!("ErrorEvent"),
            };
        let error = match event {
            Value::Object(o) => agent
                .host_slots_mut(*o)
                .and_then(|s| s.first().cloned())
                .unwrap_or_default(),
            _ => Value::Undefined,
        };
        let args = [
            from_dom_string(&message),
            from_dom_string(&filename),
            Value::Number(f64::from(lineno)),
            Value::Number(f64::from(colno)),
            error,
        ];
        agent.call(&callback, this, &args)?
    } else {
        agent.call(&callback, this, std::slice::from_ref(event))?
    };
    // Procesamiento del valor de retorno.
    let cancel = if error_special {
        matches!(result, Value::Boolean(true))
    } else {
        matches!(result, Value::Boolean(false))
    };
    if cancel && let Some(d) = event_data(agent, event) {
        d.cell.borrow_mut().prevent_default();
    }
    Ok(result)
}

/// `document.createEvent(interface)` (DOM §4.5): la tabla de interfaces heredadas, sin
/// inicializar (`initEvent` lo hace utilizable).
pub fn create_event(agent: &mut Agent, doc: DocIndex, interface: &str) -> JsResult<Value> {
    let lower = interface.to_ascii_lowercase();
    let (iface, kind) = match lower.as_str() {
        "event" | "events" | "htmlevents" | "svgevents" => (EventIface::Event, EventKind::Event),
        "customevent" => (EventIface::Custom, EventKind::Custom { detail: None }),
        "uievent" | "uievents" => (EventIface::Ui, EventKind::Ui { detail: 0 }),
        "focusevent" => (
            EventIface::Focus,
            EventKind::Focus {
                related_target: None,
            },
        ),
        "mouseevent" | "mouseevents" => (
            EventIface::Mouse,
            EventKind::Mouse(MouseEventData::default()),
        ),
        "keyboardevent" => (
            EventIface::Keyboard,
            EventKind::Keyboard(KeyboardEventData::default()),
        ),
        _ => return throw_dom(agent, bf_dom::DomException::NotSupportedError),
    };
    let cell = Rc::new(RefCell::new(Event::uninitialized(kind)));
    let o = create_event_object(agent, iface, cell, doc);
    Ok(Value::Object(o))
}
