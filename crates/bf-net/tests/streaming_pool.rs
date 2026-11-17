//! E3: las peticiones por partes de un `Fetcher` (`fetch()` y XHR de los scripts) reutilizan
//! sus conexiones HTTP/1.1 (RFC 9112 §9.3) en vez de abrir una con `Connection: close` por
//! petición; un `POST` no se arriesga a una conexión libre, y una respuesta que no se leyó
//! entera no deja su conexión en el pool.

#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bf_net::streaming::{StreamHead, StreamSink};
use bf_net::{Fetcher, RequestOptions};
use bf_url::Url;

/// Un servidor HTTP/1.1 con keep-alive que cuenta conexiones: `/c` responde por trozos,
/// `/grande` 64 KiB y el resto `ok` con `Content-Length`.
fn server() -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let conns = Arc::new(AtomicUsize::new(0));
    let c = conns.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            c.fetch_add(1, Ordering::SeqCst);
            std::thread::spawn(move || {
                let mut r = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let mut len = 0usize;
                    loop {
                        let mut h = String::new();
                        if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                            break;
                        }
                        if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                            len = v.trim().parse().unwrap_or(0);
                        }
                    }
                    let mut body = vec![0u8; len];
                    let _ = r.read_exact(&mut body);
                    let out: Vec<u8> = if line.contains(" /c ") {
                        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
                          3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n"
                            .to_vec()
                    } else if line.contains(" /grande ") {
                        let mut v = b"HTTP/1.1 200 OK\r\nContent-Length: 65536\r\n\r\n".to_vec();
                        v.extend(std::iter::repeat_n(b'x', 65536));
                        v
                    } else {
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec()
                    };
                    if stream.write_all(&out).is_err() {
                        return;
                    }
                }
            });
        }
    });
    (port, conns)
}

/// Junta el cuerpo; con `stop`, corta tras el primer trozo.
struct Collect {
    body: Vec<u8>,
    stop: bool,
}

impl StreamSink for Collect {
    fn head(&mut self, _: &StreamHead) -> bool {
        true
    }
    fn chunk(&mut self, bytes: &[u8]) -> bool {
        self.body.extend_from_slice(bytes);
        !self.stop
    }
}

fn get(f: &Fetcher, port: u16, path: &str, opts: &RequestOptions, stop: bool) -> Vec<u8> {
    let url = Url::parse(&format!("http://127.0.0.1:{port}{path}"), None).unwrap();
    let mut sink = Collect {
        body: Vec::new(),
        stop,
    };
    f.fetch_streaming(&url, opts, &mut sink).unwrap();
    sink.body
}

#[test]
fn streaming_requests_reuse_their_connections() {
    let (port, conns) = server();
    let f = Fetcher::new();
    let opts = RequestOptions::default();
    assert_eq!(get(&f, port, "/a", &opts, false), b"ok");
    assert_eq!(get(&f, port, "/c", &opts, false), b"abcde");
    assert_eq!(get(&f, port, "/b", &opts, false), b"ok");
    assert_eq!(conns.load(Ordering::SeqCst), 1);
    // Un `POST` va por una conexión nueva (no se puede repetir si la libre estaba cerrada)…
    let post = RequestOptions {
        method: "POST".into(),
        body: Some(b"x=1".to_vec()),
        ..RequestOptions::default()
    };
    assert_eq!(get(&f, port, "/p", &post, false), b"ok");
    assert_eq!(conns.load(Ordering::SeqCst), 2);
    // …y una respuesta que quien la lee corta a medias no deja su conexión libre.
    let cut = get(&f, port, "/grande", &opts, true);
    assert!(cut.len() < 65536);
    assert_eq!(get(&f, port, "/a", &opts, false), b"ok");
    assert_eq!(get(&f, port, "/a", &opts, false), b"ok");
    // Las dos libres (la del primer `GET` y la del `POST`) se usaron; la cortada, no.
    assert_eq!(conns.load(Ordering::SeqCst), 2);
}
