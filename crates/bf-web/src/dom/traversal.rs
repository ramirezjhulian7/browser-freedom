//! Traversal (DOM §6): `NodeFilter`, `NodeIterator` y `TreeWalker`. Sin los "pre-removing
//! steps" de `NodeIterator` (`bf-dom` no expone ese hook): un nodo de referencia quitado del
//! árbol deja al iterador en su posición anterior.

use std::cell::Cell;

use bf_dom::{Document, NodeId};
use bf_js::host::HostClass;
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use crate::dom::{arg_node, classes, throw_dom, wrap_node};
use crate::{DocIndex, NodeRef, WebContextHandle};

const FILTER_ACCEPT: u16 = 1;
const FILTER_REJECT: u16 = 2;
const FILTER_SKIP: u16 = 3;

pub fn node_filter_class() -> HostClass {
    let constants: [(&str, f64); 16] = [
        ("FILTER_ACCEPT", 1.0),
        ("FILTER_REJECT", 2.0),
        ("FILTER_SKIP", 3.0),
        ("SHOW_ALL", 4294967295.0),
        ("SHOW_ELEMENT", 0x1 as f64),
        ("SHOW_ATTRIBUTE", 0x2 as f64),
        ("SHOW_TEXT", 0x4 as f64),
        ("SHOW_CDATA_SECTION", 0x8 as f64),
        ("SHOW_ENTITY_REFERENCE", 0x10 as f64),
        ("SHOW_ENTITY", 0x20 as f64),
        ("SHOW_PROCESSING_INSTRUCTION", 0x40 as f64),
        ("SHOW_COMMENT", 0x80 as f64),
        ("SHOW_DOCUMENT", 0x100 as f64),
        ("SHOW_DOCUMENT_TYPE", 0x200 as f64),
        ("SHOW_DOCUMENT_FRAGMENT", 0x400 as f64),
        ("SHOW_NOTATION", 0x800 as f64),
    ];
    let mut c = HostClass::new("NodeFilter");
    for (n, v) in constants {
        c = c.constant(n, v);
    }
    c
}

/// El estado común de `NodeIterator` y `TreeWalker`.
#[derive(Debug)]
pub struct TraversalData {
    pub doc: DocIndex,
    pub root: NodeId,
    pub what_to_show: u32,
    /// `current` del `TreeWalker` o el nodo de referencia del `NodeIterator`.
    pub current: Cell<NodeId>,
    pub pointer_before: Cell<bool>,
    pub active: Cell<bool>,
}

pub fn node_iterator_class() -> HostClass {
    HostClass::new("NodeIterator")
        .attribute("root", Some(root), None)
        .attribute("referenceNode", Some(current), None)
        .attribute("pointerBeforeReferenceNode", Some(pointer_before), None)
        .attribute("whatToShow", Some(what_to_show), None)
        .attribute("filter", Some(filter), None)
        .method("nextNode", 0, ni_next_node)
        .method("previousNode", 0, ni_previous_node)
        .method("detach", 0, detach)
}

pub fn tree_walker_class() -> HostClass {
    HostClass::new("TreeWalker")
        .attribute("root", Some(root), None)
        .attribute("whatToShow", Some(what_to_show), None)
        .attribute("filter", Some(filter), None)
        .attribute("currentNode", Some(current), Some(set_current))
        .method("parentNode", 0, tw_parent_node)
        .method("firstChild", 0, tw_first_child)
        .method("lastChild", 0, tw_last_child)
        .method("previousSibling", 0, tw_previous_sibling)
        .method("nextSibling", 0, tw_next_sibling)
        .method("previousNode", 0, tw_previous_node)
        .method("nextNode", 0, tw_next_node)
}

/// `createNodeIterator`/`createTreeWalker` (DOM §4.5).
pub fn create(
    agent: &mut Agent,
    walker: bool,
    root_v: &Value,
    what_to_show: &Value,
    filter_v: &Value,
) -> JsResult<Value> {
    let r = arg_node(agent, root_v, "parameter 1")?;
    let what = if what_to_show.is_undefined() {
        0xFFFF_FFFF
    } else {
        bf_js::host::webidl::to_unsigned_long(agent, what_to_show)?
    };
    let class = if walker {
        classes(agent).tree_walker
    } else {
        classes(agent).node_iterator
    };
    let o = agent.host_create(
        class,
        Box::new(TraversalData {
            doc: r.doc,
            root: r.node,
            what_to_show: what,
            current: Cell::new(r.node),
            pointer_before: Cell::new(true),
            active: Cell::new(false),
        }),
    );
    if let Some(s) = agent.host_slots_mut(o) {
        s.push(if filter_v.is_undefined() {
            Value::Null
        } else {
            filter_v.clone()
        });
    }
    Ok(Value::Object(o))
}

fn data<'a>(agent: &'a Agent, v: &Value) -> Option<&'a TraversalData> {
    agent.host_unwrap::<TraversalData>(v)
}

fn this_data(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<(DocIndex, NodeId, u32)> {
    match data(agent, &a.this) {
        Some(d) => Ok((d.doc, d.root, d.what_to_show)),
        None => agent.type_error("Illegal invocation"),
    }
}

fn filter_value(agent: &mut Agent, this: &Value) -> Value {
    match this {
        Value::Object(o) => agent
            .host_slots_mut(*o)
            .and_then(|s| s.first().cloned())
            .unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

fn root(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, root, _) = this_data(agent, a)?;
    Ok(wrap_node(agent, NodeRef::new(doc, root)))
}

fn current(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, _, _) = this_data(agent, a)?;
    let c = data(agent, &a.this).expect("traversal").current.get();
    Ok(wrap_node(agent, NodeRef::new(doc, c)))
}

fn set_current(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, _, _) = this_data(agent, a)?;
    let n = arg_node(agent, &a.arg(0), "currentNode")?;
    if n.doc == doc
        && let Some(d) = data(agent, &a.this)
    {
        d.current.set(n.node);
    }
    Ok(Value::Undefined)
}

fn pointer_before(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_data(agent, a)?;
    Ok(Value::Boolean(
        data(agent, &a.this)
            .expect("traversal")
            .pointer_before
            .get(),
    ))
}

fn what_to_show(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (_, _, w) = this_data(agent, a)?;
    Ok(Value::Number(f64::from(w)))
}

fn filter(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_data(agent, a)?;
    Ok(filter_value(agent, &a.this))
}

fn detach(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Undefined)
}

/// "Filter" (DOM §6.1): `whatToShow` y luego el callback.
fn filter_node(agent: &mut Agent, this: &Value, n: NodeId) -> JsResult<u16> {
    let (doc, what) = {
        let d = data(agent, this).expect("traversal");
        if d.active.get() {
            return throw_dom(agent, bf_dom::DomException::InvalidStateError);
        }
        (d.doc, d.what_to_show)
    };
    let node_type = with_doc(agent, doc, |d| d.node_type(n) as u16);
    if what & (1 << (node_type - 1)) == 0 {
        return Ok(FILTER_SKIP);
    }
    let f = filter_value(agent, this);
    if f.is_null() {
        return Ok(FILTER_ACCEPT);
    }
    let node = wrap_node(agent, NodeRef::new(doc, n));
    data(agent, this).expect("traversal").active.set(true);
    let r = if agent.is_callable(&f) {
        agent.call(&f, Value::Undefined, &[node])
    } else if let Value::Object(o) = f {
        match agent.get(o, &PropertyKey::from("acceptNode"), f.clone()) {
            Ok(m) if agent.is_callable(&m) => agent.call(&m, f.clone(), &[node]),
            Ok(_) => agent.type_error("NodeFilter.acceptNode is not a function"),
            Err(e) => Err(e),
        }
    } else {
        Ok(Value::Number(f64::from(FILTER_ACCEPT)))
    };
    data(agent, this).expect("traversal").active.set(false);
    let v = r?;
    bf_js::host::webidl::to_unsigned_short(agent, &v)
}

fn with_doc<R>(agent: &Agent, doc: DocIndex, f: impl FnOnce(&Document) -> R) -> R {
    let ctx = WebContextHandle::current(agent).get(agent);
    f(ctx.doc(doc))
}

/// El siguiente en preorden dentro de `root`.
fn following(agent: &Agent, doc: DocIndex, n: NodeId, root: NodeId) -> Option<NodeId> {
    with_doc(agent, doc, |d| d.following_within(n, root))
}

/// El anterior en preorden dentro de `root`.
fn preceding(agent: &Agent, doc: DocIndex, n: NodeId, root: NodeId) -> Option<NodeId> {
    with_doc(agent, doc, |d| {
        if n == root {
            return None;
        }
        match d.previous_sibling(n) {
            Some(mut p) => {
                while let Some(l) = d.last_child(p) {
                    p = l;
                }
                Some(p)
            }
            None => d.parent(n),
        }
    })
}

/// "Traverse" de `NodeIterator` (DOM §6.1.1).
fn ni_traverse(agent: &mut Agent, a: &CallArgs<'_>, next: bool) -> JsResult<Value> {
    let (doc, root, _) = this_data(agent, a)?;
    let (mut node, mut before) = {
        let d = data(agent, &a.this).expect("traversal");
        (d.current.get(), d.pointer_before.get())
    };
    loop {
        if next {
            if !before {
                match following(agent, doc, node, root) {
                    Some(n) => node = n,
                    None => return Ok(Value::Null),
                }
            } else {
                before = false;
            }
        } else if before {
            match preceding(agent, doc, node, root) {
                Some(n) => node = n,
                None => return Ok(Value::Null),
            }
        } else {
            before = true;
        }
        if filter_node(agent, &a.this, node)? == FILTER_ACCEPT {
            break;
        }
    }
    let d = data(agent, &a.this).expect("traversal");
    d.current.set(node);
    d.pointer_before.set(before);
    Ok(wrap_node(agent, NodeRef::new(doc, node)))
}

fn ni_next_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    ni_traverse(agent, a, true)
}

fn ni_previous_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    ni_traverse(agent, a, false)
}

fn set_walker_current(agent: &mut Agent, a: &CallArgs<'_>, doc: DocIndex, n: NodeId) -> Value {
    data(agent, &a.this).expect("traversal").current.set(n);
    wrap_node(agent, NodeRef::new(doc, n))
}

fn tw_parent_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, root, _) = this_data(agent, a)?;
    let mut node = data(agent, &a.this).expect("traversal").current.get();
    while node != root {
        let Some(p) = with_doc(agent, doc, |d| d.parent(node)) else {
            break;
        };
        node = p;
        if filter_node(agent, &a.this, node)? == FILTER_ACCEPT {
            return Ok(set_walker_current(agent, a, doc, node));
        }
    }
    Ok(Value::Null)
}

/// "Traverse children" (DOM §6.2).
fn tw_children(agent: &mut Agent, a: &CallArgs<'_>, first: bool) -> JsResult<Value> {
    let (doc, root, _) = this_data(agent, a)?;
    let current = data(agent, &a.this).expect("traversal").current.get();
    let child = |agent: &Agent, n: NodeId| {
        with_doc(agent, doc, |d| {
            if first {
                d.first_child(n)
            } else {
                d.last_child(n)
            }
        })
    };
    let sibling = |agent: &Agent, n: NodeId| {
        with_doc(agent, doc, |d| {
            if first {
                d.next_sibling(n)
            } else {
                d.previous_sibling(n)
            }
        })
    };
    let mut node = child(agent, current);
    while let Some(n) = node {
        let r = filter_node(agent, &a.this, n)?;
        if r == FILTER_ACCEPT {
            return Ok(set_walker_current(agent, a, doc, n));
        }
        if r == FILTER_SKIP
            && let Some(c) = child(agent, n)
        {
            node = Some(c);
            continue;
        }
        let mut cur = n;
        loop {
            if let Some(s) = sibling(agent, cur) {
                node = Some(s);
                break;
            }
            let parent = with_doc(agent, doc, |d| d.parent(cur));
            match parent {
                Some(p) if p != root && p != current => cur = p,
                _ => return Ok(Value::Null),
            }
        }
    }
    Ok(Value::Null)
}

fn tw_first_child(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    tw_children(agent, a, true)
}

fn tw_last_child(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    tw_children(agent, a, false)
}

/// "Traverse siblings" (DOM §6.2).
fn tw_siblings(agent: &mut Agent, a: &CallArgs<'_>, next: bool) -> JsResult<Value> {
    let (doc, root, _) = this_data(agent, a)?;
    let mut node = data(agent, &a.this).expect("traversal").current.get();
    if node == root {
        return Ok(Value::Null);
    }
    loop {
        let mut sibling = with_doc(agent, doc, |d| {
            if next {
                d.next_sibling(node)
            } else {
                d.previous_sibling(node)
            }
        });
        while let Some(s) = sibling {
            node = s;
            let r = filter_node(agent, &a.this, node)?;
            if r == FILTER_ACCEPT {
                return Ok(set_walker_current(agent, a, doc, node));
            }
            let child = with_doc(agent, doc, |d| {
                if next {
                    d.first_child(node)
                } else {
                    d.last_child(node)
                }
            });
            sibling = if r == FILTER_REJECT || child.is_none() {
                with_doc(agent, doc, |d| {
                    if next {
                        d.next_sibling(node)
                    } else {
                        d.previous_sibling(node)
                    }
                })
            } else {
                child
            };
        }
        let parent = with_doc(agent, doc, |d| d.parent(node));
        match parent {
            None => return Ok(Value::Null),
            Some(p) if p == root => return Ok(Value::Null),
            Some(p) => {
                node = p;
                if filter_node(agent, &a.this, node)? == FILTER_ACCEPT {
                    return Ok(Value::Null);
                }
            }
        }
    }
}

fn tw_previous_sibling(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    tw_siblings(agent, a, false)
}

fn tw_next_sibling(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    tw_siblings(agent, a, true)
}

/// `previousNode()` (DOM §6.2).
fn tw_previous_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, root, _) = this_data(agent, a)?;
    let mut node = data(agent, &a.this).expect("traversal").current.get();
    while node != root {
        let mut sibling = with_doc(agent, doc, |d| d.previous_sibling(node));
        while let Some(s) = sibling {
            node = s;
            let mut r = filter_node(agent, &a.this, node)?;
            while r != FILTER_REJECT
                && let Some(l) = with_doc(agent, doc, |d| d.last_child(node))
            {
                node = l;
                r = filter_node(agent, &a.this, node)?;
            }
            if r == FILTER_ACCEPT {
                return Ok(set_walker_current(agent, a, doc, node));
            }
            sibling = with_doc(agent, doc, |d| d.previous_sibling(node));
        }
        let parent = with_doc(agent, doc, |d| d.parent(node));
        match parent {
            None => return Ok(Value::Null),
            Some(p) => {
                node = p;
                if node == root {
                    // El root mismo solo se acepta si pasa el filtro.
                    if filter_node(agent, &a.this, node)? == FILTER_ACCEPT {
                        return Ok(set_walker_current(agent, a, doc, node));
                    }
                    return Ok(Value::Null);
                }
                if filter_node(agent, &a.this, node)? == FILTER_ACCEPT {
                    return Ok(set_walker_current(agent, a, doc, node));
                }
            }
        }
    }
    Ok(Value::Null)
}

/// `nextNode()` (DOM §6.2).
fn tw_next_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, root, _) = this_data(agent, a)?;
    let mut node = data(agent, &a.this).expect("traversal").current.get();
    let mut r = FILTER_ACCEPT;
    loop {
        while r != FILTER_REJECT
            && let Some(c) = with_doc(agent, doc, |d| d.first_child(node))
        {
            node = c;
            r = filter_node(agent, &a.this, node)?;
            if r == FILTER_ACCEPT {
                return Ok(set_walker_current(agent, a, doc, node));
            }
        }
        let mut sibling = None;
        let mut temp = node;
        loop {
            if temp == root {
                return Ok(Value::Null);
            }
            if let Some(s) = with_doc(agent, doc, |d| d.next_sibling(temp)) {
                sibling = Some(s);
                break;
            }
            match with_doc(agent, doc, |d| d.parent(temp)) {
                Some(p) => temp = p,
                None => break,
            }
        }
        let Some(s) = sibling else {
            return Ok(Value::Null);
        };
        node = s;
        r = filter_node(agent, &a.this, node)?;
        if r == FILTER_ACCEPT {
            return Ok(set_walker_current(agent, a, doc, node));
        }
    }
}
