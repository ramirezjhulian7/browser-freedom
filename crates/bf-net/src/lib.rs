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

pub mod auth;
pub mod blob; // E2: el almacén de URLs blob:
pub mod cache;
pub mod cookies;
pub mod dns;
mod error;
mod eyeballs;
pub mod hpack;
pub mod hsts;
pub mod http;
pub mod http2;
pub mod psl;
pub mod referrer;
pub mod sniff;
pub mod streaming; // E2: respuestas por partes (EventSource)
pub mod websocket; // E2: cliente WebSocket (RFC 6455)

use std::collections::{HashMap, HashSet};
use std::io::{self, BufReader, Read, Write};
use std::net::{IpAddr, TcpStream};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use bf_infra::resource::load_local;
use bf_infra::{Resource, ResourceLoader};
use bf_url::{Host, Url};

pub use cache::{CacheMode, CacheUse, HttpCache};
pub use cookies::CookieJar;
pub use dns::Dns;
pub use error::{FetchError, NetErrorKind, TlsError, TlsReason};
pub use hsts::HstsStore;
pub use http::Headers;
pub use referrer::ReferrerPolicy;
pub use sniff::{EncodingSource, MimeType, html_encoding, html_encoding_with_source};

/// El `User-Agent` de Freedom (ver [`bf_infra::identity`]): los datos de Chrome, con Freedom
/// como producto y como motor.
pub use bf_infra::identity::USER_AGENT;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: usize = 20;
const MAX_BODY: u64 = 64 << 20;

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
    /// La política con la que se calcula `Referer` desde `referrer` (Referrer Policy §8.3).
    pub referrer_policy: ReferrerPolicy,
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
            referrer_policy: ReferrerPolicy::Default,
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

/// La IP de una conexión h2 por TLS y la cadena de certificados que presentó el servidor.
type H2Peer = (IpAddr, Vec<rustls_pki_types::CertificateDer<'static>>);

/// Conexiones HTTP/1.1 a la vez con un origen (`host`, puerto, TLS), como los navegadores (RFC
/// 9112 §9.4 pide limitarlas; Chrome y Firefox usan 6). Las peticiones de más esperan a que
/// una quede libre y la reutilizan (keep-alive, RFC 9112 §9.3).
pub const MAX_CONNECTIONS_PER_HOST: usize = 6;

/// Las conexiones HTTP/1.1 de un [`Fetcher`].
#[derive(Default)]
struct H1Pool {
    /// Libres para reutilizar.
    idle: HashMap<PoolKey, Vec<BufReader<Stream>>>,
    /// Abiertas (libres, en uso o abriéndose), por origen.
    open: HashMap<PoolKey, usize>,
}

/// Un lugar en el pool de un origen: una conexión libre o el permiso de abrir una.
enum Slot {
    Idle(BufReader<Stream>),
    New,
}

/// Mientras vive, ocupa un lugar del pool: al soltarse sin devolver la conexión, el lugar
/// queda libre para la siguiente petición en espera.
struct SlotGuard<'a> {
    fetcher: &'a Fetcher,
    key: &'a PoolKey,
    held: bool,
}

impl SlotGuard<'_> {
    /// La conexión vuelve al pool, libre.
    fn keep(mut self, conn: BufReader<Stream>) {
        self.held = false;
        let mut pool = self.fetcher.lock_pool();
        pool.idle.entry(self.key.clone()).or_default().push(conn);
        drop(pool);
        self.fetcher.pool_free.notify_all();
    }
}

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        if self.held {
            let mut pool = self.fetcher.lock_pool();
            if let Some(n) = pool.open.get_mut(self.key) {
                *n = n.saturating_sub(1);
            }
            drop(pool);
            self.fetcher.pool_free.notify_all();
        }
    }
}

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

fn server_name(host: &str) -> Result<rustls_pki_types::ServerName<'static>, FetchError> {
    rustls_pki_types::ServerName::try_from(
        host.trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned(),
    )
    .map_err(|e| FetchError::Tls(TlsError::new(TlsReason::Other, e.to_string())))
}

/// La resolución de las conexiones que no son de un [`Fetcher`] (WebSocket, respuestas por
/// partes): el resolvedor del sistema con caché.
fn system_dns() -> &'static Dns {
    static DNS: OnceLock<Dns> = OnceLock::new();
    DNS.get_or_init(Dns::default)
}

fn connect_tcp(host: &str, port: u16) -> Result<TcpStream, FetchError> {
    connect_tcp_with(system_dns(), host, port, READ_TIMEOUT)
}

fn attempt_tcp(a: std::net::SocketAddr) -> io::Result<TcpStream> {
    TcpStream::connect_timeout(&a, CONNECT_TIMEOUT)
}

/// Conexión TCP con Happy Eyeballs (RFC 8305): la resolución de [`dns`] (§3, con caché), las
/// direcciones de las dos familias intercaladas y los intentos escalonados cada 250 ms en
/// paralelo ([`eyeballs`], §4–5); la familia que se resolvió tarde se suma a la lista en curso.
/// Sin ruta hacia fuera ([`dns::has_route`]), un fallo de resolución es "sin conexión"
/// (`NetworkUnreachable`, [`NetErrorKind::Unreachable`]) y no "el nombre no existe".
fn connect_tcp_with(
    dns: &Dns,
    host: &str,
    port: u16,
    read_timeout: Duration,
) -> Result<TcpStream, FetchError> {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let resolution = dns.resolve(bare).map_err(|e| {
        if dns::has_route() {
            FetchError::Dns(e)
        } else {
            FetchError::Io(io::Error::new(
                io::ErrorKind::NetworkUnreachable,
                format!("sin conexión a la red ({e})"),
            ))
        }
    })?;
    let addrs = eyeballs::interleave(Dns::socket_addrs(&resolution.addrs, port));
    let late = resolution.late.map(|rx| (rx, port));
    let tcp = eyeballs::race_with_late(
        addrs,
        late,
        eyeballs::ATTEMPT_DELAY,
        CONNECT_TIMEOUT,
        attempt_tcp,
    )?;
    tcp.set_read_timeout(Some(read_timeout))?;
    tcp.set_write_timeout(Some(read_timeout))?;
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
        .map_err(|e| FetchError::Tls(TlsError::from_rustls(&e)))?;
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
    /// HTTP/1.1: conexiones libres y abiertas por origen, a lo sumo
    /// [`MAX_CONNECTIONS_PER_HOST`]; las peticiones de más esperan en `pool_free`.
    pool: Mutex<H1Pool>,
    pool_free: Condvar,
    /// HTTP/2: una conexión por origen, compartida por todas las peticiones (RFC 9113 §9.1).
    h2: Mutex<HashMap<PoolKey, Arc<http2::Connection>>>,
    /// RFC 9113 §9.1.1: la IP y la cadena de certificados de cada conexión h2 por TLS, para
    /// reutilizarla con otro origen (*connection coalescing*).
    h2_peers: Mutex<HashMap<PoolKey, H2Peer>>,
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
    /// Esas raíces, para verificar el certificado de una conexión h2 con otro nombre.
    roots: Option<Arc<rustls::RootCertStore>>,
    /// Registro de las peticiones (para el panel del motor).
    log: Mutex<Vec<LogEntry>>,
    /// La caché HTTP (RFC 9111); `None`: sin caché.
    pub cache: Option<HttpCache>,
    /// Los *Known HSTS Hosts* (RFC 6797).
    pub hsts: HstsStore,
    /// Autenticación HTTP: quien aporta credenciales ante un 401/407 ([`auth`]).
    pub authenticator: Option<Arc<auth::Authenticator>>,
    /// Las credenciales aceptadas, por origen y *realm*.
    auth: auth::AuthCache,
    /// La resolución de nombres con caché (RFC 8305 §3); el resolvedor se puede cambiar con
    /// `Fetcher::dns = Arc::new(Dns::new(...))`.
    pub dns: Arc<Dns>,
    /// Cuánto se espera a que el servidor mande algo (o acepte lo que se le manda) en una
    /// conexión abierta; `None`: 30 s. Los tests lo acortan.
    pub read_timeout: Option<Duration>,
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
                make_tls_config(store.clone(), &[b"http/1.1"]),
            )),
            roots: Some(Arc::new(store)),
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
        let mut tcp = connect_tcp_with(
            &self.dns,
            host,
            port,
            self.read_timeout.unwrap_or(READ_TIMEOUT),
        )?;
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
            .map_err(|e| FetchError::Tls(TlsError::from_rustls(&e)))?;
        while conn.is_handshaking() {
            conn.complete_io(&mut tcp).map_err(error::tls_io_error)?;
        }
        if conn.alpn_protocol() == Some(b"h2") {
            let certs = conn
                .peer_certificates()
                .map(|c| c.iter().map(|c| c.clone().into_owned()).collect())
                .unwrap_or_default();
            if let Ok(peer) = tcp.peer_addr() {
                self.h2_peers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(key.clone(), (peer.ip(), certs));
            }
            let transport = http2::Transport::Tls(Box::new(conn), tcp);
            return Ok(Opened::H2(http2::start(transport)?));
        }
        let stream = rustls::StreamOwned::new(conn, tcp);
        Ok(Opened::H1(Stream::Tls(Box::new(stream))))
    }

    fn lock_pool(&self) -> MutexGuard<'_, H1Pool> {
        self.pool.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Un lugar en el pool HTTP/1.1 de `key`: una conexión libre (si `reuse`) o el permiso de
    /// abrir una si hay menos de [`MAX_CONNECTIONS_PER_HOST`]; si no, espera. Sin `reuse` (las
    /// libres pueden estar cerradas), con el pool lleno se cierra una libre para abrir otra.
    fn h1_slot<'a>(&'a self, key: &'a PoolKey, reuse: bool) -> (Slot, SlotGuard<'a>) {
        let mut pool = self.lock_pool();
        let guard = SlotGuard {
            fetcher: self,
            key,
            held: true,
        };
        loop {
            if reuse && let Some(c) = pool.idle.get_mut(key).and_then(Vec::pop) {
                return (Slot::Idle(c), guard);
            }
            let open = pool.open.entry(key.clone()).or_default();
            if *open < MAX_CONNECTIONS_PER_HOST {
                *open += 1;
                return (Slot::New, guard);
            }
            if !reuse && pool.idle.get_mut(key).and_then(Vec::pop).is_some() {
                return (Slot::New, guard);
            }
            pool = self.pool_free.wait(pool).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Cuántas conexiones HTTP/1.1 hay abiertas con el origen de `url` (libres o en uso).
    pub fn open_connections(&self, url: &Url) -> usize {
        let tls = url.scheme == "https";
        let Some(host) = url.host.as_ref().map(ToString::to_string) else {
            return 0;
        };
        let key: PoolKey = (host, url.port.unwrap_or(if tls { 443 } else { 80 }), tls);
        self.lock_pool().open.get(&key).copied().unwrap_or(0)
    }

    /// RFC 9113 §9.1.1 (*connection reuse*): una conexión h2 por TLS abierta con otro origen
    /// sirve para el `https` de `key` si `host` resuelve a la IP a la que está conectada y su
    /// certificado es válido para `host` (se verifica de nuevo, con las mismas raíces). Se
    /// apunta también con `key`.
    fn coalesce(&self, host: &str, key: &PoolKey) -> Option<Arc<http2::Connection>> {
        use rustls::client::danger::ServerCertVerifier as _;
        let candidates: Vec<(PoolKey, H2Peer)> = self
            .h2_peers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(k, _)| k.1 == key.1 && k.2 && *k != key)
            .map(|(k, p)| (k.clone(), p.clone()))
            .collect();
        if !key.2 || candidates.is_empty() {
            return None;
        }
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        let addrs: Vec<IpAddr> = self.dns.lookup_all(bare).ok()?;
        let name = server_name(host).ok()?;
        let verifier = self.verifier()?;
        for (other, (ip, certs)) in candidates {
            let Some((end_entity, intermediates)) = certs.split_first() else {
                continue;
            };
            if !addrs.contains(&ip) {
                continue;
            }
            let Some(conn) = self.h2_connection(&other) else {
                continue;
            };
            let now = rustls_pki_types::UnixTime::now();
            if verifier
                .verify_server_cert(end_entity, intermediates, &name, &[], now)
                .is_ok()
            {
                self.h2
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(key.clone(), conn.clone());
                return Some(conn);
            }
        }
        None
    }

    /// El verificador de certificados de este `Fetcher` (sus raíces o las de webpki-roots).
    fn verifier(&self) -> Option<Arc<rustls::client::WebPkiServerVerifier>> {
        let build = |roots: Arc<rustls::RootCertStore>| {
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            rustls::client::WebPkiServerVerifier::builder_with_provider(roots, provider)
                .build()
                .ok()
        };
        match &self.roots {
            Some(roots) => build(roots.clone()),
            None => {
                static WEB: OnceLock<Option<Arc<rustls::client::WebPkiServerVerifier>>> =
                    OnceLock::new();
                WEB.get_or_init(|| build(Arc::new(web_roots()))).clone()
            }
        }
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
        // RFC 6797 §8.1: solo la primera `Strict-Transport-Security`, y solo sobre TLS.
        if let Some(sts) = headers.get("strict-transport-security") {
            self.hsts.note(url, sts);
        }
        // Un cuerpo vacío (304, 204, HEAD) no se decodifica: un 304 puede traer
        // `Content-Encoding: gzip` de la representación guardada (RFC 9110 §15.4.5).
        let body = if raw.is_empty() {
            raw
        } else {
            http::decode_content(raw, &headers.list("content-encoding"))?
        };
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
        // Fetch §2.2.5: el referente de la petición no cambia con las redirecciones; la
        // política sí, si la respuesta trae `Referrer-Policy` (§4.4 paso 14).
        let referrer = opts.referrer.as_ref();
        let mut policy = opts.referrer_policy;
        for redirects in 0..=MAX_REDIRECTS {
            if url.scheme == "blob" {
                return blob_response(&url, redirects);
            }
            // RFC 6797 §8.3 (Fetch §4.1 "main fetch" paso 7): a `https:` si el host es HSTS.
            if let Some(up) = self.hsts.upgrade(&url) {
                url = up;
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
                referrer_policy: policy,
            };
            let mut response = self.http(&url, &extra, referrer, &opts.accept)?;
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
            if let Some(p) = response
                .headers
                .get("referrer-policy")
                .and_then(ReferrerPolicy::from_header)
            {
                policy = p;
            }
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
        // Referrer Policy §8.3 "determine request's referrer" (Fetch §4.5 paso 8.10).
        if let Some(r) = referrer.and_then(|r| referrer::determine(r, url, extra.referrer_policy)) {
            headers.push("Referer", &r.href());
        }
        if let Some((_, Some(ct))) = body
            && !has("content-type")
        {
            headers.push("Content-Type", ct);
        }
        for (n, v) in &extra.headers.0 {
            headers.push(n, v);
        }
        let target = request_target(url);
        self.authenticate(url, extra, &headers, &target, has("authorization"))
    }

    /// Credenciales para el *realm* `realm` del origen de `url`: se mandan cuando el servidor
    /// lo pida (ver [`auth`]). Para reintentar tras un 401 sin [`Fetcher::authenticator`].
    pub fn set_credentials(&self, url: &Url, realm: &str, creds: auth::Credentials) {
        self.auth.remember(url, false, realm, creds);
    }

    /// RFC 9110 §11 (Fetch §4.5 pasos 8.21 y 14): el salto con autenticación. Manda sin esperar
    /// el reto las credenciales guardadas para el espacio de `url` o, si no hay, las de la URL
    /// (Basic). Ante un 401/407 con un reto Basic o Digest, busca credenciales (guardadas, de la
    /// URL, del embebedor) y repite; si el servidor las acepta, las guarda. Las peticiones sin
    /// credenciales (`credentials: omit`) o con `Authorization` del autor no se autentican.
    fn authenticate(
        &self,
        url: &Url,
        extra: &HttpExtra<'_>,
        headers: &Headers,
        target: &str,
        author_authorization: bool,
    ) -> Result<Response, FetchError> {
        const MAX_ATTEMPTS: usize = 4;
        let method = extra.method;
        let allowed = extra.credentials && !author_authorization;
        let url_creds = auth::Credentials::from_url(url);
        let mut sent: Option<(&str, String)> = None;
        let mut url_basic_sent = false;
        if allowed {
            if let Some(a) = self.auth.preemptive(url, method, target) {
                sent = Some(("Authorization", a));
            } else if let Some(c) = &url_creds {
                sent = Some(("Authorization", auth::basic(c)));
                url_basic_sent = true;
            }
        }
        let mut used_url = false;
        let mut used_cached: Option<String> = None;
        // Las credenciales del último intento.
        let mut current: Option<Attempt> = None;
        let mut attempts = 0;
        loop {
            let mut h = headers.clone();
            if let Some((name, value)) = &sent {
                h.push(name, value);
            }
            let response = self.hop(url, extra, &h, target)?;
            let proxy = response.status == 407;
            if !allowed || !(response.status == 401 || proxy) {
                if !matches!(response.status, 401 | 407)
                    && let Some(a) = current
                {
                    self.auth.store(url, a.proxy, &a.realm, a.creds, a.digest);
                }
                return Ok(response);
            }
            if attempts >= MAX_ATTEMPTS {
                return Ok(response);
            }
            let header = if proxy {
                "proxy-authenticate"
            } else {
                "www-authenticate"
            };
            let challenges = auth::challenges(&response.headers, header);
            let Some(chosen) = auth::choose(&challenges) else {
                return Ok(response);
            };
            let realm = chosen.realm().unwrap_or("").to_owned();
            let mut digest = if chosen.scheme == "digest" {
                auth::DigestState::from_challenge(chosen)
            } else {
                None
            };
            // RFC 7616 §3.3: `stale=true`: las credenciales valían, el `nonce` no.
            let stale = digest.is_some()
                && chosen
                    .param("stale")
                    .is_some_and(|v| v.eq_ignore_ascii_case("true"));
            if url_basic_sent && chosen.scheme == "basic" {
                used_url = true;
            }
            url_basic_sent = false;
            if let Some(a) = &current
                && !stale
            {
                self.auth.forget(url, a.proxy, &a.realm);
            }
            let origin = auth::origin_of(url);
            let creds = if let (true, Some(a)) = (stale, &current) {
                Some(a.creds.clone())
            } else if used_cached.as_deref() != Some(realm.as_str())
                && let Some(c) = self.auth.credentials(&origin, proxy, &realm)
            {
                used_cached = Some(realm.clone());
                Some(c)
            } else if !used_url && let Some(c) = &url_creds {
                used_url = true;
                Some(c.clone())
            } else if let Some(ask) = &self.authenticator {
                let mut bare = url.clone();
                bare.username.clear();
                bare.password.clear();
                ask(&auth::AuthPrompt {
                    url: bare,
                    proxy,
                    realm: chosen.realm().map(str::to_owned),
                    scheme: chosen.scheme.clone(),
                    challenges: challenges.clone(),
                    retry: current.is_some(),
                })
            } else {
                None
            };
            let Some(creds) = creds else {
                return Ok(response);
            };
            let value = match &mut digest {
                Some(d) => d.authorization(&creds, method, target, &auth::cnonce()),
                None => auth::basic(&creds),
            };
            let name = if proxy {
                "Proxy-Authorization"
            } else {
                "Authorization"
            };
            sent = Some((name, value));
            current = Some(Attempt {
                realm,
                proxy,
                creds,
                digest,
            });
            attempts += 1;
        }
    }

    /// Un salto: por la caché HTTP (si hay) o directo a la red, y al registro.
    fn hop(
        &self,
        url: &Url,
        extra: &HttpExtra<'_>,
        headers: &Headers,
        target: &str,
    ) -> Result<Response, FetchError> {
        let method = extra.method;
        let response = match &self.cache {
            None => self.send(url, extra, headers, target)?,
            Some(cache) => {
                let req = cache::CacheRequest {
                    method,
                    url,
                    headers,
                    mode: extra.cache,
                    credentials: extra.credentials,
                };
                let mut protocol = Protocol::Local;
                let (r, used) = cache.fetch_mut(&req, |h| {
                    let r = self.send(url, extra, h, target)?;
                    protocol = r.protocol;
                    Ok::<_, FetchError>(cache::CacheResponse {
                        status: r.status,
                        status_text: r.status_text,
                        headers: r.headers,
                        body: r.body,
                    })
                })?;
                for rv in cache.take_revalidations() {
                    self.revalidate_in_background(cache.clone(), rv);
                }
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

    /// RFC 5861 §3: valida `rv` en otro hilo, con un `Fetcher` propio que comparte la jarra de
    /// cookies y la lista HSTS de este (las `Set-Cookie` y `Strict-Transport-Security` de la
    /// validación llegan aquí), las raíces TLS y el h2c; sus conexiones van aparte. Las
    /// cabeceras son las de la petición original (cookies incluidas).
    fn revalidate_in_background(&self, cache: HttpCache, rv: cache::Revalidation) {
        let tls = self.tls.clone();
        let roots = self.roots.clone();
        let cookies = self.cookies.clone();
        let hsts = self.hsts.clone();
        let dns = self.dns.clone();
        let h2_prior_knowledge = self.h2_prior_knowledge;
        std::thread::spawn(move || {
            let bg = Fetcher {
                tls,
                roots,
                cookies,
                hsts,
                dns,
                h2_prior_knowledge,
                ..Fetcher::default()
            };
            let none = Headers::default();
            let extra = HttpExtra {
                method: "GET",
                body: None,
                headers: &none,
                credentials: rv.credentials,
                cache: CacheMode::NoCache,
                referrer_policy: ReferrerPolicy::Default,
            };
            let target = request_target(&rv.url);
            let _ = cache.revalidate(&rv, |h| {
                let r = bg.send(&rv.url, &extra, h, &target)?;
                Ok::<_, FetchError>(cache::CacheResponse {
                    status: r.status,
                    status_text: r.status_text,
                    headers: r.headers,
                    body: r.body,
                })
            });
        });
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
                    target,
                    headers,
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
                    // La causa, para la página de error: la conexión que se cae es un corte;
                    // un error de protocolo o un flujo reiniciado, una respuesta inválida.
                    Err(http2::Error::Io(e)) => return Err(FetchError::from(e)),
                    Err(e @ http2::Error::Lost) => {
                        return Err(FetchError::Io(io::Error::new(
                            io::ErrorKind::ConnectionReset,
                            e.to_string(),
                        )));
                    }
                    Err(e) => {
                        return Err(FetchError::Io(io::Error::new(
                            io::ErrorKind::InvalidData,
                            e.to_string(),
                        )));
                    }
                }
                self.forget_h2(&key, &conn);
                continue;
            }
            if tls
                && !self
                    .h1_only
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .contains(&key)
                && !self
                    .h1_known
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .contains(&key)
                && self.coalesce(&host, &key).is_some()
            {
                continue;
            }
            let (slot, guard) = self.h1_slot(&key, !fresh);
            let reused = matches!(slot, Slot::Idle(_));
            let mut conn = match slot {
                Slot::Idle(c) => c,
                Slot::New => {
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
                    target,
                    headers,
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
                guard.keep(conn);
            } else {
                drop(guard);
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

/// Las credenciales de un intento de autenticación (para guardarlas si el servidor las acepta).
struct Attempt {
    realm: String,
    proxy: bool,
    creds: auth::Credentials,
    digest: Option<auth::DigestState>,
}

/// El *request-target* en forma de origen (RFC 9112 §3.2.1): ruta y consulta.
fn request_target(url: &Url) -> String {
    let mut t = url.path_string();
    if t.is_empty() {
        t.push('/');
    }
    if let Some(q) = &url.query {
        t.push('?');
        t.push_str(q);
    }
    t
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
    referrer_policy: ReferrerPolicy,
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
    /// La política con la que se calcula `Referer` desde `referrer` (Referrer Policy §8.3);
    /// aplicarla a un referente ya calculado con la misma política no lo cambia.
    pub referrer_policy: ReferrerPolicy,
    /// El *cache mode* (Fetch §2.2.5, `RequestInit.cache`).
    pub cache: CacheMode,
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
            referrer_policy: ReferrerPolicy::Default,
            cache: CacheMode::Default,
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
    /// La caché HTTP se consulta según `req.cache` (Fetch §4.5).
    pub fn fetch_request(
        &self,
        url: &Url,
        req: &RequestOptions,
    ) -> Result<(Response, Vec<Url>), FetchError> {
        let cache = req.cache;
        let mut url = url.clone();
        let mut method = req.method.clone();
        let mut body = req.body.clone();
        let mut headers = req.headers.clone();
        // Fetch §2.2.5: el referente no cambia con las redirecciones; la política sí (§4.4
        // paso 14).
        let referrer = req.referrer.as_ref();
        let mut policy = req.referrer_policy;
        let mut list = vec![url.clone()];
        for redirects in 0..=MAX_REDIRECTS {
            // RFC 6797 §8.3 (Fetch §4.1 "main fetch" paso 7): la URL actual pasa a `https:`.
            if let Some(up) = self.hsts.upgrade(&url) {
                url = up;
                if let Some(last) = list.last_mut() {
                    *last = url.clone();
                }
            }
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
                referrer_policy: policy,
            };
            let mut response = self.http(&url, &extra, referrer, "*/*")?;
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
            if let Some(p) = response
                .headers
                .get("referrer-policy")
                .and_then(ReferrerPolicy::from_header)
            {
                policy = p;
            }
            url = next;
            list.push(url.clone());
        }
        Err(FetchError::TooManyRedirects)
    }
}

/// Dónde guarda Freedom sus datos (HSTS, cookies): la carpeta del perfil. `FREEDOM_PROFILE`
/// la cambia, como en `bf-shell` (`profile::dir`), para que historial, cookies y HSTS vayan
/// juntos; si no, `%LOCALAPPDATA%\Freedom` en Windows, `~/Library/Application Support/Freedom`
/// en macOS y `$XDG_DATA_HOME/freedom` (o `~/.local/share/freedom`) en el resto. Si cambia
/// una, cambia la otra.
pub fn data_dir() -> Option<std::path::PathBuf> {
    data_dir_with(|name| std::env::var_os(name), cfg!(target_os = "macos"))
}

/// [`data_dir`] con las variables de entorno de `var` y la plataforma `mac` (para los tests).
fn data_dir_with(
    var: impl Fn(&str) -> Option<std::ffi::OsString>,
    mac: bool,
) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    let var = |name: &str| var(name).filter(|v| !v.is_empty());
    if let Some(profile) = var("FREEDOM_PROFILE") {
        return Some(PathBuf::from(profile));
    }
    if mac {
        return var("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/Freedom"));
    }
    if let Some(local) = var("LOCALAPPDATA") {
        return Some(PathBuf::from(local).join("Freedom"));
    }
    if let Some(xdg) = var("XDG_DATA_HOME") {
        return Some(PathBuf::from(xdg).join("freedom"));
    }
    var("HOME").map(|h| {
        PathBuf::from(h)
            .join(".local")
            .join("share")
            .join("freedom")
    })
}

#[cfg(test)]
mod data_dir_tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn with(vars: &[(&str, &str)], mac: bool) -> Option<PathBuf> {
        super::data_dir_with(
            |name| {
                vars.iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| OsString::from(v))
            },
            mac,
        )
    }

    #[test]
    fn profile_variable_wins() {
        let vars = [
            ("FREEDOM_PROFILE", "/tmp/perfil"),
            ("HOME", "/Users/u"),
            ("LOCALAPPDATA", "C:\\L"),
        ];
        for mac in [false, true] {
            assert_eq!(with(&vars, mac), Some(PathBuf::from("/tmp/perfil")));
        }
        // Vacía no cuenta.
        let vars = [("FREEDOM_PROFILE", ""), ("HOME", "/Users/u")];
        assert_eq!(
            with(&vars, true),
            Some(PathBuf::from(
                "/Users/u/Library/Application Support/Freedom"
            ))
        );
        assert_eq!(
            with(&vars, false),
            Some(PathBuf::from("/Users/u/.local/share/freedom"))
        );
        assert_eq!(
            with(&[("XDG_DATA_HOME", "/x")], false),
            Some(PathBuf::from("/x/freedom"))
        );
        assert_eq!(with(&[], true), None);
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
    /// (`%LOCALAPPDATA%\Freedom\cache` en Windows, `~/Library/Caches/Freedom` en macOS).
    /// El perfil en disco: la caché HTTP en [`cache::default_dir`] y, en [`data_dir`], la
    /// lista HSTS (`hsts.txt`) y las cookies persistentes (`cookies.txt`).
    pub fn with_disk_cache() -> NetLoader {
        let mut net = match cache::default_dir() {
            Some(dir) => NetLoader::with_cache(HttpCache::on_disk(dir)),
            None => NetLoader::new(),
        };
        if let Some(dir) = data_dir() {
            net.fetcher.hsts = HstsStore::on_disk(dir.join("hsts.txt"));
            net.fetcher.cookies = CookieJar::on_disk(dir.join("cookies.txt"));
        }
        net
    }

    pub fn with_cache(cache: HttpCache) -> NetLoader {
        let mut net = NetLoader::default();
        net.fetcher.cache = Some(cache);
        net
    }

    /// Como [`ResourceLoader::load`], con las opciones de la petición: el shell pasa el
    /// referente y la política del documento a sus subrecursos (Fetch §2.2.5, *client*).
    pub fn load_with(&self, url: &Url, opts: &FetchOptions) -> Option<Resource> {
        match self.fetcher.fetch(url, opts) {
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

impl ResourceLoader for NetLoader {
    fn load(&self, url: &Url) -> Option<Resource> {
        self.load_with(url, &FetchOptions::default())
    }
}
