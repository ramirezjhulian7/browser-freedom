//! Construcción del árbol de cajas (CSS Display 3 y CSS 2.2 §9.2): block containers, cajas
//! inline, texto, inline-level atómicas, reemplazados, marcadores de listas y cajas anónimas.
//! Incluye la fase 1 del procesamiento de white space de CSS Text 3 §4.1.1 (la fase 2 se aplica
//! al cortar líneas).

use std::sync::Arc;

use bf_css::selectors::PseudoElement;
use bf_dom::{Document, NodeData, NodeId};
use bf_infra::Namespace;
use bf_style::StyleMap;
use bf_style::properties::types::{
    BorderStyle, Content, ContentItem, Display, Float, LengthPercentageAuto, ListStylePosition,
    ListStyleType, NonNegative, Position, Quotes,
};
use bf_style::properties::{ComputedStyle, LonghandId, PROPERTIES};
use bf_style::values::LengthPercentage;

use crate::controls::{Control, control_box};
use crate::fragment::Size;

#[derive(Debug, Clone)]
pub enum BoxKind {
    /// Block container de un elemento (block-level o out-of-flow).
    Block,
    /// Bloque anónimo que envuelve contenido inline entre bloques.
    AnonymousBlock,
    /// Caja inline no atómica.
    Inline,
    /// Texto (ya pasado por la fase 1 de white space).
    Text(String),
    /// Inline-level atómica con un block container dentro (`inline-block`, ...).
    InlineBlock,
    /// Elemento reemplazado; `natural` es su tamaño natural (o el tamaño por defecto de 300×150).
    /// `natural`: dimensiones naturales (o las de por defecto); `ratio`: relación de aspecto
    /// natural (ancho / alto) si la tiene.
    Replaced { natural: Size, ratio: Option<f32> },
    /// `<br>`: salto de línea forzado.
    LineBreak,
    /// `<wbr>`: oportunidad de corte.
    BreakOpportunity,
    /// Marcador de `list-item` con su texto.
    Marker { text: String, outside: bool },
}

#[derive(Debug, Clone)]
pub struct LayoutBox {
    pub kind: BoxKind,
    pub style: Arc<ComputedStyle>,
    pub node: Option<NodeId>,
    pub pseudo: Option<PseudoElement>,
    pub children: Vec<LayoutBox>,
    /// Control de formulario que representa la caja (con su contenido ya generado).
    pub control: Option<Control>,
    /// Para un `<fieldset>`: el índice de su leyenda renderizada entre los hijos, si tiene
    /// (HTML §15.3.13).
    pub fieldset: Option<Option<usize>>,
}

impl LayoutBox {
    pub(crate) fn new(kind: BoxKind, style: Arc<ComputedStyle>, node: Option<NodeId>) -> Self {
        LayoutBox {
            kind,
            style,
            node,
            pseudo: None,
            children: Vec::new(),
            control: None,
            fieldset: None,
        }
    }

    /// Floats y cajas con `position: absolute | fixed`.
    pub fn is_out_of_flow(&self) -> bool {
        matches!(
            self.kind,
            BoxKind::Block | BoxKind::Replaced { .. } | BoxKind::InlineBlock
        ) && (self.style.float != Float::None || self.is_absolutely_positioned())
    }

    pub fn is_absolutely_positioned(&self) -> bool {
        matches!(self.style.position, Position::Absolute | Position::Fixed)
    }

    pub fn is_float(&self) -> bool {
        self.style.float != Float::None && !self.is_absolutely_positioned()
    }

    /// Block-level en flujo.
    pub fn is_block_level(&self) -> bool {
        if self.is_out_of_flow() {
            return false;
        }
        match self.kind {
            BoxKind::Block | BoxKind::AnonymousBlock => true,
            BoxKind::Replaced { .. } => !self.style.display.is_inline_level(),
            _ => false,
        }
    }

    /// ¿Los hijos en flujo son inline-level (el block container establece un IFC)?
    pub fn has_inline_children(&self) -> bool {
        !self.children.iter().any(LayoutBox::is_block_level)
            && self.children.iter().any(|c| {
                !c.is_out_of_flow() && !matches!(c.kind, BoxKind::Marker { outside: true, .. })
            })
    }
}

/// Estilo de una caja anónima: hereda las propiedades heredables del padre y el resto toma su
/// valor inicial (CSS 2.2 §9.2.1.1).
pub fn anonymous_style(parent: &ComputedStyle, display: Display) -> Arc<ComputedStyle> {
    let mut style = ComputedStyle::initial().clone();
    for def in PROPERTIES {
        if def.inherited {
            style.set((def.get)(parent));
        }
    }
    style.custom = parent.custom.clone();
    style.set(bf_style::properties::Value::Display(display));
    let _ = LonghandId::Display;
    Arc::new(style)
}

enum Item {
    Block(LayoutBox),
    Inline(LayoutBox),
    OutOfFlow(LayoutBox),
}

struct Builder<'a> {
    doc: &'a Document,
    styles: &'a StyleMap,
    images: &'a ImageSizes,
    quote_depth: usize,
    report: LayoutReport,
}

/// Lo que el layout no soporta todavía y degradó.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayoutReport {
    /// Valor de `display` sin layout propio (`table`, `grid`, ...) → elementos que lo usan. Se
    /// maquetan como bloques (o, `ruby`, como inline).
    pub degraded_display: std::collections::BTreeMap<String, usize>,
    /// Controles de formulario dibujados (sin interactividad).
    pub form_controls: usize,
}

impl LayoutReport {
    /// ¿Se degradó algo?
    pub fn is_empty(&self) -> bool {
        self.degraded_display.is_empty()
    }
}

/// Estado de la imagen de un `<img>` (o `<input type=image>`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImageState {
    /// Decodificada, con sus dimensiones naturales en px.
    Loaded(Size),
    /// No se pudo cargar o decodificar (o el formato no está soportado).
    Broken,
}

/// Imágenes del documento por elemento.
pub type ImageSizes = std::collections::HashMap<NodeId, ImageState>;

fn is_image_element(el: &bf_dom::Element) -> bool {
    el.is_html("img")
        || (el.is_html("input")
            && el
                .attr("type")
                .is_some_and(|t| t.eq_ignore_ascii_case("image")))
}

/// Construye el árbol de cajas a partir del elemento raíz.
pub fn build_box_tree(doc: &Document, styles: &StyleMap) -> Option<LayoutBox> {
    build_box_tree_with(doc, styles, &ImageSizes::new())
}

/// Como [`build_box_tree`], con el estado de las imágenes del documento.
pub fn build_box_tree_with(
    doc: &Document,
    styles: &StyleMap,
    images: &ImageSizes,
) -> Option<LayoutBox> {
    build_box_tree_report(doc, styles, images).0
}

/// Como [`build_box_tree_with`], con el reporte de lo que se degradó.
pub fn build_box_tree_report(
    doc: &Document,
    styles: &StyleMap,
    images: &ImageSizes,
) -> (Option<LayoutBox>, LayoutReport) {
    let mut builder = Builder {
        doc,
        styles,
        images,
        quote_depth: 0,
        report: LayoutReport::default(),
    };
    let Some(root) = doc.children(doc.root()).find(|&c| doc.element(c).is_some()) else {
        return (None, builder.report);
    };
    let mut items = Vec::new();
    builder.element_items(root, &mut items);
    let root = items.into_iter().next().map(|i| match i {
        Item::Block(b) | Item::OutOfFlow(b) | Item::Inline(b) => b,
    });
    (root, builder.report)
}

/// Tamaño natural (o por defecto) y relación de aspecto de un elemento reemplazado, si `node`
/// lo es.
fn replaced_size(doc: &Document, node: NodeId, images: &ImageSizes) -> Option<(Size, Option<f32>)> {
    let el = doc.element(node)?;
    if is_image_element(el)
        && let Some(ImageState::Loaded(s)) = images.get(&node)
    {
        let ratio = (s.width > 0.0 && s.height > 0.0).then(|| s.width / s.height);
        return Some((*s, ratio));
    }
    let no_ratio = |s: Option<Size>| s.map(|s| (s, None));
    no_ratio(replaced_default(el))
}

/// Dimensiones por defecto de los reemplazados sin imagen.
fn replaced_default(el: &bf_dom::Element) -> Option<Size> {
    let attr_px = |name: &str| el.attr(name).and_then(|v| v.trim().parse::<f32>().ok());
    match el.ns {
        Namespace::Svg if el.name == "svg" => Some(Size {
            width: attr_px("width").unwrap_or(300.0),
            height: attr_px("height").unwrap_or(150.0),
        }),
        Namespace::Html => match el.name.as_str() {
            // Sin datos de imagen: "represents nothing" → dimensiones naturales 0 (HTML §15.4.2).
            "img" => Some(Size::default()),
            // Tamaño por defecto de los reemplazados sin dimensiones naturales (CSS 2.2 §10.3.2).
            "iframe" | "video" | "canvas" | "embed" | "object" => Some(Size {
                width: 300.0,
                height: 150.0,
            }),
            _ => None,
        },
        _ => None,
    }
}

const ORDINAL_SUFFIX: &str = ". ";

fn alphabetic(mut n: i64, letters: &[char]) -> String {
    if n < 1 {
        return n.to_string();
    }
    let base = letters.len() as i64;
    let mut out = Vec::new();
    while n > 0 {
        n -= 1;
        out.push(letters[(n % base) as usize]);
        n /= base;
    }
    out.iter().rev().collect()
}

fn roman(n: i64, upper: bool) -> String {
    if !(1..=3999).contains(&n) {
        return n.to_string();
    }
    let table = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut n = n;
    let mut out = String::new();
    for (v, s) in table {
        while n >= v {
            out.push_str(s);
            n -= v;
        }
    }
    if upper { out.to_uppercase() } else { out }
}

/// Texto del marcador para un counter style predefinido (CSS Counter Styles 3 §6).
fn counter_text(style: &str, n: i64) -> Option<String> {
    let latin: Vec<char> = ('a'..='z').collect();
    let greek: Vec<char> = "αβγδεζηθικλμνξοπρστυφχψω".chars().collect();
    Some(match style {
        "disc" => "• ".into(),
        "circle" => "◦ ".into(),
        "square" => "▪ ".into(),
        "disclosure-open" => "▾ ".into(),
        "disclosure-closed" => "▸ ".into(),
        "decimal" => format!("{n}{ORDINAL_SUFFIX}"),
        "decimal-leading-zero" => format!("{n:02}{ORDINAL_SUFFIX}"),
        "lower-alpha" | "lower-latin" => format!("{}{ORDINAL_SUFFIX}", alphabetic(n, &latin)),
        "upper-alpha" | "upper-latin" => {
            format!("{}{ORDINAL_SUFFIX}", alphabetic(n, &latin).to_uppercase())
        }
        "lower-greek" => format!("{}{ORDINAL_SUFFIX}", alphabetic(n, &greek)),
        "lower-roman" => format!("{}{ORDINAL_SUFFIX}", roman(n, false)),
        "upper-roman" => format!("{}{ORDINAL_SUFFIX}", roman(n, true)),
        // Counter styles desconocidos caen a `decimal`.
        _ => format!("{n}{ORDINAL_SUFFIX}"),
    })
}

impl<'a> Builder<'a> {
    fn style(&self, node: NodeId, pseudo: Option<PseudoElement>) -> Option<Arc<ComputedStyle>> {
        self.styles.get(&(node, pseudo)).cloned()
    }

    /// Ordinal del `list-item` (contador `list-item` simplificado: `start` del `ol` y `value`
    /// de los `li`).
    fn ordinal(&self, node: NodeId) -> i64 {
        let Some(parent) = self.doc.parent(node) else {
            return 1;
        };
        let parent_el = self.doc.element(parent);
        let start = parent_el
            .filter(|e| e.is_html("ol"))
            .and_then(|e| e.attr("start"))
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(1);
        let mut n = start - 1;
        for sibling in self.doc.children(parent) {
            let is_item = self
                .styles
                .get(&(sibling, None))
                .is_some_and(|s| s.display == Display::ListItem);
            if !is_item {
                continue;
            }
            match self
                .doc
                .element(sibling)
                .and_then(|e| e.attr("value"))
                .and_then(|v| v.trim().parse::<i64>().ok())
            {
                Some(v) => n = v,
                None => n += 1,
            }
            if sibling == node {
                break;
            }
        }
        n
    }

    fn marker(&mut self, node: NodeId, style: &Arc<ComputedStyle>) -> Option<LayoutBox> {
        let marker_style = self.style(node, Some(PseudoElement::Marker))?;
        let text = match &marker_style.content {
            Content::Items(items) => self.content_text(node, items, &marker_style),
            _ => match &style.list_style_type {
                ListStyleType::None => return None,
                ListStyleType::String(s) => s.clone(),
                ListStyleType::CounterStyle(name) => counter_text(name, self.ordinal(node))?,
            },
        };
        let outside = style.list_style_position == ListStylePosition::Outside;
        let mut b = LayoutBox::new(BoxKind::Marker { text, outside }, marker_style, Some(node));
        b.pseudo = Some(PseudoElement::Marker);
        Some(b)
    }

    fn content_text(
        &mut self,
        node: NodeId,
        items: &[ContentItem],
        style: &ComputedStyle,
    ) -> String {
        let mut text = String::new();
        for item in items {
            match item {
                ContentItem::String(s) => text.push_str(s),
                ContentItem::Attr(name) => {
                    if let Some(v) = self.doc.element(node).and_then(|e| e.attr(name)) {
                        text.push_str(v);
                    }
                }
                ContentItem::OpenQuote | ContentItem::CloseQuote => {
                    let open = matches!(item, ContentItem::OpenQuote);
                    if !open {
                        self.quote_depth = self.quote_depth.saturating_sub(1);
                    }
                    let quote = match &style.quotes {
                        Quotes::None => String::new(),
                        Quotes::Pairs(pairs) if !pairs.is_empty() => {
                            let (o, c) = &pairs[self.quote_depth.min(pairs.len() - 1)];
                            if open { o.clone() } else { c.clone() }
                        }
                        // `auto`: comillas tipográficas (en inglés).
                        _ => match (open, self.quote_depth) {
                            (true, 0) => "\u{201C}".into(),
                            (false, 0) => "\u{201D}".into(),
                            (true, _) => "\u{2018}".into(),
                            (false, _) => "\u{2019}".into(),
                        },
                    };
                    text.push_str(&quote);
                    if open {
                        self.quote_depth += 1;
                    }
                }
                ContentItem::NoOpenQuote => self.quote_depth += 1,
                ContentItem::NoCloseQuote => self.quote_depth = self.quote_depth.saturating_sub(1),
                // `counter()` solo se parsea por ahora; las imágenes no se cargan.
                ContentItem::Counter(..) | ContentItem::Url(_) => {}
            }
        }
        text
    }

    /// Cajas de `::before`/`::after`.
    fn pseudo_items(&mut self, node: NodeId, pseudo: PseudoElement, out: &mut Vec<Item>) {
        let Some(style) = self.style(node, Some(pseudo)) else {
            return;
        };
        let Content::Items(items) = &style.content else {
            return;
        };
        let text = self.content_text(node, &items.clone(), &style);
        let mut text_box = LayoutBox::new(BoxKind::Text(text), style.clone(), Some(node));
        text_box.pseudo = Some(pseudo);
        let display = style.display;
        let kind = if display.is_inline_level() {
            if display == Display::Inline {
                BoxKind::Inline
            } else {
                BoxKind::InlineBlock
            }
        } else {
            BoxKind::Block
        };
        let mut b = LayoutBox::new(kind, style, Some(node));
        b.pseudo = Some(pseudo);
        b.children.push(text_box);
        if matches!(b.kind, BoxKind::Block | BoxKind::InlineBlock) {
            finish_container(&mut b);
        }
        out.push(classify(b));
    }

    fn child_items(&mut self, node: NodeId, style: &Arc<ComputedStyle>, out: &mut Vec<Item>) {
        self.pseudo_items(node, PseudoElement::Before, out);
        let children: Vec<NodeId> = self.doc.children(node).collect();
        for child in children {
            match &self.doc.get(child).data {
                NodeData::Text(t) => {
                    out.push(Item::Inline(LayoutBox::new(
                        BoxKind::Text(t.clone()),
                        style.clone(),
                        Some(node),
                    )));
                }
                NodeData::Element(_) => self.element_items(child, out),
                _ => {}
            }
        }
        self.pseudo_items(node, PseudoElement::After, out);
    }

    fn element_items(&mut self, node: NodeId, out: &mut Vec<Item>) {
        let Some(style) = self.style(node, None) else {
            return;
        };
        match style.display {
            Display::None => return,
            Display::Contents => {
                self.child_items(node, &style, out);
                return;
            }
            _ => {}
        }
        let el = self.doc.element(node).expect("elemento");
        if el.is_html("br") {
            out.push(Item::Inline(LayoutBox::new(
                BoxKind::LineBreak,
                style,
                Some(node),
            )));
            return;
        }
        if el.is_html("wbr") {
            out.push(Item::Inline(LayoutBox::new(
                BoxKind::BreakOpportunity,
                style,
                Some(node),
            )));
            return;
        }
        // Imagen que no cargó con `alt`: el texto alternativo como contenido (HTML §15.4.2).
        if is_image_element(el)
            && !matches!(self.images.get(&node), Some(ImageState::Loaded(_)))
            && let Some(alt) = el.attr("alt").filter(|a| !a.trim().is_empty())
        {
            let kind = if style.display.is_inline_level() {
                BoxKind::Inline
            } else {
                BoxKind::Block
            };
            let mut b = LayoutBox::new(kind, style.clone(), Some(node));
            b.children.push(LayoutBox::new(
                BoxKind::Text(alt.to_owned()),
                style,
                Some(node),
            ));
            out.push(classify(b));
            return;
        }
        if let Some(control) = control_box(self.doc, node, el, &style) {
            self.report.form_controls += 1;
            out.push(classify(control));
            return;
        }
        if matches!(
            style.display,
            Display::Grid
                | Display::InlineGrid
                | Display::Table
                | Display::InlineTable
                | Display::TableRowGroup
                | Display::TableHeaderGroup
                | Display::TableFooterGroup
                | Display::TableRow
                | Display::TableCell
                | Display::TableColumnGroup
                | Display::TableColumn
                | Display::TableCaption
                | Display::Ruby
                | Display::RubyText
        ) {
            *self
                .report
                .degraded_display
                .entry(style.display.as_str().to_owned())
                .or_default() += 1;
        }
        if let Some((natural, ratio)) = replaced_size(self.doc, node, self.images) {
            out.push(classify(LayoutBox::new(
                BoxKind::Replaced { natural, ratio },
                style,
                Some(node),
            )));
            return;
        }
        // Contenido MathML/SVG interno (salvo `<svg>`, que es reemplazado): como inline.
        let display = style.display;
        if display == Display::Inline || matches!(display, Display::Ruby | Display::RubyText) {
            let mut inner = Vec::new();
            self.child_items(node, &style, &mut inner);
            split_inline(
                LayoutBox::new(BoxKind::Inline, style, Some(node)),
                inner,
                out,
            );
            return;
        }
        let kind = if display.is_inline_level() {
            BoxKind::InlineBlock
        } else {
            BoxKind::Block
        };
        let mut container = LayoutBox::new(kind, style.clone(), Some(node));
        let mut items = Vec::new();
        if display == Display::ListItem
            && let Some(marker) = self.marker(node, &style)
        {
            let outside = matches!(marker.kind, BoxKind::Marker { outside: true, .. });
            items.push(if outside {
                Item::OutOfFlow(marker)
            } else {
                Item::Inline(marker)
            });
        }
        self.child_items(node, &style, &mut items);
        let flex = matches!(style.display, Display::Flex | Display::InlineFlex);
        container.children = if flex {
            flex_items(items, &style)
        } else {
            assemble(items, &style)
        };
        if el.is_html("button") && !flex {
            container.control = Some(Control::Button);
        }
        if el.is_html("fieldset") {
            // La leyenda renderizada: el primer hijo `legend` que no flota ni está posicionado.
            let legend = container.children.iter().position(|c| {
                !c.is_out_of_flow()
                    && c.is_block_level()
                    && c.node
                        .and_then(|n| self.doc.element(n))
                        .is_some_and(|e| e.is_html("legend"))
            });
            container.fieldset = Some(legend);
        }
        finish_container(&mut container);
        out.push(classify(container));
    }
}

/// Hijos de un contenedor flex (Flexbox §4): cada hijo en flujo es un ítem; los tramos
/// contiguos de contenido inline van en un ítem anónimo (salvo si son solo white space); los
/// floats no flotan.
fn flex_items(items: Vec<Item>, parent_style: &Arc<ComputedStyle>) -> Vec<LayoutBox> {
    let mut out = Vec::new();
    let mut run: Vec<LayoutBox> = Vec::new();
    let flush = |run: &mut Vec<LayoutBox>, out: &mut Vec<LayoutBox>| {
        if run.iter().any(|b| !is_collapsible_whitespace(b)) {
            let mut anon = LayoutBox::new(
                BoxKind::AnonymousBlock,
                anonymous_style(parent_style, Display::Block),
                None,
            );
            anon.children = std::mem::take(run);
            finish_container(&mut anon);
            out.push(anon);
        } else {
            run.clear();
        }
    };
    for item in items {
        match item {
            Item::Inline(b) => run.push(b),
            Item::Block(b) => {
                flush(&mut run, &mut out);
                out.push(b);
            }
            Item::OutOfFlow(mut b) => {
                flush(&mut run, &mut out);
                if b.is_float() {
                    let mut style = (*b.style).clone();
                    style.float = Float::None;
                    b.style = Arc::new(style);
                }
                out.push(b);
            }
        }
    }
    flush(&mut run, &mut out);
    out
}

fn classify(b: LayoutBox) -> Item {
    if b.is_out_of_flow() {
        Item::OutOfFlow(b)
    } else if b.is_block_level() {
        Item::Block(b)
    } else {
        Item::Inline(b)
    }
}

/// Parte una caja inline alrededor de sus descendientes block-level (CSS 2.2 §9.2.1.1).
fn split_inline(template: LayoutBox, inner: Vec<Item>, out: &mut Vec<Item>) {
    if !inner.iter().any(|i| matches!(i, Item::Block(_))) {
        let mut b = template;
        b.children = inner
            .into_iter()
            .map(|i| match i {
                Item::Block(b) | Item::Inline(b) | Item::OutOfFlow(b) => b,
            })
            .collect();
        out.push(Item::Inline(b));
        return;
    }
    let mut part: Vec<LayoutBox> = Vec::new();
    let mut pieces: Vec<usize> = Vec::new();
    let flush = |part: &mut Vec<LayoutBox>, out: &mut Vec<Item>, pieces: &mut Vec<usize>| {
        if !part.is_empty() {
            let mut b = template.clone();
            b.children = std::mem::take(part);
            pieces.push(out.len());
            out.push(Item::Inline(b));
        }
    };
    for item in inner {
        match item {
            Item::Block(b) => {
                flush(&mut part, out, &mut pieces);
                out.push(Item::Block(b));
            }
            Item::Inline(b) | Item::OutOfFlow(b) => part.push(b),
        }
    }
    flush(&mut part, out, &mut pieces);
    // Los pedazos de la caja partida: el primero sin el lado final, el último sin el inicial
    // y los del medio sin ninguno (margen, borde y padding; ltr).
    let n = pieces.len();
    for (k, &idx) in pieces.iter().enumerate() {
        let (keep_start, keep_end) = (k == 0, k + 1 == n);
        if keep_start && keep_end {
            continue;
        }
        if let Item::Inline(b) = &mut out[idx] {
            let mut style = (*b.style).clone();
            let zero = || LengthPercentage::px(0.0);
            if !keep_start {
                style.margin_left = LengthPercentageAuto::LengthPercentage(zero());
                style.padding_left = NonNegative(zero());
                style.border_left_style = BorderStyle::None;
            }
            if !keep_end {
                style.margin_right = LengthPercentageAuto::LengthPercentage(zero());
                style.padding_right = NonNegative(zero());
                style.border_right_style = BorderStyle::None;
            }
            b.style = Arc::new(style);
        }
    }
}

fn is_collapsible_whitespace(b: &LayoutBox) -> bool {
    matches!(&b.kind, BoxKind::Text(t) if b.style.white_space.collapses_spaces()
        && t.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0C')))
}

/// Hijos de un block container: si hay contenido block-level, el contenido inline se envuelve en
/// bloques anónimos.
fn assemble(items: Vec<Item>, parent_style: &Arc<ComputedStyle>) -> Vec<LayoutBox> {
    if !items.iter().any(|i| matches!(i, Item::Block(_))) {
        return items
            .into_iter()
            .map(|i| match i {
                Item::Block(b) | Item::Inline(b) | Item::OutOfFlow(b) => b,
            })
            .collect();
    }
    let mut out = Vec::new();
    let mut run: Vec<LayoutBox> = Vec::new();
    let flush = |run: &mut Vec<LayoutBox>, out: &mut Vec<LayoutBox>| {
        let has_content = run
            .iter()
            .any(|b| !b.is_out_of_flow() && !is_collapsible_whitespace(b));
        if has_content {
            let mut anon = LayoutBox::new(
                BoxKind::AnonymousBlock,
                anonymous_style(parent_style, Display::Block),
                None,
            );
            anon.children = std::mem::take(run);
            finish_container(&mut anon);
            out.push(anon);
        } else {
            // Solo white space colapsable y cajas fuera de flujo: no se genera bloque anónimo.
            out.extend(run.drain(..).filter(|b| b.is_out_of_flow()));
        }
    };
    for item in items {
        match item {
            Item::Block(b) => {
                flush(&mut run, &mut out);
                out.push(b);
            }
            Item::Inline(b) | Item::OutOfFlow(b) => run.push(b),
        }
    }
    flush(&mut run, &mut out);
    out
}

/// Aplica la fase 1 de white space si el container establece un IFC.
fn finish_container(b: &mut LayoutBox) {
    if b.has_inline_children() {
        let mut prev_space = true;
        collapse_white_space(&mut b.children, &mut prev_space);
    }
}

/// CSS Text 3 §4.1.1, fase 1: colapsa espacios y tabs, transforma los segment breaks y quita
/// los espacios colapsables que siguen a otro (también a través de cajas inline). `prev_space`
/// arranca en `true` para que el espacio inicial de un IFC se quite (la fase 2 lo haría igual).
fn collapse_white_space(children: &mut Vec<LayoutBox>, prev_space: &mut bool) {
    for child in children.iter_mut() {
        let out_of_flow = child.is_out_of_flow();
        match &mut child.kind {
            BoxKind::Text(text) => {
                let ws = child.style.white_space;
                if !ws.collapses_spaces() {
                    *prev_space = false;
                    continue;
                }
                let preserve_newlines = ws.preserves_newlines();
                // Espacios y tabs alrededor de un segment break se quitan.
                let mut cleaned = String::with_capacity(text.len());
                let chars: Vec<char> = text
                    .chars()
                    .map(|c| if c == '\t' { ' ' } else { c })
                    .collect();
                for (i, &c) in chars.iter().enumerate() {
                    if c == ' ' {
                        let next_break = chars[i..].iter().find(|&&x| x != ' ') == Some(&'\n');
                        let prev_break =
                            chars[..i].iter().rev().find(|&&x| x != ' ') == Some(&'\n');
                        if next_break || prev_break {
                            continue;
                        }
                    }
                    cleaned.push(c);
                }
                let mut out = String::with_capacity(cleaned.len());
                for c in cleaned.chars() {
                    match c {
                        '\n' if preserve_newlines => {
                            out.push('\n');
                            *prev_space = true;
                        }
                        // Segment break → espacio (o nada si sigue a otro espacio).
                        '\n' | ' ' | '\r' | '\x0C' => {
                            if !*prev_space {
                                out.push(' ');
                                *prev_space = true;
                            }
                        }
                        c => {
                            out.push(c);
                            *prev_space = false;
                        }
                    }
                }
                *text = out;
            }
            BoxKind::Inline => collapse_white_space(&mut child.children, prev_space),
            BoxKind::Marker { .. } => {}
            _ if out_of_flow => {}
            _ => *prev_space = false,
        }
    }
    children.retain(|c| !matches!(&c.kind, BoxKind::Text(t) if t.is_empty()));
}
