//! E2: `IntersectionObserver` en la ventana. El motor no maqueta por su cuenta: los
//! observadores se calculan con la maqueta que le pasa la ventana ([`Engine::set_layout`]) y al
//! desplazar ([`Engine::update_view`]), que son su *update the rendering* (HTML §8.1.7.3,
//! paso 19; Intersection Observer §3.2.8).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use bf_layout::Size;
use bf_net::NetLoader;

use crate::load::CachedLoader;
use crate::script::{ConsoleEntry, Engine, PageEnv};

const PAGE: &str = "<!doctype html><body style='margin:0'>\
    <div id=cerca style='height:50px'></div>\
    <div style='height:5000px'></div>\
    <div id=lejos style='height:50px'></div>\
    <script>new IntersectionObserver((es) => { for (const e of es) \
    console.log(e.target.id + ' ' + e.isIntersecting); }).observe(cerca);\
    new IntersectionObserver((es) => { for (const e of es) \
    console.log(e.target.id + ' ' + e.isIntersecting); }).observe(lejos);</script>";

#[test]
fn intersection_entry_arrives_on_scroll() {
    let net = Arc::new(NetLoader::new());
    let fonts = bf_font::FontDb::system();
    let env = PageEnv {
        defer_load: true,
        fonts: Some(fonts.clone()),
        ..PageEnv::cli(net.clone())
    };
    let log = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();
    let mut engine = Engine::new(Box::new(move |e: ConsoleEntry| {
        sink.borrow_mut().push(e.text)
    }));
    let url = bf_infra::Url::parse("https://example.test/", None).unwrap();
    let loader = Arc::new(CachedLoader::new(net));
    let doc = engine.parse(PAGE, &url, loader.clone(), &env);
    engine.run_due_tasks();
    // Sin maqueta de la ventana todavía, nada (y el motor no maquetó por su cuenta).
    assert!(log.borrow().is_empty(), "{log:?}");

    let viewport = Size {
        width: 800.0,
        height: 600.0,
    };
    let ctx = bf_paint::PageContext {
        base: &url,
        encoding: bf_infra::encoding::Encoding::Utf8,
        loader,
        fonts,
        extra: &[],
        text_zoom: 1.0,
        device_pixel_ratio: 1.0,
    };
    let page = bf_paint::prepare_page(&doc, &ctx, viewport);
    engine.set_layout(&page, doc.version(), (0.0, 0.0), 1.0);
    engine.resources_loaded();
    engine.run_due_tasks();
    // La primera notificación de cada objetivo.
    assert_eq!(*log.borrow(), ["cerca true", "lejos false"]);

    // Desplazar sin volver a maquetar: la lejana entra en el viewport y la cercana sale.
    engine.update_view((0.0, 4800.0), 1.0);
    engine.run_due_tasks();
    assert_eq!(
        *log.borrow(),
        ["cerca true", "lejos false", "cerca false", "lejos true"]
    );
}
