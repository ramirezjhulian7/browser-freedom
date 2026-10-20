//! Almacén de propiedades propias con el orden de §10.1.11.1 (OrdinaryOwnPropertyKeys): los
//! array index en orden numérico ascendente, después los strings en orden de creación y al
//! final los símbolos en orden de creación. Redefinir una propiedad no cambia su posición.

use std::collections::{BTreeMap, HashMap};

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

/// Más de esta cantidad de propiedades con nombre usa un índice hash.
const LINEAR_LIMIT: usize = 8;

#[derive(Debug, Default, Clone)]
pub struct PropertyMap {
    indexed: BTreeMap<u32, Prop>,
    /// Strings y símbolos en orden de creación; `None` son las borradas.
    named: Vec<Option<(PropertyKey, Prop)>>,
    lookup: Option<HashMap<PropertyKey, usize>>,
    removed: usize,
}

impl PropertyMap {
    pub fn new() -> PropertyMap {
        PropertyMap::default()
    }

    fn find(&self, key: &PropertyKey) -> Option<usize> {
        match &self.lookup {
            Some(map) => map.get(key).copied(),
            None => self
                .named
                .iter()
                .position(|e| matches!(e, Some((k, _)) if k == key)),
        }
    }

    pub fn get(&self, key: &PropertyKey) -> Option<&Prop> {
        if let PropertyKey::Index(i) = key {
            return self.indexed.get(i);
        }
        let i = self.find(key)?;
        self.named[i].as_ref().map(|(_, p)| p)
    }

    pub fn get_mut(&mut self, key: &PropertyKey) -> Option<&mut Prop> {
        if let PropertyKey::Index(i) = key {
            return self.indexed.get_mut(i);
        }
        let i = self.find(key)?;
        self.named[i].as_mut().map(|(_, p)| p)
    }

    pub fn contains(&self, key: &PropertyKey) -> bool {
        self.get(key).is_some()
    }

    /// Crea o reemplaza (conservando la posición).
    pub fn insert(&mut self, key: PropertyKey, prop: Prop) {
        if let PropertyKey::Index(i) = key {
            self.indexed.insert(i, prop);
            return;
        }
        if let Some(i) = self.find(&key) {
            self.named[i] = Some((key, prop));
            return;
        }
        let pos = self.named.len();
        if let Some(map) = &mut self.lookup {
            map.insert(key.clone(), pos);
        }
        self.named.push(Some((key, prop)));
        if self.lookup.is_none() && self.named.len() - self.removed > LINEAR_LIMIT {
            self.rebuild_lookup();
        }
    }

    fn rebuild_lookup(&mut self) {
        let mut map = HashMap::new();
        for (i, e) in self.named.iter().enumerate() {
            if let Some((k, _)) = e {
                map.insert(k.clone(), i);
            }
        }
        self.lookup = Some(map);
    }

    pub fn remove(&mut self, key: &PropertyKey) -> Option<Prop> {
        if let PropertyKey::Index(i) = key {
            return self.indexed.remove(i);
        }
        let i = self.find(key)?;
        let (_, prop) = self.named[i].take()?;
        if let Some(map) = &mut self.lookup {
            map.remove(key);
        }
        self.removed += 1;
        if self.removed > 16 && self.removed * 2 > self.named.len() {
            self.named.retain(|e| e.is_some());
            self.removed = 0;
            if self.lookup.is_some() {
                self.rebuild_lookup();
            }
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
        for (k, _) in self.named.iter().flatten() {
            if !k.is_symbol() {
                out.push(k.clone());
            }
        }
        for (k, _) in self.named.iter().flatten() {
            if k.is_symbol() {
                out.push(k.clone());
            }
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
        self.indexed.len() + self.named.len() - self.removed
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn values(&self) -> impl Iterator<Item = &Prop> {
        self.indexed
            .values()
            .chain(self.named.iter().flatten().map(|(_, p)| p))
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut Prop> {
        self.indexed
            .values_mut()
            .chain(self.named.iter_mut().flatten().map(|(_, p)| p))
    }
}

impl Trace for PropertyMap {
    fn trace(&self, t: &mut Tracer) {
        for p in self.values() {
            p.trace(t);
        }
        for (k, _) in self.named.iter().flatten() {
            if let PropertyKey::Symbol(s) = k {
                t.gc(*s);
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
