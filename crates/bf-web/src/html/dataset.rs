//! E1: `HTMLElement.dataset` y `DOMStringMap` (HTML §3.2.6.6, "embedding custom non-visible
//! data with the data-* attributes").
//!
//! `DOMStringMap` es un objeto de plataforma *legacy* con getter, setter y deleter con nombre
//! (WebIDL §3.9). Los [`bf_js::host::HostExotic`] no tienen setter ni deleter, así que, como
//! `Storage`, las propiedades con nombre van en un `Proxy` sobre el objeto de plataforma (su
//! prototipo es `DOMStringMap.prototype`). Los traps leen y escriben los atributos `data-*`
//! con `getAttribute`/`setAttribute`/`removeAttribute` de `Element.prototype` (los mismos
//! efectos: mutation records, custom elements, `attributeChangedCallback`); esos métodos, y
//! lo demás que usan, se toman al registrar el realm, antes de cualquier script de la página.
//! `dataset` es `[SameObject]`: el `Proxy` de cada elemento se guarda en un `WeakMap`.

use bf_js::host::{HostClass, HostClassId, HostHandle};
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, RealmId};

/// La clase `DOMStringMap` (dato del embebedor).
struct DomStringMapClass(HostClassId);

/// La función de cada realm que da el `DOMStringMap` de un elemento.
struct DatasetFn(HostHandle);

/// Los traps del `Proxy` (`[LegacyOverrideBuiltIns]`) y la conversión de nombres (HTML
/// §3.2.6.6): recibe lo que necesita del realm recién creado y devuelve la función
/// `elemento → DOMStringMap`.
const FACTORY: &str = r#"(function (make, Element_proto, DOMException, Proxy, WeakMap, Reflect) {
  "use strict";
  var apply = Reflect.apply, getA = Element_proto.getAttribute,
      setA = Element_proto.setAttribute, remA = Element_proto.removeAttribute,
      namesA = Element_proto.getAttributeNames, rGet = Reflect.get,
      rDesc = Reflect.getOwnPropertyDescriptor, rDef = Reflect.defineProperty,
      rKeys = Reflect.ownKeys, rDel = Reflect.deleteProperty, rSet = Reflect.set;
  var cache = new WeakMap(), wGet = WeakMap.prototype.get, wSet = WeakMap.prototype.set;
  function isUpper(c) { return c >= 65 && c <= 90; }
  function isLower(c) { return c >= 97 && c <= 122; }
  // Los pares nombre-valor: los atributos `data-*` sin mayúsculas ASCII, en orden, con cada
  // "-" seguido de minúscula ASCII quitado y la minúscula en mayúscula.
  function pairs(el) {
    var names = apply(namesA, el, []), out = [];
    for (var i = 0; i < names.length; i++) {
      var n = names[i];
      if (n.length < 5 || n.slice(0, 5) !== "data-") continue;
      var rest = n.slice(5), ok = true;
      for (var j = 0; j < rest.length; j++) { if (isUpper(rest.charCodeAt(j))) { ok = false; break; } }
      if (!ok) continue;
      var camel = "";
      for (var j = 0; j < rest.length; j++) {
        var c = rest.charCodeAt(j);
        if (c === 45 && j + 1 < rest.length && isLower(rest.charCodeAt(j + 1))) {
          camel += rest.charAt(j + 1).toUpperCase(); j++;
        } else { camel += rest.charAt(j); }
      }
      out.push([camel, apply(getA, el, [n])]);
    }
    return out;
  }
  function lookup(el, k) {
    var p = pairs(el);
    for (var i = 0; i < p.length; i++) { if (p[i][0] === k) return p[i][1]; }
    return null;
  }
  // "-" delante de cada mayúscula ASCII, que pasa a minúscula, y "data-" al principio.
  function attrName(k) {
    var s = "data-";
    for (var i = 0; i < k.length; i++) {
      var c = k.charCodeAt(i);
      s += isUpper(c) ? "-" + String.fromCharCode(c + 32) : k.charAt(i);
    }
    return s;
  }
  function setter(el, k, v) {
    for (var i = 0; i + 1 < k.length; i++) {
      if (k.charCodeAt(i) === 45 && isLower(k.charCodeAt(i + 1))) {
        throw new DOMException("'" + k + "' is not a valid property name.", "SyntaxError");
      }
    }
    apply(setA, el, [attrName(k), `${v}`]);
  }
  // La visibilidad de las propiedades con nombre (WebIDL §3.9.1): `DOMStringMap` es
  // [LegacyOverrideBuiltIns], así que solo las tapa una propiedad propia del objeto.
  function visible(t, k) {
    return typeof k !== "symbol" && rDesc(t, k) === undefined;
  }
  return function (el) {
    var m = apply(wGet, cache, [el]);
    if (m !== undefined) return m;
    var target = make();
    m = new Proxy(target, {
      get: function (t, k) {
        if (visible(t, k)) { var v = lookup(el, k); if (v !== null) return v; }
        return rGet(t, k, t);
      },
      set: function (t, k, v) {
        if (typeof k === "symbol") return rSet(t, k, v, t);
        setter(el, k, v); return true;
      },
      has: function (t, k) { return (visible(t, k) && lookup(el, k) !== null) || (k in t); },
      deleteProperty: function (t, k) {
        if (!visible(t, k) || lookup(el, k) === null) return rDel(t, k);
        apply(remA, el, [attrName(k)]); return true;
      },
      ownKeys: function (t) {
        var p = pairs(el), keys = [];
        for (var i = 0; i < p.length; i++) { if (keys.indexOf(p[i][0]) < 0) keys.push(p[i][0]); }
        return keys.concat(rKeys(t));
      },
      getOwnPropertyDescriptor: function (t, k) {
        if (visible(t, k)) {
          var v = lookup(el, k);
          if (v !== null) return { value: v, writable: true, enumerable: true, configurable: true };
        }
        return rDesc(t, k);
      },
      defineProperty: function (t, k, d) {
        if (typeof k === "symbol") return rDef(t, k, d);
        if ("get" in d || "set" in d) return false;
        setter(el, k, d.value); return true;
      },
      preventExtensions: function () { return false; }
    });
    apply(wSet, cache, [el, m]);
    return m;
  };
})"#;

/// Registra `DOMStringMap` (la primera vez) y prepara la función de `realm`.
pub fn register(agent: &mut Agent, realm: RealmId) {
    if agent.embedder_data::<DomStringMapClass>().is_none() {
        let id = agent.register_host_class(HostClass::new("DOMStringMap"));
        agent.set_embedder_data(DomStringMapClass(id));
    }
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let element_proto = {
        let class = crate::dom::classes(agent).element;
        Value::Object(agent.host_class_objects(realm, class).prototype)
    };
    let g = agent.realms[realm].global_object;
    let mut global = |name: &str| {
        agent
            .get(g, &bf_js::value::PropertyKey::from(name), Value::Object(g))
            .unwrap_or_default()
    };
    let (dom_exception, proxy, weak_map, reflect) = (
        global("DOMException"),
        global("Proxy"),
        global("WeakMap"),
        global("Reflect"),
    );
    // Todo lo que se crea aquí se fija mientras dura: compilar la fábrica puede recolectar, y
    // nada más enraíza `holder` ni `make` (en una página con muchos marcos, cada realm nuevo
    // pasa por aquí).
    let holder = agent.new_plain_object();
    let holder_pin = agent.host_pin(Value::Object(holder));
    let make = bf_js::builtins::method(agent, holder, "make", 0, make_map);
    let make_pin = agent.host_pin(Value::Object(make));
    let f = match agent.eval_script(FACTORY, "dataset") {
        Ok(factory) => {
            let factory_pin = agent.host_pin(factory.clone());
            let r = agent.call(
                &factory,
                Value::Undefined,
                &[
                    Value::Object(make),
                    element_proto,
                    dom_exception,
                    proxy,
                    weak_map,
                    reflect,
                ],
            );
            agent.host_unpin(factory_pin);
            r
        }
        Err(e) => Err(e),
    };
    if let Ok(f) = f {
        let h = agent.host_pin(f);
        agent.set_realm_data(realm, DatasetFn(h));
    }
    agent.host_unpin(make_pin);
    agent.host_unpin(holder_pin);
    agent.current_realm = saved;
}

/// El objeto de plataforma `DOMStringMap` (el target del `Proxy`), en el realm actual.
fn make_map(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let Some(&DomStringMapClass(class)) = agent.embedder_data::<DomStringMapClass>() else {
        return agent.type_error("DOMStringMap no está registrado");
    };
    Ok(Value::Object(agent.host_create(class, Box::new(()))))
}

/// `HTMLElement.dataset` (HTML §3.2.6.6, `[SameObject]`).
pub fn get_dataset(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    crate::dom::this_node(agent, a)?;
    let realm = agent.current_realm;
    let f = agent
        .realm_data::<DatasetFn>(realm)
        .and_then(|d| agent.host_pinned(d.0));
    match f {
        Some(f) => agent.call(&f, Value::Undefined, std::slice::from_ref(&a.this)),
        None => Ok(Value::Undefined),
    }
}
