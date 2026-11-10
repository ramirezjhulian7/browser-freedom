//! HTTP contra un servidor local (`std::net::TcpListener`): chunked, keep-alive, cabeceras
//! raras, cuerpo hasta EOF, gzip, redirecciones (cambio de método) y cookies.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use bf_net::{CacheUse, FetchError, FetchOptions, Fetcher, HttpCache, Method, ReferrerPolicy};
use bf_url::Url;

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn read_request(r: &mut BufReader<TcpStream>) -> Option<Request> {
    let mut line = String::new();
    if r.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();
    let mut headers = Vec::new();
    loop {
        let mut l = String::new();
        r.read_line(&mut l).ok()?;
        let l = l.trim_end();
        if l.is_empty() {
            break;
        }
        let (n, v) = l.split_once(':')?;
        headers.push((n.trim().to_ascii_lowercase(), v.trim().to_owned()));
    }
    let len: usize = headers
        .iter()
        .find(|(n, _)| n == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0; len];
    r.read_exact(&mut body).ok()?;
    Some(Request {
        method,
        path,
        headers,
        body,
    })
}

fn gzip(data: &[u8]) -> Vec<u8> {
    // Un gzip "stored" (sin compresión) a mano: cabecera, bloque deflate crudo, CRC y largo.
    let mut out = vec![0x1F, 0x8B, 8, 0, 0, 0, 0, 0, 0, 0xFF];
    out.push(1);
    let len = data.len() as u16;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&(!len).to_le_bytes());
    out.extend_from_slice(data);
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    out.extend_from_slice(&(!crc).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

fn respond(stream: &mut TcpStream, req: &Request) -> bool {
    let header = |name: &str| {
        req.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let simple = |status: &str, extra: &str, body: &str| {
        format!(
            "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    };
    let (bytes, keep) = match req.path.as_str() {
        "/chunked" => (
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nHello\r\n7\r\n, world\r\n0\r\n\r\n"
                .to_vec(),
            true,
        ),
        "/weird" => (
            b"HTTP/1.1 200 Whatever\nX-Dup: a\nX-Dup: b\nContent-Type: text/plain\n\nuntil eof".to_vec(),
            false,
        ),
        "/gzip" => {
            let body = gzip(b"comprimido");
            let mut out = format!(
                "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .into_bytes();
            out.extend(body);
            (out, true)
        }
        "/post-302" => (simple("302 Found", "Location: /method\r\n", ""), true),
        "/post-307" => (simple("307 Temporary Redirect", "Location: method\r\n", ""), true),
        "/method" => {
            let body = format!("{} {}", req.method, String::from_utf8_lossy(&req.body));
            (simple("200 OK", "", &body), true)
        }
        "/loop" => (simple("302 Found", "Location: /loop\r\n", ""), true),
        "/set-cookie" => (
            simple(
                "303 See Other",
                "Set-Cookie: sid=abc; Path=/\r\nSet-Cookie: other=1; Path=/nope\r\nLocation: /echo-cookie\r\n",
                "",
            ),
            true,
        ),
        "/echo-cookie" => (simple("200 OK", "", &header("cookie")), true),
        "/echo-referer" => (simple("200 OK", "", &header("referer")), true),
        "/gz-304" if header("if-none-match") == "\"v1\"" => (
            b"HTTP/1.1 304 Not Modified\r\nContent-Encoding: gzip\r\nETag: \"v1\"\r\n\r\n".to_vec(),
            true,
        ),
        "/gz-304" => {
            let body = gzip(b"guardado");
            let mut out = format!(
                "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nETag: \"v1\"\r\nCache-Control: no-cache\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .into_bytes();
            out.extend(body);
            (out, true)
        }
        "/to-referer" => (simple("302 Found", "Location: /echo-referer\r\n", ""), true),
        "/to-referer-np" => (
            simple(
                "302 Found",
                "Referrer-Policy: unsafe-url, no-referrer, bogus\r\nLocation: /echo-referer\r\n",
                "",
            ),
            true,
        ),
        "/accept" => {
            let body = format!("{}|{}", header("user-agent"), header("accept-encoding"));
            (simple("200 OK", "", &body), true)
        }
        _ => (simple("404 Not Found", "", "no"), true),
    };
    let _ = stream.write_all(&bytes);
    keep
}

fn server() -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        for stream in listener.incoming().take(32) {
            let Ok(stream) = stream else { break };
            thread::spawn(move || {
                let mut writer = stream.try_clone().unwrap();
                let mut reader = BufReader::new(stream);
                while let Some(req) = read_request(&mut reader) {
                    if !respond(&mut writer, &req) {
                        break;
                    }
                }
            });
        }
    });
    (port, handle)
}

fn url(port: u16, path: &str) -> Url {
    Url::parse(&format!("http://127.0.0.1:{port}{path}"), None).unwrap()
}

/// E6: Referrer Policy §8.3 en el `Fetcher`. El referente no cambia con las redirecciones
/// (Fetch §2.2.5) y `Referrer-Policy` en una redirección cambia la política (Fetch §4.4 paso 14).
#[test]
fn referrer_policy_and_redirects() {
    let (port, _server) = server();
    let f = Fetcher::new();
    let get = |path: &str, policy: ReferrerPolicy, from: &str| {
        let opts = FetchOptions {
            referrer: Some(Url::parse(from, None).unwrap()),
            referrer_policy: policy,
            ..FetchOptions::default()
        };
        let body = f.fetch(&url(port, path), &opts).unwrap().body;
        String::from_utf8(body).unwrap()
    };
    let page = format!("http://u:p@127.0.0.1:{port}/pagina?q=1#f");
    let full = format!("http://127.0.0.1:{port}/pagina?q=1");
    let origin = format!("http://127.0.0.1:{port}/");
    assert_eq!(get("/echo-referer", ReferrerPolicy::Default, &page), full);
    assert_eq!(get("/echo-referer", ReferrerPolicy::Origin, &page), origin);
    assert_eq!(get("/echo-referer", ReferrerPolicy::NoReferrer, &page), "");
    // Entre orígenes, por defecto solo el origen.
    assert_eq!(
        get(
            "/echo-referer",
            ReferrerPolicy::Default,
            "http://otro.example/a?b"
        ),
        "http://otro.example/"
    );
    assert_eq!(
        get(
            "/echo-referer",
            ReferrerPolicy::UnsafeUrl,
            "http://otro.example/a?b"
        ),
        "http://otro.example/a?b"
    );
    // Tras una redirección sigue siendo la página, no la URL que redirigió.
    assert_eq!(get("/to-referer", ReferrerPolicy::Default, &page), full);
    // El último token válido de la cabecera de la redirección: `no-referrer`.
    assert_eq!(get("/to-referer-np", ReferrerPolicy::UnsafeUrl, &page), "");
}

/// E6: un 304 con `Content-Encoding: gzip` y sin cuerpo (como los de Wikipedia) valida la
/// respuesta guardada en vez de fallar al descomprimir un cuerpo vacío.
#[test]
fn not_modified_with_content_encoding() {
    let (port, _server) = server();
    let mut f = Fetcher::new();
    f.cache = Some(HttpCache::in_memory());
    let first = f
        .fetch(&url(port, "/gz-304"), &FetchOptions::default())
        .unwrap();
    assert_eq!(
        (first.body.as_slice(), first.cache),
        (&b"guardado"[..], CacheUse::Network)
    );
    let again = f
        .fetch(&url(port, "/gz-304"), &FetchOptions::default())
        .unwrap();
    assert_eq!(again.status, 200);
    assert_eq!(
        (again.body.as_slice(), again.cache),
        (&b"guardado"[..], CacheUse::Revalidated)
    );
}

/// E6: Happy Eyeballs (RFC 8305). El servidor escucha solo en 127.0.0.1; si `localhost` da
/// primero `::1`, el intento por IPv4 sale a los 250 ms en vez de esperar el rechazo de `::1`
/// (en Windows, ~2 s).
#[test]
fn localhost_connects_fast_to_an_ipv4_only_server() {
    let (port, _server) = server();
    let f = Fetcher::new();
    let start = std::time::Instant::now();
    let u = Url::parse(&format!("http://localhost:{port}/echo-cookie"), None).unwrap();
    assert_eq!(f.fetch(&u, &FetchOptions::default()).unwrap().status, 200);
    let took = start.elapsed();
    assert!(took < std::time::Duration::from_millis(1500), "{took:?}");
}

#[test]
fn http_against_local_server() {
    let (port, _server) = server();
    let f = Fetcher::new();
    let get = FetchOptions::default();

    let r = f.fetch(&url(port, "/chunked"), &get).unwrap();
    assert_eq!(r.body, b"Hello, world");
    // Keep-alive: el segundo request reutiliza la conexión.
    let r = f.fetch(&url(port, "/chunked"), &get).unwrap();
    assert_eq!(r.body, b"Hello, world");
    assert_eq!(*f.connections_opened.lock().unwrap(), 1);

    let r = f.fetch(&url(port, "/weird"), &get).unwrap();
    assert_eq!(r.status_text, "Whatever");
    assert_eq!(r.headers.get_all("x-dup").collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!(r.body, b"until eof");

    let r = f.fetch(&url(port, "/gzip"), &get).unwrap();
    assert_eq!(r.body, b"comprimido");

    let post = FetchOptions {
        method: Method::Post,
        body: Some((b"x=1".to_vec(), "application/x-www-form-urlencoded".into())),
        ..FetchOptions::default()
    };
    let r = f.fetch(&url(port, "/post-302"), &post).unwrap();
    assert_eq!(r.body, b"GET ", "302 con POST pasa a GET");
    assert_eq!(r.redirects, 1);
    assert_eq!(r.url.path_string(), "/method");
    let r = f.fetch(&url(port, "/post-307"), &post).unwrap();
    assert_eq!(r.body, b"POST x=1", "307 conserva método y cuerpo");

    assert!(matches!(
        f.fetch(&url(port, "/loop"), &get),
        Err(FetchError::TooManyRedirects)
    ));

    let r = f.fetch(&url(port, "/set-cookie"), &get).unwrap();
    assert_eq!(r.body, b"sid=abc");

    let r = f.fetch(&url(port, "/accept"), &get).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&r.body),
        format!("{}|gzip, deflate, br", bf_net::USER_AGENT)
    );

    let r = f.fetch(&url(port, "/missing"), &get).unwrap();
    assert_eq!(r.status, 404);

    let data = Url::parse("data:text/plain,hi", None).unwrap();
    assert_eq!(f.fetch(&data, &get).unwrap().body, b"hi");
}
