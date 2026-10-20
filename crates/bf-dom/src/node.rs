//! Interfaz `Node` (DOM §4.4).

use bf_infra::{DomString, Namespace};

use crate::mutation::is_text;
use crate::{AttrData, Attribute, Document, DomException, Element, NodeData, NodeId};

/// `Node.nodeType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum NodeType {
    Element = 1,
    Attribute = 2,
    Text = 3,
    CdataSection = 4,
    ProcessingInstruction = 7,
    Comment = 8,
    Document = 9,
    DocumentType = 10,
    DocumentFragment = 11,
}

/// Bits de `compareDocumentPosition`.
pub mod document_position {
    pub const DISCONNECTED: u16 = 0x01;
    pub const PRECEDING: u16 = 0x02;
    pub const FOLLOWING: u16 = 0x04;
    pub const CONTAINS: u16 = 0x08;
    pub const CONTAINED_BY: u16 = 0x10;
    pub const IMPLEMENTATION_SPECIFIC: u16 = 0x20;
}

impl Document {
    pub fn node_type(&self, n: NodeId) -> NodeType {
        match &self.get(n).data {
            NodeData::Element(_) => NodeType::Element,
            NodeData::Attr(_) => NodeType::Attribute,
            NodeData::Text(_) => NodeType::Text,
            NodeData::CdataSection(_) => NodeType::CdataSection,
            NodeData::ProcessingInstruction { .. } => NodeType::ProcessingInstruction,
            NodeData::Comment(_) => NodeType::Comment,
            NodeData::Document => NodeType::Document,
            NodeData::Doctype { .. } => NodeType::DocumentType,
            NodeData::DocumentFragment => NodeType::DocumentFragment,
        }
    }

    /// `Node.nodeName`.
    pub fn node_name(&self, n: NodeId) -> DomString {
        match &self.get(n).data {
            NodeData::Element(_) => self.tag_name(n),
            NodeData::Attr(a) => crate::qualified(a.prefix.as_ref(), &a.local_name),
            NodeData::Text(_) => "#text".into(),
            NodeData::CdataSection(_) => "#cdata-section".into(),
            NodeData::ProcessingInstruction { target, .. } => target.clone(),
            NodeData::Comment(_) => "#comment".into(),
            NodeData::Document => "#document".into(),
            NodeData::Doctype { name, .. } => name.clone(),
            NodeData::DocumentFragment => "#document-fragment".into(),
        }
    }

    /// "Document base URL" (HTML §2.4.1): el `href` del primer `base` con `href`, resuelto
    /// contra la URL del documento; si no hay, la URL del documento.
    pub fn base_url(&self) -> bf_infra::Url {
        let base = self
            .inclusive_descendants(self.root())
            .into_iter()
            .find(|&n| {
                self.element(n)
                    .is_some_and(|e| e.is_html("base") && e.attr("href").is_some())
            });
        if let Some(b) = base
            && let Some(href) = self.element(b).and_then(|e| e.attr("href"))
            && let Ok(url) = bf_infra::Url::parse(href, Some(&self.url))
        {
            return url;
        }
        self.url.clone()
    }

    /// `Node.baseURI`.
    pub fn base_uri(&self, _n: NodeId) -> DomString {
        self.base_url().href().into()
    }

    /// `Node.ownerDocument` (`None` para el documento).
    pub fn owner_document(&self, n: NodeId) -> Option<NodeId> {
        (!matches!(self.get(n).data, NodeData::Document)).then(|| self.root())
    }

    /// `Node.getRootNode()`.
    pub fn get_root_node(&self, n: NodeId) -> NodeId {
        self.tree_root(n)
    }

    /// `Node.parentElement`.
    pub fn parent_element(&self, n: NodeId) -> Option<NodeId> {
        self.parent(n).filter(|&p| self.element(p).is_some())
    }

    pub fn has_child_nodes(&self, n: NodeId) -> bool {
        self.get(n).first_child().is_some()
    }

    /// `Node.childNodes` (instantánea; la lista viva es releerla).
    pub fn child_nodes(&self, n: NodeId) -> Vec<NodeId> {
        self.children(n).collect()
    }

    pub fn first_child(&self, n: NodeId) -> Option<NodeId> {
        self.get(n).first_child()
    }

    pub fn last_child(&self, n: NodeId) -> Option<NodeId> {
        self.get(n).last_child()
    }

    pub fn previous_sibling(&self, n: NodeId) -> Option<NodeId> {
        self.get(n).prev_sibling()
    }

    pub fn next_sibling(&self, n: NodeId) -> Option<NodeId> {
        self.get(n).next_sibling()
    }

    /// Índice de `n` entre sus hermanos.
    pub fn index(&self, n: NodeId) -> usize {
        let mut i = 0;
        let mut cur = self.get(n).prev_sibling();
        while let Some(p) = cur {
            i += 1;
            cur = self.get(p).prev_sibling();
        }
        i
    }

    /// "Length" de un nodo (DOM §4.2): code units para `CharacterData`, hijos para el resto.
    pub fn node_length(&self, n: NodeId) -> usize {
        match &self.get(n).data {
            NodeData::Doctype { .. } | NodeData::Attr(_) => 0,
            NodeData::Text(d)
            | NodeData::CdataSection(d)
            | NodeData::Comment(d)
            | NodeData::ProcessingInstruction { data: d, .. } => d.len_utf16(),
            _ => self.children(n).count(),
        }
    }

    /// `Node.nodeValue`.
    pub fn node_value(&self, n: NodeId) -> Option<DomString> {
        match &self.get(n).data {
            NodeData::Attr(_) => Some(self.attr_value(n)),
            NodeData::Text(d)
            | NodeData::CdataSection(d)
            | NodeData::Comment(d)
            | NodeData::ProcessingInstruction { data: d, .. } => Some(d.clone()),
            _ => None,
        }
    }

    /// Setter de `Node.nodeValue` (`None` es el string vacío).
    pub fn set_node_value(&mut self, n: NodeId, value: Option<DomString>) {
        let value = value.unwrap_or_default();
        match &self.get(n).data {
            NodeData::Attr(_) => self.set_attr_value(n, value),
            d if crate::mutation::is_character_data(d) => {
                let len = self.node_length(n);
                let _ = self.replace_data(n, 0, len, &value);
            }
            _ => {}
        }
    }

    /// "Descendant text content": los datos de los `Text` descendientes en tree order.
    pub fn descendant_text_content(&self, n: NodeId) -> DomString {
        let mut out = DomString::new();
        for d in self.inclusive_descendants(n) {
            if let NodeData::Text(t) | NodeData::CdataSection(t) = &self.get(d).data {
                out.push_dom_str(t);
            }
        }
        out
    }

    /// "Child text content": los datos de los `Text` hijos.
    pub fn child_text_content(&self, n: NodeId) -> DomString {
        let mut out = DomString::new();
        for c in self.children(n) {
            if let NodeData::Text(t) | NodeData::CdataSection(t) = &self.get(c).data {
                out.push_dom_str(t);
            }
        }
        out
    }

    /// Getter de `Node.textContent`.
    pub fn get_text_content(&self, n: NodeId) -> Option<DomString> {
        match &self.get(n).data {
            NodeData::DocumentFragment | NodeData::Element(_) => {
                Some(self.descendant_text_content(n))
            }
            NodeData::Attr(_) => Some(self.attr_value(n)),
            NodeData::Text(d)
            | NodeData::CdataSection(d)
            | NodeData::Comment(d)
            | NodeData::ProcessingInstruction { data: d, .. } => Some(d.clone()),
            NodeData::Document | NodeData::Doctype { .. } => None,
        }
    }

    /// "String replace all" con `value` dentro de `parent`.
    pub fn string_replace_all(&mut self, value: &DomString, parent: NodeId) {
        let node = (!value.is_empty()).then(|| self.create(NodeData::Text(value.clone())));
        self.replace_all(node, parent);
    }

    /// Setter de `Node.textContent` (`None` es el string vacío).
    pub fn set_text_content(&mut self, n: NodeId, value: Option<DomString>) {
        let value = value.unwrap_or_default();
        match &self.get(n).data {
            NodeData::DocumentFragment | NodeData::Element(_) => self.string_replace_all(&value, n),
            NodeData::Attr(_) => self.set_attr_value(n, value),
            d if crate::mutation::is_character_data(d) => {
                let len = self.node_length(n);
                let _ = self.replace_data(n, 0, len, &value);
            }
            _ => {}
        }
    }

    /// `Node.normalize()`.
    pub fn normalize(&mut self, n: NodeId) {
        let texts: Vec<NodeId> = self
            .descendants(n)
            .into_iter()
            .filter(|&d| matches!(self.get(d).data, NodeData::Text(_)))
            .collect();
        for node in texts {
            // Puede que ya se haya quitado al fusionarse con un hermano anterior.
            if self.parent(node).is_none() || !self.is_inclusive_ancestor(n, node) {
                continue;
            }
            let length = self.node_length(node);
            if length == 0 {
                self.remove_node(node, false);
                continue;
            }
            let mut data = DomString::new();
            let mut cur = self.get(node).next_sibling();
            while let Some(s) = cur {
                match &self.get(s).data {
                    NodeData::Text(t) => data.push_dom_str(t),
                    _ => break,
                }
                cur = self.get(s).next_sibling();
            }
            if !data.is_empty() {
                let _ = self.replace_data(node, length, 0, &data);
            }
            let mut cur = self.get(node).next_sibling();
            while let Some(s) = cur {
                if !matches!(self.get(s).data, NodeData::Text(_)) {
                    break;
                }
                let next = self.get(s).next_sibling();
                self.remove_node(s, false);
                cur = next;
            }
        }
    }

    /// "Clone a node" (§4.4) dentro de este documento; con `deep` clona el subárbol.
    pub fn clone_node(&mut self, id: NodeId, deep: bool) -> NodeId {
        self.clone_into(id, deep, None)
    }

    fn clone_into(&mut self, node: NodeId, subtree: bool, parent: Option<NodeId>) -> NodeId {
        // 2. Clone a single node.
        let mut data = self.get(node).data.clone();
        let template_source = match &mut data {
            NodeData::Element(e) => e.template_contents.take(),
            NodeData::Attr(a) => {
                a.value = self.attr_value(node);
                a.owner = None;
                None
            }
            _ => None,
        };
        let copy = self.create(data);
        // 3. Cloning steps: `option` (selectedness) y `template` (template contents).
        self.clone_element_state(node, copy);
        if subtree
            && let (Some(source), Some(target)) = (
                template_source,
                self.element(copy).and_then(|e| e.template_contents),
            )
        {
            let children: Vec<_> = self.children(source).collect();
            for child in children {
                self.clone_into(child, true, Some(target));
            }
        }
        // 4.
        if let Some(p) = parent {
            self.insert(copy, p, None, false);
        }
        // 5.
        if subtree {
            let children: Vec<_> = self.children(node).collect();
            for child in children {
                self.clone_into(child, true, Some(copy));
            }
        }
        copy
    }

    /// Copia (profunda) en este documento el nodo `node` de otro documento `other`, con su
    /// template contents: es el "clone" con otro node document que usan los algoritmos de
    /// parseo de fragmentos (que parsean en un documento aparte).
    pub fn import_from(&mut self, other: &Document, node: NodeId) -> NodeId {
        let mut data = other.get(node).data.clone();
        let template_source = match &mut data {
            NodeData::Element(e) => e.template_contents.take(),
            NodeData::Attr(a) => {
                a.owner = None;
                None
            }
            _ => None,
        };
        let copy = self.create(data);
        if let (Some(source), Some(target)) = (
            template_source,
            self.element(copy).and_then(|e| e.template_contents),
        ) {
            for child in other.children(source) {
                let c = self.import_from(other, child);
                self.link(target, c, None);
            }
        }
        for child in other.children(node) {
            let c = self.import_from(other, child);
            self.link(copy, c, None);
        }
        copy
    }

    /// `Node.cloneNode(subtree)`.
    pub fn clone_node_checked(
        &mut self,
        id: NodeId,
        subtree: bool,
    ) -> Result<NodeId, DomException> {
        Ok(self.clone_node(id, subtree))
    }

    /// "Equals" (§4.4, `isEqualNode`).
    pub fn is_equal_node(&self, a: NodeId, b: NodeId) -> bool {
        let attr_eq =
            |x: &Attribute, y: &Attribute| x.ns == y.ns && x.name == y.name && x.value == y.value;
        let same = match (&self.get(a).data, &self.get(b).data) {
            (
                NodeData::Doctype {
                    name: n1,
                    public_id: p1,
                    system_id: s1,
                },
                NodeData::Doctype {
                    name: n2,
                    public_id: p2,
                    system_id: s2,
                },
            ) => n1 == n2 && p1 == p2 && s1 == s2,
            (NodeData::Element(e1), NodeData::Element(e2)) => {
                e1.ns == e2.ns
                    && e1.prefix == e2.prefix
                    && e1.name == e2.name
                    && e1.attrs.len() == e2.attrs.len()
                    && e1
                        .attrs
                        .iter()
                        .all(|x| e2.attrs.iter().any(|y| attr_eq(x, y)))
            }
            (NodeData::Attr(_), NodeData::Attr(_)) => {
                let (x, y) = (self.attr_data(a), self.attr_data(b));
                x.ns == y.ns
                    && x.local_name == y.local_name
                    && self.attr_value(a) == self.attr_value(b)
            }
            (
                NodeData::ProcessingInstruction {
                    target: t1,
                    data: d1,
                },
                NodeData::ProcessingInstruction {
                    target: t2,
                    data: d2,
                },
            ) => t1 == t2 && d1 == d2,
            (NodeData::Text(d1), NodeData::Text(d2))
            | (NodeData::CdataSection(d1), NodeData::CdataSection(d2))
            | (NodeData::Comment(d1), NodeData::Comment(d2)) => d1 == d2,
            (NodeData::Document, NodeData::Document)
            | (NodeData::DocumentFragment, NodeData::DocumentFragment) => true,
            _ => false,
        };
        if !same {
            return false;
        }
        let ca: Vec<NodeId> = self.children(a).collect();
        let cb: Vec<NodeId> = self.children(b).collect();
        ca.len() == cb.len() && ca.iter().zip(&cb).all(|(&x, &y)| self.is_equal_node(x, y))
    }

    /// `Node.isSameNode`.
    pub fn is_same_node(&self, a: NodeId, b: Option<NodeId>) -> bool {
        b == Some(a)
    }

    /// ¿`a` precede a `b` en tree order (mismo árbol)?
    pub fn precedes(&self, a: NodeId, b: NodeId) -> bool {
        let pa = self.inclusive_ancestors(a);
        let pb = self.inclusive_ancestors(b);
        // Rutas desde la raíz.
        let (pa, pb): (Vec<_>, Vec<_>) = (
            pa.into_iter().rev().collect(),
            pb.into_iter().rev().collect(),
        );
        let common = pa.iter().zip(&pb).take_while(|(x, y)| x == y).count();
        if common == pa.len() {
            // `a` es ancestro de `b`.
            return common < pb.len();
        }
        if common == pb.len() {
            return false;
        }
        self.index(pa[common]) < self.index(pb[common])
    }

    /// `Node.compareDocumentPosition(other)` (§4.4).
    pub fn compare_document_position(&self, this: NodeId, other: NodeId) -> u16 {
        use document_position::*;
        if this == other {
            return 0;
        }
        let mut node1 = Some(other);
        let mut node2 = Some(this);
        let mut attr1 = None;
        let mut attr2 = None;
        if let NodeData::Attr(a) = &self.get(other).data {
            attr1 = Some(other);
            node1 = a.owner;
        }
        if let NodeData::Attr(a) = &self.get(this).data {
            attr2 = Some(this);
            node2 = a.owner;
            if let (Some(a1), Some(n1)) = (attr1, node1)
                && node2 == Some(n1)
                && let Some(e) = self.element(n1)
            {
                let key = |id: NodeId| {
                    let d = self.attr_data(id);
                    (d.ns, d.local_name.clone())
                };
                for attr in &e.attrs {
                    let k = (attr.ns, attr.name.clone());
                    if k == key(a1) {
                        return IMPLEMENTATION_SPECIFIC | PRECEDING;
                    }
                    if k == key(this) {
                        return IMPLEMENTATION_SPECIFIC | FOLLOWING;
                    }
                }
            }
        }
        let (Some(n1), Some(n2)) = (node1, node2) else {
            return DISCONNECTED
                | IMPLEMENTATION_SPECIFIC
                | if other < this { PRECEDING } else { FOLLOWING };
        };
        if self.tree_root(n1) != self.tree_root(n2) {
            // Orden consistente: por índice en la arena de las raíces.
            let r1 = self.tree_root(n1);
            let r2 = self.tree_root(n2);
            return DISCONNECTED
                | IMPLEMENTATION_SPECIFIC
                | if r1 < r2 { PRECEDING } else { FOLLOWING };
        }
        if (n1 != n2 && self.is_inclusive_ancestor(n1, n2) && attr1.is_none())
            || (n1 == n2 && attr2.is_some())
        {
            return CONTAINS | PRECEDING;
        }
        if (n1 != n2 && self.is_inclusive_ancestor(n2, n1) && attr2.is_none())
            || (n1 == n2 && attr1.is_some())
        {
            return CONTAINED_BY | FOLLOWING;
        }
        if self.precedes(n1, n2) {
            return PRECEDING;
        }
        FOLLOWING
    }

    /// `Node.contains(other)`.
    pub fn contains(&self, this: NodeId, other: Option<NodeId>) -> bool {
        other.is_some_and(|o| self.is_inclusive_ancestor(this, o))
    }

    /// El elemento desde el que se buscan namespaces para `n` (§4.4, "locate a namespace").
    fn namespace_lookup_element(&self, n: NodeId) -> Option<NodeId> {
        match &self.get(n).data {
            NodeData::Element(_) => Some(n),
            NodeData::Document => self.document_element(),
            NodeData::Doctype { .. } | NodeData::DocumentFragment => None,
            NodeData::Attr(a) => a.owner,
            _ => self.parent_element(n),
        }
    }

    /// "Locate a namespace prefix" para `element`.
    fn locate_prefix(&self, element: NodeId, namespace: &str) -> Option<DomString> {
        let e = self.element(element)?;
        if e.ns.uri() == namespace && e.prefix.is_some() {
            return e.prefix.clone();
        }
        if let Some(a) = e
            .attrs
            .iter()
            .find(|a| a.prefix.as_ref().is_some_and(|p| *p == "xmlns") && a.value == namespace)
        {
            return Some(a.name.clone());
        }
        self.parent_element(element)
            .and_then(|p| self.locate_prefix(p, namespace))
    }

    /// `Node.lookupPrefix(namespace)`.
    pub fn lookup_prefix(&self, n: NodeId, namespace: Option<&str>) -> Option<DomString> {
        let ns = namespace.filter(|s| !s.is_empty())?;
        self.locate_prefix(self.namespace_lookup_element(n)?, ns)
    }

    /// "Locate a namespace" para `element` y `prefix`.
    fn locate_namespace(&self, element: NodeId, prefix: Option<&str>) -> Option<DomString> {
        let e = self.element(element)?;
        match prefix {
            Some("xml") => return Some(Namespace::Xml.uri().into()),
            Some("xmlns") => return Some(Namespace::XmlNs.uri().into()),
            _ => {}
        }
        if e.ns != Namespace::Null && e.prefix.as_ref().map(|p| p.to_string()).as_deref() == prefix
        {
            return Some(e.ns.uri().into());
        }
        let found = e.attrs.iter().find(|a| {
            a.ns == Some(Namespace::XmlNs)
                && match prefix {
                    Some(p) => a.prefix.as_ref().is_some_and(|x| *x == "xmlns") && a.name == p,
                    None => a.prefix.is_none() && a.name == "xmlns",
                }
        });
        if let Some(a) = found {
            return (!a.value.is_empty()).then(|| a.value.clone());
        }
        self.parent_element(element)
            .and_then(|p| self.locate_namespace(p, prefix))
    }

    /// `Node.lookupNamespaceURI(prefix)`.
    pub fn lookup_namespace_uri(&self, n: NodeId, prefix: Option<&str>) -> Option<DomString> {
        let prefix = prefix.filter(|p| !p.is_empty());
        self.locate_namespace(self.namespace_lookup_element(n)?, prefix)
    }

    /// `Node.isDefaultNamespace(namespace)`.
    pub fn is_default_namespace(&self, n: NodeId, namespace: Option<&str>) -> bool {
        let namespace = namespace.filter(|s| !s.is_empty());
        let default = self
            .namespace_lookup_element(n)
            .and_then(|e| self.locate_namespace(e, None));
        default.as_ref().map(|d| d.to_string()).as_deref() == namespace
    }

    /// `Node.insertBefore(node, child)`.
    pub fn insert_before_checked(
        &mut self,
        parent: NodeId,
        node: NodeId,
        child: Option<NodeId>,
    ) -> Result<NodeId, DomException> {
        self.pre_insert(node, parent, child)
    }

    /// `Node.appendChild(node)`.
    pub fn append_child(&mut self, parent: NodeId, node: NodeId) -> Result<NodeId, DomException> {
        self.pre_insert(node, parent, None)
    }

    /// `Node.replaceChild(node, child)`.
    pub fn replace_child(
        &mut self,
        parent: NodeId,
        node: NodeId,
        child: NodeId,
    ) -> Result<NodeId, DomException> {
        self.replace(child, node, parent)
    }

    /// `Node.removeChild(child)`.
    pub fn remove_child(&mut self, parent: NodeId, child: NodeId) -> Result<NodeId, DomException> {
        self.pre_remove(child, parent)
    }

    /// ¿Es un nodo `Text` exclusivo (no `CDATASection`)?
    pub fn is_exclusive_text(&self, n: NodeId) -> bool {
        matches!(self.get(n).data, NodeData::Text(_))
    }

    /// ¿Es un nodo `Text` (incluido `CDATASection`)?
    pub fn is_text_node(&self, n: NodeId) -> bool {
        is_text(&self.get(n).data)
    }

    pub(crate) fn attr_data(&self, n: NodeId) -> &AttrData {
        match &self.get(n).data {
            NodeData::Attr(a) => a,
            _ => panic!("no es un Attr"),
        }
    }

    /// Valor vigente de un nodo `Attr`.
    pub fn attr_value(&self, n: NodeId) -> DomString {
        let a = self.attr_data(n);
        if let Some(owner) = a.owner
            && let Some(e) = self.element(owner)
            && let Some(attr) = e
                .attrs
                .iter()
                .find(|x| x.ns == a.ns && x.name == a.local_name)
        {
            return attr.value.clone();
        }
        a.value.clone()
    }

    /// "Set an existing attribute value".
    pub fn set_attr_value(&mut self, n: NodeId, value: DomString) {
        let (owner, ns, local) = {
            let a = self.attr_data(n);
            (a.owner, a.ns, a.local_name.clone())
        };
        match owner {
            Some(el) => self.change_attribute(el, ns, &local, value),
            None => {
                if let NodeData::Attr(a) = &mut self.get_mut(n).data {
                    a.value = value;
                }
            }
        }
    }

    /// Elemento del nodo (para los que lo necesitan como `&Element`).
    pub fn element_ref(&self, n: NodeId) -> Option<&Element> {
        self.element(n)
    }
}
