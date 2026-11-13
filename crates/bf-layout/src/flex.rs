//! CSS Flexible Box Layout Module Level 1, §9 (algoritmo de layout).
//!
//! Solo `writing-mode: horizontal-tb`: el eje principal es horizontal para `row`/`row-reverse`
//! y vertical para `column`/`column-reverse`. E5: `direction: rtl` (§5.1): en filas el
//! main-start es el borde derecho (`row-reverse` lo devuelve a la izquierda); en columnas el
//! cross-start es el borde derecho. `justify-content: start/end/left/right` y
//! `align-self: start/end/self-start/self-end` siguen el modo de escritura (CSS Box Alignment 3
//! §5.1 y §6.1), no la dirección flex.
//!
//! Interpretaciones:
//! - `min-width`/`min-height: auto` (§4.5): para ítems que no son contenedores de scroll, el
//!   mínimo es el "content-based minimum size" (el min-content en el eje principal) limitado por
//!   el tamaño especificado si es definido y por el máximo. Sin la "transferred size suggestion"
//!   de los reemplazados con relación de aspecto.
//! - `flex-basis: content` (o `auto` con el tamaño principal `auto`): max-content en filas; en
//!   columnas, la altura del ítem maquetado con su ancho (cross size) ya resuelto.
//! - Baseline (§8.3 y §9.4): la del primer line box del ítem (en orden de árbol de fragmentos);
//!   sin line boxes se sintetiza con el borde inferior del border box. Solo en filas; en columnas
//!   `baseline` se trata como `flex-start`.

use bf_style::ComputedStyle;
use bf_style::properties::types::{
    Aligned, ContentAlign, Direction, Display, FlexBasis, FlexDirection, FlexWrap, Gap, ItemAlign,
    LengthPercentageAuto, Overflow, Size,
};

use crate::block::{
    Cb, Layout, PendingAbs, border_edges, height_limits, margin_value, padding_edges,
    specified_height, specified_width, width_limits,
};
use crate::box_tree::{BoxKind, LayoutBox};
use crate::float::FloatContext;
use crate::fragment::{Edges, Fragment, FragmentKind, Rect};

pub(crate) fn is_flex_container(b: &LayoutBox) -> bool {
    matches!(b.style.display, Display::Flex | Display::InlineFlex)
        && !matches!(b.kind, BoxKind::Replaced { .. })
}

/// Un ítem durante el algoritmo. Tamaños "principal"/"cruzado" del content box salvo que se
/// diga otra cosa.
struct Item<'a> {
    b: &'a LayoutBox,
    /// Márgenes (`None` = `auto`) en orden: main-start, main-end y, en el eje cruzado, el del
    /// lado físico bajo (arriba/izquierda) y el del alto (abajo/derecha).
    margins: [Option<f32>; 4],
    /// Borde + padding en el eje principal y en el cruzado.
    bp_main: f32,
    bp_cross: f32,
    base: f32,
    hyp: f32,
    min_main: f32,
    max_main: f32,
    target: f32,
    frozen: bool,
    grow: f32,
    shrink: f32,
    /// Tamaño cruzado del content box (hipotético, luego usado).
    cross: f32,
    /// Distancia del borde cruzado-inicial del margin box a la baseline (filas).
    baseline: Option<f32>,
    align: ItemAlign,
    /// E5: alineación propia `safe` (CSS Box Alignment 3 §4.4).
    safe: bool,
}

impl Item<'_> {
    fn margin(&self, i: usize) -> f32 {
        self.margins[i].unwrap_or(0.0)
    }
    fn outer_main(&self, size: f32) -> f32 {
        size + self.bp_main + self.margin(0) + self.margin(1)
    }
    fn outer_cross(&self) -> f32 {
        self.cross + self.bp_cross + self.margin(2) + self.margin(3)
    }
}

struct Line {
    items: Vec<usize>,
    /// E5: ítems con `visibility: collapse` de la línea (segunda pasada de §4.4).
    collapsed: Vec<usize>,
    cross: f32,
    /// Máximo de la distancia a la baseline de los ítems alineados por baseline.
    baseline: f32,
    offset: f32,
}

fn gap_px(g: &Gap, basis: f32) -> f32 {
    match g {
        Gap::Normal => 0.0,
        Gap::LengthPercentage(lp) => lp.resolve(basis),
    }
}

fn resolved_align(item: &ComputedStyle, container: &ComputedStyle) -> ItemAlign {
    // E5: `left`/`right` no son válidos en `align-*` (cuentan como `auto`); `last baseline`
    // cae a `end` sin baselines compartidas (CSS Box Alignment 3 §9.3).
    let fix = |a: ItemAlign| match a {
        ItemAlign::Left | ItemAlign::Right => ItemAlign::Auto,
        a => a,
    };
    let a = if fix(item.align_self.value) == ItemAlign::Auto {
        fix(container.align_items.value)
    } else {
        item.align_self.value
    };
    match a {
        ItemAlign::Auto | ItemAlign::Normal => ItemAlign::Stretch,
        ItemAlign::LastBaseline | ItemAlign::FlowEnd => ItemAlign::FlexEnd,
        ItemAlign::FlowStart => ItemAlign::FlexStart,
        a => a,
    }
}

/// E5: ¿la alineación propia resuelta del ítem es `safe` (CSS Box Alignment 3 §4.4)?
fn align_is_safe(item: &ComputedStyle, container: &ComputedStyle) -> bool {
    let own = !matches!(
        item.align_self.value,
        ItemAlign::Auto | ItemAlign::Left | ItemAlign::Right
    );
    if own {
        item.align_self.is_safe()
    } else {
        container.align_items.is_safe()
    }
}

/// E5: `justify-content`/`align-content` de un contenedor flex: `flow-start`/`flow-end` son
/// `flex-start`/`flex-end`, y con `safe` y desbordamiento, `start` (CSS Box Alignment 3 §4.4).
fn flex_content_align(a: Aligned<ContentAlign>, free: f32) -> ContentAlign {
    match a.value {
        _ if a.is_safe() && free < 0.0 => ContentAlign::Start,
        ContentAlign::FlowStart => ContentAlign::FlexStart,
        ContentAlign::FlowEnd => ContentAlign::FlexEnd,
        v => v,
    }
}

/// Primera baseline de un fragmento (y absoluta): la del primer line box en orden.
pub(crate) fn first_baseline(f: &Fragment) -> Option<f32> {
    if f.kind == FragmentKind::Line {
        return f.baseline;
    }
    for c in &f.children {
        if matches!(
            c.kind,
            FragmentKind::Line | FragmentKind::Block | FragmentKind::InlineBlock
        ) && c.style.float == bf_style::properties::types::Float::None
            && !matches!(
                c.style.position,
                bf_style::properties::types::Position::Absolute
                    | bf_style::properties::types::Position::Fixed
            )
            && let Some(b) = first_baseline(c)
        {
            return Some(b);
        }
    }
    None
}

/// Baseline de un contenedor flex (§8.5): la del primer ítem; si no tiene, sintetizada con
/// el borde inferior de su border box. `None` si no hay ítems.
///
/// E5: si el primer ítem no tiene baseline, se sintetiza de su border box (§8.5) con `synth`
/// (de la altura a la baseline; ver `writing_mode::synthesized_baseline`).
pub(crate) fn container_baseline(f: &Fragment, synth: &dyn Fn(f32) -> f32) -> Option<f32> {
    let item = f.children.iter().find(|c| {
        !matches!(
            c.style.position,
            bf_style::properties::types::Position::Absolute
                | bf_style::properties::types::Position::Fixed
        )
    })?;
    Some(first_baseline(item).unwrap_or(item.rect.y + synth(item.rect.height)))
}

/// E5: orientación de los ejes de un contenedor flex (§5.1), para alinear absolutos.
struct AbsAxes {
    row: bool,
    reverse: bool,
    main_rev: bool,
    cross_flip: bool,
    rtl: bool,
}

/// E5: fracción del espacio libre (física: 0 izquierda/arriba, 1 derecha/abajo) en cada eje
/// `(x, y)` para la posición estática de un hijo absoluto de un contenedor flex (§4.1).
/// `free_main` es el espacio libre en el eje principal: las distribuciones `space-around` y
/// `space-evenly` caen a `safe center` (CSS Box Alignment 3 §5.3) y `space-between` y
/// `stretch` a `flex-start`.
fn abs_static_fractions(
    b: &LayoutBox,
    cs: &ComputedStyle,
    ax: &AbsAxes,
    (free_main, free_cross): (f32, f32),
) -> (f32, f32) {
    // Eje principal, medido desde el main-start.
    let main = match flex_content_align(cs.justify_content, free_main) {
        ContentAlign::FlexEnd => 1.0,
        ContentAlign::Start => f32::from(u8::from(ax.reverse)),
        ContentAlign::End => f32::from(u8::from(!ax.reverse)),
        ContentAlign::Left if ax.row => f32::from(u8::from(ax.main_rev)),
        ContentAlign::Right if ax.row => f32::from(u8::from(!ax.main_rev)),
        ContentAlign::Left | ContentAlign::Right => f32::from(u8::from(ax.reverse)),
        ContentAlign::Center => 0.5,
        ContentAlign::SpaceAround | ContentAlign::SpaceEvenly if free_main >= 0.0 => 0.5,
        _ => 0.0,
    };
    let main = if ax.main_rev { 1.0 - main } else { main };
    // Eje cruzado (físico): como en la alineación de los ítems; `stretch`, `normal` y
    // `baseline` cuentan como `flex-start`.
    let item_rtl = !ax.row && b.style.direction == Direction::Rtl;
    let container_rtl = !ax.row && ax.rtl;
    let high = |h: bool| f32::from(u8::from(h));
    let align = if align_is_safe(&b.style, cs) && free_cross < 0.0 {
        ItemAlign::Start
    } else {
        resolved_align(&b.style, cs)
    };
    let cross = match align {
        ItemAlign::FlexEnd => high(!ax.cross_flip),
        ItemAlign::End => high(!container_rtl),
        ItemAlign::SelfEnd => high(!item_rtl),
        ItemAlign::Start => high(container_rtl),
        ItemAlign::SelfStart => high(item_rtl),
        ItemAlign::Center => 0.5,
        _ => high(ax.cross_flip),
    };
    if ax.row { (main, cross) } else { (cross, main) }
}

impl<'a> Layout<'a> {
    /// Ejecuta `f` sin dejar registradas las cajas absolutas que descubra (para medir).
    pub(crate) fn measuring<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let abs = self.abs_frames.last().map_or(0, Vec::len);
        let fixed = self.fixed.len();
        let r = f(self);
        if let Some(frame) = self.abs_frames.last_mut() {
            frame.truncate(abs);
        }
        self.fixed.truncate(fixed);
        r
    }

    /// Maqueta un ítem con su border box en `(x, y)`, ancho de contenido `content_w` y, si
    /// se da, altura de contenido forzada.
    fn layout_item(
        &mut self,
        b: &'a LayoutBox,
        cb: Cb,
        (x, y): (f32, f32),
        content_w: f32,
        height: Option<f32>,
        margins: Edges,
    ) -> Fragment {
        // E5: un ítem de otro modo de escritura es la raíz de un flujo ortogonal (CSS Writing
        // Modes 4 §7.3; CSS Flexbox 1 §9.2.3): se maqueta en su marco con el ancho y el alto
        // (si se fija) que le da el algoritmo flex, y su border box queda en `(x, y)`.
        if self.is_orthogonal(b) {
            let mut f = self.orthogonal_item_fragment(b, cb, content_w, height);
            let (dx, dy) = (x - f.rect.x, y - f.rect.y);
            f.translate(dx, dy);
            return f;
        }
        self.forced_height = height;
        let mut scratch = FloatContext::default();
        let (f, _) = self.box_contents(b, cb, x, y, content_w, margins, &mut scratch, false);
        self.forced_height = None;
        f
    }

    /// Ancho de contenido "fit-content" de un ítem en `available` (sin márgenes ni bordes).
    fn item_fit_width(&mut self, b: &'a LayoutBox, available: f32) -> f32 {
        if let BoxKind::Replaced { natural, ratio } = &b.kind {
            let cb = Cb {
                x: 0.0,
                y: 0.0,
                width: available,
                height: None,
            };
            return self.replaced_size(b, *natural, *ratio, cb).width;
        }
        let (min, max) = self.intrinsic_widths(b);
        min.max(available).min(max)
    }

    /// Altura de contenido de un ítem maquetado con ancho de contenido `w`. E5: sin su propio
    /// `height` ni sus límites (el tamaño de contenido, §4.5 y §9.2.3 E).
    fn item_content_height(&mut self, b: &'a LayoutBox, cb: Cb, w: f32, bp_v: f32) -> f32 {
        self.measuring(|this| {
            // Con `aspect-ratio` el alto de contenido sale de la relación (CSS Sizing 4 §5.2).
            this.content_probe = b.style.aspect_ratio.preferred().is_none();
            let f = this.layout_item(b, cb, (0.0, 0.0), w, None, Edges::default());
            this.content_probe = false;
            f.rect.height - bp_v
        })
    }

    /// Layout del contenido de un contenedor flex: fragmentos de los ítems y altura del
    /// contenido. `cb` es el content box del contenedor (`height` si es definida).
    pub(crate) fn flex_layout(&mut self, container: &'a LayoutBox, cb: Cb) -> (Vec<Fragment>, f32) {
        self.flex_layout_pass(container, cb, None)
    }

    /// E5: una pasada del algoritmo. Con ítems `visibility: collapse` (§4.4), la primera
    /// anota el tamaño cruzado de la línea de cada uno ("strut") y repite el layout; en la
    /// segunda (`struts`), esos ítems miden 0 en el eje principal al formar las líneas, se
    /// ignoran después y cada línea mide al menos el mayor strut de sus ítems colapsados.
    fn flex_layout_pass(
        &mut self,
        container: &'a LayoutBox,
        cb: Cb,
        struts: Option<&[f32]>,
    ) -> (Vec<Fragment>, f32) {
        let abs_mark = (self.abs_frames.last().map_or(0, Vec::len), self.fixed.len());
        let is_collapsed =
            |b: &LayoutBox| b.style.visibility == bf_style::properties::types::Visibility::Collapse;
        let cs = &container.style;
        let row = matches!(
            cs.flex_direction,
            FlexDirection::Row | FlexDirection::RowReverse
        );
        let reverse = matches!(
            cs.flex_direction,
            FlexDirection::RowReverse | FlexDirection::ColumnReverse
        );
        let wrap = cs.flex_wrap != FlexWrap::Nowrap;
        let wrap_reverse = cs.flex_wrap == FlexWrap::WrapReverse;
        // E5: CSS Flexbox 1 §5.1 con `direction: rtl`. `main_rev`: el main-start está en el
        // borde físico alto (derecha en filas, abajo en columnas). `cross_flip`: el cross-start
        // está en el borde físico alto (abajo en filas, derecha en columnas).
        let rtl = cs.direction == Direction::Rtl;
        let main_rev = if row { reverse != rtl } else { reverse };
        let cross_flip = wrap_reverse != (!row && rtl);
        // Espacio disponible: el ancho siempre es definido; el alto, si se especificó.
        let avail_main = if row { Some(cb.width) } else { cb.height };
        let main_gap = if row {
            gap_px(&cs.column_gap, cb.width)
        } else {
            gap_px(&cs.row_gap, cb.height.unwrap_or(0.0))
        };
        let cross_gap = if row {
            gap_px(&cs.row_gap, cb.height.unwrap_or(0.0))
        } else {
            gap_px(&cs.column_gap, cb.width)
        };

        // --- Ítems en orden (`order`, estable) ---
        let mut children: Vec<&'a LayoutBox> = Vec::new();
        let mut out = Vec::new();
        // E5: hueco de cada absoluto entre los pendientes (fijo, índice), en orden de árbol; la
        // posición estática se alinea al final, con el tamaño del contenedor (§4.1).
        let mut abs_slots: Vec<(&'a LayoutBox, bool, usize)> = Vec::new();
        for c in &container.children {
            if c.is_absolutely_positioned() {
                let (abs, fixed) = self.pending_abs_marks();
                let is_fixed = c.style.position == bf_style::properties::types::Position::Fixed;
                abs_slots.push((c, is_fixed, if is_fixed { fixed } else { abs }));
                self.add_abs(c, cb.x, cb.y, false);
                continue;
            }
            if matches!(c.kind, BoxKind::Marker { .. }) {
                continue;
            }
            children.push(c);
        }
        children.sort_by_key(|c| c.style.order.0);

        let mut items: Vec<Item<'a>> = Vec::with_capacity(children.len());
        for &b in &children {
            let s = &b.style;
            let border = border_edges(s);
            let padding = padding_edges(s, cb.width);
            let (bp_h, bp_v) = (
                border.horizontal() + padding.horizontal(),
                border.vertical() + padding.vertical(),
            );
            let m = |v: &LengthPercentageAuto| margin_value(v, cb.width);
            let (l, r, t, bm) = (
                m(&s.margin_left),
                m(&s.margin_right),
                m(&s.margin_top),
                m(&s.margin_bottom),
            );
            let margins = match (row, main_rev) {
                (true, false) => [l, r, t, bm],
                (true, true) => [r, l, t, bm],
                (false, false) => [t, bm, l, r],
                (false, true) => [bm, t, l, r],
            };
            let (bp_main, bp_cross) = if row { (bp_h, bp_v) } else { (bp_v, bp_h) };
            let align = resolved_align(s, cs);
            let auto_cross_margin = margins[2].is_none() || margins[3].is_none();
            // Columnas: el ancho (cruzado) se decide antes que el principal.
            let column_cross = if row {
                0.0
            } else {
                let outer = margins[2].unwrap_or(0.0) + margins[3].unwrap_or(0.0) + bp_h;
                let available = (cb.width - outer).max(0.0);
                let (wmin, wmax) = self.width_limits_for(b, cb.width, bp_h, available);
                let kw = s.width.clone();
                match specified_width(s, cb.width, bp_h) {
                    Some(w) => w,
                    // E5: keywords de tamaño (CSS Sizing 3 §3.2, CSS Sizing 4 §6.1).
                    None if kw != Size::Auto => self
                        .width_for_keyword(b, &kw, available, cb.width)
                        .unwrap_or(available),
                    // E5: con wrap, fit-content (§9.4 paso 7); se estira con la línea.
                    None if align == ItemAlign::Stretch && !auto_cross_margin && !wrap => available,
                    None => self.item_fit_width(b, available),
                }
                .min(wmax)
                .max(wmin)
            };
            // Tamaño principal especificado (content box).
            // E5: en filas, `width` con keyword cuenta como tamaño principal especificado
            // (CSS Sizing 3 §3.2).
            let main_avail =
                (cb.width - margins[0].unwrap_or(0.0) - margins[1].unwrap_or(0.0) - bp_h).max(0.0);
            let spec_main = if row {
                let kw = s.width.clone();
                specified_width(s, cb.width, bp_h)
                    .or_else(|| self.width_for_keyword(b, &kw, main_avail, cb.width))
            } else {
                specified_height(s, cb.height, bp_v)
            };
            // Tamaño de contenido en el eje principal (max-content en filas). E5: con
            // `aspect-ratio` y alto definido, el ancho transferido (CSS Flexbox 1 §9.2.3 B).
            let ratio_main = if row { self.ratio_width(b, cb) } else { None };
            // Con relación de aspecto y un tamaño cruzado definido por estiramiento (fila con
            // alto definido), la base sale de ese alto (§9.2.3 B).
            let item_ratio = match &b.kind {
                BoxKind::Replaced { ratio, .. } => ratio.or(s.aspect_ratio.preferred()),
                _ => s.aspect_ratio.preferred(),
            };
            let ratio_main = ratio_main.or_else(|| {
                let r = item_ratio?;
                let stretched = row
                    && align == ItemAlign::Stretch
                    && !auto_cross_margin
                    && s.height == Size::Auto
                    && !wrap;
                let h = cb.height.filter(|_| stretched)?;
                let inner =
                    (h - margins[2].unwrap_or(0.0) - margins[3].unwrap_or(0.0) - bp_v).max(0.0);
                let (lo, hi) = height_limits(s, cb.height, bp_v);
                Some(inner.min(hi).max(lo) * r)
            });
            let content_main = |this: &mut Self| -> f32 {
                if let Some(w) = ratio_main {
                    return w;
                }
                if row {
                    if let BoxKind::Replaced { natural, ratio } = &b.kind {
                        let rcb = Cb {
                            x: 0.0,
                            y: 0.0,
                            width: cb.width,
                            height: cb.height,
                        };
                        return this.replaced_size(b, *natural, *ratio, rcb).width;
                    }
                    this.intrinsic_widths(b).1
                } else {
                    this.item_content_height(b, cb, column_cross, bp_v)
                }
            };
            // --- Flex base size (§9.2.3) ---
            // Un porcentaje contra un tamaño principal indefinido se trata como `content`.
            let indefinite = |lp: &bf_style::values::LengthPercentage| {
                !row && cb.height.is_none() && lp.has_percentage()
            };
            let base = match &s.flex_basis {
                FlexBasis::Size(Size::LengthPercentage(lp)) if !indefinite(lp) => {
                    let basis = if row {
                        cb.width
                    } else {
                        cb.height.unwrap_or(0.0)
                    };
                    let v = lp.resolve(basis);
                    if s.box_sizing == bf_style::properties::types::BoxSizing::BorderBox {
                        (v - bp_main).max(0.0)
                    } else {
                        v
                    }
                }
                FlexBasis::Size(Size::Auto) if spec_main.is_some() => spec_main.unwrap_or(0.0),
                // E5: `flex-basis` con keyword de tamaño (CSS Flexbox 1 §7.2.3 y CSS Sizing 3
                // §3.2): en filas, el ancho de esa keyword; en columnas, el alto del contenido.
                FlexBasis::Size(kw) if row && *kw != Size::Auto => {
                    let kw = kw.clone();
                    match self.width_for_keyword(b, &kw, main_avail, cb.width) {
                        Some(w) => w,
                        None => content_main(self),
                    }
                }
                _ => content_main(self),
            };
            // --- min/max en el eje principal, con el mínimo automático (§4.5) ---
            let (mut min_main, mut max_main) = if row {
                self.width_limits_for(b, cb.width, bp_h, main_avail)
            } else {
                height_limits(s, cb.height, bp_v)
            };
            // E5: en columnas, `min-height`/`max-height` con keyword intrínseca son el alto del
            // contenido (CSS Sizing 3 §3.2).
            if !row && (s.min_height.is_intrinsic() || s.max_height.as_size().is_intrinsic()) {
                let h = self.item_content_height(b, cb, column_cross, bp_v);
                if s.min_height.is_intrinsic() {
                    min_main = min_main.max(h);
                }
                if s.max_height.as_size().is_intrinsic() {
                    max_main = h;
                }
            }
            let min_auto = if row {
                matches!(s.min_width, Size::Auto)
            } else {
                matches!(s.min_height, Size::Auto)
            };
            let overflow = if row { s.overflow_x } else { s.overflow_y };
            if min_auto && matches!(overflow, Overflow::Visible | Overflow::Clip) {
                let content_min = if row {
                    if let BoxKind::Replaced { natural, ratio } = &b.kind {
                        // E5: la sugerencia por contenido es el min-content sin el `width`
                        // propio (ese es la sugerencia por tamaño especificado): con relación de
                        // aspecto y alto definido, el alto transferido por la relación; si no, el
                        // ancho natural; y con relación, acotado por `min-height`/`max-height`
                        // transferidos (§4.5).
                        let r = ratio.filter(|r| *r > 0.0);
                        let w = match (specified_height(s, cb.height, bp_v), r) {
                            (Some(h), Some(r)) => Some(h * r),
                            _ if natural.width > 0.0 => Some(natural.width),
                            _ => None,
                        };
                        match (w, r) {
                            (Some(w), Some(r)) => {
                                let (hlo, hhi) = height_limits(s, cb.height, bp_v);
                                w.min(hhi * r).max(hlo * r)
                            }
                            (Some(w), None) => w,
                            (None, _) => {
                                let rcb = Cb {
                                    x: 0.0,
                                    y: 0.0,
                                    width: cb.width,
                                    height: cb.height,
                                };
                                self.replaced_size(b, *natural, *ratio, rcb).width
                            }
                        }
                    } else {
                        self.intrinsic_widths(b).0
                    }
                } else if let BoxKind::Replaced { natural, ratio } = &b.kind
                    && let Some(h) = {
                        // E5: lo mismo en columnas: el ancho definido transferido por la relación
                        // de aspecto o el alto natural, acotado por `min-width`/`max-width`
                        // transferidos (§4.5).
                        let r = ratio.filter(|r| *r > 0.0);
                        let h = match (specified_width(s, cb.width, bp_h), r) {
                            (Some(w), Some(r)) => Some(w / r),
                            _ if natural.height > 0.0 => Some(natural.height),
                            _ => None,
                        };
                        match (h, r) {
                            (Some(h), Some(r)) => {
                                let (wlo, whi) = width_limits(s, cb.width, bp_h);
                                Some(h.min(whi / r).max(wlo / r))
                            }
                            (h, _) => h,
                        }
                    }
                {
                    h
                } else {
                    self.item_content_height(b, cb, column_cross, bp_v)
                };
                let suggestion = spec_main.map_or(content_min, |sp| sp.min(content_min));
                min_main = suggestion.min(max_main);
            }
            let max_main = max_main.max(min_main);
            let hyp = base.min(max_main).max(min_main);
            let (base, hyp, min_main, max_main) = if struts.is_some() && is_collapsed(b) {
                (0.0, 0.0, 0.0, 0.0)
            } else {
                (base, hyp, min_main, max_main)
            };
            items.push(Item {
                b,
                margins,
                bp_main,
                bp_cross,
                base,
                hyp,
                min_main,
                max_main,
                target: hyp,
                frozen: false,
                grow: s.flex_grow.0,
                shrink: s.flex_shrink.0,
                cross: column_cross,
                baseline: None,
                align,
                safe: align_is_safe(s, cs),
            });
        }

        // --- Líneas (§9.3) ---
        let mut lines: Vec<Line> = Vec::new();
        {
            let mut current: Vec<usize> = Vec::new();
            let mut current_collapsed: Vec<usize> = Vec::new();
            let mut used = 0.0;
            for (i, item) in items.iter().enumerate() {
                if struts.is_some() && is_collapsed(item.b) {
                    current_collapsed.push(i);
                    continue;
                }
                let outer = item.outer_main(item.hyp);
                let extra = if current.is_empty() {
                    outer
                } else {
                    outer + main_gap
                };
                if wrap
                    && !current.is_empty()
                    && avail_main.is_some_and(|a| used + extra > a + 0.01)
                {
                    lines.push(Line {
                        items: std::mem::take(&mut current),
                        collapsed: std::mem::take(&mut current_collapsed),
                        cross: 0.0,
                        baseline: 0.0,
                        offset: 0.0,
                    });
                    used = 0.0;
                    current.push(i);
                    used += outer;
                    continue;
                }
                current.push(i);
                used += extra;
            }
            lines.push(Line {
                items: current,
                collapsed: current_collapsed,
                cross: 0.0,
                baseline: 0.0,
                offset: 0.0,
            });
        }

        // --- Longitudes flexibles (§9.7) ---
        for line in &lines {
            let n_gaps = line.items.len().saturating_sub(1) as f32 * main_gap;
            let available = match avail_main {
                Some(a) => a,
                // Tamaño principal indefinido (columna con altura auto): sin flexión.
                None => {
                    for &i in &line.items {
                        items[i].target = items[i].hyp;
                    }
                    continue;
                }
            };
            let sum_hyp: f32 = line
                .items
                .iter()
                .map(|&i| items[i].outer_main(items[i].hyp))
                .sum::<f32>()
                + n_gaps;
            let growing = sum_hyp < available;
            for &i in &line.items {
                let it = &mut items[i];
                let factor = if growing { it.grow } else { it.shrink };
                it.frozen = factor == 0.0
                    || (growing && it.base > it.hyp)
                    || (!growing && it.base < it.hyp);
                it.target = if it.frozen { it.hyp } else { it.base };
            }
            let free_space = |items: &[Item<'_>]| {
                available
                    - n_gaps
                    - line
                        .items
                        .iter()
                        .map(|&i| {
                            let it = &items[i];
                            it.outer_main(if it.frozen { it.target } else { it.base })
                        })
                        .sum::<f32>()
            };
            let initial_free = free_space(&items);
            for _ in 0..line.items.len() + 1 {
                if line.items.iter().all(|&i| items[i].frozen) {
                    break;
                }
                let mut remaining = free_space(&items);
                let sum_factors: f32 = line
                    .items
                    .iter()
                    .filter(|&&i| !items[i].frozen)
                    .map(|&i| {
                        if growing {
                            items[i].grow
                        } else {
                            items[i].shrink
                        }
                    })
                    .sum();
                if sum_factors < 1.0 {
                    let x = initial_free * sum_factors;
                    if x.abs() < remaining.abs() {
                        remaining = x;
                    }
                }
                if remaining != 0.0 {
                    if growing {
                        for &i in &line.items {
                            let it = &mut items[i];
                            if !it.frozen && sum_factors > 0.0 {
                                it.target = it.base + remaining * it.grow / sum_factors;
                            }
                        }
                    } else {
                        let sum_scaled: f32 = line
                            .items
                            .iter()
                            .filter(|&&i| !items[i].frozen)
                            .map(|&i| items[i].shrink * items[i].base)
                            .sum();
                        for &i in &line.items {
                            let it = &mut items[i];
                            if !it.frozen && sum_scaled > 0.0 {
                                it.target =
                                    it.base - remaining.abs() * it.shrink * it.base / sum_scaled;
                            }
                        }
                    }
                }
                // Violaciones de min/max.
                let mut total = 0.0;
                let mut violation = vec![0.0f32; items.len()];
                for &i in &line.items {
                    let it = &mut items[i];
                    if it.frozen {
                        continue;
                    }
                    let clamped = it.target.min(it.max_main).max(it.min_main).max(0.0);
                    violation[i] = clamped - it.target;
                    total += violation[i];
                    it.target = clamped;
                }
                for &i in &line.items {
                    let it = &mut items[i];
                    if it.frozen {
                        continue;
                    }
                    if total.abs() < 0.001
                        || (total > 0.0 && violation[i] > 0.0)
                        || (total < 0.0 && violation[i] < 0.0)
                    {
                        it.frozen = true;
                    }
                }
            }
        }

        // --- Tamaños cruzados hipotéticos (§9.4) ---
        for item in &mut items {
            if row {
                let b = item.b;
                let w = item.target;
                let bp_v = item.bp_cross;
                let (f, h) = self.measuring(|this| {
                    let f = this.layout_item(b, cb, (0.0, 0.0), w, None, Edges::default());
                    let h = f.rect.height - bp_v;
                    (f, h)
                });
                item.cross = h;
                if item.align == ItemAlign::Baseline {
                    let border_top = f.rect.y;
                    // E5: sin baseline propia se sintetiza del border box (CSS Box Alignment 3
                    // §9.1): el borde inferior o, en un flujo vertical con baseline central
                    // (CSS Inline 3 §4.1), el centro.
                    let synth = crate::writing_mode::synthesized_baseline(
                        self.frame,
                        &b.style,
                        f.rect.height,
                    );
                    item.baseline = Some(
                        item.margin(2) + first_baseline(&f).map_or(synth, |b| b - border_top),
                    );
                }
            }
        }
        // Tamaño cruzado de cada línea.
        let single_definite_cross = if row { cb.height } else { Some(cb.width) };
        for line in &mut lines {
            let mut cross: f32 = 0.0;
            let (mut above, mut below): (f32, f32) = (0.0, 0.0);
            for &i in &line.items {
                let it = &items[i];
                if let Some(b) = it.baseline
                    && it.margins[2].is_some()
                    && it.margins[3].is_some()
                {
                    above = above.max(b);
                    below = below.max(it.outer_cross() - b);
                } else {
                    cross = cross.max(it.outer_cross());
                }
            }
            line.baseline = above;
            line.cross = cross.max(above + below);
        }
        if !wrap && let (Some(def), Some(line)) = (single_definite_cross, lines.first_mut()) {
            line.cross = def;
        }
        // E5: sin tamaño cruzado definido (filas con alto auto), una sola línea y el
        // contenedor quedan dentro de `min-height`/`max-height` (§9.4 pasos 8 y 15).
        let cross_limits = if row && single_definite_cross.is_none() {
            let bp_v = border_edges(cs).vertical() + padding_edges(cs, cb.width).vertical();
            Some(height_limits(cs, None, bp_v))
        } else {
            None
        };
        if !wrap && let (Some((lo, hi)), Some(line)) = (cross_limits, lines.first_mut()) {
            line.cross = line.cross.min(hi).max(lo);
        }
        // E5: `visibility: collapse` (§4.4 y §9.4 paso 9).
        match struts {
            None if items.iter().any(|it| is_collapsed(it.b)) => {
                let mut s = vec![0.0f32; items.len()];
                for line in &lines {
                    for &i in &line.items {
                        s[i] = line.cross;
                    }
                }
                if let Some(frame) = self.abs_frames.last_mut() {
                    frame.truncate(abs_mark.0);
                }
                self.fixed.truncate(abs_mark.1);
                return self.flex_layout_pass(container, cb, Some(&s));
            }
            Some(s) => {
                for line in &mut lines {
                    for &i in &line.collapsed {
                        line.cross = line.cross.max(s[i]);
                    }
                }
            }
            None => {}
        }
        // Tamaño cruzado del contenedor y align-content (§9.4 paso 15 y §9.6).
        let lines_cross: f32 = lines.iter().map(|l| l.cross).sum::<f32>()
            + lines.len().saturating_sub(1) as f32 * cross_gap;
        let container_cross = match (single_definite_cross, cross_limits) {
            (Some(c), _) => c,
            (None, Some((lo, hi))) => lines_cross.min(hi).max(lo),
            (None, None) => lines_cross,
        };
        {
            let free = container_cross - lines_cross;
            let n = lines.len() as f32;
            let (mut start, mut between) = (0.0, 0.0);
            if wrap {
                // E5: `start`/`end` siguen el modo de escritura: coinciden con el cross-start
                // salvo con `wrap-reverse`.
                match flex_content_align(cs.align_content, free) {
                    ContentAlign::Normal | ContentAlign::Stretch if free > 0.0 => {
                        let extra = free / n;
                        for l in &mut lines {
                            l.cross += extra;
                        }
                    }
                    ContentAlign::Start if wrap_reverse => start = free,
                    ContentAlign::End if !wrap_reverse => start = free,
                    ContentAlign::FlexEnd => start = free,
                    ContentAlign::Center => start = free / 2.0,
                    ContentAlign::SpaceBetween if n > 1.0 && free > 0.0 => {
                        between = free / (n - 1.0)
                    }
                    ContentAlign::SpaceAround if free > 0.0 => {
                        between = free / n;
                        start = between / 2.0;
                    }
                    // E5: con espacio negativo, `safe center` (§8.4): al inicio.
                    ContentAlign::SpaceAround => {}
                    ContentAlign::SpaceEvenly if free > 0.0 => {
                        between = free / (n + 1.0);
                        start = between;
                    }
                    ContentAlign::SpaceEvenly => {}
                    _ => {}
                }
            }
            let mut offset = start;
            for l in &mut lines {
                l.offset = offset;
                offset += l.cross + cross_gap + between;
            }
            if cross_flip {
                for l in &mut lines {
                    l.offset = container_cross - l.offset - l.cross;
                }
            }
        }

        // --- Stretch (§9.4 paso 11) ---
        for line in &lines {
            for &i in &line.items {
                let it = &mut items[i];
                let auto_margin = it.margins[2].is_none() || it.margins[3].is_none();
                let cross_auto = if row {
                    matches!(it.b.style.height, Size::Auto)
                } else {
                    matches!(it.b.style.width, Size::Auto)
                };
                if it.align == ItemAlign::Stretch && cross_auto && !auto_margin {
                    let (lo, hi) = if row {
                        height_limits(&it.b.style, cb.height, it.bp_cross)
                    } else {
                        width_limits(&it.b.style, cb.width, it.bp_cross)
                    };
                    let stretched = line.cross - it.bp_cross - it.margin(2) - it.margin(3);
                    it.cross = stretched.min(hi).max(lo).max(0.0);
                } else if !row && wrap && cross_auto {
                    // E5: en una columna multilínea, un ítem que no se estira se vuelve a
                    // ajustar (fit-content) al tamaño cruzado final de su línea, sin cambiar su
                    // tamaño principal (csswg-drafts#11784; WPT `align-self-016`).
                    let (lo, hi) = width_limits(&it.b.style, cb.width, it.bp_cross);
                    let avail = (line.cross - it.bp_cross - it.margin(2) - it.margin(3)).max(0.0);
                    let b = it.b;
                    let fit = self.item_fit_width(b, avail);
                    items[i].cross = fit.min(hi).max(lo).max(0.0);
                }
            }
        }

        // --- Alineación en el eje principal (§9.5) y posiciones finales ---
        let main_len = avail_main.unwrap_or_else(|| {
            lines
                .iter()
                .map(|l| {
                    l.items
                        .iter()
                        .map(|&i| items[i].outer_main(items[i].target))
                        .sum::<f32>()
                        + l.items.len().saturating_sub(1) as f32 * main_gap
                })
                .fold(0.0, f32::max)
        });
        let main_len = if row {
            main_len
        } else {
            // Columna con altura auto: min/max del contenedor.
            let bp_v = border_edges(cs).vertical() + padding_edges(cs, cb.width).vertical();
            let (lo, hi) = height_limits(cs, None, bp_v);
            match avail_main {
                Some(a) => a,
                None => main_len.min(hi).max(lo),
            }
        };
        for line in &lines {
            let n = line.items.len() as f32;
            let used: f32 = line
                .items
                .iter()
                .map(|&i| items[i].outer_main(items[i].target))
                .sum::<f32>()
                + (n - 1.0).max(0.0) * main_gap;
            let mut free = main_len - used;
            let autos = line
                .items
                .iter()
                .map(|&i| items[i].margins[..2].iter().filter(|m| m.is_none()).count())
                .sum::<usize>();
            let mut auto_size = 0.0;
            if autos > 0 && free > 0.0 {
                auto_size = free / autos as f32;
                free = 0.0;
            }
            let (mut start, mut between) = (0.0, 0.0);
            // E5: `start`/`end` según el modo de escritura (en filas, el inline-start coincide
            // con el main-start salvo en `row-reverse`; en columnas, el block-start salvo en
            // `column-reverse`); `left`/`right` son físicos en filas y `start` en columnas
            // (CSS Box Alignment 3 §5.1).
            let jc = flex_content_align(cs.justify_content, free);
            let at_end = match jc {
                ContentAlign::FlexEnd => Some(true),
                ContentAlign::Start => Some(reverse),
                ContentAlign::End => Some(!reverse),
                ContentAlign::Left if row => Some(main_rev),
                ContentAlign::Right if row => Some(!main_rev),
                ContentAlign::Left | ContentAlign::Right => Some(reverse),
                _ => None,
            };
            match jc {
                _ if at_end.is_some() => {
                    if at_end == Some(true) {
                        start = free;
                    }
                }
                ContentAlign::Center => start = free / 2.0,
                ContentAlign::SpaceBetween if n > 1.0 && free > 0.0 => between = free / (n - 1.0),
                ContentAlign::SpaceAround if free > 0.0 => {
                    between = free / n;
                    start = between / 2.0;
                }
                // E5: con espacio negativo, `safe center` (§8.2): al inicio.
                ContentAlign::SpaceAround => {}
                ContentAlign::SpaceEvenly if free > 0.0 => {
                    between = free / (n + 1.0);
                    start = between;
                }
                ContentAlign::SpaceEvenly => {}
                _ => {}
            }
            // `pos` se mide desde el main-start; con `main_rev`, desde el borde físico alto.
            let mut pos = start;
            for &i in &line.items {
                let (m0, m1, size, bp_main) = {
                    let it = &items[i];
                    (
                        it.margins[0].unwrap_or(auto_size),
                        it.margins[1].unwrap_or(auto_size),
                        it.target,
                        it.bp_main,
                    )
                };
                let border_box = size + bp_main;
                let main_start = if main_rev {
                    main_len - pos - m0 - border_box
                } else {
                    pos + m0
                };
                pos += m0 + border_box + m1 + main_gap + between;
                // Eje cruzado (§8.1 y §9.6).
                let it = &items[i];
                let outer = it.outer_cross();
                let line_free = line.cross - outer;
                let cross_margin_start = match (it.margins[2], it.margins[3]) {
                    (None, None) => line_free.max(0.0) / 2.0,
                    (None, Some(_)) => line_free.max(0.0),
                    // E5: el margen `auto` del lado alto absorbe el espacio positivo (§8.1).
                    (Some(m), None) if line_free >= 0.0 => m,
                    (Some(m), _) => {
                        // E5: `flex-start`/`flex-end` siguen el cross-start de la línea;
                        // `start`/`end`, el modo de escritura del contenedor, y
                        // `self-start`/`self-end`, el del ítem (CSS Box Alignment 3 §6.1).
                        let item_rtl = !row && it.b.style.direction == Direction::Rtl;
                        let container_rtl = !row && rtl;
                        // E5: `safe` con desbordamiento alinea como `start` (§4.4).
                        let align = if it.safe && line_free < 0.0 {
                            ItemAlign::Start
                        } else {
                            it.align
                        };
                        let at_high = match align {
                            ItemAlign::FlexEnd => Some(!cross_flip),
                            ItemAlign::End => Some(!container_rtl),
                            ItemAlign::SelfEnd => Some(!item_rtl),
                            ItemAlign::Start => Some(container_rtl),
                            ItemAlign::SelfStart => Some(item_rtl),
                            ItemAlign::Center | ItemAlign::Baseline => None,
                            _ => Some(cross_flip),
                        };
                        let start_offset = match (at_high, align) {
                            (Some(true), _) => line_free,
                            (Some(false), _) => 0.0,
                            (None, ItemAlign::Baseline) if row => {
                                line.baseline - it.baseline.unwrap_or(line.baseline)
                            }
                            (None, ItemAlign::Baseline) => {
                                if cross_flip {
                                    line_free
                                } else {
                                    0.0
                                }
                            }
                            _ => line_free / 2.0,
                        };
                        start_offset + m
                    }
                };
                let cross_start = line.offset + cross_margin_start;
                let (x, y) = if row {
                    (cb.x + main_start, cb.y + cross_start)
                } else {
                    (cb.x + cross_start, cb.y + main_start)
                };
                let (content_w, height) = if row {
                    (size, Some(it.cross))
                } else {
                    (it.cross, Some(size))
                };
                // Un alto forzado solo si el ítem se estiró o flexionó; si no, su alto natural
                // (para no alterar el `auto` de los porcentajes internos).
                let (lo, hi) = if main_rev { (m1, m0) } else { (m0, m1) };
                let margins = if row {
                    Edges {
                        left: lo,
                        right: hi,
                        top: it.margin(2),
                        bottom: it.margin(3),
                    }
                } else {
                    Edges {
                        top: lo,
                        bottom: hi,
                        left: it.margin(2),
                        right: it.margin(3),
                    }
                };
                let b = it.b;
                let f = self.layout_item(b, cb, (x, y), content_w, height, margins);
                out.push(f);
            }
            // Los ítems colapsados quedan sin tamaño al inicio de su línea.
            for &i in &line.collapsed {
                let it = &items[i];
                let (x, y) = if row {
                    (cb.x, cb.y + line.offset)
                } else {
                    (cb.x + line.offset, cb.y)
                };
                let f = self.layout_item(it.b, cb, (x, y), 0.0, Some(0.0), Edges::default());
                out.push(f);
            }
        }
        let content_h = if row { container_cross } else { main_len };
        // E5: posición estática de los hijos absolutos (CSS Flexbox 1 §4.1): la de un único
        // ítem en un contenedor del tamaño usado, alineado con `justify-content` en el eje
        // principal y `align-self` en el cruzado (CSS Box Alignment 3 §5.1 y §6.1). Se mide la
        // caja con el content box como containing block, como en grid.
        let content_box = Rect::new(cb.x, cb.y, cb.width, content_h);
        let axes = AbsAxes {
            row,
            reverse,
            main_rev,
            cross_flip,
            rtl,
        };
        for (c, is_fixed, idx) in abs_slots {
            let f = self.measuring(|this| {
                let p = PendingAbs {
                    b: c,
                    static_x: cb.x,
                    static_y: cb.y,
                    static_rtl: false,
                };
                this.absolute(p, content_box)
            });
            let m = f.margin_rect();
            let (free_x, free_y) = (content_box.width - m.width, content_box.height - m.height);
            let free = if row {
                (free_x, free_y)
            } else {
                (free_y, free_x)
            };
            let (fx, fy) = abs_static_fractions(c, cs, &axes, free);
            let slot = if is_fixed {
                self.fixed.get_mut(idx)
            } else {
                self.abs_frames.last_mut().and_then(|f| f.get_mut(idx))
            };
            if let Some(p) = slot.filter(|p| std::ptr::eq(p.b, c)) {
                p.static_x = cb.x + fx * free_x;
                p.static_y = cb.y + fy * free_y;
            }
        }
        (out, content_h)
    }

    /// E5: anchos intrínsecos de un contenedor flex en columnas con wrap y alto definido
    /// (§9.9.1): las líneas se cortan con el tamaño principal hipotético de los ítems y cada
    /// una mide lo que su ítem más ancho; `None` si el alto no es definido.
    fn column_wrap_widths(&mut self, b: &'a LayoutBox) -> Option<(f32, f32)> {
        let cs = &b.style;
        let ev = border_edges(cs).vertical() + padding_edges(cs, 0.0).vertical();
        let (lo, hi) = height_limits(cs, self.pct_height_basis, ev);
        let avail = specified_height(cs, self.pct_height_basis, ev)
            .map(|h| h.min(hi).max(lo))
            .or(hi.is_finite().then_some(hi))?;
        let main_gap = gap_px(&cs.row_gap, avail);
        let cross_gap = gap_px(&cs.column_gap, 0.0);
        let mut items: Vec<(f32, f32, f32)> = Vec::new();
        let mut children: Vec<&'a LayoutBox> = b
            .children
            .iter()
            .filter(|c| !c.is_absolutely_positioned() && !matches!(c.kind, BoxKind::Marker { .. }))
            .collect();
        children.sort_by_key(|c| c.style.order.0);
        for c in children {
            let (cmin, cmax) = self.outer_intrinsic_widths(c);
            let s = &c.style;
            let bp_v = border_edges(s).vertical() + padding_edges(s, 0.0).vertical();
            let mv = margin_value(&s.margin_top, 0.0).unwrap_or(0.0)
                + margin_value(&s.margin_bottom, 0.0).unwrap_or(0.0);
            let spec = specified_height(s, Some(avail), bp_v);
            let base = match &s.flex_basis {
                FlexBasis::Size(Size::LengthPercentage(lp)) => {
                    let v = lp.resolve(avail);
                    if s.box_sizing == bf_style::properties::types::BoxSizing::BorderBox {
                        (v - bp_v).max(0.0)
                    } else {
                        v
                    }
                }
                FlexBasis::Size(Size::Auto) if spec.is_some() => spec.unwrap_or(0.0),
                _ => {
                    let bp_h = border_edges(s).horizontal() + padding_edges(s, 0.0).horizontal();
                    let mh = margin_value(&s.margin_left, 0.0).unwrap_or(0.0)
                        + margin_value(&s.margin_right, 0.0).unwrap_or(0.0);
                    let w = (cmax - bp_h - mh).max(0.0);
                    let cb = Cb {
                        x: 0.0,
                        y: 0.0,
                        width: w,
                        height: Some(avail),
                    };
                    self.item_content_height(c, cb, w, bp_v)
                }
            };
            let (hmin, hmax) = height_limits(s, Some(avail), bp_v);
            let hyp = base.min(hmax).max(hmin);
            items.push((hyp + bp_v + mv, cmin, cmax));
        }
        if items.is_empty() {
            return None;
        }
        let (mut min, mut max) = (0.0f32, 0.0f32);
        let (mut used, mut lmin, mut lmax, mut lines) = (0.0f32, 0.0f32, 0.0f32, 0usize);
        let mut first = true;
        for (outer, cmin, cmax) in items {
            let extra = if first { outer } else { outer + main_gap };
            if !first && used + extra > avail + 0.01 {
                min += lmin;
                max += lmax;
                lines += 1;
                used = outer;
                lmin = cmin;
                lmax = cmax;
                continue;
            }
            first = false;
            used += extra;
            lmin = lmin.max(cmin);
            lmax = lmax.max(cmax);
        }
        min += lmin;
        max += lmax;
        lines += 1;
        let gaps = cross_gap * (lines - 1) as f32;
        Some((min + gaps, max + gaps))
    }

    /// Anchos intrínsecos (min-content, max-content) de un contenedor flex (§9.9.1).
    pub(crate) fn flex_intrinsic_widths(&mut self, b: &'a LayoutBox) -> (f32, f32) {
        let cs = &b.style;
        let row = matches!(
            cs.flex_direction,
            FlexDirection::Row | FlexDirection::RowReverse
        );
        let gap = gap_px(&cs.column_gap, 0.0);
        // E5: columnas con wrap y alto definido: las líneas se forman con los tamaños
        // principales y el ancho es la suma de sus anchos (§9.9.1).
        if !row
            && cs.flex_wrap != FlexWrap::Nowrap
            && let Some(r) = self.column_wrap_widths(b)
        {
            return r;
        }
        let (mut min, mut max) = (0.0f32, 0.0f32);
        let mut n = 0;
        // E5: los ítems `visibility: collapse` no ocupan sitio en el eje principal ni llevan
        // hueco (§4.4).
        let collapsed = |c: &LayoutBox| {
            row && c.style.visibility == bf_style::properties::types::Visibility::Collapse
        };
        for c in b
            .children
            .iter()
            .filter(|c| !c.is_absolutely_positioned() && !collapsed(c))
        {
            let (mut cmin, mut cmax) = self.outer_intrinsic_widths(c);
            let content_min = cmin;
            // E5: en filas, la aportación de un ítem que no puede crecer (encoger) no pasa de
            // (no baja de) su flex base size, y luego queda entre su min/max-width (§9.9.1).
            if row {
                let s = &c.style;
                let bp = border_edges(s).horizontal() + padding_edges(s, 0.0).horizontal();
                let mh = crate::block::margin_edges(s, 0.0).horizontal();
                let fixed = |lp: &bf_style::values::LengthPercentage| {
                    (!lp.has_percentage()).then(|| {
                        let v = lp.resolve(0.0);
                        if s.box_sizing == bf_style::properties::types::BoxSizing::BorderBox {
                            (v - bp).max(0.0)
                        } else {
                            v
                        }
                    })
                };
                let outer_base = match (&s.flex_basis, &s.width) {
                    (FlexBasis::Size(Size::LengthPercentage(lp)), _) => {
                        fixed(lp).map(|v| v + bp + mh)
                    }
                    (FlexBasis::Size(Size::Auto), Size::LengthPercentage(lp)) => {
                        fixed(lp).map(|v| v + bp + mh)
                    }
                    (FlexBasis::Size(Size::Auto), Size::Auto) | (FlexBasis::Content, _) => {
                        Some(cmax)
                    }
                    _ => None,
                };
                if let Some(ob) = outer_base {
                    if s.flex_shrink.0 == 0.0 {
                        cmin = cmin.max(ob);
                        cmax = cmax.max(ob);
                    }
                    if s.flex_grow.0 == 0.0 {
                        cmin = cmin.min(ob);
                        cmax = cmax.min(ob);
                    }
                    let (lo, hi) = width_limits(s, 0.0, bp);
                    let pct_min =
                        matches!(&s.min_width, Size::LengthPercentage(lp) if lp.has_percentage());
                    let pct_max = matches!(&s.max_width,
                        bf_style::properties::types::MaxSize::LengthPercentage(lp) if lp.has_percentage());
                    let lo = if pct_min { 0.0 } else { lo + bp + mh };
                    // El mínimo automático (§4.5): la aportación min-content.
                    let lo = if matches!(s.min_width, Size::Auto)
                        && matches!(s.overflow_x, Overflow::Visible | Overflow::Clip)
                    {
                        lo.max(content_min)
                    } else {
                        lo
                    };
                    let hi = if pct_max { f32::INFINITY } else { hi + bp + mh };
                    cmin = cmin.min(hi).max(lo);
                    cmax = cmax.min(hi).max(lo);
                }
            }
            n += 1;
            if row {
                max += cmax;
                if cs.flex_wrap == FlexWrap::Nowrap {
                    min += cmin;
                } else {
                    min = min.max(cmin);
                }
            } else {
                min = min.max(cmin);
                max = max.max(cmax);
            }
        }
        if row && n > 1 {
            let g = gap * (n - 1) as f32;
            max += g;
            if cs.flex_wrap == FlexWrap::Nowrap {
                min += g;
            }
        }
        (min, max)
    }
}
