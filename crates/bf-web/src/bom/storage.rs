//! `Storage` (HTML §12.2): `sessionStorage` y `localStorage`. El `localStorage` se carga y se
//! guarda con [`super::BomHooks::load_storage`]/[`super::BomHooks::save_storage`] (el
//! navegador lo persiste por origen).
//!
//! Las propiedades con nombre (`localStorage.clave`) de la interfaz con *named properties*
//! (WebIDL §3.9) se hacen con un `Proxy` sobre el objeto de plataforma.

use bf_js::host::{HostClass, HostHandle, HostKey};
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, RealmId};

use super::{bom_classes, origin_of, state, state_mut, to_rust_string, window_url};

/// La cuota por área: 5 MiB de unidades UTF-16 (como Chrome).
const QUOTA: usize = 5 * 1024 * 1024;

/// Los pares clave/valor de un área de almacenamiento, en orden de inserción.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StorageArea {
    pub items: Vec<(String, String)>,
}

impl StorageArea {
    fn size(&self) -> usize {
        self.items
            .iter()
            .map(|(k, v)| k.encode_utf16().count() + v.encode_utf16().count())
            .sum()
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.items
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// El payload de un objeto `Storage`: `true` para `localStorage`.
#[derive(Debug, Clone, Copy)]
struct Local(bool);

pub fn storage_class() -> HostClass {
    HostClass::new("Storage")
        .attribute("length", Some(length), None)
        .method("key", 1, key)
        .method("getItem", 1, get_item)
        .method("setItem", 2, set_item)
        .method("removeItem", 1, remove_item)
        .method("clear", 0, clear)
}

/// Las *named properties* de `Storage` sobre un `Proxy` (los métodos se llaman sobre el
/// objeto de plataforma).
const NAMED_PROPERTIES: &str = r#"(function (s) {
  var own = function (t, k) { return typeof k === "symbol" || k in t; };
  return new Proxy(s, {
    get: function (t, k) {
      if (own(t, k)) { var v = Reflect.get(t, k, t); return typeof v === "function" ? v.bind(t) : v; }
      var v = t.getItem(k); return v === null ? undefined : v;
    },
    set: function (t, k, v) {
      if (typeof k === "symbol") { return Reflect.set(t, k, v, t); }
      t.setItem(k, v); return true;
    },
    has: function (t, k) { return own(t, k) || t.getItem(k) !== null; },
    deleteProperty: function (t, k) {
      if (typeof k === "symbol") { return Reflect.deleteProperty(t, k); }
      t.removeItem(k); return true;
    },
    ownKeys: function (t) {
      var keys = [];
      for (var i = 0; i < t.length; i++) { keys.push(t.key(i)); }
      return keys.concat(Reflect.ownKeys(t));
    },
    getOwnPropertyDescriptor: function (t, k) {
      if (typeof k !== "symbol") {
        var v = t.getItem(k);
        if (v !== null) { return { value: v, writable: true, enumerable: true, configurable: true }; }
      }
      return Reflect.getOwnPropertyDescriptor(t, k);
    },
    defineProperty: function (t, k, d) {
      if (typeof k === "symbol" || !("value" in d)) { return Reflect.defineProperty(t, k, d); }
      t.setItem(k, d.value); return true;
    }
  });
})"#;

/// Crea el objeto `Storage` (envuelto en el `Proxy`) y lo fija.
pub fn create(agent: &mut Agent, realm: RealmId, n: u64, local: bool) -> HostHandle {
    let class = bom_classes(agent).storage;
    let o = agent.host_wrap(
        realm,
        HostKey(0xFFFF_FFFE_0000_0000 | n),
        class,
        Box::new(Local(local)),
    );
    let wrapped = agent
        .eval_script(NAMED_PROPERTIES, "storage")
        .and_then(|f| agent.call(&f, Value::Undefined, &[Value::Object(o)]))
        .unwrap_or(Value::Object(o));
    agent.host_pin(wrapped)
}

fn is_local(agent: &Agent, this: &Value) -> bool {
    agent.host_unwrap::<Local>(this).is_some_and(|l| l.0)
}

/// El área de `this` (la de `localStorage` se carga la primera vez).
fn area<'a>(agent: &'a mut Agent, this: &Value) -> &'a mut StorageArea {
    if is_local(agent, this) {
        if state(agent).local_storage.is_none() {
            let origin = origin_of(&window_url(agent));
            let items = state_mut(agent).hooks.load_storage(&origin);
            state_mut(agent).local_storage = Some(StorageArea { items });
        }
        state_mut(agent)
            .local_storage
            .as_mut()
            .expect("localStorage cargado")
    } else {
        &mut state_mut(agent).session_storage
    }
}

/// Después de cambiar `localStorage`: se guarda.
fn persist(agent: &mut Agent, this: &Value) {
    if !is_local(agent, this) {
        return;
    }
    let origin = origin_of(&window_url(agent));
    let s = state_mut(agent);
    if let Some(area) = &s.local_storage {
        let items = area.items.clone();
        s.hooks.save_storage(&origin, &items);
    }
}

fn length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(area(agent, &a.this).items.len() as f64))
}

/// `key(index)`: `index` es `unsigned long` (WebIDL ToUint32).
fn key(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = agent.to_number(&a.arg(0))?;
    let i = if n.is_finite() {
        (n.trunc().rem_euclid(4_294_967_296.0)) as usize
    } else {
        0
    };
    Ok(area(agent, &a.this)
        .items
        .get(i)
        .map_or(Value::Null, |(k, _)| Value::str(k)))
}

fn get_item(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let k = to_rust_string(agent, &a.arg(0))?;
    Ok(area(agent, &a.this).get(&k).map_or(Value::Null, Value::str))
}

/// `setItem(key, value)`: `QuotaExceededError` si no entra.
fn set_item(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let k = to_rust_string(agent, &a.arg(0))?;
    let v = to_rust_string(agent, &a.arg(1))?;
    let area = area(agent, &a.this);
    let mut next = area.clone();
    match next.items.iter_mut().find(|(key, _)| *key == k) {
        Some((_, old)) if *old == v => return Ok(Value::Undefined),
        Some((_, old)) => *old = v,
        None => next.items.push((k, v)),
    }
    if next.size() > QUOTA {
        return crate::dom::throw_dom(agent, bf_dom::DomException::QuotaExceededError);
    }
    *area = next;
    persist(agent, &a.this);
    Ok(Value::Undefined)
}

fn remove_item(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let k = to_rust_string(agent, &a.arg(0))?;
    let area = area(agent, &a.this);
    let before = area.items.len();
    area.items.retain(|(key, _)| *key != k);
    if area.items.len() != before {
        persist(agent, &a.this);
    }
    Ok(Value::Undefined)
}

fn clear(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let area = area(agent, &a.this);
    if !area.items.is_empty() {
        area.items.clear();
        persist(agent, &a.this);
    }
    Ok(Value::Undefined)
}
