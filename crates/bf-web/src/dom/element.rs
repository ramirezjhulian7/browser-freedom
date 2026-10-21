//! `Element` (DOM §4.9) con los mixins `ParentNode`, `ChildNode` y
//! `NonDocumentTypeChildNode`, `innerHTML`/`outerHTML`/`insertAdjacentHTML` (DOM Parsing, con
//! `bf-html`) y los selectores (`matches`, `closest`, con `bf-css`).

use bf_dom::DomException;
use bf_infra::DomString;
use bf_js::host::{HostClass, HostClassId};
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use crate::dom::adopt::adopt_into;
use crate::dom::collections::{html_collection, named_node_map, token_list};
use crate::dom::insertion::{inserted, inserted_markup};
use crate::dom::{
    arg_node, classes, from_dom_string, from_opt_dom_string, mixins, mutated, this_node, throw_dom,
    to_dom_string, to_opt_dom_string, wrap_node, wrap_opt,
};
use crate::{NodeRef, with_doc, with_doc_mut};

pub fn element_class(node: HostClassId) -> HostClass {
    let c = HostClass::new("Element")
        .parent(node)
        .attribute("namespaceURI", Some(namespace_uri), None)
        .attribute("prefix", Some(prefix), None)
        .attribute("localName", Some(local_name), None)
        .attribute("tagName", Some(tag_name), None)
        .attribute("id", Some(get_id), Some(set_id))
        .attribute("className", Some(get_class_name), Some(set_class_name))
        .attribute("classList", Some(class_list), Some(set_class_list))
        .attribute("slot", Some(get_slot), Some(set_slot))
        .attribute("attributes", Some(attributes), None)
        .attribute("shadowRoot", Some(shadow_root), None)
        .attribute("innerHTML", Some(get_inner_html), Some(set_inner_html))
        .attribute("outerHTML", Some(get_outer_html), Some(set_outer_html))
        .method("hasAttributes", 0, has_attributes)
        .method("getAttributeNames", 0, get_attribute_names)
        .method("getAttribute", 1, get_attribute)
        .method("getAttributeNS", 2, get_attribute_ns)
        .method("setAttribute", 2, set_attribute)
        .method("setAttributeNS", 3, set_attribute_ns)
        .method("removeAttribute", 1, remove_attribute)
        .method("removeAttributeNS", 2, remove_attribute_ns)
        .method("toggleAttribute", 1, toggle_attribute)
        .method("hasAttribute", 1, has_attribute)
        .method("hasAttributeNS", 2, has_attribute_ns)
        .method("getAttributeNode", 1, get_attribute_node)
        .method("getAttributeNodeNS", 2, get_attribute_node_ns)
        .method("setAttributeNode", 1, set_attribute_node)
        .method("setAttributeNodeNS", 1, set_attribute_node)
        .method("removeAttributeNode", 1, remove_attribute_node)
        .method("closest", 1, mixins::closest)
        .method("matches", 1, mixins::matches)
        .method("webkitMatchesSelector", 1, mixins::matches)
        .method("getElementsByTagName", 1, get_elements_by_tag_name)
        .method("getElementsByTagNameNS", 2, get_elements_by_tag_name_ns)
        .method("getElementsByClassName", 1, get_elements_by_class_name)
        .method("insertAdjacentElement", 2, insert_adjacent_element)
        .method("insertAdjacentText", 2, insert_adjacent_text)
        .method("insertAdjacentHTML", 2, insert_adjacent_html);
    mixins::child_node(mixins::non_document_type_child_node(mixins::parent_node(c)))
}

/// Un `DOMString?` de namespace: `null` y `""` son "sin namespace".
pub fn namespace_arg(agent: &mut Agent, v: &Value) -> JsResult<Option<String>> {
    Ok(to_opt_dom_string(agent, v)?
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty()))
}

/// Una mutación de atributos sin valor de retorno.
fn done(agent: &mut Agent, r: Result<(), DomException>) -> JsResult<Value> {
    match r {
        Ok(()) => {
            mutated(agent);
            Ok(Value::Undefined)
        }
        Err(e) => throw_dom(agent, e),
    }
}

fn namespace_uri(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = with_doc(agent, n.doc, |d| d.namespace_uri(n.node));
    Ok(from_opt_dom_string(s.as_ref()))
}

fn prefix(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = with_doc(agent, n.doc, |d| d.prefix(n.node));
    Ok(from_opt_dom_string(s.as_ref()))
}

fn local_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = with_doc(agent, n.doc, |d| d.local_name(n.node));
    Ok(from_dom_string(&s))
}

fn tag_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = with_doc(agent, n.doc, |d| d.tag_name(n.node));
    Ok(from_dom_string(&s))
}

fn get_id(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = with_doc(agent, n.doc, |d| d.id(n.node));
    Ok(from_dom_string(&s))
}

fn set_id(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = to_dom_string(agent, &a.arg(0))?;
    with_doc_mut(agent, n.doc, |d| d.set_id(n.node, v));
    mutated(agent);
    Ok(Value::Undefined)
}

fn get_class_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = with_doc(agent, n.doc, |d| d.class_name(n.node));
    Ok(from_dom_string(&s))
}

fn set_class_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = to_dom_string(agent, &a.arg(0))?;
    with_doc_mut(agent, n.doc, |d| d.set_class_name(n.node, v));
    mutated(agent);
    Ok(Value::Undefined)
}

fn class_list(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(token_list(agent, n, "class"))
}

/// `[PutForwards=value]`: `el.classList = "a b"` pone el atributo `class`.
fn set_class_list(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = to_dom_string(agent, &a.arg(0))?;
    with_doc_mut(agent, n.doc, |d| d.set_class_name(n.node, v));
    mutated(agent);
    Ok(Value::Undefined)
}

/// `slot` refleja el atributo `slot`.
fn get_slot(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = with_doc(agent, n.doc, |d| d.get_attribute(n.node, &"slot".into()));
    Ok(from_dom_string(&v.unwrap_or_default()))
}

fn set_slot(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = to_dom_string(agent, &a.arg(0))?;
    let r = with_doc_mut(agent, n.doc, |d| d.set_attribute(n.node, &"slot".into(), v));
    done(agent, r)
}

fn attributes(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(named_node_map(agent, n))
}

/// Sin shadow DOM todavía: ningún elemento tiene shadow root.
fn shadow_root(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_node(agent, a)?;
    Ok(Value::Null)
}

fn has_attributes(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Boolean(with_doc(agent, n.doc, |d| {
        d.has_attributes(n.node)
    })))
}

fn get_attribute_names(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let names = with_doc(agent, n.doc, |d| d.get_attribute_names(n.node));
    let values: Vec<Value> = names.iter().map(from_dom_string).collect();
    Ok(Value::Object(agent.create_array_from_list(&values)?))
}

fn get_attribute(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let name = to_dom_string(agent, &a.arg(0))?;
    let v = with_doc(agent, n.doc, |d| d.get_attribute(n.node, &name));
    Ok(from_opt_dom_string(v.as_ref()))
}

fn get_attribute_ns(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let ns = namespace_arg(agent, &a.arg(0))?;
    let local = to_dom_string(agent, &a.arg(1))?;
    let v = with_doc(agent, n.doc, |d| {
        d.get_attribute_ns(n.node, ns.as_deref(), &local)
    });
    Ok(from_opt_dom_string(v.as_ref()))
}

fn set_attribute(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let name = to_dom_string(agent, &a.arg(0))?;
    let value = to_dom_string(agent, &a.arg(1))?;
    let r = with_doc_mut(agent, n.doc, |d| d.set_attribute(n.node, &name, value));
    let v = done(agent, r)?;
    crate::dom::events::content_attribute_changed(agent, n, &name);
    Ok(v)
}

fn set_attribute_ns(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let ns = namespace_arg(agent, &a.arg(0))?;
    let qn = to_dom_string(agent, &a.arg(1))?;
    let value = to_dom_string(agent, &a.arg(2))?;
    let r = with_doc_mut(agent, n.doc, |d| {
        d.set_attribute_ns(n.node, ns.as_deref(), &qn, value)
    });
    let v = done(agent, r)?;
    crate::dom::events::content_attribute_changed(agent, n, &qn);
    Ok(v)
}

fn remove_attribute(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let name = to_dom_string(agent, &a.arg(0))?;
    with_doc_mut(agent, n.doc, |d| d.remove_attribute(n.node, &name));
    mutated(agent);
    crate::dom::events::content_attribute_changed(agent, n, &name);
    Ok(Value::Undefined)
}

fn remove_attribute_ns(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let ns = namespace_arg(agent, &a.arg(0))?;
    let local = to_dom_string(agent, &a.arg(1))?;
    with_doc_mut(agent, n.doc, |d| {
        d.remove_attribute_ns(n.node, ns.as_deref(), &local)
    });
    mutated(agent);
    Ok(Value::Undefined)
}

fn toggle_attribute(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let name = to_dom_string(agent, &a.arg(0))?;
    let force = if a.arg(1).is_undefined() {
        None
    } else {
        Some(bf_js::host::webidl::to_boolean(agent, &a.arg(1)))
    };
    let r = with_doc_mut(agent, n.doc, |d| d.toggle_attribute(n.node, &name, force));
    match r {
        Ok(b) => {
            mutated(agent);
            crate::dom::events::content_attribute_changed(agent, n, &name);
            Ok(Value::Boolean(b))
        }
        Err(e) => throw_dom(agent, e),
    }
}

fn has_attribute(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let name = to_dom_string(agent, &a.arg(0))?;
    Ok(Value::Boolean(with_doc(agent, n.doc, |d| {
        d.has_attribute(n.node, &name)
    })))
}

fn has_attribute_ns(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let ns = namespace_arg(agent, &a.arg(0))?;
    let local = to_dom_string(agent, &a.arg(1))?;
    Ok(Value::Boolean(with_doc(agent, n.doc, |d| {
        d.has_attribute_ns(n.node, ns.as_deref(), &local)
    })))
}

fn get_attribute_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let name = to_dom_string(agent, &a.arg(0))?;
    let r = with_doc_mut(agent, n.doc, |d| d.get_attribute_node(n.node, &name));
    Ok(wrap_opt(agent, n.doc, r))
}

fn get_attribute_node_ns(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let ns = namespace_arg(agent, &a.arg(0))?;
    let local = to_dom_string(agent, &a.arg(1))?;
    let r = with_doc_mut(agent, n.doc, |d| {
        d.get_attribute_node_ns(n.node, ns.as_deref(), &local)
    });
    Ok(wrap_opt(agent, n.doc, r))
}

/// Un argumento que debe ser un `Attr`.
fn arg_attr(agent: &mut Agent, v: &Value) -> JsResult<NodeRef> {
    let attr = classes(agent).attr;
    if !agent.host_is_instance(v, attr) {
        return agent.type_error("parameter 1 is not of type 'Attr'.");
    }
    arg_node(agent, v, "parameter 1")
}

/// `setAttributeNode` / `setAttributeNodeNS` (§4.9.2 "set an attribute").
fn set_attribute_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let attr = arg_attr(agent, &a.arg(0))?;
    let attr = if attr.doc != n.doc {
        // Un `Attr` en uso en otro elemento no se puede mover (§4.9.2 paso 2).
        let owned = with_doc(agent, attr.doc, |d| d.owner_element(attr.node).is_some());
        if owned {
            return throw_dom(agent, DomException::InUseAttributeError);
        }
        adopt_into(agent, attr, n.doc)?
    } else {
        attr
    };
    let r = with_doc_mut(agent, n.doc, |d| d.set_attribute_node(n.node, attr.node));
    match r {
        Ok(old) => {
            mutated(agent);
            Ok(wrap_opt(agent, n.doc, old))
        }
        Err(e) => throw_dom(agent, e),
    }
}

fn remove_attribute_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let attr = arg_attr(agent, &a.arg(0))?;
    if attr.doc != n.doc {
        return throw_dom(agent, DomException::NotFoundError);
    }
    let r = with_doc_mut(agent, n.doc, |d| d.remove_attribute_node(n.node, attr.node));
    match r {
        Ok(old) => {
            mutated(agent);
            Ok(wrap_node(agent, NodeRef::new(n.doc, old)))
        }
        Err(e) => throw_dom(agent, e),
    }
}

/// `getElementsByTagName` (para `Element` y `Document`).
pub fn get_elements_by_tag_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let qn = to_dom_string(agent, &a.arg(0))?;
    let c = with_doc(agent, n.doc, |d| d.get_elements_by_tag_name(n.node, &qn));
    Ok(html_collection(agent, n.doc, c))
}

pub fn get_elements_by_tag_name_ns(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let ns = namespace_arg(agent, &a.arg(0))?;
    let local = to_dom_string(agent, &a.arg(1))?;
    let c = with_doc(agent, n.doc, |d| {
        d.get_elements_by_tag_name_ns(n.node, ns.as_deref(), &local)
    });
    Ok(html_collection(agent, n.doc, c))
}

pub fn get_elements_by_class_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let names = to_dom_string(agent, &a.arg(0))?;
    let c = with_doc(agent, n.doc, |d| {
        d.get_elements_by_class_name(n.node, &names)
    });
    Ok(html_collection(agent, n.doc, c))
}

fn insert_adjacent_element(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let where_ = to_dom_string(agent, &a.arg(0))?.to_string();
    let element = classes(agent).element;
    if !agent.host_is_instance(&a.arg(1), element) {
        return agent.type_error("parameter 2 is not of type 'Element'.");
    }
    let el = arg_node(agent, &a.arg(1), "parameter 2")?;
    let el = adopt_into(agent, el, n.doc)?;
    let r = with_doc_mut(agent, n.doc, |d| {
        d.insert_adjacent_element(n.node, &where_, el.node)
    });
    match r {
        Ok(r) => {
            mutated(agent);
            if r.is_some() {
                inserted(agent, n.doc, &[el.node]);
            }
            Ok(wrap_opt(agent, n.doc, r))
        }
        Err(e) => throw_dom(agent, e),
    }
}

fn insert_adjacent_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let where_ = to_dom_string(agent, &a.arg(0))?.to_string();
    let data = to_dom_string(agent, &a.arg(1))?;
    let r = with_doc_mut(agent, n.doc, |d| {
        d.insert_adjacent_text(n.node, &where_, data)
    });
    done(agent, r)
}

// --- DOM Parsing: innerHTML, outerHTML, insertAdjacentHTML ---

fn get_inner_html(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = with_doc(agent, n.doc, |d| bf_html::dom::inner_html(d, n.node));
    Ok(from_dom_string(&s))
}

fn set_inner_html(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let markup = legacy_null_to_empty(agent, &a.arg(0))?;
    with_doc_mut(agent, n.doc, |d| {
        bf_html::dom::set_inner_html(d, n.node, &markup)
    });
    mutated(agent);
    let roots = with_doc(agent, n.doc, |d| d.child_nodes(n.node));
    inserted_markup(agent, n.doc, &roots);
    Ok(Value::Undefined)
}

fn get_outer_html(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = with_doc(agent, n.doc, |d| bf_html::dom::outer_html(d, n.node));
    Ok(from_dom_string(&s))
}

fn set_outer_html(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let markup = legacy_null_to_empty(agent, &a.arg(0))?;
    let parent = with_doc(agent, n.doc, |d| d.parent(n.node));
    let r = with_doc_mut(agent, n.doc, |d| {
        bf_html::dom::set_outer_html(d, n.node, &markup)
    });
    let v = done(agent, r)?;
    if let Some(p) = parent {
        let roots = with_doc(agent, n.doc, |d| d.child_nodes(p));
        inserted_markup(agent, n.doc, &roots);
    }
    Ok(v)
}

fn insert_adjacent_html(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let position = to_dom_string(agent, &a.arg(0))?.to_string();
    let markup = to_dom_string(agent, &a.arg(1))?;
    let r = with_doc_mut(agent, n.doc, |d| {
        bf_html::dom::insert_adjacent_html(d, n.node, &position, &markup)
    });
    let v = done(agent, r)?;
    let scope = with_doc(agent, n.doc, |d| d.parent(n.node).unwrap_or(n.node));
    let roots = with_doc(agent, n.doc, |d| d.child_nodes(scope));
    inserted_markup(agent, n.doc, &roots);
    Ok(v)
}

/// `[LegacyNullToEmptyString] DOMString`.
fn legacy_null_to_empty(agent: &mut Agent, v: &Value) -> JsResult<DomString> {
    if v.is_null() {
        return Ok(DomString::new());
    }
    to_dom_string(agent, v)
}
