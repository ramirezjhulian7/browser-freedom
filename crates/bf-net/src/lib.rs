//! Red: HTTP/1.1 y HTTP/2 (negociado por ALPN) sobre TCP y TLS (rustls), pool de conexiones
//! keep-alive, redirecciones según Fetch, descompresión, cookies (RFC 6265bis),
//! `data:`/`file:`, detección de encoding de documentos y un `ResourceLoader` para el resto
//! del motor. Síncrono por ahora.
//!
//! La caché HTTP (RFC 9111, [`cache`]) se activa por `Fetcher`: [`NetLoader::new`] la trae en
//! memoria y [`NetLoader::with_disk_cache`] también en disco.
//!
//! TODO(roadmap): CORS y políticas de seguridad de Fetch.

#![forbid(unsafe_code)]

pub mod blob; // E2: el almacén de URLs blob:
pub mod cache;
pub mod cookies;
pub mod hpack;
pub mod http;
pub mod http2;
pub mod psl;
pub mod sniff;
pub mod streaming; // E2: respuestas por partes (EventSource)
pub mod websocket; // E2: cliente WebSocket (RFC 6455)

use std::collections::{HashMap, HashSet};
use std::io::{self, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use bf_infra::resource::load_local;
use bf_infra::{Resource, ResourceLoader};
use bf_url::{Host, Url};

pub use cache::{CacheMode, CacheUse, HttpCache};
pub use cookies::CookieJar;
pub use http::Headers;
pub use sniff::{EncodingSource, MimeType, html_encoding, html_encoding_with_source};

/// El `User-Agent` de Freedom (ver [`bf_infra::identity`]): los datos de Chrome, con Freedom
/// como producto y como motor.
pub use bf_infra::identity::USER_AGENT;
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
    /// El *cache mode* de Fetch (si el `Fetcher` tiene caché).
    pub cache: CacheMode,
}

impl Default for FetchOptions {
    fn default() -> Self {
        FetchOptions {
            method: Method::Get,
            body: None,
            referrer: None,
            accept: "*/*".into(),
            cache: CacheMode::Default,
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

/// Por dónde llegó una respuesta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Protocol {
    Http11,
    H2,
    /// `data:`, `file:`, `blob:`.
    #[default]
    Local,
}

impl Protocol {
    /// El identificador ALPN (`http/1.1`, `h2`) o `local`.
    pub fn name(self) -> &'static str {
        match self {
            Protocol::Http11 => "http/1.1",
            Protocol::H2 => "h2",
            Protocol::Local => "local",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Response {
    /// URL final, después de las redirecciones.
    pub url: Url,
    pub status: u16,
    /// Vacío en HTTP/2 (Fetch: no hay *reason phrase*).
    pub status_text: String,
    pub headers: Headers,
    pub body: Vec<u8>,
    pub redirects: usize,
    pub protocol: Protocol,
    /// Qué hizo la caché HTTP con el último salto.
    pub cache: CacheUse,
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

/// Una configuración de cliente TLS con `roots` y la lista ALPN `alpn`.
fn make_tls_config(roots: rustls::RootCertStore, alpn: &[&[u8]]) -> Arc<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("versiones de TLS por defecto")
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    Arc::new(config)
}

fn web_roots() -> rustls::RootCertStore {
    rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    }
}

/// Solo HTTP/1.1 (WebSocket y las respuestas por partes van por HTTP/1.1).
fn tls_config() -> Arc<rustls::ClientConfig> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| make_tls_config(web_roots(), &[b"http/1.1"]))
        .clone()
}

/// RFC 9113 §3.2: `h2` primero y `http/1.1` como repliegue.
fn tls_config_h2() -> Arc<rustls::ClientConfig> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| make_tls_config(web_roots(), &[b"h2", b"http/1.1"]))
        .clone()
}

/// Un error de E/S del handshake: si viene de rustls, es un error de TLS.
fn tls_error(e: io::Error) -> FetchError {
    if e.get_ref()
        .is_some_and(|inner| inner.downcast_ref::<rustls::Error>().is_some())
    {
        return FetchError::Tls(e.to_string());
    }
    FetchError::from(e)
}

fn server_name(host: &str) -> Result<rustls_pki_types::ServerName<'static>, FetchError> {
    rustls_pki_types::ServerName::try_from(
        host.trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned(),
    )
    .map_err(|e| FetchError::Tls(e.to_string()))
}

fn connect_tcp(host: &str, port: u16) -> Result<TcpStream, FetchError> {
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
    Ok(tcp)
}

/// Una conexión HTTP/1.1 (sin ALPN `h2`).
fn connect(host: &str, port: u16, tls: bool) -> Result<Stream, FetchError> {
    let tcp = connect_tcp(host, port)?;
    if !tls {
        return Ok(Stream::Plain(tcp));
    }
    let conn = rustls::ClientConnection::new(tls_config(), server_name(host)?)
        .map_err(|e| FetchError::Tls(e.to_string()))?;
    Ok(Stream::Tls(Box::new(rustls::StreamOwned::new(conn, tcp))))
}

/// Una conexión nueva del [`Fetcher`]: HTTP/1.1 o HTTP/2, según lo negociado.
enum Opened {
    H1(Stream),
    H2(http2::Connection),
}

/// Los idiomas del `Accept-Language`, en orden (`navigator.languages`).
pub fn languages() -> Vec<String> {
    accept_language()
        .split(',')
        .map(|l| l.split(';').next().unwrap_or("").trim().to_owned())
        .filter(|l| !l.is_empty())
        .collect()
}

/// `Accept-Language` del sistema (variables `LANGUAGE`/`LC_ALL`/`LANG`) o el de por defecto.
pub fn accept_language() -> String {
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
    /// HTTP/2: una conexión por origen, compartida por todas las peticiones (RFC 9113 §9.1).
    h2: Mutex<HashMap<PoolKey, Arc<http2::Connection>>>,
    /// Orígenes que pidieron HTTP/1.1 (`HTTP_1_1_REQUIRED`).
    h1_only: Mutex<HashSet<PoolKey>>,
    /// Orígenes que negociaron HTTP/1.1: sus conexiones se abren en paralelo.
    h1_known: Mutex<HashSet<PoolKey>>,
    /// Turno para abrir la primera conexión con un origen que puede hablar h2.
    connecting: Mutex<HashMap<PoolKey, Arc<Mutex<()>>>>,
    pub cookies: CookieJar,
    /// Conexiones abiertas (para los tests de keep-alive).
    pub connections_opened: Mutex<usize>,
    /// HTTP/2 sobre TCP sin TLS con conocimiento previo (RFC 9113 §3.3), para `http:`.
    pub h2_prior_knowledge: bool,
    /// Raíces de confianza propias (en vez de las de webpki-roots): configuraciones TLS
    /// con y sin `h2`.
    tls: Option<(Arc<rustls::ClientConfig>, Arc<rustls::ClientConfig>)>,
    /// Registro de las peticiones (para el panel del motor).
    log: Mutex<Vec<LogEntry>>,
    /// La caché HTTP (RFC 9111); `None`: sin caché.
    pub cache: Option<HttpCache>,
}

/// Una petición del registro de red de un [`Fetcher`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub method: String,
    pub url: String,
    pub status: u16,
    /// `Protocol::Local` si se sirvió de la caché sin red.
    pub protocol: Protocol,
    pub cache: CacheUse,
}

impl LogEntry {
    /// ¿Hubo una petición a la red? (Todo salvo lo servido de la caché sin validar.)
    pub fn network(&self) -> bool {
        !matches!(self.cache, CacheUse::Hit | CacheUse::Unavailable)
    }
}

/// `FREEDOM_NET_LOG=1`: cada petición se escribe en stderr.
fn net_log_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("FREEDOM_NET_LOG").is_some_and(|v| v != "0"))
}

impl Fetcher {
    pub fn new() -> Fetcher {
        Fetcher::default()
    }

    /// Un `Fetcher` que confía solo en los certificados raíz `roots` (DER), para servidores
    /// de prueba.
    pub fn with_root_certificates(roots: &[Vec<u8>]) -> Fetcher {
        let mut store = rustls::RootCertStore::empty();
        for der in roots {
            let _ = store.add(rustls_pki_types::CertificateDer::from(der.clone()));
        }
        Fetcher {
            tls: Some((
                make_tls_config(store.clone(), &[b"h2", b"http/1.1"]),
                make_tls_config(store, &[b"http/1.1"]),
            )),
            ..Fetcher::default()
        }
    }

    /// Abre una conexión: TLS negocia `h2` por ALPN (RFC 9113 §3.2) salvo que el origen
    /// pidiera HTTP/1.1; `http:` usa HTTP/2 solo con conocimiento previo (§3.3).
    fn open(&self, host: &str, port: u16, tls: bool, key: &PoolKey) -> Result<Opened, FetchError> {
        let allow_h2 = !self
            .h1_only
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(key);
        let mut tcp = connect_tcp(host, port)?;
        if !tls {
            if allow_h2 && self.h2_prior_knowledge {
                return Ok(Opened::H2(http2::start(http2::Transport::Plain(tcp))?));
            }
            return Ok(Opened::H1(Stream::Plain(tcp)));
        }
        let config = match (&self.tls, allow_h2) {
            (Some((h2, _)), true) => h2.clone(),
            (Some((_, h1)), false) => h1.clone(),
            (None, true) => tls_config_h2(),
            (None, false) => tls_config(),
        };
        let mut conn = rustls::ClientConnection::new(config, server_name(host)?)
            .map_err(|e| FetchError::Tls(e.to_string()))?;
        while conn.is_handshaking() {
            conn.complete_io(&mut tcp).map_err(tls_error)?;
        }
        if conn.alpn_protocol() == Some(b"h2") {
            let transport = http2::Transport::Tls(Box::new(conn), tcp);
            return Ok(Opened::H2(http2::start(transport)?));
        }
        let stream = rustls::StreamOwned::new(conn, tcp);
        Ok(Opened::H1(Stream::Tls(Box::new(stream))))
    }

    /// La conexión HTTP/2 viva con `key`, si hay.
    fn h2_connection(&self, key: &PoolKey) -> Option<Arc<http2::Connection>> {
        let mut map = self.h2.lock().unwrap_or_else(|e| e.into_inner());
        match map.get(key) {
            Some(c) if c.is_alive() => Some(c.clone()),
            Some(_) => {
                map.remove(key);
                None
            }
            None => None,
        }
    }

    fn forget_h2(&self, key: &PoolKey, conn: &Arc<http2::Connection>) {
        let mut map = self.h2.lock().unwrap_or_else(|e| e.into_inner());
        if map.get(key).is_some_and(|c| Arc::ptr_eq(c, conn)) {
            map.remove(key);
        }
    }

    /// Lo común a HTTP/1.1 y HTTP/2 tras recibir la respuesta: cookies y descompresión.
    fn finish(
        &self,
        url: &Url,
        extra: &HttpExtra<'_>,
        status: (u16, String),
        headers: Headers,
        raw: Vec<u8>,
        protocol: Protocol,
    ) -> Result<Response, FetchError> {
        if extra.credentials {
            for c in headers.get_all("set-cookie") {
                self.cookies.set_cookie(url, c);
            }
        }
        let body = http::decode_content(raw, &headers.list("content-encoding"))?;
        Ok(Response {
            url: url.clone(),
            status: status.0,
            status_text: status.1,
            headers,
            body,
            redirects: 0,
            protocol,
            cache: CacheUse::Network,
        })
    }

    fn record(&self, method: &str, url: &Url, r: &Response) {
        let entry = LogEntry {
            method: method.to_ascii_uppercase(),
            url: url.href(),
            status: r.status,
            protocol: r.protocol,
            cache: r.cache,
        };
        if net_log_enabled() {
            let how = match entry.cache {
                CacheUse::Network => "red",
                CacheUse::Hit => "caché",
                CacheUse::Revalidated => "caché validada (304)",
                CacheUse::Unavailable => "caché vacía (504)",
            };
            eprintln!(
                "red: {} {} {} {} [{how}]",
                entry.method,
                entry.url,
                entry.status,
                entry.protocol.name()
            );
        }
        self.log
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(entry);
    }

    /// Las peticiones que fueron a la red, en orden.
    pub fn requests(&self) -> Vec<LogEntry> {
        self.log.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Fetch de `url`: `http(s)`, `data:` y `file:`.
    pub fn fetch(&self, url: &Url, opts: &FetchOptions) -> Result<Response, FetchError> {
        let mut url = url.clone();
        let mut method = opts.method;
        let mut body = opts.body.clone();
        let mut referrer = opts.referrer.clone();
        for redirects in 0..=MAX_REDIRECTS {
            if url.scheme == "blob" {
                return blob_response(&url, redirects);
            }
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
                    protocol: Protocol::Local,
                    cache: CacheUse::Network,
                });
            }
            let no_headers = Headers::default();
            let extra = HttpExtra {
                method: method.as_str(),
                body: body
                    .as_ref()
                    .map(|(b, ct)| (b.as_slice(), Some(ct.as_str()))),
                headers: &no_headers,
                credentials: true,
                cache: opts.cache,
            };
            let mut response = self.http(&url, &extra, referrer.as_ref(), &opts.accept)?;
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
        extra: &HttpExtra<'_>,
        referrer: Option<&Url>,
        accept: &str,
    ) -> Result<Response, FetchError> {
        let method = extra.method;
        let body = extra.body;
        let tls = url.scheme == "https";
        if matches!(url.host, Some(Host::Empty) | None) {
            return Err(FetchError::InvalidUrl);
        }
        let mut headers = Headers::default();
        headers.push("Host", &url.host_string());
        // E2: las cabeceras del script reemplazan a las por defecto del mismo nombre.
        let has = |n: &str| extra.headers.get(n).is_some();
        if !has("user-agent") {
            headers.push("User-Agent", USER_AGENT);
        }
        // Client Hints de baja entropía (UA-CH §4.2), solo en peticiones seguras, como Chrome.
        if tls {
            for (name, value) in bf_infra::identity::client_hint_headers() {
                headers.push(name, &value);
            }
        }
        if !has("accept") {
            headers.push("Accept", accept);
        }
        if !has("accept-language") {
            headers.push("Accept-Language", &accept_language());
        }
        headers.push("Accept-Encoding", "gzip, deflate, br");
        headers.push("Connection", "keep-alive");
        if extra.credentials
            && let Some(c) = self.cookies.cookie_header(url)
        {
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
        if let Some((_, Some(ct))) = body
            && !has("content-type")
        {
            headers.push("Content-Type", ct);
        }
        for (n, v) in &extra.headers.0 {
            headers.push(n, v);
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
        let response = match &self.cache {
            None => self.send(url, extra, &headers, &target)?,
            Some(cache) => {
                let req = cache::CacheRequest {
                    method,
                    url,
                    headers: &headers,
                    mode: extra.cache,
                    credentials: extra.credentials,
                };
                let mut protocol = Protocol::Local;
                let (r, used) = cache.fetch(&req, |h| {
                    let r = self.send(url, extra, h, &target)?;
                    protocol = r.protocol;
                    Ok::<_, FetchError>(cache::CacheResponse {
                        status: r.status,
                        status_text: r.status_text,
                        headers: r.headers,
                        body: r.body,
                    })
                })?;
                // Fetch §4.5: `only-if-cached` sin nada guardado es un error de red.
                if used == CacheUse::Unavailable && extra.cache == CacheMode::OnlyIfCached {
                    return Err(FetchError::Io(io::Error::other(
                        "only-if-cached: no está en la caché",
                    )));
                }
                Response {
                    url: url.clone(),
                    status: r.status,
                    status_text: r.status_text,
                    headers: r.headers,
                    body: r.body,
                    redirects: 0,
                    protocol: if used == CacheUse::Network {
                        protocol
                    } else {
                        Protocol::Local
                    },
                    cache: used,
                }
            }
        };
        self.record(method, url, &response);
        Ok(response)
    }

    /// Un salto por la red (HTTP/2 si hay conexión o se negocia, si no HTTP/1.1) con
    /// `headers` ya completas.
    fn send(
        &self,
        url: &Url,
        extra: &HttpExtra<'_>,
        headers: &Headers,
        target: &str,
    ) -> Result<Response, FetchError> {
        let method = extra.method;
        let body = extra.body;
        let tls = url.scheme == "https";
        let host = match &url.host {
            Some(Host::Empty) | None => return Err(FetchError::InvalidUrl),
            Some(h) => h.to_string(),
        };
        let port = url.port.unwrap_or(if tls { 443 } else { 80 });
        let key: PoolKey = (host.clone(), port, tls);
        // RFC 9110 §9.2.2: solo los métodos idempotentes se repiten si la conexión se cae
        // a medias.
        let idempotent = ["GET", "HEAD", "OPTIONS", "TRACE", "PUT", "DELETE"]
            .iter()
            .any(|m| method.eq_ignore_ascii_case(m));
        // Una conexión reutilizada puede estar cerrada: un reintento con una nueva.
        let mut fresh = false;
        for _ in 0..5 {
            if let Some(conn) = self.h2_connection(&key) {
                let req = http2::Request::from_http1(
                    method,
                    &url.scheme,
                    &target,
                    &headers,
                    body.map(|(b, _)| b),
                );
                match conn.request(req) {
                    Ok(r) => {
                        let status = (r.status, String::new());
                        return self.finish(url, extra, status, r.headers, r.body, Protocol::H2);
                    }
                    // REFUSED_STREAM con la conexión viva: se repite en ella.
                    Err(http2::Error::Refused) if conn.is_alive() => continue,
                    Err(http2::Error::Refused) => {}
                    Err(http2::Error::Lost) if idempotent => {}
                    Err(http2::Error::Http11Required) => {
                        self.h1_only
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(key.clone());
                    }
                    Err(http2::Error::Timeout) => return Err(FetchError::Timeout),
                    Err(e) => return Err(FetchError::Io(io::Error::other(e.to_string()))),
                }
                self.forget_h2(&key, &conn);
                continue;
            }
            let pooled = if !fresh {
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
                    // Primer contacto con un origen que puede hablar h2: una conexión a la
                    // vez, para que las peticiones simultáneas compartan la que se abra.
                    let may_h2 = (tls || self.h2_prior_knowledge)
                        && !self
                            .h1_known
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .contains(&key);
                    let gate = may_h2.then(|| {
                        self.connecting
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .entry(key.clone())
                            .or_default()
                            .clone()
                    });
                    let _turn = gate
                        .as_ref()
                        .map(|g| g.lock().unwrap_or_else(|e| e.into_inner()));
                    if may_h2 && self.h2_connection(&key).is_some() {
                        continue;
                    }
                    *self
                        .connections_opened
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()) += 1;
                    match self.open(&host, port, tls, &key)? {
                        Opened::H1(s) => {
                            self.h1_known
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .insert(key.clone());
                            BufReader::new(s)
                        }
                        Opened::H2(c) => {
                            // Si otro hilo abrió una a la vez, se queda la primera.
                            let mut map = self.h2.lock().unwrap_or_else(|e| e.into_inner());
                            if !map.get(&key).is_some_and(|c| c.is_alive()) {
                                map.insert(key.clone(), Arc::new(c));
                            }
                            continue;
                        }
                    }
                }
            };
            let result = (|| -> Result<(http::Head, Vec<u8>, bool), FetchError> {
                http::write_request(
                    conn.get_mut(),
                    method,
                    &target,
                    &headers,
                    body.map(|(b, _)| b),
                )?;
                let mut head = http::read_head(&mut conn)?;
                // 1xx informativos: se descartan.
                while (100..200).contains(&head.status) && head.status != 101 {
                    head = http::read_head(&mut conn)?;
                }
                let kind = http::body_kind(&head, method.eq_ignore_ascii_case("HEAD"));
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
                Err(_) if reused => {
                    fresh = true;
                    continue;
                }
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
            let status = (head.status, head.reason);
            return self.finish(url, extra, status, head.headers, raw, Protocol::Http11);
        }
        Err(FetchError::Io(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "la conexión se cerró",
        )))
    }
}

// --- E2: peticiones con método, cabeceras y modo de redirección arbitrarios (para fetch() y
// XMLHttpRequest) ---

/// Lo que cambia de una petición HTTP respecto a las de documentos y subrecursos.
struct HttpExtra<'a> {
    method: &'a str,
    body: Option<(&'a [u8], Option<&'a str>)>,
    headers: &'a Headers,
    credentials: bool,
    cache: CacheMode,
}

/// El modo de redirección de una petición (Fetch §2.2.5, "redirect mode").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RedirectMode {
    #[default]
    Follow,
    Error,
    Manual,
}

/// Una petición de script (Fetch §2.2.5, simplificada).
#[derive(Debug, Clone)]
pub struct RequestOptions {
    /// El método, ya normalizado (`GET`, `POST`, `PUT`…).
    pub method: String,
    /// Las cabeceras del autor (sin las prohibidas).
    pub headers: Headers,
    pub body: Option<Vec<u8>>,
    pub redirect: RedirectMode,
    /// ¿Se mandan y guardan cookies? (credentials mode `include`/`same-origin`).
    pub credentials: bool,
    pub referrer: Option<Url>,
}

impl Default for RequestOptions {
    fn default() -> Self {
        RequestOptions {
            method: "GET".into(),
            headers: Headers::default(),
            body: None,
            redirect: RedirectMode::Follow,
            credentials: true,
            referrer: None,
        }
    }
}

/// La respuesta de una URL `blob:` (Fetch §4.2, "scheme fetch"): `200 OK`, o `206` con una
/// cabecera `Range` de un solo rango.
fn blob_response(url: &Url, redirects: usize) -> Result<Response, FetchError> {
    blob_response_range(url, redirects, None)
}

/// "Parse a single range header value" (Fetch §2.2.6, con espacios permitidos).
fn parse_range(v: &str) -> Option<(Option<u64>, Option<u64>)> {
    let v = v.trim_matches([' ', '\t']);
    let rest = v.strip_prefix("bytes")?.trim_start_matches([' ', '\t']);
    let rest = rest.strip_prefix('=')?.trim_matches([' ', '\t']);
    let (a, b) = rest.split_once('-')?;
    let (a, b) = (a.trim_matches([' ', '\t']), b.trim_matches([' ', '\t']));
    let num = |s: &str| -> Option<Option<u64>> {
        if s.is_empty() {
            Some(None)
        } else if s.chars().all(|c| c.is_ascii_digit()) {
            s.parse().ok().map(Some)
        } else {
            None
        }
    };
    let (start, end) = (num(a)?, num(b)?);
    if start.is_none() && end.is_none() {
        return None;
    }
    if let (Some(s), Some(e)) = (start, end)
        && s > e
    {
        return None;
    }
    Some((start, end))
}

fn blob_response_range(
    url: &Url,
    redirects: usize,
    range: Option<&str>,
) -> Result<Response, FetchError> {
    let entry = blob::resolve(url).ok_or(FetchError::NotFound)?;
    let mut headers = Headers::default();
    let Some(range) = range else {
        headers.push("Content-Length", &entry.bytes.len().to_string());
        headers.push("Content-Type", &entry.mime);
        return Ok(Response {
            url: url.clone(),
            status: 200,
            status_text: "OK".into(),
            headers,
            body: entry.bytes.to_vec(),
            redirects,
            protocol: Protocol::Local,
            cache: CacheUse::Network,
        });
    };
    let len = entry.bytes.len() as u64;
    let bad = || FetchError::Io(io::Error::other("invalid range"));
    let (start, end) = parse_range(range).ok_or_else(bad)?;
    let (start, end) = match (start, end) {
        (None, Some(suffix)) => (len.saturating_sub(suffix), len.saturating_sub(1)),
        (Some(s), e) => {
            if s >= len {
                return Err(bad());
            }
            (s, e.map_or(len - 1, |e| e.min(len - 1)))
        }
        (None, None) => return Err(bad()),
    };
    if len == 0 || start > end {
        return Err(bad());
    }
    let body = entry.bytes[start as usize..=end as usize].to_vec();
    headers.push("Content-Length", &body.len().to_string());
    headers.push("Content-Type", &entry.mime);
    headers.push("Content-Range", &format!("bytes {start}-{end}/{len}"));
    Ok(Response {
        url: url.clone(),
        status: 206,
        status_text: "Partial Content".into(),
        headers,
        body,
        redirects,
        protocol: Protocol::Local,
        cache: CacheUse::Network,
    })
}

impl Fetcher {
    /// Una petición de script: la respuesta y la lista de URLs visitadas (Fetch §2.2.5, "URL
    /// list"). Con `RedirectMode::Manual` devuelve la respuesta 3xx; con `Error`, un error.
    pub fn fetch_request(
        &self,
        url: &Url,
        req: &RequestOptions,
    ) -> Result<(Response, Vec<Url>), FetchError> {
        self.fetch_request_with_cache(url, req, CacheMode::Default)
    }

    /// Como [`Fetcher::fetch_request`], con el *cache mode* de la petición (`RequestInit.cache`).
    pub fn fetch_request_with_cache(
        &self,
        url: &Url,
        req: &RequestOptions,
        cache: CacheMode,
    ) -> Result<(Response, Vec<Url>), FetchError> {
        let mut url = url.clone();
        let mut method = req.method.clone();
        let mut body = req.body.clone();
        let mut headers = req.headers.clone();
        let mut referrer = req.referrer.clone();
        let mut list = vec![url.clone()];
        for redirects in 0..=MAX_REDIRECTS {
            if url.scheme == "blob" {
                if !method.eq_ignore_ascii_case("GET") {
                    return Err(FetchError::NotFound);
                }
                let range = headers.get("range");
                return blob_response_range(&url, redirects, range).map(|r| (r, list));
            }
            if !matches!(url.scheme.as_str(), "http" | "https") {
                let res = load_local(&url, &[]).ok_or(FetchError::NotFound)?;
                let mut h = Headers::default();
                if let Some(ct) = &res.content_type {
                    h.push("Content-Type", ct);
                }
                let r = Response {
                    url: res.url,
                    status: 200,
                    status_text: "OK".into(),
                    headers: h,
                    body: res.bytes,
                    redirects,
                    protocol: Protocol::Local,
                    cache: CacheUse::Network,
                };
                return Ok((r, list));
            }
            let extra = HttpExtra {
                method: &method,
                body: body.as_deref().map(|b| (b, None)),
                headers: &headers,
                credentials: req.credentials,
                cache,
            };
            let mut response = self.http(&url, &extra, referrer.as_ref(), "*/*")?;
            response.redirects = redirects;
            if !matches!(response.status, 301 | 302 | 303 | 307 | 308) {
                return Ok((response, list));
            }
            let Some(location) = response.headers.get("location").map(str::to_owned) else {
                return Ok((response, list));
            };
            match req.redirect {
                RedirectMode::Manual => return Ok((response, list)),
                RedirectMode::Error => {
                    return Err(FetchError::Io(io::Error::other("redirect mode is error")));
                }
                RedirectMode::Follow => {}
            }
            let Ok(mut next) = Url::parse(&location, Some(&url)) else {
                return Err(FetchError::InvalidUrl);
            };
            if next.fragment.is_none() {
                next.fragment.clone_from(&url.fragment);
            }
            // Fetch §4.4 paso 12: 301/302 con POST y 303 (salvo GET/HEAD) pasan a GET sin
            // cuerpo ni cabeceras de contenido.
            let post = method.eq_ignore_ascii_case("POST");
            let get_or_head =
                method.eq_ignore_ascii_case("GET") || method.eq_ignore_ascii_case("HEAD");
            if (matches!(response.status, 301 | 302) && post)
                || (response.status == 303 && !get_or_head)
            {
                method = "GET".into();
                body = None;
                headers.0.retain(|(n, _)| {
                    !matches!(
                        n.to_ascii_lowercase().as_str(),
                        "content-encoding"
                            | "content-language"
                            | "content-location"
                            | "content-type"
                    )
                });
            }
            referrer = Some(url.clone());
            url = next;
            list.push(url.clone());
        }
        Err(FetchError::TooManyRedirects)
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
    /// Con la caché HTTP en memoria.
    pub fn new() -> NetLoader {
        NetLoader::with_cache(HttpCache::in_memory())
    }

    /// Con la caché HTTP en memoria y en disco, en [`cache::default_dir`]
    /// (`%LOCALAPPDATA%\Freedom\cache` en Windows).
    pub fn with_disk_cache() -> NetLoader {
        match cache::default_dir() {
            Some(dir) => NetLoader::with_cache(HttpCache::on_disk(dir)),
            None => NetLoader::new(),
        }
    }

    pub fn with_cache(cache: HttpCache) -> NetLoader {
        let mut net = NetLoader::default();
        net.fetcher.cache = Some(cache);
        net
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
