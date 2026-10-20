//! Interfaz `Element` (DOM §4.9), sus atributos (§4.9.1, con los nodos `Attr` de §4.9.2 y
//! `NamedNodeMap`), `DOMTokenList` (§7.1) y los mixins `ParentNode`, `NonDocumentTypeChildNode`
//! y `ChildNode` (§4.2.6–§4.2.8).

use bf_infra::{DomString, Namespace};

use crate::names::{
    NameContext, valid_attribute_local_name, validate_and_extract,
};
use crate::observer::{Mutation, MutationType};
use crate::{AttrData, Attribute, Document, DomException, NodeData, NodeId};

/// Un argumento `(Node or DOMString)` de los mixins `ParentNode`/`ChildNode`.
#[derive(Debug, Clone)]
pub enum NodeOrString {
    Node(NodeId),
    String(DomString),
}

impl Document {
    // --- Nombres ---

    /// `Element.namespaceURI`.
    pub fn namespace_uri(&self, el: NodeId) -> Option<DomString> {
        let e = self.element(el)?;
        (e.ns != Namespace::Null).then(|| e.ns.uri().into())
    }

    /// `Element.prefix`.
    pub fn prefix(&self, el: NodeId) -> Option<DomString> {
        self.element(el)?.prefix.clone()
    }

    /// `Element.localName`.
    pub fn local_name(&self, el: NodeId) -> DomString {
        self.element(el).map(|e| e.name.clone()).unwrap_or_default()
    }

    /// `Element.tagName`: el qualified name, en mayúsculas ASCII si es HTML en un documento
    /// HTML.
    pub fn tag_name(&self, el: NodeId) -> DomString {
        let Some(e) = self.element(el) else {
            return DomString::new();
        };
        let q = e.qualified_name();
        if e.ns == Namespace::Html && self.is_html {
            q.ascii_uppercase()
        } else {
            q
        }
    }

    /// ¿Hay que pasar a minúsculas los nombres de atributo (elemento HTML en documento HTML)?
    fn lowercases_attr_names(&self, el: NodeId) -> bool {
        self.is_html && self.element(el).is_some_and(|e| e.ns == Namespace::Html)
    }

    // --- Atributos (§4.9.1) ---

    /// "Get an attribute by name": índice del primer atributo con ese qualified name.
    fn attr_index_by_name(&self, el: NodeId, name: &DomString) -> Option<usize> {
        let name = if self.lowercases_attr_names(el) {
            name.ascii_lowercase()
        } else {
            name.clone()
        };
        self.element(el)?
            .attrs
            .iter()
            .position(|a| a.qualified_name() == name)
    }

    fn attr_index_ns(&self, el: NodeId, ns: Option<Namespace>, local: &DomString) -> Option<usize> {
        self.element(el)?
            .attrs
            .iter()
            .position(|a| a.ns == ns && a.name == *local)
    }

    /// "Handle attribute changes".
    fn handle_attribute_changes(
        &mut self,
        el: NodeId,
        ns: Option<Namespace>,
        local: &DomString,
        old_value: Option<&DomString>,
    ) {
        self.queue_mutation_record(Mutation {
            type_: MutationType::Attributes,
            target: el,
            name: Some(local),
            namespace: ns,
            old_value,
            added: &[],
            removed: &[],
            previous: None,
            next: None,
        });
        self.bump_version();
    }

    /// "Change an attribute" (por namespace y local name) a `value`.
    pub(crate) fn change_attribute(
        &mut self,
        el: NodeId,
        ns: Option<Namespace>,
        local: &DomString,
        value: DomString,
    ) {
        let Some(i) = self.attr_index_ns(el, ns, local) else {
            return;
        };
        let old = {
            let e = self.element_mut(el).expect("elemento");
            std::mem::replace(&mut e.attrs[i].value, value)
        };
        self.handle_attribute_changes(el, ns, local, Some(&old));
    }

    /// "Append an attribute".
    pub(crate) fn append_attribute(&mut self, el: NodeId, attr: Attribute) {
        let (ns, local) = (attr.ns, attr.name.clone());
        self.element_mut(el).expect("elemento").attrs.push(attr);
        self.handle_attribute_changes(el, ns, &local, None);
    }

    /// "Remove an attribute" (el de índice `i`): el nodo `Attr`, si existe, se queda con el
    /// valor y sin elemento.
    fn remove_attribute_at(&mut self, el: NodeId, i: usize) -> Attribute {
        let attr = self.element_mut(el).expect("elemento").attrs.remove(i);
        if let Some(node) = self.attr_nodes.remove(&(el, attr.ns, attr.name.clone()))
            && let NodeData::Attr(a) = &mut self.get_mut(node).data
        {
            a.owner = None;
            a.value = attr.value.clone();
        }
        self.handle_attribute_changes(el, attr.ns, &attr.name, Some(&attr.value));
        attr
    }

    /// `Element.hasAttributes()`.
    pub fn has_attributes(&self, el: NodeId) -> bool {
        self.element(el).is_some_and(|e| !e.attrs.is_empty())
    }

    /// `Element.getAttributeNames()`.
    pub fn get_attribute_names(&self, el: NodeId) -> Vec<DomString> {
        self.element(el)
            .map(|e| e.attrs.iter().map(Attribute::qualified_name).collect())
            .unwrap_or_default()
    }

    /// `Element.getAttribute(qualifiedName)`.
    pub fn get_attribute(&self, el: NodeId, name: &DomString) -> Option<DomString> {
        let i = self.attr_index_by_name(el, name)?;
        Some(self.element(el)?.attrs[i].value.clone())
    }

    /// `Element.getAttributeNS(namespace, localName)`.
    pub fn get_attribute_ns(
        &self,
        el: NodeId,
        namespace: Option<&str>,
        local: &DomString,
    ) -> Option<DomString> {
        let ns = namespace.and_then(Namespace::from_uri);
        let i = self.attr_index_ns(el, ns, local)?;
        Some(self.element(el)?.attrs[i].value.clone())
    }

    /// `Element.setAttribute(qualifiedName, value)`.
    pub fn set_attribute(
        &mut self,
        el: NodeId,
        name: &DomString,
        value: DomString,
    ) -> Result<(), DomException> {
        if !valid_attribute_local_name(name) {
            return Err(DomException::InvalidCharacterError);
        }
        let name = if self.lowercases_attr_names(el) {
            name.ascii_lowercase()
        } else {
            name.clone()
        };
        match self.attr_index_by_name(el, &name) {
            None => self.append_attribute(el, Attribute::new(name, value)),
            Some(i) => {
                let (ns, local) = {
                    let a = &self.element(el).expect("elemento").attrs[i];
                    (a.ns, a.name.clone())
                };
                self.change_attribute(el, ns, &local, value);
            }
        }
        Ok(())
    }

    /// "Set an attribute value" (por namespace y local name).
    pub fn set_attribute_value(
        &mut self,
        el: NodeId,
        local: DomString,
        value: DomString,
        prefix: Option<DomString>,
        ns: Option<Namespace>,
    ) {
        if self.attr_index_ns(el, ns, &local).is_some() {
            self.change_attribute(el, ns, &local, value);
        } else {
            self.append_attribute(
                el,
                Attribute {
                    ns,
                    prefix,
                    name: local,
                    value,
                },
            );
        }
    }

    /// `Element.setAttributeNS(namespace, qualifiedName, value)`.
    pub fn set_attribute_ns(
        &mut self,
        el: NodeId,
        namespace: Option<&str>,
        qualified_name: &DomString,
        value: DomString,
    ) -> Result<(), DomException> {
        let (ns, prefix, local) =
            validate_and_extract(namespace, qualified_name, NameContext::Attribute)?;
        self.set_attribute_value(el, local, value, prefix, ns);
        Ok(())
    }

    /// `Element.removeAttribute(qualifiedName)`.
    pub fn remove_attribute(&mut self, el: NodeId, name: &DomString) {
        if let Some(i) = self.attr_index_by_name(el, name) {
            self.remove_attribute_at(el, i);
        }
    }

    /// `Element.removeAttributeNS(namespace, localName)`.
    pub fn remove_attribute_ns(&mut self, el: NodeId, namespace: Option<&str>, local: &DomString) {
        let ns = namespace.and_then(Namespace::from_uri);
        if let Some(i) = self.attr_index_ns(el, ns, local) {
            self.remove_attribute_at(el, i);
        }
    }

    /// `Element.toggleAttribute(qualifiedName, force)`.
    pub fn toggle_attribute(
        &mut self,
        el: NodeId,
        name: &DomString,
        force: Option<bool>,
    ) -> Result<bool, DomException> {
        if !valid_attribute_local_name(name) {
            return Err(DomException::InvalidCharacterError);
        }
        let name = if self.lowercases_attr_names(el) {
            name.ascii_lowercase()
        } else {
            name.clone()
        };
        match self.attr_index_by_name(el, &name) {
            None => {
                if force != Some(false) {
                    self.append_attribute(el, Attribute::new(name, ""));
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            Some(i) => {
                if force != Some(true) {
                    self.remove_attribute_at(el, i);
                    Ok(false)
                } else {
                    Ok(true)
                }
            }
        }
    }

    /// `Element.hasAttribute(qualifiedName)`.
    pub fn has_attribute(&self, el: NodeId, name: &DomString) -> bool {
        self.attr_index_by_name(el, name).is_some()
    }

    /// `Element.hasAttributeNS(namespace, localName)`.
    pub fn has_attribute_ns(&self, el: NodeId, namespace: Option<&str>, local: &DomString) -> bool {
        let ns = namespace.and_then(Namespace::from_uri);
        self.attr_index_ns(el, ns, local).is_some()
    }

    // --- Nodos Attr y NamedNodeMap ---

    /// El nodo `Attr` del atributo `i` de `el` (se crea la primera vez).
    fn attr_node_at(&mut self, el: NodeId, i: usize) -> NodeId {
        let a = self.element(el).expect("elemento").attrs[i].clone();
        let key = (el, a.ns, a.name.clone());
        if let Some(&n) = self.attr_nodes.get(&key) {
            return n;
        }
        let node = self.create(NodeData::Attr(AttrData {
            ns: a.ns,
            prefix: a.prefix,
            local_name: a.name,
            value: a.value,
            owner: Some(el),
        }));
        self.attr_nodes.insert(key, node);
        node
    }

    /// `NamedNodeMap.length` de `el.attributes`.
    pub fn attributes_length(&self, el: NodeId) -> usize {
        self.element(el).map_or(0, |e| e.attrs.len())
    }

    /// `NamedNodeMap.item(index)`.
    pub fn attributes_item(&mut self, el: NodeId, index: usize) -> Option<NodeId> {
        (index < self.attributes_length(el)).then(|| self.attr_node_at(el, index))
    }

    /// `Element.getAttributeNode(qualifiedName)` / `NamedNodeMap.getNamedItem`.
    pub fn get_attribute_node(&mut self, el: NodeId, name: &DomString) -> Option<NodeId> {
        let i = self.attr_index_by_name(el, name)?;
        Some(self.attr_node_at(el, i))
    }

    /// `Element.getAttributeNodeNS` / `NamedNodeMap.getNamedItemNS`.
    pub fn get_attribute_node_ns(
        &mut self,
        el: NodeId,
        namespace: Option<&str>,
        local: &DomString,
    ) -> Option<NodeId> {
        let ns = namespace.and_then(Namespace::from_uri);
        let i = self.attr_index_ns(el, ns, local)?;
        Some(self.attr_node_at(el, i))
    }

    /// "Set an attribute" (`setAttributeNode`, `setNamedItem`): devuelve el `Attr` anterior.
    pub fn set_attribute_node(
        &mut self,
        el: NodeId,
        attr: NodeId,
    ) -> Result<Option<NodeId>, DomException> {
        let data = self.attr_data(attr).clone();
        // 1.
        if data.owner.is_some_and(|o| o != el) {
            return Err(DomException::InUseAttributeError);
        }
        // 2-3.
        let old_index = self.attr_index_ns(el, data.ns, &data.local_name);
        if let Some(i) = old_index
            && self.attr_nodes.get(&(el, data.ns, data.local_name.clone())) == Some(&attr)
        {
            let _ = i;
            return Ok(Some(attr));
        }
        let value = data.value.clone();
        let old_node = old_index.map(|i| self.attr_node_at(el, i));
        match old_index {
            Some(i) => {
                // "Replace an attribute".
                let old_value = self.element(el).expect("elemento").attrs[i].value.clone();
                if let Some(o) = old_node
                    && let NodeData::Attr(a) = &mut self.get_mut(o).data
                {
                    a.owner = None;
                    a.value = old_value.clone();
                }
                self.attr_nodes.remove(&(el, data.ns, data.local_name.clone()));
                self.element_mut(el).expect("elemento").attrs[i] = Attribute {
                    ns: data.ns,
                    prefix: data.prefix.clone(),
                    name: data.local_name.clone(),
                    value,
                };
                self.handle_attribute_changes(el, data.ns, &data.local_name, Some(&old_value));
            }
            None => {
                self.append_attribute(
                    el,
                    Attribute {
                        ns: data.ns,
                        prefix: data.prefix.clone(),
                        name: data.local_name.clone(),
                        value,
                    },
                );
            }
        }
        if let NodeData::Attr(a) = &mut self.get_mut(attr).data {
            a.owner = Some(el);
        }
        self.attr_nodes
            .insert((el, data.ns, data.local_name.clone()), attr);
        Ok(old_node)
    }

    /// `Element.removeAttributeNode(attr)` / `NamedNodeMap.removeNamedItem*`.
    pub fn remove_attribute_node(&mut self, el: NodeId, attr: NodeId) -> Result<NodeId, DomException> {
        let data = self.attr_data(attr).clone();
        if data.owner != Some(el) {
            return Err(DomException::NotFoundError);
        }
        let i = self
            .attr_index_ns(el, data.ns, &data.local_name)
            .ok_or(DomException::NotFoundError)?;
        self.remove_attribute_at(el, i);
        Ok(attr)
    }

    /// `NamedNodeMap.removeNamedItem(qualifiedName)`.
    pub fn remove_named_item(&mut self, el: NodeId, name: &DomString) -> Result<NodeId, DomException> {
        let node = self
            .get_attribute_node(el, name)
            .ok_or(DomException::NotFoundError)?;
        self.remove_attribute_node(el, node)
    }

    /// `Attr.ownerElement`.
    pub fn owner_element(&self, attr: NodeId) -> Option<NodeId> {
        self.attr_data(attr).owner
    }

    // --- Atributos reflejados ---

    /// `Element.id`.
    pub fn id(&self, el: NodeId) -> DomString {
        self.get_attribute_ns(el, None, &"id".into()).unwrap_or_default()
    }

    pub fn set_id(&mut self, el: NodeId, value: DomString) {
        self.set_attribute_value(el, "id".into(), value, None, None);
    }

    /// `Element.className`.
    pub fn class_name(&self, el: NodeId) -> DomString {
        self.get_attribute_ns(el, None, &"class".into())
            .unwrap_or_default()
    }

    pub fn set_class_name(&mut self, el: NodeId, value: DomString) {
        self.set_attribute_value(el, "class".into(), value, None, None);
    }

    /// `Element.classList`.
    pub fn class_list(&self, el: NodeId) -> DomTokenList {
        DomTokenList {
            element: el,
            local_name: "class".into(),
        }
    }

    // --- Traversal (ParentNode, NonDocumentTypeChildNode) ---

    /// Hijos elemento de `n` (`ParentNode.children`, instantánea).
    pub fn element_children(&self, n: NodeId) -> Vec<NodeId> {
        self.children(n).filter(|&c| self.element(c).is_some()).collect()
    }

    pub fn first_element_child(&self, n: NodeId) -> Option<NodeId> {
        self.children(n).find(|&c| self.element(c).is_some())
    }

    pub fn last_element_child(&self, n: NodeId) -> Option<NodeId> {
        let mut cur = self.get(n).last_child();
        while let Some(c) = cur {
            if self.element(c).is_some() {
                return Some(c);
            }
            cur = self.get(c).prev_sibling();
        }
        None
    }

    pub fn child_element_count(&self, n: NodeId) -> usize {
        self.children(n).filter(|&c| self.element(c).is_some()).count()
    }

    pub fn previous_element_sibling(&self, n: NodeId) -> Option<NodeId> {
        let mut cur = self.get(n).prev_sibling();
        while let Some(c) = cur {
            if self.element(c).is_some() {
                return Some(c);
            }
            cur = self.get(c).prev_sibling();
        }
        None
    }

    pub fn next_element_sibling(&self, n: NodeId) -> Option<NodeId> {
        let mut cur = self.get(n).next_sibling();
        while let Some(c) = cur {
            if self.element(c).is_some() {
                return Some(c);
            }
            cur = self.get(c).next_sibling();
        }
        None
    }

    // --- ParentNode / ChildNode ---

    /// "Convert nodes into a node": un único nodo, o un fragmento con todos (el *append* al
    /// fragmento puede lanzar, p. ej. con un `Document`).
    pub fn convert_nodes_into_node(
        &mut self,
        nodes: Vec<NodeOrString>,
    ) -> Result<NodeId, DomException> {
        let mut ids: Vec<NodeId> = nodes
            .into_iter()
            .map(|n| match n {
                NodeOrString::Node(id) => id,
                NodeOrString::String(s) => self.create(NodeData::Text(s)),
            })
            .collect();
        if ids.len() == 1 {
            return Ok(ids.remove(0));
        }
        let fragment = self.create(NodeData::DocumentFragment);
        for id in ids {
            self.pre_insert(id, fragment, None)?;
        }
        Ok(fragment)
    }

    /// `ParentNode.prepend(...nodes)`.
    pub fn prepend(&mut self, parent: NodeId, nodes: Vec<NodeOrString>) -> Result<(), DomException> {
        let node = self.convert_nodes_into_node(nodes)?;
        let first = self.get(parent).first_child();
        self.pre_insert(node, parent, first).map(|_| ())
    }

    /// `ParentNode.append(...nodes)`.
    pub fn append_nodes(
        &mut self,
        parent: NodeId,
        nodes: Vec<NodeOrString>,
    ) -> Result<(), DomException> {
        let node = self.convert_nodes_into_node(nodes)?;
        self.pre_insert(node, parent, None).map(|_| ())
    }

    /// `ParentNode.replaceChildren(...nodes)`.
    pub fn replace_children(
        &mut self,
        parent: NodeId,
        nodes: Vec<NodeOrString>,
    ) -> Result<(), DomException> {
        let node = self.convert_nodes_into_node(nodes)?;
        self.ensure_pre_insertion_validity(node, parent, None)?;
        self.replace_all(Some(node), parent);
        Ok(())
    }

    fn node_set(nodes: &[NodeOrString]) -> Vec<NodeId> {
        nodes
            .iter()
            .filter_map(|n| match n {
                NodeOrString::Node(id) => Some(*id),
                NodeOrString::String(_) => None,
            })
            .collect()
    }

    /// `ChildNode.before(...nodes)`.
    pub fn before(&mut self, this: NodeId, nodes: Vec<NodeOrString>) -> Result<(), DomException> {
        let Some(parent) = self.parent(this) else {
            return Ok(());
        };
        let set = Self::node_set(&nodes);
        let mut viable_prev = self.get(this).prev_sibling();
        while let Some(p) = viable_prev
            && set.contains(&p)
        {
            viable_prev = self.get(p).prev_sibling();
        }
        let node = self.convert_nodes_into_node(nodes)?;
        let reference = match viable_prev {
            None => self.get(parent).first_child(),
            Some(p) => self.get(p).next_sibling(),
        };
        self.pre_insert(node, parent, reference).map(|_| ())
    }

    /// `ChildNode.after(...nodes)`.
    pub fn after(&mut self, this: NodeId, nodes: Vec<NodeOrString>) -> Result<(), DomException> {
        let Some(parent) = self.parent(this) else {
            return Ok(());
        };
        let set = Self::node_set(&nodes);
        let mut viable_next = self.get(this).next_sibling();
        while let Some(n) = viable_next
            && set.contains(&n)
        {
            viable_next = self.get(n).next_sibling();
        }
        let node = self.convert_nodes_into_node(nodes)?;
        self.pre_insert(node, parent, viable_next).map(|_| ())
    }

    /// `ChildNode.replaceWith(...nodes)`.
    pub fn replace_with(&mut self, this: NodeId, nodes: Vec<NodeOrString>) -> Result<(), DomException> {
        let Some(parent) = self.parent(this) else {
            return Ok(());
        };
        let set = Self::node_set(&nodes);
        let mut viable_next = self.get(this).next_sibling();
        while let Some(n) = viable_next
            && set.contains(&n)
        {
            viable_next = self.get(n).next_sibling();
        }
        let node = self.convert_nodes_into_node(nodes)?;
        if self.parent(this) == Some(parent) {
            self.replace(this, node, parent).map(|_| ())
        } else {
            self.pre_insert(node, parent, viable_next).map(|_| ())
        }
    }

    /// `ChildNode.remove()`.
    pub fn remove_self(&mut self, this: NodeId) {
        self.remove(this);
    }

    /// "Insert adjacent" (`insertAdjacentElement`/`insertAdjacentText`).
    pub fn insert_adjacent(
        &mut self,
        element: NodeId,
        where_: &str,
        node: NodeId,
    ) -> Result<Option<NodeId>, DomException> {
        match where_.to_ascii_lowercase().as_str() {
            "beforebegin" => match self.parent(element) {
                None => Ok(None),
                Some(p) => self.pre_insert(node, p, Some(element)).map(Some),
            },
            "afterbegin" => {
                let first = self.get(element).first_child();
                self.pre_insert(node, element, first).map(Some)
            }
            "beforeend" => self.pre_insert(node, element, None).map(Some),
            "afterend" => match self.parent(element) {
                None => Ok(None),
                Some(p) => {
                    let next = self.get(element).next_sibling();
                    self.pre_insert(node, p, next).map(Some)
                }
            },
            _ => Err(DomException::SyntaxError),
        }
    }

    /// `Element.insertAdjacentElement(where, element)`.
    pub fn insert_adjacent_element(
        &mut self,
        this: NodeId,
        where_: &str,
        element: NodeId,
    ) -> Result<Option<NodeId>, DomException> {
        self.insert_adjacent(this, where_, element)
    }

    /// `Element.insertAdjacentText(where, data)`.
    pub fn insert_adjacent_text(
        &mut self,
        this: NodeId,
        where_: &str,
        data: DomString,
    ) -> Result<(), DomException> {
        let text = self.create(NodeData::Text(data));
        self.insert_adjacent(this, where_, text).map(|_| ())
    }
}

/// Error de validación de un token de `DOMTokenList`.
pub type TokenListError = DomException;

/// `DOMTokenList` (§7.1) asociado a un atributo de un elemento (`classList`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomTokenList {
    pub element: NodeId,
    pub local_name: DomString,
}

fn is_ascii_whitespace(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' ')
}

/// "Ordered set parser": tokens separados por ASCII whitespace, sin duplicados.
pub(crate) fn ordered_set(value: &DomString) -> Vec<DomString> {
    let mut out: Vec<DomString> = Vec::new();
    let mut current = DomString::new();
    let flush = |current: &mut DomString, out: &mut Vec<DomString>| {
        if !current.is_empty() {
            let t = std::mem::take(current);
            if !out.contains(&t) {
                out.push(t);
            }
        }
    };
    for cp in value.code_points() {
        if char::from_u32(cp).is_some_and(is_ascii_whitespace) {
            flush(&mut current, &mut out);
        } else {
            current.push_code_point(cp);
        }
    }
    flush(&mut current, &mut out);
    out
}

fn serialize_set(tokens: &[DomString]) -> DomString {
    let mut out = DomString::new();
    for (i, t) in tokens.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_dom_str(t);
    }
    out
}

fn validate_token(token: &DomString) -> Result<(), DomException> {
    if token.is_empty() {
        return Err(DomException::SyntaxError);
    }
    if token.chars().any(is_ascii_whitespace) {
        return Err(DomException::InvalidCharacterError);
    }
    Ok(())
}

impl DomTokenList {
    fn attr_value(&self, doc: &Document) -> Option<DomString> {
        doc.get_attribute_ns(self.element, None, &self.local_name)
    }

    /// El token set.
    pub fn tokens(&self, doc: &Document) -> Vec<DomString> {
        self.attr_value(doc).map(|v| ordered_set(&v)).unwrap_or_default()
    }

    /// "Update steps".
    fn update(&self, doc: &mut Document, tokens: &[DomString]) {
        if self.attr_value(doc).is_none() && tokens.is_empty() {
            return;
        }
        doc.set_attribute_value(
            self.element,
            self.local_name.clone(),
            serialize_set(tokens),
            None,
            None,
        );
    }

    pub fn length(&self, doc: &Document) -> usize {
        self.tokens(doc).len()
    }

    pub fn item(&self, doc: &Document, index: usize) -> Option<DomString> {
        self.tokens(doc).into_iter().nth(index)
    }

    pub fn contains(&self, doc: &Document, token: &DomString) -> bool {
        self.tokens(doc).contains(token)
    }

    pub fn add(&self, doc: &mut Document, tokens: &[DomString]) -> Result<(), DomException> {
        for t in tokens {
            validate_token(t)?;
        }
        let mut set = self.tokens(doc);
        for t in tokens {
            if !set.contains(t) {
                set.push(t.clone());
            }
        }
        self.update(doc, &set);
        Ok(())
    }

    pub fn remove(&self, doc: &mut Document, tokens: &[DomString]) -> Result<(), DomException> {
        for t in tokens {
            validate_token(t)?;
        }
        let mut set = self.tokens(doc);
        set.retain(|x| !tokens.contains(x));
        self.update(doc, &set);
        Ok(())
    }

    pub fn toggle(
        &self,
        doc: &mut Document,
        token: &DomString,
        force: Option<bool>,
    ) -> Result<bool, DomException> {
        validate_token(token)?;
        let mut set = self.tokens(doc);
        if set.contains(token) {
            if force != Some(true) {
                set.retain(|x| x != token);
                self.update(doc, &set);
                return Ok(false);
            }
            return Ok(true);
        }
        if force != Some(false) {
            set.push(token.clone());
            self.update(doc, &set);
            return Ok(true);
        }
        Ok(false)
    }

    pub fn replace(
        &self,
        doc: &mut Document,
        token: &DomString,
        new_token: &DomString,
    ) -> Result<bool, DomException> {
        validate_token(token)?;
        validate_token(new_token)?;
        let mut set = self.tokens(doc);
        if !set.contains(token) {
            return Ok(false);
        }
        // "Replace" de un ordered set (Infra): la primera aparición de cualquiera de los dos
        // pasa a ser `new_token` y se quitan las demás.
        let i = set
            .iter()
            .position(|x| x == token || x == new_token)
            .expect("presente");
        set[i] = new_token.clone();
        let mut k = 0;
        set.retain(|x| {
            let keep = k == i || (x != token && x != new_token);
            k += 1;
            keep
        });
        self.update(doc, &set);
        Ok(true)
    }

    /// `supports(token)`: `class` no define supported tokens, así que lanza `TypeError`.
    pub fn supports(&self, _token: &DomString) -> Result<bool, DomException> {
        Err(DomException::TypeError)
    }

    /// `value` (el valor del atributo).
    pub fn value(&self, doc: &Document) -> DomString {
        self.attr_value(doc).unwrap_or_default()
    }

    pub fn set_value(&self, doc: &mut Document, value: DomString) {
        doc.set_attribute_value(self.element, self.local_name.clone(), value, None, None);
    }
}
