//! Un `wptserve` mínimo para el runner de WPT: los archivos se sirven desde el
//! `ResourceLoader` de la página (para cualquier `*.web-platform.test`, como el docroot
//! común de wptserve), con sus `.headers` al lado, y los handlers de Python más usados por
//! `fetch/api` y `xhr` (`status.py`, `content.py`, `inspect-headers.py`, `redirect.py`…) se
//! emulan aquí. Los retardos (`delay`, `trickle`) no esperan.

use bf_infra::Url;

use super::headers::HeaderList;
use super::{LoaderBackend, NetBackend, NetRequest, NetResponse, NetResult};

/// El backend del runner de WPT.
pub struct WptServe {
    pub files: LoaderBackend,
    /// El origen montado (`http://web-platform.test:8000`).
    pub origin: Url,
}

/// La petición vista por un handler.
struct Req<'a> {
    method: &'a str,
    url: &'a Url,
    headers: HeaderList,
    body: Option<&'a [u8]>,
    get: Vec<(String, String)>,
}

impl Req<'_> {
    fn param(&self, k: &str) -> Option<&str> {
        self.get.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }

    fn has(&self, k: &str) -> bool {
        self.get.iter().any(|(n, _)| n == k)
    }

    fn header_or_no(&self, k: &str) -> String {
        self.headers.get(k).unwrap_or_else(|| "NO".into())
    }

    fn query(&self) -> String {
        self.url.query.clone().unwrap_or_default()
    }
}

fn resp<N: AsRef<str>>(status: u16, text: &str, headers: &[(N, String)], body: impl Into<Vec<u8>>) -> NetResponse {
    let mut h = HeaderList::default();
    for (n, v) in headers {
        h.append(n.as_ref(), v);
    }
    let body = body.into();
    if !h.contains("content-length") {
        h.append("Content-Length", &body.len().to_string());
    }
    NetResponse {
        url_list: Vec::new(),
        status,
        status_text: text.to_owned(),
        headers: h,
        body,
    }
}

/// Bytes como *byte string* (cada byte un `char`).
fn latin1(b: &[u8]) -> String {
    b.iter().map(|c| char::from(*c)).collect()
}

/// `urllib.parse.urlencode` (con `quote_plus`).
fn urlencode(pairs: &[(String, String)]) -> String {
    bf_url::form::serialize(pairs)
}

impl WptServe {
    /// El URL equivalente en el origen montado (mismo docroot para todos los hosts).
    fn local(&self, url: &Url) -> Url {
        let mut u = url.clone();
        u.scheme.clone_from(&self.origin.scheme);
        u.host.clone_from(&self.origin.host);
        u.port = self.origin.port;
        u.query = None;
        u.fragment = None;
        u
    }

    fn file(&self, url: &Url) -> Option<NetResponse> {
        let local = self.local(url);
        let req = NetRequest {
            url: local.clone(),
            options: bf_net::RequestOptions::default(),
        };
        let mut r = self.files.fetch(&req).ok()?;
        // `<archivo>.headers` (wptserve: cabeceras extra, una por línea).
        let mut hpath = local.clone();
        if let bf_url::Path::List(segs) = &mut hpath.path
            && let Some(last) = segs.last_mut()
        {
            last.push_str(".headers");
        }
        if let Some(r2) = self.files.0.load(&hpath) {
            for line in String::from_utf8_lossy(&r2.bytes).lines() {
                if let Some((n, v)) = line.split_once(':') {
                    let (n, v) = (n.trim(), v.trim());
                    if n.eq_ignore_ascii_case("content-type") {
                        r.headers.set(n, v);
                    } else {
                        r.headers.append(n, v);
                    }
                }
            }
        }
        Some(r)
    }

    /// Una respuesta (sin seguir redirecciones).
    fn hop(&self, req: &Req<'_>) -> Option<NetResponse> {
        let path = req.url.path_string();
        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path.as_str()));
        let origin_hdr = req.headers.get("origin");
        let body_bytes = req.body.unwrap_or(&[]).to_vec();
        Some(match name {
            "status.py" => {
                let code = req.param("code").and_then(|c| c.parse().ok()).unwrap_or(200);
                let text = req.param("text").unwrap_or("OMG");
                resp(
                    code,
                    text,
                    &[
                        ("Content-Type", req.param("type").unwrap_or("").to_owned()),
                        ("X-Request-Method", req.method.to_owned()),
                    ],
                    req.param("content").unwrap_or("").as_bytes().to_vec(),
                )
            }
            "content.py" => {
                let ct = match req.param("response_charset_label") {
                    Some(l) => format!("text/plain;charset={l}"),
                    None => "text/plain".into(),
                };
                let q = req.query();
                let content = req
                    .param("content")
                    .map_or(body_bytes.clone(), |c| c.as_bytes().to_vec());
                resp(
                    200,
                    "OK",
                    &[
                        ("Content-type", ct),
                        ("X-Request-Method", req.method.to_owned()),
                        ("X-Request-Query", if q.is_empty() { "NO".into() } else { q }),
                        ("X-Request-Content-Length", req.header_or_no("content-length")),
                        ("X-Request-Content-Type", req.header_or_no("content-type")),
                    ],
                    content,
                )
            }
            "echo-content.py" => resp(
                200,
                "OK",
                &[
                    ("X-Request-Method", req.method.to_owned()),
                    ("X-Request-Content-Length", req.header_or_no("content-length")),
                    ("X-Request-Content-Type", req.header_or_no("content-type")),
                    ("Content-Type", "text/plain".into()),
                ],
                body_bytes,
            ),
            "method.py" => {
                let mut h: Vec<(&str, String)> = Vec::new();
                if req.has("cors") {
                    h.push(("Access-Control-Allow-Origin", "*".into()));
                    h.push(("Access-Control-Allow-Credentials", "true".into()));
                    h.push(("Access-Control-Allow-Methods", "GET, POST, PUT, FOO".into()));
                    h.push(("Access-Control-Allow-Headers", "x-test, x-foo".into()));
                    h.push(("Access-Control-Expose-Headers", "x-request-method".into()));
                }
                h.push(("x-request-method", req.method.to_owned()));
                for n in ["Content-Type", "Content-Length", "Content-Encoding", "Content-Language", "Content-Location"] {
                    h.push((
                        match n {
                            "Content-Type" => "x-request-content-type",
                            "Content-Length" => "x-request-content-length",
                            "Content-Encoding" => "x-request-content-encoding",
                            "Content-Language" => "x-request-content-language",
                            _ => "x-request-content-location",
                        },
                        req.header_or_no(n),
                    ));
                }
                resp(200, "OK", &h, body_bytes)
            }
            "inspect-headers.py" => {
                let mut h: Vec<(String, String)> = Vec::new();
                let checked: Vec<String> = req
                    .param("headers")
                    .map(|s| s.split('|').map(str::to_owned).collect())
                    .unwrap_or_default();
                let names: Vec<String> = checked.iter().map(|c| format!("x-request-{c}")).collect();
                for (c, n) in checked.iter().zip(names.iter()) {
                    if let Some(v) = req.headers.get(c) {
                        h.push((n.clone(), v));
                    }
                }
                if req.has("cors") {
                    let o = origin_hdr.clone().unwrap_or_else(|| "*".into());
                    h.push(("Access-Control-Allow-Origin".into(), o));
                    h.push(("Access-Control-Allow-Credentials".into(), "true".into()));
                    h.push(("Access-Control-Allow-Methods".into(), "GET, POST, HEAD".into()));
                    h.push(("Access-Control-Expose-Headers".into(), names.join(", ")));
                    let allow = req.param("allow_headers").map_or_else(
                        || {
                            req.headers
                                .0
                                .iter()
                                .map(|(n, _)| n.to_ascii_lowercase())
                                .collect::<Vec<_>>()
                                .join(", ")
                        },
                        str::to_owned,
                    );
                    h.push(("Access-Control-Allow-Headers".into(), allow));
                }
                h.push(("content-type".into(), "text/plain".into()));
                resp(200, "OK", &h, Vec::new())
            }
            "redirect.py" if dir.ends_with("/xhr/resources") => {
                let code = req.param("code").and_then(|c| c.parse().ok()).unwrap_or(302);
                if req.has("followed") {
                    return Some(resp(200, "OK", &[("Content:Type", "text/plain".into())], b"MAGIC HAPPENED".to_vec()));
                }
                let mut location = req
                    .param("location")
                    .map_or_else(|| format!("{path}?followed"), str::to_owned);
                if location.starts_with("redirect.py") {
                    location.push_str(&format!("&code={code}"));
                }
                resp(code, "WEBSRT MARKETING", &[("Location", location)], b"TEST".to_vec())
            }
            "redirect.py" => {
                let mut h: Vec<(&str, String)> = vec![
                    ("Content-Type", "text/plain".into()),
                    ("Cache-Control", "no-cache".into()),
                    ("Pragma", "no-cache".into()),
                ];
                match &origin_hdr {
                    Some(o) => {
                        h.push(("Access-Control-Allow-Origin", o.clone()));
                        h.push(("Access-Control-Allow-Credentials", "true".into()));
                    }
                    None => h.push(("Access-Control-Allow-Origin", "*".into())),
                }
                if req.method == "OPTIONS" {
                    if let Some(a) = req.param("allow_headers") {
                        h.push(("Access-Control-Allow-Headers", a.to_owned()));
                    }
                    if !req.has("redirect_preflight") {
                        return Some(resp(200, "OK", &h, Vec::new()));
                    }
                }
                let status = req
                    .param("redirect_status")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(302);
                if let Some(loc) = req.param("location") {
                    let mut url = loc.to_owned();
                    if !req.has("simple") {
                        let scheme = url.split_once(':').map(|(s, _)| s.to_ascii_lowercase());
                        let relative = !url.contains(':') || url.starts_with('/');
                        if relative || matches!(scheme.as_deref(), Some("http" | "https")) {
                            url.push(if url.contains('?') { '&' } else { '?' });
                            url.push_str(&urlencode(&req.get));
                            url.push_str("&count=1");
                        }
                    }
                    h.push(("Location", url));
                }
                if let Some(p) = req.param("redirect_referrerpolicy") {
                    h.push(("Referrer-Policy", p.to_owned()));
                }
                resp(status, "Found", &h, Vec::new())
            }
            "trickle.py" => {
                let count: usize = req.param("count").and_then(|c| c.parse().ok()).unwrap_or(50);
                let chunk = b"TEST_TRICKLE\n";
                let mut h = vec![("Content-type", "text/plain".to_owned())];
                if req.has("specifylength") {
                    h.push(("Content-Length", (count * chunk.len()).to_string()));
                }
                resp(200, "OK", &h, chunk.repeat(count))
            }
            "delay.py" => resp(
                200,
                "OK",
                &[
                    ("Access-Control-Allow-Origin", "*".into()),
                    ("Access-Control-Allow-Methods", "YO".into()),
                    ("Content-type", "text/plain".into()),
                ],
                b"TEST_DELAY".to_vec(),
            ),
            "headers.py" if dir.ends_with("/xhr/resources") => resp(
                200,
                "OK",
                &[
                    ("Content-Type", "text/plain".into()),
                    ("X-Custom-Header", "test".into()),
                    ("Set-Cookie", "test".into()),
                    ("Set-Cookie2", "test".into()),
                    ("X-Custom-Header-Empty", String::new()),
                    ("X-Custom-Header-Comma", "1".into()),
                    ("X-Custom-Header-Comma", "2".into()),
                    ("X-Custom-Header-Bytes", latin1("…".as_bytes())),
                ],
                b"TEST".to_vec(),
            ),
            "requri.py" => {
                let body = if req.has("full") {
                    req.url.href()
                } else {
                    let mut p = path.clone();
                    if let Some(q) = &req.url.query {
                        p.push('?');
                        p.push_str(q);
                    }
                    p
                };
                resp(200, "OK", &[("Content-Type", "text/plain".into())], body.into_bytes())
            }
            "corsenabled.py" => {
                let q = req.query();
                let mut h: Vec<(&str, String)> = vec![
                    ("Access-Control-Allow-Origin", "*".into()),
                    ("Access-Control-Allow-Credentials", "true".into()),
                    ("Access-Control-Allow-Methods", "GET, POST, PUT, FOO".into()),
                    ("Access-Control-Allow-Headers", "x-test, x-foo".into()),
                    (
                        "Access-Control-Expose-Headers",
                        "x-request-method, x-request-content-type, x-request-query, x-request-content-length, x-request-data".into(),
                    ),
                ];
                if req.has("safelist_content_type") {
                    h.push(("Access-Control-Allow-Headers", "content-type".into()));
                }
                h.push(("X-Request-Method", req.method.to_owned()));
                h.push(("X-Request-Query", if q.is_empty() { "NO".into() } else { q }));
                h.push(("X-Request-Content-Length", req.header_or_no("content-length")));
                h.push(("X-Request-Content-Type", req.header_or_no("content-type")));
                h.push(("X-Request-Data", latin1(&body_bytes)));
                resp(200, "OK", &h, b"Test".to_vec())
            }
            "access-control-basic-allow.py" => resp(
                200,
                "OK",
                &[
                    ("Content-Type", "text/plain".into()),
                    ("Access-Control-Allow-Credentials", "true".into()),
                    ("Access-Control-Allow-Origin", origin_hdr.unwrap_or_default()),
                ],
                b"PASS: Cross-domain access allowed.".to_vec(),
            ),
            "access-control-basic-allow-star.py" => resp(
                200,
                "OK",
                &[
                    ("Content-Type", "text/plain".into()),
                    ("Access-Control-Allow-Origin", "*".into()),
                ],
                b"PASS: Cross-domain access allowed.".to_vec(),
            ),
            n if n.ends_with(".py") => return None,
            _ => return self.file(req.url),
        })
    }
}

impl NetBackend for WptServe {
    fn fetch(&self, req: &NetRequest) -> NetResult {
        if req.url.scheme == "blob" || req.url.scheme == "data" {
            return NetBackend::fetch(&bf_net::Fetcher::new(), req);
        }
        let mut url = req.url.clone();
        let mut method = req.options.method.clone();
        let mut body = req.options.body.clone();
        let mut list = vec![url.clone()];
        for _ in 0..20 {
            // Las cabeceras que manda el navegador además de las del autor.
            let mut headers = HeaderList(req.options.headers.0.clone());
            headers.append("Host", &url.host_string());
            if !headers.contains("user-agent") {
                headers.append("User-Agent", bf_net::USER_AGENT);
            }
            if !headers.contains("accept") {
                headers.append("Accept", "*/*");
            }
            if !headers.contains("accept-language") {
                headers.append("Accept-Language", "en-US,en;q=0.9");
            }
            if let Some(b) = &body {
                headers.append("Content-Length", &b.len().to_string());
            }
            if let Some(r) = &req.options.referrer {
                let mut r = r.clone();
                r.fragment = None;
                headers.append("Referer", &r.href());
            }
            let get = url
                .query
                .as_deref()
                .map(|q| bf_url::form::parse(q.as_bytes()))
                .unwrap_or_default();
            let r = Req {
                method: &method,
                url: &url,
                headers,
                body: body.as_deref(),
                get,
            };
            let mut res = self
                .hop(&r)
                .ok_or_else(|| format!("wptserve: no se pudo servir {}", url.href()))?;
            if matches!(res.status, 301 | 302 | 303 | 307 | 308)
                && let Some(loc) = res.headers.get("location")
            {
                match req.options.redirect {
                    bf_net::RedirectMode::Manual => {
                        res.url_list = list;
                        return Ok(res);
                    }
                    bf_net::RedirectMode::Error => return Err("redirect mode is error".into()),
                    bf_net::RedirectMode::Follow => {}
                }
                let Ok(next) = Url::parse(&loc, Some(&url)) else {
                    return Err("bad Location".into());
                };
                let post = method.eq_ignore_ascii_case("POST");
                let get_or_head = method.eq_ignore_ascii_case("GET") || method.eq_ignore_ascii_case("HEAD");
                if (matches!(res.status, 301 | 302) && post) || (res.status == 303 && !get_or_head) {
                    method = "GET".into();
                    body = None;
                }
                url = next;
                list.push(url.clone());
                continue;
            }
            res.url_list = list;
            return Ok(res);
        }
        Err("too many redirects".into())
    }
}
