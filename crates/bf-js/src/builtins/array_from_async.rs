//! Array.fromAsync ( asyncItems [ , mapfn [ , thisArg ] ] ) (ECMA-262 2026, §23.1.2.2).
//!
//! El cuerpo es un Abstract Closure que corre como una función async (AsyncFunctionStart):
//! aquí, una máquina de estados cuyo estado vive en los `slots` de una built-in interna y
//! cuyos `Await` son reacciones de promesas (las continuaciones no son observables).

use crate::builtins::promise::Handler;
use crate::builtins::slots;
use crate::heap::Gc;
use crate::object::{Object, ObjectKind};
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, ErrorType, JsResult, Throw};

// Los slots del estado.
const MAPFN: usize = 0;
const THIS: usize = 1;
const ITER: usize = 2;
const NEXT: usize = 3;
const ARRAY: usize = 4;
const K: usize = 5;
const PROMISE: usize = 6;
const ARRAY_LIKE: usize = 7;
const LEN: usize = 8;
const ERROR: usize = 9;
const N_SLOTS: usize = 10;

/// Dónde sigue la máquina después de un Await.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// El resultado de `next()` (iterador).
    Next = 0,
    /// El valor de `mapfn` (iterador).
    Mapped = 1,
    /// El resultado de `return()` de AsyncIteratorClose: se rechaza con el error guardado.
    Closed = 2,
    /// El elemento de un array-like.
    Item = 3,
    /// El valor de `mapfn` (array-like).
    MappedItem = 4,
}

impl Phase {
    fn from(n: f64) -> Phase {
        match n as u8 {
            0 => Phase::Next,
            1 => Phase::Mapped,
            2 => Phase::Closed,
            3 => Phase::Item,
            _ => Phase::MappedItem,
        }
    }
}

fn get(agent: &Agent, st: Gc<Object>, i: usize) -> Value {
    match &agent.heap[st].kind {
        ObjectKind::Builtin(b) => b.slots[i].clone(),
        _ => Value::Undefined,
    }
}

fn put(agent: &mut Agent, st: Gc<Object>, i: usize, v: Value) {
    if let ObjectKind::Builtin(b) = &mut agent.heap[st].kind {
        b.slots[i] = v;
    }
}

fn promise_of(agent: &Agent, st: Gc<Object>) -> Gc<Object> {
    match get(agent, st, PROMISE) {
        Value::Object(p) => p,
        _ => unreachable!("la promesa de fromAsync"),
    }
}

fn index_key(k: f64) -> PropertyKey {
    if k < f64::from(u32::MAX) {
        PropertyKey::from(k as u32)
    } else {
        PropertyKey::String(crate::abstract_ops::number::number_to_string(k))
    }
}

/// Termina con una throw completion: rechaza la promesa.
fn reject(agent: &mut Agent, st: Gc<Object>, e: Throw) -> JsResult<()> {
    match e {
        Throw::Value(v) => {
            let p = promise_of(agent, st);
            agent.reject_promise(p, v);
            Ok(())
        }
        t => Err(t),
    }
}

/// Un resultado: si es abrupto, rechaza.
macro_rules! tryr {
    ($agent:expr, $st:expr, $e:expr) => {
        match $e {
            Ok(v) => v,
            Err(e) => return reject($agent, $st, e),
        }
    };
}

fn no_op(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Undefined)
}

pub fn from_async(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let p = agent.new_internal_promise();
    let mut s = vec![Value::Undefined; N_SLOTS];
    s[PROMISE] = Value::Object(p);
    s[MAPFN] = a.arg(1);
    s[THIS] = a.arg(2);
    s[K] = Value::Number(0.0);
    let st = agent.create_builtin_with_slots(no_op, 0, "", s);
    start(agent, st, a.this.clone(), a.arg(0))?;
    Ok(Value::Object(p))
}

/// Los pasos 3.a–i hasta el primer Await.
fn start(agent: &mut Agent, st: Gc<Object>, c: Value, items: Value) -> JsResult<()> {
    let mapfn = get(agent, st, MAPFN);
    if !mapfn.is_undefined() && !agent.is_callable(&mapfn) {
        let e = agent.error(
            ErrorType::TypeError,
            "Array.fromAsync: mapfn is not callable",
        );
        return reject(agent, st, e);
    }
    let k = PropertyKey::Symbol(agent.well_known.async_iterator);
    let using_async = tryr!(agent, st, agent.get_method_of(&items, &k));
    let using_sync = if using_async.is_none() {
        let k = PropertyKey::Symbol(agent.well_known.iterator);
        tryr!(agent, st, agent.get_method_of(&items, &k))
    } else {
        None
    };
    let record = if let Some(m) = using_async {
        Some(tryr!(agent, st, agent.get_iterator_from_method(&items, &m)))
    } else if let Some(m) = using_sync {
        let (it, next) = tryr!(agent, st, agent.get_iterator_from_method(&items, &m));
        Some(agent.create_async_from_sync_iterator(it, next))
    } else {
        None
    };
    if let Some((it, next)) = record {
        let a = if agent.is_constructor(&c) {
            let ctor = c.as_object().expect("constructor");
            tryr!(agent, st, agent.construct(ctor, &[], None))
        } else {
            Value::Object(tryr!(agent, st, agent.array_create(0, None)))
        };
        put(agent, st, ITER, it);
        put(agent, st, NEXT, next);
        put(agent, st, ARRAY, a);
        return iter_step(agent, st);
    }
    // Un array-like.
    let array_like = tryr!(agent, st, agent.to_object(&items));
    let len = tryr!(agent, st, agent.length_of_array_like(array_like));
    let a = if agent.is_constructor(&c) {
        let ctor = c.as_object().expect("constructor");
        tryr!(
            agent,
            st,
            agent.construct(ctor, &[Value::Number(len)], None)
        )
    } else {
        Value::Object(tryr!(agent, st, agent.array_create(len as u64, None)))
    };
    put(agent, st, ARRAY_LIKE, Value::Object(array_like));
    put(agent, st, LEN, Value::Number(len));
    put(agent, st, ARRAY, a);
    array_step(agent, st)
}

fn k_of(agent: &Agent, st: Gc<Object>) -> f64 {
    match get(agent, st, K) {
        Value::Number(n) => n,
        _ => 0.0,
    }
}

/// Await(v) y seguir en `phase`.
fn await_then(agent: &mut Agent, st: Gc<Object>, v: Value, phase: Phase) -> JsResult<()> {
    let pc = agent.intrinsics().promise_ctor;
    let promise = match agent.promise_resolve(pc, v) {
        Ok(Value::Object(p)) => p,
        Ok(_) => unreachable!("PromiseResolve devuelve una promesa"),
        Err(Throw::Value(e)) => return settled(agent, st, phase, Err(e)),
        Err(t) => return Err(t),
    };
    let mk = |agent: &mut Agent, rejected: bool| {
        let f = agent.create_builtin_with_slots(
            continuation,
            1,
            "",
            vec![
                Value::Object(st),
                Value::Number(f64::from(phase as u8)),
                Value::Boolean(rejected),
            ],
        );
        Handler::Callback(Value::Object(f))
    };
    let ok = mk(agent, false);
    let err = mk(agent, true);
    agent.perform_promise_then(promise, ok, err, None);
    Ok(())
}

fn continuation(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = slots(agent, a.callee);
    let (Value::Object(st), Value::Number(phase), Value::Boolean(rejected)) = (&s[0], &s[1], &s[2])
    else {
        return Ok(Value::Undefined);
    };
    let v = a.arg(0);
    let r = if *rejected { Err(v) } else { Ok(v) };
    settled(agent, *st, Phase::from(*phase), r)?;
    Ok(Value::Undefined)
}

/// La continuación de un Await con su resultado.
fn settled(
    agent: &mut Agent,
    st: Gc<Object>,
    phase: Phase,
    r: Result<Value, Value>,
) -> JsResult<()> {
    match phase {
        Phase::Next => {
            let result = match r {
                Ok(v) => v,
                Err(e) => return reject(agent, st, Throw::Value(e)),
            };
            let Value::Object(ro) = result else {
                let e = agent.error(
                    ErrorType::TypeError,
                    "Array.fromAsync: the iterator result is not an object",
                );
                return reject(agent, st, e);
            };
            let done = tryr!(agent, st, agent.iterator_complete(ro));
            let k = k_of(agent, st);
            if done {
                let a = get(agent, st, ARRAY);
                return finish(agent, st, a, k);
            }
            let value = tryr!(agent, st, agent.iterator_value(ro));
            let mapfn = get(agent, st, MAPFN);
            if mapfn.is_undefined() {
                return define_iter(agent, st, value);
            }
            let this = get(agent, st, THIS);
            match agent.call(&mapfn, this, &[value, Value::Number(k)]) {
                Ok(m) => await_then(agent, st, m, Phase::Mapped),
                Err(Throw::Value(e)) => close(agent, st, e),
                Err(t) => Err(t),
            }
        }
        Phase::Mapped => match r {
            Ok(v) => define_iter(agent, st, v),
            Err(e) => close(agent, st, e),
        },
        Phase::Closed => {
            let e = get(agent, st, ERROR);
            reject(agent, st, Throw::Value(e))
        }
        Phase::Item => {
            let v = match r {
                Ok(v) => v,
                Err(e) => return reject(agent, st, Throw::Value(e)),
            };
            let mapfn = get(agent, st, MAPFN);
            if mapfn.is_undefined() {
                return define_array(agent, st, v);
            }
            let this = get(agent, st, THIS);
            let k = k_of(agent, st);
            let m = tryr!(agent, st, agent.call(&mapfn, this, &[v, Value::Number(k)]));
            await_then(agent, st, m, Phase::MappedItem)
        }
        Phase::MappedItem => match r {
            Ok(v) => define_array(agent, st, v),
            Err(e) => reject(agent, st, Throw::Value(e)),
        },
    }
}

/// Set(A, "length", k, true) y resolver la promesa con A.
fn finish(agent: &mut Agent, st: Gc<Object>, a: Value, k: f64) -> JsResult<()> {
    let Value::Object(ao) = a else {
        unreachable!("A es un objeto")
    };
    let key = agent.keys.length.clone();
    let ok = tryr!(agent, st, agent.set(ao, key, Value::Number(k), a.clone()));
    if !ok {
        let e = agent.error(
            ErrorType::TypeError,
            "Cannot assign to read only property 'length'",
        );
        return reject(agent, st, e);
    }
    let p = promise_of(agent, st);
    agent.resolve_promise(p, a)
}

/// El paso del bucle del iterador (3.h.iv.1–4).
fn iter_step(agent: &mut Agent, st: Gc<Object>) -> JsResult<()> {
    let k = k_of(agent, st);
    if k >= 9_007_199_254_740_991.0 {
        let e = match agent.error(ErrorType::TypeError, "Array.fromAsync: too many elements") {
            Throw::Value(v) => v,
            t => return Err(t),
        };
        return close(agent, st, e);
    }
    let it = get(agent, st, ITER);
    let next = get(agent, st, NEXT);
    let r = tryr!(agent, st, agent.call(&next, it, &[]));
    await_then(agent, st, r, Phase::Next)
}

/// CreateDataPropertyOrThrow(A, Pk, v) en el bucle del iterador (12–13).
fn define_iter(agent: &mut Agent, st: Gc<Object>, v: Value) -> JsResult<()> {
    let k = k_of(agent, st);
    let Value::Object(ao) = get(agent, st, ARRAY) else {
        unreachable!("A es un objeto")
    };
    match agent.create_data_property_or_throw(ao, index_key(k), v) {
        Ok(()) => {}
        Err(Throw::Value(e)) => return close(agent, st, e),
        Err(t) => return Err(t),
    }
    put(agent, st, K, Value::Number(k + 1.0));
    iter_step(agent, st)
}

/// AsyncIteratorClose(iteratorRecord, ThrowCompletion(e)): llama a `return` y espera su
/// resultado, pero el rechazo es siempre con `e`.
fn close(agent: &mut Agent, st: Gc<Object>, e: Value) -> JsResult<()> {
    put(agent, st, ERROR, e.clone());
    let it = get(agent, st, ITER);
    let k = agent.keys.return_.clone();
    let ret = match agent.get_method_of(&it, &k) {
        Ok(Some(r)) => r,
        Ok(None) | Err(Throw::Value(_)) => return reject(agent, st, Throw::Value(e)),
        Err(t) => return Err(t),
    };
    match agent.call(&ret, it, &[]) {
        Ok(r) => await_then(agent, st, r, Phase::Closed),
        Err(Throw::Value(_)) => reject(agent, st, Throw::Value(e)),
        Err(t) => Err(t),
    }
}

/// El paso del bucle del array-like (3.i.vii).
fn array_step(agent: &mut Agent, st: Gc<Object>) -> JsResult<()> {
    let k = k_of(agent, st);
    let len = match get(agent, st, LEN) {
        Value::Number(n) => n,
        _ => 0.0,
    };
    if k >= len {
        let a = get(agent, st, ARRAY);
        return finish(agent, st, a, len);
    }
    let Value::Object(al) = get(agent, st, ARRAY_LIKE) else {
        unreachable!("el array-like")
    };
    let v = tryr!(agent, st, agent.get(al, &index_key(k), Value::Object(al)));
    await_then(agent, st, v, Phase::Item)
}

fn define_array(agent: &mut Agent, st: Gc<Object>, v: Value) -> JsResult<()> {
    let k = k_of(agent, st);
    let Value::Object(ao) = get(agent, st, ARRAY) else {
        unreachable!("A es un objeto")
    };
    tryr!(
        agent,
        st,
        agent.create_data_property_or_throw(ao, index_key(k), v)
    );
    put(agent, st, K, Value::Number(k + 1.0));
    array_step(agent, st)
}
