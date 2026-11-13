//! E4: la parte de Web Animations 1 que exponen las animaciones y transiciones CSS (CSS
//! Animations 2 §6 `CSSAnimation`, CSS Transitions 2 §6 `CSSTransition`): `Animation` con su
//! tiempo (`currentTime`, `startTime`, `playState`, `pause()`, `play()`, `finish()`,
//! `cancel()`, `ready`, `finished`), `KeyframeEffect` (`target`, `pseudoElement`,
//! `getTiming()`, `getComputedTiming()`), `document.timeline` (`DocumentTimeline`) y
//! `getAnimations()` de `Document` y `Element` (Web Animations 1 §6.12, `Animatable`).
//!
//! Los objetos son vistas del [`bf_style::animation::Animator`] de la ventana: cada
//! `Animation` guarda el identificador de su animación y lee o cambia su estado allí. No hay
//! `Element.animate()` ni `new Animation()` (animaciones de script), `playbackRate` distinto de
//! 1, `reverse()` ni eventos `finish`/`cancel` en el objeto.

use bf_js::heap::Gc;
use bf_js::host::{HostClass, HostClassId, HostKey};
use bf_js::object::Object;
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, RealmId};
use bf_style::animation::{AnimationTimes, Key, Timing};
use bf_style::properties::animations::{AnimationDirection, FillMode};

use super::animation::{AnimationState, document_time, state_mut};
use super::{accessor, method};
use crate::WebContextHandle;

/// Los ids de las clases de Web Animations (uno por agente).
#[derive(Debug, Clone, Copy)]
pub struct AnimationClasses {
    pub animation: HostClassId,
    pub css_animation: HostClassId,
    pub css_transition: HostClassId,
    pub animation_effect: HostClassId,
    pub keyframe_effect: HostClassId,
    pub animation_timeline: HostClassId,
    pub document_timeline: HostClassId,
}

/// El payload de un `Animation`: el identificador de su animación en el `Animator`.
struct AnimHandle(u64);

/// El payload de un `KeyframeEffect`: el de su animación.
struct EffectHandle(u64);

/// Los slots de un `Animation`: `ready`, `finished` y `effect`.
const SLOT_READY: usize = 0;
const SLOT_FINISHED: usize = 1;
const SLOT_EFFECT: usize = 2;

fn classes(agent: &Agent) -> AnimationClasses {
    *agent
        .embedder_data::<AnimationClasses>()
        .expect("bf_web::bom::web_animations::register_classes no se llamó")
}

/// Registra las clases (una vez por agente).
pub fn register_classes(agent: &mut Agent) {
    if agent.embedder_data::<AnimationClasses>().is_some() {
        return;
    }
    let animation = agent.register_host_class(
        HostClass::new("Animation")
            .attribute("id", Some(empty_string), Some(ignore))
            .attribute("effect", Some(effect), None)
            .attribute("timeline", Some(timeline), None)
            .attribute("startTime", Some(start_time), Some(set_start_time))
            .attribute("currentTime", Some(current_time), Some(set_current_time))
            .attribute("playbackRate", Some(one), None)
            .attribute("playState", Some(play_state), None)
            .attribute("replaceState", Some(replace_state), None)
            .attribute("pending", Some(false_value), None)
            .attribute("ready", Some(ready), None)
            .attribute("finished", Some(finished), None)
            .method("play", 0, play)
            .method("pause", 0, pause)
            .method("finish", 0, finish)
            .method("cancel", 0, cancel)
            .method("persist", 0, undefined)
            .method("commitStyles", 0, undefined),
    );
    let css_animation =
        agent.register_host_class(HostClass::new("CSSAnimation").parent(animation).attribute(
            "animationName",
            Some(name),
            None,
        ));
    let css_transition =
        agent.register_host_class(HostClass::new("CSSTransition").parent(animation).attribute(
            "transitionProperty",
            Some(name),
            None,
        ));
    let animation_effect = agent.register_host_class(
        HostClass::new("AnimationEffect")
            .method("getTiming", 0, get_timing)
            .method("getComputedTiming", 0, get_computed_timing),
    );
    let keyframe_effect = agent.register_host_class(
        HostClass::new("KeyframeEffect")
            .parent(animation_effect)
            .attribute("target", Some(target), None)
            .attribute("pseudoElement", Some(pseudo_element), None)
            .attribute("composite", Some(replace), None)
            .method("getKeyframes", 0, get_keyframes),
    );
    let animation_timeline = agent.register_host_class(
        HostClass::new("AnimationTimeline").attribute("currentTime", Some(timeline_time), None),
    );
    let document_timeline =
        agent.register_host_class(HostClass::new("DocumentTimeline").parent(animation_timeline));
    agent.set_embedder_data(AnimationClasses {
        animation,
        css_animation,
        css_transition,
        animation_effect,
        keyframe_effect,
        animation_timeline,
        document_timeline,
    });
}

/// `getAnimations()` en `Document` y `Element` y `document.timeline` de la ventana `realm`.
pub fn install(agent: &mut Agent, realm: RealmId) {
    let dom = crate::dom::classes(agent);
    let (document, element) = (dom.document, dom.element);
    let proto = agent.host_class_objects(realm, document).prototype;
    method(agent, proto, "getAnimations", 0, document_get_animations);
    accessor(agent, proto, "timeline", document_timeline, None);
    let proto = agent.host_class_objects(realm, element).prototype;
    method(agent, proto, "getAnimations", 0, element_get_animations);
}

/// La clave de identidad del `Animation` de `id` (fuera del espacio de los nodos y de los
/// objetos de la ventana).
fn animation_key(id: u64) -> HostKey {
    HostKey(0xFFFF_FFFD_0000_0000 | id)
}

/// El `Animation` de `id` en la ventana `realm` (el mismo objeto mientras viva).
fn wrap_animation(agent: &mut Agent, realm: RealmId, id: u64, transition: bool) -> Gc<Object> {
    let c = classes(agent);
    let class = if transition {
        c.css_transition
    } else {
        c.css_animation
    };
    agent.host_wrap(realm, animation_key(id), class, Box::new(AnimHandle(id)))
}

fn this_id(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<u64> {
    match agent.host_unwrap::<AnimHandle>(&a.this) {
        Some(h) => Ok(h.0),
        None => agent.type_error("Illegal invocation"),
    }
}

fn this_effect(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<u64> {
    match agent.host_unwrap::<EffectHandle>(&a.this) {
        Some(h) => Ok(h.0),
        None => agent.type_error("Illegal invocation"),
    }
}

/// "Update style": los estilos al día (un "style change event", que crea o cancela
/// animaciones y transiciones) antes de leer la lista o el estado.
fn flush_styles(agent: &mut Agent) {
    let _ = crate::cssom::current_styles(agent);
}

fn times(agent: &mut Agent, id: u64) -> Option<AnimationTimes> {
    state_mut(agent).animator.times(id)
}

/// Cambia el estado de tiempo de `id` con `f(times, hora)` y deja los estilos animados por
/// recalcular.
fn modify(agent: &mut Agent, id: u64, f: impl FnOnce(&mut AnimationTimes, f64)) {
    let now = document_time(agent);
    let st = state_mut(agent);
    if let Some(mut t) = st.animator.times(id) {
        f(&mut t, now);
        st.animator.set_times(id, &t);
        st.last = None;
    }
}

fn number_or_null(v: Option<f64>) -> Value {
    v.map_or(Value::Null, Value::Number)
}

fn slot(agent: &mut Agent, this: &Value, i: usize) -> Value {
    let Value::Object(o) = this else {
        return Value::Undefined;
    };
    agent
        .host_slots_mut(*o)
        .and_then(|s| s.get(i).cloned())
        .unwrap_or(Value::Undefined)
}

fn set_slot(agent: &mut Agent, this: &Value, i: usize, v: Value) {
    let Value::Object(o) = this else {
        return;
    };
    if let Some(slots) = agent.host_slots_mut(*o) {
        while slots.len() <= i {
            slots.push(Value::Undefined);
        }
        slots[i] = v;
    }
}

// --- Animation ---

fn empty_string(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(""))
}

fn ignore(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Undefined)
}

fn undefined(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Undefined)
}

fn one(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(1.0))
}

fn false_value(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(false))
}

fn replace_state(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str("active"))
}

fn replace(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str("replace"))
}

fn effect(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_id(agent, a)?;
    let v = slot(agent, &a.this, SLOT_EFFECT);
    if !v.is_undefined() {
        return Ok(v);
    }
    let class = classes(agent).keyframe_effect;
    let e = Value::Object(agent.host_create(class, Box::new(EffectHandle(id))));
    set_slot(agent, &a.this, SLOT_EFFECT, e.clone());
    Ok(e)
}

fn timeline(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_id(agent, a)?;
    Ok(timeline_object(agent))
}

/// `startTime` (Web Animations 1 §4.4.4): `null` mientras está en pausa.
fn start_time(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_id(agent, a)?;
    Ok(number_or_null(times(agent, id).and_then(|t| {
        if t.cancelled { None } else { t.start_time }
    })))
}

/// "Set the start time" (Web Animations 1 §4.4.5): sin hora, la animación queda en reposo
/// (aquí: retenida en su tiempo actual); con hora, deja de estar en pausa.
fn set_start_time(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_id(agent, a)?;
    let v = a.arg(0);
    let t = if v.is_nullish() {
        None
    } else {
        Some(bf_js::host::webidl::to_double(agent, &v)?)
    };
    modify(agent, id, |times, now| match t {
        Some(s) => {
            times.start_time = Some(s);
            times.hold_time = None;
            times.paused = false;
            times.cancelled = false;
        }
        None => {
            times.hold_time = times.current_time(now);
            times.start_time = None;
        }
    });
    Ok(Value::Undefined)
}

/// `currentTime` (Web Animations 1 §4.4.4): el tiempo local, `null` si está en reposo.
fn current_time(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_id(agent, a)?;
    let now = document_time(agent);
    Ok(number_or_null(
        times(agent, id).and_then(|t| t.current_time(now)),
    ))
}

/// "Set the current time" (Web Animations 1 §4.4.4, "silently set the current time"): con
/// la animación retenida (en pausa) se cambia la hora retenida; si no, la de inicio.
fn set_current_time(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_id(agent, a)?;
    let v = a.arg(0);
    if v.is_nullish() {
        let now = document_time(agent);
        if times(agent, id).and_then(|t| t.current_time(now)).is_some() {
            return agent.type_error("Failed to set the 'currentTime' property on 'Animation'");
        }
        return Ok(Value::Undefined);
    }
    let seek = bf_js::host::webidl::to_double(agent, &v)?;
    modify(agent, id, |times, now| {
        if times.cancelled {
            times.cancelled = false;
            times.paused = true;
        }
        if times.paused || times.start_time.is_none() {
            times.hold_time = Some(seek);
            times.start_time = None;
        } else {
            times.start_time = Some(now - seek);
            times.hold_time = None;
        }
    });
    Ok(Value::Undefined)
}

/// `playState` (Web Animations 1 §4.4.16).
fn play_state(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_id(agent, a)?;
    flush_styles(agent);
    let now = document_time(agent);
    let s = times(agent, id).map_or("idle", |t| t.play_state(now));
    Ok(Value::str(s))
}

/// `animationName` / `transitionProperty`.
fn name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_id(agent, a)?;
    let n = times(agent, id).map(|t| t.name).unwrap_or_default();
    Ok(Value::str(&n))
}

/// `ready` (Web Animations 1 §4.4.17): no hay tareas pendientes de reproducción o pausa (los
/// cambios se aplican en el acto), así que siempre está resuelta con la animación.
fn ready(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_id(agent, a)?;
    let v = slot(agent, &a.this, SLOT_READY);
    if !v.is_undefined() {
        return Ok(v);
    }
    let p = agent.new_internal_promise();
    agent.fulfill_promise(p, a.this.clone());
    set_slot(agent, &a.this, SLOT_READY, Value::Object(p));
    Ok(Value::Object(p))
}

/// `finished` (Web Animations 1 §4.4.18): se resuelve con la animación en el cuadro en que
/// termina ([`resolve_finished`]).
fn finished(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_id(agent, a)?;
    let v = slot(agent, &a.this, SLOT_FINISHED);
    if !v.is_undefined() {
        return Ok(v);
    }
    let p = agent.new_internal_promise();
    let now = document_time(agent);
    if times(agent, id).is_some_and(|t| t.play_state(now) == "finished") {
        agent.fulfill_promise(p, a.this.clone());
    } else {
        let pin = agent.host_pin(Value::Object(p));
        state_mut(agent).finished_waiting.push((id, pin));
    }
    set_slot(agent, &a.this, SLOT_FINISHED, Value::Object(p));
    Ok(Value::Object(p))
}

/// Resuelve las promesas `finished` de las animaciones de `realm` que terminaron en la hora
/// `now` (lo llama [`super::animation::update_and_send_events`]).
pub(crate) fn resolve_finished(agent: &mut Agent, realm: RealmId, now: f64) {
    let Some(st) = agent.realm_data_mut::<AnimationState>(realm) else {
        return;
    };
    if st.finished_waiting.is_empty() {
        return;
    }
    let waiting = std::mem::take(&mut st.finished_waiting);
    let mut keep = Vec::new();
    for (id, pin) in waiting {
        let state = agent
            .realm_data::<AnimationState>(realm)
            .and_then(|st| st.animator.times(id))
            .map(|t| t.play_state(now));
        match state {
            Some("finished") => {
                let animation = agent
                    .host_lookup(realm, animation_key(id))
                    .map_or(Value::Undefined, Value::Object);
                if let Some(Value::Object(p)) = agent.host_pinned(pin) {
                    agent.fulfill_promise(p, animation);
                }
                agent.host_unpin(pin);
            }
            Some(_) => keep.push((id, pin)),
            None => {
                agent.host_unpin(pin);
            }
        }
    }
    if let Some(st) = agent.realm_data_mut::<AnimationState>(realm) {
        st.finished_waiting.extend(keep);
    }
}

/// `play()` (Web Animations 1 §4.4.8): sigue desde la hora retenida, o desde el principio si
/// había terminado o estaba en reposo.
fn play(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_id(agent, a)?;
    modify(agent, id, |t, now| {
        let end = t.timing.end_time();
        let current = t.current_time(now);
        let seek = match current {
            Some(c) if c < end && c >= 0.0 => c,
            _ => 0.0,
        };
        t.cancelled = false;
        t.paused = false;
        t.hold_time = None;
        t.start_time = Some(now - seek);
    });
    Ok(Value::Undefined)
}

/// `pause()` (Web Animations 1 §4.4.10): retiene el tiempo actual (0 si estaba en reposo).
fn pause(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_id(agent, a)?;
    modify(agent, id, |t, now| {
        let hold = if t.cancelled {
            0.0
        } else {
            t.current_time(now).unwrap_or(0.0)
        };
        t.cancelled = false;
        t.paused = true;
        t.hold_time = Some(hold);
        t.start_time = None;
    });
    Ok(Value::Undefined)
}

/// `finish()` (Web Animations 1 §4.4.12): salta al final del efecto.
fn finish(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_id(agent, a)?;
    let end = times(agent, id).map(|t| t.timing.end_time());
    if end.is_some_and(f64::is_infinite) {
        return Err(bf_js::vm::Throw::Value(crate::dom::exception::create(
            agent,
            "InvalidStateError",
            "Failed to execute 'finish' on 'Animation': the effect end is infinite",
        )));
    }
    modify(agent, id, |t, now| {
        let end = t.timing.end_time();
        if t.paused {
            t.hold_time = Some(end);
            t.start_time = None;
        } else {
            t.start_time = Some(now - end);
            t.hold_time = None;
        }
        t.cancelled = false;
    });
    Ok(Value::Undefined)
}

/// `cancel()` (Web Animations 1 §4.4.14).
fn cancel(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_id(agent, a)?;
    let now = document_time(agent);
    let st = state_mut(agent);
    st.animator.cancel(id, now);
    st.last = None;
    // La promesa `finished` pendiente se descarta: la próxima lectura da una nueva.
    let mut unpin = Vec::new();
    st.finished_waiting.retain(|(i, pin)| {
        if *i == id {
            unpin.push(*pin);
            false
        } else {
            true
        }
    });
    for pin in unpin {
        agent.host_unpin(pin);
    }
    set_slot(agent, &a.this, SLOT_FINISHED, Value::Undefined);
    Ok(Value::Undefined)
}

// --- KeyframeEffect ---

fn target(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_effect(agent, a)?;
    let key = state_mut(agent).animator.target_of(id);
    Ok(match key {
        Some((node, _)) => crate::dom::wrap_opt(agent, 0, Some(node)),
        None => Value::Null,
    })
}

fn pseudo_element(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_effect(agent, a)?;
    let key: Option<Key> = state_mut(agent).animator.target_of(id);
    Ok(match key.and_then(|k| k.1) {
        Some(p) => Value::str(&format!("::{}", p.name())),
        None => Value::Null,
    })
}

fn get_keyframes(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_effect(agent, a)?;
    Ok(Value::Object(agent.create_array_from_list(&[])?))
}

fn put(agent: &mut Agent, o: Gc<Object>, k: &str, v: Value) {
    agent.put_data(o, PropertyKey::from(k), v, true, true, true);
}

fn fill_css(f: FillMode) -> &'static str {
    match f {
        FillMode::None => "none",
        FillMode::Forwards => "forwards",
        FillMode::Backwards => "backwards",
        FillMode::Both => "both",
    }
}

fn direction_css(d: AnimationDirection) -> &'static str {
    match d {
        AnimationDirection::Normal => "normal",
        AnimationDirection::Reverse => "reverse",
        AnimationDirection::Alternate => "alternate",
        AnimationDirection::AlternateReverse => "alternate-reverse",
    }
}

/// `EffectTiming` (Web Animations 1 §6.5.2) de `timing`.
fn timing_object(agent: &mut Agent, timing: &Timing) -> Gc<Object> {
    let o = agent.new_plain_object();
    put(agent, o, "delay", Value::Number(timing.delay));
    put(agent, o, "endDelay", Value::Number(0.0));
    put(agent, o, "fill", Value::str(fill_css(timing.fill)));
    put(agent, o, "iterationStart", Value::Number(0.0));
    put(agent, o, "iterations", Value::Number(timing.iterations));
    put(agent, o, "duration", Value::Number(timing.duration));
    put(
        agent,
        o,
        "direction",
        Value::str(direction_css(timing.direction)),
    );
    put(agent, o, "easing", Value::str(&timing.easing.to_string()));
    o
}

fn get_timing(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_effect(agent, a)?;
    let Some(t) = times(agent, id) else {
        return agent.type_error("The animation is no longer available");
    };
    Ok(Value::Object(timing_object(agent, &t.timing)))
}

/// `getComputedTiming()` (Web Animations 1 §6.5.3).
fn get_computed_timing(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_effect(agent, a)?;
    let Some(t) = times(agent, id) else {
        return agent.type_error("The animation is no longer available");
    };
    let now = document_time(agent);
    let o = timing_object(agent, &t.timing);
    put(agent, o, "endTime", Value::Number(t.timing.end_time()));
    put(
        agent,
        o,
        "activeDuration",
        Value::Number(t.timing.active_duration()),
    );
    let local = t.current_time(now);
    put(agent, o, "localTime", number_or_null(local));
    let ct = local.map(|l| t.timing.at(l, false));
    put(
        agent,
        o,
        "progress",
        number_or_null(ct.and_then(|c| c.progress)),
    );
    put(
        agent,
        o,
        "currentIteration",
        number_or_null(ct.and_then(|c| c.current_iteration)),
    );
    Ok(Value::Object(o))
}

// --- DocumentTimeline y getAnimations ---

/// `document.timeline` de la ventana en curso (siempre el mismo objeto).
fn timeline_object(agent: &mut Agent) -> Value {
    if let Some(h) = state_mut(agent).timeline
        && let Some(v) = agent.host_pinned(h)
    {
        return v;
    }
    let class = classes(agent).document_timeline;
    let o = Value::Object(agent.host_create(class, Box::new(())));
    let pin = agent.host_pin(o.clone());
    state_mut(agent).timeline = Some(pin);
    o
}

fn document_timeline(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = super::document::this_document(agent, a)?;
    if doc.doc != 0 {
        return Ok(Value::Null);
    }
    Ok(timeline_object(agent))
}

/// `AnimationTimeline.currentTime` (Web Animations 1 §4.3).
fn timeline_time(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(document_time(agent)))
}

/// Las animaciones relevantes del documento de la ventana para las que `keep(target)`, como
/// array de `Animation` en orden de composición.
fn animations_array(
    agent: &mut Agent,
    keep: impl Fn(&bf_dom::Document, Key) -> bool,
) -> JsResult<Value> {
    flush_styles(agent);
    let now = document_time(agent);
    let realm = WebContextHandle::current(agent).realm;
    let refs = {
        let st = state_mut(agent);
        std::mem::take(&mut st.animator)
    };
    let list = {
        let doc = WebContextHandle { realm }.get(agent).document();
        refs.relevant(doc, now)
            .into_iter()
            .filter(|r| keep(doc, r.target))
            .collect::<Vec<_>>()
    };
    state_mut(agent).animator = refs;
    let values: Vec<Value> = list
        .into_iter()
        .map(|r| Value::Object(wrap_animation(agent, realm, r.id, r.transition)))
        .collect();
    Ok(Value::Object(agent.create_array_from_list(&values)?))
}

/// `document.getAnimations()` (Web Animations 1 §6.13).
fn document_get_animations(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = super::document::this_document(agent, a)?;
    if doc.doc != 0 {
        return Ok(Value::Object(agent.create_array_from_list(&[])?));
    }
    animations_array(agent, |_, _| true)
}

/// `Element.getAnimations({subtree})` (Web Animations 1 §6.12): las del elemento y, con
/// `subtree`, también las de sus descendientes y seudoelementos.
fn element_get_animations(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = crate::dom::this_node(agent, a)?;
    let subtree = crate::dom::events::dict_bool(agent, &a.arg(0), "subtree")?;
    if n.doc != 0 {
        return Ok(Value::Object(agent.create_array_from_list(&[])?));
    }
    let node = n.node;
    animations_array(agent, move |doc, (target, pseudo)| {
        if subtree {
            target == node || doc.is_inclusive_ancestor(node, target)
        } else {
            target == node && pseudo.is_none()
        }
    })
}
