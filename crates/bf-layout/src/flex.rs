//! CSS Flexible Box Layout Module Level 1, §9 (algoritmo de layout).
//!
//! Solo `writing-mode: horizontal-tb` y `direction: ltr`: el eje principal es horizontal para
//! `row`/`row-reverse` y vertical para `column`/`column-reverse`.
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
    ContentAlign, Display, FlexBasis, FlexDirection, FlexWrap, Gap, ItemAlign,
    LengthPercentageAuto, Overflow, Size,
};

use crate::block::{
    Cb, Layout, border_edges, height_limits, margin_value, padding_edges, specified_height,
    specified_width, width_limits,
};
use crate::box_tree::{BoxKind, LayoutBox};
use crate::float::FloatContext;
use crate::fragment::{Edges, Fragment, FragmentKind};

pub(crate) fn is_flex_container(b: &LayoutBox) -> bool {
    matches!(b.style.display, Display::Flex | Display::InlineFlex)
        && !matches!(b.kind, BoxKind::Replaced { .. })
}

/// Un ítem durante el algoritmo. Tamaños "principal"/"cruzado" del content box salvo que se
/// diga otra cosa.
struct Item<'a> {
    b: &'a LayoutBox,
    /// Márgenes (`None` = `auto`) en orden: main-start, main-end, cross-start, cross-end.
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
    let a = if item.align_self == ItemAlign::Auto {
        container.align_items
    } else {
        item.align_self
    };
    match a {
        ItemAlign::Auto | ItemAlign::Normal => ItemAlign::Stretch,
        a => a,
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
pub(crate) fn container_baseline(f: &Fragment) -> Option<f32> {
    let item = f.children.iter().find(|c| {
        !matches!(
            c.style.position,
            bf_style::properties::types::Position::Absolute
                | bf_style::properties::types::Position::Fixed
        )
    })?;
    Some(first_baseline(item).unwrap_or(item.rect.bottom()))
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

    /// Altura de contenido de un ítem maquetado con ancho de contenido `w`.
    fn item_content_height(&mut self, b: &'a LayoutBox, cb: Cb, w: f32, bp_v: f32) -> f32 {
        self.measuring(|this| {
            let f = this.layout_item(b, cb, (0.0, 0.0), w, None, Edges::default());
            f.rect.height - bp_v
        })
    }

    /// Layout del contenido de un contenedor flex: fragmentos de los ítems y altura del
    /// contenido. `cb` es el content box del contenedor (`height` si es definida).
    pub(crate) fn flex_layout(&mut self, container: &'a LayoutBox, cb: Cb) -> (Vec<Fragment>, f32) {
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
        for c in &container.children {
            if c.is_absolutely_positioned() {
                // §4.1: posición estática como si fuera el único ítem (aproximada: el inicio
                // del content box).
                self.add_abs(c, cb.x, cb.y);
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
            let margins = if row {
                [
                    m(&s.margin_left),
                    m(&s.margin_right),
                    m(&s.margin_top),
                    m(&s.margin_bottom),
                ]
            } else {
                [
                    m(&s.margin_top),
                    m(&s.margin_bottom),
                    m(&s.margin_left),
                    m(&s.margin_right),
                ]
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
                let (wmin, wmax) = width_limits(s, cb.width, bp_h);
                match specified_width(s, cb.width, bp_h) {
                    Some(w) => w,
                    None if align == ItemAlign::Stretch && !auto_cross_margin => available,
                    None => self.item_fit_width(b, available),
                }
                .min(wmax)
                .max(wmin)
            };
            // Tamaño principal especificado (content box).
            let spec_main = if row {
                specified_width(s, cb.width, bp_h)
            } else {
                specified_height(s, cb.height, bp_v)
            };
            // Tamaño de contenido en el eje principal (max-content en filas).
            let content_main = |this: &mut Self| -> f32 {
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
                _ => content_main(self),
            };
            // --- min/max en el eje principal, con el mínimo automático (§4.5) ---
            let (mut min_main, max_main) = if row {
                width_limits(s, cb.width, bp_h)
            } else {
                height_limits(s, cb.height, bp_v)
            };
            let min_auto = if row {
                matches!(s.min_width, Size::Auto)
            } else {
                matches!(s.min_height, Size::Auto)
            };
            let overflow = if row { s.overflow_x } else { s.overflow_y };
            if min_auto && matches!(overflow, Overflow::Visible | Overflow::Clip) {
                let content_min = if row {
                    if let BoxKind::Replaced { natural, ratio } = &b.kind {
                        let rcb = Cb {
                            x: 0.0,
                            y: 0.0,
                            width: cb.width,
                            height: cb.height,
                        };
                        self.replaced_size(b, *natural, *ratio, rcb).width
                    } else {
                        self.intrinsic_widths(b).0
                    }
                } else {
                    self.item_content_height(b, cb, column_cross, bp_v)
                };
                let suggestion = spec_main.map_or(content_min, |sp| sp.min(content_min));
                min_main = suggestion.min(max_main);
            }
            let max_main = max_main.max(min_main);
            let hyp = base.min(max_main).max(min_main);
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
            });
        }

        // --- Líneas (§9.3) ---
        let mut lines: Vec<Line> = Vec::new();
        {
            let mut current: Vec<usize> = Vec::new();
            let mut used = 0.0;
            for (i, item) in items.iter().enumerate() {
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
                    item.baseline = Some(
                        item.margin(2)
                            + first_baseline(&f).map_or(f.rect.height, |b| b - border_top),
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
        // Tamaño cruzado del contenedor y align-content (§9.4 paso 15 y §9.6).
        let lines_cross: f32 = lines.iter().map(|l| l.cross).sum::<f32>()
            + lines.len().saturating_sub(1) as f32 * cross_gap;
        let container_cross = match single_definite_cross {
            Some(c) => c,
            None => lines_cross,
        };
        {
            let free = container_cross - lines_cross;
            let n = lines.len() as f32;
            let (mut start, mut between) = (0.0, 0.0);
            if wrap {
                match cs.align_content {
                    ContentAlign::Normal | ContentAlign::Stretch if free > 0.0 => {
                        let extra = free / n;
                        for l in &mut lines {
                            l.cross += extra;
                        }
                    }
                    ContentAlign::FlexEnd | ContentAlign::End => start = free,
                    ContentAlign::Center => start = free / 2.0,
                    ContentAlign::SpaceBetween if n > 1.0 && free > 0.0 => {
                        between = free / (n - 1.0)
                    }
                    ContentAlign::SpaceAround if free > 0.0 => {
                        between = free / n;
                        start = between / 2.0;
                    }
                    ContentAlign::SpaceAround => start = free / 2.0,
                    ContentAlign::SpaceEvenly if free > 0.0 => {
                        between = free / (n + 1.0);
                        start = between;
                    }
                    ContentAlign::SpaceEvenly => start = free / 2.0,
                    _ => {}
                }
            }
            let mut offset = start;
            for l in &mut lines {
                l.offset = offset;
                offset += l.cross + cross_gap + between;
            }
            if wrap_reverse {
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
            match cs.justify_content {
                ContentAlign::FlexEnd | ContentAlign::End | ContentAlign::Right => start = free,
                ContentAlign::Center => start = free / 2.0,
                ContentAlign::SpaceBetween if n > 1.0 && free > 0.0 => between = free / (n - 1.0),
                ContentAlign::SpaceAround if free > 0.0 => {
                    between = free / n;
                    start = between / 2.0;
                }
                ContentAlign::SpaceAround => start = free / 2.0,
                ContentAlign::SpaceEvenly if free > 0.0 => {
                    between = free / (n + 1.0);
                    start = between;
                }
                ContentAlign::SpaceEvenly => start = free / 2.0,
                _ => {}
            }
            // `flex-end`/`end` en direcciones invertidas: el "start" está al final.
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
                let main_start = if reverse {
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
                    (Some(m), _) => {
                        let flip = wrap_reverse;
                        let start_offset =
                            match it.align {
                                ItemAlign::FlexEnd | ItemAlign::End | ItemAlign::SelfEnd => {
                                    if flip { 0.0 } else { line_free }
                                }
                                ItemAlign::FlexStart | ItemAlign::Start | ItemAlign::SelfStart => {
                                    if flip {
                                        line_free
                                    } else {
                                        0.0
                                    }
                                }
                                ItemAlign::Center => line_free / 2.0,
                                ItemAlign::Baseline if row => {
                                    line.baseline - it.baseline.unwrap_or(line.baseline)
                                }
                                _ => {
                                    if flip {
                                        line_free
                                    } else {
                                        0.0
                                    }
                                }
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
                let margins = if row {
                    Edges {
                        left: m0,
                        right: m1,
                        top: it.margin(2),
                        bottom: it.margin(3),
                    }
                } else {
                    Edges {
                        top: m0,
                        bottom: m1,
                        left: it.margin(2),
                        right: it.margin(3),
                    }
                };
                let b = it.b;
                let f = self.layout_item(b, cb, (x, y), content_w, height, margins);
                out.push(f);
            }
        }
        let content_h = if row { container_cross } else { main_len };
        (out, content_h)
    }

    /// Anchos intrínsecos (min-content, max-content) de un contenedor flex (§9.9.1).
    pub(crate) fn flex_intrinsic_widths(&mut self, b: &'a LayoutBox) -> (f32, f32) {
        let cs = &b.style;
        let row = matches!(
            cs.flex_direction,
            FlexDirection::Row | FlexDirection::RowReverse
        );
        let gap = gap_px(&cs.column_gap, 0.0);
        let (mut min, mut max) = (0.0f32, 0.0f32);
        let mut n = 0;
        for c in b.children.iter().filter(|c| !c.is_absolutely_positioned()) {
            let (cmin, cmax) = self.outer_intrinsic_widths(c);
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
