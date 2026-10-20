//! Los métodos de los Environment Records (§9.1.1) que necesita la resolución dinámica de
//! nombres (`with`, `eval`, código global): HasBinding, GetBindingValue, SetMutableBinding,
//! InitializeBinding, DeleteBinding, GetIdentifierReference (§9.1.2.1), GetThisEnvironment
//! (§9.4.3) y los de Global Environment Records (§9.1.1.4).

use crate::heap::Gc;
use crate::object::PropertyDescriptor;
use crate::string::Atom;
use crate::value::{PropertyKey, Value};
use crate::vm::env::{BindingKind, DeclSlot, Env, EnvKind};
use crate::vm::{Agent, JsResult};

impl Agent {
    /// HasBinding(N) de cualquier Environment Record.
    pub fn env_has_binding(
        &mut self,
        env: Gc<Env>,
        name: &Atom,
        key: &PropertyKey,
    ) -> JsResult<bool> {
        match &self.heap[env].kind {
            EnvKind::Declarative(d) => Ok(d.find(name).is_some()),
            EnvKind::Object { object, with } => {
                let (object, with) = (*object, *with);
                self.object_env_has_binding(object, with, key)
            }
            EnvKind::Global(g) => {
                if g.decl.contains_key(name) {
                    return Ok(true);
                }
                let object = g.object;
                self.has_property(object, key)
            }
        }
    }

    /// HasBinding de un Object Environment Record (§9.1.1.2.1).
    fn object_env_has_binding(
        &mut self,
        object: Gc<crate::object::Object>,
        with: bool,
        key: &PropertyKey,
    ) -> JsResult<bool> {
        if !self.has_property(object, key)? {
            return Ok(false);
        }
        if !with {
            return Ok(true);
        }
        let sym = PropertyKey::Symbol(self.well_known.unscopables);
        let unscopables = self.get(object, &sym, Value::Object(object))?;
        if let Value::Object(u) = unscopables {
            let blocked = self.get(u, key, Value::Object(u))?;
            if crate::abstract_ops::conversion::to_boolean(self, &blocked) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// GetIdentifierReference (§9.1.2.1): el environment donde está el binding, o `None`.
    pub fn resolve_binding_from(
        &mut self,
        start: Gc<Env>,
        name: &Atom,
        key: &PropertyKey,
    ) -> JsResult<Option<Gc<Env>>> {
        let mut e = Some(start);
        while let Some(env) = e {
            if self.env_has_binding(env, name, key)? {
                return Ok(Some(env));
            }
            e = self.heap[env].outer;
        }
        Ok(None)
    }

    /// GetBindingValue(N, S).
    pub fn env_get_binding_value(
        &mut self,
        env: Gc<Env>,
        name: &Atom,
        key: &PropertyKey,
        strict: bool,
    ) -> JsResult<Value> {
        match &self.heap[env].kind {
            EnvKind::Declarative(d) => match d.find(name) {
                Some(slot) => match d.get(slot) {
                    Some(v) => Ok(v.clone()),
                    None => {
                        // Un binding de import indirecto (§9.1.1.5.1) (E1).
                        if let crate::vm::env::DeclSlot::Slot(i) = slot
                            && let Some(Some(v)) = self.module_link_value(env, i as u32)
                        {
                            return Ok(v);
                        }
                        self.reference_error(&format!(
                            "Cannot access '{name}' before initialization"
                        ))
                    }
                },
                None => self.reference_error(&format!("{name} is not defined")),
            },
            EnvKind::Object { object, .. } => {
                let object = *object;
                if !self.has_property(object, key)? {
                    if !strict {
                        return Ok(Value::Undefined);
                    }
                    return self.reference_error(&format!("{name} is not defined"));
                }
                self.get(object, key, Value::Object(object))
            }
            EnvKind::Global(g) => {
                if let Some((v, _)) = g.decl.get(name) {
                    return match v {
                        Some(v) => Ok(v.clone()),
                        None => self.reference_error(&format!(
                            "Cannot access '{name}' before initialization"
                        )),
                    };
                }
                let object = g.object;
                if !self.has_property(object, key)? {
                    if !strict {
                        return Ok(Value::Undefined);
                    }
                    return self.reference_error(&format!("{name} is not defined"));
                }
                self.get(object, key, Value::Object(object))
            }
        }
    }

    /// SetMutableBinding(N, V, S).
    pub fn env_set_mutable_binding(
        &mut self,
        env: Gc<Env>,
        name: &Atom,
        key: &PropertyKey,
        value: Value,
        strict: bool,
    ) -> JsResult<()> {
        match &mut self.heap[env].kind {
            EnvKind::Declarative(d) => {
                let Some(slot) = d.find(name) else {
                    // El binding se borró (un var de eval): §9.1.1.1.5 paso 1.
                    if strict {
                        return self.reference_error(&format!("{name} is not defined"));
                    }
                    d.extra.push((name.clone(), Some(value)));
                    return Ok(());
                };
                let kind = d.kind_of(slot);
                if d.get(slot).is_none() {
                    // Un binding de import inicializado es inmutable (§9.1.1.5) (E1).
                    if let DeclSlot::Slot(i) = slot
                        && let Some(Some(_)) = self.module_link_value(env, i as u32)
                    {
                        return self
                            .type_error(&format!("Assignment to constant variable '{name}'"));
                    }
                    return self
                        .reference_error(&format!("Cannot access '{name}' before initialization"));
                }
                let EnvKind::Declarative(d) = &mut self.heap[env].kind else {
                    unreachable!("declarativo");
                };
                let cell = d.get_mut(slot);
                match kind {
                    BindingKind::Mutable | BindingKind::Deletable => {
                        *cell = Some(value);
                        Ok(())
                    }
                    BindingKind::Const => {
                        self.type_error(&format!("Assignment to constant variable '{name}'"))
                    }
                    BindingKind::FunctionName => {
                        if strict {
                            self.type_error(&format!("Assignment to constant variable '{name}'"))
                        } else {
                            Ok(())
                        }
                    }
                }
            }
            EnvKind::Object { object, .. } => {
                let object = *object;
                let still = self.has_property(object, key)?;
                if !still && strict {
                    return self.reference_error(&format!("{name} is not defined"));
                }
                self.set_or_throw(object, key.clone(), value, strict)
            }
            EnvKind::Global(g) => {
                if let Some((cell, mutable)) = g.decl.get_mut(name) {
                    if cell.is_none() {
                        return self.reference_error(&format!(
                            "Cannot access '{name}' before initialization"
                        ));
                    }
                    if !*mutable {
                        return self
                            .type_error(&format!("Assignment to constant variable '{name}'"));
                    }
                    *cell = Some(value);
                    return Ok(());
                }
                let object = g.object;
                let still = self.has_property(object, key)?;
                if !still && strict {
                    return self.reference_error(&format!("{name} is not defined"));
                }
                self.set_or_throw(object, key.clone(), value, strict)
            }
        }
    }

    /// Set(O, P, V, Throw) (§7.3.4).
    pub fn set_or_throw(
        &mut self,
        o: Gc<crate::object::Object>,
        key: PropertyKey,
        value: Value,
        throw: bool,
    ) -> JsResult<()> {
        let ok = self.set(o, key.clone(), value, Value::Object(o))?;
        if !ok && throw {
            return self.type_error(&format!("Cannot assign to read only property '{key}'"));
        }
        Ok(())
    }

    /// InitializeBinding(N, V) en el environment donde se resolvió `name`.
    pub fn env_initialize_binding(
        &mut self,
        env: Gc<Env>,
        name: &Atom,
        key: &PropertyKey,
        value: Value,
    ) -> JsResult<()> {
        match &mut self.heap[env].kind {
            EnvKind::Declarative(d) => {
                if let Some(slot) = d.find(name) {
                    *d.get_mut(slot) = Some(value);
                }
                Ok(())
            }
            EnvKind::Object { object, .. } => {
                let object = *object;
                self.set_or_throw(object, key.clone(), value, false)
            }
            EnvKind::Global(g) => {
                if let Some((cell, _)) = g.decl.get_mut(name) {
                    *cell = Some(value);
                    return Ok(());
                }
                let object = g.object;
                self.set_or_throw(object, key.clone(), value, false)
            }
        }
    }

    /// DeleteBinding(N).
    pub fn env_delete_binding(
        &mut self,
        env: Gc<Env>,
        name: &Atom,
        key: &PropertyKey,
    ) -> JsResult<bool> {
        match &mut self.heap[env].kind {
            EnvKind::Declarative(d) => match d.find(name) {
                Some(DeclSlot::Extra(i)) => {
                    d.extra.remove(i);
                    Ok(true)
                }
                Some(DeclSlot::Slot(_)) => Ok(false),
                None => Ok(true),
            },
            EnvKind::Object { object, .. } => {
                let object = *object;
                self.delete(object, key)
            }
            EnvKind::Global(g) => {
                if g.decl.contains_key(name) {
                    return Ok(false);
                }
                let object = g.object;
                if self.has_own_property(object, key)? {
                    let status = self.delete(object, key)?;
                    if status && let Some(g) = self.heap[env].global_mut() {
                        g.var_names.remove(name);
                    }
                    return Ok(status);
                }
                Ok(true)
            }
        }
    }

    /// `withBaseObject` del environment (para el `this` de las llamadas).
    pub fn env_with_base_object(&self, env: Gc<Env>) -> Value {
        match &self.heap[env].kind {
            EnvKind::Object { object, with: true } => Value::Object(*object),
            _ => Value::Undefined,
        }
    }

    /// GetThisEnvironment (§9.4.3): el Function Environment con `this` propio, o el global.
    pub fn get_this_environment(&self, start: Gc<Env>) -> Gc<Env> {
        let mut e = start;
        loop {
            match &self.heap[e].kind {
                // Un Module Environment Record también tiene `this` (§9.1.1.5.3) (E1).
                EnvKind::Declarative(d) if d.function.is_some() || d.module.is_some() => return e,
                EnvKind::Global(_) => return e,
                _ => {}
            }
            match self.heap[e].outer {
                Some(o) => e = o,
                None => return e,
            }
        }
    }

    // --- Global Environment Records (§9.1.1.4) ---

    pub fn global_env(&self) -> Gc<Env> {
        self.realm().global_env
    }

    /// HasVarDeclaration (§9.1.1.4.12).
    pub fn global_has_var_declaration(&self, env: Gc<Env>, name: &Atom) -> bool {
        self.heap[env]
            .global()
            .is_some_and(|g| g.var_names.contains(name))
    }

    /// HasLexicalDeclaration (§9.1.1.4.13).
    pub fn global_has_lexical_declaration(&self, env: Gc<Env>, name: &Atom) -> bool {
        self.heap[env]
            .global()
            .is_some_and(|g| g.decl.contains_key(name))
    }

    fn global_object_of(&self, env: Gc<Env>) -> Gc<crate::object::Object> {
        self.heap[env].global().expect("global env").object
    }

    /// HasRestrictedGlobalProperty (§9.1.1.4.14).
    pub fn global_has_restricted_property(&mut self, env: Gc<Env>, name: &Atom) -> JsResult<bool> {
        let g = self.global_object_of(env);
        let key = PropertyKey::from_atom(name);
        match self.get_own_property(g, &key)? {
            None => Ok(false),
            Some(d) => Ok(d.configurable != Some(true)),
        }
    }

    /// CanDeclareGlobalVar (§9.1.1.4.15).
    pub fn global_can_declare_var(&mut self, env: Gc<Env>, name: &Atom) -> JsResult<bool> {
        let g = self.global_object_of(env);
        let key = PropertyKey::from_atom(name);
        if self.has_own_property(g, &key)? {
            return Ok(true);
        }
        self.is_extensible(g)
    }

    /// CanDeclareGlobalFunction (§9.1.1.4.16).
    pub fn global_can_declare_function(&mut self, env: Gc<Env>, name: &Atom) -> JsResult<bool> {
        let g = self.global_object_of(env);
        let key = PropertyKey::from_atom(name);
        match self.get_own_property(g, &key)? {
            None => self.is_extensible(g),
            Some(d) => Ok(d.configurable == Some(true)
                || (d.is_data() && d.writable == Some(true) && d.enumerable == Some(true))),
        }
    }

    /// CreateGlobalVarBinding (§9.1.1.4.17).
    pub fn global_create_var_binding(
        &mut self,
        env: Gc<Env>,
        name: &Atom,
        deletable: bool,
    ) -> JsResult<()> {
        let g = self.global_object_of(env);
        let key = PropertyKey::from_atom(name);
        let has = self.has_own_property(g, &key)?;
        let ext = self.is_extensible(g)?;
        if !has && ext {
            // ObjRec.CreateMutableBinding(N, D) + InitializeBinding(N, undefined).
            self.define_property_or_throw(
                g,
                key.clone(),
                PropertyDescriptor::data(Value::Undefined, true, true, deletable),
            )?;
        }
        if let Some(ge) = self.heap[env].global_mut() {
            ge.var_names.insert(name.clone());
        }
        Ok(())
    }

    /// CreateGlobalFunctionBinding (§9.1.1.4.18).
    pub fn global_create_function_binding(
        &mut self,
        env: Gc<Env>,
        name: &Atom,
        value: Value,
        deletable: bool,
    ) -> JsResult<()> {
        let g = self.global_object_of(env);
        let key = PropertyKey::from_atom(name);
        let existing = self.get_own_property(g, &key)?;
        let desc = match existing {
            Some(d) if d.configurable != Some(true) => PropertyDescriptor {
                value: Some(value.clone()),
                ..PropertyDescriptor::default()
            },
            _ => PropertyDescriptor::data(value.clone(), true, true, deletable),
        };
        self.define_property_or_throw(g, key.clone(), desc)?;
        self.set_or_throw(g, key, value, false)?;
        if let Some(ge) = self.heap[env].global_mut() {
            ge.var_names.insert(name.clone());
        }
        Ok(())
    }
}
