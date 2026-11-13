//! Service Workers (Service Workers, https://w3c.github.io/ServiceWorker/), primera parte:
//! registro, ciclo de vida, mensajes y `FetchEvent` para los `fetch()` de las páginas
//! controladas. **Dueño: E3.**
//!
//! # Dónde vive cada cosa
//!
//! - **El estado (Rust, global al proceso)**: el *registration map* (§2.3, por *storage key*
//!   y URL del scope), los service workers de cada registro (§2.1: estado, script, scripts
//!   importados, *skip waiting flag*) y los *service worker clients* (§2.4: las ventanas, con
//!   su id, URL y su *active service worker*). La *storage key* es `(partición, origen)`: la
//!   partición es la de `workers::set_partition` (el shell usa la 0; el runner de WPT y los
//!   tests dan una a cada documento para que sus registros no se crucen).
//! - **Los trabajos (§8.4 "Job Queue")**: una cola por (storage key, scope) que corre en su
//!   propio hilo ("in parallel"): Register (§8.6), Update (§8.7), Install (§8.8), Activate
//!   (§8.9), Try Activate, Unregister (§8.17), Clear Registration.
//! - **Los service workers**: un hilo y un agente cada uno, con el bucle de `workers::scope`
//!   y el global `ServiceWorkerGlobalScope` ([`ScopeKind::Service`]). El script (y los
//!   importados) se guardan en la versión: un service worker que se vuelve a arrancar (tras
//!   reabrir el navegador) no los vuelve a pedir.
//! - **Las interfaces** (`service.js`, una vez por realm, como IndexedDB): en las ventanas,
//!   `ServiceWorkerContainer` (`navigator.serviceWorker`), `ServiceWorkerRegistration`,
//!   `ServiceWorker` y `NavigationPreloadManager`; en el global del service worker, además,
//!   `ExtendableEvent`, `ExtendableMessageEvent`, `FetchEvent`, `Clients`, `Client` y
//!   `WindowClient`. Los objetos de un realm reflejan el estado con las tareas que manda
//!   Rust ([`Msg`]): "Update Registration State" (§8.21), "Update Worker State" (§8.22),
//!   `updatefound`, `controllerchange`, `ready`, mensajes y respuestas.
//! - **Persistencia (en el perfil, formato propio)**: con un directorio de almacenamiento
//!   (`storage::root_dir`), cada registro con *active worker* se guarda en
//!   `<raíz>/<origen>/service_workers/<hash del scope>.reg` (cabeceras de texto y los
//!   scripts como bloques de bytes con su longitud) y se carga la primera vez que un agente
//!   de esa storage key lo necesita.
//!
//! # Fetch
//!
//! `net::fetch` llama a [`handle_fetch`] antes de salir a la red: si la ventana está
//! controlada, la petición viaja al service worker, que dispara `FetchEvent`; la respuesta de
//! `respondWith` vuelve serializada (cabecera y bytes) y la página la filtra según el
//! *response tainting* (Fetch §4.3 paso 5 y §4.1 paso 14). Sin `respondWith`, la petición
//! sale a la red ("fallback"). La navegación interceptada (el documento servido por el
//! service worker) es del shell: ver `docs/pendiente/e3.md`.
//!
//! Solo en contextos seguros: las interfaces no existen en un realm que no es
//! *potentially trustworthy* y `register` exige scripts y scopes `http(s)` de confianza.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use bf_infra::Url;
use bf_js::builtins::web::serialize::{self, Serialized};
use bf_js::host::HostHandle;
use bf_js::host::agents::AgentSignal;
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::{CallArgs, NativeFn};
use bf_js::vm::{Agent, JsResult, RealmId, Throw};

use super::scope::{MimeCheck, ScopeKind, ScriptType, WorkerSpec};
use super::{AgentInbox, Envelope, WorkerControl};
use crate::WebContextHandle;
use crate::net::body::Body;
use crate::net::fetch::{RequestData, ResponseData};
use crate::net::headers::{Guard, HeaderList, HeadersData};
use crate::net::{NetBackend, NetRequest};

pub(crate) const SERVICE_JS: &str = include_str!("service.js");

/// Cuánto espera un trabajo a que el service worker termine `install` o `activate`.
const LIFECYCLE_LIMIT: Duration = Duration::from_secs(300);

// --- Tipos ---

/// `ServiceWorkerState` (§3.1.3) más "parsed" (§2.1: el estado antes de instalar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Parsed,
    Installing,
    Installed,
    Activating,
    Activated,
    Redundant,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Parsed => "parsed",
            State::Installing => "installing",
            State::Installed => "installed",
            State::Activating => "activating",
            State::Activated => "activated",
            State::Redundant => "redundant",
        }
    }
}

/// La *storage key* (Storage §2.1) con la partición del proceso.
type Key = (u64, String);

/// Lo que el hilo de un service worker sabe de sí mismo (va en [`ScopeKind::Service`]).
pub struct ServiceInfo {
    pub worker: u64,
    pub reg: u64,
    /// El script principal (no se vuelve a pedir).
    pub script: Arc<String>,
    /// El directorio de almacenamiento del registro (la Cache API del worker).
    pub root: Option<PathBuf>,
    /// Avisa al trabajo que lo arrancó si el script se evaluó sin excepción (§8.7 paso
    /// "Let runResult be the result of running the Run Service Worker algorithm").
    started: Mutex<Option<mpsc::Sender<bool>>>,
}

impl ServiceInfo {
    /// El script terminó de evaluarse (`ok`: sin excepción).
    pub(crate) fn started(&self, ok: bool) {
        if let Some(tx) = self
            .started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            let _ = tx.send(ok);
        }
    }
}

impl fmt::Debug for ServiceInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ServiceWorker({})", self.worker)
    }
}

impl PartialEq for ServiceInfo {
    fn eq(&self, other: &ServiceInfo) -> bool {
        self.worker == other.worker
    }
}

impl Eq for ServiceInfo {}

/// Un service worker (§2.1).
struct Version {
    id: u64,
    reg: u64,
    key: Key,
    script_url: Url,
    script_type: ScriptType,
    state: State,
    script: Arc<String>,
    /// El *script resource map* de los importados (URL → texto), en orden.
    imports: Vec<(String, Arc<String>)>,
    skip_waiting: bool,
    /// Su hilo, si está corriendo.
    run: Option<(Arc<AgentInbox>, Arc<WorkerControl>)>,
}

/// Un *service worker registration* (§2.2).
struct Reg {
    id: u64,
    key: Key,
    /// La URL del scope serializada sin fragmento.
    scope: String,
    update_via_cache: String,
    installing: Option<u64>,
    waiting: Option<u64>,
    active: Option<u64>,
    /// ¿Está en el *registration map*? (`unregister` lo saca; sigue vivo mientras controle
    /// clientes.)
    in_map: bool,
    backend: Option<Arc<dyn NetBackend>>,
    root: Option<PathBuf>,
    user_agent: String,
}

/// Un *service worker client* (§2.4): una ventana.
struct Client {
    id: u64,
    uuid: String,
    key: Key,
    url: String,
    inbox: Arc<AgentInbox>,
    /// Su *active service worker*.
    controller: Option<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum JobKind {
    Register,
    Update,
    Unregister,
}

/// Un trabajo (§8.4: *job*).
struct Job {
    kind: JobKind,
    key: Key,
    scope: Url,
    script_url: Option<Url>,
    script_type: ScriptType,
    update_via_cache: String,
    /// La bandeja del cliente, el id del cliente en ella y el de la promesa.
    reply: Option<(Arc<AgentInbox>, u64, u64)>,
    /// El origen del cliente (el *referrer* del trabajo).
    referrer_origin: String,
    backend: Arc<dyn NetBackend>,
    root: Option<PathBuf>,
    user_agent: String,
}

#[derive(Default)]
struct Global {
    /// Los registros en orden de creación (también los que ya no están en el mapa pero
    /// controlan clientes).
    regs: Vec<Reg>,
    versions: HashMap<u64, Version>,
    clients: Vec<Client>,
    /// Las (storage key, raíz) ya leídas del disco.
    loaded: HashSet<(Key, PathBuf)>,
    queues: HashMap<(Key, String), VecDeque<Job>>,
    running: HashSet<(Key, String)>,
}

fn global() -> MutexGuard<'static, Global> {
    static G: OnceLock<Mutex<Global>> = OnceLock::new();
    G.get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn next_id() -> u64 {
    static N: AtomicU64 = AtomicU64::new(1);
    N.fetch_add(1, Ordering::SeqCst)
}

// --- Mensajes entre agentes ---

/// El resultado de un trabajo para su promesa.
pub enum JobResult {
    /// Un `ServiceWorkerRegistration` (instantánea JSON).
    Reg(String),
    Bool(bool),
    /// Nombre (`TypeError` o el de un `DOMException`) y mensaje.
    Err(&'static str, String),
}

/// Una petición que viaja al service worker.
pub struct SwRequest {
    data: RequestData,
    headers: HeaderList,
}

/// La respuesta de `respondWith`, ya leída.
pub struct SwResponse {
    type_: String,
    url: Option<Url>,
    redirected: bool,
    status: u16,
    status_text: String,
    headers: HeaderList,
    body: Option<Vec<u8>>,
}

/// Lo que decidió el service worker para una petición.
pub enum FetchOutcome {
    /// Sin `respondWith`: a la red.
    Fallback,
    /// Error de red.
    Error(String),
    Response(Box<SwResponse>),
}

/// Un aviso para un agente (cliente o service worker). Los que no nombran un cliente van a
/// todos los realms del agente que tienen las interfaces.
pub enum Msg {
    Job {
        client: u64,
        promise: u64,
        result: JobResult,
    },
    /// "Update Registration State" (§8.21): `which` es `installing`, `waiting` o `active`.
    RegState {
        reg: u64,
        which: &'static str,
        worker: String,
    },
    /// "Update Worker State" (§8.22).
    WorkerState {
        worker: u64,
        state: State,
    },
    UpdateFound {
        reg: u64,
    },
    /// "Notify Controller Change" (§8.24).
    Controller {
        client: u64,
        worker: String,
    },
    /// La promesa `ready` del cliente (§8.9 paso 7).
    Ready {
        client: u64,
        reg: String,
    },
    /// `Client.postMessage` (§4.2.7): al `ServiceWorkerContainer` del cliente.
    ToClient {
        client: u64,
        source: String,
        origin: String,
        message: Serialized,
    },
    FetchReply {
        fetch: u64,
        outcome: FetchOutcome,
    },
    /// Al service worker: dispara `install` o `activate` y avisa cuando termina.
    Lifecycle {
        kind: &'static str,
        done: mpsc::Sender<bool>,
    },
    /// `ServiceWorker.postMessage` (§3.1.3): al global del service worker.
    ToWorker {
        client: String,
        origin: String,
        message: Serialized,
    },
    /// Al service worker: un `FetchEvent`.
    Fetch {
        fetch: u64,
        client: String,
        request: Box<SwRequest>,
        reply: Arc<AgentInbox>,
    },
}

fn send(inbox: &AgentInbox, m: Msg) {
    inbox.push(Envelope::Service(Box::new(m)));
}

// --- Instantáneas (JSON para `service.js`) ---

/// Un string JSON.
fn js(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn wsnap(g: &Global, id: Option<u64>) -> String {
    match id.and_then(|i| g.versions.get(&i)) {
        Some(v) => format!(
            "{{\"id\":{},\"scriptURL\":{},\"state\":\"{}\"}}",
            v.id,
            js(&v.script_url.serialize(false)),
            v.state.as_str()
        ),
        None => "null".into(),
    }
}

fn rsnap(g: &Global, r: &Reg) -> String {
    format!(
        "{{\"id\":{},\"scope\":{},\"updateViaCache\":{},\"installing\":{},\"waiting\":{},\"active\":{}}}",
        r.id,
        js(&r.scope),
        js(&r.update_via_cache),
        wsnap(g, r.installing),
        wsnap(g, r.waiting),
        wsnap(g, r.active)
    )
}

fn csnap(c: &Client) -> String {
    format!(
        "{{\"id\":{},\"url\":{},\"type\":\"window\",\"frameType\":\"top-level\"}}",
        js(&c.uuid),
        js(&c.url)
    )
}

// --- El estado global ---

fn reg_idx(g: &Global, id: u64) -> Option<usize> {
    g.regs.iter().position(|r| r.id == id)
}

/// "Get Registration" (§8.11): el registro del mapa con esa storage key y scope.
fn find_reg(g: &Global, key: &Key, scope: &str) -> Option<usize> {
    g.regs
        .iter()
        .position(|r| r.in_map && &r.key == key && r.scope == scope)
}

/// "Match Service Worker Registration" (§8.10): el scope más largo que es prefijo de `url`.
fn match_reg(g: &Global, key: &Key, url: &str) -> Option<usize> {
    g.regs
        .iter()
        .enumerate()
        .filter(|(_, r)| r.in_map && &r.key == key && url.starts_with(r.scope.as_str()))
        .max_by_key(|(_, r)| r.scope.len())
        .map(|(i, _)| i)
}

/// "Get Newest Worker" (§8.12).
fn newest(r: &Reg) -> Option<u64> {
    r.installing.or(r.waiting).or(r.active)
}

/// Las bandejas de los agentes de `key` (clientes y service workers que corren), sin
/// repetir.
fn inboxes(g: &Global, key: &Key) -> Vec<Arc<AgentInbox>> {
    let mut out: Vec<Arc<AgentInbox>> = Vec::new();
    let clients = g.clients.iter().filter(|c| &c.key == key).map(|c| &c.inbox);
    let workers = g
        .versions
        .values()
        .filter(|v| &v.key == key)
        .filter_map(|v| v.run.as_ref().map(|r| &r.0));
    for i in clients.chain(workers) {
        if !out.iter().any(|o| Arc::ptr_eq(o, i)) {
            out.push(i.clone());
        }
    }
    out
}

fn broadcast(g: &Global, key: &Key, make: impl Fn() -> Msg) {
    for i in inboxes(g, key) {
        send(&i, make());
    }
}

/// "Update Worker State" (§8.22); a `redundant` también lo termina (§8.14 "Terminate
/// Service Worker").
fn set_state(g: &mut Global, id: u64, state: State) {
    let Some(v) = g.versions.get_mut(&id) else {
        return;
    };
    v.state = state;
    if state == State::Redundant
        && let Some((inbox, control)) = v.run.take()
    {
        control.terminated.store(true, Ordering::SeqCst);
        control.interrupt.store(true, Ordering::SeqCst);
        inbox.signal().signal();
    }
    let key = v.key.clone();
    broadcast(g, &key, || Msg::WorkerState { worker: id, state });
}

/// "Update Registration State" (§8.21).
fn set_reg(g: &mut Global, i: usize, which: &'static str, worker: Option<u64>) {
    let r = &mut g.regs[i];
    match which {
        "installing" => r.installing = worker,
        "waiting" => r.waiting = worker,
        _ => r.active = worker,
    }
    let (id, key) = (r.id, r.key.clone());
    let w = wsnap(g, worker);
    broadcast(g, &key, || Msg::RegState {
        reg: id,
        which,
        worker: w.clone(),
    });
}

/// ¿Algún cliente usa el registro? (§2.4 "using".)
fn in_use(g: &Global, reg: u64) -> bool {
    g.clients.iter().any(|c| {
        c.controller
            .and_then(|w| g.versions.get(&w))
            .is_some_and(|v| v.reg == reg)
    })
}

/// Saca los registros que ya no están en el mapa, no tienen workers y nadie usa.
fn prune(g: &mut Global) {
    let dead: Vec<u64> = g
        .regs
        .iter()
        .filter(|r| {
            !r.in_map && r.installing.is_none() && r.waiting.is_none() && r.active.is_none()
        })
        .map(|r| r.id)
        .filter(|id| !in_use(g, *id))
        .collect();
    g.regs.retain(|r| !dead.contains(&r.id));
}

// --- Persistencia ---

fn sw_dir(root: &Path, origin: &str) -> PathBuf {
    root.join(crate::storage::backend::origin_dir_name(origin))
        .join("service_workers")
}

/// FNV-1a de 64 bits: el nombre de archivo de un scope.
fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn reg_file(root: &Path, origin: &str, scope: &str) -> PathBuf {
    sw_dir(root, origin).join(format!("{:016x}.reg", fnv(scope)))
}

/// Guarda el registro `i` con su *active worker* (escritura atómica: temporal y rename).
fn persist(g: &Global, i: usize) {
    let r = &g.regs[i];
    let (Some(root), Some(a)) = (&r.root, r.active) else {
        return;
    };
    let Some(v) = g.versions.get(&a) else {
        return;
    };
    let mut out = Vec::new();
    out.extend_from_slice(b"FREEDOM-SW 1\n");
    out.extend_from_slice(format!("scope {}\n", r.scope).as_bytes());
    out.extend_from_slice(format!("updateViaCache {}\n", r.update_via_cache).as_bytes());
    let t = match v.script_type {
        ScriptType::Classic => "classic",
        ScriptType::Module => "module",
    };
    out.extend_from_slice(format!("type {t}\n").as_bytes());
    let mut resource = |tag: &str, url: &str, text: &str| {
        out.extend_from_slice(format!("{tag} {url} {}\n", text.len()).as_bytes());
        out.extend_from_slice(text.as_bytes());
        out.push(b'\n');
    };
    resource("script", &v.script_url.serialize(false), &v.script);
    for (u, t) in &v.imports {
        resource("import", u, t);
    }
    let path = reg_file(root, &r.key.1, &r.scope);
    let write = || -> std::io::Result<()> {
        use std::io::Write;
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension("reg.tmp");
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&out)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &path)
    };
    let _ = write();
}

fn unpersist(r: &Reg) {
    if let Some(root) = &r.root {
        let _ = std::fs::remove_file(reg_file(root, &r.key.1, &r.scope));
    }
}

/// Un registro guardado: scope, updateViaCache, tipo, script y los importados.
type Saved = (
    String,
    String,
    ScriptType,
    Url,
    String,
    Vec<(String, String)>,
);

fn parse_saved(bytes: &[u8]) -> Option<Saved> {
    let mut pos = 0;
    let line = |pos: &mut usize| -> Option<String> {
        let end = bytes[*pos..].iter().position(|b| *b == b'\n')? + *pos;
        let s = String::from_utf8(bytes[*pos..end].to_vec()).ok()?;
        *pos = end + 1;
        Some(s)
    };
    if line(&mut pos)? != "FREEDOM-SW 1" {
        return None;
    }
    let scope = line(&mut pos)?.strip_prefix("scope ")?.to_owned();
    let uvc = line(&mut pos)?.strip_prefix("updateViaCache ")?.to_owned();
    let t = match line(&mut pos)?.strip_prefix("type ")? {
        "module" => ScriptType::Module,
        _ => ScriptType::Classic,
    };
    let mut script = None;
    let mut imports = Vec::new();
    while pos < bytes.len() {
        let head = line(&mut pos)?;
        let mut parts = head.splitn(3, ' ');
        let (tag, url, len) = (parts.next()?, parts.next()?, parts.next()?);
        let len: usize = len.parse().ok()?;
        let text = String::from_utf8(bytes.get(pos..pos + len)?.to_vec()).ok()?;
        pos += len + 1;
        match tag {
            "script" => script = Some((Url::parse(url, None).ok()?, text)),
            _ => imports.push((url.to_owned(), text)),
        }
    }
    let (url, text) = script?;
    Some((scope, uvc, t, url, text, imports))
}

/// Lee del disco (una vez) los registros de `key` guardados en `root`.
fn load(g: &mut Global, key: &Key, root: &Path, backend: Option<&Arc<dyn NetBackend>>, ua: &str) {
    if !g.loaded.insert((key.clone(), root.to_path_buf())) {
        return;
    }
    let Ok(rd) = std::fs::read_dir(sw_dir(root, &key.1)) else {
        return;
    };
    let mut files: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "reg"))
        .collect();
    files.sort();
    for p in files {
        let Some((scope, uvc, t, url, text, imports)) =
            std::fs::read(&p).ok().and_then(|b| parse_saved(&b))
        else {
            continue;
        };
        if find_reg(g, key, &scope).is_some() {
            continue;
        }
        let (rid, wid) = (next_id(), next_id());
        g.versions.insert(
            wid,
            Version {
                id: wid,
                reg: rid,
                key: key.clone(),
                script_url: url,
                script_type: t,
                state: State::Activated,
                script: Arc::new(text),
                imports: imports.into_iter().map(|(u, t)| (u, Arc::new(t))).collect(),
                skip_waiting: false,
                run: None,
            },
        );
        g.regs.push(Reg {
            id: rid,
            key: key.clone(),
            scope,
            update_via_cache: uvc,
            installing: None,
            waiting: None,
            active: Some(wid),
            in_map: true,
            backend: backend.cloned(),
            root: Some(root.to_path_buf()),
            user_agent: ua.to_owned(),
        });
    }
}

// --- Los hilos de los service workers ---

/// "Run Service Worker" (§8.13): arranca el hilo de `wid` si no corre. `started` recibe si
/// el script se evaluó sin excepción. `backend` y `ua` sirven si el registro no tiene (uno
/// leído del disco).
fn start(
    wid: u64,
    started: Option<mpsc::Sender<bool>>,
    backend: Option<&Arc<dyn NetBackend>>,
    ua: &str,
) -> Option<Arc<AgentInbox>> {
    let spec = {
        let mut g = global();
        let g = &mut *g;
        let v = g.versions.get_mut(&wid)?;
        if v.state == State::Redundant {
            return None;
        }
        if let Some((inbox, control)) = &v.run
            && !control.closed.load(Ordering::SeqCst)
            && !control.terminated.load(Ordering::SeqCst)
        {
            if let Some(tx) = started {
                let _ = tx.send(true);
            }
            return Some(inbox.clone());
        }
        let r = g.regs.iter_mut().find(|r| r.id == v.reg)?;
        if r.backend.is_none() {
            r.backend = backend.cloned();
        }
        let backend = r.backend.clone()?;
        let user_agent = if r.user_agent.is_empty() {
            ua.to_owned()
        } else {
            r.user_agent.clone()
        };
        let inbox = Arc::new(AgentInbox::new(Arc::new(AgentSignal::default())));
        let control = Arc::new(WorkerControl::default());
        v.run = Some((inbox.clone(), control.clone()));
        let info = Arc::new(ServiceInfo {
            worker: wid,
            reg: v.reg,
            script: v.script.clone(),
            root: r.root.clone(),
            started: Mutex::new(started),
        });
        WorkerSpec {
            url: v.script_url.clone(),
            script_type: v.script_type,
            name: String::new(),
            kind: ScopeKind::Service(info),
            backend,
            parent: None,
            worker_id: 0,
            inbox,
            control,
            cross_origin_isolated: false,
            owner_coep: Default::default(),
            user_agent,
            partition: v.key.0,
        }
    };
    let inbox = spec.inbox.clone();
    super::scope::spawn(spec).ok()?;
    Some(inbox)
}

/// Dispara `install` o `activate` en el service worker y espera a que se asienten las
/// promesas de `waitUntil` (§8.8 y §8.9). `false`: falló (o el worker terminó).
fn run_lifecycle(wid: u64, kind: &'static str) -> bool {
    let Some(inbox) = start(wid, None, None, "") else {
        return false;
    };
    let control = {
        let g = global();
        match g.versions.get(&wid).and_then(|v| v.run.as_ref()) {
            Some((_, c)) => c.clone(),
            None => return false,
        }
    };
    let (tx, rx) = mpsc::channel();
    send(&inbox, Msg::Lifecycle { kind, done: tx });
    let deadline = Instant::now() + LIFECYCLE_LIMIT;
    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(ok) => return ok,
            Err(mpsc::RecvTimeoutError::Disconnected) => return false,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if control.closed.load(Ordering::SeqCst)
                    || control.terminated.load(Ordering::SeqCst)
                    || Instant::now() > deadline
                {
                    return false;
                }
            }
        }
    }
}

/// El hilo de un service worker terminó: lo que quedaba en su bandeja se descarta (y sus
/// `Lifecycle` sueltan el canal: el trabajo que espera se entera).
pub(crate) fn worker_exited(inbox: &AgentInbox) {
    drop(inbox.drain());
}

// --- Los trabajos ---

fn settle(job: &Job, result: JobResult) {
    if let Some((inbox, client, promise)) = &job.reply {
        send(
            inbox,
            Msg::Job {
                client: *client,
                promise: *promise,
                result,
            },
        );
    }
}

fn reject(job: &Job, name: &'static str, msg: &str) {
    settle(job, JobResult::Err(name, msg.to_owned()));
}

/// "Schedule Job" (§8.4.2).
fn schedule(job: Job) {
    let qk = (job.key.clone(), job.scope.serialize(true));
    let mut g = global();
    g.queues.entry(qk.clone()).or_default().push_back(job);
    if g.running.insert(qk.clone()) {
        drop(g);
        let spawned = std::thread::Builder::new()
            .name("service worker jobs".into())
            .spawn({
                let qk = qk.clone();
                move || run_queue(qk)
            });
        if spawned.is_err() {
            let mut g = global();
            g.running.remove(&qk);
            if let Some(q) = g.queues.remove(&qk) {
                drop(g);
                for j in q {
                    reject(&j, "TypeError", "no se pudo crear el hilo de trabajos");
                }
            }
        }
    }
}

/// "Run Job" (§8.4.3) hasta vaciar la cola.
fn run_queue(qk: (Key, String)) {
    loop {
        let job = {
            let mut g = global();
            match g.queues.get_mut(&qk).and_then(VecDeque::pop_front) {
                Some(j) => j,
                None => {
                    g.queues.remove(&qk);
                    g.running.remove(&qk);
                    return;
                }
            }
        };
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match job.kind {
            JobKind::Register => register_job(&job),
            JobKind::Update => update_job(&job),
            JobKind::Unregister => unregister_job(&job),
        }));
        if r.is_err() {
            reject(&job, "TypeError", "error interno del service worker");
        }
    }
}

/// "Register" (§8.6).
fn register_job(job: &Job) {
    let Some(script) = &job.script_url else {
        return reject(job, "TypeError", "falta la URL del script");
    };
    if !super::isolation::is_potentially_trustworthy(script) {
        return reject(job, "SecurityError", "The script URL is not trustworthy.");
    }
    if script.origin().to_string() != job.referrer_origin {
        return reject(
            job,
            "SecurityError",
            "The origin of the provided scriptURL does not match the current origin.",
        );
    }
    if job.scope.origin().to_string() != job.referrer_origin {
        return reject(
            job,
            "SecurityError",
            "The origin of the provided scope does not match the current origin.",
        );
    }
    let scope = job.scope.serialize(true);
    {
        let mut g = global();
        if let Some(root) = &job.root {
            load(&mut g, &job.key, root, Some(&job.backend), &job.user_agent);
        }
        match find_reg(&g, &job.key, &scope) {
            Some(i) => {
                let r = &g.regs[i];
                if let Some(v) = newest(r).and_then(|n| g.versions.get(&n))
                    && v.script_url.serialize(false) == script.serialize(false)
                    && v.script_type == job.script_type
                    && r.update_via_cache == job.update_via_cache
                {
                    let s = rsnap(&g, r);
                    drop(g);
                    return settle(job, JobResult::Reg(s));
                }
                g.regs[i].update_via_cache = job.update_via_cache.clone();
            }
            None => {
                // "Set Registration" (§8.18).
                g.regs.push(Reg {
                    id: next_id(),
                    key: job.key.clone(),
                    scope,
                    update_via_cache: job.update_via_cache.clone(),
                    installing: None,
                    waiting: None,
                    active: None,
                    in_map: true,
                    backend: Some(job.backend.clone()),
                    root: job.root.clone(),
                    user_agent: job.user_agent.clone(),
                });
            }
        }
    }
    update_job(job);
}

/// Si el registro no tiene ningún worker, sale del mapa (§8.7: "If newestWorker is null,
/// then remove registration map[...]").
fn remove_if_empty(reg: u64) {
    let mut g = global();
    if let Some(i) = reg_idx(&g, reg)
        && newest(&g.regs[i]).is_none()
    {
        g.regs[i].in_map = false;
        prune(&mut g);
    }
}

/// Trae el script de un service worker (§8.7 paso 8: "Fetch a classic worker script" con
/// los chequeos de *perform the fetch hook*): `Err((nombre, mensaje))`.
fn fetch_sw_script(
    backend: &dyn NetBackend,
    url: &Url,
    scope: &Url,
) -> Result<String, (&'static str, String)> {
    let mut options = bf_net::RequestOptions::default();
    options.headers.push("Service-Worker", "script");
    options.redirect = bf_net::RedirectMode::Error;
    let r = backend
        .fetch(&NetRequest {
            url: url.clone(),
            options,
        })
        .map_err(|e| ("TypeError", format!("Failed to fetch the script: {e}")))?;
    if r.url_list.len() > 1 {
        return Err((
            "TypeError",
            "The script resource is behind a redirect.".into(),
        ));
    }
    if !(200..300).contains(&r.status) {
        return Err((
            "TypeError",
            format!("A bad HTTP response code ({}) was received.", r.status),
        ));
    }
    let ct = r.headers.get("content-type").unwrap_or_default();
    if !super::scope::is_javascript_mime(&ct) {
        return Err((
            "SecurityError",
            format!("The script has an unsupported MIME type ('{ct}')."),
        ));
    }
    // El scope máximo (§8.7 paso 8, *perform the fetch* pasos 9–19).
    let max_scope = match r.headers.get("service-worker-allowed") {
        None => Url::parse("./", Some(url)).ok().map(|u| u.path_string()),
        Some(v) => Url::parse(v.trim(), Some(url))
            .ok()
            .filter(|m| m.origin() == url.origin())
            .map(|m| m.path_string()),
    };
    let scope_path = scope.path_string();
    if !max_scope.is_some_and(|m| scope_path.starts_with(&m)) {
        return Err((
            "SecurityError",
            "The path of the provided scope is not under the max scope allowed.".into(),
        ));
    }
    Ok(String::from_utf8_lossy(&r.body).into_owned())
}

/// ¿Los scripts importados siguen iguales? (§8.7 paso 12, *byte-for-byte*.)
fn imports_unchanged(backend: &dyn NetBackend, imports: &[(String, Arc<String>)]) -> bool {
    imports.iter().all(|(u, old)| {
        Url::parse(u, None)
            .ok()
            .and_then(|url| super::scope::fetch_script(backend, &url, MimeCheck::Imported).ok())
            .is_some_and(|t| t == **old)
    })
}

/// "Update" (§8.7).
fn update_job(job: &Job) {
    let scope = job.scope.serialize(true);
    let (reg, newest_info) = {
        let g = global();
        let Some(i) = find_reg(&g, &job.key, &scope) else {
            drop(g);
            return reject(job, "TypeError", "The registration was not found.");
        };
        let r = &g.regs[i];
        let n = newest(r).and_then(|n| g.versions.get(&n)).map(|v| {
            (
                v.script_url.serialize(false),
                v.script.clone(),
                v.imports.clone(),
            )
        });
        (r.id, n)
    };
    let Some(script_url) = job.script_url.clone() else {
        return reject(job, "TypeError", "falta la URL del script");
    };
    if job.kind == JobKind::Update
        && let Some((u, _, _)) = &newest_info
        && *u != script_url.serialize(false)
    {
        return reject(job, "TypeError", "The script URL changed.");
    }
    let text = match fetch_sw_script(job.backend.as_ref(), &script_url, &job.scope) {
        Ok(t) => t,
        Err((name, msg)) => {
            reject(job, name, &msg);
            return remove_if_empty(reg);
        }
    };
    if let Some((_, old, imports)) = &newest_info
        && **old == text
        && imports_unchanged(job.backend.as_ref(), imports)
    {
        let g = global();
        if let Some(i) = reg_idx(&g, reg) {
            let s = rsnap(&g, &g.regs[i]);
            drop(g);
            settle(job, JobResult::Reg(s));
        }
        return;
    }
    let wid = next_id();
    global().versions.insert(
        wid,
        Version {
            id: wid,
            reg,
            key: job.key.clone(),
            script_url,
            script_type: job.script_type,
            state: State::Parsed,
            script: Arc::new(text),
            imports: Vec::new(),
            skip_waiting: false,
            run: None,
        },
    );
    let (tx, rx) = mpsc::channel();
    let ok = start(wid, Some(tx), Some(&job.backend), &job.user_agent).is_some()
        && matches!(rx.recv_timeout(LIFECYCLE_LIMIT), Ok(true));
    if !ok {
        {
            let mut g = global();
            if let Some(v) = g.versions.get_mut(&wid)
                && let Some((inbox, control)) = v.run.take()
            {
                control.terminated.store(true, Ordering::SeqCst);
                control.interrupt.store(true, Ordering::SeqCst);
                inbox.signal().signal();
            }
            g.versions.remove(&wid);
        }
        reject(job, "TypeError", "ServiceWorker script evaluation failed");
        return remove_if_empty(reg);
    }
    install(job, wid, reg);
}

/// "Install" (§8.8).
fn install(job: &Job, wid: u64, reg: u64) {
    {
        let mut g = global();
        let Some(i) = reg_idx(&g, reg) else {
            return;
        };
        set_reg(&mut g, i, "installing", Some(wid));
        set_state(&mut g, wid, State::Installing);
        let s = rsnap(&g, &g.regs[i]);
        settle(job, JobResult::Reg(s));
        let key = g.regs[i].key.clone();
        broadcast(&g, &key, || Msg::UpdateFound { reg });
    }
    let ok = run_lifecycle(wid, "install");
    let mut g = global();
    let Some(i) = reg_idx(&g, reg) else {
        return;
    };
    let still = g.regs[i].installing == Some(wid)
        && g.versions
            .get(&wid)
            .is_some_and(|v| v.state == State::Installing);
    if !ok || !still {
        if g.versions
            .get(&wid)
            .is_some_and(|v| v.state != State::Redundant)
        {
            set_state(&mut g, wid, State::Redundant);
        }
        if g.regs[i].installing == Some(wid) {
            set_reg(&mut g, i, "installing", None);
        }
        if newest(&g.regs[i]).is_none() {
            g.regs[i].in_map = false;
            prune(&mut g);
        }
        return;
    }
    if let Some(old) = g.regs[i].waiting {
        set_state(&mut g, old, State::Redundant);
    }
    set_reg(&mut g, i, "waiting", Some(wid));
    set_reg(&mut g, i, "installing", None);
    set_state(&mut g, wid, State::Installed);
    drop(g);
    try_activate(reg);
}

/// "Try Activate" (§8.10).
fn try_activate(reg: u64) {
    let go = {
        let g = global();
        let Some(i) = reg_idx(&g, reg) else {
            return;
        };
        let r = &g.regs[i];
        let Some(w) = r.waiting else {
            return;
        };
        if r.active
            .and_then(|a| g.versions.get(&a))
            .is_some_and(|v| v.state == State::Activating)
        {
            return;
        }
        r.active.is_none() || !in_use(&g, reg) || g.versions.get(&w).is_some_and(|v| v.skip_waiting)
    };
    if go {
        activate(reg);
    }
}

/// "Activate" (§8.9).
fn activate(reg: u64) {
    let wid = {
        let mut g = global();
        let Some(i) = reg_idx(&g, reg) else {
            return;
        };
        let Some(w) = g.regs[i].waiting else {
            return;
        };
        if let Some(a) = g.regs[i].active {
            set_state(&mut g, a, State::Redundant);
        }
        set_reg(&mut g, i, "active", Some(w));
        set_reg(&mut g, i, "waiting", None);
        set_state(&mut g, w, State::Activating);
        let key = g.regs[i].key.clone();
        let rs = rsnap(&g, &g.regs[i]);
        let ws = wsnap(&g, Some(w));
        // Paso 7: la promesa `ready` de los clientes cuyo registro es este.
        for c in g.clients.iter().filter(|c| c.key == key) {
            if match_reg(&g, &key, &c.url) == Some(i) {
                send(
                    &c.inbox,
                    Msg::Ready {
                        client: c.id,
                        reg: rs.clone(),
                    },
                );
            }
        }
        // Paso 8: los clientes que usan el registro pasan al nuevo worker.
        let Global {
            clients, versions, ..
        } = &mut *g;
        for c in clients.iter_mut() {
            let uses = c
                .controller
                .and_then(|x| versions.get(&x))
                .is_some_and(|v| v.reg == reg);
            if uses && c.controller != Some(w) {
                c.controller = Some(w);
                send(
                    &c.inbox,
                    Msg::Controller {
                        client: c.id,
                        worker: ws.clone(),
                    },
                );
            }
        }
        persist(&g, i);
        w
    };
    let _ = run_lifecycle(wid, "activate");
    let mut g = global();
    if g.versions
        .get(&wid)
        .is_some_and(|v| v.state == State::Activating)
    {
        set_state(&mut g, wid, State::Activated);
    }
}

/// "Clear Registration" (§8.19).
fn clear(reg: u64) {
    let mut g = global();
    let Some(i) = reg_idx(&g, reg) else {
        return;
    };
    for which in ["installing", "waiting", "active"] {
        let w = match which {
            "installing" => g.regs[i].installing,
            "waiting" => g.regs[i].waiting,
            _ => g.regs[i].active,
        };
        if let Some(w) = w {
            set_state(&mut g, w, State::Redundant);
            set_reg(&mut g, i, which, None);
        }
    }
    prune(&mut g);
}

/// "Try Clear Registration" (§8.20).
fn try_clear(reg: u64) {
    let used = in_use(&global(), reg);
    if !used {
        clear(reg);
    }
}

/// "Unregister" (§8.17).
fn unregister_job(job: &Job) {
    if job.scope.origin().to_string() != job.referrer_origin {
        return reject(
            job,
            "SecurityError",
            "The origin of the scope does not match the current origin.",
        );
    }
    let scope = job.scope.serialize(true);
    let reg = {
        let mut g = global();
        match find_reg(&g, &job.key, &scope) {
            None => None,
            Some(i) => {
                g.regs[i].in_map = false;
                unpersist(&g.regs[i]);
                Some(g.regs[i].id)
            }
        }
    };
    settle(job, JobResult::Bool(reg.is_some()));
    if let Some(r) = reg {
        try_clear(r);
    }
}

/// "Handle Service Worker Client Unload" (§8.16) de los clientes de un agente que se va.
pub(crate) fn agent_gone(inbox: &Arc<AgentInbox>) {
    let affected: Vec<u64> = {
        let mut g = global();
        let g = &mut *g;
        let mut regs = Vec::new();
        let versions = &g.versions;
        g.clients.retain(|c| {
            if !Arc::ptr_eq(&c.inbox, inbox) {
                return true;
            }
            if let Some(v) = c.controller.and_then(|w| versions.get(&w))
                && !regs.contains(&v.reg)
            {
                regs.push(v.reg);
            }
            false
        });
        regs
    };
    if affected.is_empty() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("service worker unload".into())
        .spawn(move || {
            for r in affected {
                let in_map = {
                    let g = global();
                    reg_idx(&g, r).is_some_and(|i| g.regs[i].in_map)
                };
                if in_map {
                    try_activate(r);
                } else {
                    try_clear(r);
                }
            }
        });
}

// --- El lado del agente ---

/// El estado de service workers de un agente: sus clientes (id → realm) y, si es un
/// service worker, quién es.
#[derive(Default)]
struct Local {
    realms: Vec<(u64, RealmId)>,
    /// Si el agente es un service worker: (worker, registro).
    sw: Option<(u64, u64)>,
    /// ¿Alguna ventana usó `navigator.serviceWorker`? (El sondeo sigue mientras tanto.)
    used: bool,
}

fn local(agent: &mut Agent) -> &mut Local {
    if agent.embedder_data::<Local>().is_none() {
        agent.set_embedder_data(Local::default());
    }
    agent.embedder_data_mut::<Local>().expect("Local")
}

fn client_of(agent: &Agent, realm: RealmId) -> Option<u64> {
    agent
        .embedder_data::<Local>()?
        .realms
        .iter()
        .find(|(_, r)| *r == realm)
        .map(|(c, _)| *c)
}

fn realm_of(agent: &Agent, client: u64) -> Option<RealmId> {
    agent
        .embedder_data::<Local>()?
        .realms
        .iter()
        .find(|(c, _)| *c == client)
        .map(|(_, r)| *r)
}

fn all_realms(agent: &Agent) -> Vec<RealmId> {
    agent
        .embedder_data::<Local>()
        .map(|l| l.realms.iter().map(|(_, r)| *r).collect())
        .unwrap_or_default()
}

/// Un UUID versión 4 (para `Client.id`).
fn uuid() -> String {
    use std::hash::{BuildHasher, Hasher};
    let s = std::collections::hash_map::RandomState::new();
    let mut h = s.build_hasher();
    h.write_u64(next_id());
    let a = h.finish();
    let mut h = s.build_hasher();
    h.write_u64(a ^ 0x9e37_79b9_7f4a_7c15);
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    let b = h.finish();
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&a.to_le_bytes());
    bytes[8..].copy_from_slice(&b.to_le_bytes());
    bytes[6] = (bytes[6] & 0x0F) | 0x40;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    let hex: String = bytes.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn is_secure(agent: &Agent, realm: RealmId) -> bool {
    agent
        .realm_data::<crate::WebContext>(realm)
        .is_some_and(|c| super::isolation::is_potentially_trustworthy(&c.url))
}

/// Una ventana nueva (la llama `workers::register`): se apunta como *service worker client*
/// (§2.4) y, si es contexto seguro, recibe las interfaces. Su *active service worker* es el
/// del registro que coincide con su URL ("Match Service Worker Registration" al crear el
/// entorno, HTML §7.5.1 "create and initialize a Document object" → SW §8.10).
pub(crate) fn window_created(agent: &mut Agent, realm: RealmId) {
    if !is_secure(agent, realm) {
        return;
    }
    let url = WebContextHandle { realm }.get(agent).url.clone();
    if !matches!(url.scheme.as_str(), "http" | "https") {
        // `file:` y los demás esquemas seguros tienen las interfaces pero no registros.
        install_js(agent, realm, "window");
        return;
    }
    let origin = url.origin().to_string();
    let key = (super::partition(agent), origin);
    let root = crate::storage::root_dir(agent, realm);
    let ua = agent.host.hooks.user_agent();
    let inbox = super::inbox(agent);
    let id = next_id();
    let url_s = url.serialize(false);
    {
        let mut g = global();
        if let Some(root) = &root {
            load(&mut g, &key, root, None, &ua);
        }
        let controller = match_reg(&g, &key, &url_s).and_then(|i| g.regs[i].active);
        g.clients.push(Client {
            id,
            uuid: uuid(),
            key,
            url: url_s,
            inbox,
            controller,
        });
    }
    local(agent).realms.push((id, realm));
    install_js(agent, realm, "window");
}

/// `workers::set_partition`: los clientes del agente cambian de storage key (y de
/// registro que los controla: el agente acaba de nacer).
pub(crate) fn repartition(agent: &mut Agent, p: u64) {
    let ids: Vec<u64> = all_clients(agent);
    if ids.is_empty() {
        return;
    }
    // Los registros guardados en el perfil de cada ventana, con su nueva storage key.
    let realms = all_realms(agent);
    let ua = agent.host.hooks.user_agent();
    let mut roots = Vec::new();
    for realm in realms {
        if let Some(root) = crate::storage::root_dir(agent, realm) {
            let saved = agent.current_realm;
            agent.current_realm = realm;
            let origin = crate::net::document_origin(agent);
            let backend = crate::net::current_backend(agent);
            agent.current_realm = saved;
            roots.push(((p, origin), root, backend));
        }
    }
    let mut g = global();
    let g = &mut *g;
    for (key, root, backend) in &roots {
        load(g, key, root, Some(backend), &ua);
    }
    for c in g.clients.iter_mut().filter(|c| ids.contains(&c.id)) {
        c.key.0 = p;
    }
    let matches: Vec<(u64, Option<u64>)> = g
        .clients
        .iter()
        .filter(|c| ids.contains(&c.id))
        .map(|c| {
            (
                c.id,
                match_reg(g, &c.key, &c.url).and_then(|i| g.regs[i].active),
            )
        })
        .collect();
    for (id, ctrl) in matches {
        if let Some(c) = g.clients.iter_mut().find(|c| c.id == id) {
            c.controller = ctrl;
        }
    }
}

fn all_clients(agent: &Agent) -> Vec<u64> {
    agent
        .embedder_data::<Local>()
        .map(|l| {
            l.realms
                .iter()
                .map(|(c, _)| *c)
                .filter(|c| *c != 0)
                .collect()
        })
        .unwrap_or_default()
}

/// ¿Hay trabajo de service workers pendiente para este agente? (Para el runner de WPT: espera
/// en tiempo real mientras tanto.)
pub(crate) fn busy(agent: &Agent) -> bool {
    let Some(l) = agent.embedder_data::<Local>() else {
        return false;
    };
    if l.sw.is_some() || !l.used {
        return false;
    }
    if CLIENT_FETCH.with(|f| !f.borrow().is_empty()) {
        return true;
    }
    let ids: Vec<u64> = l.realms.iter().map(|(c, _)| *c).collect();
    let g = global();
    let keys: Vec<Key> = g
        .clients
        .iter()
        .filter(|c| ids.contains(&c.id))
        .map(|c| c.key.clone())
        .collect();
    g.running.iter().any(|(k, _)| keys.contains(k))
        || g.versions.values().any(|v| {
            keys.contains(&v.key)
                && (matches!(
                    v.state,
                    State::Parsed | State::Installing | State::Activating
                ) || v
                    .run
                    .as_ref()
                    .is_some_and(|(i, c)| !c.closed.load(Ordering::SeqCst) && !i.is_idle()))
        })
}

/// ¿Debe seguir el sondeo de la bandeja? (Una ventana usa service workers.)
pub(crate) fn keeps_polling(agent: &Agent) -> bool {
    agent.embedder_data::<Local>().is_some_and(|l| l.used)
}

// --- Fetch (§4.6 "Handle Fetch") ---

/// Lo que `net::fetch` hace con la decisión del service worker: `None` es "a la red";
/// `Some(Ok(..))`, la respuesta ya filtrada; `Some(Err(..))`, un error de red.
pub type FetchDone =
    Box<dyn FnOnce(&mut Agent, Option<Result<(ResponseData, HeaderList), String>>)>;

struct PendingFetch {
    done: FetchDone,
    realm: RealmId,
    url: Url,
    mode: String,
    redirect: String,
    /// El *response tainting* de la petición: `basic`, `cors` u `opaque`.
    tainting: &'static str,
}

thread_local! {
    /// Los `fetch()` de este agente que esperan a su service worker.
    static CLIENT_FETCH: RefCell<HashMap<u64, PendingFetch>> = RefCell::new(HashMap::new());
    /// Las peticiones que este service worker está atendiendo: id → bandeja del cliente.
    static SW_FETCH: RefCell<HashMap<u64, Arc<AgentInbox>>> = RefCell::new(HashMap::new());
    /// Los `install`/`activate` en curso en este service worker.
    static LIFECYCLE: RefCell<HashMap<u64, mpsc::Sender<bool>>> = RefCell::new(HashMap::new());
}

/// Si la ventana en curso está controlada, manda la petición a su service worker (que
/// decide; `done` corre cuando responde) y devuelve `Ok`; si no, devuelve `done`.
pub fn handle_fetch(
    agent: &mut Agent,
    r: &RequestData,
    list: &HeaderList,
    done: FetchDone,
) -> Result<(), FetchDone> {
    if !matches!(r.url.scheme.as_str(), "http" | "https") {
        return Err(done);
    }
    let realm = agent.current_realm;
    let Some(cid) = client_of(agent, realm) else {
        return Err(done);
    };
    let found = {
        let g = global();
        g.clients
            .iter()
            .find(|c| c.id == cid)
            .and_then(|c| c.controller.map(|w| (w, c.uuid.clone(), c.url.clone())))
    };
    let Some((wid, uuid, client_url)) = found else {
        return Err(done);
    };
    let backend = crate::net::current_backend(agent);
    let ua = agent.host.hooks.user_agent();
    let Some(inbox) = start(wid, None, Some(&backend), &ua) else {
        return Err(done);
    };
    let same_origin = crate::net::same_origin(agent, &r.url);
    let tainting = if same_origin {
        "basic"
    } else if r.mode == "no-cors" {
        "opaque"
    } else {
        "cors"
    };
    let mut data = r.clone();
    if data.referrer == "about:client" {
        data.referrer = client_url;
    }
    let fid = next_id();
    CLIENT_FETCH.with(|f| {
        f.borrow_mut().insert(
            fid,
            PendingFetch {
                done,
                realm,
                url: r.url.clone(),
                mode: r.mode.clone(),
                redirect: r.redirect.clone(),
                tainting,
            },
        );
    });
    send(
        &inbox,
        Msg::Fetch {
            fetch: fid,
            client: uuid,
            request: Box::new(SwRequest {
                data,
                headers: list.clone(),
            }),
            reply: super::inbox(agent),
        },
    );
    local(agent).used = true;
    super::ensure_polling(agent, realm);
    Ok(())
}

/// La respuesta del service worker para la página (Fetch §4.3 paso 5.4 y main fetch paso
/// 14: el filtro según el tainting si no viene filtrada).
fn client_response(p: &PendingFetch, sr: SwResponse) -> Result<(ResponseData, HeaderList), String> {
    let bad = match sr.type_.as_str() {
        "error" => true,
        "cors" => p.mode == "same-origin",
        "opaque" => p.mode != "no-cors",
        "opaqueredirect" => p.redirect != "manual",
        _ => false,
    } || (p.redirect != "follow" && sr.redirected);
    if bad {
        return Err("the service worker responded with an unusable response".into());
    }
    let url = sr.url.or_else(|| Some(p.url.clone()));
    let null_body = matches!(sr.status, 101 | 103 | 204 | 205 | 304);
    let body = if null_body { None } else { sr.body };
    let make = |type_: &str, url, status, status_text, body| ResponseData {
        type_: type_.into(),
        url,
        redirected: sr.redirected,
        status,
        status_text,
        body: Body::new(body),
    };
    Ok(match sr.type_.as_str() {
        "default" => match p.tainting {
            "basic" => (
                make("basic", url, sr.status, sr.status_text, body),
                crate::net::basic_filter(&sr.headers),
            ),
            "cors" => (
                make("cors", url, sr.status, sr.status_text, body),
                crate::net::cors_filter(&sr.headers),
            ),
            _ => (
                make("opaque", None, 0, String::new(), None),
                HeaderList::default(),
            ),
        },
        "opaque" => (
            make("opaque", None, 0, String::new(), None),
            HeaderList::default(),
        ),
        t => (make(t, url, sr.status, sr.status_text, body), sr.headers),
    })
}

// --- Repartir los avisos ---

/// Reparte un aviso en una tarea del agente (lo llama `workers::handle`).
pub(crate) fn deliver(agent: &mut Agent, m: Msg) {
    match m {
        Msg::Job {
            client,
            promise,
            result,
        } => {
            let Some(realm) = realm_of(agent, client) else {
                return;
            };
            let (kind, payload) = match result {
                JobResult::Reg(s) => ("reg", s),
                JobResult::Bool(b) => ("bool", b.to_string()),
                JobResult::Err(n, m) => (
                    "err",
                    format!("{{\"name\":{},\"message\":{}}}", js(n), js(&m)),
                ),
            };
            call_hook(
                agent,
                realm,
                "job",
                &[
                    Value::Number(promise as f64),
                    Value::str(kind),
                    Value::str(&payload),
                ],
            );
        }
        Msg::RegState { reg, which, worker } => {
            for realm in all_realms(agent) {
                call_hook(
                    agent,
                    realm,
                    "regState",
                    &[
                        Value::Number(reg as f64),
                        Value::str(which),
                        Value::str(&worker),
                    ],
                );
            }
        }
        Msg::WorkerState { worker, state } => {
            for realm in all_realms(agent) {
                call_hook(
                    agent,
                    realm,
                    "workerState",
                    &[Value::Number(worker as f64), Value::str(state.as_str())],
                );
            }
        }
        Msg::UpdateFound { reg } => {
            for realm in all_realms(agent) {
                call_hook(agent, realm, "updateFound", &[Value::Number(reg as f64)]);
            }
        }
        Msg::Controller { client, worker } => {
            if let Some(realm) = realm_of(agent, client) {
                call_hook(agent, realm, "controller", &[Value::str(&worker)]);
            }
        }
        Msg::Ready { client, reg } => {
            if let Some(realm) = realm_of(agent, client) {
                call_hook(agent, realm, "ready", &[Value::str(&reg)]);
            }
        }
        Msg::ToClient {
            client,
            source,
            origin,
            message,
        } => {
            if let Some(realm) = realm_of(agent, client) {
                deliver_message(
                    agent,
                    realm,
                    &source,
                    &origin,
                    message,
                    "message",
                    "messageError",
                );
            }
        }
        Msg::FetchReply { fetch, outcome } => {
            let Some(p) = CLIENT_FETCH.with(|f| f.borrow_mut().remove(&fetch)) else {
                return;
            };
            let saved = agent.current_realm;
            agent.current_realm = p.realm;
            let result = match outcome {
                FetchOutcome::Fallback => None,
                FetchOutcome::Error(m) => Some(Err(m)),
                FetchOutcome::Response(sr) => Some(client_response(&p, *sr)),
            };
            (p.done)(agent, result);
            agent.current_realm = saved;
        }
        Msg::Lifecycle { kind, done } => {
            let Some(realm) = realm_of(agent, 0) else {
                let _ = done.send(false);
                return;
            };
            let token = next_id();
            LIFECYCLE.with(|l| l.borrow_mut().insert(token, done));
            if !call_hook(
                agent,
                realm,
                "lifecycle",
                &[Value::Number(token as f64), Value::str(kind)],
            ) {
                lifecycle_done(token, false);
            }
        }
        Msg::ToWorker {
            client,
            origin,
            message,
        } => {
            if let Some(realm) = realm_of(agent, 0) {
                deliver_message(
                    agent,
                    realm,
                    &client,
                    &origin,
                    message,
                    "swMessage",
                    "swMessageError",
                );
            }
        }
        Msg::Fetch {
            fetch,
            client,
            request,
            reply,
        } => {
            let Some(realm) = realm_of(agent, 0) else {
                send(
                    &reply,
                    Msg::FetchReply {
                        fetch,
                        outcome: FetchOutcome::Fallback,
                    },
                );
                return;
            };
            SW_FETCH.with(|f| f.borrow_mut().insert(fetch, reply));
            let saved = agent.current_realm;
            agent.current_realm = realm;
            let SwRequest { data, headers } = *request;
            let req = crate::net::fetch::new_signal(agent).and_then(|signal| {
                crate::net::fetch::make_request(
                    agent,
                    None,
                    data,
                    HeadersData {
                        list: headers,
                        guard: Guard::Immutable,
                    },
                    signal,
                    None,
                )
            });
            agent.current_realm = saved;
            let called = match req {
                Ok(req) => call_hook(
                    agent,
                    realm,
                    "fetch",
                    &[Value::Number(fetch as f64), req, Value::str(&client)],
                ),
                Err(_) => false,
            };
            if !called {
                fetch_reply(fetch, FetchOutcome::Fallback);
            }
        }
    }
}

/// Un mensaje serializado para `realm`: `ok_hook(source, origin, data, ports)` o, si no se
/// puede deserializar, `err_hook(source, origin)`.
fn deliver_message(
    agent: &mut Agent,
    realm: RealmId,
    source: &str,
    origin: &str,
    message: Serialized,
    ok_hook: &str,
    err_hook: &str,
) {
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let r = serialize::deserialize_with_transfer(agent, message);
    let args = match r {
        Ok((data, transferred)) => {
            let ports = super::ports::ports_of(agent, &transferred);
            let arr = agent
                .create_array_from_list(&ports)
                .map_or(Value::Null, Value::Object);
            Some(vec![Value::str(source), Value::str(origin), data, arr])
        }
        Err(_) => None,
    };
    agent.current_realm = saved;
    match args {
        Some(args) => {
            call_hook(agent, realm, ok_hook, &args);
        }
        None => {
            call_hook(
                agent,
                realm,
                err_hook,
                &[Value::str(source), Value::str(origin)],
            );
        }
    }
}

/// Las funciones de `service.js` del realm (fijadas).
struct ServiceJs {
    hooks: HostHandle,
}

/// Llama a `hooks[name](...args)` en `realm`; `false` si no hay o lanzó.
fn call_hook(agent: &mut Agent, realm: RealmId, name: &str, args: &[Value]) -> bool {
    let Some(h) = agent.realm_data::<ServiceJs>(realm).map(|s| s.hooks) else {
        return false;
    };
    let Some(Value::Object(o)) = agent.host_pinned(h) else {
        return false;
    };
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let r = agent
        .get(o, &PropertyKey::from(name), Value::Object(o))
        .and_then(|f| agent.call(&f, Value::Undefined, args));
    let ok = match r {
        Ok(_) => true,
        Err(Throw::Value(e)) => {
            crate::dom::events::report_exception(agent, &e);
            false
        }
        Err(Throw::Terminate(_)) => false,
    };
    agent.current_realm = saved;
    ok
}

fn lifecycle_done(token: u64, ok: bool) {
    if let Some(tx) = LIFECYCLE.with(|l| l.borrow_mut().remove(&token)) {
        let _ = tx.send(ok);
    }
}

fn fetch_reply(fetch: u64, outcome: FetchOutcome) {
    if let Some(reply) = SW_FETCH.with(|f| f.borrow_mut().remove(&fetch)) {
        send(&reply, Msg::FetchReply { fetch, outcome });
    }
}

// --- Instalación en un realm ---

/// Evalúa `service.js` en `realm` con sus funciones nativas (`mode`: `window` o `service`).
///
/// E1: dentro de un ámbito de enraizado ([`Agent::rooting_scope`]): crear los métodos y
/// compilar `service.js` pueden recolectar antes de que `native` quede enlazado.
fn install_js(agent: &mut Agent, realm: RealmId, mode: &str) {
    agent.rooting_scope(|agent| install_js_rooted(agent, realm, mode));
}

fn install_js_rooted(agent: &mut Agent, realm: RealmId, mode: &str) {
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let native = agent.new_plain_object();
    let fns: [(&str, u32, NativeFn); 22] = [
        ("fire", 2, n_fire),
        ("register", 5, n_register),
        ("getRegistration", 1, n_get_registration),
        ("getRegistrations", 0, n_get_registrations),
        ("controller", 0, n_controller),
        ("ready", 0, n_ready),
        ("used", 0, n_used),
        ("update", 2, n_update),
        ("unregister", 2, n_unregister),
        ("postToWorker", 3, n_post_to_worker),
        ("selfInfo", 0, n_self_info),
        ("skipWaiting", 0, n_skip_waiting),
        ("claim", 0, n_claim),
        ("matchAll", 2, n_match_all),
        ("getClient", 1, n_get_client),
        ("postToClient", 3, n_post_to_client),
        ("lifecycleDone", 2, n_lifecycle_done),
        ("fetchDone", 4, n_fetch_done),
        ("isRequest", 1, n_is_request),
        ("isResponse", 1, n_is_response),
        ("isMessagePort", 1, n_is_message_port),
        ("hasActive", 1, n_has_active),
    ];
    for (name, len, f) in fns {
        bf_js::builtins::method(agent, native, name, len, f);
    }
    let g = Value::Object(agent.realms[realm].global_object);
    if let Ok(f) = agent.eval_script_cached(SERVICE_JS, "bf-web:service.js")
        && let Ok(hooks) = agent.call(
            &f,
            Value::Undefined,
            &[g, Value::Object(native), Value::str(mode)],
        )
    {
        let h = agent.host_pin(hooks);
        agent.set_realm_data(realm, ServiceJs { hooks: h });
    }
    agent.current_realm = saved;
}

/// El global de un service worker (lo llama `workers::scope::install`).
pub(crate) fn install_scope(agent: &mut Agent, realm: RealmId, info: &ServiceInfo) {
    let l = local(agent);
    l.realms.push((0, realm));
    l.sw = Some((info.worker, info.reg));
    if info.root.is_some() {
        crate::storage::set_storage_dir(agent, realm, info.root.clone());
    }
    install_js(agent, realm, "service");
}

/// `importScripts` en un service worker (§6.3.2 "importScripts(urls)" con el *script
/// resource map*): durante `parsed`/`installing` se trae y se guarda; después, solo lo
/// guardado (si no, `NetworkError`). `None`: el agente no es un service worker.
pub(crate) fn import_script(agent: &Agent, url: &Url) -> Option<Result<String, String>> {
    let (wid, _) = agent.embedder_data::<Local>()?.sw?;
    let u = url.serialize(false);
    let (state, cached, script_url) = {
        let g = global();
        let v = g.versions.get(&wid)?;
        let cached = v
            .imports
            .iter()
            .find(|(x, _)| *x == u)
            .map(|(_, t)| t.clone());
        (v.state, cached, v.script_url.clone())
    };
    if let Some(t) = cached {
        return Some(Ok((*t).clone()));
    }
    if !matches!(state, State::Parsed | State::Installing) {
        return Some(Err(format!("The script at '{u}' is not cached.")));
    }
    let backend = {
        let g = global();
        let rid = g.versions.get(&wid)?.reg;
        g.regs.iter().find(|r| r.id == rid)?.backend.clone()?
    };
    let _ = script_url;
    let r = super::scope::fetch_script(backend.as_ref(), url, MimeCheck::Imported);
    if let Ok(t) = &r
        && let Some(v) = global().versions.get_mut(&wid)
    {
        v.imports.push((u, Arc::new(t.clone())));
    }
    Some(r)
}

// --- Las funciones nativas de `service.js` ---

fn str_arg(agent: &mut Agent, v: &Value) -> JsResult<String> {
    Ok(agent.to_string(v)?.to_string())
}

fn num_arg(agent: &mut Agent, v: &Value) -> JsResult<u64> {
    Ok(agent.to_number(v)? as u64)
}

fn error_pair(agent: &mut Agent, name: &str, msg: &str) -> JsResult<Value> {
    let a = agent.create_array_from_list(&[Value::str(name), Value::str(msg)])?;
    Ok(Value::Object(a))
}

/// `fire(target, event)`: despacha `event` como evento de confianza; `true` si no se
/// canceló. Los microtasks corren después de cada listener.
fn n_fire(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some((doc, t)) = crate::dom::events::event_target_of(agent, &a.arg(0)) else {
        return agent.type_error("fire: not an EventTarget");
    };
    let Value::Object(evo) = a.arg(1) else {
        return agent.type_error("fire: not an Event");
    };
    let Some(cell) = agent
        .host_unwrap::<crate::dom::events::EventData>(&a.arg(1))
        .map(|d| d.cell.clone())
    else {
        return agent.type_error("fire: not an Event");
    };
    cell.borrow_mut().is_trusted = true;
    let pin = agent.host_pin(Value::Object(evo));
    let r = crate::dom::events::with_checkpoint_each(|| {
        crate::dom::events::dispatch_js_event(agent, doc, t, evo, false)
    });
    agent.host_unpin(pin);
    let _ = agent.perform_microtask_checkpoint();
    Ok(Value::Boolean(matches!(r, Ok(Ok(true)))))
}

/// La storage key, el backend, la raíz y el User-Agent del realm en curso.
fn context(agent: &mut Agent) -> (Key, Arc<dyn NetBackend>, Option<PathBuf>, String) {
    let realm = agent.current_realm;
    let origin = crate::net::document_origin(agent);
    let key = (super::partition(agent), origin);
    let backend = crate::net::current_backend(agent);
    let root = crate::storage::root_dir(agent, realm);
    let ua = agent.host.hooks.user_agent();
    (key, backend, root, ua)
}

/// ¿El path de la URL tiene `%2f` o `%5c`? (§3.4.3 "Start Register" pasos 4 y 7.)
fn has_escaped_slash(u: &Url) -> bool {
    let p = u.path_string().to_ascii_lowercase();
    p.contains("%2f") || p.contains("%5c")
}

/// Programa un trabajo con la promesa `pid` del realm en curso.
fn schedule_from(
    agent: &mut Agent,
    kind: JobKind,
    scope: Url,
    script: Option<Url>,
    t: ScriptType,
    uvc: String,
    pid: u64,
) {
    let realm = agent.current_realm;
    let (key, backend, root, ua) = context(agent);
    let client = client_of(agent, realm).unwrap_or(0);
    let inbox = super::inbox(agent);
    local(agent).used = true;
    super::ensure_polling(agent, realm);
    schedule(Job {
        kind,
        key: key.clone(),
        scope,
        script_url: script,
        script_type: t,
        update_via_cache: uvc,
        reply: Some((inbox, client, pid)),
        referrer_origin: key.1,
        backend,
        root,
        user_agent: ua,
    });
}

/// `register(scriptURL, scope | null, type, updateViaCache, pid)` (§3.4.3 "Start Register"):
/// `null` si el trabajo quedó programado; `[nombre, mensaje]` para rechazar.
fn n_register(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let script = str_arg(agent, &a.arg(0))?;
    let scope = if a.arg(1).is_null() {
        None
    } else {
        Some(str_arg(agent, &a.arg(1))?)
    };
    let t = match str_arg(agent, &a.arg(2))?.as_str() {
        "module" => ScriptType::Module,
        _ => ScriptType::Classic,
    };
    let uvc = str_arg(agent, &a.arg(3))?;
    let pid = num_arg(agent, &a.arg(4))?;
    let base = WebContextHandle::current(agent).get(agent).base.clone();
    let Ok(script_url) = Url::parse(&script, Some(&base)) else {
        return error_pair(
            agent,
            "TypeError",
            &format!("Failed to register a ServiceWorker: The URL '{script}' is invalid."),
        );
    };
    if !matches!(script_url.scheme.as_str(), "http" | "https") {
        return error_pair(
            agent,
            "TypeError",
            "Failed to register a ServiceWorker: The URL protocol of the script ('{}') is not supported.",
        );
    }
    if has_escaped_slash(&script_url) {
        return error_pair(
            agent,
            "TypeError",
            "Failed to register a ServiceWorker: The provided scriptURL includes a disallowed escape character.",
        );
    }
    let scope_url = match &scope {
        None => Url::parse("./", Some(&script_url)),
        Some(s) => Url::parse(s, Some(&base)),
    };
    let Ok(mut scope_url) = scope_url else {
        return error_pair(
            agent,
            "TypeError",
            "Failed to register a ServiceWorker: The scope URL is invalid.",
        );
    };
    scope_url.fragment = None;
    if !matches!(scope_url.scheme.as_str(), "http" | "https") {
        return error_pair(
            agent,
            "TypeError",
            "Failed to register a ServiceWorker: The URL protocol of the scope is not supported.",
        );
    }
    if has_escaped_slash(&scope_url) {
        return error_pair(
            agent,
            "TypeError",
            "Failed to register a ServiceWorker: The provided scope includes a disallowed escape character.",
        );
    }
    schedule_from(
        agent,
        JobKind::Register,
        scope_url,
        Some(script_url),
        t,
        uvc,
        pid,
    );
    Ok(Value::Null)
}

/// `getRegistration(clientURL)` (§3.4.4): la instantánea, `null` (ninguno) o
/// `[nombre, mensaje]`.
fn n_get_registration(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = str_arg(agent, &a.arg(0))?;
    let base = WebContextHandle::current(agent).get(agent).base.clone();
    let Ok(url) = Url::parse(&s, Some(&base)) else {
        return error_pair(
            agent,
            "TypeError",
            "Failed to execute 'getRegistration': The provided documentURL is invalid.",
        );
    };
    let (key, backend, root, ua) = context(agent);
    if url.origin().to_string() != key.1 {
        return error_pair(
            agent,
            "SecurityError",
            "Failed to execute 'getRegistration': The origin of the provided documentURL does not match the current origin.",
        );
    }
    local(agent).used = true;
    let mut g = global();
    if let Some(root) = &root {
        load(&mut g, &key, root, Some(&backend), &ua);
    }
    Ok(match match_reg(&g, &key, &url.serialize(false)) {
        Some(i) => Value::str(&rsnap(&g, &g.regs[i])),
        None => Value::Null,
    })
}

/// `getRegistrations()` (§3.4.5): un array JSON en orden de creación.
fn n_get_registrations(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let (key, backend, root, ua) = context(agent);
    local(agent).used = true;
    let mut g = global();
    if let Some(root) = &root {
        load(&mut g, &key, root, Some(&backend), &ua);
    }
    let items: Vec<String> = g
        .regs
        .iter()
        .filter(|r| r.in_map && r.key == key)
        .map(|r| rsnap(&g, r))
        .collect();
    Ok(Value::str(&format!("[{}]", items.join(","))))
}

/// La instantánea del *active service worker* del cliente en curso (o `null`).
fn n_controller(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let realm = agent.current_realm;
    let Some(cid) = client_of(agent, realm) else {
        return Ok(Value::str("null"));
    };
    let g = global();
    let w = g
        .clients
        .iter()
        .find(|c| c.id == cid)
        .and_then(|c| c.controller);
    Ok(Value::str(&wsnap(&g, w)))
}

/// `ready` (§3.4.2): el registro de la URL del cliente si ya tiene *active worker*.
fn n_ready(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let realm = agent.current_realm;
    let Some(cid) = client_of(agent, realm) else {
        return Ok(Value::Null);
    };
    local(agent).used = true;
    super::ensure_polling(agent, realm);
    let g = global();
    let Some(c) = g.clients.iter().find(|c| c.id == cid) else {
        return Ok(Value::Null);
    };
    Ok(match match_reg(&g, &c.key, &c.url) {
        Some(i) if g.regs[i].active.is_some() => Value::str(&rsnap(&g, &g.regs[i])),
        _ => Value::Null,
    })
}

/// La ventana usó `navigator.serviceWorker`: el sondeo de su bandeja sigue.
fn n_used(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let realm = agent.current_realm;
    local(agent).used = true;
    super::ensure_polling(agent, realm);
    Ok(Value::Undefined)
}

/// `update(regId, pid)` (§3.2.7).
fn n_update(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let rid = num_arg(agent, &a.arg(0))?;
    let pid = num_arg(agent, &a.arg(1))?;
    let me = agent.embedder_data::<Local>().and_then(|l| l.sw);
    let found = {
        let g = global();
        reg_idx(&g, rid).and_then(|i| {
            let r = &g.regs[i];
            let n = newest(r).and_then(|n| g.versions.get(&n))?;
            let installing = me
                .and_then(|(w, _)| g.versions.get(&w))
                .is_some_and(|v| v.state == State::Installing);
            Some((
                r.scope.clone(),
                n.script_url.clone(),
                n.script_type,
                r.update_via_cache.clone(),
                installing,
            ))
        })
    };
    let Some((scope, script, t, uvc, installing)) = found else {
        return error_pair(
            agent,
            "InvalidStateError",
            "Failed to update a ServiceWorker: The registration has no worker.",
        );
    };
    if installing {
        return error_pair(
            agent,
            "InvalidStateError",
            "Failed to update a ServiceWorker: The service worker is installing.",
        );
    }
    let Ok(scope) = Url::parse(&scope, None) else {
        return error_pair(agent, "TypeError", "scope inválido");
    };
    schedule_from(agent, JobKind::Update, scope, Some(script), t, uvc, pid);
    Ok(Value::Null)
}

/// `unregister(regId, pid)` (§3.2.8).
fn n_unregister(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let rid = num_arg(agent, &a.arg(0))?;
    let pid = num_arg(agent, &a.arg(1))?;
    let scope = {
        let g = global();
        reg_idx(&g, rid).map(|i| g.regs[i].scope.clone())
    };
    let Some(Ok(scope)) = scope.map(|s| Url::parse(&s, None)) else {
        // Ya no existe: como un registro que no está en el mapa.
        let realm = agent.current_realm;
        if let Some(cid) = client_of(agent, realm) {
            let inbox = super::inbox(agent);
            send(
                &inbox,
                Msg::Job {
                    client: cid,
                    promise: pid,
                    result: JobResult::Bool(false),
                },
            );
        }
        return Ok(Value::Null);
    };
    schedule_from(
        agent,
        JobKind::Unregister,
        scope,
        None,
        ScriptType::Classic,
        String::new(),
        pid,
    );
    Ok(Value::Null)
}

/// `postToWorker(workerId, message, transfer)` (§3.1.3 `ServiceWorker.postMessage`).
fn n_post_to_worker(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let wid = num_arg(agent, &a.arg(0))?;
    let transfer = super::ports::transfer_argument(agent, &a.arg(2))?;
    let s = super::ports::serialize(agent, &a.arg(1), &transfer, false)?;
    let realm = agent.current_realm;
    let origin = crate::net::document_origin(agent);
    let cid = client_of(agent, realm);
    let client = {
        let g = global();
        if g.versions
            .get(&wid)
            .is_none_or(|v| v.state == State::Redundant)
        {
            return Ok(Value::Undefined);
        }
        cid.and_then(|c| g.clients.iter().find(|x| x.id == c))
            .map_or_else(|| "null".to_owned(), csnap)
    };
    let backend = crate::net::current_backend(agent);
    let ua = agent.host.hooks.user_agent();
    if let Some(inbox) = start(wid, None, Some(&backend), &ua) {
        send(
            &inbox,
            Msg::ToWorker {
                client,
                origin,
                message: s,
            },
        );
    }
    Ok(Value::Undefined)
}

fn my_sw(agent: &Agent) -> Option<(u64, u64)> {
    agent.embedder_data::<Local>().and_then(|l| l.sw)
}

/// `{"reg": …, "worker": …}` del service worker en curso.
fn n_self_info(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let Some((w, r)) = my_sw(agent) else {
        return Ok(Value::Null);
    };
    let g = global();
    let reg = reg_idx(&g, r).map_or_else(|| "null".to_owned(), |i| rsnap(&g, &g.regs[i]));
    Ok(Value::str(&format!(
        "{{\"reg\":{reg},\"worker\":{}}}",
        wsnap(&g, Some(w))
    )))
}

/// `skipWaiting()` (§4.1.3): pone el flag e invoca Try Activate en paralelo.
fn n_skip_waiting(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let Some((w, r)) = my_sw(agent) else {
        return Ok(Value::Undefined);
    };
    if let Some(v) = global().versions.get_mut(&w) {
        v.skip_waiting = true;
    }
    let _ = std::thread::Builder::new()
        .name("service worker skipWaiting".into())
        .spawn(move || try_activate(r));
    Ok(Value::Undefined)
}

/// `clients.claim()` (§4.3.4): `null` o el nombre del error.
fn n_claim(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let Some((w, r)) = my_sw(agent) else {
        return Ok(Value::str("InvalidStateError"));
    };
    let mut g = global();
    let g = &mut *g;
    let Some(i) = reg_idx(g, r) else {
        return Ok(Value::str("InvalidStateError"));
    };
    let active = g.regs[i].active == Some(w)
        && g.versions
            .get(&w)
            .is_some_and(|v| matches!(v.state, State::Activating | State::Activated));
    if !active {
        return Ok(Value::str("InvalidStateError"));
    }
    let key = g.regs[i].key.clone();
    let ws = wsnap(g, Some(w));
    let targets: Vec<u64> = g
        .clients
        .iter()
        .filter(|c| c.key == key && c.controller != Some(w))
        .filter(|c| match_reg(g, &key, &c.url) == Some(i))
        .map(|c| c.id)
        .collect();
    for c in g.clients.iter_mut().filter(|c| targets.contains(&c.id)) {
        c.controller = Some(w);
        send(
            &c.inbox,
            Msg::Controller {
                client: c.id,
                worker: ws.clone(),
            },
        );
    }
    Ok(Value::Null)
}

/// `matchAll(includeUncontrolled, type)` (§4.3.2): un array JSON de clientes.
fn n_match_all(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let include = bf_js::host::webidl::to_boolean(agent, &a.arg(0));
    let t = str_arg(agent, &a.arg(1))?;
    let Some((w, _)) = my_sw(agent) else {
        return Ok(Value::str("[]"));
    };
    if !matches!(t.as_str(), "window" | "all") {
        return Ok(Value::str("[]"));
    }
    let g = global();
    let Some(key) = g.versions.get(&w).map(|v| v.key.clone()) else {
        return Ok(Value::str("[]"));
    };
    let items: Vec<String> = g
        .clients
        .iter()
        .filter(|c| c.key == key && (include || c.controller == Some(w)))
        .map(csnap)
        .collect();
    Ok(Value::str(&format!("[{}]", items.join(","))))
}

/// `clients.get(id)` (§4.3.1): el cliente (JSON) o `"null"`.
fn n_get_client(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = str_arg(agent, &a.arg(0))?;
    let Some((w, _)) = my_sw(agent) else {
        return Ok(Value::str("null"));
    };
    let g = global();
    let key = g.versions.get(&w).map(|v| v.key.clone());
    Ok(Value::str(
        &g.clients
            .iter()
            .find(|c| c.uuid == id && Some(&c.key) == key.as_ref())
            .map_or_else(|| "null".to_owned(), csnap),
    ))
}

/// `postToClient(clientId, message, transfer)` (§4.2.7 `Client.postMessage`).
fn n_post_to_client(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = str_arg(agent, &a.arg(0))?;
    let transfer = super::ports::transfer_argument(agent, &a.arg(2))?;
    let s = super::ports::serialize(agent, &a.arg(1), &transfer, false)?;
    let Some((w, _)) = my_sw(agent) else {
        return Ok(Value::Undefined);
    };
    let origin = crate::net::document_origin(agent);
    let g = global();
    let source = wsnap(&g, Some(w));
    if let Some(c) = g.clients.iter().find(|c| c.uuid == id) {
        send(
            &c.inbox,
            Msg::ToClient {
                client: c.id,
                source,
                origin,
                message: s,
            },
        );
    }
    Ok(Value::Undefined)
}

fn n_lifecycle_done(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let token = num_arg(agent, &a.arg(0))?;
    let ok = bf_js::host::webidl::to_boolean(agent, &a.arg(1));
    lifecycle_done(token, ok);
    Ok(Value::Undefined)
}

/// `fetchDone(id, kind, response, bytes)`: `kind` es `fallback`, `error` o `response`.
fn n_fetch_done(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let fid = num_arg(agent, &a.arg(0))?;
    let kind = str_arg(agent, &a.arg(1))?;
    let outcome = match kind.as_str() {
        "fallback" => FetchOutcome::Fallback,
        "response" => match agent.host_unwrap::<ResponseData>(&a.arg(2)).cloned() {
            Some(d) => {
                let headers = match &a.arg(2) {
                    Value::Object(o) => {
                        let hv = agent
                            .host_slots_mut(*o)
                            .and_then(|s| s.first().cloned())
                            .unwrap_or(Value::Undefined);
                        crate::net::headers::data(agent, &hv)
                            .map(|h| h.list)
                            .unwrap_or_default()
                    }
                    _ => HeaderList::default(),
                };
                let body = if a.arg(3).is_null() {
                    None
                } else {
                    bf_js::builtins::web::encoding::buffer_source_bytes(agent, &a.arg(3))
                };
                FetchOutcome::Response(Box::new(SwResponse {
                    type_: d.type_,
                    url: d.url,
                    redirected: d.redirected,
                    status: d.status,
                    status_text: d.status_text,
                    headers,
                    body,
                }))
            }
            None => FetchOutcome::Error("respondWith() was not given a Response".into()),
        },
        _ => FetchOutcome::Error("the service worker rejected the request".into()),
    };
    fetch_reply(fid, outcome);
    Ok(Value::Undefined)
}

fn n_is_request(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(
        agent.host_unwrap::<RequestData>(&a.arg(0)).is_some(),
    ))
}

fn n_is_response(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(
        agent.host_unwrap::<ResponseData>(&a.arg(0)).is_some(),
    ))
}

fn n_is_message_port(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let c = crate::net::classes(agent).message_port;
    Ok(Value::Boolean(agent.host_is_instance(&a.arg(0), c)))
}

/// `hasActive(regId)`: ¿tiene el registro *active worker*? (`navigationPreload.enable()`.)
fn n_has_active(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let rid = num_arg(agent, &a.arg(0))?;
    let g = global();
    Ok(Value::Boolean(
        reg_idx(&g, rid).is_some_and(|i| g.regs[i].active.is_some()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_strings() {
        assert_eq!(js("a\"b\\c\n"), "\"a\\\"b\\\\c\\n\"");
        assert_eq!(js("\u{1}"), "\"\\u0001\"");
    }

    #[test]
    fn saved_registration_round_trip() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(
            b"FREEDOM-SW 1\nscope https://a.test/x/\nupdateViaCache imports\ntype classic\n",
        );
        bytes.extend_from_slice(b"script https://a.test/x/sw.js 5\nhola\n\n");
        bytes.extend_from_slice(b"import https://a.test/x/i.js 2\nab\n");
        let (scope, uvc, t, url, text, imports) = parse_saved(&bytes).expect("parse");
        assert_eq!(scope, "https://a.test/x/");
        assert_eq!(uvc, "imports");
        assert_eq!(t, ScriptType::Classic);
        assert_eq!(url.serialize(false), "https://a.test/x/sw.js");
        assert_eq!(text, "hola\n");
        assert_eq!(
            imports,
            vec![("https://a.test/x/i.js".to_owned(), "ab".to_owned())]
        );
    }

    #[test]
    fn uuids_are_v4() {
        let a = uuid();
        let b = uuid();
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "4");
    }
}
