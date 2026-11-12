//! E7: efectos de grupo de las cajas HTML: `filter` (Filter Effects 1 §5 y §8), `clip-path`
//! con formas básicas o cajas (CSS Masking 1 §5, CSS Shapes 1 §3) y `mix-blend-mode`
//! (Compositing and Blending 1 §3).
//!
//! Un elemento con alguno de ellos (o con `isolation: isolate`) crea un stacking context
//! ([`creates_stacking_context`]); el constructor abre con [`DisplayItem::PushEffect`] una capa
//! transparente en la que se pinta el stacking context entero y la cierra con
//! [`DisplayItem::PopEffect`]. Al cerrarla ([`compose`]) la capa se filtra, se recorta con la
//! forma y se compone sobre lo de debajo con el modo de mezcla: el orden de CSS Masking 1 §3
//! (filtro, recorte, máscara, opacidad) y la mezcla al final. La opacidad y `mask-image` se
//! aplican dentro de la capa, lo que da lo mismo porque conmutan con el filtro (lineal) y con el
//! recorte.
//!
//! Las funciones de filtro se traducen a primitivas de SVG (Filter Effects 1 §12, *shorthands
//! defined*) y las aplica el mismo código que los filtros de SVG, en sRGB (como los
//! navegadores). `url()` en `filter` o en `clip-path` no se soporta en HTML: se ignora.
//!
//! [`DisplayItem::PushEffect`]: crate::display_list::DisplayItem::PushEffect
//! [`DisplayItem::PopEffect`]: crate::display_list::DisplayItem::PopEffect

use bf_layout::fragment::{Fragment, Rect};
use bf_style::ComputedStyle;
use bf_style::properties::effects::{
    BasicShape, ClipPath, FilterFunction, GeometryBox, Isolation, MixBlendMode, ShapeRadius,
};
use bf_svg::{Filter, FilterInput, FilterKind, FilterPrimitive};
use tiny_skia::{
    BlendMode, FillRule, Mask, Path, PathBuilder, Pixmap, PixmapPaint, Rect as SkRect, Transform,
};

/// Lo que se hace con la capa al cerrarla.
#[derive(Debug, Clone)]
pub struct Effect {
    /// El filtro, en coordenadas de página (px CSS).
    pub filter: Option<Filter>,
    /// La forma de `clip-path`, en coordenadas de página.
    pub clip: Option<(Path, FillRule)>,
    pub blend: BlendMode,
}

/// ¿Crea `style` un stacking context por estos efectos? (Filter Effects 1 §5, CSS Masking 1
/// §5.1, Compositing and Blending 1 §3.2 y §3.4).
pub fn creates_stacking_context(style: &ComputedStyle) -> bool {
    !style.filter.0.is_empty()
        || !matches!(style.clip_path, ClipPath::None)
        || style.mix_blend_mode != MixBlendMode::Normal
        || style.isolation == Isolation::Isolate
}

/// El efecto de la caja `f`, si tiene alguno que pintar.
pub fn effect(f: &Fragment) -> Option<Effect> {
    let s = &f.style;
    if !creates_stacking_context(s) {
        return None;
    }
    let filter = css_filter(s);
    let clip = clip_shape(f);
    let blend = blend_mode(s.mix_blend_mode);
    // `isolation: isolate` necesita la capa aunque no haga nada al cerrarla: las mezclas de
    // dentro no ven lo que hay debajo (Compositing and Blending 1 §3.4).
    if filter.is_none()
        && clip.is_none()
        && blend == BlendMode::SourceOver
        && s.isolation != Isolation::Isolate
    {
        return None;
    }
    Some(Effect {
        filter,
        clip,
        blend,
    })
}

fn blend_mode(m: MixBlendMode) -> BlendMode {
    match m {
        MixBlendMode::Normal => BlendMode::SourceOver,
        MixBlendMode::Multiply => BlendMode::Multiply,
        MixBlendMode::Screen => BlendMode::Screen,
        MixBlendMode::Overlay => BlendMode::Overlay,
        MixBlendMode::Darken => BlendMode::Darken,
        MixBlendMode::Lighten => BlendMode::Lighten,
        MixBlendMode::ColorDodge => BlendMode::ColorDodge,
        MixBlendMode::ColorBurn => BlendMode::ColorBurn,
        MixBlendMode::HardLight => BlendMode::HardLight,
        MixBlendMode::SoftLight => BlendMode::SoftLight,
        MixBlendMode::Difference => BlendMode::Difference,
        MixBlendMode::Exclusion => BlendMode::Exclusion,
        MixBlendMode::Hue => BlendMode::Hue,
        MixBlendMode::Saturation => BlendMode::Saturation,
        MixBlendMode::Color => BlendMode::Color,
        MixBlendMode::Luminosity => BlendMode::Luminosity,
        MixBlendMode::PlusLighter => BlendMode::Plus,
    }
}

// --- filter ---

/// Matriz de `feColorMatrix` con la parte 3×3 de color `m` (sin desplazamientos).
fn rgb_matrix(m: [[f32; 3]; 3]) -> [f32; 20] {
    [
        m[0][0], m[0][1], m[0][2], 0.0, 0.0, //
        m[1][0], m[1][1], m[1][2], 0.0, 0.0, //
        m[2][0], m[2][1], m[2][2], 0.0, 0.0, //
        0.0, 0.0, 0.0, 1.0, 0.0,
    ]
}

/// Transferencia lineal `c·k + b` en los tres canales de color.
fn linear(k: f32, b: f32) -> [f32; 20] {
    [
        k, 0.0, 0.0, 0.0, b, //
        0.0, k, 0.0, 0.0, b, //
        0.0, 0.0, k, 0.0, b, //
        0.0, 0.0, 0.0, 1.0, 0.0,
    ]
}

/// Las primitivas equivalentes a una función (Filter Effects 1 §12).
fn primitive(func: &FilterFunction, color: bf_style::values::color::Rgba) -> Option<FilterKind> {
    let matrix = |m: [f32; 20]| Some(FilterKind::ColorMatrix { matrix: m });
    match func {
        FilterFunction::Blur(r) => {
            let s = r.resolve(0.0).max(0.0);
            Some(FilterKind::GaussianBlur {
                std_dev: (s, s),
                edge: Default::default(),
            })
        }
        FilterFunction::Brightness(a) => matrix(linear(*a, 0.0)),
        FilterFunction::Contrast(a) => matrix(linear(*a, 0.5 - 0.5 * a)),
        FilterFunction::Invert(a) => {
            let a = a.min(1.0);
            matrix(linear(1.0 - 2.0 * a, a))
        }
        FilterFunction::Opacity(a) => {
            let mut m = linear(1.0, 0.0);
            m[18] = a.min(1.0);
            matrix(m)
        }
        FilterFunction::Grayscale(a) => {
            let k = 1.0 - a.min(1.0);
            matrix(rgb_matrix([
                [
                    0.2126 + 0.7874 * k,
                    0.7152 - 0.7152 * k,
                    0.0722 - 0.0722 * k,
                ],
                [
                    0.2126 - 0.2126 * k,
                    0.7152 + 0.2848 * k,
                    0.0722 - 0.0722 * k,
                ],
                [
                    0.2126 - 0.2126 * k,
                    0.7152 - 0.7152 * k,
                    0.0722 + 0.9278 * k,
                ],
            ]))
        }
        FilterFunction::Sepia(a) => {
            let k = 1.0 - a.min(1.0);
            matrix(rgb_matrix([
                [0.393 + 0.607 * k, 0.769 - 0.769 * k, 0.189 - 0.189 * k],
                [0.349 - 0.349 * k, 0.686 + 0.314 * k, 0.168 - 0.168 * k],
                [0.272 - 0.272 * k, 0.534 - 0.534 * k, 0.131 + 0.869 * k],
            ]))
        }
        FilterFunction::Saturate(s) => matrix(rgb_matrix([
            [0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s],
            [0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s],
            [0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s],
        ])),
        FilterFunction::HueRotate(deg) => {
            let (sn, cs) = deg.to_radians().sin_cos();
            matrix(rgb_matrix([
                [
                    0.213 + cs * 0.787 - sn * 0.213,
                    0.715 - cs * 0.715 - sn * 0.715,
                    0.072 - cs * 0.072 + sn * 0.928,
                ],
                [
                    0.213 - cs * 0.213 + sn * 0.143,
                    0.715 + cs * 0.285 + sn * 0.140,
                    0.072 - cs * 0.072 - sn * 0.283,
                ],
                [
                    0.213 - cs * 0.213 - sn * 0.787,
                    0.715 - cs * 0.715 + sn * 0.715,
                    0.072 + cs * 0.928 + sn * 0.072,
                ],
            ]))
        }
        // §12.2: la desviación es la mitad del radio de desenfoque, como en `text-shadow`.
        FilterFunction::DropShadow {
            color: c,
            x,
            y,
            blur,
        } => {
            let s = blur.resolve(0.0).max(0.0) / 2.0;
            Some(FilterKind::DropShadow {
                std_dev: (s, s),
                dx: x.resolve(0.0),
                dy: y.resolve(0.0),
                color: c.map_or(color, |c| c.resolve(color)),
            })
        }
        FilterFunction::Url(_) => None,
    }
}

/// `filter` como cadena de primitivas de SVG, cada una sobre la anterior.
fn css_filter(s: &ComputedStyle) -> Option<Filter> {
    // Región prácticamente infinita: en HTML el filtro no recorta (lo limita el lienzo).
    const HUGE: f32 = 1.0e6;
    let region = (-HUGE, -HUGE, 2.0 * HUGE, 2.0 * HUGE);
    let color = s.color.rgba();
    let mut primitives: Vec<FilterPrimitive> = Vec::new();
    for func in &s.filter.0 {
        let Some(kind) = primitive(func, color) else {
            continue;
        };
        let input = match primitives.len() {
            0 => FilterInput::SourceGraphic,
            n => FilterInput::Primitive(n - 1),
        };
        primitives.push(FilterPrimitive {
            kind,
            input,
            subregion: region,
            linear_rgb: false,
        });
    }
    (!primitives.is_empty()).then_some(Filter { region, primitives })
}

// --- clip-path ---

/// La caja de referencia (CSS Masking 1 §5.1; en HTML `fill-box` es la de contenido y
/// `stroke-box` y `view-box`, la del borde) y sus radios.
fn reference_box(f: &Fragment, b: GeometryBox) -> (Rect, [(f32, f32); 4]) {
    let border = f.rect;
    let s = &f.style;
    let r = |c: &bf_style::properties::types::CornerRadius| {
        (c.0.resolve(border.width), c.1.resolve(border.height))
    };
    let radii = crate::display_list::Radii([
        r(&s.border_top_left_radius),
        r(&s.border_top_right_radius),
        r(&s.border_bottom_right_radius),
        r(&s.border_bottom_left_radius),
    ])
    .fit(&border);
    match b {
        GeometryBox::MarginBox => {
            let m = f.margin;
            let grow = |(h, v): (f32, f32), dh: f32, dv: f32| {
                if h > 0.0 && v > 0.0 {
                    (h + dh, v + dv)
                } else {
                    (h, v)
                }
            };
            let [tl, tr, br, bl] = radii.0;
            (
                f.margin_rect(),
                [
                    grow(tl, m.left, m.top),
                    grow(tr, m.right, m.top),
                    grow(br, m.right, m.bottom),
                    grow(bl, m.left, m.bottom),
                ],
            )
        }
        GeometryBox::PaddingBox => (f.padding_rect(), radii.shrink(&f.border).0),
        GeometryBox::ContentBox | GeometryBox::FillBox => {
            let inset = bf_layout::fragment::Edges {
                top: f.border.top + f.padding.top,
                right: f.border.right + f.padding.right,
                bottom: f.border.bottom + f.padding.bottom,
                left: f.border.left + f.padding.left,
            };
            (f.content_rect(), radii.shrink(&inset).0)
        }
        GeometryBox::BorderBox | GeometryBox::StrokeBox | GeometryBox::ViewBox => (border, radii.0),
    }
}

/// Rectángulo con esquinas elípticas (radios ya ajustados al tamaño).
fn rounded(r: &Rect, radii: [(f32, f32); 4]) -> Option<Path> {
    if radii.iter().all(|&(h, v)| h <= 0.0 || v <= 0.0) {
        return Some(PathBuilder::from_rect(SkRect::from_xywh(
            r.x, r.y, r.width, r.height,
        )?));
    }
    // Constante de la aproximación de un cuarto de elipse con una cúbica.
    const K: f32 = 0.552_284_8;
    let [tl, tr, br, bl] = radii.map(|(h, v)| {
        if h > 0.0 && v > 0.0 {
            (h, v)
        } else {
            (0.0, 0.0)
        }
    });
    let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
    let mut pb = PathBuilder::new();
    pb.move_to(x0 + tl.0, y0);
    pb.line_to(x1 - tr.0, y0);
    pb.cubic_to(
        x1 - tr.0 * (1.0 - K),
        y0,
        x1,
        y0 + tr.1 * (1.0 - K),
        x1,
        y0 + tr.1,
    );
    pb.line_to(x1, y1 - br.1);
    pb.cubic_to(
        x1,
        y1 - br.1 * (1.0 - K),
        x1 - br.0 * (1.0 - K),
        y1,
        x1 - br.0,
        y1,
    );
    pb.line_to(x0 + bl.0, y1);
    pb.cubic_to(
        x0 + bl.0 * (1.0 - K),
        y1,
        x0,
        y1 - bl.1 * (1.0 - K),
        x0,
        y1 - bl.1,
    );
    pb.line_to(x0, y0 + tl.1);
    pb.cubic_to(
        x0,
        y0 + tl.1 * (1.0 - K),
        x0 + tl.0 * (1.0 - K),
        y0,
        x0 + tl.0,
        y0,
    );
    pb.close();
    pb.finish()
}

/// Escala los radios para que los de un mismo lado no se solapen (CSS Backgrounds 3 §5.5).
fn fit_radii(radii: [(f32, f32); 4], w: f32, h: f32) -> [(f32, f32); 4] {
    let r = crate::display_list::Radii(radii);
    r.fit(&Rect::new(0.0, 0.0, w, h)).0
}

/// Una forma vacía: no se ve nada del elemento.
fn empty() -> (Path, FillRule) {
    let mut pb = PathBuilder::new();
    pb.move_to(0.0, 0.0);
    pb.line_to(0.0, 0.0);
    pb.close();
    // Un trazado sin área no siempre se puede construir: un rectángulo fuera de todo lienzo.
    let path = pb.finish().unwrap_or_else(|| {
        PathBuilder::from_rect(SkRect::from_xywh(-1.0e7, -1.0e7, 1.0, 1.0).expect("rectángulo"))
    });
    (path, FillRule::Winding)
}

/// La forma de `clip-path` en coordenadas de página.
fn clip_shape(f: &Fragment) -> Option<(Path, FillRule)> {
    match &f.style.clip_path {
        ClipPath::None | ClipPath::Url(_) => None,
        ClipPath::Box(b) => {
            let (r, radii) = reference_box(f, *b);
            Some(rounded(&r, radii).map_or_else(empty, |p| (p, FillRule::Winding)))
        }
        ClipPath::Shape(shape, b) => {
            let (r, _) = reference_box(f, *b);
            Some(basic_shape(shape, &r).unwrap_or_else(empty))
        }
    }
}

/// El centro de `circle()`/`ellipse()` dentro de `r` (relativo a su esquina).
fn center(p: &Option<bf_style::properties::effects::ShapePosition>, r: &Rect) -> (f32, f32) {
    match p {
        Some((x, y)) => (x.resolve(r.width), y.resolve(r.height)),
        None => (r.width / 2.0, r.height / 2.0),
    }
}

/// Radio de `circle()`/`ellipse()` (CSS Shapes 1 §3.2.1): `sides` son las distancias del
/// centro a los lados que cuentan; `basis`, la referencia de los porcentajes.
fn radius(v: &ShapeRadius, sides: &[f32], basis: f32) -> f32 {
    match v {
        ShapeRadius::Length(l) => l.resolve(basis),
        ShapeRadius::ClosestSide => sides.iter().copied().fold(f32::INFINITY, f32::min),
        ShapeRadius::FarthestSide => sides.iter().copied().fold(0.0, f32::max),
    }
    .max(0.0)
}

fn ellipse_path(cx: f32, cy: f32, rx: f32, ry: f32) -> Option<Path> {
    if rx <= 0.0 || ry <= 0.0 {
        return None;
    }
    PathBuilder::from_oval(SkRect::from_xywh(cx - rx, cy - ry, 2.0 * rx, 2.0 * ry)?)
}

fn basic_shape(shape: &BasicShape, r: &Rect) -> Option<(Path, FillRule)> {
    let (w, h) = (r.width, r.height);
    let nonzero = |p: Path| (p, FillRule::Winding);
    match shape {
        BasicShape::Inset { offsets, radii } => {
            let [t, rt, b, l] = [
                offsets[0].resolve(h),
                offsets[1].resolve(w),
                offsets[2].resolve(h),
                offsets[3].resolve(w),
            ];
            // CSS Shapes 1 §3.2.1: si los opuestos suman más que la caja, se reducen en
            // proporción.
            let (l, rt) = if l + rt > w && l + rt > 0.0 {
                (l * w / (l + rt), rt * w / (l + rt))
            } else {
                (l, rt)
            };
            let (t, b) = if t + b > h && t + b > 0.0 {
                (t * h / (t + b), b * h / (t + b))
            } else {
                (t, b)
            };
            let rect = Rect::new(r.x + l, r.y + t, w - l - rt, h - t - b);
            if rect.width <= 0.0 || rect.height <= 0.0 {
                return None;
            }
            let radii = radii
                .clone()
                .map(|(rh, rv)| (rh.resolve(rect.width), rv.resolve(rect.height)));
            rounded(&rect, fit_radii(radii, rect.width, rect.height)).map(nonzero)
        }
        BasicShape::Circle {
            radius: rad,
            position,
        } => {
            let (cx, cy) = center(position, r);
            let reference = ((w * w + h * h) / 2.0).sqrt();
            let rr = radius(
                rad,
                &[cx.abs(), (w - cx).abs(), cy.abs(), (h - cy).abs()],
                reference,
            );
            ellipse_path(r.x + cx, r.y + cy, rr, rr).map(nonzero)
        }
        BasicShape::Ellipse { rx, ry, position } => {
            let (cx, cy) = center(position, r);
            let rx = radius(rx, &[cx.abs(), (w - cx).abs()], w);
            let ry = radius(ry, &[cy.abs(), (h - cy).abs()], h);
            ellipse_path(r.x + cx, r.y + cy, rx, ry).map(nonzero)
        }
        BasicShape::Polygon { evenodd, points } => {
            let mut pb = PathBuilder::new();
            for (i, (x, y)) in points.iter().enumerate() {
                let (px, py) = (r.x + x.resolve(w), r.y + y.resolve(h));
                if i == 0 {
                    pb.move_to(px, py);
                } else {
                    pb.line_to(px, py);
                }
            }
            pb.close();
            let rule = if *evenodd {
                FillRule::EvenOdd
            } else {
                FillRule::Winding
            };
            pb.finish().map(|p| (p, rule))
        }
        BasicShape::Path { evenodd, d } => {
            let mut pb = PathBuilder::new();
            for seg in bf_svg::geom::parse_path(d) {
                use bf_svg::Seg;
                match seg {
                    Seg::MoveTo(x, y) => pb.move_to(r.x + x, r.y + y),
                    Seg::LineTo(x, y) => pb.line_to(r.x + x, r.y + y),
                    Seg::QuadTo(x1, y1, x, y) => pb.quad_to(r.x + x1, r.y + y1, r.x + x, r.y + y),
                    Seg::CubicTo(x1, y1, x2, y2, x, y) => {
                        pb.cubic_to(r.x + x1, r.y + y1, r.x + x2, r.y + y2, r.x + x, r.y + y)
                    }
                    Seg::Close => pb.close(),
                }
            }
            let rule = if *evenodd {
                FillRule::EvenOdd
            } else {
                FillRule::Winding
            };
            pb.finish().map(|p| (p, rule))
        }
    }
}

// --- Composición ---

/// Cierra la capa `layer` (del tamaño de `target`) sobre `target`: filtro, recorte (con la
/// forma y con `clip`, el recorte de los ancestros) y mezcla. `ts` va de la página al
/// dispositivo.
pub(crate) fn compose(
    layer: Pixmap,
    target: &mut Pixmap,
    fx: &Effect,
    ts: Transform,
    clip: Option<&Mask>,
) {
    let (w, h) = (target.width(), target.height());
    let (image, x, y) = match &fx.filter {
        Some(f) => {
            let paint = |pm: &mut Pixmap, t: Transform| {
                // `t` es `ts` desplazado a la esquina del área de trabajo.
                let (dx, dy) = ((t.tx - ts.tx).round() as i32, (t.ty - ts.ty).round() as i32);
                pm.draw_pixmap(
                    dx,
                    dy,
                    layer.as_ref(),
                    &PixmapPaint::default(),
                    Transform::identity(),
                    None,
                );
            };
            match crate::images::apply_filter(f, w, h, ts, paint) {
                Some(out) => out,
                None => return,
            }
        }
        None => (layer, 0, 0),
    };
    // El contenido ya se pintó recortado por los ancestros; solo lo que el filtro extiende
    // (desenfoques, sombras) hay que volver a recortar.
    let mut mask = if fx.filter.is_some() {
        clip.cloned()
    } else {
        None
    };
    if let Some((path, rule)) = &fx.clip {
        let mut m = match mask.take() {
            Some(m) => m,
            None => {
                let Some(mut m) = Mask::new(w, h) else {
                    return;
                };
                m.data_mut().fill(255);
                m
            }
        };
        m.intersect_path(path, *rule, true, ts);
        mask = Some(m);
    }
    let paint = PixmapPaint {
        blend_mode: fx.blend,
        ..PixmapPaint::default()
    };
    target.draw_pixmap(
        x,
        y,
        image.as_ref(),
        &paint,
        Transform::identity(),
        mask.as_ref(),
    );
}
