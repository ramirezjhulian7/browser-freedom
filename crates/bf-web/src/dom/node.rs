//! `EventTarget` (DOM §2.7), `Node` (§4.4), `DocumentType` (§4.6), `CharacterData` (§4.10),
//! `ProcessingInstruction` (§4.13) y `Attr` (§4.9.2).

use bf_dom::{NodeData, document_position};
use bf_js::host::{HostClass, HostClassId};
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use crate::dom::{
    arg_node, arg_opt_node, from_dom_string, from_opt_dom_string, mutated, this_node, throw_dom,
    to_opt_dom_string, wrap_node, wrap_opt,
};
use crate::{NodeRef, WebContextHandle, with_doc, with_doc_mut};

pub fn event_target_class() -> HostClass {
    HostClass::new("EventTarget")
}

pub fn node_class(event_target: HostClassId) -> HostClass {
    let mut c = HostClass::new("Node")
        .parent(event_target)
        .attribute("nodeType", Some(node_type), None)
        .attribute("nodeName", Some(node_name), None)
        .attribute("baseURI", Some(base_uri), None)
        .attribute("isConnected", Some(is_connected), None)
        .attribute("ownerDocument", Some(owner_document), None)
        .attribute("parentNode", Some(parent_node), None)
        .attribute("parentElement", Some(parent_element), None)
        .attribute("firstChild", Some(first_child), None)
        .attribute("lastChild", Some(last_child), None)
        .attribute("previousSibling", Some(previous_sibling), None)
        .attribute("nextSibling", Some(next_sibling), None)
        .attribute("nodeValue", Some(get_node_value), Some(set_node_value))
        .attribute(
            "textContent",
            Some(get_text_content),
            Some(set_text_content),
        )
        .method("getRootNode", 0, get_root_node)
        .method("hasChildNodes", 0, has_child_nodes)
        .method("normalize", 0, normalize)
        .method("cloneNode", 0, clone_node)
        .method("isEqualNode", 1, is_equal_node)
        .method("isSameNode", 1, is_same_node)
        .method("compareDocumentPosition", 1, compare_document_position)
        .method("contains", 1, contains)
        .method("lookupPrefix", 1, lookup_prefix)
        .method("lookupNamespaceURI", 1, lookup_namespace_uri)
        .method("isDefaultNamespace", 1, is_default_namespace)
        .method("insertBefore", 2, insert_before)
        .method("appendChild", 1, append_child)
        .method("replaceChild", 2, replace_child)
        .method("removeChild", 1, remove_child);
    let constants: [(&str, f64); 18] = [
        ("ELEMENT_NODE", 1.0),
        ("ATTRIBUTE_NODE", 2.0),
        ("TEXT_NODE", 3.0),
        ("CDATA_SECTION_NODE", 4.0),
        ("ENTITY_REFERENCE_NODE", 5.0),
        ("ENTITY_NODE", 6.0),
        ("PROCESSING_INSTRUCTION_NODE", 7.0),
        ("COMMENT_NODE", 8.0),
        ("DOCUMENT_NODE", 9.0),
        ("DOCUMENT_TYPE_NODE", 10.0),
        ("DOCUMENT_FRAGMENT_NODE", 11.0),
        ("NOTATION_NODE", 12.0),
        (
            "DOCUMENT_POSITION_DISCONNECTED",
            document_position::DISCONNECTED as f64,
        ),
        (
            "DOCUMENT_POSITION_PRECEDING",
            document_position::PRECEDING as f64,
        ),
        (
            "DOCUMENT_POSITION_FOLLOWING",
            document_position::FOLLOWING as f64,
        ),
        (
            "DOCUMENT_POSITION_CONTAINS",
            document_position::CONTAINS as f64,
        ),
        (
            "DOCUMENT_POSITION_CONTAINED_BY",
            document_position::CONTAINED_BY as f64,
        ),
        (
            "DOCUMENT_POSITION_IMPLEMENTATION_SPECIFIC",
            document_position::IMPLEMENTATION_SPECIFIC as f64,
        ),
    ];
    for (name, v) in constants {
        c = c.constant(name, v);
    }
    c
}

pub fn document_type_class(node: HostClassId) -> HostClass {
    HostClass::new("DocumentType")
        .parent(node)
        .attribute("name", Some(doctype_name), None)
        .attribute("publicId", Some(doctype_public_id), None)
        .attribute("systemId", Some(doctype_system_id), None)
}

pub fn character_data_class(node: HostClassId) -> HostClass {
    HostClass::new("CharacterData")
        .parent(node)
        .attribute("data", Some(get_data), Some(set_data))
        .attribute("length", Some(data_length), None)
        .method("substringData", 2, substring_data)
        .method("appendData", 1, append_data)
        .method("insertData", 2, insert_data)
        .method("deleteData", 2, delete_data)
        .method("replaceData", 3, replace_data)
}

pub fn attr_class(node: HostClassId) -> HostClass {
    HostClass::new("Attr")
        .parent(node)
        .attribute("namespaceURI", Some(attr_namespace), None)
        .attribute("prefix", Some(attr_prefix), None)
        .attribute("localName", Some(attr_local_name), None)
        .attribute("name", Some(attr_name), None)
        .attribute("value", Some(attr_get_value), Some(attr_set_value))
        .attribute("ownerElement", Some(attr_owner_element), None)
        .attribute("specified", Some(attr_specified), None)
}

// --- Node ---

fn node_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let t = with_doc(agent, n.doc, |d| d.node_type(n.node) as u16);
    Ok(Value::Number(f64::from(t)))
}

fn node_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = with_doc(agent, n.doc, |d| d.node_name(n.node));
    Ok(from_dom_string(&s))
}

fn base_uri(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = with_doc(agent, n.doc, |d| d.base_uri(n.node));
    Ok(from_dom_string(&s))
}

fn is_connected(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Boolean(with_doc(agent, n.doc, |d| {
        d.is_connected(n.node)
    })))
}

fn owner_document(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let o = with_doc(agent, n.doc, |d| d.owner_document(n.node));
    Ok(wrap_opt(agent, n.doc, o))
}

fn relative(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    f: fn(&bf_dom::Document, bf_dom::NodeId) -> Option<bf_dom::NodeId>,
) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let r = with_doc(agent, n.doc, |d| f(d, n.node));
    Ok(wrap_opt(agent, n.doc, r))
}

fn parent_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    relative(agent, a, |d, n| match d.get(n).data {
        NodeData::Attr(_) => None,
        _ => d.parent(n),
    })
}

fn parent_element(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    relative(agent, a, |d, n| d.parent_element(n))
}

fn first_child(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    relative(agent, a, |d, n| d.first_child(n))
}

fn last_child(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    relative(agent, a, |d, n| d.last_child(n))
}

fn previous_sibling(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    relative(agent, a, |d, n| d.previous_sibling(n))
}

fn next_sibling(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    relative(agent, a, |d, n| d.next_sibling(n))
}

fn get_node_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = with_doc(agent, n.doc, |d| d.node_value(n.node));
    Ok(from_opt_dom_string(v.as_ref()))
}

/// `nodeValue` y `textContent` usan `[LegacyNullToEmptyString]`… en el spec son `DOMString?`
/// con `null` como cadena vacía.
fn set_node_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = to_opt_dom_string(agent, &a.arg(0))?;
    with_doc_mut(agent, n.doc, |d| d.set_node_value(n.node, v));
    mutated(agent);
    Ok(Value::Undefined)
}

fn get_text_content(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = with_doc(agent, n.doc, |d| d.get_text_content(n.node));
    Ok(from_opt_dom_string(v.as_ref()))
}

fn set_text_content(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = to_opt_dom_string(agent, &a.arg(0))?;
    with_doc_mut(agent, n.doc, |d| d.set_text_content(n.node, v));
    mutated(agent);
    Ok(Value::Undefined)
}

fn get_root_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let r = with_doc(agent, n.doc, |d| d.get_root_node(n.node));
    Ok(wrap_node(agent, NodeRef::new(n.doc, r)))
}

fn has_child_nodes(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Boolean(with_doc(agent, n.doc, |d| {
        d.has_child_nodes(n.node)
    })))
}

fn normalize(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    with_doc_mut(agent, n.doc, |d| d.normalize(n.node));
    mutated(agent);
    Ok(Value::Undefined)
}

fn clone_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let deep = bf_js::host::webidl::to_boolean(agent, &a.arg(0));
    let r = with_doc_mut(agent, n.doc, |d| d.clone_node_checked(n.node, deep));
    match r {
        Ok(c) => Ok(wrap_node(agent, NodeRef::new(n.doc, c))),
        Err(e) => throw_dom(agent, e),
    }
}

fn is_equal_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let Some(o) = arg_opt_node(agent, &a.arg(0), "parameter 1")? else {
        return Ok(Value::Boolean(false));
    };
    if o.doc != n.doc {
        return Ok(Value::Boolean(false));
    }
    Ok(Value::Boolean(with_doc(agent, n.doc, |d| {
        d.is_equal_node(n.node, o.node)
    })))
}

fn is_same_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let o = arg_opt_node(agent, &a.arg(0), "parameter 1")?;
    Ok(Value::Boolean(o == Some(n)))
}

fn compare_document_position(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let o = arg_node(agent, &a.arg(0), "parameter 1")?;
    let r = if o.doc == n.doc {
        with_doc(agent, n.doc, |d| {
            d.compare_document_position(n.node, o.node)
        })
    } else {
        // Nodos de documentos distintos: desconectados, con un orden arbitrario pero
        // consistente (§4.4 compareDocumentPosition paso 6).
        let order = if o.doc < n.doc {
            document_position::PRECEDING
        } else {
            document_position::FOLLOWING
        };
        document_position::DISCONNECTED | document_position::IMPLEMENTATION_SPECIFIC | order
    };
    Ok(Value::Number(f64::from(r)))
}

fn contains(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let o = arg_opt_node(agent, &a.arg(0), "parameter 1")?;
    let r = match o {
        Some(o) if o.doc == n.doc => with_doc(agent, n.doc, |d| d.contains(n.node, Some(o.node))),
        _ => false,
    };
    Ok(Value::Boolean(r))
}

fn lookup_prefix(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let ns = to_opt_dom_string(agent, &a.arg(0))?;
    let ns = ns.map(|s| s.to_string());
    let r = with_doc(agent, n.doc, |d| {
        d.lookup_prefix(n.node, ns.as_deref().filter(|s| !s.is_empty()))
    });
    Ok(from_opt_dom_string(r.as_ref()))
}

fn lookup_namespace_uri(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let p = to_opt_dom_string(agent, &a.arg(0))?;
    let p = p.map(|s| s.to_string());
    let r = with_doc(agent, n.doc, |d| {
        d.lookup_namespace_uri(n.node, p.as_deref().filter(|s| !s.is_empty()))
    });
    Ok(from_opt_dom_string(r.as_ref()))
}

fn is_default_namespace(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let ns = to_opt_dom_string(agent, &a.arg(0))?;
    let ns = ns.map(|s| s.to_string());
    let r = with_doc(agent, n.doc, |d| {
        d.is_default_namespace(n.node, ns.as_deref().filter(|s| !s.is_empty()))
    });
    Ok(Value::Boolean(r))
}

/// Verifica que dos nodos sean del mismo documento (los de otro documento se adoptan en
/// `bf-dom`, que no mueve nodos entre arenas: aquí se rechazan).
fn same_doc(agent: &mut Agent, a: NodeRef, b: NodeRef) -> JsResult<()> {
    if a.doc != b.doc {
        return throw_dom(agent, bf_dom::DomException::WrongDocumentError);
    }
    Ok(())
}

fn insert_before(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let parent = this_node(agent, a)?;
    let node = arg_node(agent, &a.arg(0), "parameter 1")?;
    let child = arg_opt_node(agent, &a.arg(1), "parameter 2")?;
    same_doc(agent, parent, node)?;
    if let Some(c) = child {
        same_doc(agent, parent, c)?;
    }
    let r = with_doc_mut(agent, parent.doc, |d| {
        d.insert_before_checked(parent.node, node.node, child.map(|c| c.node))
    });
    after_insert(agent, parent.doc, r)
}

fn append_child(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let parent = this_node(agent, a)?;
    let node = arg_node(agent, &a.arg(0), "parameter 1")?;
    same_doc(agent, parent, node)?;
    let r = with_doc_mut(agent, parent.doc, |d| {
        d.append_child(parent.node, node.node)
    });
    after_insert(agent, parent.doc, r)
}

fn replace_child(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let parent = this_node(agent, a)?;
    let node = arg_node(agent, &a.arg(0), "parameter 1")?;
    let child = arg_node(agent, &a.arg(1), "parameter 2")?;
    same_doc(agent, parent, node)?;
    same_doc(agent, parent, child)?;
    let r = with_doc_mut(agent, parent.doc, |d| {
        d.replace_child(parent.node, node.node, child.node)
    });
    after_insert(agent, parent.doc, r)
}

fn remove_child(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let parent = this_node(agent, a)?;
    let child = arg_node(agent, &a.arg(0), "parameter 1")?;
    if child.doc != parent.doc {
        return throw_dom(agent, bf_dom::DomException::NotFoundError);
    }
    let r = with_doc_mut(agent, parent.doc, |d| {
        d.remove_child(parent.node, child.node)
    });
    after_insert(agent, parent.doc, r)
}

/// El resultado de una mutación del árbol: el nodo o el `DOMException`.
pub(crate) fn after_insert(
    agent: &mut Agent,
    doc: crate::DocIndex,
    r: Result<bf_dom::NodeId, bf_dom::DomException>,
) -> JsResult<Value> {
    match r {
        Ok(n) => {
            mutated(agent);
            Ok(wrap_node(agent, NodeRef::new(doc, n)))
        }
        Err(e) => throw_dom(agent, e),
    }
}

// --- DocumentType ---

fn doctype_field(agent: &mut Agent, a: &CallArgs<'_>, i: usize) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = with_doc(agent, n.doc, |d| match &d.get(n.node).data {
        NodeData::Doctype {
            name,
            public_id,
            system_id,
        } => [name, public_id, system_id][i].clone(),
        _ => Default::default(),
    });
    Ok(from_dom_string(&s))
}

fn doctype_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    doctype_field(agent, a, 0)
}

fn doctype_public_id(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    doctype_field(agent, a, 1)
}

fn doctype_system_id(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    doctype_field(agent, a, 2)
}

// --- CharacterData ---

fn get_data(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = with_doc(agent, n.doc, |d| d.data(n.node));
    Ok(from_dom_string(&s))
}

fn set_data(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = if a.arg(0).is_null() {
        Default::default()
    } else {
        crate::dom::to_dom_string(agent, &a.arg(0))?
    };
    with_doc_mut(agent, n.doc, |d| d.set_data(n.node, &v));
    mutated(agent);
    Ok(Value::Undefined)
}

fn data_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let l = with_doc(agent, n.doc, |d| d.data_length(n.node));
    Ok(Value::Number(l as f64))
}

fn substring_data(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let offset = bf_js::host::webidl::to_unsigned_long(agent, &a.arg(0))? as usize;
    let count = bf_js::host::webidl::to_unsigned_long(agent, &a.arg(1))? as usize;
    let r = with_doc(agent, n.doc, |d| d.substring_data(n.node, offset, count));
    match r {
        Ok(s) => Ok(from_dom_string(&s)),
        Err(e) => throw_dom(agent, e),
    }
}

fn append_data(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = crate::dom::to_dom_string(agent, &a.arg(0))?;
    with_doc_mut(agent, n.doc, |d| d.append_data(n.node, &s));
    mutated(agent);
    Ok(Value::Undefined)
}

fn insert_data(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let offset = bf_js::host::webidl::to_unsigned_long(agent, &a.arg(0))? as usize;
    let s = crate::dom::to_dom_string(agent, &a.arg(1))?;
    let r = with_doc_mut(agent, n.doc, |d| d.insert_data(n.node, offset, &s));
    unit(agent, r)
}

fn delete_data(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let offset = bf_js::host::webidl::to_unsigned_long(agent, &a.arg(0))? as usize;
    let count = bf_js::host::webidl::to_unsigned_long(agent, &a.arg(1))? as usize;
    let r = with_doc_mut(agent, n.doc, |d| d.delete_data(n.node, offset, count));
    unit(agent, r)
}

fn replace_data(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let offset = bf_js::host::webidl::to_unsigned_long(agent, &a.arg(0))? as usize;
    let count = bf_js::host::webidl::to_unsigned_long(agent, &a.arg(1))? as usize;
    let s = crate::dom::to_dom_string(agent, &a.arg(2))?;
    let r = with_doc_mut(agent, n.doc, |d| d.replace_data(n.node, offset, count, &s));
    unit(agent, r)
}

/// Una mutación sin valor: `undefined` o el `DOMException`.
pub(crate) fn unit(agent: &mut Agent, r: Result<(), bf_dom::DomException>) -> JsResult<Value> {
    match r {
        Ok(()) => {
            mutated(agent);
            Ok(Value::Undefined)
        }
        Err(e) => throw_dom(agent, e),
    }
}

pub fn pi_target(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = with_doc(agent, n.doc, |d| match &d.get(n.node).data {
        NodeData::ProcessingInstruction { target, .. } => target.clone(),
        _ => Default::default(),
    });
    Ok(from_dom_string(&s))
}

// --- Attr ---

fn attr_data(agent: &Agent, n: NodeRef) -> Option<bf_dom::AttrData> {
    let ctx = WebContextHandle::current(agent).get(agent);
    match &ctx.doc(n.doc).get(n.node).data {
        NodeData::Attr(a) => Some(a.clone()),
        _ => None,
    }
}

fn attr_namespace(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let d = attr_data(agent, n);
    Ok(match d.and_then(|d| d.ns) {
        Some(ns) => Value::str(ns.uri()),
        None => Value::Null,
    })
}

fn attr_prefix(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let d = attr_data(agent, n);
    Ok(from_opt_dom_string(d.and_then(|d| d.prefix).as_ref()))
}

fn attr_local_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let d = attr_data(agent, n);
    Ok(d.map_or(Value::Null, |d| from_dom_string(&d.local_name)))
}

fn attr_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = with_doc(agent, n.doc, |d| d.node_name(n.node));
    Ok(from_dom_string(&s))
}

fn attr_get_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = with_doc(agent, n.doc, |d| d.attr_value(n.node));
    Ok(from_dom_string(&s))
}

fn attr_set_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = crate::dom::to_dom_string(agent, &a.arg(0))?;
    with_doc_mut(agent, n.doc, |d| d.set_attr_value(n.node, v));
    mutated(agent);
    Ok(Value::Undefined)
}

fn attr_owner_element(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let o = attr_data(agent, n).and_then(|d| d.owner);
    Ok(wrap_opt(agent, n.doc, o))
}

fn attr_specified(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(true))
}
