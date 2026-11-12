//! Shadow DOM desde JavaScript (DOM §4.2.2, §4.8; HTML §4.12.4, §8.5): `ShadowRoot`,
//! `Element.attachShadow`/`shadowRoot`, `getHTML`/`setHTMLUnsafe`, `Slottable.assignedSlot`,
//! `HTMLSlotElement` (`assignedNodes`, `assignedElements`, `assign`) y `slotchange`. **E1.**

use bf_dom::events::{EventInit, EventKind, EventTarget};
use bf_dom::shadow::{ShadowRootInit, ShadowRootMode, SlotAssignmentMode};
use bf_dom::{DomException, DomString, NodeData, NodeId};
use bf_js::host::{HostClass, HostClassId};
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use crate::dom::events::{dict_bool, dict_get};
use crate::dom::insertion::inserted_markup;
use crate::dom::{
    classes, from_dom_string, mutated, this_node, throw_dom, to_dom_string, unwrap_node, wrap_node,
};
use crate::{DocIndex, NodeRef, with_doc, with_doc_mut};

/// Las clases de esta área.
#[derive(Debug, Clone, Copy)]
pub struct ShadowClasses {
    pub shadow_root: HostClassId,
}

/// Registra `ShadowRoot` y extiende `Element`, `Text` y `Node` (una vez por agente).
pub fn register(agent: &mut Agent) {
    if agent.embedder_data::<ShadowClasses>().is_some() {
        return;
    }
    let c = classes(agent).clone();
    let shadow_root = agent.register_host_class(shadow_root_class(c.document_fragment));
    agent.set_embedder_data(ShadowClasses { shadow_root });
    agent.host_extend_class(
        c.element,
        HostClass::new("Element")
            .method("attachShadow", 1, attach_shadow)
            .attribute("shadowRoot", Some(shadow_root_attr), None)
            .attribute("assignedSlot", Some(assigned_slot), None)
            .method("getHTML", 0, element_get_html)
            .method("setHTMLUnsafe", 1, element_set_html_unsafe),
    );
    agent.host_extend_class(
        c.text,
        HostClass::new("Text").attribute("assignedSlot", Some(assigned_slot), None),
    );
    agent.host_extend_class(
        c.document,
        // `Document.adoptedStyleSheets` es de E2 (`cssom/sheets.rs::document_partial`, lo lee
        // la cascada por `document_sheets`); aquí solo el de `ShadowRoot`.
        HostClass::new("Document").static_method("parseHTMLUnsafe", 1, parse_html_unsafe),
    );
}

/// `Document.parseHTMLUnsafe(html)` (HTML §8.5.1): un documento HTML nuevo, parseado con
/// declarative shadow roots.
fn parse_html_unsafe(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let html = to_dom_string(agent, &a.arg(0))?;
    let opts = bf_html::ParseOptions {
        allow_declarative_shadow_roots: true,
        ..bf_html::ParseOptions::default()
    };
    let (doc, _) = bf_html::parse_document(&html.to_string_lossy(), &opts);
    let ctx = crate::WebContextHandle::current(agent);
    let i = ctx.get_mut(agent).add_document(doc);
    let root = ctx.get(agent).doc(i).root();
    Ok(wrap_node(agent, NodeRef::new(i, root)))
}

/// La clase de los wrappers de shadow roots (para `class_for`).
pub fn shadow_root_class_id(agent: &Agent) -> Option<HostClassId> {
    agent
        .embedder_data::<ShadowClasses>()
        .map(|c| c.shadow_root)
}

fn shadow_root_class(fragment: HostClassId) -> HostClass {
    let class = HostClass::new("ShadowRoot")
        .parent(fragment)
        .attribute("mode", Some(sr_mode), None)
        .attribute("delegatesFocus", Some(sr_delegates_focus), None)
        .attribute("slotAssignment", Some(sr_slot_assignment), None)
        .attribute("clonable", Some(sr_clonable), None)
        .attribute("serializable", Some(sr_serializable), None)
        .attribute("host", Some(sr_host), None)
        .attribute("innerHTML", Some(sr_inner_html), Some(sr_set_inner_html))
        .attribute("activeElement", Some(sr_active_element), None)
        .method("getHTML", 0, sr_get_html)
        .method("setHTMLUnsafe", 1, sr_set_html_unsafe)
        .attribute("adoptedStyleSheets", Some(adopted_get), Some(adopted_set))
        .method(
            "elementFromPoint",
            2,
            crate::cssom::view::element_from_point,
        )
        .method(
            "elementsFromPoint",
            2,
            crate::cssom::view::elements_from_point,
        );
    crate::dom::events::with_event_handlers(class, &["slotchange"])
}

// --- ShadowRoot ---

/// El shadow root de `this` (`TypeError` si no lo es).
fn this_shadow_root(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<NodeRef> {
    let n = this_node(agent, a)?;
    if !with_doc(agent, n.doc, |d| d.is_shadow_root(n.node)) {
        return agent.type_error("Illegal invocation");
    }
    Ok(n)
}

fn with_data<R>(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    f: impl FnOnce(&bf_dom::shadow::ShadowRootData) -> R,
) -> JsResult<R> {
    let n = this_shadow_root(agent, a)?;
    Ok(with_doc(agent, n.doc, |d| {
        f(d.shadow_root_data(n.node).expect("shadow root"))
    }))
}

fn sr_mode(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let m = with_data(agent, a, |d| d.mode)?;
    Ok(Value::str(match m {
        ShadowRootMode::Open => "open",
        ShadowRootMode::Closed => "closed",
    }))
}

fn sr_delegates_focus(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(with_data(agent, a, |d| d.delegates_focus)?))
}

fn sr_slot_assignment(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let m = with_data(agent, a, |d| d.slot_assignment)?;
    Ok(Value::str(match m {
        SlotAssignmentMode::Named => "named",
        SlotAssignmentMode::Manual => "manual",
    }))
}

fn sr_clonable(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(with_data(agent, a, |d| d.clonable)?))
}

fn sr_serializable(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(with_data(agent, a, |d| d.serializable)?))
}

fn sr_host(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_shadow_root(agent, a)?;
    let host = with_doc(agent, n.doc, |d| d.shadow_host(n.node)).expect("host");
    Ok(wrap_node(agent, NodeRef::new(n.doc, host)))
}

fn sr_inner_html(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_shadow_root(agent, a)?;
    let s = with_doc(agent, n.doc, |d| bf_html::dom::inner_html(d, n.node));
    Ok(from_dom_string(&s))
}

/// `[LegacyNullToEmptyString]`.
fn markup_arg(agent: &mut Agent, v: &Value) -> JsResult<DomString> {
    if v.is_null() {
        return Ok(DomString::new());
    }
    to_dom_string(agent, v)
}

/// El setter de `innerHTML` de `ShadowRoot`: el fragmento con el host como contexto.
fn sr_set_inner_html(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_shadow_root(agent, a)?;
    let markup = markup_arg(agent, &a.arg(0))?;
    replace_shadow_children(agent, n, &markup, false);
    Ok(Value::Undefined)
}

fn sr_set_html_unsafe(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_shadow_root(agent, a)?;
    let markup = to_dom_string(agent, &a.arg(0))?;
    replace_shadow_children(agent, n, &markup, true);
    Ok(Value::Undefined)
}

fn replace_shadow_children(agent: &mut Agent, sr: NodeRef, markup: &DomString, dsd: bool) {
    with_doc_mut(agent, sr.doc, |d| {
        let host = d.shadow_host(sr.node).expect("host");
        let fragment = bf_html::dom::parse_fragment_into_with(d, host, markup, dsd);
        d.replace_all(Some(fragment), sr.node);
    });
    mutated(agent);
    let roots = with_doc(agent, sr.doc, |d| d.child_nodes(sr.node));
    inserted_markup(agent, sr.doc, &roots);
}

fn sr_get_html(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_shadow_root(agent, a)?;
    get_html(agent, n, &a.arg(0))
}

/// `activeElement` de `DocumentOrShadowRoot` (HTML §6.6.3).
fn sr_active_element(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_shadow_root(agent, a)?;
    match active_element_for(agent, n) {
        Some(e) => Ok(wrap_node(agent, e)),
        None => Ok(Value::Null),
    }
}

/// "The activeElement getter steps" para el documento o shadow root `this`: el elemento
/// enfocado retargeteado contra `this`, si su raíz es `this` (sin foco, el `body` del
/// documento).
pub fn active_element_for(agent: &Agent, this: NodeRef) -> Option<NodeRef> {
    let focused = crate::html::misc::focused_element(agent).filter(|f| f.doc == this.doc);
    with_doc(agent, this.doc, |d| {
        let candidate = match focused {
            Some(f) => match d.retarget(
                Some(EventTarget::Node(f.node)),
                EventTarget::Node(this.node),
            ) {
                Some(EventTarget::Node(c)) => c,
                _ => d.root(),
            },
            None => d.root(),
        };
        if d.tree_root(candidate) != this.node {
            return None;
        }
        if !matches!(d.get(candidate).data, NodeData::Document) {
            return Some(NodeRef::new(this.doc, candidate));
        }
        d.body()
            .or_else(|| d.document_element())
            .map(|e| NodeRef::new(this.doc, e))
    })
}

// --- Element ---

/// `ShadowRootInit` (WebIDL lee los miembros en orden lexicográfico).
fn shadow_root_init(agent: &mut Agent, dict: &Value) -> JsResult<ShadowRootInit> {
    if !matches!(dict, Value::Object(_)) {
        return agent.type_error(
            "Failed to execute 'attachShadow' on 'Element': 1 argument required, but only 0 \
             present.",
        );
    }
    let clonable = dict_bool(agent, dict, "clonable")?;
    let delegates_focus = dict_bool(agent, dict, "delegatesFocus")?;
    let mode = dict_get(agent, dict, "mode")?;
    if mode.is_undefined() {
        return agent.type_error(
            "Failed to execute 'attachShadow' on 'Element': Failed to read the 'mode' property \
             from 'ShadowRootInit': Required member is undefined.",
        );
    }
    let mode = match to_dom_string(agent, &mode)?.to_string().as_str() {
        "open" => ShadowRootMode::Open,
        "closed" => ShadowRootMode::Closed,
        other => {
            return agent.type_error(&format!(
                "Failed to execute 'attachShadow' on 'Element': Failed to read the 'mode' \
                 property from 'ShadowRootInit': The provided value '{other}' is not a valid \
                 enum value of type ShadowRootMode."
            ));
        }
    };
    let serializable = dict_bool(agent, dict, "serializable")?;
    let slot = dict_get(agent, dict, "slotAssignment")?;
    let slot_assignment = if slot.is_undefined() {
        SlotAssignmentMode::Named
    } else {
        match to_dom_string(agent, &slot)?.to_string().as_str() {
            "named" => SlotAssignmentMode::Named,
            "manual" => SlotAssignmentMode::Manual,
            other => {
                return agent.type_error(&format!(
                    "Failed to execute 'attachShadow' on 'Element': The provided value \
                     '{other}' is not a valid enum value of type SlotAssignmentMode."
                ));
            }
        }
    };
    Ok(ShadowRootInit {
        mode,
        delegates_focus,
        slot_assignment,
        clonable,
        serializable,
    })
}

/// `attachShadow(init)` (DOM §4.9).
fn attach_shadow(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let init = shadow_root_init(agent, &a.arg(0))?;
    // Paso 3: la definición del custom element con `disabledFeatures` "shadow".
    if crate::html::custom_elements::shadow_disabled(agent, n) {
        return throw_dom(agent, DomException::NotSupportedError);
    }
    let r = with_doc_mut(agent, n.doc, |d| d.attach_shadow(n.node, init));
    match r {
        Ok(root) => {
            // Paso 7: *available to element internals* para los custom elements.
            if crate::html::custom_elements::is_custom(agent, n) {
                with_doc_mut(agent, n.doc, |d| {
                    if let Some(sd) = d.shadow_root_data_mut(root) {
                        sd.available_to_element_internals = true;
                    }
                });
            }
            mutated(agent);
            Ok(wrap_node(agent, NodeRef::new(n.doc, root)))
        }
        Err(e) => throw_dom(agent, e),
    }
}

/// `shadowRoot`: el shadow root si es abierto.
fn shadow_root_attr(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let sr = with_doc(agent, n.doc, |d| {
        d.shadow_root(n.node).filter(|&r| {
            d.shadow_root_data(r)
                .is_some_and(|x| x.mode == ShadowRootMode::Open)
        })
    });
    match sr {
        Some(r) => Ok(wrap_node(agent, NodeRef::new(n.doc, r))),
        None => Ok(Value::Null),
    }
}

/// `assignedSlot` (Slottable): "find a slot" con *open*.
fn assigned_slot(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    match with_doc(agent, n.doc, |d| d.find_a_slot(n.node, true)) {
        Some(s) => Ok(wrap_node(agent, NodeRef::new(n.doc, s))),
        None => Ok(Value::Null),
    }
}

fn element_get_html(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    get_html(agent, n, &a.arg(0))
}

/// `getHTML({serializableShadowRoots, shadowRoots})` (HTML §8.5.1).
fn get_html(agent: &mut Agent, n: NodeRef, options: &Value) -> JsResult<Value> {
    let serializable = match options {
        Value::Undefined | Value::Null => false,
        Value::Object(_) => dict_bool(agent, options, "serializableShadowRoots")?,
        _ => return agent.type_error("The provided value is not of type 'GetHTMLOptions'"),
    };
    let mut roots: Vec<NodeId> = Vec::new();
    if let Value::Object(_) = options {
        let list = dict_get(agent, options, "shadowRoots")?;
        if !list.is_undefined() {
            for v in sequence(agent, &list)? {
                let Some(r) = unwrap_node(agent, &v)
                    .filter(|r| with_doc(agent, r.doc, |d| d.is_shadow_root(r.node)))
                else {
                    return agent.type_error(
                        "Failed to execute 'getHTML': Failed to read the 'shadowRoots' property \
                         from 'GetHTMLOptions': Failed to convert value to 'ShadowRoot'.",
                    );
                };
                if r.doc == n.doc {
                    roots.push(r.node);
                }
            }
        }
    }
    let s = with_doc(agent, n.doc, |d| {
        bf_html::dom::serialize_children_with(
            d,
            n.node,
            &bf_html::dom::ShadowRoots {
                serializable,
                roots: &roots,
            },
        )
    });
    Ok(from_dom_string(&s))
}

/// Un `sequence<T>` de WebIDL: los valores del iterador de `v`.
fn sequence(agent: &mut Agent, v: &Value) -> JsResult<Vec<Value>> {
    if !matches!(v, Value::Object(_)) {
        return agent.type_error("The provided value cannot be converted to a sequence.");
    }
    agent.iterable_to_list(v)
}

fn element_set_html_unsafe(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let markup = to_dom_string(agent, &a.arg(0))?;
    with_doc_mut(agent, n.doc, |d| {
        bf_html::dom::set_html_unsafe(d, n.node, &markup)
    });
    mutated(agent);
    let target = with_doc(agent, n.doc, |d| {
        d.element(n.node)
            .filter(|e| e.is_html("template"))
            .and_then(|e| e.template_contents)
            .unwrap_or(n.node)
    });
    let roots = with_doc(agent, n.doc, |d| d.child_nodes(target));
    inserted_markup(agent, n.doc, &roots);
    Ok(Value::Undefined)
}

// --- HTMLSlotElement (HTML §4.12.4) ---

/// Las operaciones de `HTMLSlotElement`.
pub fn extend_slot(class: HostClass) -> HostClass {
    class
        .method("assignedNodes", 0, assigned_nodes)
        .method("assignedElements", 0, assigned_elements)
        .method("assign", 0, slot_assign)
}

/// `AssignedNodesOptions.flatten`.
fn flatten(agent: &mut Agent, v: &Value) -> JsResult<bool> {
    match v {
        Value::Undefined | Value::Null => Ok(false),
        Value::Object(_) => dict_bool(agent, v, "flatten"),
        _ => agent.type_error("The provided value is not of type 'AssignedNodesOptions'"),
    }
}

fn slot_nodes(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<(DocIndex, Vec<NodeId>)> {
    let n = this_node(agent, a)?;
    let flat = flatten(agent, &a.arg(0))?;
    let nodes = with_doc(agent, n.doc, |d| {
        if flat {
            d.find_flattened_slottables(n.node)
        } else {
            d.assigned_nodes(n.node)
        }
    });
    Ok((n.doc, nodes))
}

fn node_array(agent: &mut Agent, doc: DocIndex, nodes: &[NodeId]) -> JsResult<Value> {
    let values: Vec<Value> = nodes
        .iter()
        .map(|&x| wrap_node(agent, NodeRef::new(doc, x)))
        .collect();
    Ok(Value::Object(agent.create_array_from_list(&values)?))
}

fn assigned_nodes(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, nodes) = slot_nodes(agent, a)?;
    node_array(agent, doc, &nodes)
}

fn assigned_elements(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, nodes) = slot_nodes(agent, a)?;
    let elements: Vec<NodeId> = with_doc(agent, doc, |d| {
        nodes
            .into_iter()
            .filter(|&x| d.element(x).is_some())
            .collect()
    });
    node_array(agent, doc, &elements)
}

/// `assign(...nodes)`: elementos o textos.
fn slot_assign(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let mut nodes = Vec::new();
    for v in a.args.iter() {
        let Some(x) = unwrap_node(agent, v).filter(|x| {
            with_doc(agent, x.doc, |d| {
                matches!(d.get(x.node).data, NodeData::Element(_) | NodeData::Text(_))
            })
        }) else {
            return agent.type_error(
                "Failed to execute 'assign' on 'HTMLSlotElement': The provided value is not of \
                 type '(Element or Text)'.",
            );
        };
        // Los nodos de otro documento nunca son slottables de este slot.
        if x.doc == n.doc {
            nodes.push(x.node);
        }
    }
    with_doc_mut(agent, n.doc, |d| d.slot_assign(n.node, &nodes));
    mutated(agent);
    Ok(Value::Undefined)
}

// --- adoptedStyleSheets (CSSOM §6.2, DocumentOrShadowRoot) ---

/// El array de `adoptedStyleSheets` de cada shadow root (el del documento es de E2).
#[derive(Debug, Default)]
struct Adopted(std::collections::HashMap<NodeRef, bf_js::host::HostHandle>);

/// El shadow root de `this`.
fn this_root(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<NodeRef> {
    let n = this_node(agent, a)?;
    let ok = with_doc(agent, n.doc, |d| d.is_shadow_root(n.node));
    if !ok {
        return agent.type_error("Illegal invocation");
    }
    Ok(n)
}

fn adopted_array(agent: &mut Agent, root: NodeRef) -> JsResult<Value> {
    let realm = crate::WebContextHandle::current(agent).realm;
    if agent.realm_data::<Adopted>(realm).is_none() {
        agent.set_realm_data(realm, Adopted::default());
    }
    if let Some(h) = agent
        .realm_data::<Adopted>(realm)
        .and_then(|m| m.0.get(&root).copied())
    {
        return Ok(agent.host_pinned(h).unwrap_or_default());
    }
    let arr = Value::Object(agent.create_array_from_list(&[])?);
    let h = agent.host_pin(arr.clone());
    if let Some(m) = agent.realm_data_mut::<Adopted>(realm) {
        m.0.insert(root, h);
    }
    Ok(arr)
}

fn adopted_get(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let root = this_root(agent, a)?;
    adopted_array(agent, root)
}

/// El setter: una secuencia de hojas construidas (`NotAllowedError` si alguna no lo es).
fn adopted_set(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let root = this_root(agent, a)?;
    let values = sequence(agent, &a.arg(0))?;
    for v in &values {
        let Some(id) = crate::cssom::sheets::sheet_of(agent, v) else {
            return agent.type_error(
                "Failed to set the 'adoptedStyleSheets' property: Failed to convert value to                  'CSSStyleSheet'.",
            );
        };
        if !crate::cssom::sheets::constructed_sheet(agent, id).is_some_and(|(c, _)| c) {
            return crate::net::dom_error(
                agent,
                "NotAllowedError",
                "Can't adopt non-constructed stylesheets.",
            );
        }
    }
    let arr = Value::Object(agent.create_array_from_list(&values)?);
    let realm = crate::WebContextHandle::current(agent).realm;
    let h = agent.host_pin(arr);
    if agent.realm_data::<Adopted>(realm).is_none() {
        agent.set_realm_data(realm, Adopted::default());
    }
    let old = agent
        .realm_data_mut::<Adopted>(realm)
        .and_then(|m| m.0.insert(root, h));
    if let Some(old) = old {
        agent.host_unpin(old);
    }
    sync_adopted(agent);
    Ok(Value::Undefined)
}

/// Copia a cada shadow root el texto de sus `adoptedStyleSheets` (lo lee la cascada por
/// `loader::shadow_tree_stylesheets`).
pub fn sync_adopted(agent: &mut Agent) {
    let realm = crate::WebContextHandle::current(agent).realm;
    let Some(entries) = agent
        .realm_data::<Adopted>(realm)
        .map(|m| m.0.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>())
    else {
        return;
    };
    for (root, h) in entries {
        let arr = agent.host_pinned(h).unwrap_or_default();
        let Value::Object(o) = arr else { continue };
        let len = agent
            .get_str(o, "length")
            .ok()
            .and_then(|v| v.as_number())
            .unwrap_or(0.0) as usize;
        let mut texts = Vec::new();
        for i in 0..len {
            let Ok(v) = agent.get_str(o, &i.to_string()) else {
                continue;
            };
            if let Some(id) = crate::cssom::sheets::sheet_of(agent, &v)
                && let Some((true, text)) = crate::cssom::sheets::constructed_sheet(agent, id)
            {
                texts.push(text);
            }
        }
        let same = with_doc(agent, root.doc, |d| {
            d.adopted_style_texts(root.node) == texts.as_slice()
        });
        if !same {
            with_doc_mut(agent, root.doc, |d| {
                d.set_adopted_style_texts(root.node, texts)
            });
        }
    }
}

// --- slotchange ---

/// Dispara `slotchange` en los slots señalados de `doc` (el último paso de "notify mutation
/// observers", DOM §4.3).
pub fn fire_slotchange(agent: &mut Agent, doc: DocIndex, slots: Vec<NodeId>) {
    for s in slots {
        let init = EventInit {
            bubbles: true,
            ..EventInit::default()
        };
        crate::dom::events::fire_event(
            agent,
            doc,
            EventTarget::Node(s),
            "slotchange",
            init,
            EventKind::Event,
        );
    }
}
