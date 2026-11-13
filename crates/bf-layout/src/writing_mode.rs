//! E9: modos de escritura verticales (CSS Writing Modes 4).
//!
//! El layout trabaja siempre en un marco horizontal (x = eje inline, y = eje de bloque). Un
//! flujo vertical se maqueta en su marco lógico: los estilos físicos de sus cajas se pasan a
//! ese marco ([`to_logical`]: `width` ↔ `height`, los lados según §6.4 "Abstract-to-Physical
//! Mappings"), se maqueta como si fuera `horizontal-tb` y los fragmentos resultantes se
//! devuelven a coordenadas físicas ([`fragment_to_physical`]).
//!
//! - `vertical-rl`: bloque de derecha a izquierda; el lado inline-start (con `ltr`) es arriba.
//! - `vertical-lr`: bloque de izquierda a derecha; mismo eje inline.
//!
//! `sideways-rl`/`sideways-lr` se tratan como `vertical-rl`/`vertical-lr` (falta el giro de
//! todo el texto y, en `sideways-lr`, el sentido inline de abajo arriba).
//!
//! `direction` no necesita nada aquí: en el marco lógico, `rtl` pone el inline-start a la
//! derecha, que en físico es abajo.

use std::sync::Arc;

use bf_style::properties::ComputedStyle;
use bf_style::properties::types::WritingMode;

use crate::block::{Cb, CollapsedMargin, Layout, margin_value};
use crate::box_tree::{BoxKind, LayoutBox};
use crate::float::FloatContext;
use crate::fragment::{Edges, Fragment, Rect, Size};

/// Marco de un flujo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    Horizontal,
    /// Bloque de derecha a izquierda.
    VerticalRl,
    /// Bloque de izquierda a derecha.
    VerticalLr,
}

impl Frame {
    pub fn of(style: &ComputedStyle) -> Frame {
        match style.writing_mode {
            WritingMode::HorizontalTb => Frame::Horizontal,
            WritingMode::VerticalRl | WritingMode::SidewaysRl => Frame::VerticalRl,
            WritingMode::VerticalLr | WritingMode::SidewaysLr => Frame::VerticalLr,
        }
    }

    pub fn is_vertical(self) -> bool {
        self != Frame::Horizontal
    }

    /// Lado físico (0 arriba, 1 derecha, 2 abajo, 3 izquierda) que hace de cada lado lógico
    /// del marco (en el mismo orden: block-start, inline-end con `ltr`, block-end,
    /// inline-start con `ltr`).
    fn sides(self) -> [usize; 4] {
        match self {
            Frame::Horizontal => [0, 1, 2, 3],
            Frame::VerticalRl => [1, 2, 3, 0],
            Frame::VerticalLr => [3, 2, 1, 0],
        }
    }
}

/// Permuta cuatro valores por lado: `logical[i] = physical[map[i]]` (o al revés con
/// `inverse`).
fn permute<T: Clone>(v: [T; 4], map: [usize; 4], inverse: bool) -> [T; 4] {
    let mut out = v.clone();
    for i in 0..4 {
        if inverse {
            out[map[i]] = v[i].clone();
        } else {
            out[i] = v[map[i]].clone();
        }
    }
    out
}

/// Pasa las propiedades físicas que usa el layout entre el estilo físico y el del marco
/// (`inverse`: del marco al físico).
fn convert(style: &ComputedStyle, frame: Frame, inverse: bool) -> ComputedStyle {
    let mut s = style.clone();
    if !frame.is_vertical() {
        return s;
    }
    let map = frame.sides();
    macro_rules! sides {
        ($t:ident, $r:ident, $b:ident, $l:ident) => {{
            let [t, r, b, l] = permute(
                [
                    style.$t.clone(),
                    style.$r.clone(),
                    style.$b.clone(),
                    style.$l.clone(),
                ],
                map,
                inverse,
            );
            s.$t = t;
            s.$r = r;
            s.$b = b;
            s.$l = l;
        }};
    }
    sides!(margin_top, margin_right, margin_bottom, margin_left);
    sides!(padding_top, padding_right, padding_bottom, padding_left);
    sides!(
        border_top_width,
        border_right_width,
        border_bottom_width,
        border_left_width
    );
    sides!(
        border_top_style,
        border_right_style,
        border_bottom_style,
        border_left_style
    );
    sides!(
        border_top_color,
        border_right_color,
        border_bottom_color,
        border_left_color
    );
    sides!(top, right, bottom, left);
    std::mem::swap(&mut s.width, &mut s.height);
    std::mem::swap(&mut s.min_width, &mut s.min_height);
    std::mem::swap(&mut s.max_width, &mut s.max_height);
    std::mem::swap(&mut s.overflow_x, &mut s.overflow_y);
    if let Some(r) = s.aspect_ratio.ratio.as_mut()
        && *r > 0.0
    {
        *r = 1.0 / *r;
    }
    s
}

/// Estilo físico → estilo en el marco `frame`.
pub fn to_logical(style: &ComputedStyle, frame: Frame) -> ComputedStyle {
    convert(style, frame, false)
}

/// Estilo en el marco `frame` → estilo físico.
pub fn to_physical(style: &ComputedStyle, frame: Frame) -> ComputedStyle {
    convert(style, frame, true)
}

/// Pasa a píxeles los porcentajes de los márgenes y paddings de `style` con `basis`.
fn resolve_box_percentages(style: &mut ComputedStyle, basis: f32) {
    use bf_style::properties::types::{LengthPercentageAuto, NonNegative};
    use bf_style::values::LengthPercentage;
    for m in [
        &mut style.margin_top,
        &mut style.margin_right,
        &mut style.margin_bottom,
        &mut style.margin_left,
    ] {
        if let LengthPercentageAuto::LengthPercentage(lp) = m
            && lp.has_percentage()
        {
            *m = LengthPercentageAuto::LengthPercentage(LengthPercentage::px(lp.resolve(basis)));
        }
    }
    for p in [
        &mut style.padding_top,
        &mut style.padding_right,
        &mut style.padding_bottom,
        &mut style.padding_left,
    ] {
        if p.0.has_percentage() {
            *p = NonNegative(LengthPercentage::px(p.0.resolve(basis).max(0.0)));
        }
    }
}

/// ¿La baseline dominante de un texto vertical con este estilo es la central? Lo es salvo
/// con `sideways-*` o `text-orientation: sideways` (CSS Inline 3 §4.1).
pub fn central_baseline(style: &ComputedStyle) -> bool {
    use bf_style::properties::types::TextOrientation;
    !matches!(
        style.writing_mode,
        WritingMode::SidewaysRl | WritingMode::SidewaysLr
    ) && style.text_orientation != TextOrientation::Sideways
}

/// E5: posición (desde el borde superior lógico, sobre una altura lógica `height`) de la baseline
/// sintetizada de una caja sin baseline propia en el marco `frame` (CSS Box Alignment 3 §9.1,
/// CSS Inline 3 §4.1): en horizontal, el borde inferior; en vertical con baseline central, el
/// centro; con la alfabética, el lado line-under, que en vertical es el izquierdo físico: el
/// inicio del bloque en `vertical-lr` y el final en `vertical-rl` (y `sideways-*`).
pub fn synthesized_baseline(frame: Frame, style: &ComputedStyle, height: f32) -> f32 {
    if !frame.is_vertical() {
        height
    } else if central_baseline(style) {
        height / 2.0
    } else if matches!(frame, Frame::VerticalLr) && style.writing_mode == WritingMode::VerticalLr {
        0.0
    } else {
        height
    }
}

/// ¿Hay alguna caja con un modo de escritura vertical?
pub fn has_vertical(b: &LayoutBox) -> bool {
    Frame::of(&b.style).is_vertical() || b.children.iter().any(has_vertical)
}

/// Copia del árbol de cajas en la que el estilo (y el tamaño natural de los reemplazados) de
/// cada caja está en el marco del flujo en el que participa: el de su caja padre (la raíz, el
/// suyo propio, que es también el del initial containing block). Las raíces de flujos
/// ortogonales (de otro modo que su padre) establecen un contexto de formato independiente
/// (CSS Writing Modes 4 §7.3).
pub fn logical_tree(root: &LayoutBox) -> LayoutBox {
    let mut out = root.clone();
    convert_box(&mut out, Frame::of(&root.style));
    out
}

fn convert_box(b: &mut LayoutBox, frame: Frame) {
    let own = Frame::of(&b.style);
    if own != frame && !matches!(b.kind, BoxKind::Replaced { .. }) {
        b.independent = true;
    }
    b.style = Arc::new(to_logical(&b.style, frame));
    for c in &mut b.children {
        convert_box(c, own);
    }
    if !frame.is_vertical() {
        return;
    }
    if let BoxKind::Replaced { natural, ratio } = &mut b.kind {
        *natural = Size {
            width: natural.height,
            height: natural.width,
        };
        if let Some(r) = ratio.as_mut()
            && *r > 0.0
        {
            *r = 1.0 / *r;
        }
    }
}

/// Rectángulo del marco → físico. `extent` es el tamaño físico en el eje de bloque (ancho)
/// del área sobre la que se refleja `vertical-rl`; `origin`, la esquina superior izquierda
/// común a los dos sistemas.
fn rect_to_physical(r: Rect, frame: Frame, origin: (f32, f32), extent: f32) -> Rect {
    let (lx, ly) = (r.x - origin.0, r.y - origin.1);
    let x = match frame {
        Frame::Horizontal => return r,
        Frame::VerticalLr => ly,
        Frame::VerticalRl => extent - ly - r.height,
    };
    Rect {
        x: origin.0 + x,
        y: origin.1 + lx,
        width: r.height,
        height: r.width,
    }
}

/// Inverso de [`rect_to_physical`].
fn rect_from_physical(r: Rect, frame: Frame, origin: (f32, f32), extent: f32) -> Rect {
    let (px, py) = (r.x - origin.0, r.y - origin.1);
    let y = match frame {
        Frame::Horizontal => return r,
        Frame::VerticalLr => px,
        Frame::VerticalRl => extent - px - r.width,
    };
    Rect {
        x: origin.0 + py,
        y: origin.1 + y,
        width: r.height,
        height: r.width,
    }
}

fn edges_from_physical(e: Edges, frame: Frame) -> Edges {
    let [top, right, bottom, left] =
        permute([e.top, e.right, e.bottom, e.left], frame.sides(), false);
    Edges {
        top,
        right,
        bottom,
        left,
    }
}

/// Inverso de [`fragment_to_physical`]: de coordenadas físicas al marco `frame`.
pub fn fragment_from_physical(f: &mut Fragment, frame: Frame, origin: (f32, f32), extent: f32) {
    if !frame.is_vertical() {
        return;
    }
    f.rect = rect_from_physical(f.rect, frame, origin, extent);
    f.clip = f.clip.map(|c| rect_from_physical(c, frame, origin, extent));
    f.legend = f
        .legend
        .map(|c| rect_from_physical(c, frame, origin, extent));
    f.margin = edges_from_physical(f.margin, frame);
    f.border = edges_from_physical(f.border, frame);
    f.padding = edges_from_physical(f.padding, frame);
    f.baseline = None;
    f.style = Arc::new(to_logical(&f.style, frame));
    for c in &mut f.children {
        fragment_from_physical(c, frame, origin, extent);
    }
}

/// Desplaza un fragmento y todo su subárbol.
fn translate(f: &mut Fragment, dx: f32, dy: f32) {
    f.rect.x += dx;
    f.rect.y += dy;
    if let Some(c) = f.clip.as_mut() {
        c.x += dx;
        c.y += dy;
    }
    if let Some(c) = f.legend.as_mut() {
        c.x += dx;
        c.y += dy;
    }
    if let Some(b) = f.baseline.as_mut() {
        *b += dy;
    }
    for c in &mut f.children {
        translate(c, dx, dy);
    }
}

impl<'a> Layout<'a> {
    /// ¿`b` es la raíz de un flujo de otro modo de escritura que el flujo en curso?
    pub(crate) fn is_orthogonal(&self, b: &LayoutBox) -> bool {
        Frame::of(&b.style) != self.frame && !matches!(b.kind, BoxKind::Replaced { .. })
    }

    /// Maqueta la raíz de un flujo ortogonal `b` en su propio marco (CSS Writing Modes 4
    /// §7.3) y devuelve su fragmento en el marco en curso, con la esquina de su margin box en
    /// el origen. `cb` es el containing block en el marco en curso.
    pub(crate) fn orthogonal_fragment(&mut self, b: &LayoutBox, cb: Cb) -> Fragment {
        self.orthogonal_fragment_with(b, cb, cb.width, (None, None))
    }

    /// E5: [`Layout::orthogonal_fragment`] para un ítem de grid con tamaños de contenido
    /// impuestos por su contenedor, en el marco en curso: `content_w` (ancho, el tamaño de
    /// bloque de `b`) y `height` (alto, su tamaño inline, si se estira). Sin `height`, el
    /// tamaño inline es fit-content como en cualquier raíz ortogonal (§7.3).
    pub(crate) fn orthogonal_item_fragment(
        &mut self,
        b: &LayoutBox,
        cb: Cb,
        content_w: f32,
        height: Option<f32>,
    ) -> Fragment {
        self.orthogonal_sized_fragment(b, cb, Some(content_w), height)
    }

    /// E5: [`Layout::orthogonal_fragment`] con el ancho y el alto de contenido (en el marco en
    /// curso) que se conozcan de antemano, como el de una absoluta con `left` y `right`.
    pub(crate) fn orthogonal_sized_fragment(
        &mut self,
        b: &LayoutBox,
        cb: Cb,
        width: Option<f32>,
        height: Option<f32>,
    ) -> Fragment {
        self.orthogonal_fragment_with(b, cb, cb.width, (height, width))
    }

    /// [`Layout::orthogonal_fragment`] con los porcentajes de márgenes y paddings de `b`
    /// resueltos contra `pct_basis`: el tamaño inline de su containing block, que está en el
    /// marco del padre (CSS Writing Modes 4 §7.2, CSS Box 4 §5), no en el suyo. `forced` son
    /// los tamaños de contenido impuestos (inline, bloque) en el marco de `b`, solo si los
    /// ejes se cruzan.
    fn orthogonal_fragment_with(
        &mut self,
        b: &LayoutBox,
        cb: Cb,
        pct_basis: f32,
        forced: (Option<f32>, Option<f32>),
    ) -> Fragment {
        let parent = self.frame;
        let own = Frame::of(&b.style);
        let mut inner = b.clone();
        let mut style = to_logical(&to_physical(&b.style, parent), own);
        resolve_box_percentages(&mut style, pct_basis);
        inner.style = Arc::new(style);
        inner.independent = true;
        // Ejes cruzados: el eje inline de `b` es el de bloque del padre.
        let crossed = parent.is_vertical() != own.is_vertical();
        let viewport = if crossed {
            Size {
                width: self.viewport.height,
                height: self.viewport.width,
            }
        } else {
            self.viewport
        };
        // Espacio inline disponible: el tamaño del containing block en ese eje si es
        // definido; si no, su máximo fijo y si no, el del initial containing block (§7.3.1).
        let (inline, block) = if crossed {
            (
                cb.height.or(self.cb_max_height).unwrap_or(viewport.width),
                Some(cb.width),
            )
        } else {
            (cb.width, cb.height)
        };
        let cb_inner = Cb {
            x: 0.0,
            y: 0.0,
            width: inline,
            height: block,
        };
        let mut sub = Layout::new(self.fonts, viewport);
        sub.frame = own;
        sub.abs_frames.push(Vec::new());
        let ml = margin_value(&inner.style.margin_left, inline).unwrap_or(0.0);
        let mr = margin_value(&inner.style.margin_right, inline).unwrap_or(0.0);
        let mt = margin_value(&inner.style.margin_top, inline).unwrap_or(0.0);
        // `width: auto` en un flujo cruzado: fit-content con ese espacio (§7.3).
        let mut avail = cb_inner;
        if crossed && inner.style.width == bf_style::properties::types::Size::Auto {
            let w = match forced.0 {
                Some(w) => w,
                None => sub.fit_width(&inner, cb_inner, ml + mr),
            };
            let st = &inner.style;
            let edges = crate::block::border_edges(st).horizontal()
                + crate::block::padding_edges(st, inline).horizontal();
            avail.width = w + edges + ml + mr;
        }
        if crossed {
            sub.forced_height = forced.1;
        }
        let mut floats = FloatContext::default();
        let (mut f, _) = sub.block_in(&inner, cb_inner, avail, mt, &mut floats, false);
        sub.forced_height = None;
        let border_box = f.rect;
        for p in sub.abs_frames.pop().unwrap_or_default() {
            let a = sub.absolute(p, border_box);
            f.children.push(a);
        }
        for p in std::mem::take(&mut sub.fixed) {
            let a = sub.absolute(p, border_box);
            f.children.push(a);
        }
        // Los márgenes especificados (`auto` = 0): el sub-layout resolvió el inline-end como
        // sobre-restringido contra todo el espacio disponible, y quien coloca esta caja (línea,
        // float, absoluta, `orthogonal_block`) usa su margin box.
        let mb = margin_value(&inner.style.margin_bottom, inline).unwrap_or(0.0);
        f.margin = Edges {
            top: mt,
            right: mr,
            bottom: mb,
            left: ml,
        };
        // Al físico (origen en la esquina de la margin box) y de ahí al marco del padre.
        let margin_block = f.margin.top + f.rect.height + f.margin.bottom;
        let margin_inline = f.margin.left + f.rect.width + f.margin.right;
        fragment_to_physical(&mut f, own, (0.0, 0.0), margin_block);
        let physical_width = if own.is_vertical() {
            margin_block
        } else {
            margin_inline
        };
        fragment_from_physical(&mut f, parent, (0.0, 0.0), physical_width);
        f
    }

    /// [`Layout::block_in`] para la raíz de un flujo ortogonal: su border box en `top`, con
    /// los márgenes del eje inline del padre resueltos como en CSS 2.2 §10.3.3.
    pub(crate) fn orthogonal_block(
        &mut self,
        b: &LayoutBox,
        cb: Cb,
        avail: Cb,
        top: f32,
    ) -> (Fragment, CollapsedMargin) {
        let mut f = self.orthogonal_fragment(b, cb);
        let style = &b.style;
        let width = f.rect.width;
        let (ml, mr) = match (
            margin_value(&style.margin_left, cb.width),
            margin_value(&style.margin_right, cb.width),
        ) {
            (None, None) => {
                let rest = avail.width - width;
                (rest / 2.0, rest / 2.0)
            }
            (None, Some(r)) => (avail.width - width - r, r),
            (Some(l), _) => (l, avail.width - width - l),
        };
        let mt = margin_value(&style.margin_top, cb.width).unwrap_or(0.0);
        let mb = margin_value(&style.margin_bottom, cb.width).unwrap_or(0.0);
        let (mut dx, mut dy) = (avail.x + ml - f.rect.x, top - f.rect.y);
        // `position: relative` en el marco del padre (el sub-layout lo aplicó en el suyo y
        // aquí se recoloca la caja).
        if style.position == bf_style::properties::types::Position::Relative {
            let (rx, ry) = self.relative_offset(style, cb);
            dx += rx;
            dy += ry;
        }
        translate(&mut f, dx, dy);
        f.margin = Edges {
            top: mt,
            right: mr,
            bottom: mb,
            left: ml,
        };
        (f, CollapsedMargin::new(mb))
    }

    /// Tamaños intrínsecos (de contenido) de la raíz de un flujo ortogonal en el eje inline
    /// del flujo en curso: los de su caja ya maquetada.
    pub(crate) fn orthogonal_intrinsic(&mut self, b: &LayoutBox) -> (f32, f32) {
        let cb = Cb {
            x: 0.0,
            y: 0.0,
            width: self.viewport.width,
            // El alto definido del padre que se mide (`pct_height_basis`) es el espacio inline
            // de `b` (§7.3); si no lo hay, el del initial containing block.
            height: self.pct_height_basis,
        };
        // Al medir, los porcentajes de márgenes y paddings (cíclicos) cuentan como cero.
        let f = self.orthogonal_fragment_with(b, cb, 0.0, (None, None));
        let w = (f.rect.width - f.border.left - f.border.right - f.padding.left - f.padding.right)
            .max(0.0);
        (w, w)
    }
}

fn edges_to_physical(e: Edges, frame: Frame) -> Edges {
    let [top, right, bottom, left] =
        permute([e.top, e.right, e.bottom, e.left], frame.sides(), true);
    Edges {
        top,
        right,
        bottom,
        left,
    }
}

/// Devuelve a coordenadas físicas un fragmento maquetado en el marco `frame` (y todo su
/// subárbol), con `origin` y `extent` como en [`rect_to_physical`].
pub fn fragment_to_physical(f: &mut Fragment, frame: Frame, origin: (f32, f32), extent: f32) {
    if !frame.is_vertical() {
        return;
    }
    f.rect = rect_to_physical(f.rect, frame, origin, extent);
    f.clip = f.clip.map(|c| rect_to_physical(c, frame, origin, extent));
    f.legend = f.legend.map(|c| rect_to_physical(c, frame, origin, extent));
    f.margin = edges_to_physical(f.margin, frame);
    f.border = edges_to_physical(f.border, frame);
    f.padding = edges_to_physical(f.padding, frame);
    // La baseline de un flujo vertical es una línea vertical: el pintor sitúa los glifos con
    // el rectángulo del texto (ver `bf-paint`).
    f.baseline = None;
    f.style = Arc::new(to_physical(&f.style, frame));
    for c in &mut f.children {
        fragment_to_physical(c, frame, origin, extent);
    }
}
