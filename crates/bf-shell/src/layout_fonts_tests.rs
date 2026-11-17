//! E3: la maqueta que ven los scripts mide con las fuentes web del documento, como la que se
//! pinta.
//!
//! El proveedor de maquetas del CSSOM (`ShellLayout`) medía solo con las fuentes del sistema.
//! Tras un cambio del DOM, `getBoundingClientRect` y `offsetWidth` de un texto con fuente web
//! daban otra medida que la caja pintada. `ResizeObserver` (Resize Observer §3.4.5) recogía
//! primero con la maqueta del pintado y, después de sus callbacks, con la del CSSOM: el texto
//! parecía cambiar de tamaño a una profundidad ya entregada y salía "ResizeObserver loop
//! completed with undelivered notifications" (Apple, DuckDuckGo).

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bf_layout::Size;
use bf_net::NetLoader;

use crate::load::CachedLoader;
use crate::script::{ConsoleEntry, Engine, PageEnv};

/// Ahem: cada glifo mide 1em, así que "XXXX" a 20 px mide 80 px; con la de respaldo, otra cosa.
const PAGE: &str = "<!doctype html><style>\
    @font-face { font-family: Web; src: url(/web.ttf) format('truetype') }\
    body { margin: 0 } span { font: 20px Web, serif; display: inline-block }</style>\
    <p><span id=t>XXXX</span></p><p id=o></p>\
    <script>\
    let n = 0;\
    new ResizeObserver((es) => {\
      for (const e of es) console.log('ro ' + e.contentRect.width);\
      if (n++ == 0) {\
        o.textContent = 'cambio';\
        console.log('rect ' + t.getBoundingClientRect().width + ' offset ' + t.offsetWidth);\
      }\
    }).observe(t);\
    </script>";

/// `/web.ttf`, Ahem; cualquier otra ruta, la página.
fn server(font: Vec<u8>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let font = Arc::new(font);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let font = font.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                let _ = r.read_line(&mut line);
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                        break;
                    }
                }
                let (ty, body): (&str, &[u8]) = if line.contains(" /web.ttf ") {
                    ("font/ttf", &font)
                } else {
                    ("text/html; charset=utf-8", PAGE.as_bytes())
                };
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {ty}\r\nCache-Control: no-store\r\n\
                     Connection: close\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body);
            });
        }
    });
    port
}

#[test]
fn script_layout_measures_with_the_document_web_fonts() {
    let ahem =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/wpt/fonts/Ahem.ttf");
    let Ok(font) = std::fs::read(&ahem) else {
        eprintln!("aviso: sin Ahem ({}), test omitido", ahem.display());
        return;
    };
    let port = server(font);
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
    let url = bf_infra::Url::parse(&format!("http://127.0.0.1:{port}/"), None).unwrap();
    let loader = Arc::new(CachedLoader::new(net));
    let doc = engine.parse(PAGE, &url, loader.clone(), &env);
    engine.run_due_tasks();

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
    // Se pinta hasta que el texto sale con la fuente web (la carga puede ir en segundo plano).
    let start = Instant::now();
    let page = loop {
        let generation = bf_font::web_fonts_generation();
        let page = bf_paint::prepare_page(&doc, &ctx, viewport);
        let dump = bf_layout::dump(&doc, &page.tree);
        if dump.contains("Text \"XXXX\" 0,") && dump.contains(" 80×20") {
            break page;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "{dump}");
        bf_font::wait_web_fonts_change(generation, Duration::from_millis(200));
    };
    engine.set_layout(&page, doc.version(), (0.0, 0.0), 1.0);
    engine.resources_loaded();
    engine.run_due_tasks();

    let log = log.borrow();
    // La caja pintada mide 80 px; la geometría que ve el script tras cambiar el DOM (maqueta
    // propia del CSSOM), también.
    assert!(log.iter().any(|l| l == "ro 80"), "{log:?}");
    assert!(log.iter().any(|l| l == "rect 80 offset 80"), "{log:?}");
    // Y `ResizeObserver` no ve un cambio de tamaño que no hubo.
    assert!(
        !log.iter().any(|l| l.contains("ResizeObserver loop")),
        "{log:?}"
    );
}
