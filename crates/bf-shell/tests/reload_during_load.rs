//! Un `location.reload()` mientras la primera carga de la pestaña todavía se parsea (lo hace el
//! `security.js` de Mercado Libre en la primera visita, tras poner una cookie) recarga y deja la
//! entrada del historial. Antes la pestaña quedaba sin entrada: `url=` vacía e
//! `historial=1/0`, con la página a la vista.

#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const PAGE: &str = "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>recarga</title>\
    <script>if (document.cookie.indexOf('v=1') < 0) { document.cookie = 'v=1'; \
    location.reload(); }</script></head><body><p>hola</p></body></html>";

/// Un servidor HTTP/1.1 que responde `PAGE` a todo y cuenta las peticiones.
fn server() -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut r = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut h = String::new();
                if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                    break;
                }
            }
            seen.fetch_add(1, Ordering::SeqCst);
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nCache-Control: no-store\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{PAGE}",
                    PAGE.len()
                )
                .as_bytes(),
            );
        }
    });
    (port, count)
}

#[test]
fn reload_while_loading_keeps_the_entry() {
    let dir = std::env::temp_dir().join(format!("freedom-test-recarga-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (port, count) = server();
    let url = format!("http://127.0.0.1:{port}/");
    // Un `HOME` propio: cookies y caché de disco vacías, sin tocar las del usuario.
    let out = Command::new(env!("CARGO_BIN_EXE_freedom"))
        .env("HOME", &dir)
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .arg("session")
        .arg(&url)
        .args(["wait", "wait", "-o"])
        .arg(dir.join("paso"))
        .output()
        .expect("se pudo ejecutar freedom");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    let last = stdout.lines().rfind(|l| l.contains(" url=")).unwrap_or("");
    assert!(last.contains(&format!(" url={url} ")), "{last}");
    assert!(last.contains("historial=1/1"), "{last}");
    assert!(last.contains("título=\"recarga\""), "{last}");
    // La carga y la recarga (con la cookie ya puesta, no vuelve a recargar).
    assert_eq!(count.load(Ordering::SeqCst), 2, "{stdout}");
    let _ = std::fs::remove_dir_all(&dir);
}
