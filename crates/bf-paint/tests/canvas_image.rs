//! El bitmap de un `<canvas>` se pinta como la imagen del elemento (`PageImages::insert_canvas`,
//! E1): con su tamaño natural y en su content box.

use std::sync::Arc;

use bf_css::media::MediaContext;
use bf_css::selectors::MatchContext;
use bf_html::{ParseOptions, parse_document};
use bf_layout::{Size, TestFont, layout_document_with_images};
use bf_paint::{PageImages, Pixmap, build_display_list_with_images, render};
use bf_style::style_document;
use bf_style::values::color::Rgba;

#[test]
fn canvas_bitmap_is_painted() {
    let (doc, _) = parse_document(
        "<!DOCTYPE html><body style='margin:0'><canvas width=4 height=2 style='display:block'></canvas>",
        &ParseOptions::default(),
    );
    let canvas = doc
        .inclusive_descendants(doc.root())
        .into_iter()
        .find(|n| doc.element(*n).is_some_and(|e| e.is_html("canvas")))
        .expect("canvas");
    let viewport = Size {
        width: 20.0,
        height: 10.0,
    };
    let media = MediaContext {
        width: 20.0,
        height: 10.0,
        ..MediaContext::default()
    };
    let ctx = MatchContext::for_document(&doc);
    let styles = style_document(&doc, &[], &media, &ctx);
    let mut bitmap = Pixmap::new(4, 2).unwrap();
    bitmap.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
    let mut images = PageImages::default();
    images.insert_canvas(canvas, Arc::new(bitmap));
    let tree = layout_document_with_images(&doc, &styles, viewport, &TestFont, &images.sizes);
    let dl = build_display_list_with_images(&doc, &tree, &TestFont, &images);
    let out = render(&dl, viewport, 1.0, Rgba::WHITE);
    let px = |x, y| out.pixel(x, y).map(|c| (c.red(), c.green(), c.blue()));
    // El canvas mide lo que su bitmap (4×2), no los 300×150 por defecto.
    assert_eq!(px(1, 1), Some((255, 0, 0)));
    assert_eq!(px(5, 1), Some((255, 255, 255)));
    assert_eq!(px(1, 3), Some((255, 255, 255)));
}
