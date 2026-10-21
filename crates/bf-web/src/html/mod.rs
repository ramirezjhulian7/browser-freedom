//! Interfaces `HTML*Element` (HTML §3–4: reflexión de atributos) y el runner de
//! testharness.js. **Dueño: E1.**

pub mod activation;
pub mod canvas;
pub mod elements;
pub mod forms;
pub mod misc;
pub mod reflect;
pub mod scripting;
pub mod submission;
pub mod table;
pub mod tables;
pub mod testharness;

use bf_js::vm::{Agent, RealmId};

use crate::WebContextHandle;

/// Registra las clases de esta área (lo llama [`crate::register_all`]).
pub fn register(agent: &mut Agent, realm: RealmId, ctx: &WebContextHandle) {
    canvas::register(agent);
    submission::register(agent);
    elements::register(agent, realm, ctx);
}
