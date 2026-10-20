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
use bf_layout::fragment::{Edges, Fragment, FragmentKind, FragmentTree, Rect, TableRole};
use bf_layout::{FontProvider, Widget, font_request};
use bf_style::ComputedStyle;
use bf_style::properties::types::{
    BackgroundBox, BgSize, BorderStyle, Float, Image, Position, RepeatStyle, Visibility,
};

use crate::images::PageImages;
use bf_style::values::color::Rgba;

use crate::display_list::{DisplayItem, DisplayList, GlyphInstance, Radii};
use crate::effects::resolve_gradient; // E4

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
    /// Outlines del stacking context en curso, que se pintan al terminarlo (E4).
    outlines: Vec<(ClipChain, DisplayItem)>,
    /// Border boxes de los fragmentos de cada caja inline partida en varias líneas, en orden
    /// (E4: `box-shadow` con `box-decoration-break: slice`).
    inline_parts: HashMap<(NodeId, Option<PseudoElement>), Vec<Rect>>,
    /// El viewport (área de posicionamiento de `background-attachment: fixed`) (E4).
    viewport: Rect,
    /// Raíz del árbol de fragmentos (E4: glifos de `background-clip: text` en partes de tabla).
    root: Option<&'a Fragment>,
    /// Caja cuyo `overflow` se propaga al viewport (la raíz o, si la raíz tiene `visible`, el
    /// `body`): no recorta su contenido (E4).
    viewport_overflow: Option<&'a Fragment>,
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
        // E4: el `overflow` que se propaga al viewport no recorta la caja (CSS Overflow 3 §3.3).
        let to_viewport = self.viewport_overflow.is_some_and(|v| std::ptr::eq(v, f));
        if let Some(rect) = f.clip
            && !to_viewport
        {
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
        let outer_outlines = std::mem::take(&mut self.outlines); // E4
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
        self.flush_outlines(outer_outlines); // E4
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
        // E4: bordes colapsados de una tabla, tras los fondos de sus descendientes.
        self.table_collapsed_borders(f, clip);
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
        if !is_box(f) || f.kind == FragmentKind::Marker {
            return;
        }
        // E4: las partes y celdas de una tabla las pinta la tabla (CSS 2.2 Apéndice E).
        if matches!(f.table, Some(TableRole::Part | TableRole::Cell(_))) {
            if visible(&f.style) {
                self.collect_outline(f, clip);
            }
            return;
        }
        if !visible(&f.style) {
            if matches!(f.table, Some(TableRole::Table(_))) {
                self.table_contents(f, clip);
            }
            return;
        }
        let s = &f.style;
        let rect = f.rect;
        let radii = self.sliced_radii(f, border_radii(s, &rect)); // E4
        let is_propagated = self.propagated.is_some_and(|p| std::ptr::eq(p, f));
        self.box_shadows(f, clip, false); // E4: sombras exteriores, debajo del fondo
        self.collect_outline(f, clip); // E4
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
        let (bg_rect, bg_radii) = bleed_avoidance(f, bg_rect, bg_radii); // E4
        if !is_propagated && bg.a > 0.0 && bg_rect.width > 0.0 && bg_rect.height > 0.0 {
            let clip_box = s.background_clip.for_color(s.background_image.0.len());
            self.emit_background(
                f,
                clip,
                clip_box,
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
        self.box_shadows(f, clip, true); // E4: sombras interiores, entre el fondo y el borde
        let widths = self.sliced_widths(f, f.border); // E4
        // En el modelo colapsado los bordes de la tabla van con los de las celdas.
        let collapsed_table =
            matches!(&f.table, Some(TableRole::Table(t)) if t.collapsed.is_some());
        if widths.top + widths.right + widths.bottom + widths.left > 0.0 && !collapsed_table {
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
        if matches!(f.table, Some(TableRole::Table(_))) {
            self.table_contents(f, clip);
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
    /// Los `<gradient>` (CSS Images 3 §3) no tienen tamaño natural: con `auto`, `contain` o
    /// `cover` el tile es el área de posicionamiento, y con un solo valor explícito el otro es
    /// la dimensión del área (CSS Backgrounds 3 §3.9).
    ///
    /// TODO(backgrounds): `background-attachment: fixed` se pinta como `scroll`.
    fn background_images(&mut self, f: &Fragment, canvas: Option<Rect>, clip: &ClipChain) {
        /// La fuente de una capa: imagen decodificada o gradiente (E4).
        enum Source<'g> {
            Image(std::sync::Arc<tiny_skia::Pixmap>),
            Gradient(&'g bf_style::properties::gradient::Gradient),
        }
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
            let source = match &layers[i] {
                Image::Url(url) => match self.images.for_url(url) {
                    Some(image) => Source::Image(image.clone()),
                    None => continue,
                },
                Image::Gradient(g) => Source::Gradient(g),
                Image::None => continue,
            };
            let natural = match &source {
                Source::Image(image) => Some((image.width() as f32, image.height() as f32)),
                Source::Gradient(_) => None,
            };
            let boxed = |b: BackgroundBox| match b {
                BackgroundBox::PaddingBox => (f.padding_rect(), radii.shrink(&f.border)),
                BackgroundBox::ContentBox => (f.content_rect(), radii.shrink(&content_inset)),
                _ => (f.rect, radii),
            };
            let origin = s.background_origin.0[pick(&s.background_origin.0, i)];
            let clip_box = s.background_clip.0[pick(&s.background_clip.0, i)];
            let (mut positioning, _) = boxed(origin);
            // `background-attachment: fixed` (§3.4): el área de posicionamiento es el viewport
            // (E4; sin desplazamiento, el del documento).
            let attachment = s
                .background_attachment
                .0
                .get(pick(&s.background_attachment.0, i))
                .copied();
            if attachment == Some(bf_style::properties::types::Attachment::Fixed) {
                positioning = self.viewport;
            }
            // `local` en un contenedor con scroll: relativa al área de desborde desplazable (sin
            // desplazamiento, la que empieza en la padding box) (E4).
            if attachment == Some(bf_style::properties::types::Attachment::Local)
                && f.clip.is_some()
            {
                let content = extent_with_descendants(f, true);
                positioning.width = positioning
                    .width
                    .max(content.right() + f.padding.right - positioning.x);
                positioning.height = positioning
                    .height
                    .max(content.bottom() + f.padding.bottom - positioning.y);
            }
            let (mut area, mut area_radii) = boxed(clip_box);
            (area, area_radii) = bleed_avoidance(f, area, area_radii); // E4
            if let Some(c) = canvas {
                area = c;
                area_radii = Radii::default();
            }
            if area.width <= 0.0 || area.height <= 0.0 {
                continue;
            }
            let size = &s.background_size.0[pick(&s.background_size.0, i)];
            let (pw, ph) = (positioning.width, positioning.height);
            // Sin tamaño natural (gradientes): el área de posicionamiento (§3.9).
            let (iw, ih) = natural.unwrap_or((pw, ph));
            let (mut w, mut h) = match size {
                BgSize::Cover | BgSize::Contain if natural.is_none() => (pw, ph),
                BgSize::Explicit(sw, sh) if natural.is_none() => (
                    sw.as_ref().map_or(pw, |l| l.resolve(pw)),
                    sh.as_ref().map_or(ph, |l| l.resolve(ph)),
                ),
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
            // `space`: tiles enteros con el espacio repartido entre ellos, y la misma cadencia
            // repetida en todo el área pintada (E4); con uno solo, como `no-repeat` en la
            // posición.
            let positions =
                |r: RepeatStyle, start: f32, pos: f32, len: f32, t: f32, (a0, a1): (f32, f32)| {
                    match r {
                        RepeatStyle::NoRepeat => Some(vec![pos]),
                        RepeatStyle::Space => {
                            let count = (len / t).floor();
                            if count >= 2.0 {
                                let gap = (len - count * t) / (count - 1.0);
                                let period = t + gap;
                                let first = ((a0 - start) / period).floor() as i64 - 1;
                                let last = ((a1 - start) / period).ceil() as i64 + 1;
                                Some(
                                    (first..=last)
                                        .map(|k| start + k as f32 * period)
                                        .filter(|&p| p + t > a0 && p < a1)
                                        .collect(),
                                )
                            } else {
                                Some(vec![pos])
                            }
                        }
                        _ => None,
                    }
                };
            let xs = positions(rx, positioning.x, x, pw, w, (area.x, area.right()));
            let ys = positions(ry, positioning.y, y, ph, h, (area.y, area.bottom()));
            let tile = Rect::new(x, y, w, h);
            let item = match source {
                Source::Image(image) => DisplayItem::BackgroundImage {
                    area,
                    radii: area_radii,
                    tile,
                    xs,
                    ys,
                    image,
                },
                Source::Gradient(g) => {
                    let Some(gradient) = resolve_gradient(g, w, h, s.color.rgba()) else {
                        continue;
                    };
                    DisplayItem::Gradient {
                        area,
                        radii: area_radii,
                        tile,
                        xs,
                        ys,
                        gradient: std::sync::Arc::new(gradient),
                    }
                }
            };
            if canvas.is_some() {
                self.emit(clip, item);
            } else {
                self.emit_background(f, clip, clip_box, item);
            }
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
            // E4: el contenido reemplazado se recorta a la curva interior del content box
            // (CSS Backgrounds 3 §5.3).
            let radii = border_radii(&f.style, &f.rect).shrink(&Edges {
                top: f.border.top + f.padding.top,
                right: f.border.right + f.padding.right,
                bottom: f.border.bottom + f.padding.bottom,
                left: f.border.left + f.padding.left,
            });
            let mut clip = clip.clone();
            if !radii.is_zero() {
                clip.push(Clip { rect, radii });
            }
            self.emit(&clip, DisplayItem::Image { rect, image });
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
        // E4: con `text-shadow` el texto se emite aunque sea transparente (su sombra se ve).
        let shadowed = !f.style.text_shadow.0.is_empty();
        self.set_clip(clip);
        let start = self.list.items.len();
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
            if g.font != run_font && !run.is_empty() && (color.a > 0.0 || shadowed) {
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
        if !run.is_empty() && (color.a > 0.0 || shadowed) {
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
        self.text_shadows(f, start); // E4
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

// --- E4: tablas ---

impl Builder<'_> {
    /// Fondos de las capas de la tabla y de las celdas, y después todos los bordes (CSS 2.2
    /// Apéndice E, paso 4 para tablas, y §17.5.1).
    fn table_contents(&mut self, table: &Fragment, clip: &ClipChain) {
        let Some(TableRole::Table(paint)) = &table.table else {
            return;
        };
        let mut cells: Vec<&Fragment> = Vec::new();
        for group in &table.children {
            for row in &group.children {
                for cell in &row.children {
                    if let Some(TableRole::Cell(_)) = cell.table {
                        cells.push(cell);
                    }
                }
            }
        }
        for cell in &cells {
            let Some(TableRole::Cell(data)) = &cell.table else {
                continue;
            };
            if data.hidden || !visible(&cell.style) {
                continue;
            }
            // Grupos de columnas, columnas, grupos de filas y filas: su fondo (posicionado en
            // su propia caja) solo se ve dentro de la celda.
            let mut inner = clip.clone();
            inner.push(Clip {
                rect: cell.rect,
                radii: Radii::default(),
            });
            for (style, rect) in &data.layers {
                let mut part = Fragment::new(FragmentKind::Block, *rect, style.clone());
                part.anonymous = true;
                let bg = background(style);
                if bg.a > 0.0 {
                    self.emit(
                        &inner,
                        DisplayItem::Rect {
                            rect: *rect,
                            color: bg,
                            radii: Radii::default(),
                        },
                    );
                }
                self.background_images(&part, None, &inner);
            }
            self.box_shadows(cell, clip, false);
            self.cell_background(cell, clip);
            self.box_shadows(cell, clip, true);
        }
        if paint.collapsed.is_none() {
            for cell in &cells {
                let Some(TableRole::Cell(data)) = &cell.table else {
                    continue;
                };
                if !data.hidden && visible(&cell.style) {
                    self.cell_border(cell, clip);
                }
            }
        }
    }

    /// Bordes colapsados de una tabla: delante de los fondos y bordes de sus descendientes
    /// block-level (CSS Tables 3 §3.? y css-position-4, csswg-drafts#11570), así que se emiten
    /// al terminar el paso 3 de la tabla.
    fn table_collapsed_borders(&mut self, table: &Fragment, clip: &ClipChain) {
        let Some(TableRole::Table(paint)) = &table.table else {
            return;
        };
        match &paint.collapsed {
            None => {}
            Some(segments) => {
                if !visible(&table.style) {
                    return;
                }
                for seg in segments {
                    let none = BorderStyle::None;
                    let (widths, styles) = if seg.horizontal {
                        (
                            Edges {
                                top: seg.rect.height,
                                ..Edges::default()
                            },
                            [seg.style, none, none, none],
                        )
                    } else {
                        (
                            Edges {
                                left: seg.rect.width,
                                ..Edges::default()
                            },
                            [none, none, none, seg.style],
                        )
                    };
                    self.emit(
                        clip,
                        DisplayItem::Border {
                            rect: seg.rect,
                            widths,
                            colors: [seg.color; 4],
                            styles,
                            radii: Radii::default(),
                        },
                    );
                }
            }
        }
    }

    /// Fondo propio de una celda.
    fn cell_background(&mut self, cell: &Fragment, clip: &ClipChain) {
        let s = &cell.style;
        let radii = border_radii(s, &cell.rect);
        let bg = background(s);
        if bg.a > 0.0 && cell.rect.width > 0.0 && cell.rect.height > 0.0 {
            let (rect, radii) = match s.background_clip.for_color(s.background_image.0.len()) {
                BackgroundBox::PaddingBox => (cell.padding_rect(), radii.shrink(&cell.border)),
                BackgroundBox::ContentBox => {
                    let inset = Edges {
                        top: cell.border.top + cell.padding.top,
                        right: cell.border.right + cell.padding.right,
                        bottom: cell.border.bottom + cell.padding.bottom,
                        left: cell.border.left + cell.padding.left,
                    };
                    (cell.content_rect(), radii.shrink(&inset))
                }
                _ => (cell.rect, radii),
            };
            // E4: `border-area` y `text` recortan también el color.
            let clip_box = s.background_clip.for_color(s.background_image.0.len());
            self.emit_background(
                cell,
                clip,
                clip_box,
                DisplayItem::Rect {
                    rect,
                    color: bg,
                    radii,
                },
            );
        }
        self.background_images(cell, None, clip);
    }

    /// Borde propio de una celda (modelo separado).
    fn cell_border(&mut self, cell: &Fragment, clip: &ClipChain) {
        let s = &cell.style;
        let widths = cell.border;
        if widths.top + widths.right + widths.bottom + widths.left <= 0.0 {
            return;
        }
        let current = s.color.rgba();
        self.emit(
            clip,
            DisplayItem::Border {
                rect: cell.rect,
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
                radii: border_radii(s, &cell.rect),
            },
        );
    }
}

// --- E4: sombras y outlines ---

impl Builder<'_> {
    /// Las sombras de `box-shadow` de `f` exteriores (`inset = false`) o interiores, de la
    /// última a la primera para que la primera quede arriba (CSS Backgrounds 3 §7.1).
    fn box_shadows(&mut self, f: &Fragment, clip: &ClipChain, inset: bool) {
        // Las filas, grupos y celdas de una tabla no pasan por `decorations`: sus sombras
        // exteriores se pintan con las de la tabla, antes de los fondos.
        if !inset && matches!(f.table, Some(TableRole::Table(_))) {
            self.table_part_shadows(f, clip);
        }
        let s = &f.style;
        if s.box_shadow.0.is_empty() {
            return;
        }
        // `box-decoration-break: slice` (CSS Fragmentation 3 §5.4): la sombra de una caja
        // inline partida es la de la caja entera, cortada en los lados partidos.
        let (full, cut) = self.sliced_box(f);
        let mut clip = clip.clone();
        if let Some((l, r)) = cut {
            clip.push(Clip {
                rect: Rect::new(l, -1e6, r - l, 2e6),
                radii: Radii::default(),
            });
        }
        let border_box = border_radii(s, &full);
        // Las exteriores se recortan fuera de la border box; las interiores, dentro de la
        // padding box (§7.2).
        let (rect, radii) = if inset {
            let b = &f.border;
            let padding = Rect::new(
                full.x + b.left,
                full.y + b.top,
                (full.width - b.left - b.right).max(0.0),
                (full.height - b.top - b.bottom).max(0.0),
            );
            (padding, border_box.shrink(b))
        } else {
            (full, border_box)
        };
        let current = s.color.rgba();
        for sh in s.box_shadow.0.iter().rev().filter(|sh| sh.inset == inset) {
            let (x, y, blur, spread) = sh.px();
            let color = sh.color.resolve(current);
            if color.a <= 0.0 {
                continue;
            }
            self.emit(
                &clip,
                DisplayItem::BoxShadow {
                    rect,
                    radii,
                    shadow: crate::display_list::BoxShadowParams {
                        offset: (x, y),
                        blur,
                        spread,
                        color,
                        inset,
                    },
                },
            );
        }
    }

    /// Emite una capa de fondo de `f`; con `background-clip: border-area` o `text` (CSS
    /// Backgrounds 4 §2.1) la recorta al área que pinta el borde o a los glifos del texto de
    /// `f` y sus descendientes.
    fn emit_background(
        &mut self,
        f: &Fragment,
        clip: &ClipChain,
        clip_box: BackgroundBox,
        item: DisplayItem,
    ) {
        let wrapper = match clip_box {
            BackgroundBox::BorderArea => {
                let s = &f.style;
                DisplayItem::PushClipBorder {
                    rect: f.rect,
                    widths: self.sliced_widths(f, f.border),
                    styles: [
                        s.border_top_style,
                        s.border_right_style,
                        s.border_bottom_style,
                        s.border_left_style,
                    ],
                    radii: self.sliced_radii(f, border_radii(s, &f.rect)),
                }
            }
            BackgroundBox::Text => {
                let mut glyphs = Vec::new();
                if f.anonymous && f.children.is_empty() {
                    // Fondo de una parte de tabla (la pinta la tabla con una caja sin hijos): el
                    // texto que cae dentro de su rectángulo.
                    if let Some(root) = self.root {
                        self.glyph_items_within(root, &f.rect, &mut glyphs);
                    }
                } else {
                    self.glyph_items(f, &mut glyphs);
                }
                DisplayItem::PushClipText { glyphs }
            }
            _ => return self.emit(clip, item),
        };
        self.set_clip(clip);
        self.list.push(wrapper);
        self.list.push(item);
        self.list.push(DisplayItem::PopClip);
    }

    /// Los glifos de los textos bajo `f` cuyo centro cae dentro de `area`.
    fn glyph_items_within(&self, f: &Fragment, area: &Rect, out: &mut Vec<DisplayItem>) {
        if f.kind == FragmentKind::Text {
            let (cx, cy) = (
                f.rect.x + f.rect.width / 2.0,
                f.rect.y + f.rect.height / 2.0,
            );
            if cx >= area.x && cx < area.right() && cy >= area.y && cy < area.bottom() {
                self.glyph_items(f, out);
            }
            return;
        }
        for c in &f.children {
            self.glyph_items_within(c, area, out);
        }
    }

    /// Los glifos (ítems `Text`, sin color) del texto visible de `f` y sus descendientes.
    fn glyph_items(&self, f: &Fragment, out: &mut Vec<DisplayItem>) {
        for c in &f.children {
            self.glyph_items(c, out);
        }
        let Some((_, glyphs)) = &f.text else {
            return;
        };
        if f.kind != FragmentKind::Text || !visible(&f.style) || glyphs.is_empty() {
            return;
        }
        let metrics = self.fonts.metrics(&font_request(&f.style));
        let baseline = f.baseline.unwrap_or(f.rect.y + metrics.ascent);
        let size = f.style.font_size_px();
        let mut pen = f.rect.x;
        let mut runs: Vec<(bf_layout::FontKey, Vec<GlyphInstance>)> = Vec::new();
        for g in glyphs {
            if runs.last().is_none_or(|(font, _)| *font != g.font) {
                runs.push((g.font, Vec::new()));
            }
            if let Some((_, run)) = runs.last_mut() {
                run.push(GlyphInstance {
                    id: g.id,
                    x: pen + g.x_offset,
                    y: -g.y_offset,
                    outline: self.fonts.outline(g.font, g.id),
                });
            }
            pen += g.advance;
        }
        // El color del texto decide cómo recorta (ver `push_clip_text` del rasterizador).
        let color = f.style.color.rgba();
        for (font, glyphs) in runs {
            out.push(DisplayItem::Text {
                glyphs,
                font,
                size,
                color,
                origin: (0.0, baseline),
                y_fit: self.y_fit(font, size),
            });
        }
    }

    /// ¿Está partida la caja inline de `f` antes (izquierda) y después (derecha) de `f`?
    fn split_sides(&self, f: &Fragment) -> (bool, bool) {
        let (_, cut) = self.sliced_box(f);
        match cut {
            Some((l, r)) => (l > -1e6, r < 1e6),
            None => (false, false),
        }
    }

    /// `box-decoration-break: slice` (CSS Fragmentation 3 §5.4): las esquinas de los lados
    /// partidos de un fragmento inline no se redondean.
    fn sliced_radii(&self, f: &Fragment, radii: Radii) -> Radii {
        let (left, right) = self.split_sides(f);
        if !left && !right {
            return radii;
        }
        // Se anulan antes de ajustar los radios al tamaño (§5.5), como si el lado partido no
        // tuviera esquinas.
        let s = &f.style;
        let r = |c: &bf_style::properties::types::CornerRadius| {
            (c.0.resolve(f.rect.width), c.1.resolve(f.rect.height))
        };
        let mut radii = Radii([
            r(&s.border_top_left_radius),
            r(&s.border_top_right_radius),
            r(&s.border_bottom_right_radius),
            r(&s.border_bottom_left_radius),
        ]);
        if left {
            radii.0[0] = (0.0, 0.0);
            radii.0[3] = (0.0, 0.0);
        }
        if right {
            radii.0[1] = (0.0, 0.0);
            radii.0[2] = (0.0, 0.0);
        }
        radii.fit(&f.rect)
    }

    /// Y sus lados partidos no tienen borde.
    fn sliced_widths(&self, f: &Fragment, mut widths: Edges) -> Edges {
        let (left, right) = self.split_sides(f);
        if left {
            widths.left = 0.0;
        }
        if right {
            widths.right = 0.0;
        }
        widths
    }

    /// Sombras exteriores de los grupos, filas y celdas visibles de una tabla.
    fn table_part_shadows(&mut self, f: &Fragment, clip: &ClipChain) {
        for c in &f.children {
            if matches!(c.table, Some(TableRole::Part | TableRole::Cell(_))) {
                if visible(&c.style) && !c.style.box_shadow.0.is_empty() {
                    let mut part = Fragment::new(FragmentKind::Block, c.rect, c.style.clone());
                    part.border = c.border;
                    part.padding = c.padding;
                    self.box_shadows(&part, clip, false);
                }
                self.table_part_shadows(c, clip);
            }
        }
    }

    /// Indexa los fragmentos de las cajas inline partidas en varias líneas.
    fn index_inline_parts(&mut self, root: &Fragment) {
        fn walk(f: &Fragment, out: &mut HashMap<(NodeId, Option<PseudoElement>), Vec<Rect>>) {
            if f.kind == FragmentKind::Inline
                && let Some(n) = f.node
            {
                out.entry((n, f.pseudo)).or_default().push(f.rect);
            }
            for c in &f.children {
                walk(c, out);
            }
        }
        walk(root, &mut self.inline_parts);
        self.inline_parts.retain(|_, v| v.len() > 1);
    }

    /// La border box de la caja entera de la que `f` es un fragmento (los fragmentos puestos uno
    /// tras otro) y, si está partida, el tramo horizontal `[l, r)` que corresponde a `f`.
    fn sliced_box(&self, f: &Fragment) -> (Rect, Option<(f32, f32)>) {
        let parts = match f.node {
            Some(n) if f.kind == FragmentKind::Inline => self.inline_parts.get(&(n, f.pseudo)),
            _ => None,
        };
        let Some(parts) = parts else {
            return (f.rect, None);
        };
        let Some(i) = parts.iter().position(|r| *r == f.rect) else {
            return (f.rect, None);
        };
        let before: f32 = parts[..i].iter().map(|r| r.width).sum();
        let after: f32 = parts[i + 1..].iter().map(|r| r.width).sum();
        let full = Rect::new(
            f.rect.x - before,
            f.rect.y,
            f.rect.width + before + after,
            f.rect.height,
        );
        let l = if i > 0 { f.rect.x } else { -1e6 };
        let r = if i + 1 < parts.len() {
            f.rect.right()
        } else {
            1e6
        };
        (full, Some((l, r)))
    }

    /// Registra el outline de `f` (CSS UI 4 §5): fuera del borde desplazado `outline-offset`,
    /// con `outline-width` de ancho, el estilo de un borde y las esquinas de `border-radius`
    /// agrandadas. Se pinta al terminar el stacking context (CSS 2.2 Apéndice E, paso 10).
    fn collect_outline(&mut self, f: &Fragment, clip: &ClipChain) {
        let s = &f.style;
        // Con `auto` el UA puede ignorar `outline-width` (§5.3): se pinta siempre con el ancho
        // `medium`. Si no, el ancho se ajusta como el de un borde: entero hacia abajo, pero al
        // menos 1px si es positivo (CSS Values 4 §6.1.1, "snap as a border width").
        let auto = s.outline_style == bf_style::properties::types::OutlineStyle::Auto;
        let width = match s.outline_width.px() {
            _ if auto => 3.0,
            w if w > 0.0 && w < 1.0 => 1.0,
            w => w.floor(),
        };
        if !s.outline_style.is_visible() || width <= 0.0 {
            return;
        }
        let color = s.outline_color.0.resolve(s.color.rgba());
        if color.a <= 0.0 {
            return;
        }
        let offset = s.outline_offset.px();
        let d = offset + width;
        // Con `auto` el anillo rodea también el contenido que desborda la caja (como el anillo
        // de foco de los navegadores, §5.3).
        let border_box = if auto { overflow_union(f) } else { f.rect };
        // El borde interior del outline es la border box desplazada `outline-offset`; si un
        // offset negativo la vacía, queda reducida a su centro y el outline es un rectángulo
        // lleno de `outline-width` alrededor.
        let inner_w = (border_box.width + 2.0 * offset).max(0.0);
        let inner_h = (border_box.height + 2.0 * offset).max(0.0);
        let rect = Rect::new(
            border_box.x + (border_box.width - inner_w) / 2.0 - width,
            border_box.y + (border_box.height - inner_h) / 2.0 - width,
            inner_w + 2.0 * width,
            inner_h + 2.0 * width,
        );
        let mut radii = border_radii(s, &f.rect);
        for r in &mut radii.0 {
            if r.0 > 0.0 && r.1 > 0.0 {
                *r = ((r.0 + d).max(0.0), (r.1 + d).max(0.0));
            } else {
                *r = (0.0, 0.0);
            }
        }
        let style = s.outline_style.border_style();
        let item = DisplayItem::Border {
            rect,
            widths: Edges {
                top: width,
                right: width,
                bottom: width,
                left: width,
            },
            colors: [color; 4],
            styles: [style; 4],
            radii: radii.fit(&rect),
        };
        self.outlines.push((clip.clone(), item));
    }

    /// Emite los outlines recogidos en el stacking context que termina y restaura los del
    /// stacking context padre.
    fn flush_outlines(&mut self, outer: Vec<(ClipChain, DisplayItem)>) {
        let own = std::mem::replace(&mut self.outlines, outer);
        for (clip, item) in own {
            self.emit(&clip, item);
        }
    }

    /// `text-shadow` (CSS Text Decoration 3 §4): los ítems del texto `f` emitidos desde `start`
    /// (glifos y decoraciones) se repiten debajo, desplazados, en el color de cada sombra y
    /// desenfocados; la primera sombra queda arriba.
    fn text_shadows(&mut self, f: &Fragment, start: usize) {
        let shadows = &f.style.text_shadow.0;
        if shadows.is_empty() || self.list.items.len() <= start {
            return;
        }
        let items = self.list.items.split_off(start);
        let current = f.style.color.rgba();
        let pad = f.style.font_size_px().max(0.0);
        for sh in shadows.iter().rev() {
            let (dx, dy, blur, _) = sh.px();
            let color = sh.color.resolve(current);
            if color.a <= 0.0 {
                continue;
            }
            // Una sombra desenfocada se pinta en un grupo aparte que se desenfoca entero.
            let group = blur > 0.0;
            if group {
                let bounds = Rect::new(
                    f.rect.x - pad + dx,
                    f.rect.y - pad + dy,
                    f.rect.width + 2.0 * pad,
                    f.rect.height + 2.0 * pad,
                );
                self.list.push(DisplayItem::PushShadow { bounds, blur });
            }
            self.list.push(DisplayItem::PushTransform { dx, dy });
            for item in &items {
                let mut item = item.clone();
                if let DisplayItem::Text { color: c, .. } | DisplayItem::Line { color: c, .. } =
                    &mut item
                {
                    *c = color;
                }
                self.list.push(item);
            }
            self.list.push(DisplayItem::PopTransform);
            if group {
                self.list.push(DisplayItem::PopShadow);
            }
        }
        self.list.items.extend(items);
    }
}

/// Evita que el fondo asome por el antialias del borde exterior redondeado: si el borde es
/// sólido, opaco y de al menos 1px en los cuatro lados, el fondo que llega hasta la border box se
/// pinta 1px hacia dentro (lo cubre el borde), como el "bleed avoidance" de los navegadores.
fn bleed_avoidance(f: &Fragment, area: Rect, radii: Radii) -> (Rect, Radii) {
    let s = &f.style;
    let b = &f.border;
    if area != f.rect || radii.is_zero() || b.top.min(b.right).min(b.bottom).min(b.left) < 1.0 {
        return (area, radii);
    }
    let current = s.color.rgba();
    let opaque_solid = [
        (s.border_top_style, s.border_top_color.resolve(current)),
        (s.border_right_style, s.border_right_color.resolve(current)),
        (
            s.border_bottom_style,
            s.border_bottom_color.resolve(current),
        ),
        (s.border_left_style, s.border_left_color.resolve(current)),
    ]
    .iter()
    .all(|(st, c)| *st == BorderStyle::Solid && c.a >= 1.0);
    if !opaque_solid {
        return (area, radii);
    }
    let one = Edges {
        top: 1.0,
        right: 1.0,
        bottom: 1.0,
        left: 1.0,
    };
    let inner = Rect::new(
        area.x + 1.0,
        area.y + 1.0,
        area.width - 2.0,
        area.height - 2.0,
    );
    (inner, radii.shrink(&one))
}

/// La border box de `f` unida a las de sus descendientes en flujo (sin los posicionados ni lo
/// recortado por `overflow`).
fn overflow_union(f: &Fragment) -> Rect {
    extent_with_descendants(f, false)
}

/// Como [`overflow_union`]; con `through_clip` incluye los descendientes aunque `f` recorte su
/// contenido (el área de desborde desplazable de un contenedor con scroll).
fn extent_with_descendants(f: &Fragment, through_clip: bool) -> Rect {
    fn walk(f: &Fragment, r: &mut (f32, f32, f32, f32)) {
        for c in &f.children {
            if positioned(c) || c.rect.width <= 0.0 || c.rect.height <= 0.0 {
                continue;
            }
            r.0 = r.0.min(c.rect.x);
            r.1 = r.1.min(c.rect.y);
            r.2 = r.2.max(c.rect.right());
            r.3 = r.3.max(c.rect.bottom());
            if c.clip.is_none() {
                walk(c, r);
            }
        }
    }
    let mut r = (f.rect.x, f.rect.y, f.rect.right(), f.rect.bottom());
    if f.clip.is_none() || through_clip {
        walk(f, &mut r);
    }
    Rect::new(r.0, r.1, r.2 - r.0, r.3 - r.1)
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
        outlines: Vec::new(),
        inline_parts: HashMap::new(),
        viewport: Rect::new(0.0, 0.0, tree.viewport.width, tree.viewport.height),
        root: tree.root.as_ref(),
        viewport_overflow: None,
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
    builder.index_inline_parts(root); // E4
    // E4: CSS Overflow 3 §3.3: el `overflow` de la raíz (o, si es `visible` y la raíz es `html`,
    // el del primer `body`) se aplica al viewport.
    let visible_overflow = |s: &ComputedStyle| {
        s.overflow_x == bf_style::properties::types::Overflow::Visible
            && s.overflow_y == bf_style::properties::types::Overflow::Visible
    };
    builder.viewport_overflow = if !visible_overflow(&root.style) || !root_is_html {
        Some(root)
    } else {
        body
    };
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
