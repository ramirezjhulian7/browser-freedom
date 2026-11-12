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
    /// E5: nombres de línea que hereda un subgrid del ítem `i` en las pistas `span` del eje de
    /// rejilla. Un subgrid auto-colocado no los hereda (su posición no se conoce al dimensionar;
    /// WPT `grid-subgridded-to-grid-lanes/line-names/*-line-names-016`): solo líneas sin nombre.
    fn subgrid_names(&self, i: usize, span: (usize, usize)) -> Vec<Vec<String>> {
        match self.items[i].1 {
            Placement::Auto(_) => vec![Vec::new(); span.1.saturating_sub(span.0) + 1],
            Placement::Definite(..) => span_line_names(&self.axis, self.before, span),
        }
    }

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
        // E5: un ítem subgrid de un contenedor grid lanes solo hereda el eje de rejilla (CSS
        // Grid 3, "subgrids"): en el de apilado `subgrid` cuenta como sin pistas heredadas.
        for c in &children {
            self.subgrid_mask
                .insert(*c as *const LayoutBox as usize, (!rows, rows));
        }
        let axis = explicit_axis(template, &cs.grid_template_areas, !rows, space, gap);
        let hyp = self.lanes_hypothetical_sizes(
            container,
            template,
            rows,
            &children,
            &axis,
            (space, gap, size),
            width,
        );
        let axis = match hyp {
            Some(h) => explicit_axis_hyp(
                template,
                &cs.grid_template_areas,
                !rows,
                space,
                gap,
                Some(&h),
            ),
            None => axis,
        };
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

    /// Tamaños hipotéticos de las pistas de un `repeat(auto-fill|auto-fit)` con pistas
    /// intrínsecas (§3.1.1), para contar las repeticiones: se ignora la colocación explícita
    /// (todos los ítems se auto-colocan con su span), no se colapsa ninguna pista, la repetición
    /// se expande `2 + (span mayor - 2) / (pistas de la repetición)` veces (redondeado hacia
    /// abajo; una sola si el span mayor es 1) y cada pista de la repetición toma el mayor
    /// tamaño de sus copias. `None` si no hay tal repetición.
    #[allow(clippy::too_many_arguments)]
    fn lanes_hypothetical_sizes(
        &mut self,
        container: &'a LayoutBox,
        template: &GridTemplate,
        rows: bool,
        children: &[&'a LayoutBox],
        axis: &ExplicitAxis,
        (space, gap, size): (RepeatSpace, f32, Option<f32>),
        width: Option<f32>,
    ) -> Option<Vec<f32>> {
        let GridTemplate::List(list) = template else {
            return None;
        };
        if !has_intrinsic_auto_repeat(list) {
            return None;
        }
        let (before, inner) = auto_repeat_parts(list)?;
        let per = inner
            .iter()
            .filter(|i| matches!(i, TrackListItem::Track(_)))
            .count()
            .max(1);
        let cs = &container.style;
        let spans: Vec<usize> = children
            .iter()
            .map(|c| {
                let s = &c.style;
                let p = if rows {
                    resolve_placement(&s.grid_row_start, &s.grid_row_end, axis)
                } else {
                    resolve_placement(&s.grid_column_start, &s.grid_column_end, axis)
                };
                match p {
                    Placement::Definite(a, b) => (b - a).max(1) as usize,
                    Placement::Auto(span) => span.max(1) as usize,
                }
            })
            .collect();
        let largest = spans.iter().copied().max().unwrap_or(1);
        let times = if largest < 2 {
            1
        } else {
            2 + (largest - 2) / per
        };
        let expanded: Vec<TrackListItem> = list
            .iter()
            .map(|i| match i {
                TrackListItem::Repeat(RepeatCount::AutoFill | RepeatCount::AutoFit, inner) => {
                    TrackListItem::Repeat(RepeatCount::Count(times as u32), inner.clone())
                }
                i => i.clone(),
            })
            .collect();
        let hax = explicit_axis(
            &GridTemplate::List(expanded),
            &GridTemplateAreas::None,
            !rows,
            space,
            gap,
        );
        let n = hax.count;
        if n == 0 {
            return None;
        }
        let auto = if rows {
            &cs.grid_auto_rows.0
        } else {
            &cs.grid_auto_columns.0
        };
        let mut tracks = build_tracks(&hax, auto, 0, n, size);
        let mut contribs = Vec::new();
        for (&b, &span) in children.iter().zip(&spans) {
            let span = span.min(n);
            let outer_w = rows.then(|| {
                let cb_w = width.unwrap_or(0.0);
                let (w, ml, mr) = self.lanes_stack_width(b, cb_w);
                w + border_edges(&b.style).horizontal()
                    + padding_edges(&b.style, cb_w).horizontal()
                    + ml
                    + mr
            });
            for s in 0..=n - span {
                let it = GridItem {
                    b,
                    col: if rows { (0, 1) } else { (s, s + span) },
                    row: if rows { (s, s + span) } else { (0, 1) },
                    extra: [0.0; 4],
                    area_w: None,
                    empty: false,
                };
                contribs.push(match outer_w {
                    Some(w) => self.row_contrib(&it, cs, &tracks, gap, w).0,
                    None => self.column_contrib(&it, &tracks, gap, None),
                });
            }
        }
        let sizing = size.map_or(Space::MaxContent, Space::Definite);
        size_tracks(
            &mut tracks,
            &contribs,
            gap,
            sizing,
            (0.0, f32::INFINITY),
            false,
        );
        let mut hyp = vec![0.0f32; per];
        for c in 0..times {
            for (j, h) in hyp.iter_mut().enumerate() {
                if let Some(t) = tracks.get(before + c * per + j) {
                    *h = h.max(t.base);
                }
            }
        }
        Some(hyp)
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
            // E5: con los nombres de línea del eje (para los subgrids).
            col_axis: setup.axis.clone(),
            row_axis: empty_axis(),
            col_before: setup.before,
            row_before: 0,
            sub: SubgridParent::default(),
        }
    }

    /// E5: ítems que dimensionan las columnas de la rejilla falsa de `lanes_fake_grid`: cada
    /// subgrid en las columnas recibe los nombres de línea de su posición (§9 de Grid 2; un
    /// auto-colocado aparece en cada posición posible) y se sustituye por sus ítems.
    fn lanes_column_items(
        &mut self,
        setup: &LanesSetup<'a>,
        grid: &Grid<'a>,
        gap: f32,
        basis: f32,
    ) -> Vec<GridItem<'a>> {
        let mut out = Vec::with_capacity(grid.items.len());
        // `grid.items` sigue el orden de `sizing_spans` (ver `lanes_fake_grid`).
        for (it, (i, _)) in grid.items.iter().zip(setup.sizing_spans()) {
            if self.sub_axes(it.b).0 {
                let entry = SubgridParent {
                    cols: Some(SubgridAxis {
                        names: setup.subgrid_names(i, it.col),
                        sizes: Vec::new(),
                        gap,
                    }),
                    rows: None,
                };
                self.subgrids
                    .insert(it.b as *const LayoutBox as usize, entry);
            }
            self.push_axis_item(it.clone(), true, None, basis, &mut out, 0);
        }
        out
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
    pub(super) fn lanes_layout(
        &mut self,
        container: &'a LayoutBox,
        cb: Cb,
    ) -> (Vec<Fragment>, f32) {
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
        let lines: (Vec<f32>, Vec<f32>) = if !rows {
            // --- Columnas como eje de rejilla ---
            let col_gap = gap_px(&cs.column_gap, Some(cb.width));
            let row_gap = gap_px(&cs.row_gap, cb.height);
            let mut grid = self.lanes_fake_grid(&setup);
            // Como `size_grid_columns`, con los subgrids sustituidos por sus ítems.
            let items = self.lanes_column_items(&setup, &grid, col_gap, cb.width);
            let contribs: Vec<Contrib> = items
                .iter()
                .map(|it| self.column_contrib(it, &grid.cols, col_gap, None))
                .collect();
            size_tracks(
                &mut grid.cols,
                &contribs,
                col_gap,
                Space::Definite(cb.width),
                (cb.width, cb.width),
                matches!(
                    cs.justify_content.value,
                    ContentAlign::Normal | ContentAlign::Stretch
                ),
            );
            let (starts, ends) = track_positions(&grid.cols, col_gap, cb.width, cs.justify_content);
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
            // E5: pistas finales y datos para volver a maquetar (estirado en el apilado).
            let mut spans: Vec<(usize, usize)> = Vec::new();
            let mut relayout: Vec<(Cb, f32, f32, Edges, f32)> = Vec::new();
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
                // E5: un subgrid recibe los nombres y tamaños de las pistas que ocupa.
                if self.sub_axes(b).0 {
                    let sizes =
                        subgrid_track_sizes(s, (c0, c1), (&starts, &ends), col_gap, cb.width, true)
                            .unwrap_or_default();
                    let entry = SubgridParent {
                        cols: Some(SubgridAxis {
                            names: setup.subgrid_names(i, (c0, c1)),
                            sizes,
                            gap: col_gap,
                        }),
                        rows: None,
                    };
                    self.subgrids.insert(b as *const LayoutBox as usize, entry);
                }
                let mut f = self.grid_item_fragment(b, item_cb, (x, cb.y + mt), w, None, margins);
                if f.kind == FragmentKind::Block {
                    f.kind = FragmentKind::InlineBlock;
                }
                let outer = f.rect.height + mt + mb;
                let mut hole_dx = 0.0;
                let mut span = (c0, c1);
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
                        hole_dx = x1 - x0;
                        f.translate(hole_dx, 0.0);
                        span = (t, t + (c1 - c0));
                        start
                    }
                    None => run.commit(c0, c1, outer, auto),
                };
                spans.push(span);
                relayout.push((item_cb, x, w, margins, hole_dx));
                f.translate(0.0, y);
                let key = (y, c0, i);
                if first.is_none_or(|k| key.0 < k.0 || (key.0 == k.0 && key.1 < k.1)) {
                    first = Some(key);
                }
                stack.push((y, outer));
                out.push(f);
            }
            let range = run.range_end();
            // E5: alineación del último ítem de cada pista en el eje de apilado.
            // El último de cada pista llega hasta el final del content box.
            let end = cb
                .height
                .unwrap_or_else(|| range.min(hmax).max(hmin))
                .max(range);
            let frees = stacking_free_space(&spans, &stack, n, (row_gap, end));
            for (i, free) in frees.into_iter().enumerate() {
                if free <= 0.0 {
                    continue;
                }
                let b = setup.items[i].0;
                let s = &b.style;
                let a = match block_axis(s.align_self).value {
                    ItemAlign::Auto => block_axis(cs.align_items).value,
                    a => a,
                };
                let (item_cb, x, w, margins, hole_dx) = relayout[i];
                let auto_m = margin_value(&s.margin_top, item_cb.width).is_none()
                    || margin_value(&s.margin_bottom, item_cb.width).is_none();
                if a == ItemAlign::Stretch && s.height == Size::Auto && !auto_m {
                    let bp_v =
                        border_edges(s).vertical() + padding_edges(s, item_cb.width).vertical();
                    let h = (stack[i].1 + free - margins.top - margins.bottom - bp_v).max(0.0);
                    let (hlo, hhi) = height_limits(s, item_cb.height, bp_v);
                    let h = h.min(hhi).max(hlo);
                    let mut f = self.grid_item_fragment(
                        b,
                        item_cb,
                        (x, cb.y + margins.top),
                        w,
                        Some(h),
                        margins,
                    );
                    if f.kind == FragmentKind::Block {
                        f.kind = FragmentKind::InlineBlock;
                    }
                    f.translate(hole_dx, stack[i].0);
                    stack[i].1 = f.rect.height + margins.top + margins.bottom;
                    out[i] = f;
                    continue;
                }
                let shift = stacking_shift(a, free, fill_reverse);
                if shift != 0.0 {
                    out[i].translate(0.0, shift);
                    stack[i].0 += shift;
                }
            }
            content_h = cb.height.unwrap_or_else(|| range.min(hmax).max(hmin));
            let off = stacking_offset(cs.align_content.value, content_h, range, fill_reverse);
            for (f, (y, outer)) in out.iter_mut().zip(&stack) {
                // `fill-reverse`: se refleja en el rango de apilado.
                let dy = if fill_reverse {
                    range - y - outer - y
                } else {
                    0.0
                };
                f.translate(0.0, dy + off);
            }
            (starts, ends)
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
                let outer_w = w + bp_h + ml + mr;
                // E5: un subgrid en las filas recibe los nombres de línea de esta posición y
                // lo sustituyen sus ítems (con sus columnas propias en su ancho).
                if self.sub_axes(b).1 {
                    let entry = SubgridParent {
                        cols: None,
                        rows: Some(SubgridAxis {
                            names: setup.subgrid_names(i, span),
                            sizes: Vec::new(),
                            gap: row_gap,
                        }),
                    };
                    self.subgrids.insert(b as *const LayoutBox as usize, entry);
                    let it = GridItem {
                        area_w: Some(outer_w),
                        ..it
                    };
                    let mut expanded = Vec::new();
                    self.push_axis_item(it, false, None, cb.width, &mut expanded, 0);
                    for e in &expanded {
                        let aw = e.area_w.unwrap_or(outer_w);
                        let (c, _) = self.row_contrib(e, cs, &tracks, row_gap, aw);
                        contribs.push(c);
                    }
                    continue;
                }
                let (c, _) = self.row_contrib(&it, cs, &tracks, row_gap, outer_w);
                contribs.push(c);
            }
            let stretch = matches!(
                cs.align_content.value,
                ContentAlign::Normal | ContentAlign::Stretch
            );
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
            let mut spans: Vec<(usize, usize)> = Vec::new();
            let mut relayout: Vec<(Cb, f32, f32, Option<f32>, Edges)> = Vec::new();
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
                // E5: un subgrid en las filas recibe los tamaños de las pistas que ocupa y se
                // estira siempre, con los márgenes `auto` a 0 (§9 de Grid 2).
                let sub_rows = self.sub_axes(b).1;
                let (mt, mb) = if sub_rows {
                    let sizes = subgrid_track_sizes(
                        s,
                        (r0, r1),
                        (&starts, &ends),
                        row_gap,
                        cb.width,
                        false,
                    )
                    .unwrap_or_default();
                    let entry = SubgridParent {
                        cols: None,
                        rows: Some(SubgridAxis {
                            names: setup.subgrid_names(i, (r0, r1)),
                            sizes,
                            gap: row_gap,
                        }),
                    };
                    self.subgrids.insert(b as *const LayoutBox as usize, entry);
                    (mt.or(Some(0.0)), mb.or(Some(0.0)))
                } else {
                    (mt, mb)
                };
                let forced = if (align == ItemAlign::Stretch || sub_rows)
                    && (matches!(s.height, Size::Auto) || sub_rows)
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
                spans.push((r0, r1));
                relayout.push((item_cb, x, y, forced, margins));
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
            // E5: alineación del último ítem de cada pista en el eje de apilado (inline).
            let frees = stacking_free_space(&spans, &stack, n, (col_gap, cb.width.max(range)));
            for (i, free) in frees.into_iter().enumerate() {
                if free <= 0.0 {
                    continue;
                }
                let b = setup.items[i].0;
                let s = &b.style;
                let a = match s.justify_self.value {
                    ItemAlign::Auto => cs.justify_items.value,
                    a => a,
                };
                // `left`/`right` son físicos: con el flujo de derecha a izquierda se invierten.
                let a = match a {
                    ItemAlign::Left => ItemAlign::Start,
                    ItemAlign::Right => ItemAlign::End,
                    a => a,
                };
                let mirrored = rtl != fill_reverse;
                let (item_cb, x, y, forced, margins) = relayout[i];
                let auto_m = margin_value(&s.margin_left, cb.width).is_none()
                    || margin_value(&s.margin_right, cb.width).is_none();
                if a == ItemAlign::Stretch && s.width == Size::Auto && !auto_m {
                    let w = widths[i].0 + free;
                    let bp_h =
                        border_edges(s).horizontal() + padding_edges(s, cb.width).horizontal();
                    let (wlo, whi) = width_limits(s, cb.width, bp_h);
                    let w = w.min(whi).max(wlo);
                    let mut f = self.grid_item_fragment(b, item_cb, (x, y), w, forced, margins);
                    if f.kind == FragmentKind::Block {
                        f.kind = FragmentKind::InlineBlock;
                    }
                    let dy = out[i].rect.y - f.rect.y;
                    f.translate(0.0, dy);
                    stack[i].1 = w + bp_h + margins.left + margins.right;
                    out[i] = f;
                    continue;
                }
                let shift = stacking_shift(a, free, mirrored);
                if shift != 0.0 {
                    out[i].translate(shift, 0.0);
                    stack[i].0 += shift;
                }
            }
            let off = stacking_offset(cs.justify_content.value, cb.width, range, fill_reverse);
            for (f, (x, outer)) in out.iter_mut().zip(&stack) {
                // En rtl el flujo va de derecha a izquierda; `fill-reverse` lo invierte.
                let mirrored = rtl != fill_reverse;
                let dx = if mirrored { range - x - outer - x } else { 0.0 };
                let off = if rtl { cb.width - range - off } else { off };
                f.translate(dx + off, 0.0);
            }
            (starts, ends)
        };
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
                let top =
                    to_index(l0).map_or(padding_box.y, |l| cb.y + line_pos(starts, ends, l, true));
                let bottom = to_index(l1).map_or(padding_box.bottom(), |l| {
                    cb.y + line_pos(starts, ends, l, false)
                });
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
            let items = self.lanes_column_items(&setup, &grid, gap, 0.0);
            let contribs: Vec<Contrib> = items
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

/// E5: alineación propia en el eje de apilado (CSS Grid 3, "alignment in the stacking axis"):
/// el contenedor de alineación de un ítem va desde su posición hasta el inicio (menos el gap)
/// del siguiente ítem en cualquiera de sus pistas, o hasta `end` (el final del content box)
/// si es el último en todas. `spans` son las pistas de cada ítem y `stack` su posición de
/// apilado y su tamaño exterior; `gap` es el del eje de apilado. Devuelve, por ítem, el espacio
/// libre de su contenedor de alineación.
fn stacking_free_space(
    spans: &[(usize, usize)],
    stack: &[(f32, f32)],
    n: usize,
    (gap, end): (f32, f32),
) -> Vec<f32> {
    spans
        .iter()
        .enumerate()
        .map(|(i, &(t0, t1))| {
            let (y, outer) = stack[i];
            let limit = (t0..t1.min(n))
                .map(|t| {
                    stack
                        .iter()
                        .zip(spans)
                        .enumerate()
                        .filter(|&(j, (&(yj, _), &(u0, u1)))| {
                            j != i && u0 <= t && t < u1 && (yj > y || (yj == y && j > i))
                        })
                        .map(|(_, (&(yj, _), _))| yj - gap)
                        .fold(end, f32::min)
                })
                .fold(end, f32::min);
            (limit - y - outer).max(0.0)
        })
        .collect()
}

/// E5: desplazamiento en el eje de apilado (en el sentido del flujo) de un ítem con alineación
/// `a` y espacio libre `free`; con el flujo invertido el inicio físico es el final del flujo.
fn stacking_shift(a: ItemAlign, free: f32, flow_reversed: bool) -> f32 {
    match a {
        ItemAlign::Center => free / 2.0,
        // `flow-start`/`flow-end` siguen el flujo de apilado aunque esté invertido.
        ItemAlign::FlowEnd => free,
        ItemAlign::End | ItemAlign::FlexEnd | ItemAlign::SelfEnd | ItemAlign::LastBaseline => {
            if flow_reversed {
                0.0
            } else {
                free
            }
        }
        ItemAlign::Start | ItemAlign::FlexStart | ItemAlign::SelfStart if flow_reversed => free,
        _ => 0.0,
    }
}
