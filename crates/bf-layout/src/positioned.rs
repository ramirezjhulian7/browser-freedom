//! Posicionamiento (CSS 2.2 §9.3, §10.3.7 y §10.6.4): desplazamiento relativo y cajas
//! `absolute`/`fixed`. `sticky` establece containing block como `relative` pero todavía no se
//! desplaza (TODO(sticky): necesita scroll containers reales).

use bf_style::ComputedStyle;
use bf_style::properties::types::{LengthPercentageAuto, Position};

use crate::block::{
    Cb, Layout, PendingAbs, border_edges, margin_value, padding_edges, specified_width,
    width_limits,
};
use crate::box_tree::{BoxKind, LayoutBox};
use crate::float::FloatContext;
use crate::fragment::{Edges, Fragment, Rect};

fn inset(v: &LengthPercentageAuto, basis: Option<f32>) -> Option<f32> {
    match v {
        LengthPercentageAuto::Auto => None,
        LengthPercentageAuto::LengthPercentage(lp) => {
            if lp.has_percentage() && basis.is_none() {
                None
            } else {
                Some(lp.resolve(basis.unwrap_or(0.0)))
            }
        }
    }
}

impl<'a> Layout<'a> {
    /// Registra una caja absoluta con su posición estática.
    pub(crate) fn add_abs(&mut self, b: &'a LayoutBox, static_x: f32, static_y: f32) {
        let pending = PendingAbs {
            b,
            static_x,
            static_y,
        };
        if b.style.position == Position::Fixed {
            self.fixed.push(pending);
        } else if let Some(frame) = self.abs_frames.last_mut() {
            frame.push(pending);
        }
    }

    /// Desplazamiento de `position: relative`.
    pub(crate) fn relative_offset(&self, style: &ComputedStyle, cb: Cb) -> (f32, f32) {
        let left = inset(&style.left, Some(cb.width));
        let right = inset(&style.right, Some(cb.width));
        let top = inset(&style.top, cb.height);
        let bottom = inset(&style.bottom, cb.height);
        let dx = match (left, right) {
            (Some(l), _) => l,
            (None, Some(r)) => -r,
            (None, None) => 0.0,
        };
        let dy = match (top, bottom) {
            (Some(t), _) => t,
            (None, Some(b)) => -b,
            (None, None) => 0.0,
        };
        (dx, dy)
    }

    /// Layout de una caja absoluta dentro del padding box `cb_rect` de su containing block.
    pub(crate) fn absolute(&mut self, p: PendingAbs<'a>, cb_rect: Rect) -> Fragment {
        let b = p.b;
        let s = &b.style;
        let cb = Cb {
            x: cb_rect.x,
            y: cb_rect.y,
            width: cb_rect.width,
            height: Some(cb_rect.height),
        };
        let border = border_edges(s);
        let padding = padding_edges(s, cb.width);
        let edges_h = border.horizontal() + padding.horizontal();
        let edges_v = border.vertical() + padding.vertical();

        // --- Horizontal (§10.3.7) ---
        let left = inset(&s.left, Some(cb.width));
        let right = inset(&s.right, Some(cb.width));
        let ml = margin_value(&s.margin_left, cb.width);
        let mr = margin_value(&s.margin_right, cb.width);
        let static_left = p.static_x - cb.x;
        let replaced = matches!(b.kind, BoxKind::Replaced { .. });
        let margins_h = ml.unwrap_or(0.0) + mr.unwrap_or(0.0);
        let width = if replaced {
            Some(self.fit_width(b, cb, 0.0))
        } else if let Some(w) = self.table_fixed_width(
            b,
            (cb.width - margins_h - left.unwrap_or(0.0) - right.unwrap_or(0.0)).max(0.0),
            cb.width,
        ) {
            // E4: el ancho de una tabla lo decide el algoritmo de tablas.
            Some(w)
        } else {
            specified_width(s, cb.width, edges_h).or_else(|| {
                self.keyword_width(b, (cb.width - margins_h - edges_h).max(0.0), cb.width)
            })
        };
        let h = Horizontal {
            cb_width: cb.width,
            left,
            right,
            ml,
            mr,
            edges_h,
            static_left,
        };
        // Ancho tentativo y, si viola max-width o min-width, de nuevo con ese valor como
        // `width` especificado (§10.3.7).
        let (wmin, wmax) = width_limits(s, cb.width, edges_h);
        let mut solved = self.solve_horizontal(b, &h, width);
        if solved.1 > wmax {
            solved = self.solve_horizontal(b, &h, Some(wmax));
        }
        if solved.1 < wmin {
            solved = self.solve_horizontal(b, &h, Some(wmin));
        }
        let (left, content_w, margin_left, margin_right) = solved;
        let x = cb.x + left + margin_left;

        // --- Vertical (§10.6.4): layout para conocer la altura del contenido ---
        let top = inset(&s.top, cb.height);
        let bottom = inset(&s.bottom, cb.height);
        let mt = margin_value(&s.margin_top, cb.width);
        let mb = margin_value(&s.margin_bottom, cb.width);
        let margins = Edges {
            top: mt.unwrap_or(0.0),
            right: margin_right,
            bottom: mb.unwrap_or(0.0),
            left: margin_left,
        };
        let mut scratch = FloatContext::default();
        // Si la altura depende de top/bottom, el containing block del contenido no tiene alto
        // definido hasta resolverla.
        let height_from_insets = matches!(s.height, bf_style::properties::types::Size::Auto)
            && top.is_some()
            && bottom.is_some()
            && !replaced;
        let (mut fragment, _) =
            self.box_contents(b, cb, x, 0.0, content_w, margins, &mut scratch, false);
        let mut border_h = fragment.rect.height;
        if height_from_insets {
            let content_h = (cb_rect.height
                - top.unwrap_or(0.0)
                - bottom.unwrap_or(0.0)
                - margins.top
                - margins.bottom
                - edges_v)
                .max(0.0);
            border_h = content_h + edges_v;
            fragment.rect.height = border_h;
        }
        let static_top = p.static_y - cb.y;
        let y_top = match (top, bottom) {
            (Some(t), _) => t + margins.top,
            (None, Some(bm)) => cb_rect.height - bm - margins.bottom - border_h,
            (None, None) => static_top + margins.top,
        };
        // `margin: auto` vertical con top, height y bottom definidos: centrado.
        let y_top = if let (Some(t), Some(bm), None, None) = (top, bottom, mt, mb) {
            let rest = cb_rect.height - t - bm - border_h;
            t + rest / 2.0
        } else {
            y_top
        };
        fragment.translate(0.0, cb.y + y_top);
        fragment
    }

    /// Resuelve `left`, `width` y los márgenes horizontales de una caja absoluta (§10.3.7),
    /// con `width` dado o `None` para `auto`. Devuelve (left, width, margin-left, margin-right).
    fn solve_horizontal(
        &mut self,
        b: &'a LayoutBox,
        h: &Horizontal,
        width: Option<f32>,
    ) -> (f32, f32, f32, f32) {
        let (mut ml, mut mr) = (h.ml.unwrap_or(0.0), h.mr.unwrap_or(0.0));
        let (mut left, right, mut width) = (h.left, h.right, width);
        if left.is_none() && right.is_none() {
            left = Some(h.static_left);
        }
        let cw = h.cb_width;
        let e = h.edges_h;
        match (left, width, right) {
            (Some(l), Some(w), Some(r)) => {
                let rest = cw - l - r - w - e;
                match (h.ml, h.mr) {
                    // Márgenes auto iguales, salvo que queden negativos (ltr: margin-left 0).
                    (None, None) if rest >= 0.0 => {
                        ml = rest / 2.0;
                        mr = rest / 2.0;
                    }
                    (None, None) => {
                        ml = 0.0;
                        mr = rest;
                    }
                    (None, Some(_)) => ml = rest - mr,
                    (Some(_), None) => mr = rest - ml,
                    // Sobre-restringido: se ignora `right` (ltr).
                    (Some(_), Some(_)) => {}
                }
            }
            (None, None, Some(r)) => {
                let avail = cw - r - ml - mr - e;
                let w = self.shrink_to_fit(b, avail.max(0.0));
                width = Some(w);
                left = Some(cw - r - w - e - ml - mr);
            }
            (Some(l), None, None) => {
                let avail = cw - l - ml - mr - e;
                width = Some(self.shrink_to_fit(b, avail.max(0.0)));
            }
            (None, Some(w), Some(r)) => {
                left = Some(cw - r - w - e - ml - mr);
            }
            (Some(l), None, Some(r)) => {
                width = Some((cw - l - r - ml - mr - e).max(0.0));
            }
            _ => {}
        }
        (
            left.unwrap_or(h.static_left),
            width.unwrap_or(0.0).max(0.0),
            ml,
            mr,
        )
    }
}

/// Datos de la caja y su containing block para resolver la horizontal de una absoluta.
struct Horizontal {
    cb_width: f32,
    left: Option<f32>,
    right: Option<f32>,
    ml: Option<f32>,
    mr: Option<f32>,
    edges_h: f32,
    static_left: f32,
}
