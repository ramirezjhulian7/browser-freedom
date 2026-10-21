//! Los mixins del DOM (§4.2.4–§4.2.8): `NonElementParentNode`, `ParentNode` (con los
//! selectores de `bf-css`), `NonDocumentTypeChildNode` y `ChildNode`.

use bf_dom::NodeOrString;
use bf_dom::{DomException, NodeData, NodeId};
use bf_js::host::HostClass;
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use crate::dom::adopt::adopt_into;
use crate::dom::collections::{HtmlCollectionData, static_node_list};
use crate::dom::insertion::{inserted, roots_of};
use crate::dom::{mutated, this_node, throw_dom, to_dom_string, unwrap_node, wrap_opt};
use crate::{DocIndex, with_doc, with_doc_mut};

/// `getElementById` (NonElementParentNode, para `DocumentFragment`; `Document` usa el suyo).
pub fn non_element_parent_node(c: HostClass) -> HostClass {
    c.method("getElementById", 1, get_element_by_id)
}

/// ParentNode (§4.2.6).
pub fn parent_node(c: HostClass) -> HostClass {
    c.attribute("children", Some(children), None)
        .attribute("firstElementChild", Some(first_element_child), None)
        .attribute("lastElementChild", Some(last_element_child), None)
        .attribute("childElementCount", Some(child_element_count), None)
        .method("prepend", 0, prepend)
        .method("append", 0, append)
        .method("replaceChildren", 0, replace_children)
        .method("querySelector", 1, query_selector)
        .method("querySelectorAll", 1, query_selector_all)
}

/// NonDocumentTypeChildNode (§4.2.7).
pub fn non_document_type_child_node(c: HostClass) -> HostClass {
    c.attribute(
        "previousElementSibling",
        Some(previous_element_sibling),
        None,
    )
    .attribute("nextElementSibling", Some(next_element_sibling), None)
}

/// ChildNode (§4.2.8).
pub fn child_node(c: HostClass) -> HostClass {
    c.method("before", 0, before)
        .method("after", 0, after)
        .method("replaceWith", 0, replace_with)
        .method("remove", 0, remove)
}

/// Los miembros `[Unscopable]` de ParentNode y ChildNode (para `@@unscopables`).
pub const UNSCOPABLES: &[&str] = &[
    "prepend",
    "append",
    "replaceChildren",
    "before",
    "after",
    "replaceWith",
    "remove",
];

fn get_element_by_id(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let id = to_dom_string(agent, &a.arg(0))?;
    let e = with_doc(agent, n.doc, |d| d.element_by_id_in(n.node, &id));
    Ok(wrap_opt(agent, n.doc, e))
}

fn children(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let realm = agent.current_realm;
    // El mismo objeto siempre (clave propia en la tabla de wrappers).
    let key = bf_js::host::HostKey(n.key().0 | (1 << 58));
    if let Some(o) = agent.host_lookup(realm, key) {
        return Ok(Value::Object(o));
    }
    let coll = with_doc(agent, n.doc, |d| d.children_collection(n.node));
    let class = crate::dom::classes(agent).html_collection;
    let data = HtmlCollectionData { doc: n.doc, coll };
    Ok(Value::Object(agent.host_wrap(
        realm,
        key,
        class,
        Box::new(data),
    )))
}

fn first_element_child(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let r = with_doc(agent, n.doc, |d| d.first_element_child(n.node));
    Ok(wrap_opt(agent, n.doc, r))
}

fn last_element_child(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let r = with_doc(agent, n.doc, |d| d.last_element_child(n.node));
    Ok(wrap_opt(agent, n.doc, r))
}

fn child_element_count(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let r = with_doc(agent, n.doc, |d| d.child_element_count(n.node));
    Ok(Value::Number(r as f64))
}

fn previous_element_sibling(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let r = with_doc(agent, n.doc, |d| d.previous_element_sibling(n.node));
    Ok(wrap_opt(agent, n.doc, r))
}

fn next_element_sibling(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let r = with_doc(agent, n.doc, |d| d.next_element_sibling(n.node));
    Ok(wrap_opt(agent, n.doc, r))
}

/// `(Node or DOMString)...` en el documento `doc`: los nodos de otro documento se adoptan
/// (como al insertarlos en el fragmento de "convert nodes into a node", §4.2.6).
pub fn nodes_arg(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    doc: DocIndex,
) -> JsResult<Vec<NodeOrString>> {
    let mut out = Vec::with_capacity(a.args.len());
    for v in a.args {
        match unwrap_node(agent, v) {
            Some(n) => {
                let is_document = with_doc(agent, n.doc, |d| {
                    matches!(d.get(n.node).data, NodeData::Document)
                });
                if is_document {
                    return throw_dom(agent, DomException::HierarchyRequestError);
                }
                let n = adopt_into(agent, n, doc)?;
                out.push(NodeOrString::Node(n.node));
            }
            None => out.push(NodeOrString::String(to_dom_string(agent, v)?)),
        }
    }
    Ok(out)
}

/// Las raíces que se van a insertar (los hijos de los fragmentos).
fn arg_roots(agent: &Agent, doc: DocIndex, nodes: &[NodeOrString]) -> Vec<NodeId> {
    let mut roots = Vec::new();
    for n in nodes {
        if let NodeOrString::Node(id) = n {
            roots.extend(roots_of(agent, doc, *id));
        }
    }
    roots
}

fn mutate_with_nodes(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    f: fn(&mut bf_dom::Document, NodeId, Vec<NodeOrString>) -> Result<(), DomException>,
) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let nodes = nodes_arg(agent, a, n.doc)?;
    let roots = arg_roots(agent, n.doc, &nodes);
    let r = with_doc_mut(agent, n.doc, |d| f(d, n.node, nodes));
    match r {
        Ok(()) => {
            mutated(agent);
            inserted(agent, n.doc, &roots);
            Ok(Value::Undefined)
        }
        Err(e) => throw_dom(agent, e),
    }
}

fn prepend(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    mutate_with_nodes(agent, a, |d, n, nodes| d.prepend(n, nodes))
}

fn append(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    mutate_with_nodes(agent, a, |d, n, nodes| d.append_nodes(n, nodes))
}

fn replace_children(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    mutate_with_nodes(agent, a, |d, n, nodes| d.replace_children(n, nodes))
}

fn before(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    mutate_with_nodes(agent, a, |d, n, nodes| d.before(n, nodes))
}

fn after(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    mutate_with_nodes(agent, a, |d, n, nodes| d.after(n, nodes))
}

fn replace_with(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    mutate_with_nodes(agent, a, |d, n, nodes| d.replace_with(n, nodes))
}

fn remove(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    with_doc_mut(agent, n.doc, |d| d.remove_self(n.node));
    mutated(agent);
    Ok(Value::Undefined)
}

/// Los selectores como `&str` (los surrogates sueltos se reemplazan).
fn selectors(agent: &mut Agent, v: &Value) -> JsResult<String> {
    Ok(to_dom_string(agent, v)?.to_string())
}

fn query_selector(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = selectors(agent, &a.arg(0))?;
    let r = with_doc(agent, n.doc, |d| {
        bf_css::query::query_selector(d, n.node, &s)
    });
    match r {
        Ok(e) => Ok(wrap_opt(agent, n.doc, e)),
        Err(e) => throw_dom(agent, e),
    }
}

fn query_selector_all(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = selectors(agent, &a.arg(0))?;
    let r = with_doc(agent, n.doc, |d| {
        bf_css::query::query_selector_all(d, n.node, &s)
    });
    match r {
        Ok(list) => Ok(static_node_list(agent, n.doc, list)),
        Err(e) => throw_dom(agent, e),
    }
}

/// `matches` / `webkitMatchesSelector`.
pub fn matches(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = selectors(agent, &a.arg(0))?;
    let r = with_doc(agent, n.doc, |d| {
        bf_css::query::element_matches(d, n.node, &s)
    });
    match r {
        Ok(b) => Ok(Value::Boolean(b)),
        Err(e) => throw_dom(agent, e),
    }
}

/// `closest`.
pub fn closest(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = selectors(agent, &a.arg(0))?;
    let r = with_doc(agent, n.doc, |d| bf_css::query::closest(d, n.node, &s));
    match r {
        Ok(e) => Ok(wrap_opt(agent, n.doc, e)),
        Err(e) => throw_dom(agent, e),
    }
}

/// `@@unscopables` de los prototipos con miembros `[Unscopable]` (WebIDL §3.7.3): ParentNode
/// y ChildNode (y `slot` en `Element`).
pub fn finish_realm(agent: &mut Agent, realm: bf_js::vm::RealmId) {
    let c = crate::dom::classes(agent).clone();
    let parent = ["prepend", "append", "replaceChildren"];
    let child = ["before", "after", "replaceWith", "remove"];
    let mut element: Vec<&str> = parent.to_vec();
    element.extend(child);
    element.push("slot");
    let lists: [(bf_js::host::HostClassId, Vec<&str>); 5] = [
        (c.element, element),
        (c.document, parent.to_vec()),
        (c.document_fragment, parent.to_vec()),
        (c.character_data, child.to_vec()),
        (c.document_type, child.to_vec()),
    ];
    let saved = agent.current_realm;
    agent.current_realm = realm;
    for (class, names) in lists {
        let proto = agent.host_class_objects(realm, class).prototype;
        let u = agent.new_object(None);
        for n in names {
            agent.put_data(u, n.into(), Value::Boolean(true), true, true, true);
        }
        let key = bf_js::value::PropertyKey::Symbol(agent.well_known.unscopables);
        agent.put_data(proto, key, Value::Object(u), false, false, true);
    }
    agent.current_realm = saved;
}
