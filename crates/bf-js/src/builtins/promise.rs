//! Promise (§27.2) y la cola de jobs del agente (§9.5): HostEnqueuePromiseJob, RunJobs y
//! HostPromiseRejectionTracker.
//!
//! Las reacciones guardan su handler como [`Handler`]: una función de JavaScript (el
//! JobCallback de HostMakeJobCallback) o una de las continuaciones internas que el spec crea
//! con CreateBuiltinFunction pero que nunca llegan al código (las de `Await`, las de
//! AsyncGeneratorAwaitReturn y las de AsyncFromSyncIteratorContinuation): ejecutarlas directo
//! es inobservable. Del mismo modo, una PromiseCapability de %Promise% que nunca se expone
//! (la de `then` cuando el constructor es %Promise%, las de las funciones async, etc.) es
//! [`Capability::Internal`]: la promesa misma, resuelta con el algoritmo de las Promise
//! Resolve Functions.

use crate::builtins::{FnEntry, accessor, constructor, methods, slots, to_string_tag};
use crate::heap::{Gc, Trace, Tracer};
use crate::object::{ArrayData, Object, ObjectKind};
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, ErrorType, JsResult, RealmId, Throw};

/// `[[PromiseState]]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromiseState {
    Pending,
    Fulfilled,
    Rejected,
}

/// Los internal slots de una instancia de Promise (§27.2.6).
#[derive(Debug)]
pub struct PromiseData {
    pub state: PromiseState,
    pub result: Value,
    pub fulfill_reactions: Vec<Reaction>,
    pub reject_reactions: Vec<Reaction>,
    pub is_handled: bool,
}

impl PromiseData {
    pub fn new() -> PromiseData {
        PromiseData {
            state: PromiseState::Pending,
            result: Value::Undefined,
            fulfill_reactions: Vec::new(),
            reject_reactions: Vec::new(),
            is_handled: false,
        }
    }
}

impl Default for PromiseData {
    fn default() -> PromiseData {
        PromiseData::new()
    }
}

impl Trace for PromiseData {
    fn trace(&self, t: &mut Tracer) {
        t.value(&self.result);
        for r in self.fulfill_reactions.iter().chain(&self.reject_reactions) {
            r.trace(t);
        }
    }
}

/// PromiseCapability Record (§27.2.1.1).
#[derive(Debug, Clone)]
pub enum Capability {
    /// NewPromiseCapability(%Promise%) cuyas funciones de resolución nunca se exponen.
    Internal(Gc<Object>),
    Js {
        promise: Value,
        resolve: Value,
        reject: Value,
    },
}

impl Capability {
    pub fn promise(&self) -> Value {
        match self {
            Capability::Internal(p) => Value::Object(*p),
            Capability::Js { promise, .. } => promise.clone(),
        }
    }
}

impl Trace for Capability {
    fn trace(&self, t: &mut Tracer) {
        match self {
            Capability::Internal(p) => t.gc(*p),
            Capability::Js {
                promise,
                resolve,
                reject,
            } => {
                t.value(promise);
                t.value(resolve);
                t.value(reject);
            }
        }
    }
}

/// `[[Handler]]` de una PromiseReaction.
#[derive(Debug, Clone)]
pub enum Handler {
    Empty,
    /// Un JobCallback (HostMakeJobCallback, §9.5.2).
    Callback(Value),
    /// Las funciones `fulfilledClosure`/`rejectedClosure` de Await (§27.7.5.3): reanudan la
    /// función async o el generador async `target`. Con `ret` (AsyncGeneratorUnwrapYieldResumption)
    /// el valor cumplido se entrega como una return completion.
    AsyncResume {
        target: Gc<Object>,
        ret: bool,
    },
    /// Las de AsyncGeneratorAwaitReturn (§27.6.3.9).
    AsyncGenAwaitReturn(Gc<Object>),
    /// `unwrap` de AsyncFromSyncIteratorContinuation (§27.1.6.4).
    IterResult {
        done: bool,
    },
    /// `closeIterator` de AsyncFromSyncIteratorContinuation.
    CloseSyncIterator(Value),
}

impl Trace for Handler {
    fn trace(&self, t: &mut Tracer) {
        match self {
            Handler::Empty | Handler::IterResult { .. } => {}
            Handler::Callback(v) | Handler::CloseSyncIterator(v) => t.value(v),
            Handler::AsyncResume { target, .. } => t.gc(*target),
            Handler::AsyncGenAwaitReturn(g) => t.gc(*g),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReactionType {
    Fulfill,
    Reject,
}

/// PromiseReaction Record (§27.2.1.2).
#[derive(Debug, Clone)]
pub struct Reaction {
    pub capability: Option<Capability>,
    pub kind: ReactionType,
    pub handler: Handler,
}

impl Trace for Reaction {
    fn trace(&self, t: &mut Tracer) {
        if let Some(c) = &self.capability {
            c.trace(t);
        }
        self.handler.trace(t);
    }
}

/// Un job pendiente (§9.5) con su realm.
#[derive(Debug, Clone)]
pub enum Job {
    /// NewPromiseReactionJob (§27.2.2.1).
    Reaction {
        reaction: Reaction,
        argument: Value,
        realm: RealmId,
    },
    /// NewPromiseResolveThenableJob (§27.2.2.2).
    ResolveThenable {
        promise: Gc<Object>,
        thenable: Value,
        then: Value,
        realm: RealmId,
    },
    /// HostEnqueueFinalizationRegistryCleanupJob (§9.5.6).
    FinalizationCleanup { registry: Gc<Object> },
}

impl Trace for Job {
    fn trace(&self, t: &mut Tracer) {
        match self {
            Job::Reaction {
                reaction, argument, ..
            } => {
                reaction.trace(t);
                t.value(argument);
            }
            Job::ResolveThenable {
                promise,
                thenable,
                then,
                ..
            } => {
                t.gc(*promise);
                t.value(thenable);
                t.value(then);
            }
            Job::FinalizationCleanup { registry } => t.gc(*registry),
        }
    }
}

/// El estado de HostPromiseRejectionTracker (§27.2.1.9): como en HTML, las promesas
/// rechazadas sin handler se notifican al vaciar la cola de jobs si siguen sin manejar.
#[derive(Debug, Default)]
pub struct RejectionTracker {
    /// Rechazadas sin handler desde la última notificación.
    pub pending: Vec<Gc<Object>>,
    /// Rechazos notificados que todavía no tienen handler.
    pub unhandled: usize,
    /// Rechazos notificados que recibieron un handler después.
    pub handled_late: usize,
}

impl Trace for RejectionTracker {
    fn trace(&self, t: &mut Tracer) {
        for p in &self.pending {
            t.gc(*p);
        }
    }
}

/// Un `[[AlreadyResolved]]`/`[[AlreadyCalled]]` compartido: un objeto interno sin prototipo
/// con un booleano.
fn new_flag(agent: &mut Agent) -> Value {
    Value::Object(agent.alloc(Object::new(None, ObjectKind::Boolean(false))))
}

/// Lee y enciende una bandera de [`new_flag`]; devuelve el valor anterior.
fn test_and_set(agent: &mut Agent, flag: &Value) -> bool {
    let Value::Object(o) = flag else { return true };
    let prev = matches!(agent.heap[*o].kind, ObjectKind::Boolean(true));
    agent.heap[*o].kind = ObjectKind::Boolean(true);
    prev
}

/// IsPromise (§27.2.1.6).
pub fn promise_of(agent: &Agent, v: &Value) -> Option<Gc<Object>> {
    match v {
        Value::Object(o) if matches!(agent.heap[*o].kind, ObjectKind::Promise(_)) => Some(*o),
        _ => None,
    }
}

/// Un handler a partir de un argumento de `then` (paso 3–6 de PerformPromiseThen).
pub fn js_handler(agent: &Agent, v: &Value) -> Handler {
    if agent.is_callable(v) {
        Handler::Callback(v.clone())
    } else {
        Handler::Empty
    }
}

impl Agent {
    // --- Jobs ---

    /// HostEnqueuePromiseJob (§9.5.5) y los demás jobs del agente.
    pub fn enqueue_job(&mut self, job: Job) {
        self.jobs.push_back(job);
    }

    /// RunJobs: ejecuta los jobs hasta vaciar la cola. Una excepción que nadie atrapa dentro
    /// de un job se reporta (HostReportErrors: queda en `job_errors`) y la cola sigue; solo la
    /// terminación del agente corta la ejecución.
    pub fn run_jobs(&mut self) -> JsResult<()> {
        loop {
            self.drain_jobs()?;
            // Con la cola vacía, el siguiente timer (waitAsync, `$262.agent.setTimeout`).
            if !crate::builtins::atomics::fire_next_timer(self)? {
                break;
            }
        }
        self.notify_rejections();
        Ok(())
    }

    fn drain_jobs(&mut self) -> JsResult<()> {
        self.kept_alive.clear();
        while let Some(job) = self.jobs.pop_front() {
            self.running_jobs.push(job.clone());
            let saved = self.current_realm;
            let r = self.run_job(job);
            self.current_realm = saved;
            self.running_jobs.pop();
            // ClearKeptObjects (§9.10.1) al terminar cada job.
            self.kept_alive.clear();
            match r {
                Ok(()) => {}
                Err(Throw::Value(e)) => self.job_errors.push(e),
                Err(t) => return Err(t),
            }
        }
        Ok(())
    }

    /// La cantidad de promesas rechazadas sin handler (HostPromiseRejectionTracker).
    pub fn unhandled_rejections(&self) -> usize {
        self.rejections.unhandled + self.rejections.pending.len()
    }

    fn run_job(&mut self, job: Job) -> JsResult<()> {
        match job {
            Job::Reaction {
                reaction,
                argument,
                realm,
            } => {
                self.current_realm = realm;
                self.promise_reaction_job(reaction, argument)
            }
            Job::ResolveThenable {
                promise,
                thenable,
                then,
                realm,
            } => {
                self.current_realm = realm;
                self.promise_resolve_thenable_job(promise, thenable, then)
            }
            Job::FinalizationCleanup { registry } => {
                crate::builtins::weak::cleanup_finalization_registry(self, registry)
            }
        }
    }

    /// NewPromiseReactionJob, el job (§27.2.2.1).
    fn promise_reaction_job(&mut self, reaction: Reaction, argument: Value) -> JsResult<()> {
        let Reaction {
            capability,
            kind,
            handler,
        } = reaction;
        let fulfilled = kind == ReactionType::Fulfill;
        let result = match handler {
            Handler::Empty => {
                if fulfilled {
                    Ok(argument)
                } else {
                    Err(Throw::Value(argument))
                }
            }
            // HostCallJobCallback (§9.5.3). El job está enraizado en `running_jobs`.
            Handler::Callback(f) => self.call_internal(&f, Value::Undefined, &[argument]),
            Handler::AsyncResume { target, ret } => {
                return crate::builtins::r#async::resume_after_await(
                    self, target, fulfilled, argument, ret,
                );
            }
            Handler::AsyncGenAwaitReturn(g) => {
                return crate::builtins::r#async::await_return_settled(
                    self, g, fulfilled, argument,
                );
            }
            Handler::IterResult { done } => self.create_iter_result(argument, done),
            Handler::CloseSyncIterator(it) => {
                // IteratorClose(syncIteratorRecord, ThrowCompletion(error)).
                self.iterator_close_on_throw(&it);
                Err(Throw::Value(argument))
            }
        };
        let Some(cap) = capability else {
            return match result {
                Err(Throw::Terminate(m)) => Err(Throw::Terminate(m)),
                _ => Ok(()),
            };
        };
        match result {
            Ok(v) => self.capability_resolve(&cap, v),
            Err(Throw::Value(e)) => self.capability_reject(&cap, e),
            Err(t) => Err(t),
        }
    }

    /// NewPromiseResolveThenableJob, el job (§27.2.2.2).
    fn promise_resolve_thenable_job(
        &mut self,
        promise: Gc<Object>,
        thenable: Value,
        then: Value,
    ) -> JsResult<()> {
        let (resolve, reject) = self.create_resolving_functions(promise);
        let scope = self.heap.open_scope();
        self.heap.root(resolve.clone());
        self.heap.root(reject.clone());
        let r = self.call_internal(&then, thenable, &[resolve, reject.clone()]);
        let out = match r {
            Ok(_) => Ok(()),
            Err(Throw::Value(e)) => self.call(&reject, Value::Undefined, &[e]).map(|_| ()),
            Err(t) => Err(t),
        };
        self.heap.close_scope(scope);
        out
    }

    // --- Operaciones abstractas (§27.2.1) ---

    /// Una promesa nueva y pendiente con %Promise.prototype% del realm actual.
    pub fn new_internal_promise(&mut self) -> Gc<Object> {
        let proto = self.intrinsics().promise_prototype;
        self.alloc(Object::new(
            Some(proto),
            ObjectKind::Promise(Box::default()),
        ))
    }

    /// CreateResolvingFunctions (§27.2.1.3): (resolve, reject).
    pub fn create_resolving_functions(&mut self, promise: Gc<Object>) -> (Value, Value) {
        let flag = new_flag(self);
        let resolve = self.create_builtin_with_slots(
            resolve_function,
            1,
            "",
            vec![Value::Object(promise), flag.clone()],
        );
        let reject = self.create_builtin_with_slots(
            reject_function,
            1,
            "",
            vec![Value::Object(promise), flag],
        );
        (Value::Object(resolve), Value::Object(reject))
    }

    /// Los pasos 7–16 de las Promise Resolve Functions (§27.2.1.3.2).
    pub fn resolve_promise(&mut self, promise: Gc<Object>, resolution: Value) -> JsResult<()> {
        if !self.promise_is_pending(promise) {
            return Ok(());
        }
        let Value::Object(ro) = resolution else {
            self.fulfill_promise(promise, resolution);
            return Ok(());
        };
        if ro == promise {
            let e = self.make_error(ErrorType::TypeError, "Chaining cycle detected for promise");
            self.reject_promise(promise, Value::Object(e));
            return Ok(());
        }
        let then = match self.get(ro, &PropertyKey::from("then"), Value::Object(ro)) {
            Ok(t) => t,
            Err(Throw::Value(e)) => {
                self.reject_promise(promise, e);
                return Ok(());
            }
            Err(t) => return Err(t),
        };
        if !self.is_callable(&then) {
            self.fulfill_promise(promise, Value::Object(ro));
            return Ok(());
        }
        // El realm del job: GetFunctionRealm(then) si se puede (§27.2.2.2 paso 3).
        let realm = match &then {
            Value::Object(f) => self.get_function_realm(*f).unwrap_or(self.current_realm),
            _ => self.current_realm,
        };
        self.enqueue_job(Job::ResolveThenable {
            promise,
            thenable: Value::Object(ro),
            then,
            realm,
        });
        Ok(())
    }

    fn promise_is_pending(&self, p: Gc<Object>) -> bool {
        matches!(&self.heap[p].kind, ObjectKind::Promise(d) if d.state == PromiseState::Pending)
    }

    /// FulfillPromise (§27.2.1.4).
    pub fn fulfill_promise(&mut self, p: Gc<Object>, value: Value) {
        let reactions = match &mut self.heap[p].kind {
            ObjectKind::Promise(d) if d.state == PromiseState::Pending => {
                d.state = PromiseState::Fulfilled;
                d.result = value.clone();
                d.reject_reactions.clear();
                std::mem::take(&mut d.fulfill_reactions)
            }
            _ => return,
        };
        self.trigger_promise_reactions(reactions, value);
    }

    /// RejectPromise (§27.2.1.7).
    pub fn reject_promise(&mut self, p: Gc<Object>, reason: Value) {
        let (reactions, handled) = match &mut self.heap[p].kind {
            ObjectKind::Promise(d) if d.state == PromiseState::Pending => {
                d.state = PromiseState::Rejected;
                d.result = reason.clone();
                d.fulfill_reactions.clear();
                (std::mem::take(&mut d.reject_reactions), d.is_handled)
            }
            _ => return,
        };
        if !handled {
            self.promise_rejection_tracker(p, true);
        }
        self.trigger_promise_reactions(reactions, reason);
    }

    /// TriggerPromiseReactions (§27.2.1.8).
    fn trigger_promise_reactions(&mut self, reactions: Vec<Reaction>, argument: Value) {
        for r in reactions {
            self.enqueue_reaction_job(r, argument.clone());
        }
    }

    /// NewPromiseReactionJob + HostEnqueuePromiseJob: el realm es el del handler si se
    /// puede obtener, si no el actual.
    fn enqueue_reaction_job(&mut self, reaction: Reaction, argument: Value) {
        let realm = match &reaction.handler {
            Handler::Callback(Value::Object(f)) => {
                self.get_function_realm(*f).unwrap_or(self.current_realm)
            }
            _ => self.current_realm,
        };
        self.enqueue_job(Job::Reaction {
            reaction,
            argument,
            realm,
        });
    }

    /// HostPromiseRejectionTracker (§27.2.1.9): `reject` = operación "reject", si no "handle".
    fn promise_rejection_tracker(&mut self, p: Gc<Object>, reject: bool) {
        let t = &mut self.rejections;
        if reject {
            t.pending.push(p);
        } else if let Some(i) = t.pending.iter().position(|x| *x == p) {
            t.pending.remove(i);
        } else {
            t.unhandled = t.unhandled.saturating_sub(1);
            t.handled_late += 1;
        }
    }

    /// Notifica las promesas rechazadas que siguen sin handler (al vaciar la cola).
    fn notify_rejections(&mut self) {
        let pending = std::mem::take(&mut self.rejections.pending);
        for p in pending {
            if matches!(&self.heap[p].kind, ObjectKind::Promise(d) if !d.is_handled) {
                self.rejections.unhandled += 1;
            }
        }
    }

    /// PerformPromiseThen (§27.2.5.4.1).
    pub fn perform_promise_then(
        &mut self,
        promise: Gc<Object>,
        on_fulfilled: Handler,
        on_rejected: Handler,
        capability: Option<Capability>,
    ) -> Value {
        let fulfill = Reaction {
            capability: capability.clone(),
            kind: ReactionType::Fulfill,
            handler: on_fulfilled,
        };
        let reject = Reaction {
            capability: capability.clone(),
            kind: ReactionType::Reject,
            handler: on_rejected,
        };
        let (state, result, handled) = match &self.heap[promise].kind {
            ObjectKind::Promise(d) => (d.state, d.result.clone(), d.is_handled),
            _ => unreachable!("PerformPromiseThen sobre algo que no es una promesa"),
        };
        match state {
            PromiseState::Pending => {
                if let ObjectKind::Promise(d) = &mut self.heap[promise].kind {
                    d.fulfill_reactions.push(fulfill);
                    d.reject_reactions.push(reject);
                }
            }
            PromiseState::Fulfilled => self.enqueue_reaction_job(fulfill, result),
            PromiseState::Rejected => {
                if !handled {
                    self.promise_rejection_tracker(promise, false);
                }
                self.enqueue_reaction_job(reject, result);
            }
        }
        if let ObjectKind::Promise(d) = &mut self.heap[promise].kind {
            d.is_handled = true;
        }
        capability.map_or(Value::Undefined, |c| c.promise())
    }

    /// NewPromiseCapability (§27.2.1.5).
    pub fn new_promise_capability(&mut self, c: &Value) -> JsResult<Capability> {
        if !self.is_constructor(c) {
            return self.type_error(&format!("{} is not a constructor", self.describe(c)));
        }
        let executor = self.create_builtin_with_slots(
            get_capabilities_executor,
            2,
            "",
            vec![Value::Undefined, Value::Undefined],
        );
        let ctor = c.as_object().expect("constructor");
        let promise = self.construct(ctor, &[Value::Object(executor)], None)?;
        let s = slots(self, executor);
        if !self.is_callable(&s[0]) {
            return self.type_error("Promise resolve function is not callable");
        }
        if !self.is_callable(&s[1]) {
            return self.type_error("Promise reject function is not callable");
        }
        Ok(Capability::Js {
            promise,
            resolve: s[0].clone(),
            reject: s[1].clone(),
        })
    }

    /// Una capability de `c` que no se va a exponer: interna si `c` es el %Promise% del
    /// realm actual.
    pub fn new_promise_capability_hidden(&mut self, c: Gc<Object>) -> JsResult<Capability> {
        if c == self.intrinsics().promise_ctor {
            return Ok(Capability::Internal(self.new_internal_promise()));
        }
        self.new_promise_capability(&Value::Object(c))
    }

    /// `Call(capability.[[Resolve]], undefined, « v »)`.
    pub fn capability_resolve(&mut self, cap: &Capability, v: Value) -> JsResult<()> {
        match cap {
            Capability::Internal(p) => self.resolve_promise(*p, v),
            Capability::Js { resolve, .. } => {
                let r = resolve.clone();
                self.call(&r, Value::Undefined, &[v]).map(|_| ())
            }
        }
    }

    /// `Call(capability.[[Reject]], undefined, « e »)`.
    pub fn capability_reject(&mut self, cap: &Capability, e: Value) -> JsResult<()> {
        match cap {
            Capability::Internal(p) => {
                if self.promise_is_pending(*p) {
                    self.reject_promise(*p, e);
                }
                Ok(())
            }
            Capability::Js { reject, .. } => {
                let r = reject.clone();
                self.call(&r, Value::Undefined, &[e]).map(|_| ())
            }
        }
    }

    /// IfAbruptRejectPromise (§27.2.1.1.1): rechaza y devuelve la promesa.
    pub fn if_abrupt_reject(&mut self, e: Throw, cap: &Capability) -> JsResult<Value> {
        match e {
            Throw::Value(v) => {
                self.capability_reject(cap, v)?;
                Ok(cap.promise())
            }
            t => Err(t),
        }
    }

    /// PromiseResolve (§27.2.4.7.1).
    pub fn promise_resolve(&mut self, c: Gc<Object>, x: Value) -> JsResult<Value> {
        if let Some(xo) = promise_of(self, &x) {
            let k = self.keys.constructor.clone();
            let xc = self.get(xo, &k, x.clone())?;
            if let Value::Object(xc) = xc
                && xc == c
            {
                return Ok(x);
            }
        }
        let cap = self.new_promise_capability_hidden(c)?;
        self.capability_resolve(&cap, x)?;
        Ok(cap.promise())
    }
}

// --- Funciones internas ---

/// Promise Resolve Functions (§27.2.1.3.2).
fn resolve_function(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = slots(agent, a.callee);
    if test_and_set(agent, &s[1]) {
        return Ok(Value::Undefined);
    }
    let Value::Object(p) = s[0] else {
        return Ok(Value::Undefined);
    };
    agent.resolve_promise(p, a.arg(0))?;
    Ok(Value::Undefined)
}

/// Promise Reject Functions (§27.2.1.3.1).
fn reject_function(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = slots(agent, a.callee);
    if test_and_set(agent, &s[1]) {
        return Ok(Value::Undefined);
    }
    let Value::Object(p) = s[0] else {
        return Ok(Value::Undefined);
    };
    agent.reject_promise(p, a.arg(0));
    Ok(Value::Undefined)
}

/// GetCapabilitiesExecutor Functions (§27.2.1.5 paso 4).
fn get_capabilities_executor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = slots(agent, a.callee);
    if !s[0].is_undefined() || !s[1].is_undefined() {
        return agent.type_error("Promise executor has already been invoked");
    }
    if let ObjectKind::Builtin(b) = &mut agent.heap[a.callee].kind {
        b.slots[0] = a.arg(0);
        b.slots[1] = a.arg(1);
    }
    Ok(Value::Undefined)
}

// --- Constructor y prototipo ---

pub fn init(agent: &mut Agent, realm: RealmId) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    let proto = agent.new_object(Some(op));
    const PROTO: &[FnEntry] = &[
        ("catch", 1, catch),
        ("finally", 1, finally),
        ("then", 2, then),
    ];
    methods(agent, proto, PROTO);
    to_string_tag(agent, proto, "Promise");
    let ctor = constructor(agent, "Promise", 1, promise_ctor, proto);
    const STATIC: &[FnEntry] = &[
        ("all", 1, all),
        ("allSettled", 1, all_settled),
        ("any", 1, any),
        ("race", 1, race),
        ("reject", 1, reject),
        ("resolve", 1, resolve),
        ("try", 1, try_),
        ("withResolvers", 0, with_resolvers),
    ];
    methods(agent, ctor, STATIC);
    let species = PropertyKey::Symbol(agent.well_known.species);
    accessor(
        agent,
        ctor,
        species,
        "[Symbol.species]",
        Some(species_getter),
        None,
    );
    let i = &mut agent.realms[realm].intrinsics;
    i.promise_ctor = ctor;
    i.promise_prototype = proto;
}

/// Registra los bindings globales de los constructores de E3 (`register_global`).
pub fn register_e3_globals(agent: &mut Agent, realm: RealmId) {
    let i = agent.realms[realm].intrinsics.clone();
    let ctor_of = |agent: &Agent, proto: Gc<Object>| -> Gc<Object> {
        match agent.heap[proto]
            .props
            .get(&PropertyKey::from("constructor"))
            .and_then(|p| p.to_descriptor().value)
        {
            Some(Value::Object(c)) => c,
            _ => proto,
        }
    };
    let list: Vec<(&'static str, Gc<Object>)> = vec![
        ("Promise", i.promise_ctor),
        ("Map", i.map_ctor),
        ("Set", i.set_ctor),
        ("WeakMap", ctor_of(agent, i.weak_map_prototype)),
        ("WeakSet", ctor_of(agent, i.weak_set_prototype)),
        ("WeakRef", ctor_of(agent, i.weak_ref_prototype)),
        (
            "FinalizationRegistry",
            ctor_of(agent, i.finalization_registry_prototype),
        ),
        ("ArrayBuffer", i.array_buffer_ctor),
        ("SharedArrayBuffer", i.shared_array_buffer_ctor),
        ("DataView", ctor_of(agent, i.data_view_prototype)),
        ("Atomics", i.atomics),
        ("Int8Array", i.int8_array_ctor),
        ("Uint8Array", i.uint8_array_ctor),
        ("Uint8ClampedArray", i.uint8_clamped_array_ctor),
        ("Int16Array", i.int16_array_ctor),
        ("Uint16Array", i.uint16_array_ctor),
        ("Int32Array", i.int32_array_ctor),
        ("Uint32Array", i.uint32_array_ctor),
        ("Float16Array", i.float16_array_ctor),
        ("Float32Array", i.float32_array_ctor),
        ("Float64Array", i.float64_array_ctor),
        ("BigInt64Array", i.bigint64_array_ctor),
        ("BigUint64Array", i.biguint64_array_ctor),
    ];
    for (name, v) in list {
        crate::builtins::register_global(agent, realm, name, v);
    }
}

/// Promise ( executor ) (§27.2.3.1).
fn promise_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Promise constructor cannot be invoked without 'new'");
    };
    let executor = a.arg(0);
    if !agent.is_callable(&executor) {
        return agent.type_error("Promise resolver is not a function");
    }
    let p = agent.ordinary_create_from_constructor(
        nt,
        |i| i.promise_prototype,
        ObjectKind::Promise(Box::default()),
    )?;
    let (resolve, reject) = agent.create_resolving_functions(p);
    match agent.call(&executor, Value::Undefined, &[resolve, reject.clone()]) {
        Ok(_) => {}
        Err(Throw::Value(e)) => {
            agent.call(&reject, Value::Undefined, &[e])?;
        }
        Err(t) => return Err(t),
    }
    Ok(Value::Object(p))
}

/// get Promise [ @@species ] (§27.2.4.9).
fn species_getter(_: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(a.this.clone())
}

/// Promise.prototype.then (§27.2.5.4).
fn then(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(p) = promise_of(agent, &a.this) else {
        return agent.type_error("Promise.prototype.then called on incompatible receiver");
    };
    let default = agent.intrinsics().promise_ctor;
    let c = agent.species_constructor(p, default)?;
    let cap = agent.new_promise_capability_hidden(c)?;
    let on_f = js_handler(agent, &a.arg(0));
    let on_r = js_handler(agent, &a.arg(1));
    Ok(agent.perform_promise_then(p, on_f, on_r, Some(cap)))
}

/// Promise.prototype.catch (§27.2.5.1).
fn catch(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    agent.invoke(
        &a.this,
        &PropertyKey::from("then"),
        &[Value::Undefined, a.arg(0)],
    )
}

/// Promise.prototype.finally (§27.2.5.3).
fn finally(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Value::Object(p) = a.this else {
        return agent.type_error("Promise.prototype.finally called on a non-object");
    };
    let default = agent.intrinsics().promise_ctor;
    let c = agent.species_constructor(p, default)?;
    let on_finally = a.arg(0);
    let (then_finally, catch_finally) = if !agent.is_callable(&on_finally) {
        (on_finally.clone(), on_finally)
    } else {
        let t = agent.create_builtin_with_slots(
            then_finally_fn,
            1,
            "",
            vec![on_finally.clone(), Value::Object(c)],
        );
        let k = agent.create_builtin_with_slots(
            catch_finally_fn,
            1,
            "",
            vec![on_finally, Value::Object(c)],
        );
        (Value::Object(t), Value::Object(k))
    };
    agent.invoke(
        &a.this,
        &PropertyKey::from("then"),
        &[then_finally, catch_finally],
    )
}

/// thenFinallyClosure (§27.2.5.3 paso 6.a).
fn then_finally_fn(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = slots(agent, a.callee);
    let result = agent.call(&s[0], Value::Undefined, &[])?;
    let c = s[1].as_object().expect("constructor");
    let p = agent.promise_resolve(c, result)?;
    let thunk = agent.create_builtin_with_slots(return_slot, 0, "", vec![a.arg(0)]);
    agent.invoke(&p, &PropertyKey::from("then"), &[Value::Object(thunk)])
}

/// catchFinallyClosure (§27.2.5.3 paso 6.c).
fn catch_finally_fn(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = slots(agent, a.callee);
    let result = agent.call(&s[0], Value::Undefined, &[])?;
    let c = s[1].as_object().expect("constructor");
    let p = agent.promise_resolve(c, result)?;
    let thrower = agent.create_builtin_with_slots(throw_slot, 0, "", vec![a.arg(0)]);
    agent.invoke(&p, &PropertyKey::from("then"), &[Value::Object(thrower)])
}

fn return_slot(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(slots(agent, a.callee)[0].clone())
}

fn throw_slot(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Err(Throw::Value(slots(agent, a.callee)[0].clone()))
}

/// Promise.resolve (§27.2.4.7).
fn resolve(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Value::Object(c) = a.this else {
        return agent.type_error("Promise.resolve called on a non-object");
    };
    agent.promise_resolve(c, a.arg(0))
}

/// Promise.reject (§27.2.4.6).
fn reject(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let cap = agent.new_promise_capability(&a.this)?;
    agent.capability_reject(&cap, a.arg(0))?;
    Ok(cap.promise())
}

/// Promise.withResolvers (§27.2.4.8).
fn with_resolvers(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let cap = agent.new_promise_capability(&a.this)?;
    let Capability::Js {
        promise,
        resolve,
        reject,
    } = cap
    else {
        unreachable!("capability de JS")
    };
    let o = agent.new_plain_object();
    agent.create_data_property_or_throw(o, PropertyKey::from("promise"), promise)?;
    agent.create_data_property_or_throw(o, PropertyKey::from("resolve"), resolve)?;
    agent.create_data_property_or_throw(o, PropertyKey::from("reject"), reject)?;
    Ok(Value::Object(o))
}

/// Promise.try (§27.2.4.8). Con el cambio normativo de 2026 (test262
/// `try/avoids-wrap.js`), un resultado normal pasa por PromiseResolve(C, valor): una
/// promesa de C se devuelve sin envolver.
fn try_(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Value::Object(c) = a.this else {
        return agent.type_error("Promise.try called on a non-object");
    };
    let args = a.args.get(1..).unwrap_or(&[]);
    match agent.call(&a.arg(0), Value::Undefined, args) {
        Ok(v) => {
            if !agent.is_constructor(&a.this) {
                return agent.type_error("Promise.try called on a non-constructor");
            }
            agent.promise_resolve(c, v)
        }
        Err(Throw::Value(e)) => {
            let cap = agent.new_promise_capability(&a.this)?;
            agent.capability_reject(&cap, e)?;
            Ok(cap.promise())
        }
        Err(t) => Err(t),
    }
}

// --- Combinadores (§27.2.4.1–5) ---

#[derive(Clone, Copy, PartialEq, Eq)]
enum Combinator {
    All,
    AllSettled,
    Any,
    Race,
}

fn all(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    combinator(agent, a, Combinator::All)
}

fn all_settled(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    combinator(agent, a, Combinator::AllSettled)
}

fn any(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    combinator(agent, a, Combinator::Any)
}

fn race(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    combinator(agent, a, Combinator::Race)
}

/// Los pasos comunes de Promise.all/allSettled/any/race.
fn combinator(agent: &mut Agent, a: &CallArgs<'_>, kind: Combinator) -> JsResult<Value> {
    let c = a.this.clone();
    let cap = agent.new_promise_capability(&c)?;
    // GetPromiseResolve (§27.2.4.1.1).
    let promise_resolve = match c.as_object() {
        Some(co) => match agent.get(co, &PropertyKey::from("resolve"), c.clone()) {
            Ok(r) if agent.is_callable(&r) => r,
            Ok(_) => {
                let e = agent.error(ErrorType::TypeError, "Promise resolve is not a function");
                return agent.if_abrupt_reject(e, &cap);
            }
            Err(e) => return agent.if_abrupt_reject(e, &cap),
        },
        None => return agent.type_error("not a constructor"),
    };
    let (it, next) = match agent.get_iterator(&a.arg(0)) {
        Ok(r) => r,
        Err(e) => return agent.if_abrupt_reject(e, &cap),
    };
    let mut done = false;
    let r = perform_combinator(
        agent,
        kind,
        &it,
        &next,
        &mut done,
        &c,
        &cap,
        &promise_resolve,
    );
    match r {
        Ok(v) => Ok(v),
        Err(Throw::Value(e)) => {
            if !done {
                agent.iterator_close_on_throw(&it);
            }
            agent.if_abrupt_reject(Throw::Value(e), &cap)
        }
        Err(t) => Err(t),
    }
}

/// IteratorStepValue (§7.4.8) con `[[Done]]`.
fn step_value(
    agent: &mut Agent,
    it: &Value,
    next: &Value,
    done: &mut bool,
) -> JsResult<Option<Value>> {
    *done = true;
    let r = agent.call(next, it.clone(), &[])?;
    let Value::Object(r) = r else {
        return agent.type_error("Iterator result is not an object");
    };
    if agent.iterator_complete(r)? {
        return Ok(None);
    }
    let v = agent.iterator_value(r)?;
    *done = false;
    Ok(Some(v))
}

/// Una lista interna (un Array sin prototipo que nunca se expone).
fn new_list(agent: &mut Agent) -> Gc<Object> {
    agent.alloc(Object::new(
        None,
        ObjectKind::Array(ArrayData {
            dense: Vec::new(),
            length: 0,
            length_writable: true,
        }),
    ))
}

fn list_push(agent: &mut Agent, list: Gc<Object>, v: Value) {
    if let ObjectKind::Array(d) = &mut agent.heap[list].kind {
        d.dense.push(Some(v));
        d.length += 1;
    }
}

fn list_set(agent: &mut Agent, list: Gc<Object>, i: usize, v: Value) {
    if let ObjectKind::Array(d) = &mut agent.heap[list].kind
        && i < d.dense.len()
    {
        d.dense[i] = Some(v);
    }
}

fn list_values(agent: &Agent, list: Gc<Object>) -> Vec<Value> {
    match &agent.heap[list].kind {
        ObjectKind::Array(d) => d
            .dense
            .iter()
            .map(|v| v.clone().unwrap_or_default())
            .collect(),
        _ => Vec::new(),
    }
}

/// Un contador compartido (`remainingElementsCount`).
fn new_counter(agent: &mut Agent, n: f64) -> Gc<Object> {
    agent.alloc(Object::new(None, ObjectKind::Number(n)))
}

/// Suma `delta` al contador y devuelve el valor nuevo.
fn counter_add(agent: &mut Agent, c: Gc<Object>, delta: f64) -> f64 {
    if let ObjectKind::Number(n) = &mut agent.heap[c].kind {
        *n += delta;
        return *n;
    }
    0.0
}

fn cap_values(cap: &Capability) -> [Value; 3] {
    match cap {
        Capability::Js {
            promise,
            resolve,
            reject,
        } => [promise.clone(), resolve.clone(), reject.clone()],
        Capability::Internal(p) => [Value::Object(*p), Value::Undefined, Value::Undefined],
    }
}

fn cap_from_slots(s: &[Value]) -> Capability {
    Capability::Js {
        promise: s[0].clone(),
        resolve: s[1].clone(),
        reject: s[2].clone(),
    }
}

/// Un AggregateError nuevo con `errors` (§27.2.4.3.1 paso 8.b.ii).
fn aggregate_error(agent: &mut Agent, errors: &[Value]) -> JsResult<Value> {
    let proto = agent.intrinsics().aggregate_error_prototype;
    let e = agent.alloc(Object::new(Some(proto), ObjectKind::Error));
    agent.attach_stack(e);
    let arr = agent.create_array_from_list(errors)?;
    agent.put_data(
        e,
        PropertyKey::from("errors"),
        Value::Object(arr),
        true,
        false,
        true,
    );
    Ok(Value::Object(e))
}

#[allow(clippy::too_many_arguments)]
fn perform_combinator(
    agent: &mut Agent,
    kind: Combinator,
    it: &Value,
    next: &Value,
    done: &mut bool,
    c: &Value,
    cap: &Capability,
    promise_resolve: &Value,
) -> JsResult<Value> {
    let then_key = PropertyKey::from("then");
    let [cp, cres, crej] = cap_values(cap);
    let list = new_list(agent);
    let remaining = new_counter(agent, 1.0);
    let mut index = 0usize;
    loop {
        let Some(v) = step_value(agent, it, next, done)? else {
            if kind == Combinator::Race {
                return Ok(cap.promise());
            }
            if counter_add(agent, remaining, -1.0) == 0.0 {
                let values = list_values(agent, list);
                if kind == Combinator::Any {
                    let e = aggregate_error(agent, &values)?;
                    return Err(Throw::Value(e));
                }
                let arr = agent.create_array_from_list(&values)?;
                agent.capability_resolve(cap, Value::Object(arr))?;
            }
            return Ok(cap.promise());
        };
        if kind != Combinator::Race {
            list_push(agent, list, Value::Undefined);
        }
        let next_promise = agent.call(promise_resolve, c.clone(), &[v])?;
        let element = |agent: &mut Agent, f: crate::vm::function::NativeFn, flag: &Value| {
            Value::Object(agent.create_builtin_with_slots(
                f,
                1,
                "",
                vec![
                    flag.clone(),
                    Value::Number(index as f64),
                    Value::Object(list),
                    cp.clone(),
                    cres.clone(),
                    crej.clone(),
                    Value::Object(remaining),
                ],
            ))
        };
        let (on_f, on_r) = match kind {
            Combinator::All => {
                let flag = new_flag(agent);
                (element(agent, all_resolve_element, &flag), crej.clone())
            }
            Combinator::AllSettled => {
                let flag = new_flag(agent);
                (
                    element(agent, settled_resolve_element, &flag),
                    element(agent, settled_reject_element, &flag),
                )
            }
            Combinator::Any => {
                let flag = new_flag(agent);
                (cres.clone(), element(agent, any_reject_element, &flag))
            }
            Combinator::Race => (cres.clone(), crej.clone()),
        };
        if kind != Combinator::Race {
            counter_add(agent, remaining, 1.0);
        }
        agent.invoke(&next_promise, &then_key, &[on_f, on_r])?;
        index += 1;
    }
}

/// Los slots de una element function: (bandera, índice, lista, capability, contador).
fn element_slots(
    agent: &mut Agent,
    f: Gc<Object>,
) -> Option<(usize, Gc<Object>, Capability, Gc<Object>)> {
    let s = slots(agent, f);
    if test_and_set(agent, &s[0]) {
        return None;
    }
    let index = s[1].as_number().unwrap_or(0.0) as usize;
    let list = s[2].as_object()?;
    let remaining = s[6].as_object()?;
    Some((index, list, cap_from_slots(&s[3..6]), remaining))
}

/// Promise.all Resolve Element Functions (§27.2.4.1.3).
fn all_resolve_element(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some((index, list, cap, remaining)) = element_slots(agent, a.callee) else {
        return Ok(Value::Undefined);
    };
    list_set(agent, list, index, a.arg(0));
    if counter_add(agent, remaining, -1.0) == 0.0 {
        let values = list_values(agent, list);
        let arr = agent.create_array_from_list(&values)?;
        agent.capability_resolve(&cap, Value::Object(arr))?;
    }
    Ok(Value::Undefined)
}

fn settled_element(agent: &mut Agent, a: &CallArgs<'_>, fulfilled: bool) -> JsResult<Value> {
    let Some((index, list, cap, remaining)) = element_slots(agent, a.callee) else {
        return Ok(Value::Undefined);
    };
    let o = agent.new_plain_object();
    let (status, key) = if fulfilled {
        ("fulfilled", "value")
    } else {
        ("rejected", "reason")
    };
    agent.create_data_property_or_throw(o, PropertyKey::from("status"), Value::str(status))?;
    agent.create_data_property_or_throw(o, PropertyKey::from(key), a.arg(0))?;
    list_set(agent, list, index, Value::Object(o));
    if counter_add(agent, remaining, -1.0) == 0.0 {
        let values = list_values(agent, list);
        let arr = agent.create_array_from_list(&values)?;
        agent.capability_resolve(&cap, Value::Object(arr))?;
    }
    Ok(Value::Undefined)
}

/// Promise.allSettled Resolve Element Functions (§27.2.4.2.2).
fn settled_resolve_element(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    settled_element(agent, a, true)
}

/// Promise.allSettled Reject Element Functions (§27.2.4.2.3).
fn settled_reject_element(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    settled_element(agent, a, false)
}

/// Promise.any Reject Element Functions (§27.2.4.3.2).
fn any_reject_element(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some((index, list, cap, remaining)) = element_slots(agent, a.callee) else {
        return Ok(Value::Undefined);
    };
    list_set(agent, list, index, a.arg(0));
    if counter_add(agent, remaining, -1.0) == 0.0 {
        let values = list_values(agent, list);
        let e = aggregate_error(agent, &values)?;
        agent.capability_reject(&cap, e)?;
    }
    Ok(Value::Undefined)
}
