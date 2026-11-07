//! Lo que el DOM guarda de los custom elements (HTML §4.13; DOM §4.9): el *custom element
//! state* y el *is value* de cada elemento, y los eventos que generan reacciones (conexión,
//! desconexión, cambio de atributo) para que el motor los procese. **E1.**

use std::collections::{HashMap, HashSet};

use crate::{Document, DomString, Namespace, NodeId};

/// El *custom element state* (DOM §4.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomElementState {
    Undefined,
    Failed,
    Uncustomized,
    Precustomized,
    Custom,
}

/// Un evento del árbol que puede generar una reacción de custom element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomElementEvent {
    /// El elemento quedó conectado (insertion steps, paso 7.7 de "insert").
    Connected(NodeId),
    /// El elemento dejó de estar conectado (paso 16 de "remove").
    Disconnected(NodeId),
    /// Cambió un atributo del elemento ("handle attribute changes").
    AttributeChanged {
        element: NodeId,
        local_name: DomString,
        namespace: Option<Namespace>,
        old_value: Option<DomString>,
        new_value: Option<DomString>,
    },
}

/// Las tablas del documento.
#[derive(Debug, Clone, Default)]
pub(crate) struct CustomState {
    /// Los estados explícitos (sin entrada: "uncustomized", o "undefined" si el nombre es de
    /// custom element o tiene *is value*).
    states: HashMap<NodeId, CustomElementState>,
    is_values: HashMap<NodeId, DomString>,
    /// Los elementos cuya definición es *form-associated* (HTML §4.13.7).
    form_associated: HashSet<NodeId>,
    /// ¿El motor quiere los eventos? (desde la primera definición).
    enabled: bool,
    events: Vec<CustomElementEvent>,
}

impl Document {
    /// El *custom element state* de `n`.
    pub fn custom_element_state(&self, n: NodeId) -> CustomElementState {
        if let Some(s) = self.custom.states.get(&n) {
            return *s;
        }
        let Some(e) = self.element(n) else {
            return CustomElementState::Uncustomized;
        };
        if e.ns == Namespace::Html
            && (crate::document::is_valid_custom_element_name(&e.name.to_string())
                || self.custom.is_values.contains_key(&n))
        {
            CustomElementState::Undefined
        } else {
            CustomElementState::Uncustomized
        }
    }

    pub fn set_custom_element_state(&mut self, n: NodeId, state: CustomElementState) {
        self.custom.states.insert(n, state);
        self.bump_version();
    }

    /// ¿Está *defined* (estado "uncustomized" o "custom")? Es `:defined`.
    pub fn is_defined_element(&self, n: NodeId) -> bool {
        matches!(
            self.custom_element_state(n),
            CustomElementState::Uncustomized | CustomElementState::Custom
        )
    }

    /// ¿Es un *custom element* (estado "custom")?
    pub fn is_custom_element(&self, n: NodeId) -> bool {
        self.custom_element_state(n) == CustomElementState::Custom
    }

    /// ¿Es un *form-associated custom element* (HTML §4.13.7: custom, con una definición
    /// *form-associated*)? Es *listed*, *labelable*, *submittable* y *resettable*.
    pub fn is_form_associated_custom_element(&self, n: NodeId) -> bool {
        self.custom.form_associated.contains(&n) && self.is_custom_element(n)
    }

    /// Marca (o desmarca) que la definición de `n` es *form-associated*.
    pub fn set_form_associated(&mut self, n: NodeId, value: bool) {
        if value {
            self.custom.form_associated.insert(n);
        } else {
            self.custom.form_associated.remove(&n);
        }
        self.bump_version();
    }

    /// El *is value* de `n`.
    pub fn is_value(&self, n: NodeId) -> Option<&DomString> {
        self.custom.is_values.get(&n)
    }

    pub fn set_is_value(&mut self, n: NodeId, value: Option<DomString>) {
        match value {
            Some(v) => {
                self.custom.is_values.insert(n, v);
            }
            None => {
                self.custom.is_values.remove(&n);
            }
        }
    }

    /// Activa el registro de eventos de custom elements (lo pide el motor al definir el
    /// primero).
    pub fn enable_custom_element_events(&mut self) {
        self.custom.enabled = true;
    }

    /// Los eventos pendientes.
    pub fn take_custom_element_events(&mut self) -> Vec<CustomElementEvent> {
        std::mem::take(&mut self.custom.events)
    }

    pub fn has_custom_element_events(&self) -> bool {
        !self.custom.events.is_empty()
    }

    /// ¿Puede `n` tener reacciones? (HTML, con nombre de custom element, *is value* o estado
    /// explícito.)
    fn ce_candidate(&self, n: NodeId) -> bool {
        let Some(e) = self.element(n) else {
            return false;
        };
        e.ns == Namespace::Html
            && (e.name.to_string().contains('-')
                || self.custom.is_values.contains_key(&n)
                || self.custom.states.contains_key(&n))
    }

    /// Paso 7.7 de "insert": los descendientes conectados.
    pub(crate) fn custom_connected(&mut self, node: NodeId) {
        if !self.custom.enabled || !self.is_connected(node) {
            return;
        }
        for n in self.shadow_including_inclusive_descendants(node) {
            if self.ce_candidate(n) {
                self.custom.events.push(CustomElementEvent::Connected(n));
            }
        }
    }

    /// Paso 16 de "remove": los descendientes desconectados.
    pub(crate) fn custom_disconnected(&mut self, node: NodeId, was_connected: bool) {
        if !self.custom.enabled || !was_connected {
            return;
        }
        for n in self.shadow_including_inclusive_descendants(node) {
            if self.ce_candidate(n) {
                self.custom.events.push(CustomElementEvent::Disconnected(n));
            }
        }
    }

    /// "Handle attribute changes", paso 2.
    pub(crate) fn custom_attribute_changed(
        &mut self,
        element: NodeId,
        local_name: &DomString,
        namespace: Option<Namespace>,
        old_value: Option<&DomString>,
    ) {
        if !self.custom.enabled || !self.is_custom_element(element) {
            return;
        }
        let new_value = self.element(element).and_then(|e| {
            e.attrs
                .iter()
                .find(|a| a.ns == namespace && a.name == *local_name)
                .map(|a| a.value.clone())
        });
        self.custom
            .events
            .push(CustomElementEvent::AttributeChanged {
                element,
                local_name: local_name.clone(),
                namespace,
                old_value: old_value.cloned(),
                new_value,
            });
    }

    /// Copia el estado y el *is value* de `node` de `other` a `copy` (adopción entre arenas).
    pub(crate) fn import_custom_state(&mut self, other: &Document, node: NodeId, copy: NodeId) {
        if let Some(s) = other.custom.states.get(&node) {
            self.custom.states.insert(copy, *s);
        }
        if let Some(v) = other.custom.is_values.get(&node) {
            self.custom.is_values.insert(copy, v.clone());
        }
        if other.custom.form_associated.contains(&node) {
            self.custom.form_associated.insert(copy);
        }
        if other.custom.enabled {
            self.custom.enabled = true;
        }
    }

    /// Copia el *is value* al clonar (paso 2 de "clone a single node").
    pub(crate) fn clone_custom_state(&mut self, from: NodeId, to: NodeId) {
        if let Some(v) = self.custom.is_values.get(&from).cloned() {
            self.custom.is_values.insert(to, v);
        }
    }
}
