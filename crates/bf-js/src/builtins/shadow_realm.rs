//! ShadowRealm (propuesta ShadowRealm de TC39, stage 2.7, §3): un realm aparte con sus
//! intrínsecos y su objeto global, al que solo cruzan valores primitivos y funciones
//! envueltas (Wrapped Function Exotic Objects, §2.1): `evaluate` corre un script en el otro
//! realm e `importValue` importa un módulo ahí y devuelve una de sus exportaciones.
//!
//! Las funciones envueltas son built-ins del realm que las recibe con la función de destino
//! en un slot: su `[[Call]]` envuelve los argumentos y `this` hacia el realm del destino y
//! el resultado de vuelta; cualquier excepción que cruce es un TypeError del realm que llamó.

use crate::builtins::{constructor, method, register_global, slots, to_string_tag};
use crate::heap::Gc;
use crate::object::{Object, ObjectKind, PropertyDescriptor};
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId, Throw};

pub fn init(agent: &mut Agent, realm: RealmId) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    let proto = agent.new_object(Some(op));
    method(agent, proto, "evaluate", 1, evaluate);
    method(agent, proto, "importValue", 2, import_value);
    to_string_tag(agent, proto, "ShadowRealm");
    let c = constructor(agent, "ShadowRealm", 0, shadow_realm_ctor, proto);
    register_global(agent, realm, "ShadowRealm", c);
    agent.realms[realm].intrinsics.shadow_realm_prototype = proto;
}

/// ShadowRealm ( ) (§3.2.1).
fn shadow_realm_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor ShadowRealm requires 'new'");
    };
    let o = agent.ordinary_create_from_constructor(
        nt,
        |i| i.shadow_realm_prototype,
        ObjectKind::ShadowRealm(agent.current_realm),
    )?;
    // CreateRealm + SetDefaultGlobalBindings (+ HostInitializeShadowRealm del host).
    let realm = agent.create_realm();
    agent.heap[o].kind = ObjectKind::ShadowRealm(realm);
    Ok(Value::Object(o))
}

/// ValidateShadowRealmObject (§3.1.5).
fn this_realm(agent: &mut Agent, this: &Value, method: &str) -> JsResult<RealmId> {
    if let Value::Object(o) = this
        && let ObjectKind::ShadowRealm(r) = agent.heap[*o].kind
    {
        return Ok(r);
    }
    agent.type_error(&format!(
        "ShadowRealm.prototype.{method} called on incompatible receiver"
    ))
}

/// Corre `f` con `realm` como realm actual.
fn in_realm<T>(agent: &mut Agent, realm: RealmId, f: impl FnOnce(&mut Agent) -> T) -> T {
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let r = f(agent);
    agent.current_realm = saved;
    r
}

/// Un TypeError del realm `realm` (el que llamó) por un valor que no pudo cruzar.
fn boundary_error<T>(agent: &mut Agent, realm: RealmId, what: &str) -> JsResult<T> {
    in_realm(agent, realm, |agent| agent.type_error(what))
}

/// Convierte en TypeError de `caller` una excepción que viene del otro lado.
fn wrap_throw<T>(agent: &mut Agent, caller: RealmId, r: JsResult<T>, what: &str) -> JsResult<T> {
    match r {
        Err(Throw::Value(e)) => {
            let detail = match &e {
                Value::Object(o) => {
                    let name = agent.object_class_name(*o);
                    match agent.heap[*o]
                        .props
                        .get(&PropertyKey::from("message"))
                        .map(|p| p.to_descriptor())
                    {
                        Some(PropertyDescriptor {
                            value: Some(Value::String(m)),
                            ..
                        }) => format!("{name}: {}", m.to_string_lossy()),
                        _ => name,
                    }
                }
                v => agent.describe(v),
            };
            boundary_error(agent, caller, &format!("{what}: {detail}"))
        }
        other => other,
    }
}

/// GetWrappedValue (§2.2.1): un primitivo cruza tal cual, una función se envuelve, otro
/// objeto es un TypeError (de `caller`).
pub fn get_wrapped_value(agent: &mut Agent, caller: RealmId, v: Value) -> JsResult<Value> {
    let Value::Object(o) = v else {
        return Ok(v);
    };
    if !agent.heap[o].is_callable() {
        return boundary_error(
            agent,
            caller,
            "ShadowRealm: only primitives and functions can cross the realm boundary",
        );
    }
    wrapped_function_create(agent, caller, o)
}

/// WrappedFunctionCreate (§2.2.2) con CopyNameAndLength (§2.2.3).
fn wrapped_function_create(
    agent: &mut Agent,
    caller: RealmId,
    target: Gc<Object>,
) -> JsResult<Value> {
    let f = in_realm(agent, caller, |agent| {
        agent.create_builtin_with_slots(wrapped_call, 0, "", vec![Value::Object(target)])
    });
    let r = in_realm(agent, caller, |agent| {
        copy_name_and_length(agent, f, target)
    });
    wrap_throw(
        agent,
        caller,
        r,
        "ShadowRealm: the wrapped function's length or name could not be read",
    )?;
    Ok(Value::Object(f))
}

/// CopyNameAndLength (§2.2.3) con argCount 0 y sin prefijo.
fn copy_name_and_length(agent: &mut Agent, f: Gc<Object>, target: Gc<Object>) -> JsResult<()> {
    let length_key = PropertyKey::from("length");
    let mut l = 0.0;
    if agent.has_own_property(target, &length_key)? {
        let len = agent.get(target, &length_key, Value::Object(target))?;
        if let Value::Number(n) = len {
            l = if n == f64::INFINITY {
                f64::INFINITY
            } else if n == f64::NEG_INFINITY || n.is_nan() {
                0.0
            } else {
                n.trunc().max(0.0)
            };
        }
    }
    agent.define_property_or_throw(
        f,
        length_key,
        PropertyDescriptor::data(Value::Number(l), false, false, true),
    )?;
    let name = agent.get(target, &PropertyKey::from("name"), Value::Object(target))?;
    let name = match name {
        Value::String(s) => Value::String(s),
        _ => Value::String(crate::string::JsString::from("")),
    };
    agent.define_property_or_throw(
        f,
        PropertyKey::from("name"),
        PropertyDescriptor::data(name, false, false, true),
    )
}

/// `[[Call]]` de una función envuelta (§2.1.1): OrdinaryWrappedFunctionCall; cualquier
/// excepción es un TypeError del realm de la función envuelta.
fn wrapped_call(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let caller = agent.current_realm;
    let Some(Value::Object(target)) = slots(agent, a.callee).into_iter().next() else {
        return agent.type_error("ShadowRealm: wrapped function without target");
    };
    let r = (|| -> JsResult<Value> {
        let target_realm = agent.get_function_realm(target)?;
        let mut args = Vec::with_capacity(a.args.len());
        for v in a.args {
            args.push(get_wrapped_value(agent, target_realm, v.clone())?);
        }
        let this = get_wrapped_value(agent, target_realm, a.this.clone())?;
        let result = agent.call(&Value::Object(target), this, &args)?;
        get_wrapped_value(agent, caller, result)
    })();
    wrap_throw(agent, caller, r, "ShadowRealm: the wrapped function threw")
}

/// ShadowRealm.prototype.evaluate ( sourceText ) (§3.4.1) con PerformShadowRealmEval
/// (§3.1.1).
fn evaluate(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let eval_realm = this_realm(agent, &a.this, "evaluate")?;
    let Value::String(src) = a.arg(0) else {
        return agent.type_error("ShadowRealm.prototype.evaluate requires a string");
    };
    let caller = agent.current_realm;
    // Los errores de sintaxis son del realm que llama (§3.1.1 paso 3).
    agent.compile_script_text(&src.to_string_lossy(), "ShadowRealm.evaluate")?;
    // Pasos 5–19: como un `eval` indirecto en el otro realm: las variables van al objeto
    // global y las declaraciones léxicas a un entorno nuevo de cada evaluación.
    let r = in_realm(agent, eval_realm, |agent| {
        agent.perform_eval(
            &Value::String(src),
            false,
            crate::compiler::EvalFlags::default(),
        )
    });
    let v = wrap_throw(agent, caller, r, "ShadowRealm evaluation threw")?;
    get_wrapped_value(agent, caller, v)
}

/// ShadowRealm.prototype.importValue ( specifier, exportName ) (§3.4.2) con
/// ShadowRealmImportValue (§3.1.2): `import()` en el otro realm; la exportación pedida
/// cruza con GetWrappedValue.
fn import_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let eval_realm = this_realm(agent, &a.this, "importValue")?;
    let specifier = agent.to_string(&a.arg(0))?;
    let Value::String(export) = a.arg(1) else {
        return agent.type_error("ShadowRealm.prototype.importValue: exportName must be a string");
    };
    let caller = agent.current_realm;
    let inner = in_realm(agent, eval_realm, |agent| {
        agent.evaluate_import_call(Value::String(specifier), Value::Undefined)
    })?;
    let Value::Object(inner) = inner else {
        return agent.type_error("ShadowRealm: import() did not return a promise");
    };
    let result = agent.new_internal_promise();
    let on_fulfilled = agent.create_builtin_with_slots(
        export_getter,
        1,
        "",
        vec![Value::String(export), Value::Number(caller as f64)],
    );
    let on_rejected = agent.create_builtin_with_slots(import_failed, 1, "", Vec::new());
    agent.perform_promise_then(
        inner,
        crate::builtins::promise::Handler::Callback(Value::Object(on_fulfilled)),
        crate::builtins::promise::Handler::Callback(Value::Object(on_rejected)),
        Some(crate::builtins::promise::Capability::Internal(result)),
    );
    Ok(Value::Object(result))
}

/// ExportGetter (§3.1.2 paso 9): la exportación del namespace, envuelta.
fn export_getter(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = slots(agent, a.callee);
    let (Some(Value::String(name)), Some(Value::Number(caller))) = (s.first(), s.get(1)) else {
        return agent.type_error("ShadowRealm: export getter without slots");
    };
    let caller = *caller as RealmId;
    let Value::Object(ns) = a.arg(0) else {
        return agent.type_error("ShadowRealm: the module namespace is not an object");
    };
    let key = PropertyKey::from_js_string(name.clone());
    if !agent.has_own_property(ns, &key)? {
        return agent.type_error(&format!(
            "ShadowRealm.importValue: the module has no export named '{}'",
            name.to_string_lossy()
        ));
    }
    let v = agent.get(ns, &key, Value::Object(ns))?;
    get_wrapped_value(agent, caller, v)
}

/// %ThrowTypeError% del realm que llamó (§3.1.2 paso 11): el import falló.
fn import_failed(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let detail = agent.describe(&a.arg(0));
    agent.type_error(&format!(
        "ShadowRealm.importValue: the import failed ({detail})"
    ))
}
