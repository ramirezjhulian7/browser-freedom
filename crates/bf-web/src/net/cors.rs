//! "Main fetch" con CORS (Fetch §4.1, §4.3, §4.4, §4.8, §4.9): el *response tainting* de cada
//! salto, el preflight `OPTIONS` y su caché, el CORS check, las redirecciones hechas aquí salto
//! a salto (el backend recibe siempre `redirect: manual`) y la cabecera `Origin` (§3.1).
//!
//! No toca el agente: corre en el hilo de la red (o en el acto, sin hilo). El resultado lleva
//! el tainting con el que `fetch()`/XHR filtran la respuesta (§2.2.6, "filtered response").

use std::sync::Mutex;
use std::time::{Duration, Instant};

use bf_infra::Url;
use bf_net::streaming::{StreamHead, StreamSink};

use super::headers::{self, HeaderList};
use super::{NetBackend, NetRequest, NetResponse};

/// "Response tainting" (Fetch §2.2.5) y el tipo de la respuesta filtrada que da.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tainting {
    #[default]
    Basic,
    Cors,
    Opaque,
    /// Una redirección con `redirect: "manual"` (respuesta "opaqueredirect").
    OpaqueRedirect,
}

impl Tainting {
    /// El `Response.type` de la respuesta filtrada.
    pub fn response_type(self) -> &'static str {
        match self {
            Tainting::Basic => "basic",
            Tainting::Cors => "cors",
            Tainting::Opaque => "opaque",
            Tainting::OpaqueRedirect => "opaqueredirect",
        }
    }
}

/// Lo que main fetch necesita de un request (Fetch §2.2.5).
#[derive(Debug, Clone)]
pub struct FetchParams {
    pub url: Url,
    pub method: String,
    /// El header list del autor (sin las cabeceras que agrega el navegador).
    pub headers: HeaderList,
    pub body: Option<Vec<u8>>,
    /// El cuerpo vino de un `ReadableStream` (su *source* es null): no sobrevive a una
    /// redirección que no sea 303 (§4.4 paso 11).
    pub body_from_stream: bool,
    /// `cors`, `no-cors`, `same-origin`, `navigate` o `websocket`.
    pub mode: String,
    /// `omit`, `same-origin` o `include`.
    pub credentials: String,
    pub redirect: bf_net::RedirectMode,
    /// El origen serializado del cliente (`"null"` si es opaco).
    pub origin: String,
    pub referrer: Option<Url>,
    pub referrer_policy: String,
    /// "Use-CORS-preflight flag" (XHR con listeners de subida).
    pub use_preflight: bool,
}

/// La cabecera de la respuesta final de main fetch, sin filtrar, con su tainting.
#[derive(Debug, Clone)]
pub struct FetchHead {
    pub url_list: Vec<Url>,
    pub status: u16,
    pub status_text: String,
    pub headers: HeaderList,
    pub tainting: Tainting,
}

/// Quien recibe la respuesta de main fetch: la cabecera y luego el cuerpo por partes.
/// `false` corta la conexión.
pub trait FetchSink {
    fn head(&mut self, head: FetchHead) -> bool;
    fn chunk(&mut self, bytes: &[u8]) -> bool;
}

// --- Orígenes ---

/// El origen serializado de `url` (el de la URL de dentro para `blob:`, §3.1 de File API).
pub fn url_origin(url: &Url) -> String {
    if url.scheme == "file" {
        return "file://".into();
    }
    if url.scheme == "blob"
        && let Ok(inner) = Url::parse(&url.path_string(), None)
        && matches!(inner.scheme.as_str(), "http" | "https" | "file")
    {
        return inner.origin().to_string();
    }
    url.origin().to_string()
}

/// "Same origin" entre `url` y un origen serializado (los opacos nunca coinciden).
fn same_origin(url: &Url, origin: &str) -> bool {
    origin != "null" && url.scheme != "data" && url_origin(url) == origin
}

/// "Includes credentials" (URL §4.1).
fn includes_credentials(url: &Url) -> bool {
    !url.username.is_empty() || !url.password.is_empty()
}

/// "Byte-serializing a request origin" (§3.1): `null` si el request tiene un "redirect-tainted
/// origin" (§2.2.5: un salto entre orígenes distintos del del request).
fn serialized_request_origin(origin: &str, list: &[Url]) -> String {
    let mut last: Option<&Url> = None;
    for url in list {
        if let Some(l) = last
            && url_origin(url) != url_origin(l)
            && !same_origin(l, origin)
        {
            return "null".into();
        }
        last = Some(url);
    }
    origin.to_owned()
}

// --- Cabeceras CORS (§2.2.2, §3.2) ---

/// "CORS-safelisted method".
pub fn is_safelisted_method(m: &str) -> bool {
    matches!(m, "GET" | "HEAD" | "POST")
}

/// "CORS-unsafe request-header names": ordenados, en minúsculas y sin repetir.
pub fn unsafe_header_names(list: &HeaderList) -> Vec<String> {
    let mut unsafe_names = Vec::new();
    let mut potentially = Vec::new();
    let mut size = 0;
    for (n, v) in &list.0 {
        if headers::is_cors_safelisted(n, v) {
            potentially.push(n.to_ascii_lowercase());
            size += v.len();
        } else {
            unsafe_names.push(n.to_ascii_lowercase());
        }
    }
    if size > 1024 {
        unsafe_names.extend(potentially);
    }
    unsafe_names.sort();
    unsafe_names.dedup();
    unsafe_names
}

/// "Extracting header list values" de una lista `#token` (`Access-Control-Allow-Methods`,
/// `-Headers`): `Ok(None)` si falta, `Err` si un elemento no es un token.
fn extract_tokens(list: &HeaderList, name: &str) -> Result<Option<Vec<String>>, ()> {
    let Some(v) = list.get(name) else {
        return Ok(None);
    };
    let mut out = Vec::new();
    for item in v.split(',') {
        let item = item.trim_matches([' ', '\t']);
        if item.is_empty() {
            continue;
        }
        if !headers::is_token(item) {
            return Err(());
        }
        out.push(item.to_owned());
    }
    Ok(Some(out))
}

/// "CORS check" (§4.9).
pub fn cors_check(origin: &str, credentials_include: bool, headers: &HeaderList) -> bool {
    let Some(acao) = headers.get("access-control-allow-origin") else {
        return false;
    };
    if !credentials_include && acao == "*" {
        return true;
    }
    if acao != origin {
        return false;
    }
    if !credentials_include {
        return true;
    }
    headers.get("access-control-allow-credentials").as_deref() == Some("true")
}

// --- La caché de preflight (§4.8) ---

struct CacheEntry {
    origin: String,
    url: String,
    credentials: bool,
    /// Un método o un nombre de cabecera (en minúsculas), o `*`.
    method: Option<String>,
    header: Option<String>,
    expires: Instant,
}

static CACHE: Mutex<Vec<CacheEntry>> = Mutex::new(Vec::new());

fn with_cache<R>(f: impl FnOnce(&mut Vec<CacheEntry>) -> R) -> R {
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    c.retain(|e| e.expires > now);
    f(&mut c)
}

struct CacheKey<'a> {
    origin: &'a str,
    url: String,
    credentials: bool,
}

impl CacheKey<'_> {
    fn matches(&self, e: &CacheEntry) -> bool {
        e.origin == self.origin && e.url == self.url && e.credentials == self.credentials
    }

    /// "Method cache entry match".
    fn method_match(&self, method: &str) -> bool {
        with_cache(|c| {
            c.iter().any(|e| {
                self.matches(e)
                    && e.method
                        .as_deref()
                        .is_some_and(|m| m == method || (m == "*" && !self.credentials))
            })
        })
    }

    /// "Header-name cache entry match".
    fn header_match(&self, name: &str) -> bool {
        with_cache(|c| {
            c.iter().any(|e| {
                self.matches(e)
                    && e.header.as_deref().is_some_and(|h| {
                        h.eq_ignore_ascii_case(name)
                            || (h == "*" && !self.credentials && !is_non_wildcard(name))
                    })
            })
        })
    }

    fn store(&self, method: Option<&str>, header: Option<&str>, max_age: Duration) {
        let expires = Instant::now() + max_age;
        with_cache(|c| {
            if let Some(e) = c.iter_mut().find(|e| {
                self.matches(e)
                    && e.method.as_deref() == method
                    && e.header.as_deref().map(str::to_ascii_lowercase)
                        == header.map(str::to_ascii_lowercase)
            }) {
                e.expires = expires;
                return;
            }
            c.push(CacheEntry {
                origin: self.origin.to_owned(),
                url: self.url.clone(),
                credentials: self.credentials,
                method: method.map(str::to_owned),
                header: header.map(str::to_ascii_lowercase),
                expires,
            });
        });
    }

    /// "Clear cache entries".
    fn clear(&self) {
        with_cache(|c| c.retain(|e| !(e.origin == self.origin && e.url == self.url)));
    }
}

/// "Determine request's referrer" (Referrer Policy §8.3) para la URL actual: la URL de
/// referencia sin fragmento ni credenciales, solo su origen, o nada, según la política
/// (`""` es la de por defecto, `strict-origin-when-cross-origin`).
pub fn referrer_for(p: &FetchParams, current: &Url) -> Option<Url> {
    let r = p.referrer.as_ref()?;
    if !matches!(r.scheme.as_str(), "http" | "https") {
        return None;
    }
    let mut full = r.clone();
    full.username.clear();
    full.password.clear();
    full.fragment = None;
    let mut origin_only = full.clone();
    origin_only.path = bf_url::Path::List(vec![String::new()]);
    origin_only.query = None;
    let same = url_origin(r) == url_origin(current);
    let downgrade = r.scheme == "https" && current.scheme != "https";
    match p.referrer_policy.as_str() {
        "no-referrer" => None,
        "origin" => Some(origin_only),
        "unsafe-url" => Some(full),
        "strict-origin" => (!downgrade).then_some(origin_only),
        "same-origin" => same.then_some(full),
        "origin-when-cross-origin" => Some(if same { full } else { origin_only }),
        "no-referrer-when-downgrade" => (!downgrade).then_some(full),
        _ if same => Some(full),
        _ => (!downgrade).then_some(origin_only),
    }
}

/// "CORS non-wildcard request-header name".
fn is_non_wildcard(name: &str) -> bool {
    name.eq_ignore_ascii_case("authorization")
}

/// "CORS-preflight fetch" (§4.8). `Err` = error de red.
fn preflight(
    backend: &dyn NetBackend,
    p: &FetchParams,
    url: &Url,
    method: &str,
    list: &HeaderList,
    origin: &str,
    key: &CacheKey<'_>,
) -> Result<(), String> {
    let include = p.credentials == "include";
    let mut h = bf_net::Headers::default();
    h.push("Accept", "*/*");
    h.push("Access-Control-Request-Method", method);
    let names = unsafe_header_names(list);
    if !names.is_empty() {
        h.push("Access-Control-Request-Headers", &names.join(","));
    }
    h.push("Origin", origin);
    let req = NetRequest {
        url: url.clone(),
        options: bf_net::RequestOptions {
            method: "OPTIONS".into(),
            headers: h,
            body: None,
            redirect: bf_net::RedirectMode::Manual,
            credentials: false,
            referrer: referrer_for(p, url),
        },
    };
    let res = backend.fetch(&req)?;
    let fail = |why: &str| -> Result<(), String> {
        key.clear();
        Err(format!("CORS preflight failed: {why}"))
    };
    if !cors_check(origin, include, &res.headers) || !(200..300).contains(&res.status) {
        return fail("CORS check");
    }
    let Ok(mut methods) = extract_tokens(&res.headers, "access-control-allow-methods") else {
        return fail("Access-Control-Allow-Methods");
    };
    let Ok(header_names) = extract_tokens(&res.headers, "access-control-allow-headers") else {
        return fail("Access-Control-Allow-Headers");
    };
    if methods.is_none() && p.use_preflight {
        methods = Some(vec![method.to_owned()]);
    }
    let methods = methods.unwrap_or_default();
    let header_names = header_names.unwrap_or_default();
    let star_m = methods.iter().any(|m| m == "*");
    if !methods.iter().any(|m| m == method) && !is_safelisted_method(method) && (include || !star_m)
    {
        return fail("method not allowed");
    }
    let has = |n: &str| header_names.iter().any(|h| h.eq_ignore_ascii_case(n));
    let star_h = header_names.iter().any(|h| h == "*");
    for (n, _) in &list.0 {
        if is_non_wildcard(n) && !has(n) {
            return fail("Authorization not allowed");
        }
    }
    for n in &names {
        if !has(n) && (include || !star_h) {
            return fail("header not allowed");
        }
    }
    // Access-Control-Max-Age (delta-seconds), 5 s por defecto y como mucho 2 h.
    let max_age = res
        .headers
        .get("access-control-max-age")
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
        .map(|v| v.parse().unwrap_or(u64::MAX))
        .unwrap_or(5u64)
        .min(7200);
    if max_age > 0 {
        let d = Duration::from_secs(max_age);
        for m in &methods {
            key.store(Some(m), None, d);
        }
        for n in &header_names {
            key.store(None, Some(n), d);
        }
    }
    Ok(())
}

// --- Main fetch ---

/// Cómo terminó un salto.
enum Hop {
    /// La respuesta final ya se entregó.
    Done,
    /// Una redirección a seguir (`Location` sin resolver).
    Redirect {
        status: u16,
        location: String,
    },
    Error(String),
}

/// El sink de un salto: decide si la respuesta es final o una redirección, hace el CORS
/// check y reenvía la respuesta final a `out`.
struct HopSink<'a> {
    out: &'a mut dyn FetchSink,
    url_list: Vec<Url>,
    tainting: Tainting,
    origin: String,
    include: bool,
    redirect: bf_net::RedirectMode,
    result: Option<Hop>,
    forwarding: bool,
}

impl StreamSink for HopSink<'_> {
    fn head(&mut self, h: &StreamHead) -> bool {
        let headers = HeaderList(h.headers.0.clone());
        // HTTP fetch (§4.3) paso 4.5: el CORS check se aplica a toda respuesta, también a
        // las redirecciones.
        if self.tainting == Tainting::Cors && !cors_check(&self.origin, self.include, &headers) {
            self.result = Some(Hop::Error("CORS check failed".into()));
            return false;
        }
        let mut tainting = self.tainting;
        if matches!(h.status, 301 | 302 | 303 | 307 | 308) {
            match self.redirect {
                bf_net::RedirectMode::Error => {
                    self.result = Some(Hop::Error("redirect mode is \"error\"".into()));
                    return false;
                }
                bf_net::RedirectMode::Manual => tainting = Tainting::OpaqueRedirect,
                bf_net::RedirectMode::Follow => {
                    if let Some(loc) = headers.get("location") {
                        self.result = Some(Hop::Redirect {
                            status: h.status,
                            location: loc,
                        });
                        return false;
                    }
                }
            }
        }
        self.forwarding = true;
        self.result = Some(Hop::Done);
        self.out.head(FetchHead {
            url_list: self.url_list.clone(),
            status: h.status,
            status_text: h.status_text.clone(),
            headers,
            tainting,
        })
    }

    fn chunk(&mut self, bytes: &[u8]) -> bool {
        self.forwarding && self.out.chunk(bytes)
    }
}

/// "Main fetch" (§4.1) con sus redirecciones: entrega a `out` la respuesta final sin filtrar
/// y su tainting. `Err` = error de red (con el motivo para la consola).
pub fn main_fetch(
    backend: &dyn NetBackend,
    p: &FetchParams,
    out: &mut dyn FetchSink,
) -> Result<(), String> {
    let mut url = p.url.clone();
    let mut list = vec![url.clone()];
    let mut method = p.method.clone();
    let mut headers = p.headers.clone();
    let mut body = p.body.clone();
    let mut tainting = Tainting::Basic;
    let mut redirects = 0;
    loop {
        if super::is_bad_port(&url) {
            return Err(format!("bad port {}", url.href()));
        }
        let origin = serialized_request_origin(&p.origin, &list);
        // La cabecera `Origin` de un documento `file:` es `null` (su origen es opaco).
        let origin_header = if origin == "file://" {
            "null".to_owned()
        } else {
            origin.clone()
        };
        let http = matches!(url.scheme.as_str(), "http" | "https");
        // Main fetch paso 12.
        if (same_origin(&url, &p.origin) && tainting == Tainting::Basic)
            || url.scheme == "data"
            || matches!(p.mode.as_str(), "navigate" | "websocket")
        {
            tainting = Tainting::Basic;
        } else if p.mode == "same-origin" {
            return Err("cross-origin request in same-origin mode".into());
        } else if p.mode == "no-cors" {
            if p.redirect != bf_net::RedirectMode::Follow {
                return Err("no-cors request with a redirect mode other than follow".into());
            }
            tainting = Tainting::Opaque;
        } else if !http {
            return Err(format!("cross-origin request to {}", url.scheme));
        } else {
            tainting = Tainting::Cors;
        }
        let include = p.credentials == "include"
            || (p.credentials == "same-origin" && tainting == Tainting::Basic);
        // HTTP fetch paso 4.1: el preflight, si hace falta y no está en la caché.
        if tainting == Tainting::Cors {
            let key = CacheKey {
                origin: &origin,
                url: url.href(),
                credentials: p.credentials == "include",
            };
            let method_needs =
                !key.method_match(&method) && (!is_safelisted_method(&method) || p.use_preflight);
            let headers_need = unsafe_header_names(&headers)
                .iter()
                .any(|n| !key.header_match(n));
            if method_needs || headers_need {
                preflight(backend, p, &url, &method, &headers, &origin, &key)?;
            }
        }
        // §3.1 "append a request `Origin` header".
        let mut h = bf_net::Headers::default();
        for (n, v) in &headers.0 {
            h.push(n, v);
        }
        if tainting == Tainting::Cors || p.mode == "websocket" {
            h.push("Origin", &origin_header);
        } else if !matches!(method.as_str(), "GET" | "HEAD") {
            let mut o = origin_header.clone();
            if p.mode != "cors" {
                match p.referrer_policy.as_str() {
                    "no-referrer" => o = "null".into(),
                    "same-origin" if !same_origin(&url, &p.origin) => o = "null".into(),
                    "unsafe-url" | "origin" | "origin-when-cross-origin" | "same-origin" => {}
                    _ => {
                        if p.origin.starts_with("https:") && url.scheme != "https" {
                            o = "null".into();
                        }
                    }
                }
            }
            h.push("Origin", &o);
        }
        let req = NetRequest {
            url: url.clone(),
            options: bf_net::RequestOptions {
                method: method.clone(),
                headers: h,
                body: body.clone(),
                redirect: bf_net::RedirectMode::Manual,
                credentials: include,
                referrer: referrer_for(p, &url),
            },
        };
        let mut sink = HopSink {
            out: &mut *out,
            url_list: list.clone(),
            tainting,
            origin: origin.clone(),
            include: p.credentials == "include",
            redirect: p.redirect,
            result: None,
            forwarding: false,
        };
        let r = backend.fetch_streaming(&req, &mut sink);
        let result = sink.result.take();
        let (status, location) = match result {
            Some(Hop::Done) => return r,
            Some(Hop::Error(e)) => {
                if tainting == Tainting::Cors {
                    CacheKey {
                        origin: &origin,
                        url: url.href(),
                        credentials: p.credentials == "include",
                    }
                    .clear();
                }
                return Err(e);
            }
            Some(Hop::Redirect { status, location }) => (status, location),
            None => return Err(r.err().unwrap_or_else(|| "empty response".into())),
        };
        // "HTTP-redirect fetch" (§4.4).
        let Ok(mut next) = Url::parse(&location, Some(&url)) else {
            return Err(format!("invalid Location {location}"));
        };
        if next.fragment.is_none() {
            next.fragment.clone_from(&url.fragment);
        }
        if !matches!(next.scheme.as_str(), "http" | "https") {
            return Err(format!("redirect to a non-HTTP(S) URL {}", next.href()));
        }
        if redirects == 20 {
            return Err("too many redirects".into());
        }
        redirects += 1;
        if p.mode == "cors" && includes_credentials(&next) && !same_origin(&next, &p.origin) {
            return Err("cross-origin redirect to a URL with credentials".into());
        }
        if tainting == Tainting::Cors && includes_credentials(&next) {
            return Err("CORS redirect to a URL with credentials".into());
        }
        if status != 303 && body.is_some() && p.body_from_stream {
            return Err("redirect with a streaming request body".into());
        }
        if (matches!(status, 301 | 302) && method == "POST")
            || (status == 303 && !matches!(method.as_str(), "GET" | "HEAD"))
        {
            method = "GET".into();
            body = None;
            headers.0.retain(|(n, _)| {
                !matches!(
                    n.to_ascii_lowercase().as_str(),
                    "content-encoding" | "content-language" | "content-location" | "content-type"
                )
            });
        }
        if url_origin(&url) != url_origin(&next) {
            headers.0.retain(|(n, _)| !is_non_wildcard(n));
        }
        url = next;
        list.push(url.clone());
    }
}

/// Main fetch con el cuerpo entero: la respuesta sin filtrar y su tainting.
pub fn fetch_whole(
    backend: &dyn NetBackend,
    p: &FetchParams,
) -> Result<(NetResponse, Tainting), String> {
    struct Collect {
        head: Option<FetchHead>,
        body: Vec<u8>,
    }
    impl FetchSink for Collect {
        fn head(&mut self, head: FetchHead) -> bool {
            self.head = Some(head);
            true
        }
        fn chunk(&mut self, bytes: &[u8]) -> bool {
            self.body.extend_from_slice(bytes);
            true
        }
    }
    let mut c = Collect {
        head: None,
        body: Vec::new(),
    };
    main_fetch(backend, p, &mut c)?;
    let h = c.head.ok_or("empty response")?;
    Ok((
        NetResponse {
            url_list: h.url_list,
            status: h.status,
            status_text: h.status_text,
            headers: h.headers,
            body: c.body,
        },
        h.tainting,
    ))
}
