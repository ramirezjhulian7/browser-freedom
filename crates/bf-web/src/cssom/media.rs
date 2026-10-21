//! `window.matchMedia()` y `MediaQueryList` (CSSOM View §4.2).
//!
//! `matches` se evalúa en cada lectura contra el viewport actual. Los listeners (`change`,
//! `addListener`) se guardan pero, sin redimensionado desde el shell, no se disparan.

use bf_css::media::parse_media_query_list;
use bf_js::host::{HostClass, HostClassId};
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use super::{classes, media_context};

/// El payload de un `MediaQueryList`: el texto serializado de la consulta.
#[derive(Debug, Clone)]
struct MediaQueryPayload(String);

pub fn register_class(agent: &mut Agent) -> HostClassId {
    let event_target = crate::dom::classes(agent).event_target;
    agent.register_host_class(
        HostClass::new("MediaQueryList")
            .parent(event_target)
            .attribute("media", Some(media), None)
            .attribute("matches", Some(matches), None)
            .method("addListener", 1, add_listener)
            .method("removeListener", 1, remove_listener)
            .attribute("onchange", Some(get_onchange), Some(set_onchange)),
    )
}

/// `partial interface Window { [NewObject] MediaQueryList matchMedia(CSSOMString query); }`.
pub fn window_partial() -> HostClass {
    HostClass::new("Window").method("matchMedia", 1, match_media)
}

fn match_media(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let q = agent.to_string(&a.arg(0))?.to_string();
    // "Parse a media query list" y serializarla (CSSOM §4.1).
    let text = if q.trim().is_empty() {
        String::new()
    } else {
        super::value::media_list_text(&q)
    };
    let class = classes(agent).media_query_list;
    let o = agent.host_create(class, Box::new(MediaQueryPayload(text)));
    if let Some(slots) = agent.host_slots_mut(o) {
        slots.push(Value::Null);
    }
    Ok(Value::Object(o))
}

fn this_query(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<String> {
    match agent.host_unwrap::<MediaQueryPayload>(&a.this) {
        Some(p) => Ok(p.0.clone()),
        None => agent.type_error("Illegal invocation"),
    }
}

fn media(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let q = this_query(agent, a)?;
    Ok(Value::str(&q))
}

fn matches(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let q = this_query(agent, a)?;
    let ctx = media_context(agent);
    Ok(Value::Boolean(parse_media_query_list(&q).evaluate(&ctx)))
}

/// `addListener(callback)`: alias de `addEventListener("change", callback)`.
fn add_listener(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_query(agent, a)?;
    forward(agent, a, "addEventListener")
}

fn remove_listener(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_query(agent, a)?;
    forward(agent, a, "removeEventListener")
}

fn forward(agent: &mut Agent, a: &CallArgs<'_>, method: &str) -> JsResult<Value> {
    let cb = a.arg(0);
    if cb.is_nullish() {
        return Ok(Value::Undefined);
    }
    let Value::Object(o) = a.this.clone() else {
        return Ok(Value::Undefined);
    };
    let f = agent.get(o, &method.into(), a.this.clone())?;
    if agent.is_callable(&f) {
        agent.call(&f, a.this.clone(), &[Value::str("change"), cb])?;
    }
    Ok(Value::Undefined)
}

fn get_onchange(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_query(agent, a)?;
    let Value::Object(o) = a.this.clone() else {
        return Ok(Value::Null);
    };
    Ok(agent
        .host_slots_mut(o)
        .and_then(|s| s.first().cloned())
        .unwrap_or(Value::Null))
}

fn set_onchange(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_query(agent, a)?;
    let Value::Object(o) = a.this.clone() else {
        return Ok(Value::Undefined);
    };
    let v = a.arg(0);
    let v = if agent.is_callable(&v) {
        v
    } else {
        Value::Null
    };
    if let Some(s) = agent.host_slots_mut(o) {
        if s.is_empty() {
            s.push(v);
        } else {
            s[0] = v;
        }
    }
    Ok(Value::Undefined)
}
