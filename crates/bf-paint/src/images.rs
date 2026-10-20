//! Imágenes de un documento: se juntan las URLs de `<img>`, `<input type=image>`,
//! `background-image` y `list-style-image`, se cargan en paralelo con el `ResourceLoader` (una
//! vez por URL) y se decodifican con `bf-image`. Carga síncrona: el layout espera a que
//! terminen.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bf_dom::{Document, NodeId};
use bf_infra::{ResourceLoader, Url};
use bf_layout::{ImageSizes, ImageState, Size};
use bf_style::StyleMap;
use bf_style::properties::types::Image;
use tiny_skia::{IntSize, Pixmap};

/// Imágenes decodificadas del documento.
#[derive(Default)]
pub struct PageImages {
    /// Por URL (serializada): fondos, marcadores e `<img>`.
    pub by_url: HashMap<String, Arc<Pixmap>>,
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
    for style in styles.values() {
        for img in &style.background_image.0 {
            if let Image::Url(u) = img {
                want(u.clone(), &mut urls);
            }
        }
        if let Image::Url(u) = &style.list_style_image {
            want(u.clone(), &mut urls);
        }
    }
    // Carga en paralelo: la red domina el tiempo.
    let results: Mutex<Vec<(String, Result<Pixmap, String>)>> = Mutex::default();
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
                        let decoded = bf_image::decode(&res.bytes, res.content_type.as_deref());
                        stats
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .0
                            .record(&decoded);
                        let img = decoded.map_err(|e| e.to_string())?;
                        to_pixmap(img).ok_or_else(|| "imagen vacía".to_owned())
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
            Ok(p) => {
                images.by_url.insert(href, Arc::new(p));
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
        let state = match images.by_url.get(&href) {
            Some(p) => ImageState::Loaded(Size {
                width: p.width() as f32,
                height: p.height() as f32,
            }),
            None => ImageState::Broken,
        };
        images.sizes.insert(node, state);
    }
    images
}
