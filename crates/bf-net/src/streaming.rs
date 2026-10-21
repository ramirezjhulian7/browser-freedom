//! Peticiones HTTP con el cuerpo leído por partes (para `EventSource` y los cuerpos de
//! `fetch()` que llegan de a poco): la cabecera se entrega en cuanto llega y luego cada
//! trozo del cuerpo, en una conexión propia (sin pool, sin compresión). (E2)

use std::io::{self, BufRead, BufReader, Read};

use bf_url::{Host, Url};

use super::{
    FetchError, Headers, MAX_REDIRECTS, RedirectMode, RequestOptions, USER_AGENT, accept_language,
    connect, http,
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
/// redirecciones según `req.redirect`. `cookie` es la cabecera `Cookie` a mandar; los
/// `Set-Cookie` quedan en `StreamHead::headers`.
pub fn fetch_streaming(
    url: &Url,
    req: &RequestOptions,
    cookie: Option<&str>,
    sink: &mut dyn StreamSink,
) -> Result<(), FetchError> {
    let mut url = url.clone();
    let mut list = vec![url.clone()];
    let mut method = req.method.clone();
    let mut body = req.body.clone();
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
        let mut conn = BufReader::new(connect(&host, port, tls)?);
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
        headers.push("Connection", "close");
        if req.credentials
            && let Some(c) = cookie
        {
            headers.push("Cookie", c);
        }
        for (n, v) in &req.headers.0 {
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
        http::write_request(conn.get_mut(), &method, &target, &headers, body.as_deref())?;
        let mut head = http::read_head(&mut conn)?;
        while (100..200).contains(&head.status) && head.status != 101 {
            head = http::read_head(&mut conn)?;
        }
        if matches!(head.status, 301 | 302 | 303 | 307 | 308)
            && req.redirect == RedirectMode::Follow
            && let Some(loc) = head.headers.get("location")
            && let Ok(next) = Url::parse(loc, Some(&url))
        {
            let post = method.eq_ignore_ascii_case("POST");
            if (matches!(head.status, 301 | 302) && post)
                || (head.status == 303 && !method.eq_ignore_ascii_case("HEAD"))
            {
                method = "GET".into();
                body = None;
            }
            url = next;
            list.push(url.clone());
            continue;
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
            return Ok(());
        }
        let kind = http::body_kind(&head, method.eq_ignore_ascii_case("HEAD"));
        return read_parts(&mut conn, kind, sink);
    }
    Err(FetchError::TooManyRedirects)
}

/// Lee el cuerpo según `kind` y entrega cada trozo en cuanto llega.
fn read_parts(
    r: &mut impl BufRead,
    kind: http::BodyKind,
    sink: &mut dyn StreamSink,
) -> Result<(), FetchError> {
    let mut buf = [0u8; 16 * 1024];
    match kind {
        http::BodyKind::None => {}
        http::BodyKind::Length(mut left) => {
            while left > 0 {
                let want = left.min(buf.len() as u64) as usize;
                let n = r.read(&mut buf[..want])?;
                if n == 0 {
                    break;
                }
                left -= n as u64;
                if !sink.chunk(&buf[..n]) {
                    return Ok(());
                }
            }
        }
        http::BodyKind::UntilEof => loop {
            let n = r.read(&mut buf)?;
            if n == 0 || !sink.chunk(&buf[..n]) {
                break;
            }
        },
        http::BodyKind::Chunked => loop {
            let mut line = String::new();
            if r.read_line(&mut line)? == 0 {
                break;
            }
            let size_hex = line.trim().split(';').next().unwrap_or("").trim().to_owned();
            if size_hex.is_empty() {
                continue;
            }
            let mut size = u64::from_str_radix(&size_hex, 16)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad chunk size"))?;
            if size == 0 {
                break;
            }
            while size > 0 {
                let want = size.min(buf.len() as u64) as usize;
                let n = r.read(&mut buf[..want])?;
                if n == 0 {
                    return Ok(());
                }
                size -= n as u64;
                if !sink.chunk(&buf[..n]) {
                    return Ok(());
                }
            }
            let mut crlf = String::new();
            r.read_line(&mut crlf)?;
        },
    }
    Ok(())
}
