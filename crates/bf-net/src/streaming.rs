//! Peticiones HTTP con el cuerpo leído por partes (para `EventSource` y los cuerpos de
//! `fetch()` que llegan de a poco): la cabecera se entrega en cuanto llega y luego cada
//! trozo del cuerpo, por HTTP/1.1 sin compresión. (E2)
//!
//! E3: con un [`Fetcher`] ([`Fetcher::fetch_streaming`], el de los scripts de la ventana), las
//! conexiones quedan abiertas y se reutilizan (RFC 9112 §9.3, keep-alive), con la caché de DNS
//! del `Fetcher`. Antes cada `fetch()` y cada XHR abría su conexión TCP y TLS con
//! `Connection: close`: en la primera visita a GitLab y a El País, 16 conexiones nuevas para
//! 16 peticiones a orígenes que ya tenían la suya.

use std::io::{self, BufRead, BufReader};

use bf_url::{Host, Url};

use super::{
    CookieJar, FetchError, Fetcher, Headers, MAX_REDIRECTS, RedirectMode, RequestOptions,
    USER_AGENT, accept_language, connect, http, http2,
};

/// La cabecera de una respuesta por partes.
#[derive(Debug, Clone)]
pub struct StreamHead {
    pub url: Url,
    pub url_list: Vec<Url>,
    pub status: u16,
    pub status_text: String,
    pub headers: Headers,
}

/// Lo que se hace con cada parte: `false` corta la conexión.
pub trait StreamSink {
    fn head(&mut self, head: &StreamHead) -> bool;
    fn chunk(&mut self, bytes: &[u8]) -> bool;
}

/// Una petición `http(s)` con el cuerpo entregado por partes a `sink`. Sigue las
/// redirecciones según `req.redirect` (Fetch §4.4). Con `jar` y `req.credentials`, cada salto
/// manda sus cookies y guarda sus `Set-Cookie` (que además quedan en `StreamHead::headers`).
pub fn fetch_streaming(
    url: &Url,
    req: &RequestOptions,
    jar: Option<&CookieJar>,
    sink: &mut dyn StreamSink,
) -> Result<(), FetchError> {
    fetch_streaming_with(None, url, req, jar, sink)
}

/// [`fetch_streaming`] con las conexiones del pool de `pool` (si hay).
pub(crate) fn fetch_streaming_with(
    pool: Option<&Fetcher>,
    url: &Url,
    req: &RequestOptions,
    jar: Option<&CookieJar>,
    sink: &mut dyn StreamSink,
) -> Result<(), FetchError> {
    let start = std::time::Instant::now();
    let r = fetch_streaming_inner(pool, url, req, jar, sink);
    // E3: con `FREEDOM_NET_LOG`, también las que fallan (`fetch()`, XHR, `EventSource`).
    if let Err(e) = &r
        && crate::net_log_enabled()
    {
        eprintln!(
            "red: {} {} falló tras {} ms: {e}",
            req.method.to_ascii_uppercase(),
            url.href(),
            start.elapsed().as_millis()
        );
    }
    r
}

fn fetch_streaming_inner(
    pool: Option<&Fetcher>,
    url: &Url,
    req: &RequestOptions,
    jar: Option<&CookieJar>,
    sink: &mut dyn StreamSink,
) -> Result<(), FetchError> {
    let mut url = url.clone();
    let mut list = vec![url.clone()];
    let mut method = req.method.clone();
    let mut body = req.body.clone();
    let mut extra = req.headers.clone();
    let mut referrer = req.referrer.clone();
    for _ in 0..=MAX_REDIRECTS {
        if !matches!(url.scheme.as_str(), "http" | "https") {
            return Err(FetchError::UnsupportedScheme(url.scheme.clone()));
        }
        let tls = url.scheme == "https";
        let host = match &url.host {
            Some(Host::Empty) | None => return Err(FetchError::InvalidUrl),
            Some(h) => h.to_string(),
        };
        let port = url.port.unwrap_or(if tls { 443 } else { 80 });
        let mut headers = Headers::default();
        headers.push("Host", &url.host_string());
        let has = |n: &str| req.headers.get(n).is_some();
        if !has("user-agent") {
            headers.push("User-Agent", USER_AGENT);
        }
        if !has("accept") {
            headers.push("Accept", "*/*");
        }
        if !has("accept-language") {
            headers.push("Accept-Language", &accept_language());
        }
        headers.push("Accept-Encoding", "identity");
        // Sin pool, la conexión es de esta petición.
        if pool.is_none() {
            headers.push("Connection", "close");
        }
        if req.credentials
            && let Some(c) = jar.and_then(|j| j.cookie_header(&url))
        {
            headers.push("Cookie", &c);
        }
        if let Some(r) = &referrer
            && let Some(v) = referer_value(r, &url)
        {
            headers.push("Referer", &v);
        }
        for (n, v) in &extra.0 {
            headers.push(n, v);
        }
        let mut target = url.path_string();
        if target.is_empty() {
            target.push('/');
        }
        if let Some(q) = &url.query {
            target.push('?');
            target.push_str(q);
        }
        if crate::net_log_headers() {
            crate::log_request_headers(&method, &url, &headers);
        }
        let key = (host.clone(), port, tls);
        // RFC 9110 §9.2.2: una conexión reutilizada que el servidor ya cerró se repite con una
        // nueva, solo con un método idempotente.
        let idempotent = ["GET", "HEAD", "OPTIONS", "TRACE", "PUT", "DELETE"]
            .iter()
            .any(|m| method.eq_ignore_ascii_case(m));
        // E3: por la conexión h2 del origen, si la hay o se puede abrir (RFC 9113 §9.1): todas
        // las peticiones de los scripts a un origen van por ella, multiplexadas.
        let mut h2_head = None;
        if let Some(f) = pool
            && let Some(c) = f.h2_for_streaming(&url, &key)
        {
            let r = http2::Request::from_http1(
                &method,
                &url.scheme,
                &target,
                &headers,
                body.as_deref(),
            );
            if let Ok(rx) = c.request_parts(r) {
                match rx.recv() {
                    Ok(http2::Part::Head(status, h)) => {
                        let head = http::Head {
                            version: (2, 0),
                            status,
                            reason: String::new(),
                            headers: h,
                        };
                        h2_head = Some((head, rx));
                    }
                    // §8.7: no se procesó; va por HTTP/1.1.
                    Ok(http2::Part::End(Err(
                        http2::Error::Refused | http2::Error::Http11Required,
                    ))) => {}
                    Ok(http2::Part::End(Err(http2::Error::Lost))) if idempotent => {}
                    Ok(http2::Part::End(Err(e))) => return Err(h2_error(e)),
                    Ok(_) | Err(_) => return Err(h2_error(http2::Error::Lost)),
                }
            }
        }
        if let Some((head, rx)) = h2_head {
            if let Some(done) = deliver_head(
                &mut url,
                &mut list,
                &mut method,
                &mut body,
                &mut extra,
                &mut referrer,
                &head,
                req,
                jar,
                sink,
                "h2",
            )? {
                if done {
                    read_h2_parts(&rx, sink)?;
                }
                return Ok(());
            }
            continue;
        }
        let mut fresh = false;
        let (mut conn, head) = loop {
            let (mut conn, reused) = match pool {
                // Un método que no se puede repetir no se arriesga a una conexión libre que
                // el servidor quizá ya cerró.
                Some(f) => f.stream_connection(&key, !fresh && idempotent)?,
                None => (BufReader::new(connect(&host, port, tls)?), false),
            };
            let sent = (|| -> Result<http::Head, FetchError> {
                http::write_request(conn.get_mut(), &method, &target, &headers, body.as_deref())?;
                let mut head = http::read_head(&mut conn)?;
                while (100..200).contains(&head.status) && head.status != 101 {
                    head = http::read_head(&mut conn)?;
                }
                Ok(head)
            })();
            match sent {
                Ok(head) => break (conn, head),
                Err(_) if reused && idempotent => fresh = true,
                Err(e) => return Err(e),
            }
        };
        match deliver_head(
            &mut url,
            &mut list,
            &mut method,
            &mut body,
            &mut extra,
            &mut referrer,
            &head,
            req,
            jar,
            sink,
            "http/1.1",
        )? {
            None => continue,
            Some(false) => return Ok(()),
            Some(true) => {}
        }
        let kind = http::body_kind(&head, method.eq_ignore_ascii_case("HEAD"));
        let complete = read_parts(&mut conn, kind, sink)?;
        // El cuerpo se leyó entero y el servidor no cierra: la conexión vuelve al pool.
        let close = kind == http::BodyKind::UntilEof
            || head.headers.list("connection").iter().any(|c| c == "close")
            || head.version != (1, 1);
        if let Some(f) = pool
            && complete
            && !close
        {
            f.stream_keep(key, conn);
        }
        return Ok(());
    }
    Err(FetchError::TooManyRedirects)
}

/// E3: lo común a HTTP/1.1 y HTTP/2 con la cabecera de un salto: el registro, las cookies,
/// las redirecciones (Fetch §4.4: prepara el salto siguiente y da `None`) y la entrega a
/// `sink` (`Some(true)` si quiere el cuerpo).
#[allow(clippy::too_many_arguments)]
fn deliver_head(
    url: &mut Url,
    list: &mut Vec<Url>,
    method: &mut String,
    body: &mut Option<Vec<u8>>,
    extra: &mut Headers,
    referrer: &mut Option<Url>,
    head: &http::Head,
    req: &RequestOptions,
    jar: Option<&CookieJar>,
    sink: &mut dyn StreamSink,
    protocol: &str,
) -> Result<Option<bool>, FetchError> {
    // E3: con `FREEDOM_NET_LOG=cabeceras`, las de la respuesta que deciden si el script la
    // puede leer (CORS, tipo, codificación).
    if crate::net_log_headers() {
        let mut out = format!(
            "respuesta: {} {} {}",
            method.to_ascii_uppercase(),
            url.href(),
            head.status
        );
        for (n, v) in &head.headers.0 {
            let n2 = n.to_ascii_lowercase();
            if n2.starts_with("access-control-")
                || matches!(
                    n2.as_str(),
                    "content-type"
                        | "content-encoding"
                        | "content-length"
                        | "transfer-encoding"
                        | "location"
                        | "vary"
                )
            {
                out.push_str(&format!("\n    {n}: {v}"));
            }
        }
        eprintln!("{out}");
    }
    // E3: con `FREEDOM_NET_LOG`, cada salto, como las de `Fetcher::fetch` (estas no pasan
    // por la caché: XHR, `fetch()` y `EventSource` desde los scripts).
    if crate::net_log_enabled() {
        eprintln!(
            "red: {} {} {} {protocol} [red, por partes]",
            method.to_ascii_uppercase(),
            url.href(),
            head.status
        );
    }
    if req.credentials
        && let Some(j) = jar
    {
        for c in head.headers.get_all("set-cookie") {
            j.set_cookie(url, c);
        }
    }
    if matches!(head.status, 301 | 302 | 303 | 307 | 308)
        && req.redirect == RedirectMode::Follow
        && let Some(loc) = head.headers.get("location")
        && let Ok(mut next) = Url::parse(loc, Some(url))
    {
        if next.fragment.is_none() {
            next.fragment.clone_from(&url.fragment);
        }
        // Fetch §4.4 paso 12: 301/302 con POST y 303 (salvo GET/HEAD) pasan a GET sin
        // cuerpo ni cabeceras de contenido.
        let post = method.eq_ignore_ascii_case("POST");
        let get_or_head = method.eq_ignore_ascii_case("GET") || method.eq_ignore_ascii_case("HEAD");
        if (matches!(head.status, 301 | 302) && post) || (head.status == 303 && !get_or_head) {
            *method = "GET".into();
            *body = None;
            extra.0.retain(|(n, _)| {
                !matches!(
                    n.to_ascii_lowercase().as_str(),
                    "content-encoding" | "content-language" | "content-location" | "content-type"
                )
            });
        }
        *referrer = Some(url.clone());
        *url = next;
        list.push(url.clone());
        return Ok(None);
    }
    if head.status >= 300
        && head.status < 400
        && req.redirect == RedirectMode::Error
        && head.headers.get("location").is_some()
    {
        return Err(FetchError::Io(io::Error::other("redirect mode is error")));
    }
    let sh = StreamHead {
        url: url.clone(),
        url_list: list.clone(),
        status: head.status,
        status_text: head.reason.clone(),
        headers: head.headers.clone(),
    };
    if !sink.head(&sh) {
        return Ok(Some(false));
    }
    Ok(Some(true))
}

/// E3: el cuerpo de una respuesta HTTP/2 por partes, trozo a trozo. Si `sink` no quiere más,
/// se suelta el canal y la conexión cancela el flujo.
fn read_h2_parts(
    rx: &std::sync::mpsc::Receiver<http2::Part>,
    sink: &mut dyn StreamSink,
) -> Result<(), FetchError> {
    loop {
        match rx.recv() {
            Ok(http2::Part::Data(d)) => {
                if !sink.chunk(&d) {
                    return Ok(());
                }
            }
            Ok(http2::Part::End(Ok(()))) => return Ok(()),
            Ok(http2::Part::End(Err(e))) => return Err(h2_error(e)),
            Ok(http2::Part::Head(..)) => {}
            Err(_) => return Err(h2_error(http2::Error::Lost)),
        }
    }
}

/// E3: el error de una petición por HTTP/2 como error de red, con la causa (como
/// `Fetcher::send`).
fn h2_error(e: http2::Error) -> FetchError {
    match e {
        http2::Error::Timeout => FetchError::Timeout,
        http2::Error::Io(e) => FetchError::from(e),
        e @ (http2::Error::Lost | http2::Error::Refused) => FetchError::Io(io::Error::new(
            io::ErrorKind::ConnectionReset,
            e.to_string(),
        )),
        e => FetchError::Io(io::Error::new(io::ErrorKind::InvalidData, e.to_string())),
    }
}

/// La cabecera `Referer` (política por defecto `strict-origin-when-cross-origin`,
/// Referrer Policy §3): sin fragmento ni credenciales, solo el origen entre orígenes y nada de
/// https → http.
fn referer_value(r: &Url, url: &Url) -> Option<String> {
    if r.scheme == "https" && url.scheme != "https" {
        return None;
    }
    let mut r = r.clone();
    r.username.clear();
    r.password.clear();
    r.fragment = None;
    if r.origin() != url.origin() {
        r.path = bf_url::Path::List(vec![String::new()]);
        r.query = None;
    }
    Some(r.href())
}

/// Lee el cuerpo según `kind` y entrega cada trozo en cuanto llega. `true` si lo leyó entero
/// con su marco (la conexión queda lista para otra petición).
fn read_parts(
    r: &mut impl BufRead,
    kind: http::BodyKind,
    sink: &mut dyn StreamSink,
) -> Result<bool, FetchError> {
    let mut buf = [0u8; 16 * 1024];
    match kind {
        http::BodyKind::None => {}
        http::BodyKind::Length(mut left) => {
            while left > 0 {
                let want = left.min(buf.len() as u64) as usize;
                let n = r.read(&mut buf[..want])?;
                if n == 0 {
                    return Ok(false);
                }
                left -= n as u64;
                if !sink.chunk(&buf[..n]) {
                    return Ok(false);
                }
            }
        }
        http::BodyKind::UntilEof => loop {
            let n = r.read(&mut buf)?;
            if n == 0 || !sink.chunk(&buf[..n]) {
                return Ok(false);
            }
        },
        http::BodyKind::Chunked => loop {
            let mut line = String::new();
            if r.read_line(&mut line)? == 0 {
                return Ok(false);
            }
            let size_hex = line
                .trim()
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_owned();
            if size_hex.is_empty() {
                continue;
            }
            let mut size = u64::from_str_radix(&size_hex, 16)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad chunk size"))?;
            if size == 0 {
                // Los trailers, hasta la línea vacía (RFC 9112 §7.1.2).
                loop {
                    let mut t = String::new();
                    if r.read_line(&mut t)? == 0 {
                        return Ok(false);
                    }
                    if t == "\r\n" || t == "\n" {
                        break;
                    }
                }
                break;
            }
            while size > 0 {
                let want = size.min(buf.len() as u64) as usize;
                let n = r.read(&mut buf[..want])?;
                if n == 0 {
                    return Ok(false);
                }
                size -= n as u64;
                if !sink.chunk(&buf[..n]) {
                    return Ok(false);
                }
            }
            let mut crlf = String::new();
            r.read_line(&mut crlf)?;
        },
    }
    Ok(true)
}
