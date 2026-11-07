//! Barras de desplazamiento de Freedom (E12): finas, superpuestas al contenido (no ocupan
//! espacio en el layout: son "overlay scrollbars" en el sentido de CSS Overflow 4 §6.1, por eso
//! `scrollbar-gutter` no reserva nada) y con el color que pase la interfaz.
//!
//! No forman parte de la lista de visualización del documento (los reftests no las ven): el
//! shell las pinta como una capa encima de la página con [`scrollbars`] y
//! [`viewport_scrollbars`].

use bf_layout::fragment::{Fragment, FragmentKind, FragmentTree, Rect, Size};
use bf_style::properties::types::Position;
use bf_style::values::color::Rgba;

use crate::display_list::{DisplayItem, DisplayList, Radii};

/// Grosor de la barra (px CSS).
pub const THICKNESS: f32 = 6.0;
/// Separación entre la barra y el borde del scrollport.
const INSET: f32 = 2.0;
/// Largo mínimo del pulgar.
const MIN_THUMB: f32 = 18.0;

/// El pulgar de una barra: `track` es el largo del carril, `client` el del scrollport,
/// `total` el del área desplazable y `offset` la posición. Devuelve (inicio, largo) relativos
/// al carril.
pub fn thumb(track: f32, client: f32, total: f32, offset: f32) -> Option<(f32, f32)> {
    if total <= client + 0.5 || track <= 0.0 {
        return None;
    }
    let len = (track * client / total).clamp(MIN_THUMB.min(track), track);
    let max = total - client;
    let start = (track - len) * (offset / max).clamp(0.0, 1.0);
    Some((start, len))
}

fn pill(rect: Rect, color: Rgba) -> DisplayItem {
    let r = rect.width.min(rect.height) / 2.0;
    DisplayItem::Rect {
        rect,
        color,
        radii: Radii([(r, r); 4]),
    }
}

/// Las barras de un scrollport `port` (padding box) con área `size`, posición `offset` y
/// ejes con barra `show`.
fn bars(
    port: Rect,
    client: Size,
    size: Size,
    offset: (f32, f32),
    show: (bool, bool),
    color: Rgba,
    out: &mut Vec<DisplayItem>,
) {
    let both = show.0 && show.1;
    let corner = if both { THICKNESS + INSET } else { 0.0 };
    if show.1 {
        let track = port.height - 2.0 * INSET - corner;
        if let Some((start, len)) = thumb(track, client.height, size.height, offset.1) {
            let x = port.right() - INSET - THICKNESS;
            out.push(pill(
                Rect::new(x, port.y + INSET + start, THICKNESS, len),
                color,
            ));
        }
    }
    if show.0 {
        let track = port.width - 2.0 * INSET - corner;
        if let Some((start, len)) = thumb(track, client.width, size.width, offset.0) {
            let y = port.bottom() - INSET - THICKNESS;
            out.push(pill(
                Rect::new(port.x + INSET + start, y, len, THICKNESS),
                color,
            ));
        }
    }
}

fn intersect(a: Rect, b: Rect) -> Rect {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    Rect::new(
        x,
        y,
        (a.right().min(b.right()) - x).max(0.0),
        (a.bottom().min(b.bottom()) - y).max(0.0),
    )
}

/// Las barras de los scroll containers del árbol que desbordan (o tienen `overflow: scroll`),
/// en coordenadas de página; las de cajas fijas van entre `PushFixed`/`PopFixed`. Cada barra
/// se recorta con los `overflow` de sus ancestros.
pub fn scrollbars(tree: &FragmentTree, color: Rgba) -> DisplayList {
    struct Walk<'t> {
        tree: &'t FragmentTree,
        color: Rgba,
        page: Vec<DisplayItem>,
        fixed: Vec<DisplayItem>,
    }
    fn visit(w: &mut Walk<'_>, f: &Fragment, clip: Option<Rect>, fixed: bool) {
        let is_box = !matches!(f.kind, FragmentKind::Line | FragmentKind::Text);
        let fixed = fixed || (is_box && f.style.position == Position::Fixed);
        if let Some(sb) = f
            .node
            .filter(|_| f.pseudo.is_none() && !f.anonymous)
            .and_then(|n| w.tree.scroll.boxes.get(&n))
        {
            let (ox, oy) = sb.overflows();
            let show = (
                (ox && sb.user.0) || sb.always.0,
                (oy && sb.user.1) || sb.always.1,
            );
            let mut items = Vec::new();
            bars(
                f.padding_rect(),
                sb.client,
                sb.scroll_size,
                sb.offset,
                show,
                w.color,
                &mut items,
            );
            let out = if fixed { &mut w.fixed } else { &mut w.page };
            for item in items {
                if let (DisplayItem::Rect { rect, .. }, Some(c)) = (&item, clip) {
                    let visible = intersect(*rect, c);
                    if visible.width <= 0.0 || visible.height <= 0.0 {
                        continue;
                    }
                    out.push(DisplayItem::PushClip {
                        rect: c,
                        radii: Radii::default(),
                    });
                    out.push(item);
                    out.push(DisplayItem::PopClip);
                } else {
                    out.push(item);
                }
            }
        }
        let inner = match (f.clip, clip) {
            (Some(c), Some(outer)) => Some(intersect(c, outer)),
            (Some(c), None) => Some(c),
            (None, outer) => outer,
        };
        for c in &f.children {
            let child_clip = if c.style.position == Position::Fixed {
                None
            } else {
                inner
            };
            visit(w, c, child_clip, fixed);
        }
    }
    let mut w = Walk {
        tree,
        color,
        page: Vec::new(),
        fixed: Vec::new(),
    };
    if let Some(root) = &tree.root {
        visit(&mut w, root, None, false);
    }
    let mut list = DisplayList::default();
    for item in w.page {
        list.push(item);
    }
    if !w.fixed.is_empty() {
        list.push(DisplayItem::PushFixed);
        for item in w.fixed {
            list.push(item);
        }
        list.push(DisplayItem::PopFixed);
    }
    list
}

/// Las barras del viewport (coordenadas del viewport): `extent` es el tamaño del documento.
pub fn viewport_scrollbars(
    viewport: Size,
    extent: Size,
    scroll: (f32, f32),
    color: Rgba,
) -> DisplayList {
    let mut items = Vec::new();
    let show = (
        extent.width > viewport.width + 0.5,
        extent.height > viewport.height + 0.5,
    );
    bars(
        Rect::new(0.0, 0.0, viewport.width, viewport.height),
        viewport,
        extent,
        scroll,
        show,
        color,
        &mut items,
    );
    let mut list = DisplayList::default();
    for item in items {
        list.push(item);
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_size_and_position() {
        // Mitad visible: pulgar de medio carril, al principio y al final.
        assert_eq!(thumb(100.0, 50.0, 100.0, 0.0), Some((0.0, 50.0)));
        assert_eq!(thumb(100.0, 50.0, 100.0, 50.0), Some((50.0, 50.0)));
        // Sin desborde no hay pulgar.
        assert_eq!(thumb(100.0, 100.0, 100.0, 0.0), None);
        // Largo mínimo.
        assert_eq!(
            thumb(100.0, 1.0, 10_000.0, 0.0).map(|t| t.1),
            Some(MIN_THUMB)
        );
    }

    #[test]
    fn viewport_bar_is_at_the_right_edge() {
        let vp = Size {
            width: 800.0,
            height: 600.0,
        };
        let list = viewport_scrollbars(
            vp,
            Size {
                width: 800.0,
                height: 1200.0,
            },
            (0.0, 600.0),
            Rgba::BLACK,
        );
        let rects: Vec<Rect> = list
            .items
            .iter()
            .filter_map(|i| match i {
                DisplayItem::Rect { rect, .. } => Some(*rect),
                _ => None,
            })
            .collect();
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].right(), 800.0 - INSET);
        assert_eq!(rects[0].bottom(), 600.0 - INSET);
    }
}
