//! E12: `object-fit` y `object-position` (CSS Images 3 §5.5 y §5.6).

use bf_css::syntax::ComponentValue;

use super::types::{PositionAxis, parse_bg_position};
use super::{ComputeContext, PropValue};
use crate::values::single_ident;

/// `object-fit`: cómo se ajusta el contenido de un elemento reemplazado a su content box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectFit {
    Fill,
    Contain,
    Cover,
    None,
    ScaleDown,
}

impl PropValue for ObjectFit {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        Some(match single_ident(input)?.to_ascii_lowercase().as_str() {
            "fill" => ObjectFit::Fill,
            "contain" => ObjectFit::Contain,
            "cover" => ObjectFit::Cover,
            "none" => ObjectFit::None,
            "scale-down" => ObjectFit::ScaleDown,
            _ => return None,
        })
    }

    fn to_css(&self) -> String {
        match self {
            ObjectFit::Fill => "fill",
            ObjectFit::Contain => "contain",
            ObjectFit::Cover => "cover",
            ObjectFit::None => "none",
            ObjectFit::ScaleDown => "scale-down",
        }
        .into()
    }
}

/// `object-position: <position>`: dónde va el contenido dentro del content box, como una
/// capa de `background-position` (los porcentajes, sobre content box − objeto).
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectPosition(pub PositionAxis, pub PositionAxis);

impl PropValue for ObjectPosition {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let (x, y) = parse_bg_position(crate::values::trim(input))?;
        Some(ObjectPosition(x, y))
    }

    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        let c = |a: &PositionAxis| PositionAxis {
            from_end: a.from_end,
            offset: a.offset.compute(&ctx.len),
        };
        ObjectPosition(c(&self.0), c(&self.1))
    }

    fn to_css(&self) -> String {
        let axis = |a: &PositionAxis, end: &str| {
            if a.from_end {
                format!("{end} {}", a.offset)
            } else {
                a.offset.to_string()
            }
        };
        format!("{} {}", axis(&self.0, "right"), axis(&self.1, "bottom"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::values::LengthPercentage;

    fn values(css: &str) -> Vec<ComponentValue> {
        bf_css::syntax::parse_component_value_list(css)
    }

    #[test]
    fn object_fit_and_position_syntax() {
        let fit = |css: &str| ObjectFit::parse(crate::values::trim(&values(css)));
        assert_eq!(fit("Scale-Down"), Some(ObjectFit::ScaleDown));
        assert_eq!(fit("cover"), Some(ObjectFit::Cover));
        assert_eq!(fit("auto"), None);
        let pos = |css: &str| ObjectPosition::parse(&values(css));
        let p = pos("bottom 1px right 2px").unwrap();
        assert!(p.0.from_end && p.1.from_end);
        assert_eq!(p.0.resolve(10.0), 8.0);
        assert_eq!(p.1.resolve(10.0), 9.0);
        let p = pos("top").unwrap();
        assert_eq!(p.0.offset, LengthPercentage::Percentage(50.0));
        assert_eq!(p.1.resolve(10.0), 0.0);
        assert_eq!(pos("left, right"), None);
        // `center` toma el eje que dejan libre los demás keywords.
        let p = pos("center right 25%").unwrap();
        assert_eq!((p.0.resolve(100.0), p.1.resolve(100.0)), (75.0, 50.0));
        let p = pos("top 3px center").unwrap();
        assert_eq!((p.0.resolve(100.0), p.1.resolve(100.0)), (50.0, 3.0));
        assert_eq!(pos("center center left"), None);
    }
}
