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
    "alignment-baseline",
    "baseline-shift",
    "clip-path",
    "clip-rule",
    "color",
    "color-interpolation-filters",
    "display",
    "dominant-baseline",
    "fill",
    "fill-opacity",
    "fill-rule",
    "filter",
    "flood-color",
    "flood-opacity",
    "font-family",
    "font-size",
    "font-style",
    "font-weight",
    "letter-spacing",
    "marker-end",
    "marker-mid",
    "marker-start",
    "mask",
    "mask-type",
    "opacity",
    // SVG 2 §6.6 (lo leen `marker`, `svg` anidado, `symbol` e `image`).
    "overflow",
    "paint-order",
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
    // CSS Transforms 1 §6; atributo de presentación (SVG 2 §6.6).
    "transform-origin",
    "vector-effect",
    "visibility",
    "word-spacing",
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
    /// Índice del nodo del DOM de origen de un `foreignObject` en línea (su contenido HTML lo
    /// pinta `bf-paint` como el recurso `foreign-object:<índice>`).
    pub dom: Option<usize>,
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

/// Un documento de recursos (el destino de una referencia externa): XML con cualquier raíz,
/// cuyos elementos SVG se pueden referenciar por `id` aunque no estén bajo un `<svg>`.
pub(crate) fn load_resource(text: &str) -> Result<Svg, SvgError> {
    let doc = bf_html::parse_xml_document(text);
    let root = doc
        .children(doc.root())
        .find(|&n| doc.element(n).is_some())
        .ok_or_else(|| SvgError("sin elemento raíz".into()))?;
    convert_root(&doc, root, &[], Inherited::default())
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
    convert_root(doc, root, css, inherited)
}

fn convert_root(
    doc: &Document,
    root: NodeId,
    css: &[String],
    inherited: Inherited,
) -> Result<Svg, SvgError> {
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
        shadows: HashMap::new(),
        view: None,
        externals: HashMap::new(),
        ext_count: 0,
    };
    let mut dom_ids = Vec::new();
    convert(doc, root, &sheet_refs, &media, &ctx, &mut svg, &mut dom_ids);
    // Shadow trees de `use` (SVG 2 §5.6.4): cada elemento referenciado se clona bajo un host
    // propio fuera del árbol, y el clon se estila ahí (sin `:root` ni los ancestros del
    // original).
    let mut targets: Vec<usize> = svg
        .nodes
        .iter()
        .filter(|n| n.name == "use")
        .filter_map(|n| n.attr("href"))
        .filter_map(|h| h.trim().rsplit_once('#'))
        // Solo los locales: los de otro documento se estilan en el suyo.
        .filter(|(doc, _)| doc.trim().is_empty())
        .map(|(_, id)| id.to_owned())
        .filter_map(|id| svg.ids.get(&id).copied())
        .collect();
    targets.sort_unstable();
    targets.dedup();
    if !targets.is_empty() {
        let mut shadow_doc = doc.clone();
        let mut hosts = Vec::new();
        for &t in &targets {
            let host = shadow_doc.create(NodeData::Element(bf_dom::Element::new(
                bf_infra::Namespace::Svg,
                "bf-shadow-host",
            )));
            let clone = shadow_doc.clone_node(dom_ids[t], true);
            shadow_doc.append(host, clone);
            hosts.push((t, clone));
        }
        let ctx = MatchContext::for_document(&shadow_doc);
        let mut ignored = Vec::new();
        for (t, clone) in hosts {
            if let Some(c) = convert(
                &shadow_doc,
                clone,
                &sheet_refs,
                &media,
                &ctx,
                &mut svg,
                &mut ignored,
            ) {
                svg.shadows.insert(t, c);
            }
        }
    }
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
    dom_ids: &mut Vec<NodeId>,
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
            let name = el.name.to_string_lossy().into_owned();
            let dom = (name == "foreignObject").then(|| n.index());
            Node {
                name,
                attrs,
                props,
                dom,
                ..Node::default()
            }
        }
        _ => return None,
    };
    let index = svg.nodes.len();
    dom_ids.push(n);
    if let Some(id) = node.attr("id") {
        svg.ids.entry(id.to_owned()).or_insert(index);
    }
    let is_element = !node.is_text();
    svg.nodes.push(node);
    if is_element {
        let children: Vec<usize> = doc
            .children(n)
            .filter_map(|c| convert(doc, c, sheets, media, ctx, svg, dom_ids))
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
            decls.push((
                (0, (0, 0, 0), i),
                (*name).to_owned(),
                parse_component_value_list(v),
            ));
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
        if name == "font" {
            // El atajo se expande en sus longhands en su sitio de la cascada.
            for (n, v) in expand_font(&value).unwrap_or_default() {
                out.insert(n.to_owned(), v);
            }
            continue;
        }
        out.insert(name, value);
    }
    out
}

/// El atajo `font` (CSS Fonts 4 §2.8): `[<font-style> || <font-variant-css2> ||
/// <font-weight> || <font-stretch-css3>]? <font-size> [/ <line-height>]? <font-family>`, en
/// las longhands que usamos (lo que no se da vuelve a su valor inicial). `None` si no es
/// válido o es una fuente del sistema.
fn expand_font(value: &[ComponentValue]) -> Option<Vec<(&'static str, Vec<ComponentValue>)>> {
    use bf_css::syntax::TokenKind;
    let parts: Vec<&ComponentValue> = value
        .iter()
        .filter(|c| !matches!(c.token(), Some(TokenKind::Whitespace)))
        .collect();
    let mut style = None;
    let mut weight = None;
    let mut k = 0;
    // Lo de antes del tamaño: palabras clave y el peso numérico.
    let size = loop {
        let c = *parts.get(k)?;
        k += 1;
        match c.token()? {
            TokenKind::Ident(s) => match s.to_ascii_lowercase().as_str() {
                "normal" | "small-caps" => {}
                "italic" | "oblique" => style = Some(c.clone()),
                "bold" | "bolder" | "lighter" => weight = Some(c.clone()),
                s if s.ends_with("condensed") || s.ends_with("expanded") => {}
                "xx-small" | "x-small" | "small" | "medium" | "large" | "x-large" | "xx-large"
                | "xxx-large" | "larger" | "smaller" => break c.clone(),
                _ => return None,
            },
            TokenKind::Number(n) if (1.0..=1000.0).contains(&n.value) => weight = Some(c.clone()),
            TokenKind::Dimension(..) | TokenKind::Percentage(_) => break c.clone(),
            _ => return None,
        }
    };
    // `/ <line-height>`: no se usa en SVG.
    if matches!(
        parts.get(k).and_then(|c| c.token()),
        Some(TokenKind::Delim('/'))
    ) {
        k += 2;
    }
    let family: Vec<ComponentValue> = parts.get(k..)?.iter().map(|c| (*c).clone()).collect();
    if family.is_empty() {
        return None;
    }
    let initial = |s: &str| parse_component_value_list(s);
    Some(vec![
        (
            "font-style",
            style.map_or_else(|| initial("normal"), |c| vec![c]),
        ),
        (
            "font-weight",
            weight.map_or_else(|| initial("normal"), |c| vec![c]),
        ),
        ("font-size", vec![size]),
        ("font-family", family),
    ])
}
