//! Efectos de pintura: gradientes (CSS Images 3 §3, CSS Images 4 §3.3), desenfoque de sombras
//! (CSS Backgrounds 3 §7, CSS Text Decoration 3 §4) y la forma extendida de las sombras.
//!
//! La resolución de un `<gradient>` de `bf_style` a un [`ResolvedGradient`] (geometría en px del
//! tile y paradas corregidas) se hace al construir la lista de visualización; el muestreo por
//! píxel y el desenfoque, al rasterizar.

use bf_style::properties::gradient::{
    ColorStop, EndingShape, Gradient, GradientKind, LineDirection, RadialSize, StopPosition,
};
use bf_style::values::color::Rgba;

use crate::display_list::{GradientShape, GradientStop, Radii, ResolvedGradient};

// --- Resolución de gradientes ---

/// Radio que sustituye a uno nulo en los radiales degenerados (CSS Images 3 §3.2.2): "un número
/// arbitrariamente pequeño mayor que cero".
const TINY: f32 = 1e-3;
/// Y el "arbitrariamente grande" del otro eje cuando solo uno es nulo.
const HUGE: f32 = 1e6;

/// Resuelve `g` para un tile de `w`×`h` px; `current` es `currentcolor`.
pub fn resolve_gradient(g: &Gradient, w: f32, h: f32, current: Rgba) -> Option<ResolvedGradient> {
    if w <= 0.0 || h <= 0.0 || !w.is_finite() || !h.is_finite() {
        return None;
    }
    // Longitud de la línea (o del rayo) del gradiente, contra la que se resuelven las
    // posiciones de las paradas.
    let (shape, length, angular) = match &g.kind {
        GradientKind::Linear(dir) => {
            let (dx, dy) = linear_direction(*dir, w, h);
            // §3.1.1: |W·sin a| + |H·cos a|.
            let len = (w * dx).abs() + (h * dy).abs();
            let (cx, cy) = (w / 2.0, h / 2.0);
            let start = (cx - dx * len / 2.0, cy - dy * len / 2.0);
            let end = (cx + dx * len / 2.0, cy + dy * len / 2.0);
            (GradientShape::Linear { start, end }, len, false)
        }
        GradientKind::Radial {
            shape,
            size,
            position,
        } => {
            let center = (position.0.resolve(w), position.1.resolve(h));
            let (mut rx, mut ry) = radial_radii(*shape, size, center, w, h);
            // §3.2.2: formas degeneradas.
            match (rx > 0.0, ry > 0.0) {
                (false, false) => (rx, ry) = (TINY, TINY),
                (false, true) => (rx, ry) = (TINY, HUGE),
                (true, false) => (rx, ry) = (HUGE, TINY),
                _ => {}
            }
            // El rayo del gradiente es horizontal: su longitud es el radio horizontal.
            (
                GradientShape::Radial {
                    center,
                    radii: (rx, ry),
                },
                rx,
                false,
            )
        }
        GradientKind::Conic { from, position } => {
            let center = (position.0.resolve(w), position.1.resolve(h));
            (
                GradientShape::Conic {
                    center,
                    from: *from,
                },
                1.0,
                true,
            )
        }
    };
    if length <= 0.0 || !length.is_finite() {
        // Línea de longitud cero (§3.1.1 no lo admite con tiles no vacíos, pero por si acaso):
        // el color de la última parada.
        let last = g.stops.iter().rev().find_map(|s| match s {
            ColorStop::Color { color, .. } => Some(color.resolve(current)),
            ColorStop::Hint(_) => None,
        })?;
        return Some(ResolvedGradient {
            shape,
            stops: vec![GradientStop {
                offset: 0.0,
                color: last,
                hint: None,
            }],
            repeating: false,
        });
    }
    let position = |p: &StopPosition| -> Option<f32> {
        match p {
            StopPosition::Angle(deg) if angular => Some(deg / 360.0),
            StopPosition::Angle(_) => None,
            // En los cónicos los porcentajes son de la vuelta completa.
            StopPosition::Lp(lp) if angular => Some(lp.resolve(1.0)),
            StopPosition::Lp(lp) => Some(lp.resolve(length) / length),
        }
    };
    let stops = fix_up_stops(&g.stops, current, position)?;
    Some(ResolvedGradient {
        shape,
        stops,
        repeating: g.repeating,
    })
}

/// Vector unitario de la línea de un gradiente lineal (x a la derecha, y hacia abajo).
fn linear_direction(dir: LineDirection, w: f32, h: f32) -> (f32, f32) {
    match dir {
        LineDirection::Angle(deg) => {
            let a = deg.to_radians();
            (a.sin(), -a.cos())
        }
        // `to <corner>`: el ángulo cuya perpendicular por el centro pasa por las otras dos
        // esquinas (§3.1.1), es decir, perpendicular a la diagonal entre ellas.
        LineDirection::To { x, y } if x != 0 && y != 0 => {
            let (dx, dy) = (f32::from(x) * h, f32::from(y) * w);
            let n = dx.hypot(dy);
            if n > 0.0 {
                (dx / n, dy / n)
            } else {
                (0.0, 1.0)
            }
        }
        LineDirection::To { x, y } => (f32::from(x), f32::from(y)),
    }
}

/// Radios (horizontal, vertical) de la forma final de un radial (§3.2.1).
fn radial_radii(
    shape: EndingShape,
    size: &RadialSize,
    (cx, cy): (f32, f32),
    w: f32,
    h: f32,
) -> (f32, f32) {
    let (left, right, top, bottom) = (cx.abs(), (w - cx).abs(), cy.abs(), (h - cy).abs());
    let (x_min, x_max) = (left.min(right), left.max(right));
    let (y_min, y_max) = (top.min(bottom), top.max(bottom));
    // Distancias (x, y) a la esquina más cercana y a la más lejana.
    let corners = [(left, top), (right, top), (right, bottom), (left, bottom)];
    let hyp = |c: &(f32, f32)| c.0.hypot(c.1);
    let closest = corners
        .iter()
        .copied()
        .min_by(|a, b| hyp(a).total_cmp(&hyp(b)))
        .unwrap_or_default();
    let farthest = corners
        .iter()
        .copied()
        .max_by(|a, b| hyp(a).total_cmp(&hyp(b)))
        .unwrap_or_default();
    // Elipse que pasa por la esquina `c` con la proporción de `side` (§3.2.1: closest-corner y
    // farthest-corner conservan la relación de closest-side y farthest-side).
    let through = |c: (f32, f32), side: (f32, f32)| {
        if side.0 <= 0.0 || side.1 <= 0.0 {
            return (0.0, 0.0);
        }
        let k = side.1 / side.0;
        let rx = (c.0 * c.0 + (c.1 / k) * (c.1 / k)).sqrt();
        (rx, rx * k)
    };
    let circle = shape == EndingShape::Circle;
    match size {
        RadialSize::ClosestSide if circle => {
            let r = x_min.min(y_min);
            (r, r)
        }
        RadialSize::FarthestSide if circle => {
            let r = x_max.max(y_max);
            (r, r)
        }
        RadialSize::ClosestCorner if circle => {
            let r = hyp(&closest);
            (r, r)
        }
        RadialSize::FarthestCorner if circle => {
            let r = hyp(&farthest);
            (r, r)
        }
        RadialSize::ClosestSide => (x_min, y_min),
        RadialSize::FarthestSide => (x_max, y_max),
        RadialSize::ClosestCorner => through(closest, (x_min, y_min)),
        RadialSize::FarthestCorner => through(farthest, (x_max, y_max)),
        RadialSize::Circle(r) => {
            let r = r.resolve(w).max(0.0);
            (r, r)
        }
        RadialSize::Ellipse(a, b) => (a.resolve(w).max(0.0), b.resolve(h).max(0.0)),
    }
}

/// Corrección de las paradas (CSS Images 3 §3.4.3 y CSS Images 4 §3.5.3): la primera sin
/// posición va al 0 %, la última al 100 %, ninguna posición es menor que una anterior y las
/// que siguen sin posición se reparten uniformemente entre sus vecinas. Las indicaciones se
/// guardan como fracción del tramo entre sus dos paradas.
fn fix_up_stops(
    list: &[ColorStop],
    current: Rgba,
    position: impl Fn(&StopPosition) -> Option<f32>,
) -> Option<Vec<GradientStop>> {
    // (color o None si es indicación, posición).
    let mut items: Vec<(Option<Rgba>, Option<f32>)> = list
        .iter()
        .map(|s| match s {
            ColorStop::Color { color, position: p } => {
                (Some(color.resolve(current)), p.as_ref().and_then(&position))
            }
            ColorStop::Hint(p) => (None, position(p)),
        })
        .collect();
    let color_idx: Vec<usize> = (0..items.len()).filter(|&i| items[i].0.is_some()).collect();
    let (&first, &last) = (color_idx.first()?, color_idx.last()?);
    if items[first].1.is_none() {
        items[first].1 = Some(0.0);
    }
    if items[last].1.is_none() {
        items[last].1 = Some(1.0);
    }
    // Posiciones no decrecientes (paradas e indicaciones).
    let mut max = f32::NEG_INFINITY;
    for item in &mut items {
        if let Some(p) = &mut item.1 {
            if *p < max {
                *p = max;
            }
            max = *p;
        }
    }
    // Paradas sin posición: repartidas entre la parada con posición anterior y la siguiente.
    let mut k = 0;
    while k < color_idx.len() {
        if items[color_idx[k]].1.is_some() {
            k += 1;
            continue;
        }
        let start = k - 1; // la primera siempre tiene posición
        let mut end = k;
        while items[color_idx[end]].1.is_none() {
            end += 1; // la última siempre tiene posición
        }
        let a = items[color_idx[start]].1.unwrap_or(0.0);
        let b = items[color_idx[end]].1.unwrap_or(a);
        let n = (end - start) as f32;
        for (j, &ci) in color_idx.iter().enumerate().take(end).skip(k) {
            items[ci].1 = Some(a + (b - a) * (j - start) as f32 / n);
        }
        k = end + 1;
    }
    let mut out: Vec<GradientStop> = Vec::with_capacity(color_idx.len());
    let mut pending_hint: Option<f32> = None;
    for (color, pos) in items {
        let pos = pos.unwrap_or(0.0);
        match color {
            None => pending_hint = Some(pos),
            Some(color) => {
                if let (Some(prev), Some(hint)) = (out.last_mut(), pending_hint.take()) {
                    let span = pos - prev.offset;
                    prev.hint = Some(if span > 0.0 {
                        (hint - prev.offset) / span
                    } else {
                        0.5
                    });
                }
                out.push(GradientStop {
                    offset: pos,
                    color,
                    hint: None,
                });
            }
        }
    }
    Some(out)
}

// --- Muestreo ---

/// Un color RGBA premultiplicado en [0, 1].
pub type Premul = [f32; 4];

fn premul(c: Rgba) -> Premul {
    let a = c.a.clamp(0.0, 1.0);
    [c.r / 255.0 * a, c.g / 255.0 * a, c.b / 255.0 * a, a]
}

fn mix(a: &Premul, b: &Premul, t: f32) -> Premul {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

/// Evaluador de un [`ResolvedGradient`]: interpola en sRGB premultiplicado (CSS Images 4
/// §3.5.3), lo que da el resultado esperado con `transparent`.
pub struct GradientSampler {
    shape: GradientShape,
    offsets: Vec<f32>,
    colors: Vec<Premul>,
    /// Exponente de la curva de cada tramo: `ln 0.5 / ln H` (1 = lineal); `INFINITY` y
    /// `0` para indicaciones en los extremos.
    exponents: Vec<f32>,
    repeating: bool,
    /// Periodo de repetición; `None` si no se repite o si se pinta como color sólido.
    period: Option<f32>,
    /// Color sólido de un gradiente repetido de periodo cero.
    solid: Option<Premul>,
    /// Inverso del cuadrado de la longitud de la línea (lineales).
    inv_len2: f32,
}

impl GradientSampler {
    pub fn new(g: &ResolvedGradient) -> Self {
        let offsets: Vec<f32> = g.stops.iter().map(|s| s.offset).collect();
        let colors: Vec<Premul> = g.stops.iter().map(|s| premul(s.color)).collect();
        let exponents = g
            .stops
            .iter()
            .map(|s| match s.hint {
                None => 1.0,
                Some(h) if (h - 0.5).abs() < 1e-6 => 1.0,
                Some(h) if h <= 0.0 => 0.0,
                Some(h) if h >= 1.0 => f32::INFINITY,
                Some(h) => 0.5f32.ln() / h.ln(),
            })
            .collect();
        let inv_len2 = match g.shape {
            GradientShape::Linear { start, end } => {
                let (dx, dy) = (end.0 - start.0, end.1 - start.1);
                let l2 = dx * dx + dy * dy;
                if l2 > 0.0 { 1.0 / l2 } else { 0.0 }
            }
            _ => 0.0,
        };
        let mut sampler = GradientSampler {
            shape: g.shape,
            offsets,
            colors,
            exponents,
            repeating: false,
            period: None,
            solid: None,
            inv_len2,
        };
        if g.repeating && sampler.offsets.len() > 1 {
            let first = sampler.offsets[0];
            let last = *sampler.offsets.last().unwrap_or(&first);
            let period = last - first;
            if period > 1e-6 {
                sampler.repeating = true;
                sampler.period = Some(period);
            } else {
                // CSS Images 3 §3.6: periodo cero → el color medio del gradiente con la primera
                // y la última parada movidas al principio y al final de la línea.
                let n = sampler.offsets.len();
                let mut moved = sampler.offsets.clone();
                moved[0] = moved[0].min(0.0);
                moved[n - 1] = moved[n - 1].max(1.0);
                let (lo, hi) = (moved[0], moved[n - 1]);
                let probe = GradientSampler {
                    offsets: moved,
                    ..sampler.clone_stops()
                };
                const SAMPLES: usize = 1024;
                let mut acc = [0.0f32; 4];
                for i in 0..SAMPLES {
                    let t = lo + (hi - lo) * (i as f32 + 0.5) / SAMPLES as f32;
                    let c = probe.at_offset(t);
                    for k in 0..4 {
                        acc[k] += c[k] / SAMPLES as f32;
                    }
                }
                sampler.solid = Some(acc);
            }
        }
        sampler
    }

    fn clone_stops(&self) -> GradientSampler {
        GradientSampler {
            shape: self.shape,
            offsets: self.offsets.clone(),
            colors: self.colors.clone(),
            exponents: self.exponents.clone(),
            repeating: false,
            period: None,
            solid: None,
            inv_len2: self.inv_len2,
        }
    }

    /// Posición sobre la línea del gradiente del punto `(x, y)` del tile.
    fn t(&self, x: f32, y: f32) -> f32 {
        match self.shape {
            GradientShape::Linear { start, end } => {
                ((x - start.0) * (end.0 - start.0) + (y - start.1) * (end.1 - start.1))
                    * self.inv_len2
            }
            GradientShape::Radial { center, radii } => {
                let (dx, dy) = ((x - center.0) / radii.0, (y - center.1) / radii.1);
                (dx * dx + dy * dy).sqrt()
            }
            GradientShape::Conic { center, from } => {
                // 0 = arriba, sentido horario (y crece hacia abajo).
                let deg = (x - center.0).atan2(center.1 - y).to_degrees();
                ((deg - from) / 360.0).rem_euclid(1.0)
            }
        }
    }

    /// Color premultiplicado en `(x, y)` del tile.
    pub fn at(&self, x: f32, y: f32) -> Premul {
        if let Some(c) = self.solid {
            return c;
        }
        let mut t = self.t(x, y);
        if self.repeating
            && let Some(p) = self.period
        {
            let first = self.offsets[0];
            t = first + (t - first).rem_euclid(p);
        }
        self.at_offset(t)
    }

    fn at_offset(&self, t: f32) -> Premul {
        let n = self.offsets.len();
        if n == 0 {
            return [0.0; 4];
        }
        if t.is_nan() || t < self.offsets[0] {
            return self.colors[0];
        }
        if t >= self.offsets[n - 1] {
            return self.colors[n - 1];
        }
        // El último tramo cuyo inicio es ≤ t (en las paradas duras gana la de después).
        let i = self.offsets.partition_point(|&o| o <= t).saturating_sub(1);
        let (a, b) = (self.offsets[i], self.offsets[i + 1]);
        if b <= a {
            return self.colors[i + 1];
        }
        let p = (t - a) / (b - a);
        let e = self.exponents[i];
        let w = if e == 1.0 {
            p
        } else if e == 0.0 {
            // Indicación en el inicio del tramo: ya es el color siguiente.
            1.0
        } else if e.is_infinite() {
            0.0
        } else {
            p.powf(e)
        };
        mix(&self.colors[i], &self.colors[i + 1], w)
    }
}

/// Un píxel premultiplicado de 8 bits (los canales nunca superan el alfa).
pub fn to_u8(c: &Premul) -> [u8; 4] {
    let a = (c[3].clamp(0.0, 1.0) * 255.0).round();
    let ch = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round().min(a) as u8;
    [ch(c[0]), ch(c[1]), ch(c[2]), a as u8]
}

// --- Desenfoque ---

/// Tamaños de las tres pasadas de box blur que aproximan un gaussiano de desviación `sigma`
/// (px de dispositivo), como `feGaussianBlur` (Filter Effects 1 §9.15): `(izquierda, derecha)`
/// de cada caja.
pub fn box_passes(sigma: f32) -> Option<[(usize, usize); 3]> {
    if sigma <= 0.0 {
        return None;
    }
    let d = (sigma * 3.0 * (2.0 * std::f32::consts::PI).sqrt() / 4.0 + 0.5).floor() as usize;
    if d <= 1 {
        return None;
    }
    Some(if d % 2 == 1 {
        let r = (d - 1) / 2;
        [(r, r); 3]
    } else {
        let h = d / 2;
        [(h, h - 1), (h - 1, h), (h, h)]
    })
}

/// Margen (px de dispositivo) que alcanza el desenfoque de desviación `sigma`.
pub fn blur_margin(sigma: f32) -> usize {
    box_passes(sigma).map_or(0, |p| p.iter().map(|(l, r)| l.max(r)).sum::<usize>() + 1)
}

/// Una pasada de caja sobre `src` (longitud `len`, paso `stride`), escribiendo en `dst`. Fuera
/// del búfer los valores son 0.
fn box_line(src: &[f32], dst: &mut [f32], len: usize, (l, r): (usize, usize)) {
    let size = (l + r + 1) as f32;
    let mut acc = 0.0f32;
    // Ventana [i - l, i + r].
    for v in src.iter().take(r.min(len)) {
        acc += v;
    }
    for i in 0..len {
        if i + r < len {
            acc += src[i + r];
        }
        dst[i] = acc / size;
        if i >= l {
            acc -= src[i - l];
        }
    }
}

/// Desenfoca en sitio un canal de `w`×`h` con tres pasadas horizontales y tres verticales.
pub fn blur_channel(data: &mut [f32], w: usize, h: usize, sigma: f32) {
    let Some(passes) = box_passes(sigma) else {
        return;
    };
    let mut line = vec![0.0f32; w.max(h)];
    let mut out = vec![0.0f32; w.max(h)];
    for y in 0..h {
        let row = &mut data[y * w..(y + 1) * w];
        line[..w].copy_from_slice(row);
        for p in passes {
            box_line(&line[..w], &mut out[..w], w, p);
            line[..w].copy_from_slice(&out[..w]);
        }
        row.copy_from_slice(&line[..w]);
    }
    for x in 0..w {
        for y in 0..h {
            line[y] = data[y * w + x];
        }
        for p in passes {
            box_line(&line[..h], &mut out[..h], h, p);
            line[..h].copy_from_slice(&out[..h]);
        }
        for y in 0..h {
            data[y * w + x] = line[y];
        }
    }
}

// --- Forma de las sombras ---

/// Radios de la forma de una sombra con extensión `spread` (CSS Backgrounds 3 §7.2): crecen (o
/// decrecen) con la extensión; si el radio es menor que la extensión, ésta se multiplica antes
/// por `1 + (r − 1)³`, con `r` = radio / extensión, para que las esquinas rectas sigan rectas.
/// Una esquina que ya ocupa la mitad de su lado (`border-radius: 50%`) crece sin esa
/// corrección, para que un círculo siga siendo un círculo (resolución del CSSWG de 2022 que
/// comprueba `box-shadow-border-radius-001`).
pub fn spread_radii(radii: &Radii, spread: f32, size: (f32, f32)) -> Radii {
    let adjust = |r: f32, side: f32| {
        if r <= 0.0 {
            return 0.0;
        }
        if spread >= 0.0 && 2.0 * r >= side - 0.01 {
            return r + spread;
        }
        if spread >= 0.0 {
            if r >= spread {
                r + spread
            } else {
                let ratio = r / spread;
                r + spread * (1.0 + (ratio - 1.0).powi(3))
            }
        } else {
            (r + spread).max(0.0)
        }
    };
    let mut out = *radii;
    for c in &mut out.0 {
        c.0 = adjust(c.0, size.0);
        c.1 = adjust(c.1, size.1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bf_style::properties::gradient::GradientKind;
    use bf_style::values::{Color, LengthPercentage};

    fn stop(c: Rgba, p: Option<f32>) -> ColorStop {
        ColorStop::Color {
            color: Color::Rgba(c),
            position: p.map(|p| StopPosition::Lp(LengthPercentage::Percentage(p))),
        }
    }

    fn red() -> Rgba {
        Rgba::rgb(255, 0, 0)
    }
    fn blue() -> Rgba {
        Rgba::rgb(0, 0, 255)
    }

    #[test]
    fn paradas_sin_posicion_se_reparten() {
        let g = Gradient {
            kind: GradientKind::Linear(LineDirection::Angle(180.0)),
            repeating: false,
            stops: vec![
                stop(red(), None),
                stop(blue(), None),
                stop(red(), Some(20.0)),
                stop(blue(), Some(10.0)),
                stop(red(), None),
            ],
        };
        let r = resolve_gradient(&g, 100.0, 100.0, Rgba::BLACK).unwrap();
        let o: Vec<f32> = r.stops.iter().map(|s| s.offset).collect();
        assert_eq!(o, vec![0.0, 0.1, 0.2, 0.2, 1.0]);
    }

    #[test]
    fn esquina_perpendicular_a_la_diagonal() {
        let (dx, dy) = linear_direction(LineDirection::To { x: 1, y: -1 }, 200.0, 100.0);
        // Perpendicular a la diagonal (200, 100).
        assert!((dx * 200.0 + dy * 100.0).abs() < 1e-3);
        assert!(dx > 0.0 && dy < 0.0);
    }

    #[test]
    fn indicacion_en_un_cuarto() {
        let g = Gradient {
            kind: GradientKind::Linear(LineDirection::Angle(90.0)),
            repeating: false,
            stops: vec![
                stop(red(), None),
                ColorStop::Hint(StopPosition::Lp(LengthPercentage::Percentage(25.0))),
                stop(blue(), None),
            ],
        };
        let r = resolve_gradient(&g, 100.0, 10.0, Rgba::BLACK).unwrap();
        let s = GradientSampler::new(&r);
        // En la posición de la indicación el color es la mitad.
        let c = s.at(25.0, 5.0);
        assert!(
            (c[0] - 0.5).abs() < 0.01 && (c[2] - 0.5).abs() < 0.01,
            "{c:?}"
        );
    }

    #[test]
    fn repetido_de_periodo_cero_es_el_promedio() {
        let g = Gradient {
            kind: GradientKind::Linear(LineDirection::Angle(180.0)),
            repeating: true,
            stops: vec![stop(red(), Some(50.0)), stop(blue(), Some(50.0))],
        };
        let r = resolve_gradient(&g, 10.0, 10.0, Rgba::BLACK).unwrap();
        let s = GradientSampler::new(&r);
        let c = s.at(3.0, 3.0);
        assert!(
            (c[0] - 0.5).abs() < 0.01 && (c[2] - 0.5).abs() < 0.01,
            "{c:?}"
        );
    }

    #[test]
    fn radios_de_extension() {
        let r = spread_radii(&Radii([(10.0, 10.0); 4]), 5.0, (100.0, 100.0));
        assert_eq!(r.0[0], (15.0, 15.0));
        let r = spread_radii(&Radii([(0.0, 0.0); 4]), 5.0, (100.0, 100.0));
        assert_eq!(r.0[0], (0.0, 0.0));
    }

    #[test]
    fn desenfoque_conserva_la_masa() {
        let (w, h) = (40, 40);
        let mut d = vec![0.0f32; w * h];
        for y in 15..25 {
            for x in 15..25 {
                d[y * w + x] = 1.0;
            }
        }
        blur_channel(&mut d, w, h, 3.0);
        let sum: f32 = d.iter().sum();
        assert!((sum - 100.0).abs() < 0.5, "{sum}");
        assert!(d[20 * w + 20] < 1.0 && d[20 * w + 20] > 0.5);
    }
}
