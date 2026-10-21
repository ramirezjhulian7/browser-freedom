//! La red desde JavaScript (`bf_web::net`): `fetch`, `Request`/`Response`/`Headers`,
//! `XMLHttpRequest`, `FormData`, `Blob`/`File`/`FileReader` y las URLs `blob:`, contra un
//! servidor HTTP local (`TcpListener`). La página vive en `http://127.0.0.1:<puerto>/`; el
//! "otro origen" es `http://localhost:<puerto>/`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bf_js::host::DefaultHooks;
use bf_web::{DocumentOwner, Runtime, Url, WebContext, WebContextHandle};

// --- El servidor ---

struct Req {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Req {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn read_req(stream: &mut BufReader<TcpStream>) -> Option<Req> {
    let mut line = String::new();
    stream.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        stream.read_line(&mut h).ok()?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((n, v)) = h.split_once(':') {
            headers.push((n.trim().to_owned(), v.trim().to_owned()));
        }
    }
    let len = headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; len];
    stream.read_exact(&mut body).ok()?;
    Some(Req {
        method,
        path,
        headers,
        body,
    })
}

fn respond(s: &mut TcpStream, status: &str, headers: &[(&str, String)], body: &[u8]) {
    let mut out = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (n, v) in headers {
        out.push_str(&format!("{n}: {v}\r\n"));
    }
    out.push_str("\r\n");
    let _ = s.write_all(out.as_bytes());
    let _ = s.write_all(body);
}

fn handle(stream: TcpStream, port: u16) {
    let mut r = BufReader::new(stream);
    let Some(req) = read_req(&mut r) else { return };
    let mut s = r.into_inner();
    let page_origin = format!("http://127.0.0.1:{port}");
    let text = |ct: &str| vec![("Content-Type", ct.to_owned())];
    let path = req.path.split('?').next().unwrap_or("").to_owned();
    match path.as_str() {
        "/text" => respond(&mut s, "200 OK", &text("text/plain"), b"hola"),
        "/json" => respond(
            &mut s,
            "200 OK",
            &text("application/json"),
            br#"{"a":1,"b":[true,null]}"#,
        ),
        "/html" => respond(
            &mut s,
            "200 OK",
            &text("text/html"),
            b"<!DOCTYPE html><title>T</title><p id=x>uno</p>",
        ),
        "/latin1" => respond(
            &mut s,
            "200 OK",
            &text("text/plain;charset=windows-1252"),
            b"caf\xe9",
        ),
        "/echo" => {
            let mut out = format!("{} {}\n", req.method, req.path);
            for (n, v) in &req.headers {
                let l = n.to_ascii_lowercase();
                if l.starts_with("x-") || l == "content-type" || l == "cookie" || l == "origin" {
                    out.push_str(&format!("{l}: {v}\n"));
                }
            }
            out.push('\n');
            let mut body = out.into_bytes();
            body.extend_from_slice(&req.body);
            respond(&mut s, "200 OK", &text("text/plain"), &body);
        }
        "/redirect" => respond(&mut s, "302 Found", &[("Location", "/text".into())], b""),
        "/redirect303" => respond(
            &mut s,
            "303 See Other",
            &[("Location", "/echo".into())],
            b"",
        ),
        "/missing" => respond(&mut s, "404 Not Found", &text("text/plain"), b"no"),
        "/empty" => respond(&mut s, "204 No Content", &[], b""),
        "/set-cookie" => respond(
            &mut s,
            "200 OK",
            &[
                ("Set-Cookie", "sabor=chocolate; Path=/".into()),
                ("Content-Type", "text/plain".into()),
            ],
            b"ok",
        ),
        "/headers" => respond(
            &mut s,
            "200 OK",
            &[
                ("Content-Type", "text/plain".into()),
                ("X-Uno", "1".into()),
                ("X-Dos", "a".into()),
                ("x-dos", "b".into()),
                ("Set-Cookie", "c=d".into()),
            ],
            b"h",
        ),
        "/cors" => {
            let origin = req.header("origin").unwrap_or("").to_owned();
            respond(
                &mut s,
                "200 OK",
                &[
                    ("Content-Type", "text/plain".into()),
                    ("Access-Control-Allow-Origin", origin),
                    ("X-Secreto", "s".into()),
                    ("Access-Control-Expose-Headers", "X-Visible".into()),
                    ("X-Visible", "v".into()),
                ],
                b"cors",
            );
        }
        "/no-cors" => respond(&mut s, "200 OK", &text("text/plain"), b"privado"),
        "/slow" => {
            std::thread::sleep(Duration::from_millis(400));
            respond(&mut s, "200 OK", &text("text/plain"), b"lento");
        }
        "/page.html" => respond(&mut s, "200 OK", &text("text/html"), b"<p>pagina</p>"),
        _ => {
            let _ = page_origin;
            respond(&mut s, "404 Not Found", &[], b"");
        }
    }
}

fn server() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = l.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            std::thread::spawn(move || handle(s, port));
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
    fn new(threaded: bool) -> Page {
        let port = server();
        let mut rt = Runtime::new(Box::new(DefaultHooks));
        let html = "<!DOCTYPE html><html><body><form id=f><input name=a value=1>\
            <input name=b value=x type=checkbox checked><input name=c value=no type=checkbox>\
            <select name=s><option>uno<option selected value=2>dos</select>\
            <textarea name=t>l1\nl2</textarea><button name=go value=si>Ir</button></form></body></html>";
        let (doc, _) = bf_html::parse_document(html, &bf_html::ParseOptions::default());
        let realm = rt.main_realm();
        let url = Url::parse(&format!("http://127.0.0.1:{port}/page.html"), None).expect("url");
        let ctx = bf_web::install(
            &mut rt.agent,
            realm,
            WebContext::new(DocumentOwner::Document(Box::new(doc)), url),
        );
        // Una jarra propia por test.
        bf_web::net::set_backend(
            &mut rt.agent,
            realm,
            Arc::new(bf_net::Fetcher::new()),
            threaded,
        );
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

    /// Corre `src` (con `PORT` y `OTHER` definidos) hasta que llama a `finish`.
    fn run(&mut self, src: &str) -> String {
        let realm = self.ctx.realm;
        let code = format!(
            "var PORT = {port}; var OTHER = 'http://localhost:{port}';\n\
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
            std::thread::sleep(Duration::from_millis(5));
        }
        self.eval_string("result")
    }
}

macro_rules! net {
    ($name:ident, $src:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let mut p = Page::new(true);
            let expected = $expected.replace("PORT", &p.port.to_string());
            assert_eq!(p.run($src), expected);
        }
    };
}

// --- fetch ---

net!(
    fetch_text,
    "fetch('/text').then(r => r.text()).then(finish, fail)",
    "hola"
);
net!(
    fetch_json,
    "fetch('/json').then(r => r.json()).then(j => finish(j.a + ',' + j.b[0]), fail)",
    "1,true"
);
net!(
    fetch_status,
    "fetch('/missing').then(r => finish(r.status + ' ' + r.ok + ' ' + r.statusText), fail)",
    "404 false Not Found"
);
net!(
    fetch_response_props,
    "fetch('/text').then(r => finish([r.type, r.url, r.redirected, r instanceof Response, r.headers.get('content-type')].join('|')), fail)",
    "basic|http://127.0.0.1:PORT/text|false|true|text/plain"
);
net!(
    fetch_post_body,
    "fetch('/echo', {method: 'post', body: 'cuerpo'}).then(r => r.text()).then(t => finish(t.split('\\n')[0] + '|' + t.split('\\n\\n')[1] + '|' + /content-type: (.*)/.exec(t)[1]), fail)",
    "POST /echo|cuerpo|text/plain;charset=UTF-8"
);
net!(
    fetch_custom_header,
    "fetch('/echo', {headers: {'X-Prueba': 'abc'}}).then(r => r.text()).then(t => finish(/x-prueba: (.*)/.exec(t)[1]), fail)",
    "abc"
);
net!(
    fetch_forbidden_header_dropped,
    "fetch('/echo', {headers: [['Cookie', 'x=y'], ['X-Ok', '1']]}).then(r => r.text()).then(t => finish(/cookie/.test(t) + ' ' + /x-ok: 1/.test(t)), fail)",
    "false true"
);
net!(
    fetch_redirect,
    "fetch('/redirect').then(r => r.text().then(t => finish(t + ' ' + r.redirected + ' ' + r.url.endsWith('/text'))), fail)",
    "hola true true"
);
net!(
    fetch_redirect_303_to_get,
    "fetch('/redirect303', {method: 'POST', body: 'x'}).then(r => r.text()).then(t => finish(t.split('\\n')[0]), fail)",
    "GET /echo"
);
net!(
    fetch_redirect_manual,
    "fetch('/redirect', {redirect: 'manual'}).then(r => finish(r.type + ' ' + r.status), fail)",
    "opaqueredirect 0"
);
net!(
    fetch_redirect_error,
    "fetch('/redirect', {redirect: 'error'}).then(() => finish('no'), e => finish(e.name))",
    "TypeError"
);
net!(
    fetch_network_error,
    "fetch('http://127.0.0.1:1/').then(() => finish('no'), e => finish(e.name + ' ' + (e instanceof TypeError)))",
    "TypeError true"
);
net!(
    fetch_abort_before,
    "var c = new AbortController(); c.abort(); fetch('/text', {signal: c.signal}).then(() => finish('no'), e => finish(e.name))",
    "AbortError"
);
net!(
    fetch_abort_during,
    "var c = new AbortController(); fetch('/slow', {signal: c.signal}).then(() => finish('no'), e => finish(e.name)); c.abort();",
    "AbortError"
);
net!(
    fetch_abort_reason,
    "var c = new AbortController(); fetch('/slow', {signal: c.signal}).then(() => finish('no'), e => finish(e)); c.abort('porque');",
    "porque"
);
net!(
    fetch_cors_ok,
    "fetch(OTHER + '/cors').then(r => r.text().then(t => finish([r.type, t, r.headers.get('x-secreto'), r.headers.get('x-visible')].join('|'))), fail)",
    "cors|cors||v"
);
net!(
    fetch_cors_fail,
    "fetch(OTHER + '/no-cors').then(() => finish('no'), e => finish(e.name))",
    "TypeError"
);
net!(
    fetch_no_cors_opaque,
    "fetch(OTHER + '/no-cors', {mode: 'no-cors'}).then(r => r.text().then(t => finish(r.type + ' ' + r.status + ' [' + t + ']')), fail)",
    "opaque 0 []"
);
net!(
    fetch_same_origin_mode,
    "fetch(OTHER + '/cors', {mode: 'same-origin'}).then(() => finish('no'), e => finish(e.name))",
    "TypeError"
);
net!(
    fetch_cookies,
    "fetch('/set-cookie').then(() => fetch('/echo')).then(r => r.text()).then(t => finish(/cookie: (.*)/.exec(t)[1]), fail)",
    "sabor=chocolate"
);
net!(
    fetch_credentials_omit,
    "fetch('/set-cookie').then(() => fetch('/echo', {credentials: 'omit'})).then(r => r.text()).then(t => finish(/cookie/.test(t)), fail)",
    "false"
);
net!(
    fetch_set_cookie_hidden,
    "fetch('/headers').then(r => finish(r.headers.get('set-cookie') + ' ' + r.headers.get('x-dos')), fail)",
    "null a, b"
);
net!(
    fetch_body_used,
    "fetch('/text').then(r => r.text().then(() => r.text())).then(() => finish('no'), e => finish(e.name))",
    "TypeError"
);
net!(
    fetch_array_buffer,
    "fetch('/text').then(r => r.arrayBuffer()).then(b => finish(b.byteLength + ' ' + new Uint8Array(b)[0]), fail)",
    "4 104"
);
net!(
    fetch_blob,
    "fetch('/text').then(r => r.blob()).then(b => finish(b.size + ' ' + b.type), fail)",
    "4 text/plain"
);
net!(
    fetch_data_url,
    "fetch('data:text/plain,hola%20data').then(r => r.text()).then(finish, fail)",
    "hola data"
);
net!(
    fetch_blob_url,
    "var u = URL.createObjectURL(new Blob(['de blob'], {type: 'text/x-prueba'})); fetch(u).then(r => r.text().then(t => finish(t + ' ' + r.headers.get('content-type') + ' ' + u.startsWith('blob:http://127.0.0.1:' + PORT + '/'))), fail)",
    "de blob text/x-prueba true"
);
net!(
    fetch_revoked_blob_url,
    "var u = URL.createObjectURL(new Blob(['x'])); URL.revokeObjectURL(u); fetch(u).then(() => finish('no'), e => finish(e.name))",
    "TypeError"
);
net!(
    fetch_formdata_body,
    "var fd = new FormData(); fd.append('k', 'v'); fd.append('f', new Blob(['contenido'], {type: 'text/plain'}), 'a.txt'); fetch('/echo', {method: 'POST', body: fd}).then(r => r.text()).then(t => finish(/multipart\\/form-data; boundary=/.test(t) + ' ' + /name=\"k\"\\r\\n\\r\\nv\\r\\n/.test(t) + ' ' + /filename=\"a.txt\"\\r\\nContent-Type: text\\/plain\\r\\n\\r\\ncontenido/.test(t)), fail)",
    "true true true"
);
net!(
    fetch_urlsearchparams_body,
    "fetch('/echo', {method: 'POST', body: new URLSearchParams('a=1&b=dos tres')}).then(r => r.text()).then(t => finish(t.split('\\n\\n')[1] + '|' + /content-type: (.*)/.exec(t)[1]), fail)",
    "a=1&b=dos+tres|application/x-www-form-urlencoded;charset=UTF-8"
);
net!(
    fetch_request_object,
    "fetch(new Request('/echo', {method: 'PUT', body: 'r'})).then(r => r.text()).then(t => finish(t.split('\\n')[0] + ' ' + t.split('\\n\\n')[1]), fail)",
    "PUT /echo r"
);
net!(
    fetch_empty_204,
    "fetch('/empty').then(r => r.text().then(t => finish(r.status + '[' + t + ']')), fail)",
    "204[]"
);

// --- Request / Response / Headers (sin red) ---

net!(
    headers_basic,
    "var h = new Headers({'B': '2', 'a': '1'}); h.append('a', '3'); finish([...h].join(';') + ' ' + h.get('A') + ' ' + h.has('b'))",
    "a,1, 3;b,2 1, 3 true"
);
net!(
    headers_invalid,
    "try { new Headers({'bad name': 'x'}); finish('no'); } catch (e) { finish(e.name); }",
    "TypeError"
);
net!(
    headers_immutable,
    "fetch('/text').then(r => { try { r.headers.set('x', 'y'); finish('no'); } catch (e) { finish(e.name); } }, fail)",
    "TypeError"
);
net!(
    headers_set_cookie_list,
    "var h = new Headers([['Set-Cookie', 'a=1'], ['set-cookie', 'b=2']]); finish(h.getSetCookie().join('|') + ' ' + [...h.keys()].join(','))",
    "a=1|b=2 set-cookie,set-cookie"
);
net!(
    headers_request_guard,
    "var r = new Request('/x', {headers: {'Host': 'malo', 'X-Bien': 'si'}}); finish(r.headers.has('host') + ' ' + r.headers.get('x-bien'))",
    "false si"
);
net!(
    request_defaults,
    "var r = new Request('/a?b#c'); finish([r.method, r.url, r.mode, r.credentials, r.cache, r.redirect, r.referrer, r.signal instanceof AbortSignal].join('|'))",
    "GET|http://127.0.0.1:PORT/a?b#c|cors|same-origin|default|follow|about:client|true"
);
net!(
    request_get_with_body,
    "try { new Request('/x', {body: 'a'}); finish('no'); } catch (e) { finish(e.name); }",
    "TypeError"
);
net!(
    request_bad_method,
    "try { new Request('/x', {method: 'TRACE'}); finish('no'); } catch (e) { finish(e.name); }",
    "TypeError"
);
net!(
    request_clone_and_text,
    "var r = new Request('/x', {method: 'POST', body: 'dato'}); var c = r.clone(); Promise.all([r.text(), c.text()]).then(v => finish(v.join(',') + ' ' + r.bodyUsed), fail)",
    "dato,dato true"
);
net!(
    response_constructor,
    "var r = new Response('cuerpo', {status: 201, statusText: 'Creado', headers: {'X-A': 'b'}}); r.text().then(t => finish([t, r.status, r.statusText, r.headers.get('x-a'), r.headers.get('content-type'), r.type].join('|')), fail)",
    "cuerpo|201|Creado|b|text/plain;charset=UTF-8|default"
);
net!(
    response_bad_status,
    "try { new Response('', {status: 99}); finish('no'); } catch (e) { finish(e.name); }",
    "RangeError"
);
net!(
    response_static,
    "var e = Response.error(); var r = Response.redirect('/z', 301); Response.json({x: 1}).json().then(j => finish([e.type, e.status, r.status, r.headers.get('location'), j.x].join('|')), fail)",
    "error|0|301|http://127.0.0.1:PORT/z|1"
);
net!(
    response_form_data,
    "new Response('a=1&b=2', {headers: {'Content-Type': 'application/x-www-form-urlencoded'}}).formData().then(fd => finish(fd.get('a') + fd.get('b')), fail)",
    "12"
);

// --- XMLHttpRequest ---

net!(
    xhr_get,
    "var x = new XMLHttpRequest(); x.onload = () => finish(x.status + ' ' + x.responseText + ' ' + x.readyState); x.onerror = fail; x.open('GET', '/text'); x.send();",
    "200 hola 4"
);
net!(
    xhr_ready_states,
    "var x = new XMLHttpRequest(); x.onreadystatechange = () => log.push(x.readyState); x.onloadend = () => finish(log.join('')); x.open('GET', '/text'); x.send();",
    "1234"
);
net!(
    xhr_event_order,
    "var x = new XMLHttpRequest(); ['loadstart','progress','load','loadend','readystatechange'].forEach(t => x.addEventListener(t, e => log.push(t + (t == 'readystatechange' ? x.readyState : '')))); x.addEventListener('loadend', () => finish(log.join(','))); x.open('GET', '/text'); x.send();",
    "readystatechange1,loadstart,readystatechange2,readystatechange3,progress,readystatechange4,load,loadend"
);
net!(
    xhr_progress_event,
    "var x = new XMLHttpRequest(); x.onload = e => finish([e instanceof ProgressEvent, e.lengthComputable, e.loaded, e.total, e.type].join('|')); x.open('GET', '/text'); x.send();",
    "true|true|4|4|load"
);
net!(
    xhr_post_and_headers,
    "var x = new XMLHttpRequest(); x.onload = () => finish(x.responseText.split('\\n')[0] + '|' + /x-a: (.*)/.exec(x.responseText)[1] + '|' + x.responseText.split('\\n\\n')[1]); x.open('POST', '/echo'); x.setRequestHeader('X-A', 'uno'); x.setRequestHeader('x-a', 'dos'); x.send('cuerpo');",
    "POST /echo|uno, dos|cuerpo"
);
net!(
    xhr_response_headers,
    "var x = new XMLHttpRequest(); x.onload = () => finish(JSON.stringify(x.getAllResponseHeaders()) + ' ' + x.getResponseHeader('X-DOS')); x.open('GET', '/headers'); x.send();",
    "\"connection: close\\r\\ncontent-length: 1\\r\\ncontent-type: text/plain\\r\\nx-dos: a, b\\r\\nx-uno: 1\\r\\n\" a, b"
);
net!(
    xhr_json,
    "var x = new XMLHttpRequest(); x.responseType = 'json'; x.onload = () => finish(x.response.b.length + ' ' + typeof x.response); x.open('GET', '/json'); x.send();",
    "2 object"
);
net!(
    xhr_arraybuffer,
    "var x = new XMLHttpRequest(); x.open('GET', '/text'); x.responseType = 'arraybuffer'; x.onload = () => finish(x.response.byteLength + ' ' + (x.response instanceof ArrayBuffer)); x.send();",
    "4 true"
);
net!(
    xhr_blob,
    "var x = new XMLHttpRequest(); x.open('GET', '/text'); x.responseType = 'blob'; x.onload = () => finish(x.response.size + ' ' + x.response.type); x.send();",
    "4 text/plain"
);
net!(
    xhr_document,
    "var x = new XMLHttpRequest(); x.open('GET', '/html'); x.responseType = 'document'; x.onload = () => finish(x.response.getElementById('x').textContent + ' ' + x.responseXML.title); x.send();",
    "uno T"
);
net!(
    xhr_charset,
    "var x = new XMLHttpRequest(); x.onload = () => finish(x.responseText); x.open('GET', '/latin1'); x.send();",
    "café"
);
net!(
    xhr_override_mime,
    "var x = new XMLHttpRequest(); x.onload = () => finish(x.responseText.length); x.open('GET', '/latin1'); x.overrideMimeType('text/plain;charset=utf-8'); x.send();",
    "4"
);
net!(
    xhr_404,
    "var x = new XMLHttpRequest(); x.onload = () => finish(x.status + ' ' + x.statusText); x.open('GET', '/missing'); x.send();",
    "404 Not Found"
);
net!(
    xhr_network_error,
    "var x = new XMLHttpRequest(); x.onerror = e => finish(e.type + ' ' + x.readyState + ' ' + x.status); x.onload = () => finish('no'); x.open('GET', 'http://127.0.0.1:1/'); x.send();",
    "error 4 0"
);
net!(
    xhr_abort,
    "var x = new XMLHttpRequest(); ['abort', 'loadend', 'readystatechange'].forEach(t => x.addEventListener(t, () => log.push(t + x.readyState))); x.open('GET', '/slow'); x.send(); x.abort(); finish(log.join(',') + ' ' + x.readyState);",
    "readystatechange1,readystatechange4,abort4,loadend4 0"
);
net!(
    xhr_timeout,
    "var x = new XMLHttpRequest(); x.ontimeout = e => finish(e.type + ' ' + x.readyState); x.onload = () => finish('no'); x.open('GET', '/slow'); x.timeout = 50; x.send();",
    "timeout 4"
);
net!(
    xhr_sync,
    "var x = new XMLHttpRequest(); x.open('GET', '/text', false); x.send(); finish(x.status + ' ' + x.responseText + ' ' + x.readyState);",
    "200 hola 4"
);
net!(
    xhr_sync_error,
    "var x = new XMLHttpRequest(); x.open('GET', 'http://127.0.0.1:1/', false); try { x.send(); finish('no'); } catch (e) { finish(e.name); }",
    "NetworkError"
);
net!(
    xhr_invalid_state,
    "var x = new XMLHttpRequest(); try { x.send(); finish('no'); } catch (e) { finish(e.name); }",
    "InvalidStateError"
);
net!(
    xhr_bad_method,
    "var x = new XMLHttpRequest(); try { x.open('TRACE', '/'); finish('no'); } catch (e) { finish(e.name); }",
    "SecurityError"
);
net!(
    xhr_response_url_redirect,
    "var x = new XMLHttpRequest(); x.onload = () => finish(x.responseURL.endsWith('/text') + ' ' + x.responseText); x.open('GET', '/redirect'); x.send();",
    "true hola"
);
net!(
    xhr_upload_events,
    "var x = new XMLHttpRequest(); ['loadstart', 'progress', 'load', 'loadend'].forEach(t => x.upload.addEventListener(t, e => log.push('u' + t + e.total))); x.onloadend = () => finish(log.join(',')); x.open('POST', '/echo'); x.send('12345');",
    "uloadstart5,uprogress5,uload5,uloadend5"
);
net!(
    xhr_upload_same_object,
    "var x = new XMLHttpRequest(); finish(x.upload === x.upload && x.upload instanceof XMLHttpRequestUpload && x instanceof XMLHttpRequestEventTarget)",
    "true"
);
net!(
    xhr_cors_fail,
    "var x = new XMLHttpRequest(); x.onerror = () => finish('error ' + x.status); x.onload = () => finish('no'); x.open('GET', OTHER + '/no-cors'); x.send();",
    "error 0"
);
net!(
    xhr_constants,
    "finish([XMLHttpRequest.UNSENT, XMLHttpRequest.DONE, new XMLHttpRequest().readyState].join())",
    "0,4,0"
);

// --- FormData ---

net!(
    formdata_methods,
    "var fd = new FormData(); fd.append('a', '1'); fd.append('a', '2'); fd.set('b', '3'); fd.delete('zz'); finish([fd.get('a'), fd.getAll('a').join('+'), fd.has('b'), [...fd.keys()].join('')].join('|'))",
    "1|1+2|true|aab"
);
net!(
    formdata_from_form,
    "var fd = new FormData(document.getElementById('f')); finish([...fd].map(e => e.join('=')).join('&'))",
    "a=1&b=x&s=2&t=l1\r\nl2"
);
net!(
    formdata_file_entry,
    "var fd = new FormData(); fd.append('f', new Blob(['xy']), 'n.bin'); var f = fd.get('f'); finish([f instanceof File, f.name, f.size].join('|'))",
    "true|n.bin|2"
);
net!(
    formdata_not_a_form,
    "try { new FormData(document.body); finish('no'); } catch (e) { finish(e.name); }",
    "TypeError"
);

// --- Blob / File / FileReader ---

net!(
    blob_parts,
    "var b = new Blob(['ab', new Uint8Array([99]), new Blob(['d'])], {type: 'Text/Plain'}); b.text().then(t => finish(t + ' ' + b.size + ' ' + b.type), fail)",
    "abcd 4 text/plain"
);
net!(
    blob_slice,
    "new Blob(['abcdef']).slice(1, -1, 'x/y').text().then(finish, fail)",
    "bcde"
);
net!(
    file_props,
    "var f = new File(['x'], 'nombre.txt', {type: 'text/plain', lastModified: 42}); finish([f.name, f.lastModified, f.size, f instanceof Blob].join('|'))",
    "nombre.txt|42|1|true"
);
net!(
    file_reader_text,
    "var r = new FileReader(); r.onload = () => finish(r.result + ' ' + r.readyState); r.readAsText(new Blob(['leído']));",
    "leído 2"
);
net!(
    file_reader_data_url,
    "var r = new FileReader(); r.onloadend = () => finish(r.result); r.readAsDataURL(new Blob(['hola'], {type: 'text/plain'}));",
    "data:text/plain;base64,aG9sYQ=="
);
net!(
    file_reader_events,
    "var r = new FileReader(); ['loadstart', 'progress', 'load', 'loadend'].forEach(t => r.addEventListener(t, () => log.push(t))); r.onloadend = () => finish(log.join(',')); r.readAsArrayBuffer(new Blob(['x']));",
    "loadstart,progress,load,loadend"
);
net!(
    file_reader_abort,
    "var r = new FileReader(); r.onabort = () => log.push('abort'); r.onload = () => log.push('load'); r.readAsText(new Blob(['x'])); r.abort(); setTimeout(() => finish(log.join(',') + ' ' + r.readyState + ' ' + r.result), 20);",
    "abort 2 null"
);

// --- ReadableStream como cuerpo ---

net!(
    stream_body_reader,
    "fetch('/text').then(r => { var rd = r.body.getReader(); return rd.read().then(c => finish(c.value.length + ' ' + (c.value instanceof Uint8Array) + ' ' + r.bodyUsed + ' ' + r.body.locked)); }).catch(fail)",
    "4 true true true"
);
net!(
    stream_body_used_after_text,
    "fetch('/text').then(r => r.text().then(() => { try { r.body.getReader(); finish('no'); } catch (e) { finish(e.name + ' ' + r.bodyUsed); } })).catch(fail)",
    "TypeError true"
);
net!(
    stream_response_from_stream,
    "var s = new ReadableStream({start(c) { c.enqueue(new TextEncoder().encode('a')); c.enqueue(new TextEncoder().encode('bc')); c.close(); }}); new Response(s).text().then(finish, fail)",
    "abc"
);
net!(
    stream_clone_tee,
    "var r = new Response('dup'); var c = r.clone(); Promise.all([r.text(), c.text()]).then(v => finish(v.join('+')), fail)",
    "dup+dup"
);
net!(
    stream_async_iteration,
    "(async () => { var s = new ReadableStream({start(c) { c.enqueue(1); c.enqueue(2); c.close(); }}); var t = 0; for await (var v of s) t += v; finish(t); })().catch(fail)",
    "3"
);
net!(
    stream_bad_chunk,
    "new Response(new ReadableStream({start(c) { c.enqueue('no bytes'); c.close(); }})).text().then(() => finish('no'), e => finish(e.name))",
    "TypeError"
);
net!(
    stream_upload,
    "var s = new ReadableStream({start(c) { c.enqueue(new TextEncoder().encode('flujo')); c.close(); }}); fetch('/echo', {method: 'POST', body: s, duplex: 'half'}).then(r => r.text()).then(t => finish(t.split('\\n\\n')[1]), fail)",
    "flujo"
);
net!(
    stream_upload_needs_duplex,
    "try { new Request('/x', {method: 'POST', body: new ReadableStream()}); finish('no'); } catch (e) { finish(e.name); }",
    "TypeError"
);

// --- sendBeacon y backend sin hilo ---

net!(
    send_beacon,
    "finish(navigator.sendBeacon('/echo', 'dato') + ' ' + navigator.sendBeacon('/echo', 'x'.repeat(70000)))",
    "true false"
);

#[test]
fn synchronous_backend() {
    // Sin hilo auxiliar (tests, runner): la petición se hace al momento y la promesa se
    // resuelve en una tarea posterior.
    let mut p = Page::new(false);
    assert_eq!(p.run("var s = 'antes'; fetch('/text').then(r => r.text()).then(t => finish(s + ' ' + t)); s = 'despues';"), "despues hola");
}

#[test]
fn port_placeholder_is_replaced() {
    // `PORT` en las expectativas de arriba se compara literalmente: los tests que lo usan
    // construyen la URL con el puerto real.
    let mut p = Page::new(true);
    let r = p.run("finish(location.href)");
    assert!(r.starts_with("http://127.0.0.1:"), "{r}");
}
