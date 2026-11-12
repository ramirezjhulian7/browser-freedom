//! Valores de CSS Values 4: longitudes, porcentajes, `calc()`, números, ángulos, tiempos,
//! URLs e imágenes. Los colores están en [`color`].

pub mod color;
pub mod color_space;
mod named_colors;
pub mod registered;

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
    /// `vi`/`vb` y las variantes small/large/dynamic (CSS Values 4 §6.1.2): sin barras de
    /// interfaz que aparezcan o se oculten, los tres tamaños de viewport coinciden, y con
    /// escritura horizontal `vi` = `vw` y `vb` = `vh`.
    Vi,
    Vb,
    Svw,
    Svh,
    Svi,
    Svb,
    Svmin,
    Svmax,
    Lvw,
    Lvh,
    Lvi,
    Lvb,
    Lvmin,
    Lvmax,
    Dvw,
    Dvh,
    Dvi,
    Dvb,
    Dvmin,
    Dvmax,
    /// Unidades de container query (CSS Contain 3 §7): sin contenedor de consulta elegible
    /// (todavía no hay `@container`) valen como las small viewport units.
    Cqw,
    Cqh,
    Cqi,
    Cqb,
    Cqmin,
    Cqmax,
}

/// Las unidades de viewport con su eje: (nombre, unidad, eje).
const VIEWPORT_UNITS: &[(&str, LengthUnit, ViewportAxis)] = &[
    ("vi", LengthUnit::Vi, ViewportAxis::Width),
    ("vb", LengthUnit::Vb, ViewportAxis::Height),
    ("svw", LengthUnit::Svw, ViewportAxis::Width),
    ("svh", LengthUnit::Svh, ViewportAxis::Height),
    ("svi", LengthUnit::Svi, ViewportAxis::Width),
    ("svb", LengthUnit::Svb, ViewportAxis::Height),
    ("svmin", LengthUnit::Svmin, ViewportAxis::Min),
    ("svmax", LengthUnit::Svmax, ViewportAxis::Max),
    ("lvw", LengthUnit::Lvw, ViewportAxis::Width),
    ("lvh", LengthUnit::Lvh, ViewportAxis::Height),
    ("lvi", LengthUnit::Lvi, ViewportAxis::Width),
    ("lvb", LengthUnit::Lvb, ViewportAxis::Height),
    ("lvmin", LengthUnit::Lvmin, ViewportAxis::Min),
    ("lvmax", LengthUnit::Lvmax, ViewportAxis::Max),
    ("dvw", LengthUnit::Dvw, ViewportAxis::Width),
    ("dvh", LengthUnit::Dvh, ViewportAxis::Height),
    ("dvi", LengthUnit::Dvi, ViewportAxis::Width),
    ("dvb", LengthUnit::Dvb, ViewportAxis::Height),
    ("dvmin", LengthUnit::Dvmin, ViewportAxis::Min),
    ("dvmax", LengthUnit::Dvmax, ViewportAxis::Max),
    ("cqw", LengthUnit::Cqw, ViewportAxis::Width),
    ("cqh", LengthUnit::Cqh, ViewportAxis::Height),
    ("cqi", LengthUnit::Cqi, ViewportAxis::Width),
    ("cqb", LengthUnit::Cqb, ViewportAxis::Height),
    ("cqmin", LengthUnit::Cqmin, ViewportAxis::Min),
    ("cqmax", LengthUnit::Cqmax, ViewportAxis::Max),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewportAxis {
    Width,
    Height,
    Min,
    Max,
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
            other => {
                return VIEWPORT_UNITS
                    .iter()
                    .find(|(n, ..)| *n == other)
                    .map(|&(_, u, _)| u);
            }
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
            other => VIEWPORT_UNITS
                .iter()
                .find(|&&(_, u, _)| u == other)
                .map_or("px", |&(n, ..)| n),
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
            other => {
                let axis = VIEWPORT_UNITS
                    .iter()
                    .find(|&&(_, u, _)| u == other)
                    .map_or(ViewportAxis::Width, |&(.., a)| a);
                let (w, h) = (ctx.viewport_width, ctx.viewport_height);
                v * match axis {
                    ViewportAxis::Width => w,
                    ViewportAxis::Height => h,
                    ViewportAxis::Min => w.min(h),
                    ViewportAxis::Max => w.max(h),
                } / 100.0
            }
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
    /// `clamp()`; `none` en los extremos (CSS Values 4 §10.4) es un extremo infinito.
    Clamp(Box<CalcNode>, Box<CalcNode>, Box<CalcNode>),
    /// El resto de las math functions de CSS Values 4 §10.5–§10.8.
    Func(MathFn, Vec<CalcNode>),
}

/// Math functions de CSS Values 4 §10.5 (stepped value), §10.6 (trigonométricas), §10.7
/// (exponenciales) y §10.8 (signo).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathFn {
    Round(Rounding),
    Mod,
    Rem,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Pow,
    Sqrt,
    Hypot,
    Log,
    Exp,
    Abs,
    Sign,
}

/// `<rounding-strategy>` de `round()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rounding {
    Nearest,
    Up,
    Down,
    ToZero,
}

impl MathFn {
    fn name(self) -> &'static str {
        match self {
            MathFn::Round(_) => "round",
            MathFn::Mod => "mod",
            MathFn::Rem => "rem",
            MathFn::Sin => "sin",
            MathFn::Cos => "cos",
            MathFn::Tan => "tan",
            MathFn::Asin => "asin",
            MathFn::Acos => "acos",
            MathFn::Atan => "atan",
            MathFn::Atan2 => "atan2",
            MathFn::Pow => "pow",
            MathFn::Sqrt => "sqrt",
            MathFn::Hypot => "hypot",
            MathFn::Log => "log",
            MathFn::Exp => "exp",
            MathFn::Abs => "abs",
            MathFn::Sign => "sign",
        }
    }

    /// ¿El resultado tiene el tipo de sus argumentos (y no es un número o un ángulo)?
    fn keeps_type(self) -> bool {
        matches!(
            self,
            MathFn::Round(_) | MathFn::Mod | MathFn::Rem | MathFn::Hypot | MathFn::Abs
        )
    }

    fn apply(self, a: &[f32]) -> f32 {
        let x = a.first().copied().unwrap_or(f32::NAN);
        let y = a.get(1).copied();
        match self {
            MathFn::Round(r) => {
                let step = y.unwrap_or(1.0);
                if step == 0.0 {
                    return f32::NAN;
                }
                let q = x / step;
                let n = match r {
                    // El empate va hacia +∞ (§10.5.1).
                    Rounding::Nearest => (q + 0.5).floor(),
                    Rounding::Up => q.ceil(),
                    Rounding::Down => q.floor(),
                    Rounding::ToZero => q.trunc(),
                };
                n * step
            }
            MathFn::Mod => {
                let b = y.unwrap_or(f32::NAN);
                if b == 0.0 {
                    f32::NAN
                } else {
                    x - b * (x / b).floor()
                }
            }
            MathFn::Rem => {
                let b = y.unwrap_or(f32::NAN);
                if b == 0.0 {
                    f32::NAN
                } else {
                    x - b * (x / b).trunc()
                }
            }
            MathFn::Sin => x.sin(),
            MathFn::Cos => x.cos(),
            MathFn::Tan => x.tan(),
            MathFn::Asin => x.asin(),
            MathFn::Acos => x.acos(),
            MathFn::Atan => x.atan(),
            MathFn::Atan2 => x.atan2(y.unwrap_or(f32::NAN)),
            MathFn::Pow => x.powf(y.unwrap_or(f32::NAN)),
            MathFn::Sqrt => x.sqrt(),
            MathFn::Hypot => a.iter().map(|v| v * v).sum::<f32>().sqrt(),
            MathFn::Log => match y {
                Some(base) => x.ln() / base.ln(),
                None => x.ln(),
            },
            MathFn::Exp => x.exp(),
            MathFn::Abs => x.abs(),
            MathFn::Sign => {
                if x == 0.0 || x.is_nan() {
                    x
                } else {
                    x.signum()
                }
            }
        }
    }
}

/// Valor de nivel superior de una math function (CSS Values 4 §10.9): NaN es 0 y los
/// infinitos se recortan a un valor finito grande.
fn finite(v: f32) -> f32 {
    const BIG: f32 = 33_554_428.0;
    if v.is_nan() { 0.0 } else { v.clamp(-BIG, BIG) }
}

/// Tipo de un nodo de `calc()`. Los ángulos van en radianes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CalcType {
    Number,
    Length,
    Percentage,
    LengthPercentage,
    Angle,
}

impl CalcType {
    fn add(self, other: CalcType) -> Option<CalcType> {
        use CalcType::*;
        match (self, other) {
            (Number, Number) => Some(Number),
            (Angle, Angle) => Some(Angle),
            (Number | Angle, _) | (_, Number | Angle) => None,
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
            CalcNode::Func(f, v) => CalcNode::Func(*f, v.iter().map(|n| n.compute(ctx)).collect()),
        };
        if node.has_percentage() {
            node
        } else {
            match node.eval(0.0) {
                (v, true) => CalcNode::Length(Length::px(finite(v))),
                (v, false) => CalcNode::Number(finite(v)),
            }
        }
    }

    fn has_percentage(&self) -> bool {
        match self {
            CalcNode::Percentage(_) => true,
            CalcNode::Number(_) | CalcNode::Length(_) => false,
            CalcNode::Sum(v)
            | CalcNode::Product(v)
            | CalcNode::Min(v)
            | CalcNode::Max(v)
            | CalcNode::Func(_, v) => v.iter().any(CalcNode::has_percentage),
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
            // La dimensión de longitud del producto: +1 por factor longitud, −1 por divisor
            // longitud (`100vw / 1px` es un número).
            CalcNode::Product(v) => {
                let (value, power) = v.iter().fold((1.0, 0i32), |(acc, pow), n| match n {
                    CalcNode::Invert(d) => {
                        let (x, l) = d.eval(basis);
                        (acc / x, pow - i32::from(l))
                    }
                    _ => {
                        let (x, l) = n.eval(basis);
                        (acc * x, pow + i32::from(l))
                    }
                });
                (value, power != 0)
            }
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
            CalcNode::Func(f, v) => {
                let args: Vec<(f32, bool)> = v.iter().map(|n| n.eval(basis)).collect();
                let values: Vec<f32> = args.iter().map(|a| a.0).collect();
                let len = f.keeps_type() && args.iter().any(|a| a.1);
                (f.apply(&values), len)
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
            CalcNode::Func(MathFn::Round(r), v) if *r != Rounding::Nearest => {
                let r = match r {
                    Rounding::Up => "up",
                    Rounding::Down => "down",
                    _ => "to-zero",
                };
                let items: Vec<String> = v.iter().map(ToString::to_string).collect();
                write!(f, "round({r}, {})", items.join(", "))
            }
            CalcNode::Func(m, v) => list(f, m.name(), v),
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
            LengthPercentage::Calc(c) => finite(c.eval(basis).0),
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
            // Las sumas y productos ya se escriben entre paréntesis; las demás math
            // functions se escriben con su nombre.
            LengthPercentage::Calc(c) => match **c {
                CalcNode::Sum(_) | CalcNode::Product(_) => write!(f, "calc{c}"),
                CalcNode::Min(_) | CalcNode::Max(_) | CalcNode::Clamp(..) | CalcNode::Func(..) => {
                    write!(f, "{c}")
                }
                _ => write!(f, "calc({c})"),
            },
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
            CalcType::Number | CalcType::Angle => None,
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
        return (ty == CalcType::Number).then(|| finite(node.eval(0.0).0));
    }
    number(v)
}

/// `<angle>` en grados (`0` sin unidad se acepta, como en `<angle-percentage>` legacy),
/// incluidas las math functions de ángulos.
pub fn parse_angle(v: &ComponentValue) -> Option<f32> {
    if let ComponentValue::Function(f) = v {
        let (node, ty) = parse_math_function(&f.name, &f.value)?;
        return (ty == CalcType::Angle).then(|| finite(node.eval(0.0).0.to_degrees()));
    }
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

/// Math functions: `calc()`, `min()`, `max()`, `clamp()` y las de [`parse_math_fn`].
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
            let (b, tb) = parse_calc_sum(trim(b))?;
            // `none` en un extremo: sin límite por ese lado.
            let bound = |g: &[ComponentValue], inf: f32| -> Option<(CalcNode, Option<CalcType>)> {
                if single_ident(g).is_some_and(|s| s.eq_ignore_ascii_case("none")) {
                    return Some((CalcNode::Number(inf), None));
                }
                let (n, t) = parse_calc_sum(trim(g))?;
                Some((n, Some(t)))
            };
            let (a, ta) = bound(a, f32::NEG_INFINITY)?;
            let (c, tc) = bound(c, f32::INFINITY)?;
            let mut ty = tb;
            for t in [ta, tc].into_iter().flatten() {
                ty = ty.add(t)?;
            }
            Some((CalcNode::Clamp(Box::new(a), Box::new(b), Box::new(c)), ty))
        }
        other => parse_math_fn(other, args),
    }
}

/// Las math functions de CSS Values 4 §10.5–§10.8 (sus argumentos son `<calc-sum>`).
fn parse_math_fn(name: &str, args: &[ComponentValue]) -> Option<(CalcNode, CalcType)> {
    let groups = bf_css::syntax::split_comma_separated(args);
    let mut groups: &[Vec<ComponentValue>] = &groups;
    let mut f = match name {
        "round" => MathFn::Round(Rounding::Nearest),
        "mod" => MathFn::Mod,
        "rem" => MathFn::Rem,
        "sin" => MathFn::Sin,
        "cos" => MathFn::Cos,
        "tan" => MathFn::Tan,
        "asin" => MathFn::Asin,
        "acos" => MathFn::Acos,
        "atan" => MathFn::Atan,
        "atan2" => MathFn::Atan2,
        "pow" => MathFn::Pow,
        "sqrt" => MathFn::Sqrt,
        "hypot" => MathFn::Hypot,
        "log" => MathFn::Log,
        "exp" => MathFn::Exp,
        "abs" => MathFn::Abs,
        "sign" => MathFn::Sign,
        _ => return None,
    };
    if let MathFn::Round(_) = f
        && let Some(first) = groups.first()
        && let Some(s) = single_ident(first)
    {
        let r = match s.to_ascii_lowercase().as_str() {
            "nearest" => Some(Rounding::Nearest),
            "up" => Some(Rounding::Up),
            "down" => Some(Rounding::Down),
            "to-zero" => Some(Rounding::ToZero),
            // Puede ser una constante (`round(pi)`) en vez de una estrategia.
            _ => None,
        };
        if let Some(r) = r {
            f = MathFn::Round(r);
            groups = &groups[1..];
        }
    }
    let mut nodes = Vec::with_capacity(groups.len());
    let mut types = Vec::with_capacity(groups.len());
    for g in groups {
        let (n, t) = parse_calc_sum(trim(g))?;
        nodes.push(n);
        types.push(t);
    }
    // El tipo común de los argumentos (deben poder sumarse).
    let common = |types: &[CalcType]| -> Option<CalcType> {
        let (first, rest) = types.split_first()?;
        rest.iter().try_fold(*first, |acc, t| acc.add(*t))
    };
    use CalcType::{Angle, Number};
    let ty = match (f, types.as_slice()) {
        (MathFn::Round(_), [Number]) => {
            nodes.push(CalcNode::Number(1.0));
            Number
        }
        (MathFn::Round(_) | MathFn::Mod | MathFn::Rem, [_, _]) => common(&types)?,
        (MathFn::Sin | MathFn::Cos | MathFn::Tan, [Number | Angle]) => Number,
        (MathFn::Asin | MathFn::Acos | MathFn::Atan, [Number]) => Angle,
        (MathFn::Atan2, [_, _]) => {
            common(&types)?;
            Angle
        }
        (MathFn::Pow, [Number, Number]) | (MathFn::Log, [Number] | [Number, Number]) => Number,
        (MathFn::Sqrt | MathFn::Exp, [Number]) => Number,
        (MathFn::Hypot, [_, ..]) => common(&types)?,
        (MathFn::Abs, [t]) => *t,
        (MathFn::Sign, [_]) => Number,
        _ => return None,
    };
    Some((CalcNode::Func(f, nodes), ty))
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
            // CSS Values 4 §10.9: el divisor puede tener tipo; con el mismo tipo que el
            // dividendo (longitudes o ángulos) el cociente es un número.
            TokenKind::Delim('/') => {
                match (ty, t) {
                    (_, CalcType::Number) => {}
                    (CalcType::Length, CalcType::Length) | (CalcType::Angle, CalcType::Angle) => {
                        ty = CalcType::Number;
                    }
                    _ => return None,
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
            TokenKind::Dimension(n, unit) => match numeric_len(n, unit) {
                Some(l) => Some((CalcNode::Length(l), CalcType::Length)),
                None => {
                    let deg = parse_angle(v)?;
                    Some((CalcNode::Number(deg.to_radians()), CalcType::Angle))
                }
            },
            // `<calc-keyword>` (CSS Values 4 §10.7.1).
            TokenKind::Ident(i) => {
                let n = match i.to_ascii_lowercase().as_str() {
                    "pi" => std::f32::consts::PI,
                    "e" => std::f32::consts::E,
                    "infinity" => f32::INFINITY,
                    "-infinity" => f32::NEG_INFINITY,
                    "nan" => f32::NAN,
                    _ => return None,
                };
                Some((CalcNode::Number(n), CalcType::Number))
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
    fn math_angles_and_numbers() {
        let angle = |s: &str| parse_angle(&parse_component_value(s).unwrap());
        let number = |s: &str| parse_number(&parse_component_value(s).unwrap());
        let close = |a: Option<f32>, b: f32| a.is_some_and(|a| (a - b).abs() < 1e-3);
        assert!(close(angle("calc(90deg + atan(1))"), 135.0));
        assert!(close(angle("calc(0.5turn)"), 180.0));
        assert!(close(angle("acos(0)"), 90.0));
        assert_eq!(angle("calc(1)"), None);
        assert!(close(number("sin(90deg)"), 1.0));
        assert!(close(number("round(2.5)"), 3.0));
        assert!(close(number("round(-2.5)"), -2.0));
        assert!(close(number("sign(-0.1)"), -1.0));
        assert!(close(number("calc(pi)"), std::f32::consts::PI));
        assert!(close(number("calc(round(pi))"), 3.0));
        assert_eq!(number("sin(1px)"), None);
        // Percentages en math functions: se resuelven al usarse.
        assert_eq!(
            lp("round(55%, 10px)")
                .compute(&LengthContext::default())
                .resolve(100.0),
            60.0
        );
        assert_eq!(
            lp("abs(-50%)")
                .compute(&LengthContext::default())
                .resolve(10.0),
            5.0
        );
        // Serialización de las nuevas funciones (el CSSOM las usa en el valor especificado).
        assert_eq!(lp("round(up, 50%, 3px)").to_string(), "round(up, 50%, 3px)");
        assert_eq!(lp("min(10px, 5%)").to_string(), "min(10px, 5%)");
        assert_eq!(lp("calc(10px + 5%)").to_string(), "calc(10px + 5%)");
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
