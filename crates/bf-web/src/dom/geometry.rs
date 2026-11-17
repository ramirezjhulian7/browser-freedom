//! E1: Geometry Interfaces Module Level 1: `DOMPointReadOnly`, `DOMPoint`, `DOMQuad`,
//! `DOMMatrixReadOnly` y `DOMMatrix` (`geometry.js`), en ventanas, marcos y workers
//! (`[Exposed=(Window,Worker)]`). `DOMRect` y `DOMRectReadOnly` son clases del host
//! (`cssom::view`).

use bf_js::value::Value;
use bf_js::vm::{Agent, RealmId};

/// El JavaScript de las interfaces: una función `(global, isWindow)`.
pub const GEOMETRY_JS: &str = include_str!("geometry.js");

/// Evalúa `geometry.js` en `realm`. `window`: el realm es una ventana (el constructor acepta una
/// cadena con una `<transform-list>` y existe el alias `WebKitCSSMatrix`).
pub fn install(agent: &mut Agent, realm: RealmId, window: bool) {
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let g = Value::Object(agent.realms[realm].global_object);
    if let Ok(f) = agent.eval_script_cached(GEOMETRY_JS, "bf-web:geometry.js") {
        let _ = agent.call(&f, Value::Undefined, &[g, Value::Boolean(window)]);
    }
    agent.current_realm = saved;
}
