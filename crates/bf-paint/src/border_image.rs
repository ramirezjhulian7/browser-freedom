//! E7: `border-image` (CSS Backgrounds 3 §6).
//!
//! La imagen se corta en nueve regiones con `border-image-slice` (§6.3) y cada una se coloca en
//! su región del área de la imagen de borde (la border box agrandada por `border-image-outset`,
//! §6.5) con los anchos de `border-image-width` (§6.4) y la repetición de `border-image-repeat`
//! (§6.6). Cada región sale como un [`DisplayItem::BackgroundImage`] con el trozo de la imagen
//! como tile: el rasterizador ya sabe estirarlo, repetirlo o colocarlo en posiciones dadas.
//!
//! Si la fuente es `none` o no se pudo cargar, se pintan los bordes normales (§6.2).

use std::sync::Arc;

use bf_layout::FontProvider;
use bf_layout::fragment::{Fragment, Rect};
use bf_style::properties::effects::{BorderImageLength, ImageRepeat};
use bf_style::properties::types::Image;
use tiny_skia::{IntRect, Pixmap};

use crate::display_list::{DisplayItem, Radii};
use crate::effects::{GradientSampler, resolve_gradient, to_u8};
use crate::images::PageImages;

/// La imagen fuente ya rasterizada y su tamaño en las unidades de los cortes numéricos (§6.3:
/// px de un bitmap o coordenadas de una imagen vectorial, las de su `viewBox` si lo tiene). Un
/// `<gradient>` no tiene tamaño natural: toma el del área (§6.2, *default sizing*).
fn source(
    f: &Fragment,
    images: &PageImages,
    fonts: &dyn FontProvider,
    area: &Rect,
) -> Option<(Arc<Pixmap>, f32, f32)> {
    match &f.style.border_image_source {
        Image::None => None,
        Image::Url(u) => {
            let (w, h) = images.natural_size(u, (area.width, area.height))?;
            if !(w > 0.0 && h > 0.0) {
                return None;
            }
            let image = images.image_at(u, w, h, Some(fonts))?;
            let (w, h) = images
                .svgs
                .get(u.as_str())
                .and_then(|s| s.view_box_size())
                .filter(|&(vw, vh)| vw > 0.0 && vh > 0.0)
                .unwrap_or((w, h));
            Some((image, w, h))
        }
        Image::Gradient(g) => {
            let (w, h) = (area.width, area.height);
            let resolved = resolve_gradient(g, w, h, f.style.color.rgba())?;
            let (pw, ph) = (w.ceil().max(1.0) as u32, h.ceil().max(1.0) as u32);
            let mut pixmap = Pixmap::new(pw, ph)?;
            let sampler = GradientSampler::new(&resolved);
            for (k, px) in pixmap
                .data_mut()
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .enumerate()
            {
                let (x, y) = ((k as u32 % pw) as f32 + 0.5, (k as u32 / pw) as f32 + 0.5);
                *px = to_u8(&sampler.at(x, y));
            }
            Some((Arc::new(pixmap), w, h))
        }
    }
}

/// Un eje de una región: el tamaño del tile, su origen y las posiciones explícitas de los
/// tiles (`None`: se repite por toda la región desde el origen).
struct Axis {
    tile: f32,
    origin: f32,
    positions: Option<Vec<f32>>,
}

/// Reparte tiles de tamaño natural `natural` en `[start, start + len)` según `mode` (§6.6).
/// `None` si no se pinta nada (`space` sin sitio para un tile entero).
fn axis(mode: ImageRepeat, start: f32, len: f32, natural: f32) -> Option<Axis> {
    let stretch = || Axis {
        tile: len,
        origin: start,
        positions: Some(vec![start]),
    };
    if !(natural > 0.0 && natural.is_finite()) {
        return Some(stretch());
    }
    Some(match mode {
        ImageRepeat::Stretch => stretch(),
        // Centrado: un tile queda en medio de la región.
        ImageRepeat::Repeat => Axis {
            tile: natural,
            origin: start + (len - natural) / 2.0,
            positions: None,
        },
        ImageRepeat::Round => {
            let n = (len / natural).round().max(1.0);
            Axis {
                tile: len / n,
                origin: start,
                positions: None,
            }
        }
        ImageRepeat::Space => {
            let n = (len / natural + 1e-4).floor();
            if n < 1.0 {
                return None;
            }
            let gap = (len - n * natural) / (n + 1.0);
            Axis {
                tile: natural,
                origin: start + gap,
                positions: Some(
                    (0..n as usize)
                        .map(|k| start + gap + k as f32 * (natural + gap))
                        .collect(),
                ),
            }
        }
    })
}

/// Un factor de escala usable (ni cero ni infinito ni NaN).
fn usable(k: f32) -> Option<f32> {
    (k > 0.0 && k.is_finite()).then_some(k)
}

/// Los ítems que pintan la imagen de borde de `f`, o `None` si se pintan los bordes normales.
pub(crate) fn items(
    f: &Fragment,
    images: &PageImages,
    fonts: &dyn FontProvider,
) -> Option<Vec<DisplayItem>> {
    let s = &f.style;
    if matches!(s.border_image_source, Image::None) {
        return None;
    }
    let r = f.rect;
    let bw = [f.border.top, f.border.right, f.border.bottom, f.border.left];
    // §6.5: el área de la imagen de borde.
    let mut out = [0.0f32; 4];
    for (i, o) in out.iter_mut().enumerate() {
        *o = match &s.border_image_outset.0[i] {
            BorderImageLength::Number(n) => n * bw[i],
            BorderImageLength::Length(l) => l.resolve(0.0),
            BorderImageLength::Auto => 0.0,
        }
        .max(0.0);
    }
    let area = Rect::new(
        r.x - out[3],
        r.y - out[0],
        r.width + out[1] + out[3],
        r.height + out[0] + out[2],
    );
    if !(area.width > 0.0 && area.height > 0.0) {
        return Some(Vec::new());
    }
    let (image, iw, ih) = source(f, images, fonts, &area)?;
    // §6.3: los cortes, en px de la imagen y limitados a su tamaño.
    let sl = &s.border_image_slice;
    let slice = [
        sl.sides[0].resolve(ih).clamp(0.0, ih),
        sl.sides[1].resolve(iw).clamp(0.0, iw),
        sl.sides[2].resolve(ih).clamp(0.0, ih),
        sl.sides[3].resolve(iw).clamp(0.0, iw),
    ];
    // §6.4: los anchos; si los opuestos se solapan, se reducen todos en la misma proporción.
    let mut w = [0.0f32; 4];
    for (i, v) in w.iter_mut().enumerate() {
        let basis = if i % 2 == 0 { area.height } else { area.width };
        *v = match &s.border_image_width.0[i] {
            BorderImageLength::Number(n) => n * bw[i],
            BorderImageLength::Length(l) => l.resolve(basis),
            BorderImageLength::Auto => slice[i],
        }
        .max(0.0);
    }
    let k = (area.width / (w[1] + w[3])).min(area.height / (w[0] + w[2]));
    if k < 1.0 {
        for v in &mut w {
            *v *= k;
        }
    }
    // Cortes de la imagen y de la área, en x y en y.
    let sx = [0.0, slice[3], iw - slice[1], iw];
    let sy = [0.0, slice[0], ih - slice[2], ih];
    let dx = [area.x, area.x + w[3], area.right() - w[1], area.right()];
    let dy = [area.y, area.y + w[0], area.bottom() - w[2], area.bottom()];
    // Escalas de los lados (de px de la imagen a px CSS), para el tamaño natural de los tiles.
    let top = usable(w[0] / slice[0]);
    let bottom = usable(w[2] / slice[2]);
    let left = usable(w[3] / slice[3]);
    let right = usable(w[1] / slice[1]);
    // Px del pixmap por px CSS de la imagen (los SVG se rasterizan con sobremuestreo).
    let (kx, ky) = (image.width() as f32 / iw, image.height() as f32 / ih);
    let repeat = s.border_image_repeat;
    let mut items = Vec::new();
    for j in 0..3 {
        for i in 0..3 {
            if i == 1 && j == 1 && !sl.fill {
                continue;
            }
            let (src_w, src_h) = (sx[i + 1] - sx[i], sy[j + 1] - sy[j]);
            let (dst_w, dst_h) = (dx[i + 1] - dx[i], dy[j + 1] - dy[j]);
            if src_w <= 0.0 || src_h <= 0.0 || dst_w <= 0.0 || dst_h <= 0.0 {
                continue;
            }
            // Escala de cada eje para el tamaño natural del tile (§6.6): los lados se escalan
            // con su ancho; el centro, con el de arriba (o abajo) en x y el de la izquierda (o
            // derecha) en y.
            let (hk, vk) = match (i, j) {
                (1, 1) => (top.or(bottom).unwrap_or(1.0), left.or(right).unwrap_or(1.0)),
                (1, _) => {
                    let k = dst_h / src_h;
                    (k, k)
                }
                (_, 1) => {
                    let k = dst_w / src_w;
                    (k, k)
                }
                _ => (dst_w / src_w, dst_h / src_h),
            };
            let hmode = if i == 1 {
                repeat.0
            } else {
                ImageRepeat::Stretch
            };
            let vmode = if j == 1 {
                repeat.1
            } else {
                ImageRepeat::Stretch
            };
            let (Some(ax), Some(ay)) = (
                axis(hmode, dx[i], dst_w, src_w * hk),
                axis(vmode, dy[j], dst_h, src_h * vk),
            ) else {
                continue;
            };
            // El trozo de la imagen, en px del pixmap.
            let x0 = (sx[i] * kx).round() as i32;
            let y0 = (sy[j] * ky).round() as i32;
            let x1 = ((sx[i + 1] * kx).round() as i32).min(image.width() as i32);
            let y1 = ((sy[j + 1] * ky).round() as i32).min(image.height() as i32);
            let Some(piece) = IntRect::from_ltrb(x0, y0, x1, y1).and_then(|r| image.clone_rect(r))
            else {
                continue;
            };
            items.push(DisplayItem::BackgroundImage {
                area: Rect::new(dx[i], dy[j], dst_w, dst_h),
                radii: Radii::default(),
                tile: Rect::new(ax.origin, ay.origin, ax.tile, ay.tile),
                xs: ax.positions,
                ys: ay.positions,
                image: Arc::new(piece),
            });
        }
    }
    Some(items)
}
