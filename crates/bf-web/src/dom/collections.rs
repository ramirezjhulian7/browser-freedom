//! Colecciones (DOM §4.2.10, §4.9.1, §7.1): `NodeList` (estática o viva), `HTMLCollection`
//! viva, `DOMTokenList` y `NamedNodeMap`, con sus propiedades indexadas y con nombre
//! (objetos de plataforma legacy, WebIDL §3.9).

use bf_dom::collections::HtmlCollection;
use bf_dom::{DomTokenList, NodeId};
use bf_infra::{DomString, Namespace};
use bf_js::heap::Gc;
use bf_js::host::{HostClass, HostExotic};
use bf_js::object::Object;
use bf_js::string::JsString;
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, RealmId};

use crate::dom::{
    classes, from_dom_string, mutated, throw_dom, to_dom_string, unwrap_node, wrap_node, wrap_opt,
};
use crate::{DocIndex, NodeRef, WebContextHandle, with_doc, with_doc_mut};

/// El payload de una `NodeList`.
#[derive(Debug, Clone)]
pub enum NodeListData {
    /// Una lista estática (`querySelectorAll`, `getElementsByName`).
    Static { doc: DocIndex, nodes: Vec<NodeId> },
    /// `childNodes`: viva.
    Children { doc: DocIndex, parent: NodeId },
}

/// El payload de una `HTMLCollection`.
#[derive(Debug, Clone)]
pub struct HtmlCollectionData {
    pub doc: DocIndex,
    pub coll: HtmlCollection,
}

/// El payload de un `DOMTokenList`.
#[derive(Debug, Clone)]
pub struct TokenListData {
    pub doc: DocIndex,
    pub list: DomTokenList,
}

/// El payload de un `NamedNodeMap`.
#[derive(Debug, Clone, Copy)]
pub struct NamedNodeMapData {
    pub doc: DocIndex,
    pub element: NodeId,
}

pub fn node_list_class() -> HostClass {
    HostClass::new("NodeList")
        .attribute("length", Some(nl_length), None)
        .method("item", 1, nl_item)
        .exotic(HostExotic {
            length: Some(nl_len),
            item: Some(nl_at),
            ..HostExotic::default()
        })
}

pub fn html_collection_class() -> HostClass {
    HostClass::new("HTMLCollection")
        .attribute("length", Some(hc_length), None)
        .method("item", 1, hc_item)
        .method("namedItem", 1, hc_named_item)
        .exotic(HostExotic {
            length: Some(hc_len),
            item: Some(hc_at),
            named: Some(hc_named),
            names: Some(hc_names),
            named_enumerable: false,
            override_builtins: false,
        })
}

pub fn dom_token_list_class() -> HostClass {
    HostClass::new("DOMTokenList")
        .attribute("length", Some(tl_length), None)
        .attribute("value", Some(tl_value), Some(tl_set_value))
        .method("item", 1, tl_item)
        .method("contains", 1, tl_contains)
        .method("add", 0, tl_add)
        .method("remove", 0, tl_remove)
        .method("toggle", 1, tl_toggle)
        .method("replace", 2, tl_replace)
        .method("supports", 1, tl_supports)
        .method("toString", 0, tl_value)
        .exotic(HostExotic {
            length: Some(tl_len),
            item: Some(tl_at),
            ..HostExotic::default()
        })
}

pub fn named_node_map_class() -> HostClass {
    HostClass::new("NamedNodeMap")
        .attribute("length", Some(nm_length), None)
        .method("item", 1, nm_item)
        .method("getNamedItem", 1, nm_get_named_item)
        .method("getNamedItemNS", 2, nm_get_named_item_ns)
        .method("setNamedItem", 1, nm_set_named_item)
        .method("setNamedItemNS", 1, nm_set_named_item)
        .method("removeNamedItem", 1, nm_remove_named_item)
        .method("removeNamedItemNS", 2, nm_remove_named_item_ns)
        .exotic(HostExotic {
            length: Some(nm_len),
            item: Some(nm_at),
            named: Some(nm_named),
            names: Some(nm_names),
            named_enumerable: false,
            override_builtins: false,
        })
}

/// En cada realm: `@@iterator` (y, para `NodeList` y `DOMTokenList`, `entries`, `keys`,
/// `values` y `forEach`) de `%Array.prototype%` (WebIDL §3.7.9, iterables de valores).
pub fn finish_realm(agent: &mut Agent, realm: RealmId) {
    let c = classes(agent).clone();
    let ap = agent.realms[realm].intrinsics.array_prototype;
    let get = |agent: &Agent, name: &str| {
        agent.heap[ap]
            .props
            .get(&PropertyKey::from(name))
            .and_then(|p| p.to_descriptor().value)
    };
    let values = get(agent, "values");
    let iter = PropertyKey::Symbol(agent.well_known.iterator);
    for (class, iterable) in [
        (c.node_list, true),
        (c.dom_token_list, true),
        (c.html_collection, false),
        (c.named_node_map, false),
    ] {
        let proto = agent.host_class_objects(realm, class).prototype;
        if let Some(v) = &values {
            agent.put_data(proto, iter.clone(), v.clone(), true, false, true);
        }
        if iterable {
            for name in ["entries", "keys", "values", "forEach"] {
                if let Some(f) = get(agent, name) {
                    agent.put_data(proto, PropertyKey::from(name), f, true, true, true);
                }
            }
        }
    }
}

fn payload<T: Clone + 'static>(agent: &mut Agent, v: &Value) -> JsResult<T> {
    match agent.host_unwrap::<T>(v) {
        Some(p) => Ok(p.clone()),
        None => agent.type_error("Illegal invocation"),
    }
}

fn payload_of<T: Clone + 'static>(agent: &Agent, o: Gc<Object>) -> Option<T> {
    agent.host_unwrap::<T>(&Value::Object(o)).cloned()
}

// --- NodeList ---

/// Una `NodeList` estática con `nodes`.
pub fn static_node_list(agent: &mut Agent, doc: DocIndex, nodes: Vec<NodeId>) -> Value {
    let class = classes(agent).node_list;
    Value::Object(agent.host_create(class, Box::new(NodeListData::Static { doc, nodes })))
}

/// `childNodes` de `parent` (el mismo objeto siempre: se guarda en la tabla de wrappers con
/// una clave propia).
pub fn child_nodes(agent: &mut Agent, parent: NodeRef) -> Value {
    let realm = agent.current_realm;
    let key = bf_js::host::HostKey(parent.key().0 | (1 << 62));
    if let Some(o) = agent.host_lookup(realm, key) {
        return Value::Object(o);
    }
    let class = classes(agent).node_list;
    Value::Object(agent.host_wrap(
        realm,
        key,
        class,
        Box::new(NodeListData::Children {
            doc: parent.doc,
            parent: parent.node,
        }),
    ))
}

fn nl_nodes(agent: &Agent, d: &NodeListData) -> (DocIndex, Vec<NodeId>) {
    match d {
        NodeListData::Static { doc, nodes } => (*doc, nodes.clone()),
        NodeListData::Children { doc, parent } => {
            let ctx = WebContextHandle::current(agent).get(agent);
            (*doc, ctx.doc(*doc).child_nodes(*parent))
        }
    }
}

fn nl_len(agent: &mut Agent, o: Gc<Object>) -> usize {
    payload_of::<NodeListData>(agent, o).map_or(0, |d| nl_nodes(agent, &d).1.len())
}

fn nl_at(agent: &mut Agent, o: Gc<Object>, i: u32) -> Option<Value> {
    let d = payload_of::<NodeListData>(agent, o)?;
    let (doc, nodes) = nl_nodes(agent, &d);
    let n = *nodes.get(i as usize)?;
    Some(wrap_node(agent, NodeRef::new(doc, n)))
}

fn nl_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d: NodeListData = payload(agent, &a.this)?;
    Ok(Value::Number(nl_nodes(agent, &d).1.len() as f64))
}

fn nl_item(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d: NodeListData = payload(agent, &a.this)?;
    let i = bf_js::host::webidl::to_unsigned_long(agent, &a.arg(0))? as usize;
    let (doc, nodes) = nl_nodes(agent, &d);
    Ok(wrap_opt(agent, doc, nodes.get(i).copied()))
}

// --- HTMLCollection ---

/// Una `HTMLCollection` viva.
pub fn html_collection(agent: &mut Agent, doc: DocIndex, coll: HtmlCollection) -> Value {
    let class = classes(agent).html_collection;
    Value::Object(agent.host_create(class, Box::new(HtmlCollectionData { doc, coll })))
}

fn hc_items(agent: &Agent, d: &HtmlCollectionData) -> Vec<NodeId> {
    let ctx = WebContextHandle::current(agent).get(agent);
    d.coll.items(ctx.doc(d.doc))
}

fn hc_len(agent: &mut Agent, o: Gc<Object>) -> usize {
    payload_of::<HtmlCollectionData>(agent, o).map_or(0, |d| hc_items(agent, &d).len())
}

fn hc_at(agent: &mut Agent, o: Gc<Object>, i: u32) -> Option<Value> {
    let d = payload_of::<HtmlCollectionData>(agent, o)?;
    let n = *hc_items(agent, &d).get(i as usize)?;
    Some(wrap_node(agent, NodeRef::new(d.doc, n)))
}

fn hc_named(agent: &mut Agent, o: Gc<Object>, name: &JsString) -> Option<Value> {
    let d = payload_of::<HtmlCollectionData>(agent, o)?;
    let key = DomString::from_utf16(name.units());
    let n = with_doc(agent, d.doc, |doc| d.coll.named_item(doc, &key))?;
    Some(wrap_node(agent, NodeRef::new(d.doc, n)))
}

/// Los nombres soportados (DOM §4.2.10.2): los `id` y, de los elementos HTML, los `name`.
fn hc_names(agent: &mut Agent, o: Gc<Object>) -> Vec<JsString> {
    let Some(d) = payload_of::<HtmlCollectionData>(agent, o) else {
        return Vec::new();
    };
    let items = hc_items(agent, &d);
    with_doc(agent, d.doc, |doc| {
        let mut out: Vec<DomString> = Vec::new();
        for n in items {
            let Some(e) = doc.element(n) else { continue };
            for a in &e.attrs {
                if a.ns.is_none()
                    && !a.value.is_empty()
                    && (a.name == "id" || (a.name == "name" && e.ns == Namespace::Html))
                    && !out.contains(&a.value)
                {
                    out.push(a.value.clone());
                }
            }
        }
        out.iter()
            .map(|s| JsString::from_units(s.to_utf16()))
            .collect()
    })
}

fn hc_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d: HtmlCollectionData = payload(agent, &a.this)?;
    Ok(Value::Number(hc_items(agent, &d).len() as f64))
}

fn hc_item(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d: HtmlCollectionData = payload(agent, &a.this)?;
    let i = bf_js::host::webidl::to_unsigned_long(agent, &a.arg(0))? as usize;
    let n = hc_items(agent, &d).get(i).copied();
    Ok(wrap_opt(agent, d.doc, n))
}

fn hc_named_item(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d: HtmlCollectionData = payload(agent, &a.this)?;
    let key = to_dom_string(agent, &a.arg(0))?;
    let n = with_doc(agent, d.doc, |doc| d.coll.named_item(doc, &key));
    Ok(wrap_opt(agent, d.doc, n))
}

// --- DOMTokenList ---

/// El `DOMTokenList` del atributo `attr` de `el` (siempre el mismo objeto).
pub fn token_list(agent: &mut Agent, el: NodeRef, attr: &str) -> Value {
    let realm = agent.current_realm;
    // La clave distingue el atributo (solo `class` y unos pocos de HTML).
    let salt: u64 = attr
        .bytes()
        .fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(u64::from(b)));
    let key = bf_js::host::HostKey(el.key().0 ^ ((salt & 0x3fff) << 48) | (1 << 61));
    if let Some(o) = agent.host_lookup(realm, key) {
        return Value::Object(o);
    }
    let class = classes(agent).dom_token_list;
    Value::Object(agent.host_wrap(
        realm,
        key,
        class,
        Box::new(TokenListData {
            doc: el.doc,
            list: DomTokenList {
                element: el.node,
                local_name: attr.into(),
            },
        }),
    ))
}

fn tl_tokens(agent: &Agent, d: &TokenListData) -> Vec<DomString> {
    let ctx = WebContextHandle::current(agent).get(agent);
    d.list.tokens(ctx.doc(d.doc))
}

fn tl_len(agent: &mut Agent, o: Gc<Object>) -> usize {
    payload_of::<TokenListData>(agent, o).map_or(0, |d| tl_tokens(agent, &d).len())
}

fn tl_at(agent: &mut Agent, o: Gc<Object>, i: u32) -> Option<Value> {
    let d = payload_of::<TokenListData>(agent, o)?;
    tl_tokens(agent, &d).get(i as usize).map(from_dom_string)
}

fn tl_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d: TokenListData = payload(agent, &a.this)?;
    Ok(Value::Number(tl_tokens(agent, &d).len() as f64))
}

fn tl_item(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d: TokenListData = payload(agent, &a.this)?;
    let i = bf_js::host::webidl::to_unsigned_long(agent, &a.arg(0))? as usize;
    Ok(tl_tokens(agent, &d)
        .get(i)
        .map_or(Value::Null, from_dom_string))
}

fn tl_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d: TokenListData = payload(agent, &a.this)?;
    let v = with_doc(agent, d.doc, |doc| d.list.value(doc));
    Ok(from_dom_string(&v))
}

fn tl_set_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d: TokenListData = payload(agent, &a.this)?;
    let v = to_dom_string(agent, &a.arg(0))?;
    with_doc_mut(agent, d.doc, |doc| d.list.set_value(doc, v));
    mutated(agent);
    Ok(Value::Undefined)
}

fn tl_contains(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d: TokenListData = payload(agent, &a.this)?;
    let t = to_dom_string(agent, &a.arg(0))?;
    Ok(Value::Boolean(with_doc(agent, d.doc, |doc| {
        d.list.contains(doc, &t)
    })))
}

fn tokens_args(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Vec<DomString>> {
    a.args.iter().map(|v| to_dom_string(agent, v)).collect()
}

fn tl_add(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d: TokenListData = payload(agent, &a.this)?;
    let tokens = tokens_args(agent, a)?;
    let r = with_doc_mut(agent, d.doc, |doc| d.list.add(doc, &tokens));
    crate::dom::node::unit(agent, r)
}

fn tl_remove(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d: TokenListData = payload(agent, &a.this)?;
    let tokens = tokens_args(agent, a)?;
    let r = with_doc_mut(agent, d.doc, |doc| d.list.remove(doc, &tokens));
    crate::dom::node::unit(agent, r)
}

fn tl_toggle(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d: TokenListData = payload(agent, &a.this)?;
    let t = to_dom_string(agent, &a.arg(0))?;
    let force = if a.arg(1).is_undefined() {
        None
    } else {
        Some(bf_js::host::webidl::to_boolean(agent, &a.arg(1)))
    };
    let r = with_doc_mut(agent, d.doc, |doc| d.list.toggle(doc, &t, force));
    match r {
        Ok(b) => {
            mutated(agent);
            Ok(Value::Boolean(b))
        }
        Err(e) => throw_dom(agent, e),
    }
}

fn tl_replace(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d: TokenListData = payload(agent, &a.this)?;
    let t = to_dom_string(agent, &a.arg(0))?;
    let n = to_dom_string(agent, &a.arg(1))?;
    let r = with_doc_mut(agent, d.doc, |doc| d.list.replace(doc, &t, &n));
    match r {
        Ok(b) => {
            mutated(agent);
            Ok(Value::Boolean(b))
        }
        Err(e) => throw_dom(agent, e),
    }
}

fn tl_supports(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d: TokenListData = payload(agent, &a.this)?;
    let t = to_dom_string(agent, &a.arg(0))?;
    match d.list.supports(&t) {
        Ok(_) => agent.type_error(
            "Failed to execute 'supports' on 'DOMTokenList': DOMTokenList has no supported \
             tokens.",
        ),
        Err(e) => throw_dom(agent, e),
    }
}

// --- NamedNodeMap ---

/// `Element.attributes` (siempre el mismo objeto).
pub fn named_node_map(agent: &mut Agent, el: NodeRef) -> Value {
    let realm = agent.current_realm;
    let key = bf_js::host::HostKey(el.key().0 | (1 << 60));
    if let Some(o) = agent.host_lookup(realm, key) {
        return Value::Object(o);
    }
    let class = classes(agent).named_node_map;
    Value::Object(agent.host_wrap(
        realm,
        key,
        class,
        Box::new(NamedNodeMapData {
            doc: el.doc,
            element: el.node,
        }),
    ))
}

fn nm_len(agent: &mut Agent, o: Gc<Object>) -> usize {
    payload_of::<NamedNodeMapData>(agent, o).map_or(0, |d| {
        with_doc(agent, d.doc, |doc| doc.attributes_length(d.element))
    })
}

fn nm_at(agent: &mut Agent, o: Gc<Object>, i: u32) -> Option<Value> {
    let d = payload_of::<NamedNodeMapData>(agent, o)?;
    let a = with_doc_mut(agent, d.doc, |doc| {
        doc.attributes_item(d.element, i as usize)
    })?;
    Some(wrap_node(agent, NodeRef::new(d.doc, a)))
}

fn nm_named(agent: &mut Agent, o: Gc<Object>, name: &JsString) -> Option<Value> {
    let d = payload_of::<NamedNodeMapData>(agent, o)?;
    let name = DomString::from_utf16(name.units());
    let a = with_doc_mut(agent, d.doc, |doc| doc.get_attribute_node(d.element, &name))?;
    Some(wrap_node(agent, NodeRef::new(d.doc, a)))
}

/// Los nombres soportados (DOM §4.9.2): los qualified names, sin los que tienen mayúsculas
/// ASCII en elementos HTML de documentos HTML.
fn nm_names(agent: &mut Agent, o: Gc<Object>) -> Vec<JsString> {
    let Some(d) = payload_of::<NamedNodeMapData>(agent, o) else {
        return Vec::new();
    };
    with_doc(agent, d.doc, |doc| {
        let Some(e) = doc.element(d.element) else {
            return Vec::new();
        };
        let html = e.ns == Namespace::Html && doc.is_html;
        let mut out: Vec<DomString> = Vec::new();
        for a in &e.attrs {
            let q = a.qualified_name();
            if html && q.chars().any(|c| c.is_ascii_uppercase()) {
                continue;
            }
            if !out.contains(&q) {
                out.push(q);
            }
        }
        out.iter()
            .map(|s| JsString::from_units(s.to_utf16()))
            .collect()
    })
}

fn nm_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d: NamedNodeMapData = payload(agent, &a.this)?;
    let n = with_doc(agent, d.doc, |doc| doc.attributes_length(d.element));
    Ok(Value::Number(n as f64))
}

fn nm_item(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d: NamedNodeMapData = payload(agent, &a.this)?;
    let i = bf_js::host::webidl::to_unsigned_long(agent, &a.arg(0))? as usize;
    let n = with_doc_mut(agent, d.doc, |doc| doc.attributes_item(d.element, i));
    Ok(wrap_opt(agent, d.doc, n))
}

fn nm_get_named_item(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d: NamedNodeMapData = payload(agent, &a.this)?;
    let name = to_dom_string(agent, &a.arg(0))?;
    let n = with_doc_mut(agent, d.doc, |doc| doc.get_attribute_node(d.element, &name));
    Ok(wrap_opt(agent, d.doc, n))
}

fn nm_get_named_item_ns(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d: NamedNodeMapData = payload(agent, &a.this)?;
    let ns = crate::dom::to_opt_dom_string(agent, &a.arg(0))?;
    let local = to_dom_string(agent, &a.arg(1))?;
    let ns = ns.map(|s| s.to_string()).filter(|s| !s.is_empty());
    let n = with_doc_mut(agent, d.doc, |doc| {
        doc.get_attribute_node_ns(d.element, ns.as_deref(), &local)
    });
    Ok(wrap_opt(agent, d.doc, n))
}

fn nm_set_named_item(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d: NamedNodeMapData = payload(agent, &a.this)?;
    let attr = match unwrap_node(agent, &a.arg(0)) {
        Some(n) if agent.host_is_instance(&a.arg(0), classes(agent).attr) => n,
        _ => {
            return agent.type_error(
                "Failed to execute 'setNamedItem' on 'NamedNodeMap': parameter 1 is not of \
                 type 'Attr'.",
            );
        }
    };
    if attr.doc != d.doc {
        return throw_dom(agent, bf_dom::DomException::WrongDocumentError);
    }
    let r = with_doc_mut(agent, d.doc, |doc| {
        doc.set_attribute_node(d.element, attr.node)
    });
    match r {
        Ok(old) => {
            mutated(agent);
            Ok(wrap_opt(agent, d.doc, old))
        }
        Err(e) => throw_dom(agent, e),
    }
}

fn nm_remove_named_item(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d: NamedNodeMapData = payload(agent, &a.this)?;
    let name = to_dom_string(agent, &a.arg(0))?;
    let r = with_doc_mut(agent, d.doc, |doc| doc.remove_named_item(d.element, &name));
    match r {
        Ok(n) => {
            mutated(agent);
            Ok(wrap_node(agent, NodeRef::new(d.doc, n)))
        }
        Err(e) => throw_dom(agent, e),
    }
}

fn nm_remove_named_item_ns(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d: NamedNodeMapData = payload(agent, &a.this)?;
    let ns = crate::dom::to_opt_dom_string(agent, &a.arg(0))?;
    let local = to_dom_string(agent, &a.arg(1))?;
    let ns = ns.map(|s| s.to_string()).filter(|s| !s.is_empty());
    let node = with_doc_mut(agent, d.doc, |doc| {
        doc.get_attribute_node_ns(d.element, ns.as_deref(), &local)
    });
    let Some(node) = node else {
        return throw_dom(agent, bf_dom::DomException::NotFoundError);
    };
    let r = with_doc_mut(agent, d.doc, |doc| {
        doc.remove_attribute_node(d.element, node)
    });
    match r {
        Ok(n) => {
            mutated(agent);
            Ok(wrap_node(agent, NodeRef::new(d.doc, n)))
        }
        Err(e) => throw_dom(agent, e),
    }
}
