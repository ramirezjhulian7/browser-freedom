//! Del documento al árbol de render: herencia y valores computados (SVG 2 §6, §13), viewports
//! (§8.2, `viewBox` §8.6, `preserveAspectRatio` §8.7), formas (§9, §10), `use` (§5.6), texto
//! básico (§11), servidores de pintura (§14) y `clipPath` (CSS Masking 1 §6).

use bf_css::syntax::{ComponentValue, TokenKind, parse_component_value_list};
use bf_style::values::color::{Color, parse_color};

use crate::geom::{self, Seg, Transform};
use crate::style::Node as DocNode;
use crate::{
    Chunk, ClipPath, Fill, FillRule, Gradient, GradientKind, Group, Intrinsic, LineCap, LineJoin,
    Node, Paint, Path, Rgba, Span, Spread, Stroke, Svg, Text, TextAnchor, Tree,
};

/// Profundidad máxima de `use` y `href` encadenados (protege de ciclos).
const MAX_DEPTH: usize = 16;

/// Longitud especificada.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Len {
    Px(f32),
    Pct(f32),
    Em(f32),
    Ex(f32),
}

/// Eje contra el que se resuelven los porcentajes (§8.9).
#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
    Other,
}

fn unit_px(unit: &str) -> Option<f32> {
    Some(match unit.to_ascii_lowercase().as_str() {
        "" | "px" => 1.0,
        "pt" => 4.0 / 3.0,
        "pc" => 16.0,
        "in" => 96.0,
        "cm" => 96.0 / 2.54,
        "mm" => 96.0 / 25.4,
        "q" => 96.0 / 101.6,
        _ => return None,
    })
}

fn len_from(v: f32, unit: &str) -> Option<Len> {
    match unit.to_ascii_lowercase().as_str() {
        "%" => Some(Len::Pct(v)),
        "em" => Some(Len::Em(v)),
        "ex" => Some(Len::Ex(v)),
        u => Some(Len::Px(v * unit_px(u)?)),
    }
}

/// `<length-percentage>` de un atributo.
fn parse_len(s: &str) -> Option<Len> {
    let s = s.trim();
    let mut n = geom::Numbers::new(s);
    let v = n.number()?;
    len_from(v, s.get(n.pos..)?.trim())
}

/// Los valores sin espacios de una declaración.
fn toks(v: &[ComponentValue]) -> Vec<&ComponentValue> {
    v.iter()
        .filter(|c| !matches!(c.token(), Some(TokenKind::Whitespace)))
        .collect()
}

fn one(v: &[ComponentValue]) -> Option<&ComponentValue> {
    match toks(v).as_slice() {
        [c] => Some(c),
        _ => None,
    }
}

fn ident(v: &[ComponentValue]) -> Option<String> {
    match one(v)?.token()? {
        TokenKind::Ident(s) => Some(s.to_ascii_lowercase()),
        _ => None,
    }
}

fn cv_len(c: &ComponentValue) -> Option<Len> {
    match c.token()? {
        TokenKind::Number(n) => Some(Len::Px(n.value as f32)),
        TokenKind::Percentage(n) => Some(Len::Pct(n.value as f32)),
        TokenKind::Dimension(n, unit) => len_from(n.value as f32, unit),
        _ => None,
    }
}

/// Número o porcentaje (como fracción).
fn cv_fraction(v: &[ComponentValue]) -> Option<f32> {
    match one(v)?.token()? {
        TokenKind::Number(n) => Some(n.value as f32),
        TokenKind::Percentage(n) => Some(n.value as f32 / 100.0),
        _ => None,
    }
}

/// Pintura especificada (§13.2).
#[derive(Debug, Clone, PartialEq)]
enum PaintSpec {
    None,
    Color(Rgba),
    CurrentColor,
    Url(String, Option<Box<PaintSpec>>),
}

fn url_of(c: &ComponentValue) -> Option<String> {
    match c {
        ComponentValue::Token(t) => match &t.kind {
            TokenKind::Url(u) => Some(u.clone()),
            _ => None,
        },
        ComponentValue::Function(f) if f.name.eq_ignore_ascii_case("url") => {
            toks(&f.value).first().and_then(|c| match c.token()? {
                TokenKind::String(s) => Some(s.clone()),
                _ => None,
            })
        }
        _ => None,
    }
}

/// `#id` de una URL local.
fn fragment(url: &str) -> Option<&str> {
    url.trim().rsplit_once('#').map(|(_, id)| id)
}

fn parse_paint(v: &[ComponentValue]) -> Option<PaintSpec> {
    let t = toks(v);
    let simple = |c: &ComponentValue| -> Option<PaintSpec> {
        if let Some(TokenKind::Ident(s)) = c.token()
            && s.eq_ignore_ascii_case("none")
        {
            return Some(PaintSpec::None);
        }
        // `context-fill`/`context-stroke` solo tienen sentido en marcadores y `use`.
        if let Some(TokenKind::Ident(s)) = c.token()
            && (s.eq_ignore_ascii_case("context-fill") || s.eq_ignore_ascii_case("context-stroke"))
        {
            return Some(PaintSpec::None);
        }
        match parse_color(c)? {
            Color::Rgba(c) => Some(PaintSpec::Color(c)),
            Color::CurrentColor => Some(PaintSpec::CurrentColor),
        }
    };
    match t.as_slice() {
        [c] => url_of(c)
            .map(|u| PaintSpec::Url(u, None))
            .or_else(|| simple(c)),
        [u, fb] => Some(PaintSpec::Url(url_of(u)?, Some(Box::new(simple(fb)?)))),
        _ => None,
    }
}

/// Propiedades heredadas, ya computadas.
#[derive(Debug, Clone)]
struct St {
    color: Rgba,
    fill: PaintSpec,
    fill_opacity: f32,
    fill_rule: FillRule,
    stroke: PaintSpec,
    stroke_width: f32,
    stroke_opacity: f32,
    cap: LineCap,
    join: LineJoin,
    miter: f32,
    dash: Vec<f32>,
    dash_offset: f32,
    family: Vec<String>,
    size: f32,
    weight: f32,
    italic: bool,
    anchor: TextAnchor,
    visible: bool,
    clip_rule: FillRule,
    letter_spacing: f32,
}

impl Default for St {
    fn default() -> Self {
        St {
            color: Rgba::BLACK,
            fill: PaintSpec::Color(Rgba::BLACK),
            fill_opacity: 1.0,
            fill_rule: FillRule::NonZero,
            stroke: PaintSpec::None,
            stroke_width: 1.0,
            stroke_opacity: 1.0,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter: 4.0,
            dash: Vec::new(),
            dash_offset: 0.0,
            family: vec!["serif".into()],
            size: 16.0,
            weight: 400.0,
            italic: false,
            anchor: TextAnchor::Start,
            visible: true,
            clip_rule: FillRule::NonZero,
            letter_spacing: 0.0,
        }
    }
}

/// Un viewport: tamaño para los porcentajes.
#[derive(Clone, Copy)]
struct Viewport {
    w: f32,
    h: f32,
}

impl Viewport {
    fn resolve(&self, l: Len, axis: Axis, font: f32) -> f32 {
        match l {
            Len::Px(v) => v,
            Len::Em(v) => v * font,
            Len::Ex(v) => v * font * 0.5,
            Len::Pct(p) => {
                let base = match axis {
                    Axis::X => self.w,
                    Axis::Y => self.h,
                    Axis::Other => ((self.w * self.w + self.h * self.h) / 2.0).sqrt(),
                };
                base * p / 100.0
            }
        }
    }
}

struct Builder<'a> {
    svg: &'a Svg,
    /// Elementos en curso (para cortar ciclos de `use`).
    stack: Vec<usize>,
}

fn is_inherited_reset(name: &str) -> bool {
    matches!(name, "inherit")
}

impl<'a> Builder<'a> {
    fn node(&self, i: usize) -> &'a DocNode {
        &self.svg.nodes[i]
    }

    fn by_id(&self, url: &str) -> Option<usize> {
        self.svg.ids.get(fragment(url)?).copied()
    }

    /// Aplica las propiedades heredadas declaradas en `n` sobre las del padre.
    fn computed(&self, n: &DocNode, parent: &St, vp: Viewport) -> St {
        let mut st = parent.clone();
        let get = |name: &str| n.props.get(name).filter(|v| !v.is_empty());
        // `color` y `font-size` primero: de ellos dependen `currentColor` y `em`.
        if let Some(v) = get("color")
            && let Some(c) = one(v).and_then(parse_color)
        {
            st.color = c.resolve(parent.color);
        }
        if let Some(v) = get("font-size") {
            let size = match ident(v).as_deref() {
                Some("xx-small") => Some(9.0),
                Some("x-small") => Some(10.0),
                Some("small") => Some(13.0),
                Some("medium") => Some(16.0),
                Some("large") => Some(18.0),
                Some("x-large") => Some(24.0),
                Some("xx-large") => Some(32.0),
                Some("larger") => Some(parent.size * 1.2),
                Some("smaller") => Some(parent.size / 1.2),
                _ => one(v).and_then(cv_len).map(|l| match l {
                    Len::Pct(p) => parent.size * p / 100.0,
                    l => vp.resolve(l, Axis::Other, parent.size),
                }),
            };
            if let Some(s) = size.filter(|s| *s >= 0.0) {
                st.size = s;
            }
        }
        for (name, v) in &n.props {
            if v.is_empty() {
                continue;
            }
            let id = ident(v);
            if id.as_deref().is_some_and(is_inherited_reset) {
                continue;
            }
            match name.as_str() {
                "fill" => {
                    if let Some(p) = parse_paint(v) {
                        st.fill = p;
                    }
                }
                "stroke" => {
                    if let Some(p) = parse_paint(v) {
                        st.stroke = p;
                    }
                }
                "fill-opacity" => {
                    if let Some(o) = cv_fraction(v) {
                        st.fill_opacity = o.clamp(0.0, 1.0);
                    }
                }
                "stroke-opacity" => {
                    if let Some(o) = cv_fraction(v) {
                        st.stroke_opacity = o.clamp(0.0, 1.0);
                    }
                }
                "fill-rule" | "clip-rule" => {
                    let rule = match id.as_deref() {
                        Some("nonzero") => FillRule::NonZero,
                        Some("evenodd") => FillRule::EvenOdd,
                        _ => continue,
                    };
                    if name == "fill-rule" {
                        st.fill_rule = rule;
                    } else {
                        st.clip_rule = rule;
                    }
                }
                "stroke-width" => {
                    if let Some(l) = one(v).and_then(cv_len) {
                        let w = vp.resolve(l, Axis::Other, st.size);
                        if w >= 0.0 {
                            st.stroke_width = w;
                        }
                    }
                }
                "stroke-linecap" => {
                    st.cap = match id.as_deref() {
                        Some("butt") => LineCap::Butt,
                        Some("round") => LineCap::Round,
                        Some("square") => LineCap::Square,
                        _ => continue,
                    };
                }
                "stroke-linejoin" => {
                    st.join = match id.as_deref() {
                        Some("miter") => LineJoin::Miter,
                        Some("miter-clip") => LineJoin::MiterClip,
                        Some("round") => LineJoin::Round,
                        Some("bevel") => LineJoin::Bevel,
                        Some("arcs") => LineJoin::Arcs,
                        _ => continue,
                    };
                }
                "stroke-miterlimit" => {
                    if let Some(TokenKind::Number(m)) = one(v).and_then(|c| c.token())
                        && m.value >= 1.0
                    {
                        st.miter = m.value as f32;
                    }
                }
                "stroke-dasharray" => {
                    if id.as_deref() == Some("none") {
                        st.dash.clear();
                        continue;
                    }
                    let parts: Option<Vec<f32>> = toks(v)
                        .into_iter()
                        .filter(|c| !matches!(c.token(), Some(TokenKind::Comma)))
                        .map(|c| cv_len(c).map(|l| vp.resolve(l, Axis::Other, st.size)))
                        .collect();
                    if let Some(p) = parts
                        && p.iter().all(|d| *d >= 0.0)
                    {
                        st.dash = p;
                    }
                }
                "stroke-dashoffset" => {
                    if let Some(l) = one(v).and_then(cv_len) {
                        st.dash_offset = vp.resolve(l, Axis::Other, st.size);
                    }
                }
                "font-family" => {
                    let mut families = Vec::new();
                    for part in bf_css::syntax::split_comma_separated(v) {
                        let words: Vec<String> = toks(&part)
                            .iter()
                            .filter_map(|c| match c.token()? {
                                TokenKind::Ident(s) | TokenKind::String(s) => Some(s.clone()),
                                _ => None,
                            })
                            .collect();
                        if !words.is_empty() {
                            families.push(words.join(" "));
                        }
                    }
                    if !families.is_empty() {
                        st.family = families;
                    }
                }
                "font-weight" => {
                    st.weight = match id.as_deref() {
                        Some("normal") => 400.0,
                        Some("bold") => 700.0,
                        Some("bolder") => (parent.weight + 300.0).min(900.0),
                        Some("lighter") => (parent.weight - 300.0).max(100.0),
                        _ => match one(v).and_then(|c| c.token()) {
                            Some(TokenKind::Number(n)) => n.value as f32,
                            _ => continue,
                        },
                    };
                }
                "font-style" => {
                    st.italic = matches!(id.as_deref(), Some("italic" | "oblique"));
                }
                "text-anchor" => {
                    st.anchor = match id.as_deref() {
                        Some("start") => TextAnchor::Start,
                        Some("middle") => TextAnchor::Middle,
                        Some("end") => TextAnchor::End,
                        _ => continue,
                    };
                }
                "visibility" => {
                    st.visible = match id.as_deref() {
                        Some("visible") => true,
                        Some("hidden" | "collapse") => false,
                        _ => continue,
                    };
                }
                "letter-spacing" => {
                    st.letter_spacing = match id.as_deref() {
                        Some("normal") => 0.0,
                        _ => match one(v).and_then(cv_len) {
                            Some(l) => vp.resolve(l, Axis::Other, st.size),
                            None => continue,
                        },
                    };
                }
                _ => {}
            }
        }
        st
    }

    fn len_attr(&self, n: &DocNode, name: &str) -> Option<Len> {
        if let Some(v) = n.props.get(name)
            && let Some(l) = one(v).and_then(cv_len)
        {
            return Some(l);
        }
        parse_len(n.attr(name)?)
    }

    fn len(&self, n: &DocNode, name: &str, axis: Axis, vp: Viewport, st: &St) -> Option<f32> {
        self.len_attr(n, name)
            .map(|l| vp.resolve(l, axis, st.size))
    }

    fn display_none(n: &DocNode) -> bool {
        n.props
            .get("display")
            .and_then(|v| ident(v))
            .is_some_and(|d| d == "none")
    }

    fn opacity(n: &DocNode) -> f32 {
        n.props
            .get("opacity")
            .and_then(|v| cv_fraction(v))
            .map_or(1.0, |o| o.clamp(0.0, 1.0))
    }

    fn transform(n: &DocNode) -> Transform {
        n.attr("transform")
            .and_then(geom::parse_transform)
            .unwrap_or_default()
    }

    /// Un elemento renderizable (o `None`).
    fn element(&mut self, i: usize, parent: &St, vp: Viewport) -> Option<Node> {
        if self.stack.len() > MAX_DEPTH * 4 || self.stack.contains(&i) {
            return None;
        }
        let n = self.node(i);
        if n.is_text() || Self::display_none(n) {
            return None;
        }
        let st = self.computed(n, parent, vp);
        self.stack.push(i);
        let content = self.content(i, &st, vp);
        self.stack.pop();
        let n = self.node(i);
        let mut node = content?;
        let transform = Self::transform(n);
        let opacity = Self::opacity(n);
        let clip = n
            .props
            .get("clip-path")
            .and_then(|v| one(v).and_then(url_of))
            .and_then(|u| self.by_id(&u))
            .filter(|&c| self.node(c).name == "clipPath")
            .map(|c| (c, st.clone()));
        if opacity >= 1.0 && clip.is_none() {
            match &mut node {
                Node::Path(p) => {
                    p.transform = transform.then(&p.transform);
                    return Some(node);
                }
                Node::Text(t) => {
                    t.transform = transform.then(&t.transform);
                    return Some(node);
                }
                Node::Group(g) if g.opacity >= 1.0 && g.clip.is_none() => {
                    g.transform = transform.then(&g.transform);
                    return Some(node);
                }
                Node::Group(_) => {}
            }
        }
        let clip = match clip {
            Some((c, st)) => {
                let bbox = node_bbox(&node, &Transform::IDENTITY);
                Some(self.clip_path(c, &st, vp, bbox)?)
            }
            None => None,
        };
        Some(Node::Group(Group {
            transform,
            opacity,
            clip,
            children: vec![node],
        }))
    }

    fn children(&mut self, i: usize, st: &St, vp: Viewport) -> Vec<Node> {
        let kids = self.node(i).children.clone();
        kids.into_iter()
            .filter_map(|c| self.element(c, st, vp))
            .collect()
    }

    fn content(&mut self, i: usize, st: &St, vp: Viewport) -> Option<Node> {
        let n = self.node(i);
        let shape = |segs: Vec<Seg>| Some(segs).filter(|s| !s.is_empty());
        let segs = match n.name.as_str() {
            "g" | "a" => {
                return Some(Node::Group(Group {
                    opacity: 1.0,
                    children: self.children(i, st, vp),
                    ..Group::default()
                }));
            }
            "switch" => {
                // El primer hijo renderizable (no evaluamos `systemLanguage` ni
                // `requiredExtensions`: se toman como cumplidos salvo extensiones).
                let kids = n.children.clone();
                let child = kids.into_iter().find_map(|c| {
                    let cn = self.node(c);
                    if cn.is_text() || cn.attrs.contains_key("requiredExtensions") {
                        return None;
                    }
                    self.element(c, st, vp)
                });
                return Some(Node::Group(Group {
                    opacity: 1.0,
                    children: child.into_iter().collect(),
                    ..Group::default()
                }));
            }
            "svg" => return self.nested_svg(i, i, st, vp, None),
            "use" => return self.use_element(i, st, vp),
            "text" => return self.text(i, st, vp),
            "rect" => {
                let x = self.len(n, "x", Axis::X, vp, st).unwrap_or(0.0);
                let y = self.len(n, "y", Axis::Y, vp, st).unwrap_or(0.0);
                let w = self.len(n, "width", Axis::X, vp, st)?;
                let h = self.len(n, "height", Axis::Y, vp, st)?;
                if w <= 0.0 || h <= 0.0 {
                    return None;
                }
                let rx = self.len(n, "rx", Axis::X, vp, st).filter(|v| *v >= 0.0);
                let ry = self.len(n, "ry", Axis::Y, vp, st).filter(|v| *v >= 0.0);
                let (rx, ry) = match (rx, ry) {
                    (Some(a), Some(b)) => (a, b),
                    (Some(a), None) | (None, Some(a)) => (a, a),
                    (None, None) => (0.0, 0.0),
                };
                geom::rect(x, y, w, h, rx.min(w / 2.0), ry.min(h / 2.0))
            }
            "circle" => {
                let cx = self.len(n, "cx", Axis::X, vp, st).unwrap_or(0.0);
                let cy = self.len(n, "cy", Axis::Y, vp, st).unwrap_or(0.0);
                let r = self.len(n, "r", Axis::Other, vp, st)?;
                if r <= 0.0 {
                    return None;
                }
                geom::ellipse(cx, cy, r, r)
            }
            "ellipse" => {
                let cx = self.len(n, "cx", Axis::X, vp, st).unwrap_or(0.0);
                let cy = self.len(n, "cy", Axis::Y, vp, st).unwrap_or(0.0);
                let rx = self.len(n, "rx", Axis::X, vp, st);
                let ry = self.len(n, "ry", Axis::Y, vp, st);
                let (rx, ry) = match (rx, ry) {
                    (Some(a), Some(b)) => (a, b),
                    (Some(a), None) | (None, Some(a)) => (a, a),
                    (None, None) => return None,
                };
                if rx <= 0.0 || ry <= 0.0 {
                    return None;
                }
                geom::ellipse(cx, cy, rx, ry)
            }
            "line" => {
                let x1 = self.len(n, "x1", Axis::X, vp, st).unwrap_or(0.0);
                let y1 = self.len(n, "y1", Axis::Y, vp, st).unwrap_or(0.0);
                let x2 = self.len(n, "x2", Axis::X, vp, st).unwrap_or(0.0);
                let y2 = self.len(n, "y2", Axis::Y, vp, st).unwrap_or(0.0);
                vec![Seg::MoveTo(x1, y1), Seg::LineTo(x2, y2)]
            }
            "polyline" | "polygon" => {
                let pts = geom::parse_points(n.attr("points").unwrap_or(""));
                let mut segs: Vec<Seg> = pts
                    .iter()
                    .enumerate()
                    .map(|(k, &(x, y))| {
                        if k == 0 {
                            Seg::MoveTo(x, y)
                        } else {
                            Seg::LineTo(x, y)
                        }
                    })
                    .collect();
                if n.name == "polygon" && !segs.is_empty() {
                    segs.push(Seg::Close);
                }
                segs
            }
            "path" => geom::parse_path(n.attr("d").unwrap_or("")),
            _ => return None,
        };
        let segs = shape(segs)?;
        if !st.visible {
            return None;
        }
        let bbox = geom::bbox(&segs);
        Some(Node::Path(Path {
            fill: self.fill(st, bbox, vp),
            stroke: self.stroke(st, bbox, vp),
            segs,
            transform: Transform::IDENTITY,
        }))
    }

    /// Un `svg` anidado (o un `symbol`/`svg` referenciado por `use`, con `size` del `use`):
    /// un nuevo viewport con su `viewBox`, recortado (`overflow: hidden` por defecto).
    fn nested_svg(
        &mut self,
        geometry: usize,
        content: usize,
        st: &St,
        vp: Viewport,
        size: Option<(Option<f32>, Option<f32>)>,
    ) -> Option<Node> {
        let g = self.node(geometry);
        let is_use = size.is_some();
        let (x, y) = if is_use {
            (0.0, 0.0)
        } else {
            (
                self.len(g, "x", Axis::X, vp, st).unwrap_or(0.0),
                self.len(g, "y", Axis::Y, vp, st).unwrap_or(0.0),
            )
        };
        let c = self.node(content);
        let (uw, uh) = size.unwrap_or((None, None));
        let w = uw
            .or_else(|| self.len(c, "width", Axis::X, vp, st))
            .unwrap_or(vp.w);
        let h = uh
            .or_else(|| self.len(c, "height", Axis::Y, vp, st))
            .unwrap_or(vp.h);
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        let view_box = view_box(c);
        let inner_vp = match view_box {
            Some((_, _, vw, vh)) => Viewport { w: vw, h: vh },
            None => Viewport { w, h },
        };
        let par = c.attr("preserveAspectRatio").unwrap_or("");
        let vb_t = view_box.map_or(Transform::IDENTITY, |vb| view_box_transform(vb, par, (w, h)));
        let children = self.children(content, st, inner_vp);
        let overflow_visible = c
            .props
            .get("overflow")
            .and_then(|v| ident(v))
            .is_some_and(|o| o == "visible" || o == "auto");
        let clip = (!overflow_visible).then(|| ClipPath {
            transform: Transform::IDENTITY,
            shapes: vec![(geom::rect(0.0, 0.0, w, h, 0.0, 0.0), FillRule::NonZero, Transform::IDENTITY)],
            texts: Vec::new(),
        });
        Some(Node::Group(Group {
            transform: Transform::translate(x, y),
            opacity: 1.0,
            clip,
            children: vec![Node::Group(Group {
                transform: vb_t,
                opacity: 1.0,
                clip: None,
                children,
            })],
        }))
    }

    fn use_element(&mut self, i: usize, st: &St, vp: Viewport) -> Option<Node> {
        let n = self.node(i);
        let target = n
            .attr("href")
            .and_then(|h| self.by_id(h))
            .filter(|&t| !self.stack.contains(&t))?;
        let x = self.len(n, "x", Axis::X, vp, st).unwrap_or(0.0);
        let y = self.len(n, "y", Axis::Y, vp, st).unwrap_or(0.0);
        let w = self.len(n, "width", Axis::X, vp, st);
        let h = self.len(n, "height", Axis::Y, vp, st);
        let tn = self.node(target);
        let inner = match tn.name.as_str() {
            "symbol" | "svg" => {
                if Self::display_none(tn) {
                    return None;
                }
                let tst = self.computed(tn, st, vp);
                self.stack.push(target);
                let node = self.nested_svg(target, target, &tst, vp, Some((w, h)));
                self.stack.pop();
                let tn = self.node(target);
                // Los atributos de presentación propios del `symbol` (opacidad, transformación).
                match node {
                    Some(Node::Group(mut g)) => {
                        if tn.name == "symbol" {
                            g.transform = Self::transform(tn).then(&g.transform);
                        }
                        g.opacity *= Self::opacity(tn);
                        Node::Group(g)
                    }
                    other => other?,
                }
            }
            _ => self.element(target, st, vp)?,
        };
        Some(Node::Group(Group {
            transform: Transform::translate(x, y),
            opacity: 1.0,
            clip: None,
            children: vec![inner],
        }))
    }

    fn resolve_paint(&self, spec: &PaintSpec, st: &St, bbox: Option<Bbox>, vp: Viewport) -> Option<Paint> {
        match spec {
            PaintSpec::None => None,
            PaintSpec::Color(c) => Some(Paint::Color(*c)),
            PaintSpec::CurrentColor => Some(Paint::Color(st.color)),
            PaintSpec::Url(url, fallback) => {
                let server = self
                    .by_id(url)
                    .filter(|&s| matches!(self.node(s).name.as_str(), "linearGradient" | "radialGradient"));
                match server {
                    Some(s) => self.gradient(s, bbox, vp, st),
                    // Referencia inválida: el respaldo, o nada (§13.2).
                    None => fallback
                        .as_deref()
                        .and_then(|f| self.resolve_paint(f, st, bbox, vp)),
                }
            }
        }
    }

    fn fill(&self, st: &St, bbox: Option<Bbox>, vp: Viewport) -> Option<Fill> {
        Some(Fill {
            paint: self.resolve_paint(&st.fill, st, bbox, vp)?,
            opacity: st.fill_opacity,
            rule: st.fill_rule,
        })
    }

    fn stroke(&self, st: &St, bbox: Option<Bbox>, vp: Viewport) -> Option<Stroke> {
        if st.stroke_width <= 0.0 {
            return None;
        }
        let mut dash = st.dash.clone();
        if dash.iter().sum::<f32>() <= 0.0 {
            dash.clear();
        } else if dash.len() % 2 == 1 {
            dash.extend(dash.clone());
        }
        Some(Stroke {
            paint: self.resolve_paint(&st.stroke, st, bbox, vp)?,
            opacity: st.stroke_opacity,
            width: st.stroke_width,
            cap: st.cap,
            join: st.join,
            miter_limit: st.miter,
            dasharray: dash,
            dashoffset: st.dash_offset,
        })
    }

    /// Cadena de `href` de un gradiente (él primero).
    fn chain(&self, s: usize) -> Vec<usize> {
        let mut out = vec![s];
        while out.len() < MAX_DEPTH {
            let last = self.node(out[out.len() - 1]);
            match last.attr("href").and_then(|h| self.by_id(h)) {
                Some(next)
                    if !out.contains(&next)
                        && matches!(
                            self.node(next).name.as_str(),
                            "linearGradient" | "radialGradient"
                        ) =>
                {
                    out.push(next)
                }
                _ => break,
            }
        }
        out
    }

    fn gradient(&self, s: usize, bbox: Option<Bbox>, vp: Viewport, st: &St) -> Option<Paint> {
        let chain = self.chain(s);
        let attr = |name: &str| chain.iter().find_map(|&c| self.node(c).attr(name));
        let bbox_units = attr("gradientUnits").is_none_or(|u| u.trim() != "userSpaceOnUse");
        let stops_node = chain.iter().copied().find(|&c| {
            self.node(c)
                .children
                .iter()
                .any(|&k| self.node(k).name == "stop")
        });
        let mut stops: Vec<(f32, Rgba)> = Vec::new();
        if let Some(sn) = stops_node {
            for &k in &self.node(sn).children {
                let stop = self.node(k);
                if stop.name != "stop" {
                    continue;
                }
                let offset = stop
                    .attr("offset")
                    .and_then(|o| {
                        let o = o.trim();
                        match o.strip_suffix('%') {
                            Some(p) => p.trim().parse::<f32>().ok().map(|p| p / 100.0),
                            None => o.parse::<f32>().ok(),
                        }
                    })
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0);
                let offset = stops.last().map_or(offset, |l| offset.max(l.0));
                let color_prop = stop
                    .props
                    .get("color")
                    .and_then(|v| one(v).and_then(parse_color))
                    .map_or(st.color, |c| c.resolve(st.color));
                let mut color = stop
                    .props
                    .get("stop-color")
                    .and_then(|v| one(v).and_then(parse_color))
                    .map_or(Rgba::BLACK, |c| c.resolve(color_prop));
                let opacity = stop
                    .props
                    .get("stop-opacity")
                    .and_then(|v| cv_fraction(v))
                    .unwrap_or(1.0)
                    .clamp(0.0, 1.0);
                color.a *= opacity;
                stops.push((offset, color));
            }
        }
        match stops.len() {
            0 => return None,
            1 => return Some(Paint::Color(stops[0].1)),
            _ => {}
        }
        let unit_tf = if bbox_units {
            let (bx, by, bw, bh) = bbox?;
            if bw <= 0.0 || bh <= 0.0 {
                return None;
            }
            Transform::translate(bx, by).then(&Transform::scale(bw, bh))
        } else {
            Transform::IDENTITY
        };
        let gt = attr("gradientTransform")
            .and_then(geom::parse_transform)
            .unwrap_or_default();
        let coord = |name: &str, default: &str, axis: Axis| -> f32 {
            let l = attr(name).and_then(parse_len).or_else(|| parse_len(default));
            match (l, bbox_units) {
                (Some(Len::Pct(p)), true) => p / 100.0,
                (Some(Len::Px(v)), true) => v,
                (Some(l), _) => vp.resolve(l, axis, st.size),
                (None, _) => 0.0,
            }
        };
        let is_radial = self.node(s).name == "radialGradient";
        let kind = if is_radial {
            let cx = coord("cx", "50%", Axis::X);
            let cy = coord("cy", "50%", Axis::Y);
            let r = coord("r", "50%", Axis::Other);
            if r <= 0.0 {
                // Radio cero: el color de la última parada (§14.3.3).
                return Some(Paint::Color(stops[stops.len() - 1].1));
            }
            let fx = if attr("fx").is_some() {
                coord("fx", "50%", Axis::X)
            } else {
                cx
            };
            let fy = if attr("fy").is_some() {
                coord("fy", "50%", Axis::Y)
            } else {
                cy
            };
            GradientKind::Radial {
                cx,
                cy,
                r,
                fx,
                fy,
                fr: coord("fr", "0%", Axis::Other),
            }
        } else {
            GradientKind::Linear {
                x1: coord("x1", "0%", Axis::X),
                y1: coord("y1", "0%", Axis::Y),
                x2: coord("x2", "100%", Axis::X),
                y2: coord("y2", "0%", Axis::Y),
            }
        };
        if let GradientKind::Linear { x1, y1, x2, y2 } = kind
            && x1 == x2
            && y1 == y2
        {
            // Vector nulo: el color de la última parada (§14.2.2).
            return Some(Paint::Color(stops[stops.len() - 1].1));
        }
        let spread = match attr("spreadMethod").map(str::trim) {
            Some("reflect") => Spread::Reflect,
            Some("repeat") => Spread::Repeat,
            _ => Spread::Pad,
        };
        Some(Paint::Gradient(Box::new(Gradient {
            kind,
            transform: unit_tf.then(&gt),
            spread,
            stops,
        })))
    }

    /// `clipPath` referenciado por un elemento con caja `bbox`.
    fn clip_path(&mut self, c: usize, st: &St, vp: Viewport, bbox: Option<Bbox>) -> Option<ClipPath> {
        let cn = self.node(c);
        let bbox_units = cn
            .attr("clipPathUnits")
            .is_some_and(|u| u.trim() == "objectBoundingBox");
        let mut transform = Self::transform(cn);
        if bbox_units {
            let (bx, by, bw, bh) = bbox?;
            transform = Transform::translate(bx, by)
                .then(&Transform::scale(bw, bh))
                .then(&transform);
        }
        let cst = self.computed(cn, st, vp);
        let mut clip = ClipPath {
            transform,
            shapes: Vec::new(),
            texts: Vec::new(),
        };
        if self.stack.contains(&c) {
            return Some(clip);
        }
        self.stack.push(c);
        let kids = self.node(c).children.clone();
        for k in kids {
            let kn = self.node(k);
            if kn.is_text() || !matches!(
                kn.name.as_str(),
                "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon" | "path" | "text" | "use"
            ) {
                continue;
            }
            // En un recorte cuenta solo la geometría: relleno opaco, sin trazo (§6.3).
            let mut kst = self.computed(kn, &cst, vp);
            kst.fill = PaintSpec::Color(Rgba::BLACK);
            kst.fill_opacity = 1.0;
            kst.stroke = PaintSpec::None;
            let rule = kst.clip_rule;
            if let Some(node) = self.element(k, &kst, vp) {
                flatten_clip(&node, &Transform::IDENTITY, rule, &mut clip);
            }
        }
        self.stack.pop();
        Some(clip)
    }

    fn text(&mut self, i: usize, st: &St, vp: Viewport) -> Option<Node> {
        let mut chunks: Vec<Chunk> = Vec::new();
        self.text_runs(i, st, vp, &mut chunks, true);
        // Espacios: los del principio y el final del elemento `text` no cuentan.
        if let Some(first) = chunks.iter_mut().flat_map(|c| c.spans.iter_mut()).next() {
            first.text = first.text.trim_start().to_owned();
        }
        if let Some(last) = chunks.iter_mut().flat_map(|c| c.spans.iter_mut()).next_back() {
            last.text = last.text.trim_end().to_owned();
        }
        chunks.retain(|c| !c.spans.is_empty());
        if chunks.is_empty() {
            return None;
        }
        Some(Node::Text(Text {
            transform: Transform::IDENTITY,
            chunks,
        }))
    }

    /// Recorre `text`/`tspan`/`a`: cada `x`/`y` abre un *chunk*; el texto se normaliza como
    /// `white-space: normal` (espacios colapsados también entre fragmentos).
    fn text_runs(&mut self, i: usize, st: &St, vp: Viewport, chunks: &mut Vec<Chunk>, root: bool) {
        let n = self.node(i);
        let first_len = |name: &str, axis: Axis| {
            n.attr(name)
                .and_then(|v| v.split([' ', ',', '\t', '\n']).find(|s| !s.is_empty()))
                .and_then(parse_len)
                .map(|l| vp.resolve(l, axis, st.size))
        };
        let x = first_len("x", Axis::X);
        let y = first_len("y", Axis::Y);
        let mut dx = first_len("dx", Axis::X).unwrap_or(0.0);
        let mut dy = first_len("dy", Axis::Y).unwrap_or(0.0);
        if root || x.is_some() || y.is_some() || chunks.is_empty() {
            chunks.push(Chunk {
                x: x.or(if root { Some(0.0) } else { None }),
                y: y.or(if root { Some(0.0) } else { None }),
                anchor: st.anchor,
                spans: Vec::new(),
            });
        }
        let kids = n.children.clone();
        for k in kids {
            let kn = self.node(k);
            if kn.is_text() {
                let mut text: String = kn
                    .text
                    .chars()
                    .map(|c| if c.is_whitespace() { ' ' } else { c })
                    .collect();
                // Colapsa con el espacio final del fragmento anterior.
                let prev_space = chunks
                    .iter()
                    .flat_map(|c| c.spans.iter())
                    .next_back()
                    .is_none_or(|s| s.text.ends_with(' '));
                let mut collapsed = String::with_capacity(text.len());
                let mut last_space = prev_space;
                for c in text.drain(..) {
                    if c == ' ' {
                        if !last_space {
                            collapsed.push(' ');
                        }
                        last_space = true;
                    } else {
                        collapsed.push(c);
                        last_space = false;
                    }
                }
                if collapsed.is_empty() || !st.visible {
                    continue;
                }
                let bbox = None;
                let span = Span {
                    text: collapsed,
                    family: st.family.clone(),
                    size: st.size,
                    weight: st.weight,
                    italic: st.italic,
                    fill: self.fill(st, bbox, vp),
                    stroke: self.stroke(st, bbox, vp),
                    dx: std::mem::take(&mut dx),
                    dy: std::mem::take(&mut dy),
                };
                if let Some(c) = chunks.last_mut() {
                    c.spans.push(span);
                }
                continue;
            }
            if !matches!(kn.name.as_str(), "tspan" | "a") || Self::display_none(kn) {
                continue;
            }
            let kst = self.computed(kn, st, vp);
            self.text_runs(k, &kst, vp, chunks, false);
        }
    }
}

type Bbox = (f32, f32, f32, f32);

/// Caja de un nodo en el sistema de `t` (aproximada con las esquinas transformadas).
fn node_bbox(node: &Node, t: &Transform) -> Option<Bbox> {
    let union = |a: Option<Bbox>, b: Option<Bbox>| match (a, b) {
        (Some(a), Some(b)) => {
            let x0 = a.0.min(b.0);
            let y0 = a.1.min(b.1);
            let x1 = (a.0 + a.2).max(b.0 + b.2);
            let y1 = (a.1 + a.3).max(b.1 + b.3);
            Some((x0, y0, x1 - x0, y1 - y0))
        }
        (a, None) => a,
        (None, b) => b,
    };
    let corners = |b: Bbox, t: &Transform| -> Bbox {
        let pts = [
            t.apply(b.0, b.1),
            t.apply(b.0 + b.2, b.1),
            t.apply(b.0, b.1 + b.3),
            t.apply(b.0 + b.2, b.1 + b.3),
        ];
        let x0 = pts.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
        let y0 = pts.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
        let x1 = pts.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max);
        let y1 = pts.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);
        (x0, y0, x1 - x0, y1 - y0)
    };
    match node {
        Node::Path(p) => geom::bbox(&p.segs).map(|b| corners(b, &t.then(&p.transform))),
        Node::Group(g) => {
            let t = t.then(&g.transform);
            g.children
                .iter()
                .fold(None, |acc, c| union(acc, node_bbox(c, &t)))
        }
        // Sin métricas de fuente aquí: una caja aproximada desde la posición del primer chunk.
        Node::Text(tx) => {
            let c = tx.chunks.first()?;
            let size = c.spans.first().map_or(16.0, |s| s.size);
            let len: usize = c.spans.iter().map(|s| s.text.chars().count()).sum();
            let (x, y) = (c.x.unwrap_or(0.0), c.y.unwrap_or(0.0));
            let b = (x, y - size * 0.8, len as f32 * size * 0.5, size);
            Some(corners(b, &t.then(&tx.transform)))
        }
    }
}

fn flatten_clip(node: &Node, t: &Transform, rule: crate::FillRule, out: &mut ClipPath) {
    match node {
        Node::Path(p) => out.shapes.push((p.segs.clone(), rule, t.then(&p.transform))),
        Node::Group(g) => {
            let t = t.then(&g.transform);
            for c in &g.children {
                flatten_clip(c, &t, rule, out);
            }
        }
        Node::Text(tx) => {
            let mut tx = tx.clone();
            tx.transform = t.then(&tx.transform);
            out.texts.push(tx);
        }
    }
}

fn view_box(n: &DocNode) -> Option<Bbox> {
    let v = geom::parse_numbers(n.attr("viewBox")?)?;
    match v.as_slice() {
        &[x, y, w, h] if w > 0.0 && h > 0.0 => Some((x, y, w, h)),
        _ => None,
    }
}

/// Transformación del `viewBox` al viewport de `w`×`h` (§8.7).
fn view_box_transform((vx, vy, vw, vh): Bbox, par: &str, (w, h): (f32, f32)) -> Transform {
    let mut words = par.split_ascii_whitespace();
    let mut align = words.next().unwrap_or("xMidYMid");
    if align == "defer" {
        align = words.next().unwrap_or("xMidYMid");
    }
    let slice = words.next() == Some("slice");
    let (sx, sy) = (w / vw, h / vh);
    if align == "none" {
        return Transform::scale(sx, sy).then(&Transform::translate(-vx, -vy));
    }
    let s = if slice { sx.max(sy) } else { sx.min(sy) };
    let (ax, ay) = (align.get(0..4).unwrap_or("xMid"), align.get(4..8).unwrap_or("YMid"));
    let tx = match ax {
        "xMin" => 0.0,
        "xMax" => w - vw * s,
        _ => (w - vw * s) / 2.0,
    };
    let ty = match ay {
        "YMin" => 0.0,
        "YMax" => h - vh * s,
        _ => (h - vh * s) / 2.0,
    };
    Transform::translate(tx, ty)
        .then(&Transform::scale(s, s))
        .then(&Transform::translate(-vx, -vy))
}

/// Longitud absoluta de un atributo de la raíz (los porcentajes no dan tamaño natural).
fn absolute(n: &DocNode, name: &str) -> Option<f32> {
    let l = n
        .props
        .get(name)
        .and_then(|v| one(v).and_then(cv_len))
        .or_else(|| parse_len(n.attr(name)?))?;
    match l {
        Len::Px(v) => Some(v),
        Len::Em(v) => Some(v * 16.0),
        Len::Ex(v) => Some(v * 8.0),
        Len::Pct(_) => None,
    }
    .filter(|v| *v >= 0.0)
}

pub(crate) fn intrinsic(svg: &Svg) -> Intrinsic {
    let Some(root) = svg.nodes.first() else {
        return Intrinsic::default();
    };
    let width = absolute(root, "width");
    let height = absolute(root, "height");
    let ratio = match (width, height, view_box(root)) {
        (Some(w), Some(h), _) if w > 0.0 && h > 0.0 => Some(w / h),
        (_, _, Some((_, _, vw, vh))) => Some(vw / vh),
        _ => None,
    };
    Intrinsic {
        width,
        height,
        ratio,
    }
}

pub(crate) fn tree(svg: &Svg, width: f32, height: f32) -> Tree {
    let mut b = Builder {
        svg,
        stack: Vec::new(),
    };
    let mut root = Group {
        transform: Transform::IDENTITY,
        opacity: 1.0,
        clip: None,
        children: Vec::new(),
    };
    if let Some(r) = svg.nodes.first()
        && r.name == "svg"
        && !Builder::display_none(r)
    {
        let vb = view_box(r);
        let vp = match vb {
            Some((_, _, w, h)) => Viewport { w, h },
            None => Viewport {
                w: width,
                h: height,
            },
        };
        let base = St {
            color: svg.inherited.color,
            size: svg.inherited.font_size,
            family: svg.inherited.font_family.clone(),
            ..St::default()
        };
        let st = b.computed(r, &base, vp);
        root.transform = vb.map_or(Transform::IDENTITY, |vb| {
            view_box_transform(vb, r.attr("preserveAspectRatio").unwrap_or(""), (width, height))
        });
        root.opacity = Builder::opacity(r);
        b.stack.push(0);
        root.children = b.children(0, &st, vp);
    }
    Tree {
        width,
        height,
        root,
    }
}

/// Parsea un valor suelto como lista de component values (para tests).
#[allow(dead_code)]
pub(crate) fn values(s: &str) -> Vec<ComponentValue> {
    parse_component_value_list(s)
}
