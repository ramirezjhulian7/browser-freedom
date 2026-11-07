//! Workers (HTML §10): `Worker` ([`dedicated`]), `SharedWorker` ([`shared`]), sus objetos
//! globales ([`scope`]) y los `MessagePort` entre hilos ([`ports`]). **Dueño: E3.**
//!
//! # Un agente por hilo
//!
//! Cada worker es un agente propio (ECMA-262 §9.7; HTML §8.1.2 "dedicated worker agent") en su
//! propio hilo, con su `Runtime`, su realm y su bucle de eventos: los agentes no comparten
//! objetos. Lo único compartido es la memoria de los `SharedArrayBuffer`
//! (`bf_js::host::agents`) y lo que cruza en mensajes.
//!
//! # Mensajes entre hilos
//!
//! Cada agente que tiene workers (o es uno) tiene una [`AgentInbox`]: una cola de
//! [`Envelope`] que los otros hilos llenan y que despierta al agente con su `AgentSignal`
//! (que a su vez llama al *waker* del embebedor, si hay). Los valores viajan serializados
//! (`bf_js::builtins::web::serialize`: StructuredSerializeWithTransfer en el que manda,
//! StructuredDeserializeWithTransfer en el que recibe).
//!
//! El agente vacía su bandeja con [`pump`], que encola una tarea de la fuente *posted
//! message* por sobre. Quién llama a `pump`:
//!
//! - **El shell**: `bf_web::net::pump` (que el bucle de la pestaña ya llama cuando lo despierta
//!   el waker de la red) llama también a [`pump`]; el waker de la red es el del agente.
//! - **El runner de WPT** ([`set_manual_pump`]): llama a [`pump`] en su bucle y, mientras haya
//!   workers trabajando ([`busy`]), espera sus mensajes en tiempo real ([`wait_for_events`])
//!   en vez de adelantar su reloj virtual.
//! - **Sin waker** (tests, `bf-js`): un temporizador de 4 ms vacía la bandeja mientras haya
//!   workers vivos.
//! - **El propio worker**: su bucle (en [`scope`]) vacía su bandeja antes de cada tarea.

pub mod dedicated;
pub mod isolation;
pub mod ports;
pub mod scope;
pub mod shared;

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use bf_js::builtins::web::serialize::Serialized;
use bf_js::host::agents::AgentSignal;
use bf_js::host::{ConsoleLevel, HostHandle, TaskSource, TimerId};
use bf_js::value::Value;
use bf_js::vm::{Agent, JsResult, RealmId};

/// Lo que llega a un agente desde otro hilo.
pub enum Envelope {
    /// Al global de un worker: un mensaje del objeto `Worker` de su dueño.
    ToScope(Serialized),
    /// Al dueño de un worker: algo del worker cuyo objeto `Worker` es el target `worker`.
    FromWorker { worker: u64, event: WorkerEvent },
    /// A un `MessagePort` de este agente (el id local de su target).
    Port { local: u64, message: Serialized },
    /// El puerto entrelazado con `local` se cerró.
    PortClosed { local: u64 },
    /// A un `SharedWorker`: una conexión nueva (el puerto del lado del worker).
    Connect { port: ports::PortRecord },
    /// A un `BroadcastChannel` de este agente (el id local de su target), desde un canal de
    /// otro agente con el origen `origin` (HTML §9.5).
    Broadcast {
        local: u64,
        origin: String,
        message: Serialized,
    },
}

/// Un evento de un worker para su dueño.
pub enum WorkerEvent {
    Message(Serialized),
    /// El mensaje no se pudo deserializar en el worker (`messageerror` en el dueño no; es en
    /// el worker: este lo manda el dueño). Aquí: el dueño no pudo deserializar.
    MessageError,
    /// Una excepción sin atrapar que el global del worker no canceló.
    Error {
        message: String,
        filename: String,
        lineno: u32,
        colno: u32,
    },
    /// No se pudo cargar (o compilar) el script del worker: un `Event` `error` simple.
    LoadError,
    Console(ConsoleLevel, String),
    /// El hilo del worker terminó.
    Closed,
}

/// La bandeja de un agente.
pub struct AgentInbox {
    queue: Mutex<VecDeque<Envelope>>,
    signal: Arc<AgentSignal>,
    /// El agente espera sin nada pendiente (lo pone su bucle; cada sobre lo limpia).
    idle: AtomicBool,
}

impl AgentInbox {
    pub fn new(signal: Arc<AgentSignal>) -> AgentInbox {
        AgentInbox {
            queue: Mutex::new(VecDeque::new()),
            signal,
            idle: AtomicBool::new(false),
        }
    }

    /// Deja un sobre y despierta al agente (que deja de estar ocioso).
    pub fn push(&self, e: Envelope) {
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(e);
        self.idle.store(false, Ordering::SeqCst);
        self.signal.signal();
    }

    /// El agente se declara ocioso (sin carrera con un sobre que llega a la vez: se vuelve
    /// a mirar la cola después de marcar).
    pub fn set_idle(&self, idle: bool) {
        self.idle.store(idle, Ordering::SeqCst);
        if idle && !self.is_empty() {
            self.idle.store(false, Ordering::SeqCst);
        }
    }

    /// ¿Está ocioso el agente de esta bandeja?
    pub fn is_idle(&self) -> bool {
        self.idle.load(Ordering::SeqCst)
    }

    pub fn is_empty(&self) -> bool {
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    }

    fn drain(&self) -> Vec<Envelope> {
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect()
    }

    /// La señal del agente dueño de la bandeja.
    pub fn signal(&self) -> &Arc<AgentSignal> {
        &self.signal
    }
}

/// El control de un worker, compartido entre su dueño y su hilo.
#[derive(Default)]
pub struct WorkerControl {
    /// `terminate()` (o el dueño se fue): el hilo termina en cuanto puede.
    pub terminated: AtomicBool,
    /// El hilo del worker ya terminó.
    pub closed: AtomicBool,
    /// La interrupción del agente del worker (para cortar un script que no termina).
    pub interrupt: Arc<AtomicBool>,
}

/// Un worker creado por este agente (el lado del dueño).
pub(crate) struct WorkerRecord {
    pub realm: RealmId,
    /// La bandeja del agente del worker.
    pub inbox: Arc<AgentInbox>,
    pub control: Arc<WorkerControl>,
    /// El objeto `Worker` (fijado mientras el worker vive: recibe sus eventos).
    pub pin: Option<HostHandle>,
}

/// El estado de workers de un agente (dato del embebedor).
pub struct WorkerHost {
    pub inbox: Arc<AgentInbox>,
    pub(crate) workers: HashMap<u64, WorkerRecord>,
    manual: bool,
    poll: Option<(RealmId, TimerId)>,
    /// Si este agente es un worker: su lado.
    pub(crate) scope: Option<scope::ScopeState>,
    /// Los `SharedWorker` conectados desde este agente: id del target → nombre de registro.
    pub(crate) shared: HashMap<u64, Arc<shared::SharedEntry>>,
    /// Los `SharedWorker` que no se pudieron conectar (su `error` está en camino) y su realm.
    pub(crate) shared_errors: HashMap<u64, RealmId>,
    /// La partición del proceso a la que pertenece el agente (ver [`set_partition`]).
    pub(crate) partition: u64,
}

/// El estado de workers del agente (se crea la primera vez).
pub fn host(agent: &mut Agent) -> &mut WorkerHost {
    if agent.embedder_data::<WorkerHost>().is_none() {
        let inbox = Arc::new(AgentInbox::new(agent.signal.clone()));
        agent.set_embedder_data(WorkerHost {
            inbox,
            workers: HashMap::new(),
            manual: false,
            poll: None,
            scope: None,
            shared: HashMap::new(),
            shared_errors: HashMap::new(),
            partition: 0,
        });
    }
    agent.embedder_data_mut::<WorkerHost>().expect("WorkerHost")
}

/// La bandeja del agente.
pub fn inbox(agent: &mut Agent) -> Arc<AgentInbox> {
    host(agent).inbox.clone()
}

/// Pone el agente en la partición `p` del proceso. Los `BroadcastChannel` solo se hablan
/// dentro de una partición, y los workers heredan la de su dueño. Por defecto todos los
/// agentes están en la 0 (las pestañas del shell se ven entre sí); el runner de WPT y los
/// tests, que corren documentos en paralelo con el mismo origen, dan a cada documento una
/// partición propia ([`new_partition`]) para que no se crucen sus mensajes.
pub fn set_partition(agent: &mut Agent, p: u64) {
    host(agent).partition = p;
}

/// Una partición nueva (nunca la 0).
pub fn new_partition() -> u64 {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    N.fetch_add(1, Ordering::SeqCst)
}

/// La partición del agente.
pub(crate) fn partition(agent: &Agent) -> u64 {
    agent
        .embedder_data::<WorkerHost>()
        .map_or(0, |h| h.partition)
}

/// El embebedor vacía la bandeja él mismo (el runner de WPT): sin temporizador de sondeo.
pub fn set_manual_pump(agent: &mut Agent) {
    host(agent).manual = true;
}

/// ¿Hay workers de este agente trabajando (o mensajes suyos sin repartir)? El runner espera
/// sus mensajes en tiempo real en vez de adelantar el reloj.
pub fn busy(agent: &Agent) -> bool {
    let Some(h) = agent.embedder_data::<WorkerHost>() else {
        return false;
    };
    !h.inbox.is_empty()
        || h.workers
            .values()
            .any(|w| !w.control.closed.load(Ordering::SeqCst) && !w.inbox.is_idle())
        || h.shared.values().any(|s| s.busy())
}

/// Espera (en tiempo real) un sobre o un aviso hasta `until`; `true` si llegó algo.
pub fn wait_for_events(agent: &Agent, until: Instant) -> bool {
    let seen = agent.signal.seen();
    if agent
        .embedder_data::<WorkerHost>()
        .is_some_and(|h| !h.inbox.is_empty())
    {
        return true;
    }
    agent.signal.wait(seen, Some(until))
}

/// Encola una tarea *posted message* por cada sobre llegado; cuántas encoló.
pub fn pump(agent: &mut Agent) -> usize {
    let Some(h) = agent.embedder_data::<WorkerHost>() else {
        return 0;
    };
    let envelopes = h.inbox.drain();
    let n = envelopes.len();
    for e in envelopes {
        agent.queue_task(
            TaskSource::PostedMessage,
            Box::new(move |agent: &mut Agent| handle(agent, e)),
        );
    }
    n
}

/// Reparte un sobre.
fn handle(agent: &mut Agent, e: Envelope) {
    match e {
        Envelope::ToScope(s) => scope::deliver_message(agent, s),
        Envelope::FromWorker { worker, event } => dedicated::deliver_event(agent, worker, event),
        Envelope::Port { local, message } => ports::deliver(agent, local, message),
        Envelope::PortClosed { local } => ports::peer_closed(agent, local),
        Envelope::Connect { port } => shared::deliver_connect(agent, port),
        Envelope::Broadcast {
            local,
            origin,
            message,
        } => ports::deliver_broadcast(agent, local, origin, message),
    }
}

/// Sin waker ni bombeo manual, un temporizador vacía la bandeja mientras haya workers.
pub(crate) fn ensure_polling(agent: &mut Agent, realm: RealmId) {
    // El waker de la red despierta también a los workers (el shell llama a `net::pump`).
    if let Some(w) = crate::net::waker(agent, realm) {
        agent.signal.set_waker(Some(w));
        return;
    }
    let h = host(agent);
    if h.manual || h.poll.is_some() || h.scope.is_some() {
        return;
    }
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let f = agent.create_builtin_function(poll_tick, 0, "", false);
    agent.current_realm = saved;
    let t = agent.set_timer(
        realm,
        bf_js::host::TimerHandler::Callback(Value::Object(f)),
        4.0,
        Vec::new(),
        true,
    );
    host(agent).poll = Some((realm, t));
}

fn poll_tick(agent: &mut Agent, _: &bf_js::vm::function::CallArgs<'_>) -> JsResult<Value> {
    pump(agent);
    let h = host(agent);
    let alive = h
        .workers
        .values()
        .any(|w| !w.control.closed.load(Ordering::SeqCst))
        || !h.shared.is_empty()
        || ports::has_remote_ports(agent);
    if !alive && let Some((_, t)) = host(agent).poll.take() {
        agent.clear_timer(t);
    }
    Ok(Value::Undefined)
}

impl Drop for WorkerHost {
    /// El agente se va (la pestaña se cerró, el documento de test terminó): sus workers
    /// dedicados terminan y sus `SharedWorker` pierden un cliente.
    fn drop(&mut self) {
        for w in self.workers.values() {
            dedicated::terminate(w);
        }
        for e in self.shared.values() {
            e.release();
        }
    }
}

/// ¿El documento es `crossOriginIsolated`? (Dato del realm que pone el embebedor; sin él,
/// no: los `SharedArrayBuffer` no cruzan a los workers.)
#[derive(Debug, Clone, Copy, Default)]
pub struct Isolation(pub bool);

/// Las clases de este módulo (una vez por agente).
#[derive(Debug, Clone, Copy)]
pub struct WorkerClasses {
    pub worker: bf_js::host::HostClassId,
    pub shared_worker: bf_js::host::HostClassId,
}

pub fn classes(agent: &Agent) -> WorkerClasses {
    *agent
        .embedder_data::<WorkerClasses>()
        .expect("workers::register")
}

/// Registra `Worker` y `SharedWorker` (en una ventana y en los workers, que pueden crear
/// workers anidados).
pub fn register(agent: &mut Agent, realm: RealmId, _ctx: &crate::WebContextHandle) {
    if agent.embedder_data::<WorkerClasses>().is_none() {
        let et = crate::dom::classes(agent).event_target;
        let worker = agent.register_host_class(dedicated::worker_class(et));
        let shared_worker = agent.register_host_class(shared::shared_worker_class(et));
        agent.set_embedder_data(WorkerClasses {
            worker,
            shared_worker,
        });
    }
    // `structuredClone` de la plataforma (con `Blob`, `File` y `MessagePort`).
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let g = agent.realms[realm].global_object;
    bf_js::builtins::method(agent, g, "structuredClone", 1, ports::structured_clone);
    agent.current_realm = saved;
}
