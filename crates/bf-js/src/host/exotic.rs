//! Objetos de plataforma *legacy* (WebIDL §3.9): las propiedades indexadas y con nombre de
//! las clases host que tienen `getter` (p. ej. `NodeList`, `HTMLCollection`,
//! `NamedNodeMap`). Sin setters ni deleters: las propiedades son de solo lectura.
//!
//! `crate::object::internal` delega aquí los métodos internos de los objetos host cuya clase
//! (o un ancestro) declara un [`HostExotic`].

use crate::heap::Gc;
use crate::object::{Object, ObjectKind, PropertyDescriptor};
use crate::string::JsString;
use crate::value::{PropertyKey, Value};
use crate::vm::{Agent, JsResult};

/// Los getters indexado y con nombre de una interfaz.
#[derive(Clone, Copy, Default)]
pub struct HostExotic {
    /// La cantidad de índices soportados (`length`).
    pub length: Option<fn(&mut Agent, Gc<Object>) -> usize>,
    /// El getter indexado: el valor del índice, o `None` si no está soportado.
    pub item: Option<fn(&mut Agent, Gc<Object>, u32) -> Option<Value>>,
    /// El getter con nombre: el valor, o `None` si el nombre no está soportado.
    pub named: Option<fn(&mut Agent, Gc<Object>, &JsString) -> Option<Value>>,
    /// Los nombres soportados, en orden.
    pub names: Option<fn(&mut Agent, Gc<Object>) -> Vec<JsString>>,
    /// ¿Las propiedades con nombre son enumerables? (Falso con
    /// `[LegacyUnenumerableNamedProperties]`.)
    pub named_enumerable: bool,
    /// `[LegacyOverrideBuiltIns]`.
    pub override_builtins: bool,
}

impl Agent {
    /// El [`HostExotic`] de un objeto host (el de su clase o el de un ancestro).
    pub(crate) fn host_exotic(&self, o: Gc<Object>) -> Option<HostExotic> {
        let ObjectKind::Host(h) = &self.heap[o].kind else {
            return None;
        };
        let mut class = Some(h.class);
        while let Some(c) = class {
            let def = &self.host.classes[c.0 as usize];
            if def.exotic.is_some() {
                return def.exotic;
            }
            class = def.parent;
        }
        None
    }
}

fn key_name(key: &PropertyKey) -> Option<JsString> {
    key.to_js_string()
}

/// El algoritmo de visibilidad de propiedades con nombre (WebIDL §3.9.1).
fn named_visible(
    agent: &mut Agent,
    o: Gc<Object>,
    key: &PropertyKey,
    ex: &HostExotic,
) -> JsResult<Option<Value>> {
    let (Some(named), Some(name)) = (ex.named, key_name(key)) else {
        return Ok(None);
    };
    if key.is_symbol() {
        return Ok(None);
    }
    let Some(v) = named(agent, o, &name) else {
        return Ok(None);
    };
    if agent.own_prop(o, key).is_some() {
        return Ok(None);
    }
    if !ex.override_builtins {
        let mut proto = agent.get_prototype_of(o)?;
        while let Some(p) = proto {
            if agent.get_own_property(p, key)?.is_some() {
                return Ok(None);
            }
            proto = agent.get_prototype_of(p)?;
        }
    }
    Ok(Some(v))
}

/// LegacyPlatformObjectGetOwnProperty (WebIDL §3.9.4).
pub(crate) fn get_own(
    agent: &mut Agent,
    o: Gc<Object>,
    key: &PropertyKey,
    ex: HostExotic,
    ignore_named: bool,
) -> JsResult<Option<PropertyDescriptor>> {
    let mut ignore_named = ignore_named;
    if let (Some(item), PropertyKey::Index(i)) = (ex.item, key) {
        if let Some(v) = item(agent, o, *i) {
            return Ok(Some(PropertyDescriptor::data(v, false, true, true)));
        }
        ignore_named = true;
    }
    if !ignore_named && let Some(v) = named_visible(agent, o, key, &ex)? {
        return Ok(Some(PropertyDescriptor::data(
            v,
            false,
            ex.named_enumerable,
            true,
        )));
    }
    Ok(agent.own_prop(o, key))
}

/// `[[Get]]` en el objeto mismo: `Some` si la propiedad es propia.
pub(crate) fn get_here(
    agent: &mut Agent,
    o: Gc<Object>,
    key: &PropertyKey,
    receiver: &Value,
    ex: HostExotic,
) -> JsResult<Option<Value>> {
    match get_own(agent, o, key, ex, false)? {
        None => Ok(None),
        Some(d) if d.is_data() => Ok(Some(d.value.unwrap_or_default())),
        Some(d) => match d.get.flatten() {
            Some(g) => agent
                .call(&Value::Object(g), receiver.clone(), &[])
                .map(Some),
            None => Ok(Some(Value::Undefined)),
        },
    }
}

/// `[[DefineOwnProperty]]` (WebIDL §3.9.3) sin setters.
pub(crate) fn define_own_property(
    agent: &mut Agent,
    o: Gc<Object>,
    key: PropertyKey,
    desc: PropertyDescriptor,
    ex: HostExotic,
) -> JsResult<bool> {
    if ex.item.is_some() && matches!(key, PropertyKey::Index(_)) {
        return Ok(false);
    }
    if let (Some(named), Some(name)) = (ex.named, key_name(&key))
        && !key.is_symbol()
    {
        let creating = named(agent, o, &name).is_none();
        if (ex.override_builtins || agent.own_prop(o, &key).is_none()) && !creating {
            return Ok(false);
        }
    }
    Ok(agent.ordinary_define_own_property(o, key, desc))
}

/// `[[Delete]]` (WebIDL §3.9.5): `Some` si la decide el objeto de plataforma.
pub(crate) fn delete(
    agent: &mut Agent,
    o: Gc<Object>,
    key: &PropertyKey,
    ex: HostExotic,
) -> JsResult<Option<bool>> {
    if let (Some(item), PropertyKey::Index(i)) = (ex.item, key) {
        return Ok(Some(item(agent, o, *i).is_none()));
    }
    if named_visible(agent, o, key, &ex)?.is_some() {
        return Ok(Some(false));
    }
    Ok(None)
}

/// `[[OwnPropertyKeys]]` (WebIDL §3.9.6).
pub(crate) fn own_keys(
    agent: &mut Agent,
    o: Gc<Object>,
    ex: HostExotic,
) -> JsResult<Vec<PropertyKey>> {
    let mut out = Vec::new();
    if let (Some(length), Some(_)) = (ex.length, ex.item) {
        let n = length(agent, o);
        out.extend((0..n as u32).map(PropertyKey::Index));
    }
    if let Some(names) = ex.names {
        for name in names(agent, o) {
            let key = PropertyKey::from_js_string(name);
            if named_visible(agent, o, &key, &ex)?.is_some() && !out.contains(&key) {
                out.push(key);
            }
        }
    }
    for k in agent.heap[o].props.keys() {
        if !out.contains(&k) {
            out.push(k);
        }
    }
    Ok(out)
}
