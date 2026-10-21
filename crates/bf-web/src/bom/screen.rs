//! `Screen` (CSSOM View §4.3) y `ScreenOrientation` (Screen Orientation §3): las medidas de
//! la pantalla, de [`super::BomHooks::screen`].

use bf_js::host::{HostClass, HostClassId};
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use super::{ScreenInfo, bom_classes, state};

pub fn screen_class(event_target: HostClassId) -> HostClass {
    HostClass::new("Screen")
        .parent(event_target)
        .attribute("availWidth", Some(avail_width), None)
        .attribute("availHeight", Some(avail_height), None)
        .attribute("width", Some(width), None)
        .attribute("height", Some(height), None)
        .attribute("colorDepth", Some(color_depth), None)
        .attribute("pixelDepth", Some(color_depth), None)
        .attribute("availLeft", Some(avail_left), None)
        .attribute("availTop", Some(avail_top), None)
        .attribute("isExtended", Some(no), None)
        .attribute("orientation", Some(orientation), None)
}

pub fn orientation_class(event_target: HostClassId) -> HostClass {
    HostClass::new("ScreenOrientation")
        .parent(event_target)
        .attribute("type", Some(orientation_type), None)
        .attribute("angle", Some(orientation_angle), None)
        .method("lock", 1, lock)
        .method("unlock", 0, unlock)
}

fn info(agent: &Agent) -> ScreenInfo {
    state(agent).hooks.screen()
}

fn avail_width(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(info(agent).avail_width))
}

fn avail_height(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(info(agent).avail_height))
}

fn width(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(info(agent).width))
}

fn height(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(info(agent).height))
}

fn color_depth(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(f64::from(info(agent).color_depth)))
}

fn avail_left(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(info(agent).avail_left))
}

fn avail_top(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(info(agent).avail_top))
}

fn no(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(false))
}

/// `screen.orientation`: siempre el mismo objeto (en el slot 0 de `screen`).
fn orientation(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Value::Object(screen) = a.this else {
        return Ok(Value::Null);
    };
    if let Some(v) = agent
        .host_slots_mut(screen)
        .and_then(|s| s.first().cloned())
    {
        return Ok(v);
    }
    let class = bom_classes(agent).screen_orientation;
    let o = Value::Object(agent.host_create(class, Box::new(())));
    if let Some(slots) = agent.host_slots_mut(screen) {
        slots.push(o.clone());
    }
    Ok(o)
}

fn orientation_type(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let s = info(agent);
    Ok(Value::str(if s.width >= s.height {
        "landscape-primary"
    } else {
        "portrait-primary"
    }))
}

fn orientation_angle(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(0.0))
}

/// `lock()`: en escritorio no se puede bloquear la orientación (una promesa rechazada con
/// `NotSupportedError`).
fn lock(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let e = bf_dom::DomException::NotSupportedError;
    let err = crate::dom::exception::create(agent, e.name(), e.message());
    let ctor = agent.intrinsics().promise_ctor;
    let cap = agent.new_promise_capability_hidden(ctor)?;
    agent.capability_reject(&cap, err)?;
    Ok(cap.promise())
}

fn unlock(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Undefined)
}
