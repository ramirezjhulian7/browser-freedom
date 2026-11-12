//! E7: efectos visuales de CSS sobre cajas: `border-image-*` (CSS Backgrounds 3 §6), `filter`
//! (Filter Effects 1 §5 y §8), `clip-path` con formas básicas (CSS Masking 1 §5, CSS Shapes 1
//! §3), `mix-blend-mode` e `isolation` (Compositing and Blending 1 §3).

use bf_css::syntax::{ComponentValue, TokenKind, split_comma_separated};

use super::types::{PositionAxis, parse_bg_position};
use super::{ComputeContext, PropValue};
use crate::values::color::parse_color;
use crate::values::{
    Color, LengthPercentage, components, fmt_number, ident, is_ident, parse_angle,
    parse_length_percentage, parse_non_negative_lp, parse_url, single_ident,
};

/// Repite 1–4 valores con la regla de la caja (arriba, derecha, abajo, izquierda).
fn box4<T: Clone>(v: Vec<T>) -> Option<[T; 4]> {
    Some(match v.as_slice() {
        [a] => [a.clone(), a.clone(), a.clone(), a.clone()],
        [a, b] => [a.clone(), b.clone(), a.clone(), b.clone()],
        [a, b, c] => [a.clone(), b.clone(), c.clone(), b.clone()],
        [a, b, c, d] => [a.clone(), b.clone(), c.clone(), d.clone()],
        _ => return None,
    })
}

/// Serializa cuatro lados con la forma más corta.
fn box4_css<T: PartialEq>(v: &[T; 4], f: impl Fn(&T) -> String) -> String {
    let [t, r, b, l] = v;
    let n = if l != r {
        4
    } else if b != t {
        3
    } else if r != t {
        2
    } else {
        1
    };
    v[..n].iter().map(f).collect::<Vec<_>>().join(" ")
}

/// `<number [0,∞]>` sin `calc()`.
fn non_negative_number(v: &ComponentValue) -> Option<f32> {
    match v.token()? {
        TokenKind::Number(n) if n.value >= 0.0 => Some(n.value as f32),
        _ => None,
    }
}

/// `<number> | <percentage>` (los porcentajes como fracción); `None` si es negativo.
fn number_or_fraction(v: &ComponentValue) -> Option<f32> {
    match v.token()? {
        TokenKind::Number(n) if n.value >= 0.0 => Some(n.value as f32),
        TokenKind::Percentage(p) if p.value >= 0.0 => Some(p.value as f32 / 100.0),
        _ => None,
    }
}

// --- border-image ---

/// Un valor de `border-image-slice`: número (px de la imagen) o porcentaje de su tamaño.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SliceValue {
    Number(f32),
    Percentage(f32),
}

impl SliceValue {
    /// En px de la imagen de tamaño `size` en ese eje.
    pub fn resolve(&self, size: f32) -> f32 {
        match *self {
            SliceValue::Number(n) => n,
            SliceValue::Percentage(p) => p / 100.0 * size,
        }
    }

    fn to_css(&self) -> String {
        match *self {
            SliceValue::Number(n) => fmt_number(n),
            SliceValue::Percentage(p) => format!("{}%", fmt_number(p)),
        }
    }
}

/// `border-image-slice: [<number [0,∞]> | <percentage [0,∞]>]{1,4} && fill?` (§6.3).
#[derive(Debug, Clone, PartialEq)]
pub struct BorderImageSlice {
    pub sides: [SliceValue; 4],
    pub fill: bool,
}

impl BorderImageSlice {
    fn parse_items(items: &[&ComponentValue]) -> Option<Self> {
        let mut fill = false;
        let mut values = Vec::new();
        for (i, v) in items.iter().enumerate() {
            if is_ident(v, "fill") && !fill && (i == 0 || i == items.len() - 1) {
                fill = true;
                continue;
            }
            values.push(match v.token()? {
                TokenKind::Number(n) if n.value >= 0.0 => SliceValue::Number(n.value as f32),
                TokenKind::Percentage(p) if p.value >= 0.0 => {
                    SliceValue::Percentage(p.value as f32)
                }
                _ => return None,
            });
        }
        Some(BorderImageSlice {
            sides: box4(values)?,
            fill,
        })
    }
}

impl PropValue for BorderImageSlice {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        Self::parse_items(&components(input))
    }
    fn to_css(&self) -> String {
        let s = box4_css(&self.sides, SliceValue::to_css);
        if self.fill { format!("{s} fill") } else { s }
    }
}

/// Un valor de `border-image-width` (§6.4) o de `border-image-outset` (§6.5): longitud (o
/// porcentaje del área, solo en el ancho), múltiplo del ancho del borde, o `auto`.
#[derive(Debug, Clone, PartialEq)]
pub enum BorderImageLength {
    Length(LengthPercentage),
    Number(f32),
    Auto,
}

impl BorderImageLength {
    fn parse_one(v: &ComponentValue, outset: bool) -> Option<Self> {
        if let Some(n) = non_negative_number(v) {
            return Some(BorderImageLength::Number(n));
        }
        if !outset && is_ident(v, "auto") {
            return Some(BorderImageLength::Auto);
        }
        let l = parse_non_negative_lp(v)?;
        if outset && l.has_percentage() {
            return None;
        }
        Some(BorderImageLength::Length(l))
    }

    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        match self {
            BorderImageLength::Length(l) => BorderImageLength::Length(l.compute(&ctx.len)),
            other => other.clone(),
        }
    }

    fn to_css(&self) -> String {
        match self {
            BorderImageLength::Length(l) => l.to_string(),
            BorderImageLength::Number(n) => fmt_number(*n),
            BorderImageLength::Auto => "auto".into(),
        }
    }

    fn parse_list(items: &[&ComponentValue], outset: bool) -> Option<[Self; 4]> {
        box4(
            items
                .iter()
                .map(|v| Self::parse_one(v, outset))
                .collect::<Option<Vec<_>>>()?,
        )
    }
}

/// `border-image-width: [<length-percentage [0,∞]> | <number [0,∞]> | auto]{1,4}`.
#[derive(Debug, Clone, PartialEq)]
pub struct BorderImageWidth(pub [BorderImageLength; 4]);

impl PropValue for BorderImageWidth {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        BorderImageLength::parse_list(&components(input), false).map(BorderImageWidth)
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        BorderImageWidth(self.0.clone().map(|v| v.compute(ctx)))
    }
    fn to_css(&self) -> String {
        box4_css(&self.0, BorderImageLength::to_css)
    }
}

/// `border-image-outset: [<length [0,∞]> | <number [0,∞]>]{1,4}`.
#[derive(Debug, Clone, PartialEq)]
pub struct BorderImageOutset(pub [BorderImageLength; 4]);

impl PropValue for BorderImageOutset {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        BorderImageLength::parse_list(&components(input), true).map(BorderImageOutset)
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        BorderImageOutset(self.0.clone().map(|v| v.compute(ctx)))
    }
    fn to_css(&self) -> String {
        box4_css(&self.0, BorderImageLength::to_css)
    }
}

/// Cómo se reparte la imagen en un lado (§6.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageRepeat {
    Stretch,
    Repeat,
    Round,
    Space,
}

impl ImageRepeat {
    fn from_ident(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "stretch" => ImageRepeat::Stretch,
            "repeat" => ImageRepeat::Repeat,
            "round" => ImageRepeat::Round,
            "space" => ImageRepeat::Space,
            _ => return None,
        })
    }

    fn as_str(self) -> &'static str {
        match self {
            ImageRepeat::Stretch => "stretch",
            ImageRepeat::Repeat => "repeat",
            ImageRepeat::Round => "round",
            ImageRepeat::Space => "space",
        }
    }
}

/// `border-image-repeat: [stretch | repeat | round | space]{1,2}`: horizontal y vertical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BorderImageRepeat(pub ImageRepeat, pub ImageRepeat);

impl BorderImageRepeat {
    fn parse_items(items: &[&ComponentValue]) -> Option<Self> {
        let get = |v: &ComponentValue| ident(v).and_then(ImageRepeat::from_ident);
        match items {
            [a] => {
                let a = get(a)?;
                Some(BorderImageRepeat(a, a))
            }
            [a, b] => Some(BorderImageRepeat(get(a)?, get(b)?)),
            _ => None,
        }
    }
}

impl PropValue for BorderImageRepeat {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        Self::parse_items(&components(input))
    }
    fn to_css(&self) -> String {
        if self.0 == self.1 {
            self.0.as_str().into()
        } else {
            format!("{} {}", self.0.as_str(), self.1.as_str())
        }
    }
}

/// Partes del shorthand `border-image` (§6.7): `<'border-image-source'> ||
/// <'border-image-slice'> [ / <'border-image-width'> | / <'border-image-width'>? /
/// <'border-image-outset'> ]? || <'border-image-repeat'>`.
pub struct BorderImageParts {
    pub source: Option<super::types::Image>,
    pub slice: Option<BorderImageSlice>,
    pub width: Option<BorderImageWidth>,
    pub outset: Option<BorderImageOutset>,
    pub repeat: Option<BorderImageRepeat>,
}

/// Parsea el shorthand `border-image`.
pub fn parse_border_image(input: &[ComponentValue]) -> Option<BorderImageParts> {
    let items = components(input);
    let mut parts = BorderImageParts {
        source: None,
        slice: None,
        width: None,
        outset: None,
        repeat: None,
    };
    if let [v] = items.as_slice()
        && is_ident(v, "none")
    {
        parts.source = Some(super::types::Image::None);
        return Some(parts);
    }
    let is_slash = |v: &&ComponentValue| v.token() == Some(&TokenKind::Delim('/'));
    let slice_like = |v: &ComponentValue| is_ident(v, "fill") || number_or_fraction(v).is_some();
    let mut i = 0;
    while i < items.len() {
        let v = items[i];
        // Fuente.
        if parts.source.is_none()
            && let Some(img) = <super::types::Image as PropValue>::parse(std::slice::from_ref(v))
                .filter(|im| !matches!(im, super::types::Image::None) || is_ident(v, "none"))
        {
            parts.source = Some(img);
            i += 1;
            continue;
        }
        // Repetición.
        if parts.repeat.is_none() && ident(v).and_then(ImageRepeat::from_ident).is_some() {
            let mut n = 1;
            if items
                .get(i + 1)
                .and_then(|w| ident(w))
                .and_then(ImageRepeat::from_ident)
                .is_some()
            {
                n = 2;
            }
            parts.repeat = Some(BorderImageRepeat::parse_items(&items[i..i + n])?);
            i += n;
            continue;
        }
        // Corte, con `/ ancho` y `/ ancho? / desplazamiento` detrás.
        if parts.slice.is_none() && slice_like(v) {
            let start = i;
            while i < items.len() && slice_like(items[i]) {
                i += 1;
            }
            parts.slice = Some(BorderImageSlice::parse_items(&items[start..i])?);
            if i < items.len() && is_slash(&items[i]) {
                i += 1;
                let ws = i;
                while i < items.len() && !is_slash(&items[i]) && is_width_item(items[i]) {
                    i += 1;
                }
                if i > ws {
                    parts.width = Some(BorderImageWidth(BorderImageLength::parse_list(
                        &items[ws..i],
                        false,
                    )?));
                }
                if i < items.len() && is_slash(&items[i]) {
                    i += 1;
                    let os = i;
                    while i < items.len() && is_width_item(items[i]) && !is_ident(items[i], "auto")
                    {
                        i += 1;
                    }
                    parts.outset = Some(BorderImageOutset(BorderImageLength::parse_list(
                        &items[os..i],
                        true,
                    )?));
                } else if i == ws {
                    // `/` sin ancho ni desplazamiento.
                    return None;
                }
            }
            continue;
        }
        return None;
    }
    Some(parts)
}

fn is_width_item(v: &ComponentValue) -> bool {
    is_ident(v, "auto") || non_negative_number(v).is_some() || parse_length_percentage(v).is_some()
}

// --- filter ---

/// Una función de filtro (Filter Effects 1 §8). Las cantidades son las especificadas (sin
/// limitar a 1 las que se limitan al usarlas).
#[derive(Debug, Clone, PartialEq)]
pub enum FilterFunction {
    Blur(LengthPercentage),
    Brightness(f32),
    Contrast(f32),
    Grayscale(f32),
    /// En grados.
    HueRotate(f32),
    Invert(f32),
    Opacity(f32),
    Saturate(f32),
    Sepia(f32),
    DropShadow {
        color: Option<Color>,
        x: LengthPercentage,
        y: LengthPercentage,
        blur: LengthPercentage,
    },
    /// Referencia a un elemento `filter` de SVG (no soportada: el filtro no se aplica).
    Url(String),
}

impl FilterFunction {
    fn parse(v: &ComponentValue) -> Option<Self> {
        if let Some(u) = parse_url(v) {
            return Some(FilterFunction::Url(u));
        }
        let ComponentValue::Function(f) = v else {
            return None;
        };
        let args = components(&f.value);
        let name = f.name.to_ascii_lowercase();
        // `<number> | <percentage>` opcional, con su valor por omisión.
        let amount = |default: f32| -> Option<f32> {
            match args.as_slice() {
                [] => Some(default),
                [a] => number_or_fraction(a),
                _ => None,
            }
        };
        Some(match name.as_str() {
            "blur" => FilterFunction::Blur(match args.as_slice() {
                [] => LengthPercentage::zero(),
                [a] => parse_non_negative_lp(a).filter(|l| !l.has_percentage())?,
                _ => return None,
            }),
            "brightness" => FilterFunction::Brightness(amount(1.0)?),
            "contrast" => FilterFunction::Contrast(amount(1.0)?),
            "grayscale" => FilterFunction::Grayscale(amount(1.0)?),
            "invert" => FilterFunction::Invert(amount(1.0)?),
            "opacity" => FilterFunction::Opacity(amount(1.0)?),
            "saturate" => FilterFunction::Saturate(amount(1.0)?),
            "sepia" => FilterFunction::Sepia(amount(1.0)?),
            "hue-rotate" => FilterFunction::HueRotate(match args.as_slice() {
                [] => 0.0,
                [a] => parse_angle(a)?,
                _ => return None,
            }),
            "drop-shadow" => {
                // `<color>? && <length>{2,3}`: las longitudes van juntas.
                let mut color = None;
                let mut lengths = Vec::new();
                let mut closed = false;
                for a in &args {
                    if let Some(l) = parse_length_percentage(a).filter(|l| !l.has_percentage()) {
                        if closed {
                            return None;
                        }
                        lengths.push(l);
                    } else if color.is_none()
                        && let Some(c) = parse_color(a)
                    {
                        closed = !lengths.is_empty();
                        color = Some(c);
                    } else {
                        return None;
                    }
                }
                let mut l = lengths.into_iter();
                let (x, y) = (l.next()?, l.next()?);
                let blur = l.next().unwrap_or_else(LengthPercentage::zero);
                if l.next().is_some()
                    || matches!(&blur, LengthPercentage::Length(b) if b.value < 0.0)
                {
                    return None;
                }
                FilterFunction::DropShadow { color, x, y, blur }
            }
            _ => return None,
        })
    }

    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        match self {
            FilterFunction::Blur(l) => FilterFunction::Blur(l.compute(&ctx.len)),
            FilterFunction::DropShadow { color, x, y, blur } => FilterFunction::DropShadow {
                color: *color,
                x: x.compute(&ctx.len),
                y: y.compute(&ctx.len),
                blur: blur.compute(&ctx.len),
            },
            other => other.clone(),
        }
    }

    fn to_css(&self) -> String {
        let n = |name: &str, v: f32| format!("{name}({})", fmt_number(v));
        match self {
            FilterFunction::Blur(l) => format!("blur({l})"),
            FilterFunction::Brightness(v) => n("brightness", *v),
            FilterFunction::Contrast(v) => n("contrast", *v),
            FilterFunction::Grayscale(v) => n("grayscale", *v),
            FilterFunction::HueRotate(v) => format!("hue-rotate({}deg)", fmt_number(*v)),
            FilterFunction::Invert(v) => n("invert", *v),
            FilterFunction::Opacity(v) => n("opacity", *v),
            FilterFunction::Saturate(v) => n("saturate", *v),
            FilterFunction::Sepia(v) => n("sepia", *v),
            FilterFunction::DropShadow { color, x, y, blur } => match color {
                Some(c) => format!("drop-shadow({c} {x} {y} {blur})"),
                None => format!("drop-shadow({x} {y} {blur})"),
            },
            FilterFunction::Url(u) => format!("url({u:?})"),
        }
    }
}

/// `filter: none | <filter-value-list>` (Filter Effects 1 §5).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Filter(pub Vec<FilterFunction>);

impl PropValue for Filter {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        if single_ident(input).is_some_and(|i| i.eq_ignore_ascii_case("none")) {
            return Some(Filter(Vec::new()));
        }
        let list = components(input)
            .into_iter()
            .map(FilterFunction::parse)
            .collect::<Option<Vec<_>>>()?;
        (!list.is_empty()).then_some(Filter(list))
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        Filter(self.0.iter().map(|f| f.compute(ctx)).collect())
    }
    fn to_css(&self) -> String {
        if self.0.is_empty() {
            return "none".into();
        }
        self.0
            .iter()
            .map(FilterFunction::to_css)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

// --- clip-path ---

/// `<geometry-box>` (CSS Masking 1 §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeometryBox {
    MarginBox,
    BorderBox,
    PaddingBox,
    ContentBox,
    FillBox,
    StrokeBox,
    ViewBox,
}

impl GeometryBox {
    fn from_ident(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "margin-box" => GeometryBox::MarginBox,
            "border-box" => GeometryBox::BorderBox,
            "padding-box" => GeometryBox::PaddingBox,
            "content-box" => GeometryBox::ContentBox,
            "fill-box" => GeometryBox::FillBox,
            "stroke-box" => GeometryBox::StrokeBox,
            "view-box" => GeometryBox::ViewBox,
            _ => return None,
        })
    }

    fn as_str(self) -> &'static str {
        match self {
            GeometryBox::MarginBox => "margin-box",
            GeometryBox::BorderBox => "border-box",
            GeometryBox::PaddingBox => "padding-box",
            GeometryBox::ContentBox => "content-box",
            GeometryBox::FillBox => "fill-box",
            GeometryBox::StrokeBox => "stroke-box",
            GeometryBox::ViewBox => "view-box",
        }
    }
}

/// `<shape-radius>` (CSS Shapes 1 §3.2.1).
#[derive(Debug, Clone, PartialEq)]
pub enum ShapeRadius {
    Length(LengthPercentage),
    ClosestSide,
    FarthestSide,
}

impl ShapeRadius {
    fn parse(v: &ComponentValue) -> Option<Self> {
        if is_ident(v, "closest-side") {
            return Some(ShapeRadius::ClosestSide);
        }
        if is_ident(v, "farthest-side") {
            return Some(ShapeRadius::FarthestSide);
        }
        parse_non_negative_lp(v).map(ShapeRadius::Length)
    }

    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        match self {
            ShapeRadius::Length(l) => ShapeRadius::Length(l.compute(&ctx.len)),
            other => other.clone(),
        }
    }

    fn to_css(&self) -> String {
        match self {
            ShapeRadius::Length(l) => l.to_string(),
            ShapeRadius::ClosestSide => "closest-side".into(),
            ShapeRadius::FarthestSide => "farthest-side".into(),
        }
    }
}

/// Posición de `circle()`/`ellipse()`: la del centro de la caja si se omite.
pub type ShapePosition = (PositionAxis, PositionAxis);

/// `<basic-shape>` (CSS Shapes 1 §3.2), con las longitudes computadas.
#[derive(Debug, Clone, PartialEq)]
pub enum BasicShape {
    /// `inset()`: desplazamientos (arriba, derecha, abajo, izquierda) y radios de las esquinas
    /// (horizontal, vertical) en el orden de `border-radius`.
    Inset {
        offsets: [LengthPercentage; 4],
        radii: [(LengthPercentage, LengthPercentage); 4],
    },
    Circle {
        radius: ShapeRadius,
        position: Option<ShapePosition>,
    },
    Ellipse {
        rx: ShapeRadius,
        ry: ShapeRadius,
        position: Option<ShapePosition>,
    },
    Polygon {
        evenodd: bool,
        points: Vec<(LengthPercentage, LengthPercentage)>,
    },
    /// `path()`: datos de trazado de SVG, en px de la caja de referencia.
    Path { evenodd: bool, d: String },
}

/// `at <position>` al final de los argumentos de `circle()`/`ellipse()`.
fn shape_position(rest: &[&ComponentValue]) -> Option<Option<ShapePosition>> {
    match rest {
        [] => Some(None),
        [at, pos @ ..] if is_ident(at, "at") && !pos.is_empty() => {
            let run: Vec<ComponentValue> = pos.iter().map(|v| (*v).clone()).collect();
            Some(Some(parse_bg_position(&run)?))
        }
        _ => None,
    }
}

/// `<'border-radius'>` de `inset(… round …)`.
fn parse_round(items: &[&ComponentValue]) -> Option<[(LengthPercentage, LengthPercentage); 4]> {
    let slash = items
        .iter()
        .position(|v| v.token() == Some(&TokenKind::Delim('/')));
    let (h, v) = match slash {
        Some(i) => (&items[..i], &items[i + 1..]),
        None => (items, items),
    };
    let list = |vals: &[&ComponentValue]| {
        box4(
            vals.iter()
                .map(|v| parse_non_negative_lp(v))
                .collect::<Option<Vec<_>>>()?,
        )
    };
    let (h, v) = (list(h)?, list(v)?);
    let [h0, h1, h2, h3] = h;
    let [v0, v1, v2, v3] = v;
    Some([(h0, v0), (h1, v1), (h2, v2), (h3, v3)])
}

fn fill_rule(v: &ComponentValue) -> Option<bool> {
    if is_ident(v, "evenodd") {
        Some(true)
    } else if is_ident(v, "nonzero") {
        Some(false)
    } else {
        None
    }
}

impl BasicShape {
    fn parse(v: &ComponentValue) -> Option<Self> {
        let ComponentValue::Function(f) = v else {
            return None;
        };
        let args = components(&f.value);
        match f.name.to_ascii_lowercase().as_str() {
            "inset" => {
                let round = args.iter().position(|v| is_ident(v, "round"));
                let (offs, radii) = match round {
                    Some(i) => (&args[..i], Some(parse_round(&args[i + 1..])?)),
                    None => (&args[..], None),
                };
                let offsets = box4(
                    offs.iter()
                        .map(|v| parse_length_percentage(v))
                        .collect::<Option<Vec<_>>>()?,
                )?;
                let zero = || (LengthPercentage::zero(), LengthPercentage::zero());
                Some(BasicShape::Inset {
                    offsets,
                    radii: radii.unwrap_or_else(|| [zero(), zero(), zero(), zero()]),
                })
            }
            "circle" => {
                let (radius, rest) = match args.first() {
                    Some(r) if !is_ident(r, "at") => (ShapeRadius::parse(r)?, &args[1..]),
                    _ => (ShapeRadius::ClosestSide, &args[..]),
                };
                Some(BasicShape::Circle {
                    radius,
                    position: shape_position(rest)?,
                })
            }
            "ellipse" => {
                let (rx, ry, rest) = match args.as_slice() {
                    [a, b, rest @ ..] if !is_ident(a, "at") => {
                        (ShapeRadius::parse(a)?, ShapeRadius::parse(b)?, rest)
                    }
                    _ => (
                        ShapeRadius::ClosestSide,
                        ShapeRadius::ClosestSide,
                        &args[..],
                    ),
                };
                Some(BasicShape::Ellipse {
                    rx,
                    ry,
                    position: shape_position(rest)?,
                })
            }
            "polygon" => {
                let groups = split_comma_separated(&f.value);
                let mut evenodd = false;
                let mut points = Vec::new();
                for (i, g) in groups.iter().enumerate() {
                    let items = components(g);
                    if i == 0
                        && let [v] = items.as_slice()
                        && let Some(r) = fill_rule(v)
                    {
                        evenodd = r;
                        continue;
                    }
                    match items.as_slice() {
                        [x, y] => {
                            points.push((parse_length_percentage(x)?, parse_length_percentage(y)?))
                        }
                        _ => return None,
                    }
                }
                (!points.is_empty()).then_some(BasicShape::Polygon { evenodd, points })
            }
            "path" => {
                let groups = split_comma_separated(&f.value);
                let (evenodd, d) = match groups.as_slice() {
                    [d] => (false, d),
                    [r, d] => (fill_rule(components(r).first()?)?, d),
                    _ => return None,
                };
                match components(d).as_slice() {
                    [s] => match s.token()? {
                        TokenKind::String(s) => Some(BasicShape::Path {
                            evenodd,
                            d: s.clone(),
                        }),
                        _ => None,
                    },
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        let lp = |l: &LengthPercentage| l.compute(&ctx.len);
        let pos = |p: &Option<ShapePosition>| {
            p.as_ref().map(|(x, y)| {
                (
                    PositionAxis {
                        from_end: x.from_end,
                        offset: lp(&x.offset),
                    },
                    PositionAxis {
                        from_end: y.from_end,
                        offset: lp(&y.offset),
                    },
                )
            })
        };
        match self {
            BasicShape::Inset { offsets, radii } => BasicShape::Inset {
                offsets: offsets.clone().map(|o| lp(&o)),
                radii: radii.clone().map(|(h, v)| (lp(&h), lp(&v))),
            },
            BasicShape::Circle { radius, position } => BasicShape::Circle {
                radius: radius.compute(ctx),
                position: pos(position),
            },
            BasicShape::Ellipse { rx, ry, position } => BasicShape::Ellipse {
                rx: rx.compute(ctx),
                ry: ry.compute(ctx),
                position: pos(position),
            },
            BasicShape::Polygon { evenodd, points } => BasicShape::Polygon {
                evenodd: *evenodd,
                points: points.iter().map(|(x, y)| (lp(x), lp(y))).collect(),
            },
            BasicShape::Path { .. } => self.clone(),
        }
    }

    fn to_css(&self) -> String {
        let pos = |p: &Option<ShapePosition>| match p {
            Some((x, y)) => format!(" at {} {}", x.offset, y.offset),
            None => String::new(),
        };
        match self {
            BasicShape::Inset { offsets, .. } => {
                format!("inset({})", box4_css(offsets, |o| o.to_string()))
            }
            BasicShape::Circle { radius, position } => {
                format!("circle({}{})", radius.to_css(), pos(position))
            }
            BasicShape::Ellipse { rx, ry, position } => {
                format!("ellipse({} {}{})", rx.to_css(), ry.to_css(), pos(position))
            }
            BasicShape::Polygon { evenodd, points } => {
                let pts = points
                    .iter()
                    .map(|(x, y)| format!("{x} {y}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                if *evenodd {
                    format!("polygon(evenodd, {pts})")
                } else {
                    format!("polygon({pts})")
                }
            }
            BasicShape::Path { evenodd, d } => {
                if *evenodd {
                    format!("path(evenodd, {d:?})")
                } else {
                    format!("path({d:?})")
                }
            }
        }
    }
}

/// `clip-path: <clip-source> | [<basic-shape> || <geometry-box>] | none` (CSS Masking 1 §5.1).
#[derive(Debug, Clone, PartialEq)]
pub enum ClipPath {
    None,
    /// Forma con su caja de referencia (`border-box` si se omite).
    Shape(BasicShape, GeometryBox),
    /// Solo la caja: la forma es la caja (con sus radios).
    Box(GeometryBox),
    /// Referencia a un `clipPath` de SVG (no soportada en HTML: no recorta).
    Url(String),
}

impl PropValue for ClipPath {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let items = components(input);
        match items.as_slice() {
            [v] if is_ident(v, "none") => return Some(ClipPath::None),
            [v] if parse_url(v).is_some() => return parse_url(v).map(ClipPath::Url),
            _ => {}
        }
        let mut shape = None;
        let mut reference = None;
        for v in items {
            if shape.is_none()
                && let Some(s) = BasicShape::parse(v)
            {
                shape = Some(s);
            } else if reference.is_none()
                && let Some(b) = ident(v).and_then(GeometryBox::from_ident)
            {
                reference = Some(b);
            } else {
                return None;
            }
        }
        match (shape, reference) {
            (Some(s), r) => Some(ClipPath::Shape(s, r.unwrap_or(GeometryBox::BorderBox))),
            (None, Some(b)) => Some(ClipPath::Box(b)),
            (None, None) => None,
        }
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        match self {
            ClipPath::Shape(s, b) => ClipPath::Shape(s.compute(ctx), *b),
            other => other.clone(),
        }
    }
    fn to_css(&self) -> String {
        match self {
            ClipPath::None => "none".into(),
            ClipPath::Shape(s, GeometryBox::BorderBox) => s.to_css(),
            ClipPath::Shape(s, b) => format!("{} {}", s.to_css(), b.as_str()),
            ClipPath::Box(b) => b.as_str().into(),
            ClipPath::Url(u) => format!("url({u:?})"),
        }
    }
}

// --- mix-blend-mode e isolation ---

macro_rules! keywords {
    ($(#[$m:meta])* $name:ident { $($variant:ident = $css:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name { $($variant),+ }

        impl PropValue for $name {
            fn parse(input: &[ComponentValue]) -> Option<Self> {
                let s = single_ident(input)?;
                $(if s.eq_ignore_ascii_case($css) { return Some($name::$variant); })+
                None
            }
            fn to_css(&self) -> String {
                match self { $($name::$variant => $css.into()),+ }
            }
        }
    };
}

keywords!(
    /// `<blend-mode>` (Compositing and Blending 1 §3.2 y §5).
    MixBlendMode {
        Normal = "normal",
        Multiply = "multiply",
        Screen = "screen",
        Overlay = "overlay",
        Darken = "darken",
        Lighten = "lighten",
        ColorDodge = "color-dodge",
        ColorBurn = "color-burn",
        HardLight = "hard-light",
        SoftLight = "soft-light",
        Difference = "difference",
        Exclusion = "exclusion",
        Hue = "hue",
        Saturation = "saturation",
        Color = "color",
        Luminosity = "luminosity",
        PlusLighter = "plus-lighter",
    }
);

keywords!(
    /// `isolation` (Compositing and Blending 1 §3.4).
    Isolation {
        Auto = "auto",
        Isolate = "isolate",
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use bf_css::syntax::parse_component_value_list;

    fn parse<T: PropValue>(s: &str) -> Option<T> {
        T::parse(&parse_component_value_list(s))
    }

    #[test]
    fn border_image_longhands() {
        let s: BorderImageSlice = parse("10 20% fill").unwrap();
        assert!(s.fill);
        assert_eq!(s.sides[3], SliceValue::Percentage(20.0));
        assert!(parse::<BorderImageSlice>("fill 1 fill").is_none());
        let w: BorderImageWidth = parse("auto 2 10px").unwrap();
        assert_eq!(w.0[3], BorderImageLength::Number(2.0));
        assert!(parse::<BorderImageOutset>("10%").is_none());
        let r: BorderImageRepeat = parse("round space").unwrap();
        assert_eq!((r.0, r.1), (ImageRepeat::Round, ImageRepeat::Space));
    }

    #[test]
    fn border_image_shorthand() {
        let p = parse_border_image(&parse_component_value_list(
            "url(a.png) 30 / 10px / 2 round",
        ))
        .unwrap();
        assert!(matches!(p.source, Some(super::super::types::Image::Url(_))));
        assert_eq!(p.slice.unwrap().sides[0], SliceValue::Number(30.0));
        assert!(p.width.is_some());
        assert_eq!(p.outset.unwrap().0[0], BorderImageLength::Number(2.0));
        assert_eq!(
            p.repeat,
            Some(BorderImageRepeat(ImageRepeat::Round, ImageRepeat::Round))
        );
        let p = parse_border_image(&parse_component_value_list(
            "linear-gradient(red, blue) 10 // 5px",
        ))
        .unwrap();
        assert!(p.width.is_none() && p.outset.is_some());
        assert!(parse_border_image(&parse_component_value_list("10 /")).is_none());
    }

    #[test]
    fn filters() {
        let f: Filter =
            parse("blur(2px) grayscale() hue-rotate(90deg) drop-shadow(red 1px 2px)").unwrap();
        assert_eq!(f.0.len(), 4);
        assert_eq!(f.0[1], FilterFunction::Grayscale(1.0));
        assert!(parse::<Filter>("blur(-1px)").is_none());
        assert!(
            parse::<Filter>("brightness(50%)")
                .is_some_and(|f| f.0[0] == FilterFunction::Brightness(0.5))
        );
        assert_eq!(parse::<Filter>("none"), Some(Filter(Vec::new())));
    }

    #[test]
    fn clip_paths() {
        let c: ClipPath = parse("circle(50% at 10px 20px) content-box").unwrap();
        assert!(matches!(
            c,
            ClipPath::Shape(BasicShape::Circle { .. }, GeometryBox::ContentBox)
        ));
        let c: ClipPath = parse("polygon(evenodd, 0 0, 100% 0, 0 100%)").unwrap();
        assert!(
            matches!(c, ClipPath::Shape(BasicShape::Polygon { evenodd: true, ref points }, _) if points.len() == 3)
        );
        let c: ClipPath = parse("inset(10px 20% round 5px / 2px)").unwrap();
        assert!(matches!(
            c,
            ClipPath::Shape(BasicShape::Inset { .. }, GeometryBox::BorderBox)
        ));
        assert!(matches!(
            parse::<ClipPath>("path('M0 0 L10 0 Z')"),
            Some(ClipPath::Shape(..))
        ));
        assert_eq!(
            parse::<ClipPath>("padding-box"),
            Some(ClipPath::Box(GeometryBox::PaddingBox))
        );
        assert_eq!(
            parse::<MixBlendMode>("color-dodge"),
            Some(MixBlendMode::ColorDodge)
        );
    }
}
