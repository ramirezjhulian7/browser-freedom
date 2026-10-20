//! Transcripción a Rust de tests de WPT de `dom/nodes/` sobre la interfaz `Node`, la creación
//! de elementos y `getElementById`.
//!
//! Los árboles se construyen a mano con la API de `bf-dom` (no hay parser en este crate).
//!
//! Adaptaciones:
//! - `document.implementation.createHTMLDocument(title)` se emula con un `Document::new()`
//!   nuevo al que se le agregan doctype, `html`, `head` (con `title` si hay título) y `body`
//!   (ver [`html_document`]). Es otro documento de verdad (otra arena), así que los subtests
//!   que solo operan dentro de ese documento se conservan.
//! - Cuando un subtest necesita *un nodo Document distinto* dentro del mismo árbol de pruebas
//!   (insertar un documento en otro, los `foreignDoc`/`xmlDoc` de `common.js`, comparar
//!   documentos con `isEqualNode`), se crea un nodo `Document` suelto en la misma arena con
//!   `doc.create(NodeData::Document)`. Para estos no se comprueba `ownerDocument`.
//! - Los `CDATASection` en un documento HTML se crean con `doc.create(NodeData::CdataSection)`
//!   (en `common.js` vienen de otro documento XML).
//! - Los documentos XML se crean con `Document::new_xml()` y los XHTML con `Document::new_xml()`
//!   más `content_type = "application/xhtml+xml"` (en WPT son iframes con `dummy.xml` y
//!   `dummy.xhtml`).
//! - En JS `null`/`undefined` como `DOMString` se convierten en `"null"`/`"undefined"`; como
//!   `DOMString?` (namespace, `textContent`) son `None`.
//!
//! Subtests omitidos y por qué:
//! - Todos los de WebIDL que esperan `TypeError` por pasar `null` o un objeto que no es `Node`
//!   (`appendChild(null)`, `insertBefore(null, ...)`, `replaceChild(null, ...)`,
//!   `removeChild(null)`, `insertBefore(node)` sin segundo argumento): el sistema de tipos de
//!   Rust los hace imposibles.
//! - `Node-appendChild.html`: "Adopting an orphan" y "Adopting a non-orphan" (usan el documento
//!   de un iframe y comprueban el cambio de `ownerDocument`; hay un solo documento por arena).
//! - `Node-removeChild.html`: las variantes "from a frame document" y "from a synthetic
//!   document" (nodos de otro documento pasados al `body` del documento principal).
//! - `Node-cloneNode.html`: "implementation.createDocument" e "implementation.createHTMLDocument"
//!   (clonar un nodo `Document` crea otro documento) y "Node with custom prototype" (prototipos
//!   de JS). Los chequeos `instanceof` se aproximan comparando `html_kind`.
//! - `Node-replaceChild.html`: el listener de `DOMNodeRemoved` de "replaceChild should work in
//!   the presence of mutation events" (no hay mutation events); el resto del subtest se conserva.
//! - `Document-getElementById.html`: "add/remove id attribute via innerHTML/outerHTML"
//!   (necesitan el parser de `bf-html`).
//! - `Node-properties.html`: las entradas de `foreignDoc`, `xmlDoc` y sus nodos (dependen de
//!   otros documentos: `ownerDocument`, `URL`, `contentType`...), y las propiedades de
//!   `document` que no son de árbol (`URL`, `compatMode`...).
//! - `Node-lookupNamespaceURI.html`, `Node-compareDocumentPosition.html`, `Node-contains.html`:
//!   nada omitido salvo que `foreignDoc`/`xmlDoc` son nodos `Document` sueltos en la arena.

use bf_dom::document_position::{
    CONTAINED_BY, CONTAINS, DISCONNECTED, FOLLOWING, IMPLEMENTATION_SPECIFIC, PRECEDING,
};
use bf_dom::{Document, DomException, DomString, Namespace, NodeData, NodeId, NodeType};

const HTML_NS: &str = "http://www.w3.org/1999/xhtml";
const SVG_NS: &str = "http://www.w3.org/2000/svg";
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

const HRE: DomException = DomException::HierarchyRequestError;
const NFE: DomException = DomException::NotFoundError;

// --- Helpers ---

fn ds(s: &str) -> DomString {
    DomString::from(s)
}

fn st(o: Option<DomString>) -> Option<String> {
    o.map(|d| d.to_string())
}

fn elem(doc: &mut Document, name: &str) -> NodeId {
    doc.create_element(&ds(name)).unwrap()
}

fn elem_ns(doc: &mut Document, ns: Option<&str>, qname: &str) -> NodeId {
    doc.create_element_ns(ns, &ds(qname)).unwrap()
}

fn text(doc: &mut Document, data: &str) -> NodeId {
    doc.create_text_node(ds(data))
}

fn comment(doc: &mut Document, data: &str) -> NodeId {
    doc.create_comment(ds(data))
}

fn pi(doc: &mut Document, target: &str, data: &str) -> NodeId {
    doc.create_processing_instruction(ds(target), ds(data))
        .unwrap()
}

fn doctype(doc: &mut Document, name: &str, public_id: &str, system_id: &str) -> NodeId {
    doc.create_document_type(ds(name), ds(public_id), ds(system_id))
        .unwrap()
}

/// Un `CDATASection` creado sin validar (en un documento HTML `createCDATASection` lanza).
fn cdata_raw(doc: &mut Document, data: &str) -> NodeId {
    doc.create(NodeData::CdataSection(ds(data)))
}

/// Un nodo `Document` suelto en la arena: hace de "otro documento".
fn raw_document(doc: &mut Document) -> NodeId {
    doc.create(NodeData::Document)
}

fn append(doc: &mut Document, parent: NodeId, child: NodeId) -> NodeId {
    doc.append_child(parent, child).unwrap()
}

fn set_attr(doc: &mut Document, el: NodeId, name: &str, value: &str) {
    doc.set_attribute(el, &ds(name), ds(value)).unwrap();
}

fn get_attr(doc: &Document, el: NodeId, name: &str) -> Option<String> {
    st(doc.get_attribute(el, &ds(name)))
}

fn by_id(doc: &Document, id: &str) -> Option<NodeId> {
    doc.get_element_by_id(&ds(id))
}

fn text_content(doc: &Document, n: NodeId) -> Option<String> {
    st(doc.get_text_content(n))
}

fn data(doc: &Document, n: NodeId) -> String {
    doc.data(n).to_string()
}

/// Emula `document.implementation.createHTMLDocument(title)`: doctype `html`, `html`, `head`
/// (con `title` y su texto si hay título) y `body`.
fn html_document(title: Option<&str>) -> Document {
    let mut doc = Document::new();
    let root = doc.root();
    let dt = doctype(&mut doc, "html", "", "");
    append(&mut doc, root, dt);
    let html = elem(&mut doc, "html");
    append(&mut doc, root, html);
    let head = elem(&mut doc, "head");
    append(&mut doc, html, head);
    if let Some(t) = title {
        let title_el = elem(&mut doc, "title");
        append(&mut doc, head, title_el);
        let tn = text(&mut doc, t);
        append(&mut doc, title_el, tn);
    }
    let body = elem(&mut doc, "body");
    append(&mut doc, html, body);
    doc
}

fn doctype_fields(doc: &Document, n: NodeId) -> (String, String, String) {
    match &doc.get(n).data {
        NodeData::Doctype {
            name,
            public_id,
            system_id,
        } => (
            name.to_string(),
            public_id.to_string(),
            system_id.to_string(),
        ),
        other => panic!("no es un doctype: {other:?}"),
    }
}

/// (namespaceURI, prefix, localName) de un nodo `Attr`.
fn attr_names(doc: &Document, n: NodeId) -> (Option<Namespace>, Option<String>, String) {
    match &doc.get(n).data {
        NodeData::Attr(a) => (
            a.ns,
            a.prefix.as_ref().map(|p| p.to_string()),
            a.local_name.to_string(),
        ),
        other => panic!("no es un Attr: {other:?}"),
    }
}

fn pi_target(doc: &Document, n: NodeId) -> String {
    match &doc.get(n).data {
        NodeData::ProcessingInstruction { target, .. } => target.to_string(),
        other => panic!("no es una PI: {other:?}"),
    }
}

// --- Node-appendChild.html ---

// dom/nodes/Node-appendChild.html: "Appending to a leaf node." (con los subtests "Appending to a
// text node", "Appending to a comment" y "Appending to a doctype")
#[test]
fn append_child_to_leaf_nodes() {
    let mut doc = html_document(Some("title"));
    let t = text(&mut doc, "Foo");
    let c = comment(&mut doc, "Foo");
    let dt = doc.doctype().unwrap();
    for leaf in [t, c, dt] {
        let fail = text(&mut doc, "fail");
        assert_eq!(doc.append_child(leaf, fail), Err(HRE));
    }
}

// dom/nodes/Node-appendChild.html: "Appending a document"
#[test]
fn append_child_document() {
    let mut doc = html_document(Some("title"));
    let body = doc.body().unwrap();
    let frame_doc = raw_document(&mut doc);
    assert_eq!(doc.append_child(body, frame_doc), Err(HRE));
}

// --- pre-insertion-validation-notfound.js (para insertBefore y replaceChild) ---

type InsertFunc = fn(&mut Document, NodeId, NodeId, NodeId) -> Result<NodeId, DomException>;

fn insert_before_func(
    doc: &mut Document,
    parent: NodeId,
    node: NodeId,
    child: NodeId,
) -> Result<NodeId, DomException> {
    doc.insert_before_checked(parent, node, Some(child))
}

fn replace_child_func(
    doc: &mut Document,
    parent: NodeId,
    node: NodeId,
    child: NodeId,
) -> Result<NodeId, DomException> {
    doc.replace_child(parent, node, child)
}

/// `getNonParentNodes()` de `pre-insertion-validation-notfound.js`.
fn non_parent_nodes(doc: &mut Document) -> Vec<NodeId> {
    vec![
        doctype(doc, "html", "", ""),
        text(doc, "text"),
        pi(doc, "foo", "bar"),
        comment(doc, "comment"),
        cdata_raw(doc, "data"),
    ]
}

// pre-insertion-validation-notfound.js: "Should check the 'parent' type before checking whether
// 'child' is a child of 'parent'"
fn notfound_step1_before_step3(insert: InsertFunc) {
    let mut doc = Document::new();
    let parents = non_parent_nodes(&mut doc);
    let child = elem(&mut doc, "div");
    let node = elem(&mut doc, "div");
    for parent in parents {
        assert_eq!(insert(&mut doc, parent, node, child), Err(HRE));
    }
}

// pre-insertion-validation-notfound.js: "Should check that 'node' is not an ancestor of 'parent'
// before checking whether 'child' is a child of 'parent'"
fn notfound_step2_before_step3(insert: InsertFunc) {
    let mut doc = Document::new();
    let parent = elem(&mut doc, "div");
    let child = elem(&mut doc, "div");
    let node = elem(&mut doc, "div");
    append(&mut doc, node, parent);
    assert_eq!(insert(&mut doc, parent, node, child), Err(HRE));
}

// pre-insertion-validation-notfound.js: "Should check whether 'child' is a child of 'parent'
// before checking whether 'node' is of a type that can have a parent."
fn notfound_step3_before_step4(insert: InsertFunc) {
    let mut doc = Document::new();
    let parent = elem(&mut doc, "div");
    let child = elem(&mut doc, "div");
    let node = raw_document(&mut doc);
    assert_eq!(insert(&mut doc, parent, node, child), Err(NFE));
}

// pre-insertion-validation-notfound.js: "Should check whether 'child' is a child of 'parent'
// before checking whether 'node' is of a type that can have a parent of the type that 'parent'
// is."
fn notfound_step3_before_step5(insert: InsertFunc) {
    let mut doc = Document::new();
    let child = elem(&mut doc, "div");
    let node = text(&mut doc, "");
    // createDocument(null, "foo", null): un documento con un elemento `foo`.
    let parent = raw_document(&mut doc);
    let foo_el = elem_ns(&mut doc, None, "foo");
    append(&mut doc, parent, foo_el);
    assert_eq!(insert(&mut doc, parent, node, child), Err(NFE));

    let node = doctype(&mut doc, "html", "", "");
    let div = elem(&mut doc, "div");
    let frag = doc.create_document_fragment();
    for parent in [div, frag] {
        assert_eq!(insert(&mut doc, parent, node, child), Err(NFE));
    }
}

// pre-insertion-validation-notfound.js: "Should check whether 'child' is a child of 'parent'
// before checking whether 'node' can be inserted into the document given the kids the document
// has right now."
fn notfound_step3_before_step6(insert: InsertFunc) {
    let mut doc = Document::new();
    let child = elem(&mut doc, "div");
    let parent = raw_document(&mut doc);

    let node = doc.create_document_fragment();
    let d1 = elem(&mut doc, "div");
    let d2 = elem(&mut doc, "div");
    append(&mut doc, node, d1);
    append(&mut doc, node, d2);
    assert_eq!(insert(&mut doc, parent, node, child), Err(NFE));

    let node = elem(&mut doc, "div");
    let d3 = elem(&mut doc, "div");
    append(&mut doc, parent, d3);
    assert_eq!(insert(&mut doc, parent, node, child), Err(NFE));

    doc.remove_self(d3);
    let dt = doctype(&mut doc, "html", "", "");
    append(&mut doc, parent, dt);
    let node = doctype(&mut doc, "html", "", "");
    assert_eq!(insert(&mut doc, parent, node, child), Err(NFE));
}

// dom/nodes/Node-insertBefore.html (pre-insertion-validation-notfound.js): los cinco subtests
// de orden de pasos con insertBefore.
#[test]
fn insert_before_notfound_step_order() {
    notfound_step1_before_step3(insert_before_func);
    notfound_step2_before_step3(insert_before_func);
    notfound_step3_before_step4(insert_before_func);
    notfound_step3_before_step5(insert_before_func);
    notfound_step3_before_step6(insert_before_func);
}

// dom/nodes/Node-replaceChild.html (pre-insertion-validation-notfound.js): los cinco subtests
// de orden de pasos con replaceChild.
#[test]
fn replace_child_notfound_step_order() {
    notfound_step1_before_step3(replace_child_func);
    notfound_step2_before_step3(replace_child_func);
    notfound_step3_before_step4(replace_child_func);
    notfound_step3_before_step5(replace_child_func);
    notfound_step3_before_step6(replace_child_func);
}

// --- pre-insertion-validation-hierarchy.js con insertBefore(node, null) ---

fn insert_null(doc: &mut Document, parent: NodeId, node: NodeId) -> Result<NodeId, DomException> {
    doc.insert_before_checked(parent, node, None)
}

// dom/nodes/Node-insertBefore.html (pre-insertion-validation-hierarchy.js): "If node is a
// host-including inclusive ancestor of parent, then throw a HierarchyRequestError DOMException."
#[test]
fn insert_before_hierarchy_inclusive_ancestor() {
    let mut doc = html_document(Some("title"));
    let body = doc.body().unwrap();
    let html = doc.document_element().unwrap();
    assert_eq!(insert_null(&mut doc, body, body), Err(HRE));
    assert_eq!(insert_null(&mut doc, body, html), Err(HRE));
}

// dom/nodes/Node-insertBefore.html (pre-insertion-validation-hierarchy.js): "If node is not a
// DocumentFragment, DocumentType, Element, Text, ProcessingInstruction, or Comment node, then
// throw a HierarchyRequestError DOMException."
#[test]
fn insert_before_hierarchy_document_node() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let doc2 = raw_document(&mut doc);
    assert_eq!(insert_null(&mut doc, root, doc2), Err(HRE));
}

// dom/nodes/Node-insertBefore.html (pre-insertion-validation-hierarchy.js): "If node is a Text
// node and parent is a document, then throw a HierarchyRequestError DOMException."
#[test]
fn insert_before_hierarchy_text_in_document() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let t = text(&mut doc, "text");
    assert_eq!(insert_null(&mut doc, root, t), Err(HRE));
}

// dom/nodes/Node-insertBefore.html (pre-insertion-validation-hierarchy.js): "If node is a
// doctype and parent is not a document, then throw a HierarchyRequestError DOMException."
#[test]
fn insert_before_hierarchy_doctype_in_element() {
    let mut doc = html_document(Some("title"));
    let dt = doc.child_nodes(doc.root())[0];
    let a = elem(&mut doc, "a");
    assert_eq!(insert_null(&mut doc, a, dt), Err(HRE));
}

// dom/nodes/Node-insertBefore.html (pre-insertion-validation-hierarchy.js): "If node is a
// DocumentFragment with multiple elements and parent is a document, then throw a
// HierarchyRequestError DOMException."
#[test]
fn insert_before_hierarchy_fragment_multiple_elements() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let html = doc.document_element().unwrap();
    doc.remove_self(html);
    let df = doc.create_document_fragment();
    let a = elem(&mut doc, "a");
    let b = elem(&mut doc, "b");
    append(&mut doc, df, a);
    append(&mut doc, df, b);
    assert_eq!(insert_null(&mut doc, root, df), Err(HRE));
}

// dom/nodes/Node-insertBefore.html (pre-insertion-validation-hierarchy.js): "If node is a
// DocumentFragment with an element and parent is a document with another element, then throw a
// HierarchyRequestError DOMException."
#[test]
fn insert_before_hierarchy_fragment_element_with_element() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let df = doc.create_document_fragment();
    let a = elem(&mut doc, "a");
    append(&mut doc, df, a);
    assert_eq!(insert_null(&mut doc, root, df), Err(HRE));
}

// dom/nodes/Node-insertBefore.html (pre-insertion-validation-hierarchy.js): "If node is an
// Element and parent is a document with another element, then throw a HierarchyRequestError
// DOMException."
#[test]
fn insert_before_hierarchy_element_with_element() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let el = elem(&mut doc, "a");
    assert_eq!(insert_null(&mut doc, root, el), Err(HRE));
}

// dom/nodes/Node-insertBefore.html (pre-insertion-validation-hierarchy.js): "If node is a
// doctype and parent is a document with another doctype, then throw a HierarchyRequestError
// DOMException."
#[test]
fn insert_before_hierarchy_doctype_with_doctype() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let first = doc.child_nodes(root)[0];
    let dt = doc.clone_node(first, false);
    let html = doc.document_element().unwrap();
    doc.remove_self(html);
    assert_eq!(insert_null(&mut doc, root, dt), Err(HRE));
}

// dom/nodes/Node-insertBefore.html (pre-insertion-validation-hierarchy.js): "If node is a
// doctype and parent is a document with an element, then throw a HierarchyRequestError
// DOMException."
#[test]
fn insert_before_hierarchy_doctype_with_element() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let first = doc.child_nodes(root)[0];
    let dt = doc.clone_node(first, false);
    doc.remove_self(first);
    assert_eq!(insert_null(&mut doc, root, dt), Err(HRE));
}

// --- Node-insertBefore.html ---

// dom/nodes/Node-insertBefore.html: "Calling insertBefore an a leaf node {DocumentType, Text,
// Comment, ProcessingInstruction} must throw HIERARCHY_REQUEST_ERR."
#[test]
fn insert_before_leaf_nodes() {
    let mut doc = html_document(Some("title"));
    let dt = doc.doctype().unwrap();
    let t = text(&mut doc, "Foo");
    let c = comment(&mut doc, "Foo");
    let p = pi(&mut doc, "foo", "bar");
    for node in [dt, t, c, p] {
        let fail = text(&mut doc, "fail");
        assert_eq!(doc.insert_before_checked(node, fail, None), Err(HRE));
        // Sería el paso 2.
        assert_eq!(doc.insert_before_checked(node, node, None), Err(HRE));
        // Sería el paso 3.
        let child = text(&mut doc, "child");
        assert_eq!(doc.insert_before_checked(node, node, Some(child)), Err(HRE));
    }
}

// dom/nodes/Node-insertBefore.html: "Calling insertBefore with an inclusive ancestor of the
// context object must throw HIERARCHY_REQUEST_ERR."
#[test]
fn insert_before_inclusive_ancestor() {
    let mut doc = html_document(Some("title"));
    let body = doc.body().unwrap();
    let log = elem(&mut doc, "div");
    set_attr(&mut doc, log, "id", "log");
    append(&mut doc, body, log);
    let html = doc.document_element().unwrap();
    assert_eq!(doc.insert_before_checked(body, body, Some(log)), Err(HRE));
    assert_eq!(doc.insert_before_checked(body, html, Some(log)), Err(HRE));
}

// dom/nodes/Node-insertBefore.html: "Calling insertBefore with a reference child whose parent is
// not the context node must throw a NotFoundError."
#[test]
fn insert_before_reference_not_child() {
    let mut doc = Document::new();
    let a = elem(&mut doc, "div");
    let b = elem(&mut doc, "div");
    let c = elem(&mut doc, "div");
    assert_eq!(doc.insert_before_checked(a, b, Some(c)), Err(NFE));
}

// dom/nodes/Node-insertBefore.html: "If the context node is a document, inserting a document or
// text node should throw a HierarchyRequestError."
#[test]
fn insert_before_document_or_text_into_document() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let html = doc.document_element();
    let doc2 = raw_document(&mut doc);
    assert_eq!(doc.insert_before_checked(root, doc2, html), Err(HRE));
    let t = text(&mut doc, "text");
    assert_eq!(doc.insert_before_checked(root, t, html), Err(HRE));
}

// dom/nodes/Node-insertBefore.html: "If the context node is a document, inserting a
// DocumentFragment that contains a text node or too many elements should throw a
// HierarchyRequestError."
#[test]
fn insert_before_fragment_with_text_or_many_elements_into_document() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let html = doc.document_element().unwrap();
    doc.remove_child(root, html).unwrap();

    let df = doc.create_document_fragment();
    let a = elem(&mut doc, "a");
    let b = elem(&mut doc, "b");
    append(&mut doc, df, a);
    append(&mut doc, df, b);
    let first = doc.first_child(root);
    assert_eq!(doc.insert_before_checked(root, df, first), Err(HRE));

    let df = doc.create_document_fragment();
    let t = text(&mut doc, "text");
    append(&mut doc, df, t);
    assert_eq!(doc.insert_before_checked(root, df, first), Err(HRE));

    let df = doc.create_document_fragment();
    let c = comment(&mut doc, "comment");
    let t = text(&mut doc, "text");
    append(&mut doc, df, c);
    append(&mut doc, df, t);
    assert_eq!(doc.insert_before_checked(root, df, first), Err(HRE));
}

// dom/nodes/Node-insertBefore.html: "If the context node is a document, inserting a
// DocumentFragment with an element if there already is an element child should throw a
// HierarchyRequestError."
#[test]
fn insert_before_fragment_with_element_when_element_exists() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let c = comment(&mut doc, "foo");
    let comment_node = append(&mut doc, root, c);
    let (dt, html) = (doc.doctype().unwrap(), doc.document_element().unwrap());
    assert_eq!(doc.child_nodes(root), vec![dt, html, comment_node]);

    let df = doc.create_document_fragment();
    let a = elem(&mut doc, "a");
    append(&mut doc, df, a);
    assert_eq!(doc.insert_before_checked(root, df, Some(dt)), Err(HRE));
    assert_eq!(doc.insert_before_checked(root, df, Some(html)), Err(HRE));
    assert_eq!(
        doc.insert_before_checked(root, df, Some(comment_node)),
        Err(HRE)
    );
    assert_eq!(doc.insert_before_checked(root, df, None), Err(HRE));
}

/// `[comment, doctype]`: comentario antes del doctype y sin elemento documento.
fn doc_comment_then_doctype() -> (Document, NodeId, NodeId) {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let c = comment(&mut doc, "foo");
    let first = doc.first_child(root);
    let comment_node = doc.insert_before_checked(root, c, first).unwrap();
    let html = doc.document_element().unwrap();
    doc.remove_child(root, html).unwrap();
    let dt = doc.doctype().unwrap();
    assert_eq!(doc.child_nodes(root), vec![comment_node, dt]);
    (doc, comment_node, dt)
}

// dom/nodes/Node-insertBefore.html: "If the context node is a document and a doctype is
// following the reference child, inserting a DocumentFragment with an element should throw a
// HierarchyRequestError."
#[test]
fn insert_before_fragment_with_element_before_doctype_child() {
    let (mut doc, _comment, dt) = doc_comment_then_doctype();
    let root = doc.root();
    let df = doc.create_document_fragment();
    let a = elem(&mut doc, "a");
    append(&mut doc, df, a);
    assert_eq!(doc.insert_before_checked(root, df, Some(dt)), Err(HRE));
}

// dom/nodes/Node-insertBefore.html: "If the context node is a document, inserting a
// DocumentFragment with an element before the doctype should throw a HierarchyRequestError."
#[test]
fn insert_before_fragment_with_element_before_doctype() {
    let (mut doc, comment_node, _dt) = doc_comment_then_doctype();
    let root = doc.root();
    let df = doc.create_document_fragment();
    let a = elem(&mut doc, "a");
    append(&mut doc, df, a);
    assert_eq!(
        doc.insert_before_checked(root, df, Some(comment_node)),
        Err(HRE)
    );
}

// dom/nodes/Node-insertBefore.html: "If the context node is a document, inserting an element if
// there already is an element child should throw a HierarchyRequestError."
#[test]
fn insert_before_element_when_element_exists() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let c = comment(&mut doc, "foo");
    let comment_node = append(&mut doc, root, c);
    let (dt, html) = (doc.doctype().unwrap(), doc.document_element().unwrap());
    assert_eq!(doc.child_nodes(root), vec![dt, html, comment_node]);

    let a = elem(&mut doc, "a");
    assert_eq!(doc.insert_before_checked(root, a, Some(dt)), Err(HRE));
    assert_eq!(doc.insert_before_checked(root, a, Some(html)), Err(HRE));
    assert_eq!(
        doc.insert_before_checked(root, a, Some(comment_node)),
        Err(HRE)
    );
    assert_eq!(doc.insert_before_checked(root, a, None), Err(HRE));
}

// dom/nodes/Node-insertBefore.html: "If the context node is a document, inserting an element
// before the doctype should throw a HierarchyRequestError."
#[test]
fn insert_before_element_before_doctype_child() {
    let (mut doc, _comment, dt) = doc_comment_then_doctype();
    let root = doc.root();
    let a = elem(&mut doc, "a");
    assert_eq!(doc.insert_before_checked(root, a, Some(dt)), Err(HRE));
}

// dom/nodes/Node-insertBefore.html: "If the context node is a document and a doctype is
// following the reference child, inserting an element should throw a HierarchyRequestError."
#[test]
fn insert_before_element_with_doctype_following() {
    let (mut doc, comment_node, _dt) = doc_comment_then_doctype();
    let root = doc.root();
    let a = elem(&mut doc, "a");
    assert_eq!(
        doc.insert_before_checked(root, a, Some(comment_node)),
        Err(HRE)
    );
}

// dom/nodes/Node-insertBefore.html: "If the context node is a document, inserting a doctype if
// there already is a doctype child should throw a HierarchyRequestError."
#[test]
fn insert_before_doctype_when_doctype_exists() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let c = comment(&mut doc, "foo");
    let first = doc.first_child(root);
    let comment_node = doc.insert_before_checked(root, c, first).unwrap();
    let (dt, html) = (doc.doctype().unwrap(), doc.document_element().unwrap());
    assert_eq!(doc.child_nodes(root), vec![comment_node, dt, html]);

    let new_dt = doctype(&mut doc, "html", "", "");
    assert_eq!(
        doc.insert_before_checked(root, new_dt, Some(comment_node)),
        Err(HRE)
    );
    assert_eq!(doc.insert_before_checked(root, new_dt, Some(dt)), Err(HRE));
    assert_eq!(
        doc.insert_before_checked(root, new_dt, Some(html)),
        Err(HRE)
    );
    assert_eq!(doc.insert_before_checked(root, new_dt, None), Err(HRE));
}

/// `[html, comment]`: sin doctype y con un comentario tras el elemento documento.
fn doc_element_then_comment() -> (Document, NodeId) {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let c = comment(&mut doc, "foo");
    let comment_node = append(&mut doc, root, c);
    let dt = doc.doctype().unwrap();
    doc.remove_child(root, dt).unwrap();
    let html = doc.document_element().unwrap();
    assert_eq!(doc.child_nodes(root), vec![html, comment_node]);
    (doc, comment_node)
}

// dom/nodes/Node-insertBefore.html: "If the context node is a document, inserting a doctype
// after the document element should throw a HierarchyRequestError."
#[test]
fn insert_before_doctype_after_document_element() {
    let (mut doc, comment_node) = doc_element_then_comment();
    let root = doc.root();
    let dt = doctype(&mut doc, "html", "", "");
    assert_eq!(
        doc.insert_before_checked(root, dt, Some(comment_node)),
        Err(HRE)
    );
}

// dom/nodes/Node-insertBefore.html: "If the context node is a document with and element child,
// appending a doctype should throw a HierarchyRequestError."
#[test]
fn insert_before_append_doctype_with_element() {
    let (mut doc, _comment) = doc_element_then_comment();
    let root = doc.root();
    let dt = doctype(&mut doc, "html", "", "");
    assert_eq!(doc.insert_before_checked(root, dt, None), Err(HRE));
}

// dom/nodes/Node-insertBefore.html: "If the context node is a DocumentFragment, inserting a
// document or a doctype should throw a HierarchyRequestError."
#[test]
fn insert_before_document_or_doctype_into_fragment() {
    let mut doc = Document::new();
    let df = doc.create_document_fragment();
    let a = elem(&mut doc, "a");
    append(&mut doc, df, a);
    let other = raw_document(&mut doc);
    assert_eq!(doc.insert_before_checked(df, other, Some(a)), Err(HRE));
    assert_eq!(doc.insert_before_checked(df, other, None), Err(HRE));
    let dt = doctype(&mut doc, "html", "", "");
    assert_eq!(doc.insert_before_checked(df, dt, Some(a)), Err(HRE));
    assert_eq!(doc.insert_before_checked(df, dt, None), Err(HRE));
}

// dom/nodes/Node-insertBefore.html: "If the context node is an element, inserting a document or
// a doctype should throw a HierarchyRequestError."
#[test]
fn insert_before_document_or_doctype_into_element() {
    let mut doc = Document::new();
    let el = elem(&mut doc, "div");
    let a = elem(&mut doc, "a");
    append(&mut doc, el, a);
    let other = raw_document(&mut doc);
    assert_eq!(doc.insert_before_checked(el, other, Some(a)), Err(HRE));
    assert_eq!(doc.insert_before_checked(el, other, None), Err(HRE));
    let dt = doctype(&mut doc, "html", "", "");
    assert_eq!(doc.insert_before_checked(el, dt, Some(a)), Err(HRE));
    assert_eq!(doc.insert_before_checked(el, dt, None), Err(HRE));
}

// dom/nodes/Node-insertBefore.html: "Inserting a node before itself should not move the node"
#[test]
fn insert_before_itself() {
    let mut doc = Document::new();
    let a = elem(&mut doc, "div");
    let b = elem(&mut doc, "div");
    let c = elem(&mut doc, "div");
    append(&mut doc, a, b);
    append(&mut doc, a, c);
    assert_eq!(doc.child_nodes(a), vec![b, c]);
    assert_eq!(doc.insert_before_checked(a, b, Some(b)), Ok(b));
    assert_eq!(doc.child_nodes(a), vec![b, c]);
    assert_eq!(doc.insert_before_checked(a, c, Some(c)), Ok(c));
    assert_eq!(doc.child_nodes(a), vec![b, c]);
}

// --- Node-replaceChild.html ---

// dom/nodes/Node-replaceChild.html: "If child's parent is not the context node, a NotFoundError
// exception should be thrown"
#[test]
fn replace_child_not_a_child() {
    let mut doc = Document::new();
    let a = elem(&mut doc, "div");
    let b = elem(&mut doc, "div");
    let c = elem(&mut doc, "div");
    assert_eq!(doc.replace_child(a, b, c), Err(NFE));
    let d = elem(&mut doc, "div");
    append(&mut doc, d, b);
    assert_eq!(doc.replace_child(a, b, c), Err(NFE));
    assert_eq!(doc.replace_child(a, b, a), Err(NFE));
}

// dom/nodes/Node-replaceChild.html: "If the context node is not a node that can contain
// children, a HierarchyRequestError exception should be thrown"
#[test]
fn replace_child_non_parent_context() {
    let mut doc = Document::new();
    let nodes = non_parent_nodes(&mut doc);
    let a = elem(&mut doc, "div");
    let b = elem(&mut doc, "div");
    for node in nodes {
        assert_eq!(doc.replace_child(node, a, b), Err(HRE));
    }
}

// dom/nodes/Node-replaceChild.html: "If node is an inclusive ancestor of the context node, a
// HierarchyRequestError should be thrown."
#[test]
fn replace_child_inclusive_ancestor() {
    let mut doc = Document::new();
    let a = elem(&mut doc, "div");
    let b = elem(&mut doc, "div");
    assert_eq!(doc.replace_child(a, a, a), Err(HRE));
    append(&mut doc, a, b);
    assert_eq!(doc.replace_child(a, a, b), Err(HRE));
    let c = elem(&mut doc, "div");
    append(&mut doc, c, a);
    assert_eq!(doc.replace_child(a, c, b), Err(HRE));
}

// dom/nodes/Node-replaceChild.html: "If the context node is a document, inserting a document or
// text node should throw a HierarchyRequestError."
#[test]
fn replace_child_document_or_text_into_document() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let html = doc.document_element().unwrap();
    let doc2 = raw_document(&mut doc);
    assert_eq!(doc.replace_child(root, doc2, html), Err(HRE));
    let t = text(&mut doc, "text");
    assert_eq!(doc.replace_child(root, t, html), Err(HRE));
}

// dom/nodes/Node-replaceChild.html: "If the context node is a document, inserting a
// DocumentFragment that contains a text node or too many elements should throw a
// HierarchyRequestError."
#[test]
fn replace_child_fragment_with_text_or_many_elements() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let html = doc.document_element().unwrap();

    let df = doc.create_document_fragment();
    let a = elem(&mut doc, "a");
    let b = elem(&mut doc, "b");
    append(&mut doc, df, a);
    append(&mut doc, df, b);
    assert_eq!(doc.replace_child(root, df, html), Err(HRE));

    let df = doc.create_document_fragment();
    let t = text(&mut doc, "text");
    append(&mut doc, df, t);
    assert_eq!(doc.replace_child(root, df, html), Err(HRE));

    let df = doc.create_document_fragment();
    let c = comment(&mut doc, "comment");
    let t = text(&mut doc, "text");
    append(&mut doc, df, c);
    append(&mut doc, df, t);
    assert_eq!(doc.replace_child(root, df, html), Err(HRE));
}

// dom/nodes/Node-replaceChild.html: "If the context node is a document (without element
// children), inserting a DocumentFragment that contains multiple elements should throw a
// HierarchyRequestError."
#[test]
fn replace_child_fragment_multiple_elements_without_element() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let html = doc.document_element().unwrap();
    doc.remove_child(root, html).unwrap();
    let df = doc.create_document_fragment();
    let a = elem(&mut doc, "a");
    let b = elem(&mut doc, "b");
    append(&mut doc, df, a);
    append(&mut doc, df, b);
    let dt = doc.doctype().unwrap();
    assert_eq!(doc.replace_child(root, df, dt), Err(HRE));
}

// dom/nodes/Node-replaceChild.html: "If the context node is a document, inserting a
// DocumentFragment with an element if there already is an element child should throw a
// HierarchyRequestError."
#[test]
fn replace_child_fragment_with_element_when_element_exists() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let c = comment(&mut doc, "foo");
    let comment_node = append(&mut doc, root, c);
    let (dt, html) = (doc.doctype().unwrap(), doc.document_element().unwrap());
    assert_eq!(doc.child_nodes(root), vec![dt, html, comment_node]);
    let df = doc.create_document_fragment();
    let a = elem(&mut doc, "a");
    append(&mut doc, df, a);
    assert_eq!(doc.replace_child(root, df, comment_node), Err(HRE));
    assert_eq!(doc.replace_child(root, df, dt), Err(HRE));
}

// dom/nodes/Node-replaceChild.html: "If the context node is a document, inserting a
// DocumentFragment with an element before the doctype should throw a HierarchyRequestError."
#[test]
fn replace_child_fragment_with_element_before_doctype() {
    let (mut doc, comment_node, _dt) = doc_comment_then_doctype();
    let root = doc.root();
    let df = doc.create_document_fragment();
    let a = elem(&mut doc, "a");
    append(&mut doc, df, a);
    assert_eq!(doc.replace_child(root, df, comment_node), Err(HRE));
}

// dom/nodes/Node-replaceChild.html: "If the context node is a document, inserting an element if
// there already is an element child should throw a HierarchyRequestError."
#[test]
fn replace_child_element_when_element_exists() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let c = comment(&mut doc, "foo");
    let comment_node = append(&mut doc, root, c);
    let (dt, html) = (doc.doctype().unwrap(), doc.document_element().unwrap());
    assert_eq!(doc.child_nodes(root), vec![dt, html, comment_node]);
    let a = elem(&mut doc, "a");
    assert_eq!(doc.replace_child(root, a, comment_node), Err(HRE));
    assert_eq!(doc.replace_child(root, a, dt), Err(HRE));
}

// dom/nodes/Node-replaceChild.html: "If the context node is a document, inserting an element
// before the doctype should throw a HierarchyRequestError."
#[test]
fn replace_child_element_before_doctype() {
    let (mut doc, comment_node, _dt) = doc_comment_then_doctype();
    let root = doc.root();
    let a = elem(&mut doc, "a");
    assert_eq!(doc.replace_child(root, a, comment_node), Err(HRE));
}

// dom/nodes/Node-replaceChild.html: "If the context node is a document, inserting a doctype if
// there already is a doctype child should throw a HierarchyRequestError."
#[test]
fn replace_child_doctype_when_doctype_exists() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let c = comment(&mut doc, "foo");
    let first = doc.first_child(root);
    let comment_node = doc.insert_before_checked(root, c, first).unwrap();
    let (dt, html) = (doc.doctype().unwrap(), doc.document_element().unwrap());
    assert_eq!(doc.child_nodes(root), vec![comment_node, dt, html]);
    let new_dt = doctype(&mut doc, "html", "", "");
    assert_eq!(doc.replace_child(root, new_dt, comment_node), Err(HRE));
    assert_eq!(doc.replace_child(root, new_dt, html), Err(HRE));
}

// dom/nodes/Node-replaceChild.html: "If the context node is a document, inserting a doctype
// after the document element should throw a HierarchyRequestError."
#[test]
fn replace_child_doctype_after_document_element() {
    let (mut doc, comment_node) = doc_element_then_comment();
    let root = doc.root();
    let dt = doctype(&mut doc, "html", "", "");
    assert_eq!(doc.replace_child(root, dt, comment_node), Err(HRE));
}

// dom/nodes/Node-replaceChild.html: "If the context node is a DocumentFragment, inserting a
// document or a doctype should throw a HierarchyRequestError."
#[test]
fn replace_child_document_or_doctype_into_fragment() {
    let mut doc = Document::new();
    let df = doc.create_document_fragment();
    let a = elem(&mut doc, "a");
    append(&mut doc, df, a);
    let other = raw_document(&mut doc);
    assert_eq!(doc.replace_child(df, other, a), Err(HRE));
    let dt = doctype(&mut doc, "html", "", "");
    assert_eq!(doc.replace_child(df, dt, a), Err(HRE));
}

// dom/nodes/Node-replaceChild.html: "If the context node is an element, inserting a document or
// a doctype should throw a HierarchyRequestError."
#[test]
fn replace_child_document_or_doctype_into_element() {
    let mut doc = Document::new();
    let el = elem(&mut doc, "div");
    let a = elem(&mut doc, "a");
    append(&mut doc, el, a);
    let other = raw_document(&mut doc);
    assert_eq!(doc.replace_child(el, other, a), Err(HRE));
    let dt = doctype(&mut doc, "html", "", "");
    assert_eq!(doc.replace_child(el, dt, a), Err(HRE));
}

// dom/nodes/Node-replaceChild.html: "Replacing a node with its next sibling should work (2
// children)"
#[test]
fn replace_child_with_next_sibling_two_children() {
    let mut doc = Document::new();
    let a = elem(&mut doc, "div");
    let b = elem(&mut doc, "div");
    let c = elem(&mut doc, "div");
    append(&mut doc, a, b);
    append(&mut doc, a, c);
    assert_eq!(doc.child_nodes(a), vec![b, c]);
    assert_eq!(doc.replace_child(a, c, b), Ok(b));
    assert_eq!(doc.child_nodes(a), vec![c]);
}

// dom/nodes/Node-replaceChild.html: "Replacing a node with its next sibling should work (4
// children)"
#[test]
fn replace_child_with_next_sibling_four_children() {
    let mut doc = Document::new();
    let a = elem(&mut doc, "div");
    let kids: Vec<NodeId> = (0..4).map(|_| elem(&mut doc, "div")).collect();
    for &k in &kids {
        append(&mut doc, a, k);
    }
    let (b, c, d, e) = (kids[0], kids[1], kids[2], kids[3]);
    assert_eq!(doc.child_nodes(a), vec![b, c, d, e]);
    assert_eq!(doc.replace_child(a, d, c), Ok(c));
    assert_eq!(doc.child_nodes(a), vec![b, d, e]);
}

// dom/nodes/Node-replaceChild.html: "Replacing a node with itself should not move the node"
#[test]
fn replace_child_with_itself() {
    let mut doc = Document::new();
    let a = elem(&mut doc, "div");
    let b = elem(&mut doc, "div");
    let c = elem(&mut doc, "div");
    append(&mut doc, a, b);
    append(&mut doc, a, c);
    assert_eq!(doc.child_nodes(a), vec![b, c]);
    assert_eq!(doc.replace_child(a, b, b), Ok(b));
    assert_eq!(doc.child_nodes(a), vec![b, c]);
    assert_eq!(doc.replace_child(a, c, c), Ok(c));
    assert_eq!(doc.child_nodes(a), vec![b, c]);
}

// dom/nodes/Node-replaceChild.html: "If the context node is a document, inserting a new doctype
// should work." (doc2 es un nodo Document suelto en la misma arena)
#[test]
fn replace_child_new_doctype() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let dt = doc.doctype().unwrap();
    let html = doc.document_element().unwrap();
    assert_eq!(doc.child_nodes(root), vec![dt, html]);

    let doc2 = raw_document(&mut doc);
    let dt2 = doctype(&mut doc, "html", "", "");
    append(&mut doc, doc2, dt2);
    let html2 = elem(&mut doc, "html");
    append(&mut doc, doc2, html2);
    assert_eq!(doc.child_nodes(doc2), vec![dt2, html2]);

    doc.replace_child(root, dt2, dt).unwrap();
    assert_eq!(doc.child_nodes(root), vec![dt2, html]);
    assert_eq!(doc.child_nodes(doc2), vec![html2]);
    assert_eq!(doc.parent(dt), None);
    assert_eq!(doc.owner_document(dt), Some(root));
    assert_eq!(doc.parent(dt2), Some(root));
    assert_eq!(doc.owner_document(dt2), Some(root));
}

// dom/nodes/Node-replaceChild.html: "Replacing the document element with a DocumentFragment
// containing a single element should work."
#[test]
fn replace_document_element_with_fragment() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let df = doc.create_document_fragment();
    let a = elem(&mut doc, "a");
    append(&mut doc, df, a);
    let html = doc.document_element().unwrap();
    assert_eq!(doc.replace_child(root, df, html), Ok(html));
    let dt = doc.doctype().unwrap();
    assert_eq!(doc.child_nodes(root), vec![dt, a]);
}

// dom/nodes/Node-replaceChild.html: "Replacing the document element with a DocumentFragment
// containing a single element and comments should work."
#[test]
fn replace_document_element_with_fragment_and_comments() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let df = doc.create_document_fragment();
    let a = comment(&mut doc, "a");
    let b = elem(&mut doc, "b");
    let c = comment(&mut doc, "c");
    append(&mut doc, df, a);
    append(&mut doc, df, b);
    append(&mut doc, df, c);
    let html = doc.document_element().unwrap();
    assert_eq!(doc.replace_child(root, df, html), Ok(html));
    let dt = doc.doctype().unwrap();
    assert_eq!(doc.child_nodes(root), vec![dt, a, b, c]);
}

// dom/nodes/Node-replaceChild.html: "Replacing the document element with a single element should
// work."
#[test]
fn replace_document_element_with_element() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let a = elem(&mut doc, "a");
    let html = doc.document_element().unwrap();
    assert_eq!(doc.replace_child(root, a, html), Ok(html));
    let dt = doc.doctype().unwrap();
    assert_eq!(doc.child_nodes(root), vec![dt, a]);
}

// dom/nodes/Node-replaceChild.html: "replaceChild should work in the presence of mutation
// events." (sin el listener de DOMNodeRemoved: no hay mutation events)
#[test]
fn replace_child_sibling_in_body() {
    // <body><a><b></b><c></c></a>
    let mut doc = html_document(Some("title"));
    let body = doc.body().unwrap();
    let a = elem(&mut doc, "a");
    append(&mut doc, body, a);
    let b = elem(&mut doc, "b");
    append(&mut doc, a, b);
    let c = elem(&mut doc, "c");
    append(&mut doc, a, c);
    assert_eq!(doc.replace_child(a, c, b), Ok(b));
    assert_eq!(doc.parent(b), None);
    assert_eq!(doc.first_child(a), Some(c));
    assert_eq!(doc.parent(c), Some(a));
}

// dom/nodes/Node-replaceChild.html: "Replacing an element with a DocumentFragment should allow a
// child of the DocumentFragment to be found by Id."
#[test]
fn replace_child_fragment_found_by_id() {
    let mut doc = html_document(Some("title"));
    let body = doc.body().unwrap();
    let parent = elem(&mut doc, "div");
    append(&mut doc, body, parent);
    let child = elem(&mut doc, "div");
    append(&mut doc, parent, child);
    let df = doc.create_document_fragment();
    let frag_child = elem(&mut doc, "div");
    append(&mut doc, df, frag_child);
    set_attr(&mut doc, frag_child, "id", "findme");
    doc.replace_child(parent, df, child).unwrap();
    assert_eq!(by_id(&doc, "findme"), Some(frag_child));
}

// --- Node-removeChild.html (solo "the main document") ---

/// `creators.js`: element, text, comment.
fn create_with(doc: &mut Document, creator: &str, arg: &str) -> NodeId {
    match creator {
        "element" => elem(doc, arg),
        "text" => text(doc, arg),
        "comment" => comment(doc, arg),
        _ => unreachable!(),
    }
}

const CREATORS: [&str; 3] = ["element", "text", "comment"];

// dom/nodes/Node-removeChild.html: "Passing a detached {element, text, comment} from the main
// document to removeChild should not affect it."
#[test]
fn remove_child_detached_node() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let body = doc.body().unwrap();
    for creator in CREATORS {
        let s = create_with(&mut doc, creator, "a");
        assert_eq!(doc.owner_document(s), Some(root));
        assert_eq!(doc.remove_child(body, s), Err(NFE));
        assert_eq!(doc.owner_document(s), Some(root));
    }
}

// dom/nodes/Node-removeChild.html: "Passing a non-detached {element, text, comment} from the
// main document to removeChild should not affect it."
#[test]
fn remove_child_non_detached_node() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let body = doc.body().unwrap();
    let html = doc.document_element().unwrap();
    for creator in CREATORS {
        let s = create_with(&mut doc, creator, "b");
        append(&mut doc, html, s);
        assert_eq!(doc.owner_document(s), Some(root));
        assert_eq!(doc.remove_child(body, s), Err(NFE));
        assert_eq!(doc.owner_document(s), Some(root));
        assert_eq!(doc.parent(s), Some(html));
    }
}

// dom/nodes/Node-removeChild.html: "Calling removeChild on a {element, text, comment} from the
// main document with no children should throw NOT_FOUND_ERR."
#[test]
fn remove_child_from_childless_node() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let body = doc.body().unwrap();
    for creator in CREATORS {
        let s = create_with(&mut doc, creator, "test");
        append(&mut doc, body, s);
        assert_eq!(doc.owner_document(s), Some(root));
        assert_eq!(doc.remove_child(s, root), Err(NFE));
    }
}

// --- Node-cloneNode.html ---

/// `check_copy` + `assert_equal_node`: copia distinta con los mismos nombres y atributos.
fn check_copy(doc: &Document, orig: NodeId, copy: NodeId) {
    assert_ne!(orig, copy, "Object equality");
    assert_eq!(doc.node_type(copy), doc.node_type(orig), "nodeType");
    assert_eq!(doc.node_name(copy), doc.node_name(orig), "nodeName");
    if doc.node_type(orig) == NodeType::Element {
        assert_eq!(doc.prefix(copy), doc.prefix(orig), "prefix");
        assert_eq!(
            doc.namespace_uri(copy),
            doc.namespace_uri(orig),
            "namespaceURI"
        );
        assert_eq!(doc.local_name(copy), doc.local_name(orig), "localName");
        assert_eq!(doc.tag_name(copy), doc.tag_name(orig), "tagName");
        assert_eq!(doc.attributes_length(copy), doc.attributes_length(orig));
        let (a, b) = (doc.element(orig).unwrap(), doc.element(copy).unwrap());
        assert_eq!(a.attrs, b.attrs, "attributes");
        // Aproximación de `instanceof`: la misma interfaz HTML.
        assert_eq!(doc.html_kind(copy), doc.html_kind(orig), "interfaz");
    }
}

// dom/nodes/Node-cloneNode.html: "createElement(a)" ... "createElement(wbr)" (todos los nombres
// de la lista)
#[test]
fn clone_node_create_element() {
    let names = [
        "a",
        "abbr",
        "acronym",
        "address",
        "area",
        "article",
        "aside",
        "audio",
        "b",
        "base",
        "bdi",
        "bdo",
        "bgsound",
        "big",
        "blockquote",
        "body",
        "br",
        "button",
        "canvas",
        "caption",
        "center",
        "cite",
        "code",
        "col",
        "colgroup",
        "data",
        "datalist",
        "dialog",
        "dd",
        "del",
        "details",
        "dfn",
        "dir",
        "div",
        "dl",
        "dt",
        "embed",
        "fieldset",
        "figcaption",
        "figure",
        "font",
        "footer",
        "form",
        "frame",
        "frameset",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "head",
        "header",
        "hgroup",
        "hr",
        "html",
        "i",
        "iframe",
        "img",
        "input",
        "ins",
        "isindex",
        "kbd",
        "label",
        "legend",
        "li",
        "link",
        "main",
        "map",
        "mark",
        "marquee",
        "meta",
        "meter",
        "nav",
        "nobr",
        "noframes",
        "noscript",
        "object",
        "ol",
        "optgroup",
        "option",
        "output",
        "p",
        "param",
        "pre",
        "progress",
        "q",
        "rp",
        "rt",
        "ruby",
        "s",
        "samp",
        "script",
        "section",
        "select",
        "small",
        "source",
        "spacer",
        "span",
        "strike",
        "style",
        "sub",
        "summary",
        "sup",
        "table",
        "tbody",
        "td",
        "template",
        "textarea",
        "th",
        "time",
        "title",
        "tr",
        "tt",
        "track",
        "u",
        "ul",
        "var",
        "video",
        "unknown",
        "wbr",
    ];
    let mut doc = Document::new();
    for name in names {
        let element = elem(&mut doc, name);
        let copy = doc.clone_node(element, false);
        check_copy(&doc, element, copy);
        assert_eq!(doc.local_name(copy), name);
    }
}

// dom/nodes/Node-cloneNode.html: "createDocumentFragment"
#[test]
fn clone_node_document_fragment() {
    let mut doc = Document::new();
    let fragment = doc.create_document_fragment();
    let copy = doc.clone_node(fragment, false);
    check_copy(&doc, fragment, copy);
    assert_eq!(doc.node_type(copy), NodeType::DocumentFragment);
}

// dom/nodes/Node-cloneNode.html: "createTextNode"
#[test]
fn clone_node_text() {
    let mut doc = Document::new();
    let t = text(&mut doc, "hello world");
    let copy = doc.clone_node(t, false);
    check_copy(&doc, t, copy);
    assert_eq!(data(&doc, t), data(&doc, copy));
    assert_eq!(doc.whole_text(t), doc.whole_text(copy));
}

// dom/nodes/Node-cloneNode.html: "createComment"
#[test]
fn clone_node_comment() {
    let mut doc = Document::new();
    let c = comment(&mut doc, "a comment");
    let copy = doc.clone_node(c, false);
    check_copy(&doc, c, copy);
    assert_eq!(data(&doc, c), data(&doc, copy));
}

// dom/nodes/Node-cloneNode.html: "createElement with attributes"
#[test]
fn clone_node_element_with_attributes() {
    let mut doc = Document::new();
    let el = elem(&mut doc, "foo");
    set_attr(&mut doc, el, "a", "b");
    set_attr(&mut doc, el, "c", "d");
    let c = doc.clone_node(el, false);
    check_copy(&doc, el, c);
    assert_eq!(doc.attributes_length(c), 2);
    assert_eq!(get_attr(&doc, c, "a").as_deref(), Some("b"));
    assert_eq!(get_attr(&doc, c, "c").as_deref(), Some("d"));
    // Las copias de los Attr son nodos distintos.
    let (oa, ca) = (
        doc.attributes_item(el, 0).unwrap(),
        doc.attributes_item(c, 0).unwrap(),
    );
    assert_ne!(oa, ca);
}

// dom/nodes/Node-cloneNode.html: "createElementNS HTML"
#[test]
fn clone_node_element_ns_html() {
    let mut doc = Document::new();
    let el = elem_ns(&mut doc, Some(HTML_NS), "foo:div");
    let c = doc.clone_node(el, false);
    check_copy(&doc, el, c);
    assert_eq!(st(doc.prefix(c)).as_deref(), Some("foo"));
    assert_eq!(doc.html_kind(c), Some(bf_dom::HtmlElementKind::Div));
}

// dom/nodes/Node-cloneNode.html: "createElementNS non-HTML"
#[test]
fn clone_node_element_ns_non_html() {
    let mut doc = Document::new();
    let el = elem_ns(&mut doc, Some("http://www.example.com/"), "foo:div");
    let c = doc.clone_node(el, false);
    check_copy(&doc, el, c);
    assert_eq!(
        st(doc.namespace_uri(c)).as_deref(),
        Some("http://www.example.com/")
    );
    assert_eq!(doc.html_kind(c), None);
}

// dom/nodes/Node-cloneNode.html: "createProcessingInstruction"
#[test]
fn clone_node_processing_instruction() {
    let mut doc = Document::new();
    let p = pi(&mut doc, "target", "data");
    let copy = doc.clone_node(p, false);
    check_copy(&doc, p, copy);
    assert_eq!(data(&doc, p), data(&doc, copy));
    assert_eq!(pi_target(&doc, copy), "target");
}

/// Cuerpo común de "createAttribute" y "createAttributeNS".
fn check_attr_clone(doc: &mut Document, attr: NodeId) {
    let copy = doc.clone_node(attr, false);
    check_copy(doc, attr, copy);
    assert_eq!(attr_names(doc, attr), attr_names(doc, copy));
    assert_eq!(doc.attr_value(attr), doc.attr_value(copy));

    doc.set_attr_value(attr, ds("abc"));
    assert_eq!(attr_names(doc, attr), attr_names(doc, copy));
    assert_ne!(doc.attr_value(attr), doc.attr_value(copy));

    let copy2 = doc.clone_node(attr, false);
    check_copy(doc, attr, copy2);
    assert_eq!(attr_names(doc, attr), attr_names(doc, copy2));
    assert_eq!(doc.attr_value(attr), doc.attr_value(copy2));
}

// dom/nodes/Node-cloneNode.html: "createAttribute"
#[test]
fn clone_node_attribute() {
    let mut doc = Document::new();
    let attr = doc.create_attribute(&ds("class")).unwrap();
    check_attr_clone(&mut doc, attr);
}

// dom/nodes/Node-cloneNode.html: "createAttributeNS"
#[test]
fn clone_node_attribute_ns() {
    let mut doc = Document::new();
    let attr = doc
        .create_attribute_ns(Some(HTML_NS), &ds("foo:class"))
        .unwrap();
    check_attr_clone(&mut doc, attr);
}

// dom/nodes/Node-cloneNode.html: "implementation.createDocumentType"
#[test]
fn clone_node_doctype() {
    let mut doc = Document::new();
    let dt = doctype(&mut doc, "html", "public", "system");
    let copy = doc.clone_node(dt, false);
    check_copy(&doc, dt, copy);
    assert_eq!(doctype_fields(&doc, dt), doctype_fields(&doc, copy));
}

// dom/nodes/Node-cloneNode.html: "node with children"
#[test]
fn clone_node_with_children() {
    let mut doc = Document::new();
    let parent = elem(&mut doc, "div");
    let child1 = elem(&mut doc, "div");
    let child2 = elem(&mut doc, "div");
    let grand_child = elem(&mut doc, "div");
    append(&mut doc, child2, grand_child);
    append(&mut doc, parent, child1);
    append(&mut doc, parent, child2);

    let copy = doc.clone_node(parent, true);
    check_copy(&doc, parent, copy);
    let kids = doc.child_nodes(copy);
    assert_eq!(kids.len(), 2, "copy.childNodes.length with deep copy");
    check_copy(&doc, child1, kids[0]);
    assert_eq!(doc.child_nodes(kids[0]).len(), 0);
    check_copy(&doc, child2, kids[1]);
    let grand = doc.child_nodes(kids[1]);
    assert_eq!(grand.len(), 1);
    check_copy(&doc, grand_child, grand[0]);

    let copy = doc.clone_node(parent, false);
    check_copy(&doc, parent, copy);
    assert_eq!(
        doc.child_nodes(copy).len(),
        0,
        "copy.childNodes.length with non-deep copy"
    );
}

// --- Node-isEqualNode.html ---

// dom/nodes/Node-isEqualNode.html: "doctypes should be compared on name, public ID, and system
// ID"
#[test]
fn is_equal_node_doctypes() {
    let mut doc = Document::new();
    let d1 = doctype(&mut doc, "qualifiedName", "publicId", "systemId");
    let d2 = doctype(&mut doc, "qualifiedName", "publicId", "systemId");
    let d3 = doctype(&mut doc, "qualifiedName2", "publicId", "systemId");
    let d4 = doctype(&mut doc, "qualifiedName", "publicId2", "systemId");
    let d5 = doctype(&mut doc, "qualifiedName", "publicId", "systemId3");
    assert!(doc.is_equal_node(d1, d1), "self-comparison");
    assert!(doc.is_equal_node(d1, d2), "same properties");
    assert!(!doc.is_equal_node(d1, d3), "different name");
    assert!(!doc.is_equal_node(d1, d4), "different public ID");
    assert!(!doc.is_equal_node(d1, d5), "different system ID");
}

// dom/nodes/Node-isEqualNode.html: "elements should be compared on namespace, namespace prefix,
// local name, and number of attributes"
#[test]
fn is_equal_node_elements() {
    let mut doc = Document::new();
    let e1 = elem_ns(&mut doc, Some("namespace"), "prefix:localName");
    let e2 = elem_ns(&mut doc, Some("namespace"), "prefix:localName");
    let e3 = elem_ns(&mut doc, Some("namespace2"), "prefix:localName");
    let e4 = elem_ns(&mut doc, Some("namespace"), "prefix2:localName");
    let e5 = elem_ns(&mut doc, Some("namespace"), "prefix:localName2");
    let e6 = elem_ns(&mut doc, Some("namespace"), "prefix:localName");
    set_attr(&mut doc, e6, "foo", "bar");
    assert!(doc.is_equal_node(e1, e1), "self-comparison");
    assert!(doc.is_equal_node(e1, e2), "same properties");
    assert!(!doc.is_equal_node(e1, e3), "different namespace");
    assert!(!doc.is_equal_node(e1, e4), "different prefix");
    assert!(!doc.is_equal_node(e1, e5), "different local name");
    assert!(!doc.is_equal_node(e1, e6), "different number of attributes");
}

// dom/nodes/Node-isEqualNode.html: "elements should be compared on attribute namespace, local
// name, and value"
#[test]
fn is_equal_node_element_attributes() {
    let mut doc = Document::new();
    let mut make = |ns: &str, qname: &str, value: &str| {
        let e = elem(&mut doc, "element");
        doc.set_attribute_ns(e, Some(ns), &ds(qname), ds(value))
            .unwrap();
        e
    };
    let e1 = make("namespace", "prefix:localName", "value");
    let e2 = make("namespace", "prefix:localName", "value");
    let e3 = make("namespace2", "prefix:localName", "value");
    let e4 = make("namespace", "prefix2:localName", "value");
    let e5 = make("namespace", "prefix:localName2", "value");
    let e6 = make("namespace", "prefix:localName", "value2");
    assert!(doc.is_equal_node(e1, e1), "self-comparison");
    assert!(doc.is_equal_node(e1, e2), "attribute with same properties");
    assert!(
        !doc.is_equal_node(e1, e3),
        "attribute with different namespace"
    );
    assert!(doc.is_equal_node(e1, e4), "attribute with different prefix");
    assert!(
        !doc.is_equal_node(e1, e5),
        "attribute with different local name"
    );
    assert!(!doc.is_equal_node(e1, e6), "attribute with different value");
}

// dom/nodes/Node-isEqualNode.html: "processing instructions should be compared on target and
// data"
#[test]
fn is_equal_node_processing_instructions() {
    let mut doc = Document::new();
    let p1 = pi(&mut doc, "target", "data");
    let p2 = pi(&mut doc, "target", "data");
    let p3 = pi(&mut doc, "target2", "data");
    let p4 = pi(&mut doc, "target", "data2");
    assert!(doc.is_equal_node(p1, p1), "self-comparison");
    assert!(doc.is_equal_node(p1, p2), "same properties");
    assert!(!doc.is_equal_node(p1, p3), "different target");
    assert!(!doc.is_equal_node(p1, p4), "different data");
}

// dom/nodes/Node-isEqualNode.html: "text nodes should be compared on data"
#[test]
fn is_equal_node_text() {
    let mut doc = Document::new();
    let t1 = text(&mut doc, "data");
    let t2 = text(&mut doc, "data");
    let t3 = text(&mut doc, "data2");
    assert!(doc.is_equal_node(t1, t1), "self-comparison");
    assert!(doc.is_equal_node(t1, t2), "same properties");
    assert!(!doc.is_equal_node(t1, t3), "different data");
}

// dom/nodes/Node-isEqualNode.html: "comments should be compared on data"
#[test]
fn is_equal_node_comments() {
    let mut doc = Document::new();
    let c1 = comment(&mut doc, "data");
    let c2 = comment(&mut doc, "data");
    let c3 = comment(&mut doc, "data2");
    assert!(doc.is_equal_node(c1, c1), "self-comparison");
    assert!(doc.is_equal_node(c1, c2), "same properties");
    assert!(!doc.is_equal_node(c1, c3), "different data");
}

// dom/nodes/Node-isEqualNode.html: "document fragments should not be compared based on
// properties"
#[test]
fn is_equal_node_fragments() {
    let mut doc = Document::new();
    let f1 = doc.create_document_fragment();
    let f2 = doc.create_document_fragment();
    assert!(doc.is_equal_node(f1, f1), "self-comparison");
    assert!(doc.is_equal_node(f1, f2), "same properties");
}

// dom/nodes/Node-isEqualNode.html: "documents should not be compared based on properties"
// (los documentos son nodos Document sueltos en la arena)
#[test]
fn is_equal_node_documents() {
    let mut doc = Document::new();
    let d1 = raw_document(&mut doc);
    let d2 = raw_document(&mut doc);
    assert!(doc.is_equal_node(d1, d1), "self-comparison");
    assert!(doc.is_equal_node(d1, d2), "another empty XML document");

    // createDocument(XHTML, "html", doctype) + head + body.
    let d3 = raw_document(&mut doc);
    let dt3 = doctype(&mut doc, "html", "", "");
    append(&mut doc, d3, dt3);
    let html3 = elem_ns(&mut doc, Some(HTML_NS), "html");
    append(&mut doc, d3, html3);
    let head3 = elem(&mut doc, "head");
    append(&mut doc, html3, head3);
    let body3 = elem(&mut doc, "body");
    append(&mut doc, html3, body3);
    // createHTMLDocument() sin título.
    let d4 = raw_document(&mut doc);
    let dt4 = doctype(&mut doc, "html", "", "");
    append(&mut doc, d4, dt4);
    let html4 = elem(&mut doc, "html");
    append(&mut doc, d4, html4);
    let head4 = elem(&mut doc, "head");
    append(&mut doc, html4, head4);
    let body4 = elem(&mut doc, "body");
    append(&mut doc, html4, body4);
    assert!(
        doc.is_equal_node(d3, d4),
        "default HTML documents, created different ways"
    );
}

// dom/nodes/Node-isEqualNode.html: "node equality testing should test descendant equality too"
#[test]
fn is_equal_node_descendants() {
    let mut doc = Document::new();
    let factories: [fn(&mut Document) -> NodeId; 4] = [
        |d| elem(d, "foo"),
        |d| d.create_document_fragment(),
        raw_document,
        |d| {
            let r = raw_document(d);
            let dt = doctype(d, "html", "", "");
            append(d, r, dt);
            let html = elem(d, "html");
            append(d, r, html);
            let head = elem(d, "head");
            append(d, html, head);
            let body = elem(d, "body");
            append(d, html, body);
            r
        },
    ];
    for factory in factories {
        let parent_a = factory(&mut doc);
        let parent_b = factory(&mut doc);
        let c = comment(&mut doc, "data");
        append(&mut doc, parent_a, c);
        assert!(!doc.is_equal_node(parent_a, parent_b));
        let c = comment(&mut doc, "data");
        append(&mut doc, parent_b, c);
        assert!(doc.is_equal_node(parent_a, parent_b));
    }
}

// --- common.js (árboles para compareDocumentPosition, contains y Node-properties) ---

/// Los nodos de `setupRangeTests()` de `dom/common.js`. `foreign_doc` y `xml_doc` son nodos
/// `Document` sueltos en la misma arena.
struct Fixture {
    doc: Document,
    log: NodeId,
    test_div: NodeId,
    paras: [NodeId; 6],
    detached_div: NodeId,
    detached_para1: NodeId,
    detached_para2: NodeId,
    foreign_doc: NodeId,
    foreign_para1: NodeId,
    foreign_para2: NodeId,
    xml_doc: NodeId,
    xml_element: NodeId,
    detached_xml_element: NodeId,
    detached_text_node: NodeId,
    foreign_text_node: NodeId,
    detached_foreign_text_node: NodeId,
    xml_text_node: NodeId,
    detached_xml_text_node: NodeId,
    processing_instruction: NodeId,
    detached_processing_instruction: NodeId,
    comment: NodeId,
    detached_comment: NodeId,
    foreign_comment: NodeId,
    detached_foreign_comment: NodeId,
    xml_comment: NodeId,
    detached_xml_comment: NodeId,
    docfrag: NodeId,
    foreign_docfrag: NodeId,
    xml_docfrag: NodeId,
    doctype: NodeId,
    foreign_doctype: NodeId,
    xml_doctype: NodeId,
}

const PARA0_TEXT: &str = "A\u{308}b\u{308}c\u{308}d\u{308}e\u{308}f\u{308}g\u{308}h\u{308}\n";

fn setup_range_tests() -> Fixture {
    // El documento del test: doctype, html > (head, body > div#log).
    let mut doc = html_document(Some("Node tests"));
    let root = doc.root();
    let body = doc.body().unwrap();
    let log = elem(&mut doc, "div");
    set_attr(&mut doc, log, "id", "log");
    append(&mut doc, body, log);

    let test_div = elem(&mut doc, "div");
    doc.set_id(test_div, ds("test"));
    let first = doc.first_child(body);
    doc.insert_before_checked(body, test_div, first).unwrap();

    let para = |doc: &mut Document, id: Option<&str>, hidden: bool, content: Option<&str>| {
        let p = elem(doc, "p");
        if let Some(id) = id {
            set_attr(doc, p, "id", id);
        }
        if hidden {
            set_attr(doc, p, "style", "display:none");
        }
        if let Some(c) = content {
            doc.set_text_content(p, Some(ds(c)));
        }
        append(doc, test_div, p);
        p
    };
    let p0 = para(&mut doc, Some("a"), false, Some(PARA0_TEXT));
    let p1 = para(&mut doc, Some("b"), true, Some("Ijklmnop\n"));
    let p2 = para(&mut doc, Some("c"), false, Some("Qrstuvwx"));
    let p3 = para(&mut doc, Some("d"), true, Some("Yzabcdef"));
    let p4 = para(&mut doc, Some("e"), true, Some("Ghijklmn"));
    let p5 = para(&mut doc, None, false, None);
    let c1 = cdata_raw(&mut doc, "1234");
    append(&mut doc, p5, c1);
    let c2 = cdata_raw(&mut doc, "5678");
    append(&mut doc, p5, c2);
    doc.append_nodes(p5, vec![bf_dom::NodeOrString::String(ds("9012"))])
        .unwrap();

    let detached_div = elem(&mut doc, "div");
    let detached_para1 = elem(&mut doc, "p");
    let t = text(&mut doc, "Opqrstuv");
    append(&mut doc, detached_para1, t);
    let detached_para2 = elem(&mut doc, "p");
    let t = text(&mut doc, "Wxyzabcd");
    append(&mut doc, detached_para2, t);
    append(&mut doc, detached_div, detached_para1);
    append(&mut doc, detached_div, detached_para2);

    // foreignDoc = createHTMLDocument("").
    let foreign_doc = raw_document(&mut doc);
    let foreign_doctype = doctype(&mut doc, "html", "", "");
    append(&mut doc, foreign_doc, foreign_doctype);
    let fhtml = elem(&mut doc, "html");
    append(&mut doc, foreign_doc, fhtml);
    let fhead = elem(&mut doc, "head");
    append(&mut doc, fhtml, fhead);
    let ftitle = elem(&mut doc, "title");
    append(&mut doc, fhead, ftitle);
    let ftitle_text = text(&mut doc, "");
    append(&mut doc, ftitle, ftitle_text);
    let fbody = elem(&mut doc, "body");
    append(&mut doc, fhtml, fbody);
    let foreign_para1 = elem(&mut doc, "p");
    let t = text(&mut doc, "Efghijkl");
    append(&mut doc, foreign_para1, t);
    let foreign_para2 = elem(&mut doc, "p");
    let t = text(&mut doc, "Mnopqrst");
    append(&mut doc, foreign_para2, t);
    append(&mut doc, fbody, foreign_para1);
    append(&mut doc, fbody, foreign_para2);

    let xml_doctype = doctype(&mut doc, "qorflesnorf", "abcde", "x\"'y");
    let xml_doc = raw_document(&mut doc);
    append(&mut doc, xml_doc, xml_doctype);
    let detached_xml_element = elem_ns(&mut doc, None, "everyone-hates-hyphenated-element-names");
    let detached_text_node = text(&mut doc, "Uvwxyzab");
    let detached_foreign_text_node = text(&mut doc, "Cdefghij");
    let detached_xml_text_node = text(&mut doc, "Klmnopqr");
    let detached_processing_instruction = pi(&mut doc, "whippoorwill", "chirp chirp chirp");
    let detached_comment = comment(&mut doc, "Stuvwxyz");
    let detached_foreign_comment = comment(&mut doc, "אריה יהודה");
    let detached_xml_comment = comment(&mut doc, "בן חיים אליעזר");

    let docfrag = doc.create_document_fragment();
    let foreign_docfrag = doc.create_document_fragment();
    let xml_docfrag = doc.create_document_fragment();

    let xml_element = elem_ns(&mut doc, None, "igiveuponcreativenames");
    let xml_text_node = text(&mut doc, "do re mi fa so la ti");
    append(&mut doc, xml_element, xml_text_node);
    let processing_instruction = pi(
        &mut doc,
        "somePI",
        "Did you know that \":syn sync fromstart\" is very useful when using vim to edit large amounts of JavaScript embedded in HTML?",
    );
    append(&mut doc, xml_doc, xml_element);
    append(&mut doc, xml_doc, processing_instruction);
    let xml_comment = comment(
        &mut doc,
        "I maliciously created a comment that will break incautious XML serializers, but Firefox threw an exception, so all I got was this lousy T-shirt",
    );
    append(&mut doc, xml_doc, xml_comment);

    let comment_node = comment(&mut doc, "Alphabet soup?");
    append(&mut doc, test_div, comment_node);

    let foreign_comment = comment(
        &mut doc,
        "\"Commenter\" and \"commentator\" mean different things.  I've seen non-native speakers trip up on this.",
    );
    append(&mut doc, foreign_doc, foreign_comment);
    let foreign_text_node = text(
        &mut doc,
        "I admit that I harbor doubts about whether we really need so many things to test, but it's too late to stop now.",
    );
    append(&mut doc, fbody, foreign_text_node);

    let doctype_node = doc.doctype().unwrap();
    assert_eq!(doc.parent(doctype_node), Some(root));

    Fixture {
        doc,
        log,
        test_div,
        paras: [p0, p1, p2, p3, p4, p5],
        detached_div,
        detached_para1,
        detached_para2,
        foreign_doc,
        foreign_para1,
        foreign_para2,
        xml_doc,
        xml_element,
        detached_xml_element,
        detached_text_node,
        foreign_text_node,
        detached_foreign_text_node,
        xml_text_node,
        detached_xml_text_node,
        processing_instruction,
        detached_processing_instruction,
        comment: comment_node,
        detached_comment,
        foreign_comment,
        detached_foreign_comment,
        xml_comment,
        detached_xml_comment,
        docfrag,
        foreign_docfrag,
        xml_docfrag,
        doctype: doctype_node,
        foreign_doctype,
        xml_doctype,
    }
}

impl Fixture {
    /// `testNodes` (en el mismo orden que `common.js`).
    fn test_nodes(&self) -> Vec<(&'static str, NodeId)> {
        let d = &self.doc;
        vec![
            ("paras[0]", self.paras[0]),
            ("paras[0].firstChild", d.first_child(self.paras[0]).unwrap()),
            ("paras[1].firstChild", d.first_child(self.paras[1]).unwrap()),
            ("paras[5].firstChild", d.first_child(self.paras[5]).unwrap()),
            ("foreignPara1", self.foreign_para1),
            (
                "foreignPara1.firstChild",
                d.first_child(self.foreign_para1).unwrap(),
            ),
            ("detachedPara1", self.detached_para1),
            (
                "detachedPara1.firstChild",
                d.first_child(self.detached_para1).unwrap(),
            ),
            ("document", d.root()),
            ("detachedDiv", self.detached_div),
            ("foreignDoc", self.foreign_doc),
            ("foreignPara2", self.foreign_para2),
            ("xmlDoc", self.xml_doc),
            ("xmlElement", self.xml_element),
            ("detachedTextNode", self.detached_text_node),
            ("foreignTextNode", self.foreign_text_node),
            ("processingInstruction", self.processing_instruction),
            (
                "detachedProcessingInstruction",
                self.detached_processing_instruction,
            ),
            ("comment", self.comment),
            ("detachedComment", self.detached_comment),
            ("docfrag", self.docfrag),
            ("doctype", self.doctype),
            ("foreignDoctype", self.foreign_doctype),
            ("paras[1]", self.paras[1]),
            ("detachedPara2", self.detached_para2),
            (
                "detachedPara2.firstChild",
                d.first_child(self.detached_para2).unwrap(),
            ),
            ("testDiv", self.test_div),
            ("detachedXmlElement", self.detached_xml_element),
            ("detachedForeignTextNode", self.detached_foreign_text_node),
            ("xmlTextNode", self.xml_text_node),
            ("detachedXmlTextNode", self.detached_xml_text_node),
            ("xmlComment", self.xml_comment),
            ("foreignComment", self.foreign_comment),
            ("detachedForeignComment", self.detached_foreign_comment),
            ("detachedXmlComment", self.detached_xml_comment),
            ("foreignDocfrag", self.foreign_docfrag),
            ("xmlDocfrag", self.xml_docfrag),
            ("xmlDoctype", self.xml_doctype),
        ]
    }
}

/// `furthestAncestor` de `common.js`.
fn furthest_ancestor(doc: &Document, node: NodeId) -> NodeId {
    let mut n = node;
    while let Some(p) = doc.parent(n) {
        n = p;
    }
    n
}

/// `previousNode` de `common.js`: el nodo anterior en tree order.
fn previous_node(doc: &Document, node: NodeId) -> Option<NodeId> {
    if let Some(mut prev) = doc.previous_sibling(node) {
        while let Some(last) = doc.last_child(prev) {
            prev = last;
        }
        return Some(prev);
    }
    doc.parent(node)
}

fn is_ancestor_of(doc: &Document, ancestor: NodeId, node: NodeId) -> bool {
    let mut a = doc.parent(node);
    while let Some(x) = a {
        if x == ancestor {
            return true;
        }
        a = doc.parent(x);
    }
    false
}

// --- Node-compareDocumentPosition.html ---

// dom/nodes/Node-compareDocumentPosition.html: "<reference>.compareDocumentPosition(<other>)"
// para todos los pares de testNodes.
#[test]
fn compare_document_position_all_pairs() {
    let f = setup_range_tests();
    let doc = &f.doc;
    let nodes = f.test_nodes();
    let expected1 = DISCONNECTED + IMPLEMENTATION_SPECIFIC + PRECEDING;
    let expected2 = DISCONNECTED + IMPLEMENTATION_SPECIFIC + FOLLOWING;
    for &(ref_name, reference) in &nodes {
        for &(other_name, other) in &nodes {
            let name = format!("{ref_name}.compareDocumentPosition({other_name})");
            let result = doc.compare_document_position(reference, other);
            if other == reference {
                assert_eq!(result, 0, "{name}");
                continue;
            }
            if furthest_ancestor(doc, reference) != furthest_ancestor(doc, other) {
                assert!(
                    result == expected1 || result == expected2,
                    "{name}: {result}"
                );
                let anti = doc.compare_document_position(other, reference);
                if result == expected1 {
                    assert_eq!(anti, expected2, "{name} (anticonmutativo)");
                } else {
                    assert_eq!(anti, expected1, "{name} (anticonmutativo)");
                }
                continue;
            }
            if is_ancestor_of(doc, other, reference) {
                assert_eq!(result, CONTAINS + PRECEDING, "{name}");
                continue;
            }
            if is_ancestor_of(doc, reference, other) {
                assert_eq!(result, CONTAINED_BY + FOLLOWING, "{name}");
                continue;
            }
            let mut prev = previous_node(doc, reference);
            while let Some(p) = prev {
                if p == other {
                    break;
                }
                prev = previous_node(doc, p);
            }
            if prev == Some(other) {
                assert_eq!(result, PRECEDING, "{name}");
                continue;
            }
            assert_eq!(result, FOLLOWING, "{name}");
        }
    }
}

// --- Node-contains.html ---

// dom/nodes/Node-contains.html: "<reference>.contains(null)" para todos los testNodes.
#[test]
fn contains_null() {
    let f = setup_range_tests();
    for (name, reference) in f.test_nodes() {
        assert!(!f.doc.contains(reference, None), "{name}.contains(null)");
    }
}

// dom/nodes/Node-contains.html: "<reference>.contains(<other>)" para todos los pares.
#[test]
fn contains_all_pairs() {
    let f = setup_range_tests();
    let doc = &f.doc;
    let nodes = f.test_nodes();
    for &(ref_name, reference) in &nodes {
        for &(other_name, other) in &nodes {
            let expected = other == reference || is_ancestor_of(doc, reference, other);
            assert_eq!(
                doc.contains(reference, Some(other)),
                expected,
                "{ref_name}.contains({other_name})"
            );
        }
    }
}

// --- Node-properties.html (entradas del documento principal) ---

/// Expectativas de un elemento (las que `Node-properties.html` escribe a mano; las derivadas se
/// calculan en [`check_element`]).
struct ElementExpect<'a> {
    parent: Option<NodeId>,
    child_nodes: &'a [NodeId],
    child_nodes_len: usize,
    previous_sibling: Option<NodeId>,
    next_sibling: Option<NodeId>,
    text_content: &'a str,
    local_name: &'a str,
    tag_name: &'a str,
    id: &'a str,
    children: &'a [NodeId],
    previous_element_sibling: Option<NodeId>,
    next_element_sibling: Option<Option<NodeId>>,
    child_element_count: usize,
}

fn check_element(doc: &Document, n: NodeId, e: &ElementExpect<'_>) {
    assert_eq!(doc.node_type(n), NodeType::Element, "nodeType");
    assert_eq!(doc.owner_document(n), Some(doc.root()), "ownerDocument");
    assert_eq!(doc.parent(n), e.parent, "parentNode");
    let parent_element = e.parent.filter(|&p| doc.node_type(p) == NodeType::Element);
    assert_eq!(doc.parent_element(n), parent_element, "parentElement");
    let kids = doc.child_nodes(n);
    assert_eq!(kids.len(), e.child_nodes_len, "childNodes.length");
    for (i, &c) in e.child_nodes.iter().enumerate() {
        assert_eq!(kids[i], c, "childNodes[{i}]");
    }
    assert_eq!(
        doc.previous_sibling(n),
        e.previous_sibling,
        "previousSibling"
    );
    assert_eq!(doc.next_sibling(n), e.next_sibling, "nextSibling");
    assert_eq!(
        text_content(doc, n).as_deref(),
        Some(e.text_content),
        "textContent"
    );
    assert_eq!(
        st(doc.namespace_uri(n)).as_deref(),
        Some(HTML_NS),
        "namespaceURI"
    );
    assert_eq!(doc.prefix(n), None, "prefix");
    assert_eq!(doc.local_name(n), e.local_name, "localName");
    assert_eq!(doc.tag_name(n), e.tag_name, "tagName");
    assert_eq!(doc.id(n), e.id, "id");
    let children = doc.element_children(n);
    for (i, &c) in e.children.iter().enumerate() {
        assert_eq!(children[i], c, "children[{i}]");
    }
    assert_eq!(
        doc.previous_element_sibling(n),
        e.previous_element_sibling,
        "previousElementSibling"
    );
    if let Some(next) = e.next_element_sibling {
        assert_eq!(doc.next_element_sibling(n), next, "nextElementSibling");
    }
    assert_eq!(
        doc.child_element_count(n),
        e.child_element_count,
        "childElementCount"
    );
    // Derivadas.
    assert_eq!(doc.node_name(n), e.tag_name, "nodeName");
    assert_eq!(doc.node_value(n), None, "nodeValue");
    assert_eq!(children.len(), e.child_element_count, "children.length");
    assert_eq!(doc.class_name(n), "", "className");
    assert_eq!(
        doc.first_element_child(n),
        children.first().copied(),
        "firstElementChild"
    );
    assert_eq!(
        doc.last_element_child(n),
        children.last().copied(),
        "lastElementChild"
    );
    assert_eq!(doc.first_child(n), kids.first().copied(), "firstChild");
    assert_eq!(doc.last_child(n), kids.last().copied(), "lastChild");
    assert_eq!(
        doc.has_child_nodes(n),
        e.child_nodes_len > 0,
        "hasChildNodes()"
    );
}

// dom/nodes/Node-properties.html: "testDiv.*", "detachedDiv.*", "detachedPara1.*" y
// "detachedPara2.*"
#[test]
fn node_properties_divs() {
    let f = setup_range_tests();
    let doc = &f.doc;
    let body = doc.body().unwrap();
    let p = f.paras;
    let all_kids = [p[0], p[1], p[2], p[3], p[4], p[5], f.comment];
    check_element(
        doc,
        f.test_div,
        &ElementExpect {
            parent: Some(body),
            child_nodes: &all_kids,
            child_nodes_len: 7,
            previous_sibling: None,
            next_sibling: Some(f.log),
            text_content: "A\u{308}b\u{308}c\u{308}d\u{308}e\u{308}f\u{308}g\u{308}h\u{308}\nIjklmnop\nQrstuvwxYzabcdefGhijklmn123456789012",
            local_name: "div",
            tag_name: "DIV",
            id: "test",
            children: &p,
            previous_element_sibling: None,
            next_element_sibling: None,
            child_element_count: 6,
        },
    );
    check_element(
        doc,
        f.detached_div,
        &ElementExpect {
            parent: None,
            child_nodes: &[f.detached_para1, f.detached_para2],
            child_nodes_len: 2,
            previous_sibling: None,
            next_sibling: None,
            text_content: "OpqrstuvWxyzabcd",
            local_name: "div",
            tag_name: "DIV",
            id: "",
            children: &[f.detached_para1, f.detached_para2],
            previous_element_sibling: None,
            next_element_sibling: Some(None),
            child_element_count: 2,
        },
    );
    check_element(
        doc,
        f.detached_para1,
        &ElementExpect {
            parent: Some(f.detached_div),
            child_nodes: &[],
            child_nodes_len: 1,
            previous_sibling: None,
            next_sibling: Some(f.detached_para2),
            text_content: "Opqrstuv",
            local_name: "p",
            tag_name: "P",
            id: "",
            children: &[],
            previous_element_sibling: None,
            next_element_sibling: Some(Some(f.detached_para2)),
            child_element_count: 0,
        },
    );
    check_element(
        doc,
        f.detached_para2,
        &ElementExpect {
            parent: Some(f.detached_div),
            child_nodes: &[],
            child_nodes_len: 1,
            previous_sibling: Some(f.detached_para1),
            next_sibling: None,
            text_content: "Wxyzabcd",
            local_name: "p",
            tag_name: "P",
            id: "",
            children: &[],
            previous_element_sibling: Some(f.detached_para1),
            next_element_sibling: Some(None),
            child_element_count: 0,
        },
    );
}

// dom/nodes/Node-properties.html: "paras[0].*" ... "paras[5].*"
#[test]
fn node_properties_paras() {
    let f = setup_range_tests();
    let doc = &f.doc;
    let p = f.paras;
    let texts = [
        PARA0_TEXT,
        "Ijklmnop\n",
        "Qrstuvwx",
        "Yzabcdef",
        "Ghijklmn",
        "123456789012",
    ];
    let ids = ["a", "b", "c", "d", "e", ""];
    for i in 0..6 {
        check_element(
            doc,
            p[i],
            &ElementExpect {
                parent: Some(f.test_div),
                child_nodes: &[],
                child_nodes_len: if i == 5 { 3 } else { 1 },
                previous_sibling: if i == 0 { None } else { Some(p[i - 1]) },
                next_sibling: Some(if i == 5 { f.comment } else { p[i + 1] }),
                text_content: texts[i],
                local_name: "p",
                tag_name: "P",
                id: ids[i],
                children: &[],
                previous_element_sibling: if i == 0 { None } else { Some(p[i - 1]) },
                next_element_sibling: Some(if i == 5 { None } else { Some(p[i + 1]) }),
                child_element_count: 0,
            },
        );
    }
}

// dom/nodes/Node-properties.html: "document.*" (propiedades de árbol), "doctype.*" y "docfrag.*"
#[test]
fn node_properties_document_doctype_docfrag() {
    let f = setup_range_tests();
    let doc = &f.doc;
    let root = doc.root();
    let html = doc.document_element().unwrap();

    // document
    assert_eq!(doc.node_type(root), NodeType::Document);
    assert_eq!(doc.child_nodes(root), vec![f.doctype, html]);
    assert_eq!(doc.doctype(), Some(f.doctype));
    assert_eq!(doc.node_name(root), "#document");
    assert_eq!(doc.owner_document(root), None);
    assert_eq!(doc.parent(root), None);
    assert_eq!(doc.parent_element(root), None);
    assert_eq!(doc.previous_sibling(root), None);
    assert_eq!(doc.next_sibling(root), None);
    assert_eq!(doc.node_value(root), None);
    assert_eq!(doc.get_text_content(root), None);
    assert_eq!(doc.first_child(root), Some(f.doctype));
    assert_eq!(doc.last_child(root), Some(html));
    assert!(doc.has_child_nodes(root));
    assert_eq!(doc.content_type, "text/html");
    assert_eq!(doc.character_set, "UTF-8");
    assert_eq!(doc.compat_mode(), "CSS1Compat");

    // doctype
    let dt = f.doctype;
    assert_eq!(doc.node_type(dt), NodeType::DocumentType);
    assert_eq!(doc.owner_document(dt), Some(root));
    assert_eq!(doc.parent(dt), Some(root));
    assert_eq!(doc.previous_sibling(dt), None);
    assert_eq!(doc.next_sibling(dt), Some(html));
    assert_eq!(
        doctype_fields(doc, dt),
        ("html".into(), String::new(), String::new())
    );
    assert_eq!(doc.node_name(dt), "html");
    assert_eq!(doc.child_nodes(dt).len(), 0);
    assert_eq!(doc.parent_element(dt), None);
    assert_eq!(doc.node_value(dt), None);
    assert_eq!(doc.get_text_content(dt), None);
    assert_eq!(doc.first_child(dt), None);
    assert_eq!(doc.last_child(dt), None);
    assert!(!doc.has_child_nodes(dt));

    // docfrag
    let df = f.docfrag;
    assert_eq!(doc.node_type(df), NodeType::DocumentFragment);
    assert_eq!(doc.owner_document(df), Some(root));
    assert_eq!(doc.child_nodes(df).len(), 0);
    assert_eq!(text_content(doc, df).as_deref(), Some(""));
    assert_eq!(doc.node_name(df), "#document-fragment");
    assert_eq!(doc.parent(df), None);
    assert_eq!(doc.parent_element(df), None);
    assert_eq!(doc.previous_sibling(df), None);
    assert_eq!(doc.next_sibling(df), None);
    assert_eq!(doc.node_value(df), None);
    assert!(!doc.has_child_nodes(df));
}

/// Expectativas de un nodo `CharacterData` (texto o comentario).
fn check_character_data(
    doc: &Document,
    n: NodeId,
    node_type: NodeType,
    parent: Option<NodeId>,
    prev: Option<NodeId>,
    next: Option<NodeId>,
    value: &str,
) {
    assert_eq!(doc.node_type(n), node_type, "nodeType");
    assert_eq!(doc.owner_document(n), Some(doc.root()), "ownerDocument");
    assert_eq!(doc.parent(n), parent, "parentNode");
    let parent_element = parent.filter(|&p| doc.node_type(p) == NodeType::Element);
    assert_eq!(doc.parent_element(n), parent_element, "parentElement");
    assert_eq!(doc.previous_sibling(n), prev, "previousSibling");
    assert_eq!(doc.next_sibling(n), next, "nextSibling");
    assert_eq!(st(doc.node_value(n)).as_deref(), Some(value), "nodeValue");
    let name = if node_type == NodeType::Text {
        "#text"
    } else {
        "#comment"
    };
    assert_eq!(doc.node_name(n), name, "nodeName");
    assert_eq!(doc.child_nodes(n).len(), 0, "childNodes.length");
    assert_eq!(text_content(doc, n).as_deref(), Some(value), "textContent");
    assert_eq!(data(doc, n), value, "data");
    assert_eq!(
        doc.data_length(n),
        value.encode_utf16().count(),
        "length (code units)"
    );
    assert_eq!(doc.first_child(n), None, "firstChild");
    assert_eq!(doc.last_child(n), None, "lastChild");
    assert!(!doc.has_child_nodes(n), "hasChildNodes()");
    if node_type == NodeType::Text {
        assert_eq!(doc.whole_text(n), value, "wholeText");
    }
}

// dom/nodes/Node-properties.html: "detachedTextNode.*", "comment.*" y "detachedComment.*"
#[test]
fn node_properties_character_data() {
    let f = setup_range_tests();
    let doc = &f.doc;
    check_character_data(
        doc,
        f.detached_text_node,
        NodeType::Text,
        None,
        None,
        None,
        "Uvwxyzab",
    );
    check_character_data(
        doc,
        f.comment,
        NodeType::Comment,
        Some(f.test_div),
        Some(f.paras[5]),
        None,
        "Alphabet soup?",
    );
    check_character_data(
        doc,
        f.detached_comment,
        NodeType::Comment,
        None,
        None,
        None,
        "Stuvwxyz",
    );
}

// --- Node-normalize.html ---

// dom/nodes/Node-normalize.html: primer test (sin nombre: "Node.normalize()")
#[test]
fn normalize_fragment_and_element() {
    let mut doc = Document::new();
    let df = doc.create_document_fragment();
    let t1 = text(&mut doc, "1");
    let t2 = text(&mut doc, "2");
    let t3 = text(&mut doc, "3");
    let t4 = text(&mut doc, "4");
    append(&mut doc, df, t1);
    append(&mut doc, df, t2);
    assert_eq!(doc.child_nodes(df).len(), 2);
    assert_eq!(text_content(&doc, df).as_deref(), Some("12"));
    let el = elem(&mut doc, "x");
    append(&mut doc, df, el);
    append(&mut doc, el, t3);
    append(&mut doc, el, t4);
    let root = doc.root();
    doc.normalize(root);
    assert_eq!(doc.child_nodes(el).len(), 2);
    assert_eq!(text_content(&doc, el).as_deref(), Some("34"));
    assert_eq!(doc.child_nodes(df).len(), 3);
    assert_eq!(data(&doc, t1), "1");
    doc.normalize(df);
    assert_eq!(doc.child_nodes(df).len(), 2);
    assert_eq!(doc.first_child(df), Some(t1));
    assert_eq!(data(&doc, t1), "12");
    assert_eq!(data(&doc, t2), "2");
    assert_eq!(doc.first_child(el), Some(t3));
    assert_eq!(data(&doc, t3), "34");
    assert_eq!(data(&doc, t4), "4");
}

// dom/nodes/Node-normalize.html: "Empty text nodes separated by a non-empty text node"
#[test]
fn normalize_empty_text_around_non_empty() {
    let mut doc = Document::new();
    let div = elem(&mut doc, "div");
    let t1 = text(&mut doc, "");
    let t2 = text(&mut doc, "a");
    let t3 = text(&mut doc, "");
    for t in [t1, t2, t3] {
        append(&mut doc, div, t);
    }
    assert_eq!(doc.child_nodes(div), vec![t1, t2, t3]);
    doc.normalize(div);
    assert_eq!(doc.child_nodes(div), vec![t2]);
}

// dom/nodes/Node-normalize.html: "Empty text nodes"
#[test]
fn normalize_empty_text_nodes() {
    let mut doc = Document::new();
    let div = elem(&mut doc, "div");
    let t1 = text(&mut doc, "");
    let t2 = text(&mut doc, "");
    append(&mut doc, div, t1);
    append(&mut doc, div, t2);
    assert_eq!(doc.child_nodes(div), vec![t1, t2]);
    doc.normalize(div);
    assert_eq!(doc.child_nodes(div), Vec::<NodeId>::new());
}

// dom/nodes/Node-normalize.html: "Non-text nodes with empty textContent values."
#[test]
fn normalize_non_text_nodes_with_empty_content() {
    let mut doc = Document::new_xml();
    let root = doc.root();
    let div = elem(&mut doc, "div");
    append(&mut doc, root, div);
    let t1 = text(&mut doc, "a");
    let t2 = pi(&mut doc, "pi", "");
    let t3 = text(&mut doc, "b");
    let t4 = doc.create_cdata_section(ds("")).unwrap();
    let t5 = text(&mut doc, "c");
    let t6 = comment(&mut doc, "");
    let t7 = text(&mut doc, "d");
    let t8 = elem(&mut doc, "el");
    let t9 = text(&mut doc, "e");
    let expected = vec![t1, t2, t3, t4, t5, t6, t7, t8, t9];
    for &n in &expected {
        append(&mut doc, div, n);
    }
    assert_eq!(doc.child_nodes(div), expected);
    doc.normalize(div);
    assert_eq!(doc.child_nodes(div), expected);
}

// --- Node-textContent.html ---

// dom/nodes/Node-textContent.html: "For an empty Element, textContent should be the empty string"
// y "For an empty DocumentFragment, textContent should be the empty string"
#[test]
fn text_content_empty_element_and_fragment() {
    let mut doc = Document::new();
    let el = elem(&mut doc, "div");
    assert_eq!(text_content(&doc, el).as_deref(), Some(""));
    let df = doc.create_document_fragment();
    assert_eq!(text_content(&doc, df).as_deref(), Some(""));
}

/// Agrega `<!-- abc -->`, `"\tDEF\t"` y `<?x  ghi ?>` a `parent`.
fn add_mixed_children(doc: &mut Document, parent: NodeId) {
    let c = comment(doc, " abc ");
    append(doc, parent, c);
    let t = text(doc, "\tDEF\t");
    append(doc, parent, t);
    let p = pi(doc, "x", " ghi ");
    append(doc, parent, p);
}

// dom/nodes/Node-textContent.html: "Element with children", "Element with descendants",
// "DocumentFragment with children" y "DocumentFragment with descendants"
#[test]
fn text_content_with_children_and_descendants() {
    let mut doc = Document::new();
    let el = elem(&mut doc, "div");
    add_mixed_children(&mut doc, el);
    assert_eq!(text_content(&doc, el).as_deref(), Some("\tDEF\t"));

    let el = elem(&mut doc, "div");
    let child = elem(&mut doc, "div");
    append(&mut doc, el, child);
    add_mixed_children(&mut doc, child);
    assert_eq!(text_content(&doc, el).as_deref(), Some("\tDEF\t"));

    let df = doc.create_document_fragment();
    add_mixed_children(&mut doc, df);
    assert_eq!(text_content(&doc, df).as_deref(), Some("\tDEF\t"));

    let df = doc.create_document_fragment();
    let child = elem(&mut doc, "div");
    append(&mut doc, df, child);
    add_mixed_children(&mut doc, child);
    assert_eq!(text_content(&doc, df).as_deref(), Some("\tDEF\t"));
}

// dom/nodes/Node-textContent.html: "For an empty {Text, ProcessingInstruction, Comment},
// textContent should be the empty string" y "For a {Text, ProcessingInstruction, Comment} with
// data, textContent should be that data"
#[test]
fn text_content_character_data_getters() {
    let mut doc = Document::new();
    let t = text(&mut doc, "");
    let p = pi(&mut doc, "x", "");
    let c = comment(&mut doc, "");
    for n in [t, p, c] {
        assert_eq!(text_content(&doc, n).as_deref(), Some(""));
    }
    let t = text(&mut doc, "abc");
    let p = pi(&mut doc, "x", "abc");
    let c = comment(&mut doc, "abc");
    for n in [t, p, c] {
        assert_eq!(text_content(&doc, n).as_deref(), Some("abc"));
    }
}

/// Los tres documentos de `documents`: "parser", "createDocument" y "createHTMLDocument".
fn text_content_documents() -> Vec<(Document, &'static str)> {
    let parser = html_document(Some("Node.textContent"));
    let mut xml = Document::new_xml();
    let root = xml.root();
    let test_el = elem(&mut xml, "test");
    append(&mut xml, root, test_el);
    let created = html_document(Some("title"));
    vec![
        (parser, "parser"),
        (xml, "createDocument"),
        (created, "createHTMLDocument"),
    ]
}

// dom/nodes/Node-textContent.html: "For Documents created by {parser, createDocument,
// createHTMLDocument}, textContent should be null"
#[test]
fn text_content_documents_null() {
    for (doc, creator) in text_content_documents() {
        assert_eq!(doc.get_text_content(doc.root()), None, "{creator}");
    }
}

// dom/nodes/Node-textContent.html: "For DocumentType created by {parser, script}, textContent
// should be null"
#[test]
fn text_content_doctypes_null() {
    let mut doc = html_document(Some("title"));
    let parser = doc.doctype().unwrap();
    let script = doctype(&mut doc, "x", "", "");
    for dt in [parser, script] {
        assert_eq!(doc.get_text_content(dt), None);
    }
}

/// `testArgs`: (argumento, expectativa). `None` es `null`/`undefined`.
const TEXT_CONTENT_ARGS: [(Option<&str>, Option<&str>); 7] = [
    (None, None),             // null
    (None, None),             // undefined
    (Some(""), None),         // ""
    (Some("42"), Some("42")), // 42
    (Some("abc"), Some("abc")),
    (Some("<b>xyz</b>"), Some("<b>xyz</b>")),
    (Some("d\0e"), Some("d\0e")),
];

fn check_text_content_set(doc: &Document, n: NodeId, expectation: Option<&str>) {
    match expectation {
        None => {
            assert_eq!(text_content(doc, n).as_deref(), Some(""));
            assert_eq!(doc.first_child(n), None);
        }
        Some(exp) => {
            assert_eq!(text_content(doc, n).as_deref(), Some(exp));
            assert_eq!(doc.child_nodes(n).len(), 1, "Should have one child");
            let first = doc.first_child(n).unwrap();
            assert_eq!(
                doc.node_type(first),
                NodeType::Text,
                "child should be a Text"
            );
            assert_eq!(data(doc, first), exp);
        }
    }
}

/// Crea el contenedor (elemento o fragmento) para los tests de asignación.
fn container(doc: &mut Document, fragment: bool) -> NodeId {
    if fragment {
        doc.create_document_fragment()
    } else {
        elem(doc, "div")
    }
}

// dom/nodes/Node-textContent.html: "{Element, DocumentFragment} without children set to <arg>"
#[test]
fn text_content_set_without_children() {
    for fragment in [false, true] {
        for (arg, exp) in TEXT_CONTENT_ARGS {
            let mut doc = Document::new();
            let n = container(&mut doc, fragment);
            doc.set_text_content(n, arg.map(ds));
            check_text_content_set(&doc, n, exp);
        }
    }
}

// dom/nodes/Node-textContent.html: "{Element, DocumentFragment} with empty text node as child set
// to <arg>"
#[test]
fn text_content_set_with_empty_text_child() {
    for fragment in [false, true] {
        for (arg, exp) in TEXT_CONTENT_ARGS {
            let mut doc = Document::new();
            let n = container(&mut doc, fragment);
            let t = text(&mut doc, "");
            append(&mut doc, n, t);
            doc.set_text_content(n, arg.map(ds));
            check_text_content_set(&doc, n, exp);
            assert_eq!(
                doc.parent(t),
                None,
                "Preexisting Text should have been removed"
            );
        }
    }
}

// dom/nodes/Node-textContent.html: "{Element, DocumentFragment} with children set to <arg>"
#[test]
fn text_content_set_with_children() {
    for fragment in [false, true] {
        for (arg, exp) in TEXT_CONTENT_ARGS {
            let mut doc = Document::new();
            let n = container(&mut doc, fragment);
            add_mixed_children(&mut doc, n);
            doc.set_text_content(n, arg.map(ds));
            check_text_content_set(&doc, n, exp);
        }
    }
}

// dom/nodes/Node-textContent.html: "{Element, DocumentFragment} with descendants set to <arg>"
#[test]
fn text_content_set_with_descendants() {
    for fragment in [false, true] {
        for (arg, exp) in TEXT_CONTENT_ARGS {
            let mut doc = Document::new();
            let n = container(&mut doc, fragment);
            let child = elem(&mut doc, "div");
            append(&mut doc, n, child);
            add_mixed_children(&mut doc, child);
            doc.set_text_content(n, arg.map(ds));
            check_text_content_set(&doc, n, exp);
            assert_eq!(
                doc.child_nodes(child).len(),
                3,
                "Should not have changed the internal structure of the removed nodes."
            );
        }
    }
}

// dom/nodes/Node-textContent.html: "For a Text, textContent should set the data", "For a
// ProcessingInstruction, textContent should set the data" y "For a Comment, textContent should
// set the data"
#[test]
fn text_content_set_character_data() {
    let mut doc = Document::new();
    let t = text(&mut doc, "abc");
    doc.set_text_content(t, Some(ds("def")));
    assert_eq!(text_content(&doc, t).as_deref(), Some("def"));
    assert_eq!(data(&doc, t), "def");

    let p = pi(&mut doc, "x", "abc");
    doc.set_text_content(p, Some(ds("def")));
    assert_eq!(text_content(&doc, p).as_deref(), Some("def"));
    assert_eq!(data(&doc, p), "def");
    assert_eq!(pi_target(&doc, p), "x");

    let c = comment(&mut doc, "abc");
    doc.set_text_content(c, Some(ds("def")));
    assert_eq!(text_content(&doc, c).as_deref(), Some("def"));
    assert_eq!(data(&doc, c), "def");
}

// dom/nodes/Node-textContent.html: "For Documents created by {parser, createDocument,
// createHTMLDocument}, setting textContent should do nothing"
#[test]
fn text_content_set_on_documents_does_nothing() {
    for (mut doc, creator) in text_content_documents() {
        let root = doc.root();
        let element = doc.document_element();
        doc.set_text_content(root, Some(ds("a")));
        assert_eq!(doc.get_text_content(root), None, "{creator}");
        assert_eq!(doc.document_element(), element, "{creator}");
    }
}

// dom/nodes/Node-textContent.html: "For DocumentType created by {parser, script}, setting
// textContent should do nothing"
#[test]
fn text_content_set_on_doctypes_does_nothing() {
    let mut doc = html_document(Some("title"));
    let parser = doc.doctype().unwrap();
    let script = doctype(&mut doc, "x", "", "");
    for dt in [parser, script] {
        let props = doctype_fields(&doc, dt);
        doc.set_text_content(dt, Some(ds("b")));
        assert_eq!(doc.get_text_content(dt), None);
        assert_eq!(doctype_fields(&doc, dt), props);
    }
}

// --- Node-nodeName.html ---

// dom/nodes/Node-nodeName.html: "For Element nodes, nodeName should return the same as tagName."
#[test]
fn node_name_elements() {
    let mut doc = Document::new();
    let cases = [
        (HTML_NS, "I", "I"),
        (HTML_NS, "i", "I"),
        (SVG_NS, "svg", "svg"),
        (SVG_NS, "SVG", "SVG"),
        (HTML_NS, "x:b", "X:B"),
    ];
    for (ns, qname, expected) in cases {
        let el = elem_ns(&mut doc, Some(ns), qname);
        assert_eq!(doc.node_name(el), expected, "{qname}");
        assert_eq!(doc.node_name(el), doc.tag_name(el));
    }
}

// dom/nodes/Node-nodeName.html: "For Text nodes, nodeName should return \"#text\"."
#[test]
fn node_name_text() {
    let mut doc = Document::new();
    let t = text(&mut doc, "foo");
    assert_eq!(doc.node_name(t), "#text");
}

// dom/nodes/Node-nodeName.html: "For Comment nodes, nodeName should return \"#comment\"."
#[test]
fn node_name_comment() {
    let mut doc = Document::new();
    let c = comment(&mut doc, "foo");
    assert_eq!(doc.node_name(c), "#comment");
}

// dom/nodes/Node-nodeName.html: "For Document nodes, nodeName should return \"#document\"."
#[test]
fn node_name_document() {
    let doc = Document::new();
    assert_eq!(doc.node_name(doc.root()), "#document");
}

// dom/nodes/Node-nodeName.html: "For DocumentType nodes, nodeName should return the name."
#[test]
fn node_name_doctype() {
    let doc = html_document(Some("title"));
    assert_eq!(doc.node_name(doc.doctype().unwrap()), "html");
}

// dom/nodes/Node-nodeName.html: "For DocumentFragment nodes, nodeName should return
// \"#document-fragment\"."
#[test]
fn node_name_fragment() {
    let mut doc = Document::new();
    let df = doc.create_document_fragment();
    assert_eq!(doc.node_name(df), "#document-fragment");
}

// --- Node-parentElement.html ---

// dom/nodes/Node-parentElement.html: "When the parent is null, parentElement should be null"
#[test]
fn parent_element_null_parent() {
    let doc = html_document(Some("title"));
    assert_eq!(doc.parent_element(doc.root()), None);
}

// dom/nodes/Node-parentElement.html: "When the parent is a document, parentElement should be null
// (doctype)", "(element)" y "(comment)"
#[test]
fn parent_element_document_parent() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    assert_eq!(doc.parent_element(doc.doctype().unwrap()), None);
    assert_eq!(doc.parent_element(doc.document_element().unwrap()), None);
    let c = comment(&mut doc, "foo");
    let c = append(&mut doc, root, c);
    assert_eq!(doc.parent_element(c), None);
}

// dom/nodes/Node-parentElement.html: "parentElement should return null for children of
// DocumentFragments (element)" y "(text)"
#[test]
fn parent_element_fragment_children() {
    let mut doc = Document::new();
    let df = doc.create_document_fragment();
    assert_eq!(doc.parent_element(df), None);
    let el = elem(&mut doc, "div");
    assert_eq!(doc.parent_element(el), None);
    append(&mut doc, df, el);
    assert_eq!(doc.parent(el), Some(df));
    assert_eq!(doc.parent_element(el), None);

    let df = doc.create_document_fragment();
    let t = text(&mut doc, "bar");
    assert_eq!(doc.parent_element(t), None);
    append(&mut doc, df, t);
    assert_eq!(doc.parent(t), Some(df));
    assert_eq!(doc.parent_element(t), None);
}

// dom/nodes/Node-parentElement.html: "parentElement should work correctly with DocumentFragments
// (element)" y "(text)"
#[test]
fn parent_element_inside_fragment() {
    let mut doc = Document::new();
    for make_text in [false, true] {
        let df = doc.create_document_fragment();
        let parent = elem(&mut doc, "div");
        append(&mut doc, df, parent);
        let n = if make_text {
            text(&mut doc, "bar")
        } else {
            elem(&mut doc, "div")
        };
        assert_eq!(doc.parent_element(n), None);
        append(&mut doc, parent, n);
        assert_eq!(doc.parent_element(n), Some(parent));
    }
}

// dom/nodes/Node-parentElement.html: "parentElement should work correctly in disconnected
// subtrees (element)" y "(text)"
#[test]
fn parent_element_disconnected_subtrees() {
    let mut doc = Document::new();
    for make_text in [false, true] {
        let parent = elem(&mut doc, "div");
        let n = if make_text {
            text(&mut doc, "bar")
        } else {
            elem(&mut doc, "div")
        };
        assert_eq!(doc.parent_element(n), None);
        append(&mut doc, parent, n);
        assert_eq!(doc.parent_element(n), Some(parent));
    }
}

// dom/nodes/Node-parentElement.html: "parentElement should work correctly in a document
// (element)" y "(text)" (el original usa un div en ambos)
#[test]
fn parent_element_in_document() {
    let mut doc = html_document(Some("title"));
    let body = doc.body().unwrap();
    for _ in 0..2 {
        let el = elem(&mut doc, "div");
        assert_eq!(doc.parent_element(el), None);
        append(&mut doc, body, el);
        assert_eq!(doc.parent_element(el), Some(body));
    }
}

// --- Node-lookupNamespaceURI.html ---

fn lookup(doc: &Document, n: NodeId, prefix: Option<&str>) -> Option<String> {
    st(doc.lookup_namespace_uri(n, prefix))
}

// dom/nodes/Node-lookupNamespaceURI.html: "DocumentFragment should have null namespace, prefix
// {null, "", "foo", "xml", "xmlns"}" y "DocumentFragment is in default namespace, prefix ..."
#[test]
fn lookup_namespace_uri_fragment() {
    let mut doc = html_document(Some("title"));
    let frag = doc.create_document_fragment();
    for prefix in [None, Some(""), Some("foo"), Some("xml"), Some("xmlns")] {
        assert_eq!(lookup(&doc, frag, prefix), None, "{prefix:?}");
    }
    assert!(doc.is_default_namespace(frag, None));
    assert!(doc.is_default_namespace(frag, Some("")));
    assert!(!doc.is_default_namespace(frag, Some("foo")));
    assert!(!doc.is_default_namespace(frag, Some("xmlns")));
}

// dom/nodes/Node-lookupNamespaceURI.html: "DocumentType should have null namespace, prefix ..." y
// "DocumentType is in default namespace, prefix ..."
#[test]
fn lookup_namespace_uri_doctype() {
    let doc = html_document(Some("title"));
    let dt = doc.doctype().unwrap();
    for prefix in [None, Some(""), Some("foo"), Some("xml"), Some("xmlns")] {
        assert_eq!(lookup(&doc, dt, prefix), None, "{prefix:?}");
    }
    assert!(doc.is_default_namespace(dt, None));
    assert!(doc.is_default_namespace(dt, Some("")));
    assert!(!doc.is_default_namespace(dt, Some("foo")));
    assert!(!doc.is_default_namespace(dt, Some("xmlns")));
}

/// `fooElem` con el atributo `bar` (y, si `with_xmlns`, `xmlns:bar` y `xmlns`).
fn foo_elem(doc: &mut Document, with_xmlns: bool) -> NodeId {
    let foo_el = elem_ns(doc, Some("fooNamespace"), "prefix:elem");
    set_attr(doc, foo_el, "bar", "value");
    if with_xmlns {
        doc.set_attribute_ns(foo_el, Some(XMLNS_NS), &ds("xmlns:bar"), ds("barURI"))
            .unwrap();
        doc.set_attribute_ns(foo_el, Some(XMLNS_NS), &ds("xmlns"), ds("bazURI"))
            .unwrap();
    }
    foo_el
}

// dom/nodes/Node-lookupNamespaceURI.html: "Element should have null namespace, prefix null" ...
// "xmlns namespace is not default" (fooElem sin declaraciones xmlns)
#[test]
fn lookup_namespace_uri_element() {
    let mut doc = html_document(Some("title"));
    let foo_el = foo_elem(&mut doc, false);
    assert_eq!(lookup(&doc, foo_el, None), None);
    assert_eq!(lookup(&doc, foo_el, Some("")), None);
    assert_eq!(lookup(&doc, foo_el, Some("fooNamespace")), None);
    assert_eq!(lookup(&doc, foo_el, Some("xml")).as_deref(), Some(XML_NS));
    assert_eq!(
        lookup(&doc, foo_el, Some("xmlns")).as_deref(),
        Some(XMLNS_NS)
    );
    assert_eq!(
        lookup(&doc, foo_el, Some("prefix")).as_deref(),
        Some("fooNamespace")
    );
    assert!(doc.is_default_namespace(foo_el, None));
    assert!(doc.is_default_namespace(foo_el, Some("")));
    assert!(!doc.is_default_namespace(foo_el, Some("fooNamespace")));
    assert!(!doc.is_default_namespace(foo_el, Some(XMLNS_NS)));
}

// dom/nodes/Node-lookupNamespaceURI.html: "Element should have baz namespace, prefix null" ...
// "baz namespace is default" (fooElem con xmlns:bar y xmlns)
#[test]
fn lookup_namespace_uri_element_with_xmlns() {
    let mut doc = html_document(Some("title"));
    let foo_el = foo_elem(&mut doc, true);
    assert_eq!(lookup(&doc, foo_el, None).as_deref(), Some("bazURI"));
    assert_eq!(lookup(&doc, foo_el, Some("")).as_deref(), Some("bazURI"));
    assert_eq!(
        lookup(&doc, foo_el, Some("xmlns")).as_deref(),
        Some(XMLNS_NS)
    );
    assert_eq!(lookup(&doc, foo_el, Some("bar")).as_deref(), Some("barURI"));
    assert!(!doc.is_default_namespace(foo_el, None));
    assert!(!doc.is_default_namespace(foo_el, Some("")));
    assert!(!doc.is_default_namespace(foo_el, Some("barURI")));
    assert!(doc.is_default_namespace(foo_el, Some("bazURI")));
}

// dom/nodes/Node-lookupNamespaceURI.html: "Comment should inherit baz namespace" ... "For
// comment, inherited baz namespace is default"
#[test]
fn lookup_namespace_uri_comment_child() {
    let mut doc = html_document(Some("title"));
    let foo_el = foo_elem(&mut doc, true);
    let c = comment(&mut doc, "comment");
    append(&mut doc, foo_el, c);
    assert_eq!(lookup(&doc, c, None).as_deref(), Some("bazURI"));
    assert_eq!(lookup(&doc, c, Some("")).as_deref(), Some("bazURI"));
    assert_eq!(
        lookup(&doc, c, Some("prefix")).as_deref(),
        Some("fooNamespace")
    );
    assert_eq!(lookup(&doc, c, Some("bar")).as_deref(), Some("barURI"));
    assert!(!doc.is_default_namespace(c, None));
    assert!(!doc.is_default_namespace(c, Some("")));
    assert!(!doc.is_default_namespace(c, Some("fooNamespace")));
    assert!(!doc.is_default_namespace(c, Some(XMLNS_NS)));
    assert!(!doc.is_default_namespace(c, Some("barURI")));
    assert!(doc.is_default_namespace(c, Some("bazURI")));
}

// dom/nodes/Node-lookupNamespaceURI.html: "Child element should inherit baz namespace" ...
// "childNamespace is default for child"
#[test]
fn lookup_namespace_uri_child_element() {
    let mut doc = html_document(Some("title"));
    let foo_el = foo_elem(&mut doc, true);
    let c = comment(&mut doc, "comment");
    append(&mut doc, foo_el, c);
    let child = elem_ns(&mut doc, Some("childNamespace"), "childElem");
    append(&mut doc, foo_el, child);
    assert_eq!(lookup(&doc, child, None).as_deref(), Some("childNamespace"));
    assert_eq!(
        lookup(&doc, child, Some("")).as_deref(),
        Some("childNamespace")
    );
    assert_eq!(
        lookup(&doc, child, Some("xmlns")).as_deref(),
        Some(XMLNS_NS)
    );
    assert_eq!(
        lookup(&doc, child, Some("prefix")).as_deref(),
        Some("fooNamespace")
    );
    assert!(!doc.is_default_namespace(child, None));
    assert!(!doc.is_default_namespace(child, Some("")));
    assert!(!doc.is_default_namespace(child, Some("fooNamespace")));
    assert!(!doc.is_default_namespace(child, Some(XMLNS_NS)));
    assert!(!doc.is_default_namespace(child, Some("barURI")));
    assert!(!doc.is_default_namespace(child, Some("bazURI")));
    assert!(doc.is_default_namespace(child, Some("childNamespace")));
}

// dom/nodes/Node-lookupNamespaceURI.html: "Document should have xhtml namespace, prefix null" ...
// "For document, xhtml namespace is default"
#[test]
fn lookup_namespace_uri_document() {
    let mut doc = html_document(Some("title"));
    let root = doc.root();
    let html = doc.document_element().unwrap();
    doc.set_attribute_ns(html, Some(XMLNS_NS), &ds("xmlns:bar"), ds("barURI"))
        .unwrap();
    doc.set_attribute_ns(html, Some(XMLNS_NS), &ds("xmlns"), ds("bazURI"))
        .unwrap();
    assert_eq!(lookup(&doc, root, None).as_deref(), Some(HTML_NS));
    assert_eq!(lookup(&doc, root, Some("")).as_deref(), Some(HTML_NS));
    assert_eq!(lookup(&doc, root, Some("prefix")), None);
    assert_eq!(lookup(&doc, root, Some("bar")).as_deref(), Some("barURI"));
    assert!(!doc.is_default_namespace(root, None));
    assert!(!doc.is_default_namespace(root, Some("")));
    assert!(!doc.is_default_namespace(root, Some("fooNamespace")));
    assert!(!doc.is_default_namespace(root, Some(XMLNS_NS)));
    assert!(!doc.is_default_namespace(root, Some("barURI")));
    assert!(!doc.is_default_namespace(root, Some("bazURI")));
    assert!(doc.is_default_namespace(root, Some(HTML_NS)));

    // "Comment does not have bar namespace": un comentario hijo del documento.
    let c = comment(&mut doc, "comment");
    append(&mut doc, root, c);
    assert_eq!(lookup(&doc, c, Some("bar")), None);
}

// dom/nodes/Node-lookupNamespaceURI.html: "Document without documentElement has no namespace URI
// matching \"xml\"" y "... \"xmlns\""
#[test]
fn lookup_namespace_uri_empty_document() {
    let doc = Document::new_xml();
    assert_eq!(lookup(&doc, doc.root(), Some("xml")), None);
    assert_eq!(lookup(&doc, doc.root(), Some("xmlns")), None);
}

// dom/nodes/Node-lookupNamespaceURI.html: "Disconnected Attr has no namespace URI matching ..." y
// "Connected Attr has namespace URI matching ..."
#[test]
fn lookup_namespace_uri_attr() {
    let mut doc = html_document(Some("title"));
    let attr = doc.create_attribute(&ds("foo")).unwrap();
    assert_eq!(lookup(&doc, attr, Some("xml")), None);
    assert_eq!(lookup(&doc, attr, Some("xmlns")), None);
    let body = doc.body().unwrap();
    doc.set_attribute_node(body, attr).unwrap();
    assert_eq!(lookup(&doc, attr, Some("xml")).as_deref(), Some(XML_NS));
    assert_eq!(lookup(&doc, attr, Some("xmlns")).as_deref(), Some(XMLNS_NS));
}

// --- Node-lookupPrefix.xhtml ---

// dom/nodes/Node-lookupPrefix.xhtml: los once `lookupPrefix(node, ns, prefix)` (subtests sin
// nombre)
#[test]
fn lookup_prefix_xhtml() {
    let mut doc = Document::new_xml();
    doc.content_type = ds("application/xhtml+xml");
    let root = doc.root();
    let xmlns = |doc: &mut Document, el: NodeId, qname: &str, value: &str| {
        doc.set_attribute_ns(el, Some(XMLNS_NS), &ds(qname), ds(value))
            .unwrap();
    };
    // <html xmlns="http://www.w3.org/1999/xhtml" xmlns:x="test">
    let html = elem_ns(&mut doc, Some(HTML_NS), "html");
    xmlns(&mut doc, html, "xmlns", HTML_NS);
    xmlns(&mut doc, html, "xmlns:x", "test");
    append(&mut doc, root, html);
    let head = elem_ns(&mut doc, Some(HTML_NS), "head");
    append(&mut doc, html, head);
    // <body xmlns:s="test">
    let body = elem_ns(&mut doc, Some(HTML_NS), "body");
    xmlns(&mut doc, body, "xmlns:s", "test");
    append(&mut doc, html, body);
    let log = elem_ns(&mut doc, Some(HTML_NS), "div");
    append(&mut doc, body, log);
    // <x xmlns:t="test"><!--comment--><?test test?>TEST<x/></x>
    let x = elem_ns(&mut doc, Some(HTML_NS), "x");
    xmlns(&mut doc, x, "xmlns:t", "test");
    append(&mut doc, body, x);
    let c = comment(&mut doc, "comment");
    append(&mut doc, x, c);
    let p = pi(&mut doc, "test", "test");
    append(&mut doc, x, p);
    let t = text(&mut doc, "TEST");
    append(&mut doc, x, t);
    let inner = elem_ns(&mut doc, Some(HTML_NS), "x");
    append(&mut doc, x, inner);

    let lp = |n: NodeId, ns: Option<&str>| st(doc.lookup_prefix(n, ns));
    assert_eq!(lp(root, Some("test")).as_deref(), Some("x"));
    assert_eq!(lp(root, None), None);
    assert_eq!(lp(x, Some(HTML_NS)), None);
    assert_eq!(lp(x, Some("something")), None);
    assert_eq!(lp(x, None), None);
    assert_eq!(lp(x, Some("test")).as_deref(), Some("t"));
    assert_eq!(
        lp(doc.parent(x).unwrap(), Some("test")).as_deref(),
        Some("s")
    );
    assert_eq!(
        lp(doc.first_child(x).unwrap(), Some("test")).as_deref(),
        Some("t")
    );
    assert_eq!(
        lp(doc.child_nodes(x)[1], Some("test")).as_deref(),
        Some("t")
    );
    assert_eq!(
        lp(doc.child_nodes(x)[2], Some("test")).as_deref(),
        Some("t")
    );
    assert_eq!(
        lp(doc.last_child(x).unwrap(), Some("test")).as_deref(),
        Some("t")
    );
}

// --- Document-createElement.html ---

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DocKind {
    Html,
    Xml,
    Xhtml,
}

const DOC_KINDS: [DocKind; 3] = [DocKind::Html, DocKind::Xml, DocKind::Xhtml];

/// El documento de cada variante ("HTML document", "XML document", "XHTML document").
fn make_doc(kind: DocKind) -> Document {
    match kind {
        DocKind::Html => Document::new(),
        DocKind::Xml => Document::new_xml(),
        DocKind::Xhtml => {
            let mut d = Document::new_xml();
            d.content_type = ds("application/xhtml+xml");
            d
        }
    }
}

/// `valid` de `Document-createElement.html` (`undefined` y `null` ya convertidos a string).
const CREATE_ELEMENT_VALID: &[&str] = &[
    "undefined",
    "null",
    "foo",
    "f1oo",
    "foo1",
    "f\u{0BC6}",
    "foo\u{0BC6}",
    ":",
    ":foo",
    "f:oo",
    "foo:",
    "f:o:o",
    "f::oo",
    "f::oo:",
    "foo:0",
    "foo:_",
    // Combining char tras `:`: QName inválido pero Name válido.
    "foo:\u{0BC6}",
    "foo:foo\u{0BC6}",
    "foo\u{0BC6}:foo",
    "xml",
    "xmlns",
    "xmlfoo",
    "xml:foo",
    "xmlns:foo",
    "xmlfoo:bar",
    "svg",
    "math",
    "FOO",
    // Los caracteres no ASCII no cambian de caja.
    "mar\u{212a}",
    "\u{0130}nput",
    "\u{0131}nput",
    "\u{0300}foo",
    "f}oo",
    "foo}",
    "\u{ffff}foo",
    "f\u{ffff}oo",
    "foo\u{ffff}",
    "f<oo",
    "\u{0300}",
];

/// `invalid` de `Document-createElement.html`.
const CREATE_ELEMENT_INVALID: &[&str] = &[
    "", "1foo", "1:foo", "fo o", "}foo", "<foo", "foo>", "<foo>", "-foo", ".foo",
];

// dom/nodes/Document-createElement.html: "createElement(<valid>) in {HTML, XML, XHTML} document"
#[test]
fn create_element_valid_names() {
    for kind in DOC_KINDS {
        let mut doc = make_doc(kind);
        for &t in CREATE_ELEMENT_VALID {
            let name = format!("createElement({t:?}) in {kind:?} document");
            let el = doc
                .create_element(&ds(t))
                .unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(doc.node_type(el), NodeType::Element, "{name}");
            let (local, tag) = if kind == DocKind::Html {
                (t.to_ascii_lowercase(), t.to_ascii_uppercase())
            } else {
                (t.to_string(), t.to_string())
            };
            assert_eq!(doc.local_name(el).to_string(), local, "{name}: localName");
            assert_eq!(doc.tag_name(el).to_string(), tag, "{name}: tagName");
            assert_eq!(doc.prefix(el), None, "{name}: prefix");
            let ns = if kind == DocKind::Xml {
                None
            } else {
                Some(HTML_NS)
            };
            assert_eq!(
                st(doc.namespace_uri(el)).as_deref(),
                ns,
                "{name}: namespaceURI"
            );
        }
    }
}

// dom/nodes/Document-createElement.html: "createElement(<invalid>) in {HTML, XML, XHTML}
// document"
#[test]
fn create_element_invalid_names() {
    for kind in DOC_KINDS {
        let mut doc = make_doc(kind);
        for &arg in CREATE_ELEMENT_INVALID {
            assert_eq!(
                doc.create_element(&ds(arg)),
                Err(DomException::InvalidCharacterError),
                "createElement({arg:?}) in {kind:?} document"
            );
        }
    }
}

// --- Document-createElementNS.html + Document-createElementNS.js ---

/// Un argumento de JS: `null`, `undefined` o un string.
#[derive(Clone, Copy, Debug)]
enum Arg {
    Null,
    Undefined,
    S(&'static str),
}

/// La excepción esperada (`null` = ninguna).
#[derive(Clone, Copy, Debug)]
enum Exp {
    Ok,
    InvalidChar,
    Namespace,
}

use Arg::{Null, S, Undefined};
use Exp::{InvalidChar as ICE, Namespace as NSE, Ok as OK};

/// `createElementNS_tests` de `Document-createElementNS.js` seguido de los casos extra de
/// `Document-createElementNS.html`, sin cambios: [namespace, qualifiedName, excepción].
const CREATE_ELEMENT_NS_TESTS: &[(Arg, Arg, Exp)] = &[
    (Null, Null, OK),
    (Null, Undefined, OK),
    (Null, S("foo"), OK),
    (Null, S("1foo"), ICE),
    (Null, S("f1oo"), OK),
    (Null, S("foo1"), OK),
    (Null, S("\u{0BC6}foo"), OK),
    (Null, S("\u{037E}foo"), OK),
    (Null, S("}foo"), ICE),
    (Null, S("f}oo"), OK),
    (Null, S("foo}"), OK),
    (Null, S("\u{FFFF}foo"), OK),
    (Null, S("f\u{FFFF}oo"), OK),
    (Null, S("foo\u{FFFF}"), OK),
    (Null, S("<foo"), ICE),
    (Null, S("foo>"), ICE),
    (Null, S("<foo>"), ICE),
    (Null, S("f<oo"), OK),
    (Null, S("^^"), ICE),
    (Null, S("fo o"), ICE),
    (Null, S("-foo"), ICE),
    (Null, S(".foo"), ICE),
    (Null, S(":foo"), ICE),
    (Null, S("f:oo"), NSE),
    (Null, S("foo:"), ICE),
    (Null, S("f:o:o"), NSE),
    (Null, S(":"), ICE),
    (Null, S("xml"), OK),
    (Null, S("xmlns"), NSE),
    (Null, S("xmlfoo"), OK),
    (Null, S("xml:foo"), NSE),
    (Null, S("xmlns:foo"), NSE),
    (Null, S("xmlfoo:bar"), NSE),
    (Null, S("null:xml"), NSE),
    (S(""), Null, OK),
    (S(""), S(":foo"), ICE),
    (S(""), S("f:oo"), NSE),
    (S(""), S("foo:"), ICE),
    (Undefined, Null, OK),
    (Undefined, Undefined, OK),
    (Undefined, S("foo"), OK),
    (Undefined, S("1foo"), ICE),
    (Undefined, S("f1oo"), OK),
    (Undefined, S("foo1"), OK),
    (Undefined, S(":foo"), ICE),
    (Undefined, S("f:oo"), NSE),
    (Undefined, S("foo:"), ICE),
    (Undefined, S("f::oo"), NSE),
    (Undefined, S("xml"), OK),
    (Undefined, S("xmlns"), NSE),
    (Undefined, S("xmlfoo"), OK),
    (Undefined, S("xml:foo"), NSE),
    (Undefined, S("xmlns:foo"), NSE),
    (Undefined, S("xmlfoo:bar"), NSE),
    (S("http://example.com/"), S("foo"), OK),
    (S("http://example.com/"), S("1foo"), ICE),
    (S("http://example.com/"), S("<foo>"), ICE),
    (S("http://example.com/"), S("fo<o"), OK),
    (S("http://example.com/"), S("-foo"), ICE),
    (S("http://example.com/"), S(".foo"), ICE),
    (S("http://example.com/"), S("f1oo"), OK),
    (S("http://example.com/"), S("foo1"), OK),
    (S("http://example.com/"), S(":foo"), ICE),
    (S("http://example.com/"), S("f:oo"), OK),
    (S("http://example.com/"), S("f:o:o"), OK),
    (S("http://example.com/"), S("foo:"), ICE),
    (S("http://example.com/"), S("f::oo"), OK),
    (S("http://example.com/"), S("a:0"), ICE),
    (S("http://example.com/"), S("0:a"), OK),
    (S("http://example.com/"), S("a:_"), OK),
    (S("http://example.com/"), S("a:\u{0BC6}"), OK),
    (S("http://example.com/"), S("a:\u{037E}"), OK),
    (S("http://example.com/"), S("a:\u{0300}"), OK),
    (S("http://example.com/"), S("\u{0BC6}:a"), OK),
    (S("http://example.com/"), S("\u{0300}:a"), OK),
    (S("http://example.com/"), S("\u{037E}:a"), OK),
    (S("http://example.com/"), S("a:a\u{0BC6}"), OK),
    (S("http://example.com/"), S("a\u{0BC6}:a"), OK),
    (S("http://example.com/"), S("xml:test"), NSE),
    (S("http://example.com/"), S("xmlns:test"), NSE),
    (S("http://example.com/"), S("test:xmlns"), OK),
    (S("http://example.com/"), S("xmlns"), NSE),
    (S("http://example.com/"), S("_:_"), OK),
    (S("http://example.com/"), S("_:h0"), OK),
    (S("http://example.com/"), S("_:test"), OK),
    (S("http://example.com/"), S("l_:_"), OK),
    (S("http://example.com/"), S("ns:_0"), OK),
    (S("http://example.com/"), S("ns:a0"), OK),
    (S("http://example.com/"), S("ns0:test"), OK),
    (S("http://example.com/"), S("a.b:c"), OK),
    (S("http://example.com/"), S("a-b:c"), OK),
    (S("http://example.com/"), S("xml"), OK),
    (S("http://example.com/"), S("XMLNS"), OK),
    (S("http://example.com/"), S("xmlfoo"), OK),
    (S("http://example.com/"), S("xml:foo"), NSE),
    (S("http://example.com/"), S("XML:foo"), OK),
    (S("http://example.com/"), S("xmlns:foo"), NSE),
    (S("http://example.com/"), S("XMLNS:foo"), OK),
    (S("http://example.com/"), S("xmlfoo:bar"), OK),
    (S("http://example.com/"), S("prefix::local"), OK),
    (S("http://example.com/"), S("namespaceURI:{"), ICE),
    (S("http://example.com/"), S("namespaceURI:}"), ICE),
    (S("http://example.com/"), S("namespaceURI:~"), ICE),
    (S("http://example.com/"), S("namespaceURI:'"), ICE),
    (S("http://example.com/"), S("namespaceURI:!"), ICE),
    (S("http://example.com/"), S("namespaceURI:@"), ICE),
    (S("http://example.com/"), S("namespaceURI:#"), ICE),
    (S("http://example.com/"), S("namespaceURI:$"), ICE),
    (S("http://example.com/"), S("namespaceURI:%"), ICE),
    (S("http://example.com/"), S("namespaceURI:^"), ICE),
    (S("http://example.com/"), S("namespaceURI:&"), ICE),
    (S("http://example.com/"), S("namespaceURI:*"), ICE),
    (S("http://example.com/"), S("namespaceURI:("), ICE),
    (S("http://example.com/"), S("namespaceURI:)"), ICE),
    (S("http://example.com/"), S("namespaceURI:+"), ICE),
    (S("http://example.com/"), S("namespaceURI:="), ICE),
    (S("http://example.com/"), S("namespaceURI:["), ICE),
    (S("http://example.com/"), S("namespaceURI:]"), ICE),
    (S("http://example.com/"), S("namespaceURI:\\"), ICE),
    (S("http://example.com/"), S("namespaceURI:/"), ICE),
    (S("http://example.com/"), S("namespaceURI:;"), ICE),
    (S("http://example.com/"), S("namespaceURI:`"), ICE),
    (S("http://example.com/"), S("namespaceURI:<"), ICE),
    (S("http://example.com/"), S("namespaceURI:>"), ICE),
    (S("http://example.com/"), S("namespaceURI:,"), ICE),
    (S("http://example.com/"), S("namespaceURI:a "), ICE),
    (S("http://example.com/"), S("namespaceURI:\""), ICE),
    (S("/"), S("foo"), OK),
    (S("/"), S("1foo"), ICE),
    (S("/"), S("f1oo"), OK),
    (S("/"), S("foo1"), OK),
    (S("/"), S(":foo"), ICE),
    (S("/"), S("f:oo"), OK),
    (S("/"), S("foo:"), ICE),
    (S("/"), S("xml"), OK),
    (S("/"), S("xmlns"), NSE),
    (S("/"), S("xmlfoo"), OK),
    (S("/"), S("xml:foo"), NSE),
    (S("/"), S("xmlns:foo"), NSE),
    (S("/"), S("xmlfoo:bar"), OK),
    (S("http://www.w3.org/XML/1998/namespace"), S("foo"), OK),
    (S("http://www.w3.org/XML/1998/namespace"), S("1foo"), ICE),
    (S("http://www.w3.org/XML/1998/namespace"), S("f1oo"), OK),
    (S("http://www.w3.org/XML/1998/namespace"), S("foo1"), OK),
    (S("http://www.w3.org/XML/1998/namespace"), S(":foo"), ICE),
    (S("http://www.w3.org/XML/1998/namespace"), S("f:oo"), OK),
    (S("http://www.w3.org/XML/1998/namespace"), S("foo:"), ICE),
    (S("http://www.w3.org/XML/1998/namespace"), S("xml"), OK),
    (S("http://www.w3.org/XML/1998/namespace"), S("xmlns"), NSE),
    (S("http://www.w3.org/XML/1998/namespace"), S("xmlfoo"), OK),
    (S("http://www.w3.org/XML/1998/namespace"), S("xml:foo"), OK),
    (
        S("http://www.w3.org/XML/1998/namespace"),
        S("xmlns:foo"),
        NSE,
    ),
    (
        S("http://www.w3.org/XML/1998/namespace"),
        S("xmlfoo:bar"),
        OK,
    ),
    (
        S("http://www.w3.org/XML/1998/namespaces"),
        S("xml:foo"),
        NSE,
    ),
    (S("http://www.w3.org/xml/1998/namespace"), S("xml:foo"), NSE),
    (S("http://www.w3.org/2000/xmlns/"), S("foo"), NSE),
    (S("http://www.w3.org/2000/xmlns/"), S("1foo"), ICE),
    (S("http://www.w3.org/2000/xmlns/"), S("f1oo"), NSE),
    (S("http://www.w3.org/2000/xmlns/"), S("foo1"), NSE),
    (S("http://www.w3.org/2000/xmlns/"), S(":foo"), ICE),
    (S("http://www.w3.org/2000/xmlns/"), S("f:oo"), NSE),
    (S("http://www.w3.org/2000/xmlns/"), S("foo:"), ICE),
    (S("http://www.w3.org/2000/xmlns/"), S("xml"), NSE),
    (S("http://www.w3.org/2000/xmlns/"), S("xmlns"), OK),
    (S("http://www.w3.org/2000/xmlns/"), S("xmlfoo"), NSE),
    (S("http://www.w3.org/2000/xmlns/"), S("xml:foo"), NSE),
    (S("http://www.w3.org/2000/xmlns/"), S("xmlns:foo"), OK),
    (S("http://www.w3.org/2000/xmlns/"), S("xmlfoo:bar"), NSE),
    (S("http://www.w3.org/2000/xmlns/"), S("foo:xmlns"), NSE),
    (S("foo:"), S("foo"), OK),
    (S("foo:"), S("1foo"), ICE),
    (S("foo:"), S("f1oo"), OK),
    (S("foo:"), S("foo1"), OK),
    (S("foo:"), S(":foo"), ICE),
    (S("foo:"), S("f:oo"), OK),
    (S("foo:"), S("foo:"), ICE),
    (S("foo:"), S("xml"), OK),
    (S("foo:"), S("xmlns"), NSE),
    (S("foo:"), S("xmlfoo"), OK),
    (S("foo:"), S("xml:foo"), NSE),
    (S("foo:"), S("xmlns:foo"), NSE),
    (S("foo:"), S("xmlfoo:bar"), OK),
    // Casos extra de Document-createElementNS.html.
    (S(""), S(""), ICE),
    (Null, S(""), ICE),
    (Undefined, S(""), ICE),
    (S("http://example.com/"), Null, OK),
    (S("http://example.com/"), S(""), ICE),
    (S("/"), Null, OK),
    (S("/"), S(""), ICE),
    (S("http://www.w3.org/XML/1998/namespace"), Null, OK),
    (S("http://www.w3.org/XML/1998/namespace"), S(""), ICE),
    (S("http://www.w3.org/2000/xmlns/"), Null, NSE),
    (S("http://www.w3.org/2000/xmlns/"), S(""), ICE),
    (S("foo:"), Null, OK),
    (S("foo:"), S(""), ICE),
];

/// `DOMString?` (namespace): `null` y `undefined` son `None`.
fn ns_arg(a: Arg) -> Option<&'static str> {
    match a {
        Null | Undefined => None,
        S(s) => Some(s),
    }
}

/// `DOMString` (qualifiedName): `null` → "null", `undefined` → "undefined".
fn qname_arg(a: Arg) -> &'static str {
    match a {
        Null => "null",
        Undefined => "undefined",
        S(s) => s,
    }
}

// dom/nodes/Document-createElementNS.html: "createElementNS test in {HTML, XML, XHTML} document:
// <namespace>,<qualifiedName>,<expected>" (la tabla entera en los tres documentos)
#[test]
fn create_element_ns_table() {
    for kind in DOC_KINDS {
        let mut doc = make_doc(kind);
        for &(namespace, qualified_name, expected) in CREATE_ELEMENT_NS_TESTS {
            let ns = ns_arg(namespace);
            let qualified = qname_arg(qualified_name);
            let name = format!(
                "createElementNS test in {kind:?} document: {namespace:?},{qualified_name:?},{expected:?}"
            );
            let result = doc.create_element_ns(ns, &ds(qualified));
            let element = match expected {
                Exp::InvalidChar => {
                    assert_eq!(result, Err(DomException::InvalidCharacterError), "{name}");
                    continue;
                }
                Exp::Namespace => {
                    assert_eq!(result, Err(DomException::NamespaceError), "{name}");
                    continue;
                }
                Exp::Ok => result.unwrap_or_else(|e| panic!("{name}: {e:?}")),
            };
            assert_eq!(doc.node_type(element), NodeType::Element, "{name}");
            assert_eq!(doc.node_value(element), None, "{name}");
            assert_eq!(doc.owner_document(element), Some(doc.root()), "{name}");
            let (prefix, local) = match qualified.find(':') {
                Some(i) => (Some(&qualified[..i]), &qualified[i + 1..]),
                None => (None, qualified),
            };
            assert_eq!(st(doc.prefix(element)).as_deref(), prefix, "{name}: prefix");
            assert_eq!(doc.local_name(element), local, "{name}: localName");
            assert_eq!(doc.tag_name(element), qualified, "{name}: tagName");
            assert_eq!(doc.node_name(element), qualified, "{name}: nodeName");
            let expected_ns = ns.filter(|s| !s.is_empty());
            assert_eq!(
                st(doc.namespace_uri(element)).as_deref(),
                expected_ns,
                "{name}: namespaceURI"
            );
        }
    }
}

/// Chequeo común de los tests con nombre de `Document-createElementNS.html`.
fn check_created_ns(
    ns: Option<&str>,
    qname: &str,
    expected_ns: Option<&str>,
    prefix: Option<&str>,
    local: &str,
    tag: &str,
) -> (Document, NodeId) {
    let mut doc = Document::new();
    let element = elem_ns(&mut doc, ns, qname);
    assert_eq!(st(doc.namespace_uri(element)).as_deref(), expected_ns);
    assert_eq!(st(doc.prefix(element)).as_deref(), prefix);
    assert_eq!(doc.local_name(element), local);
    assert_eq!(doc.tag_name(element), tag);
    assert_eq!(doc.node_type(element), NodeType::Element);
    (doc, element)
}

// dom/nodes/Document-createElementNS.html: "Lower-case HTML element without a prefix"
#[test]
fn create_element_ns_lower_html_no_prefix() {
    let (doc, el) = check_created_ns(Some(HTML_NS), "span", Some(HTML_NS), None, "span", "SPAN");
    assert_eq!(doc.html_kind(el), Some(bf_dom::HtmlElementKind::Span));
}

// dom/nodes/Document-createElementNS.html: "Lower-case HTML element with a prefix"
#[test]
fn create_element_ns_lower_html_prefix() {
    let (doc, el) = check_created_ns(
        Some(HTML_NS),
        "html:span",
        Some(HTML_NS),
        Some("html"),
        "span",
        "HTML:SPAN",
    );
    assert_eq!(doc.html_kind(el), Some(bf_dom::HtmlElementKind::Span));
}

// dom/nodes/Document-createElementNS.html: "Lower-case non-HTML element without a prefix"
#[test]
fn create_element_ns_lower_non_html_no_prefix() {
    let (doc, el) = check_created_ns(Some("test"), "span", Some("test"), None, "span", "span");
    assert_eq!(doc.html_kind(el), None);
}

// dom/nodes/Document-createElementNS.html: "Lower-case non-HTML element with a prefix"
#[test]
fn create_element_ns_lower_non_html_prefix() {
    let (doc, el) = check_created_ns(
        Some("test"),
        "html:span",
        Some("test"),
        Some("html"),
        "span",
        "html:span",
    );
    assert_eq!(doc.html_kind(el), None);
}

// dom/nodes/Document-createElementNS.html: "Upper-case HTML element without a prefix"
#[test]
fn create_element_ns_upper_html_no_prefix() {
    let (doc, el) = check_created_ns(Some(HTML_NS), "SPAN", Some(HTML_NS), None, "SPAN", "SPAN");
    // HTMLUnknownElement, no HTMLSpanElement.
    assert_eq!(doc.html_kind(el), Some(bf_dom::HtmlElementKind::Unknown));
}

// dom/nodes/Document-createElementNS.html: "Upper-case HTML element with a prefix"
#[test]
fn create_element_ns_upper_html_prefix() {
    let (doc, el) = check_created_ns(
        Some(HTML_NS),
        "html:SPAN",
        Some(HTML_NS),
        Some("html"),
        "SPAN",
        "HTML:SPAN",
    );
    assert_ne!(doc.html_kind(el), Some(bf_dom::HtmlElementKind::Span));
    assert!(doc.html_kind(el).is_some(), "Should be an HTMLElement");
}

// dom/nodes/Document-createElementNS.html: "Upper-case non-HTML element without a prefix"
#[test]
fn create_element_ns_upper_non_html_no_prefix() {
    let (doc, el) = check_created_ns(Some("test"), "SPAN", Some("test"), None, "SPAN", "SPAN");
    assert_eq!(doc.html_kind(el), None);
}

// dom/nodes/Document-createElementNS.html: "Upper-case non-HTML element with a prefix"
#[test]
fn create_element_ns_upper_non_html_prefix() {
    let (doc, el) = check_created_ns(
        Some("test"),
        "html:SPAN",
        Some("test"),
        Some("html"),
        "SPAN",
        "html:SPAN",
    );
    assert_eq!(doc.html_kind(el), None);
}

// dom/nodes/Document-createElementNS.html: "null namespace", "undefined namespace" y "empty
// string namespace"
#[test]
fn create_element_ns_null_undefined_empty_namespace() {
    for ns in [None, None, Some("")] {
        let (doc, el) = check_created_ns(ns, "span", None, None, "span", "span");
        assert_eq!(doc.html_kind(el), None);
    }
}

// --- Document-getElementById.html ---

/// El body estático del test: `div#log`, `div[id=""]`, `div#test1`, los tres `#test5` y
/// `#outer > #middle > #inner`.
fn get_element_by_id_doc() -> Document {
    let mut doc = html_document(Some("Document.getElementById"));
    let body = doc.body().unwrap();
    let mk = |doc: &mut Document, parent: NodeId, name: &str, attrs: &[(&str, &str)]| {
        let e = elem(doc, name);
        for &(k, v) in attrs {
            set_attr(doc, e, k, v);
        }
        append(doc, parent, e);
        e
    };
    mk(&mut doc, body, "div", &[("id", "log")]);
    mk(&mut doc, body, "div", &[("id", "")]);
    mk(&mut doc, body, "div", &[("id", "test1")]);
    let t5 = mk(
        &mut doc,
        body,
        "div",
        &[("id", "test5"), ("data-name", "1st")],
    );
    let p = mk(&mut doc, t5, "p", &[("id", "test5"), ("data-name", "2nd")]);
    let t = text(&mut doc, "P");
    append(&mut doc, p, t);
    mk(
        &mut doc,
        t5,
        "input",
        &[
            ("id", "test5"),
            ("type", "submit"),
            ("value", "Submit"),
            ("data-name", "3rd"),
        ],
    );
    let outer = mk(&mut doc, body, "div", &[("id", "outer")]);
    let middle = mk(&mut doc, outer, "div", &[("id", "middle")]);
    mk(&mut doc, middle, "div", &[("id", "inner")]);
    doc
}

// dom/nodes/Document-getElementById.html: "Calling document.getElementById with an empty string
// argument."
#[test]
fn get_element_by_id_empty_string() {
    let doc = get_element_by_id_doc();
    assert_eq!(by_id(&doc, ""), None);
}

// dom/nodes/Document-getElementById.html: "Calling document.getElementById with a null argument."
// y "... with an undefined argument."
#[test]
fn get_element_by_id_null_and_undefined() {
    let mut doc = get_element_by_id_doc();
    let body = doc.body().unwrap();
    for id in ["null", "undefined"] {
        let element = elem(&mut doc, "div");
        set_attr(&mut doc, element, "id", id);
        append(&mut doc, body, element);
        assert_eq!(by_id(&doc, id), Some(element));
        doc.remove_child(body, element).unwrap();
    }
}

// dom/nodes/Document-getElementById.html: "on static page"
#[test]
fn get_element_by_id_static_page() {
    let doc = get_element_by_id_doc();
    let bar = by_id(&doc, "test1").expect("should not be null");
    assert_eq!(doc.tag_name(bar), "DIV");
    assert_eq!(doc.html_kind(bar), Some(bf_dom::HtmlElementKind::Div));
}

// dom/nodes/Document-getElementById.html: "Document.getElementById with a script-inserted
// element"
#[test]
fn get_element_by_id_script_inserted() {
    let mut doc = get_element_by_id_doc();
    let body = doc.body().unwrap();
    let test = elem(&mut doc, "div");
    set_attr(&mut doc, test, "id", "test2");
    append(&mut doc, body, test);
    let result = by_id(&doc, "test2").expect("should not be null.");
    assert_eq!(doc.tag_name(result), "DIV");
    doc.remove_child(body, test).unwrap();
    assert_eq!(
        by_id(&doc, "test2"),
        None,
        "should not get removed element."
    );
}

// dom/nodes/Document-getElementById.html: "update `id` attribute via setAttribute/removeAttribute"
#[test]
fn get_element_by_id_set_remove_attribute() {
    let mut doc = get_element_by_id_doc();
    let body = doc.body().unwrap();
    let test = elem(&mut doc, "div");
    set_attr(&mut doc, test, "id", "test3");
    append(&mut doc, body, test);
    set_attr(&mut doc, test, "id", "test3-updated");
    assert_eq!(by_id(&doc, "test3-updated"), Some(test));
    assert_eq!(by_id(&doc, "test3"), None);
    doc.remove_attribute(test, &ds("id"));
    assert_eq!(by_id(&doc, "test3-updated"), None);
}

// dom/nodes/Document-getElementById.html: "Ensure that the id attribute only affects elements
// present in a document"
#[test]
fn get_element_by_id_only_in_document() {
    let mut doc = get_element_by_id_doc();
    let body = doc.body().unwrap();
    let e = elem(&mut doc, "div");
    set_attr(&mut doc, e, "id", "test4-should-not-exist");
    assert_eq!(by_id(&doc, "test4-should-not-exist"), None);
    append(&mut doc, body, e);
    assert_eq!(by_id(&doc, "test4-should-not-exist"), Some(e));
}

// dom/nodes/Document-getElementById.html: "in tree order, within the context object's tree"
#[test]
fn get_element_by_id_tree_order() {
    let mut doc = get_element_by_id_doc();
    let body = doc.body().unwrap();
    let target = by_id(&doc, "test5").expect("should not be null");
    assert_eq!(get_attr(&doc, target, "data-name").as_deref(), Some("1st"));

    let element4 = elem(&mut doc, "div");
    set_attr(&mut doc, element4, "id", "test5");
    set_attr(&mut doc, element4, "data-name", "4th");
    append(&mut doc, body, element4);
    let target2 = by_id(&doc, "test5").expect("should not be null");
    assert_eq!(get_attr(&doc, target2, "data-name").as_deref(), Some("1st"));

    let parent = doc.parent(target2).unwrap();
    doc.remove_child(parent, target2).unwrap();
    let target3 = by_id(&doc, "test5").expect("should not be null");
    assert_eq!(get_attr(&doc, target3, "data-name").as_deref(), Some("4th"));
}

// dom/nodes/Document-getElementById.html: "Modern browsers optimize this method with using
// internal id cache. This test checks that their optimization should effect only append to
// `Document`, not append to `Node`."
#[test]
fn get_element_by_id_append_to_detached_node() {
    let mut doc = get_element_by_id_doc();
    let s = elem(&mut doc, "div");
    set_attr(&mut doc, s, "id", "test6");
    let d = elem(&mut doc, "div");
    append(&mut doc, d, s);
    assert_eq!(by_id(&doc, "test6"), None);
}

// dom/nodes/Document-getElementById.html: "changing attribute's value via `Attr` gotten from
// `Element.attribute`."
#[test]
fn get_element_by_id_attr_value_change() {
    let mut doc = get_element_by_id_doc();
    let body = doc.body().unwrap();
    let element = elem(&mut doc, "div");
    set_attr(&mut doc, element, "id", "test7");
    append(&mut doc, body, element);
    assert_eq!(by_id(&doc, "test7"), Some(element));
    let attr = doc.attributes_item(element, 0).unwrap();
    doc.set_attr_value(attr, ds("test7-updated"));
    assert_eq!(by_id(&doc, "test7"), None);
    assert_eq!(by_id(&doc, "test7-updated"), Some(element));
}

// dom/nodes/Document-getElementById.html: "update `id` attribute via element.id"
#[test]
fn get_element_by_id_element_id_setter() {
    let mut doc = get_element_by_id_doc();
    let body = doc.body().unwrap();
    let test = elem(&mut doc, "div");
    doc.set_id(test, ds("test12"));
    append(&mut doc, body, test);
    doc.set_id(test, ds("test12-updated"));
    assert_eq!(by_id(&doc, "test12-updated"), Some(test));
    assert_eq!(by_id(&doc, "test12"), None);
    doc.set_id(test, ds(""));
    assert_eq!(by_id(&doc, "test12-updated"), None);
}

// dom/nodes/Document-getElementById.html: "where insertion order and tree order don't match"
#[test]
fn get_element_by_id_insertion_vs_tree_order() {
    let mut doc = get_element_by_id_doc();
    let body = doc.body().unwrap();
    let mut make = |order: &str| {
        let e = elem(&mut doc, "div");
        set_attr(&mut doc, e, "id", "test13");
        set_attr(&mut doc, e, "data-order", order);
        e
    };
    let (e1, e2, e3, e4) = (make("1"), make("2"), make("3"), make("4"));
    let container = elem(&mut doc, "div");
    set_attr(&mut doc, container, "id", "test13-fixture");
    append(&mut doc, body, container);
    // 2 -> 4 -> 3 -> 1
    append(&mut doc, container, e2);
    append(&mut doc, container, e4);
    doc.insert_before_checked(container, e3, Some(e4)).unwrap();
    doc.insert_before_checked(container, e1, Some(e2)).unwrap();
    for (e, next) in [(e1, Some(e2)), (e2, Some(e3)), (e3, Some(e4)), (e4, None)] {
        assert_eq!(by_id(&doc, "test13"), Some(e));
        doc.remove_child(container, e).unwrap();
        assert_eq!(by_id(&doc, "test13"), next);
    }
}

// dom/nodes/Document-getElementById.html: "Inserting an id by inserting its parent node"
#[test]
fn get_element_by_id_insert_parent() {
    let mut doc = get_element_by_id_doc();
    let body = doc.body().unwrap();
    let a = elem(&mut doc, "a");
    let b = elem(&mut doc, "b");
    append(&mut doc, a, b);
    doc.set_id(b, ds("test14"));
    assert_eq!(by_id(&doc, "test14"), None);
    append(&mut doc, body, a);
    assert_eq!(by_id(&doc, "test14"), Some(b));
}

// dom/nodes/Document-getElementById.html: "Document.getElementById must not return nodes not
// present in document"
#[test]
fn get_element_by_id_removed_subtree() {
    let mut doc = get_element_by_id_doc();
    let outer = by_id(&doc, "outer").unwrap();
    let middle = by_id(&doc, "middle").unwrap();
    let inner = by_id(&doc, "inner").unwrap();
    doc.remove_child(outer, middle).unwrap();
    let new_el = elem(&mut doc, "h1");
    doc.set_id(new_el, ds("heading"));
    append(&mut doc, inner, new_el);
    assert_eq!(by_id(&doc, "heading"), None);
}
