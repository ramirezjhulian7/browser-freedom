//! Custom elements (HTML §4.13). **E1.**

use bf_js::vm::Agent;

use crate::NodeRef;

/// ¿La definición del custom element de `n` deshabilita el shadow DOM
/// (`static disabledFeatures = ["shadow"]`)?
pub fn shadow_disabled(_agent: &Agent, _n: NodeRef) -> bool {
    false
}

/// ¿El *custom element state* de `n` es "precustomized" o "custom"?
pub fn is_custom(_agent: &Agent, _n: NodeRef) -> bool {
    false
}
