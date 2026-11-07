//! Rasterizado de un árbol de `bf-svg` con `tiny-skia`, al tamaño de píxeles pedido. Los
//! grupos con `opacity` o `clipPath` se pintan en una capa aparte y se componen con su
//! opacidad y su máscara (`clipPath` y `mask`); los `pattern` se pintan en una tesela que se
//! repite; el texto usa las fuentes de `bf-font` (vía [`FontProvider`]).

use super::{SVG_IMAGE_DEPTH, SvgImage, SvgResources};
use bf_layout::{FontProvider, FontRequest, PathCommand};
use bf_style::properties::types::{FamilyName, FontFamily};
use bf_svg::{
    Chunk, ClipPath, Fill, FillRule, Gradient, GradientKind, Group, LineCap, LineJoin, Node, Paint,
    Pattern, Rgba, Seg, Spread, Stroke, Text, TextAnchor, Tree,
};
use tiny_skia::{
    Color, FillRule as SkFillRule, FilterQuality, GradientStop, LinearGradient, Mask, MaskType,
    Path, PathBuilder, Pixmap, PixmapPaint, Point, RadialGradient, Rect, Shader, SpreadMode,
    StrokeDash, Transform,
};

/// Tope de lado de la tesela de un `pattern` en píxeles.
const MAX_TILE: f32 = 2048.0;

/// Tope de lado del bitmap (protege la memoria con tamaños absurdos).
const MAX_SIDE: u32 = 4096;

/// Pinta `tree` en un bitmap de `width`×`height` píxeles (el viewport del árbol escalado),
/// sin recursos para sus elementos `image`.
pub fn rasterize(
    tree: &Tree,
    width: u32,
    height: u32,
    fonts: Option<&dyn FontProvider>,
) -> Option<Pixmap> {
    rasterize_with(tree, width, height, fonts, &SvgResources::default())
}

/// [`rasterize`] con los recursos `res` para los elementos `image`.
pub fn rasterize_with(
    tree: &Tree,
    width: u32,
    height: u32,
    fonts: Option<&dyn FontProvider>,
    res: &SvgResources,
) -> Option<Pixmap> {
    let (width, height) = (width.clamp(1, MAX_SIDE), height.clamp(1, MAX_SIDE));
    let mut pixmap = Pixmap::new(width, height)?;
    if tree.width <= 0.0 || tree.height <= 0.0 {
        return Some(pixmap);
    }
    let (kx, ky) = (width as f32 / tree.width, height as f32 / tree.height);
    let base = Transform::from_scale(kx, ky);
    // El espacio del host de `non-scaling-stroke` es el del viewport raíz en px de CSS.
    let mut r = Raster {
        fonts,
        host_scale: (kx * ky).sqrt(),
        res,
        depth: 0,
    };
    r.group(&tree.root, &mut pixmap, base);
    Some(pixmap)
}

fn sk(t: &bf_svg::Transform) -> Transform {
    Transform::from_row(t.a, t.b, t.c, t.d, t.e, t.f)
}

fn color(c: Rgba, opacity: f32) -> Color {
    Color::from_rgba(
        (c.r / 255.0).clamp(0.0, 1.0),
        (c.g / 255.0).clamp(0.0, 1.0),
        (c.b / 255.0).clamp(0.0, 1.0),
        (c.a * opacity).clamp(0.0, 1.0),
    )
    .unwrap_or(Color::TRANSPARENT)
}

fn path(segs: &[Seg]) -> Option<Path> {
    let mut pb = PathBuilder::new();
    for s in segs {
        match *s {
            Seg::MoveTo(x, y) => pb.move_to(x, y),
            Seg::LineTo(x, y) => pb.line_to(x, y),
            Seg::QuadTo(x1, y1, x, y) => pb.quad_to(x1, y1, x, y),
            Seg::CubicTo(x1, y1, x2, y2, x, y) => pb.cubic_to(x1, y1, x2, y2, x, y),
            Seg::Close => pb.close(),
        }
    }
    pb.finish()
}

fn rule(r: FillRule) -> SkFillRule {
    match r {
        FillRule::NonZero => SkFillRule::Winding,
        FillRule::EvenOdd => SkFillRule::EvenOdd,
    }
}

fn shader(paint: &Paint, opacity: f32) -> Option<Shader<'static>> {
    match paint {
        Paint::Color(c) => Some(Shader::SolidColor(color(*c, opacity))),
        Paint::Gradient(g) => gradient(g, opacity),
        // Necesita su tesela: ver `Raster::paint_shader`.
        Paint::Pattern(_) => None,
    }
}

/// Escala de una transformación en cada eje (longitud de las columnas).
fn axis_scales(t: &Transform) -> (f32, f32) {
    (t.sx.hypot(t.ky), t.kx.hypot(t.sy))
}

/// `a *= b` píxel a píxel (intersección de dos máscaras con cobertura parcial).
fn multiply_masks(a: &mut Mask, b: &Mask) {
    for (x, y) in a.data_mut().iter_mut().zip(b.data()) {
        *x = ((*x as u16 * *y as u16 + 127) / 255) as u8;
    }
}

fn gradient(g: &Gradient, opacity: f32) -> Option<Shader<'static>> {
    let stops: Vec<GradientStop> = g
        .stops
        .iter()
        .map(|(o, c)| GradientStop::new(*o, color(*c, opacity)))
        .collect();
    let mode = match g.spread {
        Spread::Pad => SpreadMode::Pad,
        Spread::Reflect => SpreadMode::Reflect,
        Spread::Repeat => SpreadMode::Repeat,
    };
    let ts = sk(&g.transform);
    match g.kind {
        GradientKind::Linear { x1, y1, x2, y2 } => LinearGradient::new(
            Point::from_xy(x1, y1),
            Point::from_xy(x2, y2),
            stops,
            mode,
            ts,
        ),
        GradientKind::Radial {
            cx, cy, r, fx, fy, ..
        } => RadialGradient::new(
            Point::from_xy(fx, fy),
            Point::from_xy(cx, cy),
            r,
            stops,
            mode,
            ts,
        ),
    }
}

fn stroke_style(s: &Stroke) -> tiny_skia::Stroke {
    tiny_skia::Stroke {
        width: s.width,
        miter_limit: s.miter_limit,
        line_cap: match s.cap {
            LineCap::Butt => tiny_skia::LineCap::Butt,
            LineCap::Round => tiny_skia::LineCap::Round,
            LineCap::Square => tiny_skia::LineCap::Square,
        },
        line_join: match s.join {
            LineJoin::Miter | LineJoin::Arcs => tiny_skia::LineJoin::Miter,
            LineJoin::MiterClip => tiny_skia::LineJoin::MiterClip,
            LineJoin::Round => tiny_skia::LineJoin::Round,
            LineJoin::Bevel => tiny_skia::LineJoin::Bevel,
        },
        dash: if s.dasharray.is_empty() {
            None
        } else {
            StrokeDash::new(s.dasharray.clone(), s.dashoffset)
        },
    }
}

struct Raster<'f> {
    fonts: Option<&'f dyn FontProvider>,
    /// Píxeles por px de CSS del viewport raíz.
    host_scale: f32,
    res: &'f SvgResources,
    /// Niveles de `image` con SVG dentro de los que se está pintando.
    depth: u32,
}

impl Raster<'_> {
    fn group(&mut self, g: &Group, canvas: &mut Pixmap, ts: Transform) {
        let mut ts = ts.pre_concat(sk(&g.transform));
        if g.host_units {
            let (sx, sy) = axis_scales(&ts);
            let s = (sx * sy).sqrt();
            if s > 0.0 && s.is_finite() {
                let k = self.host_scale / s;
                ts = ts.pre_scale(k, k);
            }
        }
        if g.opacity <= 0.0 {
            return;
        }
        let layered = g.opacity < 1.0 || g.clip.is_some() || g.mask.is_some();
        if !layered {
            for c in &g.children {
                self.node(c, canvas, ts);
            }
            return;
        }
        let Some(mut layer) = Pixmap::new(canvas.width(), canvas.height()) else {
            return;
        };
        for c in &g.children {
            self.node(c, &mut layer, ts);
        }
        let mut mask = g.clip.as_ref().and_then(|c| self.clip_mask(c, canvas, ts));
        if g.clip.is_some() && mask.is_none() {
            return;
        }
        if let Some(m) = &g.mask {
            let Some(mm) = self.mask_mask(m, canvas, ts) else {
                return;
            };
            mask = Some(match mask {
                Some(mut c) => {
                    multiply_masks(&mut c, &mm);
                    c
                }
                None => mm,
            });
        }
        canvas.draw_pixmap(
            0,
            0,
            layer.as_ref(),
            &PixmapPaint {
                opacity: g.opacity,
                ..PixmapPaint::default()
            },
            Transform::identity(),
            mask.as_ref(),
        );
    }

    fn clip_mask(&mut self, clip: &ClipPath, canvas: &Pixmap, ts: Transform) -> Option<Mask> {
        let mut mask = Mask::new(canvas.width(), canvas.height())?;
        let ts = ts.pre_concat(sk(&clip.transform));
        for (segs, r, t) in &clip.shapes {
            if let Some(p) = path(segs) {
                mask.fill_path(&p, rule(*r), true, ts.pre_concat(sk(t)));
            }
        }
        for text in &clip.texts {
            for (p, ..) in self.text_paths(text) {
                mask.fill_path(
                    &p,
                    SkFillRule::Winding,
                    true,
                    ts.pre_concat(sk(&text.transform)),
                );
            }
        }
        Some(mask)
    }

    /// `mask` (CSS Masking 1 §7.1): el contenido pintado en una capa, su luminancia (o su alfa)
    /// como cobertura, recortada a la región de la máscara.
    fn mask_mask(&mut self, m: &bf_svg::Mask, canvas: &Pixmap, ts: Transform) -> Option<Mask> {
        let (w, h) = (canvas.width(), canvas.height());
        let mut out = Mask::new(w, h)?;
        let (rx, ry, rw, rh) = m.rect;
        let Some(rect) = Rect::from_xywh(rx, ry, rw, rh) else {
            return Some(out);
        };
        let mut layer = Pixmap::new(w, h)?;
        self.group(&m.content, &mut layer, ts);
        let kind = if m.luminance {
            MaskType::Luminance
        } else {
            MaskType::Alpha
        };
        let content = Mask::from_pixmap(layer.as_ref(), kind);
        out.fill_path(&PathBuilder::from_rect(rect), SkFillRule::Winding, true, ts);
        multiply_masks(&mut out, &content);
        Some(out)
    }

    /// La tesela de un `pattern` (SVG 2 §14.4) rasterizada a la resolución con la que se va a
    /// pintar, y la transformación del shader (de la tesela en píxeles al espacio de usuario).
    fn pattern_tile(&mut self, p: &Pattern, ts: Transform) -> Option<(Pixmap, Transform)> {
        let total = ts.pre_concat(sk(&p.transform));
        let (sx, sy) = axis_scales(&total);
        let pw = (p.width * sx).ceil().clamp(1.0, MAX_TILE);
        let ph = (p.height * sy).ceil().clamp(1.0, MAX_TILE);
        let mut tile = Pixmap::new(pw as u32, ph as u32)?;
        let (kx, ky) = (pw / p.width, ph / p.height);
        self.group(&p.content, &mut tile, Transform::from_scale(kx, ky));
        Some((tile, sk(&p.transform).pre_scale(1.0 / kx, 1.0 / ky)))
    }

    /// El shader de una pintura; los `pattern` dejan su tesela en `slot`.
    fn paint_shader<'a>(
        &mut self,
        paint: &Paint,
        opacity: f32,
        ts: Transform,
        slot: &'a mut Option<(Pixmap, Transform)>,
    ) -> Option<Shader<'a>> {
        match paint {
            Paint::Pattern(p) => {
                *slot = self.pattern_tile(p, ts);
                let (tile, t) = slot.as_ref()?;
                Some(tiny_skia::Pattern::new(
                    tile.as_ref(),
                    SpreadMode::Repeat,
                    FilterQuality::Bicubic,
                    opacity.clamp(0.0, 1.0),
                    *t,
                ))
            }
            other => shader(other, opacity),
        }
    }

    fn node(&mut self, n: &Node, canvas: &mut Pixmap, ts: Transform) {
        match n {
            Node::Group(g) => self.group(g, canvas, ts),
            Node::Path(p) => {
                let Some(sk_path) = path(&p.segs) else {
                    return;
                };
                let ts = ts.pre_concat(sk(&p.transform));
                self.paint(&sk_path, p.fill.as_ref(), p.stroke.as_ref(), canvas, ts);
            }
            Node::Image(im) => self.image(im, canvas, ts.pre_concat(sk(&im.transform))),
            Node::Text(t) => {
                let ts = ts.pre_concat(sk(&t.transform));
                for (p, fill, stroke, stroke_first) in self.text_paths(t) {
                    if stroke_first {
                        self.paint(&p, None, stroke.as_ref(), canvas, ts);
                        self.paint(&p, fill.as_ref(), None, canvas, ts);
                    } else {
                        self.paint(&p, fill.as_ref(), stroke.as_ref(), canvas, ts);
                    }
                }
            }
        }
    }

    /// Elemento `image` (SVG 2 §12.3): un bitmap colocado con `preserveAspectRatio` y
    /// recortado a su viewport, o un SVG con su viewport en el rectángulo del elemento.
    fn image(&mut self, im: &bf_svg::Image, canvas: &mut Pixmap, ts: Transform) {
        if self.depth >= SVG_IMAGE_DEPTH {
            return;
        }
        let Some(res) = self.res.get(&im.href).cloned() else {
            return;
        };
        let (nw, nh) = match &res {
            SvgImage::Bitmap(p) => (p.width() as f32, p.height() as f32),
            SvgImage::Svg(s) => s.default_size(),
        };
        if !(nw > 0.0 && nh > 0.0) {
            return;
        }
        // `auto` (§12.3.2): el tamaño natural, o el que da la proporción con el otro lado.
        let (w, h) = match (im.width, im.height) {
            (Some(w), Some(h)) => (w, h),
            (Some(w), None) => (w, w * nh / nw),
            (None, Some(h)) => (h * nw / nh, h),
            (None, None) => (nw, nh),
        };
        if !(w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite()) {
            return;
        }
        match res {
            SvgImage::Bitmap(p) => {
                let place = bf_svg::aspect_transform((0.0, 0.0, nw, nh), &im.par, (w, h));
                let to_user = Transform::from_translate(im.x, im.y).pre_concat(sk(&place));
                // Lo que se ve: la imagen colocada, recortada al viewport (`slice`).
                let (x0, y0) = place.apply(0.0, 0.0);
                let (x1, y1) = place.apply(nw, nh);
                let (l, t) = (x0.min(x1).max(0.0), y0.min(y1).max(0.0));
                let (r, b) = (x0.max(x1).min(w), y0.max(y1).min(h));
                let Some(rect) = Rect::from_ltrb(im.x + l, im.y + t, im.x + r, im.y + b) else {
                    return;
                };
                let paint = tiny_skia::Paint {
                    shader: tiny_skia::Pattern::new(
                        p.as_ref().as_ref(),
                        SpreadMode::Pad,
                        FilterQuality::Bilinear,
                        1.0,
                        to_user,
                    ),
                    anti_alias: true,
                    ..tiny_skia::Paint::default()
                };
                canvas.fill_rect(rect, &paint, ts, None);
            }
            SvgImage::Svg(s) => {
                // Con `viewBox`, el viewport del SVG es el rectángulo del elemento y manda el
                // `preserveAspectRatio` de aquí; sin él, se coloca a su tamaño natural.
                let (doc, size, place) = if s.has_view_box() {
                    (s.with_aspect(&im.par), (w, h), bf_svg::Transform::IDENTITY)
                } else {
                    let place = bf_svg::aspect_transform((0.0, 0.0, nw, nh), &im.par, (w, h));
                    (s.as_ref().clone(), (nw, nh), place)
                };
                let tree = doc.tree(size.0, size.1);
                let inner = bf_svg::Transform::translate(im.x, im.y).then(&place);
                let group = Group {
                    host_units: false,
                    transform: bf_svg::Transform::IDENTITY,
                    opacity: 1.0,
                    clip: Some(bf_svg::ClipPath {
                        transform: bf_svg::Transform::IDENTITY,
                        shapes: vec![(
                            bf_svg::geom::rect(im.x, im.y, w, h, 0.0, 0.0),
                            FillRule::NonZero,
                            bf_svg::Transform::IDENTITY,
                        )],
                        texts: Vec::new(),
                    }),
                    mask: None,
                    children: vec![Node::Group(Group {
                        host_units: false,
                        transform: inner,
                        opacity: 1.0,
                        clip: None,
                        mask: None,
                        children: vec![Node::Group(tree.root)],
                    })],
                };
                // `non-scaling-stroke` dentro: el host es el viewport del SVG referenciado.
                let (sx, sy) = axis_scales(&ts.pre_concat(sk(&inner)));
                let host = std::mem::replace(&mut self.host_scale, (sx * sy).sqrt());
                self.depth += 1;
                self.group(&group, canvas, ts);
                self.depth -= 1;
                self.host_scale = host;
            }
        }
    }

    fn paint(
        &mut self,
        p: &Path,
        fill: Option<&Fill>,
        stroke: Option<&Stroke>,
        canvas: &mut Pixmap,
        ts: Transform,
    ) {
        if let Some(f) = fill {
            let mut slot = None;
            if let Some(shader) = self.paint_shader(&f.paint, f.opacity, ts, &mut slot) {
                let paint = tiny_skia::Paint {
                    shader,
                    anti_alias: true,
                    ..tiny_skia::Paint::default()
                };
                canvas.fill_path(p, &paint, rule(f.rule), ts, None);
            }
        }
        if let Some(s) = stroke {
            let mut slot = None;
            let Some(mut shader) = self.paint_shader(&s.paint, s.opacity, ts, &mut slot) else {
                return;
            };
            if !s.non_scaling {
                let paint = tiny_skia::Paint {
                    shader,
                    anti_alias: true,
                    ..tiny_skia::Paint::default()
                };
                canvas.stroke_path(p, &paint, &stroke_style(s), ts, None);
                return;
            }
            // `non-scaling-stroke` (SVG 2 §13.6.2): el trazado se lleva al dispositivo y se
            // traza allí con el ancho y los guiones en px del host.
            let Some(dev) = p.clone().transform(ts) else {
                return;
            };
            shader.transform(ts);
            let k = self.host_scale;
            let mut style = stroke_style(&Stroke {
                width: s.width * k,
                dasharray: s.dasharray.iter().map(|d| d * k).collect(),
                dashoffset: s.dashoffset * k,
                ..s.clone()
            });
            style.miter_limit = s.miter_limit;
            let paint = tiny_skia::Paint {
                shader,
                anti_alias: true,
                ..tiny_skia::Paint::default()
            };
            canvas.stroke_path(&dev, &paint, &style, Transform::identity(), None);
        }
    }

    /// Los glifos de cada fragmento como paths en coordenadas de usuario, con su relleno y
    /// trazo. Cada *chunk* se mide entero para aplicar `text-anchor`.
    fn text_paths(&self, t: &Text) -> Vec<(Path, Option<Fill>, Option<Stroke>, bool)> {
        let Some(fonts) = self.fonts else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let (mut pen_x, mut pen_y) = (0.0f32, 0.0f32);
        for chunk in &t.chunks {
            if let Some(x) = chunk.x {
                pen_x = x;
            }
            if let Some(y) = chunk.y {
                pen_y = y;
            }
            let shaped = shape_chunk(fonts, chunk);
            let width: f32 = shaped
                .iter()
                .map(|(span, glyphs)| span.dx + glyphs.iter().map(|g| g.advance).sum::<f32>())
                .sum();
            pen_x -= match chunk.anchor {
                TextAnchor::Start => 0.0,
                TextAnchor::Middle => width / 2.0,
                TextAnchor::End => width,
            };
            for (span, glyphs) in shaped {
                pen_x += span.dx;
                pen_y += span.dy;
                let mut pb = PathBuilder::new();
                for g in &glyphs {
                    if let Some(outline) = fonts.outline(g.font, g.id) {
                        let k = span.size / outline.units_per_em;
                        let (ox, oy) = (pen_x + g.x_offset, pen_y - g.y_offset);
                        let map = |x: f32, y: f32| (ox + x * k, oy - y * k);
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
                    pen_x += g.advance;
                }
                if let Some(p) = pb.finish() {
                    out.push((p, span.fill.clone(), span.stroke.clone(), span.stroke_first));
                }
            }
        }
        out
    }
}

/// Familia de CSS a partir de los nombres de `font-family`.
fn family(names: &[String]) -> FontFamily {
    const GENERIC: &[&str] = &[
        "serif",
        "sans-serif",
        "monospace",
        "cursive",
        "fantasy",
        "system-ui",
    ];
    FontFamily(
        names
            .iter()
            .map(|n| {
                if GENERIC.contains(&n.to_ascii_lowercase().as_str()) {
                    FamilyName::Generic(n.to_ascii_lowercase())
                } else {
                    FamilyName::Named(n.clone())
                }
            })
            .collect(),
    )
}

fn shape_chunk<'c>(
    fonts: &dyn FontProvider,
    chunk: &'c Chunk,
) -> Vec<(&'c bf_svg::Span, Vec<bf_layout::Glyph>)> {
    chunk
        .spans
        .iter()
        .map(|span| {
            let fam = family(&span.family);
            let req = FontRequest {
                family: &fam,
                size: span.size,
                weight: span.weight,
                italic: span.italic,
            };
            (span, fonts.shape(&span.text, &req))
        })
        .collect()
}
