//! Los objetos built-in. Cada módulo define sus funciones como `NativeFn` y las instala con
//! tablas `(nombre, length, función)`; las propiedades siguen las convenciones de §18
//! (métodos `{writable, configurable}` no enumerables; `length` y `name` no escribibles).

pub mod array;
pub mod boolean;
pub mod error;
pub mod function;
pub mod generator;
pub mod global;
pub mod host;
pub mod iterator;
pub mod math;
pub mod number;
pub mod object;
pub mod reflect;
pub mod string;
pub mod symbol;
// E3
pub mod array_buffer;
pub mod r#async;
pub mod atomics;
pub mod dataview;
pub mod map_set;
pub mod promise;
pub mod typed_array;
pub mod weak;

use crate::heap::Gc;
use crate::object::{Object, ObjectKind, Prop, PropValue};
use crate::string::JsString;
use crate::value::{PropertyKey, Symbol, Value};
use crate::vm::function::{BuiltinFunction, NativeFn};
use crate::vm::{Agent, RealmId};

/// Una entrada de tabla: nombre, `length` y la función.
pub type FnEntry = (&'static str, u32, NativeFn);

/// Define un método (CreateBuiltinFunction + CreateMethodProperty).
pub fn method(
    agent: &mut Agent,
    target: Gc<Object>,
    name: &str,
    length: u32,
    f: NativeFn,
) -> Gc<Object> {
    let func = agent.create_builtin_function(f, length, name, false);
    agent.put_data(
        target,
        PropertyKey::from(name),
        Value::Object(func),
        true,
        false,
        true,
    );
    func
}

/// Define los métodos de una tabla.
pub fn methods(agent: &mut Agent, target: Gc<Object>, table: &[FnEntry]) {
    for &(name, length, f) in table {
        method(agent, target, name, length, f);
    }
}

/// Un método con clave de símbolo (`name` es el nombre de la función, p. ej.
/// `"[Symbol.iterator]"`).
pub fn method_sym(
    agent: &mut Agent,
    target: Gc<Object>,
    sym: Gc<Symbol>,
    name: &str,
    length: u32,
    f: NativeFn,
) -> Gc<Object> {
    let func = agent.create_builtin_function(f, length, name, false);
    agent.put_data(
        target,
        PropertyKey::Symbol(sym),
        Value::Object(func),
        true,
        false,
        true,
    );
    func
}

/// Un accessor built-in (`get name` / `set name`).
pub fn accessor(
    agent: &mut Agent,
    target: Gc<Object>,
    key: PropertyKey,
    name: &str,
    get: Option<NativeFn>,
    set: Option<NativeFn>,
) {
    let g = get.map(|f| agent.create_builtin_function(f, 0, &format!("get {name}"), false));
    let s = set.map(|f| agent.create_builtin_function(f, 1, &format!("set {name}"), false));
    agent.heap[target].props.insert(
        key,
        Prop {
            value: PropValue::Accessor { get: g, set: s },
            enumerable: false,
            configurable: true,
        },
    );
}

/// Una propiedad de datos `{writable, configurable}` no enumerable.
pub fn value_prop(agent: &mut Agent, target: Gc<Object>, name: &str, v: Value) {
    agent.put_data(target, PropertyKey::from(name), v, true, false, true);
}

/// Una propiedad de datos inmutable `{false, false, false}`.
pub fn frozen_prop(agent: &mut Agent, target: Gc<Object>, name: &str, v: Value) {
    agent.put_data(target, PropertyKey::from(name), v, false, false, false);
}

/// `@@toStringTag` `{writable: false, configurable: true}`.
pub fn to_string_tag(agent: &mut Agent, target: Gc<Object>, tag: &str) {
    let k = PropertyKey::Symbol(agent.well_known.to_string_tag);
    agent.put_data(target, k, Value::str(tag), false, false, true);
}

/// Un constructor built-in con su `prototype` (no escribible) y `prototype.constructor`.
pub fn constructor(
    agent: &mut Agent,
    name: &str,
    length: u32,
    f: NativeFn,
    prototype: Gc<Object>,
) -> Gc<Object> {
    let c = agent.create_builtin_function(f, length, name, true);
    let k = agent.keys.prototype.clone();
    agent.put_data(c, k, Value::Object(prototype), false, false, false);
    let k = agent.keys.constructor.clone();
    agent.put_data(prototype, k, Value::Object(c), true, false, true);
    c
}

/// Los datos de una función built-in (`slots`).
pub fn slots(agent: &Agent, f: Gc<Object>) -> Vec<Value> {
    match &agent.heap[f].kind {
        ObjectKind::Builtin(b) => b.slots.clone(),
        _ => Vec::new(),
    }
}

/// `this` como objeto de cierto tipo, o TypeError.
pub fn this_string(agent: &mut Agent, this: &Value, method: &str) -> crate::vm::JsResult<JsString> {
    match this {
        Value::String(s) => Ok(s.clone()),
        Value::Object(o) => match &agent.heap[*o].kind {
            ObjectKind::String(s) => Ok(s.clone()),
            _ => agent.type_error(&format!("{method} requires that 'this' be a String")),
        },
        _ => agent.type_error(&format!("{method} requires that 'this' be a String")),
    }
}

fn function_prototype_call(
    _: &mut Agent,
    _: &crate::vm::function::CallArgs<'_>,
) -> crate::vm::JsResult<Value> {
    Ok(Value::Undefined)
}

/// CreateIntrinsics (§9.3.2): crea los intrínsecos del realm `realm`.
pub fn init_realm(agent: &mut Agent, realm: RealmId, object_prototype: Gc<Object>) {
    // %Function.prototype% es una función built-in que devuelve undefined (§20.2.3).
    let function_prototype = agent.heap.alloc(Object::new(
        Some(object_prototype),
        ObjectKind::Builtin(Box::new(BuiltinFunction {
            func: function_prototype_call,
            realm,
            constructor: false,
            slots: Vec::new(),
        })),
    ));
    agent.realms[realm].intrinsics.object_prototype = object_prototype;
    agent.realms[realm].intrinsics.function_prototype = function_prototype;
    let k = agent.keys.length.clone();
    agent.put_data(
        function_prototype,
        k,
        Value::Number(0.0),
        false,
        false,
        true,
    );
    let k = agent.keys.name.clone();
    agent.put_data(function_prototype, k, Value::str(""), false, false, true);

    object::init(agent, realm);
    function::init(agent, realm);
    error::init(agent, realm);
    symbol::init(agent, realm);
    iterator::init(agent, realm);
    array::init(agent, realm);
    string::init(agent, realm);
    number::init(agent, realm);
    boolean::init(agent, realm);
    generator::init(agent, realm);
    reflect::init(agent, realm);
    math::init(agent, realm);
    global::init_functions(agent, realm);
    // %BigInt.prototype% (el constructor y sus métodos son del paso 4).
    let bp = agent.new_object(Some(object_prototype));
    to_string_tag(agent, bp, "BigInt");
    agent.realms[realm].intrinsics.bigint_prototype = bp;
    // E3
    promise::init(agent, realm);
    r#async::init(agent, realm);
    map_set::init(agent, realm);
    weak::init(agent, realm);
    array_buffer::init(agent, realm);
    typed_array::init(agent, realm);
    dataview::init(agent, realm);
    atomics::init(agent, realm);
}
