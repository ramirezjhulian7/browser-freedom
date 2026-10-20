//! Map (§24.1) y Set (§24.2), sus iteradores (§24.1.5, §24.2.6), los métodos de conjuntos
//! de ES2025 (§24.2.4, con GetSetRecord §24.2.1.2) y `Map.groupBy` (§24.1.2.1).
//!
//! `[[MapData]]`/`[[SetData]]` es la lista de entradas del spec con sus huecos: borrar o
//! limpiar deja la posición vacía, así que un iterador que recorre la lista por índice ve
//! exactamente lo que pide el spec cuando la colección cambia durante la iteración. Cuando
//! los huecos superan a las entradas la lista se compacta y los iteradores vivos (que la
//! colección conoce sin retenerlos) se reubican; no se compacta mientras un bucle nativo
//! (`forEach`, los métodos de conjuntos) recorre la lista por índice. Un índice hash
//! (SameValueZero, con −0 normalizado) evita las búsquedas lineales.

use std::collections::HashMap;

use crate::bigint::BigInt;
use crate::builtins::{FnEntry, accessor, constructor, methods, to_string_tag};
use crate::heap::{Gc, Trace, Tracer};
use crate::object::{IterKind, Object, ObjectKind};
use crate::string::JsString;
use crate::value::{PropertyKey, Symbol, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, ErrorType, JsResult, RealmId, Throw};

/// La clave de hash de un valor según SameValueZero.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MapKey {
    Undefined,
    Null,
    Bool(bool),
    Number(u64),
    String(JsString),
    Symbol(Gc<Symbol>),
    BigInt(bool, Vec<u32>),
    Object(Gc<Object>),
}

impl MapKey {
    pub fn of(agent: &Agent, v: &Value) -> MapKey {
        match v {
            Value::Undefined => MapKey::Undefined,
            Value::Null => MapKey::Null,
            Value::Boolean(b) => MapKey::Bool(*b),
            Value::Number(n) => {
                let n = if *n == 0.0 {
                    0.0
                } else if n.is_nan() {
                    f64::NAN
                } else {
                    *n
                };
                MapKey::Number(n.to_bits())
            }
            Value::String(s) => MapKey::String(s.clone()),
            Value::Symbol(s) => MapKey::Symbol(*s),
            Value::BigInt(b) => {
                let b: &BigInt = &agent.heap[*b];
                MapKey::BigInt(b.negative, b.mag.clone())
            }
            Value::Object(o) => MapKey::Object(*o),
        }
    }
}

/// CanonicalizeKeyedCollectionKey (§24.5.1): −0 pasa a +0.
pub fn canonicalize(v: Value) -> Value {
    match v {
        Value::Number(n) => Value::Number(if n == 0.0 { 0.0 } else { n }),
        other => other,
    }
}

/// `[[MapData]]` o `[[SetData]]` (en un Set, el valor de cada entrada es `undefined`).
#[derive(Debug, Default)]
pub struct MapData {
    pub entries: Vec<Option<(Value, Value)>>,
    index: HashMap<MapKey, usize>,
    pub size: usize,
    /// Los Map/Set Iterator creados sobre esta colección (referencias no trazadas: se
    /// verifican con `Heap::is_alive`).
    iterators: Vec<Gc<Object>>,
    /// Bucles nativos recorriendo la lista por índice (impiden compactar).
    busy: u32,
}

impl MapData {
    pub fn find(&self, key: &MapKey) -> Option<usize> {
        self.index.get(key).copied()
    }

    /// Agrega o reemplaza.
    pub fn set(&mut self, k: MapKey, key: Value, value: Value) {
        if let Some(&i) = self.index.get(&k)
            && let Some(Some(e)) = self.entries.get_mut(i)
        {
            e.1 = value;
            return;
        }
        self.index.insert(k, self.entries.len());
        self.entries.push(Some((key, value)));
        self.size += 1;
    }

    pub fn remove(&mut self, k: &MapKey) -> bool {
        match self.index.remove(k) {
            Some(i) => {
                self.entries[i] = None;
                self.size -= 1;
                true
            }
            None => false,
        }
    }

    pub fn clear(&mut self) {
        self.entries.fill(None);
        self.index.clear();
        self.size = 0;
    }

    /// ¿Conviene compactar? (Más huecos que entradas, y no pocos.)
    fn should_compact(&self) -> bool {
        let holes = self.entries.len() - self.size;
        self.busy == 0 && holes > 32 && holes > self.size
    }

    /// Quita los huecos; devuelve, para cada posición vieja (y el final), la nueva.
    fn compact(&mut self) -> Vec<usize> {
        let mut remap = Vec::with_capacity(self.entries.len() + 1);
        let mut live = 0;
        for e in &self.entries {
            remap.push(live);
            if e.is_some() {
                live += 1;
            }
        }
        remap.push(live);
        self.entries.retain(Option::is_some);
        for i in self.index.values_mut() {
            *i = remap[*i];
        }
        remap
    }

    /// Una lista nueva con las claves presentes.
    fn from_keys(agent: &Agent, keys: impl IntoIterator<Item = Value>) -> MapData {
        let mut d = MapData::default();
        for k in keys {
            let mk = MapKey::of(agent, &k);
            if d.find(&mk).is_none() {
                d.set(mk, k, Value::Undefined);
            }
        }
        d
    }
}

impl Trace for MapData {
    fn trace(&self, t: &mut Tracer) {
        for (k, v) in self.entries.iter().flatten() {
            t.value(k);
            t.value(v);
        }
    }
}

/// El estado de un Map/Set Iterator: la colección (`None` al terminar), la posición y la
/// clase.
#[derive(Debug)]
pub struct CollectionIterator {
    pub target: Option<Gc<Object>>,
    pub index: usize,
    pub kind: IterKind,
}

impl Trace for CollectionIterator {
    fn trace(&self, t: &mut Tracer) {
        t.opt(self.target);
    }
}

fn map_data(agent: &Agent, o: Gc<Object>) -> Option<&MapData> {
    match &agent.heap[o].kind {
        ObjectKind::Map(d) | ObjectKind::Set(d) => Some(d),
        _ => None,
    }
}

fn map_data_mut(agent: &mut Agent, o: Gc<Object>) -> Option<&mut MapData> {
    match &mut agent.heap[o].kind {
        ObjectKind::Map(d) | ObjectKind::Set(d) => Some(d),
        _ => None,
    }
}

/// Compacta la colección `o` si conviene, reubicando sus iteradores vivos.
fn maybe_compact(agent: &mut Agent, o: Gc<Object>) {
    let Some(d) = map_data_mut(agent, o) else {
        return;
    };
    if !d.should_compact() {
        return;
    }
    let remap = d.compact();
    let iterators = std::mem::take(&mut d.iterators);
    let mut alive = Vec::new();
    for it in iterators {
        if !agent.heap.is_alive(it) {
            continue;
        }
        if let ObjectKind::MapIterator(c) | ObjectKind::SetIterator(c) = &mut agent.heap[it].kind
            && c.target == Some(o)
        {
            c.index = remap[c.index.min(remap.len() - 1)];
            alive.push(it);
        }
    }
    if let Some(d) = map_data_mut(agent, o) {
        d.iterators = alive;
    }
}

/// Corre `f` (un bucle nativo por índice sobre la lista de `o`) sin permitir compactarla.
fn without_compaction<T>(
    agent: &mut Agent,
    o: Gc<Object>,
    f: impl FnOnce(&mut Agent) -> JsResult<T>,
) -> JsResult<T> {
    if let Some(d) = map_data_mut(agent, o) {
        d.busy += 1;
    }
    let r = f(agent);
    if let Some(d) = map_data_mut(agent, o) {
        d.busy -= 1;
    }
    maybe_compact(agent, o);
    r
}

/// RequireInternalSlot(M, [[MapData]]).
fn this_map(agent: &mut Agent, this: &Value, method: &str) -> JsResult<Gc<Object>> {
    match this {
        Value::Object(o) if matches!(agent.heap[*o].kind, ObjectKind::Map(_)) => Ok(*o),
        _ => agent.type_error(&format!(
            "Method Map.prototype.{method} called on incompatible receiver"
        )),
    }
}

/// RequireInternalSlot(S, [[SetData]]).
fn this_set(agent: &mut Agent, this: &Value, method: &str) -> JsResult<Gc<Object>> {
    match this {
        Value::Object(o) if matches!(agent.heap[*o].kind, ObjectKind::Set(_)) => Ok(*o),
        _ => agent.type_error(&format!(
            "Method Set.prototype.{method} called on incompatible receiver"
        )),
    }
}

pub fn init(agent: &mut Agent, realm: RealmId) {
    let i = agent.realms[realm].intrinsics.clone();
    let species = PropertyKey::Symbol(agent.well_known.species);
    let iter_sym = agent.well_known.iterator;
    // --- Map ---
    let mp = agent.new_object(Some(i.object_prototype));
    const MAP_PROTO: &[FnEntry] = &[
        ("clear", 0, map_clear),
        ("delete", 1, map_delete),
        ("forEach", 1, map_for_each),
        ("get", 1, map_get),
        ("getOrInsert", 2, map_get_or_insert),
        ("getOrInsertComputed", 2, map_get_or_insert_computed),
        ("has", 1, map_has),
        ("keys", 0, map_keys),
        ("set", 2, map_set),
        ("values", 0, map_values),
    ];
    methods(agent, mp, MAP_PROTO);
    let entries = crate::builtins::method(agent, mp, "entries", 0, map_entries);
    agent.put_data(
        mp,
        PropertyKey::Symbol(iter_sym),
        Value::Object(entries),
        true,
        false,
        true,
    );
    accessor(
        agent,
        mp,
        PropertyKey::from("size"),
        "size",
        Some(map_size),
        None,
    );
    to_string_tag(agent, mp, "Map");
    let mc = constructor(agent, "Map", 0, map_ctor, mp);
    crate::builtins::method(agent, mc, "groupBy", 2, map_group_by);
    accessor(
        agent,
        mc,
        species.clone(),
        "[Symbol.species]",
        Some(species_getter),
        None,
    );
    // %MapIteratorPrototype% (§24.1.5.2).
    let mip = agent.new_object(Some(i.iterator_prototype));
    crate::builtins::method(agent, mip, "next", 0, map_iterator_next);
    to_string_tag(agent, mip, "Map Iterator");
    // --- Set ---
    let sp = agent.new_object(Some(i.object_prototype));
    const SET_PROTO: &[FnEntry] = &[
        ("add", 1, set_add),
        ("clear", 0, set_clear),
        ("delete", 1, set_delete),
        ("difference", 1, set_difference),
        ("entries", 0, set_entries),
        ("forEach", 1, set_for_each),
        ("has", 1, set_has),
        ("intersection", 1, set_intersection),
        ("isDisjointFrom", 1, set_is_disjoint_from),
        ("isSubsetOf", 1, set_is_subset_of),
        ("isSupersetOf", 1, set_is_superset_of),
        ("symmetricDifference", 1, set_symmetric_difference),
        ("union", 1, set_union),
    ];
    methods(agent, sp, SET_PROTO);
    let values = crate::builtins::method(agent, sp, "values", 0, set_values);
    agent.put_data(
        sp,
        PropertyKey::from("keys"),
        Value::Object(values),
        true,
        false,
        true,
    );
    agent.put_data(
        sp,
        PropertyKey::Symbol(iter_sym),
        Value::Object(values),
        true,
        false,
        true,
    );
    accessor(
        agent,
        sp,
        PropertyKey::from("size"),
        "size",
        Some(set_size),
        None,
    );
    to_string_tag(agent, sp, "Set");
    let sc = constructor(agent, "Set", 0, set_ctor, sp);
    accessor(
        agent,
        sc,
        species,
        "[Symbol.species]",
        Some(species_getter),
        None,
    );
    let sip = agent.new_object(Some(i.iterator_prototype));
    crate::builtins::method(agent, sip, "next", 0, set_iterator_next);
    to_string_tag(agent, sip, "Set Iterator");
    let ints = &mut agent.realms[realm].intrinsics;
    ints.map_ctor = mc;
    ints.map_prototype = mp;
    ints.map_iterator_prototype = mip;
    ints.set_ctor = sc;
    ints.set_prototype = sp;
    ints.set_iterator_prototype = sip;
}

fn species_getter(_: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(a.this.clone())
}

/// IfAbruptCloseIterator (§7.4.12) para un error dentro del bucle.
fn close_on_error<T>(agent: &mut Agent, it: &Value, e: Throw) -> JsResult<T> {
    if let Throw::Value(_) = e {
        agent.iterator_close_on_throw(it);
    }
    Err(e)
}

/// AddEntriesFromIterable (§24.1.1.2), también para WeakMap.
pub fn add_entries_from_iterable(
    agent: &mut Agent,
    target: Gc<Object>,
    iterable: &Value,
    adder: &Value,
) -> JsResult<()> {
    let (it, next) = agent.get_iterator(iterable)?;
    loop {
        let Some(entry) = agent.iterator_step_value(&it, &next)? else {
            return Ok(());
        };
        let Value::Object(eo) = entry else {
            let e = agent.error(
                ErrorType::TypeError,
                "Iterator value is not an entry object",
            );
            return close_on_error(agent, &it, e);
        };
        let k = match agent.get(eo, &PropertyKey::Index(0), Value::Object(eo)) {
            Ok(k) => k,
            Err(e) => return close_on_error(agent, &it, e),
        };
        let v = match agent.get(eo, &PropertyKey::Index(1), Value::Object(eo)) {
            Ok(v) => v,
            Err(e) => return close_on_error(agent, &it, e),
        };
        if let Err(e) = agent.call(adder, Value::Object(target), &[k, v]) {
            return close_on_error(agent, &it, e);
        }
    }
}

/// Agrega cada valor de `iterable` llamando a `adder` (Set y WeakSet).
pub fn add_values_from_iterable(
    agent: &mut Agent,
    target: Gc<Object>,
    iterable: &Value,
    adder: &Value,
) -> JsResult<()> {
    let (it, next) = agent.get_iterator(iterable)?;
    loop {
        let Some(v) = agent.iterator_step_value(&it, &next)? else {
            return Ok(());
        };
        if let Err(e) = agent.call(adder, Value::Object(target), &[v]) {
            return close_on_error(agent, &it, e);
        }
    }
}

// --- Map ---

/// Map ( [ iterable ] ) (§24.1.1.1).
fn map_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor Map requires 'new'");
    };
    let map = agent.ordinary_create_from_constructor(
        nt,
        |i| i.map_prototype,
        ObjectKind::Map(Box::default()),
    )?;
    let iterable = a.arg(0);
    if iterable.is_nullish() {
        return Ok(Value::Object(map));
    }
    let adder = agent.get(map, &PropertyKey::from("set"), Value::Object(map))?;
    if !agent.is_callable(&adder) {
        return agent.type_error("Map.prototype.set is not a function");
    }
    add_entries_from_iterable(agent, map, &iterable, &adder)?;
    Ok(Value::Object(map))
}

/// Map.groupBy (§24.1.2.1).
fn map_group_by(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let items = a.arg(0);
    let callback = a.arg(1);
    agent.require_object_coercible(&items)?;
    if !agent.is_callable(&callback) {
        return agent.type_error("callback is not a function");
    }
    // GroupBy (§7.3.35) con keyCoercion zero.
    let (it, next) = agent.get_iterator(&items)?;
    let mut groups: Vec<(Value, Vec<Value>)> = Vec::new();
    let mut index: HashMap<MapKey, usize> = HashMap::new();
    let mut k = 0f64;
    loop {
        if k >= 9007199254740991.0 {
            let e = agent.error(ErrorType::TypeError, "Too many elements");
            return close_on_error(agent, &it, e);
        }
        let Some(value) = agent.iterator_step_value(&it, &next)? else {
            break;
        };
        let key = match agent.call(
            &callback,
            Value::Undefined,
            &[value.clone(), Value::Number(k)],
        ) {
            Ok(key) => canonicalize(key),
            Err(e) => return close_on_error(agent, &it, e),
        };
        let mk = MapKey::of(agent, &key);
        match index.get(&mk) {
            Some(&g) => groups[g].1.push(value),
            None => {
                index.insert(mk, groups.len());
                groups.push((key, vec![value]));
            }
        }
        k += 1.0;
    }
    let proto = agent.intrinsics().map_prototype;
    let mut data = MapData::default();
    for (key, elems) in groups {
        let arr = agent.create_array_from_list(&elems)?;
        let mk = MapKey::of(agent, &key);
        data.set(mk, key, Value::Object(arr));
    }
    Ok(Value::Object(agent.alloc(Object::new(
        Some(proto),
        ObjectKind::Map(Box::new(data)),
    ))))
}

fn map_clear(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let m = this_map(agent, &a.this, "clear")?;
    if let Some(d) = map_data_mut(agent, m) {
        d.clear();
    }
    maybe_compact(agent, m);
    Ok(Value::Undefined)
}

fn map_delete(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let m = this_map(agent, &a.this, "delete")?;
    let k = MapKey::of(agent, &a.arg(0));
    let r = map_data_mut(agent, m).is_some_and(|d| d.remove(&k));
    maybe_compact(agent, m);
    Ok(Value::Boolean(r))
}

/// Map.prototype.forEach (§24.1.3.5) y Set.prototype.forEach (§24.2.4.8).
fn for_each(agent: &mut Agent, o: Gc<Object>, a: &CallArgs<'_>, is_set: bool) -> JsResult<Value> {
    let callback = a.arg(0);
    if !agent.is_callable(&callback) {
        return agent.type_error(&format!("{} is not a function", agent.describe(&callback)));
    }
    let this_arg = a.arg(1);
    without_compaction(agent, o, |agent| {
        let mut index = 0;
        loop {
            let e = match map_data(agent, o) {
                Some(d) if index < d.entries.len() => d.entries[index].clone(),
                _ => break,
            };
            index += 1;
            if let Some((k, v)) = e {
                let args = if is_set {
                    [k.clone(), k, Value::Object(o)]
                } else {
                    [v, k, Value::Object(o)]
                };
                agent.call(&callback, this_arg.clone(), &args)?;
            }
        }
        Ok(Value::Undefined)
    })
}

fn map_for_each(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let m = this_map(agent, &a.this, "forEach")?;
    for_each(agent, m, a, false)
}

fn map_get(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let m = this_map(agent, &a.this, "get")?;
    let k = MapKey::of(agent, &a.arg(0));
    let d = map_data(agent, m).expect("map");
    Ok(d.find(&k)
        .and_then(|i| d.entries[i].as_ref().map(|e| e.1.clone()))
        .unwrap_or_default())
}

/// Map.prototype.getOrInsert (propuesta upsert, `features: [upsert]`).
fn map_get_or_insert(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let m = this_map(agent, &a.this, "getOrInsert")?;
    let key = canonicalize(a.arg(0));
    let k = MapKey::of(agent, &key);
    let d = map_data_mut(agent, m).expect("map");
    if let Some(i) = d.find(&k)
        && let Some(e) = &d.entries[i]
    {
        return Ok(e.1.clone());
    }
    d.set(k, key, a.arg(1));
    Ok(a.arg(1))
}

/// Map.prototype.getOrInsertComputed (propuesta upsert).
fn map_get_or_insert_computed(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let m = this_map(agent, &a.this, "getOrInsertComputed")?;
    let callback = a.arg(1);
    if !agent.is_callable(&callback) {
        return agent.type_error("callbackfn is not a function");
    }
    let key = canonicalize(a.arg(0));
    let k = MapKey::of(agent, &key);
    {
        let d = map_data(agent, m).expect("map");
        if let Some(i) = d.find(&k)
            && let Some(e) = &d.entries[i]
        {
            return Ok(e.1.clone());
        }
    }
    let value = agent.call(&callback, Value::Undefined, std::slice::from_ref(&key))?;
    let k = MapKey::of(agent, &key);
    map_data_mut(agent, m)
        .expect("map")
        .set(k, key, value.clone());
    Ok(value)
}

fn map_has(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let m = this_map(agent, &a.this, "has")?;
    let k = MapKey::of(agent, &a.arg(0));
    Ok(Value::Boolean(
        map_data(agent, m).is_some_and(|d| d.find(&k).is_some()),
    ))
}

fn map_set(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let m = this_map(agent, &a.this, "set")?;
    let key = canonicalize(a.arg(0));
    let k = MapKey::of(agent, &key);
    if let Some(d) = map_data_mut(agent, m) {
        d.set(k, key, a.arg(1));
    }
    Ok(a.this.clone())
}

fn map_size(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let m = this_map(agent, &a.this, "size")?;
    Ok(Value::Number(
        map_data(agent, m).map_or(0, |d| d.size) as f64
    ))
}

fn create_iterator(agent: &mut Agent, o: Gc<Object>, kind: IterKind, set: bool) -> Value {
    let i = agent.intrinsics();
    let proto = if set {
        i.set_iterator_prototype
    } else {
        i.map_iterator_prototype
    };
    let data = Box::new(CollectionIterator {
        target: Some(o),
        index: 0,
        kind,
    });
    let k = if set {
        ObjectKind::SetIterator(data)
    } else {
        ObjectKind::MapIterator(data)
    };
    let it = agent.alloc(Object::new(Some(proto), k));
    // La colección conoce sus iteradores para reubicarlos al compactar (sin retenerlos).
    let alive: Vec<bool> = map_data(agent, o).map_or(Vec::new(), |d| {
        if d.iterators.len() >= 64 && d.iterators.len().is_power_of_two() {
            d.iterators
                .iter()
                .map(|i| agent.heap.is_alive(*i))
                .collect()
        } else {
            Vec::new()
        }
    });
    if let Some(d) = map_data_mut(agent, o) {
        if !alive.is_empty() {
            let mut keep = alive.into_iter();
            d.iterators.retain(|_| keep.next().unwrap_or(false));
        }
        d.iterators.push(it);
    }
    Value::Object(it)
}

fn map_entries(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let m = this_map(agent, &a.this, "entries")?;
    Ok(create_iterator(agent, m, IterKind::Entries, false))
}

fn map_keys(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let m = this_map(agent, &a.this, "keys")?;
    Ok(create_iterator(agent, m, IterKind::Keys, false))
}

fn map_values(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let m = this_map(agent, &a.this, "values")?;
    Ok(create_iterator(agent, m, IterKind::Values, false))
}

/// %MapIteratorPrototype%.next y %SetIteratorPrototype%.next (§24.1.5.2.1, §24.2.6.2.1).
fn iterator_next(agent: &mut Agent, this: &Value, set: bool) -> JsResult<Value> {
    let it = match this {
        Value::Object(o) => match &agent.heap[*o].kind {
            ObjectKind::MapIterator(_) if !set => *o,
            ObjectKind::SetIterator(_) if set => *o,
            _ => return agent.type_error("next method called on incompatible receiver"),
        },
        _ => return agent.type_error("next method called on incompatible receiver"),
    };
    let (target, mut index, kind) = match &agent.heap[it].kind {
        ObjectKind::MapIterator(d) | ObjectKind::SetIterator(d) => (d.target, d.index, d.kind),
        _ => unreachable!("iterador"),
    };
    let Some(target) = target else {
        return agent.create_iter_result(Value::Undefined, true);
    };
    let found = {
        let d = map_data(agent, target).expect("colección");
        let mut found = None;
        while index < d.entries.len() {
            let e = d.entries[index].clone();
            index += 1;
            if let Some(e) = e {
                found = Some(e);
                break;
            }
        }
        found
    };
    if let ObjectKind::MapIterator(d) | ObjectKind::SetIterator(d) = &mut agent.heap[it].kind {
        d.index = index;
        if found.is_none() {
            d.target = None;
        }
    }
    let Some((k, v)) = found else {
        return agent.create_iter_result(Value::Undefined, true);
    };
    let result = match kind {
        IterKind::Keys => k,
        IterKind::Values => {
            if set {
                k
            } else {
                v
            }
        }
        IterKind::Entries => {
            let v = if set { k.clone() } else { v };
            Value::Object(agent.create_array_from_list(&[k, v])?)
        }
    };
    agent.create_iter_result(result, false)
}

fn map_iterator_next(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    iterator_next(agent, &a.this, false)
}

fn set_iterator_next(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    iterator_next(agent, &a.this, true)
}

// --- Set ---

/// Set ( [ iterable ] ) (§24.2.2.1).
fn set_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor Set requires 'new'");
    };
    let set = agent.ordinary_create_from_constructor(
        nt,
        |i| i.set_prototype,
        ObjectKind::Set(Box::default()),
    )?;
    let iterable = a.arg(0);
    if iterable.is_nullish() {
        return Ok(Value::Object(set));
    }
    let adder = agent.get(set, &PropertyKey::from("add"), Value::Object(set))?;
    if !agent.is_callable(&adder) {
        return agent.type_error("Set.prototype.add is not a function");
    }
    add_values_from_iterable(agent, set, &iterable, &adder)?;
    Ok(Value::Object(set))
}

fn set_add(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_set(agent, &a.this, "add")?;
    let v = canonicalize(a.arg(0));
    let k = MapKey::of(agent, &v);
    if let Some(d) = map_data_mut(agent, s)
        && d.find(&k).is_none()
    {
        d.set(k, v, Value::Undefined);
    }
    Ok(a.this.clone())
}

fn set_clear(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_set(agent, &a.this, "clear")?;
    if let Some(d) = map_data_mut(agent, s) {
        d.clear();
    }
    maybe_compact(agent, s);
    Ok(Value::Undefined)
}

fn set_delete(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_set(agent, &a.this, "delete")?;
    let k = MapKey::of(agent, &a.arg(0));
    let r = map_data_mut(agent, s).is_some_and(|d| d.remove(&k));
    maybe_compact(agent, s);
    Ok(Value::Boolean(r))
}

fn set_entries(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_set(agent, &a.this, "entries")?;
    Ok(create_iterator(agent, s, IterKind::Entries, true))
}

fn set_values(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_set(agent, &a.this, "values")?;
    Ok(create_iterator(agent, s, IterKind::Values, true))
}

fn set_for_each(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_set(agent, &a.this, "forEach")?;
    for_each(agent, s, a, true)
}

fn set_has(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_set(agent, &a.this, "has")?;
    let k = MapKey::of(agent, &a.arg(0));
    Ok(Value::Boolean(
        map_data(agent, s).is_some_and(|d| d.find(&k).is_some()),
    ))
}

fn set_size(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_set(agent, &a.this, "size")?;
    Ok(Value::Number(
        map_data(agent, s).map_or(0, |d| d.size) as f64
    ))
}

// --- Métodos de conjuntos (§24.2.4) ---

/// Set Record (§24.2.1.1).
struct SetRecord {
    object: Value,
    size: f64,
    has: Value,
    keys: Value,
}

/// GetSetRecord (§24.2.1.2).
fn get_set_record(agent: &mut Agent, obj: &Value) -> JsResult<SetRecord> {
    let Value::Object(o) = obj else {
        return agent.type_error("The argument must be an object");
    };
    let raw = agent.get(*o, &PropertyKey::from("size"), obj.clone())?;
    let num = agent.to_number(&raw)?;
    if num.is_nan() {
        return agent.type_error("The 'size' property must be a number");
    }
    let int = crate::abstract_ops::conversion::integer_or_infinity(num);
    if int < 0.0 {
        return agent.range_error("The 'size' property must be non-negative");
    }
    let has = agent.get(*o, &PropertyKey::from("has"), obj.clone())?;
    if !agent.is_callable(&has) {
        return agent.type_error("The 'has' property must be a function");
    }
    let keys = agent.get(*o, &PropertyKey::from("keys"), obj.clone())?;
    if !agent.is_callable(&keys) {
        return agent.type_error("The 'keys' property must be a function");
    }
    Ok(SetRecord {
        object: obj.clone(),
        size: int,
        has,
        keys,
    })
}

/// GetIteratorFromMethod(otherRec.[[SetObject]], otherRec.[[Keys]]) (§24.2.4, GetKeysIterator).
fn keys_iterator(agent: &mut Agent, rec: &SetRecord) -> JsResult<(Value, Value)> {
    let it = agent.call(&rec.keys, rec.object.clone(), &[])?;
    let Value::Object(io) = it else {
        return agent.type_error("keys() did not return an object");
    };
    let k = agent.keys.next.clone();
    let next = agent.get(io, &k, it.clone())?;
    if !agent.is_callable(&next) {
        return agent.type_error("The iterator's 'next' property must be a function");
    }
    Ok((it, next))
}

fn set_size_of(agent: &Agent, s: Gc<Object>) -> usize {
    map_data(agent, s).map_or(0, |d| d.size)
}

fn set_entry(agent: &Agent, s: Gc<Object>, i: usize) -> Option<Value> {
    map_data(agent, s)
        .and_then(|d| d.entries.get(i).cloned())
        .flatten()
        .map(|e| e.0)
}

fn set_list_len(agent: &Agent, s: Gc<Object>) -> usize {
    map_data(agent, s).map_or(0, |d| d.entries.len())
}

fn set_contains(agent: &Agent, s: Gc<Object>, v: &Value) -> bool {
    let k = MapKey::of(agent, v);
    map_data(agent, s).is_some_and(|d| d.find(&k).is_some())
}

/// Una copia de `[[SetData]]` con sus huecos.
fn copy_set_data(agent: &Agent, s: Gc<Object>) -> Vec<Option<Value>> {
    map_data(agent, s).map_or(Vec::new(), |d| {
        d.entries
            .iter()
            .map(|e| e.as_ref().map(|e| e.0.clone()))
            .collect()
    })
}

fn new_set_from(agent: &mut Agent, list: Vec<Option<Value>>) -> Value {
    let data = MapData::from_keys(agent, list.into_iter().flatten());
    let proto = agent.intrinsics().set_prototype;
    Value::Object(agent.alloc(Object::new(Some(proto), ObjectKind::Set(Box::new(data)))))
}

fn list_index(agent: &Agent, list: &[Option<Value>], v: &Value) -> Option<usize> {
    let k = MapKey::of(agent, v);
    list.iter()
        .position(|e| e.as_ref().is_some_and(|e| MapKey::of(agent, e) == k))
}

fn call_has(agent: &mut Agent, rec: &SetRecord, e: Value) -> JsResult<bool> {
    let r = agent.call(&rec.has, rec.object.clone(), &[e])?;
    Ok(crate::abstract_ops::conversion::to_boolean(agent, &r))
}

/// Set.prototype.union (§24.2.4.18).
fn set_union(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_set(agent, &a.this, "union")?;
    let rec = get_set_record(agent, &a.arg(0))?;
    let (it, next) = keys_iterator(agent, &rec)?;
    let mut result = copy_set_data(agent, s);
    while let Some(v) = agent.iterator_step_value(&it, &next)? {
        let v = canonicalize(v);
        if list_index(agent, &result, &v).is_none() {
            result.push(Some(v));
        }
    }
    Ok(new_set_from(agent, result))
}

/// Set.prototype.intersection (§24.2.4.9).
fn set_intersection(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_set(agent, &a.this, "intersection")?;
    let rec = get_set_record(agent, &a.arg(0))?;
    let mut result: Vec<Option<Value>> = Vec::new();
    if (set_size_of(agent, s) as f64) <= rec.size {
        without_compaction(agent, s, |agent| {
            let mut this_size = set_list_len(agent, s);
            let mut index = 0;
            while index < this_size {
                let e = set_entry(agent, s, index);
                index += 1;
                if let Some(e) = e {
                    if call_has(agent, &rec, e.clone())? && list_index(agent, &result, &e).is_none()
                    {
                        result.push(Some(e));
                    }
                    this_size = set_list_len(agent, s);
                }
            }
            Ok(())
        })?;
    } else {
        let (it, next) = keys_iterator(agent, &rec)?;
        while let Some(v) = agent.iterator_step_value(&it, &next)? {
            let v = canonicalize(v);
            if set_contains(agent, s, &v) && list_index(agent, &result, &v).is_none() {
                result.push(Some(v));
            }
        }
    }
    Ok(new_set_from(agent, result))
}

/// Set.prototype.difference (§24.2.4.5).
fn set_difference(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_set(agent, &a.this, "difference")?;
    let rec = get_set_record(agent, &a.arg(0))?;
    let mut result = copy_set_data(agent, s);
    if (set_size_of(agent, s) as f64) <= rec.size {
        let this_size = set_list_len(agent, s);
        let mut index = 0;
        while index < this_size {
            if let Some(Some(e)) = result.get(index).cloned()
                && call_has(agent, &rec, e)?
            {
                result[index] = None;
            }
            index += 1;
        }
    } else {
        let (it, next) = keys_iterator(agent, &rec)?;
        while let Some(v) = agent.iterator_step_value(&it, &next)? {
            let v = canonicalize(v);
            if let Some(i) = list_index(agent, &result, &v) {
                result[i] = None;
            }
        }
    }
    Ok(new_set_from(agent, result))
}

/// Set.prototype.symmetricDifference (§24.2.4.15).
fn set_symmetric_difference(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_set(agent, &a.this, "symmetricDifference")?;
    let rec = get_set_record(agent, &a.arg(0))?;
    let (it, next) = keys_iterator(agent, &rec)?;
    let mut result = copy_set_data(agent, s);
    while let Some(v) = agent.iterator_step_value(&it, &next)? {
        let v = canonicalize(v);
        let ri = list_index(agent, &result, &v);
        if set_contains(agent, s, &v) {
            if let Some(i) = ri {
                result[i] = None;
            }
        } else if ri.is_none() {
            result.push(Some(v));
        }
    }
    Ok(new_set_from(agent, result))
}

/// Set.prototype.isSubsetOf (§24.2.4.11).
fn set_is_subset_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_set(agent, &a.this, "isSubsetOf")?;
    let rec = get_set_record(agent, &a.arg(0))?;
    if (set_size_of(agent, s) as f64) > rec.size {
        return Ok(Value::Boolean(false));
    }
    without_compaction(agent, s, |agent| {
        let mut this_size = set_list_len(agent, s);
        let mut index = 0;
        while index < this_size {
            let e = set_entry(agent, s, index);
            index += 1;
            if let Some(e) = e {
                if !call_has(agent, &rec, e)? {
                    return Ok(Value::Boolean(false));
                }
                this_size = set_list_len(agent, s);
            }
        }
        Ok(Value::Boolean(true))
    })
}

/// Set.prototype.isSupersetOf (§24.2.4.12).
fn set_is_superset_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_set(agent, &a.this, "isSupersetOf")?;
    let rec = get_set_record(agent, &a.arg(0))?;
    if (set_size_of(agent, s) as f64) < rec.size {
        return Ok(Value::Boolean(false));
    }
    let (it, next) = keys_iterator(agent, &rec)?;
    while let Some(v) = agent.iterator_step_value(&it, &next)? {
        if !set_contains(agent, s, &v) {
            agent.iterator_close(&it)?;
            return Ok(Value::Boolean(false));
        }
    }
    Ok(Value::Boolean(true))
}

/// Set.prototype.isDisjointFrom (§24.2.4.10).
fn set_is_disjoint_from(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_set(agent, &a.this, "isDisjointFrom")?;
    let rec = get_set_record(agent, &a.arg(0))?;
    if (set_size_of(agent, s) as f64) <= rec.size {
        let disjoint = without_compaction(agent, s, |agent| {
            let mut this_size = set_list_len(agent, s);
            let mut index = 0;
            while index < this_size {
                let e = set_entry(agent, s, index);
                index += 1;
                if let Some(e) = e {
                    if call_has(agent, &rec, e)? {
                        return Ok(false);
                    }
                    this_size = set_list_len(agent, s);
                }
            }
            Ok(true)
        })?;
        if !disjoint {
            return Ok(Value::Boolean(false));
        }
    } else {
        let (it, next) = keys_iterator(agent, &rec)?;
        while let Some(v) = agent.iterator_step_value(&it, &next)? {
            if set_contains(agent, s, &v) {
                agent.iterator_close(&it)?;
                return Ok(Value::Boolean(false));
            }
        }
    }
    Ok(Value::Boolean(true))
}
