//! Tablas (CSS 2.2 §17, con CSS Tables 3 donde CSS 2.2 deja el comportamiento indefinido).
//!
//! La caja envoltorio (§17.4) es un block container (`display: table | inline-table`) con los
//! títulos y la caja de tabla ([`BoxKind::Table`]) como hijos block-level; su ancho es el de la
//! caja de tabla. La caja de tabla tiene como hijos grupos de columnas y grupos de filas ya
//! normalizados por `box_tree` (§17.2.1).
//!
//! - La rejilla (`rowspan`/`colspan`, columnas de `<col>`) y los bordes colapsados (§17.6.2) se
//!   resuelven al construir el árbol de cajas ([`finish_table`]): en el modelo colapsado las
//!   celdas y la tabla reciben como ancho de borde la mitad del borde ganador.
//! - Anchos: `table-layout: fixed` según §17.5.2.1; el automático con el algoritmo de
//!   distribución de CSS Tables 3 §3.9.3 (las "guesses" min-content, min-content-percentage,
//!   min-content-specified y max-content, con interpolación lineal entre ellas).
//! - Alturas: §17.5.3 con `vertical-align` `baseline | top | middle | bottom` (los demás
//!   valores se tratan como `baseline`); el exceso de las celdas con `rowspan` y de la altura
//!   especificada de la tabla se reparte proporcionalmente entre las filas.
//!
//! Interpretaciones:
//! - La baseline de una fila sin celdas alineadas por baseline es el borde inferior del
//!   contenido de la celda más baja (§17.5.3).
//! - `visibility: collapse` en filas y columnas las quita (alto/ancho 0) sin recalcular el resto.
//! - Las celdas con `rowspan` que salen de su grupo se recortan al final del grupo (HTML
//!   §4.9.12).
//! - Las celdas sin line boxes no participan en la alineación por baseline (se alinean
//!   arriba); su baseline cuenta solo para la de la fila.
//! - `height` en una celda es solo un mínimo para la fila: el contenido se alinea con su altura
//!   natural (§17.5.3).
//! - En el modelo colapsado, los bordes laterales de la tabla son la mitad del máximo de todas
//!   las filas (CSS Tables 3), no solo de la primera (CSS 2.2).
//! - En `table-layout: fixed` una celda mide exactamente su columna aunque su padding y borde
//!   no quepan.
//! - Una tabla con `width` en longitud conserva su ancho dentro de un envoltorio más ancho
//!   (ítem flex/grid estirado).

use std::rc::Rc;
use std::sync::Arc;

use bf_style::ComputedStyle;
use bf_style::properties::types::{
    BorderCollapse, BorderStyle, BorderWidth, BoxSizing, Display, EmptyCells, Image, MaxSize,
    NonNegative, Size, TableLayout, VerticalAlign, VerticalAlignKeyword, Visibility,
};
use bf_style::values::color::Rgba;
use bf_style::values::{Length, LengthPercentage};

use crate::block::{Cb, Layout, border_edges, padding_edges};
use crate::box_tree::{BoxKind, LayoutBox};
use crate::float::FloatContext;
use crate::fragment::{
    BorderSegment, CellPaint, Edges, Fragment, FragmentKind, Rect, TablePaint, TableRole,
};

/// Una celda en la rejilla. `group`, `row` y `child` son índices en los hijos de la tabla, del
/// grupo y de la fila.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridCell {
    pub group: usize,
    pub row_child: usize,
    pub child: usize,
    pub row: usize,
    pub col: usize,
    pub rowspan: usize,
    pub colspan: usize,
}

/// Un borde colapsado ganador.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeBorder {
    pub width: f32,
    pub style: BorderStyle,
    pub color: Rgba,
}

impl EdgeBorder {
    const NONE: EdgeBorder = EdgeBorder {
        width: 0.0,
        style: BorderStyle::None,
        color: Rgba::TRANSPARENT,
    };

    fn visible_width(&self) -> f32 {
        if matches!(self.style, BorderStyle::None | BorderStyle::Hidden) {
            0.0
        } else {
            self.width
        }
    }
}

/// Bordes colapsados: `horizontal[r][c]` es el borde sobre la fila `r` en la columna `c`
/// (`r` hasta `filas`), `vertical[r][c]` el borde a la izquierda de la columna `c` en la fila
/// `r` (`c` hasta `columnas`).
#[derive(Debug, Clone, PartialEq)]
pub struct CollapsedBorders {
    pub horizontal: Vec<Vec<EdgeBorder>>,
    pub vertical: Vec<Vec<EdgeBorder>>,
}

/// La rejilla de una tabla (HTML §4.9.12, "forming a table", con las cajas CSS).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TableGrid {
    pub columns: usize,
    /// Grupos de filas en orden visual: (índice en los hijos de la tabla, primera fila, fin).
    pub groups: Vec<(usize, usize, usize)>,
    /// Filas en orden visual: (grupo en los hijos de la tabla, índice en el grupo).
    pub rows: Vec<(usize, usize)>,
    /// Celdas en orden visual (filas y, dentro de cada fila, orden de árbol).
    pub cells: Vec<GridCell>,
    /// Por columna: (grupo de columnas en los hijos de la tabla, columna en el grupo).
    pub column_boxes: Vec<(usize, Option<usize>)>,
    pub collapsed: Option<CollapsedBorders>,
}

fn is_row_group(d: Display) -> bool {
    matches!(
        d,
        Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup
    )
}

/// ¿Es `b` la caja envoltorio de una tabla?
pub(crate) fn is_table_wrapper(b: &LayoutBox) -> bool {
    matches!(b.style.display, Display::Table | Display::InlineTable)
        && matches!(
            b.kind,
            BoxKind::Block | BoxKind::AnonymousBlock | BoxKind::InlineBlock
        )
        && b.children.iter().any(|c| matches!(c.kind, BoxKind::Table))
}

fn table_child(wrapper: &LayoutBox) -> Option<&LayoutBox> {
    wrapper
        .children
        .iter()
        .find(|c| matches!(c.kind, BoxKind::Table))
}

// --- Construcción (desde box_tree) ---

/// Arma la rejilla de la caja de tabla `t` y, en el modelo colapsado, resuelve los bordes y
/// ajusta los anchos de borde de las celdas y de la tabla.
pub(crate) fn finish_table(t: &mut LayoutBox) {
    let mut grid = TableGrid::default();
    // Columnas (§17.3): cada `col` cuenta `span` veces; un grupo sin columnas, `span` veces.
    for (gi, g) in t.children.iter().enumerate() {
        if g.style.display != Display::TableColumnGroup {
            continue;
        }
        if g.children.is_empty() {
            for _ in 0..g.span.0 {
                grid.column_boxes.push((gi, None));
            }
        }
        for (ci, c) in g.children.iter().enumerate() {
            for _ in 0..c.span.0 {
                grid.column_boxes.push((gi, Some(ci)));
            }
        }
    }
    // Grupos en orden visual: el primer `table-header-group` arriba y el primer
    // `table-footer-group` abajo (§17.2).
    let groups: Vec<usize> = (0..t.children.len())
        .filter(|&i| is_row_group(t.children[i].style.display))
        .collect();
    let header = groups
        .iter()
        .copied()
        .find(|&i| t.children[i].style.display == Display::TableHeaderGroup);
    let footer = groups
        .iter()
        .copied()
        .find(|&i| t.children[i].style.display == Display::TableFooterGroup);
    let mut order: Vec<usize> = header.into_iter().collect();
    order.extend(
        groups
            .iter()
            .copied()
            .filter(|&i| Some(i) != header && Some(i) != footer),
    );
    order.extend(footer);
    // Ocupación de la rejilla.
    let mut occupied: Vec<Vec<bool>> = Vec::new();
    let mut columns = grid.column_boxes.len();
    for &gi in &order {
        let g = &t.children[gi];
        let first = grid.rows.len();
        let n_rows = g.children.len();
        for (ri, row) in g.children.iter().enumerate() {
            let r = first + ri;
            grid.rows.push((gi, ri));
            if occupied.len() <= r {
                occupied.resize(r + 1, Vec::new());
            }
            let mut c = 0;
            for (ci, cell) in row.children.iter().enumerate() {
                while occupied[r].get(c).copied().unwrap_or(false) {
                    c += 1;
                }
                let colspan = cell.span.0.max(1) as usize;
                // rowspan 0: hasta el final del grupo; nunca más allá (HTML §4.9.12).
                let rowspan = match cell.span.1 {
                    0 => n_rows - ri,
                    n => (n as usize).min(n_rows - ri),
                }
                .max(1);
                for rr in r..r + rowspan {
                    if occupied.len() <= rr {
                        occupied.resize(rr + 1, Vec::new());
                    }
                    if occupied[rr].len() < c + colspan {
                        occupied[rr].resize(c + colspan, false);
                    }
                    for slot in &mut occupied[rr][c..c + colspan] {
                        *slot = true;
                    }
                }
                grid.cells.push(GridCell {
                    group: gi,
                    row_child: ri,
                    child: ci,
                    row: r,
                    col: c,
                    rowspan,
                    colspan,
                });
                columns = columns.max(c + colspan);
                c += colspan;
            }
        }
        grid.groups.push((gi, first, grid.rows.len()));
    }
    grid.columns = columns;
    if t.style.border_collapse == BorderCollapse::Collapse {
        let borders = resolve_collapsed(t, &grid);
        apply_collapsed(t, &grid, &borders);
        grid.collapsed = Some(borders);
    }
    t.table = Some(Box::new(grid));
}

/// Prioridad de los estilos de borde en el conflicto (§17.6.2.1, regla 3).
fn style_rank(s: BorderStyle) -> u8 {
    match s {
        BorderStyle::Double => 8,
        BorderStyle::Solid => 7,
        BorderStyle::Dashed => 6,
        BorderStyle::Dotted => 5,
        BorderStyle::Ridge => 4,
        BorderStyle::Outset => 3,
        BorderStyle::Groove => 2,
        BorderStyle::Inset => 1,
        _ => 0,
    }
}

/// ¿Gana `new` (de un elemento de rango `new_rank`) sobre `cur`? (§17.6.2.1). En empate total
/// gana el que ya estaba (el de más arriba o más a la izquierda, que se aplica antes).
fn wins(new: &EdgeBorder, new_rank: u8, cur: &(EdgeBorder, u8)) -> bool {
    let (cur, cur_rank) = (&cur.0, cur.1);
    if cur.style == BorderStyle::Hidden {
        return false;
    }
    if new.style == BorderStyle::Hidden {
        return true;
    }
    if new.style == BorderStyle::None {
        return false;
    }
    if cur.style == BorderStyle::None {
        return true;
    }
    if new.width != cur.width {
        return new.width > cur.width;
    }
    if style_rank(new.style) != style_rank(cur.style) {
        return style_rank(new.style) > style_rank(cur.style);
    }
    new_rank > cur_rank
}

/// Bordes (arriba, derecha, abajo, izquierda) de un estilo.
fn sides(s: &ComputedStyle) -> [EdgeBorder; 4] {
    let current = s.color.rgba();
    let e = |w: BorderWidth, st: BorderStyle, c: bf_style::values::Color| EdgeBorder {
        width: w.px(),
        style: st,
        color: c.resolve(current),
    };
    [
        e(s.border_top_width, s.border_top_style, s.border_top_color),
        e(
            s.border_right_width,
            s.border_right_style,
            s.border_right_color,
        ),
        e(
            s.border_bottom_width,
            s.border_bottom_style,
            s.border_bottom_color,
        ),
        e(
            s.border_left_width,
            s.border_left_style,
            s.border_left_color,
        ),
    ]
}

const RANK_CELL: u8 = 5;
const RANK_ROW: u8 = 4;
const RANK_ROW_GROUP: u8 = 3;
const RANK_COLUMN: u8 = 2;
const RANK_COLUMN_GROUP: u8 = 1;
const RANK_TABLE: u8 = 0;

/// Filas y columnas con `visibility: collapse` (propia o de su grupo).
fn hidden_tracks(t: &LayoutBox, grid: &TableGrid) -> (Vec<bool>, Vec<bool>) {
    let rows = grid
        .rows
        .iter()
        .map(|&(gi, ri)| {
            is_collapsed(&t.children[gi].style) || is_collapsed(&t.children[gi].children[ri].style)
        })
        .collect();
    let mut cols = vec![false; grid.columns];
    for (c, &(gi, ci)) in grid.column_boxes.iter().enumerate() {
        let g = &t.children[gi];
        cols[c] =
            is_collapsed(&g.style) || ci.is_some_and(|ci| is_collapsed(&g.children[ci].style));
    }
    (rows, cols)
}

/// Resuelve los conflictos de bordes de la tabla `t` (§17.6.2.1). Las filas y columnas con
/// `visibility: collapse` no participan: el conflicto se resuelve en la rejilla sin ellas
/// (CSS Tables 3 §3.6.2) y el resultado se reparte a los bordes originales.
fn resolve_collapsed(t: &LayoutBox, grid: &TableGrid) -> CollapsedBorders {
    let (hidden_rows, hidden_cols) = hidden_tracks(t, grid);
    if !hidden_rows.contains(&true) && !hidden_cols.contains(&true) {
        return resolve_grid(t, grid);
    }
    // Índice comprimido de cada línea: cuántas pistas visibles hay antes.
    let map = |hidden: &[bool]| {
        let mut m = Vec::with_capacity(hidden.len() + 1);
        let mut n = 0;
        for &h in hidden {
            m.push(n);
            if !h {
                n += 1;
            }
        }
        m.push(n);
        m
    };
    let (rmap, cmap) = (map(&hidden_rows), map(&hidden_cols));
    let mut cg = TableGrid {
        columns: cmap[grid.columns],
        ..TableGrid::default()
    };
    cg.rows = grid
        .rows
        .iter()
        .zip(&hidden_rows)
        .filter(|(_, h)| !**h)
        .map(|(r, _)| *r)
        .collect();
    cg.groups = grid
        .groups
        .iter()
        .map(|&(gi, a, b)| (gi, rmap[a], rmap[b]))
        .collect();
    cg.column_boxes = grid
        .column_boxes
        .iter()
        .zip(&hidden_cols)
        .filter(|(_, h)| !**h)
        .map(|(c, _)| *c)
        .collect();
    for c in &grid.cells {
        let (r0, r1) = (rmap[c.row], rmap[c.row + c.rowspan]);
        let (c0, c1) = (cmap[c.col], cmap[c.col + c.colspan]);
        if r0 == r1 || c0 == c1 {
            continue;
        }
        cg.cells.push(GridCell {
            row: r0,
            rowspan: r1 - r0,
            col: c0,
            colspan: c1 - c0,
            ..c.clone()
        });
    }
    let compressed = resolve_grid(t, &cg);
    let rows = grid.rows.len();
    let cols = grid.columns;
    CollapsedBorders {
        horizontal: (0..=rows)
            .map(|r| {
                (0..cols)
                    .map(|c| {
                        if hidden_cols[c] {
                            EdgeBorder::NONE
                        } else {
                            compressed.horizontal[rmap[r]][cmap[c]]
                        }
                    })
                    .collect()
            })
            .collect(),
        vertical: (0..rows)
            .map(|r| {
                (0..=cols)
                    .map(|c| {
                        if hidden_rows[r] {
                            EdgeBorder::NONE
                        } else {
                            compressed.vertical[rmap[r]][cmap[c]]
                        }
                    })
                    .collect()
            })
            .collect(),
    }
}

/// [`resolve_collapsed`] sobre una rejilla sin pistas colapsadas.
fn resolve_grid(t: &LayoutBox, grid: &TableGrid) -> CollapsedBorders {
    let (rows, cols) = (grid.rows.len(), grid.columns);
    let empty = (EdgeBorder::NONE, 0u8);
    let mut h = vec![vec![empty; cols]; rows + 1];
    let mut v = vec![vec![empty; cols + 1]; rows];
    let apply = |slot: &mut (EdgeBorder, u8), b: EdgeBorder, rank: u8| {
        if wins(&b, rank, slot) {
            *slot = (b, rank);
        }
    };
    let apply_h = |h: &mut [Vec<(EdgeBorder, u8)>],
                   r: usize,
                   range: std::ops::Range<usize>,
                   b: EdgeBorder,
                   rank: u8| {
        for slot in &mut h[r][range] {
            apply(slot, b, rank);
        }
    };
    // Celdas (de arriba abajo y de izquierda a derecha).
    for c in &grid.cells {
        let cell = &t.children[c.group].children[c.row_child].children[c.child];
        let [top, right, bottom, left] = sides(&cell.style);
        let (r0, r1, c0, c1) = (c.row, c.row + c.rowspan, c.col, c.col + c.colspan);
        apply_h(&mut h, r0, c0..c1, top, RANK_CELL);
        apply_h(&mut h, r1, c0..c1, bottom, RANK_CELL);
        for row in v.iter_mut().take(r1).skip(r0) {
            apply(&mut row[c0], left, RANK_CELL);
            apply(&mut row[c1], right, RANK_CELL);
        }
    }
    // Filas.
    for (r, &(gi, ri)) in grid.rows.iter().enumerate() {
        let [top, right, bottom, left] = sides(&t.children[gi].children[ri].style);
        apply_h(&mut h, r, 0..cols, top, RANK_ROW);
        apply_h(&mut h, r + 1, 0..cols, bottom, RANK_ROW);
        apply(&mut v[r][0], left, RANK_ROW);
        apply(&mut v[r][cols], right, RANK_ROW);
    }
    // Grupos de filas.
    for &(gi, first, end) in &grid.groups {
        let [top, right, bottom, left] = sides(&t.children[gi].style);
        apply_h(&mut h, first, 0..cols, top, RANK_ROW_GROUP);
        apply_h(&mut h, end, 0..cols, bottom, RANK_ROW_GROUP);
        for row in v.iter_mut().take(end).skip(first) {
            apply(&mut row[0], left, RANK_ROW_GROUP);
            apply(&mut row[cols], right, RANK_ROW_GROUP);
        }
    }
    // Columnas y grupos de columnas.
    for (c, &(gi, ci)) in grid.column_boxes.iter().enumerate() {
        let Some(ci) = ci else {
            continue;
        };
        let [top, right, bottom, left] = sides(&t.children[gi].children[ci].style);
        for row in v.iter_mut() {
            apply(&mut row[c], left, RANK_COLUMN);
            apply(&mut row[c + 1], right, RANK_COLUMN);
        }
        apply(&mut h[0][c], top, RANK_COLUMN);
        apply(&mut h[rows][c], bottom, RANK_COLUMN);
    }
    let mut c = 0;
    while c < grid.column_boxes.len() {
        let gi = grid.column_boxes[c].0;
        let end = (c..grid.column_boxes.len())
            .find(|&k| grid.column_boxes[k].0 != gi)
            .unwrap_or(grid.column_boxes.len());
        let [top, right, bottom, left] = sides(&t.children[gi].style);
        for row in v.iter_mut() {
            apply(&mut row[c], left, RANK_COLUMN_GROUP);
            apply(&mut row[end], right, RANK_COLUMN_GROUP);
        }
        apply_h(&mut h, 0, c..end, top, RANK_COLUMN_GROUP);
        apply_h(&mut h, rows, c..end, bottom, RANK_COLUMN_GROUP);
        c = end;
    }
    // Tabla.
    let [top, right, bottom, left] = sides(&t.style);
    apply_h(&mut h, 0, 0..cols, top, RANK_TABLE);
    apply_h(&mut h, rows, 0..cols, bottom, RANK_TABLE);
    for row in v.iter_mut() {
        apply(&mut row[0], left, RANK_TABLE);
        apply(&mut row[cols], right, RANK_TABLE);
    }
    // Los bordes interiores de las celdas con span no existen.
    for c in &grid.cells {
        for row in h.iter_mut().take(c.row + c.rowspan).skip(c.row + 1) {
            for e in &mut row[c.col..c.col + c.colspan] {
                *e = empty;
            }
        }
        for row in v.iter_mut().take(c.row + c.rowspan).skip(c.row) {
            for e in &mut row[c.col + 1..c.col + c.colspan] {
                *e = empty;
            }
        }
    }
    let finish = |e: (EdgeBorder, u8)| {
        let mut b = e.0;
        if matches!(b.style, BorderStyle::None | BorderStyle::Hidden) {
            b = EdgeBorder::NONE;
        }
        b
    };
    CollapsedBorders {
        horizontal: h
            .into_iter()
            .map(|r| r.into_iter().map(finish).collect())
            .collect(),
        vertical: v
            .into_iter()
            .map(|r| r.into_iter().map(finish).collect())
            .collect(),
    }
}

/// Estilo con los anchos de borde `w` (arriba, derecha, abajo, izquierda) y padding opcional
/// en 0: los bordes colapsados se pintan aparte, así que el estilo solo fija la geometría.
fn with_borders(s: &ComputedStyle, w: [f32; 4], zero_padding: bool) -> Arc<ComputedStyle> {
    let mut s = s.clone();
    let bw = |v: f32| BorderWidth(Length::px(v));
    let st = |v: f32| {
        if v > 0.0 {
            BorderStyle::Solid
        } else {
            BorderStyle::None
        }
    };
    s.border_top_width = bw(w[0]);
    s.border_right_width = bw(w[1]);
    s.border_bottom_width = bw(w[2]);
    s.border_left_width = bw(w[3]);
    s.border_top_style = st(w[0]);
    s.border_right_style = st(w[1]);
    s.border_bottom_style = st(w[2]);
    s.border_left_style = st(w[3]);
    if zero_padding {
        let z = || NonNegative(LengthPercentage::px(0.0));
        s.padding_top = z();
        s.padding_right = z();
        s.padding_bottom = z();
        s.padding_left = z();
    }
    Arc::new(s)
}

/// Mitades de los bordes colapsados como bordes de las celdas y de la tabla (§17.6.2).
fn apply_collapsed(t: &mut LayoutBox, grid: &TableGrid, b: &CollapsedBorders) {
    let max = |it: &mut dyn Iterator<Item = f32>| it.fold(0.0f32, f32::max);
    for c in &grid.cells {
        let (r0, r1, c0, c1) = (c.row, c.row + c.rowspan, c.col, c.col + c.colspan);
        let top = max(&mut b.horizontal[r0][c0..c1]
            .iter()
            .map(EdgeBorder::visible_width));
        let bottom = max(&mut b.horizontal[r1][c0..c1]
            .iter()
            .map(EdgeBorder::visible_width));
        let left = max(&mut b.vertical[r0..r1].iter().map(|r| r[c0].visible_width()));
        let right = max(&mut b.vertical[r0..r1].iter().map(|r| r[c1].visible_width()));
        let cell = &mut t.children[c.group].children[c.row_child].children[c.child];
        cell.style = with_borders(
            &cell.style,
            [top / 2.0, right / 2.0, bottom / 2.0, left / 2.0],
            false,
        );
    }
    let (rows, cols) = (grid.rows.len(), grid.columns);
    if rows == 0 || cols == 0 {
        t.style = with_borders(&t.style, [0.0; 4], true);
        return;
    }
    // La tabla: la mitad del máximo de los bordes de cada lado. CSS 2.2 §17.6.2 toma los
    // laterales de la primera fila y deja que el resto desborde al margen; CSS Tables 3 y los
    // navegadores usan el máximo de todas las filas (css/CSS2/tables/border-collapse-006).
    let top = max(&mut b.horizontal[0].iter().map(EdgeBorder::visible_width));
    let bottom = max(&mut b.horizontal[rows].iter().map(EdgeBorder::visible_width));
    let left = max(&mut b.vertical.iter().map(|r| r[0].visible_width()));
    let right = max(&mut b.vertical.iter().map(|r| r[cols].visible_width()));
    t.style = with_borders(
        &t.style,
        [top / 2.0, right / 2.0, bottom / 2.0, left / 2.0],
        true,
    );
}

// --- Layout ---

/// Anchos intrínsecos de las columnas (border box de las celdas, sin espaciado).
#[derive(Debug, Clone, Default)]
pub struct ColumnInfo {
    pub min: Vec<f32>,
    pub max: Vec<f32>,
    /// Porcentaje (0 si no tiene).
    pub percent: Vec<f32>,
    /// ¿Tiene un ancho especificado en longitud?
    pub fixed: Vec<bool>,
}

/// Ancho especificado de la border box de `s` (`Ok(px)`), un porcentaje (`Err(%)`) o nada.
fn specified_outer(s: &ComputedStyle, bp: f32) -> Option<Result<f32, f32>> {
    let Size::LengthPercentage(lp) = &s.width else {
        return None;
    };
    match lp {
        LengthPercentage::Percentage(p) => Some(Err(*p)),
        // `calc()` con porcentajes: cuenta como su parte porcentual (CSS Tables 3,
        // csswg-drafts#94 y #3482).
        lp if lp.has_percentage() => {
            let p = lp.resolve(100.0) - lp.resolve(0.0);
            (p > 0.0).then_some(Err(p))
        }
        lp => {
            let w = lp.resolve(0.0);
            Some(Ok(if s.box_sizing == BoxSizing::BorderBox {
                w.max(bp)
            } else {
                w + bp
            }))
        }
    }
}

fn has_background(s: &ComputedStyle) -> bool {
    s.background_color.resolve(s.color.rgba()).a > 0.0
        || s.background_image.0.iter().any(|i| *i != Image::None)
}

fn is_collapsed(s: &ComputedStyle) -> bool {
    s.visibility == Visibility::Collapse
}

/// Alineación vertical de una celda.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CellAlign {
    Baseline,
    Top,
    Middle,
    Bottom,
}

fn cell_align(s: &ComputedStyle) -> CellAlign {
    match s.vertical_align {
        VerticalAlign::Keyword(VerticalAlignKeyword::Top) => CellAlign::Top,
        VerticalAlign::Keyword(VerticalAlignKeyword::Middle) => CellAlign::Middle,
        VerticalAlign::Keyword(VerticalAlignKeyword::Bottom) => CellAlign::Bottom,
        _ => CellAlign::Baseline,
    }
}

/// Reparte `extra` entre `sizes[range]` en proporción a `weights` (o en partes iguales si
/// todos son 0).
fn distribute(sizes: &mut [f32], weights: &[f32], extra: f32) {
    if sizes.is_empty() || extra <= 0.0 {
        return;
    }
    let total: f32 = weights.iter().sum();
    let n = sizes.len() as f32;
    for (s, w) in sizes.iter_mut().zip(weights) {
        *s += if total > 0.0 {
            extra * w / total
        } else {
            extra / n
        };
    }
}

impl<'a> Layout<'a> {
    fn cell_box(t: &'a LayoutBox, c: &GridCell) -> &'a LayoutBox {
        &t.children[c.group].children[c.row_child].children[c.child]
    }

    /// Espaciado (horizontal, vertical) entre celdas.
    fn spacing(t: &LayoutBox) -> (f32, f32) {
        if t.style.border_collapse == BorderCollapse::Collapse {
            (0.0, 0.0)
        } else {
            t.style.border_spacing.px()
        }
    }

    /// Espaciado horizontal total de `cols` columnas (incluidos los bordes de la tabla).
    fn h_spacing_total(t: &LayoutBox, cols: usize) -> f32 {
        if cols == 0 {
            0.0
        } else {
            Self::spacing(t).0 * (cols as f32 + 1.0)
        }
    }

    /// Anchos intrínsecos de las columnas de la caja de tabla `t` (CSS Tables 3 §3.8).
    pub(crate) fn table_columns(&mut self, t: &'a LayoutBox) -> Rc<ColumnInfo> {
        let key = t as *const LayoutBox as usize;
        if let Some(info) = self.table_cache.get(&key) {
            return info.clone();
        }
        let info = Rc::new(self.compute_columns(t));
        self.table_cache.insert(key, info.clone());
        info
    }

    fn compute_columns(&mut self, t: &'a LayoutBox) -> ColumnInfo {
        let Some(grid) = t.table.as_deref() else {
            return ColumnInfo::default();
        };
        let n = grid.columns;
        let (hsp, _) = Self::spacing(t);
        let mut info = ColumnInfo {
            min: vec![0.0; n],
            max: vec![0.0; n],
            percent: vec![0.0; n],
            fixed: vec![false; n],
        };
        // Anchos de `col` y `colgroup` (el del grupo vale para sus columnas sin ancho).
        for (c, &(gi, ci)) in grid.column_boxes.iter().enumerate() {
            let group = &t.children[gi];
            let spec = ci
                .and_then(|ci| specified_outer(&group.children[ci].style, 0.0))
                .or_else(|| specified_outer(&group.style, 0.0));
            match spec {
                Some(Ok(w)) => {
                    info.max[c] = info.max[c].max(w);
                    info.fixed[c] = true;
                }
                Some(Err(p)) => info.percent[c] = info.percent[c].max(p),
                None => {}
            }
        }
        // Celdas, de menor a mayor colspan.
        let mut cells: Vec<&GridCell> = grid.cells.iter().collect();
        cells.sort_by_key(|c| c.colspan);
        for c in cells {
            let cell = Self::cell_box(t, c);
            let s = &cell.style;
            let bp = border_edges(s).horizontal() + padding_edges(s, 0.0).horizontal();
            let (imin, imax) = self.intrinsic_widths(cell);
            let mut cmin = imin + bp;
            let mut cmax = imax + bp;
            let mut fixed = false;
            let mut pct = 0.0;
            match specified_outer(s, bp) {
                Some(Ok(w)) => {
                    cmax = cmin.max(w);
                    fixed = true;
                }
                Some(Err(p)) => pct = p,
                None => {}
            }
            // min-width en celdas (CSS Tables 3 §3.8.2).
            if let Size::LengthPercentage(lp) = &s.min_width
                && !lp.has_percentage()
            {
                cmin = cmin.max(lp.resolve(0.0) + bp);
            }
            if let MaxSize::LengthPercentage(lp) = &s.max_width
                && !lp.has_percentage()
                && !fixed
            {
                cmax = cmax.min((lp.resolve(0.0) + bp).max(cmin));
            }
            cmax = cmax.max(cmin);
            let range = c.col..c.col + c.colspan;
            if c.colspan == 1 {
                let k = c.col;
                info.min[k] = info.min[k].max(cmin);
                info.max[k] = info.max[k].max(cmax);
                info.fixed[k] |= fixed;
                info.percent[k] = info.percent[k].max(pct);
                continue;
            }
            // Celdas con colspan: el exceso se reparte según el max-content de las columnas.
            let gaps = hsp * (c.colspan as f32 - 1.0);
            let weights: Vec<f32> = info.max[range.clone()].to_vec();
            let cur_min: f32 = info.min[range.clone()].iter().sum::<f32>() + gaps;
            if cmin > cur_min {
                distribute(&mut info.min[range.clone()], &weights, cmin - cur_min);
            }
            let cur_max: f32 = info.max[range.clone()].iter().sum::<f32>() + gaps;
            if cmax > cur_max {
                distribute(&mut info.max[range.clone()], &weights, cmax - cur_max);
            }
            let cur_pct: f32 = info.percent[range.clone()].iter().sum();
            if pct > cur_pct {
                let without: Vec<usize> =
                    range.clone().filter(|&k| info.percent[k] == 0.0).collect();
                let w: f32 = without.iter().map(|&k| info.max[k]).sum();
                for &k in &without {
                    let share = if w > 0.0 {
                        info.max[k] / w
                    } else {
                        1.0 / without.len() as f32
                    };
                    info.percent[k] = (pct - cur_pct) * share;
                }
            }
        }
        for k in 0..n {
            info.max[k] = info.max[k].max(info.min[k]);
        }
        // Las columnas colapsadas no ocupan ancho (CSS Tables 3 §3.6.2).
        let (_, hidden_cols) = hidden_tracks(t, grid);
        for (k, _) in hidden_cols.iter().enumerate().filter(|(_, h)| **h) {
            info.min[k] = 0.0;
            info.max[k] = 0.0;
            info.percent[k] = 0.0;
            info.fixed[k] = false;
        }
        info
    }

    /// (min, max) del contenido de la caja de tabla: columnas más espaciado.
    fn table_content_range(&mut self, t: &'a LayoutBox) -> (f32, f32) {
        let info = self.table_columns(t);
        let n = info.min.len();
        let spacing = Self::h_spacing_total(t, n);
        let min: f32 = info.min.iter().sum::<f32>() + spacing;
        let mut max: f32 = info.max.iter().sum::<f32>() + spacing;
        // Las columnas con porcentaje necesitan que la tabla sea lo bastante ancha
        // (CSS Tables 3 §3.9.2).
        let total_pct: f32 = info.percent.iter().sum::<f32>().min(100.0);
        if total_pct > 0.0 {
            let non_pct: f32 = (0..n)
                .filter(|&k| info.percent[k] == 0.0)
                .map(|k| info.max[k])
                .sum();
            if total_pct < 100.0 {
                max = max.max(non_pct / (1.0 - total_pct / 100.0) + spacing);
            }
            for k in 0..n {
                if info.percent[k] > 0.0 {
                    max = max.max(info.max[k] * 100.0 / info.percent[k] + spacing);
                }
            }
        }
        (min, max.max(min))
    }

    /// Ancho de la border box de la caja de tabla dentro de su envoltorio: `available` es el
    /// espacio disponible y `cb_width` la base de los porcentajes (§17.5.2).
    fn table_border_width(&mut self, wrapper: &'a LayoutBox, available: f32, cb_width: f32) -> f32 {
        let Some(t) = table_child(wrapper) else {
            return 0.0;
        };
        let s = &t.style;
        let bp = border_edges(s).horizontal() + padding_edges(s, cb_width).horizontal();
        let spec = match &s.width {
            Size::LengthPercentage(lp) => {
                let w = lp.resolve(cb_width);
                Some(if s.box_sizing == BoxSizing::BorderBox {
                    w
                } else {
                    w + bp
                })
            }
            _ => None,
        };
        // Los títulos: la tabla es al menos tan ancha como su min-content.
        let mut caption_min = 0.0f32;
        for c in wrapper
            .children
            .iter()
            .filter(|c| !matches!(c.kind, BoxKind::Table))
        {
            caption_min = caption_min.max(self.outer_intrinsic_widths(c).0);
        }
        if s.table_layout == TableLayout::Fixed
            && let Some(w) = spec
        {
            let cols = self.fixed_columns(t, (w - bp).max(0.0));
            let total: f32 = cols.iter().sum::<f32>() + Self::h_spacing_total(t, cols.len());
            return w.max(total + bp).max(caption_min);
        }
        let (min, max) = self.table_content_range(t);
        let min_bb = (min + bp).max(caption_min);
        let mut w = match spec {
            Some(w) => w.max(min_bb),
            None => available.max(min_bb).min((max + bp).max(min_bb)),
        };
        if let Size::LengthPercentage(lp) = &s.min_width {
            let m = lp.resolve(cb_width);
            w = w.max(if s.box_sizing == BoxSizing::BorderBox {
                m
            } else {
                m + bp
            });
        }
        w
    }

    /// Ancho fijo de una caja de tabla o de su envoltorio (`None` para el resto): para el
    /// envoltorio, el de la tabla; para la caja de tabla, su content box dentro del envoltorio
    /// (`available` es el ancho del envoltorio).
    pub(crate) fn table_fixed_width(
        &mut self,
        b: &'a LayoutBox,
        available: f32,
        cb_width: f32,
    ) -> Option<f32> {
        if matches!(b.kind, BoxKind::Table) {
            let s = &b.style;
            let bp = border_edges(s).horizontal() + padding_edges(s, cb_width).horizontal();
            // Un ancho en longitud se respeta aunque el envoltorio sea más ancho (ítem flex o
            // grid estirado); si no, la tabla ocupa el envoltorio.
            if let Size::LengthPercentage(lp) = &s.width
                && !lp.has_percentage()
                && s.table_layout != TableLayout::Fixed
            {
                let w = lp.resolve(0.0);
                let spec = if s.box_sizing == BoxSizing::BorderBox {
                    w
                } else {
                    w + bp
                };
                let (min, _) = self.table_content_range(b);
                return Some((spec.max(min + bp).min(available.max(min + bp)) - bp).max(0.0));
            }
            return Some((available - bp).max(0.0));
        }
        if !is_table_wrapper(b) {
            return None;
        }
        Some(self.table_border_width(b, available, cb_width))
    }

    /// Anchos intrínsecos de una caja de tabla o de su envoltorio.
    pub(crate) fn table_intrinsic_widths(&mut self, b: &'a LayoutBox) -> Option<(f32, f32)> {
        if matches!(b.kind, BoxKind::Table) {
            return Some(self.table_content_range(b));
        }
        if !is_table_wrapper(b) {
            return None;
        }
        let t = table_child(b)?;
        let s = &t.style;
        let bp = border_edges(s).horizontal() + padding_edges(s, 0.0).horizontal();
        let mut caption_min = 0.0f32;
        for c in b
            .children
            .iter()
            .filter(|c| !matches!(c.kind, BoxKind::Table))
        {
            caption_min = caption_min.max(self.outer_intrinsic_widths(c).0);
        }
        let spec = match &s.width {
            Size::LengthPercentage(lp) if !lp.has_percentage() => {
                let w = lp.resolve(0.0);
                Some(if s.box_sizing == BoxSizing::BorderBox {
                    w
                } else {
                    w + bp
                })
            }
            _ => None,
        };
        if s.table_layout == TableLayout::Fixed
            && let Some(w) = spec
        {
            let w = w.max(caption_min);
            return Some((w, w));
        }
        let (min, max) = self.table_content_range(t);
        let min = (min + bp).max(caption_min);
        let max = (max + bp).max(min);
        Some(match spec {
            Some(w) => (w.max(min), w.max(min)),
            None => (min, max),
        })
    }

    /// Anchos de columna (border box) de `table-layout: fixed` (§17.5.2.1) para un ancho de
    /// contenido `width`.
    fn fixed_columns(&mut self, t: &'a LayoutBox, width: f32) -> Vec<f32> {
        let Some(grid) = t.table.as_deref() else {
            return Vec::new();
        };
        let n = grid.columns;
        let avail = (width - Self::h_spacing_total(t, n)).max(0.0);
        let (hsp, _) = Self::spacing(t);
        let mut cols: Vec<Option<f32>> = vec![None; n];
        let resolve = |s: &ComputedStyle, bp: f32| match specified_outer(s, bp) {
            Some(Ok(w)) => Some(w),
            Some(Err(p)) if s.box_sizing == BoxSizing::BorderBox => Some(p / 100.0 * avail),
            Some(Err(p)) => Some(p / 100.0 * avail + bp),
            None => None,
        };
        for (c, &(gi, ci)) in grid.column_boxes.iter().enumerate() {
            let group = &t.children[gi];
            cols[c] = ci
                .and_then(|ci| resolve(&group.children[ci].style, 0.0))
                .or_else(|| resolve(&group.style, 0.0));
        }
        for c in grid.cells.iter().filter(|c| c.row == 0) {
            if cols[c.col..c.col + c.colspan].iter().any(Option::is_some) {
                continue;
            }
            let s = &Self::cell_box(t, c).style;
            let bp = border_edges(s).horizontal() + padding_edges(s, avail).horizontal();
            if let Some(w) = resolve(s, bp) {
                let each = (w - hsp * (c.colspan as f32 - 1.0)).max(0.0) / c.colspan as f32;
                for slot in &mut cols[c.col..c.col + c.colspan] {
                    *slot = Some(each);
                }
            }
        }
        let assigned: f32 = cols.iter().flatten().sum();
        let free = cols.iter().filter(|c| c.is_none()).count();
        let rest = (avail - assigned).max(0.0);
        let mut out: Vec<f32> = cols
            .iter()
            .map(|c| c.unwrap_or(if free > 0 { rest / free as f32 } else { 0.0 }))
            .collect();
        // Sin columnas libres, el sobrante se reparte en proporción a los anchos.
        if free == 0 && rest > 0.0 {
            let weights = out.clone();
            distribute(&mut out, &weights, rest);
        }
        out
    }

    /// Distribución del ancho `target` (sin espaciado) entre las columnas (CSS Tables 3
    /// §3.9.3).
    fn auto_columns(&mut self, t: &'a LayoutBox, target: f32) -> Vec<f32> {
        let info = self.table_columns(t);
        let n = info.min.len();
        let pct_w = |k: usize| info.min[k].max(info.percent[k] / 100.0 * target);
        let is_pct = |k: usize| info.percent[k] > 0.0;
        let g0: Vec<f32> = info.min.clone();
        let g1: Vec<f32> = (0..n)
            .map(|k| if is_pct(k) { pct_w(k) } else { info.min[k] })
            .collect();
        let g2: Vec<f32> = (0..n)
            .map(|k| {
                if is_pct(k) {
                    pct_w(k)
                } else if info.fixed[k] {
                    info.max[k]
                } else {
                    info.min[k]
                }
            })
            .collect();
        let g3: Vec<f32> = (0..n)
            .map(|k| if is_pct(k) { pct_w(k) } else { info.max[k] })
            .collect();
        let sum = |g: &[f32]| g.iter().sum::<f32>();
        if target <= sum(&g0) {
            return g0;
        }
        for (a, b) in [(&g0, &g1), (&g1, &g2), (&g2, &g3)] {
            let (sa, sb) = (sum(a), sum(b));
            if target <= sb {
                let f = if sb > sa {
                    (target - sa) / (sb - sa)
                } else {
                    1.0
                };
                return a.iter().zip(b).map(|(x, y)| x + (y - x) * f).collect();
            }
        }
        // Más ancho que el max-content: el exceso va a las columnas automáticas, luego a las
        // de ancho fijo y por último a las de porcentaje.
        let mut out = g3;
        let extra = target - sum(&out);
        let auto: Vec<usize> = (0..n).filter(|&k| !is_pct(k) && !info.fixed[k]).collect();
        let fixed: Vec<usize> = (0..n).filter(|&k| !is_pct(k) && info.fixed[k]).collect();
        let pct: Vec<usize> = (0..n).filter(|&k| is_pct(k)).collect();
        let (set, weights): (Vec<usize>, Vec<f32>) = if !auto.is_empty() {
            let w: Vec<f32> = auto.iter().map(|&k| info.max[k]).collect();
            (auto, w)
        } else if !fixed.is_empty() {
            let w: Vec<f32> = fixed.iter().map(|&k| info.max[k]).collect();
            (fixed, w)
        } else if !pct.is_empty() {
            let w: Vec<f32> = pct.iter().map(|&k| info.percent[k]).collect();
            (pct, w)
        } else {
            ((0..n).collect(), vec![0.0; n])
        };
        let mut sizes: Vec<f32> = set.iter().map(|&k| out[k]).collect();
        distribute(&mut sizes, &weights, extra);
        for (i, &k) in set.iter().enumerate() {
            out[k] = sizes[i];
        }
        out
    }

    /// Maqueta una celda con su border box en `(x, y)` y ancho `width`.
    fn layout_cell(
        &mut self,
        cell: &'a LayoutBox,
        cb: Cb,
        (x, y): (f32, f32),
        width: f32,
    ) -> Fragment {
        let s = &cell.style;
        let bp = border_edges(s).horizontal() + padding_edges(s, cb.width).horizontal();
        let mut scratch = FloatContext::default();
        let (f, _) = self.box_contents(
            cell,
            cb,
            x,
            y,
            (width - bp).max(0.0),
            Edges::default(),
            &mut scratch,
            false,
        );
        f
    }

    /// Layout del contenido de la caja de tabla `t`: grupos de filas (con filas y celdas) y
    /// altura del contenido. `cb` es el content box de la tabla.
    pub(crate) fn table_layout(&mut self, t: &'a LayoutBox, cb: Cb) -> (Vec<Fragment>, f32) {
        let Some(grid) = t.table.as_deref() else {
            return (Vec::new(), 0.0);
        };
        let collapse = t.style.border_collapse == BorderCollapse::Collapse;
        let (hsp, vsp) = Self::spacing(t);
        let ncols = grid.columns;
        let nrows = grid.rows.len();
        // --- Anchos de columna ---
        let spec_width = !matches!(t.style.width, Size::Auto);
        let mut widths = if t.style.table_layout == TableLayout::Fixed && spec_width {
            self.fixed_columns(t, cb.width)
        } else {
            let target = (cb.width - Self::h_spacing_total(t, ncols)).max(0.0);
            self.auto_columns(t, target)
        };
        // Columnas con `visibility: collapse`: sin ancho ni espaciado (CSS Tables 3 §3.6.2).
        let mut col_hidden = vec![false; ncols];
        for (c, &(gi, ci)) in grid.column_boxes.iter().enumerate() {
            let g = &t.children[gi];
            if is_collapsed(&g.style) || ci.is_some_and(|ci| is_collapsed(&g.children[ci].style)) {
                widths[c] = 0.0;
                col_hidden[c] = true;
            }
        }
        let mut col_x = Vec::with_capacity(ncols + 1);
        let mut x = cb.x + if ncols > 0 { hsp } else { 0.0 };
        for (&w, &hidden) in widths.iter().zip(&col_hidden) {
            col_x.push(x);
            if !hidden {
                x += w + hsp;
            }
        }
        col_x.push(x - if ncols > 0 { hsp } else { 0.0 });
        let span_width = |c0: usize, cs: usize| {
            let last = c0 + cs - 1;
            (col_x[last] + widths[last] - col_x[c0]).max(0.0)
        };
        // --- Medición de las celdas (§17.5.3) ---
        let cell_cb = Cb {
            x: cb.x,
            y: cb.y,
            width: cb.width,
            height: None,
        };
        struct Measure {
            height: f32,
            /// Alto mínimo de la fila por el `height` de la celda (border box).
            specified: f32,
            /// Distancia del borde superior a la baseline (la de su primera línea; sin líneas,
            /// no participa en la alineación por baseline y se alinea arriba).
            baseline: Option<f32>,
            align: CellAlign,
        }
        let mut measures: Vec<Measure> = Vec::with_capacity(grid.cells.len());
        for c in &grid.cells {
            let cell = Self::cell_box(t, c);
            let w = span_width(c.col, c.colspan);
            let m = self.measuring(|this| {
                let f = this.layout_cell(cell, cell_cb, (0.0, 0.0), w);
                let baseline = crate::flex::first_baseline(&f).map(|b| b - f.rect.y);
                let s = &cell.style;
                let edges_v = border_edges(s).vertical() + padding_edges(s, cb.width).vertical();
                // Los porcentajes no se resuelven contra la altura de la tabla.
                let specified =
                    crate::block::specified_height(s, None, edges_v).map_or(0.0, |h| h + edges_v);
                Measure {
                    height: f.rect.height,
                    specified,
                    baseline,
                    align: cell_align(&cell.style),
                }
            });
            measures.push(m);
        }
        // --- Alturas de las filas ---
        let row_style = |r: usize| {
            let (gi, ri) = grid.rows[r];
            &t.children[gi].children[ri].style
        };
        let mut heights = vec![0.0f32; nrows];
        let mut baselines: Vec<Option<f32>> = vec![None; nrows];
        for (r, h) in heights.iter_mut().enumerate() {
            if let Size::LengthPercentage(lp) = &row_style(r).height
                && (!lp.has_percentage() || cb.height.is_some())
            {
                *h = lp.resolve(cb.height.unwrap_or(0.0));
            }
        }
        for (c, m) in grid.cells.iter().zip(&measures) {
            if m.align == CellAlign::Baseline
                && let Some(mb) = m.baseline
            {
                let b = &mut baselines[c.row];
                *b = Some(b.map_or(mb, |v: f32| v.max(mb)));
            }
        }
        let needed = |c: &GridCell, m: &Measure| {
            let h = match (m.align, baselines[c.row], m.baseline) {
                (CellAlign::Baseline, Some(b), Some(mb)) => b - mb + m.height,
                _ => m.height,
            };
            h.max(m.specified)
        };
        for (c, m) in grid.cells.iter().zip(&measures) {
            if c.rowspan == 1 {
                heights[c.row] = heights[c.row].max(needed(c, m));
            }
        }
        let mut spanning: Vec<(&GridCell, &Measure)> = grid
            .cells
            .iter()
            .zip(&measures)
            .filter(|(c, _)| c.rowspan > 1)
            .collect();
        spanning.sort_by_key(|(c, _)| c.rowspan);
        for (c, m) in spanning {
            let range = c.row..c.row + c.rowspan;
            let cur = heights[range.clone()].iter().sum::<f32>() + vsp * (c.rowspan as f32 - 1.0);
            let need = needed(c, m);
            if need > cur {
                let weights = heights[range.clone()].to_vec();
                distribute(&mut heights[range], &weights, need - cur);
            }
        }
        for (r, h) in heights.iter_mut().enumerate() {
            let (gi, _) = grid.rows[r];
            if is_collapsed(row_style(r)) || is_collapsed(&t.children[gi].style) {
                *h = 0.0;
            }
        }
        // Las filas con `visibility: collapse` tampoco aportan espaciado.
        let row_hidden: Vec<bool> = (0..nrows)
            .map(|r| is_collapsed(row_style(r)) || is_collapsed(&t.children[grid.rows[r].0].style))
            .collect();
        let visible_rows = row_hidden.iter().filter(|h| !**h).count();
        let v_total = |heights: &[f32]| {
            heights.iter().sum::<f32>()
                + if visible_rows > 0 {
                    vsp * (visible_rows as f32 + 1.0)
                } else {
                    0.0
                }
        };
        // Altura especificada de la tabla: el exceso se reparte entre las filas.
        if let Some(h) = cb.height
            && nrows > 0
        {
            let total = v_total(&heights);
            if h > total {
                let weights = heights.clone();
                distribute(&mut heights, &weights, h - total);
            }
        }
        let mut row_y = Vec::with_capacity(nrows + 1);
        let mut y = cb.y + if visible_rows > 0 { vsp } else { 0.0 };
        for (&h, &hidden) in heights.iter().zip(&row_hidden) {
            row_y.push(y);
            if !hidden {
                y += h + vsp;
            }
        }
        row_y.push(y - if visible_rows > 0 { vsp } else { 0.0 });
        let content_h = v_total(&heights);
        // --- Layout final de las celdas ---
        let x0 = col_x.first().copied().unwrap_or(cb.x);
        let x1 = col_x.last().copied().unwrap_or(cb.x);
        let mut row_frags: Vec<Vec<Fragment>> = vec![Vec::new(); nrows];
        let mut lowest: Vec<f32> = vec![f32::NEG_INFINITY; nrows];
        for (c, m) in grid.cells.iter().zip(&measures) {
            if heights[c.row..c.row + c.rowspan].iter().all(|&h| h == 0.0)
                && is_collapsed(row_style(c.row))
            {
                continue;
            }
            let cell = Self::cell_box(t, c);
            let (x, w) = (col_x[c.col], span_width(c.col, c.colspan));
            let top = row_y[c.row];
            let h = heights[c.row..c.row + c.rowspan].iter().sum::<f32>()
                + vsp * (c.rowspan as f32 - 1.0);
            let offset = match m.align {
                CellAlign::Top => 0.0,
                CellAlign::Middle => ((h - m.height) / 2.0).max(0.0),
                CellAlign::Bottom => (h - m.height).max(0.0),
                CellAlign::Baseline => match (baselines[c.row], m.baseline) {
                    (Some(b), Some(mb)) => (b - mb).max(0.0),
                    _ => 0.0,
                },
            };
            let mut f = self.layout_cell(cell, cell_cb, (x, top + offset), w);
            // La celda mide su columna aunque su padding y borde no quepan (columnas de
            // `table-layout: fixed`).
            f.rect.width = w;
            f.rect.y -= offset;
            f.rect.height = h.max(m.height);
            // Sin celdas alineadas por baseline, la de la fila es el borde inferior del
            // contenido de la celda más baja (§17.5.3).
            if c.rowspan == 1 {
                lowest[c.row] = lowest[c.row].max(f.content_rect().bottom());
            }
            if f.clip.is_some() {
                f.clip = Some(f.padding_rect());
            }
            // Capas de la tabla detrás de la celda (§17.5.1).
            let mut layers: Vec<(Arc<ComputedStyle>, Rect)> = Vec::new();
            if let Some(&(gi, ci)) = grid.column_boxes.get(c.col) {
                let g = &t.children[gi];
                let first = grid
                    .column_boxes
                    .iter()
                    .position(|b| b.0 == gi)
                    .unwrap_or(c.col);
                let last = grid
                    .column_boxes
                    .iter()
                    .rposition(|b| b.0 == gi)
                    .unwrap_or(c.col);
                let (gx, gx1) = (col_x[first], col_x[last] + widths[last]);
                let (ty, ty1) = (row_y[0], row_y[nrows]);
                if has_background(&g.style) {
                    layers.push((g.style.clone(), Rect::new(gx, ty, gx1 - gx, ty1 - ty)));
                }
                if let Some(ci) = ci
                    && has_background(&g.children[ci].style)
                {
                    let k0 = grid
                        .column_boxes
                        .iter()
                        .position(|b| *b == (gi, Some(ci)))
                        .unwrap_or(c.col);
                    let k1 = grid
                        .column_boxes
                        .iter()
                        .rposition(|b| *b == (gi, Some(ci)))
                        .unwrap_or(c.col);
                    let (cx, cx1) = (col_x[k0], col_x[k1] + widths[k1]);
                    layers.push((
                        g.children[ci].style.clone(),
                        Rect::new(cx, ty, cx1 - cx, ty1 - ty),
                    ));
                }
            }
            if let Some(&(gi, first, end)) = grid.groups.iter().find(|g| g.0 == c.group) {
                let g = &t.children[gi];
                if has_background(&g.style) && end > first {
                    let gy = row_y[first];
                    let gy1 = row_y[end - 1] + heights[end - 1];
                    layers.push((g.style.clone(), Rect::new(x0, gy, x1 - x0, gy1 - gy)));
                }
            }
            let rs = row_style(c.row);
            if has_background(rs) {
                layers.push((
                    Arc::clone(rs),
                    Rect::new(x0, row_y[c.row], x1 - x0, heights[c.row]),
                ));
            }
            let hidden =
                !collapse && cell.style.empty_cells == EmptyCells::Hide && cell.children.is_empty();
            f.table = Some(TableRole::Cell(Box::new(CellPaint { layers, hidden })));
            row_frags[c.row].push(f);
        }
        // --- Fragmentos de filas y grupos ---
        let mut out = Vec::new();
        for &(gi, first, end) in &grid.groups {
            let g = &t.children[gi];
            let gy = row_y[first];
            let gy1 = if end > first {
                row_y[end - 1] + heights[end - 1]
            } else {
                gy
            };
            let mut gf = Fragment::new(
                FragmentKind::Block,
                Rect::new(x0, gy, x1 - x0, gy1 - gy),
                g.style.clone(),
            );
            gf.node = g.node;
            gf.anonymous = g.node.is_none();
            gf.table = Some(TableRole::Part);
            for r in first..end {
                let (_, ri) = grid.rows[r];
                let row = &g.children[ri];
                let mut rf = Fragment::new(
                    FragmentKind::Block,
                    Rect::new(x0, row_y[r], x1 - x0, heights[r]),
                    row.style.clone(),
                );
                rf.node = row.node;
                rf.anonymous = row.node.is_none();
                rf.table = Some(TableRole::Part);
                rf.baseline = Some(match baselines[r] {
                    Some(b) => row_y[r] + b,
                    None if lowest[r].is_finite() => lowest[r],
                    None => row_y[r] + heights[r],
                });
                rf.children = std::mem::take(&mut row_frags[r]);
                gf.children.push(rf);
            }
            out.push(gf);
        }
        // --- Bordes colapsados ---
        let collapsed = grid.collapsed.as_ref().map(|b| {
            let mut segs = Vec::new();
            for (r, row) in b.vertical.iter().enumerate() {
                for (c, e) in row.iter().enumerate() {
                    if e.visible_width() <= 0.0 || heights[r] <= 0.0 {
                        continue;
                    }
                    let x = if c == ncols { x1 } else { col_x[c] };
                    segs.push(BorderSegment {
                        rect: Rect::new(x - e.width / 2.0, row_y[r], e.width, heights[r]),
                        horizontal: false,
                        style: e.style,
                        color: e.color,
                    });
                }
            }
            for (r, row) in b.horizontal.iter().enumerate() {
                for (c, e) in row.iter().enumerate() {
                    if e.visible_width() <= 0.0 || widths[c] <= 0.0 {
                        continue;
                    }
                    // Las esquinas: el tramo horizontal se extiende la mitad del vertical más
                    // ancho de cada extremo.
                    let vw = |cc: usize| {
                        let up = r
                            .checked_sub(1)
                            .map_or(0.0, |rr| b.vertical[rr][cc].visible_width());
                        let down = b.vertical.get(r).map_or(0.0, |row| row[cc].visible_width());
                        up.max(down) / 2.0
                    };
                    let (l, rr) = (vw(c), vw(c + 1));
                    let y = row_y[r];
                    segs.push(BorderSegment {
                        rect: Rect::new(
                            col_x[c] - l,
                            y - e.width / 2.0,
                            widths[c] + l + rr,
                            e.width,
                        ),
                        horizontal: true,
                        style: e.style,
                        color: e.color,
                    });
                }
            }
            segs
        });
        self.table_paint = Some(TablePaint { collapsed });
        (out, content_h)
    }

    /// Completa el fragmento de una caja de tabla (pintura y baseline) o de su envoltorio
    /// (baseline de la tabla).
    pub(crate) fn table_fragment(&mut self, b: &'a LayoutBox, f: &mut Fragment) {
        if matches!(b.kind, BoxKind::Table) {
            let paint = self.table_paint.take().unwrap_or_default();
            f.table = Some(TableRole::Table(Box::new(paint)));
            // La baseline de una tabla es la de su primera fila (§10.8.1).
            f.baseline = f
                .children
                .iter()
                .flat_map(|g| g.children.first())
                .next()
                .and_then(|r| r.baseline);
            return;
        }
        if is_table_wrapper(b) {
            f.baseline = f
                .children
                .iter()
                .find(|c| matches!(c.table, Some(TableRole::Table(_))))
                .and_then(|t| t.baseline.or(Some(t.rect.bottom())));
        }
    }
}
