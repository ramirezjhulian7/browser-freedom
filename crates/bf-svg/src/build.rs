//! Del documento al árbol de render: herencia y valores computados (SVG 2 §6, §13), viewports
//! (§8.2, `viewBox` §8.6, `preserveAspectRatio` §8.7), formas (§9, §10), `use` (§5.6), texto
//! básico (§11), servidores de pintura (§14) y `clipPath` (CSS Masking 1 §6).

use bf_css::syntax::{ComponentValue, TokenKind, parse_component_value_list};
use bf_style::values::color::{Color, parse_color};

use crate::geom::{self, Seg, Transform};
use crate::style::Node as DocNode;
use crate::{
    ClipPath, ContextPaint, ContextPaints, Fill, FillRule, Gradient, GradientKind, Group,
    Intrinsic, LineCap, LineJoin, Mask, Node, Paint, Path, Pattern, Rgba, Spread, Stroke, Svg,
    TextAnchor, Tree,
};

mod filter;
mod text;

/// Profundidad máxima de `use` y `href` encadenados (protege de ciclos).
const MAX_DEPTH: usize = 16;

/// Longitud especificada.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Len {
    Px(f32),
    Pct(f32),
    Em(f32),
    Ex(f32),
    /// `calc()`: px + % + em.
    Calc(f32, f32, f32),
    /// Unidades del viewport inicial (CSS Values 4 §6.1.2): `vw`, `vh`, `vmin`, `vmax`.
    Vw(f32),
    Vh(f32),
    Vmin(f32),
    Vmax(f32),
}

/// Valor de un `calc()` (CSS Values 4 §10): número puro o combinación lineal px/%/em.
#[derive(Debug, Clone, Copy)]
enum CalcVal {
    Num(f32),
    Len(f32, f32, f32),
}

impl CalcVal {
    fn lin(self) -> (f32, f32, f32) {
        match self {
            CalcVal::Num(n) => (n, 0.0, 0.0),
            CalcVal::Len(a, b, c) => (a, b, c),
        }
    }
}

/// Evalúa la suma de productos de un `calc()` (sin `min()`/`max()`).
fn calc(values: &[ComponentValue]) -> Option<CalcVal> {
    let t = toks(values);
    let factor = |c: &ComponentValue| -> Option<CalcVal> {
        match c {
            ComponentValue::Function(f) if f.name.eq_ignore_ascii_case("calc") => calc(&f.value),
            ComponentValue::Block(b) => calc(&b.value),
            _ => match c.token()? {
                TokenKind::Number(n) => Some(CalcVal::Num(n.value as f32)),
                _ => match cv_len(c)? {
                    Len::Px(v) => Some(CalcVal::Len(v, 0.0, 0.0)),
                    Len::Pct(v) => Some(CalcVal::Len(0.0, v, 0.0)),
                    Len::Em(v) => Some(CalcVal::Len(0.0, 0.0, v)),
                    Len::Ex(v) => Some(CalcVal::Len(0.0, 0.0, v / 2.0)),
                    Len::Calc(a, b, c) => Some(CalcVal::Len(a, b, c)),
                    // Sin unidades del viewport dentro de `calc()`.
                    Len::Vw(_) | Len::Vh(_) | Len::Vmin(_) | Len::Vmax(_) => None,
                },
            },
        }
    };
    let mut sum: Option<CalcVal> = None;
    let mut sign = 1.0f32;
    let mut i = 0;
    while i < t.len() {
        let mut term = factor(t[i])?;
        i += 1;
        while let Some(TokenKind::Delim(op @ ('*' | '/'))) = t.get(i).and_then(|c| c.token()) {
            let rhs = factor(t.get(i + 1)?)?;
            i += 2;
            term = match (term, rhs, op) {
                (CalcVal::Num(a), CalcVal::Num(b), '*') => CalcVal::Num(a * b),
                (CalcVal::Num(a), CalcVal::Num(b), _) => CalcVal::Num(a / b),
                (CalcVal::Num(k), CalcVal::Len(a, b, c), '*')
                | (CalcVal::Len(a, b, c), CalcVal::Num(k), '*') => {
                    CalcVal::Len(a * k, b * k, c * k)
                }
                (CalcVal::Len(a, b, c), CalcVal::Num(k), '/') => CalcVal::Len(a / k, b / k, c / k),
                _ => return None,
            };
        }
        let term = match term {
            CalcVal::Num(n) => CalcVal::Num(n * sign),
            CalcVal::Len(a, b, c) => CalcVal::Len(a * sign, b * sign, c * sign),
        };
        sum = Some(match (sum, term) {
            (None, t) => t,
            (Some(CalcVal::Num(a)), CalcVal::Num(b)) => CalcVal::Num(a + b),
            (Some(s), t) => {
                let ((a, b, c), (d, e, f)) = (s.lin(), t.lin());
                CalcVal::Len(a + d, b + e, c + f)
            }
        });
        match t.get(i).and_then(|c| c.token()) {
            None => break,
            Some(TokenKind::Delim('+')) => sign = 1.0,
            Some(TokenKind::Delim('-')) => sign = -1.0,
            _ => return None,
        }
        i += 1;
    }
    sum
}

/// ¿Es `prop` una propiedad de geometría de `element` (SVG 2 §9.1–§10)? Las demás longitudes
/// solo se leen del atributo (`use` ignora `width` de CSS, por ejemplo).
fn geometry_property(element: &str, prop: &str) -> bool {
    match prop {
        "x" | "y" | "width" | "height" => {
            // Los `svg` anidados no toman `width`/`height` de CSS (como los navegadores).
            matches!(element, "rect" | "image" | "foreignObject")
        }
        "cx" | "cy" => matches!(element, "circle" | "ellipse"),
        "r" => element == "circle",
        "rx" | "ry" => matches!(element, "rect" | "ellipse"),
        _ => false,
    }
}

/// Pruebas de procesamiento condicional (§5.7): no soportamos extensiones salvo XHTML, y el
/// idioma del usuario es inglés.
fn conditions_pass(n: &DocNode) -> bool {
    if let Some(ext) = n.attr("requiredExtensions") {
        let list: Vec<&str> = ext.split_ascii_whitespace().collect();
        if list.is_empty() || list.iter().any(|e| *e != "http://www.w3.org/1999/xhtml") {
            return false;
        }
    }
    if let Some(langs) = n.attr("systemLanguage") {
        let ok = langs.split(',').map(str::trim).any(|l| {
            let l = l.to_ascii_lowercase();
            l == "en" || l.starts_with("en-")
        });
        if !ok {
            return false;
        }
    }
    true
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
        // Sin métricas de la fuente aquí: `ch` vale 0,5em (CSS Values 4 §6.1.1, el valor por
        // defecto cuando no se puede medir).
        "ch" => Some(Len::Em(v * 0.5)),
        "vw" | "svw" | "lvw" | "dvw" | "vi" | "svi" | "lvi" | "dvi" => Some(Len::Vw(v)),
        "vh" | "svh" | "lvh" | "dvh" | "vb" | "svb" | "lvb" | "dvb" => Some(Len::Vh(v)),
        "vmin" | "svmin" | "lvmin" | "dvmin" => Some(Len::Vmin(v)),
        "vmax" | "svmax" | "lvmax" | "dvmax" => Some(Len::Vmax(v)),
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
    if let ComponentValue::Function(f) = c
        && f.name.eq_ignore_ascii_case("calc")
    {
        let (a, b, c) = calc(&f.value)?.lin();
        return Some(Len::Calc(a, b, c));
    }
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
    /// `context-fill`/`context-stroke` (SVG 2 §13.3).
    Context(ContextPaint),
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

fn parse_paint(v: &[ComponentValue]) -> Option<PaintSpec> {
    let t = toks(v);
    let simple = |c: &ComponentValue| -> Option<PaintSpec> {
        if let Some(TokenKind::Ident(s)) = c.token()
            && s.eq_ignore_ascii_case("none")
        {
            return Some(PaintSpec::None);
        }
        // `context-fill`/`context-stroke`: la pintura del `use` o de la forma del marcador.
        if let Some(TokenKind::Ident(s)) = c.token() {
            if s.eq_ignore_ascii_case("context-fill") {
                return Some(PaintSpec::Context(ContextPaint::Fill));
            }
            if s.eq_ignore_ascii_case("context-stroke") {
                return Some(PaintSpec::Context(ContextPaint::Stroke));
            }
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
    word_spacing: f32,
    /// `dominant-baseline` (SVG 2 §11.10.2.1, heredada).
    dominant_baseline: crate::Baseline,
    /// `marker-start`, `marker-mid` y `marker-end` (URLs).
    markers: [Option<String>; 3],
    /// `paint-order` (SVG 2 §13.7.3): 0 relleno, 1 trazo, 2 marcadores.
    paint_order: [u8; 3],
    /// `line-height` computado (CSS Inline 3 §5.1; heredado).
    line_height: LineHeight,
}

/// Valor computado de `line-height`: un número se hereda como número y una longitud, ya
/// absoluta.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum LineHeight {
    #[default]
    Normal,
    Factor(f32),
    Px(f32),
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
            word_spacing: 0.0,
            dominant_baseline: crate::Baseline::Alphabetic,
            markers: [None, None, None],
            paint_order: [0, 1, 2],
            line_height: LineHeight::Normal,
        }
    }
}

/// Un viewport: tamaño para los porcentajes.
#[derive(Clone, Copy)]
struct Viewport {
    w: f32,
    h: f32,
    /// Tamaño del viewport inicial, para `vw`/`vh`: el del documento SVG (el tamaño con el que
    /// se pinta; en un SVG usado como imagen, el de la imagen).
    icb: (f32, f32),
}

impl Viewport {
    fn resolve(&self, l: Len, axis: Axis, font: f32) -> f32 {
        match l {
            Len::Px(v) => v,
            Len::Em(v) => v * font,
            Len::Ex(v) => v * font * 0.5,
            Len::Vw(v) => v * self.icb.0 / 100.0,
            Len::Vh(v) => v * self.icb.1 / 100.0,
            Len::Vmin(v) => v * self.icb.0.min(self.icb.1) / 100.0,
            Len::Vmax(v) => v * self.icb.0.max(self.icb.1) / 100.0,
            Len::Calc(px, pct, em) => px + em * font + self.resolve(Len::Pct(pct), axis, font),
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
    /// Relleno y trazo especificados de los elementos de contexto en curso (SVG 2 §13.3), del
    /// exterior al interior; ya sin `context-*` (se resuelven al apilarlos).
    context: Vec<[PaintSpec; 2]>,
}

fn is_inherited_reset(name: &str) -> bool {
    matches!(name, "inherit")
}

impl<'a> Builder<'a> {
    fn node(&self, i: usize) -> &'a DocNode {
        &self.svg.nodes[i]
    }

    /// El elemento de una referencia: `#id` local o de un documento externo unido.
    fn by_id(&self, url: &str) -> Option<usize> {
        self.svg.lookup(url)
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
                "marker-start" | "marker-mid" | "marker-end" | "marker" => {
                    let url = if id.as_deref() == Some("none") {
                        None
                    } else {
                        match one(v).and_then(url_of) {
                            Some(u) => Some(u),
                            None => continue,
                        }
                    };
                    match name.as_str() {
                        "marker-start" => st.markers[0] = url,
                        "marker-mid" => st.markers[1] = url,
                        "marker-end" => st.markers[2] = url,
                        _ => st.markers = [url.clone(), url.clone(), url],
                    }
                }
                "paint-order" => {
                    let parts = toks(v);
                    let words: Vec<String> = parts
                        .iter()
                        .filter_map(|c| match c.token()? {
                            TokenKind::Ident(s) => Some(s.to_ascii_lowercase()),
                            _ => None,
                        })
                        .collect();
                    if words.len() != parts.len() || words.is_empty() || words.len() > 3 {
                        continue;
                    }
                    if words.len() == 1 && words[0] == "normal" {
                        st.paint_order = [0, 1, 2];
                        continue;
                    }
                    let mut order: Vec<u8> = Vec::new();
                    for w in &words {
                        let k = match w.as_str() {
                            "fill" => 0,
                            "stroke" => 1,
                            "markers" => 2,
                            _ => break,
                        };
                        if order.contains(&k) {
                            break;
                        }
                        order.push(k);
                    }
                    if order.len() != words.len() {
                        continue;
                    }
                    // Los que falten van detrás, en el orden por defecto.
                    for k in 0..3 {
                        if !order.contains(&k) {
                            order.push(k);
                        }
                    }
                    st.paint_order = [order[0], order[1], order[2]];
                }
                // CSS Text 3 §8.1–§8.2 (los porcentajes, CSS Text 4: del tamaño de la fuente).
                "letter-spacing" | "word-spacing" => {
                    let space = match id.as_deref() {
                        Some("normal") => 0.0,
                        _ => match one(v).and_then(cv_len) {
                            Some(Len::Pct(p)) => st.size * p / 100.0,
                            Some(l) => vp.resolve(l, Axis::Other, st.size),
                            None => continue,
                        },
                    };
                    if name == "letter-spacing" {
                        st.letter_spacing = space;
                    } else {
                        st.word_spacing = space;
                    }
                }
                "line-height" => {
                    st.line_height = match id.as_deref() {
                        Some("normal") => LineHeight::Normal,
                        _ => match one(v).and_then(|c| c.token().cloned()) {
                            Some(TokenKind::Number(n)) if n.value >= 0.0 => {
                                LineHeight::Factor(n.value as f32)
                            }
                            Some(TokenKind::Percentage(p)) if p.value >= 0.0 => {
                                LineHeight::Px(st.size * p.value as f32 / 100.0)
                            }
                            _ => match one(v).and_then(cv_len) {
                                Some(l @ (Len::Px(_) | Len::Em(_) | Len::Ex(_))) => {
                                    LineHeight::Px(vp.resolve(l, Axis::Other, st.size).max(0.0))
                                }
                                _ => continue,
                            },
                        },
                    };
                }
                "dominant-baseline" => {
                    if let Some(b) = id.as_deref().and_then(text::baseline_keyword) {
                        st.dominant_baseline = b;
                    } else if id.as_deref() == Some("auto") {
                        st.dominant_baseline = crate::Baseline::Alphabetic;
                    }
                }
                _ => {}
            }
        }
        st
    }

    fn len_attr(&self, n: &DocNode, name: &str) -> Option<Len> {
        if geometry_property(&n.name, name)
            && let Some(v) = n.props.get(name)
            && let Some(l) = one(v).and_then(cv_len)
        {
            return Some(l);
        }
        let a = n.attr(name)?;
        parse_len(a).or_else(|| one(&parse_component_value_list(a)).and_then(cv_len))
    }

    fn len(&self, n: &DocNode, name: &str, axis: Axis, vp: Viewport, st: &St) -> Option<f32> {
        self.len_attr(n, name).map(|l| vp.resolve(l, axis, st.size))
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

    /// La transformación de un elemento: la propiedad `transform` de CSS si está (si no, el
    /// atributo), alrededor de `transform-origin` en su `transform-box` (CSS Transforms 1 §6).
    fn element_transform(&self, n: &DocNode, node: &Node, vp: Viewport, st: &St) -> Transform {
        let base = n
            .props
            .get("transform")
            .filter(|v| !v.is_empty())
            .and_then(|v| css_transform(v, vp, st.size))
            .unwrap_or_else(|| Self::transform(n));
        let Some(origin) = n.props.get("transform-origin").filter(|v| !v.is_empty()) else {
            return base;
        };
        if base.is_identity() {
            return base;
        }
        let fill_box = n
            .props
            .get("transform-box")
            .and_then(|v| ident(v))
            .is_some_and(|b| {
                b == "fill-box" || b == "content-box" || b == "stroke-box" || b == "border-box"
            });
        let (rx, ry, rw, rh) = if fill_box {
            node_bbox(node, &Transform::IDENTITY).unwrap_or((0.0, 0.0, 0.0, 0.0))
        } else {
            (0.0, 0.0, vp.w, vp.h)
        };
        let parts = toks(origin);
        let mut ox = None;
        let mut oy = None;
        for (k, c) in parts.iter().take(2).enumerate() {
            let key = c.token().and_then(|t| match t {
                TokenKind::Ident(s) => Some(s.to_ascii_lowercase()),
                _ => None,
            });
            match key.as_deref() {
                Some("left") => ox = Some(0.0),
                Some("right") => ox = Some(rw),
                Some("top") => oy = Some(0.0),
                Some("bottom") => oy = Some(rh),
                Some("center") => {
                    if k == 0 && ox.is_none() {
                        ox = Some(rw / 2.0);
                    } else {
                        oy = Some(rh / 2.0);
                    }
                }
                _ => {
                    let l = cv_len(c);
                    let v = l.map(|l| {
                        let size = if k == 0 { rw } else { rh };
                        Viewport {
                            w: size,
                            h: size,
                            icb: vp.icb,
                        }
                        .resolve(l, Axis::X, st.size)
                    });
                    if k == 0 {
                        ox = v;
                    } else {
                        oy = v;
                    }
                }
            }
        }
        let (ox, oy) = (rx + ox.unwrap_or(rw / 2.0), ry + oy.unwrap_or(rh / 2.0));
        Transform::translate(ox, oy)
            .then(&base)
            .then(&Transform::translate(-ox, -oy))
    }

    /// Un elemento renderizable (o `None`).
    fn element(&mut self, i: usize, parent: &St, vp: Viewport) -> Option<Node> {
        if self.stack.len() > MAX_DEPTH * 4 || self.stack.contains(&i) {
            return None;
        }
        let n = self.node(i);
        if n.is_text() || Self::display_none(n) || !conditions_pass(n) {
            return None;
        }
        let st = self.computed(n, parent, vp);
        self.stack.push(i);
        let content = self.content(i, &st, vp);
        self.stack.pop();
        let n = self.node(i);
        let mut node = content?;
        let transform = self.element_transform(n, &node, vp, &st);
        let opacity = Self::opacity(n);
        let clip = n
            .props
            .get("clip-path")
            .and_then(|v| one(v).and_then(url_of))
            .and_then(|u| self.by_id(&u))
            .filter(|&c| self.node(c).name == "clipPath")
            .map(|c| (c, st.clone()));
        let mask = n
            .props
            .get("mask")
            .and_then(|v| one(v).and_then(url_of))
            .and_then(|u| self.by_id(&u))
            .filter(|&m| self.node(m).name == "mask");
        let filter = self.filter_ref(n);
        if opacity >= 1.0 && clip.is_none() && mask.is_none() && filter.is_none() {
            match &mut node {
                Node::Path(p) => {
                    p.transform = transform.then(&p.transform);
                    return Some(node);
                }
                Node::Text(t) => {
                    t.transform = transform.then(&t.transform);
                    return Some(node);
                }
                Node::Image(im) => {
                    im.transform = transform.then(&im.transform);
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
        let mask = match mask {
            Some(m) => {
                let bbox = node_bbox(&node, &Transform::IDENTITY);
                Some(Box::new(self.mask(m, vp, bbox)?))
            }
            None => None,
        };
        // Filter Effects 1 §9.2: sin primitivas o con una región vacía, el elemento no se
        // pinta.
        let filter = match filter {
            Some(f) => {
                let bbox = node_bbox(&node, &Transform::IDENTITY);
                Some(Box::new(self.filter(f, vp, bbox)?))
            }
            None => None,
        };
        Some(Node::Group(Group {
            host_units: false,
            context: None,
            transform,
            opacity,
            clip,
            mask,
            filter,
            children: vec![node],
        }))
    }

    /// Estilo base de un recurso (`marker`, `pattern`, `mask`): sus propiedades heredan de sus
    /// ancestros, no del elemento que lo usa; aproximamos con lo heredado del documento.
    fn resource_st(&self) -> St {
        St {
            color: self.svg.inherited.color,
            size: self.svg.inherited.font_size,
            family: self.svg.inherited.font_family.clone(),
            ..St::default()
        }
    }

    /// Un `x`/`y`/`width`/`height` de un recurso en `objectBoundingBox`: fracción (o
    /// porcentaje) de la caja.
    fn bbox_fraction(a: Option<&str>, default: f32) -> f32 {
        match a.and_then(parse_len) {
            Some(Len::Pct(p)) => p / 100.0,
            Some(Len::Px(v)) => v,
            _ => default,
        }
    }

    /// `mask` referenciada por un elemento con caja `bbox` (CSS Masking 1 §7.1). Si la región
    /// queda vacía, una máscara vacía: el elemento no se ve.
    fn mask(&mut self, m: usize, vp: Viewport, bbox: Option<Bbox>) -> Option<Mask> {
        let empty = || Mask {
            luminance: true,
            rect: (0.0, 0.0, 0.0, 0.0),
            content: Group::default(),
        };
        if self.stack.contains(&m) {
            return Some(empty());
        }
        let mn = self.node(m);
        let user_units = mn
            .attr("maskUnits")
            .is_some_and(|u| u.trim() == "userSpaceOnUse");
        let content_bbox = mn
            .attr("maskContentUnits")
            .is_some_and(|u| u.trim() == "objectBoundingBox");
        let mst = self.computed(mn, &self.resource_st(), vp);
        let rect = if user_units {
            let get = |name: &str, def: &str, axis: Axis| {
                self.len(mn, name, axis, vp, &mst)
                    .or_else(|| parse_len(def).map(|l| vp.resolve(l, axis, mst.size)))
                    .unwrap_or(0.0)
            };
            (
                get("x", "-10%", Axis::X),
                get("y", "-10%", Axis::Y),
                get("width", "120%", Axis::X),
                get("height", "120%", Axis::Y),
            )
        } else {
            let Some((bx, by, bw, bh)) = bbox.filter(|b| b.2 > 0.0 && b.3 > 0.0) else {
                return Some(empty());
            };
            (
                bx + Self::bbox_fraction(mn.attr("x"), -0.1) * bw,
                by + Self::bbox_fraction(mn.attr("y"), -0.1) * bh,
                Self::bbox_fraction(mn.attr("width"), 1.2) * bw,
                Self::bbox_fraction(mn.attr("height"), 1.2) * bh,
            )
        };
        if rect.2 <= 0.0 || rect.3 <= 0.0 {
            return Some(empty());
        }
        let content_t = match (content_bbox, bbox) {
            (true, Some((bx, by, bw, bh))) => {
                Transform::translate(bx, by).then(&Transform::scale(bw, bh))
            }
            (true, None) => return Some(empty()),
            (false, _) => Transform::IDENTITY,
        };
        let luminance = mn
            .props
            .get("mask-type")
            .and_then(|v| ident(v))
            .is_none_or(|t| t != "alpha");
        self.stack.push(m);
        let children = self.children(m, &mst, vp);
        self.stack.pop();
        Some(Mask {
            luminance,
            rect,
            content: Group {
                host_units: false,
                context: None,
                transform: content_t,
                opacity: 1.0,
                clip: None,
                mask: None,
                filter: None,
                children,
            },
        })
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
                    if cn.is_text() || !conditions_pass(cn) {
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
                // Un radio negativo es inválido y vale `auto` (SVG 2 §10.4): toma el otro.
                let rx = self.len(n, "rx", Axis::X, vp, st).filter(|v| *v >= 0.0);
                let ry = self.len(n, "ry", Axis::Y, vp, st).filter(|v| *v >= 0.0);
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
            "foreignObject" => {
                // SVG 2 §23.2: el contenido es de otro namespace (HTML) y lo maqueta quien
                // tiene el documento; aquí queda como una imagen del tamaño del elemento,
                // recortada a él (`overflow: hidden` en la hoja UA). Sin DOM de origen (un SVG
                // cargado como imagen) no se pinta.
                if !st.visible {
                    return None;
                }
                let dom = n.dom?;
                let x = self.len(n, "x", Axis::X, vp, st).unwrap_or(0.0);
                let y = self.len(n, "y", Axis::Y, vp, st).unwrap_or(0.0);
                let w = self.len(n, "width", Axis::X, vp, st)?;
                let h = self.len(n, "height", Axis::Y, vp, st)?;
                if w <= 0.0 || h <= 0.0 {
                    return None;
                }
                return Some(Node::Image(crate::Image {
                    href: format!("{FOREIGN_OBJECT_SCHEME}{dom}"),
                    x,
                    y,
                    width: Some(w),
                    height: Some(h),
                    par: "none".into(),
                    overflow_visible: n
                        .props
                        .get("overflow")
                        .and_then(|v| ident(v))
                        .is_some_and(|o| o == "visible"),
                    transform: Transform::IDENTITY,
                }));
            }
            "image" => {
                if !st.visible {
                    return None;
                }
                let href = n.attr("href").map(str::trim).filter(|h| !h.is_empty())?;
                let x = self.len(n, "x", Axis::X, vp, st).unwrap_or(0.0);
                let y = self.len(n, "y", Axis::Y, vp, st).unwrap_or(0.0);
                let width = self.len(n, "width", Axis::X, vp, st);
                let height = self.len(n, "height", Axis::Y, vp, st);
                // Un ancho o un alto cero (o negativo) desactiva el renderizado (§12.3.2).
                if width.is_some_and(|w| w <= 0.0) || height.is_some_and(|h| h <= 0.0) {
                    return None;
                }
                return Some(Node::Image(crate::Image {
                    href: href.to_owned(),
                    x,
                    y,
                    width,
                    height,
                    par: n.attr("preserveAspectRatio").unwrap_or("").to_owned(),
                    overflow_visible: false,
                    transform: Transform::IDENTITY,
                }));
            }
            _ => return None,
        };
        let segs = shape(segs)?;
        if !st.visible {
            return None;
        }
        let bbox = geom::bbox(&segs);
        let non_scaling = n
            .props
            .get("vector-effect")
            .and_then(|v| ident(v))
            .is_some_and(|v| v == "non-scaling-stroke");
        let has_markers = st.markers.iter().any(Option::is_some)
            && matches!(n.name.as_str(), "path" | "line" | "polyline" | "polygon");
        let fill = self.fill(st, bbox, vp);
        let mut stroke = self.stroke(st, bbox, vp);
        let markers = if has_markers {
            // La forma es el elemento de contexto de sus marcadores (SVG 2 §13.3).
            let context = ContextPaints {
                fill: fill.as_ref().map(|f| f.paint.clone()),
                stroke: stroke.as_ref().map(|s| s.paint.clone()),
            };
            self.markers(&segs, st, vp, non_scaling, context)
        } else {
            Vec::new()
        };
        let n = self.node(i);
        if let Some(s) = &mut stroke {
            // `pathLength` (§9.5.1): las distancias del trazo se escalan por longitud real /
            // `pathLength`; un valor negativo es un error (se ignora) y cero escala a infinito.
            if let Some(pl) = n
                .attr("pathLength")
                .and_then(|v| v.trim().parse::<f32>().ok())
                .filter(|v| *v >= 0.0 && v.is_finite())
                && !s.dasharray.is_empty()
            {
                if pl == 0.0 {
                    if s.dasharray[0] > 0.0 {
                        s.dasharray.clear();
                    } else {
                        for d in &mut s.dasharray {
                            *d = if *d > 0.0 { 1e9 } else { 0.0 };
                        }
                        s.dashoffset = 0.0;
                    }
                } else {
                    let k = geom::length(&segs) / pl;
                    for d in &mut s.dasharray {
                        *d *= k;
                    }
                    s.dashoffset *= k;
                }
            }
            s.non_scaling = non_scaling;
        }
        let default_order = st.paint_order == [0, 1, 2];
        if markers.is_empty() && (default_order || fill.is_none() || stroke.is_none()) {
            return Some(Node::Path(Path {
                fill,
                stroke,
                segs,
                transform: Transform::IDENTITY,
            }));
        }
        // `paint-order`: relleno, trazo y marcadores como nodos aparte, en su orden.
        let (mut fill, mut stroke, mut markers) = (fill, stroke, Some(markers));
        let mut children = Vec::new();
        for k in st.paint_order {
            match k {
                0 => {
                    if let Some(f) = fill.take() {
                        children.push(Node::Path(Path {
                            fill: Some(f),
                            stroke: None,
                            segs: segs.clone(),
                            transform: Transform::IDENTITY,
                        }));
                    }
                }
                1 => {
                    if let Some(s) = stroke.take() {
                        children.push(Node::Path(Path {
                            fill: None,
                            stroke: Some(s),
                            segs: segs.clone(),
                            transform: Transform::IDENTITY,
                        }));
                    }
                }
                _ => children.extend(markers.take().unwrap_or_default()),
            }
        }
        Some(Node::Group(Group {
            opacity: 1.0,
            children,
            ..Group::default()
        }))
    }

    /// Los marcadores de un trazado (§13.7): uno por vértice, orientados según `orient`.
    fn markers(
        &mut self,
        segs: &[Seg],
        st: &St,
        vp: Viewport,
        non_scaling: bool,
        context: ContextPaints,
    ) -> Vec<Node> {
        let verts = vertices(segs);
        let mut out = Vec::new();
        let last = verts.len().saturating_sub(1);
        for (k, v) in verts.iter().enumerate() {
            let kind = if k == 0 {
                0
            } else if k == last {
                2
            } else {
                1
            };
            let Some(url) = &st.markers[kind] else {
                continue;
            };
            let Some(m) = self.by_id(url).filter(|&m| self.node(m).name == "marker") else {
                continue;
            };
            self.push_context(st);
            let node = self.marker(m, v, kind == 0, st, vp, non_scaling);
            self.context.pop();
            if let Some(node) = node {
                out.push(Node::Group(Group {
                    context: Some(Box::new(context.clone())),
                    opacity: 1.0,
                    children: vec![node],
                    ..Group::default()
                }));
            }
        }
        out
    }

    fn marker(
        &mut self,
        m: usize,
        v: &Vertex,
        start: bool,
        st: &St,
        vp: Viewport,
        non_scaling: bool,
    ) -> Option<Node> {
        if self.stack.contains(&m) {
            return None;
        }
        let mn = self.node(m);
        let mst = self.computed(mn, &self.resource_st(), vp);
        let mw = self
            .len(mn, "markerWidth", Axis::X, vp, &mst)
            .unwrap_or(3.0);
        let mh = self
            .len(mn, "markerHeight", Axis::Y, vp, &mst)
            .unwrap_or(3.0);
        if mw <= 0.0 || mh <= 0.0 {
            return None;
        }
        let angle = match mn.attr("orient").map(str::trim) {
            Some("auto") => v.angle(),
            Some("auto-start-reverse") => v.angle() + if start { 180.0 } else { 0.0 },
            // `<angle>` o número en grados (SVG 2 §13.7.2; CSS Values 4 §7.1).
            Some(a) => {
                let a = a.to_ascii_lowercase();
                let (num, k) = [
                    ("deg", 1.0),
                    ("grad", 0.9),
                    ("rad", 180.0 / std::f32::consts::PI),
                    ("turn", 360.0),
                ]
                .iter()
                .find_map(|&(u, k)| Some((a.strip_suffix(u)?, k)))
                .unwrap_or((a.as_str(), 1.0));
                num.trim().parse::<f32>().map_or(0.0, |v| v * k)
            }
            None => 0.0,
        };
        let stroke_units = !mn
            .attr("markerUnits")
            .is_some_and(|u| u.trim() == "userSpaceOnUse");
        let scale = if stroke_units { st.stroke_width } else { 1.0 };
        let vb = view_box_state(mn).ok()?;
        let par = mn.attr("preserveAspectRatio").unwrap_or("");
        let vb_t = vb.map_or(Transform::IDENTITY, |vb| {
            view_box_transform(vb, par, (mw, mh))
        });
        // `refX`/`refY` están en el sistema del contenido (el del `viewBox`).
        let rx = self.len(mn, "refX", Axis::X, vp, &mst).unwrap_or(0.0);
        let ry = self.len(mn, "refY", Axis::Y, vp, &mst).unwrap_or(0.0);
        let (rx, ry) = vb_t.apply(rx, ry);
        let inner_vp = match vb {
            Some((_, _, w, h)) => Viewport { w, h, icb: vp.icb },
            None => Viewport {
                w: mw,
                h: mh,
                icb: vp.icb,
            },
        };
        self.stack.push(m);
        let children = self.children(m, &mst, inner_vp);
        self.stack.pop();
        let overflow_visible = mn
            .props
            .get("overflow")
            .and_then(|v| ident(v))
            .is_some_and(|o| o == "visible" || o == "auto");
        let clip = (!overflow_visible).then(|| ClipPath {
            transform: Transform::IDENTITY,
            shapes: vec![(
                geom::rect(0.0, 0.0, mw, mh, 0.0, 0.0),
                FillRule::NonZero,
                Transform::IDENTITY,
            )],
            texts: Vec::new(),
            clip: None,
        });
        // Con `non-scaling-stroke` el ancho del trazo está en px del host: el marcador se
        // escala en el host, desde su vértice.
        let host_units = stroke_units && non_scaling;
        let place = Transform::rotate(angle)
            .then(&Transform::scale(scale, scale))
            .then(&Transform::translate(-rx, -ry));
        let transform = if host_units {
            place
        } else {
            Transform::translate(v.x, v.y).then(&place)
        };
        let node = Node::Group(Group {
            host_units: false,
            context: None,
            transform,
            opacity: Self::opacity(mn),
            clip,
            mask: None,
            filter: None,
            children: vec![Node::Group(Group {
                host_units: false,
                context: None,
                transform: vb_t,
                opacity: 1.0,
                clip: None,
                mask: None,
                filter: None,
                children,
            })],
        });
        if !host_units {
            return Some(node);
        }
        Some(Node::Group(Group {
            transform: Transform::translate(v.x, v.y),
            opacity: 1.0,
            clip: None,
            mask: None,
            filter: None,
            host_units: true,
            context: None,
            children: vec![node],
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
        let view_box = match view_box_state(c) {
            Err(()) => return None,
            Ok(vb) => vb,
        };
        let inner_vp = match view_box {
            Some((_, _, vw, vh)) => Viewport {
                w: vw,
                h: vh,
                icb: vp.icb,
            },
            None => Viewport { w, h, icb: vp.icb },
        };
        let par = c.attr("preserveAspectRatio").unwrap_or("");
        let vb_t = view_box.map_or(Transform::IDENTITY, |vb| {
            view_box_transform(vb, par, (w, h))
        });
        let children = self.children(content, st, inner_vp);
        let overflow_visible = c
            .props
            .get("overflow")
            .and_then(|v| ident(v))
            .is_some_and(|o| o == "visible" || o == "auto");
        let clip = (!overflow_visible).then(|| ClipPath {
            transform: Transform::IDENTITY,
            shapes: vec![(
                geom::rect(0.0, 0.0, w, h, 0.0, 0.0),
                FillRule::NonZero,
                Transform::IDENTITY,
            )],
            texts: Vec::new(),
            clip: None,
        });
        Some(Node::Group(Group {
            host_units: false,
            context: None,
            transform: Transform::translate(x, y),
            opacity: 1.0,
            clip,
            mask: None,
            filter: None,
            children: vec![Node::Group(Group {
                host_units: false,
                context: None,
                transform: vb_t,
                opacity: 1.0,
                clip: None,
                mask: None,
                filter: None,
                children,
            })],
        }))
    }

    fn use_element(&mut self, i: usize, st: &St, vp: Viewport) -> Option<Node> {
        let n = self.node(i);
        let original = n
            .attr("href")
            .and_then(|h| self.by_id(h))
            .filter(|&t| !self.stack.contains(&t))?;
        // El clon del shadow tree (con la cascada de un árbol aparte), si lo hay.
        let target = self.svg.shadows.get(&original).copied().unwrap_or(original);
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
                self.push_context(st);
                let node = self.nested_svg(target, target, &tst, vp, Some((w, h)));
                self.context.pop();
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
            _ => {
                self.stack.push(original);
                self.push_context(st);
                let node = self.element(target, st, vp);
                self.context.pop();
                self.stack.pop();
                node?
            }
        };
        let placed = Node::Group(Group {
            host_units: false,
            context: None,
            transform: Transform::translate(x, y),
            opacity: 1.0,
            clip: None,
            mask: None,
            filter: None,
            children: vec![inner],
        });
        // El `use` es el elemento de contexto de su contenido (SVG 2 §13.3): sus pinturas,
        // con su caja, en su sistema (sin el `x`/`y`, que solo mueve el contenido).
        let bbox = node_bbox(&placed, &Transform::IDENTITY);
        let context = self.context_paints(st, bbox, vp);
        Some(Node::Group(Group {
            context: Some(Box::new(context)),
            opacity: 1.0,
            children: vec![placed],
            ..Group::default()
        }))
    }

    /// Apila el relleno y el trazo especificados de un elemento de contexto, con sus
    /// `context-*` resueltos con el contexto exterior.
    fn push_context(&mut self, st: &St) {
        let outer = self.context.last().cloned();
        let own = |p: &PaintSpec| match p {
            PaintSpec::Context(k) => outer
                .as_ref()
                .map_or(PaintSpec::None, |o| o[*k as usize].clone()),
            p => p.clone(),
        };
        let entry = [own(&st.fill), own(&st.stroke)];
        self.context.push(entry);
    }

    /// Las pinturas de un elemento de contexto ya pintado, con la caja `bbox` (en el sistema
    /// del grupo que las llevará).
    fn context_paints(&mut self, st: &St, bbox: Option<Bbox>, vp: Viewport) -> ContextPaints {
        ContextPaints {
            fill: self.resolve_paint(&st.fill, st, bbox, vp),
            stroke: self.resolve_paint(&st.stroke, st, bbox, vp),
        }
    }

    fn resolve_paint(
        &mut self,
        spec: &PaintSpec,
        st: &St,
        bbox: Option<Bbox>,
        vp: Viewport,
    ) -> Option<Paint> {
        match spec {
            PaintSpec::None => None,
            PaintSpec::Color(c) => Some(Paint::Color(*c)),
            PaintSpec::CurrentColor => Some(Paint::Color(st.color)),
            // SVG 2 §13.3: sin elemento de contexto no hay pintura. `currentColor` se resuelve
            // con el `color` del elemento que la usa (WPT `paint-context-003.svg`); los
            // servidores de pintura, en el sistema del contexto (ver `Paint::Context`).
            PaintSpec::Context(k) => match &self.context.last()?[*k as usize] {
                PaintSpec::None => None,
                PaintSpec::Color(c) => Some(Paint::Color(*c)),
                PaintSpec::CurrentColor => Some(Paint::Color(st.color)),
                PaintSpec::Url(..) => Some(Paint::Context(*k)),
                PaintSpec::Context(_) => None,
            },
            PaintSpec::Url(url, fallback) => {
                let server = self.by_id(url).filter(|&s| {
                    matches!(
                        self.node(s).name.as_str(),
                        "linearGradient" | "radialGradient" | "pattern"
                    )
                });
                let result = match server {
                    Some(s) if self.node(s).name == "pattern" => Some(self.pattern(s, bbox, vp)),
                    Some(s) => Some(self.gradient(s, bbox, vp, st)),
                    None => None,
                };
                match result {
                    Some(Ok(paint)) => paint,
                    // Referencia inválida (o transformación no invertible): el respaldo, o nada
                    // (§13.2).
                    Some(Err(())) | None => fallback
                        .as_deref()
                        .and_then(|f| self.resolve_paint(f, st, bbox, vp)),
                }
            }
        }
    }

    fn fill(&mut self, st: &St, bbox: Option<Bbox>, vp: Viewport) -> Option<Fill> {
        Some(Fill {
            paint: self.resolve_paint(&st.fill, st, bbox, vp)?,
            opacity: st.fill_opacity,
            rule: st.fill_rule,
        })
    }

    fn stroke(&mut self, st: &St, bbox: Option<Bbox>, vp: Viewport) -> Option<Stroke> {
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
            non_scaling: false,
        })
    }

    /// `patternTransform`/`gradientTransform` de una cadena de `href`: el primero que esté
    /// especificado. Un valor vacío o inválido cuenta como no especificado y no corta la herencia
    /// (SVG 2 §14.4.1: «if this element has no defined … attribute»; WPT
    /// `pservers/reftests/pattern-transform-empty-attr.svg`).
    fn chain_transform(&self, chain: &[usize], name: &str) -> Transform {
        chain
            .iter()
            .filter_map(|&c| self.node(c).attr(name))
            .filter(|v| !v.trim().is_empty())
            .find_map(geom::parse_transform)
            .unwrap_or_default()
    }

    /// Cadena de `href` de un `pattern` (él primero, SVG 2 §14.4.1).
    fn pattern_chain(&self, s: usize) -> Vec<usize> {
        let mut out = vec![s];
        while out.len() < MAX_DEPTH {
            let last = self.node(out[out.len() - 1]);
            match last.attr("href").and_then(|h| self.by_id(h)) {
                Some(next) if !out.contains(&next) && self.node(next).name == "pattern" => {
                    out.push(next)
                }
                _ => break,
            }
        }
        out
    }

    /// `pattern` (SVG 2 §14.4): `Err` si es una referencia inválida (transformación no
    /// invertible o ciclo), `Ok(None)` si no pinta nada (tesela vacía o `viewBox` nulo).
    fn pattern(&mut self, s: usize, bbox: Option<Bbox>, vp: Viewport) -> Result<Option<Paint>, ()> {
        if self.stack.contains(&s) {
            return Err(());
        }
        let chain = self.pattern_chain(s);
        let attr = |name: &str| chain.iter().find_map(|&c| self.node(c).attr(name));
        let pt = self.chain_transform(&chain, "patternTransform");
        if !pt.is_invertible() {
            return Err(());
        }
        let bbox_units = attr("patternUnits").is_none_or(|u| u.trim() != "userSpaceOnUse");
        let content_bbox =
            attr("patternContentUnits").is_some_and(|u| u.trim() == "objectBoundingBox");
        let pst = self.computed(self.node(s), &self.resource_st(), vp);
        let (x, y, w, h) = if bbox_units {
            let Some((bx, by, bw, bh)) = bbox.filter(|b| b.2 > 0.0 && b.3 > 0.0) else {
                return Ok(None);
            };
            (
                bx + Self::bbox_fraction(attr("x"), 0.0) * bw,
                by + Self::bbox_fraction(attr("y"), 0.0) * bh,
                Self::bbox_fraction(attr("width"), 0.0) * bw,
                Self::bbox_fraction(attr("height"), 0.0) * bh,
            )
        } else {
            let get = |name: &str, axis: Axis| {
                attr(name)
                    .and_then(parse_len)
                    .map_or(0.0, |l| vp.resolve(l, axis, pst.size))
            };
            (
                get("x", Axis::X),
                get("y", Axis::Y),
                get("width", Axis::X),
                get("height", Axis::Y),
            )
        };
        if w <= 0.0 || h <= 0.0 {
            return Ok(None);
        }
        let mut view_box = None;
        for &c in &chain {
            match view_box_state(self.node(c)) {
                Err(()) => return Ok(None),
                Ok(Some(vb)) => {
                    view_box = Some(vb);
                    break;
                }
                Ok(None) => {}
            }
        }
        let content_t = match (view_box, content_bbox, bbox) {
            (Some(vb), _, _) => {
                view_box_transform(vb, attr("preserveAspectRatio").unwrap_or(""), (w, h))
            }
            (None, true, Some((_, _, bw, bh))) => Transform::scale(bw, bh),
            (None, true, None) => return Ok(None),
            (None, false, _) => Transform::IDENTITY,
        };
        let inner_vp = match view_box {
            Some((_, _, vw, vh)) => Viewport {
                w: vw,
                h: vh,
                icb: vp.icb,
            },
            None => vp,
        };
        // El contenido es el del primero de la cadena que tenga hijos elemento.
        let content = chain
            .iter()
            .copied()
            .find(|&c| {
                self.node(c)
                    .children
                    .iter()
                    .any(|&k| !self.node(k).is_text())
            })
            .unwrap_or(s);
        self.stack.push(s);
        let children = self.children(content, &pst, inner_vp);
        self.stack.pop();
        Ok(Some(Paint::Pattern(Box::new(Pattern {
            width: w,
            height: h,
            transform: pt.then(&Transform::translate(x, y)),
            content: Group {
                host_units: false,
                context: None,
                transform: content_t,
                opacity: 1.0,
                clip: None,
                mask: None,
                filter: None,
                children,
            },
        }))))
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

    /// `Err` si el gradiente no se puede usar (transformación no invertible): cuenta como una
    /// referencia inválida.
    fn gradient(
        &self,
        s: usize,
        bbox: Option<Bbox>,
        vp: Viewport,
        st: &St,
    ) -> Result<Option<Paint>, ()> {
        let gt = self.chain_transform(&self.chain(s), "gradientTransform");
        if !gt.is_invertible() {
            return Err(());
        }
        Ok(self.gradient_paint(s, bbox, vp, st))
    }

    fn gradient_paint(&self, s: usize, bbox: Option<Bbox>, vp: Viewport, st: &St) -> Option<Paint> {
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
        let gt = self.chain_transform(&chain, "gradientTransform");
        let coord = |name: &str, default: &str, axis: Axis| -> f32 {
            let l = attr(name)
                .and_then(parse_len)
                .or_else(|| parse_len(default));
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
    fn clip_path(
        &mut self,
        c: usize,
        st: &St,
        vp: Viewport,
        bbox: Option<Bbox>,
    ) -> Option<ClipPath> {
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
            clip: None,
        };
        if self.stack.contains(&c) {
            return Some(clip);
        }
        self.stack.push(c);
        // `clip-path` en el propio `clipPath` (CSS Masking 1 §6.3): con la misma caja y en el
        // sistema del elemento recortado, como este.
        if let Some(inner) = cn
            .props
            .get("clip-path")
            .and_then(|v| one(v).and_then(url_of))
            .and_then(|u| self.by_id(&u))
            .filter(|&k| self.node(k).name == "clipPath" && !self.stack.contains(&k))
        {
            let Some(nested) = self.clip_path(inner, st, vp, bbox) else {
                self.stack.pop();
                return None;
            };
            clip.clip = Some(Box::new(nested));
        }
        let kids = self.node(c).children.clone();
        for k in kids {
            let kn = self.node(k);
            if kn.is_text()
                || !matches!(
                    kn.name.as_str(),
                    "rect"
                        | "circle"
                        | "ellipse"
                        | "line"
                        | "polyline"
                        | "polygon"
                        | "path"
                        | "text"
                        | "use"
                )
            {
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

    /// Elemento `text` (SVG 2 §11): ver [`text`].
    fn text(&mut self, i: usize, st: &St, vp: Viewport) -> Option<Node> {
        text::build(self, i, st, vp)
    }
}

type Bbox = (f32, f32, f32, f32);

/// Un vértice de un trazado con las direcciones (en grados) de los segmentos que llegan y
/// salen (§13.7.4).
#[derive(Debug, Clone, Copy)]
struct Vertex {
    x: f32,
    y: f32,
    din: Option<f32>,
    dout: Option<f32>,
}

impl Vertex {
    /// Orientación de `orient="auto"`: la bisectriz de las dos direcciones, o la que haya.
    fn angle(&self) -> f32 {
        match (self.din, self.dout) {
            (Some(a), Some(b)) => {
                let mut d = b - a;
                while d > 180.0 {
                    d -= 360.0;
                }
                while d < -180.0 {
                    d += 360.0;
                }
                a + d / 2.0
            }
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => 0.0,
        }
    }
}

fn dir((x0, y0): (f32, f32), (x1, y1): (f32, f32)) -> Option<f32> {
    ((x0, y0) != (x1, y1)).then(|| (y1 - y0).atan2(x1 - x0).to_degrees())
}

/// Los vértices de un trazado, con las tangentes de los segmentos en sus extremos.
fn vertices(segs: &[Seg]) -> Vec<Vertex> {
    let mut out: Vec<Vertex> = Vec::new();
    let mut cur = (0.0, 0.0);
    let mut start = (0.0, 0.0);
    let mut start_index = 0;
    let push = |out: &mut Vec<Vertex>, p: (f32, f32), din: Option<f32>, dout: Option<f32>| {
        if let Some(last) = out.last_mut()
            && last.dout.is_none()
        {
            last.dout = dout;
        }
        out.push(Vertex {
            x: p.0,
            y: p.1,
            din,
            dout: None,
        });
    };
    for s in segs {
        match *s {
            Seg::MoveTo(x, y) => {
                out.push(Vertex {
                    x,
                    y,
                    din: None,
                    dout: None,
                });
                cur = (x, y);
                start = cur;
                start_index = out.len() - 1;
            }
            Seg::LineTo(x, y) => {
                let d = dir(cur, (x, y));
                push(&mut out, (x, y), d, d);
                cur = (x, y);
            }
            Seg::QuadTo(x1, y1, x, y) => {
                let d0 = dir(cur, (x1, y1)).or_else(|| dir(cur, (x, y)));
                let d1 = dir((x1, y1), (x, y)).or_else(|| dir(cur, (x, y)));
                push(&mut out, (x, y), d1, d0);
                cur = (x, y);
            }
            Seg::CubicTo(x1, y1, x2, y2, x, y) => {
                let d0 = dir(cur, (x1, y1))
                    .or_else(|| dir(cur, (x2, y2)))
                    .or_else(|| dir(cur, (x, y)));
                let d1 = dir((x2, y2), (x, y))
                    .or_else(|| dir((x1, y1), (x, y)))
                    .or_else(|| dir(cur, (x, y)));
                push(&mut out, (x, y), d1, d0);
                cur = (x, y);
            }
            Seg::Close => {
                let d = dir(cur, start);
                if d.is_some() {
                    push(&mut out, start, d, d);
                }
                // El vértice que cierra apunta como el principio del subtrazado, y este recibe
                // la dirección del cierre.
                let first_out = out.get(start_index).and_then(|v| v.dout);
                let close_in = out.last().and_then(|v| v.din);
                if let Some(last) = out.last_mut() {
                    last.dout = first_out;
                }
                if let Some(first) = out.get_mut(start_index) {
                    first.din = close_in;
                }
                cur = start;
            }
        }
    }
    out
}

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
        // Con `auto` el tamaño depende del recurso, que aquí no se conoce.
        Node::Image(im) => {
            let b = (im.x, im.y, im.width?, im.height?);
            Some(corners(b, &t.then(&im.transform)))
        }
    }
}

fn flatten_clip(node: &Node, t: &Transform, rule: crate::FillRule, out: &mut ClipPath) {
    match node {
        Node::Path(p) => out
            .shapes
            .push((p.segs.clone(), rule, t.then(&p.transform))),
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
        // `image` no es contenido válido de `clipPath` (§14.3.5).
        Node::Image(_) => {}
    }
}

/// Prefijo del recurso con el contenido pintado de un `foreignObject`.
pub(crate) const FOREIGN_OBJECT_SCHEME: &str = "foreign-object:";

fn view_box(n: &DocNode) -> Option<Bbox> {
    view_box_state(n).ok().flatten()
}

/// `viewBox` (§8.6): `Err` si su ancho o alto es cero o negativo (desactiva el render del
/// elemento); `Ok(None)` si falta o no se puede parsear.
fn view_box_state(n: &DocNode) -> Result<Option<Bbox>, ()> {
    let Some(v) = n.attr("viewBox").and_then(geom::parse_numbers) else {
        return Ok(None);
    };
    match *v.as_slice() {
        [x, y, w, h] if w > 0.0 && h > 0.0 => Ok(Some((x, y, w, h))),
        [_, _, _, _] => Err(()),
        _ => Ok(None),
    }
}

/// La propiedad `transform` de CSS (CSS Transforms 1 §7): funciones con unidades.
fn css_transform(v: &[ComponentValue], vp: Viewport, font: f32) -> Option<Transform> {
    if ident(v).as_deref() == Some("none") {
        return Some(Transform::IDENTITY);
    }
    let mut t = Transform::IDENTITY;
    for c in toks(v) {
        let ComponentValue::Function(f) = c else {
            return None;
        };
        let args: Vec<&ComponentValue> = toks(&f.value)
            .into_iter()
            .filter(|a| !matches!(a.token(), Some(TokenKind::Comma)))
            .collect();
        let len = |k: usize, axis: Axis| -> Option<f32> {
            args.get(k)
                .and_then(|a| cv_len(a))
                .map(|l| vp.resolve(l, axis, font))
        };
        let num = |k: usize| -> Option<f32> {
            match args.get(k)?.token()? {
                TokenKind::Number(n) => Some(n.value as f32),
                TokenKind::Percentage(n) => Some(n.value as f32 / 100.0),
                _ => None,
            }
        };
        let angle = |k: usize| -> Option<f32> {
            match args.get(k)?.token()? {
                TokenKind::Number(n) => Some(n.value as f32),
                TokenKind::Dimension(n, u) => {
                    let v = n.value as f32;
                    Some(match u.to_ascii_lowercase().as_str() {
                        "deg" => v,
                        "rad" => v.to_degrees(),
                        "grad" => v * 0.9,
                        "turn" => v * 360.0,
                        _ => return None,
                    })
                }
                _ => None,
            }
        };
        let skew = |a: f32| (a * std::f32::consts::PI / 180.0).tan();
        let m = match f.name.to_ascii_lowercase().as_str() {
            "translate" => Transform::translate(len(0, Axis::X)?, len(1, Axis::Y).unwrap_or(0.0)),
            "translatex" => Transform::translate(len(0, Axis::X)?, 0.0),
            "translatey" => Transform::translate(0.0, len(0, Axis::Y)?),
            "scale" => {
                let sx = num(0)?;
                Transform::scale(sx, num(1).unwrap_or(sx))
            }
            "scalex" => Transform::scale(num(0)?, 1.0),
            "scaley" => Transform::scale(1.0, num(0)?),
            "rotate" => Transform::rotate(angle(0)?),
            "skew" => Transform {
                c: skew(angle(0)?),
                b: skew(angle(1).unwrap_or(0.0)),
                ..Transform::IDENTITY
            },
            "skewx" => Transform {
                c: skew(angle(0)?),
                ..Transform::IDENTITY
            },
            "skewy" => Transform {
                b: skew(angle(0)?),
                ..Transform::IDENTITY
            },
            "matrix" => Transform {
                a: num(0)?,
                b: num(1)?,
                c: num(2)?,
                d: num(3)?,
                e: num(4)?,
                f: num(5)?,
            },
            _ => return None,
        };
        t = t.then(&m);
    }
    Some(t)
}

/// Transformación del `viewBox` al viewport de `w`×`h` (§8.7).
pub(crate) fn root_view_box(n: &DocNode) -> Option<Bbox> {
    view_box(n)
}

pub(crate) fn view_box_transform(
    (vx, vy, vw, vh): Bbox,
    par: &str,
    (w, h): (f32, f32),
) -> Transform {
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
    let (ax, ay) = (
        align.get(0..4).unwrap_or("xMid"),
        align.get(4..8).unwrap_or("YMid"),
    );
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
        Len::Calc(px, 0.0, em) => Some(px + em * 16.0),
        // Los porcentajes y las unidades del viewport dependen del contenedor.
        Len::Pct(_) | Len::Calc(..) | Len::Vw(_) | Len::Vh(_) | Len::Vmin(_) | Len::Vmax(_) => None,
    }
    .filter(|v| *v >= 0.0)
}

/// Región de un fragmento `#xywh=` (Media Fragments URI 1.0 §4.2.2), en px del tamaño por
/// defecto de la imagen y recortada a ella.
fn crop_region(svg: &Svg) -> Option<Bbox> {
    let crop = svg.view.as_ref()?.crop?;
    let mut inner = svg.clone();
    if let Some(v) = &mut inner.view {
        v.crop = None;
    }
    let (dw, dh) = inner.default_size();
    let (x, y, w, h) = match crop {
        crate::Crop::Pixel(x, y, w, h) => (x, y, w, h),
        crate::Crop::Percent(x, y, w, h) => (
            x * dw / 100.0,
            y * dh / 100.0,
            w * dw / 100.0,
            h * dh / 100.0,
        ),
    };
    let x0 = x.clamp(0.0, dw);
    let y0 = y.clamp(0.0, dh);
    let x1 = (x + w).clamp(0.0, dw);
    let y1 = (y + h).clamp(0.0, dh);
    Some((x0, y0, x1 - x0, y1 - y0))
}

pub(crate) fn intrinsic(svg: &Svg) -> Intrinsic {
    let Some(root) = svg.nodes.first() else {
        return Intrinsic::default();
    };
    if let Some((_, _, w, h)) = crop_region(svg) {
        return Intrinsic {
            width: Some(w),
            height: Some(h),
            ratio: (w > 0.0 && h > 0.0).then(|| w / h),
        };
    }
    let width = absolute(root, "width");
    let height = absolute(root, "height");
    let vb = svg
        .view
        .as_ref()
        .and_then(|v| v.view_box)
        .or_else(|| view_box(root));
    let ratio = match (width, height, vb) {
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
    if let Some((rx, ry, rw, rh)) = crop_region(svg)
        && rw > 0.0
        && rh > 0.0
    {
        let mut inner = svg.clone();
        if let Some(v) = &mut inner.view {
            v.crop = None;
        }
        let (dw, dh) = inner.default_size();
        let full = tree(&inner, dw, dh);
        let root = Group {
            host_units: false,
            context: None,
            transform: Transform::scale(width / rw, height / rh)
                .then(&Transform::translate(-rx, -ry)),
            opacity: 1.0,
            clip: None,
            mask: None,
            filter: None,
            children: vec![Node::Group(full.root)],
        };
        return Tree {
            width,
            height,
            root,
        };
    }
    let mut b = Builder {
        svg,
        stack: Vec::new(),
        context: Vec::new(),
    };
    let mut root = Group {
        host_units: false,
        context: None,
        transform: Transform::IDENTITY,
        opacity: 1.0,
        clip: None,
        mask: None,
        filter: None,
        children: Vec::new(),
    };
    if let Some(r) = svg.nodes.first()
        && r.name == "svg"
        && !Builder::display_none(r)
    {
        let view = svg.view.as_ref();
        let disabled = view_box_state(r).is_err() && view.and_then(|v| v.view_box).is_none();
        let vb = view.and_then(|v| v.view_box).or_else(|| view_box(r));
        let vp = match vb {
            Some((_, _, w, h)) => Viewport {
                w,
                h,
                icb: (width, height),
            },
            None => Viewport {
                w: width,
                h: height,
                icb: (width, height),
            },
        };
        let base = St {
            color: svg.inherited.color,
            size: svg.inherited.font_size,
            family: svg.inherited.font_family.clone(),
            ..St::default()
        };
        let st = b.computed(r, &base, vp);
        let par = view
            .and_then(|v| v.par.as_deref())
            .or_else(|| r.attr("preserveAspectRatio"))
            .unwrap_or("");
        root.transform = vb.map_or(Transform::IDENTITY, |vb| {
            view_box_transform(vb, par, (width, height))
        });
        if let Some(t) = view.and_then(|v| v.transform) {
            root.transform = root.transform.then(&t);
        }
        root.opacity = Builder::opacity(r);
        b.stack.push(0);
        if !disabled {
            root.children = b.children(0, &st, vp);
        }
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

#[cfg(test)]
mod tests {
    use crate::{ContextPaint, Group, Node, Paint, Path, Rgba, Svg};

    fn tree(body: &str, w: f32, h: f32) -> Group {
        let src = format!(
            "<svg xmlns='http://www.w3.org/2000/svg' xmlns:xlink='http://www.w3.org/1999/xlink'>\
             {body}</svg>"
        );
        Svg::parse(&src).unwrap().tree(w, h).root
    }

    fn paths(g: &Group, out: &mut Vec<Path>) {
        for c in &g.children {
            match c {
                Node::Path(p) => out.push(p.clone()),
                Node::Group(g) => paths(g, out),
                _ => {}
            }
        }
    }

    fn groups<'a>(g: &'a Group, out: &mut Vec<&'a Group>) {
        out.push(g);
        for c in &g.children {
            if let Node::Group(k) = c {
                groups(k, out);
            }
        }
    }

    fn color(p: &Option<crate::Fill>) -> Option<Rgba> {
        match p.as_ref()?.paint {
            Paint::Color(c) => Some(c),
            _ => None,
        }
    }

    #[test]
    fn context_paint_in_use() {
        // SVG 2 §13.3: el relleno y el trazo del `use`; `currentColor`, con el `color` del
        // elemento que la usa.
        let g = tree(
            "<defs><rect id='r' width='10' height='10' color='lime' \
             fill='context-stroke' stroke='context-fill'/></defs>\
             <use href='#r' fill='blue' stroke='currentColor' color='red'/>",
            100.0,
            100.0,
        );
        let mut ps = Vec::new();
        paths(&g, &mut ps);
        assert_eq!(color(&ps[0].fill), Some(Rgba::rgb(0, 255, 0)));
        let stroke = ps[0].stroke.as_ref().map(|s| s.paint.clone());
        assert_eq!(stroke, Some(Paint::Color(Rgba::rgb(0, 0, 255))));
        // Sin elemento de contexto no hay pintura.
        let g = tree(
            "<rect width='10' height='10' fill='context-fill'/>",
            100.0,
            100.0,
        );
        let mut ps = Vec::new();
        paths(&g, &mut ps);
        assert!(ps.is_empty() || ps[0].fill.is_none());
    }

    #[test]
    fn context_paint_server_resolved_by_group() {
        // Con un gradiente, la forma deja `Paint::Context` y el grupo del `use` lleva la
        // pintura en su sistema.
        let g = tree(
            "<defs><linearGradient id='lg'><stop offset='0' stop-color='red'/>\
             <stop offset='1' stop-color='blue'/></linearGradient>\
             <rect id='r' width='10' height='10' fill='context-fill'/></defs>\
             <use href='#r' x='5' fill='url(#lg)'/>",
            100.0,
            100.0,
        );
        let mut ps = Vec::new();
        paths(&g, &mut ps);
        assert_eq!(
            ps[0].fill.as_ref().map(|f| f.paint.clone()),
            Some(Paint::Context(ContextPaint::Fill))
        );
        let mut gs = Vec::new();
        groups(&g, &mut gs);
        let ctx = gs
            .iter()
            .find_map(|g| g.context.as_ref())
            .expect("contexto");
        assert!(matches!(ctx.fill, Some(Paint::Gradient(_))));
    }

    #[test]
    fn viewport_units_and_auto_radii() {
        // `vw`/`vh` (CSS Values 4 §6.1.2) del viewport inicial; un radio negativo es `auto`
        // (SVG 2 §10.4).
        let g = tree(
            "<rect style='width:10vw;height:50vh' fill='green'/>\
             <ellipse cx='50' cy='50' rx='-5' ry='20'/>",
            200.0,
            100.0,
        );
        let mut ps = Vec::new();
        paths(&g, &mut ps);
        let b = crate::geom::bbox(&ps[0].segs).unwrap();
        assert_eq!((b.2, b.3), (20.0, 50.0));
        let b = crate::geom::bbox(&ps[1].segs).unwrap();
        assert!((b.2 - 40.0).abs() < 1e-3 && (b.3 - 40.0).abs() < 1e-3);
    }

    #[test]
    fn overflow_attribute_and_nested_clip() {
        // `overflow` es atributo de presentación (§6.6): un `svg` anidado visible no recorta.
        let g = tree(
            "<svg width='1' height='1' overflow='visible'><rect width='50' height='50'/></svg>",
            100.0,
            100.0,
        );
        let mut gs = Vec::new();
        groups(&g, &mut gs);
        assert!(gs.iter().all(|g| g.clip.is_none()));
        // `clip-path` en un `clipPath`: el recorte lleva el anidado (CSS Masking 1 §6.3).
        let g = tree(
            "<clipPath id='a'><circle r='5'/></clipPath>\
             <clipPath id='b' clip-path='url(#a)'><rect width='10' height='10'/></clipPath>\
             <rect width='20' height='20' clip-path='url(#b)'/>",
            100.0,
            100.0,
        );
        let mut gs = Vec::new();
        groups(&g, &mut gs);
        let clip = gs.iter().find_map(|g| g.clip.as_ref()).expect("recorte");
        assert_eq!(clip.shapes.len(), 1);
        assert_eq!(clip.clip.as_ref().map(|c| c.shapes.len()), Some(1));
    }

    #[test]
    fn markers_on_polyline_vertices() {
        // SVG 2 §13.7: un marcador por vértice, con su (`refX`, `refY`) sobre el vértice; con
        // `orient='auto'` el del medio va girado por la bisectriz, pero el punto de referencia
        // sigue cayendo en el vértice.
        let g = tree(
            "<marker id='m' markerWidth='4' markerHeight='4' refX='2' refY='2' orient='auto' \
             markerUnits='userSpaceOnUse'><rect width='4' height='4' fill='red'/></marker>\
             <polyline points='5,5 35,5 35,35' fill='none' stroke='blue' stroke-width='0.5' \
             marker-start='url(#m)' marker-mid='url(#m)' marker-end='url(#m)'/>",
            40.0,
            40.0,
        );
        let mut gs = Vec::new();
        groups(&g, &mut gs);
        // Cada marcador recorta a su caja (`overflow: hidden` por defecto).
        let markers: Vec<_> = gs.iter().filter(|g| g.clip.is_some()).collect();
        assert_eq!(markers.len(), 3);
        for (m, (vx, vy)) in markers.iter().zip([(5.0, 5.0), (35.0, 5.0), (35.0, 35.0)]) {
            let (x, y) = m.transform.apply(2.0, 2.0);
            assert!((x - vx).abs() < 1e-3 && (y - vy).abs() < 1e-3, "({x}, {y})");
        }
    }

    #[test]
    fn malformed_start_tag_stops_the_tree() {
        // XML 1.0 §3.1: entre atributos el espacio es obligatorio (`STag ::= '<' Name (S
        // Attribute)* S? '>'`); sin él es un error fatal y el documento queda en lo parseado
        // hasta ahí: la forma con la etiqueta mal formada no se pinta, ni sus marcadores.
        let g = tree(
            "<rect width='10' height='10' fill='red'/>\
             <polyline points='5,5 35,5' stroke='blue'marker-start='url(#m)'/>\
             <rect y='20' width='10' height='10'/>",
            40.0,
            40.0,
        );
        let mut ps = Vec::new();
        paths(&g, &mut ps);
        assert_eq!(ps.len(), 1);
        assert_eq!(color(&ps[0].fill), Some(Rgba::rgb(255, 0, 0)));
    }
}
