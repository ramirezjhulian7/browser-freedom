//! `MutationObserver` (DOM §4.3): registro de observadores en los nodos, *queue a mutation
//! record* (lo llaman todas las mutaciones) y la cola de records de cada observador.
//!
//! La entrega ("notify mutation observers") la hace el event loop: [`Document::notify_mutation_observers`]
//! devuelve, por observador, los records a pasarle al callback (y quita los registros
//! transitorios), y [`Document::mutation_observer_microtask_queued`] dice si hay una entrega
//! pendiente.

use std::collections::HashMap;

use bf_infra::{DomString, Namespace};

use crate::{Document, DomException, NodeId};

/// Identificador de un `MutationObserver`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObserverId(pub u32);

/// `MutationObserverInit`: los campos opcionales son `None` cuando el diccionario los omite.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObserverInit {
    pub child_list: bool,
    pub attributes: Option<bool>,
    pub character_data: Option<bool>,
    pub subtree: bool,
    pub attribute_old_value: Option<bool>,
    pub character_data_old_value: Option<bool>,
    pub attribute_filter: Option<Vec<DomString>>,
}

/// Opciones ya normalizadas por `observe()`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObserverOptions {
    pub child_list: bool,
    pub attributes: bool,
    pub character_data: bool,
    pub subtree: bool,
    pub attribute_old_value: bool,
    pub character_data_old_value: bool,
    pub attribute_filter: Option<Vec<DomString>>,
}

/// Tipo de un `MutationRecord`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationType {
    Attributes,
    CharacterData,
    ChildList,
}

impl MutationType {
    pub fn as_str(self) -> &'static str {
        match self {
            MutationType::Attributes => "attributes",
            MutationType::CharacterData => "characterData",
            MutationType::ChildList => "childList",
        }
    }
}

/// `MutationRecord`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationRecord {
    pub type_: MutationType,
    pub target: NodeId,
    pub added_nodes: Vec<NodeId>,
    pub removed_nodes: Vec<NodeId>,
    pub previous_sibling: Option<NodeId>,
    pub next_sibling: Option<NodeId>,
    pub attribute_name: Option<DomString>,
    pub attribute_namespace: Option<Namespace>,
    pub old_value: Option<DomString>,
}

/// Un registered observer (o uno transitorio, con su `source`).
#[derive(Debug, Clone)]
struct Registration {
    observer: ObserverId,
    options: ObserverOptions,
    /// Para un transient registered observer: el nodo y el registro del que salió.
    source: Option<NodeId>,
}

#[derive(Debug, Clone, Default)]
struct ObserverState {
    records: Vec<MutationRecord>,
    /// Nodos en los que está registrado (node list).
    nodes: Vec<NodeId>,
}

/// Estado de los observadores del documento.
#[derive(Debug, Clone, Default)]
pub(crate) struct Observers {
    observers: Vec<ObserverState>,
    registered: HashMap<NodeId, Vec<Registration>>,
    /// "mutation observer microtask queued".
    microtask_queued: bool,
}

impl Observers {
    pub(crate) fn has_any(&self) -> bool {
        !self.registered.is_empty()
    }
}

/// Datos de una mutación para *queue a mutation record*.
pub(crate) struct Mutation<'a> {
    pub type_: MutationType,
    pub target: NodeId,
    pub name: Option<&'a DomString>,
    pub namespace: Option<Namespace>,
    pub old_value: Option<&'a DomString>,
    pub added: &'a [NodeId],
    pub removed: &'a [NodeId],
    pub previous: Option<NodeId>,
    pub next: Option<NodeId>,
}

impl Document {
    /// `new MutationObserver(callback)`: el callback lo guarda quien llama, indexado por el id.
    pub fn create_mutation_observer(&mut self) -> ObserverId {
        let id = ObserverId(u32::try_from(self.observers.observers.len()).expect("observers"));
        self.observers.observers.push(ObserverState::default());
        id
    }

    /// `observe(target, options)` (§4.3.1).
    pub fn observe(
        &mut self,
        observer: ObserverId,
        target: NodeId,
        init: ObserverInit,
    ) -> Result<(), DomException> {
        let mut attributes = init.attributes;
        let mut character_data = init.character_data;
        // 1-2.
        if (init.attribute_old_value.is_some() || init.attribute_filter.is_some())
            && attributes.is_none()
        {
            attributes = Some(true);
        }
        if init.character_data_old_value.is_some() && character_data.is_none() {
            character_data = Some(true);
        }
        let attributes = attributes.unwrap_or(false);
        let character_data = character_data.unwrap_or(false);
        // 3-6.
        if !init.child_list && !attributes && !character_data {
            return Err(DomException::TypeError);
        }
        if init.attribute_old_value == Some(true) && !attributes {
            return Err(DomException::TypeError);
        }
        if init.attribute_filter.is_some() && !attributes {
            return Err(DomException::TypeError);
        }
        if init.character_data_old_value == Some(true) && !character_data {
            return Err(DomException::TypeError);
        }
        let options = ObserverOptions {
            child_list: init.child_list,
            attributes,
            character_data,
            subtree: init.subtree,
            attribute_old_value: init.attribute_old_value.unwrap_or(false),
            character_data_old_value: init.character_data_old_value.unwrap_or(false),
            attribute_filter: init.attribute_filter,
        };
        // 7. Si ya estaba registrado en `target`, se reemplazan las opciones y se quitan los
        // transitorios que salieron de ese registro.
        let already = self
            .observers
            .registered
            .get(&target)
            .is_some_and(|regs| regs.iter().any(|r| r.observer == observer && r.source.is_none()));
        if already {
            let nodes = self.observers.observers[observer.0 as usize].nodes.clone();
            for node in nodes {
                if let Some(regs) = self.observers.registered.get_mut(&node) {
                    regs.retain(|r| !(r.observer == observer && r.source == Some(target)));
                }
            }
            if let Some(regs) = self.observers.registered.get_mut(&target) {
                for r in regs
                    .iter_mut()
                    .filter(|r| r.observer == observer && r.source.is_none())
                {
                    r.options = options.clone();
                }
            }
            return Ok(());
        }
        // 8.
        self.observers
            .registered
            .entry(target)
            .or_default()
            .push(Registration {
                observer,
                options,
                source: None,
            });
        self.observers.observers[observer.0 as usize].nodes.push(target);
        Ok(())
    }

    /// `disconnect()`.
    pub fn disconnect_observer(&mut self, observer: ObserverId) {
        let state = &mut self.observers.observers[observer.0 as usize];
        let nodes = std::mem::take(&mut state.nodes);
        state.records.clear();
        for node in nodes {
            if let Some(regs) = self.observers.registered.get_mut(&node) {
                regs.retain(|r| r.observer != observer);
                if regs.is_empty() {
                    self.observers.registered.remove(&node);
                }
            }
        }
    }

    /// `takeRecords()`.
    pub fn take_records(&mut self, observer: ObserverId) -> Vec<MutationRecord> {
        std::mem::take(&mut self.observers.observers[observer.0 as usize].records)
    }

    /// ¿Hay un "mutation observer microtask" pendiente?
    pub fn mutation_observer_microtask_queued(&self) -> bool {
        self.observers.microtask_queued
    }

    /// "Notify mutation observers" (§4.3, pasos sin el callback): devuelve los observadores
    /// con records, en orden de creación, y sus records; quita los registros transitorios.
    pub fn notify_mutation_observers(&mut self) -> Vec<(ObserverId, Vec<MutationRecord>)> {
        self.observers.microtask_queued = false;
        let mut out = Vec::new();
        for i in 0..self.observers.observers.len() {
            let id = ObserverId(i as u32);
            let records = std::mem::take(&mut self.observers.observers[i].records);
            let nodes = self.observers.observers[i].nodes.clone();
            for node in nodes {
                if let Some(regs) = self.observers.registered.get_mut(&node) {
                    regs.retain(|r| !(r.observer == id && r.source.is_some()));
                }
            }
            if !records.is_empty() {
                out.push((id, records));
            }
        }
        out
    }

    /// Paso de *remove* (§4.2.3): por cada ancestro inclusivo de `parent` con observadores
    /// `subtree`, un registro transitorio en `node`.
    pub(crate) fn add_transient_observers(&mut self, parent: NodeId, node: NodeId) {
        if !self.observers.has_any() {
            return;
        }
        let mut transient = Vec::new();
        for ancestor in self.inclusive_ancestors(parent) {
            if let Some(regs) = self.observers.registered.get(&ancestor) {
                for r in regs.iter().filter(|r| r.options.subtree) {
                    transient.push(Registration {
                        observer: r.observer,
                        options: r.options.clone(),
                        source: Some(r.source.unwrap_or(ancestor)),
                    });
                }
            }
        }
        if transient.is_empty() {
            return;
        }
        for r in &transient {
            let nodes = &mut self.observers.observers[r.observer.0 as usize].nodes;
            if !nodes.contains(&node) {
                nodes.push(node);
            }
        }
        self.observers
            .registered
            .entry(node)
            .or_default()
            .extend(transient);
    }

    /// "Queue a mutation record" (§4.3.2).
    pub(crate) fn queue_mutation_record(&mut self, m: Mutation<'_>) {
        if !self.observers.has_any() {
            return;
        }
        // 1-3: observadores interesados y su old value.
        let mut interested: Vec<(ObserverId, Option<DomString>)> = Vec::new();
        for node in self.inclusive_ancestors(m.target) {
            let Some(regs) = self.observers.registered.get(&node) else {
                continue;
            };
            for r in regs {
                let o = &r.options;
                let skip = (node != m.target && !o.subtree)
                    || (m.type_ == MutationType::Attributes && !o.attributes)
                    || (m.type_ == MutationType::Attributes
                        && o.attribute_filter.as_ref().is_some_and(|f| {
                            m.namespace.is_some() || !m.name.is_some_and(|n| f.contains(n))
                        }))
                    || (m.type_ == MutationType::CharacterData && !o.character_data)
                    || (m.type_ == MutationType::ChildList && !o.child_list);
                if skip {
                    continue;
                }
                let idx = match interested.iter().position(|(id, _)| *id == r.observer) {
                    Some(i) => i,
                    None => {
                        interested.push((r.observer, None));
                        interested.len() - 1
                    }
                };
                if (m.type_ == MutationType::Attributes && o.attribute_old_value)
                    || (m.type_ == MutationType::CharacterData && o.character_data_old_value)
                {
                    interested[idx].1 = m.old_value.cloned();
                }
            }
        }
        // 4.
        for (observer, old_value) in interested {
            let record = MutationRecord {
                type_: m.type_,
                target: m.target,
                added_nodes: m.added.to_vec(),
                removed_nodes: m.removed.to_vec(),
                previous_sibling: m.previous,
                next_sibling: m.next,
                attribute_name: m.name.cloned(),
                attribute_namespace: m.namespace,
                old_value,
            };
            self.observers.observers[observer.0 as usize]
                .records
                .push(record);
        }
        // 5. Queue a mutation observer microtask.
        self.observers.microtask_queued = true;
    }

    /// "Queue a tree mutation record".
    pub(crate) fn queue_tree_mutation_record(
        &mut self,
        target: NodeId,
        added: &[NodeId],
        removed: &[NodeId],
        previous: Option<NodeId>,
        next: Option<NodeId>,
    ) {
        debug_assert!(!added.is_empty() || !removed.is_empty());
        self.queue_mutation_record(Mutation {
            type_: MutationType::ChildList,
            target,
            name: None,
            namespace: None,
            old_value: None,
            added,
            removed,
            previous,
            next,
        });
    }
}
