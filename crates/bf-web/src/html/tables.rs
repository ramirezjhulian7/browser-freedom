//! Las tablas (HTML §4.9): `HTMLTableElement` (`caption`, `tHead`, `tFoot` y sus
//! `create*`/`delete*`, `createTBody`, `insertRow`, `deleteRow`), `HTMLTableSectionElement`
//! (`insertRow`, `deleteRow`), `HTMLTableRowElement` (`rowIndex`, `sectionRowIndex`,
//! `insertCell`, `deleteCell`) y `HTMLTableCellElement` (`cellIndex`).
//!
//! Las listas son las del spec ("the rows collection": las filas del `thead`, las hijas
//! directas y las de los `tbody`, y las del `tfoot`, cada grupo en orden de árbol).

use bf_dom::{Document, DomException, Namespace, NodeId};
use bf_infra::DomString;
use bf_js::host::{HostClass, webidl};
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use super::elements::{html_element_arg, live_collection};
use crate::dom::{mutated, this_node, throw_dom, wrap_node, wrap_opt};
use crate::{NodeRef, with_doc, with_doc_mut};

/// Las operaciones y atributos propios de cada interfaz de tabla.
pub fn extend(name: &str, class: HostClass) -> HostClass {
    match name {
        "HTMLTableElement" => class
            .attribute("caption", Some(get_caption), Some(set_caption))
            .method("createCaption", 0, create_caption)
            .method("deleteCaption", 0, delete_caption)
            .attribute("tHead", Some(get_thead), Some(set_thead))
            .method("createTHead", 0, create_thead)
            .method("deleteTHead", 0, delete_thead)
            .attribute("tFoot", Some(get_tfoot), Some(set_tfoot))
            .method("createTFoot", 0, create_tfoot)
            .method("deleteTFoot", 0, delete_tfoot)
            .method("createTBody", 0, create_tbody)
            .attribute("rows", Some(table_rows_attr), None)
            .attribute("tBodies", Some(table_bodies_attr), None)
            .method("insertRow", 0, table_insert_row)
            .method("deleteRow", 1, table_delete_row),
        "HTMLTableSectionElement" => class
            .attribute("rows", Some(section_rows_attr), None)
            .method("insertRow", 0, section_insert_row)
            .method("deleteRow", 1, section_delete_row),
        "HTMLTableRowElement" => class
            .attribute("rowIndex", Some(row_index), None)
            .attribute("sectionRowIndex", Some(section_row_index), None)
            .attribute("cells", Some(row_cells_attr), None)
            .method("insertCell", 0, insert_cell)
            .method("deleteCell", 1, delete_cell),
        "HTMLTableCellElement" => class.attribute("cellIndex", Some(cell_index), None),
        _ => class,
    }
}

fn is(doc: &Document, n: NodeId, name: &str) -> bool {
    doc.element(n).is_some_and(|e| e.is_html(name))
}

/// Los hijos elemento HTML `name` de `n`.
fn children_named(doc: &Document, n: NodeId, name: &str) -> Vec<NodeId> {
    doc.element_children(n)
        .into_iter()
        .filter(|c| is(doc, *c, name))
        .collect()
}

/// "The rows collection" de una tabla (§4.9.1).
pub fn table_rows(doc: &Document, table: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let kids = doc.element_children(table);
    for c in &kids {
        if is(doc, *c, "thead") {
            out.extend(children_named(doc, *c, "tr"));
        }
    }
    for c in &kids {
        if is(doc, *c, "tr") {
            out.push(*c);
        } else if is(doc, *c, "tbody") {
            out.extend(children_named(doc, *c, "tr"));
        }
    }
    for c in &kids {
        if is(doc, *c, "tfoot") {
            out.extend(children_named(doc, *c, "tr"));
        }
    }
    out
}

/// Los `tbody` hijos de una tabla (`tBodies`).
pub fn table_bodies(doc: &Document, table: NodeId) -> Vec<NodeId> {
    children_named(doc, table, "tbody")
}

/// Las filas de una sección (`rows` de `thead`/`tbody`/`tfoot`).
pub fn section_rows(doc: &Document, section: NodeId) -> Vec<NodeId> {
    children_named(doc, section, "tr")
}

/// Las celdas de una fila (`cells`: los `td` y `th` hijos).
pub fn row_cells(doc: &Document, row: NodeId) -> Vec<NodeId> {
    doc.element_children(row)
        .into_iter()
        .filter(|c| is(doc, *c, "td") || is(doc, *c, "th"))
        .collect()
}

fn new_element(doc: &mut Document, name: &str) -> NodeId {
    doc.create_element_raw(Namespace::Html, None, DomString::from(name))
}

/// El primer hijo `name` de la tabla.
fn first_child_named(agent: &Agent, t: NodeRef, name: &str) -> Option<NodeId> {
    with_doc(agent, t.doc, |d| {
        children_named(d, t.node, name).first().copied()
    })
}

fn get_caption(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = this_node(agent, a)?;
    let c = first_child_named(agent, t, "caption");
    Ok(wrap_opt(agent, t.doc, c))
}

/// Los setters de `caption`/`tHead`/`tFoot`: quitan el actual e insertan el nuevo (`null`
/// solo quita); `tHead`/`tFoot` validan el nombre (HierarchyRequestError).
fn replace_part(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    name: &str,
    insert: fn(&Document, NodeId) -> Option<NodeId>,
) -> JsResult<Value> {
    let t = this_node(agent, a)?;
    let v = a.arg(0);
    let new = if matches!(v, Value::Null) {
        None
    } else {
        match html_element_arg(agent, &v) {
            Some(n) => Some(n),
            None => return agent.type_error(&format!("Failed to set '{name}': not an element")),
        }
    };
    if let Some(n) = new
        && !with_doc(agent, n.doc, |d| is(d, n.node, name))
    {
        return throw_dom(agent, DomException::HierarchyRequestError);
    }
    let r = with_doc_mut(agent, t.doc, |d| {
        if let Some(old) = children_named(d, t.node, name).first().copied() {
            d.remove(old);
        }
        match new {
            Some(n) => {
                let before = insert(d, t.node);
                d.insert_before_checked(t.node, n.node, before).map(|_| ())
            }
            None => Ok(()),
        }
    });
    match r {
        Ok(()) => {
            mutated(agent);
            Ok(Value::Undefined)
        }
        Err(e) => throw_dom(agent, e),
    }
}

/// El caption va primero.
fn caption_position(d: &Document, t: NodeId) -> Option<NodeId> {
    d.first_child(t)
}

/// El `thead` va antes del primer hijo que no es `caption` ni `colgroup`.
fn thead_position(d: &Document, t: NodeId) -> Option<NodeId> {
    let mut c = d.first_child(t);
    while let Some(n) = c {
        if d.element(n).is_some() && !is(d, n, "caption") && !is(d, n, "colgroup") {
            return Some(n);
        }
        c = d.next_sibling(n);
    }
    None
}

/// El `tfoot` va al final.
fn tfoot_position(_: &Document, _: NodeId) -> Option<NodeId> {
    None
}

fn set_caption(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    replace_part(agent, a, "caption", caption_position)
}

fn create_part(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    name: &str,
    position: fn(&Document, NodeId) -> Option<NodeId>,
) -> JsResult<Value> {
    let t = this_node(agent, a)?;
    if let Some(c) = first_child_named(agent, t, name) {
        return Ok(wrap_node(agent, NodeRef::new(t.doc, c)));
    }
    let n = with_doc_mut(agent, t.doc, |d| {
        let n = new_element(d, name);
        let before = position(d, t.node);
        d.insert_before(t.node, n, before);
        n
    });
    mutated(agent);
    Ok(wrap_node(agent, NodeRef::new(t.doc, n)))
}

fn delete_part(agent: &mut Agent, a: &CallArgs<'_>, name: &str) -> JsResult<Value> {
    let t = this_node(agent, a)?;
    if let Some(c) = first_child_named(agent, t, name) {
        with_doc_mut(agent, t.doc, |d| d.remove(c));
        mutated(agent);
    }
    Ok(Value::Undefined)
}

fn create_caption(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    create_part(agent, a, "caption", caption_position)
}
fn delete_caption(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    delete_part(agent, a, "caption")
}
fn get_thead(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = this_node(agent, a)?;
    let c = first_child_named(agent, t, "thead");
    Ok(wrap_opt(agent, t.doc, c))
}
fn set_thead(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    replace_part(agent, a, "thead", thead_position)
}
fn create_thead(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    create_part(agent, a, "thead", thead_position)
}
fn delete_thead(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    delete_part(agent, a, "thead")
}
fn get_tfoot(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = this_node(agent, a)?;
    let c = first_child_named(agent, t, "tfoot");
    Ok(wrap_opt(agent, t.doc, c))
}
fn set_tfoot(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    replace_part(agent, a, "tfoot", tfoot_position)
}
fn create_tfoot(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    create_part(agent, a, "tfoot", tfoot_position)
}
fn delete_tfoot(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    delete_part(agent, a, "tfoot")
}

/// `createTBody()`: después del último `tbody`, o al final.
fn create_tbody(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = this_node(agent, a)?;
    let n = with_doc_mut(agent, t.doc, |d| {
        let n = new_element(d, "tbody");
        let before = table_bodies(d, t.node)
            .last()
            .and_then(|last| d.next_sibling(*last));
        d.insert_before(t.node, n, before);
        n
    });
    mutated(agent);
    Ok(wrap_node(agent, NodeRef::new(t.doc, n)))
}

/// El índice de `insertRow`/`insertCell` (`long`, por defecto -1).
fn index_arg(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<i64> {
    match a.arg(0) {
        Value::Undefined => Ok(-1),
        v => Ok(i64::from(webidl::to_long(agent, &v)?)),
    }
}

/// `table.insertRow(index)` (§4.9.1).
fn table_insert_row(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = this_node(agent, a)?;
    let index = index_arg(agent, a)?;
    let rows = with_doc(agent, t.doc, |d| table_rows(d, t.node));
    if index < -1 || index > rows.len() as i64 {
        return throw_dom(agent, DomException::IndexSizeError);
    }
    let tr = with_doc_mut(agent, t.doc, |d| {
        let tr = new_element(d, "tr");
        if rows.is_empty() {
            // Sin filas: en el último tbody, o en un tbody nuevo al final.
            match table_bodies(d, t.node).last().copied() {
                Some(b) => d.insert_before(b, tr, None),
                None => {
                    let b = new_element(d, "tbody");
                    d.insert_before(t.node, b, None);
                    d.insert_before(b, tr, None);
                }
            }
        } else if index == -1 || index == rows.len() as i64 {
            let last = *rows.last().expect("filas");
            let parent = d.parent(last).expect("padre de la fila");
            d.insert_before(parent, tr, None);
        } else {
            let at = rows[index as usize];
            let parent = d.parent(at).expect("padre de la fila");
            d.insert_before(parent, tr, Some(at));
        }
        tr
    });
    mutated(agent);
    Ok(wrap_node(agent, NodeRef::new(t.doc, tr)))
}

fn remove_at(agent: &mut Agent, owner: NodeRef, items: Vec<NodeId>, index: i64) -> JsResult<Value> {
    let target = if index == -1 {
        match items.last() {
            Some(n) => Some(*n),
            None => return Ok(Value::Undefined),
        }
    } else if index < 0 || index >= items.len() as i64 {
        return throw_dom(agent, DomException::IndexSizeError);
    } else {
        Some(items[index as usize])
    };
    if let Some(n) = target {
        with_doc_mut(agent, owner.doc, |d| d.remove(n));
        mutated(agent);
    }
    Ok(Value::Undefined)
}

fn table_delete_row(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = this_node(agent, a)?;
    let index = i64::from(webidl::to_long(agent, &a.arg(0))?);
    let rows = with_doc(agent, t.doc, |d| table_rows(d, t.node));
    remove_at(agent, t, rows, index)
}

/// `section.insertRow(index)`.
fn section_insert_row(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_node(agent, a)?;
    let index = index_arg(agent, a)?;
    let rows = with_doc(agent, s.doc, |d| section_rows(d, s.node));
    if index < -1 || index > rows.len() as i64 {
        return throw_dom(agent, DomException::IndexSizeError);
    }
    let tr = with_doc_mut(agent, s.doc, |d| {
        let tr = new_element(d, "tr");
        let before = if index == -1 || index == rows.len() as i64 {
            None
        } else {
            Some(rows[index as usize])
        };
        d.insert_before(s.node, tr, before);
        tr
    });
    mutated(agent);
    Ok(wrap_node(agent, NodeRef::new(s.doc, tr)))
}

fn section_delete_row(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_node(agent, a)?;
    let index = i64::from(webidl::to_long(agent, &a.arg(0))?);
    let rows = with_doc(agent, s.doc, |d| section_rows(d, s.node));
    remove_at(agent, s, rows, index)
}

/// `rowIndex`: la posición en las filas de su tabla (-1 si no está en una).
fn row_index(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_node(agent, a)?;
    let i = with_doc(agent, r.doc, |d| {
        let p = d.parent(r.node)?;
        let table = if is(d, p, "table") {
            p
        } else if is(d, p, "thead") || is(d, p, "tbody") || is(d, p, "tfoot") {
            d.parent(p).filter(|t| is(d, *t, "table"))?
        } else {
            return None;
        };
        table_rows(d, table).iter().position(|x| *x == r.node)
    });
    Ok(Value::Number(i.map_or(-1.0, |i| i as f64)))
}

/// `sectionRowIndex`: la posición entre las filas de su padre (tabla o sección).
fn section_row_index(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_node(agent, a)?;
    let i = with_doc(agent, r.doc, |d| {
        let p = d.parent(r.node)?;
        if is(d, p, "table") {
            children_named(d, p, "tr").iter().position(|x| *x == r.node)
        } else if is(d, p, "thead") || is(d, p, "tbody") || is(d, p, "tfoot") {
            section_rows(d, p).iter().position(|x| *x == r.node)
        } else {
            None
        }
    });
    Ok(Value::Number(i.map_or(-1.0, |i| i as f64)))
}

/// `row.insertCell(index)`: un `td` nuevo.
fn insert_cell(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_node(agent, a)?;
    let index = index_arg(agent, a)?;
    let cells = with_doc(agent, r.doc, |d| row_cells(d, r.node));
    if index < -1 || index > cells.len() as i64 {
        return throw_dom(agent, DomException::IndexSizeError);
    }
    let td = with_doc_mut(agent, r.doc, |d| {
        let td = new_element(d, "td");
        let before = if index == -1 || index == cells.len() as i64 {
            None
        } else {
            Some(cells[index as usize])
        };
        d.insert_before(r.node, td, before);
        td
    });
    mutated(agent);
    Ok(wrap_node(agent, NodeRef::new(r.doc, td)))
}

fn delete_cell(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_node(agent, a)?;
    let index = i64::from(webidl::to_long(agent, &a.arg(0))?);
    let cells = with_doc(agent, r.doc, |d| row_cells(d, r.node));
    remove_at(agent, r, cells, index)
}

/// `cellIndex`: la posición en las celdas de su fila (-1 si el padre no es un `tr`).
fn cell_index(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let c = this_node(agent, a)?;
    let i = with_doc(agent, c.doc, |d| {
        let p = d.parent(c.node).filter(|p| is(d, *p, "tr"))?;
        row_cells(d, p).iter().position(|x| *x == c.node)
    });
    Ok(Value::Number(i.map_or(-1.0, |i| i as f64)))
}

fn table_rows_attr(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = this_node(agent, a)?;
    Ok(live_collection(agent, t, 1, table_rows))
}

fn table_bodies_attr(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = this_node(agent, a)?;
    Ok(live_collection(agent, t, 2, table_bodies))
}

fn section_rows_attr(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_node(agent, a)?;
    Ok(live_collection(agent, s, 3, section_rows))
}

fn row_cells_attr(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_node(agent, a)?;
    Ok(live_collection(agent, r, 4, row_cells))
}
