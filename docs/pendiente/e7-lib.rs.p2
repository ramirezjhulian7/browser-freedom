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
//! También `pattern`, `mask`, `marker`, `paint-order`, `vector-effect: non-scaling-stroke` y
//! `pathLength`.
//!
//! TODO(svg): `image`, `filter`, `foreignObject`, animación SMIL, `textPath` y el layout de
//! texto completo (§11).

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
    /// Elemento referenciado por un `use` → su clon con la cascada de un shadow tree (SVG 2
    /// §5.6.4: los selectores no ven los ancestros del original).
    pub(crate) shadows: HashMap<usize, usize>,
    /// Vista pedida por el fragmento de la URL.
    pub(crate) view: Option<View>,
}

/// Región `#xywh=` (Media Fragments URI 1.0 §4.2.2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Crop {
    Pixel(f32, f32, f32, f32),
    Percent(f32, f32, f32, f32),
}

/// Vista de un fragmento de URL: `#svgView(...)` o el `id` de un `<view>` (SVG 2 §16.1.4) y
/// `#xywh=`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct View {
    pub view_box: Option<(f32, f32, f32, f32)>,
    pub par: Option<String>,
    pub transform: Option<Transform>,
    pub crop: Option<Crop>,
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

    /// La misma imagen vista con el fragmento `fragment` de su URL (ya sin `#` y decodificado):
    /// `svgView(viewBox(…);preserveAspectRatio(…);transform(…))`, el `id` de un `<view>` o
    /// `xywh=[pixel:|percent:]x,y,w,h` (gana el último).
    pub fn with_fragment(&self, fragment: &str) -> Svg {
        let mut svg = self.clone();
        let mut view = View::default();
        let f = fragment.trim();
        // `svgView(…)` solo admite detrás un segmento temporal: con `xywh` se ignora todo
        // (§16.3.2).
        if f.starts_with("svgView(") && f.contains("xywh=") {
            return svg;
        }
        if let Some(spec) = f
            .strip_prefix("svgView(")
            .and_then(|s| s.trim_end().strip_suffix(')'))
        {
            for part in spec.split(';') {
                let part = part.trim();
                let Some((name, arg)) = part.split_once('(') else {
                    continue;
                };
                let arg = arg.trim_end().trim_end_matches(')');
                match name.trim() {
                    "viewBox" => {
                        if let Some(&[x, y, w, h]) = geom::parse_numbers(arg).as_deref()
                            && w > 0.0
                            && h > 0.0
                        {
                            view.view_box = Some((x, y, w, h));
                        }
                    }
                    "preserveAspectRatio" => view.par = Some(arg.replace(',', " ")),
                    "transform" => {
                        view.transform = geom::parse_transform(&format!("{arg})"))
                            .or_else(|| geom::parse_transform(arg))
                    }
                    _ => {}
                }
            }
        } else if let Some(&i) = self.ids.get(f)
            && self.nodes[i].name == "view"
        {
            let n = &self.nodes[i];
            view.view_box = n
                .attr("viewBox")
                .and_then(geom::parse_numbers)
                .and_then(|v| match v.as_slice() {
                    &[x, y, w, h] if w > 0.0 && h > 0.0 => Some((x, y, w, h)),
                    _ => None,
                });
            view.par = n.attr("preserveAspectRatio").map(str::to_owned);
        }
        for param in f.split('&') {
            let Some(v) = param.trim().strip_prefix("xywh=") else {
                continue;
            };
            let (unit, nums) = match v.split_once(':') {
                Some((u, n)) => (u, n),
                None => ("pixel", v),
            };
            // En píxeles solo enteros; en porcentaje, también decimales.
            let n: Vec<f32> = nums
                .split(',')
                .map(str::trim)
                .filter(|s| unit != "pixel" || s.bytes().all(|b| b.is_ascii_digit()))
                .filter_map(|s| s.parse().ok())
                .collect();
            if let &[x, y, w, h] = n.as_slice()
                && x >= 0.0
                && y >= 0.0
                && w > 0.0
                && h > 0.0
            {
                view.crop = match unit {
                    "pixel" => Some(Crop::Pixel(x, y, w, h)),
                    "percent" => Some(Crop::Percent(x, y, w, h)),
                    _ => view.crop,
                };
            }
        }
        if view != View::default() {
            svg.view = Some(view);
        }
        svg
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
    /// Máscara (`mask`, CSS Masking 1 §7), también en el sistema del grupo.
    pub mask: Option<Box<Mask>>,
    pub children: Vec<Node>,
}

/// Elemento `mask` resuelto (CSS Masking 1 §7.1): el contenido se pinta, se recorta a la
/// región `rect` y su luminancia (o su alfa) multiplica la capa del elemento enmascarado.
#[derive(Debug, Clone, PartialEq)]
pub struct Mask {
    /// `mask-type: luminance` (por defecto) o `alpha`.
    pub luminance: bool,
    /// Región de la máscara (`x`, `y`, `width`, `height` ya resueltos según `maskUnits`).
    pub rect: (f32, f32, f32, f32),
    /// Contenido, con la transformación de `maskContentUnits` en el grupo.
    pub content: Group,
}

/// Servidor `pattern` resuelto (SVG 2 §14.4): una tesela de `width`×`height` que se repite.
#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    pub width: f32,
    pub height: f32,
    /// Del sistema de la tesela (origen en su esquina) al espacio de usuario de la forma:
    /// `patternTransform` y el desplazamiento `x`, `y` (con `patternUnits` ya aplicadas).
    pub transform: Transform,
    /// Contenido en el sistema de la tesela (`viewBox` o `patternContentUnits` en el grupo).
    pub content: Group,
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
    Pattern(Box<Pattern>),
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
    /// `vector-effect: non-scaling-stroke` (SVG 2 §13.6.2): ancho y guiones en el espacio del
    /// host (el del viewport raíz), no en el de usuario.
    pub non_scaling: bool,
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
    /// `paint-order` con el trazo antes que el relleno (SVG 2 §13.7.3).
    pub stroke_first: bool,
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
