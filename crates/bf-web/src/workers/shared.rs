//! `SharedWorker` (HTML §10.2.6.4), básico: un worker por (origen, URL del script, nombre)
//! en el proceso, que comparten todos los documentos de ese origen. Cada `new SharedWorker`
//! crea un par de puertos: el de afuera es su `port`, el de adentro llega al global del
//! worker en un evento `connect`. El worker termina cuando no le queda ningún documento
//! conectado y no tiene nada pendiente.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use bf_dom::events::EventTarget;
use bf_js::host::agents::AgentSignal;
use bf_js::host::{HostClass, HostClassId};
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use super::dedicated::{cross_origin_isolated, script_url, worker_options};
use super::ports::PortRecord;
use super::scope::{ScopeKind, ScriptType, WorkerSpec};
use super::{AgentInbox, Envelope, WorkerControl, host};
use crate::WebContextHandle;
use crate::dom::events::{TargetRef, with_event_handlers};

/// La clave de un `SharedWorker` en el registro: partición (`workers::set_partition`), origen,
/// URL del script y nombre. La partición hace de *storage key* (HTML §10.2.6.4 paso 11.2): dos
/// documentos de particiones distintas no comparten worker aunque coincidan en lo demás.
type SharedKey = (u64, String, String, String);

/// Un `SharedWorker` del proceso.
pub struct SharedEntry {
    key: SharedKey,
    script_type: ScriptType,
    credentials: String,
    pub(crate) inbox: Arc<AgentInbox>,
    pub(crate) control: Arc<WorkerControl>,
    /// Los objetos `SharedWorker` (de cualquier documento) que lo usan.
    clients: AtomicUsize,
    failed: AtomicBool,
    /// Los objetos `SharedWorker` conectados (su agente y su target), para avisarles si el
    /// script no carga.
    objects: Mutex<Vec<(Arc<AgentInbox>, u64)>>,
}

impl fmt::Debug for SharedEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SharedWorker({:?})", self.key)
    }
}

impl PartialEq for SharedEntry {
    fn eq(&self, other: &SharedEntry) -> bool {
        self.key == other.key
    }
}

impl Eq for SharedEntry {}

impl SharedEntry {
    /// ¿Está trabajando (para el runner)?
    pub(crate) fn busy(&self) -> bool {
        !self.control.closed.load(Ordering::SeqCst) && !self.inbox.is_idle()
    }

    /// ¿No le queda ningún documento conectado?
    pub(crate) fn orphaned(&self) -> bool {
        self.clients.load(Ordering::SeqCst) == 0
    }

    /// El script no cargó: un `error` en cada `SharedWorker` conectado.
    pub(crate) fn load_failed(&self) {
        self.failed.store(true, Ordering::SeqCst);
        for (inbox, id) in self
            .objects
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
        {
            inbox.push(Envelope::FromWorker {
                worker: *id,
                event: super::WorkerEvent::LoadError,
            });
        }
    }

    /// Un documento dejó de usarlo.
    pub(crate) fn release(&self) {
        let _ = self
            .clients
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |c| c.checked_sub(1));
        self.inbox.signal().signal();
    }
}

type Registry = Mutex<HashMap<SharedKey, Arc<SharedEntry>>>;

fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(Mutex::default)
}

pub fn shared_worker_class(event_target: HostClassId) -> HostClass {
    with_event_handlers(
        HostClass::new("SharedWorker")
            .parent(event_target)
            .constructor(shared_ctor, 1)
            .attribute("port", Some(shared_port), None),
        &["error"],
    )
}

/// `new SharedWorker(scriptURL, options)`.
fn shared_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent
            .type_error("Failed to construct 'SharedWorker': Please use the 'new' operator.");
    };
    if a.args.is_empty() {
        return agent.type_error(
            "Failed to construct 'SharedWorker': 1 argument required, but only 0 present.",
        );
    }
    let url = script_url(agent, &a.arg(0), "SharedWorker")?;
    let options = worker_options(agent, &a.arg(1), true)?;
    let class = super::classes(agent).shared_worker;
    let (t, o) = crate::net::xhr::new_target(agent, class, Some(nt))?;
    let id = match t.target {
        EventTarget::Object(id) => id,
        _ => 0,
    };
    let (outside, inside) = super::ports::new_pair_for_transfer(agent)?;
    if let Some(s) = agent.host_slots_mut(o) {
        *s = vec![outside];
    }
    let origin = crate::net::document_origin(agent);
    let partition = super::partition(agent);
    let key = (
        partition,
        origin,
        url.serialize(false),
        options.name.clone(),
    );
    let entry = {
        let mut r = registry().lock().unwrap_or_else(|e| e.into_inner());
        let alive = r.get(&key).filter(|e| {
            !e.control.closed.load(Ordering::SeqCst)
                && !e.control.terminated.load(Ordering::SeqCst)
                && !e.failed.load(Ordering::SeqCst)
        });
        match alive {
            // Las mismas opciones: se conecta; otras: `error` (§10.2.6.4 paso 11.4).
            Some(e)
                if e.script_type != options.script_type || e.credentials != options.credentials =>
            {
                let _ = e;
                None
            }
            Some(e) => {
                let e = e.clone();
                e.clients.fetch_add(1, Ordering::SeqCst);
                Some(e)
            }
            None => {
                let e = Arc::new(SharedEntry {
                    key: key.clone(),
                    script_type: options.script_type,
                    credentials: options.credentials.clone(),
                    inbox: Arc::new(AgentInbox::new(Arc::new(AgentSignal::default()))),
                    control: Arc::new(WorkerControl::default()),
                    clients: AtomicUsize::new(1),
                    failed: AtomicBool::new(false),
                    objects: Mutex::new(Vec::new()),
                });
                r.insert(key, e.clone());
                drop(r);
                let spec = WorkerSpec {
                    url,
                    script_type: options.script_type,
                    name: options.name,
                    kind: ScopeKind::Shared(e.clone()),
                    backend: crate::net::current_backend(agent),
                    parent: None,
                    worker_id: 0,
                    inbox: e.inbox.clone(),
                    control: e.control.clone(),
                    cross_origin_isolated: cross_origin_isolated(agent),
                    owner_coep: super::dedicated::embedder_policy(agent),
                    user_agent: agent.host.hooks.user_agent(),
                    partition,
                };
                if super::scope::spawn(spec).is_err() {
                    e.load_failed();
                    None
                } else {
                    Some(e)
                }
            }
        }
    };
    let Some(entry) = entry else {
        // El `error` llega en una tarea (el objeto ya está devuelto).
        let inbox = super::inbox(agent);
        let realm = WebContextHandle::current(agent).realm;
        host(agent).shared_errors.insert(id, realm);
        inbox.push(Envelope::FromWorker {
            worker: id,
            event: super::WorkerEvent::LoadError,
        });
        return Ok(Value::Object(o));
    };
    let inbox = super::inbox(agent);
    entry
        .objects
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((inbox, id));
    if entry.failed.load(Ordering::SeqCst) {
        entry.load_failed();
    }
    entry.inbox.push(Envelope::Connect { port: inside });
    host(agent).shared.insert(id, entry);
    Ok(Value::Object(o))
}

fn shared_port(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let class = super::classes(agent).shared_worker;
    if !agent.host_is_instance(&a.this, class) {
        return agent.type_error("Illegal invocation");
    }
    let Value::Object(o) = &a.this else {
        return agent.type_error("Illegal invocation");
    };
    Ok(agent
        .host_slots_mut(*o)
        .and_then(|s| s.first().cloned())
        .unwrap_or(Value::Null))
}

/// Una conexión nueva en el global del `SharedWorker`: un `MessageEvent` `connect` con el
/// puerto (§10.2.6.4 paso 11).
pub(crate) fn deliver_connect(agent: &mut Agent, port: PortRecord) {
    let p = super::ports::adopt_port(agent, port);
    let t = TargetRef {
        doc: 0,
        target: EventTarget::Window,
    };
    // `ports` es [puerto] (congelado) y `source` es el puerto.
    let ports = match agent.create_array_from_list(std::slice::from_ref(&p)) {
        Ok(a) => {
            let _ = agent.set_integrity_level(a, true);
            Value::Object(a)
        }
        Err(_) => Value::Null,
    };
    let ev =
        crate::net::channels::create_message_event(agent, "connect", Value::str(""), "", "", ports);
    if let Some(s) = agent.host_slots_mut(ev)
        && let Some(slot) = s.get_mut(6)
    {
        *slot = p;
    }
    crate::net::channels::dispatch(agent, t, ev);
}
