//! "Adopt" (DOM §4.5) entre documentos. `bf-dom` tiene una arena por documento y no mueve
//! nodos entre arenas: adoptar copia el subárbol en la arena destino, lo quita del origen y
//! reasigna a la copia la identidad de los wrappers, los listeners y los event handlers, así
//! JavaScript sigue viendo el mismo objeto.

use bf_dom::events::EventTarget;
use bf_dom::{DomException, NodeData, NodeId};
use bf_js::vm::{Agent, JsResult};

use crate::dom::state::{ListenerRec, state};
use crate::dom::{mutated, throw_dom};
use crate::{DocIndex, NodeRef, WebContextHandle, with_doc, with_doc_mut};

/// Adopta `n` en el documento `to` y devuelve su nuevo `NodeRef` (el mismo si ya es de `to`).
/// Los `Document` no se pueden adoptar (`NotSupportedError`).
pub fn adopt_into(agent: &mut Agent, n: NodeRef, to: DocIndex) -> JsResult<NodeRef> {
    let is_document = with_doc(agent, n.doc, |d| {
        matches!(d.get(n.node).data, NodeData::Document)
    });
    if is_document {
        return throw_dom(agent, DomException::NotSupportedError);
    }
    if n.doc == to {
        with_doc_mut(agent, to, |d| d.adopt(n.node));
        return Ok(n);
    }
    let from = n.doc;
    // Paso 2 de "adopt": quitarlo de su padre (o de su elemento, si es un `Attr`).
    with_doc_mut(agent, from, |d| {
        if matches!(d.get(n.node).data, NodeData::Attr(_)) {
            if let Some(el) = d.owner_element(n.node) {
                let _ = d.remove_attribute_node(el, n.node);
            }
        } else if d.parent(n.node).is_some() {
            d.remove(n.node);
        }
    });
    let old: Vec<NodeId> = with_doc(agent, from, |d| d.inclusive_descendants(n.node));
    let ctx = WebContextHandle::current(agent);
    let copy = {
        let wc = ctx.get_mut(agent);
        let mut target = std::mem::take(wc.doc_mut(to));
        let copy = target.import_from(wc.doc(from), n.node);
        *wc.doc_mut(to) = target;
        copy
    };
    let new: Vec<NodeId> = with_doc(agent, to, |d| d.inclusive_descendants(copy));
    crate::dom::insertion::copy_script_state(agent, (from, &old), (to, &new));
    let realm = ctx.realm;
    for (&o, &nw) in old.iter().zip(new.iter()) {
        let (old_ref, new_ref) = (NodeRef::new(from, o), NodeRef::new(to, nw));
        if let Some(obj) = agent.host_rekey(realm, old_ref.key(), new_ref.key())
            && let Some(p) = agent.host_unwrap_mut::<NodeRef>(&bf_js::value::Value::Object(obj))
        {
            *p = new_ref;
        }
        {
            let wc = ctx.get_mut(agent);
            let mut target = std::mem::take(wc.doc_mut(to));
            wc.doc_mut(from).move_listeners(
                EventTarget::Node(o),
                &mut target,
                EventTarget::Node(nw),
            );
            *wc.doc_mut(to) = target;
        }
        move_handlers(agent, (from, o), (to, nw));
    }
    mutated(agent);
    Ok(NodeRef::new(to, copy))
}

/// Mueve los event handlers (y sus listeners) de un nodo a su copia.
fn move_handlers(agent: &mut Agent, (fd, fo): (DocIndex, NodeId), (td, tn): (DocIndex, NodeId)) {
    let st = state(agent);
    let from_t = EventTarget::Node(fo);
    let to_t = EventTarget::Node(tn);
    let keys: Vec<_> = st
        .handlers
        .keys()
        .filter(|(d, t, _)| *d == fd && *t == from_t)
        .cloned()
        .collect();
    for k in keys {
        if let Some(h) = st.handlers.remove(&k) {
            st.handlers.insert((td, to_t, k.2.clone()), h);
        }
        if st.handler_listeners.remove(&k) {
            st.handler_listeners.insert((td, to_t, k.2.clone()));
        }
    }
    for rec in st.listeners.values_mut() {
        if let ListenerRec::Handler { doc, target, .. } = rec
            && *doc == fd
            && *target == from_t
        {
            *doc = td;
            *target = to_t;
        }
    }
}

/// Prepara `node` para insertarse en `parent`: si es de otro documento, primero los chequeos
/// de "ensure pre-insertion validity" (§4.2.3) que no dependen de la arena y luego la
/// adopción; devuelve el nodo en el documento de `parent`.
pub fn prepare_insert(
    agent: &mut Agent,
    parent: NodeRef,
    node: NodeRef,
    child: Option<NodeRef>,
) -> JsResult<NodeRef> {
    if node.doc == parent.doc {
        return Ok(node);
    }
    let parent_kind = with_doc(agent, parent.doc, |d| d.get(parent.node).data.clone());
    let node_kind = with_doc(agent, node.doc, |d| d.get(node.node).data.clone());
    // Paso 1: el padre debe ser Document, DocumentFragment o Element.
    if !matches!(
        parent_kind,
        NodeData::Document | NodeData::DocumentFragment | NodeData::Element(_)
    ) {
        return throw_dom(agent, DomException::HierarchyRequestError);
    }
    // Paso 3: el hijo de referencia debe ser hijo del padre.
    if let Some(c) = child
        && (c.doc != parent.doc
            || with_doc(agent, c.doc, |d| d.parent(c.node)) != Some(parent.node))
    {
        return throw_dom(agent, DomException::NotFoundError);
    }
    // Pasos 4-5.
    match node_kind {
        NodeData::Document | NodeData::Attr(_) => {
            return throw_dom(agent, DomException::HierarchyRequestError);
        }
        NodeData::Text(_) | NodeData::CdataSection(_)
            if matches!(parent_kind, NodeData::Document) =>
        {
            return throw_dom(agent, DomException::HierarchyRequestError);
        }
        NodeData::Doctype { .. } if !matches!(parent_kind, NodeData::Document) => {
            return throw_dom(agent, DomException::HierarchyRequestError);
        }
        _ => {}
    }
    adopt_into(agent, node, parent.doc)
}
