//! Module Namespace exótico (§10.4.6): `[[Module]]`, `[[Exports]]` (ordenado) y sus métodos
//! internos. Las claves símbolo (solo `@@toStringTag`) se tratan como en un objeto ordinario;
//! las de string son los exports, cuyo valor sale del binding del módulo (vivo).

use crate::heap::Gc;
use crate::object::{Object, ObjectKind, PropertyDescriptor};
use crate::string::JsString;
use crate::value::{PropertyKey, Value};
use crate::vm::module::{ModuleId, NsTarget};
use crate::vm::{Agent, JsResult};

#[derive(Debug)]
pub struct NamespaceData {
    /// `[[Module]]`.
    pub module: ModuleId,
    /// `[[Exports]]` en orden de code units, con el binding que resuelve cada uno.
    pub exports: Vec<(JsString, NsTarget)>,
}

/// ModuleNamespaceCreate (§10.4.6.12).
pub fn create(
    agent: &mut Agent,
    module: ModuleId,
    exports: Vec<(JsString, NsTarget)>,
) -> Gc<Object> {
    let mut o = Object::new(
        None,
        ObjectKind::ModuleNamespace(Box::new(NamespaceData { module, exports })),
    );
    o.extensible = false;
    let o = agent.heap.alloc(o);
    // §28.3.1 @@toStringTag: "Module", { [[Writable]]: false, [[Enumerable]]: false,
    // [[Configurable]]: false }.
    let tag = PropertyKey::Symbol(agent.well_known.to_string_tag);
    agent.put_data(o, tag, Value::str("Module"), false, false, false);
    o
}

impl Agent {
    pub fn is_module_namespace(&self, o: Gc<Object>) -> bool {
        matches!(self.heap[o].kind, ObjectKind::ModuleNamespace(_))
    }

    /// El export de la clave (si la clave es un string que está en `[[Exports]]`).
    fn namespace_export(&self, o: Gc<Object>, key: &PropertyKey) -> Option<NsTarget> {
        let ObjectKind::ModuleNamespace(d) = &self.heap[o].kind else {
            return None;
        };
        let name = key.to_js_string()?;
        d.exports
            .binary_search_by(|(n, _)| n.units().cmp(name.units()))
            .ok()
            .map(|i| d.exports[i].1)
    }

    /// `[[GetOwnProperty]]` (§10.4.6.5) de una clave string.
    pub fn namespace_get_own_property(
        &mut self,
        o: Gc<Object>,
        key: &PropertyKey,
    ) -> JsResult<Option<PropertyDescriptor>> {
        if self.namespace_export(o, key).is_none() {
            return Ok(None);
        }
        let v = self.namespace_get(o, key)?;
        Ok(Some(PropertyDescriptor::data(v, true, true, false)))
    }

    /// `[[DefineOwnProperty]]` (§10.4.6.6) de una clave string.
    pub fn namespace_define_own_property(
        &mut self,
        o: Gc<Object>,
        key: &PropertyKey,
        desc: &PropertyDescriptor,
    ) -> JsResult<bool> {
        let Some(current) = self.namespace_get_own_property(o, key)? else {
            return Ok(false);
        };
        if desc.configurable == Some(true)
            || desc.enumerable == Some(false)
            || desc.is_accessor()
            || desc.writable == Some(false)
        {
            return Ok(false);
        }
        if let Some(v) = &desc.value {
            return Ok(self.same_value(v, &current.value.unwrap_or_default()));
        }
        Ok(true)
    }

    /// `[[HasProperty]]` (§10.4.6.7) de una clave string.
    pub fn namespace_has(&self, o: Gc<Object>, key: &PropertyKey) -> bool {
        self.namespace_export(o, key).is_some()
    }

    /// `[[Get]]` (§10.4.6.8) de una clave string: el valor del binding (ReferenceError si
    /// todavía no se inicializó).
    pub fn namespace_get(&mut self, o: Gc<Object>, key: &PropertyKey) -> JsResult<Value> {
        match self.namespace_export(o, key) {
            None => Ok(Value::Undefined),
            Some(NsTarget::Namespace(m)) => Ok(Value::Object(self.get_module_namespace(m))),
            Some(NsTarget::Binding(m, slot)) => match self.module_slot_value(m, slot) {
                Some(v) => Ok(v),
                None => {
                    let name = key.to_js_string().unwrap_or_default().to_string_lossy();
                    self.reference_error(&format!("Cannot access '{name}' before initialization"))
                }
            },
        }
    }

    /// `[[Delete]]` (§10.4.6.10) de una clave string.
    pub fn namespace_delete(&self, o: Gc<Object>, key: &PropertyKey) -> bool {
        self.namespace_export(o, key).is_none()
    }

    /// `[[OwnPropertyKeys]]` (§10.4.6.11): los exports y después los símbolos propios.
    pub fn namespace_own_keys(&self, o: Gc<Object>) -> Vec<PropertyKey> {
        let obj = &self.heap[o];
        let ObjectKind::ModuleNamespace(d) = &obj.kind else {
            return Vec::new();
        };
        let mut out: Vec<PropertyKey> = d
            .exports
            .iter()
            .map(|(n, _)| PropertyKey::from_js_string(n.clone()))
            .collect();
        out.extend(obj.props.keys().into_iter().filter(PropertyKey::is_symbol));
        out
    }
}
