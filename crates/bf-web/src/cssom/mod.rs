//! CSSOM y CSSOM View: `CSSStyleDeclaration`, `getComputedStyle`, `document.styleSheets` y
//! la geometría (`getBoundingClientRect`, `offset*`, `scroll*`). **Dueño: E2.**

use bf_js::vm::{Agent, RealmId};

use crate::WebContextHandle;

/// El resultado del último layout que necesitan las APIs de geometría (lo define E2).
#[derive(Debug, Default)]
pub struct LayoutSnapshot {}

/// Registra las clases de esta área (lo llama [`crate::register_all`]).
pub fn register(_agent: &mut Agent, _realm: RealmId, _ctx: &WebContextHandle) {}
