//! E4: `transform`, `transform-origin` (CSS Transforms 1 §6, §7) y las propiedades de
//! transformación individuales `translate`, `rotate` y `scale` (CSS Transforms 2 §5).
//!
//! Las funciones 3D (CSS Transforms 2 §12) se parsean y se componen en una matriz 4×4; la
//! pintura usa su aplanamiento ortográfico (CSS Transforms 2 §6.1, "flat"): la parte 2D de la
//! matriz, sin la división de perspectiva.

use std::fmt::Write as _;

use bf_css::syntax::{ComponentValue, split_comma_separated};

use super::{ComputeContext, PropValue};
use crate::values::{
    LengthPercentage, components, fmt_number, ident, is_ident, parse_angle, parse_length,
    parse_length_percentage, parse_number, trim,
};

/// Una matriz 4×4 en la convención de CSS (CSS Transforms 2 §12, `matrix3d()`): `m[i][j]` es
/// `m(i+1)(j+1)`, los puntos son vectores fila (`p' = p·M`) y la traslación está en la fila 4.
pub type Mat4 = [[f64; 4]; 4];

pub const IDENTITY: Mat4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// `a·b` (con vectores fila: primero `a`, luego `b`).
pub fn mat_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut r = [[0.0; 4]; 4];
    for (i, row) in r.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..4).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

fn translate_mat(x: f64, y: f64, z: f64) -> Mat4 {
    let mut m = IDENTITY;
    m[3][0] = x;
    m[3][1] = y;
    m[3][2] = z;
    m
}

fn scale_mat(x: f64, y: f64, z: f64) -> Mat4 {
    let mut m = IDENTITY;
    m[0][0] = x;
    m[1][1] = y;
    m[2][2] = z;
    m
}

/// `rotate3d(x, y, z, a)` (CSS Transforms 2 §12, "Mathematical Description").
fn rotate_mat(x: f64, y: f64, z: f64, deg: f64) -> Mat4 {
    let len = (x * x + y * y + z * z).sqrt();
    if len == 0.0 {
        return IDENTITY;
    }
    let (x, y, z) = (x / len, y / len, z / len);
    let a = deg.to_radians();
    let sc = (a / 2.0).sin() * (a / 2.0).cos();
    let sq = (a / 2.0).sin().powi(2);
    [
        [
            1.0 - 2.0 * (y * y + z * z) * sq,
            2.0 * (x * y * sq + z * sc),
            2.0 * (x * z * sq - y * sc),
            0.0,
        ],
        [
            2.0 * (x * y * sq - z * sc),
            1.0 - 2.0 * (x * x + z * z) * sq,
            2.0 * (y * z * sq + x * sc),
            0.0,
        ],
        [
            2.0 * (x * z * sq + y * sc),
            2.0 * (y * z * sq - x * sc),
            1.0 - 2.0 * (x * x + y * y) * sq,
            0.0,
        ],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// ¿Es una matriz 2D (CSS Transforms 2 §12: m13..m34 = 0, m33 = m44 = 1)?
pub fn is_2d(m: &Mat4) -> bool {
    m[0][2] == 0.0
        && m[0][3] == 0.0
        && m[1][2] == 0.0
        && m[1][3] == 0.0
        && m[2][0] == 0.0
        && m[2][1] == 0.0
        && m[2][2] == 1.0
        && m[2][3] == 0.0
        && m[3][2] == 0.0
        && m[3][3] == 1.0
}

/// La parte 2D `[a, b, c, d, e, f]` de una matriz (como `matrix()`).
pub fn to_affine(m: &Mat4) -> [f32; 6] {
    [
        m[0][0] as f32,
        m[0][1] as f32,
        m[1][0] as f32,
        m[1][1] as f32,
        m[3][0] as f32,
        m[3][1] as f32,
    ]
}

/// Serialización de una matriz (CSSOM: "serialize a transform matrix"): `matrix()` si es 2D,
/// si no `matrix3d()`.
pub fn matrix_css(m: &Mat4) -> String {
    let n = |v: f64| fmt_matrix_number(v);
    if is_2d(m) {
        let [a, b, c, d, e, f] = [m[0][0], m[0][1], m[1][0], m[1][1], m[3][0], m[3][1]];
        format!(
            "matrix({}, {}, {}, {}, {}, {})",
            n(a),
            n(b),
            n(c),
            n(d),
            n(e),
            n(f)
        )
    } else {
        let items: Vec<String> = m.iter().flatten().map(|v| n(*v)).collect();
        format!("matrix3d({})", items.join(", "))
    }
}

/// Número de una matriz serializada: hasta 6 decimales significativos, sin `-0`.
fn fmt_matrix_number(v: f64) -> String {
    let r = (v * 1e6).round() / 1e6;
    if r == 0.0 {
        return "0".to_owned();
    }
    let s = format!("{r}");
    if s.contains('e') {
        format!("{r:.6}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    } else {
        s
    }
}

/// Una `<transform-function>` (CSS Transforms 1 §13 y 2 §12), con las longitudes
/// absolutizadas en el valor computado. Los ángulos están en grados.
#[derive(Debug, Clone, PartialEq)]
pub enum TransformFunction {
    Matrix([f32; 6]),
    Matrix3d([f32; 16]),
    /// `translate()`, `translateX()`, `translateY()`, `translate3d()`, `translateZ()`: el
    /// nombre se conserva para serializar el valor especificado.
    Translate {
        name: TranslateName,
        x: LengthPercentage,
        y: LengthPercentage,
        z: LengthPercentage,
    },
    Scale {
        name: ScaleName,
        x: f32,
        y: f32,
        z: f32,
    },
    /// `rotate()`/`rotateZ()` son `(0, 0, 1)`.
    Rotate {
        name: RotateName,
        x: f32,
        y: f32,
        z: f32,
        angle: f32,
    },
    Skew {
        name: SkewName,
        x: f32,
        y: f32,
    },
    /// `perspective()`; `None` es `perspective(none)`.
    Perspective(Option<LengthPercentage>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranslateName {
    Translate,
    X,
    Y,
    Z,
    ThreeD,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleName {
    Scale,
    X,
    Y,
    Z,
    ThreeD,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotateName {
    Rotate,
    X,
    Y,
    Z,
    ThreeD,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkewName {
    Skew,
    X,
    Y,
}

/// `<number> | <percentage>` (escalas de CSS Transforms 2).
fn number_or_percent(v: &ComponentValue) -> Option<f32> {
    if let Some(bf_css::syntax::TokenKind::Percentage(p)) = v.token() {
        return Some(p.value as f32 / 100.0);
    }
    parse_number(v)
}

/// `<angle> | <zero>`; los ángulos de `calc()` no se aceptan todavía.
fn angle(v: &ComponentValue) -> Option<f32> {
    parse_angle(v)
}

fn length(v: &ComponentValue) -> Option<LengthPercentage> {
    parse_length_percentage(v).filter(|lp| !lp.has_percentage())
}

impl TransformFunction {
    /// Parsea una función (`name(args)`).
    pub fn parse(v: &ComponentValue) -> Option<TransformFunction> {
        let ComponentValue::Function(f) = v else {
            return None;
        };
        // Una coma al final deja un argumento vacío (`translate(1px,)` es inválido), pero
        // `split_comma_separated` no lo devuelve.
        if trim(&f.value)
            .last()
            .is_some_and(|v| matches!(v.token(), Some(bf_css::syntax::TokenKind::Comma)))
        {
            return None;
        }
        let args: Vec<Vec<ComponentValue>> = split_comma_separated(&f.value);
        let args: Vec<&[ComponentValue]> = args.iter().map(|a| trim(a)).collect();
        // Cada argumento es un único component value.
        if args.iter().any(|a| a.len() != 1) && !(args.len() == 1 && args[0].is_empty()) {
            return None;
        }
        let a: Vec<&ComponentValue> = args.iter().filter_map(|a| a.first()).collect();
        let zero = LengthPercentage::zero;
        use TransformFunction as T;
        Some(match f.name.to_ascii_lowercase().as_str() {
            "matrix" if a.len() == 6 => {
                let mut m = [0.0; 6];
                for (slot, v) in m.iter_mut().zip(&a) {
                    *slot = parse_number(v)?;
                }
                T::Matrix(m)
            }
            "matrix3d" if a.len() == 16 => {
                let mut m = [0.0; 16];
                for (slot, v) in m.iter_mut().zip(&a) {
                    *slot = parse_number(v)?;
                }
                T::Matrix3d(m)
            }
            "translate" if (1..=2).contains(&a.len()) => T::Translate {
                name: TranslateName::Translate,
                x: parse_length_percentage(a[0])?,
                y: match a.get(1) {
                    Some(v) => parse_length_percentage(v)?,
                    None => zero(),
                },
                z: zero(),
            },
            "translatex" if a.len() == 1 => T::Translate {
                name: TranslateName::X,
                x: parse_length_percentage(a[0])?,
                y: zero(),
                z: zero(),
            },
            "translatey" if a.len() == 1 => T::Translate {
                name: TranslateName::Y,
                x: zero(),
                y: parse_length_percentage(a[0])?,
                z: zero(),
            },
            "translatez" if a.len() == 1 => T::Translate {
                name: TranslateName::Z,
                x: zero(),
                y: zero(),
                z: length(a[0])?,
            },
            "translate3d" if a.len() == 3 => T::Translate {
                name: TranslateName::ThreeD,
                x: parse_length_percentage(a[0])?,
                y: parse_length_percentage(a[1])?,
                z: length(a[2])?,
            },
            "scale" if (1..=2).contains(&a.len()) => {
                let x = number_or_percent(a[0])?;
                T::Scale {
                    name: ScaleName::Scale,
                    x,
                    y: match a.get(1) {
                        Some(v) => number_or_percent(v)?,
                        None => x,
                    },
                    z: 1.0,
                }
            }
            "scalex" if a.len() == 1 => T::Scale {
                name: ScaleName::X,
                x: number_or_percent(a[0])?,
                y: 1.0,
                z: 1.0,
            },
            "scaley" if a.len() == 1 => T::Scale {
                name: ScaleName::Y,
                x: 1.0,
                y: number_or_percent(a[0])?,
                z: 1.0,
            },
            "scalez" if a.len() == 1 => T::Scale {
                name: ScaleName::Z,
                x: 1.0,
                y: 1.0,
                z: number_or_percent(a[0])?,
            },
            "scale3d" if a.len() == 3 => T::Scale {
                name: ScaleName::ThreeD,
                x: number_or_percent(a[0])?,
                y: number_or_percent(a[1])?,
                z: number_or_percent(a[2])?,
            },
            "rotate" | "rotatez" if a.len() == 1 => T::Rotate {
                name: if f.name.eq_ignore_ascii_case("rotate") {
                    RotateName::Rotate
                } else {
                    RotateName::Z
                },
                x: 0.0,
                y: 0.0,
                z: 1.0,
                angle: angle(a[0])?,
            },
            "rotatex" if a.len() == 1 => T::Rotate {
                name: RotateName::X,
                x: 1.0,
                y: 0.0,
                z: 0.0,
                angle: angle(a[0])?,
            },
            "rotatey" if a.len() == 1 => T::Rotate {
                name: RotateName::Y,
                x: 0.0,
                y: 1.0,
                z: 0.0,
                angle: angle(a[0])?,
            },
            "rotate3d" if a.len() == 4 => T::Rotate {
                name: RotateName::ThreeD,
                x: parse_number(a[0])?,
                y: parse_number(a[1])?,
                z: parse_number(a[2])?,
                angle: angle(a[3])?,
            },
            "skew" if (1..=2).contains(&a.len()) => T::Skew {
                name: SkewName::Skew,
                x: angle(a[0])?,
                y: match a.get(1) {
                    Some(v) => angle(v)?,
                    None => 0.0,
                },
            },
            "skewx" if a.len() == 1 => T::Skew {
                name: SkewName::X,
                x: angle(a[0])?,
                y: 0.0,
            },
            "skewy" if a.len() == 1 => T::Skew {
                name: SkewName::Y,
                x: 0.0,
                y: angle(a[0])?,
            },
            "perspective" if a.len() == 1 => {
                if is_ident(a[0], "none") {
                    T::Perspective(None)
                } else {
                    let l = length(a[0])?;
                    if matches!(&l, LengthPercentage::Length(l) if l.value < 0.0) {
                        return None;
                    }
                    T::Perspective(Some(l))
                }
            }
            _ => return None,
        })
    }

    fn compute(&self, ctx: &ComputeContext<'_>) -> TransformFunction {
        match self {
            TransformFunction::Translate { name, x, y, z } => TransformFunction::Translate {
                name: *name,
                x: x.compute(&ctx.len),
                y: y.compute(&ctx.len),
                z: z.compute(&ctx.len),
            },
            TransformFunction::Perspective(Some(l)) => {
                TransformFunction::Perspective(Some(l.compute(&ctx.len)))
            }
            other => other.clone(),
        }
    }

    /// La matriz de la función con la caja de referencia `(w, h)` (para los porcentajes).
    pub fn matrix(&self, w: f32, h: f32) -> Mat4 {
        let f = f64::from;
        match self {
            TransformFunction::Matrix([a, b, c, d, e, g]) => [
                [f(*a), f(*b), 0.0, 0.0],
                [f(*c), f(*d), 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [f(*e), f(*g), 0.0, 1.0],
            ],
            TransformFunction::Matrix3d(m) => {
                let mut r = [[0.0; 4]; 4];
                for (i, v) in m.iter().enumerate() {
                    r[i / 4][i % 4] = f(*v);
                }
                r
            }
            TransformFunction::Translate { x, y, z, .. } => {
                translate_mat(f(x.resolve(w)), f(y.resolve(h)), f(z.resolve(0.0)))
            }
            TransformFunction::Scale { x, y, z, .. } => scale_mat(f(*x), f(*y), f(*z)),
            TransformFunction::Rotate { x, y, z, angle, .. } => {
                rotate_mat(f(*x), f(*y), f(*z), f(*angle))
            }
            TransformFunction::Skew { x, y, .. } => {
                let mut m = IDENTITY;
                m[1][0] = f(*x).to_radians().tan();
                m[0][1] = f(*y).to_radians().tan();
                m
            }
            TransformFunction::Perspective(p) => {
                let mut m = IDENTITY;
                if let Some(p) = p {
                    // CSS Transforms 2 §12: m34 = -1/d (d < 1 se trata como 1).
                    let d = p.resolve(0.0).max(1.0);
                    m[2][3] = -1.0 / f64::from(d);
                }
                m
            }
        }
    }

    fn to_css(&self) -> String {
        let n = |v: f32| fmt_number(v);
        let deg = |v: f32| format!("{}deg", fmt_number(v));
        match self {
            TransformFunction::Matrix(m) => {
                let items: Vec<String> = m.iter().map(|v| n(*v)).collect();
                format!("matrix({})", items.join(", "))
            }
            TransformFunction::Matrix3d(m) => {
                let items: Vec<String> = m.iter().map(|v| n(*v)).collect();
                format!("matrix3d({})", items.join(", "))
            }
            TransformFunction::Translate { name, x, y, z } => match name {
                TranslateName::Translate => {
                    if *y == LengthPercentage::zero() {
                        format!("translate({x})")
                    } else {
                        format!("translate({x}, {y})")
                    }
                }
                TranslateName::X => format!("translateX({x})"),
                TranslateName::Y => format!("translateY({y})"),
                TranslateName::Z => format!("translateZ({z})"),
                TranslateName::ThreeD => format!("translate3d({x}, {y}, {z})"),
            },
            TransformFunction::Scale { name, x, y, z } => match name {
                ScaleName::Scale if x == y => format!("scale({})", n(*x)),
                ScaleName::Scale => format!("scale({}, {})", n(*x), n(*y)),
                ScaleName::X => format!("scaleX({})", n(*x)),
                ScaleName::Y => format!("scaleY({})", n(*y)),
                ScaleName::Z => format!("scaleZ({})", n(*z)),
                ScaleName::ThreeD => format!("scale3d({}, {}, {})", n(*x), n(*y), n(*z)),
            },
            TransformFunction::Rotate {
                name,
                x,
                y,
                z,
                angle,
            } => match name {
                RotateName::Rotate => format!("rotate({})", deg(*angle)),
                RotateName::X => format!("rotateX({})", deg(*angle)),
                RotateName::Y => format!("rotateY({})", deg(*angle)),
                RotateName::Z => format!("rotateZ({})", deg(*angle)),
                RotateName::ThreeD => {
                    format!("rotate3d({}, {}, {}, {})", n(*x), n(*y), n(*z), deg(*angle))
                }
            },
            TransformFunction::Skew { name, x, y } => match name {
                SkewName::Skew if *y == 0.0 => format!("skew({})", deg(*x)),
                SkewName::Skew => format!("skew({}, {})", deg(*x), deg(*y)),
                SkewName::X => format!("skewX({})", deg(*x)),
                SkewName::Y => format!("skewY({})", deg(*y)),
            },
            TransformFunction::Perspective(None) => "perspective(none)".to_owned(),
            TransformFunction::Perspective(Some(l)) => format!("perspective({l})"),
        }
    }
}

/// `transform: none | <transform-list>` (CSS Transforms 1 §6). La lista vacía es `none`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Transform(pub Vec<TransformFunction>);

impl Transform {
    pub fn is_none(&self) -> bool {
        self.0.is_empty()
    }

    /// La matriz de la lista (sin el origen) con la caja de referencia `(w, h)`: las funciones
    /// se aplican de derecha a izquierda (CSS Transforms 1 §6.1).
    pub fn matrix(&self, w: f32, h: f32) -> Mat4 {
        let mut total = IDENTITY;
        for f in &self.0 {
            total = mat_mul(&f.matrix(w, h), &total);
        }
        total
    }
}

impl PropValue for Transform {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let items = components(trim(input));
        if let [v] = items.as_slice()
            && is_ident(v, "none")
        {
            return Some(Transform(Vec::new()));
        }
        if items.is_empty() {
            return None;
        }
        items
            .iter()
            .map(|v| TransformFunction::parse(v))
            .collect::<Option<Vec<_>>>()
            .map(Transform)
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        Transform(self.0.iter().map(|f| f.compute(ctx)).collect())
    }
    fn to_css(&self) -> String {
        if self.0.is_empty() {
            return "none".to_owned();
        }
        let items: Vec<String> = self.0.iter().map(TransformFunction::to_css).collect();
        items.join(" ")
    }
}

/// `transform-origin` (CSS Transforms 1 §7): x, y (relativas a la caja de referencia) y z.
#[derive(Debug, Clone, PartialEq)]
pub struct TransformOrigin {
    pub x: LengthPercentage,
    pub y: LengthPercentage,
    pub z: LengthPercentage,
}

impl TransformOrigin {
    /// El origen en px relativo a la esquina de una caja `(w, h)`.
    pub fn resolve(&self, w: f32, h: f32) -> (f32, f32) {
        (self.x.resolve(w), self.y.resolve(h))
    }
}

impl PropValue for TransformOrigin {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        #[derive(Clone, Copy, PartialEq)]
        enum K {
            Left,
            Right,
            Top,
            Bottom,
            Center,
        }
        let items = components(trim(input));
        let key = |v: &ComponentValue| -> Option<K> {
            Some(match ident(v)?.to_ascii_lowercase().as_str() {
                "left" => K::Left,
                "right" => K::Right,
                "top" => K::Top,
                "bottom" => K::Bottom,
                "center" => K::Center,
                _ => return None,
            })
        };
        let pct = LengthPercentage::Percentage;
        let kx = |k: K| match k {
            K::Left => Some(pct(0.0)),
            K::Right => Some(pct(100.0)),
            K::Center => Some(pct(50.0)),
            _ => None,
        };
        let ky = |k: K| match k {
            K::Top => Some(pct(0.0)),
            K::Bottom => Some(pct(100.0)),
            K::Center => Some(pct(50.0)),
            _ => None,
        };
        let z = |v: Option<&&ComponentValue>| -> Option<LengthPercentage> {
            match v {
                None => Some(LengthPercentage::zero()),
                Some(v) => parse_length(v).map(LengthPercentage::Length),
            }
        };
        let (x, y, rest) = match items.as_slice() {
            [] => return None,
            [a] => match key(a) {
                Some(k @ (K::Top | K::Bottom)) => (pct(50.0), ky(k)?, None),
                Some(k) => (kx(k)?, pct(50.0), None),
                None => (parse_length_percentage(a)?, pct(50.0), None),
            },
            [a, b, rest @ ..] if rest.len() <= 1 => {
                let (ka, kb) = (key(a), key(b));
                let (x, y) = match (ka, kb) {
                    // Dos palabras clave en cualquier orden.
                    (Some(ka), Some(kb)) => {
                        if let (Some(x), Some(y)) = (kx(ka), ky(kb)) {
                            (x, y)
                        } else if let (Some(x), Some(y)) = (kx(kb), ky(ka)) {
                            (x, y)
                        } else {
                            return None;
                        }
                    }
                    (Some(ka), None) => (kx(ka)?, parse_length_percentage(b)?),
                    (None, Some(kb)) => (parse_length_percentage(a)?, ky(kb)?),
                    (None, None) => (parse_length_percentage(a)?, parse_length_percentage(b)?),
                };
                (x, y, rest.first())
            }
            _ => return None,
        };
        Some(TransformOrigin { x, y, z: z(rest)? })
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        TransformOrigin {
            x: self.x.compute(&ctx.len),
            y: self.y.compute(&ctx.len),
            z: self.z.compute(&ctx.len),
        }
    }
    fn to_css(&self) -> String {
        let mut s = format!("{} {}", self.x, self.y);
        if self.z != LengthPercentage::zero() {
            let _ = write!(s, " {}", self.z);
        }
        s
    }
}

/// `translate: none | <length-percentage> [<length-percentage> <length>?]?` (CSS Transforms
/// 2 §5.1).
#[derive(Debug, Clone, PartialEq)]
pub struct Translate(pub Option<(LengthPercentage, LengthPercentage, LengthPercentage)>);

impl PropValue for Translate {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let items = components(trim(input));
        let zero = LengthPercentage::zero;
        Some(Translate(Some(match items.as_slice() {
            [v] if is_ident(v, "none") => return Some(Translate(None)),
            [x] => (parse_length_percentage(x)?, zero(), zero()),
            [x, y] => (
                parse_length_percentage(x)?,
                parse_length_percentage(y)?,
                zero(),
            ),
            [x, y, z] => (
                parse_length_percentage(x)?,
                parse_length_percentage(y)?,
                length(z)?,
            ),
            _ => return None,
        })))
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        Translate(self.0.as_ref().map(|(x, y, z)| {
            (
                x.compute(&ctx.len),
                y.compute(&ctx.len),
                z.compute(&ctx.len),
            )
        }))
    }
    fn to_css(&self) -> String {
        match &self.0 {
            None => "none".to_owned(),
            Some((x, y, z)) => {
                let zero = LengthPercentage::zero();
                if *z != zero {
                    format!("{x} {y} {z}")
                } else if *y != zero {
                    format!("{x} {y}")
                } else {
                    x.to_string()
                }
            }
        }
    }
}

/// `rotate: none | <angle> | [x | y | z | <number>{3}] && <angle>` (CSS Transforms 2 §5.2):
/// eje y ángulo en grados.
#[derive(Debug, Clone, PartialEq)]
pub struct Rotate(pub Option<([f32; 3], f32)>);

impl PropValue for Rotate {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let items = components(trim(input));
        if let [v] = items.as_slice()
            && is_ident(v, "none")
        {
            return Some(Rotate(None));
        }
        let axis_key = |v: &ComponentValue| -> Option<[f32; 3]> {
            Some(match ident(v)?.to_ascii_lowercase().as_str() {
                "x" => [1.0, 0.0, 0.0],
                "y" => [0.0, 1.0, 0.0],
                "z" => [0.0, 0.0, 1.0],
                _ => return None,
            })
        };
        let (angle_at, axis) = match items.as_slice() {
            [_] => (0, [0.0, 0.0, 1.0]),
            [a, b] => match (axis_key(a), axis_key(b)) {
                (Some(ax), None) => (1, ax),
                (None, Some(ax)) => (0, ax),
                _ => return None,
            },
            [a, b, c, d] => {
                if let (Some(x), Some(y), Some(z)) =
                    (parse_number(a), parse_number(b), parse_number(c))
                {
                    (3, [x, y, z])
                } else if let (Some(x), Some(y), Some(z)) =
                    (parse_number(b), parse_number(c), parse_number(d))
                {
                    (0, [x, y, z])
                } else {
                    return None;
                }
            }
            _ => return None,
        };
        Some(Rotate(Some((axis, angle(items[angle_at])?))))
    }
    fn to_css(&self) -> String {
        match &self.0 {
            None => "none".to_owned(),
            Some((axis, a)) => {
                let deg = format!("{}deg", fmt_number(*a));
                match axis {
                    [0.0, 0.0, 1.0] => deg,
                    [1.0, 0.0, 0.0] => format!("x {deg}"),
                    [0.0, 1.0, 0.0] => format!("y {deg}"),
                    [x, y, z] => format!(
                        "{} {} {} {deg}",
                        fmt_number(*x),
                        fmt_number(*y),
                        fmt_number(*z)
                    ),
                }
            }
        }
    }
}

/// `scale: none | [<number> | <percentage>]{1,3}` (CSS Transforms 2 §5.3).
#[derive(Debug, Clone, PartialEq)]
pub struct Scale(pub Option<[f32; 3]>);

impl PropValue for Scale {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let items = components(trim(input));
        Some(Scale(Some(match items.as_slice() {
            [v] if is_ident(v, "none") => return Some(Scale(None)),
            [x] => {
                let x = number_or_percent(x)?;
                [x, x, 1.0]
            }
            [x, y] => [number_or_percent(x)?, number_or_percent(y)?, 1.0],
            [x, y, z] => [
                number_or_percent(x)?,
                number_or_percent(y)?,
                number_or_percent(z)?,
            ],
            _ => return None,
        })))
    }
    fn to_css(&self) -> String {
        match &self.0 {
            None => "none".to_owned(),
            Some([x, y, z]) => {
                if *z != 1.0 {
                    format!("{} {} {}", fmt_number(*x), fmt_number(*y), fmt_number(*z))
                } else if x != y {
                    format!("{} {}", fmt_number(*x), fmt_number(*y))
                } else {
                    fmt_number(*x)
                }
            }
        }
    }
}

/// La matriz de transformación completa de una caja (CSS Transforms 2 §6.1, "current
/// transformation matrix"): `translate`, `rotate`, `scale` y `transform` alrededor de
/// `transform-origin`, con la caja de referencia `rect` = `(x, y, w, h)` en coordenadas de
/// página. `None` si el elemento no se transforma.
pub fn box_matrix(style: &super::ComputedStyle, rect: (f32, f32, f32, f32)) -> Option<Mat4> {
    if style.transform.is_none()
        && style.translate.0.is_none()
        && style.rotate.0.is_none()
        && style.scale.0.is_none()
    {
        return None;
    }
    let (x, y, w, h) = rect;
    let (ox, oy) = style.transform_origin.resolve(w, h);
    let oz = style.transform_origin.z.resolve(0.0);
    let (ox, oy, oz) = (f64::from(x + ox), f64::from(y + oy), f64::from(oz));
    // Con vectores fila: T(-origen) · transform · scale · rotate · translate · T(origen).
    let mut m = translate_mat(-ox, -oy, -oz);
    m = mat_mul(&m, &style.transform.matrix(w, h));
    if let Some([sx, sy, sz]) = style.scale.0 {
        m = mat_mul(&m, &scale_mat(f64::from(sx), f64::from(sy), f64::from(sz)));
    }
    if let Some(([ax, ay, az], a)) = style.rotate.0 {
        let r = rotate_mat(f64::from(ax), f64::from(ay), f64::from(az), f64::from(a));
        m = mat_mul(&m, &r);
    }
    if let Some((tx, ty, tz)) = &style.translate.0 {
        let t = translate_mat(
            f64::from(tx.resolve(w)),
            f64::from(ty.resolve(h)),
            f64::from(tz.resolve(0.0)),
        );
        m = mat_mul(&m, &t);
    }
    Some(mat_mul(&m, &translate_mat(ox, oy, oz)))
}

/// La inversa de una transformación afín `[a, b, c, d, e, f]` (para el hit testing).
pub fn invert_affine(m: [f32; 6]) -> Option<[f32; 6]> {
    let [a, b, c, d, e, f] = m.map(f64::from);
    let det = a * d - b * c;
    if det.abs() < 1e-12 {
        return None;
    }
    let ia = d / det;
    let ib = -b / det;
    let ic = -c / det;
    let id = a / det;
    let ie = -(e * ia + f * ic);
    let if_ = -(e * ib + f * id);
    Some([ia, ib, ic, id, ie, if_].map(|v| v as f32))
}

/// Aplica `[a, b, c, d, e, f]` al punto `(x, y)`.
pub fn apply_affine(m: [f32; 6], (x, y): (f32, f32)) -> (f32, f32) {
    let [a, b, c, d, e, f] = m;
    (a * x + c * y + e, b * x + d * y + f)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(css: &str) -> Option<Transform> {
        let tokens = bf_css::syntax::parse_component_value_list(css);
        <Transform as PropValue>::parse(&tokens)
    }

    #[test]
    fn parses_and_composes() {
        let t = parse("translate(10px, 20px) scale(2)").unwrap();
        assert_eq!(t.to_css(), "translate(10px, 20px) scale(2)");
        assert_eq!(
            matrix_css(&t.matrix(0.0, 0.0)),
            "matrix(2, 0, 0, 2, 10, 20)"
        );
        let r = parse("rotate(90deg)").unwrap();
        assert_eq!(matrix_css(&r.matrix(0.0, 0.0)), "matrix(0, 1, -1, 0, 0, 0)");
        let p = parse("translateX(50%)").unwrap();
        assert_eq!(to_affine(&p.matrix(200.0, 0.0))[4], 100.0);
        assert!(parse("none").unwrap().is_none());
        assert!(parse("translate(1px,)").is_none());
        assert!(parse("rotate(10px)").is_none());
        assert!(parse("").is_none());
        let s = parse("skewX(45deg)").unwrap();
        assert_eq!(matrix_css(&s.matrix(0.0, 0.0)), "matrix(1, 0, 1, 1, 0, 0)");
        let x = parse("rotateX(0deg) translateZ(5px)").unwrap();
        assert_eq!(
            matrix_css(&x.matrix(0.0, 0.0)),
            "matrix3d(1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 5, 1)"
        );
    }

    #[test]
    fn origin_and_inverse() {
        let tokens = bf_css::syntax::parse_component_value_list("right top");
        let o = <TransformOrigin as PropValue>::parse(&tokens).unwrap();
        assert_eq!(o.to_css(), "100% 0%");
        let tokens = bf_css::syntax::parse_component_value_list("top left 3px");
        let o = <TransformOrigin as PropValue>::parse(&tokens).unwrap();
        assert_eq!(o.to_css(), "0% 0% 3px");
        let m = [2.0, 0.0, 0.0, 4.0, 10.0, 20.0];
        let inv = invert_affine(m).unwrap();
        assert_eq!(apply_affine(inv, apply_affine(m, (3.0, 5.0))), (3.0, 5.0));
    }
}
