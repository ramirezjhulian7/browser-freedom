//! HTTP/2 contra un servidor escrito aquí (frames a mano y HPACK de `bf_net::hpack`): h2c
//! con conocimiento previo, flujos concurrentes en una conexión, control de flujo en los
//! dos sentidos, CONTINUATION, 1xx, trailers, PING, GOAWAY, REFUSED_STREAM, repliegue a
//! HTTP/1.1 y TLS con ALPN (`h2` y `http/1.1`).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;

use bf_net::hpack::{Decoder, Encoder};
use bf_net::http2::{
    Frame, PREFACE, error_code, flags, frame_type as ft, setting, take_frame, write_frame,
};
use bf_net::{FetchOptions, Fetcher, Method, Protocol};
use bf_url::Url;

/// Una conexión de servidor HTTP/2 sobre cualquier transporte.
struct Server<S: Read + Write> {
    io: S,
    buf: Vec<u8>,
    enc: Encoder,
    dec: Decoder,
    /// Ventanas de envío (conexión y por flujo) que nos dio el cliente.
    conn_window: i64,
    initial_window: i64,
    windows: HashMap<u32, i64>,
    /// Cuerpos recibidos por flujo.
    bodies: HashMap<u32, Vec<u8>>,
    pings_acked: usize,
    stream_updates: usize,
    /// El primer byte del último bloque de cabeceras recibido.
    last_block_first: Option<u8>,
}

/// Una petición leída: pseudo-cabeceras y cabeceras juntas.
struct Req {
    id: u32,
    fields: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Req {
    fn get(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

impl<S: Read + Write> Server<S> {
    /// Lee el prefacio y los SETTINGS del cliente; manda los nuestros.
    fn handshake(io: S, settings: &[(u16, u32)]) -> Server<S> {
        let mut s = Server {
            io,
            buf: Vec::new(),
            enc: Encoder::new(4096),
            dec: Decoder::new(4096),
            conn_window: 65_535,
            initial_window: 65_535,
            windows: HashMap::new(),
            bodies: HashMap::new(),
            pings_acked: 0,
            stream_updates: 0,
            last_block_first: None,
        };
        let mut preface = [0u8; 24];
        s.io.read_exact(&mut preface).unwrap();
        assert_eq!(&preface[..], PREFACE);
        let mut payload = Vec::new();
        for (id, v) in settings {
            payload.extend_from_slice(&id.to_be_bytes());
            payload.extend_from_slice(&v.to_be_bytes());
        }
        s.send(ft::SETTINGS, 0, 0, &payload);
        let first = s.raw_frame();
        assert_eq!(first.ty, ft::SETTINGS, "el cliente empieza con SETTINGS");
        s.apply_settings(&first);
        s.send(ft::SETTINGS, flags::ACK, 0, &[]);
        s
    }

    fn apply_settings(&mut self, f: &Frame) {
        for p in f.payload.chunks(6) {
            let id = u16::from_be_bytes([p[0], p[1]]);
            let v = u32::from_be_bytes([p[2], p[3], p[4], p[5]]);
            if id == setting::INITIAL_WINDOW_SIZE {
                self.initial_window = i64::from(v);
            }
            if id == setting::ENABLE_PUSH {
                assert_eq!(v, 0, "el cliente desactiva el push");
            }
        }
    }

    fn send(&mut self, ty: u8, fl: u8, stream: u32, payload: &[u8]) {
        let mut out = Vec::new();
        write_frame(&mut out, ty, fl, stream, payload);
        self.io.write_all(&out).unwrap();
        self.io.flush().unwrap();
    }

    fn raw_frame(&mut self) -> Frame {
        loop {
            if let Some(f) = take_frame(&mut self.buf, 1 << 24).unwrap() {
                return f;
            }
            let mut tmp = [0u8; 65536];
            let n = self.io.read(&mut tmp).unwrap();
            assert!(n > 0, "el cliente cerró la conexión");
            self.buf.extend_from_slice(&tmp[..n]);
        }
    }

    /// Lee un frame: los de control (SETTINGS, PING ACK, WINDOW_UPDATE) se atienden aquí y
    /// dan `None`.
    fn step(&mut self) -> Option<Frame> {
        let f = self.raw_frame();
        match f.ty {
            ft::SETTINGS if f.flags & flags::ACK == 0 => {
                self.apply_settings(&f);
                self.send(ft::SETTINGS, flags::ACK, 0, &[]);
            }
            ft::SETTINGS => {}
            ft::PING if f.flags & flags::ACK != 0 => self.pings_acked += 1,
            ft::WINDOW_UPDATE => {
                let inc = i64::from(u32::from_be_bytes([
                    f.payload[0],
                    f.payload[1],
                    f.payload[2],
                    f.payload[3],
                ]));
                if f.stream == 0 {
                    self.conn_window += inc;
                } else {
                    self.stream_updates += 1;
                    *self.windows.entry(f.stream).or_insert(self.initial_window) += inc;
                }
            }
            _ => return Some(f),
        }
        None
    }

    /// El próximo frame que no sea de control.
    fn frame(&mut self) -> Frame {
        loop {
            if let Some(f) = self.step() {
                return f;
            }
        }
    }

    /// Lee frames hasta tener una petición completa (cabeceras y cuerpo). Con `ack_data`,
    /// devuelve la ventana de cada DATA recibido.
    fn request(&mut self, ack_data: bool) -> Req {
        let f = self.frame();
        assert_eq!(f.ty, ft::HEADERS, "se esperaba HEADERS, llegó {}", f.ty);
        let id = f.stream;
        assert_eq!(id % 2, 1, "los flujos del cliente son impares");
        let mut block = f.payload.clone();
        let mut end_stream = f.flags & flags::END_STREAM != 0;
        let mut end_headers = f.flags & flags::END_HEADERS != 0;
        while !end_headers {
            let c = self.raw_frame();
            assert_eq!((c.ty, c.stream), (ft::CONTINUATION, id));
            block.extend_from_slice(&c.payload);
            end_headers = c.flags & flags::END_HEADERS != 0;
        }
        self.last_block_first = block.first().copied();
        let fields = self
            .dec
            .decode(&block)
            .unwrap()
            .into_iter()
            .map(|(n, v)| (String::from_utf8(n).unwrap(), String::from_utf8(v).unwrap()))
            .collect();
        let mut body = Vec::new();
        while !end_stream {
            let d = self.frame();
            assert_eq!((d.ty, d.stream), (ft::DATA, id));
            body.extend_from_slice(&d.payload);
            end_stream = d.flags & flags::END_STREAM != 0;
            if ack_data && !d.payload.is_empty() {
                let inc = (d.payload.len() as u32).to_be_bytes();
                self.send(ft::WINDOW_UPDATE, 0, 0, &inc);
                self.send(ft::WINDOW_UPDATE, 0, id, &inc);
            }
        }
        self.bodies.insert(id, body.clone());
        Req { id, fields, body }
    }

    fn headers(&mut self, id: u32, fl: u8, fields: &[(&str, &str)]) {
        let block = self
            .enc
            .encode(fields.iter().map(|(n, v)| (n.as_bytes(), v.as_bytes())));
        self.send(ft::HEADERS, fl | flags::END_HEADERS, id, &block);
    }

    /// DATA respetando las ventanas del cliente (espera WINDOW_UPDATE si hace falta).
    fn data(&mut self, id: u32, body: &[u8], end: bool) {
        let mut sent = 0;
        while sent < body.len() {
            let window = *self.windows.entry(id).or_insert(self.initial_window);
            if window <= 0 || self.conn_window <= 0 {
                // Sin ventana: se atienden frames hasta que el cliente la amplíe.
                if let Some(f) = self.step() {
                    panic!("frame inesperado mientras se esperaba ventana: {}", f.ty);
                }
                continue;
            }
            let n = (body.len() - sent)
                .min(16_384)
                .min(window as usize)
                .min(self.conn_window as usize);
            let last = sent + n == body.len();
            let fl = if last && end { flags::END_STREAM } else { 0 };
            self.send(ft::DATA, fl, id, &body[sent..sent + n]);
            sent += n;
            *self.windows.get_mut(&id).unwrap() -= n as i64;
            self.conn_window -= n as i64;
        }
        if body.is_empty() && end {
            self.send(ft::DATA, flags::END_STREAM, id, &[]);
        }
    }

    fn respond(&mut self, id: u32, status: &str, extra: &[(&str, &str)], body: &[u8]) {
        let mut fields = vec![(":status", status)];
        fields.extend_from_slice(extra);
        self.headers(id, 0, &fields);
        self.data(id, body, true);
    }
}

fn h2c_fetcher() -> Fetcher {
    let mut f = Fetcher::new();
    f.h2_prior_knowledge = true;
    f
}

fn url(port: u16, path: &str) -> Url {
    Url::parse(&format!("http://127.0.0.1:{port}{path}"), None).unwrap()
}

#[test]
fn get_over_h2c_with_cookies_and_padding() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut s = Server::handshake(stream, &[]);
        let r = s.request(false);
        assert_eq!(r.get(":method"), Some("GET"));
        assert_eq!(r.get(":scheme"), Some("http"));
        assert_eq!(r.get(":path"), Some("/hola?x=1"));
        assert_eq!(r.get(":authority"), Some(&*format!("127.0.0.1:{port}")));
        assert!(r.get("user-agent").is_some());
        assert!(r.get("host").is_none() && r.get("connection").is_none());
        // Las pseudo-cabeceras van primero (RFC 9113 §8.3).
        let first_regular = r
            .fields
            .iter()
            .position(|(n, _)| !n.starts_with(':'))
            .unwrap();
        assert!(
            r.fields[first_regular..]
                .iter()
                .all(|(n, _)| !n.starts_with(':'))
        );
        s.headers(
            r.id,
            0,
            &[
                (":status", "200"),
                ("content-type", "text/plain"),
                ("set-cookie", "sabor=fresa"),
            ],
        );
        // DATA con relleno: 3 bytes de padding.
        let mut padded = vec![3];
        padded.extend_from_slice(b"hola ");
        padded.extend_from_slice(&[0, 0, 0]);
        s.send(ft::DATA, flags::PADDED, r.id, &padded);
        s.send(ft::DATA, flags::END_STREAM, r.id, b"h2");
        let r2 = s.request(false);
        assert_eq!(r2.id, 3);
        assert_eq!(r2.get("cookie"), Some("sabor=fresa"));
        s.respond(r2.id, "404", &[], b"");
        s
    });
    let f = h2c_fetcher();
    let res = f
        .fetch(&url(port, "/hola?x=1"), &FetchOptions::default())
        .unwrap();
    assert_eq!(res.status, 200);
    assert_eq!(res.status_text, "");
    assert_eq!(res.protocol, Protocol::H2);
    assert_eq!(res.body, b"hola h2");
    assert_eq!(res.headers.get("content-type"), Some("text/plain"));
    let res = f
        .fetch(&url(port, "/otra"), &FetchOptions::default())
        .unwrap();
    assert_eq!(res.status, 404);
    assert!(res.body.is_empty());
    let log = f.requests();
    assert_eq!(log.len(), 2);
    assert!(
        log.iter()
            .all(|e| e.protocol == Protocol::H2 && e.method == "GET")
    );
    assert_eq!((log[0].status, log[1].status), (200, 404));
    assert_eq!(*f.connections_opened.lock().unwrap(), 1);
    server.join().unwrap();
}

#[test]
fn concurrent_streams_share_one_connection() {
    const N: usize = 6;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut s = Server::handshake(stream, &[]);
        // Todas las peticiones llegan antes de responder ninguna: están multiplexadas.
        let reqs: Vec<Req> = (0..N).map(|_| s.request(false)).collect();
        let mut ids: Vec<u32> = reqs.iter().map(|r| r.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), N);
        // Las respuestas, al revés y con los DATA intercalados.
        for r in reqs.iter().rev() {
            s.headers(r.id, 0, &[(":status", "200")]);
        }
        for r in &reqs {
            s.send(ft::DATA, 0, r.id, b"cuerpo de ");
        }
        for r in reqs.iter().rev() {
            let path = r.get(":path").unwrap().to_owned();
            s.send(ft::DATA, flags::END_STREAM, r.id, path.as_bytes());
        }
        // Que ninguna conexión más llegue.
        listener.set_nonblocking(true).unwrap();
        assert!(listener.accept().is_err());
        s
    });
    let f = Arc::new(h2c_fetcher());
    let threads: Vec<_> = (0..N)
        .map(|i| {
            let f = f.clone();
            thread::spawn(move || {
                let path = format!("/r{i}");
                let res = f
                    .fetch(&url(port, &path), &FetchOptions::default())
                    .unwrap();
                assert_eq!(res.protocol, Protocol::H2);
                assert_eq!(res.body, format!("cuerpo de {path}").into_bytes());
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    server.join().unwrap();
    assert_eq!(*f.connections_opened.lock().unwrap(), 1);
}

#[test]
fn flow_control_in_both_directions() {
    let upload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let download: Vec<u8> = (0..3_000_000u32).map(|i| (i % 241) as u8).collect();
    let (up, down) = (upload.clone(), download.clone());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        // Ventana de flujo de 1000 bytes: el cliente tiene que esperar nuestros
        // WINDOW_UPDATE para mandar los 200 KB.
        let mut s = Server::handshake(stream, &[(setting::INITIAL_WINDOW_SIZE, 1000)]);
        let r = s.request(true);
        assert_eq!(r.get(":method"), Some("POST"));
        assert_eq!(r.get("content-length"), Some("200000"));
        assert_eq!(r.body, up);
        // 3 MB de bajada con la ventana de flujo de 1 MiB del cliente: hacen falta sus
        // WINDOW_UPDATE.
        s.respond(r.id, "200", &[], &down);
        assert!(
            s.stream_updates >= 2,
            "WINDOW_UPDATE del cliente: {}",
            s.stream_updates
        );
        s
    });
    let f = h2c_fetcher();
    let opts = FetchOptions {
        method: Method::Post,
        body: Some((upload, "application/octet-stream".into())),
        ..FetchOptions::default()
    };
    let res = f.fetch(&url(port, "/subir"), &opts).unwrap();
    assert_eq!(res.status, 200);
    assert_eq!(res.body.len(), download.len());
    assert!(res.body == download);
    server.join().unwrap();
}

#[test]
fn continuation_informational_trailers_ping_and_table_size() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut s = Server::handshake(stream, &[]);
        let r = s.request(false);
        s.send(ft::PING, 0, 0, b"12345678");
        // 103 Early Hints, que se descarta.
        s.headers(
            r.id,
            0,
            &[(":status", "103"), ("link", "</a.css>; rel=preload")],
        );
        // La respuesta final, en HEADERS (con PRIORITY y relleno) + 2 CONTINUATION.
        let long = "x".repeat(3000);
        let block = s.enc.encode(
            [
                (":status", "200"),
                ("x-largo", long.as_str()),
                ("content-type", "text/html"),
            ]
            .iter()
            .map(|(n, v)| (n.as_bytes(), v.as_bytes())),
        );
        let (a, rest) = block.split_at(10);
        let (b, c) = rest.split_at(rest.len() / 2);
        let mut first = vec![2, 0, 0, 0, 0, 16];
        first.extend_from_slice(a);
        first.extend_from_slice(&[0, 0]);
        s.send(ft::HEADERS, flags::PADDED | flags::PRIORITY, r.id, &first);
        s.send(ft::CONTINUATION, 0, r.id, b);
        s.send(ft::CONTINUATION, flags::END_HEADERS, r.id, c);
        // Un tipo de frame desconocido se ignora (§5.5).
        s.send(0xEE, 0, 0, b"?");
        s.send(ft::DATA, 0, r.id, b"<p>hola</p>");
        // Trailers.
        s.headers(r.id, flags::END_STREAM, &[("x-trailer", "fin")]);
        // Tabla dinámica a 0: el cliente lo anuncia al principio del siguiente bloque.
        let mut payload = setting::HEADER_TABLE_SIZE.to_be_bytes().to_vec();
        payload.extend_from_slice(&0u32.to_be_bytes());
        s.send(ft::SETTINGS, 0, 0, &payload);
        let r2 = s.request(false);
        assert_eq!(s.last_block_first, Some(0x20));
        assert_eq!(s.pings_acked, 1);
        s.respond(r2.id, "204", &[], b"");
        s
    });
    let f = h2c_fetcher();
    let res = f.fetch(&url(port, "/"), &FetchOptions::document()).unwrap();
    assert_eq!(res.status, 200);
    assert_eq!(res.headers.get("x-largo").map(str::len), Some(3000));
    assert!(res.headers.get("x-trailer").is_none());
    assert!(res.headers.get("link").is_none());
    assert_eq!(res.body, b"<p>hola</p>");
    let res = f.fetch(&url(port, "/2"), &FetchOptions::default()).unwrap();
    assert_eq!(res.status, 204);
    server.join().unwrap();
}

#[test]
fn refused_stream_and_goaway_are_retried() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut s = Server::handshake(stream, &[]);
        let r = s.request(false);
        s.respond(r.id, "200", &[], b"uno");
        // REFUSED_STREAM: el cliente repite en la misma conexión.
        let r = s.request(false);
        assert_eq!(r.id, 3);
        s.send(
            ft::RST_STREAM,
            0,
            r.id,
            &error_code::REFUSED_STREAM.to_be_bytes(),
        );
        let r = s.request(false);
        assert_eq!((r.id, r.get(":path")), (5, Some("/dos")));
        s.respond(r.id, "200", &[], b"dos");
        // GOAWAY: el último flujo procesado es el 5; la conexión se cierra.
        let mut goaway = 5u32.to_be_bytes().to_vec();
        goaway.extend_from_slice(&error_code::NO_ERROR.to_be_bytes());
        s.send(ft::GOAWAY, 0, 0, &goaway);
        drop(s);
        // La siguiente petición llega por una conexión nueva.
        let (stream, _) = listener.accept().unwrap();
        let mut s = Server::handshake(stream, &[]);
        let r = s.request(false);
        assert_eq!((r.id, r.get(":path")), (1, Some("/tres")));
        s.respond(r.id, "200", &[], b"tres");
        s
    });
    let f = h2c_fetcher();
    let get = |p: &str| {
        f.fetch(&url(port, p), &FetchOptions::default())
            .unwrap()
            .body
    };
    assert_eq!(get("/uno"), b"uno");
    assert_eq!(get("/dos"), b"dos");
    // Puede que el cliente aún no haya visto el GOAWAY: entonces su petición cae en una
    // conexión cerrada y se repite (GET es idempotente).
    assert_eq!(get("/tres"), b"tres");
    assert_eq!(*f.connections_opened.lock().unwrap(), 2);
    server.join().unwrap();
}

#[test]
fn http_1_1_required_falls_back() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut s = Server::handshake(stream, &[]);
        let r = s.request(false);
        s.send(
            ft::RST_STREAM,
            0,
            r.id,
            &error_code::HTTP_1_1_REQUIRED.to_be_bytes(),
        );
        // La repetición llega por HTTP/1.1.
        let (stream, _) = listener.accept().unwrap();
        let mut r = BufReader::new(stream);
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        assert_eq!(line, "GET /viejo HTTP/1.1\r\n");
        loop {
            let mut l = String::new();
            r.read_line(&mut l).unwrap();
            if l == "\r\n" {
                break;
            }
        }
        r.get_mut()
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nviejo")
            .unwrap();
        (s, r)
    });
    let f = h2c_fetcher();
    let res = f
        .fetch(&url(port, "/viejo"), &FetchOptions::default())
        .unwrap();
    assert_eq!(res.protocol, Protocol::Http11);
    assert_eq!(res.status_text, "OK");
    assert_eq!(res.body, b"viejo");
    server.join().unwrap();
}

fn tls_server_config(alpn: &[&[u8]]) -> Arc<rustls::ServerConfig> {
    let cert = rustls_pki_types::CertificateDer::from(include_bytes!("data/server.der").to_vec());
    let key = rustls_pki_types::PrivateKeyDer::Pkcs8(rustls_pki_types::PrivatePkcs8KeyDer::from(
        include_bytes!("data/server.key.der").to_vec(),
    ));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .unwrap();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    Arc::new(config)
}

fn tls_accept(
    listener: &TcpListener,
    config: Arc<rustls::ServerConfig>,
) -> rustls::StreamOwned<rustls::ServerConnection, TcpStream> {
    let (tcp, _) = listener.accept().unwrap();
    let conn = rustls::ServerConnection::new(config).unwrap();
    rustls::StreamOwned::new(conn, tcp)
}

fn test_fetcher() -> Fetcher {
    Fetcher::with_root_certificates(&[include_bytes!("data/ca.der").to_vec()])
}

#[test]
fn tls_negotiates_h2_by_alpn() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let stream = tls_accept(&listener, tls_server_config(&[b"h2", b"http/1.1"]));
        let mut s = Server::handshake(stream, &[(setting::MAX_CONCURRENT_STREAMS, 1)]);
        assert_eq!(s.io.conn.alpn_protocol(), Some(&b"h2"[..]));
        for _ in 0..3 {
            let r = s.request(false);
            assert_eq!(r.get(":scheme"), Some("https"));
            assert_eq!(r.get(":authority"), Some(&*format!("127.0.0.1:{port}")));
            assert!(r.get("sec-ch-ua").is_some(), "Client Hints en HTTPS");
            let body = r.get(":path").unwrap().to_owned();
            s.respond(
                r.id,
                "200",
                &[("content-type", "text/plain")],
                body.as_bytes(),
            );
        }
        s
    });
    // Tres peticiones a la vez con SETTINGS_MAX_CONCURRENT_STREAMS = 1: van en cola.
    let f = Arc::new(test_fetcher());
    let threads: Vec<_> = (0..3)
        .map(|i| {
            let f = f.clone();
            thread::spawn(move || {
                let u = Url::parse(&format!("https://127.0.0.1:{port}/t{i}"), None).unwrap();
                let res = f.fetch(&u, &FetchOptions::default()).unwrap();
                assert_eq!(res.protocol, Protocol::H2);
                assert_eq!(res.body, format!("/t{i}").into_bytes());
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    server.join().unwrap();
    assert_eq!(*f.connections_opened.lock().unwrap(), 1);
}

#[test]
fn tls_without_h2_uses_http_1_1() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let mut stream = tls_accept(&listener, tls_server_config(&[b"http/1.1"]));
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = stream.read(&mut tmp).unwrap();
            buf.extend_from_slice(&tmp[..n]);
        }
        assert!(buf.starts_with(b"GET /uno HTTP/1.1\r\n"));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            .unwrap();
        stream.flush().unwrap();
        stream
    });
    let f = test_fetcher();
    let u = Url::parse(&format!("https://127.0.0.1:{port}/uno"), None).unwrap();
    let res = f.fetch(&u, &FetchOptions::default()).unwrap();
    assert_eq!(res.protocol, Protocol::Http11);
    assert_eq!(res.body, b"ok");
    server.join().unwrap();
}

#[test]
fn tls_bad_certificate_is_a_tls_error() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let mut stream = tls_accept(&listener, tls_server_config(&[b"h2"]));
        let _ = stream.read(&mut [0u8; 16]);
    });
    // Sin nuestra CA: el certificado no es de confianza.
    let f = Fetcher::new();
    let u = Url::parse(&format!("https://127.0.0.1:{port}/"), None).unwrap();
    match f.fetch(&u, &FetchOptions::default()) {
        Err(bf_net::FetchError::Tls(_)) => {}
        other => panic!("se esperaba un error de TLS: {other:?}"),
    }
    server.join().unwrap();
}

/// E6: HSTS (RFC 6797) de punta a punta. Una respuesta por TLS con `Strict-Transport-Security`
/// hace que el siguiente `http://localhost:<puerto>/` vaya por `https:` al mismo puerto (§8.3
/// paso 5), reusando la conexión. El servidor solo escucha en 127.0.0.1: si `localhost` resuelve
/// primero a `::1`, la conexión llega por IPv4 gracias a Happy Eyeballs (RFC 8305).
#[test]
fn hsts_upgrades_http_to_https() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let mut stream = tls_accept(&listener, tls_server_config(&[b"http/1.1"]));
        let mut paths = Vec::new();
        for sts in ["max-age=600", ""] {
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut tmp).unwrap();
                assert!(n > 0, "el cliente cerró");
                buf.extend_from_slice(&tmp[..n]);
            }
            let line = String::from_utf8_lossy(&buf)
                .lines()
                .next()
                .unwrap()
                .to_owned();
            paths.push(line.split(' ').nth(1).unwrap().to_owned());
            let extra = if sts.is_empty() {
                String::new()
            } else {
                format!("Strict-Transport-Security: {sts}\r\n")
            };
            stream
                .write_all(
                    format!("HTTP/1.1 200 OK\r\n{extra}Content-Length: 2\r\n\r\nok").as_bytes(),
                )
                .unwrap();
            stream.flush().unwrap();
        }
        paths
    });
    let f = test_fetcher();
    let start = std::time::Instant::now();
    let first = Url::parse(&format!("https://localhost:{port}/primera"), None).unwrap();
    assert_eq!(
        f.fetch(&first, &FetchOptions::default()).unwrap().body,
        b"ok"
    );
    assert!(f.hsts.is_known("localhost"));
    let plain = Url::parse(&format!("http://localhost:{port}/segunda"), None).unwrap();
    let res = f.fetch(&plain, &FetchOptions::default()).unwrap();
    assert_eq!(res.url.href(), format!("https://localhost:{port}/segunda"));
    assert_eq!(res.body, b"ok");
    assert_eq!(server.join().unwrap(), ["/primera", "/segunda"]);
    assert_eq!(*f.connections_opened.lock().unwrap(), 1);
    assert!(
        start.elapsed() < std::time::Duration::from_millis(1500),
        "{:?}",
        start.elapsed()
    );
}

/// La red de verdad: Wikipedia habla h2. `cargo test -p bf-net --test h2 -- --ignored`.
#[test]
#[ignore]
fn wikipedia_over_h2() {
    let f = Fetcher::new();
    let u = Url::parse("https://es.wikipedia.org/wiki/Wikipedia:Portada", None).unwrap();
    let res = f.fetch(&u, &FetchOptions::document()).unwrap();
    assert_eq!(res.protocol, Protocol::H2);
    assert_eq!(res.status, 200);
    assert!(res.body.len() > 10_000);
}
