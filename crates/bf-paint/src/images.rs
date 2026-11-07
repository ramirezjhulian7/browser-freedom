//! Imágenes de un documento: se juntan las URLs de `<img>`, `<input type=image>`,
//! `background-image`, `mask-image` y `list-style-image`, se cargan en paralelo con el
//! `ResourceLoader` (una vez por URL) y se decodifican con `bf-image` (bitmaps) o `bf-svg`.
//! Carga síncrona: el layout espera a que terminen.
//!
//! Los SVG se rasterizan al tamaño en que se pintan ([`PageImages::image_at`], con caché); los
//! GIF y APNG animados guardan todos sus fotogramas y [`PageImages::frame_at`] da el visible en
//! un instante. Nada de esto programa trabajo: quien anima (el bucle de cuadros) pregunta.

mod svg;

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

/// Sobremuestreo de los SVG: se rasterizan al doble del tamaño CSS para que se vean nítidos
/// también con DPR 2 (el rasterizador reduce con filtro bilineal).
const SVG_OVERSAMPLE: f32 = 2.0;

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
    /// URL del contenido de cada `<img>`/`<input type=image>`.
    pub element_url: HashMap<NodeId, String>,
    /// Estado para el layout.
    pub sizes: ImageSizes,
    /// URLs que fallaron y por qué.
    pub errors: Vec<(String, String)>,
    /// Contadores de decodificación (formatos sin soporte, fallos).
    pub stats: bf_image::DecodeStats,
    /// Imágenes que no se pudieron descargar.
    pub fetch_failed: usize,
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
    let frames = bf_image::decode_frames(bytes, mime)?;
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
fn inline_svgs(doc: &Document, styles: &StyleMap, images: &mut PageImages) {
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
        let Ok(svg) = result else {
            continue;
        };
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

/// Carga y decodifica las imágenes que usa el documento.
pub fn load_images(
    doc: &Document,
    styles: &StyleMap,
    base: &Url,
    loader: &dyn ResourceLoader,
) -> PageImages {
    let mut images = PageImages::default();
    let mut urls: Vec<String> = Vec::new();
    let want = |u: String, urls: &mut Vec<String>| {
        if !urls.contains(&u) {
            urls.push(u);
        }
    };
    for node in doc.inclusive_descendants(doc.root()) {
        let Some(el) = doc.element(node) else {
            continue;
        };
        if is_image_element(el) {
            match el
                .attr("src")
                .filter(|s| !s.trim().is_empty())
                .and_then(|s| Url::parse(s.trim(), Some(base)).ok())
            {
                Some(u) => {
                    let href = u.href();
                    images.element_url.insert(node, href.clone());
                    want(href, &mut urls);
                }
                None => {
                    images.sizes.insert(node, ImageState::Broken);
                }
            }
        }
    }
    inline_svgs(doc, styles, &mut images);
    // Los SVG en línea son parte del documento: sus `image` se cargan como las del HTML.
    let inline: Vec<Arc<bf_svg::Svg>> = images.svgs.values().cloned().collect();
    for svg in inline {
        images.svg_resources.load(&svg, Some(base), Some(loader));
    }
    for style in styles.values() {
        for img in style.background_image.0.iter().chain(&style.mask_image.0) {
            if let Image::Url(u) = img {
                want(u.clone(), &mut urls);
            }
        }
        if let Image::Url(u) = &style.list_style_image {
            want(u.clone(), &mut urls);
        }
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
            (Some(svg), _) => {
                let (width, height) = svg.default_size();
                ImageState::Loaded(Size { width, height })
            }
            (None, Some(p)) => ImageState::Loaded(Size {
                width: p.width() as f32,
                height: p.height() as f32,
            }),
            (None, None) => ImageState::Broken,
        };
        images.sizes.insert(node, state);
    }
    images
}
