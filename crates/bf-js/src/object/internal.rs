//! Los métodos internos de los objetos (§6.1.7.2) despachados por [`ObjectKind`]: las
//! versiones ordinarias siguen §10.1 literalmente; los exóticos de esta etapa son Array
//! (§10.4.2), String (§10.4.3), arguments (§10.4.4), Module Namespace (§10.4.6) y Proxy
//! (§10.5).

use crate::heap::Gc;
use crate::object::{
    ArrayData, MappedArguments, Object, ObjectKind, Prop, PropValue, PropertyDescriptor,
};
use crate::string::JsString;
use crate::value::{PropertyKey, Value};
use crate::vm::{Agent, JsResult};

/// Cuánto puede crecer un array denso más allá de su final.
const DENSE_GAP: u32 = 64;

impl Agent {
    // --- [[GetPrototypeOf]] / [[SetPrototypeOf]] ---

    /// `[[GetPrototypeOf]]` (§10.1.1).
    pub fn get_prototype_of(&mut self, o: Gc<Object>) -> JsResult<Option<Gc<Object>>> {
        if self.is_proxy(o) {
            return self.proxy_get_prototype_of(o);
        }
        Ok(self.heap[o].proto)
    }

    /// `[[SetPrototypeOf]]` (§10.1.2, OrdinarySetPrototypeOf; %Object.prototype% es
    /// inmutable, §10.4.7).
    pub fn set_prototype_of(&mut self, o: Gc<Object>, v: Option<Gc<Object>>) -> JsResult<bool> {
        if self.is_proxy(o) {
            return self.proxy_set_prototype_of(o, v);
        }
        let current = self.heap[o].proto;
        if current == v {
            return Ok(true);
        }
        if o == self.intrinsics().object_prototype
            || self
                .realms
                .iter()
                .any(|r| r.intrinsics.object_prototype == o)
        {
            return Ok(false);
        }
        if !self.heap[o].extensible {
            return Ok(false);
        }
        let mut p = v;
        while let Some(pp) = p {
            if pp == o {
                return Ok(false);
            }
            // Un Proxy en la cadena corta el chequeo (§10.1.2.1 paso 8.c.i).
            if self.is_proxy(pp) {
                break;
            }
            p = self.heap[pp].proto;
        }
        self.heap[o].proto = v;
        Ok(true)
    }

    /// `[[IsExtensible]]` (§10.1.3).
    pub fn is_extensible(&mut self, o: Gc<Object>) -> JsResult<bool> {
        if self.is_proxy(o) {
            return self.proxy_is_extensible(o);
        }
        Ok(self.heap[o].extensible)
    }

    /// `[[PreventExtensions]]` (§10.1.4).
    pub fn prevent_extensions(&mut self, o: Gc<Object>) -> JsResult<bool> {
        if !crate::builtins::typed_array::can_prevent_extensions(self, o) {
            return Ok(false); // E3: TypedArray [[PreventExtensions]] (§10.4.5.1)
        }
        if self.host_exotic(o).is_some() {
            return Ok(false); // E3: objeto de plataforma legacy (WebIDL §3.9.4)
        }
        if self.is_proxy(o) {
            return self.proxy_prevent_extensions(o);
        }
        // Los elementos densos pasan al almacén general (el vector supone extensibilidad).
        self.array_make_sparse(o);
        self.heap[o].extensible = false;
        Ok(true)
    }

    // --- [[GetOwnProperty]] ---

    /// `[[GetOwnProperty]]` (§10.1.5 y exóticos).
    pub fn get_own_property(
        &mut self,
        o: Gc<Object>,
        key: &PropertyKey,
    ) -> JsResult<Option<PropertyDescriptor>> {
        if let Some(i) = self.ta_numeric_key(o, key) {
            return Ok(crate::builtins::typed_array::get_own_element(self, o, i)); // E3
        }
        if let Some(ex) = self.host_exotic(o) {
            return crate::host::exotic::get_own(self, o, key, ex, false); // E3
        }
        if self.is_proxy(o) {
            return self.proxy_get_own_property(o, key);
        }
        if !key.is_symbol() && self.is_module_namespace(o) {
            return self.namespace_get_own_property(o, key);
        }
        Ok(self.own_prop(o, key))
    }

    /// La propiedad propia sin efectos (todos los exóticos de esta etapa son puros).
    pub fn own_prop(&self, o: Gc<Object>, key: &PropertyKey) -> Option<PropertyDescriptor> {
        let obj = &self.heap[o];
        match &obj.kind {
            ObjectKind::Array(a) => {
                if let PropertyKey::Index(i) = key
                    && let Some(Some(v)) = a.dense.get(*i as usize)
                {
                    return Some(PropertyDescriptor::data(v.clone(), true, true, true));
                }
                if *key == self.keys.length {
                    return Some(PropertyDescriptor::data(
                        Value::Number(f64::from(a.length)),
                        a.length_writable,
                        false,
                        false,
                    ));
                }
            }
            ObjectKind::String(s) => {
                // StringGetOwnProperty (§10.4.3.5).
                if let PropertyKey::Index(i) = key
                    && (*i as usize) < s.len()
                {
                    let c = JsString::from_units(vec![s.units()[*i as usize]]);
                    return Some(PropertyDescriptor::data(
                        Value::String(c),
                        false,
                        true,
                        false,
                    ));
                }
            }
            ObjectKind::Arguments(Some(m)) => {
                // §10.4.4.1: el valor de un índice mapeado sale del environment.
                if let PropertyKey::Index(i) = key
                    && let Some(Some(slot)) = m.map.get(*i as usize)
                    && let Some(p) = obj.props.get(key)
                {
                    let mut d = p.to_descriptor();
                    let v = self.heap[m.env]
                        .decl()
                        .and_then(|e| e.slots[*slot as usize].clone())
                        .unwrap_or_default();
                    d.value = Some(v);
                    return Some(d);
                }
            }
            _ => {}
        }
        obj.props.get(key).map(Prop::to_descriptor)
    }

    // --- [[DefineOwnProperty]] ---

    /// `[[DefineOwnProperty]]` (§10.1.6 y exóticos).
    pub fn define_own_property(
        &mut self,
        o: Gc<Object>,
        key: PropertyKey,
        desc: PropertyDescriptor,
    ) -> JsResult<bool> {
        match &self.heap[o].kind {
            ObjectKind::Proxy(_) => self.proxy_define_own_property(o, key, desc),
            ObjectKind::Host(_) if self.host_exotic(o).is_some() => {
                let ex = self.host_exotic(o).expect("exótico");
                crate::host::exotic::define_own_property(self, o, key, desc, ex) // E3
            }
            ObjectKind::ModuleNamespace(_) if !key.is_symbol() => {
                self.namespace_define_own_property(o, &key, &desc)
            }
            ObjectKind::Array(_) => self.array_define_own_property(o, key, desc),
            ObjectKind::String(s) => {
                // §10.4.3.2
                if let PropertyKey::Index(i) = key
                    && (i as usize) < s.len()
                {
                    let current = self.own_prop(o, &key);
                    return Ok(is_compatible_property_descriptor(
                        false,
                        &desc,
                        current.as_ref(),
                    ));
                }
                Ok(self.ordinary_define_own_property(o, key, desc))
            }
            ObjectKind::Arguments(Some(_)) => self.arguments_define_own_property(o, key, desc),
            ObjectKind::TypedArray(_) => {
                crate::builtins::typed_array::define_own_property(self, o, key, desc) // E3
            }
            _ => Ok(self.ordinary_define_own_property(o, key, desc)),
        }
    }

    /// OrdinaryDefineOwnProperty (§10.1.6.1).
    pub fn ordinary_define_own_property(
        &mut self,
        o: Gc<Object>,
        key: PropertyKey,
        desc: PropertyDescriptor,
    ) -> bool {
        let current = self.own_prop(o, &key);
        let extensible = self.heap[o].extensible;
        self.validate_and_apply(Some(o), key, extensible, desc, current)
    }

    /// ValidateAndApplyPropertyDescriptor (§10.1.6.3).
    pub fn validate_and_apply(
        &mut self,
        o: Option<Gc<Object>>,
        key: PropertyKey,
        extensible: bool,
        desc: PropertyDescriptor,
        current: Option<PropertyDescriptor>,
    ) -> bool {
        let Some(current) = current else {
            if !extensible {
                return false;
            }
            if let Some(o) = o {
                let prop = desc.to_prop();
                self.store_prop(o, key, prop);
            }
            return true;
        };
        if desc.is_generic() && desc.enumerable.is_none() && desc.configurable.is_none() {
            return true;
        }
        if current.configurable == Some(false) {
            if desc.configurable == Some(true) {
                return false;
            }
            if let Some(e) = desc.enumerable
                && Some(e) != current.enumerable
            {
                return false;
            }
            if !desc.is_generic() && desc.is_accessor() != current.is_accessor() {
                return false;
            }
            if current.is_accessor() {
                if let Some(g) = desc.get
                    && Some(g) != current.get
                {
                    return false;
                }
                if let Some(s) = desc.set
                    && Some(s) != current.set
                {
                    return false;
                }
            } else if current.writable == Some(false) {
                if desc.writable == Some(true) {
                    return false;
                }
                if let Some(v) = &desc.value
                    && !self.same_value(v, current.value.as_ref().expect("valor"))
                {
                    return false;
                }
            }
        }
        if let Some(o) = o {
            let new = if current.is_data() && desc.is_accessor() {
                Prop {
                    value: PropValue::Accessor {
                        get: desc.get.flatten(),
                        set: desc.set.flatten(),
                    },
                    enumerable: desc.enumerable.or(current.enumerable).unwrap_or(false),
                    configurable: desc.configurable.or(current.configurable).unwrap_or(false),
                }
            } else if current.is_accessor() && desc.is_data() {
                Prop {
                    value: PropValue::Data {
                        value: desc.value.clone().unwrap_or_default(),
                        writable: desc.writable.unwrap_or(false),
                    },
                    enumerable: desc.enumerable.or(current.enumerable).unwrap_or(false),
                    configurable: desc.configurable.or(current.configurable).unwrap_or(false),
                }
            } else if current.is_accessor() {
                Prop {
                    value: PropValue::Accessor {
                        get: desc.get.unwrap_or(current.get.flatten()),
                        set: desc.set.unwrap_or(current.set.flatten()),
                    },
                    enumerable: desc.enumerable.or(current.enumerable).unwrap_or(false),
                    configurable: desc.configurable.or(current.configurable).unwrap_or(false),
                }
            } else {
                Prop {
                    value: PropValue::Data {
                        value: desc
                            .value
                            .clone()
                            .or(current.value.clone())
                            .unwrap_or_default(),
                        writable: desc.writable.or(current.writable).unwrap_or(false),
                    },
                    enumerable: desc.enumerable.or(current.enumerable).unwrap_or(false),
                    configurable: desc.configurable.or(current.configurable).unwrap_or(false),
                }
            };
            self.store_prop(o, key, new);
        }
        true
    }

    /// Guarda una propiedad propia ya validada (en el vector denso de los arrays si se puede).
    fn store_prop(&mut self, o: Gc<Object>, key: PropertyKey, prop: Prop) {
        let obj = &mut self.heap[o];
        if let ObjectKind::Array(a) = &mut obj.kind {
            if let PropertyKey::Index(i) = key {
                let default_attrs = prop.enumerable
                    && prop.configurable
                    && matches!(prop.value, PropValue::Data { writable: true, .. });
                let idx = i as usize;
                if default_attrs && obj.extensible && !obj.props.contains(&key) {
                    if idx < a.dense.len() {
                        if let PropValue::Data { value, .. } = prop.value {
                            a.dense[idx] = Some(value);
                        }
                        return;
                    }
                    if i <= a.dense.len() as u32 + DENSE_GAP
                        && obj.props.index_keys().next_back().is_none_or(|k| k < i)
                    {
                        if let PropValue::Data { value, .. } = prop.value {
                            a.dense.resize(idx, None);
                            a.dense.push(Some(value));
                        }
                        return;
                    }
                }
                // Fuera del vector denso.
                if idx < a.dense.len() {
                    a.dense[idx] = None;
                }
            } else if key == self.keys.length {
                if let PropValue::Data {
                    value: Value::Number(n),
                    writable,
                } = prop.value
                {
                    a.length = n as u32;
                    a.length_writable = writable;
                }
                return;
            }
        }
        obj.props.insert(key, prop);
    }

    /// Pasa los elementos densos de un array al almacén general.
    fn array_make_sparse(&mut self, o: Gc<Object>) {
        let obj = &mut self.heap[o];
        if let ObjectKind::Array(a) = &mut obj.kind {
            let dense = std::mem::take(&mut a.dense);
            for (i, v) in dense.into_iter().enumerate() {
                if let Some(v) = v {
                    obj.props.insert(
                        PropertyKey::Index(i as u32),
                        Prop::data(v, true, true, true),
                    );
                }
            }
        }
    }

    // --- [[HasProperty]], [[Get]], [[Set]], [[Delete]] ---

    /// `[[HasProperty]]` (§10.1.7, OrdinaryHasProperty).
    pub fn has_property(&mut self, o: Gc<Object>, key: &PropertyKey) -> JsResult<bool> {
        let mut cur = o;
        loop {
            if let Some(i) = self.ta_numeric_key(cur, key) {
                return Ok(crate::builtins::typed_array::is_valid_integer_index(
                    self, cur, i,
                )); // E3
            }
            if self.is_proxy(cur) {
                return self.proxy_has(cur, key);
            }
            if let Some(ex) = self.host_exotic(cur) {
                // E3: objeto de plataforma legacy (WebIDL §3.9).
                if crate::host::exotic::get_own(self, cur, key, ex, false)?.is_some() {
                    return Ok(true);
                }
                match self.get_prototype_of(cur)? {
                    Some(p) => {
                        cur = p;
                        continue;
                    }
                    None => return Ok(false),
                }
            }
            if !key.is_symbol() && self.is_module_namespace(cur) {
                return Ok(self.namespace_has(cur, key));
            }
            if self.own_prop(cur, key).is_some() {
                return Ok(true);
            }
            match self.get_prototype_of(cur)? {
                Some(p) => cur = p,
                None => return Ok(false),
            }
        }
    }

    /// `[[Get]]` (§10.1.8, OrdinaryGet).
    pub fn get(&mut self, o: Gc<Object>, key: &PropertyKey, receiver: Value) -> JsResult<Value> {
        let mut cur = o;
        loop {
            if let Some(i) = self.ta_numeric_key(cur, key) {
                return Ok(crate::builtins::typed_array::get_element(self, cur, i)); // E3
            }
            if self.is_proxy(cur) {
                return self.proxy_get(cur, key, receiver);
            }
            if let Some(ex) = self.host_exotic(cur) {
                // E3: objeto de plataforma legacy (WebIDL §3.9).
                if let Some(v) = crate::host::exotic::get_here(self, cur, key, &receiver, ex)? {
                    return Ok(v);
                }
                match self.get_prototype_of(cur)? {
                    Some(p) => {
                        cur = p;
                        continue;
                    }
                    None => return Ok(Value::Undefined),
                }
            }
            if !key.is_symbol() && self.is_module_namespace(cur) {
                return self.namespace_get(cur, key);
            }
            match self.own_prop(cur, key) {
                Some(d) => {
                    if d.is_data() {
                        return Ok(d.value.unwrap_or_default());
                    }
                    return match d.get.flatten() {
                        Some(g) => self.call(&Value::Object(g), receiver, &[]),
                        None => Ok(Value::Undefined),
                    };
                }
                None => match self.get_prototype_of(cur)? {
                    Some(p) => cur = p,
                    None => return Ok(Value::Undefined),
                },
            }
        }
    }

    /// `[[Set]]` (§10.1.9, OrdinarySet).
    pub fn set(
        &mut self,
        o: Gc<Object>,
        key: PropertyKey,
        v: Value,
        receiver: Value,
    ) -> JsResult<bool> {
        if let Some(i) = self.ta_numeric_key(o, &key) {
            return crate::builtins::typed_array::set(self, o, i, key, v, receiver); // E3
        }
        // Camino rápido: escritura de un elemento denso propio con receiver = o.
        if let Value::Object(r) = &receiver
            && *r == o
            && let PropertyKey::Index(i) = key
            && let ObjectKind::Array(a) = &mut self.heap[o].kind
            && let Some(slot @ Some(_)) = a.dense.get_mut(i as usize)
        {
            *slot = Some(v);
            return Ok(true);
        }
        if self.is_proxy(o) {
            return self.proxy_set(o, key, v, receiver);
        }
        if let Some(ex) = self.host_exotic(o) {
            // E3: LegacyPlatformObjectGetOwnProperty(O, P, true) (WebIDL §3.9.2).
            let own = crate::host::exotic::get_own(self, o, &key, ex, true)?;
            return self.ordinary_set_with_own_descriptor(o, key, v, receiver, own);
        }
        // §10.4.6.9: el namespace no se escribe.
        if self.is_module_namespace(o) {
            return Ok(false);
        }
        let own = self.own_prop(o, &key);
        self.ordinary_set_with_own_descriptor(o, key, v, receiver, own)
    }

    /// OrdinarySetWithOwnDescriptor (§10.1.9.2).
    pub fn ordinary_set_with_own_descriptor(
        &mut self,
        o: Gc<Object>,
        key: PropertyKey,
        v: Value,
        receiver: Value,
        own: Option<PropertyDescriptor>,
    ) -> JsResult<bool> {
        let own = match own {
            Some(d) => d,
            None => match self.get_prototype_of(o)? {
                Some(parent) => return self.set(parent, key, v, receiver),
                None => PropertyDescriptor::data(Value::Undefined, true, true, true),
            },
        };
        if own.is_data() {
            if own.writable == Some(false) {
                return Ok(false);
            }
            let Value::Object(r) = receiver else {
                return Ok(false);
            };
            let existing = self.get_own_property(r, &key)?;
            if let Some(e) = existing {
                if e.is_accessor() || e.writable == Some(false) {
                    return Ok(false);
                }
                return self.define_own_property(
                    r,
                    key,
                    PropertyDescriptor {
                        value: Some(v),
                        ..PropertyDescriptor::default()
                    },
                );
            }
            return self.create_data_property(r, key, v);
        }
        match own.set.flatten() {
            Some(s) => {
                self.call(&Value::Object(s), receiver, &[v])?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// `[[Delete]]` (§10.1.10, OrdinaryDelete; arguments §10.4.4.5).
    pub fn delete(&mut self, o: Gc<Object>, key: &PropertyKey) -> JsResult<bool> {
        if let Some(i) = self.ta_numeric_key(o, key) {
            return Ok(!crate::builtins::typed_array::is_valid_integer_index(
                self, o, i,
            )); // E3
        }
        if self.is_proxy(o) {
            return self.proxy_delete(o, key);
        }
        if let Some(ex) = self.host_exotic(o)
            && let Some(r) = crate::host::exotic::delete(self, o, key, ex)?
        {
            return Ok(r); // E3
        }
        if !key.is_symbol() && self.is_module_namespace(o) {
            return Ok(self.namespace_delete(o, key));
        }
        let Some(d) = self.own_prop(o, key) else {
            return Ok(true);
        };
        if d.configurable != Some(true) {
            return Ok(false);
        }
        let obj = &mut self.heap[o];
        match &mut obj.kind {
            ObjectKind::Array(a) => {
                if let PropertyKey::Index(i) = key
                    && let Some(slot) = a.dense.get_mut(*i as usize)
                    && slot.is_some()
                {
                    *slot = None;
                    if *i as usize + 1 == a.dense.len() {
                        while matches!(a.dense.last(), Some(None)) {
                            a.dense.pop();
                        }
                    }
                    return Ok(true);
                }
            }
            ObjectKind::Arguments(Some(m)) => {
                if let PropertyKey::Index(i) = key
                    && let Some(e) = m.map.get_mut(*i as usize)
                {
                    *e = None;
                }
            }
            _ => {}
        }
        obj.props.remove(key);
        Ok(true)
    }

    /// `[[OwnPropertyKeys]]` (§10.1.11, OrdinaryOwnPropertyKeys; Array y String).
    pub fn own_property_keys(&mut self, o: Gc<Object>) -> JsResult<Vec<PropertyKey>> {
        if let ObjectKind::TypedArray(_) = self.heap[o].kind {
            return Ok(crate::builtins::typed_array::own_keys(self, o)); // E3
        }
        if self.is_proxy(o) {
            return self.proxy_own_keys(o);
        }
        if let Some(ex) = self.host_exotic(o) {
            return crate::host::exotic::own_keys(self, o, ex); // E3
        }
        if self.is_module_namespace(o) {
            return Ok(self.namespace_own_keys(o));
        }
        let obj = &self.heap[o];
        let mut out = Vec::new();
        match &obj.kind {
            ObjectKind::Array(a) => {
                // Los índices densos y los del almacén general, en orden.
                let mut idx: Vec<u32> = a
                    .dense
                    .iter()
                    .enumerate()
                    .filter(|(_, v)| v.is_some())
                    .map(|(i, _)| i as u32)
                    .collect();
                idx.extend(obj.props.index_keys());
                idx.sort_unstable();
                out.extend(idx.into_iter().map(PropertyKey::Index));
                out.push(self.keys.length.clone());
                obj.props.push_named_keys(&mut out);
                return Ok(out);
            }
            ObjectKind::String(s) => {
                let n = s.len() as u32;
                out.extend((0..n).map(PropertyKey::Index));
                out.extend(
                    obj.props
                        .index_keys()
                        .filter(|&i| i >= n)
                        .map(PropertyKey::Index),
                );
                obj.props.push_named_keys(&mut out);
                return Ok(out);
            }
            _ => {}
        }
        Ok(obj.props.keys())
    }

    // --- Array exótico (§10.4.2) ---

    /// ArrayCreate (§10.4.2.2).
    pub fn array_create(&mut self, length: u64, proto: Option<Gc<Object>>) -> JsResult<Gc<Object>> {
        if length > 4294967295 {
            return self.range_error("Invalid array length");
        }
        let proto = proto.unwrap_or(self.intrinsics().array_prototype);
        Ok(self.heap.alloc(Object::new(
            Some(proto),
            ObjectKind::Array(ArrayData {
                dense: Vec::new(),
                length: length as u32,
                length_writable: true,
            }),
        )))
    }

    /// `[[DefineOwnProperty]]` de un Array (§10.4.2.1).
    fn array_define_own_property(
        &mut self,
        o: Gc<Object>,
        key: PropertyKey,
        desc: PropertyDescriptor,
    ) -> JsResult<bool> {
        if key == self.keys.length {
            return self.array_set_length(o, desc);
        }
        if let PropertyKey::Index(index) = key {
            let (length, length_writable) = match &self.heap[o].kind {
                ObjectKind::Array(a) => (a.length, a.length_writable),
                _ => unreachable!("array"),
            };
            if index >= length && !length_writable {
                return Ok(false);
            }
            if !self.ordinary_define_own_property(o, key, desc) {
                return Ok(false);
            }
            if index >= length
                && let ObjectKind::Array(a) = &mut self.heap[o].kind
            {
                a.length = index + 1;
            }
            return Ok(true);
        }
        Ok(self.ordinary_define_own_property(o, key, desc))
    }

    /// ArraySetLength (§10.4.2.4).
    fn array_set_length(&mut self, o: Gc<Object>, desc: PropertyDescriptor) -> JsResult<bool> {
        let Some(v) = desc.value.clone() else {
            let current = self.own_prop(o, &self.keys.length.clone());
            let extensible = self.heap[o].extensible;
            let k = self.keys.length.clone();
            return Ok(self.validate_and_apply(Some(o), k, extensible, desc, current));
        };
        let new_len = self.to_uint32(&v)?;
        let number_len = self.to_number(&v)?;
        if f64::from(new_len) != number_len {
            return self.range_error("Invalid array length");
        }
        let (old_len, writable) = match &self.heap[o].kind {
            ObjectKind::Array(a) => (a.length, a.length_writable),
            _ => unreachable!("array"),
        };
        let mut new_desc = desc.clone();
        new_desc.value = Some(Value::Number(f64::from(new_len)));
        let k = self.keys.length.clone();
        let current = self.own_prop(o, &k);
        if new_len >= old_len {
            let extensible = self.heap[o].extensible;
            return Ok(self.validate_and_apply(Some(o), k, extensible, new_desc, current));
        }
        if !writable {
            return Ok(false);
        }
        let new_writable = new_desc.writable != Some(false);
        new_desc.writable = Some(true);
        let extensible = self.heap[o].extensible;
        if !self.validate_and_apply(Some(o), k.clone(), extensible, new_desc, current) {
            return Ok(false);
        }
        // Borra los elementos desde el final hasta el primero no configurable.
        let obj = &mut self.heap[o];
        let sparse = obj.props.index_keys_from_desc(new_len);
        let stop = sparse
            .iter()
            .find(|&&i| {
                obj.props
                    .get(&PropertyKey::Index(i))
                    .is_some_and(|p| !p.configurable)
            })
            .map_or(new_len, |&i| i + 1);
        let final_len = stop;
        if let ObjectKind::Array(a) = &mut obj.kind
            && a.dense.len() > stop as usize
        {
            a.dense.truncate(stop as usize);
        }
        for i in sparse {
            if i < stop {
                break;
            }
            obj.props.remove(&PropertyKey::Index(i));
        }
        if let ObjectKind::Array(a) = &mut obj.kind {
            a.length = final_len;
            if !new_writable {
                a.length_writable = false;
            }
        }
        if final_len != new_len {
            return Ok(false);
        }
        Ok(true)
    }

    /// ArraySpeciesCreate (§10.4.2.3).
    pub fn array_species_create(
        &mut self,
        original: Gc<Object>,
        length: f64,
    ) -> JsResult<Gc<Object>> {
        if !self.is_array(&Value::Object(original))? {
            return self.array_create(length as u64, None);
        }
        let k = self.keys.constructor.clone();
        let mut c = self.get(original, &k, Value::Object(original))?;
        if self.is_constructor(&c) {
            let co = c.as_object().expect("constructor");
            let this_realm = self.current_realm;
            let realm_c = self.get_function_realm(co)?;
            if this_realm != realm_c && co == self.intrinsics_of(realm_c).array_ctor {
                c = Value::Undefined;
            }
        }
        if let Value::Object(co) = c {
            let sk = PropertyKey::Symbol(self.well_known.species);
            c = self.get(co, &sk, Value::Object(co))?;
            if c.is_null() {
                c = Value::Undefined;
            }
        }
        if c.is_undefined() {
            return self.array_create(length as u64, None);
        }
        if !self.is_constructor(&c) {
            return self.type_error("object.constructor[Symbol.species] is not a constructor");
        }
        let r = self.construct(
            c.as_object().expect("constructor"),
            &[Value::Number(length)],
            None,
        )?;
        match r {
            Value::Object(o) => Ok(o),
            _ => self.type_error("species constructor did not return an object"),
        }
    }

    // --- arguments (§10.4.4) ---

    /// CreateUnmappedArgumentsObject (§10.4.4.6).
    pub fn create_unmapped_arguments(&mut self) -> Gc<Object> {
        let proto = self.intrinsics().object_prototype;
        let args = self.frames.last().expect("frame").args.clone();
        let o = self
            .heap
            .alloc(Object::new(Some(proto), ObjectKind::Arguments(None)));
        let k = self.keys.length.clone();
        self.put_data(o, k, Value::Number(args.len() as f64), true, false, true);
        for (i, a) in args.into_iter().enumerate() {
            self.put_data(o, PropertyKey::Index(i as u32), a, true, true, true);
        }
        let sym = PropertyKey::Symbol(self.well_known.iterator);
        let values = self.intrinsics().array_proto_values;
        self.put_data(o, sym, Value::Object(values), true, false, true);
        let thrower = self.intrinsics().throw_type_error;
        let k = self.keys.callee.clone();
        self.heap[o].props.insert(
            k,
            Prop {
                value: PropValue::Accessor {
                    get: Some(thrower),
                    set: Some(thrower),
                },
                enumerable: false,
                configurable: false,
            },
        );
        o
    }

    /// CreateMappedArgumentsObject (§10.4.4.7): `map[i]` es el slot del parámetro en el
    /// environment actual.
    pub fn create_mapped_arguments(&mut self, map: &[Option<u32>]) -> Gc<Object> {
        let proto = self.intrinsics().object_prototype;
        let f = self.frames.last().expect("frame");
        let args = f.args.clone();
        let env = f.env;
        let callee = f.callee;
        let mut m: Vec<Option<u32>> = map.to_vec();
        m.truncate(args.len());
        let o = self.heap.alloc(Object::new(
            Some(proto),
            ObjectKind::Arguments(Some(Box::new(MappedArguments { env, map: m }))),
        ));
        for (i, a) in args.iter().enumerate() {
            self.put_data(o, PropertyKey::Index(i as u32), a.clone(), true, true, true);
        }
        let k = self.keys.length.clone();
        self.put_data(o, k, Value::Number(args.len() as f64), true, false, true);
        let sym = PropertyKey::Symbol(self.well_known.iterator);
        let values = self.intrinsics().array_proto_values;
        self.put_data(o, sym, Value::Object(values), true, false, true);
        let k = self.keys.callee.clone();
        self.put_data(o, k, callee.into(), true, false, true);
        o
    }

    /// `[[DefineOwnProperty]]` de un arguments mapeado (§10.4.4.2).
    fn arguments_define_own_property(
        &mut self,
        o: Gc<Object>,
        key: PropertyKey,
        desc: PropertyDescriptor,
    ) -> JsResult<bool> {
        let mapped = self.mapped_slot(o, &key);
        let mut new_desc = desc.clone();
        if let Some(slot) = mapped
            && desc.is_data()
            && desc.value.is_none()
            && desc.writable == Some(false)
        {
            new_desc.value = Some(self.read_mapped(o, slot));
        }
        if !self.ordinary_define_own_property(o, key.clone(), new_desc) {
            return Ok(false);
        }
        if let Some(slot) = mapped {
            if desc.is_accessor() {
                self.unmap(o, &key);
            } else {
                if let Some(v) = desc.value {
                    self.write_mapped(o, slot, v);
                }
                if desc.writable == Some(false) {
                    self.unmap(o, &key);
                }
            }
        }
        Ok(true)
    }

    fn mapped_slot(&self, o: Gc<Object>, key: &PropertyKey) -> Option<u32> {
        match (&self.heap[o].kind, key) {
            (ObjectKind::Arguments(Some(m)), PropertyKey::Index(i)) => {
                m.map.get(*i as usize).copied().flatten()
            }
            _ => None,
        }
    }

    fn read_mapped(&self, o: Gc<Object>, slot: u32) -> Value {
        match &self.heap[o].kind {
            ObjectKind::Arguments(Some(m)) => self.heap[m.env]
                .decl()
                .and_then(|e| e.slots[slot as usize].clone())
                .unwrap_or_default(),
            _ => Value::Undefined,
        }
    }

    fn write_mapped(&mut self, o: Gc<Object>, slot: u32, v: Value) {
        let env = match &self.heap[o].kind {
            ObjectKind::Arguments(Some(m)) => m.env,
            _ => return,
        };
        if let Some(d) = self.heap[env].decl_mut() {
            d.slots[slot as usize] = Some(v);
        }
    }

    fn unmap(&mut self, o: Gc<Object>, key: &PropertyKey) {
        if let (ObjectKind::Arguments(Some(m)), PropertyKey::Index(i)) =
            (&mut self.heap[o].kind, key)
            && let Some(e) = m.map.get_mut(*i as usize)
        {
            *e = None;
        }
    }

    /// `[[Set]]` de un arguments mapeado escribe también el parámetro (§10.4.4.4); lo hace
    /// `[[DefineOwnProperty]]` vía OrdinarySet, así que no hace falta un caso propio.
    #[allow(dead_code)]
    fn arguments_set_note(&self) {}
}

/// IsCompatiblePropertyDescriptor (§10.1.6.2).
pub fn is_compatible_property_descriptor(
    extensible: bool,
    desc: &PropertyDescriptor,
    current: Option<&PropertyDescriptor>,
) -> bool {
    let Some(c) = current else { return extensible };
    if desc.is_generic() && desc.enumerable.is_none() && desc.configurable.is_none() {
        return true;
    }
    if c.configurable == Some(false) {
        if desc.configurable == Some(true) {
            return false;
        }
        if desc.enumerable.is_some() && desc.enumerable != c.enumerable {
            return false;
        }
        if !desc.is_generic() && desc.is_accessor() != c.is_accessor() {
            return false;
        }
        if c.is_data() && c.writable == Some(false) {
            if desc.writable == Some(true) {
                return false;
            }
            if let (Some(v), Some(cv)) = (&desc.value, &c.value) {
                let same = match (v, cv) {
                    (Value::String(a), Value::String(b)) => a == b,
                    _ => false,
                };
                if !same {
                    return false;
                }
            }
        }
    }
    true
}
