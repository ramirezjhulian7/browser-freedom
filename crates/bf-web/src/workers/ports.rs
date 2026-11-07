//! `MessagePort` entre agentes (HTML §9.4.3–9.4.4) y la serialización de los objetos de la
//! plataforma (`Blob`, `File`, `MessagePort`) para los mensajes entre hilos.
//!
//! Los puertos de E2 (`net::channels`) viven en un agente. Cuando un puerto se transfiere a
//! otro agente, él y su puerto entrelazado pasan a tener un **id global** en un registro del
//! proceso que dice en qué agente (su bandeja) y con qué id local vive cada uno. Un puerto
//! cuyo compañero está en otro agente es "remoto": `postMessage` serializa el mensaje y lo
//! deja en la bandeja del agente del compañero, que lo encola en el puerto como cualquier
//! mensaje local. Mientras un puerto está en tránsito (serializado pero todavía sin
//! deserializar), lo que le mandan queda pendiente en el registro y se entrega al llegar; si se
//! transfiere otra vez, su id global se mantiene y el registro cambia de agente.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use bf_js::builtins::web::serialize::{self, HostCloner, HostRecord, Options, Serialized};
use bf_js::heap::Gc;
use bf_js::object::Object;
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::{Agent, JsResult};

use super::{AgentInbox, Envelope};
use crate::dom::events::TargetRef;
use crate::net::blob::BlobData;

// --- El registro global ---

enum Slot {
    /// El puerto vive en el agente de `inbox` con el id local `local`.
    Bound { inbox: Arc<AgentInbox>, local: u64 },
    /// En tránsito: los mensajes que le llegaron mientras tanto.
    Pending(Vec<Serialized>),
}

fn registry() -> &'static Mutex<HashMap<u64, Slot>> {
    static R: OnceLock<Mutex<HashMap<u64, Slot>>> = OnceLock::new();
    R.get_or_init(Mutex::default)
}

fn next_gid() -> u64 {
    static N: AtomicU64 = AtomicU64::new(1);
    N.fetch_add(1, Ordering::SeqCst)
}

/// Manda un mensaje al puerto global `gid` (o lo deja pendiente si está en tránsito).
fn send_to(gid: u64, message: Serialized) {
    let mut r = registry().lock().unwrap_or_else(|e| e.into_inner());
    match r.get_mut(&gid) {
        Some(Slot::Bound { inbox, local }) => inbox.push(Envelope::Port {
            local: *local,
            message,
        }),
        Some(Slot::Pending(v)) => v.push(message),
        None => {}
    }
}

/// Avisa al puerto global `gid` que su compañero se cerró.
fn send_closed(gid: u64) {
    let r = registry().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(Slot::Bound { inbox, local }) = r.get(&gid) {
        inbox.push(Envelope::PortClosed { local: *local });
    }
}

// --- El estado de puertos remotos de un agente ---

/// Los puertos de este agente con id global (dato del embebedor).
#[derive(Default)]
struct PortHost {
    /// id local → id global.
    gids: HashMap<u64, u64>,
    /// id local → id global del compañero en otro agente.
    remote: HashMap<u64, u64>,
    /// Puertos que se fueron a otro agente: id local → id global (para reenviar lo que
    /// llegue tarde).
    moved: HashMap<u64, u64>,
}

fn port_host(agent: &mut Agent) -> &mut PortHost {
    if agent.embedder_data::<PortHost>().is_none() {
        agent.set_embedder_data(PortHost::default());
    }
    agent.embedder_data_mut::<PortHost>().expect("PortHost")
}

/// ¿Hay puertos de este agente entrelazados con otro agente?
pub fn has_remote_ports(agent: &Agent) -> bool {
    agent
        .embedder_data::<PortHost>()
        .is_some_and(|p| !p.remote.is_empty())
}

/// El id global del puerto local `local` (lo crea y lo registra aquí si no tiene).
fn gid_of(agent: &mut Agent, local: u64) -> u64 {
    if let Some(&g) = port_host(agent).gids.get(&local) {
        return g;
    }
    let g = next_gid();
    let inbox = super::inbox(agent);
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(g, Slot::Bound { inbox, local });
    port_host(agent).gids.insert(local, g);
    g
}

/// El id local del target de un `MessagePort`.
pub fn port_id(agent: &Agent, v: &Value) -> Option<u64> {
    let class = crate::net::classes(agent).message_port;
    if !agent.host_is_instance(v, class) {
        return None;
    }
    agent
        .host_unwrap::<TargetRef>(v)
        .and_then(|t| match t.target {
            bf_dom::events::EventTarget::Object(id) => Some(id),
            _ => None,
        })
}

/// Los `MessagePort` de una lista de objetos transferidos (los `ports` del `MessageEvent`).
pub fn ports_of(agent: &Agent, transferred: &[Value]) -> Vec<Value> {
    transferred
        .iter()
        .filter(|v| port_id(agent, v).is_some())
        .cloned()
        .collect()
}

/// `postMessage` de un puerto entrelazado con otro agente: serializa y manda. `false` si el
/// puerto no es remoto (sigue el camino local de `net::channels`).
pub(crate) fn post_if_remote(
    agent: &mut Agent,
    local: u64,
    message: &Value,
    transfer: &Value,
) -> JsResult<bool> {
    let Some(&peer) = agent
        .embedder_data::<PortHost>()
        .and_then(|p| p.remote.get(&local))
    else {
        return Ok(false);
    };
    let list = transfer_argument(agent, transfer)?;
    for &o in &list {
        if port_id(agent, &Value::Object(o)) == Some(local) {
            return crate::net::dom_error(
                agent,
                "DataCloneError",
                "A MessagePort cannot transfer itself.",
            );
        }
    }
    let s = serialize(agent, message, &list, false)?;
    send_to(peer, s);
    Ok(true)
}

/// `close()` de un puerto: si su compañero está en otro agente, le avisa.
pub(crate) fn close_if_remote(agent: &mut Agent, local: u64) {
    let Some(p) = agent.embedder_data_mut::<PortHost>() else {
        return;
    };
    let peer = p.remote.remove(&local);
    if let Some(g) = p.gids.remove(&local) {
        registry()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&g);
    }
    if let Some(peer) = peer {
        send_closed(peer);
    }
}

/// Un mensaje llegado al puerto local `local` desde otro agente.
pub(crate) fn deliver(agent: &mut Agent, local: u64, message: Serialized) {
    // Si el puerto se fue a otro agente, se reenvía.
    if let Some(&g) = agent
        .embedder_data::<PortHost>()
        .and_then(|p| p.moved.get(&local))
    {
        send_to(g, message);
        return;
    }
    match serialize::deserialize_with_transfer(agent, message) {
        Ok((data, transferred)) => {
            let ports = ports_of(agent, &transferred);
            crate::net::channels::enqueue_port_message(agent, local, data, ports);
        }
        Err(_) => crate::net::channels::port_message_error(agent, local),
    }
}

/// El compañero de `local` (en otro agente) se cerró.
pub(crate) fn peer_closed(agent: &mut Agent, local: u64) {
    if let Some(p) = agent.embedder_data_mut::<PortHost>() {
        p.remote.remove(&local);
    }
}

// --- Serialización ---

/// `transfer` de `postMessage`: una secuencia o `StructuredSerializeOptions { transfer }`.
pub fn transfer_argument(agent: &mut Agent, transfer: &Value) -> JsResult<Vec<Gc<Object>>> {
    let list = match transfer {
        Value::Undefined | Value::Null => return Ok(Vec::new()),
        Value::Object(o) => {
            let k = PropertyKey::Symbol(agent.well_known.iterator);
            let m = agent.get(*o, &k, transfer.clone())?;
            if agent.is_callable(&m) {
                transfer.clone()
            } else {
                agent.get(*o, &PropertyKey::from("transfer"), transfer.clone())?
            }
        }
        _ => {
            return agent.type_error(
                "Failed to execute 'postMessage': The provided value cannot be converted to a sequence.",
            );
        }
    };
    if list.is_undefined() {
        return Ok(Vec::new());
    }
    let items = agent.iterable_to_list(&list)?;
    let mut out = Vec::with_capacity(items.len());
    for v in items {
        match v {
            Value::Object(o) => out.push(o),
            _ => {
                return agent.type_error(
                    "Failed to execute 'postMessage': Value at index 0 does not have a transferable type.",
                );
            }
        }
    }
    Ok(out)
}

/// StructuredSerializeWithTransfer con los objetos de la plataforma.
pub fn serialize(
    agent: &mut Agent,
    value: &Value,
    transfer: &[Gc<Object>],
    allow_shared: bool,
) -> JsResult<Serialized> {
    match serialize::serialize_with_transfer(
        agent,
        value,
        transfer,
        &mut WebCloner,
        Options { allow_shared },
    ) {
        // El `DOMException` del motor pasa a ser el de la plataforma.
        Err(bf_js::vm::Throw::Value(Value::Object(e))) if matches!(agent.get_str(e, "name"), Ok(Value::String(n)) if n.to_string_lossy() == "DataCloneError") =>
        {
            let msg = match agent.get_str(e, "message") {
                Ok(Value::String(m)) => m.to_string_lossy(),
                _ => String::new(),
            };
            crate::net::dom_error(agent, "DataCloneError", &msg)
        }
        r => r,
    }
}

/// StructuredSerializeWithTransfer seguido de StructuredDeserializeWithTransfer en el mismo
/// agente (el `structuredClone` de la plataforma y los mensajes locales).
pub fn clone_within_agent(
    agent: &mut Agent,
    value: &Value,
    transfer: &[Gc<Object>],
) -> JsResult<Value> {
    let isolated = super::dedicated::cross_origin_isolated(agent);
    let s = serialize(agent, value, transfer, isolated)?;
    Ok(serialize::deserialize_with_transfer(agent, s)?.0)
}

/// `structuredClone(value, { transfer })` (HTML §2.7.10) con los objetos de la plataforma.
pub(crate) fn structured_clone(
    agent: &mut Agent,
    a: &bf_js::vm::function::CallArgs<'_>,
) -> JsResult<Value> {
    if a.args.is_empty() {
        return agent.type_error(
            "Failed to execute 'structuredClone' on 'Window': 1 argument required, but only 0 present.",
        );
    }
    let transfer = match &a.arg(1) {
        Value::Undefined | Value::Null => Vec::new(),
        Value::Object(o) => {
            let o = *o;
            let t = agent.get(o, &PropertyKey::from("transfer"), Value::Object(o))?;
            if t.is_undefined() {
                Vec::new()
            } else {
                let items = agent.iterable_to_list(&t)?;
                let mut out = Vec::with_capacity(items.len());
                for v in items {
                    match v {
                        Value::Object(o) => out.push(o),
                        _ => {
                            return agent.type_error("Value in the transfer list is not an object");
                        }
                    }
                }
                out
            }
        }
        _ => {
            return agent
                .type_error("The provided value is not of type 'StructuredSerializeOptions'");
        }
    };
    clone_within_agent(agent, &a.arg(0), &transfer)
}

/// Los objetos de la plataforma serializables (`Blob`, `File`) y transferibles
/// (`MessagePort`).
pub struct WebCloner;

/// Un `Blob`/`File` serializado.
struct BlobRecord(BlobData);

impl HostRecord for BlobRecord {
    fn deserialize(self: Box<Self>, agent: &mut Agent) -> JsResult<Value> {
        Ok(crate::net::blob::create(agent, self.0))
    }
}

/// Un `MessagePort` transferido.
pub struct PortRecord {
    gid: u64,
    /// El id global del compañero (`None`: sin entrelazar).
    peer: Option<u64>,
    /// Los mensajes que esperaban a `start()`.
    queued: Vec<Serialized>,
}

impl HostRecord for PortRecord {
    fn deserialize(self: Box<Self>, agent: &mut Agent) -> JsResult<Value> {
        Ok(adopt_port(agent, *self))
    }
}

/// Crea el puerto transferido en este agente y lo registra (lo pendiente se entrega).
pub(crate) fn adopt_port(agent: &mut Agent, rec: PortRecord) -> Value {
    let (local, o) = match crate::net::channels::new_port(agent) {
        Ok(p) => p,
        Err(_) => return Value::Null,
    };
    let inbox = super::inbox(agent);
    let pending = {
        let mut r = registry().lock().unwrap_or_else(|e| e.into_inner());
        let old = r.insert(
            rec.gid,
            Slot::Bound {
                inbox: inbox.clone(),
                local,
            },
        );
        match old {
            Some(Slot::Pending(v)) => v,
            _ => Vec::new(),
        }
    };
    let ph = port_host(agent);
    ph.gids.insert(local, rec.gid);
    if let Some(peer) = rec.peer {
        ph.remote.insert(local, peer);
    }
    for m in rec.queued.into_iter().chain(pending) {
        inbox.push(Envelope::Port { local, message: m });
    }
    let realm = agent.current_realm;
    super::ensure_polling(agent, realm);
    Value::Object(o)
}

impl HostCloner for WebCloner {
    fn serialize(
        &mut self,
        agent: &mut Agent,
        o: Gc<Object>,
    ) -> Option<JsResult<Box<dyn HostRecord>>> {
        let v = Value::Object(o);
        if let Some(d) = crate::net::blob::data(agent, &v) {
            return Some(Ok(Box::new(BlobRecord(d))));
        }
        if port_id(agent, &v).is_some() {
            return Some(crate::net::dom_error(
                agent,
                "DataCloneError",
                "A MessagePort could not be cloned because it was not transferred.",
            ));
        }
        None
    }

    fn is_transferable(&self, agent: &Agent, o: Gc<Object>) -> bool {
        port_id(agent, &Value::Object(o))
            .is_some_and(|id| !crate::net::channels::is_detached(agent, id))
    }

    fn transfer(&mut self, agent: &mut Agent, o: Gc<Object>) -> JsResult<Box<dyn HostRecord>> {
        let local = port_id(agent, &Value::Object(o)).expect("MessagePort");
        Ok(Box::new(transfer_port(agent, local)?))
    }
}

/// Los pasos de transferencia de un `MessagePort` (§9.4.3): el registro con su id global y
/// el de su compañero; el puerto de aquí queda inutilizado.
pub(crate) fn transfer_port(agent: &mut Agent, local: u64) -> JsResult<PortRecord> {
    let gid = gid_of(agent, local);
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(gid, Slot::Pending(Vec::new()));
    let (entangled, queue) = crate::net::channels::neuter_port(agent, local);
    let peer = match port_host(agent).remote.remove(&local) {
        Some(p) => Some(p),
        None => match entangled {
            Some(q) => {
                // El compañero se queda aquí: pasa a ser remoto respecto de este puerto.
                let qg = gid_of(agent, q);
                crate::net::channels::disentangle(agent, q);
                port_host(agent).remote.insert(q, gid);
                Some(qg)
            }
            None => None,
        },
    };
    let mut queued = Vec::with_capacity(queue.len());
    for (data, _) in queue {
        if let Ok(s) = serialize(agent, &data, &[], false) {
            queued.push(s);
        }
    }
    let ph = port_host(agent);
    ph.gids.remove(&local);
    ph.moved.insert(local, gid);
    Ok(PortRecord { gid, peer, queued })
}

/// Un par de puertos para un `SharedWorker`: el de afuera (un `MessagePort` de este agente) y
/// el registro del de adentro, que se manda al worker.
pub(crate) fn new_pair_for_transfer(agent: &mut Agent) -> JsResult<(Value, PortRecord)> {
    let (outside, o) = crate::net::channels::new_port(agent)?;
    let og = gid_of(agent, outside);
    let inside = next_gid();
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(inside, Slot::Pending(Vec::new()));
    port_host(agent).remote.insert(outside, inside);
    let realm = agent.current_realm;
    super::ensure_polling(agent, realm);
    Ok((
        Value::Object(o),
        PortRecord {
            gid: inside,
            peer: Some(og),
            queued: Vec::new(),
        },
    ))
}

// --- BroadcastChannel entre agentes (HTML §9.5) ---
//
// Los `BroadcastChannel` de `net::channels` se entregan en su propio agente. Para llegar a los
// de otros agentes (workers y pestañas), cada canal abierto con origen tupla se apunta en un
// registro del proceso, en orden de creación: (id global, partición, origen, nombre, bandeja
// de su agente, id local). `postMessage` serializa (el `DataCloneError` corta todo, §9.5
// paso 4) y deja un `Envelope::Broadcast` en la bandeja de cada destino de otro agente; los
// del mismo agente siguen el camino local. Los orígenes opacos no se apuntan: un origen opaco
// solo es igual a sí mismo y no cruza de agente.

struct BroadcastEntry {
    gid: u64,
    /// La partición del agente (`workers::set_partition`).
    partition: u64,
    origin: String,
    name: String,
    inbox: Arc<AgentInbox>,
    local: u64,
}

fn broadcast_registry() -> &'static Mutex<Vec<BroadcastEntry>> {
    static R: OnceLock<Mutex<Vec<BroadcastEntry>>> = OnceLock::new();
    R.get_or_init(Mutex::default)
}

/// Los `BroadcastChannel` de este agente apuntados en el registro (dato del embebedor):
/// id local → id global. Cuando el agente se va, sus entradas salen del registro.
#[derive(Default)]
struct BroadcastHost {
    open: HashMap<u64, u64>,
}

impl Drop for BroadcastHost {
    fn drop(&mut self) {
        if self.open.is_empty() {
            return;
        }
        let gids: Vec<u64> = self.open.values().copied().collect();
        broadcast_registry()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|b| !gids.contains(&b.gid));
    }
}

fn broadcast_host(agent: &mut Agent) -> &mut BroadcastHost {
    if agent.embedder_data::<BroadcastHost>().is_none() {
        agent.set_embedder_data(BroadcastHost::default());
    }
    agent
        .embedder_data_mut::<BroadcastHost>()
        .expect("BroadcastHost")
}

/// El constructor de `BroadcastChannel` (§9.5): apunta el canal `local` en el registro.
pub(crate) fn broadcast_open(agent: &mut Agent, local: u64, origin: &str, name: &str) {
    if origin == "null" {
        return;
    }
    let gid = next_gid();
    let inbox = super::inbox(agent);
    let partition = super::partition(agent);
    broadcast_registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(BroadcastEntry {
            gid,
            partition,
            origin: origin.to_owned(),
            name: name.to_owned(),
            inbox,
            local,
        });
    broadcast_host(agent).open.insert(local, gid);
    // El waker de la pestaña (o el sondeo) para recibir lo de otros agentes.
    let realm = agent.current_realm;
    super::ensure_polling(agent, realm);
}

/// "Eligible for messaging" (§9.5): el global es una ventana o un `WorkerGlobalScope` sin el
/// *closing flag*.
fn eligible_for_messaging(agent: &Agent) -> bool {
    !super::scope::closing(agent)
}

/// `postMessage(message)` de `BroadcastChannel` (§9.5), la parte entre agentes. `false` si
/// el canal no es *eligible for messaging* (no se manda nada). Si no, serializa el mensaje
/// (aunque no haya destinos: el `DataCloneError` sube antes de entregar nada) y lo deja en
/// la bandeja de cada destino de otro agente, en orden de creación.
pub(crate) fn broadcast_post(agent: &mut Agent, local: u64, message: &Value) -> JsResult<bool> {
    if !eligible_for_messaging(agent) {
        return Ok(false);
    }
    let allow_shared = super::dedicated::cross_origin_isolated(agent);
    let first = serialize(agent, message, &[], allow_shared)?;
    let Some(gid) = agent
        .embedder_data::<BroadcastHost>()
        .and_then(|b| b.open.get(&local).copied())
    else {
        return Ok(true);
    };
    let mine = super::inbox(agent);
    let targets: Vec<(Arc<AgentInbox>, u64, String)> = {
        let r = broadcast_registry()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(me) = r.iter().find(|b| b.gid == gid) else {
            return Ok(true);
        };
        r.iter()
            .filter(|b| {
                b.gid != gid
                    && b.partition == me.partition
                    && b.origin == me.origin
                    && b.name == me.name
                    && !Arc::ptr_eq(&b.inbox, &mine)
            })
            .map(|b| (b.inbox.clone(), b.local, b.origin.clone()))
            .collect()
    };
    // `Serialized` no se copia: cada destino lleva su serialización.
    let mut first = Some(first);
    for (inbox, to, origin) in targets {
        let message = match first.take() {
            Some(s) => s,
            None => serialize(agent, message, &[], allow_shared)?,
        };
        inbox.push(Envelope::Broadcast {
            local: to,
            origin,
            message,
        });
    }
    Ok(true)
}

/// `close()` de `BroadcastChannel`: sale del registro (lo que esté en camino se descarta).
pub(crate) fn broadcast_close(agent: &mut Agent, local: u64) {
    let Some(gid) = agent
        .embedder_data_mut::<BroadcastHost>()
        .and_then(|b| b.open.remove(&local))
    else {
        return;
    };
    broadcast_registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|b| b.gid != gid);
}

/// Un mensaje de otro agente para el `BroadcastChannel` local `local`: `message` (o
/// `messageerror` si no se puede deserializar) con el origen del que lo mandó. Si el canal
/// ya se cerró, o el worker está cerrando, se descarta.
pub(crate) fn deliver_broadcast(
    agent: &mut Agent,
    local: u64,
    origin: String,
    message: Serialized,
) {
    let open = agent
        .embedder_data::<BroadcastHost>()
        .is_some_and(|b| b.open.contains_key(&local));
    if !open || !eligible_for_messaging(agent) {
        return;
    }
    let t = TargetRef {
        doc: 0,
        target: bf_dom::events::EventTarget::Object(local),
    };
    let (type_, data) = match serialize::deserialize_with_transfer(agent, message) {
        Ok((data, _)) => ("message", data),
        Err(_) => ("messageerror", Value::Null),
    };
    crate::net::channels::fire_message(agent, t, type_, data, &origin, "", Vec::new());
}
