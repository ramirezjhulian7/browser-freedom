//! E12: `text-overflow` (CSS Overflow 3 §5 y CSS Overflow 4 §5.1) y `overflow-clip-margin`
//! (CSS Overflow 3 §3.2).

use bf_css::syntax::{ComponentValue, TokenKind};

use super::{ComputeContext, PropValue};
use crate::values::{LengthPercentage, components, ident, parse_length};

/// Qué se hace con el contenido inline que desborda un extremo de la línea.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverflowEnd {
    /// Se recorta por el borde del block container (o en mitad de un carácter).
    Clip,
    /// Se ocultan caracteres y cajas atómicas hasta que cabe U+2026.
    Ellipsis,
    /// Como `ellipsis` con la cadena dada (CSS Overflow 4).
    String(String),
}

impl OverflowEnd {
    /// El texto que sustituye al contenido ocultado, si hay.
    pub fn text(&self) -> Option<&str> {
        match self {
            OverflowEnd::Clip => None,
            OverflowEnd::Ellipsis => Some("\u{2026}"),
            OverflowEnd::String(s) => Some(s),
        }
    }

    fn parse_one(v: &ComponentValue) -> Option<Self> {
        if let Some(TokenKind::String(s)) = v.token() {
            return Some(OverflowEnd::String(s.clone()));
        }
        match ident(v)?.to_ascii_lowercase().as_str() {
            "clip" => Some(OverflowEnd::Clip),
            "ellipsis" => Some(OverflowEnd::Ellipsis),
            _ => None,
        }
    }

    fn to_css(&self) -> String {
        match self {
            OverflowEnd::Clip => "clip".into(),
            OverflowEnd::Ellipsis => "ellipsis".into(),
            OverflowEnd::String(s) => format!("{s:?}"),
        }
    }
}

/// `text-overflow: [ clip | ellipsis | <string> ]{1,2}`. Con un valor, se aplica al extremo
/// del final de la línea (el otro es `clip`); con dos, el primero es el del lado line-left y
/// el segundo el del line-right (CSS Overflow 4 §5.1). `fade` no está implementado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextOverflow {
    pub first: OverflowEnd,
    pub second: Option<OverflowEnd>,
}

impl TextOverflow {
    /// El valor de los extremos (inicio, final) de una línea con dirección `rtl`.
    pub fn sides(&self, rtl: bool) -> (&OverflowEnd, &OverflowEnd) {
        match &self.second {
            None => (&OverflowEnd::Clip, &self.first),
            // line-left es el inicio en ltr y el final en rtl.
            Some(right) if rtl => (right, &self.first),
            Some(right) => (&self.first, right),
        }
    }

    /// ¿Algún extremo pide elipsis?
    pub fn is_clip(&self) -> bool {
        self.first == OverflowEnd::Clip
            && self.second.as_ref().is_none_or(|s| *s == OverflowEnd::Clip)
    }
}

impl PropValue for TextOverflow {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match components(input).as_slice() {
            [a] => Some(TextOverflow {
                first: OverflowEnd::parse_one(a)?,
                second: None,
            }),
            [a, b] => Some(TextOverflow {
                first: OverflowEnd::parse_one(a)?,
                second: Some(OverflowEnd::parse_one(b)?),
            }),
            _ => None,
        }
    }

    fn to_css(&self) -> String {
        match &self.second {
            None => self.first.to_css(),
            Some(s) => format!("{} {}", self.first.to_css(), s.to_css()),
        }
    }
}

/// `<visual-box>` (CSS Box 4 §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualBox {
    ContentBox,
    PaddingBox,
    BorderBox,
}

/// `overflow-clip-margin: <visual-box> || <length>` (CSS Overflow 3 §3.2): el borde de
/// recorte de `overflow: clip` es la caja indicada (por omisión, la padding box) agrandada en
/// la longitud. Computado, la longitud está en px. La gramática del TR pide `[0,∞]`, pero los
/// WPT actuales (`overflow-clip-margin-016`/`017`/`019`/`020`) usan longitudes negativas para
/// encoger el borde: se aceptan.
#[derive(Debug, Clone, PartialEq)]
pub struct OverflowClipMargin {
    pub visual_box: VisualBox,
    pub length: LengthPercentage,
}

impl OverflowClipMargin {
    pub fn px(&self) -> f32 {
        self.length.resolve(0.0)
    }
}

impl PropValue for OverflowClipMargin {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let mut visual_box = None;
        let mut length = None;
        for v in components(input) {
            let b = ident(v).and_then(|s| match s.to_ascii_lowercase().as_str() {
                "content-box" => Some(VisualBox::ContentBox),
                "padding-box" => Some(VisualBox::PaddingBox),
                "border-box" => Some(VisualBox::BorderBox),
                _ => None,
            });
            match b {
                Some(b) if visual_box.is_none() => visual_box = Some(b),
                Some(_) => return None,
                None if length.is_none() => {
                    let l = parse_length(v)?;
                    length = Some(LengthPercentage::Length(l));
                }
                None => return None,
            }
        }
        if visual_box.is_none() && length.is_none() {
            return None;
        }
        Some(OverflowClipMargin {
            visual_box: visual_box.unwrap_or(VisualBox::PaddingBox),
            length: length.unwrap_or_else(LengthPercentage::zero),
        })
    }

    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        OverflowClipMargin {
            visual_box: self.visual_box,
            length: self.length.compute(&ctx.len),
        }
    }

    fn to_css(&self) -> String {
        let l = self.length.to_string();
        match self.visual_box {
            VisualBox::PaddingBox => l,
            VisualBox::ContentBox => format!("content-box {l}"),
            VisualBox::BorderBox => format!("border-box {l}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(css: &str) -> Vec<ComponentValue> {
        bf_css::syntax::parse_component_value_list(css)
    }

    #[test]
    fn text_overflow_syntax() {
        let p = |css: &str| TextOverflow::parse(crate::values::trim(&values(css)));
        let one = |first| {
            Some(TextOverflow {
                first,
                second: None,
            })
        };
        assert_eq!(p("clip"), one(OverflowEnd::Clip));
        assert_eq!(p("Ellipsis"), one(OverflowEnd::Ellipsis));
        assert_eq!(p("\"123\""), one(OverflowEnd::String("123".into())));
        let two = p("clip ellipsis").unwrap();
        assert_eq!(
            two.sides(false),
            (&OverflowEnd::Clip, &OverflowEnd::Ellipsis)
        );
        assert_eq!(
            two.sides(true),
            (&OverflowEnd::Ellipsis, &OverflowEnd::Clip)
        );
        assert_eq!(p("clip clip clip"), None);
        assert_eq!(p("auto"), None);
    }

    #[test]
    fn overflow_clip_margin_syntax() {
        let p = |css: &str| OverflowClipMargin::parse(crate::values::trim(&values(css)));
        let m = p("10px").unwrap();
        assert_eq!((m.visual_box, m.px()), (VisualBox::PaddingBox, 10.0));
        let m = p("20px content-box").unwrap();
        assert_eq!((m.visual_box, m.px()), (VisualBox::ContentBox, 20.0));
        assert_eq!(p("border-box").unwrap().px(), 0.0);
        assert_eq!(p("padding-box -10px").unwrap().px(), -10.0);
        assert_eq!(p("auto"), None);
        assert_eq!(p("border-box content-box"), None);
    }
}
