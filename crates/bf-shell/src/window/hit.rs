//! Hit testing sobre el árbol de fragmentos: qué caja está bajo el puntero (la más alta en el
//! orden de pintura), qué enlace la contiene y qué cursor corresponde.

use bf_dom::{Document, NodeId};
use bf_layout::fragment::{Fragment, FragmentKind, FragmentTree, Rect};
use bf_style::properties::transform::{apply_affine, invert_affine};
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
        local: Option<(f32, f32)>,
    ) {
        let is_box = !matches!(f.kind, FragmentKind::Line | FragmentKind::Text);
        let fixed = fixed || (is_box && f.style.position == Position::Fixed);
        // Lo fijo está en coordenadas del viewport; el resto, en las de la página. Dentro de
        // una caja transformada, `local` es el punto llevado a sus coordenadas.
        let point = local.unwrap_or(if fixed {
            self.point
        } else {
            (self.point.0 + self.scroll.0, self.point.1 + self.scroll.1)
        });
        // E4: `transform` (CSS Transforms 1 §6): el punto pasa por la inversa de la matriz; los
        // recortes de afuera se comprueban antes, en sus coordenadas. Una matriz no invertible
        // no pinta nada.
        let (point, clip, local) = match bf_paint::builder::fragment_matrix(f) {
            Some(m) => {
                if clip.is_some_and(|c| !contains(&c, point)) {
                    return;
                }
                let Some(inv) = invert_affine(m) else {
                    return;
                };
                let p = apply_affine(inv, point);
                (p, None, Some(p))
            }
            None => (point, clip, local),
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
            self.visit(c, z, positioned, fixed, child_clip, local);
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
    walk.visit(root, 0, false, false, None, None);
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

/// Lo que hay bajo el puntero, a través de los `<iframe>` (E4).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FrameHit {
    /// El camino de `<iframe>` hasta el documento bajo el puntero (vacío: la página).
    pub frame: Vec<NodeId>,
    /// El punto en el viewport de ese documento.
    pub client: (f32, f32),
    /// El nodo bajo el puntero en ese documento.
    pub node: Option<NodeId>,
}

/// El documento y el nodo bajo `(x, y)` del viewport de `page` (desplazada `scroll`): si el
/// punto cae en el content box de un `<iframe>` maquetado, se sigue en el documento del marco
/// (HTML §4.8.5: el contenido del marco es su viewport).
pub fn frame_hit(page: &bf_paint::Page, x: f32, y: f32, scroll: (f32, f32)) -> FrameHit {
    let mut hit = FrameHit {
        client: (x, y),
        ..FrameHit::default()
    };
    let (mut page, mut scroll) = (page, scroll);
    loop {
        hit.node = hit_test(&page.tree, hit.client.0, hit.client.1, scroll).and_then(|f| f.node);
        let Some(n) = hit.node else {
            return hit;
        };
        let at = (hit.client.0 + scroll.0, hit.client.1 + scroll.1);
        let Some((fp, local)) = bf_paint::FramePage::locate(&page.frames, at) else {
            return hit;
        };
        if fp.node != n {
            return hit;
        }
        hit.frame.push(n);
        hit.client = local;
        page = &fp.page;
        scroll = fp.scroll;
    }
}

/// E4: el marco más interior bajo `(x, y)` que puede desplazarse `delta`, y su nuevo
/// desplazamiento (limitado a su documento).
pub fn scrollable_frame(
    page: &bf_paint::Page,
    x: f32,
    y: f32,
    scroll: (f32, f32),
    delta: (f32, f32),
) -> Option<(Vec<NodeId>, (f32, f32))> {
    let hit = frame_hit(page, x, y, scroll);
    // Los marcos del camino, del más interior al exterior.
    let mut chain = Vec::new();
    let mut p = page;
    for n in &hit.frame {
        let fp = p.frames.iter().find(|f| f.node == *n)?;
        chain.push(fp);
        p = &fp.page;
    }
    for depth in (0..chain.len()).rev() {
        let fp = chain[depth];
        let extent = bf_paint::document_extent(&fp.page.tree);
        let max = (
            (extent.right() - fp.rect.width).max(0.0),
            (extent.bottom() - fp.rect.height).max(0.0),
        );
        let next = (
            (fp.scroll.0 + delta.0).clamp(0.0, max.0),
            (fp.scroll.1 + delta.1).clamp(0.0, max.1),
        );
        if next != fp.scroll {
            return Some((hit.frame[..=depth].to_vec(), next));
        }
    }
    None
}

/// Anota en `page` el desplazamiento del marco `path` (hasta que llegue el cuadro nuevo).
pub fn set_frame_scroll(page: &mut bf_paint::Page, path: &[NodeId], scroll: (f32, f32)) {
    let mut p = page;
    for (i, n) in path.iter().enumerate() {
        let Some(fp) = p.frames.iter_mut().find(|f| f.node == *n) else {
            return;
        };
        if i + 1 == path.len() {
            fp.scroll = scroll;
            return;
        }
        p = &mut fp.page;
    }
}
