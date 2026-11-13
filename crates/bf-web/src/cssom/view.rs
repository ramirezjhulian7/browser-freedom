//! CSSOM View: `DOMRectReadOnly`/`DOMRect`/`DOMRectList` (Geometry Interfaces §4), la
//! geometría de `Element` (CSSOM View §6, §7), `elementFromPoint` (§5) y las propiedades
//! de `Window` (§4: `innerWidth`, `scrollX`, `scrollTo`…). Todo sale de la maqueta actual
//! ([`super::current_layout`]); un elemento sin caja da ceros.

use bf_dom::{Document, NodeId};
use std::collections::HashSet;
use std::sync::Arc;

use bf_js::host::{HostClass, HostClassId, TaskSource};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, RealmId};
use bf_layout::scroll::{self as layout_scroll, ScrollBox};
use bf_layout::{Fragment, FragmentKind, FragmentTree, Rect};
use bf_style::properties::types::{Display, Overflow, PointerEvents, Position, Visibility};

use super::{
    classes, current_layout, current_styles, set_indexed, state, state_of, window_metrics,
};
use crate::dom::{this_node, unwrap_node, wrap_node};
use crate::{NodeRef, WebContextHandle, with_doc};

// --- Fragmentos ---

/// Las cajas (no líneas ni texto) generadas por `node`, en orden de árbol.
pub fn element_fragments(tree: &FragmentTree, node: NodeId) -> Vec<&Fragment> {
    fn walk<'a>(f: &'a Fragment, node: NodeId, out: &mut Vec<&'a Fragment>) {
        if f.node == Some(node)
            && f.pseudo.is_none()
            && !matches!(f.kind, FragmentKind::Line | FragmentKind::Text)
        {
            out.push(f);
        }
        for c in &f.children {
            walk(c, node, out);
        }
    }
    let mut out = Vec::new();
    if let Some(root) = &tree.root {
        walk(root, node, &mut out);
    }
    out
}

fn union(rects: &[Rect]) -> Option<Rect> {
    let first = *rects.first()?;
    let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.right(), first.bottom());
    for r in &rects[1..] {
        x0 = x0.min(r.x);
        y0 = y0.min(r.y);
        x1 = x1.max(r.right());
        y1 = y1.max(r.bottom());
    }
    Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
}

/// El rectángulo del bloque contenedor de una caja posicionada (el padding box del
/// ancestro posicionado más cercano, o el viewport).
pub fn containing_block_rect(doc: &Document, tree: &FragmentTree, node: NodeId) -> Option<Rect> {
    let mut cur = doc.parent(node);
    while let Some(n) = cur {
        if doc.element(n).is_some()
            && let Some(f) = element_fragments(tree, n).first()
            && f.style.position != Position::Static
        {
            return Some(f.padding_rect());
        }
        cur = doc.parent(n);
    }
    Some(Rect::new(
        0.0,
        0.0,
        tree.viewport.width,
        tree.viewport.height,
    ))
}

/// El extremo del contenido de la página (para el área de desplazamiento del documento).
fn document_extent(tree: &FragmentTree) -> (f32, f32) {
    fn walk(f: &Fragment, w: &mut f32, h: &mut f32) {
        if !matches!(f.kind, FragmentKind::Line) {
            *w = w.max(f.rect.right());
            *h = h.max(f.rect.bottom());
        }
        if f.clip.is_none() {
            for c in &f.children {
                walk(c, w, h);
            }
        }
    }
    let (mut w, mut h) = (tree.viewport.width, tree.viewport.height);
    if let Some(r) = &tree.root {
        walk(r, &mut w, &mut h);
    }
    (w, h)
}

/// El extremo del contenido de una caja (para `scrollWidth`/`scrollHeight`).
fn content_extent(f: &Fragment) -> (f32, f32) {
    fn walk(f: &Fragment, w: &mut f32, h: &mut f32) {
        for c in &f.children {
            if !matches!(c.kind, FragmentKind::Line) {
                *w = w.max(c.rect.right());
                *h = h.max(c.rect.bottom());
            }
            walk(c, w, h);
        }
    }
    let p = f.padding_rect();
    let (mut w, mut h) = (p.right(), p.bottom());
    walk(f, &mut w, &mut h);
    (w - p.x, h - p.y)
}

// --- DOMRect ---

/// Los datos de un `DOMRectReadOnly`/`DOMRect`.
#[derive(Debug, Clone, Copy, Default)]
pub struct RectData {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// El payload de un `DOMRectList`.
#[derive(Debug, Clone)]
struct RectList(Vec<RectData>);

pub fn register_classes(agent: &mut Agent) -> (HostClassId, HostClassId, HostClassId) {
    let read_only = agent.register_host_class(
        HostClass::new("DOMRectReadOnly")
            .constructor(rect_read_only_ctor, 0)
            .static_method("fromRect", 0, rect_read_only_from_rect)
            .attribute("x", Some(rect_x), None)
            .attribute("y", Some(rect_y), None)
            .attribute("width", Some(rect_width), None)
            .attribute("height", Some(rect_height), None)
            .attribute("top", Some(rect_top), None)
            .attribute("right", Some(rect_right), None)
            .attribute("bottom", Some(rect_bottom), None)
            .attribute("left", Some(rect_left), None)
            .method("toJSON", 0, rect_to_json),
    );
    let rect = agent.register_host_class(
        HostClass::new("DOMRect")
            .parent(read_only)
            .constructor(rect_ctor, 0)
            .static_method("fromRect", 0, rect_from_rect)
            .attribute("x", Some(rect_x), Some(set_rect_x))
            .attribute("y", Some(rect_y), Some(set_rect_y))
            .attribute("width", Some(rect_width), Some(set_rect_width))
            .attribute("height", Some(rect_height), Some(set_rect_height)),
    );
    let list = agent.register_host_class(
        HostClass::new("DOMRectList")
            .attribute("length", Some(rect_list_length), None)
            .method("item", 1, rect_list_item),
    );
    (read_only, rect, list)
}

/// Un `DOMRect` nuevo.
pub fn new_rect(agent: &mut Agent, r: RectData) -> Value {
    let class = classes(agent).rect;
    Value::Object(agent.host_create(class, Box::new(r)))
}

fn rect_of(r: Rect, scroll: (f32, f32)) -> RectData {
    RectData {
        x: f64::from(r.x - scroll.0),
        y: f64::from(r.y - scroll.1),
        width: f64::from(r.width),
        height: f64::from(r.height),
    }
}

fn rect_args(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<RectData> {
    let mut v = [0.0; 4];
    for (i, slot) in v.iter_mut().enumerate() {
        let arg = a.arg(i);
        if !arg.is_undefined() {
            *slot = agent.to_number(&arg)?;
        }
    }
    Ok(RectData {
        x: v[0],
        y: v[1],
        width: v[2],
        height: v[3],
    })
}

fn rect_from_dict(agent: &mut Agent, v: &Value) -> JsResult<RectData> {
    let mut r = RectData::default();
    if let Value::Object(o) = v {
        for (name, slot) in [
            ("x", &mut r.x),
            ("y", &mut r.y),
            ("width", &mut r.width),
            ("height", &mut r.height),
        ] {
            let x = agent.get(*o, &PropertyKey::from(name), v.clone())?;
            if !x.is_undefined() {
                *slot = agent.to_number(&x)?;
            }
        }
    } else if !v.is_nullish() {
        return agent.type_error("DOMRectInit must be an object");
    }
    Ok(r)
}

fn construct(agent: &mut Agent, a: &CallArgs<'_>, read_only: bool) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Illegal constructor");
    };
    let r = rect_args(agent, a)?;
    let c = classes(agent);
    let class = if read_only { c.rect_read_only } else { c.rect };
    Ok(Value::Object(agent.host_create_from_new_target(
        nt,
        class,
        None,
        Box::new(r),
    )?))
}

fn rect_read_only_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    construct(agent, a, true)
}

fn rect_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    construct(agent, a, false)
}

fn rect_read_only_from_rect(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = rect_from_dict(agent, &a.arg(0))?;
    let class = classes(agent).rect_read_only;
    Ok(Value::Object(agent.host_create(class, Box::new(r))))
}

fn rect_from_rect(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = rect_from_dict(agent, &a.arg(0))?;
    Ok(new_rect(agent, r))
}

fn this_rect(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<RectData> {
    match agent.host_unwrap::<RectData>(&a.this) {
        Some(r) => Ok(*r),
        None => agent.type_error("Illegal invocation"),
    }
}

fn rect_x(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(this_rect(agent, a)?.x))
}

fn rect_y(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(this_rect(agent, a)?.y))
}

fn rect_width(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(this_rect(agent, a)?.width))
}

fn rect_height(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(this_rect(agent, a)?.height))
}

/// `min` y `max` que propagan NaN (Geometry §4.1: "min(x, x + width)").
fn nan_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.min(b)
    }
}

fn nan_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

fn rect_top(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rect(agent, a)?;
    Ok(Value::Number(nan_min(r.y, r.y + r.height)))
}

fn rect_right(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rect(agent, a)?;
    Ok(Value::Number(nan_max(r.x, r.x + r.width)))
}

fn rect_bottom(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rect(agent, a)?;
    Ok(Value::Number(nan_max(r.y, r.y + r.height)))
}

fn rect_left(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rect(agent, a)?;
    Ok(Value::Number(nan_min(r.x, r.x + r.width)))
}

fn rect_to_json(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rect(agent, a)?;
    let o = agent.new_plain_object();
    let fields = [
        ("x", r.x),
        ("y", r.y),
        ("width", r.width),
        ("height", r.height),
        ("top", nan_min(r.y, r.y + r.height)),
        ("right", nan_max(r.x, r.x + r.width)),
        ("bottom", nan_max(r.y, r.y + r.height)),
        ("left", nan_min(r.x, r.x + r.width)),
    ];
    for (k, v) in fields {
        agent.create_data_property_or_throw(o, PropertyKey::from(k), Value::Number(v))?;
    }
    Ok(Value::Object(o))
}

fn set_field(agent: &mut Agent, a: &CallArgs<'_>, f: fn(&mut RectData, f64)) -> JsResult<Value> {
    let v = agent.to_number(&a.arg(0))?;
    match agent.host_unwrap_mut::<RectData>(&a.this) {
        Some(r) => f(r, v),
        None => return agent.type_error("Illegal invocation"),
    }
    Ok(Value::Undefined)
}

fn set_rect_x(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    set_field(agent, a, |r, v| r.x = v)
}

fn set_rect_y(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    set_field(agent, a, |r, v| r.y = v)
}

fn set_rect_width(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    set_field(agent, a, |r, v| r.width = v)
}

fn set_rect_height(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    set_field(agent, a, |r, v| r.height = v)
}

fn new_rect_list(agent: &mut Agent, rects: Vec<RectData>) -> Value {
    let class = classes(agent).rect_list;
    let o = agent.host_create(class, Box::new(RectList(rects.clone())));
    let items: Vec<Value> = rects.into_iter().map(|r| new_rect(agent, r)).collect();
    set_indexed(agent, o, &items);
    Value::Object(o)
}

fn rect_list_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    match agent.host_unwrap::<RectList>(&a.this) {
        Some(l) => Ok(Value::Number(l.0.len() as f64)),
        None => agent.type_error("Illegal invocation"),
    }
}

fn rect_list_item(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let i = bf_js::host::webidl::to_unsigned_long(agent, &a.arg(0))? as usize;
    let Value::Object(o) = a.this.clone() else {
        return agent.type_error("Illegal invocation");
    };
    if agent.host_unwrap::<RectList>(&a.this).is_none() {
        return agent.type_error("Illegal invocation");
    }
    let k = PropertyKey::from(i as u32);
    match agent.heap[o].props.get(&k).map(|p| p.value.clone()) {
        Some(bf_js::object::PropValue::Data { value, .. }) => Ok(value),
        _ => Ok(Value::Null),
    }
}

// --- Element ---

/// `partial interface Element` (CSSOM View §6.1) y los `offset*` de `HTMLElement` (§7).
pub fn element_partial() -> HostClass {
    HostClass::new("Element")
        .method("getClientRects", 0, get_client_rects)
        .method("getBoundingClientRect", 0, get_bounding_client_rect)
        .method("checkVisibility", 0, check_visibility)
        .method("scrollIntoView", 0, scroll_into_view)
        .method("scroll", 0, element_scroll_to)
        .method("scrollTo", 0, element_scroll_to)
        .method("scrollBy", 0, element_scroll_by)
        .attribute("scrollTop", Some(scroll_top), Some(set_scroll_top))
        .attribute("scrollLeft", Some(scroll_left), Some(set_scroll_left))
        .attribute("scrollWidth", Some(scroll_width), None)
        .attribute("scrollHeight", Some(scroll_height), None)
        .attribute("clientTop", Some(client_top), None)
        .attribute("clientLeft", Some(client_left), None)
        .attribute("clientWidth", Some(client_width), None)
        .attribute("clientHeight", Some(client_height), None)
        .attribute("offsetParent", Some(offset_parent), None)
        .attribute("offsetTop", Some(offset_top), None)
        .attribute("offsetLeft", Some(offset_left), None)
        .attribute("offsetWidth", Some(offset_width), None)
        .attribute("offsetHeight", Some(offset_height), None)
}

/// Las cajas de un elemento del documento de la ventana (vacío si no tiene).
fn boxes(agent: &mut Agent, n: NodeRef) -> Vec<Fragment> {
    if n.doc != 0 || !with_doc(agent, 0, |d| d.is_connected(n.node)) {
        return Vec::new();
    }
    let tree = geometry_layout(agent);
    element_fragments(&tree, n.node)
        .into_iter()
        .map(|f| {
            let mut c = f.clone();
            c.children.clear();
            c
        })
        .collect()
}

/// La primera caja con sus hijos (para `scrollWidth`).
fn first_box_with_children(agent: &mut Agent, n: NodeRef) -> Option<Fragment> {
    if n.doc != 0 {
        return None;
    }
    let tree = geometry_layout(agent);
    element_fragments(&tree, n.node)
        .first()
        .map(|f| (*f).clone())
}

// --- Scroll de los elementos (E12) ---
//
// La posición de desplazamiento de cada scroll container vive en `CssomState::element_scroll`
// y se aplica a las maquetas (la del snapshot del shell y la calculada aquí) con
// `bf_layout::scroll`, de modo que la geometría (`getBoundingClientRect`…) la refleja. El
// shell se entera con [`take_element_scroll_changes`] y avisa los desplazamientos del
// usuario con [`user_scrolled_element`].

/// Los elementos con un evento `scroll` pendiente ("pending scroll event targets", CSSOM
/// View §13.1).
#[derive(Default)]
struct PendingScrollEvents(HashSet<NodeRef>);

/// Los desplazamientos hechos desde script que el shell todavía no aplicó a su página.
#[derive(Default)]
struct ElementScrollChanges(Vec<(NodeId, (f32, f32))>);

/// Aplica `f` a las maquetas en caché del realm en curso (el snapshot del shell y la propia).
fn update_layouts(agent: &mut Agent, f: impl Fn(&mut FragmentTree)) {
    if let Some((_, _, t)) = &mut state(agent).layout {
        f(Arc::make_mut(t));
    }
    let h = WebContextHandle::current(agent);
    if let Some(snap) = h.get_mut(agent).layout.as_mut() {
        f(Arc::make_mut(&mut snap.fragments));
    }
}

/// La maqueta para la geometría: la actual, con las cajas `sticky` del viewport al día con
/// el scroll de la página.
fn geometry_layout(agent: &mut Agent) -> Arc<FragmentTree> {
    let tree = current_layout(agent);
    let scroll = window_metrics(agent).scroll;
    if tree.scroll.viewport_sticky && tree.scroll.page_scroll != scroll {
        drop(tree);
        update_layouts(agent, |t| {
            layout_scroll::page_scrolled(t, scroll);
        });
        return current_layout(agent);
    }
    tree
}

/// Aplica a una maqueta recién hecha los desplazamientos guardados (limitados a la nueva
/// área desplazable) y los guarda ya limitados.
pub fn restore_element_scroll(agent: &mut Agent, tree: &mut FragmentTree) {
    let offsets: Vec<(NodeId, (f32, f32))> = state(agent)
        .element_scroll
        .iter()
        .filter(|(n, _)| n.doc == 0)
        .map(|(n, o)| (n.node, *o))
        .collect();
    let applied = layout_scroll::apply_offsets(tree, offsets);
    let st = state(agent);
    for (node, o) in applied {
        st.element_scroll.insert(NodeRef::new(0, node), o);
    }
    let scroll = window_metrics(agent).scroll;
    layout_scroll::page_scrolled(tree, scroll);
}

/// El scroll container de `n` en la maqueta actual, si lo es.
fn scroll_box(agent: &mut Agent, n: NodeRef) -> Option<ScrollBox> {
    if n.doc != 0 || !with_doc(agent, 0, |d| d.is_connected(n.node)) {
        return None;
    }
    current_layout(agent).scroll.boxes.get(&n.node).copied()
}

/// Cambia la posición de desplazamiento de `n` (ya limitada) en el estado y las maquetas, y
/// encola el evento `scroll` (CSSOM View §13.1: no burbujea en los elementos).
fn set_element_scroll(agent: &mut Agent, n: NodeRef, offset: (f32, f32), from_script: bool) {
    state(agent).element_scroll.insert(n, offset);
    update_layouts(agent, |t| {
        layout_scroll::set_scroll_offset(t, n.node, offset);
    });
    let realm = WebContextHandle::current(agent).realm;
    if from_script {
        if agent.realm_data::<ElementScrollChanges>(realm).is_none() {
            agent.set_realm_data(realm, ElementScrollChanges::default());
        }
        if let Some(c) = agent.realm_data_mut::<ElementScrollChanges>(realm) {
            c.0.retain(|(node, _)| *node != n.node);
            c.0.push((n.node, offset));
        }
    }
    if agent.realm_data::<PendingScrollEvents>(realm).is_none() {
        agent.set_realm_data(realm, PendingScrollEvents::default());
    }
    let fresh = agent
        .realm_data_mut::<PendingScrollEvents>(realm)
        .is_some_and(|p| p.0.insert(n));
    if fresh {
        agent.queue_task(
            TaskSource::UserInteraction,
            Box::new(move |agent: &mut Agent| fire_element_scroll(agent, realm, n)),
        );
    }
}

fn fire_element_scroll(agent: &mut Agent, realm: RealmId, n: NodeRef) {
    let pending = agent
        .realm_data_mut::<PendingScrollEvents>(realm)
        .is_some_and(|p| p.0.remove(&n));
    if !pending {
        return;
    }
    let saved = agent.current_realm;
    agent.current_realm = realm;
    if with_doc(agent, n.doc, |d| d.is_connected(n.node)) {
        crate::dom::events::fire_event(
            agent,
            n.doc,
            bf_dom::events::EventTarget::Node(n.node),
            "scroll",
            bf_dom::events::EventInit::default(),
            bf_dom::events::EventKind::Event,
        );
    }
    agent.current_realm = saved;
}

/// Los desplazamientos de elementos hechos desde script desde la última llamada (para que el
/// shell los aplique a su página).
pub fn take_element_scroll_changes(agent: &mut Agent, realm: RealmId) -> Vec<(NodeId, (f32, f32))> {
    agent
        .realm_data_mut::<ElementScrollChanges>(realm)
        .map(|c| std::mem::take(&mut c.0))
        .unwrap_or_default()
}

/// Las posiciones de desplazamiento de los elementos del documento de la ventana.
pub fn element_scroll_offsets(agent: &mut Agent, realm: RealmId) -> Vec<(NodeId, (f32, f32))> {
    state_of(agent, realm)
        .element_scroll
        .iter()
        .filter(|(n, o)| n.doc == 0 && **o != (0.0, 0.0))
        .map(|(n, o)| (n.node, *o))
        .collect()
}

/// El usuario desplazó el elemento `node` (la rueda): el shell ya lo aplicó a su página; aquí
/// se guarda y se encola el evento `scroll`.
pub fn user_scrolled_element(agent: &mut Agent, realm: RealmId, node: NodeId, offset: (f32, f32)) {
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let n = NodeRef::new(0, node);
    let cur = element_scroll_pos(agent, n);
    if cur != offset {
        set_element_scroll(agent, n, offset, false);
    }
    agent.current_realm = saved;
}

fn scroll_offset(agent: &mut Agent) -> (f32, f32) {
    window_metrics(agent).scroll
}

fn get_client_rects(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let scroll = scroll_offset(agent);
    let rects: Vec<RectData> = boxes(agent, n)
        .iter()
        .map(|f| rect_of(f.rect, fixed_scroll(f, scroll)))
        .collect();
    Ok(new_rect_list(agent, rects))
}

/// Las cajas fijas no se desplazan con la página.
fn fixed_scroll(f: &Fragment, scroll: (f32, f32)) -> (f32, f32) {
    if f.style.position == Position::Fixed {
        (0.0, 0.0)
    } else {
        scroll
    }
}

/// E2: el rectángulo de `getBoundingClientRect()` de `n` (x, y, ancho, alto respecto del
/// viewport), o `None` si no tiene caja (no se pinta).
pub(crate) fn client_rect(agent: &mut Agent, n: NodeRef) -> Option<(f32, f32, f32, f32)> {
    let scroll = scroll_offset(agent);
    let bs = boxes(agent, n);
    let rects: Vec<Rect> = bs.iter().map(|f| f.rect).collect();
    let non_empty: Vec<Rect> = rects
        .iter()
        .filter(|r| r.width > 0.0 || r.height > 0.0)
        .copied()
        .collect();
    let r = if non_empty.is_empty() {
        rects.first().copied()
    } else {
        union(&non_empty)
    }?;
    let s = bs.first().map_or(scroll, |f| fixed_scroll(f, scroll));
    Some((r.x - s.0, r.y - s.1, r.width, r.height))
}

/// `getBoundingClientRect()` (CSSOM View §6.1): la unión de los rectángulos no vacíos, o
/// el primero si todos lo son.
fn get_bounding_client_rect(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let scroll = scroll_offset(agent);
    let bs = boxes(agent, n);
    let rects: Vec<Rect> = bs.iter().map(|f| f.rect).collect();
    let non_empty: Vec<Rect> = rects
        .iter()
        .filter(|r| r.width > 0.0 || r.height > 0.0)
        .copied()
        .collect();
    let r = if non_empty.is_empty() {
        rects.first().copied()
    } else {
        union(&non_empty)
    };
    let s = bs.first().map_or(scroll, |f| fixed_scroll(f, scroll));
    let data = r.map_or_else(RectData::default, |r| rect_of(r, s));
    Ok(new_rect(agent, data))
}

/// `checkVisibility(options)` (CSSOM View §6.1).
fn check_visibility(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let opts = a.arg(0);
    let mut check_opacity = false;
    let mut check_visibility = false;
    if let Value::Object(o) = &opts {
        for (k, opacity) in [
            ("checkOpacity", true),
            ("opacityProperty", true),
            ("checkVisibilityCSS", false),
            ("visibilityProperty", false),
        ] {
            let v = agent.get(*o, &PropertyKey::from(k), opts.clone())?;
            let on = bf_js::host::webidl::to_boolean(agent, &v);
            if opacity {
                check_opacity |= on;
            } else {
                check_visibility |= on;
            }
        }
    }
    if n.doc != 0 || !with_doc(agent, 0, |d| d.is_connected(n.node)) {
        return Ok(Value::Boolean(false));
    }
    let styles = current_styles(agent);
    // Sin caja (display: none en él o en un ancestro, o display: contents) → false.
    let chain: Vec<NodeId> = with_doc(agent, 0, |d| {
        d.inclusive_ancestors(n.node)
            .into_iter()
            .filter(|x| d.element(*x).is_some())
            .collect()
    });
    for (i, x) in chain.iter().enumerate() {
        let Some(s) = styles.get(&(*x, None)) else {
            return Ok(Value::Boolean(false));
        };
        if s.display == Display::None || (i == 0 && s.display == Display::Contents) {
            return Ok(Value::Boolean(false));
        }
        if check_opacity && s.opacity.0 == 0.0 {
            return Ok(Value::Boolean(false));
        }
    }
    if check_visibility
        && let Some(s) = styles.get(&(n.node, None))
        && s.visibility != Visibility::Visible
    {
        return Ok(Value::Boolean(false));
    }
    Ok(Value::Boolean(true))
}

/// ¿Es el elemento el que desplaza el viewport (`html`, o `body` en modo quirks)?
fn is_scrolling_element(agent: &mut Agent, n: NodeRef) -> bool {
    with_doc(agent, n.doc, |d| {
        let Some(el) = d.element(n.node) else {
            return false;
        };
        let is_root = d.parent(n.node) == Some(d.root());
        if d.mode == bf_dom::QuirksMode::Quirks {
            el.is_html("body")
        } else {
            is_root && el.is_html("html")
        }
    })
}

fn is_inline_box(f: &Fragment) -> bool {
    f.kind == FragmentKind::Inline
}

fn client_top(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let b = boxes(agent, n);
    Ok(Value::Number(match b.first() {
        Some(f) if !is_inline_box(f) => f64::from(f.border.top),
        _ => 0.0,
    }))
}

fn client_left(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let b = boxes(agent, n);
    Ok(Value::Number(match b.first() {
        Some(f) if !is_inline_box(f) => f64::from(f.border.left),
        _ => 0.0,
    }))
}

fn client_size(agent: &mut Agent, a: &CallArgs<'_>, width: bool) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    if is_scrolling_element(agent, n) {
        let v = window_metrics(agent).viewport;
        return Ok(Value::Number(
            f64::from(if width { v.width } else { v.height }).floor(),
        ));
    }
    let b = boxes(agent, n);
    Ok(Value::Number(match b.first() {
        Some(f) if !is_inline_box(f) => {
            let p = f.padding_rect();
            f64::from(if width { p.width } else { p.height }).round()
        }
        _ => 0.0,
    }))
}

fn client_width(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    client_size(agent, a, true)
}

fn client_height(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    client_size(agent, a, false)
}

/// El área de desplazamiento (`scrollWidth`/`scrollHeight`, CSSOM View §6.1).
fn scroll_size(agent: &mut Agent, n: NodeRef) -> (f32, f32) {
    if is_scrolling_element(agent, n) {
        let tree = current_layout(agent);
        return document_extent(&tree);
    }
    if let Some(sb) = scroll_box(agent, n) {
        return (sb.scroll_size.width, sb.scroll_size.height);
    }
    match first_box_with_children(agent, n) {
        Some(f) if !is_inline_box(&f) => {
            if f.style.overflow_x == Overflow::Visible && f.style.overflow_y == Overflow::Visible {
                let p = f.padding_rect();
                let (w, h) = content_extent(&f);
                (w.max(p.width), h.max(p.height))
            } else {
                content_extent(&f)
            }
        }
        _ => (0.0, 0.0),
    }
}

fn scroll_width(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Number(f64::from(scroll_size(agent, n).0).round()))
}

fn scroll_height(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Number(f64::from(scroll_size(agent, n).1).round()))
}

fn element_scroll_pos(agent: &mut Agent, n: NodeRef) -> (f32, f32) {
    if is_scrolling_element(agent, n) {
        return scroll_offset(agent);
    }
    scroll_box(agent, n).map_or((0.0, 0.0), |sb| sb.offset)
}

/// Desplaza un elemento a `(x, y)` (sujeto a los límites de su área de desplazamiento).
fn scroll_element_to(agent: &mut Agent, n: NodeRef, x: Option<f64>, y: Option<f64>) {
    if is_scrolling_element(agent, n) {
        window_scroll_to(agent, x, y);
        return;
    }
    // CSSOM View §6.1 (`scroll()` sobre un elemento): sin caja de scroll container, nada.
    let Some(sb) = scroll_box(agent, n) else {
        return;
    };
    let pick =
        |v: Option<f64>, cur: f32| v.map_or(cur, |v| if v.is_finite() { v as f32 } else { 0.0 });
    let new = sb.clamp((pick(x, sb.offset.0), pick(y, sb.offset.1)));
    if new != sb.offset {
        set_element_scroll(agent, n, new, true);
    }
}

fn scroll_top(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Number(f64::from(element_scroll_pos(agent, n).1)))
}

fn scroll_left(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Number(f64::from(element_scroll_pos(agent, n).0)))
}

fn set_scroll_top(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = agent.to_number(&a.arg(0))?;
    scroll_element_to(agent, n, None, Some(v));
    Ok(Value::Undefined)
}

fn set_scroll_left(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = agent.to_number(&a.arg(0))?;
    scroll_element_to(agent, n, Some(v), None);
    Ok(Value::Undefined)
}

/// Los argumentos de `scroll()`/`scrollTo()`/`scrollBy()`: `(x, y)` o `ScrollToOptions`.
fn scroll_args(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<(Option<f64>, Option<f64>)> {
    let first = a.arg(0);
    if a.args.len() >= 2 {
        let x = agent.to_number(&first)?;
        let y = agent.to_number(&a.arg(1))?;
        return Ok((Some(x), Some(y)));
    }
    match &first {
        Value::Object(o) => {
            let left = agent.get(*o, &PropertyKey::from("left"), first.clone())?;
            let top = agent.get(*o, &PropertyKey::from("top"), first.clone())?;
            let left = if left.is_undefined() {
                None
            } else {
                Some(agent.to_number(&left)?)
            };
            let top = if top.is_undefined() {
                None
            } else {
                Some(agent.to_number(&top)?)
            };
            Ok((left, top))
        }
        Value::Undefined | Value::Null => Ok((None, None)),
        _ => agent.type_error("ScrollToOptions must be an object"),
    }
}

fn element_scroll_to(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let (x, y) = scroll_args(agent, a)?;
    scroll_element_to(agent, n, x, y);
    Ok(Value::Undefined)
}

fn element_scroll_by(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let (dx, dy) = scroll_args(agent, a)?;
    let cur = element_scroll_pos(agent, n);
    scroll_element_to(
        agent,
        n,
        Some(f64::from(cur.0) + dx.unwrap_or(0.0)),
        Some(f64::from(cur.1) + dy.unwrap_or(0.0)),
    );
    Ok(Value::Undefined)
}

/// `ScrollLogicalPosition` de `scrollIntoView`.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Align {
    Start,
    Center,
    End,
    Nearest,
}

impl Align {
    fn parse(s: &str) -> Option<Align> {
        Some(match s {
            "start" => Align::Start,
            "center" => Align::Center,
            "end" => Align::End,
            "nearest" => Align::Nearest,
            _ => return None,
        })
    }

    /// El desplazamiento nuevo de un eje ("determine the scroll-into-view position", CSSOM
    /// View §6.1): `cur` es el actual, `(e0, e1)` los bordes del elemento y `(p0, p1)` los
    /// del scrollport, en el mismo sistema de coordenadas.
    fn position(self, cur: f32, (e0, e1): (f32, f32), (p0, p1): (f32, f32)) -> f32 {
        match self {
            Align::Start => cur + e0 - p0,
            Align::End => cur + e1 - p1,
            Align::Center => cur + (e0 + e1) / 2.0 - (p0 + p1) / 2.0,
            Align::Nearest => {
                let (ew, pw) = (e1 - e0, p1 - p0);
                if (e0 < p0 && ew <= pw) || (e1 > p1 && ew > pw) {
                    cur + e0 - p0
                } else if (e1 > p1 && ew <= pw) || (e0 < p0 && ew > pw) {
                    cur + e1 - p1
                } else {
                    cur
                }
            }
        }
    }
}

/// `scrollIntoView(arg)` (CSSOM View §6.1, "scroll an element into view"): cada scroll
/// container que contiene al elemento, del más interno al viewport, se desplaza para alinear
/// la caja según `block` (por defecto `start`) e `inline` (por defecto `nearest`).
fn scroll_into_view(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let arg = a.arg(0);
    let (mut block, mut inline) = (Align::Start, Align::Nearest);
    match &arg {
        Value::Boolean(false) => block = Align::End,
        Value::Object(o) => {
            for (key, slot) in [("block", &mut block), ("inline", &mut inline)] {
                let v = agent.get(*o, &PropertyKey::from(key), arg.clone())?;
                if !v.is_undefined() {
                    let text = agent.to_string(&v)?.to_string();
                    match Align::parse(&text) {
                        Some(al) => *slot = al,
                        None => {
                            return agent.type_error(&format!(
                                "'{text}' is not a valid ScrollLogicalPosition"
                            ));
                        }
                    }
                }
            }
        }
        _ => {}
    }
    if boxes(agent, n).is_empty() {
        return Ok(Value::Undefined);
    }
    // Los scroll containers que contienen al elemento, del más interno al más externo.
    let mut cur = n.node;
    loop {
        let tree = geometry_layout(agent);
        let Some(parent) = layout_scroll::scroll_parent(&tree, cur) else {
            break;
        };
        let element = boxes(agent, n).first().map(|f| f.rect);
        let port = element_fragments(&tree, parent)
            .first()
            .map(|f| f.padding_rect());
        let sb = tree.scroll.boxes.get(&parent).copied();
        drop(tree);
        if let (Some(e), Some(p), Some(sb)) = (element, port, sb) {
            let x = inline.position(sb.offset.0, (e.x, e.right()), (p.x, p.right()));
            let y = block.position(sb.offset.1, (e.y, e.bottom()), (p.y, p.bottom()));
            let new = sb.clamp((x, y));
            if new != sb.offset {
                set_element_scroll(agent, NodeRef::new(0, parent), new, true);
            }
        }
        cur = parent;
    }
    // El viewport.
    let Some(e) = boxes(agent, n).first().map(|f| f.rect) else {
        return Ok(Value::Undefined);
    };
    let m = window_metrics(agent);
    let (sx, sy) = m.scroll;
    let (vw, vh) = (m.viewport.width, m.viewport.height);
    let x = inline.position(sx, (e.x, e.right()), (sx, sx + vw));
    let y = block.position(sy, (e.y, e.bottom()), (sy, sy + vh));
    window_scroll_to(agent, Some(f64::from(x)), Some(f64::from(y)));
    Ok(Value::Undefined)
}

// --- offset* (CSSOM View §7) ---

/// `offsetParent` (CSSOM View §7).
fn offset_parent_of(agent: &mut Agent, n: NodeRef) -> Option<NodeId> {
    if n.doc != 0 {
        return None;
    }
    let tree = current_layout(agent);
    let mine = element_fragments(&tree, n.node).first().copied()?;
    let is_root_or_body = with_doc(agent, 0, |d| {
        d.element(n.node).is_some_and(|e| {
            e.is_html("body") || (e.is_html("html") && d.parent(n.node) == Some(d.root()))
        })
    });
    if is_root_or_body || mine.style.position == Position::Fixed {
        return None;
    }
    let static_self = mine.style.position == Position::Static;
    with_doc(agent, 0, |d| {
        let mut cur = d.parent(n.node);
        while let Some(x) = cur {
            if let Some(el) = d.element(x) {
                if el.is_html("body") {
                    return Some(x);
                }
                if let Some(f) = element_fragments(&tree, x).first() {
                    if f.style.position != Position::Static {
                        return Some(x);
                    }
                    if static_self && (el.is_html("td") || el.is_html("th") || el.is_html("table"))
                    {
                        return Some(x);
                    }
                }
            }
            cur = d.parent(x);
        }
        None
    })
}

fn offset_parent(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(match offset_parent_of(agent, n) {
        Some(p) => wrap_node(agent, NodeRef::new(0, p)),
        None => Value::Null,
    })
}

/// `offsetLeft`/`offsetTop`: el borde de la caja menos el padding edge del `offsetParent`
/// (o el origen de la página si no hay, o si es un `body` sin posicionar).
fn offset_pos(agent: &mut Agent, n: NodeRef) -> (f32, f32) {
    let Some(mine) = boxes(agent, n).into_iter().next() else {
        return (0.0, 0.0);
    };
    let parent = offset_parent_of(agent, n);
    let tree = geometry_layout(agent);
    // E12: la posición de layout no cuenta el scroll de los contenedores intermedios.
    let (sx, sy) = layout_scroll::scroll_offset_between(&tree, n.node, parent);
    let (x, y) = (mine.rect.x + sx, mine.rect.y + sy);
    let Some(p) = parent else {
        return (x, y);
    };
    let Some(pf) = element_fragments(&tree, p).first().copied() else {
        return (x, y);
    };
    let body_static = with_doc(agent, 0, |d| {
        d.element(p).is_some_and(|e| e.is_html("body"))
    }) && pf.style.position == Position::Static;
    if body_static {
        return (x, y);
    }
    let pad = pf.padding_rect();
    (x - pad.x, y - pad.y)
}

fn offset_left(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Number(f64::from(offset_pos(agent, n).0).round()))
}

fn offset_top(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Number(f64::from(offset_pos(agent, n).1).round()))
}

fn offset_size(agent: &mut Agent, a: &CallArgs<'_>, width: bool) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let rects: Vec<Rect> = boxes(agent, n).iter().map(|f| f.rect).collect();
    let r = union(&rects);
    Ok(Value::Number(r.map_or(0.0, |r| {
        f64::from(if width { r.width } else { r.height }).round()
    })))
}

fn offset_width(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    offset_size(agent, a, true)
}

fn offset_height(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    offset_size(agent, a, false)
}

// --- Document: elementFromPoint (CSSOM View §5) ---

pub fn document_partial() -> HostClass {
    HostClass::new("Document")
        .method("elementFromPoint", 2, element_from_point)
        .method("elementsFromPoint", 2, elements_from_point)
        .attribute("scrollingElement", Some(scrolling_element), None)
}

/// El recorrido del hit testing en orden de pintura aproximado: `(z-index efectivo, dentro
/// de una caja posicionada, orden de árbol)` (CSS 2.2 Apéndice E).
struct HitWalk {
    point: (f32, f32),
    scroll: (f32, f32),
    counter: usize,
    hits: Vec<((i32, bool, usize), NodeId)>,
}

impl HitWalk {
    fn visit(&mut self, f: &Fragment, z: i32, positioned: bool, fixed: bool, clip: Option<Rect>) {
        let is_box = !matches!(f.kind, FragmentKind::Line | FragmentKind::Text);
        let fixed = fixed || (is_box && f.style.position == Position::Fixed);
        // Lo fijo está en coordenadas del viewport; el resto, en las de la página.
        let p = if fixed {
            self.point
        } else {
            (self.point.0 + self.scroll.0, self.point.1 + self.scroll.1)
        };
        let own = is_box && f.style.position != Position::Static;
        let positioned = positioned || own;
        let z = if own { f.z_index.unwrap_or(z) } else { z };
        let order = self.counter;
        self.counter += 1;
        let inside = |r: &Rect| p.0 >= r.x && p.0 < r.right() && p.1 >= r.y && p.1 < r.bottom();
        if f.kind != FragmentKind::Line
            && let Some(node) = f.node
            && f.style.visibility == Visibility::Visible
            && f.style.pointer_events != PointerEvents::None
            && inside(&f.rect)
            && clip.is_none_or(|c| inside(&c))
        {
            self.hits.push(((z, positioned, order), node));
        }
        let inner = match (f.clip, clip) {
            (Some(c), Some(o)) => {
                let x = c.x.max(o.x);
                let y = c.y.max(o.y);
                Some(Rect::new(
                    x,
                    y,
                    (c.right().min(o.right()) - x).max(0.0),
                    (c.bottom().min(o.bottom()) - y).max(0.0),
                ))
            }
            (Some(c), None) => Some(c),
            (None, o) => o,
        };
        for c in &f.children {
            // Un fijo no lo recortan los `overflow` de sus ancestros.
            let cc = if c.style.position == Position::Fixed {
                None
            } else {
                inner
            };
            self.visit(c, z, positioned, fixed, cc);
        }
    }
}

/// Los elementos bajo `(x, y)` del viewport, del más alto al más bajo en el orden de
/// pintura (con el elemento raíz al final), retargeteados contra `this` (el documento o un
/// shadow root, E1).
fn hit_elements(agent: &mut Agent, x: f64, y: f64, this: NodeId) -> Vec<NodeId> {
    let m = window_metrics(agent);
    if x < 0.0 || y < 0.0 || x > f64::from(m.viewport.width) || y > f64::from(m.viewport.height) {
        return Vec::new();
    }
    let tree = current_layout(agent);
    let mut walk = HitWalk {
        point: (x as f32, y as f32),
        scroll: m.scroll,
        counter: 0,
        hits: Vec::new(),
    };
    if let Some(root) = &tree.root {
        walk.visit(root, 0, false, false, None);
    }
    let mut hits = walk.hits;
    hits.sort_by_key(|h| std::cmp::Reverse(h.0));
    // Los nodos de texto cuentan como su elemento padre.
    let mut out: Vec<NodeId> = Vec::new();
    with_doc(agent, 0, |d| {
        for (_, n) in hits {
            // E1: retargeting (DOM §4.8) contra el `DocumentOrShadowRoot`.
            let n = match d.retarget(
                Some(bf_dom::events::EventTarget::Node(n)),
                bf_dom::events::EventTarget::Node(this),
            ) {
                Some(bf_dom::events::EventTarget::Node(t)) => t,
                _ => n,
            };
            if d.tree_root(n) != this {
                continue;
            }
            let mut e = Some(n);
            while let Some(x) = e {
                if d.element(x).is_some() {
                    break;
                }
                e = d.parent(x);
            }
            if let Some(e) = e
                && !out.contains(&e)
            {
                out.push(e);
            }
        }
        if this == d.root()
            && let Some(root) = d.children(d.root()).find(|c| d.element(*c).is_some())
            && !out.contains(&root)
        {
            out.push(root);
        }
    });
    out
}

fn point_args(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<(f64, f64)> {
    let x = agent.to_number(&a.arg(0))?;
    let y = agent.to_number(&a.arg(1))?;
    if !x.is_finite() || !y.is_finite() {
        return agent.type_error("The provided double value is non-finite.");
    }
    Ok((x, y))
}

pub(crate) fn element_from_point(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let (x, y) = point_args(agent, a)?;
    if n.doc != 0 {
        return Ok(Value::Null);
    }
    let hits = hit_elements(agent, x, y, n.node);
    Ok(match hits.first() {
        Some(e) => wrap_node(agent, NodeRef::new(0, *e)),
        None => Value::Null,
    })
}

pub(crate) fn elements_from_point(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let (x, y) = point_args(agent, a)?;
    let hits = if n.doc == 0 {
        hit_elements(agent, x, y, n.node)
    } else {
        Vec::new()
    };
    let vals: Vec<Value> = hits
        .into_iter()
        .map(|e| wrap_node(agent, NodeRef::new(0, e)))
        .collect();
    Ok(Value::Object(agent.create_array_from_list(&vals)?))
}

/// `document.scrollingElement` (CSSOM View §5): `html`, o `body` en modo quirks.
fn scrolling_element(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let found = with_doc(agent, n.doc, |d| {
        let html = d
            .children(d.root())
            .find(|c| d.element(*c).is_some_and(|e| e.is_html("html")))?;
        if d.mode == bf_dom::QuirksMode::Quirks {
            d.children(html)
                .find(|c| d.element(*c).is_some_and(|e| e.is_html("body")))
        } else {
            Some(html)
        }
    });
    Ok(match found {
        Some(e) => wrap_node(agent, NodeRef::new(n.doc, e)),
        None => Value::Null,
    })
}

// --- Window (CSSOM View §4) ---

pub fn window_partial() -> HostClass {
    HostClass::new("Window")
        .attribute("innerWidth", Some(inner_width), None)
        .attribute("innerHeight", Some(inner_height), None)
        .attribute("outerWidth", Some(outer_width), None)
        .attribute("outerHeight", Some(outer_height), None)
        .attribute("devicePixelRatio", Some(device_pixel_ratio), None)
        .attribute("scrollX", Some(scroll_x), None)
        .attribute("pageXOffset", Some(scroll_x), None)
        .attribute("scrollY", Some(scroll_y), None)
        .attribute("pageYOffset", Some(scroll_y), None)
        .attribute("screenX", Some(screen_x), None)
        .attribute("screenLeft", Some(screen_x), None)
        .attribute("screenY", Some(screen_y), None)
        .attribute("screenTop", Some(screen_y), None)
        .method("scroll", 0, window_scroll)
        .method("scrollTo", 0, window_scroll)
        .method("scrollBy", 0, window_scroll_by)
        .method("moveTo", 2, no_op)
        .method("moveBy", 2, no_op)
        .method("resizeTo", 2, no_op)
        .method("resizeBy", 2, no_op)
}

fn num(v: f32) -> JsResult<Value> {
    Ok(Value::Number(f64::from(v)))
}

fn inner_width(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    num(window_metrics(agent).viewport.width.floor())
}

fn inner_height(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    num(window_metrics(agent).viewport.height.floor())
}

fn outer_width(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    num(window_metrics(agent).outer_size.0.floor())
}

fn outer_height(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    num(window_metrics(agent).outer_size.1.floor())
}

fn device_pixel_ratio(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    num(window_metrics(agent).dpr)
}

fn scroll_x(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    num(window_metrics(agent).scroll.0)
}

fn scroll_y(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    num(window_metrics(agent).scroll.1)
}

fn screen_x(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    num(window_metrics(agent).screen_position.0)
}

fn screen_y(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    num(window_metrics(agent).screen_position.1)
}

/// Desplaza el viewport (sujeto a los límites del documento) y deja el pedido al shell.
fn window_scroll_to(agent: &mut Agent, x: Option<f64>, y: Option<f64>) {
    let m = window_metrics(agent);
    let tree = current_layout(agent);
    let (w, h) = document_extent(&tree);
    let clamp = |v: Option<f64>, cur: f32, max: f32| {
        v.map_or(cur, |v| {
            let v = if v.is_finite() { v as f32 } else { 0.0 };
            v.clamp(0.0, max.max(0.0))
        })
    };
    let nx = clamp(x, m.scroll.0, w - m.viewport.width);
    let ny = clamp(y, m.scroll.1, h - m.viewport.height);
    let st = state(agent);
    st.scroll = (nx, ny);
    st.scroll_request = Some((nx, ny));
    if tree.scroll.viewport_sticky {
        update_layouts(agent, |t| {
            layout_scroll::page_scrolled(t, (nx, ny));
        });
    }
}

fn window_scroll(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (x, y) = scroll_args(agent, a)?;
    window_scroll_to(agent, x, y);
    Ok(Value::Undefined)
}

fn window_scroll_by(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (dx, dy) = scroll_args(agent, a)?;
    let cur = window_metrics(agent).scroll;
    window_scroll_to(
        agent,
        Some(f64::from(cur.0) + dx.unwrap_or(0.0)),
        Some(f64::from(cur.1) + dy.unwrap_or(0.0)),
    );
    Ok(Value::Undefined)
}

fn no_op(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Undefined)
}

/// El elemento de un valor (para las APIs que reciben `Element`).
pub fn element_arg(agent: &Agent, v: &Value) -> Option<NodeRef> {
    unwrap_node(agent, v)
}
