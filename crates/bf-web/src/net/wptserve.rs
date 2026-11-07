//! Un `wptserve` mínimo para el runner de WPT: los archivos se sirven desde el
//! `ResourceLoader` de la página (para cualquier `*.web-platform.test`, como el docroot
//! común de wptserve), con sus `.headers` al lado, y los handlers de Python más usados por
//! `fetch/api` y `xhr` (`status.py`, `content.py`, `inspect-headers.py`, `redirect.py`…) se
//! emulan aquí, con un stash compartido como el de wptserve (`preflight.py`, `redirect.py`).
//! Los retardos (`delay`, `trickle`) no esperan. [`SubLoader`] aplica las sustituciones de
//! los archivos `*.sub.*`.

use std::collections::HashMap;
use std::sync::Mutex;

use bf_infra::{Resource, ResourceLoader, Url};

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
        self.get
            .iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.as_str())
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

fn resp<N: AsRef<str>>(
    status: u16,
    text: &str,
    headers: &[(N, String)],
    body: impl Into<Vec<u8>>,
) -> NetResponse {
    let mut h = HeaderList::default();
    for (n, v) in headers {
        // Como las verá el parser HTTP: sin espacios a los lados.
        h.append(n.as_ref(), &super::headers::normalize(v));
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
                let code = req
                    .param("code")
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(200);
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
                        (
                            "X-Request-Query",
                            if q.is_empty() { "NO".into() } else { q },
                        ),
                        (
                            "X-Request-Content-Length",
                            req.header_or_no("content-length"),
                        ),
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
                    (
                        "X-Request-Content-Length",
                        req.header_or_no("content-length"),
                    ),
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
                for n in [
                    "Content-Type",
                    "Content-Length",
                    "Content-Encoding",
                    "Content-Language",
                    "Content-Location",
                ] {
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
            "inspect-headers.py" if dir.ends_with("/xhr/resources") => {
                let mut h: Vec<(&str, String)> = Vec::new();
                if req.has("cors") {
                    h.push(("Access-Control-Allow-Origin", "*".into()));
                    h.push(("Access-Control-Allow-Credentials", "true".into()));
                    h.push(("Access-Control-Allow-Methods", "GET, POST, PUT, FOO".into()));
                    h.push(("Access-Control-Allow-Headers", "x-test, x-foo".into()));
                    h.push((
                        "Access-Control-Expose-Headers",
                        "x-request-method, x-request-content-type, x-request-query, x-request-content-length".into(),
                    ));
                }
                h.push(("content-type", "text/plain".into()));
                let fv = req.param("filter_value").unwrap_or("");
                let fname = req.param("filter_name").unwrap_or("").to_ascii_lowercase();
                let mut out = String::new();
                for (n, v) in &req.headers.0 {
                    if !fv.is_empty() {
                        if v == fv {
                            out.push_str(n);
                            out.push(',');
                        }
                    } else if n.to_ascii_lowercase() == fname {
                        out.push_str(&format!(
                            "{n}: {v}
"
                        ));
                    }
                }
                resp(200, "OK", &h, out.into_bytes())
            }
            "message.py" if dir.ends_with("/eventsource/resources") => {
                let mime = req.param("mime").unwrap_or("text/event-stream").to_owned();
                let mut body = req
                    .param("message")
                    .unwrap_or("data: data")
                    .as_bytes()
                    .to_vec();
                if req.param("newline") != Some("none") {
                    body.extend_from_slice(b"\n\n");
                }
                body.push(b'\n');
                resp(200, "OK", &[("Content-Type", mime)], body)
            }
            "last-event-id.py" if dir.ends_with("/eventsource/resources") => {
                let ct = [("Content-Type", "text/event-stream".to_owned())];
                match req.headers.get("last-event-id") {
                    Some(id) if !id.is_empty() => {
                        resp(200, "OK", &ct, format!("data: {id}\n\n").into_bytes())
                    }
                    _ => {
                        let idv = req.param("idvalue").unwrap_or("\u{2026}");
                        let body = format!("id: {idv}\nretry: 200\ndata: hello\n\n");
                        resp(200, "OK", &ct, body.into_bytes())
                    }
                }
            }
            "status-error.py" if dir.ends_with("/eventsource/resources") => {
                let status = req
                    .param("status")
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(404);
                let body: &[u8] = if matches!(status, 204 | 205) {
                    b""
                } else {
                    b"data: data\n\n"
                };
                let ct = [("Content-Type", "text/event-stream".to_owned())];
                resp(status, "HAHAHAHA", &ct, body.to_vec())
            }
            "parse-headers.py" => {
                let mut h: Vec<(&str, String)> = Vec::new();
                if let Some(v) = req.param("my-custom-header") {
                    h.push(("My-Custom-Header", v.to_owned()));
                }
                resp(200, "OK", &h, Vec::new())
            }
            "echo-method.py" => resp(
                200,
                "OK",
                &[("Content-type", "text/plain".into())],
                req.method.as_bytes().to_vec(),
            ),
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
                    h.push((
                        "Access-Control-Allow-Methods".into(),
                        "GET, POST, HEAD".into(),
                    ));
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
                let code = req
                    .param("code")
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(302);
                if req.has("followed") {
                    return Some(resp(
                        200,
                        "OK",
                        &[("Content:Type", "text/plain".into())],
                        b"MAGIC HAPPENED".to_vec(),
                    ));
                }
                let mut location = req
                    .param("location")
                    .map_or_else(|| format!("{path}?followed"), str::to_owned);
                if location.starts_with("redirect.py") {
                    location.push_str(&format!("&code={code}"));
                }
                resp(
                    code,
                    "WEBSRT MARKETING",
                    &[("Location", location)],
                    b"TEST".to_vec(),
                )
            }
            "redirect.py" if dir == "/common" => {
                // `common/redirect.py`: sin CORS salvo con `enable-cors`.
                let status = req
                    .param("status")
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(302);
                let mut h: Vec<(&str, String)> =
                    vec![("Location", req.param("location").unwrap_or("").to_owned())];
                if req.has("enable-cors")
                    && let Some(o) = &origin_hdr
                {
                    h.push(("Content-Type", "text/plain".into()));
                    h.push(("Access-Control-Allow-Origin", o.clone()));
                    h.push(("Access-Control-Allow-Credentials", "true".into()));
                }
                resp(status, "Found", &h, Vec::new())
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
                // El stash del token: cuántas redirecciones lleva y si hubo preflight.
                let token = req.param("token");
                let mut count: u64 = 0;
                let mut preflight = "0".to_owned();
                if let Some(t) = token
                    && let Some(d) = stash_take(&path, t)
                {
                    count = d.get("count").and_then(|c| c.parse().ok()).unwrap_or(0);
                    preflight = d.get("preflight").cloned().unwrap_or(preflight);
                }
                let save = |count: u64, preflight: &str| {
                    if let Some(t) = token {
                        let d: Stashed = [
                            ("count".to_owned(), count.to_string()),
                            ("preflight".to_owned(), preflight.to_owned()),
                        ]
                        .into_iter()
                        .collect();
                        stash_put(&path, t, d);
                    }
                };
                if req.method == "OPTIONS" {
                    if let Some(a) = req.param("allow_headers") {
                        h.push(("Access-Control-Allow-Headers", a.to_owned()));
                    }
                    preflight = "1".into();
                    if !req.has("redirect_preflight") {
                        save(count, &preflight);
                        return Some(resp(200, "OK", &h, Vec::new()));
                    }
                }
                let post: Vec<(String, String)> = bf_url::form::parse(&body_bytes);
                let status = req
                    .param("redirect_status")
                    .or_else(|| {
                        post.iter()
                            .find(|(n, _)| n == "redirect_status")
                            .map(|(_, v)| v.as_str())
                    })
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(302);
                count += 1;
                if let Some(loc) = req.param("location") {
                    let mut url = loc.to_owned();
                    if !req.has("simple") {
                        let scheme = url.split_once(':').map(|(s, _)| s.to_ascii_lowercase());
                        let relative = !url.contains(':') || url.starts_with('/');
                        if relative || matches!(scheme.as_deref(), Some("http" | "https")) {
                            // `urlencode` de los parámetros (la primera aparición de cada uno).
                            let mut params: Vec<(String, String)> = Vec::new();
                            for (k, v) in &req.get {
                                if !params.iter().any(|(n, _)| n == k) {
                                    params.push((k.clone(), v.clone()));
                                }
                            }
                            url.push(if url.contains('?') { '&' } else { '?' });
                            url.push_str(&urlencode(&params));
                            url.push_str(&format!("&count={count}"));
                        }
                    }
                    h.push(("Location", url));
                }
                if let Some(p) = req.param("redirect_referrerpolicy") {
                    h.push(("Referrer-Policy", p.to_owned()));
                }
                save(count, &preflight);
                if let Some(max) = req.param("max_count").and_then(|m| m.parse::<u64>().ok())
                    && token.is_some()
                    && count > max
                {
                    return Some(resp::<&str>(
                        200,
                        "OK",
                        &[],
                        (count - 1).to_string().into_bytes(),
                    ));
                }
                resp(status, "Found", &h, Vec::new())
            }
            "preflight.py" => {
                let mut h: Vec<(&str, String)> = vec![("Content-Type", "text/plain".into())];
                let mut stashed: Stashed = [
                    ("control_request_headers", ""),
                    ("preflight", "0"),
                    ("preflight_referrer", ""),
                ]
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect();
                let token = req.param("token");
                match req.param("origin") {
                    Some(o) => {
                        for o in o.split(", ") {
                            h.push(("Access-Control-Allow-Origin", o.to_owned()));
                        }
                    }
                    None => h.push(("Access-Control-Allow-Origin", "*".into())),
                }
                if req.has("clear-stash") {
                    let had = token.is_some_and(|t| stash_take(&path, t).is_some());
                    return Some(resp(200, "OK", &h, if had { "1" } else { "0" }));
                }
                if req.has("credentials") {
                    h.push(("Access-Control-Allow-Credentials", "true".into()));
                }
                if req.method == "OPTIONS" {
                    if !req.headers.contains("access-control-request-method") {
                        return Some(resp::<&str>(
                            400,
                            "No Access-Control-Request-Method header",
                            &[],
                            "ERROR: No access-control-request-method in preflight!",
                        ));
                    }
                    if req.headers.get("accept").as_deref() != Some("*/*") {
                        return Some(resp::<&str>(
                            400,
                            "Request does not have 'Accept: */*' header",
                            &[],
                            "ERROR: Invalid access in preflight!",
                        ));
                    }
                    if req.has("control_request_headers") {
                        match req.headers.get("access-control-request-headers") {
                            Some(v) => {
                                stashed.insert("control_request_headers".into(), v);
                            }
                            None => {
                                stashed.remove("control_request_headers");
                            }
                        }
                    }
                    for (p, n) in [
                        ("max_age", "Access-Control-Max-Age"),
                        ("allow_headers", "Access-Control-Allow-Headers"),
                        ("allow_methods", "Access-Control-Allow-Methods"),
                    ] {
                        if let Some(v) = req.param(p) {
                            h.push((n, v.to_owned()));
                        }
                    }
                    let status = req
                        .param("preflight_status")
                        .and_then(|c| c.parse().ok())
                        .unwrap_or(200);
                    stashed.insert("preflight".into(), "1".into());
                    stashed.insert(
                        "preflight_referrer".into(),
                        req.headers.get("referer").unwrap_or_default(),
                    );
                    stashed.insert(
                        "preflight_user_agent".into(),
                        req.headers.get("user-agent").unwrap_or_default(),
                    );
                    if let Some(t) = token {
                        stash_put(&path, t, stashed);
                    }
                    return Some(resp(status, "OK", &h, ""));
                }
                if let Some(t) = token
                    && let Some(d) = stash_take(&path, t)
                {
                    stashed = d;
                }
                if req.has("checkUserAgentHeaderInPreflight")
                    && req.headers.get("user-agent").as_deref()
                        != stashed.get("preflight_user_agent").map(String::as_str)
                {
                    return Some(resp(
                        400,
                        "OK",
                        &h,
                        "ERROR: No user-agent header in preflight",
                    ));
                }
                h.push((
                    "Access-Control-Expose-Headers",
                    "x-did-preflight, x-control-request-headers, x-referrer, x-preflight-referrer, x-origin".into(),
                ));
                h.push((
                    "x-did-preflight",
                    stashed.get("preflight").cloned().unwrap_or_default(),
                ));
                if let Some(v) = stashed.get("control_request_headers") {
                    h.push(("x-control-request-headers", v.clone()));
                }
                h.push((
                    "x-preflight-referrer",
                    stashed
                        .get("preflight_referrer")
                        .cloned()
                        .unwrap_or_default(),
                ));
                h.push(("x-referrer", req.headers.get("referer").unwrap_or_default()));
                h.push(("x-origin", origin_hdr.clone().unwrap_or_default()));
                if let Some(t) = token {
                    stash_put(&path, t, stashed);
                }
                resp(200, "OK", &h, "")
            }
            "clean-stash.py" => {
                let had = req
                    .param("token")
                    .is_some_and(|t| stash_take(&path, t).is_some());
                resp::<&str>(200, "OK", &[], if had { "1" } else { "0" })
            }
            "trickle.py" => {
                let count: usize = req
                    .param("count")
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(50);
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
                resp(
                    200,
                    "OK",
                    &[("Content-Type", "text/plain".into())],
                    body.into_bytes(),
                )
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
                h.push((
                    "X-Request-Query",
                    if q.is_empty() { "NO".into() } else { q },
                ));
                h.push((
                    "X-Request-Content-Length",
                    req.header_or_no("content-length"),
                ));
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
                    (
                        "Access-Control-Allow-Origin",
                        origin_hdr.unwrap_or_default(),
                    ),
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
            _ => {
                let mut r = self.file(req.url)?;
                if let Some(p) = req.param("pipe") {
                    apply_pipes(&mut r, p, req.url);
                }
                return Some(r);
            }
        })
    }
}

impl NetBackend for WptServe {
    fn fetch(&self, req: &NetRequest) -> NetResult {
        if req.url.scheme == "blob" || req.url.scheme == "data" {
            return NetBackend::fetch(&bf_net::Fetcher::new(), req);
        }
        // E3: `x.any.worker.js`, generado a partir de `x.any.js` (como wptserve).
        let path = req.url.path_string();
        if let Some(stem) = path.strip_suffix(".any.worker.js") {
            let mut any = req.url.clone();
            any.set_pathname(&format!("{stem}.any.js"));
            any.query = None;
            let src = self.files.fetch(&NetRequest {
                url: any,
                options: bf_net::RequestOptions::default(),
            })?;
            let text = crate::html::testharness::any_worker_script(
                &String::from_utf8_lossy(&src.body),
                &format!("{stem}.any.js"),
            );
            let mut headers = HeaderList::default();
            headers.append("Content-Type", "text/javascript; charset=utf-8");
            return Ok(NetResponse {
                url_list: vec![req.url.clone()],
                status: 200,
                status_text: "OK".into(),
                headers,
                body: text.into_bytes(),
            });
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
                let get_or_head =
                    method.eq_ignore_ascii_case("GET") || method.eq_ignore_ascii_case("HEAD");
                if (matches!(res.status, 301 | 302) && post) || (res.status == 303 && !get_or_head)
                {
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

// --- El stash de wptserve y las sustituciones `.sub.` ---

/// Un valor del stash (los handlers guardan diccionarios de cadenas).
type Stashed = HashMap<String, String>;

/// `request.server.stash` (wptserve `stash.py`): por `(ruta, clave)`; `take` lo saca.
static STASH: Mutex<Option<HashMap<(String, String), Stashed>>> = Mutex::new(None);

fn stash_put(path: &str, key: &str, v: Stashed) {
    STASH
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert((path.to_owned(), key.to_owned()), v);
}

fn stash_take(path: &str, key: &str) -> Option<Stashed> {
    STASH
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .remove(&(path.to_owned(), key.to_owned()))
}

/// Un `ResourceLoader` que aplica las sustituciones de wptserve (`{{host}}`,
/// `{{ports[http][0]}}`, `{{domains[www1]}}`…, el *pipe* `sub`) a los archivos `*.sub.*`,
/// como `/common/get-host-info.sub.js`.
pub struct SubLoader<L>(pub L);

impl<L: ResourceLoader> ResourceLoader for SubLoader<L> {
    fn load(&self, url: &Url) -> Option<Resource> {
        let mut r = self.0.load(url)?;
        let name = url.path_string();
        let name = name.rsplit('/').next().unwrap_or("");
        if name.contains(".sub.")
            && !name.ends_with(".headers")
            && let Ok(text) = std::str::from_utf8(&r.bytes)
        {
            r.bytes = substitute(text, url).into_bytes();
        }
        Some(r)
    }
}

/// El host de WPT del runner (`{{host}}`).
const HOST: &str = "web-platform.test";

/// Una expresión `{{…}}` de wptserve (`sub` pipe), o `None` si no se conoce.
fn sub_value(expr: &str, url: &Url) -> Option<String> {
    let expr = expr.trim();
    if expr == "host" {
        return Some(HOST.into());
    }
    if expr == "uuid()" {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return Some(format!("00000000-0000-4000-8000-{n:012x}"));
    }
    let args: Vec<&str> = expr
        .split('[')
        .skip(1)
        .map(|a| a.trim_end_matches(']'))
        .collect();
    let head = expr.split('[').next().unwrap_or("");
    let sub = |s: &str, base: &str| {
        if s.is_empty() {
            base.to_owned()
        } else {
            format!("{s}.{base}")
        }
    };
    match (head, args.as_slice()) {
        ("domains", [d]) => Some(sub(d, HOST)),
        ("hosts", [alt, d]) => Some(sub(
            d,
            if *alt == "alt" {
                "not-web-platform.test"
            } else {
                HOST
            },
        )),
        ("ports", [scheme, i]) => {
            let i: usize = i.parse().ok()?;
            let ports: &[u16] = match *scheme {
                "http" => &[8000, 8001],
                "https" => &[8443, 8444],
                "ws" => &[8000],
                "wss" => &[8443],
                "h2" => &[9000],
                _ => return None,
            };
            ports.get(i).map(u16::to_string)
        }
        ("location", [part]) => Some(match *part {
            "scheme" => url.scheme.clone(),
            "host" => url.host_string(),
            "hostname" => url
                .host
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
            "port" => url.port.map(|p| p.to_string()).unwrap_or_default(),
            "path" => url.path_string(),
            "query" => url
                .query
                .as_ref()
                .map(|q| format!("?{q}"))
                .unwrap_or_default(),
            "server" => format!("{}://{}", url.scheme, url.host_string()),
            _ => url.href(),
        }),
        ("GET", [k]) => {
            let q = url.query.as_deref().unwrap_or("");
            bf_url::form::parse(q.as_bytes())
                .into_iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v)
        }
        _ => None,
    }
}

/// Aplica el pipe `sub` a `text` (las expresiones desconocidas quedan igual).
fn substitute(text: &str, url: &Url) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        match after.find("}}") {
            Some(j) => {
                match sub_value(&after[..j], url) {
                    Some(v) => out.push_str(&v),
                    None => out.push_str(&rest[i..i + 2 + j + 2]),
                }
                rest = &after[j + 2..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

// --- Los pipes de wptserve (`?pipe=header(n,v)|status(c)|sub`) ---

/// Separa `text` por `sep` fuera de paréntesis y respetando `\\` como escape.
fn split_top(text: &str, sep: char) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut depth = 0;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(n) = chars.next() {
                    out.last_mut().expect("parte").push(n);
                }
                continue;
            }
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if c == sep && depth == 0 {
            out.push(String::new());
        } else {
            out.last_mut().expect("parte").push(c);
        }
    }
    out
}

/// Aplica los pipes de wptserve que se conocen a una respuesta de archivo: `header(n, v[,
/// True])` (reemplaza o agrega), `status(c)` y `sub`.
fn apply_pipes(r: &mut NetResponse, pipes: &str, url: &Url) {
    for pipe in split_top(pipes, '|') {
        let (name, args) = match pipe.split_once('(') {
            Some((n, a)) => (n.trim(), a.strip_suffix(')').unwrap_or(a)),
            None => (pipe.trim(), ""),
        };
        let args: Vec<String> = split_top(args, ',')
            .into_iter()
            .map(|a| a.trim().to_owned())
            .collect();
        match (name, args.as_slice()) {
            ("header", [n, v]) => r.headers.set(n, v),
            ("header", [n, v, append]) if append.eq_ignore_ascii_case("true") => {
                r.headers.append(n, v);
            }
            ("header", [n, v, _]) => r.headers.set(n, v),
            ("status", [c]) => {
                if let Ok(c) = c.parse() {
                    r.status = c;
                }
            }
            ("sub", _) => {
                if let Ok(t) = std::str::from_utf8(&r.body) {
                    r.body = substitute(t, url).into_bytes();
                    r.headers.set("Content-Length", &r.body.len().to_string());
                }
            }
            _ => {}
        }
    }
}
