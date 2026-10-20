//! CSS Grid Layout Module Level 2: rejilla explícita e implícita (§7), colocación de los ítems
//! (§8), elementos absolutos (§9), alineación (§10) y dimensionado de pistas (§11 y §12).
//!
//! Solo `writing-mode: horizontal-tb` y `direction: ltr`: las columnas son el eje inline y las
//! filas el de bloque. Sin `subgrid` ni `masonry`.
//!
//! Interpretaciones:
//! - Las pistas con porcentaje contra un tamaño indefinido se tratan como `auto` (§7.2.1); las
//!   columnas siempre tienen el ancho del contenedor salvo al calcular sus tamaños intrínsecos.
//! - Las filas de un contenedor con altura indefinida se dimensionan como bajo una restricción
//!   max-content (§12.6: el espacio libre se trata como infinito), limitada por `max-height`.
//!   Si hay pistas o gaps con porcentaje o pistas flexibles, se vuelven a dimensionar con la
//!   altura resultante como espacio definido (como los navegadores).
//! - Con `repeat(auto-fill|auto-fit)`, al calcular los tamaños intrínsecos del contenedor el
//!   espacio es indefinido aunque `width` sea fijo: solo cuentan `max-width`/`min-width`.
//! - Contribuciones en el eje de bloque: la altura del ítem maquetado con el ancho final de su
//!   área (min-content = max-content). Los porcentajes de altura de los ítems se resuelven
//!   contra el área ya dimensionada.
//! - Alineación propia sin `safe`: un ítem más grande que su área desborda por ambos lados con
//!   `center` y por el inicio con `end`. `baseline` se trata como `start` en las columnas; en
//!   las filas alinea los ítems de span 1 que comparten fila (§10.6, sin el "shim" de §12.5).
//! - Baseline del contenedor (§10.7): la del primer ítem (en orden de rejilla) de la primera fila
//!   con ítems; sin line boxes se sintetiza con el borde inferior de su border box.
//! - Los ítems establecen un formato independiente (§6.1: sus márgenes no colapsan con su
//!   contenido) y se pintan como inline-blocks (§6.4); `z-index` en ítems no posicionados no se
//!   aplica (lo decide la pintura).
//! - Elementos absolutos (§9): con el contenedor posicionado, los hijos y los descendientes
//!   cuyo containing block es el contenedor usan el área de sus líneas (`auto` = padding box);
//!   con los insets `auto` se alinean en ella con `justify-self`/`align-self`. Si no está
//!   posicionado, la posición estática de los hijos se alinea en el content box (§9.2).
//! - El límite de la rejilla implícita es de ±1000 líneas (§8.3.1, como los navegadores).

use bf_style::ComputedStyle;
use bf_style::properties::types::{
    BoxSizing, ContentAlign, Display, Gap, GridLine, GridTemplate, GridTemplateAreas, ItemAlign,
    MaxSize, Overflow, Position, RepeatCount, Size, TrackBreadth, TrackListItem, TrackSize,
};

use crate::block::{
    Cb, Layout, PendingAbs, border_edges, height_limits, is_positioned, margin_value,
    padding_edges, specified_width, width_limits,
};
use crate::box_tree::{BoxKind, LayoutBox};
use crate::float::FloatContext;
use crate::fragment::{Edges, Fragment, FragmentKind, Rect};

/// Línea más alejada admitida a cada lado de la rejilla explícita (§8.3.1, nota sobre límites).
const MAX_LINE: i32 = 1000;

pub(crate) fn is_grid_container(b: &LayoutBox) -> bool {
    matches!(b.style.display, Display::Grid | Display::InlineGrid)
        && !matches!(b.kind, BoxKind::Replaced { .. })
}

/// Baseline de un contenedor grid (§10.7): la que calculó [`Layout::grid_layout`] y dejó en el
/// fragmento; `None` si no hay ítems (se sintetiza).
pub(crate) fn container_baseline(f: &Fragment) -> Option<f32> {
    f.baseline
}

/// ¿Es el fragmento de un ítem de grid? Se pintan como inline-blocks (§6.4), pero son hijos
/// directos de un bloque: el único `InlineBlock` en flujo fuera de una line box.
pub(crate) fn is_item_fragment(f: &Fragment) -> bool {
    f.kind == FragmentKind::InlineBlock
        && !matches!(f.style.position, Position::Absolute | Position::Fixed)
}

fn gap_px(g: &Gap, basis: Option<f32>) -> f32 {
    match g {
        Gap::Normal => 0.0,
        // Un porcentaje contra un tamaño indefinido (tamaños intrínsecos) se resuelve contra 0
        // (§10.1).
        Gap::LengthPercentage(lp) => lp.resolve(basis.unwrap_or(0.0)),
    }
}

// ---------------------------------------------------------------------------------------------
// Rejilla explícita (§7)
// ---------------------------------------------------------------------------------------------

/// Espacio con el que se calcula el número de repeticiones de `repeat(auto-fill|auto-fit)`
/// (§7.2.3.2) y la base de los porcentajes de las pistas.
#[derive(Clone, Copy)]
struct RepeatSpace {
    /// Tamaño definido (o máximo) del content box en el eje.
    size: Option<f32>,
    /// Tamaño mínimo definido, si no hay tamaño.
    min: Option<f32>,
    /// Base de los porcentajes de las pistas.
    basis: Option<f32>,
}

/// Rejilla explícita en un eje.
struct ExplicitAxis {
    /// Funciones de dimensionado de `grid-template-*` (con las repeticiones expandidas).
    sizes: Vec<TrackSize>,
    /// Nombres de cada línea explícita (`count + 1` entradas), con los implícitos de las áreas
    /// con nombre (§7.3.2).
    names: Vec<Vec<String>>,
    /// Pistas que generó `repeat(auto-fit)`: se colapsan si quedan vacías (§7.2.3.4).
    auto_fit: Option<(usize, usize)>,
    /// Número de pistas explícitas (puede superar `sizes.len()` por `grid-template-areas`).
    count: usize,
}

impl ExplicitAxis {
    /// Índices de las líneas explícitas con el nombre dado, en orden.
    fn lines_named(&self, name: &str) -> Vec<i32> {
        self.names
            .iter()
            .enumerate()
            .filter(|(_, n)| n.iter().any(|x| x == name))
            .map(|(i, _)| i as i32)
            .collect()
    }
}

/// Tamaño fijo de una pista para contar repeticiones automáticas (§7.2.3.2): su función máxima
/// si es definida o, si no, la mínima.
fn repeat_track_size(t: &TrackSize, basis: Option<f32>) -> f32 {
    let fixed = |b: &TrackBreadth| match b {
        TrackBreadth::Length(lp) if !lp.has_percentage() || basis.is_some() => {
            Some(lp.resolve(basis.unwrap_or(0.0)).max(0.0))
        }
        _ => None,
    };
    let min = fixed(t.min());
    match (fixed(t.max()), min) {
        (Some(max), Some(min)) => max.max(min),
        (Some(max), None) => max,
        (None, Some(min)) => min,
        (None, None) => 0.0,
    }
}

/// Expande una lista de pistas: tamaños y nombres de línea (los nombres en los extremos de una
/// repetición se juntan con los vecinos, §7.2.3).
fn expand_tracks(
    items: &[TrackListItem],
    auto_count: usize,
    sizes: &mut Vec<TrackSize>,
    names: &mut Vec<Vec<String>>,
    auto_fit: &mut Option<(usize, usize)>,
) {
    for item in items {
        match item {
            TrackListItem::Names(n) => {
                if let Some(last) = names.last_mut() {
                    last.extend(n.iter().cloned());
                }
            }
            TrackListItem::Track(t) => {
                sizes.push(t.clone());
                names.push(Vec::new());
            }
            TrackListItem::Repeat(count, inner) => {
                let (n, fit) = match count {
                    RepeatCount::Count(n) => (*n as usize, false),
                    RepeatCount::AutoFill => (auto_count, false),
                    RepeatCount::AutoFit => (auto_count, true),
                };
                let start = sizes.len();
                for _ in 0..n.min(MAX_LINE as usize) {
                    if sizes.len() >= MAX_LINE as usize {
                        break;
                    }
                    expand_tracks(inner, auto_count, sizes, names, auto_fit);
                }
                if fit {
                    *auto_fit = Some((start, sizes.len()));
                }
            }
        }
    }
}

/// Suma de tamaños fijos y número de pistas de una lista (repeticiones con número expandidas).
fn fixed_sum(items: &[TrackListItem], times: usize, basis: Option<f32>) -> (f32, usize) {
    let (mut sum, mut n) = (0.0, 0);
    for i in items {
        match i {
            TrackListItem::Track(t) => {
                sum += repeat_track_size(t, basis) * times as f32;
                n += times;
            }
            TrackListItem::Repeat(RepeatCount::Count(c), inner) => {
                let (s, k) = fixed_sum(inner, times * *c as usize, basis);
                sum += s;
                n += k;
            }
            _ => {}
        }
    }
    (sum, n)
}

/// Número de repeticiones de `repeat(auto-fill|auto-fit)` (§7.2.3.2).
fn auto_repeat_count(items: &[TrackListItem], space: RepeatSpace, gap: f32) -> usize {
    let Some(inner) = items.iter().find_map(|i| match i {
        TrackListItem::Repeat(RepeatCount::AutoFill | RepeatCount::AutoFit, inner) => Some(inner),
        _ => None,
    }) else {
        return 1;
    };
    let (other_sum, other_count) = fixed_sum(items, 1, space.basis);
    let (rep_sum, rep_count) = fixed_sum(inner, 1, space.basis);
    if rep_count == 0 {
        return 1;
    }
    let total1 = other_sum + rep_sum + gap * (other_count + rep_count - 1) as f32;
    let step = rep_sum + gap * rep_count as f32;
    if step <= 0.0 {
        return 1;
    }
    let n = match (space.size, space.min) {
        (Some(avail), _) => ((avail - total1) / step + 1e-3).floor() + 1.0,
        (None, Some(min)) => ((min - total1) / step - 1e-3).ceil() + 1.0,
        _ => 1.0,
    };
    (n.max(1.0) as usize).min(MAX_LINE as usize)
}

fn explicit_axis(
    template: &GridTemplate,
    areas: &GridTemplateAreas,
    columns: bool,
    space: RepeatSpace,
    gap: f32,
) -> ExplicitAxis {
    let mut sizes = Vec::new();
    let mut names = vec![Vec::new()];
    let mut auto_fit = None;
    if let GridTemplate::List(items) = template {
        let n = auto_repeat_count(items, space, gap);
        expand_tracks(items, n, &mut sizes, &mut names, &mut auto_fit);
    }
    let mut count = sizes.len();
    if let GridTemplateAreas::Areas {
        rows,
        columns: cols,
        areas,
        ..
    } = areas
    {
        count = count.max(if columns { *cols } else { *rows } as usize);
        names.resize(count + 1, Vec::new());
        for a in areas {
            let (s, e) = if columns {
                (a.column_start, a.column_end)
            } else {
                (a.row_start, a.row_end)
            };
            names[s as usize - 1].push(format!("{}-start", a.name));
            names[e as usize - 1].push(format!("{}-end", a.name));
        }
    }
    ExplicitAxis {
        sizes,
        names,
        auto_fit,
        count,
    }
}

// ---------------------------------------------------------------------------------------------
// Resolución de líneas (§8.3) y errores de colocación (§8.3.1)
// ---------------------------------------------------------------------------------------------

/// Colocación de un ítem en un eje, en índices de línea relativos a la rejilla explícita (la
/// línea 1 es 0; pueden ser negativos).
#[derive(Clone, Copy, Debug)]
enum Placement {
    /// `[inicio, fin)`.
    Definite(i32, i32),
    /// Auto-colocado con este span.
    Auto(i32),
}

/// Línea `<integer>? <custom-ident>?` (§8.3). `start` indica si es la de inicio (para los
/// nombres implícitos `-start`/`-end` de las áreas).
fn resolve_line(integer: i32, name: Option<&str>, start: bool, ax: &ExplicitAxis) -> i32 {
    let explicit = ax.count as i32;
    let integer = integer.clamp(-4 * MAX_LINE, 4 * MAX_LINE);
    let Some(name) = name else {
        return if integer > 0 {
            integer - 1
        } else {
            explicit + 1 + integer
        };
    };
    if integer == 0 {
        // Solo un nombre: primero el borde del área con ese nombre (§8.3, <custom-ident>).
        let suffixed = format!("{name}-{}", if start { "start" } else { "end" });
        if let Some(&l) = ax.lines_named(&suffixed).first() {
            return l;
        }
        return resolve_line(1, Some(name), start, ax);
    }
    let lines = ax.lines_named(name);
    let k = integer.unsigned_abs() as usize;
    if integer > 0 {
        // Si no hay suficientes, todas las líneas implícitas tienen ese nombre.
        lines
            .get(k - 1)
            .copied()
            .unwrap_or_else(|| explicit + (k - lines.len()) as i32)
    } else if k <= lines.len() {
        lines[lines.len() - k]
    } else {
        -((k - lines.len()) as i32)
    }
}

/// Línea a `n` líneas (con nombre `name`, si lo hay) de `from` hacia el final o el inicio.
fn span_from(from: i32, n: u32, name: Option<&str>, forward: bool, ax: &ExplicitAxis) -> i32 {
    let n = n.clamp(1, MAX_LINE as u32 * 2) as i32;
    let Some(name) = name else {
        return if forward { from + n } else { from - n };
    };
    let lines = ax.lines_named(name);
    let explicit = ax.count as i32;
    if forward {
        let after: Vec<i32> = lines.into_iter().filter(|&l| l > from).collect();
        after
            .get(n as usize - 1)
            .copied()
            .unwrap_or(from.max(explicit) + n - after.len() as i32)
    } else {
        let before: Vec<i32> = lines.into_iter().rev().filter(|&l| l < from).collect();
        before
            .get(n as usize - 1)
            .copied()
            .unwrap_or(from.min(0) - (n - before.len() as i32))
    }
}

fn clamp_line(l: i32, ax: &ExplicitAxis) -> i32 {
    l.clamp(-MAX_LINE, ax.count as i32 + MAX_LINE)
}

/// Resuelve `grid-*-start`/`grid-*-end` de un ítem en flujo (§8.3 y §8.3.1).
fn resolve_placement(start: &GridLine, end: &GridLine, ax: &ExplicitAxis) -> Placement {
    let mut start = start.clone();
    let mut end = end.clone();
    // Dos spans: se descarta el del final.
    if matches!(start, GridLine::Span { .. }) && matches!(end, GridLine::Span { .. }) {
        end = GridLine::Auto;
    }
    // Un span con nombre frente a `auto` cuenta como span 1.
    let one = GridLine::Span {
        integer: 1,
        name: None,
    };
    if matches!(start, GridLine::Span { name: Some(_), .. }) && end.is_auto() {
        start = one.clone();
    }
    if matches!(end, GridLine::Span { name: Some(_), .. }) && start.is_auto() {
        end = one;
    }
    let p = match (&start, &end) {
        (GridLine::Span { integer, .. }, GridLine::Auto)
        | (GridLine::Auto, GridLine::Span { integer, .. }) => {
            Placement::Auto((*integer).clamp(1, MAX_LINE as u32) as i32)
        }
        (GridLine::Line { integer, name }, GridLine::Auto) => {
            let a = resolve_line(*integer, name.as_deref(), true, ax);
            Placement::Definite(a, a + 1)
        }
        (GridLine::Auto, GridLine::Line { integer, name }) => {
            let b = resolve_line(*integer, name.as_deref(), false, ax);
            Placement::Definite(b - 1, b)
        }
        (
            GridLine::Line {
                integer: i1,
                name: n1,
            },
            GridLine::Line {
                integer: i2,
                name: n2,
            },
        ) => {
            let a = resolve_line(*i1, n1.as_deref(), true, ax);
            let b = resolve_line(*i2, n2.as_deref(), false, ax);
            match a.cmp(&b) {
                std::cmp::Ordering::Less => Placement::Definite(a, b),
                std::cmp::Ordering::Greater => Placement::Definite(b, a),
                std::cmp::Ordering::Equal => Placement::Definite(a, a + 1),
            }
        }
        (
            GridLine::Line { integer, name },
            GridLine::Span {
                integer: n,
                name: sn,
            },
        ) => {
            let a = resolve_line(*integer, name.as_deref(), true, ax);
            Placement::Definite(a, span_from(a, *n, sn.as_deref(), true, ax))
        }
        (
            GridLine::Span {
                integer: n,
                name: sn,
            },
            GridLine::Line { integer, name },
        ) => {
            let b = resolve_line(*integer, name.as_deref(), false, ax);
            Placement::Definite(span_from(b, *n, sn.as_deref(), false, ax), b)
        }
        _ => Placement::Auto(1),
    };
    match p {
        Placement::Definite(a, b) => {
            let a = clamp_line(a, ax);
            let b = clamp_line(b, ax).max(a + 1);
            Placement::Definite(a, b)
        }
        p => p,
    }
}

/// Líneas de un elemento absoluto cuyo containing block es el contenedor (§9.1): `None` es el
/// borde del padding box (`auto`, o una línea fuera de la rejilla implícita).
fn resolve_abs_placement(
    start: &GridLine,
    end: &GridLine,
    ax: &ExplicitAxis,
) -> (Option<i32>, Option<i32>) {
    let line = |l: &GridLine, s: bool| match l {
        GridLine::Line { integer, name } => resolve_line(*integer, name.as_deref(), s, ax),
        _ => 0,
    };
    match (start, end) {
        (GridLine::Line { .. }, GridLine::Line { .. }) => {
            let (a, b) = (line(start, true), line(end, false));
            match a.cmp(&b) {
                std::cmp::Ordering::Less => (Some(a), Some(b)),
                std::cmp::Ordering::Greater => (Some(b), Some(a)),
                std::cmp::Ordering::Equal => (Some(a), None),
            }
        }
        (GridLine::Line { .. }, GridLine::Span { integer, name }) => {
            let a = line(start, true);
            (
                Some(a),
                Some(span_from(a, *integer, name.as_deref(), true, ax)),
            )
        }
        (GridLine::Span { integer, name }, GridLine::Line { .. }) => {
            let b = line(end, false);
            (
                Some(span_from(b, *integer, name.as_deref(), false, ax)),
                Some(b),
            )
        }
        (GridLine::Line { .. }, _) => (Some(line(start, true)), None),
        (_, GridLine::Line { .. }) => (None, Some(line(end, false))),
        _ => (None, None),
    }
}

// ---------------------------------------------------------------------------------------------
// Auto-colocación (§8.5)
// ---------------------------------------------------------------------------------------------

/// Celdas ocupadas, indexadas por (eje mayor, eje menor): en `grid-auto-flow: row` el mayor son
/// las filas y el menor las columnas.
#[derive(Default)]
struct Occupancy {
    cells: Vec<Vec<bool>>,
}

impl Occupancy {
    fn free(&self, major: (usize, usize), minor: (usize, usize)) -> bool {
        (major.0..major.1).all(|r| {
            self.cells
                .get(r)
                .is_none_or(|row| (minor.0..minor.1).all(|c| !row.get(c).copied().unwrap_or(false)))
        })
    }

    fn mark(&mut self, major: (usize, usize), minor: (usize, usize)) {
        if self.cells.len() < major.1 {
            self.cells.resize(major.1, Vec::new());
        }
        for row in &mut self.cells[major.0..major.1] {
            if row.len() < minor.1 {
                row.resize(minor.1, false);
            }
            for c in &mut row[minor.0..minor.1] {
                *c = true;
            }
        }
    }
}

type Area = ((usize, usize), (usize, usize));

/// Coloca los ítems (§8.5). `major`/`minor` son sus colocaciones en el eje mayor/menor ya
/// desplazadas a índices no negativos; devuelve las áreas `(mayor, menor)`.
fn auto_place(
    major: &[Placement],
    minor: &[Placement],
    minor_explicit_end: usize,
    dense: bool,
) -> Vec<Area> {
    let n = major.len();
    let mut out: Vec<Option<Area>> = vec![None; n];
    let mut occ = Occupancy::default();
    let def = |p: Placement| match p {
        Placement::Definite(a, b) => Some((a.max(0) as usize, b.max(1) as usize)),
        Placement::Auto(_) => None,
    };
    let span = |p: Placement| match p {
        Placement::Definite(a, b) => (b - a).max(1) as usize,
        Placement::Auto(s) => s.max(1) as usize,
    };
    // 1. Ítems con posición definida en ambos ejes.
    for i in 0..n {
        if let (Some(ma), Some(mi)) = (def(major[i]), def(minor[i])) {
            occ.mark(ma, mi);
            out[i] = Some((ma, mi));
        }
    }
    // 2. Ítems bloqueados en una fila (eje mayor definido, menor automático).
    let mut cursors: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for i in 0..n {
        let (Some(ma), None) = (def(major[i]), def(minor[i])) else {
            continue;
        };
        let s = span(minor[i]);
        let mut c = if dense {
            0
        } else {
            cursors.get(&ma.0).copied().unwrap_or(0)
        };
        while !occ.free(ma, (c, c + s)) {
            c += 1;
        }
        occ.mark(ma, (c, c + s));
        cursors.insert(ma.0, c + s);
        out[i] = Some((ma, (c, c + s)));
    }
    // 3. Número de pistas en el eje menor de la rejilla implícita.
    let mut minor_count = minor_explicit_end;
    for i in 0..n {
        match (out[i], def(minor[i])) {
            (Some((_, mi)), _) | (None, Some(mi)) => minor_count = minor_count.max(mi.1),
            (None, None) => minor_count = minor_count.max(span(minor[i])),
        }
    }
    // 4. El resto, con el cursor de auto-colocación.
    let (mut cmaj, mut cmin) = (0usize, 0usize);
    for i in 0..n {
        if out[i].is_some() {
            continue;
        }
        let smaj = span(major[i]);
        if let Some(mi) = def(minor[i]) {
            if dense {
                cmaj = 0;
            } else if mi.0 < cmin {
                cmaj += 1;
            }
            cmin = mi.0;
            while !occ.free((cmaj, cmaj + smaj), mi) {
                cmaj += 1;
            }
            occ.mark((cmaj, cmaj + smaj), mi);
            out[i] = Some(((cmaj, cmaj + smaj), mi));
        } else {
            let smin = span(minor[i]);
            if dense {
                cmaj = 0;
                cmin = 0;
            }
            loop {
                while cmin + smin <= minor_count
                    && !occ.free((cmaj, cmaj + smaj), (cmin, cmin + smin))
                {
                    cmin += 1;
                }
                if cmin + smin <= minor_count {
                    break;
                }
                cmaj += 1;
                cmin = 0;
            }
            occ.mark((cmaj, cmaj + smaj), (cmin, cmin + smin));
            out[i] = Some(((cmaj, cmaj + smaj), (cmin, cmin + smin)));
        }
    }
    out.into_iter()
        .map(|o| o.unwrap_or(((0, 1), (0, 1))))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Pistas y dimensionado (§11 y §12)
// ---------------------------------------------------------------------------------------------

/// Función de dimensionado ya resuelta a px.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Sizing {
    Fixed(f32),
    MinContent,
    MaxContent,
    Auto,
    Flex(f32),
}

impl Sizing {
    fn intrinsic(self) -> bool {
        matches!(self, Sizing::MinContent | Sizing::MaxContent | Sizing::Auto)
    }
}

#[derive(Clone, Debug)]
struct Track {
    min: Sizing,
    max: Sizing,
    /// Argumento de `fit-content()` (la función máxima es entonces `max-content`).
    fit: Option<f32>,
    base: f32,
    /// Límite de crecimiento (`INFINITY` mientras sea infinito).
    limit: f32,
    /// "Infinitely growable" (§12.5.1 y §12.5 paso 3).
    growable: bool,
    /// Pista vacía de `repeat(auto-fit)` (§7.2.3.4).
    collapsed: bool,
}

impl Track {
    fn new(t: &TrackSize, basis: Option<f32>) -> Track {
        let conv = |b: &TrackBreadth| match b {
            TrackBreadth::Length(lp) if lp.has_percentage() && basis.is_none() => Sizing::Auto,
            TrackBreadth::Length(lp) => Sizing::Fixed(lp.resolve(basis.unwrap_or(0.0)).max(0.0)),
            TrackBreadth::Flex(f) => Sizing::Flex(*f),
            TrackBreadth::MinContent => Sizing::MinContent,
            TrackBreadth::MaxContent => Sizing::MaxContent,
            TrackBreadth::Auto => Sizing::Auto,
        };
        let (min, max, fit) = match t {
            TrackSize::FitContent(lp) => {
                let fit = if lp.has_percentage() && basis.is_none() {
                    None
                } else {
                    Some(lp.resolve(basis.unwrap_or(0.0)).max(0.0))
                };
                (Sizing::Auto, Sizing::MaxContent, fit)
            }
            t => (conv(t.min()), conv(t.max()), None),
        };
        Track {
            min,
            max,
            fit,
            base: 0.0,
            limit: 0.0,
            growable: false,
            collapsed: false,
        }
    }

    fn flex(&self) -> Option<f32> {
        match self.max {
            Sizing::Flex(f) if !self.collapsed => Some(f),
            _ => None,
        }
    }
}

/// Pistas de un eje: implícitas antes de la explícita (`before`), las explícitas y las
/// implícitas después (§7.6: el patrón de `grid-auto-*` se repite en ambos sentidos).
fn build_tracks(
    ax: &ExplicitAxis,
    auto: &[TrackSize],
    before: usize,
    total: usize,
    basis: Option<f32>,
) -> Vec<Track> {
    let auto: Vec<TrackSize> = if auto.is_empty() {
        vec![TrackSize::AUTO]
    } else {
        auto.to_vec()
    };
    let m = auto.len();
    (0..total)
        .map(|i| {
            let t = if i < before {
                let k = before - 1 - i;
                &auto[m - 1 - k % m]
            } else if i - before < ax.sizes.len() {
                &ax.sizes[i - before]
            } else {
                &auto[(i - before - ax.sizes.len()) % m]
            };
            Track::new(t, basis)
        })
        .collect()
}

/// Espacio disponible para el dimensionado de un eje (§12.1).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Space {
    Definite(f32),
    MinContent,
    MaxContent,
}

/// Contribuciones de un ítem en un eje (outer sizes, con márgenes).
#[derive(Clone, Copy, Debug)]
struct Contrib {
    span: (usize, usize),
    minimum: f32,
    min_content: f32,
    max_content: f32,
}

/// Fases de §12.5 paso 3 (y 4 para las pistas flexibles).
#[derive(Clone, Copy, PartialEq)]
enum Phase {
    IntrinsicMin,
    ContentMin,
    MaxContentMin,
    IntrinsicMax,
    MaxContentMax,
}

impl Phase {
    fn base(self) -> bool {
        matches!(
            self,
            Phase::IntrinsicMin | Phase::ContentMin | Phase::MaxContentMin
        )
    }
}

fn gaps_in(tracks: &[Track], span: (usize, usize), gap: f32) -> f32 {
    let n = tracks[span.0..span.1]
        .iter()
        .filter(|t| !t.collapsed)
        .count();
    n.saturating_sub(1) as f32 * gap
}

fn total_size(tracks: &[Track], gap: f32) -> f32 {
    tracks.iter().map(|t| t.base).sum::<f32>() + gaps_in(tracks, (0, tracks.len()), gap)
}

/// Contribución "limitada" (§12.5 paso 2): la min-content (o max-content bajo una restricción
/// max-content) limitada por las funciones máximas fijas, con suelo en la mínima. Con espacio
/// definido, la contribución mínima.
fn limited(c: &Contrib, tracks: &[Track], gap: f32, space: Space) -> f32 {
    let v = match space {
        Space::MinContent => c.min_content,
        Space::MaxContent => c.max_content,
        Space::Definite(_) => return c.minimum,
    };
    let mut sum = gaps_in(tracks, c.span, gap);
    for t in &tracks[c.span.0..c.span.1] {
        match (t.max, t.fit) {
            (Sizing::Fixed(f), _) | (_, Some(f)) => sum += f,
            _ => return v.max(c.minimum),
        }
    }
    v.min(sum).max(c.minimum)
}

fn affected(t: &Track, phase: Phase, space: Space, flex_mode: bool) -> bool {
    if t.collapsed || (flex_mode && t.flex().is_none()) {
        return false;
    }
    match phase {
        Phase::IntrinsicMin => t.min.intrinsic(),
        Phase::ContentMin => matches!(t.min, Sizing::MinContent | Sizing::MaxContent),
        Phase::MaxContentMin => {
            t.min == Sizing::MaxContent || (t.min == Sizing::Auto && space == Space::MaxContent)
        }
        Phase::IntrinsicMax => t.max.intrinsic(),
        Phase::MaxContentMax => matches!(t.max, Sizing::MaxContent | Sizing::Auto),
    }
}

/// Reparte `space` entre pistas con espacio `room` (ponderado por `weights`), congelando las que
/// lo agotan (§12.5.1 pasos 2.1 y 2.3).
fn share(space: &mut f32, inc: &mut [f32], room: &[f32], weights: &[f32], frozen: &mut [bool]) {
    for _ in 0..inc.len() + 1 {
        let total_w: f32 = (0..inc.len())
            .filter(|&i| !frozen[i])
            .map(|i| weights[i])
            .sum();
        if *space <= 1e-4 || total_w <= 0.0 {
            return;
        }
        let mut used = 0.0;
        let mut froze = false;
        for i in 0..inc.len() {
            if frozen[i] {
                continue;
            }
            let s = *space * weights[i] / total_w;
            let r = (room[i] - inc[i]).max(0.0);
            if s >= r {
                inc[i] += r;
                used += r;
                frozen[i] = true;
                froze = true;
            } else {
                inc[i] += s;
                used += s;
            }
        }
        *space -= used;
        if !froze {
            return;
        }
    }
}

/// Una fase de "distribute extra space" (§12.5.1) para un grupo de ítems.
fn run_phase(
    tracks: &mut [Track],
    items: &[&Contrib],
    phase: Phase,
    space: Space,
    gap: f32,
    flex_mode: bool,
) {
    let n = tracks.len();
    let mut planned = vec![0.0f32; n];
    let mut touched = vec![false; n];
    let is_base = phase.base();
    let size_of = |t: &Track| {
        if is_base || t.limit.is_infinite() {
            t.base
        } else {
            t.limit
        }
    };
    let fit_cap = |t: &Track| t.fit.unwrap_or(f32::INFINITY);
    for c in items {
        let aff: Vec<usize> = (c.span.0..c.span.1)
            .filter(|&i| affected(&tracks[i], phase, space, flex_mode))
            .collect();
        if aff.is_empty() {
            continue;
        }
        for &i in &aff {
            touched[i] = true;
        }
        let contribution = match phase {
            Phase::IntrinsicMin => limited(c, tracks, gap, space),
            Phase::ContentMin | Phase::IntrinsicMax => c.min_content,
            // Bajo una restricción max-content, la contribución max-content limitada.
            Phase::MaxContentMin if space == Space::MaxContent => limited(c, tracks, gap, space),
            Phase::MaxContentMin | Phase::MaxContentMax => c.max_content,
        };
        let spanned: f32 = (c.span.0..c.span.1)
            .map(|i| size_of(&tracks[i]))
            .sum::<f32>()
            + gaps_in(tracks, c.span, gap);
        let mut extra = contribution - spanned;
        if extra <= 0.0 {
            continue;
        }
        let cur: Vec<f32> = aff.iter().map(|&i| size_of(&tracks[i])).collect();
        let caps: Vec<f32> = aff
            .iter()
            .map(|&i| {
                let t = &tracks[i];
                if is_base {
                    t.limit.min(fit_cap(t))
                } else if t.growable || t.limit.is_infinite() {
                    fit_cap(t)
                } else {
                    t.limit
                }
            })
            .collect();
        let weights: Vec<f32> = if flex_mode {
            let w: Vec<f32> = aff
                .iter()
                .map(|&i| tracks[i].flex().unwrap_or(0.0))
                .collect();
            if w.iter().sum::<f32>() > 0.0 {
                w
            } else {
                vec![1.0; aff.len()]
            }
        } else {
            vec![1.0; aff.len()]
        };
        let mut inc = vec![0.0f32; aff.len()];
        let room: Vec<f32> = cur
            .iter()
            .zip(&caps)
            .map(|(c, l)| (l - c).max(0.0))
            .collect();
        let mut frozen: Vec<bool> = room.iter().map(|&r| r <= 0.0).collect();
        share(&mut extra, &mut inc, &room, &weights, &mut frozen);
        if extra > 1e-4 {
            // Más allá de los límites (§12.5.1 paso 2.3).
            let target = |t: &Track| match phase {
                Phase::IntrinsicMin | Phase::ContentMin => t.max.intrinsic(),
                Phase::MaxContentMin => matches!(t.max, Sizing::MaxContent | Sizing::Auto),
                _ => true,
            };
            let mut targets: Vec<bool> = aff.iter().map(|&i| target(&tracks[i])).collect();
            if !targets.iter().any(|&t| t) {
                targets = vec![true; aff.len()];
            }
            let room: Vec<f32> = aff
                .iter()
                .zip(&cur)
                .map(|(&i, c)| {
                    let t = &tracks[i];
                    if !is_base && t.fit.is_some() {
                        (fit_cap(t) - c).max(0.0)
                    } else {
                        f32::INFINITY
                    }
                })
                .collect();
            let mut frozen: Vec<bool> = targets.iter().map(|t| !t).collect();
            share(&mut extra, &mut inc, &room, &weights, &mut frozen);
        }
        for (k, &i) in aff.iter().enumerate() {
            planned[i] = planned[i].max(inc[k]);
        }
    }
    for (i, t) in tracks.iter_mut().enumerate() {
        if !touched[i] {
            continue;
        }
        if is_base {
            t.base += planned[i];
        } else if t.limit.is_infinite() {
            t.limit = t.base + planned[i];
            if phase == Phase::IntrinsicMax {
                t.growable = true;
            }
        } else {
            t.limit += planned[i];
        }
        if t.limit < t.base {
            t.limit = t.base;
        }
    }
}

/// Tamaño de un `fr` (§12.7.1) sobre las pistas `idx` para llenar `space`.
fn find_fr(tracks: &[Track], idx: &[usize], space: f32) -> f32 {
    let mut inflexible = vec![false; idx.len()];
    loop {
        let mut leftover = space;
        let mut sum_flex = 0.0;
        for (k, &i) in idx.iter().enumerate() {
            match tracks[i].flex() {
                Some(f) if !inflexible[k] => sum_flex += f,
                _ => leftover -= tracks[i].base,
            }
        }
        let hyp = leftover / if sum_flex < 1.0 { 1.0 } else { sum_flex };
        let mut changed = false;
        for (k, &i) in idx.iter().enumerate() {
            if let Some(f) = tracks[i].flex()
                && !inflexible[k]
                && hyp * f < tracks[i].base
            {
                inflexible[k] = true;
                changed = true;
            }
        }
        if !changed {
            return hyp.max(0.0);
        }
    }
}

/// Algoritmo de dimensionado de pistas (§12.3) en un eje. `limits` son el mínimo y el máximo
/// del content box del contenedor (para las pistas flexibles con espacio indefinido, §12.7, y el
/// estiramiento de §12.8); `stretch` indica si `*-content` es `normal`/`stretch`.
fn size_tracks(
    tracks: &mut [Track],
    contribs: &[Contrib],
    gap: f32,
    space: Space,
    limits: (f32, f32),
    stretch: bool,
) {
    // §12.4: inicializar.
    for t in tracks.iter_mut() {
        if t.collapsed {
            t.min = Sizing::Fixed(0.0);
            t.max = Sizing::Fixed(0.0);
            t.fit = None;
        }
        t.base = match t.min {
            Sizing::Fixed(v) => v,
            _ => 0.0,
        };
        t.limit = match t.max {
            Sizing::Fixed(v) => v,
            _ => f32::INFINITY,
        };
        if t.limit < t.base {
            t.limit = t.base;
        }
        t.growable = false;
    }
    let crosses_flex = |tracks: &[Track], c: &Contrib| {
        tracks[c.span.0..c.span.1]
            .iter()
            .any(|t| t.flex().is_some())
    };
    // §12.5 paso 2: ítems de span 1 en pistas no flexibles.
    for c in contribs {
        if c.span.1 - c.span.0 != 1 || crosses_flex(tracks, c) {
            continue;
        }
        let lim = limited(c, tracks, gap, space);
        let t = &mut tracks[c.span.0];
        if t.collapsed {
            continue;
        }
        match t.min {
            Sizing::MinContent => t.base = t.base.max(c.min_content),
            Sizing::MaxContent => t.base = t.base.max(c.max_content),
            Sizing::Auto => t.base = t.base.max(lim),
            _ => {}
        }
        let grow = match t.max {
            Sizing::MinContent => Some(c.min_content),
            Sizing::MaxContent | Sizing::Auto => {
                Some(t.fit.map_or(c.max_content, |f| c.max_content.min(f)))
            }
            _ => None,
        };
        if let Some(v) = grow {
            t.limit = if t.limit.is_infinite() {
                v
            } else {
                t.limit.max(v)
            };
        }
    }
    for t in tracks.iter_mut() {
        if t.limit < t.base {
            t.limit = t.base;
        }
    }
    // §12.5 paso 3: ítems de span > 1 sin pistas flexibles, por span creciente.
    let mut spans: Vec<usize> = contribs
        .iter()
        .filter(|c| c.span.1 - c.span.0 > 1 && !crosses_flex(tracks, c))
        .map(|c| c.span.1 - c.span.0)
        .collect();
    spans.sort_unstable();
    spans.dedup();
    for s in spans {
        let group: Vec<&Contrib> = contribs
            .iter()
            .filter(|c| c.span.1 - c.span.0 == s && !crosses_flex(tracks, c))
            .collect();
        for phase in [
            Phase::IntrinsicMin,
            Phase::ContentMin,
            Phase::MaxContentMin,
            Phase::IntrinsicMax,
            Phase::MaxContentMax,
        ] {
            run_phase(tracks, &group, phase, space, gap, false);
        }
        for t in tracks.iter_mut() {
            t.growable = false;
        }
    }
    // §12.5 paso 4: ítems que cruzan pistas flexibles (todos juntos).
    let group: Vec<&Contrib> = contribs
        .iter()
        .filter(|c| crosses_flex(tracks, c))
        .collect();
    if !group.is_empty() {
        for phase in [Phase::IntrinsicMin, Phase::ContentMin, Phase::MaxContentMin] {
            run_phase(tracks, &group, phase, space, gap, true);
        }
    }
    // §12.5 paso 5.
    for t in tracks.iter_mut() {
        if t.limit.is_infinite() {
            t.limit = t.base;
        }
    }
    // §12.6: maximizar.
    let maximize_to = |tracks: &mut [Track], avail: f32| {
        let mut free = avail - total_size(tracks, gap);
        if free <= 0.0 {
            return;
        }
        let room: Vec<f32> = tracks.iter().map(|t| (t.limit - t.base).max(0.0)).collect();
        let mut inc = vec![0.0; tracks.len()];
        let weights = vec![1.0; tracks.len()];
        let mut frozen: Vec<bool> = room.iter().map(|&r| r <= 0.0).collect();
        share(&mut free, &mut inc, &room, &weights, &mut frozen);
        for (t, i) in tracks.iter_mut().zip(inc) {
            t.base += i;
        }
    };
    match space {
        Space::Definite(a) => maximize_to(tracks, a),
        Space::MaxContent => {
            let grown: f32 = tracks.iter().map(|t| t.limit).sum::<f32>()
                + gaps_in(tracks, (0, tracks.len()), gap);
            if grown > limits.1 {
                maximize_to(tracks, limits.1);
            } else {
                for t in tracks.iter_mut() {
                    t.base = t.limit;
                }
            }
        }
        Space::MinContent => {}
    }
    // §12.7: expandir las pistas flexibles.
    let all: Vec<usize> = (0..tracks.len()).collect();
    if tracks.iter().any(|t| t.flex().is_some()) {
        let gaps = gaps_in(tracks, (0, tracks.len()), gap);
        let fr = match space {
            Space::MinContent => None,
            Space::Definite(a) => {
                (a - total_size(tracks, gap) > 0.0).then(|| find_fr(tracks, &all, a - gaps))
            }
            Space::MaxContent => {
                let mut fr = 0.0f32;
                for t in tracks.iter() {
                    if let Some(f) = t.flex() {
                        fr = fr.max(if f > 1.0 { t.base / f } else { t.base });
                    }
                }
                for c in contribs.iter().filter(|c| crosses_flex(tracks, c)) {
                    let idx: Vec<usize> = (c.span.0..c.span.1).collect();
                    let space = c.max_content - gaps_in(tracks, c.span, gap);
                    fr = fr.max(find_fr(tracks, &idx, space));
                }
                let size: f32 = tracks
                    .iter()
                    .map(|t| t.flex().map_or(t.base, |f| t.base.max(f * fr)))
                    .sum::<f32>()
                    + gaps;
                if size < limits.0 {
                    fr = find_fr(tracks, &all, limits.0 - gaps);
                } else if size > limits.1 {
                    fr = find_fr(tracks, &all, limits.1 - gaps);
                }
                Some(fr)
            }
        };
        if let Some(fr) = fr {
            for t in tracks.iter_mut() {
                if let Some(f) = t.flex() {
                    t.base = t.base.max(fr * f);
                }
            }
        }
    }
    // §12.8: estirar las pistas `auto`.
    if stretch {
        let avail = match space {
            Space::Definite(a) => Some(a),
            Space::MaxContent if limits.0 > 0.0 => Some(limits.0),
            _ => None,
        };
        if let Some(a) = avail {
            let free = a - total_size(tracks, gap);
            let autos = tracks
                .iter()
                .filter(|t| t.max == Sizing::Auto && !t.collapsed)
                .count();
            if free > 0.0 && autos > 0 {
                for t in tracks.iter_mut() {
                    if t.max == Sizing::Auto && !t.collapsed {
                        t.base += free / autos as f32;
                    }
                }
            }
        }
    }
}

/// Inicio y fin de cada pista (relativos al content box) con los gaps entre pistas no
/// colapsadas y la distribución de `justify-content`/`align-content` (§10.5).
fn track_positions(
    tracks: &[Track],
    gap: f32,
    container: f32,
    align: ContentAlign,
) -> (Vec<f32>, Vec<f32>) {
    let n = tracks.iter().filter(|t| !t.collapsed).count();
    let free = container - total_size(tracks, gap);
    let nf = n as f32;
    let (start, between) = match align {
        ContentAlign::End | ContentAlign::FlexEnd | ContentAlign::Right => (free, 0.0),
        ContentAlign::Center => (free / 2.0, 0.0),
        ContentAlign::SpaceBetween if n > 1 && free > 0.0 => (0.0, free / (nf - 1.0)),
        ContentAlign::SpaceAround if n > 0 && free > 0.0 => (free / nf / 2.0, free / nf),
        ContentAlign::SpaceEvenly if n > 0 && free > 0.0 => (free / (nf + 1.0), free / (nf + 1.0)),
        _ => (0.0, 0.0),
    };
    let (mut starts, mut ends) = (Vec::with_capacity(tracks.len()), Vec::new());
    let mut pos = start;
    let mut first = true;
    for t in tracks {
        if t.collapsed {
            starts.push(pos);
            ends.push(pos);
            continue;
        }
        if !first {
            pos += gap + between;
        }
        first = false;
        starts.push(pos);
        pos += t.base;
        ends.push(pos);
    }
    (starts, ends)
}

/// Posición de la línea `l` (0..=n) para el inicio (`start`) o el fin de un área.
fn line_pos(starts: &[f32], ends: &[f32], l: usize, start: bool) -> f32 {
    let n = starts.len();
    if n == 0 {
        return 0.0;
    }
    if start {
        if l < n { starts[l] } else { ends[n - 1] }
    } else if l > 0 {
        ends[(l - 1).min(n - 1)]
    } else {
        starts[0]
    }
}

// ---------------------------------------------------------------------------------------------
// Alineación propia (§10.3 y §10.4)
// ---------------------------------------------------------------------------------------------

fn self_align(item: ItemAlign, container: ItemAlign, replaced: bool) -> ItemAlign {
    let a = if item == ItemAlign::Auto {
        container
    } else {
        item
    };
    match a {
        // `normal` es `stretch` salvo en los reemplazados (con relación de aspecto): `start`.
        ItemAlign::Auto | ItemAlign::Normal if replaced => ItemAlign::Start,
        ItemAlign::Auto | ItemAlign::Normal => ItemAlign::Stretch,
        a => a,
    }
}

fn align_offset(a: ItemAlign, free: f32) -> f32 {
    match a {
        ItemAlign::End | ItemAlign::FlexEnd | ItemAlign::SelfEnd => free,
        ItemAlign::Center => free / 2.0,
        _ => 0.0,
    }
}

/// Reparte el espacio libre `free` de un eje (área menos el margin box con los márgenes `auto`
/// a 0) entre los márgenes `auto` (§10.2) o con la alineación. Devuelve el desplazamiento del
/// margin box desde el inicio del área y los márgenes usados.
fn place_in_area(free: f32, m0: Option<f32>, m1: Option<f32>, align: ItemAlign) -> (f32, f32, f32) {
    match (m0, m1) {
        (None, None) => {
            let h = free.max(0.0) / 2.0;
            (0.0, h, h)
        }
        (None, Some(b)) => (0.0, free.max(0.0), b),
        (Some(a), None) => (0.0, a, free.max(0.0)),
        (Some(a), Some(b)) => (align_offset(align, free), a, b),
    }
}

fn is_replaced(b: &LayoutBox) -> bool {
    matches!(b.kind, BoxKind::Replaced { .. })
}

/// ¿El tamaño preferido "se comporta como auto" o depende del containing block (§6.6)?
fn pref_auto(s: &Size) -> bool {
    match s {
        Size::Auto => true,
        Size::LengthPercentage(lp) => lp.has_percentage(),
        _ => false,
    }
}

fn scrolls(o: Overflow) -> bool {
    !matches!(o, Overflow::Visible | Overflow::Clip)
}

/// ¿Se aplica el mínimo automático basado en el contenido (§6.6)? Si el ítem abarca alguna
/// pista con función mínima `auto` y, si abarca más de una, ninguna es flexible.
fn auto_min_applies(span: &[Track]) -> bool {
    span.iter().any(|t| t.min == Sizing::Auto)
        && (span.len() == 1 || span.iter().all(|t| t.flex().is_none()))
}

/// Suma de las funciones máximas fijas de un span más sus gaps, si todas son fijas (§6.6).
fn fixed_max_sum(tracks: &[Track], span: (usize, usize), gap: f32) -> Option<f32> {
    let mut sum = gaps_in(tracks, span, gap);
    for t in &tracks[span.0..span.1] {
        match t.max {
            Sizing::Fixed(f) => sum += f,
            _ => return None,
        }
    }
    Some(sum)
}

/// Desplazamiento de un elemento absoluto con los insets `auto` en un eje dentro de su área
/// (CSS Box Alignment 3 §6.1 y §6.2 para cajas absolutas): `f` está colocado al inicio del área.
fn abs_align_offset(
    b: &LayoutBox,
    container: &ComputedStyle,
    f: &Fragment,
    area: Rect,
) -> (f32, f32) {
    let s = &b.style;
    let auto = |v: &bf_style::properties::types::LengthPercentageAuto| {
        matches!(v, bf_style::properties::types::LengthPercentageAuto::Auto)
    };
    let m = f.margin_rect();
    let offset = |item: ItemAlign, cont: ItemAlign, free: f32| {
        let a = if item == ItemAlign::Auto { cont } else { item };
        align_offset(a, free)
    };
    let dx = if auto(&s.left) && auto(&s.right) {
        offset(
            s.justify_self,
            container.justify_items,
            area.width - m.width,
        )
    } else {
        0.0
    };
    let dy = if auto(&s.top) && auto(&s.bottom) {
        offset(s.align_self, container.align_items, area.height - m.height)
    } else {
        0.0
    };
    (dx, dy)
}

/// ¿Alguna pista de fila o el gap entre filas tiene porcentajes?
fn rows_have_percentages(s: &ComputedStyle) -> bool {
    fn pct(t: &TrackSize) -> bool {
        let b = |b: &TrackBreadth| matches!(b, TrackBreadth::Length(lp) if lp.has_percentage());
        match t {
            TrackSize::FitContent(lp) => lp.has_percentage(),
            t => b(t.min()) || b(t.max()),
        }
    }
    fn list(items: &[TrackListItem]) -> bool {
        items.iter().any(|i| match i {
            TrackListItem::Track(t) => pct(t),
            TrackListItem::Repeat(_, inner) => list(inner),
            TrackListItem::Names(_) => false,
        })
    }
    matches!(&s.grid_template_rows, GridTemplate::List(items) if list(items))
        || s.grid_auto_rows.0.iter().any(pct)
        || matches!(&s.row_gap, Gap::LengthPercentage(lp) if lp.has_percentage())
}

/// Un ítem en flujo ya colocado.
struct GridItem<'a> {
    b: &'a LayoutBox,
    /// Área en índices de pista: `[inicio, fin)` de columnas y filas.
    col: (usize, usize),
    row: (usize, usize),
}

/// Rejilla colocada de un contenedor.
struct Grid<'a> {
    items: Vec<GridItem<'a>>,
    cols: Vec<Track>,
    rows: Vec<Track>,
    abs: Vec<&'a LayoutBox>,
    col_axis: ExplicitAxis,
    row_axis: ExplicitAxis,
    /// Pistas implícitas antes de la línea 1 en cada eje.
    col_before: usize,
    row_before: usize,
}

/// Espacio para las repeticiones automáticas de las columnas: el ancho si es definido (o el del
/// contenedor en flujo), si no `max-width` y luego `min-width` (§7.2.3.2).
fn column_repeat_space(b: &LayoutBox, width: Option<f32>) -> RepeatSpace {
    let s = &b.style;
    let bp = border_edges(s).horizontal() + padding_edges(s, 0.0).horizontal();
    let adjust = |v: f32| {
        if s.box_sizing == BoxSizing::BorderBox {
            (v - bp).max(0.0)
        } else {
            v
        }
    };
    let shrink_to_fit = matches!(b.kind, BoxKind::InlineBlock) || b.is_out_of_flow();
    // Al calcular los tamaños intrínsecos (`width` = `None`) el espacio es indefinido aunque
    // el `width` sea fijo (como Blink): solo cuentan `max-width` y `min-width`.
    let size = match (&s.width, width) {
        (_, Some(w)) if !shrink_to_fit || !pref_auto(&s.width) => Some(w),
        _ => match &s.max_width {
            MaxSize::LengthPercentage(lp) if !lp.has_percentage() => Some(adjust(lp.resolve(0.0))),
            _ => None,
        },
    };
    let min = match &s.min_width {
        Size::LengthPercentage(lp) if !lp.has_percentage() => Some(adjust(lp.resolve(0.0))),
        _ => None,
    };
    RepeatSpace {
        size,
        min,
        basis: width,
    }
}

fn row_repeat_space(s: &ComputedStyle, height: Option<f32>, bp_v: f32) -> RepeatSpace {
    let (lo, hi) = height_limits(s, None, bp_v);
    RepeatSpace {
        size: height.or(hi.is_finite().then_some(hi)),
        min: (lo > 0.0).then_some(lo),
        basis: height,
    }
}

impl<'a> Layout<'a> {
    /// Número de cajas absolutas y fijas pendientes (para desplazar después las que registre
    /// una caja que se maqueta en un sitio y luego se mueve, como un float).
    pub(crate) fn pending_abs_marks(&self) -> (usize, usize) {
        (self.abs_frames.last().map_or(0, Vec::len), self.fixed.len())
    }

    /// Desplaza la posición estática de las cajas pendientes registradas desde `marks`.
    pub(crate) fn shift_pending_abs(&mut self, (abs, fixed): (usize, usize), dx: f32, dy: f32) {
        let frame = self.abs_frames.last_mut().map(|f| {
            let n = f.len();
            &mut f[abs.min(n)..]
        });
        let n = self.fixed.len();
        let fixed = &mut self.fixed[fixed.min(n)..];
        for p in frame.into_iter().flatten().chain(fixed.iter_mut()) {
            p.static_x += dx;
            p.static_y += dy;
        }
    }

    /// Rejilla explícita, colocación de los ítems (§8.5) y pistas sin dimensionar.
    fn grid_setup(
        &mut self,
        container: &'a LayoutBox,
        width: Option<f32>,
        height: Option<f32>,
    ) -> Grid<'a> {
        let cs = &container.style;
        let bp_v = border_edges(cs).vertical() + padding_edges(cs, width.unwrap_or(0.0)).vertical();
        let col_gap = gap_px(&cs.column_gap, width);
        let row_gap = gap_px(&cs.row_gap, height);
        let col_axis = explicit_axis(
            &cs.grid_template_columns,
            &cs.grid_template_areas,
            true,
            column_repeat_space(container, width),
            col_gap,
        );
        let row_axis = explicit_axis(
            &cs.grid_template_rows,
            &cs.grid_template_areas,
            false,
            row_repeat_space(cs, height, bp_v),
            row_gap,
        );
        // Ítems en orden modificado por `order` (§8.5 y CSS Display 3 §3).
        let mut children: Vec<&'a LayoutBox> = Vec::new();
        let mut abs = Vec::new();
        for c in &container.children {
            if c.is_absolutely_positioned() {
                abs.push(c);
            } else if !matches!(c.kind, BoxKind::Marker { .. })
                // La leyenda renderizada de un `<fieldset>` la coloca `box_contents`.
                && !self.legend.is_some_and(|l| std::ptr::eq(l, c))
            {
                children.push(c);
            }
        }
        children.sort_by_key(|c| c.style.order.0);
        let cols: Vec<Placement> = children
            .iter()
            .map(|c| {
                let s = &c.style;
                resolve_placement(&s.grid_column_start, &s.grid_column_end, &col_axis)
            })
            .collect();
        let rows: Vec<Placement> = children
            .iter()
            .map(|c| {
                let s = &c.style;
                resolve_placement(&s.grid_row_start, &s.grid_row_end, &row_axis)
            })
            .collect();
        // Pistas implícitas antes de la rejilla explícita.
        let before = |ps: &[Placement]| {
            ps.iter()
                .map(|p| match p {
                    Placement::Definite(a, _) => (-a).max(0),
                    Placement::Auto(_) => 0,
                })
                .max()
                .unwrap_or(0) as usize
        };
        let (col_before, row_before) = (before(&cols), before(&rows));
        let shift = |p: Placement, by: usize| match p {
            Placement::Definite(a, b) => Placement::Definite(a + by as i32, b + by as i32),
            p => p,
        };
        let cols: Vec<Placement> = cols.into_iter().map(|p| shift(p, col_before)).collect();
        let rows: Vec<Placement> = rows.into_iter().map(|p| shift(p, row_before)).collect();
        let flow = cs.grid_auto_flow;
        let areas: Vec<Area> = if flow.column {
            auto_place(&cols, &rows, row_before + row_axis.count, flow.dense)
        } else {
            auto_place(&rows, &cols, col_before + col_axis.count, flow.dense)
                .into_iter()
                .map(|(r, c)| (c, r))
                .collect()
        };
        let n_cols = areas
            .iter()
            .map(|(c, _)| c.1)
            .max()
            .unwrap_or(0)
            .max(col_before + col_axis.count);
        let n_rows = areas
            .iter()
            .map(|(_, r)| r.1)
            .max()
            .unwrap_or(0)
            .max(row_before + row_axis.count);
        let mut col_tracks = build_tracks(
            &col_axis,
            &cs.grid_auto_columns.0,
            col_before,
            n_cols,
            width,
        );
        let mut row_tracks =
            build_tracks(&row_axis, &cs.grid_auto_rows.0, row_before, n_rows, height);
        // §7.2.3.4: las pistas vacías de `auto-fit` se colapsan.
        for (tracks, ax, before, columns) in [
            (&mut col_tracks, &col_axis, col_before, true),
            (&mut row_tracks, &row_axis, row_before, false),
        ] {
            if let Some((s, e)) = ax.auto_fit {
                for (i, t) in tracks
                    .iter_mut()
                    .enumerate()
                    .take(before + e)
                    .skip(before + s)
                {
                    let used = areas.iter().any(|(c, r)| {
                        let span = if columns { c } else { r };
                        span.0 <= i && i < span.1
                    });
                    t.collapsed = !used;
                }
            }
        }
        let items = children
            .into_iter()
            .zip(areas)
            .map(|(b, (col, row))| GridItem { b, col, row })
            .collect();
        Grid {
            items,
            cols: col_tracks,
            rows: row_tracks,
            abs,
            col_axis,
            row_axis,
            col_before,
            row_before,
        }
    }

    /// `min-width` intrínseco (CSS Sizing 3 §3.1) de un ítem, como ancho de contenido.
    fn keyword_min_width(&mut self, b: &'a LayoutBox) -> Option<f32> {
        match &b.style.min_width {
            Size::MinContent | Size::FitContent => Some(self.intrinsic_widths(b).0),
            Size::MaxContent => Some(self.intrinsic_widths(b).1),
            _ => None,
        }
    }

    /// Contribuciones de un ítem en el eje de las columnas (§11.5 y §6.6).
    fn column_contrib(&mut self, it: &GridItem<'a>, tracks: &[Track], gap: f32) -> Contrib {
        let b = it.b;
        let s = &b.style;
        let (mut min_c, mut max_c) = self.outer_intrinsic_widths(b);
        let bp = border_edges(s).horizontal() + padding_edges(s, 0.0).horizontal();
        let margins = margin_value(&s.margin_left, 0.0).unwrap_or(0.0)
            + margin_value(&s.margin_right, 0.0).unwrap_or(0.0);
        // Reemplazados "comprimibles" (CSS Sizing 3 §5.2.2): un porcentaje en `width` o
        // `max-width` se resuelve contra 0 para la contribución min-content.
        let pct_w = matches!(&s.width, Size::LengthPercentage(lp) if lp.has_percentage());
        let pct_max = matches!(&s.max_width, MaxSize::LengthPercentage(lp) if lp.has_percentage());
        if is_replaced(b) && (pct_w || pct_max) {
            let (lo, _) = width_limits(s, 0.0, bp);
            min_c = min_c.min(lo + bp + margins);
        }
        // Un `min-width` intrínseco (`min-content`, `max-content`) es el suelo de las
        // contribuciones.
        if let Some(v) = self.keyword_min_width(b) {
            min_c = min_c.max(v + bp + margins);
            max_c = max_c.max(v + bp + margins);
        }
        let minimum = if !pref_auto(&s.width) {
            min_c
        } else {
            let adjust = |v: f32| {
                if s.box_sizing == BoxSizing::BorderBox {
                    (v - bp).max(0.0)
                } else {
                    v
                }
            };
            let used_min = match &s.min_width {
                Size::Auto => {
                    let span = &tracks[it.col.0..it.col.1];
                    if auto_min_applies(span) && !scrolls(s.overflow_x) {
                        // Mínimo automático basado en el contenido (§6.6).
                        let (_, hi) = width_limits(s, 0.0, bp);
                        let mut v = self.intrinsic_widths(b).0.min(hi);
                        if let Some(lim) = fixed_max_sum(tracks, it.col, gap) {
                            v = v.min((lim - margins - bp).max(0.0));
                        }
                        v
                    } else {
                        0.0
                    }
                }
                Size::LengthPercentage(lp) => adjust(lp.resolve(0.0)),
                Size::MinContent | Size::FitContent => self.intrinsic_widths(b).0,
                Size::MaxContent => self.intrinsic_widths(b).1,
            };
            used_min + bp + margins
        };
        Contrib {
            span: it.col,
            minimum,
            min_content: min_c,
            max_content: max_c,
        }
    }

    /// Ancho de contenido y márgenes horizontales (`None` = `auto`) de un ítem en un área de
    /// ancho `area_w` (§10.3 y CSS Box Alignment 3 §6.1).
    fn grid_item_width(
        &mut self,
        b: &'a LayoutBox,
        container: &ComputedStyle,
        area_w: f32,
        area_h: Option<f32>,
    ) -> (f32, Option<f32>, Option<f32>) {
        let s = &b.style;
        let bp = border_edges(s).horizontal() + padding_edges(s, area_w).horizontal();
        let ml = margin_value(&s.margin_left, area_w);
        let mr = margin_value(&s.margin_right, area_w);
        let avail = (area_w - ml.unwrap_or(0.0) - mr.unwrap_or(0.0) - bp).max(0.0);
        let align = self_align(s.justify_self, container.justify_items, is_replaced(b));
        let stretch = align == ItemAlign::Stretch && ml.is_some() && mr.is_some();
        let (lo, hi) = width_limits(s, area_w, bp);
        let lo = self.keyword_min_width(b).map_or(lo, |v| lo.max(v));
        if let Some(w) = self.table_fixed_width(b, avail, area_w) {
            // Una tabla con `width: auto` también se estira (no se ajusta a su contenido).
            let w = if stretch && matches!(s.width, Size::Auto) {
                w.max(avail)
            } else {
                w
            };
            return (w, ml, mr);
        }
        let w = if let Some(w) = specified_width(s, area_w, bp) {
            w
        } else if let BoxKind::Replaced { natural, ratio } = &b.kind {
            if stretch {
                avail
            } else {
                let cb = Cb {
                    x: 0.0,
                    y: 0.0,
                    width: area_w,
                    height: area_h,
                };
                return (self.replaced_size(b, *natural, *ratio, cb).width, ml, mr);
            }
        } else if stretch {
            avail
        } else if let Some(w) = self.control_size(b).0 {
            w
        } else {
            let (min, max) = self.intrinsic_widths(b);
            min.max(avail).min(max)
        };
        (w.min(hi).max(lo), ml, mr)
    }

    /// Maqueta un ítem con su border box en `(x, y)`, ancho de contenido `content_w` y, si se
    /// da, altura de contenido forzada.
    fn grid_item_fragment(
        &mut self,
        b: &'a LayoutBox,
        cb: Cb,
        (x, y): (f32, f32),
        content_w: f32,
        height: Option<f32>,
        margins: Edges,
    ) -> Fragment {
        // Un ítem establece un formato independiente (§6.1): sus márgenes no colapsan con los
        // de su contenido y contiene sus floats.
        self.bfc_items.push(b as *const LayoutBox as usize);
        self.forced_height = height;
        let mut scratch = FloatContext::default();
        let (f, _) = self.box_contents(b, cb, x, y, content_w, margins, &mut scratch, false);
        self.forced_height = None;
        self.bfc_items.pop();
        f
    }

    /// Contribuciones de un ítem en el eje de las filas: su altura maquetado con el ancho de su
    /// área (§12.1 paso 2), y su baseline relativa al borde superior del margin box.
    fn row_contrib(
        &mut self,
        it: &GridItem<'a>,
        container: &ComputedStyle,
        tracks: &[Track],
        gap: f32,
        area_w: f32,
    ) -> (Contrib, Option<f32>) {
        let b = it.b;
        let s = &b.style;
        let (w, ml, mr) = self.grid_item_width(b, container, area_w, None);
        let mt = margin_value(&s.margin_top, area_w).unwrap_or(0.0);
        let mb = margin_value(&s.margin_bottom, area_w).unwrap_or(0.0);
        let margins = Edges {
            top: mt,
            bottom: mb,
            left: ml.unwrap_or(0.0),
            right: mr.unwrap_or(0.0),
        };
        let cb = Cb {
            x: 0.0,
            y: 0.0,
            width: area_w,
            height: None,
        };
        let (h, baseline) = self.measuring(|this| {
            let f = this.grid_item_fragment(b, cb, (0.0, 0.0), w, None, margins);
            let baseline = crate::flex::first_baseline(&f).map(|y| y - f.rect.y + mt);
            (f.rect.height, baseline)
        });
        let outer = h + mt + mb;
        let bp_v = border_edges(s).vertical() + padding_edges(s, area_w).vertical();
        let minimum = if !pref_auto(&s.height) {
            outer
        } else {
            let used_min = match &s.min_height {
                Size::Auto => {
                    let span = &tracks[it.row.0..it.row.1];
                    if auto_min_applies(span) && !scrolls(s.overflow_y) {
                        let (_, hi) = height_limits(s, None, bp_v);
                        let mut v = (h - bp_v).max(0.0).min(hi);
                        if let Some(lim) = fixed_max_sum(tracks, it.row, gap) {
                            v = v.min((lim - mt - mb - bp_v).max(0.0));
                        }
                        v
                    } else {
                        0.0
                    }
                }
                _ => height_limits(s, None, bp_v).0,
            };
            (used_min + bp_v + mt + mb).min(outer)
        };
        (
            Contrib {
                span: it.row,
                minimum,
                min_content: outer,
                max_content: outer,
            },
            baseline,
        )
    }

    /// Layout del contenido de un contenedor grid: fragmentos de los ítems (en orden modificado
    /// por `order`) y altura del contenido. `cb` es el content box del contenedor (`height` si
    /// es definida).
    pub(crate) fn grid_layout(&mut self, container: &'a LayoutBox, cb: Cb) -> (Vec<Fragment>, f32) {
        let cs = &container.style;
        let mut grid = self.grid_setup(container, Some(cb.width), cb.height);
        let col_gap = gap_px(&cs.column_gap, Some(cb.width));
        let row_gap = gap_px(&cs.row_gap, cb.height);
        let stretch_content =
            |a: ContentAlign| matches!(a, ContentAlign::Normal | ContentAlign::Stretch);

        // --- Columnas (§12.1 paso 1) ---
        let contribs: Vec<Contrib> = grid
            .items
            .iter()
            .map(|it| self.column_contrib(it, &grid.cols, col_gap))
            .collect();
        size_tracks(
            &mut grid.cols,
            &contribs,
            col_gap,
            Space::Definite(cb.width),
            (cb.width, cb.width),
            stretch_content(cs.justify_content),
        );
        let (col_starts, col_ends) =
            track_positions(&grid.cols, col_gap, cb.width, cs.justify_content);
        let area_w = |it: &GridItem<'_>| {
            (line_pos(&col_starts, &col_ends, it.col.1, false)
                - line_pos(&col_starts, &col_ends, it.col.0, true))
            .max(0.0)
        };

        // --- Filas (§12.1 paso 2) ---
        let bp_v = border_edges(cs).vertical() + padding_edges(cs, cb.width).vertical();
        let (hmin, hmax) = height_limits(cs, None, bp_v);
        let mut row_contribs = Vec::with_capacity(grid.items.len());
        let mut baselines = Vec::with_capacity(grid.items.len());
        for it in &grid.items {
            let (c, bl) = self.row_contrib(it, cs, &grid.rows, row_gap, area_w(it));
            row_contribs.push(c);
            baselines.push(bl);
        }
        // Alineación por baseline en las filas (§10.6, simplificada): los ítems de span 1 con
        // `align-self: baseline` de una fila comparten baseline; su contribución incluye el
        // desplazamiento.
        let baseline_aligned = |it: &GridItem<'_>| {
            self_align(it.b.style.align_self, cs.align_items, is_replaced(it.b))
                == ItemAlign::Baseline
                && it.row.1 - it.row.0 == 1
        };
        let mut shims = vec![0.0f32; grid.items.len()];
        {
            let mut groups: std::collections::HashMap<usize, f32> =
                std::collections::HashMap::new();
            for (i, it) in grid.items.iter().enumerate() {
                if baseline_aligned(it) {
                    let bl = baselines[i].unwrap_or(row_contribs[i].max_content);
                    let e = groups.entry(it.row.0).or_insert(0.0);
                    *e = e.max(bl);
                }
            }
            for (i, it) in grid.items.iter().enumerate() {
                if baseline_aligned(it)
                    && let Some(&max) = groups.get(&it.row.0)
                {
                    let bl = baselines[i].unwrap_or(row_contribs[i].max_content);
                    shims[i] = max - bl;
                    let c = &mut row_contribs[i];
                    c.min_content += shims[i];
                    c.max_content += shims[i];
                    c.minimum += shims[i];
                }
            }
        }
        let row_space = match cb.height {
            Some(h) => Space::Definite(h),
            None => Space::MaxContent,
        };
        let limits = match cb.height {
            Some(h) => (h, h),
            None => (hmin, hmax),
        };
        size_tracks(
            &mut grid.rows,
            &row_contribs,
            row_gap,
            row_space,
            limits,
            stretch_content(cs.align_content),
        );
        let rows_total = total_size(&grid.rows, row_gap);
        let content_h = cb.height.unwrap_or_else(|| rows_total.min(hmax).max(hmin));
        // §7.2.1 y §10.1: con altura indefinida, las pistas y el gap con porcentaje se trataron
        // como `auto`/0 para la altura del contenedor; se resuelven contra ella y se vuelven a
        // dimensionar las filas.
        // Lo mismo con las pistas flexibles: con la altura ya conocida se vuelve a buscar el
        // tamaño del `fr` (§12.7.1 con espacio definido).
        let rerun = rows_have_percentages(cs) || grid.rows.iter().any(|t| t.flex().is_some());
        let row_gap = if cb.height.is_none() && rerun {
            let collapsed: Vec<bool> = grid.rows.iter().map(|t| t.collapsed).collect();
            grid.rows = build_tracks(
                &grid.row_axis,
                &cs.grid_auto_rows.0,
                grid.row_before,
                grid.rows.len(),
                Some(content_h),
            );
            for (t, c) in grid.rows.iter_mut().zip(collapsed) {
                t.collapsed = c;
            }
            let row_gap = gap_px(&cs.row_gap, Some(content_h));
            size_tracks(
                &mut grid.rows,
                &row_contribs,
                row_gap,
                Space::Definite(content_h),
                (content_h, content_h),
                stretch_content(cs.align_content),
            );
            row_gap
        } else {
            row_gap
        };
        let (row_starts, row_ends) =
            track_positions(&grid.rows, row_gap, content_h, cs.align_content);

        // --- Layout final y alineación de los ítems (§10.3 y §10.4) ---
        let mut out = Vec::with_capacity(grid.items.len() + grid.abs.len());
        let mut first: Option<(usize, usize, usize)> = None;
        for (i, it) in grid.items.iter().enumerate() {
            let b = it.b;
            let s = &b.style;
            let x0 = line_pos(&col_starts, &col_ends, it.col.0, true);
            let aw = area_w(it);
            let y0 = line_pos(&row_starts, &row_ends, it.row.0, true);
            let ah = (line_pos(&row_starts, &row_ends, it.row.1, false) - y0).max(0.0);
            let (w, ml, mr) = self.grid_item_width(b, cs, aw, Some(ah));
            let bp_h = border_edges(s).horizontal() + padding_edges(s, aw).horizontal();
            let bp_v = border_edges(s).vertical() + padding_edges(s, aw).vertical();
            let justify = self_align(s.justify_self, cs.justify_items, is_replaced(b));
            let free_h = aw - w - bp_h - ml.unwrap_or(0.0) - mr.unwrap_or(0.0);
            let (dx, ml, mr) = place_in_area(free_h, ml, mr, justify);
            let mt = margin_value(&s.margin_top, aw);
            let mb = margin_value(&s.margin_bottom, aw);
            let align = self_align(s.align_self, cs.align_items, is_replaced(b));
            let item_cb = Cb {
                x: cb.x + x0,
                y: cb.y + y0,
                width: aw,
                height: Some(ah),
            };
            // Estiramiento en el eje de bloque: alto forzado al del área (§10.4).
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
            let x = cb.x + x0 + dx + ml;
            let y = cb.y + y0 + mt.unwrap_or(0.0);
            let mut f = self.grid_item_fragment(b, item_cb, (x, y), w, forced, margins);
            // §6.4: los ítems se pintan como inline-blocks (cada uno atómico), en orden
            // modificado por `order`.
            if f.kind == FragmentKind::Block {
                f.kind = FragmentKind::InlineBlock;
            }
            let free_v = ah - f.rect.height - mt.unwrap_or(0.0) - mb.unwrap_or(0.0);
            let (dy, mt_used, mb_used) = if baseline_aligned(it) && mt.is_some() && mb.is_some() {
                (shims[i], mt.unwrap_or(0.0), mb.unwrap_or(0.0))
            } else {
                place_in_area(free_v, mt, mb, align)
            };
            let shift = dy + mt_used - mt.unwrap_or(0.0);
            if shift != 0.0 {
                f.translate(0.0, shift);
            }
            f.margin.top = mt_used;
            f.margin.bottom = mb_used;
            // Primer ítem (en orden de rejilla) de la primera fila con ítems.
            let key = (it.row.0, it.col.0, i);
            if first.is_none_or(|k| key < k) {
                first = Some(key);
            }
            out.push(f);
        }
        let baseline = first.map(|(_, _, i)| {
            let f = &out[i];
            crate::flex::first_baseline(f).unwrap_or(f.rect.bottom())
        });

        // --- Elementos absolutos (§9) ---
        let positioned = is_positioned(cs);
        let pad = padding_edges(cs, cb.width);
        let content_box = Rect::new(cb.x, cb.y, cb.width, content_h);
        let padding_box = Rect::new(
            cb.x - pad.left,
            cb.y - pad.top,
            cb.width + pad.horizontal(),
            content_h + pad.vertical(),
        );
        // §9.1: el containing block es el área de rejilla; las líneas `auto` (o fuera de la
        // rejilla implícita) son los bordes del padding box.
        let abs_area = |s: &ComputedStyle| {
            let to_index = |l: Option<i32>, before: usize, n: usize| {
                l.map(|l| l + before as i32)
                    .filter(|&l| l >= 0 && l as usize <= n)
                    .map(|l| l as usize)
            };
            let (nc, nr) = (grid.cols.len(), grid.rows.len());
            let (c0, c1) =
                resolve_abs_placement(&s.grid_column_start, &s.grid_column_end, &grid.col_axis);
            let (r0, r1) =
                resolve_abs_placement(&s.grid_row_start, &s.grid_row_end, &grid.row_axis);
            let left = to_index(c0, grid.col_before, nc).map_or(padding_box.x, |l| {
                cb.x + line_pos(&col_starts, &col_ends, l, true)
            });
            let right = to_index(c1, grid.col_before, nc).map_or(padding_box.right(), |l| {
                cb.x + line_pos(&col_starts, &col_ends, l, false)
            });
            let top = to_index(r0, grid.row_before, nr).map_or(padding_box.y, |l| {
                cb.y + line_pos(&row_starts, &row_ends, l, true)
            });
            let bottom = to_index(r1, grid.row_before, nr).map_or(padding_box.bottom(), |l| {
                cb.y + line_pos(&row_starts, &row_ends, l, false)
            });
            Rect::new(left, top, (right - left).max(0.0), (bottom - top).max(0.0))
        };
        // Descendientes absolutos (no hijos) cuyo containing block es este contenedor: también
        // usan su área de rejilla, con su posición estática.
        let descendants = if positioned {
            self.abs_frames
                .last_mut()
                .map(std::mem::take)
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        for p in descendants {
            let area = abs_area(&p.b.style);
            let f = self.absolute(p, area);
            out.push(f);
        }
        for &c in &grid.abs {
            let s = &c.style;
            if !positioned || s.position == Position::Fixed {
                // §9.2: la posición estática es la de un único ítem en un área con los bordes
                // del content box, alineado con `justify-self`/`align-self`.
                let f = self.measuring(|this| {
                    let p = PendingAbs {
                        b: c,
                        static_x: content_box.x,
                        static_y: content_box.y,
                    };
                    this.absolute(p, content_box)
                });
                let (dx, dy) = abs_align_offset(c, cs, &f, content_box);
                self.add_abs(c, content_box.x + dx, content_box.y + dy);
                continue;
            }
            let area = abs_area(s);
            let mut f = self.absolute(
                PendingAbs {
                    b: c,
                    static_x: area.x,
                    static_y: area.y,
                },
                area,
            );
            let (dx, dy) = abs_align_offset(c, cs, &f, area);
            f.translate(dx, dy);
            out.push(f);
        }
        self.grid_baseline = baseline;
        (out, content_h)
    }

    /// Anchos intrínsecos (min-content, max-content) de un contenedor grid: el algoritmo de
    /// §12.1 con el espacio disponible bajo una restricción min-content o max-content.
    pub(crate) fn grid_intrinsic_widths(&mut self, b: &'a LayoutBox) -> (f32, f32) {
        let cs = &b.style;
        let grid = self.grid_setup(b, None, None);
        let gap = gap_px(&cs.column_gap, None);
        let contribs: Vec<Contrib> = grid
            .items
            .iter()
            .map(|it| self.column_contrib(it, &grid.cols, gap))
            .collect();
        let mut sizes = [0.0f32; 2];
        for (k, space) in [Space::MinContent, Space::MaxContent]
            .into_iter()
            .enumerate()
        {
            let mut tracks = grid.cols.clone();
            size_tracks(
                &mut tracks,
                &contribs,
                gap,
                space,
                (0.0, f32::INFINITY),
                false,
            );
            sizes[k] = total_size(&tracks, gap);
        }
        (sizes[0], sizes[1].max(sizes[0]))
    }
}
