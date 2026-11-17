//! E3: cómo responde el `Fetcher` a los tres fallos de red más comunes, con servidores locales:
//!
//! - una conexión **reutilizada que el servidor ya cerró**: se repite con una nueva solo si el
//!   método es idempotente (RFC 9110 §9.2.2); un `POST` no se repite, porque no se sabe si el
//!   servidor llegó a aplicarlo;
//! - una **respuesta HTTP/2 cortada** a mitad del cuerpo (la conexión se cae sin `END_STREAM`):
//!   es un error de red, no una respuesta con el cuerpo a medias (RFC 9113 §8.1: un mensaje
//!   termina con `END_STREAM`); el `REFUSED_STREAM` (§8.7) ya lo cubre `h2.rs`;
//! - un servidor que **no responde**: el error es "tiempo agotado", con su causa, cuando pasa el
//!   `read_timeout`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use bf_net::hpack::Encoder;
use bf_net::http2::{PREFACE, flags, frame_type as ft, take_frame, write_frame};
use bf_net::{FetchOptions, Fetcher, Method, NetErrorKind};
use bf_url::Url;

fn url(port: u16, path: &str) -> Url {
    Url::parse(&format!("http://127.0.0.1:{port}{path}"), None).unwrap()
}

/// Lee una petición HTTP/1.1 (línea, cabeceras y un cuerpo con `Content-Length`); `None` si el
/// cliente cerró.
fn read_request(r: &mut BufReader<TcpStream>) -> Option<String> {
    let mut line = String::new();
    if r.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut len = 0usize;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).ok()? == 0 || h == "\r\n" {
            break;
        }
        if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).ok()?;
    Some(line)
}

/// Un servidor HTTP/1.1: en la primera conexión responde a la primera petición con keep-alive
/// y cierra sin responder a la segunda; en las siguientes conexiones responde `otra`. Cuenta
/// las conexiones.
fn stale_server() -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = accepted.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let n = count.fetch_add(1, Ordering::SeqCst);
            let mut r = BufReader::new(stream.try_clone().unwrap());
            if n == 0 {
                if read_request(&mut r).is_none() {
                    continue;
                }
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: keep-alive\r\n\r\nuno",
                );
                // La segunda petición en esta conexión: se cierra sin responder.
                let _ = read_request(&mut r);
                continue;
            }
            while read_request(&mut r).is_some() {
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: keep-alive\r\n\r\notra",
                );
            }
        }
    });
    (port, accepted)
}

#[test]
fn a_stale_connection_is_retried_only_for_idempotent_methods() {
    // GET: se repite en una conexión nueva.
    let (port, accepted) = stale_server();
    let f = Fetcher::new();
    let get = |p: &str| f.fetch(&url(port, p), &FetchOptions::default());
    assert_eq!(get("/a").unwrap().body, b"uno");
    assert_eq!(get("/b").unwrap().body, b"otra");
    assert_eq!(accepted.load(Ordering::SeqCst), 2);

    // POST: no se repite; el error llega a quien pidió, con su causa.
    let (port, accepted) = stale_server();
    let f = Fetcher::new();
    assert_eq!(
        f.fetch(&url(port, "/a"), &FetchOptions::default())
            .unwrap()
            .body,
        b"uno"
    );
    let post = FetchOptions {
        method: Method::Post,
        body: Some((b"x=1".to_vec(), "application/x-www-form-urlencoded".into())),
        ..FetchOptions::default()
    };
    let err = f.fetch(&url(port, "/enviar"), &post).unwrap_err();
    assert_eq!(err.kind(), NetErrorKind::ConnectionReset);
    // Solo la primera conexión: el POST no se mandó otra vez.
    thread::sleep(Duration::from_millis(100));
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
}

/// Un servidor HTTP/2 (h2c con conocimiento previo) que responde a la primera petición con
/// las cabeceras y la mitad del cuerpo, y cierra la conexión sin `END_STREAM`.
fn cut_h2_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut buf = vec![0u8; PREFACE.len()];
        s.read_exact(&mut buf).unwrap();
        assert_eq!(buf, PREFACE);
        let mut out = Vec::new();
        write_frame(&mut out, ft::SETTINGS, 0, 0, &[]);
        s.write_all(&out).unwrap();
        // Hasta las cabeceras de la petición (el flujo 1).
        let mut pending = Vec::new();
        loop {
            if let Some(f) = take_frame(&mut pending, 1 << 24).unwrap() {
                if f.ty == ft::SETTINGS && f.flags & flags::ACK == 0 {
                    let mut ack = Vec::new();
                    write_frame(&mut ack, ft::SETTINGS, flags::ACK, 0, &[]);
                    s.write_all(&ack).unwrap();
                }
                if f.ty == ft::HEADERS {
                    break;
                }
                continue;
            }
            let mut tmp = [0u8; 4096];
            let n = s.read(&mut tmp).unwrap();
            assert!(n > 0);
            pending.extend_from_slice(&tmp[..n]);
        }
        let fields = [(":status", "200"), ("content-length", "10")];
        let block =
            Encoder::new(4096).encode(fields.iter().map(|(n, v)| (n.as_bytes(), v.as_bytes())));
        let mut out = Vec::new();
        write_frame(&mut out, ft::HEADERS, flags::END_HEADERS, 1, &block);
        write_frame(&mut out, ft::DATA, 0, 1, b"mitad");
        s.write_all(&out).unwrap();
        s.flush().unwrap();
        thread::sleep(Duration::from_millis(50));
        // Se cae la conexión: el cuerpo no termina.
        drop(s);
    });
    port
}

#[test]
fn a_cut_http2_response_is_a_network_error() {
    let port = cut_h2_server();
    let mut f = Fetcher::new();
    f.h2_prior_knowledge = true;
    f.read_timeout = Some(Duration::from_secs(5));
    let r = f.fetch(&url(port, "/cortada"), &FetchOptions::default());
    match r {
        Ok(res) => panic!(
            "una respuesta cortada no es una respuesta: {} con {:?}",
            res.status,
            String::from_utf8_lossy(&res.body)
        ),
        Err(e) => assert_eq!(e.kind(), NetErrorKind::ConnectionReset, "{e}"),
    }
}

#[test]
fn a_silent_server_times_out() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        let (s, _) = listener.accept().unwrap();
        // Acepta y no responde nunca (mientras dura el test).
        thread::sleep(Duration::from_secs(5));
        drop(s);
    });
    let mut f = Fetcher::new();
    f.read_timeout = Some(Duration::from_millis(300));
    let start = Instant::now();
    let err = f
        .fetch(&url(port, "/lento"), &FetchOptions::default())
        .unwrap_err();
    assert_eq!(err.kind(), NetErrorKind::Timeout, "{err}");
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "{:?}",
        start.elapsed()
    );
}
