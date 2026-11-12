//! Las conexiones de IndexedDB de todo el proceso: el registro que comparten las pestañas (y
//! los iframes) que guardan en el mismo directorio de origen, para los pasos que miran "las
//! conexiones abiertas de la base de datos" de cualquier agente (IndexedDB 3.0 §5.1 paso 10,
//! §5.3 paso 5: `versionchange` y `blocked`) y para saber cuándo la copia en memoria de una
//! base de datos quedó vieja porque otro agente escribió en su log.
//!
//! - Una conexión se registra con la bandeja de su agente ([`crate::workers::AgentInbox`]) y su
//!   realm; los avisos viajan como [`Envelope::Run`] y corren en el hilo de destino como una
//!   tarea, que llama al gancho que `idb2.js` dejó en su realm (`native.setHook`).
//! - El que espera (`open` con versión mayor, `deleteDatabase`) manda `versionchange` a las
//!   conexiones de los otros realms y espera sus acuses: cuando llegan todos, si alguna sigue
//!   abierta recibe `"blocked"`, y cuando se cierra la última, `"resume"`.
//! - Una conexión cuyo agente ya no existe (se soltó su bandeja) o cuyo realm ya no tiene el
//!   gancho cuenta como cerrada.
//! - Sin directorio (almacén en memoria del agente) no hay nada que compartir: todo es local.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use bf_js::host::HostHandle;
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, RealmId};

use crate::workers::{AgentInbox, Envelope};

/// Una base de datos del proceso: el directorio de su origen y su nombre.
type Key = (PathBuf, String);

/// Quién: la bandeja del agente y el realm.
#[derive(Clone)]
struct Owner {
    inbox: Weak<AgentInbox>,
    realm: RealmId,
}

impl Owner {
    fn same(&self, other: &Owner) -> bool {
        self.realm == other.realm && Weak::ptr_eq(&self.inbox, &other.inbox)
    }

    fn alive(&self) -> bool {
        self.inbox.strong_count() > 0
    }

    /// Manda `f` al agente del dueño, para que corra en su realm; `false` si ya no existe.
    fn send(&self, f: impl FnOnce(&mut Agent, RealmId) + Send + 'static) -> bool {
        let Some(inbox) = self.inbox.upgrade() else {
            return false;
        };
        let realm = self.realm;
        inbox.push(Envelope::Run(Box::new(move |agent: &mut Agent| {
            f(agent, realm);
        })));
        true
    }
}

struct Conn {
    owner: Owner,
    id: u64,
}

/// Un `open`/`deleteDatabase` que espera a que se cierren las conexiones de otros realms.
struct Waiter {
    owner: Owner,
    token: u64,
    /// Acuses de `versionchange` que faltan.
    acks: usize,
    /// Ya se le avisó `blocked`.
    blocked: bool,
}

#[derive(Default)]
struct Entry {
    conns: Vec<Conn>,
    waiters: Vec<Waiter>,
    /// Sube con cada escritura en el log (de cualquier agente).
    generation: u64,
}

fn registry() -> &'static Mutex<HashMap<Key, Entry>> {
    static R: OnceLock<Mutex<HashMap<Key, Entry>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

fn with_registry<T>(f: impl FnOnce(&mut HashMap<Key, Entry>) -> T) -> T {
    f(&mut registry().lock().unwrap_or_else(|e| e.into_inner()))
}

/// El gancho de `idb2.js` en un realm: `hook(kind, a, b, c)`.
struct Hook(HostHandle);

/// La clave de `name` en el realm en curso (`None` si el almacén está en memoria).
fn key(agent: &mut Agent, name: &str) -> Option<Key> {
    let dir = super::backend_mut(agent).dir.clone()?;
    Some((dir, name.to_owned()))
}

fn owner(agent: &mut Agent, realm: RealmId) -> Owner {
    Owner {
        inbox: Arc::downgrade(&crate::workers::inbox(agent)),
        realm,
    }
}

fn me(agent: &mut Agent) -> Owner {
    let realm = crate::WebContextHandle::current(agent).realm;
    owner(agent, realm)
}

/// ¿Quedan conexiones vivas de otros realms que el que espera no puede cerrar él mismo?
fn others_open(e: &Entry, w: &Owner) -> bool {
    e.conns.iter().any(|c| !c.owner.same(w) && c.owner.alive())
}

/// Revisa los que esperan en `e`: a los que ya no tienen acuses pendientes les toca
/// `blocked` (si quedan conexiones de otros) o `resume` (si no).
fn settle(e: &mut Entry) {
    e.conns.retain(|c| c.owner.alive());
    e.waiters.retain(|w| w.owner.alive());
    let mut i = 0;
    while i < e.waiters.len() {
        if e.waiters[i].acks > 0 {
            i += 1;
            continue;
        }
        let who = e.waiters[i].owner.clone();
        let token = e.waiters[i].token;
        if others_open(e, &who) {
            if !e.waiters[i].blocked {
                e.waiters[i].blocked = true;
                who.send(move |agent, realm| {
                    call_hook(agent, realm, "blocked", token, 0.0, 0.0);
                });
            }
            i += 1;
        } else {
            e.waiters.remove(i);
            who.send(move |agent, realm| {
                call_hook(agent, realm, "resume", token, 0.0, 0.0);
            });
        }
    }
}

/// Llama al gancho de `realm` (si sigue ahí); `false` si no hay gancho.
fn call_hook(agent: &mut Agent, realm: RealmId, kind: &str, a: u64, b: f64, c: f64) -> bool {
    let Some(hook) = agent
        .realm_data::<Hook>(realm)
        .and_then(|h| agent.host_pinned(h.0))
    else {
        return false;
    };
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let _ = agent.call(
        &hook,
        Value::Undefined,
        &[
            Value::str(kind),
            Value::Number(a as f64),
            Value::Number(b),
            Value::Number(c),
        ],
    );
    let _ = agent.perform_microtask_checkpoint();
    agent.current_realm = saved;
    true
}

fn str_arg(agent: &mut Agent, v: &Value) -> JsResult<String> {
    Ok(agent.to_string(v)?.to_string())
}

fn u64_arg(agent: &mut Agent, v: &Value) -> JsResult<u64> {
    Ok(agent.to_number(v)? as u64)
}

/// `setHook(f)`: el gancho de este realm para los avisos de otros agentes.
pub(super) fn set_hook(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let realm = crate::WebContextHandle::current(agent).realm;
    let h = agent.host_pin(a.arg(0));
    if let Some(Hook(old)) = agent.take_realm_data::<Hook>(realm) {
        agent.host_unpin(old);
    }
    agent.set_realm_data(realm, Hook(h));
    Ok(Value::Undefined)
}

/// `shareOpen(name, id)`: registra una conexión abierta de este realm.
pub(super) fn share_open(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = str_arg(agent, &a.arg(0))?;
    let id = u64_arg(agent, &a.arg(1))?;
    let Some(k) = key(agent, &name) else {
        return Ok(Value::Undefined);
    };
    let who = me(agent);
    with_registry(|r| {
        r.entry(k).or_default().conns.push(Conn { owner: who, id });
    });
    let realm = crate::WebContextHandle::current(agent).realm;
    crate::workers::ensure_polling(agent, realm);
    Ok(Value::Undefined)
}

/// `shareClose(name, id)`: la conexión se cerró; puede destrabar a los que esperan.
pub(super) fn share_close(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = str_arg(agent, &a.arg(0))?;
    let id = u64_arg(agent, &a.arg(1))?;
    let Some(k) = key(agent, &name) else {
        return Ok(Value::Undefined);
    };
    let who = me(agent);
    with_registry(|r| {
        if let Some(e) = r.get_mut(&k) {
            e.conns.retain(|c| !(c.id == id && c.owner.same(&who)));
            settle(e);
        }
    });
    Ok(Value::Undefined)
}

/// `shareNotify(name, oldVersion, newVersion, token)`: manda `versionchange` a las conexiones
/// de los otros realms (`newVersion` < 0 es `null`, para `deleteDatabase`) y deja al realm en
/// curso esperando con `token`; `true` si había alguna (entonces llegará `"resume"`, y antes
/// `"blocked"` si alguna no se cierra al recibir el evento).
pub(super) fn share_notify(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = str_arg(agent, &a.arg(0))?;
    let old = agent.to_number(&a.arg(1))?;
    let new = agent.to_number(&a.arg(2))?;
    let token = u64_arg(agent, &a.arg(3))?;
    let Some(k) = key(agent, &name) else {
        return Ok(Value::Boolean(false));
    };
    let who = me(agent);
    let realm = who.realm;
    let any = with_registry(|r| {
        let e = r.entry(k.clone()).or_default();
        e.conns.retain(|c| c.owner.alive());
        let targets: Vec<(Owner, u64)> = e
            .conns
            .iter()
            .filter(|c| !c.owner.same(&who))
            .map(|c| (c.owner.clone(), c.id))
            .collect();
        if targets.is_empty() {
            return false;
        }
        let mut acks = 0;
        for (target, id) in targets {
            let k = k.clone();
            let waiter = who.clone();
            let sent = target.send(move |agent, realm| {
                // Sin gancho (el realm se fue), la conexión cuenta como cerrada.
                if !call_hook(agent, realm, "versionchange", id, old, new) {
                    let gone = owner(agent, realm);
                    with_registry(|r| {
                        if let Some(e) = r.get_mut(&k) {
                            e.conns.retain(|c| !(c.id == id && c.owner.same(&gone)));
                        }
                    });
                }
                ack(&k, &waiter, token);
            });
            if sent {
                acks += 1;
            }
        }
        e.waiters.push(Waiter {
            owner: who.clone(),
            token,
            acks,
            blocked: false,
        });
        settle(e);
        true
    });
    if any {
        crate::workers::ensure_polling(agent, realm);
    }
    Ok(Value::Boolean(any))
}

/// Llegó el acuse de un `versionchange` del que espera `waiter` con `token`.
fn ack(k: &Key, waiter: &Owner, token: u64) {
    with_registry(|r| {
        if let Some(e) = r.get_mut(k) {
            if let Some(w) = e
                .waiters
                .iter_mut()
                .find(|w| w.token == token && w.owner.same(waiter))
            {
                w.acks = w.acks.saturating_sub(1);
            }
            settle(e);
        }
    });
}

/// `shareGeneration(name)`: cuántas veces se escribió el log de `name` en este proceso (para
/// saber si la copia en memoria quedó vieja).
pub(super) fn share_generation(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = str_arg(agent, &a.arg(0))?;
    let Some(k) = key(agent, &name) else {
        return Ok(Value::Number(0.0));
    };
    let g = with_registry(|r| r.get(&k).map_or(0, |e| e.generation));
    Ok(Value::Number(g as f64))
}

/// El log de `name` cambió (lo llaman `append`, `compact` y `remove`).
pub(super) fn bump(agent: &mut Agent, name: &str) {
    if let Some(k) = key(agent, name) {
        with_registry(|r| r.entry(k).or_default().generation += 1);
    }
}

/// ¿Tiene este agente conexiones registradas (o espera a las de otros)? Mientras sí, su
/// bandeja se sigue vaciando aunque no tenga workers.
pub fn active(agent: &mut Agent) -> bool {
    let inbox = Arc::downgrade(&crate::workers::inbox(agent));
    with_registry(|r| {
        r.values().any(|e| {
            e.conns.iter().any(|c| Weak::ptr_eq(&c.owner.inbox, &inbox))
                || e.waiters
                    .iter()
                    .any(|w| Weak::ptr_eq(&w.owner.inbox, &inbox))
        })
    })
}
