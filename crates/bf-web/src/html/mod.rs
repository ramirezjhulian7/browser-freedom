//! Interfaces `HTML*Element` (HTML §3–4: reflexión de atributos) y el runner de
//! testharness.js. **Dueño: E1.**

use bf_js::vm::{Agent, RealmId};

use crate::WebContextHandle;

/// Registra las clases de esta área (lo llama [`crate::register_all`]).
pub fn register(_agent: &mut Agent, _realm: RealmId, _ctx: &WebContextHandle) {}
