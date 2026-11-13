//! La *popover visibility state* de los elementos HTML (HTML §6.12 "The popover attribute"):
//! `showing` o `hidden`. **E5.**
//!
//! La guarda el DOM para que el casado de `:popover-open` (HTML §4.16.3, en `bf_css`) no dependa
//! del motor de JS; `showPopover()`/`hidePopover()`/`togglePopover()` (en `bf-web`) la cambian
//! con [`Document::set_popover_showing`]. Cada cambio real sube la versión del documento para
//! que el estilo se recalcule (la hoja UA oculta los popovers cerrados, HTML §15.3.1). Como el
//! *states set*, no se clona ni se importa: un elemento nuevo empieza en `hidden`.

use crate::{Document, NodeId};

impl Document {
    /// ¿Está `node` en la *popover visibility state* `showing`?
    pub fn is_popover_showing(&self, node: NodeId) -> bool {
        self.popover_showing.contains(&node)
    }

    /// Pone la *popover visibility state* de `node` en `showing` (`true`) o `hidden`
    /// (`false`). Devuelve `false` si ya estaba así.
    pub fn set_popover_showing(&mut self, node: NodeId, showing: bool) -> bool {
        let changed = if showing {
            self.popover_showing.insert(node)
        } else {
            self.popover_showing.remove(&node)
        };
        if changed {
            self.bump_version();
        }
        changed
    }
}
