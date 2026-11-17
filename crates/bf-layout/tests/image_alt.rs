//! E2: HTML §15.4.2: una imagen muestra su texto alternativo solo si no está disponible. Un SVG
//! cargado con `viewBox` y sin `width`/`height` (`ImageState::Ratio`: solo proporción) está
//! disponible: el logo y el lema de en.wikipedia salían como texto.

use bf_css::media::MediaContext;
use bf_css::selectors::MatchContext;
use bf_html::{ParseOptions, parse_document};
use bf_layout::{ImageSizes, ImageState, Size, TestFont, dump, layout_document_with_images};

fn layout(state: ImageState) -> String {
    let html = "<!doctype html><style>body{margin:0}</style>\
                <img alt=Wikipedia style='width:140px'>";
    let (doc, _) = parse_document(html, &ParseOptions::default());
    let img = doc
        .inclusive_descendants(doc.root())
        .into_iter()
        .find(|&n| doc.element(n).is_some_and(|e| e.is_html("img")))
        .expect("img");
    let ctx = MatchContext::for_document(&doc);
    let styles = bf_style::style_document(&doc, &[], &MediaContext::default(), &ctx);
    let mut images = ImageSizes::new();
    images.insert(img, state);
    let viewport = Size {
        width: 800.0,
        height: 600.0,
    };
    let tree = layout_document_with_images(&doc, &styles, viewport, &TestFont, &images);
    dump(&doc, &tree)
}

#[test]
fn an_image_with_only_a_ratio_is_available() {
    let d = layout(ImageState::Ratio(140.0 / 22.0));
    assert!(d.contains("Replaced <img> 0,0 140×22"), "{d}");
    assert!(!d.contains("Text \"Wikipedia\""), "{d}");
    // Rota: el texto alternativo.
    let d = layout(ImageState::Broken);
    assert!(d.contains("Text \"Wikipedia\""), "{d}");
}
