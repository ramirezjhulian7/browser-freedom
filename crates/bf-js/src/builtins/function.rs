//! Function (§20.2), Function.prototype y %ThrowTypeError% (§10.2.4.1).

use crate::builtins::{FnEntry, constructor, method_sym, methods};
use crate::object::{Object, ObjectKind, Prop, PropValue};
use crate::string::JsString;
use crate::value::{PropertyKey, Value};
use crate::vm::eval::DynamicKind;
use crate::vm::function::{BoundFunction, CallArgs};
use crate::vm::{Agent, JsResult, RealmId};

pub fn init(agent: &mut Agent, realm: RealmId) {
    let proto = agent.realms[realm].intrinsics.function_prototype;
    let ctor = constructor(agent, "Function", 1, function_ctor, proto);
    agent.realms[realm].intrinsics.function_ctor = ctor;
    const PROTO: &[FnEntry] = &[
        ("apply", 2, apply),
        ("bind", 1, bind),
        ("call", 1, call),
        ("toString", 0, to_string),
    ];
    methods(agent, proto, PROTO);
    let hi = agent.well_known.has_instance;
    let f = method_sym(agent, proto, hi, "[Symbol.hasInstance]", 1, has_instance);
    // §20.2.3.6: { [[Writable]]: false, [[Enumerable]]: false, [[Configurable]]: false }.
    agent.put_data(
        proto,
        PropertyKey::Symbol(hi),
        Value::Object(f),
        false,
        false,
        false,
    );
    // %ThrowTypeError%.
    let tte = agent.create_builtin_function(throw_type_error, 0, "", false);
    let k = agent.keys.length.clone();
    agent.put_data(tte, k, Value::Number(0.0), false, false, false);
    let k = agent.keys.name.clone();
    agent.put_data(tte, k, Value::str(""), false, false, false);
    agent.heap[tte].extensible = false;
    agent.realms[realm].intrinsics.throw_type_error = tte;
    // AddRestrictedFunctionProperties (§10.2.4).
    for name in ["caller", "arguments"] {
        agent.heap[proto].props.insert(
            PropertyKey::from(name),
            Prop {
                value: PropValue::Accessor {
                    get: Some(tte),
                    set: Some(tte),
                },
                enumerable: false,
                configurable: true,
            },
        );
    }
}

fn throw_type_error(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    agent.type_error("'caller', 'callee', and 'arguments' properties may not be accessed on strict mode functions or the arguments objects for calls to them")
}

/// Function ( ...parameterArgs, bodyArg ) (§20.2.1.1).
fn function_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    agent.create_dynamic_function(a.callee, a.new_target, DynamicKind::Normal, a.args)
}

/// Function.prototype.apply (§20.2.3.1).
fn apply(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    if !agent.is_callable(&a.this) {
        return agent.type_error("Function.prototype.apply was called on a non-function");
    }
    let arr = a.arg(1);
    if arr.is_nullish() {
        return agent.call_internal(&a.this, a.arg(0), &[]);
    }
    let list = agent.create_list_from_array_like(&arr, false)?;
    agent.call_internal(&a.this, a.arg(0), &list)
}

/// Function.prototype.bind (§20.2.3.2).
fn bind(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Value::Object(target) = a.this else {
        return agent.type_error("Bind must be called on a function");
    };
    if !agent.heap[target].is_callable() {
        return agent.type_error("Bind must be called on a function");
    }
    let bound_args: Vec<Value> = a.args.iter().skip(1).cloned().collect();
    // BoundFunctionCreate (§10.4.1.3).
    let proto = agent.get_prototype_of(target)?;
    let is_ctor = agent.heap[target].is_constructor();
    let n_args = bound_args.len() as f64;
    let f = agent.alloc(Object::new(
        proto,
        ObjectKind::Bound(Box::new(BoundFunction {
            target,
            this: a.arg(0),
            args: bound_args,
            constructor: is_ctor,
        })),
    ));
    let mut length = 0.0;
    let lk = agent.keys.length.clone();
    if agent.has_own_property(target, &lk)? {
        let tl = agent.get(target, &lk, Value::Object(target))?;
        if let Value::Number(n) = tl {
            if n == f64::INFINITY {
                length = f64::INFINITY;
            } else if n == f64::NEG_INFINITY {
                length = 0.0;
            } else {
                let i = crate::abstract_ops::conversion::integer_or_infinity(n);
                length = (i - n_args).max(0.0);
            }
        }
    }
    agent.put_data(f, lk, Value::Number(length), false, false, true);
    let nk = agent.keys.name.clone();
    let name = agent.get(target, &nk, Value::Object(target))?;
    let name = match name {
        Value::String(s) => s,
        _ => JsString::default(),
    };
    let mut u: Vec<u16> = "bound ".encode_utf16().collect();
    u.extend_from_slice(name.units());
    agent.put_data(
        f,
        nk,
        Value::String(JsString::from_units(u)),
        false,
        false,
        true,
    );
    Ok(Value::Object(f))
}

/// Function.prototype.call (§20.2.3.3).
fn call(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    if !agent.is_callable(&a.this) {
        return agent.type_error("Function.prototype.call was called on a non-function");
    }
    let rest: Vec<Value> = a.args.iter().skip(1).cloned().collect();
    agent.call_internal(&a.this, a.arg(0), &rest)
}

/// Function.prototype.toString (§20.2.3.5).
fn to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    if let Value::Object(o) = a.this {
        match &agent.heap[o].kind {
            ObjectKind::Function(c) => {
                let s = c.code.source.slice(c.code.span);
                return Ok(Value::String(s));
            }
            ObjectKind::Builtin(_) | ObjectKind::Bound(_) => {
                let nk = agent.keys.name.clone();
                let name = match agent.heap[o].props.get(&nk).map(|p| p.value.clone()) {
                    Some(PropValue::Data {
                        value: Value::String(s),
                        ..
                    }) => s.to_string_lossy(),
                    _ => String::new(),
                };
                // NativeFunction: el nombre tiene que ser un PropertyName válido o vacío.
                let valid = name
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '$');
                let shown = if valid { name } else { String::new() };
                return Ok(Value::str(&format!(
                    "function {shown}() {{ [native code] }}"
                )));
            }
            _ => {}
        }
        // Cualquier otro objeto llamable (Proxy): NativeFunction (§20.2.3.5 paso 4).
        if agent.heap[o].is_callable() {
            return Ok(Value::str("function () { [native code] }"));
        }
    }
    agent.type_error("Function.prototype.toString requires that 'this' be a Function")
}

/// Function.prototype[@@hasInstance] (§20.2.3.6).
fn has_instance(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(
        agent.ordinary_has_instance(&a.this, &a.arg(0))?,
    ))
}
