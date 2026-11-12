//! "Adopt" (DOM §4.5) entre documentos. `bf-dom` tiene una arena por documento y no mueve
//! nodos entre arenas: adoptar copia el subárbol en la arena destino, lo quita del origen y
//! reasigna a la copia la identidad de los wrappers, los listeners y los event handlers, así
//! JavaScript sigue viendo el mismo objeto.

use bf_dom::events::EventTarget;
use bf_dom::{DomException, NodeData, NodeId};
use bf_js::value::Value;
use bf_js::vm::{Agent, JsResult, RealmId};

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
    // E1: un shadow root no se adopta.
    if with_doc(agent, n.doc, |d| d.is_shadow_root(n.node)) {
        return throw_dom(agent, DomException::HierarchyRequestError);
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
    let old: Vec<NodeId> = with_doc(agent, from, |d| {
        d.shadow_including_inclusive_descendants(n.node)
    }); // E1
    let ctx = WebContextHandle::current(agent);
    let copy = {
        let wc = ctx.get_mut(agent);
        let mut target = std::mem::take(wc.doc_mut(to));
        let copy = target.import_from(wc.doc(from), n.node);
        *wc.doc_mut(to) = target;
        copy
    };
    let new: Vec<NodeId> = with_doc(agent, to, |d| {
        d.shadow_including_inclusive_descendants(copy)
    }); // E1
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
        // E1: la definición y `adoptedCallback` de los custom elements.
        crate::html::custom_elements::adopted(agent, old_ref, new_ref);
    }
    mutated(agent);
    Ok(NodeRef::new(to, copy))
}

/// E1: un argumento `Node` que puede ser de otra ventana (para las operaciones que lo adoptan).
#[derive(Debug, Clone, Copy)]
pub enum NodeArg {
    /// Un nodo de este realm.
    Local(NodeRef),
    /// Un nodo del documento de otra ventana: su realm y su `NodeRef` allí.
    Foreign(RealmId, NodeRef),
}

/// E1: un argumento que debe ser un `Node`, de esta ventana o de otra.
pub fn arg_node_any(agent: &mut Agent, v: &Value, what: &str) -> JsResult<NodeArg> {
    if let Some(n) = crate::dom::unwrap_node(agent, v) {
        return Ok(NodeArg::Local(n));
    }
    if let Some((r, n)) = crate::dom::foreign_home(agent, v) {
        return Ok(NodeArg::Foreign(r, n));
    }
    // El error de siempre (TypeError, o NotSupportedError si no se encontró su ventana).
    crate::dom::arg_node(agent, v, what).map(NodeArg::Local)
}

impl From<NodeRef> for NodeArg {
    fn from(n: NodeRef) -> NodeArg {
        NodeArg::Local(n)
    }
}

/// E1: adopta `n` (de esta ventana o de otra) en el documento `to` de esta ventana.
pub fn adopt_arg(agent: &mut Agent, n: NodeArg, to: DocIndex) -> JsResult<NodeRef> {
    match n {
        NodeArg::Local(n) => adopt_into(agent, n, to),
        NodeArg::Foreign(realm, n) => adopt_foreign(agent, realm, n, to),
    }
}

/// Los datos del nodo de `n` (en su ventana).
fn node_data(agent: &mut Agent, n: NodeArg) -> NodeData {
    match n {
        NodeArg::Local(n) => with_doc(agent, n.doc, |d| d.get(n.node).data.clone()),
        NodeArg::Foreign(realm, n) => in_realm(agent, realm, |agent| {
            with_doc(agent, n.doc, |d| d.get(n.node).data.clone())
        }),
    }
}

/// Corre `f` con `realm` como realm actual (para leer el `WebContext` y el `DomState` de otra
/// ventana con las funciones de siempre).
fn in_realm<R>(agent: &mut Agent, realm: RealmId, f: impl FnOnce(&mut Agent) -> R) -> R {
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let r = f(agent);
    agent.current_realm = saved;
    r
}

/// E1: "adopt" (DOM §4.5) de un nodo del documento de otra ventana (`from`) en el documento
/// `to` de esta. Como [`adopt_into`], copia el subárbol en la arena destino; además el wrapper
/// pasa a la tabla de identidad de esta ventana con el prototipo de su clase aquí
/// (`Agent::host_transplant`), y los listeners de script, los event handlers y el estado de los
/// `<script>` pasan del `DomState` de la otra ventana al de esta (los ids de listener son por
/// ventana). JavaScript sigue viendo el mismo objeto, ahora atado a esta ventana.
pub fn adopt_foreign(
    agent: &mut Agent,
    from: RealmId,
    n: NodeRef,
    to: DocIndex,
) -> JsResult<NodeRef> {
    let here = agent.current_realm;
    let (is_document, is_shadow) = in_realm(agent, from, |agent| {
        with_doc(agent, n.doc, |d| {
            (
                matches!(d.get(n.node).data, NodeData::Document),
                d.is_shadow_root(n.node),
            )
        })
    });
    if is_document {
        return throw_dom(agent, DomException::NotSupportedError);
    }
    if is_shadow {
        return throw_dom(agent, DomException::HierarchyRequestError);
    }
    // Paso 2: quitarlo de su padre (o de su elemento, si es un `Attr`), en su ventana.
    let (old, started) = in_realm(agent, from, |agent| {
        with_doc_mut(agent, n.doc, |d| {
            if matches!(d.get(n.node).data, NodeData::Attr(_)) {
                if let Some(el) = d.owner_element(n.node) {
                    let _ = d.remove_attribute_node(el, n.node);
                }
            } else if d.parent(n.node).is_some() {
                d.remove(n.node);
            }
        });
        mutated(agent);
        with_doc(agent, n.doc, |d| {
            let old = d.shadow_including_inclusive_descendants(n.node);
            let started: Vec<bool> = old
                .iter()
                .map(|o| d.script_state(*o).already_started)
                .collect();
            (old, started)
        })
    });
    // La copia en la arena de este documento (`foreign_home` ya vio que ambas ventanas tienen
    // `WebContext`).
    let copy = {
        let ctx = WebContextHandle { realm: here };
        let mut target = std::mem::take(ctx.get_mut(agent).doc_mut(to));
        let src = WebContextHandle { realm: from };
        let copy = target.import_from(src.get(agent).doc(n.doc), n.node);
        *ctx.get_mut(agent).doc_mut(to) = target;
        copy
    };
    let new: Vec<NodeId> = with_doc(agent, to, |d| {
        d.shadow_including_inclusive_descendants(copy)
    });
    with_doc_mut(agent, to, |d| {
        for (nw, s) in new.iter().zip(&started) {
            if *s {
                d.script_state_mut(*nw).already_started = true;
            }
        }
    });
    for (&o, &nw) in old.iter().zip(new.iter()) {
        let (old_ref, new_ref) = (NodeRef::new(n.doc, o), NodeRef::new(to, nw));
        if let Some(obj) = agent.host_transplant((from, old_ref.key()), (here, new_ref.key()))
            && let Some(p) = agent.host_unwrap_mut::<NodeRef>(&Value::Object(obj))
        {
            *p = new_ref;
        }
        move_foreign_listeners(agent, from, old_ref, new_ref);
        in_realm(agent, from, |agent| {
            crate::html::custom_elements::forget(agent, old_ref);
        });
    }
    mutated(agent);
    Ok(NodeRef::new(to, copy))
}

/// E1: los listeners de script y los event handlers de `old` (en la ventana `from`) pasan a
/// `new` (en esta). Un callback toma el id de listener que tiene en esta ventana (o uno nuevo);
/// el listener de un event handler es por (documento, target, tipo) y se crea de nuevo aquí.
fn move_foreign_listeners(agent: &mut Agent, from: RealmId, old: NodeRef, new: NodeRef) {
    let (old_t, new_t) = (EventTarget::Node(old.node), EventTarget::Node(new.node));
    let ids = in_realm(agent, from, |agent| {
        with_doc(agent, old.doc, |d| d.script_listener_ids(old_t))
    });
    // El valor de los event handlers de `old`, quitado del estado de la otra ventana.
    let handlers: Vec<_> = in_realm(agent, from, |agent| {
        let st = state(agent);
        let keys: Vec<_> = st
            .handlers
            .keys()
            .filter(|(d, t, _)| *d == old.doc && *t == old_t)
            .cloned()
            .collect();
        keys.into_iter()
            .filter_map(|k| {
                st.handler_listeners.remove(&k);
                st.handlers.remove(&k).map(|h| (k.2, h))
            })
            .collect()
    });
    let mut remap = std::collections::HashMap::new();
    for id in ids {
        if remap.contains_key(&id) {
            continue;
        }
        let rec = in_realm(agent, from, |agent| {
            state(agent).listeners.get(&id).cloned()
        });
        let new_id = match rec {
            Some(ListenerRec::Callback(h)) => match agent.host_pinned(h) {
                Some(Value::Object(cb)) => crate::dom::events::listener_id(agent, cb),
                _ => continue,
            },
            Some(ListenerRec::Handler { type_, .. }) => {
                in_realm(agent, from, |agent| state(agent).listeners.remove(&id));
                let st = state(agent);
                st.handler_listeners.insert((new.doc, new_t, type_.clone()));
                st.next_listener += 1;
                let nid = st.next_listener;
                st.listeners.insert(
                    nid,
                    ListenerRec::Handler {
                        doc: new.doc,
                        target: new_t,
                        type_,
                    },
                );
                nid
            }
            None => continue,
        };
        remap.insert(id, new_id);
    }
    let st = state(agent);
    for (type_, h) in handlers {
        st.handlers.insert((new.doc, new_t, type_), h);
    }
    let (ctx, src) = (
        WebContextHandle::current(agent),
        WebContextHandle { realm: from },
    );
    let mut target = std::mem::take(ctx.get_mut(agent).doc_mut(new.doc));
    // Un listener sin traducción (su callback ya no está) no se mueve.
    src.get_mut(agent)
        .doc_mut(old.doc)
        .move_listeners_remapped(old_t, &mut target, new_t, |id| remap.get(&id).copied());
    *ctx.get_mut(agent).doc_mut(new.doc) = target;
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

/// Prepara `node` para insertarse en `parent`: si es de otro documento (E1: o de otra ventana),
/// primero los chequeos de "ensure pre-insertion validity" (§4.2.3) que no dependen de la arena
/// y luego la adopción; devuelve el nodo en el documento de `parent`.
pub fn prepare_insert(
    agent: &mut Agent,
    parent: NodeRef,
    node: impl Into<NodeArg>,
    child: Option<NodeRef>,
) -> JsResult<NodeRef> {
    let node = node.into();
    if let NodeArg::Local(n) = node
        && n.doc == parent.doc
    {
        return Ok(n);
    }
    let parent_kind = with_doc(agent, parent.doc, |d| d.get(parent.node).data.clone());
    let node_kind = node_data(agent, node);
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
    adopt_arg(agent, node, parent.doc)
}

/// E1: el documento de la ventana `realm` está por ser reemplazado sin cambiar de realm (un marco
/// que navega: HTML §7.4.2.3 reusa la `Window` del `about:blank` inicial, y aquí el realm se
/// reusa siempre). El documento viejo pasa a otro `DocIndex` y sus wrappers, event handlers,
/// caches, selección y foco van con él: si no, un `NodeRef` viejo `(0, id)` se resolvería en la
/// arena del documento nuevo (otro nodo, o un pánico de índice en `bf-dom` si es más chica), y un
/// `Range` viejo buscaría su rango vivo en el documento nuevo.
pub fn retire_window_document(agent: &mut Agent, realm: bf_js::vm::RealmId) {
    let Some(ctx) = agent.realm_data_mut::<crate::WebContext>(realm) else {
        return;
    };
    // El documento vacío de antes del primer parse no tiene nada que conservar.
    if ctx.documents[0].doc().node_count() <= 1 {
        return;
    }
    let old = std::mem::replace(
        &mut ctx.documents[0],
        crate::DocumentOwner::Document(Box::default()),
    );
    let ids: Vec<NodeId> = old.doc().node_ids().collect();
    ctx.documents.push(old);
    let to = (ctx.documents.len() - 1) as DocIndex;
    for id in ids {
        let (o, n) = (NodeRef::new(0, id), NodeRef::new(to, id));
        if let Some(obj) = agent.host_rekey(realm, o.key(), n.key())
            && let Some(p) = agent.host_unwrap_mut::<NodeRef>(&bf_js::value::Value::Object(obj))
        {
            *p = n;
        }
    }
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let st = state(agent);
    let keys: Vec<_> = st
        .handlers
        .keys()
        .filter(|(d, t, _)| *d == 0 && matches!(t, EventTarget::Node(_)))
        .cloned()
        .collect();
    for k in keys {
        if let Some(h) = st.handlers.remove(&k) {
            st.handlers.insert((to, k.1, k.2.clone()), h);
        }
        if st.handler_listeners.remove(&k) {
            st.handler_listeners.insert((to, k.1, k.2.clone()));
        }
    }
    for rec in st.listeners.values_mut() {
        if let ListenerRec::Handler { doc, target, .. } = rec
            && *doc == 0
            && matches!(target, EventTarget::Node(_))
        {
            *doc = to;
        }
    }
    let cached: Vec<_> = st
        .child_cache
        .keys()
        .filter(|(d, _)| *d == 0)
        .copied()
        .collect();
    for k in cached {
        if let Some(v) = st.child_cache.remove(&k) {
            st.child_cache.insert((to, k.1), v);
        }
    }
    if let Some(s) = st.selections.remove(&0) {
        st.selections.insert(to, s);
    }
    st.window_handler_source.clear();
    if let Some(f) = agent.realm_data_mut::<crate::html::misc::Focus>(realm)
        && let Some(n) = f.0
        && n.doc == 0
    {
        f.0 = Some(NodeRef::new(to, n.node));
    }
    crate::cssom::document_replaced(agent, realm, to);
    agent.current_realm = saved;
}
