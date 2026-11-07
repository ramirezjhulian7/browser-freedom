//! E4: el documento de un `<iframe>` se pinta dentro del content box de su caja (HTML §4.8.5:
//! el content box es el viewport del marco), recortado a ella y desplazado por su scroll.

use std::sync::Arc;

use bf_font::FontDb;
use bf_html::{ParseOptions, parse_document};
use bf_infra::Url;
use bf_infra::encoding::Encoding;
use bf_layout::Size;
use bf_paint::{FrameDoc, PageContext, Pixmap, prepare_page_with_frames, render};
use bf_style::values::color::Rgba;

const VIEWPORT: Size = Size {
    width: 300.0,
    height: 200.0,
};

fn paint(scroll: (f32, f32)) -> (Pixmap, bf_paint::Page) {
    let (doc, _) = parse_document(
        "<body style='margin:0'><iframe id=f style='margin:10px;border:5px solid black;\
         width:100px;height:50px;display:block'></iframe>",
        &ParseOptions::default(),
    );
    let node = doc.get_element_by_id(&"f".into()).expect("iframe");
    let (inner, _) = parse_document(
        "<body style='margin:0;background:rgb(0,128,0)'>\
         <div style='height:30px'></div><div style='height:200px;width:500px;background:rgb(0,0,255)'></div>",
        &ParseOptions::default(),
    );
    let base = Url::parse("about:blank", None).unwrap();
    let ctx = PageContext {
        base: &base,
        encoding: Encoding::Utf8,
        loader: Arc::new(bf_infra::FileLoader::new()),
        fonts: Arc::new(FontDb::from_faces(Vec::new())),
        extra: &[],
    };
    let frames = [FrameDoc {
        node,
        doc: inner,
        base: base.clone(),
        encoding: Encoding::Utf8,
        scroll,
        frames: Vec::new(),
    }];
    let page = prepare_page_with_frames(&doc, &ctx, VIEWPORT, &[], &frames);
    (render(&page.display_list, VIEWPORT, 1.0, Rgba::WHITE), page)
}

fn px(p: &Pixmap, x: u32, y: u32) -> [u8; 3] {
    let c = p.pixel(x, y).expect("dentro del pixmap");
    [c.red(), c.green(), c.blue()]
}

#[test]
fn frame_document_is_painted_inside_its_box() {
    let (p, page) = paint((0.0, 0.0));
    // Content box del iframe: (15, 15) 100×50.
    assert_eq!(page.frames.len(), 1);
    let r = page.frames[0].rect;
    assert_eq!((r.x, r.y, r.width, r.height), (15.0, 15.0, 100.0, 50.0));
    // El fondo del marco (verde) arriba, su bloque azul desde y = 30 del marco.
    assert_eq!(px(&p, 20, 20), [0, 128, 0]);
    assert_eq!(px(&p, 20, 50), [0, 0, 255]);
    // Recortado al content box: ni el borde (negro) ni afuera se pintan con el marco.
    assert_eq!(px(&p, 12, 30), [0, 0, 0]);
    assert_eq!(px(&p, 125, 30), [255, 255, 255]);
    assert_eq!(px(&p, 50, 75), [255, 255, 255]);
}

#[test]
fn frame_scroll_moves_its_content() {
    // Con el marco desplazado 30 px, el bloque azul llega arriba del todo.
    let (p, _) = paint((0.0, 30.0));
    assert_eq!(px(&p, 20, 17), [0, 0, 255]);
}
