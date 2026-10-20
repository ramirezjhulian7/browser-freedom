//! WeakMap (§24.3), WeakSet (§24.4), WeakRef (§26.1) y FinalizationRegistry (§26.2).
//!
//! Las referencias débiles no se trazan: el heap ([`crate::heap::Heap::register_weak`])
//! conoce estos objetos, marca los valores de WeakMap como efímeros y, antes de barrer,
//! vacía las referencias cuyo destino murió ([`weak_targets`], [`clear_dead`]). Los
//! FinalizationRegistry con celdas vaciadas piden un job de limpieza
//! (HostEnqueueFinalizationRegistryCleanupJob, §9.5.6).

use std::collections::HashMap;

use crate::builtins::map_set::{add_entries_from_iterable, add_values_from_iterable};
use crate::builtins::promise::Job;
use crate::builtins::{FnEntry, constructor, methods, to_string_tag};
use crate::heap::{Gc, Trace, Tracer};
use crate::object::{Object, ObjectKind};
use crate::value::{PropertyKey, Symbol, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

/// Un valor que se puede tener débilmente (CanBeHeldWeakly, §9.13): un objeto o un
/// símbolo no registrado.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WeakKey {
    Object(Gc<Object>),
    Symbol(Gc<Symbol>),
}

impl WeakKey {
    pub fn to_value(self) -> Value {
        match self {
            WeakKey::Object(o) => Value::Object(o),
            WeakKey::Symbol(s) => Value::Symbol(s),
        }
    }
}

/// CanBeHeldWeakly (§9.13).
pub fn can_be_held_weakly(agent: &Agent, v: &Value) -> Option<WeakKey> {
    match v {
        Value::Object(o) => Some(WeakKey::Object(*o)),
        Value::Symbol(s) if agent.symbol_key_for(*s).is_none() => Some(WeakKey::Symbol(*s)),
        _ => None,
    }
}

/// `[[WeakMapData]]` / `[[WeakSetData]]`.
#[derive(Debug, Default)]
pub struct WeakMapData {
    pub entries: HashMap<WeakKey, Value>,
}

impl Trace for WeakMapData {
    /// Ni las claves ni los valores: los valores son efímeros (los marca el heap).
    fn trace(&self, _t: &mut Tracer) {}
}

/// `[[WeakRefTarget]]`.
#[derive(Debug, Default)]
pub struct WeakRefData {
    pub target: Option<WeakKey>,
}

impl Trace for WeakRefData {
    fn trace(&self, _t: &mut Tracer) {}
}

/// Una celda de `[[Cells]]`.
#[derive(Debug)]
pub struct FinalizationCell {
    pub target: Option<WeakKey>,
    pub held: Value,
    pub token: Option<WeakKey>,
}

#[derive(Debug)]
pub struct FinalizationRegistryData {
    pub realm: RealmId,
    pub cleanup: Value,
    pub cells: Vec<FinalizationCell>,
    /// ¿Ya hay un job de limpieza en la cola?
    pub queued: bool,
}

impl Trace for FinalizationRegistryData {
    fn trace(&self, t: &mut Tracer) {
        t.value(&self.cleanup);
        for c in &self.cells {
            t.value(&c.held);
        }
    }
}

// --- Interfaz con el GC ---

/// Las entradas efímeras de un contenedor (las de WeakMap; las de WeakSet no tienen valor).
pub(crate) fn ephemerons(kind: &ObjectKind) -> Vec<(WeakKey, Value)> {
    match kind {
        ObjectKind::WeakMap(d) => d
            .entries
            .iter()
            .filter(|(_, v)| matches!(v, Value::Object(_) | Value::Symbol(_) | Value::BigInt(_)))
            .map(|(k, v)| (*k, v.clone()))
            .collect(),
        _ => Vec::new(),
    }
}

/// Todas las referencias débiles de un contenedor.
pub(crate) fn weak_targets(kind: &ObjectKind) -> Vec<WeakKey> {
    match kind {
        ObjectKind::WeakMap(d) | ObjectKind::WeakSet(d) => d.entries.keys().copied().collect(),
        ObjectKind::WeakRef(r) => r.target.into_iter().collect(),
        ObjectKind::FinalizationRegistry(f) => f
            .cells
            .iter()
            .flat_map(|c| c.target.into_iter().chain(c.token))
            .collect(),
        _ => Vec::new(),
    }
}

/// Vacía las referencias a `dead`; `true` si un FinalizationRegistry tiene celdas nuevas
/// para limpiar.
pub(crate) fn clear_dead(kind: &mut ObjectKind, dead: &[WeakKey]) -> bool {
    match kind {
        ObjectKind::WeakMap(d) | ObjectKind::WeakSet(d) => {
            for k in dead {
                d.entries.remove(k);
            }
            false
        }
        ObjectKind::WeakRef(r) => {
            if r.target.is_some_and(|t| dead.contains(&t)) {
                r.target = None;
            }
            false
        }
        ObjectKind::FinalizationRegistry(f) => {
            let mut emptied = false;
            for c in &mut f.cells {
                if c.token.is_some_and(|t| dead.contains(&t)) {
                    c.token = None;
                }
                if c.target.is_some_and(|t| dead.contains(&t)) {
                    c.target = None;
                    emptied = true;
                }
            }
            emptied
        }
        _ => false,
    }
}

/// HostEnqueueFinalizationRegistryCleanupJob (§9.5.6) para los registros con celdas
/// vaciadas por la última recolección.
pub fn enqueue_cleanup_jobs(agent: &mut Agent) {
    for registry in agent.heap.take_finalization_pending() {
        if let ObjectKind::FinalizationRegistry(f) = &mut agent.heap[registry].kind
            && !f.queued
        {
            f.queued = true;
            agent.enqueue_job(Job::FinalizationCleanup { registry });
        }
    }
}

/// CleanupFinalizationRegistry (§9.13): llama al callback con el valor de cada celda vacía.
pub fn cleanup_finalization_registry(agent: &mut Agent, registry: Gc<Object>) -> JsResult<()> {
    let realm = match &mut agent.heap[registry].kind {
        ObjectKind::FinalizationRegistry(f) => {
            f.queued = false;
            f.realm
        }
        _ => return Ok(()),
    };
    agent.current_realm = realm;
    loop {
        let next = match &mut agent.heap[registry].kind {
            ObjectKind::FinalizationRegistry(f) => {
                match f.cells.iter().position(|c| c.target.is_none()) {
                    Some(i) => Some((f.cleanup.clone(), f.cells.remove(i).held)),
                    None => None,
                }
            }
            _ => None,
        };
        let Some((callback, held)) = next else {
            return Ok(());
        };
        agent.call(&callback, Value::Undefined, &[held])?;
    }
}

// --- Intrínsecos ---

pub fn init(agent: &mut Agent, realm: RealmId) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    // WeakMap (§24.3).
    let wmp = agent.new_object(Some(op));
    const WM: &[FnEntry] = &[
        ("delete", 1, weak_map_delete),
        ("get", 1, weak_map_get),
        ("getOrInsert", 2, weak_map_get_or_insert),
        ("getOrInsertComputed", 2, weak_map_get_or_insert_computed),
        ("has", 1, weak_map_has),
        ("set", 2, weak_map_set),
    ];
    methods(agent, wmp, WM);
    to_string_tag(agent, wmp, "WeakMap");
    constructor(agent, "WeakMap", 0, weak_map_ctor, wmp);
    // WeakSet (§24.4).
    let wsp = agent.new_object(Some(op));
    const WS: &[FnEntry] = &[
        ("add", 1, weak_set_add),
        ("delete", 1, weak_set_delete),
        ("has", 1, weak_set_has),
    ];
    methods(agent, wsp, WS);
    to_string_tag(agent, wsp, "WeakSet");
    constructor(agent, "WeakSet", 0, weak_set_ctor, wsp);
    // WeakRef (§26.1).
    let wrp = agent.new_object(Some(op));
    methods(agent, wrp, &[("deref", 0, weak_ref_deref)]);
    to_string_tag(agent, wrp, "WeakRef");
    constructor(agent, "WeakRef", 1, weak_ref_ctor, wrp);
    // FinalizationRegistry (§26.2).
    let frp = agent.new_object(Some(op));
    const FR: &[FnEntry] = &[
        ("register", 2, registry_register),
        ("unregister", 1, registry_unregister),
    ];
    methods(agent, frp, FR);
    to_string_tag(agent, frp, "FinalizationRegistry");
    constructor(agent, "FinalizationRegistry", 1, registry_ctor, frp);
    let ints = &mut agent.realms[realm].intrinsics;
    ints.weak_map_prototype = wmp;
    ints.weak_set_prototype = wsp;
    ints.weak_ref_prototype = wrp;
    ints.finalization_registry_prototype = frp;
}

fn this_weak(agent: &mut Agent, this: &Value, set: bool, method: &str) -> JsResult<Gc<Object>> {
    if let Value::Object(o) = this {
        match &agent.heap[*o].kind {
            ObjectKind::WeakMap(_) if !set => return Ok(*o),
            ObjectKind::WeakSet(_) if set => return Ok(*o),
            _ => {}
        }
    }
    let class = if set { "WeakSet" } else { "WeakMap" };
    agent.type_error(&format!(
        "Method {class}.prototype.{method} called on incompatible receiver"
    ))
}

fn weak_entries(agent: &mut Agent, o: Gc<Object>) -> &mut HashMap<WeakKey, Value> {
    match &mut agent.heap[o].kind {
        ObjectKind::WeakMap(d) | ObjectKind::WeakSet(d) => &mut d.entries,
        _ => unreachable!("WeakMap o WeakSet"),
    }
}

// --- WeakMap ---

/// WeakMap ( [ iterable ] ) (§24.3.1.1).
fn weak_map_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor WeakMap requires 'new'");
    };
    let map = agent.ordinary_create_from_constructor(
        nt,
        |i| i.weak_map_prototype,
        ObjectKind::WeakMap(Box::default()),
    )?;
    agent.heap.register_weak(map);
    let iterable = a.arg(0);
    if iterable.is_nullish() {
        return Ok(Value::Object(map));
    }
    let adder = agent.get(map, &PropertyKey::from("set"), Value::Object(map))?;
    if !agent.is_callable(&adder) {
        return agent.type_error("WeakMap.prototype.set is not a function");
    }
    add_entries_from_iterable(agent, map, &iterable, &adder)?;
    Ok(Value::Object(map))
}

fn weak_map_delete(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let m = this_weak(agent, &a.this, false, "delete")?;
    let Some(k) = can_be_held_weakly(agent, &a.arg(0)) else {
        return Ok(Value::Boolean(false));
    };
    Ok(Value::Boolean(weak_entries(agent, m).remove(&k).is_some()))
}

fn weak_map_get(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let m = this_weak(agent, &a.this, false, "get")?;
    let Some(k) = can_be_held_weakly(agent, &a.arg(0)) else {
        return Ok(Value::Undefined);
    };
    Ok(weak_entries(agent, m).get(&k).cloned().unwrap_or_default())
}

fn weak_map_has(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let m = this_weak(agent, &a.this, false, "has")?;
    let Some(k) = can_be_held_weakly(agent, &a.arg(0)) else {
        return Ok(Value::Boolean(false));
    };
    Ok(Value::Boolean(weak_entries(agent, m).contains_key(&k)))
}

fn weak_map_set(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let m = this_weak(agent, &a.this, false, "set")?;
    let Some(k) = can_be_held_weakly(agent, &a.arg(0)) else {
        return agent.type_error("Invalid value used as weak map key");
    };
    weak_entries(agent, m).insert(k, a.arg(1));
    Ok(a.this.clone())
}

/// WeakMap.prototype.getOrInsert (propuesta upsert).
fn weak_map_get_or_insert(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let m = this_weak(agent, &a.this, false, "getOrInsert")?;
    let Some(k) = can_be_held_weakly(agent, &a.arg(0)) else {
        return agent.type_error("Invalid value used as weak map key");
    };
    let entries = weak_entries(agent, m);
    Ok(entries.entry(k).or_insert_with(|| a.arg(1)).clone())
}

/// WeakMap.prototype.getOrInsertComputed (propuesta upsert).
fn weak_map_get_or_insert_computed(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let m = this_weak(agent, &a.this, false, "getOrInsertComputed")?;
    let Some(k) = can_be_held_weakly(agent, &a.arg(0)) else {
        return agent.type_error("Invalid value used as weak map key");
    };
    let callback = a.arg(1);
    if !agent.is_callable(&callback) {
        return agent.type_error("callbackfn is not a function");
    }
    if let Some(v) = weak_entries(agent, m).get(&k) {
        return Ok(v.clone());
    }
    let v = agent.call(&callback, Value::Undefined, &[k.to_value()])?;
    weak_entries(agent, m).insert(k, v.clone());
    Ok(v)
}

// --- WeakSet ---

/// WeakSet ( [ iterable ] ) (§24.4.1.1).
fn weak_set_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor WeakSet requires 'new'");
    };
    let set = agent.ordinary_create_from_constructor(
        nt,
        |i| i.weak_set_prototype,
        ObjectKind::WeakSet(Box::default()),
    )?;
    agent.heap.register_weak(set);
    let iterable = a.arg(0);
    if iterable.is_nullish() {
        return Ok(Value::Object(set));
    }
    let adder = agent.get(set, &PropertyKey::from("add"), Value::Object(set))?;
    if !agent.is_callable(&adder) {
        return agent.type_error("WeakSet.prototype.add is not a function");
    }
    add_values_from_iterable(agent, set, &iterable, &adder)?;
    Ok(Value::Object(set))
}

fn weak_set_add(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_weak(agent, &a.this, true, "add")?;
    let Some(k) = can_be_held_weakly(agent, &a.arg(0)) else {
        return agent.type_error("Invalid value used in weak set");
    };
    weak_entries(agent, s).insert(k, Value::Undefined);
    Ok(a.this.clone())
}

fn weak_set_delete(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_weak(agent, &a.this, true, "delete")?;
    let Some(k) = can_be_held_weakly(agent, &a.arg(0)) else {
        return Ok(Value::Boolean(false));
    };
    Ok(Value::Boolean(weak_entries(agent, s).remove(&k).is_some()))
}

fn weak_set_has(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_weak(agent, &a.this, true, "has")?;
    let Some(k) = can_be_held_weakly(agent, &a.arg(0)) else {
        return Ok(Value::Boolean(false));
    };
    Ok(Value::Boolean(weak_entries(agent, s).contains_key(&k)))
}

// --- WeakRef ---

/// AddToKeptObjects (§9.10.2).
fn add_to_kept_objects(agent: &mut Agent, v: Value) {
    agent.kept_alive.push(v);
}

/// WeakRef ( target ) (§26.1.1.1).
fn weak_ref_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor WeakRef requires 'new'");
    };
    let target = a.arg(0);
    let Some(k) = can_be_held_weakly(agent, &target) else {
        return agent.type_error("WeakRef: invalid target");
    };
    let r = agent.ordinary_create_from_constructor(
        nt,
        |i| i.weak_ref_prototype,
        ObjectKind::WeakRef(Box::new(WeakRefData { target: Some(k) })),
    )?;
    agent.heap.register_weak(r);
    add_to_kept_objects(agent, target);
    Ok(Value::Object(r))
}

/// WeakRef.prototype.deref (§26.1.3.2) con WeakRefDeref (§26.1.4.1).
fn weak_ref_deref(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let target = match &a.this {
        Value::Object(o) => match &agent.heap[*o].kind {
            ObjectKind::WeakRef(r) => r.target,
            _ => {
                return agent.type_error("WeakRef.prototype.deref called on incompatible receiver");
            }
        },
        _ => return agent.type_error("WeakRef.prototype.deref called on incompatible receiver"),
    };
    match target {
        Some(t) => {
            let v = t.to_value();
            add_to_kept_objects(agent, v.clone());
            Ok(v)
        }
        None => Ok(Value::Undefined),
    }
}

// --- FinalizationRegistry ---

/// FinalizationRegistry ( cleanupCallback ) (§26.2.1.1).
fn registry_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor FinalizationRegistry requires 'new'");
    };
    let cleanup = a.arg(0);
    if !agent.is_callable(&cleanup) {
        return agent.type_error("FinalizationRegistry: cleanup must be callable");
    }
    let realm = agent.current_realm;
    let r = agent.ordinary_create_from_constructor(
        nt,
        |i| i.finalization_registry_prototype,
        ObjectKind::FinalizationRegistry(Box::new(FinalizationRegistryData {
            realm,
            cleanup,
            cells: Vec::new(),
            queued: false,
        })),
    )?;
    agent.heap.register_weak(r);
    Ok(Value::Object(r))
}

fn this_registry(agent: &mut Agent, this: &Value, method: &str) -> JsResult<Gc<Object>> {
    match this {
        Value::Object(o) if matches!(agent.heap[*o].kind, ObjectKind::FinalizationRegistry(_)) => {
            Ok(*o)
        }
        _ => agent.type_error(&format!(
            "Method FinalizationRegistry.prototype.{method} called on incompatible receiver"
        )),
    }
}

/// FinalizationRegistry.prototype.register (§26.2.3.2).
fn registry_register(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_registry(agent, &a.this, "register")?;
    let target = a.arg(0);
    let held = a.arg(1);
    let token = a.arg(2);
    let Some(t) = can_be_held_weakly(agent, &target) else {
        return agent.type_error("FinalizationRegistry.prototype.register: invalid target");
    };
    if agent.same_value(&target, &held) {
        return agent.type_error(
            "FinalizationRegistry.prototype.register: target and holdings must not be same",
        );
    }
    let token = match can_be_held_weakly(agent, &token) {
        Some(k) => Some(k),
        None if token.is_undefined() => None,
        None => {
            return agent
                .type_error("FinalizationRegistry.prototype.register: invalid unregister token");
        }
    };
    if let ObjectKind::FinalizationRegistry(f) = &mut agent.heap[r].kind {
        f.cells.push(FinalizationCell {
            target: Some(t),
            held,
            token,
        });
    }
    Ok(Value::Undefined)
}

/// FinalizationRegistry.prototype.unregister (§26.2.3.3).
fn registry_unregister(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_registry(agent, &a.this, "unregister")?;
    let Some(token) = can_be_held_weakly(agent, &a.arg(0)) else {
        return agent
            .type_error("FinalizationRegistry.prototype.unregister: invalid unregister token");
    };
    let mut removed = false;
    if let ObjectKind::FinalizationRegistry(f) = &mut agent.heap[r].kind {
        f.cells.retain(|c| {
            let m = c.token == Some(token);
            removed |= m;
            !m
        });
    }
    Ok(Value::Boolean(removed))
}
