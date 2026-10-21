//! La red desde JavaScript: Fetch Standard (`fetch`, `Request`, `Response`, `Headers`),
//! XMLHttpRequest, `FormData`, File API (`Blob`, `File`, `FileReader`, `blob:` URLs) y
//! `navigator.sendBeacon`. **Dueño: E2.**
//!
//! # El camino de una petición
//!
//! Las peticiones las hace un [`NetBackend`] (bloqueante y `Send + Sync`): por defecto el
//! `Fetcher` de `bf-net` del proceso; en el runner de WPT, el `ResourceLoader` de la página
//! ([`LoaderBackend`]); el shell instala el suyo (el `NetLoader` de la pestaña, que comparte
//! la jarra de cookies) con [`set_backend`].
//!
//! - **Con hilo auxiliar** (`threaded = true`, el shell): cada petición corre en un hilo; el
//!   resultado se deja en una bandeja y se llama al *waker* ([`set_waker`]); el bucle de la
//!   pestaña llama a [`pump`], que encola una tarea de la fuente *networking* por resultado.
//! - **Sin hilo** (tests, runner, `bf-js run`): la petición se hace en el momento y la tarea
//!   se encola enseguida (el resultado sigue llegando de forma asíncrona para el script).
//!
//! `Request.body`/`Response.body` son `null`: no hay Streams todavía; los cuerpos se leen
//! enteros (`text()`, `json()`, `arrayBuffer()`, `blob()`, `formData()`, `bytes()`).

pub mod blob;
pub mod body;
pub mod fetch;
pub mod formdata;
pub mod headers;
pub mod wptserve;
pub mod xhr;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use bf_infra::{ResourceLoader, Url};
use bf_js::host::{HostClassId, TaskSource};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::{Agent, JsResult, RealmId};

use crate::WebContextHandle;
use headers::HeaderList;

// --- El backend ---

/// Una petición lista para la red.
#[derive(Debug, Clone)]
pub struct NetRequest {
    pub url: Url,
    pub options: bf_net::RequestOptions,
}

/// Una respuesta de la red (antes de filtrarla por CORS).
#[derive(Debug, Clone)]
pub struct NetResponse {
    /// Las URLs visitadas (la última es la de la respuesta).
    pub url_list: Vec<Url>,
    pub status: u16,
    pub status_text: String,
    pub headers: HeaderList,
    pub body: Vec<u8>,
}

/// `Err` es un "network error" (Fetch §2.2.6) con un mensaje para la consola.
pub type NetResult = Result<NetResponse, String>;

/// Quien hace las peticiones (bloqueante; se puede llamar desde cualquier hilo).
pub trait NetBackend: Send + Sync {
    fn fetch(&self, req: &NetRequest) -> NetResult;
}

fn from_bf_net(r: Result<(bf_net::Response, Vec<Url>), bf_net::FetchError>) -> NetResult {
    let (r, list) = r.map_err(|e| e.to_string())?;
    Ok(NetResponse {
        url_list: list,
        status: r.status,
        status_text: r.status_text,
        headers: HeaderList(r.headers.0),
        body: r.body,
    })
}

impl NetBackend for bf_net::Fetcher {
    fn fetch(&self, req: &NetRequest) -> NetResult {
        from_bf_net(self.fetch_request(&req.url, &req.options))
    }
}

impl NetBackend for bf_net::NetLoader {
    fn fetch(&self, req: &NetRequest) -> NetResult {
        NetBackend::fetch(&self.fetcher, req)
    }
}

/// Un backend sobre un `ResourceLoader` (archivos montados, `data:`): `200 OK` con el tipo
/// del recurso o el de la extensión; lo que no existe es un error de red.
pub struct LoaderBackend(pub Arc<dyn ResourceLoader>);

/// El tipo MIME por la extensión de la ruta (para los recursos sin `Content-Type`).
pub fn mime_for_path(path: &str) -> &'static str {
    let ext = path
        .rsplit('/')
        .next()
        .and_then(|f| f.rsplit_once('.'))
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html",
        "xhtml" => "application/xhtml+xml",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "txt" => "text/plain",
        "png" => "image/png",
        "gif" => "image/gif",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

impl NetBackend for LoaderBackend {
    fn fetch(&self, req: &NetRequest) -> NetResult {
        if req.url.scheme == "blob" {
            return NetBackend::fetch(&bf_net::Fetcher::new(), req);
        }
        let r = self
            .0
            .load(&req.url)
            .ok_or_else(|| format!("no se pudo cargar {}", req.url.href()))?;
        let ct = r
            .content_type
            .unwrap_or_else(|| mime_for_path(&req.url.path_string()).to_owned());
        let mut headers = HeaderList::default();
        headers.append("Content-Type", &ct);
        headers.append("Content-Length", &r.bytes.len().to_string());
        Ok(NetResponse {
            url_list: vec![req.url.clone()],
            status: 200,
            status_text: "OK".into(),
            headers,
            body: r.bytes,
        })
    }
}

/// El `Fetcher` del proceso (cuando nadie instaló un backend).
fn default_fetcher() -> Arc<bf_net::Fetcher> {
    static F: OnceLock<Arc<bf_net::Fetcher>> = OnceLock::new();
    F.get_or_init(|| Arc::new(bf_net::Fetcher::new())).clone()
}

type Done = Box<dyn FnOnce(&mut Agent, NetResult)>;

/// El estado de la red de una ventana (dato del realm).
pub struct NetState {
    backend: Option<Arc<dyn NetBackend>>,
    threaded: bool,
    inbox: Arc<Mutex<Vec<(u64, NetResult)>>>,
    waker: Option<Arc<dyn Fn() + Send + Sync>>,
    pending: HashMap<u64, Done>,
    next_id: u64,
    /// Los XMLHttpRequest y FileReader por id de target.
    pub(crate) xhrs: HashMap<u64, xhr::XhrState>,
    pub(crate) readers: HashMap<u64, blob::ReaderState>,
}

impl Default for NetState {
    fn default() -> Self {
        NetState {
            backend: None,
            threaded: false,
            inbox: Arc::new(Mutex::new(Vec::new())),
            waker: None,
            pending: HashMap::new(),
            next_id: 0,
            xhrs: HashMap::new(),
            readers: HashMap::new(),
        }
    }
}

fn state_of(agent: &mut Agent, realm: RealmId) -> &mut NetState {
    if agent.realm_data::<NetState>(realm).is_none() {
        agent.set_realm_data(realm, NetState::default());
    }
    agent.realm_data_mut::<NetState>(realm).expect("NetState")
}

/// El estado de la red de la ventana en curso.
pub fn state(agent: &mut Agent) -> &mut NetState {
    let realm = WebContextHandle::current(agent).realm;
    state_of(agent, realm)
}

/// Instala el backend de `realm`. Con `threaded`, cada petición corre en un hilo y el
/// embebedor debe llamar a [`pump`] cuando lo despierte el waker.
pub fn set_backend(
    agent: &mut Agent,
    realm: RealmId,
    backend: Arc<dyn NetBackend>,
    threaded: bool,
) {
    let s = state_of(agent, realm);
    s.backend = Some(backend);
    s.threaded = threaded;
}

/// La función que despierta el bucle del documento cuando llega un resultado (desde el hilo
/// de la red).
pub fn set_waker(agent: &mut Agent, realm: RealmId, waker: Arc<dyn Fn() + Send + Sync>) {
    state_of(agent, realm).waker = Some(waker);
}

/// Encola una tarea *networking* por cada resultado llegado de los hilos de red; devuelve
/// cuántas encoló.
pub fn pump(agent: &mut Agent, realm: RealmId) -> usize {
    let done: Vec<(u64, NetResult)> = {
        let s = state_of(agent, realm);
        std::mem::take(&mut *s.inbox.lock().unwrap_or_else(|e| e.into_inner()))
    };
    let mut n = 0;
    for (id, result) in done {
        let Some(cb) = state_of(agent, realm).pending.remove(&id) else {
            continue;
        };
        queue_result(agent, realm, cb, result);
        n += 1;
    }
    n
}

/// Las peticiones en vuelo de `realm` (para que el embebedor sepa que debe esperar).
pub fn pending_requests(agent: &mut Agent, realm: RealmId) -> usize {
    state_of(agent, realm).pending.len()
}

fn queue_result(agent: &mut Agent, realm: RealmId, cb: Done, result: NetResult) {
    agent.queue_task(
        TaskSource::Networking,
        Box::new(move |agent: &mut Agent| {
            let saved = agent.current_realm;
            agent.current_realm = realm;
            cb(agent, result);
            agent.current_realm = saved;
        }),
    );
}

/// El backend de la ventana en curso.
fn backend(agent: &mut Agent) -> Arc<dyn NetBackend> {
    let realm = WebContextHandle::current(agent).realm;
    if let Some(b) = state_of(agent, realm).backend.clone() {
        return b;
    }
    if let Some(r) = agent.realm_data::<crate::html::scripting::PageResources>(realm) {
        let loader = r.loader.clone();
        let url = WebContextHandle::current(agent).get(agent).url.clone();
        // Las páginas de WPT: el docroot común y los handlers emulados de wptserve.
        if url.host.as_ref().is_some_and(|h| h.to_string().ends_with("web-platform.test")) {
            let mut origin = url.clone();
            origin.host = Url::parse("http://web-platform.test/", None).ok().and_then(|u| u.host);
            return Arc::new(wptserve::WptServe {
                files: LoaderBackend(loader),
                origin,
            });
        }
        return Arc::new(LoaderBackend(loader));
    }
    default_fetcher()
}

/// "Bad port" (Fetch §2.9): los puertos a los que nunca se conecta por HTTP(S).
pub fn is_bad_port(url: &Url) -> bool {
    const BAD: &[u16] = &[
        0, 1, 7, 9, 11, 13, 15, 17, 19, 20, 21, 22, 23, 25, 37, 42, 43, 53, 69, 77, 79, 87, 95,
        101, 102, 103, 104, 109, 110, 111, 113, 115, 117, 119, 123, 135, 137, 139, 143, 161, 179,
        389, 427, 465, 512, 513, 514, 515, 526, 530, 531, 532, 540, 548, 554, 556, 563, 587, 601,
        636, 989, 990, 993, 995, 1719, 1720, 1723, 2049, 3659, 4045, 4190, 5060, 5061, 6000,
        6566, 6665, 6666, 6667, 6668, 6669, 6679, 6697, 10080,
    ];
    matches!(url.scheme.as_str(), "http" | "https") && url.port.is_some_and(|p| BAD.contains(&p))
}

/// Empieza una petición; `done` corre en una tarea *networking* con el resultado. Devuelve
/// un id para [`cancel`].
pub fn start(agent: &mut Agent, req: NetRequest, done: Done) -> u64 {
    let realm = WebContextHandle::current(agent).realm;
    let b = backend(agent);
    let s = state_of(agent, realm);
    s.next_id += 1;
    let id = s.next_id;
    if is_bad_port(&req.url) {
        let msg = format!("Failed to fetch: bad port {}", req.url.href());
        queue_result(agent, realm, done, Err(msg));
        return id;
    }
    let s = state_of(agent, realm);
    if s.threaded {
        s.pending.insert(id, done);
        let inbox = s.inbox.clone();
        let waker = s.waker.clone();
        std::thread::spawn(move || {
            let r = b.fetch(&req);
            inbox
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((id, r));
            if let Some(w) = waker {
                w();
            }
        });
    } else {
        let r = b.fetch(&req);
        queue_result(agent, realm, done, r);
    }
    id
}

/// Olvida una petición en vuelo (su resultado se descarta).
pub fn cancel(agent: &mut Agent, id: u64) {
    state(agent).pending.remove(&id);
}

/// Una petición bloqueante (XMLHttpRequest síncrono).
pub fn fetch_blocking(agent: &mut Agent, req: &NetRequest) -> NetResult {
    if is_bad_port(&req.url) {
        return Err(format!("bad port {}", req.url.href()));
    }
    backend(agent).fetch(req)
}

// --- Orígenes y CORS (Fetch §3.2, simplificado: sin preflight) ---

/// El origen serializado de la ventana.
pub fn document_origin(agent: &mut Agent) -> String {
    WebContextHandle::current(agent)
        .get(agent)
        .url
        .origin()
        .to_string()
}

/// La URL base de la ventana.
pub fn base_url(agent: &mut Agent) -> Url {
    WebContextHandle::current(agent).get(agent).base.clone()
}

/// ¿Es `url` del mismo origen que el documento? (`data:` y `blob:` del mismo origen cuentan.)
pub fn same_origin(agent: &mut Agent, url: &Url) -> bool {
    if url.scheme == "data" {
        return true;
    }
    let mine = document_origin(agent);
    if url.scheme == "blob" {
        return url.href().starts_with(&format!("blob:{mine}/"));
    }
    url.origin().to_string() == mine
}

/// "CORS check" (Fetch §4.9): `Access-Control-Allow-Origin` (y `-Credentials` con cookies).
pub fn cors_check(origin: &str, headers: &HeaderList, credentials: bool) -> bool {
    let Some(acao) = headers.get("access-control-allow-origin") else {
        return false;
    };
    if acao == "*" && !credentials {
        return true;
    }
    if acao != origin {
        return false;
    }
    !credentials || headers.get("access-control-allow-credentials").as_deref() == Some("true")
}

/// "CORS-safelisted response-header names" más los de `Access-Control-Expose-Headers`.
pub fn cors_filter(headers: &HeaderList) -> HeaderList {
    const SAFE: &[&str] = &[
        "cache-control",
        "content-language",
        "content-length",
        "content-type",
        "expires",
        "last-modified",
        "pragma",
    ];
    let exposed: Vec<String> = headers
        .get("access-control-expose-headers")
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default();
    let star = exposed.iter().any(|e| e == "*");
    HeaderList(
        headers
            .0
            .iter()
            .filter(|(n, _)| {
                let n = n.to_ascii_lowercase();
                !headers::is_forbidden_response_header(&n)
                    && (SAFE.contains(&n.as_str()) || star || exposed.contains(&n))
            })
            .cloned()
            .collect(),
    )
}

/// El filtro "basic": sin `Set-Cookie`.
pub fn basic_filter(headers: &HeaderList) -> HeaderList {
    HeaderList(
        headers
            .0
            .iter()
            .filter(|(n, _)| !headers::is_forbidden_response_header(n))
            .cloned()
            .collect(),
    )
}

// --- Clases ---

/// Las clases host de la red (una vez por agente).
#[derive(Debug, Clone)]
pub struct NetClasses {
    pub headers: HostClassId,
    pub request: HostClassId,
    pub response: HostClassId,
    pub blob: HostClassId,
    pub file: HostClassId,
    pub file_reader: HostClassId,
    pub form_data: HostClassId,
    pub progress_event: HostClassId,
    pub xhr_event_target: HostClassId,
    pub xhr: HostClassId,
    pub xhr_upload: HostClassId,
}

pub fn classes(agent: &Agent) -> &NetClasses {
    agent
        .embedder_data::<NetClasses>()
        .expect("bf_web::net::register no se llamó")
}

/// Registra las clases de la red (lo llama [`crate::register_all`]).
pub fn register(agent: &mut Agent, realm: RealmId, _ctx: &WebContextHandle) {
    if agent.embedder_data::<NetClasses>().is_none() {
        let dom = crate::dom::classes(agent).clone();
        let headers = agent.register_host_class(headers::class());
        let blob = agent.register_host_class(blob::blob_class());
        let file = agent.register_host_class(blob::file_class(blob));
        let file_reader = agent.register_host_class(blob::file_reader_class(dom.event_target));
        let form_data = agent.register_host_class(formdata::class());
        let request = agent.register_host_class(fetch::request_class());
        let response = agent.register_host_class(fetch::response_class());
        let progress_event = agent.register_host_class(xhr::progress_event_class(dom.event));
        let xhr_event_target =
            agent.register_host_class(xhr::xhr_event_target_class(dom.event_target));
        let xhr = agent.register_host_class(xhr::xhr_class(xhr_event_target));
        let xhr_upload = agent.register_host_class(xhr::upload_class(xhr_event_target));
        agent.set_embedder_data(NetClasses {
            headers,
            request,
            response,
            blob,
            file,
            file_reader,
            form_data,
            progress_event,
            xhr_event_target,
            xhr,
            xhr_upload,
        });
        agent.host_extend_class(dom.window, fetch::window_partial());
    }
    if agent.realm_data::<NetState>(realm).is_none() {
        agent.set_realm_data(realm, NetState::default());
    }
    let saved = agent.current_realm;
    agent.current_realm = realm;
    for (name, f) in [
        ("Headers", headers::entries as bf_js::vm::function::NativeFn),
        ("FormData", formdata::entries),
    ] {
        set_iterator(agent, realm, name, f);
    }
    blob::install_object_urls(agent, realm);
    fetch::install_send_beacon(agent, realm);
    agent.current_realm = saved;
}

/// `[Symbol.iterator]` = `entries` en el prototipo de una interfaz iterable.
fn set_iterator(agent: &mut Agent, realm: RealmId, name: &str, f: bf_js::vm::function::NativeFn) {
    let g = agent.realms[realm].global_object;
    let Ok(Value::Object(iface)) = agent.get(g, &PropertyKey::from(name), Value::Object(g)) else {
        return;
    };
    let kp = agent.keys.prototype.clone();
    let Ok(Value::Object(proto)) = agent.get(iface, &kp, Value::Object(iface)) else {
        return;
    };
    let Ok(entries) = agent.get(proto, &PropertyKey::from("entries"), Value::Object(proto)) else {
        return;
    };
    let entries = if agent.is_callable(&entries) {
        entries
    } else {
        Value::Object(agent.create_builtin_function(f, 0, "entries", false))
    };
    let k = PropertyKey::Symbol(agent.well_known.iterator);
    agent.put_data(proto, k, entries, true, false, true);
}

/// Un iterador de pares (WebIDL §3.7.9, simplificado): `kind` 0 = claves, 1 = valores,
/// 2 = pares `[k, v]`. Itera una copia de la lista.
pub fn iterator_from_pairs(
    agent: &mut Agent,
    _tag: &str,
    pairs: Vec<(String, String)>,
    kind: u8,
) -> JsResult<Value> {
    let mut vals = Vec::new();
    for (k, v) in pairs {
        vals.push(match kind {
            0 => Value::str(&k),
            1 => Value::str(&v),
            _ => Value::Object(agent.create_array_from_list(&[Value::str(&k), Value::str(&v)])?),
        });
    }
    values_iterator(agent, vals)
}

/// Un iterador sobre `vals` (el `%ArrayIteratorPrototype%` de un array nuevo).
pub fn values_iterator(agent: &mut Agent, vals: Vec<Value>) -> JsResult<Value> {
    let arr = agent.create_array_from_list(&vals)?;
    let k = PropertyKey::from("values");
    let f = agent.get(arr, &k, Value::Object(arr))?;
    agent.call(&f, Value::Object(arr), &[])
}

/// Una excepción `DOMException` con `name`.
pub fn dom_error<T>(agent: &mut Agent, name: &str, message: &str) -> JsResult<T> {
    let e = crate::dom::exception::create(agent, name, message);
    Err(bf_js::vm::Throw::Value(e))
}
