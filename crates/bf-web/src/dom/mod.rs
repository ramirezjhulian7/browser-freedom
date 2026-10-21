//! El DOM Standard y el `Window` (HTML §7.3) como clases host. **Dueño: E3.**
//!
//! Aquí están las piezas del patrón que usan las demás áreas: [`DomClasses`] (los ids de las
//! clases, para heredar de ellas y para elegir la clase de cada nodo), [`wrap_node`] /
//! [`unwrap_node`] / [`this_node`], las conversiones de strings y [`throw_dom`].

pub mod abort;
pub mod collections;
pub mod document;
pub mod element;
pub mod events;
pub mod exception;
pub mod implementation;
pub mod node;
pub mod observer;
pub mod state;
pub mod traversal;
pub mod window;

use std::collections::HashMap;

use bf_dom::{DomException, Namespace, NodeData};
use bf_infra::DomString;
use bf_js::host::HostClassId;
use bf_js::string::JsString;
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, RealmId};

use crate::{DocIndex, NodeRef, WebContextHandle};

/// Los ids de las clases del DOM (uno por agente: `agent.embedder_data::<DomClasses>()`).
#[derive(Debug, Clone)]
pub struct DomClasses {
    pub event_target: HostClassId,
    pub node: HostClassId,
    pub document: HostClassId,
    pub document_fragment: HostClassId,
    pub document_type: HostClassId,
    pub element: HostClassId,
    pub character_data: HostClassId,
    pub text: HostClassId,
    pub cdata_section: HostClassId,
    pub comment: HostClassId,
    pub processing_instruction: HostClassId,
    pub attr: HostClassId,
    pub window: HostClassId,
    pub dom_exception: HostClassId,
    pub event: HostClassId,
    pub custom_event: HostClassId,
    pub ui_event: HostClassId,
    pub focus_event: HostClassId,
    pub mouse_event: HostClassId,
    pub keyboard_event: HostClassId,
    pub input_event: HostClassId,
    pub error_event: HostClassId,
    pub promise_rejection_event: HostClassId,
    pub node_list: HostClassId,
    pub html_collection: HostClassId,
    pub dom_token_list: HostClassId,
    pub named_node_map: HostClassId,
    pub mutation_observer: HostClassId,
    pub mutation_record: HostClassId,
    pub abort_controller: HostClassId,
    pub abort_signal: HostClassId,
    pub node_filter: HostClassId,
    pub node_iterator: HostClassId,
    pub tree_walker: HostClassId,
    pub dom_implementation: HostClassId,
    /// La clase de los elementos por (namespace, local name), p. ej. `HTMLDivElement` (la
    /// llena E1 con [`set_element_class`]).
    pub element_classes: HashMap<(Namespace, DomString), HostClassId>,
    /// La clase de los elementos HTML sin entrada propia (`HTMLUnknownElement`, de E1).
    pub html_default: Option<HostClassId>,
}

/// Los ids de las clases del DOM del agente.
pub fn classes(agent: &Agent) -> &DomClasses {
    agent
        .embedder_data::<DomClasses>()
        .expect("bf_web::dom::register no se llamó")
}

/// Asocia una clase a los elementos `(ns, local_name)` (para las interfaces HTML de E1).
pub fn set_element_class(agent: &mut Agent, ns: Namespace, local_name: &str, class: HostClassId) {
    if let Some(c) = agent.embedder_data_mut::<DomClasses>() {
        c.element_classes.insert((ns, local_name.into()), class);
    }
}

/// La clase por defecto de los elementos HTML (`HTMLUnknownElement`).
pub fn set_html_default_class(agent: &mut Agent, class: HostClassId) {
    if let Some(c) = agent.embedder_data_mut::<DomClasses>() {
        c.html_default = Some(class);
    }
}

/// Registra las clases del DOM (la primera vez) y convierte el global de `realm` en `Window`.
pub fn register(agent: &mut Agent, realm: RealmId, ctx: &WebContextHandle) {
    if agent.embedder_data::<DomClasses>().is_none() {
        let event_target = agent.register_host_class(events::event_target_class());
        let dom_exception = agent.register_host_class(exception::dom_exception_class());
        let n = agent.register_host_class(node::node_class(event_target));
        let document = agent.register_host_class(document::document_class(n));
        let document_fragment = agent.register_host_class(
            bf_js::host::HostClass::new("DocumentFragment")
                .parent(n)
                .constructor(document::document_fragment_ctor, 0),
        );
        let document_type = agent.register_host_class(node::document_type_class(n));
        let element = agent.register_host_class(element::element_class(n));
        let character_data = agent.register_host_class(node::character_data_class(n));
        let text = agent.register_host_class(
            bf_js::host::HostClass::new("Text")
                .parent(character_data)
                .constructor(document::text_ctor, 0),
        );
        let cdata_section =
            agent.register_host_class(bf_js::host::HostClass::new("CDATASection").parent(text));
        let comment = agent.register_host_class(
            bf_js::host::HostClass::new("Comment")
                .parent(character_data)
                .constructor(document::comment_ctor, 0),
        );
        let processing_instruction = agent.register_host_class(
            bf_js::host::HostClass::new("ProcessingInstruction")
                .parent(character_data)
                .attribute("target", Some(node::pi_target), None),
        );
        let attr = agent.register_host_class(node::attr_class(n));
        let window = agent.register_host_class(window::window_class(event_target));
        let event = agent.register_host_class(events::event_class());
        let custom_event = agent.register_host_class(events::custom_event_class(event));
        let ui_event = agent.register_host_class(events::ui_event_class(event));
        let focus_event = agent.register_host_class(events::focus_event_class(ui_event));
        let mouse_event = agent.register_host_class(events::mouse_event_class(ui_event));
        let keyboard_event = agent.register_host_class(events::keyboard_event_class(ui_event));
        let input_event = agent.register_host_class(events::input_event_class(ui_event));
        let error_event = agent.register_host_class(events::error_event_class(event));
        let promise_rejection_event =
            agent.register_host_class(events::promise_rejection_event_class(event));
        let node_list = agent.register_host_class(collections::node_list_class());
        let html_collection = agent.register_host_class(collections::html_collection_class());
        let dom_token_list = agent.register_host_class(collections::dom_token_list_class());
        let named_node_map = agent.register_host_class(collections::named_node_map_class());
        let mutation_observer = agent.register_host_class(observer::mutation_observer_class());
        let mutation_record = agent.register_host_class(observer::mutation_record_class());
        let abort_controller = agent.register_host_class(abort::abort_controller_class());
        let abort_signal = agent.register_host_class(abort::abort_signal_class(event_target));
        let node_filter = agent.register_host_class(traversal::node_filter_class());
        let node_iterator = agent.register_host_class(traversal::node_iterator_class());
        let tree_walker = agent.register_host_class(traversal::tree_walker_class());
        let dom_implementation =
            agent.register_host_class(implementation::dom_implementation_class());
        agent.set_embedder_data(DomClasses {
            event_target,
            node: n,
            document,
            document_fragment,
            document_type,
            element,
            character_data,
            text,
            cdata_section,
            comment,
            processing_instruction,
            attr,
            window,
            dom_exception,
            event,
            custom_event,
            ui_event,
            focus_event,
            mouse_event,
            keyboard_event,
            input_event,
            error_event,
            promise_rejection_event,
            node_list,
            html_collection,
            dom_token_list,
            named_node_map,
            mutation_observer,
            mutation_record,
            abort_controller,
            abort_signal,
            node_filter,
            node_iterator,
            tree_walker,
            dom_implementation,
            element_classes: HashMap::new(),
            html_default: None,
        });
    }
    exception::finish_realm(agent, realm);
    collections::finish_realm(agent, realm);
    window::install(agent, realm, ctx);
}

// --- El patrón de los wrappers ---

/// La clase de un nodo según su tipo (y, para los elementos, su nombre).
fn class_for(agent: &Agent, n: NodeRef) -> HostClassId {
    let c = classes(agent);
    let ctx = WebContextHandle::current(agent).get(agent);
    match &ctx.doc(n.doc).get(n.node).data {
        NodeData::Document => c.document,
        NodeData::DocumentFragment => c.document_fragment,
        NodeData::Doctype { .. } => c.document_type,
        NodeData::Element(e) => match c.element_classes.get(&(e.ns, e.name.clone())) {
            Some(k) => *k,
            None if e.ns == Namespace::Html => c.html_default.unwrap_or(c.element),
            None => c.element,
        },
        NodeData::Attr(_) => c.attr,
        NodeData::Text(_) => c.text,
        NodeData::CdataSection(_) => c.cdata_section,
        NodeData::Comment(_) => c.comment,
        NodeData::ProcessingInstruction { .. } => c.processing_instruction,
    }
}

/// El wrapper de un nodo (siempre el mismo objeto mientras JavaScript lo alcance).
pub fn wrap_node(agent: &mut Agent, n: NodeRef) -> Value {
    let realm = agent.current_realm;
    if let Some(o) = agent.host_lookup(realm, n.key()) {
        return Value::Object(o);
    }
    let class = class_for(agent, n);
    Value::Object(agent.host_wrap(realm, n.key(), class, Box::new(n)))
}

/// El wrapper de un nodo opcional (`null` si no hay).
pub fn wrap_opt(agent: &mut Agent, doc: DocIndex, n: Option<bf_dom::NodeId>) -> Value {
    match n {
        Some(n) => wrap_node(agent, NodeRef::new(doc, n)),
        None => Value::Null,
    }
}

/// El nodo de un wrapper.
pub fn unwrap_node(agent: &Agent, v: &Value) -> Option<NodeRef> {
    agent.host_unwrap::<NodeRef>(v).copied()
}

/// El nodo de `this` (el trampolín ya hizo el brand check).
pub fn this_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<NodeRef> {
    match unwrap_node(agent, &a.this) {
        Some(n) => Ok(n),
        None => agent.type_error("Illegal invocation"),
    }
}

/// Un argumento que debe ser un `Node` (TypeError si no lo es).
pub fn arg_node(agent: &mut Agent, v: &Value, what: &str) -> JsResult<NodeRef> {
    match unwrap_node(agent, v) {
        Some(n) => Ok(n),
        None => agent.type_error(&format!("{what} is not of type 'Node'")),
    }
}

/// Un `Node?`.
pub fn arg_opt_node(agent: &mut Agent, v: &Value, what: &str) -> JsResult<Option<NodeRef>> {
    if v.is_nullish() {
        return Ok(None);
    }
    arg_node(agent, v, what).map(Some)
}

/// `DOMString` (ToString) como `DomString`.
pub fn to_dom_string(agent: &mut Agent, v: &Value) -> JsResult<DomString> {
    let s = agent.to_string(v)?;
    Ok(DomString::from_utf16(s.units()))
}

/// `DOMString?`.
pub fn to_opt_dom_string(agent: &mut Agent, v: &Value) -> JsResult<Option<DomString>> {
    if v.is_nullish() {
        return Ok(None);
    }
    to_dom_string(agent, v).map(Some)
}

/// Un `DomString` como valor de JavaScript.
pub fn from_dom_string(s: &DomString) -> Value {
    Value::String(JsString::from_units(s.to_utf16()))
}

/// Un `DomString?` (`null` si no hay).
pub fn from_opt_dom_string(s: Option<&DomString>) -> Value {
    s.map_or(Value::Null, from_dom_string)
}

/// Lanza un error del DOM (`DOMException`, o `TypeError` si es ese).
pub fn throw_dom<T>(agent: &mut Agent, e: DomException) -> JsResult<T> {
    exception::throw(agent, e)
}

/// Registra una mutación hecha desde JavaScript (re-render) y agenda la entrega de los
/// `MutationObserver` de los documentos que encolaron records.
pub fn mutated(agent: &mut Agent) {
    let ctx = WebContextHandle::current(agent);
    ctx.get_mut(agent).mutated();
    let n = ctx.get(agent).documents.len();
    for doc in 0..n as DocIndex {
        observer::schedule_delivery(agent, doc);
    }
}

/// Después de que un algoritmo de `bf-dom` corrió con script (dispatch, abort): agenda la
/// entrega de los observers de `doc`.
pub fn after_dom_mutation(agent: &mut Agent, doc: DocIndex) {
    observer::schedule_delivery(agent, doc);
}
