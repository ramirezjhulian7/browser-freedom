//! Shadow trees (DOM §4.2.2, §4.8 "attach a shadow root") y slots (§4.2.2.3–4.2.2.4). **E1.**
//!
//! Un shadow root es un nodo `DocumentFragment` de la misma arena sin padre; lo que lo hace
//! shadow root (su host, modo, flags) y las asignaciones de los slots viven en tablas aparte
//! ([`ShadowState`]). Incluye los conceptos "shadow-including" (raíz, ancestros, orden), el
//! *retargeting* y el *flat tree* (CSS Scoping §2.2) que usan el estilo y el layout.

use std::collections::HashMap;

use crate::events::EventTarget;
use crate::{Document, DomException, DomString, Namespace, NodeData, NodeId};

/// `ShadowRootMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadowRootMode {
    Open,
    Closed,
}

/// `SlotAssignmentMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SlotAssignmentMode {
    #[default]
    Named,
    Manual,
}

/// Los datos de un shadow root (DOM §4.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowRootData {
    pub host: NodeId,
    pub mode: ShadowRootMode,
    pub delegates_focus: bool,
    pub slot_assignment: SlotAssignmentMode,
    pub clonable: bool,
    pub serializable: bool,
    pub declarative: bool,
    pub available_to_element_internals: bool,
}

/// `ShadowRootInit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadowRootInit {
    pub mode: ShadowRootMode,
    pub delegates_focus: bool,
    pub slot_assignment: SlotAssignmentMode,
    pub clonable: bool,
    pub serializable: bool,
}

impl Default for ShadowRootInit {
    fn default() -> Self {
        ShadowRootInit {
            mode: ShadowRootMode::Open,
            delegates_focus: false,
            slot_assignment: SlotAssignmentMode::Named,
            clonable: false,
            serializable: false,
        }
    }
}

/// Las tablas de shadow roots y slots de un documento.
#[derive(Debug, Clone, Default)]
pub(crate) struct ShadowState {
    roots: HashMap<NodeId, ShadowRootData>,
    by_host: HashMap<NodeId, NodeId>,
    /// Los *assigned nodes* de cada slot.
    assigned_nodes: HashMap<NodeId, Vec<NodeId>>,
    /// El *assigned slot* de cada slottable.
    assigned_slot: HashMap<NodeId, NodeId>,
    /// Los *manually assigned nodes* de cada slot.
    manual_nodes: HashMap<NodeId, Vec<NodeId>>,
    /// La *manual slot assignment* de cada slottable.
    manual_slot: HashMap<NodeId, NodeId>,
    /// La *signal slots* del agente (aquí, del documento): los slots con `slotchange`
    /// pendiente.
    signal_slots: Vec<NodeId>,
    /// El texto de las `adoptedStyleSheets` de cada `DocumentOrShadowRoot` (la raíz del
    /// documento o un shadow root), para la cascada.
    adopted: HashMap<NodeId, Vec<String>>,
}

/// ¿Es un *valid shadow host name* (DOM §4.8)?
fn is_valid_shadow_host_name(name: &str) -> bool {
    matches!(
        name,
        "article"
            | "aside"
            | "blockquote"
            | "body"
            | "div"
            | "footer"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "main"
            | "nav"
            | "p"
            | "section"
            | "span"
    ) || crate::document::is_valid_custom_element_name(name)
}

impl Document {
    // --- Shadow roots ---

    /// "Attach a shadow root" (DOM §4.8) a `host`, salvo el chequeo de la definición del
    /// custom element (*disable shadow*), que hace el motor antes.
    pub fn attach_shadow(
        &mut self,
        host: NodeId,
        init: ShadowRootInit,
    ) -> Result<NodeId, DomException> {
        let Some(e) = self.element(host) else {
            return Err(DomException::NotSupportedError);
        };
        // 1-2.
        if e.ns != Namespace::Html || !is_valid_shadow_host_name(&e.name.to_string()) {
            return Err(DomException::NotSupportedError);
        }
        // 4.
        if let Some(current) = self.shadow_root(host) {
            let data = &self.shadow.roots[&current];
            if !data.declarative || data.mode != init.mode {
                return Err(DomException::NotSupportedError);
            }
            for c in self.child_nodes(current) {
                self.remove_node(c, false);
            }
            if let Some(d) = self.shadow.roots.get_mut(&current) {
                d.declarative = false;
            }
            return Ok(current);
        }
        // 5-12.
        let root = self.create(NodeData::DocumentFragment);
        self.shadow.roots.insert(
            root,
            ShadowRootData {
                host,
                mode: init.mode,
                delegates_focus: init.delegates_focus,
                slot_assignment: init.slot_assignment,
                clonable: init.clonable,
                serializable: init.serializable,
                declarative: false,
                available_to_element_internals: false,
            },
        );
        self.shadow.by_host.insert(host, root);
        self.bump_version();
        Ok(root)
    }

    /// El shadow root de `host`.
    pub fn shadow_root(&self, host: NodeId) -> Option<NodeId> {
        self.shadow.by_host.get(&host).copied()
    }

    /// Los datos del shadow root `root` (`None` si no es un shadow root).
    pub fn shadow_root_data(&self, root: NodeId) -> Option<&ShadowRootData> {
        self.shadow.roots.get(&root)
    }

    pub fn shadow_root_data_mut(&mut self, root: NodeId) -> Option<&mut ShadowRootData> {
        self.shadow.roots.get_mut(&root)
    }

    /// ¿Es `n` un shadow root?
    pub fn is_shadow_root(&self, n: NodeId) -> bool {
        self.shadow.roots.contains_key(&n)
    }

    /// El host del shadow root `root`.
    pub fn shadow_host(&self, root: NodeId) -> Option<NodeId> {
        self.shadow.roots.get(&root).map(|d| d.host)
    }

    /// Los shadow roots del documento, en orden de creación.
    pub fn shadow_roots(&self) -> Vec<NodeId> {
        let mut v: Vec<NodeId> = self.shadow.roots.keys().copied().collect();
        v.sort();
        v
    }

    /// La profundidad del árbol de `n`: 0 en el documento (o un árbol suelto), 1 en el
    /// shadow tree de un host del documento…
    pub fn shadow_depth(&self, n: NodeId) -> u8 {
        let mut depth = 0u8;
        let mut r = self.tree_root(n);
        while let Some(h) = self.shadow_host(r) {
            depth = depth.saturating_add(1);
            r = self.tree_root(h);
        }
        depth
    }

    /// Fija el texto de las `adoptedStyleSheets` de `root` (el documento o un shadow root).
    pub fn set_adopted_style_texts(&mut self, root: NodeId, texts: Vec<String>) {
        if texts.is_empty() {
            self.shadow.adopted.remove(&root);
        } else {
            self.shadow.adopted.insert(root, texts);
        }
        self.bump_version();
    }

    /// El texto de las `adoptedStyleSheets` de `root`.
    pub fn adopted_style_texts(&self, root: NodeId) -> &[String] {
        self.shadow
            .adopted
            .get(&root)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// ¿Hay algún shadow root en el documento?
    pub fn has_shadow_roots(&self) -> bool {
        !self.shadow.roots.is_empty()
    }

    /// El shadow root en el que está `n` (la raíz de su árbol, si es un shadow root).
    pub fn containing_shadow_root(&self, n: NodeId) -> Option<NodeId> {
        let r = self.tree_root(n);
        self.is_shadow_root(r).then_some(r)
    }

    /// "Shadow-including root": la raíz, o la shadow-including root del host si la raíz es
    /// un shadow root.
    pub fn shadow_including_root(&self, n: NodeId) -> NodeId {
        let mut r = self.tree_root(n);
        while let Some(h) = self.shadow_host(r) {
            r = self.tree_root(h);
        }
        r
    }

    /// `getRootNode({composed})`.
    pub fn get_root_node_composed(&self, n: NodeId, composed: bool) -> NodeId {
        if composed {
            self.shadow_including_root(n)
        } else {
            self.tree_root(n)
        }
    }

    /// ¿Es `a` un *shadow-including inclusive ancestor* de `b`?
    pub fn is_shadow_including_inclusive_ancestor(&self, a: NodeId, b: NodeId) -> bool {
        let mut current = b;
        loop {
            if self.is_inclusive_ancestor(a, current) {
                return true;
            }
            match self.shadow_host(self.tree_root(current)) {
                Some(h) => current = h,
                None => return false,
            }
        }
    }

    /// Los *shadow-including inclusive descendants* de `n` en *shadow-including tree
    /// order* (el shadow root de un host va justo después del host).
    pub fn shadow_including_inclusive_descendants(&self, n: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        if self.shadow.roots.is_empty() {
            return self.inclusive_descendants(n);
        }
        self.push_shadow_including(n, &mut out);
        out
    }

    fn push_shadow_including(&self, n: NodeId, out: &mut Vec<NodeId>) {
        out.push(n);
        if let Some(sr) = self.shadow_root(n) {
            self.push_shadow_including(sr, out);
        }
        let mut c = self.get(n).first_child();
        while let Some(child) = c {
            self.push_shadow_including(child, out);
            c = self.get(child).next_sibling();
        }
    }

    /// "Retarget" `a` contra `b` (DOM §4.8): mientras la raíz de `a` sea un shadow root que
    /// no es shadow-including inclusive ancestor de `b`, `a` pasa a ser su host.
    pub fn retarget(&self, a: Option<EventTarget>, b: EventTarget) -> Option<EventTarget> {
        let mut a = a?;
        loop {
            let EventTarget::Node(an) = a else {
                return Some(a);
            };
            let root = self.tree_root(an);
            let Some(host) = self.shadow_host(root) else {
                return Some(a);
            };
            if let EventTarget::Node(bn) = b
                && self.is_shadow_including_inclusive_ancestor(root, bn)
            {
                return Some(a);
            }
            a = EventTarget::Node(host);
        }
    }

    /// ¿Está `a` *closed-shadow-hidden* de `b`?
    pub fn is_closed_shadow_hidden(&self, a: NodeId, b: NodeId) -> bool {
        let root = self.tree_root(a);
        let Some(data) = self.shadow_root_data(root) else {
            return false;
        };
        if self.is_shadow_including_inclusive_ancestor(root, b) {
            return false;
        }
        data.mode == ShadowRootMode::Closed || self.is_closed_shadow_hidden(data.host, b)
    }

    /// Quita el shadow root de `host` (solo para deshacer un attach que falló a medias).
    pub fn detach_shadow(&mut self, host: NodeId) {
        if let Some(r) = self.shadow.by_host.remove(&host) {
            self.shadow.roots.remove(&r);
            self.bump_version();
        }
    }

    // --- Slots (§4.2.2) ---

    /// ¿Es un `slot` (HTML)?
    pub fn is_slot(&self, n: NodeId) -> bool {
        self.element(n).is_some_and(|e| e.is_html("slot"))
    }

    /// ¿Es un *slottable* (elemento o texto)?
    pub fn is_slottable(&self, n: NodeId) -> bool {
        matches!(self.get(n).data, NodeData::Element(_) | NodeData::Text(_))
    }

    /// El *name* de un slot (atributo `name`) o de un slottable (atributo `slot`).
    fn slot_name_attr(&self, n: NodeId, attr: &str) -> DomString {
        self.element(n)
            .and_then(|e| {
                e.attrs
                    .iter()
                    .find(|a| a.ns.is_none() && a.name == attr)
                    .map(|a| a.value.clone())
            })
            .unwrap_or_default()
    }

    /// El *assigned slot* de un slottable.
    pub fn assigned_slot(&self, n: NodeId) -> Option<NodeId> {
        self.shadow.assigned_slot.get(&n).copied()
    }

    /// Los *assigned nodes* de un slot.
    pub fn assigned_nodes(&self, slot: NodeId) -> Vec<NodeId> {
        self.shadow
            .assigned_nodes
            .get(&slot)
            .cloned()
            .unwrap_or_default()
    }

    /// Los *manually assigned nodes* de un slot.
    pub fn manually_assigned_nodes(&self, slot: NodeId) -> Vec<NodeId> {
        self.shadow
            .manual_nodes
            .get(&slot)
            .cloned()
            .unwrap_or_default()
    }

    /// "Find a slot" para `slottable` (`open`: solo en shadow roots abiertos).
    pub fn find_a_slot(&self, slottable: NodeId, open: bool) -> Option<NodeId> {
        let parent = self.parent(slottable)?;
        let shadow = self.shadow_root(parent)?;
        let data = &self.shadow.roots[&shadow];
        if open && data.mode != ShadowRootMode::Open {
            return None;
        }
        if data.slot_assignment == SlotAssignmentMode::Manual {
            let slot = *self.shadow.manual_slot.get(&slottable)?;
            return (self.tree_root(slot) == shadow && slot != shadow).then_some(slot);
        }
        let name = self.slot_name_attr(slottable, "slot");
        self.descendants(shadow)
            .into_iter()
            .find(|&s| self.is_slot(s) && self.slot_name_attr(s, "name") == name)
    }

    /// "Find slottables" de `slot`.
    pub fn find_slottables(&self, slot: NodeId) -> Vec<NodeId> {
        let root = self.tree_root(slot);
        let Some(data) = self.shadow_root_data(root) else {
            return Vec::new();
        };
        let host = data.host;
        if data.slot_assignment == SlotAssignmentMode::Manual {
            return self
                .manually_assigned_nodes(slot)
                .into_iter()
                .filter(|&s| self.parent(s) == Some(host))
                .collect();
        }
        self.children(host)
            .filter(|&c| self.is_slottable(c) && self.find_a_slot(c, false) == Some(slot))
            .collect()
    }

    /// "Find flattened slottables" de `slot`.
    pub fn find_flattened_slottables(&self, slot: NodeId) -> Vec<NodeId> {
        let mut result = Vec::new();
        if !self.is_shadow_root(self.tree_root(slot)) {
            return result;
        }
        let mut slottables = self.find_slottables(slot);
        if slottables.is_empty() {
            slottables = self
                .children(slot)
                .filter(|&c| self.is_slottable(c))
                .collect();
        }
        for n in slottables {
            if self.is_slot(n) && self.is_shadow_root(self.tree_root(n)) {
                result.extend(self.find_flattened_slottables(n));
            } else {
                result.push(n);
            }
        }
        result
    }

    /// "Assign slottables" de `slot`.
    pub(crate) fn assign_slottables(&mut self, slot: NodeId) {
        let slottables = self.find_slottables(slot);
        let old = self.assigned_nodes(slot);
        if slottables != old {
            self.signal_a_slot_change(slot);
        }
        for o in old {
            if !slottables.contains(&o) && self.shadow.assigned_slot.get(&o) == Some(&slot) {
                self.shadow.assigned_slot.remove(&o);
            }
        }
        for &s in &slottables {
            self.shadow.assigned_slot.insert(s, slot);
        }
        if slottables.is_empty() {
            self.shadow.assigned_nodes.remove(&slot);
        } else {
            self.shadow.assigned_nodes.insert(slot, slottables);
        }
    }

    /// "Assign slottables for a tree" con raíz `root`.
    pub(crate) fn assign_slottables_for_tree(&mut self, root: NodeId) {
        if self.shadow.roots.is_empty() {
            return;
        }
        // Fuera de un shadow tree solo cambian los slots que tenían nodos asignados.
        if !self.is_shadow_root(self.tree_root(root)) {
            if self.shadow.assigned_nodes.is_empty() {
                return;
            }
            let slots: Vec<NodeId> = self
                .inclusive_descendants(root)
                .into_iter()
                .filter(|s| self.shadow.assigned_nodes.contains_key(s))
                .collect();
            for s in slots {
                self.assign_slottables(s);
            }
            return;
        }
        let slots: Vec<NodeId> = self
            .inclusive_descendants(root)
            .into_iter()
            .filter(|&s| self.is_slot(s))
            .collect();
        for s in slots {
            self.assign_slottables(s);
        }
    }

    /// Calcula las asignaciones de los slots de los shadow roots dentro de `node` (para los
    /// árboles armados sin el algoritmo *insert*, como las copias entre documentos).
    pub fn assign_slots_in(&mut self, node: NodeId) {
        let roots: Vec<NodeId> = self
            .shadow_including_inclusive_descendants(node)
            .into_iter()
            .filter(|&n| self.is_shadow_root(n))
            .collect();
        for r in roots {
            self.assign_slottables_for_tree(r);
        }
    }

    /// "Assign a slot" para `slottable`.
    pub(crate) fn assign_a_slot(&mut self, slottable: NodeId) {
        if let Some(slot) = self.find_a_slot(slottable, false) {
            self.assign_slottables(slot);
        }
    }

    /// "Signal a slot change".
    pub(crate) fn signal_a_slot_change(&mut self, slot: NodeId) {
        if !self.shadow.signal_slots.contains(&slot) {
            self.shadow.signal_slots.push(slot);
        }
        self.queue_mutation_observer_microtask();
    }

    /// Los slots con `slotchange` pendiente (el motor los dispara al notificar a los
    /// mutation observers, DOM §4.3.3).
    pub fn take_signal_slots(&mut self) -> Vec<NodeId> {
        std::mem::take(&mut self.shadow.signal_slots)
    }

    /// ¿Hay slots con `slotchange` pendiente?
    pub fn has_signal_slots(&self) -> bool {
        !self.shadow.signal_slots.is_empty()
    }

    /// `slot.assign(...nodes)` (HTML §4.12.4).
    pub fn slot_assign(&mut self, slot: NodeId, nodes: &[NodeId]) {
        for n in self.manually_assigned_nodes(slot) {
            self.shadow.manual_slot.remove(&n);
        }
        let mut set: Vec<NodeId> = Vec::new();
        for &n in nodes {
            if let Some(prev) = self.shadow.manual_slot.get(&n).copied()
                && let Some(list) = self.shadow.manual_nodes.get_mut(&prev)
            {
                list.retain(|x| *x != n);
            }
            self.shadow.manual_slot.insert(n, slot);
            if !set.contains(&n) {
                set.push(n);
            }
        }
        self.shadow.manual_nodes.insert(slot, set);
        let root = self.tree_root(slot);
        self.assign_slottables_for_tree(root);
    }

    // --- Los pasos de inserción y remoción (§4.2.3) ---

    /// Pasos 7.4–7.6 de "insert" para `node` insertado en `parent`.
    pub(crate) fn shadow_insert_steps(&mut self, node: NodeId, parent: NodeId) {
        if self.shadow.roots.is_empty() {
            return;
        }
        // 7.4.
        if let Some(sr) = self.shadow_root(parent)
            && self.shadow.roots[&sr].slot_assignment == SlotAssignmentMode::Named
            && self.is_slottable(node)
        {
            self.assign_a_slot(node);
        }
        // 7.5.
        if self.is_shadow_root(self.tree_root(parent))
            && self.is_slot(parent)
            && self.assigned_nodes(parent).is_empty()
        {
            self.signal_a_slot_change(parent);
        }
        // 7.6.
        let root = self.tree_root(node);
        self.assign_slottables_for_tree(root);
    }

    /// Pasos 12–14 de "remove" para `node` quitado de `parent` (ya desvinculado).
    pub(crate) fn shadow_remove_steps(&mut self, node: NodeId, parent: NodeId) {
        if self.shadow.roots.is_empty() {
            return;
        }
        // 12.
        if let Some(slot) = self.assigned_slot(node) {
            self.assign_slottables(slot);
        }
        // 13.
        if self.is_shadow_root(self.tree_root(parent))
            && self.is_slot(parent)
            && self.assigned_nodes(parent).is_empty()
        {
            self.signal_a_slot_change(parent);
        }
        // 14.
        if self
            .inclusive_descendants(node)
            .iter()
            .any(|&n| self.is_slot(n))
        {
            let root = self.tree_root(parent);
            self.assign_slottables_for_tree(root);
            self.assign_slottables_for_tree(node);
        }
    }

    /// Los pasos de cambio de atributo de los slots (`name`) y slottables (`slot`).
    pub(crate) fn shadow_attribute_changed(
        &mut self,
        el: NodeId,
        local: &DomString,
        old: Option<&DomString>,
    ) {
        if self.shadow.roots.is_empty() {
            return;
        }
        let is_name = *local == "name" && self.is_slot(el);
        let is_slot_attr = *local == "slot";
        if !is_name && !is_slot_attr {
            return;
        }
        let value = self.element(el).and_then(|e| {
            e.attrs
                .iter()
                .find(|a| a.ns.is_none() && a.name == *local)
                .map(|a| a.value.clone())
        });
        let old_s = old.cloned().unwrap_or_default();
        let new_s = value.clone().unwrap_or_default();
        if old_s == new_s {
            return;
        }
        if is_name {
            let root = self.tree_root(el);
            self.assign_slottables_for_tree(root);
        }
        if is_slot_attr {
            if let Some(slot) = self.assigned_slot(el) {
                self.assign_slottables(slot);
            }
            self.assign_a_slot(el);
        }
    }

    /// Clona el shadow root de `node` en `copy` si es *clonable* (paso 6 de "clone a node").
    pub(crate) fn clone_shadow_into(&mut self, node: NodeId, copy: NodeId, subtree: bool) {
        let Some(sr) = self.shadow_root(node) else {
            return;
        };
        let data = self.shadow.roots[&sr].clone();
        if !data.clonable {
            return;
        }
        let init = ShadowRootInit {
            mode: data.mode,
            delegates_focus: data.delegates_focus,
            slot_assignment: data.slot_assignment,
            clonable: true,
            serializable: data.serializable,
        };
        let Ok(copy_root) = self.attach_shadow(copy, init) else {
            return;
        };
        if let Some(d) = self.shadow.roots.get_mut(&copy_root) {
            d.declarative = data.declarative;
        }
        for c in self.child_nodes(sr) {
            self.clone_into(c, subtree, Some(copy_root));
        }
    }

    // --- Flat tree (CSS Scoping §2.2) ---

    /// Los hijos de `n` en el *flat tree*: los de su shadow root si es un host, los nodos
    /// asignados (aplanados) de un slot de un shadow tree o, si no tiene, sus hijos.
    pub fn flat_tree_children(&self, n: NodeId) -> Vec<NodeId> {
        if self.shadow.roots.is_empty() {
            return self.child_nodes(n);
        }
        if let Some(sr) = self.shadow_root(n) {
            return self.child_nodes(sr);
        }
        if self.is_slot(n) && self.is_shadow_root(self.tree_root(n)) {
            let assigned = self.assigned_nodes(n);
            if !assigned.is_empty() {
                return assigned;
            }
        }
        self.child_nodes(n)
    }

    /// El padre de `n` en el *flat tree*: su slot asignado, el host si el padre es un
    /// shadow root, o el padre.
    pub fn flat_tree_parent(&self, n: NodeId) -> Option<NodeId> {
        if self.shadow.roots.is_empty() {
            return self.parent(n);
        }
        if let Some(slot) = self.assigned_slot(n) {
            return Some(slot);
        }
        let p = self.parent(n)?;
        if let Some(h) = self.shadow_host(p) {
            return Some(h);
        }
        // Un hijo de un host sin slot no está en el flat tree; su padre sigue siendo el host
        // para quien pregunte (la herencia lo usa poco).
        Some(p)
    }

    /// ¿Está `n` en el *flat tree*? (Un hijo de un shadow host que no está asignado a
    /// ningún slot no se renderiza.)
    pub fn in_flat_tree(&self, n: NodeId) -> bool {
        if self.shadow.roots.is_empty() {
            return true;
        }
        let mut cur = n;
        loop {
            if let Some(slot) = self.assigned_slot(cur) {
                cur = slot;
                continue;
            }
            if let Some(h) = self.shadow_host(cur) {
                cur = h;
                continue;
            }
            let Some(p) = self.parent(cur) else {
                return true;
            };
            // Un hijo de un host sin slot asignado no se renderiza.
            if self.shadow_root(p).is_some() {
                return false;
            }
            // El contenido de respaldo de un slot con nodos asignados tampoco.
            if self.is_slot(p)
                && self.is_shadow_root(self.tree_root(p))
                && !self.assigned_nodes(p).is_empty()
            {
                return false;
            }
            cur = p;
        }
    }
}
