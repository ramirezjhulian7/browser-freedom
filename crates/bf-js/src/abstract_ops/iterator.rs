//! Operaciones de iteradores (§7.4) e iteración de `for-in` (§14.7.5.9–10).

use std::collections::{HashSet, VecDeque};

use crate::heap::Gc;
use crate::object::{ForInData, Object, ObjectKind};
use crate::value::{PropertyKey, Value};
use crate::vm::{Agent, JsResult, Throw};

impl Agent {
    /// GetIterator(obj, sync) (§7.4.2): (iterador, next).
    pub fn get_iterator(&mut self, obj: &Value) -> JsResult<(Value, Value)> {
        let key = PropertyKey::Symbol(self.well_known.iterator);
        let method = self.get_method_of(obj, &key)?;
        let Some(method) = method else {
            return self.type_error(&format!("{} is not iterable", self.describe(obj)));
        };
        self.get_iterator_from_method(obj, &method)
    }

    /// GetIteratorFromMethod (§7.4.3).
    pub fn get_iterator_from_method(
        &mut self,
        obj: &Value,
        method: &Value,
    ) -> JsResult<(Value, Value)> {
        let it = self.call(method, obj.clone(), &[])?;
        if !it.is_object() {
            return self.type_error("Result of the Symbol.iterator method is not an object");
        }
        let k = self.keys.next.clone();
        let next = self.get_value_of(&it, &k)?;
        Ok((it, next))
    }

    /// IteratorNext (§7.4.4).
    pub fn iterator_next(
        &mut self,
        it: &Value,
        next: &Value,
        value: Option<Value>,
    ) -> JsResult<Gc<Object>> {
        let args: Vec<Value> = value.into_iter().collect();
        let r = self.call(next, it.clone(), &args)?;
        match r {
            Value::Object(o) => Ok(o),
            _ => self.type_error("Iterator result is not an object"),
        }
    }

    /// IteratorComplete (§7.4.5).
    pub fn iterator_complete(&mut self, result: Gc<Object>) -> JsResult<bool> {
        let k = self.keys.done.clone();
        let d = self.get(result, &k, Value::Object(result))?;
        Ok(crate::abstract_ops::conversion::to_boolean(self, &d))
    }

    /// IteratorValue (§7.4.6).
    pub fn iterator_value(&mut self, result: Gc<Object>) -> JsResult<Value> {
        let k = self.keys.value.clone();
        self.get(result, &k, Value::Object(result))
    }

    /// IteratorStepValue (§7.4.8): el siguiente valor, o `None` si terminó.
    pub fn iterator_step_value(&mut self, it: &Value, next: &Value) -> JsResult<Option<Value>> {
        let r = self.iterator_next(it, next, None)?;
        if self.iterator_complete(r)? {
            return Ok(None);
        }
        Ok(Some(self.iterator_value(r)?))
    }

    /// IteratorClose (§7.4.9) con una completion normal.
    pub fn iterator_close(&mut self, it: &Value) -> JsResult<()> {
        let k = self.keys.return_.clone();
        let ret = self.get_method_of(it, &k)?;
        let Some(ret) = ret else { return Ok(()) };
        let r = self.call(&ret, it.clone(), &[])?;
        if !r.is_object() {
            return self.type_error("iterator.return() did not return an object");
        }
        Ok(())
    }

    /// IteratorClose con una throw completion: los errores de `return` se ignoran.
    pub fn iterator_close_on_throw(&mut self, it: &Value) {
        let k = self.keys.return_.clone();
        if let Ok(Some(ret)) = self.get_method_of(it, &k) {
            let r = self.call(&ret, it.clone(), &[]);
            if let Err(Throw::Terminate(_)) = r {
                // La terminación no se puede ignorar, pero aquí ya se está propagando un error.
            }
        }
    }

    /// IteratorToList(GetIterator(v)) (§7.4.12).
    pub fn iterable_to_list(&mut self, v: &Value) -> JsResult<Vec<Value>> {
        let (it, next) = self.get_iterator(v)?;
        let mut out = Vec::new();
        while let Some(x) = self.iterator_step_value(&it, &next)? {
            out.push(x);
        }
        Ok(out)
    }

    /// CreateIterResultObject (§7.4.14).
    pub fn create_iter_result(&mut self, value: Value, done: bool) -> JsResult<Value> {
        let o = self.new_plain_object();
        let (kv, kd) = (self.keys.value.clone(), self.keys.done.clone());
        self.put_data(o, kv, value, true, true, true);
        self.put_data(o, kd, Value::Boolean(done), true, true, true);
        Ok(Value::Object(o))
    }

    /// CreateForInIterator (§14.7.5.10).
    pub fn create_for_in_iterator(&mut self, o: Gc<Object>) -> Gc<Object> {
        let proto = self.intrinsics().for_in_iterator_prototype;
        self.heap.alloc(Object::new(
            Some(proto),
            ObjectKind::ForIn(Box::new(ForInData {
                object: Some(o),
                object_was_visited: false,
                visited: HashSet::new(),
                remaining: VecDeque::new(),
            })),
        ))
    }

    /// %ForInIteratorPrototype%.next (§14.7.5.10.2.1): la siguiente clave, o `None`.
    pub fn for_in_next(&mut self, it: Gc<Object>) -> JsResult<Option<Value>> {
        loop {
            let (object, visited) = match &self.heap[it].kind {
                ObjectKind::ForIn(d) => (d.object, d.object_was_visited),
                _ => return Ok(None),
            };
            let Some(object) = object else {
                return Ok(None);
            };
            if !visited {
                let keys = self.own_property_keys(object)?;
                if let ObjectKind::ForIn(d) = &mut self.heap[it].kind {
                    d.remaining = keys.into_iter().filter(|k| !k.is_symbol()).collect();
                    d.object_was_visited = true;
                }
            }
            loop {
                let next = match &mut self.heap[it].kind {
                    ObjectKind::ForIn(d) => d.remaining.pop_front(),
                    _ => None,
                };
                let Some(k) = next else { break };
                let seen = match &self.heap[it].kind {
                    ObjectKind::ForIn(d) => d.visited.contains(&k),
                    _ => true,
                };
                if seen {
                    continue;
                }
                let desc = self.get_own_property(object, &k)?;
                if let Some(desc) = desc {
                    if let ObjectKind::ForIn(d) = &mut self.heap[it].kind {
                        d.visited.insert(k.clone());
                    }
                    if desc.enumerable == Some(true) {
                        return Ok(Some(k.to_value()));
                    }
                }
            }
            let proto = self.get_prototype_of(object)?;
            if let ObjectKind::ForIn(d) = &mut self.heap[it].kind {
                d.object = proto;
                d.object_was_visited = false;
            }
        }
    }
}
