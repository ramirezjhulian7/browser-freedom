//! E4: animaciones y transiciones CSS sobre los estilos computados.
//!
//! [`Animator`] guarda, por elemento, las animaciones CSS (CSS Animations 1) y las transiciones
//! (CSS Transitions 1) en curso. Cada vez que se calculan los estilos del documento ("style
//! change event", CSS Transitions 1 §3) se llama a [`Animator::update`] con los estilos de la
//! cascada y el tiempo de la línea de tiempo del documento: arranca o cancela animaciones y
//! transiciones, reemplaza en los estilos los valores animados y encola los eventos
//! (`animationstart`, `transitionend`, ...). El modelo de tiempo es el de Web Animations 1 §4
//! (fases, tiempo activo, iteraciones y dirección).

use std::collections::HashMap;
use std::sync::Arc;

use bf_css::selectors::PseudoElement;
use bf_css::stylesheet::{KeyframesRule, LayerRule, Rule, Stylesheet};
use bf_css::syntax::ComponentValue;
use bf_dom::{Document, NodeId};

use crate::StyleMap;
use crate::cascade::is_supported;
use crate::properties::animations::{
    AnimationDirection, FillMode, PlayState, TimingFunction, TransitionBehavior, TransitionProperty,
};
use crate::properties::shorthands::shorthand_by_name;
use crate::properties::transform::{Mat4, Transform, TransformFunction, matrix_css};
use crate::properties::types::*;
use crate::properties::{
    ComputeContext, ComputedStyle, LonghandId, PROPERTIES, Value, longhand_by_name,
};
use crate::values::{CalcNode, Color, Length, LengthContext, LengthPercentage, Rgba, trim};

/// Elemento (o pseudo-elemento) animado.
pub type Key = (NodeId, Option<PseudoElement>);

// --- Interpolación (CSS Values 4 §3, CSS Transitions 1 §5) ---

fn lerp(a: f32, b: f32, p: f64) -> f32 {
    (f64::from(a) + (f64::from(b) - f64::from(a)) * p) as f32
}

/// Interpola dos `<length-percentage>` computados.
pub fn lerp_lp(a: &LengthPercentage, b: &LengthPercentage, p: f64) -> LengthPercentage {
    use LengthPercentage as LP;
    match (a, b) {
        (LP::Length(x), LP::Length(y)) => LP::Length(Length::px(lerp(x.value, y.value, p))),
        (LP::Percentage(x), LP::Percentage(y)) => LP::Percentage(lerp(*x, *y, p)),
        _ => {
            let node = |v: &LP| match v {
                LP::Length(l) => CalcNode::Length(*l),
                LP::Percentage(x) => CalcNode::Percentage(*x),
                LP::Calc(c) => (**c).clone(),
            };
            LP::Calc(Box::new(CalcNode::Sum(vec![
                CalcNode::Product(vec![node(a), CalcNode::Number((1.0 - p) as f32)]),
                CalcNode::Product(vec![node(b), CalcNode::Number(p as f32)]),
            ])))
        }
    }
}

/// Interpola dos colores en sRGB premultiplicado (CSS Color 4 §12.3).
fn lerp_rgba(a: Rgba, b: Rgba, p: f64) -> Rgba {
    let alpha = lerp(a.a, b.a, p).clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return Rgba::TRANSPARENT;
    }
    let c = |x: f32, y: f32| (lerp(x * a.a, y * b.a, p) / alpha).clamp(0.0, 255.0);
    Rgba {
        r: c(a.r, b.r),
        g: c(a.g, b.g),
        b: c(a.b, b.b),
        a: alpha,
    }
}

fn lerp_color(a: &Color, b: &Color, current: (Rgba, Rgba), p: f64) -> Color {
    Color::Rgba(lerp_rgba(a.resolve(current.0), b.resolve(current.1), p))
}

/// Interpola dos funciones del mismo tipo (CSS Transforms 1 §9).
fn lerp_function(
    a: &TransformFunction,
    b: &TransformFunction,
    p: f64,
) -> Option<TransformFunction> {
    use TransformFunction as T;
    Some(match (a, b) {
        (
            T::Translate {
                name,
                x: ax,
                y: ay,
                z: az,
            },
            T::Translate {
                x: bx,
                y: by,
                z: bz,
                ..
            },
        ) => T::Translate {
            name: *name,
            x: lerp_lp(ax, bx, p),
            y: lerp_lp(ay, by, p),
            z: lerp_lp(az, bz, p),
        },
        (
            T::Scale {
                name,
                x: ax,
                y: ay,
                z: az,
            },
            T::Scale {
                x: bx,
                y: by,
                z: bz,
                ..
            },
        ) => T::Scale {
            name: *name,
            x: lerp(*ax, *bx, p),
            y: lerp(*ay, *by, p),
            z: lerp(*az, *bz, p),
        },
        (
            T::Rotate {
                name,
                x,
                y,
                z,
                angle: aa,
            },
            T::Rotate {
                x: bx,
                y: by,
                z: bz,
                angle: ba,
                ..
            },
        ) if (x, y, z) == (bx, by, bz) => T::Rotate {
            name: *name,
            x: *x,
            y: *y,
            z: *z,
            angle: lerp(*aa, *ba, p),
        },
        (T::Skew { name, x: ax, y: ay }, T::Skew { x: bx, y: by, .. }) => T::Skew {
            name: *name,
            x: lerp(*ax, *bx, p),
            y: lerp(*ay, *by, p),
        },
        _ => return None,
    })
}

/// La función neutra ("identity transform function") del mismo tipo que `f`.
fn neutral(f: &TransformFunction) -> Option<TransformFunction> {
    use TransformFunction as T;
    Some(match f {
        T::Translate { name, .. } => T::Translate {
            name: *name,
            x: LengthPercentage::zero(),
            y: LengthPercentage::zero(),
            z: LengthPercentage::zero(),
        },
        T::Scale { name, .. } => T::Scale {
            name: *name,
            x: 1.0,
            y: 1.0,
            z: 1.0,
        },
        T::Rotate { name, x, y, z, .. } => T::Rotate {
            name: *name,
            x: *x,
            y: *y,
            z: *z,
            angle: 0.0,
        },
        T::Skew { name, .. } => T::Skew {
            name: *name,
            x: 0.0,
            y: 0.0,
        },
        _ => return None,
    })
}

/// Descomposición de una matriz 2D (CSS Transforms 1 §10.3.2), con vectores columna:
/// `A = T · R(ángulo) · K · S`. Devuelve `[tx, ty, sx, sy, ángulo, k00, k01, k10, k11]`.
fn decompose_2d(m: &Mat4) -> [f64; 9] {
    let (a, b, c, d) = (m[0][0], m[0][1], m[1][0], m[1][1]);
    let mut sx = (a * a + b * b).sqrt();
    let mut sy = (c * c + d * d).sqrt();
    if a * d - b * c < 0.0 {
        if a < d {
            sx = -sx;
        } else {
            sy = -sy;
        }
    }
    // Columnas normalizadas.
    let (a1, b1) = if sx != 0.0 { (a / sx, b / sx) } else { (a, b) };
    let (c1, d1) = if sy != 0.0 { (c / sy, d / sy) } else { (c, d) };
    let angle = b1.atan2(a1);
    let (s, co) = angle.sin_cos();
    // K = R(-ángulo) · A'.
    let k00 = co * a1 + s * b1;
    let k10 = -s * a1 + co * b1;
    let k01 = co * c1 + s * d1;
    let k11 = -s * c1 + co * d1;
    [m[3][0], m[3][1], sx, sy, angle, k00, k01, k10, k11]
}

/// Recompone `[a, b, c, d, e, f]` desde [`decompose_2d`].
fn recompose_2d(v: &[f64; 9]) -> [f32; 6] {
    let [tx, ty, sx, sy, angle, k00, k01, k10, k11] = *v;
    let (s, co) = angle.sin_cos();
    // R · K
    let rk00 = co * k00 - s * k10;
    let rk01 = co * k01 - s * k11;
    let rk10 = s * k00 + co * k10;
    let rk11 = s * k01 + co * k11;
    // · S
    [
        (rk00 * sx) as f32,
        (rk10 * sx) as f32,
        (rk01 * sy) as f32,
        (rk11 * sy) as f32,
        tx as f32,
        ty as f32,
    ]
}

/// Interpola dos matrices 2D descompuestas (CSS Transforms 1 §10.3).
fn lerp_matrix(a: &Mat4, b: &Mat4, p: f64) -> TransformFunction {
    let mut da = decompose_2d(a);
    let mut db = decompose_2d(b);
    // Si las escalas tienen signos opuestos se gira 180° en lugar de invertir los dos ejes.
    if (da[2] < 0.0 && db[3] < 0.0) || (da[3] < 0.0 && db[2] < 0.0) {
        da[2] = -da[2];
        da[3] = -da[3];
        da[4] += if da[4] < 0.0 {
            std::f64::consts::PI
        } else {
            -std::f64::consts::PI
        };
    }
    // El camino de rotación más corto.
    if (da[4] - db[4]).abs() > std::f64::consts::PI {
        if da[4] > db[4] {
            da[4] -= 2.0 * std::f64::consts::PI;
        } else {
            db[4] -= 2.0 * std::f64::consts::PI;
        }
    }
    let mut d = [0.0; 9];
    for i in 0..9 {
        d[i] = da[i] + (db[i] - da[i]) * p;
    }
    TransformFunction::Matrix(recompose_2d(&d))
}

/// Interpola dos valores de `transform` (CSS Transforms 1 §9): función a función si las listas
/// son compatibles; si no, por matrices descompuestas (con la caja de referencia `size`).
pub fn lerp_transform(a: &Transform, b: &Transform, p: f64, size: (f32, f32)) -> Transform {
    let (fa, fb) = (&a.0, &b.0);
    // `none` contra una lista: la lista neutra del mismo tipo.
    let neutral_list = |list: &[TransformFunction]| -> Option<Vec<TransformFunction>> {
        list.iter().map(neutral).collect()
    };
    let (fa, fb): (Vec<TransformFunction>, Vec<TransformFunction>) =
        if fa.is_empty() && fb.is_empty() {
            return Transform::default();
        } else if fa.is_empty() {
            match neutral_list(fb) {
                Some(n) => (n, fb.clone()),
                None => (Vec::new(), fb.clone()),
            }
        } else if fb.is_empty() {
            match neutral_list(fa) {
                Some(n) => (fa.clone(), n),
                None => (fa.clone(), Vec::new()),
            }
        } else {
            (fa.clone(), fb.clone())
        };
    if fa.len() == fb.len() {
        let pairwise: Option<Vec<TransformFunction>> = fa
            .iter()
            .zip(&fb)
            .map(|(x, y)| lerp_function(x, y, p))
            .collect();
        if let Some(list) = pairwise {
            return Transform(list);
        }
    }
    let ma = Transform(fa).matrix(size.0, size.1);
    let mb = Transform(fb).matrix(size.0, size.1);
    Transform(vec![lerp_matrix(&ma, &mb, p)])
}

/// `matrix()`/`matrix3d()` de un `transform` interpolado (para depurar).
pub fn transform_css(t: &Transform, size: (f32, f32)) -> String {
    matrix_css(&t.matrix(size.0, size.1))
}

fn discrete(a: &Value, b: &Value, p: f64) -> Value {
    if p < 0.5 { a.clone() } else { b.clone() }
}

/// ¿Se interpola `id` de forma continua (no discreta)?
pub fn is_interpolable(id: LonghandId) -> bool {
    id.def().animatable
        || matches!(
            id,
            LonghandId::Transform
                | LonghandId::TransformOrigin
                | LonghandId::Translate
                | LonghandId::Rotate
                | LonghandId::Scale
        )
}

/// Interpola dos valores computados de un mismo longhand en `p` (0 = `a`, 1 = `b`). Los
/// colores `currentcolor` se resuelven con el `color` de cada extremo (`colors`). Los pares no
/// interpolables cambian a mitad de camino (animación discreta, CSS Values 4 §3).
pub fn interpolate(a: &Value, b: &Value, p: f64, colors: (Rgba, Rgba), size: (f32, f32)) -> Value {
    use Value as V;
    let lp_auto = |x: &LengthPercentageAuto, y: &LengthPercentageAuto| match (x, y) {
        (LengthPercentageAuto::LengthPercentage(x), LengthPercentageAuto::LengthPercentage(y)) => {
            Some(LengthPercentageAuto::LengthPercentage(lerp_lp(x, y, p)))
        }
        _ => None,
    };
    let size_v = |x: &Size, y: &Size| match (x, y) {
        (Size::LengthPercentage(x), Size::LengthPercentage(y)) => {
            Some(Size::LengthPercentage(lerp_lp(x, y, p)))
        }
        _ => None,
    };
    let max_v = |x: &MaxSize, y: &MaxSize| match (x, y) {
        (MaxSize::LengthPercentage(x), MaxSize::LengthPercentage(y)) => {
            Some(MaxSize::LengthPercentage(lerp_lp(x, y, p)))
        }
        _ => None,
    };
    let nn = |x: &LengthPercentage| x.clone();
    let clamp0 = |l: LengthPercentage| match l {
        LengthPercentage::Length(l) if l.value < 0.0 => LengthPercentage::zero(),
        LengthPercentage::Percentage(v) if v < 0.0 => LengthPercentage::Percentage(0.0),
        other => other,
    };
    let r: Option<Value> = match (a, b) {
        (V::Top(x), V::Top(y)) => lp_auto(x, y).map(V::Top),
        (V::Right(x), V::Right(y)) => lp_auto(x, y).map(V::Right),
        (V::Bottom(x), V::Bottom(y)) => lp_auto(x, y).map(V::Bottom),
        (V::Left(x), V::Left(y)) => lp_auto(x, y).map(V::Left),
        (V::MarginTop(x), V::MarginTop(y)) => lp_auto(x, y).map(V::MarginTop),
        (V::MarginRight(x), V::MarginRight(y)) => lp_auto(x, y).map(V::MarginRight),
        (V::MarginBottom(x), V::MarginBottom(y)) => lp_auto(x, y).map(V::MarginBottom),
        (V::MarginLeft(x), V::MarginLeft(y)) => lp_auto(x, y).map(V::MarginLeft),
        (V::Width(x), V::Width(y)) => size_v(x, y).map(V::Width),
        (V::Height(x), V::Height(y)) => size_v(x, y).map(V::Height),
        (V::MinWidth(x), V::MinWidth(y)) => size_v(x, y).map(V::MinWidth),
        (V::MinHeight(x), V::MinHeight(y)) => size_v(x, y).map(V::MinHeight),
        (V::MaxWidth(x), V::MaxWidth(y)) => max_v(x, y).map(V::MaxWidth),
        (V::MaxHeight(x), V::MaxHeight(y)) => max_v(x, y).map(V::MaxHeight),
        (V::PaddingTop(x), V::PaddingTop(y)) => Some(V::PaddingTop(NonNegative(clamp0(lerp_lp(
            &nn(&x.0),
            &y.0,
            p,
        ))))),
        (V::PaddingRight(x), V::PaddingRight(y)) => {
            Some(V::PaddingRight(NonNegative(clamp0(lerp_lp(&x.0, &y.0, p)))))
        }
        (V::PaddingBottom(x), V::PaddingBottom(y)) => Some(V::PaddingBottom(NonNegative(clamp0(
            lerp_lp(&x.0, &y.0, p),
        )))),
        (V::PaddingLeft(x), V::PaddingLeft(y)) => {
            Some(V::PaddingLeft(NonNegative(clamp0(lerp_lp(&x.0, &y.0, p)))))
        }
        (V::BorderTopWidth(x), V::BorderTopWidth(y)) => Some(V::BorderTopWidth(BorderWidth(
            Length::px(lerp(x.px(), y.px(), p).max(0.0)),
        ))),
        (V::BorderRightWidth(x), V::BorderRightWidth(y)) => Some(V::BorderRightWidth(BorderWidth(
            Length::px(lerp(x.px(), y.px(), p).max(0.0)),
        ))),
        (V::BorderBottomWidth(x), V::BorderBottomWidth(y)) => Some(V::BorderBottomWidth(
            BorderWidth(Length::px(lerp(x.px(), y.px(), p).max(0.0))),
        )),
        (V::BorderLeftWidth(x), V::BorderLeftWidth(y)) => Some(V::BorderLeftWidth(BorderWidth(
            Length::px(lerp(x.px(), y.px(), p).max(0.0)),
        ))),
        (V::OutlineWidth(x), V::OutlineWidth(y)) => Some(V::OutlineWidth(BorderWidth(Length::px(
            lerp(x.px(), y.px(), p).max(0.0),
        )))),
        (V::BorderTopColor(x), V::BorderTopColor(y)) => {
            Some(V::BorderTopColor(lerp_color(x, y, colors, p)))
        }
        (V::BorderRightColor(x), V::BorderRightColor(y)) => {
            Some(V::BorderRightColor(lerp_color(x, y, colors, p)))
        }
        (V::BorderBottomColor(x), V::BorderBottomColor(y)) => {
            Some(V::BorderBottomColor(lerp_color(x, y, colors, p)))
        }
        (V::BorderLeftColor(x), V::BorderLeftColor(y)) => {
            Some(V::BorderLeftColor(lerp_color(x, y, colors, p)))
        }
        (V::BackgroundColor(x), V::BackgroundColor(y)) => {
            Some(V::BackgroundColor(lerp_color(x, y, colors, p)))
        }
        (V::TextDecorationColor(x), V::TextDecorationColor(y)) => {
            Some(V::TextDecorationColor(lerp_color(x, y, colors, p)))
        }
        (V::OutlineColor(x), V::OutlineColor(y)) => Some(V::OutlineColor(OutlineColor(
            lerp_color(&x.0, &y.0, colors, p),
        ))),
        (V::Color(x), V::Color(y)) => Some(V::Color(ColorProperty(Color::Rgba(lerp_rgba(
            x.rgba(),
            y.rgba(),
            p,
        ))))),
        (V::Opacity(x), V::Opacity(y)) => {
            Some(V::Opacity(Opacity(lerp(x.0, y.0, p).clamp(0.0, 1.0))))
        }
        (V::FlexGrow(x), V::FlexGrow(y)) => {
            Some(V::FlexGrow(NonNegativeNumber(lerp(x.0, y.0, p).max(0.0))))
        }
        (V::FlexShrink(x), V::FlexShrink(y)) => {
            Some(V::FlexShrink(NonNegativeNumber(lerp(x.0, y.0, p).max(0.0))))
        }
        (V::Order(x), V::Order(y)) => Some(V::Order(Integer(
            lerp(x.0 as f32, y.0 as f32, p).round() as i32,
        ))),
        (V::ZIndex(ZIndex::Integer(x)), V::ZIndex(ZIndex::Integer(y))) => Some(V::ZIndex(
            ZIndex::Integer(lerp(*x as f32, *y as f32, p).round() as i32),
        )),
        (V::FontSize(x), V::FontSize(y)) => {
            Some(V::FontSize(FontSize::Px(lerp(x.px(), y.px(), p).max(0.0))))
        }
        (V::FontWeight(x), V::FontWeight(y)) => Some(V::FontWeight(FontWeight::Absolute(
            lerp(x.value(), y.value(), p).clamp(1.0, 1000.0),
        ))),
        (V::LineHeight(LineHeight::Number(x)), V::LineHeight(LineHeight::Number(y))) => {
            Some(V::LineHeight(LineHeight::Number(lerp(*x, *y, p).max(0.0))))
        }
        (
            V::LineHeight(LineHeight::LengthPercentage(x)),
            V::LineHeight(LineHeight::LengthPercentage(y)),
        ) => Some(V::LineHeight(LineHeight::LengthPercentage(lerp_lp(
            x, y, p,
        )))),
        (V::LetterSpacing(Spacing::Length(x)), V::LetterSpacing(Spacing::Length(y))) => Some(
            V::LetterSpacing(Spacing::Length(Length::px(lerp(x.value, y.value, p)))),
        ),
        (V::WordSpacing(Spacing::Length(x)), V::WordSpacing(Spacing::Length(y))) => Some(
            V::WordSpacing(Spacing::Length(Length::px(lerp(x.value, y.value, p)))),
        ),
        (V::TextIndent(x), V::TextIndent(y)) => Some(V::TextIndent(lerp_lp(x, y, p))),
        (V::OutlineOffset(x), V::OutlineOffset(y)) => {
            Some(V::OutlineOffset(LengthPx(lerp_lp(&x.0, &y.0, p))))
        }
        (V::BorderTopLeftRadius(x), V::BorderTopLeftRadius(y)) => {
            Some(V::BorderTopLeftRadius(CornerRadius(
                clamp0(lerp_lp(&x.0, &y.0, p)),
                clamp0(lerp_lp(&x.1, &y.1, p)),
            )))
        }
        (V::BorderTopRightRadius(x), V::BorderTopRightRadius(y)) => {
            Some(V::BorderTopRightRadius(CornerRadius(
                clamp0(lerp_lp(&x.0, &y.0, p)),
                clamp0(lerp_lp(&x.1, &y.1, p)),
            )))
        }
        (V::BorderBottomRightRadius(x), V::BorderBottomRightRadius(y)) => {
            Some(V::BorderBottomRightRadius(CornerRadius(
                clamp0(lerp_lp(&x.0, &y.0, p)),
                clamp0(lerp_lp(&x.1, &y.1, p)),
            )))
        }
        (V::BorderBottomLeftRadius(x), V::BorderBottomLeftRadius(y)) => {
            Some(V::BorderBottomLeftRadius(CornerRadius(
                clamp0(lerp_lp(&x.0, &y.0, p)),
                clamp0(lerp_lp(&x.1, &y.1, p)),
            )))
        }
        (
            V::VerticalAlign(VerticalAlign::LengthPercentage(x)),
            V::VerticalAlign(VerticalAlign::LengthPercentage(y)),
        ) => Some(V::VerticalAlign(VerticalAlign::LengthPercentage(lerp_lp(
            x, y, p,
        )))),
        (V::RowGap(Gap::LengthPercentage(x)), V::RowGap(Gap::LengthPercentage(y))) => {
            Some(V::RowGap(Gap::LengthPercentage(clamp0(lerp_lp(x, y, p)))))
        }
        (V::ColumnGap(Gap::LengthPercentage(x)), V::ColumnGap(Gap::LengthPercentage(y))) => Some(
            V::ColumnGap(Gap::LengthPercentage(clamp0(lerp_lp(x, y, p)))),
        ),
        // `visibility` (CSS Values 4 §3): `visible` si uno de los extremos lo es, en (0, 1).
        (V::Visibility(x), V::Visibility(y)) => {
            let vis = Visibility::Visible;
            Some(V::Visibility(if *x == vis && *y != vis {
                if p >= 1.0 { *y } else { vis }
            } else if *y == vis && *x != vis {
                if p <= 0.0 { *x } else { vis }
            } else {
                discrete_vis(*x, *y, p)
            }))
        }
        (V::Transform(x), V::Transform(y)) => Some(V::Transform(lerp_transform(x, y, p, size))),
        (V::TransformOrigin(x), V::TransformOrigin(y)) => Some(V::TransformOrigin(
            crate::properties::transform::TransformOrigin {
                x: lerp_lp(&x.x, &y.x, p),
                y: lerp_lp(&x.y, &y.y, p),
                z: lerp_lp(&x.z, &y.z, p),
            },
        )),
        (V::Translate(x), V::Translate(y)) => {
            let zero = || {
                (
                    LengthPercentage::zero(),
                    LengthPercentage::zero(),
                    LengthPercentage::zero(),
                )
            };
            let (tx, ty) = (
                x.0.clone().unwrap_or_else(zero),
                y.0.clone().unwrap_or_else(zero),
            );
            Some(V::Translate(crate::properties::transform::Translate(Some(
                (
                    lerp_lp(&tx.0, &ty.0, p),
                    lerp_lp(&tx.1, &ty.1, p),
                    lerp_lp(&tx.2, &ty.2, p),
                ),
            ))))
        }
        (V::Scale(x), V::Scale(y)) => {
            let (sx, sy) = (x.0.unwrap_or([1.0; 3]), y.0.unwrap_or([1.0; 3]));
            Some(V::Scale(crate::properties::transform::Scale(Some([
                lerp(sx[0], sy[0], p),
                lerp(sx[1], sy[1], p),
                lerp(sx[2], sy[2], p),
            ]))))
        }
        (V::Rotate(x), V::Rotate(y)) => {
            let (rx, ry) = (
                x.0.unwrap_or(([0.0, 0.0, 1.0], 0.0)),
                y.0.unwrap_or(([0.0, 0.0, 1.0], 0.0)),
            );
            let axis = if x.0.is_none() { ry.0 } else { rx.0 };
            if rx.0 == ry.0 || x.0.is_none() || y.0.is_none() {
                Some(V::Rotate(crate::properties::transform::Rotate(Some((
                    axis,
                    lerp(rx.1, ry.1, p),
                )))))
            } else {
                None
            }
        }
        _ => None,
    };
    r.unwrap_or_else(|| discrete(a, b, p))
}

fn discrete_vis(a: Visibility, b: Visibility, p: f64) -> Visibility {
    if p < 0.5 { a } else { b }
}

// --- Keyframes ---

/// Las reglas `@keyframes` que aplican, por nombre (la última con cada nombre gana, CSS
/// Animations 1 §4).
pub fn keyframes_rules<'a>(
    sheets: &[&'a Stylesheet],
    media: &bf_css::media::MediaContext,
) -> HashMap<String, &'a KeyframesRule> {
    fn walk<'a>(
        rules: &'a [Rule],
        media: &bf_css::media::MediaContext,
        out: &mut HashMap<String, &'a KeyframesRule>,
    ) {
        for rule in rules {
            match rule {
                Rule::Keyframes(k) => {
                    out.insert(k.name.clone(), k);
                }
                Rule::Media(m) if m.queries.evaluate(media) => walk(&m.rules, media, out),
                Rule::Supports(s) if s.condition.evaluate(&is_supported) => {
                    walk(&s.rules, media, out);
                }
                Rule::Layer(LayerRule::Block { rules, .. }) => walk(rules, media, out),
                _ => {}
            }
        }
    }
    let mut out = HashMap::new();
    for sheet in sheets {
        walk(&sheet.rules, media, &mut out);
    }
    out
}

/// Un keyframe ya computado para un elemento: offset, valores y easing.
#[derive(Debug, Clone)]
struct ComputedKeyframe {
    offset: f64,
    values: Vec<Value>,
    easing: Option<TimingFunction>,
}

/// Computa los keyframes de `rule` para el estilo `base` (CSS Animations 1 §4: las
/// declaraciones `!important` se ignoran; las propiedades de animación también).
fn compute_keyframes(
    rule: &KeyframesRule,
    base: &ComputedStyle,
    parent: Option<&ComputedStyle>,
) -> Vec<ComputedKeyframe> {
    let len = LengthContext {
        font_size: base.font_size_px(),
        line_height: base
            .line_height
            .resolve(base.font_size_px(), crate::values::NORMAL_LINE_HEIGHT),
        ..LengthContext::default()
    };
    let ctx = ComputeContext { len, parent };
    let mut out: Vec<ComputedKeyframe> = Vec::new();
    for kf in &rule.keyframes {
        let mut values = Vec::new();
        let mut easing = None;
        for d in &kf.declarations {
            if d.important {
                continue;
            }
            let value = trim(&d.value);
            let name = d.name.to_ascii_lowercase();
            if name == "animation-timing-function" {
                easing = TimingFunction::parse(value);
                continue;
            }
            for v in parse_longhands(&name, value) {
                if is_animation_property(v.id()) {
                    continue;
                }
                values.push((v.id().def().compute)(&v, &ctx));
            }
        }
        for &offset in &kf.offsets {
            // Keyframes con el mismo offset (y easing) se combinan (CSS Animations 1 §4).
            match out
                .iter_mut()
                .find(|k| k.offset == offset && k.easing == easing)
            {
                Some(k) => {
                    for v in &values {
                        k.values.retain(|x| x.id() != v.id());
                        k.values.push(v.clone());
                    }
                }
                None => out.push(ComputedKeyframe {
                    offset,
                    values: values.clone(),
                    easing: easing.clone(),
                }),
            }
        }
    }
    out.sort_by(|a, b| a.offset.total_cmp(&b.offset));
    out
}

/// Las propiedades de animación y transición no se animan.
fn is_animation_property(id: LonghandId) -> bool {
    id.name().starts_with("animation-") || id.name().starts_with("transition-")
}

/// Los valores de longhands de una declaración `name: value` (expandiendo shorthands).
fn parse_longhands(name: &str, value: &[ComponentValue]) -> Vec<Value> {
    if let Some(id) = longhand_by_name(name) {
        return (id.def().parse)(value).into_iter().collect();
    }
    if let Some(sh) = shorthand_by_name(name) {
        return (sh.expand)(value).unwrap_or_default();
    }
    Vec::new()
}

// --- Modelo de tiempo (Web Animations 1 §4) ---

/// Parámetros de tiempo de un efecto.
#[derive(Debug, Clone, PartialEq)]
pub struct Timing {
    /// Retardo en ms.
    pub delay: f64,
    /// Duración de una iteración en ms.
    pub duration: f64,
    pub iterations: f64,
    pub direction: AnimationDirection,
    pub fill: FillMode,
    /// El easing de todo el efecto (`linear` para las animaciones CSS).
    pub easing: TimingFunction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Before,
    Active,
    After,
}

/// El estado calculado de un efecto en un instante (Web Animations 1 §4.7).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComputedTiming {
    pub phase: Phase,
    /// Tiempo activo en ms (`None` si el efecto no tiene efecto en este instante).
    pub active_time: Option<f64>,
    pub current_iteration: Option<f64>,
    /// Progreso transformado (tras dirección y easing).
    pub progress: Option<f64>,
}

impl Timing {
    pub fn active_duration(&self) -> f64 {
        if self.iterations == 0.0 || self.duration == 0.0 {
            0.0
        } else {
            self.duration * self.iterations
        }
    }

    pub fn end_time(&self) -> f64 {
        (self.delay + self.active_duration()).max(0.0)
    }

    /// El estado en el tiempo local `local` (ms).
    pub fn at(&self, local: f64, playing_backwards: bool) -> ComputedTiming {
        let active = self.active_duration();
        let before_end = self.delay.max(0.0).min(self.end_time());
        let active_end = (self.delay + active).min(self.end_time());
        let phase = if local < before_end || (playing_backwards && local == before_end) {
            Phase::Before
        } else if local >= active_end && !(playing_backwards && local == active_end && false) {
            Phase::After
        } else {
            Phase::Active
        };
        let fill_back = matches!(self.fill, FillMode::Backwards | FillMode::Both);
        let fill_fwd = matches!(self.fill, FillMode::Forwards | FillMode::Both);
        let active_time = match phase {
            Phase::Before if fill_back => Some((local - self.delay).max(0.0)),
            Phase::Before => None,
            Phase::Active => Some(local - self.delay),
            Phase::After if fill_fwd => Some((local - self.delay).min(active).max(0.0)),
            Phase::After => None,
        };
        let Some(at) = active_time else {
            return ComputedTiming {
                phase,
                active_time: None,
                current_iteration: None,
                progress: None,
            };
        };
        // Progreso total y de la iteración (§4.8).
        let overall = if self.duration == 0.0 {
            if phase == Phase::Before {
                0.0
            } else {
                self.iterations
            }
        } else {
            at / self.duration
        };
        let mut simple = if overall.is_infinite() {
            0.0
        } else {
            overall % 1.0
        };
        if simple == 0.0
            && (phase == Phase::Active || phase == Phase::After)
            && at == active
            && self.iterations != 0.0
            && overall != 0.0
        {
            simple = 1.0;
        }
        let iteration = if phase == Phase::After && self.iterations.is_infinite() {
            f64::INFINITY
        } else if simple == 1.0 {
            overall.floor() - 1.0
        } else {
            overall.floor()
        };
        let forwards = match self.direction {
            AnimationDirection::Normal => true,
            AnimationDirection::Reverse => false,
            AnimationDirection::Alternate => iteration % 2.0 == 0.0 || iteration.is_infinite(),
            AnimationDirection::AlternateReverse => {
                !(iteration % 2.0 == 0.0 || iteration.is_infinite())
            }
        };
        let directed = if forwards { simple } else { 1.0 - simple };
        let before_flag = phase == Phase::Before || (!forwards && phase == Phase::After && false);
        ComputedTiming {
            phase,
            active_time: Some(at),
            current_iteration: Some(iteration),
            progress: Some(self.easing.eval(directed, before_flag)),
        }
    }
}

// --- Animaciones y transiciones en curso ---

/// Una animación CSS en curso (CSS Animations 1 §3).
#[derive(Debug, Clone)]
pub struct CssAnimation {
    pub name: String,
    /// Índice en `animation-name` (orden de composición).
    pub index: usize,
    /// Tiempo de la línea de tiempo en que empezó (ms); `None` mientras está pausada antes de
    /// empezar.
    pub start_time: Option<f64>,
    /// Tiempo local congelado mientras está pausada.
    pub hold_time: Option<f64>,
    pub timing: Timing,
    /// Easing de los keyframes (`animation-timing-function`).
    pub keyframe_easing: TimingFunction,
    pub paused: bool,
    last_phase: Option<Phase>,
    last_iteration: Option<f64>,
    /// El script cambió el tiempo o el estado (Web Animations): se respeta hasta que cambie el
    /// `animation-name`.
    pub overridden: bool,
    /// El identificador (para la API de Web Animations).
    pub id: u64,
}

impl CssAnimation {
    pub fn local_time(&self, now: f64) -> Option<f64> {
        match (self.hold_time, self.start_time) {
            (Some(h), _) => Some(h),
            (None, Some(s)) => Some(now - s),
            _ => None,
        }
    }
}

/// Una transición en curso (CSS Transitions 1 §3).
#[derive(Debug, Clone)]
pub struct Transition {
    pub property: LonghandId,
    pub from: Value,
    pub to: Value,
    pub start_time: f64,
    pub hold_time: Option<f64>,
    pub timing: Timing,
    /// "reversing-adjusted start value" y "reversing shortening factor" (§3, paso 4).
    pub reversing_adjusted_from: Value,
    pub reversing_shortening: f64,
    last_phase: Option<Phase>,
    pub id: u64,
    /// `transition-property` como se escribió (para `transitionProperty`).
    pub name: String,
}

/// Un evento de animación pendiente de despachar.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimationEvent {
    pub target: Key,
    /// `animationstart`, `animationiteration`, `animationend`, `animationcancel`,
    /// `transitionrun`, `transitionstart`, `transitionend` o `transitioncancel`.
    pub kind: &'static str,
    /// `animationName` o `propertyName`.
    pub name: String,
    /// `elapsedTime` en segundos.
    pub elapsed: f64,
    /// Tiempo de la línea de tiempo del evento (para ordenarlos, CSS Animations 2 §4.2).
    pub time: f64,
}

#[derive(Debug, Default)]
struct ElementState {
    animations: Vec<CssAnimation>,
    transitions: Vec<Transition>,
    /// El estilo de la cascada y el que se mostró en el último "style change event".
    base: Option<Arc<ComputedStyle>>,
    shown: Option<Arc<ComputedStyle>>,
    /// El tamaño de la caja de referencia (para interpolar `transform` por matrices).
    size: (f32, f32),
}

/// Las animaciones y transiciones CSS de un documento.
#[derive(Debug, Default)]
pub struct Animator {
    elements: HashMap<Key, ElementState>,
    events: Vec<AnimationEvent>,
    next_id: u64,
    /// ¿Queda algo animándose (hace falta otro cuadro)?
    active: bool,
}

/// Lo que cada elemento animado necesita del documento para actualizarse.
pub struct UpdateInput<'a> {
    pub doc: &'a Document,
    pub keyframes: &'a HashMap<String, &'a KeyframesRule>,
    /// Tiempo actual de la línea de tiempo del documento (ms).
    pub now: f64,
}

impl Animator {
    pub fn new() -> Animator {
        Animator::default()
    }

    /// ¿Hay animaciones o transiciones que necesiten cuadros nuevos?
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// ¿Hay alguna animación o transición (activa o con relleno)?
    pub fn is_empty(&self) -> bool {
        self.elements
            .values()
            .all(|e| e.animations.is_empty() && e.transitions.is_empty())
    }

    /// Los eventos pendientes, en orden.
    pub fn take_events(&mut self) -> Vec<AnimationEvent> {
        let mut ev = std::mem::take(&mut self.events);
        ev.sort_by(|a, b| a.time.total_cmp(&b.time));
        ev
    }

    /// Las animaciones CSS de `key`.
    pub fn animations(&self, key: &Key) -> &[CssAnimation] {
        self.elements.get(key).map_or(&[], |e| &e.animations)
    }

    pub fn animations_mut(&mut self, key: &Key) -> Option<&mut Vec<CssAnimation>> {
        self.elements.get_mut(key).map(|e| &mut e.animations)
    }

    /// Las transiciones de `key`.
    pub fn transitions(&self, key: &Key) -> &[Transition] {
        self.elements.get(key).map_or(&[], |e| &e.transitions)
    }

    pub fn transitions_mut(&mut self, key: &Key) -> Option<&mut Vec<Transition>> {
        self.elements.get_mut(key).map(|e| &mut e.transitions)
    }

    /// Los elementos con animaciones o transiciones, en orden de árbol.
    pub fn keys(&self, doc: &Document) -> Vec<Key> {
        let order: HashMap<NodeId, usize> = doc
            .inclusive_descendants(doc.root())
            .into_iter()
            .enumerate()
            .map(|(i, n)| (n, i))
            .collect();
        let mut keys: Vec<Key> = self
            .elements
            .iter()
            .filter(|(_, e)| !e.animations.is_empty() || !e.transitions.is_empty())
            .map(|(k, _)| *k)
            .collect();
        keys.sort_by_key(|k| {
            (
                order.get(&k.0).copied().unwrap_or(usize::MAX),
                k.1.map(|p| p as u8),
            )
        });
        keys
    }

    /// Anota el tamaño de la caja de referencia de `key` (tras el layout).
    pub fn set_box_size(&mut self, key: Key, size: (f32, f32)) {
        if let Some(e) = self.elements.get_mut(&key) {
            e.size = size;
        }
    }

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// El "style change event": actualiza las animaciones y transiciones con los estilos de la
    /// cascada `styles` y reemplaza en ellos los valores animados en el instante `input.now`.
    pub fn update(&mut self, styles: &mut StyleMap, input: &UpdateInput<'_>) {
        let now = input.now;
        self.active = false;
        // Elementos que ya no tienen estilo (o con `display: none`): se cancelan.
        let gone: Vec<Key> = self
            .elements
            .keys()
            .filter(|k| {
                styles
                    .get(k)
                    .is_none_or(|s| s.display == Display::None || !input.doc.is_connected(k.0))
            })
            .copied()
            .collect();
        for key in gone {
            if let Some(state) = self.elements.remove(&key) {
                for a in state.animations {
                    if a.last_phase == Some(Phase::Active) {
                        self.events.push(AnimationEvent {
                            target: key,
                            kind: "animationcancel",
                            name: a.name.clone(),
                            elapsed: 0.0,
                            time: now,
                        });
                    }
                }
                for t in state.transitions {
                    self.events.push(AnimationEvent {
                        target: key,
                        kind: "transitioncancel",
                        name: t.name.clone(),
                        elapsed: 0.0,
                        time: now,
                    });
                }
            }
        }
        // En orden de árbol, para heredar los valores animados de los padres.
        let order: Vec<Key> = {
            let mut v: Vec<Key> = Vec::new();
            for n in input.doc.inclusive_descendants(input.doc.root()) {
                for pseudo in [
                    None,
                    Some(PseudoElement::Marker),
                    Some(PseudoElement::Before),
                    Some(PseudoElement::After),
                ] {
                    if styles.contains_key(&(n, pseudo)) {
                        v.push((n, pseudo));
                    }
                }
            }
            v
        };
        // Valores animados heredables de cada nodo animado: (propiedad, base, animado).
        let mut inherited_changes: HashMap<NodeId, Vec<(LonghandId, Value, Value)>> =
            HashMap::new();
        for key in order {
            let Some(base) = styles.get(&key).cloned() else {
                continue;
            };
            if base.display == Display::None {
                continue;
            }
            // Herencia de los valores animados del padre (si el hijo heredaba el valor base).
            let parent_node = input.doc.parent(key.0);
            let parent_key = match key.1 {
                Some(_) => Some(key.0),
                None => parent_node,
            };
            let mut base = base;
            if let Some(changes) = parent_key.and_then(|p| inherited_changes.get(&p)).cloned() {
                let mut s = (*base).clone();
                let mut changed = false;
                for (id, pbase, panim) in &changes {
                    if s.get(*id) == *pbase {
                        s.set(panim.clone());
                        changed = true;
                    }
                }
                if changed {
                    base = Arc::new(s);
                }
                if key.1.is_none() {
                    inherited_changes.insert(key.0, changes);
                }
            }
            let parent_style = parent_key.and_then(|p| styles.get(&(p, None)).cloned());
            let has_anims = base.animation_name.0.iter().any(|n| n.name().is_some());
            let has_transitions = !base.transition_property.0.is_empty()
                && base.transition_property.0[0] != TransitionProperty::None
                && (base.transition_duration.0.iter().any(|d| d.secs() > 0.0)
                    || base.transition_delay.0.iter().any(|d| d.0 > 0.0)
                    || base
                        .transition_behavior
                        .0
                        .contains(&TransitionBehavior::AllowDiscrete));
            if !has_anims && !has_transitions && !self.elements.contains_key(&key) {
                continue;
            }
            let shown = self.update_element(key, &base, parent_style.as_deref(), input);
            if !Arc::ptr_eq(&shown, &base) || !Arc::ptr_eq(&base, &styles[&key]) {
                if key.1.is_none() {
                    let mut list = inherited_changes.remove(&key.0).unwrap_or_default();
                    for def in PROPERTIES.iter().filter(|d| d.inherited) {
                        let (b, a) = (base.get(def.id), shown.get(def.id));
                        if b != a {
                            list.retain(|(id, _, _)| *id != def.id);
                            list.push((def.id, styles[&key].get(def.id), a));
                        }
                    }
                    if !list.is_empty() {
                        inherited_changes.insert(key.0, list);
                    }
                }
                styles.insert(key, shown);
            }
        }
    }

    /// Actualiza un elemento y devuelve el estilo a mostrar.
    fn update_element(
        &mut self,
        key: Key,
        base: &Arc<ComputedStyle>,
        parent: Option<&ComputedStyle>,
        input: &UpdateInput<'_>,
    ) -> Arc<ComputedStyle> {
        let now = input.now;
        let mut state = self.elements.remove(&key).unwrap_or_default();
        let mut events = Vec::new();

        // 1. Animaciones CSS (CSS Animations 1 §3): una por nombre de `animation-name` con
        //    `@keyframes`.
        let mut kept: Vec<CssAnimation> = Vec::new();
        let mut old = std::mem::take(&mut state.animations);
        for (i, name) in base.animation_name.0.iter().enumerate() {
            let Some(name) = name.name() else {
                continue;
            };
            if !input.keyframes.contains_key(name) {
                continue;
            }
            let timing = Timing {
                delay: f64::from(base.animation_delay.get(i).map_or(0.0, |t| t.0)) * 1000.0,
                duration: f64::from(base.animation_duration.get(i).map_or(0.0, |d| d.secs()))
                    * 1000.0,
                iterations: f64::from(base.animation_iteration_count.get(i).map_or(1.0, |c| c.0)),
                direction: base
                    .animation_direction
                    .get(i)
                    .copied()
                    .unwrap_or(AnimationDirection::Normal),
                fill: base
                    .animation_fill_mode
                    .get(i)
                    .copied()
                    .unwrap_or(FillMode::None),
                easing: TimingFunction::Linear,
            };
            let paused = base.animation_play_state.get(i) == Some(&PlayState::Paused);
            let easing = base
                .animation_timing_function
                .get(i)
                .cloned()
                .unwrap_or(TimingFunction::EASE);
            let existing = old.iter().position(|a| a.name == name);
            let mut anim = match existing {
                Some(pos) => old.remove(pos),
                None => {
                    let id = self.id();
                    CssAnimation {
                        name: name.to_owned(),
                        index: i,
                        start_time: if paused { None } else { Some(now) },
                        hold_time: if paused { Some(0.0) } else { None },
                        timing: timing.clone(),
                        keyframe_easing: easing.clone(),
                        paused,
                        last_phase: None,
                        last_iteration: None,
                        overridden: false,
                        id,
                    }
                }
            };
            anim.index = i;
            if !anim.overridden {
                anim.timing = timing;
                anim.keyframe_easing = easing;
                // `animation-play-state` (§3.9): pausar congela el tiempo local.
                if paused && !anim.paused {
                    anim.hold_time = anim.local_time(now);
                    anim.paused = true;
                } else if !paused && anim.paused {
                    let hold = anim.hold_time.unwrap_or(0.0);
                    anim.start_time = Some(now - hold);
                    anim.hold_time = None;
                    anim.paused = false;
                }
            }
            kept.push(anim);
        }
        // Las que ya no están se cancelan (CSS Animations 2 §4.2, `animationcancel`).
        for a in old {
            if matches!(a.last_phase, Some(Phase::Active) | Some(Phase::Before)) {
                let elapsed = a.local_time(now).map_or(0.0, |t| {
                    (t - a.timing.delay).clamp(0.0, a.timing.active_duration())
                });
                events.push(AnimationEvent {
                    target: key,
                    kind: "animationcancel",
                    name: a.name.clone(),
                    elapsed: elapsed / 1000.0,
                    time: now,
                });
            }
        }
        state.animations = kept;

        // 2. Transiciones (CSS Transitions 1 §3): se comparan el estilo anterior y el nuevo.
        let previous_base = state.base.clone();
        let previous_shown = state.shown.clone();
        let colors_of = |s: &ComputedStyle| s.color.rgba();
        // Las que terminaron se quitan.
        let mut transitions = std::mem::take(&mut state.transitions);
        let props = transition_properties(base);
        if let (Some(prev), Some(shown_prev)) = (&previous_base, &previous_shown) {
            for (id, i) in &props {
                let (id, i) = (*id, *i);
                let after = base.get(id);
                let before_base = prev.get(id);
                let running = transitions.iter().position(|t| t.property == id);
                let duration =
                    f64::from(base.transition_duration.get(i).map_or(0.0, |d| d.secs())) * 1000.0;
                let delay = f64::from(base.transition_delay.get(i).map_or(0.0, |t| t.0)) * 1000.0;
                let behavior = base
                    .transition_behavior
                    .get(i)
                    .copied()
                    .unwrap_or(TransitionBehavior::Normal);
                let easing = base
                    .transition_timing_function
                    .get(i)
                    .cloned()
                    .unwrap_or(TimingFunction::EASE);
                let can_animate =
                    is_interpolable(id) || behavior == TransitionBehavior::AllowDiscrete;
                let combined = duration.max(0.0) + delay;
                if let Some(r) = running {
                    if transitions[r].to == after {
                        continue;
                    }
                    // Cambió el destino: se cancela la que corría (paso 4) y quizá se invierte.
                    let old_t = transitions.remove(r);
                    let current = current_value(&old_t, now, colors_of(base), state.size);
                    if combined <= 0.0 || !can_animate || current == after {
                        events.push(cancel_event(key, &old_t, now));
                        continue;
                    }
                    events.push(cancel_event(key, &old_t, now));
                    let id_n = self.id();
                    let (from, factor, adj_from, dur, del) =
                        if after == old_t.reversing_adjusted_from {
                            // Inversión: se acorta (§3, paso 4.4).
                            let progress = old_t
                                .timing
                                .at(now - old_t.start_time, false)
                                .progress
                                .unwrap_or(0.0)
                                .clamp(0.0, 1.0);
                            let factor = (progress * old_t.reversing_shortening
                                + (1.0 - old_t.reversing_shortening))
                                .clamp(0.0, 1.0);
                            let del = if delay < 0.0 { delay * factor } else { delay };
                            (
                                current.clone(),
                                factor,
                                old_t.to.clone(),
                                duration * factor,
                                del,
                            )
                        } else {
                            (current.clone(), 1.0, current.clone(), duration, delay)
                        };
                    let t = Transition {
                        property: id,
                        from,
                        to: after.clone(),
                        start_time: now,
                        hold_time: None,
                        timing: Timing {
                            delay: del,
                            duration: dur,
                            iterations: 1.0,
                            direction: AnimationDirection::Normal,
                            fill: FillMode::Backwards,
                            easing,
                        },
                        reversing_adjusted_from: adj_from,
                        reversing_shortening: factor,
                        last_phase: None,
                        id: id_n,
                        name: id.name().to_owned(),
                    };
                    events.push(AnimationEvent {
                        target: key,
                        kind: "transitionrun",
                        name: t.name.clone(),
                        elapsed: (-t.timing.delay).clamp(0.0, t.timing.duration) / 1000.0,
                        time: now,
                    });
                    transitions.push(t);
                    continue;
                }
                if before_base == after || combined <= 0.0 || !can_animate {
                    continue;
                }
                let from = shown_prev.get(id);
                if from == after {
                    continue;
                }
                // Sin interpolación posible (p. ej. `auto`) no hay transición salvo
                // `allow-discrete`.
                if behavior != TransitionBehavior::AllowDiscrete
                    && !values_interpolate(&from, &after)
                {
                    continue;
                }
                let id_n = self.id();
                let t = Transition {
                    property: id,
                    from: from.clone(),
                    to: after.clone(),
                    start_time: now,
                    hold_time: None,
                    timing: Timing {
                        delay,
                        duration,
                        iterations: 1.0,
                        direction: AnimationDirection::Normal,
                        fill: FillMode::Backwards,
                        easing,
                    },
                    reversing_adjusted_from: from,
                    reversing_shortening: 1.0,
                    last_phase: None,
                    id: id_n,
                    name: id.name().to_owned(),
                };
                events.push(AnimationEvent {
                    target: key,
                    kind: "transitionrun",
                    name: t.name.clone(),
                    elapsed: (-t.timing.delay).clamp(0.0, t.timing.duration) / 1000.0,
                    time: now,
                });
                transitions.push(t);
            }
        }
        // Las transiciones de propiedades que ya no transicionan se cancelan (§3, paso 3).
        let mut i = 0;
        while i < transitions.len() {
            let still = props.iter().any(|(id, _)| *id == transitions[i].property);
            if !still {
                let t = transitions.remove(i);
                events.push(cancel_event(key, &t, now));
            } else {
                i += 1;
            }
        }

        // 3. El estilo mostrado: la cascada, más las transiciones, más las animaciones.
        let mut shown: Option<ComputedStyle> = None;
        let mut finished = Vec::new();
        for (ti, t) in transitions.iter_mut().enumerate() {
            let local = t.hold_time.unwrap_or(now - t.start_time);
            let ct = t.timing.at(local, false);
            let prev_phase = t.last_phase;
            t.last_phase = Some(ct.phase);
            if prev_phase != Some(ct.phase) {
                let start_elapsed = (-t.timing.delay).clamp(0.0, t.timing.duration) / 1000.0;
                let push = |events: &mut Vec<AnimationEvent>, kind, elapsed| {
                    events.push(AnimationEvent {
                        target: key,
                        kind,
                        name: t.name.clone(),
                        elapsed,
                        time: t.start_time
                            + if kind == "transitionend" {
                                t.timing.end_time()
                            } else {
                                t.timing.delay.max(0.0)
                            },
                    });
                };
                match (prev_phase, ct.phase) {
                    (Some(Phase::Before) | None, Phase::Active) => {
                        push(&mut events, "transitionstart", start_elapsed);
                    }
                    (Some(Phase::Before) | None, Phase::After) => {
                        push(&mut events, "transitionstart", start_elapsed);
                        push(&mut events, "transitionend", t.timing.duration / 1000.0);
                    }
                    (Some(Phase::Active), Phase::After) => {
                        push(&mut events, "transitionend", t.timing.duration / 1000.0);
                    }
                    _ => {}
                }
            }
            if ct.phase == Phase::After {
                finished.push(ti);
                continue;
            }
            self.active = true;
            if let Some(p) = ct.progress {
                let s = shown.get_or_insert_with(|| (**base).clone());
                let colors = (base.color.rgba(), base.color.rgba());
                s.set(interpolate(&t.from, &t.to, p, colors, state.size));
            }
        }
        for ti in finished.into_iter().rev() {
            transitions.remove(ti);
        }
        state.transitions = transitions;

        let mut anims = std::mem::take(&mut state.animations);
        for a in &mut anims {
            let Some(rule) = input.keyframes.get(&a.name) else {
                continue;
            };
            let Some(local) = a.local_time(now) else {
                continue;
            };
            let ct = a.timing.at(local, false);
            self.animation_events(key, a, &ct, now, &mut events);
            if !a.paused && (ct.phase != Phase::After || a.timing.iterations.is_infinite()) {
                self.active = true;
            }
            let Some(progress) = ct.progress else {
                continue;
            };
            let s = shown.get_or_insert_with(|| (**base).clone());
            let keyframes = compute_keyframes(rule, base, parent);
            apply_keyframes(
                s,
                base,
                &keyframes,
                &a.keyframe_easing,
                progress,
                state.size,
            );
        }
        state.animations = anims;
        state.base = Some(base.clone());
        let shown = match shown {
            Some(s) if s != **base => Arc::new(s),
            _ => base.clone(),
        };
        state.shown = Some(shown.clone());
        self.events.extend(events);
        self.elements.insert(key, state);
        shown
    }

    fn animation_events(
        &mut self,
        key: Key,
        a: &mut CssAnimation,
        ct: &ComputedTiming,
        now: f64,
        events: &mut Vec<AnimationEvent>,
    ) {
        let prev = a.last_phase;
        let prev_iter = a.last_iteration;
        a.last_phase = Some(ct.phase);
        a.last_iteration = ct.current_iteration;
        let active = a.timing.active_duration();
        let start_elapsed = (-a.timing.delay).clamp(0.0, active) / 1000.0;
        let end_elapsed = active / 1000.0;
        let start = a.start_time.unwrap_or(now);
        let mut push = |kind, elapsed: f64, time: f64| {
            events.push(AnimationEvent {
                target: key,
                kind,
                name: a.name.clone(),
                elapsed,
                time,
            });
        };
        // CSS Animations 2 §4.2: eventos según el cambio de fase.
        match (prev, ct.phase) {
            (None | Some(Phase::Before), Phase::Active) => {
                push(
                    "animationstart",
                    start_elapsed,
                    start + a.timing.delay.max(0.0),
                );
            }
            (None | Some(Phase::Before), Phase::After) => {
                push(
                    "animationstart",
                    start_elapsed,
                    start + a.timing.delay.max(0.0),
                );
                push("animationend", end_elapsed, start + a.timing.end_time());
            }
            (Some(Phase::Active), Phase::Before) => {
                push("animationend", start_elapsed, now);
            }
            (Some(Phase::Active), Phase::Active) => {
                if let (Some(p), Some(c)) = (prev_iter, ct.current_iteration)
                    && p != c
                {
                    let it = if c > p { p + 1.0 } else { p };
                    push(
                        "animationiteration",
                        it * a.timing.duration / 1000.0,
                        start + a.timing.delay + it * a.timing.duration,
                    );
                }
            }
            (Some(Phase::Active), Phase::After) => {
                push("animationend", end_elapsed, start + a.timing.end_time());
            }
            (Some(Phase::After), Phase::Active) => {
                push("animationstart", end_elapsed, now);
            }
            (Some(Phase::After), Phase::Before) => {
                push("animationstart", end_elapsed, now);
                push("animationend", start_elapsed, now);
            }
            _ => {}
        }
    }
}

fn cancel_event(key: Key, t: &Transition, now: f64) -> AnimationEvent {
    let local = t.hold_time.unwrap_or(now - t.start_time);
    let elapsed = (local - t.timing.delay).clamp(0.0, t.timing.duration);
    AnimationEvent {
        target: key,
        kind: "transitioncancel",
        name: t.name.clone(),
        elapsed: elapsed / 1000.0,
        time: now,
    }
}

/// El valor de una transición en `now`.
fn current_value(t: &Transition, now: f64, color: Rgba, size: (f32, f32)) -> Value {
    let local = t.hold_time.unwrap_or(now - t.start_time);
    match t.timing.at(local, false).progress {
        Some(p) => interpolate(&t.from, &t.to, p, (color, color), size),
        None => t.to.clone(),
    }
}

/// ¿Se interpolan `a` y `b` de forma continua?
fn values_interpolate(a: &Value, b: &Value) -> bool {
    let mid = interpolate(a, b, 0.5, (Rgba::BLACK, Rgba::BLACK), (0.0, 0.0));
    let q = interpolate(a, b, 0.25, (Rgba::BLACK, Rgba::BLACK), (0.0, 0.0));
    // Una animación discreta da `a` en 0.25 y `b` en 0.5.
    !(q == *a && mid == *b)
}

/// Los longhands que transicionan según `transition-property`, con el índice de su entrada
/// (la última que los nombra gana, CSS Transitions 1 §2.1).
fn transition_properties(style: &ComputedStyle) -> Vec<(LonghandId, usize)> {
    let mut out: Vec<(LonghandId, usize)> = Vec::new();
    let add = |id: LonghandId, i: usize, out: &mut Vec<(LonghandId, usize)>| {
        if is_animation_property(id) {
            return;
        }
        match out.iter_mut().find(|(x, _)| *x == id) {
            Some(slot) => slot.1 = i,
            None => out.push((id, i)),
        }
    };
    for (i, p) in style.transition_property.0.iter().enumerate() {
        match p {
            TransitionProperty::None => {}
            TransitionProperty::All => {
                for def in PROPERTIES {
                    add(def.id, i, &mut out);
                }
            }
            TransitionProperty::Name(n) => {
                if let Some(id) = longhand_by_name(n) {
                    add(id, i, &mut out);
                } else if let Some(sh) = shorthand_by_name(n) {
                    for l in sh.longhands {
                        add(*l, i, &mut out);
                    }
                }
            }
        }
    }
    out
}

/// Aplica los keyframes en el progreso `p` de la iteración (CSS Animations 1 §3 y Web
/// Animations 1 §5.3.4: los offsets 0 y 1 que faltan toman el valor de la cascada).
fn apply_keyframes(
    style: &mut ComputedStyle,
    base: &ComputedStyle,
    keyframes: &[ComputedKeyframe],
    default_easing: &TimingFunction,
    p: f64,
    size: (f32, f32),
) {
    let mut props: Vec<LonghandId> = Vec::new();
    for k in keyframes {
        for v in &k.values {
            if !props.contains(&v.id()) {
                props.push(v.id());
            }
        }
    }
    for id in props {
        // Los keyframes con esta propiedad, más los implícitos en 0 y 1.
        let mut frames: Vec<(f64, Value, Option<&TimingFunction>)> = keyframes
            .iter()
            .filter_map(|k| {
                k.values
                    .iter()
                    .find(|v| v.id() == id)
                    .map(|v| (k.offset, v.clone(), k.easing.as_ref()))
            })
            .collect();
        if frames.first().is_none_or(|f| f.0 > 0.0) {
            let easing = keyframes
                .iter()
                .find(|k| k.offset == 0.0)
                .and_then(|k| k.easing.as_ref());
            frames.insert(0, (0.0, base.get(id), easing));
        }
        if frames.last().is_none_or(|f| f.0 < 1.0) {
            frames.push((1.0, base.get(id), None));
        }
        // El intervalo que contiene `p` (fuera de [0, 1], el primero o el último).
        let n = frames.len();
        let i = if p < 0.0 {
            0
        } else if p >= 1.0 {
            frames
                .iter()
                .rposition(|f| f.0 < 1.0)
                .unwrap_or(0)
                .min(n - 2)
        } else {
            frames
                .iter()
                .rposition(|f| f.0 <= p)
                .unwrap_or(0)
                .min(n - 2)
        };
        let (a, b) = (&frames[i], &frames[i + 1]);
        let span = b.0 - a.0;
        let local = if span == 0.0 { 1.0 } else { (p - a.0) / span };
        let easing = a.2.unwrap_or(default_easing);
        let local = easing.eval(local, false);
        let color = base.color.rgba();
        let v = interpolate(&a.1, &b.1, local, (color, color), size);
        style.set(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_phases() {
        let t = Timing {
            delay: 100.0,
            duration: 1000.0,
            iterations: 2.0,
            direction: AnimationDirection::Alternate,
            fill: FillMode::None,
            easing: TimingFunction::Linear,
        };
        assert_eq!(t.at(50.0, false).phase, Phase::Before);
        assert_eq!(t.at(50.0, false).progress, None);
        let mid = t.at(600.0, false);
        assert_eq!(mid.phase, Phase::Active);
        assert_eq!(mid.progress, Some(0.5));
        // Segunda iteración, alternada: va hacia atrás.
        let back = t.at(1350.0, false);
        assert_eq!(back.current_iteration, Some(1.0));
        assert!((back.progress.unwrap() - 0.75).abs() < 1e-9);
        assert_eq!(t.at(2200.0, false).phase, Phase::After);
    }

    #[test]
    fn interpolates_lengths_and_colors() {
        let a = Value::Opacity(Opacity(0.0));
        let b = Value::Opacity(Opacity(1.0));
        assert_eq!(
            interpolate(&a, &b, 0.25, (Rgba::BLACK, Rgba::BLACK), (0.0, 0.0)),
            Value::Opacity(Opacity(0.25))
        );
        let a = Value::BackgroundColor(Color::Rgba(Rgba::rgb(0, 0, 0)));
        let b = Value::BackgroundColor(Color::Rgba(Rgba::rgb(200, 100, 0)));
        assert_eq!(
            interpolate(&a, &b, 0.5, (Rgba::BLACK, Rgba::BLACK), (0.0, 0.0)),
            Value::BackgroundColor(Color::Rgba(Rgba::rgb(100, 50, 0)))
        );
        let a = Value::Width(Size::Auto);
        let b = Value::Width(Size::LengthPercentage(LengthPercentage::px(10.0)));
        assert_eq!(
            interpolate(&a, &b, 0.4, (Rgba::BLACK, Rgba::BLACK), (0.0, 0.0)),
            a
        );
    }
}
