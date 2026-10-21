//! Árbol DOM conforme al DOM Living Standard (<https://dom.spec.whatwg.org>): nodos,
//! documentos, algoritmos de mutación (§4.2), la interfaz `Node` y sus subclases, eventos
//! (§2), `MutationObserver` (§4.3) y `AbortController` (§3).
//!
//! Los nodos viven en una arena (`Document::nodes`) y se referencian con índices [`NodeId`].
//! La arena nunca libera nodos: `remove` solo los desvincula del árbol. Los métodos siguen los
//! nombres de las interfaces del spec en `snake_case`; los que pueden lanzar devuelven
//! `Result<_, DomException>`.
//!
//! Organización:
//! - [`mutation`], §4.2.3 (pre-insert, insert, replace, remove, adopt) y los hooks.
//! - `node`, `element`, `document`, `character_data`: las interfaces.
//! - [`collections`], `HTMLCollection` viva y `NodeList`.
//! - [`events`], `Event`, `EventTarget` y el dispatch de §2.9.
//! - [`observer`], `MutationObserver`.
//! - [`abort`], `AbortController` y `AbortSignal`.
//!
//! `matches`/`closest`/`querySelector` están en `bf_css::query` y la serialización HTML
//! (`innerHTML`, `outerHTML`, `insertAdjacentHTML`) en `bf_html::dom`, porque esos crates
//! dependen de este.

#![forbid(unsafe_code)]

pub mod abort;
mod character_data;
pub mod collections;
mod document;
mod element;
pub mod events;
mod exception;
mod forms;
pub mod mutation;
mod names;
mod node;
pub mod observer;
pub mod range; // E3
pub mod script;
mod serialize;

use std::collections::HashMap;
use std::sync::Mutex;

pub use bf_infra::{Atom, DomString, Namespace, Url};
pub use document::{HtmlElementKind, html_element_kind};
pub use element::{DomTokenList, NodeOrString, TokenListError};
pub use exception::DomException;
pub use node::{NodeType, document_position};
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

/// Datos de un nodo `Attr` (§4.9.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttrData {
    pub ns: Option<Namespace>,
    pub prefix: Option<DomString>,
    pub local_name: DomString,
    /// Valor propio; mientras tiene elemento, el valor vigente es el del elemento.
    pub value: DomString,
    /// El elemento al que pertenece.
    pub owner: Option<NodeId>,
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
    Attr(AttrData),
    Text(DomString),
    CdataSection(DomString),
    Comment(DomString),
    ProcessingInstruction {
        target: DomString,
        data: DomString,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    /// Namespace (`Namespace::Null` para los elementos sin namespace).
    pub ns: Namespace,
    /// Namespace prefix.
    pub prefix: Option<DomString>,
    /// Local name.
    pub name: DomString,
    pub attrs: Vec<Attribute>,
    /// Para `template` del namespace HTML: el `DocumentFragment` de su template contents.
    pub template_contents: Option<NodeId>,
}

impl Element {
    /// Elemento sin atributos ni prefijo.
    pub fn new(ns: Namespace, name: impl Into<DomString>) -> Self {
        Element {
            ns,
            prefix: None,
            name: name.into(),
            attrs: Vec::new(),
            template_contents: None,
        }
    }

    /// ¿Es un elemento del namespace HTML con este local name?
    pub fn is_html(&self, name: &str) -> bool {
        self.ns == Namespace::Html && self.name == name
    }

    /// Valor del atributo sin namespace llamado `name`, como `&str`. Un valor con surrogates
    /// sueltos no se ve por aquí (ver [`Document::get_attribute`]).
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|a| a.ns.is_none() && a.name == name)
            .and_then(|a| a.value.as_str())
    }

    /// Qualified name (prefijo `:` local name).
    pub fn qualified_name(&self) -> DomString {
        qualified(self.prefix.as_ref(), &self.name)
    }
}

pub(crate) fn qualified(prefix: Option<&DomString>, local: &DomString) -> DomString {
    match prefix {
        Some(p) => {
            let mut q = p.clone();
            q.push(':');
            q.push_dom_str(local);
            q
        }
        None => local.clone(),
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

    /// Qualified name.
    pub fn qualified_name(&self) -> DomString {
        qualified(self.prefix.as_ref(), &self.name)
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

/// Un documento y la arena de todos sus nodos (también de los que creó y no insertó).
#[derive(Debug, Clone)]
pub struct Document {
    nodes: Vec<Node>,
    pub mode: QuirksMode,
    microtasks: Vec<forms::Microtask>,
    /// URL del documento (`about:blank` por defecto).
    pub url: Url,
    /// Content type (`text/html` para documentos HTML).
    pub content_type: DomString,
    /// Encoding del documento (nombre canónico).
    pub character_set: DomString,
    /// ¿Documento HTML (y no XML)?
    pub is_html: bool,
    /// Contador de mutaciones del árbol y de atributos: invalida las colecciones vivas.
    version: u64,
    /// Mapa id → primer elemento en tree order, reconstruido cuando cambia `version`.
    id_cache: IdCache,
    /// Nodos `Attr` ya creados por elemento y (namespace, local name).
    attr_nodes: HashMap<(NodeId, Option<Namespace>, DomString), NodeId>,
    pub(crate) observers: observer::Observers,
    pub(crate) listeners: events::Listeners,
    pub(crate) aborts: abort::AbortState,
    pub(crate) scripts: script::ScriptLists,
    pub(crate) live_ranges: range::LiveRanges, // E3
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    /// Documento HTML vacío: solo el nodo `Document` raíz.
    pub fn new() -> Self {
        let mut doc = Document {
            nodes: Vec::new(),
            mode: QuirksMode::NoQuirks,
            microtasks: Vec::new(),
            url: Url::parse("about:blank", None).expect("about:blank es una URL válida"),
            content_type: "text/html".into(),
            character_set: "UTF-8".into(),
            is_html: true,
            version: 0,
            id_cache: IdCache::default(),
            attr_nodes: HashMap::new(),
            observers: observer::Observers::default(),
            listeners: events::Listeners::default(),
            aborts: abort::AbortState::default(),
            scripts: script::ScriptLists::default(),
            live_ranges: range::LiveRanges::default(), // E3
        };
        doc.push(NodeData::Document);
        doc
    }

    /// Documento XML vacío (`application/xml`).
    pub fn new_xml() -> Self {
        let mut doc = Document::new();
        doc.is_html = false;
        doc.content_type = "application/xml".into();
        doc
    }

    pub fn root(&self) -> NodeId {
        NodeId(0)
    }

    /// Cantidad de nodos de la arena.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
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

    pub fn element_mut(&mut self, id: NodeId) -> Option<&mut Element> {
        self.get_mut(id).element_mut()
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

    /// Ancestros inclusivos de `id`, del propio nodo a la raíz.
    pub fn inclusive_ancestors(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = vec![id];
        let mut current = id;
        while let Some(p) = self.parent(current) {
            out.push(p);
            current = p;
        }
        out
    }

    /// La raíz del árbol de `id`.
    pub fn tree_root(&self, id: NodeId) -> NodeId {
        let mut current = id;
        while let Some(p) = self.parent(current) {
            current = p;
        }
        current
    }

    /// Agrega `child` como último hijo de `parent` (desvinculándolo antes si tenía padre).
    /// Es el *append* sin validación que usa el parser; corre el *insert* del spec.
    pub fn append(&mut self, parent: NodeId, child: NodeId) {
        self.insert_before(parent, child, None);
    }

    /// Inserta `child` en `parent` antes de `reference` (al final si es `None`) con el
    /// algoritmo *insert* (§4.2.3), sin la validación de *pre-insert* (la garantiza quien
    /// llama, como el parser).
    pub fn insert_before(&mut self, parent: NodeId, child: NodeId, reference: Option<NodeId>) {
        debug_assert!(reference.is_none_or(|r| self.parent(r) == Some(parent)));
        if self.parent(child).is_some() {
            self.remove(child);
        }
        self.insert(child, parent, reference, false);
    }

    pub(crate) fn link(&mut self, parent: NodeId, child: NodeId, reference: Option<NodeId>) {
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

    /// Desvincula `child`; devuelve si tenía padre.
    pub(crate) fn unlink(&mut self, child: NodeId) -> bool {
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
        self.tree_root(id) == self.root()
    }

    /// Descendientes inclusivos de `id` en tree order (sin entrar a template contents).
    pub fn inclusive_descendants(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut node = Some(id);
        while let Some(n) = node {
            out.push(n);
            node = self.following_within(n, id);
        }
        out
    }

    /// Descendientes de `id` en tree order.
    pub fn descendants(&self, id: NodeId) -> Vec<NodeId> {
        let mut v = self.inclusive_descendants(id);
        v.remove(0);
        v
    }

    /// El siguiente nodo en preorden dentro del subárbol de `root`.
    pub fn following_within(&self, n: NodeId, root: NodeId) -> Option<NodeId> {
        if let Some(c) = self.get(n).first_child {
            return Some(c);
        }
        let mut up = n;
        while up != root {
            if let Some(next) = self.get(up).next_sibling {
                return Some(next);
            }
            up = self.parent(up)?;
        }
        None
    }

    /// Contador de mutaciones (cambia con cada inserción, remoción o cambio de atributo o
    /// de datos).
    pub fn version(&self) -> u64 {
        self.version
    }

    pub(crate) fn bump_version(&mut self) {
        self.version += 1;
    }
}

/// Caché de `getElementById`: (versión, id → elemento). Clonar un documento la vacía.
#[derive(Debug, Default)]
pub(crate) struct IdCache(pub(crate) Mutex<Option<(u64, HashMap<DomString, NodeId>)>>);

impl Clone for IdCache {
    fn clone(&self) -> Self {
        IdCache::default()
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
                NodeData::Text(t) => t.to_string(),
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
        let t = doc.create(NodeData::Element(Element::new(Namespace::Html, "template")));
        let contents = doc.element(t).unwrap().template_contents.unwrap();
        assert_eq!(doc.get(contents).data, NodeData::DocumentFragment);
    }
}
