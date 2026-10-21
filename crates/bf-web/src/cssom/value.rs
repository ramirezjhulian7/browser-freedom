//! Propiedades soportadas y serialización de valores especificados (CSSOM §6.7.1 y §6.7.2,
//! "serialize a CSS value" y "serialize a CSS component value").

use std::fmt::Write as _;
use std::sync::OnceLock;

use bf_css::syntax::{BlockKind, ComponentValue, TokenKind, parse_component_value_list};
use bf_style::properties::shorthands::{SHORTHANDS, ShorthandDef, shorthand_by_name};
use bf_style::properties::{LonghandId, PROPERTIES, longhand_by_name};
use bf_style::values::color::parse_color;
use bf_style::values::trim;

/// Una propiedad conocida por el motor.
#[derive(Debug, Clone, Copy)]
pub enum Property {
    Longhand(LonghandId),
    Shorthand(&'static ShorthandDef),
    Custom,
}

/// Los shorthands que no se usan para comprimir `cssText` (alias y lógicos: no tienen
/// longhands propios en el motor, se expanden a los físicos).
pub fn is_alias_shorthand(name: &str) -> bool {
    name.starts_with("grid-") && name.ends_with("gap")
        || name.contains("-block")
        || name.contains("-inline")
        || name == "grid"
        || name == "grid-template"
        || name == "grid-area"
}

/// "Supported CSS property" (CSSOM §6.7.1): `None` si el motor no la conoce.
pub fn property(name: &str) -> Option<Property> {
    if is_custom(name) {
        return Some(Property::Custom);
    }
    if let Some(id) = longhand_by_name(name) {
        return Some(Property::Longhand(id));
    }
    shorthand_by_name(name).map(Property::Shorthand)
}

/// Una custom property (`--*`, CSS Variables §2).
pub fn is_custom(name: &str) -> bool {
    name.starts_with("--")
}

/// Todos los nombres soportados (longhands y shorthands), ordenados.
pub fn supported_names() -> &'static [&'static str] {
    static NAMES: OnceLock<Vec<&'static str>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut v: Vec<&'static str> = PROPERTIES.iter().map(|p| p.name).collect();
        v.extend(SHORTHANDS.iter().map(|s| s.name));
        v.sort_unstable();
        v.dedup();
        v
    })
}

/// Los longhands en orden lexicográfico (la lista de una declaración computada).
pub fn sorted_longhands() -> &'static [LonghandId] {
    static IDS: OnceLock<Vec<LonghandId>> = OnceLock::new();
    IDS.get_or_init(|| {
        let mut v: Vec<LonghandId> = PROPERTIES.iter().map(|p| p.id).collect();
        v.sort_by_key(|id| id.name());
        v
    })
}

/// "CSS property to IDL attribute" (CSSOM §6.7.1): `background-color` → `backgroundColor`.
pub fn camel_case(property: &str, lowercase_first: bool) -> String {
    let property = if lowercase_first {
        property.strip_prefix('-').unwrap_or(property)
    } else {
        property
    };
    let mut out = String::new();
    let mut upper = false;
    for c in property.chars() {
        if c == '-' {
            upper = true;
            continue;
        }
        if upper {
            out.push(c.to_ascii_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// Las propiedades donde los identificadores distinguen mayúsculas (nombres de familia,
/// áreas de grid, contadores…): no se pasan a minúsculas.
fn case_sensitive(property: &str) -> bool {
    matches!(
        property,
        "font-family"
            | "font"
            | "content"
            | "quotes"
            | "list-style-type"
            | "list-style"
            | "grid-template-areas"
            | "grid-template"
            | "grid"
            | "grid-area"
            | "grid-row"
            | "grid-column"
            | "grid-row-start"
            | "grid-row-end"
            | "grid-column-start"
            | "grid-column-end"
            | "grid-template-columns"
            | "grid-template-rows"
            | "grid-auto-columns"
            | "grid-auto-rows"
            | "cursor"
    )
}

/// "Serialize a CSS value" de un valor especificado: la forma canónica de sus component
/// values (números mínimos, unidades y palabras clave en minúsculas, colores hexadecimales y
/// funcionales como `rgb()`, strings entre comillas dobles, `", "` entre elementos de lista).
pub fn serialize_value(property: &str, values: &[ComponentValue]) -> String {
    let values = trim(values);
    if is_custom(property) {
        return serialize_raw(values);
    }
    let mut out = String::new();
    write_values(&mut out, values, !case_sensitive(property), true);
    out
}

/// El texto de un valor tal cual (custom properties): espacios de los extremos fuera.
pub fn serialize_raw(values: &[ComponentValue]) -> String {
    let mut out = String::new();
    write_values(&mut out, trim(values), false, false);
    out
}

/// Parsea `text` y lo serializa para `property`.
pub fn normalize(property: &str, text: &str) -> String {
    serialize_value(property, &parse_component_value_list(text))
}

fn write_values(out: &mut String, values: &[ComponentValue], lower: bool, canon: bool) {
    let mut pending_space = false;
    for v in values {
        match v.token() {
            Some(TokenKind::Whitespace) => {
                pending_space = true;
                continue;
            }
            Some(TokenKind::Comma) if canon => {
                out.push_str(", ");
                pending_space = false;
                continue;
            }
            Some(TokenKind::Delim('/')) if canon => {
                if pending_space && !out.is_empty() {
                    out.push(' ');
                }
                out.push('/');
                continue;
            }
            _ => {}
        }
        if pending_space && !out.is_empty() && !out.ends_with(' ') && !out.ends_with('(') {
            out.push(' ');
        }
        pending_space = false;
        write_value(out, v, lower, canon);
    }
}

fn write_value(out: &mut String, v: &ComponentValue, lower: bool, canon: bool) {
    match v {
        ComponentValue::Token(t) => write_token(out, &t.kind, lower, canon),
        ComponentValue::Function(f) => {
            let name = f.name.to_ascii_lowercase();
            if canon
                && matches!(name.as_str(), "rgb" | "rgba" | "hsl" | "hsla" | "hwb")
                && let Some(bf_style::values::Color::Rgba(c)) = parse_color(v)
            {
                let _ = write!(out, "{c}");
                return;
            }
            if canon {
                out.push_str(&name);
            } else {
                out.push_str(&f.name);
            }
            out.push('(');
            write_values(out, trim(&f.value), lower, canon);
            out.push(')');
        }
        ComponentValue::Block(b) => {
            let (open, close) = match b.kind {
                BlockKind::Curly => ('{', '}'),
                BlockKind::Square => ('[', ']'),
                BlockKind::Paren => ('(', ')'),
            };
            out.push(open);
            write_values(out, trim(&b.value), lower, canon);
            out.push(close);
        }
    }
}

fn write_token(out: &mut String, t: &TokenKind, lower: bool, canon: bool) {
    match t {
        TokenKind::Ident(s) if canon => write_ident(out, &maybe_lower(s, lower)),
        TokenKind::Hash { value, .. } if canon => {
            if let Some(c) = bf_style::values::color::parse_hex(value) {
                let _ = write!(out, "{c}");
            } else {
                out.push('#');
                write_ident(out, value);
            }
        }
        TokenKind::String(s) if canon => write_string(out, s),
        TokenKind::Url(u) if canon => {
            out.push_str("url(");
            write_string(out, u);
            out.push(')');
        }
        TokenKind::Number(n) if canon => out.push_str(&number(n.value)),
        TokenKind::Percentage(n) if canon => {
            out.push_str(&number(n.value));
            out.push('%');
        }
        TokenKind::Dimension(n, unit) if canon => {
            out.push_str(&number(n.value));
            write_ident(out, &unit.to_ascii_lowercase());
        }
        TokenKind::String(s) => write_string(out, s),
        other => {
            let _ = write!(out, "{other}");
        }
    }
}

fn maybe_lower(s: &str, lower: bool) -> String {
    if lower {
        s.to_ascii_lowercase()
    } else {
        s.to_owned()
    }
}

/// "Serialize a media query list" (CSSOM §4.1.2): cada consulta como se escribió (palabras
/// clave en minúsculas, `": "` en las features, espacios normalizados); una consulta inválida
/// es `not all`.
pub fn media_list_text(text: &str) -> String {
    let tokens = parse_component_value_list(text);
    let mut out: Vec<String> = Vec::new();
    for piece in tokens.split(|v| matches!(v.token(), Some(TokenKind::Comma))) {
        let piece = trim(piece);
        if piece.is_empty() {
            if !trim(&tokens).is_empty() {
                out.push("not all".into());
            }
            continue;
        }
        let mut raw = String::new();
        write_values(&mut raw, piece, false, false);
        let parsed = bf_css::media::parse_media_query_list(&raw).to_string();
        let mut q = String::new();
        write_values(&mut q, piece, true, true);
        let mut fixed = String::new();
        let chars: Vec<char> = q.chars().collect();
        for (i, &ch) in chars.iter().enumerate() {
            if ch == ' ' && chars.get(i + 1) == Some(&':') {
                continue;
            }
            fixed.push(ch);
            if ch == ':' && chars.get(i + 1).is_some_and(|n| *n != ' ') {
                fixed.push(' ');
            }
        }
        if parsed == "not all" && fixed != "not all" {
            out.push("not all".into());
        } else {
            out.push(fixed);
        }
    }
    out.join(", ")
}

/// "Serialize a <number>": la representación decimal más corta (hasta 6 decimales, como
/// los navegadores), sin exponente.
pub fn number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let r = format!("{v:.6}");
    let r = r.trim_end_matches('0').trim_end_matches('.');
    if r == "-0" { "0".into() } else { r.to_owned() }
}

/// "Serialize an identifier" (CSSOM §2.1).
pub fn write_ident(out: &mut String, s: &str) {
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let first_digit = c.is_ascii_digit() && (i == 0 || (i == 1 && chars[0] == '-'));
        if c == '\0' {
            out.push('\u{FFFD}');
        } else if ('\u{1}'..='\u{1f}').contains(&c) || c == '\u{7f}' || first_digit {
            let _ = write!(out, "\\{:x} ", c as u32);
        } else if i == 0 && c == '-' && chars.len() == 1 {
            out.push_str("\\-");
        } else if c >= '\u{80}' || c == '-' || c == '_' || c.is_ascii_alphanumeric() {
            out.push(c);
        } else {
            out.push('\\');
            out.push(c);
        }
    }
}

/// "Serialize a string" (CSSOM §2.1).
pub fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '\0' => out.push('\u{FFFD}'),
            '\u{1}'..='\u{1f}' | '\u{7f}' => {
                let _ = write!(out, "\\{:x} ", c as u32);
            }
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values() {
        assert_eq!(normalize("color", "  RED "), "red");
        assert_eq!(normalize("color", "#00f"), "rgb(0, 0, 255)");
        assert_eq!(normalize("color", "rgba(0,0,0,.5)"), "rgba(0, 0, 0, 0.5)");
        assert_eq!(normalize("width", "10.0PX"), "10px");
        assert_eq!(normalize("margin", "1px   2px"), "1px 2px");
        assert_eq!(
            normalize("font-family", "Arial,  'Times New'"),
            "Arial, \"Times New\""
        );
        assert_eq!(normalize("--x", "  a  b "), "a b");
        assert_eq!(number(0.1), "0.1");
        assert_eq!(
            media_list_text("SCREEN and (MIN-WIDTH:500px) , print"),
            "screen and (min-width: 500px), print"
        );
        assert_eq!(media_list_text("screen and and"), "not all");
        assert_eq!(number(1.0 / 3.0), "0.333333");
        assert_eq!(camel_case("background-color", false), "backgroundColor");
        assert_eq!(camel_case("-webkit-box", true), "webkitBox");
        assert_eq!(camel_case("-webkit-box", false), "WebkitBox");
    }
}
