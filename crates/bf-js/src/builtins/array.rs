//! Array (§23.1): el constructor, `isArray`, `of`, `from` y Array.prototype completo
//! (`Array.fromAsync` llega con Promise).

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
        ("copyWithin", 2, copy_within),
        ("entries", 0, entries),
        ("every", 1, every),
        ("fill", 1, fill),
        ("filter", 1, filter),
        ("find", 1, find),
        ("findIndex", 1, find_index),
        ("findLast", 1, find_last),
        ("findLastIndex", 1, find_last_index),
        ("flat", 0, flat),
        ("flatMap", 1, flat_map),
        ("forEach", 1, for_each),
        ("includes", 1, includes),
        ("indexOf", 1, index_of),
        ("join", 1, join),
        ("keys", 0, keys),
        ("lastIndexOf", 1, last_index_of),
        ("map", 1, map),
        ("pop", 0, pop),
        ("push", 1, push),
        ("reduce", 1, reduce),
        ("reduceRight", 1, reduce_right),
        ("reverse", 0, reverse),
        ("shift", 0, shift),
        ("slice", 2, slice),
        ("some", 1, some),
        ("sort", 1, sort),
        ("splice", 2, splice),
        ("toLocaleString", 0, to_locale_string),
        ("toReversed", 0, to_reversed),
        ("toSorted", 1, to_sorted),
        ("toSpliced", 2, to_spliced),
        ("toString", 0, to_string),
        ("unshift", 1, unshift),
        ("with", 2, with),
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

/// HasProperty + Get del índice `p` de `o`: directo si es un elemento de un Array denso (una
/// propiedad propia de datos: HasProperty es verdadero y Get la devuelve). E3.
fn present(agent: &mut Agent, o: Gc<Object>, p: &PropertyKey) -> JsResult<Option<Value>> {
    if let PropertyKey::Index(i) = p
        && let ObjectKind::Array(d) = &agent.heap[o].kind
        && let Some(Some(v)) = d.dense.get(*i as usize)
    {
        return Ok(Some(v.clone()));
    }
    if agent.has_property(o, p)? {
        return Ok(Some(agent.get(o, p, Value::Object(o))?));
    }
    Ok(None)
}

/// CreateDataPropertyOrThrow(A, p, v) (§7.3.7); si `p` es la posición siguiente al vector
/// denso de un Array extensible sin índices propios fuera de él, definirla es agregarla al
/// vector (E3). Una definición no mira la cadena de prototipos.
fn define_append(agent: &mut Agent, a: Gc<Object>, p: PropertyKey, v: Value) -> JsResult<()> {
    if let PropertyKey::Index(i) = p {
        let obj = &mut agent.heap[a];
        let plain = obj.extensible && obj.props.index_keys().next().is_none();
        if let ObjectKind::Array(d) = &mut obj.kind
            && plain
            && d.dense.len() == i as usize
            && (i < d.length || (d.length_writable && i < u32::MAX - 1))
        {
            d.dense.push(Some(v));
            d.length = d.length.max(i + 1);
            return Ok(());
        }
    }
    agent.create_data_property_or_throw(a, p, v)?;
    Ok(())
}

/// ¿Agregar elementos al final de `o` con `[[Set]]` equivale a extender su vector denso? Un
/// Array denso, extensible, con `length` escribible y sin índices propios fuera del vector, y
/// una cadena de prototipos sin propiedades indexadas (ningún setter ni propiedad de solo
/// lectura para esos índices). E3.
fn plain_append(agent: &Agent, o: Gc<Object>, n: usize) -> bool {
    let obj = &agent.heap[o];
    let ObjectKind::Array(d) = &obj.kind else {
        return false;
    };
    if !obj.extensible
        || !d.length_writable
        || d.dense.len() != d.length as usize
        || (d.length as u64) + (n as u64) >= u64::from(u32::MAX)
        || obj.props.index_keys().next().is_some()
    {
        return false;
    }
    let mut p = obj.proto;
    while let Some(x) = p {
        let px = &agent.heap[x];
        match &px.kind {
            ObjectKind::Ordinary => {}
            ObjectKind::Array(d) if d.dense.iter().all(Option::is_none) => {}
            _ => return false,
        }
        if px.props.index_keys().next().is_some() {
            return false;
        }
        p = px.proto;
    }
    true
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
        if let Some(v) = present(agent, o, &p)? {
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
        if let Some(v) = present(agent, o, &p)? {
            let r = agent.call(
                &cb,
                this_arg.clone(),
                &[v.clone(), Value::Number(k), Value::Object(o)],
            )?;
            if crate::abstract_ops::conversion::to_boolean(agent, &r) {
                define_append(agent, arr, key(to), v)?;
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
        if let Some(v) = present(agent, o, &p)? {
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
        if let Some(v) = present(agent, o, &p)? {
            let r = agent.call(
                &cb,
                this_arg.clone(),
                &[v, Value::Number(k), Value::Object(o)],
            )?;
            define_append(agent, arr, p, r)?;
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
    // Ruta rápida (E3): extender el vector denso (el mismo resultado que los [[Set]]).
    if let Value::Object(o) = a.this
        && plain_append(agent, o, a.len())
        && let ObjectKind::Array(d) = &mut agent.heap[o].kind
    {
        d.dense.extend(a.args.iter().cloned().map(Some));
        d.length += a.len() as u32;
        return Ok(Value::Number(f64::from(d.length)));
    }
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
            if let Some(v) = present(agent, o, &p)? {
                found = Some(v);
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
        if let Some(v) = present(agent, o, &p)? {
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
        if let Some(v) = present(agent, o, &p)? {
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

// --- Más de Array.prototype ---

/// Array.prototype.copyWithin (§23.1.3.4).
fn copy_within(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    let mut to = relative(agent, &a.arg(0), len, 0.0)?;
    let mut from = relative(agent, &a.arg(1), len, 0.0)?;
    let fin = relative(agent, &a.arg(2), len, len)?;
    let mut count = (fin - from).min(len - to);
    let dir = if from < to && to < from + count {
        from += count - 1.0;
        to += count - 1.0;
        -1.0
    } else {
        1.0
    };
    while count > 0.0 {
        let fk = key(from);
        let tk = key(to);
        if agent.has_property(o, &fk)? {
            let v = agent.get(o, &fk, Value::Object(o))?;
            agent.set_or_throw(o, tk, v, true)?;
        } else {
            agent.delete_property_or_throw(o, &tk)?;
        }
        from += dir;
        to += dir;
        count -= 1.0;
    }
    Ok(Value::Object(o))
}

/// Array.prototype.fill (§23.1.3.7).
fn fill(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    let k0 = relative(agent, &a.arg(1), len, 0.0)?;
    let fin = relative(agent, &a.arg(2), len, len)?;
    let mut k = k0;
    while k < fin {
        agent.set_or_throw(o, key(k), a.arg(0), true)?;
        k += 1.0;
    }
    Ok(Value::Object(o))
}

/// FindViaPredicate (§23.1.3.12.1) desde el final.
fn find_last_impl(agent: &mut Agent, a: &CallArgs<'_>, index: bool) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    let cb = callback(agent, a, if index { "findLastIndex" } else { "findLast" })?;
    let this_arg = a.arg(1);
    let mut k = len - 1.0;
    while k >= 0.0 {
        let v = agent.get(o, &key(k), Value::Object(o))?;
        let r = agent.call(
            &cb,
            this_arg.clone(),
            &[v.clone(), Value::Number(k), Value::Object(o)],
        )?;
        if crate::abstract_ops::conversion::to_boolean(agent, &r) {
            return Ok(if index { Value::Number(k) } else { v });
        }
        k -= 1.0;
    }
    Ok(if index {
        Value::Number(-1.0)
    } else {
        Value::Undefined
    })
}

fn find_last(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    find_last_impl(agent, a, false)
}

fn find_last_index(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    find_last_impl(agent, a, true)
}

/// FlattenIntoArray (§23.1.3.13.1).
fn flatten_into_array(
    agent: &mut Agent,
    target: Gc<Object>,
    source: Gc<Object>,
    source_len: f64,
    start: f64,
    depth: f64,
    mapper: Option<(&Value, &Value)>,
) -> JsResult<f64> {
    let mut target_index = start;
    let mut si = 0.0;
    while si < source_len {
        let p = key(si);
        if agent.has_property(source, &p)? {
            let mut element = agent.get(source, &p, Value::Object(source))?;
            if let Some((f, this_arg)) = mapper {
                element = agent.call(
                    f,
                    this_arg.clone(),
                    &[element, Value::Number(si), Value::Object(source)],
                )?;
            }
            let flatten = depth > 0.0 && agent.is_array(&element)?;
            if flatten {
                if agent.stack_exhausted() {
                    return agent.range_error("Maximum call stack size exceeded");
                }
                let eo = element.as_object().expect("array");
                let el = agent.length_of_array_like(eo)?;
                target_index =
                    flatten_into_array(agent, target, eo, el, target_index, depth - 1.0, None)?;
            } else {
                if target_index >= 9007199254740991.0 {
                    return agent.type_error("Array too long");
                }
                agent.create_data_property_or_throw(target, key(target_index), element)?;
                target_index += 1.0;
            }
        }
        si += 1.0;
    }
    Ok(target_index)
}

/// Array.prototype.flat (§23.1.3.13).
fn flat(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    let depth = if a.arg(0).is_undefined() {
        1.0
    } else {
        let d = agent.to_integer_or_infinity(&a.arg(0))?;
        if d < 0.0 { 0.0 } else { d }
    };
    let arr = agent.array_species_create(o, 0.0)?;
    flatten_into_array(agent, arr, o, len, 0.0, depth, None)?;
    Ok(Value::Object(arr))
}

/// Array.prototype.flatMap (§23.1.3.14).
fn flat_map(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    let f = callback(agent, a, "flatMap")?;
    let arr = agent.array_species_create(o, 0.0)?;
    let this_arg = a.arg(1);
    flatten_into_array(agent, arr, o, len, 0.0, 1.0, Some((&f, &this_arg)))?;
    Ok(Value::Object(arr))
}

/// Array.prototype.lastIndexOf (§23.1.3.20).
fn last_index_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    if len == 0.0 {
        return Ok(Value::Number(-1.0));
    }
    let n = if a.len() > 1 {
        agent.to_integer_or_infinity(&a.arg(1))?
    } else {
        len - 1.0
    };
    if n == f64::NEG_INFINITY {
        return Ok(Value::Number(-1.0));
    }
    let mut k = if n >= 0.0 { n.min(len - 1.0) } else { len + n };
    let target = a.arg(0);
    while k >= 0.0 {
        let p = key(k);
        if let Some(v) = present(agent, o, &p)? {
            if agent.is_strictly_equal(&v, &target) {
                return Ok(Value::Number(k));
            }
        }
        k -= 1.0;
    }
    Ok(Value::Number(-1.0))
}

/// Array.prototype.reduceRight (§23.1.3.25).
fn reduce_right(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    let cb = callback(agent, a, "reduceRight")?;
    if len == 0.0 && a.len() < 2 {
        return agent.type_error("Reduce of empty array with no initial value");
    }
    let mut k = len - 1.0;
    let mut acc = if a.len() >= 2 {
        a.arg(1)
    } else {
        let mut found = None;
        while found.is_none() && k >= 0.0 {
            let p = key(k);
            if let Some(v) = present(agent, o, &p)? {
                found = Some(v);
            }
            k -= 1.0;
        }
        match found {
            Some(v) => v,
            None => return agent.type_error("Reduce of empty array with no initial value"),
        }
    };
    while k >= 0.0 {
        let p = key(k);
        if let Some(v) = present(agent, o, &p)? {
            acc = agent.call(
                &cb,
                Value::Undefined,
                &[acc, v, Value::Number(k), Value::Object(o)],
            )?;
        }
        k -= 1.0;
    }
    Ok(acc)
}

/// Array.prototype.reverse (§23.1.3.26).
fn reverse(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    let middle = (len / 2.0).floor();
    let mut lower = 0.0;
    while lower != middle {
        let upper = len - lower - 1.0;
        let (lp, up) = (key(lower), key(upper));
        let lower_exists = agent.has_property(o, &lp)?;
        let lower_value = if lower_exists {
            agent.get(o, &lp, Value::Object(o))?
        } else {
            Value::Undefined
        };
        let upper_exists = agent.has_property(o, &up)?;
        let upper_value = if upper_exists {
            agent.get(o, &up, Value::Object(o))?
        } else {
            Value::Undefined
        };
        match (lower_exists, upper_exists) {
            (true, true) => {
                agent.set_or_throw(o, lp, upper_value, true)?;
                agent.set_or_throw(o, up, lower_value, true)?;
            }
            (false, true) => {
                agent.set_or_throw(o, lp, upper_value, true)?;
                agent.delete_property_or_throw(o, &up)?;
            }
            (true, false) => {
                agent.delete_property_or_throw(o, &lp)?;
                agent.set_or_throw(o, up, lower_value, true)?;
            }
            (false, false) => {}
        }
        lower += 1.0;
    }
    Ok(Value::Object(o))
}

/// CompareArrayElements (§23.1.3.30.2).
fn compare_elements(
    agent: &mut Agent,
    x: &Value,
    y: &Value,
    cmp: &Value,
) -> JsResult<std::cmp::Ordering> {
    use std::cmp::Ordering;
    match (x.is_undefined(), y.is_undefined()) {
        (true, true) => return Ok(Ordering::Equal),
        (true, false) => return Ok(Ordering::Greater),
        (false, true) => return Ok(Ordering::Less),
        _ => {}
    }
    if !cmp.is_undefined() {
        let r = agent.call(cmp, Value::Undefined, &[x.clone(), y.clone()])?;
        let v = agent.to_number(&r)?;
        return Ok(if v < 0.0 {
            Ordering::Less
        } else if v > 0.0 {
            Ordering::Greater
        } else {
            Ordering::Equal
        });
    }
    let xs = agent.to_string(x)?;
    let ys = agent.to_string(y)?;
    Ok(xs.units().cmp(ys.units()))
}

/// Merge sort estable con un comparador que puede lanzar.
fn merge_sort(agent: &mut Agent, items: Vec<Value>, cmp: &Value) -> JsResult<Vec<Value>> {
    if items.len() <= 1 {
        return Ok(items);
    }
    let mut items = items;
    let right = items.split_off(items.len() / 2);
    let left = merge_sort(agent, items, cmp)?;
    let right = merge_sort(agent, right, cmp)?;
    let mut out = Vec::with_capacity(left.len() + right.len());
    let (mut i, mut j) = (0, 0);
    while i < left.len() && j < right.len() {
        if compare_elements(agent, &right[j], &left[i], cmp)? == std::cmp::Ordering::Less {
            out.push(right[j].clone());
            j += 1;
        } else {
            out.push(left[i].clone());
            i += 1;
        }
    }
    out.extend_from_slice(&left[i..]);
    out.extend_from_slice(&right[j..]);
    Ok(out)
}

/// SortIndexedProperties (§23.1.3.30.1): `skip_holes` omite los índices ausentes.
fn sort_indexed_properties(
    agent: &mut Agent,
    o: Gc<Object>,
    len: f64,
    cmp: &Value,
    skip_holes: bool,
) -> JsResult<Vec<Value>> {
    let mut items = Vec::new();
    let mut k = 0.0;
    while k < len {
        let p = key(k);
        if !skip_holes || agent.has_property(o, &p)? {
            items.push(agent.get(o, &p, Value::Object(o))?);
        }
        k += 1.0;
    }
    merge_sort(agent, items, cmp)
}

fn check_comparefn(agent: &mut Agent, cmp: &Value) -> JsResult<()> {
    if !cmp.is_undefined() && !agent.is_callable(cmp) {
        return agent.type_error("The comparison function must be either a function or undefined");
    }
    Ok(())
}

/// Array.prototype.sort (§23.1.3.30).
fn sort(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let cmp = a.arg(0);
    check_comparefn(agent, &cmp)?;
    let (o, len) = this_obj(agent, a)?;
    let sorted = sort_indexed_properties(agent, o, len, &cmp, true)?;
    let n = sorted.len();
    for (j, v) in sorted.into_iter().enumerate() {
        agent.set_or_throw(o, key(j as f64), v, true)?;
    }
    let mut j = n as f64;
    while j < len {
        agent.delete_property_or_throw(o, &key(j))?;
        j += 1.0;
    }
    Ok(Value::Object(o))
}

/// Array.prototype.toLocaleString (§23.1.3.32).
fn to_locale_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    let mut out: Vec<u16> = Vec::new();
    let mut k = 0.0;
    while k < len {
        if k > 0.0 {
            out.push(u16::from(b','));
        }
        let e = agent.get(o, &key(k), Value::Object(o))?;
        if !e.is_nullish() {
            // E2: ECMA-402 §19.4.1 pasa `locales` y `options`.
            let r = agent.invoke(
                &e,
                &PropertyKey::from("toLocaleString"),
                &[a.arg(0), a.arg(1)],
            )?;
            let s = agent.to_string(&r)?;
            out.extend_from_slice(s.units());
        }
        k += 1.0;
    }
    Ok(Value::String(crate::string::JsString::from_units(out)))
}

fn new_array_checked(agent: &mut Agent, len: f64) -> JsResult<Gc<Object>> {
    if len > 4294967295.0 {
        return agent.range_error("Invalid array length");
    }
    agent.array_create(len as u64, None)
}

/// Array.prototype.toReversed (§23.1.3.33).
fn to_reversed(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    let arr = new_array_checked(agent, len)?;
    let mut k = 0.0;
    while k < len {
        let v = agent.get(o, &key(len - k - 1.0), Value::Object(o))?;
        agent.create_data_property_or_throw(arr, key(k), v)?;
        k += 1.0;
    }
    Ok(Value::Object(arr))
}

/// Array.prototype.toSorted (§23.1.3.34).
fn to_sorted(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let cmp = a.arg(0);
    check_comparefn(agent, &cmp)?;
    let (o, len) = this_obj(agent, a)?;
    let arr = new_array_checked(agent, len)?;
    let sorted = sort_indexed_properties(agent, o, len, &cmp, false)?;
    for (j, v) in sorted.into_iter().enumerate() {
        agent.create_data_property_or_throw(arr, key(j as f64), v)?;
    }
    Ok(Value::Object(arr))
}

/// Array.prototype.toSpliced (§23.1.3.35).
fn to_spliced(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    let start = relative(agent, &a.arg(0), len, 0.0)?;
    let items: Vec<Value> = a.args.iter().skip(2).cloned().collect();
    let skip = if a.is_empty() {
        0.0
    } else if a.len() == 1 {
        len - start
    } else {
        let dc = agent.to_integer_or_infinity(&a.arg(1))?;
        dc.max(0.0).min(len - start)
    };
    let new_len = len + items.len() as f64 - skip;
    if new_len > 9007199254740991.0 {
        return agent.type_error("Array too long");
    }
    let arr = new_array_checked(agent, new_len)?;
    let mut i = 0.0;
    let mut r = start + skip;
    while i < start {
        let v = agent.get(o, &key(i), Value::Object(o))?;
        agent.create_data_property_or_throw(arr, key(i), v)?;
        i += 1.0;
    }
    for e in items {
        agent.create_data_property_or_throw(arr, key(i), e)?;
        i += 1.0;
    }
    while i < new_len {
        let v = agent.get(o, &key(r), Value::Object(o))?;
        agent.create_data_property_or_throw(arr, key(i), v)?;
        i += 1.0;
        r += 1.0;
    }
    Ok(Value::Object(arr))
}

/// Array.prototype.toString (§23.1.3.36).
fn to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = agent.to_object(&a.this)?;
    let f = agent.get_str(o, "join")?;
    if agent.is_callable(&f) {
        return agent.call(&f, Value::Object(o), &[]);
    }
    crate::builtins::object::to_string(
        agent,
        &CallArgs {
            this: Value::Object(o),
            args: &[],
            new_target: None,
            callee: a.callee,
        },
    )
}

/// Array.prototype.with (§23.1.3.39).
fn with(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (o, len) = this_obj(agent, a)?;
    let rel = agent.to_integer_or_infinity(&a.arg(0))?;
    let actual = if rel >= 0.0 { rel } else { len + rel };
    if actual >= len || actual < 0.0 {
        return agent.range_error("Invalid index");
    }
    let arr = new_array_checked(agent, len)?;
    let mut k = 0.0;
    while k < len {
        let v = if k == actual {
            a.arg(1)
        } else {
            agent.get(o, &key(k), Value::Object(o))?
        };
        agent.create_data_property_or_throw(arr, key(k), v)?;
        k += 1.0;
    }
    Ok(Value::Object(arr))
}
