//! Un `<iframe>` lento no detiene a su página (HTML §7.4.2.2, *navigate*: el fetch del
//! navegable hijo corre "in parallel"; §4.8.5): la página pinta y atiende un clic mientras el
//! documento del marco, que el servidor tarda 2 s en dar, sigue cargando; el `load` del
//! `<iframe>` llega después. Antes, el hilo de la pestaña pedía y parseaba el marco de una vez
//! y el clic se atendía después del `load` del marco.

#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::Command;
use std::time::{Duration, Instant};

const PAGE: &str = "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>padre</title>\
    </head><body><p><a href=\"#\" onclick=\"document.title += '|clic'; return false\">pulsar</a></p>\
    <iframe src=\"/lento\" onload=\"document.title += '|marco'\"></iframe></body></html>";

const FRAME: &str = "<!DOCTYPE html><html><body><p>marco</p></body></html>";

/// Un servidor HTTP/1.1, un hilo por conexión: `/lento` responde a los 2 s.
fn server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
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
                let body = if line.contains(" /lento ") {
                    std::thread::sleep(Duration::from_secs(2));
                    FRAME
                } else {
                    PAGE
                };
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nCache-Control: no-store\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                );
            });
        }
    });
    port
}

#[test]
fn a_slow_frame_does_not_block_its_page() {
    let dir = std::env::temp_dir().join(format!("freedom-test-marco-lento-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let port = server();
    let started = Instant::now();
    // Un `HOME` propio: caché de disco y cookies vacías, sin tocar las del usuario.
    let out = Command::new(env!("CARGO_BIN_EXE_freedom"))
        .env("HOME", &dir)
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .arg("session")
        .arg(format!("http://127.0.0.1:{port}/"))
        .arg("link:pulsar")
        .args(std::iter::repeat_n("wait", 12))
        .arg("-o")
        .arg(dir.join("paso"))
        .output()
        .expect("se pudo ejecutar freedom");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    let titles: Vec<&str> = stdout
        .lines()
        .filter(|l| l.contains(" url="))
        .map(|l| {
            l.split_once("título=\"")
                .and_then(|(_, t)| t.split_once('"'))
                .map_or("", |(t, _)| t)
        })
        .collect();
    // La página pintó (hubo dónde hacer clic) y atendió el clic antes del `load` del marco…
    assert_eq!(titles.get(1), Some(&"padre|clic"), "{stdout}");
    // …que llegó después, cuando el servidor dio el marco.
    assert_eq!(titles.last(), Some(&"padre|clic|marco"), "{stdout}");
    assert!(started.elapsed() >= Duration::from_secs(2));
    let _ = std::fs::remove_dir_all(&dir);
}
