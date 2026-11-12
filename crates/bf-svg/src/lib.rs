//! SVG 2 como imagen (`<img>`, `background-image`, `mask-image`): parseo con el parser XML de
//! `bf-html`, cascada de atributos de presentación, `<style>` y `style` con `bf-css`, y un
//! árbol de render ya resuelto ([`Tree`]) para un tamaño de viewport. El rasterizado es de
//! `bf-paint`.
//!
//! Soportado: `svg` (también anidados), `g`, `a`, `switch`, `defs`, `symbol`, `use`, las
//! formas básicas, `path` completo (arcos incluidos), `text`/`tspan`/`textPath` (listas `x`,
//! `y`, `dx`, `dy`, `rotate`, `letter-spacing`, `word-spacing`, líneas base), `viewBox` y
//! `preserveAspectRatio`, `transform`, `fill`/`stroke` (ancho, extremos, uniones,
//! `miterlimit`, guiones), gradientes lineales y radiales (con `href`, unidades y
//! `gradientTransform`), `opacity`, `clipPath`, `display` y `visibility`.
//!
//! También `pattern`, `mask`, `marker`, `paint-order`, `vector-effect: non-scaling-stroke` y
//! `pathLength`.
//!
//! `image` queda en el árbol con su `href` sin resolver ([`Image`]): el recurso lo carga y
//! lo decodifica quien pinta (`bf-paint`), con [`Svg::image_hrefs`] para saber cuáles hacen
//! falta.
//!
//! `foreignObject` en línea queda como una [`Image`] con `href`
//! [`FOREIGN_OBJECT_SCHEME`]`<nodo>`: su HTML lo maqueta `bf-paint`.
//!
//! Filtros (Filter Effects 1): `filter: url(#id)` con el elemento `filter` (`filterUnits`,
//! `primitiveUnits`, región, `href`) y las primitivas `feGaussianBlur`, `feOffset`, `feFlood`,
//! `feMerge`, `feColorMatrix`, `feComposite` y `feBlend` ([`Filter`]); las aplica `bf-paint`.
//!
//! TODO(svg): el resto de primitivas (`feImage`, `feTurbulence`, luces, …), las funciones de
//! filtro de CSS (`blur()`, …), animación SMIL y del layout de texto (§11) `textLength`,
//! `inline-size`/`shape-inside` (texto en varias líneas) y el texto vertical.

#![forbid(unsafe_code)]

mod build;
mod external;
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
    /// Documentos externos unidos ([`Svg::attach_external`]), por la parte de documento de
    /// la referencia (la escrita y la clave sintética).
    pub(crate) externals: HashMap<String, external::ExtDoc>,
    /// Documentos unidos hasta ahora (numera las claves sintéticas).
    pub(crate) ext_count: usize,
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

    /// Un documento de recursos externo (SVG 2 §16.1.5): XML con cualquier elemento raíz (un
    /// HTML bien formado, un XML sin `<svg>` arriba); sus elementos se referencian por `id`.
    pub fn parse_resource(bytes: &[u8]) -> Result<Svg, SvgError> {
        let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
        style::load_resource(&String::from_utf8_lossy(bytes))
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

    /// Los `href` de los elementos `image` del documento, tal como están escritos (sin
    /// repetir, en orden de documento).
    pub fn image_hrefs(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for n in &self.nodes {
            if n.name != "image" {
                continue;
            }
            if let Some(h) = n.attr("href").map(str::trim).filter(|h| !h.is_empty())
                && !out.iter().any(|o| o == h)
            {
                out.push(h.to_owned());
            }
        }
        out
    }

    /// Si la raíz tiene un `viewBox` válido (o lo da la vista del fragmento).
    /// Ancho y alto del `viewBox` efectivo (el de la vista del fragmento o el de la raíz).
    pub fn view_box_size(&self) -> Option<(f32, f32)> {
        let vb = self.view.as_ref().and_then(|v| v.view_box).or_else(|| {
            self.nodes
                .first()
                .filter(|r| r.name == "svg")
                .and_then(build::root_view_box)
        })?;
        Some((vb.2, vb.3))
    }

    pub fn has_view_box(&self) -> bool {
        self.view.as_ref().and_then(|v| v.view_box).is_some()
            || self
                .nodes
                .first()
                .is_some_and(|r| r.name == "svg" && build::root_view_box(r).is_some())
    }

    /// La misma imagen con otro `preserveAspectRatio` en la raíz: el de un elemento `image`
    /// que la referencia sustituye al suyo (SVG 2 §12.3, «the preserveAspectRatio attribute on
    /// the root element in the referenced SVG image shall be ignored»).
    pub fn with_aspect(&self, par: &str) -> Svg {
        let mut svg = self.clone();
        svg.view.get_or_insert_with(View::default).par = Some(par.to_owned());
        svg
    }
}

/// `href` con el que un `foreignObject` en línea queda en el árbol: este prefijo y el índice
/// del nodo del DOM (`NodeId::index`). Su contenido lo pinta quien tiene el documento.
pub const FOREIGN_OBJECT_SCHEME: &str = build::FOREIGN_OBJECT_SCHEME;

/// La transformación de `preserveAspectRatio` (SVG 2 §8.6) que lleva el rectángulo
/// `view_box` (x, y, ancho, alto) a un viewport de `size` con origen en (0, 0).
pub fn aspect_transform(view_box: (f32, f32, f32, f32), par: &str, size: (f32, f32)) -> Transform {
    build::view_box_transform(view_box, par, size)
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
    Image(Image),
}

/// Elemento `image` (SVG 2 §12.3). El recurso se resuelve al pintar: un bitmap o un SVG
/// (este con su viewport en el rectángulo del elemento y el `preserveAspectRatio` de aquí).
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    /// `href` tal como está escrito (sin resolver contra la URL del documento).
    pub href: String,
    pub x: f32,
    pub y: f32,
    /// `None` = `auto`: el tamaño natural del recurso (o el que da su proporción).
    pub width: Option<f32>,
    pub height: Option<f32>,
    /// `preserveAspectRatio` sin parsear (vacío = `xMidYMid meet`).
    pub par: String,
    /// `overflow: visible` en un `foreignObject`: su contenido no se recorta a la caja.
    pub overflow_visible: bool,
    pub transform: Transform,
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
    /// Filtro (`filter`, Filter Effects 1 §5), en el sistema del grupo. Se aplica antes que
    /// el recorte, la máscara y la opacidad (CSS Masking 1 §3).
    pub filter: Option<Box<Filter>>,
    /// Tras `transform`, una unidad del grupo mide un px del host (el viewport raíz): los
    /// marcadores con `markerUnits="strokeWidth"` de un trazo `non-scaling-stroke`, que se
    /// escalan por el ancho del trazo en el host (SVG 2 §13.7.2).
    pub host_units: bool,
    /// Pinturas del elemento de contexto para `context-fill`/`context-stroke` del contenido.
    pub context: Option<Box<ContextPaints>>,
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

/// Elemento `filter` resuelto (Filter Effects 1 §9.2): la región del filtro y la cadena de
/// primitivas, todo ya en el sistema de coordenadas del grupo filtrado (`filterUnits` y
/// `primitiveUnits` aplicadas). El resultado de la última primitiva es lo que se pinta,
/// recortado a la región.
#[derive(Debug, Clone, PartialEq)]
pub struct Filter {
    /// Región del filtro (x, y, ancho, alto), §9.2.
    pub region: (f32, f32, f32, f32),
    /// Primitivas en orden de documento (al menos una).
    pub primitives: Vec<FilterPrimitive>,
}

/// Entrada de una primitiva (`in`, `in2`, `feMergeNode`), Filter Effects 1 §9.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterInput {
    /// `SourceGraphic`: el elemento pintado (RGBA premultiplicado).
    SourceGraphic,
    /// `SourceAlpha`: solo el canal alfa de `SourceGraphic`.
    SourceAlpha,
    /// El resultado de la primitiva con ese índice (anterior a la que lo usa).
    Primitive(usize),
    /// Entradas que no soportamos (`BackgroundImage`, `FillPaint`, …): negro transparente.
    Transparent,
}

/// `edgeMode` de `feGaussianBlur` (§9.14): cómo se extiende la imagen de entrada por fuera
/// de sus bordes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EdgeMode {
    /// Valor inicial: ceros fuera de la imagen.
    #[default]
    None,
    Duplicate,
    Wrap,
}

/// Operador de `feComposite` (§9.8).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CompositeOp {
    Over,
    In,
    Out,
    Atop,
    Xor,
    Lighter,
    Arithmetic { k1: f32, k2: f32, k3: f32, k4: f32 },
}

/// Modo de `feBlend` (§9.5, Compositing and Blending 1 §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendMode {
    Normal,
    Multiply,
    Screen,
    Darken,
    Lighten,
}

/// Operación de una primitiva, con sus magnitudes ya en el sistema del grupo.
#[derive(Debug, Clone, PartialEq)]
pub enum FilterKind {
    /// `feGaussianBlur` (§9.14): desviación típica en x e y (≥ 0; 0 = sin desenfoque en ese
    /// eje).
    GaussianBlur { std_dev: (f32, f32), edge: EdgeMode },
    /// `feOffset` (§9.18).
    Offset { dx: f32, dy: f32 },
    /// `feFlood` (§9.13): `flood-color` con `flood-opacity` ya aplicada (en sRGB).
    Flood { color: Rgba },
    /// `feMerge` (§9.16): las entradas de sus `feMergeNode`, de abajo arriba.
    Merge { inputs: Vec<FilterInput> },
    /// `feColorMatrix` (§9.6): matriz 4×5 por filas sobre RGBA no premultiplicado (todos los
    /// `type` se reducen a una matriz).
    ColorMatrix { matrix: [f32; 20] },
    /// `feComposite` (§9.8): `in` con el operador sobre `in2`.
    Composite { op: CompositeOp, in2: FilterInput },
    /// `feBlend` (§9.5): `in` (arriba) mezclada sobre `in2` (fondo).
    Blend { mode: BlendMode, in2: FilterInput },
    /// `feDropShadow` (§9.12): el alfa de `in` desenfocado, desplazado y teñido de `color`,
    /// con `in` encima.
    DropShadow {
        std_dev: (f32, f32),
        dx: f32,
        dy: f32,
        color: Rgba,
    },
    /// Primitiva que no soportamos: deja pasar su entrada tal cual.
    Passthrough,
}

/// Una primitiva de filtro (§9.2–§9.4).
#[derive(Debug, Clone, PartialEq)]
pub struct FilterPrimitive {
    pub kind: FilterKind,
    /// `in` (se ignora en `feFlood` y `feMerge`).
    pub input: FilterInput,
    /// Subregión de la primitiva (§9.4), en el sistema del grupo.
    pub subregion: (f32, f32, f32, f32),
    /// `color-interpolation-filters: linearRGB` (valor inicial, §10); `false` = sRGB. No
    /// afecta a `feOffset` ni a `feFlood` (nota de §10).
    pub linear_rgb: bool,
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
    /// `context-fill`/`context-stroke` (SVG 2 §13.3) cuando la pintura del elemento de contexto
    /// es un servidor de pintura: la da el [`Group::context`] más cercano, que la define en su
    /// propio sistema de coordenadas (los colores y `none` ya vienen resueltos).
    Context(ContextPaint),
}

/// Qué pintura del elemento de contexto se usa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextPaint {
    Fill,
    Stroke,
}

/// Pinturas de un elemento de contexto (SVG 2 §13.3: el `use`, o la forma que pinta un
/// marcador), en el sistema del grupo que las lleva (tras su `transform`). Una pintura puede
/// ser a su vez [`Paint::Context`]: se resuelve con el contexto exterior.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ContextPaints {
    pub fill: Option<Paint>,
    pub stroke: Option<Paint>,
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

/// Línea base con la que se alinea un fragmento (`dominant-baseline` y `alignment-baseline`,
/// SVG 2 §11.10.2 y CSS Inline 3 §4): la posición del texto es la de esa línea, y el glifo se
/// desplaza respecto de su línea alfabética con las métricas de la fuente.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Baseline {
    #[default]
    Alphabetic,
    /// `hanging`: 0,8 del ascent (como los navegadores sin tabla `BASE`).
    Hanging,
    /// `middle`: la mitad de la altura de la x.
    Middle,
    /// `central`: el centro de la caja ascent–descent.
    Central,
    /// `text-before-edge` / `text-top`: el borde del ascent.
    TextBeforeEdge,
    /// `text-after-edge` / `text-bottom`: el borde del descent.
    TextAfterEdge,
    /// `ideographic`: como el borde del descent.
    Ideographic,
    /// `mathematical`: la mitad del ascent.
    Mathematical,
}

/// Fragmento de texto con un solo estilo y sin posiciones absolutas dentro (cada `x`/`y`
/// absoluto abre un [`Chunk`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub text: String,
    pub family: Vec<String>,
    pub size: f32,
    pub weight: f32,
    pub italic: bool,
    pub fill: Option<Fill>,
    pub stroke: Option<Stroke>,
    /// `dx`/`dy` de cada carácter de `text` (SVG 2 §11.8.2, se acumulan); los que falten
    /// valen 0.
    pub dx: Vec<f32>,
    pub dy: Vec<f32>,
    /// `rotate` de cada carácter, en grados (§11.8.2); los que falten valen 0.
    pub rotate: Vec<f32>,
    /// `letter-spacing` y `word-spacing` (CSS Text 3 §8), en unidades de usuario.
    pub letter_spacing: f32,
    pub word_spacing: f32,
    /// Línea base de alineación y `baseline-shift` (hacia arriba, en unidades de usuario).
    pub baseline: Baseline,
    pub baseline_shift: f32,
    /// `paint-order` con el trazo antes que el relleno (SVG 2 §13.7.3).
    pub stroke_first: bool,
    /// `line-height` en unidades de usuario (`None`: `normal`, con las métricas de la fuente).
    /// Solo cuenta en el texto con `inline-size`.
    pub line_height: Option<f32>,
}

/// *Text chunk* (§11.8.3): empieza en una posición absoluta (o donde terminó el anterior) y
/// se alinea entero según `anchor`.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    /// En un camino (`path`), `x` es la distancia a lo largo de él desde `startOffset`.
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub anchor: TextAnchor,
    pub spans: Vec<Span>,
    /// Índice en [`Text::paths`] del `textPath` en el que va el chunk.
    pub path: Option<usize>,
}

/// El camino de un `textPath` (SVG 2 §11.8): en el espacio de usuario del `text`, ya invertido
/// con `side="right"`.
#[derive(Debug, Clone, PartialEq)]
pub struct TextPath {
    pub segs: Vec<Seg>,
    /// `startOffset` resuelto (con `pathLength` aplicado; infinito si la escala lo es).
    pub start_offset: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub transform: Transform,
    pub chunks: Vec<Chunk>,
    pub paths: Vec<TextPath>,
    /// `inline-size` (SVG 2 §11.4.1) en unidades de usuario: el texto (un solo chunk) se corta
    /// en líneas de ese ancho como máximo.
    pub inline_size: Option<f32>,
}

/// `clipPath`: la unión de las formas, cada una con su regla y su transformación.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipPath {
    pub transform: Transform,
    pub shapes: Vec<(Vec<Seg>, FillRule, Transform)>,
    /// Textos del recorte (se usan sus contornos).
    pub texts: Vec<Text>,
    /// `clip-path` del propio `clipPath` (CSS Masking 1 §6.3): el recorte es la intersección.
    /// Está en el mismo sistema que este recorte (antes de su `transform`).
    pub clip: Option<Box<ClipPath>>,
}
