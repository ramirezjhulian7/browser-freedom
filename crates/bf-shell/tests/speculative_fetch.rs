//! La descarga especulativa (HTML §13.2.8, `preload::speculative_fetch`): con la respuesta
//! del documento, sus `<script src>` se piden en paralelo mientras el parser se detiene en el
//! primero. Antes se pedían de uno en uno, cada uno al llegar el parser a él. Cada script se
//! pide una sola vez y corre en orden.

#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const PAGE: &str = "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>t</title>\
    <script src=/s1.js></script><script src=/s2.js></script>\
    <script src=/s3.js></script><script src=/s4.js></script>\
    </head><body><p>hola</p></body></html>";

/// Lo que vio el servidor: peticiones por camino y el máximo de peticiones a la vez.
#[derive(Default)]
struct Seen {
    requests: std::sync::Mutex<Vec<String>>,
    now: AtomicUsize,
    max: AtomicUsize,
}

/// Un servidor HTTP/1.1 que atiende cada conexión en su hilo; los scripts tardan 400 ms.
fn server() -> (u16, Arc<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Seen::default());
    let s = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let s = s.clone();
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
                s.requests.lock().unwrap().push(path.clone());
                let now = s.now.fetch_add(1, Ordering::SeqCst) + 1;
                s.max.fetch_max(now, Ordering::SeqCst);
                let (ct, body) = match path.strip_prefix('/').and_then(|p| p.strip_suffix(".js")) {
                    Some(n) => {
                        std::thread::sleep(Duration::from_millis(400));
                        ("text/javascript", format!("document.title += '|{n}';"))
                    }
                    None => ("text/html", PAGE.to_owned()),
                };
                s.now.fetch_sub(1, Ordering::SeqCst);
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nCache-Control: no-store\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                );
            });
        }
    });
    (port, seen)
}

#[test]
fn parser_blocking_scripts_are_fetched_in_parallel() {
    let dir =
        std::env::temp_dir().join(format!("freedom-test-especulativa-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (port, seen) = server();
    // Un `HOME` propio: caché de disco y cookies vacías, sin tocar las del usuario.
    let out = Command::new(env!("CARGO_BIN_EXE_freedom"))
        .env("HOME", &dir)
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .arg("session")
        .arg(format!("http://127.0.0.1:{port}/"))
        .args(["wait", "-o"])
        .arg(dir.join("paso"))
        .output()
        .expect("se pudo ejecutar freedom");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    let last = stdout.lines().rfind(|l| l.contains(" url=")).unwrap_or("");
    // Los cuatro corrieron, en orden de documento.
    assert!(last.contains("título=\"t|s1|s2|s3|s4\""), "{last}");
    // Cada uno se pidió una vez (el parser se une a la petición especulativa)…
    let mut requests = seen.requests.lock().unwrap().clone();
    requests.sort();
    assert_eq!(requests, ["/", "/s1.js", "/s2.js", "/s3.js", "/s4.js"]);
    // …y no de uno en uno.
    assert!(seen.max.load(Ordering::SeqCst) >= 2, "{requests:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
