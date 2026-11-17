//! E3: las sugerencias de recursos (HTML §4.6.7) de los `<link>` y de la cabecera `Link` de la
//! respuesta del documento (RFC 8288):
//! - `preconnect` abre una conexión con el origen sin pedir nada (el servidor B la recibe y no
//!   llega ninguna petición; el C, igual, desde un `<link>`);
//! - `dns-prefetch` a un origen que no responde no abre nada;
//! - `preload as=font` y `as=image` se piden una sola vez: la `@font-face` y el `<img>` que los
//!   usan después no los vuelven a descargar.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::{Arc, Mutex};

/// Lo que vio un servidor: conexiones y peticiones por ruta.
#[derive(Default)]
struct Seen {
    connections: usize,
    requests: HashMap<String, usize>,
}

/// Un servidor HTTP/1.1 con keep-alive. `respond` da cabeceras extra, tipo y cuerpo de cada
/// ruta.
fn server(
    respond: impl Fn(&str, u16) -> (String, &'static str, Vec<u8>) + Send + Sync + 'static,
) -> (u16, Arc<Mutex<Seen>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let (s, respond) = (seen.clone(), Arc::new(respond));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            s.lock().unwrap().connections += 1;
            let (s, respond) = (s.clone(), respond.clone());
            std::thread::spawn(move || {
                let mut r = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    loop {
                        let mut h = String::new();
                        if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                            break;
                        }
                    }
                    let path = line.split(' ').nth(1).unwrap_or("/").to_owned();
                    *s.lock().unwrap().requests.entry(path.clone()).or_default() += 1;
                    let (extra, ty, body) = respond(&path, port);
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {ty}\r\nCache-Control: no-store\r\n\
                         {extra}Content-Length: {}\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body);
                }
            });
        }
    });
    (port, seen)
}

/// Un PNG de 1×1 escrito a mano (rojo).
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
    0x44, 0xAE, 0x42, 0x60, 0x82,
];

#[test]
fn resource_hints_connect_early_and_preloads_are_downloaded_once() {
    let ahem =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/wpt/fonts/Ahem.ttf");
    let Ok(font) = std::fs::read(&ahem) else {
        eprintln!("aviso: sin Ahem ({}), test omitido", ahem.display());
        return;
    };
    let dir = std::env::temp_dir().join(format!("freedom-test-sugerencias-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // B y C: orígenes a los que solo se preconecta.
    let nothing = |_: &str, _: u16| (String::new(), "text/plain", b"nada".to_vec());
    let (b, seen_b) = server(nothing);
    let (c, seen_c) = server(nothing);
    // Un puerto sin nadie: el `dns-prefetch` no abre conexiones.
    let closed = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let page = format!(
        "<!doctype html><html><head><meta charset=utf-8><title>t</title>\
         <link rel=preconnect href=http://127.0.0.1:{c}>\
         <link rel=dns-prefetch href=http://127.0.0.1:{closed}>\
         <link rel=preload as=image href=/i.png>\
         <style>@font-face {{ font-family: W; src: url(/f.ttf) format('truetype') }}\
         p {{ font: 20px W, serif }}</style></head>\
         <body><p>XXXX</p><script>\
         setTimeout(() => {{ const i = new Image(); i.src = '/i.png'; \
           i.onload = () => document.title = 'img'; document.body.append(i); }}, 300);\
         </script></body></html>"
    );
    let (a, seen_a) = server(move |path, _| match path {
        "/f.ttf" => (String::new(), "font/ttf", font.clone()),
        "/i.png" => (String::new(), "image/png", PNG.to_vec()),
        _ => (
            format!(
                "Link: <http://127.0.0.1:{b}/>; rel=preconnect, </f.ttf>; rel=preload; as=font; \
                 crossorigin\r\n"
            ),
            "text/html; charset=utf-8",
            page.clone().into_bytes(),
        ),
    });
    let out = Command::new(env!("CARGO_BIN_EXE_freedom"))
        .env("HOME", &dir)
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("FREEDOM_NET_LOG", "1")
        .arg("session")
        .arg(format!("http://127.0.0.1:{a}/"))
        .args(std::iter::repeat_n("wait", 8))
        .arg("-o")
        .arg(dir.join("paso"))
        .output()
        .expect("se pudo ejecutar freedom");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}{stderr}");
    // El `<img>` del script cargó.
    let last = stdout.lines().rfind(|l| l.contains(" url=")).unwrap_or("");
    assert!(last.contains("título=\"img\""), "{stdout}{stderr}");
    // `preconnect` por la cabecera `Link` (B) y por el `<link>` (C): una conexión y ninguna
    // petición.
    for (name, seen) in [("B", &seen_b), ("C", &seen_c)] {
        let seen = seen.lock().unwrap();
        assert_eq!(seen.connections, 1, "{name}: {stderr}");
        assert!(seen.requests.is_empty(), "{name}: {:?}", seen.requests);
    }
    // `preload`: la fuente (por `Link`) y la imagen (por `<link>`), una vez cada una aunque
    // las usen después la `@font-face`, el `<img>` y el pintado.
    let seen = seen_a.lock().unwrap();
    assert_eq!(seen.requests.get("/f.ttf"), Some(&1), "{:?}", seen.requests);
    assert_eq!(
        seen.requests.get("/i.png"),
        Some(&1),
        "{:?} {}",
        seen.requests,
        stderr
            .lines()
            .filter(|l| l.contains("i.png"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(stderr.contains("preconexión"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}
