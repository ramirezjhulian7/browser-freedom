P = 'D:/Personales/Repos/browser-freedom-army-e7/crates/bf-paint/src/images/svg.rs'
s = open(P, encoding='utf-8').read()


def rep(a, b, cnt=1):
    global s
    assert s.count(a) == cnt, (a[:80], s.count(a))
    s = s.replace(a, b)


rep('''//! opacidad y su máscara; el texto usa las fuentes de `bf-font` (vía [`FontProvider`]).''',
    '''//! opacidad y su máscara (`clipPath` y `mask`); los `pattern` se pintan en una tesela que se
//! repite; el texto usa las fuentes de `bf-font` (vía [`FontProvider`]).''')
rep('''use bf_svg::{
    Chunk, ClipPath, Fill, FillRule, Gradient, GradientKind, Group, LineCap, LineJoin, Node, Paint,
    Rgba, Seg, Spread, Stroke, Text, TextAnchor, Tree,
};
use tiny_skia::{
    Color, FillRule as SkFillRule, GradientStop, LinearGradient, Mask, Path, PathBuilder, Pixmap,
    PixmapPaint, Point, RadialGradient, Shader, SpreadMode, StrokeDash, Transform,
};''', '''use bf_svg::{
    Chunk, ClipPath, Fill, FillRule, Gradient, GradientKind, Group, LineCap, LineJoin, Node, Paint,
    Pattern, Rgba, Seg, Spread, Stroke, Text, TextAnchor, Tree,
};
use tiny_skia::{
    Color, FillRule as SkFillRule, FilterQuality, GradientStop, LinearGradient, Mask, MaskType,
    Path, PathBuilder, Pixmap, PixmapPaint, Point, RadialGradient, Rect, Shader, SpreadMode,
    StrokeDash, Transform,
};

/// Tope de lado de la tesela de un `pattern` en píxeles.
const MAX_TILE: f32 = 2048.0;''')
rep('''    let base = Transform::from_scale(width as f32 / tree.width, height as f32 / tree.height);
    let mut r = Raster { fonts };''', '''    let (kx, ky) = (width as f32 / tree.width, height as f32 / tree.height);
    let base = Transform::from_scale(kx, ky);
    // El espacio del host de `non-scaling-stroke` es el del viewport raíz en px de CSS.
    let mut r = Raster {
        fonts,
        host_scale: (kx * ky).sqrt(),
    };''')
rep('''fn shader(paint: &Paint, opacity: f32) -> Option<Shader<'static>> {
    match paint {
        Paint::Color(c) => Some(Shader::SolidColor(color(*c, opacity))),
        Paint::Gradient(g) => gradient(g, opacity),
    }
}''', '''fn shader(paint: &Paint, opacity: f32) -> Option<Shader<'static>> {
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
}''')
rep('''struct Raster<'f> {
    fonts: Option<&'f dyn FontProvider>,
}''', '''struct Raster<'f> {
    fonts: Option<&'f dyn FontProvider>,
    /// Píxeles por px de CSS del viewport raíz.
    host_scale: f32,
}''')
rep('''        let layered = g.opacity < 1.0 || g.clip.is_some();''',
    '''        let layered = g.opacity < 1.0 || g.clip.is_some() || g.mask.is_some();''')
rep('''        let mask = g.clip.as_ref().and_then(|c| self.clip_mask(c, canvas, ts));
        if g.clip.is_some() && mask.is_none() {
            return;
        }''', '''        let mut mask = g.clip.as_ref().and_then(|c| self.clip_mask(c, canvas, ts));
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
        }''')
rep('''    fn node(&mut self, n: &Node, canvas: &mut Pixmap, ts: Transform) {''', '''    /// `mask` (CSS Masking 1 §7.1): el contenido pintado en una capa, su luminancia (o su alfa)
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

    fn node(&mut self, n: &Node, canvas: &mut Pixmap, ts: Transform) {''')
rep('''            Node::Text(t) => {
                let ts = ts.pre_concat(sk(&t.transform));
                for (p, fill, stroke) in self.text_paths(t) {
                    self.paint(&p, fill.as_ref(), stroke.as_ref(), canvas, ts);
                }
            }''', '''            Node::Text(t) => {
                let ts = ts.pre_concat(sk(&t.transform));
                for (p, fill, stroke, stroke_first) in self.text_paths(t) {
                    if stroke_first {
                        self.paint(&p, None, stroke.as_ref(), canvas, ts);
                        self.paint(&p, fill.as_ref(), None, canvas, ts);
                    } else {
                        self.paint(&p, fill.as_ref(), stroke.as_ref(), canvas, ts);
                    }
                }
            }''')
rep('''    fn paint(
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
    }''', '''    fn paint(
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
    }''')
rep('''    fn text_paths(&self, t: &Text) -> Vec<(Path, Option<Fill>, Option<Stroke>)> {''',
    '''    fn text_paths(&self, t: &Text) -> Vec<(Path, Option<Fill>, Option<Stroke>, bool)> {''')
rep('''                    out.push((p, span.fill.clone(), span.stroke.clone()));''',
    '''                    out.push((p, span.fill.clone(), span.stroke.clone(), span.stroke_first));''')
rep('''        for text in &clip.texts {
            for (p, _, _) in self.text_paths(text) {''', '''        for text in &clip.texts {
            for (p, ..) in self.text_paths(text) {''')
open(P, 'w', encoding='utf-8', newline='\n').write(s)
print('ok')
