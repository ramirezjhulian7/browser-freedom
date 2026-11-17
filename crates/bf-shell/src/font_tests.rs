//! E2: las fuentes web no congelan la pestaña (CSS Fonts 4 §4.9, CSS Font Loading 3 §2.2 y
//! §4.2). Un servidor local tarda 2 s en servir cada fuente:
//! - la página se pinta enseguida con la fuente de respaldo (ni la cascada ni la maqueta
//!   esperan a la red);
//! - cuando llega la fuente, el documento queda marcado para volver a pintar y la maqueta
//!   siguiente la usa;
//! - `FontFace.load()` descarga en segundo plano: el `load` del documento no la espera, su
//!   promesa se resuelve después y `document.fonts.ready`, después de ella.
//!
//! E2: lo mismo con una fuente WOFF 1.0 (`format('woff')`): la primera pintura usa la de
//! respaldo y la siguiente, la fuente ya descomprimida.

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bf_layout::Size;
use bf_net::NetLoader;

use crate::load::{LoadContext, load_with};
use crate::script::{Engine, PageEnv};
use crate::theme::Theme;

const PAGE: &str = "<!doctype html><style>\
    @font-face { font-family: Lenta; src: url(/lenta.FORMATO) format('FORMATO') }\
    body { margin: 0 } p { font: 16px Lenta, serif }</style>\
    <p><span id=t>XXXX</span></p>\
    <script>\
    addEventListener('load', () => console.log('load'));\
    const f = new FontFace('Otra', 'url(/otra.ttf)');\
    document.fonts.add(f);\
    f.load().then(() => console.log('otra-ok'), (e) => console.log('otra-' + e.name));\
    document.fonts.ready.then(() => console.log('ready'));\
    </script>";

/// `/` la página; `/lenta.*` y `/otra.ttf`, Ahem al cabo de 2 s (`/lenta.woff`, en WOFF).
fn server(page: String, font: Vec<u8>, woff: Vec<u8>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (page, font, woff) = (Arc::new(page), Arc::new(font), Arc::new(woff));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let (page, font, woff) = (page.clone(), font.clone(), woff.clone());
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
                let path = line.split(' ').nth(1).unwrap_or("/").to_owned();
                let (ty, body): (&str, &[u8]) = if path == "/" {
                    ("text/html; charset=utf-8", page.as_bytes())
                } else if path.ends_with(".woff") {
                    std::thread::sleep(Duration::from_secs(2));
                    ("font/woff", &woff)
                } else {
                    std::thread::sleep(Duration::from_secs(2));
                    ("font/ttf", &font)
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

/// Un WOFF 1.0 con las tablas sin comprimir (WOFF §5: `compLength` igual a `origLength`).
fn woff_of(ttf: &[u8]) -> Vec<u8> {
    let be16 = |at: usize| usize::from(u16::from_be_bytes([ttf[at], ttf[at + 1]]));
    let be32 = |at: usize| u32::from_be_bytes([ttf[at], ttf[at + 1], ttf[at + 2], ttf[at + 3]]);
    let n = be16(4);
    let mut out = vec![0u8; 44 + 20 * n];
    out[0..4].copy_from_slice(b"wOFF");
    out[4..8].copy_from_slice(&ttf[0..4]);
    out[12..14].copy_from_slice(&(n as u16).to_be_bytes());
    out[16..20].copy_from_slice(&(ttf.len() as u32).to_be_bytes());
    for i in 0..n {
        let e = 12 + 16 * i;
        let (off, len) = (be32(e + 8) as usize, be32(e + 12));
        let at = out.len() as u32;
        let d = 44 + 20 * i;
        out[d..d + 4].copy_from_slice(&ttf[e..e + 4]);
        out[d + 4..d + 8].copy_from_slice(&at.to_be_bytes());
        out[d + 8..d + 12].copy_from_slice(&len.to_be_bytes());
        out[d + 12..d + 16].copy_from_slice(&len.to_be_bytes());
        out[d + 16..d + 20].copy_from_slice(&ttf[e + 4..e + 8]);
        out.extend_from_slice(&ttf[off..off + len as usize]);
        out.resize(out.len().div_ceil(4) * 4, 0);
    }
    let total = out.len() as u32;
    out[8..12].copy_from_slice(&total.to_be_bytes());
    out
}

#[test]
fn web_fonts_do_not_block_the_page() {
    web_fonts_arrive_later("truetype");
}

#[test]
fn woff_fonts_do_not_block_the_page() {
    web_fonts_arrive_later("woff");
}

fn web_fonts_arrive_later(format: &str) {
    let ahem =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/wpt/fonts/Ahem.ttf");
    let Ok(font) = std::fs::read(&ahem) else {
        eprintln!("aviso: sin Ahem ({}), test omitido", ahem.display());
        return;
    };
    let woff = woff_of(&font);
    let ext = if format == "woff" { "woff" } else { "ttf" };
    let page = PAGE
        .replace("lenta.FORMATO", &format!("lenta.{ext}"))
        .replace("FORMATO", format);
    let port = server(page, font, woff);
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
    // Como la ventana: red en hilos (con quien despierta el bucle) y proveedor de maquetas.
    let env = PageEnv {
        fonts: Some(ctx.fonts.clone()),
        net_waker: Some(Arc::new(|| {})),
        ..PageEnv::cli(net)
    };
    let start = Instant::now();
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
        bf_layout::dump(&doc, &page.tree)
    };
    // Ahem: cada glifo mide 1em, así que "XXXX" a 16 px mide 64 px; la de respaldo, otra cosa.
    let ahem_width = |dump: &str| dump.contains("Text \"XXXX\" 0,") && dump.contains(" 64×16");
    let first = paint(&mut engine);
    assert!(
        start.elapsed() < Duration::from_millis(1500),
        "la primera pintura esperó a la fuente: {:?}",
        start.elapsed()
    );
    assert!(!ahem_width(&first), "{first}");
    // Cuando llega la fuente, el documento se marca para volver a pintar.
    let mut dirty = false;
    while start.elapsed() < Duration::from_secs(15) {
        engine.run_due_tasks();
        if engine.take_dirty()
            && start.elapsed() >= Duration::from_millis(1900)
            && log.borrow().iter().any(|l| l == "ready")
        {
            dirty = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(dirty, "{:?}", log.borrow());
    let second = paint(&mut engine);
    assert!(ahem_width(&second), "{second}");
    // `load` no esperó a `FontFace.load()`; `ready`, sí.
    let log = log.borrow();
    let at = |s: &str| log.iter().position(|l| l == s).unwrap_or(usize::MAX);
    assert!(at("load") < at("otra-ok"), "{log:?}");
    assert!(at("otra-ok") < at("ready"), "{log:?}");
}
