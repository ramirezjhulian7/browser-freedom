//! Imágenes de un documento: se juntan las URLs de `<img>`, `<input type=image>`,
//! `background-image`, `mask-image` y `list-style-image`, se cargan en paralelo con el
//! `ResourceLoader` (una vez por URL) y se decodifican con `bf-image` (bitmaps) o `bf-svg`.
//! Carga síncrona: el layout espera a que terminen.
//!
//! Los SVG se rasterizan al tamaño en que se pintan ([`PageImages::image_at`], con caché); los
//! GIF y APNG animados guardan todos sus fotogramas y [`PageImages::frame_at`] da el visible en
//! un instante. Nada de esto programa trabajo: quien anima (el bucle de cuadros) pregunta.

mod svg;
pub(crate) use svg::apply_filter; // E7

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bf_dom::{Document, NodeId};
use bf_infra::{ResourceLoader, Url};
use bf_layout::{FontProvider, ImageSizes, ImageState, Size};
use bf_style::StyleMap;
use bf_style::properties::types::Image;
use tiny_skia::{IntSize, Pixmap};

pub use svg::{rasterize as rasterize_svg, rasterize_with as rasterize_svg_with};

/// Niveles de `<image>` dentro de `<image>` que se cargan y se pintan.
const SVG_IMAGE_DEPTH: u32 = 4;

/// El recurso de un elemento `image` de SVG, decodificado.
#[derive(Clone)]
pub enum SvgImage {
    Bitmap(Arc<Pixmap>),
    Svg(Arc<bf_svg::Svg>),
    /// El contenido de un `foreignObject`, pintado a [`SVG_OVERSAMPLE`] px por px CSS desde
    /// la esquina de su caja (puede ser más grande que ella: lo que desborda).
    Foreign(Arc<Pixmap>),
}

/// Los recursos de los elementos `image` de los SVG, por `href` tal como está escrito. Un
/// solo mapa por página: dos SVG con el mismo `href` relativo y distinta base comparten
/// entrada (raro; las `data:` son únicas por contenido).
#[derive(Clone, Default)]
pub struct SvgResources {
    map: HashMap<String, SvgImage>,
}

impl SvgResources {
    pub fn get(&self, href: &str) -> Option<&SvgImage> {
        self.map.get(href.trim())
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn insert(&mut self, href: String, image: SvgImage) {
        self.map.insert(href, image);
    }

    /// Carga los `image` de `svg` y los de los SVG que estos usen. `base`: la URL del
    /// documento SVG. Sin `loader` es el modo seguro de un SVG usado como imagen (`<img>`,
    /// fondos: SVG Integration §4, *secure static mode*): solo se cargan URLs `data:`.
    pub fn load(
        &mut self,
        svg: &bf_svg::Svg,
        base: Option<&Url>,
        loader: Option<&dyn ResourceLoader>,
    ) {
        self.load_depth(svg, base, loader, 0);
    }

    fn load_depth(
        &mut self,
        svg: &bf_svg::Svg,
        base: Option<&Url>,
        loader: Option<&dyn ResourceLoader>,
        depth: u32,
    ) {
        if depth >= SVG_IMAGE_DEPTH {
            return;
        }
        for href in svg.image_hrefs() {
            if self.map.contains_key(&href) {
                continue;
            }
            let Ok(url) = Url::parse(&href, base) else {
                continue;
            };
            let res = if url.scheme == "data" {
                bf_infra::resource::load_local(&url, &[])
            } else {
                loader.and_then(|l| l.load(&url))
            };
            let Some(res) = res else {
                continue;
            };
            let fragment = href.split_once('#').map(|(_, f)| f);
            match decode(&res.bytes, res.content_type.as_deref(), fragment) {
                Ok(Loaded::Bitmap(p)) => {
                    self.map.insert(href, SvgImage::Bitmap(Arc::new(p)));
                }
                Ok(Loaded::Animated(a)) => {
                    self.map
                        .insert(href, SvgImage::Bitmap(a.pixmaps[0].clone()));
                }
                Ok(Loaded::Svg(s, _)) => {
                    // Se registra antes de bajar: un ciclo de referencias se corta aquí.
                    self.map.insert(href, SvgImage::Svg(s.clone()));
                    self.load_depth(&s, Some(&url), loader, depth + 1);
                }
                Err(_) => {}
            }
        }
    }
}

/// Niveles de documentos externos (`use href="otro.svg#id"` y demás) que se cargan.
const SVG_EXTERNAL_DEPTH: u32 = 4;

/// Carga los documentos externos a los que se refiere `svg` (SVG 2 §5.6.2: `use`, pinturas,
/// marcadores, recortes, máscaras) y los une a él, con los suyos ya unidos. `base`: la URL del
/// documento. Sin `loader` (SVG como imagen, modo seguro) solo se cargan URLs `data:`.
pub fn load_svg_externals(
    svg: &mut bf_svg::Svg,
    base: Option<&Url>,
    loader: Option<&dyn ResourceLoader>,
) {
    load_externals_depth(svg, base, loader, 0);
}

fn load_externals_depth(
    svg: &mut bf_svg::Svg,
    base: Option<&Url>,
    loader: Option<&dyn ResourceLoader>,
    depth: u32,
) {
    if depth >= SVG_EXTERNAL_DEPTH {
        return;
    }
    for doc in svg.external_documents() {
        let Ok(url) = Url::parse(&doc, base) else {
            continue;
        };
        let res = if url.scheme == "data" {
            bf_infra::resource::load_local(&url, &[])
        } else {
            loader.and_then(|l| l.load(&url))
        };
        let Some(res) = res else {
            continue;
        };
        // Se parsea como XML sea cual sea su tipo y su raíz (como hacen los navegadores con
        // un HTML bien formado que contiene un `<svg>`).
        let Ok(mut ext) = bf_svg::Svg::parse_resource(&res.bytes) else {
            continue;
        };
        load_externals_depth(&mut ext, Some(&url), loader, depth + 1);
        svg.attach_external(&doc, &ext);
    }
}

/// Sobremuestreo de los SVG: se rasterizan al doble del tamaño CSS para que se vean nítidos
/// también con DPR 2 (el rasterizador reduce con filtro bilineal).
pub(crate) const SVG_OVERSAMPLE: f32 = 2.0;

/// Entradas máximas de la caché de rasterizados SVG.
const SVG_CACHE: usize = 256;

/// Una imagen animada: los fotogramas como bitmaps y sus tiempos.
pub struct Animation {
    /// Tiempos y disposición (las imágenes se movieron a `pixmaps`).
    pub timing: bf_image::ImageFrames,
    pub pixmaps: Vec<Arc<Pixmap>>,
}

impl Animation {
    fn new(mut frames: bf_image::ImageFrames) -> Option<Animation> {
        let mut pixmaps = Vec::with_capacity(frames.frames.len());
        for f in &mut frames.frames {
            let img = std::mem::replace(
                &mut f.image,
                bf_image::Image {
                    width: 0,
                    height: 0,
                    rgba: Vec::new(),
                },
            );
            pixmaps.push(Arc::new(to_pixmap(img)?));
        }
        Some(Animation {
            timing: frames,
            pixmaps,
        })
    }

    /// El fotograma visible `t` después de empezar la animación.
    pub fn frame_at(&self, t: Duration) -> Arc<Pixmap> {
        self.pixmaps[self.timing.index_at(t).min(self.pixmaps.len() - 1)].clone()
    }
}

/// Imágenes decodificadas del documento.
#[derive(Default)]
pub struct PageImages {
    /// Por URL (serializada): fondos, marcadores e `<img>`. Los SVG, rasterizados a su tamaño
    /// por defecto; las animadas, su primer fotograma.
    pub by_url: HashMap<String, Arc<Pixmap>>,
    /// Documentos SVG por URL.
    pub svgs: HashMap<String, Arc<bf_svg::Svg>>,
    /// GIF/APNG con más de un fotograma, por URL.
    pub animations: HashMap<String, Arc<Animation>>,
    /// Recursos de los `image` de los SVG de la página.
    pub svg_resources: SvgResources,
    /// Rasterizados de SVG por (URL, ancho, alto) en píxeles.
    svg_cache: Mutex<HashMap<(String, u32, u32), Arc<Pixmap>>>,
    /// URL del contenido de cada `<img>`/`<input type=image>`: la fuente elegida por
    /// `srcset`/`<picture>` ya resuelta (lo que `currentSrc` devuelve, HTML §4.8.4.3).
    pub element_url: HashMap<NodeId, String>,
    /// Densidad de la fuente elegida de cada `<img>` cuando no es 1 (`2x`, o `w` con `sizes`):
    /// su tamaño natural es el de los píxeles dividido por ella (HTML §4.8.4.4).
    pub element_density: HashMap<NodeId, f32>,
    /// Estado para el layout.
    pub sizes: ImageSizes,
    /// URLs que fallaron y por qué.
    pub errors: Vec<(String, String)>,
    /// Contadores de decodificación (formatos sin soporte, fallos).
    pub stats: bf_image::DecodeStats,
    /// Imágenes que no se pudieron descargar.
    pub fetch_failed: usize,
    /// E2: densidad de la pantalla con la que se eligieron las fuentes (`srcset`,
    /// `image-set()`); 0 = sin fijar (1).
    pub device_pixel_ratio: f32,
}

impl PageImages {
    pub fn for_node(&self, node: NodeId) -> Option<&Arc<Pixmap>> {
        self.by_url.get(self.element_url.get(&node)?)
    }

    pub fn for_url(&self, url: &str) -> Option<&Arc<Pixmap>> {
        self.by_url.get(url)
    }

    /// La imagen de `url` para pintarla en `width`×`height` px CSS: los SVG se rasterizan a
    /// ese tamaño (con su texto, si hay `fonts`); los bitmaps se devuelven tal cual.
    pub fn image_at(
        &self,
        url: &str,
        width: f32,
        height: f32,
        fonts: Option<&dyn FontProvider>,
    ) -> Option<Arc<Pixmap>> {
        let Some(svg) = self.svgs.get(url) else {
            return self.by_url.get(url).cloned();
        };
        if !(width > 0.0 && height > 0.0 && width.is_finite() && height.is_finite()) {
            return None;
        }
        let pw = (width * SVG_OVERSAMPLE).ceil().max(1.0) as u32;
        let ph = (height * SVG_OVERSAMPLE).ceil().max(1.0) as u32;
        let key = (url.to_owned(), pw, ph);
        if let Some(p) = self
            .svg_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        {
            return Some(p.clone());
        }
        let pixmap = Arc::new(svg::rasterize_with(
            &svg.tree(width, height),
            pw,
            ph,
            fonts,
            &self.svg_resources,
        )?);
        let mut cache = self.svg_cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= SVG_CACHE {
            cache.clear();
        }
        cache.insert(key, pixmap.clone());
        Some(pixmap)
    }

    /// E2: la imagen que se usa de `img` y su resolución (dppx): la opción de un
    /// `image-set()` que se elige con la densidad de la pantalla y el `type` (CSS Images 4
    /// §2.4, con [`bf_image::source::choose`]); cualquier otra, ella misma a `1x`. `None` para
    /// `none` o un `image-set()` sin opción usable.
    pub fn chosen<'i>(&self, img: &'i Image) -> Option<(&'i Image, f32)> {
        match img {
            Image::None => None,
            Image::Set(options) => {
                let set = options
                    .iter()
                    .enumerate()
                    // Una resolución de 0 (o un `calc()` negativo, recortado a 0) daría un
                    // tamaño natural infinito: la opción no se usa.
                    .filter(|(_, o)| {
                        o.resolution > 0.0
                            && o.mime
                                .as_deref()
                                .is_none_or(bf_image::source::supports_type)
                    })
                    .map(|(i, o)| bf_image::source::ImageSource {
                        url: i.to_string(),
                        density: f64::from(o.resolution),
                    })
                    .collect();
                let dpr = if self.device_pixel_ratio > 0.0 {
                    self.device_pixel_ratio
                } else {
                    1.0
                };
                let pick = bf_image::source::choose(set, f64::from(dpr))?;
                let o = options.get(pick.url.parse::<usize>().ok()?)?;
                Some((&o.image, o.resolution))
            }
            other => Some((other, 1.0)),
        }
    }

    /// La densidad de la fuente elegida para `node` (1 si no se eligió por densidad).
    pub fn density(&self, node: NodeId) -> f32 {
        self.element_density.get(&node).copied().unwrap_or(1.0)
    }

    /// [`PageImages::image_at`] para el contenido de un `<img>`.
    pub fn for_node_at(
        &self,
        node: NodeId,
        width: f32,
        height: f32,
        fonts: Option<&dyn FontProvider>,
    ) -> Option<Arc<Pixmap>> {
        self.image_at(self.element_url.get(&node)?, width, height, fonts)
    }

    /// Tamaño natural de la imagen de `url` para una capa de fondo o máscara con área de
    /// posicionamiento `area` (CSS Images 3 §5.3.2, *default sizing*): el de los bitmaps; en
    /// un SVG, lo que tenga de ancho, alto y proporción, completado contra `area`.
    pub fn natural_size(&self, url: &str, area: (f32, f32)) -> Option<(f32, f32)> {
        let Some(svg) = self.svgs.get(url) else {
            return self
                .by_url
                .get(url)
                .map(|p| (p.width() as f32, p.height() as f32));
        };
        let i = svg.intrinsic();
        let (aw, ah) = area;
        Some(match (i.width, i.height, i.ratio) {
            (Some(w), Some(h), _) => (w, h),
            (Some(w), None, Some(r)) => (w, w / r),
            (None, Some(h), Some(r)) => (h * r, h),
            (Some(w), None, None) => (w, ah),
            (None, Some(h), None) => (aw, h),
            (None, None, Some(r)) => {
                // `contain` en el área.
                if aw / ah.max(f32::EPSILON) > r {
                    (ah * r, ah)
                } else {
                    (aw, aw / r)
                }
            }
            (None, None, None) => (aw, ah),
        })
    }

    /// Las dimensiones naturales de la imagen de `url` (CSS Images 3 §5.1): ancho, alto y
    /// proporción, cada una opcional (un SVG puede no tener ninguna; un bitmap las tiene todas).
    pub fn natural_dimensions(&self, url: &str) -> Option<(Option<f32>, Option<f32>, Option<f32>)> {
        if let Some(svg) = self.svgs.get(url) {
            let i = svg.intrinsic();
            return Some((i.width, i.height, i.ratio));
        }
        let p = self.by_url.get(url)?;
        let (w, h) = (p.width() as f32, p.height() as f32);
        Some((Some(w), Some(h), Some(w / h)))
    }

    /// El fotograma de `url` visible `t` después de empezar su animación (la imagen fija si
    /// no está animada).
    pub fn frame_at(&self, url: &str, t: Duration) -> Option<Arc<Pixmap>> {
        match self.animations.get(url) {
            Some(a) => Some(a.frame_at(t)),
            None => self.by_url.get(url).cloned(),
        }
    }

    /// ¿Hay alguna imagen animada? Sin ellas no hace falta ningún cuadro más.
    pub fn has_animations(&self) -> bool {
        !self.animations.is_empty()
    }

    /// Cuánto falta desde `t` para el próximo cambio de fotograma de cualquier animación
    /// (`None` si ninguna cambia más): el bucle de cuadros duerme hasta entonces.
    pub fn next_frame_change(&self, t: Duration) -> Option<Duration> {
        self.animations
            .values()
            .filter_map(|a| a.timing.next_change(t))
            .min()
    }

    /// Pone en `list` el fotograma visible en `t` de cada imagen animada (`<img>`, fondos y
    /// máscaras), sin rehacer layout ni lista: los bitmaps de una animación se reconocen por
    /// identidad (el builder pinta el primero, `by_url`). Devuelve si cambió alguno. Lo usa
    /// el bucle de cuadros de la ventana con `t` = tiempo desde que se mostró la página.
    pub fn animate(&self, list: &mut crate::DisplayList, t: Duration) -> bool {
        if self.animations.is_empty() {
            return false;
        }
        let mut changed = false;
        for item in &mut list.items {
            let image = match item {
                crate::DisplayItem::Image { image, .. }
                | crate::DisplayItem::BackgroundImage { image, .. } => image,
                _ => continue,
            };
            let Some(a) = self
                .animations
                .values()
                .find(|a| a.pixmaps.iter().any(|p| Arc::ptr_eq(p, image)))
            else {
                continue;
            };
            let frame = a.frame_at(t);
            if !Arc::ptr_eq(&frame, image) {
                *image = frame;
                changed = true;
            }
        }
        changed
    }

    /// El bitmap de un `<canvas>` como la imagen `canvas:<id>` del elemento (E1).
    pub fn insert_canvas(&mut self, node: NodeId, pixmap: Arc<Pixmap>) {
        let url = format!("canvas:{}", node.index());
        self.sizes.insert(
            node,
            ImageState::Loaded(Size {
                width: pixmap.width() as f32,
                height: pixmap.height() as f32,
            }),
        );
        self.by_url.insert(url.clone(), pixmap);
        self.element_url.insert(node, url);
    }
}

fn is_image_element(el: &bf_dom::Element) -> bool {
    el.is_html("img")
        || (el.is_html("input")
            && el
                .attr("type")
                .is_some_and(|t| t.eq_ignore_ascii_case("image")))
}

/// E12: la imagen que muestra un `<video>` (su `poster`, HTML §4.8.9) o un `<object>`/
/// `<embed>` cuyo recurso es una imagen (por su `type` o la extensión de la URL; HTML
/// §4.8.6 y §4.8.7). Su contenido se pinta como el de un `<img>` (con `object-fit`).
fn embedded_image(el: &bf_dom::Element) -> Option<&str> {
    let image_like = |src: &str| {
        let ty = el.attr("type").unwrap_or("").trim().to_ascii_lowercase();
        if !ty.is_empty() {
            return ty.starts_with("image/");
        }
        let path = src
            .split(['?', '#'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        [
            ".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".svg", ".ico", ".avif",
        ]
        .iter()
        .any(|e| path.ends_with(e))
    };
    let (attr, check) = if el.is_html("video") {
        ("poster", false)
    } else if el.is_html("object") {
        ("data", true)
    } else if el.is_html("embed") {
        ("src", true)
    } else {
        return None;
    };
    el.attr(attr)
        .map(str::trim)
        .filter(|s| !s.is_empty() && (!check || image_like(s)))
}

fn to_pixmap(img: bf_image::Image) -> Option<Pixmap> {
    let size = IntSize::from_wh(img.width, img.height)?;
    Pixmap::from_vec(img.rgba, size)
}

/// Una imagen cargada.
enum Loaded {
    Bitmap(Pixmap),
    Svg(Arc<bf_svg::Svg>, Pixmap),
    Animated(Arc<Animation>),
}

/// Decodifica los bytes de una imagen (bitmap, animada o SVG). `fragment`: el de la URL, que
/// en un SVG elige la vista (`#svgView(…)`, `#xywh=…`, el `id` de un `<view>`).
fn decode(
    bytes: &[u8],
    mime: Option<&str>,
    fragment: Option<&str>,
) -> Result<Loaded, bf_image::ImageError> {
    if bf_image::kind(bytes, mime) == Some("image/svg+xml") {
        let mut svg = bf_svg::Svg::parse_bytes(bytes)
            .map_err(|e| bf_image::ImageError::Decode(e.to_string()))?;
        if let Some(f) = fragment.filter(|f| !f.is_empty()) {
            let decoded = bf_url::percent::decode(f.as_bytes());
            svg = svg.with_fragment(&String::from_utf8_lossy(&decoded));
        }
        let (w, h) = svg.default_size();
        let pixmap = svg::rasterize(&svg.tree(w, h), w.ceil() as u32, h.ceil() as u32, None)
            .ok_or_else(|| bf_image::ImageError::Decode("SVG vacío".into()))?;
        return Ok(Loaded::Svg(Arc::new(svg), pixmap));
    }
    // E2: con la caché del proceso (`bf-web` decodifica la misma imagen para sus eventos y
    // `naturalWidth`): una sola decodificación; aquí se copian los píxeles.
    let frames = (*bf_image::decode_frames_shared(bytes, mime)?).clone();
    if frames.is_animated() {
        let anim = Animation::new(frames)
            .ok_or_else(|| bf_image::ImageError::Decode("imagen vacía".into()))?;
        return Ok(Loaded::Animated(Arc::new(anim)));
    }
    let image = frames
        .frames
        .into_iter()
        .next()
        .map(|f| f.image)
        .ok_or_else(|| bf_image::ImageError::Decode("imagen vacía".into()))?;
    to_pixmap(image)
        .map(Loaded::Bitmap)
        .ok_or_else(|| bf_image::ImageError::Decode("imagen vacía".into()))
}

/// Texto de los `<style>` HTML del documento (los SVG en línea también los usan).
fn document_css(doc: &Document) -> Vec<String> {
    let mut out = Vec::new();
    for n in doc.inclusive_descendants(doc.root()) {
        if doc.element(n).is_some_and(|e| e.is_html("style")) {
            let mut text = String::new();
            for c in doc.descendants(n) {
                if let bf_dom::NodeData::Text(t) = &doc.get(c).data {
                    text.push_str(&t.to_string_lossy());
                }
            }
            out.push(text);
        }
    }
    out
}

/// Los `<svg>` en línea de un documento HTML (los de más afuera) se pintan como imágenes del
/// elemento (`inline-svg:<id>`): el layout ya los trata como reemplazados.
fn inline_svgs(
    doc: &Document,
    styles: &StyleMap,
    images: &mut PageImages,
    base: &Url,
    loader: &dyn ResourceLoader,
) {
    let mut css: Option<Vec<String>> = None;
    for node in doc.inclusive_descendants(doc.root()) {
        let Some(el) = doc.element(node) else {
            continue;
        };
        if el.ns != bf_infra::Namespace::Svg || el.name != "svg" {
            continue;
        }
        let nested = doc
            .parent(node)
            .and_then(|p| doc.element(p))
            .is_some_and(|p| p.ns == bf_infra::Namespace::Svg);
        if nested {
            continue;
        }
        let css = css.get_or_insert_with(|| document_css(doc));
        let inherited = match styles.get(&(node, None)) {
            Some(s) => bf_svg::Inherited {
                color: s.color.rgba(),
                font_size: s.font_size_px(),
                font_family: s
                    .font_family
                    .0
                    .iter()
                    .map(|f| match f {
                        bf_style::properties::types::FamilyName::Generic(n)
                        | bf_style::properties::types::FamilyName::Named(n) => n.clone(),
                    })
                    .collect(),
            },
            None => bf_svg::Inherited::default(),
        };
        let result = bf_svg::Svg::from_element(doc, node, css, inherited);
        images.stats.record(
            &result
                .as_ref()
                .map_err(|e| bf_image::ImageError::Decode(e.to_string())),
        );
        let Ok(mut svg) = result else {
            continue;
        };
        // Parte del documento: sus referencias externas se cargan como sus recursos.
        load_svg_externals(&mut svg, Some(base), Some(loader));
        let (w, h) = svg.default_size();
        let Some(pixmap) = svg::rasterize(&svg.tree(w, h), w.ceil() as u32, h.ceil() as u32, None)
        else {
            continue;
        };
        let url = format!("inline-svg:{}", node.index());
        images.by_url.insert(url.clone(), Arc::new(pixmap));
        images.svgs.insert(url.clone(), Arc::new(svg));
        images.element_url.insert(node, url);
    }
}

/// Tamaño natural corregido por densidad (HTML §4.8.4.4, "density-corrected natural width
/// and height"): los píxeles divididos por la densidad de la fuente elegida. Una densidad
/// infinita (descriptor `w` con `sizes` 0) da 0×0; una nula o inválida deja los píxeles.
fn density_corrected(size: Size, density: f32) -> Size {
    if density.is_infinite() && density > 0.0 {
        return Size::default();
    }
    if !(density > 0.0 && density.is_finite()) || density == 1.0 {
        return size;
    }
    Size {
        width: size.width / density,
        height: size.height / density,
    }
}

/// Carga y decodifica las imágenes que usa el documento, con un entorno por defecto
/// (viewport 1280×720, densidad 1) para elegir entre `srcset` y `<picture>`.
pub fn load_images(
    doc: &Document,
    styles: &StyleMap,
    base: &Url,
    loader: &dyn ResourceLoader,
) -> PageImages {
    load_images_in(
        doc,
        styles,
        base,
        loader,
        &bf_css::media::MediaContext::default(),
    )
}

/// [`load_images`] en el entorno `media`: su viewport y su `device_pixel_ratio` eligen la
/// fuente de cada `<img>` (`srcset`, `sizes` y los `<source>` de `<picture>`, HTML §4.8.4.3).
pub fn load_images_in(
    doc: &Document,
    styles: &StyleMap,
    base: &Url,
    loader: &dyn ResourceLoader,
    media: &bf_css::media::MediaContext,
) -> PageImages {
    let mut images = PageImages {
        device_pixel_ratio: media.device_pixel_ratio as f32,
        ..PageImages::default()
    };
    let mut urls: Vec<String> = Vec::new();
    let want = |u: String, urls: &mut Vec<String>| {
        if !urls.contains(&u) {
            urls.push(u);
        }
    };
    let mut deferred = 0usize;
    for node in doc.inclusive_descendants(doc.root()) {
        let Some(el) = doc.element(node) else {
            continue;
        };
        if is_image_element(el) {
            // E2: `loading=lazy` (HTML §4.8.4.3.9): la imagen que el motor web tiene aplazada
            // (lejos del viewport) o aún en camino no se pide; se maqueta como imagen aún no
            // disponible (con el tamaño de sus atributos y su `aspect-ratio`). Cuando llega,
            // el motor quita la anotación y la deja en la caché del loader: se pinta sin
            // descargarla otra vez. Sin motor (un documento sin scripts) no hay anotaciones y
            // `lazy` no aplica: se piden todas.
            if doc.image_deferred(node) {
                deferred += 1;
                continue;
            }
            // `<img>`: la fuente que eligen `srcset`/`sizes`/`<picture>`; `<input type=image>`
            // no tiene `srcset`, solo `src`.
            let chosen = if el.is_html("img") {
                bf_image::source::select_image_source(doc, node, media)
                    .map(|s| (s.url, s.density as f32))
            } else {
                el.attr("src").map(|s| (s.to_owned(), 1.0))
            };
            match chosen.and_then(|(src, density)| {
                let src = src.trim_matches(|c: char| c.is_ascii_whitespace());
                if src.is_empty() {
                    return None;
                }
                Url::parse(src, Some(base)).ok().map(|u| (u, density))
            }) {
                Some((u, density)) => {
                    let href = u.href();
                    images.element_url.insert(node, href.clone());
                    if density != 1.0 {
                        images.element_density.insert(node, density);
                    }
                    want(href, &mut urls);
                }
                None => {
                    images.sizes.insert(node, ImageState::Broken);
                }
            }
        } else if let Some(u) = embedded_image(el).and_then(|s| Url::parse(s, Some(base)).ok()) {
            let href = u.href();
            images.element_url.insert(node, href.clone());
            want(href, &mut urls);
        }
    }
    inline_svgs(doc, styles, &mut images, base, loader);
    // Los SVG en línea son parte del documento: sus `image` se cargan como las del HTML.
    let inline: Vec<Arc<bf_svg::Svg>> = images.svgs.values().cloned().collect();
    for svg in inline {
        images.svg_resources.load(&svg, Some(base), Some(loader));
    }
    for style in styles.values() {
        let used = style
            .background_image
            .0
            .iter()
            .chain(&style.mask_image.0)
            .chain([&style.list_style_image, &style.border_image_source]);
        for img in used {
            // De un `image-set()`, solo la opción elegida.
            if let Some((Image::Url(u), _)) = images.chosen(img) {
                want(u.clone(), &mut urls);
            }
        }
    }
    // Para medir (`docs/rendimiento.md`): cuántas imágenes pide esta maqueta.
    if std::env::var_os("BF_TRACE").is_some() {
        eprintln!(
            "perf imágenes pedidas: {} ({deferred} <img> perezosas aplazadas)",
            urls.len()
        );
    }
    // Carga en paralelo: la red domina el tiempo.
    let results: Mutex<Vec<(String, Result<Loaded, String>)>> = Mutex::default();
    let stats: Mutex<(bf_image::DecodeStats, usize)> = Mutex::default();
    let next = Mutex::new(0usize);
    let workers = urls.len().clamp(1, 8);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = {
                        let mut n = next.lock().unwrap_or_else(|e| e.into_inner());
                        let i = *n;
                        *n += 1;
                        i
                    };
                    let Some(href) = urls.get(i) else {
                        break;
                    };
                    let result = (|| {
                        // Las URLs de atributos `style` llegan sin resolver: base del documento.
                        let url = Url::parse(href, Some(base)).map_err(|e| e.to_string())?;
                        let Some(res) = loader.load(&url) else {
                            stats.lock().unwrap_or_else(|e| e.into_inner()).1 += 1;
                            return Err("no se pudo cargar".to_owned());
                        };
                        let fragment = href.split_once('#').map(|(_, f)| f);
                        let decoded = decode(&res.bytes, res.content_type.as_deref(), fragment);
                        stats
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .0
                            .record(&decoded);
                        decoded.map_err(|e| e.to_string())
                    })();
                    results
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push((href.clone(), result));
                }
            });
        }
    });
    (images.stats, images.fetch_failed) = stats.into_inner().unwrap_or_else(|e| e.into_inner());
    for (href, result) in results.into_inner().unwrap_or_else(|e| e.into_inner()) {
        match result {
            Ok(Loaded::Bitmap(p)) => {
                images.by_url.insert(href, Arc::new(p));
            }
            Ok(Loaded::Svg(svg, p)) => {
                // SVG como imagen: modo seguro, sin recursos externos.
                let url = Url::parse(&href, Some(base)).ok();
                images.svg_resources.load(&svg, url.as_ref(), None);
                images.by_url.insert(href.clone(), Arc::new(p));
                images.svgs.insert(href, svg);
            }
            Ok(Loaded::Animated(a)) => {
                images.by_url.insert(href.clone(), a.pixmaps[0].clone());
                images.animations.insert(href, a);
            }
            Err(e) => images.errors.push((href, e)),
        }
    }
    let element_urls: Vec<(NodeId, String)> = images
        .element_url
        .iter()
        .map(|(n, u)| (*n, u.clone()))
        .collect();
    for (node, href) in element_urls {
        let state = match (images.svgs.get(&href), images.by_url.get(&href)) {
            // Sin ancho ni alto naturales (solo `viewBox`), el layout recibe solo la proporción
            // (CSS Images 3 §5.3.1; SVG 2 §8.2): el tamaño lo decide el contexto, no 300×150.
            (Some(svg), _) => match svg.intrinsic() {
                bf_svg::Intrinsic {
                    width: None,
                    height: None,
                    ratio: Some(r),
                } if r > 0.0 && r.is_finite() => ImageState::Ratio(r),
                _ => {
                    let (width, height) = svg.default_size();
                    ImageState::Loaded(density_corrected(
                        Size { width, height },
                        images.density(node),
                    ))
                }
            },
            (None, Some(p)) => ImageState::Loaded(density_corrected(
                Size {
                    width: p.width() as f32,
                    height: p.height() as f32,
                },
                images.density(node),
            )),
            (None, None) => ImageState::Broken,
        };
        images.sizes.insert(node, state);
    }
    images
}

/// Hoja para el documento de un `foreignObject`: el `body` es la caja del elemento, sin
/// márgenes ni fondo propio (lo de debajo es el SVG).
const FOREIGN_OBJECT_CSS: &str = "html, body { margin: 0 !important; padding: 0 !important; \
     border: 0 !important; background: none !important; transform: none !important; \
     width: auto !important; height: auto !important; }";

/// Lado máximo, en px CSS, de lo que se pinta del contenido de un `foreignObject`.
const FOREIGN_OBJECT_MAX: f32 = 2048.0;

/// Esquina inferior derecha de todo lo maquetado bajo `f` (coordenadas absolutas).
fn extent(f: &bf_layout::fragment::Fragment, right: &mut f32, bottom: &mut f32) {
    if f.rect.width > 0.0 && f.rect.height > 0.0 {
        *right = right.max(f.rect.x + f.rect.width);
        *bottom = bottom.max(f.rect.y + f.rect.height);
    }
    for c in &f.children {
        extent(c, right, bottom);
    }
}

/// Longitud en px de un atributo de geometría (`10`, `10px`); los porcentajes y las unidades
/// relativas no se resuelven aquí.
fn px_attr(el: &bf_dom::Element, name: &str) -> Option<f32> {
    let v = el.attr(name)?.trim();
    let v = v.strip_suffix("px").unwrap_or(v).trim();
    v.parse::<f32>().ok().filter(|v| v.is_finite())
}

/// Pinta el contenido HTML de los `foreignObject` de los SVG en línea de `doc` (SVG 2 §23.2,
/// básico): cada uno es un documento aparte (el mismo, con el `body` reducido a una copia de
/// los hijos del elemento) maquetado a `width`×`height` con fondo transparente, y queda en
/// `res` como `foreign-object:<nodo>`. Solo `width`/`height` en px.
pub(crate) fn foreign_objects(
    doc: &Document,
    ctx: &crate::PageContext<'_>,
    res: &mut SvgResources,
) {
    let targets: Vec<(NodeId, f32, f32)> = doc
        .inclusive_descendants(doc.root())
        .into_iter()
        .filter_map(|n| {
            let el = doc.element(n)?;
            if el.ns != bf_infra::Namespace::Svg || el.name != "foreignObject" {
                return None;
            }
            let (w, h) = (px_attr(el, "width")?, px_attr(el, "height")?);
            (w > 0.0 && h > 0.0).then_some((n, w, h))
        })
        .collect();
    if targets.is_empty() || doc.body().is_none() {
        return;
    }
    let sheet = bf_css::stylesheet::Stylesheet::parse(
        FOREIGN_OBJECT_CSS,
        bf_css::stylesheet::Origin::Author,
    );
    let mut extra: Vec<&bf_css::stylesheet::Stylesheet> = ctx.extra.to_vec();
    extra.push(&sheet);
    for (node, w, h) in targets {
        let mut child = doc.clone();
        let Some(body) = child.body() else {
            continue;
        };
        let copies: Vec<NodeId> = child
            .child_nodes(node)
            .into_iter()
            .map(|c| child.clone_node(c, true))
            .collect();
        for c in child.child_nodes(body) {
            let _ = child.remove_child(body, c);
        }
        for c in copies {
            let _ = child.append_child(body, c);
        }
        let size = Size {
            width: w,
            height: h,
        };
        let child_ctx = crate::PageContext {
            base: ctx.base,
            encoding: ctx.encoding,
            loader: ctx.loader.clone(),
            fonts: ctx.fonts.clone(),
            extra: &extra,
            text_zoom: ctx.text_zoom,
            device_pixel_ratio: ctx.device_pixel_ratio,
        };
        let page = crate::prepare_page(&child, &child_ctx, size);
        // Se pinta todo lo maquetado (con `overflow: visible` se ve lo que desborda); el
        // recorte a la caja lo hace el rasterizador del SVG.
        let (mut right, mut bottom) = (w, h);
        if let Some(root) = &page.tree.root {
            extent(root, &mut right, &mut bottom);
        }
        let canvas = Size {
            width: right.min(FOREIGN_OBJECT_MAX),
            height: bottom.min(FOREIGN_OBJECT_MAX),
        };
        let pixmap = crate::render(
            &page.display_list,
            canvas,
            SVG_OVERSAMPLE,
            bf_style::values::color::Rgba::TRANSPARENT,
        );
        res.insert(
            format!("{}{}", bf_svg::FOREIGN_OBJECT_SCHEME, node.index()),
            SvgImage::Foreign(Arc::new(pixmap)),
        );
    }
}
