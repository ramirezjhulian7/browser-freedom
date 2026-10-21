//! `AbstractRange`, `Range` y `StaticRange` (DOM §5) y `Selection` (Selection API §2).
//! Los `Range` son rangos vivos de `bf-dom` ([`bf_dom::range`]), que los actualiza en cada
//! mutación; aquí están sus algoritmos (comparar boundary points, borrar, extraer, clonar,
//! insertar, rodear). `getBoundingClientRect` usa el último layout (CSSOM View §6.2).

use std::cell::Cell;
use std::cmp::Ordering;

use bf_dom::range::{BoundaryPoint, RangeBounds, RangeId};
use bf_dom::{Document, DomException, NodeData, NodeId};
use bf_js::host::{HostClass, HostClassId, HostKey};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use crate::dom::adopt::adopt_into;
use crate::dom::insertion::{inserted, roots_of};
use crate::dom::state::state;
use crate::dom::{
    arg_node, arg_opt_node, classes, from_dom_string, mutated, this_node, throw_dom, to_dom_string,
    wrap_node,
};
use crate::{DocIndex, NodeRef, with_doc, with_doc_mut};

/// El payload de un `Range`: el rango vivo y su documento (cambia si sus boundary points
/// pasan a otro documento).
#[derive(Debug)]
pub struct RangeData {
    pub doc: Cell<DocIndex>,
    pub id: Cell<RangeId>,
}

/// El payload de un `StaticRange`.
#[derive(Debug, Clone, Copy)]
pub struct StaticRangeData {
    pub doc: DocIndex,
    pub bounds: RangeBounds,
}

pub fn abstract_range_class() -> HostClass {
    HostClass::new("AbstractRange")
        .attribute("startContainer", Some(start_container), None)
        .attribute("startOffset", Some(start_offset), None)
        .attribute("endContainer", Some(end_container), None)
        .attribute("endOffset", Some(end_offset), None)
        .attribute("collapsed", Some(collapsed), None)
}

pub fn static_range_class(abstract_range: HostClassId) -> HostClass {
    HostClass::new("StaticRange")
        .parent(abstract_range)
        .constructor(static_range_ctor, 1)
}

pub fn range_class(abstract_range: HostClassId) -> HostClass {
    HostClass::new("Range")
        .parent(abstract_range)
        .constructor(range_ctor, 0)
        .attribute(
            "commonAncestorContainer",
            Some(common_ancestor_container),
            None,
        )
        .method("setStart", 2, set_start)
        .method("setEnd", 2, set_end)
        .method("setStartBefore", 1, set_start_before)
        .method("setStartAfter", 1, set_start_after)
        .method("setEndBefore", 1, set_end_before)
        .method("setEndAfter", 1, set_end_after)
        .method("collapse", 0, collapse)
        .method("selectNode", 1, select_node)
        .method("selectNodeContents", 1, select_node_contents)
        .method("compareBoundaryPoints", 2, compare_boundary_points)
        .method("deleteContents", 0, delete_contents)
        .method("extractContents", 0, extract_contents)
        .method("cloneContents", 0, clone_contents)
        .method("insertNode", 1, insert_node)
        .method("surroundContents", 1, surround_contents)
        .method("cloneRange", 0, clone_range)
        .method("detach", 0, detach)
        .method("isPointInRange", 2, is_point_in_range)
        .method("comparePoint", 2, compare_point)
        .method("intersectsNode", 1, intersects_node)
        .method("toString", 0, range_to_string)
        .method("getBoundingClientRect", 0, get_bounding_client_rect)
        .method("createContextualFragment", 1, create_contextual_fragment)
        .constant("START_TO_START", 0.0)
        .constant("START_TO_END", 1.0)
        .constant("END_TO_END", 2.0)
        .constant("END_TO_START", 3.0)
}

// --- Boundary points (§5.2) ---

fn bp(node: NodeId, offset: u32) -> BoundaryPoint {
    BoundaryPoint { node, offset }
}

/// ¿`n` es un `CharacterData`?
fn is_char_data(d: &Document, n: NodeId) -> bool {
    matches!(
        d.get(n).data,
        NodeData::Text(_)
            | NodeData::CdataSection(_)
            | NodeData::Comment(_)
            | NodeData::ProcessingInstruction { .. }
    )
}

fn is_text(d: &Document, n: NodeId) -> bool {
    matches!(d.get(n).data, NodeData::Text(_) | NodeData::CdataSection(_))
}

fn is_doctype(d: &Document, n: NodeId) -> bool {
    matches!(d.get(n).data, NodeData::Doctype { .. })
}

/// "The position of a boundary point" (§5.2): `a` respecto de `b`.
fn bp_cmp(d: &Document, a: BoundaryPoint, b: BoundaryPoint) -> Ordering {
    if a.node == b.node {
        return a.offset.cmp(&b.offset);
    }
    // 3. Si a.node sigue a b.node, el resultado es el inverso.
    if d.compare_document_position(a.node, b.node) & bf_dom::document_position::PRECEDING != 0 {
        return bp_cmp(d, b, a).reverse();
    }
    // 4. a.node es ancestro de b.node.
    if d.is_inclusive_ancestor(a.node, b.node) {
        let mut child = b.node;
        while let Some(p) = d.parent(child) {
            if p == a.node {
                break;
            }
            child = p;
        }
        if (d.index(child) as u32) < a.offset {
            return Ordering::Greater;
        }
    }
    Ordering::Less
}

fn same_root(d: &Document, a: NodeId, b: NodeId) -> bool {
    d.get_root_node(a) == d.get_root_node(b)
}

/// "Contained" (§5.5).
fn contained(d: &Document, r: RangeBounds, n: NodeId) -> bool {
    same_root(d, n, r.start.node)
        && bp_cmp(d, bp(n, 0), r.start) == Ordering::Greater
        && bp_cmp(d, bp(n, d.node_length(n) as u32), r.end) == Ordering::Less
}

/// "Partially contained" (§5.5).
fn partially_contained(d: &Document, r: RangeBounds, n: NodeId) -> bool {
    d.is_inclusive_ancestor(n, r.start.node) != d.is_inclusive_ancestor(n, r.end.node)
}

fn common_ancestor(d: &Document, r: RangeBounds) -> NodeId {
    let mut c = r.start.node;
    while !d.is_inclusive_ancestor(c, r.end.node) {
        match d.parent(c) {
            Some(p) => c = p,
            None => break,
        }
    }
    c
}

// --- Acceso al rango de `this` ---

fn this_range(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<(DocIndex, RangeId)> {
    match agent.host_unwrap::<RangeData>(&a.this) {
        Some(r) => Ok((r.doc.get(), r.id.get())),
        None => agent.type_error("Illegal invocation"),
    }
}

/// Los boundary points de un `AbstractRange` (vivo o estático).
fn abstract_bounds(agent: &mut Agent, v: &Value) -> JsResult<(DocIndex, RangeBounds)> {
    if let Some(r) = agent.host_unwrap::<RangeData>(v) {
        let (doc, id) = (r.doc.get(), r.id.get());
        return Ok((doc, with_doc(agent, doc, |d| d.live_range(id))));
    }
    if let Some(s) = agent.host_unwrap::<StaticRangeData>(v) {
        return Ok((s.doc, s.bounds));
    }
    agent.type_error("Illegal invocation")
}

fn range_bounds(agent: &Agent, doc: DocIndex, id: RangeId) -> RangeBounds {
    with_doc(agent, doc, |d| d.live_range(id))
}

/// Pone los boundary points del rango `v` (en el documento `doc`, que puede ser otro).
fn store(agent: &mut Agent, v: &Value, doc: DocIndex, bounds: RangeBounds) {
    let Some(r) = agent.host_unwrap::<RangeData>(v) else {
        return;
    };
    let (old_doc, id) = (r.doc.get(), r.id.get());
    if old_doc == doc {
        with_doc_mut(agent, doc, |d| d.set_live_range(id, bounds));
        return;
    }
    with_doc_mut(agent, old_doc, |d| d.drop_live_range(id));
    let new_id = with_doc_mut(agent, doc, |d| d.create_live_range(bounds));
    if let Some(r) = agent.host_unwrap::<RangeData>(v) {
        r.doc.set(doc);
        r.id.set(new_id);
    }
}

/// Un `Range` nuevo con `bounds` en `doc`.
pub fn new_range(agent: &mut Agent, doc: DocIndex, bounds: RangeBounds) -> Value {
    let id = with_doc_mut(agent, doc, |d| d.create_live_range(bounds));
    let class = classes(agent).range;
    Value::Object(agent.host_create(
        class,
        Box::new(RangeData {
            doc: Cell::new(doc),
            id: Cell::new(id),
        }),
    ))
}

/// Un rango colapsado al inicio del documento `doc` (`new Range()`, `createRange()`).
pub fn document_range(agent: &mut Agent, doc: DocIndex) -> Value {
    let root = with_doc(agent, doc, |d| d.root());
    new_range(
        agent,
        doc,
        RangeBounds {
            start: bp(root, 0),
            end: bp(root, 0),
        },
    )
}

fn range_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let root = with_doc(agent, 0, |d| d.root());
    let bounds = RangeBounds {
        start: bp(root, 0),
        end: bp(root, 0),
    };
    let id = with_doc_mut(agent, 0, |d| d.create_live_range(bounds));
    let class = classes(agent).range;
    let nt = a.new_target.expect("new.target");
    let o = agent.host_create_from_new_target(
        nt,
        class,
        None,
        Box::new(RangeData {
            doc: Cell::new(0),
            id: Cell::new(id),
        }),
    )?;
    Ok(Value::Object(o))
}

/// `new StaticRange(init)` (§5.4).
fn static_range_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let dict = a.arg(0);
    let Value::Object(o) = dict else {
        return agent.type_error("Failed to construct 'StaticRange': 1 argument required.");
    };
    let get = |agent: &mut Agent, name: &str| agent.get(o, &PropertyKey::from(name), dict.clone());
    // Los miembros en orden lexicográfico.
    let end_container = get(agent, "endContainer")?;
    let end_c = arg_node(agent, &end_container, "endContainer")?;
    let end_offset = get(agent, "endOffset")?;
    let end_o = bf_js::host::webidl::to_unsigned_long(agent, &end_offset)?;
    let start_container = get(agent, "startContainer")?;
    let start_c = arg_node(agent, &start_container, "startContainer")?;
    let start_offset = get(agent, "startOffset")?;
    let start_o = bf_js::host::webidl::to_unsigned_long(agent, &start_offset)?;
    for n in [start_c, end_c] {
        let bad = with_doc(agent, n.doc, |d| {
            matches!(
                d.get(n.node).data,
                NodeData::Doctype { .. } | NodeData::Attr(_)
            )
        });
        if bad {
            return throw_dom(agent, DomException::InvalidNodeTypeError);
        }
    }
    let class = classes(agent).static_range;
    let nt = a.new_target.expect("new.target");
    // Un StaticRange puede tener extremos de documentos distintos; se guarda el del inicio
    // (el final de otro documento se ve como su nodo: el wrapper usa su propio `NodeRef`).
    let data = StaticRangeData {
        doc: start_c.doc,
        bounds: RangeBounds {
            start: bp(start_c.node, start_o),
            end: bp(end_c.node, end_o),
        },
    };
    let o = agent.host_create_from_new_target(nt, class, None, Box::new(data))?;
    if end_c.doc != start_c.doc
        && let Some(s) = agent.host_slots_mut(o)
    {
        s.push(end_container);
    }
    Ok(Value::Object(o))
}

// --- AbstractRange ---

fn start_container(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, r) = abstract_bounds(agent, &a.this)?;
    Ok(wrap_node(agent, NodeRef::new(doc, r.start.node)))
}

fn start_offset(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (_, r) = abstract_bounds(agent, &a.this)?;
    Ok(Value::Number(f64::from(r.start.offset)))
}

fn end_container(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    // Un StaticRange con el final en otro documento lo guardó en el slot 0.
    if let Value::Object(o) = a.this
        && agent.host_unwrap::<StaticRangeData>(&a.this).is_some()
        && let Some(v) = agent.host_slots_mut(o).and_then(|s| s.first().cloned())
    {
        return Ok(v);
    }
    let (doc, r) = abstract_bounds(agent, &a.this)?;
    Ok(wrap_node(agent, NodeRef::new(doc, r.end.node)))
}

fn end_offset(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (_, r) = abstract_bounds(agent, &a.this)?;
    Ok(Value::Number(f64::from(r.end.offset)))
}

fn collapsed(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (_, r) = abstract_bounds(agent, &a.this)?;
    Ok(Value::Boolean(r.start == r.end))
}

// --- Range ---

fn common_ancestor_container(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, id) = this_range(agent, a)?;
    let r = range_bounds(agent, doc, id);
    let c = with_doc(agent, doc, |d| common_ancestor(d, r));
    Ok(wrap_node(agent, NodeRef::new(doc, c)))
}

/// "Set the start or end" (§5.5).
fn set_boundary(
    agent: &mut Agent,
    this: &Value,
    n: NodeRef,
    offset: u32,
    start: bool,
) -> JsResult<()> {
    let (len, doctype) = with_doc(agent, n.doc, |d| {
        (d.node_length(n.node) as u32, is_doctype(d, n.node))
    });
    if doctype {
        return throw_dom(agent, DomException::InvalidNodeTypeError);
    }
    if offset > len {
        return throw_dom(agent, DomException::IndexSizeError);
    }
    let Some(rd) = agent.host_unwrap::<RangeData>(this) else {
        return agent.type_error("Illegal invocation");
    };
    let (doc, id) = (rd.doc.get(), rd.id.get());
    let mut r = range_bounds(agent, doc, id);
    let point = bp(n.node, offset);
    let other_root = doc != n.doc || with_doc(agent, doc, |d| !same_root(d, r.start.node, n.node));
    if start {
        if other_root
            || with_doc(agent, n.doc, |d| {
                bp_cmp(d, point, r.end) == Ordering::Greater
            })
        {
            r.end = point;
        }
        r.start = point;
    } else {
        if other_root
            || with_doc(agent, n.doc, |d| {
                bp_cmp(d, point, r.start) == Ordering::Less
            })
        {
            r.start = point;
        }
        r.end = point;
    }
    store(agent, this, n.doc, r);
    Ok(())
}

fn set_start(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_range(agent, a)?;
    let n = arg_node(agent, &a.arg(0), "parameter 1")?;
    let off = bf_js::host::webidl::to_unsigned_long(agent, &a.arg(1))?;
    set_boundary(agent, &a.this, n, off, true)?;
    Ok(Value::Undefined)
}

fn set_end(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_range(agent, a)?;
    let n = arg_node(agent, &a.arg(0), "parameter 1")?;
    let off = bf_js::host::webidl::to_unsigned_long(agent, &a.arg(1))?;
    set_boundary(agent, &a.this, n, off, false)?;
    Ok(Value::Undefined)
}

/// El padre de `n` y su índice (InvalidNodeTypeError sin padre).
fn parent_and_index(agent: &mut Agent, n: NodeRef) -> JsResult<(NodeRef, u32)> {
    let r = with_doc(agent, n.doc, |d| {
        d.parent(n.node).map(|p| (p, d.index(n.node) as u32))
    });
    match r {
        Some((p, i)) => Ok((NodeRef::new(n.doc, p), i)),
        None => throw_dom(agent, DomException::InvalidNodeTypeError),
    }
}

fn set_relative(agent: &mut Agent, a: &CallArgs<'_>, start: bool, after: bool) -> JsResult<Value> {
    this_range(agent, a)?;
    let n = arg_node(agent, &a.arg(0), "parameter 1")?;
    let (p, i) = parent_and_index(agent, n)?;
    set_boundary(agent, &a.this, p, i + u32::from(after), start)?;
    Ok(Value::Undefined)
}

fn set_start_before(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    set_relative(agent, a, true, false)
}

fn set_start_after(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    set_relative(agent, a, true, true)
}

fn set_end_before(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    set_relative(agent, a, false, false)
}

fn set_end_after(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    set_relative(agent, a, false, true)
}

fn collapse(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, id) = this_range(agent, a)?;
    let to_start = bf_js::host::webidl::to_boolean(agent, &a.arg(0));
    let mut r = range_bounds(agent, doc, id);
    if to_start {
        r.end = r.start;
    } else {
        r.start = r.end;
    }
    store(agent, &a.this, doc, r);
    Ok(Value::Undefined)
}

/// "Select" (§5.5).
fn select(agent: &mut Agent, this: &Value, n: NodeRef) -> JsResult<()> {
    let (p, i) = parent_and_index(agent, n)?;
    store(
        agent,
        this,
        n.doc,
        RangeBounds {
            start: bp(p.node, i),
            end: bp(p.node, i + 1),
        },
    );
    Ok(())
}

fn select_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_range(agent, a)?;
    let n = arg_node(agent, &a.arg(0), "parameter 1")?;
    select(agent, &a.this, n)?;
    Ok(Value::Undefined)
}

fn select_contents(agent: &mut Agent, this: &Value, n: NodeRef) -> JsResult<()> {
    let (len, doctype) = with_doc(agent, n.doc, |d| {
        (d.node_length(n.node) as u32, is_doctype(d, n.node))
    });
    if doctype {
        return throw_dom(agent, DomException::InvalidNodeTypeError);
    }
    store(
        agent,
        this,
        n.doc,
        RangeBounds {
            start: bp(n.node, 0),
            end: bp(n.node, len),
        },
    );
    Ok(())
}

fn select_node_contents(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_range(agent, a)?;
    let n = arg_node(agent, &a.arg(0), "parameter 1")?;
    select_contents(agent, &a.this, n)?;
    Ok(Value::Undefined)
}

fn ord_value(o: Ordering) -> Value {
    Value::Number(match o {
        Ordering::Less => -1.0,
        Ordering::Equal => 0.0,
        Ordering::Greater => 1.0,
    })
}

/// `compareBoundaryPoints(how, sourceRange)` (§5.5).
fn compare_boundary_points(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, id) = this_range(agent, a)?;
    let how = bf_js::host::webidl::to_unsigned_short(agent, &a.arg(0))?;
    let Some(src) = agent.host_unwrap::<RangeData>(&a.arg(1)) else {
        return agent.type_error("parameter 2 is not of type 'Range'.");
    };
    let (sdoc, sid) = (src.doc.get(), src.id.get());
    if how > 3 {
        return throw_dom(agent, DomException::NotSupportedError);
    }
    let r = range_bounds(agent, doc, id);
    let s = range_bounds(agent, sdoc, sid);
    if doc != sdoc || with_doc(agent, doc, |d| !same_root(d, r.start.node, s.start.node)) {
        return throw_dom(agent, DomException::WrongDocumentError);
    }
    let (this_point, other_point) = match how {
        0 => (r.start, s.start),
        1 => (r.end, s.start),
        2 => (r.end, s.end),
        _ => (r.start, s.end),
    };
    Ok(ord_value(with_doc(agent, doc, |d| {
        bp_cmp(d, this_point, other_point)
    })))
}

/// "Delete the contents" (§5.5).
fn delete(agent: &mut Agent, doc: DocIndex, this: &Value) -> JsResult<()> {
    let Some(rd) = agent.host_unwrap::<RangeData>(this) else {
        return Ok(());
    };
    let id = rd.id.get();
    let r = range_bounds(agent, doc, id);
    if r.start == r.end {
        return Ok(());
    }
    let (s, e) = (r.start, r.end);
    let simple = with_doc(agent, doc, |d| s.node == e.node && is_char_data(d, s.node));
    if simple {
        let res = with_doc_mut(agent, doc, |d| {
            d.replace_data(
                s.node,
                s.offset as usize,
                (e.offset - s.offset) as usize,
                &"".into(),
            )
        });
        mutated(agent);
        return res.or_else(|err| throw_dom(agent, err));
    }
    let (to_remove, new_point) = with_doc(agent, doc, |d| {
        let common = common_ancestor(d, r);
        let nodes: Vec<NodeId> = d
            .descendants(common)
            .into_iter()
            .filter(|&n| contained(d, r, n) && d.parent(n).is_none_or(|p| !contained(d, r, p)))
            .collect();
        let point = if d.is_inclusive_ancestor(s.node, e.node) {
            s
        } else {
            let mut reference = s.node;
            while let Some(p) = d.parent(reference) {
                if d.is_inclusive_ancestor(p, e.node) {
                    break;
                }
                reference = p;
            }
            let parent = d.parent(reference).unwrap_or(reference);
            bp(parent, d.index(reference) as u32 + 1)
        };
        (nodes, point)
    });
    with_doc_mut(agent, doc, |d| {
        if is_char_data(d, s.node) {
            let len = d.node_length(s.node);
            let _ = d.replace_data(
                s.node,
                s.offset as usize,
                len - s.offset as usize,
                &"".into(),
            );
        }
        for n in to_remove {
            d.remove(n);
        }
        if is_char_data(d, e.node) {
            let _ = d.replace_data(e.node, 0, e.offset as usize, &"".into());
        }
    });
    mutated(agent);
    store(
        agent,
        this,
        doc,
        RangeBounds {
            start: new_point,
            end: new_point,
        },
    );
    Ok(())
}

fn delete_contents(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, _) = this_range(agent, a)?;
    delete(agent, doc, &a.this)?;
    Ok(Value::Undefined)
}

/// Un clon de `n` (un `CharacterData`) con los datos `[from, to)`.
fn clone_char_data(d: &mut Document, n: NodeId, from: u32, to: u32) -> NodeId {
    let data = d
        .substring_data(n, from as usize, (to - from) as usize)
        .unwrap_or_default();
    let c = d.clone_node(n, false);
    d.set_data(c, &data);
    c
}

/// "Extract" (`extract = true`) o "clone the contents" (§5.5) de `r` en un fragmento nuevo;
/// en la extracción devuelve también el punto donde colapsa el rango.
fn extract_or_clone(
    d: &mut Document,
    r: RangeBounds,
    extract: bool,
) -> Result<(NodeId, Option<BoundaryPoint>), DomException> {
    let fragment = d.create_document_fragment();
    if r.start == r.end {
        return Ok((fragment, None));
    }
    let (s, e) = (r.start, r.end);
    // 4.
    if s.node == e.node && is_char_data(d, s.node) {
        let c = clone_char_data(d, s.node, s.offset, e.offset);
        d.append(fragment, c);
        if extract {
            d.replace_data(
                s.node,
                s.offset as usize,
                (e.offset - s.offset) as usize,
                &"".into(),
            )?;
        }
        return Ok((fragment, Some(s)));
    }
    // 5-9.
    let common = common_ancestor(d, r);
    let children: Vec<NodeId> = d.child_nodes(common);
    let first_partial = if d.is_inclusive_ancestor(s.node, e.node) {
        None
    } else {
        children
            .iter()
            .copied()
            .find(|&c| partially_contained(d, r, c))
    };
    let last_partial = if d.is_inclusive_ancestor(e.node, s.node) {
        None
    } else {
        children
            .iter()
            .rev()
            .copied()
            .find(|&c| partially_contained(d, r, c))
    };
    let contained_children: Vec<NodeId> = children
        .iter()
        .copied()
        .filter(|&c| contained(d, r, c))
        .collect();
    // 10.
    if contained_children.iter().any(|&c| is_doctype(d, c)) {
        return Err(DomException::HierarchyRequestError);
    }
    // 11-12.
    let new_point = if d.is_inclusive_ancestor(s.node, e.node) {
        s
    } else {
        let mut reference = s.node;
        while let Some(p) = d.parent(reference) {
            if d.is_inclusive_ancestor(p, e.node) {
                break;
            }
            reference = p;
        }
        bp(
            d.parent(reference).unwrap_or(reference),
            d.index(reference) as u32 + 1,
        )
    };
    // 13-14.
    if let Some(first) = first_partial {
        if is_char_data(d, first) {
            let len = d.node_length(s.node) as u32;
            let c = clone_char_data(d, s.node, s.offset, len);
            d.append(fragment, c);
            if extract {
                d.replace_data(
                    s.node,
                    s.offset as usize,
                    (len - s.offset) as usize,
                    &"".into(),
                )?;
            }
        } else {
            let c = d.clone_node(first, false);
            d.append(fragment, c);
            let sub = RangeBounds {
                start: s,
                end: bp(first, d.node_length(first) as u32),
            };
            let (subfragment, _) = extract_or_clone(d, sub, extract)?;
            d.append(c, subfragment);
        }
    }
    // 15.
    for c in contained_children {
        if extract {
            d.append(fragment, c);
        } else {
            let copy = d.clone_node(c, true);
            d.append(fragment, copy);
        }
    }
    // 16-17.
    if let Some(last) = last_partial {
        if is_char_data(d, last) {
            let c = clone_char_data(d, e.node, 0, e.offset);
            d.append(fragment, c);
            if extract {
                d.replace_data(e.node, 0, e.offset as usize, &"".into())?;
            }
        } else {
            let c = d.clone_node(last, false);
            d.append(fragment, c);
            let sub = RangeBounds {
                start: bp(last, 0),
                end: e,
            };
            let (subfragment, _) = extract_or_clone(d, sub, extract)?;
            d.append(c, subfragment);
        }
    }
    Ok((fragment, Some(new_point)))
}

fn extract_contents(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, id) = this_range(agent, a)?;
    let r = range_bounds(agent, doc, id);
    let res = with_doc_mut(agent, doc, |d| extract_or_clone(d, r, true));
    match res {
        Ok((fragment, point)) => {
            mutated(agent);
            if let Some(p) = point {
                store(agent, &a.this, doc, RangeBounds { start: p, end: p });
            }
            Ok(wrap_node(agent, NodeRef::new(doc, fragment)))
        }
        Err(e) => throw_dom(agent, e),
    }
}

fn clone_contents(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, id) = this_range(agent, a)?;
    let r = range_bounds(agent, doc, id);
    let res = with_doc_mut(agent, doc, |d| extract_or_clone(d, r, false));
    match res {
        Ok((fragment, _)) => Ok(wrap_node(agent, NodeRef::new(doc, fragment))),
        Err(e) => throw_dom(agent, e),
    }
}

/// "Insert" un nodo en un rango (§5.5).
fn insert_into(agent: &mut Agent, this: &Value, doc: DocIndex, node: NodeRef) -> JsResult<()> {
    let Some(rd) = agent.host_unwrap::<RangeData>(this) else {
        return Ok(());
    };
    let id = rd.id.get();
    let r = range_bounds(agent, doc, id);
    let s = r.start;
    // 1.
    let bad = (node.doc == doc && s.node == node.node)
        || with_doc(agent, doc, |d| {
            matches!(
                d.get(s.node).data,
                NodeData::ProcessingInstruction { .. } | NodeData::Comment(_)
            ) || (is_text(d, s.node) && d.parent(s.node).is_none())
        });
    if bad {
        return throw_dom(agent, DomException::HierarchyRequestError);
    }
    // 2-5.
    let (reference, parent) = with_doc(agent, doc, |d| {
        let reference = if is_text(d, s.node) {
            Some(s.node)
        } else {
            d.child_nodes(s.node).get(s.offset as usize).copied()
        };
        let parent = match reference {
            Some(r) => d.parent(r).unwrap_or(s.node),
            None => s.node,
        };
        (reference, parent)
    });
    // 6. "Ensure pre-insertion validity" (los nodos de otro documento se adoptan antes).
    let node = if node.doc == doc {
        let r = with_doc(agent, doc, |d| {
            d.ensure_pre_insertion_validity(node.node, parent, reference)
        });
        if let Err(e) = r {
            return throw_dom(agent, e);
        }
        node
    } else {
        crate::dom::adopt::prepare_insert(
            agent,
            NodeRef::new(doc, parent),
            node,
            reference.map(|r| NodeRef::new(doc, r)),
        )?
    };
    // 7.
    let mut reference = reference;
    if with_doc(agent, doc, |d| is_text(d, s.node)) {
        let split = with_doc_mut(agent, doc, |d| d.split_text(s.node, s.offset as usize));
        match split {
            Ok(n) => reference = Some(n),
            Err(e) => return throw_dom(agent, e),
        }
    }
    // 8.
    if reference == Some(node.node) {
        reference = with_doc(agent, doc, |d| d.next_sibling(node.node));
    }
    // 9.
    with_doc_mut(agent, doc, |d| {
        if d.parent(node.node).is_some() {
            d.remove(node.node);
        }
    });
    // 10-11.
    let new_offset = with_doc(agent, doc, |d| {
        let base = match reference {
            Some(r) => d.index(r) as u32,
            None => d.node_length(parent) as u32,
        };
        base + match d.get(node.node).data {
            NodeData::DocumentFragment => d.node_length(node.node) as u32,
            _ => 1,
        }
    });
    // 12.
    let roots = roots_of(agent, doc, node.node);
    let res = with_doc_mut(agent, doc, |d| {
        d.insert_before_checked(parent, node.node, reference)
    });
    if let Err(e) = res {
        return throw_dom(agent, e);
    }
    mutated(agent);
    inserted(agent, doc, &roots);
    // 13.
    let r = range_bounds(agent, doc, id);
    if r.start == r.end {
        store(
            agent,
            this,
            doc,
            RangeBounds {
                start: r.start,
                end: bp(parent, new_offset),
            },
        );
    }
    Ok(())
}

fn insert_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, _) = this_range(agent, a)?;
    let n = arg_node(agent, &a.arg(0), "parameter 1")?;
    insert_into(agent, &a.this, doc, n)?;
    Ok(Value::Undefined)
}

/// `surroundContents(newParent)` (§5.5).
fn surround_contents(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, id) = this_range(agent, a)?;
    let new_parent = arg_node(agent, &a.arg(0), "parameter 1")?;
    let r = range_bounds(agent, doc, id);
    // 1.
    let bad = with_doc(agent, doc, |d| {
        d.descendants(d.get_root_node(r.start.node))
            .into_iter()
            .any(|n| !is_text(d, n) && partially_contained(d, r, n))
    });
    if bad {
        return throw_dom(agent, DomException::InvalidStateError);
    }
    // 2.
    let bad_type = with_doc(agent, new_parent.doc, |d| {
        matches!(
            d.get(new_parent.node).data,
            NodeData::Document | NodeData::Doctype { .. } | NodeData::DocumentFragment
        )
    });
    if bad_type {
        return throw_dom(agent, DomException::InvalidNodeTypeError);
    }
    // 3.
    let res = with_doc_mut(agent, doc, |d| extract_or_clone(d, r, true));
    let fragment = match res {
        Ok((f, point)) => {
            if let Some(p) = point {
                store(agent, &a.this, doc, RangeBounds { start: p, end: p });
            }
            f
        }
        Err(e) => return throw_dom(agent, e),
    };
    // 4.
    let new_parent = adopt_into(agent, new_parent, doc)?;
    with_doc_mut(agent, doc, |d| d.replace_all(None, new_parent.node));
    // 5.
    insert_into(agent, &a.this, doc, new_parent)?;
    // 6.
    let res = with_doc_mut(agent, doc, |d| d.append_child(new_parent.node, fragment));
    if let Err(e) = res {
        return throw_dom(agent, e);
    }
    // 7.
    select(agent, &a.this, new_parent)?;
    mutated(agent);
    Ok(Value::Undefined)
}

fn clone_range(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, id) = this_range(agent, a)?;
    let r = range_bounds(agent, doc, id);
    Ok(new_range(agent, doc, r))
}

fn detach(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_range(agent, a)?;
    Ok(Value::Undefined)
}

/// Los chequeos de `isPointInRange`/`comparePoint`: `None` si las raíces difieren.
fn point_checks(
    agent: &mut Agent,
    a: &CallArgs<'_>,
) -> JsResult<(DocIndex, RangeBounds, Option<BoundaryPoint>)> {
    let (doc, id) = this_range(agent, a)?;
    let n = arg_node(agent, &a.arg(0), "parameter 1")?;
    let offset = bf_js::host::webidl::to_unsigned_long(agent, &a.arg(1))?;
    let r = range_bounds(agent, doc, id);
    if n.doc != doc || with_doc(agent, doc, |d| !same_root(d, n.node, r.start.node)) {
        return Ok((doc, r, None));
    }
    let (doctype, len) = with_doc(agent, doc, |d| {
        (is_doctype(d, n.node), d.node_length(n.node) as u32)
    });
    if doctype {
        return throw_dom(agent, DomException::InvalidNodeTypeError);
    }
    if offset > len {
        return throw_dom(agent, DomException::IndexSizeError);
    }
    Ok((doc, r, Some(bp(n.node, offset))))
}

fn is_point_in_range(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, r, p) = point_checks(agent, a)?;
    let Some(p) = p else {
        return Ok(Value::Boolean(false));
    };
    let inside = with_doc(agent, doc, |d| {
        bp_cmp(d, p, r.start) != Ordering::Less && bp_cmp(d, p, r.end) != Ordering::Greater
    });
    Ok(Value::Boolean(inside))
}

fn compare_point(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, r, p) = point_checks(agent, a)?;
    let Some(p) = p else {
        return throw_dom(agent, DomException::WrongDocumentError);
    };
    let o = with_doc(agent, doc, |d| {
        if bp_cmp(d, p, r.start) == Ordering::Less {
            Ordering::Less
        } else if bp_cmp(d, p, r.end) == Ordering::Greater {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    });
    Ok(ord_value(o))
}

fn intersects_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, id) = this_range(agent, a)?;
    let n = arg_node(agent, &a.arg(0), "parameter 1")?;
    let r = range_bounds(agent, doc, id);
    if n.doc != doc {
        return Ok(Value::Boolean(false));
    }
    let res = with_doc(agent, doc, |d| {
        if !same_root(d, n.node, r.start.node) {
            return false;
        }
        let Some(p) = d.parent(n.node) else {
            return true;
        };
        let i = d.index(n.node) as u32;
        bp_cmp(d, bp(p, i), r.end) == Ordering::Less
            && bp_cmp(d, bp(p, i + 1), r.start) == Ordering::Greater
    });
    Ok(Value::Boolean(res))
}

/// El texto de un rango (§5.5 "stringification behavior").
fn stringify(d: &Document, r: RangeBounds) -> bf_infra::DomString {
    let (s, e) = (r.start, r.end);
    if s.node == e.node && is_text(d, s.node) {
        return d
            .substring_data(s.node, s.offset as usize, (e.offset - s.offset) as usize)
            .unwrap_or_default();
    }
    let mut out = bf_infra::DomString::new();
    if is_text(d, s.node) {
        let len = d.node_length(s.node);
        if let Ok(t) = d.substring_data(s.node, s.offset as usize, len - s.offset as usize) {
            out.push_dom_str(&t);
        }
    }
    for n in d.descendants(d.get_root_node(s.node)) {
        if is_text(d, n) && contained(d, r, n) {
            out.push_dom_str(&d.data(n));
        }
    }
    if is_text(d, e.node)
        && let Ok(t) = d.substring_data(e.node, 0, e.offset as usize)
    {
        out.push_dom_str(&t);
    }
    out
}

fn range_to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, id) = this_range(agent, a)?;
    let r = range_bounds(agent, doc, id);
    let s = with_doc(agent, doc, |d| stringify(d, r));
    Ok(from_dom_string(&s))
}

/// `getBoundingClientRect()` (CSSOM View §6.2): la unión de las cajas de los nodos del rango
/// en el último layout (solo el documento de la ventana tiene layout).
fn get_bounding_client_rect(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, id) = this_range(agent, a)?;
    let r = range_bounds(agent, doc, id);
    let mut rects = Vec::new();
    if doc == 0 {
        let nodes: Vec<NodeId> = with_doc(agent, doc, |d| {
            let mut v: Vec<NodeId> = d
                .descendants(common_ancestor(d, r))
                .into_iter()
                .filter(|&n| contained(d, r, n) || (is_text(d, n) && partially_contained(d, r, n)))
                .collect();
            for n in [r.start.node, r.end.node] {
                if is_text(d, n) && !v.contains(&n) {
                    v.push(n);
                }
            }
            v
        });
        let tree = crate::cssom::current_layout(agent);
        fn walk(f: &bf_layout::Fragment, nodes: &[NodeId], out: &mut Vec<bf_layout::Rect>) {
            if let Some(n) = f.node
                && f.pseudo.is_none()
                && nodes.contains(&n)
                && (f.rect.width > 0.0 || f.rect.height > 0.0)
            {
                out.push(f.rect);
            }
            for c in &f.children {
                walk(c, nodes, out);
            }
        }
        if let Some(root) = &tree.root {
            walk(root, &nodes, &mut rects);
        }
    }
    let data = rects.iter().copied().reduce(|acc, r| {
        let (x0, y0) = (acc.x.min(r.x), acc.y.min(r.y));
        let (x1, y1) = (acc.right().max(r.right()), acc.bottom().max(r.bottom()));
        bf_layout::Rect::new(x0, y0, x1 - x0, y1 - y0)
    });
    let rect = data.map_or_else(crate::cssom::view::RectData::default, |r| {
        crate::cssom::view::RectData {
            x: f64::from(r.x),
            y: f64::from(r.y),
            width: f64::from(r.width),
            height: f64::from(r.height),
        }
    });
    Ok(crate::cssom::view::new_rect(agent, rect))
}

/// `createContextualFragment(string)` (DOM Parsing §7.2): el fragmento parseado con el
/// elemento del inicio como contexto (o un `body`). Sus scripts se ejecutan al insertarlos.
fn create_contextual_fragment(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, id) = this_range(agent, a)?;
    let markup = to_dom_string(agent, &a.arg(0))?;
    let r = range_bounds(agent, doc, id);
    let fragment = with_doc_mut(agent, doc, |d| {
        let node = r.start.node;
        let element = match &d.get(node).data {
            NodeData::Element(_) => Some(node),
            NodeData::Text(_) | NodeData::CdataSection(_) | NodeData::Comment(_) => {
                d.parent_element(node)
            }
            _ => None,
        };
        let element =
            element.filter(|&e| !(d.is_html && d.element(e).is_some_and(|x| x.is_html("html"))));
        let context = match element {
            Some(e) => e,
            None => d.create_element(&"body".into()).unwrap_or(node),
        };
        bf_html::dom::parse_fragment_into(d, context, &markup)
    });
    Ok(wrap_node(agent, NodeRef::new(doc, fragment)))
}

// --- Selection (Selection API §2) ---

/// La dirección de una selección.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    #[default]
    None,
    Forwards,
    Backwards,
}

/// El estado de la selección de un documento: su `Range` (retenido) y la dirección.
#[derive(Debug, Default, Clone, Copy)]
pub struct SelectionState {
    pub range: Option<bf_js::host::HostHandle>,
    pub direction: Direction,
}

/// El payload de un `Selection`: su documento.
#[derive(Debug, Clone, Copy)]
pub struct SelectionData {
    pub doc: DocIndex,
}

pub fn selection_class() -> HostClass {
    HostClass::new("Selection")
        .attribute("anchorNode", Some(anchor_node), None)
        .attribute("anchorOffset", Some(anchor_offset), None)
        .attribute("focusNode", Some(focus_node), None)
        .attribute("focusOffset", Some(focus_offset), None)
        .attribute("isCollapsed", Some(sel_is_collapsed), None)
        .attribute("rangeCount", Some(range_count), None)
        .attribute("type", Some(sel_type), None)
        .attribute("direction", Some(sel_direction), None)
        .method("getRangeAt", 1, get_range_at)
        .method("addRange", 1, add_range)
        .method("removeRange", 1, remove_range)
        .method("removeAllRanges", 0, remove_all_ranges)
        .method("empty", 0, remove_all_ranges)
        .method("collapse", 1, sel_collapse)
        .method("setPosition", 1, sel_collapse)
        .method("collapseToStart", 0, collapse_to_start)
        .method("collapseToEnd", 0, collapse_to_end)
        .method("extend", 1, extend)
        .method("setBaseAndExtent", 4, set_base_and_extent)
        .method("selectAllChildren", 1, select_all_children)
        .method("deleteFromDocument", 0, delete_from_document)
        .method("containsNode", 1, contains_node)
        .method("toString", 0, sel_to_string)
}

/// `getSelection()` del documento `doc` (siempre el mismo objeto).
pub fn selection_of(agent: &mut Agent, doc: DocIndex) -> Value {
    let realm = agent.current_realm;
    let key = HostKey((1 << 57) | u64::from(doc));
    if let Some(o) = agent.host_lookup(realm, key) {
        return Value::Object(o);
    }
    let class = classes(agent).selection;
    Value::Object(agent.host_wrap(realm, key, class, Box::new(SelectionData { doc })))
}

/// `Window.getSelection()`.
pub fn window_get_selection(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(selection_of(agent, 0))
}

/// `Document.getSelection()`: `null` en los documentos sin ventana.
pub fn document_get_selection(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    if n.doc != 0 {
        return Ok(Value::Null);
    }
    Ok(selection_of(agent, 0))
}

/// `Document.createRange()`.
pub fn document_create_range(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(document_range(agent, n.doc))
}

fn this_sel(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<DocIndex> {
    match agent.host_unwrap::<SelectionData>(&a.this) {
        Some(s) => Ok(s.doc),
        None => agent.type_error("Illegal invocation"),
    }
}

fn sel_state(agent: &mut Agent, doc: DocIndex) -> SelectionState {
    state(agent)
        .selections
        .get(&doc)
        .copied()
        .unwrap_or_default()
}

/// El `Range` de la selección (el wrapper) y sus boundary points.
fn sel_range(agent: &mut Agent, doc: DocIndex) -> Option<(Value, DocIndex, RangeBounds)> {
    let st = sel_state(agent, doc);
    let v = agent.host_pinned(st.range?)?;
    let rd = agent.host_unwrap::<RangeData>(&v)?;
    let (rdoc, id) = (rd.doc.get(), rd.id.get());
    let b = range_bounds(agent, rdoc, id);
    Some((v, rdoc, b))
}

fn set_sel(agent: &mut Agent, doc: DocIndex, range: Option<Value>, dir: Direction) {
    let old = state(agent).selections.remove(&doc);
    if let Some(h) = old.and_then(|s| s.range) {
        agent.host_unpin(h);
    }
    let h = range.map(|r| agent.host_pin(r));
    state(agent).selections.insert(
        doc,
        SelectionState {
            range: h,
            direction: dir,
        },
    );
}

/// El anchor (`true`) o el focus de la selección.
fn sel_point(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    anchor: bool,
) -> JsResult<Option<(DocIndex, BoundaryPoint)>> {
    let doc = this_sel(agent, a)?;
    let dir = sel_state(agent, doc).direction;
    let Some((_, rdoc, b)) = sel_range(agent, doc) else {
        return Ok(None);
    };
    let use_start = (dir != Direction::Backwards) == anchor;
    Ok(Some((rdoc, if use_start { b.start } else { b.end })))
}

fn anchor_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(match sel_point(agent, a, true)? {
        Some((d, p)) => wrap_node(agent, NodeRef::new(d, p.node)),
        None => Value::Null,
    })
}

fn anchor_offset(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(
        sel_point(agent, a, true)?.map_or(0.0, |(_, p)| f64::from(p.offset)),
    ))
}

fn focus_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(match sel_point(agent, a, false)? {
        Some((d, p)) => wrap_node(agent, NodeRef::new(d, p.node)),
        None => Value::Null,
    })
}

fn focus_offset(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(
        sel_point(agent, a, false)?.map_or(0.0, |(_, p)| f64::from(p.offset)),
    ))
}

fn sel_is_collapsed(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_sel(agent, a)?;
    Ok(Value::Boolean(
        sel_range(agent, doc).is_none_or(|(_, _, b)| b.start == b.end),
    ))
}

fn range_count(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_sel(agent, a)?;
    Ok(Value::Number(if sel_range(agent, doc).is_some() {
        1.0
    } else {
        0.0
    }))
}

fn sel_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_sel(agent, a)?;
    Ok(Value::str(match sel_range(agent, doc) {
        None => "None",
        Some((_, _, b)) if b.start == b.end => "Caret",
        Some(_) => "Range",
    }))
}

fn sel_direction(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_sel(agent, a)?;
    let st = sel_state(agent, doc);
    let collapsed = sel_range(agent, doc).is_none_or(|(_, _, b)| b.start == b.end);
    Ok(Value::str(match st.direction {
        _ if collapsed => "none",
        Direction::None => "none",
        Direction::Forwards => "forward",
        Direction::Backwards => "backward",
    }))
}

fn get_range_at(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_sel(agent, a)?;
    let i = bf_js::host::webidl::to_unsigned_long(agent, &a.arg(0))?;
    match sel_range(agent, doc) {
        Some((v, _, _)) if i == 0 => Ok(v),
        _ => throw_dom(agent, DomException::IndexSizeError),
    }
}

fn add_range(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_sel(agent, a)?;
    let range = a.arg(0);
    let Some(rd) = agent.host_unwrap::<RangeData>(&range) else {
        return agent.type_error("parameter 1 is not of type 'Range'.");
    };
    let (rdoc, id) = (rd.doc.get(), rd.id.get());
    // 1. El rango debe estar en el documento de la selección (su raíz es el documento).
    let b = range_bounds(agent, rdoc, id);
    let in_doc = rdoc == doc && with_doc(agent, doc, |d| d.get_root_node(b.start.node) == d.root());
    if !in_doc || sel_range(agent, doc).is_some() {
        return Ok(Value::Undefined);
    }
    set_sel(agent, doc, Some(range), Direction::None);
    Ok(Value::Undefined)
}

fn remove_range(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_sel(agent, a)?;
    if agent.host_unwrap::<RangeData>(&a.arg(0)).is_none() {
        return agent.type_error("parameter 1 is not of type 'Range'.");
    }
    match sel_range(agent, doc) {
        Some((v, _, _)) if agent.same_value(&v, &a.arg(0)) => {
            set_sel(agent, doc, None, Direction::None);
            Ok(Value::Undefined)
        }
        _ => throw_dom(agent, DomException::NotFoundError),
    }
}

fn remove_all_ranges(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_sel(agent, a)?;
    set_sel(agent, doc, None, Direction::None);
    Ok(Value::Undefined)
}

/// Un nodo con su offset validado para la selección: `None` si su raíz no es el documento.
fn sel_arg_point(
    agent: &mut Agent,
    doc: DocIndex,
    n: NodeRef,
    offset: u32,
) -> JsResult<Option<BoundaryPoint>> {
    let (doctype, len) = with_doc(agent, n.doc, |d| {
        (is_doctype(d, n.node), d.node_length(n.node) as u32)
    });
    if doctype {
        return throw_dom(agent, DomException::InvalidNodeTypeError);
    }
    if offset > len {
        return throw_dom(agent, DomException::IndexSizeError);
    }
    if n.doc != doc || with_doc(agent, doc, |d| d.get_root_node(n.node) != d.root()) {
        return Ok(None);
    }
    Ok(Some(bp(n.node, offset)))
}

fn sel_collapse(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_sel(agent, a)?;
    let Some(n) = arg_opt_node(agent, &a.arg(0), "parameter 1")? else {
        set_sel(agent, doc, None, Direction::None);
        return Ok(Value::Undefined);
    };
    let offset = bf_js::host::webidl::to_unsigned_long(agent, &a.arg(1))?;
    if let Some(p) = sel_arg_point(agent, doc, n, offset)? {
        let r = new_range(agent, doc, RangeBounds { start: p, end: p });
        set_sel(agent, doc, Some(r), Direction::None);
    }
    Ok(Value::Undefined)
}

fn collapse_to(agent: &mut Agent, a: &CallArgs<'_>, start: bool) -> JsResult<Value> {
    let doc = this_sel(agent, a)?;
    let Some((_, rdoc, b)) = sel_range(agent, doc) else {
        return throw_dom(agent, DomException::InvalidStateError);
    };
    let p = if start { b.start } else { b.end };
    let r = new_range(agent, rdoc, RangeBounds { start: p, end: p });
    set_sel(agent, doc, Some(r), Direction::None);
    Ok(Value::Undefined)
}

fn collapse_to_start(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    collapse_to(agent, a, true)
}

fn collapse_to_end(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    collapse_to(agent, a, false)
}

/// `extend(node, offset)` (Selection API §2).
fn extend(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_sel(agent, a)?;
    let n = arg_node(agent, &a.arg(0), "parameter 1")?;
    let offset = bf_js::host::webidl::to_unsigned_long(agent, &a.arg(1))?;
    let Some((_, _, b)) = sel_range(agent, doc) else {
        return throw_dom(agent, DomException::InvalidStateError);
    };
    let dir = sel_state(agent, doc).direction;
    let Some(new_focus) = sel_arg_point(agent, doc, n, offset)? else {
        return Ok(Value::Undefined);
    };
    let old_anchor = if dir == Direction::Backwards {
        b.end
    } else {
        b.start
    };
    let (bounds, backwards) = with_doc(agent, doc, |d| {
        if !same_root(d, new_focus.node, old_anchor.node) {
            (
                RangeBounds {
                    start: new_focus,
                    end: new_focus,
                },
                false,
            )
        } else if bp_cmp(d, old_anchor, new_focus) != Ordering::Greater {
            (
                RangeBounds {
                    start: old_anchor,
                    end: new_focus,
                },
                false,
            )
        } else {
            (
                RangeBounds {
                    start: new_focus,
                    end: old_anchor,
                },
                true,
            )
        }
    });
    let r = new_range(agent, doc, bounds);
    let dir = if backwards {
        Direction::Backwards
    } else {
        Direction::Forwards
    };
    set_sel(agent, doc, Some(r), dir);
    Ok(Value::Undefined)
}

fn set_base_and_extent(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_sel(agent, a)?;
    let an = arg_node(agent, &a.arg(0), "parameter 1")?;
    let ao = bf_js::host::webidl::to_unsigned_long(agent, &a.arg(1))?;
    let fnode = arg_node(agent, &a.arg(2), "parameter 3")?;
    let fo = bf_js::host::webidl::to_unsigned_long(agent, &a.arg(3))?;
    let anchor = sel_arg_point(agent, doc, an, ao)?;
    let focus = sel_arg_point(agent, doc, fnode, fo)?;
    let (Some(anchor), Some(focus)) = (anchor, focus) else {
        return Ok(Value::Undefined);
    };
    let backwards = with_doc(agent, doc, |d| bp_cmp(d, focus, anchor) == Ordering::Less);
    let bounds = if backwards {
        RangeBounds {
            start: focus,
            end: anchor,
        }
    } else {
        RangeBounds {
            start: anchor,
            end: focus,
        }
    };
    let r = new_range(agent, doc, bounds);
    let dir = if backwards {
        Direction::Backwards
    } else {
        Direction::Forwards
    };
    set_sel(agent, doc, Some(r), dir);
    Ok(Value::Undefined)
}

fn select_all_children(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_sel(agent, a)?;
    let n = arg_node(agent, &a.arg(0), "parameter 1")?;
    let len = with_doc(agent, n.doc, |d| d.child_nodes(n.node).len() as u32);
    let Some(start) = sel_arg_point(agent, doc, n, 0)? else {
        return Ok(Value::Undefined);
    };
    let r = new_range(
        agent,
        doc,
        RangeBounds {
            start,
            end: bp(n.node, len),
        },
    );
    set_sel(agent, doc, Some(r), Direction::Forwards);
    Ok(Value::Undefined)
}

fn delete_from_document(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_sel(agent, a)?;
    if let Some((v, rdoc, _)) = sel_range(agent, doc) {
        delete(agent, rdoc, &v)?;
    }
    Ok(Value::Undefined)
}

fn contains_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_sel(agent, a)?;
    let n = arg_node(agent, &a.arg(0), "parameter 1")?;
    let partial = bf_js::host::webidl::to_boolean(agent, &a.arg(1));
    let Some((_, rdoc, b)) = sel_range(agent, doc) else {
        return Ok(Value::Boolean(false));
    };
    if n.doc != rdoc {
        return Ok(Value::Boolean(false));
    }
    let r = with_doc(agent, rdoc, |d| {
        if d.get_root_node(n.node) != d.root() {
            return false;
        }
        let len = d.node_length(n.node) as u32;
        if partial {
            bp_cmp(d, b.start, bp(n.node, len)) != Ordering::Greater
                && bp_cmp(d, b.end, bp(n.node, 0)) != Ordering::Less
        } else {
            bp_cmp(d, b.start, bp(n.node, 0)) != Ordering::Greater
                && bp_cmp(d, b.end, bp(n.node, len)) != Ordering::Less
        }
    });
    Ok(Value::Boolean(r))
}

fn sel_to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_sel(agent, a)?;
    let Some((_, rdoc, b)) = sel_range(agent, doc) else {
        return Ok(Value::str(""));
    };
    let s = with_doc(agent, rdoc, |d| stringify(d, b));
    Ok(from_dom_string(&s))
}
