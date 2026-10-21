//! `Document` (DOM §4.5, HTML §3.1) y los constructores de `DocumentFragment`, `Text` y
//! `Comment`.

use bf_js::host::{HostClass, HostClassId};
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use crate::dom::{this_node, throw_dom, to_dom_string, wrap_node, wrap_opt};
use crate::{NodeRef, with_doc, with_doc_mut};

pub fn document_class(node: HostClassId) -> HostClass {
    HostClass::new("Document")
        .parent(node)
        .attribute("documentElement", Some(document_element), None)
        .attribute("head", Some(head), None)
        .attribute("body", Some(body), None)
        .method("createElement", 1, create_element)
        .method("createTextNode", 1, create_text_node)
        .method("createComment", 1, create_comment)
        .method("createDocumentFragment", 0, create_document_fragment)
        .method("getElementById", 1, get_element_by_id)
}

fn document_element(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let e = with_doc(agent, n.doc, |d| d.document_element());
    Ok(wrap_opt(agent, n.doc, e))
}

fn head(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let e = with_doc(agent, n.doc, |d| d.head());
    Ok(wrap_opt(agent, n.doc, e))
}

fn body(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let e = with_doc(agent, n.doc, |d| d.body());
    Ok(wrap_opt(agent, n.doc, e))
}

fn create_element(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let name = to_dom_string(agent, &a.arg(0))?;
    let r = with_doc_mut(agent, n.doc, |d| d.create_element(&name));
    match r {
        Ok(e) => Ok(wrap_node(agent, NodeRef::new(n.doc, e))),
        Err(e) => throw_dom(agent, e),
    }
}

fn create_text_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let data = to_dom_string(agent, &a.arg(0))?;
    let t = with_doc_mut(agent, n.doc, |d| d.create_text_node(data));
    Ok(wrap_node(agent, NodeRef::new(n.doc, t)))
}

fn create_comment(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let data = to_dom_string(agent, &a.arg(0))?;
    let t = with_doc_mut(agent, n.doc, |d| d.create_comment(data));
    Ok(wrap_node(agent, NodeRef::new(n.doc, t)))
}

fn create_document_fragment(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let f = with_doc_mut(agent, n.doc, |d| d.create_document_fragment());
    Ok(wrap_node(agent, NodeRef::new(n.doc, f)))
}

fn get_element_by_id(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let id = to_dom_string(agent, &a.arg(0))?;
    let e = with_doc(agent, n.doc, |d| d.get_element_by_id(&id));
    Ok(wrap_opt(agent, n.doc, e))
}

/// `new DocumentFragment()`: en el documento de la ventana del constructor.
pub fn document_fragment_ctor(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let f = with_doc_mut(agent, 0, |d| d.create_document_fragment());
    Ok(wrap_node(agent, NodeRef::new(0, f)))
}

/// `new Text(data = "")`.
pub fn text_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let data = if a.arg(0).is_undefined() {
        Default::default()
    } else {
        to_dom_string(agent, &a.arg(0))?
    };
    let t = with_doc_mut(agent, 0, |d| d.create_text_node(data));
    Ok(wrap_node(agent, NodeRef::new(0, t)))
}

/// `new Comment(data = "")`.
pub fn comment_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let data = if a.arg(0).is_undefined() {
        Default::default()
    } else {
        to_dom_string(agent, &a.arg(0))?
    };
    let t = with_doc_mut(agent, 0, |d| d.create_comment(data));
    Ok(wrap_node(agent, NodeRef::new(0, t)))
}
