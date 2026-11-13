//! Rasterizado de la lista de visualización con `tiny-skia`.
//!
//! Los rectángulos sin radios se pintan sin antialias (como el pixel snapping de los
//! navegadores, y para que dos formas que comparten un borde no dejen costura); las curvas y
//! el texto, con antialias. El texto se dibuja como paths rellenos, sin hinting. Cada
//! `PushOpacity` abre una capa que se compone al cerrarla.

use bf_layout::fragment::{Edges, Rect, Size};
use bf_layout::{MeterLevel, PathCommand, Widget};
use bf_style::properties::types::{BorderStyle, TextDecorationStyle};
use bf_style::values::color::Rgba;
use tiny_skia::{
    FillRule, FilterQuality, LineCap, Mask, Paint, Path, PathBuilder, Pattern, Pixmap, PixmapPaint,
    SpreadMode, Stroke, StrokeDash, Transform,
};

use crate::display_list::{
    BoxShadowParams, DisplayItem, DisplayList, PathOp, Radii, ResolvedGradient,
};
use crate::effects::{GradientSampler, blur_channel, blur_margin, spread_radii, to_u8};

/// Constante de Bézier para aproximar un cuarto de elipse.
const KAPPA: f32 = 0.552_284_8;

/// El color de `tiny-skia` para un [`Rgba`] (canales redondeados a 8 bits).
pub fn raster_color(c: Rgba) -> tiny_skia::Color {
    color(c)
}

fn color(c: Rgba) -> tiny_skia::Color {
    let ch = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    tiny_skia::Color::from_rgba8(
        ch(c.r),
        ch(c.g),
        ch(c.b),
        (c.a * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

fn paint(c: Rgba, anti_alias: bool) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(color(c));
    p.anti_alias = anti_alias;
    // Los colores translúcidos se componen con el pipeline de alta precisión (el de baja
    // precisión redondea `x / 255` hacia arriba y se aparta en 1 de la composición de los
    // navegadores), con el color ya premultiplicado y cuantizado a 8 bits como hace Skia (E4).
    if c.a < 1.0 {
        let a8 = (c.a * 255.0).round().clamp(0.0, 255.0);
        if a8 > 0.0 {
            let q = |v: f32| (v.round().clamp(0.0, 255.0) * a8 / 255.0).round() / a8;
            if let Some(col) = tiny_skia::Color::from_rgba(q(c.r), q(c.g), q(c.b), a8 / 255.0) {
                p.set_color(col);
            }
        }
        p.force_hq_pipeline = true;
    }
    p
}

fn push_rounded(pb: &mut PathBuilder, r: &Rect, radii: &Radii) {
    let (x, y, w, h) = (r.x, r.y, r.width, r.height);
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    if radii.is_zero() {
        if let Some(rect) = tiny_skia::Rect::from_xywh(x, y, w, h) {
            pb.push_rect(rect);
        }
        return;
    }
    let [tl, tr, br, bl] = radii.0;
    pb.move_to(x + tl.0, y);
    pb.line_to(x + w - tr.0, y);
    pb.cubic_to(
        x + w - tr.0 + tr.0 * KAPPA,
        y,
        x + w,
        y + tr.1 - tr.1 * KAPPA,
        x + w,
        y + tr.1,
    );
    pb.line_to(x + w, y + h - br.1);
    pb.cubic_to(
        x + w,
        y + h - br.1 + br.1 * KAPPA,
        x + w - br.0 + br.0 * KAPPA,
        y + h,
        x + w - br.0,
        y + h,
    );
    pb.line_to(x + bl.0, y + h);
    pb.cubic_to(
        x + bl.0 - bl.0 * KAPPA,
        y + h,
        x,
        y + h - bl.1 + bl.1 * KAPPA,
        x,
        y + h - bl.1,
    );
    pb.line_to(x, y + tl.1);
    pb.cubic_to(
        x,
        y + tl.1 - tl.1 * KAPPA,
        x + tl.0 - tl.0 * KAPPA,
        y,
        x + tl.0,
        y,
    );
    pb.close();
}

fn rounded_path(r: &Rect, radii: &Radii) -> Option<Path> {
    let mut pb = PathBuilder::new();
    push_rounded(&mut pb, r, radii);
    pb.finish()
}

fn polygon(points: &[(f32, f32)]) -> Option<Path> {
    let mut pb = PathBuilder::new();
    let (&(x, y), rest) = points.split_first()?;
    pb.move_to(x, y);
    for &(x, y) in rest {
        pb.line_to(x, y);
    }
    pb.close();
    pb.finish()
}

fn shrink(r: &Rect, e: &Edges) -> Rect {
    Rect::new(
        r.x + e.left,
        r.y + e.top,
        (r.width - e.horizontal()).max(0.0),
        (r.height - e.vertical()).max(0.0),
    )
}

fn scale_edges(e: &Edges, k: f32) -> Edges {
    Edges {
        top: e.top * k,
        right: e.right * k,
        bottom: e.bottom * k,
        left: e.left * k,
    }
}

/// Más oscuro / más claro para `inset`, `outset`, `groove` y `ridge` (dependiente del UA:
/// la mitad del color hacia el negro).
fn shade(c: Rgba, dark: bool) -> Rgba {
    // Un color casi negro no tiene versión más oscura: el lado "claro" se aclara para que el
    // relieve se vea (como hacen los navegadores con el `currentcolor` negro por defecto).
    if c.r.max(c.g).max(c.b) < 32.0 {
        return if dark {
            c
        } else {
            Rgba {
                r: c.r + (255.0 - c.r) * 0.6,
                g: c.g + (255.0 - c.g) * 0.6,
                b: c.b + (255.0 - c.b) * 0.6,
                a: c.a,
            }
        };
    }
    if dark {
        Rgba {
            r: c.r * 0.5,
            g: c.g * 0.5,
            b: c.b * 0.5,
            a: c.a,
        }
    } else {
        c
    }
}

struct Canvas {
    layers: Vec<(Pixmap, f32)>,
    masks: Vec<Mask>,
    transforms: Vec<Transform>,
    /// Grupos de `text-shadow` abiertos (cada uno con su capa en `layers`).
    shadows: Vec<ShadowGroup>,
    width: u32,
    height: u32,
}

/// Un grupo de sombra abierto: lo que se pinta dentro va a una capa propia, sin recortes, y al
/// cerrarlo se desenfoca y se compone con los recortes guardados.
struct ShadowGroup {
    bounds: Rect,
    blur: f32,
    saved_masks: Vec<Mask>,
}

impl Canvas {
    fn target(&mut self) -> &mut Pixmap {
        &mut self.layers.last_mut().expect("siempre hay una capa").0
    }

    fn ts(&self) -> Transform {
        *self
            .transforms
            .last()
            .expect("siempre hay una transformación")
    }

    fn fill(&mut self, path: &Path, c: Rgba, aa: bool, rule: FillRule) {
        if c.a <= 0.0 {
            return;
        }
        let ts = self.ts();
        let mask = self.masks.last().cloned();
        self.target()
            .fill_path(path, &paint(c, aa), rule, ts, mask.as_ref());
    }

    /// Rellena `path` restringido además por `extra` (en coordenadas de página).
    fn fill_masked(&mut self, path: &Path, c: Rgba, aa: bool, extra: &Path, extra_rule: FillRule) {
        let ts = self.ts();
        let mut mask = match self.masks.last() {
            Some(m) => m.clone(),
            None => {
                let Some(mut m) = Mask::new(self.width, self.height) else {
                    return;
                };
                m.data_mut().fill(255);
                m
            }
        };
        mask.intersect_path(extra, extra_rule, true, ts);
        self.target()
            .fill_path(path, &paint(c, aa), FillRule::Winding, ts, Some(&mask));
    }

    fn stroke_masked(&mut self, path: &Path, c: Rgba, stroke: &Stroke, extra: &Path) {
        let ts = self.ts();
        let mut mask = match self.masks.last() {
            Some(m) => m.clone(),
            None => {
                let Some(mut m) = Mask::new(self.width, self.height) else {
                    return;
                };
                m.data_mut().fill(255);
                m
            }
        };
        mask.intersect_path(extra, FillRule::Winding, true, ts);
        self.target()
            .stroke_path(path, &paint(c, true), stroke, ts, Some(&mask));
    }

    fn push_clip(&mut self, rect: &Rect, radii: &Radii) {
        let ts = self.ts();
        let Some(path) = rounded_path(rect, radii) else {
            // Recorte vacío: nada de lo que siga se ve.
            let empty = Mask::new(self.width, self.height).expect("tamaño válido");
            self.masks.push(empty);
            return;
        };
        let aa = !radii.is_zero();
        let mask = match self.masks.last() {
            Some(prev) => {
                let mut m = prev.clone();
                m.intersect_path(&path, FillRule::Winding, aa, ts);
                m
            }
            None => {
                let mut m = Mask::new(self.width, self.height).expect("tamaño válido");
                m.fill_path(&path, FillRule::Winding, aa, ts);
                m
            }
        };
        self.masks.push(mask);
    }

    fn border(
        &mut self,
        rect: &Rect,
        widths: &Edges,
        colors: &[Rgba; 4],
        styles: &[BorderStyle; 4],
        radii: &Radii,
    ) {
        let w = [widths.top, widths.right, widths.bottom, widths.left];
        // Todos los lados que se ven son sólidos y del mismo color (y los demás no tienen
        // ancho): un solo anillo, sin costuras en las esquinas (E4).
        let shown: Vec<usize> = (0..4)
            .filter(|&i| {
                w[i] > 0.0 && !matches!(styles[i], BorderStyle::None | BorderStyle::Hidden)
            })
            .collect();
        let hidden_have_width = (0..4).any(|i| !shown.contains(&i) && w[i] > 0.0);
        if let Some(&first) = shown.first()
            && !hidden_have_width
            && shown
                .iter()
                .all(|&i| styles[i] == BorderStyle::Solid && colors[i] == colors[first])
        {
            let inner = shrink(rect, widths);
            let mut pb = PathBuilder::new();
            push_rounded(&mut pb, rect, radii);
            push_rounded(&mut pb, &inner, &radii.shrink(widths));
            if let Some(ring) = pb.finish() {
                self.fill(&ring, colors[first], !radii.is_zero(), FillRule::EvenOdd);
            }
            return;
        }
        for side in 0..4 {
            if w[side] <= 0.0 || matches!(styles[side], BorderStyle::None | BorderStyle::Hidden) {
                continue;
            }
            let c = colors[side];
            // inset/outset: oscuro arriba e izquierda (inset) o abajo y derecha (outset).
            let top_left = side == 0 || side == 3;
            match styles[side] {
                BorderStyle::Double if w[side] >= 3.0 => {
                    let third = scale_edges(widths, 1.0 / 3.0);
                    let two = scale_edges(widths, 2.0 / 3.0);
                    self.band(rect, radii, &Edges::default(), &third, widths, side, c);
                    self.band(rect, radii, &two, widths, widths, side, c);
                }
                BorderStyle::Groove | BorderStyle::Ridge => {
                    let half = scale_edges(widths, 0.5);
                    let groove = styles[side] == BorderStyle::Groove;
                    let outer = shade(c, top_left == groove);
                    let inner = shade(c, top_left != groove);
                    self.band(rect, radii, &Edges::default(), &half, widths, side, outer);
                    self.band(rect, radii, &half, widths, widths, side, inner);
                }
                BorderStyle::Inset | BorderStyle::Outset => {
                    let dark = top_left == (styles[side] == BorderStyle::Inset);
                    self.band(
                        rect,
                        radii,
                        &Edges::default(),
                        widths,
                        widths,
                        side,
                        shade(c, dark),
                    );
                }
                BorderStyle::Dotted | BorderStyle::Dashed => {
                    self.dashed_side(
                        rect,
                        widths,
                        radii,
                        side,
                        c,
                        styles[side] == BorderStyle::Dotted,
                    );
                }
                _ => self.band(rect, radii, &Edges::default(), widths, widths, side, c),
            }
        }
    }

    /// Cuña del lado `side` entre las esquinas exteriores, las interiores y el centro.
    fn wedge(outer: &Rect, inner: &Rect, side: usize) -> Option<Path> {
        let (ox0, oy0, ox1, oy1) = (outer.x, outer.y, outer.right(), outer.bottom());
        let (ix0, iy0, ix1, iy1) = (inner.x, inner.y, inner.right(), inner.bottom());
        let c = (inner.x + inner.width / 2.0, inner.y + inner.height / 2.0);
        let pts = match side {
            0 => [(ox0, oy0), (ox1, oy0), (ix1, iy0), c, (ix0, iy0)],
            1 => [(ox1, oy0), (ox1, oy1), (ix1, iy1), c, (ix1, iy0)],
            2 => [(ox1, oy1), (ox0, oy1), (ix0, iy1), c, (ix1, iy1)],
            _ => [(ox0, oy1), (ox0, oy0), (ix0, iy0), c, (ix0, iy1)],
        };
        polygon(&pts)
    }

    /// Banda del borde entre `from` y `to` (distancias desde la border box), en el lado
    /// `side`. Las esquinas se cortan en diagonal (de la esquina exterior a la interior).
    #[allow(clippy::too_many_arguments)]
    fn band(
        &mut self,
        rect: &Rect,
        radii: &Radii,
        from: &Edges,
        to: &Edges,
        widths: &Edges,
        side: usize,
        c: Rgba,
    ) {
        let outer = shrink(rect, from);
        let inner = shrink(rect, to);
        if radii.is_zero() {
            // Las esquinas de los rectángulos intermedios caen sobre la diagonal de la
            // esquina: la banda es un trapecio exacto.
            let (ox0, oy0, ox1, oy1) = (outer.x, outer.y, outer.right(), outer.bottom());
            let (ix0, iy0, ix1, iy1) = (inner.x, inner.y, inner.right(), inner.bottom());
            let pts = match side {
                0 => [(ox0, oy0), (ox1, oy0), (ix1, iy0), (ix0, iy0)],
                1 => [(ox1, oy0), (ox1, oy1), (ix1, iy1), (ix1, iy0)],
                2 => [(ox1, oy1), (ox0, oy1), (ix0, iy1), (ix1, iy1)],
                _ => [(ox0, oy1), (ox0, oy0), (ix0, iy0), (ix0, iy1)],
            };
            if let Some(p) = polygon(&pts) {
                self.fill(&p, c, false, FillRule::Winding);
            }
            return;
        }
        let border_inner = shrink(rect, widths);
        let Some(wedge) = Self::wedge(rect, &border_inner, side) else {
            return;
        };
        let mut pb = PathBuilder::new();
        push_rounded(&mut pb, &outer, &radii.shrink(from));
        push_rounded(&mut pb, &inner, &radii.shrink(to));
        if let Some(ring) = pb.finish() {
            self.fill_masked(&wedge, c, true, &ring, FillRule::EvenOdd);
        }
    }

    fn dashed_side(
        &mut self,
        rect: &Rect,
        widths: &Edges,
        radii: &Radii,
        side: usize,
        c: Rgba,
        dotted: bool,
    ) {
        let w = [widths.top, widths.right, widths.bottom, widths.left][side];
        let inner = shrink(rect, widths);
        let Some(wedge) = Self::wedge(rect, &inner, side) else {
            return;
        };
        let (x0, y0, x1, y1) = match side {
            0 => (rect.x, rect.y + w / 2.0, rect.right(), rect.y + w / 2.0),
            1 => (
                rect.right() - w / 2.0,
                rect.y,
                rect.right() - w / 2.0,
                rect.bottom(),
            ),
            2 => (
                rect.right(),
                rect.bottom() - w / 2.0,
                rect.x,
                rect.bottom() - w / 2.0,
            ),
            _ => (rect.x + w / 2.0, rect.bottom(), rect.x + w / 2.0, rect.y),
        };
        let mut pb = PathBuilder::new();
        pb.move_to(x0, y0);
        pb.line_to(x1, y1);
        let Some(line) = pb.finish() else {
            return;
        };
        let (pattern, cap) = if dotted {
            (vec![0.0, 2.0 * w], LineCap::Round)
        } else {
            (vec![3.0 * w, 3.0 * w], LineCap::Butt)
        };
        let stroke = Stroke {
            width: w,
            line_cap: cap,
            dash: StrokeDash::new(pattern, 0.0),
            ..Stroke::default()
        };
        let mut clip = PathBuilder::new();
        if radii.is_zero() {
            self.stroke_masked(&line, c, &stroke, &wedge);
            return;
        }
        push_rounded(&mut clip, rect, radii);
        push_rounded(&mut clip, &inner, &radii.shrink(widths));
        if let Some(ring) = clip.finish() {
            // Cuña ∩ anillo.
            let ts = self.ts();
            let mut mask = match self.masks.last() {
                Some(m) => m.clone(),
                None => {
                    let mut m = Mask::new(self.width, self.height).expect("tamaño válido");
                    m.data_mut().fill(255);
                    m
                }
            };
            mask.intersect_path(&ring, FillRule::EvenOdd, true, ts);
            mask.intersect_path(&wedge, FillRule::Winding, true, ts);
            self.target()
                .stroke_path(&line, &paint(c, true), &stroke, ts, Some(&mask));
        }
    }

    /// La imagen escalada a `rect` (filtro bilineal).
    fn image(&mut self, rect: &Rect, image: &Pixmap) {
        let (iw, ih) = (image.width() as f32, image.height() as f32);
        if iw <= 0.0 || ih <= 0.0 || rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }
        let ts = self
            .ts()
            .pre_translate(rect.x, rect.y)
            .pre_scale(rect.width / iw, rect.height / ih);
        let paint = PixmapPaint {
            quality: FilterQuality::Bilinear,
            ..PixmapPaint::default()
        };
        let mask = self.masks.last().cloned();
        self.target()
            .draw_pixmap(0, 0, image.as_ref(), &paint, ts, mask.as_ref());
    }

    /// Una capa de fondo: tiles de `tile.width`×`tile.height` alineados con `tile`, repetidos
    /// en todo `area` en los ejes sin lista de posiciones y solo en las posiciones dadas en los
    /// demás. Cada región se rellena con un patrón repetido; `area` recorta (con sus radios).
    fn background_image(
        &mut self,
        area: &Rect,
        radii: &Radii,
        tile: &Rect,
        xs: Option<&[f32]>,
        ys: Option<&[f32]>,
        image: &Pixmap,
    ) {
        let (iw, ih) = (image.width() as f32, image.height() as f32);
        if iw <= 0.0 || ih <= 0.0 || tile.width <= 0.0 || tile.height <= 0.0 {
            return;
        }
        // Cada tile se pinta por separado con `SpreadMode::Pad`, para que el filtro bilineal no
        // mezcle un borde de la imagen con el opuesto (E4); si hay demasiados tiles, un solo
        // patrón repetido por región.
        let tiles_on = |start: f32, end: f32, origin: f32, size: f32| -> Vec<f32> {
            let first = ((start - origin) / size).floor() as i64;
            let last = ((end - origin) / size).ceil() as i64;
            (first..last).map(|k| origin + k as f32 * size).collect()
        };
        let mut x_tiles: Vec<f32> = match xs {
            None => tiles_on(area.x, area.right(), tile.x, tile.width),
            Some(v) => v.to_vec(),
        };
        let mut y_tiles: Vec<f32> = match ys {
            None => tiles_on(area.y, area.bottom(), tile.y, tile.height),
            Some(v) => v.to_vec(),
        };
        let ts = self.ts();
        // Tiles de menos de 2 px de dispositivo: el patrón repetido (rellenar regiones subpíxel
        // sin antialias dejaría huecos).
        let per_tile = x_tiles.len() * y_tiles.len() <= 4096
            && tile.width * ts.sx.abs() >= 2.0
            && tile.height * ts.sy.abs() >= 2.0;
        // (inicio, fin, origen del patrón) de cada franja.
        let spans =
            |tiles: &mut Vec<f32>, list: Option<&[f32]>, a0: f32, a1: f32, o: f32, t: f32| {
                if per_tile || list.is_some() {
                    std::mem::take(tiles)
                        .into_iter()
                        .map(|p| (p, p + t, p))
                        .collect()
                } else {
                    vec![(a0, a1, o)]
                }
            };
        let x_spans: Vec<(f32, f32, f32)> =
            spans(&mut x_tiles, xs, area.x, area.right(), tile.x, tile.width);
        let y_spans: Vec<(f32, f32, f32)> =
            spans(&mut y_tiles, ys, area.y, area.bottom(), tile.y, tile.height);
        let spread = if per_tile {
            SpreadMode::Pad
        } else {
            SpreadMode::Repeat
        };
        let mut mask = self.masks.last().cloned();
        if !radii.is_zero()
            && let Some(ring) = rounded_path(area, radii)
        {
            let mut m = mask.unwrap_or_else(|| {
                let mut m = Mask::new(self.width, self.height).expect("tamaño válido");
                m.data_mut().fill(255);
                m
            });
            m.intersect_path(&ring, FillRule::Winding, true, ts);
            mask = Some(m);
        }
        for &(x0, x1, ox) in &x_spans {
            for &(y0, y1, oy) in &y_spans {
                let (l, r) = (x0.max(area.x), x1.min(area.right()));
                let (t, b) = (y0.max(area.y), y1.min(area.bottom()));
                if r <= l || b <= t {
                    continue;
                }
                let Some(region) = rounded_path(&Rect::new(l, t, r - l, b - t), &Radii::default())
                else {
                    continue;
                };
                let shader = Pattern::new(
                    image.as_ref(),
                    spread,
                    FilterQuality::Bilinear,
                    1.0,
                    Transform::from_translate(ox, oy).pre_scale(tile.width / iw, tile.height / ih),
                );
                let paint = Paint {
                    shader,
                    anti_alias: false,
                    ..Paint::default()
                };
                self.target()
                    .fill_path(&region, &paint, FillRule::Winding, ts, mask.as_ref());
            }
        }
    }

    fn decoration(&mut self, rect: &Rect, c: Rgba, style: TextDecorationStyle) {
        let t = rect.height;
        match style {
            TextDecorationStyle::Solid => {
                if let Some(p) = rounded_path(rect, &Radii::default()) {
                    self.fill(&p, c, false, FillRule::Winding);
                }
            }
            TextDecorationStyle::Double => {
                for dy in [0.0, 2.0 * t] {
                    let r = Rect::new(rect.x, rect.y + dy, rect.width, t);
                    if let Some(p) = rounded_path(&r, &Radii::default()) {
                        self.fill(&p, c, false, FillRule::Winding);
                    }
                }
            }
            TextDecorationStyle::Dotted
            | TextDecorationStyle::Dashed
            | TextDecorationStyle::Wavy => {
                let y = rect.y + t / 2.0;
                let mut pb = PathBuilder::new();
                pb.move_to(rect.x, y);
                if style == TextDecorationStyle::Wavy {
                    // Zigzag suavizado de amplitud `t` y longitud de onda `4t`.
                    let mut x = rect.x;
                    let step = 2.0 * t.max(1.0);
                    let mut up = true;
                    while x < rect.right() {
                        let nx = (x + step).min(rect.right());
                        let cy = if up { y - 2.0 * t } else { y + 2.0 * t };
                        pb.quad_to((x + nx) / 2.0, cy, nx, y);
                        x = nx;
                        up = !up;
                    }
                } else {
                    pb.line_to(rect.right(), y);
                }
                let Some(path) = pb.finish() else {
                    return;
                };
                let dash = match style {
                    TextDecorationStyle::Dotted => StrokeDash::new(vec![t, t], 0.0),
                    TextDecorationStyle::Dashed => StrokeDash::new(vec![3.0 * t, 3.0 * t], 0.0),
                    _ => None,
                };
                let stroke = Stroke {
                    width: t,
                    dash,
                    ..Stroke::default()
                };
                let ts = self.ts();
                let mask = self.masks.last().cloned();
                self.target()
                    .stroke_path(&path, &paint(c, true), &stroke, ts, mask.as_ref());
            }
        }
    }
}

/// Colores del aspecto nativo de los controles (esquema claro).
const ACCENT: Rgba = Rgba {
    r: 0.0,
    g: 117.0,
    b: 255.0,
    a: 1.0,
};
const CONTROL_BORDER: Rgba = Rgba {
    r: 118.0,
    g: 118.0,
    b: 118.0,
    a: 1.0,
};
const TRACK: Rgba = Rgba {
    r: 239.0,
    g: 239.0,
    b: 239.0,
    a: 1.0,
};

fn circle(cx: f32, cy: f32, r: f32) -> Option<Path> {
    PathBuilder::from_circle(cx, cy, r.max(0.1))
}

impl Canvas {
    fn stroke(&mut self, path: &Path, c: Rgba, width: f32) {
        let stroke = Stroke {
            width,
            line_cap: LineCap::Round,
            line_join: tiny_skia::LineJoin::Round,
            ..Stroke::default()
        };
        let ts = self.ts();
        let mask = self.masks.last().cloned();
        self.target()
            .stroke_path(path, &paint(c, true), &stroke, ts, mask.as_ref());
    }

    fn rounded_fill(&mut self, r: &Rect, radius: f32, c: Rgba) {
        let radii = Radii([(radius, radius); 4]).fit(r);
        if let Some(p) = rounded_path(r, &radii) {
            self.fill(&p, c, true, FillRule::Winding);
        }
    }

    fn rounded_stroke(&mut self, r: &Rect, radius: f32, c: Rgba) {
        // El trazo de 1px va por dentro del rectángulo.
        let inner = Rect::new(r.x + 0.5, r.y + 0.5, r.width - 1.0, r.height - 1.0);
        let radii = Radii([(radius, radius); 4]).fit(&inner);
        if let Some(p) = rounded_path(&inner, &radii) {
            self.stroke(&p, c, 1.0);
        }
    }

    /// Una barra horizontal (progress, meter): pista y tramo lleno.
    fn bar(&mut self, rect: &Rect, fraction: Option<f32>, fill: Rgba) {
        let radius = (rect.height / 2.0).min(4.0);
        self.rounded_fill(rect, radius, TRACK);
        if let Some(f) = fraction
            && f > 0.0
        {
            let filled = Rect::new(rect.x, rect.y, rect.width * f.min(1.0), rect.height);
            self.rounded_fill(&filled, radius, fill);
        }
        self.rounded_stroke(rect, radius, CONTROL_BORDER);
    }

    /// Parte nativa de un control (HTML §15.5): el aspecto lo decide el UA.
    fn widget(&mut self, rect: &Rect, widget: Widget, text: Rgba) {
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }
        let (cx, cy) = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        match widget {
            Widget::Checkbox { checked } => {
                let radius = 2.0;
                if checked {
                    self.rounded_fill(rect, radius, ACCENT);
                    let (w, h) = (rect.width, rect.height);
                    let mut pb = PathBuilder::new();
                    pb.move_to(rect.x + 0.22 * w, rect.y + 0.52 * h);
                    pb.line_to(rect.x + 0.42 * w, rect.y + 0.72 * h);
                    pb.line_to(rect.x + 0.78 * w, rect.y + 0.3 * h);
                    if let Some(p) = pb.finish() {
                        self.stroke(&p, Rgba::WHITE, (0.15 * w).max(1.0));
                    }
                } else {
                    self.rounded_fill(rect, radius, Rgba::WHITE);
                    self.rounded_stroke(rect, radius, CONTROL_BORDER);
                }
            }
            Widget::Radio { checked } => {
                let r = rect.width.min(rect.height) / 2.0;
                if let Some(p) = circle(cx, cy, r) {
                    self.fill(
                        &p,
                        if checked { ACCENT } else { Rgba::WHITE },
                        true,
                        FillRule::Winding,
                    );
                }
                if checked {
                    if let Some(p) = circle(cx, cy, r * 0.8) {
                        self.fill(&p, Rgba::WHITE, true, FillRule::Winding);
                    }
                    if let Some(p) = circle(cx, cy, r * 0.5) {
                        self.fill(&p, ACCENT, true, FillRule::Winding);
                    }
                } else if let Some(p) = circle(cx, cy, r - 0.5) {
                    self.stroke(&p, CONTROL_BORDER, 1.0);
                }
            }
            Widget::SelectArrow => {
                let half = (rect.width * 0.2).clamp(2.0, 5.0);
                let mut pb = PathBuilder::new();
                pb.move_to(cx - half, cy - half / 2.0);
                pb.line_to(cx, cy + half / 2.0);
                pb.line_to(cx + half, cy - half / 2.0);
                if let Some(p) = pb.finish() {
                    self.stroke(&p, text, 1.5);
                }
            }
            Widget::Progress(fraction) => self.bar(rect, fraction, ACCENT),
            Widget::Meter { fraction, level } => {
                let fill = match level {
                    MeterLevel::Optimum => Rgba::rgb(16, 124, 16),
                    MeterLevel::Suboptimum => Rgba::rgb(255, 185, 0),
                    MeterLevel::EvenLessGood => Rgba::rgb(216, 59, 1),
                };
                self.bar(rect, Some(fraction), fill);
            }
            Widget::Range(fraction) => {
                let thumb = (rect.height / 2.0).min(8.0);
                let track_h = 4.0f32.min(rect.height);
                let x0 = rect.x + thumb;
                let x1 = (rect.right() - thumb).max(x0);
                let tx = x0 + (x1 - x0) * fraction.clamp(0.0, 1.0);
                let track = Rect::new(x0, cy - track_h / 2.0, x1 - x0, track_h);
                self.rounded_fill(&track, track_h / 2.0, TRACK);
                self.rounded_stroke(&track, track_h / 2.0, CONTROL_BORDER);
                let done = Rect::new(x0, track.y, tx - x0, track_h);
                self.rounded_fill(&done, track_h / 2.0, ACCENT);
                if let Some(p) = circle(tx, cy, thumb) {
                    self.fill(&p, ACCENT, true, FillRule::Winding);
                }
            }
            Widget::ColorSwatch(c) => {
                let swatch = shrink(
                    rect,
                    &Edges {
                        top: 2.0,
                        right: 2.0,
                        bottom: 2.0,
                        left: 2.0,
                    },
                );
                if let Some(p) = rounded_path(&swatch, &Radii::default()) {
                    self.fill(&p, c, false, FillRule::Winding);
                }
                self.rounded_stroke(&swatch, 0.0, CONTROL_BORDER);
            }
        }
    }
}

impl Canvas {
    /// Glifos rellenos como paths, con el origen de cada uno ajustado al píxel de dispositivo.
    fn text(
        &mut self,
        glyphs: &[crate::display_list::GlyphInstance],
        size: f32,
        color: Rgba,
        origin: (f32, f32),
        y_fit: (f32, f32),
    ) {
        let mut pb = PathBuilder::new();
        // El origen de cada glifo se ajusta al píxel de dispositivo (sin posicionamiento
        // subpíxel), como hacen los navegadores: así un glifo de Ahem coincide con un
        // rectángulo en la misma posición.
        let ts = self.ts();
        let snap = |v: f32, s: f32, t: f32| {
            if s != 0.0 {
                ((v * s + t).round() - t) / s
            } else {
                v
            }
        };
        let mut bitmaps = Vec::new();
        for g in glyphs {
            let Some(outline) = &g.outline else {
                continue;
            };
            let k = size / outline.units_per_em;
            let ox = snap(origin.0 + g.x, ts.sx, ts.tx);
            let oy = snap(origin.1 + g.y, ts.sy, ts.ty);
            // E5: glifo de mapa de bits (emoji en color): se pinta como imagen.
            if !outline.bitmaps.is_empty() {
                bitmaps.push((ox, oy, outline));
                continue;
            }
            let map = |x: f32, y: f32| {
                let fit = if y >= 0.0 { y_fit.0 } else { y_fit.1 };
                (ox + x * k, oy - y * k * fit)
            };
            for cmd in &outline.commands {
                match *cmd {
                    PathCommand::MoveTo(x, y) => {
                        let (x, y) = map(x, y);
                        pb.move_to(x, y);
                    }
                    PathCommand::LineTo(x, y) => {
                        let (x, y) = map(x, y);
                        pb.line_to(x, y);
                    }
                    PathCommand::QuadTo(x1, y1, x, y) => {
                        let (x1, y1) = map(x1, y1);
                        let (x, y) = map(x, y);
                        pb.quad_to(x1, y1, x, y);
                    }
                    PathCommand::CurveTo(x1, y1, x2, y2, x, y) => {
                        let (x1, y1) = map(x1, y1);
                        let (x2, y2) = map(x2, y2);
                        let (x, y) = map(x, y);
                        pb.cubic_to(x1, y1, x2, y2, x, y);
                    }
                    PathCommand::Close => pb.close(),
                }
            }
        }
        if let Some(path) = pb.finish() {
            self.fill(&path, color, true, FillRule::Winding);
        }
        // Tamaño en píxeles de dispositivo, para elegir el mapa de bits más cercano.
        let device_ppem = size * ts.sx.hypot(ts.ky).max(ts.sy.hypot(ts.kx));
        for (ox, oy, outline) in bitmaps {
            let Some(b) = nearest_bitmap(&outline.bitmaps, device_ppem) else {
                continue;
            };
            let Some(image) = bitmap_pixmap(&b.png) else {
                continue;
            };
            // OpenType `sbix`: la esquina inferior izquierda de la imagen en
            // (originOffsetX, originOffsetY) píxeles del tamaño, escalados al de la fuente.
            let s = size / f32::from(b.ppem.max(1));
            let (w, h) = (image.width() as f32 * s, image.height() as f32 * s);
            let rect = Rect::new(ox + b.x * s, oy - b.y * s - h, w, h);
            self.image(&rect, &image);
        }
    }
}

/// E5: el mapa de bits de un glifo para `ppem` píxeles por em: el menor que no sea más pequeño
/// (se reduce) o, si no hay, el mayor. `bitmaps` va de menor a mayor.
fn nearest_bitmap(
    bitmaps: &[bf_layout::GlyphBitmap],
    ppem: f32,
) -> Option<&bf_layout::GlyphBitmap> {
    bitmaps
        .iter()
        .find(|b| f32::from(b.ppem) >= ppem)
        .or_else(|| bitmaps.last())
}

/// E5: decodifica (con caché) el PNG de un mapa de bits de glifo. La caché guarda el `Arc` de
/// los bytes, así que su dirección no se reutiliza mientras la entrada exista.
fn bitmap_pixmap(png: &std::sync::Arc<[u8]>) -> Option<std::sync::Arc<Pixmap>> {
    type Cache =
        std::collections::HashMap<usize, (std::sync::Arc<[u8]>, Option<std::sync::Arc<Pixmap>>)>;
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Cache>> = std::sync::OnceLock::new();
    let key = std::sync::Arc::as_ptr(png).cast::<u8>() as usize;
    let cache = CACHE.get_or_init(Default::default);
    if let Some((_, p)) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return p.clone();
    }
    let pixmap = bf_image::decode(png, Some("image/png"))
        .ok()
        .and_then(|img| {
            let size = tiny_skia::IntSize::from_wh(img.width, img.height)?;
            Pixmap::from_vec(img.rgba, size).map(std::sync::Arc::new)
        });
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() >= 2048 {
        cache.clear();
    }
    cache.insert(key, (png.clone(), pixmap.clone()));
    pixmap
}

// --- E7: máscaras ---

impl Canvas {
    /// Cierra una capa de `mask-image`: pinta las capas de la máscara en otra capa y compone
    /// la de contenido con el alfa de esa como máscara.
    fn pop_mask(&mut self, items: &[DisplayItem]) {
        if self.layers.len() <= 1 {
            return;
        }
        let Some(layer) = Pixmap::new(self.width, self.height) else {
            return;
        };
        self.layers.push((layer, 1.0));
        let clips = self.masks.len();
        for item in items {
            match item {
                DisplayItem::BackgroundImage {
                    area,
                    radii,
                    tile,
                    xs,
                    ys,
                    image,
                } => self.background_image(area, radii, tile, xs.as_deref(), ys.as_deref(), image),
                DisplayItem::Gradient {
                    area,
                    radii,
                    tile,
                    xs,
                    ys,
                    gradient,
                } => self.gradient(area, radii, tile, xs.as_deref(), ys.as_deref(), gradient),
                DisplayItem::PushClip { rect, radii } => self.push_clip(rect, radii),
                DisplayItem::PushClipBorder {
                    rect,
                    widths,
                    styles,
                    radii,
                } => {
                    let (rect, widths, styles, radii) = (*rect, *widths, *styles, *radii);
                    self.push_clip_drawn(|c| {
                        c.border(&rect, &widths, &[Rgba::BLACK; 4], &styles, &radii);
                    });
                }
                DisplayItem::PushClipText { glyphs } => self.push_clip_text(glyphs),
                DisplayItem::PopClip if self.masks.len() > clips => {
                    self.masks.pop();
                }
                _ => {}
            }
        }
        self.masks.truncate(clips);
        let (alpha, _) = self.layers.pop().expect("capa de la máscara");
        let (content, _) = self.layers.pop().expect("capa enmascarada");
        let mask = Mask::from_pixmap(alpha.as_ref(), tiny_skia::MaskType::Alpha);
        self.target().draw_pixmap(
            0,
            0,
            content.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            Some(&mask),
        );
    }
}

// --- E4: gradientes y sombras ---

/// Rectángulo de píxeles de dispositivo `[x0, x1) × [y0, y1)`.
#[derive(Debug, Clone, Copy)]
struct DevRect {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

impl DevRect {
    fn width(&self) -> u32 {
        (self.x1 - self.x0).max(0) as u32
    }
    fn height(&self) -> u32 {
        (self.y1 - self.y0).max(0) as u32
    }
    fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }
    fn expand(&self, m: i32) -> DevRect {
        DevRect {
            x0: self.x0 - m,
            y0: self.y0 - m,
            x1: self.x1 + m,
            y1: self.y1 + m,
        }
    }
    fn intersect(&self, o: &DevRect) -> DevRect {
        DevRect {
            x0: self.x0.max(o.x0),
            y0: self.y0.max(o.y0),
            x1: self.x1.min(o.x1),
            y1: self.y1.min(o.y1),
        }
    }
}

/// `r` agrandado `d` px por cada lado (encogido si `d < 0`).
fn inflate(r: &Rect, d: f32) -> Rect {
    Rect::new(
        r.x - d,
        r.y - d,
        (r.width + 2.0 * d).max(0.0),
        (r.height + 2.0 * d).max(0.0),
    )
}

/// `a` menos `b`, como hasta cuatro rectángulos (arriba, abajo, izquierda y derecha).
fn rect_difference(a: &Rect, b: &Rect) -> Vec<Rect> {
    let (ax1, ay1) = (a.right(), a.bottom());
    let (bx0, by0) = (b.x.max(a.x), b.y.max(a.y));
    let (bx1, by1) = (b.right().min(ax1), b.bottom().min(ay1));
    if b.width <= 0.0 || b.height <= 0.0 || bx1 <= bx0 || by1 <= by0 {
        return vec![*a];
    }
    let mut out = Vec::with_capacity(4);
    let mut push = |x0: f32, y0: f32, x1: f32, y1: f32| {
        if x1 > x0 && y1 > y0 {
            out.push(Rect::new(x0, y0, x1 - x0, y1 - y0));
        }
    };
    push(a.x, a.y, ax1, by0);
    push(a.x, by1, ax1, ay1);
    push(a.x, by0, bx0, by1);
    push(bx1, by0, ax1, by1);
    out
}

/// Coordenada local al tile del punto `p` de un eje: repetido (`list` = `None`) o solo en los
/// tiles que empiezan en `list`.
fn tile_local(p: f32, origin: f32, size: f32, list: Option<&[f32]>) -> Option<f32> {
    match list {
        None => Some((p - origin).rem_euclid(size)),
        Some(v) => v.iter().find(|&&s| p >= s && p < s + size).map(|&s| p - s),
    }
}

impl Canvas {
    fn canvas_rect(&self) -> DevRect {
        DevRect {
            x0: 0,
            y0: 0,
            x1: self.width as i32,
            y1: self.height as i32,
        }
    }

    /// Píxeles de dispositivo que toca `r` (px CSS) con la transformación actual.
    fn device_rect(&self, r: &Rect) -> DevRect {
        let ts = self.ts();
        let (ax, bx) = (r.x * ts.sx + ts.tx, r.right() * ts.sx + ts.tx);
        let (ay, by) = (r.y * ts.sy + ts.ty, r.bottom() * ts.sy + ts.ty);
        let clampi = |v: f32| v.clamp(-1e7, 1e7) as i32;
        DevRect {
            x0: clampi(ax.min(bx).floor()),
            y0: clampi(ay.min(by).floor()),
            x1: clampi(ax.max(bx).ceil()),
            y1: clampi(ay.max(by).ceil()),
        }
    }

    /// El recorte activo, o una máscara que deja pasar todo.
    fn full_mask(&self) -> Option<Mask> {
        match self.masks.last() {
            Some(m) => Some(m.clone()),
            None => {
                let mut m = Mask::new(self.width, self.height)?;
                m.data_mut().fill(255);
                Some(m)
            }
        }
    }

    /// Una capa de fondo con un gradiente (CSS Images 3 §3): cada píxel de dispositivo del área
    /// se muestrea en su centro, en la posición correspondiente de su tile.
    fn gradient(
        &mut self,
        area: &Rect,
        radii: &Radii,
        tile: &Rect,
        xs: Option<&[f32]>,
        ys: Option<&[f32]>,
        g: &ResolvedGradient,
    ) {
        let ts = self.ts();
        if tile.width <= 0.0 || tile.height <= 0.0 || ts.sx <= 0.0 || ts.sy <= 0.0 {
            return;
        }
        let dev = self.device_rect(area).intersect(&self.canvas_rect());
        if dev.is_empty() {
            return;
        }
        let sampler = GradientSampler::new(g);
        let us: Vec<Option<f32>> = (dev.x0..dev.x1)
            .map(|d| {
                let p = (d as f32 + 0.5 - ts.tx) / ts.sx;
                if p < area.x || p >= area.right() {
                    return None;
                }
                tile_local(p, tile.x, tile.width, xs)
            })
            .collect();
        let vs: Vec<Option<f32>> = (dev.y0..dev.y1)
            .map(|d| {
                let p = (d as f32 + 0.5 - ts.ty) / ts.sy;
                if p < area.y || p >= area.bottom() {
                    return None;
                }
                tile_local(p, tile.y, tile.height, ys)
            })
            .collect();

        let mask = if radii.is_zero() {
            self.masks.last().cloned()
        } else {
            let mut m = self.full_mask();
            if let (Some(m), Some(path)) = (&mut m, rounded_path(area, radii)) {
                m.intersect_path(&path, FillRule::Winding, true, ts);
            }
            m
        };
        // Se compone en `f32` directamente sobre el destino (source-over premultiplicado, con
        // la máscara como cobertura) y se redondea una sola vez: componer un tile de 8 bits
        // redondea dos veces y deja saltos no monótonos (anillos) en los gradientes
        // semitransparentes.
        let canvas_w = self.width as usize;
        let target = self.target().data_mut();
        for (j, v) in vs.iter().enumerate() {
            let Some(v) = *v else { continue };
            let y = dev.y0 as usize + j;
            for (i, u) in us.iter().enumerate() {
                let Some(u) = *u else { continue };
                let idx = y * canvas_w + dev.x0 as usize + i;
                let coverage = mask
                    .as_ref()
                    .map_or(1.0, |m| f32::from(m.data()[idx]) / 255.0);
                if coverage <= 0.0 {
                    continue;
                }
                let src = sampler.at(u, v);
                let k = idx * 4;
                let alpha = src[3].clamp(0.0, 1.0) * coverage;
                let mut out = [0u8; 4];
                for c in 0..4 {
                    let d = f32::from(target[k + c]) / 255.0;
                    let value = src[c].clamp(0.0, 1.0) * coverage + d * (1.0 - alpha);
                    out[c] = (value * 255.0).round().clamp(0.0, 255.0) as u8;
                }
                // Premultiplicado: ningún canal supera el alfa.
                for c in 0..3 {
                    out[c] = out[c].min(out[3]);
                }
                target[k..k + 4].copy_from_slice(&out);
            }
        }
    }

    /// Una sombra de `box-shadow` (CSS Backgrounds 3 §7.1–§7.2). Exterior: la border box
    /// desplazada y extendida, desenfocada y visible solo fuera de la border box. Interior: el
    /// exterior de la padding box desplazada y encogida, visible solo dentro de la padding box.
    fn box_shadow(&mut self, rect: &Rect, radii: &Radii, s: &BoxShadowParams) {
        if s.color.a <= 0.0 {
            return;
        }
        let ts = self.ts();
        // σ = blur / 2 (§7.1), en px de dispositivo.
        let sigma = s.blur / 2.0 * ts.sx.abs();
        let margin = blur_margin(sigma) as i32 + 1;
        let spread = if s.inset { -s.spread } else { s.spread };
        let mut shape = inflate(rect, spread);
        shape.translate(s.offset.0, s.offset.1);
        let shape_radii = spread_radii(radii, spread, (rect.width, rect.height)).fit(&shape);
        // Sin desenfoque ni radios: la sombra es una diferencia de rectángulos que se rellena
        // igual que un fondo (mismos píxeles y redondeos que un `background-color`).
        if sigma <= 0.0 && radii.is_zero() && shape_radii.is_zero() {
            let (from, minus) = if s.inset {
                (*rect, shape)
            } else {
                (shape, *rect)
            };
            for r in rect_difference(&from, &minus) {
                if let Some(p) = rounded_path(&r, &Radii::default()) {
                    self.fill(&p, s.color, false, FillRule::Winding);
                }
            }
            return;
        }
        let shape_path = rounded_path(&shape, &shape_radii);
        if !s.inset && shape_path.is_none() {
            return;
        }
        let visible = self.canvas_rect().expand(margin);
        let region = if s.inset {
            self.device_rect(rect)
        } else {
            self.device_rect(&shape)
        }
        .expand(margin)
        .intersect(&visible);
        if region.is_empty() {
            return;
        }
        let (bw, bh) = (region.width(), region.height());
        let Some(mut shape_mask) = Mask::new(bw, bh) else {
            return;
        };
        let local = ts.post_translate(-region.x0 as f32, -region.y0 as f32);
        // Sin radios, sin antialias: como los fondos, para que los bordes coincidan.
        if let Some(path) = &shape_path {
            shape_mask.fill_path(path, FillRule::Winding, !shape_radii.is_zero(), local);
        }
        if s.inset {
            shape_mask.invert();
        }
        let mut alpha: Vec<f32> = shape_mask
            .data()
            .iter()
            .map(|&a| f32::from(a) / 255.0)
            .collect();
        blur_channel(&mut alpha, bw as usize, bh as usize, sigma);
        // Solo la parte dentro del canvas.
        let out = region.intersect(&self.canvas_rect());
        if out.is_empty() {
            return;
        }
        let Some(mut pix) = Pixmap::new(out.width(), out.height()) else {
            return;
        };
        let c = [
            s.color.r / 255.0 * s.color.a,
            s.color.g / 255.0 * s.color.a,
            s.color.b / 255.0 * s.color.a,
            s.color.a,
        ];
        let ow = out.width() as usize;
        let data = pix.data_mut();
        for y in out.y0..out.y1 {
            for x in out.x0..out.x1 {
                let a = alpha[((y - region.y0) as usize) * bw as usize + (x - region.x0) as usize];
                if a <= 0.0 {
                    continue;
                }
                let k = (((y - out.y0) as usize) * ow + (x - out.x0) as usize) * 4;
                let px = [c[0] * a, c[1] * a, c[2] * a, c[3] * a];
                data[k..k + 4].copy_from_slice(&to_u8(&px));
            }
        }
        // Recorte: fuera de la border box (exterior) o dentro de la padding box (interior).
        let Some(mut mask) = self.full_mask() else {
            return;
        };
        if let Some(box_path) = rounded_path(rect, radii) {
            let aa = !radii.is_zero();
            if s.inset {
                mask.intersect_path(&box_path, FillRule::Winding, aa, ts);
            } else if let Some(mut inside) = Mask::new(self.width, self.height) {
                inside.fill_path(&box_path, FillRule::Winding, aa, ts);
                for (m, i) in mask.data_mut().iter_mut().zip(inside.data()) {
                    *m = ((u16::from(*m) * (255 - u16::from(*i)) + 127) / 255) as u8;
                }
            }
        } else if s.inset {
            return;
        }
        self.target().draw_pixmap(
            out.x0,
            out.y0,
            pix.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            Some(&mask),
        );
    }

    /// Cobertura de lo que pinta `draw` (en una capa aparte, sin recortes).
    fn coverage(&mut self, draw: impl FnOnce(&mut Canvas)) -> Option<Mask> {
        let layer = Pixmap::new(self.width, self.height)?;
        let saved = std::mem::take(&mut self.masks);
        self.layers.push((layer, 1.0));
        draw(self);
        let (layer, _) = self.layers.pop().expect("capa del recorte");
        self.masks = saved;
        Some(Mask::from_pixmap(
            layer.as_ref(),
            tiny_skia::MaskType::Alpha,
        ))
    }

    /// Apila `mask` intersecada con el recorte activo. Se cierra con `PopClip`.
    fn push_mask(&mut self, mut mask: Mask) {
        if let Some(prev) = self.masks.last() {
            for (m, p) in mask.data_mut().iter_mut().zip(prev.data()) {
                *m = ((u16::from(*m) * u16::from(*p) + 127) / 255) as u8;
            }
        }
        self.masks.push(mask);
    }

    /// Recorte con la cobertura de lo que pinta `draw`.
    fn push_clip_drawn(&mut self, draw: impl FnOnce(&mut Canvas)) {
        let mask = self
            .coverage(draw)
            .or_else(|| Mask::new(self.width, self.height));
        if let Some(mask) = mask {
            self.push_mask(mask);
        }
    }

    /// Recorte a los glifos (`background-clip: text`). Los glifos de texto opaco (que se pinta
    /// encima) solo dejan pasar los píxeles que cubren del todo: así el fondo no asoma por el
    /// antialias de sus bordes; los de texto translúcido, su cobertura.
    fn push_clip_text(&mut self, items: &[DisplayItem]) {
        let draw = |c: &mut Canvas, opaque: bool| {
            for item in items {
                if let DisplayItem::Text {
                    glyphs,
                    size,
                    origin,
                    y_fit,
                    color,
                    ..
                } = item
                    && (color.a >= 1.0) == opaque
                {
                    c.text(glyphs, *size, Rgba::BLACK, *origin, *y_fit);
                }
            }
        };
        let soft = self.coverage(|c| draw(c, false));
        let hard = self.coverage(|c| draw(c, true));
        let (Some(mut soft), Some(hard)) = (soft, hard) else {
            return self.push_clip_drawn(|_| {});
        };
        for (s, h) in soft.data_mut().iter_mut().zip(hard.data()) {
            if *h == 255 {
                *s = 255;
            }
        }
        self.push_mask(soft);
    }

    /// Abre un grupo de sombra de texto: una capa nueva, sin recortes.
    fn push_shadow(&mut self, bounds: Rect, blur: f32) {
        let Some(layer) = Pixmap::new(self.width, self.height) else {
            return;
        };
        let saved_masks = std::mem::take(&mut self.masks);
        self.layers.push((layer, 1.0));
        self.shadows.push(ShadowGroup {
            bounds,
            blur,
            saved_masks,
        });
    }

    /// Cierra el grupo: desenfoca la capa (σ = blur / 2, CSS Text Decoration 3 §4) en la zona
    /// de `bounds` y la compone con los recortes que había al abrirlo.
    fn pop_shadow(&mut self) {
        let Some(group) = self.shadows.pop() else {
            return;
        };
        if self.layers.len() <= 1 {
            self.masks = group.saved_masks;
            return;
        }
        let (layer, _) = self.layers.pop().expect("capa del grupo");
        self.masks = group.saved_masks;
        let ts = self.ts();
        let sigma = group.blur / 2.0 * ts.sx.abs();
        let margin = blur_margin(sigma) as i32 + 1;
        let region = self
            .device_rect(&group.bounds)
            .expand(margin)
            .intersect(&self.canvas_rect());
        if region.is_empty() {
            return;
        }
        let (rw, rh) = (region.width() as usize, region.height() as usize);
        let lw = layer.width() as usize;
        let src = layer.data();
        let mut channels = vec![vec![0.0f32; rw * rh]; 4];
        for y in 0..rh {
            for x in 0..rw {
                let k = ((y + region.y0 as usize) * lw + x + region.x0 as usize) * 4;
                for (c, ch) in channels.iter_mut().enumerate() {
                    ch[y * rw + x] = f32::from(src[k + c]) / 255.0;
                }
            }
        }
        for ch in &mut channels {
            blur_channel(ch, rw, rh, sigma);
        }
        let Some(mut pix) = Pixmap::new(region.width(), region.height()) else {
            return;
        };
        let data = pix.data_mut();
        for i in 0..rw * rh {
            let px = [
                channels[0][i],
                channels[1][i],
                channels[2][i],
                channels[3][i],
            ];
            data[i * 4..i * 4 + 4].copy_from_slice(&to_u8(&px));
        }
        let mask = self.masks.last().cloned();
        self.target().draw_pixmap(
            region.x0,
            region.y0,
            pix.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            mask.as_ref(),
        );
    }
}

/// Rasteriza `dl` en un pixmap de `size` px CSS por `scale` (RGBA8 premultiplicado), sobre el
/// color de fondo `bg`.
pub fn render(dl: &DisplayList, size: Size, scale: f32, bg: Rgba) -> Pixmap {
    render_scrolled(dl, size, scale, bg, (0.0, 0.0))
}

/// Como [`render`], con la página desplazada `scroll` px CSS (el origen de pintura queda en
/// `-scroll`).
pub fn render_scrolled(
    dl: &DisplayList,
    size: Size,
    scale: f32,
    bg: Rgba,
    scroll: (f32, f32),
) -> Pixmap {
    let height = frame_size(size, scale).1;
    render_rows(dl, size, scale, bg, scroll, 0..height)
}

/// E4: compone `dl` (sin fondo) sobre `target`, con el origen del cuadro de `size` px CSS en
/// `at` (px de dispositivo de `target`): lo mismo que [`render_scrolled`] con fondo
/// transparente y `draw_pixmap` del resultado en `at`, pero rasterizando y componiendo solo la
/// caja que cubren los ítems dentro del cuadro. Para las capas pequeñas sobre la página
/// (barras de desplazamiento, resaltados, el cursor): un pixmap del tamaño del viewport
/// compuesto entero costaba más que el raster incremental de la página.
pub fn render_over(
    dl: &DisplayList,
    target: &mut Pixmap,
    at: (f32, f32),
    size: Size,
    scale: f32,
    scroll: (f32, f32),
) {
    let (w, h) = frame_size(size, scale);
    let (w, h) = (i64::from(w), i64::from(h));
    let (x0, y0, x1, y1) = match painted_box(dl) {
        Painted::Nothing => return,
        Painted::Unknown => (0, 0, w, h),
        // La caja en px de dispositivo, con 2 px de holgura para el antialiasing (así ningún
        // camino cruza el borde del pixmap y se pinta como en el cuadro entero).
        Painted::Within(r) => {
            let dev = |v: f32, s: f32| (v - s) * scale;
            (
                (dev(r.x, scroll.0).floor() as i64 - 2).max(0),
                (dev(r.y, scroll.1).floor() as i64 - 2).max(0),
                (dev(r.right(), scroll.0).ceil() as i64 + 2).min(w),
                (dev(r.bottom(), scroll.1).ceil() as i64 + 2).min(h),
            )
        }
    };
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let layer = render_region(
        dl,
        scale,
        Rgba::TRANSPARENT,
        scroll,
        (x0 as f32, y0 as f32),
        ((x1 - x0) as u32, (y1 - y0) as u32),
        false,
    );
    target.draw_pixmap(
        (at.0.round() as i64 + x0) as i32,
        (at.1.round() as i64 + y0) as i32,
        layer.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
}

/// Qué pinta una lista, en px CSS sin desplazar (ver [`render_over`]).
enum Painted {
    Nothing,
    Within(Rect),
    /// Hay transformaciones, efectos o ítems sin caja conocida: todo el cuadro.
    Unknown,
}

fn painted_box(dl: &DisplayList) -> Painted {
    let mut b: Option<Rect> = None;
    for item in &dl.items {
        match item {
            // Los recortes y la opacidad no pintan ni mueven nada fuera de lo que encierran.
            DisplayItem::PushClip { .. }
            | DisplayItem::PopClip
            | DisplayItem::PushOpacity(_)
            | DisplayItem::PopOpacity => {}
            _ => match crate::display_list::item_bounds(item) {
                Some(r) => {
                    b = Some(match b {
                        None => r,
                        Some(a) => {
                            let x = a.x.min(r.x);
                            let y = a.y.min(r.y);
                            Rect::new(
                                x,
                                y,
                                a.right().max(r.right()) - x,
                                a.bottom().max(r.bottom()) - y,
                            )
                        }
                    });
                }
                None => return Painted::Unknown,
            },
        }
    }
    b.map_or(Painted::Nothing, Painted::Within)
}

/// El tamaño en px de dispositivo del cuadro de `size` px CSS a escala `scale`.
pub fn frame_size(size: Size, scale: f32) -> (u32, u32) {
    (
        (size.width * scale).ceil().max(1.0) as u32,
        (size.height * scale).ceil().max(1.0) as u32,
    )
}

/// E4: solo las filas `rows` (px de dispositivo) del cuadro de [`render_scrolled`]: un pixmap
/// del ancho del cuadro y `rows.len()` filas. Se pinta con la misma transformación que el
/// cuadro entero más una traslación entera de `-rows.start` px de dispositivo, así que esas
/// filas salen como en el cuadro entero salvo lo que lea píxeles fuera de la banda (los
/// desenfoques: ver [`crate::display_list::ScrollInfo::margin`]).
pub fn render_rows(
    dl: &DisplayList,
    size: Size,
    scale: f32,
    bg: Rgba,
    scroll: (f32, f32),
    rows: std::ops::Range<u32>,
) -> Pixmap {
    let n = rows.end.saturating_sub(rows.start);
    let width = frame_size(size, scale).0;
    render_region(
        dl,
        scale,
        bg,
        scroll,
        (0.0, rows.start as f32),
        (width, n),
        false,
    )
}

/// La región del cuadro en `scroll` que empieza en `start` (px de dispositivo, puede ser
/// negativa) y mide `pixels`. Con `cull`, los ítems de dibujo cuya caja (conservadora, ver
/// `display_list::item_bounds`) no toca el pixmap no se pintan: no cambiarían ningún píxel.
fn render_region(
    dl: &DisplayList,
    scale: f32,
    bg: Rgba,
    scroll: (f32, f32),
    start: (f32, f32),
    pixels: (u32, u32),
    cull: bool,
) -> Pixmap {
    let width = pixels.0.max(1);
    let height = pixels.1.max(1);
    let origin = Transform::from_translate(-start.0, -start.1);
    let mut base = Pixmap::new(width, height).expect("tamaño de pixmap válido");
    base.fill(color(bg));
    let mut canvas = Canvas {
        layers: vec![(base, 1.0)],
        masks: Vec::new(),
        transforms: vec![
            origin
                .pre_scale(scale, scale)
                .pre_translate(-scroll.0, -scroll.1),
        ],
        shadows: Vec::new(),
        width,
        height,
    };
    for item in &dl.items {
        if cull
            && let Some(b) = crate::display_list::item_bounds(item)
            && !device_hits(&canvas.ts(), &b, width, height)
        {
            continue;
        }
        match item {
            DisplayItem::Rect { rect, color, radii } => {
                if let Some(p) = rounded_path(rect, radii) {
                    canvas.fill(&p, *color, !radii.is_zero(), FillRule::Winding);
                }
            }
            DisplayItem::Border {
                rect,
                widths,
                colors,
                styles,
                radii,
            } => canvas.border(rect, widths, colors, styles, radii),
            DisplayItem::Text {
                glyphs,
                size,
                color,
                origin,
                y_fit,
                ..
            } => canvas.text(glyphs, *size, *color, *origin, *y_fit),
            DisplayItem::Line { rect, color, style } => canvas.decoration(rect, *color, *style),
            DisplayItem::Image { rect, image } => canvas.image(rect, image),
            DisplayItem::Widget {
                rect,
                widget,
                color,
            } => canvas.widget(rect, *widget, *color),
            DisplayItem::Path { ops, fill, stroke } => {
                let mut pb = PathBuilder::new();
                for op in ops {
                    match *op {
                        PathOp::MoveTo(x, y) => pb.move_to(x, y),
                        PathOp::LineTo(x, y) => pb.line_to(x, y),
                        PathOp::QuadTo(x1, y1, x, y) => pb.quad_to(x1, y1, x, y),
                        PathOp::CubicTo(x1, y1, x2, y2, x, y) => pb.cubic_to(x1, y1, x2, y2, x, y),
                        PathOp::Close => pb.close(),
                    }
                }
                if let Some(path) = pb.finish() {
                    if let Some(c) = fill {
                        canvas.fill(&path, *c, true, FillRule::Winding);
                    }
                    if let Some((w, c)) = stroke {
                        canvas.stroke(&path, *c, *w);
                    }
                }
            }
            DisplayItem::BackgroundImage {
                area,
                radii,
                tile,
                xs,
                ys,
                image,
            } => canvas.background_image(area, radii, tile, xs.as_deref(), ys.as_deref(), image),
            DisplayItem::Gradient {
                area,
                radii,
                tile,
                xs,
                ys,
                gradient,
            } => canvas.gradient(area, radii, tile, xs.as_deref(), ys.as_deref(), gradient),
            DisplayItem::BoxShadow {
                rect,
                radii,
                shadow,
            } => canvas.box_shadow(rect, radii, shadow),
            DisplayItem::PushShadow { bounds, blur } => canvas.push_shadow(*bounds, *blur),
            DisplayItem::PopShadow => canvas.pop_shadow(),
            DisplayItem::PushClip { rect, radii } => canvas.push_clip(rect, radii),
            DisplayItem::PushClipBorder {
                rect,
                widths,
                styles,
                radii,
            } => {
                let (rect, widths, styles, radii) = (*rect, *widths, *styles, *radii);
                canvas.push_clip_drawn(|c| {
                    c.border(&rect, &widths, &[Rgba::BLACK; 4], &styles, &radii);
                });
            }
            DisplayItem::PushClipText { glyphs } => canvas.push_clip_text(glyphs),
            DisplayItem::PopClip => {
                canvas.masks.pop();
            }
            DisplayItem::PushOpacity(opacity) => {
                let layer = Pixmap::new(width, height).expect("tamaño de pixmap válido");
                canvas.layers.push((layer, *opacity));
            }
            DisplayItem::PopOpacity => {
                if canvas.layers.len() > 1 {
                    let (layer, opacity) = canvas.layers.pop().expect("capa abierta");
                    compose_layer(canvas.target(), &layer, opacity);
                }
            }
            DisplayItem::PushMask => {
                let layer = Pixmap::new(width, height).expect("tamaño de pixmap válido");
                canvas.layers.push((layer, 1.0));
            }
            DisplayItem::PopMask { layers } => canvas.pop_mask(layers), // E7
            DisplayItem::PushTransform { dx, dy } => {
                let ts = canvas.ts().pre_translate(*dx, *dy);
                canvas.transforms.push(ts);
            }
            DisplayItem::PushMatrix([a, b, c, d, e, f]) => {
                let ts = canvas
                    .ts()
                    .pre_concat(Transform::from_row(*a, *b, *c, *d, *e, *f));
                canvas.transforms.push(ts);
            }
            DisplayItem::PushSticky {
                constraints,
                applied,
                viewport,
            } => {
                // E4: el desplazamiento `sticky` con el scroll de este cuadro.
                let view = Rect::new(scroll.0, scroll.1, viewport.width, viewport.height);
                let (dx, dy) = constraints.offset(view);
                let ts = canvas.ts().pre_translate(dx - applied.0, dy - applied.1);
                canvas.transforms.push(ts);
            }
            DisplayItem::PopTransform | DisplayItem::PopFixed | DisplayItem::PopSticky => {
                if canvas.transforms.len() > 1 {
                    canvas.transforms.pop();
                }
            }
            DisplayItem::PushFixed => canvas.transforms.push(origin.pre_scale(scale, scale)),
            // E7: `filter`, `clip-path` y `mix-blend-mode` (ver `crate::filters`).
            DisplayItem::PushEffect => {
                let layer = Pixmap::new(width, height).expect("tamaño de pixmap válido");
                canvas.layers.push((layer, 1.0));
            }
            DisplayItem::PopEffect(fx) => {
                if canvas.layers.len() > 1 {
                    let (layer, _) = canvas.layers.pop().expect("capa abierta");
                    let ts = canvas.ts();
                    let clip = canvas.masks.last().cloned();
                    crate::filters::compose(layer, canvas.target(), fx, ts, clip.as_ref());
                }
            }
        }
    }
    while canvas.layers.len() > 1 {
        let (layer, opacity) = canvas.layers.pop().expect("capa abierta");
        compose_layer(canvas.target(), &layer, opacity);
    }
    canvas.layers.pop().expect("capa base").0
}

/// E4: copia las filas `from` de `src` en `dst` a partir de la fila `at` (mismo ancho).
fn copy_rows(dst: &mut Pixmap, at: u32, src: &Pixmap, from: std::ops::Range<u32>) {
    let stride = dst.width() as usize * 4;
    debug_assert_eq!(src.width(), dst.width());
    let n = from.end.saturating_sub(from.start) as usize;
    let s = from.start as usize * stride;
    let d = at as usize * stride;
    dst.data_mut()[d..d + n * stride].copy_from_slice(&src.data()[s..s + n * stride]);
}

// --- E4: raster en tiles de la página (desplazamiento incremental y en paralelo) ---
//
// El cuadro se arma con tiles horizontales fijos en la página: el tile `k` son las filas de
// dispositivo `[k·T, (k + 1)·T)` de la página (`T` = [`tile_height`]), y se pinta siempre con
// la misma transformación (`página × escala − k·T`), sin importar el scroll. Por eso un tile
// sale igual en cualquier cuadro (salvo lo `fixed` y `sticky`, que dependen del scroll y se
// repintan aparte) y el cuadro de un desplazamiento incremental, el de un raster en paralelo y
// el de un raster entero en tiles son idénticos píxel a píxel. Con el raster de una sola
// pasada ([`render_scrolled`]) difieren solo en el antialiasing de las curvas que cruzan el
// borde de un tile: tiny-skia corta los bordes de un camino en el borde del pixmap y eso
// cambia cómo subdivide la curva (medido en `tests/scroll_raster.rs`).

/// Tamaño de un tile en px de dispositivo (ancho, alto).
pub fn tile_size(scale: f32) -> (u32, u32) {
    (
        (256.0 * scale).ceil().max(64.0) as u32,
        (64.0 * scale).ceil().max(16.0) as u32,
    )
}

/// ¿La caja `b` (px CSS), con la transformación `ts`, toca el pixmap de `w` × `h`?
fn device_hits(ts: &Transform, b: &Rect, w: u32, h: u32) -> bool {
    let mut pts = [
        tiny_skia::Point::from_xy(b.x, b.y),
        tiny_skia::Point::from_xy(b.x + b.width, b.y),
        tiny_skia::Point::from_xy(b.x, b.y + b.height),
        tiny_skia::Point::from_xy(b.x + b.width, b.y + b.height),
    ];
    ts.map_points(&mut pts);
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in pts {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    // Un píxel de holgura por el antialiasing.
    x1 >= -1.0 && y1 >= -1.0 && x0 <= w as f32 + 1.0 && y0 <= h as f32 + 1.0
}

/// El scroll en px de dispositivo, si es entero (si no, no hay tiles alineados).
fn device_scroll(scroll: (f32, f32), scale: f32) -> Option<(i64, i64)> {
    let x = f64::from(scroll.0) * f64::from(scale);
    let y = f64::from(scroll.1) * f64::from(scale);
    let (xi, yi) = (x.round(), y.round());
    ((x - xi).abs() <= 1e-3 && (y - yi).abs() <= 1e-3).then_some((xi as i64, yi as i64))
}

/// Un rectángulo de px de dispositivo del cuadro: `(x, y, ancho, alto)`.
type DevBox = (i64, i64, i64, i64);

/// Copia el rectángulo `(sx, sy, w, h)` de `src` en `dst` en `(dx, dy)`.
fn copy_box(
    dst: &mut Pixmap,
    (dx, dy): (u32, u32),
    src: &Pixmap,
    (sx, sy, w, h): (u32, u32, u32, u32),
) {
    let ds = dst.width() as usize * 4;
    let ss = src.width() as usize * 4;
    let n = w as usize * 4;
    for r in 0..h as usize {
        let d = (dy as usize + r) * ds + dx as usize * 4;
        let s = (sy as usize + r) * ss + sx as usize * 4;
        dst.data_mut()[d..d + n].copy_from_slice(&src.data()[s..s + n]);
    }
}

/// Lo que se pinta de un tile: su índice `(columna, fila)` en la página.
type Tile = (i64, i64);

/// Pinta los tiles (en `threads` hilos) y copia cada uno, recortado al cuadro, en `out`.
#[allow(clippy::too_many_arguments)]
fn paint_tiles(
    out: &mut Pixmap,
    dl: &DisplayList,
    scale: f32,
    bg: Rgba,
    scroll: (f32, f32),
    s_dev: (i64, i64),
    tiles: &[Tile],
    margin: u32,
) {
    let (tw, th) = tile_size(scale);
    let (tw, th) = (i64::from(tw), i64::from(th));
    let m = i64::from(margin);
    let render = |&(c, r): &Tile| {
        // En coordenadas del cuadro: la página está corrida `-s_dev`.
        let x0 = c * tw - s_dev.0 - m;
        let y0 = r * th - s_dev.1 - m;
        render_region(
            dl,
            scale,
            bg,
            scroll,
            (x0 as f32, y0 as f32),
            ((tw + 2 * m) as u32, (th + 2 * m) as u32),
            true,
        )
    };
    let threads = raster_threads_for(tiles.len());
    let pixmaps: Vec<Pixmap> = if threads <= 1 {
        tiles.iter().map(render).collect()
    } else {
        let per = tiles.len().div_ceil(threads);
        std::thread::scope(|s| {
            let handles: Vec<_> = tiles
                .chunks(per)
                .map(|chunk| s.spawn(move || chunk.iter().map(render).collect::<Vec<Pixmap>>()))
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().expect("hilo de raster"))
                .collect::<Vec<Pixmap>>()
        })
    };
    let (fw, fh) = (i64::from(out.width()), i64::from(out.height()));
    for (&(c, r), tile) in tiles.iter().zip(&pixmaps) {
        let x0 = c * tw - s_dev.0;
        let y0 = r * th - s_dev.1;
        let (ax, ay) = (x0.max(0), y0.max(0));
        let (bx, by) = ((x0 + tw).min(fw), (y0 + th).min(fh));
        if ax >= bx || ay >= by {
            continue;
        }
        copy_box(
            out,
            (ax as u32, ay as u32),
            tile,
            (
                (ax - x0 + m) as u32,
                (ay - y0 + m) as u32,
                (bx - ax) as u32,
                (by - ay) as u32,
            ),
        );
    }
}

thread_local! {
    /// Los hilos que puede usar el raster en tiles (ver [`set_raster_threads`]).
    static THREADS: std::cell::Cell<usize> = const { std::cell::Cell::new(1) };
}

/// E4: cuántos hilos usan [`render_tiled`] y [`scroll_frame`] en este hilo (1 por defecto).
pub fn set_raster_threads(n: usize) {
    THREADS.with(|t| t.set(n.max(1)));
}

fn raster_threads_for(tiles: usize) -> usize {
    THREADS.with(std::cell::Cell::get).min(tiles).max(1)
}

/// Los tiles que tocan el rectángulo `b` del cuadro.
fn tiles_for(b: DevBox, s_dev: (i64, i64), tw: i64, th: i64) -> impl Iterator<Item = Tile> {
    let (x, y, w, h) = b;
    let (c0, c1) = (
        (s_dev.0 + x).div_euclid(tw),
        (s_dev.0 + x + w - 1).div_euclid(tw),
    );
    let (r0, r1) = (
        (s_dev.1 + y).div_euclid(th),
        (s_dev.1 + y + h - 1).div_euclid(th),
    );
    (r0..=r1).flat_map(move |r| (c0..=c1).map(move |c| (c, r)))
}

/// El margen de un tile en px de dispositivo: solo si la lista tiene desenfoques (para que
/// se vean continuos de un tile al siguiente; los píxeles no dependen del margen).
fn tile_margin(info: &crate::display_list::ScrollInfo, scale: f32) -> u32 {
    if info.blur > 0.0 {
        (info.margin * scale).ceil() as u32
    } else {
        0
    }
}

/// E4: el cuadro de [`render_scrolled`] pintado en tiles de la página (en los hilos de
/// [`set_raster_threads`]). `None` si el scroll no cae en px de dispositivo enteros o la lista
/// no admite tiles (un `filter`, grupos `fixed`/`sticky` anidados: ver
/// [`crate::display_list::scroll_info`]).
pub fn render_tiled(
    dl: &DisplayList,
    size: Size,
    scale: f32,
    bg: Rgba,
    scroll: (f32, f32),
) -> Option<Pixmap> {
    let s_dev = device_scroll(scroll, scale)?;
    let info = crate::display_list::scroll_info(dl)?;
    let (width, height) = frame_size(size, scale);
    let (tw, th) = tile_size(scale);
    let tiles: Vec<Tile> = tiles_for(
        (0, 0, i64::from(width), i64::from(height)),
        s_dev,
        i64::from(tw),
        i64::from(th),
    )
    .collect();
    let mut out = Pixmap::new(width, height)?;
    paint_tiles(
        &mut out,
        dl,
        scale,
        bg,
        scroll,
        s_dev,
        &tiles,
        tile_margin(&info, scale),
    );
    Some(out)
}

/// E4: el cuadro de [`render_tiled`] en `scroll` a partir del de `prev_scroll` (`prev`, también
/// en tiles), sin pintarlo entero: se corren sus filas lo que se desplazó la página y se pintan
/// solo los tiles de la franja que aparece y de los grupos `fixed` y `sticky` (donde estaban y
/// donde quedan). Da los mismos píxeles que [`render_tiled`] en `scroll`. `None` si no se puede:
/// otro tamaño, desplazamiento horizontal o que no cae en px de dispositivo enteros, más de un
/// cuadro de distancia, o una lista que no admite tiles.
pub fn scroll_frame(
    dl: &DisplayList,
    prev: &Pixmap,
    prev_scroll: (f32, f32),
    size: Size,
    scale: f32,
    bg: Rgba,
    scroll: (f32, f32),
) -> Option<Pixmap> {
    let (width, height) = frame_size(size, scale);
    let (px, py) = device_scroll(prev_scroll, scale)?;
    let s_dev = device_scroll(scroll, scale)?;
    if prev.width() != width || prev.height() != height || s_dev.0 != px {
        return None;
    }
    let di = s_dev.1 - py;
    let (w, h) = (i64::from(width), i64::from(height));
    if di.abs() >= h {
        return None;
    }
    let info = crate::display_list::scroll_info(dl)?;
    // Lo que hay que pintar (rectángulos del cuadro nuevo).
    let mut dirty: Vec<DevBox> = Vec::new();
    if di > 0 {
        dirty.push((0, h - di, w, di));
    } else if di < 0 {
        dirty.push((0, 0, w, -di));
    }
    let dev = |r: &Rect, dx: f32, dy: f32| -> DevBox {
        let x0 = (f64::from(r.x + dx) * f64::from(scale)).floor() as i64 - 1;
        let y0 = (f64::from(r.y + dy) * f64::from(scale)).floor() as i64 - 1;
        let x1 = (f64::from(r.x + r.width + dx) * f64::from(scale)).ceil() as i64 + 1;
        let y1 = (f64::from(r.y + r.height + dy) * f64::from(scale)).ceil() as i64 + 1;
        (x0, y0, x1 - x0, y1 - y0)
    };
    for r in &info.fixed {
        // Quieto en el viewport: donde está y donde dejó la copia corrida del cuadro anterior.
        let (x, y, bw, bh) = dev(r, 0.0, 0.0);
        dirty.push((x, y, bw, bh));
        dirty.push((x, y - di, bw, bh));
    }
    for g in &info.sticky {
        let at = |s: (f32, f32)| {
            let view = Rect::new(s.0, s.1, g.viewport.width, g.viewport.height);
            let (dx, dy) = g.constraints.offset(view);
            dev(&g.rect, dx - g.applied.0 - s.0, dy - g.applied.1 - s.1)
        };
        dirty.push(at(scroll));
        let (x, y, bw, bh) = at(prev_scroll);
        dirty.push((x, y - di, bw, bh));
    }
    let mut out = Pixmap::new(width, height)?;
    let (from, at) = if di >= 0 {
        (di as u32..height, 0)
    } else {
        (0..(h + di) as u32, (-di) as u32)
    };
    copy_rows(&mut out, at, prev, from);
    let (tw, th) = tile_size(scale);
    let mut tiles: Vec<Tile> = Vec::new();
    for (x, y, bw, bh) in dirty {
        // Recortado al cuadro.
        let (ax, ay) = (x.max(0), y.max(0));
        let (bx, by) = ((x + bw).min(w), (y + bh).min(h));
        if ax < bx && ay < by {
            tiles.extend(tiles_for(
                (ax, ay, bx - ax, by - ay),
                s_dev,
                i64::from(tw),
                i64::from(th),
            ));
        }
    }
    tiles.sort_unstable();
    tiles.dedup();
    paint_tiles(
        &mut out,
        dl,
        scale,
        bg,
        scroll,
        s_dev,
        &tiles,
        tile_margin(&info, scale),
    );
    Some(out)
}

/// E4: compone una capa de opacidad (`source-over` con `opacity`) solo en la franja de filas
/// donde tiene algo pintado. Las capas son del tamaño del cuadro y lo que pinta un elemento
/// fuera de la vista las deja vacías: componerlas enteras era la mayor parte del raster al
/// desplazar una página con varias capas. Encontrar la franja cuesta una lectura de memoria;
/// componer, el pipeline de tiny-skia por píxel.
fn compose_layer(target: &mut Pixmap, layer: &Pixmap, opacity: f32) {
    let Some((y0, y1)) = painted_rows(layer) else {
        return;
    };
    let paint = PixmapPaint {
        opacity,
        ..PixmapPaint::default()
    };
    if y0 == 0 && y1 == layer.height() {
        target.draw_pixmap(0, 0, layer.as_ref(), &paint, Transform::identity(), None);
        return;
    }
    let Some(band) = tiny_skia::IntRect::from_xywh(0, y0 as i32, layer.width(), y1 - y0)
        .and_then(|r| layer.clone_rect(r))
    else {
        return;
    };
    target.draw_pixmap(
        0,
        y0 as i32,
        band.as_ref(),
        &paint,
        Transform::identity(),
        None,
    );
}

/// Las filas `[y0, y1)` de `p` con algún píxel no transparente (`None` si está vacía).
fn painted_rows(p: &Pixmap) -> Option<(u32, u32)> {
    let stride = p.width() as usize * 4;
    if stride == 0 {
        return None;
    }
    let rows: Vec<&[u8]> = p.data().chunks_exact(stride).collect();
    let empty = |r: &&[u8]| r.iter().all(|&b| b == 0);
    let y0 = rows.iter().position(|r| !empty(r))?;
    let y1 = rows.iter().rposition(|r| !empty(r))? + 1;
    Some((y0 as u32, y1 as u32))
}

#[cfg(test)]
mod tests {
    use super::nearest_bitmap;

    #[test]
    fn nearest_glyph_bitmap() {
        let b = |ppem| bf_layout::GlyphBitmap {
            ppem,
            x: 0.0,
            y: 0.0,
            width: ppem,
            height: ppem,
            png: std::sync::Arc::from(&b""[..]),
        };
        let strikes = [b(20), b(40), b(160)];
        // El menor que no sea más pequeño; si no hay, el mayor.
        assert_eq!(nearest_bitmap(&strikes, 16.0).map(|b| b.ppem), Some(20));
        assert_eq!(nearest_bitmap(&strikes, 40.0).map(|b| b.ppem), Some(40));
        assert_eq!(nearest_bitmap(&strikes, 41.0).map(|b| b.ppem), Some(160));
        assert_eq!(nearest_bitmap(&strikes, 400.0).map(|b| b.ppem), Some(160));
        assert!(nearest_bitmap(&[], 16.0).is_none());
    }
}
