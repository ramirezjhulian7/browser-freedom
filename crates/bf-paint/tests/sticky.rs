//! E4 (pedido de E12): `DisplayItem::PushSticky`. Una caja `position: sticky` cuyo scrollport
//! es el viewport se desplaza en el raster con el scroll de la página (CSS Position 3 §3.4),
//! sin rehacer la lista de visualización.

use std::sync::Arc;

use bf_font::FontDb;
use bf_html::{ParseOptions, parse_document};
use bf_infra::Url;
use bf_infra::encoding::Encoding;
use bf_layout::Size;
use bf_paint::{DisplayItem, PageContext, Pixmap, prepare_page_with_frames, render_scrolled};
use bf_style::values::color::Rgba;

const VIEWPORT: Size = Size {
    width: 100.0,
    height: 100.0,
};

fn page(html: &str) -> bf_paint::Page {
    page_and_doc(html).0
}

fn page_and_doc(html: &str) -> (bf_paint::Page, bf_dom::Document) {
    let (doc, _) = parse_document(html, &ParseOptions::default());
    let base = Url::parse("about:blank", None).unwrap();
    let ctx = PageContext {
        base: &base,
        encoding: Encoding::Utf8,
        loader: Arc::new(bf_infra::FileLoader::new()),
        fonts: Arc::new(FontDb::from_faces(Vec::new())),
        extra: &[],
        text_zoom: 1.0,
    };
    let page = prepare_page_with_frames(&doc, &ctx, VIEWPORT, &[], &[]);
    (page, doc)
}

fn px(p: &Pixmap, x: u32, y: u32) -> [u8; 3] {
    let c = p.pixel(x, y).expect("dentro del pixmap");
    [c.red(), c.green(), c.blue()]
}

const GREEN: [u8; 3] = [0, 128, 0];
const WHITE: [u8; 3] = [255, 255, 255];

/// Una barra pegada arriba en (0, 40) de 100×10, dentro de un bloque de 300px de alto.
const STICKY: &str = "<body style='margin:0'><div style='height:300px'>\
     <div style='height:40px'></div>\
     <div style='position:sticky;top:0;height:10px;background:rgb(0,128,0)'></div></div>\
     <div style='height:500px'></div>";

#[test]
fn sticky_follows_the_page_scroll_in_the_raster() {
    let p = page(STICKY);
    // La lista lleva el ítem con las restricciones de la caja.
    assert!(
        p.display_list
            .items
            .iter()
            .any(|i| matches!(i, DisplayItem::PushSticky { .. }))
    );
    // Sin scroll, en su sitio.
    let a = render_scrolled(&p.display_list, VIEWPORT, 1.0, Rgba::WHITE, (0.0, 0.0));
    assert_eq!(px(&a, 50, 45), GREEN);
    assert_eq!(px(&a, 50, 5), WHITE);
    // Con la misma lista y la página desplazada 100px, queda pegada arriba del viewport.
    let b = render_scrolled(&p.display_list, VIEWPORT, 1.0, Rgba::WHITE, (0.0, 100.0));
    assert_eq!(px(&b, 50, 5), GREEN);
    assert_eq!(px(&b, 50, 15), WHITE);
    // Al final de su containing block (300px) deja de seguir el scroll: con 295px de scroll
    // ocupa 290..300 de la página, es decir -5..5 del viewport (sin el límite, 0..10).
    let c = render_scrolled(&p.display_list, VIEWPORT, 1.0, Rgba::WHITE, (0.0, 295.0));
    assert_eq!(px(&c, 50, 2), GREEN);
    assert_eq!(px(&c, 50, 7), WHITE);
}

#[test]
fn sticky_inside_a_scroll_container_has_no_sticky_item() {
    // El scrollport es el del contenedor: lo resuelve la maqueta (E12), no el raster.
    let p = page(
        "<body style='margin:0'><div style='height:50px;overflow:auto'>\
         <div style='position:sticky;top:0;height:10px;background:rgb(0,128,0)'></div>\
         <div style='height:200px'></div></div>",
    );
    assert!(
        !p.display_list
            .items
            .iter()
            .any(|i| matches!(i, DisplayItem::PushSticky { .. }))
    );
}

fn applied(p: &bf_paint::Page) -> Vec<(f32, f32)> {
    p.display_list
        .items
        .iter()
        .filter_map(|i| match i {
            DisplayItem::PushSticky { applied, .. } => Some(*applied),
            _ => None,
        })
        .collect()
}

#[test]
fn page_scroll_with_sticky_items_does_not_rebuild_the_list() {
    let (mut p, doc) = page_and_doc(STICKY);
    assert_eq!(applied(&p), [(0.0, 0.0)]);
    // El árbol se pone al día (hit testing) y hay que volver a pintar...
    assert!(p.apply_scroll(&doc, &[], (0.0, 100.0)));
    assert_ne!(p.tree.scroll.sticky.values().next(), Some(&(0.0, 0.0)));
    // ...pero la lista es la misma: el raster resuelve el desplazamiento.
    assert_eq!(applied(&p), [(0.0, 0.0)]);
    let b = render_scrolled(&p.display_list, VIEWPORT, 1.0, Rgba::WHITE, (0.0, 100.0));
    assert_eq!(px(&b, 50, 5), GREEN);
    assert_eq!(px(&b, 50, 15), WHITE);
}
