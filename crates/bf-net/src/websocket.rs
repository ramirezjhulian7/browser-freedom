//! Cliente WebSocket (RFC 6455): handshake de apertura (§4.1) con `Sec-WebSocket-Key` y su
//! comprobación (`Sec-WebSocket-Accept`), framing (§5.2) con máscara (§5.3), fragmentación
//! (§5.4), ping/pong (§5.5.2–3) y cierre con código y razón (§5.5.1, §7). `wss://` va sobre
//! el mismo TLS (rustls) que HTTP. (E2)
//!
//! Cada conexión tiene un hilo de E/S que es dueño del socket: escribe los comandos que le
//! llegan por un canal y lee frames con un timeout corto, entregando los eventos a un
//! callback.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, BufReader, Read, Write};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use bf_url::{Host, Url};

use super::{Headers, Stream, USER_AGENT, connect, http};

/// El GUID de RFC 6455 §1.3.
const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Lo que pasa en una conexión.
#[derive(Debug, Clone, PartialEq)]
pub enum WsEvent {
    /// El handshake terminó: subprotocolo y extensiones elegidos por el servidor, y los
    /// `Set-Cookie` de la respuesta (para la jarra del navegador).
    Open {
        protocol: String,
        extensions: String,
        set_cookies: Vec<String>,
    },
    Text(String),
    Binary(Vec<u8>),
    /// Se escribieron `n` bytes de datos de aplicación (para `bufferedAmount`).
    Sent(usize),
    /// La conexión se cerró (§7.1.4); `clean` si hubo cierre de ambos lados.
    Closed {
        code: u16,
        reason: String,
        clean: bool,
    },
    /// "Fail the WebSocket connection" (§7.1.7), antes del `Closed`.
    Error(String),
}

/// Lo que se le pide a la conexión.
#[derive(Debug, Clone)]
pub enum WsCommand {
    Text(String),
    Binary(Vec<u8>),
    /// Iniciar el cierre (§7.1.2), con código y razón opcionales.
    Close(Option<u16>, String),
}

/// La punta del script de una conexión.
#[derive(Debug, Clone)]
pub struct WsClient {
    tx: Sender<WsCommand>,
}

impl WsClient {
    pub fn send(&self, cmd: WsCommand) {
        let _ = self.tx.send(cmd);
    }
}

/// Las opciones de apertura.
#[derive(Debug, Clone, Default)]
pub struct WsOptions {
    pub protocols: Vec<String>,
    /// El origen del documento (cabecera `Origin`).
    pub origin: String,
    /// La cabecera `Cookie` (RFC 6455 §4.1 paso 12), si hay cookies.
    pub cookie: Option<String>,
}

// --- SHA-1 (FIPS 180-4) y base64, para Sec-WebSocket-Accept ---

/// SHA-1 de `data`.
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for block in msg.chunks(64) {
        let mut w = [0u32; 80];
        for (i, c) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

/// base64 (RFC 4648 §4).
pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// El `Sec-WebSocket-Accept` esperado para `key` (§4.2.2 paso 5.4).
pub fn accept_key(key: &str) -> String {
    base64(&sha1(format!("{key}{GUID}").as_bytes()))
}

/// Bytes aleatorios (para la clave y las máscaras).
fn random_bytes(n: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let mut h = RandomState::new().build_hasher();
        h.write_usize(out.len());
        out.extend_from_slice(&h.finish().to_le_bytes());
    }
    out.truncate(n);
    out
}

// --- Frames (§5.2) ---

/// Un frame del cliente: FIN, `opcode`, máscara y longitud extendida.
pub fn encode_frame(opcode: u8, payload: &[u8], fin: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push(if fin { 0x80 } else { 0 } | (opcode & 0x0F));
    let len = payload.len();
    if len < 126 {
        out.push(0x80 | len as u8);
    } else if len <= 0xFFFF {
        out.push(0x80 | 126);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0x80 | 127);
        out.extend_from_slice(&(len as u64).to_be_bytes());
    }
    let mask = random_bytes(4);
    out.extend_from_slice(&mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    out
}

/// Un frame del servidor ya leído.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub fin: bool,
    pub opcode: u8,
    pub payload: Vec<u8>,
}

/// Lee un frame de `buf` si está completo: `Ok(Some((frame, bytes consumidos)))`; un error
/// de protocolo da el código de cierre (§7.4.1).
pub fn decode_frame(buf: &[u8]) -> Result<Option<(Frame, usize)>, (u16, &'static str)> {
    if buf.len() < 2 {
        return Ok(None);
    }
    let fin = buf[0] & 0x80 != 0;
    if buf[0] & 0x70 != 0 {
        return Err((1002, "RSV bits set without a negotiated extension"));
    }
    let opcode = buf[0] & 0x0F;
    if !matches!(opcode, 0 | 1 | 2 | 8 | 9 | 10) {
        return Err((1002, "reserved opcode"));
    }
    let masked = buf[1] & 0x80 != 0;
    if masked {
        return Err((1002, "the server must not mask its frames"));
    }
    let mut len = u64::from(buf[1] & 0x7F);
    let mut pos = 2;
    if len == 126 {
        if buf.len() < 4 {
            return Ok(None);
        }
        len = u64::from(u16::from_be_bytes([buf[2], buf[3]]));
        pos = 4;
    } else if len == 127 {
        if buf.len() < 10 {
            return Ok(None);
        }
        let mut b = [0u8; 8];
        b.copy_from_slice(&buf[2..10]);
        len = u64::from_be_bytes(b);
        if len >> 63 != 0 {
            return Err((1002, "invalid payload length"));
        }
        pos = 10;
    }
    if opcode >= 8 && (!fin || len > 125) {
        return Err((1002, "invalid control frame"));
    }
    if len > 64 * 1024 * 1024 {
        return Err((1009, "message too big"));
    }
    let end = pos + len as usize;
    if buf.len() < end {
        return Ok(None);
    }
    Ok(Some((
        Frame {
            fin,
            opcode,
            payload: buf[pos..end].to_vec(),
        },
        end,
    )))
}

// --- La conexión ---

fn tcp_of(s: &Stream) -> &std::net::TcpStream {
    match s {
        Stream::Plain(t) => t,
        Stream::Tls(t) => &t.sock,
    }
}

/// Abre una conexión a `url` (`ws:` o `wss:`) en un hilo propio; `on_event` recibe los
/// eventos desde ese hilo. `cookies` manda y guarda las cookies de la jarra.
pub fn open(
    url: &Url,
    options: WsOptions,
    mut on_event: Box<dyn FnMut(WsEvent) + Send>,
) -> WsClient {
    let (tx, rx) = mpsc::channel();
    let url = url.clone();
    std::thread::spawn(move || {
        let r = run(&url, &options, &rx, &mut on_event);
        if let Err(msg) = r {
            on_event(WsEvent::Error(msg));
            on_event(WsEvent::Closed {
                code: 1006,
                reason: String::new(),
                clean: false,
            });
        }
    });
    WsClient { tx }
}

/// El handshake de apertura (§4.1) y el bucle de la conexión.
fn run(
    url: &Url,
    options: &WsOptions,
    rx: &Receiver<WsCommand>,
    on_event: &mut Box<dyn FnMut(WsEvent) + Send>,
) -> Result<(), String> {
    let tls = url.scheme == "wss";
    let host = match &url.host {
        Some(Host::Empty) | None => return Err("invalid host".into()),
        Some(h) => h.to_string(),
    };
    let port = url.port.unwrap_or(if tls { 443 } else { 80 });
    let stream = connect(&host, port, tls).map_err(|e| e.to_string())?;
    let mut conn = BufReader::new(stream);
    let key = base64(&random_bytes(16));
    let mut headers = Headers::default();
    headers.push("Host", &url.host_string());
    headers.push("User-Agent", USER_AGENT);
    headers.push("Upgrade", "websocket");
    headers.push("Connection", "Upgrade");
    headers.push("Sec-WebSocket-Key", &key);
    headers.push("Sec-WebSocket-Version", "13");
    if !options.origin.is_empty() {
        headers.push("Origin", &options.origin);
    }
    if !options.protocols.is_empty() {
        headers.push("Sec-WebSocket-Protocol", &options.protocols.join(", "));
    }
    if let Some(c) = &options.cookie {
        headers.push("Cookie", c);
    }
    let mut target = url.path_string();
    if target.is_empty() {
        target.push('/');
    }
    if let Some(q) = &url.query {
        target.push('?');
        target.push_str(q);
    }
    http::write_request(conn.get_mut(), "GET", &target, &headers, None)
        .map_err(|e| e.to_string())?;
    let head = http::read_head(&mut conn).map_err(|e| e.to_string())?;
    if head.status != 101 {
        return Err(format!("unexpected response code {}", head.status));
    }
    let has_token = |name: &str, token: &str| {
        head.headers
            .list(name)
            .iter()
            .any(|t| t.eq_ignore_ascii_case(token))
    };
    if !has_token("upgrade", "websocket") || !has_token("connection", "upgrade") {
        return Err("missing Upgrade/Connection headers".into());
    }
    if head.headers.get("sec-websocket-accept") != Some(accept_key(&key).as_str()) {
        return Err("incorrect Sec-WebSocket-Accept".into());
    }
    let extensions = head
        .headers
        .get("sec-websocket-extensions")
        .unwrap_or("")
        .to_owned();
    if !extensions.is_empty() {
        return Err("the server selected an extension that was not requested".into());
    }
    let protocol = head
        .headers
        .get("sec-websocket-protocol")
        .unwrap_or("")
        .to_owned();
    if !protocol.is_empty() && !options.protocols.iter().any(|p| *p == protocol) {
        return Err("the server selected a subprotocol that was not requested".into());
    }
    let set_cookies = head
        .headers
        .get_all("set-cookie")
        .into_iter()
        .map(str::to_owned)
        .collect();
    on_event(WsEvent::Open {
        protocol,
        extensions,
        set_cookies,
    });
    let _ = tcp_of(conn.get_ref()).set_read_timeout(Some(Duration::from_millis(10)));
    frames(&mut conn, rx, on_event)
}

/// El bucle de la conexión abierta (§6): escribe los comandos y lee frames.
fn frames(
    conn: &mut BufReader<Stream>,
    rx: &Receiver<WsCommand>,
    on_event: &mut Box<dyn FnMut(WsEvent) + Send>,
) -> Result<(), String> {
    let mut buf: Vec<u8> = Vec::new();
    let mut message: Option<(u8, Vec<u8>)> = None;
    let mut close_sent = false;
    let mut close_deadline: Option<Instant> = None;
    let mut tmp = [0u8; 16 * 1024];
    let fail = |conn: &mut BufReader<Stream>, code: u16, msg: &str| -> Result<(), String> {
        let mut payload = code.to_be_bytes().to_vec();
        payload.extend_from_slice(msg.as_bytes());
        let _ = conn.get_mut().write_all(&encode_frame(8, &payload, true));
        Err(msg.to_owned())
    };
    loop {
        // Los comandos del script.
        loop {
            match rx.try_recv() {
                Ok(WsCommand::Text(s)) if !close_sent => {
                    let f = encode_frame(1, s.as_bytes(), true);
                    conn.get_mut().write_all(&f).map_err(|e| e.to_string())?;
                    on_event(WsEvent::Sent(s.len()));
                }
                Ok(WsCommand::Binary(b)) if !close_sent => {
                    let f = encode_frame(2, &b, true);
                    conn.get_mut().write_all(&f).map_err(|e| e.to_string())?;
                    on_event(WsEvent::Sent(b.len()));
                }
                Ok(WsCommand::Close(code, reason)) if !close_sent => {
                    let mut payload = Vec::new();
                    if let Some(c) = code {
                        payload.extend_from_slice(&c.to_be_bytes());
                        payload.extend_from_slice(reason.as_bytes());
                    }
                    let _ = conn.get_mut().write_all(&encode_frame(8, &payload, true));
                    close_sent = true;
                    close_deadline = Some(Instant::now() + Duration::from_secs(5));
                }
                Ok(_) => {}
                Err(TryRecvError::Empty) => break,
                // El script ya no existe: cierre normal.
                Err(TryRecvError::Disconnected) => {
                    if !close_sent {
                        let _ = conn.get_mut().write_all(&encode_frame(
                            8,
                            &1001u16.to_be_bytes(),
                            true,
                        ));
                    }
                    return Ok(());
                }
            }
        }
        if close_deadline.is_some_and(|d| Instant::now() > d) {
            on_event(WsEvent::Closed {
                code: 1006,
                reason: String::new(),
                clean: false,
            });
            return Ok(());
        }
        // Los bytes del servidor.
        match conn.read(&mut tmp) {
            Ok(0) => {
                on_event(WsEvent::Closed {
                    code: 1006,
                    reason: String::new(),
                    clean: false,
                });
                return Ok(());
            }
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(e) => return Err(e.to_string()),
        }
        loop {
            let (frame, used) = match decode_frame(&buf) {
                Ok(Some(f)) => f,
                Ok(None) => break,
                Err((code, msg)) => return fail(conn, code, msg),
            };
            buf.drain(..used);
            match frame.opcode {
                9 => {
                    let _ = conn
                        .get_mut()
                        .write_all(&encode_frame(10, &frame.payload, true));
                }
                10 => {}
                8 => {
                    let (code, reason) = if frame.payload.len() >= 2 {
                        let code = u16::from_be_bytes([frame.payload[0], frame.payload[1]]);
                        let Ok(reason) = String::from_utf8(frame.payload[2..].to_vec()) else {
                            return fail(conn, 1007, "invalid UTF-8 in close reason");
                        };
                        (code, reason)
                    } else if frame.payload.len() == 1 {
                        return fail(conn, 1002, "invalid close payload");
                    } else {
                        (1005, String::new())
                    };
                    if !close_sent {
                        let echo = if code == 1005 {
                            Vec::new()
                        } else {
                            code.to_be_bytes().to_vec()
                        };
                        let _ = conn.get_mut().write_all(&encode_frame(8, &echo, true));
                    }
                    on_event(WsEvent::Closed {
                        code,
                        reason,
                        clean: true,
                    });
                    return Ok(());
                }
                0 => {
                    let Some((op, data)) = message.as_mut() else {
                        return fail(conn, 1002, "unexpected continuation frame");
                    };
                    data.extend_from_slice(&frame.payload);
                    if frame.fin {
                        let (op, data) = (*op, std::mem::take(data));
                        message = None;
                        deliver(conn, op, data, on_event)?;
                    }
                }
                op => {
                    if message.is_some() {
                        return fail(conn, 1002, "expected a continuation frame");
                    }
                    if frame.fin {
                        deliver(conn, op, frame.payload, on_event)?;
                    } else {
                        message = Some((op, frame.payload));
                    }
                }
            }
        }
    }
}

fn deliver(
    conn: &mut BufReader<Stream>,
    opcode: u8,
    data: Vec<u8>,
    on_event: &mut Box<dyn FnMut(WsEvent) + Send>,
) -> Result<(), String> {
    if opcode == 1 {
        match String::from_utf8(data) {
            Ok(s) => on_event(WsEvent::Text(s)),
            Err(_) => {
                let mut payload = 1007u16.to_be_bytes().to_vec();
                payload.extend_from_slice(b"invalid UTF-8");
                let _ = conn.get_mut().write_all(&encode_frame(8, &payload, true));
                return Err("invalid UTF-8 in a text message".into());
            }
        }
    } else {
        on_event(WsEvent::Binary(data));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_and_accept() {
        let h: String = sha1(b"abc").iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(h, "a9993e364706816aba3e25717850c26c9cd0d89d");
        // RFC 6455 §1.3.
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn frames_roundtrip() {
        let f = encode_frame(1, b"hola", true);
        assert_eq!(f[0], 0x81);
        assert_eq!(f[1], 0x80 | 4);
        // Un frame del servidor (sin máscara) con longitud de 16 bits.
        let mut s = vec![0x82, 126, 0x01, 0x00];
        s.extend(std::iter::repeat_n(7u8, 256));
        let (frame, used) = decode_frame(&s).expect("ok").expect("completo");
        assert_eq!((frame.opcode, frame.payload.len(), used), (2, 256, 260));
        assert_eq!(decode_frame(&s[..100]), Ok(None));
        assert!(decode_frame(&[0x81, 0x84, 0, 0, 0, 0]).is_err());
        assert!(decode_frame(&[0x89, 126, 0, 200]).is_err());
    }
}
