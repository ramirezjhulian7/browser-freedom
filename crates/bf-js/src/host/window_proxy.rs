//! E1: los métodos internos del `WindowProxy` (HTML §7.2.2, §7.2.2.1 a §7.2.2.10).
//!
//! Un `WindowProxy` es un `ObjectKind::Proxy` con `ProxyData::window` (ver
//! [`Agent::create_window_proxy`]): su `target` es el `[[Window]]`. En cada operación el host dice
//! si el acceso es del mismo origen (*IsPlatformObjectSameOrigin*, con el realm en curso) con
//! [`super::HostState::window_proxy_view`]:
//! - del mismo origen, la operación ordinaria sobre el `Window`, con el `WindowProxy` como
//!   *receiver*. Como dice el estándar, esto viola las invariantes de los métodos internos de
//!   ECMAScript (un `WindowProxy` deja ver propiedades no configurables del `Window` que cambian
//!   al navegar), y por eso no se comprueban como en un `Proxy`;
//! - de otro origen, la misma operación sobre el objeto restringido que da el host (sus
//!   *CrossOriginGet*, *CrossOriginSet*, *CrossOriginGetOwnPropertyHelper*…), también con el
//!   `WindowProxy` como *receiver*, para que `w.window === w`.
//!
//! Sin gancho del host, todo acceso es del mismo origen.

use crate::heap::Gc;
use crate::object::{Object, PropertyDescriptor};
use crate::value::{PropertyKey, Value};
use crate::vm::{Agent, JsResult};

/// Dónde va una operación sobre el `WindowProxy`.
enum Access {
    /// Del mismo origen: el `[[Window]]`.
    Same(Gc<Object>),
    /// De otro origen: el objeto restringido del host.
    Cross(Gc<Object>),
}

fn access(agent: &mut Agent, w: Gc<Object>) -> JsResult<Access> {
    let Some(view) = agent.host.window_proxy_view else {
        return Ok(Access::Same(w));
    };
    Ok(match view(agent, w)? {
        Some(v) => Access::Cross(v),
        None => Access::Same(w),
    })
}

/// El objeto sobre el que se hace la operación.
fn object(agent: &mut Agent, w: Gc<Object>) -> JsResult<Gc<Object>> {
    Ok(match access(agent, w)? {
        Access::Same(o) | Access::Cross(o) => o,
    })
}

/// `[[GetPrototypeOf]]` (§7.2.2.1): el del `Window`, o `null` entre orígenes.
pub(crate) fn get_prototype_of(agent: &mut Agent, w: Gc<Object>) -> JsResult<Option<Gc<Object>>> {
    match access(agent, w)? {
        Access::Same(w) => agent.get_prototype_of(w),
        Access::Cross(_) => Ok(None),
    }
}

/// `[[SetPrototypeOf]]` (§7.2.2.2): *SetImmutablePrototype*.
pub(crate) fn set_prototype_of(
    agent: &mut Agent,
    w: Gc<Object>,
    v: Option<Gc<Object>>,
) -> JsResult<bool> {
    Ok(get_prototype_of(agent, w)? == v)
}

/// `[[GetOwnProperty]]` (§7.2.2.5).
pub(crate) fn get_own_property(
    agent: &mut Agent,
    w: Gc<Object>,
    key: &PropertyKey,
) -> JsResult<Option<PropertyDescriptor>> {
    let o = object(agent, w)?;
    agent.get_own_property(o, key)
}

/// `[[DefineOwnProperty]]` (§7.2.2.6): del mismo origen, en el `Window` salvo los índices de
/// los marcos (`false`); entre orígenes, `SecurityError` (lo lanza la vista).
pub(crate) fn define_own_property(
    agent: &mut Agent,
    w: Gc<Object>,
    key: PropertyKey,
    desc: PropertyDescriptor,
) -> JsResult<bool> {
    match access(agent, w)? {
        Access::Same(_) if matches!(key, PropertyKey::Index(_)) => Ok(false),
        Access::Same(o) | Access::Cross(o) => agent.define_own_property(o, key, desc),
    }
}

/// `[[HasProperty]]` (por *OrdinaryHasProperty* con los métodos de arriba).
pub(crate) fn has(agent: &mut Agent, w: Gc<Object>, key: &PropertyKey) -> JsResult<bool> {
    let o = object(agent, w)?;
    agent.has_property(o, key)
}

/// `[[Get]]` (§7.2.2.7).
pub(crate) fn get(
    agent: &mut Agent,
    w: Gc<Object>,
    key: &PropertyKey,
    receiver: Value,
) -> JsResult<Value> {
    let o = object(agent, w)?;
    agent.get(o, key, receiver)
}

/// `[[Set]]` (§7.2.2.8).
pub(crate) fn set(
    agent: &mut Agent,
    w: Gc<Object>,
    key: PropertyKey,
    v: Value,
    receiver: Value,
) -> JsResult<bool> {
    let o = object(agent, w)?;
    agent.set(o, key, v, receiver)
}

/// `[[Delete]]` (§7.2.2.9): del mismo origen, un índice de marco no se borra.
pub(crate) fn delete(agent: &mut Agent, w: Gc<Object>, key: &PropertyKey) -> JsResult<bool> {
    match access(agent, w)? {
        Access::Same(o) if matches!(key, PropertyKey::Index(_)) => {
            Ok(agent.get_own_property(o, key)?.is_none())
        }
        Access::Same(o) | Access::Cross(o) => agent.delete(o, key),
    }
}

/// `[[OwnPropertyKeys]]` (§7.2.2.10).
pub(crate) fn own_keys(agent: &mut Agent, w: Gc<Object>) -> JsResult<Vec<PropertyKey>> {
    let o = object(agent, w)?;
    agent.own_property_keys(o)
}
