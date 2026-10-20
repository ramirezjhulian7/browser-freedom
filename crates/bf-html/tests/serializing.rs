//! Tests del HTML fragment serialization algorithm (`innerHTML`/`outerHTML`) transcritos de WPT
//! (`tests/wpt/html/syntax/serializing-html-fragments/`).
//!
//! Los documentos de WPT se cargan en un navegador con scripting habilitado; aquí los documentos
//! no tienen browsing context, así que el scripting está deshabilitado salvo donde se indica
//! (`ParseOptions { scripting: true }` + `serialize_children(.., true)`).

use bf_dom::{Document, DomString, NodeId};
use bf_html::dom::{
    inner_html, insert_adjacent_html, outer_html, serialize_children, set_inner_html,
};
use bf_html::{ParseOptions, parse_document, parse_xml_document};

const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

fn s(x: &str) -> DomString {
    DomString::from(x)
}

fn parse(markup: &str) -> Document {
    parse_document(markup, &ParseOptions::default()).0
}

fn parse_con_scripting(markup: &str) -> Document {
    parse_document(
        markup,
        &ParseOptions {
            scripting: true,
            ..ParseOptions::default()
        },
    )
    .0
}

fn por_id(doc: &Document, id: &str) -> NodeId {
    doc.get_element_by_id(&s(id))
        .unwrap_or_else(|| panic!("no existe #{id}"))
}

fn crear(doc: &mut Document, tag: &str) -> NodeId {
    doc.create_element(&s(tag)).unwrap()
}

fn agregar(doc: &mut Document, parent: NodeId, tag: &str) -> NodeId {
    let e = crear(doc, tag);
    doc.append_child(parent, e).unwrap();
    e
}

fn texto(doc: &mut Document, parent: NodeId, data: &str) {
    let t = doc.create_text_node(s(data));
    doc.append_child(parent, t).unwrap();
}

fn inner(doc: &Document, n: NodeId) -> String {
    inner_html(doc, n).to_string()
}

fn outer(doc: &Document, n: NodeId) -> String {
    outer_html(doc, n).to_string()
}

// --- serializing.html: la tabla `expected` ---

/// `(markup del hijo de #test, innerHTML esperado, outerHTML esperado)` de serializing.html.
/// La fila de `<noscript>` está aparte (depende del scripting).
const TABLA: &[(&str, &str, &str)] = &[
    ("<span></span>", "", "<span></span>"),
    ("<span><a></a></span>", "<a></a>", "<span><a></a></span>"),
    (
        "<span><a b=c></a></span>",
        "<a b=\"c\"></a>",
        "<span><a b=\"c\"></a></span>",
    ),
    (
        "<span><a b='c'></a></span>",
        "<a b=\"c\"></a>",
        "<span><a b=\"c\"></a></span>",
    ),
    (
        "<span><a b='&'></a></span>",
        "<a b=\"&amp;\"></a>",
        "<span><a b=\"&amp;\"></a></span>",
    ),
    (
        "<span><a b='&nbsp;'></a></span>",
        "<a b=\"&nbsp;\"></a>",
        "<span><a b=\"&nbsp;\"></a></span>",
    ),
    (
        "<span><a b='\"'></a></span>",
        "<a b=\"&quot;\"></a>",
        "<span><a b=\"&quot;\"></a></span>",
    ),
    (
        "<span><a b=\"<\"></a></span>",
        "<a b=\"&lt;\"></a>",
        "<span><a b=\"&lt;\"></a></span>",
    ),
    (
        "<span><a b=\">\"></a></span>",
        "<a b=\"&gt;\"></a>",
        "<span><a b=\"&gt;\"></a></span>",
    ),
    (
        "<span><a href=\"javascript:&quot;&lt;>&quot;\"></a></span>",
        "<a href=\"javascript:&quot;&lt;&gt;&quot;\"></a>",
        "<span><a href=\"javascript:&quot;&lt;&gt;&quot;\"></a></span>",
    ),
    (
        "<span><svg xlink:href=\"a\"></svg></span>",
        "<svg xlink:href=\"a\"></svg>",
        "<span><svg xlink:href=\"a\"></svg></span>",
    ),
    (
        "<span><svg xmlns:svg=\"test\"></svg></span>",
        "<svg xmlns:svg=\"test\"></svg>",
        "<span><svg xmlns:svg=\"test\"></svg></span>",
    ),
    ("<span>a</span>", "a", "<span>a</span>"),
    ("<span>&amp;</span>", "&amp;", "<span>&amp;</span>"),
    ("<span>&nbsp;</span>", "&nbsp;", "<span>&nbsp;</span>"),
    ("<span>&lt;</span>", "&lt;", "<span>&lt;</span>"),
    ("<span>&gt;</span>", "&gt;", "<span>&gt;</span>"),
    ("<span>&quot;</span>", "\"", "<span>\"</span>"),
    (
        "<span><style><&></style></span>",
        "<style><&></style>",
        "<span><style><&></style></span>",
    ),
    (
        "<span><script type=\"test\"><&></script></span>",
        "<script type=\"test\"><&></script>",
        "<span><script type=\"test\"><&></script></span>",
    ),
    (
        "<script type=\"test\"><&></script>",
        "<&>",
        "<script type=\"test\"><&></script>",
    ),
    (
        "<span><xmp><&></xmp></span>",
        "<xmp><&></xmp>",
        "<span><xmp><&></xmp></span>",
    ),
    (
        "<span><iframe><&></iframe></span>",
        "<iframe><&></iframe>",
        "<span><iframe><&></iframe></span>",
    ),
    (
        "<span><noembed><&></noembed></span>",
        "<noembed><&></noembed>",
        "<span><noembed><&></noembed></span>",
    ),
    (
        "<span><noframes><&></noframes></span>",
        "<noframes><&></noframes>",
        "<span><noframes><&></noframes></span>",
    ),
    (
        "<span><!--data--></span>",
        "<!--data-->",
        "<span><!--data--></span>",
    ),
    (
        "<span><a><b><c></c></b><d>e</d><f><g>h</g></f></a></span>",
        "<a><b><c></c></b><d>e</d><f><g>h</g></f></a>",
        "<span><a><b><c></c></b><d>e</d><f><g>h</g></f></a></span>",
    ),
    ("<span b=c></span>", "", "<span b=\"c\"></span>"),
];

/// El `<div id=test>` de serializing.html con las filas de [`TABLA`]; devuelve sus hijos
/// elemento (`test_data`).
fn tabla_doc() -> (Document, Vec<NodeId>) {
    let mut markup = String::from("<!DOCTYPE html><div id=\"test\" style=\"display:none\">\n");
    for (m, _, _) in TABLA {
        markup.push_str(m);
        markup.push('\n');
    }
    markup.push_str("</div>");
    let doc = parse(&markup);
    let test = por_id(&doc, "test");
    let hijos = doc.element_children(test);
    assert_eq!(hijos.len(), TABLA.len());
    (doc, hijos)
}

// WPT: serializing.html — "innerHTML i ..." (las filas de `expected` salvo la de noscript)
#[test]
fn tabla_inner_html() {
    let (doc, hijos) = tabla_doc();
    for (i, (&n, (m, esperado, _))) in hijos.iter().zip(TABLA).enumerate() {
        assert_eq!(inner(&doc, n), *esperado, "innerHTML {i}: {m}");
    }
}

// WPT: serializing.html — "outerHTML i ..." (las filas de `expected` salvo la de noscript)
#[test]
fn tabla_outer_html() {
    let (doc, hijos) = tabla_doc();
    for (i, (&n, (m, _, esperado))) in hijos.iter().zip(TABLA).enumerate() {
        assert_eq!(outer(&doc, n), *esperado, "outerHTML {i}: {m}");
    }
}

// WPT: serializing.html — "innerHTML 25 <noscript><&></noscript>" (con scripting habilitado, como
// en el navegador)
#[test]
fn noscript_con_scripting_no_se_escapa() {
    let doc = parse_con_scripting("<span id=s><noscript><&></noscript></span>");
    let span = por_id(&doc, "s");
    assert_eq!(
        serialize_children(&doc, span, true).to_string(),
        "<noscript><&></noscript>"
    );
}

// WPT: serializing.html — fila de noscript en un documento sin scripting (como en escaping.html,
// "DOMParser.parseFromString"): el contenido es texto normal y se escapa.
#[test]
fn noscript_sin_scripting_se_escapa() {
    let doc = parse("<span id=s><noscript><&></noscript></span>");
    let span = por_id(&doc, "s");
    assert_eq!(inner(&doc, span), "<noscript>&lt;&amp;&gt;</noscript>");
    assert_eq!(
        outer(&doc, span),
        "<span id=\"s\"><noscript>&lt;&amp;&gt;</noscript></span>"
    );
}

// --- serializing.html: `dom_tests` ---

/// `span > svg` (ambos HTML, creados con `createElement`) con un atributo `setAttributeNS`.
fn span_svg_con_atributo(ns: &str, nombre: &str) -> (Document, NodeId) {
    let mut doc = Document::new();
    let span = crear(&mut doc, "span");
    let svg = agregar(&mut doc, span, "svg");
    doc.set_attribute_ns(svg, Some(ns), &s(nombre), s("test"))
        .unwrap();
    (doc, span)
}

// WPT: serializing.html — "innerHTML/outerHTML Attribute in the XML namespace"
#[test]
fn atributo_en_namespace_xml() {
    let (doc, span) = span_svg_con_atributo(XML_NS, "xml:foo");
    assert_eq!(inner(&doc, span), "<svg xml:foo=\"test\"></svg>");
    assert_eq!(
        outer(&doc, span),
        "<span><svg xml:foo=\"test\"></svg></span>"
    );
}

// WPT: serializing.html — "innerHTML/outerHTML Attribute in the XML namespace with the prefix not
// set to xml:"
#[test]
fn atributo_en_namespace_xml_con_otro_prefijo() {
    let (doc, span) = span_svg_con_atributo(XML_NS, "abc:foo");
    assert_eq!(inner(&doc, span), "<svg xml:foo=\"test\"></svg>");
    assert_eq!(
        outer(&doc, span),
        "<span><svg xml:foo=\"test\"></svg></span>"
    );
}

// WPT: serializing.html — "innerHTML/outerHTML Non-'xmlns' attribute in the xmlns namespace"
#[test]
fn atributo_no_xmlns_en_namespace_xmlns() {
    let (doc, span) = span_svg_con_atributo(XMLNS_NS, "xmlns:foo");
    assert_eq!(inner(&doc, span), "<svg xmlns:foo=\"test\"></svg>");
    assert_eq!(
        outer(&doc, span),
        "<span><svg xmlns:foo=\"test\"></svg></span>"
    );
}

// WPT: serializing.html — "innerHTML/outerHTML 'xmlns' attribute in the xmlns namespace"
#[test]
fn atributo_xmlns_en_namespace_xmlns() {
    let (doc, span) = span_svg_con_atributo(XMLNS_NS, "xmlns");
    assert_eq!(inner(&doc, span), "<svg xmlns=\"test\"></svg>");
    assert_eq!(outer(&doc, span), "<span><svg xmlns=\"test\"></svg></span>");
}

// WPT: serializing.html — "innerHTML/outerHTML Attribute in non-standard namespace"
#[test]
fn atributo_en_namespace_no_estandar() {
    let (doc, span) = span_svg_con_atributo("fake_ns", "abc:def");
    assert_eq!(inner(&doc, span), "<svg abc:def=\"test\"></svg>");
    assert_eq!(
        outer(&doc, span),
        "<span><svg abc:def=\"test\"></svg></span>"
    );
}

// WPT: serializing.html — "innerHTML/outerHTML <span> starting with U+000A"
#[test]
fn span_que_empieza_con_lf() {
    let mut doc = Document::new();
    let span = crear(&mut doc, "span");
    texto(&mut doc, span, "\n");
    assert_eq!(inner(&doc, span), "\n");
    assert_eq!(outer(&doc, span), "<span>\n</span>");
}

// --- serializing.html: `text_tests` × pre/textarea/listing ---

const TEXT_ELEMENTS: [&str; 3] = ["pre", "textarea", "listing"];

// WPT: serializing.html — "innerHTML/outerHTML <%text> context starting with U+000A" (no se agrega
// un LF extra)
#[test]
fn texto_contexto_que_empieza_con_lf() {
    for tag in TEXT_ELEMENTS {
        let mut doc = Document::new();
        let e = crear(&mut doc, tag);
        texto(&mut doc, e, "\n");
        assert_eq!(inner(&doc, e), "\n", "{tag}");
        assert_eq!(outer(&doc, e), format!("<{tag}>\n</{tag}>"), "{tag}");
    }
}

// WPT: serializing.html — "innerHTML/outerHTML <%text> context not starting with U+000A"
#[test]
fn texto_contexto_que_no_empieza_con_lf() {
    for tag in TEXT_ELEMENTS {
        let mut doc = Document::new();
        let e = crear(&mut doc, tag);
        texto(&mut doc, e, "a\n");
        assert_eq!(inner(&doc, e), "a\n", "{tag}");
        assert_eq!(outer(&doc, e), format!("<{tag}>a\n</{tag}>"), "{tag}");
    }
}

// WPT: serializing.html — "innerHTML/outerHTML <%text> non-context starting with U+000A" y
// "... not starting with U+000A"
#[test]
fn texto_no_contexto() {
    for tag in TEXT_ELEMENTS {
        for data in ["\n", "a\n"] {
            let mut doc = Document::new();
            let span = crear(&mut doc, "span");
            let e = agregar(&mut doc, span, tag);
            texto(&mut doc, e, data);
            assert_eq!(inner(&doc, span), format!("<{tag}>{data}</{tag}>"), "{tag}");
            assert_eq!(
                outer(&doc, span),
                format!("<span><{tag}>{data}</{tag}></span>"),
                "{tag}"
            );
        }
    }
}

// --- serializing.html: `void_tests` × elementos void ---

const VOID_ELEMENTS: [&str; 18] = [
    "area", "base", "basefont", "bgsound", "br", "col", "embed", "frame", "hr", "img", "input",
    "keygen", "link", "meta", "param", "source", "track", "wbr",
];

/// `make_void(name)`: un void con hijos `<a><!--abc--></a><b><c>abc</c></b>`.
fn make_void(doc: &mut Document, name: &str) -> NodeId {
    let v = crear(doc, name);
    let a = agregar(doc, v, "a");
    let c = doc.create_comment(s("abc"));
    doc.append_child(a, c).unwrap();
    let b = agregar(doc, v, "b");
    let c = agregar(doc, b, "c");
    texto(doc, c, "abc");
    v
}

// WPT: serializing.html — "innerHTML/outerHTML Void context node <void>"
#[test]
fn void_como_contexto() {
    for tag in VOID_ELEMENTS {
        let mut doc = Document::new();
        let v = make_void(&mut doc, tag);
        assert_eq!(inner(&doc, v), "", "{tag}");
        assert_eq!(outer(&doc, v), format!("<{tag}>"), "{tag}");
    }
}

/// `span` con `<a>test</a>`, `<b></b>` y el void en la posición `pos`.
fn span_con_void(doc: &mut Document, tag: &str, pos: usize) -> NodeId {
    let span = crear(doc, "span");
    let v = make_void(doc, tag);
    for i in 0..3 {
        match (i == pos, i - usize::from(i > pos)) {
            (true, _) => {
                doc.append_child(span, v).unwrap();
            }
            (false, 0) => {
                let a = agregar(doc, span, "a");
                texto(doc, a, "test");
            }
            (false, _) => {
                agregar(doc, span, "b");
            }
        }
    }
    span
}

// WPT: serializing.html — "innerHTML/outerHTML void as first child with following siblings",
// "void as second child with following siblings" y "void as last child with preceding siblings"
#[test]
fn void_entre_hermanos() {
    for tag in VOID_ELEMENTS {
        let esperados = [
            format!("<{tag}><a>test</a><b></b>"),
            format!("<a>test</a><{tag}><b></b>"),
            format!("<a>test</a><b></b><{tag}>"),
        ];
        for (pos, esperado) in esperados.iter().enumerate() {
            let mut doc = Document::new();
            let span = span_con_void(&mut doc, tag, pos);
            assert_eq!(inner(&doc, span), *esperado, "{tag} en {pos}");
            assert_eq!(
                outer(&doc, span),
                format!("<span>{esperado}</span>"),
                "{tag} en {pos}"
            );
        }
    }
}

// --- escaping.html ---

const HTML_ESCAPED: &str = "&amp;&nbsp;&lt;&gt;";
const HTML_UNESCAPED: &str = "&\u{a0}<>";

/// `testDoc(escapeSerialize, parsedNode)`.
fn test_doc(doc: &Document, parsed: NodeId, escaped: bool, scripting: bool) {
    let node = doc.first_child(parsed).unwrap();
    assert_eq!(
        doc.get_text_content(node).unwrap().to_string(),
        HTML_UNESCAPED
    );
    let esperado = if escaped {
        HTML_ESCAPED
    } else {
        HTML_UNESCAPED
    };
    assert_eq!(
        serialize_children(doc, node, scripting).to_string(),
        esperado
    );
}

// WPT: escaping.html — "div.innerHTML" (documento principal con scripting: `noscript` no se
// re-escapa)
#[test]
fn escaping_div_inner_html_con_scripting() {
    let mut doc = parse_con_scripting("<body></body>");
    let body = doc.body().unwrap();
    let div = agregar(&mut doc, body, "div");
    set_inner_html(
        &mut doc,
        div,
        &s(&format!("<noscript>{HTML_UNESCAPED}</noscript>")),
    );
    test_doc(&doc, div, false, true);
}

// WPT: escaping.html — "div.insertAdjacentHTML" (con scripting)
#[test]
fn escaping_insert_adjacent_html_con_scripting() {
    let mut doc = parse_con_scripting("<body></body>");
    let div = crear(&mut doc, "div");
    insert_adjacent_html(
        &mut doc,
        div,
        "afterbegin",
        &s(&format!("<noscript>{HTML_UNESCAPED}</noscript>")),
    )
    .unwrap();
    test_doc(&doc, div, false, true);
}

// WPT: escaping.html — "DOMParser.parseFromString" (documento sin scripting: se re-escapa)
#[test]
fn escaping_documento_sin_scripting() {
    let doc = parse(&format!("<body><noscript>{HTML_ESCAPED}</noscript></body>"));
    let body = doc.body().unwrap();
    test_doc(&doc, body, true, false);
}

// WPT: escaping.html — "template.innerHTML" (el contenido del template es inerte: se re-escapa)
#[test]
fn escaping_template_inner_html() {
    let mut doc = parse("<body></body>");
    let body = doc.body().unwrap();
    let template = agregar(&mut doc, body, "template");
    set_inner_html(
        &mut doc,
        template,
        &s(&format!("<noscript>{HTML_ESCAPED}</noscript>")),
    );
    let content = doc.element(template).unwrap().template_contents.unwrap();
    test_doc(&doc, content, true, false);
}

// WPT: escaping.html — "document.implementation.createHTMLDocument and innerHTML" (`<pre>` con
// `noscript` adentro en un documento sin scripting)
#[test]
fn escaping_pre_en_documento_sin_scripting() {
    let mut doc = parse("");
    let body = doc.body().unwrap();
    set_inner_html(
        &mut doc,
        body,
        &s(&format!("<pre><noscript>{HTML_ESCAPED}</noscript></pre>")),
    );
    let pre = doc.first_child(body).unwrap();
    test_doc(&doc, pre, true, false);
}

// --- outerHTML.html ---

/// `HTML5_ELEMENTS` de `html/resources/common.js` (no está en el repo; lista de elementos HTML).
const HTML5_ELEMENTS: &[&str] = &[
    "a",
    "abbr",
    "address",
    "area",
    "article",
    "aside",
    "audio",
    "b",
    "base",
    "bdi",
    "bdo",
    "blockquote",
    "body",
    "br",
    "button",
    "canvas",
    "caption",
    "cite",
    "code",
    "col",
    "colgroup",
    "data",
    "datalist",
    "dd",
    "del",
    "details",
    "dfn",
    "dialog",
    "div",
    "dl",
    "dt",
    "em",
    "embed",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
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
    "kbd",
    "label",
    "legend",
    "li",
    "link",
    "main",
    "map",
    "mark",
    "menu",
    "meta",
    "meter",
    "nav",
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
    "slot",
    "small",
    "source",
    "span",
    "strong",
    "style",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "template",
    "textarea",
    "tfoot",
    "th",
    "thead",
    "time",
    "title",
    "tr",
    "track",
    "u",
    "ul",
    "var",
    "video",
    "wbr",
];

// WPT: outerHTML.html — "Node for <ele>" (elementos no void)
#[test]
fn outer_html_elementos_no_void() {
    let mut doc = Document::new();
    for ele in HTML5_ELEMENTS.iter().filter(|e| !VOID_ELEMENTS.contains(e)) {
        let e = crear(&mut doc, ele);
        assert_eq!(outer(&doc, e), format!("<{ele}></{ele}>"));
    }
}

// WPT: outerHTML.html — "Node for <ele>" (elementos void)
#[test]
fn outer_html_elementos_void() {
    let mut doc = Document::new();
    for ele in VOID_ELEMENTS {
        let e = crear(&mut doc, ele);
        assert_eq!(outer(&doc, e), format!("<{ele}>"));
    }
}

// --- template.html ---

// WPT: template.html — "Template element content is correctly serialized"
#[test]
fn template_contenido_serializado() {
    let mut doc = parse("<!DOCTYPE html><body></body>");
    let body = doc.body().unwrap();
    insert_adjacent_html(
        &mut doc,
        body,
        "afterbegin",
        &s("<div><template><table><td></table></template></div>"),
    )
    .unwrap();
    let div = doc.first_child(body).unwrap();
    let t = doc.first_child(div).unwrap();
    assert_eq!(
        inner(&doc, t),
        "<table><tbody><tr><td></td></tr></tbody></table>"
    );
    assert_eq!(
        inner(&doc, div),
        "<template><table><tbody><tr><td></td></tr></tbody></table></template>"
    );
}

// WPT: template.html — "HTML fragment serialization algorithm should be applied to the template
// content" (sin `new Document()`/`adoptNode`: un documento por arena)
#[test]
fn template_contenido_con_namespaces() {
    let mut doc = Document::new();
    let t = crear(&mut doc, "template");
    let content = doc.element(t).unwrap().template_contents.unwrap();
    let c = doc.create_element_ns(Some("xx"), &s("div")).unwrap();
    doc.append_child(content, c).unwrap();
    doc.set_attribute_ns(c, Some(XML_NS), &s("xml:lang"), s("en-us"))
        .unwrap();
    doc.set_attribute_ns(c, Some("uri2"), &s("p:attr"), s("v"))
        .unwrap();
    assert_eq!(
        inner(&doc, t),
        "<div xml:lang=\"en-us\" p:attr=\"v\"></div>"
    );
}

// --- initial-linefeed-pre.html ---

const LINEFEED_HTML: &str = "<!doctype html>
<title>innerHTML getter for pre/textarea/listing with initial LF</title>
<div id=\"outer\">
<div id=\"inner\">
<pre id=\"pre1\">
x</pre>
<pre id=\"pre2\">

x</pre>
<textarea id=\"textarea1\">
x</textarea>
<textarea id=\"textarea2\">

x</textarea>
<listing id=\"listing1\">
x</listing>
<listing id=\"listing2\">

x</listing>
</div>
</div>
";

const EXPECTED_OUTER: &str = "\n<div id=\"inner\">\n<pre id=\"pre1\">x</pre>\n<pre id=\"pre2\">\nx</pre>\n<textarea id=\"textarea1\">x</textarea>\n<textarea id=\"textarea2\">\nx</textarea>\n<listing id=\"listing1\">x</listing>\n<listing id=\"listing2\">\nx</listing>\n</div>\n";

// WPT: initial-linefeed-pre.html — "outer div"
#[test]
fn linefeed_inicial_outer_div() {
    let doc = parse(LINEFEED_HTML);
    assert_eq!(inner(&doc, por_id(&doc, "outer")), EXPECTED_OUTER);
}

// WPT: initial-linefeed-pre.html — "inner div"
#[test]
fn linefeed_inicial_inner_div() {
    let doc = parse(LINEFEED_HTML);
    let esperado = EXPECTED_OUTER
        .replacen("\n<div id=\"inner\">", "", 1)
        .replacen("</div>\n", "", 1);
    assert_eq!(inner(&doc, por_id(&doc, "inner")), esperado);
}

// WPT: initial-linefeed-pre.html — "pre1", "pre2", "textarea1", "textarea2", "listing1", "listing2"
#[test]
fn linefeed_inicial_pre_textarea_listing() {
    let doc = parse(LINEFEED_HTML);
    for tag in TEXT_ELEMENTS {
        assert_eq!(inner(&doc, por_id(&doc, &format!("{tag}1"))), "x", "{tag}1");
        assert_eq!(
            inner(&doc, por_id(&doc, &format!("{tag}2"))),
            "\nx",
            "{tag}2"
        );
    }
}

// --- serializing-lt-gt.html ---

// WPT: serializing-lt-gt.html — "innerHTML" y "outerHTML"
#[test]
fn lt_gt_en_atributos() {
    let doc = parse("<span id=\"test\"><a b=\"<>\"></a></span>");
    let test = por_id(&doc, "test");
    assert_eq!(inner(&doc, test), "<a b=\"&lt;&gt;\"></a>");
    assert_eq!(
        outer(&doc, test),
        "<span id=\"test\"><a b=\"&lt;&gt;\"></a></span>"
    );
}

// --- processing-instructions.html ---

// WPT: processing-instructions.html — "PI with empty data"
#[test]
fn pi_con_data_vacia() {
    let mut doc = Document::new();
    let pi = doc
        .create_processing_instruction(s("target"), s(""))
        .unwrap();
    let div = crear(&mut doc, "div");
    doc.append_child(div, pi).unwrap();
    assert_eq!(inner(&doc, div), "<?target ?>");
}

// WPT: processing-instructions.html — "PI with non-empty data"
#[test]
fn pi_con_data() {
    let mut doc = Document::new();
    let pi = doc
        .create_processing_instruction(s("target"), s("data"))
        .unwrap();
    let div = crear(&mut doc, "div");
    doc.append_child(div, pi).unwrap();
    assert_eq!(inner(&doc, div), "<?target data?>");
}

// WPT: processing-instructions.html — "PI adopted from XML document" (adaptado: sin adoptar entre
// documentos, se serializa la PI dentro del documento XML)
#[test]
fn pi_de_documento_xml() {
    let doc = parse_xml_document("<root><?target data?></root>");
    let root = doc.document_element().unwrap();
    assert_eq!(inner(&doc, root), "<?target data?>");
}

// --- serializing-cdata-in-html-document.html ---

// WPT: serializing-cdata-in-html-document.html — (único subtest) una sección CDATA se serializa
// como texto escapado (adaptado: sin `adoptNode`, se serializa en el documento XML)
#[test]
fn cdata_se_serializa_como_texto_escapado() {
    let doc =
        parse_xml_document("<svg xmlns=\"http://www.w3.org/2000/svg\"><![CDATA[<img>]]></svg>");
    let el = doc.document_element().unwrap();
    assert_eq!(
        outer(&doc, el),
        "<svg xmlns=\"http://www.w3.org/2000/svg\">&lt;img&gt;</svg>"
    );
}
