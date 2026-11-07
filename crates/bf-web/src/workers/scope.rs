//! El lado del worker (HTML §10.2–10.3): el hilo y su agente, los objetos globales
//! `WorkerGlobalScope`, `DedicatedWorkerGlobalScope` y `SharedWorkerGlobalScope`,
//! `WorkerLocation`, `WorkerNavigator`, `importScripts`, `postMessage`/`close` y el bucle de
//! eventos del worker ("run a worker", §10.2.4).
//!
//! El global reutiliza las clases del DOM y de la red del agente (EventTarget, eventos,
//! `fetch`, `XMLHttpRequest`, `WebSocket`, `MessageChannel`, `Blob`…) sobre un documento vacío
//! que nadie ve (da la URL y el origen); las interfaces que solo existen en `Window` (`Node`,
//! `Document`, `Window`…) se quitan del global. El payload del global es el de la ventana
//! (`WindowData`): para el mecanismo de eventos es el target global (`onerror`, "report an
//! exception", `unhandledrejection`).

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use bf_dom::events::EventTarget;
use bf_infra::Url;
use bf_js::builtins::web::serialize::{self, Serialized};
use bf_js::host::{ConsoleLevel, HostClass, HostClassId, HostHooks, Runtime};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, AgentOptions, JsResult, RealmId, Throw};

use super::{AgentInbox, Envelope, WorkerControl, WorkerEvent, WorkerHost};
use crate::dom::events::{GLOBAL_EVENT_HANDLERS_WORKER, TargetRef, with_event_handlers};
use crate::net::{NetBackend, NetRequest};
use crate::{DocumentOwner, WebContext, WebContextHandle};

/// La pila del hilo de un worker y el límite de la VM dentro de ella.
const THREAD_STACK: usize = 64 << 20;
const JS_STACK: usize = 32 << 20;

/// `WorkerType` (HTML §10.2.6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptType {
    Classic,
    Module,
}

/// Qué clase de worker es.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeKind {
    Dedicated,
    /// Un `SharedWorker` con su registro.
    Shared(Arc<super::shared::SharedEntry>),
}

/// Todo lo que el hilo de un worker necesita (se manda al hilo).
pub struct WorkerSpec {
    pub url: Url,
    pub script_type: ScriptType,
    pub name: String,
    pub kind: ScopeKind,
    pub backend: Arc<dyn NetBackend>,
    /// La bandeja del dueño (para un `SharedWorker`, `None`: habla por sus puertos).
    pub parent: Option<Arc<AgentInbox>>,
    /// El id del target del objeto `Worker` en el dueño.
    pub worker_id: u64,
    /// La bandeja del worker (su agente la vacía).
    pub inbox: Arc<AgentInbox>,
    pub control: Arc<WorkerControl>,
    /// ¿Pueden cruzar `SharedArrayBuffer`? (El agent cluster del dueño está aislado.)
    pub cross_origin_isolated: bool,
    /// El User-Agent del dueño.
    pub user_agent: String,
    /// La partición del dueño (`workers::set_partition`).
    pub partition: u64,
}

/// El estado del lado worker (en el `WorkerHost` del agente del worker).
pub(crate) struct ScopeState {
    pub parent: Option<Arc<AgentInbox>>,
    pub worker_id: u64,
    pub kind: ScopeKind,
    pub name: String,
    pub script_type: ScriptType,
    pub url: Url,
    /// `close()`: el bucle termina después de la tarea en curso.
    pub closing: bool,
    pub cross_origin_isolated: bool,
    pub user_agent: String,
}

/// Arranca el hilo de un worker.
pub fn spawn(spec: WorkerSpec) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name(format!("worker {}", spec.url.serialize(false)))
        .stack_size(THREAD_STACK)
        .spawn(move || run(spec))
        .map(|_| ())
}

// --- Los hooks del agente del worker ---

/// La consola y las excepciones sin atrapar van al dueño.
struct WorkerHooks {
    parent: Option<Arc<AgentInbox>>,
    worker_id: u64,
    url: String,
}

impl HostHooks for WorkerHooks {
    fn console(&mut self, level: ConsoleLevel, message: &str) {
        match &self.parent {
            Some(p) => p.push(Envelope::FromWorker {
                worker: self.worker_id,
                event: WorkerEvent::Console(level, message.to_owned()),
            }),
            None => eprintln!("[SharedWorker] {message}"),
        }
    }

    fn report_exception(&mut self, message: &str, stack: &str) {
        let (filename, lineno, colno) =
            location_from_stack(stack).unwrap_or((self.url.clone(), 0, 0));
        match &self.parent {
            Some(p) => p.push(Envelope::FromWorker {
                worker: self.worker_id,
                event: WorkerEvent::Error {
                    message: format!("Uncaught {message}"),
                    filename,
                    lineno,
                    colno,
                },
            }),
            None => eprintln!("[SharedWorker] Uncaught {message}"),
        }
    }
}

/// `(archivo, línea, columna)` del primer marco de una pila (`at f (url:l:c)`).
pub(crate) fn location_from_stack(stack: &str) -> Option<(String, u32, u32)> {
    for line in stack.lines() {
        let t = line.trim();
        let t = t.strip_prefix("at ").unwrap_or(t);
        let t = match (t.rfind('('), t.ends_with(')')) {
            (Some(i), true) => &t[i + 1..t.len() - 1],
            _ => t,
        };
        let mut parts = t.rsplitn(3, ':');
        let (Some(c), Some(l), Some(file)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        if let (Ok(l), Ok(c)) = (l.parse(), c.parse()) {
            return Some((file.to_owned(), l, c));
        }
    }
    None
}

// --- El hilo ---

fn run(spec: WorkerSpec) {
    let parent = spec.parent.clone();
    let worker_id = spec.worker_id;
    let control = spec.control.clone();
    let send = |event: WorkerEvent| {
        if let Some(p) = &parent {
            p.push(Envelope::FromWorker {
                worker: worker_id,
                event,
            });
        }
    };
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_inner(spec)));
    if r.is_err() {
        send(WorkerEvent::Console(
            ConsoleLevel::Error,
            "el worker terminó por un error interno".into(),
        ));
    }
    control.closed.store(true, Ordering::SeqCst);
    send(WorkerEvent::Closed);
}

/// Crea el agente del worker con su global; `None` si el realm no se pudo preparar.
fn create_agent(spec: &WorkerSpec) -> Runtime {
    let hooks = WorkerHooks {
        parent: spec.parent.clone(),
        worker_id: spec.worker_id,
        url: spec.url.serialize(false),
    };
    let mut rt = Runtime::with_options(
        Box::new(hooks),
        AgentOptions {
            max_stack: JS_STACK,
            ..AgentOptions::default()
        },
    );
    let agent = &mut rt.agent;
    // La señal del agente es la de su bandeja (el dueño lo despierta por ahí).
    agent.signal = spec.inbox.signal().clone();
    agent.interrupt = Some(spec.control.interrupt.clone());
    // Un agente de worker puede suspenderse (`Atomics.wait`).
    agent.can_block = true;
    let realm = agent.current_realm;
    agent.set_embedder_data(WorkerHost {
        inbox: spec.inbox.clone(),
        workers: Default::default(),
        manual: true,
        poll: None,
        scope: Some(ScopeState {
            parent: spec.parent.clone(),
            worker_id: spec.worker_id,
            kind: spec.kind.clone(),
            name: spec.name.clone(),
            script_type: spec.script_type,
            url: spec.url.clone(),
            closing: false,
            cross_origin_isolated: spec.cross_origin_isolated,
            user_agent: spec.user_agent.clone(),
        }),
        shared: Default::default(),
        shared_errors: Default::default(),
        partition: spec.partition,
    });
    install(agent, realm, &spec.url, &spec.kind, spec.backend.clone());
    rt
}

fn run_inner(spec: WorkerSpec) {
    let mut rt = create_agent(&spec);
    let agent = &mut rt.agent;
    // "Fetch a classic/module worker script" (§10.2.4 paso 12) y su ejecución.
    let ok = match spec.script_type {
        ScriptType::Classic => {
            match fetch_script(spec.backend.as_ref(), &spec.url, MimeCheck::WorkerScript) {
                Ok(text) => {
                    let src = spec.url.serialize(false);
                    match agent.eval_script(&text, &src) {
                        Ok(_) => true,
                        Err(Throw::Value(e)) => {
                            crate::dom::events::report_exception(agent, &e);
                            true
                        }
                        Err(Throw::Terminate(_)) => false,
                    }
                }
                Err(_) => {
                    load_error(&spec);
                    false
                }
            }
        }
        ScriptType::Module => {
            let loader = std::rc::Rc::new(ModuleFetcher {
                backend: spec.backend.clone(),
            });
            agent.set_module_loader(loader);
            // Un error al cargar el grafo es un error de carga; uno al evaluar, una
            // excepción del worker.
            if fetch_script(spec.backend.as_ref(), &spec.url, MimeCheck::WorkerScript).is_err() {
                load_error(&spec);
                false
            } else {
                match agent.run_module(&spec.url.serialize(false)) {
                    Ok(_) => true,
                    Err(Throw::Value(e)) => {
                        let name = match &e {
                            Value::Object(o) => match agent.get_str(*o, "name") {
                                Ok(Value::String(n)) => n.to_string_lossy(),
                                _ => String::new(),
                            },
                            _ => String::new(),
                        };
                        if name == "SyntaxError" {
                            load_error(&spec);
                            false
                        } else {
                            crate::dom::events::report_exception(agent, &e);
                            true
                        }
                    }
                    Err(Throw::Terminate(_)) => false,
                }
            }
        }
    };
    if !ok {
        return;
    }
    if let Err(Throw::Terminate(_)) = agent.perform_microtask_checkpoint() {
        return;
    }
    event_loop(agent, &spec.inbox, &spec.control);
}

fn load_error(spec: &WorkerSpec) {
    if let Some(p) = &spec.parent {
        p.push(Envelope::FromWorker {
            worker: spec.worker_id,
            event: WorkerEvent::LoadError,
        });
    }
    if let ScopeKind::Shared(entry) = &spec.kind {
        entry.load_failed();
    }
}

/// El bucle de eventos del worker: tareas, mensajes y temporizadores hasta `close()`,
/// `terminate()` o que el dueño se vaya.
fn event_loop(agent: &mut Agent, inbox: &Arc<AgentInbox>, control: &Arc<WorkerControl>) {
    loop {
        if control.terminated.load(Ordering::SeqCst) || closing(agent) {
            return;
        }
        let seen = agent.signal.seen();
        super::pump(agent);
        crate::net::pump(agent, agent.current_realm);
        match agent.run_one_task() {
            Ok(true) => continue,
            Ok(false) => {}
            Err(Throw::Terminate(_)) => return,
            Err(Throw::Value(e)) => crate::dom::events::report_exception(agent, &e),
        }
        if closing(agent) || control.terminated.load(Ordering::SeqCst) {
            return;
        }
        if !inbox.is_empty() {
            continue;
        }
        let now_ms = agent.host.hooks.now();
        let next = agent.next_wake();
        let busy_net = crate::net::pending_requests(agent, agent.current_realm) > 0;
        // Sin nada pendiente: el dueño ve al worker ocioso hasta el próximo mensaje.
        // Los workers anidados que trabajan también cuentan.
        let children = super::busy(agent);
        inbox.set_idle(next.is_none() && !busy_net && !children);
        let until = next
            .map(|t| Instant::now() + Duration::from_secs_f64(((t - now_ms).max(0.0)) / 1000.0));
        // Un SharedWorker sin conexiones y sin nada que hacer termina.
        if next.is_none() && !busy_net && shared_done(agent) {
            return;
        }
        agent.signal.wait(
            seen,
            until.or(Some(Instant::now() + Duration::from_secs(1))),
        );
        inbox.set_idle(false);
    }
}

/// ¿Es este agente un worker con el *closing flag* puesto (`close()`)?
pub(crate) fn closing(agent: &Agent) -> bool {
    agent
        .embedder_data::<WorkerHost>()
        .and_then(|h| h.scope.as_ref())
        .is_some_and(|s| s.closing)
}

fn shared_done(agent: &Agent) -> bool {
    agent
        .embedder_data::<WorkerHost>()
        .and_then(|h| h.scope.as_ref())
        .is_some_and(|s| matches!(&s.kind, ScopeKind::Shared(e) if e.orphaned()))
}

/// ¿Es un JavaScript MIME type (MIME Sniffing §4.6)? Solo la esencia, sin mayúsculas.
pub(crate) fn is_javascript_mime(content_type: &str) -> bool {
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    matches!(
        essence.as_str(),
        "application/ecmascript"
            | "application/javascript"
            | "application/x-ecmascript"
            | "application/x-javascript"
            | "text/ecmascript"
            | "text/javascript"
            | "text/javascript1.0"
            | "text/javascript1.1"
            | "text/javascript1.2"
            | "text/javascript1.3"
            | "text/javascript1.4"
            | "text/javascript1.5"
            | "text/jscript"
            | "text/livescript"
            | "text/x-ecmascript"
            | "text/x-javascript"
    )
}

/// Cuándo se exige un JavaScript MIME type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MimeCheck {
    /// El script principal de un worker: solo en HTTP(S) (`blob:` y `data:` cargan igual).
    WorkerScript,
    /// `importScripts`: siempre, también `data:` y `blob:` (whatwg/html#8869).
    Imported,
    /// Los módulos (el loader ya los trata como JavaScript).
    None,
}

/// Trae el texto de un script (estado 2xx y, según `check`, JavaScript MIME type); `Err`
/// es un error de red.
pub(crate) fn fetch_script(
    backend: &dyn NetBackend,
    url: &Url,
    check: MimeCheck,
) -> Result<String, String> {
    let req = NetRequest {
        url: url.clone(),
        options: bf_net::RequestOptions::default(),
    };
    let r = backend.fetch(&req)?;
    if !(200..300).contains(&r.status) {
        return Err(format!("{} {}", r.status, r.status_text));
    }
    let strict = match check {
        MimeCheck::WorkerScript => matches!(url.scheme.as_str(), "http" | "https"),
        MimeCheck::Imported => true,
        MimeCheck::None => false,
    };
    if strict {
        let ct = r.headers.get("content-type").unwrap_or_default();
        if !is_javascript_mime(&ct) {
            return Err(format!("tipo MIME no válido para un script: {ct:?}"));
        }
    }
    Ok(String::from_utf8_lossy(&r.body).into_owned())
}

/// Los módulos de un worker de tipo `module`, por la red del worker.
struct ModuleFetcher {
    backend: Arc<dyn NetBackend>,
}

impl bf_js::vm::module::ModuleLoader for ModuleFetcher {
    fn resolve(&self, specifier: &str, referrer: Option<&str>) -> Result<String, String> {
        let base = referrer.and_then(|r| Url::parse(r, None).ok());
        let is_relative = specifier.starts_with("./")
            || specifier.starts_with("../")
            || specifier.starts_with('/');
        if !is_relative && Url::parse(specifier, None).is_err() {
            return Err(format!("specifier inválido: {specifier}"));
        }
        Url::parse(specifier, base.as_ref())
            .map(|u| u.serialize(false))
            .map_err(|_| format!("specifier inválido: {specifier}"))
    }

    fn load(&self, url: &str) -> Result<String, String> {
        let u = Url::parse(url, None).map_err(|_| format!("URL inválida: {url}"))?;
        fetch_script(self.backend.as_ref(), &u, MimeCheck::None)
    }
}

// --- El global ---

/// Las clases del lado worker (una vez por agente).
#[derive(Debug, Clone, Copy)]
pub struct ScopeClasses {
    pub worker_global_scope: HostClassId,
    pub dedicated: HostClassId,
    pub shared: HostClassId,
    pub location: HostClassId,
    pub navigator: HostClassId,
    pub file_reader_sync: HostClassId,
}

/// Las interfaces del DOM que no están expuestas en los workers (`[Exposed=Window]`).
const WINDOW_ONLY: &[&str] = &[
    "Window",
    "Node",
    "Document",
    "HTMLDocument",
    "XMLDocument",
    "DOMParser",
    "AbstractRange",
    "Range",
    "StaticRange",
    "Selection",
    "DocumentFragment",
    "DocumentType",
    "Element",
    "CharacterData",
    "Text",
    "CDATASection",
    "Comment",
    "ProcessingInstruction",
    "Attr",
    "UIEvent",
    "FocusEvent",
    "MouseEvent",
    "KeyboardEvent",
    "InputEvent",
    "HashChangeEvent",
    "PopStateEvent",
    "PageTransitionEvent",
    "StorageEvent",
    "CompositionEvent",
    "WheelEvent",
    "AnimationEvent",
    "TransitionEvent",
    "TouchEvent",
    "NodeList",
    "HTMLCollection",
    "DOMTokenList",
    "NamedNodeMap",
    "MutationObserver",
    "MutationRecord",
    "NodeFilter",
    "NodeIterator",
    "TreeWalker",
    "DOMImplementation",
    "SharedWorker",
];

/// Instala el global del worker en `realm`.
fn install(
    agent: &mut Agent,
    realm: RealmId,
    url: &Url,
    kind: &ScopeKind,
    backend: Arc<dyn NetBackend>,
) {
    let doc = bf_dom::Document::new();
    agent.set_realm_data(
        realm,
        WebContext::new(DocumentOwner::Document(Box::new(doc)), url.clone()),
    );
    let h = WebContextHandle { realm };
    crate::dom::register_classes(agent, realm, &h);
    crate::net::register(agent, realm, &h);
    crate::net::set_backend(agent, realm, backend, false);
    super::register(agent, realm, &h);
    let classes = register_scope_classes(agent);
    let class = match kind {
        ScopeKind::Dedicated => classes.dedicated,
        ScopeKind::Shared(_) => classes.shared,
    };
    let g = agent.host_make_global(
        realm,
        class,
        Box::new(crate::dom::window::WindowData { realm }),
    );
    crate::dom::window::track_rejections(agent, realm);
    // IndexedDB (E2) también está expuesto en los workers.
    crate::storage::register(agent, realm, &h);
    let saved = agent.current_realm;
    agent.current_realm = realm;
    for name in WINDOW_ONLY {
        let _ = agent.delete(g, &PropertyKey::from(*name));
    }
    agent.current_realm = saved;
}

/// Registra (una vez) las clases del lado worker.
fn register_scope_classes(agent: &mut Agent) -> ScopeClasses {
    if let Some(c) = agent.embedder_data::<ScopeClasses>() {
        return *c;
    }
    let dom = crate::dom::classes(agent).clone();
    let wgs =
        agent.register_host_class(HostClass::new("WorkerGlobalScope").parent(dom.event_target));
    let fetch = crate::net::fetch::window_partial();
    let dedicated = agent.register_host_class(dedicated_scope_class(wgs));
    let shared = agent.register_host_class(shared_scope_class(wgs));
    agent.host_extend_class(dedicated, fetch);
    agent.host_extend_class(shared, crate::net::fetch::window_partial());
    let location = agent.register_host_class(location_class());
    let navigator = agent.register_host_class(navigator_class());
    let file_reader_sync = agent.register_host_class(file_reader_sync_class());
    let c = ScopeClasses {
        worker_global_scope: wgs,
        dedicated,
        shared,
        location,
        navigator,
        file_reader_sync,
    };
    agent.set_embedder_data(c);
    c
}

/// Los miembros de `WorkerGlobalScope` (§10.2.1), en la clase global concreta.
fn worker_scope_members(c: HostClass) -> HostClass {
    let c = c
        .global()
        .attribute("self", Some(scope_self), Some(replaceable_self))
        .attribute("location", Some(scope_location), None)
        .attribute("navigator", Some(scope_navigator), None)
        .attribute("isSecureContext", Some(secure_context), None)
        .attribute("crossOriginIsolated", Some(cross_origin_isolated), None)
        .attribute("origin", Some(scope_origin), None)
        .method("importScripts", 0, import_scripts);
    with_event_handlers(c, GLOBAL_EVENT_HANDLERS_WORKER)
}

fn dedicated_scope_class(wgs: HostClassId) -> HostClass {
    let c = HostClass::new("DedicatedWorkerGlobalScope")
        .parent(wgs)
        .attribute("name", Some(scope_name), Some(replaceable_name))
        .method("postMessage", 1, scope_post_message)
        .method("close", 0, scope_close);
    with_event_handlers(worker_scope_members(c), &["message", "messageerror"])
}

fn shared_scope_class(wgs: HostClassId) -> HostClass {
    let c = HostClass::new("SharedWorkerGlobalScope")
        .parent(wgs)
        .attribute("name", Some(scope_name), Some(replaceable_name))
        .method("close", 0, scope_close);
    with_event_handlers(worker_scope_members(c), &["connect"])
}

fn scope_state(agent: &Agent) -> Option<&ScopeState> {
    agent
        .embedder_data::<WorkerHost>()
        .and_then(|h| h.scope.as_ref())
}

fn global(agent: &Agent) -> Value {
    let realm = WebContextHandle::current(agent).realm;
    Value::Object(agent.realms[realm].global_object)
}

fn scope_self(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(global(agent))
}

/// `[Replaceable]` (WebIDL §3.7.6): el setter crea una propiedad de datos propia.
fn replace(agent: &mut Agent, a: &CallArgs<'_>, name: &str) -> JsResult<Value> {
    let this = match &a.this {
        Value::Object(o) => *o,
        _ => match global(agent) {
            Value::Object(g) => g,
            _ => return Ok(Value::Undefined),
        },
    };
    agent.create_data_property_or_throw(this, PropertyKey::from(name), a.arg(0))?;
    Ok(Value::Undefined)
}

fn replaceable_self(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    replace(agent, a, "self")
}

fn replaceable_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    replace(agent, a, "name")
}

fn scope_name(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(scope_state(agent).map_or("", |s| &s.name)))
}

fn secure_context(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    // "Potentially trustworthy" (Secure Contexts §3.1).
    let url = WebContextHandle::current(agent).get(agent).url.clone();
    let host = url.hostname();
    let secure = matches!(url.scheme.as_str(), "https" | "wss" | "file" | "about")
        || host == "localhost"
        || host.ends_with(".localhost")
        || host == "127.0.0.1"
        || host == "[::1]";
    Ok(Value::Boolean(secure))
}

fn cross_origin_isolated(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(
        scope_state(agent).is_some_and(|s| s.cross_origin_isolated),
    ))
}

fn scope_origin(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let o = crate::net::document_origin(agent);
    Ok(Value::str(&o))
}

/// `close()` (§10.2.1): descarta lo pendiente y termina después de la tarea en curso.
fn scope_close(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    if let Some(s) = agent
        .embedder_data_mut::<WorkerHost>()
        .and_then(|h| h.scope.as_mut())
    {
        s.closing = true;
    }
    Ok(Value::Undefined)
}

/// `postMessage(message, transfer)` del global de un worker dedicado (§10.2.2).
fn scope_post_message(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (parent, id, isolated) = match scope_state(agent) {
        Some(s) => (s.parent.clone(), s.worker_id, s.cross_origin_isolated),
        None => return Ok(Value::Undefined),
    };
    let transfer = super::ports::transfer_argument(agent, &a.arg(1))?;
    let s = super::ports::serialize(agent, &a.arg(0), &transfer, isolated)?;
    if let Some(p) = parent {
        p.push(Envelope::FromWorker {
            worker: id,
            event: WorkerEvent::Message(s),
        });
    }
    Ok(Value::Undefined)
}

/// Un mensaje del dueño: un `MessageEvent` `message` en el global (o `messageerror` si no se
/// puede deserializar).
pub(crate) fn deliver_message(agent: &mut Agent, s: Serialized) {
    if closing(agent) {
        return;
    }
    let t = TargetRef {
        doc: 0,
        target: EventTarget::Window,
    };
    match serialize::deserialize_with_transfer(agent, s) {
        Ok((data, transferred)) => {
            let ports = super::ports::ports_of(agent, &transferred);
            crate::net::channels::fire_message(agent, t, "message", data, "", "", ports);
        }
        Err(_) => {
            crate::net::channels::fire_message(
                agent,
                t,
                "messageerror",
                Value::Null,
                "",
                "",
                Vec::new(),
            );
        }
    }
}

/// `importScripts(...urls)` (§10.3.4): se traen todos y se ejecutan en orden.
fn import_scripts(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (script_type, base) = match scope_state(agent) {
        Some(s) => (
            s.script_type,
            WebContextHandle::current(agent).get(agent).base.clone(),
        ),
        None => return Ok(Value::Undefined),
    };
    if script_type == ScriptType::Module {
        return agent.type_error("Module scripts don't support importScripts().");
    }
    let mut urls = Vec::with_capacity(a.args.len());
    for v in a.args {
        let s = agent.to_string(v)?.to_string_lossy();
        match Url::parse(&s, Some(&base)) {
            Ok(u) => urls.push(u),
            Err(_) => {
                return crate::net::dom_error(
                    agent,
                    "SyntaxError",
                    &format!("Failed to execute 'importScripts': The URL '{s}' is invalid."),
                );
            }
        }
    }
    let backend = crate::net::current_backend(agent);
    let mut scripts = Vec::with_capacity(urls.len());
    for u in &urls {
        match fetch_script(backend.as_ref(), u, MimeCheck::Imported) {
            Ok(t) => scripts.push(t),
            Err(_) => {
                return crate::net::dom_error(
                    agent,
                    "NetworkError",
                    &format!(
                        "Failed to execute 'importScripts': The script at '{}' failed to load.",
                        u.serialize(false)
                    ),
                );
            }
        }
    }
    for (u, text) in urls.iter().zip(scripts) {
        agent.eval_script(&text, &u.serialize(false))?;
    }
    Ok(Value::Undefined)
}

// --- FileReaderSync (File API §7) ---

fn file_reader_sync_class() -> HostClass {
    HostClass::new("FileReaderSync")
        .constructor(frs_ctor, 0)
        .method("readAsArrayBuffer", 1, frs_array_buffer)
        .method("readAsBinaryString", 1, frs_binary_string)
        .method("readAsText", 1, frs_text)
        .method("readAsDataURL", 1, frs_data_url)
}

#[derive(Debug)]
struct FileReaderSyncData;

fn frs_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent
            .type_error("Failed to construct 'FileReaderSync': Please use the 'new' operator.");
    };
    let class = register_scope_classes(agent).file_reader_sync;
    let o = agent.host_create_from_new_target(nt, class, None, Box::new(FileReaderSyncData))?;
    Ok(Value::Object(o))
}

fn frs_blob(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    what: &str,
) -> JsResult<crate::net::blob::BlobData> {
    match crate::net::blob::data(agent, &a.arg(0)) {
        Some(d) => Ok(d),
        None => agent.type_error(&format!(
            "Failed to execute '{what}' on 'FileReaderSync': parameter 1 is not of type 'Blob'."
        )),
    }
}

fn frs_array_buffer(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = frs_blob(agent, a, "readAsArrayBuffer")?;
    let b = bf_js::builtins::web::encoding::new_array_buffer(agent, d.bytes.to_vec());
    Ok(Value::Object(b))
}

fn frs_binary_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = frs_blob(agent, a, "readAsBinaryString")?;
    let units: Vec<u16> = d.bytes.iter().map(|b| u16::from(*b)).collect();
    Ok(Value::String(bf_js::string::JsString::from_units(units)))
}

fn frs_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = frs_blob(agent, a, "readAsText")?;
    let bytes = d
        .bytes
        .strip_prefix(&[0xEF, 0xBB, 0xBF][..])
        .unwrap_or(&d.bytes);
    Ok(Value::str(&String::from_utf8_lossy(bytes)))
}

fn frs_data_url(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = frs_blob(agent, a, "readAsDataURL")?;
    let mime = if d.mime.is_empty() {
        "application/octet-stream".to_owned()
    } else {
        d.mime.clone()
    };
    let b64 = bf_js::builtins::web::base64::encode(&d.bytes);
    Ok(Value::str(&format!("data:{mime};base64,{b64}")))
}

// --- WorkerLocation (§10.3.3) y WorkerNavigator (§10.3.2) ---

fn location_class() -> HostClass {
    let mut c = HostClass::new("WorkerLocation").method("toString", 0, loc_href);
    for (name, f) in [
        ("href", loc_href as bf_js::vm::function::NativeFn),
        ("origin", loc_origin),
        ("protocol", loc_protocol),
        ("host", loc_host),
        ("hostname", loc_hostname),
        ("port", loc_port),
        ("pathname", loc_pathname),
        ("search", loc_search),
        ("hash", loc_hash),
    ] {
        c = c.attribute(name, Some(f), None);
    }
    c
}

fn scope_url(agent: &Agent) -> Url {
    scope_state(agent).map_or_else(
        || WebContextHandle::current(agent).get(agent).url.clone(),
        |s| s.url.clone(),
    )
}

fn scope_location(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let realm = WebContextHandle::current(agent).realm;
    let key = bf_js::host::HostKey(u64::MAX - 1);
    if let Some(o) = agent.host_lookup(realm, key) {
        return Ok(Value::Object(o));
    }
    let class = register_scope_classes(agent).location;
    Ok(Value::Object(agent.host_wrap(
        realm,
        key,
        class,
        Box::new(LocationData),
    )))
}

fn scope_navigator(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let realm = WebContextHandle::current(agent).realm;
    let key = bf_js::host::HostKey(u64::MAX - 2);
    if let Some(o) = agent.host_lookup(realm, key) {
        return Ok(Value::Object(o));
    }
    let class = register_scope_classes(agent).navigator;
    Ok(Value::Object(agent.host_wrap(
        realm,
        key,
        class,
        Box::new(NavigatorData),
    )))
}

#[derive(Debug)]
struct LocationData;

#[derive(Debug)]
struct NavigatorData;

fn loc_href(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&scope_url(agent).serialize(false)))
}

fn loc_origin(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&scope_url(agent).origin().to_string()))
}

fn loc_protocol(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&format!("{}:", scope_url(agent).scheme)))
}

fn loc_host(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let u = scope_url(agent);
    let host = u.host.as_ref().map(ToString::to_string).unwrap_or_default();
    Ok(Value::str(&match u.port {
        Some(p) => format!("{host}:{p}"),
        None => host,
    }))
}

fn loc_hostname(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let u = scope_url(agent);
    Ok(Value::str(
        &u.host.as_ref().map(ToString::to_string).unwrap_or_default(),
    ))
}

fn loc_port(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(
        &scope_url(agent)
            .port
            .map(|p| p.to_string())
            .unwrap_or_default(),
    ))
}

fn loc_pathname(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&scope_url(agent).path_string()))
}

fn loc_search(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let q = scope_url(agent).query.unwrap_or_default();
    Ok(Value::str(&if q.is_empty() { q } else { format!("?{q}") }))
}

fn loc_hash(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let f = scope_url(agent).fragment.unwrap_or_default();
    Ok(Value::str(&if f.is_empty() { f } else { format!("#{f}") }))
}

fn navigator_class() -> HostClass {
    let mut c = HostClass::new("WorkerNavigator");
    for (name, f) in [
        ("userAgent", nav_user_agent as bf_js::vm::function::NativeFn),
        ("appVersion", nav_app_version),
        ("appName", nav_app_name),
        ("appCodeName", nav_app_code_name),
        ("product", nav_product),
        ("platform", nav_platform),
        ("language", nav_language),
        ("languages", nav_languages),
        ("onLine", nav_online),
        ("hardwareConcurrency", nav_hardware_concurrency),
    ] {
        c = c.attribute(name, Some(f), None);
    }
    c
}

fn user_agent(agent: &Agent) -> String {
    scope_state(agent).map_or_else(
        || bf_infra::identity::USER_AGENT.to_owned(),
        |s| s.user_agent.clone(),
    )
}

fn nav_user_agent(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&user_agent(agent)))
}

/// `appVersion`: el User-Agent sin el `Mozilla/` del principio (HTML §8.9.1.1).
fn nav_app_version(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let ua = user_agent(agent);
    Ok(Value::str(ua.strip_prefix("Mozilla/").unwrap_or(&ua)))
}

fn nav_app_name(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str("Netscape"))
}

fn nav_app_code_name(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str("Mozilla"))
}

fn nav_product(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str("Gecko"))
}

fn nav_platform(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(bf_infra::identity::NAVIGATOR_PLATFORM))
}

fn nav_language(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str("en-US"))
}

fn nav_languages(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let a = agent.create_array_from_list(&[Value::str("en-US"), Value::str("en")])?;
    let _ = agent.set_integrity_level(a, true);
    Ok(Value::Object(a))
}

fn nav_online(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(true))
}

fn nav_hardware_concurrency(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let n = std::thread::available_parallelism().map_or(1, |n| n.get());
    Ok(Value::Number(n as f64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_locations() {
        assert_eq!(
            location_from_stack("Error: x\n    at f (http://a.test/w.js:3:7)\n"),
            Some(("http://a.test/w.js".into(), 3, 7))
        );
        assert_eq!(
            location_from_stack("    at http://a.test/w.js:10:2"),
            Some(("http://a.test/w.js".into(), 10, 2))
        );
        assert_eq!(location_from_stack(""), None);
    }
}
