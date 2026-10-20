//! Array (§23.1): el constructor, `isArray`, `of`, `from` y los métodos de Array.prototype de
//! esta etapa (el resto llega con los built-ins del paso 3).

use crate::builtins::iterator::create_array_iterator;
use crate::builtins::{FnEntry, accessor, constructor, methods};
use crate::heap::Gc;
use crate::object::{IterKind, Object, ObjectKind};
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

pub fn init(agent: &mut Agent, realm: RealmId) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    // Array.prototype es un Array exótico (§23.1.3).
    let proto = agent.alloc(Object::new(
        Some(op),
        ObjectKind::Array(crate::object::ArrayData {
            dense: Vec::new(),
            length: 0,
            length_writable: true,
        }),
    ));
    agent.realms[realm].intrinsics.array_prototype = proto;
    let ctor = constructor(agent, "Array", 1, array_ctor, proto);
    agent.realms[realm].intrinsics.array_ctor = ctor;
    const STATICS: &[FnEntry] = &[("from", 1, from), ("isArray", 1, is_array), ("of", 0, of)];
    methods(agent, ctor, STATICS);
    let species = PropertyKey::Symbol(agent.well_known.species);
    accessor(
        agent,
        ctor,
        species,
        "[Symbol.species]",
        Some(species_getter),
        None,
    );
    const PROTO: &[FnEntry] = &[
        ("at", 1, at),
        ("concat", 1, concat),
        ("entries", 0, entries),
        ("every", 1, every),
        ("filter", 1, filter),
        ("find", 1, find),
        ("findIndex", 1, find_index),
        ("forEach", 1, for_each),
        ("includes", 1, includes),
        ("indexOf", 1, index_of),
        ("join", 1, join),
        ("keys", 0, keys),
        ("map", 1, map),
        ("pop", 0, pop),
        ("push", 1, push),
        ("reduce", 1, reduce),
        ("shift", 0, shift),
        ("slice", 2, slice),
        ("some", 1, some),
        ("splice", 2, splice),
        ("unshift", 1, unshift),
    ];
    methods(agent, proto, PROTO);
    let values = crate::builtins::method(agent, proto, "values", 0, values);
    let it = agent.well_known.iterator;
    agent.put_data(
        proto,
        PropertyKey::Symbol(it),
        Value::Object(values),
        true,
        false,
        true,
    );
    agent.realms[realm].intrinsics.array_proto_values = values;
    // Array.prototype [ @@unscopables ] (§23.1.3.41).
    let u = agent.new_object(None);
    for name in [
        "at",
        "copyWithin",
        "entries",
        "fill",
        "find",
        "findIndex",
        "findLast",
        "findLastIndex",
        "flat",
        "flatMap",
        "includes",
        "keys",
        "toReversed",
        "toSorted",
        "toSpliced",
        "values",
    ] {
        agent.put_data(
            u,
            PropertyKey::from(name),
            Value::Boolean(true),
            true,
            true,
            true,
        );
    }
    let us = agent.well_known.unscopables;
    agent.put_data(
        proto,
        PropertyKey::Symbol(us),
        Value::Object(u),
        false,
        false,
        true,
    );
}

fn species_getter(_: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(a.this.clone())
}

/// Array ( ...values ) (§23.1.1.1).
fn array_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let nt = a.new_target.unwrap_or(a.callee);
    let proto = agent.get_prototype_from_constructor(nt, |i| i.array_prototype)?;
    match a.len() {
        0 => Ok(Value::Object(agent.array_create(0, Some(proto))?)),
        1 => {
            let len = a.arg(0);
            let arr = agent.array_create(0, Some(proto))?;
            let int_len = match len {
                Value::Number(n) => {
                    let i = crate::abstract_ops::conversion::to_uint32(n);
                    if f64::from(i) != n {
                        return agent.range_error("Invalid array length");
                    }
                    i
                }
                other => {
                    agent.create_data_property_or_throw(arr, PropertyKey::Index(0), other)?;
                    1
                }
            };
            agent.set_or_throw(
                arr,
                agent.keys.length.clone(),
                Value::Number(f64::from(int_len)),
                true,
            )?;
            Ok(Value::Object(arr))
        }
        _ => {
            let arr = agent.array_create(0, Some(proto))?;
            for (k, v) in a.args.iter().enumerate() {
                agent.create_data_property_or_throw(
                    arr,
                    PropertyKey::Index(k as u32),
                    v.clone(),
                )?;
            }
            Ok(Value::Object(arr))
        }
    }
}

/// Array.from (§23.1.2.1).
fn from(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let c = a.this.clone();
    let items = a.arg(0);
    let mapfn = a.arg(1);
    let this_arg = a.arg(2);
    let mapping = !mapfn.is_undefined();
    if mapping && !agent.is_callable(&mapfn) {
        return agent.type_error("Array.from: mapper is not a function");
    }
    let key = PropertyKey::Symbol(agent.well_known.iterator);
    let using = agent.get_method_of(&items, &key)?;
    if let Some(method) = using {
        let arr = if agent.is_constructor(&c) {
            match agent.construct(c.as_object().expect("constructor"), &[], None)? {
                Value::Object(o) => o,
                _ => return agent.type_error("constructor did not return an object"),
            }
        } else {
            agent.array_create(0, None)?
        };
        let (it, next) = agent.get_iterator_from_method(&items, &method)?;
        let mut k: u32 = 0;
        loop {
            let Some(v) = agent.iterator_step_value(&it, &next)? else {
                agent.set_or_throw(
                    arr,
                    agent.keys.length.clone(),
                    Value::Number(f64::from(k)),
                    true,
                )?;
                return Ok(Value::Object(arr));
            };
            let r = (|| -> JsResult<()> {
                let mapped = if mapping {
                    agent.call(&mapfn, this_arg.clone(), &[v, Value::Number(f64::from(k))])?
                } else {
                    v
                };
                agent.create_data_property_or_throw(arr, PropertyKey::Index(k), mapped)
            })();
            if let Err(e) = r {
                agent.iterator_close_on_throw(&it);
                return Err(e);
            }
            k += 1;
        }
    }
    // Array-like.
    let array_like = agent.to_object(&items)?;
    let len = agent.length_of_array_like(array_like)?;
    let arr = if agent.is_constructor(&c) {
        match agent.construct(
            c.as_object().expect("constructor"),
            &[Value::Number(len)],
            None,
        )? {
            Value::Object(o) => o,
            _ => return agent.type_error("constructor did not return an object"),
        }
    } else {
        agent.array_create(len as u64, None)?
    };
    let mut k = 0.0;
    while k < len {
        let pk = PropertyKey::from_number(k);
        let v = agent.get(array_like, &pk, Value::Object(array_like))?;
        let mapped = if mapping {
            agent.call(&mapfn, this_arg.clone(), &[v, Value::Number(k)])?
        } else {
            v
        };
        agent.create_data_property_or_throw(arr, pk, mapped)?;
        k += 1.0;
    }
    agent.set_or_throw(arr, agent.keys.length.clone(), Value::Number(len), true)?;
    Ok(Value::Object(arr))
}

fn is_array(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(agent.is_array(&a.arg(0))?))
}

/// Array.of (§23.1.2.3).
fn of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let len = a.len() as f64;
    let arr = if agent.is_constructor(&a.this) {
        match agent.construct(
            a.this.as_object().expect("constructor"),
            &[Value::Number(len)],
            None,
        )? {
            Value::Object(o) => o,
            _ => return agent.type_error("constructor did not return an object"),
        }
    } else {
        agent.array_create(len as u64, None)?
    };
    for (k, v) in a.args.iter().enumerate() {
        agent.create_data_property_or_throw(arr, PropertyKey::Index(k as u32), v.clone())?;
    }
    agent.set_or_throw(arr, agent.keys.length.clone(), Value::Number(len), true)?;
    Ok(Value::Object(arr))
}

// --- Ayudas ---

fn this_obj(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<(Gc<Object>, f64)> {
    let o = agent.to_object(&a.this)?;
    let len = agent.length_of_array_like(o)?;
    Ok((o, len))
}

fn key(k: f64) -> PropertyKey {
    PropertyKey::from_number(k)
}

/// Un índice relativo (ToIntegerOrInfinity con negativos desde el final).
fn relative(agent: &mut Agent, v: &Value, len: f64, default: f64) -> JsResult<f64> {
    if v.is_undefined() {
        return Ok(default);
    }
    let r = agent.to_integer_or_infinity(v)?;
    Ok(if r == f64::NEG_INFINITY {
        0.0
    } else if r < 0.0 {
        (len + r).max(0.0)
    } else {
        r.min(len)
    })
}

fn callback(agent: &mut Agent, a: &CallArgs<'_>, name: &str) -> JsResult<Value> {
    let f = a.arg(0);
    if !agent.is_callable(&f) {
        return agent.type_error(&format!(
            "Array.prototype.{name}: callback is not a function"
        ));
    }
    Ok(f)
}

fn set_length(agent: &mut Agent, o: Gc<Object>, len: f64) -> JsResult<()> {
    let k = agent.keys.length.clone();
    agent.set_or_throw(o, k, Value::Number(len), true)
}

// --- Array.prototype ---

/// Array.prototype.at (§23.1.3.1).
fn at(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    let rel = agent.to_integer_or_infinity(&a.arg(0))?;
    let k = if rel >= 0.0 { rel } else { len + rel };
    if k < 0.0 || k >= len {
        return Ok(Value::Undefined);
    }
    agent.get(o, &key(k), Value::Object(o))
}

/// IsConcatSpreadable (§23.1.3.2.1).
fn is_concat_spreadable(agent: &mut Agent, v: &Value) -> JsResult<bool> {
    let Value::Object(o) = v else {
        return Ok(false);
    };
    let k = PropertyKey::Symbol(agent.well_known.is_concat_spreadable);
    let s = agent.get(*o, &k, v.clone())?;
    if !s.is_undefined() {
        return Ok(crate::abstract_ops::conversion::to_boolean(agent, &s));
    }
    agent.is_array(v)
}

/// Array.prototype.concat (§23.1.3.2).
fn concat(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = agent.to_object(&a.this)?;
    let arr = agent.array_species_create(o, 0.0)?;
    let mut n = 0.0;
    let items = std::iter::once(Value::Object(o)).chain(a.args.iter().cloned());
    for e in items {
        if is_concat_spreadable(agent, &e)? {
            let eo = e.as_object().expect("objeto");
            let len = agent.length_of_array_like(eo)?;
            if n + len > 9007199254740991.0 {
                return agent.type_error("Array too long");
            }
            let mut k = 0.0;
            while k < len {
                let p = key(k);
                if agent.has_property(eo, &p)? {
                    let v = agent.get(eo, &p, e.clone())?;
                    agent.create_data_property_or_throw(arr, key(n), v)?;
                }
                n += 1.0;
                k += 1.0;
            }
        } else {
            if n >= 9007199254740991.0 {
                return agent.type_error("Array too long");
            }
            agent.create_data_property_or_throw(arr, key(n), e)?;
            n += 1.0;
        }
    }
    set_length(agent, arr, n)?;
    Ok(Value::Object(arr))
}

fn entries(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = agent.to_object(&a.this)?;
    Ok(create_array_iterator(agent, o, IterKind::Entries))
}

fn keys(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = agent.to_object(&a.this)?;
    Ok(create_array_iterator(agent, o, IterKind::Keys))
}

fn values(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = agent.to_object(&a.this)?;
    Ok(create_array_iterator(agent, o, IterKind::Values))
}

/// every / some / forEach: recorre los índices presentes.
fn iterate(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    name: &str,
    mut f: impl FnMut(&mut Agent, Value, f64, Value) -> JsResult<Option<Value>>,
) -> JsResult<Option<Value>> {
    let (o, len) = this_obj(agent, a)?;
    let cb = callback(agent, a, name)?;
    let this_arg = a.arg(1);
    let mut k = 0.0;
    while k < len {
        let p = key(k);
        if agent.has_property(o, &p)? {
            let v = agent.get(o, &p, Value::Object(o))?;
            let r = agent.call(
                &cb,
                this_arg.clone(),
                &[v.clone(), Value::Number(k), Value::Object(o)],
            )?;
            if let Some(out) = f(agent, v, k, r)? {
                return Ok(Some(out));
            }
        }
        k += 1.0;
    }
    Ok(None)
}

fn every(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = iterate(agent, a, "every", |ag, _, _, r| {
        Ok(if crate::abstract_ops::conversion::to_boolean(ag, &r) {
            None
        } else {
            Some(Value::Boolean(false))
        })
    })?;
    Ok(r.unwrap_or(Value::Boolean(true)))
}

fn some(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = iterate(agent, a, "some", |ag, _, _, r| {
        Ok(if crate::abstract_ops::conversion::to_boolean(ag, &r) {
            Some(Value::Boolean(true))
        } else {
            None
        })
    })?;
    Ok(r.unwrap_or(Value::Boolean(false)))
}

fn for_each(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    iterate(agent, a, "forEach", |_, _, _, _| Ok(None))?;
    Ok(Value::Undefined)
}

/// Array.prototype.filter (§23.1.3.8).
fn filter(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    let cb = callback(agent, a, "filter")?;
    let this_arg = a.arg(1);
    let arr = agent.array_species_create(o, 0.0)?;
    let mut to = 0.0;
    let mut k = 0.0;
    while k < len {
        let p = key(k);
        if agent.has_property(o, &p)? {
            let v = agent.get(o, &p, Value::Object(o))?;
            let r = agent.call(
                &cb,
                this_arg.clone(),
                &[v.clone(), Value::Number(k), Value::Object(o)],
            )?;
            if crate::abstract_ops::conversion::to_boolean(agent, &r) {
                agent.create_data_property_or_throw(arr, key(to), v)?;
                to += 1.0;
            }
        }
        k += 1.0;
    }
    Ok(Value::Object(arr))
}

/// find / findIndex (§23.1.3.9–10): todos los índices, presentes o no.
fn find_impl(agent: &mut Agent, a: &CallArgs<'_>, index: bool) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    let cb = callback(agent, a, if index { "findIndex" } else { "find" })?;
    let this_arg = a.arg(1);
    let mut k = 0.0;
    while k < len {
        let v = agent.get(o, &key(k), Value::Object(o))?;
        let r = agent.call(
            &cb,
            this_arg.clone(),
            &[v.clone(), Value::Number(k), Value::Object(o)],
        )?;
        if crate::abstract_ops::conversion::to_boolean(agent, &r) {
            return Ok(if index { Value::Number(k) } else { v });
        }
        k += 1.0;
    }
    Ok(if index {
        Value::Number(-1.0)
    } else {
        Value::Undefined
    })
}

fn find(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    find_impl(agent, a, false)
}

fn find_index(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    find_impl(agent, a, true)
}

/// Array.prototype.includes (§23.1.3.16).
fn includes(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    if len == 0.0 {
        return Ok(Value::Boolean(false));
    }
    let mut n = agent.to_integer_or_infinity(&a.arg(1))?;
    if n == f64::INFINITY {
        return Ok(Value::Boolean(false));
    }
    if n == f64::NEG_INFINITY {
        n = 0.0;
    }
    let mut k = if n >= 0.0 { n } else { (len + n).max(0.0) };
    let target = a.arg(0);
    while k < len {
        let v = agent.get(o, &key(k), Value::Object(o))?;
        if agent.same_value_zero(&v, &target) {
            return Ok(Value::Boolean(true));
        }
        k += 1.0;
    }
    Ok(Value::Boolean(false))
}

/// Array.prototype.indexOf (§23.1.3.17).
fn index_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    if len == 0.0 {
        return Ok(Value::Number(-1.0));
    }
    let mut n = agent.to_integer_or_infinity(&a.arg(1))?;
    if n == f64::INFINITY {
        return Ok(Value::Number(-1.0));
    }
    if n == f64::NEG_INFINITY {
        n = 0.0;
    }
    let mut k = if n >= 0.0 { n } else { (len + n).max(0.0) };
    let target = a.arg(0);
    while k < len {
        let p = key(k);
        if agent.has_property(o, &p)? {
            let v = agent.get(o, &p, Value::Object(o))?;
            if agent.is_strictly_equal(&v, &target) {
                return Ok(Value::Number(k));
            }
        }
        k += 1.0;
    }
    Ok(Value::Number(-1.0))
}

/// Array.prototype.join (§23.1.3.18).
fn join(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    let sep = if a.arg(0).is_undefined() {
        crate::string::JsString::from(",")
    } else {
        agent.to_string(&a.arg(0))?
    };
    let mut out: Vec<u16> = Vec::new();
    let mut k = 0.0;
    while k < len {
        if k > 0.0 {
            out.extend_from_slice(sep.units());
        }
        let e = agent.get(o, &key(k), Value::Object(o))?;
        if !e.is_nullish() {
            let s = agent.to_string(&e)?;
            out.extend_from_slice(s.units());
        }
        k += 1.0;
    }
    Ok(Value::String(crate::string::JsString::from_units(out)))
}

/// Array.prototype.map (§23.1.3.21).
fn map(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    let cb = callback(agent, a, "map")?;
    let this_arg = a.arg(1);
    let arr = agent.array_species_create(o, len)?;
    let mut k = 0.0;
    while k < len {
        let p = key(k);
        if agent.has_property(o, &p)? {
            let v = agent.get(o, &p, Value::Object(o))?;
            let r = agent.call(
                &cb,
                this_arg.clone(),
                &[v, Value::Number(k), Value::Object(o)],
            )?;
            agent.create_data_property_or_throw(arr, p, r)?;
        }
        k += 1.0;
    }
    Ok(Value::Object(arr))
}

/// Array.prototype.pop (§23.1.3.22).
fn pop(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    if len == 0.0 {
        set_length(agent, o, 0.0)?;
        return Ok(Value::Undefined);
    }
    let idx = key(len - 1.0);
    let e = agent.get(o, &idx, Value::Object(o))?;
    agent.delete_property_or_throw(o, &idx)?;
    set_length(agent, o, len - 1.0)?;
    Ok(e)
}

/// Array.prototype.push (§23.1.3.23).
fn push(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    let n = a.len() as f64;
    if len + n > 9007199254740991.0 {
        return agent.type_error("Pushing too many elements");
    }
    let mut l = len;
    for v in a.args {
        agent.set_or_throw(o, key(l), v.clone(), true)?;
        l += 1.0;
    }
    set_length(agent, o, l)?;
    Ok(Value::Number(l))
}

/// Array.prototype.reduce (§23.1.3.24).
fn reduce(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    let cb = callback(agent, a, "reduce")?;
    if len == 0.0 && a.len() < 2 {
        return agent.type_error("Reduce of empty array with no initial value");
    }
    let mut k = 0.0;
    let mut acc = if a.len() >= 2 {
        a.arg(1)
    } else {
        let mut found = None;
        while found.is_none() && k < len {
            let p = key(k);
            if agent.has_property(o, &p)? {
                found = Some(agent.get(o, &p, Value::Object(o))?);
            }
            k += 1.0;
        }
        match found {
            Some(v) => v,
            None => return agent.type_error("Reduce of empty array with no initial value"),
        }
    };
    while k < len {
        let p = key(k);
        if agent.has_property(o, &p)? {
            let v = agent.get(o, &p, Value::Object(o))?;
            acc = agent.call(
                &cb,
                Value::Undefined,
                &[acc, v, Value::Number(k), Value::Object(o)],
            )?;
        }
        k += 1.0;
    }
    Ok(acc)
}

/// Array.prototype.shift (§23.1.3.27).
fn shift(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    if len == 0.0 {
        set_length(agent, o, 0.0)?;
        return Ok(Value::Undefined);
    }
    let first = agent.get(o, &key(0.0), Value::Object(o))?;
    let mut k = 1.0;
    while k < len {
        let from = key(k);
        let to = key(k - 1.0);
        if agent.has_property(o, &from)? {
            let v = agent.get(o, &from, Value::Object(o))?;
            agent.set_or_throw(o, to, v, true)?;
        } else {
            agent.delete_property_or_throw(o, &to)?;
        }
        k += 1.0;
    }
    agent.delete_property_or_throw(o, &key(len - 1.0))?;
    set_length(agent, o, len - 1.0)?;
    Ok(first)
}

/// Array.prototype.slice (§23.1.3.28).
fn slice(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    let k0 = relative(agent, &a.arg(0), len, 0.0)?;
    let fin = relative(agent, &a.arg(1), len, len)?;
    let count = (fin - k0).max(0.0);
    let arr = agent.array_species_create(o, count)?;
    let mut k = k0;
    let mut n = 0.0;
    while k < fin {
        let p = key(k);
        if agent.has_property(o, &p)? {
            let v = agent.get(o, &p, Value::Object(o))?;
            agent.create_data_property_or_throw(arr, key(n), v)?;
        }
        k += 1.0;
        n += 1.0;
    }
    set_length(agent, arr, n)?;
    Ok(Value::Object(arr))
}

/// Array.prototype.splice (§23.1.3.31).
fn splice(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    let start = relative(agent, &a.arg(0), len, 0.0)?;
    let items: Vec<Value> = a.args.iter().skip(2).cloned().collect();
    let delete_count = if a.is_empty() {
        0.0
    } else if a.len() == 1 {
        len - start
    } else {
        let dc = agent.to_integer_or_infinity(&a.arg(1))?;
        dc.max(0.0).min(len - start)
    };
    let item_count = items.len() as f64;
    if len + item_count - delete_count > 9007199254740991.0 {
        return agent.type_error("Array too long");
    }
    let arr = agent.array_species_create(o, delete_count)?;
    let mut k = 0.0;
    while k < delete_count {
        let from = key(start + k);
        if agent.has_property(o, &from)? {
            let v = agent.get(o, &from, Value::Object(o))?;
            agent.create_data_property_or_throw(arr, key(k), v)?;
        }
        k += 1.0;
    }
    set_length(agent, arr, delete_count)?;
    if item_count < delete_count {
        let mut k = start;
        while k < len - delete_count {
            let from = key(k + delete_count);
            let to = key(k + item_count);
            if agent.has_property(o, &from)? {
                let v = agent.get(o, &from, Value::Object(o))?;
                agent.set_or_throw(o, to, v, true)?;
            } else {
                agent.delete_property_or_throw(o, &to)?;
            }
            k += 1.0;
        }
        let mut k = len;
        while k > len - delete_count + item_count {
            agent.delete_property_or_throw(o, &key(k - 1.0))?;
            k -= 1.0;
        }
    } else if item_count > delete_count {
        let mut k = len - delete_count;
        while k > start {
            let from = key(k + delete_count - 1.0);
            let to = key(k + item_count - 1.0);
            if agent.has_property(o, &from)? {
                let v = agent.get(o, &from, Value::Object(o))?;
                agent.set_or_throw(o, to, v, true)?;
            } else {
                agent.delete_property_or_throw(o, &to)?;
            }
            k -= 1.0;
        }
    }
    for (i, v) in items.into_iter().enumerate() {
        agent.set_or_throw(o, key(start + i as f64), v, true)?;
    }
    set_length(agent, o, len - delete_count + item_count)?;
    Ok(Value::Object(arr))
}

/// Array.prototype.unshift (§23.1.3.34).
fn unshift(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    let n = a.len() as f64;
    if n > 0.0 {
        if len + n > 9007199254740991.0 {
            return agent.type_error("Array too long");
        }
        let mut k = len;
        while k > 0.0 {
            let from = key(k - 1.0);
            let to = key(k + n - 1.0);
            if agent.has_property(o, &from)? {
                let v = agent.get(o, &from, Value::Object(o))?;
                agent.set_or_throw(o, to, v, true)?;
            } else {
                agent.delete_property_or_throw(o, &to)?;
            }
            k -= 1.0;
        }
        for (j, v) in a.args.iter().enumerate() {
            agent.set_or_throw(o, key(j as f64), v.clone(), true)?;
        }
    }
    set_length(agent, o, len + n)?;
    Ok(Value::Number(len + n))
}
