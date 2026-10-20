//! `<color>` de CSS Color 4: hex, colores con nombre, `transparent`, `currentcolor`, colores del
//! sistema, `rgb()`/`rgba()` (sintaxis legacy y moderna), `hsl()`/`hsla()` y `hwb()`.

use std::fmt;

use bf_css::syntax::{ComponentValue, TokenKind};

use super::named_colors::NAMED_COLORS;
use super::{components, fmt_number, parse_angle};

/// Color sRGB: `r`, `g`, `b` en [0, 255] y `a` en [0, 1].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub const BLACK: Rgba = Rgba::rgb(0, 0, 0);
    pub const WHITE: Rgba = Rgba::rgb(255, 255, 255);
    pub const TRANSPARENT: Rgba = Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };

    pub const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
        Rgba {
            r: r as f32,
            g: g as f32,
            b: b as f32,
            a: 1.0,
        }
    }
}

impl fmt::Display for Rgba {
    /// Serialización de CSSOM: `rgb(r, g, b)` o `rgba(r, g, b, a)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let c = |v: f32| v.round().clamp(0.0, 255.0) as u8;
        if self.a >= 1.0 {
            write!(f, "rgb({}, {}, {})", c(self.r), c(self.g), c(self.b))
        } else {
            write!(
                f,
                "rgba({}, {}, {}, {})",
                c(self.r),
                c(self.g),
                c(self.b),
                fmt_number(self.a)
            )
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Color {
    Rgba(Rgba),
    /// Se resuelve contra la propiedad `color` al usarse (o al heredar en `color` mismo).
    CurrentColor,
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Color::Rgba(c) => write!(f, "{c}"),
            Color::CurrentColor => f.write_str("currentcolor"),
        }
    }
}

impl Color {
    pub fn resolve(self, current: Rgba) -> Rgba {
        match self {
            Color::Rgba(c) => c,
            Color::CurrentColor => current,
        }
    }
}

/// Colores del sistema (CSS Color 4 §6.2), con valores de un esquema claro. Los obsoletos de
/// §6.3 se mapean a su reemplazo.
fn system_color(name: &str) -> Option<Rgba> {
    Some(match name.to_ascii_lowercase().as_str() {
        "canvas" | "field" | "buttonhighlight" | "window" => Rgba::WHITE,
        "canvastext" | "fieldtext" | "buttontext" | "windowtext" | "marktext"
        | "selecteditemtext" => Rgba::BLACK,
        "linktext" => Rgba::rgb(0, 0, 0xEE),
        "visitedtext" => Rgba::rgb(0x55, 0x1A, 0x8B),
        "activetext" => Rgba::rgb(0xFF, 0, 0),
        "buttonface" | "threedface" | "threedlightshadow" => Rgba::rgb(0xEF, 0xEF, 0xEF),
        "buttonborder" | "threeddarkshadow" | "threedshadow" | "threedhighlight" => {
            Rgba::rgb(0x76, 0x76, 0x76)
        }
        "graytext" => Rgba::rgb(0x6D, 0x6D, 0x6D),
        "highlight" | "selecteditem" => Rgba::rgb(0x33, 0x99, 0xFF),
        "highlighttext" | "accentcolortext" => Rgba::WHITE,
        "mark" => Rgba::rgb(0xFF, 0xFF, 0),
        "accentcolor" => Rgba::rgb(0, 0x75, 0xFF),
        _ => return None,
    })
}

pub fn named_color(name: &str) -> Option<Rgba> {
    let lower = name.to_ascii_lowercase();
    NAMED_COLORS
        .binary_search_by_key(&lower.as_str(), |(n, _)| n)
        .ok()
        .map(|i| {
            let [r, g, b] = NAMED_COLORS[i].1;
            Rgba::rgb(r, g, b)
        })
}

/// `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`.
pub fn parse_hex(hex: &str) -> Option<Rgba> {
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |i: usize| u8::from_str_radix(&hex[i..=i], 16).ok();
    let pair = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    let (r, g, b, a) = match hex.len() {
        3 | 4 => (
            digit(0)? * 17,
            digit(1)? * 17,
            digit(2)? * 17,
            if hex.len() == 4 { digit(3)? * 17 } else { 255 },
        ),
        6 | 8 => (
            pair(0)?,
            pair(2)?,
            pair(4)?,
            if hex.len() == 8 { pair(6)? } else { 255 },
        ),
        _ => return None,
    };
    Some(Rgba {
        r: f32::from(r),
        g: f32::from(g),
        b: f32::from(b),
        a: f32::from(a) / 255.0,
    })
}

/// Parsea un `<color>`.
pub fn parse_color(v: &ComponentValue) -> Option<Color> {
    match v {
        ComponentValue::Token(t) => match &t.kind {
            TokenKind::Hash { value, .. } => parse_hex(value).map(Color::Rgba),
            TokenKind::Ident(name) => {
                if name.eq_ignore_ascii_case("currentcolor") {
                    Some(Color::CurrentColor)
                } else if name.eq_ignore_ascii_case("transparent") {
                    Some(Color::Rgba(Rgba::TRANSPARENT))
                } else {
                    named_color(name)
                        .or_else(|| system_color(name))
                        .map(Color::Rgba)
                }
            }
            _ => None,
        },
        ComponentValue::Function(f) => {
            let name = f.name.to_ascii_lowercase();
            match name.as_str() {
                "rgb" | "rgba" => parse_rgb(&f.value).map(Color::Rgba),
                "hsl" | "hsla" => parse_hsl(&f.value).map(Color::Rgba),
                "hwb" => parse_hwb(&f.value).map(Color::Rgba),
                // CSS Color 5: `light-dark(<color>, <color>)`. El esquema usado es siempre
                // `light` (no hay preferencia de esquema oscuro todavía).
                "light-dark" => {
                    let args = bf_css::syntax::split_comma_separated(&f.value);
                    let [light, dark] = args.as_slice() else {
                        return None;
                    };
                    let one = |a: &[ComponentValue]| {
                        let items: Vec<&ComponentValue> =
                            a.iter().filter(|v| !v.is_whitespace()).collect();
                        match items.as_slice() {
                            [v] => parse_color(v),
                            _ => None,
                        }
                    };
                    one(dark)?;
                    one(light)
                }
                _ => None,
            }
        }
        ComponentValue::Block(_) => None,
    }
}

/// Un argumento de una función de color: número, porcentaje o `none` (= 0).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Arg {
    Number(f32),
    Percentage(f32),
    None,
    Angle(f32),
}

fn arg(v: &ComponentValue) -> Option<Arg> {
    match v.token()? {
        TokenKind::Number(n) => Some(Arg::Number(n.value as f32)),
        TokenKind::Percentage(p) => Some(Arg::Percentage(p.value as f32)),
        TokenKind::Ident(i) if i.eq_ignore_ascii_case("none") => Some(Arg::None),
        TokenKind::Dimension(..) => parse_angle(v).map(Arg::Angle),
        _ => None,
    }
}

/// Separa los argumentos: sintaxis legacy (con comas, `legacy = true`) o moderna (espacios y
/// `/ alpha`). Devuelve los canales y el alfa opcional.
fn split_args(values: &[ComponentValue]) -> Option<(Vec<Arg>, Option<Arg>, bool)> {
    let items = components(values);
    if items.iter().any(|v| v.token() == Some(&TokenKind::Comma)) {
        // Legacy: a, b, c[, alpha] — sin `none`.
        let mut channels = Vec::new();
        for (i, item) in items.iter().enumerate() {
            if i % 2 == 1 {
                if item.token() != Some(&TokenKind::Comma) {
                    return None;
                }
                continue;
            }
            let a = arg(item)?;
            if a == Arg::None {
                return None;
            }
            channels.push(a);
        }
        if items.len().is_multiple_of(2) {
            return None;
        }
        let alpha = match channels.len() {
            3 => None,
            4 => channels.pop(),
            _ => return None,
        };
        return Some((channels, alpha, true));
    }
    let slash = items
        .iter()
        .position(|v| v.token() == Some(&TokenKind::Delim('/')));
    let (channel_items, alpha) = match slash {
        Some(i) => {
            let [a] = &items[i + 1..] else {
                return None;
            };
            (&items[..i], Some(arg(a)?))
        }
        None => (&items[..], None),
    };
    let channels = channel_items
        .iter()
        .map(|v| arg(v))
        .collect::<Option<Vec<_>>>()?;
    (channels.len() == 3).then_some((channels, alpha, false))
}

fn alpha_value(a: Option<Arg>) -> Option<f32> {
    Some(
        match a {
            None => 1.0,
            Some(Arg::Number(n)) => n,
            Some(Arg::Percentage(p)) => p / 100.0,
            Some(Arg::None) => 0.0,
            Some(Arg::Angle(_)) => return None,
        }
        .clamp(0.0, 1.0),
    )
}

fn parse_rgb(values: &[ComponentValue]) -> Option<Rgba> {
    let (channels, alpha, legacy) = split_args(values)?;
    if legacy {
        // Todos números o todos porcentajes.
        let all_numbers = channels.iter().all(|c| matches!(c, Arg::Number(_)));
        let all_percent = channels.iter().all(|c| matches!(c, Arg::Percentage(_)));
        if !all_numbers && !all_percent {
            return None;
        }
    }
    let channel = |c: Arg| -> Option<f32> {
        Some(
            match c {
                Arg::Number(n) => n,
                Arg::Percentage(p) => p * 2.55,
                Arg::None => 0.0,
                Arg::Angle(_) => return None,
            }
            .clamp(0.0, 255.0),
        )
    };
    Some(Rgba {
        r: channel(channels[0])?,
        g: channel(channels[1])?,
        b: channel(channels[2])?,
        a: alpha_value(alpha)?,
    })
}

fn hue(a: Arg) -> Option<f32> {
    match a {
        Arg::Number(n) | Arg::Angle(n) => Some(n.rem_euclid(360.0)),
        Arg::None => Some(0.0),
        Arg::Percentage(_) => None,
    }
}

fn percent_or_number(a: Arg, legacy: bool) -> Option<f32> {
    match a {
        Arg::Percentage(p) => Some(p),
        Arg::Number(n) if !legacy => Some(n),
        Arg::None => Some(0.0),
        _ => None,
    }
}

/// HSL → sRGB (CSS Color 4 §7.1).
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    let f = |n: f32| {
        let k = (n + h / 30.0) % 12.0;
        let a = s * l.min(1.0 - l);
        l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)
    };
    (f(0.0), f(8.0), f(4.0))
}

fn parse_hsl(values: &[ComponentValue]) -> Option<Rgba> {
    let (channels, alpha, legacy) = split_args(values)?;
    let h = hue(channels[0])?;
    let s = percent_or_number(channels[1], legacy)?.clamp(0.0, 100.0) / 100.0;
    let l = percent_or_number(channels[2], legacy)?.clamp(0.0, 100.0) / 100.0;
    let (r, g, b) = hsl_to_rgb(h, s, l);
    Some(Rgba {
        r: r * 255.0,
        g: g * 255.0,
        b: b * 255.0,
        a: alpha_value(alpha)?,
    })
}

fn parse_hwb(values: &[ComponentValue]) -> Option<Rgba> {
    let (channels, alpha, legacy) = split_args(values)?;
    if legacy {
        return None;
    }
    let h = hue(channels[0])?;
    let w = percent_or_number(channels[1], false)?.clamp(0.0, 100.0) / 100.0;
    let bl = percent_or_number(channels[2], false)?.clamp(0.0, 100.0) / 100.0;
    if w + bl >= 1.0 {
        let gray = w / (w + bl);
        return Some(Rgba {
            r: gray * 255.0,
            g: gray * 255.0,
            b: gray * 255.0,
            a: alpha_value(alpha)?,
        });
    }
    let (r, g, b) = hsl_to_rgb(h, 1.0, 0.5);
    let scale = |c: f32| c * (1.0 - w - bl) + w;
    let (r, g, b) = (scale(r), scale(g), scale(b));
    Some(Rgba {
        r: r * 255.0,
        g: g * 255.0,
        b: b * 255.0,
        a: alpha_value(alpha)?,
    })
}

/// Rules for parsing a legacy color value (HTML §2.3.6.4), para `bgcolor`, `color`, etc.
pub fn parse_legacy_color(input: &str) -> Option<Rgba> {
    let input = input.trim_matches(|c: char| matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' '));
    if input.is_empty() || input.eq_ignore_ascii_case("transparent") {
        return None;
    }
    if let Some(c) = named_color(input) {
        return Some(c);
    }
    if input.len() == 4
        && let Some(hex) = input.strip_prefix('#')
        && hex.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return parse_hex(hex);
    }
    // Algoritmo de "rescate" del spec.
    let mut chars: Vec<char> = input
        .chars()
        .flat_map(|c| {
            if (c as u32) > 0xFFFF {
                vec!['0', '0']
            } else {
                vec![c]
            }
        })
        .collect();
    chars.truncate(128);
    if chars.first() == Some(&'#') {
        chars.remove(0);
    }
    for c in &mut chars {
        if !c.is_ascii_hexdigit() {
            *c = '0';
        }
    }
    while chars.is_empty() || !chars.len().is_multiple_of(3) {
        chars.push('0');
    }
    let len = chars.len() / 3;
    let mut parts: Vec<Vec<char>> = chars.chunks(len).map(<[char]>::to_vec).collect();
    if len > 8 {
        for p in &mut parts {
            p.drain(..len - 8);
        }
    }
    let mut len = parts[0].len();
    while len > 2 && parts.iter().all(|p| p[0] == '0') {
        for p in &mut parts {
            p.remove(0);
        }
        len -= 1;
    }
    let component = |p: &[char]| {
        let s: String = p.iter().take(2).collect();
        u8::from_str_radix(&s, 16).unwrap_or(0)
    };
    Some(Rgba::rgb(
        component(&parts[0]),
        component(&parts[1]),
        component(&parts[2]),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bf_css::syntax::parse_component_value;

    fn c(s: &str) -> Option<String> {
        parse_color(&parse_component_value(s).ok()?).map(|c| c.to_string())
    }

    #[test]
    fn colors() {
        assert_eq!(c("red").as_deref(), Some("rgb(255, 0, 0)"));
        assert_eq!(c("RebeccaPurple").as_deref(), Some("rgb(102, 51, 153)"));
        assert_eq!(c("#0f0").as_deref(), Some("rgb(0, 255, 0)"));
        assert_eq!(c("#0000ff80").as_deref(), Some("rgba(0, 0, 255, 0.502)"));
        assert_eq!(c("rgb(1, 2, 3)").as_deref(), Some("rgb(1, 2, 3)"));
        assert_eq!(
            c("rgba(255, 0, 0, 0.5)").as_deref(),
            Some("rgba(255, 0, 0, 0.5)")
        );
        assert_eq!(
            c("rgb(255 0 0 / 50%)").as_deref(),
            Some("rgba(255, 0, 0, 0.5)")
        );
        assert_eq!(c("rgb(100%, 0%, 0%)").as_deref(), Some("rgb(255, 0, 0)"));
        assert_eq!(c("rgb(100%, 0, 0)"), None);
        assert_eq!(c("rgb(none 0 0)").as_deref(), Some("rgb(0, 0, 0)"));
        assert_eq!(c("hsl(120, 100%, 50%)").as_deref(), Some("rgb(0, 255, 0)"));
        assert_eq!(
            c("hsl(240deg 100% 50% / 0.25)").as_deref(),
            Some("rgba(0, 0, 255, 0.25)")
        );
        assert_eq!(c("hwb(0 0% 0%)").as_deref(), Some("rgb(255, 0, 0)"));
        assert_eq!(c("hwb(0 50% 50%)").as_deref(), Some("rgb(128, 128, 128)"));
        assert_eq!(c("transparent").as_deref(), Some("rgba(0, 0, 0, 0)"));
        assert_eq!(c("currentColor").as_deref(), Some("currentcolor"));
        assert_eq!(c("notacolor"), None);
        assert_eq!(
            parse_legacy_color("chucknorris"),
            Some(Rgba::rgb(0xC0, 0, 0))
        );
        assert_eq!(parse_legacy_color("#f00"), Some(Rgba::rgb(255, 0, 0)));
        assert_eq!(parse_legacy_color("lime"), Some(Rgba::rgb(0, 255, 0)));
    }
}
