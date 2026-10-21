//! El Browser Object Model: `navigator`, `location`, `history`, `screen`, `localStorage`/
//! `sessionStorage`, `document.cookie` y los diálogos. **Dueño: E4.**

use bf_js::vm::{Agent, RealmId};

use crate::WebContextHandle;

/// Registra las clases de esta área (lo llama [`crate::register_all`]).
pub fn register(_agent: &mut Agent, _realm: RealmId, _ctx: &WebContextHandle) {}
