//! `ResizeObserver`, `ResizeObserverEntry` y `ResizeObserverSize` (W3C Resize Observer, §2 las
//! interfaces y §3 el modelo de proceso). **E2.**
//!
//! - El observador vive como dato del realm ([`RoState`]); el objeto JS lleva su id y, en su
//!   primer slot, el callback. Queda fijado mientras observa algún elemento.
//! - Una observación está activa si el tamaño de su caja (`content-box`, `border-box` o
//!   `device-pixel-content-box`) difiere del último informado (§3.1.1 *isActive()*; el primero
//!   es −1×−1, así que la primera observación siempre se notifica, también la de un elemento
//!   sin caja o de 0×0).
//! - [`run_steps`] es lo que HTML hace en *update the rendering* (§8.1.7.3, paso 14; Resize
//!   Observer §3.4.1): reunir las observaciones activas a partir de una profundidad, difundir
//!   (los callbacks se llaman en el momento), volver a maquetar y repetir más hondo mientras
//!   queden; si quedaron observaciones saltadas, se informa el error "ResizeObserver loop
//!   completed with undelivered notifications." (§3.4.6).

use std::cell::RefCell;
use std::rc::Rc;

use bf_dom::events::{Event, EventInit, EventTarget};
use bf_dom::{Document, NodeId};
use bf_js::host::{HostClass, HostClassId, HostHandle};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, RealmId, Throw};
use bf_layout::{FragmentKind, FragmentTree};

use super::view::RectData;
use crate::dom::events::EventIface;
use crate::dom::{unwrap_node, wrap_node};
use crate::{NodeRef, WebContextHandle, with_doc};

/// La caja que se observa (§2.1, `ResizeObserverBoxOptions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoxKind {
    Content,
    Border,
    DevicePixelContent,
}

/// Un tamaño (`ResizeObserverSize`): en el sentido de la línea y en el de los bloques.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Size {
    inline: f64,
    block: f64,
}

/// Una observación (§3.1, `ResizeObservation`).
#[derive(Debug, Clone, Copy)]
struct Observation {
    target: NodeRef,
    observed_box: BoxKind,
    last_reported: Size,
}

struct Observer {
    id: u64,
    pinned: Option<HostHandle>,
    observations: Vec<Observation>,
    /// `[[activeTargets]]` (índices en `observations`) y si hubo `[[skippedTargets]]`.
    active: Vec<usize>,
    skipped: bool,
}

/// Los `ResizeObserver` de una ventana (dato del realm).
#[derive(Default)]
pub struct RoState {
    observers: Vec<Observer>,
    next_id: u64,
    last_key: Option<super::RenderingKey>,
}

#[derive(Debug)]
struct ObserverData {
    id: u64,
}

/// Los datos de un `ResizeObserverEntry`.
#[derive(Debug, Clone)]
struct EntryData {
    target: NodeRef,
    content_rect: RectData,
    border_box: Size,
    content_box: Size,
    device_pixel_content_box: Size,
}

fn ro_state(agent: &mut Agent, realm: RealmId) -> &mut RoState {
    if agent.realm_data::<RoState>(realm).is_none() {
        agent.set_realm_data(realm, RoState::default());
    }
    agent.realm_data_mut::<RoState>(realm).expect("RoState")
}

fn current_realm(agent: &Agent) -> RealmId {
    WebContextHandle::current(agent).realm
}

pub fn register_classes(agent: &mut Agent) -> (HostClassId, HostClassId, HostClassId) {
    let observer = agent.register_host_class(
        HostClass::new("ResizeObserver")
            .constructor(ctor, 1)
            .method("observe", 1, observe)
            .method("unobserve", 1, unobserve)
            .method("disconnect", 0, disconnect),
    );
    let entry = agent.register_host_class(
        HostClass::new("ResizeObserverEntry")
            .attribute("target", Some(entry_target), None)
            .attribute("contentRect", Some(entry_content_rect), None)
            .attribute("borderBoxSize", Some(entry_border_box_size), None)
            .attribute("contentBoxSize", Some(entry_content_box_size), None)
            .attribute(
                "devicePixelContentBoxSize",
                Some(entry_device_pixel_content_box_size),
                None,
            ),
    );
    let size = agent.register_host_class(
        HostClass::new("ResizeObserverSize")
            .attribute("inlineSize", Some(size_inline), None)
            .attribute("blockSize", Some(size_block), None),
    );
    (observer, entry, size)
}

/// `new ResizeObserver(callback)` (§2.2).
fn ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let cb = a.arg(0);
    if !agent.is_callable(&cb) {
        return agent.type_error(
            "Failed to construct 'ResizeObserver': The callback provided as parameter 1 is not \
             a function.",
        );
    }
    let realm = current_realm(agent);
    let st = ro_state(agent, realm);
    st.next_id += 1;
    let id = st.next_id;
    st.observers.push(Observer {
        id,
        pinned: None,
        observations: Vec::new(),
        active: Vec::new(),
        skipped: false,
    });
    let class = super::classes(agent).resize_observer;
    let nt = a.new_target.expect("new.target");
    let o = agent.host_create_from_new_target(nt, class, None, Box::new(ObserverData { id }))?;
    if let Some(s) = agent.host_slots_mut(o) {
        *s = vec![cb];
    }
    Ok(Value::Object(o))
}

fn this_id(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<u64> {
    match agent.host_unwrap::<ObserverData>(&a.this) {
        Some(d) => Ok(d.id),
        None => agent.type_error("Illegal invocation"),
    }
}

fn with_observer<T>(agent: &mut Agent, id: u64, f: impl FnOnce(&mut Observer) -> T) -> Option<T> {
    let realm = current_realm(agent);
    ro_state(agent, realm)
        .observers
        .iter_mut()
        .find(|o| o.id == id)
        .map(f)
}

fn arg_element(agent: &mut Agent, v: &Value, method: &str) -> JsResult<NodeRef> {
    if let Some(n) = unwrap_node(agent, v)
        && with_doc(agent, n.doc, |d| d.element(n.node).is_some())
    {
        return Ok(n);
    }
    agent.type_error(&format!(
        "Failed to execute '{method}' on 'ResizeObserver': parameter 1 is not of type 'Element'."
    ))
}

/// `observe(target, options)` (§2.2): si ya lo observaba, la observación vuelve a empezar.
fn observe(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_id(agent, a)?;
    let target = arg_element(agent, &a.arg(0), "observe")?;
    let opts = a.arg(1);
    let box_v = match &opts {
        Value::Object(o) => agent.get(*o, &PropertyKey::from("box"), opts.clone())?,
        Value::Undefined | Value::Null => Value::Undefined,
        _ => {
            return agent.type_error(
                "Failed to execute 'observe' on 'ResizeObserver': parameter 2 is not of type \
                 'ResizeObserverOptions'.",
            );
        }
    };
    let observed_box = if box_v.is_undefined() {
        BoxKind::Content
    } else {
        match bf_js::host::webidl::to_rust_string(agent, &box_v)?.as_str() {
            "content-box" => BoxKind::Content,
            "border-box" => BoxKind::Border,
            "device-pixel-content-box" => BoxKind::DevicePixelContent,
            other => {
                return agent.type_error(&format!(
                    "Failed to execute 'observe' on 'ResizeObserver': Failed to read the 'box' \
                     property from 'ResizeObserverOptions': The provided value '{other}' is not \
                     a valid enum value of type ResizeObserverBoxOptions."
                ));
            }
        }
    };
    let this = a.this.clone();
    let needs_pin = with_observer(agent, id, |o| {
        o.observations.retain(|ob| ob.target != target);
        o.active.clear();
        o.observations.push(Observation {
            target,
            observed_box,
            last_reported: Size {
                inline: -1.0,
                block: -1.0,
            },
        });
        o.pinned.is_none()
    })
    .unwrap_or(false);
    if needs_pin {
        let h = agent.host_pin(this);
        with_observer(agent, id, |o| o.pinned = Some(h));
    }
    let realm = current_realm(agent);
    ro_state(agent, realm).last_key = None;
    Ok(Value::Undefined)
}

fn unpin_if_idle(agent: &mut Agent, id: u64) {
    let h = with_observer(agent, id, |o| {
        if o.observations.is_empty() {
            o.pinned.take()
        } else {
            None
        }
    })
    .flatten();
    if let Some(h) = h {
        agent.host_unpin(h);
    }
}

fn unobserve(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_id(agent, a)?;
    let target = arg_element(agent, &a.arg(0), "unobserve")?;
    with_observer(agent, id, |o| {
        o.observations.retain(|ob| ob.target != target);
        o.active.clear();
    });
    unpin_if_idle(agent, id);
    Ok(Value::Undefined)
}

fn disconnect(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_id(agent, a)?;
    with_observer(agent, id, |o| {
        o.observations.clear();
        o.active.clear();
        o.skipped = false;
    });
    unpin_if_idle(agent, id);
    Ok(Value::Undefined)
}

// --- Tamaños (§3.1.1, "calculate box size") ---

/// Las tres cajas de `n` y su `contentRect`; todo cero si no tiene caja o si es una caja en
/// línea no reemplazada (no tiene tamaño propio).
fn sizes(tree: &FragmentTree, doc: &Document, n: NodeRef, dpr: f64) -> EntryData {
    let empty = EntryData {
        target: n,
        content_rect: RectData::default(),
        border_box: Size {
            inline: 0.0,
            block: 0.0,
        },
        content_box: Size {
            inline: 0.0,
            block: 0.0,
        },
        device_pixel_content_box: Size {
            inline: 0.0,
            block: 0.0,
        },
    };
    if n.doc != 0 || !doc.is_connected(n.node) {
        return empty;
    }
    let frags = super::view::element_fragments(tree, n.node);
    let Some(f) = frags.first() else {
        return empty;
    };
    if f.kind == FragmentKind::Inline {
        return empty;
    }
    let c = f.content_rect();
    let (cw, ch) = (f64::from(c.width), f64::from(c.height));
    EntryData {
        target: n,
        content_rect: RectData {
            x: f64::from(f.padding.left),
            y: f64::from(f.padding.top),
            width: cw,
            height: ch,
        },
        border_box: Size {
            inline: f64::from(f.rect.width),
            block: f64::from(f.rect.height),
        },
        content_box: Size {
            inline: cw,
            block: ch,
        },
        device_pixel_content_box: Size {
            inline: (cw * dpr).round(),
            block: (ch * dpr).round(),
        },
    }
}

fn observed(e: &EntryData, b: BoxKind) -> Size {
    match b {
        BoxKind::Content => e.content_box,
        BoxKind::Border => e.border_box,
        BoxKind::DevicePixelContent => e.device_pixel_content_box,
    }
}

/// La profundidad de un nodo en el árbol plano (§3.4.4, "calculate depth for node"): los nodos
/// del camino hasta el elemento raíz, el propio incluido. Un elemento fuera del documento es la
/// raíz de su árbol: profundidad 1 (y no 0, que lo dejaría siempre saltado).
fn depth(doc: &Document, node: NodeId) -> usize {
    let mut d = 1;
    let mut cur = doc.parent(node);
    while let Some(n) = cur {
        if n == doc.root() {
            break;
        }
        d += 1;
        cur = doc.shadow_host(n).or_else(|| doc.parent(n));
    }
    d
}

/// "Gather active resize observations at depth" (§3.4.2).
fn gather(agent: &mut Agent, realm: RealmId, at: usize) -> (bool, bool) {
    let tree = super::current_layout(agent);
    let dpr = f64::from(super::window_metrics(agent).dpr);
    let Some(mut st) = agent.take_realm_data::<RoState>(realm) else {
        return (false, false);
    };
    {
        let doc = WebContextHandle { realm }.get(agent).document();
        for o in &mut st.observers {
            o.active.clear();
            o.skipped = false;
            for (i, ob) in o.observations.iter().enumerate() {
                let now = observed(&sizes(&tree, doc, ob.target, dpr), ob.observed_box);
                if now != ob.last_reported {
                    let d = if ob.target.doc == 0 {
                        depth(doc, ob.target.node)
                    } else {
                        1
                    };
                    if d > at {
                        o.active.push(i);
                    } else {
                        o.skipped = true;
                    }
                }
            }
        }
    }
    let has_active = st.observers.iter().any(|o| !o.active.is_empty());
    let has_skipped = st.observers.iter().any(|o| o.skipped);
    agent.set_realm_data(realm, st);
    (has_active, has_skipped)
}

/// "Broadcast active resize observations" (§3.4.3): llama a cada callback con sus entradas;
/// devuelve la profundidad más baja de los objetivos difundidos.
fn broadcast(agent: &mut Agent, realm: RealmId) -> usize {
    let tree = super::current_layout(agent);
    let dpr = f64::from(super::window_metrics(agent).dpr);
    let mut shallowest = usize::MAX;
    let mut batches: Vec<(u64, Vec<EntryData>)> = Vec::new();
    if let Some(mut st) = agent.take_realm_data::<RoState>(realm) {
        {
            let doc = WebContextHandle { realm }.get(agent).document();
            for o in &mut st.observers {
                if o.active.is_empty() {
                    continue;
                }
                let mut entries = Vec::new();
                for &i in &o.active {
                    let ob = &mut o.observations[i];
                    let e = sizes(&tree, doc, ob.target, dpr);
                    ob.last_reported = observed(&e, ob.observed_box);
                    if ob.target.doc == 0 {
                        shallowest = shallowest.min(depth(doc, ob.target.node));
                    }
                    entries.push(e);
                }
                o.active.clear();
                batches.push((o.id, entries));
            }
        }
        agent.set_realm_data(realm, st);
    }
    drop(tree);
    for (id, entries) in batches {
        let h = agent
            .realm_data::<RoState>(realm)
            .and_then(|s| s.observers.iter().find(|o| o.id == id))
            .and_then(|o| o.pinned);
        let Some(obj) = h.and_then(|h| agent.host_pinned(h)) else {
            continue;
        };
        let cb = match &obj {
            Value::Object(o) => agent
                .host_slots_mut(*o)
                .and_then(|s| s.first().cloned())
                .unwrap_or_default(),
            _ => continue,
        };
        let class = super::classes(agent).resize_observer_entry;
        let values: Vec<Value> = entries
            .into_iter()
            .map(|e| Value::Object(agent.host_create(class, Box::new(e))))
            .collect();
        let Ok(arr) = agent.create_array_from_list(&values) else {
            continue;
        };
        match agent.call(&cb, obj.clone(), &[Value::Object(arr), obj]) {
            Ok(_) => {}
            Err(Throw::Value(e)) => crate::dom::events::report_exception(agent, &e),
            Err(_) => break,
        }
        if agent.frames.is_empty()
            && let Err(Throw::Value(e)) = agent.perform_microtask_checkpoint()
        {
            agent.report_exception(&e);
        }
    }
    shallowest
}

/// Lo que HTML hace con los `ResizeObserver` en *update the rendering* (§8.1.7.3, paso 14).
pub(crate) fn run_steps(agent: &mut Agent, realm: RealmId) {
    let has = agent
        .realm_data::<RoState>(realm)
        .is_some_and(|s| s.observers.iter().any(|o| !o.observations.is_empty()));
    if !has {
        return;
    }
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let mut at = 0;
    let (mut active, mut skipped) = gather(agent, realm, at);
    // Cada vuelta solo difunde objetivos más hondos que la anterior, así que termina.
    while active {
        at = broadcast(agent, realm);
        (active, skipped) = gather(agent, realm, at);
    }
    if skipped {
        deliver_loop_error(agent, realm);
    }
    let key = super::rendering_key(agent, realm);
    ro_state(agent, realm).last_key = Some(key);
    agent.current_realm = saved;
}

/// ¿Hay algo que mirar en la próxima actualización? (Una observación nueva, o la geometría pudo
/// cambiar.)
pub(crate) fn pending(agent: &Agent, realm: RealmId, key: &super::RenderingKey) -> bool {
    agent.realm_data::<RoState>(realm).is_some_and(|s| {
        s.observers.iter().any(|o| !o.observations.is_empty()) && s.last_key.as_ref() != Some(key)
    })
}

/// "Deliver resize loop error notification" (§3.4.6): un `ErrorEvent` en la ventana con el
/// mensaje de la especificación, como "report an exception" (HTML §8.1.4.6).
fn deliver_loop_error(agent: &mut Agent, realm: RealmId) {
    const MESSAGE: &str = "ResizeObserver loop completed with undelivered notifications.";
    let filename = WebContextHandle { realm }.get(agent).url.serialize(false);
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
        message: MESSAGE.into(),
        filename: filename.into(),
        lineno: 0,
        colno: 0,
    };
    let o = crate::dom::events::create_event_object(agent, iface, cell, 0);
    if let Some(s) = agent.host_slots_mut(o) {
        s[0] = Value::Null;
    }
    let pin = agent.host_pin(Value::Object(o));
    let not_canceled = matches!(
        crate::dom::events::dispatch_js_event(agent, 0, EventTarget::Window, o, false),
        Ok(Ok(true))
    );
    agent.host_unpin(pin);
    if not_canceled {
        agent.host.hooks.report_exception(MESSAGE, "");
    }
}

// --- ResizeObserverEntry y ResizeObserverSize ---

fn entry(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<EntryData> {
    match agent.host_unwrap::<EntryData>(&a.this) {
        Some(e) => Ok(e.clone()),
        None => agent.type_error("Illegal invocation"),
    }
}

/// El valor guardado en el slot `i` del objeto, o el que da `make` (que se guarda): los
/// atributos `[SameObject]`.
fn cached(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    i: usize,
    make: impl FnOnce(&mut Agent) -> JsResult<Value>,
) -> JsResult<Value> {
    let Value::Object(o) = a.this else {
        return agent.type_error("Illegal invocation");
    };
    if let Some(v) = agent.host_slots_mut(o).and_then(|s| s.get(i).cloned())
        && !v.is_undefined()
    {
        return Ok(v);
    }
    let v = make(agent)?;
    if let Some(s) = agent.host_slots_mut(o) {
        while s.len() <= i {
            s.push(Value::Undefined);
        }
        s[i] = v.clone();
    }
    Ok(v)
}

fn entry_target(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = entry(agent, a)?.target;
    Ok(wrap_node(agent, t))
}

fn entry_content_rect(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = entry(agent, a)?.content_rect;
    cached(agent, a, 0, |agent| {
        let class = super::classes(agent).rect_read_only;
        Ok(Value::Object(agent.host_create(class, Box::new(r))))
    })
}

/// Un `FrozenArray<ResizeObserverSize>` con un tamaño (un solo fragmento).
fn size_array(agent: &mut Agent, s: Size) -> JsResult<Value> {
    let class = super::classes(agent).resize_observer_size;
    let size = Value::Object(agent.host_create(class, Box::new(s)));
    let arr = agent.create_array_from_list(&[size])?;
    agent.set_integrity_level(arr, true)?;
    Ok(Value::Object(arr))
}

fn entry_border_box_size(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = entry(agent, a)?.border_box;
    cached(agent, a, 1, |agent| size_array(agent, s))
}

fn entry_content_box_size(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = entry(agent, a)?.content_box;
    cached(agent, a, 2, |agent| size_array(agent, s))
}

fn entry_device_pixel_content_box_size(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = entry(agent, a)?.device_pixel_content_box;
    cached(agent, a, 3, |agent| size_array(agent, s))
}

fn size_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Size> {
    match agent.host_unwrap::<Size>(&a.this) {
        Some(s) => Ok(*s),
        None => agent.type_error("Illegal invocation"),
    }
}

fn size_inline(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(size_of(agent, a)?.inline))
}

fn size_block(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(size_of(agent, a)?.block))
}
