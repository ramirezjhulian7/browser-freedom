//! `<gradient>` de CSS Images 3 §3 y CSS Images 4 §3: `linear-gradient()`,
//! `radial-gradient()`, `conic-gradient()` y sus variantes `repeating-*`.
//!
//! El `<color-interpolation-method>` (`in <espacio>`) se acepta pero se interpola siempre en
//! sRGB premultiplicado: es el espacio que usan los navegadores para los colores legacy (CSS
//! Color 4 §12.1).

use std::fmt::Write as _;

use bf_css::syntax::{ComponentValue, split_comma_separated};

use super::ComputeContext;
use super::types::{PositionAxis, parse_bg_position};
use crate::values::color::parse_color;
use crate::values::{
    Color, LengthPercentage, components, is_ident, parse_angle, parse_length_percentage,
};

/// Dirección de un gradiente lineal (§3.1.1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineDirection {
    /// Ángulo en grados (0deg = hacia arriba, sentido horario).
    Angle(f32),
    /// `to <side-or-corner>`: `x` = -1 (left), 0 o 1 (right); `y` = -1 (top), 0 o 1 (bottom).
    To { x: i8, y: i8 },
}

/// `<ending-shape>` de un gradiente radial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndingShape {
    Circle,
    Ellipse,
}

/// `<radial-size>` (§3.2.1).
#[derive(Debug, Clone, PartialEq)]
pub enum RadialSize {
    ClosestSide,
    FarthestSide,
    ClosestCorner,
    FarthestCorner,
    /// Radio de un círculo (solo `<length>`).
    Circle(LengthPercentage),
    /// Radios horizontal y vertical de una elipse.
    Ellipse(LengthPercentage, LengthPercentage),
}

/// Posición de una parada: `<length-percentage>` en lineales y radiales, `<angle-percentage>`
/// en cónicos (los porcentajes van como `Lp`).
#[derive(Debug, Clone, PartialEq)]
pub enum StopPosition {
    Lp(LengthPercentage),
    /// Grados.
    Angle(f32),
}

/// Un elemento de la lista de paradas (§3.4).
#[derive(Debug, Clone, PartialEq)]
pub enum ColorStop {
    Color {
        color: Color,
        position: Option<StopPosition>,
    },
    /// Indicación de interpolación (`<color-hint>`) entre dos paradas.
    Hint(StopPosition),
}

#[derive(Debug, Clone, PartialEq)]
pub enum GradientKind {
    Linear(LineDirection),
    Radial {
        shape: EndingShape,
        size: RadialSize,
        position: (PositionAxis, PositionAxis),
    },
    Conic {
        /// Ángulo inicial en grados.
        from: f32,
        position: (PositionAxis, PositionAxis),
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Gradient {
    pub kind: GradientKind,
    pub repeating: bool,
    pub stops: Vec<ColorStop>,
}

fn center() -> (PositionAxis, PositionAxis) {
    let half = || PositionAxis {
        from_end: false,
        offset: LengthPercentage::Percentage(50.0),
    };
    (half(), half())
}

/// `in <color-space> [<hue-interpolation-method> hue]?` al inicio de `items`: devuelve cuántos
/// component values consume (0 si no hay).
fn interpolation_method(items: &[&ComponentValue]) -> Option<usize> {
    if !items.first().is_some_and(|v| is_ident(v, "in")) {
        return Some(0);
    }
    let space = items.get(1)?;
    const SPACES: [&str; 13] = [
        "srgb",
        "srgb-linear",
        "display-p3",
        "a98-rgb",
        "prophoto-rgb",
        "rec2020",
        "lab",
        "oklab",
        "xyz",
        "xyz-d50",
        "xyz-d65",
        "hsl",
        "hwb",
    ];
    let polar = ["hsl", "hwb", "lch", "oklch"];
    if !SPACES
        .iter()
        .chain(&["lch", "oklch"])
        .any(|s| is_ident(space, s))
    {
        return None;
    }
    let mut n = 2;
    if polar.iter().any(|s| is_ident(space, s))
        && let (Some(m), Some(h)) = (items.get(2), items.get(3))
        && ["shorter", "longer", "increasing", "decreasing"]
            .iter()
            .any(|k| is_ident(m, k))
        && is_ident(h, "hue")
    {
        n = 4;
    }
    Some(n)
}

/// `at <position>` al inicio de `items`: la posición y cuántos consume.
fn at_position(items: &[&ComponentValue]) -> Option<((PositionAxis, PositionAxis), usize)> {
    if !items.first().is_some_and(|v| is_ident(v, "at")) {
        return None;
    }
    // La posición es lo que sigue hasta un `in` (o el final).
    let end = items
        .iter()
        .skip(1)
        .position(|v| is_ident(v, "in"))
        .map_or(items.len(), |p| p + 1);
    let run: Vec<ComponentValue> = items[1..end].iter().map(|v| (*v).clone()).collect();
    if run.is_empty() {
        return None;
    }
    Some((parse_bg_position(&run)?, end))
}

fn parse_position(v: &ComponentValue, angular: bool) -> Option<StopPosition> {
    if angular {
        if let Some(a) = parse_angle(v) {
            return Some(StopPosition::Angle(a));
        }
        return match parse_length_percentage(v)? {
            lp @ LengthPercentage::Percentage(_) => Some(StopPosition::Lp(lp)),
            _ => None,
        };
    }
    parse_length_percentage(v).map(StopPosition::Lp)
}

/// `<color-stop-list>` (o `<angular-color-stop-list>`), con las paradas de dos posiciones
/// desdobladas.
fn parse_stops(args: &[Vec<ComponentValue>], angular: bool) -> Option<Vec<ColorStop>> {
    let mut out = Vec::new();
    for arg in args {
        let items = components(arg);
        match items.as_slice() {
            // <color-hint>
            [p] if parse_color(p).is_none() => {
                out.push(ColorStop::Hint(parse_position(p, angular)?))
            }
            _ => {
                // <color> && <position>{0,2} (la posición puede ir antes del color).
                let ci = items.iter().position(|v| parse_color(v).is_some())?;
                let color = parse_color(items[ci])?;
                let positions: Vec<StopPosition> = items
                    .iter()
                    .enumerate()
                    .filter(|&(i, _)| i != ci)
                    .map(|(_, v)| parse_position(v, angular))
                    .collect::<Option<_>>()?;
                // Las posiciones van juntas, todas antes o todas después del color.
                if positions.len() > 2 || (ci != 0 && ci != items.len() - 1) {
                    return None;
                }
                if positions.is_empty() {
                    out.push(ColorStop::Color {
                        color,
                        position: None,
                    });
                }
                for p in positions {
                    out.push(ColorStop::Color {
                        color,
                        position: Some(p),
                    });
                }
            }
        }
    }
    // Al menos una parada; las indicaciones solo entre dos paradas.
    let is_hint = |s: &ColorStop| matches!(s, ColorStop::Hint(_));
    if out.is_empty()
        || out.first().is_some_and(is_hint)
        || out.last().is_some_and(is_hint)
        || out.windows(2).any(|w| is_hint(&w[0]) && is_hint(&w[1]))
    {
        return None;
    }
    Some(out)
}

fn parse_linear(args: &[Vec<ComponentValue>]) -> Option<(GradientKind, usize)> {
    let first = components(&args[0]);
    let mut dir = LineDirection::Angle(180.0);
    let mut consumed_args = 0;
    let mut i = 0;
    let mut matched = false;
    while i < first.len() {
        let v = first[i];
        if is_ident(v, "to") {
            let mut x = 0i8;
            let mut y = 0i8;
            let mut j = i + 1;
            while j < first.len() && j < i + 3 {
                let w = first[j];
                if x == 0 && is_ident(w, "left") {
                    x = -1;
                } else if x == 0 && is_ident(w, "right") {
                    x = 1;
                } else if y == 0 && is_ident(w, "top") {
                    y = -1;
                } else if y == 0 && is_ident(w, "bottom") {
                    y = 1;
                } else {
                    break;
                }
                j += 1;
            }
            if j == i + 1 {
                return None;
            }
            dir = LineDirection::To { x, y };
            i = j;
            matched = true;
            continue;
        }
        if let Some(a) = parse_angle(v) {
            dir = LineDirection::Angle(a);
            i += 1;
            matched = true;
            continue;
        }
        let n = interpolation_method(&first[i..])?;
        if n == 0 {
            break;
        }
        i += n;
        matched = true;
    }
    if matched {
        if i != first.len() {
            return None;
        }
        consumed_args = 1;
    }
    Some((GradientKind::Linear(dir), consumed_args))
}

fn parse_radial(args: &[Vec<ComponentValue>]) -> Option<(GradientKind, usize)> {
    let first = components(&args[0]);
    let mut shape = None;
    let mut extent = None;
    let mut lengths: Vec<LengthPercentage> = Vec::new();
    let mut position = None;
    let mut i = 0;
    let mut matched = false;
    while i < first.len() {
        let v = first[i];
        if shape.is_none() && is_ident(v, "circle") {
            shape = Some(EndingShape::Circle);
        } else if shape.is_none() && is_ident(v, "ellipse") {
            shape = Some(EndingShape::Ellipse);
        } else if extent.is_none()
            && lengths.is_empty()
            && let Some(e) = [
                ("closest-side", RadialSize::ClosestSide),
                ("farthest-side", RadialSize::FarthestSide),
                ("closest-corner", RadialSize::ClosestCorner),
                ("farthest-corner", RadialSize::FarthestCorner),
            ]
            .into_iter()
            .find(|(k, _)| is_ident(v, k))
        {
            extent = Some(e.1);
        } else if extent.is_none()
            && lengths.len() < 2
            && let Some(lp) = parse_length_percentage(v)
        {
            if matches!(lp, LengthPercentage::Length(l) if l.value < 0.0)
                || matches!(lp, LengthPercentage::Percentage(p) if p < 0.0)
            {
                return None;
            }
            lengths.push(lp);
        } else if is_ident(v, "at") {
            let (p, n) = at_position(&first[i..])?;
            position = Some(p);
            i += n;
            matched = true;
            continue;
        } else {
            let n = interpolation_method(&first[i..])?;
            if n == 0 {
                return if matched || i > 0 {
                    None
                } else {
                    Some(default_radial())
                };
            }
            i += n;
            matched = true;
            continue;
        }
        i += 1;
        matched = true;
    }
    if !matched {
        return Some(default_radial());
    }
    let size = match (shape, extent, lengths.as_slice()) {
        (_, Some(e), []) => e,
        (Some(EndingShape::Circle) | None, None, [r]) => {
            if r.has_percentage() {
                return None;
            }
            shape = Some(EndingShape::Circle);
            RadialSize::Circle(r.clone())
        }
        (Some(EndingShape::Ellipse) | None, None, [a, b]) => {
            shape = Some(EndingShape::Ellipse);
            RadialSize::Ellipse(a.clone(), b.clone())
        }
        (_, None, []) => RadialSize::FarthestCorner,
        _ => return None,
    };
    let (kind, _) = default_radial();
    let GradientKind::Radial { .. } = kind else {
        unreachable!()
    };
    Some((
        GradientKind::Radial {
            shape: shape.unwrap_or(EndingShape::Ellipse),
            size,
            position: position.unwrap_or_else(center),
        },
        1,
    ))
}

fn default_radial() -> (GradientKind, usize) {
    (
        GradientKind::Radial {
            shape: EndingShape::Ellipse,
            size: RadialSize::FarthestCorner,
            position: center(),
        },
        0,
    )
}

fn parse_conic(args: &[Vec<ComponentValue>]) -> Option<(GradientKind, usize)> {
    let first = components(&args[0]);
    let mut from = None;
    let mut position = None;
    let mut i = 0;
    while i < first.len() {
        let v = first[i];
        if from.is_none() && is_ident(v, "from") {
            from = Some(parse_angle(first.get(i + 1)?)?);
            i += 2;
        } else if position.is_none() && is_ident(v, "at") {
            let (p, n) = at_position(&first[i..])?;
            position = Some(p);
            i += n;
        } else {
            let n = interpolation_method(&first[i..])?;
            if n == 0 {
                if i == 0 {
                    break;
                }
                return None;
            }
            i += n;
        }
    }
    let consumed = usize::from(i > 0);
    Some((
        GradientKind::Conic {
            from: from.unwrap_or(0.0),
            position: position.unwrap_or_else(center),
        },
        consumed,
    ))
}

impl Gradient {
    /// Parsea una función de gradiente.
    pub fn parse(v: &ComponentValue) -> Option<Gradient> {
        let ComponentValue::Function(f) = v else {
            return None;
        };
        let name = f.name.to_ascii_lowercase();
        let (repeating, base) = match name.strip_prefix("repeating-") {
            Some(b) => (true, b),
            None => (false, name.as_str()),
        };
        let args = split_comma_separated(&f.value);
        if args.is_empty() || args.iter().any(|a| components(a).is_empty()) {
            return None;
        }
        let (kind, consumed, angular) = match base {
            "linear-gradient" => {
                let (k, n) = parse_linear(&args)?;
                (k, n, false)
            }
            "radial-gradient" => {
                let (k, n) = parse_radial(&args)?;
                (k, n, false)
            }
            "conic-gradient" => {
                let (k, n) = parse_conic(&args)?;
                (k, n, true)
            }
            _ => return None,
        };
        let stops = parse_stops(&args[consumed..], angular)?;
        Some(Gradient {
            kind,
            repeating,
            stops,
        })
    }

    pub(crate) fn compute(&self, ctx: &ComputeContext<'_>) -> Gradient {
        let lp = |l: &LengthPercentage| l.compute(&ctx.len);
        let axis = |a: &PositionAxis| PositionAxis {
            from_end: a.from_end,
            offset: lp(&a.offset),
        };
        let kind = match &self.kind {
            GradientKind::Linear(d) => GradientKind::Linear(*d),
            GradientKind::Radial {
                shape,
                size,
                position,
            } => GradientKind::Radial {
                shape: *shape,
                size: match size {
                    RadialSize::Circle(r) => RadialSize::Circle(lp(r)),
                    RadialSize::Ellipse(a, b) => RadialSize::Ellipse(lp(a), lp(b)),
                    other => other.clone(),
                },
                position: (axis(&position.0), axis(&position.1)),
            },
            GradientKind::Conic { from, position } => GradientKind::Conic {
                from: *from,
                position: (axis(&position.0), axis(&position.1)),
            },
        };
        let pos = |p: &StopPosition| match p {
            StopPosition::Lp(l) => StopPosition::Lp(lp(l)),
            a => a.clone(),
        };
        let stops = self
            .stops
            .iter()
            .map(|s| match s {
                ColorStop::Color { color, position } => ColorStop::Color {
                    color: *color,
                    position: position.as_ref().map(pos),
                },
                ColorStop::Hint(p) => ColorStop::Hint(pos(p)),
            })
            .collect();
        Gradient {
            kind,
            repeating: self.repeating,
            stops,
        }
    }

    pub(crate) fn to_css(&self) -> String {
        let mut s = String::new();
        if self.repeating {
            s.push_str("repeating-");
        }
        let pos_css = |p: &StopPosition| match p {
            StopPosition::Lp(l) => l.to_string(),
            StopPosition::Angle(a) => format!("{}deg", crate::values::fmt_number(*a)),
        };
        let axis = |p: &(PositionAxis, PositionAxis)| {
            let a = |x: &PositionAxis, end: &str| {
                if x.from_end {
                    format!("{end} {}", x.offset)
                } else {
                    x.offset.to_string()
                }
            };
            format!("{} {}", a(&p.0, "right"), a(&p.1, "bottom"))
        };
        match &self.kind {
            GradientKind::Linear(d) => {
                s.push_str("linear-gradient(");
                match d {
                    LineDirection::Angle(a) => {
                        let _ = write!(s, "{}deg, ", crate::values::fmt_number(*a));
                    }
                    LineDirection::To { x, y } => {
                        s.push_str("to");
                        if *y != 0 {
                            s.push_str(if *y < 0 { " top" } else { " bottom" });
                        }
                        if *x != 0 {
                            s.push_str(if *x < 0 { " left" } else { " right" });
                        }
                        s.push_str(", ");
                    }
                }
            }
            GradientKind::Radial {
                shape,
                size,
                position,
            } => {
                s.push_str("radial-gradient(");
                s.push_str(match shape {
                    EndingShape::Circle => "circle ",
                    EndingShape::Ellipse => "ellipse ",
                });
                match size {
                    RadialSize::ClosestSide => s.push_str("closest-side"),
                    RadialSize::FarthestSide => s.push_str("farthest-side"),
                    RadialSize::ClosestCorner => s.push_str("closest-corner"),
                    RadialSize::FarthestCorner => s.push_str("farthest-corner"),
                    RadialSize::Circle(r) => s.push_str(&r.to_string()),
                    RadialSize::Ellipse(a, b) => {
                        let _ = write!(s, "{a} {b}");
                    }
                }
                let _ = write!(s, " at {}, ", axis(position));
            }
            GradientKind::Conic { from, position } => {
                let _ = write!(
                    s,
                    "conic-gradient(from {}deg at {}, ",
                    crate::values::fmt_number(*from),
                    axis(position)
                );
            }
        }
        let stops: Vec<String> = self
            .stops
            .iter()
            .map(|st| match st {
                ColorStop::Color { color, position } => match position {
                    Some(p) => format!("{color} {}", pos_css(p)),
                    None => color.to_string(),
                },
                ColorStop::Hint(p) => pos_css(p),
            })
            .collect();
        s.push_str(&stops.join(", "));
        s.push(')');
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Option<Gradient> {
        let tokens = bf_css::syntax::parse_component_value_list(s);
        let items = components(&tokens);
        match items.as_slice() {
            [v] => Gradient::parse(v),
            _ => None,
        }
    }

    #[test]
    fn linear() {
        let g = parse("linear-gradient(red, blue)").expect("válido");
        assert_eq!(g.kind, GradientKind::Linear(LineDirection::Angle(180.0)));
        assert_eq!(g.stops.len(), 2);
        let g = parse("linear-gradient(to top right, red 10% 20%, 30%, blue)").expect("válido");
        assert_eq!(
            g.kind,
            GradientKind::Linear(LineDirection::To { x: 1, y: -1 })
        );
        assert_eq!(g.stops.len(), 4);
        assert!(parse("linear-gradient(45deg in oklab, red, blue)").is_some());
        assert!(parse("repeating-linear-gradient(0.25turn, red 0 10px, blue 10px 20px)").is_some());
        assert!(parse("linear-gradient(red, 10%)").is_none());
        assert!(parse("linear-gradient(to middle, red, blue)").is_none());
        assert!(parse("linear-gradient(red)").is_some());
    }

    #[test]
    fn radial_and_conic() {
        let g = parse("radial-gradient(circle 20px at 10px 30%, red, blue)").expect("válido");
        let GradientKind::Radial { shape, size, .. } = g.kind else {
            panic!("radial")
        };
        assert_eq!(shape, EndingShape::Circle);
        assert_eq!(size, RadialSize::Circle(LengthPercentage::px(20.0)));
        assert!(parse("radial-gradient(closest-side, red, blue)").is_some());
        assert!(parse("radial-gradient(50% 20px, red, blue)").is_some());
        assert!(parse("radial-gradient(circle 50%, red, blue)").is_none());
        assert!(parse("radial-gradient(red, blue)").is_some());
        let g = parse("conic-gradient(from 90deg at 25% 25%, red, blue 50%, green 270deg)")
            .expect("válido");
        assert!(matches!(g.kind, GradientKind::Conic { from, .. } if from == 90.0));
        assert!(parse("conic-gradient(red 10px, blue)").is_none());
    }
}
