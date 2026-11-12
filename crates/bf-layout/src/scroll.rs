//! Scroll containers (CSS Overflow 3 §3) y `position: sticky` (CSS Position 3 §3.4).
//!
//! El layout produce el árbol sin desplazar; este módulo, después:
//! - reconoce los scroll containers y mide su área de desborde desplazable (CSS Overflow 3
//!   §2.2, "scrollable overflow rectangle"),
//! - aplica el desplazamiento de cada uno trasladando su contenido (sus fragmentos hijos),
//! - y desplaza las cajas `sticky` contra su scrollport más cercano (§3.4 de CSS Position 3).
//!
//! Todo es incremental: [`ScrollState`] guarda el desplazamiento aplicado a cada caja y
//! cambiarlo traslada el contenido por la diferencia, sin rehacer el layout. El scroll del
//! viewport no se aplica al árbol (lo aplica la pintura), pero las cajas `sticky` cuyo
//! scrollport es el viewport sí dependen de él: [`update_sticky`] las recalcula.

use std::collections::HashMap;

use bf_dom::{Document, NodeId};
use bf_style::ComputedStyle;
use bf_style::properties::types::{LengthPercentageAuto, Overflow, Position};

use crate::fragment::{Fragment, FragmentKind, FragmentTree, Rect, Size};

/// Un scroll container (CSS Overflow 3 §3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollBox {
    /// Tamaño del scrollport (la padding box).
    pub client: Size,
    /// Tamaño del área de desborde desplazable, medida desde el origen de la padding box
    /// (nunca menor que `client`).
    pub scroll_size: Size,
    /// Desplazamiento aplicado al contenido (`scrollLeft`, `scrollTop`).
    pub offset: (f32, f32),
    /// Ejes que el usuario puede desplazar (`auto` o `scroll`; `hidden` solo por programa,
    /// §3.1).
    pub user: (bool, bool),
    /// `overflow: scroll` en cada eje: la barra se muestra aunque no haya desborde.
    pub always: (bool, bool),
}

impl ScrollBox {
    /// El desplazamiento máximo en cada eje.
    pub fn max_offset(&self) -> (f32, f32) {
        (
            (self.scroll_size.width - self.client.width).max(0.0),
            (self.scroll_size.height - self.client.height).max(0.0),
        )
    }

    /// `offset` limitado al área desplazable (CSSOM View §4.1: la posición queda entre 0 y el
    /// máximo; ltr y horizontal-tb).
    pub fn clamp(&self, (x, y): (f32, f32)) -> (f32, f32) {
        let (mx, my) = self.max_offset();
        let fix = |v: f32, max: f32| {
            if v.is_finite() {
                v.clamp(0.0, max)
            } else {
                0.0
            }
        };
        (fix(x, mx), fix(y, my))
    }

    /// ¿Hay desborde que desplazar en cada eje?
    pub fn overflows(&self) -> (bool, bool) {
        let (mx, my) = self.max_offset();
        (mx > 0.5, my > 0.5)
    }
}

/// El estado de desplazamiento de un árbol de fragmentos.
#[derive(Debug, Clone, Default)]
pub struct ScrollState {
    /// Los scroll containers del documento (salvo el viewport).
    pub boxes: HashMap<NodeId, ScrollBox>,
    /// Desplazamiento `sticky` aplicado a cada caja.
    pub sticky: HashMap<NodeId, (f32, f32)>,
    /// ¿Hay cajas `sticky` cuyo scrollport es el viewport? Entonces cambian con el scroll de
    /// la página.
    pub viewport_sticky: bool,
    /// Scroll de la página con el que se calcularon las cajas `sticky`.
    pub page_scroll: (f32, f32),
    /// E4: las restricciones de cada caja `sticky` cuyo scrollport es el viewport, para que la
    /// pintura (`DisplayItem::PushSticky` de bf-paint) resuelva su desplazamiento con el
    /// scroll de la página sin rehacer la lista.
    pub viewport_constraints: HashMap<NodeId, StickyConstraints>,
}

/// Lo que fija el desplazamiento de una caja `sticky` (CSS Position 3 §3.4), salvo el
/// rectángulo de vista: su border box sin desplazar, sus márgenes, sus `inset` resueltos y la
/// content box de su containing block (E4, extraído de `sticky_shift` de E12).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StickyConstraints {
    /// Border box de la caja sin el desplazamiento `sticky`.
    pub rect: Rect,
    /// Márgenes (los `auto` cuentan como 0).
    pub margin: crate::fragment::Edges,
    /// `top`, `right`, `bottom`, `left` resueltos contra el rectángulo de vista (`None`:
    /// `auto`).
    pub insets: [Option<f32>; 4],
    /// Content box del containing block: el límite del desplazamiento.
    pub containing: Rect,
}

impl StickyConstraints {
    /// El desplazamiento `sticky` contra el rectángulo de vista `svr` (el scrollport menos su
    /// padding). Si chocan, ganan `top` y `left` (ltr).
    pub fn offset(&self, svr: Rect) -> (f32, f32) {
        let (r, m, cb) = (self.rect, self.margin, self.containing);
        let [top, right, bottom, left] = self.insets;
        let axis = |start: Option<f32>,
                    end: Option<f32>,
                    (r0, r1): (f32, f32),
                    (m0, m1): (f32, f32),
                    (s0, s1): (f32, f32),
                    (c0, c1): (f32, f32)| {
            let mut d = 0.0f32;
            if let Some(e) = end {
                let limit = s1 - e;
                if r1 > limit {
                    // Hacia el inicio, sin que el margen salga del containing block.
                    d = (limit - r1).max((c0 - (r0 - m0)).min(0.0));
                }
            }
            if let Some(s) = start {
                let limit = s0 + s;
                if r0 + d < limit {
                    // Hacia el final, sin que el margen salga del containing block.
                    d = (limit - r0).min((c1 - (r1 + m1)).max(0.0));
                }
            }
            d
        };
        let dx = axis(
            left,
            right,
            (r.x, r.right()),
            (m.left, m.right),
            (svr.x, svr.right()),
            (cb.x, cb.right()),
        );
        let dy = axis(
            top,
            bottom,
            (r.y, r.bottom()),
            (m.top, m.bottom),
            (svr.y, svr.bottom()),
            (cb.y, cb.bottom()),
        );
        (dx, dy)
    }
}

/// El valor usado de `overflow-x`/`overflow-y` (CSS Overflow 3 §3.1: si un eje es
/// `visible`/`clip` y el otro no, `visible` se comporta como `auto` y `clip` como `hidden`).
pub fn used_overflow(style: &ComputedStyle) -> (Overflow, Overflow) {
    let (x, y) = (style.overflow_x, style.overflow_y);
    let fix = |o: Overflow, other: Overflow| {
        if other.is_scrollable() {
            match o {
                Overflow::Visible => Overflow::Auto,
                Overflow::Clip => Overflow::Hidden,
                o => o,
            }
        } else {
            o
        }
    };
    (fix(x, y), fix(y, x))
}

/// Mitad del tamaño del rectángulo de recorte en un eje que no recorta (`overflow-x: clip`
/// con `overflow-y: visible`): "sin límite" sin infinitos, que no sobreviven a las
/// traslaciones ni a la pintura.
pub const UNCLIPPED_HALF: f32 = 4.0e6;

/// El borde de recorte por `overflow` de una caja (CSS Overflow 3 §3.2, "overflow clip
/// edge"), o `None` si no recorta. `forced`: la caja recorta aunque su `overflow` sea
/// `visible` (controles de formulario con scroll interno).
///
/// - Con `overflow: clip` (o `contain: paint`, CSS Containment 2 §3.4), la caja de
///   `overflow-clip-margin` (por omisión la padding box) agrandada en su longitud (negativa:
///   encoge, como en `overflow-clip-margin-016`/`017`).
/// - En un scroll container, el borde es la padding box: `overflow-clip-margin` solo puede
///   encogerlo (`overflow-clip-margin-018`..`020`) y `border-box` se ignora (`021`/`022`).
/// - Un eje `visible` no recorta (`overflow-x: clip` solo recorta en horizontal, `clip-002`):
///   en ese eje el rectángulo se extiende [`UNCLIPPED_HALF`] a cada lado.
pub fn overflow_clip(f: &Fragment, forced: bool) -> Option<Rect> {
    use bf_style::properties::overflow::VisualBox;
    let style = &f.style;
    let (mut x, mut y) = used_overflow(style);
    if style.contain.paint {
        for o in [&mut x, &mut y] {
            if *o == Overflow::Visible {
                *o = Overflow::Clip;
            }
        }
    }
    let padding = f.padding_rect();
    if x == Overflow::Visible && y == Overflow::Visible {
        return forced.then_some(padding);
    }
    let margin = &style.overflow_clip_margin;
    let scroller = x.is_scrollable() || y.is_scrollable();
    let (visual, length) = match margin.visual_box {
        VisualBox::BorderBox if scroller => (padding, 0.0),
        VisualBox::BorderBox => (f.rect, margin.px()),
        VisualBox::PaddingBox => (padding, margin.px()),
        VisualBox::ContentBox => (f.content_rect(), margin.px()),
    };
    let length = if scroller { length.min(0.0) } else { length };
    let mut edge = Rect::new(
        visual.x - length,
        visual.y - length,
        (visual.width + 2.0 * length).max(0.0),
        (visual.height + 2.0 * length).max(0.0),
    );
    if scroller {
        // Nunca fuera de la padding box (`content-box` con una longitud positiva).
        let right = edge.right().min(padding.right());
        let bottom = edge.bottom().min(padding.bottom());
        edge.x = edge.x.max(padding.x);
        edge.y = edge.y.max(padding.y);
        edge.width = (right - edge.x).max(0.0);
        edge.height = (bottom - edge.y).max(0.0);
    }
    if x == Overflow::Visible {
        edge.x = f.rect.x - UNCLIPPED_HALF;
        edge.width = 2.0 * UNCLIPPED_HALF;
    }
    if y == Overflow::Visible {
        edge.y = f.rect.y - UNCLIPPED_HALF;
        edge.height = 2.0 * UNCLIPPED_HALF;
    }
    Some(edge)
}

/// ¿El elemento propaga su `overflow` al viewport (CSS Overflow 3 §3.3)? El raíz siempre; el
/// `body` si el raíz tiene `overflow: visible` en los dos ejes.
fn propagates_to_viewport(doc: &Document, root: &Fragment, f: &Fragment) -> bool {
    let Some(node) = f.node else {
        return false;
    };
    if Some(node) == doc.document_element() {
        return true;
    }
    let root_visible =
        root.style.overflow_x == Overflow::Visible && root.style.overflow_y == Overflow::Visible;
    root_visible
        && doc.body() == Some(node)
        && doc.element(node).is_some_and(|e| e.is_html("body"))
        && doc.parent(node) == doc.document_element()
}

/// ¿Es un control con scroll interno (`<textarea>`, list box)?
fn scrolling_control(doc: &Document, f: &Fragment) -> bool {
    let Some(el) = f.node.and_then(|n| doc.element(n)) else {
        return false;
    };
    f.clip.is_some() && (el.is_html("textarea") || (el.is_html("select") && f.widget.is_none()))
}

/// ¿Es `f` un scroll container? Devuelve los ejes desplazables por el usuario y los de
/// `overflow: scroll`.
fn container_axes(doc: &Document, f: &Fragment) -> Option<((bool, bool), (bool, bool))> {
    if f.node.is_none()
        || f.pseudo.is_some()
        || f.anonymous
        || !matches!(f.kind, FragmentKind::Block | FragmentKind::InlineBlock)
    {
        return None;
    }
    if scrolling_control(doc, f) {
        return Some(((true, true), (false, false)));
    }
    let (x, y) = used_overflow(&f.style);
    if !x.is_scrollable() && !y.is_scrollable() {
        return None;
    }
    let user = |o: Overflow| matches!(o, Overflow::Auto | Overflow::Scroll);
    Some((
        (user(x), user(y)),
        (x == Overflow::Scroll, y == Overflow::Scroll),
    ))
}

fn is_out_of_flow(f: &Fragment) -> bool {
    !matches!(f.kind, FragmentKind::Line | FragmentKind::Text)
        && matches!(f.style.position, Position::Absolute | Position::Fixed)
}

/// El área de desborde desplazable de `f` (CSS Overflow 3 §2.2): la padding box, las border
/// boxes de los descendientes (atravesando los que no recortan) y, para el contenido en
/// flujo, sus margin boxes más el padding del final (§2.2: el contenido en flujo se extiende
/// con el padding de inline-end y block-end). Devuelve (derecha, abajo) absolutos.
fn scrollable_extent(f: &Fragment) -> (f32, f32) {
    fn walk(f: &Fragment, r: &mut (f32, f32)) {
        for c in &f.children {
            if c.style.position == Position::Fixed && is_out_of_flow(c) {
                continue;
            }
            r.0 = r.0.max(c.rect.right());
            r.1 = r.1.max(c.rect.bottom());
            match c.clip {
                None => walk(c, r),
                // Lo de dentro cuenta hasta el borde de recorte (en un eje que no recorta,
                // entero: `overflow-x: clip` deja desbordar en vertical).
                Some(clip) => {
                    let mut inner = *r;
                    walk(c, &mut inner);
                    r.0 = r.0.max(inner.0.min(clip.right()));
                    r.1 = r.1.max(inner.1.min(clip.bottom()));
                }
            }
        }
    }
    let p = f.padding_rect();
    let mut r = (p.right(), p.bottom());
    walk(f, &mut r);
    // Contenido en flujo: margin box de los hijos más el padding del final.
    let mut flow = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for c in &f.children {
        if is_out_of_flow(c) {
            continue;
        }
        let m = c.margin_rect();
        flow.0 = flow.0.max(m.right());
        flow.1 = flow.1.max(m.bottom());
    }
    if flow.0.is_finite() {
        r.0 = r.0.max(flow.0 + f.padding.right);
        r.1 = r.1.max(flow.1 + f.padding.bottom);
    }
    r
}

/// Mide el área desplazable de un scroll container (su contenido sin desplazar: `offset` es el
/// desplazamiento ya aplicado a sus hijos).
fn measure(f: &Fragment, offset: (f32, f32)) -> (Size, Size) {
    let p = f.padding_rect();
    let (right, bottom) = scrollable_extent(f);
    let client = Size {
        width: p.width.max(0.0),
        height: p.height.max(0.0),
    };
    let scroll_size = Size {
        width: (right + offset.0 - p.x).max(client.width),
        height: (bottom + offset.1 - p.y).max(client.height),
    };
    (client, scroll_size)
}

/// Reconoce los scroll containers del árbol recién maquetado y prepara su estado. Quita el
/// recorte de las cajas que propagan su `overflow` al viewport (CSS Overflow 3 §3.3: lo
/// recorta el viewport, no ellas).
pub fn init(tree: &mut FragmentTree, doc: &Document) {
    let mut state = ScrollState::default();
    let Some(root) = tree.root.as_mut() else {
        tree.scroll = state;
        return;
    };
    let root_style = root.clone_shallow();
    fn walk(f: &mut Fragment, doc: &Document, root: &Fragment, state: &mut ScrollState) {
        let propagated = propagates_to_viewport(doc, root, f);
        if propagated && !scrolling_control(doc, f) {
            f.clip = None;
        } else if let (Some(node), Some((user, always))) = (f.node, container_axes(doc, f)) {
            let (client, scroll_size) = measure(f, (0.0, 0.0));
            state.boxes.insert(
                node,
                ScrollBox {
                    client,
                    scroll_size,
                    offset: (0.0, 0.0),
                    user,
                    always,
                },
            );
        }
        for c in &mut f.children {
            walk(c, doc, root, state);
        }
    }
    walk(root, doc, &root_style, &mut state);
    tree.scroll = state;
    update_sticky(tree, (0.0, 0.0));
}

impl Fragment {
    /// Copia del fragmento sin hijos.
    fn clone_shallow(&self) -> Fragment {
        let mut c = Fragment::new(self.kind, self.rect, self.style.clone());
        c.node = self.node;
        c
    }
}

/// El fragmento del elemento `node` (el primero en orden de árbol).
fn find_mut(f: &mut Fragment, node: NodeId) -> Option<&mut Fragment> {
    if f.node == Some(node) && f.pseudo.is_none() && !f.anonymous {
        return Some(f);
    }
    f.children.iter_mut().find_map(|c| find_mut(c, node))
}

/// Los elementos de los fragmentos que contienen al de `target`, del más interno al raíz
/// (incluido el propio `target`).
fn ancestor_chain(f: &Fragment, target: NodeId, path: &mut Vec<NodeId>) -> bool {
    if f.node == Some(target) && !f.anonymous {
        path.push(target);
        return true;
    }
    for c in &f.children {
        if ancestor_chain(c, target, path) {
            if let Some(n) = f.node.filter(|_| !f.anonymous && f.pseudo.is_none())
                && path.last() != Some(&n)
            {
                path.push(n);
            }
            return true;
        }
    }
    false
}

/// Desplaza el scroll container `node` a `offset` (limitado a su área desplazable) y
/// recalcula las cajas `sticky`. Devuelve el desplazamiento aplicado, o `None` si `node` no
/// es un scroll container.
pub fn set_scroll_offset(
    tree: &mut FragmentTree,
    node: NodeId,
    offset: (f32, f32),
) -> Option<(f32, f32)> {
    let sb = *tree.scroll.boxes.get(&node)?;
    let new = sb.clamp(offset);
    if new == sb.offset {
        return Some(new);
    }
    let f = tree.root.as_mut().and_then(|r| find_mut(r, node))?;
    let (dx, dy) = (sb.offset.0 - new.0, sb.offset.1 - new.1);
    for c in &mut f.children {
        // Los fijos no se desplazan con su ancestro (su containing block es el viewport).
        if c.style.position == Position::Fixed && is_out_of_flow(c) {
            continue;
        }
        c.translate(dx, dy);
    }
    if let Some(b) = tree.scroll.boxes.get_mut(&node) {
        b.offset = new;
    }
    let page = tree.scroll.page_scroll;
    update_sticky(tree, page);
    Some(new)
}

/// Aplica los desplazamientos de `offsets` (los que no son scroll containers se ignoran).
/// Devuelve los aplicados, ya limitados.
pub fn apply_offsets(
    tree: &mut FragmentTree,
    offsets: impl IntoIterator<Item = (NodeId, (f32, f32))>,
) -> Vec<(NodeId, (f32, f32))> {
    offsets
        .into_iter()
        .filter_map(|(n, o)| set_scroll_offset(tree, n, o).map(|o| (n, o)))
        .collect()
}

/// La rueda sobre el elemento `target` (el que está bajo el puntero): desplaza el scroll
/// container más interno que lo contiene y que todavía puede moverse en la dirección de
/// `delta` (px CSS; positivo = hacia abajo/derecha) en un eje desplazable por el usuario.
/// Devuelve el contenedor y su desplazamiento nuevo; `None` si le toca al viewport.
pub fn scroll_at(
    tree: &mut FragmentTree,
    target: NodeId,
    delta: (f32, f32),
) -> Option<(NodeId, (f32, f32))> {
    scroll_at_with(tree, target, |_| delta)
}

/// Como [`scroll_at`], con un desplazamiento que depende del contenedor (el teclado: AvPág
/// desplaza casi un scrollport entero, Inicio/Fin hasta el borde).
pub fn scroll_at_with(
    tree: &mut FragmentTree,
    target: NodeId,
    delta: impl Fn(&ScrollBox) -> (f32, f32),
) -> Option<(NodeId, (f32, f32))> {
    let mut path = Vec::new();
    ancestor_chain(tree.root.as_ref()?, target, &mut path);
    for node in path {
        let Some(sb) = tree.scroll.boxes.get(&node).copied() else {
            continue;
        };
        let delta = delta(&sb);
        let want = (
            if sb.user.0 {
                sb.offset.0 + delta.0
            } else {
                sb.offset.0
            },
            if sb.user.1 {
                sb.offset.1 + delta.1
            } else {
                sb.offset.1
            },
        );
        let new = sb.clamp(want);
        if (new.0 - sb.offset.0).abs() > 0.01 || (new.1 - sb.offset.1).abs() > 0.01 {
            set_scroll_offset(tree, node, new);
            return Some((node, new));
        }
    }
    None
}

/// El scroll container más cercano que contiene a `node` (sin contar el propio `node`).
pub fn scroll_parent(tree: &FragmentTree, node: NodeId) -> Option<NodeId> {
    let mut path = Vec::new();
    ancestor_chain(tree.root.as_ref()?, node, &mut path);
    path.into_iter()
        .skip(1)
        .find(|n| tree.scroll.boxes.contains_key(n))
}

/// La suma de los desplazamientos de los scroll containers que contienen a `node` (sin contar
/// el propio `node`), hasta `upto` incluido (o hasta el raíz): lo que hay que sumar a la
/// posición de su caja para obtener la de layout, sin scroll (`offsetTop`, CSSOM View §7).
pub fn scroll_offset_between(
    tree: &FragmentTree,
    node: NodeId,
    upto: Option<NodeId>,
) -> (f32, f32) {
    let mut path = Vec::new();
    if let Some(root) = tree.root.as_ref() {
        ancestor_chain(root, node, &mut path);
    }
    let mut sum = (0.0, 0.0);
    for n in path.into_iter().skip(1) {
        if let Some(sb) = tree.scroll.boxes.get(&n) {
            sum.0 += sb.offset.0;
            sum.1 += sb.offset.1;
        }
        if Some(n) == upto {
            break;
        }
    }
    sum
}

fn inset(v: &LengthPercentageAuto, basis: f32) -> Option<f32> {
    match v {
        LengthPercentageAuto::Auto => None,
        LengthPercentageAuto::LengthPercentage(lp) => Some(lp.resolve(basis)),
    }
}

/// Las restricciones `sticky` de una caja con border box `r` (sin desplazar) contra un
/// rectángulo de vista de tamaño `svr` y la content box `cb` de su containing block (CSS
/// Position 3 §3.4). E4: antes `sticky_shift`; el cálculo pasó a [`StickyConstraints::offset`].
fn sticky_constraints(style: &ComputedStyle, r: Rect, svr: Size, cb: Rect) -> StickyConstraints {
    // Los márgenes `auto` cuentan como 0 para el límite (Blink y Gecko; Gecko 1488080: una
    // caja sticky alineada con `margin-top: auto` en un flex sigue pegándose).
    let margin = |v: &LengthPercentageAuto| crate::block::margin_value(v, cb.width).unwrap_or(0.0);
    StickyConstraints {
        rect: r,
        margin: crate::fragment::Edges {
            top: margin(&style.margin_top),
            right: margin(&style.margin_right),
            bottom: margin(&style.margin_bottom),
            left: margin(&style.margin_left),
        },
        insets: [
            inset(&style.top, svr.height),
            inset(&style.right, svr.width),
            inset(&style.bottom, svr.height),
            inset(&style.left, svr.width),
        ],
        containing: cb,
    }
}

/// El rectángulo de vista sticky de un scroll container: su scrollport menos su padding
/// (como Blink y Gecko).
fn sticky_view(f: &Fragment) -> Rect {
    let p = f.padding_rect();
    Rect::new(
        p.x + f.padding.left,
        p.y + f.padding.top,
        (p.width - f.padding.horizontal()).max(0.0),
        (p.height - f.padding.vertical()).max(0.0),
    )
}

/// Recalcula las cajas `position: sticky` con el scroll de la página `page_scroll` (px CSS).
pub fn update_sticky(tree: &mut FragmentTree, page_scroll: (f32, f32)) {
    struct Ctx<'s> {
        state: &'s mut ScrollState,
        viewport: Size,
        page_scroll: (f32, f32),
        viewport_sticky: bool,
    }
    /// `svr`: el rectángulo de vista y si es el viewport desplazable; `cb`: la content box del
    /// containing block de los hijos.
    fn walk(f: &mut Fragment, ctx: &mut Ctx<'_>, svr: (Rect, bool), cb: Rect) {
        let is_box = !matches!(
            f.kind,
            FragmentKind::Line | FragmentKind::Text | FragmentKind::Inline
        );
        let mut svr = svr;
        if is_box && f.style.position == Position::Fixed {
            // Lo fijo está en coordenadas del viewport y no se mueve con el scroll.
            svr = (
                Rect::new(0.0, 0.0, ctx.viewport.width, ctx.viewport.height),
                false,
            );
        }
        if is_box
            && f.style.position == Position::Sticky
            && f.pseudo.is_none()
            && let Some(node) = f.node
        {
            let old = ctx.state.sticky.get(&node).copied().unwrap_or((0.0, 0.0));
            let mut r = f.rect;
            r.translate(-old.0, -old.1);
            let style = f.style.clone();
            let size = Size {
                width: svr.0.width,
                height: svr.0.height,
            };
            let constraints = sticky_constraints(&style, r, size, cb);
            let new = constraints.offset(svr.0);
            if new != old {
                f.translate(new.0 - old.0, new.1 - old.1);
            }
            ctx.state.sticky.insert(node, new);
            ctx.viewport_sticky |= svr.1;
            if svr.1 {
                ctx.state.viewport_constraints.insert(node, constraints);
            }
        }
        let mut child_svr = svr;
        if let Some(node) = f.node
            && ctx.state.boxes.contains_key(&node)
            && f.pseudo.is_none()
        {
            child_svr = (sticky_view(f), false);
        }
        let child_cb = if is_box { f.content_rect() } else { cb };
        for c in &mut f.children {
            walk(c, ctx, child_svr, child_cb);
        }
    }
    let viewport = tree.viewport;
    let mut state = std::mem::take(&mut tree.scroll);
    // E4: se rehacen todas en el recorrido.
    state.viewport_constraints.clear();
    if let Some(root) = tree.root.as_mut() {
        let mut ctx = Ctx {
            state: &mut state,
            viewport,
            page_scroll,
            viewport_sticky: false,
        };
        let vp = Rect::new(
            ctx.page_scroll.0,
            ctx.page_scroll.1,
            viewport.width,
            viewport.height,
        );
        // El containing block del raíz es el inicial (el viewport en el origen).
        let icb = Rect::new(0.0, 0.0, viewport.width, viewport.height);
        walk(root, &mut ctx, (vp, true), icb);
        let viewport_sticky = ctx.viewport_sticky;
        state.viewport_sticky = viewport_sticky;
    }
    state.page_scroll = page_scroll;
    tree.scroll = state;
}

/// El scroll de la página cambió: recalcula lo `sticky` que depende del viewport. Devuelve si
/// cambió algo en el árbol.
pub fn page_scrolled(tree: &mut FragmentTree, page_scroll: (f32, f32)) -> bool {
    if tree.scroll.page_scroll == page_scroll {
        return false;
    }
    if !tree.scroll.viewport_sticky {
        tree.scroll.page_scroll = page_scroll;
        return false;
    }
    let before = tree.scroll.sticky.clone();
    update_sticky(tree, page_scroll);
    before != tree.scroll.sticky
}
