//! Funciones async (§27.7), generadores async (§27.6), %AsyncIteratorPrototype% (§27.1.3),
//! %AsyncFromSyncIteratorPrototype% (§27.1.6) y los constructores %AsyncFunction% y
//! %AsyncGeneratorFunction% (§27.4, §27.7.1).
//!
//! `await` reutiliza la suspensión de frames de los generadores: el intérprete devuelve
//! [`FrameExit::Await`] y el frame queda guardado en un objeto interno
//! ([`ObjectKind::AsyncFunction`]) o en el generador async; la reacción de la promesa
//! esperada ([`Handler::AsyncResume`]) lo reanuda desde un job.

use std::collections::VecDeque;

use crate::builtins::promise::{Capability, Handler};
use crate::builtins::{FnEntry, method_sym, methods, to_string_tag};
use crate::heap::{Gc, Trace, Tracer};
use crate::object::{Object, ObjectKind};
use crate::value::{PropertyKey, Value};
use crate::vm::eval::DynamicKind;
use crate::vm::frame::Frame;
use crate::vm::function::{CallArgs, resume_frame};
use crate::vm::interp::{FrameExit, Resume};
use crate::vm::{Agent, ErrorType, JsResult, RealmId, Throw};

/// El estado de una función async suspendida en un `await`.
#[derive(Debug)]
pub struct AsyncFnData {
    pub frame: Option<Box<Frame>>,
    /// La promesa que devolvió la llamada (la capability de %Promise%).
    pub promise: Gc<Object>,
}

impl Trace for AsyncFnData {
    fn trace(&self, t: &mut Tracer) {
        if let Some(f) = &self.frame {
            f.trace(t);
        }
        t.gc(self.promise);
    }
}

/// `[[AsyncGeneratorState]]` (§27.6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsyncGenState {
    SuspendedStart,
    SuspendedYield,
    Executing,
    DrainingQueue,
    Completed,
}

/// AsyncGeneratorRequest Record (§27.6.3.1).
#[derive(Debug)]
pub struct AsyncGenRequest {
    /// `Next`, `Throw` o `Return`.
    pub completion: Resume,
    pub capability: Capability,
}

#[derive(Debug)]
pub struct AsyncGenData {
    pub state: AsyncGenState,
    pub frame: Option<Box<Frame>>,
    pub queue: VecDeque<AsyncGenRequest>,
}

impl Trace for AsyncGenData {
    fn trace(&self, t: &mut Tracer) {
        if let Some(f) = &self.frame {
            f.trace(t);
        }
        for r in &self.queue {
            match &r.completion {
                Resume::Next(v) | Resume::Throw(v) | Resume::Return(v) => t.value(v),
                Resume::Normal => {}
            }
            r.capability.trace(t);
        }
    }
}

/// `[[SyncIteratorRecord]]` de un AsyncFromSyncIterator.
#[derive(Debug)]
pub struct SyncIteratorRecord {
    pub iterator: Value,
    pub next: Value,
}

impl Trace for SyncIteratorRecord {
    fn trace(&self, t: &mut Tracer) {
        t.value(&self.iterator);
        t.value(&self.next);
    }
}

// --- Funciones async ---

/// [[Call]] de una función async (§10.2.1 + EvaluateAsyncFunctionBody, §15.8.4): el cuerpo
/// corre hasta el primer `await` (AsyncFunctionStart, §27.7.5.1) y la llamada devuelve la
/// promesa.
pub(crate) fn async_function_start(agent: &mut Agent, frame: Frame) -> JsResult<Value> {
    let promise = agent.in_realm(frame.realm, |a| a.new_internal_promise());
    let scope = agent.heap.open_scope();
    agent.heap.root(Value::Object(promise));
    let r = drive_async_function(agent, None, promise, frame, Resume::Normal);
    agent.heap.close_scope(scope);
    r?;
    Ok(Value::Object(promise))
}

/// Corre (o reanuda) el frame de una función async hasta que termina o se suspende en un
/// `await` (AsyncBlockStart, §27.7.5.2).
fn drive_async_function(
    agent: &mut Agent,
    mut state: Option<Gc<Object>>,
    promise: Gc<Object>,
    mut frame: Frame,
    mut mode: Resume,
) -> JsResult<()> {
    loop {
        let realm = frame.realm;
        let (r, f) = resume_frame(agent, frame, mode);
        match r {
            Ok(FrameExit::Return(v)) => {
                return agent.in_realm(realm, |a| a.resolve_promise(promise, v));
            }
            Err(Throw::Value(e)) => {
                agent.in_realm(realm, |a| a.reject_promise(promise, e));
                return Ok(());
            }
            Err(t) => return Err(t),
            Ok(FrameExit::Await(v)) => {
                let s = match state {
                    Some(s) => s,
                    None => {
                        let s = agent.alloc(Object::new(
                            None,
                            ObjectKind::AsyncFunction(Box::new(AsyncFnData {
                                frame: None,
                                promise,
                            })),
                        ));
                        state = Some(s);
                        s
                    }
                };
                if let ObjectKind::AsyncFunction(d) = &mut agent.heap[s].kind {
                    d.frame = Some(Box::new(f));
                }
                match agent.in_realm(realm, |a| a.await_value(s, v, false)) {
                    Ok(()) => return Ok(()),
                    Err(Throw::Value(e)) => {
                        // `? PromiseResolve(...)` lanzó: la excepción sale del `await`.
                        frame = take_async_frame(agent, s).expect("frame async");
                        mode = Resume::Throw(e);
                    }
                    Err(t) => return Err(t),
                }
            }
            Ok(FrameExit::Yield(..) | FrameExit::GeneratorStart(_)) => {
                unreachable!("yield en una función async")
            }
        }
    }
}

fn take_async_frame(agent: &mut Agent, s: Gc<Object>) -> Option<Frame> {
    match &mut agent.heap[s].kind {
        ObjectKind::AsyncFunction(d) => d.frame.take().map(|f| *f),
        ObjectKind::AsyncGenerator(d) => d.frame.take().map(|f| *f),
        _ => None,
    }
}

impl Agent {
    /// Await (§27.7.5.3) pasos 2–9 desde el contexto suspendido `target`: la reanudación la
    /// hacen las reacciones. Devuelve el error de PromiseResolve, que sale del `await`.
    pub(crate) fn await_value(&mut self, target: Gc<Object>, v: Value, ret: bool) -> JsResult<()> {
        let pc = self.intrinsics().promise_ctor;
        let p = self.promise_resolve(pc, v)?;
        let p = p.as_object().expect("promesa");
        let h = Handler::AsyncResume { target, ret };
        self.perform_promise_then(p, h.clone(), h, None);
        Ok(())
    }

    /// GetIterator(obj, async) (§7.4.3): (iterador, next).
    pub fn get_async_iterator(&mut self, obj: &Value) -> JsResult<(Value, Value)> {
        let key = PropertyKey::Symbol(self.well_known.async_iterator);
        let method = self.get_method_of(obj, &key)?;
        match method {
            Some(m) => self.get_iterator_from_method(obj, &m),
            None => {
                let key = PropertyKey::Symbol(self.well_known.iterator);
                let Some(sync) = self.get_method_of(obj, &key)? else {
                    return self
                        .type_error(&format!("{} is not async iterable", self.describe(obj)));
                };
                let (it, next) = self.get_iterator_from_method(obj, &sync)?;
                Ok(self.create_async_from_sync_iterator(it, next))
            }
        }
    }

    /// CreateAsyncFromSyncIterator (§27.1.6.1): (iterador async, next).
    pub fn create_async_from_sync_iterator(
        &mut self,
        iterator: Value,
        next: Value,
    ) -> (Value, Value) {
        let proto = self.intrinsics().async_from_sync_iterator_prototype;
        let o = self.alloc(Object::new(
            Some(proto),
            ObjectKind::AsyncFromSyncIterator(Box::new(SyncIteratorRecord { iterator, next })),
        ));
        // `! Get(asyncIterator, "next")`: el método del prototipo.
        let k = self.keys.next.clone();
        let next = self.heap[proto]
            .props
            .get(&k)
            .and_then(|p| p.to_descriptor().value)
            .unwrap_or_default();
        (Value::Object(o), next)
    }
}

/// La reacción a la promesa esperada: reanuda la función o el generador async.
pub(crate) fn resume_after_await(
    agent: &mut Agent,
    target: Gc<Object>,
    fulfilled: bool,
    value: Value,
    ret: bool,
) -> JsResult<()> {
    let mode = match (fulfilled, ret) {
        (false, _) => Resume::Throw(value),
        (true, false) => Resume::Next(value),
        (true, true) => Resume::Return(value),
    };
    match &agent.heap[target].kind {
        ObjectKind::AsyncFunction(d) => {
            let promise = d.promise;
            let Some(frame) = take_async_frame(agent, target) else {
                return Ok(());
            };
            drive_async_function(agent, Some(target), promise, frame, mode)
        }
        ObjectKind::AsyncGenerator(_) => async_gen_drive(agent, target, mode),
        _ => Ok(()),
    }
}

// --- Generadores async (§27.6.3) ---

/// Crea el objeto generador async (OrdinaryCreateFromConstructor(functionObject,
/// "%AsyncGeneratorPrototype%"), §15.6.2 EvaluateAsyncGeneratorBody).
pub(crate) fn create_async_generator(agent: &mut Agent, f: Gc<Object>) -> JsResult<Gc<Object>> {
    agent.ordinary_create_from_constructor(
        f,
        |i| i.async_generator_prototype,
        ObjectKind::AsyncGenerator(Box::new(AsyncGenData {
            state: AsyncGenState::SuspendedStart,
            frame: None,
            queue: VecDeque::new(),
        })),
    )
}

fn gen_data(agent: &mut Agent, g: Gc<Object>) -> &mut AsyncGenData {
    match &mut agent.heap[g].kind {
        ObjectKind::AsyncGenerator(d) => d,
        _ => unreachable!("generador async"),
    }
}

fn gen_state(agent: &Agent, g: Gc<Object>) -> AsyncGenState {
    match &agent.heap[g].kind {
        ObjectKind::AsyncGenerator(d) => d.state,
        _ => AsyncGenState::Completed,
    }
}

fn set_gen_state(agent: &mut Agent, g: Gc<Object>, s: AsyncGenState) {
    let d = gen_data(agent, g);
    d.state = s;
    if s == AsyncGenState::Completed || s == AsyncGenState::DrainingQueue {
        d.frame = None;
    }
}

fn gen_realm(agent: &Agent, g: Gc<Object>) -> RealmId {
    match &agent.heap[g].kind {
        ObjectKind::AsyncGenerator(d) => d.frame.as_ref().map_or(agent.current_realm, |f| f.realm),
        _ => agent.current_realm,
    }
}

/// AsyncGeneratorResume (§27.6.3.6) con la completion de un pedido. Una return completion
/// llega a un `yield` y la desenvuelve AsyncGeneratorUnwrapYieldResumption (§27.6.3.7): se
/// espera el valor antes de reanudar.
fn async_gen_resume(agent: &mut Agent, g: Gc<Object>, completion: Resume) -> JsResult<()> {
    if let Resume::Return(x) = completion {
        set_gen_state(agent, g, AsyncGenState::Executing);
        let realm = gen_realm(agent, g);
        return match agent.in_realm(realm, |a| a.await_value(g, x, true)) {
            Ok(()) => Ok(()),
            Err(Throw::Value(e)) => async_gen_drive(agent, g, Resume::Throw(e)),
            Err(t) => Err(t),
        };
    }
    async_gen_drive(agent, g, completion)
}

/// Corre el frame del generador async hasta que se suspende (en un `await` o en un `yield`
/// sin pedidos pendientes) o termina.
fn async_gen_drive(agent: &mut Agent, g: Gc<Object>, mut mode: Resume) -> JsResult<()> {
    loop {
        let frame = {
            let d = gen_data(agent, g);
            d.state = AsyncGenState::Executing;
            d.frame.take()
        };
        let Some(frame) = frame else {
            return Ok(());
        };
        let realm = frame.realm;
        let (r, f) = resume_frame(agent, *frame, mode);
        match r {
            Ok(FrameExit::Await(v)) => {
                gen_data(agent, g).frame = Some(Box::new(f));
                match agent.in_realm(realm, |a| a.await_value(g, v, false)) {
                    Ok(()) => return Ok(()),
                    Err(Throw::Value(e)) => mode = Resume::Throw(e),
                    Err(t) => return Err(t),
                }
            }
            Ok(FrameExit::Yield(v, _)) => {
                // AsyncGeneratorYield (§27.6.3.8).
                gen_data(agent, g).frame = Some(Box::new(f));
                let previous_realm = agent.current_realm;
                async_gen_complete_step(agent, g, Ok(v), false, Some(previous_realm))?;
                let next = gen_data(agent, g)
                    .queue
                    .front()
                    .map(|r| r.completion.clone());
                match next {
                    None => {
                        gen_data(agent, g).state = AsyncGenState::SuspendedYield;
                        return Ok(());
                    }
                    Some(Resume::Return(x)) => {
                        match agent.in_realm(realm, |a| a.await_value(g, x, true)) {
                            Ok(()) => return Ok(()),
                            Err(Throw::Value(e)) => mode = Resume::Throw(e),
                            Err(t) => return Err(t),
                        }
                    }
                    Some(c) => mode = c,
                }
            }
            Ok(FrameExit::Return(v)) => {
                set_gen_state(agent, g, AsyncGenState::DrainingQueue);
                async_gen_complete_step(agent, g, Ok(v), true, None)?;
                return async_gen_drain_queue(agent, g);
            }
            Err(Throw::Value(e)) => {
                set_gen_state(agent, g, AsyncGenState::DrainingQueue);
                async_gen_complete_step(agent, g, Err(e), true, None)?;
                return async_gen_drain_queue(agent, g);
            }
            Err(t) => {
                set_gen_state(agent, g, AsyncGenState::Completed);
                return Err(t);
            }
            Ok(FrameExit::GeneratorStart(_)) => unreachable!("GeneratorStart al reanudar"),
        }
    }
}

/// AsyncGeneratorCompleteStep (§27.6.3.4).
fn async_gen_complete_step(
    agent: &mut Agent,
    g: Gc<Object>,
    completion: Result<Value, Value>,
    done: bool,
    realm: Option<RealmId>,
) -> JsResult<()> {
    let Some(req) = gen_data(agent, g).queue.pop_front() else {
        return Ok(());
    };
    match completion {
        Err(e) => agent.capability_reject(&req.capability, e),
        Ok(v) => {
            let r = match realm {
                Some(r) => agent.in_realm(r, |a| a.create_iter_result(v, done))?,
                None => agent.create_iter_result(v, done)?,
            };
            agent.capability_resolve(&req.capability, r)
        }
    }
}

/// AsyncGeneratorDrainQueue (§27.6.3.10).
fn async_gen_drain_queue(agent: &mut Agent, g: Gc<Object>) -> JsResult<()> {
    loop {
        let front = gen_data(agent, g)
            .queue
            .front()
            .map(|r| r.completion.clone());
        match front {
            None => {
                set_gen_state(agent, g, AsyncGenState::Completed);
                return Ok(());
            }
            Some(Resume::Return(_)) => return async_gen_await_return(agent, g),
            Some(c) => {
                let completion = match c {
                    Resume::Throw(e) => Err(e),
                    _ => Ok(Value::Undefined),
                };
                async_gen_complete_step(agent, g, completion, true, None)?;
            }
        }
    }
}

/// AsyncGeneratorAwaitReturn (§27.6.3.9).
fn async_gen_await_return(agent: &mut Agent, g: Gc<Object>) -> JsResult<()> {
    let value = match gen_data(agent, g)
        .queue
        .front()
        .map(|r| r.completion.clone())
    {
        Some(Resume::Return(v)) => v,
        _ => return Ok(()),
    };
    let pc = agent.intrinsics().promise_ctor;
    match agent.promise_resolve(pc, value) {
        Ok(p) => {
            let p = p.as_object().expect("promesa");
            let h = Handler::AsyncGenAwaitReturn(g);
            agent.perform_promise_then(p, h.clone(), h, None);
            Ok(())
        }
        Err(Throw::Value(e)) => {
            async_gen_complete_step(agent, g, Err(e), true, None)?;
            async_gen_drain_queue(agent, g)
        }
        Err(t) => Err(t),
    }
}

/// Las funciones `fulfilledClosure`/`rejectedClosure` de AsyncGeneratorAwaitReturn.
pub(crate) fn await_return_settled(
    agent: &mut Agent,
    g: Gc<Object>,
    fulfilled: bool,
    value: Value,
) -> JsResult<()> {
    let completion = if fulfilled { Ok(value) } else { Err(value) };
    async_gen_complete_step(agent, g, completion, true, None)?;
    async_gen_drain_queue(agent, g)
}

/// AsyncGeneratorValidate (§27.6.3.2) + AsyncGeneratorEnqueue (§27.6.3.3) para
/// next/return/throw: devuelve la promesa del pedido.
fn async_gen_method(agent: &mut Agent, this: &Value, completion: Resume) -> JsResult<Value> {
    // El cuerpo del generador corre sin inhibir el GC: la promesa se enraíza mientras tanto.
    let scope = agent.heap.open_scope();
    let promise = agent.new_internal_promise();
    agent.heap.root(Value::Object(promise));
    agent.heap.root(this.clone());
    let r = async_gen_method_inner(agent, this, completion, promise);
    agent.heap.close_scope(scope);
    r
}

fn async_gen_method_inner(
    agent: &mut Agent,
    this: &Value,
    completion: Resume,
    promise: Gc<Object>,
) -> JsResult<Value> {
    let cap = Capability::Internal(promise);
    let g = match this {
        Value::Object(o) if matches!(agent.heap[*o].kind, ObjectKind::AsyncGenerator(_)) => *o,
        _ => {
            let e = agent.error(ErrorType::TypeError, "not an AsyncGenerator object");
            return agent.if_abrupt_reject(e, &cap);
        }
    };
    let mut state = gen_state(agent, g);
    match &completion {
        Resume::Throw(e) => {
            // %AsyncGeneratorPrototype%.throw pasos 6–7.
            if state == AsyncGenState::SuspendedStart {
                set_gen_state(agent, g, AsyncGenState::Completed);
                state = AsyncGenState::Completed;
            }
            if state == AsyncGenState::Completed {
                agent.capability_reject(&cap, e.clone())?;
                return Ok(Value::Object(promise));
            }
        }
        Resume::Return(_) => {}
        _ => {
            if state == AsyncGenState::Completed {
                let r = agent.create_iter_result(Value::Undefined, true)?;
                agent.capability_resolve(&cap, r)?;
                return Ok(Value::Object(promise));
            }
        }
    }
    gen_data(agent, g).queue.push_back(AsyncGenRequest {
        completion: completion.clone(),
        capability: cap,
    });
    match completion {
        Resume::Return(_) => {
            // %AsyncGeneratorPrototype%.return pasos 8–9.
            if matches!(
                state,
                AsyncGenState::SuspendedStart | AsyncGenState::Completed
            ) {
                set_gen_state(agent, g, AsyncGenState::DrainingQueue);
                async_gen_await_return(agent, g)?;
            } else if state == AsyncGenState::SuspendedYield {
                async_gen_resume(agent, g, completion)?;
            }
        }
        _ => {
            if matches!(
                state,
                AsyncGenState::SuspendedStart | AsyncGenState::SuspendedYield
            ) {
                async_gen_resume(agent, g, completion)?;
            }
        }
    }
    Ok(Value::Object(promise))
}

/// %AsyncGeneratorPrototype%.next (§27.6.1.2).
fn async_gen_next(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    async_gen_method(agent, &a.this, Resume::Next(a.arg(0)))
}

/// %AsyncGeneratorPrototype%.return (§27.6.1.3).
fn async_gen_return(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    async_gen_method(agent, &a.this, Resume::Return(a.arg(0)))
}

/// %AsyncGeneratorPrototype%.throw (§27.6.1.4).
fn async_gen_throw(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    async_gen_method(agent, &a.this, Resume::Throw(a.arg(0)))
}

// --- %AsyncFromSyncIteratorPrototype% (§27.1.6.2) ---

fn sync_record(agent: &Agent, this: &Value) -> Option<(Value, Value)> {
    match this {
        Value::Object(o) => match &agent.heap[*o].kind {
            ObjectKind::AsyncFromSyncIterator(r) => Some((r.iterator.clone(), r.next.clone())),
            _ => None,
        },
        _ => None,
    }
}

/// %AsyncFromSyncIteratorPrototype%.next (§27.1.6.2.1).
fn from_sync_next(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let cap = Capability::Internal(agent.new_internal_promise());
    let Some((it, next)) = sync_record(agent, &a.this) else {
        return agent.type_error("not an AsyncFromSyncIterator");
    };
    let args: Vec<Value> = a.args.first().cloned().into_iter().collect();
    let result = match agent.call(&next, it.clone(), &args) {
        Ok(Value::Object(o)) => o,
        Ok(_) => {
            let e = agent.error(ErrorType::TypeError, "Iterator result is not an object");
            return agent.if_abrupt_reject(e, &cap);
        }
        Err(e) => return agent.if_abrupt_reject(e, &cap),
    };
    async_from_sync_continuation(agent, result, &cap, &it, true)
}

/// %AsyncFromSyncIteratorPrototype%.return (§27.1.6.2.2).
fn from_sync_return(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let cap = Capability::Internal(agent.new_internal_promise());
    let Some((it, _)) = sync_record(agent, &a.this) else {
        return agent.type_error("not an AsyncFromSyncIterator");
    };
    let k = agent.keys.return_.clone();
    let ret = match agent.get_method_of(&it, &k) {
        Ok(r) => r,
        Err(e) => return agent.if_abrupt_reject(e, &cap),
    };
    let Some(ret) = ret else {
        let r = agent.create_iter_result(a.arg(0), true)?;
        agent.capability_resolve(&cap, r)?;
        return Ok(cap.promise());
    };
    let args: Vec<Value> = a.args.first().cloned().into_iter().collect();
    let result = match agent.call(&ret, it.clone(), &args) {
        Ok(Value::Object(o)) => o,
        Ok(_) => {
            let e = agent.error(
                ErrorType::TypeError,
                "iterator.return() did not return an object",
            );
            return agent.if_abrupt_reject(e, &cap);
        }
        Err(e) => return agent.if_abrupt_reject(e, &cap),
    };
    async_from_sync_continuation(agent, result, &cap, &it, false)
}

/// %AsyncFromSyncIteratorPrototype%.throw (§27.1.6.2.3).
fn from_sync_throw(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let cap = Capability::Internal(agent.new_internal_promise());
    let Some((it, _)) = sync_record(agent, &a.this) else {
        return agent.type_error("not an AsyncFromSyncIterator");
    };
    let k = agent.keys.throw.clone();
    let throw = match agent.get_method_of(&it, &k) {
        Ok(t) => t,
        Err(e) => return agent.if_abrupt_reject(e, &cap),
    };
    let Some(throw) = throw else {
        // Paso 6: se cierra el iterador y se rechaza con un TypeError.
        if let Err(e) = agent.iterator_close(&it) {
            return agent.if_abrupt_reject(e, &cap);
        }
        let e = agent.make_error(
            ErrorType::TypeError,
            "The iterator does not provide a 'throw' method",
        );
        agent.capability_reject(&cap, Value::Object(e))?;
        return Ok(cap.promise());
    };
    let args: Vec<Value> = a.args.first().cloned().into_iter().collect();
    let result = match agent.call(&throw, it.clone(), &args) {
        Ok(Value::Object(o)) => o,
        Ok(_) => {
            let e = agent.error(
                ErrorType::TypeError,
                "iterator.throw() did not return an object",
            );
            return agent.if_abrupt_reject(e, &cap);
        }
        Err(e) => return agent.if_abrupt_reject(e, &cap),
    };
    async_from_sync_continuation(agent, result, &cap, &it, true)
}

/// AsyncFromSyncIteratorContinuation (§27.1.6.4).
fn async_from_sync_continuation(
    agent: &mut Agent,
    result: Gc<Object>,
    cap: &Capability,
    iterator: &Value,
    close_on_rejection: bool,
) -> JsResult<Value> {
    let done = match agent.iterator_complete(result) {
        Ok(d) => d,
        Err(e) => return agent.if_abrupt_reject(e, cap),
    };
    let value = match agent.iterator_value(result) {
        Ok(v) => v,
        Err(e) => return agent.if_abrupt_reject(e, cap),
    };
    let pc = agent.intrinsics().promise_ctor;
    let wrapper = match agent.promise_resolve(pc, value) {
        Ok(w) => w,
        Err(Throw::Value(e)) => {
            if !done && close_on_rejection {
                // IteratorClose(syncIteratorRecord, valueWrapper) con una throw completion.
                agent.iterator_close_on_throw(iterator);
            }
            return agent.if_abrupt_reject(Throw::Value(e), cap);
        }
        Err(t) => return Err(t),
    };
    let on_rejected = if done || !close_on_rejection {
        Handler::Empty
    } else {
        Handler::CloseSyncIterator(iterator.clone())
    };
    let p = wrapper.as_object().expect("promesa");
    agent.perform_promise_then(
        p,
        Handler::IterResult { done },
        on_rejected,
        Some(cap.clone()),
    );
    Ok(cap.promise())
}

// --- Intrínsecos ---

pub fn init(agent: &mut Agent, realm: RealmId) {
    let i = agent.realms[realm].intrinsics.clone();
    // %AsyncIteratorPrototype% (§27.1.3).
    let aip = agent.new_object(Some(i.object_prototype));
    let ai = agent.well_known.async_iterator;
    method_sym(agent, aip, ai, "[Symbol.asyncIterator]", 0, return_this);
    // %AsyncFromSyncIteratorPrototype% (§27.1.6.2).
    let afs = agent.new_object(Some(aip));
    const FROM_SYNC: &[FnEntry] = &[
        ("next", 1, from_sync_next),
        ("return", 1, from_sync_return),
        ("throw", 1, from_sync_throw),
    ];
    methods(agent, afs, FROM_SYNC);
    // %AsyncFunction.prototype% (§27.7.3) y %AsyncFunction% (§27.7.1).
    let afp = agent.new_object(Some(i.function_prototype));
    to_string_tag(agent, afp, "AsyncFunction");
    let af = agent.create_builtin_function(async_function_ctor, 1, "AsyncFunction", true);
    agent.heap[af].proto = Some(i.function_ctor);
    let kp = agent.keys.prototype.clone();
    let kc = agent.keys.constructor.clone();
    agent.put_data(af, kp.clone(), Value::Object(afp), false, false, false);
    agent.put_data(afp, kc.clone(), Value::Object(af), false, false, true);
    // %AsyncGeneratorPrototype% (§27.6.1).
    let agp = agent.new_object(Some(aip));
    const GEN: &[FnEntry] = &[
        ("next", 1, async_gen_next),
        ("return", 1, async_gen_return),
        ("throw", 1, async_gen_throw),
    ];
    methods(agent, agp, GEN);
    to_string_tag(agent, agp, "AsyncGenerator");
    // %AsyncGeneratorFunction.prototype% (§27.4.3) y %AsyncGeneratorFunction% (§27.4.1).
    let agfp = agent.new_object(Some(i.function_prototype));
    to_string_tag(agent, agfp, "AsyncGeneratorFunction");
    agent.put_data(agfp, kp.clone(), Value::Object(agp), false, false, true);
    agent.put_data(agp, kc.clone(), Value::Object(agfp), false, false, true);
    let agf = agent.create_builtin_function(
        async_generator_function_ctor,
        1,
        "AsyncGeneratorFunction",
        true,
    );
    agent.heap[agf].proto = Some(i.function_ctor);
    agent.put_data(agf, kp, Value::Object(agfp), false, false, false);
    agent.put_data(agfp, kc, Value::Object(agf), false, false, true);
    let ints = &mut agent.realms[realm].intrinsics;
    ints.async_iterator_prototype = aip;
    ints.async_from_sync_iterator_prototype = afs;
    ints.async_function = af;
    ints.async_function_prototype = afp;
    ints.async_generator_prototype = agp;
    ints.async_generator_function = agf;
    ints.async_generator_function_prototype = agfp;
}

fn return_this(_: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(a.this.clone())
}

/// AsyncFunction ( ...parameterArgs, bodyArg ) (§27.7.1.1).
fn async_function_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    agent.create_dynamic_function(a.callee, a.new_target, DynamicKind::Async, a.args)
}

/// AsyncGeneratorFunction ( ...parameterArgs, bodyArg ) (§27.4.1.1).
fn async_generator_function_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    agent.create_dynamic_function(a.callee, a.new_target, DynamicKind::AsyncGenerator, a.args)
}
