//! El constructor Iterator (§27.1.3), los métodos de %Iterator.prototype% (§27.1.4:
//! iterator helpers, `chunks`/`windows`, `includes`, `join`, `@@dispose`), `Iterator.from`,
//! `Iterator.concat`, `Iterator.zip`/`zipKeyed` y %WrapForValidIteratorPrototype%.
//!
//! Los helpers que el spec escribe como generadores (CreateIteratorFromClosure con Yield) son
//! aquí máquinas de estado: cada `next()` ejecuta un paso hasta el siguiente Yield, y
//! `return()` hace lo que haría la completion de retorno en ese Yield (cerrar los iteradores
//! subyacentes).

use crate::abstract_ops::conversion::to_boolean;
use crate::builtins::{FnEntry, accessor, method, method_sym, methods, to_string_tag};
use crate::heap::{Gc, Trace, Tracer};
use crate::object::{Object, ObjectKind, PropertyDescriptor};
use crate::string::JsString;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId, Throw};

/// Un Iterator Record (§7.4.1).
#[derive(Debug, Clone)]
pub struct IterRec {
    pub iterator: Value,
    pub next: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperState {
    SuspendedStart,
    SuspendedYield,
    Executing,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZipMode {
    Shortest,
    Longest,
    Strict,
}

#[derive(Debug)]
pub enum HelperKind {
    Map {
        f: Value,
        counter: f64,
    },
    Filter {
        f: Value,
        counter: f64,
    },
    Take {
        remaining: f64,
    },
    Drop {
        remaining: f64,
    },
    FlatMap {
        f: Value,
        counter: f64,
        inner: Option<IterRec>,
    },
    Concat {
        items: Vec<(Value, Value)>,
        index: usize,
        inner: Option<IterRec>,
    },
    Chunks {
        size: usize,
    },
    Windows {
        size: usize,
        allow_partial: bool,
        buffer: Vec<Value>,
    },
    Zip {
        iters: Vec<Option<IterRec>>,
        /// Índices de los iteradores todavía abiertos, en orden.
        open: Vec<usize>,
        mode: ZipMode,
        padding: Vec<Value>,
        keys: Option<Vec<PropertyKey>>,
    },
}

/// Un objeto de %IteratorHelperPrototype%.
#[derive(Debug)]
pub struct HelperData {
    pub state: HelperState,
    pub underlying: Option<IterRec>,
    pub kind: HelperKind,
}

impl Trace for IterRec {
    fn trace(&self, t: &mut Tracer) {
        t.value(&self.iterator);
        t.value(&self.next);
    }
}

impl Trace for HelperData {
    fn trace(&self, t: &mut Tracer) {
        if let Some(u) = &self.underlying {
            u.trace(t);
        }
        match &self.kind {
            HelperKind::Map { f, .. } | HelperKind::Filter { f, .. } => t.value(f),
            HelperKind::FlatMap { f, inner, .. } => {
                t.value(f);
                if let Some(i) = inner {
                    i.trace(t);
                }
            }
            HelperKind::Concat { items, inner, .. } => {
                for (a, b) in items {
                    t.value(a);
                    t.value(b);
                }
                if let Some(i) = inner {
                    i.trace(t);
                }
            }
            HelperKind::Windows { buffer, .. } => {
                for v in buffer {
                    t.value(v);
                }
            }
            HelperKind::Zip {
                iters,
                padding,
                keys,
                ..
            } => {
                for i in iters.iter().flatten() {
                    i.trace(t);
                }
                for v in padding {
                    t.value(v);
                }
                for k in keys.iter().flatten() {
                    if let PropertyKey::Symbol(s) = k {
                        t.gc(*s);
                    }
                }
            }
            HelperKind::Take { .. } | HelperKind::Drop { .. } | HelperKind::Chunks { .. } => {}
        }
    }
}

pub fn init(agent: &mut Agent, realm: RealmId) {
    let ip = agent.realms[realm].intrinsics.iterator_prototype;
    let fp = agent.realms[realm].intrinsics.function_prototype;
    // %Iterator% (§27.1.3.1).
    let ctor = agent.create_builtin_function(iterator_ctor, 0, "Iterator", true);
    let k = agent.keys.prototype.clone();
    agent.put_data(ctor, k, Value::Object(ip), false, false, false);
    agent.heap[ctor].proto = Some(fp);
    methods(
        agent,
        ctor,
        &[
            ("concat", 0, concat),
            ("from", 1, from),
            ("zip", 1, zip),
            ("zipKeyed", 1, zip_keyed),
        ],
    );
    agent.realms[realm].intrinsics.iterator_ctor = ctor;
    // %Iterator.prototype% (§27.1.4).
    const PROTO: &[FnEntry] = &[
        ("chunks", 1, chunks),
        ("drop", 1, drop),
        ("every", 1, every),
        ("filter", 1, filter),
        ("find", 1, find),
        ("flatMap", 1, flat_map),
        ("forEach", 1, for_each),
        ("includes", 1, includes),
        ("join", 1, join),
        ("map", 1, map),
        ("reduce", 1, reduce),
        ("some", 1, some),
        ("take", 1, take),
        ("toArray", 0, to_array),
        ("windows", 1, windows),
    ];
    methods(agent, ip, PROTO);
    let k = agent.keys.constructor.clone();
    accessor(agent, ip, k, "constructor", Some(ctor_get), Some(ctor_set));
    let tag = PropertyKey::Symbol(agent.well_known.to_string_tag);
    accessor(
        agent,
        ip,
        tag,
        "[Symbol.toStringTag]",
        Some(tag_get),
        Some(tag_set),
    );
    let d = agent.well_known.dispose;
    method_sym(agent, ip, d, "[Symbol.dispose]", 0, dispose);
    // %IteratorHelperPrototype% (§27.1.2.1).
    let hp = agent.new_object(Some(ip));
    method(agent, hp, "next", 0, helper_next);
    method(agent, hp, "return", 0, helper_return);
    to_string_tag(agent, hp, "Iterator Helper");
    agent.realms[realm].intrinsics.iterator_helper_prototype = hp;
    // %WrapForValidIteratorPrototype% (§27.1.3.2.1.1).
    let wp = agent.new_object(Some(ip));
    method(agent, wp, "next", 0, wrap_next);
    method(agent, wp, "return", 0, wrap_return);
    agent.realms[realm]
        .intrinsics
        .wrap_for_valid_iterator_prototype = wp;
    crate::builtins::register_global(agent, realm, "Iterator", ctor);
}

// --- Ayudas ---

/// GetIteratorDirect (§7.4.2).
fn get_iterator_direct(agent: &mut Agent, obj: &Value) -> JsResult<IterRec> {
    let next = agent.get_value_of(obj, &agent.keys.next.clone())?;
    Ok(IterRec {
        iterator: obj.clone(),
        next,
    })
}

/// GetIteratorFlattenable (§7.4.4): `strings` permite los String primitivos.
fn get_iterator_flattenable(agent: &mut Agent, obj: &Value, strings: bool) -> JsResult<IterRec> {
    if !obj.is_object() && !(strings && matches!(obj, Value::String(_))) {
        return agent.type_error("value is not an object");
    }
    let key = PropertyKey::Symbol(agent.well_known.iterator);
    let method = agent.get_method_of(obj, &key)?;
    let iterator = match method {
        None => obj.clone(),
        Some(m) => {
            let it = agent.call(&m, obj.clone(), &[])?;
            if !it.is_object() {
                return agent.type_error("iterator is not an object");
            }
            it
        }
    };
    get_iterator_direct(agent, &iterator)
}

fn step_value(agent: &mut Agent, r: &IterRec) -> JsResult<Option<Value>> {
    agent.iterator_step_value(&r.iterator, &r.next)
}

/// IteratorClose con una completion normal o de retorno (los errores de `return` cuentan).
fn close(agent: &mut Agent, r: &IterRec) -> JsResult<()> {
    agent.iterator_close(&r.iterator)
}

/// IteratorClose con una throw completion: se cierra y se relanza `e`.
fn close_throw<T>(agent: &mut Agent, r: &IterRec, e: Throw) -> JsResult<T> {
    if matches!(e, Throw::Value(_)) {
        agent.iterator_close_on_throw(&r.iterator);
    }
    Err(e)
}

/// IteratorCloseAll (§7.4.13): en orden inverso, propagando la primera completion abrupta.
fn close_all(agent: &mut Agent, iters: &[IterRec], completion: JsResult<()>) -> JsResult<()> {
    let mut completion = completion;
    for r in iters.iter().rev() {
        completion = match completion {
            Err(e) => {
                if matches!(e, Throw::Value(_)) {
                    agent.iterator_close_on_throw(&r.iterator);
                }
                Err(e)
            }
            Ok(()) => close(agent, r),
        };
    }
    completion
}

fn this_object(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    if !a.this.is_object() {
        return agent.type_error("Iterator.prototype method called on a non-object");
    }
    Ok(a.this.clone())
}

/// Lanza `e` después de cerrar `O` (los argumentos inválidos cierran el iterador, §27.1.4).
fn invalid<T>(agent: &mut Agent, o: &Value, e: Throw) -> JsResult<T> {
    close_throw(
        agent,
        &IterRec {
            iterator: o.clone(),
            next: Value::Undefined,
        },
        e,
    )
}

fn new_helper(agent: &mut Agent, underlying: Option<IterRec>, kind: HelperKind) -> Value {
    let proto = agent.intrinsics().iterator_helper_prototype;
    Value::Object(agent.alloc(Object::new(
        Some(proto),
        ObjectKind::IteratorHelper(Box::new(HelperData {
            state: HelperState::SuspendedStart,
            underlying,
            kind,
        })),
    )))
}

/// Un límite de `take`/`drop` (§27.1.4.3 pasos 4–7): ToNumber, NaN → RangeError, negativo
/// → RangeError (cerrando el iterador en cada error).
fn limit(agent: &mut Agent, o: &Value, v: &Value) -> JsResult<f64> {
    let n = match agent.to_number(v) {
        Ok(n) => n,
        Err(e) => return invalid(agent, o, e),
    };
    if n.is_nan() {
        let e = agent.error(crate::vm::ErrorType::RangeError, "limit must be a number");
        return invalid(agent, o, e);
    }
    let i = crate::abstract_ops::conversion::integer_or_infinity(n);
    if i < 0.0 || (i.is_finite() && i > 9007199254740991.0) {
        let e = agent.error(crate::vm::ErrorType::RangeError, "limit out of range");
        return invalid(agent, o, e);
    }
    Ok(i)
}

fn callable_arg(agent: &mut Agent, o: &Value, f: &Value) -> JsResult<()> {
    if !agent.is_callable(f) {
        let e = agent.error(
            crate::vm::ErrorType::TypeError,
            "argument is not a function",
        );
        return invalid(agent, o, e);
    }
    Ok(())
}

// --- %Iterator% ---

/// Iterator ( ) (§27.1.3.1).
fn iterator_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    match a.new_target {
        None => agent.type_error("Constructor Iterator requires 'new'"),
        Some(nt) if nt == a.callee => {
            agent.type_error("Abstract class Iterator not directly constructable")
        }
        Some(nt) => {
            let o = agent.ordinary_create_from_constructor(
                nt,
                |i| i.iterator_prototype,
                ObjectKind::Ordinary,
            )?;
            Ok(Value::Object(o))
        }
    }
}

/// Iterator.from ( O ) (§27.1.3.2.1).
fn from(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let rec = get_iterator_flattenable(agent, &a.arg(0), true)?;
    let ctor = Value::Object(agent.intrinsics().iterator_ctor);
    if agent.ordinary_has_instance(&ctor, &rec.iterator)? {
        return Ok(rec.iterator);
    }
    let proto = agent.intrinsics().wrap_for_valid_iterator_prototype;
    Ok(Value::Object(agent.alloc(Object::new(
        Some(proto),
        ObjectKind::WrappedIterator(Box::new(rec)),
    ))))
}

fn wrapped(agent: &mut Agent, this: &Value) -> JsResult<IterRec> {
    if let Value::Object(o) = this
        && let ObjectKind::WrappedIterator(r) = &agent.heap[*o].kind
    {
        return Ok((**r).clone());
    }
    agent.type_error("not a wrapped iterator")
}

/// %WrapForValidIteratorPrototype%.next (§27.1.3.2.1.1.1).
fn wrap_next(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = wrapped(agent, &a.this)?;
    agent.call(&r.next, r.iterator, &[])
}

/// %WrapForValidIteratorPrototype%.return (§27.1.3.2.1.1.2).
fn wrap_return(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = wrapped(agent, &a.this)?;
    let k = agent.keys.return_.clone();
    match agent.get_method_of(&r.iterator, &k)? {
        None => agent.create_iter_result(Value::Undefined, true),
        Some(m) => agent.call(&m, r.iterator, &[]),
    }
}

/// Iterator.concat ( ...items ) (iterator-sequencing).
fn concat(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let mut items = Vec::new();
    let key = PropertyKey::Symbol(agent.well_known.iterator);
    for item in a.args {
        if !item.is_object() {
            return agent.type_error("Iterator.concat: argument is not an object");
        }
        let Some(m) = agent.get_method_of(item, &key)? else {
            return agent.type_error("Iterator.concat: argument is not iterable");
        };
        items.push((item.clone(), m));
    }
    Ok(new_helper(
        agent,
        None,
        HelperKind::Concat {
            items,
            index: 0,
            inner: None,
        },
    ))
}

/// Las opciones de zip/zipKeyed: (mode, padding).
fn zip_options(agent: &mut Agent, options: &Value) -> JsResult<(ZipMode, Value)> {
    let opts = match options {
        Value::Undefined => return Ok((ZipMode::Shortest, Value::Undefined)),
        Value::Object(_) => options.clone(),
        _ => return agent.type_error("options must be an object"),
    };
    let mode = agent.get_value_of(&opts, &PropertyKey::from("mode"))?;
    let mode = match &mode {
        Value::Undefined => ZipMode::Shortest,
        Value::String(s) if s.units() == "shortest".encode_utf16().collect::<Vec<_>>() => {
            ZipMode::Shortest
        }
        Value::String(s) if s.units() == "longest".encode_utf16().collect::<Vec<_>>() => {
            ZipMode::Longest
        }
        Value::String(s) if s.units() == "strict".encode_utf16().collect::<Vec<_>>() => {
            ZipMode::Strict
        }
        _ => return agent.type_error("mode must be \"shortest\", \"longest\" or \"strict\""),
    };
    let mut padding = Value::Undefined;
    if mode == ZipMode::Longest {
        padding = agent.get_value_of(&opts, &PropertyKey::from("padding"))?;
        if !padding.is_undefined() && !padding.is_object() {
            return agent.type_error("padding must be an object");
        }
    }
    Ok((mode, padding))
}

/// IfAbruptCloseIterators: cierra `iters` (en orden inverso) y relanza.
fn abrupt_close_iters<T>(agent: &mut Agent, iters: &[IterRec], e: Throw) -> JsResult<T> {
    let _ = close_all(agent, iters, Err(e.clone()));
    Err(e)
}

/// Iterator.zip ( iterables [ , options ] ) (joint-iteration).
fn zip(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let iterables = a.arg(0);
    if !iterables.is_object() {
        return agent.type_error("Iterator.zip: iterables is not an object");
    }
    let (mode, padding_opt) = zip_options(agent, &a.arg(1))?;
    let (input_it, input_next) = agent.get_iterator(&iterables)?;
    let input = IterRec {
        iterator: input_it,
        next: input_next,
    };
    let mut iters: Vec<IterRec> = Vec::new();
    loop {
        let next = match step_value(agent, &input) {
            Ok(n) => n,
            Err(e) => return abrupt_close_iters(agent, &iters, e),
        };
        let Some(v) = next else { break };
        match get_iterator_flattenable(agent, &v, false) {
            Ok(r) => iters.push(r),
            Err(e) => {
                let mut all = vec![input.clone()];
                all.extend(iters.iter().cloned());
                return abrupt_close_iters(agent, &all, e);
            }
        }
    }
    let count = iters.len();
    let mut padding = Vec::new();
    if mode == ZipMode::Longest {
        if padding_opt.is_undefined() {
            padding = vec![Value::Undefined; count];
        } else {
            let (pit, pnext) = match agent.get_iterator(&padding_opt) {
                Ok(p) => p,
                Err(e) => return abrupt_close_iters(agent, &iters, e),
            };
            let prec = IterRec {
                iterator: pit,
                next: pnext,
            };
            let mut using = true;
            for _ in 0..count {
                if using {
                    match step_value(agent, &prec) {
                        Err(e) => return abrupt_close_iters(agent, &iters, e),
                        Ok(None) => using = false,
                        Ok(Some(v)) => padding.push(v),
                    }
                }
                if !using {
                    padding.push(Value::Undefined);
                }
            }
            if using && let Err(e) = close(agent, &prec) {
                return abrupt_close_iters(agent, &iters, e);
            }
        }
    }
    Ok(new_zip(agent, iters, mode, padding, None))
}

fn new_zip(
    agent: &mut Agent,
    iters: Vec<IterRec>,
    mode: ZipMode,
    padding: Vec<Value>,
    keys: Option<Vec<PropertyKey>>,
) -> Value {
    let open = (0..iters.len()).collect();
    new_helper(
        agent,
        None,
        HelperKind::Zip {
            iters: iters.into_iter().map(Some).collect(),
            open,
            mode,
            padding,
            keys,
        },
    )
}

/// Iterator.zipKeyed ( iterables [ , options ] ) (joint-iteration).
fn zip_keyed(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let iterables = a.arg(0);
    let Value::Object(io) = iterables else {
        return agent.type_error("Iterator.zipKeyed: iterables is not an object");
    };
    let (mode, padding_opt) = zip_options(agent, &a.arg(1))?;
    let all_keys = agent.own_property_keys(io)?;
    let mut keys = Vec::new();
    let mut iters: Vec<IterRec> = Vec::new();
    for k in all_keys {
        let desc = match agent.get_own_property(io, &k) {
            Ok(d) => d,
            Err(e) => return abrupt_close_iters(agent, &iters, e),
        };
        if desc.is_some_and(|d| d.enumerable == Some(true)) {
            let v = match agent.get(io, &k, iterables.clone()) {
                Ok(v) => v,
                Err(e) => return abrupt_close_iters(agent, &iters, e),
            };
            if !v.is_undefined() {
                match get_iterator_flattenable(agent, &v, false) {
                    Ok(r) => {
                        keys.push(k);
                        iters.push(r);
                    }
                    Err(e) => return abrupt_close_iters(agent, &iters, e),
                }
            }
        }
    }
    let count = iters.len();
    let mut padding = Vec::new();
    if mode == ZipMode::Longest {
        if padding_opt.is_undefined() {
            padding = vec![Value::Undefined; count];
        } else {
            for k in &keys {
                match agent.get_value_of(&padding_opt, k) {
                    Ok(v) => padding.push(v),
                    Err(e) => return abrupt_close_iters(agent, &iters, e),
                }
            }
        }
    }
    Ok(new_zip(agent, iters, mode, padding, Some(keys)))
}

// --- %Iterator.prototype% ---

fn ctor_get(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Object(agent.intrinsics().iterator_ctor))
}

/// SetterThatIgnoresPrototypeProperties (§27.1.4.1.1).
fn setter_ignoring_prototype(
    agent: &mut Agent,
    this: &Value,
    key: PropertyKey,
    v: Value,
) -> JsResult<Value> {
    let Value::Object(o) = this else {
        return agent.type_error("setter called on a non-object");
    };
    let home = agent.intrinsics().iterator_prototype;
    if *o == home {
        return agent.type_error("Cannot assign to this property of Iterator.prototype");
    }
    if agent.get_own_property(*o, &key)?.is_none() {
        agent.create_data_property_or_throw(*o, key, v)?;
    } else {
        agent.set_or_throw(*o, key, v, true)?;
    }
    Ok(Value::Undefined)
}

fn ctor_set(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let k = agent.keys.constructor.clone();
    setter_ignoring_prototype(agent, &a.this, k, a.arg(0))
}

fn tag_get(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str("Iterator"))
}

fn tag_set(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let k = PropertyKey::Symbol(agent.well_known.to_string_tag);
    setter_ignoring_prototype(agent, &a.this, k, a.arg(0))
}

/// %Iterator.prototype% [ @@dispose ] ( ) (explicit-resource-management).
fn dispose(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let k = agent.keys.return_.clone();
    if let Some(m) = agent.get_method_of(&a.this, &k)? {
        agent.call(&m, a.this.clone(), &[])?;
    }
    Ok(Value::Undefined)
}

fn map(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_object(agent, a)?;
    callable_arg(agent, &o, &a.arg(0))?;
    let r = get_iterator_direct(agent, &o)?;
    Ok(new_helper(
        agent,
        Some(r),
        HelperKind::Map {
            f: a.arg(0),
            counter: 0.0,
        },
    ))
}

fn filter(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_object(agent, a)?;
    callable_arg(agent, &o, &a.arg(0))?;
    let r = get_iterator_direct(agent, &o)?;
    Ok(new_helper(
        agent,
        Some(r),
        HelperKind::Filter {
            f: a.arg(0),
            counter: 0.0,
        },
    ))
}

fn take(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_object(agent, a)?;
    let n = limit(agent, &o, &a.arg(0))?;
    let r = get_iterator_direct(agent, &o)?;
    Ok(new_helper(
        agent,
        Some(r),
        HelperKind::Take { remaining: n },
    ))
}

fn drop(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_object(agent, a)?;
    let n = limit(agent, &o, &a.arg(0))?;
    let r = get_iterator_direct(agent, &o)?;
    Ok(new_helper(
        agent,
        Some(r),
        HelperKind::Drop { remaining: n },
    ))
}

fn flat_map(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_object(agent, a)?;
    callable_arg(agent, &o, &a.arg(0))?;
    let r = get_iterator_direct(agent, &o)?;
    Ok(new_helper(
        agent,
        Some(r),
        HelperKind::FlatMap {
            f: a.arg(0),
            counter: 0.0,
            inner: None,
        },
    ))
}

/// El tamaño de `chunks`/`windows`: Number entero en [1, 2³²−1] (sin coerción).
fn size_arg(agent: &mut Agent, o: &Value, v: &Value) -> JsResult<usize> {
    let Value::Number(n) = v else {
        let e = agent.error(crate::vm::ErrorType::TypeError, "size must be a number");
        return invalid(agent, o, e);
    };
    if !(n.is_finite() && n.trunc() == *n) {
        let e = agent.error(crate::vm::ErrorType::TypeError, "size must be an integer");
        return invalid(agent, o, e);
    }
    if !(1.0..=4294967295.0).contains(n) {
        let e = agent.error(crate::vm::ErrorType::RangeError, "size out of range");
        return invalid(agent, o, e);
    }
    Ok(*n as usize)
}

/// Iterator.prototype.chunks ( chunkSize ) (iterator-chunking).
fn chunks(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_object(agent, a)?;
    let size = size_arg(agent, &o, &a.arg(0))?;
    let r = get_iterator_direct(agent, &o)?;
    Ok(new_helper(agent, Some(r), HelperKind::Chunks { size }))
}

/// Iterator.prototype.windows ( windowSize [ , undersized ] ) (iterator-chunking).
fn windows(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_object(agent, a)?;
    let size = size_arg(agent, &o, &a.arg(0))?;
    let allow_partial = match a.arg(1) {
        Value::Undefined => false,
        Value::String(s) if s.units() == "only-full".encode_utf16().collect::<Vec<_>>() => false,
        Value::String(s) if s.units() == "allow-partial".encode_utf16().collect::<Vec<_>>() => true,
        _ => {
            let e = agent.error(
                crate::vm::ErrorType::TypeError,
                "undersized must be \"only-full\" or \"allow-partial\"",
            );
            return invalid(agent, &o, e);
        }
    };
    let r = get_iterator_direct(agent, &o)?;
    Ok(new_helper(
        agent,
        Some(r),
        HelperKind::Windows {
            size,
            allow_partial,
            buffer: Vec::new(),
        },
    ))
}

/// Iterator.prototype.reduce ( reducer [ , initialValue ] ) (§27.1.4.8).
fn reduce(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_object(agent, a)?;
    let f = a.arg(0);
    callable_arg(agent, &o, &f)?;
    let r = get_iterator_direct(agent, &o)?;
    let (mut acc, mut counter) = if a.len() < 2 {
        match step_value(agent, &r)? {
            None => return agent.type_error("Reduce of empty iterator with no initial value"),
            Some(v) => (v, 1.0),
        }
    } else {
        (a.arg(1), 0.0)
    };
    loop {
        let Some(v) = step_value(agent, &r)? else {
            return Ok(acc);
        };
        acc = match agent.call(&f, Value::Undefined, &[acc, v, Value::Number(counter)]) {
            Ok(x) => x,
            Err(e) => return close_throw(agent, &r, e),
        };
        counter += 1.0;
    }
}

/// Iterator.prototype.toArray ( ) (§27.1.4.12).
fn to_array(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_object(agent, a)?;
    let r = get_iterator_direct(agent, &o)?;
    let mut items = Vec::new();
    while let Some(v) = step_value(agent, &r)? {
        items.push(v);
    }
    Ok(Value::Object(agent.create_array_from_list(&items)?))
}

/// forEach / some / every / find (§27.1.4.5–7, §27.1.4.10).
fn consume(agent: &mut Agent, a: &CallArgs<'_>, which: u8) -> JsResult<Value> {
    let o = this_object(agent, a)?;
    let f = a.arg(0);
    callable_arg(agent, &o, &f)?;
    let r = get_iterator_direct(agent, &o)?;
    let mut counter = 0.0;
    loop {
        let Some(v) = step_value(agent, &r)? else {
            return Ok(match which {
                0 | 3 => Value::Undefined,
                1 => Value::Boolean(false),
                _ => Value::Boolean(true),
            });
        };
        let res = match agent.call(&f, Value::Undefined, &[v.clone(), Value::Number(counter)]) {
            Ok(x) => x,
            Err(e) => return close_throw(agent, &r, e),
        };
        let b = to_boolean(agent, &res);
        match which {
            1 if b => {
                close(agent, &r)?;
                return Ok(Value::Boolean(true));
            }
            2 if !b => {
                close(agent, &r)?;
                return Ok(Value::Boolean(false));
            }
            3 if b => {
                close(agent, &r)?;
                return Ok(v);
            }
            _ => {}
        }
        counter += 1.0;
    }
}

fn for_each(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    consume(agent, a, 0)
}

fn some(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    consume(agent, a, 1)
}

fn every(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    consume(agent, a, 2)
}

fn find(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    consume(agent, a, 3)
}

/// Iterator.prototype.includes ( searchElement [ , skippedElements ] ) (iterator-includes).
fn includes(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_object(agent, a)?;
    let to_skip = match a.arg(1) {
        Value::Undefined => 0.0,
        Value::Number(n) if n.is_infinite() || n.trunc() == n => n,
        _ => {
            let e = agent.error(
                crate::vm::ErrorType::TypeError,
                "skippedElements must be an integer",
            );
            return invalid(agent, &o, e);
        }
    };
    if to_skip < 0.0 {
        let e = agent.error(
            crate::vm::ErrorType::RangeError,
            "skippedElements must be positive",
        );
        return invalid(agent, &o, e);
    }
    if to_skip.is_finite() && to_skip > 9007199254740991.0 {
        let e = agent.error(
            crate::vm::ErrorType::RangeError,
            "skippedElements too large",
        );
        return invalid(agent, &o, e);
    }
    let r = get_iterator_direct(agent, &o)?;
    let target = a.arg(0);
    let mut skipped = 0.0;
    loop {
        let Some(v) = step_value(agent, &r)? else {
            return Ok(Value::Boolean(false));
        };
        if skipped < to_skip {
            skipped += 1.0;
            continue;
        }
        if agent.same_value_zero(&v, &target) {
            close(agent, &r)?;
            return Ok(Value::Boolean(true));
        }
    }
}

/// Iterator.prototype.join ( separator ) (propuesta Iterator Join).
fn join(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_object(agent, a)?;
    let sep = if a.arg(0).is_undefined() {
        JsString::from(",")
    } else {
        match agent.to_string(&a.arg(0)) {
            Ok(s) => s,
            Err(e) => return invalid(agent, &o, e),
        }
    };
    let r = get_iterator_direct(agent, &o)?;
    let mut out: Vec<u16> = Vec::new();
    let mut first = true;
    while let Some(v) = step_value(agent, &r)? {
        if !first {
            out.extend_from_slice(sep.units());
        }
        first = false;
        if !v.is_nullish() {
            match agent.to_string(&v) {
                Ok(s) => out.extend_from_slice(s.units()),
                Err(e) => return close_throw(agent, &r, e),
            }
        }
    }
    Ok(Value::String(JsString::from_units(out)))
}

// --- %IteratorHelperPrototype% ---

fn helper_obj(agent: &mut Agent, this: &Value) -> JsResult<Gc<Object>> {
    if let Value::Object(o) = this
        && matches!(agent.heap[*o].kind, ObjectKind::IteratorHelper(_))
    {
        return Ok(*o);
    }
    agent.type_error("not an Iterator Helper object")
}

fn data(agent: &mut Agent, o: Gc<Object>) -> &mut HelperData {
    match &mut agent.heap[o].kind {
        ObjectKind::IteratorHelper(d) => d,
        _ => unreachable!("iterator helper"),
    }
}

/// %IteratorHelperPrototype%.next (§27.1.2.1.1): GeneratorResume.
fn helper_next(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = helper_obj(agent, &a.this)?;
    match data(agent, o).state {
        HelperState::Executing => return agent.type_error("Iterator Helper is already running"),
        HelperState::Completed => return agent.create_iter_result(Value::Undefined, true),
        _ => {}
    }
    data(agent, o).state = HelperState::Executing;
    let r = step(agent, o);
    match r {
        Ok(Some(v)) => {
            data(agent, o).state = HelperState::SuspendedYield;
            agent.create_iter_result(v, false)
        }
        Ok(None) => {
            data(agent, o).state = HelperState::Completed;
            agent.create_iter_result(Value::Undefined, true)
        }
        Err(e) => {
            data(agent, o).state = HelperState::Completed;
            Err(e)
        }
    }
}

/// %IteratorHelperPrototype%.return (§27.1.2.1.2).
fn helper_return(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = helper_obj(agent, &a.this)?;
    let state = data(agent, o).state;
    match state {
        HelperState::Executing => return agent.type_error("Iterator Helper is already running"),
        HelperState::Completed => return agent.create_iter_result(Value::Undefined, true),
        _ => {}
    }
    // GeneratorResumeAbrupt: mientras se cierran los subyacentes el helper está "executing"
    // (un `return()` reentrante lanza TypeError); en suspended-start ya queda completado.
    data(agent, o).state = if state == HelperState::SuspendedStart {
        HelperState::Completed
    } else {
        HelperState::Executing
    };
    let d = data(agent, o);
    let underlying = d.underlying.clone();
    let r: JsResult<()> = match (&mut d.kind, state) {
        (HelperKind::Concat { inner, .. }, _) => match inner.take() {
            Some(i) => close(agent, &i),
            None => Ok(()),
        },
        (HelperKind::Zip { iters, open, .. }, _) => {
            let recs: Vec<IterRec> = open.iter().filter_map(|&i| iters[i].clone()).collect();
            close_all(agent, &recs, Ok(()))
        }
        (HelperKind::FlatMap { inner, .. }, HelperState::SuspendedYield) => {
            let inner = inner.take();
            let u = underlying.expect("subyacente");
            match inner {
                Some(i) => match close(agent, &i) {
                    Ok(()) => close(agent, &u),
                    Err(e) => close_throw(agent, &u, e),
                },
                None => close(agent, &u),
            }
        }
        _ => match underlying {
            Some(u) => close(agent, &u),
            None => Ok(()),
        },
    };
    data(agent, o).state = HelperState::Completed;
    r?;
    agent.create_iter_result(Value::Undefined, true)
}

/// Un paso del closure del helper: `Some(valor)` en el Yield, `None` al terminar.
fn step(agent: &mut Agent, o: Gc<Object>) -> JsResult<Option<Value>> {
    let u = data(agent, o).underlying.clone();
    let kind = match &data(agent, o).kind {
        HelperKind::Map { .. } => 0,
        HelperKind::Filter { .. } => 1,
        HelperKind::Take { .. } => 2,
        HelperKind::Drop { .. } => 3,
        HelperKind::FlatMap { .. } => 4,
        HelperKind::Concat { .. } => 5,
        HelperKind::Chunks { .. } => 6,
        HelperKind::Windows { .. } => 7,
        HelperKind::Zip { .. } => 8,
    };
    match kind {
        0 | 1 => {
            let r = u.expect("subyacente");
            loop {
                let Some(v) = step_value(agent, &r)? else {
                    return Ok(None);
                };
                let (f, counter) = match &mut data(agent, o).kind {
                    HelperKind::Map { f, counter } | HelperKind::Filter { f, counter } => {
                        let c = *counter;
                        *counter += 1.0;
                        (f.clone(), c)
                    }
                    _ => unreachable!(),
                };
                let res =
                    match agent.call(&f, Value::Undefined, &[v.clone(), Value::Number(counter)]) {
                        Ok(x) => x,
                        Err(e) => return close_throw(agent, &r, e),
                    };
                if kind == 0 {
                    return Ok(Some(res));
                }
                if to_boolean(agent, &res) {
                    return Ok(Some(v));
                }
            }
        }
        2 => {
            let r = u.expect("subyacente");
            let remaining = match &mut data(agent, o).kind {
                HelperKind::Take { remaining } => remaining,
                _ => unreachable!(),
            };
            if *remaining == 0.0 {
                close(agent, &r)?;
                return Ok(None);
            }
            if *remaining != f64::INFINITY {
                *remaining -= 1.0;
            }
            step_value(agent, &r)
        }
        3 => {
            let r = u.expect("subyacente");
            loop {
                let remaining = match &mut data(agent, o).kind {
                    HelperKind::Drop { remaining } => remaining,
                    _ => unreachable!(),
                };
                if *remaining <= 0.0 {
                    break;
                }
                if *remaining != f64::INFINITY {
                    *remaining -= 1.0;
                }
                let n = agent.iterator_next(&r.iterator, &r.next, None)?;
                if agent.iterator_complete(n)? {
                    return Ok(None);
                }
            }
            step_value(agent, &r)
        }
        4 => {
            let r = u.expect("subyacente");
            loop {
                let inner = match &data(agent, o).kind {
                    HelperKind::FlatMap { inner, .. } => inner.clone(),
                    _ => unreachable!(),
                };
                if let Some(i) = inner {
                    match step_value(agent, &i) {
                        Err(e) => return close_throw(agent, &r, e),
                        Ok(Some(v)) => return Ok(Some(v)),
                        Ok(None) => {
                            if let HelperKind::FlatMap { inner, .. } = &mut data(agent, o).kind {
                                *inner = None;
                            }
                            continue;
                        }
                    }
                }
                let Some(v) = step_value(agent, &r)? else {
                    return Ok(None);
                };
                let (f, counter) = match &mut data(agent, o).kind {
                    HelperKind::FlatMap { f, counter, .. } => {
                        let c = *counter;
                        *counter += 1.0;
                        (f.clone(), c)
                    }
                    _ => unreachable!(),
                };
                let mapped = match agent.call(&f, Value::Undefined, &[v, Value::Number(counter)]) {
                    Ok(x) => x,
                    Err(e) => return close_throw(agent, &r, e),
                };
                let it = match get_iterator_flattenable(agent, &mapped, false) {
                    Ok(x) => x,
                    Err(e) => return close_throw(agent, &r, e),
                };
                if let HelperKind::FlatMap { inner, .. } = &mut data(agent, o).kind {
                    *inner = Some(it);
                }
            }
        }
        5 => loop {
            let (inner, item) = match &data(agent, o).kind {
                HelperKind::Concat {
                    items,
                    index,
                    inner,
                } => (inner.clone(), items.get(*index).cloned()),
                _ => unreachable!(),
            };
            if let Some(i) = inner {
                match step_value(agent, &i)? {
                    Some(v) => return Ok(Some(v)),
                    None => {
                        if let HelperKind::Concat { inner, index, .. } = &mut data(agent, o).kind {
                            *inner = None;
                            *index += 1;
                        }
                        continue;
                    }
                }
            }
            let Some((obj, m)) = item else {
                return Ok(None);
            };
            let it = agent.call(&m, obj, &[])?;
            if !it.is_object() {
                return agent.type_error("iterator is not an object");
            }
            let rec = get_iterator_direct(agent, &it)?;
            if let HelperKind::Concat { inner, .. } = &mut data(agent, o).kind {
                *inner = Some(rec);
            }
        },
        6 => {
            let r = u.expect("subyacente");
            let size = match &data(agent, o).kind {
                HelperKind::Chunks { size } => *size,
                _ => unreachable!(),
            };
            let mut buffer = Vec::new();
            loop {
                match step_value(agent, &r)? {
                    None => {
                        if buffer.is_empty() {
                            return Ok(None);
                        }
                        return Ok(Some(Value::Object(agent.create_array_from_list(&buffer)?)));
                    }
                    Some(v) => {
                        buffer.push(v);
                        if buffer.len() == size {
                            return Ok(Some(Value::Object(agent.create_array_from_list(&buffer)?)));
                        }
                    }
                }
            }
        }
        7 => {
            let r = u.expect("subyacente");
            loop {
                let v = step_value(agent, &r)?;
                let HelperKind::Windows {
                    size,
                    allow_partial,
                    buffer,
                } = &mut data(agent, o).kind
                else {
                    unreachable!()
                };
                let (size, allow_partial) = (*size, *allow_partial);
                match v {
                    None => {
                        if allow_partial && !buffer.is_empty() && buffer.len() < size {
                            let b = std::mem::take(buffer);
                            return Ok(Some(Value::Object(agent.create_array_from_list(&b)?)));
                        }
                        return Ok(None);
                    }
                    Some(v) => {
                        if buffer.len() == size {
                            buffer.remove(0);
                        }
                        buffer.push(v);
                        if buffer.len() == size {
                            let b = buffer.clone();
                            return Ok(Some(Value::Object(agent.create_array_from_list(&b)?)));
                        }
                    }
                }
            }
        }
        _ => zip_step(agent, o),
    }
}

/// Un paso de IteratorZip (joint-iteration).
fn zip_step(agent: &mut Agent, o: Gc<Object>) -> JsResult<Option<Value>> {
    let (count, mode) = match &data(agent, o).kind {
        HelperKind::Zip { iters, mode, .. } => (iters.len(), *mode),
        _ => unreachable!(),
    };
    if count == 0 {
        return Ok(None);
    }
    let open_recs = |agent: &mut Agent| -> Vec<IterRec> {
        match &data(agent, o).kind {
            HelperKind::Zip { iters, open, .. } => {
                open.iter().filter_map(|&i| iters[i].clone()).collect()
            }
            _ => Vec::new(),
        }
    };
    let remove_open = |agent: &mut Agent, i: usize| {
        if let HelperKind::Zip { open, .. } = &mut data(agent, o).kind {
            open.retain(|&x| x != i);
        }
    };
    let mut results = Vec::with_capacity(count);
    for i in 0..count {
        let (iter, pad) = match &data(agent, o).kind {
            HelperKind::Zip { iters, padding, .. } => (
                iters[i].clone(),
                padding.get(i).cloned().unwrap_or_default(),
            ),
            _ => unreachable!(),
        };
        let Some(iter) = iter else {
            results.push(pad);
            continue;
        };
        match step_value(agent, &iter) {
            Err(e) => {
                remove_open(agent, i);
                let recs = open_recs(agent);
                close_all(agent, &recs, Err(e.clone()))?;
                return Err(e);
            }
            Ok(Some(v)) => results.push(v),
            Ok(None) => {
                remove_open(agent, i);
                match mode {
                    ZipMode::Shortest => {
                        let recs = open_recs(agent);
                        close_all(agent, &recs, Ok(()))?;
                        return Ok(None);
                    }
                    ZipMode::Strict => {
                        if i != 0 {
                            let recs = open_recs(agent);
                            let e = agent.error(
                                crate::vm::ErrorType::TypeError,
                                "Iterator.zip: iterators have different lengths",
                            );
                            close_all(agent, &recs, Err(e.clone()))?;
                            return Err(e);
                        }
                        for k in 1..count {
                            let rk = match &data(agent, o).kind {
                                HelperKind::Zip { iters, .. } => iters[k].clone(),
                                _ => None,
                            };
                            let Some(rk) = rk else { continue };
                            let open_res = agent
                                .iterator_next(&rk.iterator, &rk.next, None)
                                .and_then(|n| agent.iterator_complete(n));
                            match open_res {
                                Err(e) => {
                                    remove_open(agent, k);
                                    let recs = open_recs(agent);
                                    close_all(agent, &recs, Err(e.clone()))?;
                                    return Err(e);
                                }
                                Ok(true) => remove_open(agent, k),
                                Ok(false) => {
                                    let recs = open_recs(agent);
                                    let e = agent.error(
                                        crate::vm::ErrorType::TypeError,
                                        "Iterator.zip: iterators have different lengths",
                                    );
                                    close_all(agent, &recs, Err(e.clone()))?;
                                    return Err(e);
                                }
                            }
                        }
                        return Ok(None);
                    }
                    ZipMode::Longest => {
                        if open_recs(agent).is_empty() {
                            return Ok(None);
                        }
                        if let HelperKind::Zip { iters, .. } = &mut data(agent, o).kind {
                            iters[i] = None;
                        }
                        results.push(pad);
                    }
                }
            }
        }
    }
    let keys = match &data(agent, o).kind {
        HelperKind::Zip { keys, .. } => keys.clone(),
        _ => None,
    };
    match keys {
        None => Ok(Some(Value::Object(agent.create_array_from_list(&results)?))),
        Some(keys) => {
            let obj = agent.new_object(None);
            for (k, v) in keys.into_iter().zip(results) {
                agent.define_property_or_throw(
                    obj,
                    k,
                    PropertyDescriptor::data(v, true, true, true),
                )?;
            }
            Ok(Some(Value::Object(obj)))
        }
    }
}
