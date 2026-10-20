//! Red: HTTP/1.1 sobre TCP y TLS (rustls), pool de conexiones keep-alive, redirecciones según
//! Fetch, descompresión, cookies (RFC 6265bis), `data:`/`file:`, detección de encoding de
//! documentos y un `ResourceLoader` para el resto del motor. Síncrono por ahora.
//!
//! TODO(roadmap): HTTP/2, caché HTTP, CORS y políticas de seguridad de Fetch.

#![forbid(unsafe_code)]

pub mod cookies;
pub mod http;
pub mod sniff;

use std::collections::HashMap;
use std::io::{self, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use bf_infra::resource::load_local;
use bf_infra::{Resource, ResourceLoader};
use bf_url::{Host, Url};

pub use cookies::CookieJar;
pub use http::Headers;
pub use sniff::{EncodingSource, MimeType, html_encoding, html_encoding_with_source};

pub const USER_AGENT: &str = "Mozilla/5.0 (compatible; BrowserFreedom/0.1)";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: usize = 20;
const MAX_BODY: u64 = 64 << 20;

#[derive(Debug)]
pub enum FetchError {
    UnsupportedScheme(String),
    InvalidUrl,
    Io(io::Error),
    /// No se pudo resolver el nombre del host.
    Dns(io::Error),
    /// Se agotó el tiempo de conexión o de lectura.
    Timeout,
    Tls(String),
    TooManyRedirects,
    /// `file:` o `data:` que no se pudo leer.
    NotFound,
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::UnsupportedScheme(s) => write!(f, "esquema no soportado: {s}"),
            FetchError::InvalidUrl => f.write_str("URL inválida"),
            FetchError::Io(e) => write!(f, "error de red: {e}"),
            FetchError::Dns(e) => write!(f, "no se pudo resolver el host: {e}"),
            FetchError::Timeout => f.write_str("se agotó el tiempo de espera"),
            FetchError::Tls(e) => write!(f, "error de TLS: {e}"),
            FetchError::TooManyRedirects => f.write_str("demasiadas redirecciones"),
            FetchError::NotFound => f.write_str("recurso no encontrado"),
        }
    }
}

impl std::error::Error for FetchError {}

impl From<io::Error> for FetchError {
    fn from(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => FetchError::Timeout,
            _ => FetchError::Io(e),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
    Post,
}

impl Method {
    fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Head => "HEAD",
            Method::Post => "POST",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FetchOptions {
    pub method: Method,
    /// Cuerpo y su `Content-Type` (para POST).
    pub body: Option<(Vec<u8>, String)>,
    pub referrer: Option<Url>,
    pub accept: String,
}

impl Default for FetchOptions {
    fn default() -> Self {
        FetchOptions {
            method: Method::Get,
            body: None,
            referrer: None,
            accept: "*/*".into(),
        }
    }
}

impl FetchOptions {
    /// Para documentos (navegación).
    pub fn document() -> Self {
        FetchOptions {
            accept: "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".into(),
            ..FetchOptions::default()
        }
    }
}

#[derive(Debug, Clone)]
pub struct Response {
    /// URL final, después de las redirecciones.
    pub url: Url,
    pub status: u16,
    pub status_text: String,
    pub headers: Headers,
    pub body: Vec<u8>,
    pub redirects: usize,
}

impl Response {
    pub fn content_type(&self) -> Option<MimeType> {
        self.headers.get("content-type").and_then(MimeType::parse)
    }

    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

// --- Conexiones ---

enum Stream {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(s) => s.flush(),
        }
    }
}

type PoolKey = (String, u16, bool);

fn tls_config() -> Arc<rustls::ClientConfig> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let roots = rustls::RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            };
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let mut config = rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .expect("versiones de TLS por defecto")
                .with_root_certificates(roots)
                .with_no_client_auth();
            // HTTP/2 queda para el roadmap.
            config.alpn_protocols = vec![b"http/1.1".to_vec()];
            Arc::new(config)
        })
        .clone()
}

fn connect(host: &str, port: u16, tls: bool) -> Result<Stream, FetchError> {
    let addrs: Vec<_> = (host.trim_start_matches('[').trim_end_matches(']'), port)
        .to_socket_addrs()
        .map_err(FetchError::Dns)?
        .collect();
    let mut last = io::Error::new(io::ErrorKind::NotFound, "sin direcciones");
    let mut tcp = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(s) => {
                tcp = Some(s);
                break;
            }
            Err(e) => last = e,
        }
    }
    let tcp = tcp.ok_or_else(|| FetchError::from(last))?;
    tcp.set_read_timeout(Some(READ_TIMEOUT))?;
    tcp.set_write_timeout(Some(READ_TIMEOUT))?;
    let _ = tcp.set_nodelay(true);
    if !tls {
        return Ok(Stream::Plain(tcp));
    }
    let name = rustls_pki_types::ServerName::try_from(
        host.trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned(),
    )
    .map_err(|e| FetchError::Tls(e.to_string()))?;
    let conn = rustls::ClientConnection::new(tls_config(), name)
        .map_err(|e| FetchError::Tls(e.to_string()))?;
    Ok(Stream::Tls(Box::new(rustls::StreamOwned::new(conn, tcp))))
}

/// `Accept-Language` del sistema (variables `LANGUAGE`/`LC_ALL`/`LANG`) o el de por defecto.
fn accept_language() -> String {
    for var in ["LANGUAGE", "LC_ALL", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            let tag = v
                .split([':', '.', '@'])
                .next()
                .unwrap_or("")
                .replace('_', "-");
            if tag.len() >= 2 && tag != "C" && tag != "POSIX" {
                let primary = tag.split('-').next().unwrap_or(&tag).to_owned();
                return if primary == tag {
                    format!("{tag},en;q=0.8")
                } else {
                    format!("{tag},{primary};q=0.9,en;q=0.8")
                };
            }
        }
    }
    "es-419,es;q=0.9,en;q=0.8".into()
}

/// Cliente HTTP con pool de conexiones y jarra de cookies.
#[derive(Default)]
pub struct Fetcher {
    pool: Mutex<HashMap<PoolKey, Vec<BufReader<Stream>>>>,
    pub cookies: CookieJar,
    /// Conexiones abiertas (para los tests de keep-alive).
    pub connections_opened: Mutex<usize>,
}

impl Fetcher {
    pub fn new() -> Fetcher {
        Fetcher::default()
    }

    /// Fetch de `url`: `http(s)`, `data:` y `file:`.
    pub fn fetch(&self, url: &Url, opts: &FetchOptions) -> Result<Response, FetchError> {
        let mut url = url.clone();
        let mut method = opts.method;
        let mut body = opts.body.clone();
        let mut referrer = opts.referrer.clone();
        for redirects in 0..=MAX_REDIRECTS {
            if !matches!(url.scheme.as_str(), "http" | "https") {
                let res = load_local(&url, &[]).ok_or(
                    if matches!(url.scheme.as_str(), "data" | "file") {
                        FetchError::NotFound
                    } else {
                        FetchError::UnsupportedScheme(url.scheme.clone())
                    },
                )?;
                let mut headers = Headers::default();
                if let Some(ct) = &res.content_type {
                    headers.push("Content-Type", ct);
                }
                return Ok(Response {
                    url: res.url,
                    status: 200,
                    status_text: "OK".into(),
                    headers,
                    body: res.bytes,
                    redirects,
                });
            }
            let mut response = self.http(&url, method, body.as_ref(), referrer.as_ref(), opts)?;
            response.redirects = redirects;
            if !matches!(response.status, 301 | 302 | 303 | 307 | 308) {
                return Ok(response);
            }
            let Some(location) = response.headers.get("location") else {
                return Ok(response);
            };
            let Ok(mut next) = Url::parse(location, Some(&url)) else {
                return Ok(response);
            };
            // Fetch §4.4: el fragmento se hereda si la Location no trae uno.
            if next.fragment.is_none() {
                next.fragment.clone_from(&url.fragment);
            }
            // Fetch §4.4 paso 12: 301/302 con POST y 303 (salvo HEAD) pasan a GET sin cuerpo.
            if (matches!(response.status, 301 | 302) && method == Method::Post)
                || (response.status == 303 && method != Method::Head)
            {
                method = Method::Get;
                body = None;
            }
            referrer = Some(url.clone());
            url = next;
        }
        Err(FetchError::TooManyRedirects)
    }

    fn http(
        &self,
        url: &Url,
        method: Method,
        body: Option<&(Vec<u8>, String)>,
        referrer: Option<&Url>,
        opts: &FetchOptions,
    ) -> Result<Response, FetchError> {
        let tls = url.scheme == "https";
        let host = match &url.host {
            Some(Host::Empty) | None => return Err(FetchError::InvalidUrl),
            Some(h) => h.to_string(),
        };
        let port = url.port.unwrap_or(if tls { 443 } else { 80 });
        let key: PoolKey = (host.clone(), port, tls);
        let mut headers = Headers::default();
        headers.push("Host", &url.host_string());
        headers.push("User-Agent", USER_AGENT);
        headers.push("Accept", &opts.accept);
        headers.push("Accept-Language", &accept_language());
        headers.push("Accept-Encoding", "gzip, deflate, br");
        headers.push("Connection", "keep-alive");
        if let Some(c) = self.cookies.cookie_header(url) {
            headers.push("Cookie", &c);
        }
        if let Some(r) = referrer {
            // Política por defecto (strict-origin-when-cross-origin), simplificada: sin
            // fragmento ni credenciales, y nada de https → http.
            if !(r.scheme == "https" && !tls) {
                let mut r = r.clone();
                r.username.clear();
                r.password.clear();
                r.fragment = None;
                if r.origin() != url.origin() {
                    r.path = bf_url::Path::List(vec![String::new()]);
                    r.query = None;
                }
                headers.push("Referer", &r.href());
            }
        }
        if let Some((_, ct)) = body {
            headers.push("Content-Type", ct);
        }
        let target = {
            let mut t = url.path_string();
            if t.is_empty() {
                t.push('/');
            }
            if let Some(q) = &url.query {
                t.push('?');
                t.push_str(q);
            }
            t
        };
        // Una conexión reutilizada puede estar cerrada: un reintento con una nueva.
        for attempt in 0..2 {
            let pooled = if attempt == 0 {
                self.pool
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_mut(&key)
                    .and_then(Vec::pop)
            } else {
                None
            };
            let reused = pooled.is_some();
            let mut conn = match pooled {
                Some(c) => c,
                None => {
                    *self
                        .connections_opened
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()) += 1;
                    BufReader::new(connect(&host, port, tls)?)
                }
            };
            let result = (|| -> Result<(http::Head, Vec<u8>, bool), FetchError> {
                http::write_request(
                    conn.get_mut(),
                    method.as_str(),
                    &target,
                    &headers,
                    body.map(|(b, _)| b.as_slice()),
                )?;
                let mut head = http::read_head(&mut conn)?;
                // 1xx informativos: se descartan.
                while (100..200).contains(&head.status) && head.status != 101 {
                    head = http::read_head(&mut conn)?;
                }
                let kind = http::body_kind(&head, method == Method::Head);
                let raw = http::read_body(&mut conn, kind, MAX_BODY)?;
                let close = kind == http::BodyKind::UntilEof
                    || head.headers.list("connection").iter().any(|c| c == "close")
                    || (head.version == (1, 0)
                        && !head
                            .headers
                            .list("connection")
                            .iter()
                            .any(|c| c == "keep-alive"));
                Ok((head, raw, !close))
            })();
            let (head, raw, keep) = match result {
                Ok(r) => r,
                Err(_) if reused => continue,
                Err(e) => return Err(e),
            };
            if keep {
                self.pool
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .entry(key.clone())
                    .or_default()
                    .push(conn);
            }
            for c in head.headers.get_all("set-cookie") {
                self.cookies.set_cookie(url, c);
            }
            let body = http::decode_content(raw, &head.headers.list("content-encoding"))?;
            return Ok(Response {
                url: url.clone(),
                status: head.status,
                status_text: head.reason,
                headers: head.headers,
                body,
                redirects: 0,
            });
        }
        Err(FetchError::Io(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "la conexión se cerró",
        )))
    }
}

/// `ResourceLoader` con red: `http(s)` por el [`Fetcher`], `data:` y `file:` locales.
#[derive(Default)]
pub struct NetLoader {
    pub fetcher: Fetcher,
    /// Errores de carga (URL y motivo), para diagnóstico.
    pub errors: Mutex<Vec<(String, String)>>,
}

impl NetLoader {
    pub fn new() -> NetLoader {
        NetLoader::default()
    }
}

impl ResourceLoader for NetLoader {
    fn load(&self, url: &Url) -> Option<Resource> {
        match self.fetcher.fetch(url, &FetchOptions::default()) {
            Ok(r) if r.ok() => Some(Resource {
                content_type: r.headers.get("content-type").map(str::to_owned),
                bytes: r.body,
                url: r.url,
            }),
            Ok(r) => {
                self.errors
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((url.href(), format!("HTTP {}", r.status)));
                None
            }
            Err(e) => {
                self.errors
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((url.href(), e.to_string()));
                None
            }
        }
    }
}
