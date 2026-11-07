//! Custom elements (HTML §4.13): `CustomElementRegistry` (`customElements.define`, `get`,
//! `getName`, `whenDefined`, `upgrade`), los constructores HTML (`new MyElement()` con
//! `super()`), el upgrade, la cola de reacciones (`connectedCallback`, `disconnectedCallback`,
//! `adoptedCallback`, `attributeChangedCallback`) y `ElementInternals` básico. **E1.**
//!
//! Las reacciones se encolan con los eventos que registra `bf-dom`
//! ([`bf_dom::Document::take_custom_element_events`]) y se invocan al final de cada operación
//! del DOM ([`process_reactions`], desde `mutated`), una aproximación a `[CEReactions]`; las
//! del parser se procesan antes de cada script y al terminar.

use std::collections::{HashMap, VecDeque};

use bf_dom::custom::{CustomElementEvent, CustomElementState};
use bf_dom::{DomString, Namespace, NodeData, NodeId};
use bf_js::heap::Gc;
use bf_js::host::{HostClass, HostClassId, HostHandle};
use bf_js::object::Object;
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, RealmId, Throw};

use crate::dom::{from_dom_string, this_node, to_dom_string, unwrap_node, wrap_node};
use crate::{NodeRef, WebContextHandle, with_doc, with_doc_mut};

// --- Definiciones y registro ---

/// Un *custom element definition* (§4.13.2).
struct Definition {
    name: String,
    local_name: String,
    constructor: HostHandle,
    observed: Vec<String>,
    connected: Option<HostHandle>,
    disconnected: Option<HostHandle>,
    adopted: Option<HostHandle>,
    attribute_changed: Option<HostHandle>,
    form_associated: bool,
    disable_internals: bool,
    disable_shadow: bool,
    /// La *construction stack*: `None` es el *already constructed marker*.
    construction_stack: Vec<Option<NodeRef>>,
}

/// Una reacción encolada (§4.13.6).
#[derive(Debug, Clone)]
enum Reaction {
    Upgrade(usize),
    Connected,
    Disconnected,
    Adopted(crate::DocIndex, crate::DocIndex),
    AttributeChanged {
        name: DomString,
        old: Option<DomString>,
        new: Option<DomString>,
        namespace: Option<Namespace>,
    },
}

/// El `CustomElementRegistry` de una ventana.
#[derive(Default)]
struct Registry {
    defs: Vec<Definition>,
    running: bool,
    /// Las promesas de `whenDefined` pendientes.
    when_defined: HashMap<String, HostHandle>,
    /// La *custom element definition* de cada elemento.
    element_defs: HashMap<NodeRef, usize>,
    /// La cola de elementos con reacciones y las reacciones de cada uno.
    queue: VecDeque<NodeRef>,
    reactions: HashMap<NodeRef, VecDeque<Reaction>>,
    /// El objeto `customElements`.
    object: Option<HostHandle>,
    /// Los `ElementInternals` ya creados.
    internals: HashMap<NodeRef, HostHandle>,
}

#[derive(Debug, Clone, Copy)]
struct CeClasses {
    registry: HostClassId,
    internals: HostClassId,
    state_set: HostClassId,
}

fn realm(agent: &Agent) -> RealmId {
    WebContextHandle::current(agent).realm
}

fn registry(agent: &mut Agent) -> &mut Registry {
    let r = realm(agent);
    if agent.realm_data::<Registry>(r).is_none() {
        agent.set_realm_data(r, Registry::default());
    }
    agent.realm_data_mut::<Registry>(r).expect("Registry")
}

fn registry_ref(agent: &Agent) -> Option<&Registry> {
    agent.realm_data::<Registry>(realm(agent))
}

/// Registra las clases (una vez por agente).
pub fn register(agent: &mut Agent) {
    if agent.embedder_data::<CeClasses>().is_some() {
        return;
    }
    let registry = agent.register_host_class(
        HostClass::new("CustomElementRegistry")
            .method("define", 2, define)
            .method("get", 1, get)
            .method("getName", 1, get_name)
            .method("whenDefined", 1, when_defined)
            .method("upgrade", 1, upgrade_method),
    );
    let mut internals = HostClass::new("ElementInternals")
        .attribute("shadowRoot", Some(internals_shadow_root), None)
        .attribute("states", Some(internals_states), None)
        .attribute("form", Some(internals_form), None)
        .attribute("willValidate", Some(internals_false), None)
        .attribute("validationMessage", Some(internals_empty), None)
        .attribute("labels", Some(internals_labels), None)
        .method("setFormValue", 1, internals_set_form_value)
        .method("setValidity", 0, internals_noop)
        .method("checkValidity", 0, internals_true)
        .method("reportValidity", 0, internals_true);
    for name in ARIA_ATTRIBUTES {
        internals = internals.attribute(name, Some(aria_get), Some(aria_set));
    }
    let internals = agent.register_host_class(internals);
    let state_set = agent.register_host_class(
        HostClass::new("CustomStateSet")
            .method("add", 1, states_add)
            .method("delete", 1, states_delete)
            .method("has", 1, states_has)
            .method("clear", 0, states_clear)
            .method("forEach", 1, states_for_each)
            .attribute("size", Some(states_size), None),
    );
    agent.set_embedder_data(CeClasses {
        registry,
        internals,
        state_set,
    });
    let window = crate::dom::classes(agent).window;
    agent.host_extend_class(
        window,
        HostClass::new("Window").attribute("customElements", Some(custom_elements), None),
    );
}

fn classes(agent: &Agent) -> CeClasses {
    *agent.embedder_data::<CeClasses>().expect("CeClasses")
}

/// `window.customElements`.
fn custom_elements(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    if let Some(h) = registry(agent).object {
        return Ok(agent.host_pinned(h).unwrap_or_default());
    }
    let class = classes(agent).registry;
    let o = Value::Object(agent.host_create(class, Box::new(())));
    let h = agent.host_pin(o.clone());
    registry(agent).object = Some(h);
    Ok(o)
}

fn pinned(agent: &Agent, h: HostHandle) -> Value {
    agent.host_pinned(h).unwrap_or_default()
}

/// "Look up a custom element definition" (§4.13.4) en el documento de la ventana.
fn lookup(
    agent: &Agent,
    doc: crate::DocIndex,
    ns: Namespace,
    local: &str,
    is: Option<&str>,
) -> Option<usize> {
    if ns != Namespace::Html || doc != 0 {
        return None;
    }
    let reg = registry_ref(agent)?;
    if let Some(i) = reg
        .defs
        .iter()
        .position(|d| d.name == local && d.local_name == local)
    {
        return Some(i);
    }
    let is = is?;
    reg.defs
        .iter()
        .position(|d| d.name == is && d.local_name == local)
}

/// La definición para un elemento existente (según su nombre e *is value*).
fn lookup_element(agent: &Agent, n: NodeRef) -> Option<usize> {
    let (ns, local, is) = with_doc(agent, n.doc, |d| {
        let e = d.element(n.node)?;
        Some((
            e.ns,
            e.name.to_string(),
            d.is_value(n.node).map(|v| v.to_string()),
        ))
    })?;
    lookup(agent, n.doc, ns, &local, is.as_deref())
}

// --- define (§4.13.4) ---

fn get_callback(agent: &mut Agent, proto: Gc<Object>, name: &str) -> JsResult<Option<HostHandle>> {
    let v = agent.get_str(proto, name)?;
    if v.is_undefined() {
        return Ok(None);
    }
    if !agent.is_callable(&v) {
        return agent.type_error(&format!(
            "Failed to execute 'define' on 'CustomElementRegistry': The '{name}' property on \
             the prototype is not a function."
        ));
    }
    Ok(Some(agent.host_pin(v)))
}

fn string_sequence(agent: &mut Agent, v: &Value) -> JsResult<Vec<String>> {
    if v.is_undefined() {
        return Ok(Vec::new());
    }
    if !matches!(v, Value::Object(_)) {
        return agent.type_error("The provided value cannot be converted to a sequence.");
    }
    let items = agent.iterable_to_list(v)?;
    let mut out = Vec::with_capacity(items.len());
    for i in items {
        out.push(to_dom_string(agent, &i)?.to_string());
    }
    Ok(out)
}

fn define(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = to_dom_string(agent, &a.arg(0))?.to_string();
    let constructor = a.arg(1);
    // 1.
    if !agent.is_constructor(&constructor) {
        return agent.type_error(
            "Failed to execute 'define' on 'CustomElementRegistry': parameter 2 is not a \
             constructor.",
        );
    }
    // 2.
    if !bf_dom::is_valid_custom_element_name(&name) {
        return crate::net::dom_error(
            agent,
            "SyntaxError",
            &format!(
                "Failed to execute 'define' on 'CustomElementRegistry': \"{name}\" is not a valid custom element name"
            ),
        );
    }
    // 3-4.
    let dup_name = registry_ref(agent).is_some_and(|r| r.defs.iter().any(|d| d.name == name));
    if dup_name {
        return crate::net::dom_error(
            agent,
            "NotSupportedError",
            &format!(
                "Failed to execute 'define' on 'CustomElementRegistry': the name \"{name}\" has already been used with this registry"
            ),
        );
    }
    let dup_ctor = {
        let defs: Vec<HostHandle> = registry(agent).defs.iter().map(|d| d.constructor).collect();
        defs.into_iter()
            .any(|h| agent.same_value(&pinned(agent, h), &constructor))
    };
    if dup_ctor {
        return crate::net::dom_error(
            agent,
            "NotSupportedError",
            "Failed to execute 'define' on 'CustomElementRegistry': this constructor has already been used with this registry",
        );
    }
    // 5-7.
    let mut local_name = name.clone();
    let options = a.arg(2);
    let extends = match &options {
        Value::Object(o) => {
            let v = agent.get_str(*o, "extends")?;
            if v.is_undefined() {
                None
            } else {
                Some(to_dom_string(agent, &v)?.to_string())
            }
        }
        Value::Undefined | Value::Null => None,
        _ => {
            return agent
                .type_error("The provided value is not of type 'ElementDefinitionOptions'.");
        }
    };
    if let Some(ext) = &extends {
        if bf_dom::is_valid_custom_element_name(ext) {
            return crate::net::dom_error(
                agent,
                "NotSupportedError",
                &format!(
                    "Failed to execute 'define' on 'CustomElementRegistry': \"{ext}\" is a valid custom element name"
                ),
            );
        }
        if super::table::interface_for(ext) == "HTMLUnknownElement" {
            return crate::net::dom_error(
                agent,
                "NotSupportedError",
                &format!(
                    "Failed to execute 'define' on 'CustomElementRegistry': \"{ext}\" is an HTMLUnknownElement"
                ),
            );
        }
        local_name = ext.clone();
    }
    // 8-9.
    if registry(agent).running {
        return crate::net::dom_error(
            agent,
            "NotSupportedError",
            "Failed to execute 'define' on 'CustomElementRegistry': an element definition is already being processed",
        );
    }
    registry(agent).running = true;
    // 10-11.
    let Value::Object(ctor) = constructor.clone() else {
        unreachable!("constructor");
    };
    let parts = (|| -> JsResult<_> {
        let proto = agent.get_str(ctor, "prototype")?;
        let Value::Object(proto) = proto else {
            return agent.type_error(
                "Failed to execute 'define' on 'CustomElementRegistry': The 'prototype' \
                 property of the constructor is not an object.",
            );
        };
        let connected = get_callback(agent, proto, "connectedCallback")?;
        let disconnected = get_callback(agent, proto, "disconnectedCallback")?;
        let adopted = get_callback(agent, proto, "adoptedCallback")?;
        let attribute_changed = get_callback(agent, proto, "attributeChangedCallback")?;
        let _connected_move = get_callback(agent, proto, "connectedMoveCallback")?;
        let mut observed = Vec::new();
        if attribute_changed.is_some() {
            let v = agent.get_str(ctor, "observedAttributes")?;
            observed = string_sequence(agent, &v)?;
        }
        let disabled = agent.get_str(ctor, "disabledFeatures")?;
        let disabled = string_sequence(agent, &disabled)?;
        let form_associated = agent.get_str(ctor, "formAssociated")?;
        let form_associated = bf_js::host::webidl::to_boolean(agent, &form_associated);
        if form_associated {
            for cb in [
                "formAssociatedCallback",
                "formResetCallback",
                "formDisabledCallback",
                "formStateRestoreCallback",
            ] {
                if let Some(h) = get_callback(agent, proto, cb)? {
                    agent.host_unpin(h);
                }
            }
        }
        Ok((
            connected,
            disconnected,
            adopted,
            attribute_changed,
            observed,
            disabled,
            form_associated,
        ))
    })();
    registry(agent).running = false;
    let (connected, disconnected, adopted, attribute_changed, observed, disabled, form_associated) =
        parts?;
    // 13-14.
    let ctor_handle = agent.host_pin(constructor.clone());
    let index = {
        let reg = registry(agent);
        reg.defs.push(Definition {
            name: name.clone(),
            local_name: local_name.clone(),
            constructor: ctor_handle,
            observed,
            connected,
            disconnected,
            adopted,
            attribute_changed,
            form_associated,
            disable_internals: disabled.iter().any(|f| f == "internals"),
            disable_shadow: disabled.iter().any(|f| f == "shadow"),
            construction_stack: Vec::new(),
        });
        reg.defs.len() - 1
    };
    // Desde ahora el documento registra los eventos de conexión.
    with_doc_mut(agent, 0, |d| d.enable_custom_element_events());
    // 15-17: los candidatos del documento.
    let candidates: Vec<NodeId> = with_doc(agent, 0, |d| {
        d.shadow_including_inclusive_descendants(d.root())
            .into_iter()
            .filter(|&n| {
                d.element(n).is_some_and(|e| {
                    e.ns == Namespace::Html
                        && e.name == local_name.as_str()
                        && (extends.is_none() || d.is_value(n).is_some_and(|v| *v == name.as_str()))
                })
            })
            .collect()
    });
    for n in candidates {
        enqueue(agent, NodeRef::new(0, n), Reaction::Upgrade(index));
    }
    // 18.
    if let Some(h) = registry(agent).when_defined.remove(&name) {
        let p = pinned(agent, h);
        agent.host_unpin(h);
        if let Value::Object(p) = p {
            let cap = bf_js::builtins::promise::Capability::Internal(p);
            agent.capability_resolve(&cap, constructor)?;
        }
    }
    process_reactions(agent);
    Ok(Value::Undefined)
}

fn get(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = to_dom_string(agent, &a.arg(0))?.to_string();
    let h = registry_ref(agent).and_then(|r| {
        r.defs
            .iter()
            .find(|d| d.name == name)
            .map(|d| d.constructor)
    });
    Ok(h.map_or(Value::Undefined, |h| pinned(agent, h)))
}

fn get_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let ctor = a.arg(0);
    if !agent.is_callable(&ctor) {
        return agent.type_error(
            "Failed to execute 'getName' on 'CustomElementRegistry': parameter 1 is not of type \
             'Function'.",
        );
    }
    let defs: Vec<(String, HostHandle)> = registry(agent)
        .defs
        .iter()
        .map(|d| (d.name.clone(), d.constructor))
        .collect();
    for (name, h) in defs {
        if agent.same_value(&pinned(agent, h), &ctor) {
            return Ok(Value::str(&name));
        }
    }
    Ok(Value::Null)
}

fn when_defined(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = to_dom_string(agent, &a.arg(0))?.to_string();
    let promise_ctor = Value::Object(agent.intrinsics().promise_ctor);
    let cap = agent.new_promise_capability(&promise_ctor)?;
    if !bf_dom::is_valid_custom_element_name(&name) {
        let e = crate::dom::exception::create(
            agent,
            "SyntaxError",
            &format!("\"{name}\" is not a valid custom element name"),
        );
        agent.capability_reject(&cap, e)?;
        return Ok(cap.promise());
    }
    let defined = registry_ref(agent).and_then(|r| {
        r.defs
            .iter()
            .find(|d| d.name == name)
            .map(|d| d.constructor)
    });
    if let Some(h) = defined {
        let c = pinned(agent, h);
        agent.capability_resolve(&cap, c)?;
        return Ok(cap.promise());
    }
    if let Some(h) = registry(agent).when_defined.get(&name).copied() {
        return Ok(pinned(agent, h));
    }
    let p = cap.promise();
    let h = agent.host_pin(p.clone());
    registry(agent).when_defined.insert(name, h);
    Ok(p)
}

/// `upgrade(root)`: "try to upgrade" cada elemento shadow-including de `root`.
fn upgrade_method(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(root) = unwrap_node(agent, &a.arg(0)) else {
        return agent.type_error(
            "Failed to execute 'upgrade' on 'CustomElementRegistry': parameter 1 is not of type \
             'Node'.",
        );
    };
    let nodes = with_doc(agent, root.doc, |d| {
        d.shadow_including_inclusive_descendants(root.node)
    });
    for n in nodes {
        let r = NodeRef::new(root.doc, n);
        try_to_upgrade(agent, r, true);
    }
    process_reactions(agent);
    Ok(Value::Undefined)
}

// --- Reacciones (§4.13.6) ---

fn enqueue(agent: &mut Agent, el: NodeRef, reaction: Reaction) {
    let reg = registry(agent);
    let list = reg.reactions.entry(el).or_default();
    list.push_back(reaction);
    if !reg.queue.contains(&el) {
        reg.queue.push_back(el);
    }
}

/// "Enqueue a custom element callback reaction" si la definición tiene el callback.
fn enqueue_callback(agent: &mut Agent, el: NodeRef, reaction: Reaction) {
    let Some(def) = registry_ref(agent).and_then(|r| r.element_defs.get(&el).copied()) else {
        return;
    };
    let reg = registry_ref(agent).expect("registry");
    let d = &reg.defs[def];
    let has = match &reaction {
        Reaction::Connected => d.connected.is_some(),
        Reaction::Disconnected => d.disconnected.is_some(),
        Reaction::Adopted(..) => d.adopted.is_some(),
        Reaction::AttributeChanged {
            name, namespace, ..
        } => {
            d.attribute_changed.is_some()
                && namespace.is_none()
                && d.observed.iter().any(|o| *name == o.as_str())
                || (d.attribute_changed.is_some()
                    && namespace.is_some()
                    && d.observed.iter().any(|o| *name == o.as_str()))
        }
        Reaction::Upgrade(_) => true,
    };
    if has {
        enqueue(agent, el, reaction);
    }
}

/// "Try to upgrade" `el`: si hay definición, encola (o hace, con `sync`) el upgrade.
fn try_to_upgrade(agent: &mut Agent, el: NodeRef, sync: bool) {
    if !with_doc(agent, el.doc, |d| d.element(el.node).is_some()) {
        return;
    }
    let state = with_doc(agent, el.doc, |d| d.custom_element_state(el.node));
    if !matches!(
        state,
        CustomElementState::Undefined | CustomElementState::Uncustomized
    ) {
        return;
    }
    if let Some(def) = lookup_element(agent, el) {
        if sync {
            if let Err(Throw::Value(e)) = upgrade(agent, el, def) {
                crate::dom::events::report_exception(agent, &e);
            }
        } else {
            enqueue(agent, el, Reaction::Upgrade(def));
        }
    }
}

/// Convierte los eventos de `bf-dom` en reacciones e invoca la cola. Lo llaman las
/// operaciones del DOM al terminar (`mutated`) y el parser antes de cada script.
pub fn process_reactions(agent: &mut Agent) {
    if registry_ref(agent).is_none_or(|r| r.defs.is_empty()) {
        return;
    }
    let n = WebContextHandle::current(agent).get(agent).documents.len();
    for doc in 0..n as crate::DocIndex {
        let events = with_doc_mut(agent, doc, |d| d.take_custom_element_events());
        for ev in events {
            match ev {
                CustomElementEvent::Connected(node) => {
                    let el = NodeRef::new(doc, node);
                    if with_doc(agent, doc, |d| d.is_custom_element(node)) {
                        enqueue_callback(agent, el, Reaction::Connected);
                    } else {
                        try_to_upgrade(agent, el, false);
                    }
                }
                CustomElementEvent::Disconnected(node) => {
                    let el = NodeRef::new(doc, node);
                    if with_doc(agent, doc, |d| d.is_custom_element(node)) {
                        enqueue_callback(agent, el, Reaction::Disconnected);
                    }
                }
                CustomElementEvent::AttributeChanged {
                    element,
                    local_name,
                    namespace,
                    old_value,
                    new_value,
                } => {
                    enqueue_callback(
                        agent,
                        NodeRef::new(doc, element),
                        Reaction::AttributeChanged {
                            name: local_name,
                            old: old_value,
                            new: new_value,
                            namespace,
                        },
                    );
                }
            }
        }
    }
    invoke_reactions(agent);
}

/// "Invoke custom element reactions" de la cola.
fn invoke_reactions(agent: &mut Agent) {
    loop {
        let Some(el) = registry(agent).queue.pop_front() else {
            return;
        };
        while let Some(reaction) = registry(agent)
            .reactions
            .get_mut(&el)
            .and_then(VecDeque::pop_front)
        {
            let r = match reaction {
                Reaction::Upgrade(def) => upgrade(agent, el, def),
                other => invoke_callback(agent, el, other),
            };
            if let Err(Throw::Value(e)) = r {
                crate::dom::events::report_exception(agent, &e);
            }
        }
        registry(agent).reactions.remove(&el);
        // Las reacciones encoladas por los callbacks (eventos nuevos del DOM).
        let pending = (0..WebContextHandle::current(agent).get(agent).documents.len()).any(|d| {
            with_doc(agent, d as crate::DocIndex, |x| {
                x.has_custom_element_events()
            })
        });
        if pending {
            let n = WebContextHandle::current(agent).get(agent).documents.len();
            for doc in 0..n as crate::DocIndex {
                let events = with_doc_mut(agent, doc, |d| d.take_custom_element_events());
                for ev in events {
                    requeue_event(agent, doc, ev);
                }
            }
        }
    }
}

fn requeue_event(agent: &mut Agent, doc: crate::DocIndex, ev: CustomElementEvent) {
    match ev {
        CustomElementEvent::Connected(node) => {
            let el = NodeRef::new(doc, node);
            if with_doc(agent, doc, |d| d.is_custom_element(node)) {
                enqueue_callback(agent, el, Reaction::Connected);
            } else {
                try_to_upgrade(agent, el, false);
            }
        }
        CustomElementEvent::Disconnected(node) => {
            let el = NodeRef::new(doc, node);
            if with_doc(agent, doc, |d| d.is_custom_element(node)) {
                enqueue_callback(agent, el, Reaction::Disconnected);
            }
        }
        CustomElementEvent::AttributeChanged {
            element,
            local_name,
            namespace,
            old_value,
            new_value,
        } => enqueue_callback(
            agent,
            NodeRef::new(doc, element),
            Reaction::AttributeChanged {
                name: local_name,
                old: old_value,
                new: new_value,
                namespace,
            },
        ),
    }
}

fn opt_string(s: &Option<DomString>) -> Value {
    s.as_ref().map_or(Value::Null, from_dom_string)
}

fn invoke_callback(agent: &mut Agent, el: NodeRef, reaction: Reaction) -> JsResult<()> {
    let Some(def) = registry_ref(agent).and_then(|r| r.element_defs.get(&el).copied()) else {
        return Ok(());
    };
    let (cb, args) = {
        let reg = registry_ref(agent).expect("registry");
        let d = &reg.defs[def];
        match &reaction {
            Reaction::Connected => (d.connected, Vec::new()),
            Reaction::Disconnected => (d.disconnected, Vec::new()),
            Reaction::Adopted(..) => (d.adopted, Vec::new()),
            Reaction::AttributeChanged { .. } => (d.attribute_changed, Vec::new()),
            Reaction::Upgrade(_) => (None, Vec::new()),
        }
    };
    let Some(cb) = cb else {
        return Ok(());
    };
    let mut args: Vec<Value> = args;
    match reaction {
        Reaction::Adopted(old, new) => {
            for doc in [old, new] {
                let root = with_doc(agent, doc, |d| d.root());
                args.push(wrap_node(agent, NodeRef::new(doc, root)));
            }
        }
        Reaction::AttributeChanged {
            name,
            old,
            new,
            namespace,
        } => {
            args.push(from_dom_string(&name));
            args.push(opt_string(&old));
            args.push(opt_string(&new));
            args.push(namespace.map_or(Value::Null, |n| Value::str(n.uri())));
        }
        _ => {}
    }
    let f = pinned(agent, cb);
    let this = wrap_node(agent, el);
    agent.call(&f, this, &args)?;
    Ok(())
}

// --- Upgrade (§4.13.5) ---

/// "Upgrade an element".
fn upgrade(agent: &mut Agent, el: NodeRef, def: usize) -> JsResult<()> {
    // 1.
    let state = with_doc(agent, el.doc, |d| d.custom_element_state(el.node));
    if !matches!(
        state,
        CustomElementState::Undefined | CustomElementState::Uncustomized
    ) {
        return Ok(());
    }
    // 2-3.
    registry(agent).element_defs.insert(el, def);
    with_doc_mut(agent, el.doc, |d| {
        d.set_custom_element_state(el.node, CustomElementState::Failed)
    });
    // 4: un attributeChangedCallback por atributo.
    let attrs: Vec<(DomString, Option<Namespace>, DomString)> = with_doc(agent, el.doc, |d| {
        d.element(el.node)
            .map(|e| {
                e.attrs
                    .iter()
                    .map(|a| (a.name.clone(), a.ns, a.value.clone()))
                    .collect()
            })
            .unwrap_or_default()
    });
    for (name, namespace, value) in attrs {
        enqueue_callback(
            agent,
            el,
            Reaction::AttributeChanged {
                name,
                old: None,
                new: Some(value),
                namespace,
            },
        );
    }
    // 5.
    if with_doc(agent, el.doc, |d| d.is_connected(el.node)) {
        enqueue_callback(agent, el, Reaction::Connected);
    }
    // 6.
    registry(agent).defs[def].construction_stack.push(Some(el));
    let ctor = {
        let h = registry(agent).defs[def].constructor;
        pinned(agent, h)
    };
    // 9.
    let result = (|| -> JsResult<()> {
        let disable_shadow = registry(agent).defs[def].disable_shadow;
        if disable_shadow && with_doc(agent, el.doc, |d| d.shadow_root(el.node).is_some()) {
            return crate::net::dom_error(
                agent,
                "NotSupportedError",
                "The element has a shadow root and its definition disables shadow DOM.",
            );
        }
        with_doc_mut(agent, el.doc, |d| {
            d.set_custom_element_state(el.node, CustomElementState::Precustomized)
        });
        let Value::Object(c) = ctor else {
            return agent.type_error("constructor");
        };
        let r = agent.construct(c, &[], None)?;
        let wrapper = wrap_node(agent, el);
        if !agent.same_value(&r, &wrapper) {
            return agent.type_error(
                "Failed to upgrade the custom element: the constructor did not return the \
                 element being upgraded.",
            );
        }
        Ok(())
    })();
    // 10.
    registry(agent).defs[def].construction_stack.pop();
    // 11.
    if let Err(e) = result {
        let reg = registry(agent);
        reg.element_defs.remove(&el);
        reg.reactions.remove(&el);
        reg.queue.retain(|x| *x != el);
        return Err(e);
    }
    // 13.
    with_doc_mut(agent, el.doc, |d| {
        d.set_custom_element_state(el.node, CustomElementState::Custom)
    });
    Ok(())
}

// --- Los constructores HTML (§3.2.3 "HTML element constructors") ---

/// El constructor de `HTMLElement` y de las interfaces HTML (para `super()` desde una clase
/// de custom element).
pub fn html_constructor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(new_target) = a.new_target else {
        return agent.type_error("Illegal constructor");
    };
    // 2.
    if new_target == a.callee {
        return agent.type_error("Illegal constructor");
    }
    // 3.
    let defs: Vec<HostHandle> = registry(agent).defs.iter().map(|d| d.constructor).collect();
    let nt_value = Value::Object(new_target);
    let Some(def) = defs
        .iter()
        .position(|h| agent.same_value(&pinned(agent, *h), &nt_value))
    else {
        return agent.type_error("Illegal constructor");
    };
    let (name, local_name) = {
        let d = &registry(agent).defs[def];
        (d.name.clone(), d.local_name.clone())
    };
    // 4-6: la interfaz debe ser la del elemento (HTMLElement para los autónomos).
    let realm = realm(agent);
    let active = Some(a.callee);
    let expected = if name == local_name {
        "HTMLElement"
    } else {
        super::table::interface_for(&local_name)
    };
    let expected_class = super::elements::class_named(agent, expected);
    let ok = expected_class
        .map(|c| Some(agent.host_class_objects(realm, c).interface) == active)
        .unwrap_or(false);
    if !ok {
        return agent.type_error("Illegal constructor");
    }
    let is_value = (name != local_name).then(|| name.clone());
    // 8.
    let proto = agent.get_str(new_target, "prototype")?;
    let proto = match proto {
        Value::Object(p) => p,
        _ => {
            let c = expected_class.expect("class");
            agent.host_class_objects(realm, c).prototype
        }
    };
    // 7.
    let top = registry(agent).defs[def].construction_stack.last().copied();
    match top {
        None => {
            let node = with_doc_mut(agent, 0, |d| {
                d.create(NodeData::Element(bf_dom::Element::new(
                    Namespace::Html,
                    local_name.as_str(),
                )))
            });
            let el = NodeRef::new(0, node);
            with_doc_mut(agent, 0, |d| {
                d.set_is_value(node, is_value.map(DomString::from));
                d.set_custom_element_state(node, CustomElementState::Custom);
            });
            registry(agent).element_defs.insert(el, def);
            let w = wrap_node(agent, el);
            if let Value::Object(o) = w {
                agent.heap[o].proto = Some(proto);
            }
            Ok(w)
        }
        // 10.
        Some(None) => agent
            .type_error("Failed to construct 'HTMLElement': this instance is already constructed"),
        // 9, 11-13.
        Some(Some(el)) => {
            let w = wrap_node(agent, el);
            if let Value::Object(o) = w {
                agent.heap[o].proto = Some(proto);
            }
            if let Some(last) = registry(agent).defs[def].construction_stack.last_mut() {
                *last = None;
            }
            Ok(w)
        }
    }
}

// --- Creación de elementos y otros enganches ---

/// "Create an element" (DOM §4.9) para `createElement(NS)` (con el *synchronous custom
/// elements flag*): `node` ya creado sin definición; devuelve el elemento final.
pub fn create_element(agent: &mut Agent, el: NodeRef, is: Option<String>) -> JsResult<Value> {
    if let Some(is) = &is {
        with_doc_mut(agent, el.doc, |d| {
            d.set_is_value(el.node, Some(DomString::from(is.as_str())))
        });
    }
    let Some(def) = lookup_element(agent, el) else {
        return Ok(wrap_node(agent, el));
    };
    let (name, local_name) = {
        let d = &registry(agent).defs[def];
        (d.name.clone(), d.local_name.clone())
    };
    if name == local_name {
        // Autónomo: se construye ahora.
        let ctor = {
            let h = registry(agent).defs[def].constructor;
            pinned(agent, h)
        };
        let result = (|| -> JsResult<Value> {
            let Value::Object(c) = ctor else {
                return agent.type_error("constructor");
            };
            let r = agent.construct(c, &[], None)?;
            let Some(n) = unwrap_node(agent, &r) else {
                return agent.type_error("The result must implement HTMLElement interface");
            };
            let bad = with_doc(agent, n.doc, |d| {
                let e = d.element(n.node);
                e.is_none_or(|e| !e.attrs.is_empty() || e.name != local_name.as_str())
                    || d.first_child(n.node).is_some()
                    || d.parent(n.node).is_some()
            });
            if bad || n.doc != el.doc {
                return crate::net::dom_error(
                    agent,
                    "NotSupportedError",
                    "The result must not have attributes, children or a parent, and must be in the same document",
                );
            }
            Ok(r)
        })();
        match result {
            Ok(r) => Ok(r),
            Err(Throw::Value(e)) => {
                crate::dom::events::report_exception(agent, &e);
                // Un HTMLUnknownElement "failed".
                with_doc_mut(agent, el.doc, |d| {
                    d.set_custom_element_state(el.node, CustomElementState::Failed)
                });
                let w = wrap_node(agent, el);
                if let (Value::Object(o), Some(c)) = (
                    &w,
                    super::elements::class_named(agent, "HTMLUnknownElement"),
                ) {
                    let r = realm(agent);
                    let proto = agent.host_class_objects(r, c).prototype;
                    agent.heap[*o].proto = Some(proto);
                }
                Ok(w)
            }
            Err(t) => Err(t),
        }
    } else {
        // Customized built-in: upgrade síncrono.
        if let Err(Throw::Value(e)) = upgrade(agent, el, def) {
            crate::dom::events::report_exception(agent, &e);
        }
        Ok(wrap_node(agent, el))
    }
}

/// Después de clonar o importar: los elementos copiados con definición se actualizan (en
/// la cola de reacciones).
pub fn cloned(agent: &mut Agent, root: NodeRef) {
    if registry_ref(agent).is_none_or(|r| r.defs.is_empty()) {
        return;
    }
    let nodes = with_doc(agent, root.doc, |d| d.inclusive_descendants(root.node));
    for n in nodes {
        try_to_upgrade(agent, NodeRef::new(root.doc, n), false);
    }
    process_reactions(agent);
}

/// Al adoptar `old` (que pasó a ser `new` en otro documento): la definición sigue al nodo y,
/// si es custom, se encola `adoptedCallback`.
pub fn adopted(agent: &mut Agent, old: NodeRef, new: NodeRef) {
    let Some(reg) = agent.realm_data_mut::<Registry>(realm(agent)) else {
        return;
    };
    let Some(def) = reg.element_defs.remove(&old) else {
        return;
    };
    reg.element_defs.insert(new, def);
    if let Some(h) = reg.internals.remove(&old) {
        reg.internals.insert(new, h);
    }
    if with_doc(agent, new.doc, |d| d.is_custom_element(new.node)) {
        enqueue_callback(agent, new, Reaction::Adopted(old.doc, new.doc));
    }
}

/// ¿La definición de `n` deshabilita el shadow DOM?
pub fn shadow_disabled(agent: &Agent, n: NodeRef) -> bool {
    let Some(def) = lookup_element(agent, n) else {
        return false;
    };
    registry_ref(agent).is_some_and(|r| r.defs[def].disable_shadow)
}

/// ¿El *custom element state* de `n` es "precustomized" o "custom"?
pub fn is_custom(agent: &Agent, n: NodeRef) -> bool {
    matches!(
        with_doc(agent, n.doc, |d| d.custom_element_state(n.node)),
        CustomElementState::Precustomized | CustomElementState::Custom
    )
}

/// ¿Es un *form-associated custom element*?
pub fn is_form_associated(agent: &Agent, n: NodeRef) -> bool {
    registry_ref(agent)
        .and_then(|r| r.element_defs.get(&n).map(|d| r.defs[*d].form_associated))
        .unwrap_or(false)
}

// --- ElementInternals (§4.13.7.1) ---

/// El payload de `ElementInternals`.
struct Internals {
    target: NodeRef,
    states: Vec<String>,
    /// El `CustomStateSet` (el mismo objeto en cada acceso).
    states_object: Option<HostHandle>,
    aria: HashMap<&'static str, String>,
    value: Option<String>,
}

/// `attachInternals()`.
pub fn attach_internals(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let not_supported = |agent: &mut Agent, msg: &str| -> JsResult<Value> {
        crate::net::dom_error(agent, "NotSupportedError", msg)
    };
    // 1.
    if with_doc(agent, n.doc, |d| d.is_value(n.node).is_some()) {
        return not_supported(
            agent,
            "Unable to attach ElementInternals to a customized built-in element.",
        );
    }
    // 2-3.
    let (ns, local) = with_doc(agent, n.doc, |d| {
        d.element(n.node)
            .map(|e| (e.ns, e.name.to_string()))
            .unwrap_or((Namespace::Null, String::new()))
    });
    let Some(def) = lookup(agent, n.doc, ns, &local, None) else {
        return not_supported(
            agent,
            "Unable to attach ElementInternals to non-custom elements.",
        );
    };
    // 4.
    if registry(agent).defs[def].disable_internals {
        return not_supported(agent, "ElementInternals is disabled by disabledFeatures.");
    }
    // 5.
    if registry(agent).internals.contains_key(&n) {
        return not_supported(
            agent,
            "ElementInternals for the specified element was already attached.",
        );
    }
    // 6.
    if !is_custom(agent, n) {
        return not_supported(
            agent,
            "The attachInternals() function cannot be called prior to the execution of the custom element constructor.",
        );
    }
    let class = classes(agent).internals;
    let o = Value::Object(agent.host_create(
        class,
        Box::new(Internals {
            target: n,
            states: Vec::new(),
            states_object: None,
            aria: HashMap::new(),
            value: None,
        }),
    ));
    let h = agent.host_pin(o.clone());
    registry(agent).internals.insert(n, h);
    Ok(o)
}

fn this_internals<'a>(agent: &'a mut Agent, a: &CallArgs<'_>) -> JsResult<&'a mut Internals> {
    if agent.host_unwrap::<Internals>(&a.this).is_none() {
        return agent.type_error("Illegal invocation");
    }
    Ok(agent
        .host_unwrap_mut::<Internals>(&a.this)
        .expect("internals"))
}

fn internals_shadow_root(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let target = this_internals(agent, a)?.target;
    let sr = with_doc(agent, target.doc, |d| {
        d.shadow_root(target.node).filter(|&r| {
            d.shadow_root_data(r)
                .is_some_and(|s| s.available_to_element_internals)
        })
    });
    Ok(sr.map_or(Value::Null, |r| {
        wrap_node(agent, NodeRef::new(target.doc, r))
    }))
}

fn internals_states(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let i = this_internals(agent, a)?;
    let (target, existing) = (i.target, i.states_object);
    if let Some(h) = existing {
        return Ok(pinned(agent, h));
    }
    let class = classes(agent).state_set;
    let set = Value::Object(agent.host_create(class, Box::new(StateSetOf(target))));
    let h = agent.host_pin(set.clone());
    this_internals(agent, a)?.states_object = Some(h);
    Ok(set)
}

fn internals_form(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let target = this_internals(agent, a)?.target;
    if !is_form_associated(agent, target) {
        return crate::net::dom_error(
            agent,
            "NotSupportedError",
            "The target element is not a form-associated custom element.",
        );
    }
    match super::forms::form_owner_of(agent, target) {
        Some(f) => Ok(wrap_node(agent, f)),
        None => Ok(Value::Null),
    }
}

fn internals_set_form_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let target = this_internals(agent, a)?.target;
    if !is_form_associated(agent, target) {
        return crate::net::dom_error(
            agent,
            "NotSupportedError",
            "The target element is not a form-associated custom element.",
        );
    }
    let v = a.arg(0);
    let value = if v.is_null() {
        None
    } else {
        Some(to_dom_string(agent, &v)?.to_string())
    };
    this_internals(agent, a)?.value = value;
    Ok(Value::Undefined)
}

fn internals_labels(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let target = this_internals(agent, a)?.target;
    if !is_form_associated(agent, target) {
        return crate::net::dom_error(
            agent,
            "NotSupportedError",
            "The target element is not a form-associated custom element.",
        );
    }
    Ok(Value::Object(agent.create_array_from_list(&[])?))
}

fn internals_noop(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_internals(agent, a)?;
    Ok(Value::Undefined)
}

fn internals_true(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_internals(agent, a)?;
    Ok(Value::Boolean(true))
}

fn internals_false(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_internals(agent, a)?;
    Ok(Value::Boolean(false))
}

fn internals_empty(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_internals(agent, a)?;
    Ok(Value::str(""))
}

/// Los atributos de `ARIAMixin` en `ElementInternals` (WAI-ARIA 1.2 §10.1).
const ARIA_ATTRIBUTES: &[&str] = &[
    "role",
    "ariaAtomic",
    "ariaAutoComplete",
    "ariaBrailleLabel",
    "ariaBrailleRoleDescription",
    "ariaBusy",
    "ariaChecked",
    "ariaColCount",
    "ariaColIndex",
    "ariaColIndexText",
    "ariaColSpan",
    "ariaCurrent",
    "ariaDescription",
    "ariaDisabled",
    "ariaExpanded",
    "ariaHasPopup",
    "ariaHidden",
    "ariaInvalid",
    "ariaKeyShortcuts",
    "ariaLabel",
    "ariaLevel",
    "ariaLive",
    "ariaModal",
    "ariaMultiLine",
    "ariaMultiSelectable",
    "ariaOrientation",
    "ariaPlaceholder",
    "ariaPosInSet",
    "ariaPressed",
    "ariaReadOnly",
    "ariaRelevant",
    "ariaRequired",
    "ariaRoleDescription",
    "ariaRowCount",
    "ariaRowIndex",
    "ariaRowIndexText",
    "ariaRowSpan",
    "ariaSelected",
    "ariaSetSize",
    "ariaSort",
    "ariaValueMax",
    "ariaValueMin",
    "ariaValueNow",
    "ariaValueText",
];

/// El nombre del atributo del accessor en curso (`get ariaLabel` → `ariaLabel`).
fn accessor_name(agent: &mut Agent, a: &CallArgs<'_>) -> Option<&'static str> {
    let name = agent
        .get_str(a.callee, "name")
        .ok()?
        .as_string()?
        .to_string_lossy();
    let name = name
        .strip_prefix("get ")
        .or_else(|| name.strip_prefix("set "))?
        .to_owned();
    ARIA_ATTRIBUTES.iter().copied().find(|n| *n == name)
}

fn aria_get(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(name) = accessor_name(agent, a) else {
        return Ok(Value::Null);
    };
    let v = this_internals(agent, a)?.aria.get(name).cloned();
    Ok(v.map_or(Value::Null, |s| Value::str(&s)))
}

fn aria_set(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(name) = accessor_name(agent, a) else {
        return Ok(Value::Undefined);
    };
    let v = a.arg(0);
    let value = if v.is_null() {
        None
    } else {
        Some(to_dom_string(agent, &v)?.to_string())
    };
    let i = this_internals(agent, a)?;
    match value {
        Some(s) => {
            i.aria.insert(name, s);
        }
        None => {
            i.aria.remove(name);
        }
    }
    Ok(Value::Undefined)
}

// --- CustomStateSet ---

struct StateSetOf(NodeRef);

fn with_states<R>(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    f: impl FnOnce(&mut Vec<String>) -> R,
) -> JsResult<R> {
    let Some(target) = agent.host_unwrap::<StateSetOf>(&a.this).map(|s| s.0) else {
        return agent.type_error("Illegal invocation");
    };
    let Some(h) = registry(agent).internals.get(&target).copied() else {
        return agent.type_error("Illegal invocation");
    };
    let iv = pinned(agent, h);
    let Some(i) = agent.host_unwrap_mut::<Internals>(&iv) else {
        return agent.type_error("Illegal invocation");
    };
    Ok(f(&mut i.states))
}

fn states_add(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = to_dom_string(agent, &a.arg(0))?.to_string();
    with_states(agent, a, |s| {
        if !s.contains(&v) {
            s.push(v);
        }
    })?;
    Ok(a.this.clone())
}

fn states_delete(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = to_dom_string(agent, &a.arg(0))?.to_string();
    let had = with_states(agent, a, |s| {
        let had = s.contains(&v);
        s.retain(|x| *x != v);
        had
    })?;
    Ok(Value::Boolean(had))
}

fn states_has(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = to_dom_string(agent, &a.arg(0))?.to_string();
    Ok(Value::Boolean(with_states(agent, a, |s| s.contains(&v))?))
}

fn states_clear(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    with_states(agent, a, Vec::clear)?;
    Ok(Value::Undefined)
}

fn states_size(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(with_states(agent, a, |s| s.len())? as f64))
}

fn states_for_each(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let cb = a.arg(0);
    if !agent.is_callable(&cb) {
        return agent.type_error("The callback provided as parameter 1 is not a function.");
    }
    let values = with_states(agent, a, |s| s.clone())?;
    for v in values {
        let s = Value::str(&v);
        agent.call(&cb, a.arg(1), &[s.clone(), s, a.this.clone()])?;
    }
    Ok(Value::Undefined)
}
