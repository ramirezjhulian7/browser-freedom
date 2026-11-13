//! `IntersectionObserver` e `IntersectionObserverEntry` (W3C Intersection Observer, §2.2–§2.5
//! las interfaces y §3.2 el modelo de proceso). **E2.**
//!
//! - El observador vive como dato del realm ([`IoState`]); el objeto JS lleva su id y, en sus
//!   slots, el callback y el array congelado de `thresholds`. Mientras observa algún elemento
//!   queda fijado (§2.2: el elemento observado mantiene vivo a su observador, aunque el script
//!   ya no lo referencie).
//! - [`update_observations`] es "run the update intersection observations steps" (§3.2.8), que
//!   HTML corre en *update the rendering* (§8.1.7.3, paso 19) después de maquetar: calcula, con
//!   la maqueta al día, la intersección de cada objetivo con la raíz (§3.2.9, recortada por los
//!   ancestros con `overflow` de su cadena de bloques contenedores) y encola una entrada cuando
//!   cambia el umbral cruzado o `isIntersecting` (la primera vez siempre: el índice anterior es
//!   −1).
//! - Las entradas se entregan en una tarea ("queue an intersection observer task", §3.2.4;
//!   "notify intersection observers", §3.2.5).

use std::collections::HashMap;
use std::sync::Arc;

use bf_dom::{Document, NodeId};
use bf_js::host::{HostClass, HostClassId, HostHandle, TaskSource, webidl};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, RealmId, Throw};
use bf_layout::{FragmentKind, FragmentTree, Rect};
use bf_style::properties::types::{Overflow, Position};

use super::view::RectData;
use crate::dom::{throw_dom, unwrap_node, wrap_node};
use crate::{NodeRef, WebContextHandle, with_doc};

// --- Estado ---

/// La raíz de intersección (§2.2, "intersection root").
#[derive(Debug, Clone, Copy, PartialEq)]
enum Root {
    /// Sin `root`: el viewport del documento de nivel superior ("implicit root").
    Implicit,
    /// Un `Document` como raíz: su viewport.
    Document(crate::DocIndex),
    Element(NodeRef),
}

/// Un lado de `rootMargin`: píxeles o porcentaje.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Margin {
    Px(f64),
    Percent(f64),
}

impl Margin {
    fn resolve(self, basis: f64) -> f64 {
        match self {
            Margin::Px(v) => v,
            Margin::Percent(p) => basis * p / 100.0,
        }
    }

    fn serialize(self) -> String {
        match self {
            Margin::Px(v) => format!("{}px", number(v)),
            Margin::Percent(p) => format!("{}%", number(p)),
        }
    }
}

/// Un número como lo escribe JavaScript (`10`, `0.5`).
fn number(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// Un objetivo observado (§2.2, "IntersectionObserverRegistration").
#[derive(Debug, Clone, Copy)]
struct Registration {
    target: NodeRef,
    /// *previousThresholdIndex*: −1 hasta la primera observación.
    previous_threshold: i64,
    previous_intersecting: bool,
}

/// Una entrada encolada (los datos de un `IntersectionObserverEntry`).
#[derive(Debug, Clone, Copy)]
pub struct EntryData {
    time: f64,
    root_bounds: Option<RectData>,
    bounding_client_rect: RectData,
    intersection_rect: RectData,
    is_intersecting: bool,
    intersection_ratio: f64,
    target: NodeRef,
}

struct Observer {
    id: u64,
    /// El objeto JS, fijado mientras observe algo (sin objetivos se suelta y, si el script ya
    /// no lo referencia, se recolecta; sus entradas pendientes se descartan).
    pinned: Option<HostHandle>,
    root: Root,
    margin: [Margin; 4],
    /// `scrollMargin` (Intersection Observer v2 §2.2): se guarda y se devuelve, pero no recorta
    /// todavía los scroll containers intermedios.
    scroll_margin: [Margin; 4],
    thresholds: Vec<f64>,
    targets: Vec<Registration>,
    queue: Vec<EntryData>,
}

/// Los `IntersectionObserver` de una ventana (dato del realm).
#[derive(Default)]
pub struct IoState {
    observers: Vec<Observer>,
    next_id: u64,
    /// El *IntersectionObserverTaskQueued flag* del documento (§3.1).
    task_queued: bool,
    /// Lo que determinaba la geometría en la última actualización (ver [`super::rendering_key`]).
    last_key: Option<super::RenderingKey>,
}

/// El payload de un `IntersectionObserver`: su id en [`IoState`].
#[derive(Debug)]
struct ObserverData {
    id: u64,
}

fn io_state(agent: &mut Agent, realm: RealmId) -> &mut IoState {
    if agent.realm_data::<IoState>(realm).is_none() {
        agent.set_realm_data(realm, IoState::default());
    }
    agent.realm_data_mut::<IoState>(realm).expect("IoState")
}

fn current_realm(agent: &Agent) -> RealmId {
    WebContextHandle::current(agent).realm
}

// --- Clases ---

pub fn register_classes(agent: &mut Agent) -> (HostClassId, HostClassId) {
    let observer = agent.register_host_class(
        HostClass::new("IntersectionObserver")
            .constructor(ctor, 1)
            .attribute("root", Some(get_root), None)
            .attribute("rootMargin", Some(get_root_margin), None)
            .attribute("scrollMargin", Some(get_scroll_margin), None)
            .attribute("thresholds", Some(get_thresholds), None)
            .attribute("delay", Some(get_delay), None)
            .attribute("trackVisibility", Some(get_track_visibility), None)
            .method("observe", 1, observe)
            .method("unobserve", 1, unobserve)
            .method("disconnect", 0, disconnect)
            .method("takeRecords", 0, take_records),
    );
    let entry = agent.register_host_class(
        HostClass::new("IntersectionObserverEntry")
            .constructor(entry_ctor, 1)
            .attribute("time", Some(entry_time), None)
            .attribute("rootBounds", Some(entry_root_bounds), None)
            .attribute("boundingClientRect", Some(entry_bounding_client_rect), None)
            .attribute("intersectionRect", Some(entry_intersection_rect), None)
            .attribute("isIntersecting", Some(entry_is_intersecting), None)
            .attribute("isVisible", Some(entry_is_visible), None)
            .attribute("intersectionRatio", Some(entry_intersection_ratio), None)
            .attribute("target", Some(entry_target), None),
    );
    (observer, entry)
}

// --- El constructor (§2.2, "initialize a new IntersectionObserver") ---

/// "Parse a root margin" (§2.2): 1 a 4 longitudes absolutas o porcentajes separados por
/// espacios, repetidos como en `margin`; cualquier otra cosa es un error.
fn parse_margin(s: &str) -> Option<[Margin; 4]> {
    let tokens: Vec<&str> = s
        .split(|c: char| c.is_ascii_whitespace())
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.len() > 4 {
        return None;
    }
    let mut v = Vec::new();
    for t in &tokens {
        let lower = t.to_ascii_lowercase();
        let m = if let Some(n) = lower.strip_suffix('%') {
            Margin::Percent(css_number(n)?)
        } else {
            // Las unidades absolutas (CSS Values 4 §6.2), en px.
            const UNITS: [(&str, f64); 7] = [
                ("px", 1.0),
                ("cm", 96.0 / 2.54),
                ("mm", 96.0 / 25.4),
                ("in", 96.0),
                ("pt", 96.0 / 72.0),
                ("pc", 16.0),
                ("q", 96.0 / 101.6),
            ];
            let (n, factor) = UNITS
                .iter()
                .find_map(|(u, f)| lower.strip_suffix(u).map(|n| (n, *f)))?;
            // Los navegadores guardan el margen en px enteros (WPT
            // `root-margin-scroll-margin-units`: 10cm son 377px).
            Margin::Px((css_number(n)? * factor).trunc())
        };
        v.push(m);
    }
    let zero = Margin::Px(0.0);
    Some(match v.as_slice() {
        [] => [zero; 4],
        [a] => [*a; 4],
        [a, b] => [*a, *b, *a, *b],
        [a, b, c] => [*a, *b, *c, *b],
        [a, b, c, d] => [*a, *b, *c, *d],
        _ => return None,
    })
}

/// Un `<number>` de CSS (sin exponentes raros ni espacios).
fn css_number(s: &str) -> Option<f64> {
    if s.is_empty()
        || !s
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | '+' | '-' | 'e' | 'E'))
    {
        return None;
    }
    s.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn serialize_margin(m: &[Margin; 4]) -> String {
    m.iter()
        .map(|v| v.serialize())
        .collect::<Vec<_>>()
        .join(" ")
}

fn ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let cb = a.arg(0);
    if !agent.is_callable(&cb) {
        return agent.type_error(
            "Failed to construct 'IntersectionObserver': The callback provided as parameter 1 is \
             not a function.",
        );
    }
    let opts = a.arg(1);
    let get = |agent: &mut Agent, name: &str| -> JsResult<Value> {
        match &opts {
            Value::Object(o) => agent.get(*o, &PropertyKey::from(name), opts.clone()),
            Value::Undefined | Value::Null => Ok(Value::Undefined),
            _ => agent.type_error(
                "Failed to construct 'IntersectionObserver': The provided value is not of type \
                 'IntersectionObserverInit'.",
            ),
        }
    };
    // Los miembros del diccionario en orden lexicográfico (WebIDL §3.2.17).
    let delay = get(agent, "delay")?;
    let delay = if delay.is_undefined() {
        0.0
    } else {
        webidl::to_double(agent, &delay)?
    };
    let root_v = get(agent, "root")?;
    let root = match &root_v {
        Value::Undefined | Value::Null => Root::Implicit,
        v => match unwrap_node(agent, v) {
            Some(n) => {
                let (is_element, is_document) = with_doc(agent, n.doc, |d| {
                    (d.element(n.node).is_some(), n.node == d.root())
                });
                if is_element {
                    Root::Element(n)
                } else if is_document {
                    Root::Document(n.doc)
                } else {
                    return agent.type_error(
                        "Failed to construct 'IntersectionObserver': Failed to read the 'root' \
                         property from 'IntersectionObserverInit': The provided value is not of \
                         type '(Document or Element)'.",
                    );
                }
            }
            None => {
                return agent.type_error(
                    "Failed to construct 'IntersectionObserver': Failed to read the 'root' \
                     property from 'IntersectionObserverInit': The provided value is not of type \
                     '(Document or Element)'.",
                );
            }
        },
    };
    let margin_v = get(agent, "rootMargin")?;
    let margin_s = if margin_v.is_undefined() {
        "0px".to_owned()
    } else {
        webidl::to_rust_string(agent, &margin_v)?
    };
    let scroll_margin_v = get(agent, "scrollMargin")?;
    let scroll_margin_s = if scroll_margin_v.is_undefined() {
        "0px".to_owned()
    } else {
        webidl::to_rust_string(agent, &scroll_margin_v)?
    };
    let threshold_v = get(agent, "threshold")?;
    let mut thresholds = match &threshold_v {
        Value::Undefined => vec![0.0],
        Value::Object(_) => webidl::to_sequence(agent, &threshold_v, webidl::to_double)?,
        v => vec![webidl::to_double(agent, v)?],
    };
    let track = get(agent, "trackVisibility")?;
    let track = webidl::to_boolean(agent, &track);
    // "Initialize a new IntersectionObserver" (§2.2): el margen, los umbrales y `delay`.
    let Some(margin) = parse_margin(&margin_s) else {
        return throw_dom(agent, bf_dom::DomException::SyntaxError);
    };
    let Some(scroll_margin) = parse_margin(&scroll_margin_s) else {
        return throw_dom(agent, bf_dom::DomException::SyntaxError);
    };
    if thresholds.iter().any(|t| !(0.0..=1.0).contains(t)) {
        return agent.range_error(
            "Failed to construct 'IntersectionObserver': Threshold values must be numbers \
             between 0 and 1",
        );
    }
    thresholds.sort_by(f64::total_cmp);
    if thresholds.is_empty() {
        thresholds.push(0.0);
    }
    let _ = (delay, track);
    let realm = current_realm(agent);
    let st = io_state(agent, realm);
    st.next_id += 1;
    let id = st.next_id;
    st.observers.push(Observer {
        id,
        pinned: None,
        root,
        margin,
        scroll_margin,
        thresholds: thresholds.clone(),
        targets: Vec::new(),
        queue: Vec::new(),
    });
    let class = super::classes(agent).intersection_observer;
    let nt = a.new_target.expect("new.target");
    let o = agent.host_create_from_new_target(nt, class, None, Box::new(ObserverData { id }))?;
    // `thresholds` es un FrozenArray: el mismo objeto en cada lectura.
    let values: Vec<Value> = thresholds.iter().map(|t| Value::Number(*t)).collect();
    let arr = agent.create_array_from_list(&values)?;
    agent.set_integrity_level(arr, true)?;
    if let Some(s) = agent.host_slots_mut(o) {
        *s = vec![cb, Value::Object(arr), root_v];
    }
    Ok(Value::Object(o))
}

/// El id del observador de `this`.
fn this_id(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<u64> {
    match agent.host_unwrap::<ObserverData>(&a.this) {
        Some(d) => Ok(d.id),
        None => agent.type_error("Illegal invocation"),
    }
}

fn with_observer<T>(agent: &mut Agent, id: u64, f: impl FnOnce(&mut Observer) -> T) -> Option<T> {
    let realm = current_realm(agent);
    io_state(agent, realm)
        .observers
        .iter_mut()
        .find(|o| o.id == id)
        .map(f)
}

fn slot(agent: &mut Agent, a: &CallArgs<'_>, i: usize) -> Value {
    match &a.this {
        Value::Object(o) => agent
            .host_slots_mut(*o)
            .and_then(|s| s.get(i).cloned())
            .unwrap_or_default(),
        _ => Value::Undefined,
    }
}

fn get_root(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_id(agent, a)?;
    let v = slot(agent, a, 2);
    Ok(if v.is_undefined() { Value::Null } else { v })
}

fn get_root_margin(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_id(agent, a)?;
    let s = with_observer(agent, id, |o| serialize_margin(&o.margin)).unwrap_or_default();
    Ok(Value::str(&s))
}

fn get_scroll_margin(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_id(agent, a)?;
    let s = with_observer(agent, id, |o| serialize_margin(&o.scroll_margin)).unwrap_or_default();
    Ok(Value::str(&s))
}

fn get_thresholds(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_id(agent, a)?;
    Ok(slot(agent, a, 1))
}

fn get_delay(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_id(agent, a)?;
    Ok(Value::Number(0.0))
}

fn get_track_visibility(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_id(agent, a)?;
    Ok(Value::Boolean(false))
}

/// El elemento del argumento `target` (TypeError si no lo es).
fn arg_element(agent: &mut Agent, v: &Value, method: &str) -> JsResult<NodeRef> {
    if let Some(n) = unwrap_node(agent, v)
        && with_doc(agent, n.doc, |d| d.element(n.node).is_some())
    {
        return Ok(n);
    }
    agent.type_error(&format!(
        "Failed to execute '{method}' on 'IntersectionObserver': parameter 1 is not of type \
         'Element'."
    ))
}

/// `observe(target)` (§2.2): agrega el objetivo con *previousThresholdIndex* −1, para que la
/// próxima actualización lo notifique.
fn observe(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_id(agent, a)?;
    let target = arg_element(agent, &a.arg(0), "observe")?;
    let this = a.this.clone();
    let realm = current_realm(agent);
    let needs_pin = with_observer(agent, id, |o| {
        if o.targets.iter().any(|r| r.target == target) {
            return false;
        }
        o.targets.push(Registration {
            target,
            previous_threshold: -1,
            previous_intersecting: false,
        });
        o.pinned.is_none()
    })
    .unwrap_or(false);
    if needs_pin {
        let h = agent.host_pin(this);
        with_observer(agent, id, |o| o.pinned = Some(h));
    }
    io_state(agent, realm).last_key = None;
    Ok(Value::Undefined)
}

/// Suelta el objeto del observador si ya no observa nada.
fn unpin_if_idle(agent: &mut Agent, id: u64) {
    let h = with_observer(agent, id, |o| {
        if o.targets.is_empty() {
            o.pinned.take()
        } else {
            None
        }
    })
    .flatten();
    if let Some(h) = h {
        agent.host_unpin(h);
    }
}

fn unobserve(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_id(agent, a)?;
    let target = arg_element(agent, &a.arg(0), "unobserve")?;
    with_observer(agent, id, |o| o.targets.retain(|r| r.target != target));
    unpin_if_idle(agent, id);
    Ok(Value::Undefined)
}

fn disconnect(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_id(agent, a)?;
    with_observer(agent, id, |o| o.targets.clear());
    unpin_if_idle(agent, id);
    Ok(Value::Undefined)
}

/// `takeRecords()`: la cola de entradas, que queda vacía.
fn take_records(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_id(agent, a)?;
    let queue = with_observer(agent, id, |o| std::mem::take(&mut o.queue)).unwrap_or_default();
    let values: Vec<Value> = queue.iter().map(|e| entry_object(agent, *e)).collect();
    Ok(Value::Object(agent.create_array_from_list(&values)?))
}

// --- La geometría (§3.2.9) ---

/// La caja de un elemento en coordenadas del viewport.
#[derive(Clone)]
struct BoxInfo {
    /// La unión de sus border boxes no vacíos (la de `getBoundingClientRect`).
    border: Rect,
    padding: Rect,
    position: Position,
    clips: bool,
}

/// Las cajas de los elementos de una maqueta, por nodo (una pasada por el árbol).
fn element_boxes(tree: &FragmentTree, scroll: (f32, f32)) -> HashMap<NodeId, BoxInfo> {
    fn walk(f: &bf_layout::Fragment, scroll: (f32, f32), out: &mut HashMap<NodeId, BoxInfo>) {
        // Lo que está dentro de una caja `fixed` tampoco se desplaza con la página.
        let scroll = if f.style.position == Position::Fixed {
            (0.0, 0.0)
        } else {
            scroll
        };
        if let Some(n) = f.node
            && f.pseudo.is_none()
            && !matches!(f.kind, FragmentKind::Line | FragmentKind::Text)
        {
            let s = scroll;
            let shift = |r: Rect| Rect::new(r.x - s.0, r.y - s.1, r.width, r.height);
            let border = shift(f.rect);
            match out.get_mut(&n) {
                Some(b) => {
                    if border.width > 0.0 || border.height > 0.0 {
                        b.border = if b.border.width > 0.0 || b.border.height > 0.0 {
                            union(b.border, border)
                        } else {
                            border
                        };
                    }
                }
                None => {
                    out.insert(
                        n,
                        BoxInfo {
                            border,
                            padding: shift(f.padding_rect()),
                            position: f.style.position,
                            clips: f.kind != FragmentKind::Inline
                                && (f.style.overflow_x != Overflow::Visible
                                    || f.style.overflow_y != Overflow::Visible),
                        },
                    );
                }
            }
        }
        for c in &f.children {
            walk(c, scroll, out);
        }
    }
    let mut out = HashMap::new();
    if let Some(root) = &tree.root {
        walk(root, scroll, &mut out);
    }
    out
}

fn union(a: Rect, b: Rect) -> Rect {
    let x0 = a.x.min(b.x);
    let y0 = a.y.min(b.y);
    let x1 = (a.x + a.width).max(b.x + b.width);
    let y1 = (a.y + a.height).max(b.y + b.height);
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

/// Un rectángulo en f64 (bordes), para intersecar sin perder los de área cero.
#[derive(Debug, Clone, Copy)]
struct Edges {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Edges {
    fn of(r: Rect) -> Edges {
        Edges {
            x0: f64::from(r.x),
            y0: f64::from(r.y),
            x1: f64::from(r.x + r.width),
            y1: f64::from(r.y + r.height),
        }
    }

    /// La intersección "edge-inclusive" (§3.2.9): dos rectángulos que solo se tocan por un
    /// borde intersecan, con área cero.
    fn intersect(self, o: Edges) -> Option<Edges> {
        let r = Edges {
            x0: self.x0.max(o.x0),
            y0: self.y0.max(o.y0),
            x1: self.x1.min(o.x1),
            y1: self.y1.min(o.y1),
        };
        (r.x1 >= r.x0 && r.y1 >= r.y0).then_some(r)
    }

    fn area(self) -> f64 {
        (self.x1 - self.x0).max(0.0) * (self.y1 - self.y0).max(0.0)
    }

    fn rect(self) -> RectData {
        RectData {
            x: self.x0,
            y: self.y0,
            width: (self.x1 - self.x0).max(0.0),
            height: (self.y1 - self.y0).max(0.0),
        }
    }
}

/// El bloque contenedor de `node` en la cadena que sigue §3.2.9 (CSS 2 §10.1): `fixed` sale
/// al viewport (`None`); `absolute`, al ancestro posicionado más cercano o al elemento raíz;
/// lo demás, al elemento padre.
fn containing_block(
    doc: &Document,
    boxes: &HashMap<NodeId, BoxInfo>,
    node: NodeId,
) -> Option<NodeId> {
    let position = boxes.get(&node).map(|b| b.position);
    let mut cur = parent_element(doc, node);
    match position {
        Some(Position::Fixed) => None,
        Some(Position::Absolute) => {
            while let Some(n) = cur {
                if boxes
                    .get(&n)
                    .is_some_and(|b| b.position != Position::Static)
                {
                    return Some(n);
                }
                cur = parent_element(doc, n);
            }
            doc.document_element().filter(|&r| r != node)
        }
        _ => cur,
    }
}

/// El elemento padre en el árbol plano (el anfitrión para la raíz de un shadow tree).
fn parent_element(doc: &Document, node: NodeId) -> Option<NodeId> {
    let mut cur = doc.parent(node);
    while let Some(n) = cur {
        if doc.element(n).is_some() {
            return Some(n);
        }
        cur = doc.shadow_host(n).or_else(|| doc.parent(n));
    }
    None
}

/// Lo que hace falta para calcular las observaciones de un documento.
struct Geometry {
    boxes: HashMap<NodeId, BoxInfo>,
    viewport: Edges,
}

/// El rectángulo de la raíz (§2.2, "root intersection rectangle"), ya con `rootMargin`; `None`
/// si una raíz elemento no tiene caja.
fn root_rect(geo: &Geometry, root: Root, margin: &[Margin; 4]) -> Option<Edges> {
    let base = match root {
        Root::Implicit | Root::Document(_) => geo.viewport,
        Root::Element(n) => {
            let b = geo.boxes.get(&n.node)?;
            // Con *content clip*, su padding area; si no, su border box.
            Edges::of(if b.clips { b.padding } else { b.border })
        }
    };
    let w = base.x1 - base.x0;
    let h = base.y1 - base.y0;
    let x0 = base.x0 - margin[3].resolve(w);
    let y0 = base.y0 - margin[0].resolve(h);
    // Unos márgenes negativos que suman el tamaño entero no deben dejar, por redondeo, un
    // rectángulo negativo (que no intersecaría con nada; WPT `root-margin-rounding`).
    Some(Edges {
        x0,
        y0,
        x1: (base.x1 + margin[1].resolve(w)).max(x0),
        y1: (base.y1 + margin[2].resolve(h)).max(y0),
    })
}

/// "Compute the intersection of a target element and the root" (§3.2.9) en el documento 0:
/// el border box del objetivo recortado por los ancestros con *content clip* de su cadena de
/// bloques contenedores hasta la raíz, y por el rectángulo de la raíz. `None` si no intersecan
/// o si una raíz elemento no está en esa cadena.
fn compute_intersection(
    doc: &Document,
    geo: &Geometry,
    target: NodeId,
    root: Root,
    root_bounds: Edges,
) -> Option<Edges> {
    let mut rect = Edges::of(geo.boxes.get(&target)?.border);
    let root_el = match root {
        Root::Element(n) => Some(n.node),
        _ => None,
    };
    let html = doc.document_element();
    let mut container = containing_block(doc, &geo.boxes, target);
    loop {
        match container {
            Some(c) if Some(c) == root_el => break,
            Some(c) => {
                // El elemento raíz (y el `body` cuyo `overflow` pasa al viewport) recortan con
                // el viewport, que ya es el rectángulo de la raíz implícita.
                let propagated = Some(c) == html || Some(c) == doc.body();
                if !propagated
                    && let Some(b) = geo.boxes.get(&c)
                    && b.clips
                {
                    rect = rect.intersect(Edges::of(b.padding))?;
                }
                container = containing_block(doc, &geo.boxes, c);
            }
            // Llegó al viewport sin pasar por la raíz elemento: no está en su cadena.
            None if root_el.is_some() => return None,
            None => break,
        }
    }
    rect.intersect(root_bounds)
}

/// "Run the update intersection observations steps" (§3.2.8) para la ventana `realm`, con su
/// maqueta actual. Encola una tarea si quedaron entradas.
pub(crate) fn update_observations(agent: &mut Agent, realm: RealmId) {
    let has_targets = agent
        .realm_data::<IoState>(realm)
        .is_some_and(|s| s.observers.iter().any(|o| !o.targets.is_empty()));
    if !has_targets {
        return;
    }
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let key = super::rendering_key(agent, realm);
    let time = crate::bom::performance::now_ms(agent);
    let tree: Arc<FragmentTree> = super::current_layout(agent);
    let metrics = super::window_metrics(agent);
    let geo = Geometry {
        boxes: element_boxes(&tree, metrics.scroll),
        viewport: Edges {
            x0: 0.0,
            y0: 0.0,
            x1: f64::from(metrics.viewport.width),
            y1: f64::from(metrics.viewport.height),
        },
    };
    drop(tree);
    let mut queued = false;
    let ctx = WebContextHandle { realm };
    let st = agent.take_realm_data::<IoState>(realm).unwrap_or_default();
    let mut st = st;
    {
        let doc0 = ctx.get(agent).document();
        for o in &mut st.observers {
            let root_in_doc0 = match o.root {
                Root::Implicit => true,
                Root::Document(d) => d == 0,
                Root::Element(n) => n.doc == 0 && doc0.is_connected(n.node),
            };
            let bounds = if root_in_doc0 {
                root_rect(&geo, o.root, &o.margin)
            } else {
                None
            };
            for reg in &mut o.targets {
                let t = reg.target;
                // Un objetivo de otro documento (uno sin ventana, creado con
                // `createHTMLDocument`): con la raíz implícita no se observa hasta que entre en
                // este; con una raíz explícita se informa que no interseca (§3.2.8 paso 2.2;
                // WPT `target-in-detached-document`, `explicit-root-different-document`).
                if t.doc != 0 && o.root == Root::Implicit {
                    continue;
                }
                let observable =
                    t.doc == 0 && doc0.is_connected(t.node) && geo.boxes.contains_key(&t.node);
                // Sin caja que medir, los tres rectángulos de la entrada quedan en cero.
                let (target_rect, inter, root_bounds) = match bounds {
                    Some(b) if observable => {
                        let tr = Edges::of(geo.boxes[&t.node].border);
                        (
                            Some(tr),
                            compute_intersection(doc0, &geo, t.node, o.root, b),
                            Some(b),
                        )
                    }
                    _ => (None, None, None),
                };
                let intersects = inter.is_some();
                let target_area = target_rect.map_or(0.0, Edges::area);
                let inter_area = inter.map_or(0.0, Edges::area);
                let ratio = if target_area > 0.0 {
                    (inter_area / target_area).min(1.0)
                } else if intersects {
                    1.0
                } else {
                    0.0
                };
                // §3.2.8 paso 2.10: el primer umbral mayor que la proporción, o la longitud de
                // la lista si la alcanza o pasa el último.
                let threshold = if intersects {
                    o.thresholds
                        .iter()
                        .position(|&th| th > ratio)
                        .unwrap_or(o.thresholds.len()) as i64
                } else {
                    0
                };
                // Como los navegadores (WPT `initial-observation-with-threshold`,
                // `isIntersecting-threshold`): por debajo del primer umbral la entrada no
                // interseca, aunque los rectángulos se toquen.
                let is_intersecting = intersects && threshold > 0;
                if threshold != reg.previous_threshold
                    || is_intersecting != reg.previous_intersecting
                {
                    o.queue.push(EntryData {
                        time,
                        root_bounds: Some(root_bounds.map_or_else(RectData::default, Edges::rect)),
                        bounding_client_rect: target_rect
                            .map_or_else(RectData::default, Edges::rect),
                        intersection_rect: inter.map_or_else(RectData::default, Edges::rect),
                        is_intersecting,
                        intersection_ratio: ratio,
                        target: t,
                    });
                    reg.previous_threshold = threshold;
                    reg.previous_intersecting = is_intersecting;
                    queued = true;
                }
            }
        }
    }
    st.last_key = Some(key);
    let queue_task = queued && !st.task_queued;
    if queue_task {
        st.task_queued = true;
    }
    agent.set_realm_data(realm, st);
    if queue_task {
        // "Queue an intersection observer task" (§3.2.4).
        agent.queue_task(
            TaskSource::DomManipulation,
            Box::new(move |agent: &mut Agent| notify(agent, realm)),
        );
    }
    agent.current_realm = saved;
}

/// ¿Hay algo que calcular en la próxima actualización? (Objetivos sin su primera observación,
/// o la geometría pudo cambiar desde la última.)
pub(crate) fn pending(agent: &Agent, realm: RealmId, key: &super::RenderingKey) -> bool {
    agent.realm_data::<IoState>(realm).is_some_and(|s| {
        s.observers.iter().any(|o| !o.targets.is_empty())
            && (s.last_key.as_ref() != Some(key)
                || s.observers
                    .iter()
                    .any(|o| o.targets.iter().any(|r| r.previous_threshold < 0)))
    })
}

/// "Notify intersection observers" (§3.2.5): cada observador con entradas recibe su cola.
fn notify(agent: &mut Agent, realm: RealmId) {
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let batches: Vec<(u64, Vec<EntryData>)> = {
        let st = io_state(agent, realm);
        st.task_queued = false;
        st.observers
            .iter_mut()
            .filter(|o| !o.queue.is_empty())
            .map(|o| (o.id, std::mem::take(&mut o.queue)))
            .collect()
    };
    for (id, entries) in batches {
        let Some(obj) = observer_object(agent, realm, id) else {
            continue;
        };
        let cb = match &obj {
            Value::Object(o) => agent
                .host_slots_mut(*o)
                .and_then(|s| s.first().cloned())
                .unwrap_or_default(),
            _ => continue,
        };
        let values: Vec<Value> = entries.iter().map(|e| entry_object(agent, *e)).collect();
        let arr = match agent.create_array_from_list(&values) {
            Ok(a) => Value::Object(a),
            Err(_) => continue,
        };
        match agent.call(&cb, obj.clone(), &[arr, obj]) {
            Ok(_) => {}
            Err(Throw::Value(e)) => crate::dom::events::report_exception(agent, &e),
            Err(_) => break,
        }
        if agent.frames.is_empty()
            && let Err(Throw::Value(e)) = agent.perform_microtask_checkpoint()
        {
            agent.report_exception(&e);
        }
    }
    agent.current_realm = saved;
}

/// El objeto JS de un observador (fijado mientras observa; si ya no observa nada pero tiene
/// entradas, se perdió: las entradas se descartan como las de un observador recolectado).
fn observer_object(agent: &mut Agent, realm: RealmId, id: u64) -> Option<Value> {
    let h = agent
        .realm_data::<IoState>(realm)?
        .observers
        .iter()
        .find(|o| o.id == id)?
        .pinned?;
    agent.host_pinned(h)
}

// --- IntersectionObserverEntry ---

fn entry_object(agent: &mut Agent, e: EntryData) -> Value {
    let class = super::classes(agent).intersection_observer_entry;
    Value::Object(agent.host_create(class, Box::new(e)))
}

fn entry(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<EntryData> {
    match agent.host_unwrap::<EntryData>(&a.this) {
        Some(e) => Ok(*e),
        None => agent.type_error("Illegal invocation"),
    }
}

/// `new IntersectionObserverEntry(init)` (§2.4).
fn entry_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let init = a.arg(0);
    let Value::Object(o) = init else {
        return agent.type_error(
            "Failed to construct 'IntersectionObserverEntry': parameter 1 is not of type \
             'IntersectionObserverEntryInit'.",
        );
    };
    let get = |agent: &mut Agent, name: &str| agent.get(o, &PropertyKey::from(name), init.clone());
    let rect = |agent: &mut Agent, v: &Value| -> JsResult<RectData> {
        let Value::Object(r) = v else {
            return Ok(RectData::default());
        };
        let mut out = [0.0; 4];
        for (i, k) in ["height", "width", "x", "y"].iter().enumerate() {
            let x = agent.get(*r, &PropertyKey::from(*k), v.clone())?;
            out[i] = if x.is_undefined() {
                0.0
            } else {
                webidl::to_unrestricted_double(agent, &x)?
            };
        }
        Ok(RectData {
            x: out[2],
            y: out[3],
            width: out[1],
            height: out[0],
        })
    };
    let v = get(agent, "boundingClientRect")?;
    let bounding_client_rect = rect(agent, &v)?;
    let v = get(agent, "intersectionRatio")?;
    let ratio = if v.is_undefined() {
        None
    } else {
        Some(webidl::to_double(agent, &v)?)
    };
    let v = get(agent, "intersectionRect")?;
    let intersection_rect = rect(agent, &v)?;
    let v = get(agent, "isIntersecting")?;
    let is_intersecting = webidl::to_boolean(agent, &v);
    let v = get(agent, "rootBounds")?;
    let root_bounds = match v {
        Value::Undefined | Value::Null => None,
        ref r => Some(rect(agent, r)?),
    };
    let v = get(agent, "target")?;
    let Some(target) = unwrap_node(agent, &v) else {
        return agent.type_error(
            "Failed to construct 'IntersectionObserverEntry': required member target is \
             undefined.",
        );
    };
    let v = get(agent, "time")?;
    let time = webidl::to_double(agent, &v)?;
    let target_area = bounding_client_rect.width * bounding_client_rect.height;
    let intersection_ratio = ratio.unwrap_or(if target_area > 0.0 {
        intersection_rect.width * intersection_rect.height / target_area
    } else {
        0.0
    });
    let class = super::classes(agent).intersection_observer_entry;
    let nt = a.new_target.expect("new.target");
    let data = EntryData {
        time,
        root_bounds,
        bounding_client_rect,
        intersection_rect,
        is_intersecting,
        intersection_ratio,
        target,
    };
    let o = agent.host_create_from_new_target(nt, class, None, Box::new(data))?;
    Ok(Value::Object(o))
}

/// Un `DOMRectReadOnly` guardado en el slot `i` (el mismo objeto en cada lectura).
fn cached_rect(agent: &mut Agent, a: &CallArgs<'_>, i: usize, r: RectData) -> JsResult<Value> {
    let Value::Object(o) = a.this else {
        return agent.type_error("Illegal invocation");
    };
    if let Some(v) = agent.host_slots_mut(o).and_then(|s| s.get(i).cloned())
        && !v.is_undefined()
    {
        return Ok(v);
    }
    let class = super::classes(agent).rect_read_only;
    let v = Value::Object(agent.host_create(class, Box::new(r)));
    if let Some(s) = agent.host_slots_mut(o) {
        while s.len() <= i {
            s.push(Value::Undefined);
        }
        s[i] = v.clone();
    }
    Ok(v)
}

fn entry_time(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(entry(agent, a)?.time))
}

fn entry_root_bounds(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    match entry(agent, a)?.root_bounds {
        Some(r) => cached_rect(agent, a, 0, r),
        None => Ok(Value::Null),
    }
}

fn entry_bounding_client_rect(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = entry(agent, a)?.bounding_client_rect;
    cached_rect(agent, a, 1, r)
}

fn entry_intersection_rect(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = entry(agent, a)?.intersection_rect;
    cached_rect(agent, a, 2, r)
}

fn entry_is_intersecting(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(entry(agent, a)?.is_intersecting))
}

fn entry_is_visible(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    entry(agent, a)?;
    Ok(Value::Boolean(false))
}

fn entry_intersection_ratio(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(entry(agent, a)?.intersection_ratio))
}

fn entry_target(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = entry(agent, a)?.target;
    Ok(wrap_node(agent, t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_margin_parsing() {
        assert_eq!(
            serialize_margin(&parse_margin("").unwrap()),
            "0px 0px 0px 0px"
        );
        assert_eq!(
            serialize_margin(&parse_margin("10px 5%").unwrap()),
            "10px 5% 10px 5%"
        );
        assert_eq!(
            serialize_margin(&parse_margin(" 1px 2px 3px ").unwrap()),
            "1px 2px 3px 2px"
        );
        assert_eq!(
            serialize_margin(&parse_margin("1in").unwrap()),
            "96px 96px 96px 96px"
        );
        for bad in [
            "1",
            "1em",
            "auto",
            "1px 2px 3px 4px 5px",
            "1px,2px",
            "calc(1px)",
        ] {
            assert!(parse_margin(bad).is_none(), "{bad}");
        }
    }
}
