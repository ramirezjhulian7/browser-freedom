//! Filtros SVG (Filter Effects Module Level 1) sobre un árbol de `bf-svg`: el grupo filtrado
//! se pinta en una imagen aparte del tamaño de la región del filtro (en px de dispositivo) y
//! la cadena de primitivas trabaja sobre RGBA premultiplicado en `f32`, en sRGB o en
//! linearRGB según `color-interpolation-filters` (§10). Las magnitudes en espacio de usuario
//! (`stdDeviation`, `dx`, …) se llevan al dispositivo con la transformación actual.

use bf_svg::{BlendMode, CompositeOp, EdgeMode, Filter, FilterInput, FilterKind, Rgba};
use tiny_skia::{Pixmap, Transform};

/// Píxel RGBA premultiplicado, cada canal en 0–1.
type Px = [f32; 4];

/// Margen máximo (px) que se pinta fuera del lienzo para lo que un desenfoque o un
/// desplazamiento puede traer hacia dentro.
const MAX_MARGIN: f32 = 256.0;

/// Imagen intermedia de la cadena de filtros, del tamaño del área del filtro.
#[derive(Clone)]
struct Img {
    w: usize,
    h: usize,
    px: Vec<Px>,
    /// Los colores están en linearRGB (si no, en sRGB).
    linear: bool,
}

/// Rectángulo de píxeles de una imagen: `[x0, x1) × [y0, y1)`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Area {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

impl Img {
    fn new(w: usize, h: usize, linear: bool) -> Img {
        Img {
            w,
            h,
            px: vec![[0.0; 4]; w * h],
            linear,
        }
    }

    fn full(&self) -> Area {
        Area {
            x0: 0,
            y0: 0,
            x1: self.w,
            y1: self.h,
        }
    }

    /// Pone a cero lo que queda fuera de `a`.
    fn clip(&mut self, a: Area) {
        for y in 0..self.h {
            for x in 0..self.w {
                if x < a.x0 || x >= a.x1 || y < a.y0 || y >= a.y1 {
                    self.px[y * self.w + x] = [0.0; 4];
                }
            }
        }
    }

    /// La misma imagen en el espacio de color pedido.
    fn to_space(&self, linear: bool) -> Img {
        if self.linear == linear {
            return self.clone();
        }
        let conv: fn(f32) -> f32 = if linear { to_linear } else { to_srgb };
        let px = self
            .px
            .iter()
            .map(|&p| {
                let a = p[3];
                if a <= 0.0 {
                    return [0.0; 4];
                }
                [
                    conv(p[0] / a) * a,
                    conv(p[1] / a) * a,
                    conv(p[2] / a) * a,
                    a,
                ]
            })
            .collect();
        Img {
            w: self.w,
            h: self.h,
            px,
            linear,
        }
    }
}

/// sRGB → linearRGB (§10, con la función de transferencia de sRGB).
fn to_linear(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// linearRGB → sRGB.
fn to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Caja en el dispositivo (redondeada al píxel) de un rectángulo de usuario.
fn device_rect(r: (f32, f32, f32, f32), ts: &Transform) -> (f32, f32, f32, f32) {
    let corners = [
        (r.0, r.1),
        (r.0 + r.2, r.1),
        (r.0, r.1 + r.3),
        (r.0 + r.2, r.1 + r.3),
    ];
    let mut x0 = f32::INFINITY;
    let mut y0 = f32::INFINITY;
    let mut x1 = f32::NEG_INFINITY;
    let mut y1 = f32::NEG_INFINITY;
    for (x, y) in corners {
        let dx = ts.sx * x + ts.kx * y + ts.tx;
        let dy = ts.ky * x + ts.sy * y + ts.ty;
        x0 = x0.min(dx);
        y0 = y0.min(dy);
        x1 = x1.max(dx);
        y1 = y1.max(dy);
    }
    (x0.round(), y0.round(), x1.round(), y1.round())
}

/// Escala de la transformación en cada eje (longitud de las columnas).
fn scales(ts: &Transform) -> (f32, f32) {
    (ts.sx.hypot(ts.ky), ts.kx.hypot(ts.sy))
}

/// Un vector de usuario en px de dispositivo.
fn device_vec(ts: &Transform, dx: f32, dy: f32) -> (f32, f32) {
    (ts.sx * dx + ts.kx * dy, ts.ky * dx + ts.sy * dy)
}

/// Aplica `filter` al grupo que `paint` pinta (con la transformación que recibe) sobre un
/// lienzo de `canvas_w`×`canvas_h`; `ts` es la transformación del grupo. Devuelve la imagen
/// filtrada y la posición de su esquina en el lienzo, o `None` si no queda nada que pintar.
pub(super) fn apply(
    f: &Filter,
    canvas_w: u32,
    canvas_h: u32,
    ts: Transform,
    paint: impl FnOnce(&mut Pixmap, Transform),
) -> Option<(Pixmap, i32, i32)> {
    let (sx, sy) = scales(&ts);
    // Lo que una primitiva puede traer desde fuera del lienzo: desenfoques y desplazamientos.
    let margin: f32 = f
        .primitives
        .iter()
        .map(|p| match p.kind {
            FilterKind::GaussianBlur { std_dev, .. } => 3.0 * (std_dev.0 * sx).max(std_dev.1 * sy),
            FilterKind::Offset { dx, dy } => {
                let (x, y) = device_vec(&ts, dx, dy);
                x.abs().max(y.abs())
            }
            FilterKind::DropShadow {
                std_dev, dx, dy, ..
            } => {
                let (x, y) = device_vec(&ts, dx, dy);
                3.0 * (std_dev.0 * sx).max(std_dev.1 * sy) + x.abs().max(y.abs())
            }
            _ => 0.0,
        })
        .filter(|m| m.is_finite())
        .sum::<f32>()
        .min(MAX_MARGIN)
        .ceil();
    // El área de trabajo: la región del filtro (que recorta la entrada, §9.2), limitada al
    // lienzo más el margen.
    let (rx0, ry0, rx1, ry1) = device_rect(f.region, &ts);
    let ax0 = rx0.max(-margin);
    let ay0 = ry0.max(-margin);
    let ax1 = rx1.min(canvas_w as f32 + margin);
    let ay1 = ry1.min(canvas_h as f32 + margin);
    if !(ax1 > ax0 && ay1 > ay0) {
        return None;
    }
    let (ox, oy) = (ax0 as i32, ay0 as i32);
    let (w, h) = ((ax1 - ax0) as u32, (ay1 - ay0) as u32);
    let mut source = Pixmap::new(w, h)?;
    paint(&mut source, ts.post_translate(-(ox as f32), -(oy as f32)));
    let mut src = Img::new(w as usize, h as usize, false);
    for (p, c) in src.px.iter_mut().zip(source.data().as_chunks::<4>().0) {
        *p = c.map(|v| v as f32 / 255.0);
    }
    // Subregión de una primitiva en píxeles del área.
    let area_of = |r: (f32, f32, f32, f32)| -> Area {
        if !(r.2 > 0.0 && r.3 > 0.0) {
            return Area {
                x0: 0,
                y0: 0,
                x1: 0,
                y1: 0,
            };
        }
        let (x0, y0, x1, y1) = device_rect(r, &ts);
        let cx = |v: f32| (v - ax0).clamp(0.0, w as f32) as usize;
        let cy = |v: f32| (v - ay0).clamp(0.0, h as f32) as usize;
        Area {
            x0: cx(x0),
            y0: cy(y0),
            x1: cx(x1),
            y1: cy(y1),
        }
    };
    let mut results: Vec<(Img, Area)> = Vec::with_capacity(f.primitives.len());
    for p in &f.primitives {
        let sub = area_of(p.subregion);
        let input = |i: FilterInput, linear: bool| -> (Img, Area) {
            match i {
                FilterInput::SourceGraphic => (src.to_space(linear), src.full()),
                FilterInput::SourceAlpha => {
                    let mut a = Img::new(src.w, src.h, linear);
                    for (o, s) in a.px.iter_mut().zip(&src.px) {
                        o[3] = s[3];
                    }
                    (a, src.full())
                }
                FilterInput::Primitive(k) => match results.get(k) {
                    Some((img, area)) => (img.to_space(linear), *area),
                    None => (Img::new(src.w, src.h, linear), src.full()),
                },
                FilterInput::Transparent => (Img::new(src.w, src.h, linear), src.full()),
            }
        };
        let lin = p.linear_rgb;
        let mut out = match &p.kind {
            FilterKind::GaussianBlur { std_dev, edge } => {
                let (img, area) = input(p.input, lin);
                gaussian_blur(&img, area, (std_dev.0 * sx, std_dev.1 * sy), *edge)
            }
            FilterKind::Offset { dx, dy } => {
                // `color-interpolation-filters` no le afecta (§10): se queda en su espacio.
                let (img, _) = input(p.input, src.linear);
                let (dx, dy) = device_vec(&ts, *dx, *dy);
                offset(&img, dx.round() as isize, dy.round() as isize)
            }
            FilterKind::Passthrough => input(p.input, lin).0,
            FilterKind::Flood { color } => {
                let mut img = Img::new(src.w, src.h, false);
                let c = premul(*color);
                img.px.fill(c);
                img
            }
            FilterKind::Merge { inputs } => {
                let mut img = Img::new(src.w, src.h, lin);
                for &i in inputs {
                    let (top, _) = input(i, lin);
                    for (o, t) in img.px.iter_mut().zip(&top.px) {
                        *o = over(*t, *o);
                    }
                }
                img
            }
            FilterKind::ColorMatrix { matrix } => {
                let (mut img, _) = input(p.input, lin);
                for px in &mut img.px {
                    *px = color_matrix(matrix, *px);
                }
                img
            }
            FilterKind::Composite { op, in2 } => {
                let (a, _) = input(p.input, lin);
                let (b, _) = input(*in2, lin);
                let mut img = a;
                for (o, b) in img.px.iter_mut().zip(&b.px) {
                    *o = composite(*op, *o, *b);
                }
                img
            }
            FilterKind::Blend { mode, in2 } => {
                let (a, _) = input(p.input, lin);
                let (b, _) = input(*in2, lin);
                let mut img = a;
                for (o, b) in img.px.iter_mut().zip(&b.px) {
                    *o = blend(*mode, *o, *b);
                }
                img
            }
            FilterKind::DropShadow {
                std_dev,
                dx,
                dy,
                color,
            } => {
                let (img, area) = input(p.input, lin);
                let mut alpha = Img::new(img.w, img.h, lin);
                for (o, s) in alpha.px.iter_mut().zip(&img.px) {
                    o[3] = s[3];
                }
                let blurred = gaussian_blur(
                    &alpha,
                    area,
                    (std_dev.0 * sx, std_dev.1 * sy),
                    EdgeMode::None,
                );
                let (dx, dy) = device_vec(&ts, *dx, *dy);
                let shadow = offset(&blurred, dx.round() as isize, dy.round() as isize);
                let mut c = *color;
                if lin {
                    c.r = to_linear(c.r / 255.0) * 255.0;
                    c.g = to_linear(c.g / 255.0) * 255.0;
                    c.b = to_linear(c.b / 255.0) * 255.0;
                }
                let c = premul(c);
                let mut out = img;
                for (o, s) in out.px.iter_mut().zip(&shadow.px) {
                    let a = s[3];
                    *o = over(*o, [c[0] * a, c[1] * a, c[2] * a, c[3] * a]);
                }
                out
            }
        };
        // El resultado de cada primitiva se recorta a su subregión (§9.4).
        out.clip(sub);
        results.push((out, sub));
    }
    let (last, _) = results.pop()?;
    let last = last.to_space(false);
    let mut pixmap = Pixmap::new(w, h)?;
    for (c, p) in pixmap
        .data_mut()
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(&last.px)
    {
        let a = (p[3].clamp(0.0, 1.0) * 255.0).round() as u8;
        let ch = |v: f32| ((v.clamp(0.0, 1.0) * 255.0).round() as u8).min(a);
        c[0] = ch(p[0]);
        c[1] = ch(p[1]);
        c[2] = ch(p[2]);
        c[3] = a;
    }
    Some((pixmap, ox, oy))
}

/// Un color de `bf-svg` (canales 0–255, alfa 0–1) premultiplicado en 0–1.
fn premul(c: Rgba) -> Px {
    let a = c.a.clamp(0.0, 1.0);
    [
        (c.r / 255.0).clamp(0.0, 1.0) * a,
        (c.g / 255.0).clamp(0.0, 1.0) * a,
        (c.b / 255.0).clamp(0.0, 1.0) * a,
        a,
    ]
}

/// `top` sobre `bottom` (Porter-Duff *source-over*, premultiplicado).
fn over(top: Px, bottom: Px) -> Px {
    let k = 1.0 - top[3];
    [
        top[0] + bottom[0] * k,
        top[1] + bottom[1] * k,
        top[2] + bottom[2] * k,
        top[3] + bottom[3] * k,
    ]
}

/// `feColorMatrix` (§9.6) sobre el color no premultiplicado, con el resultado acotado.
fn color_matrix(m: &[f32; 20], p: Px) -> Px {
    let a = p[3];
    let (r, g, b) = if a > 0.0 {
        (p[0] / a, p[1] / a, p[2] / a)
    } else {
        (0.0, 0.0, 0.0)
    };
    let row = |i: usize| {
        (m[i * 5] * r + m[i * 5 + 1] * g + m[i * 5 + 2] * b + m[i * 5 + 3] * a + m[i * 5 + 4])
            .clamp(0.0, 1.0)
    };
    let na = row(3);
    [row(0) * na, row(1) * na, row(2) * na, na]
}

/// `feComposite` (§9.8): `a` (`in`) con `b` (`in2`), premultiplicados.
fn composite(op: CompositeOp, a: Px, b: Px) -> Px {
    let (aa, ba) = (a[3], b[3]);
    let o: Px = std::array::from_fn(|i| {
        let (x, y) = (a[i], b[i]);
        match op {
            CompositeOp::Over => x + y * (1.0 - aa),
            CompositeOp::In => x * ba,
            CompositeOp::Out => x * (1.0 - ba),
            CompositeOp::Atop => x * ba + y * (1.0 - aa),
            CompositeOp::Xor => x * (1.0 - ba) + y * (1.0 - aa),
            CompositeOp::Lighter => (x + y).min(1.0),
            CompositeOp::Arithmetic { k1, k2, k3, k4 } => {
                (k1 * x * y + k2 * x + k3 * y + k4).clamp(0.0, 1.0)
            }
        }
    });
    // Premultiplicado válido: ningún canal por encima del alfa.
    [o[0].min(o[3]), o[1].min(o[3]), o[2].min(o[3]), o[3]]
}

/// `feBlend` (§9.5) con las fórmulas de Compositing and Blending 1 §10 sobre premultiplicado:
/// `s` (`in`) encima de `b` (`in2`).
fn blend(mode: BlendMode, s: Px, b: Px) -> Px {
    let (sa, ba) = (s[3], b[3]);
    std::array::from_fn(|i| {
        if i == 3 {
            return sa + ba - sa * ba;
        }
        let (cs, cb) = (s[i], b[i]);
        let mixed = match mode {
            BlendMode::Normal => cs * ba,
            BlendMode::Multiply => cs * cb,
            BlendMode::Screen => cs * ba + cb * sa - cs * cb,
            BlendMode::Darken => (cs * ba).min(cb * sa),
            BlendMode::Lighten => (cs * ba).max(cb * sa),
        };
        (cs * (1.0 - ba) + cb * (1.0 - sa) + mixed).clamp(0.0, 1.0)
    })
}

/// Desplaza la imagen `(dx, dy)` píxeles (`feOffset`, §9.18); lo que entra es transparente.
fn offset(img: &Img, dx: isize, dy: isize) -> Img {
    let mut out = Img::new(img.w, img.h, img.linear);
    let (w, h) = (img.w as isize, img.h as isize);
    for y in 0..h {
        let sy = y - dy;
        if sy < 0 || sy >= h {
            continue;
        }
        for x in 0..w {
            let sx = x - dx;
            if sx < 0 || sx >= w {
                continue;
            }
            out.px[(y * w + x) as usize] = img.px[(sy * w + sx) as usize];
        }
    }
    out
}

/// Pasos de un eje de `feGaussianBlur` (§9.14): con desviación ≥ 2 px, tres cajas de
/// tamaño `d = floor(s · 3·√(2π)/4 + 0.5)` (la aproximación que permite la spec: con `d`
/// par, dos cajas de `d` desplazadas medio píxel y una de `d + 1`); por debajo, el núcleo
/// gaussiano exacto.
enum Kernel {
    /// Cajas, cada una como el rango de desplazamientos `[lo, hi]` que promedia.
    Boxes(Vec<(isize, isize)>),
    /// Pesos de `-r` a `r`.
    Gauss(Vec<f32>),
}

fn kernel(s: f32) -> Option<Kernel> {
    if !s.is_finite() || s <= 0.0 {
        return None;
    }
    if s >= 2.0 {
        let d = (s * 3.0 * (2.0 * std::f32::consts::PI).sqrt() / 4.0 + 0.5).floor() as isize;
        let d = d.clamp(1, 2 * MAX_MARGIN as isize);
        let boxes = if d % 2 == 1 {
            let r = (d - 1) / 2;
            vec![(-r, r); 3]
        } else {
            let r = d / 2;
            vec![(-r, r - 1), (-r + 1, r), (-r, r)]
        };
        return Some(Kernel::Boxes(boxes));
    }
    let r = (3.0 * s).ceil() as isize;
    let mut w: Vec<f32> = (-r..=r)
        .map(|i| (-(i * i) as f32 / (2.0 * s * s)).exp())
        .collect();
    let sum: f32 = w.iter().sum();
    for v in &mut w {
        *v /= sum;
    }
    Some(Kernel::Gauss(w))
}

/// Un píxel de la línea `line` con el modo de borde `edge` fuera de ella.
fn sample(line: &[Px], j: isize, edge: EdgeMode) -> Px {
    let n = line.len() as isize;
    if (0..n).contains(&j) {
        return line[j as usize];
    }
    match edge {
        EdgeMode::None => [0.0; 4],
        EdgeMode::Duplicate => line[j.clamp(0, n - 1) as usize],
        EdgeMode::Wrap => line[j.rem_euclid(n) as usize],
    }
}

/// Aplica el núcleo a una línea.
fn blur_line(line: &mut [Px], k: &Kernel, edge: EdgeMode) {
    let n = line.len();
    if n == 0 {
        return;
    }
    let mut tmp = vec![[0.0f32; 4]; n];
    match k {
        Kernel::Boxes(boxes) => {
            for &(lo, hi) in boxes {
                let inv = 1.0 / (hi - lo + 1) as f32;
                let mut sum = [0.0f32; 4];
                for j in lo..=hi {
                    let p = sample(line, j, edge);
                    for (s, v) in sum.iter_mut().zip(p) {
                        *s += v;
                    }
                }
                for (i, t) in tmp.iter_mut().enumerate() {
                    *t = sum.map(|s| (s * inv).max(0.0));
                    let i = i as isize;
                    let add = sample(line, i + hi + 1, edge);
                    let sub = sample(line, i + lo, edge);
                    for ((s, a), b) in sum.iter_mut().zip(add).zip(sub) {
                        *s += a - b;
                    }
                }
                line.copy_from_slice(&tmp);
            }
        }
        Kernel::Gauss(w) => {
            let r = (w.len() / 2) as isize;
            for (i, t) in tmp.iter_mut().enumerate() {
                let mut acc = [0.0f32; 4];
                for (k, wk) in w.iter().enumerate() {
                    let p = sample(line, i as isize + k as isize - r, edge);
                    for (a, v) in acc.iter_mut().zip(p) {
                        *a += v * wk;
                    }
                }
                *t = acc;
            }
            line.copy_from_slice(&tmp);
        }
    }
}

/// `feGaussianBlur` (§9.14) con desviaciones `(sx, sy)` en px de dispositivo. Con
/// `edgeMode` `duplicate` o `wrap`, los bordes son los de `area` (la subregión de la
/// entrada); con `none`, fuera de la imagen hay ceros.
fn gaussian_blur(img: &Img, area: Area, (sx, sy): (f32, f32), edge: EdgeMode) -> Img {
    let mut out = img.clone();
    let a = if edge == EdgeMode::None {
        img.full()
    } else {
        area
    };
    if a.x1 <= a.x0 || a.y1 <= a.y0 {
        return out;
    }
    if let Some(k) = kernel(sx) {
        for y in a.y0..a.y1 {
            let row = &mut out.px[y * img.w + a.x0..y * img.w + a.x1];
            blur_line(row, &k, edge);
        }
    }
    if let Some(k) = kernel(sy) {
        let mut col = vec![[0.0f32; 4]; a.y1 - a.y0];
        for x in a.x0..a.x1 {
            for (j, y) in (a.y0..a.y1).enumerate() {
                col[j] = out.px[y * img.w + x];
            }
            blur_line(&mut col, &k, edge);
            for (j, y) in (a.y0..a.y1).enumerate() {
                out.px[y * img.w + x] = col[j];
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caja_impar_y_par() {
        // s = 2 → d = floor(2·1.88 + 0.5) = 4 (par): dos cajas de 4 y una de 5.
        let Some(Kernel::Boxes(b)) = kernel(2.0) else {
            panic!("se esperaban cajas");
        };
        assert_eq!(b, vec![(-2, 1), (-1, 2), (-2, 2)]);
        // s = 3 → d = floor(3·1.88 + 0.5) = 6; s = 2.5 → d = 5 (impar).
        let Some(Kernel::Boxes(b)) = kernel(2.5) else {
            panic!("se esperaban cajas");
        };
        assert_eq!(b, vec![(-2, 2); 3]);
        assert!(kernel(0.0).is_none());
        assert!(matches!(kernel(1.0), Some(Kernel::Gauss(_))));
    }

    #[test]
    fn desenfoque_conserva_la_masa() {
        let mut img = Img::new(41, 1, false);
        img.px[20] = [1.0, 1.0, 1.0, 1.0];
        let out = gaussian_blur(&img, img.full(), (3.0, 0.0), EdgeMode::None);
        let total: f32 = out.px.iter().map(|p| p[3]).sum();
        assert!((total - 1.0).abs() < 1e-4, "{total}");
        assert!(out.px[20][3] < 0.5 && out.px[17][3] > 0.0);
    }

    #[test]
    fn conversion_de_espacio_ida_y_vuelta() {
        for v in [0.0, 0.02, 0.2, 0.5, 0.9, 1.0] {
            assert!((to_srgb(to_linear(v)) - v).abs() < 1e-4);
        }
    }

    #[test]
    fn composite_y_blend() {
        let red = [1.0, 0.0, 0.0, 1.0];
        let half_blue = [0.0, 0.0, 0.5, 0.5];
        assert_eq!(
            composite(CompositeOp::In, red, half_blue),
            [0.5, 0.0, 0.0, 0.5]
        );
        assert_eq!(
            composite(CompositeOp::Out, red, half_blue),
            [0.5, 0.0, 0.0, 0.5]
        );
        assert_eq!(blend(BlendMode::Normal, red, half_blue), red);
        let white = [1.0; 4];
        assert_eq!(blend(BlendMode::Multiply, red, white), red);
        assert_eq!(blend(BlendMode::Screen, red, white), white);
    }
}
