//! Block formatting context (CSS 2.2 §9.4.1, §10.3, §10.6 y §8.3.1): anchos y alturas de
//! bloques, colapso de márgenes, floats en flujo de bloques, elementos reemplazados, tamaños
//! intrínsecos y el layout del elemento raíz.
//!
//! Colapso de márgenes: el margen superior de un bloque se combina con el de su primer hijo
//! (y a través de los hijos vacíos) antes de posicionarlo. Esa "cadena" se calcula de forma
//! estructural ([`Layout::top_chain`]) para conocer la posición del border box antes de hacer el
//! layout del bloque; los hijos cuyo margen ya quedó incluido en la cadena se colocan sin volver
//! a sumarlo. Los márgenes inferiores salen del layout de cada bloque hacia el padre.

use bf_style::ComputedStyle;
use bf_style::properties::types::{
    BoxSizing, Clear, ContentAlign, Display, LengthPercentageAuto, MaxSize, Overflow, Position,
    Size, ZIndex,
};

use crate::box_tree::{BoxKind, LayoutBox};
use crate::controls::Control;
use crate::float::FloatContext;
use crate::font::{FontMetrics, FontProvider, FontRequest, Glyph};
use crate::fragment::{Edges, Fragment, FragmentKind, Rect, Size as LayoutSize};

/// Content box del containing block. `height` es `Some` si es definida (para porcentajes).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Cb {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: Option<f32>,
}

/// Margen colapsado: el máximo de los positivos más el mínimo de los negativos (§8.3.1).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct CollapsedMargin {
    positive: f32,
    negative: f32,
}

impl CollapsedMargin {
    pub fn new(m: f32) -> Self {
        CollapsedMargin::default().adjoin(m)
    }

    pub fn adjoin(mut self, m: f32) -> Self {
        if m > 0.0 {
            self.positive = self.positive.max(m);
        } else {
            self.negative = self.negative.min(m);
        }
        self
    }

    pub fn merge(self, other: CollapsedMargin) -> Self {
        CollapsedMargin {
            positive: self.positive.max(other.positive),
            negative: self.negative.min(other.negative),
        }
    }

    pub fn resolve(self) -> f32 {
        self.positive + self.negative
    }
}

/// Caja con `position: absolute | fixed` pendiente de layout hasta conocer su containing
/// block.
pub(crate) struct PendingAbs<'a> {
    pub b: &'a LayoutBox,
    pub static_x: f32,
    pub static_y: f32,
}

pub(crate) struct Layout<'a> {
    pub fonts: &'a dyn FontProvider,
    pub viewport: LayoutSize,
    /// Un frame por caja posicionada en curso (la primera es el initial containing block).
    pub abs_frames: Vec<Vec<PendingAbs<'a>>>,
    pub fixed: Vec<PendingAbs<'a>>,
    /// Caja del elemento raíz (siempre BFC root y sin colapso de márgenes).
    pub root: Option<&'a LayoutBox>,
    /// Altura de contenido impuesta a la próxima caja que maquete `box_contents` (ítems flex
    /// estirados o flexionados en columna).
    pub forced_height: Option<f32>,
    /// Leyenda renderizada del `<fieldset>` en curso, que `block_children` no coloca.
    pub legend: Option<&'a LayoutBox>,
    /// E4: columnas de cada tabla ya medidas (por dirección de la caja de tabla).
    pub table_cache: std::collections::HashMap<usize, std::rc::Rc<crate::table::ColumnInfo>>,
    /// E4: datos de pintura de la última tabla maquetada, para su fragmento.
    pub table_paint: Option<crate::fragment::TablePaint>,
    pub grid_baseline: Option<f32>, // E4 grid: baseline del último contenedor grid maquetado
    pub bfc_items: Vec<usize>,      // E4 grid: ítems en curso (raíces de formato independientes)
}

/// Tabla de §10.4 para un reemplazado con ancho y alto `auto` y relación de aspecto: respeta
/// min/max sin deformar (salvo cuando las restricciones lo obligan).
fn constrain_with_ratio(
    w: f32,
    h: f32,
    (min_w, max_w): (f32, f32),
    (min_h, max_h): (f32, f32),
) -> (f32, f32) {
    if w <= 0.0 || h <= 0.0 {
        return (w.min(max_w).max(min_w), h.min(max_h).max(min_h));
    }
    let (over_w, under_w, over_h, under_h) = (w > max_w, w < min_w, h > max_h, h < min_h);
    match (over_w, under_w, over_h, under_h) {
        (true, _, true, _) if max_w / w <= max_h / h => (max_w, min_h.max(max_w * h / w)),
        (true, _, true, _) => (min_w.max(max_h * w / h), max_h),
        (_, true, _, true) if min_w / w <= min_h / h => (max_w.min(min_h * w / h), min_h),
        (_, true, _, true) => (min_w, max_h.min(min_w * h / w)),
        (_, true, true, _) => (min_w, max_h),
        (true, _, _, true) => (max_w, min_h),
        (true, _, _, _) => (max_w, (max_w * h / w).max(min_h)),
        (_, true, _, _) => (min_w, (min_w * h / w).min(max_h)),
        (_, _, true, _) => ((max_h * w / h).max(min_w), max_h),
        (_, _, _, true) => ((min_h * w / h).min(max_w), min_h),
        _ => (w, h),
    }
}

pub(crate) fn margin_value(m: &LengthPercentageAuto, basis: f32) -> Option<f32> {
    match m {
        LengthPercentageAuto::Auto => None,
        LengthPercentageAuto::LengthPercentage(lp) => Some(lp.resolve(basis)),
    }
}

pub(crate) fn border_edges(style: &ComputedStyle) -> Edges {
    let [top, right, bottom, left] = style.used_border_widths();
    Edges {
        top,
        right,
        bottom,
        left,
    }
}

pub(crate) fn padding_edges(style: &ComputedStyle, basis: f32) -> Edges {
    Edges {
        top: style.padding_top.0.resolve(basis),
        right: style.padding_right.0.resolve(basis),
        bottom: style.padding_bottom.0.resolve(basis),
        left: style.padding_left.0.resolve(basis),
    }
}

/// Márgenes con `auto` como 0.
pub(crate) fn margin_edges(style: &ComputedStyle, basis: f32) -> Edges {
    Edges {
        top: margin_value(&style.margin_top, basis).unwrap_or(0.0),
        right: margin_value(&style.margin_right, basis).unwrap_or(0.0),
        bottom: margin_value(&style.margin_bottom, basis).unwrap_or(0.0),
        left: margin_value(&style.margin_left, basis).unwrap_or(0.0),
    }
}

pub(crate) fn is_positioned(style: &ComputedStyle) -> bool {
    style.position != Position::Static
}

/// ¿La caja establece un block formatting context? (CSS 2.2 §9.4.1, CSS Display 3.)
pub(crate) fn establishes_bfc(b: &LayoutBox, is_root: bool) -> bool {
    // Un `<fieldset>` establece un BFC (HTML §15.3.13).
    if is_root
        || b.is_out_of_flow()
        || matches!(b.kind, BoxKind::InlineBlock)
        || b.fieldset.is_some()
    {
        return true;
    }
    let s = &b.style;
    let scroll = |o: Overflow| !matches!(o, Overflow::Visible | Overflow::Clip);
    scroll(s.overflow_x)
        || scroll(s.overflow_y)
        || matches!(
            s.display,
            Display::FlowRoot
                | Display::Flex
                | Display::InlineFlex
                | Display::Grid
                | Display::InlineGrid
                | Display::Table
                | Display::InlineTable
                | Display::TableCell
                | Display::TableCaption
        )
}

/// Alto del contenido especificado (`height`), si se puede resolver.
pub(crate) fn specified_height(
    style: &ComputedStyle,
    cb_height: Option<f32>,
    edges_v: f32,
) -> Option<f32> {
    let Size::LengthPercentage(lp) = &style.height else {
        return None;
    };
    if lp.has_percentage() && cb_height.is_none() {
        return None;
    }
    let h = lp.resolve(cb_height.unwrap_or(0.0));
    Some(if style.box_sizing == BoxSizing::BorderBox {
        (h - edges_v).max(0.0)
    } else {
        h
    })
}

/// `min-height` y `max-height` (contenido), con porcentajes solo si el containing block tiene
/// alto definido.
pub(crate) fn height_limits(
    style: &ComputedStyle,
    cb_height: Option<f32>,
    edges_v: f32,
) -> (f32, f32) {
    let adjust = |v: f32| {
        if style.box_sizing == BoxSizing::BorderBox {
            (v - edges_v).max(0.0)
        } else {
            v
        }
    };
    let min = match &style.min_height {
        Size::LengthPercentage(lp) if !lp.has_percentage() || cb_height.is_some() => {
            adjust(lp.resolve(cb_height.unwrap_or(0.0)))
        }
        _ => 0.0,
    };
    let max = match &style.max_height {
        MaxSize::LengthPercentage(lp) if !lp.has_percentage() || cb_height.is_some() => {
            adjust(lp.resolve(cb_height.unwrap_or(0.0)))
        }
        _ => f32::INFINITY,
    };
    (min, max)
}

/// `width` especificado (contenido), o `None` si es `auto` (o una keyword intrínseca).
pub(crate) fn specified_width(style: &ComputedStyle, cb_width: f32, edges_h: f32) -> Option<f32> {
    let Size::LengthPercentage(lp) = &style.width else {
        return None;
    };
    let w = lp.resolve(cb_width);
    Some(if style.box_sizing == BoxSizing::BorderBox {
        (w - edges_h).max(0.0)
    } else {
        w
    })
}

pub(crate) fn width_limits(style: &ComputedStyle, cb_width: f32, edges_h: f32) -> (f32, f32) {
    let adjust = |v: f32| {
        if style.box_sizing == BoxSizing::BorderBox {
            (v - edges_h).max(0.0)
        } else {
            v
        }
    };
    let min = match &style.min_width {
        Size::LengthPercentage(lp) => adjust(lp.resolve(cb_width)),
        _ => 0.0,
    };
    let max = match &style.max_width {
        MaxSize::LengthPercentage(lp) => adjust(lp.resolve(cb_width)),
        _ => f32::INFINITY,
    };
    (min, max)
}

fn is_zero_or_auto_height(style: &ComputedStyle) -> bool {
    match &style.height {
        Size::LengthPercentage(lp) => !lp.has_percentage() && lp.resolve(0.0) == 0.0,
        _ => true,
    }
}

fn min_height_is_zero(style: &ComputedStyle) -> bool {
    match &style.min_height {
        Size::LengthPercentage(lp) => lp.resolve(0.0) == 0.0,
        _ => true,
    }
}

impl<'a> Layout<'a> {
    pub fn new(fonts: &'a dyn FontProvider, viewport: LayoutSize) -> Self {
        Layout {
            fonts,
            viewport,
            abs_frames: Vec::new(),
            fixed: Vec::new(),
            root: None,
            forced_height: None,
            legend: None,
            table_cache: std::collections::HashMap::new(),
            table_paint: None,
            grid_baseline: None,   // E4 grid
            bfc_items: Vec::new(), // E4 grid
        }
    }

    pub fn font_request<'s>(style: &'s ComputedStyle) -> FontRequest<'s> {
        crate::font::font_request(style)
    }

    pub fn metrics(&self, style: &ComputedStyle) -> FontMetrics {
        self.fonts.metrics(&Self::font_request(style))
    }

    /// Glifos con `letter-spacing` y `word-spacing` aplicados.
    pub fn shape(&self, text: &str, style: &ComputedStyle) -> Vec<Glyph> {
        let mut glyphs = self.fonts.shape(text, &Self::font_request(style));
        let letter = style.letter_spacing.px();
        let word = style.word_spacing.px();
        // letter-spacing va después de cada carácter tipográfico: el último glifo del
        // cluster (CSS Text 3 §8.2).
        for i in 0..glyphs.len() {
            let cluster = glyphs[i].cluster;
            let last_of_cluster = glyphs.get(i + 1).is_none_or(|n| n.cluster != cluster);
            if last_of_cluster {
                glyphs[i].advance += letter;
                // Separadores de palabras (CSS Text 3 §4.2): U+0020, U+00A0, U+1361, U+10100,
                // U+10101, U+1039F y U+1091F.
                if text[cluster..].starts_with([
                    ' ',
                    '\u{A0}',
                    '\u{1361}',
                    '\u{10100}',
                    '\u{10101}',
                    '\u{1039F}',
                    '\u{1091F}',
                ]) {
                    glyphs[i].advance += word;
                }
            }
        }
        glyphs
    }

    pub fn line_height(&self, style: &ComputedStyle) -> f32 {
        let m = self.metrics(style);
        let fs = style.font_size_px();
        let normal = if fs > 0.0 {
            m.normal_line_height() / fs
        } else {
            1.0
        };
        style.line_height.resolve(fs, normal)
    }

    // --- Propiedades estructurales para el colapso de márgenes ---

    fn is_root(&self, b: &LayoutBox) -> bool {
        self.root.is_some_and(|r| std::ptr::eq(r, b))
    }

    pub(crate) fn bfc(&self, b: &LayoutBox) -> bool {
        establishes_bfc(b, self.is_root(b)) || self.bfc_items.contains(&(b as *const _ as usize)) // E4 grid
    }

    fn collapses_top(&self, b: &LayoutBox) -> bool {
        matches!(b.kind, BoxKind::Block | BoxKind::AnonymousBlock)
            && !self.bfc(b)
            && border_edges(&b.style).top == 0.0
            && b.style.padding_top.0.resolve(0.0) == 0.0
            && !b.style.padding_top.0.has_percentage()
            && !b.has_inline_children()
    }

    fn collapses_bottom(&self, b: &LayoutBox) -> bool {
        matches!(b.kind, BoxKind::Block | BoxKind::AnonymousBlock)
            && !self.bfc(b)
            && border_edges(&b.style).bottom == 0.0
            && b.style.padding_bottom.0.resolve(0.0) == 0.0
            && !b.style.padding_bottom.0.has_percentage()
            && matches!(b.style.height, Size::Auto)
            && min_height_is_zero(&b.style)
    }

    /// ¿La caja no genera line boxes ni tiene altura, de modo que sus márgenes superior e
    /// inferior colapsan entre sí (§8.3.1)?
    pub(crate) fn is_empty(&self, b: &LayoutBox) -> bool {
        if !matches!(b.kind, BoxKind::Block | BoxKind::AnonymousBlock) || self.bfc(b) {
            return false;
        }
        let s = &b.style;
        if border_edges(s).top != 0.0
            || border_edges(s).bottom != 0.0
            || s.padding_top.0.resolve(0.0) != 0.0
            || s.padding_bottom.0.resolve(0.0) != 0.0
            || !is_zero_or_auto_height(s)
            || !min_height_is_zero(s)
        {
            return false;
        }
        if b.has_inline_children() {
            return !b.children.iter().any(inline_has_content);
        }
        b.children
            .iter()
            .filter(|c| !c.is_out_of_flow() && !matches!(c.kind, BoxKind::Marker { .. }))
            .all(|c| self.is_empty(c))
    }

    /// Márgenes de una caja vacía y sus hijos, que colapsan todos juntos.
    fn through_margin(&self, b: &LayoutBox, cb_width: f32) -> CollapsedMargin {
        let m = margin_edges(&b.style, cb_width);
        let mut c = CollapsedMargin::new(m.top).adjoin(m.bottom);
        let inner = self.content_width(b, cb_width);
        for child in b.children.iter().filter(|c| c.is_block_level()) {
            c = c.merge(self.through_margin(child, inner));
        }
        c
    }

    /// Ancho del contenido de un bloque en flujo dentro de un containing block de
    /// `cb_width` (base de los porcentajes de sus hijos).
    fn content_width(&self, b: &LayoutBox, cb_width: f32) -> f32 {
        let s = &b.style;
        let edges_h = border_edges(s).horizontal() + padding_edges(s, cb_width).horizontal();
        self.block_width(s, cb_width, edges_h).0
    }

    /// Margen superior de `b` combinado con los de los descendientes con los que colapsa.
    pub(crate) fn top_chain(&self, b: &LayoutBox, cb_width: f32) -> CollapsedMargin {
        let mut m = CollapsedMargin::new(margin_edges(&b.style, cb_width).top);
        if self.collapses_top(b) {
            for child in b.children.iter().filter(|c| c.is_block_level()) {
                if child.style.clear != Clear::None {
                    break;
                }
                let inner = self.content_width(b, cb_width);
                if self.is_empty(child) {
                    m = m.merge(self.through_margin(child, inner));
                    continue;
                }
                m = m.merge(self.top_chain(child, inner));
                break;
            }
        }
        m
    }

    // --- Layout de bloques ---

    /// Anchos de un bloque en flujo no reemplazado (CSS 2.2 §10.3.3, con min/max de §10.4):
    /// `(ancho del contenido, margin-left, margin-right)`.
    fn block_width(&self, style: &ComputedStyle, cb_width: f32, edges_h: f32) -> (f32, f32, f32) {
        let ml = margin_value(&style.margin_left, cb_width);
        let mr = margin_value(&style.margin_right, cb_width);
        let solve = |w: Option<f32>| match w {
            None => {
                let (l, r) = (ml.unwrap_or(0.0), mr.unwrap_or(0.0));
                ((cb_width - l - r - edges_h).max(0.0), l, r)
            }
            Some(w) => {
                let rest = cb_width - w - edges_h;
                // §10.3.3: si lo que no es auto ya excede el containing block, los márgenes
                // auto valen 0 (y sobre-restringido, se ajusta margin-right).
                if w + edges_h + ml.unwrap_or(0.0) + mr.unwrap_or(0.0) > cb_width {
                    let l = ml.unwrap_or(0.0);
                    return (w, l, rest - l);
                }
                match (ml, mr) {
                    (None, None) => (w, rest / 2.0, rest / 2.0),
                    (None, Some(r)) => (w, rest - r, r),
                    // Sobre-restringido (o margin-right auto): se ajusta margin-right (ltr).
                    (Some(l), _) => (w, l, rest - l),
                }
            }
        };
        let (min, max) = width_limits(style, cb_width, edges_h);
        let mut result = solve(specified_width(style, cb_width, edges_h));
        if result.0 > max {
            result = solve(Some(max));
        }
        if result.0 < min {
            result = solve(Some(min));
        }
        result
    }

    /// Tamaño usado del contenido de un reemplazado (CSS 2.2 §10.3.2, §10.6.2 y la tabla de
    /// restricciones de §10.4 cuando ancho y alto son `auto` y hay relación de aspecto).
    pub(crate) fn replaced_size(
        &self,
        b: &LayoutBox,
        natural: LayoutSize,
        ratio: Option<f32>,
        cb: Cb,
    ) -> LayoutSize {
        let s = &b.style;
        let border = border_edges(s);
        let padding = padding_edges(s, cb.width);
        let (eh, ev) = (
            border.horizontal() + padding.horizontal(),
            border.vertical() + padding.vertical(),
        );
        let sw = specified_width(s, cb.width, eh);
        let sh = specified_height(s, cb.height, ev);
        let (wmin, wmax) = width_limits(s, cb.width, eh);
        let (hmin, hmax) = height_limits(s, cb.height, ev);
        let (wmax, hmax) = (wmax.max(wmin), hmax.max(hmin));
        match (sw, sh, ratio) {
            (None, None, Some(_)) => {
                let (w, h) = (natural.width, natural.height);
                let (w, h) = constrain_with_ratio(w, h, (wmin, wmax), (hmin, hmax));
                LayoutSize {
                    width: w,
                    height: h,
                }
            }
            (None, Some(h), Some(r)) => {
                let h = h.min(hmax).max(hmin);
                LayoutSize {
                    width: (h * r).min(wmax).max(wmin),
                    height: h,
                }
            }
            (Some(w), None, Some(r)) if r > 0.0 => {
                let w = w.min(wmax).max(wmin);
                LayoutSize {
                    width: w,
                    height: (w / r).min(hmax).max(hmin),
                }
            }
            _ => LayoutSize {
                width: sw.unwrap_or(natural.width).min(wmax).max(wmin),
                height: sh.unwrap_or(natural.height).min(hmax).max(hmin),
            },
        }
    }

    /// Layout de una caja block-level en flujo con su border box en `top`. Devuelve el
    /// fragmento y el margen inferior que puede colapsar con lo que sigue.
    pub(crate) fn block(
        &mut self,
        b: &'a LayoutBox,
        cb: Cb,
        top: f32,
        floats: &mut FloatContext,
        is_root: bool,
    ) -> (Fragment, CollapsedMargin) {
        if establishes_bfc(b, is_root) && !is_root && !floats.is_empty() {
            return self.bfc_root_beside_floats(b, cb, top, floats);
        }
        self.block_in(b, cb, cb, top, floats, is_root)
    }

    /// Un BFC root no se superpone con los floats de su BFC padre (CSS 2.2 §9.5): se
    /// dimensiona con el espacio libre en toda su altura (rehaciendo el layout si al crecer
    /// alcanza floats más anchos) y, si no cabe junto a ellos, baja hasta el borde inferior del
    /// siguiente float.
    fn bfc_root_beside_floats(
        &mut self,
        b: &'a LayoutBox,
        cb: Cb,
        top: f32,
        floats: &mut FloatContext,
    ) -> (Fragment, CollapsedMargin) {
        let mut y = top;
        let mut guess = 0.0f32;
        let snapshot = |l: &Self| (l.abs_frames.last().map_or(0, Vec::len), l.fixed.len());
        let restore = |l: &mut Self, (a, f): (usize, usize)| {
            if let Some(frame) = l.abs_frames.last_mut() {
                frame.truncate(a);
            }
            l.fixed.truncate(f);
        };
        for attempt in 0..8 {
            let (l, r) = floats.band(y, guess, cb.x, cb.x + cb.width);
            let narrowed = l > cb.x + 0.01 || r < cb.x + cb.width - 0.01;
            let avail = Cb {
                x: l,
                width: (r - l).max(0.0),
                ..cb
            };
            let saved = snapshot(self);
            let (f, m) = self.block_in(b, cb, avail, y, floats, false);
            if attempt == 7 {
                return (f, m);
            }
            let height = f.rect.height;
            // ¿Cabe junto a los floats? El border box más el margen izquierdo (que lo desplaza)
            // y el derecho solo si es negativo: uno positivo no lo empuja hacia abajo (auto = 0).
            let ml = margin_value(&b.style.margin_left, cb.width).unwrap_or(0.0);
            let mr = margin_value(&b.style.margin_right, cb.width).unwrap_or(0.0);
            let needed = ml + f.rect.width + mr.min(0.0);
            if narrowed
                && needed > avail.width + 0.01
                && let Some(next) = floats.next_bottom(y, guess.max(height))
            {
                restore(self, saved);
                y = next;
                guess = 0.0;
                continue;
            }
            let (l2, r2) = floats.band(y, height, cb.x, cb.x + cb.width);
            if height > guess + 0.01 && (l2 > l + 0.01 || r2 < r - 0.01) {
                restore(self, saved);
                guess = height;
                continue;
            }
            return (f, m);
        }
        unreachable!("el último intento siempre devuelve")
    }

    /// [`Self::block`] con el espacio horizontal disponible `avail` (dentro de `cb`).
    fn block_in(
        &mut self,
        b: &'a LayoutBox,
        cb: Cb,
        avail: Cb,
        top: f32,
        floats: &mut FloatContext,
        is_root: bool,
    ) -> (Fragment, CollapsedMargin) {
        let style = &b.style;
        let border = border_edges(style);
        let padding = padding_edges(style, cb.width);
        let edges_h = border.horizontal() + padding.horizontal();
        // E4: el ancho de una tabla (y de su envoltorio) lo decide el algoritmo de tablas.
        let margins_h = margin_value(&style.margin_left, cb.width).unwrap_or(0.0)
            + margin_value(&style.margin_right, cb.width).unwrap_or(0.0);
        let table_w = self.table_fixed_width(b, (avail.width - margins_h).max(0.0), cb.width);
        // Ancho fijo: el de un reemplazado o el intrínseco de un control con `width: auto`.
        let fixed_w = table_w
            .or(match &b.kind {
                BoxKind::Replaced { natural, ratio } => {
                    Some(self.replaced_size(b, *natural, *ratio, cb).width)
                }
                _ if specified_width(style, cb.width, edges_h).is_none() => {
                    self.control_size(b).0.map(|w| {
                        let (min, max) = width_limits(style, cb.width, edges_h);
                        w.min(max).max(min)
                    })
                }
                _ => None,
            })
            .or_else(|| self.keyword_width(b, avail.width - margins_h - edges_h, cb.width));
        let (content_w, ml, mr) = match fixed_w {
            Some(width) => {
                let rest = avail.width - width - edges_h;
                let (l, r) = match (
                    margin_value(&style.margin_left, cb.width),
                    margin_value(&style.margin_right, cb.width),
                ) {
                    (None, None) => (rest / 2.0, rest / 2.0),
                    (None, Some(r)) => (rest - r, r),
                    (Some(l), _) => (l, rest - l),
                };
                (width, l, r)
            }
            None => self.block_width(style, avail.width, edges_h),
        };
        let margins = Edges {
            top: margin_value(&style.margin_top, cb.width).unwrap_or(0.0),
            right: mr,
            bottom: margin_value(&style.margin_bottom, cb.width).unwrap_or(0.0),
            left: ml,
        };
        let x = avail.x + ml;
        let (fragment, trailing) =
            self.box_contents(b, cb, x, top, content_w, margins, floats, is_root);
        let bottom = if self.collapses_bottom(b) && !is_root {
            trailing.adjoin(margins.bottom)
        } else {
            CollapsedMargin::new(margins.bottom)
        };
        (fragment, bottom)
    }

    /// Layout del contenido de una caja ya dimensionada horizontalmente: hijos, altura,
    /// cajas absolutas que contiene y desplazamiento relativo.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn box_contents(
        &mut self,
        b: &'a LayoutBox,
        cb: Cb,
        x: f32,
        top: f32,
        content_w: f32,
        margins: Edges,
        floats: &mut FloatContext,
        is_root: bool,
    ) -> (Fragment, CollapsedMargin) {
        let forced = self.forced_height.take();
        let style = &b.style;
        let border = border_edges(style);
        let padding = padding_edges(style, cb.width);
        let edges_v = border.vertical() + padding.vertical();
        let content_x = x + border.left + padding.left;
        // Leyenda renderizada de un `<fieldset>` (HTML §15.3.13): se coloca al inicio del
        // contenido, centrada sobre el borde superior; si es más alta que el borde, el contenido
        // baja lo que sobresale y el borde se pinta a la mitad de esa diferencia.
        let legend_box = match b.fieldset {
            Some(Some(i)) => b.children.get(i),
            _ => None,
        };
        let mut legend_frag = None;
        let mut legend_extra = 0.0f32;
        if let Some(legend) = legend_box {
            let lcb = Cb {
                x: content_x,
                y: top,
                width: content_w,
                height: None,
            };
            let lm = margin_edges(&legend.style, content_w);
            let lw = self.fit_width(legend, lcb, lm.horizontal());
            let lx = content_x + lm.left;
            let outer_h = self.measuring(|l| {
                let mut scratch = FloatContext::default();
                let (f, _) = l.box_contents(legend, lcb, lx, 0.0, lw, lm, &mut scratch, false);
                f.margin_rect().height
            });
            let ly = if outer_h > border.top {
                top
            } else {
                top + (border.top - outer_h) / 2.0
            };
            legend_extra = (outer_h - border.top).max(0.0);
            let mut scratch = FloatContext::default();
            let (f, _) =
                self.box_contents(legend, lcb, lx, ly + lm.top, lw, lm, &mut scratch, false);
            legend_frag = Some(f);
        }
        let content_top = top + border.top + legend_extra + padding.top;
        let control_h = self.control_size(b).1;
        let spec_h = forced
            .or_else(|| specified_height(style, cb.height, edges_v))
            .or(control_h);
        // Los porcentajes de los hijos se resuelven contra la altura usada (ya limitada por
        // min-height/max-height), no la especificada (§10.7).
        let child_cb = Cb {
            x: content_x,
            y: content_top,
            width: content_w,
            height: spec_h
                .map(|h| {
                    let (lo, hi) = height_limits(style, cb.height, edges_v);
                    h.min(hi).max(lo)
                })
                // E4: el alto en % de una tabla se resuelve contra el containing block de su
                // envoltorio.
                .or_else(|| cb.height.filter(|_| crate::table::is_table_wrapper(b))),
        };
        let positioned = is_positioned(style);
        if positioned {
            self.abs_frames.push(Vec::new());
        }
        let bfc = establishes_bfc(b, is_root) || self.bfc(b); // E4 grid
        let mut own_floats = FloatContext::default();
        let fl: &mut FloatContext = if bfc { &mut own_floats } else { floats };
        let mut trailing = CollapsedMargin::default();
        let outer_legend = std::mem::replace(&mut self.legend, legend_box);
        let (mut children, auto_h) = match &b.kind {
            BoxKind::Replaced { natural, ratio } => {
                let size = self.replaced_size(b, *natural, *ratio, cb);
                (Vec::new(), size.height)
            }
            _ if crate::flex::is_flex_container(b) => self.flex_layout(b, child_cb),
            // E4: grid y tablas.
            _ if crate::grid::is_grid_container(b) => self.grid_layout(b, child_cb),
            BoxKind::Table => self.table_layout(b, child_cb),
            _ if b.has_inline_children() => self.inline_content(b, child_cb, fl),
            _ => {
                let (frags, end, pending) = self.block_children(b, child_cb, fl);
                let mut h = end - content_top;
                if self.collapses_bottom(b) && !is_root {
                    trailing = pending;
                } else {
                    h += pending.resolve();
                }
                (frags, h)
            }
        };
        self.legend = outer_legend;
        let mut content_h = spec_h.unwrap_or(auto_h);
        // E4: en tablas `height` es un mínimo; en celdas solo cuenta para la altura de la fila
        // (CSS 2.2 §17.5.3), así que la celda mide su contenido.
        if matches!(b.kind, BoxKind::Table) {
            content_h = spec_h.map_or(auto_h, |h| h.max(auto_h));
        } else if b.style.display == Display::TableCell && forced.is_none() {
            content_h = auto_h;
        }
        // Un botón vacío mide una línea.
        if spec_h.is_none() && b.control == Some(Control::Button) {
            content_h = content_h.max(self.line_height(style));
        }
        if bfc
            && spec_h.is_none()
            && let Some(bottom) = fl.bottom()
        {
            content_h = content_h.max(bottom - content_top);
        }
        let (hmin, hmax) = height_limits(style, cb.height, edges_v);
        // CSS 2.2 §8.3.1: con altura computada `auto` el margen del último hijo es adyacente
        // al inferior de la caja aunque max-height la recorte (margin-collapse-038);
        // normal-flow/max-height-separates-margin espera lo contrario.
        content_h = content_h.min(hmax).max(hmin).max(0.0);
        // `align-content` en un block container (CSS Box Alignment 3 §5.2): el contenido se
        // desplaza dentro de un content box más alto. Los campos de texto usan `unsafe center`
        // (hoja UA): se centran aunque el texto no quepa.
        // Los list box y los textarea empiezan arriba (el `align-content` de la hoja UA es el
        // de los controles de una línea).
        let scrolls = matches!(
            b.control,
            Some(Control::ListBox { .. } | Control::TextArea { .. })
        );
        if !matches!(b.kind, BoxKind::Replaced { .. })
            && !crate::flex::is_flex_container(b)
            && !crate::grid::is_grid_container(b) // E4
            && !matches!(b.kind, BoxKind::Table) // E4
            && !scrolls
        {
            let free = content_h - auto_h;
            let unsafe_center = matches!(
                b.control,
                Some(Control::TextField { .. } | Control::Select { .. })
            );
            let shift = match style.align_content {
                ContentAlign::Center if unsafe_center => free / 2.0,
                ContentAlign::Center => (free / 2.0).max(0.0),
                ContentAlign::End | ContentAlign::FlexEnd => free.max(0.0),
                _ => 0.0,
            };
            if shift != 0.0 {
                for c in &mut children {
                    c.translate(0.0, shift);
                }
            }
        }
        // El borde de un fieldset con leyenda alta se pinta a la mitad de lo que sobresale.
        let legend_offset = legend_extra / 2.0;
        let rect = Rect::new(
            x,
            top + legend_offset,
            content_w + border.horizontal() + padding.horizontal(),
            content_h + edges_v + legend_extra - legend_offset,
        );
        let kind = match b.kind {
            BoxKind::Replaced { .. } => FragmentKind::Replaced,
            BoxKind::InlineBlock => FragmentKind::InlineBlock,
            _ => FragmentKind::Block,
        };
        let mut fragment = Fragment::new(kind, rect, b.style.clone());
        fragment.node = b.node;
        fragment.pseudo = b.pseudo;
        fragment.anonymous = matches!(b.kind, BoxKind::AnonymousBlock) || b.node.is_none();
        fragment.margin = margins;
        fragment.border = border;
        fragment.padding = padding;
        if style.overflow_x != Overflow::Visible
            || style.overflow_y != Overflow::Visible
            || b.control.as_ref().is_some_and(Control::clips)
        {
            fragment.clip = Some(fragment.padding_rect());
        }
        if let Some(control) = &b.control {
            fragment.widget = control.widget();
            // Baseline de un control de una línea: la de su texto o, si está vacío, la de una
            // línea vacía centrada en el content box.
            if control.single_line() {
                let m = self.metrics(style);
                let lh = self.line_height(style);
                let synthesized = content_top
                    + (content_h - lh) / 2.0
                    + (lh - m.ascent - m.descent) / 2.0
                    + m.ascent;
                fragment.baseline = Some(first_line(&children).map_or(synthesized, |(_, y)| y));
            }
        }
        if let Some(legend) = legend_frag {
            fragment.legend = Some(legend.rect);
            children.insert(0, legend);
        }
        // Marcador exterior de un list-item con hijos de bloque.
        if let Some(marker) = b
            .children
            .iter()
            .find(|c| matches!(c.kind, BoxKind::Marker { outside: true, .. }))
            && !b.has_inline_children()
        {
            let anchor = first_line(&children);
            children.push(self.marker_fragment(marker, anchor, content_x, content_top));
        }
        fragment.children = children;
        self.table_fragment(b, &mut fragment); // E4
        if crate::grid::is_grid_container(b) {
            fragment.baseline = self.grid_baseline.take(); // E4 grid
        }
        if positioned {
            fragment.z_index = match style.z_index {
                ZIndex::Integer(z) => Some(z),
                ZIndex::Auto => None,
            };
            let pending = self.abs_frames.pop().unwrap_or_default();
            let padding_box = fragment.padding_rect();
            for p in pending {
                let f = self.absolute(p, padding_box);
                fragment.children.push(f);
            }
        }
        if style.position == Position::Relative {
            let (dx, dy) = self.relative_offset(style, cb);
            fragment.translate(dx, dy);
        }
        (fragment, trailing)
    }

    /// Hijos block-level de un block container, con colapso de márgenes entre hermanos.
    fn block_children(
        &mut self,
        b: &'a LayoutBox,
        cb: Cb,
        floats: &mut FloatContext,
    ) -> (Vec<Fragment>, f32, CollapsedMargin) {
        let mut frags = Vec::new();
        let mut cursor = cb.y;
        let mut pending = CollapsedMargin::default();
        // Si el margen superior de `b` colapsa con sus primeros hijos, sus márgenes ya se
        // aplicaron al posicionar `b`.
        let mut consumed = self.collapses_top(b);
        // §8.3.1: si una caja vacía con clearance deja sus márgenes adyacentes, el margen que
        // forman con los hermanos siguientes no colapsa con el inferior del padre.
        let mut after_cleared_empty = false;
        let mut sibling_merged = false;
        for child in &b.children {
            if matches!(child.kind, BoxKind::Marker { .. })
                || self.legend.is_some_and(|l| std::ptr::eq(l, child))
            {
                continue;
            }
            if child.is_out_of_flow() {
                let y = cursor + pending.resolve();
                if child.is_float() {
                    let f = self.float_box(child, cb, y, floats);
                    frags.push(f);
                } else {
                    self.add_abs(child, cb.x, y);
                }
                continue;
            }
            if consumed && child.style.clear != Clear::None {
                consumed = false;
            }
            let mut top = if consumed {
                cursor
            } else {
                cursor + pending.merge(self.top_chain(child, cb.width)).resolve()
            };
            // Clearance (§9.5.2): el border box queda debajo de los floats despejados.
            let mut cleared = false;
            if child.style.clear != Clear::None
                && let Some(clear_y) = floats.clear_y(child.style.clear)
                && clear_y > top
            {
                top = clear_y;
                cleared = true;
            }
            if self.is_empty(child) && !cleared {
                let (f, _) = self.block(child, cb, top, floats, false);
                frags.push(f);
                if !consumed {
                    pending = pending.merge(self.through_margin(child, cb.width));
                }
                sibling_merged |= after_cleared_empty;
                continue;
            }
            after_cleared_empty = cleared && self.is_empty(child);
            sibling_merged = false;
            let (f, bottom) = self.block(child, cb, top, floats, false);
            cursor = f.rect.bottom();
            // Un bloque desplazado con `position: relative` no mueve el flujo.
            if child.style.position == Position::Relative {
                let (_, dy) = self.relative_offset(&child.style, cb);
                cursor -= dy;
            }
            pending = bottom;
            consumed = false;
            frags.push(f);
        }
        if after_cleared_empty && sibling_merged {
            cursor += pending.resolve();
            pending = CollapsedMargin::default();
        }
        (frags, cursor, pending)
    }

    /// Ancho shrink-to-fit (CSS 2.2 §10.3.5): `min(max(min-content, disponible), max-content)`.
    pub(crate) fn shrink_to_fit(&mut self, b: &'a LayoutBox, available: f32) -> f32 {
        let (min, max) = self.intrinsic_widths(b);
        min.max(available).min(max)
    }

    /// Ancho de contenido de una caja que se dimensiona shrink-to-fit (floats, inline-blocks,
    /// absolutas con `width: auto`).
    pub(crate) fn fit_width(&mut self, b: &'a LayoutBox, cb: Cb, margins_h: f32) -> f32 {
        let style = &b.style;
        let border = border_edges(style);
        let padding = padding_edges(style, cb.width);
        let edges_h = border.horizontal() + padding.horizontal();
        if let BoxKind::Replaced { natural, ratio } = &b.kind {
            return self.replaced_size(b, *natural, *ratio, cb).width;
        }
        // E4: tablas.
        if let Some(w) = self.table_fixed_width(b, (cb.width - margins_h).max(0.0), cb.width) {
            return w;
        }
        if let Some(w) = self.keyword_width(b, cb.width - margins_h - edges_h, cb.width) {
            return w;
        }
        let w = match specified_width(style, cb.width, edges_h) {
            Some(w) => w,
            None => match self.control_size(b).0 {
                Some(w) => w,
                None => self.shrink_to_fit(b, (cb.width - margins_h - edges_h).max(0.0)),
            },
        };
        let (min, max) = width_limits(style, cb.width, edges_h);
        w.min(max).max(min)
    }

    /// E4: `width: min-content | max-content | fit-content` (CSS Sizing 3 §3.1 y §3.2): el
    /// ancho de contenido, limitado por `min-width`/`max-width`.
    pub(crate) fn keyword_width(
        &mut self,
        b: &'a LayoutBox,
        available: f32,
        cb_width: f32,
    ) -> Option<f32> {
        if matches!(b.kind, BoxKind::Replaced { .. }) {
            return None;
        }
        let kind = b.style.width.clone();
        if !matches!(kind, Size::MinContent | Size::MaxContent | Size::FitContent) {
            return None;
        }
        let (min, max) = self.intrinsic_widths(b);
        let w = match kind {
            Size::MinContent => min,
            Size::MaxContent => max,
            _ => min.max(available).min(max),
        };
        let s = &b.style;
        let edges_h = border_edges(s).horizontal() + padding_edges(s, cb_width).horizontal();
        let (lo, hi) = width_limits(s, cb_width, edges_h);
        Some(w.min(hi).max(lo))
    }

    /// Layout y colocación de un float.
    pub(crate) fn float_box(
        &mut self,
        b: &'a LayoutBox,
        cb: Cb,
        y_min: f32,
        floats: &mut FloatContext,
    ) -> Fragment {
        let margins = margin_edges(&b.style, cb.width);
        let content_w = self.fit_width(b, cb, margins.horizontal());
        let mut scratch = FloatContext::default();
        let pending = self.pending_abs_marks(); // E4 grid
        let (mut f, _) =
            self.box_contents(b, cb, 0.0, 0.0, content_w, margins, &mut scratch, false);
        let outer = f.margin_rect();
        let left = b.style.float == bf_style::properties::types::Float::Left;
        // `clear` en un float (§9.5.2): su margin box queda debajo de los floats anteriores
        // que despeja.
        let y_min = floats
            .clear_y(b.style.clear)
            .map_or(y_min, |c| y_min.max(c));
        let (x, y) = floats.place(
            left,
            outer.width,
            outer.height,
            y_min,
            cb.x,
            cb.x + cb.width,
        );
        f.translate(x - outer.x, y - outer.y);
        self.shift_pending_abs(pending, x - outer.x, y - outer.y); // E4 grid
        f
    }

    /// Anchos intrínsecos (min-content, max-content) del contenido de `b`.
    pub(crate) fn intrinsic_widths(&mut self, b: &'a LayoutBox) -> (f32, f32) {
        if let BoxKind::Replaced { natural, ratio } = &b.kind {
            let cb = Cb {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: None,
            };
            let w = if matches!(&b.style.width, Size::LengthPercentage(lp) if lp.has_percentage()) {
                natural.width
            } else {
                self.replaced_size(b, *natural, *ratio, cb).width
            };
            return (w, w);
        }
        if let Some(w) = self.control_size(b).0 {
            return (w, w);
        }
        if crate::flex::is_flex_container(b) {
            return self.flex_intrinsic_widths(b);
        }
        // E4: grid y tablas.
        if crate::grid::is_grid_container(b) {
            return self.grid_intrinsic_widths(b);
        }
        if let Some(w) = self.table_intrinsic_widths(b) {
            return w;
        }
        if b.has_inline_children() {
            return self.inline_intrinsic_widths(b);
        }
        let (mut min, mut max) = (0.0f32, 0.0f32);
        for child in &b.children {
            if child.is_absolutely_positioned() || matches!(child.kind, BoxKind::Marker { .. }) {
                continue;
            }
            let (cmin, cmax) = self.outer_intrinsic_widths(child);
            min = min.max(cmin);
            max = max.max(cmax);
        }
        (min, max)
    }

    /// Anchos intrínsecos del margin box (porcentajes como 0).
    pub(crate) fn outer_intrinsic_widths(&mut self, b: &'a LayoutBox) -> (f32, f32) {
        let s = &b.style;
        let edges = border_edges(s).horizontal()
            + padding_edges(s, 0.0).horizontal()
            + margin_edges(s, 0.0).horizontal();
        let inner = match specified_width(
            s,
            0.0,
            border_edges(s).horizontal() + padding_edges(s, 0.0).horizontal(),
        ) {
            Some(w) if !matches!(&s.width, Size::LengthPercentage(lp) if lp.has_percentage()) => {
                (w, w)
            }
            _ => self.intrinsic_widths(b),
        };
        // min-width/max-width limitan la contribución (los porcentajes se ignoran: no hay
        // containing block todavía).
        let bp = border_edges(s).horizontal() + padding_edges(s, 0.0).horizontal();
        let pct_min = matches!(&s.min_width, Size::LengthPercentage(lp) if lp.has_percentage());
        let pct_max = matches!(&s.max_width, MaxSize::LengthPercentage(lp) if lp.has_percentage());
        let (lo, hi) = width_limits(s, 0.0, bp);
        let (lo, hi) = (
            if pct_min { 0.0 } else { lo },
            if pct_max { f32::INFINITY } else { hi },
        );
        let clamp = |v: f32| v.min(hi).max(lo);
        (clamp(inner.0) + edges, clamp(inner.1) + edges)
    }

    /// Fragmento del marcador exterior de un list-item, a la izquierda de la primera línea
    /// (`anchor` = x de la línea y su baseline) o del content box si no hay líneas.
    pub(crate) fn marker_fragment(
        &mut self,
        marker: &'a LayoutBox,
        anchor: Option<(f32, f32)>,
        content_x: f32,
        content_top: f32,
    ) -> Fragment {
        let BoxKind::Marker { text, .. } = &marker.kind else {
            unreachable!("se esperaba un marcador");
        };
        let style = &marker.style;
        let glyphs = self.shape(text, style);
        let width: f32 = glyphs.iter().map(|g| g.advance).sum();
        let m = self.metrics(style);
        let (line_x, baseline) = anchor.unwrap_or((content_x, content_top + m.ascent));
        let rect = Rect::new(
            line_x - width,
            baseline - m.ascent,
            width,
            m.ascent + m.descent,
        );
        let mut f = Fragment::new(FragmentKind::Marker, rect, style.clone());
        f.node = marker.node;
        f.pseudo = marker.pseudo;
        f.text = Some((text.clone(), glyphs));
        f.baseline = Some(baseline);
        f
    }
}

/// ¿El contenido inline genera una line box no vacía (§9.4.2)?
pub(crate) fn inline_has_content(b: &LayoutBox) -> bool {
    match &b.kind {
        BoxKind::Text(t) => !t.is_empty(),
        BoxKind::Inline => {
            let s = &b.style;
            border_edges(s).horizontal() > 0.0
                || padding_edges(s, 0.0).horizontal() > 0.0
                || margin_edges(s, 0.0).horizontal() != 0.0
                || b.children.iter().any(inline_has_content)
        }
        BoxKind::Marker { outside, .. } => !outside,
        _ if b.is_out_of_flow() => false,
        _ => true,
    }
}

/// La primera line box (x, baseline) entre los fragmentos y sus descendientes en flujo.
pub(crate) fn first_line(frags: &[Fragment]) -> Option<(f32, f32)> {
    for f in frags {
        if f.kind == FragmentKind::Line {
            return Some((f.rect.x, f.baseline.unwrap_or(f.rect.y)));
        }
        if f.kind == FragmentKind::Block
            && let Some(l) = first_line(&f.children)
        {
            return Some(l);
        }
    }
    None
}

/// Layout de un documento ya estilado.
pub(crate) fn layout_root<'a>(layout: &mut Layout<'a>, root: &'a LayoutBox) -> Fragment {
    let icb = Cb {
        x: 0.0,
        y: 0.0,
        width: layout.viewport.width,
        height: Some(layout.viewport.height),
    };
    layout.root = Some(root);
    layout.abs_frames.push(Vec::new());
    let mut floats = FloatContext::default();
    // Los márgenes del elemento raíz no colapsan (§8.3.1).
    let top = margin_value(&root.style.margin_top, icb.width).unwrap_or(0.0);
    let (mut fragment, _) = layout.block(root, icb, top, &mut floats, true);
    let icb_rect = Rect::new(0.0, 0.0, icb.width, layout.viewport.height);
    let pending = layout.abs_frames.pop().unwrap_or_default();
    for p in pending {
        let f = layout.absolute(p, icb_rect);
        fragment.children.push(f);
    }
    for p in std::mem::take(&mut layout.fixed) {
        let f = layout.absolute(p, icb_rect);
        fragment.children.push(f);
    }
    fragment
}
