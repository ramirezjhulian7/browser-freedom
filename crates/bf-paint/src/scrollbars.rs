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

/// Un pulgar dibujado: su rectángulo, su eje y cuántos px de scroll mueve cada px que el
/// puntero lo arrastra a lo largo del carril.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thumb {
    pub rect: Rect,
    pub vertical: bool,
    pub ratio: f32,
}

/// Los pulgares de un scrollport `port` (padding box) con área `size`, posición `offset` y
/// ejes con barra `show`.
fn thumbs(
    port: Rect,
    client: Size,
    size: Size,
    offset: (f32, f32),
    show: (bool, bool),
) -> Vec<Thumb> {
    let mut out = Vec::new();
    let both = show.0 && show.1;
    let corner = if both { THICKNESS + INSET } else { 0.0 };
    let ratio = |track: f32, len: f32, client: f32, total: f32| {
        if track - len > 0.0 {
            (total - client) / (track - len)
        } else {
            0.0
        }
    };
    if show.1 {
        let track = port.height - 2.0 * INSET - corner;
        if let Some((start, len)) = thumb(track, client.height, size.height, offset.1) {
            let x = port.right() - INSET - THICKNESS;
            out.push(Thumb {
                rect: Rect::new(x, port.y + INSET + start, THICKNESS, len),
                vertical: true,
                ratio: ratio(track, len, client.height, size.height),
            });
        }
    }
    if show.0 {
        let track = port.width - 2.0 * INSET - corner;
        if let Some((start, len)) = thumb(track, client.width, size.width, offset.0) {
            let y = port.bottom() - INSET - THICKNESS;
            out.push(Thumb {
                rect: Rect::new(port.x + INSET + start, y, len, THICKNESS),
                vertical: false,
                ratio: ratio(track, len, client.width, size.width),
            });
        }
    }
    out
}

/// Las barras de un scrollport (ver [`thumbs`]).
fn bars(
    port: Rect,
    client: Size,
    size: Size,
    offset: (f32, f32),
    show: (bool, bool),
    color: Rgba,
    out: &mut Vec<DisplayItem>,
) {
    for t in thumbs(port, client, size, offset, show) {
        out.push(pill(t.rect, color));
    }
}

/// Los ejes con barra de un scroll container.
fn shown(sb: &bf_layout::scroll::ScrollBox) -> (bool, bool) {
    let (ox, oy) = sb.overflows();
    (
        (ox && sb.user.0) || sb.always.0,
        (oy && sb.user.1) || sb.always.1,
    )
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
            let show = shown(sb);
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

/// Margen alrededor del pulgar que todavía lo agarra (la barra es fina).
const GRAB: f32 = 4.0;

/// Un pulgar bajo el puntero: el scroll container (`None`: el viewport), el pulgar y la
/// posición de scroll al agarrarlo.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThumbHit {
    pub node: Option<bf_dom::NodeId>,
    pub thumb: Thumb,
    pub offset: (f32, f32),
}

fn grabs(r: Rect, (x, y): (f32, f32)) -> bool {
    x >= r.x - GRAB && x < r.right() + GRAB && y >= r.y - GRAB && y < r.bottom() + GRAB
}

/// El pulgar bajo `point` (coordenadas del viewport) con la página desplazada `scroll`: los
/// del viewport van encima; entre los de los scroll containers gana el último pintado (el más
/// interno). Respeta los recortes de los ancestros como [`scrollbars`].
pub fn thumb_at(
    tree: &FragmentTree,
    viewport: Size,
    extent: Size,
    scroll: (f32, f32),
    point: (f32, f32),
) -> Option<ThumbHit> {
    let port = Rect::new(0.0, 0.0, viewport.width, viewport.height);
    let show = (
        extent.width > viewport.width + 0.5,
        extent.height > viewport.height + 0.5,
    );
    if let Some(t) = thumbs(port, viewport, extent, scroll, show)
        .into_iter()
        .find(|t| grabs(t.rect, point))
    {
        return Some(ThumbHit {
            node: None,
            thumb: t,
            offset: scroll,
        });
    }
    struct Walk<'t> {
        tree: &'t FragmentTree,
        point: (f32, f32),
        scroll: (f32, f32),
        hit: Option<ThumbHit>,
    }
    fn visit(w: &mut Walk<'_>, f: &Fragment, clip: Option<Rect>, fixed: bool) {
        let is_box = !matches!(f.kind, FragmentKind::Line | FragmentKind::Text);
        let fixed = fixed || (is_box && f.style.position == Position::Fixed);
        let p = if fixed {
            w.point
        } else {
            (w.point.0 + w.scroll.0, w.point.1 + w.scroll.1)
        };
        let inside_clip =
            clip.is_none_or(|c| p.0 >= c.x && p.0 < c.right() && p.1 >= c.y && p.1 < c.bottom());
        if let Some((node, sb)) = f
            .node
            .filter(|_| f.pseudo.is_none() && !f.anonymous && inside_clip)
            .and_then(|n| w.tree.scroll.boxes.get(&n).map(|sb| (n, sb)))
            && let Some(t) = thumbs(
                f.padding_rect(),
                sb.client,
                sb.scroll_size,
                sb.offset,
                shown(sb),
            )
            .into_iter()
            .find(|t| grabs(t.rect, p))
        {
            w.hit = Some(ThumbHit {
                node: Some(node),
                thumb: t,
                offset: sb.offset,
            });
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
        point,
        scroll,
        hit: None,
    };
    if let Some(root) = &tree.root {
        visit(&mut w, root, None, false);
    }
    w.hit
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
    fn grabbing_the_viewport_thumb() {
        let vp = Size {
            width: 800.0,
            height: 600.0,
        };
        let doc = Size {
            width: 800.0,
            height: 2400.0,
        };
        let tree = FragmentTree {
            root: None,
            viewport: vp,
            report: Default::default(),
            scroll: Default::default(),
        };
        // Carril de 596 px y pulgar de 149: cada px arrastrado mueve 1800 / 447 px.
        let hit = thumb_at(&tree, vp, doc, (0.0, 0.0), (795.0, 20.0)).expect("pulgar");
        assert_eq!(hit.node, None);
        assert!(hit.thumb.vertical);
        assert!((hit.thumb.ratio - 1800.0 / 447.0).abs() < 1e-3);
        // Fuera del pulgar (más abajo) no agarra nada.
        assert_eq!(thumb_at(&tree, vp, doc, (0.0, 0.0), (795.0, 400.0)), None);
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
