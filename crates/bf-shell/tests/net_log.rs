//! `FREEDOM_NET_LOG=1` anota también las peticiones de los scripts (`XMLHttpRequest`,
//! `fetch()`), que van por `bf_net::streaming` y no pasaban por el registro, y las que fallan,
//! con el error y lo que tardaron. Sin ellas no se veía qué pedía el reto de Cloudflare.

#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::Command;

/// Un servidor HTTP/1.1 que responde `ok` a todo.
fn server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
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
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nAccess-Control-Allow-Origin: *\r\n\
                  Content-Length: 2\r\nConnection: close\r\n\r\nok",
            );
        }
    });
    port
}

/// Un puerto donde no escucha nadie.
fn closed_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

#[test]
fn script_requests_and_failures_are_logged() {
    let dir = std::env::temp_dir().join(format!("freedom-test-netlog-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (port, closed) = (server(), closed_port());
    let page = format!(
        "<!DOCTYPE html><meta charset=utf-8><title>t</title><script>\
         for (const u of ['http://127.0.0.1:{port}/datos', 'http://127.0.0.1:{closed}/nada']) {{\
           const x = new XMLHttpRequest(); x.open('GET', u);\
           x.onloadend = () => document.title += '|' + x.status; x.send(); }}\
         </script>"
    );
    std::fs::write(dir.join("doc.html"), page).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_freedom"))
        .env("FREEDOM_NET_LOG", "1")
        .env("HOME", &dir)
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .arg("session")
        .arg(dir.join("doc.html"))
        .args(["wait", "wait", "wait", "-o"])
        .arg(dir.join("paso"))
        .output()
        .expect("se pudo ejecutar freedom");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}{stderr}");
    // Las dos terminaron (200 y error de red, `status` 0)…
    let last = stdout.lines().rfind(|l| l.contains(" url=")).unwrap_or("");
    assert!(last.contains("|200") && last.contains("|0"), "{last}");
    // …y las dos están en el registro.
    let ok = format!("red: GET http://127.0.0.1:{port}/datos 200");
    assert!(stderr.contains(&ok), "{stderr}");
    let failed = format!("red: GET http://127.0.0.1:{closed}/nada falló tras");
    assert!(stderr.contains(&failed), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}
