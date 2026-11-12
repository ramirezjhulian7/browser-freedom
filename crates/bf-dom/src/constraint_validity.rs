//! El resultado de la validación de restricciones que calcula el host (HTML §4.10.20.1,
//! "constraint validation"), para `:valid` y `:invalid` (HTML §4.16.3). **E10.**
//!
//! El casado de selectores (`bf_css`) calcula por su cuenta lo que se ve en el árbol (los
//! candidatos y el *value missing* de los controles nativos según sus atributos). Lo que solo
//! sabe el host, como los *validity flags* que pone `ElementInternals.setValidity()` en un
//! form-associated custom element (E1, en `bf-web`) o el *dirty value* de un control, lo
//! anota aquí con [`Document::set_constraint_validity`]; cada cambio real sube la versión del
//! documento para que el estilo se recalcule.

use crate::{Document, NodeId};

impl Document {
    /// ¿Satisface `node` sus restricciones, según el host? `None` si el host no lo anotó
    /// (entonces el casado de selectores usa su propio cálculo).
    pub fn constraint_validity(&self, node: NodeId) -> Option<bool> {
        self.constraint_validity.get(&node).copied()
    }

    /// Anota si `node` satisface sus restricciones (`Some(true)`), si sufre algún problema de
    /// validez (`Some(false)`), o quita la anotación (`None`). Que `node` sea candidato para
    /// la validación (no deshabilitado, sin `readonly`, fuera de un `datalist`…) lo sigue
    /// decidiendo el casado de selectores.
    pub fn set_constraint_validity(&mut self, node: NodeId, valid: Option<bool>) {
        let old = match valid {
            Some(v) => self.constraint_validity.insert(node, v),
            None => self.constraint_validity.remove(&node),
        };
        if old != valid {
            self.bump_version();
        }
    }
}
