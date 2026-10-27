//! Rasterizado de un árbol de `bf-svg` con `tiny-skia`, al tamaño de píxeles pedido. Los
//! grupos con `opacity` o `clipPath` se pintan en una capa aparte y se componen con su
//! opacidad y su máscara; el texto usa las fuentes de `bf-font` (vía [`FontProvider`]).

use bf_layout::{FontProvider, FontRequest, PathCommand};
use bf_style::properties::types::{FamilyName, FontFamily};
use bf_svg::{
    Chunk, ClipPath, Fill, FillRule, Gradient, GradientKind, Group, LineCap, LineJoin, Node,
    Paint, Rgba, Seg, Spread, Stroke, Text, TextAnchor, Tree,
};
use tiny_skia::{
    Color, FillRule as SkFillRule, GradientStop, LinearGradient, Mask, Path, PathBuilder, Pixmap,
    PixmapPaint, Point, RadialGradient, Shader, SpreadMode, StrokeDash, Transform,
};

/// Tope de lado del bitmap (protege la memoria con tamaños absurdos).
const MAX_SIDE: u32 = 4096;

/// Pinta `tree` en un bitmap de `width`×`height` píxeles (el viewport del árbol escalado).
pub fn rasterize(
    tree: &Tree,
    width: u32,
    height: u32,
    fonts: Option<&dyn FontProvider>,
) -> Option<Pixmap> {
    let (width, height) = (width.clamp(1, MAX_SIDE), height.clamp(1, MAX_SIDE));
    let mut pixmap = Pixmap::new(width, height)?;
    if tree.width <= 0.0 || tree.height <= 0.0 {
        return Some(pixmap);
    }
    let base = Transform::from_scale(width as f32 / tree.width, height as f32 / tree.height);
    let mut r = Raster { fonts };
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
}

impl Raster<'_> {
    fn group(&mut self, g: &Group, canvas: &mut Pixmap, ts: Transform) {
        let ts = ts.pre_concat(sk(&g.transform));
        if g.opacity <= 0.0 {
            return;
        }
        let layered = g.opacity < 1.0 || g.clip.is_some();
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
        let mask = g.clip.as_ref().and_then(|c| self.clip_mask(c, canvas, ts));
        if g.clip.is_some() && mask.is_none() {
            return;
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
            for (p, _, _) in self.text_paths(text) {
                mask.fill_path(&p, SkFillRule::Winding, true, ts.pre_concat(sk(&text.transform)));
            }
        }
        Some(mask)
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
            Node::Text(t) => {
                let ts = ts.pre_concat(sk(&t.transform));
                for (p, fill, stroke) in self.text_paths(t) {
                    self.paint(&p, fill.as_ref(), stroke.as_ref(), canvas, ts);
                }
            }
        }
    }

    fn paint(
        &self,
        p: &Path,
        fill: Option<&Fill>,
        stroke: Option<&Stroke>,
        canvas: &mut Pixmap,
        ts: Transform,
    ) {
        if let Some(f) = fill
            && let Some(shader) = shader(&f.paint, f.opacity)
        {
            let paint = tiny_skia::Paint {
                shader,
                anti_alias: true,
                ..tiny_skia::Paint::default()
            };
            canvas.fill_path(p, &paint, rule(f.rule), ts, None);
        }
        if let Some(s) = stroke
            && let Some(shader) = shader(&s.paint, s.opacity)
        {
            let paint = tiny_skia::Paint {
                shader,
                anti_alias: true,
                ..tiny_skia::Paint::default()
            };
            canvas.stroke_path(p, &paint, &stroke_style(s), ts, None);
        }
    }

    /// Los glifos de cada fragmento como paths en coordenadas de usuario, con su relleno y
    /// trazo. Cada *chunk* se mide entero para aplicar `text-anchor`.
    fn text_paths(&self, t: &Text) -> Vec<(Path, Option<Fill>, Option<Stroke>)> {
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
                    out.push((p, span.fill.clone(), span.stroke.clone()));
                }
            }
        }
        out
    }
}

/// Familia de CSS a partir de los nombres de `font-family`.
fn family(names: &[String]) -> FontFamily {
    const GENERIC: &[&str] = &["serif", "sans-serif", "monospace", "cursive", "fantasy", "system-ui"];
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
