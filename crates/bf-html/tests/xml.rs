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

/// E10: el parser XML avisa al host de los `link` XHTML insertados, en orden, también los que
/// quedan antes de un error de buena formación.
#[test]
fn xml_parser_reports_inserted_links() {
    let mut seen = Vec::new();
    let (doc, error) = bf_html::parse_xml_partial_with(
        "<html xmlns='http://www.w3.org/1999/xhtml'><head><link href='a'/>\
         <x:link xmlns:x='urn:x' href='no'/><link href='b'></link></head><body><oops></body></html>",
        &mut |doc, link| {
            assert!(doc.is_connected(link));
            seen.push(doc.element(link).unwrap().attr("href").unwrap().to_owned());
        },
    );
    assert!(error.is_some());
    assert_eq!(seen, ["a", "b"]);
    assert!(doc.element(doc.document_element().unwrap()).is_some());
}

/// E10: con XML scripting support disabled (`DOMParser`) los `script` HTML y SVG nacen
/// *already started*; el parser de navegación no los toca.
#[test]
fn inert_xml_parser_marks_scripts_already_started() {
    let input = "<r><script xmlns='http://www.w3.org/1999/xhtml'>1</script>\
                 <script xmlns='http://www.w3.org/2000/svg'>2</script><script>3</script></r>";
    let started = |doc: &bf_dom::Document| -> Vec<bool> {
        doc.inclusive_descendants(doc.root())
            .into_iter()
            .filter(|&n| doc.element(n).is_some_and(|e| e.name == "script"))
            .map(|n| doc.script_state(n).already_started)
            .collect()
    };
    let inert = bf_html::parse_xml_inert(input).unwrap();
    assert_eq!(started(&inert), [true, true, false]);
    let normal = parse_xml(input).unwrap();
    assert_eq!(started(&normal), [false, false, false]);
    assert!(bf_html::parse_xml_inert("<a>").is_err());
}

fn attr(doc: &Document, n: NodeId, name: &str) -> Option<String> {
    doc.element(n)
        .expect("elemento")
        .attrs
        .iter()
        .find(|a| a.name == name)
        .map(|a| a.value.to_string())
}

/// E10: las parameter entities internas se expanden entre declaraciones del subset interno
/// (XML §4.4.8) y su texto de reemplazo debe ser declaraciones completas.
#[test]
fn parameter_entities_del_subset_interno() {
    let doc = ok("<!DOCTYPE r [\n\
         <!ENTITY % decl \"<!ENTITY t 'desde pe'>\">\n\
         %decl;\n\
         <!ENTITY % vacio \"\">%vacio;\n\
         ]><r>&t;</r>");
    assert_eq!(doc.text_content(root(&doc)), "desde pe");
    // Una declaración partida entre la entidad y el subset no está bien formada.
    assert!(!err("<!DOCTYPE r [<!ENTITY % p \"<!ENTITY t 'x'\"> %p; >]><r/>").is_empty());
    // `]` dentro de una parameter entity.
    assert!(err("<!DOCTYPE r [<!ENTITY % p \"]\"> %p; ]><r/>").contains("internal subset"));
    // Recursión: `a` declara `b` (cuyo texto es `%a;`) y lo referencia.
    assert!(
        err("<!DOCTYPE r [<!ENTITY % a \"<!ENTITY &#37; b '&#38;#37;a;'> &#37;b;\"> %a; ]><r/>")
            .contains("recursive")
    );
    // Un `%` literal en el valor de una entidad del subset interno es error.
    assert!(
        err("<!DOCTYPE r [<!ENTITY % a \"<!ENTITY &#37; b '&#37;a;'>\"> %a; ]><r/>")
            .contains("parameter entity")
    );
}

/// E10: tras una parameter entity que no se lee, las declaraciones `ENTITY`/`ATTLIST`
/// siguientes no se procesan (XML §5.1), salvo con `standalone="yes"`.
#[test]
fn declaraciones_tras_una_parameter_entity_no_leida() {
    let doc = ok("<!DOCTYPE r [\n\
         <!ENTITY antes \"a\">\n\
         <!ENTITY % ext SYSTEM \"ext.dtd\">\n\
         %ext;\n\
         <!ENTITY despues \"d\">\n\
         <!ATTLIST r k CDATA \"v\">\n\
         ]><r>&antes;&despues;</r>");
    let r = root(&doc);
    assert_eq!(doc.text_content(r), "a");
    assert_eq!(attr(&doc, r, "k"), None);

    let doc = ok("<?xml version=\"1.0\" standalone=\"yes\"?><!DOCTYPE r [\n\
         <!ENTITY % ext SYSTEM \"ext.dtd\">\n\
         %ext;\n\
         <!ENTITY despues \"d\">\n\
         ]><r>&despues;</r>");
    assert_eq!(doc.text_content(root(&doc)), "d");
    // Con standalone="yes" una entidad no declarada es error aunque haya subset externo o
    // parameter entities.
    assert!(
        err("<?xml version=\"1.0\" standalone=\"yes\"?><!DOCTYPE r SYSTEM \"r.dtd\"><r>&x;</r>")
            .contains("undefined entity")
    );
    assert!(
        err("<?xml version=\"1.0\" standalone='yes'?><!DOCTYPE r [ %nada; ]><r/>")
            .contains("undefined parameter entity")
    );
}

/// E10: `ATTLIST` del subset interno: valores por defecto, `#FIXED`, normalización de los
/// tipos que no son `CDATA` y `xmlns` por defecto (XML §3.3).
#[test]
fn attlist_del_subset_interno() {
    let doc = ok("<!DOCTYPE r [\n\
         <!ENTITY e \"ent\">\n\
         <!ATTLIST r\n\
             c CDATA \"  dos  espacios \"\n\
             t NMTOKENS \"  a   b \"\n\
             f CDATA #FIXED \"fijo&e;\"\n\
             req ID #REQUIRED\n\
             imp IDREF #IMPLIED\n\
             en (uno|dos) 'uno'\n\
             no NOTATION (n1 | n2) #IMPLIED>\n\
         <!ATTLIST r c CDATA \"ignorado\" otro CDATA \"o\">\n\
         <!ATTLIST x xmlns CDATA \"http://www.w3.org/2000/svg\" xmlns:p CDATA \"urn:p\">\n\
         ]><r t=' x  y ' req='  i1 '><x p:k='1'/><r c='dado'/></r>");
    let r = root(&doc);
    assert_eq!(attr(&doc, r, "c").as_deref(), Some("  dos  espacios "));
    assert_eq!(attr(&doc, r, "t").as_deref(), Some("x y"));
    assert_eq!(attr(&doc, r, "req").as_deref(), Some("i1"));
    assert_eq!(attr(&doc, r, "f").as_deref(), Some("fijoent"));
    assert_eq!(attr(&doc, r, "en").as_deref(), Some("uno"));
    assert_eq!(attr(&doc, r, "otro").as_deref(), Some("o"));
    assert_eq!(attr(&doc, r, "imp"), None);
    // Los defaults se agregan después de los atributos dados, en orden de declaración.
    let names: Vec<String> = doc
        .element(r)
        .unwrap()
        .attrs
        .iter()
        .map(|a| a.name.to_string())
        .collect();
    assert_eq!(names, ["t", "req", "c", "f", "en", "otro"]);
    // Un `xmlns` por defecto declara el namespace del elemento y de sus prefijos.
    let x = child(&doc, r, 0);
    assert_eq!(ns(&doc, x), Namespace::Svg);
    let k = doc
        .element(x)
        .unwrap()
        .attrs
        .iter()
        .find(|a| a.name == "k")
        .expect("p:k");
    assert_eq!(k.ns.map(|n| n.uri().to_owned()).as_deref(), Some("urn:p"));
    let inner = child(&doc, r, 1);
    assert_eq!(attr(&doc, inner, "c").as_deref(), Some("dado"));
    // Errores de sintaxis.
    assert!(err("<!DOCTYPE r [<!ATTLIST r a BOGUS #IMPLIED>]><r/>").contains("invalid type"));
    assert!(!err("<!DOCTYPE r [<!ATTLIST r a (x|) #IMPLIED>]><r/>").is_empty());
    assert!(!err("<!DOCTYPE r [<!ATTLIST r a CDATA>]><r/>").is_empty());
    assert!(err("<!DOCTYPE r [<!ATTLIST r a CDATA \"<\">]><r/>").contains("'<'"));
    assert!(err("<!DOCTYPE r [<!ATTLIST r a CDATA \"&nada;\">]><r/>").contains("undefined"));
}

/// E10: `xml:space`, `xml:lang` y `xml:base` son atributos del namespace XML con el prefijo
/// `xml`; no cambian el árbol (ni el whitespace ni la base URL).
#[test]
fn atributos_xml_space_lang_base() {
    let doc = ok(
        "<r xml:space='preserve' xml:lang='es' xml:base='http://e.com/'>\n  <a xml:space='default'>  b  </a>\n</r>",
    );
    let r = root(&doc);
    let el = doc.element(r).unwrap();
    assert_eq!(el.attrs.len(), 3);
    for a in &el.attrs {
        assert_eq!(a.ns, Some(Namespace::Xml));
        assert_eq!(
            a.prefix.as_ref().map(ToString::to_string).as_deref(),
            Some("xml")
        );
    }
    assert_eq!(doc.text_content(r), "\n    b  \n");
    // `xmlns:xml` con el namespace XML se permite; con otro, o otro prefijo para él, no.
    ok("<r xmlns:xml='http://www.w3.org/XML/1998/namespace' xml:lang='en'/>");
    assert!(err("<r xmlns:xml='urn:x'/>").contains("namespace declaration"));
    assert!(
        err("<r xmlns:y='http://www.w3.org/XML/1998/namespace'/>")
            .contains("namespace declaration")
    );
}

/// E10: al navegar, el error se informa en línea con un `parsererror` (como Blink/WebKit).
#[test]
fn parsererror_en_linea_al_navegar() {
    use bf_html::parse_xml_document_with_error_report as display;
    let name = |doc: &Document, n: NodeId| doc.element(n).unwrap().name.to_string();
    let doc = display(
        "<html xmlns='http://www.w3.org/1999/xhtml'><body><p>uno</p><p>dos</b></body></html>",
    );
    let html = root(&doc);
    let body = child(&doc, html, 0);
    let report = child(&doc, body, 0);
    assert_eq!(ns(&doc, report), Namespace::Html);
    assert_eq!(name(&doc, report), "parsererror");
    let text = doc.text_content(report);
    assert!(text.contains("error on line 1 at column"), "{text}");
    assert!(text.contains("mismatched end tag"), "{text}");
    // Lo parseado hasta el error sigue después del informe.
    assert_eq!(doc.children(body).count(), 3);
    // Sin errores no se toca nada.
    let doc = display("<r><a/></r>");
    assert_eq!(doc.children(root(&doc)).count(), 1);
    // Sin elemento raíz: html/body nuevos con el informe.
    let doc = display("<?xml version='1.0'?>");
    let html = root(&doc);
    assert_eq!(name(&doc, html), "html");
    let body = child(&doc, html, 0);
    assert_eq!(name(&doc, child(&doc, body, 0)), "parsererror");
    // Raíz SVG: pasa dentro del body, después del informe.
    let doc = display("<svg xmlns='http://www.w3.org/2000/svg'><rect></svg>");
    let html = root(&doc);
    assert_eq!(doc.children(doc.root()).count(), 1);
    let body = child(&doc, html, 0);
    assert_eq!(doc.children(body).count(), 2);
    assert_eq!(ns(&doc, child(&doc, body, 1)), Namespace::Svg);
    // Otra raíz XML: el informe es su primer hijo.
    let doc = display("<r><a></r>");
    let r = root(&doc);
    assert_eq!(name(&doc, child(&doc, r, 0)), "parsererror");
}
