//! Tests del parser XML (`bf_html::xml`): namespaces (Namespaces in XML 1.0), buena formación
//! (XML 1.0), entidades del subset interno, normalización de atributos y el documento
//! `parsererror` de `DOMParser`.

use bf_dom::{Document, NodeData, NodeId};
use bf_html::{PARSERERROR_NAMESPACE, parse_xml, parse_xml_partial, parsererror_document};
use bf_infra::Namespace;

fn ok(input: &str) -> Document {
    match parse_xml(input) {
        Ok(doc) => doc,
        Err(e) => panic!("{input:?} debería ser bien formado: {e}"),
    }
}

fn err(input: &str) -> String {
    match parse_xml(input) {
        Ok(_) => panic!("{input:?} debería ser mal formado"),
        Err(e) => e.message,
    }
}

fn root(doc: &Document) -> NodeId {
    doc.document_element().expect("elemento raíz")
}

fn child(doc: &Document, n: NodeId, i: usize) -> NodeId {
    doc.children(n).nth(i).expect("hijo")
}

fn ns(doc: &Document, n: NodeId) -> Namespace {
    doc.element(n).expect("elemento").ns
}

#[test]
fn namespace_ausente_es_null() {
    let doc = ok("<root><a/></root>");
    assert_eq!(ns(&doc, root(&doc)), Namespace::Null);
    assert!(!doc.is_html);
    let doc = ok("<html xmlns='http://www.w3.org/1999/xhtml'><p xmlns=''/><q/></html>");
    let html = root(&doc);
    assert_eq!(ns(&doc, html), Namespace::Html);
    assert_eq!(ns(&doc, child(&doc, html, 0)), Namespace::Null);
    assert_eq!(ns(&doc, child(&doc, html, 1)), Namespace::Html);
}

#[test]
fn namespace_desconocido_y_prefijos() {
    let doc = ok("<a:x xmlns:a='http://example.com/ns' xmlns='urn:d'><y a:k='1' k='2'/></a:x>");
    let x = root(&doc);
    let e = doc.element(x).unwrap();
    assert_eq!(e.ns.uri(), "http://example.com/ns");
    assert_eq!(
        e.prefix.as_ref().map(|p| p.to_string()).as_deref(),
        Some("a")
    );
    assert_eq!(e.name.to_string(), "x");
    let y = doc.element(child(&doc, x, 0)).unwrap();
    assert_eq!(y.ns.uri(), "urn:d");
    // Los atributos sin prefijo no tienen namespace.
    assert_eq!(
        y.attrs[0].ns.map(|n| n.uri()),
        Some("http://example.com/ns")
    );
    assert_eq!(y.attrs[1].ns, None);
}

#[test]
fn xml_space_y_xml_lang_son_del_namespace_xml() {
    let doc = ok("<r xml:space='preserve' xml:lang='es'>  a  </r>");
    let r = doc.element(root(&doc)).unwrap();
    assert!(r.attrs.iter().all(|a| a.ns == Some(Namespace::Xml)));
    assert_eq!(doc.text_content(root(&doc)), "  a  ");
}

#[test]
fn errores_de_namespaces() {
    assert!(err("<a:b/>").contains("unbound"));
    assert!(err("<r a:k='1'/>").contains("unbound"));
    assert!(err("<r xmlns:a=''/>").contains("namespace declaration"));
    assert!(err("<r xmlns:xmlns='urn:x'/>").contains("namespace declaration"));
    assert!(err("<r xmlns:xml='urn:x'/>").contains("namespace declaration"));
    assert!(err("<r xmlns:p='http://www.w3.org/2000/xmlns/'/>").contains("namespace declaration"));
    assert!(err("<xmlns:r xmlns:xmlns='urn:x'/>").contains("namespace declaration"));
    assert!(err("<a:b:c xmlns:a='urn:x'/>").contains("qualified name"));
    assert!(
        err("<r xmlns:a='urn:x' xmlns:b='urn:x' a:k='1' b:k='2'/>").contains("duplicate attribute")
    );
    // Prefijo `xml` sin declarar y redeclarado con su URI: bien.
    ok("<r xml:lang='en' xmlns:xml='http://www.w3.org/XML/1998/namespace'/>");
}

#[test]
fn errores_de_buena_formacion() {
    assert!(err("").contains("no root"));
    assert!(err("<!-- c -->").contains("no root"));
    assert!(err("<a></b>").contains("mismatched"));
    assert!(err("<a>").contains("unclosed"));
    assert!(err("<a/><b/>").contains("junk"));
    assert!(err("<a/>text").contains("junk"));
    assert!(err("text<a/>").contains("before the root"));
    assert!(err("<a b='1' b='2'/>").contains("duplicate"));
    assert!(err("<a b='1'c='2'/>").contains("whitespace"));
    assert!(err("<a b=1/>").contains("quoted"));
    assert!(err("<a b='<'/>").contains("'<'"));
    assert!(err("<a>&nbsp;</a>").contains("undefined entity"));
    assert!(err("<a>&#0;</a>").contains("character reference"));
    assert!(err("<a>&#xD800;</a>").contains("character reference"));
    assert!(err("<a>&#X41;</a>").contains("character reference"));
    assert!(err("<a>&amp</a>").contains("unterminated"));
    assert!(err("<a>]]></a>").contains("]]>"));
    assert!(err("<a><!-- a -- b --></a>").contains("--"));
    assert!(err("<a><!-- a ---></a>").contains("--"));
    assert!(err("<a><?xml x?></a>").contains("reserved"));
    assert!(err("<a/><?xml version='1.0'?>").contains("reserved"));
    assert!(err("<a><![CDATA[x</a>").contains("CDATA"));
    assert!(err("<a>\u{1}</a>").contains("illegal character"));
    assert!(err("<1a/>").contains("name"));
    assert!(err("<?xml version='2.0'?><a/>").contains("XML declaration"));
    assert!(err("<?xml encoding='UTF-8'?><a/>").contains("XML declaration"));
    assert!(err("<!DOCTYPE a><!DOCTYPE a><a/>").contains("DOCTYPE"));
    assert!(err("<a/><!DOCTYPE a>").contains("junk"));
}

#[test]
fn posicion_del_error() {
    let e = parse_xml("<a>\n  <b></c>\n</a>").unwrap_err();
    assert_eq!((e.line, e.column), (2, 10));
    let e = parse_xml("<a>\u{FFFE}</a>").unwrap_err();
    assert_eq!((e.line, e.column), (1, 4));
}

#[test]
fn prologo_doctype_comentarios_y_pi() {
    let doc = ok(
        "\u{FEFF}<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <!-- antes --><?style href='a.css'?>\n\
         <!DOCTYPE r PUBLIC \"-//X//DTD R//EN\" \"r.dtd\" [\n\
           <!ELEMENT r (#PCDATA)>\n\
           <!ATTLIST r k CDATA \"d>\">\n\
           <!-- en el DTD --><?en-el-dtd?>\n\
         ]>\n<r/><!-- después -->",
    );
    let kinds: Vec<String> = doc
        .children(doc.root())
        .map(|n| match &doc.get(n).data {
            NodeData::Comment(c) => format!("comment {c}"),
            NodeData::ProcessingInstruction { target, data } => format!("pi {target} {data}"),
            NodeData::Doctype {
                name,
                public_id,
                system_id,
            } => format!("doctype {name} {public_id} {system_id}"),
            NodeData::Element(e) => format!("element {}", e.name),
            _ => "otro".into(),
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "comment  antes ",
            "pi style href='a.css'",
            "doctype r -//X//DTD R//EN r.dtd",
            "element r",
            "comment  después ",
        ]
    );
}

#[test]
fn cdata_y_texto() {
    let doc = ok("<a>x &lt;y&gt; <![CDATA[<b>&amp;]]>&#x1F600;&#65;</a>");
    let a = root(&doc);
    let parts: Vec<String> = doc
        .children(a)
        .map(|n| match &doc.get(n).data {
            NodeData::Text(t) => format!("text {t}"),
            NodeData::CdataSection(t) => format!("cdata {t}"),
            _ => "otro".into(),
        })
        .collect();
    assert_eq!(parts, ["text x <y> ", "cdata <b>&amp;", "text \u{1F600}A"]);
}

#[test]
fn fin_de_linea_y_normalizacion_de_atributos() {
    let doc = ok("<a k='x\r\ny\tz&#10;w&#9;' j=\"&apos;&quot;\">l1\r\nl2\rl3</a>");
    let a = doc.element(root(&doc)).unwrap();
    assert_eq!(a.attrs[0].value.to_string(), "x y z\nw\t");
    assert_eq!(a.attrs[1].value.to_string(), "'\"");
    assert_eq!(doc.text_content(root(&doc)), "l1\nl2\nl3");
}

#[test]
fn entidades_del_subset_interno() {
    let doc = ok("<!DOCTYPE r [\n\
         <!ENTITY t \"texto\">\n\
         <!ENTITY t \"ignorada\">\n\
         <!ENTITY m \"<b k='&t;'>&t;&#38;amp;</b>\">\n\
         <!ENTITY ext SYSTEM \"ext.xml\">\n\
         <!ENTITY % pe \"x\">\n\
         ]><r a='[&t;]'>&t;|&m;|&ext;</r>");
    let r = root(&doc);
    assert_eq!(
        doc.element(r).unwrap().attrs[0].value.to_string(),
        "[texto]"
    );
    assert_eq!(doc.text_content(r), "texto|texto&|");
    let b = child(&doc, r, 1);
    let be = doc.element(b).unwrap();
    assert_eq!(be.name.to_string(), "b");
    assert_eq!(be.attrs[0].value.to_string(), "texto");
}

#[test]
fn errores_de_entidades() {
    assert!(err("<!DOCTYPE r [<!ENTITY a \"&a;\">]><r>&a;</r>").contains("recursive"));
    assert!(
        err("<!DOCTYPE r [<!ENTITY a \"&b;\"><!ENTITY b \"&a;\">]><r k='&a;'/>")
            .contains("recursive")
    );
    assert!(err("<!DOCTYPE r [<!ENTITY a \"<x>\">]><r>&a;</x></r>").contains("entity"));
    assert!(err("<!DOCTYPE r [<!ENTITY a \"</r>\">]><r>&a;</r>").contains("entity"));
    assert!(err("<!DOCTYPE r [<!ENTITY a \"&#60;\">]><r k='&a;'/>").contains("'<'"));
    assert!(err("<!DOCTYPE r [<!ENTITY e SYSTEM \"e\">]><r k='&e;'/>").contains("external"));
    assert!(
        err("<!DOCTYPE r [<!NOTATION n SYSTEM \"n\"><!ENTITY u SYSTEM \"u\" NDATA n>]><r>&u;</r>")
            .contains("unparsed")
    );
    assert!(err("<!DOCTYPE r [<!ENTITY a \"%p;\">]><r/>").contains("parameter entity"));
    assert!(err("<!DOCTYPE r [ junk ]><r/>").contains("internal subset"));
    // Expansión exponencial.
    let mut dtd = String::from("<!ENTITY e0 \"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\">");
    for i in 1..12 {
        let p = i - 1;
        dtd.push_str(&format!(
            "<!ENTITY e{i} \"&e{p};&e{p};&e{p};&e{p};&e{p};&e{p};&e{p};&e{p};\">"
        ));
    }
    assert!(err(&format!("<!DOCTYPE r [{dtd}]><r>&e11;</r>")).contains("limit"));
}

#[test]
fn entidades_no_declaradas() {
    // Con un public id de HTML §14.2 se resuelven las referencias de HTML.
    let doc = ok("<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.1//EN\" \
         \"http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd\"><html>&nbsp;&eacute;</html>");
    assert_eq!(doc.text_content(root(&doc)), "\u{a0}é");
    // Con otro subset externo no se cargan ni son error.
    let doc = ok("<!DOCTYPE r SYSTEM \"r.dtd\"><r>a&nbsp;b</r>");
    assert_eq!(doc.text_content(root(&doc)), "ab");
    // Igual tras una referencia a parameter entity.
    ok("<!DOCTYPE r [<!ENTITY % p SYSTEM \"p.dtd\"> %p; ]><r>&x;</r>");
}

#[test]
fn arbol_parcial_hasta_el_error() {
    let (doc, e) = parse_xml_partial("<r><a>1</a><b>2</c></r>");
    assert!(e.is_some_and(|e| e.message.contains("mismatched")));
    let r = root(&doc);
    assert_eq!(doc.children(r).count(), 2);
    assert_eq!(doc.text_content(r), "12");
}

#[test]
fn documento_parsererror() {
    let e = parse_xml("<a>").unwrap_err();
    let doc = parsererror_document(&e);
    let r = root(&doc);
    let el = doc.element(r).unwrap();
    assert_eq!(el.name.to_string(), "parsererror");
    assert_eq!(el.ns.uri(), PARSERERROR_NAMESPACE);
    assert_eq!(doc.children(doc.root()).count(), 1);
    assert!(doc.text_content(r).contains("unclosed element <a>"));
}
