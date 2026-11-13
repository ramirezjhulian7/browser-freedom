//! E2: `loading=lazy` en el pintado de la ventana (HTML §4.8.4.3.9). Una imagen perezosa
//! lejos del viewport no genera ninguna petición, ni del motor web ni del pintado (que antes
//! pedía todos los `<img>` antes de la primera maqueta); al desplazar se pide una sola vez:
//! la pide el motor, la deja en la caché del loader de la ventana, y el pintado la toma de
//! ahí. El servidor responde con `Cache-Control: no-store`, para que la caché HTTP no tape
//! una segunda descarga.

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bf_dom::{Document, NodeId};
use bf_layout::Size;
use bf_net::NetLoader;

use crate::load::{LoadContext, load_with};
use crate::script::{Engine, PageEnv};
use crate::theme::Theme;

/// Un GIF de 1×1.
const GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff!\xf9\x04\x01\
                     \x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x01D\x00;";

const PAGE: &str = "<!doctype html><body style='margin:0'>\
    <img id=cerca loading=lazy src='/cerca.gif' onload=\"console.log('cerca-cargó')\">\
    <div style='height:10000px'></div>\
    <img id=lejos loading=lazy src='/lejos.gif' onload=\"console.log('lejos-cargó')\">";

/// Un servidor que responde `PAGE` en `/` y el GIF en las demás rutas; anota las rutas
/// pedidas.
fn server() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let paths = Arc::new(Mutex::new(Vec::new()));
    let seen = paths.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut r = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            let _ = r.read_line(&mut line);
            loop {
                let mut h = String::new();
                if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                    break;
                }
            }
            let path = line.split(' ').nth(1).unwrap_or("/").to_owned();
            let (ty, body) = if path == "/" {
                ("text/html; charset=utf-8", PAGE.as_bytes())
            } else {
                seen.lock().unwrap().push(path);
                ("image/gif", GIF)
            };
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {ty}\r\nCache-Control: no-store\r\n\
                 Connection: close\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body);
        }
    });
    (port, paths)
}

/// Corre las tareas del motor hasta que `done` aparezca en la consola (o 10 s).
fn run_until(engine: &mut Engine, log: &RefCell<Vec<String>>, done: &str) {
    let start = Instant::now();
    while !log.borrow().iter().any(|l| l == done) && start.elapsed() < Duration::from_secs(10) {
        engine.run_due_tasks();
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn by_id(doc: &Document, id: &str) -> NodeId {
    doc.inclusive_descendants(doc.root())
        .into_iter()
        .find(|&n| doc.element(n).is_some_and(|e| e.attr("id") == Some(id)))
        .unwrap()
}

#[test]
fn lazy_image_is_painted_without_a_second_request() {
    let (port, paths) = server();
    let net = Arc::new(NetLoader::new());
    let ctx = LoadContext {
        net: net.clone(),
        fonts: bf_font::FontDb::system(),
        viewport: Size {
            width: 800.0,
            height: 600.0,
        },
        recent: Vec::new(),
        theme: Theme::for_system(None),
        post: None,
    };
    let log = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();
    let mut engine = Engine::new(Box::new(move |e: crate::script::ConsoleEntry| {
        sink.borrow_mut().push(e.text)
    }));
    let env = PageEnv::cli(net);
    let url = bf_infra::Url::parse(&format!("http://127.0.0.1:{port}/"), None).unwrap();
    let loaded = load_with(&url, &ctx, Some((&mut engine, &env)));
    assert!(loaded.meta.error.is_none(), "{:?}", loaded.meta.error);
    run_until(&mut engine, &log, "cerca-cargó");

    // La maqueta de la ventana, como la de `window::worker`: con el loader del documento (el
    // mismo que usa el motor).
    let paint = |engine: &mut Engine| {
        let doc = engine.render_copy().unwrap();
        let pctx = bf_paint::PageContext {
            base: &loaded.base,
            encoding: loaded.encoding,
            loader: loaded.loader.clone(),
            fonts: ctx.fonts.clone(),
            extra: &[],
            text_zoom: 1.0,
            device_pixel_ratio: 1.0,
        };
        let page = bf_paint::prepare_page(&doc, &pctx, ctx.viewport);
        let painted = |id: &str| page.images.for_node(by_id(&doc, id)).is_some();
        let shown = (painted("cerca"), painted("lejos"));
        (page, doc.version(), shown)
    };

    // Primera pintura: la cercana sale de la caché (la trajo el motor); la lejana, ni pedida.
    let (page, version, shown) = paint(&mut engine);
    assert_eq!(shown, (true, false));
    assert_eq!(*paths.lock().unwrap(), ["/cerca.gif"]);
    engine.set_layout(&page, version, (0.0, 0.0), 1.0);
    for _ in 0..20 {
        engine.run_due_tasks();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(*paths.lock().unwrap(), ["/cerca.gif"]);

    // Desplazar: el motor la pide; cuando llega, el documento cambia (la ventana vuelve a
    // maquetar) y el pintado la pinta sin pedirla otra vez.
    engine.update_view((0.0, 9500.0), 1.0);
    run_until(&mut engine, &log, "lejos-cargó");
    assert!(log.borrow().iter().any(|l| l == "lejos-cargó"), "{log:?}");
    assert!(engine.take_dirty());
    let (_, _, shown) = paint(&mut engine);
    assert_eq!(shown, (true, true));
    assert_eq!(*paths.lock().unwrap(), ["/cerca.gif", "/lejos.gif"]);
}

/// Como la ventana (`window::worker`): el motor con proveedor de maquetas y `load` demorado
/// hasta la primera maqueta. La revisión de cercanía (HTML §4.8.4.3.9) corre a más tardar con
/// esa maqueta ([`Engine::set_layout`]); el pintado salta las perezosas que el motor aún no
/// tiene. Resultado: la cercana se pide (una vez) y sale en la pintura siguiente a su llegada;
/// la lejana no se pide.
#[test]
fn window_paints_near_lazy_image_after_its_layout() {
    let (port, paths) = server();
    let net = Arc::new(NetLoader::new());
    let ctx = LoadContext {
        net: net.clone(),
        fonts: bf_font::FontDb::system(),
        viewport: Size {
            width: 800.0,
            height: 600.0,
        },
        recent: Vec::new(),
        theme: Theme::for_system(None),
        post: None,
    };
    let log = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();
    let mut engine = Engine::new(Box::new(move |e: crate::script::ConsoleEntry| {
        sink.borrow_mut().push(e.text)
    }));
    let env = PageEnv {
        defer_load: true,
        fonts: Some(ctx.fonts.clone()),
        ..PageEnv::cli(net)
    };
    let url = bf_infra::Url::parse(&format!("http://127.0.0.1:{port}/"), None).unwrap();
    let loaded = load_with(&url, &ctx, Some((&mut engine, &env)));
    assert!(loaded.meta.error.is_none(), "{:?}", loaded.meta.error);
    let paint = |engine: &mut Engine| {
        let doc = engine.render_copy().unwrap();
        let pctx = bf_paint::PageContext {
            base: &loaded.base,
            encoding: loaded.encoding,
            loader: loaded.loader.clone(),
            fonts: ctx.fonts.clone(),
            extra: &[],
            text_zoom: 1.0,
            device_pixel_ratio: 1.0,
        };
        let page = bf_paint::prepare_page(&doc, &pctx, ctx.viewport);
        let painted = |id: &str| page.images.for_node(by_id(&doc, id)).is_some();
        let shown = (painted("cerca"), painted("lejos"));
        (page, doc.version(), shown)
    };
    // Primera maqueta: la lejana no sale (ni se pidió).
    let (page, version, shown) = paint(&mut engine);
    assert!(!shown.1);
    engine.set_layout(&page, version, (0.0, 0.0), 1.0);
    engine.resources_loaded();
    run_until(&mut engine, &log, "cerca-cargó");
    assert!(log.borrow().iter().any(|l| l == "cerca-cargó"), "{log:?}");
    // Llegó: el documento cambió y la maqueta siguiente la pinta, sin pedirla otra vez.
    let _ = engine.take_dirty();
    let (_, _, shown) = paint(&mut engine);
    assert_eq!(shown, (true, false));
    assert_eq!(*paths.lock().unwrap(), ["/cerca.gif"]);
}
