//! Árbol de fragmentos: la salida del layout, con geometría absoluta (px, origen en el
//! viewport), y su volcado legible.

use std::fmt::Write as _;
use std::sync::Arc;

use bf_css::selectors::PseudoElement;
use bf_dom::{Document, NodeId};
use bf_style::ComputedStyle;
use bf_style::values::color::Rgba;
use bf_style::values::fmt_number;

use crate::font::Glyph;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        self.x += dx;
        self.y += dy;
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

/// Anchos de los cuatro lados.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Edges {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Edges {
    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }

    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FragmentKind {
    /// Caja de bloque (incluidas las anónimas).
    Block,
    /// Caja atómica inline-level con un block container dentro (`inline-block`, ...).
    InlineBlock,
    /// Fragmento de una caja inline en una línea.
    Inline,
    /// Line box.
    Line,
    Text,
    /// Elemento reemplazado (`img`, ...).
    Replaced,
    /// Marcador de `list-item`.
    Marker,
}

/// Región del medidor en la que cae el valor de un `<meter>` (HTML §4.10.14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeterLevel {
    Optimum,
    Suboptimum,
    EvenLessGood,
}

/// Parte nativa de un control de formulario que se pinta sobre su content box (HTML §15.5):
/// lo que no se puede expresar como fondo, borde o texto.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Widget {
    Checkbox {
        checked: bool,
    },
    Radio {
        checked: bool,
    },
    /// Flecha de un `<select>` de una línea, al final del content box.
    SelectArrow,
    /// Barra de `<progress>`: fracción completada (`None` = indeterminada).
    Progress(Option<f32>),
    Meter {
        fraction: f32,
        level: MeterLevel,
    },
    /// `<input type=range>`: posición del control deslizante (0 a 1).
    Range(f32),
    /// `<input type=color>`: la muestra del color.
    ColorSwatch(Rgba),
}

#[derive(Debug, Clone)]
pub struct Fragment {
    /// Border box (para `Line`, la line box; para `Text`, el content area del texto).
    pub rect: Rect,
    pub kind: FragmentKind,
    pub style: Arc<ComputedStyle>,
    pub children: Vec<Fragment>,
    pub node: Option<NodeId>,
    pub pseudo: Option<PseudoElement>,
    /// Texto y glifos (para `Text` y `Marker`).
    pub text: Option<(String, Vec<Glyph>)>,
    pub margin: Edges,
    pub border: Edges,
    pub padding: Edges,
    /// `z-index` de las cajas posicionadas (los stacking contexts se arman al pintar).
    pub z_index: Option<i32>,
    /// Recorte por `overflow` (padding box), si aplica.
    pub clip: Option<Rect>,
    /// Caja anónima (sin elemento propio).
    pub anonymous: bool,
    /// Baseline (y absoluta) de line boxes, textos, marcadores y controles de una línea.
    pub baseline: Option<f32>,
    /// Parte nativa de un control de formulario.
    pub widget: Option<Widget>,
    /// Border box de la leyenda que corta el borde superior de un `<fieldset>` (HTML
    /// §15.3.13).
    pub legend: Option<Rect>,
}

impl Fragment {
    pub fn new(kind: FragmentKind, rect: Rect, style: Arc<ComputedStyle>) -> Self {
        Fragment {
            rect,
            kind,
            style,
            children: Vec::new(),
            node: None,
            pseudo: None,
            text: None,
            margin: Edges::default(),
            border: Edges::default(),
            padding: Edges::default(),
            z_index: None,
            clip: None,
            anonymous: false,
            baseline: None,
            widget: None,
            legend: None,
        }
    }

    /// Desplaza el fragmento y todos sus descendientes.
    pub fn translate(&mut self, dx: f32, dy: f32) {
        self.rect.translate(dx, dy);
        if let Some(b) = &mut self.baseline {
            *b += dy;
        }
        if let Some(clip) = &mut self.clip {
            clip.translate(dx, dy);
        }
        if let Some(legend) = &mut self.legend {
            legend.translate(dx, dy);
        }
        for child in &mut self.children {
            child.translate(dx, dy);
        }
    }

    /// Margin box.
    pub fn margin_rect(&self) -> Rect {
        Rect::new(
            self.rect.x - self.margin.left,
            self.rect.y - self.margin.top,
            self.rect.width + self.margin.horizontal(),
            self.rect.height + self.margin.vertical(),
        )
    }

    /// Content box.
    pub fn content_rect(&self) -> Rect {
        Rect::new(
            self.rect.x + self.border.left + self.padding.left,
            self.rect.y + self.border.top + self.padding.top,
            self.rect.width - self.border.horizontal() - self.padding.horizontal(),
            self.rect.height - self.border.vertical() - self.padding.vertical(),
        )
    }

    /// Padding box.
    pub fn padding_rect(&self) -> Rect {
        Rect::new(
            self.rect.x + self.border.left,
            self.rect.y + self.border.top,
            self.rect.width - self.border.horizontal(),
            self.rect.height - self.border.vertical(),
        )
    }
}

#[derive(Debug, Clone)]
pub struct FragmentTree {
    /// Fragmento del elemento raíz (si genera una caja).
    pub root: Option<Fragment>,
    pub viewport: Size,
    /// Lo que el layout degradó al construir el árbol de cajas.
    pub report: crate::box_tree::LayoutReport,
}

fn label(doc: &Document, f: &Fragment) -> String {
    if f.anonymous {
        return "<anonymous>".into();
    }
    let Some(el) = f.node.and_then(|n| doc.element(n)) else {
        return "<?>".into();
    };
    let mut s = format!("<{}", el.name);
    if let Some(id) = el.attr("id") {
        let _ = write!(s, "#{id}");
    }
    if let Some(class) = el.attr("class") {
        for c in class.split_ascii_whitespace() {
            let _ = write!(s, ".{c}");
        }
    }
    if let Some(p) = f.pseudo {
        let _ = write!(s, "::{}", p.name());
    }
    s.push('>');
    s
}

fn rect_str(r: &Rect) -> String {
    format!(
        "{},{} {}×{}",
        fmt_number(r.x),
        fmt_number(r.y),
        fmt_number(r.width),
        fmt_number(r.height)
    )
}

/// Una línea por fragmento, indentada dos espacios por nivel.
pub fn dump(doc: &Document, tree: &FragmentTree) -> String {
    fn walk(doc: &Document, f: &Fragment, depth: usize, out: &mut String) {
        let pad = "  ".repeat(depth);
        let rect = rect_str(&f.rect);
        let line = match f.kind {
            FragmentKind::Block => format!("Block {} {rect}", label(doc, f)),
            FragmentKind::InlineBlock => format!("InlineBlock {} {rect}", label(doc, f)),
            FragmentKind::Inline => format!("Inline {} {rect}", label(doc, f)),
            FragmentKind::Replaced => format!("Replaced {} {rect}", label(doc, f)),
            FragmentKind::Line => format!("Line {rect}"),
            FragmentKind::Text => {
                let text = f.text.as_ref().map_or("", |(t, _)| t.as_str());
                format!("Text {text:?} {rect}")
            }
            FragmentKind::Marker => {
                let text = f.text.as_ref().map_or("", |(t, _)| t.as_str());
                format!("Marker {text:?} {rect}")
            }
        };
        let _ = writeln!(out, "{pad}{line}");
        for c in &f.children {
            walk(doc, c, depth + 1, out);
        }
    }
    let mut out = String::new();
    if let Some(root) = &tree.root {
        walk(doc, root, 0, &mut out);
    }
    out
}
