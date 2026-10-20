//! Transcripción a Rust de tests de WPT (`tests/wpt/dom/nodes/`) sobre `Element`, sus
//! atributos (`Attr`, `NamedNodeMap`), `DOMTokenList` (`classList`), `getElementsByClassName`,
//! el traversal por elementos, `CharacterData`/`Text`, los mixins `ParentNode`/`ChildNode`,
//! `insertAdjacentElement`/`insertAdjacentText` y `MutationObserver`; más tests propios de
//! `AbortController` (DOM §3), porque `tests/wpt/dom/abort/` no está en disco.
//!
//! Los árboles se arman a mano con la API (en `bf-dom` no hay parser). Las entregas de
//! `MutationObserver` se simulan con `notify_mutation_observers()` o `take_records()`.
//! Los argumentos `null`/`undefined` de JS se transcriben como los strings `"null"` y
//! `"undefined"` (la conversión de WebIDL a `DOMString`), y los `unsigned long` negativos o
//! mayores que 2^32 se reducen módulo 2^32 con [`u32_arg`].
//!
//! Subtests omitidos (dependen de JS, de otro documento, del parser, de ranges, de shadow DOM o
//! de `Window`):
//! - Element-classlist.html: "Assigning to classList" en su forma JS (solo se prueba el setter de
//!   `value`), el acceso indexado `classList[i]` (se prueba `item(i)`) y `toString()` como
//!   stringifier (se prueba `value`).
//! - attributes.html: "Specified attributes should be accessible" y "Entities in attributes
//!   should have been expanded while parsing" (parser); el async_test de `DOMAttrModified`
//!   (mutation events, que no existen); los "Own property ..." (propiedades JS de
//!   `NamedNodeMap`) y el que usa `document.implementation.createDocument`.
//! - attributes-namednodemap.html: solo se transcribe setNamedItem/removeNamedItem; el resto son
//!   propiedades JS con nombre.
//! - Element-hasAttributes.html: los elementos del markup se arman a mano.
//! - Element-getElementsByClassName.html: "Interface should be correct." (`instanceof`).
//! - getElementsByClassName-10/11 (documentos XML), 19–23 y 25–32 (fixtures grandes del parser;
//!   cubren lo mismo que los transcritos).
//! - Element-children.html: la enumeración de propiedades JS (`for..in`, `getOwnPropertyNames`).
//! - CharacterData-*: los subtests de aridad JS (`appendData()` sin argumentos, `substringData()`
//!   con argumentos de menos o de más, `substringData("test", 3)`).
//! - Variantes `-svg.svg`/`-xhtml.xhtml` de los tests de traversal (mismo algoritmo).
//! - pre-insertion-validation-hierarchy.js: el paso 4 (insertar otro `Document`, que no se
//!   puede crear en la misma arena).
//! - ParentNode-replaceChildren.html: nada (los fixtures de `innerHTML` se arman a mano).
//! - MutationObserver-childList.html: los de `Range` (n70–n100).
//! - MutationObserver-attributes.html: n03 (propiedad JS `unknown`) y n04 (`HTMLInputElement.type`,
//!   reflejo de HTML).
//! - MutationObserver-characterData.html: los de `Range` (n70–n81).
//! - Element-insertAdjacent*.html: el estilo `visibility` del documentElement (irrelevante).

use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use bf_dom::abort::AbortReason;
use bf_dom::events::{Event, Listener, ListenerOptions, NoScript};
use bf_dom::observer::{MutationRecord, MutationType, ObserverInit};
use bf_dom::{
    Document, DomException, DomString, NodeData, NodeId, NodeOrString, NodeType, QuirksMode,
};

const XHTML: &str = "http://www.w3.org/1999/xhtml";
const MATHML: &str = "http://www.w3.org/1998/Math/MathML";
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

// --- Helpers generales ---

fn s(x: &str) -> DomString {
    DomString::from(x)
}

fn ss(v: &[&str]) -> Vec<DomString> {
    v.iter().map(|x| s(x)).collect()
}

/// Un `unsigned long` de WebIDL a partir de un número de JS (módulo 2^32).
fn u32_arg(x: i64) -> usize {
    usize::try_from(x.rem_euclid(1 << 32)).expect("cabe en usize")
}

fn el(doc: &mut Document, name: &str) -> NodeId {
    doc.create_element(&s(name)).expect("createElement")
}

fn el_ns(doc: &mut Document, ns: Option<&str>, qualified: &str) -> NodeId {
    doc.create_element_ns(ns, &s(qualified))
        .expect("createElementNS")
}

fn text(doc: &mut Document, data: &str) -> NodeId {
    doc.create_text_node(s(data))
}

fn comment(doc: &mut Document, data: &str) -> NodeId {
    doc.create_comment(s(data))
}

fn append(doc: &mut Document, parent: NodeId, child: NodeId) {
    doc.append_child(parent, child).expect("appendChild");
}

fn set_attr(doc: &mut Document, e: NodeId, name: &str, value: &str) {
    doc.set_attribute(e, &s(name), s(value))
        .expect("setAttribute");
}

fn set_attr_ns(doc: &mut Document, e: NodeId, ns: Option<&str>, qualified: &str, value: &str) {
    doc.set_attribute_ns(e, ns, &s(qualified), s(value))
        .expect("setAttributeNS");
}

fn get_attr(doc: &Document, e: NodeId, name: &str) -> Option<String> {
    doc.get_attribute(e, &s(name)).map(|v| v.to_string())
}

fn get_attr_ns(doc: &Document, e: NodeId, ns: Option<&str>, local: &str) -> Option<String> {
    doc.get_attribute_ns(e, ns, &s(local))
        .map(|v| v.to_string())
}

fn has_attr(doc: &Document, e: NodeId, name: &str) -> bool {
    doc.has_attribute(e, &s(name))
}

fn has_attr_ns(doc: &Document, e: NodeId, ns: Option<&str>, local: &str) -> bool {
    doc.has_attribute_ns(e, ns, &s(local))
}

fn node(id: NodeId) -> NodeOrString {
    NodeOrString::Node(id)
}

fn string(x: &str) -> NodeOrString {
    NodeOrString::String(s(x))
}

fn data(doc: &Document, n: NodeId) -> String {
    doc.data(n).to_string()
}

/// `innerHTML` mínimo para los tests de `ChildNode`: elementos sin atributos, texto y
/// comentarios.
fn inner_html(doc: &Document, n: NodeId) -> String {
    let mut out = String::new();
    for c in doc.child_nodes(n) {
        match doc.node_type(c) {
            NodeType::Element => {
                let name = doc.local_name(c);
                write!(out, "<{name}>{}</{name}>", inner_html(doc, c)).expect("write");
            }
            NodeType::Text => out.push_str(&data(doc, c)),
            NodeType::Comment => write!(out, "<!--{}-->", doc.data(c)).expect("write"),
            other => panic!("nodo inesperado en inner_html: {other:?}"),
        }
    }
    out
}

/// Un documento como el de `document.implementation.createHTMLDocument("title")`.
struct HtmlDoc {
    doc: Document,
    doctype: NodeId,
    html: NodeId,
    body: NodeId,
}

fn html_document() -> HtmlDoc {
    let mut doc = Document::new();
    let root = doc.root();
    let doctype = doc
        .create_document_type(s("html"), s(""), s(""))
        .expect("doctype");
    append(&mut doc, root, doctype);
    let html = el(&mut doc, "html");
    append(&mut doc, root, html);
    let head = el(&mut doc, "head");
    append(&mut doc, html, head);
    let title = el(&mut doc, "title");
    append(&mut doc, head, title);
    let t = text(&mut doc, "title");
    append(&mut doc, title, t);
    let body = el(&mut doc, "body");
    append(&mut doc, html, body);
    HtmlDoc {
        doc,
        doctype,
        html,
        body,
    }
}

// --- Element-classlist.html ---

/// Los cinco elementos sobre los que corre `testClassList`, con su descripción.
fn class_list_elements(doc: &mut Document) -> Vec<(NodeId, &'static str)> {
    let content = el(doc, "div");
    let list = vec![
        (el(doc, "div"), " (HTML node)"),
        (el_ns(doc, Some(XHTML), "div"), " (XHTML node)"),
        (el_ns(doc, Some(MATHML), "math"), " (MathML node)"),
        (el_ns(doc, None, "foo"), " (XML node with null namespace)"),
        (
            el_ns(doc, Some("http://example.org/foo"), "foo"),
            " (foo node)",
        ),
    ];
    for &(e, _) in &list {
        append(doc, content, e);
    }
    list
}

/// `setClass` del test: `None` quita el atributo.
fn set_class(doc: &mut Document, e: NodeId, value: Option<&str>) {
    match value {
        None => doc.remove_attribute(e, &s("class")),
        Some(v) => set_attr(doc, e, "class", v),
    }
}

#[derive(Debug, Clone, Copy)]
enum Op<'a> {
    Add(&'a [&'a str]),
    Remove(&'a [&'a str]),
    Toggle(&'a str, Option<bool>),
    Replace(&'a str, &'a str),
}

#[derive(Debug, Clone, Copy)]
enum Expect<'a> {
    /// Valor devuelto (`None` para `add`/`remove`) y valor de `class` después.
    Ok(Option<bool>, Option<&'a str>),
    Throws(DomException),
}

/// `checkModification` del test.
fn check_modification(
    doc: &mut Document,
    e: NodeId,
    desc: &str,
    op: Op<'_>,
    before: Option<&str>,
    expect: Expect<'_>,
) {
    set_class(doc, e, before);
    let obs = doc.create_mutation_observer();
    doc.observe(
        obs,
        e,
        ObserverInit {
            attributes: Some(true),
            ..Default::default()
        },
    )
    .expect("observe");
    let list = doc.class_list(e);
    let res = match op {
        Op::Add(tokens) => list.add(doc, &ss(tokens)).map(|()| None),
        Op::Remove(tokens) => list.remove(doc, &ss(tokens)).map(|()| None),
        Op::Toggle(t, force) => list.toggle(doc, &s(t), force).map(Some),
        Op::Replace(t, new) => list.replace(doc, &s(t), &s(new)).map(Some),
    };
    let records = doc.take_records(obs);
    doc.disconnect_observer(obs);
    let label = format!("classList.{op:?} con valor {before:?}{desc}");
    let after = match expect {
        Expect::Throws(err) => {
            assert_eq!(res, Err(err), "{label}");
            assert_eq!(records.len(), 0, "{label}: no debe haber mutaciones");
            before
        }
        Expect::Ok(expected_res, after) => {
            if let Op::Replace(..) = op {
                assert_eq!(
                    records.len() == 1,
                    expected_res == Some(true),
                    "{label}: hay una mutación exactamente cuando replace() devuelve true"
                );
            }
            assert_eq!(res, Ok(expected_res), "{label}: valor devuelto");
            after
        }
    };
    assert_eq!(
        get_attr(doc, e, "class").as_deref(),
        after,
        "{label}: class después de la modificación"
    );
}

#[derive(Debug, Clone, Copy)]
enum Param {
    Normal,
    Noop,
    Throws(DomException),
}

const OK: Param = Param::Normal;
const NOOP: Param = Param::Noop;
const SYNTAX: Param = Param::Throws(DomException::SyntaxError);
const INVALID: Param = Param::Throws(DomException::InvalidCharacterError);

type ModCase<'a> = (Option<&'a str>, &'a [&'a str], Option<&'a str>, Param);

/// `checkAdd`/`checkRemove`: la operación y, con un solo token, `toggle(token, force)`.
fn check_add_or_remove(doc: &mut Document, e: NodeId, desc: &str, add: bool, case: ModCase<'_>) {
    let (before, args, after, param) = case;
    let expect = match param {
        Param::Throws(x) => Expect::Throws(x),
        _ => Expect::Ok(None, after),
    };
    let op = if add { Op::Add(args) } else { Op::Remove(args) };
    check_modification(doc, e, desc, op, before, expect);
    // En JS, un argumento que no es array también se prueba con toggle forzado (que no corre
    // los update steps si no cambia nada).
    if let [arg] = args {
        let expect = match param {
            Param::Throws(x) => Expect::Throws(x),
            Param::Noop => Expect::Ok(Some(add), before),
            Param::Normal => Expect::Ok(Some(add), after),
        };
        check_modification(doc, e, desc, Op::Toggle(arg, Some(add)), before, expect);
    }
}

const WS: &str = "\t\n\x0C\r a\t\n\x0C\r b\t\n\x0C\r ";

#[test]
// Element-classlist.html: ".supports() must throw TypeError".
fn classlist_supports_lanza_type_error() {
    let mut doc = Document::new();
    for (e, desc) in class_list_elements(&mut doc) {
        assert_eq!(
            doc.class_list(e).supports(&s("a")),
            Err(DomException::TypeError),
            "{desc}"
        );
    }
}

#[test]
// Element-classlist.html: "classList.length when ..." (checkLength).
fn classlist_length() {
    let mut doc = Document::new();
    let cases: &[(Option<&str>, usize)] = &[
        (None, 0),
        (Some(""), 0),
        (Some("   \t  \x0C"), 0),
        (Some("a"), 1),
        (Some("a A"), 2),
        (Some("\r\na\t\x0C"), 1),
        (Some("a a"), 1),
        (Some("a a a a a a"), 1),
        (Some("a a b b"), 2),
        (Some("a A B b"), 4),
        (Some("a b c c b a a b c c"), 3),
        (Some("   a  a b"), 2),
        (Some("a\tb\nc\x0Cd\re f"), 6),
    ];
    for (e, desc) in class_list_elements(&mut doc) {
        for &(value, length) in cases {
            set_class(&mut doc, e, value);
            assert_eq!(
                doc.class_list(e).length(&doc),
                length,
                "length con {value:?}{desc}"
            );
        }
    }
}

#[test]
// Element-classlist.html: "classList.toString() when ..." (el stringifier es `value`) y
// "Assigning to classList" (el setter de `value`, al que reenvía [PutForwards]).
fn classlist_value_y_asignacion() {
    let mut doc = Document::new();
    for (e, desc) in class_list_elements(&mut doc) {
        for (value, expected) in [
            (None, ""),
            (Some("foo"), "foo"),
            (Some("   a  a b"), "   a  a b"),
        ] {
            set_class(&mut doc, e, value);
            assert_eq!(doc.class_list(e).value(&doc), expected, "{value:?}{desc}");
        }
        let list = doc.class_list(e);
        list.set_value(&mut doc, s("foo"));
        assert_eq!(get_attr(&doc, e, "class").as_deref(), Some("foo"), "{desc}");
        assert_eq!(
            doc.class_list(e),
            list,
            "classList no cambia al asignarle{desc}"
        );
        doc.remove_attribute(e, &s("class"));
    }
}

#[test]
// Element-classlist.html: "classList.item() when set to ..." (checkItems).
fn classlist_item() {
    let mut doc = Document::new();
    let cases: &[(Option<&str>, &[&str])] = &[
        (None, &[]),
        (Some("a"), &["a"]),
        (Some("aa AA aa"), &["aa", "AA"]),
        (Some("a b"), &["a", "b"]),
        (Some("   a  a b"), &["a", "b"]),
        (Some(WS), &["a", "b"]),
    ];
    for (e, desc) in class_list_elements(&mut doc) {
        for &(value, expected) in cases {
            set_class(&mut doc, e, value);
            let list = doc.class_list(e);
            assert_eq!(list.item(&doc, u32_arg(-1)), None, "item(-1){desc}");
            for (i, token) in expected.iter().enumerate() {
                assert_eq!(
                    list.item(&doc, i).map(|t| t.to_string()).as_deref(),
                    Some(*token),
                    "item({i}) con {value:?}{desc}"
                );
            }
            assert_eq!(list.item(&doc, expected.len()), None, "{value:?}{desc}");
            assert_eq!(list.item(&doc, 0xffff_ffff), None, "{value:?}{desc}");
            assert_eq!(list.item(&doc, 0xffff_fffe), None, "{value:?}{desc}");
        }
    }
}

#[test]
// Element-classlist.html: "classList.contains(...) when set to ..." (checkContains).
fn classlist_contains() {
    let mut doc = Document::new();
    type Checks<'a> = &'a [(&'a str, bool)];
    let cases: &[(Option<&str>, Checks<'_>)] = &[
        (None, &[("a", false), ("", false), ("  ", false)]),
        (Some(""), &[("a", false)]),
        (Some("a"), &[("a", true)]),
        (
            Some("a"),
            &[
                ("aa", false),
                ("b", false),
                ("A", false),
                ("a.", false),
                ("a)", false),
                ("undefined", false),
                ("a'", false),
                ("a\"", false),
                ("a$", false),
                ("a~", false),
                ("a?", false),
                ("a\\", false),
            ],
        ),
        (
            Some("a"),
            &[
                ("a\t", false),
                ("\ta", false),
                ("a\n", false),
                ("\na", false),
                ("a\x0C", false),
                ("\x0Ca", false),
                ("a\r", false),
                ("\ra", false),
                ("a ", false),
                (" a", false),
            ],
        ),
        (Some("aa AA"), &[("aa", true), ("AA", true), ("aA", false)]),
        (Some("a a a"), &[("a", true), ("aa", false), ("b", false)]),
        (Some("a b c"), &[("a", true), ("b", true)]),
        (
            Some("null undefined"),
            &[("null", true), ("undefined", true)],
        ),
        (Some(WS), &[("a", true), ("b", true)]),
    ];
    for (e, desc) in class_list_elements(&mut doc) {
        for &(value, checks) in cases {
            set_class(&mut doc, e, value);
            for &(token, expected) in checks {
                assert_eq!(
                    doc.class_list(e).contains(&doc, &s(token)),
                    expected,
                    "contains({token:?}) con {value:?}{desc}"
                );
            }
        }
    }
}

#[test]
// Element-classlist.html: checkAdd con SyntaxError/InvalidCharacterError (y toggle(x, true)).
fn classlist_add_errores() {
    let mut doc = Document::new();
    let cases: &[ModCase<'_>] = &[
        (None, &[""], None, SYNTAX),
        (None, &["a", ""], None, SYNTAX),
        (None, &[" "], None, INVALID),
        (None, &["\ta"], None, INVALID),
        (None, &["a\t"], None, INVALID),
        (None, &["\na"], None, INVALID),
        (None, &["a\n"], None, INVALID),
        (None, &["\x0Ca"], None, INVALID),
        (None, &["a\x0C"], None, INVALID),
        (None, &["\ra"], None, INVALID),
        (None, &["a\r"], None, INVALID),
        (None, &[" a"], None, INVALID),
        (None, &["a "], None, INVALID),
        (None, &["a", " "], None, INVALID),
        (None, &["a", "aa "], None, INVALID),
    ];
    for (e, desc) in class_list_elements(&mut doc) {
        for &case in cases {
            check_add_or_remove(&mut doc, e, desc, true, case);
        }
    }
}

#[test]
// Element-classlist.html: checkAdd (incluido "multiple add" y toggle(x, true)).
fn classlist_add() {
    let mut doc = Document::new();
    let cases: &[ModCase<'_>] = &[
        (Some("a"), &["a"], Some("a"), OK),
        (Some("aa"), &["AA"], Some("aa AA"), OK),
        (Some("a b c"), &["a"], Some("a b c"), OK),
        (Some("a a a  b"), &["a"], Some("a b"), NOOP),
        (None, &["a"], Some("a"), OK),
        (Some(""), &["a"], Some("a"), OK),
        (Some(" "), &["a"], Some("a"), OK),
        (Some("   \x0C"), &["a"], Some("a"), OK),
        (Some("a"), &["b"], Some("a b"), OK),
        (Some("a b c"), &["d"], Some("a b c d"), OK),
        (Some("a b c "), &["d"], Some("a b c d"), OK),
        (Some("   a  a b"), &["c"], Some("a b c"), OK),
        (Some("   a  a b"), &["a"], Some("a b"), NOOP),
        (Some(WS), &["c"], Some("a b c"), OK),
        (Some("a b c "), &["d", "e"], Some("a b c d e"), OK),
        (Some("a b c "), &["a", "a"], Some("a b c"), OK),
        (Some("a b c "), &["d", "d"], Some("a b c d"), OK),
        (Some("a b c a "), &[], Some("a b c"), OK),
        (None, &["a", "b"], Some("a b"), OK),
        (Some(""), &["a", "b"], Some("a b"), OK),
        (None, &["null"], Some("null"), OK),
        (None, &["undefined"], Some("undefined"), OK),
    ];
    for (e, desc) in class_list_elements(&mut doc) {
        for &case in cases {
            check_add_or_remove(&mut doc, e, desc, true, case);
        }
    }
}

#[test]
// Element-classlist.html: checkRemove con SyntaxError/InvalidCharacterError (y
// toggle(x, false)).
fn classlist_remove_errores() {
    let mut doc = Document::new();
    let cases: &[ModCase<'_>] = &[
        (None, &[""], None, SYNTAX),
        (None, &[" "], None, INVALID),
        (Some("\ta"), &["\ta"], Some("\ta"), INVALID),
        (Some("a\t"), &["a\t"], Some("a\t"), INVALID),
        (Some("\na"), &["\na"], Some("\na"), INVALID),
        (Some("a\n"), &["a\n"], Some("a\n"), INVALID),
        (Some("\x0Ca"), &["\x0Ca"], Some("\x0Ca"), INVALID),
        (Some("a\x0C"), &["a\x0C"], Some("a\x0C"), INVALID),
        (Some("\ra"), &["\ra"], Some("\ra"), INVALID),
        (Some("a\r"), &["a\r"], Some("a\r"), INVALID),
        (Some(" a"), &[" a"], Some(" a"), INVALID),
        (Some("a "), &["a "], Some("a "), INVALID),
        (Some("aa "), &["aa "], None, INVALID),
    ];
    for (e, desc) in class_list_elements(&mut doc) {
        for &case in cases {
            check_add_or_remove(&mut doc, e, desc, false, case);
        }
    }
}

#[test]
// Element-classlist.html: checkRemove (incluido "multiple remove" y toggle(x, false)).
fn classlist_remove() {
    let mut doc = Document::new();
    let cases: &[ModCase<'_>] = &[
        (None, &["a"], None, OK),
        (Some(""), &["a"], Some(""), OK),
        (Some("a b  c"), &["d"], Some("a b c"), NOOP),
        (Some("a b  c"), &["A"], Some("a b c"), NOOP),
        (Some(" a a a "), &["a"], Some(""), OK),
        (Some("a  b"), &["a"], Some("b"), OK),
        (Some("a  b  "), &["a"], Some("b"), OK),
        (Some("a a b"), &["a"], Some("b"), OK),
        (Some("aa aa bb"), &["aa"], Some("bb"), OK),
        (Some("a a b a a c a a"), &["a"], Some("b c"), OK),
        (Some("a  b  c"), &["b"], Some("a c"), OK),
        (Some("aaa  bbb  ccc"), &["bbb"], Some("aaa ccc"), OK),
        (Some(" a  b  c "), &["b"], Some("a c"), OK),
        (Some("a b b b c"), &["b"], Some("a c"), OK),
        (Some("a  b  c"), &["c"], Some("a b"), OK),
        (Some(" a  b  c "), &["c"], Some("a b"), OK),
        (Some("a b c c c"), &["c"], Some("a b"), OK),
        (Some("a b a c a d a"), &["a"], Some("b c d"), OK),
        (
            Some("AA BB aa CC AA dd aa"),
            &["AA"],
            Some("BB aa CC dd"),
            OK,
        ),
        (Some("\ra\na\ta\x0C"), &["a"], Some(""), OK),
        (Some(WS), &["a"], Some("b"), OK),
        (Some("a b c "), &["d", "e"], Some("a b c"), OK),
        (Some("a b c "), &["a", "b"], Some("c"), OK),
        (Some("a b c "), &["a", "c"], Some("b"), OK),
        (Some("a b c "), &["a", "a"], Some("b c"), OK),
        (Some("a b c "), &["d", "d"], Some("a b c"), OK),
        (Some("a b c "), &[], Some("a b c"), OK),
        (None, &["a", "b"], None, OK),
        (Some(""), &["a", "b"], Some(""), OK),
        (Some("a a"), &[], Some("a"), OK),
        (Some("null"), &["null"], Some(""), OK),
        (Some("undefined"), &["undefined"], Some(""), OK),
    ];
    for (e, desc) in class_list_elements(&mut doc) {
        for &case in cases {
            check_add_or_remove(&mut doc, e, desc, false, case);
        }
    }
}

#[test]
// Element-classlist.html: checkToggle.
fn classlist_toggle() {
    let mut doc = Document::new();
    let errors: &[(&str, DomException)] = &[
        ("", DomException::SyntaxError),
        ("aa ", DomException::InvalidCharacterError),
    ];
    let cases: &[(Option<&str>, &str, bool, &str)] = &[
        (None, "a", true, "a"),
        (Some(""), "a", true, "a"),
        (Some(" "), "a", true, "a"),
        (Some("   \x0C"), "a", true, "a"),
        (Some("a"), "b", true, "a b"),
        (Some("a"), "A", true, "a A"),
        (Some("a b c"), "d", true, "a b c d"),
        (Some("   a  a b"), "d", true, "a b d"),
        (Some("a"), "a", false, ""),
        (Some(" a a a "), "a", false, ""),
        (Some(" A A A "), "a", true, "A a"),
        (Some(" a b c "), "b", false, "a c"),
        (Some(" a b c b b"), "b", false, "a c"),
        (Some(" a b  c  "), "c", false, "a b"),
        (Some(" a b c "), "a", false, "b c"),
        (Some("   a  a b"), "b", false, "a"),
        (Some(WS), "a", false, "b"),
        (Some(WS), "c", true, "a b c"),
        (Some("null"), "null", false, ""),
        (Some(""), "null", true, "null"),
        (Some("undefined"), "undefined", false, ""),
        (Some(""), "undefined", true, "undefined"),
    ];
    for (e, desc) in class_list_elements(&mut doc) {
        for &(token, err) in errors {
            check_modification(
                &mut doc,
                e,
                desc,
                Op::Toggle(token, None),
                None,
                Expect::Throws(err),
            );
        }
        for &(before, token, res, after) in cases {
            check_modification(
                &mut doc,
                e,
                desc,
                Op::Toggle(token, None),
                before,
                Expect::Ok(Some(res), Some(after)),
            );
        }
    }
}

#[test]
// Element-classlist.html: checkReplace con SyntaxError/InvalidCharacterError.
fn classlist_replace_errores() {
    let mut doc = Document::new();
    let syntax = DomException::SyntaxError;
    let invalid = DomException::InvalidCharacterError;
    let cases: &[(&str, &str, DomException)] = &[
        ("", "a", syntax),
        ("", " ", syntax),
        (" ", "a", invalid),
        ("\ta", "b", invalid),
        ("a\t", "b", invalid),
        ("\na", "b", invalid),
        ("a\n", "b", invalid),
        ("\x0Ca", "b", invalid),
        ("a\x0C", "b", invalid),
        ("\ra", "b", invalid),
        ("a\r", "b", invalid),
        (" a", "b", invalid),
        ("a ", "b", invalid),
        ("a", "", syntax),
        ("a", " ", invalid),
        ("b", "\ta", invalid),
        ("b", "a\t", invalid),
        ("b", "\na", invalid),
        ("b", "a\n", invalid),
        ("b", "\x0Ca", invalid),
        ("b", "a\x0C", invalid),
        ("b", "\ra", invalid),
        ("b", "a\r", invalid),
        ("b", " a", invalid),
        ("b", "a ", invalid),
    ];
    for (e, desc) in class_list_elements(&mut doc) {
        for &(token, new, err) in cases {
            check_modification(
                &mut doc,
                e,
                desc,
                Op::Replace(token, new),
                None,
                Expect::Throws(err),
            );
        }
    }
}

#[test]
// Element-classlist.html: `checkReplace(null, " ", "", null, null, "SyntaxError")`. El paso 1
// de replace() (SyntaxError si cualquiera es vacío) va antes que el paso 2 (whitespace).
fn classlist_replace_vacio_antes_que_whitespace() {
    let mut doc = Document::new();
    for (e, desc) in class_list_elements(&mut doc) {
        check_modification(
            &mut doc,
            e,
            desc,
            Op::Replace(" ", ""),
            None,
            Expect::Throws(DomException::SyntaxError),
        );
    }
}

#[test]
// Element-classlist.html: checkReplace (casos válidos; whatwg/dom#443).
fn classlist_replace() {
    let mut doc = Document::new();
    // (antes, token, newToken, valor devuelto, después).
    type Case<'a> = (Option<&'a str>, &'a str, &'a str, bool, Option<&'a str>);
    let cases: &[Case<'_>] = &[
        (Some("a"), "a", "a", true, Some("a")),
        (Some("a"), "a", "b", true, Some("b")),
        (Some("a"), "A", "b", false, Some("a")),
        (Some("a b"), "b", "A", true, Some("a A")),
        (Some("a b"), "c", "a", false, Some("a b")),
        (Some("a b c"), "d", "e", false, Some("a b c")),
        (Some("a a a  b"), "a", "a", true, Some("a b")),
        (Some("a a a  b"), "c", "d", false, Some("a a a  b")),
        (None, "a", "b", false, None),
        (Some(""), "a", "b", false, Some("")),
        (Some(" "), "a", "b", false, Some(" ")),
        (Some(" a  \x0C"), "a", "b", true, Some("b")),
        (Some("a b c"), "b", "d", true, Some("a d c")),
        (Some("a b c"), "c", "a", true, Some("a b")),
        (Some("c b a"), "c", "a", true, Some("a b")),
        (Some("a b a"), "a", "c", true, Some("c b")),
        (Some("a b a"), "b", "c", true, Some("a c")),
        (Some("   a  a b"), "a", "c", true, Some("c b")),
        (Some("   a  a b"), "b", "c", true, Some("a c")),
        (Some(WS), "a", "c", true, Some("c b")),
        (Some(WS), "b", "c", true, Some("a c")),
        (Some("a null"), "null", "b", true, Some("a b")),
        (Some("a b"), "a", "null", true, Some("null b")),
        (Some("a undefined"), "undefined", "b", true, Some("a b")),
        (Some("a b"), "a", "undefined", true, Some("undefined b")),
    ];
    for (e, desc) in class_list_elements(&mut doc) {
        for &(before, token, new, res, after) in cases {
            check_modification(
                &mut doc,
                e,
                desc,
                Op::Replace(token, new),
                before,
                Expect::Ok(Some(res), after),
            );
        }
    }
}

// --- attributes.html (+ attributes.js y productions.js) ---

const INVALID_NAMES: &[&str] = &[""];
const VALID_NAMES: &[&str] = &[
    "x",
    "X",
    ":",
    "a:0",
    "invalid^Name",
    "\\",
    "'",
    "\"",
    "0",
    "0:a",
    ":a",
    "x:y:x",
    "~",
];
const INVALID_QNAMES: &[&str] = &["b:"];

/// `attr_is` de attributes.js.
fn attr_is(
    doc: &Document,
    attr: NodeId,
    v: &str,
    ln: &str,
    ns: Option<&str>,
    p: Option<&str>,
    n: &str,
) {
    assert_eq!(doc.attr_value(attr), v, "value");
    assert_eq!(
        doc.node_value(attr).map(|x| x.to_string()).as_deref(),
        Some(v),
        "nodeValue"
    );
    assert_eq!(
        doc.get_text_content(attr).map(|x| x.to_string()).as_deref(),
        Some(v),
        "textContent"
    );
    let NodeData::Attr(a) = &doc.get(attr).data else {
        panic!("no es un Attr");
    };
    assert_eq!(a.local_name, ln, "localName");
    assert_eq!(a.ns.as_ref().map(|x| x.uri()), ns, "namespaceURI");
    assert_eq!(
        a.prefix.as_ref().map(ToString::to_string).as_deref(),
        p,
        "prefix"
    );
    assert_eq!(doc.node_name(attr), n, "name/nodeName");
    assert_eq!(doc.node_type(attr), NodeType::Attribute);
}

/// `attributes_are` de attributes.js: (nombre, valor, namespace).
fn attributes_are(doc: &mut Document, e: NodeId, list: &[(&str, &str, Option<&str>)]) {
    for (i, &(name, value, ns)) in list.iter().enumerate() {
        let a = doc.attributes_item(e, i).expect("attributes[i]");
        attr_is(doc, a, value, name, ns, None, name);
        assert_eq!(
            doc.owner_element(a),
            Some(e),
            "attributes[{i}].ownerElement"
        );
    }
}

fn create_attr(doc: &mut Document, name: &str, value: &str) -> NodeId {
    let a = doc.create_attribute(&s(name)).expect("createAttribute");
    doc.set_attr_value(a, s(value));
    a
}

fn create_attr_ns(doc: &mut Document, ns: &str, qualified: &str, value: &str) -> NodeId {
    let a = doc
        .create_attribute_ns(Some(ns), &s(qualified))
        .expect("createAttributeNS");
    doc.set_attr_value(a, s(value));
    a
}

#[test]
// attributes.html: "When qualifiedName does not match the Name production, an
// INVALID_CHARACTER_ERR exception is to be thrown. (toggleAttribute)".
fn toggle_attribute_nombre_invalido() {
    let mut doc = Document::new();
    let e = el(&mut doc, "foo");
    for name in INVALID_NAMES {
        for force in [Some(true), None, Some(false)] {
            assert_eq!(
                doc.toggle_attribute(e, &s(name), force),
                Err(DomException::InvalidCharacterError),
                "toggleAttribute({name:?}, {force:?})"
            );
        }
    }
}

#[test]
// attributes.html: "toggleAttribute should lowercase its name argument (upper case attribute)"
// y "(mixed case attribute)".
fn toggle_attribute_pasa_a_minusculas() {
    let mut doc = Document::new();
    let e = el(&mut doc, "div");
    assert_eq!(doc.toggle_attribute(e, &s("ALIGN"), None), Ok(true));
    assert!(!has_attr_ns(&doc, e, Some(""), "ALIGN"));
    assert!(has_attr_ns(&doc, e, Some(""), "align"));
    assert!(has_attr(&doc, e, "align"));
    assert_eq!(doc.toggle_attribute(e, &s("ALIGN"), None), Ok(false));
    assert!(!has_attr_ns(&doc, e, Some(""), "ALIGN"));
    assert!(!has_attr_ns(&doc, e, Some(""), "align"));
    assert!(!has_attr(&doc, e, "align"));

    let e = el(&mut doc, "div");
    assert_eq!(doc.toggle_attribute(e, &s("CHEEseCaKe"), None), Ok(true));
    assert!(!has_attr_ns(&doc, e, Some(""), "CHEEseCaKe"));
    assert!(has_attr_ns(&doc, e, Some(""), "cheesecake"));
    assert!(has_attr(&doc, e, "cheesecake"));
}

#[test]
// attributes.html: "toggleAttribute should not throw even when qualifiedName starts with
// 'xmlns'" y "Basic functionality should be intact. (toggleAttribute)".
fn toggle_attribute_funcionalidad_basica() {
    let mut doc = Document::new();
    let e = el(&mut doc, "foo");
    for name in ["xmlns", "xmlns:a", "xmlnsx", "xmlns0"] {
        assert_eq!(doc.toggle_attribute(e, &s(name), None), Ok(true), "{name}");
        assert!(has_attr(&doc, e, name), "{name}");
    }
    let e = el(&mut doc, "foo");
    for name in VALID_NAMES {
        let n = s(name);
        assert_eq!(doc.toggle_attribute(e, &n, None), Ok(true), "{name}");
        assert!(doc.has_attribute(e, &n), "{name}");
        assert_eq!(doc.toggle_attribute(e, &n, None), Ok(false), "{name}");
        assert!(!doc.has_attribute(e, &n), "{name}");
        assert_eq!(doc.toggle_attribute(e, &n, Some(true)), Ok(true), "{name}");
        assert!(doc.has_attribute(e, &n), "{name}");
        assert_eq!(doc.toggle_attribute(e, &n, Some(true)), Ok(true), "{name}");
        assert!(doc.has_attribute(e, &n), "{name}");
        assert_eq!(
            doc.toggle_attribute(e, &n, Some(false)),
            Ok(false),
            "{name}"
        );
        assert!(!doc.has_attribute(e, &n), "{name}");
    }
}

#[test]
// attributes.html: "toggleAttribute should not change the order of previously set
// attributes.", "toggleAttribute should set the first attribute with the given name" y
// "toggleAttribute should set the attribute with the given qualified name".
fn toggle_attribute_orden_y_primer_atributo() {
    let mut doc = Document::new();
    let e = el(&mut doc, "foo");
    doc.toggle_attribute(e, &s("a"), None).unwrap();
    doc.toggle_attribute(e, &s("b"), None).unwrap();
    set_attr(&mut doc, e, "a", "thing");
    doc.toggle_attribute(e, &s("c"), None).unwrap();
    attributes_are(
        &mut doc,
        e,
        &[("a", "thing", None), ("b", "", None), ("c", "", None)],
    );

    let e = el(&mut doc, "baz");
    set_attr_ns(&mut doc, e, Some("ab"), "attr", "fail");
    set_attr_ns(&mut doc, e, Some("kl"), "attr", "pass");
    doc.toggle_attribute(e, &s("attr"), None).unwrap();
    attributes_are(&mut doc, e, &[("attr", "pass", Some("kl"))]);

    let e = el(&mut doc, "baz");
    set_attr_ns(&mut doc, e, Some("foo"), "foo:bar", "1");
    set_attr_ns(&mut doc, e, Some("foo"), "foo:bat", "2");
    assert_eq!(get_attr(&doc, e, "foo:bar").as_deref(), Some("1"));
    assert_eq!(get_attr(&doc, e, "foo:bat").as_deref(), Some("2"));
    let a0 = doc.attributes_item(e, 0).unwrap();
    attr_is(&doc, a0, "1", "bar", Some("foo"), Some("foo"), "foo:bar");
    let a1 = doc.attributes_item(e, 1).unwrap();
    attr_is(&doc, a1, "2", "bat", Some("foo"), Some("foo"), "foo:bat");
    doc.toggle_attribute(e, &s("foo:bar"), None).unwrap();
    assert!(!has_attr(&doc, e, "foo:bar"));
    let a0 = doc.attributes_item(e, 0).unwrap();
    attr_is(&doc, a0, "2", "bat", Some("foo"), Some("foo"), "foo:bat");
}

#[test]
// attributes.html: "Toggling element with inline style should make inline style disappear"
// (adaptado: `el.style = ...` reenvía al atributo `style`).
fn toggle_attribute_style() {
    let mut doc = Document::new();
    let e = el(&mut doc, "foo");
    set_attr(&mut doc, e, "style", "color: red; background-color: green");
    assert_eq!(doc.toggle_attribute(e, &s("style"), None), Ok(false));
    assert!(!has_attr(&doc, e, "style"));
}

#[test]
// attributes.html: "When qualifiedName does not match the Name production, an
// INVALID_CHARACTER_ERR exception is to be thrown. (setAttribute)", "setAttribute should
// lowercase its name argument" (upper y mixed case) y "setAttribute should not throw even when
// qualifiedName starts with 'xmlns'".
fn set_attribute_validacion_y_minusculas() {
    let mut doc = Document::new();
    let e = el(&mut doc, "foo");
    for name in INVALID_NAMES {
        assert_eq!(
            doc.set_attribute(e, &s(name), s("test")),
            Err(DomException::InvalidCharacterError)
        );
    }
    let e = el(&mut doc, "div");
    set_attr(&mut doc, e, "ALIGN", "left");
    assert_eq!(get_attr_ns(&doc, e, Some(""), "ALIGN"), None);
    assert_eq!(
        get_attr_ns(&doc, e, Some(""), "align").as_deref(),
        Some("left")
    );
    assert_eq!(get_attr(&doc, e, "align").as_deref(), Some("left"));

    let e = el(&mut doc, "div");
    set_attr(&mut doc, e, "CHEEseCaKe", "tasty");
    assert_eq!(get_attr_ns(&doc, e, Some(""), "CHEEseCaKe"), None);
    assert_eq!(
        get_attr_ns(&doc, e, Some(""), "cheesecake").as_deref(),
        Some("tasty")
    );
    assert_eq!(get_attr(&doc, e, "cheesecake").as_deref(), Some("tasty"));

    let e = el(&mut doc, "foo");
    for name in ["xmlns", "xmlns:a", "xmlnsx", "xmlns0"] {
        assert_eq!(
            doc.set_attribute(e, &s(name), s("success")),
            Ok(()),
            "{name}"
        );
    }
}

#[test]
// attributes.html: "Basic functionality should be intact." (setAttribute) y "setAttribute
// should not change the order of previously set attributes.".
fn set_attribute_funcionalidad_basica_y_orden() {
    let mut doc = Document::new();
    let e = el(&mut doc, "foo");
    for name in VALID_NAMES {
        set_attr(&mut doc, e, name, "test");
        assert_eq!(get_attr(&doc, e, name).as_deref(), Some("test"), "{name}");
    }
    let e = el(&mut doc, "foo");
    set_attr(&mut doc, e, "a", "1");
    set_attr(&mut doc, e, "b", "2");
    set_attr(&mut doc, e, "a", "3");
    set_attr(&mut doc, e, "c", "4");
    attributes_are(
        &mut doc,
        e,
        &[("a", "3", None), ("b", "2", None), ("c", "4", None)],
    );
}

#[test]
// attributes.html: "setAttribute should set the first attribute with the given name" y
// "setAttribute should set the attribute with the given qualified name".
fn set_attribute_primer_atributo_y_qualified_name() {
    let mut doc = Document::new();
    let e = el(&mut doc, "baz");
    set_attr_ns(&mut doc, e, Some("ab"), "attr", "fail");
    set_attr_ns(&mut doc, e, Some("kl"), "attr", "pass");
    set_attr(&mut doc, e, "attr", "pass");
    attributes_are(
        &mut doc,
        e,
        &[("attr", "pass", Some("ab")), ("attr", "pass", Some("kl"))],
    );

    let e = el(&mut doc, "baz");
    set_attr_ns(&mut doc, e, Some("foo"), "foo:bar", "1");
    assert_eq!(get_attr(&doc, e, "foo:bar").as_deref(), Some("1"));
    let a = doc.attributes_item(e, 0).unwrap();
    attr_is(&doc, a, "1", "bar", Some("foo"), Some("foo"), "foo:bar");
    set_attr(&mut doc, e, "foo:bar", "2");
    assert_eq!(get_attr(&doc, e, "foo:bar").as_deref(), Some("2"));
    let a = doc.attributes_item(e, 0).unwrap();
    attr_is(&doc, a, "2", "bar", Some("foo"), Some("foo"), "foo:bar");
}

#[test]
// attributes.html: setAttributeNS pasos 1-4: "... Name production ... (setAttributeNS)",
// "... QName production ...", "null and the empty string should result in a null namespace."
// y "A namespace is required to use a prefix.".
fn set_attribute_ns_validacion() {
    let mut doc = Document::new();
    let e = el(&mut doc, "foo");
    for name in INVALID_NAMES.iter().chain(INVALID_QNAMES) {
        assert_eq!(
            doc.set_attribute_ns(e, Some("a"), &s(name), s("fail")),
            Err(DomException::InvalidCharacterError),
            "{name:?}"
        );
    }
    let e = el(&mut doc, "foo");
    set_attr_ns(&mut doc, e, None, "aa", "bb");
    set_attr_ns(&mut doc, e, Some(""), "xx", "bb");
    attributes_are(&mut doc, e, &[("aa", "bb", None), ("xx", "bb", None)]);

    let e = el(&mut doc, "foo");
    for ns in [Some(""), None] {
        assert_eq!(
            doc.set_attribute_ns(e, ns, &s("aa:bb"), s("fail")),
            Err(DomException::NamespaceError)
        );
    }
}

#[test]
// attributes.html: setAttributeNS pasos 5-7 ("The xml prefix should not be allowed for
// arbitrary namespaces", "XML-namespaced attributes don't need an xml prefix", "The xmlns
// prefix ...", "The xmlns qualified name ...", "xmlns should be allowed as local name", "The
// XMLNS namespace should require xmlns as prefix or qualified name", "xmlns should be allowed
// as prefix in the XMLNS namespace", "xmlns should be allowed as qualified name in the XMLNS
// namespace").
fn set_attribute_ns_prefijos_xml_y_xmlns() {
    let mut doc = Document::new();
    let ns_err = Err(DomException::NamespaceError);
    let e = el(&mut doc, "foo");
    assert_eq!(
        doc.set_attribute_ns(e, Some("a"), &s("xml:bb"), s("fail")),
        ns_err
    );

    let e = el(&mut doc, "foo");
    set_attr_ns(&mut doc, e, Some(XML_NS), "a:bb", "pass");
    assert_eq!(doc.attributes_length(e), 1);
    let a = doc.attributes_item(e, 0).unwrap();
    attr_is(&doc, a, "pass", "bb", Some(XML_NS), Some("a"), "a:bb");

    let e = el(&mut doc, "foo");
    assert_eq!(
        doc.set_attribute_ns(e, Some("a"), &s("xmlns:bb"), s("fail")),
        ns_err
    );
    assert_eq!(
        doc.set_attribute_ns(e, Some("a"), &s("xmlns"), s("fail")),
        ns_err
    );

    let e = el(&mut doc, "foo");
    set_attr_ns(&mut doc, e, Some("ns"), "a:xmlns", "pass");
    assert_eq!(doc.attributes_length(e), 1);
    let a = doc.attributes_item(e, 0).unwrap();
    attr_is(&doc, a, "pass", "xmlns", Some("ns"), Some("a"), "a:xmlns");

    let e = el(&mut doc, "foo");
    assert_eq!(
        doc.set_attribute_ns(e, Some(XMLNS_NS), &s("a:xmlns"), s("fail")),
        ns_err
    );
    assert_eq!(
        doc.set_attribute_ns(e, Some(XMLNS_NS), &s("b:foo"), s("fail")),
        ns_err
    );

    let e = el(&mut doc, "foo");
    set_attr_ns(&mut doc, e, Some(XMLNS_NS), "xmlns:a", "pass");
    assert_eq!(doc.attributes_length(e), 1);
    let a = doc.attributes_item(e, 0).unwrap();
    attr_is(
        &doc,
        a,
        "pass",
        "a",
        Some(XMLNS_NS),
        Some("xmlns"),
        "xmlns:a",
    );

    let e = el(&mut doc, "foo");
    set_attr_ns(&mut doc, e, Some(XMLNS_NS), "xmlns", "pass");
    assert_eq!(doc.attributes_length(e), 1);
    let a = doc.attributes_item(e, 0).unwrap();
    attr_is(&doc, a, "pass", "xmlns", Some(XMLNS_NS), None, "xmlns");
}

#[test]
// attributes.html: "Setting the same attribute with another prefix should not change the
// prefix".
fn set_attribute_ns_mismo_atributo_otro_prefijo() {
    let mut doc = Document::new();
    let e = el(&mut doc, "foo");
    set_attr_ns(&mut doc, e, Some("a"), "foo:bar", "X");
    assert_eq!(doc.attributes_length(e), 1);
    let a = doc.attributes_item(e, 0).unwrap();
    attr_is(&doc, a, "X", "bar", Some("a"), Some("foo"), "foo:bar");
    set_attr_ns(&mut doc, e, Some("a"), "quux:bar", "Y");
    assert_eq!(doc.attributes_length(e), 1);
    let a = doc.attributes_item(e, 0).unwrap();
    attr_is(&doc, a, "Y", "bar", Some("a"), Some("foo"), "foo:bar");
    doc.remove_attribute_ns(e, Some("a"), &s("bar"));
    assert_eq!(doc.attributes_length(e), 0);
}

#[test]
// attributes.html: "setAttribute should not throw even if a load is not allowed", "Attributes
// should work in document fragments." y "Attribute values should not be parsed.".
fn atributos_varios() {
    let mut doc = Document::new();
    let e = el(&mut doc, "iframe");
    set_attr(&mut doc, e, "src", "file:///home");
    assert_eq!(get_attr(&doc, e, "src").as_deref(), Some("file:///home"));

    let fragment = doc.create_document_fragment();
    let new_one = el(&mut doc, "newElement");
    set_attr(&mut doc, new_one, "newdomestic", "Yes");
    append(&mut doc, fragment, new_one);
    let domestic = doc.first_child(fragment).unwrap();
    let a = doc.attributes_item(domestic, 0).unwrap();
    attr_is(&doc, a, "Yes", "newdomestic", None, None, "newdomestic");

    let e = el(&mut doc, "foo");
    set_attr(&mut doc, e, "x", "y");
    let a = doc.attributes_item(e, 0).unwrap();
    doc.set_attr_value(a, s("Y&lt;"));
    attr_is(&doc, a, "Y&lt;", "x", None, None, "x");
    assert_eq!(get_attr(&doc, e, "x").as_deref(), Some("Y&lt;"));
}

#[test]
// attributes.html: "Unset attributes return null", "First set attribute is returned by
// getAttribute" y "Style attributes are not normalized".
fn get_attribute_y_has_attribute() {
    let mut doc = Document::new();
    let e = el(&mut doc, "div");
    assert!(!has_attr(&doc, e, "bar"));
    assert!(!has_attr_ns(&doc, e, None, "bar"));
    assert!(!has_attr_ns(&doc, e, Some(""), "bar"));
    assert_eq!(get_attr(&doc, e, "bar"), None);
    assert_eq!(get_attr_ns(&doc, e, None, "bar"), None);
    assert_eq!(get_attr_ns(&doc, e, Some(""), "bar"), None);

    let e = el(&mut doc, "div");
    set_attr_ns(&mut doc, e, Some("ab"), "attr", "t1");
    set_attr_ns(&mut doc, e, Some("kl"), "attr", "t2");
    assert!(has_attr(&doc, e, "attr"));
    assert!(has_attr_ns(&doc, e, Some("ab"), "attr"));
    assert!(has_attr_ns(&doc, e, Some("kl"), "attr"));
    assert_eq!(get_attr(&doc, e, "attr").as_deref(), Some("t1"));
    assert_eq!(
        get_attr_ns(&doc, e, Some("ab"), "attr").as_deref(),
        Some("t1")
    );
    assert_eq!(
        get_attr_ns(&doc, e, Some("kl"), "attr").as_deref(),
        Some("t2")
    );
    assert_eq!(get_attr_ns(&doc, e, None, "attr"), None);
    assert_eq!(get_attr_ns(&doc, e, Some(""), "attr"), None);

    let e = el(&mut doc, "div");
    set_attr(&mut doc, e, "style", "color:#fff;");
    assert!(has_attr(&doc, e, "style"));
    assert!(has_attr_ns(&doc, e, None, "style"));
    assert!(has_attr_ns(&doc, e, Some(""), "style"));
    assert_eq!(get_attr(&doc, e, "style").as_deref(), Some("color:#fff;"));
    assert_eq!(
        get_attr_ns(&doc, e, None, "style").as_deref(),
        Some("color:#fff;")
    );
    assert_eq!(
        get_attr_ns(&doc, e, Some(""), "style").as_deref(),
        Some("color:#fff;")
    );
}

#[test]
// attributes.html: "Only lowercase attributes are returned on HTML elements (upper case
// attribute)" y "(mixed case attribute)".
fn solo_atributos_en_minusculas_en_elementos_html() {
    let mut doc = Document::new();
    let e = el(&mut doc, "div");
    set_attr_ns(&mut doc, e, Some(""), "ALIGN", "left");
    assert!(!has_attr(&doc, e, "ALIGN"));
    assert!(!has_attr(&doc, e, "align"));
    assert!(has_attr_ns(&doc, e, None, "ALIGN"));
    assert!(!has_attr_ns(&doc, e, None, "align"));
    assert!(has_attr_ns(&doc, e, Some(""), "ALIGN"));
    assert!(!has_attr_ns(&doc, e, Some(""), "align"));
    assert_eq!(get_attr(&doc, e, "ALIGN"), None);
    assert_eq!(get_attr(&doc, e, "align"), None);
    assert_eq!(get_attr_ns(&doc, e, None, "ALIGN").as_deref(), Some("left"));
    assert_eq!(
        get_attr_ns(&doc, e, Some(""), "ALIGN").as_deref(),
        Some("left")
    );
    assert_eq!(get_attr_ns(&doc, e, None, "align"), None);
    assert_eq!(get_attr_ns(&doc, e, Some(""), "align"), None);
    doc.remove_attribute_ns(e, Some(""), &s("ALIGN"));
    assert!(!doc.has_attributes(e));

    let e = el(&mut doc, "div");
    set_attr_ns(&mut doc, e, Some(""), "CHEEseCaKe", "tasty");
    for name in ["CHEESECAKE", "CHEEseCaKe", "cheesecake"] {
        assert!(!has_attr(&doc, e, name), "{name}");
        assert_eq!(get_attr(&doc, e, name), None, "{name}");
    }
    for ns in [Some(""), None] {
        assert!(!has_attr_ns(&doc, e, ns, "CHEESECAKE"));
        assert!(has_attr_ns(&doc, e, ns, "CHEEseCaKe"));
        assert!(!has_attr_ns(&doc, e, ns, "cheesecake"));
        assert_eq!(get_attr_ns(&doc, e, ns, "CHEESECAKE"), None);
        assert_eq!(
            get_attr_ns(&doc, e, ns, "CHEEseCaKe").as_deref(),
            Some("tasty")
        );
        assert_eq!(get_attr_ns(&doc, e, ns, "cheesecake"), None);
    }
    doc.remove_attribute_ns(e, Some(""), &s("CHEEseCaKe"));
    assert!(!doc.has_attributes(e));
}

#[test]
// attributes.html: "First set attribute is returned with mapped attribute set first" y "...
// set later".
fn primer_atributo_con_atributo_mapeado() {
    let mut h = html_document();
    let doc = &mut h.doc;
    let e = el(doc, "div");
    append(doc, h.body, e);
    set_attr_ns(doc, e, Some(""), "align", "left");
    set_attr_ns(doc, e, Some("xx"), "align", "right");
    set_attr_ns(doc, e, Some(""), "foo", "left");
    set_attr_ns(doc, e, Some("xx"), "foo", "right");
    assert!(has_attr(doc, e, "align"));
    assert!(has_attr(doc, e, "foo"));
    assert!(has_attr_ns(doc, e, Some("xx"), "align"));
    assert!(has_attr_ns(doc, e, None, "foo"));
    assert_eq!(get_attr(doc, e, "align").as_deref(), Some("left"));
    assert_eq!(get_attr(doc, e, "foo").as_deref(), Some("left"));
    assert_eq!(
        get_attr_ns(doc, e, Some("xx"), "align").as_deref(),
        Some("right")
    );
    assert_eq!(get_attr_ns(doc, e, None, "foo").as_deref(), Some("left"));
    assert_eq!(
        get_attr_ns(doc, e, Some(""), "foo").as_deref(),
        Some("left")
    );
    for (ns, name) in [
        (Some(""), "align"),
        (Some("xx"), "align"),
        (Some(""), "foo"),
        (Some("xx"), "foo"),
    ] {
        doc.remove_attribute_ns(e, ns, &s(name));
    }
    assert!(!doc.has_attributes(e));
    doc.remove_child(h.body, e).unwrap();

    let e = el(doc, "div");
    set_attr_ns(doc, e, Some("xx"), "align", "right");
    set_attr_ns(doc, e, Some(""), "align", "left");
    set_attr_ns(doc, e, Some("xx"), "foo", "right");
    set_attr_ns(doc, e, Some(""), "foo", "left");
    assert!(has_attr(doc, e, "align"));
    assert!(has_attr(doc, e, "foo"));
    assert!(has_attr_ns(doc, e, Some("xx"), "align"));
    assert!(has_attr_ns(doc, e, None, "foo"));
    assert_eq!(get_attr(doc, e, "align").as_deref(), Some("right"));
    assert_eq!(get_attr(doc, e, "foo").as_deref(), Some("right"));
    assert_eq!(
        get_attr_ns(doc, e, Some("xx"), "align").as_deref(),
        Some("right")
    );
    assert_eq!(get_attr_ns(doc, e, None, "foo").as_deref(), Some("left"));
    assert_eq!(
        get_attr_ns(doc, e, Some(""), "foo").as_deref(),
        Some("left")
    );
}

#[test]
// attributes.html: "Non-HTML element with upper-case attribute".
fn elemento_no_html_con_atributo_en_mayusculas() {
    let mut doc = Document::new();
    let e = el_ns(&mut doc, Some("http://www.example.com"), "foo");
    set_attr(&mut doc, e, "A", "test");
    assert!(has_attr(&doc, e, "A"), "hasAttribute()");
    assert!(has_attr_ns(&doc, e, Some(""), "A"));
    assert!(has_attr_ns(&doc, e, None, "A"));
    assert!(!has_attr_ns(&doc, e, Some("foo"), "A"));
    assert_eq!(get_attr(&doc, e, "A").as_deref(), Some("test"));
    assert_eq!(get_attr_ns(&doc, e, Some(""), "A").as_deref(), Some("test"));
    assert_eq!(get_attr_ns(&doc, e, None, "A").as_deref(), Some("test"));
    assert_eq!(get_attr_ns(&doc, e, Some("foo"), "A"), None);
}

#[test]
// attributes.html: "Attribute with prefix in local name" y "Attribute loses its owner when
// removed".
fn atributo_con_prefijo_en_local_name_y_owner() {
    let mut doc = Document::new();
    let e = el(&mut doc, "div");
    set_attr(&mut doc, e, "pre:fix", "value 1");
    set_attr(&mut doc, e, "fix", "value 2");
    let prefixed = doc.attributes_item(e, 0).unwrap();
    attr_is(&doc, prefixed, "value 1", "pre:fix", None, None, "pre:fix");
    let unprefixed = doc.attributes_item(e, 1).unwrap();
    attr_is(&doc, unprefixed, "value 2", "fix", None, None, "fix");
    doc.remove_attribute_ns(e, None, &s("pre:fix"));
    assert_eq!(doc.attributes_item(e, 0), Some(unprefixed));

    let e = el(&mut doc, "div");
    set_attr(&mut doc, e, "foo", "bar");
    let a = doc.attributes_item(e, 0).unwrap();
    assert_eq!(doc.owner_element(a), Some(e));
    doc.remove_attribute(e, &s("foo"));
    assert_eq!(doc.owner_element(a), None);
}

#[test]
// attributes.html: "Basic functionality of getAttributeNode/getAttributeNodeNS".
fn get_attribute_node_basico() {
    let mut doc = Document::new();
    let e = el(&mut doc, "div");
    set_attr(&mut doc, e, "foo", "bar");
    let a = doc.attributes_item(e, 0).unwrap();
    assert_eq!(doc.get_attribute_node(e, &s("foo")), Some(a));
    assert_eq!(doc.get_attribute_node_ns(e, Some(""), &s("foo")), Some(a));
    set_attr_ns(&mut doc, e, Some("x"), "foo2", "bar2");
    let a2 = doc.attributes_item(e, 1).unwrap();
    assert_eq!(
        doc.get_attribute_node_ns(e, Some("x"), &s("foo2")),
        Some(a2)
    );
}

#[test]
// attributes.html: "Basic functionality of setAttributeNode" y "Basic functionality of
// setAttributeNodeNS".
fn set_attribute_node_basico() {
    let mut doc = Document::new();
    let e = el(&mut doc, "div");
    set_attr(&mut doc, e, "foo", "bar");
    let attr = doc.get_attribute_node(e, &s("foo")).unwrap();
    assert_eq!(
        doc.get_attribute_node_ns(e, Some(""), &s("foo")),
        Some(attr)
    );
    doc.remove_attribute(e, &s("foo"));
    let e2 = el(&mut doc, "div");
    doc.set_attribute_node(e2, attr).unwrap();
    assert_eq!(doc.get_attribute_node(e2, &s("foo")), Some(attr));
    assert_eq!(doc.attributes_item(e2, 0), Some(attr));
    assert_eq!(doc.owner_element(attr), Some(e2));
    assert_eq!(doc.attr_value(attr), "bar");

    let e3 = el(&mut doc, "div");
    doc.remove_attribute(e2, &s("foo"));
    set_attr(&mut doc, e3, "foo", "baz");
    doc.set_attribute_node(e3, attr).unwrap();
    assert_eq!(get_attr(&doc, e3, "foo").as_deref(), Some("bar"));

    // setAttributeNodeNS.
    let e = el(&mut doc, "div");
    set_attr_ns(&mut doc, e, Some("x"), "foo", "bar");
    let attr = doc.get_attribute_node_ns(e, Some("x"), &s("foo")).unwrap();
    doc.remove_attribute(e, &s("foo"));
    let e2 = el(&mut doc, "div");
    set_attr_ns(&mut doc, e2, Some("x"), "foo", "baz");
    doc.set_attribute_node(e2, attr).unwrap();
    assert_eq!(
        get_attr_ns(&doc, e2, Some("x"), "foo").as_deref(),
        Some("bar")
    );
}

#[test]
// attributes.html: "setAttributeNode should distinguish attributes with same local name and
// different namespaces" y "setAttributeNode doesn't have case-insensitivity even with an
// HTMLElement" 1, 2 y 3.
fn set_attribute_node_namespaces_y_mayusculas() {
    let mut doc = Document::new();
    let e = el(&mut doc, "div");
    let a1 = create_attr_ns(&mut doc, "ns1", "p1:name", "value1");
    let a2 = create_attr_ns(&mut doc, "ns2", "p2:name", "value2");
    doc.set_attribute_node(e, a1).unwrap();
    doc.set_attribute_node(e, a2).unwrap();
    let n1 = doc
        .get_attribute_node_ns(e, Some("ns1"), &s("name"))
        .unwrap();
    assert_eq!(doc.attr_value(n1), "value1");
    let n2 = doc
        .get_attribute_node_ns(e, Some("ns2"), &s("name"))
        .unwrap();
    assert_eq!(doc.attr_value(n2), "value2");

    let e = el(&mut doc, "div");
    let a1 = create_attr_ns(&mut doc, "ns1", "p1:name", "value1");
    let a2 = create_attr_ns(&mut doc, "ns1", "p1:NAME", "VALUE2");
    doc.set_attribute_node(e, a1).unwrap();
    doc.set_attribute_node(e, a2).unwrap();
    let n1 = doc
        .get_attribute_node_ns(e, Some("ns1"), &s("name"))
        .unwrap();
    assert_eq!(doc.attr_value(n1), "value1");
    let n2 = doc
        .get_attribute_node_ns(e, Some("ns1"), &s("NAME"))
        .unwrap();
    assert_eq!(doc.attr_value(n2), "VALUE2");

    let e = el(&mut doc, "div");
    let a1 = create_attr_ns(&mut doc, "ns1", "FOOBAR", "");
    let a2 = create_attr_ns(&mut doc, "ns1", "FOOBAR", "");
    assert_eq!(doc.set_attribute_node(e, a1), Ok(None));
    assert_eq!(doc.owner_element(a1), Some(e));
    assert_eq!(doc.owner_element(a2), None);
    assert_eq!(doc.set_attribute_node(e, a2), Ok(Some(a1)));
    assert_eq!(doc.owner_element(a1), None);
    assert_eq!(doc.owner_element(a2), Some(e));

    let e = el(&mut doc, "div");
    let a1 = create_attr_ns(&mut doc, "ns1", "foobar", "");
    let a2 = create_attr_ns(&mut doc, "ns1", "FOOBAR", "");
    assert_eq!(doc.set_attribute_node(e, a1), Ok(None));
    assert_eq!(doc.owner_element(a1), Some(e));
    assert_eq!(doc.owner_element(a2), None);
    assert_eq!(doc.set_attribute_node(e, a2), Ok(None));
    assert_eq!(doc.owner_element(a1), Some(e));
    assert_eq!(doc.owner_element(a2), Some(e));
}

#[test]
// attributes.html: "If attr’s element is neither null nor element, throw an
// InUseAttributeError.", "setAttributeNode on bound attribute should throw
// InUseAttributeError" y "Replacing an attr by itself".
fn set_attribute_node_in_use_y_por_si_mismo() {
    let mut doc = Document::new();
    let e = el(&mut doc, "div");
    let other = el(&mut doc, "div");
    let attr = doc.create_attribute(&s("foo")).unwrap();
    assert_eq!(doc.set_attribute_node(e, attr), Ok(None));
    assert_eq!(doc.owner_element(attr), Some(e));
    assert_eq!(
        doc.set_attribute_node(other, attr),
        Err(DomException::InUseAttributeError)
    );

    let e = el(&mut doc, "div");
    set_attr(&mut doc, e, "foo", "bar");
    let attr = doc.get_attribute_node(e, &s("foo")).unwrap();
    let e2 = el(&mut doc, "div");
    assert_eq!(
        doc.set_attribute_node(e2, attr),
        Err(DomException::InUseAttributeError)
    );

    let e = el(&mut doc, "div");
    let attr = doc.create_attribute(&s("foo")).unwrap();
    assert_eq!(doc.set_attribute_node(e, attr), Ok(None));
    set_attr(&mut doc, e, "bar", "qux");
    assert_eq!(doc.set_attribute_node(e, attr), Ok(Some(attr)));
    assert_eq!(doc.attributes_item(e, 0), Some(attr));
}

#[test]
// attributes.html: "Basic functionality of removeAttributeNode".
fn remove_attribute_node_basico() {
    let mut doc = Document::new();
    let e = el(&mut doc, "div");
    set_attr(&mut doc, e, "foo", "bar");
    let attr = doc.get_attribute_node(e, &s("foo")).unwrap();
    assert_eq!(doc.remove_attribute_node(e, attr), Ok(attr));
    let e2 = el(&mut doc, "div");
    doc.set_attribute_node(e2, attr).unwrap();
    assert_eq!(doc.attributes_item(e2, 0), Some(attr));
    assert_eq!(doc.attributes_length(e), 0);
    // Quitar un Attr que ya no es del elemento lanza NotFoundError.
    assert_eq!(
        doc.remove_attribute_node(e, attr),
        Err(DomException::NotFoundError)
    );
}

#[test]
// attributes.html: "setAttributeNode, if it fires mutation events, should fire one with the new
// node when resetting an existing attribute (outer shell)" y "setAttributeNode called with an
// Attr that has the same name as an existing one should not change attribute order".
fn set_attribute_node_reemplaza_sin_cambiar_orden() {
    let mut doc = Document::new();
    let e = el(&mut doc, "div");
    let n1 = create_attr(&mut doc, "foo", "bar");
    doc.set_attribute_node(e, n1).unwrap();
    let n2 = create_attr(&mut doc, "foo", "baz");
    assert_eq!(
        doc.set_attribute_node(e, n2),
        Ok(Some(n1)),
        "Must return the old attr node from a setAttributeNode call"
    );

    let e = el(&mut doc, "div");
    set_attr(&mut doc, e, "a", "b");
    set_attr(&mut doc, e, "c", "d");
    let names_values = |doc: &mut Document| {
        (0..doc.attributes_length(e))
            .map(|i| {
                let a = doc.attributes_item(e, i).unwrap();
                (doc.node_name(a).to_string(), doc.attr_value(a).to_string())
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names_values(&mut doc),
        [("a".into(), "b".into()), ("c".into(), "d".into())]
    );
    let attr = create_attr(&mut doc, "a", "e");
    doc.set_attribute_node(e, attr).unwrap();
    assert_eq!(
        names_values(&mut doc),
        [("a".into(), "e".into()), ("c".into(), "d".into())]
    );
}

#[test]
// attributes.html: "getAttributeNames tests".
fn get_attribute_names() {
    let mut doc = Document::new();
    let e = el(&mut doc, "div");
    let names = |doc: &Document| -> Vec<String> {
        doc.get_attribute_names(e)
            .iter()
            .map(ToString::to_string)
            .collect()
    };
    let attr_names = |doc: &mut Document| -> Vec<String> {
        (0..doc.attributes_length(e))
            .map(|i| {
                let a = doc.attributes_item(e, i).unwrap();
                doc.node_name(a).to_string()
            })
            .collect()
    };
    set_attr(&mut doc, e, "foo", "bar");
    assert_eq!(names(&doc), ["foo"]);
    assert_eq!(names(&doc), attr_names(&mut doc));
    doc.remove_attribute(e, &s("foo"));
    assert!(names(&doc).is_empty());

    set_attr(&mut doc, e, "foo", "bar");
    set_attr_ns(&mut doc, e, Some(""), "FOO", "bar");
    set_attr_ns(&mut doc, e, Some("dummy1"), "foo", "bar");
    set_attr_ns(&mut doc, e, Some("dummy2"), "dummy:foo", "bar");
    assert_eq!(names(&doc), ["foo", "FOO", "foo", "dummy:foo"]);
    assert_eq!(names(&doc), attr_names(&mut doc));

    doc.remove_attribute_ns(e, Some(""), &s("FOO"));
    assert_eq!(names(&doc), ["foo", "foo", "dummy:foo"]);
    assert_eq!(names(&doc), attr_names(&mut doc));
}

#[test]
// attributes-namednodemap.html: "setNamedItem and removeNamedItem on `attributes` should add
// and remove fields from `attributes`" (más NotFoundError al quitar uno que no está).
fn named_node_map_set_y_remove_named_item() {
    let mut doc = Document::new();
    let e = el(&mut doc, "div");
    assert_eq!(doc.attributes_length(e), 0);
    let a1 = doc.create_attribute(&s("attr1")).unwrap();
    doc.set_attribute_node(e, a1).unwrap();
    assert_eq!(doc.get_attribute_node(e, &s("attr1")), Some(a1));
    assert_eq!(doc.attributes_length(e), 1);
    let a2 = doc.create_attribute(&s("attr2")).unwrap();
    doc.set_attribute_node(e, a2).unwrap();
    assert_eq!(doc.get_attribute_node(e, &s("attr2")), Some(a2));
    assert_eq!(doc.attributes_length(e), 2);
    assert_eq!(doc.remove_named_item(e, &s("attr1")), Ok(a1));
    assert_eq!(doc.attributes_length(e), 1);
    assert_eq!(doc.remove_named_item(e, &s("attr2")), Ok(a2));
    assert_eq!(doc.attributes_length(e), 0);
    assert_eq!(
        doc.remove_named_item(e, &s("attr2")),
        Err(DomException::NotFoundError)
    );
}

// --- Element-setAttribute.html, Element-removeAttribute.html, Element-hasAttributes.html ---

#[test]
// Element-setAttribute.html: "setAttribute should change the first attribute, irrespective of
// namespace" y "setAttribute should lowercase before setting".
fn element_set_attribute() {
    let mut doc = Document::new();
    let e = el(&mut doc, "p");
    set_attr_ns(&mut doc, e, Some("foo"), "x", "first");
    set_attr_ns(&mut doc, e, Some("foo2"), "x", "second");
    set_attr(&mut doc, e, "x", "changed");
    assert_eq!(doc.attributes_length(e), 2);
    assert_eq!(get_attr(&doc, e, "x").as_deref(), Some("changed"));
    assert_eq!(
        get_attr_ns(&doc, e, Some("foo"), "x").as_deref(),
        Some("changed")
    );
    assert_eq!(
        get_attr_ns(&doc, e, Some("foo2"), "x").as_deref(),
        Some("second")
    );

    let e = el(&mut doc, "p");
    set_attr(&mut doc, e, "FOO", "bar");
    assert_eq!(get_attr(&doc, e, "foo").as_deref(), Some("bar"));
    assert_eq!(get_attr(&doc, e, "FOO").as_deref(), Some("bar"));
    assert_eq!(
        get_attr_ns(&doc, e, Some(""), "foo").as_deref(),
        Some("bar")
    );
    assert_eq!(get_attr_ns(&doc, e, Some(""), "FOO"), None);
}

#[test]
// Element-removeAttribute.html: "removeAttribute should remove the first attribute,
// irrespective of namespace, when the first attribute is not in a namespace" y "... is in a
// namespace".
fn element_remove_attribute() {
    let mut doc = Document::new();
    let e = el(&mut doc, "p");
    set_attr(&mut doc, e, "x", "first");
    set_attr_ns(&mut doc, e, Some("foo"), "x", "second");
    assert_eq!(doc.attributes_length(e), 2);
    assert_eq!(get_attr(&doc, e, "x").as_deref(), Some("first"));
    assert_eq!(get_attr_ns(&doc, e, None, "x").as_deref(), Some("first"));
    assert_eq!(
        get_attr_ns(&doc, e, Some("foo"), "x").as_deref(),
        Some("second")
    );
    doc.remove_attribute(e, &s("x"));
    assert_eq!(get_attr(&doc, e, "x").as_deref(), Some("second"));
    assert_eq!(get_attr_ns(&doc, e, None, "x"), None);
    assert_eq!(
        get_attr_ns(&doc, e, Some("foo"), "x").as_deref(),
        Some("second")
    );
    assert_eq!(doc.attributes_length(e), 1, "one attribute");

    let e = el(&mut doc, "p");
    set_attr_ns(&mut doc, e, Some("foo"), "x", "first");
    set_attr_ns(&mut doc, e, Some("foo2"), "x", "second");
    assert_eq!(doc.attributes_length(e), 2);
    assert_eq!(get_attr(&doc, e, "x").as_deref(), Some("first"));
    assert_eq!(
        get_attr_ns(&doc, e, Some("foo"), "x").as_deref(),
        Some("first")
    );
    assert_eq!(
        get_attr_ns(&doc, e, Some("foo2"), "x").as_deref(),
        Some("second")
    );
    doc.remove_attribute(e, &s("x"));
    assert_eq!(get_attr(&doc, e, "x").as_deref(), Some("second"));
    assert_eq!(get_attr_ns(&doc, e, Some("foo"), "x"), None);
    assert_eq!(
        get_attr_ns(&doc, e, Some("foo2"), "x").as_deref(),
        Some("second")
    );
    assert_eq!(doc.attributes_length(e), 1, "one attribute");
}

#[test]
// Element-hasAttributes.html: "element.hasAttributes() must return false when the element does
// not have attribute." y "... must return true when the element has attribute.".
fn element_has_attributes() {
    let mut h = html_document();
    let doc = &mut h.doc;
    let button = el(doc, "button");
    append(doc, h.body, button);
    let div_foo = el(doc, "div");
    set_attr(doc, div_foo, "id", "foo");
    append(doc, h.body, div_foo);
    let p = el(doc, "p");
    set_attr(doc, p, "data-foo", "");
    append(doc, h.body, p);

    assert!(!doc.has_attributes(button));
    let empty_div = el(doc, "div");
    assert!(!doc.has_attributes(empty_div));

    assert!(doc.has_attributes(div_foo));
    let div_class = el(doc, "div");
    set_attr(doc, div_class, "class", "foo");
    assert!(doc.has_attributes(div_class));
    assert!(doc.has_attributes(p));
    let div_custom = el(doc, "div");
    set_attr(doc, div_custom, "data-custom", "foo");
    assert!(doc.has_attributes(div_custom));
}

// --- getElementsByClassName ---

/// `getElementsByClassName(classNames)` como lista de nodos.
fn by_class(doc: &Document, root: NodeId, names: &str) -> Vec<NodeId> {
    doc.get_elements_by_class_name(root, &s(names)).items(doc)
}

/// El documento de los getElementsByClassName-NN.htm: `<html class>` y `<body class>` con un
/// `div#log`.
fn class_doc(html_class: Option<&str>, body_class: Option<&str>) -> (Document, NodeId, NodeId) {
    let mut doc = Document::new();
    let root = doc.root();
    let html = el(&mut doc, "html");
    append(&mut doc, root, html);
    if let Some(c) = html_class {
        set_attr(&mut doc, html, "class", c);
    }
    let head = el(&mut doc, "head");
    append(&mut doc, html, head);
    let body = el(&mut doc, "body");
    append(&mut doc, html, body);
    if let Some(c) = body_class {
        set_attr(&mut doc, body, "class", c);
    }
    let log = el(&mut doc, "div");
    set_attr(&mut doc, log, "id", "log");
    append(&mut doc, body, log);
    (doc, html, body)
}

#[test]
// Element-getElementsByClassName.html: "getElementsByClassName should work on disconnected
// subtrees." y "getElementsByClassName() should be a live collection".
fn element_get_elements_by_class_name() {
    let mut doc = Document::new();
    let a = el(&mut doc, "a");
    let b = el(&mut doc, "b");
    doc.set_class_name(b, s("foo"));
    append(&mut doc, a, b);
    assert_eq!(by_class(&doc, a, "foo"), [b]);

    let mut h = html_document();
    let doc = &mut h.doc;
    let a = el(doc, "a");
    let b = el(doc, "b");
    let c = el(doc, "c");
    doc.set_class_name(b, s("foo"));
    append(doc, h.body, a);
    append(doc, a, b);
    let l = doc.get_elements_by_class_name(a, &s("foo"));
    assert_eq!(l.length(doc), 1);
    doc.set_class_name(c, s("foo"));
    append(doc, a, c);
    assert_eq!(l.length(doc), 2);
    doc.remove_child(a, c).unwrap();
    assert_eq!(l.length(doc), 1);
}

#[test]
// getElementsByClassName-01.htm (simple), -02.htm (con salto de línea), -07.htm, -08.htm,
// -15.htm, -16.htm, -17.htm y -18.htm (los arrays de JS se convierten a string con comas).
fn get_elements_by_class_name_espacios_y_arrays() {
    // 01.
    let (doc, html, body) = class_doc(Some("a"), Some("a"));
    assert_eq!(by_class(&doc, doc.root(), "\ta\n"), [html, body]);
    // 02 y 15 (["a\n"] → "a\n").
    let (doc, html, body) = class_doc(Some("a\nb"), Some("a\n"));
    assert_eq!(by_class(&doc, doc.root(), "a\n"), [html, body]);
    // 07.
    let (doc, _, body) = class_doc(None, Some("a b"));
    assert_eq!(by_class(&doc, doc.root(), "b\t\x0C\n\na\rb"), [body]);
    // 08.
    let (doc, _, body) = class_doc(None, Some("a"));
    assert_eq!(by_class(&doc, doc.root(), "a\x0Ca"), [body]);
    // 16 (["b", "a"] → "b,a").
    let (doc, _, body) = class_doc(Some("a\nb"), Some("b,a"));
    assert_eq!(by_class(&doc, doc.root(), "b,a"), [body]);
    // 17 (["b a"]).
    let (doc, _, body) = class_doc(None, Some("a b"));
    assert_eq!(by_class(&doc, doc.root(), "b a"), [body]);
    // 18 (["\fa", "b\n"] → "\fa,b\n", desde documentElement).
    let (mut doc, html, body) = class_doc(Some("a,b"), Some("a,b x"));
    let p = el(&mut doc, "p");
    set_attr(&mut doc, p, "class", "a,bx");
    append(&mut doc, body, p);
    let script = el(&mut doc, "script");
    set_attr(&mut doc, script, "class", "xa,b");
    append(&mut doc, body, script);
    assert_eq!(by_class(&doc, html, "\x0Ca,b\n"), [body]);
}

#[test]
// getElementsByClassName-03.htm a -06.htm: la colección es viva (cambios de class, quitar el
// atributo y agregar elementos).
fn get_elements_by_class_name_coleccion_viva() {
    // 03.
    let (mut doc, html, body) = class_doc(Some("a"), Some("a"));
    let collection = doc.get_elements_by_class_name(doc.root(), &s("a"));
    doc.set_class_name(body, s("b"));
    assert_eq!(collection.items(&doc), [html]);
    // 04.
    let (mut doc, html, body) = class_doc(Some("a"), Some("a"));
    let collection = doc.get_elements_by_class_name(doc.root(), &s("a"));
    let mut class = doc.class_name(body);
    class.push_str("\tb");
    doc.set_class_name(body, class);
    assert_eq!(collection.items(&doc), [html, body]);
    // 05.
    let (mut doc, html, body) = class_doc(Some("a"), Some("a"));
    let collection = doc.get_elements_by_class_name(doc.root(), &s("a"));
    doc.remove_attribute(body, &s("class"));
    assert_eq!(collection.items(&doc), [html]);
    // 06.
    let (mut doc, _, body) = class_doc(None, Some("a"));
    let collection = doc.get_elements_by_class_name(doc.root(), &s("a"));
    let e = el(&mut doc, "foo");
    set_attr(&mut doc, e, "class", "a");
    append(&mut doc, body, e);
    assert_eq!(collection.items(&doc), [body, e]);
}

#[test]
// getElementsByClassName-09.htm (case sensitive), -12.htm y -13.htm (desde body: el propio
// elemento no cuenta) y -24.htm ("handle unicode chars").
fn get_elements_by_class_name_mayusculas_raiz_y_unicode() {
    // 09.
    let (doc, html, _) = class_doc(Some("a A"), Some("a a"));
    assert_eq!(by_class(&doc, doc.root(), "A a"), [html]);
    // 12.
    let (doc, _, body) = class_doc(Some("a"), Some("a"));
    assert!(by_class(&doc, body, "a").is_empty());
    // 13.
    let (mut doc, _, body) = class_doc(Some("a"), Some("a"));
    let collection = doc.get_elements_by_class_name(body, &s("a"));
    let e = el(&mut doc, "x-y-z");
    doc.set_class_name(e, s("a"));
    append(&mut doc, body, e);
    assert_eq!(collection.items(&doc), [e]);
    // 24.
    let (mut doc, _, body) = class_doc(None, None);
    let div = el(&mut doc, "div");
    append(&mut doc, body, div);
    let inner = el(&mut doc, "div");
    append(&mut doc, div, inner);
    let a = el(&mut doc, "a");
    set_attr(&mut doc, a, "class", "ΔЙあ叶葉 말 link");
    append(&mut doc, inner, a);
    let b = el(&mut doc, "b");
    set_attr(&mut doc, b, "class", "text");
    append(&mut doc, div, b);
    let d2 = el(&mut doc, "div");
    set_attr(&mut doc, d2, "class", "ΔЙあ叶葉 קم");
    append(&mut doc, body, d2);
    let collection = by_class(&doc, doc.root(), "ΔЙあ叶葉");
    assert_eq!(collection.len(), 2);
    assert_eq!(doc.node_name(doc.parent(collection[0]).unwrap()), "DIV");
    assert_eq!(doc.node_name(doc.parent(collection[1]).unwrap()), "BODY");
}

#[test]
// getElementsByClassName-14.htm: en quirks mode la comparación es ASCII case-insensitive
// ("A a"), pero "Unicode-case should be sensitive even in quirks mode.".
fn get_elements_by_class_name_quirks() {
    let (mut doc, html, body) = class_doc(Some("a A"), Some("a a"));
    doc.mode = QuirksMode::Quirks;
    let mut divs = Vec::new();
    for class in ["k", "K", "\u{212a}"] {
        let d = el(&mut doc, "div");
        set_attr(&mut doc, d, "class", class);
        append(&mut doc, body, d);
        divs.push(d);
    }
    assert_eq!(by_class(&doc, doc.root(), "A a"), [html, body]);
    assert_eq!(by_class(&doc, doc.root(), "\u{212a}"), [divs[2]]);
}

#[test]
// getElementsByClassName-empty-set.html: "Passing an empty string ...", "... a space ..." y
// "... three spaces to getElementsByClassName should return an empty HTMLCollection".
fn get_elements_by_class_name_conjunto_vacio() {
    let (mut doc, _, body) = class_doc(None, None);
    let span = el(&mut doc, "span");
    set_attr(&mut doc, span, "class", " ");
    append(&mut doc, body, span);
    for names in ["", " ", "   "] {
        assert!(by_class(&doc, doc.root(), names).is_empty(), "{names:?}");
    }
}

#[test]
// getElementsByClassName-whitespace-class-names.html: "Passing a <espacio no ASCII> to
// getElementsByClassName still finds the span" (para cada carácter).
fn get_elements_by_class_name_espacios_no_ascii() {
    let chars = [
        '\u{000B}', '\u{0085}', '\u{00A0}', '\u{1680}', '\u{2000}', '\u{2001}', '\u{2002}',
        '\u{2003}', '\u{2004}', '\u{2005}', '\u{2006}', '\u{2007}', '\u{2008}', '\u{2009}',
        '\u{200A}', '\u{2028}', '\u{2029}', '\u{202F}', '\u{205F}', '\u{3000}', '\u{180E}',
        '\u{200B}', '\u{200C}', '\u{200D}', '\u{2060}', '\u{FEFF}',
    ];
    let (mut doc, _, body) = class_doc(None, None);
    let mut spans = Vec::new();
    for c in chars {
        let span = el(&mut doc, "span");
        doc.set_attribute(span, &s("class"), DomString::from(c))
            .unwrap();
        append(&mut doc, body, span);
        spans.push(span);
    }
    for (c, span) in chars.into_iter().zip(spans) {
        let class = doc.get_attribute(span, &s("class")).unwrap();
        assert_eq!(class.len_utf16(), 1, "U+{:04X}", u32::from(c));
        assert_eq!(
            doc.get_elements_by_class_name(doc.root(), &class)
                .items(&doc),
            [span],
            "U+{:04X}",
            u32::from(c)
        );
    }
}

// --- Element-children.html y traversal por elementos ---

#[test]
// Element-children.html: `children.item("foo")` (→ item(0)) y namedItem con ids/names de
// elementos HTML y no HTML.
fn element_children() {
    let mut doc = Document::new();
    let container = el(&mut doc, "div");
    set_attr(&mut doc, container, "id", "test");
    let img0 = el(&mut doc, "img");
    append(&mut doc, container, img0);
    for (attr, value) in [("id", "foo"), ("id", "foo"), ("name", "bar")] {
        let img = el(&mut doc, "img");
        set_attr(&mut doc, img, attr, value);
        append(&mut doc, container, img);
    }
    let child = el_ns(&mut doc, Some(""), "img");
    set_attr(&mut doc, child, "id", "baz");
    append(&mut doc, container, child);
    let child = el_ns(&mut doc, Some(""), "img");
    set_attr(&mut doc, child, "name", "qux");
    append(&mut doc, container, child);

    let list = doc.children_collection(container);
    let result = list.item(&doc, 0).unwrap();
    assert_eq!(doc.node_type(result), NodeType::Element);
    assert!(
        !has_attr(&doc, result, "id"),
        "Expected the IDless Element."
    );

    assert_eq!(list.length(&doc), 6);
    for (name, index) in [("foo", 1), ("bar", 3), ("baz", 4)] {
        assert_eq!(
            list.named_item(&doc, &s(name)),
            list.item(&doc, index),
            "{name}"
        );
        assert!(list.named_item(&doc, &s(name)).is_some(), "{name}");
    }
    assert_eq!(list.named_item(&doc, &s("qux")), None);
}

/// `<p id="parentEl">` con hijos de texto y `span`s; devuelve (p, spans).
fn parent_el(doc: &mut Document, parts: &[Option<&str>]) -> (NodeId, Vec<NodeId>) {
    let p = el(doc, "p");
    set_attr(doc, p, "id", "parentEl");
    let mut spans = Vec::new();
    for part in parts {
        match part {
            Some(t) => {
                let n = text(doc, t);
                append(doc, p, n);
            }
            None => {
                let span = el(doc, "span");
                append(doc, p, span);
                spans.push(span);
            }
        }
    }
    (p, spans)
}

#[test]
// Element-firstElementChild.html y Element-lastElementChild.html.
fn element_first_y_last_element_child() {
    let mut doc = Document::new();
    let (p, spans) = parent_el(&mut doc, &[Some("The result of this test is\n"), None]);
    set_attr(&mut doc, spans[0], "id", "first_element_child");
    let fec = doc.first_element_child(p).unwrap();
    assert_eq!(doc.node_type(fec), NodeType::Element);
    assert_eq!(
        get_attr(&doc, fec, "id").as_deref(),
        Some("first_element_child")
    );

    let (p, spans) = parent_el(
        &mut doc,
        &[
            Some("The result of "),
            None,
            Some(" is "),
            None,
            Some(" above."),
        ],
    );
    set_attr(&mut doc, spans[0], "id", "first_element_child");
    set_attr(&mut doc, spans[1], "id", "last_element_child");
    let lec = doc.last_element_child(p).unwrap();
    assert_eq!(doc.node_type(lec), NodeType::Element);
    assert_eq!(
        get_attr(&doc, lec, "id").as_deref(),
        Some("last_element_child")
    );
}

#[test]
// Element-nextElementSibling.html, Element-previousElementSibling.html y
// Element-siblingElement-null.html.
fn element_next_y_previous_element_sibling() {
    let mut doc = Document::new();
    let (_, spans) = parent_el(
        &mut doc,
        &[
            Some("The result of "),
            None,
            Some(" is "),
            None,
            Some(" above."),
        ],
    );
    let nes = doc.next_element_sibling(spans[0]).unwrap();
    assert_eq!(doc.node_type(nes), NodeType::Element);
    assert_eq!(nes, spans[1]);

    let (_, spans) = parent_el(
        &mut doc,
        &[
            Some("The result of "),
            None,
            Some(" is\n"),
            None,
            Some("\n\n\n"),
            None,
            Some(" "),
        ],
    );
    let pes = doc.previous_element_sibling(spans[2]).unwrap();
    assert_eq!(doc.node_type(pes), NodeType::Element);
    assert_eq!(pes, spans[1]);

    let (_, spans) = parent_el(&mut doc, &[Some("The result of this test is "), None]);
    assert_eq!(doc.previous_element_sibling(spans[0]), None);
    assert_eq!(doc.next_element_sibling(spans[0]), None);
}

#[test]
// Element-childElementCount.html, -nochild.html, -dynamic-add.html y -dynamic-remove.html.
fn element_child_element_count() {
    let mut doc = Document::new();
    let (p, spans) = parent_el(
        &mut doc,
        &[
            Some("The result of "),
            None,
            Some(" is\n"),
            None,
            Some("\n"),
            None,
            Some(" "),
        ],
    );
    // El primer span tiene sus propios hijos span, que no cuentan.
    for _ in 0..2 {
        let inner = el(&mut doc, "span");
        append(&mut doc, spans[0], inner);
    }
    assert_eq!(doc.child_element_count(p), 3);

    let (p, _) = parent_el(&mut doc, &[Some("The result of this test is unknown.")]);
    assert_eq!(doc.child_element_count(p), 0);

    let (p, _) = parent_el(&mut doc, &[Some("The result of this test is\n"), None]);
    let new_child = el(&mut doc, "span");
    append(&mut doc, p, new_child);
    assert_eq!(doc.child_element_count(p), 2);

    let (p, _) = parent_el(
        &mut doc,
        &[Some("The result of this test is\n"), None, None],
    );
    let lec = doc.last_element_child(p).unwrap();
    doc.remove_child(p, lec).unwrap();
    assert_eq!(doc.child_element_count(p), 1);
}

// --- CharacterData ---

/// Los dos nodos de `testNode`: un `Text` y un `Comment` con "test".
fn char_nodes(doc: &mut Document) -> [(NodeId, &'static str); 2] {
    [
        (text(doc, "test"), "Text"),
        (comment(doc, "test"), "Comment"),
    ]
}

#[test]
// CharacterData-appendData.html: appendData('bar'), (''), (non-ASCII), (null), (undefined) y
// ('', 'bar').
fn character_data_append_data() {
    let cases: &[(&str, &str)] = &[
        ("bar", "testbar"),
        ("", "test"),
        (
            ", append more 資料，測試資料",
            "test, append more 資料，測試資料",
        ),
        ("null", "testnull"),
        ("undefined", "testundefined"),
    ];
    for &(arg, expected) in cases {
        let mut doc = Document::new();
        for (n, ty) in char_nodes(&mut doc) {
            assert_eq!(data(&doc, n), "test");
            doc.append_data(n, &s(arg));
            assert_eq!(data(&doc, n), expected, "{ty}.appendData({arg:?})");
        }
    }
    let mut doc = Document::new();
    for (n, _) in char_nodes(&mut doc) {
        doc.append_data(n, &s(", append more 資料，測試資料"));
        assert_eq!(doc.data_length(n), 25);
    }
}

#[test]
// CharacterData-deleteData.html: "out of bounds".
fn character_data_delete_data_fuera_de_rango() {
    let mut doc = Document::new();
    for (n, ty) in char_nodes(&mut doc) {
        for (offset, count) in [(5, 10), (5, 0), (-1, 10), (-1, 0)] {
            assert_eq!(
                doc.delete_data(n, u32_arg(offset), u32_arg(count)),
                Err(DomException::IndexSizeError),
                "{ty}.deleteData({offset}, {count})"
            );
        }
        assert_eq!(data(&doc, n), "test");
    }
}

#[test]
// CharacterData-deleteData.html: al inicio, al final, en el medio, count cero, count negativo
// chico y grande, datos no ASCII y fuera del BMP.
fn character_data_delete_data() {
    let cases: &[(i64, i64, &str)] = &[
        (0, 2, "st"),
        (2, 10, "te"),
        (1, 1, "tst"),
        (2, 0, "test"),
        (0, 0, "test"),
        (2, -1, "te"),
        (1, -0x1_0000_0000 + 2, "tt"),
    ];
    for &(offset, count, expected) in cases {
        let mut doc = Document::new();
        for (n, ty) in char_nodes(&mut doc) {
            doc.delete_data(n, u32_arg(offset), u32_arg(count)).unwrap();
            assert_eq!(
                data(&doc, n),
                expected,
                "{ty}.deleteData({offset}, {count})"
            );
        }
    }
    let mut doc = Document::new();
    for (n, ty) in char_nodes(&mut doc) {
        doc.set_data(
            n,
            &s("This is the character data test, append more 資料，更多測試資料"),
        );
        doc.delete_data(n, 40, 5).unwrap();
        assert_eq!(
            data(&doc, n),
            "This is the character data test, append 資料，更多測試資料",
            "{ty}"
        );
        doc.delete_data(n, 45, 2).unwrap();
        assert_eq!(
            data(&doc, n),
            "This is the character data test, append 資料，更多資料",
            "{ty}"
        );

        doc.set_data(n, &s("🌠 test 🌠 TEST"));
        doc.delete_data(n, 5, 8).unwrap();
        assert_eq!(data(&doc, n), "🌠 teST", "{ty} (code units UTF-16)");
    }
}

#[test]
// CharacterData-insertData.html: "out of bounds", "negative out of bounds", "negative in
// bounds", ('') , al inicio, en el medio, al final, no ASCII y fuera del BMP.
fn character_data_insert_data() {
    let mut doc = Document::new();
    for (n, ty) in char_nodes(&mut doc) {
        for (offset, arg) in [(5, "x"), (5, ""), (-1, "x"), (-0x1_0000_0000 + 5, "x")] {
            assert_eq!(
                doc.insert_data(n, u32_arg(offset), &s(arg)),
                Err(DomException::IndexSizeError),
                "{ty}.insertData({offset}, {arg:?})"
            );
        }
    }
    let cases: &[(i64, &str, &str)] = &[
        (-0x1_0000_0000 + 2, "X", "teXst"),
        (0, "", "test"),
        (0, "X", "Xtest"),
        (2, "X", "teXst"),
        (4, "ing", "testing"),
    ];
    for &(offset, arg, expected) in cases {
        let mut doc = Document::new();
        for (n, ty) in char_nodes(&mut doc) {
            doc.insert_data(n, u32_arg(offset), &s(arg)).unwrap();
            assert_eq!(
                data(&doc, n),
                expected,
                "{ty}.insertData({offset}, {arg:?})"
            );
        }
    }
    let mut doc = Document::new();
    for (n, ty) in char_nodes(&mut doc) {
        doc.set_data(
            n,
            &s("This is the character data, append more 資料，測試資料"),
        );
        doc.insert_data(n, 26, &s(" test")).unwrap();
        assert_eq!(
            data(&doc, n),
            "This is the character data test, append more 資料，測試資料",
            "{ty}"
        );
        doc.insert_data(n, 48, &s("更多")).unwrap();
        assert_eq!(
            data(&doc, n),
            "This is the character data test, append more 資料，更多測試資料",
            "{ty}"
        );
        doc.set_data(n, &s("🌠 test 🌠 TEST"));
        doc.insert_data(n, 5, &s("--")).unwrap();
        assert_eq!(data(&doc, n), "🌠 te--st 🌠 TEST", "{ty}");
    }
}

#[test]
// CharacterData-replaceData.html: "with invalid offset".
fn character_data_replace_data_offset_invalido() {
    let mut doc = Document::new();
    for (n, ty) in char_nodes(&mut doc) {
        for (offset, count, arg) in [(5, 1, "x"), (5, 0, ""), (-1, 1, "x"), (-1, 0, "")] {
            assert_eq!(
                doc.replace_data(n, u32_arg(offset), u32_arg(count), &s(arg)),
                Err(DomException::IndexSizeError),
                "{ty}.replaceData({offset}, {count}, {arg:?})"
            );
        }
        assert_eq!(data(&doc, n), "test");
    }
}

#[test]
// CharacterData-replaceData.html: count recortado (también negativo), antes del inicio, al
// inicio/en el medio/al final (más corto, igual, más largo), todo el string, string vacío, no
// ASCII y fuera del BMP.
fn character_data_replace_data() {
    let cases: &[(i64, i64, &str, &str)] = &[
        (2, 10, "yo", "teyo"),
        (2, -1, "yo", "teyo"),
        (0, 0, "yo", "yotest"),
        (0, 2, "y", "yst"),
        (0, 2, "yo", "yost"),
        (0, 2, "yoa", "yoast"),
        (1, 2, "o", "tot"),
        (1, 2, "yo", "tyot"),
        (1, 1, "waddup", "twaddupst"),
        (1, 20, "yo", "tyo"),
        (2, 20, "yo", "teyo"),
        (4, 20, "yo", "testyo"),
        (0, 4, "quux", "quux"),
        (0, 4, "", ""),
    ];
    for &(offset, count, arg, expected) in cases {
        let mut doc = Document::new();
        for (n, ty) in char_nodes(&mut doc) {
            doc.replace_data(n, u32_arg(offset), u32_arg(count), &s(arg))
                .unwrap();
            assert_eq!(
                data(&doc, n),
                expected,
                "{ty}.replaceData({offset}, {count}, {arg:?})"
            );
        }
    }
    let mut doc = Document::new();
    for (n, ty) in char_nodes(&mut doc) {
        doc.replace_data(n, 1, 1, &s("waddup")).unwrap();
        doc.replace_data(n, 1, 1, &s("yup")).unwrap();
        assert_eq!(data(&doc, n), "tyupaddupst", "{ty}");

        doc.set_data(
            n,
            &s("This is the character data test, append 資料，更多資料"),
        );
        doc.replace_data(n, 33, 6, &s("other")).unwrap();
        assert_eq!(
            data(&doc, n),
            "This is the character data test, other 資料，更多資料",
            "{ty}"
        );
        doc.replace_data(n, 44, 2, &s("文字")).unwrap();
        assert_eq!(
            data(&doc, n),
            "This is the character data test, other 資料，更多文字",
            "{ty}"
        );

        doc.set_data(n, &s("🌠 test 🌠 TEST"));
        doc.replace_data(n, 5, 8, &s("--")).unwrap();
        assert_eq!(data(&doc, n), "🌠 te--ST", "{ty}");
    }
}

#[test]
// CharacterData-substringData.html: offset inválido, en rango, count cero, offsets muy grandes
// y negativos, counts en rango, grandes, muy grandes y negativos, no ASCII y fuera del BMP.
fn character_data_substring_data() {
    let mut doc = Document::new();
    for (n, ty) in char_nodes(&mut doc) {
        let sub = |doc: &Document, offset: i64, count: i64| {
            doc.substring_data(n, u32_arg(offset), u32_arg(count))
                .map(|x| x.to_string())
        };
        for offset in [5, 6, -1] {
            assert_eq!(
                sub(&doc, offset, 0),
                Err(DomException::IndexSizeError),
                "{ty}.substringData({offset}, 0)"
            );
        }
        let ok: &[(i64, i64, &str)] = &[
            (0, 1, "t"),
            (1, 1, "e"),
            (2, 1, "s"),
            (3, 1, "t"),
            (4, 1, ""),
            (0, 0, ""),
            (1, 0, ""),
            (2, 0, ""),
            (3, 0, ""),
            (4, 0, ""),
            (0x1_0000_0000, 1, "t"),
            (0x1_0000_0000 + 1, 1, "e"),
            (0x1_0000_0000 + 2, 1, "s"),
            (0x1_0000_0000 + 3, 1, "t"),
            (0x1_0000_0000 + 4, 1, ""),
            (-0x1_0000_0000 + 2, 1, "s"),
            (0, 2, "te"),
            (0, 3, "tes"),
            (0, 4, "test"),
            (0, 5, "test"),
            (2, 20, "st"),
            (2, 0x1_0000_0000 + 1, "s"),
            (0, -1, "test"),
            (0, -0x1_0000_0000 + 2, "te"),
        ];
        for &(offset, count, expected) in ok {
            assert_eq!(
                sub(&doc, offset, count).as_deref(),
                Ok(expected),
                "{ty}.substringData({offset}, {count})"
            );
        }
        doc.set_data(
            n,
            &s("This is the character data test, other 資料，更多文字"),
        );
        assert_eq!(sub(&doc, 12, 4).as_deref(), Ok("char"));
        assert_eq!(sub(&doc, 39, 2).as_deref(), Ok("資料"));
        doc.set_data(n, &s("🌠 test 🌠 TEST"));
        assert_eq!(sub(&doc, 5, 8).as_deref(), Ok("st 🌠 TE"));
    }
}

#[test]
// CharacterData-data.html: valor inicial, data = null ('' por [LegacyNullToEmptyString]),
// undefined, 0, '', '--', '資料' y '🌠 test 🌠 TEST' (length en code units UTF-16).
fn character_data_data() {
    let cases: &[(&str, &str, usize)] = &[
        ("", "", 0),
        ("undefined", "undefined", 9),
        ("0", "0", 1),
        ("", "", 0),
        ("--", "--", 2),
        ("資料", "資料", 2),
        ("🌠 test 🌠 TEST", "🌠 test 🌠 TEST", 15),
    ];
    let mut doc = Document::new();
    for (n, _) in char_nodes(&mut doc) {
        assert_eq!(data(&doc, n), "test");
        assert_eq!(doc.data_length(n), 4);
    }
    for &(value, expected, length) in cases {
        let mut doc = Document::new();
        for (n, ty) in char_nodes(&mut doc) {
            doc.set_data(n, &s(value));
            assert_eq!(data(&doc, n), expected, "{ty}.data = {value:?}");
            assert_eq!(doc.data_length(n), length, "{ty}.length tras {value:?}");
        }
    }
}

// --- Text-splitText.html y Text-wholeText.html ---

#[test]
// Text-splitText.html: "Split text after end of data", "Split empty text", "Split text at
// beginning", "Split text at end", "Split root" y "Split child".
fn text_split_text() {
    let mut doc = Document::new();
    let t = text(&mut doc, "camembert");
    assert_eq!(doc.split_text(t, 10), Err(DomException::IndexSizeError));

    for (initial, offset, before, after) in [
        ("", 0, "", ""),
        ("comté", 0, "", "comté"),
        ("comté", 5, "comté", ""),
        ("comté", 3, "com", "té"),
    ] {
        let t = text(&mut doc, initial);
        let new_text = doc.split_text(t, offset).unwrap();
        assert_eq!(data(&doc, t), before, "{initial:?}.splitText({offset})");
        assert_eq!(
            data(&doc, new_text),
            after,
            "{initial:?}.splitText({offset})"
        );
        assert_eq!(doc.parent(new_text), None);
    }

    let parent = el(&mut doc, "div");
    let t = text(&mut doc, "bleu");
    append(&mut doc, parent, t);
    let new_text = doc.split_text(t, 2).unwrap();
    assert_eq!(data(&doc, t), "bl");
    assert_eq!(data(&doc, new_text), "eu");
    assert_eq!(doc.next_sibling(t), Some(new_text));
    assert_eq!(doc.parent(new_text), Some(parent));
}

#[test]
// Text-wholeText.html: "wholeText returns text of all Text nodes logically adjacent to the
// node, in document order.".
fn text_whole_text() {
    let mut doc = Document::new();
    let parent = el(&mut doc, "div");
    let t1 = text(&mut doc, "a");
    let t2 = text(&mut doc, "b");
    let t3 = text(&mut doc, "c");
    let whole = |doc: &Document, t| doc.whole_text(t).to_string();
    assert_eq!(whole(&doc, t1), "a");
    append(&mut doc, parent, t1);
    assert_eq!(whole(&doc, t1), "a");
    append(&mut doc, parent, t2);
    assert_eq!(whole(&doc, t1), "ab");
    assert_eq!(whole(&doc, t2), "ab");
    append(&mut doc, parent, t3);
    for t in [t1, t2, t3] {
        assert_eq!(whole(&doc, t), "abc");
    }
    let a = el(&mut doc, "a");
    doc.set_text_content(a, Some(s("I'm an Anchor")));
    doc.insert_before_checked(parent, a, Some(t3)).unwrap();
    let span = el(&mut doc, "span");
    doc.set_text_content(span, Some(s("I'm a Span")));
    let other_span = el(&mut doc, "span");
    append(&mut doc, parent, other_span);
    assert_eq!(whole(&doc, t1), "ab");
    assert_eq!(whole(&doc, t2), "ab");
    assert_eq!(whole(&doc, t3), "c");
}

// --- ParentNode-prepend/append/replaceChildren.html ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParentOp {
    Prepend,
    Append,
    ReplaceChildren,
}

fn parent_op(
    doc: &mut Document,
    op: ParentOp,
    parent: NodeId,
    nodes: Vec<NodeOrString>,
) -> Result<(), DomException> {
    match op {
        ParentOp::Prepend => doc.prepend(parent, nodes),
        ParentOp::Append => doc.append_nodes(parent, nodes),
        ParentOp::ReplaceChildren => doc.replace_children(parent, nodes),
    }
}

/// `node.cloneNode()` para los dos tipos de padre del test (un `div` y un fragmento).
fn new_parent(doc: &mut Document, fragment: bool) -> NodeId {
    if fragment {
        doc.create_document_fragment()
    } else {
        el(doc, "div")
    }
}

fn text_content(doc: &Document, n: NodeId) -> String {
    doc.get_text_content(n)
        .map(|x| x.to_string())
        .unwrap_or_default()
}

/// pre-insertion-validation-hierarchy.js (`preInsertionValidateHierarchy(methodName)`).
fn pre_insertion_validate_hierarchy(op: ParentOp) {
    let hre = Err(DomException::HierarchyRequestError);
    // Paso 2.
    let mut h = html_document();
    assert_eq!(
        parent_op(&mut h.doc, op, h.body, vec![node(h.body)]),
        hre,
        "{op:?}"
    );
    assert_eq!(
        parent_op(&mut h.doc, op, h.body, vec![node(h.html)]),
        hre,
        "{op:?}"
    );
    // Paso 5: un Text en el documento.
    let mut h = html_document();
    let root = h.doc.root();
    let t = text(&mut h.doc, "text");
    assert_eq!(
        parent_op(&mut h.doc, op, root, vec![node(t)]),
        hre,
        "{op:?}"
    );
    // Paso 5: un doctype en algo que no es un documento.
    let mut h = html_document();
    let a = el(&mut h.doc, "a");
    assert_eq!(
        parent_op(&mut h.doc, op, a, vec![node(h.doctype)]),
        hre,
        "{op:?}"
    );
    // Paso 6: fragmento con varios elementos.
    let mut h = html_document();
    let root = h.doc.root();
    h.doc.remove_self(h.html);
    let df = h.doc.create_document_fragment();
    let a = el(&mut h.doc, "a");
    let b = el(&mut h.doc, "b");
    append(&mut h.doc, df, a);
    append(&mut h.doc, df, b);
    assert_eq!(
        parent_op(&mut h.doc, op, root, vec![node(df)]),
        hre,
        "{op:?}"
    );
    if op != ParentOp::ReplaceChildren {
        // Paso 6: fragmento con un elemento en un documento que ya tiene uno.
        let mut h = html_document();
        let root = h.doc.root();
        let df = h.doc.create_document_fragment();
        let a = el(&mut h.doc, "a");
        append(&mut h.doc, df, a);
        assert_eq!(
            parent_op(&mut h.doc, op, root, vec![node(df)]),
            hre,
            "{op:?}"
        );
        // Paso 6: un elemento en un documento que ya tiene uno.
        let mut h = html_document();
        let root = h.doc.root();
        let a = el(&mut h.doc, "a");
        assert_eq!(
            parent_op(&mut h.doc, op, root, vec![node(a)]),
            hre,
            "{op:?}"
        );
        // Paso 6: un doctype en un documento que ya tiene otro.
        let mut h = html_document();
        let root = h.doc.root();
        let doctype = h.doc.clone_node(h.doctype, false);
        h.doc.remove_self(h.html);
        assert_eq!(
            parent_op(&mut h.doc, op, root, vec![node(doctype)]),
            hre,
            "{op:?}"
        );
    }
    if op == ParentOp::Append {
        // Paso 6: un doctype en un documento con un elemento.
        let mut h = html_document();
        let root = h.doc.root();
        let doctype = h.doc.clone_node(h.doctype, false);
        h.doc.remove_self(h.doctype);
        assert_eq!(
            parent_op(&mut h.doc, op, root, vec![node(doctype)]),
            hre,
            "{op:?}"
        );
    }
}

#[test]
// ParentNode-prepend.html: preInsertionValidateHierarchy("prepend").
fn prepend_validacion_jerarquia() {
    pre_insertion_validate_hierarchy(ParentOp::Prepend);
}

#[test]
// ParentNode-append.html: preInsertionValidateHierarchy("append").
fn append_validacion_jerarquia() {
    pre_insertion_validate_hierarchy(ParentOp::Append);
}

#[test]
// ParentNode-replaceChildren.html: preInsertionValidateHierarchy("replaceChildren").
fn replace_children_validacion_jerarquia() {
    pre_insertion_validate_hierarchy(ParentOp::ReplaceChildren);
}

#[test]
// ParentNode-prepend.html, -append.html y -replaceChildren.html: "... without any argument",
// "... with null/undefined/only text/only one element as an argument, on a parent having no
// child." (Element y DocumentFragment).
fn parent_node_sin_hijos_previos() {
    for op in [
        ParentOp::Prepend,
        ParentOp::Append,
        ParentOp::ReplaceChildren,
    ] {
        for fragment in [false, true] {
            let mut doc = Document::new();
            let parent = new_parent(&mut doc, fragment);
            parent_op(&mut doc, op, parent, vec![]).unwrap();
            assert!(doc.child_nodes(parent).is_empty(), "{op:?}");
            for arg in ["null", "undefined", "text"] {
                let parent = new_parent(&mut doc, fragment);
                parent_op(&mut doc, op, parent, vec![string(arg)]).unwrap();
                let first = doc.first_child(parent).unwrap();
                assert_eq!(text_content(&doc, first), arg, "{op:?} {arg}");
            }
            let parent = new_parent(&mut doc, fragment);
            let x = el(&mut doc, "x");
            parent_op(&mut doc, op, parent, vec![node(x)]).unwrap();
            assert_eq!(doc.child_nodes(parent), [x], "{op:?}");
        }
    }
}

#[test]
// ParentNode-prepend.html: "... with null as an argument, on a parent having a child." y "...
// with one element and text as argument, on a parent having a child.".
fn prepend_con_hijo_previo() {
    for fragment in [false, true] {
        let mut doc = Document::new();
        let parent = new_parent(&mut doc, fragment);
        let child = el(&mut doc, "test");
        append(&mut doc, parent, child);
        doc.prepend(parent, vec![string("null")]).unwrap();
        let nodes = doc.child_nodes(parent);
        assert_eq!(text_content(&doc, nodes[0]), "null");
        assert_eq!(nodes[1], child);

        let parent = new_parent(&mut doc, fragment);
        let x = el(&mut doc, "x");
        let child = el(&mut doc, "test");
        append(&mut doc, parent, child);
        doc.prepend(parent, vec![node(x), string("text")]).unwrap();
        let nodes = doc.child_nodes(parent);
        assert_eq!(nodes[0], x);
        assert_eq!(text_content(&doc, nodes[1]), "text");
        assert_eq!(nodes[2], child);
    }
}

#[test]
// ParentNode-append.html: "... with null as an argument, on a parent having a child.", "...
// with one element and text as argument, on a parent having a child." y "... with the same
// element twice.".
fn append_con_hijo_previo() {
    for fragment in [false, true] {
        let mut doc = Document::new();
        let parent = new_parent(&mut doc, fragment);
        let child = el(&mut doc, "test");
        append(&mut doc, parent, child);
        doc.append_nodes(parent, vec![string("null")]).unwrap();
        let nodes = doc.child_nodes(parent);
        assert_eq!(nodes[0], child);
        assert_eq!(text_content(&doc, nodes[1]), "null");

        let parent = new_parent(&mut doc, fragment);
        let x = el(&mut doc, "x");
        let child = el(&mut doc, "test");
        append(&mut doc, parent, child);
        doc.append_nodes(parent, vec![node(x), string("text")])
            .unwrap();
        let nodes = doc.child_nodes(parent);
        assert_eq!(nodes[0], child);
        assert_eq!(nodes[1], x);
        assert_eq!(text_content(&doc, nodes[2]), "text");

        let parent = new_parent(&mut doc, fragment);
        let x = el(&mut doc, "x");
        let y = el(&mut doc, "y");
        doc.append_nodes(parent, vec![node(x), node(y), node(x)])
            .unwrap();
        assert_eq!(doc.child_nodes(parent), [y, x]);
    }
}

#[test]
// ParentNode-replaceChildren.html: "... without any argument, on a parent having a child.",
// "... with null as an argument, on a parent having a child." y "... with one element and text
// as argument, on a parent having a child.".
fn replace_children_con_hijo_previo() {
    for fragment in [false, true] {
        let mut doc = Document::new();
        let parent = new_parent(&mut doc, fragment);
        let child = el(&mut doc, "test");
        append(&mut doc, parent, child);
        doc.replace_children(parent, vec![]).unwrap();
        assert!(doc.child_nodes(parent).is_empty());

        let parent = new_parent(&mut doc, fragment);
        let child = el(&mut doc, "test");
        append(&mut doc, parent, child);
        doc.replace_children(parent, vec![string("null")]).unwrap();
        let nodes = doc.child_nodes(parent);
        assert_eq!(nodes.len(), 1);
        assert_eq!(text_content(&doc, nodes[0]), "null");

        let parent = new_parent(&mut doc, fragment);
        let x = el(&mut doc, "x");
        let child = el(&mut doc, "test");
        append(&mut doc, parent, child);
        doc.replace_children(parent, vec![node(x), string("text")])
            .unwrap();
        let nodes = doc.child_nodes(parent);
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0], x);
        assert_eq!(text_content(&doc, nodes[1]), "text");
    }
}

#[test]
// ParentNode-replaceChildren.html: "... should move nodes in the right order" (Element y
// DocumentFragment): primero el observer del padre anterior con dos remociones, después uno
// con las dos inserciones y las dos remociones.
fn replace_children_mueve_nodos_en_orden() {
    for fragment in [false, true] {
        let mut doc = Document::new();
        let previous_parent = new_parent(&mut doc, fragment);
        let insertions = [el(&mut doc, "test1"), el(&mut doc, "test2")];
        doc.append_nodes(
            previous_parent,
            insertions.iter().map(|&n| node(n)).collect(),
        )
        .unwrap();
        let parent = new_parent(&mut doc, fragment);
        let children = [el(&mut doc, "test3"), el(&mut doc, "test4")];
        doc.append_nodes(parent, children.iter().map(|&n| node(n)).collect())
            .unwrap();

        let previous_observer = doc.create_mutation_observer();
        doc.observe(previous_observer, previous_parent, child_list())
            .unwrap();
        let observer = doc.create_mutation_observer();
        doc.observe(observer, parent, child_list()).unwrap();

        let moved = doc
            .element_children(previous_parent)
            .into_iter()
            .map(node)
            .collect();
        doc.replace_children(parent, moved).unwrap();

        let delivered = doc.notify_mutation_observers();
        assert_eq!(delivered.len(), 2);
        let (id, mutations) = &delivered[0];
        assert_eq!(*id, previous_observer, "phase 0");
        assert_eq!(mutations.len(), 2);
        for (i, m) in mutations.iter().enumerate() {
            assert_eq!(m.type_, MutationType::ChildList);
            assert!(m.added_nodes.is_empty());
            assert_eq!(m.removed_nodes, [insertions[i]]);
        }
        let (id, mutations) = &delivered[1];
        assert_eq!(*id, observer, "phase 1");
        assert_eq!(mutations.len(), 1, "mutations.length");
        assert_eq!(mutations[0].type_, MutationType::ChildList);
        assert_eq!(mutations[0].added_nodes, insertions, "added nodes");
        assert_eq!(mutations[0].removed_nodes, children, "removed nodes");
    }
}

#[test]
// ParentNode-replaceChildren.html: "Document.replaceChildren() with an element, replacing an
// existing doctype and element.", "... with a DocumentFragment containing a single element,
// ..." y "... with a doctype, replacing an existing doctype and element.".
fn replace_children_en_document_reemplaza_doctype_y_elemento() {
    let mut h = html_document();
    let root = h.doc.root();
    let a = el(&mut h.doc, "a");
    assert_eq!(h.doc.replace_children(root, vec![node(a)]), Ok(()));
    assert_eq!(h.doc.child_nodes(root), [a]);

    let mut h = html_document();
    let root = h.doc.root();
    let df = h.doc.create_document_fragment();
    let a = el(&mut h.doc, "a");
    append(&mut h.doc, df, a);
    assert_eq!(h.doc.replace_children(root, vec![node(df)]), Ok(()));
    assert_eq!(h.doc.child_nodes(root), [a]);

    let mut h = html_document();
    let root = h.doc.root();
    let doctype = h.doc.clone_node(h.doctype, false);
    assert_eq!(h.doc.replace_children(root, vec![node(doctype)]), Ok(()));
    assert_eq!(h.doc.child_nodes(root), [doctype]);
}

#[test]
// ParentNode-replaceChildren.html: "Document.replaceChildren() without any argument, removing
// an existing doctype and element.", "... with two elements throws a HierarchyRequestError." y
// "... with text throws a HierarchyRequestError.".
fn replace_children_en_document_errores_y_vacio() {
    let mut h = html_document();
    let root = h.doc.root();
    h.doc.replace_children(root, vec![]).unwrap();
    assert!(h.doc.child_nodes(root).is_empty());

    let mut h = html_document();
    let root = h.doc.root();
    let a = el(&mut h.doc, "a");
    let b = el(&mut h.doc, "b");
    assert_eq!(
        h.doc.replace_children(root, vec![node(a), node(b)]),
        Err(DomException::HierarchyRequestError)
    );
    assert_eq!(
        h.doc.replace_children(root, vec![string("text")]),
        Err(DomException::HierarchyRequestError)
    );
}

#[test]
// ParentNode-replaceChildren.html: "There should be a MutationRecord for the node removed from
// another parent node." y "There should be MutationRecords for the nodes removed from another
// parent node.".
fn replace_children_records_de_remocion_en_otro_padre() {
    // <div id='a'>text<div id='b'>text2</div></div>
    let mut doc = Document::new();
    let root = el(&mut doc, "div");
    let a = el(&mut doc, "div");
    append(&mut doc, root, a);
    let txt = text(&mut doc, "text");
    append(&mut doc, a, txt);
    let b = el(&mut doc, "div");
    append(&mut doc, a, b);
    let txt2 = text(&mut doc, "text2");
    append(&mut doc, b, txt2);
    run_mutation_test(
        &mut doc,
        a,
        ObserverInit {
            subtree: true,
            ..child_list()
        },
        "replaceChildren(txt)",
        |doc| {
            doc.replace_children(b, vec![node(txt)]).unwrap();
            // El test de WPT no mira previousSibling/nextSibling; son los del spec (remove).
            vec![
                Exp {
                    removed: vec![txt],
                    next: Some(b),
                    ..Exp::child_list()
                },
                Exp {
                    target: Some(b),
                    removed: vec![txt2],
                    added: vec![txt],
                    ..Exp::child_list()
                },
            ]
        },
    );

    // <div id='a'><div id='c'></div>text<div id='b'>text2</div></div>
    let mut doc = Document::new();
    let root = el(&mut doc, "div");
    let a = el(&mut doc, "div");
    append(&mut doc, root, a);
    let c = el(&mut doc, "div");
    append(&mut doc, a, c);
    let txt = text(&mut doc, "text");
    append(&mut doc, a, txt);
    let b = el(&mut doc, "div");
    append(&mut doc, a, b);
    let txt2 = text(&mut doc, "text2");
    append(&mut doc, b, txt2);
    run_mutation_test(
        &mut doc,
        a,
        ObserverInit {
            subtree: true,
            ..child_list()
        },
        "replaceChildren(c, txt)",
        |doc| {
            doc.replace_children(b, vec![node(c), node(txt)]).unwrap();
            vec![
                Exp {
                    removed: vec![c],
                    next: Some(txt),
                    ..Exp::child_list()
                },
                Exp {
                    removed: vec![txt],
                    next: Some(b),
                    ..Exp::child_list()
                },
                Exp {
                    target: Some(b),
                    removed: vec![txt2],
                    added: vec![c, txt],
                    ..Exp::child_list()
                },
            ]
        },
    );
}

// --- ChildNode-before/after/replaceWith.html y ChildNode-remove.js ---

/// Los tres `child` de los tests de ChildNode: (nodo, innerHTML esperado).
fn child_node_kinds(doc: &mut Document) -> [(NodeId, &'static str); 3] {
    [
        (comment(doc, "test"), "<!--test-->"),
        (el(doc, "test"), "<test></test>"),
        (text(doc, "test"), "test"),
    ]
}

#[test]
// ChildNode-before.html: sin argumentos, con null, undefined, '' y texto, con un elemento, con
// un elemento y texto, y con el propio nodo como argumento (Comment, Element y Text).
fn child_node_before_basico() {
    for kind in 0..3 {
        let fresh = || {
            let mut doc = Document::new();
            let (child, html) = child_node_kinds(&mut doc)[kind];
            let parent = el(&mut doc, "div");
            append(&mut doc, parent, child);
            (doc, parent, child, html)
        };
        let (mut doc, parent, child, html) = fresh();
        doc.before(child, vec![]).unwrap();
        assert_eq!(inner_html(&doc, parent), html);
        for arg in ["null", "undefined", "text"] {
            let (mut doc, parent, child, html) = fresh();
            doc.before(child, vec![string(arg)]).unwrap();
            assert_eq!(inner_html(&doc, parent), format!("{arg}{html}"));
        }
        let (mut doc, parent, child, _) = fresh();
        doc.before(child, vec![string("")]).unwrap();
        assert_eq!(data(&doc, doc.first_child(parent).unwrap()), "");

        let (mut doc, parent, child, html) = fresh();
        let x = el(&mut doc, "x");
        doc.before(child, vec![node(x)]).unwrap();
        assert_eq!(inner_html(&doc, parent), format!("<x></x>{html}"));

        let (mut doc, parent, child, html) = fresh();
        let x = el(&mut doc, "x");
        doc.before(child, vec![node(x), string("text")]).unwrap();
        assert_eq!(inner_html(&doc, parent), format!("<x></x>text{html}"));

        let (mut doc, parent, child, html) = fresh();
        doc.before(child, vec![string("text"), node(child)])
            .unwrap();
        assert_eq!(inner_html(&doc, parent), format!("text{html}"));
    }
}

#[test]
// ChildNode-before.html: con el propio nodo y un hermano (cambian de lugar), con todos los
// hermanos, con algunos hermanos (sin cambios; viable sibling primero o no), cuando pre-insert
// equivale a prepend, con un hermano y texto, y sobre un nodo sin padre.
fn child_node_before_con_hermanos() {
    for kind in 0..3 {
        // (antes, argumentos, después): los nombres son elementos; "child" es el nodo; "1" es
        // texto.
        let cases: &[(&[&str], &[&str], &str)] = &[
            (&["child", "x"], &["x", "child"], "<x></x>{}"),
            (
                &["y", "child", "x"],
                &["x", "y", "z"],
                "<x></x><y></y><z></z>{}",
            ),
            (
                &["x", "y", "z", "child"],
                &["y", "z"],
                "<x></x><y></y><z></z>{}",
            ),
            (
                &["v", "x", "y", "z", "child"],
                &["y", "z"],
                "<v></v><x></x><y></y><z></z>{}",
            ),
            (&["x", "y", "child"], &["y", "x"], "<y></y><x></x>{}"),
            (
                &["x", "1", "y", "child"],
                &["x", "'2"],
                "1<y></y><x></x>2{}",
            ),
        ];
        for &(initial, args, expected) in cases {
            let (mut doc, parent, child, html) = sibling_fixture(kind, initial);
            let args = sibling_args(&mut doc, parent, child, args);
            doc.before(child, args).unwrap();
            assert_eq!(inner_html(&doc, parent), expected.replace("{}", html));
        }
    }
    let mut doc = Document::new();
    let x = el(&mut doc, "x");
    let y = el(&mut doc, "y");
    doc.before(x, vec![node(y)]).unwrap();
    assert_eq!(doc.previous_sibling(x), None);
}

/// Arma un `div` con los hijos nombrados (elementos por nombre, `child` para el nodo bajo
/// prueba, dígitos para nodos de texto).
fn sibling_fixture(kind: usize, initial: &[&str]) -> (Document, NodeId, NodeId, &'static str) {
    let mut doc = Document::new();
    let (child, html) = child_node_kinds(&mut doc)[kind];
    let parent = el(&mut doc, "div");
    for &name in initial {
        let n = match name {
            "child" => child,
            _ if name.chars().all(|c| c.is_ascii_digit()) => text(&mut doc, name),
            _ => el(&mut doc, name),
        };
        append(&mut doc, parent, n);
    }
    (doc, parent, child, html)
}

/// Los argumentos de los tests de hermanos: `child`, un hijo existente por nombre, un
/// elemento nuevo, o `'texto` para un string.
fn sibling_args(
    doc: &mut Document,
    parent: NodeId,
    child: NodeId,
    args: &[&str],
) -> Vec<NodeOrString> {
    args.iter()
        .map(|&a| {
            if let Some(t) = a.strip_prefix('\'') {
                return string(t);
            }
            if a == "child" {
                return node(child);
            }
            let existing = doc
                .child_nodes(parent)
                .into_iter()
                .find(|&c| doc.node_type(c) == NodeType::Element && doc.local_name(c) == a);
            node(existing.unwrap_or_else(|| el(doc, a)))
        })
        .collect()
}

#[test]
// ChildNode-after.html: sin argumentos, con null, undefined, '' y texto, con un elemento, con
// un elemento y texto, y con el propio nodo como argumento (Comment, Element y Text).
fn child_node_after_basico() {
    for kind in 0..3 {
        let fresh = || {
            let mut doc = Document::new();
            let (child, html) = child_node_kinds(&mut doc)[kind];
            let parent = el(&mut doc, "div");
            append(&mut doc, parent, child);
            (doc, parent, child, html)
        };
        let (mut doc, parent, child, html) = fresh();
        doc.after(child, vec![]).unwrap();
        assert_eq!(inner_html(&doc, parent), html);
        for arg in ["null", "undefined", "text"] {
            let (mut doc, parent, child, html) = fresh();
            doc.after(child, vec![string(arg)]).unwrap();
            assert_eq!(inner_html(&doc, parent), format!("{html}{arg}"));
        }
        let (mut doc, parent, child, _) = fresh();
        doc.after(child, vec![string("")]).unwrap();
        assert_eq!(data(&doc, doc.last_child(parent).unwrap()), "");

        let (mut doc, parent, child, html) = fresh();
        let x = el(&mut doc, "x");
        doc.after(child, vec![node(x)]).unwrap();
        assert_eq!(inner_html(&doc, parent), format!("{html}<x></x>"));

        let (mut doc, parent, child, html) = fresh();
        let x = el(&mut doc, "x");
        doc.after(child, vec![node(x), string("text")]).unwrap();
        assert_eq!(inner_html(&doc, parent), format!("{html}<x></x>text"));

        let (mut doc, parent, child, html) = fresh();
        doc.after(child, vec![string("text"), node(child)]).unwrap();
        assert_eq!(inner_html(&doc, parent), format!("text{html}"));
    }
}

#[test]
// ChildNode-after.html: con el propio nodo y un hermano, con todos los hermanos, con algunos
// hermanos (sin cambios), cuando pre-insert equivale a append, con un hermano y texto, y sobre
// un nodo sin padre.
fn child_node_after_con_hermanos() {
    for kind in 0..3 {
        let cases: &[(&[&str], &[&str], &str)] = &[
            (&["x", "child"], &["child", "x"], "{}<x></x>"),
            (
                &["y", "child", "x"],
                &["x", "y", "z"],
                "{}<x></x><y></y><z></z>",
            ),
            (
                &["child", "x", "y", "z"],
                &["x", "y"],
                "{}<x></x><y></y><z></z>",
            ),
            (
                &["child", "v", "x", "y", "z"],
                &["v", "x"],
                "{}<v></v><x></x><y></y><z></z>",
            ),
            (&["child", "x", "y"], &["y", "x"], "{}<y></y><x></x>"),
            (
                &["child", "x", "1", "y"],
                &["x", "'2"],
                "{}<x></x>21<y></y>",
            ),
        ];
        for &(initial, args, expected) in cases {
            let (mut doc, parent, child, html) = sibling_fixture(kind, initial);
            let args = sibling_args(&mut doc, parent, child, args);
            doc.after(child, args).unwrap();
            assert_eq!(inner_html(&doc, parent), expected.replace("{}", html));
        }
    }
    let mut doc = Document::new();
    let x = el(&mut doc, "x");
    let y = el(&mut doc, "y");
    doc.after(x, vec![node(y)]).unwrap();
    assert_eq!(doc.next_sibling(x), None);
}

#[test]
// ChildNode-replaceWith.html: sin argumentos, con null, undefined, '' y texto, con un elemento,
// con hermanos, con un hermano y texto, con un hermano y el propio nodo, con un elemento y
// texto, y sobre un nodo sin padre (Comment, Element y Text).
fn child_node_replace_with() {
    for kind in 0..3 {
        let cases: &[(&[&str], &[&str], &str)] = &[
            (&["child"], &[], ""),
            (&["child"], &["'null"], "null"),
            (&["child"], &["'undefined"], "undefined"),
            (&["child"], &["'"], ""),
            (&["child"], &["'text"], "text"),
            (&["child"], &["x"], "<x></x>"),
            (
                &["y", "child", "x"],
                &["x", "y", "z"],
                "<x></x><y></y><z></z>",
            ),
            (&["child", "x", "1"], &["x", "'2"], "<x></x>21"),
            (&["child", "x", "9"], &["x", "child"], "<x></x>{}9"),
            (&["child"], &["x", "'text"], "<x></x>text"),
        ];
        for &(initial, args, expected) in cases {
            let (mut doc, parent, child, html) = sibling_fixture(kind, initial);
            let args = sibling_args(&mut doc, parent, child, args);
            doc.replace_with(child, args).unwrap();
            assert_eq!(
                inner_html(&doc, parent),
                expected.replace("{}", html),
                "{initial:?}"
            );
        }
        // "... on a parentless child with two elements as arguments.".
        let mut doc = Document::new();
        let (child, _) = child_node_kinds(&mut doc)[kind];
        let parent = el(&mut doc, "div");
        let x = el(&mut doc, "x");
        let y = el(&mut doc, "y");
        append(&mut doc, parent, x);
        append(&mut doc, parent, y);
        doc.replace_with(child, vec![node(x), node(y)]).unwrap();
        assert_eq!(inner_html(&doc, parent), "<x></x><y></y>");
    }
}

#[test]
// ChildNode-remove.js (de Element-remove.html, CharacterData-remove.html): "remove() should
// work if <tipo> doesn't have a parent", "... does have a parent" y "... does have a parent
// and siblings".
fn child_node_remove() {
    let mut doc = Document::new();
    let parent = el(&mut doc, "div");
    let nodes = [
        (el(&mut doc, "div"), "element"),
        (text(&mut doc, "text"), "text"),
        (comment(&mut doc, "comment"), "comment"),
    ];
    for (n, ty) in nodes {
        assert_eq!(doc.parent(n), None, "{ty}");
        doc.remove_self(n);
        assert_eq!(doc.parent(n), None, "{ty}");

        append(&mut doc, parent, n);
        assert_eq!(doc.parent(n), Some(parent), "{ty}");
        doc.remove_self(n);
        assert_eq!(doc.parent(n), None, "{ty}");
        assert!(doc.child_nodes(parent).is_empty(), "{ty}");

        let before = comment(&mut doc, "before");
        append(&mut doc, parent, before);
        append(&mut doc, parent, n);
        let after = comment(&mut doc, "after");
        append(&mut doc, parent, after);
        assert_eq!(doc.parent(n), Some(parent), "{ty}");
        doc.remove_self(n);
        assert_eq!(doc.parent(n), None, "{ty}");
        assert_eq!(doc.child_nodes(parent), [before, after], "{ty}");
        doc.replace_children(parent, vec![]).unwrap();
    }
}

// --- Element-insertAdjacentElement.html y Element-insertAdjacentText.html ---

struct AdjacentDoc {
    doc: Document,
    html: NodeId,
    target: NodeId,
    target2: NodeId,
    tests: [NodeId; 4],
}

/// `<div id="target"></div><div id="parent"><span id=target2></span></div><div id="log">`
/// y los `span#test1..4`.
fn adjacent_doc() -> AdjacentDoc {
    let mut doc = Document::new();
    let root = doc.root();
    let html = el(&mut doc, "html");
    append(&mut doc, root, html);
    let body = el(&mut doc, "body");
    append(&mut doc, html, body);
    let target = el(&mut doc, "div");
    set_attr(&mut doc, target, "id", "target");
    append(&mut doc, body, target);
    let parent = el(&mut doc, "div");
    set_attr(&mut doc, parent, "id", "parent");
    append(&mut doc, body, parent);
    let target2 = el(&mut doc, "span");
    set_attr(&mut doc, target2, "id", "target2");
    append(&mut doc, parent, target2);
    let log = el(&mut doc, "div");
    set_attr(&mut doc, log, "id", "log");
    append(&mut doc, body, log);
    let mut tests = [target; 4];
    for (i, t) in tests.iter_mut().enumerate() {
        *t = el(&mut doc, "span");
        set_attr(&mut doc, *t, "id", &format!("test{}", i + 1));
        append(&mut doc, body, *t);
    }
    AdjacentDoc {
        doc,
        html,
        target,
        target2,
        tests,
    }
}

#[test]
// Element-insertAdjacentElement.html: "Inserting to an invalid location should cause a Syntax
// Error exception" y los casos 'beforebegin', 'afterbegin', 'beforeend' y 'afterend'.
fn insert_adjacent_element() {
    let mut a = adjacent_doc();
    let doc = &mut a.doc;
    for t in [a.target, a.target2] {
        assert_eq!(
            doc.insert_adjacent_element(t, "test", a.tests[0]),
            Err(DomException::SyntaxError)
        );
    }
    let id = |doc: &Document, n: Option<NodeId>| n.map(|n| doc.id(n).to_string());
    for t in [a.target, a.target2] {
        let r = doc.insert_adjacent_element(t, "beforebegin", a.tests[0]);
        assert_eq!(r, Ok(Some(a.tests[0])));
        assert_eq!(id(doc, doc.previous_sibling(t)).as_deref(), Some("test1"));
        let r = doc.insert_adjacent_element(t, "afterbegin", a.tests[1]);
        assert_eq!(r, Ok(Some(a.tests[1])));
        assert_eq!(id(doc, doc.first_child(t)).as_deref(), Some("test2"));
        let r = doc.insert_adjacent_element(t, "beforeend", a.tests[2]);
        assert_eq!(r, Ok(Some(a.tests[2])));
        assert_eq!(id(doc, doc.last_child(t)).as_deref(), Some("test3"));
        let r = doc.insert_adjacent_element(t, "afterend", a.tests[3]);
        assert_eq!(r, Ok(Some(a.tests[3])));
        assert_eq!(id(doc, doc.next_sibling(t)).as_deref(), Some("test4"));
    }
}

#[test]
// Element-insertAdjacentElement.html: "Adding more than one child to document should cause a
// HierarchyRequestError exception".
fn insert_adjacent_element_en_document_element() {
    let mut a = adjacent_doc();
    let doc = &mut a.doc;
    let html = a.html;
    assert_eq!(
        doc.insert_adjacent_element(html, "beforebegin", a.tests[0]),
        Err(DomException::HierarchyRequestError)
    );
    assert_eq!(
        doc.insert_adjacent_element(html, "afterbegin", a.tests[1]),
        Ok(Some(a.tests[1]))
    );
    assert_eq!(doc.first_child(html), Some(a.tests[1]));
    assert_eq!(
        doc.insert_adjacent_element(html, "beforeend", a.tests[2]),
        Ok(Some(a.tests[2]))
    );
    assert_eq!(doc.last_child(html), Some(a.tests[2]));
    assert_eq!(
        doc.insert_adjacent_element(html, "afterend", a.tests[3]),
        Err(DomException::HierarchyRequestError)
    );
}

#[test]
// Element-insertAdjacentText.html: "Inserting to an invalid location should cause a Syntax
// Error exception" y los casos 'beforebegin', 'afterbegin', 'beforeend' y 'afterend' (el
// último compara `target.nextSibling` también para target2, como el test original).
fn insert_adjacent_text() {
    let mut a = adjacent_doc();
    let doc = &mut a.doc;
    for t in [a.target, a.target2] {
        assert_eq!(
            doc.insert_adjacent_text(t, "test", s("text")),
            Err(DomException::SyntaxError)
        );
    }
    let value = |doc: &Document, n: Option<NodeId>| {
        n.and_then(|n| doc.node_value(n)).map(|v| v.to_string())
    };
    for t in [a.target, a.target2] {
        doc.insert_adjacent_text(t, "beforebegin", s("test1"))
            .unwrap();
        assert_eq!(
            value(doc, doc.previous_sibling(t)).as_deref(),
            Some("test1")
        );
        doc.insert_adjacent_text(t, "afterbegin", s("test2"))
            .unwrap();
        assert_eq!(value(doc, doc.first_child(t)).as_deref(), Some("test2"));
        doc.insert_adjacent_text(t, "beforeend", s("test3"))
            .unwrap();
        assert_eq!(value(doc, doc.last_child(t)).as_deref(), Some("test3"));
        doc.insert_adjacent_text(t, "afterend", s("test4")).unwrap();
        assert_eq!(
            value(doc, doc.next_sibling(a.target)).as_deref(),
            Some("test4")
        );
    }
}

#[test]
// Element-insertAdjacentText.html: "Adding more than one child to document should cause a
// HierarchyRequestError exception".
fn insert_adjacent_text_en_document_element() {
    let mut a = adjacent_doc();
    let doc = &mut a.doc;
    let html = a.html;
    assert_eq!(
        doc.insert_adjacent_text(html, "beforebegin", s("text1")),
        Err(DomException::HierarchyRequestError)
    );
    doc.insert_adjacent_text(html, "afterbegin", s("test2"))
        .unwrap();
    assert_eq!(data(doc, doc.first_child(html).unwrap()), "test2");
    doc.insert_adjacent_text(html, "beforeend", s("test3"))
        .unwrap();
    assert_eq!(data(doc, doc.last_child(html).unwrap()), "test3");
    assert_eq!(
        doc.insert_adjacent_text(html, "afterend", s("test4")),
        Err(DomException::HierarchyRequestError)
    );
}

// --- MutationObserver (mutationobservers.js) ---

/// Un record esperado de `checkRecords` (lo omitido vale null o vacío).
struct Exp {
    type_: MutationType,
    target: Option<NodeId>,
    added: Vec<NodeId>,
    removed: Vec<NodeId>,
    prev: Option<NodeId>,
    next: Option<NodeId>,
    name: Option<&'static str>,
    ns: Option<&'static str>,
    old: Option<&'static str>,
}

impl Exp {
    fn new(type_: MutationType) -> Exp {
        Exp {
            type_,
            target: None,
            added: Vec::new(),
            removed: Vec::new(),
            prev: None,
            next: None,
            name: None,
            ns: None,
            old: None,
        }
    }

    fn child_list() -> Exp {
        Exp::new(MutationType::ChildList)
    }

    fn attributes(name: &'static str) -> Exp {
        Exp {
            name: Some(name),
            ..Exp::new(MutationType::Attributes)
        }
    }

    fn attributes_old(name: &'static str, old: &'static str) -> Exp {
        Exp {
            old: Some(old),
            ..Exp::attributes(name)
        }
    }

    fn character_data(old: Option<&'static str>) -> Exp {
        Exp {
            old,
            ..Exp::new(MutationType::CharacterData)
        }
    }
}

fn child_list() -> ObserverInit {
    ObserverInit {
        child_list: true,
        ..Default::default()
    }
}

fn attributes_old_value() -> ObserverInit {
    ObserverInit {
        attributes: Some(true),
        attribute_old_value: Some(true),
        ..Default::default()
    }
}

fn character_data_old_value() -> ObserverInit {
    ObserverInit {
        character_data: Some(true),
        character_data_old_value: Some(true),
        ..Default::default()
    }
}

/// `checkRecords` de mutationobservers.js.
fn check_records(target: NodeId, records: &[MutationRecord], expected: &[Exp], label: &str) {
    assert_eq!(
        records.len(),
        expected.len(),
        "{label}: mutation records must match ({records:?})"
    );
    for (i, (r, e)) in records.iter().zip(expected).enumerate() {
        assert_eq!(r.target, e.target.unwrap_or(target), "{label}[{i}]: target");
        assert_eq!(r.type_, e.type_, "{label}[{i}]: type");
        assert_eq!(r.added_nodes, e.added, "{label}[{i}]: addedNodes");
        assert_eq!(r.removed_nodes, e.removed, "{label}[{i}]: removedNodes");
        assert_eq!(r.previous_sibling, e.prev, "{label}[{i}]: previousSibling");
        assert_eq!(r.next_sibling, e.next, "{label}[{i}]: nextSibling");
        assert_eq!(
            r.attribute_name
                .as_ref()
                .map(ToString::to_string)
                .as_deref(),
            e.name,
            "{label}[{i}]: attributeName"
        );
        assert_eq!(
            r.attribute_namespace.as_ref().map(|n| n.uri()),
            e.ns,
            "{label}[{i}]: attributeNamespace"
        );
        assert_eq!(
            r.old_value.as_ref().map(ToString::to_string).as_deref(),
            e.old,
            "{label}[{i}]: oldValue"
        );
    }
}

/// `runMutationTest`: observa `node`, corre la mutación (que devuelve los records esperados) y
/// simula la entrega con `notify_mutation_observers()`.
fn run_mutation_test(
    doc: &mut Document,
    target: NodeId,
    init: ObserverInit,
    label: &str,
    mutate: impl FnOnce(&mut Document) -> Vec<Exp>,
) {
    let obs = doc.create_mutation_observer();
    doc.observe(obs, target, init).expect("observe");
    let expected = mutate(doc);
    let records = doc
        .notify_mutation_observers()
        .into_iter()
        .find(|(id, _)| *id == obs)
        .map(|(_, r)| r)
        .unwrap_or_default();
    check_records(target, &records, &expected, label);
    doc.disconnect_observer(obs);
}

/// Un `<p>` dentro de `parent` con un `span` (con ese texto) por cada string de `spans`.
fn p_in(doc: &mut Document, parent: NodeId, spans: &[&str]) -> NodeId {
    let p = el(doc, "p");
    append(doc, parent, p);
    for t in spans {
        let span = el(doc, "span");
        let tn = text(doc, t);
        append(doc, span, tn);
        append(doc, p, span);
    }
    p
}

/// `createFragment()`: un fragmento con los textos "11" y "22".
fn create_fragment(doc: &mut Document) -> NodeId {
    let f = doc.create_document_fragment();
    let a = text(doc, "11");
    let b = text(doc, "22");
    append(doc, f, a);
    append(doc, f, b);
    f
}

#[test]
// MutationObserver-childList.html: "childList Node.nodeValue: no mutation" y los de
// Node.textContent ("replace content mutation", "no previous content mutation", "textContent no
// mutation", "empty string mutation").
fn mutation_observer_child_list_text_content() {
    let mut h = html_document();
    let doc = &mut h.doc;
    let body = h.body;
    let both = ObserverInit {
        child_list: true,
        attributes: Some(true),
        ..Default::default()
    };

    let n00 = p_in(doc, body, &["text content"]);
    run_mutation_test(doc, n00, both.clone(), "n00", |doc| {
        doc.set_node_value(n00, Some(s("")));
        set_attr(doc, n00, "class", "dummy");
        vec![Exp::attributes("class")]
    });

    let n10 = p_in(doc, body, &["text content"]);
    let first = doc.first_child(n10).unwrap();
    run_mutation_test(doc, n10, child_list(), "n10", |doc| {
        doc.set_text_content(n10, Some(s("new data")));
        vec![Exp {
            removed: vec![first],
            added: vec![doc.first_child(n10).unwrap()],
            ..Exp::child_list()
        }]
    });

    let n11 = p_in(doc, body, &[]);
    run_mutation_test(doc, n11, child_list(), "n11", |doc| {
        doc.set_text_content(n11, Some(s("new data")));
        vec![Exp {
            added: vec![doc.first_child(n11).unwrap()],
            ..Exp::child_list()
        }]
    });

    let n12 = p_in(doc, body, &[]);
    run_mutation_test(doc, n12, both, "n12", |doc| {
        doc.set_text_content(n12, Some(s("")));
        set_attr(doc, n12, "class", "dummy");
        vec![Exp::attributes("class")]
    });

    let n13 = p_in(doc, body, &["text content"]);
    let first = doc.first_child(n13).unwrap();
    run_mutation_test(doc, n13, child_list(), "n13", |doc| {
        doc.set_text_content(n13, Some(s("")));
        vec![Exp {
            removed: vec![first],
            ..Exp::child_list()
        }]
    });
}

#[test]
// MutationObserver-childList.html: "childList Node.normalize mutation" y "childList
// Node.normalize mutations".
fn mutation_observer_child_list_normalize() {
    let mut h = html_document();
    let doc = &mut h.doc;
    let body = h.body;

    let n20 = p_in(doc, body, &[]);
    let pas = text(doc, "PAS");
    append(doc, n20, pas);
    let s_text = text(doc, "S");
    append(doc, n20, s_text);
    run_mutation_test(doc, n20, child_list(), "n20", |doc| {
        doc.normalize(n20);
        vec![Exp {
            removed: vec![s_text],
            prev: Some(pas),
            ..Exp::child_list()
        }]
    });
    assert_eq!(data(doc, pas), "PASS");

    let n21 = p_in(doc, body, &[]);
    let ch = text(doc, "CH");
    append(doc, n21, ch);
    let an = text(doc, "AN");
    append(doc, n21, an);
    let ged = text(doc, "GED");
    append(doc, n21, ged);
    run_mutation_test(doc, n21, child_list(), "n21", |doc| {
        doc.normalize(n21);
        vec![
            Exp {
                removed: vec![an],
                prev: Some(ch),
                next: Some(ged),
                ..Exp::child_list()
            },
            Exp {
                removed: vec![ged],
                prev: Some(ch),
                ..Exp::child_list()
            },
        ]
    });
}

#[test]
// MutationObserver-childList.html: los de Node.insertBefore ("addition mutation", "removal
// mutation", "removal and addition mutations", "fragment addition mutations", "fragment removal
// mutations", "last child addition mutation").
fn mutation_observer_child_list_insert_before() {
    let mut h = html_document();
    let doc = &mut h.doc;
    let body = h.body;
    let dummies = p_in(doc, body, &["text content", "text content"]);
    let d30 = doc.first_child(dummies).unwrap();
    let d35 = doc.last_child(dummies).unwrap();

    let n30 = p_in(doc, body, &["text content"]);
    let first = doc.first_child(n30).unwrap();
    run_mutation_test(doc, n30, child_list(), "n30", |doc| {
        doc.insert_before_checked(n30, d30, Some(first)).unwrap();
        vec![Exp {
            added: vec![d30],
            next: Some(first),
            ..Exp::child_list()
        }]
    });

    let n31 = p_in(doc, body, &["text content"]);
    let first = doc.first_child(n31).unwrap();
    run_mutation_test(doc, n31, child_list(), "n31", |doc| {
        let df = doc.first_child(dummies);
        doc.insert_before_checked(dummies, first, df).unwrap();
        vec![Exp {
            removed: vec![first],
            ..Exp::child_list()
        }]
    });

    let n32 = p_in(doc, body, &["AN", "CH", "GED"]);
    let [a, b, c] = doc.child_nodes(n32)[..] else {
        unreachable!()
    };
    run_mutation_test(doc, n32, child_list(), "n32", |doc| {
        doc.insert_before_checked(n32, b, Some(a)).unwrap();
        vec![
            Exp {
                removed: vec![b],
                prev: Some(a),
                next: Some(c),
                ..Exp::child_list()
            },
            Exp {
                added: vec![b],
                next: Some(a),
                ..Exp::child_list()
            },
        ]
    });

    let n33 = p_in(doc, body, &["text content"]);
    let first = doc.first_child(n33).unwrap();
    let f33 = create_fragment(doc);
    let f_nodes = doc.child_nodes(f33);
    run_mutation_test(doc, n33, child_list(), "n33", |doc| {
        doc.insert_before_checked(n33, f33, Some(first)).unwrap();
        vec![Exp {
            added: f_nodes,
            next: Some(first),
            ..Exp::child_list()
        }]
    });

    let n34 = p_in(doc, body, &["text content"]);
    let first = doc.first_child(n34).unwrap();
    let f34 = create_fragment(doc);
    let f_nodes = doc.child_nodes(f34);
    run_mutation_test(doc, f34, child_list(), "n34", |doc| {
        doc.insert_before_checked(n34, f34, Some(first)).unwrap();
        vec![Exp {
            removed: f_nodes,
            ..Exp::child_list()
        }]
    });

    let n35 = p_in(doc, body, &["text content"]);
    let first = doc.first_child(n35).unwrap();
    run_mutation_test(doc, n35, child_list(), "n35", |doc| {
        doc.insert_before_checked(n35, d35, None).unwrap();
        vec![Exp {
            added: vec![d35],
            prev: Some(first),
            ..Exp::child_list()
        }]
    });
}

#[test]
// MutationObserver-childList.html: los de Node.appendChild ("addition mutation", "removal
// mutation", "removal and addition mutations", "fragment addition mutations", "fragment removal
// mutations", "addition outside document tree mutation").
fn mutation_observer_child_list_append_child() {
    let mut h = html_document();
    let doc = &mut h.doc;
    let body = h.body;
    let dummies = p_in(doc, body, &["text content"]);
    let d40 = doc.first_child(dummies).unwrap();

    let n40 = p_in(doc, body, &["text content"]);
    let first = doc.first_child(n40).unwrap();
    run_mutation_test(doc, n40, child_list(), "n40", |doc| {
        append(doc, n40, d40);
        vec![Exp {
            added: vec![d40],
            prev: Some(first),
            ..Exp::child_list()
        }]
    });

    let n41 = p_in(doc, body, &["text content"]);
    let first = doc.first_child(n41).unwrap();
    run_mutation_test(doc, n41, child_list(), "n41", |doc| {
        append(doc, dummies, first);
        vec![Exp {
            removed: vec![first],
            ..Exp::child_list()
        }]
    });

    let n42 = p_in(doc, body, &["CH", "GED", "AN"]);
    let [a, b, c] = doc.child_nodes(n42)[..] else {
        unreachable!()
    };
    run_mutation_test(doc, n42, child_list(), "n42", |doc| {
        append(doc, n42, b);
        vec![
            Exp {
                removed: vec![b],
                prev: Some(a),
                next: Some(c),
                ..Exp::child_list()
            },
            Exp {
                added: vec![b],
                prev: Some(c),
                ..Exp::child_list()
            },
        ]
    });

    let n43 = p_in(doc, body, &["text content"]);
    let first = doc.first_child(n43).unwrap();
    let f43 = create_fragment(doc);
    let f_nodes = doc.child_nodes(f43);
    run_mutation_test(doc, n43, child_list(), "n43", |doc| {
        append(doc, n43, f43);
        vec![Exp {
            added: f_nodes,
            prev: Some(first),
            ..Exp::child_list()
        }]
    });

    let n44 = p_in(doc, body, &["text content"]);
    let f44 = create_fragment(doc);
    let f_nodes = doc.child_nodes(f44);
    run_mutation_test(doc, f44, child_list(), "n44", |doc| {
        append(doc, n44, f44);
        vec![Exp {
            removed: f_nodes,
            ..Exp::child_list()
        }]
    });

    let n45 = el(doc, "p");
    let d45 = el(doc, "span");
    run_mutation_test(doc, n45, child_list(), "n45", |doc| {
        append(doc, n45, d45);
        vec![Exp {
            added: vec![d45],
            ..Exp::child_list()
        }]
    });
}

#[test]
// MutationObserver-childList.html: "childList Node.replaceChild: replacement mutation",
// "removal mutation" y "childList Node.removeChild: removal mutation".
fn mutation_observer_child_list_replace_y_remove_child() {
    let mut h = html_document();
    let doc = &mut h.doc;
    let body = h.body;
    let dummies = p_in(doc, body, &["text content", "text content"]);
    let d50 = doc.first_child(dummies).unwrap();
    let d51 = doc.last_child(dummies).unwrap();

    let n50 = p_in(doc, body, &["text content"]);
    let first = doc.first_child(n50).unwrap();
    run_mutation_test(doc, n50, child_list(), "n50", |doc| {
        doc.replace_child(n50, d50, first).unwrap();
        vec![Exp {
            removed: vec![first],
            added: vec![d50],
            ..Exp::child_list()
        }]
    });

    let n51 = p_in(doc, body, &["text content"]);
    let first = doc.first_child(n51).unwrap();
    run_mutation_test(doc, n51, child_list(), "n51", |doc| {
        let p = doc.parent(d51).unwrap();
        doc.replace_child(p, first, d51).unwrap();
        vec![Exp {
            removed: vec![first],
            ..Exp::child_list()
        }]
    });

    let n60 = p_in(doc, body, &["text content"]);
    let first = doc.first_child(n60).unwrap();
    run_mutation_test(doc, n60, child_list(), "n60", |doc| {
        doc.remove_child(n60, first).unwrap();
        vec![Exp {
            removed: vec![first],
            ..Exp::child_list()
        }]
    });
}

#[test]
// MutationObserver-childList.html: "childList Node.replaceChild: internal replacement
// mutation".
fn mutation_observer_child_list_replace_child_interno() {
    let mut h = html_document();
    let doc = &mut h.doc;
    let n52 = p_in(doc, h.body, &["NO ", "CHANGED"]);
    let first = doc.first_child(n52).unwrap();
    let last = doc.last_child(n52).unwrap();
    run_mutation_test(doc, n52, child_list(), "n52", |doc| {
        doc.replace_child(n52, last, first).unwrap();
        vec![
            Exp {
                removed: vec![last],
                prev: Some(first),
                ..Exp::child_list()
            },
            Exp {
                removed: vec![first],
                added: vec![last],
                ..Exp::child_list()
            },
        ]
    });
}

#[test]
// MutationObserver-childList.html: "childList Node.replaceChild: self internal replacement
// mutation".
fn mutation_observer_child_list_replace_child_por_si_mismo() {
    let mut h = html_document();
    let doc = &mut h.doc;
    let n53 = p_in(doc, h.body, &["text content"]);
    let first = doc.first_child(n53).unwrap();
    run_mutation_test(doc, n53, child_list(), "n53", |doc| {
        doc.replace_child(n53, first, first).unwrap();
        vec![
            Exp {
                removed: vec![first],
                ..Exp::child_list()
            },
            Exp {
                added: vec![first],
                ..Exp::child_list()
            },
        ]
    });
}

#[test]
// MutationObserver-childList.html: "childList ParentNode.replaceChildren: move an existing
// child" y "self replacement of only child".
fn mutation_observer_child_list_replace_children() {
    let mut h = html_document();
    let doc = &mut h.doc;
    let n110 = p_in(doc, h.body, &["NO ", "CHANGED", "TOO"]);
    let nodes = doc.child_nodes(n110);
    run_mutation_test(doc, n110, child_list(), "n110", |doc| {
        doc.replace_children(n110, vec![node(nodes[1])]).unwrap();
        vec![Exp {
            removed: nodes.clone(),
            added: vec![nodes[1]],
            ..Exp::child_list()
        }]
    });
    assert_eq!(doc.child_nodes(n110), [nodes[1]]);

    let n111 = p_in(doc, h.body, &["text content"]);
    let first = doc.first_child(n111).unwrap();
    run_mutation_test(doc, n111, child_list(), "n111", |doc| {
        doc.replace_children(n111, vec![node(first)]).unwrap();
        vec![Exp {
            removed: vec![first],
            added: vec![first],
            ..Exp::child_list()
        }]
    });
}

/// `<p id=...>` con atributos, dentro de `parent`.
fn p_with_attrs(doc: &mut Document, parent: NodeId, attrs: &[(&str, &str)]) -> NodeId {
    let p = el(doc, "p");
    for &(name, value) in attrs {
        set_attr(doc, p, name, value);
    }
    append(doc, parent, p);
    p
}

#[test]
// MutationObserver-attributes.html: los de Element.id y Element.className (n, n00, n01, n02,
// n10, n11, n12, n13).
fn mutation_observer_attributes_id_y_class_name() {
    let mut h = html_document();
    let doc = &mut h.doc;
    let body = h.body;
    let attributes = ObserverInit {
        attributes: Some(true),
        ..Default::default()
    };

    let n = p_with_attrs(doc, body, &[("id", "n")]);
    run_mutation_test(doc, n, attributes, "n", |doc| {
        doc.set_id(n, s("n000"));
        vec![Exp::attributes("id")]
    });
    let n00 = p_with_attrs(doc, body, &[("id", "n00")]);
    run_mutation_test(doc, n00, attributes_old_value(), "n00", |doc| {
        doc.set_id(n00, s("n000"));
        vec![Exp::attributes_old("id", "n00")]
    });
    let n01 = p_with_attrs(doc, body, &[("id", "n01")]);
    run_mutation_test(doc, n01, attributes_old_value(), "n01", |doc| {
        doc.set_id(n01, s(""));
        vec![Exp::attributes_old("id", "n01")]
    });
    let n02 = p_with_attrs(doc, body, &[("id", "n02")]);
    run_mutation_test(doc, n02, attributes_old_value(), "n02", |doc| {
        doc.set_id(n02, s("n02"));
        set_attr(doc, n02, "class", "c01");
        vec![Exp::attributes_old("id", "n02"), Exp::attributes("class")]
    });
    let n10 = p_with_attrs(doc, body, &[("id", "n10")]);
    run_mutation_test(doc, n10, attributes_old_value(), "n10", |doc| {
        doc.set_class_name(n10, s("c01"));
        vec![Exp::attributes("class")]
    });
    let n11 = p_with_attrs(doc, body, &[("id", "n11")]);
    run_mutation_test(doc, n11, attributes_old_value(), "n11", |doc| {
        doc.set_class_name(n11, s(""));
        vec![Exp::attributes("class")]
    });
    let n12 = p_with_attrs(doc, body, &[("id", "n12"), ("class", "c01")]);
    run_mutation_test(doc, n12, attributes_old_value(), "n12", |doc| {
        doc.set_class_name(n12, s("c01"));
        vec![Exp::attributes_old("class", "c01")]
    });
    let n13 = p_with_attrs(doc, body, &[("id", "n13"), ("class", "c01 c02")]);
    run_mutation_test(doc, n13, attributes_old_value(), "n13", |doc| {
        doc.set_class_name(n13, s("c01 c02"));
        vec![Exp::attributes_old("class", "c01 c02")]
    });
}

#[test]
// MutationObserver-attributes.html: los de Element.classList (add, remove y toggle: n20–n24,
// n30–n32, n40–n45).
fn mutation_observer_attributes_class_list() {
    let mut h = html_document();
    let doc = &mut h.doc;
    let body = h.body;
    let opts = attributes_old_value();

    let n20 = p_with_attrs(doc, body, &[("id", "n20")]);
    run_mutation_test(doc, n20, opts.clone(), "n20", |doc| {
        doc.class_list(n20).add(doc, &ss(&["c01"])).unwrap();
        vec![Exp::attributes("class")]
    });
    let n21 = p_with_attrs(doc, body, &[("id", "n21")]);
    run_mutation_test(doc, n21, opts.clone(), "n21", |doc| {
        doc.class_list(n21)
            .add(doc, &ss(&["c01", "c02", "c03"]))
            .unwrap();
        vec![Exp::attributes("class")]
    });
    for (id, tokens, err) in [
        ("n22", ["c01", "", "c03"], DomException::SyntaxError),
        (
            "n23",
            ["c01", "c 02", "c03"],
            DomException::InvalidCharacterError,
        ),
    ] {
        let p = p_with_attrs(doc, body, &[("id", id)]);
        run_mutation_test(doc, p, opts.clone(), id, |doc| {
            assert_eq!(doc.class_list(p).add(doc, &ss(&tokens)), Err(err));
            doc.set_id(p, s(&format!("{id}0")));
            vec![Exp::attributes_old("id", id)]
        });
    }
    let n24 = p_with_attrs(doc, body, &[("id", "n24"), ("class", "c01 c02")]);
    run_mutation_test(doc, n24, opts.clone(), "n24", |doc| {
        doc.class_list(n24).add(doc, &ss(&["c02"])).unwrap();
        doc.set_id(n24, s("n240"));
        vec![
            Exp::attributes_old("class", "c01 c02"),
            Exp::attributes_old("id", "n24"),
        ]
    });

    let n30 = p_with_attrs(doc, body, &[("id", "n30"), ("class", "c01 c02")]);
    run_mutation_test(doc, n30, opts.clone(), "n30", |doc| {
        doc.class_list(n30).remove(doc, &ss(&["c01"])).unwrap();
        vec![Exp::attributes_old("class", "c01 c02")]
    });
    let n31 = p_with_attrs(doc, body, &[("id", "n31"), ("class", "c01 c02")]);
    run_mutation_test(doc, n31, opts.clone(), "n31", |doc| {
        doc.class_list(n31)
            .remove(doc, &ss(&["c01", "c02"]))
            .unwrap();
        vec![Exp::attributes_old("class", "c01 c02")]
    });
    let n32 = p_with_attrs(doc, body, &[("id", "n32"), ("class", "c01 c02")]);
    run_mutation_test(doc, n32, opts.clone(), "n32", |doc| {
        doc.class_list(n32).remove(doc, &ss(&["c03"])).unwrap();
        doc.set_id(n32, s("n320"));
        vec![
            Exp::attributes_old("class", "c01 c02"),
            Exp::attributes_old("id", "n32"),
        ]
    });

    // toggle: (id, token, force, records).
    let toggles: [(&str, &str, Option<bool>, bool); 6] = [
        ("n40", "c01", None, true),
        ("n41", "c03", None, true),
        ("n42", "c01", Some(false), true),
        ("n43", "c03", Some(false), false),
        ("n44", "c01", Some(true), false),
        ("n45", "c03", Some(true), true),
    ];
    for (id, token, force, mutates) in toggles {
        let p = p_with_attrs(doc, body, &[("id", id), ("class", "c01 c02")]);
        run_mutation_test(doc, p, opts.clone(), id, |doc| {
            doc.class_list(p).toggle(doc, &s(token), force).unwrap();
            if mutates {
                vec![Exp::attributes_old("class", "c01 c02")]
            } else {
                doc.set_id(p, s(&format!("{id}0")));
                vec![Exp::attributes_old("id", id)]
            }
        });
    }
}

#[test]
// MutationObserver-attributes.html: Element.attributes.value (n50, n51), Element.setAttribute
// (n60, n61, n62), Element.removeAttribute (n70, n71, n72) y Element.setAttributeNS /
// removeAttributeNS (n80, n81, n90, n91, n92).
fn mutation_observer_attributes_set_y_remove() {
    let mut h = html_document();
    let doc = &mut h.doc;
    let body = h.body;
    let opts = attributes_old_value();

    let n50 = p_with_attrs(doc, body, &[("id", "n50"), ("class", "c01 c02")]);
    run_mutation_test(doc, n50, opts.clone(), "n50", |doc| {
        for i in 0..doc.attributes_length(n50) {
            let attr = doc.attributes_item(n50, i).unwrap();
            if doc.node_name(attr) == "class" {
                doc.set_attr_value(attr, s("c03"));
            }
        }
        vec![Exp::attributes_old("class", "c01 c02")]
    });
    let n51 = p_with_attrs(doc, body, &[("id", "n51")]);
    run_mutation_test(doc, n51, opts.clone(), "n51", |doc| {
        let attr = doc.attributes_item(n51, 0).unwrap();
        doc.set_attr_value(attr, s("n51"));
        vec![Exp::attributes_old("id", "n51")]
    });

    let n60 = p_with_attrs(doc, body, &[("id", "n60")]);
    run_mutation_test(doc, n60, opts.clone(), "n60", |doc| {
        set_attr(doc, n60, "id", "n601");
        vec![Exp::attributes_old("id", "n60")]
    });
    let n61 = p_with_attrs(doc, body, &[("id", "n61"), ("class", "c01")]);
    run_mutation_test(doc, n61, opts.clone(), "n61", |doc| {
        set_attr(doc, n61, "class", "c01");
        vec![Exp::attributes_old("class", "c01")]
    });
    let n62 = p_with_attrs(doc, body, &[("id", "n62")]);
    run_mutation_test(doc, n62, opts.clone(), "n62", |doc| {
        set_attr(doc, n62, "classname", "c01");
        vec![Exp::attributes("classname")]
    });

    let n70 = p_with_attrs(doc, body, &[("id", "n70"), ("class", "c01")]);
    run_mutation_test(doc, n70, opts.clone(), "n70", |doc| {
        doc.remove_attribute(n70, &s("class"));
        vec![Exp::attributes_old("class", "c01")]
    });
    let n71 = p_with_attrs(doc, body, &[("id", "n71")]);
    run_mutation_test(doc, n71, opts.clone(), "n71", |doc| {
        doc.remove_attribute(n71, &s("class"));
        doc.set_id(n71, s("n710"));
        vec![Exp::attributes_old("id", "n71")]
    });
    let n72 = el(doc, "input");
    set_attr(doc, n72, "id", "n72");
    set_attr(doc, n72, "type", "text");
    append(doc, body, n72);
    run_mutation_test(doc, n72, opts.clone(), "n72", |doc| {
        doc.remove_attribute(n72, &s("type"));
        doc.set_id(n72, s("n720"));
        vec![
            Exp::attributes_old("type", "text"),
            Exp::attributes_old("id", "n72"),
        ]
    });

    let n80 = p_with_attrs(doc, body, &[("id", "n80")]);
    run_mutation_test(doc, n80, opts.clone(), "n80", |doc| {
        set_attr_ns(doc, n80, Some("http://example.org/"), "private", "42");
        vec![Exp {
            ns: Some("http://example.org/"),
            ..Exp::attributes("private")
        }]
    });
    let n81 = p_with_attrs(doc, body, &[("id", "n81")]);
    run_mutation_test(doc, n81, opts.clone(), "n81", |doc| {
        set_attr_ns(doc, n81, Some(XML_NS), "xml:lang", "42");
        vec![Exp {
            ns: Some(XML_NS),
            ..Exp::attributes("lang")
        }]
    });
    let n90 = p_with_attrs(doc, body, &[("id", "n90")]);
    set_attr_ns(doc, n90, Some("http://example.org/"), "private", "42");
    run_mutation_test(doc, n90, opts.clone(), "n90", |doc| {
        doc.remove_attribute_ns(n90, Some("http://example.org/"), &s("private"));
        vec![Exp {
            ns: Some("http://example.org/"),
            ..Exp::attributes_old("private", "42")
        }]
    });
    for (id, ns, local, old) in [
        ("n91", "http://example.org/", "private", "n91"),
        ("n92", XML_NS, "xml:lang", "n92"),
    ] {
        let p = p_with_attrs(doc, body, &[("id", id)]);
        run_mutation_test(doc, p, opts.clone(), id, |doc| {
            doc.remove_attribute_ns(p, Some(ns), &s(local));
            doc.set_id(p, s(&format!("{id}0")));
            vec![Exp::attributes_old("id", old)]
        });
    }
}

#[test]
// MutationObserver-attributes.html: attributeFilter (n1000, n1001), "attributeOldValue alone"
// (n2000), "attributeFilter alone" (n2001) y "childList false: no childList mutation" (n3000).
fn mutation_observer_attributes_filtros_y_opciones() {
    let mut h = html_document();
    let doc = &mut h.doc;
    let body = h.body;

    let n1000 = p_with_attrs(doc, body, &[("id", "n1000")]);
    let opts = ObserverInit {
        attribute_filter: Some(ss(&["id"])),
        ..attributes_old_value()
    };
    run_mutation_test(doc, n1000, opts, "n1000", |doc| {
        doc.set_id(n1000, s("abc"));
        doc.set_class_name(n1000, s("c01"));
        vec![Exp::attributes_old("id", "n1000")]
    });

    let n1001 = p_with_attrs(doc, body, &[("id", "n1001"), ("class", "c01")]);
    let opts = ObserverInit {
        attribute_filter: Some(ss(&["id", "class"])),
        ..attributes_old_value()
    };
    run_mutation_test(doc, n1001, opts, "n1001", |doc| {
        doc.set_id(n1001, s("abc"));
        doc.set_class_name(n1001, s("c02"));
        set_attr(doc, n1001, "lang", "fr");
        vec![
            Exp::attributes_old("id", "n1001"),
            Exp::attributes_old("class", "c01"),
        ]
    });

    let n2000 = p_with_attrs(doc, body, &[("id", "n2000")]);
    let opts = ObserverInit {
        attribute_old_value: Some(true),
        ..Default::default()
    };
    run_mutation_test(doc, n2000, opts, "n2000", |doc| {
        doc.set_id(n2000, s("abc"));
        vec![Exp::attributes_old("id", "n2000")]
    });

    let n2001 = p_with_attrs(doc, body, &[("id", "n2001"), ("class", "c01")]);
    let opts = ObserverInit {
        attribute_filter: Some(ss(&["id", "class"])),
        ..Default::default()
    };
    run_mutation_test(doc, n2001, opts, "n2001", |doc| {
        doc.set_id(n2001, s("abcd"));
        doc.set_class_name(n2001, s("c02"));
        set_attr(doc, n2001, "lang", "fr");
        vec![Exp::attributes("id"), Exp::attributes("class")]
    });

    let n3000 = p_with_attrs(doc, body, &[("id", "n3000")]);
    let opts = ObserverInit {
        subtree: true,
        child_list: false,
        attributes: Some(true),
        ..Default::default()
    };
    run_mutation_test(doc, n3000, opts, "n3000", |doc| {
        doc.set_text_content(n3000, Some(s("CHANGED")));
        doc.set_id(n3000, s("abc"));
        vec![Exp::attributes("id")]
    });
}

/// `<p>` con un único hijo de texto o comentario; devuelve ese hijo.
fn p_with_char_data(doc: &mut Document, parent: NodeId, data: &str, is_comment: bool) -> NodeId {
    let p = el(doc, "p");
    append(doc, parent, p);
    let n = if is_comment {
        comment(doc, data)
    } else {
        text(doc, data)
    };
    append(doc, p, n);
    n
}

#[test]
// MutationObserver-characterData.html: Text.data ("simple mutation without oldValue", "simple
// mutation") y "characterData/characterDataOldValue alone Text.data: simple mutation".
fn mutation_observer_character_data_data() {
    let mut h = html_document();
    let doc = &mut h.doc;
    let body = h.body;

    let n = p_with_char_data(doc, body, "text content", false);
    let opts = ObserverInit {
        character_data: Some(true),
        ..Default::default()
    };
    run_mutation_test(doc, n, opts, "n", |doc| {
        doc.set_data(n, &s("NEW VALUE"));
        vec![Exp::character_data(None)]
    });

    let n00 = p_with_char_data(doc, body, "text content", false);
    run_mutation_test(doc, n00, character_data_old_value(), "n00", |doc| {
        doc.set_data(n00, &s("CHANGED"));
        vec![Exp::character_data(Some("text content"))]
    });

    let n90 = p_with_char_data(doc, body, "CHANN", false);
    let opts = ObserverInit {
        character_data_old_value: Some(true),
        ..Default::default()
    };
    run_mutation_test(doc, n90, opts, "n90", |doc| {
        doc.set_data(n90, &s("CHANGED"));
        vec![Exp::character_data(Some("CHANN"))]
    });
}

#[test]
// MutationObserver-characterData.html: Text.appendData, insertData, deleteData y replaceData
// (n10–n12, n20–n22, n30, n31, n40, n41; null se convierte en "null").
fn mutation_observer_character_data_metodos() {
    let mut h = html_document();
    let doc = &mut h.doc;
    let body = h.body;
    type Mutate = fn(&mut Document, NodeId);
    let cases: [(&str, &'static str, Mutate, &str); 9] = [
        ("n10", "CHAN", |d, n| d.append_data(n, &s("GED")), "CHANGED"),
        ("n11", "CHANGED", |d, n| d.append_data(n, &s("")), "CHANGED"),
        (
            "n12",
            "CHANGED",
            |d, n| d.append_data(n, &s("null")),
            "CHANGEDnull",
        ),
        (
            "n20",
            "CHGED",
            |d, n| d.insert_data(n, 2, &s("AN")).unwrap(),
            "CHANGED",
        ),
        (
            "n21",
            "CHANGED",
            |d, n| d.insert_data(n, 2, &s("")).unwrap(),
            "CHANGED",
        ),
        (
            "n22",
            "CHANGED",
            |d, n| d.insert_data(n, 2, &s("null")).unwrap(),
            "CHnullANGED",
        ),
        (
            "n30",
            "CCCHANGED",
            |d, n| d.delete_data(n, 0, 2).unwrap(),
            "CHANGED",
        ),
        (
            "n40",
            "CCCHANGED",
            |d, n| d.replace_data(n, 0, 2, &s("CH")).unwrap(),
            "CHCHANGED",
        ),
        (
            "n41",
            "CHANGED",
            |d, n| d.replace_data(n, 0, 0, &s("CH")).unwrap(),
            "CHCHANGED",
        ),
    ];
    for (id, initial, mutate, after) in cases {
        let n = p_with_char_data(doc, body, initial, false);
        run_mutation_test(doc, n, character_data_old_value(), id, |doc| {
            mutate(doc, n);
            vec![Exp::character_data(Some(initial))]
        });
        assert_eq!(data(doc, n), after, "{id}");
    }

    let n31 = p_with_char_data(doc, body, "CHANGED", false);
    run_mutation_test(doc, n31, character_data_old_value(), "n31", |doc| {
        doc.delete_data(n31, 0, 0).unwrap();
        doc.set_data(n31, &s("n31"));
        vec![
            Exp::character_data(Some("CHANGED")),
            Exp::character_data(Some("CHANGED")),
        ]
    });
}

#[test]
// MutationObserver-characterData.html: "characterData ProcessingInstruction: data mutations"
// (en HTML `<?processing data?>` se parsea como comentario) y "characterData Comment: data
// mutations".
fn mutation_observer_character_data_comentarios() {
    let mut h = html_document();
    let doc = &mut h.doc;
    let body = h.body;
    for (id, initial) in [("n50", "?processing data?"), ("n60", " data ")] {
        let n = p_with_char_data(doc, body, initial, true);
        run_mutation_test(doc, n, character_data_old_value(), id, |doc| {
            doc.set_data(n, &s("CHANGED"));
            doc.delete_data(n, 0, 0).unwrap();
            doc.replace_data(n, 0, 2, &s("CH")).unwrap();
            vec![
                Exp::character_data(Some(initial)),
                Exp::character_data(Some("CHANGED")),
                Exp::character_data(Some("CHANGED")),
            ]
        });
    }
}

// --- AbortController (DOM §3; tests propios: tests/wpt/dom/abort/ no está en disco) ---

#[test]
// Propio (DOM §3.1): un AbortController nuevo tiene un signal sin abortar, sin reason, y
// throwIfAborted() no lanza.
fn abort_controller_estado_inicial() {
    let mut doc = Document::new();
    let c = doc.create_abort_controller();
    assert!(!doc.signal_aborted(c.signal));
    assert_eq!(doc.signal_reason(c.signal), None);
    assert_eq!(doc.throw_if_aborted(c.signal), Ok(()));
}

#[test]
// Propio (DOM §3.2 "signal abort"): sin reason queda un AbortError; los abort algorithms corren
// antes de disparar `abort`, el evento ve el signal ya abortado, y un segundo abort() no hace
// nada.
fn abort_controller_abort_dispara_evento_una_vez() {
    let mut doc = Document::new();
    let c = doc.create_abort_controller();
    let signal = c.signal;
    let log = Arc::new(Mutex::new(Vec::<String>::new()));
    let l = Arc::clone(&log);
    doc.add_abort_algorithm(
        signal,
        Arc::new(move |_: &mut Document| l.lock().unwrap().push("algoritmo".into())),
    );
    let l = Arc::clone(&log);
    doc.add_event_listener(
        signal,
        "abort",
        Listener::Native(Arc::new(move |doc: &mut Document, ev: &mut Event| {
            l.lock().unwrap().push(format!(
                "{} aborted={} target={:?}",
                ev.type_,
                doc.signal_aborted(signal),
                ev.target == Some(signal)
            ));
        })),
        ListenerOptions::default(),
    );
    doc.signal_abort(signal, None, &mut NoScript);
    assert!(doc.signal_aborted(signal));
    let abort_error = AbortReason::Exception(DomException::AbortError);
    let reason = Some(abort_error);
    assert_eq!(doc.signal_reason(signal), reason);
    assert_eq!(doc.throw_if_aborted(signal), Err(abort_error));
    assert_eq!(
        *log.lock().unwrap(),
        ["algoritmo", "abort aborted=true target=true"]
    );

    doc.signal_abort(signal, Some(AbortReason::Script(5)), &mut NoScript);
    assert_eq!(log.lock().unwrap().len(), 2, "el segundo abort no dispara");
    assert_eq!(doc.signal_reason(signal), reason, "la reason no cambia");
}

#[test]
// Propio (DOM §3.1–3.2): abort(reason) conserva la reason dada, AbortSignal.abort(reason) da
// un signal ya abortado, y un abort algorithm agregado a un signal abortado no corre.
fn abort_controller_reason_y_signal_abortado() {
    let mut doc = Document::new();
    let c = doc.create_abort_controller();
    doc.signal_abort(c.signal, Some(AbortReason::Script(3)), &mut NoScript);
    assert_eq!(doc.signal_reason(c.signal), Some(AbortReason::Script(3)));
    assert_eq!(doc.throw_if_aborted(c.signal), Err(AbortReason::Script(3)));

    let signal = doc.aborted_signal(Some(AbortReason::Script(7)));
    assert!(doc.signal_aborted(signal));
    assert_eq!(doc.signal_reason(signal), Some(AbortReason::Script(7)));
    let ran = Arc::new(Mutex::new(false));
    let r = Arc::clone(&ran);
    doc.add_abort_algorithm(
        signal,
        Arc::new(move |_: &mut Document| *r.lock().unwrap() = true),
    );
    doc.signal_abort(signal, None, &mut NoScript);
    assert!(!*ran.lock().unwrap());

    let signal = doc.aborted_signal(None);
    assert_eq!(
        doc.signal_reason(signal),
        Some(AbortReason::Exception(DomException::AbortError))
    );
}
