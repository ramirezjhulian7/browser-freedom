//! Almacén de propiedades propias con el orden de §10.1.11.1 (OrdinaryOwnPropertyKeys): los
//! array index en orden numérico ascendente, después los strings en orden de creación y al
//! final los símbolos en orden de creación. Redefinir una propiedad no cambia su posición.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::rc::{Rc, Weak};

use crate::heap::{Gc, Trace, Tracer};
use crate::object::Object;
use crate::value::{PropertyKey, Value};

/// Una propiedad propia (§6.1.7.1).
#[derive(Debug, Clone)]
pub struct Prop {
    pub value: PropValue,
    pub enumerable: bool,
    pub configurable: bool,
}

#[derive(Debug, Clone)]
pub enum PropValue {
    Data {
        value: Value,
        writable: bool,
    },
    Accessor {
        get: Option<Gc<Object>>,
        set: Option<Gc<Object>>,
    },
}

impl Prop {
    /// Propiedad de datos con atributos `{writable, enumerable, configurable}`.
    pub fn data(value: Value, writable: bool, enumerable: bool, configurable: bool) -> Prop {
        Prop {
            value: PropValue::Data { value, writable },
            enumerable,
            configurable,
        }
    }

    pub fn is_accessor(&self) -> bool {
        matches!(self.value, PropValue::Accessor { .. })
    }

    pub fn to_descriptor(&self) -> PropertyDescriptor {
        match &self.value {
            PropValue::Data { value, writable } => PropertyDescriptor {
                value: Some(value.clone()),
                writable: Some(*writable),
                get: None,
                set: None,
                enumerable: Some(self.enumerable),
                configurable: Some(self.configurable),
            },
            PropValue::Accessor { get, set } => PropertyDescriptor {
                value: None,
                writable: None,
                get: Some(*get),
                set: Some(*set),
                enumerable: Some(self.enumerable),
                configurable: Some(self.configurable),
            },
        }
    }
}

impl Trace for Prop {
    fn trace(&self, t: &mut Tracer) {
        match &self.value {
            PropValue::Data { value, .. } => t.value(value),
            PropValue::Accessor { get, set } => {
                t.opt(*get);
                t.opt(*set);
            }
        }
    }
}

/// Property Descriptor (§6.2.6): cada campo puede estar ausente. `get`/`set` son
/// `Some(None)` cuando el campo está presente con valor `undefined`.
#[derive(Debug, Clone, Default)]
pub struct PropertyDescriptor {
    pub value: Option<Value>,
    pub writable: Option<bool>,
    pub get: Option<Option<Gc<Object>>>,
    pub set: Option<Option<Gc<Object>>>,
    pub enumerable: Option<bool>,
    pub configurable: Option<bool>,
}

impl PropertyDescriptor {
    /// Descriptor de datos completo.
    pub fn data(value: Value, writable: bool, enumerable: bool, configurable: bool) -> Self {
        PropertyDescriptor {
            value: Some(value),
            writable: Some(writable),
            get: None,
            set: None,
            enumerable: Some(enumerable),
            configurable: Some(configurable),
        }
    }

    /// Descriptor de accessor completo.
    pub fn accessor(
        get: Option<Gc<Object>>,
        set: Option<Gc<Object>>,
        enumerable: bool,
        configurable: bool,
    ) -> Self {
        PropertyDescriptor {
            value: None,
            writable: None,
            get: Some(get),
            set: Some(set),
            enumerable: Some(enumerable),
            configurable: Some(configurable),
        }
    }

    /// IsAccessorDescriptor (§6.2.6.1).
    pub fn is_accessor(&self) -> bool {
        self.get.is_some() || self.set.is_some()
    }

    /// IsDataDescriptor (§6.2.6.2).
    pub fn is_data(&self) -> bool {
        self.value.is_some() || self.writable.is_some()
    }

    /// IsGenericDescriptor (§6.2.6.3).
    pub fn is_generic(&self) -> bool {
        !self.is_accessor() && !self.is_data()
    }

    /// El descriptor completo como propiedad, con los valores por defecto de §6.1.7.1 para
    /// los campos ausentes.
    pub fn to_prop(&self) -> Prop {
        let value = if self.is_accessor() {
            PropValue::Accessor {
                get: self.get.flatten(),
                set: self.set.flatten(),
            }
        } else {
            PropValue::Data {
                value: self.value.clone().unwrap_or_default(),
                writable: self.writable.unwrap_or(false),
            }
        };
        Prop {
            value,
            enumerable: self.enumerable.unwrap_or(false),
            configurable: self.configurable.unwrap_or(false),
        }
    }
}

impl Trace for PropertyDescriptor {
    fn trace(&self, t: &mut Tracer) {
        t.opt_value(&self.value);
        t.opt(self.get.flatten());
        t.opt(self.set.flatten());
    }
}

// --- Shapes (E3: rendimiento de la VM) ---
//
// Las propiedades con nombre de un objeto son un [`Shape`] (las claves en orden de creación)
// más un vector de valores paralelo. Los objetos que reciben las mismas claves en el mismo
// orden comparten el shape (un árbol de transiciones desde el shape vacío), así que el `id`
// del shape identifica la disposición de las propiedades: las inline caches de la VM guardan
// `(id, posición)` y leen el valor sin buscar la clave. Borrar una clave o tener muchas pasa
// el objeto a *modo diccionario* (entradas propias con un índice hash) con un `id` que cambia
// en cada cambio de claves. Cambiar el valor o los atributos de una propiedad existente no
// cambia el `id`.

/// Más de esta cantidad de claves en un shape usa un índice hash.
const LINEAR_LIMIT: usize = 8;
/// Más de esta cantidad de claves pasa el objeto a modo diccionario.
const DICTIONARY_LIMIT: usize = 64;

thread_local! {
    static NEXT_SHAPE_ID: Cell<u64> = const { Cell::new(1) };
    static ROOT_SHAPE: Rc<Shape> = Rc::new(Shape::empty());
    static SETTER_EPOCH: Cell<u64> = const { Cell::new(0) };
}

fn next_shape_id() -> u64 {
    NEXT_SHAPE_ID.with(|c| {
        let id = c.get();
        c.set(id + 1);
        id
    })
}

/// Un contador que avanza cada vez que aparece en algún objeto una propiedad accessor o no
/// escribible, o cambia un prototipo: las caches que agregan propiedades (que suponen que la
/// cadena de prototipos no tiene un setter ni una propiedad de solo lectura para la clave)
/// solo valen con el mismo epoch.
pub fn setter_epoch() -> u64 {
    SETTER_EPOCH.with(Cell::get)
}

/// Avanza [`setter_epoch`] (un `[[SetPrototypeOf]]`, o un cambio que la VM no ve).
pub fn bump_setter_epoch() {
    SETTER_EPOCH.with(|c| c.set(c.get() + 1));
}

/// La disposición compartida de las propiedades con nombre.
pub struct Shape {
    id: u64,
    keys: Vec<PropertyKey>,
    lookup: Option<HashMap<PropertyKey, u32>>,
    /// La primera transición (la mayoría de los shapes tiene una sola): sin hash.
    first: RefCell<Option<(PropertyKey, Weak<Shape>)>>,
    /// Las demás transiciones: clave agregada → shape hijo.
    transitions: RefCell<HashMap<PropertyKey, Weak<Shape>>>,
}

impl fmt::Debug for Shape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Shape#{}({} claves)", self.id, self.keys.len())
    }
}

impl Shape {
    fn empty() -> Shape {
        Shape {
            id: next_shape_id(),
            keys: Vec::new(),
            lookup: None,
            first: RefCell::new(None),
            transitions: RefCell::new(HashMap::new()),
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// La última clave agregada.
    pub fn last_key(&self) -> Option<&PropertyKey> {
        self.keys.last()
    }

    /// La posición de `key`.
    pub fn find(&self, key: &PropertyKey) -> Option<usize> {
        match &self.lookup {
            Some(map) => map.get(key).map(|&i| i as usize),
            None => self.keys.iter().position(|k| k == key),
        }
    }

    /// El shape con `key` agregada al final (lo crea la primera vez).
    pub fn transition(self: &Rc<Shape>, key: &PropertyKey) -> Rc<Shape> {
        if let Some((k, w)) = &*self.first.borrow()
            && k == key
            && let Some(s) = w.upgrade()
        {
            return s;
        }
        if let Some(s) = self.transitions.borrow().get(key).and_then(Weak::upgrade) {
            return s;
        }
        let mut keys = Vec::with_capacity(self.keys.len() + 1);
        keys.extend(self.keys.iter().cloned());
        keys.push(key.clone());
        let lookup = (keys.len() > LINEAR_LIMIT).then(|| {
            keys.iter()
                .enumerate()
                .map(|(i, k)| (k.clone(), i as u32))
                .collect()
        });
        let child = Rc::new(Shape {
            id: next_shape_id(),
            keys,
            lookup,
            first: RefCell::new(None),
            transitions: RefCell::new(HashMap::new()),
        });
        let mut first = self.first.borrow_mut();
        let free = first.as_ref().is_none_or(|(_, w)| w.strong_count() == 0);
        if free {
            *first = Some((key.clone(), Rc::downgrade(&child)));
        } else {
            self.transitions
                .borrow_mut()
                .insert(key.clone(), Rc::downgrade(&child));
        }
        child
    }
}

/// Las propiedades con nombre de un objeto en modo diccionario.
#[derive(Debug, Clone)]
struct Dictionary {
    /// Cambia en cada cambio de claves (como un shape propio).
    id: u64,
    /// En orden de creación; `None` son las borradas.
    entries: Vec<Option<(PropertyKey, Prop)>>,
    lookup: HashMap<PropertyKey, usize>,
    removed: usize,
}

impl Dictionary {
    fn compact(&mut self) {
        self.entries.retain(|e| e.is_some());
        self.removed = 0;
        self.lookup = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.as_ref().map(|(k, _)| (k.clone(), i)))
            .collect();
    }
}

#[derive(Debug, Clone)]
enum Named {
    Shaped { shape: Rc<Shape>, values: Vec<Prop> },
    Dict(Box<Dictionary>),
}

#[derive(Debug, Clone)]
pub struct PropertyMap {
    indexed: BTreeMap<u32, Prop>,
    named: Named,
}

impl Default for PropertyMap {
    fn default() -> PropertyMap {
        PropertyMap {
            indexed: BTreeMap::new(),
            named: Named::Shaped {
                shape: ROOT_SHAPE.with(Rc::clone),
                values: Vec::new(),
            },
        }
    }
}

/// ¿Agregar o cambiar `prop` puede alterar una asignación cacheada (un setter o una
/// propiedad de solo lectura)?
fn affects_sets(prop: &Prop) -> bool {
    match &prop.value {
        PropValue::Accessor { .. } => true,
        PropValue::Data { writable, .. } => !writable,
    }
}

impl PropertyMap {
    pub fn new() -> PropertyMap {
        PropertyMap::default()
    }

    /// El shape compartido (`None` en modo diccionario).
    pub fn shape(&self) -> Option<&Rc<Shape>> {
        match &self.named {
            Named::Shaped { shape, .. } => Some(shape),
            Named::Dict(_) => None,
        }
    }

    /// El `id` de la disposición de las claves con nombre.
    pub fn shape_id(&self) -> u64 {
        match &self.named {
            Named::Shaped { shape, .. } => shape.id,
            Named::Dict(d) => d.id,
        }
    }

    pub fn is_dictionary(&self) -> bool {
        matches!(self.named, Named::Dict(_))
    }

    /// Cuántas claves con nombre hay.
    pub fn named_len(&self) -> usize {
        match &self.named {
            Named::Shaped { values, .. } => values.len(),
            Named::Dict(d) => d.entries.len() - d.removed,
        }
    }

    /// La propiedad con nombre de la posición `slot`.
    pub fn slot(&self, slot: usize) -> Option<&Prop> {
        match &self.named {
            Named::Shaped { values, .. } => values.get(slot),
            Named::Dict(d) => d.entries.get(slot)?.as_ref().map(|(_, p)| p),
        }
    }

    pub fn slot_mut(&mut self, slot: usize) -> Option<&mut Prop> {
        match &mut self.named {
            Named::Shaped { values, .. } => values.get_mut(slot),
            Named::Dict(d) => d.entries.get_mut(slot)?.as_mut().map(|(_, p)| p),
        }
    }

    /// La posición de una clave con nombre.
    pub fn find_slot(&self, key: &PropertyKey) -> Option<usize> {
        match &self.named {
            Named::Shaped { shape, .. } => shape.find(key),
            Named::Dict(d) => d.lookup.get(key).copied(),
        }
    }

    /// Agrega una clave nueva con la transición ya conocida `to` (la de una inline cache):
    /// `to` debe ser la transición del shape actual con esa clave.
    pub fn push_with_shape(&mut self, to: Rc<Shape>, prop: Prop) {
        if affects_sets(&prop) {
            bump_setter_epoch();
        }
        if let Named::Shaped { shape, values } = &mut self.named {
            *shape = to;
            values.push(prop);
        }
    }

    pub fn get(&self, key: &PropertyKey) -> Option<&Prop> {
        if let PropertyKey::Index(i) = key {
            return self.indexed.get(i);
        }
        let i = self.find_slot(key)?;
        self.slot(i)
    }

    pub fn get_mut(&mut self, key: &PropertyKey) -> Option<&mut Prop> {
        if let PropertyKey::Index(i) = key {
            return self.indexed.get_mut(i);
        }
        let i = self.find_slot(key)?;
        // Un cambio de atributos por esta vía no pasa por `insert`.
        bump_setter_epoch();
        self.slot_mut(i)
    }

    pub fn contains(&self, key: &PropertyKey) -> bool {
        self.get(key).is_some()
    }

    /// Pasa a modo diccionario.
    fn to_dictionary(&mut self) -> &mut Dictionary {
        if let Named::Shaped { shape, values } = &mut self.named {
            let entries: Vec<Option<(PropertyKey, Prop)>> = shape
                .keys
                .iter()
                .cloned()
                .zip(std::mem::take(values))
                .map(Some)
                .collect();
            let lookup = shape
                .keys
                .iter()
                .enumerate()
                .map(|(i, k)| (k.clone(), i))
                .collect();
            self.named = Named::Dict(Box::new(Dictionary {
                id: next_shape_id(),
                entries,
                lookup,
                removed: 0,
            }));
        }
        match &mut self.named {
            Named::Dict(d) => d,
            Named::Shaped { .. } => unreachable!("diccionario"),
        }
    }

    /// Crea o reemplaza (conservando la posición).
    pub fn insert(&mut self, key: PropertyKey, prop: Prop) {
        if let PropertyKey::Index(i) = key {
            self.indexed.insert(i, prop);
            return;
        }
        if affects_sets(&prop) {
            bump_setter_epoch();
        }
        if let Some(i) = self.find_slot(&key) {
            if let Some(p) = self.slot_mut(i) {
                *p = prop;
            }
            return;
        }
        let to_dict = matches!(&self.named, Named::Shaped { shape, .. } if shape.keys.len() >= DICTIONARY_LIMIT);
        if to_dict {
            self.to_dictionary();
        }
        match &mut self.named {
            Named::Shaped { shape, values } => {
                *shape = shape.transition(&key);
                values.push(prop);
            }
            Named::Dict(d) => {
                d.id = next_shape_id();
                d.lookup.insert(key.clone(), d.entries.len());
                d.entries.push(Some((key, prop)));
            }
        }
    }

    pub fn remove(&mut self, key: &PropertyKey) -> Option<Prop> {
        if let PropertyKey::Index(i) = key {
            return self.indexed.remove(i);
        }
        self.find_slot(key)?;
        // Borrar pasa el objeto a modo diccionario.
        let d = self.to_dictionary();
        let i = d.lookup.remove(key)?;
        let (_, prop) = d.entries[i].take()?;
        d.id = next_shape_id();
        d.removed += 1;
        if d.removed > 16 && d.removed * 2 > d.entries.len() {
            d.compact();
        }
        Some(prop)
    }

    /// Las claves en el orden de §10.1.11.1.
    pub fn keys(&self) -> Vec<PropertyKey> {
        let mut out: Vec<PropertyKey> = self
            .indexed
            .keys()
            .map(|&i| PropertyKey::Index(i))
            .collect();
        self.push_named_keys(&mut out);
        out
    }

    /// Los strings y después los símbolos (sin los índices).
    pub fn push_named_keys(&self, out: &mut Vec<PropertyKey>) {
        fn push<'a>(out: &mut Vec<PropertyKey>, keys: impl Iterator<Item = &'a PropertyKey> + Clone) {
            out.extend(keys.clone().filter(|k| !k.is_symbol()).cloned());
            out.extend(keys.filter(|k| k.is_symbol()).cloned());
        }
        match &self.named {
            Named::Shaped { shape, .. } => push(out, shape.keys.iter()),
            Named::Dict(d) => push(out, d.entries.iter().flatten().map(|(k, _)| k)),
        }
    }

    /// Los índices en orden ascendente.
    pub fn index_keys(&self) -> impl DoubleEndedIterator<Item = u32> + '_ {
        self.indexed.keys().copied()
    }

    /// Índices `>= from`, del mayor al menor.
    pub fn index_keys_from_desc(&self, from: u32) -> Vec<u32> {
        self.indexed.range(from..).rev().map(|(&k, _)| k).collect()
    }

    pub fn len(&self) -> usize {
        self.indexed.len() + self.named_len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn values(&self) -> Box<dyn Iterator<Item = &Prop> + '_> {
        match &self.named {
            Named::Shaped { values, .. } => Box::new(self.indexed.values().chain(values.iter())),
            Named::Dict(d) => Box::new(
                self.indexed
                    .values()
                    .chain(d.entries.iter().flatten().map(|(_, p)| p)),
            ),
        }
    }

    pub fn values_mut(&mut self) -> Box<dyn Iterator<Item = &mut Prop> + '_> {
        // Puede cambiar atributos sin pasar por `insert`.
        bump_setter_epoch();
        match &mut self.named {
            Named::Shaped { values, .. } => {
                Box::new(self.indexed.values_mut().chain(values.iter_mut()))
            }
            Named::Dict(d) => Box::new(
                self.indexed
                    .values_mut()
                    .chain(d.entries.iter_mut().flatten().map(|(_, p)| p)),
            ),
        }
    }
}

impl Trace for PropertyMap {
    fn trace(&self, t: &mut Tracer) {
        for p in self.indexed.values() {
            p.trace(t);
        }
        let trace_key = |t: &mut Tracer, k: &PropertyKey| {
            if let PropertyKey::Symbol(s) = k {
                t.gc(*s);
            }
        };
        match &self.named {
            Named::Shaped { shape, values } => {
                for p in values {
                    p.trace(t);
                }
                for k in &shape.keys {
                    trace_key(t, k);
                }
            }
            Named::Dict(d) => {
                for (k, p) in d.entries.iter().flatten() {
                    p.trace(t);
                    trace_key(t, k);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(n: f64) -> Prop {
        Prop::data(Value::Number(n), true, true, true)
    }

    #[test]
    fn order_is_indices_then_strings() {
        let mut m = PropertyMap::new();
        m.insert(PropertyKey::from("b"), d(1.0));
        m.insert(PropertyKey::from("2"), d(2.0));
        m.insert(PropertyKey::from("a"), d(3.0));
        m.insert(PropertyKey::from("0"), d(4.0));
        m.insert(PropertyKey::from("b"), d(5.0));
        let keys: Vec<String> = m.keys().iter().map(|k| format!("{k:?}")).collect();
        assert_eq!(keys, ["0", "2", "\"b\"", "\"a\""]);
        m.remove(&PropertyKey::from("b"));
        m.insert(PropertyKey::from("b"), d(6.0));
        let keys: Vec<String> = m.keys().iter().map(|k| format!("{k:?}")).collect();
        assert_eq!(keys, ["0", "2", "\"a\"", "\"b\""]);
        for i in 0..20 {
            m.insert(PropertyKey::from(format!("k{i}").as_str()), d(f64::from(i)));
        }
        assert!(m.get(&PropertyKey::from("k7")).is_some());
        assert_eq!(m.len(), 24);
    }
}
