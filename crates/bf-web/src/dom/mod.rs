//! El DOM Standard y el `Window` (HTML §7.3) como clases host. **Dueño: E3.**
//!
//! Aquí están las piezas del patrón que usan las demás áreas: [`DomClasses`] (los ids de las
//! clases, para heredar de ellas y para elegir la clase de cada nodo), [`wrap_node`] /
//! [`unwrap_node`] / [`this_node`], las conversiones de strings y [`throw_dom`].

pub mod document;
pub mod element;
pub mod exception;
pub mod node;
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
        let event_target = agent.register_host_class(node::event_target_class());
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
            element_classes: HashMap::new(),
            html_default: None,
        });
    }
    exception::finish_realm(agent, realm);
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

/// Registra una mutación hecha desde JavaScript (re-render).
pub fn mutated(agent: &mut Agent) {
    WebContextHandle::current(agent).get_mut(agent).mutated();
}
