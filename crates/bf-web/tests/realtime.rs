//! Conexiones abiertas desde JavaScript (`bf_web::net`): `WebSocket` (contra un servidor
//! RFC 6455 local), `EventSource` (un servidor `text/event-stream` local, con reconexión) y
//! `MessageChannel`/`MessagePort`/`BroadcastChannel`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bf_js::host::DefaultHooks;
use bf_web::{DocumentOwner, Runtime, Url, WebContext, WebContextHandle};

// --- El servidor ---

fn read_head(r: &mut BufReader<TcpStream>) -> Option<(String, Vec<(String, String)>)> {
    let mut line = String::new();
    r.read_line(&mut line).ok()?;
    let path = line.split_whitespace().nth(1)?.to_owned();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        r.read_line(&mut h).ok()?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((n, v)) = h.split_once(':') {
            headers.push((n.trim().to_ascii_lowercase(), v.trim().to_owned()));
        }
    }
    Some((path, headers))
}

fn header<'a>(h: &'a [(String, String)], n: &str) -> Option<&'a str> {
    h.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str())
}

/// Un frame del servidor (sin máscara).
fn server_frame(opcode: u8, payload: &[u8], fin: bool) -> Vec<u8> {
    let mut out = vec![if fin { 0x80 } else { 0 } | opcode];
    if payload.len() < 126 {
        out.push(payload.len() as u8);
    } else {
        out.push(126);
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    }
    out.extend_from_slice(payload);
    out
}

/// Lee un frame del cliente (con máscara).
fn client_frame(r: &mut impl Read) -> Option<(u8, Vec<u8>)> {
    let mut h = [0u8; 2];
    r.read_exact(&mut h).ok()?;
    let mut len = u64::from(h[1] & 0x7F);
    if len == 126 {
        let mut b = [0u8; 2];
        r.read_exact(&mut b).ok()?;
        len = u64::from(u16::from_be_bytes(b));
    } else if len == 127 {
        let mut b = [0u8; 8];
        r.read_exact(&mut b).ok()?;
        len = u64::from_be_bytes(b);
    }
    let mut mask = [0u8; 4];
    r.read_exact(&mut mask).ok()?;
    let mut data = vec![0u8; len as usize];
    r.read_exact(&mut data).ok()?;
    for (i, b) in data.iter_mut().enumerate() {
        *b ^= mask[i % 4];
    }
    Some((h[0] & 0x0F, data))
}

fn websocket(mut r: BufReader<TcpStream>, path: &str, headers: &[(String, String)]) {
    let key = header(headers, "sec-websocket-key")
        .unwrap_or("")
        .to_owned();
    let mut resp = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n",
        bf_net::websocket::accept_key(&key)
    );
    if path == "/ws/proto" {
        resp.push_str("Sec-WebSocket-Protocol: chat\r\n");
    }
    if path == "/ws/bad-accept" {
        resp = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: malo\r\n".into();
    }
    if path == "/ws/cookie" {
        resp.push_str("Set-Cookie: ws=1; Path=/\r\n");
    }
    resp.push_str("\r\n");
    let mut w = r.get_ref().try_clone().expect("clone");
    let _ = w.write_all(resp.as_bytes());
    match path {
        "/ws/fragment" => {
            let _ = w.write_all(&server_frame(1, b"hola ", false));
            let _ = w.write_all(&server_frame(9, b"p", true));
            let _ = w.write_all(&server_frame(0, b"mun", false));
            let _ = w.write_all(&server_frame(0, b"do", true));
        }
        "/ws/close" => {
            let mut p = 4000u16.to_be_bytes().to_vec();
            p.extend_from_slice(b"bye");
            let _ = w.write_all(&server_frame(8, &p, true));
        }
        "/ws/drop" => return,
        "/ws/info" => {
            let info = format!(
                "proto={} origin={} cookie={}",
                header(headers, "sec-websocket-protocol").unwrap_or("-"),
                header(headers, "origin").unwrap_or("-"),
                header(headers, "cookie").unwrap_or("-")
            );
            let _ = w.write_all(&server_frame(1, info.as_bytes(), true));
        }
        "/ws/ping" => {
            let _ = w.write_all(&server_frame(9, b"hey", true));
        }
        _ => {}
    }
    // Eco hasta el cierre.
    while let Some((op, data)) = client_frame(&mut r) {
        match op {
            1 | 2 => {
                let _ = w.write_all(&server_frame(op, &data, true));
            }
            10 => {
                let _ = w.write_all(&server_frame(
                    1,
                    format!("pong:{}", String::from_utf8_lossy(&data)).as_bytes(),
                    true,
                ));
            }
            8 => {
                let _ = w.write_all(&server_frame(8, &data, true));
                return;
            }
            _ => {}
        }
    }
}

fn sse(r: BufReader<TcpStream>, path: &str, headers: &[(String, String)]) {
    let mut s = r.into_inner();
    let last = header(headers, "last-event-id").unwrap_or("").to_owned();
    if path == "/sse-bad" {
        let _ = s.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nno",
        );
        return;
    }
    if path == "/sse-404" {
        let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        return;
    }
    let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n");
    if !last.is_empty() {
        let _ = s.write_all(format!("data: reconnected {last}\n\n").as_bytes());
        std::thread::sleep(Duration::from_millis(200));
        return;
    }
    let body: &[&[u8]] = &[
        b"\xEF\xBB\xBFretry: 30\n",
        b": comentario\n",
        b"data: uno\n\n",
        b"event: custom\r\ndata: dos\r\nid: 7\r\n\r\n",
        b"data: a\ndata: b\n\n",
        b"data: caf\xC3",
        b"\xA9\n\n",
    ];
    for part in body {
        let _ = s.write_all(part);
        let _ = s.flush();
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn handle(stream: TcpStream) {
    let mut r = BufReader::new(stream);
    let Some((path, headers)) = read_head(&mut r) else {
        return;
    };
    if path.starts_with("/ws") {
        websocket(r, &path, &headers);
    } else if path.starts_with("/sse") {
        sse(r, &path, &headers);
    } else {
        let mut s = r.into_inner();
        let _ = s.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 2\r\n\r\nok",
        );
    }
}

fn server() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = l.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            std::thread::spawn(move || handle(s));
        }
    });
    port
}

// --- La página ---

struct Page {
    rt: Runtime,
    ctx: WebContextHandle,
    port: u16,
}

const PRELUDE: &str = "var result = null, done = false, log = [];\
    function finish(v) { if (!done) { result = String(v); done = true; } }\
    function fail(e) { finish('ERR ' + (e && e.name) + ': ' + (e && e.message)); }";

impl Page {
    fn new() -> Page {
        let port = server();
        let mut rt = Runtime::new(Box::new(DefaultHooks));
        let (doc, _) =
            bf_html::parse_document("<!DOCTYPE html><p>x", &bf_html::ParseOptions::default());
        let realm = rt.main_realm();
        let url = Url::parse(&format!("http://127.0.0.1:{port}/page.html"), None).expect("url");
        let ctx = bf_web::install(
            &mut rt.agent,
            realm,
            WebContext::new(DocumentOwner::Document(Box::new(doc)), url),
        );
        bf_web::net::set_backend(&mut rt.agent, realm, Arc::new(bf_net::Fetcher::new()), true);
        let mut p = Page { rt, ctx, port };
        p.exec(PRELUDE);
        p
    }

    fn exec(&mut self, src: &str) {
        let realm = self.ctx.realm;
        if let Err(e) = self.rt.eval_script(realm, src, "test.js") {
            panic!("error: {e}\n{}", e.stack);
        }
    }

    fn eval_string(&mut self, src: &str) -> String {
        let realm = self.ctx.realm;
        match self
            .rt
            .eval_script(realm, &format!("String({src})"), "test.js")
        {
            Ok(v) => self.rt.convert::<String>(&v).expect("string"),
            Err(e) => panic!("error: {e}\n{}", e.stack),
        }
    }

    fn run(&mut self, src: &str) -> String {
        let realm = self.ctx.realm;
        let code = format!(
            "var PORT = {port}; var WS = 'ws://127.0.0.1:{port}'; var BASE = 'http://127.0.0.1:{port}';\n\
             try {{ {src} }} catch (e) {{ fail(e); }}",
            port = self.port
        );
        self.exec(&code);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            bf_web::net::pump(&mut self.rt.agent, realm);
            let _ = self.rt.run_until_idle();
            if self.eval_string("done") == "true" {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "tiempo agotado; log = {}",
                self.eval_string("JSON.stringify(log)")
            );
            std::thread::sleep(Duration::from_millis(3));
        }
        self.eval_string("result")
    }
}

macro_rules! rt {
    ($name:ident, $src:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let mut p = Page::new();
            let expected = $expected.replace("PORT", &p.port.to_string());
            assert_eq!(p.run($src), expected);
        }
    };
}

// --- WebSocket ---

rt!(
    ws_echo_text,
    "var ws = new WebSocket(WS + '/ws/echo'); ws.onopen = () => ws.send('hola'); ws.onmessage = e => finish(e.data + ' ' + (e instanceof MessageEvent) + ' ' + e.origin);",
    "hola true ws://127.0.0.1:PORT"
);
rt!(
    ws_ready_states,
    "var ws = new WebSocket(WS + '/ws/echo'); log.push(ws.readyState); ws.onopen = () => { log.push(ws.readyState); ws.close(); log.push(ws.readyState); }; ws.onclose = () => { log.push(ws.readyState); finish(log.join('')); };",
    "0123"
);
rt!(
    ws_echo_binary_arraybuffer,
    "var ws = new WebSocket(WS + '/ws/echo'); ws.binaryType = 'arraybuffer'; ws.onopen = () => ws.send(new Uint8Array([1, 2, 3])); ws.onmessage = e => finish((e.data instanceof ArrayBuffer) + ' ' + new Uint8Array(e.data).join(','));",
    "true 1,2,3"
);
rt!(
    ws_echo_binary_blob,
    "var ws = new WebSocket(WS + '/ws/echo'); ws.onopen = () => ws.send(new Blob(['xyz'])); ws.onmessage = e => e.data.text().then(t => finish((e.data instanceof Blob) + ' ' + t));",
    "true xyz"
);
rt!(
    ws_fragmented_and_ping,
    "var ws = new WebSocket(WS + '/ws/fragment'); ws.onmessage = e => finish(e.data);",
    "hola mundo"
);
rt!(
    ws_server_close,
    "var ws = new WebSocket(WS + '/ws/close'); ws.onclose = e => finish([e instanceof CloseEvent, e.wasClean, e.code, e.reason, ws.readyState].join(' '));",
    "true true 4000 bye 3"
);
rt!(
    ws_client_close_code,
    "var ws = new WebSocket(WS + '/ws/echo'); ws.onopen = () => ws.close(3001, 'chau'); ws.onclose = e => finish(e.wasClean + ' ' + e.code + ' ' + e.reason);",
    "true 3001 chau"
);
rt!(
    ws_dropped_connection,
    "var ws = new WebSocket(WS + '/ws/drop'); ws.onerror = () => log.push('error'); ws.onclose = e => finish(log.join() + ' ' + e.wasClean + ' ' + e.code);",
    "error false 1006"
);
rt!(
    ws_bad_accept,
    "var ws = new WebSocket(WS + '/ws/bad-accept'); ws.onopen = () => finish('no'); ws.onerror = () => log.push('error'); ws.onclose = e => finish(log.join() + ' ' + e.code);",
    "error 1006"
);
rt!(
    ws_connection_refused,
    "var ws = new WebSocket('ws://127.0.0.1:1/'); ws.onerror = () => log.push('error'); ws.onclose = e => finish(log.join() + ' ' + e.code + ' ' + ws.readyState);",
    "error 1006 3"
);
rt!(
    ws_subprotocol,
    "var ws = new WebSocket(WS + '/ws/proto', ['chat', 'otro']); ws.onopen = () => finish(ws.protocol + ' [' + ws.extensions + ']');",
    "chat []"
);
rt!(
    ws_handshake_headers,
    "fetch('/x').then(() => { var ws = new WebSocket(WS + '/ws/info', 'p1'); ws.onmessage = e => finish(e.data); });",
    "proto=p1 origin=http://127.0.0.1:PORT cookie=-"
);
rt!(
    ws_pong,
    "var ws = new WebSocket(WS + '/ws/ping'); ws.onmessage = e => finish(e.data);",
    "pong:hey"
);
rt!(
    ws_close_during_connecting,
    "var ws = new WebSocket(WS + '/ws/echo'); ws.close(); ws.onerror = () => log.push('error'); ws.onclose = e => finish(log.join() + ' ' + e.wasClean + ' ' + e.code);",
    "error false 1006"
);
rt!(
    ws_send_while_connecting,
    "var ws = new WebSocket(WS + '/ws/echo'); try { ws.send('x'); finish('no'); } catch (e) { finish(e.name); }",
    "InvalidStateError"
);
rt!(
    ws_bad_url,
    "try { new WebSocket('ftp://x/'); finish('no'); } catch (e) { finish(e.name); }",
    "SyntaxError"
);
rt!(
    ws_bad_close_code,
    "var ws = new WebSocket(WS + '/ws/echo'); try { ws.close(1001); finish('no'); } catch (e) { finish(e.name); }",
    "InvalidAccessError"
);
rt!(
    ws_duplicate_protocols,
    "try { new WebSocket(WS + '/ws/echo', ['a', 'a']); finish('no'); } catch (e) { finish(e.name); }",
    "SyntaxError"
);
rt!(
    ws_http_scheme_and_constants,
    "var ws = new WebSocket(BASE + '/ws/echo'); finish(ws.url.startsWith('ws://') + ' ' + WebSocket.OPEN + ws.CLOSED + ' ' + ws.binaryType);",
    "true 13 blob"
);
rt!(
    ws_buffered_amount,
    "var ws = new WebSocket(WS + '/ws/echo'); ws.onopen = () => { ws.send('12345'); log.push(ws.bufferedAmount); }; ws.onmessage = () => finish(log.join() + ' ' + ws.bufferedAmount);",
    "5 0"
);
rt!(
    ws_many_messages_in_order,
    "var ws = new WebSocket(WS + '/ws/echo'); ws.onopen = () => { for (var i = 0; i < 20; i++) ws.send(String(i)); }; ws.onmessage = e => { log.push(e.data); if (log.length == 20) finish(log.join(',')); };",
    "0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19"
);
rt!(
    ws_large_message,
    "var ws = new WebSocket(WS + '/ws/echo'); var big = 'x'.repeat(40000); ws.onopen = () => ws.send(big); ws.onmessage = e => finish(e.data.length);",
    "40000"
);

// --- EventSource ---

rt!(
    sse_messages,
    "var es = new EventSource('/sse'); es.onmessage = e => { log.push(e.data); if (log.length == 3) { es.close(); finish(log.join('|')); } };",
    "uno|a\nb|café"
);
rt!(
    sse_custom_event_and_id,
    "var es = new EventSource('/sse'); es.addEventListener('custom', e => { es.close(); finish(e.data + ' ' + e.lastEventId + ' ' + e.origin); });",
    "dos 7 http://127.0.0.1:PORT"
);
rt!(
    sse_open_and_ready_state,
    "var es = new EventSource('/sse'); log.push(es.readyState); es.onopen = () => { log.push(es.readyState); es.close(); finish(log.join() + ' ' + es.readyState + ' ' + es.url.endsWith('/sse')); };",
    "0,1 2 true"
);
rt!(
    sse_reconnect_with_last_event_id,
    "var es = new EventSource('/sse'); es.onerror = () => log.push('error' + es.readyState); es.onmessage = e => { if (e.data.startsWith('reconnected')) { es.close(); finish(e.data + ' ' + log.join()); } };",
    "reconnected 7 error0"
);
rt!(
    sse_bad_content_type,
    "var es = new EventSource('/sse-bad'); es.onerror = () => finish('error ' + es.readyState);",
    "error 2"
);
rt!(
    sse_not_found,
    "var es = new EventSource('/sse-404'); es.onerror = () => finish('error ' + es.readyState);",
    "error 2"
);
rt!(
    sse_bad_url,
    "try { new EventSource('http://[x'); finish('no'); } catch (e) { finish(e.name); }",
    "SyntaxError"
);
rt!(
    sse_with_credentials,
    "var es = new EventSource('/sse', {withCredentials: true}); var r = es.withCredentials + ' ' + EventSource.CLOSED; es.close(); finish(r);",
    "true 2"
);

// --- MessageChannel / MessagePort / BroadcastChannel ---

rt!(
    channel_basic,
    "var c = new MessageChannel(); c.port2.onmessage = e => finish(e.data.a + ' ' + (e instanceof MessageEvent)); c.port1.postMessage({a: 'dato'});",
    "dato true"
);
rt!(
    channel_structured_clone,
    "var c = new MessageChannel(); var o = {d: new Date(0), m: new Map([[1, 2]])}; c.port2.onmessage = e => finish((e.data !== o) + ' ' + e.data.d.getTime() + ' ' + e.data.m.get(1)); c.port1.postMessage(o);",
    "true 0 2"
);
rt!(
    channel_start_needed,
    "var c = new MessageChannel(); c.port2.addEventListener('message', e => finish(log.join() + ' ' + e.data)); c.port1.postMessage('x'); setTimeout(() => { log.push('antes'); c.port2.start(); }, 10);",
    "antes x"
);
rt!(
    channel_both_directions,
    "var c = new MessageChannel(); c.port1.onmessage = e => finish('1<-' + e.data); c.port2.onmessage = e => c.port2.postMessage(e.data + '!'); c.port1.postMessage('ping');",
    "1<-ping!"
);
rt!(
    channel_close_stops,
    "var c = new MessageChannel(); c.port2.onmessage = () => finish('no'); c.port1.close(); c.port1.postMessage('x'); setTimeout(() => finish('ok'), 20);",
    "ok"
);
rt!(
    channel_transfer_port,
    "var a = new MessageChannel(), b = new MessageChannel(); a.port2.onmessage = e => { e.ports[0].postMessage('via puerto'); }; b.port2.onmessage = e => finish(e.data); a.port1.postMessage('toma', [b.port1]);",
    "via puerto"
);
rt!(
    channel_data_clone_error,
    "var c = new MessageChannel(); try { c.port1.postMessage(function () {}); finish('no'); } catch (e) { finish(e.name); }",
    "DataCloneError"
);
rt!(
    channel_async_delivery,
    "var c = new MessageChannel(); c.port2.onmessage = () => log.push('msg'); c.port1.postMessage(1); log.push('sync'); setTimeout(() => finish(log.join()), 10);",
    "sync,msg"
);
rt!(
    broadcast_same_name,
    "var a = new BroadcastChannel('canal'), b = new BroadcastChannel('canal'), c = new BroadcastChannel('otro'); c.onmessage = () => finish('no'); a.onmessage = () => finish('no'); b.onmessage = e => finish(e.data + ' ' + b.name + ' ' + e.origin); a.postMessage('x');",
    "x canal http://127.0.0.1:PORT"
);
rt!(
    broadcast_closed_throws,
    "var a = new BroadcastChannel('c'); a.close(); try { a.postMessage(1); finish('no'); } catch (e) { finish(e.name); }",
    "InvalidStateError"
);
rt!(
    message_event_ctor,
    "var e = new MessageEvent('message', {data: 5, origin: 'o', lastEventId: 'i'}); finish([e.type, e.data, e.origin, e.lastEventId, e.source, e.ports.length].join())",
    "message,5,o,i,,0"
);
rt!(
    close_event_ctor,
    "var e = new CloseEvent('close', {code: 1000, reason: 'r', wasClean: true}); finish([e.code, e.reason, e.wasClean].join())",
    "1000,r,true"
);

#[test]
fn broadcast_message_is_delivered() {
    // El `postMessage` del test `broadcast_same_name` necesita mandar el mensaje.
    let mut p = Page::new();
    assert_eq!(
        p.run("var a = new BroadcastChannel('k'), b = new BroadcastChannel('k'); b.onmessage = e => finish(e.data); a.postMessage('hola');"),
        "hola"
    );
}
