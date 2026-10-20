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
    BorderStyle, CaptionSide, Content, ContentItem, Display, Float, LengthPercentageAuto,
    ListStylePosition, ListStyleType, NonNegative, Position, Quotes,
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
    /// Caja de tabla (CSS 2.2 §17.4), hija de su caja envoltorio. Sus hijos son grupos de
    /// columnas y grupos de filas ya normalizados (§17.2.1).
    Table,
    /// Grupo de filas, fila, grupo de columnas o columna (según su `display`).
    TablePart,
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
    /// Celdas: (colspan, rowspan; 0 = hasta el final del grupo). Columnas y grupos de
    /// columnas: (span, 1). HTML §4.9.11 y §4.9.4.
    pub span: (u32, u32),
    /// La rejilla de una caja de tabla (E4).
    pub table: Option<Box<crate::table::TableGrid>>,
    /// Ítem flex o grid: establece un contexto de formato independiente (CSS Flexbox §4,
    /// CSS Grid §6.1), así que sus márgenes no colapsan con los de sus hijos.
    pub independent: bool,
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
            span: (1, 1),
            table: None,
            independent: false,
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
            BoxKind::Block | BoxKind::AnonymousBlock | BoxKind::Table => true,
            // E4: un reemplazado con `display` interno de tabla es inline (CSS Display 3
            // §2.4: los tipos layout-internal no aplican a reemplazados).
            BoxKind::Replaced { .. } => {
                !self.style.display.is_inline_level() && !is_table_part(self.style.display)
            }
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
    /// Caja interna de tabla o `table-caption`, antes de las correcciones de §17.2.1.
    TablePart(LayoutBox),
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
        Item::Block(b) | Item::OutOfFlow(b) | Item::Inline(b) | Item::TablePart(b) => b,
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
        Namespace::Html => match el.name.as_lossy_str() {
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
        // E4: pseudo-elementos con `display` de tabla.
        if matches!(display, Display::Table | Display::InlineTable) {
            let wrapper = make_table(
                Some((node, Some(pseudo))),
                &style,
                vec![Item::Inline(text_box)],
                display == Display::InlineTable,
            );
            out.push(classify(wrapper));
            return;
        }
        if is_table_part(display) {
            let mut b = table_part_box(style, Some(node), vec![Item::Inline(text_box)]);
            b.pseudo = Some(pseudo);
            out.push(Item::TablePart(b));
            return;
        }
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
                NodeData::Text(t) | NodeData::CdataSection(t) => {
                    out.push(Item::Inline(LayoutBox::new(
                        BoxKind::Text(t.to_string()),
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
        if matches!(style.display, Display::Ruby | Display::RubyText) {
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
        // E4: tablas (CSS 2.2 §17.2 y §17.4).
        if matches!(display, Display::Table | Display::InlineTable) {
            let mut items = Vec::new();
            self.child_items(node, &style, &mut items);
            let wrapper = make_table(
                Some((node, None)),
                &style,
                items,
                display == Display::InlineTable,
            );
            out.push(classify(wrapper));
            return;
        }
        if is_table_part(display) {
            let mut items = Vec::new();
            self.child_items(node, &style, &mut items);
            let mut b = table_part_box(style.clone(), Some(node), items);
            b.span = table_span(el, display);
            out.push(Item::TablePart(b));
            return;
        }
        if display == Display::Inline || matches!(display, Display::Ruby | Display::RubyText) {
            let mut inner = Vec::new();
            self.child_items(node, &style, &mut inner);
            let inner = fixup_table_parts(inner, &style, true);
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
        let items = fixup_table_parts(items, &style, false);
        let flex = matches!(
            style.display,
            Display::Flex | Display::InlineFlex | Display::Grid | Display::InlineGrid
        );
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
            Item::Block(b) | Item::TablePart(b) => {
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
    for item in &mut out {
        item.independent = true;
    }
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
                Item::Block(b) | Item::Inline(b) | Item::OutOfFlow(b) | Item::TablePart(b) => b,
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
            Item::Block(b) | Item::TablePart(b) => {
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
                Item::Block(b) | Item::Inline(b) | Item::OutOfFlow(b) | Item::TablePart(b) => b,
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
            Item::Block(b) | Item::TablePart(b) => {
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

// --- E4: tablas (CSS 2.2 §17.2.1 y §17.4) ---

/// ¿`display` interno de tabla o `table-caption`?
fn is_table_part(d: Display) -> bool {
    matches!(
        d,
        Display::TableRowGroup
            | Display::TableHeaderGroup
            | Display::TableFooterGroup
            | Display::TableRow
            | Display::TableCell
            | Display::TableColumnGroup
            | Display::TableColumn
            | Display::TableCaption
    )
}

fn is_row_group(d: Display) -> bool {
    matches!(
        d,
        Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup
    )
}

/// "Proper table child" (§17.2.1): grupos de filas, filas, grupos de columnas, columnas y
/// títulos.
fn is_proper_table_child(d: Display) -> bool {
    is_row_group(d)
        || matches!(
            d,
            Display::TableRow
                | Display::TableColumnGroup
                | Display::TableColumn
                | Display::TableCaption
        )
}

fn part_display(item: &Item) -> Option<Display> {
    match item {
        Item::TablePart(b) => Some(b.style.display),
        _ => None,
    }
}

fn is_whitespace_item(item: &Item) -> bool {
    matches!(item, Item::Inline(b) if is_collapsible_whitespace(b))
}

/// Quita los tramos de white space colapsable cuyos vecinos inmediatos (si los hay) cumplen
/// `neighbor` (reglas 1.3 y 1.4 de §17.2.1). Con `edges`, un extremo de la lista cuenta como
/// vecino válido (regla 1.3, hijos de un contenedor tabular).
fn remove_whitespace(items: Vec<Item>, edges: bool) -> Vec<Item> {
    let n = items.len();
    let mut keep = vec![true; n];
    let mut i = 0;
    while i < n {
        if !is_whitespace_item(&items[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < n && is_whitespace_item(&items[i]) {
            i += 1;
        }
        let ok = |j: Option<usize>| match j {
            Some(j) => part_display(&items[j]).is_some(),
            None => edges,
        };
        let before = start.checked_sub(1);
        let after = (i < n).then_some(i);
        if ok(before) && ok(after) {
            for k in &mut keep[start..i] {
                *k = false;
            }
        }
    }
    items
        .into_iter()
        .zip(keep)
        .filter_map(|(it, k)| k.then_some(it))
        .collect()
}

/// Estilo de la caja envoltorio de una tabla (§17.4): `position`, `float`, márgenes y
/// desplazamientos (y, de CSS Tables 3 y los módulos de flex/grid, `clear`, `z-index` y las
/// propiedades de ítem) de la tabla; el resto, heredado o inicial.
fn wrapper_style(table: &ComputedStyle) -> Arc<ComputedStyle> {
    let mut s = (*anonymous_style(table, table.display)).clone();
    s.position = table.position;
    s.float = table.float;
    s.clear = table.clear;
    s.margin_top = table.margin_top.clone();
    s.margin_right = table.margin_right.clone();
    s.margin_bottom = table.margin_bottom.clone();
    s.margin_left = table.margin_left.clone();
    s.top = table.top.clone();
    s.right = table.right.clone();
    s.bottom = table.bottom.clone();
    s.left = table.left.clone();
    s.z_index = table.z_index;
    s.flex_grow = table.flex_grow;
    s.flex_shrink = table.flex_shrink;
    s.flex_basis = table.flex_basis.clone();
    s.align_self = table.align_self;
    s.justify_self = table.justify_self;
    s.order = table.order;
    s.grid_row_start = table.grid_row_start.clone();
    s.grid_row_end = table.grid_row_end.clone();
    s.grid_column_start = table.grid_column_start.clone();
    s.grid_column_end = table.grid_column_end.clone();
    s.original_display = table.original_display;
    Arc::new(s)
}

/// Estilo de la caja de tabla: el del elemento sin lo que pasó al envoltorio.
fn table_box_style(table: &ComputedStyle) -> Arc<ComputedStyle> {
    let mut s = table.clone();
    let initial = ComputedStyle::initial();
    s.position = Position::Static;
    s.float = Float::None;
    s.clear = initial.clear;
    let zero = || LengthPercentageAuto::LengthPercentage(LengthPercentage::px(0.0));
    s.margin_top = zero();
    s.margin_right = zero();
    s.margin_bottom = zero();
    s.margin_left = zero();
    s.top = LengthPercentageAuto::Auto;
    s.right = LengthPercentageAuto::Auto;
    s.bottom = LengthPercentageAuto::Auto;
    s.left = LengthPercentageAuto::Auto;
    s.z_index = initial.z_index;
    s.display = Display::Table;
    Arc::new(s)
}

/// Caja anónima de tabla (fila, celda, grupo) con el estilo heredado de `parent`.
fn anonymous_part(parent: &ComputedStyle, display: Display, kind: BoxKind) -> LayoutBox {
    LayoutBox::new(kind, anonymous_style(parent, display), None)
}

/// Fila anónima alrededor de `items` (reglas 2.1 y 2.2).
fn anonymous_row(items: Vec<Item>, parent: &ComputedStyle) -> LayoutBox {
    let mut row = anonymous_part(parent, Display::TableRow, BoxKind::TablePart);
    row.children = row_children(items, &row.style.clone());
    row
}

/// Hijos de una fila: cada tramo de hijos que no son celdas va en una celda anónima (regla
/// 2.3).
fn row_children(items: Vec<Item>, row: &Arc<ComputedStyle>) -> Vec<LayoutBox> {
    let items = remove_whitespace(items, true);
    let mut out = Vec::new();
    let mut run: Vec<Item> = Vec::new();
    let flush = |run: &mut Vec<Item>, out: &mut Vec<LayoutBox>| {
        if run.is_empty() {
            return;
        }
        let mut cell = anonymous_part(row, Display::TableCell, BoxKind::AnonymousBlock);
        let style = cell.style.clone();
        let inner = fixup_table_parts(std::mem::take(run), &style, false);
        cell.children = assemble(inner, &style);
        finish_container(&mut cell);
        out.push(cell);
    };
    for item in items {
        match item {
            Item::TablePart(b) if b.style.display == Display::TableCell => {
                flush(&mut run, &mut out);
                out.push(b);
            }
            other => run.push(other),
        }
    }
    flush(&mut run, &mut out);
    out
}

/// Hijos de un grupo de filas: los tramos que no son filas van en filas anónimas (regla 2.2).
fn group_children(items: Vec<Item>, group: &Arc<ComputedStyle>) -> Vec<LayoutBox> {
    let items = remove_whitespace(items, true);
    let mut out = Vec::new();
    let mut run: Vec<Item> = Vec::new();
    for item in items {
        match item {
            Item::TablePart(b) if b.style.display == Display::TableRow => {
                if !run.is_empty() {
                    out.push(anonymous_row(std::mem::take(&mut run), group));
                }
                out.push(b);
            }
            other => run.push(other),
        }
    }
    if !run.is_empty() {
        out.push(anonymous_row(run, group));
    }
    out
}

/// La caja de una parte interna de tabla o un título, con sus hijos ya corregidos.
fn table_part_box(style: Arc<ComputedStyle>, node: Option<NodeId>, items: Vec<Item>) -> LayoutBox {
    let display = style.display;
    match display {
        Display::TableCell | Display::TableCaption => {
            let items = fixup_table_parts(items, &style, false);
            let mut b = LayoutBox::new(BoxKind::Block, style.clone(), node);
            b.children = assemble(items, &style);
            finish_container(&mut b);
            b
        }
        Display::TableRow => {
            let mut b = LayoutBox::new(BoxKind::TablePart, style.clone(), node);
            b.children = row_children(items, &style);
            b
        }
        Display::TableColumnGroup => {
            // Regla 1.2: solo cuentan las columnas.
            let mut b = LayoutBox::new(BoxKind::TablePart, style, node);
            b.children = items
                .into_iter()
                .filter_map(|i| match i {
                    Item::TablePart(c) if c.style.display == Display::TableColumn => Some(c),
                    _ => None,
                })
                .collect();
            b
        }
        // Regla 1.1: los hijos de una columna no se muestran.
        Display::TableColumn => LayoutBox::new(BoxKind::TablePart, style, node),
        _ => {
            let mut b = LayoutBox::new(BoxKind::TablePart, style.clone(), node);
            b.children = group_children(items, &style);
            b
        }
    }
}

/// `colspan`/`rowspan` de `td`/`th` y `span` de `col`/`colgroup` (HTML §4.9.11 y §4.9.3).
fn table_span(el: &bf_dom::Element, display: Display) -> (u32, u32) {
    let attr = |name: &str| {
        el.attr(name)
            .and_then(bf_style::hints::parse_non_negative_integer)
    };
    match display {
        Display::TableCell if el.is_html("td") || el.is_html("th") => {
            let colspan = attr("colspan").filter(|&v| v > 0).unwrap_or(1).min(1000);
            let rowspan = attr("rowspan").unwrap_or(1).min(65534);
            (colspan, rowspan)
        }
        Display::TableColumn | Display::TableColumnGroup
            if el.is_html("col") || el.is_html("colgroup") =>
        {
            (attr("span").filter(|&v| v > 0).unwrap_or(1).min(1000), 1)
        }
        _ => (1, 1),
    }
}

/// Hijos de una tabla: los títulos aparte y el resto normalizado en grupos de columnas y
/// grupos de filas (reglas 1.3 y 2.1; las filas y columnas sueltas se agrupan en grupos
/// anónimos, que no cambian el layout ni la pintura).
fn table_children(
    items: Vec<Item>,
    table: &Arc<ComputedStyle>,
) -> (Vec<LayoutBox>, Vec<LayoutBox>) {
    let items = remove_whitespace(items, true);
    let mut proper: Vec<LayoutBox> = Vec::new();
    let mut run: Vec<Item> = Vec::new();
    for item in items {
        match item {
            Item::TablePart(b) if is_proper_table_child(b.style.display) => {
                if !run.is_empty() {
                    proper.push(anonymous_row(std::mem::take(&mut run), table));
                }
                proper.push(b);
            }
            other => run.push(other),
        }
    }
    if !run.is_empty() {
        proper.push(anonymous_row(run, table));
    }
    let mut captions = Vec::new();
    let mut parts: Vec<LayoutBox> = Vec::new();
    for b in proper {
        match b.style.display {
            Display::TableCaption => captions.push(b),
            Display::TableRow => match parts.last_mut() {
                Some(g) if g.node.is_none() && g.style.display == Display::TableRowGroup => {
                    g.children.push(b)
                }
                _ => {
                    let mut g = anonymous_part(table, Display::TableRowGroup, BoxKind::TablePart);
                    g.children.push(b);
                    parts.push(g);
                }
            },
            Display::TableColumn => match parts.last_mut() {
                Some(g) if g.node.is_none() && g.style.display == Display::TableColumnGroup => {
                    g.children.push(b)
                }
                _ => {
                    let mut g =
                        anonymous_part(table, Display::TableColumnGroup, BoxKind::TablePart);
                    g.children.push(b);
                    parts.push(g);
                }
            },
            _ => parts.push(b),
        }
    }
    (captions, parts)
}

/// Caja envoltorio con la caja de tabla y sus títulos. `element` es el elemento (o
/// pseudo-elemento) de la tabla; `None` para una tabla anónima, cuyo estilo hereda de
/// `style` (el del padre).
fn make_table(
    element: Option<(NodeId, Option<PseudoElement>)>,
    style: &Arc<ComputedStyle>,
    items: Vec<Item>,
    inline: bool,
) -> LayoutBox {
    let display = if inline {
        Display::InlineTable
    } else {
        Display::Table
    };
    let (wrapper_style, table_style) = match element {
        Some(_) => (wrapper_style(style), table_box_style(style)),
        None => {
            let s = anonymous_style(style, display);
            let mut t = (*s).clone();
            t.display = Display::Table;
            (s, Arc::new(t))
        }
    };
    let node = element.map(|(n, _)| n);
    let pseudo = element.and_then(|(_, p)| p);
    let (captions, parts) = table_children(items, &table_style);
    let mut table = LayoutBox::new(BoxKind::Table, table_style, node);
    table.pseudo = pseudo;
    table.children = parts;
    crate::table::finish_table(&mut table);
    let kind = if inline {
        BoxKind::InlineBlock
    } else if node.is_some() {
        BoxKind::Block
    } else {
        BoxKind::AnonymousBlock
    };
    let mut wrapper = LayoutBox::new(kind, wrapper_style, node);
    wrapper.pseudo = pseudo;
    let (bottom, top): (Vec<LayoutBox>, Vec<LayoutBox>) = captions
        .into_iter()
        .partition(|c| c.style.caption_side == CaptionSide::Bottom);
    wrapper.children = top;
    wrapper.children.push(table);
    wrapper.children.extend(bottom);
    wrapper
}

/// Reglas 1.4, 3.1 y 3.2 de §17.2.1 en un padre que no es tabular: las partes de tabla sueltas
/// (y el white space entre ellas) van en tablas anónimas, `inline-table` si el padre es una
/// caja inline.
fn fixup_table_parts(items: Vec<Item>, parent: &Arc<ComputedStyle>, inline: bool) -> Vec<Item> {
    if !items.iter().any(|i| matches!(i, Item::TablePart(_))) {
        return items;
    }
    let items = remove_whitespace(items, false);
    let mut out = Vec::new();
    let mut run: Vec<Item> = Vec::new();
    for item in items {
        match item {
            Item::TablePart(b) => run.push(Item::TablePart(b)),
            other => {
                if !run.is_empty() {
                    let t = make_table(None, parent, std::mem::take(&mut run), inline);
                    out.push(classify(t));
                }
                out.push(other);
            }
        }
    }
    if !run.is_empty() {
        out.push(classify(make_table(None, parent, run, inline)));
    }
    out
}
