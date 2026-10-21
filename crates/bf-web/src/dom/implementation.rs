//! `DOMImplementation` (DOM §4.5.1): `createDocumentType`, `createDocument`,
//! `createHTMLDocument` y `hasFeature`. Los documentos nuevos se agregan al
//! [`crate::WebContext`].

use bf_dom::{Document, Element, Namespace, NodeData};
use bf_js::host::HostClass;
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use crate::dom::{classes, throw_dom, to_dom_string, to_opt_dom_string, unwrap_node, wrap_node};
use crate::{DocIndex, NodeRef, WebContextHandle, with_doc_mut};

/// El payload de un `DOMImplementation`: su documento.
#[derive(Debug, Clone, Copy)]
pub struct ImplementationData {
    pub doc: DocIndex,
}

pub fn dom_implementation_class() -> HostClass {
    HostClass::new("DOMImplementation")
        .method("createDocumentType", 3, create_document_type)
        .method("createDocument", 2, create_document)
        .method("createHTMLDocument", 0, create_html_document)
        .method("hasFeature", 0, has_feature)
}

/// `document.implementation` (siempre el mismo objeto por documento).
pub fn implementation_of(agent: &mut Agent, doc: DocIndex) -> Value {
    let realm = agent.current_realm;
    let key = bf_js::host::HostKey((1 << 59) | u64::from(doc));
    if let Some(o) = agent.host_lookup(realm, key) {
        return Value::Object(o);
    }
    let class = classes(agent).dom_implementation;
    Value::Object(agent.host_wrap(realm, key, class, Box::new(ImplementationData { doc })))
}

fn this_doc(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<DocIndex> {
    match agent.host_unwrap::<ImplementationData>(&a.this) {
        Some(d) => Ok(d.doc),
        None => agent.type_error("Illegal invocation"),
    }
}

/// `createDocumentType(qualifiedName, publicId, systemId)`.
fn create_document_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let qn = to_dom_string(agent, &a.arg(0))?;
    let public_id = to_dom_string(agent, &a.arg(1))?;
    let system_id = to_dom_string(agent, &a.arg(2))?;
    let r = with_doc_mut(agent, doc, |d| {
        d.create_document_type(&qn, public_id, system_id)
    });
    match r {
        Ok(n) => Ok(wrap_node(agent, NodeRef::new(doc, n))),
        Err(e) => throw_dom(agent, e),
    }
}

/// `createDocument(namespace, qualifiedName, doctype)` (un `XMLDocument`).
fn create_document(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let parent = this_doc(agent, a)?;
    let ns = to_opt_dom_string(agent, &a.arg(0))?;
    let qn = if a.arg(1).is_null() {
        bf_infra::DomString::new()
    } else {
        to_dom_string(agent, &a.arg(1))?
    };
    let doctype = if a.arg(2).is_nullish() {
        None
    } else {
        match unwrap_node(agent, &a.arg(2)) {
            Some(n) if agent.host_is_instance(&a.arg(2), classes(agent).document_type) => Some(n),
            _ => {
                return agent.type_error(
                    "Failed to execute 'createDocument': parameter 3 is not of type \
                     'DocumentType'.",
                );
            }
        }
    };
    let mut d = Document::new_xml();
    let ns_str = ns.as_ref().map(|s| s.to_string());
    d.content_type = match ns_str.as_deref() {
        Some("http://www.w3.org/1999/xhtml") => "application/xhtml+xml".into(),
        Some("http://www.w3.org/2000/svg") => "image/svg+xml".into(),
        _ => "application/xml".into(),
    };
    let ctx = WebContextHandle::current(agent);
    d.url = ctx.get(agent).document().url.clone();
    let root = d.root();
    if let Some(dt) = doctype {
        // El doctype se importa (los nodos no se mueven entre arenas).
        let src = ctx.get(agent).doc(dt.doc).clone();
        let copy = d.import_from(&src, dt.node);
        d.append(root, copy);
    }
    if !qn.is_empty() {
        match d.create_element_ns(ns_str.as_deref().filter(|s| !s.is_empty()), &qn) {
            Ok(e) => d.append(root, e),
            Err(e) => return throw_dom(agent, e),
        }
    }
    let i = ctx.get_mut(agent).add_document(d);
    let _ = parent;
    let root = ctx.get(agent).doc(i).root();
    Ok(wrap_node(agent, NodeRef::new(i, root)))
}

/// `createHTMLDocument(title)` (DOM §4.5.1).
fn create_html_document(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_doc(agent, a)?;
    let title = if a.arg(0).is_undefined() {
        None
    } else {
        Some(to_dom_string(agent, &a.arg(0))?)
    };
    let mut d = Document::new();
    let root = d.root();
    let dt = d.create(NodeData::Doctype {
        name: "html".into(),
        public_id: Default::default(),
        system_id: Default::default(),
    });
    d.append(root, dt);
    let html = d.create(NodeData::Element(Element::new(Namespace::Html, "html")));
    d.append(root, html);
    let head = d.create(NodeData::Element(Element::new(Namespace::Html, "head")));
    d.append(html, head);
    if let Some(t) = title {
        let te = d.create(NodeData::Element(Element::new(Namespace::Html, "title")));
        d.append(head, te);
        let text = d.create_text_node(t);
        d.append(te, text);
    }
    let body = d.create(NodeData::Element(Element::new(Namespace::Html, "body")));
    d.append(html, body);
    let ctx = WebContextHandle::current(agent);
    d.url = ctx.get(agent).document().url.clone();
    let i = ctx.get_mut(agent).add_document(d);
    let root = ctx.get(agent).doc(i).root();
    Ok(wrap_node(agent, NodeRef::new(i, root)))
}

fn has_feature(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(true))
}
