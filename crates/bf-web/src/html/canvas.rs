//! `HTMLCanvasElement` y `CanvasRenderingContext2D` (HTML §4.12.5): el bitmap de cada canvas
//! es un [`Pixmap`] de `tiny-skia` (premultiplicado) guardado por elemento en [`Canvases`].
//!
//! Lo que hay: el estado de dibujo con `save`/`restore` (transformación, estilos de relleno y
//! de trazo con colores CSS, gradientes lineales y radiales y patrones, `lineWidth`,
//! `lineCap`, `lineJoin`, `miterLimit`, `setLineDash`, `globalAlpha`,
//! `globalCompositeOperation`, `font`, `textAlign`, `textBaseline`, la región de recorte); los
//! paths (`moveTo`, `lineTo`, `quadraticCurveTo`, `bezierCurveTo`, `arc`, `arcTo`, `ellipse`,
//! `rect`, `roundRect`, `closePath`), `fill`/`stroke`/`clip`/`isPointInPath`; los rectángulos;
//! el texto con `bf-font` (`fillText`, `strokeText`, `measureText`); `drawImage` desde otro
//! canvas o desde una imagen ya decodificada ([`ImageBitmaps`]); `ImageData` con
//! `getImageData`/`putImageData`/`createImageData`; y `toDataURL` en PNG. Las sombras se
//! guardan pero no se pintan.
//!
//! El navegador muestra el canvas como una imagen: [`snapshot`] da los bitmaps del documento
//! con su versión, y `bf-paint` los pone en `PageImages` bajo `canvas:<id>`.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::f32::consts::PI;
use std::rc::Rc;
use std::sync::Arc;

use bf_dom::{DomException, NodeId};
use bf_infra::DomString;
use bf_js::host::{HostClass, HostClassId, HostHandle, HostKey, webidl};
use bf_js::object::ObjectKind;
use bf_js::string::JsString;
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, RealmId};
use bf_layout::{FontMetrics, FontProvider, FontRequest, PathCommand};
use bf_style::properties::types::{FamilyName, FontFamily};
use bf_style::values::color::Rgba;
use tiny_skia::{
    BlendMode, FillRule, FilterQuality, GradientStop, LineCap, LineJoin, LinearGradient, Mask,
    Paint, Path, PathBuilder, Pattern, Pixmap, PixmapPaint, Point, RadialGradient, Rect, Shader,
    SpreadMode, Stroke, StrokeDash, Transform,
};

use super::elements::{attr_value, set_attr};
use crate::dom::{mutated, this_node, throw_dom, unwrap_node, wrap_node};
use crate::{NodeRef, WebContextHandle};

fn js_str(s: &str) -> Value {
    Value::String(JsString::from(s))
}

// --- Las clases ---

/// Los ids de las clases del canvas.
#[derive(Debug, Clone, Copy)]
pub struct CanvasClasses {
    pub context: HostClassId,
    pub gradient: HostClassId,
    pub pattern: HostClassId,
    pub image_data: HostClassId,
    pub text_metrics: HostClassId,
}

fn classes(agent: &Agent) -> CanvasClasses {
    *agent
        .embedder_data::<CanvasClasses>()
        .expect("canvas::register no se llamó")
}

/// Registra las clases del canvas (una vez por agente).
pub fn register(agent: &mut Agent) {
    if agent.embedder_data::<CanvasClasses>().is_some() {
        return;
    }
    let context = agent.register_host_class(context_class());
    let gradient = agent.register_host_class(HostClass::new("CanvasGradient").method(
        "addColorStop",
        2,
        add_color_stop,
    ));
    let pattern = agent.register_host_class(HostClass::new("CanvasPattern").method(
        "setTransform",
        0,
        pattern_set_transform,
    ));
    let image_data = agent.register_host_class(
        HostClass::new("ImageData")
            .constructor(image_data_ctor, 2)
            .attribute("width", Some(image_data_width), None)
            .attribute("height", Some(image_data_height), None)
            .attribute("data", Some(image_data_data), None)
            .attribute("colorSpace", Some(image_data_color_space), None),
    );
    let metrics_attrs: [(&'static str, bf_js::vm::function::NativeFn); 12] = [
        ("width", tm_width),
        ("actualBoundingBoxLeft", tm_left),
        ("actualBoundingBoxRight", tm_right),
        ("fontBoundingBoxAscent", tm_font_ascent),
        ("fontBoundingBoxDescent", tm_font_descent),
        ("actualBoundingBoxAscent", tm_ascent),
        ("actualBoundingBoxDescent", tm_descent),
        ("emHeightAscent", tm_font_ascent),
        ("emHeightDescent", tm_font_descent),
        ("hangingBaseline", tm_hanging),
        ("alphabeticBaseline", tm_zero),
        ("ideographicBaseline", tm_ideographic),
    ];
    let mut tm = HostClass::new("TextMetrics");
    for (name, f) in metrics_attrs {
        tm = tm.attribute(name, Some(f), None);
    }
    let text_metrics = agent.register_host_class(tm);
    agent.set_embedder_data(CanvasClasses {
        context,
        gradient,
        pattern,
        image_data,
        text_metrics,
    });
}

/// Las operaciones y atributos de `HTMLCanvasElement` (los de `canvas.rs`).
pub fn extend_canvas(class: HostClass) -> HostClass {
    class
        .attribute("width", Some(canvas_width), Some(set_canvas_width))
        .attribute("height", Some(canvas_height), Some(set_canvas_height))
        .method("getContext", 1, get_context)
        .method("toDataURL", 0, to_data_url)
}

fn context_class() -> HostClass {
    HostClass::new("CanvasRenderingContext2D")
        .attribute("canvas", Some(ctx_canvas), None)
        .method("getContextAttributes", 0, get_context_attributes)
        .method("save", 0, save)
        .method("restore", 0, restore)
        .method("reset", 0, reset_context)
        .method("isContextLost", 0, false_fn)
        // Transformaciones.
        .method("scale", 2, scale)
        .method("rotate", 1, rotate)
        .method("translate", 2, translate)
        .method("transform", 6, transform)
        .method("setTransform", 0, set_transform)
        .method("resetTransform", 0, reset_transform)
        .method("getTransform", 0, get_transform)
        // Compositing y estilos.
        .attribute(
            "globalAlpha",
            Some(get_global_alpha),
            Some(set_global_alpha),
        )
        .attribute(
            "globalCompositeOperation",
            Some(get_composite),
            Some(set_composite),
        )
        .attribute(
            "imageSmoothingEnabled",
            Some(get_smoothing),
            Some(set_smoothing),
        )
        .attribute(
            "imageSmoothingQuality",
            Some(get_smoothing_quality),
            Some(set_smoothing_quality),
        )
        .attribute(
            "strokeStyle",
            Some(get_stroke_style),
            Some(set_stroke_style),
        )
        .attribute("fillStyle", Some(get_fill_style), Some(set_fill_style))
        .method("createLinearGradient", 4, create_linear_gradient)
        .method("createRadialGradient", 6, create_radial_gradient)
        .method("createConicGradient", 3, create_conic_gradient)
        .method("createPattern", 2, create_pattern)
        // Sombras (se guardan).
        .attribute("shadowOffsetX", Some(get_shadow_x), Some(set_shadow_x))
        .attribute("shadowOffsetY", Some(get_shadow_y), Some(set_shadow_y))
        .attribute("shadowBlur", Some(get_shadow_blur), Some(set_shadow_blur))
        .attribute(
            "shadowColor",
            Some(get_shadow_color),
            Some(set_shadow_color),
        )
        .attribute("filter", Some(get_filter), Some(set_filter))
        // Rectángulos.
        .method("clearRect", 4, clear_rect)
        .method("fillRect", 4, fill_rect)
        .method("strokeRect", 4, stroke_rect)
        // Paths.
        .method("beginPath", 0, begin_path)
        .method("fill", 0, fill)
        .method("stroke", 0, stroke)
        .method("clip", 0, clip)
        .method("isPointInPath", 2, is_point_in_path)
        .method("isPointInStroke", 2, is_point_in_stroke)
        .method("closePath", 0, close_path)
        .method("moveTo", 2, move_to)
        .method("lineTo", 2, line_to)
        .method("quadraticCurveTo", 4, quadratic_curve_to)
        .method("bezierCurveTo", 6, bezier_curve_to)
        .method("arcTo", 5, arc_to)
        .method("rect", 4, rect)
        .method("roundRect", 4, round_rect)
        .method("arc", 5, arc)
        .method("ellipse", 7, ellipse)
        // Texto.
        .method("fillText", 3, fill_text)
        .method("strokeText", 3, stroke_text)
        .method("measureText", 1, measure_text)
        .attribute("font", Some(get_font), Some(set_font))
        .attribute("textAlign", Some(get_text_align), Some(set_text_align))
        .attribute(
            "textBaseline",
            Some(get_text_baseline),
            Some(set_text_baseline),
        )
        .attribute("direction", Some(get_direction), Some(set_direction))
        // Imágenes y píxeles.
        .method("drawImage", 3, draw_image)
        .method("createImageData", 1, create_image_data)
        .method("getImageData", 4, get_image_data)
        .method("putImageData", 3, put_image_data)
        // Estilo de línea.
        .attribute("lineWidth", Some(get_line_width), Some(set_line_width))
        .attribute("lineCap", Some(get_line_cap), Some(set_line_cap))
        .attribute("lineJoin", Some(get_line_join), Some(set_line_join))
        .attribute("miterLimit", Some(get_miter_limit), Some(set_miter_limit))
        .method("setLineDash", 1, set_line_dash)
        .method("getLineDash", 0, get_line_dash)
        .attribute(
            "lineDashOffset",
            Some(get_line_dash_offset),
            Some(set_line_dash_offset),
        )
}

// --- El estado ---

/// Un estilo de relleno o de trazo.
#[derive(Debug, Clone)]
enum Style {
    Color(Rgba),
    /// Un `CanvasGradient` o `CanvasPattern` (fijado mientras el estado lo use).
    Object(Rc<StyleObject>),
}

/// El objeto de un estilo no color: el handle que lo mantiene vivo.
#[derive(Debug)]
struct StyleObject(HostHandle);

#[derive(Debug, Clone, Copy, PartialEq)]
enum Align {
    Start,
    End,
    Left,
    Right,
    Center,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Baseline {
    Top,
    Hanging,
    Middle,
    Alphabetic,
    Ideographic,
    Bottom,
}

/// El *drawing state* (§4.12.5.1.2).
#[derive(Clone)]
struct DrawState {
    transform: Transform,
    fill: Style,
    stroke: Style,
    line_width: f32,
    line_cap: LineCap,
    line_join: LineJoin,
    miter_limit: f32,
    dash: Vec<f32>,
    dash_offset: f32,
    global_alpha: f32,
    composite: &'static str,
    smoothing: bool,
    smoothing_quality: &'static str,
    shadow_x: f64,
    shadow_y: f64,
    shadow_blur: f64,
    shadow_color: Rgba,
    filter: String,
    font: Font,
    align: Align,
    baseline: Baseline,
    direction: &'static str,
    clip: Option<Mask>,
}

impl Default for DrawState {
    fn default() -> DrawState {
        DrawState {
            transform: Transform::identity(),
            fill: Style::Color(Rgba::BLACK),
            stroke: Style::Color(Rgba::BLACK),
            line_width: 1.0,
            line_cap: LineCap::Butt,
            line_join: LineJoin::Miter,
            miter_limit: 10.0,
            dash: Vec::new(),
            dash_offset: 0.0,
            global_alpha: 1.0,
            composite: "source-over",
            smoothing: true,
            smoothing_quality: "low",
            shadow_x: 0.0,
            shadow_y: 0.0,
            shadow_blur: 0.0,
            shadow_color: Rgba::TRANSPARENT,
            filter: "none".to_owned(),
            font: Font::default(),
            align: Align::Start,
            baseline: Baseline::Alphabetic,
            direction: "inherit",
            clip: None,
        }
    }
}

/// La fuente del contexto: la serialización y lo que pide a `bf-font`.
#[derive(Debug, Clone)]
struct Font {
    css: String,
    size: f32,
    weight: f32,
    italic: bool,
    family: FontFamily,
}

impl Default for Font {
    fn default() -> Font {
        Font {
            css: "10px sans-serif".to_owned(),
            size: 10.0,
            weight: 400.0,
            italic: false,
            family: FontFamily(vec![FamilyName::Generic("sans-serif".to_owned())]),
        }
    }
}

/// Un segmento del path actual, en coordenadas del bitmap (los puntos se transforman al
/// agregarlos, §4.12.5.1.5).
#[derive(Debug, Clone, Copy)]
enum Seg {
    Move(Point),
    Line(Point),
    Quad(Point, Point),
    Cubic(Point, Point, Point),
    Close,
}

/// El path actual.
#[derive(Debug, Clone, Default)]
struct CanvasPath {
    segs: Vec<Seg>,
    /// El último punto (bitmap).
    last: Option<Point>,
    /// El primer punto del subpath actual.
    start: Option<Point>,
}

impl CanvasPath {
    fn move_to(&mut self, p: Point) {
        self.segs.push(Seg::Move(p));
        self.last = Some(p);
        self.start = Some(p);
    }

    /// "Ensure there is a subpath" y la línea.
    fn line_to(&mut self, p: Point) {
        if self.last.is_none() {
            self.move_to(p);
        } else {
            self.segs.push(Seg::Line(p));
            self.last = Some(p);
        }
    }

    fn quad_to(&mut self, c: Point, p: Point) {
        if self.last.is_none() {
            self.move_to(c);
        }
        self.segs.push(Seg::Quad(c, p));
        self.last = Some(p);
    }

    fn cubic_to(&mut self, c1: Point, c2: Point, p: Point) {
        if self.last.is_none() {
            self.move_to(c1);
        }
        self.segs.push(Seg::Cubic(c1, c2, p));
        self.last = Some(p);
    }

    fn close(&mut self) {
        if self.last.is_some() {
            self.segs.push(Seg::Close);
            self.last = self.start;
            // El subpath nuevo empieza en el mismo punto.
            if let Some(s) = self.start {
                self.segs.push(Seg::Move(s));
            }
        }
    }

    /// El path de `tiny-skia` (`None` si no tiene segmentos con área o largo).
    fn build(&self) -> Option<Path> {
        let mut b = PathBuilder::new();
        for s in &self.segs {
            match *s {
                Seg::Move(p) => b.move_to(p.x, p.y),
                Seg::Line(p) => b.line_to(p.x, p.y),
                Seg::Quad(c, p) => b.quad_to(c.x, c.y, p.x, p.y),
                Seg::Cubic(c1, c2, p) => b.cubic_to(c1.x, c1.y, c2.x, c2.y, p.x, p.y),
                Seg::Close => b.close(),
            }
        }
        b.finish()
    }
}

/// El estado de un canvas.
struct Canvas {
    /// Las dimensiones con las que se creó el bitmap.
    width: u32,
    height: u32,
    /// `None` si alguna dimensión es 0.
    pixmap: Option<Pixmap>,
    state: DrawState,
    stack: Vec<DrawState>,
    path: CanvasPath,
    /// Cambia con cada dibujo.
    version: u64,
    /// ¿Ya se pidió el contexto `2d`?
    has_context: bool,
}

impl Canvas {
    fn new(width: u32, height: u32) -> Canvas {
        Canvas {
            width,
            height,
            pixmap: Pixmap::new(width, height),
            state: DrawState::default(),
            stack: Vec::new(),
            path: CanvasPath::default(),
            version: 1,
            has_context: false,
        }
    }

    /// "Reset the rendering context to its default state" (con el bitmap transparente).
    fn reset(&mut self, width: u32, height: u32) {
        let has_context = self.has_context;
        let version = self.version + 1;
        *self = Canvas::new(width, height);
        self.has_context = has_context;
        self.version = version;
    }
}

/// Los canvas de una ventana.
#[derive(Default)]
pub struct Canvases {
    map: HashMap<NodeRef, Canvas>,
}

/// Las imágenes que el navegador ya decodificó (URL → bitmap premultiplicado), para
/// `drawImage` y `createPattern` con `<img>`.
#[derive(Default, Clone)]
pub struct ImageBitmaps(pub HashMap<String, Arc<Pixmap>>);

/// Los bitmaps de los canvas del documento de la ventana (con su versión): lo que el
/// navegador muestra en lugar de cada `<canvas>`.
pub fn snapshot(agent: &Agent, realm: RealmId) -> Vec<(NodeId, u64, Arc<Pixmap>)> {
    let Some(c) = agent.realm_data::<Canvases>(realm) else {
        return Vec::new();
    };
    let mut out: Vec<(NodeId, u64, Arc<Pixmap>)> = c
        .map
        .iter()
        .filter(|(n, _)| n.doc == 0)
        .filter_map(|(n, c)| Some((n.node, c.version, Arc::new(c.pixmap.clone()?))))
        .collect();
    out.sort_by_key(|(n, _, _)| n.index());
    out
}

/// La suma de las versiones de los canvas (cambia cuando alguno se dibuja).
pub fn version(agent: &Agent, realm: RealmId) -> u64 {
    agent
        .realm_data::<Canvases>(realm)
        .map_or(0, |c| c.map.values().map(|c| c.version).sum())
}

/// Las dimensiones del bitmap según los atributos (`unsigned long`, 300×150 por defecto).
fn attr_dims(agent: &Agent, n: NodeRef) -> (u32, u32) {
    let dim = |name: &str, d: u32| {
        attr_value(agent, n, name)
            .and_then(|v| super::reflect::parse_non_negative_integer(&v.to_string_lossy()))
            .filter(|v| *v <= 2147483647)
            .map_or(d, |v| v as u32)
    };
    (dim("width", 300), dim("height", 150))
}

/// El canvas de `n`, creándolo o reiniciándolo si sus atributos cambiaron.
fn canvas_mut(agent: &mut Agent, n: NodeRef) -> &mut Canvas {
    let (w, h) = attr_dims(agent, n);
    let realm = WebContextHandle::current(agent).realm;
    if agent.realm_data::<Canvases>(realm).is_none() {
        agent.set_realm_data(realm, Canvases::default());
    }
    let map = &mut agent
        .realm_data_mut::<Canvases>(realm)
        .expect("Canvases")
        .map;
    let c = map.entry(n).or_insert_with(|| Canvas::new(w, h));
    if c.width != w || c.height != h {
        c.reset(w, h);
    }
    c
}

/// El canvas del contexto en `this` (brand check de `CanvasRenderingContext2D`).
fn this_canvas(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<NodeRef> {
    match agent.host_unwrap::<ContextData>(&a.this) {
        Some(d) => Ok(d.canvas),
        None => agent.type_error("Illegal invocation"),
    }
}

/// El payload del contexto.
struct ContextData {
    canvas: NodeRef,
}

/// Marca el canvas como dibujado (el navegador vuelve a pintar).
fn touched(agent: &mut Agent, n: NodeRef) {
    canvas_mut(agent, n).version += 1;
    mutated(agent);
}

// --- HTMLCanvasElement ---

fn canvas_width(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Number(f64::from(attr_dims(agent, n).0)))
}

fn canvas_height(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Number(f64::from(attr_dims(agent, n).1)))
}

/// Los setters de `width`/`height`: reflejan el atributo y reinician el bitmap y el contexto
/// aunque el valor no cambie (§4.12.5).
fn set_dimension(agent: &mut Agent, a: &CallArgs<'_>, name: &str, default: u32) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = webidl::to_unsigned_long(agent, &a.arg(0))?;
    let v = if v <= 2147483647 { v } else { default };
    set_attr(agent, n, name, DomString::from(v.to_string().as_str()))?;
    let (w, h) = attr_dims(agent, n);
    canvas_mut(agent, n).reset(w, h);
    mutated(agent);
    Ok(Value::Undefined)
}

fn set_canvas_width(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    set_dimension(agent, a, "width", 300)
}

fn set_canvas_height(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    set_dimension(agent, a, "height", 150)
}

/// `getContext(contextId)`: el `CanvasRenderingContext2D` (siempre el mismo objeto) para
/// `"2d"`; `null` para los demás tipos (WebGL no está).
fn get_context(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let id = webidl::to_dom_string(agent, &a.arg(0))?.to_string_lossy();
    if id != "2d" {
        return Ok(Value::Null);
    }
    canvas_mut(agent, n).has_context = true;
    let realm = agent.current_realm;
    let key = HostKey(n.key().0 ^ (1 << 59));
    if let Some(o) = agent.host_lookup(realm, key) {
        return Ok(Value::Object(o));
    }
    let class = classes(agent).context;
    Ok(Value::Object(agent.host_wrap(
        realm,
        key,
        class,
        Box::new(ContextData { canvas: n }),
    )))
}

/// `toDataURL(type)`: PNG en base64 (cualquier tipo: PNG, como pide el spec para los no
/// soportados); `"data:,"` sin bitmap.
fn to_data_url(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let png = canvas_mut(agent, n)
        .pixmap
        .as_ref()
        .and_then(|p| p.encode_png().ok());
    match png {
        Some(bytes) => Ok(js_str(&format!("data:image/png;base64,{}", base64(&bytes)))),
        None => Ok(js_str("data:,")),
    }
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

// --- El contexto: atributos y estado ---

fn ctx_canvas(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_canvas(agent, a)?;
    Ok(wrap_node(agent, n))
}

fn get_context_attributes(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let o = agent.new_plain_object();
    for (k, v) in [
        ("alpha", Value::Boolean(true)),
        ("desynchronized", Value::Boolean(false)),
        ("colorSpace", js_str("srgb")),
        ("willReadFrequently", Value::Boolean(false)),
    ] {
        agent.create_data_property_or_throw(o, bf_js::value::PropertyKey::from(k), v)?;
    }
    Ok(Value::Object(o))
}

fn false_fn(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(false))
}

/// Acceso al estado de dibujo del contexto en `this`.
fn with_state<R>(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    f: impl FnOnce(&mut DrawState) -> R,
) -> JsResult<R> {
    let n = this_canvas(agent, a)?;
    Ok(f(&mut canvas_mut(agent, n).state))
}

fn save(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_canvas(agent, a)?;
    let c = canvas_mut(agent, n);
    c.stack.push(c.state.clone());
    Ok(Value::Undefined)
}

fn restore(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_canvas(agent, a)?;
    let c = canvas_mut(agent, n);
    if let Some(s) = c.stack.pop() {
        c.state = s;
    }
    Ok(Value::Undefined)
}

fn reset_context(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_canvas(agent, a)?;
    let (w, h) = attr_dims(agent, n);
    canvas_mut(agent, n).reset(w, h);
    touched(agent, n);
    Ok(Value::Undefined)
}

/// Los argumentos `unrestricted double`; `None` si alguno no es finito (la operación se
/// ignora, §4.12.5.1).
fn finite_args(agent: &mut Agent, a: &CallArgs<'_>, n: usize) -> JsResult<Option<Vec<f32>>> {
    let mut out = Vec::with_capacity(n);
    let mut ok = true;
    for i in 0..n {
        let x = webidl::to_unrestricted_double(agent, &a.arg(i))?;
        ok &= x.is_finite();
        out.push(x as f32);
    }
    Ok(ok.then_some(out))
}

fn scale(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    if let Some(v) = finite_args(agent, a, 2)? {
        with_state(agent, a, |s| {
            s.transform = s.transform.pre_scale(v[0], v[1])
        })?;
    }
    Ok(Value::Undefined)
}

fn rotate(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    if let Some(v) = finite_args(agent, a, 1)? {
        let (sin, cos) = v[0].sin_cos();
        let r = Transform::from_row(cos, sin, -sin, cos, 0.0, 0.0);
        with_state(agent, a, |s| s.transform = s.transform.pre_concat(r))?;
    }
    Ok(Value::Undefined)
}

fn translate(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    if let Some(v) = finite_args(agent, a, 2)? {
        with_state(agent, a, |s| {
            s.transform = s.transform.pre_translate(v[0], v[1])
        })?;
    }
    Ok(Value::Undefined)
}

fn transform(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    if let Some(v) = finite_args(agent, a, 6)? {
        let m = Transform::from_row(v[0], v[1], v[2], v[3], v[4], v[5]);
        with_state(agent, a, |s| s.transform = s.transform.pre_concat(m))?;
    }
    Ok(Value::Undefined)
}

/// `setTransform(a, b, c, d, e, f)` o `setTransform(DOMMatrix2DInit)`.
fn set_transform(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    if a.args.len() >= 6 {
        if let Some(v) = finite_args(agent, a, 6)? {
            let m = Transform::from_row(v[0], v[1], v[2], v[3], v[4], v[5]);
            with_state(agent, a, |s| s.transform = m)?;
        }
        return Ok(Value::Undefined);
    }
    let m = match a.arg(0) {
        Value::Object(o) => {
            let mut v = [1.0f32, 0.0, 0.0, 1.0, 0.0, 0.0];
            for (i, (k1, k2)) in [
                ("a", "m11"),
                ("b", "m12"),
                ("c", "m21"),
                ("d", "m22"),
                ("e", "m41"),
                ("f", "m42"),
            ]
            .iter()
            .enumerate()
            {
                let mut x =
                    agent.get(o, &bf_js::value::PropertyKey::from(*k1), Value::Object(o))?;
                if matches!(x, Value::Undefined) {
                    x = agent.get(o, &bf_js::value::PropertyKey::from(*k2), Value::Object(o))?;
                }
                if !matches!(x, Value::Undefined) {
                    v[i] = webidl::to_unrestricted_double(agent, &x)? as f32;
                }
            }
            Transform::from_row(v[0], v[1], v[2], v[3], v[4], v[5])
        }
        _ => Transform::identity(),
    };
    with_state(agent, a, |s| s.transform = m)?;
    Ok(Value::Undefined)
}

fn reset_transform(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    with_state(agent, a, |s| s.transform = Transform::identity())?;
    Ok(Value::Undefined)
}

/// `getTransform()`: un objeto con `a`–`f` (y sus alias `m11`…; `DOMMatrix` no está todavía).
fn get_transform(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let t = with_state(agent, a, |s| s.transform)?;
    let o = agent.new_plain_object();
    let vals = [
        ("a", t.sx),
        ("b", t.ky),
        ("c", t.kx),
        ("d", t.sy),
        ("e", t.tx),
        ("f", t.ty),
        ("m11", t.sx),
        ("m12", t.ky),
        ("m21", t.kx),
        ("m22", t.sy),
        ("m41", t.tx),
        ("m42", t.ty),
    ];
    for (k, v) in vals {
        agent.create_data_property_or_throw(
            o,
            bf_js::value::PropertyKey::from(k),
            Value::Number(f64::from(v)),
        )?;
    }
    agent.create_data_property_or_throw(
        o,
        bf_js::value::PropertyKey::from("is2D"),
        Value::Boolean(true),
    )?;
    Ok(Value::Object(o))
}

fn get_global_alpha(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(f64::from(with_state(agent, a, |s| {
        s.global_alpha
    })?)))
}

fn set_global_alpha(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = webidl::to_unrestricted_double(agent, &a.arg(0))?;
    if v.is_finite() && (0.0..=1.0).contains(&v) {
        with_state(agent, a, |s| s.global_alpha = v as f32)?;
    }
    Ok(Value::Undefined)
}

/// Las operaciones de composición (Compositing and Blending §9) que admite el contexto.
fn blend_mode(op: &str) -> Option<(&'static str, BlendMode)> {
    Some(match op {
        "source-over" => ("source-over", BlendMode::SourceOver),
        "source-in" => ("source-in", BlendMode::SourceIn),
        "source-out" => ("source-out", BlendMode::SourceOut),
        "source-atop" => ("source-atop", BlendMode::SourceAtop),
        "destination-over" => ("destination-over", BlendMode::DestinationOver),
        "destination-in" => ("destination-in", BlendMode::DestinationIn),
        "destination-out" => ("destination-out", BlendMode::DestinationOut),
        "destination-atop" => ("destination-atop", BlendMode::DestinationAtop),
        "lighter" => ("lighter", BlendMode::Plus),
        "copy" => ("copy", BlendMode::Source),
        "xor" => ("xor", BlendMode::Xor),
        "multiply" => ("multiply", BlendMode::Multiply),
        "screen" => ("screen", BlendMode::Screen),
        "overlay" => ("overlay", BlendMode::Overlay),
        "darken" => ("darken", BlendMode::Darken),
        "lighten" => ("lighten", BlendMode::Lighten),
        "color-dodge" => ("color-dodge", BlendMode::ColorDodge),
        "color-burn" => ("color-burn", BlendMode::ColorBurn),
        "hard-light" => ("hard-light", BlendMode::HardLight),
        "soft-light" => ("soft-light", BlendMode::SoftLight),
        "difference" => ("difference", BlendMode::Difference),
        "exclusion" => ("exclusion", BlendMode::Exclusion),
        "hue" => ("hue", BlendMode::Hue),
        "saturation" => ("saturation", BlendMode::Saturation),
        "color" => ("color", BlendMode::Color),
        "luminosity" => ("luminosity", BlendMode::Luminosity),
        _ => return None,
    })
}

fn get_composite(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(js_str(with_state(agent, a, |s| s.composite)?))
}

fn set_composite(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = webidl::to_dom_string(agent, &a.arg(0))?.to_string_lossy();
    if let Some((name, _)) = blend_mode(&v) {
        with_state(agent, a, |s| s.composite = name)?;
    }
    Ok(Value::Undefined)
}

fn get_smoothing(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(with_state(agent, a, |s| s.smoothing)?))
}

fn set_smoothing(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let b = webidl::to_boolean(agent, &a.arg(0));
    with_state(agent, a, |s| s.smoothing = b)?;
    Ok(Value::Undefined)
}

fn get_smoothing_quality(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(js_str(with_state(agent, a, |s| s.smoothing_quality)?))
}

fn set_smoothing_quality(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = webidl::to_dom_string(agent, &a.arg(0))?.to_string_lossy();
    let q = match v.as_str() {
        "low" => "low",
        "medium" => "medium",
        "high" => "high",
        _ => return Ok(Value::Undefined),
    };
    with_state(agent, a, |s| s.smoothing_quality = q)?;
    Ok(Value::Undefined)
}

// --- Colores y estilos ---

/// Parsea un `<color>` CSS (sin `currentcolor`, que en un contexto sin elemento es negro).
pub fn parse_css_color(s: &str) -> Option<Rgba> {
    let values = bf_css::syntax::parse_component_value_list(s);
    let values: Vec<_> = values
        .into_iter()
        .filter(|v| {
            !matches!(v, bf_css::syntax::ComponentValue::Token(t)
                if matches!(t.kind, bf_css::syntax::TokenKind::Whitespace))
        })
        .collect();
    let [v] = values.as_slice() else {
        return None;
    };
    match bf_style::values::color::parse_color(v)? {
        bf_style::values::Color::Rgba(c) => Some(c),
        bf_style::values::Color::CurrentColor => Some(Rgba::BLACK),
    }
}

/// "Serialization of a color" para los estilos (§4.12.5.1.7): `#rrggbb` si es opaco y
/// `rgba(r, g, b, a)` si no.
fn serialize_color(c: Rgba) -> String {
    let (r, g, b) = (
        c.r.round().clamp(0.0, 255.0) as u8,
        c.g.round().clamp(0.0, 255.0) as u8,
        c.b.round().clamp(0.0, 255.0) as u8,
    );
    if c.a >= 1.0 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("rgba({r}, {g}, {b}, {})", serialize_alpha(c.a))
    }
}

/// El alfa como el decimal más corto (2 o 3 decimales) que da el mismo valor de 8 bits (CSS
/// Color 4 §15.2, "serializing alpha values").
fn serialize_alpha(a: f32) -> String {
    let byte = (a.clamp(0.0, 1.0) * 255.0).round();
    let exact = f64::from(byte) / 255.0;
    for decimals in [2, 3] {
        let scale = 10f64.powi(decimals);
        let v = (exact * scale).round() / scale;
        if (v * 255.0).round() == f64::from(byte) {
            return super::reflect::float_to_string(v);
        }
    }
    super::reflect::float_to_string((exact * 1000.0).round() / 1000.0)
}

fn style_value(agent: &Agent, s: &Style) -> Value {
    match s {
        Style::Color(c) => js_str(&serialize_color(*c)),
        Style::Object(o) => agent.host_pinned(o.0).unwrap_or(Value::Null),
    }
}

/// El nuevo estilo para `fillStyle`/`strokeStyle` (`None`: se ignora).
fn parse_style(agent: &mut Agent, v: &Value) -> JsResult<Option<Style>> {
    let cls = classes(agent);
    if agent.host_is_instance(v, cls.gradient) || agent.host_is_instance(v, cls.pattern) {
        let h = agent.host_pin(v.clone());
        return Ok(Some(Style::Object(Rc::new(StyleObject(h)))));
    }
    let s = webidl::to_dom_string(agent, v)?.to_string_lossy();
    Ok(parse_css_color(&s).map(Style::Color))
}

fn get_fill_style(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = with_state(agent, a, |s| s.fill.clone())?;
    Ok(style_value(agent, &s))
}

fn set_fill_style(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    if let Some(s) = parse_style(agent, &a.arg(0))? {
        with_state(agent, a, |st| st.fill = s)?;
    }
    Ok(Value::Undefined)
}

fn get_stroke_style(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = with_state(agent, a, |s| s.stroke.clone())?;
    Ok(style_value(agent, &s))
}

fn set_stroke_style(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    if let Some(s) = parse_style(agent, &a.arg(0))? {
        with_state(agent, a, |st| st.stroke = s)?;
    }
    Ok(Value::Undefined)
}

/// Un `CanvasGradient`.
#[derive(Debug, Clone)]
struct GradientData {
    kind: GradientKind,
    stops: Vec<(f32, Rgba)>,
}

#[derive(Debug, Clone, Copy)]
enum GradientKind {
    Linear(f32, f32, f32, f32),
    Radial(f32, f32, f32, f32, f32, f32),
    /// `createConicGradient`: se pinta con el color promedio (sin shader cónico).
    Conic,
}

fn new_gradient(agent: &mut Agent, kind: GradientKind) -> Value {
    let class = classes(agent).gradient;
    Value::Object(agent.host_create(
        class,
        Box::new(GradientData {
            kind,
            stops: Vec::new(),
        }),
    ))
}

fn create_linear_gradient(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_canvas(agent, a)?;
    let v = finite_args(agent, a, 4)?;
    let Some(v) = v else {
        return agent.type_error("The provided double value is non-finite.");
    };
    Ok(new_gradient(
        agent,
        GradientKind::Linear(v[0], v[1], v[2], v[3]),
    ))
}

fn create_radial_gradient(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_canvas(agent, a)?;
    let Some(v) = finite_args(agent, a, 6)? else {
        return agent.type_error("The provided double value is non-finite.");
    };
    if v[2] < 0.0 || v[5] < 0.0 {
        return throw_dom(agent, DomException::IndexSizeError);
    }
    Ok(new_gradient(
        agent,
        GradientKind::Radial(v[0], v[1], v[2], v[3], v[4], v[5]),
    ))
}

fn create_conic_gradient(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_canvas(agent, a)?;
    if finite_args(agent, a, 3)?.is_none() {
        return agent.type_error("The provided double value is non-finite.");
    }
    Ok(new_gradient(agent, GradientKind::Conic))
}

/// `addColorStop(offset, color)` (§4.12.5.1.11).
fn add_color_stop(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let offset = webidl::to_double(agent, &a.arg(0))?;
    let color = webidl::to_dom_string(agent, &a.arg(1))?.to_string_lossy();
    if !(0.0..=1.0).contains(&offset) {
        return throw_dom(agent, DomException::IndexSizeError);
    }
    let Some(c) = parse_css_color(&color) else {
        return throw_dom(agent, DomException::SyntaxError);
    };
    match agent.host_unwrap_mut::<GradientData>(&a.this) {
        Some(g) => {
            // Las paradas con el mismo offset quedan en el orden en que se agregaron.
            let i = g
                .stops
                .iter()
                .position(|(o, _)| *o > offset as f32)
                .unwrap_or(g.stops.len());
            g.stops.insert(i, (offset as f32, c));
            Ok(Value::Undefined)
        }
        None => agent.type_error("Illegal invocation"),
    }
}

/// Un `CanvasPattern`.
struct PatternData {
    pixmap: Arc<Pixmap>,
    repeat_x: bool,
    repeat_y: bool,
    transform: Transform,
}

fn pattern_set_transform(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let mut v = [1.0f32, 0.0, 0.0, 1.0, 0.0, 0.0];
    if let Value::Object(o) = a.arg(0) {
        for (i, k) in ["a", "b", "c", "d", "e", "f"].iter().enumerate() {
            let x = agent.get(o, &bf_js::value::PropertyKey::from(*k), Value::Object(o))?;
            if !matches!(x, Value::Undefined) {
                v[i] = webidl::to_unrestricted_double(agent, &x)? as f32;
            }
        }
    }
    if let Some(p) = agent.host_unwrap_mut::<PatternData>(&a.this) {
        p.transform = Transform::from_row(v[0], v[1], v[2], v[3], v[4], v[5]);
    }
    Ok(Value::Undefined)
}

/// `createPattern(image, repetition)` (§4.12.5.1.12).
fn create_pattern(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_canvas(agent, a)?;
    let source = image_source(agent, &a.arg(0))?;
    let rep = match a.arg(1) {
        Value::Null => String::new(),
        v => webidl::to_dom_string(agent, &v)?.to_string_lossy(),
    };
    let (rx, ry) = match rep.as_str() {
        "" | "repeat" => (true, true),
        "repeat-x" => (true, false),
        "repeat-y" => (false, true),
        "no-repeat" => (false, false),
        _ => return throw_dom(agent, DomException::SyntaxError),
    };
    let Some(pixmap) = source else {
        return Ok(Value::Null);
    };
    let class = classes(agent).pattern;
    Ok(Value::Object(agent.host_create(
        class,
        Box::new(PatternData {
            pixmap,
            repeat_x: rx,
            repeat_y: ry,
            transform: Transform::identity(),
        }),
    )))
}

macro_rules! num_attr {
    ($get:ident, $set:ident, $field:ident, $ty:ty, $check:expr) => {
        fn $get(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
            Ok(Value::Number(f64::from(with_state(agent, a, |s| {
                s.$field
            })?)))
        }
        fn $set(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
            let v = webidl::to_unrestricted_double(agent, &a.arg(0))?;
            let check: fn(f64) -> bool = $check;
            if v.is_finite() && check(v) {
                with_state(agent, a, |s| s.$field = v as $ty)?;
            }
            Ok(Value::Undefined)
        }
    };
}

num_attr!(get_line_width, set_line_width, line_width, f32, |v| v > 0.0);
num_attr!(get_miter_limit, set_miter_limit, miter_limit, f32, |v| v
    > 0.0);
num_attr!(
    get_line_dash_offset,
    set_line_dash_offset,
    dash_offset,
    f32,
    |_| true
);
num_attr!(get_shadow_x, set_shadow_x, shadow_x, f64, |_| true);
num_attr!(get_shadow_y, set_shadow_y, shadow_y, f64, |_| true);
num_attr!(get_shadow_blur, set_shadow_blur, shadow_blur, f64, |v| v
    >= 0.0);

fn get_shadow_color(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(js_str(&serialize_color(with_state(agent, a, |s| {
        s.shadow_color
    })?)))
}

fn set_shadow_color(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = webidl::to_dom_string(agent, &a.arg(0))?.to_string_lossy();
    if let Some(c) = parse_css_color(&v) {
        with_state(agent, a, |s| s.shadow_color = c)?;
    }
    Ok(Value::Undefined)
}

fn get_filter(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(js_str(&with_state(agent, a, |s| s.filter.clone())?))
}

fn set_filter(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = webidl::to_dom_string(agent, &a.arg(0))?.to_string_lossy();
    with_state(agent, a, |s| s.filter = v)?;
    Ok(Value::Undefined)
}

fn get_line_cap(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let c = with_state(agent, a, |s| s.line_cap)?;
    Ok(js_str(match c {
        LineCap::Butt => "butt",
        LineCap::Round => "round",
        LineCap::Square => "square",
    }))
}

fn set_line_cap(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = webidl::to_dom_string(agent, &a.arg(0))?.to_string_lossy();
    let c = match v.as_str() {
        "butt" => LineCap::Butt,
        "round" => LineCap::Round,
        "square" => LineCap::Square,
        _ => return Ok(Value::Undefined),
    };
    with_state(agent, a, |s| s.line_cap = c)?;
    Ok(Value::Undefined)
}

fn get_line_join(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let j = with_state(agent, a, |s| s.line_join)?;
    Ok(js_str(match j {
        LineJoin::Miter | LineJoin::MiterClip => "miter",
        LineJoin::Round => "round",
        LineJoin::Bevel => "bevel",
    }))
}

fn set_line_join(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = webidl::to_dom_string(agent, &a.arg(0))?.to_string_lossy();
    let j = match v.as_str() {
        "miter" => LineJoin::Miter,
        "round" => LineJoin::Round,
        "bevel" => LineJoin::Bevel,
        _ => return Ok(Value::Undefined),
    };
    with_state(agent, a, |s| s.line_join = j)?;
    Ok(Value::Undefined)
}

/// `setLineDash(segments)`: con un número impar de valores se duplica la lista; con alguno
/// negativo o no finito se ignora.
fn set_line_dash(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let list = webidl::to_sequence(agent, &a.arg(0), |agent, v| {
        webidl::to_unrestricted_double(agent, v)
    })?;
    if list.iter().any(|v| !v.is_finite() || *v < 0.0) {
        return Ok(Value::Undefined);
    }
    let mut dash: Vec<f32> = list.iter().map(|v| *v as f32).collect();
    if dash.len() % 2 == 1 {
        dash.extend(dash.clone());
    }
    with_state(agent, a, |s| s.dash = dash)?;
    Ok(Value::Undefined)
}

fn get_line_dash(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let dash = with_state(agent, a, |s| s.dash.clone())?;
    let arr = agent.array_create(0, None)?;
    for (i, d) in dash.iter().enumerate() {
        agent.create_data_property_or_throw(
            arr,
            bf_js::value::PropertyKey::Index(i as u32),
            Value::Number(f64::from(*d)),
        )?;
    }
    Ok(Value::Object(arr))
}

// --- Texto: atributos ---

/// Parsea el shorthand `font` de CSS (§4.12.5.1.4): estilo, peso, tamaño (px, pt, em, %…
/// relativos a 10px) y familias. `None` si no es válido.
fn parse_font(s: &str) -> Option<Font> {
    use bf_css::syntax::{ComponentValue, TokenKind};
    let values = bf_css::syntax::parse_component_value_list(s);
    let mut italic = false;
    let mut weight = 400.0;
    let mut size: Option<f32> = None;
    let mut i = 0;
    let tokens: Vec<&ComponentValue> = values.iter().collect();
    // Antes del tamaño: estilo, variante, peso, stretch o `normal`.
    while i < tokens.len() && size.is_none() {
        match tokens[i] {
            ComponentValue::Token(t) => match &t.kind {
                TokenKind::Whitespace => {}
                TokenKind::Ident(id) => match id.to_ascii_lowercase().as_str() {
                    "italic" | "oblique" => italic = true,
                    "bold" | "bolder" => weight = 700.0,
                    "lighter" => weight = 300.0,
                    "normal" | "small-caps" | "condensed" | "expanded" | "semi-condensed"
                    | "semi-expanded" | "ultra-condensed" | "ultra-expanded"
                    | "extra-condensed" | "extra-expanded" => {}
                    "xx-small" => size = Some(9.0),
                    "x-small" => size = Some(10.0),
                    "small" => size = Some(13.0),
                    "medium" => size = Some(16.0),
                    "large" => size = Some(18.0),
                    "x-large" => size = Some(24.0),
                    "xx-large" => size = Some(32.0),
                    _ => return None,
                },
                TokenKind::Number(n) => weight = (n.value as f32).clamp(1.0, 1000.0),
                TokenKind::Dimension(n, unit) => {
                    let v = n.value as f32;
                    let px = match unit.to_ascii_lowercase().as_str() {
                        "px" => v,
                        "pt" => v * 4.0 / 3.0,
                        "pc" => v * 16.0,
                        "in" => v * 96.0,
                        "cm" => v * 96.0 / 2.54,
                        "mm" => v * 96.0 / 25.4,
                        "em" | "rem" => v * 10.0,
                        _ => return None,
                    };
                    size = Some(px);
                }
                TokenKind::Percentage(p) => size = Some(p.value as f32 / 10.0),
                _ => return None,
            },
            _ => return None,
        }
        i += 1;
    }
    let size = size?;
    // Un `/line-height` opcional.
    let mut rest: Vec<ComponentValue> = tokens[i..].iter().map(|v| (*v).clone()).collect();
    let first = rest.iter().position(
        |v| !matches!(v, ComponentValue::Token(t) if matches!(t.kind, TokenKind::Whitespace)),
    );
    if let Some(f) = first
        && matches!(&rest[f], ComponentValue::Token(t) if matches!(t.kind, TokenKind::Delim('/')))
    {
        let mut j = f + 1;
        while j < rest.len()
            && matches!(&rest[j], ComponentValue::Token(t) if matches!(t.kind, TokenKind::Whitespace))
        {
            j += 1;
        }
        rest.drain(..=j.min(rest.len() - 1));
    }
    use bf_style::properties::PropValue;
    let family = FontFamily::parse(&rest)?;
    if family.0.is_empty() {
        return None;
    }
    let css = serialize_font(italic, weight, size, &family);
    Some(Font {
        css,
        size,
        weight,
        italic,
        family,
    })
}

fn serialize_font(italic: bool, weight: f32, size: f32, family: &FontFamily) -> String {
    let mut out = String::new();
    if italic {
        out.push_str("italic ");
    }
    if weight == 700.0 {
        out.push_str("bold ");
    } else if weight != 400.0 {
        out.push_str(&format!("{} ", weight as u32));
    }
    out.push_str(&format!(
        "{}px ",
        super::reflect::float_to_string(f64::from(size))
    ));
    let names: Vec<String> = family
        .0
        .iter()
        .map(|f| match f {
            FamilyName::Generic(g) => g.clone(),
            FamilyName::Named(n) if n.contains(' ') => format!("\"{n}\""),
            FamilyName::Named(n) => n.clone(),
        })
        .collect();
    out.push_str(&names.join(", "));
    out
}

fn get_font(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(js_str(&with_state(agent, a, |s| s.font.css.clone())?))
}

fn set_font(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = webidl::to_dom_string(agent, &a.arg(0))?.to_string_lossy();
    if let Some(f) = parse_font(&v) {
        with_state(agent, a, |s| s.font = f)?;
    }
    Ok(Value::Undefined)
}

fn get_text_align(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = with_state(agent, a, |s| s.align)?;
    Ok(js_str(match v {
        Align::Start => "start",
        Align::End => "end",
        Align::Left => "left",
        Align::Right => "right",
        Align::Center => "center",
    }))
}

fn set_text_align(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = webidl::to_dom_string(agent, &a.arg(0))?.to_string_lossy();
    let al = match v.as_str() {
        "start" => Align::Start,
        "end" => Align::End,
        "left" => Align::Left,
        "right" => Align::Right,
        "center" => Align::Center,
        _ => return Ok(Value::Undefined),
    };
    with_state(agent, a, |s| s.align = al)?;
    Ok(Value::Undefined)
}

fn get_text_baseline(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = with_state(agent, a, |s| s.baseline)?;
    Ok(js_str(match v {
        Baseline::Top => "top",
        Baseline::Hanging => "hanging",
        Baseline::Middle => "middle",
        Baseline::Alphabetic => "alphabetic",
        Baseline::Ideographic => "ideographic",
        Baseline::Bottom => "bottom",
    }))
}

fn set_text_baseline(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = webidl::to_dom_string(agent, &a.arg(0))?.to_string_lossy();
    let b = match v.as_str() {
        "top" => Baseline::Top,
        "hanging" => Baseline::Hanging,
        "middle" => Baseline::Middle,
        "alphabetic" => Baseline::Alphabetic,
        "ideographic" => Baseline::Ideographic,
        "bottom" => Baseline::Bottom,
        _ => return Ok(Value::Undefined),
    };
    with_state(agent, a, |s| s.baseline = b)?;
    Ok(Value::Undefined)
}

fn get_direction(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(js_str(with_state(agent, a, |s| s.direction)?))
}

fn set_direction(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = webidl::to_dom_string(agent, &a.arg(0))?.to_string_lossy();
    let d = match v.as_str() {
        "ltr" => "ltr",
        "rtl" => "rtl",
        "inherit" => "inherit",
        _ => return Ok(Value::Undefined),
    };
    with_state(agent, a, |s| s.direction = d)?;
    Ok(Value::Undefined)
}

// --- Paths ---

fn map_point(t: &Transform, x: f32, y: f32) -> Point {
    let mut p = [Point::from_xy(x, y)];
    t.map_points(&mut p);
    p[0]
}

/// Agrega al path con la transformación actual.
fn with_path(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    n_args: usize,
    f: impl FnOnce(&mut CanvasPath, &Transform, &[f32]),
) -> JsResult<Value> {
    let n = this_canvas(agent, a)?;
    let Some(v) = finite_args(agent, a, n_args)? else {
        return Ok(Value::Undefined);
    };
    let c = canvas_mut(agent, n);
    let t = c.state.transform;
    f(&mut c.path, &t, &v);
    Ok(Value::Undefined)
}

fn begin_path(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_canvas(agent, a)?;
    canvas_mut(agent, n).path = CanvasPath::default();
    Ok(Value::Undefined)
}

fn close_path(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_canvas(agent, a)?;
    canvas_mut(agent, n).path.close();
    Ok(Value::Undefined)
}

fn move_to(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    with_path(agent, a, 2, |p, t, v| p.move_to(map_point(t, v[0], v[1])))
}

fn line_to(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    with_path(agent, a, 2, |p, t, v| p.line_to(map_point(t, v[0], v[1])))
}

fn quadratic_curve_to(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    with_path(agent, a, 4, |p, t, v| {
        p.quad_to(map_point(t, v[0], v[1]), map_point(t, v[2], v[3]))
    })
}

fn bezier_curve_to(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    with_path(agent, a, 6, |p, t, v| {
        p.cubic_to(
            map_point(t, v[0], v[1]),
            map_point(t, v[2], v[3]),
            map_point(t, v[4], v[5]),
        )
    })
}

fn rect_path(p: &mut CanvasPath, t: &Transform, x: f32, y: f32, w: f32, h: f32) {
    p.move_to(map_point(t, x, y));
    p.line_to(map_point(t, x + w, y));
    p.line_to(map_point(t, x + w, y + h));
    p.line_to(map_point(t, x, y + h));
    p.close();
}

fn rect(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    with_path(agent, a, 4, |p, t, v| {
        rect_path(p, t, v[0], v[1], v[2], v[3])
    })
}

/// Un arco elíptico como curvas de Bézier (de a lo sumo 90°), en coordenadas de usuario
/// transformadas con `t`. Une el primer punto con una línea al path existente.
#[allow(clippy::too_many_arguments)]
fn ellipse_path(
    p: &mut CanvasPath,
    t: &Transform,
    cx: f32,
    cy: f32,
    rx: f32,
    ry: f32,
    rotation: f32,
    start: f32,
    end: f32,
    ccw: bool,
) {
    // El barrido según §4.12.5.1.5 (ellipse): un círculo completo si la diferencia es ≥ 2π.
    let tau = 2.0 * PI;
    let mut sweep = end - start;
    if !ccw {
        if sweep >= tau {
            sweep = tau;
        } else {
            sweep = sweep.rem_euclid(tau);
        }
    } else if -sweep >= tau {
        sweep = -tau;
    } else {
        sweep = -((start - end).rem_euclid(tau));
    }
    let (rs, rc) = rotation.sin_cos();
    let point = |ang: f32| -> (f32, f32) {
        let (s, c) = ang.sin_cos();
        let (x, y) = (rx * c, ry * s);
        (cx + x * rc - y * rs, cy + x * rs + y * rc)
    };
    let (sx, sy) = point(start);
    p.line_to(map_point(t, sx, sy));
    if sweep == 0.0 {
        return;
    }
    let segments = (sweep.abs() / (PI / 2.0)).ceil().max(1.0) as usize;
    let step = sweep / segments as f32;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let mut a0 = start;
    for _ in 0..segments {
        let a1 = a0 + step;
        let (s0, c0) = a0.sin_cos();
        let (s1, c1) = a1.sin_cos();
        // Los puntos de control del arco unitario, escalados y rotados.
        let ctrl = |x: f32, y: f32| -> Point {
            let (ex, ey) = (rx * x, ry * y);
            map_point(t, cx + ex * rc - ey * rs, cy + ex * rs + ey * rc)
        };
        let c1p = ctrl(c0 - k * s0, s0 + k * c0);
        let c2p = ctrl(c1 + k * s1, s1 - k * c1);
        let endp = ctrl(c1, s1);
        p.cubic_to(c1p, c2p, endp);
        a0 = a1;
    }
}

fn arc(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_canvas(agent, a)?;
    let ccw = webidl::to_boolean(agent, &a.arg(5));
    let Some(v) = finite_args(agent, a, 5)? else {
        return Ok(Value::Undefined);
    };
    if v[2] < 0.0 {
        return throw_dom(agent, DomException::IndexSizeError);
    }
    let c = canvas_mut(agent, n);
    let t = c.state.transform;
    ellipse_path(
        &mut c.path,
        &t,
        v[0],
        v[1],
        v[2],
        v[2],
        0.0,
        v[3],
        v[4],
        ccw,
    );
    Ok(Value::Undefined)
}

fn ellipse(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_canvas(agent, a)?;
    let ccw = webidl::to_boolean(agent, &a.arg(7));
    let Some(v) = finite_args(agent, a, 7)? else {
        return Ok(Value::Undefined);
    };
    if v[2] < 0.0 || v[3] < 0.0 {
        return throw_dom(agent, DomException::IndexSizeError);
    }
    let c = canvas_mut(agent, n);
    let t = c.state.transform;
    ellipse_path(
        &mut c.path,
        &t,
        v[0],
        v[1],
        v[2],
        v[3],
        v[4],
        v[5],
        v[6],
        ccw,
    );
    Ok(Value::Undefined)
}

/// `arcTo(x1, y1, x2, y2, radius)` (§4.12.5.1.5), en coordenadas de usuario.
fn arc_to(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_canvas(agent, a)?;
    let Some(v) = finite_args(agent, a, 5)? else {
        return Ok(Value::Undefined);
    };
    if v[4] < 0.0 {
        return throw_dom(agent, DomException::IndexSizeError);
    }
    let c = canvas_mut(agent, n);
    let t = c.state.transform;
    let (x1, y1, x2, y2, r) = (v[0], v[1], v[2], v[3], v[4]);
    let Some(last) = c.path.last else {
        c.path.move_to(map_point(&t, x1, y1));
        return Ok(Value::Undefined);
    };
    // El punto actual en coordenadas de usuario.
    let Some(inv) = t.invert() else {
        return Ok(Value::Undefined);
    };
    let p0 = map_point(&inv, last.x, last.y);
    let (x0, y0) = (p0.x, p0.y);
    let collinear = ((x1 - x0) * (y2 - y1) - (y1 - y0) * (x2 - x1)).abs() < 1e-6;
    if (x0 == x1 && y0 == y1) || (x1 == x2 && y1 == y2) || r == 0.0 || collinear {
        c.path.line_to(map_point(&t, x1, y1));
        return Ok(Value::Undefined);
    }
    // Las tangentes y el centro del círculo.
    let (d0x, d0y) = (x0 - x1, y0 - y1);
    let (d2x, d2y) = (x2 - x1, y2 - y1);
    let l0 = (d0x * d0x + d0y * d0y).sqrt();
    let l2 = (d2x * d2x + d2y * d2y).sqrt();
    let (u0x, u0y) = (d0x / l0, d0y / l0);
    let (u2x, u2y) = (d2x / l2, d2y / l2);
    let cos = (u0x * u2x + u0y * u2y).clamp(-1.0, 1.0);
    let half = cos.acos() / 2.0;
    let dist = r / half.tan();
    let (t0x, t0y) = (x1 + u0x * dist, y1 + u0y * dist);
    let (t2x, t2y) = (x1 + u2x * dist, y1 + u2y * dist);
    let (bx, by) = (u0x + u2x, u0y + u2y);
    let bl = (bx * bx + by * by).sqrt();
    let cd = r / half.sin();
    let (cx, cy) = (x1 + bx / bl * cd, y1 + by / bl * cd);
    let a0 = (t0y - cy).atan2(t0x - cx);
    let a2 = (t2y - cy).atan2(t2x - cx);
    // El sentido: el arco corto entre las tangentes.
    let cross = u0x * u2y - u0y * u2x;
    ellipse_path(&mut c.path, &t, cx, cy, r, r, 0.0, a0, a2, cross > 0.0);
    Ok(Value::Undefined)
}

/// `roundRect(x, y, w, h, radii)` con un radio uniforme (el primero de la lista).
fn round_rect(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_canvas(agent, a)?;
    let Some(v) = finite_args(agent, a, 4)? else {
        return Ok(Value::Undefined);
    };
    let r = match a.arg(4) {
        Value::Undefined => 0.0,
        Value::Object(o) if agent.is_array(&Value::Object(o))? => {
            let first = agent.get(o, &bf_js::value::PropertyKey::Index(0), Value::Object(o))?;
            match first {
                Value::Undefined => 0.0,
                x => webidl::to_unrestricted_double(agent, &x)? as f32,
            }
        }
        x => webidl::to_unrestricted_double(agent, &x)? as f32,
    };
    if r < 0.0 {
        return agent.range_error("The radius provided is negative.");
    }
    let c = canvas_mut(agent, n);
    let t = c.state.transform;
    let (x, y, w, h) = (v[0], v[1], v[2], v[3]);
    let r = r.min(w.abs() / 2.0).min(h.abs() / 2.0);
    if r == 0.0 {
        rect_path(&mut c.path, &t, x, y, w, h);
        return Ok(Value::Undefined);
    }
    let (sx, sy) = (w.signum(), h.signum());
    c.path.move_to(map_point(&t, x + r * sx, y));
    ellipse_path(
        &mut c.path,
        &t,
        x + w - r * sx,
        y + r * sy,
        r,
        r,
        0.0,
        -PI / 2.0,
        0.0,
        false,
    );
    ellipse_path(
        &mut c.path,
        &t,
        x + w - r * sx,
        y + h - r * sy,
        r,
        r,
        0.0,
        0.0,
        PI / 2.0,
        false,
    );
    ellipse_path(
        &mut c.path,
        &t,
        x + r * sx,
        y + h - r * sy,
        r,
        r,
        0.0,
        PI / 2.0,
        PI,
        false,
    );
    ellipse_path(
        &mut c.path,
        &t,
        x + r * sx,
        y + r * sy,
        r,
        r,
        0.0,
        PI,
        1.5 * PI,
        false,
    );
    c.path.close();
    Ok(Value::Undefined)
}

// --- Pintar ---

/// El `Paint` de un estilo con la transformación (de usuario) de los gradientes. Un patrón
/// usa `storage` para su mosaico (el shader lo toma prestado).
fn make_paint<'a>(
    agent: &Agent,
    style: &Style,
    state: &DrawState,
    storage: &'a mut Option<Pixmap>,
) -> Option<Paint<'a>> {
    let alpha = state.global_alpha;
    let mut paint = Paint {
        anti_alias: true,
        blend_mode: blend_mode(state.composite).map_or(BlendMode::SourceOver, |(_, m)| m),
        ..Paint::default()
    };
    match style {
        Style::Color(c) => paint.set_color(to_color(*c, alpha)),
        Style::Object(o) => {
            let v = agent.host_pinned(o.0)?;
            if let Some(g) = agent.host_unwrap::<GradientData>(&v) {
                let stops: Vec<GradientStop> = g
                    .stops
                    .iter()
                    .map(|(o, c)| GradientStop::new(*o, to_color(*c, alpha)))
                    .collect();
                match (g.kind, stops.len()) {
                    (_, 0) => return None,
                    (GradientKind::Conic, _) | (_, 1) => {
                        paint.set_color(to_color(g.stops[0].1, alpha));
                    }
                    (GradientKind::Linear(x0, y0, x1, y1), _) => {
                        paint.shader = LinearGradient::new(
                            Point::from_xy(x0, y0),
                            Point::from_xy(x1, y1),
                            stops,
                            SpreadMode::Pad,
                            state.transform,
                        )
                        .unwrap_or_else(|| {
                            Shader::SolidColor(
                                g.stops.last().map_or(tiny_skia::Color::TRANSPARENT, |s| {
                                    to_color(s.1, alpha)
                                }),
                            )
                        });
                    }
                    (GradientKind::Radial(x0, y0, _r0, x1, y1, r1), _) => {
                        paint.shader = RadialGradient::new(
                            Point::from_xy(x0, y0),
                            Point::from_xy(x1, y1),
                            r1,
                            stops,
                            SpreadMode::Pad,
                            state.transform,
                        )
                        .unwrap_or_else(|| {
                            Shader::SolidColor(
                                g.stops.last().map_or(tiny_skia::Color::TRANSPARENT, |s| {
                                    to_color(s.1, alpha)
                                }),
                            )
                        });
                    }
                }
            } else {
                let p = agent.host_unwrap::<PatternData>(&v)?;
                // El mosaico del patrón (`Pattern` toma prestado su pixmap).
                let tiled: &'a Pixmap = storage.insert(tile_pattern(p));
                let shader = Pattern::new(
                    tiled.as_ref(),
                    SpreadMode::Repeat,
                    if state.smoothing {
                        FilterQuality::Bilinear
                    } else {
                        FilterQuality::Nearest
                    },
                    alpha,
                    state.transform.pre_concat(p.transform),
                );
                paint.shader = shader;
            }
        }
    }
    Some(paint)
}

/// Un patrón sin repetición en un eje se pinta como un mosaico con transparente alrededor.
fn tile_pattern(p: &PatternData) -> Pixmap {
    let (w, h) = (p.pixmap.width(), p.pixmap.height());
    let tw = if p.repeat_x {
        w
    } else {
        w.saturating_mul(64).min(16384)
    };
    let th = if p.repeat_y {
        h
    } else {
        h.saturating_mul(64).min(16384)
    };
    let mut out = Pixmap::new(tw.max(1), th.max(1)).expect("pixmap del patrón");
    out.draw_pixmap(
        0,
        0,
        p.pixmap.as_ref().as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
    out
}

fn to_color(c: Rgba, alpha: f32) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba(
        (c.r / 255.0).clamp(0.0, 1.0),
        (c.g / 255.0).clamp(0.0, 1.0),
        (c.b / 255.0).clamp(0.0, 1.0),
        (c.a * alpha).clamp(0.0, 1.0),
    )
    .unwrap_or(tiny_skia::Color::BLACK)
}

/// Rellena `path` (en coordenadas del bitmap) con el estilo de relleno.
fn paint_fill(agent: &mut Agent, n: NodeRef, path: &Path, rule: FillRule, clear: bool) {
    let c = canvas_mut(agent, n);
    let state = c.state.clone();
    let mut storage = None;
    let paint = if clear {
        Some(Paint {
            blend_mode: BlendMode::Clear,
            ..Paint::default()
        })
    } else {
        make_paint(agent, &state.fill, &state, &mut storage)
    };
    let c = canvas_mut(agent, n);
    if let (Some(pixmap), Some(paint)) = (c.pixmap.as_mut(), paint) {
        pixmap.fill_path(
            path,
            &paint,
            rule,
            Transform::identity(),
            state.clip.as_ref(),
        );
    }
    touched(agent, n);
}

/// Traza `path` (en coordenadas del bitmap) con el estilo de trazo: el path se lleva a
/// coordenadas de usuario y se traza con la transformación actual (el grosor también se
/// transforma).
fn paint_stroke(agent: &mut Agent, n: NodeRef, path: &Path) {
    let c = canvas_mut(agent, n);
    let state = c.state.clone();
    let Some(inv) = state.transform.invert() else {
        return;
    };
    let Some(user) = path.clone().transform(inv) else {
        return;
    };
    let mut storage = None;
    let Some(paint) = make_paint(agent, &state.stroke, &state, &mut storage) else {
        return;
    };
    let stroke = Stroke {
        width: state.line_width,
        miter_limit: state.miter_limit,
        line_cap: state.line_cap,
        line_join: state.line_join,
        dash: if state.dash.is_empty() {
            None
        } else {
            StrokeDash::new(state.dash.clone(), state.dash_offset)
        },
    };
    let c = canvas_mut(agent, n);
    if let Some(pixmap) = c.pixmap.as_mut() {
        pixmap.stroke_path(&user, &paint, &stroke, state.transform, state.clip.as_ref());
    }
    touched(agent, n);
}

fn fill_rule(agent: &mut Agent, v: &Value) -> JsResult<FillRule> {
    if matches!(v, Value::Undefined) {
        return Ok(FillRule::Winding);
    }
    let s = webidl::to_dom_string(agent, v)?.to_string_lossy();
    Ok(if s == "evenodd" {
        FillRule::EvenOdd
    } else {
        FillRule::Winding
    })
}

fn current_path(agent: &mut Agent, n: NodeRef) -> Option<Path> {
    canvas_mut(agent, n).path.build()
}

fn fill(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_canvas(agent, a)?;
    let rule = fill_rule(agent, &a.arg(0))?;
    if let Some(p) = current_path(agent, n) {
        paint_fill(agent, n, &p, rule, false);
    }
    Ok(Value::Undefined)
}

fn stroke(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_canvas(agent, a)?;
    if let Some(p) = current_path(agent, n) {
        paint_stroke(agent, n, &p);
    }
    Ok(Value::Undefined)
}

/// `clip(fillRule)`: la región de recorte pasa a ser su intersección con el path.
fn clip(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_canvas(agent, a)?;
    let rule = fill_rule(agent, &a.arg(0))?;
    let path = current_path(agent, n);
    let c = canvas_mut(agent, n);
    let Some(pixmap) = c.pixmap.as_ref() else {
        return Ok(Value::Undefined);
    };
    let (w, h) = (pixmap.width(), pixmap.height());
    let Some(mut mask) = Mask::new(w, h) else {
        return Ok(Value::Undefined);
    };
    if let Some(p) = path {
        mask.fill_path(&p, rule, true, Transform::identity());
    }
    if let Some(old) = &c.state.clip {
        // Intersección: el mínimo de las dos coberturas.
        let data: Vec<u8> = mask
            .data()
            .iter()
            .zip(old.data())
            .map(|(a, b)| (u16::from(*a) * u16::from(*b) / 255) as u8)
            .collect();
        mask.data_mut().copy_from_slice(&data);
    }
    c.state.clip = Some(mask);
    Ok(Value::Undefined)
}

/// `isPointInPath(x, y, fillRule)`: el punto (sin transformar) dentro del path actual.
fn is_point_in_path(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_canvas(agent, a)?;
    let x = webidl::to_unrestricted_double(agent, &a.arg(0))? as f32;
    let y = webidl::to_unrestricted_double(agent, &a.arg(1))? as f32;
    let rule = fill_rule(agent, &a.arg(2))?;
    if !x.is_finite() || !y.is_finite() {
        return Ok(Value::Boolean(false));
    }
    let Some(path) = current_path(agent, n) else {
        return Ok(Value::Boolean(false));
    };
    Ok(Value::Boolean(point_in_path(&path, x, y, rule)))
}

/// La cobertura del píxel del punto en una máscara de 3×3 alrededor.
fn point_in_path(path: &Path, x: f32, y: f32, rule: FillRule) -> bool {
    let Some(mut mask) = Mask::new(1, 1) else {
        return false;
    };
    mask.fill_path(
        path,
        rule,
        false,
        Transform::from_translate(-x + 0.5, -y + 0.5),
    );
    mask.data()[0] > 0
}

fn is_point_in_stroke(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_canvas(agent, a)?;
    let x = webidl::to_unrestricted_double(agent, &a.arg(0))? as f32;
    let y = webidl::to_unrestricted_double(agent, &a.arg(1))? as f32;
    let Some(path) = current_path(agent, n) else {
        return Ok(Value::Boolean(false));
    };
    let width = canvas_mut(agent, n).state.line_width;
    let stroke = Stroke {
        width,
        ..Stroke::default()
    };
    let r = path
        .stroke(&stroke, 1.0)
        .is_some_and(|s| point_in_path(&s, x, y, FillRule::Winding));
    Ok(Value::Boolean(r))
}

/// Un rectángulo del usuario como path del bitmap.
fn rect_device(t: &Transform, x: f32, y: f32, w: f32, h: f32) -> Option<Path> {
    let mut p = CanvasPath::default();
    rect_path(&mut p, t, x, y, w, h);
    p.build()
}

fn fill_rect(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_canvas(agent, a)?;
    let Some(v) = finite_args(agent, a, 4)? else {
        return Ok(Value::Undefined);
    };
    let t = canvas_mut(agent, n).state.transform;
    if v[2] != 0.0
        && v[3] != 0.0
        && let Some(p) = rect_device(&t, v[0], v[1], v[2], v[3])
    {
        paint_fill(agent, n, &p, FillRule::Winding, false);
    }
    Ok(Value::Undefined)
}

fn clear_rect(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_canvas(agent, a)?;
    let Some(v) = finite_args(agent, a, 4)? else {
        return Ok(Value::Undefined);
    };
    let t = canvas_mut(agent, n).state.transform;
    if v[2] != 0.0
        && v[3] != 0.0
        && let Some(p) = rect_device(&t, v[0], v[1], v[2], v[3])
    {
        paint_fill(agent, n, &p, FillRule::Winding, true);
    }
    Ok(Value::Undefined)
}

fn stroke_rect(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_canvas(agent, a)?;
    let Some(v) = finite_args(agent, a, 4)? else {
        return Ok(Value::Undefined);
    };
    let t = canvas_mut(agent, n).state.transform;
    if (v[2] != 0.0 || v[3] != 0.0)
        && let Some(p) = rect_device(&t, v[0], v[1], v[2], v[3])
    {
        paint_stroke(agent, n, &p);
    }
    Ok(Value::Undefined)
}

// --- Texto ---

thread_local! {
    static SYSTEM_FONTS: OnceCell<Rc<bf_font::Fonts>> = const { OnceCell::new() };
}

/// Las fuentes del canvas: las del navegador si las fijó (`CanvasFonts`), si no las del
/// sistema.
#[derive(Clone)]
pub struct CanvasFonts(pub Rc<dyn FontProvider>);

fn fonts(agent: &Agent) -> Rc<dyn FontProvider> {
    let realm = WebContextHandle::current(agent).realm;
    if let Some(f) = agent.realm_data::<CanvasFonts>(realm) {
        return f.0.clone();
    }
    SYSTEM_FONTS.with(|c| c.get_or_init(|| Rc::new(bf_font::Fonts::system())).clone())
}

/// El texto con los espacios en blanco reemplazados por espacios (§4.12.5.1.4, "text
/// preparation algorithm").
fn prepare_text(s: &str) -> String {
    s.chars()
        .map(|c| {
            if matches!(c, '\t' | '\n' | '\x0C' | '\r') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// El texto medido: glifos, ancho, métricas y la caja de tinta (x mínimo y máximo, alto
/// sobre y bajo la baseline).
struct Shaped {
    glyphs: Vec<bf_layout::Glyph>,
    width: f32,
    metrics: FontMetrics,
    ink: (f32, f32, f32, f32),
}

fn shape(fonts: &dyn FontProvider, font: &Font, text: &str) -> Shaped {
    let req = FontRequest {
        family: &font.family,
        size: font.size,
        weight: font.weight,
        italic: font.italic,
    };
    let glyphs = fonts.shape(text, &req);
    let metrics = fonts.metrics(&req);
    let width: f32 = glyphs.iter().map(|g| g.advance).sum();
    // La caja de tinta con los contornos (o la caja de la fuente si no hay).
    let (mut x0, mut x1, mut up, mut down) = (f32::MAX, f32::MIN, 0.0f32, 0.0f32);
    let mut pen = 0.0;
    for g in &glyphs {
        if let Some(o) = fonts.outline(g.font, g.id) {
            let s = font.size / o.units_per_em;
            for cmd in &o.commands {
                let pts: &[(f32, f32)] = &match *cmd {
                    PathCommand::MoveTo(x, y) | PathCommand::LineTo(x, y) => vec![(x, y)],
                    PathCommand::QuadTo(a, b, x, y) => vec![(a, b), (x, y)],
                    PathCommand::CurveTo(a, b, c, d, x, y) => vec![(a, b), (c, d), (x, y)],
                    PathCommand::Close => vec![],
                };
                for (x, y) in pts {
                    let px = pen + g.x_offset + x * s;
                    let py = y * s + g.y_offset;
                    x0 = x0.min(px);
                    x1 = x1.max(px);
                    up = up.max(py);
                    down = down.max(-py);
                }
            }
        }
        pen += g.advance;
    }
    if x0 > x1 {
        (x0, x1, up, down) = (0.0, width, metrics.ascent, metrics.descent);
    }
    Shaped {
        glyphs,
        width,
        metrics,
        ink: (x0, x1, up, down),
    }
}

/// El desplazamiento horizontal según `textAlign` (con `direction` ltr) y el vertical según
/// `textBaseline` (§4.12.5.1.4, pasos 6 y 7).
fn text_anchor(state: &DrawState, shaped: &Shaped) -> (f32, f32) {
    let rtl = state.direction == "rtl";
    let dx = match (state.align, rtl) {
        (Align::Left, _) | (Align::Start, false) | (Align::End, true) => 0.0,
        (Align::Right, _) | (Align::End, false) | (Align::Start, true) => -shaped.width,
        (Align::Center, _) => -shaped.width / 2.0,
    };
    let m = &shaped.metrics;
    let dy = match state.baseline {
        Baseline::Top => m.ascent,
        Baseline::Hanging => m.ascent * 0.8,
        Baseline::Middle => (m.ascent - m.descent) / 2.0,
        Baseline::Alphabetic => 0.0,
        Baseline::Ideographic | Baseline::Bottom => -m.descent,
    };
    (dx, dy)
}

/// El path del texto en coordenadas de usuario.
fn text_path(
    fonts: &dyn FontProvider,
    shaped: &Shaped,
    x: f32,
    y: f32,
    sx: f32,
    size: f32,
) -> Option<Path> {
    let mut b = PathBuilder::new();
    let mut pen = 0.0;
    for g in &shaped.glyphs {
        if let Some(o) = fonts.outline(g.font, g.id) {
            let s = size / o.units_per_em;
            let gx = x + (pen + g.x_offset) * sx;
            let gy = y - g.y_offset;
            let pt = |px: f32, py: f32| (gx + px * s * sx, gy - py * s);
            for cmd in &o.commands {
                match *cmd {
                    PathCommand::MoveTo(px, py) => {
                        let (a, c) = pt(px, py);
                        b.move_to(a, c);
                    }
                    PathCommand::LineTo(px, py) => {
                        let (a, c) = pt(px, py);
                        b.line_to(a, c);
                    }
                    PathCommand::QuadTo(c1x, c1y, px, py) => {
                        let (a1, b1) = pt(c1x, c1y);
                        let (a, c) = pt(px, py);
                        b.quad_to(a1, b1, a, c);
                    }
                    PathCommand::CurveTo(c1x, c1y, c2x, c2y, px, py) => {
                        let (a1, b1) = pt(c1x, c1y);
                        let (a2, b2) = pt(c2x, c2y);
                        let (a, c) = pt(px, py);
                        b.cubic_to(a1, b1, a2, b2, a, c);
                    }
                    PathCommand::Close => b.close(),
                }
            }
        }
        pen += g.advance;
    }
    b.finish()
}

/// `fillText`/`strokeText` (§4.12.5.1.4 "text drawing algorithm").
fn draw_text(agent: &mut Agent, a: &CallArgs<'_>, stroke: bool) -> JsResult<Value> {
    let n = this_canvas(agent, a)?;
    let text = webidl::to_dom_string(agent, &a.arg(0))?.to_string_lossy();
    let x = webidl::to_unrestricted_double(agent, &a.arg(1))?;
    let y = webidl::to_unrestricted_double(agent, &a.arg(2))?;
    let max = match a.arg(3) {
        Value::Undefined => None,
        v => Some(webidl::to_unrestricted_double(agent, &v)?),
    };
    if !x.is_finite() || !y.is_finite() || max.is_some_and(|m| !m.is_finite() || m <= 0.0) {
        return Ok(Value::Undefined);
    }
    let fonts = fonts(agent);
    let state = canvas_mut(agent, n).state.clone();
    let shaped = shape(&*fonts, &state.font, &prepare_text(&text));
    // Con `maxWidth` menor que el ancho, el texto se comprime horizontalmente.
    let sx = match max {
        Some(m) if (shaped.width as f64) > m => (m / f64::from(shaped.width)) as f32,
        _ => 1.0,
    };
    let (dx, dy) = text_anchor(&state, &shaped);
    let Some(user) = text_path(
        &*fonts,
        &shaped,
        x as f32 + dx * sx,
        y as f32 + dy,
        sx,
        state.font.size,
    ) else {
        return Ok(Value::Undefined);
    };
    let Some(device) = user.transform(state.transform) else {
        return Ok(Value::Undefined);
    };
    if stroke {
        paint_stroke(agent, n, &device);
    } else {
        paint_fill(agent, n, &device, FillRule::Winding, false);
    }
    Ok(Value::Undefined)
}

fn fill_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    draw_text(agent, a, false)
}

fn stroke_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    draw_text(agent, a, true)
}

/// Un `TextMetrics`.
#[derive(Debug, Clone, Copy)]
struct Metrics {
    width: f64,
    left: f64,
    right: f64,
    font_ascent: f64,
    font_descent: f64,
    ascent: f64,
    descent: f64,
    hanging: f64,
    ideographic: f64,
}

/// `measureText(text)` (§4.12.5.1.4): las medidas relativas al punto de anclaje según
/// `textAlign` y `textBaseline`.
fn measure_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_canvas(agent, a)?;
    let text = webidl::to_dom_string(agent, &a.arg(0))?.to_string_lossy();
    let fonts = fonts(agent);
    let state = canvas_mut(agent, n).state.clone();
    let shaped = shape(&*fonts, &state.font, &prepare_text(&text));
    let (dx, dy) = text_anchor(&state, &shaped);
    let m = &shaped.metrics;
    let (ix0, ix1, up, down) = shaped.ink;
    let metrics = Metrics {
        width: f64::from(shaped.width),
        left: f64::from(-(ix0 + dx)),
        right: f64::from(ix1 + dx),
        font_ascent: f64::from(m.ascent + dy),
        font_descent: f64::from(m.descent - dy),
        ascent: f64::from(up + dy),
        descent: f64::from(down - dy),
        hanging: f64::from(m.ascent * 0.8 - dy),
        ideographic: f64::from(-m.descent - dy),
    };
    let class = classes(agent).text_metrics;
    Ok(Value::Object(agent.host_create(class, Box::new(metrics))))
}

macro_rules! metric {
    ($name:ident, $field:ident) => {
        fn $name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
            match agent.host_unwrap::<Metrics>(&a.this) {
                Some(m) => Ok(Value::Number(m.$field)),
                None => agent.type_error("Illegal invocation"),
            }
        }
    };
}

metric!(tm_width, width);
metric!(tm_left, left);
metric!(tm_right, right);
metric!(tm_font_ascent, font_ascent);
metric!(tm_font_descent, font_descent);
metric!(tm_ascent, ascent);
metric!(tm_descent, descent);
metric!(tm_hanging, hanging);
metric!(tm_ideographic, ideographic);

fn tm_zero(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    match agent.host_unwrap::<Metrics>(&a.this) {
        Some(_) => Ok(Value::Number(0.0)),
        None => agent.type_error("Illegal invocation"),
    }
}

// --- Imágenes ---

/// El bitmap de una fuente de imagen (§4.12.5.1.14 "check the usability of the image
/// argument"): un canvas, o un `<img>` ya decodificado. `Ok(None)` si no está listo; `Err`
/// si el tipo no sirve (TypeError) o el canvas no tiene píxeles (InvalidStateError).
fn image_source(agent: &mut Agent, v: &Value) -> JsResult<Option<Arc<Pixmap>>> {
    let Some(node) = unwrap_node(agent, v) else {
        return agent.type_error(
            "The provided value is not of type '(CSSImageValue or HTMLCanvasElement or HTMLImageElement or HTMLVideoElement or ImageBitmap or OffscreenCanvas or SVGImageElement or VideoFrame)'",
        );
    };
    let name = super::elements::html_local_name(agent, node);
    match name.as_deref() {
        Some("canvas") => {
            let c = canvas_mut(agent, node);
            match &c.pixmap {
                Some(p) => Ok(Some(Arc::new(p.clone()))),
                None => throw_dom(agent, DomException::InvalidStateError),
            }
        }
        Some("img") => {
            let src = attr_value(agent, node, "src")
                .and_then(|s| super::elements::parse_url(agent, node, &s.to_string_lossy()));
            let realm = WebContextHandle::current(agent).realm;
            Ok(src.and_then(|u| {
                agent
                    .realm_data::<ImageBitmaps>(realm)?
                    .0
                    .get(&u.serialize(false))
                    .cloned()
            }))
        }
        Some("video") => Ok(None),
        _ => agent.type_error("The provided value is not an image source"),
    }
}

/// `drawImage(image, dx, dy[, dw, dh])` y `drawImage(image, sx, sy, sw, sh, dx, dy, dw, dh)`
/// (§4.12.5.1.14).
fn draw_image(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_canvas(agent, a)?;
    let source = image_source(agent, &a.arg(0))?;
    let count = a.args.len();
    if !matches!(count, 3 | 5 | 9) {
        return agent.type_error("Valid arities are: [3, 5, 9]");
    }
    let mut v = Vec::with_capacity(count - 1);
    for i in 1..count {
        let x = webidl::to_unrestricted_double(agent, &a.arg(i))?;
        if !x.is_finite() {
            return Ok(Value::Undefined);
        }
        v.push(x as f32);
    }
    let Some(img) = source else {
        return Ok(Value::Undefined);
    };
    let (iw, ih) = (img.width() as f32, img.height() as f32);
    let (sx, sy, sw, sh, dx, dy, dw, dh) = match count {
        3 => (0.0, 0.0, iw, ih, v[0], v[1], iw, ih),
        5 => (0.0, 0.0, iw, ih, v[0], v[1], v[2], v[3]),
        _ => (v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7]),
    };
    if sw == 0.0 || sh == 0.0 || dw == 0.0 || dh == 0.0 {
        return Ok(Value::Undefined);
    }
    let state = canvas_mut(agent, n).state.clone();
    // El rectángulo destino recibe la imagen escalada de modo que el rectángulo fuente
    // coincida con él.
    let shader_t = state
        .transform
        .pre_translate(dx, dy)
        .pre_scale(dw / sw, dh / sh)
        .pre_translate(-sx, -sy);
    let shader = Pattern::new(
        img.as_ref().as_ref(),
        SpreadMode::Pad,
        if state.smoothing {
            FilterQuality::Bilinear
        } else {
            FilterQuality::Nearest
        },
        state.global_alpha,
        shader_t,
    );
    let paint = Paint {
        shader,
        anti_alias: true,
        blend_mode: blend_mode(state.composite).map_or(BlendMode::SourceOver, |(_, m)| m),
        ..Paint::default()
    };
    // El área: la intersección del rectángulo fuente con la imagen, llevada al destino.
    let (cx0, cy0) = (sx.max(0.0), sy.max(0.0));
    let (cx1, cy1) = ((sx + sw).min(iw), (sy + sh).min(ih));
    if cx1 <= cx0 || cy1 <= cy0 {
        return Ok(Value::Undefined);
    }
    let map_x = |x: f32| dx + (x - sx) * dw / sw;
    let map_y = |y: f32| dy + (y - sy) * dh / sh;
    let Some(r) = Rect::from_ltrb(
        map_x(cx0).min(map_x(cx1)),
        map_y(cy0).min(map_y(cy1)),
        map_x(cx0).max(map_x(cx1)),
        map_y(cy0).max(map_y(cy1)),
    ) else {
        return Ok(Value::Undefined);
    };
    let path = PathBuilder::from_rect(r);
    let c = canvas_mut(agent, n);
    if let Some(pixmap) = c.pixmap.as_mut() {
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            state.transform,
            state.clip.as_ref(),
        );
    }
    touched(agent, n);
    Ok(Value::Undefined)
}

// --- ImageData ---

/// Un `ImageData`: el `Uint8ClampedArray` está en el slot 0 del objeto.
#[derive(Debug, Clone, Copy)]
struct ImageDataInfo {
    width: u32,
    height: u32,
}

/// Un `Uint8ClampedArray` nuevo con `bytes`.
fn new_clamped_array(agent: &mut Agent, bytes: Vec<u8>) -> JsResult<Value> {
    let buffer = bf_js::builtins::array_buffer::allocate_default(agent, bytes.len() as u64)?;
    if let ObjectKind::ArrayBuffer(d) = &mut agent.heap[buffer].kind {
        d.data = Some(bytes);
    }
    let ctor = agent.intrinsics().uint8_clamped_array_ctor;
    agent.construct(ctor, &[Value::Object(buffer)], None)
}

/// Los bytes de un `Uint8ClampedArray`.
fn clamped_bytes(agent: &Agent, v: &Value) -> Option<Vec<u8>> {
    let Value::Object(o) = v else { return None };
    let ta = bf_js::builtins::typed_array::ta_data(agent, *o)?;
    if ta.kind != bf_js::builtins::array_buffer::ElementType::Uint8Clamped {
        return None;
    }
    let len = bf_js::builtins::typed_array::ta_length(agent, *o)?;
    let data = bf_js::builtins::array_buffer::buffer_data(agent, ta.buffer)?
        .data
        .as_ref()?;
    data.get(ta.byte_offset..ta.byte_offset + len)
        .map(<[u8]>::to_vec)
}

/// Crea un `ImageData` de `width`×`height` con `bytes` (RGBA sin premultiplicar).
fn new_image_data(agent: &mut Agent, width: u32, height: u32, bytes: Vec<u8>) -> JsResult<Value> {
    let data = new_clamped_array(agent, bytes)?;
    let class = classes(agent).image_data;
    let o = agent.host_create(class, Box::new(ImageDataInfo { width, height }));
    if let Some(slots) = agent.host_slots_mut(o) {
        slots.push(data);
    }
    Ok(Value::Object(o))
}

/// Las dimensiones válidas de un `ImageData` (IndexSizeError con 0; RangeError si es
/// demasiado grande).
fn image_dims(agent: &mut Agent, w: &Value, h: &Value) -> JsResult<(u32, u32)> {
    let w = webidl::to_long_enforce_range(agent, w)?;
    let h = webidl::to_long_enforce_range(agent, h)?;
    if w == 0 || h == 0 {
        return throw_dom(agent, DomException::IndexSizeError);
    }
    let (w, h) = (w.unsigned_abs(), h.unsigned_abs());
    if u64::from(w) * u64::from(h) * 4 > (1 << 31) {
        return agent.range_error("Out of memory at ImageData creation");
    }
    Ok((w, h))
}

/// `new ImageData(sw, sh)` y `new ImageData(data, sw[, sh])`.
fn image_data_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Illegal constructor");
    };
    let class = classes(agent).image_data;
    let (w, h, data) = if let Some(bytes) = clamped_bytes(agent, &a.arg(0)) {
        let w = webidl::to_unsigned_long(agent, &a.arg(1))?;
        if bytes.is_empty() || bytes.len() % 4 != 0 {
            return throw_dom(agent, DomException::InvalidStateError);
        }
        if w == 0 || (bytes.len() / 4) % w as usize != 0 {
            return throw_dom(agent, DomException::IndexSizeError);
        }
        let h = (bytes.len() / 4) as u32 / w;
        if !matches!(a.arg(2), Value::Undefined) && webidl::to_unsigned_long(agent, &a.arg(2))? != h
        {
            return throw_dom(agent, DomException::IndexSizeError);
        }
        (w, h, a.arg(0))
    } else {
        let (w, h) = image_dims(agent, &a.arg(0), &a.arg(1))?;
        let data = new_clamped_array(agent, vec![0; (w * h * 4) as usize])?;
        (w, h, data)
    };
    let o = agent.host_create_from_new_target(
        nt,
        class,
        None,
        Box::new(ImageDataInfo {
            width: w,
            height: h,
        }),
    )?;
    if let Some(slots) = agent.host_slots_mut(o) {
        slots.push(data);
    }
    Ok(Value::Object(o))
}

fn image_data_info(agent: &mut Agent, v: &Value) -> JsResult<(ImageDataInfo, Value)> {
    let Some(info) = agent.host_unwrap::<ImageDataInfo>(v).copied() else {
        return agent.type_error("The provided value is not of type 'ImageData'");
    };
    let data = match v {
        Value::Object(o) => agent
            .host_slots_mut(*o)
            .and_then(|s| s.first().cloned())
            .unwrap_or(Value::Undefined),
        _ => Value::Undefined,
    };
    Ok((info, data))
}

fn image_data_width(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (i, _) = image_data_info(agent, &a.this)?;
    Ok(Value::Number(f64::from(i.width)))
}

fn image_data_height(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (i, _) = image_data_info(agent, &a.this)?;
    Ok(Value::Number(f64::from(i.height)))
}

fn image_data_data(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (_, d) = image_data_info(agent, &a.this)?;
    Ok(d)
}

fn image_data_color_space(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    image_data_info(agent, &a.this)?;
    Ok(js_str("srgb"))
}

/// `createImageData(sw, sh)` o `createImageData(imagedata)`: transparente negro.
fn create_image_data(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_canvas(agent, a)?;
    let (w, h) = if agent.host_unwrap::<ImageDataInfo>(&a.arg(0)).is_some() {
        let (i, _) = image_data_info(agent, &a.arg(0))?;
        (i.width, i.height)
    } else {
        image_dims(agent, &a.arg(0), &a.arg(1))?
    };
    new_image_data(agent, w, h, vec![0; (w * h * 4) as usize])
}

/// `getImageData(sx, sy, sw, sh)`: los píxeles (sin premultiplicar); fuera del bitmap,
/// transparente negro.
fn get_image_data(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_canvas(agent, a)?;
    let mut v = [0i64; 4];
    for (i, x) in v.iter_mut().enumerate() {
        *x = i64::from(webidl::to_long_enforce_range(agent, &a.arg(i))?);
    }
    if v[2] == 0 || v[3] == 0 {
        return throw_dom(agent, DomException::IndexSizeError);
    }
    let (mut sx, mut sy, mut sw, mut sh) = (v[0], v[1], v[2], v[3]);
    if sw < 0 {
        sx += sw;
        sw = -sw;
    }
    if sh < 0 {
        sy += sh;
        sh = -sh;
    }
    if sw * sh * 4 > (1 << 31) {
        return agent.range_error("Out of memory at ImageData creation");
    }
    let mut out = vec![0u8; (sw * sh * 4) as usize];
    if let Some(p) = &canvas_mut(agent, n).pixmap {
        let (pw, ph) = (i64::from(p.width()), i64::from(p.height()));
        for y in 0..sh {
            let py = sy + y;
            if py < 0 || py >= ph {
                continue;
            }
            for x in 0..sw {
                let px = sx + x;
                if px < 0 || px >= pw {
                    continue;
                }
                if let Some(c) = p.pixel(px as u32, py as u32) {
                    let c = c.demultiply();
                    let i = ((y * sw + x) * 4) as usize;
                    out[i] = c.red();
                    out[i + 1] = c.green();
                    out[i + 2] = c.blue();
                    out[i + 3] = c.alpha();
                }
            }
        }
    }
    new_image_data(agent, sw as u32, sh as u32, out)
}

/// `putImageData(imagedata, dx, dy[, dirtyX, dirtyY, dirtyWidth, dirtyHeight])`: escribe
/// los píxeles tal cual (sin transformación, alfa global, composición ni recorte).
fn put_image_data(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_canvas(agent, a)?;
    let (info, data) = image_data_info(agent, &a.arg(0))?;
    let dx = i64::from(webidl::to_long_enforce_range(agent, &a.arg(1))?);
    let dy = i64::from(webidl::to_long_enforce_range(agent, &a.arg(2))?);
    let (mut rx, mut ry, mut rw, mut rh) =
        (0i64, 0i64, i64::from(info.width), i64::from(info.height));
    if a.args.len() >= 7 {
        rx = i64::from(webidl::to_long_enforce_range(agent, &a.arg(3))?);
        ry = i64::from(webidl::to_long_enforce_range(agent, &a.arg(4))?);
        rw = i64::from(webidl::to_long_enforce_range(agent, &a.arg(5))?);
        rh = i64::from(webidl::to_long_enforce_range(agent, &a.arg(6))?);
        if rw < 0 {
            rx += rw;
            rw = -rw;
        }
        if rh < 0 {
            ry += rh;
            rh = -rh;
        }
        if rx < 0 {
            rw += rx;
            rx = 0;
        }
        if ry < 0 {
            rh += ry;
            ry = 0;
        }
        rw = rw.min(i64::from(info.width) - rx);
        rh = rh.min(i64::from(info.height) - ry);
    }
    let Some(bytes) = clamped_bytes(agent, &data) else {
        return throw_dom(agent, DomException::InvalidStateError);
    };
    let c = canvas_mut(agent, n);
    if let Some(p) = c.pixmap.as_mut() {
        let (pw, ph) = (i64::from(p.width()), i64::from(p.height()));
        let pixels = p.pixels_mut();
        for y in ry..ry + rh.max(0) {
            for x in rx..rx + rw.max(0) {
                let (tx, ty) = (dx + x, dy + y);
                if tx < 0 || ty < 0 || tx >= pw || ty >= ph {
                    continue;
                }
                let i = ((y * i64::from(info.width) + x) * 4) as usize;
                let Some(px) = bytes.get(i..i + 4) else {
                    continue;
                };
                let c = tiny_skia::ColorU8::from_rgba(px[0], px[1], px[2], px[3]).premultiply();
                pixels[(ty * pw + tx) as usize] = c;
            }
        }
    }
    touched(agent, n);
    Ok(Value::Undefined)
}
