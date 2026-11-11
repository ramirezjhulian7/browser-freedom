//! E12: `line-clamp` (CSS Overflow 4 §5), su forma heredada `-webkit-line-clamp` y
//! `-webkit-box-orient` (que decide si la heredada se aplica).

use bf_css::syntax::{ComponentValue, TokenKind};

use super::PropValue;
use crate::values::{components, ident, integer, single_ident};

/// `block-ellipsis` (CSS Overflow 4 §5.4): `none | auto | <string>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockEllipsis {
    None,
    /// La elipsis del idioma: U+2026.
    Auto,
    String(String),
}

impl BlockEllipsis {
    /// El texto que se inserta, si hay.
    pub fn text(&self) -> Option<&str> {
        match self {
            BlockEllipsis::None => None,
            BlockEllipsis::Auto => Some("\u{2026}"),
            BlockEllipsis::String(s) => Some(s),
        }
    }

    fn parse_one(v: &ComponentValue) -> Option<Self> {
        if let Some(TokenKind::String(s)) = v.token() {
            return Some(BlockEllipsis::String(s.clone()));
        }
        match ident(v)?.to_ascii_lowercase().as_str() {
            "none" => Some(BlockEllipsis::None),
            "auto" => Some(BlockEllipsis::Auto),
            _ => None,
        }
    }

    fn to_css(&self) -> String {
        match self {
            BlockEllipsis::None => "none".into(),
            BlockEllipsis::Auto => "auto".into(),
            BlockEllipsis::String(s) => format!("{s:?}"),
        }
    }
}

/// Dónde se corta: tras un número de líneas o (`auto`) en la última línea que cabe en la
/// altura (o `max-height`) de la caja.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaxLines {
    None,
    Auto,
    Count(u32),
}

/// `line-clamp: none | [ <integer [1,∞]> | auto ] || <'block-ellipsis'>` (CSS Overflow 4
/// §5.1); el `block-ellipsis` omitido es `auto`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineClamp {
    pub lines: MaxLines,
    pub ellipsis: BlockEllipsis,
}

impl PropValue for LineClamp {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let parts = components(input);
        if let [v] = parts.as_slice()
            && ident(v).is_some_and(|s| s.eq_ignore_ascii_case("none"))
        {
            return Some(LineClamp {
                lines: MaxLines::None,
                ellipsis: BlockEllipsis::None,
            });
        }
        let mut lines = None;
        let mut ellipsis = None;
        for v in parts {
            if lines.is_none() {
                if let Some(n) = integer(v) {
                    if n < 1 {
                        return None;
                    }
                    lines = Some(MaxLines::Count(n as u32));
                    continue;
                }
                // `auto` primero es el número de líneas; después, la elipsis.
                if ident(v).is_some_and(|s| s.eq_ignore_ascii_case("auto")) && ellipsis.is_none() {
                    lines = Some(MaxLines::Auto);
                    continue;
                }
            }
            if ellipsis.is_none() {
                ellipsis = Some(BlockEllipsis::parse_one(v)?);
                continue;
            }
            return None;
        }
        Some(LineClamp {
            lines: lines?,
            ellipsis: ellipsis.unwrap_or(BlockEllipsis::Auto),
        })
    }

    fn to_css(&self) -> String {
        let lines = match self.lines {
            MaxLines::None => return "none".into(),
            MaxLines::Auto => "auto".to_owned(),
            MaxLines::Count(n) => n.to_string(),
        };
        match self.ellipsis {
            BlockEllipsis::Auto => lines,
            ref e => format!("{lines} {}", e.to_css()),
        }
    }
}

/// `-webkit-line-clamp: none | <integer [1,∞]>` (CSS Overflow 4 §5.5): solo actúa con
/// `display: -webkit-box` y `-webkit-box-orient: vertical`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebkitLineClamp(pub Option<u32>);

impl PropValue for WebkitLineClamp {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        if single_ident(input).is_some_and(|s| s.eq_ignore_ascii_case("none")) {
            return Some(WebkitLineClamp(None));
        }
        match components(input).as_slice() {
            [v] => {
                let n = integer(v)?;
                (n >= 1).then_some(WebkitLineClamp(Some(n as u32)))
            }
            _ => None,
        }
    }

    fn to_css(&self) -> String {
        self.0.map_or_else(|| "none".into(), |n| n.to_string())
    }
}

/// `-webkit-box-orient` (Compat): solo importa si es vertical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxOrient {
    Horizontal,
    Vertical,
    InlineAxis,
    BlockAxis,
}

impl BoxOrient {
    pub fn is_vertical(self) -> bool {
        matches!(self, BoxOrient::Vertical | BoxOrient::BlockAxis)
    }
}

impl PropValue for BoxOrient {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        Some(match single_ident(input)?.to_ascii_lowercase().as_str() {
            "horizontal" => BoxOrient::Horizontal,
            "vertical" => BoxOrient::Vertical,
            "inline-axis" => BoxOrient::InlineAxis,
            "block-axis" => BoxOrient::BlockAxis,
            _ => return None,
        })
    }

    fn to_css(&self) -> String {
        match self {
            BoxOrient::Horizontal => "horizontal",
            BoxOrient::Vertical => "vertical",
            BoxOrient::InlineAxis => "inline-axis",
            BoxOrient::BlockAxis => "block-axis",
        }
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(css: &str) -> Option<LineClamp> {
        let values = bf_css::syntax::parse_component_value_list(css);
        LineClamp::parse(crate::values::trim(&values))
    }

    #[test]
    fn line_clamp_syntax() {
        let n = |lines, ellipsis| Some(LineClamp { lines, ellipsis });
        assert_eq!(parse("none"), n(MaxLines::None, BlockEllipsis::None));
        assert_eq!(parse("3"), n(MaxLines::Count(3), BlockEllipsis::Auto));
        assert_eq!(parse("auto"), n(MaxLines::Auto, BlockEllipsis::Auto));
        assert_eq!(parse("2 none"), n(MaxLines::Count(2), BlockEllipsis::None));
        assert_eq!(
            parse("\"(…)\" 2"),
            n(MaxLines::Count(2), BlockEllipsis::String("(…)".into()))
        );
        assert_eq!(parse("0"), None);
        assert_eq!(parse("2 no-ellipsis"), None);
    }
}
