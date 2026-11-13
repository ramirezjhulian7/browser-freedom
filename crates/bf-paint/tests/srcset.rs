//! Imágenes adaptables (HTML §4.8.4.3, E2): qué fuente carga un `<img>` según `srcset`,
//! `sizes` y los `<source>` de `<picture>`, con la densidad de la página
//! (`PageContext::device_pixel_ratio`), y su tamaño natural corregido por densidad (§4.8.4.4).
//!
//! Imágenes de `crates/bf-image/tests/vectors/`: `bmp/icon_png.png` (32×32) hace de `a.png` y
//! `webp/ly_deltas.png` (64×64) de `b.png`, su versión `2x`.

use std::sync::Arc;

use bf_html::{ParseOptions, parse_document};
use bf_infra::{FileLoader, Url};
use bf_layout::Size;
use bf_layout::fragment::Fragment;
use bf_paint::{PageContext, prepare_page};

const A: &str = "bmp/icon_png.png";
const B: &str = "webp/ly_deltas.png";

/// Lo que carga y mide el primer `<img>` de `html`: (archivo cargado, ancho, alto en px CSS).
fn load(html: &str, dpr: f32) -> (String, f32, f32) {
    let (doc, _) = parse_document(html, &ParseOptions::default());
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../bf-image/tests/vectors/");
    let base = Url::from_file_path(&std::path::absolute(dir).unwrap()).unwrap();
    let ctx = PageContext {
        base: &base,
        encoding: bf_infra::encoding::Encoding::Utf8,
        loader: Arc::new(FileLoader::new()),
        fonts: Arc::new(bf_font::FontDb::from_faces(Vec::new())),
        extra: &[],
        text_zoom: 1.0,
        device_pixel_ratio: dpr,
    };
    let viewport = Size {
        width: 800.0,
        height: 600.0,
    };
    let page = prepare_page(&doc, &ctx, viewport);
    assert!(page.images.errors.is_empty(), "{:?}", page.images.errors);
    let img = doc
        .inclusive_descendants(doc.root())
        .into_iter()
        .find(|&n| doc.element(n).is_some_and(|e| e.is_html("img")))
        .expect("un <img>");
    let url = page.images.element_url.get(&img).expect("fuente elegida");
    let file = url.rsplit("/vectors/").next().unwrap().to_owned();
    fn find(f: &Fragment, node: bf_dom::NodeId) -> Option<&Fragment> {
        if f.node == Some(node) {
            return Some(f);
        }
        f.children.iter().find_map(|c| find(c, node))
    }
    let frag = find(page.tree.root.as_ref().unwrap(), img).expect("caja del <img>");
    // Redondeado: la densidad de un `w` (64 / 800) no es exacta en `f32`.
    let r = frag.content_rect();
    (file, r.width.round(), r.height.round())
}

#[test]
fn density_descriptors() {
    let html = format!("<img srcset='{A} 1x, {B} 2x'>");
    // Con DPR 2 se carga la `2x` y ocupa el tamaño de la `1x` (64 px / 2).
    assert_eq!(load(&html, 2.0), (B.to_owned(), 32.0, 32.0));
    assert_eq!(load(&html, 1.0), (A.to_owned(), 32.0, 32.0));
    // Sin candidato suficiente, el de más densidad; `src` cuenta como `1x`.
    assert_eq!(
        load(&format!("<img src='{A}' srcset='{B} 2x'>"), 3.0),
        (B.to_owned(), 32.0, 32.0)
    );
    assert_eq!(
        load(&format!("<img src='{A}' srcset='{B} 2x'>"), 1.0),
        (A.to_owned(), 32.0, 32.0)
    );
    // Solo `src`: igual que antes.
    assert_eq!(
        load(&format!("<img src='{B}'>"), 2.0),
        (B.to_owned(), 64.0, 64.0)
    );
}

#[test]
fn width_descriptors_with_sizes() {
    // Viewport de 800 px: `sizes` da 32 px, así que `A` es 1x (32w / 32) y `B` 2x (64w / 32).
    let html = format!("<img srcset='{A} 32w, {B} 64w' sizes='(max-width: 500px) 100vw, 32px'>");
    assert_eq!(load(&html, 1.0), (A.to_owned(), 32.0, 32.0));
    assert_eq!(load(&html, 2.0), (B.to_owned(), 32.0, 32.0));
    // `sizes` de 16 px: `A` ya es 2x y basta con DPR 2; se muestra a 16×16.
    let html = format!("<img srcset='{B} 64w, {A} 32w' sizes='calc(4vw - 16px)'>");
    assert_eq!(load(&html, 2.0), (A.to_owned(), 16.0, 16.0));
    // Sin `sizes`, `100vw`: 64w / 800 px es menos de 1x; se elige la de más densidad, que se
    // muestra a 800 px de ancho.
    let html = format!("<img srcset='{A} 32w, {B} 64w'>");
    assert_eq!(load(&html, 1.0), (B.to_owned(), 800.0, 800.0));
}

#[test]
fn picture_media_and_type() {
    // El `media` que no se cumple y el `type` sin soporte (AVIF) se saltan.
    let html = format!(
        "<picture>\
         <source media='(min-width: 2000px)' srcset='{A}'>\
         <source type='image/avif' srcset='nada.avif'>\
         <source type='image/png' media='(min-width: 600px)' srcset='{B} 2x, {A}'>\
         <img src='nada.png'></picture>"
    );
    assert_eq!(load(&html, 1.0), (A.to_owned(), 32.0, 32.0));
    assert_eq!(load(&html, 2.0), (B.to_owned(), 32.0, 32.0));
    // Ningún `<source>` sirve: el `src` del `<img>`.
    let html = format!(
        "<picture><source type='image/jxl' srcset='nada.jxl'>\
         <source media='print' srcset='nada.png'><img src='{A}'></picture>"
    );
    assert_eq!(load(&html, 1.0), (A.to_owned(), 32.0, 32.0));
    // WebP sí tiene soporte.
    let html = format!(
        "<picture><source type='image/webp' srcset='webp/ly_deltas.webp'>\
         <img src='{A}'></picture>"
    );
    assert_eq!(
        load(&html, 1.0),
        ("webp/ly_deltas.webp".to_owned(), 64.0, 64.0)
    );
    // Los `<source>` después del `<img>` no cuentan.
    let html = format!("<picture><img src='{A}'><source srcset='{B}'></picture>");
    assert_eq!(load(&html, 1.0), (A.to_owned(), 32.0, 32.0));
}
