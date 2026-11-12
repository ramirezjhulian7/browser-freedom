//! `Document` (DOM §4.5, HTML §3.1) y los constructores de `Document`, `DocumentFragment`,
//! `Text` y `Comment`. Los miembros de la carga y del BOM (`cookie`, `referrer`,
//! `lastModified`, `readyState`, `currentScript`, `visibilityState`, `hidden`, `hasFocus` y
//! `write`/`writeln`/`open`/`close`) los agrega E4 en `bom::document`.

use bf_dom::{Document, NodeData};
use bf_js::host::{HostClass, HostClassId};
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use crate::dom::collections::static_node_list;
use crate::dom::element::{
    get_elements_by_class_name, get_elements_by_tag_name, get_elements_by_tag_name_ns,
    namespace_arg,
};
use crate::dom::events::{GLOBAL_EVENT_HANDLERS, create_event, with_event_handlers};
use crate::dom::{
    classes, from_dom_string, implementation, mixins, mutated, this_node, throw_dom, to_dom_string,
    traversal, wrap_node, wrap_opt,
};
use crate::{DocIndex, NodeRef, WebContextHandle, with_doc, with_doc_mut};

pub fn document_class(node: HostClassId) -> HostClass {
    let c = HostClass::new("Document")
        .parent(node)
        .constructor(document_ctor, 0)
        // E1: ParentNode `moveBefore` (DOM §4.2.6).
        .method("moveBefore", 2, crate::dom::node::move_before)
        .attribute("implementation", Some(get_implementation), None)
        .attribute("URL", Some(url), None)
        .attribute("documentURI", Some(url), None)
        .attribute("compatMode", Some(compat_mode), None)
        .attribute("characterSet", Some(character_set), None)
        .attribute("charset", Some(character_set), None)
        .attribute("inputEncoding", Some(character_set), None)
        .attribute("contentType", Some(content_type), None)
        .attribute("doctype", Some(doctype), None)
        .attribute("documentElement", Some(document_element), None)
        // E10: SVG 2 `partial interface Document { readonly attribute SVGSVGElement?
        // rootElement; }`.
        .attribute("rootElement", Some(root_element), None)
        .attribute("title", Some(get_title), Some(set_title))
        .attribute("head", Some(head), None)
        .attribute("body", Some(body), None)
        .attribute("defaultView", Some(default_view), None)
        .attribute("activeElement", Some(active_element), None)
        .attribute("dir", Some(get_dir), Some(set_dir))
        .method("getElementsByTagName", 1, get_elements_by_tag_name)
        .method("getElementsByTagNameNS", 2, get_elements_by_tag_name_ns)
        .method("getElementsByClassName", 1, get_elements_by_class_name)
        .method("getElementsByName", 1, get_elements_by_name)
        .method("getElementById", 1, get_element_by_id)
        .method("createElement", 1, create_element)
        .method("createElementNS", 2, create_element_ns)
        .method("createDocumentFragment", 0, create_document_fragment)
        .method("createTextNode", 1, create_text_node)
        .method("createCDATASection", 1, create_cdata_section)
        .method("createComment", 1, create_comment)
        .method(
            "createProcessingInstruction",
            2,
            create_processing_instruction,
        )
        .method("createAttribute", 1, create_attribute)
        .method("createAttributeNS", 2, create_attribute_ns)
        .method("createEvent", 1, create_event_method)
        .method("createNodeIterator", 1, create_node_iterator)
        .method("createTreeWalker", 1, create_tree_walker)
        .method("importNode", 1, import_node)
        .method("adoptNode", 1, adopt_node)
        .method("createRange", 0, crate::dom::range::document_create_range)
        .method("getSelection", 0, crate::dom::range::document_get_selection);

    let c = mixins::parent_node(c);
    let mut names: Vec<&'static str> = GLOBAL_EVENT_HANDLERS.to_vec();
    names.extend(["readystatechange", "visibilitychange"]);
    with_event_handlers(c, &names)
}

/// `DocumentFragment` con ParentNode y NonElementParentNode.
pub fn document_fragment_class(node: HostClassId) -> HostClass {
    let c = HostClass::new("DocumentFragment")
        .parent(node)
        .constructor(document_fragment_ctor, 0)
        // E1: ParentNode `moveBefore` (DOM §4.2.6).
        .method("moveBefore", 2, crate::dom::node::move_before);
    mixins::non_element_parent_node(mixins::parent_node(c))
}

fn this_doc(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<DocIndex> {
    Ok(this_node(agent, a)?.doc)
}

fn root_of(agent: &Agent, doc: DocIndex) -> NodeRef {
    NodeRef::new(doc, with_doc(agent, doc, |d| d.root()))
}

/// `new Document()` (DOM §4.5): un documento XML vacío.
fn document_ctor(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let mut d = Document::new_xml();
    d.url = bf_infra::Url::parse("about:blank", None).expect("about:blank");
    let ctx = WebContextHandle::current(agent);
    let i = ctx.get_mut(agent).add_document(d);
    crate::dom::state::state(agent).plain_documents.insert(i);
    let r = root_of(agent, i);
    Ok(wrap_node(agent, r))
}

/// `cloneNode()` de un documento: un documento nuevo en el contexto (con o sin hijos).
pub fn clone_document(agent: &mut Agent, doc: DocIndex, deep: bool) -> Value {
    let mut d = with_doc(agent, doc, |d| d.clone());
    // Clonar no copia los listeners (DOM §4.4 "clone a node").
    d.erase_all_event_listeners();
    d.clear_live_ranges();
    if !deep {
        let root = d.root();
        d.replace_all(None, root);
    }
    let ctx = WebContextHandle::current(agent);
    let i = ctx.get_mut(agent).add_document(d);
    let r = root_of(agent, i);
    wrap_node(agent, r)
}

fn get_implementation(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    Ok(implementation::implementation_of(agent, doc))
}

fn url(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let s = with_doc(agent, doc, |d| d.document_url());
    Ok(from_dom_string(&s))
}

fn compat_mode(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    Ok(Value::str(with_doc(agent, doc, |d| d.compat_mode())))
}

fn character_set(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let s = with_doc(agent, doc, |d| d.character_set.clone());
    Ok(from_dom_string(&s))
}

fn content_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let s = with_doc(agent, doc, |d| d.content_type.clone());
    Ok(from_dom_string(&s))
}

fn doctype(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let e = with_doc(agent, doc, |d| d.doctype());
    Ok(wrap_opt(agent, doc, e))
}

fn document_element(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let e = with_doc(agent, doc, |d| d.document_element());
    Ok(wrap_opt(agent, doc, e))
}

/// E10: `Document.rootElement` (SVG 2): el document element si es un `svg` SVG, o `null`.
fn root_element(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let e = with_doc(agent, doc, |d| d.svg_root_element());
    Ok(wrap_opt(agent, doc, e))
}

fn get_title(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let s = with_doc(agent, doc, |d| d.title());
    Ok(from_dom_string(&s))
}

fn set_title(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let v = to_dom_string(agent, &a.arg(0))?;
    with_doc_mut(agent, doc, |d| d.set_title(v));
    mutated(agent);
    Ok(Value::Undefined)
}

fn head(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let e = with_doc(agent, doc, |d| d.head());
    Ok(wrap_opt(agent, doc, e))
}

fn body(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let e = with_doc(agent, doc, |d| d.body());
    Ok(wrap_opt(agent, doc, e))
}

/// `defaultView`: la ventana para el documento de la ventana; `null` para los demás.
fn default_view(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    if doc != 0 {
        return Ok(Value::Null);
    }
    let realm = WebContextHandle::current(agent).realm;
    Ok(Value::Object(agent.realms[realm].global_object))
}

/// `activeElement` (HTML §6.6.2): sin foco en otro elemento, el `body` (o el elemento
/// raíz).
fn active_element(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    // E1: el elemento enfocado, retargeteado (DocumentOrShadowRoot).
    let root = with_doc(agent, doc, |d| d.root());
    let e = crate::dom::shadow::active_element_for(agent, crate::NodeRef::new(doc, root));
    Ok(wrap_opt(agent, doc, e.map(|e| e.node)))
}

/// `dir` refleja el atributo `dir` del elemento raíz (`ltr`/`rtl`/`auto` o vacío).
fn get_dir(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let v = with_doc(agent, doc, |d| {
        d.document_element()
            .and_then(|e| d.get_attribute(e, &"dir".into()))
            .map(|v| v.to_string().to_ascii_lowercase())
    });
    let v = v
        .as_deref()
        .filter(|v| matches!(*v, "ltr" | "rtl" | "auto"))
        .unwrap_or("");
    Ok(Value::str(v))
}

fn set_dir(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let v = to_dom_string(agent, &a.arg(0))?;
    with_doc_mut(agent, doc, |d| {
        if let Some(e) = d.document_element() {
            let _ = d.set_attribute(e, &"dir".into(), v);
        }
    });
    mutated(agent);
    Ok(Value::Undefined)
}

/// `getElementsByName` (HTML §3.1.5): los elementos HTML con ese `name` (como lista
/// estática).
fn get_elements_by_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let name = to_dom_string(agent, &a.arg(0))?;
    let list = with_doc(agent, doc, |d| {
        d.descendants(d.root())
            .into_iter()
            .filter(|&n| {
                d.element(n).is_some_and(|e| {
                    e.ns == bf_infra::Namespace::Html
                        && e.attrs
                            .iter()
                            .any(|at| at.ns.is_none() && at.name == "name" && at.value == name)
                })
            })
            .collect()
    });
    Ok(static_node_list(agent, doc, list))
}

fn get_element_by_id(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let id = to_dom_string(agent, &a.arg(0))?;
    let e = with_doc(agent, doc, |d| d.get_element_by_id(&id));
    Ok(wrap_opt(agent, doc, e))
}

/// El resultado de una creación de `bf-dom`.
fn created(
    agent: &mut Agent,
    doc: DocIndex,
    r: Result<bf_dom::NodeId, bf_dom::DomException>,
) -> JsResult<Value> {
    match r {
        Ok(n) => Ok(wrap_node(agent, NodeRef::new(doc, n))),
        Err(e) => throw_dom(agent, e),
    }
}

fn create_element(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
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
}

fn create_text_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let data = to_dom_string(agent, &a.arg(0))?;
    let t = with_doc_mut(agent, doc, |d| d.create_text_node(data));
    Ok(wrap_node(agent, NodeRef::new(doc, t)))
}

fn create_cdata_section(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let data = to_dom_string(agent, &a.arg(0))?;
    let r = with_doc_mut(agent, doc, |d| d.create_cdata_section(data));
    created(agent, doc, r)
}

fn create_comment(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let data = to_dom_string(agent, &a.arg(0))?;
    let t = with_doc_mut(agent, doc, |d| d.create_comment(data));
    Ok(wrap_node(agent, NodeRef::new(doc, t)))
}

fn create_processing_instruction(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let target = to_dom_string(agent, &a.arg(0))?;
    let data = to_dom_string(agent, &a.arg(1))?;
    let r = with_doc_mut(agent, doc, |d| {
        d.create_processing_instruction(target, data)
    });
    created(agent, doc, r)
}

fn create_attribute(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let name = to_dom_string(agent, &a.arg(0))?;
    let r = with_doc_mut(agent, doc, |d| d.create_attribute(&name));
    created(agent, doc, r)
}

fn create_attribute_ns(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let ns = namespace_arg(agent, &a.arg(0))?;
    let qn = to_dom_string(agent, &a.arg(1))?;
    let r = with_doc_mut(agent, doc, |d| d.create_attribute_ns(ns.as_deref(), &qn));
    created(agent, doc, r)
}

fn create_document_fragment(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let f = with_doc_mut(agent, doc, |d| d.create_document_fragment());
    Ok(wrap_node(agent, NodeRef::new(doc, f)))
}

fn create_event_method(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    let iface = to_dom_string(agent, &a.arg(0))?.to_string();
    create_event(agent, doc, &iface)
}

fn create_node_iterator(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_doc(agent, a)?;
    traversal::create(agent, false, &a.arg(0), &a.arg(1), &a.arg(2))
}

fn create_tree_walker(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_doc(agent, a)?;
    traversal::create(agent, true, &a.arg(0), &a.arg(1), &a.arg(2))
}

/// `importNode(node, subtree = false)` (§4.5): una copia en este documento.
fn import_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    // E1: un nodo del documento de otra ventana también se clona (DOM §4.5 importNode: "clone
    // a node" con este documento; el original no cambia).
    let n = match crate::dom::adopt::arg_node_any(
        agent,
        &a.arg(0),
        "Failed to execute 'importNode' on 'Document': parameter 1",
    )? {
        crate::dom::adopt::NodeArg::Local(n) => n,
        crate::dom::adopt::NodeArg::Foreign(realm, n) => {
            let deep = bf_js::host::webidl::to_boolean(agent, &a.arg(1));
            return import_foreign(agent, doc, realm, n, deep);
        }
    };
    let deep = bf_js::host::webidl::to_boolean(agent, &a.arg(1));
    let kind = with_doc(agent, n.doc, |d| d.get(n.node).data.clone());
    if matches!(kind, NodeData::Document) {
        return throw_dom(agent, bf_dom::DomException::NotSupportedError);
    }
    if n.doc == doc {
        let r = with_doc_mut(agent, doc, |d| d.import_node(n.node, deep));
        // E1: los custom elements de la copia.
        if let Ok(c) = r {
            crate::html::custom_elements::cloned(agent, NodeRef::new(doc, c));
        }
        return created(agent, doc, r);
    }
    let ctx = WebContextHandle::current(agent);
    let copy = {
        let wc = ctx.get_mut(agent);
        let mut target = std::mem::take(wc.doc_mut(doc));
        let copy = target.import_from(wc.doc(n.doc), n.node);
        if !deep {
            target.replace_all(None, copy);
        }
        *wc.doc_mut(doc) = target;
        copy
    };
    let old = with_doc(agent, n.doc, |d| d.inclusive_descendants(n.node));
    let new = with_doc(agent, doc, |d| d.inclusive_descendants(copy));
    crate::dom::insertion::copy_script_state(agent, (n.doc, &old), (doc, &new));
    // E1: los custom elements de la copia.
    crate::html::custom_elements::cloned(agent, NodeRef::new(doc, copy));
    Ok(wrap_node(agent, NodeRef::new(doc, copy)))
}

/// E1: `importNode` de un nodo `n` del documento de otra ventana (`realm`): la copia se hace
/// en la arena de `doc` leyendo la de la otra ventana.
fn import_foreign(
    agent: &mut Agent,
    doc: crate::DocIndex,
    realm: bf_js::vm::RealmId,
    n: NodeRef,
    deep: bool,
) -> JsResult<Value> {
    let there = WebContextHandle { realm };
    let (is_document, started) = {
        let d = there.get(agent).doc(n.doc);
        let started: Vec<bool> = d
            .inclusive_descendants(n.node)
            .iter()
            .map(|&o| d.script_state(o).already_started)
            .collect();
        (matches!(d.get(n.node).data, NodeData::Document), started)
    };
    if is_document {
        return throw_dom(agent, bf_dom::DomException::NotSupportedError);
    }
    let here = WebContextHandle::current(agent);
    let mut target = std::mem::take(here.get_mut(agent).doc_mut(doc));
    let copy = target.import_from(there.get(agent).doc(n.doc), n.node);
    if !deep {
        target.replace_all(None, copy);
    }
    *here.get_mut(agent).doc_mut(doc) = target;
    with_doc_mut(agent, doc, |d| {
        let new = d.inclusive_descendants(copy);
        for (nw, s) in new.iter().zip(&started) {
            if *s {
                d.script_state_mut(*nw).already_started = true;
            }
        }
    });
    crate::html::custom_elements::cloned(agent, NodeRef::new(doc, copy));
    Ok(wrap_node(agent, NodeRef::new(doc, copy)))
}

fn adopt_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let doc = this_doc(agent, a)?;
    // E1: también un nodo del documento de otra ventana.
    let n = crate::dom::adopt::arg_node_any(
        agent,
        &a.arg(0),
        "Failed to execute 'adoptNode' on 'Document': parameter 1",
    )?;
    let r = crate::dom::adopt::adopt_arg(agent, n, doc)?;
    // Paso 2 de "adopt" (§4.5) para un nodo que ya era de este documento: quitarlo de su padre
    // (`adopt_into` solo mueve de arena).
    if let crate::dom::adopt::NodeArg::Local(l) = n
        && l.doc == doc
    {
        // (Un `Attr` no tiene padre: sigue en su elemento.)
        with_doc_mut(agent, doc, |d| d.adopt(r.node));
        crate::dom::mutated(agent);
    }
    Ok(wrap_node(agent, r))
}

/// `new DocumentFragment()`: en el documento de la ventana del constructor.
pub fn document_fragment_ctor(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let f = with_doc_mut(agent, 0, |d| d.create_document_fragment());
    Ok(wrap_node(agent, NodeRef::new(0, f)))
}

/// `new Text(data = "")`.
pub fn text_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let data = if a.arg(0).is_undefined() {
        Default::default()
    } else {
        to_dom_string(agent, &a.arg(0))?
    };
    let t = with_doc_mut(agent, 0, |d| d.create_text_node(data));
    Ok(wrap_node(agent, NodeRef::new(0, t)))
}

/// `new Comment(data = "")`.
pub fn comment_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let data = if a.arg(0).is_undefined() {
        Default::default()
    } else {
        to_dom_string(agent, &a.arg(0))?
    };
    let t = with_doc_mut(agent, 0, |d| d.create_comment(data));
    Ok(wrap_node(agent, NodeRef::new(0, t)))
}

/// ¿Es `v` un `Document`? (Para los chequeos de tipo de otras áreas.)
pub fn is_document(agent: &Agent, v: &Value) -> bool {
    agent.host_is_instance(v, classes(agent).document)
}
