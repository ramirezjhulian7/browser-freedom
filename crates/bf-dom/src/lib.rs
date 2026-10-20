//! Árbol DOM conforme al DOM Living Standard: nodos, documentos, eventos y mutaciones.
//!
//! Los nodos viven en una arena (`Document::nodes`) y se referencian con índices [`NodeId`].
//! En esta etapa la arena nunca libera nodos: `remove` solo los desvincula del árbol.

#![forbid(unsafe_code)]

mod forms;
mod serialize;

pub use bf_infra::{DomString, Namespace};
pub use serialize::test_serialize;

/// Índice de un nodo en la arena de su [`Document`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(u32);

impl NodeId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// <https://dom.spec.whatwg.org/#concept-document-mode>
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QuirksMode {
    #[default]
    NoQuirks,
    Quirks,
    LimitedQuirks,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeData {
    Document,
    DocumentFragment,
    Doctype {
        name: DomString,
        public_id: DomString,
        system_id: DomString,
    },
    Element(Element),
    Text(DomString),
    Comment(DomString),
    ProcessingInstruction {
        target: DomString,
        data: DomString,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub ns: Namespace,
    /// Local name.
    pub name: DomString,
    pub attrs: Vec<Attribute>,
    /// Para `template` del namespace HTML: el `DocumentFragment` de su template contents.
    pub template_contents: Option<NodeId>,
}

impl Element {
    /// ¿Es un elemento del namespace HTML con este local name?
    pub fn is_html(&self, name: &str) -> bool {
        self.ns == Namespace::Html && self.name == name
    }

    /// Valor del atributo sin namespace llamado `name`.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|a| a.ns.is_none() && a.name == name)
            .map(|a| a.value.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    pub ns: Option<Namespace>,
    pub prefix: Option<DomString>,
    /// Local name.
    pub name: DomString,
    pub value: DomString,
}

impl Attribute {
    /// Atributo sin namespace.
    pub fn new(name: impl Into<DomString>, value: impl Into<DomString>) -> Self {
        Attribute {
            ns: None,
            prefix: None,
            name: name.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Node {
    parent: Option<NodeId>,
    first_child: Option<NodeId>,
    last_child: Option<NodeId>,
    prev_sibling: Option<NodeId>,
    next_sibling: Option<NodeId>,
    /// Estado interno de elementos con comportamiento propio (p. ej. `option`).
    state: forms::ElementState,
    pub data: NodeData,
}

impl Node {
    pub fn parent(&self) -> Option<NodeId> {
        self.parent
    }
    pub fn first_child(&self) -> Option<NodeId> {
        self.first_child
    }
    pub fn last_child(&self) -> Option<NodeId> {
        self.last_child
    }
    pub fn prev_sibling(&self) -> Option<NodeId> {
        self.prev_sibling
    }
    pub fn next_sibling(&self) -> Option<NodeId> {
        self.next_sibling
    }

    pub fn element(&self) -> Option<&Element> {
        match &self.data {
            NodeData::Element(e) => Some(e),
            _ => None,
        }
    }

    pub fn element_mut(&mut self) -> Option<&mut Element> {
        match &mut self.data {
            NodeData::Element(e) => Some(e),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Document {
    nodes: Vec<Node>,
    pub mode: QuirksMode,
    microtasks: Vec<forms::Microtask>,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    /// Documento vacío: solo el nodo `Document` raíz.
    pub fn new() -> Self {
        let mut doc = Document {
            nodes: Vec::new(),
            mode: QuirksMode::NoQuirks,
            microtasks: Vec::new(),
        };
        doc.create(NodeData::Document);
        doc
    }

    pub fn root(&self) -> NodeId {
        NodeId(0)
    }

    /// Crea un nodo desvinculado. Un `template` HTML recibe su template contents.
    pub fn create(&mut self, mut data: NodeData) -> NodeId {
        if let NodeData::Element(e) = &mut data
            && e.is_html("template")
            && e.template_contents.is_none()
        {
            e.template_contents = Some(self.push(NodeData::DocumentFragment));
        }
        let id = self.push(data);
        self.init_element_state(id);
        id
    }

    /// Clona `id` (y su subárbol si `deep`), como el algoritmo *clone a node* del DOM.
    pub fn clone_node(&mut self, id: NodeId, deep: bool) -> NodeId {
        let mut data = self.get(id).data.clone();
        let template_source = match &mut data {
            NodeData::Element(e) => e.template_contents.take(),
            _ => None,
        };
        let copy = self.create(data);
        self.clone_element_state(id, copy);
        if deep {
            // Cloning steps de template: copiar también el template contents.
            if let (Some(source), Some(target)) = (
                template_source,
                self.element(copy).and_then(|e| e.template_contents),
            ) {
                let children: Vec<_> = self.children(source).collect();
                for child in children {
                    let child_copy = self.clone_node(child, true);
                    self.append(target, child_copy);
                }
            }
            let children: Vec<_> = self.children(id).collect();
            for child in children {
                let child_copy = self.clone_node(child, true);
                self.append(copy, child_copy);
            }
        }
        copy
    }

    fn push(&mut self, data: NodeData) -> NodeId {
        let id = NodeId(u32::try_from(self.nodes.len()).expect("arena de nodos llena"));
        self.nodes.push(Node {
            parent: None,
            first_child: None,
            last_child: None,
            prev_sibling: None,
            next_sibling: None,
            state: forms::ElementState::default(),
            data,
        });
        id
    }

    pub fn get(&self, id: NodeId) -> &Node {
        &self.nodes[id.index()]
    }

    pub fn get_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id.index()]
    }

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.get(id).parent
    }

    pub fn element(&self, id: NodeId) -> Option<&Element> {
        self.get(id).element()
    }

    pub fn children(&self, id: NodeId) -> Children<'_> {
        Children {
            doc: self,
            next: self.get(id).first_child,
        }
    }

    /// ¿`ancestor` es `node` o uno de sus ancestros?
    pub fn is_inclusive_ancestor(&self, ancestor: NodeId, node: NodeId) -> bool {
        let mut current = Some(node);
        while let Some(n) = current {
            if n == ancestor {
                return true;
            }
            current = self.parent(n);
        }
        false
    }

    /// Agrega `child` como último hijo de `parent` (desvinculándolo antes si tenía padre).
    pub fn append(&mut self, parent: NodeId, child: NodeId) {
        self.insert_before(parent, child, None);
    }

    /// Inserta `child` en `parent` antes de `reference` (al final si es `None`), corriendo los
    /// insertion steps y post-connection steps de los elementos insertados.
    pub fn insert_before(&mut self, parent: NodeId, child: NodeId, reference: Option<NodeId>) {
        debug_assert!(reference.is_none_or(|r| self.parent(r) == Some(parent)));
        self.remove(child);
        self.link(parent, child, reference);
        self.run_insertion_steps(child);
    }

    fn link(&mut self, parent: NodeId, child: NodeId, reference: Option<NodeId>) {
        let prev = match reference {
            Some(r) => self.get(r).prev_sibling,
            None => self.get(parent).last_child,
        };
        {
            let node = self.get_mut(child);
            node.parent = Some(parent);
            node.prev_sibling = prev;
            node.next_sibling = reference;
        }
        match prev {
            Some(p) => self.get_mut(p).next_sibling = Some(child),
            None => self.get_mut(parent).first_child = Some(child),
        }
        match reference {
            Some(r) => self.get_mut(r).prev_sibling = Some(child),
            None => self.get_mut(parent).last_child = Some(child),
        }
    }

    /// Desvincula `child` de su padre (corriendo los removing steps). No libera el nodo.
    pub fn remove(&mut self, child: NodeId) {
        if self.unlink(child) {
            self.run_removing_steps(child);
        }
    }

    /// Desvincula `child`; devuelve si tenía padre.
    fn unlink(&mut self, child: NodeId) -> bool {
        let Some(parent) = self.get(child).parent else {
            return false;
        };
        let (prev, next) = {
            let node = self.get(child);
            (node.prev_sibling, node.next_sibling)
        };
        match prev {
            Some(p) => self.get_mut(p).next_sibling = next,
            None => self.get_mut(parent).first_child = next,
        }
        match next {
            Some(n) => self.get_mut(n).prev_sibling = prev,
            None => self.get_mut(parent).last_child = prev,
        }
        let node = self.get_mut(child);
        node.parent = None;
        node.prev_sibling = None;
        node.next_sibling = None;
        true
    }

    /// ¿El nodo está conectado (su raíz es el nodo `Document`)?
    pub fn is_connected(&self, id: NodeId) -> bool {
        let mut current = id;
        while let Some(parent) = self.parent(current) {
            current = parent;
        }
        current == self.root()
    }

    /// Descendientes inclusivos de `id` en tree order (sin entrar a template contents).
    pub fn inclusive_descendants(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut node = Some(id);
        while let Some(n) = node {
            out.push(n);
            node = self.get(n).first_child;
            if node.is_none() {
                // Siguiente nodo en preorden sin salir del subárbol de `id`.
                let mut up = n;
                while up != id {
                    if let Some(next) = self.get(up).next_sibling {
                        node = Some(next);
                        break;
                    }
                    up = self.parent(up).expect("dentro del subárbol");
                }
            }
        }
        out
    }
}

/// Iterador sobre los hijos de un nodo.
pub struct Children<'a> {
    doc: &'a Document,
    next: Option<NodeId>,
}

impl Iterator for Children<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        let id = self.next?;
        self.next = self.doc.get(id).next_sibling;
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(doc: &mut Document, s: &str) -> NodeId {
        doc.create(NodeData::Text(s.into()))
    }

    fn texts(doc: &Document, parent: NodeId) -> Vec<String> {
        doc.children(parent)
            .map(|c| match &doc.get(c).data {
                NodeData::Text(t) => t.clone(),
                _ => unreachable!(),
            })
            .collect()
    }

    #[test]
    fn append_insert_remove() {
        let mut doc = Document::new();
        let root = doc.root();
        let (a, b, c) = (
            text(&mut doc, "a"),
            text(&mut doc, "b"),
            text(&mut doc, "c"),
        );
        doc.append(root, a);
        doc.append(root, c);
        doc.insert_before(root, b, Some(c));
        assert_eq!(texts(&doc, root), ["a", "b", "c"]);
        doc.remove(b);
        assert_eq!(texts(&doc, root), ["a", "c"]);
        assert_eq!(doc.parent(b), None);
        // Mover un nodo lo desvincula de su posición anterior.
        doc.insert_before(root, c, Some(a));
        assert_eq!(texts(&doc, root), ["c", "a"]);
        assert_eq!(doc.get(root).last_child(), Some(a));
    }

    #[test]
    fn template_gets_contents() {
        let mut doc = Document::new();
        let t = doc.create(NodeData::Element(Element {
            ns: Namespace::Html,
            name: "template".into(),
            attrs: vec![],
            template_contents: None,
        }));
        let contents = doc.element(t).unwrap().template_contents.unwrap();
        assert_eq!(doc.get(contents).data, NodeData::DocumentFragment);
    }
}
