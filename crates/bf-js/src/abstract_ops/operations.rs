//! Operaciones sobre objetos (§7.3) y el acceso a propiedades de las referencias (GetValue /
//! PutValue, §6.2.5.5–6) con bases primitivas.

use crate::abstract_ops::conversion::to_boolean;
use crate::heap::Gc;
use crate::object::{Object, ObjectKind, PropertyDescriptor};
use crate::string::JsString;
use crate::value::{PropertyKey, Value};
use crate::vm::{Agent, JsResult};

impl Agent {
    /// GetV (§7.3.3) / GetValue de una referencia de propiedad: las bases primitivas usan
    /// el prototipo de su wrapper con el primitivo como receiver.
    pub fn get_value_of(&mut self, base: &Value, key: &PropertyKey) -> JsResult<Value> {
        match base {
            Value::Object(o) => self.get(*o, key, base.clone()),
            Value::String(s) => {
                // Las propiedades propias del String exótico.
                if let PropertyKey::Index(i) = key
                    && (*i as usize) < s.len()
                {
                    return Ok(Value::String(JsString::from_units(vec![
                        s.units()[*i as usize],
                    ])));
                }
                if *key == self.keys.length {
                    return Ok(Value::Number(s.len() as f64));
                }
                let p = self.intrinsics().string_prototype;
                self.get(p, key, base.clone())
            }
            Value::Undefined | Value::Null => {
                let what = if base.is_null() { "null" } else { "undefined" };
                self.type_error(&format!(
                    "Cannot read properties of {what} (reading '{key}')"
                ))
            }
            _ => {
                let i = self.intrinsics();
                let p = match base {
                    Value::Number(_) => i.number_prototype,
                    Value::Boolean(_) => i.boolean_prototype,
                    Value::Symbol(_) => i.symbol_prototype,
                    _ => i.bigint_prototype,
                };
                self.get(p, key, base.clone())
            }
        }
    }

    /// PutValue de una referencia de propiedad (§6.2.5.6 paso 3).
    pub fn put_value_of(
        &mut self,
        base: &Value,
        key: PropertyKey,
        v: Value,
        strict: bool,
    ) -> JsResult<()> {
        let o = match base {
            Value::Object(o) => *o,
            other => self.to_object(other)?,
        };
        let ok = self.set(o, key.clone(), v, base.clone())?;
        if !ok && strict {
            return self.type_error(&format!(
                "Cannot assign to read only property '{key}' of object"
            ));
        }
        Ok(())
    }

    /// Get(O, P) (§7.3.2).
    pub fn get_prop(&mut self, o: Gc<Object>, key: &PropertyKey) -> JsResult<Value> {
        self.get(o, key, Value::Object(o))
    }

    /// Get con una clave de string.
    pub fn get_str(&mut self, o: Gc<Object>, key: &str) -> JsResult<Value> {
        let k = PropertyKey::from(key);
        self.get(o, &k, Value::Object(o))
    }

    /// Set(O, P, V, true).
    pub fn set_prop(&mut self, o: Gc<Object>, key: PropertyKey, v: Value) -> JsResult<()> {
        self.set_or_throw(o, key, v, true)
    }

    /// CreateDataProperty (§7.3.5).
    pub fn create_data_property(
        &mut self,
        o: Gc<Object>,
        key: PropertyKey,
        v: Value,
    ) -> JsResult<bool> {
        self.define_own_property(o, key, PropertyDescriptor::data(v, true, true, true))
    }

    /// CreateDataPropertyOrThrow (§7.3.7).
    pub fn create_data_property_or_throw(
        &mut self,
        o: Gc<Object>,
        key: PropertyKey,
        v: Value,
    ) -> JsResult<()> {
        if !self.create_data_property(o, key.clone(), v)? {
            return self.type_error(&format!("Cannot define property {key}"));
        }
        Ok(())
    }

    /// CreateNonEnumerableDataPropertyOrThrow (§7.3.8).
    pub fn create_non_enumerable_data_property_or_throw(
        &mut self,
        o: Gc<Object>,
        key: PropertyKey,
        v: Value,
    ) -> JsResult<()> {
        self.define_property_or_throw(o, key, PropertyDescriptor::data(v, true, false, true))
    }

    /// DefinePropertyOrThrow (§7.3.9).
    pub fn define_property_or_throw(
        &mut self,
        o: Gc<Object>,
        key: PropertyKey,
        desc: PropertyDescriptor,
    ) -> JsResult<()> {
        if !self.define_own_property(o, key.clone(), desc)? {
            return self.type_error(&format!("Cannot redefine property: {key}"));
        }
        Ok(())
    }

    /// DeletePropertyOrThrow (§7.3.10).
    pub fn delete_property_or_throw(&mut self, o: Gc<Object>, key: &PropertyKey) -> JsResult<()> {
        if !self.delete(o, key)? {
            return self.type_error(&format!("Cannot delete property '{key}'"));
        }
        Ok(())
    }

    /// GetMethod (§7.3.11): `None` si es undefined o null.
    pub fn get_method_of(&mut self, v: &Value, key: &PropertyKey) -> JsResult<Option<Value>> {
        let f = self.get_value_of(v, key)?;
        if f.is_nullish() {
            return Ok(None);
        }
        if !self.is_callable(&f) {
            return self.type_error(&format!("{key} is not a function"));
        }
        Ok(Some(f))
    }

    /// HasOwnProperty (§7.3.13).
    pub fn has_own_property(&mut self, o: Gc<Object>, key: &PropertyKey) -> JsResult<bool> {
        Ok(self.get_own_property(o, key)?.is_some())
    }

    /// SetIntegrityLevel (§7.3.16): `frozen` o sealed.
    pub fn set_integrity_level(&mut self, o: Gc<Object>, frozen: bool) -> JsResult<bool> {
        if !self.prevent_extensions(o)? {
            return Ok(false);
        }
        let keys = self.own_property_keys(o)?;
        for k in keys {
            if frozen {
                if let Some(cur) = self.get_own_property(o, &k)? {
                    let desc = if cur.is_accessor() {
                        PropertyDescriptor {
                            configurable: Some(false),
                            ..PropertyDescriptor::default()
                        }
                    } else {
                        PropertyDescriptor {
                            configurable: Some(false),
                            writable: Some(false),
                            ..PropertyDescriptor::default()
                        }
                    };
                    self.define_property_or_throw(o, k, desc)?;
                }
            } else {
                self.define_property_or_throw(
                    o,
                    k,
                    PropertyDescriptor {
                        configurable: Some(false),
                        ..PropertyDescriptor::default()
                    },
                )?;
            }
        }
        Ok(true)
    }

    /// TestIntegrityLevel (§7.3.17).
    pub fn test_integrity_level(&mut self, o: Gc<Object>, frozen: bool) -> JsResult<bool> {
        if self.is_extensible(o)? {
            return Ok(false);
        }
        let keys = self.own_property_keys(o)?;
        for k in keys {
            if let Some(d) = self.get_own_property(o, &k)? {
                if d.configurable == Some(true) {
                    return Ok(false);
                }
                if frozen && d.is_data() && d.writable == Some(true) {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// CreateArrayFromList (§7.3.18).
    pub fn create_array_from_list(&mut self, values: &[Value]) -> JsResult<Gc<Object>> {
        let a = self.array_create(0, None)?;
        if let ObjectKind::Array(d) = &mut self.heap[a].kind {
            d.dense = values.iter().cloned().map(Some).collect();
            d.length = values.len() as u32;
        }
        Ok(a)
    }

    /// LengthOfArrayLike (§7.3.19).
    pub fn length_of_array_like(&mut self, o: Gc<Object>) -> JsResult<f64> {
        let k = self.keys.length.clone();
        let v = self.get(o, &k, Value::Object(o))?;
        self.to_length(&v)
    }

    /// CreateListFromArrayLike (§7.3.20).
    pub fn create_list_from_array_like(
        &mut self,
        v: &Value,
        only_keys: bool,
    ) -> JsResult<Vec<Value>> {
        let Value::Object(o) = v else {
            return self.type_error("CreateListFromArrayLike called on non-object");
        };
        let len = self.length_of_array_like(*o)?;
        if len > 1e7 {
            return self.range_error("Too many arguments");
        }
        let mut out = Vec::with_capacity(len as usize);
        for i in 0..len as u64 {
            let next = self.get(*o, &PropertyKey::from(i as u32), v.clone())?;
            if only_keys && !matches!(next, Value::String(_) | Value::Symbol(_)) {
                return self.type_error("property key must be a string or symbol");
            }
            out.push(next);
        }
        Ok(out)
    }

    /// Invoke (§7.3.21).
    pub fn invoke(&mut self, v: &Value, key: &PropertyKey, args: &[Value]) -> JsResult<Value> {
        let f = self.get_value_of(v, key)?;
        self.call(&f, v.clone(), args)
    }

    /// SpeciesConstructor (§7.3.22).
    pub fn species_constructor(
        &mut self,
        o: Gc<Object>,
        default: Gc<Object>,
    ) -> JsResult<Gc<Object>> {
        let k = self.keys.constructor.clone();
        let c = self.get(o, &k, Value::Object(o))?;
        if c.is_undefined() {
            return Ok(default);
        }
        let Value::Object(c) = c else {
            return self.type_error("object.constructor is not an object");
        };
        let sk = PropertyKey::Symbol(self.well_known.species);
        let s = self.get(c, &sk, Value::Object(c))?;
        if s.is_nullish() {
            return Ok(default);
        }
        if self.is_constructor(&s) {
            return Ok(s.as_object().expect("constructor"));
        }
        self.type_error("object.constructor[Symbol.species] is not a constructor")
    }

    /// EnumerableOwnProperties (§7.3.23): claves, valores o entradas.
    pub fn enumerable_own_properties(
        &mut self,
        o: Gc<Object>,
        kind: crate::object::IterKind,
    ) -> JsResult<Vec<Value>> {
        let keys = self.own_property_keys(o)?;
        let mut out = Vec::new();
        for k in keys {
            if k.is_symbol() {
                continue;
            }
            let Some(d) = self.get_own_property(o, &k)? else {
                continue;
            };
            if d.enumerable != Some(true) {
                continue;
            }
            match kind {
                crate::object::IterKind::Keys => out.push(k.to_value()),
                crate::object::IterKind::Values => {
                    let v = self.get(o, &k, Value::Object(o))?;
                    out.push(v);
                }
                crate::object::IterKind::Entries => {
                    let v = self.get(o, &k, Value::Object(o))?;
                    let e = self.create_array_from_list(&[k.to_value(), v])?;
                    out.push(Value::Object(e));
                }
            }
        }
        Ok(out)
    }

    /// CopyDataProperties (§7.3.25).
    pub fn copy_data_properties(
        &mut self,
        target: Gc<Object>,
        source: &Value,
        excluded: &[PropertyKey],
    ) -> JsResult<()> {
        if source.is_nullish() {
            return Ok(());
        }
        let from = self.to_object(source)?;
        let keys = self.own_property_keys(from)?;
        for k in keys {
            if excluded.contains(&k) {
                continue;
            }
            if let Some(d) = self.get_own_property(from, &k)?
                && d.enumerable == Some(true)
            {
                let v = self.get(from, &k, Value::Object(from))?;
                self.create_data_property_or_throw(target, k, v)?;
            }
        }
        Ok(())
    }

    /// ToPropertyDescriptor (§6.2.6.5).
    pub fn to_property_descriptor(&mut self, v: &Value) -> JsResult<PropertyDescriptor> {
        let Value::Object(o) = v else {
            return self.type_error("Property description must be an object");
        };
        let o = *o;
        let mut d = PropertyDescriptor::default();
        let keys = self.keys.clone();
        if self.has_property(o, &keys.enumerable)? {
            let x = self.get(o, &keys.enumerable, v.clone())?;
            d.enumerable = Some(to_boolean(self, &x));
        }
        if self.has_property(o, &keys.configurable)? {
            let x = self.get(o, &keys.configurable, v.clone())?;
            d.configurable = Some(to_boolean(self, &x));
        }
        if self.has_property(o, &keys.value)? {
            d.value = Some(self.get(o, &keys.value, v.clone())?);
        }
        if self.has_property(o, &keys.writable)? {
            let x = self.get(o, &keys.writable, v.clone())?;
            d.writable = Some(to_boolean(self, &x));
        }
        if self.has_property(o, &keys.get)? {
            let g = self.get(o, &keys.get, v.clone())?;
            if !g.is_undefined() && !self.is_callable(&g) {
                return self.type_error("Getter must be a function");
            }
            d.get = Some(g.as_object());
        }
        if self.has_property(o, &keys.set)? {
            let s = self.get(o, &keys.set, v.clone())?;
            if !s.is_undefined() && !self.is_callable(&s) {
                return self.type_error("Setter must be a function");
            }
            d.set = Some(s.as_object());
        }
        if (d.get.is_some() || d.set.is_some()) && (d.value.is_some() || d.writable.is_some()) {
            return self.type_error("Invalid property descriptor. Cannot both specify accessors and a value or writable attribute");
        }
        Ok(d)
    }

    /// FromPropertyDescriptor (§6.2.6.4).
    pub fn from_property_descriptor(&mut self, d: Option<&PropertyDescriptor>) -> JsResult<Value> {
        let Some(d) = d else {
            return Ok(Value::Undefined);
        };
        let o = self.new_plain_object();
        let keys = self.keys.clone();
        if let Some(v) = &d.value {
            self.create_data_property_or_throw(o, keys.value.clone(), v.clone())?;
        }
        if let Some(w) = d.writable {
            self.create_data_property_or_throw(o, keys.writable.clone(), Value::Boolean(w))?;
        }
        if let Some(g) = d.get {
            self.create_data_property_or_throw(o, keys.get.clone(), g.into())?;
        }
        if let Some(s) = d.set {
            self.create_data_property_or_throw(o, keys.set.clone(), s.into())?;
        }
        if let Some(e) = d.enumerable {
            self.create_data_property_or_throw(o, keys.enumerable.clone(), Value::Boolean(e))?;
        }
        if let Some(c) = d.configurable {
            self.create_data_property_or_throw(o, keys.configurable.clone(), Value::Boolean(c))?;
        }
        Ok(Value::Object(o))
    }
}
