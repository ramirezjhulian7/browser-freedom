//! SVG 2 como imagen (`<img>`, `background-image`, `mask-image`): parseo con el parser XML de
//! `bf-html`, cascada de atributos de presentación, `<style>` y `style` con `bf-css`, y un
//! árbol de render ya resuelto ([`Tree`]) para un tamaño de viewport. El rasterizado es de
//! `bf-paint`.
//!
//! Soportado: `svg` (también anidados), `g`, `a`, `switch`, `defs`, `symbol`, `use`, las
//! formas básicas, `path` completo (arcos incluidos), `text`/`tspan` básicos, `viewBox` y
//! `preserveAspectRatio`, `transform`, `fill`/`stroke` (ancho, extremos, uniones,
//! `miterlimit`, guiones), gradientes lineales y radiales (con `href`, unidades y
//! `gradientTransform`), `opacity`, `clipPath`, `display` y `visibility`.
//!
//! TODO(svg): `mask`, `pattern`, `marker`, `image`, `filter`, `foreignObject`, animación SMIL,
//! `textPath` y el layout de texto completo (§11).

#![forbid(unsafe_code)]

mod build;
pub mod geom;
mod style;

use std::collections::HashMap;

pub use bf_style::values::color::Rgba;
pub use geom::{Seg, Transform};

/// Un documento SVG listo para generar árboles de render.
#[derive(Debug, Clone)]
pub struct Svg {
    pub(crate) nodes: Vec<style::Node>,
    pub(crate) ids: HashMap<String, usize>,
    pub(crate) inherited: Inherited,
}

/// Propiedades que un `<svg>` en línea hereda de sus ancestros HTML.
#[derive(Debug, Clone, PartialEq)]
pub struct Inherited {
    pub color: Rgba,
    pub font_size: f32,
    pub font_family: Vec<String>,
}

impl Default for Inherited {
    fn default() -> Self {
        Inherited {
            color: Rgba::BLACK,
            font_size: 16.0,
            font_family: vec!["serif".into()],
        }
    }
}

/// Tamaño natural de una imagen SVG (CSS Images 3 §5.1): `width`/`height` absolutos del
/// elemento raíz y la proporción del `viewBox`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Intrinsic {
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub ratio: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SvgError(pub String);

impl std::fmt::Display for SvgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SVG inválido: {}", self.0)
    }
}

impl std::error::Error for SvgError {}

impl Svg {
    /// Parsea un documento cuyo elemento raíz es `<svg>`.
    pub fn parse(text: &str) -> Result<Svg, SvgError> {
        style::load(text)
    }

    /// Un `<svg>` en línea de un documento HTML (`root`), con las hojas `css` del documento.
    pub fn from_element(
        doc: &bf_dom::Document,
        root: bf_dom::NodeId,
        css: &[String],
        inherited: Inherited,
    ) -> Result<Svg, SvgError> {
        style::from_element(doc, root, css, inherited)
    }

    /// Parsea bytes (UTF-8, con o sin BOM).
    pub fn parse_bytes(bytes: &[u8]) -> Result<Svg, SvgError> {
        let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
        Svg::parse(&String::from_utf8_lossy(bytes))
    }

    pub fn intrinsic(&self) -> Intrinsic {
        build::intrinsic(self)
    }

    /// Tamaño concreto por defecto (CSS Images 3 §5.3.2, *default sizing*): el natural, lo
    /// que falte por la proporción, y si no 300×150 (o el lado conocido con la proporción).
    pub fn default_size(&self) -> (f32, f32) {
        let i = self.intrinsic();
        match (i.width, i.height, i.ratio) {
            (Some(w), Some(h), _) => (w, h),
            (Some(w), None, Some(r)) => (w, w / r),
            (None, Some(h), Some(r)) => (h * r, h),
            (Some(w), None, None) => (w, 150.0),
            (None, Some(h), None) => (300.0, h),
            (None, None, Some(r)) => (300.0, 300.0 / r),
            (None, None, None) => (300.0, 150.0),
        }
    }

    /// El árbol de render para un viewport de `width`×`height` px.
    pub fn tree(&self, width: f32, height: f32) -> Tree {
        build::tree(self, width, height)
    }
}

/// Árbol de render: todo en coordenadas de usuario, con transformaciones explícitas.
#[derive(Debug, Clone, PartialEq)]
pub struct Tree {
    pub width: f32,
    pub height: f32,
    pub root: Group,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Group(Group),
    Path(Path),
    Text(Text),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Group {
    pub transform: Transform,
    /// `opacity` del grupo (se compone como una capa).
    pub opacity: f32,
    /// Recorte, en el sistema de coordenadas del grupo (ya transformado).
    pub clip: Option<ClipPath>,
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FillRule {
    #[default]
    NonZero,
    EvenOdd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineCap {
    #[default]
    Butt,
    Round,
    Square,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineJoin {
    #[default]
    Miter,
    MiterClip,
    Round,
    Bevel,
    Arcs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Spread {
    #[default]
    Pad,
    Reflect,
    Repeat,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GradientKind {
    Linear {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
    },
    Radial {
        cx: f32,
        cy: f32,
        r: f32,
        fx: f32,
        fy: f32,
        fr: f32,
    },
}

/// Gradiente con la geometría en su propio sistema; `transform` lo lleva al espacio de
/// usuario de la forma (incluye la caja si las unidades son `objectBoundingBox`).
#[derive(Debug, Clone, PartialEq)]
pub struct Gradient {
    pub kind: GradientKind,
    pub transform: Transform,
    pub spread: Spread,
    /// Paradas (posición 0–1 no decreciente, color con la opacidad ya aplicada).
    pub stops: Vec<(f32, Rgba)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Paint {
    Color(Rgba),
    Gradient(Box<Gradient>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fill {
    pub paint: Paint,
    pub opacity: f32,
    pub rule: FillRule,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stroke {
    pub paint: Paint,
    pub opacity: f32,
    pub width: f32,
    pub cap: LineCap,
    pub join: LineJoin,
    pub miter_limit: f32,
    /// Vacío = línea continua.
    pub dasharray: Vec<f32>,
    pub dashoffset: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    pub segs: Vec<Seg>,
    pub transform: Transform,
    pub fill: Option<Fill>,
    pub stroke: Option<Stroke>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextAnchor {
    #[default]
    Start,
    Middle,
    End,
}

/// Fragmento de texto con un solo estilo.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub text: String,
    pub family: Vec<String>,
    pub size: f32,
    pub weight: f32,
    pub italic: bool,
    pub fill: Option<Fill>,
    pub stroke: Option<Stroke>,
    /// Desplazamiento antes del fragmento (`dx`, `dy`).
    pub dx: f32,
    pub dy: f32,
}

/// *Text chunk* (§11.8.3): empieza en una posición absoluta (o donde terminó el anterior) y
/// se alinea entero según `anchor`.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub anchor: TextAnchor,
    pub spans: Vec<Span>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub transform: Transform,
    pub chunks: Vec<Chunk>,
}

/// `clipPath`: la unión de las formas, cada una con su regla y su transformación.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipPath {
    pub transform: Transform,
    pub shapes: Vec<(Vec<Seg>, FillRule, Transform)>,
    /// Textos del recorte (se usan sus contornos).
    pub texts: Vec<Text>,
}
