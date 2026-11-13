//! "Adopt" (DOM §4.5) entre documentos. `bf-dom` tiene una arena por documento y no mueve
//! nodos entre arenas: adoptar copia el subárbol en la arena destino, lo quita del origen y
//! reasigna a la copia la identidad de los wrappers, los listeners y los event handlers, así
//! JavaScript sigue viendo el mismo objeto.

use std::collections::HashMap;

use bf_dom::events::EventTarget;
use bf_dom::{DomException, NodeData, NodeId};
use bf_js::value::Value;
use bf_js::vm::{Agent, JsResult, RealmId};

use crate::dom::state::{ListenerRec, state};
use crate::dom::{mutated, throw_dom};
use crate::{DocIndex, NodeRef, WebContextHandle, with_doc, with_doc_mut};

/// Adopta `n` en el documento `to` y devuelve su nuevo `NodeRef` (el mismo si ya es de `to`,
/// y entonces no lo quita de su padre: ver `adoptNode`).
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
    // E1: en el mismo documento no hay nada que mover de arena. El paso 2 de "adopt" (quitarlo
    // de su padre) lo hace `adoptNode`; los demás usos (insertar, "convert nodes into a node")
    // lo quitan al insertarlo, y hacerlo aquí antes cambiaría el "viable sibling" de
    // `before()`/`after()`/`replaceWith()` cuando el propio nodo es un argumento (§4.2.8).
    if n.doc == to {
        return Ok(n);
    }
    // E1: la raíz viva de un espejo vuelve al documento del parser con su original.
    if to == 0
        && let Some(stale) = reclaim_by_script(agent, n)
    {
        mutated(agent);
        return Ok(NodeRef::new(0, stale));
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
    let mut new: Vec<NodeId> = with_doc(agent, to, |d| {
        d.shadow_including_inclusive_descendants(copy)
    }); // E1
    crate::dom::insertion::copy_script_state(agent, (from, &old), (to, &new));
    let mut old = old;
    {
        let wc = ctx.get_mut(agent);
        let mut target = std::mem::take(wc.doc_mut(to));
        let (ao, an) = attr_pairs(wc.doc(from), &mut target, &old, &new);
        *wc.doc_mut(to) = target;
        old.extend(ao);
        new.extend(an);
    }
    transfer(agent, (from, &old), (to, &new));
    retarget_mirror(agent, n, NodeRef::new(to, copy), &old, &new);
    // E1: si el nodo sale del documento que el parser está construyendo, el parser sigue con
    // el original: queda emparejado con la copia (ver [`mirror_sync_to_live`]).
    if from == 0 && parser_active(agent) {
        start_mirror(agent, n.node, NodeRef::new(to, copy), &old, &new);
    }
    mutated(agent);
    Ok(NodeRef::new(to, copy))
}

/// La identidad de cada nodo de `old` (en `from`) pasa a su par de `new` (en `to`): el
/// wrapper de JavaScript, los listeners, los event handlers y el estado de custom element (con
/// `adoptedCallback`).
fn transfer(
    agent: &mut Agent,
    (from, old): (DocIndex, &[NodeId]),
    (to, new): (DocIndex, &[NodeId]),
) {
    let ctx = WebContextHandle::current(agent);
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
    let mut new: Vec<NodeId> = with_doc(agent, to, |d| {
        d.shadow_including_inclusive_descendants(copy)
    });
    with_doc_mut(agent, to, |d| {
        for (nw, s) in new.iter().zip(&started) {
            if *s {
                d.script_state_mut(*nw).already_started = true;
            }
        }
    });
    let mut old = old;
    {
        let ctx = WebContextHandle { realm: here };
        let mut target = std::mem::take(ctx.get_mut(agent).doc_mut(to));
        let src = WebContextHandle { realm: from };
        let (ao, an) = attr_pairs(src.get(agent).doc(n.doc), &mut target, &old, &new);
        *ctx.get_mut(agent).doc_mut(to) = target;
        old.extend(ao);
        new.extend(an);
    }
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

/// E1: los `Attr` que JavaScript pudo haber visto de los elementos `old` (de `src`), con el
/// que les corresponde en su copia `new` (de `dst`): DOM §4.5 "adopt" paso 3.1 adopta los
/// `Attr` de cada elemento con él, así que su wrapper debe pasar al `Attr` de la copia.
fn attr_pairs(
    src: &bf_dom::Document,
    dst: &mut bf_dom::Document,
    old: &[NodeId],
    new: &[NodeId],
) -> (Vec<NodeId>, Vec<NodeId>) {
    let (mut ao, mut an) = (Vec::new(), Vec::new());
    for (&o, &nw) in old.iter().zip(new) {
        for a in src.existing_attr_nodes(o) {
            if let Some(b) = dst.matching_attr_node(nw, src, a) {
                ao.push(a);
                an.push(b);
            }
        }
    }
    (ao, an)
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

// --- E1: los nodos que el parser tiene abiertos y un script lleva a otro documento ---
//
// HTML §13.2.6.1 ("appropriate place for inserting a node") y el adoption agency
// (§13.2.6.4.7) trabajan sobre los nodos vivos de la pila de elementos abiertos: si un script
// lleva uno a otro documento (DOM §4.5, *adopt*), el parser sigue insertando donde esté ahora.
// `bf-dom` tiene una arena por documento y el tree builder (`bf-html`) solo escribe en la del
// suyo, así que adoptar copia el subárbol y el parser seguiría con el original suelto.
//
// En vez de una arena compartida, el original y la copia quedan **emparejados** nodo a nodo
// mientras dure el parseo, y en cada frontera de script se copia la estructura del lado que
// pudo cambiar al otro:
//
// - al devolver el parser al contexto (antes de correr JavaScript o al ceder el control,
//   [`mirror_sync_to_live`]): lo que hizo el parser en los originales pasa a los nodos vivos;
//   lo que insertó junto a la raíz (el foster parenting delante de una `table` que ahora está
//   en otro documento) se inserta junto al nodo vivo con la validez de *pre-insert*, y si no
//   cabe (un `Document` que ya tiene elemento) se descarta, como pide "insert an element at the
//   adjusted insertion location"; si el parser volvió a meter la raíz en su documento (el
//   adoption agency la inserta en el *common ancestor*), el nodo vivo vuelve con él;
// - al sacarlo para seguir parseando ([`mirror_sync_from_live`]): lo que hizo el script en los
//   nodos vivos pasa a los originales, y la raíz original cuelga de un fragmento suplente si
//   la viva tiene padre (así el parser ve "la tabla tiene padre" y hace foster parenting hacia
//   él), o queda suelta si no.
//
// Entre dos fronteras solo escribe un lado (el parser o el script), y JavaScript solo ve los
// nodos en las fronteras, ya sincronizados. Lo que no se copia: los atributos (el parser solo
// los añade a `html`/`body`), las raíces shadow y el contenido de `template` de los nodos
// emparejados, y los registros de `MutationObserver` de esas copias.

/// Un subárbol emparejado: la raíz original (en el documento del parser, el 0) y la viva.
#[derive(Debug)]
struct Mirror {
    stale: NodeId,
    live: NodeRef,
    /// Original → vivo y vivo → original.
    to_live: HashMap<NodeId, NodeId>,
    to_stale: HashMap<NodeId, NodeId>,
    /// El fragmento del documento del parser del que cuelga la raíz original mientras la viva
    /// tiene padre.
    standin: Option<NodeId>,
}

#[derive(Debug, Default)]
struct Mirrors(Vec<Mirror>);

/// ¿El documento 0 de la ventana actual tiene un parser que todavía construye el árbol?
fn parser_active(agent: &Agent) -> bool {
    let ctx = WebContextHandle::current(agent);
    matches!(
        &ctx.get(agent).documents[0],
        crate::DocumentOwner::Parser(p) if matches!(
            p.status(),
            bf_html::ParserStatus::Parsing | bf_html::ParserStatus::Blocked(_)
        )
    )
}

fn mirrors(agent: &mut Agent) -> Option<&mut Mirrors> {
    let realm = WebContextHandle::current(agent).realm;
    agent.realm_data_mut::<Mirrors>(realm)
}

fn start_mirror(agent: &mut Agent, stale: NodeId, live: NodeRef, old: &[NodeId], new: &[NodeId]) {
    let realm = WebContextHandle::current(agent).realm;
    if agent.realm_data::<Mirrors>(realm).is_none() {
        agent.set_realm_data(realm, Mirrors::default());
    }
    let mut m = Mirror {
        stale,
        live,
        to_live: HashMap::new(),
        to_stale: HashMap::new(),
        standin: None,
    };
    for (&o, &n) in old.iter().zip(new) {
        m.to_live.insert(o, n);
        m.to_stale.insert(n, o);
    }
    if let Some(ms) = mirrors(agent) {
        // Si la raíz ya estaba emparejada (el script la vuelve a llevar), manda la nueva.
        ms.0.retain(|x| x.stale != stale);
        ms.0.push(m);
    }
}

/// `n` (vivo) vuelve al documento del parser: es la raíz viva de un espejo, así que vuelve su
/// original (con lo que el script hizo en el vivo) en vez de una copia nueva. Devuelve el
/// original, suelto; `None` si `n` no es la raíz viva de ningún espejo.
fn reclaim_by_script(agent: &mut Agent, n: NodeRef) -> Option<NodeId> {
    let i = mirrors(agent)?.0.iter().position(|m| m.live == n)?;
    let mut m = mirrors(agent)?.0.remove(i);
    sync_tree(agent, &mut m, Direction::FromLive);
    with_doc_mut(agent, n.doc, |d| {
        if d.parent(n.node).is_some() {
            d.remove(n.node);
        }
    });
    with_doc_mut(agent, 0, |d| {
        if d.parent(m.stale).is_some() {
            d.remove(m.stale);
        }
    });
    give_back(agent, &m);
    Some(m.stale)
}

/// La identidad de los nodos vivos vuelve a sus originales.
fn give_back(agent: &mut Agent, m: &Mirror) {
    let (live, stale): (Vec<NodeId>, Vec<NodeId>) =
        m.to_stale.iter().map(|(&l, &s)| (l, s)).unzip();
    transfer(agent, (m.live.doc, &live), (0, &stale));
}

/// `n` (la raíz viva de un espejo) se fue a un tercer documento: el espejo sigue a la copia.
fn retarget_mirror(agent: &mut Agent, n: NodeRef, copy: NodeRef, old: &[NodeId], new: &[NodeId]) {
    let Some(ms) = mirrors(agent) else {
        return;
    };
    let Some(m) = ms.0.iter_mut().find(|m| m.live == n) else {
        return;
    };
    let moved: HashMap<NodeId, NodeId> = old.iter().copied().zip(new.iter().copied()).collect();
    m.live = copy;
    let pairs: Vec<(NodeId, NodeId)> = m
        .to_live
        .iter()
        .filter_map(|(&s, l)| moved.get(l).map(|&nl| (s, nl)))
        .collect();
    m.to_live = pairs.iter().copied().collect();
    m.to_stale = pairs.iter().map(|&(s, l)| (l, s)).collect();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// Del original (el parser escribió) al vivo.
    ToLive,
    /// Del vivo (escribió un script) al original.
    FromLive,
}

/// Antes de que corra JavaScript, o al ceder el control: lo que el parser hizo en los
/// originales pasa a los nodos vivos (ver la explicación de la sección).
pub fn mirror_sync_to_live(agent: &mut Agent, realm: RealmId) {
    if agent
        .realm_data::<Mirrors>(realm)
        .is_none_or(|m| m.0.is_empty())
    {
        return;
    }
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let active = parser_active(agent);
    let all = std::mem::take(&mut mirrors(agent).expect("espejos").0);
    let mut keep = Vec::new();
    for mut m in all {
        let parent = with_doc(agent, 0, |d| d.parent(m.stale));
        if parent.is_some() && parent != m.standin {
            // El parser la volvió a insertar en su documento (el adoption agency, en el
            // *common ancestor*): insertar el nodo vivo allí lo adopta de vuelta.
            with_doc_mut(agent, m.live.doc, |d| {
                if d.parent(m.live.node).is_some() {
                    d.remove(m.live.node);
                }
            });
            give_back(agent, &m);
            continue;
        }
        sync_tree(agent, &mut m, Direction::ToLive);
        place_fostered(agent, &mut m);
        if active {
            keep.push(m);
        }
    }
    if let Some(ms) = mirrors(agent) {
        ms.0 = keep;
    }
    mutated(agent);
    agent.current_realm = saved;
}

/// Al sacar el parser del contexto para seguir parseando: lo que un script hizo en los nodos
/// vivos pasa a los originales, y la raíz original cuelga del suplente si la viva tiene padre.
pub fn mirror_sync_from_live(agent: &mut Agent, realm: RealmId) {
    if agent
        .realm_data::<Mirrors>(realm)
        .is_none_or(|m| m.0.is_empty())
    {
        return;
    }
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let mut all = std::mem::take(&mut mirrors(agent).expect("espejos").0);
    for m in &mut all {
        sync_tree(agent, m, Direction::FromLive);
        let live_parent = with_doc(agent, m.live.doc, |d| d.parent(m.live.node));
        let stale = m.stale;
        if live_parent.is_some() {
            let standin = match m.standin {
                Some(f) => f,
                None => {
                    let f = with_doc_mut(agent, 0, |d| d.create_document_fragment());
                    m.standin = Some(f);
                    f
                }
            };
            with_doc_mut(agent, 0, |d| {
                if d.parent(stale) != Some(standin) {
                    d.append(standin, stale);
                }
            });
        } else {
            with_doc_mut(agent, 0, |d| {
                if d.parent(stale).is_some() {
                    d.remove(stale);
                }
            });
        }
    }
    if let Some(ms) = mirrors(agent) {
        ms.0 = all;
    }
    agent.current_realm = saved;
}

/// Copia sin hijos de `n` de `src` en `dst` (los elementos, con sus atributos; un `template`
/// recibe contents nuevos vacíos).
fn shallow_copy(src: &bf_dom::Document, dst: &mut bf_dom::Document, n: NodeId) -> NodeId {
    let mut data = src.get(n).data.clone();
    if let NodeData::Element(e) = &mut data {
        e.template_contents = None;
    }
    dst.create(data)
}

/// Iguala la estructura del subárbol de destino a la del de origen, reusando los pares y
/// creando (y emparejando) los nodos nuevos.
fn sync_tree(agent: &mut Agent, m: &mut Mirror, dir: Direction) {
    let (sd, dd) = match dir {
        Direction::ToLive => (0, m.live.doc),
        Direction::FromLive => (m.live.doc, 0),
    };
    let (sroot, droot) = match dir {
        Direction::ToLive => (m.stale, m.live.node),
        Direction::FromLive => (m.live.node, m.stale),
    };
    let ctx = WebContextHandle::current(agent);
    let mut created: Vec<(NodeId, NodeId)> = Vec::new();
    {
        let wc = ctx.get_mut(agent);
        let mut dst = std::mem::take(wc.doc_mut(dd));
        let src = wc.doc(sd);
        let (fwd, back) = match dir {
            Direction::ToLive => (&mut m.to_live, &mut m.to_stale),
            Direction::FromLive => (&mut m.to_stale, &mut m.to_live),
        };
        let mut stack = vec![(sroot, droot)];
        while let Some((s, d)) = stack.pop() {
            match (&src.get(s).data, &dst.get(d).data) {
                (NodeData::Text(a), NodeData::Text(b))
                | (NodeData::Comment(a), NodeData::Comment(b))
                    if a != b =>
                {
                    let a = a.clone();
                    dst.set_data(d, &a);
                    continue;
                }
                (NodeData::Element(_) | NodeData::DocumentFragment, _) => {}
                _ => continue,
            }
            let mut desired = Vec::new();
            for sc in src.children(s).collect::<Vec<_>>() {
                let dc = match fwd.get(&sc) {
                    Some(&dc) => dc,
                    None => {
                        let dc = shallow_copy(src, &mut dst, sc);
                        fwd.insert(sc, dc);
                        back.insert(dc, sc);
                        created.push((sc, dc));
                        dc
                    }
                };
                // Un par que acabaría conteniendo a su propio padre (no debería pasar con el
                // árbol del parser): se deja.
                if dst.is_inclusive_ancestor(dc, d) {
                    continue;
                }
                desired.push(dc);
                stack.push((sc, dc));
            }
            let current: Vec<NodeId> = dst.children(d).collect();
            if current == desired {
                continue;
            }
            // Los hijos de destino sin par (no deberían existir: el otro lado no escribió) van
            // al final.
            let extras: Vec<NodeId> = current
                .iter()
                .copied()
                .filter(|c| !back.contains_key(c))
                .collect();
            for c in &current {
                if back.contains_key(c) && !desired.contains(c) {
                    dst.remove(*c);
                }
            }
            for c in desired.iter().chain(extras.iter()) {
                dst.insert_before(d, *c, None);
            }
        }
        *ctx.get_mut(agent).doc_mut(dd) = dst;
    }
    if !created.is_empty() {
        let (s, d): (Vec<NodeId>, Vec<NodeId>) = created.into_iter().unzip();
        crate::dom::insertion::copy_script_state(agent, (sd, &s), (dd, &d));
    }
}

/// Lo que el parser insertó en el suplente junto a la raíz original (foster parenting hacia
/// el padre de una `table` que ahora vive en otro documento) va junto al nodo vivo, en su
/// orden: "insert an element at the adjusted insertion location" (HTML §13.2.6.1) inserta si
/// se puede y si no lo descarta (DOM §4.2.3, *ensure pre-insert validity*).
fn place_fostered(agent: &mut Agent, m: &mut Mirror) {
    let Some(standin) = m.standin else {
        return;
    };
    let siblings: Vec<NodeId> = with_doc(agent, 0, |d| d.children(standin).collect());
    let Some(at) = siblings.iter().position(|&c| c == m.stale) else {
        return;
    };
    let live = m.live;
    for (i, &sib) in siblings.iter().enumerate() {
        if sib == m.stale || m.to_live.contains_key(&sib) {
            continue;
        }
        let ctx = WebContextHandle::current(agent);
        let wc = ctx.get_mut(agent);
        let mut dst = std::mem::take(wc.doc_mut(live.doc));
        let src = wc.doc(0);
        let copy = shallow_copy(src, &mut dst, sib);
        let placed = dst.parent(live.node).and_then(|parent| {
            let reference = if i < at {
                Some(live.node)
            } else {
                dst.next_sibling(live.node)
            };
            dst.ensure_pre_insertion_validity(copy, parent, reference)
                .ok()
                .map(|()| dst.insert_before(parent, copy, reference))
        });
        *ctx.get_mut(agent).doc_mut(live.doc) = dst;
        if placed.is_some() {
            m.to_live.insert(sib, copy);
            m.to_stale.insert(copy, sib);
            // Su contenido, como el de la raíz.
            let mut sub = Mirror {
                stale: sib,
                live: NodeRef::new(live.doc, copy),
                to_live: std::mem::take(&mut m.to_live),
                to_stale: std::mem::take(&mut m.to_stale),
                standin: None,
            };
            sync_tree(agent, &mut sub, Direction::ToLive);
            m.to_live = sub.to_live;
            m.to_stale = sub.to_stale;
        } else {
            // No cabe: se descarta (queda suelto, como el nodo que el estándar no inserta).
            with_doc_mut(agent, 0, |d| d.remove(sib));
        }
    }
}
