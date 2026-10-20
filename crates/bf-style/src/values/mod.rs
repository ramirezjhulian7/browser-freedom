//! Valores de CSS Values 4: longitudes, porcentajes, `calc()`, números, ángulos, tiempos,
//! URLs e imágenes. Los colores están en [`color`].

pub mod color;
mod named_colors;

use std::fmt;

use bf_css::syntax::{BlockKind, ComponentValue, Numeric, TokenKind};

pub use color::{Color, Rgba};

/// Unidades de longitud (CSS Values 4 §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LengthUnit {
    Px,
    Cm,
    Mm,
    Q,
    In,
    Pt,
    Pc,
    Em,
    Rem,
    Ex,
    Ch,
    Lh,
    Vw,
    Vh,
    Vmin,
    Vmax,
}

impl LengthUnit {
    pub fn from_unit(unit: &str) -> Option<Self> {
        Some(match unit.to_ascii_lowercase().as_str() {
            "px" => LengthUnit::Px,
            "cm" => LengthUnit::Cm,
            "mm" => LengthUnit::Mm,
            "q" => LengthUnit::Q,
            "in" => LengthUnit::In,
            "pt" => LengthUnit::Pt,
            "pc" => LengthUnit::Pc,
            "em" => LengthUnit::Em,
            "rem" => LengthUnit::Rem,
            "ex" => LengthUnit::Ex,
            "ch" => LengthUnit::Ch,
            "lh" => LengthUnit::Lh,
            "vw" => LengthUnit::Vw,
            "vh" => LengthUnit::Vh,
            "vmin" => LengthUnit::Vmin,
            "vmax" => LengthUnit::Vmax,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            LengthUnit::Px => "px",
            LengthUnit::Cm => "cm",
            LengthUnit::Mm => "mm",
            LengthUnit::Q => "q",
            LengthUnit::In => "in",
            LengthUnit::Pt => "pt",
            LengthUnit::Pc => "pc",
            LengthUnit::Em => "em",
            LengthUnit::Rem => "rem",
            LengthUnit::Ex => "ex",
            LengthUnit::Ch => "ch",
            LengthUnit::Lh => "lh",
            LengthUnit::Vw => "vw",
            LengthUnit::Vh => "vh",
            LengthUnit::Vmin => "vmin",
            LengthUnit::Vmax => "vmax",
        }
    }
}

/// Lo necesario para absolutizar longitudes relativas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LengthContext {
    /// Font size del elemento (para `font-size` mismo, el del padre).
    pub font_size: f32,
    pub root_font_size: f32,
    /// Line height del elemento en px (para `lh`).
    pub line_height: f32,
    pub viewport_width: f32,
    pub viewport_height: f32,
    /// x-height / font-size de la fuente (para `ex`); 0.5 sin métricas reales.
    pub ex_ratio: f32,
    /// Avance de "0" / font-size (para `ch`); 0.5 sin métricas reales.
    pub ch_ratio: f32,
}

impl Default for LengthContext {
    fn default() -> Self {
        LengthContext {
            font_size: 16.0,
            root_font_size: 16.0,
            line_height: 16.0 * NORMAL_LINE_HEIGHT,
            viewport_width: 800.0,
            viewport_height: 600.0,
            ex_ratio: 0.5,
            ch_ratio: 0.5,
        }
    }
}

/// Valor usado para `line-height: normal` cuando no hay métricas de fuente (unidad `lh`).
pub const NORMAL_LINE_HEIGHT: f32 = 1.2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Length {
    pub value: f32,
    pub unit: LengthUnit,
}

impl Length {
    pub fn px(value: f32) -> Self {
        Length {
            value,
            unit: LengthUnit::Px,
        }
    }

    /// Longitud absoluta en px.
    pub fn to_px(self, ctx: &LengthContext) -> f32 {
        let v = self.value;
        match self.unit {
            LengthUnit::Px => v,
            LengthUnit::Cm => v * 96.0 / 2.54,
            LengthUnit::Mm => v * 96.0 / 25.4,
            LengthUnit::Q => v * 96.0 / 101.6,
            LengthUnit::In => v * 96.0,
            LengthUnit::Pt => v * 96.0 / 72.0,
            LengthUnit::Pc => v * 16.0,
            LengthUnit::Em => v * ctx.font_size,
            LengthUnit::Rem => v * ctx.root_font_size,
            // Sin métricas de fuente reales, los ratios valen 0.5 (CSS Values 4 §6.1.1).
            LengthUnit::Ex => v * ctx.font_size * ctx.ex_ratio,
            LengthUnit::Ch => v * ctx.font_size * ctx.ch_ratio,
            LengthUnit::Lh => v * ctx.line_height,
            LengthUnit::Vw => v * ctx.viewport_width / 100.0,
            LengthUnit::Vh => v * ctx.viewport_height / 100.0,
            LengthUnit::Vmin => v * ctx.viewport_width.min(ctx.viewport_height) / 100.0,
            LengthUnit::Vmax => v * ctx.viewport_width.max(ctx.viewport_height) / 100.0,
        }
    }
}

impl fmt::Display for Length {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", fmt_number(self.value), self.unit.as_str())
    }
}

/// Número sin ceros superfluos (para volcados).
pub fn fmt_number(v: f32) -> String {
    let rounded = (v * 1000.0).round() / 1000.0;
    if rounded == rounded.trunc() {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded}")
    }
}

/// Árbol de `calc()` (CSS Values 4 §10). Se conserva hasta el cómputo; si al computar quedan
/// porcentajes, se conserva hasta el uso.
#[derive(Debug, Clone, PartialEq)]
pub enum CalcNode {
    Number(f32),
    Length(Length),
    Percentage(f32),
    Sum(Vec<CalcNode>),
    Product(Vec<CalcNode>),
    Negate(Box<CalcNode>),
    Invert(Box<CalcNode>),
    Min(Vec<CalcNode>),
    Max(Vec<CalcNode>),
    Clamp(Box<CalcNode>, Box<CalcNode>, Box<CalcNode>),
}

/// Tipo de un nodo de `calc()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CalcType {
    Number,
    Length,
    Percentage,
    LengthPercentage,
}

impl CalcType {
    fn add(self, other: CalcType) -> Option<CalcType> {
        use CalcType::*;
        match (self, other) {
            (Number, Number) => Some(Number),
            (Number, _) | (_, Number) => None,
            (Length, Length) => Some(Length),
            (Percentage, Percentage) => Some(Percentage),
            _ => Some(LengthPercentage),
        }
    }
}

impl CalcNode {
    /// Absolutiza las longitudes y simplifica las operaciones sin porcentajes.
    fn compute(&self, ctx: &LengthContext) -> CalcNode {
        let node = match self {
            CalcNode::Length(l) => CalcNode::Length(Length::px(l.to_px(ctx))),
            CalcNode::Number(_) | CalcNode::Percentage(_) => self.clone(),
            CalcNode::Sum(v) => CalcNode::Sum(v.iter().map(|n| n.compute(ctx)).collect()),
            CalcNode::Product(v) => CalcNode::Product(v.iter().map(|n| n.compute(ctx)).collect()),
            CalcNode::Negate(n) => CalcNode::Negate(Box::new(n.compute(ctx))),
            CalcNode::Invert(n) => CalcNode::Invert(Box::new(n.compute(ctx))),
            CalcNode::Min(v) => CalcNode::Min(v.iter().map(|n| n.compute(ctx)).collect()),
            CalcNode::Max(v) => CalcNode::Max(v.iter().map(|n| n.compute(ctx)).collect()),
            CalcNode::Clamp(a, b, c) => CalcNode::Clamp(
                Box::new(a.compute(ctx)),
                Box::new(b.compute(ctx)),
                Box::new(c.compute(ctx)),
            ),
        };
        if node.has_percentage() {
            node
        } else {
            match node.eval(0.0) {
                (v, true) => CalcNode::Length(Length::px(v)),
                (v, false) => CalcNode::Number(v),
            }
        }
    }

    fn has_percentage(&self) -> bool {
        match self {
            CalcNode::Percentage(_) => true,
            CalcNode::Number(_) | CalcNode::Length(_) => false,
            CalcNode::Sum(v) | CalcNode::Product(v) | CalcNode::Min(v) | CalcNode::Max(v) => {
                v.iter().any(CalcNode::has_percentage)
            }
            CalcNode::Negate(n) | CalcNode::Invert(n) => n.has_percentage(),
            CalcNode::Clamp(a, b, c) => {
                a.has_percentage() || b.has_percentage() || c.has_percentage()
            }
        }
    }

    /// Evalúa con las longitudes ya en px y `basis` para los porcentajes. Devuelve el valor y
    /// si es una longitud (no un número).
    fn eval(&self, basis: f32) -> (f32, bool) {
        match self {
            CalcNode::Number(n) => (*n, false),
            CalcNode::Length(l) => (l.value, true),
            CalcNode::Percentage(p) => (p / 100.0 * basis, true),
            CalcNode::Sum(v) => v.iter().fold((0.0, false), |(acc, len), n| {
                let (x, l) = n.eval(basis);
                (acc + x, len || l)
            }),
            CalcNode::Product(v) => v.iter().fold((1.0, false), |(acc, len), n| {
                let (x, l) = n.eval(basis);
                (acc * x, len || l)
            }),
            CalcNode::Negate(n) => {
                let (x, l) = n.eval(basis);
                (-x, l)
            }
            CalcNode::Invert(n) => {
                let (x, l) = n.eval(basis);
                (1.0 / x, l)
            }
            CalcNode::Min(v) => v.iter().fold((f32::INFINITY, false), |(acc, len), n| {
                let (x, l) = n.eval(basis);
                (acc.min(x), len || l)
            }),
            CalcNode::Max(v) => v.iter().fold((f32::NEG_INFINITY, false), |(acc, len), n| {
                let (x, l) = n.eval(basis);
                (acc.max(x), len || l)
            }),
            CalcNode::Clamp(min, val, max) => {
                let (lo, l1) = min.eval(basis);
                let (v, l2) = val.eval(basis);
                let (hi, l3) = max.eval(basis);
                (v.min(hi).max(lo), l1 || l2 || l3)
            }
        }
    }
}

impl fmt::Display for CalcNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = |f: &mut fmt::Formatter<'_>, name: &str, v: &[CalcNode]| {
            let items: Vec<String> = v.iter().map(ToString::to_string).collect();
            write!(f, "{name}({})", items.join(", "))
        };
        match self {
            CalcNode::Number(n) => f.write_str(&fmt_number(*n)),
            CalcNode::Length(l) => write!(f, "{l}"),
            CalcNode::Percentage(p) => write!(f, "{}%", fmt_number(*p)),
            CalcNode::Sum(v) => {
                let items: Vec<String> = v.iter().map(ToString::to_string).collect();
                write!(f, "({})", items.join(" + "))
            }
            CalcNode::Product(v) => {
                let items: Vec<String> = v.iter().map(ToString::to_string).collect();
                write!(f, "({})", items.join(" * "))
            }
            CalcNode::Negate(n) => write!(f, "-{n}"),
            CalcNode::Invert(n) => write!(f, "1 / {n}"),
            CalcNode::Min(v) => list(f, "min", v),
            CalcNode::Max(v) => list(f, "max", v),
            CalcNode::Clamp(a, b, c) => write!(f, "clamp({a}, {b}, {c})"),
        }
    }
}

/// `<length-percentage>` (especificado o computado: en el computado las longitudes están en
/// px y `Calc` solo queda si contiene porcentajes).
#[derive(Debug, Clone, PartialEq)]
pub enum LengthPercentage {
    Length(Length),
    Percentage(f32),
    Calc(Box<CalcNode>),
}

impl LengthPercentage {
    pub fn zero() -> Self {
        LengthPercentage::Length(Length::px(0.0))
    }

    pub fn px(v: f32) -> Self {
        LengthPercentage::Length(Length::px(v))
    }

    pub fn compute(&self, ctx: &LengthContext) -> Self {
        match self {
            LengthPercentage::Length(l) => LengthPercentage::Length(Length::px(l.to_px(ctx))),
            LengthPercentage::Percentage(_) => self.clone(),
            LengthPercentage::Calc(c) => match c.compute(ctx) {
                CalcNode::Length(l) => LengthPercentage::Length(l),
                CalcNode::Percentage(p) => LengthPercentage::Percentage(p),
                other => LengthPercentage::Calc(Box::new(other)),
            },
        }
    }

    /// Valor usado en px; `basis` es la referencia de los porcentajes. Requiere valor computado.
    pub fn resolve(&self, basis: f32) -> f32 {
        match self {
            LengthPercentage::Length(l) => l.value,
            LengthPercentage::Percentage(p) => p / 100.0 * basis,
            LengthPercentage::Calc(c) => c.eval(basis).0,
        }
    }

    /// ¿Depende de la referencia de porcentajes?
    pub fn has_percentage(&self) -> bool {
        match self {
            LengthPercentage::Length(_) => false,
            LengthPercentage::Percentage(_) => true,
            LengthPercentage::Calc(c) => c.has_percentage(),
        }
    }

    fn is_negative(&self) -> bool {
        match self {
            LengthPercentage::Length(l) => l.value < 0.0,
            LengthPercentage::Percentage(p) => *p < 0.0,
            LengthPercentage::Calc(_) => false,
        }
    }
}

impl fmt::Display for LengthPercentage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LengthPercentage::Length(l) => write!(f, "{l}"),
            LengthPercentage::Percentage(p) => write!(f, "{}%", fmt_number(*p)),
            LengthPercentage::Calc(c) => write!(f, "calc{c}"),
        }
    }
}

// --- Helpers de parseo sobre component values ---

/// Quita el whitespace de los extremos.
pub fn trim(values: &[ComponentValue]) -> &[ComponentValue] {
    let start = values
        .iter()
        .position(|v| !v.is_whitespace())
        .unwrap_or(values.len());
    let end = values
        .iter()
        .rposition(|v| !v.is_whitespace())
        .map_or(start, |e| e + 1);
    &values[start..end]
}

/// Los component values no-whitespace.
pub fn components(values: &[ComponentValue]) -> Vec<&ComponentValue> {
    values.iter().filter(|v| !v.is_whitespace()).collect()
}

/// Si `values` es un único ident, lo devuelve.
pub fn single_ident(values: &[ComponentValue]) -> Option<&str> {
    match trim(values) {
        [v] => ident(v),
        _ => None,
    }
}

pub fn ident(v: &ComponentValue) -> Option<&str> {
    match v.token() {
        Some(TokenKind::Ident(s)) => Some(s),
        _ => None,
    }
}

pub fn is_ident(v: &ComponentValue, name: &str) -> bool {
    ident(v).is_some_and(|s| s.eq_ignore_ascii_case(name))
}

pub fn number(v: &ComponentValue) -> Option<f32> {
    match v.token() {
        Some(TokenKind::Number(n)) => Some(n.value as f32),
        _ => None,
    }
}

pub fn integer(v: &ComponentValue) -> Option<i32> {
    match v.token() {
        Some(TokenKind::Number(n)) => n.as_integer(),
        _ => None,
    }
}

fn numeric_len(n: &Numeric, unit: &str) -> Option<Length> {
    Some(Length {
        value: n.value as f32,
        unit: LengthUnit::from_unit(unit)?,
    })
}

/// `<length>`; `0` sin unidad es válido.
pub fn parse_length(v: &ComponentValue) -> Option<Length> {
    match v.token()? {
        TokenKind::Dimension(n, unit) => numeric_len(n, unit),
        TokenKind::Number(n) if n.value == 0.0 => Some(Length::px(0.0)),
        _ => None,
    }
}

/// `<length-percentage>`, incluidas las math functions.
pub fn parse_length_percentage(v: &ComponentValue) -> Option<LengthPercentage> {
    if let ComponentValue::Function(f) = v {
        let (node, ty) = parse_math_function(&f.name, &f.value)?;
        return match ty {
            CalcType::Number => None,
            _ => Some(LengthPercentage::Calc(Box::new(node))),
        };
    }
    match v.token()? {
        TokenKind::Percentage(p) => Some(LengthPercentage::Percentage(p.value as f32)),
        _ => parse_length(v).map(LengthPercentage::Length),
    }
}

/// `<length-percentage [0,∞]>`.
pub fn parse_non_negative_lp(v: &ComponentValue) -> Option<LengthPercentage> {
    parse_length_percentage(v).filter(|lp| !lp.is_negative())
}

/// `<number>`, incluido `calc()` de números.
pub fn parse_number(v: &ComponentValue) -> Option<f32> {
    if let ComponentValue::Function(f) = v {
        let (node, ty) = parse_math_function(&f.name, &f.value)?;
        return (ty == CalcType::Number).then(|| node.eval(0.0).0);
    }
    number(v)
}

/// `<angle>` en grados (`0` sin unidad se acepta, como en `<angle-percentage>` legacy).
pub fn parse_angle(v: &ComponentValue) -> Option<f32> {
    match v.token()? {
        TokenKind::Dimension(n, unit) => {
            let n = n.value as f32;
            Some(match unit.to_ascii_lowercase().as_str() {
                "deg" => n,
                "grad" => n * 0.9,
                "rad" => n.to_degrees(),
                "turn" => n * 360.0,
                _ => return None,
            })
        }
        TokenKind::Number(n) if n.value == 0.0 => Some(0.0),
        _ => None,
    }
}

/// `<time>` en segundos.
pub fn parse_time(v: &ComponentValue) -> Option<f32> {
    match v.token()? {
        TokenKind::Dimension(n, unit) => match unit.to_ascii_lowercase().as_str() {
            "s" => Some(n.value as f32),
            "ms" => Some(n.value as f32 / 1000.0),
            _ => None,
        },
        _ => None,
    }
}

/// `<url>`: `url(...)` sin comillas o `url("...")`.
pub fn parse_url(v: &ComponentValue) -> Option<String> {
    match v {
        ComponentValue::Token(t) => match &t.kind {
            TokenKind::Url(u) => Some(u.clone()),
            _ => None,
        },
        ComponentValue::Function(f)
            if f.name.eq_ignore_ascii_case("url") || f.name.eq_ignore_ascii_case("src") =>
        {
            match components(&f.value).as_slice() {
                [arg] => match arg.token()? {
                    TokenKind::String(s) => Some(s.clone()),
                    _ => None,
                },
                _ => None,
            }
        }
        _ => None,
    }
}

/// Math functions: `calc()`, `min()`, `max()`, `clamp()`.
fn parse_math_function(name: &str, args: &[ComponentValue]) -> Option<(CalcNode, CalcType)> {
    match name.to_ascii_lowercase().as_str() {
        "calc" => parse_calc_sum(trim(args)),
        "min" | "max" => {
            let mut nodes = Vec::new();
            let mut ty: Option<CalcType> = None;
            for group in bf_css::syntax::split_comma_separated(args) {
                let (n, t) = parse_calc_sum(trim(&group))?;
                ty = Some(match ty {
                    None => t,
                    Some(prev) => prev.add(t)?,
                });
                nodes.push(n);
            }
            let ty = ty?;
            Some((
                if name.eq_ignore_ascii_case("min") {
                    CalcNode::Min(nodes)
                } else {
                    CalcNode::Max(nodes)
                },
                ty,
            ))
        }
        "clamp" => {
            let groups = bf_css::syntax::split_comma_separated(args);
            let [a, b, c] = groups.as_slice() else {
                return None;
            };
            let (a, ta) = parse_calc_sum(trim(a))?;
            let (b, tb) = parse_calc_sum(trim(b))?;
            let (c, tc) = parse_calc_sum(trim(c))?;
            let ty = ta.add(tb)?.add(tc)?;
            Some((CalcNode::Clamp(Box::new(a), Box::new(b), Box::new(c)), ty))
        }
        _ => None,
    }
}

/// `<calc-sum> = <calc-product> [ [ '+' | '-' ] <calc-product> ]*` (los operadores aditivos
/// deben ir rodeados de whitespace).
fn parse_calc_sum(values: &[ComponentValue]) -> Option<(CalcNode, CalcType)> {
    let mut terms: Vec<(bool, &[ComponentValue])> = Vec::new();
    let mut start = 0;
    let mut negative = false;
    let mut i = 0;
    while i < values.len() {
        let op = match values[i].token() {
            Some(TokenKind::Delim(c @ ('+' | '-'))) => Some(*c),
            _ => None,
        };
        if let Some(op) = op
            && i > 0
            && values[i - 1].is_whitespace()
            && values.get(i + 1).is_some_and(ComponentValue::is_whitespace)
        {
            terms.push((negative, trim(&values[start..i])));
            negative = op == '-';
            start = i + 1;
        }
        i += 1;
    }
    terms.push((negative, trim(&values[start..])));
    let mut nodes = Vec::new();
    let mut ty: Option<CalcType> = None;
    for (neg, term) in terms {
        let (node, t) = parse_calc_product(term)?;
        ty = Some(match ty {
            None => t,
            Some(prev) => prev.add(t)?,
        });
        nodes.push(if neg {
            CalcNode::Negate(Box::new(node))
        } else {
            node
        });
    }
    let ty = ty?;
    Some((
        if nodes.len() == 1 {
            nodes.pop().expect("un término")
        } else {
            CalcNode::Sum(nodes)
        },
        ty,
    ))
}

/// `<calc-product> = <calc-value> [ [ '*' | '/' ] <calc-value> ]*`.
fn parse_calc_product(values: &[ComponentValue]) -> Option<(CalcNode, CalcType)> {
    let items = components(values);
    let (first, rest) = items.split_first()?;
    let (mut node, mut ty) = parse_calc_value(first)?;
    let mut factors = vec![];
    let mut rest = rest;
    while let [op, operand, tail @ ..] = rest {
        let (n, t) = parse_calc_value(operand)?;
        match op.token()? {
            TokenKind::Delim('*') => {
                ty = match (ty, t) {
                    (CalcType::Number, t) => t,
                    (t, CalcType::Number) => t,
                    _ => return None,
                };
                factors.push(n);
            }
            TokenKind::Delim('/') => {
                if t != CalcType::Number {
                    return None;
                }
                factors.push(CalcNode::Invert(Box::new(n)));
            }
            _ => return None,
        }
        rest = tail;
    }
    if !rest.is_empty() {
        return None;
    }
    if !factors.is_empty() {
        factors.insert(0, node);
        node = CalcNode::Product(factors);
    }
    Some((node, ty))
}

fn parse_calc_value(v: &ComponentValue) -> Option<(CalcNode, CalcType)> {
    match v {
        ComponentValue::Block(b) if b.kind == BlockKind::Paren => parse_calc_sum(trim(&b.value)),
        ComponentValue::Function(f) => parse_math_function(&f.name, &f.value),
        ComponentValue::Token(t) => match &t.kind {
            TokenKind::Number(n) => Some((CalcNode::Number(n.value as f32), CalcType::Number)),
            TokenKind::Percentage(p) => {
                Some((CalcNode::Percentage(p.value as f32), CalcType::Percentage))
            }
            TokenKind::Dimension(n, unit) => {
                Some((CalcNode::Length(numeric_len(n, unit)?), CalcType::Length))
            }
            TokenKind::Ident(i) if i.eq_ignore_ascii_case("pi") => {
                Some((CalcNode::Number(std::f32::consts::PI), CalcType::Number))
            }
            TokenKind::Ident(i) if i.eq_ignore_ascii_case("e") => {
                Some((CalcNode::Number(std::f32::consts::E), CalcType::Number))
            }
            _ => None,
        },
        ComponentValue::Block(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bf_css::syntax::parse_component_value;

    fn lp(s: &str) -> LengthPercentage {
        parse_length_percentage(&parse_component_value(s).unwrap()).unwrap()
    }

    #[test]
    fn calc_and_units() {
        let ctx = LengthContext {
            font_size: 20.0,
            ..LengthContext::default()
        };
        assert_eq!(lp("2em").compute(&ctx).resolve(0.0), 40.0);
        assert_eq!(lp("1in").compute(&ctx).resolve(0.0), 96.0);
        assert_eq!(
            lp("calc(10px + 2em)").compute(&ctx),
            LengthPercentage::px(50.0)
        );
        let mixed = lp("calc(50% - 10px)").compute(&ctx);
        assert!(mixed.has_percentage());
        assert_eq!(mixed.resolve(200.0), 90.0);
        assert_eq!(
            lp("calc((1px + 2px) * 3)").compute(&ctx),
            LengthPercentage::px(9.0)
        );
        assert_eq!(lp("min(10px, 5%)").compute(&ctx).resolve(100.0), 5.0);
        assert_eq!(
            lp("clamp(10px, 50%, 30px)").compute(&ctx).resolve(100.0),
            30.0
        );
        assert_eq!(
            lp("calc(100px / 4)").compute(&ctx),
            LengthPercentage::px(25.0)
        );
        assert!(
            parse_length_percentage(&parse_component_value("calc(10px+2px)").unwrap()).is_none()
        );
        assert!(
            parse_length_percentage(&parse_component_value("calc(10px * 2px)").unwrap()).is_none()
        );
        assert!(
            parse_length_percentage(&parse_component_value("calc(10px + 2)").unwrap()).is_none()
        );
        assert!(parse_length_percentage(&parse_component_value("5").unwrap()).is_none());
    }
}
