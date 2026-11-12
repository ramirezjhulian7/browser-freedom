//! E4: `transform` 2D en la pintura (CSS Transforms 1 §6): la caja y su contenido se pintan
//! con la matriz alrededor de `transform-origin`, como stacking context.

use std::sync::Arc;

use bf_font::FontDb;
use bf_html::{ParseOptions, parse_document};
use bf_infra::Url;
use bf_infra::encoding::Encoding;
use bf_layout::Size;
use bf_paint::{DisplayItem, PageContext, Pixmap, prepare_page_with_frames, render};
use bf_style::values::color::Rgba;

const VIEWPORT: Size = Size {
    width: 200.0,
    height: 200.0,
};

fn paint(html: &str) -> (Pixmap, bf_paint::Page) {
    let (doc, _) = parse_document(html, &ParseOptions::default());
    let base = Url::parse("about:blank", None).unwrap();
    let ctx = PageContext {
        base: &base,
        encoding: Encoding::Utf8,
        loader: Arc::new(bf_infra::FileLoader::new()),
        fonts: Arc::new(FontDb::from_faces(Vec::new())),
        extra: &[],
    };
    let page = prepare_page_with_frames(&doc, &ctx, VIEWPORT, &[], &[]);
    (render(&page.display_list, VIEWPORT, 1.0, Rgba::WHITE), page)
}

fn px(p: &Pixmap, x: u32, y: u32) -> [u8; 3] {
    let c = p.pixel(x, y).expect("dentro del pixmap");
    [c.red(), c.green(), c.blue()]
}

const GREEN: [u8; 3] = [0, 128, 0];
const WHITE: [u8; 3] = [255, 255, 255];

#[test]
fn translate_moves_the_box() {
    let (p, page) = paint(
        "<body style='margin:0'><div style='width:50px;height:50px;background:rgb(0,128,0);\
         transform:translate(100px, 20px)'></div>",
    );
    assert!(
        page.display_list
            .items
            .iter()
            .any(|i| matches!(i, DisplayItem::PushMatrix(_)))
    );
    assert_eq!(px(&p, 25, 25), WHITE);
    assert_eq!(px(&p, 125, 45), GREEN);
    assert_eq!(px(&p, 160, 45), WHITE);
}

#[test]
fn rotate_turns_around_the_center() {
    // 100×20 en (0, 50) girado 90° alrededor de su centro (50, 60): queda 20×100 en (40, 10).
    let (p, _) = paint(
        "<body style='margin:0'><div style='margin-top:50px;width:100px;height:20px;\
         background:rgb(0,128,0);transform:rotate(90deg)'></div>",
    );
    assert_eq!(px(&p, 50, 30), GREEN);
    assert_eq!(px(&p, 50, 100), GREEN);
    assert_eq!(px(&p, 5, 60), WHITE);
    assert_eq!(px(&p, 95, 60), WHITE);
}

#[test]
fn scale_with_origin_and_individual_properties() {
    // `scale: 2` con origen arriba a la izquierda: 20×20 pasa a 40×40.
    let (p, _) = paint(
        "<body style='margin:0'><div style='width:20px;height:20px;background:rgb(0,128,0);\
         scale:2;transform-origin:0 0'></div>",
    );
    assert_eq!(px(&p, 35, 35), GREEN);
    assert_eq!(px(&p, 45, 45), WHITE);
}

#[test]
fn transformed_descendants_and_clip() {
    // El hijo se mueve con el padre; el `overflow: hidden` del abuelo recorta en coordenadas
    // de afuera.
    let (p, _) = paint(
        "<body style='margin:0'><div style='width:100px;height:100px;overflow:hidden'>\
         <div style='transform:translateX(60px)'><div style='width:80px;height:30px;\
         background:rgb(0,128,0)'></div></div></div>",
    );
    assert_eq!(px(&p, 30, 10), WHITE);
    assert_eq!(px(&p, 90, 10), GREEN);
    assert_eq!(px(&p, 120, 10), WHITE);
}

#[test]
fn fixed_inside_a_transform_moves_with_it() {
    // CSS Transforms 1 §3: la caja transformada es el bloque contenedor de los `fixed`, que se
    // pintan con su matriz y no respecto del viewport.
    let (p, _) = paint(
        "<body style='margin:0'><div style='transform:translateX(50px)'>\
         <div style='position:fixed;top:0;left:0;width:20px;height:20px;\
         background:rgb(0,128,0)'></div></div>",
    );
    assert_eq!(px(&p, 10, 10), WHITE);
    assert_eq!(px(&p, 60, 10), GREEN);
}
