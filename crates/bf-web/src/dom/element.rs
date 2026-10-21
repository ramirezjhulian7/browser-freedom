//! `Element` (DOM §4.9) con los mixins `ParentNode`, `ChildNode` y
//! `NonDocumentTypeChildNode`.

use bf_js::host::{HostClass, HostClassId};
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use crate::dom::{
    from_dom_string, from_opt_dom_string, mutated, this_node, throw_dom, to_dom_string,
};
use crate::{with_doc, with_doc_mut};

pub fn element_class(node: HostClassId) -> HostClass {
    HostClass::new("Element")
        .parent(node)
        .attribute("namespaceURI", Some(namespace_uri), None)
        .attribute("prefix", Some(prefix), None)
        .attribute("localName", Some(local_name), None)
        .attribute("tagName", Some(tag_name), None)
        .attribute("id", Some(get_id), Some(set_id))
        .attribute("className", Some(get_class_name), Some(set_class_name))
        .method("hasAttributes", 0, has_attributes)
        .method("getAttribute", 1, get_attribute)
        .method("setAttribute", 2, set_attribute)
        .method("removeAttribute", 1, remove_attribute)
        .method("hasAttribute", 1, has_attribute)
}

fn namespace_uri(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = with_doc(agent, n.doc, |d| d.namespace_uri(n.node));
    Ok(from_opt_dom_string(s.as_ref()))
}

fn prefix(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = with_doc(agent, n.doc, |d| d.prefix(n.node));
    Ok(from_opt_dom_string(s.as_ref()))
}

fn local_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = with_doc(agent, n.doc, |d| d.local_name(n.node));
    Ok(from_dom_string(&s))
}

fn tag_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = with_doc(agent, n.doc, |d| d.tag_name(n.node));
    Ok(from_dom_string(&s))
}

fn get_id(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = with_doc(agent, n.doc, |d| d.id(n.node));
    Ok(from_dom_string(&s))
}

fn set_id(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = to_dom_string(agent, &a.arg(0))?;
    with_doc_mut(agent, n.doc, |d| d.set_id(n.node, v));
    mutated(agent);
    Ok(Value::Undefined)
}

fn get_class_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = with_doc(agent, n.doc, |d| d.class_name(n.node));
    Ok(from_dom_string(&s))
}

fn set_class_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = to_dom_string(agent, &a.arg(0))?;
    with_doc_mut(agent, n.doc, |d| d.set_class_name(n.node, v));
    mutated(agent);
    Ok(Value::Undefined)
}

fn has_attributes(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Boolean(with_doc(agent, n.doc, |d| {
        d.has_attributes(n.node)
    })))
}

fn get_attribute(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let name = to_dom_string(agent, &a.arg(0))?;
    let v = with_doc(agent, n.doc, |d| d.get_attribute(n.node, &name));
    Ok(from_opt_dom_string(v.as_ref()))
}

fn set_attribute(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let name = to_dom_string(agent, &a.arg(0))?;
    let value = to_dom_string(agent, &a.arg(1))?;
    let r = with_doc_mut(agent, n.doc, |d| d.set_attribute(n.node, &name, value));
    match r {
        Ok(()) => {
            mutated(agent);
            Ok(Value::Undefined)
        }
        Err(e) => throw_dom(agent, e),
    }
}

fn remove_attribute(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let name = to_dom_string(agent, &a.arg(0))?;
    with_doc_mut(agent, n.doc, |d| d.remove_attribute(n.node, &name));
    mutated(agent);
    Ok(Value::Undefined)
}

fn has_attribute(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let name = to_dom_string(agent, &a.arg(0))?;
    Ok(Value::Boolean(with_doc(agent, n.doc, |d| {
        d.has_attribute(n.node, &name)
    })))
}
