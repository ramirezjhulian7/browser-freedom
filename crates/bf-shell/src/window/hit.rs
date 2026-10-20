//! Hit testing sobre el árbol de fragmentos: qué caja está bajo el puntero (la más alta en el
//! orden de pintura), qué enlace la contiene y qué cursor corresponde.

use bf_dom::{Document, NodeId};
use bf_layout::fragment::{Fragment, FragmentKind, FragmentTree, Rect};
use bf_style::properties::types::{CursorKeyword, PointerEvents, Position, Visibility};
use winit::window::CursorIcon;

/// Clave de orden de pintura aproximada: `z-index` efectivo, si está dentro de una caja
/// posicionada (que se pinta después del flujo, CSS 2.2 Apéndice E) y orden de árbol.
type Key = (i32, bool, usize);

struct Walk<'a> {
    point: (f32, f32),
    scroll: (f32, f32),
    counter: usize,
    best: Option<(Key, &'a Fragment)>,
}

fn contains(r: &Rect, (x, y): (f32, f32)) -> bool {
    x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
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

impl<'a> Walk<'a> {
    fn visit(
        &mut self,
        f: &'a Fragment,
        z: i32,
        positioned: bool,
        fixed: bool,
        clip: Option<Rect>,
    ) {
        let is_box = !matches!(f.kind, FragmentKind::Line | FragmentKind::Text);
        let fixed = fixed || (is_box && f.style.position == Position::Fixed);
        // Lo fijo está en coordenadas del viewport; el resto, en las de la página.
        let point = if fixed {
            self.point
        } else {
            (self.point.0 + self.scroll.0, self.point.1 + self.scroll.1)
        };
        let own_positioned = is_box && f.style.position != Position::Static;
        let positioned = positioned || own_positioned;
        let z = if own_positioned {
            f.z_index.unwrap_or(z)
        } else {
            z
        };
        let order = self.counter;
        self.counter += 1;
        let candidate = f.kind != FragmentKind::Line
            && f.node.is_some()
            && f.style.visibility == Visibility::Visible
            && f.style.pointer_events != PointerEvents::None
            && contains(&f.rect, point)
            && clip.is_none_or(|c| contains(&c, point));
        if candidate {
            let key = (z, positioned, order);
            if self.best.as_ref().is_none_or(|(k, _)| key >= *k) {
                self.best = Some((key, f));
            }
        }
        let inner = match (f.clip, clip) {
            (Some(c), Some(outer)) => Some(intersect(c, outer)),
            (Some(c), None) => Some(c),
            (None, outer) => outer,
        };
        for c in &f.children {
            // Un fijo no lo recortan los `overflow` de sus ancestros.
            let child_clip = if c.style.position == Position::Fixed {
                None
            } else {
                inner
            };
            self.visit(c, z, positioned, fixed, child_clip);
        }
    }
}

/// El fragmento más alto bajo el punto `(x, y)` del viewport, con la página desplazada
/// `scroll`.
pub fn hit_test(tree: &FragmentTree, x: f32, y: f32, scroll: (f32, f32)) -> Option<&Fragment> {
    let root = tree.root.as_ref()?;
    let mut walk = Walk {
        point: (x, y),
        scroll,
        counter: 0,
        best: None,
    };
    walk.visit(root, 0, false, false, None);
    walk.best.map(|(_, f)| f)
}

/// El enlace (`<a>` o `<area>` con `href`, HTML §4.6.1) que contiene a `node`, y su `href`.
pub fn link_at(doc: &Document, node: NodeId) -> Option<(NodeId, String)> {
    let mut current = Some(node);
    while let Some(n) = current {
        if let Some(el) = doc.element(n)
            && (el.is_html("a") || el.is_html("area"))
            && let Some(href) = el.attr("href")
        {
            return Some((n, href.to_owned()));
        }
        current = doc.parent(n);
    }
    None
}

/// El cursor para el fragmento `f` según `cursor` (CSS UI 4 §5.1): `auto` es una mano sobre
/// los enlaces, la barra de texto sobre el texto y la flecha en el resto.
pub fn cursor_for(f: &Fragment, over_link: bool) -> CursorIcon {
    use CursorKeyword as K;
    match f.style.cursor.keyword {
        K::Auto if over_link => CursorIcon::Pointer,
        K::Auto if f.kind == FragmentKind::Text => CursorIcon::Text,
        K::Auto | K::Default | K::None => CursorIcon::Default,
        K::ContextMenu => CursorIcon::ContextMenu,
        K::Help => CursorIcon::Help,
        K::Pointer => CursorIcon::Pointer,
        K::Progress => CursorIcon::Progress,
        K::Wait => CursorIcon::Wait,
        K::Cell => CursorIcon::Cell,
        K::Crosshair => CursorIcon::Crosshair,
        K::Text => CursorIcon::Text,
        K::VerticalText => CursorIcon::VerticalText,
        K::Alias => CursorIcon::Alias,
        K::Copy => CursorIcon::Copy,
        K::Move => CursorIcon::Move,
        K::NoDrop => CursorIcon::NoDrop,
        K::NotAllowed => CursorIcon::NotAllowed,
        K::Grab => CursorIcon::Grab,
        K::Grabbing => CursorIcon::Grabbing,
        K::EResize => CursorIcon::EResize,
        K::NResize => CursorIcon::NResize,
        K::NeResize => CursorIcon::NeResize,
        K::NwResize => CursorIcon::NwResize,
        K::SResize => CursorIcon::SResize,
        K::SeResize => CursorIcon::SeResize,
        K::SwResize => CursorIcon::SwResize,
        K::WResize => CursorIcon::WResize,
        K::EwResize => CursorIcon::EwResize,
        K::NsResize => CursorIcon::NsResize,
        K::NeswResize => CursorIcon::NeswResize,
        K::NwseResize => CursorIcon::NwseResize,
        K::ColResize => CursorIcon::ColResize,
        K::RowResize => CursorIcon::RowResize,
        K::AllScroll => CursorIcon::AllScroll,
        K::ZoomIn => CursorIcon::ZoomIn,
        K::ZoomOut => CursorIcon::ZoomOut,
    }
}

/// El border box de la primera caja generada por `node` (en coordenadas de la página).
pub fn node_rect(tree: &FragmentTree, node: NodeId) -> Option<Rect> {
    fn walk(f: &Fragment, node: NodeId) -> Option<Rect> {
        if f.node == Some(node) && !matches!(f.kind, FragmentKind::Line | FragmentKind::Text) {
            return Some(f.rect);
        }
        f.children.iter().find_map(|c| walk(c, node))
    }
    walk(tree.root.as_ref()?, node)
}
