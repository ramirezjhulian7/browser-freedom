//! Proxy exótico (§10.5): los 13 métodos internos llaman al trap del handler y verifican sus
//! invariantes contra el target; sin trap, delegan en el target.

use crate::abstract_ops::conversion::to_boolean;
use crate::heap::Gc;
use crate::object::{Object, ObjectKind, PropertyDescriptor};
use crate::value::{PropertyKey, Value};
use crate::vm::{Agent, JsResult};

/// `[[ProxyTarget]]` y `[[ProxyHandler]]` (`None` cuando se revocó).
#[derive(Debug, Clone)]
pub struct ProxyData {
    pub target: Option<Gc<Object>>,
    pub handler: Option<Gc<Object>>,
    pub callable: bool,
    pub constructor: bool,
}

impl Agent {
    pub fn is_proxy(&self, o: Gc<Object>) -> bool {
        matches!(self.heap[o].kind, ObjectKind::Proxy(_))
    }

    /// ProxyCreate (§10.5.14).
    pub fn proxy_create(&mut self, target: &Value, handler: &Value) -> JsResult<Gc<Object>> {
        let (Value::Object(t), Value::Object(h)) = (target, handler) else {
            return self.type_error("Cannot create proxy with a non-object as target or handler");
        };
        let callable = self.heap[*t].is_callable();
        let constructor = self.heap[*t].is_constructor();
        Ok(self.alloc(Object::new(
            None,
            ObjectKind::Proxy(Box::new(ProxyData {
                target: Some(*t),
                handler: Some(*h),
                callable,
                constructor,
            })),
        )))
    }

    /// ValidateNonRevokedProxy (§10.5.15): (target, handler).
    fn proxy_parts(&mut self, p: Gc<Object>) -> JsResult<(Gc<Object>, Gc<Object>)> {
        match &self.heap[p].kind {
            ObjectKind::Proxy(d) => match (d.target, d.handler) {
                (Some(t), Some(h)) => Ok((t, h)),
                _ => self.type_error("Cannot perform operation on a revoked proxy"),
            },
            _ => unreachable!("proxy"),
        }
    }

    /// El trap `name` del handler (GetMethod).
    fn trap(&mut self, handler: Gc<Object>, name: &str) -> JsResult<Option<Value>> {
        self.get_method_of(&Value::Object(handler), &PropertyKey::from(name))
    }

    /// IsCompatiblePropertyDescriptor (§10.1.6.2).
    fn compatible(
        &mut self,
        extensible: bool,
        desc: &PropertyDescriptor,
        current: Option<PropertyDescriptor>,
    ) -> bool {
        self.validate_and_apply(
            None,
            PropertyKey::from(""),
            extensible,
            desc.clone(),
            current,
        )
    }

    /// `[[GetPrototypeOf]]` (§10.5.1).
    pub fn proxy_get_prototype_of(&mut self, p: Gc<Object>) -> JsResult<Option<Gc<Object>>> {
        let (target, handler) = self.proxy_parts(p)?;
        let Some(trap) = self.trap(handler, "getPrototypeOf")? else {
            return self.get_prototype_of(target);
        };
        let r = self.call(&trap, Value::Object(handler), &[Value::Object(target)])?;
        let proto = match r {
            Value::Object(o) => Some(o),
            Value::Null => None,
            _ => {
                return self.type_error(
                    "'getPrototypeOf' on proxy: trap returned neither object nor null",
                );
            }
        };
        if self.is_extensible(target)? {
            return Ok(proto);
        }
        if self.get_prototype_of(target)? != proto {
            return self.type_error("'getPrototypeOf' on proxy: proxy target is non-extensible but the trap did not return its actual prototype");
        }
        Ok(proto)
    }

    /// `[[SetPrototypeOf]]` (§10.5.2).
    pub fn proxy_set_prototype_of(
        &mut self,
        p: Gc<Object>,
        v: Option<Gc<Object>>,
    ) -> JsResult<bool> {
        let (target, handler) = self.proxy_parts(p)?;
        let Some(trap) = self.trap(handler, "setPrototypeOf")? else {
            return self.set_prototype_of(target, v);
        };
        let r = self.call(
            &trap,
            Value::Object(handler),
            &[Value::Object(target), v.map_or(Value::Null, Value::Object)],
        )?;
        if !to_boolean(self, &r) {
            return Ok(false);
        }
        if self.is_extensible(target)? {
            return Ok(true);
        }
        if self.get_prototype_of(target)? != v {
            return self.type_error("'setPrototypeOf' on proxy: trap returned truish for setting a new prototype on the non-extensible proxy target");
        }
        Ok(true)
    }

    /// `[[IsExtensible]]` (§10.5.3).
    pub fn proxy_is_extensible(&mut self, p: Gc<Object>) -> JsResult<bool> {
        let (target, handler) = self.proxy_parts(p)?;
        let Some(trap) = self.trap(handler, "isExtensible")? else {
            return self.is_extensible(target);
        };
        let r = self.call(&trap, Value::Object(handler), &[Value::Object(target)])?;
        let b = to_boolean(self, &r);
        if b != self.is_extensible(target)? {
            return self.type_error("'isExtensible' on proxy: trap result does not reflect extensibility of proxy target");
        }
        Ok(b)
    }

    /// `[[PreventExtensions]]` (§10.5.4).
    pub fn proxy_prevent_extensions(&mut self, p: Gc<Object>) -> JsResult<bool> {
        let (target, handler) = self.proxy_parts(p)?;
        let Some(trap) = self.trap(handler, "preventExtensions")? else {
            return self.prevent_extensions(target);
        };
        let r = self.call(&trap, Value::Object(handler), &[Value::Object(target)])?;
        let b = to_boolean(self, &r);
        if b && self.is_extensible(target)? {
            return self.type_error("'preventExtensions' on proxy: trap returned truish but the proxy target is extensible");
        }
        Ok(b)
    }

    /// `[[GetOwnProperty]]` (§10.5.5).
    pub fn proxy_get_own_property(
        &mut self,
        p: Gc<Object>,
        key: &PropertyKey,
    ) -> JsResult<Option<PropertyDescriptor>> {
        let (target, handler) = self.proxy_parts(p)?;
        let Some(trap) = self.trap(handler, "getOwnPropertyDescriptor")? else {
            return self.get_own_property(target, key);
        };
        let r = self.call(
            &trap,
            Value::Object(handler),
            &[Value::Object(target), key.to_value()],
        )?;
        if !matches!(r, Value::Object(_) | Value::Undefined) {
            return self.type_error(
                "'getOwnPropertyDescriptor' on proxy: trap returned neither object nor undefined",
            );
        }
        let target_desc = self.get_own_property(target, key)?;
        if r.is_undefined() {
            let Some(td) = target_desc else {
                return Ok(None);
            };
            if td.configurable == Some(false) {
                return self.type_error("'getOwnPropertyDescriptor' on proxy: trap returned undefined for a non-configurable property");
            }
            if !self.is_extensible(target)? {
                return self.type_error("'getOwnPropertyDescriptor' on proxy: trap returned undefined for a property of a non-extensible target");
            }
            return Ok(None);
        }
        let extensible = self.is_extensible(target)?;
        let mut desc = self.to_property_descriptor(&r)?;
        // CompletePropertyDescriptor (§6.2.6.6).
        if desc.is_generic() || desc.is_data() {
            desc.value.get_or_insert(Value::Undefined);
            desc.writable.get_or_insert(false);
        } else {
            desc.get.get_or_insert(None);
            desc.set.get_or_insert(None);
        }
        desc.enumerable.get_or_insert(false);
        desc.configurable.get_or_insert(false);
        if !self.compatible(extensible, &desc, target_desc.clone()) {
            return self.type_error("'getOwnPropertyDescriptor' on proxy: trap returned descriptor incompatible with the target property");
        }
        if desc.configurable == Some(false) {
            match &target_desc {
                None => return self.type_error("'getOwnPropertyDescriptor' on proxy: trap reported non-configurability for a property that does not exist on the target"),
                Some(td) if td.configurable == Some(true) => {
                    return self.type_error("'getOwnPropertyDescriptor' on proxy: trap reported non-configurability for a configurable property");
                }
                Some(td) => {
                    if desc.writable == Some(false) && td.writable == Some(true) {
                        return self.type_error("'getOwnPropertyDescriptor' on proxy: trap reported non-configurable and non-writable for a writable property");
                    }
                }
            }
        }
        Ok(Some(desc))
    }

    /// `[[DefineOwnProperty]]` (§10.5.6).
    pub fn proxy_define_own_property(
        &mut self,
        p: Gc<Object>,
        key: PropertyKey,
        desc: PropertyDescriptor,
    ) -> JsResult<bool> {
        let (target, handler) = self.proxy_parts(p)?;
        let Some(trap) = self.trap(handler, "defineProperty")? else {
            return self.define_own_property(target, key, desc);
        };
        let desc_obj = self.from_property_descriptor(Some(&desc))?;
        let r = self.call(
            &trap,
            Value::Object(handler),
            &[Value::Object(target), key.to_value(), desc_obj],
        )?;
        if !to_boolean(self, &r) {
            return Ok(false);
        }
        let target_desc = self.get_own_property(target, &key)?;
        let extensible = self.is_extensible(target)?;
        let setting_config_false = desc.configurable == Some(false);
        match target_desc {
            None => {
                if !extensible {
                    return self.type_error("'defineProperty' on proxy: trap returned truish for adding a property to a non-extensible target");
                }
                if setting_config_false {
                    return self.type_error("'defineProperty' on proxy: trap returned truish for defining a non-configurable property that does not exist on the target");
                }
            }
            Some(td) => {
                if !self.compatible(extensible, &desc, Some(td.clone())) {
                    return self.type_error("'defineProperty' on proxy: trap returned truish for a property incompatible with the target");
                }
                if setting_config_false && td.configurable == Some(true) {
                    return self.type_error("'defineProperty' on proxy: trap returned truish for defining a non-configurable property that is configurable on the target");
                }
                if td.is_data()
                    && td.configurable == Some(false)
                    && td.writable == Some(true)
                    && desc.writable == Some(false)
                {
                    return self.type_error("'defineProperty' on proxy: trap returned truish for defining a non-writable property that is writable on the target");
                }
            }
        }
        Ok(true)
    }

    /// `[[HasProperty]]` (§10.5.7).
    pub fn proxy_has(&mut self, p: Gc<Object>, key: &PropertyKey) -> JsResult<bool> {
        let (target, handler) = self.proxy_parts(p)?;
        let Some(trap) = self.trap(handler, "has")? else {
            return self.has_property(target, key);
        };
        let r = self.call(
            &trap,
            Value::Object(handler),
            &[Value::Object(target), key.to_value()],
        )?;
        let b = to_boolean(self, &r);
        if !b && let Some(td) = self.get_own_property(target, key)? {
            if td.configurable == Some(false) {
                return self.type_error(
                    "'has' on proxy: trap returned falsish for a non-configurable property",
                );
            }
            if !self.is_extensible(target)? {
                return self.type_error("'has' on proxy: trap returned falsish for a property of a non-extensible target");
            }
        }
        Ok(b)
    }

    /// `[[Get]]` (§10.5.8).
    pub fn proxy_get(
        &mut self,
        p: Gc<Object>,
        key: &PropertyKey,
        receiver: Value,
    ) -> JsResult<Value> {
        let (target, handler) = self.proxy_parts(p)?;
        let Some(trap) = self.trap(handler, "get")? else {
            return self.get(target, key, receiver);
        };
        let r = self.call(
            &trap,
            Value::Object(handler),
            &[Value::Object(target), key.to_value(), receiver],
        )?;
        if let Some(td) = self.get_own_property(target, key)?
            && td.configurable == Some(false)
        {
            if td.is_data()
                && td.writable == Some(false)
                && !self.same_value(&r, td.value.as_ref().expect("valor"))
            {
                return self.type_error("'get' on proxy: trap returned a different value for a non-writable, non-configurable property");
            }
            if td.is_accessor() && td.get == Some(None) && !r.is_undefined() {
                return self.type_error("'get' on proxy: trap returned a value for a non-configurable accessor without getter");
            }
        }
        Ok(r)
    }

    /// `[[Set]]` (§10.5.9).
    pub fn proxy_set(
        &mut self,
        p: Gc<Object>,
        key: PropertyKey,
        v: Value,
        receiver: Value,
    ) -> JsResult<bool> {
        let (target, handler) = self.proxy_parts(p)?;
        let Some(trap) = self.trap(handler, "set")? else {
            return self.set(target, key, v, receiver);
        };
        let r = self.call(
            &trap,
            Value::Object(handler),
            &[Value::Object(target), key.to_value(), v.clone(), receiver],
        )?;
        if !to_boolean(self, &r) {
            return Ok(false);
        }
        if let Some(td) = self.get_own_property(target, &key)?
            && td.configurable == Some(false)
        {
            if td.is_data()
                && td.writable == Some(false)
                && !self.same_value(&v, td.value.as_ref().expect("valor"))
            {
                return self.type_error("'set' on proxy: trap returned truish for a non-writable, non-configurable property with a different value");
            }
            if td.is_accessor() && td.set == Some(None) {
                return self.type_error("'set' on proxy: trap returned truish for a non-configurable accessor without setter");
            }
        }
        Ok(true)
    }

    /// `[[Delete]]` (§10.5.10).
    pub fn proxy_delete(&mut self, p: Gc<Object>, key: &PropertyKey) -> JsResult<bool> {
        let (target, handler) = self.proxy_parts(p)?;
        let Some(trap) = self.trap(handler, "deleteProperty")? else {
            return self.delete(target, key);
        };
        let r = self.call(
            &trap,
            Value::Object(handler),
            &[Value::Object(target), key.to_value()],
        )?;
        if !to_boolean(self, &r) {
            return Ok(false);
        }
        let Some(td) = self.get_own_property(target, key)? else {
            return Ok(true);
        };
        if td.configurable == Some(false) {
            return self.type_error(
                "'deleteProperty' on proxy: trap returned truish for a non-configurable property",
            );
        }
        if !self.is_extensible(target)? {
            return self.type_error("'deleteProperty' on proxy: trap returned truish for a property of a non-extensible target");
        }
        Ok(true)
    }

    /// `[[OwnPropertyKeys]]` (§10.5.11).
    pub fn proxy_own_keys(&mut self, p: Gc<Object>) -> JsResult<Vec<PropertyKey>> {
        let (target, handler) = self.proxy_parts(p)?;
        let Some(trap) = self.trap(handler, "ownKeys")? else {
            return self.own_property_keys(target);
        };
        let r = self.call(&trap, Value::Object(handler), &[Value::Object(target)])?;
        let list = self.create_list_from_array_like(&r, true)?;
        let mut keys: Vec<PropertyKey> = Vec::with_capacity(list.len());
        for v in &list {
            let k = self.to_property_key(v)?;
            if keys.contains(&k) {
                return self.type_error("'ownKeys' on proxy: trap returned duplicate entries");
            }
            keys.push(k);
        }
        let extensible = self.is_extensible(target)?;
        let target_keys = self.own_property_keys(target)?;
        let mut configurable = Vec::new();
        let mut nonconfigurable = Vec::new();
        for k in target_keys {
            let d = self.get_own_property(target, &k)?;
            if d.is_some_and(|d| d.configurable == Some(false)) {
                nonconfigurable.push(k);
            } else {
                configurable.push(k);
            }
        }
        if extensible && nonconfigurable.is_empty() {
            return Ok(keys);
        }
        let mut unchecked = keys.clone();
        for k in nonconfigurable {
            match unchecked.iter().position(|x| *x == k) {
                Some(i) => {
                    unchecked.remove(i);
                }
                None => {
                    return self.type_error(
                        "'ownKeys' on proxy: trap result did not include a non-configurable key",
                    );
                }
            }
        }
        if extensible {
            return Ok(keys);
        }
        for k in configurable {
            match unchecked.iter().position(|x| *x == k) {
                Some(i) => {
                    unchecked.remove(i);
                }
                None => {
                    return self.type_error("'ownKeys' on proxy: trap result did not include a key of the non-extensible target");
                }
            }
        }
        if !unchecked.is_empty() {
            return self.type_error(
                "'ownKeys' on proxy: trap returned extra keys but proxy target is non-extensible",
            );
        }
        Ok(keys)
    }

    /// `[[Call]]` (§10.5.12).
    pub fn proxy_call(&mut self, p: Gc<Object>, this: Value, args: &[Value]) -> JsResult<Value> {
        let (target, handler) = self.proxy_parts(p)?;
        let Some(trap) = self.trap(handler, "apply")? else {
            return self.call(&Value::Object(target), this, args);
        };
        let arr = self.create_array_from_list(args)?;
        self.call(
            &trap,
            Value::Object(handler),
            &[Value::Object(target), this, Value::Object(arr)],
        )
    }

    /// `[[Construct]]` (§10.5.13).
    pub fn proxy_construct(
        &mut self,
        p: Gc<Object>,
        args: &[Value],
        new_target: Gc<Object>,
    ) -> JsResult<Value> {
        let (target, handler) = self.proxy_parts(p)?;
        let Some(trap) = self.trap(handler, "construct")? else {
            return self.construct(target, args, Some(new_target));
        };
        let arr = self.create_array_from_list(args)?;
        let r = self.call(
            &trap,
            Value::Object(handler),
            &[
                Value::Object(target),
                Value::Object(arr),
                Value::Object(new_target),
            ],
        )?;
        if !r.is_object() {
            return self.type_error("'construct' on proxy: trap returned non-object");
        }
        Ok(r)
    }
}
