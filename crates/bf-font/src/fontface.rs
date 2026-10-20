//! Descriptores de `@font-face` (CSS Fonts 4 §4).

use bf_css::syntax::{ComponentValue, Declaration, TokenKind, split_comma_separated};

use crate::face::FaceStyle;

#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// URL (ya resuelta por `bf-style`) y la pista de `format()`, si hay.
    Url {
        url: String,
        format: Option<String>,
    },
    Local(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FontFaceRule {
    /// En minúsculas.
    pub family: String,
    pub sources: Vec<Source>,
    pub weight: (f32, f32),
    pub style: FaceStyle,
    /// Rangos de code points (inclusive).
    pub ranges: Vec<(u32, u32)>,
}

fn non_ws(values: &[ComponentValue]) -> Vec<&ComponentValue> {
    values.iter().filter(|v| !v.is_whitespace()).collect()
}

fn ident(v: &ComponentValue) -> Option<&str> {
    match v.token() {
        Some(TokenKind::Ident(s)) => Some(s),
        _ => None,
    }
}

fn string_or_idents(values: &[&ComponentValue]) -> Option<String> {
    match values {
        [v] if matches!(v.token(), Some(TokenKind::String(_))) => match v.token() {
            Some(TokenKind::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => {
            let parts: Option<Vec<&str>> = values.iter().map(|v| ident(v)).collect();
            let parts = parts?;
            (!parts.is_empty()).then(|| parts.join(" "))
        }
    }
}

fn number(v: &ComponentValue) -> Option<f32> {
    match v.token() {
        Some(TokenKind::Number(n)) => Some(n.value as f32),
        _ => None,
    }
}

fn weight_value(v: &ComponentValue) -> Option<f32> {
    match ident(v).map(str::to_ascii_lowercase).as_deref() {
        Some("normal") => Some(400.0),
        Some("bold") => Some(700.0),
        Some(_) => None,
        None => number(v).filter(|w| (1.0..=1000.0).contains(w)),
    }
}

fn sources(value: &[ComponentValue]) -> Vec<Source> {
    let mut out = Vec::new();
    for group in split_comma_separated(value) {
        let items = non_ws(&group);
        let Some(first) = items.first() else {
            continue;
        };
        let format = items.iter().skip(1).find_map(|v| match v {
            ComponentValue::Function(f) if f.name.eq_ignore_ascii_case("format") => {
                non_ws(&f.value).first().and_then(|a| match a.token() {
                    Some(TokenKind::String(s) | TokenKind::Ident(s)) => {
                        Some(s.to_ascii_lowercase())
                    }
                    _ => None,
                })
            }
            _ => None,
        });
        match first {
            ComponentValue::Token(t) => {
                if let TokenKind::Url(u) = &t.kind {
                    out.push(Source::Url {
                        url: u.clone(),
                        format,
                    });
                }
            }
            ComponentValue::Function(f) if f.name.eq_ignore_ascii_case("url") => {
                if let Some(TokenKind::String(u)) = non_ws(&f.value).first().and_then(|v| v.token())
                {
                    out.push(Source::Url {
                        url: u.clone(),
                        format,
                    });
                }
            }
            ComponentValue::Function(f) if f.name.eq_ignore_ascii_case("local") => {
                if let Some(name) = string_or_idents(&non_ws(&f.value)) {
                    out.push(Source::Local(name));
                }
            }
            _ => {}
        }
    }
    out
}

fn unicode_ranges(value: &[ComponentValue]) -> Option<Vec<(u32, u32)>> {
    let mut out = Vec::new();
    for group in split_comma_separated(value) {
        match non_ws(&group).as_slice() {
            [v] => match v.token() {
                Some(TokenKind::UnicodeRange { start, end }) if start <= end => {
                    out.push((*start, (*end).min(0x10FFFF)));
                }
                _ => return None,
            },
            _ => return None,
        }
    }
    Some(out)
}

/// Interpreta una regla `@font-face`; `None` si falta `font-family` o `src`.
pub fn parse_font_face(declarations: &[Declaration]) -> Option<FontFaceRule> {
    let mut rule = FontFaceRule {
        family: String::new(),
        sources: Vec::new(),
        weight: (400.0, 400.0),
        style: FaceStyle::Normal,
        ranges: vec![(0, 0x10FFFF)],
    };
    for d in declarations {
        let values = non_ws(&d.value);
        match d.name.to_ascii_lowercase().as_str() {
            "font-family" => {
                if let Some(f) = string_or_idents(&values) {
                    rule.family = f.to_lowercase();
                }
            }
            "src" => rule.sources = sources(&d.value),
            "font-weight" => match values.as_slice() {
                [v] => {
                    if let Some(w) = weight_value(v) {
                        rule.weight = (w, w);
                    } else if ident(v).is_some_and(|i| i.eq_ignore_ascii_case("auto")) {
                        rule.weight = (1.0, 1000.0);
                    }
                }
                [a, b] => {
                    if let (Some(a), Some(b)) = (weight_value(a), weight_value(b)) {
                        rule.weight = (a.min(b), a.max(b));
                    }
                }
                _ => {}
            },
            "font-style" => {
                if let Some(first) = values.first().and_then(|v| ident(v)) {
                    match first.to_ascii_lowercase().as_str() {
                        "normal" => rule.style = FaceStyle::Normal,
                        "italic" => rule.style = FaceStyle::Italic,
                        "oblique" => rule.style = FaceStyle::Oblique,
                        _ => {}
                    }
                }
            }
            "unicode-range" => {
                if let Some(r) = unicode_ranges(&d.value) {
                    rule.ranges = r;
                }
            }
            _ => {}
        }
    }
    (!rule.family.is_empty() && !rule.sources.is_empty()).then_some(rule)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bf_css::stylesheet::{Origin, Rule, Stylesheet};

    fn rule(css: &str) -> Option<FontFaceRule> {
        let sheet = Stylesheet::parse(css, Origin::Author);
        match &sheet.rules[0] {
            Rule::FontFace(d) => parse_font_face(d),
            _ => None,
        }
    }

    #[test]
    fn descriptors() {
        let r = rule(
            "@font-face { font-family: \"My Font\"; src: local(Foo Bar), url(a.woff) format('woff'), url('b.ttf') format(\"truetype\"); font-weight: 300 700; font-style: italic; unicode-range: U+0-7F, U+4??; }",
        )
        .unwrap();
        assert_eq!(r.family, "my font");
        assert_eq!(
            r.sources,
            vec![
                Source::Local("Foo Bar".into()),
                Source::Url {
                    url: "a.woff".into(),
                    format: Some("woff".into())
                },
                Source::Url {
                    url: "b.ttf".into(),
                    format: Some("truetype".into())
                },
            ]
        );
        assert_eq!(r.weight, (300.0, 700.0));
        assert_eq!(r.style, FaceStyle::Italic);
        assert_eq!(r.ranges, vec![(0, 0x7F), (0x400, 0x4FF)]);
        assert!(rule("@font-face { src: url(a.ttf) }").is_none());
    }
}
