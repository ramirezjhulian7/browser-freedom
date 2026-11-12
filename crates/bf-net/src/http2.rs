//! HTTP/2 (RFC 9113) del lado cliente: framing (§4), `SETTINGS` (§6.5), flujos (§5.1),
//! control de flujo (§5.2, §6.9), `GOAWAY` (§6.8) y multiplexado de peticiones sobre una
//! conexión por origen. Las cabeceras se comprimen con [`crate::hpack`].
//!
//! Cada conexión la lleva un hilo propio (el conductor) dueño del socket y del estado TLS;
//! un segundo hilo solo lee bytes del socket. Los dos le hablan al conductor por el mismo
//! canal, igual que las peticiones de [`Connection::request`], así que el conductor nunca
//! espera con un timeout sobre el socket: espera eventos.

use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use crate::hpack;
use crate::http::Headers;

/// §3.4: el prefacio de conexión del cliente.
pub const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// §6: tipos de frame.
pub mod frame_type {
    pub const DATA: u8 = 0x0;
    pub const HEADERS: u8 = 0x1;
    pub const PRIORITY: u8 = 0x2;
    pub const RST_STREAM: u8 = 0x3;
    pub const SETTINGS: u8 = 0x4;
    pub const PUSH_PROMISE: u8 = 0x5;
    pub const PING: u8 = 0x6;
    pub const GOAWAY: u8 = 0x7;
    pub const WINDOW_UPDATE: u8 = 0x8;
    pub const CONTINUATION: u8 = 0x9;
}

/// §6: flags.
pub mod flags {
    pub const END_STREAM: u8 = 0x1;
    pub const ACK: u8 = 0x1;
    pub const END_HEADERS: u8 = 0x4;
    pub const PADDED: u8 = 0x8;
    pub const PRIORITY: u8 = 0x20;
}

/// §6.5.2: identificadores de los parámetros de `SETTINGS`.
pub mod setting {
    pub const HEADER_TABLE_SIZE: u16 = 0x1;
    pub const ENABLE_PUSH: u16 = 0x2;
    pub const MAX_CONCURRENT_STREAMS: u16 = 0x3;
    pub const INITIAL_WINDOW_SIZE: u16 = 0x4;
    pub const MAX_FRAME_SIZE: u16 = 0x5;
    pub const MAX_HEADER_LIST_SIZE: u16 = 0x6;
    /// RFC 9218 §2.1.
    pub const NO_RFC7540_PRIORITIES: u16 = 0x9;
}

/// §7: códigos de error.
pub mod error_code {
    pub const NO_ERROR: u32 = 0x0;
    pub const PROTOCOL_ERROR: u32 = 0x1;
    pub const INTERNAL_ERROR: u32 = 0x2;
    pub const FLOW_CONTROL_ERROR: u32 = 0x3;
    pub const STREAM_CLOSED: u32 = 0x5;
    pub const FRAME_SIZE_ERROR: u32 = 0x6;
    pub const REFUSED_STREAM: u32 = 0x7;
    pub const CANCEL: u32 = 0x8;
    pub const COMPRESSION_ERROR: u32 = 0x9;
    pub const HTTP_1_1_REQUIRED: u32 = 0xd;
}

use error_code as ec;
use frame_type as ft;

/// §6.5.2: valor por defecto de `SETTINGS_INITIAL_WINDOW_SIZE` y de la ventana de conexión.
const DEFAULT_WINDOW: i64 = 65_535;
/// §6.5.2: `SETTINGS_MAX_FRAME_SIZE` por defecto (y el que anunciamos).
const DEFAULT_MAX_FRAME: usize = 16_384;
/// §6.9.1: el máximo de una ventana.
const MAX_WINDOW: i64 = (1 << 31) - 1;
/// Las ventanas de recepción que anunciamos: por flujo y para la conexión.
const STREAM_WINDOW: i64 = 1 << 20;
const CONNECTION_WINDOW: i64 = 16 << 20;
/// Tope de la lista de cabeceras que aceptamos (`SETTINGS_MAX_HEADER_LIST_SIZE`).
const MAX_HEADER_LIST: usize = 256 << 10;
/// Sin flujos abiertos, la conexión se cierra tras este tiempo.
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// Una petición ya preparada: las pseudo-cabeceras (§8.3.1) y el resto, en minúsculas y sin
/// las específicas de conexión (§8.2.2).
#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub scheme: String,
    pub authority: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

impl Request {
    /// Una petición de HTTP/2 a partir de cabeceras de HTTP/1.1: `Host` pasa a `:authority`
    /// y se quitan las cabeceras de conexión (§8.2.2); los nombres van en minúsculas (§8.2.1).
    pub fn from_http1(
        method: &str,
        scheme: &str,
        path: &str,
        headers: &Headers,
        body: Option<&[u8]>,
    ) -> Request {
        let mut authority = String::new();
        let mut out = Vec::new();
        for (n, v) in &headers.0 {
            let n = n.to_ascii_lowercase();
            match n.as_str() {
                "host" => authority.clone_from(v),
                "connection" | "keep-alive" | "proxy-connection" | "transfer-encoding"
                | "upgrade" => {}
                "te" if !v.eq_ignore_ascii_case("trailers") => {}
                _ => out.push((n, v.clone())),
            }
        }
        if let Some(b) = body
            && !out.iter().any(|(n, _)| n == "content-length")
        {
            out.push(("content-length".into(), b.len().to_string()));
        }
        Request {
            method: method.to_owned(),
            scheme: scheme.to_owned(),
            authority,
            path: path.to_owned(),
            headers: out,
            body: body.map(<[u8]>::to_vec),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Headers,
    pub body: Vec<u8>,
}

/// Por qué una petición no terminó.
#[derive(Debug)]
pub enum Error {
    /// El servidor no la procesó (`GOAWAY` con un id menor, `REFUSED_STREAM`, conexión que
    /// ya no acepta flujos): se puede repetir en otra conexión (§8.7).
    Refused,
    /// La conexión se cayó antes de recibir nada del flujo: solo se repite si es idempotente.
    Lost,
    /// El servidor pide HTTP/1.1 (`HTTP_1_1_REQUIRED`).
    Http11Required,
    /// El flujo se reinició con este código.
    Reset(u32),
    Timeout,
    Io(io::Error),
    /// Error de protocolo de la conexión (código y motivo).
    Protocol(u32, &'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Refused => f.write_str("HTTP/2: petición rechazada"),
            Error::Lost => f.write_str("HTTP/2: la conexión se cerró"),
            Error::Http11Required => f.write_str("HTTP/2: el servidor pide HTTP/1.1"),
            Error::Reset(c) => write!(f, "HTTP/2: flujo reiniciado (código {c:#x})"),
            Error::Timeout => f.write_str("HTTP/2: se agotó el tiempo de espera"),
            Error::Io(e) => write!(f, "HTTP/2: {e}"),
            Error::Protocol(c, m) => write!(f, "HTTP/2: error de protocolo {c:#x}: {m}"),
        }
    }
}

impl std::error::Error for Error {}

/// Por donde vuelve la respuesta de una petición.
type Reply = Sender<Result<Response, Error>>;

/// Lo que despierta al conductor.
enum Event {
    Request(Box<Request>, Reply),
    Bytes(Vec<u8>),
    Closed(Option<io::Error>),
    Shutdown,
}

/// Escribe un frame (§4.1).
pub fn write_frame(out: &mut Vec<u8>, ty: u8, flags: u8, stream: u32, payload: &[u8]) {
    let len = payload.len() as u32;
    out.extend_from_slice(&len.to_be_bytes()[1..]);
    out.push(ty);
    out.push(flags);
    out.extend_from_slice(&(stream & 0x7FFF_FFFF).to_be_bytes());
    out.extend_from_slice(payload);
}

/// Un frame leído: tipo, flags, flujo y carga.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub ty: u8,
    pub flags: u8,
    pub stream: u32,
    pub payload: Vec<u8>,
}

/// Saca un frame completo del principio de `buf`, si lo hay. `Err` si pasa de `max_len`
/// (§4.2: `FRAME_SIZE_ERROR`).
pub fn take_frame(buf: &mut Vec<u8>, max_len: usize) -> Result<Option<Frame>, u32> {
    if buf.len() < 9 {
        return Ok(None);
    }
    let len = (usize::from(buf[0]) << 16) | (usize::from(buf[1]) << 8) | usize::from(buf[2]);
    if len > max_len {
        return Err(ec::FRAME_SIZE_ERROR);
    }
    if buf.len() < 9 + len {
        return Ok(None);
    }
    let frame = Frame {
        ty: buf[3],
        flags: buf[4],
        stream: u32::from_be_bytes([buf[5], buf[6], buf[7], buf[8]]) & 0x7FFF_FFFF,
        payload: buf[9..9 + len].to_vec(),
    };
    buf.drain(..9 + len);
    Ok(Some(frame))
}

/// Quita el relleno de un frame `PADDED` (§6.1, §6.2).
fn strip_padding(flags: u8, payload: &[u8]) -> Result<&[u8], u32> {
    if flags & flags::PADDED == 0 {
        return Ok(payload);
    }
    let pad = usize::from(*payload.first().ok_or(ec::FRAME_SIZE_ERROR)?);
    if pad >= payload.len() {
        return Err(ec::PROTOCOL_ERROR);
    }
    Ok(&payload[1..payload.len() - pad])
}

/// La conexión del lado de quien pide: se comparte entre hilos.
pub struct Connection {
    tx: Sender<Event>,
    alive: Arc<AtomicBool>,
}

impl Connection {
    /// ¿Acepta flujos nuevos? (No, tras un `GOAWAY` o si se cayó.)
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    /// Hace una petición y espera la respuesta completa.
    pub fn request(&self, req: Request) -> Result<Response, Error> {
        if !self.is_alive() {
            return Err(Error::Refused);
        }
        let (tx, rx) = mpsc::channel();
        if self.tx.send(Event::Request(Box::new(req), tx)).is_err() {
            return Err(Error::Refused);
        }
        rx.recv().unwrap_or(Err(Error::Lost))
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.tx.send(Event::Shutdown);
    }
}

/// El transporte: TCP plano (h2c) o TLS; el conductor escribe por aquí.
pub enum Transport {
    Plain(TcpStream),
    Tls(Box<rustls::ClientConnection>, TcpStream),
}

impl Transport {
    fn tcp(&self) -> &TcpStream {
        match self {
            Transport::Plain(s) | Transport::Tls(_, s) => s,
        }
    }

    fn send(&mut self, data: &[u8]) -> io::Result<()> {
        match self {
            Transport::Plain(s) => s.write_all(data),
            Transport::Tls(conn, s) => {
                let mut rest = data;
                while !rest.is_empty() {
                    let n = conn.writer().write(rest)?;
                    rest = &rest[n..];
                    while conn.wants_write() {
                        conn.write_tls(s)?;
                    }
                    if n == 0 && !rest.is_empty() {
                        return Err(io::Error::new(io::ErrorKind::WriteZero, "TLS lleno"));
                    }
                }
                Ok(())
            }
        }
    }

    /// Bytes recibidos del socket → texto plano de HTTP/2 en `out`. `Ok(false)` si el par
    /// cerró la sesión TLS.
    fn receive(&mut self, mut bytes: &[u8], out: &mut Vec<u8>) -> io::Result<bool> {
        match self {
            Transport::Plain(_) => {
                out.extend_from_slice(bytes);
                Ok(true)
            }
            Transport::Tls(conn, s) => {
                let mut open = true;
                loop {
                    let state = conn
                        .process_new_packets()
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                    let mut buf = [0u8; 16 * 1024];
                    loop {
                        match conn.reader().read(&mut buf) {
                            Ok(0) => {
                                open = false;
                                break;
                            }
                            Ok(n) => out.extend_from_slice(&buf[..n]),
                            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                            Err(e) => return Err(e),
                        }
                    }
                    while conn.wants_write() {
                        conn.write_tls(s)?;
                    }
                    if state.peer_has_closed() {
                        open = false;
                    }
                    if bytes.is_empty() {
                        return Ok(open);
                    }
                    conn.read_tls(&mut bytes)?;
                }
            }
        }
    }
}

/// Abre HTTP/2 sobre `transport` (TLS ya negociado con ALPN `h2`, o TCP con conocimiento
/// previo, §3.3): manda el prefacio y deja la conexión en manos de su hilo.
pub fn start(transport: Transport) -> io::Result<Connection> {
    let reader = transport.tcp().try_clone()?;
    reader.set_read_timeout(None)?;
    let (tx, rx) = mpsc::channel();
    let alive = Arc::new(AtomicBool::new(true));
    let net_tx = tx.clone();
    std::thread::Builder::new()
        .name("h2-lectura".into())
        .spawn(move || read_loop(reader, net_tx))?;
    let mut driver = Driver::new(transport, rx, alive.clone());
    driver.preface()?;
    // El handshake pudo dejar frames del servidor ya descifrados en rustls.
    if driver.on_bytes(&[]).is_err() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "HTTP/2: prefacio inválido",
        ));
    }
    driver.flush()?;
    std::thread::Builder::new()
        .name("h2-conexión".into())
        .spawn(move || driver.run())?;
    Ok(Connection { tx, alive })
}

fn read_loop(mut tcp: TcpStream, tx: Sender<Event>) {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match tcp.read(&mut buf) {
            Ok(0) => {
                let _ = tx.send(Event::Closed(None));
                return;
            }
            Ok(n) => {
                if tx.send(Event::Bytes(buf[..n].to_vec())).is_err() {
                    return;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => {
                let _ = tx.send(Event::Closed(Some(e)));
                return;
            }
        }
    }
}

/// Lo que mandó el par en `SETTINGS`.
struct PeerSettings {
    header_table_size: usize,
    max_concurrent: usize,
    initial_window: i64,
    max_frame: usize,
}

/// Un flujo abierto por nosotros (§5.1: *open* o *half-closed*).
struct Stream {
    reply: Reply,
    /// Cuerpo pendiente de mandar y si ya se mandó `END_STREAM`.
    body: Vec<u8>,
    sent: usize,
    end_sent: bool,
    send_window: i64,
    recv_window: i64,
    /// Bytes recibidos que aún no devolvimos con `WINDOW_UPDATE`.
    unacked: i64,
    status: Option<u16>,
    headers: Headers,
    data: Vec<u8>,
    /// ¿Llegó algo (para saber si una caída permite repetir)?
    received: bool,
}

/// Un bloque de cabeceras a medias (§6.10: esperando `CONTINUATION`).
struct PendingBlock {
    stream: u32,
    block: Vec<u8>,
    end_stream: bool,
    push: bool,
}

/// Error de conexión (§5.4.1).
struct ConnError(u32, &'static str);

struct Driver {
    transport: Transport,
    rx: Receiver<Event>,
    alive: Arc<AtomicBool>,
    encoder: hpack::Encoder,
    decoder: hpack::Decoder,
    peer: PeerSettings,
    /// Texto plano recibido, aún sin formar frames.
    inbuf: Vec<u8>,
    /// Frames a mandar en la próxima escritura.
    outbuf: Vec<u8>,
    got_settings: bool,
    next_id: u32,
    streams: HashMap<u32, Stream>,
    /// Peticiones que esperan por `SETTINGS_MAX_CONCURRENT_STREAMS`.
    queue: VecDeque<(Box<Request>, Reply)>,
    send_window: i64,
    recv_window: i64,
    unacked: i64,
    continuation: Option<PendingBlock>,
    /// `GOAWAY` recibido (el último flujo que el servidor procesará).
    goaway: Option<u32>,
}

impl Driver {
    fn new(transport: Transport, rx: Receiver<Event>, alive: Arc<AtomicBool>) -> Driver {
        let mut decoder = hpack::Decoder::new(4096);
        decoder.max_list_size = MAX_HEADER_LIST;
        Driver {
            transport,
            rx,
            alive,
            encoder: hpack::Encoder::new(4096),
            decoder,
            peer: PeerSettings {
                header_table_size: 4096,
                max_concurrent: usize::MAX,
                initial_window: DEFAULT_WINDOW,
                max_frame: DEFAULT_MAX_FRAME,
            },
            inbuf: Vec::new(),
            outbuf: Vec::new(),
            got_settings: false,
            next_id: 1,
            streams: HashMap::new(),
            queue: VecDeque::new(),
            send_window: DEFAULT_WINDOW,
            recv_window: DEFAULT_WINDOW,
            unacked: 0,
            continuation: None,
            goaway: None,
        }
    }

    /// §3.4: prefacio, nuestros `SETTINGS` y la ventana de conexión ampliada.
    fn preface(&mut self) -> io::Result<()> {
        let mut settings = Vec::new();
        for (id, value) in [
            (setting::ENABLE_PUSH, 0u32),
            (setting::MAX_CONCURRENT_STREAMS, 100),
            (setting::INITIAL_WINDOW_SIZE, STREAM_WINDOW as u32),
            (setting::MAX_HEADER_LIST_SIZE, MAX_HEADER_LIST as u32),
            // RFC 9218: sin el esquema de prioridades de RFC 7540 (§5.3 de RFC 9113 lo
            // desaconseja); la urgencia va en la cabecera `priority`.
            (setting::NO_RFC7540_PRIORITIES, 1),
        ] {
            settings.extend_from_slice(&id.to_be_bytes());
            settings.extend_from_slice(&value.to_be_bytes());
        }
        let mut out = PREFACE.to_vec();
        write_frame(&mut out, ft::SETTINGS, 0, 0, &settings);
        let increment = (CONNECTION_WINDOW - DEFAULT_WINDOW) as u32;
        write_frame(&mut out, ft::WINDOW_UPDATE, 0, 0, &increment.to_be_bytes());
        self.recv_window = CONNECTION_WINDOW;
        self.transport.send(&out)
    }

    fn run(mut self) {
        let end = loop {
            let busy = !self.streams.is_empty() || !self.queue.is_empty();
            let timeout = if busy {
                crate::READ_TIMEOUT
            } else {
                IDLE_TIMEOUT
            };
            let event = match self.rx.recv_timeout(timeout) {
                Ok(e) => e,
                Err(RecvTimeoutError::Timeout) if busy => break Error::Timeout,
                Err(_) => {
                    self.goaway_and_flush(ec::NO_ERROR);
                    break Error::Lost;
                }
            };
            let result = match event {
                Event::Request(req, reply) => {
                    self.submit(req, reply);
                    Ok(())
                }
                Event::Bytes(bytes) => self.on_bytes(&bytes),
                Event::Closed(e) => {
                    break e.map_or(Error::Lost, Error::Io);
                }
                Event::Shutdown => {
                    self.goaway_and_flush(ec::NO_ERROR);
                    break Error::Lost;
                }
            };
            if let Err(ConnError(code, why)) = result {
                self.goaway_and_flush(code);
                break Error::Protocol(code, why);
            }
            if let Err(e) = self.flush() {
                break Error::Io(e);
            }
            if self.goaway.is_some() && self.streams.is_empty() && self.queue.is_empty() {
                break Error::Lost;
            }
        };
        self.alive.store(false, Ordering::Release);
        let _ = self.transport.tcp().shutdown(Shutdown::Both);
        for (_, s) in self.streams.drain() {
            let err = match &end {
                Error::Timeout => Error::Timeout,
                Error::Protocol(c, w) => Error::Protocol(*c, w),
                _ if !s.received => Error::Lost,
                Error::Io(e) => Error::Io(io::Error::new(e.kind(), e.to_string())),
                _ => Error::Io(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "la conexión HTTP/2 se cerró",
                )),
            };
            let _ = s.reply.send(Err(err));
        }
        for (_, reply) in self.queue.drain(..) {
            let _ = reply.send(Err(Error::Refused));
        }
        // Lo que quede en el canal (peticiones que llegaron tarde) se rechaza.
        while let Ok(ev) = self.rx.try_recv() {
            if let Event::Request(_, reply) = ev {
                let _ = reply.send(Err(Error::Refused));
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.outbuf.is_empty() {
            return Ok(());
        }
        let out = std::mem::take(&mut self.outbuf);
        self.transport.send(&out)
    }

    fn goaway_and_flush(&mut self, code: u32) {
        let mut payload = 0u32.to_be_bytes().to_vec();
        payload.extend_from_slice(&code.to_be_bytes());
        write_frame(&mut self.outbuf, ft::GOAWAY, 0, 0, &payload);
        let _ = self.flush();
    }

    fn submit(&mut self, req: Box<Request>, reply: Reply) {
        // §5.1.1: los ids no se reutilizan; agotados, hace falta otra conexión.
        if self.goaway.is_some() || self.next_id > MAX_WINDOW as u32 {
            self.alive.store(false, Ordering::Release);
            let _ = reply.send(Err(Error::Refused));
            return;
        }
        if self.streams.len() >= self.peer.max_concurrent {
            self.queue.push_back((req, reply));
            return;
        }
        self.open_stream(&req, reply);
    }

    fn open_stream(&mut self, req: &Request, reply: Reply) {
        let id = self.next_id;
        self.next_id += 2;
        // §8.3.1: las pseudo-cabeceras van antes que el resto.
        let mut fields: Vec<(&[u8], &[u8])> = vec![
            (b":method", req.method.as_bytes()),
            (b":scheme", req.scheme.as_bytes()),
            (b":authority", req.authority.as_bytes()),
            (b":path", req.path.as_bytes()),
        ];
        fields.extend(
            req.headers
                .iter()
                .map(|(n, v)| (n.as_bytes(), v.as_bytes())),
        );
        let block = self.encoder.encode(fields);
        let body = req.body.clone().unwrap_or_default();
        let end_stream = req.body.is_none() || body.is_empty();
        // §6.2 y §6.10: el bloque se parte en HEADERS + CONTINUATION según el tamaño de frame.
        let mut chunks = block.chunks(self.peer.max_frame).peekable();
        let first = chunks.next().unwrap_or(&[]);
        let mut f = if end_stream { flags::END_STREAM } else { 0 };
        if chunks.peek().is_none() {
            f |= flags::END_HEADERS;
        }
        write_frame(&mut self.outbuf, ft::HEADERS, f, id, first);
        while let Some(chunk) = chunks.next() {
            let f = if chunks.peek().is_none() {
                flags::END_HEADERS
            } else {
                0
            };
            write_frame(&mut self.outbuf, ft::CONTINUATION, f, id, chunk);
        }
        self.streams.insert(
            id,
            Stream {
                reply,
                body,
                sent: 0,
                end_sent: end_stream,
                send_window: self.peer.initial_window,
                recv_window: STREAM_WINDOW,
                unacked: 0,
                status: None,
                headers: Headers::default(),
                data: Vec::new(),
                received: false,
            },
        );
        self.pump(id);
    }

    /// Manda todo el cuerpo de `id` que dejen las ventanas (§5.2, §6.9).
    fn pump(&mut self, id: u32) {
        let max_frame = self.peer.max_frame as i64;
        let Some(s) = self.streams.get_mut(&id) else {
            return;
        };
        while !s.end_sent {
            let left = (s.body.len() - s.sent) as i64;
            let n = left.min(s.send_window).min(self.send_window).min(max_frame);
            if n <= 0 && left > 0 {
                return;
            }
            let n = n.max(0) as usize;
            let end = s.sent + n == s.body.len();
            let f = if end { flags::END_STREAM } else { 0 };
            write_frame(
                &mut self.outbuf,
                ft::DATA,
                f,
                id,
                &s.body[s.sent..s.sent + n],
            );
            s.sent += n;
            s.send_window -= n as i64;
            self.send_window -= n as i64;
            s.end_sent = end;
        }
        s.body = Vec::new();
    }

    fn pump_all(&mut self) {
        let mut ids: Vec<u32> = self
            .streams
            .iter()
            .filter(|(_, s)| !s.end_sent)
            .map(|(&id, _)| id)
            .collect();
        ids.sort_unstable();
        for id in ids {
            self.pump(id);
        }
    }

    fn on_bytes(&mut self, bytes: &[u8]) -> Result<(), ConnError> {
        let mut plain = std::mem::take(&mut self.inbuf);
        let open = self
            .transport
            .receive(bytes, &mut plain)
            .map_err(|_| ConnError(ec::PROTOCOL_ERROR, "TLS"))?;
        self.inbuf = plain;
        loop {
            let frame = match take_frame(&mut self.inbuf, DEFAULT_MAX_FRAME) {
                Ok(Some(f)) => f,
                Ok(None) => break,
                Err(code) => return Err(ConnError(code, "frame demasiado grande")),
            };
            self.on_frame(frame)?;
        }
        if !open {
            return Err(ConnError(ec::NO_ERROR, "TLS cerrado"));
        }
        Ok(())
    }

    fn on_frame(&mut self, f: Frame) -> Result<(), ConnError> {
        // §3.4: lo primero del servidor es un SETTINGS.
        if !self.got_settings {
            if f.ty != ft::SETTINGS || f.flags & flags::ACK != 0 {
                return Err(ConnError(ec::PROTOCOL_ERROR, "falta el SETTINGS inicial"));
            }
            self.got_settings = true;
        }
        // §6.10: tras un bloque sin END_HEADERS solo vale CONTINUATION del mismo flujo.
        if let Some(p) = &self.continuation
            && (f.ty != ft::CONTINUATION || f.stream != p.stream)
        {
            return Err(ConnError(ec::PROTOCOL_ERROR, "se esperaba CONTINUATION"));
        }
        match f.ty {
            ft::DATA => self.on_data(&f),
            ft::HEADERS => {
                if f.stream == 0 {
                    return Err(ConnError(ec::PROTOCOL_ERROR, "HEADERS en el flujo 0"));
                }
                let mut payload =
                    strip_padding(f.flags, &f.payload).map_err(|c| ConnError(c, "relleno"))?;
                if f.flags & flags::PRIORITY != 0 {
                    // §6.2: dependencia y peso (5 bytes); se ignoran (§5.3.2).
                    payload = payload
                        .get(5..)
                        .ok_or(ConnError(ec::FRAME_SIZE_ERROR, "HEADERS corto"))?;
                }
                self.block_fragment(
                    f.stream,
                    payload,
                    f.flags & flags::END_HEADERS != 0,
                    f.flags & flags::END_STREAM != 0,
                    false,
                )
            }
            ft::CONTINUATION => {
                let Some(p) = &mut self.continuation else {
                    return Err(ConnError(ec::PROTOCOL_ERROR, "CONTINUATION suelto"));
                };
                p.block.extend_from_slice(&f.payload);
                if p.block.len() > MAX_HEADER_LIST * 2 {
                    return Err(ConnError(ec::PROTOCOL_ERROR, "bloque demasiado grande"));
                }
                if f.flags & flags::END_HEADERS != 0 {
                    let p = self.continuation.take().expect("hay bloque");
                    self.headers_done(p.stream, &p.block, p.end_stream, p.push)?;
                }
                Ok(())
            }
            ft::PRIORITY => {
                if f.stream == 0 {
                    return Err(ConnError(ec::PROTOCOL_ERROR, "PRIORITY en el flujo 0"));
                }
                if f.payload.len() != 5 {
                    self.reset(f.stream, ec::FRAME_SIZE_ERROR);
                }
                Ok(())
            }
            ft::RST_STREAM => {
                if f.stream == 0 {
                    return Err(ConnError(ec::PROTOCOL_ERROR, "RST_STREAM en el flujo 0"));
                }
                if f.payload.len() != 4 {
                    return Err(ConnError(ec::FRAME_SIZE_ERROR, "RST_STREAM"));
                }
                if f.stream >= self.next_id {
                    return Err(ConnError(
                        ec::PROTOCOL_ERROR,
                        "RST_STREAM de un flujo ocioso",
                    ));
                }
                let code =
                    u32::from_be_bytes([f.payload[0], f.payload[1], f.payload[2], f.payload[3]]);
                if let Some(s) = self.streams.remove(&f.stream) {
                    let err = match code {
                        ec::REFUSED_STREAM => Error::Refused,
                        ec::HTTP_1_1_REQUIRED => Error::Http11Required,
                        // §8.1: un NO_ERROR tras la respuesta completa solo corta nuestro
                        // cuerpo; si la respuesta no llegó, es un reinicio más.
                        c => Error::Reset(c),
                    };
                    let _ = s.reply.send(Err(err));
                    self.dequeue();
                }
                Ok(())
            }
            ft::SETTINGS => self.on_settings(&f),
            ft::PUSH_PROMISE => {
                // §8.4: desactivamos el push con SETTINGS_ENABLE_PUSH = 0.
                Err(ConnError(ec::PROTOCOL_ERROR, "PUSH_PROMISE sin push"))
            }
            ft::PING => {
                if f.stream != 0 {
                    return Err(ConnError(ec::PROTOCOL_ERROR, "PING en un flujo"));
                }
                if f.payload.len() != 8 {
                    return Err(ConnError(ec::FRAME_SIZE_ERROR, "PING"));
                }
                if f.flags & flags::ACK == 0 {
                    write_frame(&mut self.outbuf, ft::PING, flags::ACK, 0, &f.payload);
                }
                Ok(())
            }
            ft::GOAWAY => {
                if f.stream != 0 {
                    return Err(ConnError(ec::PROTOCOL_ERROR, "GOAWAY en un flujo"));
                }
                if f.payload.len() < 8 {
                    return Err(ConnError(ec::FRAME_SIZE_ERROR, "GOAWAY"));
                }
                let last =
                    u32::from_be_bytes([f.payload[0], f.payload[1], f.payload[2], f.payload[3]])
                        & 0x7FFF_FFFF;
                self.goaway = Some(last);
                self.alive.store(false, Ordering::Release);
                // §6.8: los flujos por encima de `last` no se procesaron: se pueden repetir.
                let refused: Vec<u32> = self
                    .streams
                    .keys()
                    .copied()
                    .filter(|&id| id > last)
                    .collect();
                for id in refused {
                    if let Some(s) = self.streams.remove(&id) {
                        let _ = s.reply.send(Err(Error::Refused));
                    }
                }
                for (_, reply) in self.queue.drain(..) {
                    let _ = reply.send(Err(Error::Refused));
                }
                Ok(())
            }
            ft::WINDOW_UPDATE => self.on_window_update(&f),
            // §5.5: los tipos desconocidos se ignoran.
            _ => Ok(()),
        }
    }

    fn on_data(&mut self, f: &Frame) -> Result<(), ConnError> {
        if f.stream == 0 {
            return Err(ConnError(ec::PROTOCOL_ERROR, "DATA en el flujo 0"));
        }
        if f.stream >= self.next_id || f.stream.is_multiple_of(2) {
            return Err(ConnError(ec::PROTOCOL_ERROR, "DATA en un flujo ocioso"));
        }
        // §6.9.1: todo el payload (relleno incluido) cuenta para el control de flujo.
        let len = f.payload.len() as i64;
        self.recv_window -= len;
        if self.recv_window < 0 {
            return Err(ConnError(ec::FLOW_CONTROL_ERROR, "ventana de conexión"));
        }
        self.unacked += len;
        if self.unacked >= CONNECTION_WINDOW / 2 {
            write_frame(
                &mut self.outbuf,
                ft::WINDOW_UPDATE,
                0,
                0,
                &(self.unacked as u32).to_be_bytes(),
            );
            self.recv_window += self.unacked;
            self.unacked = 0;
        }
        let data = strip_padding(f.flags, &f.payload).map_err(|c| ConnError(c, "relleno"))?;
        let end = f.flags & flags::END_STREAM != 0;
        let Some(s) = self.streams.get_mut(&f.stream) else {
            // §5.1: DATA de un flujo ya cerrado por nosotros (lo reiniciamos): se descarta.
            return Ok(());
        };
        if s.status.is_none() {
            // §8.1: DATA antes de las cabeceras: respuesta mal formada.
            self.reset(f.stream, ec::PROTOCOL_ERROR);
            return Ok(());
        }
        s.recv_window -= len;
        if s.recv_window < 0 {
            self.reset(f.stream, ec::FLOW_CONTROL_ERROR);
            return Ok(());
        }
        s.received = true;
        s.data.extend_from_slice(data);
        if end {
            self.complete(f.stream);
            return Ok(());
        }
        s.unacked += len;
        if s.unacked >= STREAM_WINDOW / 2 {
            let inc = s.unacked;
            s.recv_window += inc;
            s.unacked = 0;
            write_frame(
                &mut self.outbuf,
                ft::WINDOW_UPDATE,
                0,
                f.stream,
                &(inc as u32).to_be_bytes(),
            );
        }
        Ok(())
    }

    fn block_fragment(
        &mut self,
        stream: u32,
        fragment: &[u8],
        end_headers: bool,
        end_stream: bool,
        push: bool,
    ) -> Result<(), ConnError> {
        if end_headers {
            return self.headers_done(stream, fragment, end_stream, push);
        }
        self.continuation = Some(PendingBlock {
            stream,
            block: fragment.to_vec(),
            end_stream,
            push,
        });
        Ok(())
    }

    fn headers_done(
        &mut self,
        stream: u32,
        block: &[u8],
        end_stream: bool,
        _push: bool,
    ) -> Result<(), ConnError> {
        // §4.3: el bloque se decodifica siempre (para mantener la tabla en sincronía), aunque
        // el flujo ya no exista.
        let fields = self
            .decoder
            .decode(block)
            .map_err(|e| ConnError(ec::COMPRESSION_ERROR, e.0))?;
        if stream.is_multiple_of(2) || stream >= self.next_id {
            return Err(ConnError(ec::PROTOCOL_ERROR, "HEADERS de un flujo ocioso"));
        }
        let Some(s) = self.streams.get_mut(&stream) else {
            return Ok(());
        };
        s.received = true;
        if s.status.is_some() {
            // Trailers (§8.1): tienen que cerrar el flujo; su contenido no se usa.
            if !end_stream {
                self.reset(stream, ec::PROTOCOL_ERROR);
            } else {
                self.complete(stream);
            }
            return Ok(());
        }
        match parse_response_fields(fields) {
            Ok((status, headers)) => {
                // §8.1: las respuestas 1xx se descartan; la final viene después.
                if (100..200).contains(&status) {
                    if end_stream {
                        self.reset(stream, ec::PROTOCOL_ERROR);
                    }
                    return Ok(());
                }
                s.status = Some(status);
                s.headers = headers;
                if end_stream {
                    self.complete(stream);
                }
            }
            Err(()) => self.reset(stream, ec::PROTOCOL_ERROR),
        }
        Ok(())
    }

    fn on_settings(&mut self, f: &Frame) -> Result<(), ConnError> {
        if f.stream != 0 {
            return Err(ConnError(ec::PROTOCOL_ERROR, "SETTINGS en un flujo"));
        }
        if f.flags & flags::ACK != 0 {
            if !f.payload.is_empty() {
                return Err(ConnError(ec::FRAME_SIZE_ERROR, "SETTINGS ACK con datos"));
            }
            return Ok(());
        }
        if !f.payload.len().is_multiple_of(6) {
            return Err(ConnError(ec::FRAME_SIZE_ERROR, "SETTINGS"));
        }
        for p in f.payload.chunks(6) {
            let id = u16::from_be_bytes([p[0], p[1]]);
            let value = u32::from_be_bytes([p[2], p[3], p[4], p[5]]);
            match id {
                setting::HEADER_TABLE_SIZE => {
                    // Nuestro codificador no pasa de 4096 aunque el servidor permita más.
                    let size = (value as usize).min(4096);
                    if size != self.peer.header_table_size {
                        self.peer.header_table_size = size;
                        self.encoder.set_max_size(size);
                    }
                }
                setting::ENABLE_PUSH => {
                    if value > 1 {
                        return Err(ConnError(ec::PROTOCOL_ERROR, "ENABLE_PUSH"));
                    }
                }
                setting::MAX_CONCURRENT_STREAMS => self.peer.max_concurrent = value as usize,
                setting::INITIAL_WINDOW_SIZE => {
                    if i64::from(value) > MAX_WINDOW {
                        return Err(ConnError(ec::FLOW_CONTROL_ERROR, "INITIAL_WINDOW_SIZE"));
                    }
                    // §6.9.2: el cambio se aplica a las ventanas de todos los flujos.
                    let delta = i64::from(value) - self.peer.initial_window;
                    self.peer.initial_window = i64::from(value);
                    for s in self.streams.values_mut() {
                        s.send_window += delta;
                        if s.send_window > MAX_WINDOW {
                            return Err(ConnError(ec::FLOW_CONTROL_ERROR, "ventana de flujo"));
                        }
                    }
                }
                setting::MAX_FRAME_SIZE => {
                    if !(16_384..=16_777_215).contains(&value) {
                        return Err(ConnError(ec::PROTOCOL_ERROR, "MAX_FRAME_SIZE"));
                    }
                    self.peer.max_frame = value as usize;
                }
                // §6.5.2: los desconocidos se ignoran (MAX_HEADER_LIST_SIZE es orientativo).
                _ => {}
            }
        }
        write_frame(&mut self.outbuf, ft::SETTINGS, flags::ACK, 0, &[]);
        self.pump_all();
        self.dequeue();
        Ok(())
    }

    fn on_window_update(&mut self, f: &Frame) -> Result<(), ConnError> {
        if f.payload.len() != 4 {
            return Err(ConnError(ec::FRAME_SIZE_ERROR, "WINDOW_UPDATE"));
        }
        let inc = i64::from(
            u32::from_be_bytes([f.payload[0], f.payload[1], f.payload[2], f.payload[3]])
                & 0x7FFF_FFFF,
        );
        if f.stream == 0 {
            if inc == 0 {
                return Err(ConnError(ec::PROTOCOL_ERROR, "WINDOW_UPDATE de 0"));
            }
            self.send_window += inc;
            if self.send_window > MAX_WINDOW {
                return Err(ConnError(ec::FLOW_CONTROL_ERROR, "ventana de conexión"));
            }
            self.pump_all();
            return Ok(());
        }
        let Some(s) = self.streams.get_mut(&f.stream) else {
            return Ok(());
        };
        if inc == 0 {
            self.reset(f.stream, ec::PROTOCOL_ERROR);
            return Ok(());
        }
        s.send_window += inc;
        if s.send_window > MAX_WINDOW {
            self.reset(f.stream, ec::FLOW_CONTROL_ERROR);
            return Ok(());
        }
        self.pump(f.stream);
        Ok(())
    }

    /// §5.4.2: error de flujo: `RST_STREAM` y la petición falla.
    fn reset(&mut self, id: u32, code: u32) {
        let mut payload = Vec::new();
        payload.extend_from_slice(&code.to_be_bytes());
        write_frame(&mut self.outbuf, ft::RST_STREAM, 0, id, &payload);
        if let Some(s) = self.streams.remove(&id) {
            let _ = s.reply.send(Err(Error::Reset(code)));
        }
        self.dequeue();
    }

    fn complete(&mut self, id: u32) {
        let Some(s) = self.streams.remove(&id) else {
            return;
        };
        // §8.1: si la respuesta llega antes de terminar nuestro cuerpo, dejamos de mandarlo.
        if !s.end_sent {
            write_frame(
                &mut self.outbuf,
                ft::RST_STREAM,
                0,
                id,
                &ec::NO_ERROR.to_be_bytes(),
            );
        }
        let _ = s.reply.send(Ok(Response {
            status: s.status.unwrap_or(0),
            headers: s.headers,
            body: s.data,
        }));
        self.dequeue();
    }

    fn dequeue(&mut self) {
        while self.streams.len() < self.peer.max_concurrent && self.goaway.is_none() {
            let Some((req, reply)) = self.queue.pop_front() else {
                break;
            };
            self.open_stream(&req, reply);
        }
    }
}

/// `:status` y las cabeceras normales de una respuesta. `Err` si está mal formada (§8.1.1,
/// §8.2, §8.3.2): nombres con mayúsculas, pseudo-cabeceras desconocidas o tras las normales,
/// sin `:status`, o cabeceras de conexión.
fn parse_response_fields(fields: Vec<hpack::Field>) -> Result<(u16, Headers), ()> {
    let mut status = None;
    let mut headers = Headers::default();
    let mut regular = false;
    for (name, value) in fields {
        if name.iter().any(u8::is_ascii_uppercase) {
            return Err(());
        }
        let value: String = value.iter().map(|&b| char::from(b)).collect();
        if name.first() == Some(&b':') {
            if regular || name != b":status" || status.is_some() {
                return Err(());
            }
            status = Some(value.parse::<u16>().map_err(|_| ())?);
            continue;
        }
        regular = true;
        let name = String::from_utf8(name).map_err(|_| ())?;
        if matches!(
            name.as_str(),
            "connection" | "keep-alive" | "proxy-connection" | "transfer-encoding" | "upgrade"
        ) {
            return Err(());
        }
        headers.push(&name, &value);
    }
    status
        .filter(|s| (100..1000).contains(s))
        .map(|s| (s, headers))
        .ok_or(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, ft::PING, 0, 0, &[1, 2, 3, 4, 5, 6, 7, 8]);
        write_frame(&mut buf, ft::DATA, flags::END_STREAM, 3, b"hola");
        let mut partial = buf[..12].to_vec();
        assert_eq!(take_frame(&mut partial, DEFAULT_MAX_FRAME), Ok(None));
        let f = take_frame(&mut buf, DEFAULT_MAX_FRAME).unwrap().unwrap();
        assert_eq!((f.ty, f.stream, f.payload.len()), (ft::PING, 0, 8));
        let f = take_frame(&mut buf, DEFAULT_MAX_FRAME).unwrap().unwrap();
        assert_eq!((f.ty, f.flags, f.stream), (ft::DATA, flags::END_STREAM, 3));
        assert_eq!(f.payload, b"hola");
        assert!(buf.is_empty());
        // Un frame más grande que SETTINGS_MAX_FRAME_SIZE.
        let mut big = Vec::new();
        write_frame(&mut big, ft::DATA, 0, 1, &vec![0; DEFAULT_MAX_FRAME + 1]);
        assert_eq!(
            take_frame(&mut big, DEFAULT_MAX_FRAME),
            Err(ec::FRAME_SIZE_ERROR)
        );
    }

    #[test]
    fn padding() {
        assert_eq!(
            strip_padding(flags::PADDED, &[2, b'a', 0, 0]),
            Ok(&b"a"[..])
        );
        assert_eq!(strip_padding(0, &[2, b'a']), Ok(&[2, b'a'][..]));
        assert_eq!(
            strip_padding(flags::PADDED, &[3, b'a', 0]),
            Err(ec::PROTOCOL_ERROR)
        );
    }

    #[test]
    fn request_from_http1_headers() {
        let mut h = Headers::default();
        h.push("Host", "example.com:8443");
        h.push("Connection", "keep-alive");
        h.push("User-Agent", "Freedom");
        h.push("TE", "gzip");
        let r = Request::from_http1("POST", "https", "/x?y", &h, Some(b"abc"));
        assert_eq!(r.authority, "example.com:8443");
        assert_eq!(
            r.headers,
            vec![
                ("user-agent".to_owned(), "Freedom".to_owned()),
                ("content-length".to_owned(), "3".to_owned())
            ]
        );
    }

    #[test]
    fn malformed_responses() {
        let f = |list: &[(&str, &str)]| -> Vec<hpack::Field> {
            list.iter()
                .map(|(n, v)| (n.as_bytes().to_vec(), v.as_bytes().to_vec()))
                .collect()
        };
        let (status, h) = parse_response_fields(f(&[(":status", "204"), ("x", "1")])).unwrap();
        assert_eq!((status, h.get("x")), (204, Some("1")));
        assert!(parse_response_fields(f(&[("x", "1"), (":status", "200")])).is_err());
        assert!(parse_response_fields(f(&[(":status", "200"), ("X", "1")])).is_err());
        assert!(parse_response_fields(f(&[(":status", "200"), ("connection", "close")])).is_err());
        assert!(parse_response_fields(f(&[(":path", "/")])).is_err());
        assert!(parse_response_fields(f(&[])).is_err());
    }
}
