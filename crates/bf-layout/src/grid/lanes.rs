//! E5: grid lanes (CSS Grid Layout Module Level 3, antes "masonry"): `display: grid-lanes` e
//! `inline-grid-lanes` (§2.2).
//!
//! Un contenedor grid lanes tiene un eje de rejilla (con pistas, `grid-template-*`) y un eje de
//! apilado sin pistas (§2.3). Los ítems se colocan uno a uno, en orden modificado por `order`
//! (§4.4), en las pistas cuyas posiciones de apilado ("running positions") son menores, con la
//! tolerancia de `flow-tolerance` (§4.2) y el cursor de auto-colocación para desempatar.
//!
//! Interpretaciones:
//! - Eje de rejilla (§2.3): `grid-lanes-direction: row` lo pone en las filas y `column` en las
//!   columnas; con `normal`, son las filas si `grid-template-columns` es `none` y
//!   `grid-template-rows` no, y las columnas en otro caso.
//! - Dimensionado de las pistas (§3.4): los ítems auto-colocados contribuyen a todas las pistas
//!   en las que podrían caer (una contribución por cada línea de inicio posible); los colocados
//!   con línea definida, solo a las suyas. `repeat(auto-fit)` colapsa las pistas que quedan
//!   vacías en una colocación hipotética en orden estricto (como con tolerancia infinita).
//! - Eje de apilado: el containing block del ítem es el content box del contenedor (§4.4.1);
//!   la alineación propia `normal`/`stretch` es `start` (§6.4), así que con columnas como eje de
//!   rejilla el alto es el del contenido y con filas el ancho es fit-content. Los márgenes
//!   `auto` del eje de apilado son 0.
//! - El gap del eje de apilado va antes de cada ítem salvo el primero de cada pista (§6.1).
//! - `grid-lanes-direction: ... fill-reverse` apila desde el final del eje de apilado y
//!   `track-reverse` recorre las pistas desde la última (para elegir y desempatar).
//! - `align-content`/`justify-content` en el eje de apilado mueven el rango de apilado entero
//!   (§6.3); `normal`/`stretch` y las distribuciones son `start` del flujo.
//! - `grid-lanes-pack: dense` (y `grid-auto-flow: dense`) rellena huecos anteriores (§4.3): un
//!   ítem auto-colocado puede ir antes del final de una pista si cabe en un hueco que dejó un
//!   ítem que abarca varias pistas, entre la posición de apilado de pistas con el mismo tamaño
//!   total.
//! - Tamaño intrínseco en el eje de apilado (§5): el del rango de apilado, con los ítems a su
//!   tamaño min-content o max-content.

use super::*;
use crate::block::specified_height;
use bf_style::properties::types::{FlowTolerance, GridLanesPack, LanesAxis};

/// ¿Es `b` un contenedor grid lanes?
pub(crate) fn is_lanes_container(b: &LayoutBox) -> bool {
    matches!(
        b.style.display,
        Display::GridLanes | Display::InlineGridLanes
    )
}

/// ¿Son las filas el eje de rejilla (§2.3)?
fn rows_are_lanes(s: &ComputedStyle) -> bool {
    match s.grid_lanes_direction.axis {
        LanesAxis::Row => true,
        LanesAxis::Column => false,
        LanesAxis::Normal => {
            matches!(s.grid_template_columns, GridTemplate::None)
                && !matches!(s.grid_template_rows, GridTemplate::None)
        }
    }
}

/// Tolerancia de colocación (§4.2): `normal` es 1em; los porcentajes, contra el tamaño del
/// content box en el eje de rejilla.
fn tolerance(s: &ComputedStyle, grid_axis_size: f32) -> f32 {
    match &s.flow_tolerance {
        FlowTolerance::Normal => s.font_size_px(),
        FlowTolerance::LengthPercentage(lp) => lp.resolve(grid_axis_size).max(0.0),
        FlowTolerance::Infinite => f32::INFINITY,
    }
}

fn dense(s: &ComputedStyle) -> bool {
    s.grid_lanes_pack == GridLanesPack::Dense || s.grid_auto_flow.dense
}

/// Un hueco del eje de apilado que dejó un ítem que abarca varias pistas (§4.3): en la pista
/// `track`, entre `start` y `end`.
#[derive(Clone, Copy, Debug)]
struct Hole {
    track: usize,
    start: f32,
    end: f32,
}

/// Posiciones de apilado de las pistas y cursor de auto-colocación (§4.4).
struct Running {
    pos: Vec<f32>,
    used: Vec<bool>,
    /// Cursor, en orden de recorrido (con `track-reverse`, desde la última pista).
    cursor: usize,
    tol: f32,
    reverse: bool,
    gap: f32,
    dense: bool,
    holes: Vec<Hole>,
}

impl Running {
    fn new(n: usize, tol: f32, gap: f32, reverse: bool, dense: bool) -> Running {
        Running {
            pos: vec![0.0; n],
            used: vec![false; n],
            cursor: 0,
            tol,
            reverse,
            gap,
            dense,
            holes: Vec::new(),
        }
    }

    /// Línea física de inicio de la `v`-ésima posición posible (en orden de recorrido) de un
    /// ítem de `span` pistas.
    fn physical(&self, v: usize, span: usize) -> usize {
        if self.reverse {
            self.pos.len() - span - v
        } else {
            v
        }
    }

    fn max_pos(&self, s: usize, e: usize) -> f32 {
        self.pos[s..e].iter().copied().fold(0.0, f32::max)
    }

    /// Línea de inicio (física) de un ítem auto-colocado de `span` pistas (§4.4 paso 1): la
    /// primera, desde el cursor, de las que quedan a `tol` de la menor posición de apilado.
    fn choose(&self, span: usize) -> usize {
        let n = self.pos.len();
        let span = span.clamp(1, n);
        let count = n - span + 1;
        let maxes: Vec<f32> = (0..count)
            .map(|v| {
                let s = self.physical(v, span);
                self.max_pos(s, s + span)
            })
            .collect();
        let min = maxes.iter().copied().fold(f32::INFINITY, f32::min);
        let tied = |v: usize| maxes[v] - min <= self.tol + 0.01;
        let v = (self.cursor..count)
            .find(|&v| tied(v))
            .or_else(|| (0..count).find(|&v| tied(v)))
            .unwrap_or(0);
        self.physical(v, span)
    }

    /// Hueco anterior donde cabe un ítem de `span` pistas y tamaño `outer` (§4.3): el más alto.
    fn find_hole(&self, span: usize, outer: f32) -> Option<(usize, f32)> {
        if !self.dense || span != 1 {
            return None;
        }
        self.holes
            .iter()
            .filter(|h| h.end - h.start >= outer.max(0.0) + self.gap - 0.01)
            .min_by(|a, b| {
                a.start
                    .partial_cmp(&b.start)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.track.cmp(&b.track))
            })
            .map(|h| (h.track, h.start))
    }

    /// Ocupa un hueco con un ítem de tamaño `outer` colocado en su inicio.
    fn fill_hole(&mut self, track: usize, start: f32, outer: f32) {
        if let Some(i) = self
            .holes
            .iter()
            .position(|h| h.track == track && (h.start - start).abs() < 0.01)
        {
            let h = self.holes[i];
            let next = start + outer.max(0.0) + self.gap;
            if h.end - next > 0.01 {
                self.holes[i].start = next;
            } else {
                self.holes.remove(i);
            }
        }
    }

    /// Coloca un ítem en `[s, e)` con tamaño exterior `outer` (§4.4 pasos 2 y 3) y devuelve su
    /// posición de apilado.
    /// Solo los auto-colocados mueven el cursor (a su última línea).
    fn commit(&mut self, s: usize, e: usize, outer: f32, auto: bool) -> f32 {
        let p = self.max_pos(s, e);
        if self.dense && e - s > 1 {
            for t in s..e {
                // El hueco cuenta desde donde iría el siguiente ítem de la pista.
                if p - self.pos[t] > 0.01 {
                    self.holes.push(Hole {
                        track: t,
                        start: self.pos[t],
                        end: p,
                    });
                }
            }
        }
        let end = p + outer.max(0.0) + self.gap;
        for t in s..e {
            self.pos[t] = self.pos[t].max(end);
            self.used[t] = true;
        }
        if auto {
            let n = self.pos.len();
            self.cursor = if self.reverse { n - s } else { e };
        }
        p
    }

    /// Final del rango de apilado (§5): el mayor final de las pistas con ítems.
    fn range_end(&self) -> f32 {
        self.pos
            .iter()
            .zip(&self.used)
            .filter(|(_, u)| **u)
            .map(|(p, _)| p - self.gap)
            .fold(0.0, f32::max)
    }
}

/// Rejilla del eje de rejilla de un contenedor grid lanes.
struct LanesSetup<'a> {
    axis: ExplicitAxis,
    before: usize,
    tracks: Vec<Track>,
    /// Ítems en flujo, en orden modificado por `order`, con su colocación en el eje de rejilla
    /// (ya desplazada por `before`).
    items: Vec<(&'a LayoutBox, Placement)>,
    abs: Vec<&'a LayoutBox>,
}

impl LanesSetup<'_> {
    /// Span (en pistas) de un ítem auto-colocado.
    fn auto_span(&self, span: i32) -> usize {
        (span.max(1) as usize).min(self.tracks.len().max(1))
    }

    /// Ítems para el dimensionado de las pistas (§3.4): uno por posición posible de cada
    /// auto-colocado. Devuelve el índice del ítem y el span `[inicio, fin)`.
    fn sizing_spans(&self) -> Vec<(usize, (usize, usize))> {
        let n = self.tracks.len();
        let mut out = Vec::new();
        for (i, (_, p)) in self.items.iter().enumerate() {
            match *p {
                Placement::Definite(a, b) => out.push((i, (a as usize, b as usize))),
                Placement::Auto(span) => {
                    let span = self.auto_span(span);
                    for s in 0..=n - span {
                        out.push((i, (s, s + span)));
                    }
                }
            }
        }
        out
    }
}

/// Vacío para el eje sin pistas de la rejilla falsa con la que se reutiliza el dimensionado.
fn empty_axis() -> ExplicitAxis {
    explicit_axis(
        &GridTemplate::None,
        &GridTemplateAreas::None,
        false,
        RepeatSpace {
            size: None,
            min: None,
            basis: None,
        },
        0.0,
    )
}

/// Desplazamiento del rango de apilado (de tamaño `range`) en un content box de tamaño `size`
/// por `align-content`/`justify-content` (§6.3); `reverse` = `fill-reverse` (el flujo empieza
/// al final).
fn stacking_offset(a: ContentAlign, size: f32, range: f32, reverse: bool) -> f32 {
    let free = size - range;
    let flow_start = if reverse { free } else { 0.0 };
    match a {
        ContentAlign::Start | ContentAlign::FlexStart | ContentAlign::Left => 0.0,
        ContentAlign::End | ContentAlign::FlexEnd | ContentAlign::Right => free,
        ContentAlign::Center => free / 2.0,
        _ => flow_start,
    }
}

impl<'a> Layout<'a> {
    /// Pistas del eje de rejilla y colocación definida de los ítems (§3 y §4.1).
    fn lanes_setup(
        &mut self,
        container: &'a LayoutBox,
        rows: bool,
        width: Option<f32>,
        height: Option<f32>,
    ) -> LanesSetup<'a> {
        let cs = &container.style;
        let bp_v = border_edges(cs).vertical() + padding_edges(cs, width.unwrap_or(0.0)).vertical();
        let (template, space, gap, size) = if rows {
            (
                &cs.grid_template_rows,
                row_repeat_space(cs, height, bp_v),
                gap_px(&cs.row_gap, height),
                height,
            )
        } else {
            (
                &cs.grid_template_columns,
                column_repeat_space(container, width),
                gap_px(&cs.column_gap, width),
                width,
            )
        };
        // `subgrid` no aplica a un contenedor grid lanes que no es ítem de un grid: se trata
        // como `none`.
        let template = match template {
            GridTemplate::Subgrid(_) => &GridTemplate::None,
            t => t,
        };
        let axis = explicit_axis(template, &cs.grid_template_areas, !rows, space, gap);
        let mut children: Vec<&'a LayoutBox> = Vec::new();
        let mut abs = Vec::new();
        for c in &container.children {
            if c.is_absolutely_positioned() {
                abs.push(c);
            } else if !matches!(c.kind, BoxKind::Marker { .. })
                && !self.legend.is_some_and(|l| std::ptr::eq(l, c))
            {
                children.push(c);
            }
        }
        children.sort_by_key(|c| c.style.order.0);
        let placements: Vec<Placement> = children
            .iter()
            .map(|c| {
                let s = &c.style;
                if rows {
                    resolve_placement(&s.grid_row_start, &s.grid_row_end, &axis)
                } else {
                    resolve_placement(&s.grid_column_start, &s.grid_column_end, &axis)
                }
            })
            .collect();
        let before = placements
            .iter()
            .map(|p| match p {
                Placement::Definite(a, _) => (-a).max(0),
                Placement::Auto(_) => 0,
            })
            .max()
            .unwrap_or(0) as usize;
        let placements: Vec<Placement> = placements
            .into_iter()
            .map(|p| match p {
                Placement::Definite(a, b) => {
                    Placement::Definite(a + before as i32, b + before as i32)
                }
                p => p,
            })
            .collect();
        let mut n = before + axis.count;
        for p in &placements {
            n = n.max(match *p {
                Placement::Definite(_, b) => b as usize,
                Placement::Auto(span) => span.max(1) as usize,
            });
        }
        if !children.is_empty() {
            n = n.max(1);
        }
        let auto = if rows {
            &cs.grid_auto_rows.0
        } else {
            &cs.grid_auto_columns.0
        };
        let mut tracks = build_tracks(&axis, auto, before, n, size);
        let items: Vec<(&'a LayoutBox, Placement)> = children.into_iter().zip(placements).collect();
        // §3.3 (`repeat(auto-fit)`): se colapsan las pistas vacías de una colocación hipotética
        // en orden estricto.
        if let Some((s, e)) = axis.auto_fit {
            let mut used = vec![false; n];
            let mut cursor = 0usize;
            for (_, p) in &items {
                let (a, b) = match *p {
                    Placement::Definite(a, b) => (a as usize, b as usize),
                    Placement::Auto(span) => {
                        let span = (span.max(1) as usize).min(n);
                        if cursor + span > n {
                            cursor = 0;
                        }
                        let r = (cursor, cursor + span);
                        cursor += span;
                        r
                    }
                };
                for u in used.iter_mut().take(b.min(n)).skip(a) {
                    *u = true;
                }
            }
            for (i, t) in tracks
                .iter_mut()
                .enumerate()
                .take(before + e)
                .skip(before + s)
            {
                t.collapsed = !used[i];
            }
        }
        LanesSetup {
            axis,
            before,
            tracks,
            items,
            abs,
        }
    }

    /// Rejilla falsa (las pistas de lanes como columnas y una sola fila) para reutilizar el
    /// dimensionado de las columnas de grid.
    fn lanes_fake_grid(&self, setup: &LanesSetup<'a>) -> Grid<'a> {
        let items = setup
            .sizing_spans()
            .into_iter()
            .map(|(i, col)| GridItem {
                b: setup.items[i].0,
                col,
                row: (0, 1),
                extra: [0.0; 4],
                area_w: None,
                empty: false,
            })
            .collect();
        Grid {
            items,
            cols: setup.tracks.clone(),
            rows: vec![Track::new(&TrackSize::AUTO, None)],
            abs: Vec::new(),
            col_axis: explicit_axis(
                &GridTemplate::None,
                &GridTemplateAreas::None,
                true,
                RepeatSpace {
                    size: None,
                    min: None,
                    basis: None,
                },
                0.0,
            ),
            row_axis: empty_axis(),
            col_before: setup.before,
            row_before: 0,
            sub: SubgridParent::default(),
        }
    }

    /// Ancho de contenido y márgenes de un ítem en el eje de apilado inline (filas como eje de
    /// rejilla): fit-content en el content box del contenedor (§4.4.1 y §6.4).
    fn lanes_stack_width(&mut self, b: &'a LayoutBox, cb_w: f32) -> (f32, f32, f32) {
        let s = &b.style;
        let bp = border_edges(s).horizontal() + padding_edges(s, cb_w).horizontal();
        let ml = margin_value(&s.margin_left, cb_w).unwrap_or(0.0);
        let mr = margin_value(&s.margin_right, cb_w).unwrap_or(0.0);
        let avail = (cb_w - ml - mr - bp).max(0.0);
        let (lo, hi) = self.width_limits_for(b, cb_w, bp, avail);
        let w = if let Some(w) = self.table_fixed_width(b, avail, cb_w) {
            w
        } else if let Some(w) = specified_width(s, cb_w, bp) {
            w
        } else if let Some(w) = self.keyword_width(b, avail, cb_w) {
            w
        } else if let BoxKind::Replaced { natural, ratio } = &b.kind {
            let cb = Cb {
                x: 0.0,
                y: 0.0,
                width: cb_w,
                height: None,
            };
            return (self.replaced_size(b, *natural, *ratio, cb).width, ml, mr);
        } else if let Some(w) = self.control_size(b).0 {
            w
        } else {
            let (min, max) = self.intrinsic_widths(b);
            min.max(avail).min(max)
        };
        (w.min(hi).max(lo), ml, mr)
    }

    /// Layout de un contenedor grid lanes: fragmentos de los ítems y altura del contenido.
    pub(super) fn lanes_layout(&mut self, container: &'a LayoutBox, cb: Cb) -> (Vec<Fragment>, f32) {
        let cs = &container.style;
        let rows = rows_are_lanes(cs);
        let setup = self.lanes_setup(container, rows, Some(cb.width), cb.height);
        let rtl = cs.direction == Direction::Rtl;
        let bp_v = border_edges(cs).vertical() + padding_edges(cs, cb.width).vertical();
        let (hmin, hmax) = height_limits(cs, None, bp_v);
        let fill_reverse = cs.grid_lanes_direction.fill_reverse;
        let track_reverse = cs.grid_lanes_direction.track_reverse;
        let mut out: Vec<Fragment> = Vec::with_capacity(setup.items.len() + setup.abs.len());
        // (posición de apilado, pista, índice) del primer ítem, para la baseline.
        let mut first: Option<(f32, usize, usize)> = None;
        let content_h;
        // Líneas del eje de rejilla (inicio y fin de cada pista) para los absolutos.
        let lines: (Vec<f32>, Vec<f32>);
        if !rows {
            // --- Columnas como eje de rejilla ---
            let col_gap = gap_px(&cs.column_gap, Some(cb.width));
            let row_gap = gap_px(&cs.row_gap, cb.height);
            let mut grid = self.lanes_fake_grid(&setup);
            let (starts, ends) = self.size_grid_columns(container, &mut grid, cb.width, col_gap);
            let n = setup.tracks.len();
            let mut run = Running::new(
                n,
                tolerance(cs, cb.width),
                row_gap,
                track_reverse,
                dense(cs),
            );
            // Posición de apilado y tamaño exterior de cada fragmento.
            let mut stack: Vec<(f32, f32)> = Vec::new();
            for (i, (b, p)) in setup.items.iter().enumerate() {
                let b = *b;
                let s = &b.style;
                let (c0, c1, auto) = match *p {
                    Placement::Definite(a, e) => (a as usize, e as usize, false),
                    Placement::Auto(span) => {
                        let span = setup.auto_span(span);
                        let c = run.choose(span);
                        (c, c + span, true)
                    }
                };
                let aw = (line_pos(&starts, &ends, c1, false) - line_pos(&starts, &ends, c0, true))
                    .max(0.0);
                let (w, ml, mr) = self.grid_item_width(b, cs, aw, None);
                let bp_h = border_edges(s).horizontal() + padding_edges(s, aw).horizontal();
                let justify = physical(
                    self_align(s.justify_self, cs.justify_items, has_ratio(b)),
                    rtl,
                );
                let free_h = aw - w - bp_h - ml.unwrap_or(0.0) - mr.unwrap_or(0.0);
                let both = ml.is_some() && mr.is_some();
                let (dx, ml, mr) = place_in_area(free_h, ml, mr, justify);
                let dx = if rtl && both { free_h - dx } else { dx };
                let mt = margin_value(&s.margin_top, aw).unwrap_or(0.0);
                let mb = margin_value(&s.margin_bottom, aw).unwrap_or(0.0);
                let margins = Edges {
                    left: ml,
                    right: mr,
                    top: mt,
                    bottom: mb,
                };
                // Se maqueta primero al inicio del eje de apilado para conocer su tamaño.
                let x0 = line_pos(&starts, &ends, c0, true);
                let x0 = if rtl { cb.width - x0 - aw } else { x0 };
                let item_cb = Cb {
                    x: cb.x + x0,
                    y: cb.y,
                    width: aw,
                    height: cb.height,
                };
                let x = cb.x + x0 + dx + ml;
                let mut f = self.grid_item_fragment(b, item_cb, (x, cb.y + mt), w, None, margins);
                if f.kind == FragmentKind::Block {
                    f.kind = FragmentKind::InlineBlock;
                }
                let outer = f.rect.height + mt + mb;
                let y = match run.find_hole(c1 - c0, outer).filter(|_| auto) {
                    Some((t, start)) => {
                        // Hueco anterior (§4.3): el ítem va en esa pista.
                        run.fill_hole(t, start, outer);
                        let x1 = line_pos(&starts, &ends, t, true);
                        let x1 = if rtl {
                            cb.width - x1 - (ends[t] - starts[t])
                        } else {
                            x1
                        };
                        f.translate(x1 - x0, 0.0);
                        start
                    }
                    None => run.commit(c0, c1, outer, auto),
                };
                f.translate(0.0, y);
                let key = (y, c0, i);
                if first.is_none_or(|k| key.0 < k.0 || (key.0 == k.0 && key.1 < k.1)) {
                    first = Some(key);
                }
                stack.push((y, outer));
                out.push(f);
            }
            let range = run.range_end();
            content_h = cb.height.unwrap_or_else(|| range.min(hmax).max(hmin));
            let off = stacking_offset(cs.align_content, content_h, range, fill_reverse);
            for (f, (y, outer)) in out.iter_mut().zip(&stack) {
                // `fill-reverse`: se refleja en el rango de apilado.
                let dy = if fill_reverse {
                    range - y - outer - y
                } else {
                    0.0
                };
                f.translate(0.0, dy + off);
            }
            lines = (starts, ends);
        } else {
            // --- Filas como eje de rejilla ---
            let col_gap = gap_px(&cs.column_gap, Some(cb.width));
            let row_gap = gap_px(&cs.row_gap, cb.height);
            let mut tracks = setup.tracks.clone();
            // Ancho (fit-content) de cada ítem y contribuciones a las filas.
            let widths: Vec<(f32, f32, f32)> = setup
                .items
                .iter()
                .map(|(b, _)| self.lanes_stack_width(b, cb.width))
                .collect();
            let mut contribs = Vec::new();
            for (i, span) in setup.sizing_spans() {
                let (b, _) = setup.items[i];
                let (w, ml, mr) = widths[i];
                let bp_h = border_edges(&b.style).horizontal()
                    + padding_edges(&b.style, cb.width).horizontal();
                let it = GridItem {
                    b,
                    col: (0, 1),
                    row: span,
                    extra: [0.0; 4],
                    area_w: None,
                    empty: false,
                };
                let (c, _) = self.row_contrib(&it, cs, &tracks, row_gap, w + bp_h + ml + mr);
                contribs.push(c);
            }
            let stretch = matches!(cs.align_content, ContentAlign::Normal | ContentAlign::Stretch);
            let (space, limits) = match cb.height {
                Some(h) => (Space::Definite(h), (h, h)),
                None => (Space::MaxContent, (hmin, hmax)),
            };
            size_tracks(&mut tracks, &contribs, row_gap, space, limits, stretch);
            let total = total_size(&tracks, row_gap);
            content_h = cb.height.unwrap_or_else(|| total.min(hmax).max(hmin));
            let rerun = rows_have_percentages(cs) || tracks.iter().any(|t| t.flex().is_some());
            let row_gap = if cb.height.is_none() && rerun {
                let collapsed: Vec<bool> = tracks.iter().map(|t| t.collapsed).collect();
                tracks = build_tracks(
                    &setup.axis,
                    &cs.grid_auto_rows.0,
                    setup.before,
                    tracks.len(),
                    Some(content_h),
                );
                for (t, c) in tracks.iter_mut().zip(collapsed) {
                    t.collapsed = c;
                }
                let row_gap = gap_px(&cs.row_gap, Some(content_h));
                size_tracks(
                    &mut tracks,
                    &contribs,
                    row_gap,
                    Space::Definite(content_h),
                    (content_h, content_h),
                    stretch,
                );
                row_gap
            } else {
                row_gap
            };
            let (starts, ends) = track_positions(&tracks, row_gap, content_h, cs.align_content);
            let n = tracks.len();
            let mut run = Running::new(
                n,
                tolerance(cs, content_h),
                col_gap,
                track_reverse,
                dense(cs),
            );
            let mut stack: Vec<(f32, f32)> = Vec::new();
            for (i, (b, p)) in setup.items.iter().enumerate() {
                let b = *b;
                let s = &b.style;
                let (r0, r1, auto) = match *p {
                    Placement::Definite(a, e) => (a as usize, e as usize, false),
                    Placement::Auto(span) => {
                        let span = setup.auto_span(span);
                        let r = run.choose(span);
                        (r, r + span, true)
                    }
                };
                let (w, ml, mr) = widths[i];
                let bp_h = border_edges(s).horizontal() + padding_edges(s, cb.width).horizontal();
                let bp_v = border_edges(s).vertical() + padding_edges(s, cb.width).vertical();
                let outer_w = w + bp_h + ml + mr;
                let place = |run: &mut Running, r0: usize, r1: usize| -> (usize, f32) {
                    match run.find_hole(r1 - r0, outer_w).filter(|_| auto) {
                        Some((t, start)) => {
                            run.fill_hole(t, start, outer_w);
                            (t, start)
                        }
                        None => (r0, run.commit(r0, r1, outer_w, auto)),
                    }
                };
                let span = r1 - r0;
                let (r0, xpos) = place(&mut run, r0, r1);
                let r1 = r0 + span;
                let y0 = line_pos(&starts, &ends, r0, true);
                let ah = (line_pos(&starts, &ends, r1, false) - y0).max(0.0);
                let mt = margin_value(&s.margin_top, cb.width);
                let mb = margin_value(&s.margin_bottom, cb.width);
                let align = self_align(
                    block_axis(s.align_self),
                    block_axis(cs.align_items),
                    has_ratio(b),
                );
                let forced = if align == ItemAlign::Stretch
                    && matches!(s.height, Size::Auto)
                    && mt.is_some()
                    && mb.is_some()
                {
                    let (lo, hi) = height_limits(s, Some(ah), bp_v);
                    Some(
                        (ah - mt.unwrap_or(0.0) - mb.unwrap_or(0.0) - bp_v)
                            .min(hi)
                            .max(lo)
                            .max(0.0),
                    )
                } else {
                    None
                };
                let margins = Edges {
                    left: ml,
                    right: mr,
                    top: mt.unwrap_or(0.0),
                    bottom: mb.unwrap_or(0.0),
                };
                let item_cb = Cb {
                    x: cb.x,
                    y: cb.y + y0,
                    width: cb.width,
                    height: Some(ah),
                };
                let x = cb.x + xpos + ml;
                let y = cb.y + y0 + mt.unwrap_or(0.0);
                let mut f = self.grid_item_fragment(b, item_cb, (x, y), w, forced, margins);
                if f.kind == FragmentKind::Block {
                    f.kind = FragmentKind::InlineBlock;
                }
                let free_v = ah - f.rect.height - mt.unwrap_or(0.0) - mb.unwrap_or(0.0);
                let (dy, mt_used, mb_used) = place_in_area(free_v, mt, mb, align);
                let shift = dy + mt_used - mt.unwrap_or(0.0);
                if shift != 0.0 {
                    f.translate(0.0, shift);
                }
                f.margin.top = mt_used;
                f.margin.bottom = mb_used;
                let key = (xpos, r0, i);
                if first.is_none_or(|k| key.1 < k.1 || (key.1 == k.1 && key.0 < k.0)) {
                    first = Some(key);
                }
                stack.push((xpos, outer_w));
                out.push(f);
            }
            let range = run.range_end();
            let off = stacking_offset(cs.justify_content, cb.width, range, fill_reverse);
            for (f, (x, outer)) in out.iter_mut().zip(&stack) {
                // En rtl el flujo va de derecha a izquierda; `fill-reverse` lo invierte.
                let mirrored = rtl != fill_reverse;
                let dx = if mirrored { range - x - outer - x } else { 0.0 };
                let off = if rtl {
                    cb.width - range - off
                } else {
                    off
                };
                f.translate(dx + off, 0.0);
            }
            lines = (starts, ends);
        }
        let baseline = first.map(|(_, _, i)| {
            let f = &out[i];
            crate::flex::first_baseline(f).unwrap_or(f.rect.bottom())
        });
        self.lanes_abs(container, cb, content_h, &setup, rows, &lines, &mut out);
        self.grid_baseline = baseline;
        (out, content_h)
    }

    /// Elementos absolutos de un contenedor grid lanes (como §9 de Grid 2): las líneas del eje
    /// de rejilla; en el eje de apilado, el padding box.
    #[allow(clippy::too_many_arguments)]
    fn lanes_abs(
        &mut self,
        container: &'a LayoutBox,
        cb: Cb,
        content_h: f32,
        setup: &LanesSetup<'a>,
        rows: bool,
        (starts, ends): &(Vec<f32>, Vec<f32>),
        out: &mut Vec<Fragment>,
    ) {
        let cs = &container.style;
        let rtl = cs.direction == Direction::Rtl;
        let positioned = is_positioned(cs);
        let pad = padding_edges(cs, cb.width);
        let content_box = Rect::new(cb.x, cb.y, cb.width, content_h);
        let padding_box = Rect::new(
            cb.x - pad.left,
            cb.y - pad.top,
            cb.width + pad.horizontal(),
            content_h + pad.vertical(),
        );
        let n = setup.tracks.len();
        let area = |s: &ComputedStyle| {
            let (l0, l1) = if rows {
                resolve_abs_placement(&s.grid_row_start, &s.grid_row_end, &setup.axis)
            } else {
                resolve_abs_placement(&s.grid_column_start, &s.grid_column_end, &setup.axis)
            };
            let to_index = |l: Option<i32>| {
                l.map(|l| l + setup.before as i32)
                    .filter(|&l| l >= 0 && l as usize <= n)
                    .map(|l| l as usize)
            };
            if rows {
                let top = to_index(l0)
                    .map_or(padding_box.y, |l| cb.y + line_pos(starts, ends, l, true));
                let bottom = to_index(l1)
                    .map_or(padding_box.bottom(), |l| cb.y + line_pos(starts, ends, l, false));
                Rect::new(
                    padding_box.x,
                    top,
                    padding_box.width,
                    (bottom - top).max(0.0),
                )
            } else {
                let (pad_s, pad_e) = if rtl {
                    (pad.right, pad.left)
                } else {
                    (pad.left, pad.right)
                };
                let u0 = to_index(l0).map_or(-pad_s, |l| line_pos(starts, ends, l, true));
                let u1 =
                    to_index(l1).map_or(cb.width + pad_e, |l| line_pos(starts, ends, l, false));
                let (left, right) = if rtl {
                    (cb.x + cb.width - u1, cb.x + cb.width - u0)
                } else {
                    (cb.x + u0, cb.x + u1)
                };
                Rect::new(
                    left,
                    padding_box.y,
                    (right - left).max(0.0),
                    padding_box.height,
                )
            }
        };
        let descendants = if positioned {
            self.abs_frames
                .last_mut()
                .map(std::mem::take)
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        for p in descendants {
            let a = area(&p.b.style);
            let f = self.absolute(p, a);
            out.push(f);
        }
        for &c in &setup.abs {
            let s = &c.style;
            if !positioned || s.position == Position::Fixed {
                let f = self.measuring(|this| {
                    let p = PendingAbs {
                        b: c,
                        static_x: content_box.x,
                        static_y: content_box.y,
                    };
                    this.absolute(p, content_box)
                });
                let (dx, dy) = abs_align_offset(c, cs, &f, content_box);
                let dx = if rtl {
                    content_box.width - f.margin_rect().width - dx
                } else {
                    dx
                };
                self.add_abs(c, content_box.x + dx, content_box.y + dy);
                continue;
            }
            let a = area(s);
            let mut f = self.absolute(
                PendingAbs {
                    b: c,
                    static_x: a.x,
                    static_y: a.y,
                },
                a,
            );
            let (dx, dy) = abs_align_offset(c, cs, &f, a);
            let dx = if rtl && abs_auto_inline(s) {
                a.width - f.margin_rect().width - dx
            } else {
                dx
            };
            f.translate(dx, dy);
            out.push(f);
        }
    }

    /// Anchos intrínsecos de un contenedor grid lanes: con columnas como eje de rejilla, el
    /// dimensionado de las pistas bajo restricción min-/max-content (§3.4); con filas, el rango
    /// de apilado con los ítems a su tamaño min-/max-content (§5).
    pub(super) fn lanes_intrinsic_widths(&mut self, b: &'a LayoutBox) -> (f32, f32) {
        let cs = &b.style;
        let rows = rows_are_lanes(cs);
        let bp_v = border_edges(cs).vertical() + padding_edges(cs, 0.0).vertical();
        let height = specified_height(cs, self.pct_height_basis, bp_v);
        let setup = self.lanes_setup(b, rows, None, height);
        if !rows {
            let gap = gap_px(&cs.column_gap, None);
            let tracks = setup.tracks.clone();
            let grid = self.lanes_fake_grid(&setup);
            let contribs: Vec<Contrib> = grid
                .items
                .iter()
                .map(|it| self.column_contrib(it, &tracks, gap, None))
                .collect();
            let mut sizes = [0.0f32; 2];
            for (k, space) in [Space::MinContent, Space::MaxContent]
                .into_iter()
                .enumerate()
            {
                let mut t = tracks.clone();
                size_tracks(&mut t, &contribs, gap, space, (0.0, f32::INFINITY), false);
                sizes[k] = total_size(&t, gap);
            }
            return (sizes[0], sizes[1].max(sizes[0]));
        }
        let gap = gap_px(&cs.column_gap, None);
        let tol = tolerance(cs, height.unwrap_or(0.0));
        let widths: Vec<(f32, f32)> = setup
            .items
            .iter()
            .map(|(c, _)| self.outer_intrinsic_widths(c))
            .collect();
        let mut sizes = [0.0f32; 2];
        for (k, size) in sizes.iter_mut().enumerate() {
            let mut run = Running::new(
                setup.tracks.len(),
                tol,
                gap,
                cs.grid_lanes_direction.track_reverse,
                false,
            );
            for (i, (_, p)) in setup.items.iter().enumerate() {
                let (s, e, auto) = match *p {
                    Placement::Definite(a, e) => (a as usize, e as usize, false),
                    Placement::Auto(span) => {
                        let span = setup.auto_span(span);
                        let s = run.choose(span);
                        (s, s + span, true)
                    }
                };
                let w = if k == 0 { widths[i].0 } else { widths[i].1 };
                run.commit(s, e, w, auto);
            }
            *size = run.range_end();
        }
        (sizes[0], sizes[1].max(sizes[0]))
    }
}
