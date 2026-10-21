//! `Window` (HTML §7.2–7.3) como objeto global del realm: la cadena
//! `Window.prototype → WindowProperties → EventTarget.prototype → Object.prototype`.

use bf_js::host::{HostClass, HostClassId};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, RealmId};

use crate::dom::{classes, wrap_node};
use crate::{NodeRef, WebContextHandle};

/// El payload del `Window`.
#[derive(Debug, Clone, Copy)]
pub struct WindowData {
    pub realm: RealmId,
}

pub fn window_class(event_target: HostClassId) -> HostClass {
    HostClass::new("Window")
        .parent(event_target)
        .unforgeable("window", Some(window_self), None)
        .unforgeable("document", Some(document), None)
        .attribute("self", Some(window_self), None)
        .attribute("frames", Some(window_self), None)
        .attribute("parent", Some(window_self), None)
        .attribute("top", Some(window_self), None)
}

/// Convierte el global de `realm` en el `Window` e inserta `WindowProperties`.
pub fn install(agent: &mut Agent, realm: RealmId, _ctx: &WebContextHandle) {
    let class = classes(agent).window;
    let g = agent.host_make_global(realm, class, Box::new(WindowData { realm }));
    // WindowProperties (HTML §7.2.3.1): entre Window.prototype y EventTarget.prototype.
    let wp = agent.heap[g].proto.expect("Window.prototype");
    let et = agent.heap[wp].proto;
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let props = agent.new_object(et);
    let tag = PropertyKey::Symbol(agent.well_known.to_string_tag);
    agent.put_data(
        props,
        tag,
        Value::str("WindowProperties"),
        false,
        false,
        true,
    );
    agent.heap[wp].proto = Some(props);
    agent.current_realm = saved;
}

/// `window`, `self`, `frames`, `parent`, `top`: el `WindowProxy` (sin iframes, el propio
/// global).
fn window_self(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let realm = WebContextHandle::current(agent).realm;
    Ok(Value::Object(agent.realms[realm].global_object))
}

/// `window.document`.
fn document(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let root = WebContextHandle::current(agent)
        .get(agent)
        .document()
        .root();
    Ok(wrap_node(agent, NodeRef::new(0, root)))
}
