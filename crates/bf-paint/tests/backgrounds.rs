//! Fondos (E2, ronda 5): tamaño de las capas con imágenes sin dimensiones naturales completas
//! (CSS Backgrounds 3 §3.9 con el *default sizing* de CSS Images 3 §5.3) y propagación del
//! fondo del `body` al canvas (§2.11.2, CSS Contain 2).

use std::sync::Arc;

use bf_html::{ParseOptions, parse_document};
use bf_infra::{FileLoader, Url};
use bf_layout::Size;
use bf_paint::{PageContext, Pixmap};

fn render(html: &str) -> Pixmap {
    let (doc, _) = parse_document(html, &ParseOptions::default());
    let base = Url::parse("about:blank", None).unwrap();
    let ctx = PageContext {
        base: &base,
        encoding: bf_infra::encoding::Encoding::Utf8,
        loader: Arc::new(FileLoader::new()),
        fonts: Arc::new(bf_font::FontDb::from_faces(Vec::new())),
        extra: &[],
        text_zoom: 1.0,
        device_pixel_ratio: 1.0,
    };
    let viewport = Size {
        width: 100.0,
        height: 100.0,
    };
    bf_paint::render_document(&doc, &ctx, viewport, 1.0)
}

/// (r, g, b) sin premultiplicar.
fn px(p: &Pixmap, x: u32, y: u32) -> (u8, u8, u8) {
    let c = p.pixel(x, y).expect("dentro").demultiply();
    (c.red(), c.green(), c.blue())
}

/// Un SVG sin `width`, `height` ni `viewBox`, mitad de arriba lima y mitad de abajo azul.
const NO_SIZE: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg'%3E\
     %3Crect width='100%25' height='50%25' fill='lime'/%3E\
     %3Crect y='50%25' width='100%25' height='50%25' fill='blue'/%3E%3C/svg%3E";

/// Sin proporción ni ancho natural, `auto 20px` toma el ancho del área (§3.9): 80×20, y no
/// 16×20 (el alto escalado con la proporción del área, como antes).
#[test]
fn svg_without_size_auto_width() {
    let p = render(&format!(
        "<style>div {{ width:80px;height:100px;background:url(\"{NO_SIZE}\") no-repeat;\
         background-size:auto 20px }}</style><body style='margin:0'><div></div>"
    ));
    assert_eq!(px(&p, 79, 5), (0, 255, 0));
    assert_eq!(px(&p, 79, 15), (0, 0, 255));
    assert_eq!(px(&p, 40, 25), (255, 255, 255));
}

/// `contain` y `cover` sin proporción dan el área entera.
#[test]
fn svg_without_size_cover_and_contain() {
    for size in ["contain", "cover"] {
        let p = render(&format!(
            "<style>div {{ width:80px;height:60px;background:url(\"{NO_SIZE}\") no-repeat;\
             background-size:{size} }}</style><body style='margin:0'><div></div>"
        ));
        assert_eq!(px(&p, 79, 25), (0, 255, 0), "{size}");
        assert_eq!(px(&p, 79, 35), (0, 0, 255), "{size}");
    }
}

/// El fondo del `body` no se propaga al canvas si la raíz o el `body` tienen contención de
/// pintura; si se propaga, sus imágenes se colocan respecto de la raíz, no del `body`.
#[test]
fn body_background_propagation() {
    let p = render("<html style='contain:paint'><body style='background:lime;height:10px'>");
    assert_eq!(px(&p, 50, 50), (255, 255, 255));
    assert_eq!(px(&p, 50, 12), (0, 255, 0));
    let p = render(
        "<html style='margin:10px'><body style='margin:20px;background:linear-gradient(lime,lime) \
         no-repeat;background-size:10px 10px'>",
    );
    assert_eq!(px(&p, 12, 12), (0, 255, 0));
    assert_eq!(px(&p, 32, 32), (255, 255, 255));
}

/// Un SVG lima de 10×10 (con `viewBox`, para que escale al tamaño en que se pinta).
const LIME_10: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='10' height='10' \
     viewBox='0 0 10 10'%3E\
     %3Crect width='10' height='10' fill='lime'/%3E%3C/svg%3E";

/// `image-set()` (CSS Images 4 §2.4): la opción elegida pinta con su tamaño natural dividido
/// por su resolución (`0.5x` dobla); una con `type()` sin soporte se salta; una de `0x` no
/// pinta.
#[test]
fn image_set_resolution_and_type() {
    let page = |value: &str| {
        render(&format!(
            "<style>div {{ width:100px;height:100px;background-repeat:no-repeat;\
             background-image:{value} }}</style><body style='margin:0'><div></div>"
        ))
    };
    let p = page(&format!("image-set(url(\"{LIME_10}\") 0.5x)"));
    assert_eq!(px(&p, 18, 18), (0, 255, 0));
    assert_eq!(px(&p, 22, 22), (255, 255, 255));
    let p = page(&format!(
        "image-set(url(\"nada.avif\") type(\"image/avif\"), url(\"{LIME_10}\") 2x type(\"image/svg+xml\"))"
    ));
    assert_eq!(px(&p, 4, 4), (0, 255, 0));
    assert_eq!(px(&p, 6, 6), (255, 255, 255));
    let p = page(&format!("image-set(url(\"{LIME_10}\") calc(-1 * 1x))"));
    assert_eq!(px(&p, 4, 4), (255, 255, 255));
}
