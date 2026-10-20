//! Atomics (§25.4) para un solo agente: sin otros hilos, las operaciones atómicas son
//! lecturas y escrituras simples del Data Block. `Atomics.wait` solo puede terminar por
//! timeout (nadie más puede notificar) y lanza TypeError si el agente no puede suspenderse
//! (AgentCanSuspend, `Agent::can_block`); los waiters de `Atomics.waitAsync` quedan como
//! timers que RunJobs dispara cuando la cola de jobs se vacía y que `Atomics.notify`
//! despierta.
//!
//! También están aquí los timers del host de test262 (`$262.agent.setTimeout`,
//! `monotonicNow`, `sleep`), que usan la misma cola.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::builtins::array_buffer::{
    self as ab, ElementType, is_shared, numeric_to_raw, raw_to_numeric, read_raw, to_big_int,
    write_raw,
};
use crate::builtins::typed_array::{kind_of, ta_data, ta_length, validate};
use crate::builtins::{FnEntry, method, methods, to_string_tag};
use crate::heap::{Gc, Trace, Tracer};
use crate::object::Object;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId, Throw};

/// Un timer pendiente del agente.
#[derive(Debug, Clone)]
pub enum Timer {
    /// Un waiter de `Atomics.waitAsync` (§25.4.3.14 DoWait, modo async); `deadline` es
    /// `None` si espera sin límite.
    Waiter {
        buffer: Gc<Object>,
        byte_index: usize,
        promise: Gc<Object>,
        deadline: Option<Instant>,
    },
    /// `$262.agent.setTimeout`.
    Callback { deadline: Instant, callback: Value },
}

impl Timer {
    fn deadline(&self) -> Option<Instant> {
        match self {
            Timer::Waiter { deadline, .. } => *deadline,
            Timer::Callback { deadline, .. } => Some(*deadline),
        }
    }
}

impl Trace for Timer {
    fn trace(&self, t: &mut Tracer) {
        match self {
            Timer::Waiter {
                buffer, promise, ..
            } => {
                t.gc(*buffer);
                t.gc(*promise);
            }
            Timer::Callback { callback, .. } => t.value(callback),
        }
    }
}

/// Espera hasta `until` sin pasar el límite de tiempo del agente.
fn sleep_until(agent: &Agent, until: Instant) -> JsResult<()> {
    if let Some(limit) = agent.options.deadline
        && until > limit
    {
        return Err(Throw::Terminate("tiempo agotado".into()));
    }
    let now = Instant::now();
    if until > now {
        std::thread::sleep(until - now);
    }
    Ok(())
}

/// Dispara el timer con el vencimiento más próximo; `false` si no queda ninguno con
/// vencimiento.
pub fn fire_next_timer(agent: &mut Agent) -> JsResult<bool> {
    let next = agent
        .timers
        .iter()
        .enumerate()
        .filter_map(|(i, t)| t.deadline().map(|d| (d, i)))
        .min();
    let Some((deadline, i)) = next else {
        return Ok(false);
    };
    sleep_until(agent, deadline)?;
    match agent.timers.remove(i) {
        Timer::Waiter { promise, .. } => {
            agent.resolve_promise(promise, Value::str("timed-out"))?;
        }
        Timer::Callback { callback, .. } => match agent.call(&callback, Value::Undefined, &[]) {
            Ok(_) => {}
            Err(Throw::Value(e)) => agent.job_errors.push(e),
            Err(t) => return Err(t),
        },
    }
    Ok(true)
}

// --- Validaciones (§25.4.3.1–4) ---

/// ValidateIntegerTypedArray (§25.4.3.1).
fn validate_integer_ta(
    agent: &mut Agent,
    v: &Value,
    waitable: bool,
) -> JsResult<(Gc<Object>, usize)> {
    let (o, len) = validate(agent, v)?;
    let kind = kind_of(agent, o);
    let ok = if waitable {
        matches!(kind, ElementType::Int32 | ElementType::BigInt64)
    } else {
        !matches!(
            kind,
            ElementType::Uint8Clamped
                | ElementType::Float16
                | ElementType::Float32
                | ElementType::Float64
        )
    };
    if !ok {
        return agent.type_error("Invalid TypedArray type for an atomic operation");
    }
    Ok((o, len))
}

/// ValidateAtomicAccess (§25.4.3.2): el índice de byte en el buffer.
fn validate_atomic_access(
    agent: &mut Agent,
    o: Gc<Object>,
    len: usize,
    index: &Value,
) -> JsResult<usize> {
    let i = agent.to_index(index)?;
    if i >= len as u64 {
        return agent.range_error("Invalid atomic access index");
    }
    let d = ta_data(agent, o).expect("typed array");
    Ok(i as usize * d.kind.size() + d.byte_offset)
}

/// ValidateAtomicAccessOnIntegerTypedArray (§25.4.3.3).
fn validate_access(
    agent: &mut Agent,
    ta: &Value,
    index: &Value,
    waitable: bool,
) -> JsResult<(Gc<Object>, usize)> {
    let (o, len) = validate_integer_ta(agent, ta, waitable)?;
    let i = validate_atomic_access(agent, o, len, index)?;
    Ok((o, i))
}

/// RevalidateAtomicAccess (§25.4.3.4).
fn revalidate(agent: &mut Agent, o: Gc<Object>, byte_index: usize) -> JsResult<()> {
    if ta_length(agent, o).is_none() {
        return agent.type_error("TypedArray is detached or out of bounds");
    }
    let buffer = ta_data(agent, o).expect("typed array").buffer;
    if byte_index >= ab::byte_length(agent, buffer) {
        return agent.range_error("Invalid atomic access index");
    }
    Ok(())
}

/// El valor de una operación atómica: ToBigInt o 𝔽(ToIntegerOrInfinity).
fn atomic_value(agent: &mut Agent, kind: ElementType, v: &Value) -> JsResult<Value> {
    if kind.is_bigint() {
        to_big_int(agent, v)
    } else {
        Ok(Value::Number(agent.to_integer_or_infinity(v)? + 0.0))
    }
}

fn mask(size: usize) -> u64 {
    if size == 8 {
        u64::MAX
    } else {
        (1u64 << (8 * size)) - 1
    }
}

/// AtomicReadModifyWrite (§25.4.3.17).
fn read_modify_write(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    op: fn(u64, u64) -> u64,
) -> JsResult<Value> {
    let (o, byte_index) = validate_access(agent, &a.arg(0), &a.arg(1), false)?;
    let kind = kind_of(agent, o);
    let v = atomic_value(agent, kind, &a.arg(2))?;
    revalidate(agent, o, byte_index)?;
    let buffer = ta_data(agent, o).expect("typed array").buffer;
    let size = kind.size();
    let old = read_raw(agent, buffer, byte_index, size, true);
    let new = op(old, numeric_to_raw(agent, kind, &v)) & mask(size);
    write_raw(agent, buffer, byte_index, size, new, true);
    Ok(raw_to_numeric(agent, kind, old))
}

pub fn init(agent: &mut Agent, realm: RealmId) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    let o = agent.new_object(Some(op));
    const FNS: &[FnEntry] = &[
        ("add", 3, add),
        ("and", 3, and),
        ("compareExchange", 4, compare_exchange),
        ("exchange", 3, exchange),
        ("isLockFree", 1, is_lock_free),
        ("load", 2, load),
        ("notify", 3, notify),
        ("or", 3, or),
        ("pause", 0, pause),
        ("store", 3, store),
        ("sub", 3, sub),
        ("wait", 4, wait),
        ("waitAsync", 4, wait_async),
        ("xor", 3, xor),
    ];
    methods(agent, o, FNS);
    to_string_tag(agent, o, "Atomics");
    agent.realms[realm].intrinsics.atomics = o;
}

/// Atomics.add (§25.4.4).
fn add(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    read_modify_write(agent, a, u64::wrapping_add)
}

/// Atomics.and (§25.4.5).
fn and(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    read_modify_write(agent, a, |x, y| x & y)
}

/// Atomics.exchange (§25.4.7).
fn exchange(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    read_modify_write(agent, a, |_, y| y)
}

/// Atomics.or (§25.4.11).
fn or(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    read_modify_write(agent, a, |x, y| x | y)
}

/// Atomics.sub (§25.4.14).
fn sub(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    read_modify_write(agent, a, u64::wrapping_sub)
}

/// Atomics.xor (§25.4.17).
fn xor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    read_modify_write(agent, a, |x, y| x ^ y)
}

/// Atomics.compareExchange (§25.4.6).
fn compare_exchange(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, byte_index) = validate_access(agent, &a.arg(0), &a.arg(1), false)?;
    let kind = kind_of(agent, o);
    let expected = atomic_value(agent, kind, &a.arg(2))?;
    let replacement = atomic_value(agent, kind, &a.arg(3))?;
    revalidate(agent, o, byte_index)?;
    let buffer = ta_data(agent, o).expect("typed array").buffer;
    let size = kind.size();
    let old = read_raw(agent, buffer, byte_index, size, true);
    if old == numeric_to_raw(agent, kind, &expected) & mask(size) {
        let r = numeric_to_raw(agent, kind, &replacement);
        write_raw(agent, buffer, byte_index, size, r, true);
    }
    Ok(raw_to_numeric(agent, kind, old))
}

/// Atomics.isLockFree (§25.4.8).
fn is_lock_free(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = agent.to_integer_or_infinity(&a.arg(0))?;
    Ok(Value::Boolean(n == 1.0 || n == 2.0 || n == 4.0 || n == 8.0))
}

/// Atomics.load (§25.4.9).
fn load(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, byte_index) = validate_access(agent, &a.arg(0), &a.arg(1), false)?;
    revalidate(agent, o, byte_index)?;
    let kind = kind_of(agent, o);
    let buffer = ta_data(agent, o).expect("typed array").buffer;
    Ok(ab::get_value_from_buffer(
        agent, buffer, byte_index, kind, true,
    ))
}

/// Atomics.store (§25.4.15).
fn store(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, byte_index) = validate_access(agent, &a.arg(0), &a.arg(1), false)?;
    let kind = kind_of(agent, o);
    let v = atomic_value(agent, kind, &a.arg(2))?;
    revalidate(agent, o, byte_index)?;
    let buffer = ta_data(agent, o).expect("typed array").buffer;
    ab::set_value_in_buffer(agent, buffer, byte_index, kind, &v, true);
    Ok(v)
}

/// Atomics.pause (§25.4.10).
fn pause(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = a.arg(0);
    let iterations = match n {
        Value::Undefined => 1.0,
        Value::Number(x) if x.is_finite() && x.trunc() == x => x.clamp(1.0, 1000.0),
        _ => return agent.type_error("Atomics.pause: argument must be an integral Number"),
    };
    for _ in 0..iterations as u32 {
        std::hint::spin_loop();
    }
    Ok(Value::Undefined)
}

/// Atomics.notify (§25.4.12): despierta waiters de `waitAsync` del agente.
fn notify(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = validate_integer_ta(agent, &a.arg(0), true)?;
    let byte_index = validate_atomic_access(agent, o, len, &a.arg(1))?;
    let count = if a.arg(2).is_undefined() {
        f64::INFINITY
    } else {
        agent.to_integer_or_infinity(&a.arg(2))?.max(0.0)
    };
    let buffer = ta_data(agent, o).expect("typed array").buffer;
    if !is_shared(agent, buffer) {
        return Ok(Value::Number(0.0));
    }
    let mut woken = Vec::new();
    let mut i = 0;
    while i < agent.timers.len() && (woken.len() as f64) < count {
        match &agent.timers[i] {
            Timer::Waiter {
                buffer: b,
                byte_index: bi,
                promise,
                ..
            } if *b == buffer && *bi == byte_index => {
                woken.push(*promise);
                agent.timers.remove(i);
            }
            _ => i += 1,
        }
    }
    let n = woken.len();
    for p in woken {
        agent.resolve_promise(p, Value::str("ok"))?;
    }
    Ok(Value::Number(n as f64))
}

/// DoWait (§25.4.3.14) hasta la comparación: (TypedArray, índice de byte, ¿igual?, t).
fn do_wait_prefix(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    sync: bool,
) -> JsResult<(Gc<Object>, usize, bool, f64)> {
    let (o, len) = validate_integer_ta(agent, &a.arg(0), true)?;
    let buffer = ta_data(agent, o).expect("typed array").buffer;
    if !is_shared(agent, buffer) {
        return agent.type_error("Atomics.wait requires a shared Int32Array or BigInt64Array");
    }
    let byte_index = validate_atomic_access(agent, o, len, &a.arg(1))?;
    let kind = kind_of(agent, o);
    let v = if kind == ElementType::BigInt64 {
        let b = to_big_int(agent, &a.arg(2))?;
        numeric_to_raw(agent, kind, &b)
    } else {
        let n = agent.to_number(&a.arg(2))?;
        numeric_to_raw(agent, ElementType::Int32, &Value::Number(n))
    };
    let q = agent.to_number(&a.arg(3))?;
    let t = if q.is_nan() || q == f64::INFINITY {
        f64::INFINITY
    } else if q == f64::NEG_INFINITY {
        0.0
    } else {
        q.max(0.0)
    };
    if sync && !agent.can_block {
        return agent.type_error("Atomics.wait cannot be called in this context");
    }
    let w = read_raw(agent, buffer, byte_index, kind.size(), true);
    Ok((o, byte_index, v & mask(kind.size()) == w, t))
}

/// Atomics.wait (§25.4.16): sin otros agentes, solo puede vencer el timeout.
fn wait(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (_, _, equal, t) = do_wait_prefix(agent, a, true)?;
    if !equal {
        return Ok(Value::str("not-equal"));
    }
    if t.is_infinite() {
        return Err(Throw::Terminate(
            "Atomics.wait sin timeout y sin otros agentes que notifiquen".into(),
        ));
    }
    sleep_until(agent, Instant::now() + Duration::from_secs_f64(t / 1000.0))?;
    Ok(Value::str("timed-out"))
}

/// Atomics.waitAsync (§25.4.17).
fn wait_async(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, byte_index, equal, t) = do_wait_prefix(agent, a, false)?;
    let result = agent.new_plain_object();
    let k_async = PropertyKey::from("async");
    let k_value = PropertyKey::from("value");
    if !equal || t == 0.0 {
        let v = if equal { "timed-out" } else { "not-equal" };
        agent.create_data_property_or_throw(result, k_async, Value::Boolean(false))?;
        agent.create_data_property_or_throw(result, k_value, Value::str(v))?;
        return Ok(Value::Object(result));
    }
    let promise = agent.new_internal_promise();
    let buffer = ta_data(agent, o).expect("typed array").buffer;
    let deadline = if t.is_finite() {
        Some(Instant::now() + Duration::from_secs_f64(t / 1000.0))
    } else {
        None
    };
    agent.timers.push(Timer::Waiter {
        buffer,
        byte_index,
        promise,
        deadline,
    });
    agent.create_data_property_or_throw(result, k_async, Value::Boolean(true))?;
    agent.create_data_property_or_throw(result, k_value, Value::Object(promise))?;
    Ok(Value::Object(result))
}

// --- `$262.agent` del host de test262 (solo lo que no necesita otros hilos) ---

fn start_instant() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

/// Instala `$262.agent` con `monotonicNow`, `setTimeout` y `sleep` (INTERPRETING.md). Los
/// métodos de varios agentes (`start`, `broadcast`, `getReport`…) necesitan hilos y no están.
pub fn install_agent_hooks(agent: &mut Agent, o262: Gc<Object>) {
    let o = agent.new_plain_object();
    method(agent, o, "monotonicNow", 0, monotonic_now);
    method(agent, o, "setTimeout", 2, set_timeout);
    method(agent, o, "sleep", 1, sleep);
    agent.put_data(
        o262,
        PropertyKey::from("agent"),
        Value::Object(o),
        true,
        false,
        true,
    );
}

fn monotonic_now(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(
        start_instant().elapsed().as_secs_f64() * 1000.0,
    ))
}

fn set_timeout(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let ms = agent.to_number(&a.arg(1))?;
    let ms = if ms.is_finite() { ms.max(0.0) } else { 0.0 };
    agent.timers.push(Timer::Callback {
        deadline: Instant::now() + Duration::from_secs_f64(ms / 1000.0),
        callback: a.arg(0),
    });
    Ok(Value::Undefined)
}

fn sleep(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let ms = agent.to_number(&a.arg(0))?;
    let ms = if ms.is_finite() { ms.max(0.0) } else { 0.0 };
    sleep_until(agent, Instant::now() + Duration::from_secs_f64(ms / 1000.0))?;
    Ok(Value::Undefined)
}
