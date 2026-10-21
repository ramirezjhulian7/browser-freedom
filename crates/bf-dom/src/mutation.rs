//! Algoritmos de mutación (DOM §4.2.3): *ensure pre-insertion validity*, *pre-insert*,
//! *insert*, *append*, *replace*, *replace all*, *pre-remove*, *remove* y *adopt*, con sus
//! records de `MutationObserver` y los hooks de especificaciones dependientes (insertion,
//! removing, children changed, adopting y post-connection steps; ver `forms.rs`).
//!
//! No hay live ranges, shadow trees, slots ni custom elements: los pasos que los tocan se
//! omiten. Un `Document` es una sola arena con un único nodo documento, así que *adopt* solo
//! desvincula (nunca cambia el node document).

use crate::{Document, DomException, NodeData, NodeId};

/// ¿Es un nodo `CharacterData` (Text, CDATASection, Comment o ProcessingInstruction)?
pub(crate) fn is_character_data(data: &NodeData) -> bool {
    matches!(
        data,
        NodeData::Text(_)
            | NodeData::CdataSection(_)
            | NodeData::Comment(_)
            | NodeData::ProcessingInstruction { .. }
    )
}

/// ¿Es un nodo `Text` (incluye `CDATASection`, que hereda de `Text`)?
pub(crate) fn is_text(data: &NodeData) -> bool {
    matches!(data, NodeData::Text(_) | NodeData::CdataSection(_))
}

impl Document {
    fn is_document(&self, n: NodeId) -> bool {
        matches!(self.get(n).data, NodeData::Document)
    }

    fn is_doctype(&self, n: NodeId) -> bool {
        matches!(self.get(n).data, NodeData::Doctype { .. })
    }

    fn is_element(&self, n: NodeId) -> bool {
        matches!(self.get(n).data, NodeData::Element(_))
    }

    /// El template cuyo template contents es `fragment` (su *host*).
    pub fn template_host(&self, fragment: NodeId) -> Option<NodeId> {
        if !matches!(self.get(fragment).data, NodeData::DocumentFragment) {
            return None;
        }
        (0..self.node_count() as u32).map(NodeId).find(|&n| {
            self.element(n)
                .is_some_and(|e| e.template_contents == Some(fragment))
        })
    }

    /// ¿`a` es un *host-including inclusive ancestor* de `b`?
    pub fn is_host_including_inclusive_ancestor(&self, a: NodeId, b: NodeId) -> bool {
        let mut current = b;
        loop {
            if self.is_inclusive_ancestor(a, current) {
                return true;
            }
            let root = self.tree_root(current);
            match self.template_host(root) {
                Some(host) => current = host,
                None => return false,
            }
        }
    }

    /// ¿`parent` tiene un hijo elemento (sin contar los excluidos)?
    fn has_element_child(&self, parent: NodeId, excluded: &[NodeId]) -> bool {
        self.children(parent)
            .any(|c| !excluded.contains(&c) && self.is_element(c))
    }

    fn has_doctype_child(&self, parent: NodeId, excluded: &[NodeId]) -> bool {
        self.children(parent)
            .any(|c| !excluded.contains(&c) && self.is_doctype(c))
    }

    /// ¿Hay un doctype (no excluido) después de `child` entre sus hermanos?
    fn doctype_following(&self, child: NodeId, excluded: &[NodeId]) -> bool {
        let mut n = self.get(child).next_sibling();
        while let Some(s) = n {
            if !excluded.contains(&s) && self.is_doctype(s) {
                return true;
            }
            n = self.get(s).next_sibling();
        }
        false
    }

    /// ¿Hay un elemento (no excluido) antes de `child` entre sus hermanos?
    fn element_preceding(&self, child: NodeId, excluded: &[NodeId]) -> bool {
        let mut n = self.get(child).prev_sibling();
        while let Some(s) = n {
            if !excluded.contains(&s) && self.is_element(s) {
                return true;
            }
            n = self.get(s).prev_sibling();
        }
        false
    }

    /// "Ensure pre-insert validity" de `node` en `parent` antes de `child`, con los hijos
    /// `excluded` como si no estuvieran (`replace` excluye el hijo reemplazado y
    /// `replaceChildren` todos).
    pub fn ensure_pre_insert_validity_excluding(
        &self,
        node: NodeId,
        parent: NodeId,
        child: Option<NodeId>,
        excluded: &[NodeId],
    ) -> Result<(), DomException> {
        // 1.
        if !matches!(
            self.get(parent).data,
            NodeData::Document | NodeData::DocumentFragment | NodeData::Element(_)
        ) {
            return Err(DomException::HierarchyRequestError);
        }
        // 2.
        if self.is_host_including_inclusive_ancestor(node, parent) {
            return Err(DomException::HierarchyRequestError);
        }
        // 3.
        if let Some(c) = child
            && self.parent(c) != Some(parent)
        {
            return Err(DomException::NotFoundError);
        }
        let data = &self.get(node).data;
        // 4.
        if !(matches!(
            data,
            NodeData::DocumentFragment | NodeData::Doctype { .. } | NodeData::Element(_)
        ) || is_character_data(data))
        {
            return Err(DomException::HierarchyRequestError);
        }
        // 5.
        let parent_is_doc = self.is_document(parent);
        if (is_text(data) && parent_is_doc) || (self.is_doctype(node) && !parent_is_doc) {
            return Err(DomException::HierarchyRequestError);
        }
        // 6.
        if parent_is_doc {
            let child_is_doctype =
                child.is_some_and(|c| !excluded.contains(&c) && self.is_doctype(c));
            let doctype_after = child.is_some_and(|c| self.doctype_following(c, excluded));
            let bad = match data {
                NodeData::DocumentFragment => {
                    let elements = self.children(node).filter(|&c| self.is_element(c)).count();
                    let has_text = self.children(node).any(|c| is_text(&self.get(c).data));
                    elements > 1
                        || has_text
                        || (elements == 1
                            && (self.has_element_child(parent, excluded)
                                || child_is_doctype
                                || doctype_after))
                }
                NodeData::Element(_) => {
                    self.has_element_child(parent, excluded) || child_is_doctype || doctype_after
                }
                NodeData::Doctype { .. } => {
                    self.has_doctype_child(parent, excluded)
                        || child.is_some_and(|c| self.element_preceding(c, excluded))
                        || (child.is_none() && self.has_element_child(parent, excluded))
                }
                _ => false,
            };
            if bad {
                return Err(DomException::HierarchyRequestError);
            }
        }
        Ok(())
    }

    /// "Ensure pre-insert validity" de `node` en `parent` antes de `child`.
    pub fn ensure_pre_insertion_validity(
        &self,
        node: NodeId,
        parent: NodeId,
        child: Option<NodeId>,
    ) -> Result<(), DomException> {
        self.ensure_pre_insert_validity_excluding(node, parent, child, &[])
    }

    /// "Pre-insert" `node` en `parent` antes de `child`; devuelve `node`.
    pub fn pre_insert(
        &mut self,
        node: NodeId,
        parent: NodeId,
        child: Option<NodeId>,
    ) -> Result<NodeId, DomException> {
        self.ensure_pre_insertion_validity(node, parent, child)?;
        let mut reference = child;
        if reference == Some(node) {
            reference = self.get(node).next_sibling();
        }
        self.insert(node, parent, reference, false);
        Ok(node)
    }

    /// "Insert" `node` en `parent` antes de `child` (sin validación).
    pub(crate) fn insert(
        &mut self,
        node: NodeId,
        parent: NodeId,
        child: Option<NodeId>,
        suppress_observers: bool,
    ) {
        // 1-3.
        let is_fragment = matches!(self.get(node).data, NodeData::DocumentFragment);
        let nodes: Vec<NodeId> = if is_fragment {
            self.children(node).collect()
        } else {
            vec![node]
        };
        if nodes.is_empty() {
            return;
        }
        // 4. Un fragmento se vacía primero.
        if is_fragment {
            for &n in &nodes {
                self.remove_node(n, true);
            }
            self.queue_tree_mutation_record(node, &[], &nodes, None, None);
        }
        // E3: paso 5, los rangos vivos.
        if let Some(c) = child
            && self.has_live_ranges()
        {
            let index = self.index(c) as u32;
            self.ranges_insert(parent, index, nodes.len() as u32);
        }
        // 6.
        let previous = match child {
            Some(c) => self.get(c).prev_sibling(),
            None => self.get(parent).last_child(),
        };
        // 7.
        for &n in &nodes {
            self.adopt_node_into(n);
            self.link(parent, n, child);
            self.bump_version();
        }
        // 8.
        if !suppress_observers {
            self.queue_tree_mutation_record(parent, &nodes, &[], previous, child);
        }
        // 9.
        self.children_changed(parent);
        // 7.7 y 10-11: insertion steps y post-connection steps.
        for &n in &nodes {
            self.run_insertion_steps(n);
        }
    }

    /// "Append" `node` a `parent` (pre-insert antes de null).
    pub fn append_checked(&mut self, node: NodeId, parent: NodeId) -> Result<NodeId, DomException> {
        self.pre_insert(node, parent, None)
    }

    /// "Replace" `child` con `node` en `parent`; devuelve `child`.
    pub fn replace(
        &mut self,
        child: NodeId,
        node: NodeId,
        parent: NodeId,
    ) -> Result<NodeId, DomException> {
        // 1. Validez con `child` excluido.
        self.ensure_pre_insert_validity_excluding(node, parent, Some(child), &[child])?;
        // 2.
        let mut reference = self.get(child).next_sibling();
        if reference == Some(node) {
            reference = self.get(node).next_sibling();
        }
        // 3.
        let previous = self.get(child).prev_sibling();
        // 4. Adopt `node` (lo quita de su padre, con su record) antes de quitar `child`.
        self.adopt(node);
        // 5.
        let mut removed = Vec::new();
        if self.parent(child).is_some() {
            removed.push(child);
            self.remove_node(child, true);
        }
        // 6-7.
        let nodes: Vec<NodeId> = if matches!(self.get(node).data, NodeData::DocumentFragment) {
            self.children(node).collect()
        } else {
            vec![node]
        };
        self.insert(node, parent, reference, true);
        // 8.
        if !nodes.is_empty() || !removed.is_empty() {
            self.queue_tree_mutation_record(parent, &nodes, &removed, previous, reference);
        }
        Ok(child)
    }

    /// "Replace all" con `node` dentro de `parent`.
    pub fn replace_all(&mut self, node: Option<NodeId>, parent: NodeId) {
        let removed: Vec<NodeId> = self.children(parent).collect();
        let added: Vec<NodeId> = match node {
            Some(n) if matches!(self.get(n).data, NodeData::DocumentFragment) => {
                self.children(n).collect()
            }
            Some(n) => vec![n],
            None => Vec::new(),
        };
        for &c in &removed {
            self.remove_node(c, true);
        }
        if let Some(n) = node {
            self.insert(n, parent, None, true);
        }
        if !added.is_empty() || !removed.is_empty() {
            self.queue_tree_mutation_record(parent, &added, &removed, None, None);
        }
    }

    /// "Pre-remove" `child` de `parent`; devuelve `child`.
    pub fn pre_remove(&mut self, child: NodeId, parent: NodeId) -> Result<NodeId, DomException> {
        if self.parent(child) != Some(parent) {
            return Err(DomException::NotFoundError);
        }
        self.remove_node(child, false);
        Ok(child)
    }

    /// "Remove" `child` de su padre (sin efecto si no tiene). Es el que usa el parser.
    pub fn remove(&mut self, child: NodeId) {
        self.remove_node(child, false);
    }

    /// "Remove" con el *suppress observers flag*.
    pub(crate) fn remove_node(&mut self, node: NodeId, suppress_observers: bool) {
        let Some(parent) = self.parent(node) else {
            return;
        };
        // E3: pasos 4-7, los rangos vivos.
        if self.has_live_ranges() {
            let index = self.index(node) as u32;
            self.ranges_remove(node, parent, index);
        }
        // 9-11.
        let old_previous = self.get(node).prev_sibling();
        let old_next = self.get(node).next_sibling();
        self.unlink(node);
        self.bump_version();
        // 15. Removing steps.
        self.run_removing_steps(node);
        // 17. Registros transitorios.
        self.add_transient_observers(parent, node);
        // 18.
        if !suppress_observers {
            self.queue_tree_mutation_record(parent, &[], &[node], old_previous, old_next);
        }
        // 19.
        self.children_changed(parent);
    }

    /// "Adopt" `node` en este documento: lo desvincula de su padre (un `Document` es una sola
    /// arena, así que el node document no cambia y no hay adopting steps que correr).
    pub fn adopt(&mut self, node: NodeId) {
        if self.parent(node).is_some() {
            self.remove_node(node, false);
        }
    }

    /// Paso 7.1 de *insert*: adopt sin record (el nodo ya se quitó de su padre o es de un
    /// fragmento vaciado).
    fn adopt_node_into(&mut self, node: NodeId) {
        if self.parent(node).is_some() {
            self.remove_node(node, false);
        }
    }

    /// "Children changed steps".
    fn children_changed(&mut self, _parent: NodeId) {
        // Ningún elemento implementado define children changed steps todavía (`title` y
        // `option` leen su texto bajo demanda).
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Element, Namespace};

    fn el(doc: &mut Document, name: &str) -> NodeId {
        doc.create(NodeData::Element(Element::new(Namespace::Html, name)))
    }

    #[test]
    fn hierarchy_errors() {
        let mut doc = Document::new();
        let root = doc.root();
        let html = el(&mut doc, "html");
        doc.pre_insert(html, root, None).unwrap();
        let other = el(&mut doc, "body");
        assert_eq!(
            doc.pre_insert(other, root, None),
            Err(DomException::HierarchyRequestError)
        );
        let text = doc.create(NodeData::Text("x".into()));
        assert_eq!(
            doc.pre_insert(text, root, None),
            Err(DomException::HierarchyRequestError)
        );
        assert_eq!(
            doc.pre_insert(html, other, None),
            Ok(html),
            "mover el elemento raíz a otro padre vale"
        );
        assert_eq!(
            doc.pre_insert(other, html, None),
            Err(DomException::HierarchyRequestError),
            "un ancestro no puede ir dentro de su descendiente"
        );
    }
}
