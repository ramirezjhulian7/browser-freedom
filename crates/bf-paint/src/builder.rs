//! Construcción de la lista de visualización desde el árbol de fragmentos, en el orden de
//! CSS 2.2 Apéndice E.
//!
//! Crean stacking context: el elemento raíz, las cajas posicionadas con `z-index` distinto de
//! `auto`, `position: fixed` y `opacity < 1`. Las posicionadas con `z-index: auto`, los floats
//! y los inline-level atómicos se pintan "como si" crearan uno (sus descendientes posicionados
//! pertenecen al stacking context padre). Para cada stacking context:
//!
//! 1. fondo y bordes de la raíz;
//! 2. hijos con `z-index` negativo;
//! 3. fondos y bordes de los descendientes block-level en flujo;
//! 4. floats;
//! 5. contenido inline (fondos y bordes de cajas inline, texto, decoraciones, atómicos);
//! 6. posicionados con `z-index: auto` o `0`, en orden de árbol;
//! 7. hijos con `z-index` positivo.
//!
//! El recorte de `overflow` se registra por fragmento con la cadena de recortes de sus
//! ancestros en el árbol de fragmentos (que sigue la cadena de containing blocks).

use std::collections::HashMap;

use bf_css::selectors::PseudoElement;
use bf_dom::{Document, NodeId};
use bf_layout::controls::select_arrow;
use bf_layout::fragment::{Edges, Fragment, FragmentKind, FragmentTree, Rect};
use bf_layout::{FontProvider, Widget, font_request};
use bf_style::ComputedStyle;
use bf_style::properties::types::{
    BackgroundBox, BgSize, Float, Image, Position, RepeatStyle, Visibility,
};

use crate::images::PageImages;
use bf_style::values::color::Rgba;

use crate::display_list::{DisplayItem, DisplayList, GlyphInstance, Radii};

#[derive(Debug, Clone, PartialEq)]
struct Clip {
    rect: Rect,
    radii: Radii,
}

type ClipChain = Vec<Clip>;

struct Entry<'a> {
    f: &'a Fragment,
    clip: ClipChain,
    /// Stacking context real (si no, posicionado con `z-index: auto`).
    real: bool,
    z: i32,
}

#[derive(Default)]
struct Layers<'a> {
    negative: Vec<Entry<'a>>,
    zero: Vec<Entry<'a>>,
    positive: Vec<Entry<'a>>,
}

struct Builder<'a> {
    fonts: &'a dyn FontProvider,
    images: &'a PageImages,
    list: DisplayList,
    /// Cadena de recortes activa en la lista emitida hasta ahora.
    emitted: ClipChain,
    /// Orden de árbol (preorden) de cada nodo.
    order: HashMap<NodeId, usize>,
    /// Fragmento cuyo fondo se propagó al canvas.
    propagated: Option<&'a Fragment>,
}

/// Índice de la capa `i` en una lista que se repite hasta la cantidad de imágenes (CSS
/// Backgrounds 3 §2.2).
fn pick<T>(v: &[T], i: usize) -> usize {
    i % v.len().max(1)
}

fn is_box(f: &Fragment) -> bool {
    !matches!(f.kind, FragmentKind::Line | FragmentKind::Text)
}

fn positioned(f: &Fragment) -> bool {
    is_box(f) && f.style.position != Position::Static
}

fn real_sc(f: &Fragment) -> bool {
    is_box(f)
        && ((positioned(f) && f.z_index.is_some())
            || f.style.opacity.0 < 1.0
            || f.style.position == Position::Fixed)
}

fn is_float(f: &Fragment) -> bool {
    f.kind == FragmentKind::Block && f.style.float != Float::None && !positioned(f)
}

fn background(style: &ComputedStyle) -> Rgba {
    style.background_color.resolve(style.color.rgba())
}

/// Radios de la border box (CSS Backgrounds 3 §5).
fn border_radii(style: &ComputedStyle, rect: &Rect) -> Radii {
    let r = |c: &bf_style::properties::types::CornerRadius| {
        (c.0.resolve(rect.width), c.1.resolve(rect.height))
    };
    Radii([
        r(&style.border_top_left_radius),
        r(&style.border_top_right_radius),
        r(&style.border_bottom_right_radius),
        r(&style.border_bottom_left_radius),
    ])
    .fit(rect)
}

fn visible(style: &ComputedStyle) -> bool {
    style.visibility == Visibility::Visible
}

impl<'a> Builder<'a> {
    fn tree_key(&self, f: &Fragment) -> (usize, u8) {
        let index = f
            .node
            .and_then(|n| self.order.get(&n).copied())
            .unwrap_or(usize::MAX);
        let pseudo = match f.pseudo {
            None => 1,
            Some(PseudoElement::Before | PseudoElement::Marker) => 0,
            Some(_) => 2,
        };
        (index, pseudo)
    }

    fn set_clip(&mut self, clip: &ClipChain) {
        let common = self
            .emitted
            .iter()
            .zip(clip)
            .take_while(|(a, b)| a == b)
            .count();
        while self.emitted.len() > common {
            self.emitted.pop();
            self.list.push(DisplayItem::PopClip);
        }
        for c in &clip[common..] {
            self.list.push(DisplayItem::PushClip {
                rect: c.rect,
                radii: c.radii,
            });
            self.emitted.push(c.clone());
        }
    }

    fn emit(&mut self, clip: &ClipChain, item: DisplayItem) {
        self.set_clip(clip);
        self.list.push(item);
    }

    /// La cadena de recortes para los hijos de `f`.
    fn inner_clip(&self, clip: &ClipChain, f: &Fragment) -> ClipChain {
        let mut chain = clip.clone();
        if let Some(rect) = f.clip {
            let radii = border_radii(&f.style, &f.rect).shrink(&f.border);
            chain.push(Clip { rect, radii });
        }
        chain
    }

    // --- Stacking contexts ---

    fn collect(&self, f: &'a Fragment, clip: &ClipChain, layers: &mut Layers<'a>) {
        let inner = self.inner_clip(clip, f);
        for c in &f.children {
            if positioned(c) || real_sc(c) {
                let real = real_sc(c);
                let z = if real { c.z_index.unwrap_or(0) } else { 0 };
                let entry = Entry {
                    f: c,
                    clip: inner.clone(),
                    real,
                    z,
                };
                match z {
                    z if z < 0 => layers.negative.push(entry),
                    0 => layers.zero.push(entry),
                    _ => layers.positive.push(entry),
                }
                if !real {
                    self.collect(c, &inner, layers);
                }
            } else {
                self.collect(c, &inner, layers);
            }
        }
    }

    fn stacking_context(&mut self, f: &'a Fragment, clip: &ClipChain) {
        let opacity = f.style.opacity.0;
        if opacity < 1.0 {
            self.set_clip(clip);
            self.list.push(DisplayItem::PushOpacity(opacity.max(0.0)));
        }
        let mut layers = Layers::default();
        self.collect(f, clip, &mut layers);
        let mut negative = std::mem::take(&mut layers.negative);
        let mut zero = std::mem::take(&mut layers.zero);
        let mut positive = std::mem::take(&mut layers.positive);
        negative.sort_by_key(|e| (e.z, self.tree_key(e.f)));
        zero.sort_by_key(|e| self.tree_key(e.f));
        positive.sort_by_key(|e| (e.z, self.tree_key(e.f)));

        self.decorations(f, clip);
        if f.kind == FragmentKind::Replaced {
            self.replaced_content(f, clip);
        }
        for e in &negative {
            self.child_context(e);
        }
        self.flow(f, clip);
        for e in &zero {
            if e.real {
                self.child_context(e);
            } else {
                self.pseudo_context(e.f, &e.clip);
            }
        }
        for e in &positive {
            self.child_context(e);
        }
        if opacity < 1.0 {
            self.set_clip(clip);
            self.list.push(DisplayItem::PopOpacity);
        }
    }

    /// Un stacking context hijo. Los de `position: fixed` no se desplazan con el scroll ni los
    /// recortan los `overflow` de sus ancestros (su containing block es el viewport).
    fn child_context(&mut self, e: &Entry<'a>) {
        if e.f.style.position != Position::Fixed {
            self.stacking_context(e.f, &e.clip);
            return;
        }
        let none = Vec::new();
        self.set_clip(&none);
        self.list.push(DisplayItem::PushFixed);
        self.stacking_context(e.f, &none);
        self.set_clip(&none);
        self.list.push(DisplayItem::PopFixed);
    }

    /// Floats, atómicos inline y posicionados con `z-index: auto`: como un stacking context,
    /// pero sin sus descendientes posicionados (que pinta el padre).
    fn pseudo_context(&mut self, f: &'a Fragment, clip: &ClipChain) {
        self.decorations(f, clip);
        if f.kind == FragmentKind::Replaced {
            self.replaced_content(f, clip);
        }
        self.flow(f, clip);
    }

    /// Pasos 3 a 5 para el contenido de `f`.
    fn flow(&mut self, f: &'a Fragment, clip: &ClipChain) {
        let inner = self.inner_clip(clip, f);
        self.block_backgrounds(f, &inner);
        self.floats(f, &inner);
        let mut decorating = Vec::new();
        if f.kind != FragmentKind::Text {
            decorating.push(&*f.style);
        }
        self.inline_content(f, &inner, &mut decorating);
    }

    fn skip(c: &Fragment) -> bool {
        positioned(c) || real_sc(c)
    }

    /// Paso 3.
    fn block_backgrounds(&mut self, f: &'a Fragment, clip: &ClipChain) {
        for c in &f.children {
            if Self::skip(c) || is_float(c) {
                continue;
            }
            if matches!(c.kind, FragmentKind::Block | FragmentKind::Replaced) {
                self.decorations(c, clip);
                if c.kind == FragmentKind::Replaced {
                    self.replaced_content(c, clip);
                }
                let inner = self.inner_clip(clip, c);
                self.block_backgrounds(c, &inner);
            }
        }
    }

    /// Paso 4.
    fn floats(&mut self, f: &'a Fragment, clip: &ClipChain) {
        for c in &f.children {
            if Self::skip(c) {
                continue;
            }
            if is_float(c) {
                self.pseudo_context(c, clip);
            } else if c.kind == FragmentKind::Block {
                let inner = self.inner_clip(clip, c);
                self.floats(c, &inner);
            }
        }
    }

    /// Paso 5. `decorating` son los estilos de las cajas cuyas decoraciones de texto se
    /// propagan al texto que contienen (CSS Text Decoration 3 §2).
    fn inline_content(
        &mut self,
        f: &'a Fragment,
        clip: &ClipChain,
        decorating: &mut Vec<&'a ComputedStyle>,
    ) {
        for c in &f.children {
            if Self::skip(c) || is_float(c) {
                continue;
            }
            match c.kind {
                FragmentKind::Line => self.inline_content(c, clip, decorating),
                FragmentKind::Block => {
                    let inner = self.inner_clip(clip, c);
                    decorating.push(&c.style);
                    self.inline_content(c, &inner, decorating);
                    decorating.pop();
                }
                FragmentKind::Inline => {
                    self.decorations(c, clip);
                    let inner = self.inner_clip(clip, c);
                    decorating.push(&c.style);
                    self.inline_content(c, &inner, decorating);
                    decorating.pop();
                }
                FragmentKind::Text => self.text(c, clip, decorating),
                FragmentKind::Marker => self.marker(c, clip),
                FragmentKind::InlineBlock => self.pseudo_context(c, clip),
                FragmentKind::Replaced => {
                    // Los reemplazados block-level ya se pintaron en el paso 3.
                    if self.in_line(f) {
                        self.decorations(c, clip);
                        self.replaced_content(c, clip);
                    }
                }
            }
        }
    }

    fn in_line(&self, parent: &Fragment) -> bool {
        matches!(parent.kind, FragmentKind::Line | FragmentKind::Inline)
    }

    // --- Pintura de una caja ---

    /// Fondo y bordes de `f`.
    fn decorations(&mut self, f: &'a Fragment, clip: &ClipChain) {
        if !is_box(f) || f.kind == FragmentKind::Marker || !visible(&f.style) {
            return;
        }
        let s = &f.style;
        let rect = f.rect;
        let radii = border_radii(s, &rect);
        let is_propagated = self.propagated.is_some_and(|p| std::ptr::eq(p, f));
        let bg = background(s);
        // background-clip (CSS Backgrounds 3 §3.7): el área pintada y sus radios interiores.
        // `text` y `border-area` todavía se pintan como `border-box`.
        let (bg_rect, bg_radii) = match s.background_clip.for_color(s.background_image.0.len()) {
            BackgroundBox::PaddingBox => (f.padding_rect(), radii.shrink(&f.border)),
            BackgroundBox::ContentBox => {
                let inset = Edges {
                    top: f.border.top + f.padding.top,
                    right: f.border.right + f.padding.right,
                    bottom: f.border.bottom + f.padding.bottom,
                    left: f.border.left + f.padding.left,
                };
                (f.content_rect(), radii.shrink(&inset))
            }
            _ => (rect, radii),
        };
        if !is_propagated && bg.a > 0.0 && bg_rect.width > 0.0 && bg_rect.height > 0.0 {
            self.emit(
                clip,
                DisplayItem::Rect {
                    rect: bg_rect,
                    color: bg,
                    radii: bg_radii,
                },
            );
        }
        if !is_propagated {
            self.background_images(f, None, clip);
        }
        let widths = f.border;
        if widths.top + widths.right + widths.bottom + widths.left > 0.0 {
            let current = s.color.rgba();
            let border = DisplayItem::Border {
                rect,
                widths,
                colors: [
                    s.border_top_color.resolve(current),
                    s.border_right_color.resolve(current),
                    s.border_bottom_color.resolve(current),
                    s.border_left_color.resolve(current),
                ],
                styles: [
                    s.border_top_style,
                    s.border_right_style,
                    s.border_bottom_style,
                    s.border_left_style,
                ],
                radii,
            };
            match f.legend {
                // La leyenda de un fieldset corta el borde superior (HTML §15.3.13): el borde se
                // pinta en tres franjas que dejan libre el tramo bajo la leyenda.
                Some(legend) => {
                    let parts = [
                        Rect::new(rect.x, rect.y, legend.x - rect.x, rect.height),
                        Rect::new(
                            legend.right(),
                            rect.y,
                            rect.right() - legend.right(),
                            rect.height,
                        ),
                        Rect::new(
                            legend.x,
                            rect.y + widths.top,
                            legend.width,
                            rect.height - widths.top,
                        ),
                    ];
                    self.set_clip(clip);
                    for part in parts {
                        if part.width <= 0.0 || part.height <= 0.0 {
                            continue;
                        }
                        self.list.push(DisplayItem::PushClip {
                            rect: part,
                            radii: Radii::default(),
                        });
                        self.list.push(border.clone());
                        self.list.push(DisplayItem::PopClip);
                    }
                }
                None => self.emit(clip, border),
            }
        }
        if let Some(widget) = f.widget {
            let content = f.content_rect();
            let rect = match widget {
                // La flecha ocupa el final del content box.
                Widget::SelectArrow => {
                    let w = select_arrow(s.font_size_px()).min(content.width);
                    Rect::new(content.right() - w, content.y, w, content.height)
                }
                _ => content,
            };
            self.emit(
                clip,
                DisplayItem::Widget {
                    rect,
                    widget,
                    color: s.color.rgba(),
                },
            );
        }
    }

    /// Las capas de `background-image` de `f`, de la última (abajo) a la primera (CSS
    /// Backgrounds 3 §3). `canvas`: área pintada del canvas cuando el fondo se propagó.
    ///
    /// TODO(backgrounds): `background-attachment: fixed` se pinta como `scroll`; los gradientes
    /// (`<gradient>`) todavía no se dibujan.
    fn background_images(&mut self, f: &Fragment, canvas: Option<Rect>, clip: &ClipChain) {
        let s = &f.style;
        let layers = &s.background_image.0;
        let n = layers.len();
        let radii = border_radii(s, &f.rect);
        let content_inset = Edges {
            top: f.border.top + f.padding.top,
            right: f.border.right + f.padding.right,
            bottom: f.border.bottom + f.padding.bottom,
            left: f.border.left + f.padding.left,
        };
        for i in (0..n).rev() {
            let Image::Url(url) = &layers[i] else {
                continue;
            };
            let Some(image) = self.images.for_url(url).cloned() else {
                continue;
            };
            let boxed = |b: BackgroundBox| match b {
                BackgroundBox::PaddingBox => (f.padding_rect(), radii.shrink(&f.border)),
                BackgroundBox::ContentBox => (f.content_rect(), radii.shrink(&content_inset)),
                _ => (f.rect, radii),
            };
            let origin = s.background_origin.0[pick(&s.background_origin.0, i)];
            let clip_box = s.background_clip.0[pick(&s.background_clip.0, i)];
            let (positioning, _) = boxed(origin);
            let (mut area, mut area_radii) = boxed(clip_box);
            if let Some(c) = canvas {
                area = c;
                area_radii = Radii::default();
            }
            if area.width <= 0.0 || area.height <= 0.0 {
                continue;
            }
            let (iw, ih) = (image.width() as f32, image.height() as f32);
            let size = &s.background_size.0[pick(&s.background_size.0, i)];
            let (pw, ph) = (positioning.width, positioning.height);
            let (mut w, mut h) = match size {
                BgSize::Cover | BgSize::Contain => {
                    let (sx, sy) = (pw / iw, ph / ih);
                    let k = if matches!(size, BgSize::Cover) {
                        sx.max(sy)
                    } else {
                        sx.min(sy)
                    };
                    (iw * k, ih * k)
                }
                BgSize::Explicit(sw, sh) => {
                    let w = sw.as_ref().map(|l| l.resolve(pw));
                    let h = sh.as_ref().map(|l| l.resolve(ph));
                    match (w, h) {
                        (Some(w), Some(h)) => (w, h),
                        (Some(w), None) => (w, if iw > 0.0 { w * ih / iw } else { ih }),
                        (None, Some(h)) => (if ih > 0.0 { h * iw / ih } else { iw }, h),
                        (None, None) => (iw, ih),
                    }
                }
            };
            let (rx, ry) = s.background_repeat.0[pick(&s.background_repeat.0, i)];
            // `round` (§3.5): se ajusta el tamaño para que entre un número entero de tiles.
            let round = |len: f32, t: f32| {
                if t <= 0.0 {
                    return t;
                }
                len / (len / t).round().max(1.0)
            };
            let (orig_w, orig_h) = (w, h);
            if rx == RepeatStyle::Round {
                w = round(pw, w);
            }
            if ry == RepeatStyle::Round {
                h = round(ph, h);
            }
            if rx == RepeatStyle::Round
                && ry != RepeatStyle::Round
                && matches!(size, BgSize::Explicit(_, None) | BgSize::Explicit(None, _))
                && orig_w > 0.0
            {
                h = orig_h * w / orig_w;
            }
            if w <= 0.0 || h <= 0.0 {
                continue;
            }
            let (px, py) = &s.background_position.0[pick(&s.background_position.0, i)];
            let x = positioning.x + px.resolve(pw - w);
            let y = positioning.y + py.resolve(ph - h);
            // `space`: tiles enteros con el espacio repartido entre ellos; con uno solo, como
            // `no-repeat` en la posición.
            let positions = |r: RepeatStyle, start: f32, pos: f32, len: f32, t: f32| match r {
                RepeatStyle::NoRepeat => Some(vec![pos]),
                RepeatStyle::Space => {
                    let count = (len / t).floor();
                    if count >= 2.0 {
                        let gap = (len - count * t) / (count - 1.0);
                        Some(
                            (0..count as usize)
                                .map(|k| start + k as f32 * (t + gap))
                                .collect(),
                        )
                    } else {
                        Some(vec![pos])
                    }
                }
                _ => None,
            };
            let xs = positions(rx, positioning.x, x, pw, w);
            let ys = positions(ry, positioning.y, y, ph, h);
            self.emit(
                clip,
                DisplayItem::BackgroundImage {
                    area,
                    radii: area_radii,
                    tile: Rect::new(x, y, w, h),
                    xs,
                    ys,
                    image,
                },
            );
        }
    }

    /// Contenido de un `<img>` (o `<input type=image>`): la imagen en el content box.
    fn replaced_content(&mut self, f: &Fragment, clip: &ClipChain) {
        if !visible(&f.style) {
            return;
        }
        let Some(image) = f.node.and_then(|n| self.images.for_node(n)).cloned() else {
            return;
        };
        let rect = f.content_rect();
        if rect.width > 0.0 && rect.height > 0.0 {
            self.emit(clip, DisplayItem::Image { rect, image });
        }
    }

    /// Marcador con `list-style-image`: la imagen en lugar del texto, apoyada en la baseline
    /// y terminando donde termina el marcador.
    fn marker(&mut self, f: &'a Fragment, clip: &ClipChain) {
        if let Image::Url(url) = &f.style.list_style_image
            && let Some(image) = self.images.for_url(url).cloned()
        {
            let (w, h) = (image.width() as f32, image.height() as f32);
            let baseline = f.baseline.unwrap_or(f.rect.bottom());
            let rect = Rect::new(f.rect.right() - w, baseline - h, w, h);
            if visible(&f.style) {
                self.emit(clip, DisplayItem::Image { rect, image });
            }
            return;
        }
        self.text(f, clip, &[]);
    }

    fn text(&mut self, f: &'a Fragment, clip: &ClipChain, decorating: &[&'a ComputedStyle]) {
        let Some((_, glyphs)) = &f.text else {
            return;
        };
        if !visible(&f.style) || glyphs.is_empty() {
            return;
        }
        let color = f.style.color.rgba();
        let metrics = self.fonts.metrics(&font_request(&f.style));
        let baseline = f.baseline.unwrap_or(f.rect.y + metrics.ascent);
        let lines: Vec<(&ComputedStyle, Edges)> = decorating
            .iter()
            .filter(|d| {
                let l = d.text_decoration_line;
                l.underline || l.overline || l.line_through
            })
            .map(|d| (*d, self.decoration_offsets(d, baseline)))
            .collect();
        // Subrayado y sobrerrayado debajo del texto; tachado encima.
        for (d, e) in &lines {
            let l = d.text_decoration_line;
            if l.underline {
                self.decoration_line(clip, f, d, e.bottom, e.left);
            }
            if l.overline {
                self.decoration_line(clip, f, d, e.top, e.left);
            }
        }
        let size = f.style.font_size_px();
        let mut pen = f.rect.x;
        let mut run: Vec<GlyphInstance> = Vec::new();
        let mut run_font = glyphs[0].font;
        for g in glyphs {
            if g.font != run_font && !run.is_empty() && color.a > 0.0 {
                self.emit(
                    clip,
                    DisplayItem::Text {
                        glyphs: std::mem::take(&mut run),
                        font: run_font,
                        size,
                        color,
                        origin: (0.0, baseline),
                        y_fit: self.y_fit(run_font, size),
                    },
                );
            }
            run_font = g.font;
            run.push(GlyphInstance {
                id: g.id,
                x: pen + g.x_offset,
                y: -g.y_offset,
                outline: self.fonts.outline(g.font, g.id),
            });
            pen += g.advance;
        }
        if !run.is_empty() && color.a > 0.0 {
            self.emit(
                clip,
                DisplayItem::Text {
                    glyphs: run,
                    font: run_font,
                    size,
                    color,
                    origin: (0.0, baseline),
                    y_fit: self.y_fit(run_font, size),
                },
            );
        }
        for (d, e) in &lines {
            if d.text_decoration_line.line_through {
                self.decoration_line(clip, f, d, e.right, e.left);
            }
        }
    }

    fn y_fit(&self, font: bf_layout::FontKey, size: f32) -> (f32, f32) {
        if self.fonts.vertical_fit_enabled() {
            self.fonts.vertical_fit(font, size)
        } else {
            (1.0, 1.0)
        }
    }

    /// Posición (y superior) de subrayado (`bottom`), sobrerrayado (`top`) y tachado
    /// (`right`), y el grosor (`left`), según la fuente de la caja decorante.
    fn decoration_offsets(&self, d: &ComputedStyle, baseline: f32) -> Edges {
        let m = self.fonts.metrics(&font_request(d));
        let thickness = m.underline_thickness.max(1.0);
        Edges {
            top: baseline - m.ascent,
            bottom: baseline + m.underline_offset,
            right: baseline - m.strikeout_offset - m.strikeout_thickness.max(1.0) / 2.0,
            left: thickness,
        }
    }

    fn decoration_line(
        &mut self,
        clip: &ClipChain,
        f: &Fragment,
        d: &ComputedStyle,
        y: f32,
        thickness: f32,
    ) {
        let color = d.text_decoration_color.resolve(d.color.rgba());
        if color.a <= 0.0 || f.rect.width <= 0.0 {
            return;
        }
        self.emit(
            clip,
            DisplayItem::Line {
                rect: Rect::new(f.rect.x, y, f.rect.width, thickness),
                color,
                style: d.text_decoration_style,
            },
        );
    }
}

/// Extensión del documento: la unión de las cajas de todos los fragmentos.
pub fn document_extent(tree: &FragmentTree) -> Rect {
    fn walk(f: &Fragment, r: &mut (f32, f32)) {
        let m = f.margin_rect();
        r.0 = r.0.max(m.right());
        r.1 = r.1.max(m.bottom());
        if f.clip.is_none() {
            for c in &f.children {
                walk(c, r);
            }
        } else {
            for c in f.children.iter().filter(|c| positioned(c)) {
                walk(c, r);
            }
        }
    }
    let mut r = (tree.viewport.width, tree.viewport.height);
    if let Some(root) = &tree.root {
        walk(root, &mut r);
    }
    Rect::new(0.0, 0.0, r.0, r.1)
}

/// Lista de visualización del documento (CSS 2.2 Apéndice E).
pub fn build_display_list(
    doc: &Document,
    tree: &FragmentTree,
    fonts: &dyn FontProvider,
) -> DisplayList {
    build_display_list_with_images(doc, tree, fonts, &PageImages::default())
}

/// Como [`build_display_list`], con las imágenes decodificadas del documento.
pub fn build_display_list_with_images(
    doc: &Document,
    tree: &FragmentTree,
    fonts: &dyn FontProvider,
    images: &PageImages,
) -> DisplayList {
    let mut builder = Builder {
        fonts,
        images,
        list: DisplayList::default(),
        emitted: Vec::new(),
        order: doc
            .inclusive_descendants(doc.root())
            .into_iter()
            .enumerate()
            .map(|(i, n)| (n, i))
            .collect(),
        propagated: None,
    };
    // Raíz sin caja (`display: none`): el canvas queda sin fondo (CSS Backgrounds 3 §2.11.2;
    // CSS2/box-display/root-box-003 espera lo contrario, según CSS 2.1).
    let Some(root) = &tree.root else {
        return builder.list;
    };
    // Fondo del canvas (CSS Backgrounds 3 §2.11.2): el del elemento raíz o, si es
    // transparente y la raíz es `html`, el del primer hijo `body`.
    let root_is_html = root
        .node
        .and_then(|n| doc.element(n))
        .is_some_and(|e| e.is_html("html"));
    let body = root.children.iter().find(|c| {
        c.node
            .and_then(|n| doc.element(n))
            .is_some_and(|e| e.is_html("body"))
    });
    let has_background = |s: &ComputedStyle| {
        background(s).a > 0.0 || s.background_image.0.iter().any(|i| *i != Image::None)
    };
    let (canvas, source) = if has_background(&root.style) || !root_is_html {
        (background(&root.style), Some(root))
    } else if let Some(body) = body {
        (background(&body.style), Some(body))
    } else {
        (Rgba::TRANSPARENT, None)
    };
    builder.propagated = source;
    let extent = document_extent(tree);
    if canvas.a > 0.0 {
        builder.list.push(DisplayItem::Rect {
            rect: extent,
            color: canvas,
            radii: Radii::default(),
        });
    }
    // Las imágenes del fondo propagado cubren el canvas, posicionadas respecto del elemento.
    if let Some(src) = source {
        builder.background_images(src, Some(extent), &Vec::new());
    }
    builder.stacking_context(root, &Vec::new());
    builder.set_clip(&Vec::new());
    builder.list
}
