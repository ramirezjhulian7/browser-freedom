//! Carga del documento y cascada (SVG 2 §6): atributos de presentación (§6.6, por debajo de
//! todas las reglas de autor), hojas `<style>` (con los selectores de `bf-css`) y el atributo
//! `style`, con `!important`. Cada elemento queda con el valor declarado ganador de cada
//! propiedad; la herencia y los valores computados los resuelve [`crate::build`].

use std::collections::HashMap;

use bf_css::media::MediaContext;
use bf_css::selectors::MatchContext;
use bf_css::stylesheet::{Origin, Stylesheet, matching_declarations};
use bf_css::syntax::{ComponentValue, parse_component_value_list, parse_declaration_list};
use bf_dom::{Document, NodeData, NodeId};

use crate::{Inherited, Svg, SvgError};

/// Propiedades que también se pueden dar como atributo (§6.6, más las de texto que usamos).
const PRESENTATION: &[&str] = &[
    "clip-path",
    "clip-rule",
    "color",
    "display",
    "fill",
    "fill-opacity",
    "fill-rule",
    "font-family",
    "font-size",
    "font-style",
    "font-weight",
    "letter-spacing",
    "opacity",
    "stop-color",
    "stop-opacity",
    "stroke",
    "stroke-dasharray",
    "stroke-dashoffset",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "stroke-opacity",
    "stroke-width",
    "text-anchor",
    "visibility",
];

/// Un nodo del documento, ya sin DOM: elementos (con atributos por nombre local y propiedades
/// declaradas) y texto (`name == "#text"`).
#[derive(Debug, Clone, Default)]
pub(crate) struct Node {
    pub name: String,
    pub attrs: HashMap<String, String>,
    pub props: HashMap<String, Vec<ComponentValue>>,
    pub children: Vec<usize>,
    pub text: String,
}

impl Node {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.get(name).map(String::as_str)
    }

    pub fn is_text(&self) -> bool {
        self.name == "#text"
    }
}

/// Prioridad en la cascada: (nivel, especificidad, orden). Niveles: 0 presentación, 1 hojas,
/// 2 `style`, 3 hojas `!important`, 4 `style` `!important`.
type Priority = (u8, (u32, u32, u32), usize);

pub(crate) fn load(text: &str) -> Result<Svg, SvgError> {
    let doc = bf_html::parse_xml_document(text);
    let root = doc
        .children(doc.root())
        .find(|&n| doc.element(n).is_some())
        .ok_or_else(|| SvgError("sin elemento raíz".into()))?;
    from_element(&doc, root, &[], Inherited::default())
}

/// Un `<svg>` dentro de otro documento (SVG en línea en HTML): `css` son hojas extra del
/// documento y `inherited`, lo que hereda de sus ancestros.
pub(crate) fn from_element(
    doc: &Document,
    root: NodeId,
    css: &[String],
    inherited: Inherited,
) -> Result<Svg, SvgError> {
    if doc.element(root).is_none_or(|e| e.name != "svg") {
        return Err(SvgError("el elemento raíz no es <svg>".into()));
    }
    // Hojas de `<style>` (sin `@import`: una imagen no carga recursos).
    let mut sheets: Vec<Stylesheet> = css
        .iter()
        .map(|c| Stylesheet::parse(c, Origin::Author))
        .collect();
    for n in doc.inclusive_descendants(root) {
        if doc.element(n).is_some_and(|e| e.name == "style")
            && doc
                .element(n)
                .and_then(|e| e.attr("type"))
                .is_none_or(|t| t.trim().is_empty() || t.eq_ignore_ascii_case("text/css"))
        {
            sheets.push(Stylesheet::parse(&text_content(doc, n), Origin::Author));
        }
    }
    let sheet_refs: Vec<&Stylesheet> = sheets.iter().collect();
    let media = MediaContext::default();
    let ctx = MatchContext::for_document(doc);
    let mut svg = Svg {
        nodes: Vec::new(),
        ids: HashMap::new(),
        inherited,
    };
    convert(doc, root, &sheet_refs, &media, &ctx, &mut svg);
    Ok(svg)
}

fn text_content(doc: &Document, n: NodeId) -> String {
    let mut out = String::new();
    for d in doc.descendants(n) {
        match &doc.get(d).data {
            NodeData::Text(t) | NodeData::CdataSection(t) => out.push_str(&t.to_string_lossy()),
            _ => {}
        }
    }
    out
}

/// Convierte el subárbol de `n` y devuelve el índice del nodo.
fn convert(
    doc: &Document,
    n: NodeId,
    sheets: &[&Stylesheet],
    media: &MediaContext,
    ctx: &MatchContext,
    svg: &mut Svg,
) -> Option<usize> {
    let node = match &doc.get(n).data {
        NodeData::Text(t) | NodeData::CdataSection(t) => Node {
            name: "#text".into(),
            text: t.to_string_lossy().into_owned(),
            ..Node::default()
        },
        NodeData::Element(el) => {
            let mut attrs = HashMap::new();
            for a in &el.attrs {
                let name = a.name.to_string_lossy().into_owned();
                let value = a.value.to_string_lossy().into_owned();
                // `xlink:href` solo si no hay un `href` sin namespace (SVG 2 §5.6).
                if a.ns.is_some() && attrs.contains_key(&name) {
                    continue;
                }
                attrs.insert(name, value);
            }
            let props = cascade(doc, n, &attrs, sheets, media, ctx);
            Node {
                name: el.name.to_string_lossy().into_owned(),
                attrs,
                props,
                ..Node::default()
            }
        }
        _ => return None,
    };
    let index = svg.nodes.len();
    if let Some(id) = node.attr("id") {
        svg.ids.entry(id.to_owned()).or_insert(index);
    }
    let is_element = !node.is_text();
    svg.nodes.push(node);
    if is_element {
        let children: Vec<usize> = doc
            .children(n)
            .filter_map(|c| convert(doc, c, sheets, media, ctx, svg))
            .collect();
        svg.nodes[index].children = children;
    }
    Some(index)
}

fn cascade(
    doc: &Document,
    n: NodeId,
    attrs: &HashMap<String, String>,
    sheets: &[&Stylesheet],
    media: &MediaContext,
    ctx: &MatchContext,
) -> HashMap<String, Vec<ComponentValue>> {
    let mut decls: Vec<(Priority, String, Vec<ComponentValue>)> = Vec::new();
    for (i, name) in PRESENTATION.iter().enumerate() {
        if let Some(v) = attrs.get(*name) {
            decls.push(((0, (0, 0, 0), i), (*name).to_owned(), parse_component_value_list(v)));
        }
    }
    let supports = |_: &str, _: &[ComponentValue]| true;
    for m in matching_declarations(doc, n, None, sheets, media, &supports, ctx) {
        let s = m.specificity;
        let level = if m.important { 3 } else { 1 };
        decls.push((
            (level, (s.0, s.1, s.2), m.source_order),
            m.declaration.name.to_ascii_lowercase(),
            m.declaration.value.clone(),
        ));
    }
    if let Some(style) = attrs.get("style") {
        for (i, d) in parse_declaration_list(style).into_iter().enumerate() {
            let level = if d.important { 4 } else { 2 };
            decls.push(((level, (0, 0, 0), i), d.name.to_ascii_lowercase(), d.value));
        }
    }
    decls.sort_by_key(|d| d.0);
    let mut out = HashMap::new();
    for (_, name, value) in decls {
        out.insert(name, value);
    }
    out
}
