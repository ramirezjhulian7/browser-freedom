import sys

sys.path.insert(0, "D:/Personales/Repos/browser-freedom-duo-e1/target/scratch")
from patchlib import patch

W = "crates/bf-web/src/"

patch(W + "html/mod.rs", [
    ("""    canvas::register(agent);
    crate::dom::shadow::register(agent);""", """    canvas::register(agent);
    crate::dom::shadow::register(agent);
    custom_elements::register(agent);"""),
])

patch(W + "html/elements.rs", [
    ("""        let html_element = agent.register_host_class(html_element_class(element));""",
     """        let html_element = agent.register_host_class(html_element_class(element));"""),
    ("""        for iface in INTERFACES {
            let parent = by_name[iface.parent];
            let class = agent.register_host_class(extend(
                iface.name,
                HostClass::new(iface.name).parent(parent),
            ));""", """        for iface in INTERFACES {
            let parent = by_name[iface.parent];
            // E1: [HTMLConstructor] (todas menos HTMLUnknownElement), para los customized
            // built-ins.
            let mut base = HostClass::new(iface.name).parent(parent);
            if iface.name != "HTMLUnknownElement" {
                base = base.constructor(super::custom_elements::html_constructor, 0);
            }
            let class = agent.register_host_class(extend(iface.name, base));"""),
    ("""    class
        .parent(element)
        .attribute("tabIndex", Some(get_tab_index), Some(set_tab_index))""", """    class
        .parent(element)
        // E1: [HTMLConstructor] y `attachInternals()` (HTML §3.2.3, §4.13.7).
        .constructor(super::custom_elements::html_constructor, 0)
        .method("attachInternals", 0, super::custom_elements::attach_internals)
        .attribute("tabIndex", Some(get_tab_index), Some(set_tab_index))"""),
])

patch(W + "dom/document.rs", [
    ("""fn create_element(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let name = to_dom_string(agent, &a.arg(0))?;
    let r = with_doc_mut(agent, doc, |d| d.create_element(&name));
    created(agent, doc, r)
}

fn create_element_ns(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let ns = namespace_arg(agent, &a.arg(0))?;
    let qn = to_dom_string(agent, &a.arg(1))?;
    let r = with_doc_mut(agent, doc, |d| d.create_element_ns(ns.as_deref(), &qn));
    created(agent, doc, r)
}""", """fn create_element(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let name = to_dom_string(agent, &a.arg(0))?;
    let is = element_creation_is(agent, &a.arg(1))?;
    let r = with_doc_mut(agent, doc, |d| d.create_element(&name));
    custom_created(agent, doc, r, is)
}

fn create_element_ns(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let ns = namespace_arg(agent, &a.arg(0))?;
    let qn = to_dom_string(agent, &a.arg(1))?;
    let is = element_creation_is(agent, &a.arg(2))?;
    let r = with_doc_mut(agent, doc, |d| d.create_element_ns(ns.as_deref(), &qn));
    custom_created(agent, doc, r, is)
}

/// E1: el `is` de `ElementCreationOptions` (un string como opciones no cuenta).
fn element_creation_is(agent: &mut Agent, options: &Value) -> JsResult<Option<String>> {
    let Value::Object(o) = options else {
        return Ok(None);
    };
    let v = agent.get_str(*o, "is")?;
    if v.is_undefined() {
        return Ok(None);
    }
    Ok(Some(to_dom_string(agent, &v)?.to_string()))
}

/// E1: "create an element" con los custom elements (definición, *is value*, constructor).
fn custom_created(
    agent: &mut Agent,
    doc: DocIndex,
    r: Result<bf_dom::NodeId, bf_dom::DomException>,
    is: Option<String>,
) -> JsResult<Value> {
    match r {
        Ok(n) => crate::html::custom_elements::create_element(agent, NodeRef::new(doc, n), is),
        Err(e) => throw_dom(agent, e),
    }
}"""),
    ("""    let old = with_doc(agent, n.doc, |d| d.inclusive_descendants(n.node));
    let new = with_doc(agent, doc, |d| d.inclusive_descendants(copy));
    crate::dom::insertion::copy_script_state(agent, (n.doc, &old), (doc, &new));
    Ok(wrap_node(agent, NodeRef::new(doc, copy)))""", """    let old = with_doc(agent, n.doc, |d| d.inclusive_descendants(n.node));
    let new = with_doc(agent, doc, |d| d.inclusive_descendants(copy));
    crate::dom::insertion::copy_script_state(agent, (n.doc, &old), (doc, &new));
    // E1: los custom elements de la copia.
    crate::html::custom_elements::cloned(agent, NodeRef::new(doc, copy));
    Ok(wrap_node(agent, NodeRef::new(doc, copy)))"""),
    ("""    if n.doc == doc {
        let r = with_doc_mut(agent, doc, |d| d.import_node(n.node, deep));
        return created(agent, doc, r);
    }""", """    if n.doc == doc {
        let r = with_doc_mut(agent, doc, |d| d.import_node(n.node, deep));
        // E1: los custom elements de la copia.
        if let Ok(c) = r {
            crate::html::custom_elements::cloned(agent, NodeRef::new(doc, c));
        }
        return created(agent, doc, r);
    }"""),
])

patch(W + "dom/node.rs", [
    ("""    let r = with_doc_mut(agent, n.doc, |d| d.clone_node_checked(n.node, deep));
    match r {
        Ok(c) => Ok(wrap_node(agent, NodeRef::new(n.doc, c))),""", """    let r = with_doc_mut(agent, n.doc, |d| d.clone_node_checked(n.node, deep));
    match r {
        Ok(c) => {
            // E1: los custom elements de la copia.
            crate::html::custom_elements::cloned(agent, NodeRef::new(n.doc, c));
            Ok(wrap_node(agent, NodeRef::new(n.doc, c)))
        }"""),
])

patch(W + "dom/mod.rs", [
    ("""    crate::dom::shadow::sync_adopted(agent); // E1
}""", """    crate::dom::shadow::sync_adopted(agent); // E1
    crate::html::custom_elements::process_reactions(agent); // E1
}"""),
    ("""    crate::html::activation::queue_details_toggles(agent, doc); // E1""",
     """    crate::html::activation::queue_details_toggles(agent, doc); // E1
    crate::html::custom_elements::process_reactions(agent); // E1"""),
])

patch(W + "dom/adopt.rs", [
    ("""        move_handlers(agent, (from, o), (to, nw));
    }
    mutated(agent);""", """        move_handlers(agent, (from, o), (to, nw));
        // E1: la definición y `adoptedCallback` de los custom elements.
        crate::html::custom_elements::adopted(agent, old_ref, new_ref);
    }
    mutated(agent);"""),
])

patch(W + "bom/script.rs", [
    ("""        let parser = std::mem::replace(doc, placeholder);
        put_parser(self.agent, self.realm, parser);
        match info.type_ {""", """        let parser = std::mem::replace(doc, placeholder);
        put_parser(self.agent, self.realm, parser);
        // E1: los custom elements que el parser insertó se actualizan antes del script.
        crate::html::custom_elements::process_reactions(self.agent);
        match info.type_ {"""),
])

patch(W + "html/scripting.rs", [
    ("""        let saved = std::mem::replace(
            &mut ctx(self.agent, self.realm).documents[0],
            DocumentOwner::Parser(Box::new(real)),
        );
        self.run_classic_script(&text, &url);""", """        let saved = std::mem::replace(
            &mut ctx(self.agent, self.realm).documents[0],
            DocumentOwner::Parser(Box::new(real)),
        );
        // E1: los custom elements que el parser insertó se actualizan antes del script.
        crate::html::custom_elements::process_reactions(self.agent);
        self.run_classic_script(&text, &url);"""),
])

patch(W + "dom/events.rs", [
    ("""pub fn invoke_listener(agent: &mut Agent, handle: u64, event: &mut Event) -> bool {
    invoke_listener_in(agent, 0, handle, event)
}""", """pub fn invoke_listener(agent: &mut Agent, handle: u64, event: &mut Event) -> bool {
    // E1: los custom elements del parser se actualizan antes (`DOMContentLoaded`, `load`).
    crate::html::custom_elements::process_reactions(agent);
    invoke_listener_in(agent, 0, handle, event)
}"""),
])

# bf-html: el atributo `is` del token es el *is value*.
patch("crates/bf-html/src/tree_builder/mod.rs", [
    ("""    fn create_element_for_token(&mut self, tag: &TagToken, ns: Namespace) -> NodeId {
        self.create_element(Element {
            ns,
            prefix: None,
            name: tag.name.clone(),
            attrs: tag.attrs.clone(),
            template_contents: None,
        })
    }""", """    fn create_element_for_token(&mut self, tag: &TagToken, ns: Namespace) -> NodeId {
        let id = self.create_element(Element {
            ns,
            prefix: None,
            name: tag.name.clone(),
            attrs: tag.attrs.clone(),
            template_contents: None,
        });
        // E1: el *is value* (paso 4 de "create an element for a token").
        if ns == Namespace::Html
            && let Some(is) = tag.attr("is")
        {
            self.doc.set_is_value(id, Some(is.into()));
        }
        id
    }"""),
])

# bf-dom: la copia entre documentos conserva el estado y el *is value*.
patch("crates/bf-dom/src/node.rs", [
    ("""    fn import_from_inner(&mut self, other: &Document, node: NodeId) -> NodeId {
        let mut data = other.get(node).data.clone();""", """    fn import_from_inner(&mut self, other: &Document, node: NodeId) -> NodeId {
        let copy = self.import_from_node(other, node);
        // E1: el estado de custom element y el *is value*.
        self.import_custom_state(other, node, copy);
        copy
    }

    fn import_from_node(&mut self, other: &Document, node: NodeId) -> NodeId {
        let mut data = other.get(node).data.clone();"""),
])
patch("crates/bf-dom/src/custom.rs", [
    ("""    /// Copia el *is value* al clonar (paso 2 de "clone a single node").""",
     """    /// Copia el estado y el *is value* de `node` de `other` a `copy` (adopción entre arenas).
    pub(crate) fn import_custom_state(&mut self, other: &Document, node: NodeId, copy: NodeId) {
        if let Some(s) = other.custom.states.get(&node) {
            self.custom.states.insert(copy, *s);
        }
        if let Some(v) = other.custom.is_values.get(&node) {
            self.custom.is_values.insert(copy, v.clone());
        }
        if other.custom.enabled {
            self.custom.enabled = true;
        }
    }

    /// Copia el *is value* al clonar (paso 2 de "clone a single node")."""),
])
print("ok")
