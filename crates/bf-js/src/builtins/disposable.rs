//! Explicit Resource Management (ECMA-262 2026): los DisposeCapability Records
//! (NewDisposeCapability, AddDisposableResource, CreateDisposableResource, GetDisposeMethod,
//! DisposeResources), SuppressedError (§20.5.8), DisposableStack (§27.3),
//! AsyncDisposableStack (§27.4) y `%AsyncIteratorPrototype% [ @@asyncDispose ]` (§27.1.3.1).
//!
//! Las declaraciones `using`/`await using` usan un objeto interno
//! [`ObjectKind::DisposeCapability`] por scope (lo crea `Op::DisposeInit`); los
//! DisposableStack guardan el mismo registro. DisposeResources con `await` corre en el
//! intérprete (`Op::DisposeAsyncStep` + `Await`) o, para `disposeAsync`, como una cadena de
//! reacciones de promesas ([`drive_async_stack`]).

use crate::builtins::promise::Handler;
use crate::builtins::{FnEntry, accessor, constructor, methods, slots, to_string_tag};
use crate::heap::{Gc, Trace, Tracer};
use crate::object::{Object, ObjectKind};
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId, Throw};

/// `[[Hint]]` de un DisposableResource Record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    Sync,
    Async,
}

/// DisposableResource Record.
#[derive(Debug, Clone)]
pub struct Resource {
    pub value: Value,
    pub hint: Hint,
    /// `[[DisposeMethod]]` (undefined: un `await using` de null o undefined).
    pub method: Value,
}

/// DisposeCapability Record, con el `[[DisposableState]]` de los stacks y el estado de un
/// DisposeResources asíncrono en curso.
#[derive(Debug, Default)]
pub struct DisposeData {
    /// `[[DisposableResourceStack]]`.
    pub resources: Vec<Resource>,
    /// `[[DisposableState]]` / `[[AsyncDisposableState]]` es disposed.
    pub disposed: bool,
    /// needsAwait y hasAwaited de DisposeResources.
    pub needs_await: bool,
    pub has_awaited: bool,
    /// La completion de un `disposeAsync` en curso (Some: throw).
    pub completion: Option<Value>,
}

impl Trace for DisposeData {
    fn trace(&self, t: &mut Tracer) {
        for r in &self.resources {
            t.value(&r.value);
            t.value(&r.method);
        }
        if let Some(c) = &self.completion {
            t.value(c);
        }
    }
}

fn data(agent: &mut Agent, o: Gc<Object>) -> &mut DisposeData {
    match &mut agent.heap[o].kind {
        ObjectKind::DisposeCapability(d)
        | ObjectKind::DisposableStack(d)
        | ObjectKind::AsyncDisposableStack(d) => d,
        _ => unreachable!("DisposeCapability"),
    }
}

/// Lo que pide un paso de DisposeResources asíncrono.
pub enum AsyncStep {
    /// Esperar este valor (Await) y seguir.
    Await(Value),
    Done,
}

impl Agent {
    /// NewDisposeCapability: el objeto interno de un scope con `using`.
    pub fn new_dispose_capability(&mut self) -> Gc<Object> {
        self.alloc(Object::new(
            None,
            ObjectKind::DisposeCapability(Box::default()),
        ))
    }

    /// GetDisposeMethod ( V, hint ).
    fn get_dispose_method(&mut self, v: &Value, hint: Hint) -> JsResult<Option<Value>> {
        if hint == Hint::Async {
            let k = PropertyKey::Symbol(self.well_known.async_dispose);
            if let Some(m) = self.get_method_of(v, &k)? {
                return Ok(Some(m));
            }
            let k = PropertyKey::Symbol(self.well_known.dispose);
            let Some(m) = self.get_method_of(v, &k)? else {
                return Ok(None);
            };
            // Paso 1.b.ii: el closure que envuelve un @@dispose síncrono.
            let f = self.create_builtin_with_slots(sync_dispose_wrapper, 0, "", vec![m]);
            return Ok(Some(Value::Object(f)));
        }
        let k = PropertyKey::Symbol(self.well_known.dispose);
        self.get_method_of(v, &k)
    }

    /// AddDisposableResource ( disposeCapability, V, hint [ , method ] ).
    pub fn add_disposable_resource(
        &mut self,
        cap: Gc<Object>,
        v: Value,
        hint: Hint,
        method: Option<Value>,
    ) -> JsResult<()> {
        let resource = match method {
            None => {
                if v.is_nullish() && hint == Hint::Sync {
                    return Ok(());
                }
                // CreateDisposableResource ( V, hint ).
                if v.is_nullish() {
                    Resource {
                        value: Value::Undefined,
                        hint,
                        method: Value::Undefined,
                    }
                } else {
                    if !v.is_object() {
                        return self.type_error("using: the resource is not an object");
                    }
                    let Some(m) = self.get_dispose_method(&v, hint)? else {
                        return self.type_error(if hint == Hint::Async {
                            "await using: the resource has no [Symbol.asyncDispose] or [Symbol.dispose] method"
                        } else {
                            "using: the resource has no [Symbol.dispose] method"
                        });
                    };
                    Resource {
                        value: v,
                        hint,
                        method: m,
                    }
                }
            }
            Some(m) => {
                if !self.is_callable(&m) {
                    return self.type_error("The dispose method is not callable");
                }
                Resource {
                    value: Value::Undefined,
                    hint,
                    method: m,
                }
            }
        };
        data(self, cap).resources.push(resource);
        Ok(())
    }

    /// Los pasos 3.e.iii de DisposeResources: combina el error `e` con la completion.
    pub fn merge_dispose_error(&mut self, completion: &mut Option<Value>, e: Value) {
        *completion = Some(match completion.take() {
            Some(suppressed) => Value::Object(self.new_suppressed_error(e, suppressed)),
            None => e,
        });
    }

    /// Un SuppressedError nuevo con `error` y `suppressed` (DisposeResources 3.e.iii.1).
    pub fn new_suppressed_error(&mut self, error: Value, suppressed: Value) -> Gc<Object> {
        let proto = self.intrinsics().suppressed_error_prototype;
        let o = self.alloc(Object::new(Some(proto), ObjectKind::Error));
        self.put_data(o, PropertyKey::from("error"), error, true, false, true);
        self.put_data(
            o,
            PropertyKey::from("suppressed"),
            suppressed,
            true,
            false,
            true,
        );
        self.attach_stack(o);
        o
    }

    /// DisposeResources ( disposeCapability, completion ) sin `await` (todos los recursos son
    /// sync-dispose). `completion`: Some(valor) si es una throw completion.
    pub fn dispose_resources_sync(
        &mut self,
        cap: Gc<Object>,
        mut completion: Option<Value>,
    ) -> JsResult<Option<Value>> {
        while let Some(r) = data(self, cap).resources.pop() {
            if r.method.is_undefined() {
                continue;
            }
            match self.call(&r.method, r.value, &[]) {
                Ok(_) => {}
                Err(Throw::Value(e)) => self.merge_dispose_error(&mut completion, e),
                Err(t) => return Err(t),
            }
        }
        Ok(completion)
    }

    /// Un paso de DisposeResources con `await`: hace las llamadas que no esperan y devuelve
    /// el siguiente valor a esperar (el resultado de un método async-dispose, o el
    /// `Await(undefined)` de los pasos 3.d y 4), o Done.
    pub fn dispose_async_step(
        &mut self,
        cap: Gc<Object>,
        completion: &mut Option<Value>,
    ) -> JsResult<AsyncStep> {
        loop {
            let d = data(self, cap);
            let Some(r) = d.resources.last().cloned() else {
                // Paso 4.
                if d.needs_await && !d.has_awaited {
                    d.needs_await = false;
                    d.has_awaited = true;
                    return Ok(AsyncStep::Await(Value::Undefined));
                }
                d.needs_await = false;
                d.has_awaited = false;
                return Ok(AsyncStep::Done);
            };
            // Paso 3.d.
            if r.hint == Hint::Sync && d.needs_await && !d.has_awaited {
                d.needs_await = false;
                return Ok(AsyncStep::Await(Value::Undefined));
            }
            d.resources.pop();
            if r.method.is_undefined() {
                // Paso 3.f.
                d.needs_await = true;
                continue;
            }
            match self.call(&r.method, r.value, &[]) {
                Ok(v) => {
                    if r.hint == Hint::Async {
                        data(self, cap).has_awaited = true;
                        return Ok(AsyncStep::Await(v));
                    }
                }
                Err(Throw::Value(e)) => self.merge_dispose_error(completion, e),
                Err(t) => return Err(t),
            }
        }
    }
}

/// El closure de GetDisposeMethod (paso 1.b.ii.1) para un `@@dispose` usado por `await using`.
fn sync_dispose_wrapper(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let method = slots(agent, a.callee)[0].clone();
    let p = agent.new_internal_promise();
    match agent.call(&method, a.this.clone(), &[]) {
        Ok(_) => agent.resolve_promise(p, Value::Undefined)?,
        Err(Throw::Value(e)) => agent.reject_promise(p, e),
        Err(t) => return Err(t),
    }
    Ok(Value::Object(p))
}

// --- Intrínsecos ---

pub fn init(agent: &mut Agent, realm: RealmId) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    let ep = agent.realms[realm].intrinsics.error_prototype;
    let ec = agent.realms[realm].intrinsics.error_ctor;
    // SuppressedError (§20.5.8).
    let sp = agent.new_object(Some(ep));
    let sc = constructor(agent, "SuppressedError", 3, suppressed_error, sp);
    agent.heap[sc].proto = Some(ec);
    crate::builtins::value_prop(agent, sp, "name", Value::str("SuppressedError"));
    crate::builtins::value_prop(agent, sp, "message", Value::str(""));
    crate::builtins::register_global(agent, realm, "SuppressedError", sc);
    agent.realms[realm].intrinsics.suppressed_error_prototype = sp;

    // DisposableStack (§27.3).
    let dp = agent.new_object(Some(op));
    const DS: &[FnEntry] = &[
        ("adopt", 2, ds_adopt),
        ("defer", 1, ds_defer),
        ("move", 0, ds_move),
        ("use", 1, ds_use),
    ];
    methods(agent, dp, DS);
    let dispose = crate::builtins::method(agent, dp, "dispose", 0, ds_dispose);
    accessor(
        agent,
        dp,
        PropertyKey::from("disposed"),
        "disposed",
        Some(ds_disposed),
        None,
    );
    let k = PropertyKey::Symbol(agent.well_known.dispose);
    agent.put_data(dp, k, Value::Object(dispose), true, false, true);
    to_string_tag(agent, dp, "DisposableStack");
    let dc = constructor(agent, "DisposableStack", 0, ds_ctor, dp);
    crate::builtins::register_global(agent, realm, "DisposableStack", dc);

    // AsyncDisposableStack (§27.4).
    let ap = agent.new_object(Some(op));
    const ADS: &[FnEntry] = &[
        ("adopt", 2, ads_adopt),
        ("defer", 1, ads_defer),
        ("move", 0, ads_move),
        ("use", 1, ads_use),
    ];
    methods(agent, ap, ADS);
    let dispose_async = crate::builtins::method(agent, ap, "disposeAsync", 0, ads_dispose_async);
    accessor(
        agent,
        ap,
        PropertyKey::from("disposed"),
        "disposed",
        Some(ads_disposed),
        None,
    );
    let k = PropertyKey::Symbol(agent.well_known.async_dispose);
    agent.put_data(ap, k, Value::Object(dispose_async), true, false, true);
    to_string_tag(agent, ap, "AsyncDisposableStack");
    let ac = constructor(agent, "AsyncDisposableStack", 0, ads_ctor, ap);
    crate::builtins::register_global(agent, realm, "AsyncDisposableStack", ac);

    // %AsyncIteratorPrototype% [ @@asyncDispose ] ( ) (§27.1.3.1).
    let aip = agent.realms[realm].intrinsics.async_iterator_prototype;
    let k = agent.well_known.async_dispose;
    crate::builtins::method_sym(
        agent,
        aip,
        k,
        "[Symbol.asyncDispose]",
        0,
        async_iterator_dispose,
    );

    let i = &mut agent.realms[realm].intrinsics;
    i.disposable_stack_ctor = dc;
    i.disposable_stack_prototype = dp;
    i.async_disposable_stack_ctor = ac;
    i.async_disposable_stack_prototype = ap;
}

/// SuppressedError ( error, suppressed, message ) (§20.5.8.1.1).
fn suppressed_error(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let nt = a.new_target.unwrap_or(a.callee);
    let o = agent.ordinary_create_from_constructor(
        nt,
        |i| i.suppressed_error_prototype,
        ObjectKind::Error,
    )?;
    let message = a.arg(2);
    if !message.is_undefined() {
        let m = agent.to_string(&message)?;
        let k = agent.keys.message.clone();
        agent.create_non_enumerable_data_property_or_throw(o, k, Value::String(m))?;
    }
    agent.create_non_enumerable_data_property_or_throw(o, PropertyKey::from("error"), a.arg(0))?;
    agent.create_non_enumerable_data_property_or_throw(
        o,
        PropertyKey::from("suppressed"),
        a.arg(1),
    )?;
    agent.attach_stack(o);
    Ok(Value::Object(o))
}

// --- DisposableStack ---

fn this_stack(
    agent: &mut Agent,
    this: &Value,
    is_async: bool,
    method: &str,
) -> JsResult<Gc<Object>> {
    if let Value::Object(o) = this {
        match &agent.heap[*o].kind {
            ObjectKind::DisposableStack(_) if !is_async => return Ok(*o),
            ObjectKind::AsyncDisposableStack(_) if is_async => return Ok(*o),
            _ => {}
        }
    }
    let class = if is_async {
        "AsyncDisposableStack"
    } else {
        "DisposableStack"
    };
    agent.type_error(&format!(
        "Method {class}.prototype.{method} called on incompatible receiver"
    ))
}

/// RequireInternalSlot + el paso "If [[DisposableState]] is disposed, throw a ReferenceError".
fn pending_stack(
    agent: &mut Agent,
    this: &Value,
    is_async: bool,
    method: &str,
) -> JsResult<Gc<Object>> {
    let s = this_stack(agent, this, is_async, method)?;
    if data(agent, s).disposed {
        let class = if is_async {
            "AsyncDisposableStack"
        } else {
            "DisposableStack"
        };
        return agent.reference_error(&format!("{class} already disposed"));
    }
    Ok(s)
}

/// DisposableStack ( ) (§27.3.1.1).
fn ds_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor DisposableStack requires 'new'");
    };
    let s = agent.ordinary_create_from_constructor(
        nt,
        |i| i.disposable_stack_prototype,
        ObjectKind::DisposableStack(Box::default()),
    )?;
    Ok(Value::Object(s))
}

/// DisposableStack.prototype.adopt ( value, onDispose ) (§27.3.3.1).
fn ds_adopt(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = pending_stack(agent, &a.this, false, "adopt")?;
    adopt(agent, s, a, Hint::Sync)
}

fn adopt(agent: &mut Agent, s: Gc<Object>, a: &CallArgs<'_>, hint: Hint) -> JsResult<Value> {
    let on_dispose = a.arg(1);
    if !agent.is_callable(&on_dispose) {
        return agent.type_error("onDispose is not a function");
    }
    let value = a.arg(0);
    let f = agent.create_builtin_with_slots(adopt_closure, 0, "", vec![value.clone(), on_dispose]);
    agent.add_disposable_resource(s, Value::Undefined, hint, Some(Value::Object(f)))?;
    Ok(value)
}

/// El closure de `adopt`: `Call(onDispose, undefined, « value »)`.
fn adopt_closure(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = slots(agent, a.callee);
    agent.call(&s[1], Value::Undefined, &[s[0].clone()])
}

/// DisposableStack.prototype.defer ( onDispose ) (§27.3.3.2).
fn ds_defer(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = pending_stack(agent, &a.this, false, "defer")?;
    defer(agent, s, a, Hint::Sync)
}

fn defer(agent: &mut Agent, s: Gc<Object>, a: &CallArgs<'_>, hint: Hint) -> JsResult<Value> {
    let on_dispose = a.arg(0);
    if !agent.is_callable(&on_dispose) {
        return agent.type_error("onDispose is not a function");
    }
    agent.add_disposable_resource(s, Value::Undefined, hint, Some(on_dispose))?;
    Ok(Value::Undefined)
}

/// DisposableStack.prototype.dispose ( ) (§27.3.3.3).
fn ds_dispose(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_stack(agent, &a.this, false, "dispose")?;
    if data(agent, s).disposed {
        return Ok(Value::Undefined);
    }
    data(agent, s).disposed = true;
    match agent.dispose_resources_sync(s, None)? {
        Some(e) => Err(Throw::Value(e)),
        None => Ok(Value::Undefined),
    }
}

/// get DisposableStack.prototype.disposed (§27.3.3.4).
fn ds_disposed(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_stack(agent, &a.this, false, "disposed")?;
    Ok(Value::Boolean(data(agent, s).disposed))
}

/// DisposableStack.prototype.move ( ) (§27.3.3.5).
fn ds_move(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = pending_stack(agent, &a.this, false, "move")?;
    let ctor = agent.intrinsics().disposable_stack_ctor;
    let n = agent.ordinary_create_from_constructor(
        ctor,
        |i| i.disposable_stack_prototype,
        ObjectKind::DisposableStack(Box::default()),
    )?;
    move_resources(agent, s, n);
    Ok(Value::Object(n))
}

fn move_resources(agent: &mut Agent, from: Gc<Object>, to: Gc<Object>) {
    let d = data(agent, from);
    let resources = std::mem::take(&mut d.resources);
    d.disposed = true;
    data(agent, to).resources = resources;
}

/// DisposableStack.prototype.use ( value ) (§27.3.3.6).
fn ds_use(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = pending_stack(agent, &a.this, false, "use")?;
    let v = a.arg(0);
    agent.add_disposable_resource(s, v.clone(), Hint::Sync, None)?;
    Ok(v)
}

// --- AsyncDisposableStack ---

/// AsyncDisposableStack ( ) (§27.4.1.1).
fn ads_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor AsyncDisposableStack requires 'new'");
    };
    let s = agent.ordinary_create_from_constructor(
        nt,
        |i| i.async_disposable_stack_prototype,
        ObjectKind::AsyncDisposableStack(Box::default()),
    )?;
    Ok(Value::Object(s))
}

/// AsyncDisposableStack.prototype.adopt ( value, onDisposeAsync ) (§27.4.3.1).
fn ads_adopt(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = pending_stack(agent, &a.this, true, "adopt")?;
    adopt(agent, s, a, Hint::Async)
}

/// AsyncDisposableStack.prototype.defer ( onDisposeAsync ) (§27.4.3.2).
fn ads_defer(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = pending_stack(agent, &a.this, true, "defer")?;
    defer(agent, s, a, Hint::Async)
}

/// get AsyncDisposableStack.prototype.disposed (§27.4.3.4).
fn ads_disposed(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_stack(agent, &a.this, true, "disposed")?;
    Ok(Value::Boolean(data(agent, s).disposed))
}

/// AsyncDisposableStack.prototype.move ( ) (§27.4.3.5).
fn ads_move(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = pending_stack(agent, &a.this, true, "move")?;
    let ctor = agent.intrinsics().async_disposable_stack_ctor;
    let n = agent.ordinary_create_from_constructor(
        ctor,
        |i| i.async_disposable_stack_prototype,
        ObjectKind::AsyncDisposableStack(Box::default()),
    )?;
    move_resources(agent, s, n);
    Ok(Value::Object(n))
}

/// AsyncDisposableStack.prototype.use ( value ) (§27.4.3.6).
fn ads_use(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = pending_stack(agent, &a.this, true, "use")?;
    let v = a.arg(0);
    agent.add_disposable_resource(s, v.clone(), Hint::Async, None)?;
    Ok(v)
}

/// AsyncDisposableStack.prototype.disposeAsync ( ) (§27.4.3.3).
fn ads_dispose_async(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let p = agent.new_internal_promise();
    let s = match &a.this {
        Value::Object(o) if matches!(agent.heap[*o].kind, ObjectKind::AsyncDisposableStack(_)) => {
            *o
        }
        _ => {
            let e = agent.error(
                crate::vm::ErrorType::TypeError,
                "Method AsyncDisposableStack.prototype.disposeAsync called on incompatible receiver",
            );
            match e {
                Throw::Value(e) => agent.reject_promise(p, e),
                t => return Err(t),
            }
            return Ok(Value::Object(p));
        }
    };
    if data(agent, s).disposed {
        agent.resolve_promise(p, Value::Undefined)?;
        return Ok(Value::Object(p));
    }
    let d = data(agent, s);
    d.disposed = true;
    d.completion = None;
    d.needs_await = false;
    d.has_awaited = false;
    drive_async_stack(agent, s, p)?;
    Ok(Value::Object(p))
}

/// DisposeResources de `disposeAsync` como continuaciones: avanza hasta el próximo Await y
/// se reanuda desde la reacción de la promesa; al terminar resuelve o rechaza `p`.
fn drive_async_stack(agent: &mut Agent, s: Gc<Object>, p: Gc<Object>) -> JsResult<()> {
    loop {
        let mut completion = data(agent, s).completion.take();
        let step = agent.dispose_async_step(s, &mut completion)?;
        data(agent, s).completion = completion;
        match step {
            AsyncStep::Await(v) => {
                let pc = agent.intrinsics().promise_ctor;
                match agent.promise_resolve(pc, v) {
                    Ok(Value::Object(promise)) => {
                        let ok = agent.create_builtin_with_slots(
                            dispose_async_fulfilled,
                            1,
                            "",
                            vec![Value::Object(s), Value::Object(p)],
                        );
                        let err = agent.create_builtin_with_slots(
                            dispose_async_rejected,
                            1,
                            "",
                            vec![Value::Object(s), Value::Object(p)],
                        );
                        agent.perform_promise_then(
                            promise,
                            Handler::Callback(Value::Object(ok)),
                            Handler::Callback(Value::Object(err)),
                            None,
                        );
                        return Ok(());
                    }
                    Ok(_) => unreachable!("PromiseResolve devuelve una promesa"),
                    Err(Throw::Value(e)) => {
                        let mut c = data(agent, s).completion.take();
                        agent.merge_dispose_error(&mut c, e);
                        data(agent, s).completion = c;
                    }
                    Err(t) => return Err(t),
                }
            }
            AsyncStep::Done => {
                match data(agent, s).completion.take() {
                    Some(e) => agent.reject_promise(p, e),
                    None => agent.resolve_promise(p, Value::Undefined)?,
                }
                return Ok(());
            }
        }
    }
}

fn dispose_async_fulfilled(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let sl = slots(agent, a.callee);
    let (Value::Object(s), Value::Object(p)) = (&sl[0], &sl[1]) else {
        return Ok(Value::Undefined);
    };
    drive_async_stack(agent, *s, *p)?;
    Ok(Value::Undefined)
}

fn dispose_async_rejected(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let sl = slots(agent, a.callee);
    let (Value::Object(s), Value::Object(p)) = (&sl[0], &sl[1]) else {
        return Ok(Value::Undefined);
    };
    let mut c = data(agent, *s).completion.take();
    agent.merge_dispose_error(&mut c, a.arg(0));
    data(agent, *s).completion = c;
    drive_async_stack(agent, *s, *p)?;
    Ok(Value::Undefined)
}

/// %AsyncIteratorPrototype% [ @@asyncDispose ] ( ) (§27.1.3.1).
fn async_iterator_dispose(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let p = agent.new_internal_promise();
    let o = a.this.clone();
    let k = agent.keys.return_.clone();
    let ret = match agent.get_method_of(&o, &k) {
        Ok(r) => r,
        Err(Throw::Value(e)) => {
            agent.reject_promise(p, e);
            return Ok(Value::Object(p));
        }
        Err(t) => return Err(t),
    };
    let Some(ret) = ret else {
        agent.resolve_promise(p, Value::Undefined)?;
        return Ok(Value::Object(p));
    };
    let result = match agent.call(&ret, o, &[Value::Undefined]) {
        Ok(r) => r,
        Err(Throw::Value(e)) => {
            agent.reject_promise(p, e);
            return Ok(Value::Object(p));
        }
        Err(t) => return Err(t),
    };
    let pc = agent.intrinsics().promise_ctor;
    let wrapper = match agent.promise_resolve(pc, result) {
        Ok(Value::Object(w)) => w,
        Ok(_) => unreachable!("PromiseResolve devuelve una promesa"),
        Err(Throw::Value(e)) => {
            agent.reject_promise(p, e);
            return Ok(Value::Object(p));
        }
        Err(t) => return Err(t),
    };
    let unwrap = agent.create_builtin_function(return_undefined, 1, "", false);
    agent.perform_promise_then(
        wrapper,
        Handler::Callback(Value::Object(unwrap)),
        Handler::Empty,
        Some(crate::builtins::promise::Capability::Internal(p)),
    );
    Ok(Value::Object(p))
}

fn return_undefined(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Undefined)
}
