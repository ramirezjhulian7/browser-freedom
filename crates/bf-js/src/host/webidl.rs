//! Conversiones de WebIDL (§3.2, "JavaScript type mapping") que usan las operaciones y
//! atributos del host: `DOMString`, `USVString`, `boolean`, `long`, `unsigned long`,
//! `unrestricted double`, `any`, `object?`, `sequence<T>`, dictionaries, callback
//! functions y `EventHandler`.

use crate::heap::Gc;
use crate::object::Object;
use crate::string::JsString;
use crate::value::{PropertyKey, Value};
use crate::vm::{Agent, JsResult};

/// `DOMString` (§3.2.10): ToString (los símbolos lanzan TypeError).
pub fn to_dom_string(agent: &mut Agent, v: &Value) -> JsResult<JsString> {
    agent.to_string(v)
}

/// `DOMString` como `String` de Rust (los surrogates sueltos pasan a U+FFFD, como en
/// `USVString`).
pub fn to_rust_string(agent: &mut Agent, v: &Value) -> JsResult<String> {
    Ok(agent.to_string(v)?.to_string_lossy())
}

/// `[LegacyNullToEmptyString] DOMString`: `null` → "".
pub fn to_dom_string_null_empty(agent: &mut Agent, v: &Value) -> JsResult<JsString> {
    if v.is_null() {
        return Ok(JsString::default());
    }
    agent.to_string(v)
}

/// `DOMString?`: `null`/`undefined` → `None`.
pub fn to_nullable_dom_string(agent: &mut Agent, v: &Value) -> JsResult<Option<JsString>> {
    if v.is_nullish() {
        return Ok(None);
    }
    Ok(Some(agent.to_string(v)?))
}

/// `boolean` (§3.2.2).
pub fn to_boolean(agent: &Agent, v: &Value) -> bool {
    crate::abstract_ops::conversion::to_boolean(agent, v)
}

/// ConvertToInt (§3.2.4.9) sin `[EnforceRange]` ni `[Clamp]`, para `bits` bits.
fn convert_to_int(agent: &mut Agent, v: &Value, bits: u32, signed: bool) -> JsResult<f64> {
    let x = agent.to_number(v)?;
    if !x.is_finite() || x == 0.0 {
        return Ok(0.0);
    }
    let x = x.trunc();
    let m = 2f64.powi(bits as i32);
    let mut x = x.rem_euclid(m);
    if signed && x >= m / 2.0 {
        x -= m;
    }
    Ok(x)
}

/// `long` (§3.2.4.6).
pub fn to_long(agent: &mut Agent, v: &Value) -> JsResult<i32> {
    Ok(convert_to_int(agent, v, 32, true)? as i32)
}

/// `unsigned long` (§3.2.4.7).
pub fn to_unsigned_long(agent: &mut Agent, v: &Value) -> JsResult<u32> {
    Ok(convert_to_int(agent, v, 32, false)? as u32)
}

/// `unsigned short` (§3.2.4.5).
pub fn to_unsigned_short(agent: &mut Agent, v: &Value) -> JsResult<u16> {
    Ok(convert_to_int(agent, v, 16, false)? as u16)
}

/// `long long` (§3.2.4.8, hasta 2⁵³ en la práctica).
pub fn to_long_long(agent: &mut Agent, v: &Value) -> JsResult<i64> {
    Ok(convert_to_int(agent, v, 64, true)? as i64)
}

/// `[EnforceRange] long` (§3.2.4.9 paso 6): TypeError fuera de rango.
pub fn to_long_enforce_range(agent: &mut Agent, v: &Value) -> JsResult<i32> {
    let x = agent.to_number(v)?;
    if !x.is_finite() {
        return agent.type_error("Value is not a finite number");
    }
    let x = x.trunc();
    if !(-2147483648.0..=2147483647.0).contains(&x) {
        return agent.type_error("Value is outside the 'long' value range");
    }
    Ok(x as i32)
}

/// `double` (§3.2.5): TypeError si no es finito.
pub fn to_double(agent: &mut Agent, v: &Value) -> JsResult<f64> {
    let x = agent.to_number(v)?;
    if !x.is_finite() {
        return agent.type_error("The provided double value is non-finite");
    }
    Ok(x)
}

/// `unrestricted double` (§3.2.6).
pub fn to_unrestricted_double(agent: &mut Agent, v: &Value) -> JsResult<f64> {
    agent.to_number(v)
}

/// `object` (§3.2.18): TypeError si no es un objeto.
pub fn to_object(agent: &mut Agent, v: &Value) -> JsResult<Gc<Object>> {
    match v {
        Value::Object(o) => Ok(*o),
        _ => agent.type_error("The provided value is not of type 'object'"),
    }
}

/// `object?`: `null`/`undefined` → `None`.
pub fn to_nullable_object(agent: &mut Agent, v: &Value) -> JsResult<Option<Gc<Object>>> {
    match v {
        Value::Undefined | Value::Null => Ok(None),
        Value::Object(o) => Ok(Some(*o)),
        _ => agent.type_error("The provided value is not of type 'object'"),
    }
}

/// `sequence<T>` (§3.2.20): itera el valor con `conv`.
pub fn to_sequence<T>(
    agent: &mut Agent,
    v: &Value,
    mut conv: impl FnMut(&mut Agent, &Value) -> JsResult<T>,
) -> JsResult<Vec<T>> {
    if !v.is_object() {
        return agent.type_error("The provided value cannot be converted to a sequence");
    }
    let key = PropertyKey::Symbol(agent.well_known.iterator);
    let Some(method) = agent.get_method_of(v, &key)? else {
        return agent.type_error("The object must have a callable @@iterator property");
    };
    let (it, next) = agent.get_iterator_from_method(v, &method)?;
    let mut out = Vec::new();
    while let Some(x) = agent.iterator_step_value(&it, &next)? {
        out.push(conv(agent, &x)?);
    }
    Ok(out)
}

/// Un dictionary (§3.2.17): `undefined`/`null` es el diccionario vacío; otro primitivo
/// lanza TypeError.
pub struct Dictionary(pub Option<Gc<Object>>);

impl Dictionary {
    pub fn from_value(agent: &mut Agent, v: &Value) -> JsResult<Dictionary> {
        match v {
            Value::Undefined | Value::Null => Ok(Dictionary(None)),
            Value::Object(o) => Ok(Dictionary(Some(*o))),
            _ => agent.type_error("The provided value is not of type 'dictionary'"),
        }
    }

    /// El valor del miembro `name` (`undefined` si falta).
    pub fn get(&self, agent: &mut Agent, name: &str) -> JsResult<Value> {
        match self.0 {
            Some(o) => agent.get(o, &PropertyKey::from(name), Value::Object(o)),
            None => Ok(Value::Undefined),
        }
    }

    /// Un miembro `boolean` con su valor por defecto.
    pub fn get_bool(&self, agent: &mut Agent, name: &str, default: bool) -> JsResult<bool> {
        let v = self.get(agent, name)?;
        Ok(if v.is_undefined() {
            default
        } else {
            to_boolean(agent, &v)
        })
    }
}

/// Una callback function (§3.2.21): TypeError si no es invocable.
pub fn to_callback(agent: &mut Agent, v: &Value) -> JsResult<Value> {
    if !agent.is_callable(v) {
        return agent.type_error("The provided callback is not a function");
    }
    Ok(v.clone())
}

/// Una callback function nullable.
pub fn to_nullable_callback(agent: &mut Agent, v: &Value) -> JsResult<Option<Value>> {
    if v.is_nullish() {
        return Ok(None);
    }
    to_callback(agent, v).map(Some)
}

/// `EventHandler` (HTML §8.1.8.1, `[LegacyTreatNonObjectAsNull]`): un objeto (invocable o
/// no) o `null`; cualquier otro valor es `null`.
pub fn to_event_handler(v: &Value) -> Option<Value> {
    match v {
        Value::Object(_) => Some(v.clone()),
        _ => None,
    }
}

/// `any`: el valor tal cual.
pub fn to_any(v: &Value) -> Value {
    v.clone()
}
