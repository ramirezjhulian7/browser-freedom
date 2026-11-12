//! El *states set* de los custom elements (HTML §4.13.7.4 "Custom state pseudo-class"): los
//! estados que un custom element expone con `ElementInternals.states` (`CustomStateSet`) y
//! que casa la pseudo-clase `:state(<ident>)`. **E10.**
//!
//! El DOM guarda una copia de los estados de cada elemento para que el casado de selectores
//! (`bf_css`) no dependa del motor de JS. Quien implementa `CustomStateSet` (E1, en `bf-web`)
//! llama a [`Document::add_custom_state`], [`Document::delete_custom_state`] y
//! [`Document::clear_custom_states`] cuando cambia el set; cada cambio real sube la versión del
//! documento para que el estilo se recalcule. Los estados no se clonan ni se importan: el
//! `CustomStateSet` es de las `ElementInternals` del elemento, no del nodo.

use crate::{Document, DomString, NodeId};

impl Document {
    /// Los estados del *states set* de `node`, en orden de inserción (vacío si no tiene).
    pub fn custom_states(&self, node: NodeId) -> &[DomString] {
        self.custom_states.get(&node).map_or(&[], Vec::as_slice)
    }

    /// ¿Contiene el *states set* de `node` el estado `state`? La comparación distingue
    /// mayúsculas (es un `<ident>` de CSS que casa con un valor de un `Set`).
    pub fn has_custom_state(&self, node: NodeId, state: &str) -> bool {
        self.custom_states(node).iter().any(|s| *s == state)
    }

    /// Agrega `state` al *states set* de `node` (`CustomStateSet.add`). Devuelve `false` si
    /// ya estaba.
    pub fn add_custom_state(&mut self, node: NodeId, state: impl Into<DomString>) -> bool {
        let state = state.into();
        let set = self.custom_states.entry(node).or_default();
        if set.contains(&state) {
            return false;
        }
        set.push(state);
        self.bump_version();
        true
    }

    /// Quita `state` del *states set* de `node` (`CustomStateSet.delete`). Devuelve `false` si
    /// no estaba.
    pub fn delete_custom_state(&mut self, node: NodeId, state: &str) -> bool {
        let Some(set) = self.custom_states.get_mut(&node) else {
            return false;
        };
        let Some(i) = set.iter().position(|s| *s == state) else {
            return false;
        };
        set.remove(i);
        if set.is_empty() {
            self.custom_states.remove(&node);
        }
        self.bump_version();
        true
    }

    /// Vacía el *states set* de `node` (`CustomStateSet.clear`).
    pub fn clear_custom_states(&mut self, node: NodeId) {
        if self.custom_states.remove(&node).is_some() {
            self.bump_version();
        }
    }
}
