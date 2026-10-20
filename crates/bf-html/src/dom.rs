//! Partes del DOM que necesitan el parser o el serializador HTML: el *HTML fragment
//! serialization algorithm* (HTML §13.3) y `innerHTML`, `outerHTML` e `insertAdjacentHTML`
//! (DOM Parsing / HTML §8.5).
//!
//! Los documentos XML también se serializan con el algoritmo HTML (el serializador XML no está
//! implementado).

use bf_dom::{Document, DomException, DomString, Element, Namespace, NodeData, NodeId};

use crate::{FragmentContext, ParseOptions, parse_fragment};

/// Elementos que "serialize as void".
const VOID: [&str; 18] = [
    "area", "base", "basefont", "bgsound", "br", "col", "embed", "frame", "hr", "img", "input",
    "keygen", "link", "meta", "param", "source", "track", "wbr",
];

fn serializes_as_void(e: &Element) -> bool {
    e.ns == Namespace::Html && VOID.iter().any(|v| e.name == *v)
}

/// "Escaping a string" (HTML §13.3): `&`, U+00A0 y, en modo atributo, `"`; `<` y `>` siempre.
pub fn escape(s: &DomString, attribute_mode: bool, out: &mut DomString) {
    for cp in s.code_points() {
        match char::from_u32(cp) {
            Some('&') => out.push_str("&amp;"),
            Some('\u{A0}') => out.push_str("&nbsp;"),
            Some('"') if attribute_mode => out.push_str("&quot;"),
            Some('<') => out.push_str("&lt;"),
            Some('>') => out.push_str("&gt;"),
            _ => out.push_code_point(cp),
        }
    }
}

/// El "serialized name" de un atributo.
fn attribute_name(a: &bf_dom::Attribute) -> DomString {
    match a.ns {
        None => a.name.clone(),
        Some(Namespace::Xml) => format!("xml:{}", a.name).into(),
        Some(Namespace::XmlNs) if a.name == "xmlns" => "xmlns".into(),
        Some(Namespace::XmlNs) => format!("xmlns:{}", a.name).into(),
        Some(Namespace::XLink) => format!("xlink:{}", a.name).into(),
        Some(_) => a.qualified_name(),
    }
}

/// HTML fragment serialization algorithm (HTML §13.3) de los hijos de `node` (o de su template
/// contents). `scripting` decide si `noscript` serializa su texto sin escapar.
pub fn serialize_children(doc: &Document, node: NodeId, scripting: bool) -> DomString {
    let mut out = DomString::new();
    serialize_children_into(doc, node, scripting, &mut out);
    out
}

fn serialize_children_into(doc: &Document, node: NodeId, scripting: bool, out: &mut DomString) {
    // 1.
    if doc.element(node).is_some_and(serializes_as_void) {
        return;
    }
    // 3.
    let node = doc
        .element(node)
        .and_then(|e| e.template_contents)
        .filter(|_| doc.element(node).is_some_and(|e| e.is_html("template")))
        .unwrap_or(node);
    for child in doc.children(node) {
        serialize_node(doc, child, scripting, out);
    }
}

fn serialize_node(doc: &Document, child: NodeId, scripting: bool, out: &mut DomString) {
    match &doc.get(child).data {
        NodeData::Element(e) => {
            let tagname = if matches!(e.ns, Namespace::Html | Namespace::Svg | Namespace::MathMl) {
                e.name.clone()
            } else {
                e.qualified_name()
            };
            out.push('<');
            out.push_dom_str(&tagname);
            for a in &e.attrs {
                out.push(' ');
                out.push_dom_str(&attribute_name(a));
                out.push_str("=\"");
                escape(&a.value, true, out);
                out.push('"');
            }
            out.push('>');
            if serializes_as_void(e) {
                return;
            }
            serialize_children_into(doc, child, scripting, out);
            out.push_str("</");
            out.push_dom_str(&tagname);
            out.push('>');
        }
        NodeData::Text(t) | NodeData::CdataSection(t) => {
            let raw = doc.parent(child).and_then(|p| doc.element(p)).is_some_and(|p| {
                p.ns == Namespace::Html
                    && (["style", "script", "xmp", "iframe", "noembed", "noframes", "plaintext"]
                        .iter()
                        .any(|n| p.name == *n)
                        || (scripting && p.name == "noscript"))
            });
            if raw {
                out.push_dom_str(t);
            } else {
                escape(t, false, out);
            }
        }
        NodeData::Comment(c) => {
            out.push_str("<!--");
            out.push_dom_str(c);
            out.push_str("-->");
        }
        NodeData::ProcessingInstruction { target, data } => {
            out.push_str("<?");
            out.push_dom_str(target);
            out.push(' ');
            out.push_dom_str(data);
            out.push('>');
        }
        NodeData::Doctype { name, .. } => {
            out.push_str("<!DOCTYPE ");
            out.push_dom_str(name);
            out.push('>');
        }
        NodeData::Document | NodeData::DocumentFragment | NodeData::Attr(_) => {}
    }
}

/// Getter de `innerHTML` de un elemento, fragmento o documento.
pub fn inner_html(doc: &Document, node: NodeId) -> DomString {
    serialize_children(doc, node, false)
}

/// Getter de `outerHTML`: el elemento mismo serializado.
pub fn outer_html(doc: &Document, el: NodeId) -> DomString {
    let mut out = DomString::new();
    serialize_node(doc, el, false, &mut out);
    out
}

/// "Fragment parsing algorithm steps" (HTML) con `context`: un `DocumentFragment` de este
/// documento con los nodos parseados.
pub fn parse_fragment_into(doc: &mut Document, context: NodeId, markup: &DomString) -> NodeId {
    let (ns, name) = match doc.element(context) {
        Some(e) => (e.ns, e.name.to_string()),
        None => (Namespace::Html, "body".to_owned()),
    };
    let ctx_ns = match ns {
        Namespace::Other(_) | Namespace::Null => Namespace::Html,
        n => n,
    };
    let (parsed, _) = parse_fragment(
        &markup.to_string_lossy(),
        FragmentContext {
            ns: ctx_ns,
            name: &name,
        },
        &ParseOptions::default(),
    );
    let fragment = doc.create(NodeData::DocumentFragment);
    // El resultado del parser: un documento con un `html` raíz cuyos hijos son el fragmento.
    if let Some(root) = parsed.children(parsed.root()).find(|&c| parsed.element(c).is_some()) {
        for child in parsed.children(root) {
            let copy = doc.import_from(&parsed, child);
            doc.append(fragment, copy);
        }
    }
    fragment
}

/// Setter de `innerHTML`.
pub fn set_inner_html(doc: &mut Document, el: NodeId, markup: &DomString) {
    let fragment = parse_fragment_into(doc, el, markup);
    let context = doc
        .element(el)
        .filter(|e| e.is_html("template"))
        .and_then(|e| e.template_contents)
        .unwrap_or(el);
    doc.replace_all(Some(fragment), context);
}

/// Setter de `outerHTML`.
pub fn set_outer_html(doc: &mut Document, el: NodeId, markup: &DomString) -> Result<(), DomException> {
    let Some(parent) = doc.parent(el) else {
        return Ok(());
    };
    let context = match &doc.get(parent).data {
        NodeData::Document => return Err(DomException::NoModificationAllowedError),
        NodeData::DocumentFragment => {
            doc.create(NodeData::Element(Element::new(Namespace::Html, "body")))
        }
        _ => parent,
    };
    let fragment = parse_fragment_into(doc, context, markup);
    doc.replace(el, fragment, parent)?;
    Ok(())
}

/// `insertAdjacentHTML(position, string)`.
pub fn insert_adjacent_html(
    doc: &mut Document,
    el: NodeId,
    position: &str,
    markup: &DomString,
) -> Result<(), DomException> {
    let position = position.to_ascii_lowercase();
    let mut context = match position.as_str() {
        "beforebegin" | "afterend" => match doc.parent(el) {
            Some(p) if !matches!(doc.get(p).data, NodeData::Document) => p,
            _ => return Err(DomException::NoModificationAllowedError),
        },
        "afterbegin" | "beforeend" => el,
        _ => return Err(DomException::SyntaxError),
    };
    let html_root = doc
        .element(context)
        .is_some_and(|e| doc.is_html && e.is_html("html"));
    if doc.element(context).is_none() || html_root {
        context = doc.create(NodeData::Element(Element::new(Namespace::Html, "body")));
    }
    let fragment = parse_fragment_into(doc, context, markup);
    match position.as_str() {
        "beforebegin" => {
            let parent = doc.parent(el).expect("comprobado");
            doc.pre_insert(fragment, parent, Some(el))?;
        }
        "afterbegin" => {
            let first = doc.first_child(el);
            doc.pre_insert(fragment, el, first)?;
        }
        "beforeend" => {
            doc.pre_insert(fragment, el, None)?;
        }
        _ => {
            let parent = doc.parent(el).expect("comprobado");
            let next = doc.next_sibling(el);
            doc.pre_insert(fragment, parent, next)?;
        }
    }
    Ok(())
}
