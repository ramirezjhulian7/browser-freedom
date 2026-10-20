//! Valores del lenguaje (ECMA-262 §6.1): los tipos primitivos, los objetos (en el heap) y las
//! claves de propiedades (§6.1.7) con los índices de array canónicos ya detectados.

use std::fmt;

use crate::bigint::BigInt;
use crate::heap::Gc;
use crate::object::Object;
use crate::string::{Atom, JsString};

/// Un valor de ECMAScript.
#[derive(Clone, Default)]
pub enum Value {
    #[default]
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    String(JsString),
    Symbol(Gc<Symbol>),
    BigInt(Gc<BigInt>),
    Object(Gc<Object>),
}

impl Value {
    pub fn is_undefined(&self) -> bool {
        matches!(self, Value::Undefined)
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// `undefined` o `null`.
    pub fn is_nullish(&self) -> bool {
        matches!(self, Value::Undefined | Value::Null)
    }

    pub fn is_object(&self) -> bool {
        matches!(self, Value::Object(_))
    }

    pub fn as_object(&self) -> Option<Gc<Object>> {
        match self {
            Value::Object(o) => Some(*o),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&JsString> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn str(s: &str) -> Value {
        Value::String(JsString::from(s))
    }

    pub fn from_bool(b: bool) -> Value {
        Value::Boolean(b)
    }

    /// El nombre del tipo según la tabla de §6.1 (para los mensajes de error).
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Undefined => "undefined",
            Value::Null => "null",
            Value::Boolean(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Symbol(_) => "symbol",
            Value::BigInt(_) => "bigint",
            Value::Object(_) => "object",
        }
    }
}

impl From<f64> for Value {
    fn from(n: f64) -> Value {
        Value::Number(n)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Value {
        Value::Boolean(b)
    }
}

impl From<JsString> for Value {
    fn from(s: JsString) -> Value {
        Value::String(s)
    }
}

impl From<Gc<Object>> for Value {
    fn from(o: Gc<Object>) -> Value {
        Value::Object(o)
    }
}

impl From<Option<Gc<Object>>> for Value {
    fn from(o: Option<Gc<Object>>) -> Value {
        match o {
            Some(o) => Value::Object(o),
            None => Value::Undefined,
        }
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Undefined => f.write_str("undefined"),
            Value::Null => f.write_str("null"),
            Value::Boolean(b) => write!(f, "{b}"),
            Value::Number(n) => write!(f, "{n}"),
            Value::String(s) => write!(f, "{s:?}"),
            Value::Symbol(s) => write!(f, "Symbol({s:?})"),
            Value::BigInt(b) => write!(f, "BigInt({b:?})"),
            Value::Object(o) => write!(f, "Object({o:?})"),
        }
    }
}

/// Un Symbol (§6.1.5). Los Private Names (§6.2.12) también son símbolos, marcados con
/// `private`: nunca llegan al código JavaScript como valores.
#[derive(Debug, Clone)]
pub struct Symbol {
    pub description: Option<JsString>,
    pub private: bool,
}

/// Una clave de propiedad (§6.1.7): los strings que son array index (§6.1.7, "array index":
/// enteros canónicos en `[0, 2³² − 2]`) se representan como `Index`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum PropertyKey {
    Index(u32),
    String(JsString),
    Symbol(Gc<Symbol>),
}

impl PropertyKey {
    /// Normaliza un string: si es un array index canónico, `Index`.
    pub fn from_js_string(s: JsString) -> PropertyKey {
        match array_index(s.units()) {
            Some(i) => PropertyKey::Index(i),
            None => PropertyKey::String(s),
        }
    }

    pub fn from_atom(a: &Atom) -> PropertyKey {
        PropertyKey::from_js_string(a.to_js_string())
    }

    /// La clave de un Number: `Index` si es un entero en el rango, si no `ToString(n)`.
    pub fn from_number(n: f64) -> PropertyKey {
        if n >= 0.0 && n < 4294967295.0 && n.fract() == 0.0 {
            return PropertyKey::Index(n as u32);
        }
        PropertyKey::from_js_string(crate::abstract_ops::number::number_to_string(n))
    }

    pub fn is_symbol(&self) -> bool {
        matches!(self, PropertyKey::Symbol(_))
    }

    pub fn as_index(&self) -> Option<u32> {
        match self {
            PropertyKey::Index(i) => Some(*i),
            _ => None,
        }
    }

    /// El valor de la clave (String o Symbol).
    pub fn to_value(&self) -> Value {
        match self {
            PropertyKey::Index(i) => Value::String(JsString::from(i.to_string().as_str())),
            PropertyKey::String(s) => Value::String(s.clone()),
            PropertyKey::Symbol(s) => Value::Symbol(*s),
        }
    }

    /// La clave como string (las de símbolo no tienen).
    pub fn to_js_string(&self) -> Option<JsString> {
        match self {
            PropertyKey::Index(i) => Some(JsString::from(i.to_string().as_str())),
            PropertyKey::String(s) => Some(s.clone()),
            PropertyKey::Symbol(_) => None,
        }
    }

    /// ¿La clave es el string `s`?
    pub fn is_str(&self, s: &str) -> bool {
        match self {
            PropertyKey::String(k) => k.units().iter().copied().eq(s.encode_utf16()),
            PropertyKey::Index(i) => i.to_string() == s,
            PropertyKey::Symbol(_) => false,
        }
    }
}

impl From<&str> for PropertyKey {
    fn from(s: &str) -> PropertyKey {
        PropertyKey::from_js_string(JsString::from(s))
    }
}

impl From<u32> for PropertyKey {
    fn from(i: u32) -> PropertyKey {
        if i == u32::MAX {
            PropertyKey::String(JsString::from("4294967295"))
        } else {
            PropertyKey::Index(i)
        }
    }
}

impl From<Gc<Symbol>> for PropertyKey {
    fn from(s: Gc<Symbol>) -> PropertyKey {
        PropertyKey::Symbol(s)
    }
}

impl fmt::Display for PropertyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PropertyKey::Index(i) => write!(f, "{i}"),
            PropertyKey::String(s) => write!(f, "{s}"),
            PropertyKey::Symbol(_) => f.write_str("Symbol()"),
        }
    }
}

impl fmt::Debug for PropertyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PropertyKey::Index(i) => write!(f, "{i}"),
            PropertyKey::String(s) => write!(f, "{s:?}"),
            PropertyKey::Symbol(s) => write!(f, "Symbol({s:?})"),
        }
    }
}

/// ¿`units` es un array index canónico? (Sin ceros a la izquierda, menor que 2³² − 1.)
pub fn array_index(units: &[u16]) -> Option<u32> {
    if units.is_empty() || units.len() > 10 {
        return None;
    }
    if units[0] == u16::from(b'0') {
        return if units.len() == 1 { Some(0) } else { None };
    }
    let mut v: u64 = 0;
    for &u in units {
        if !(u16::from(b'0')..=u16::from(b'9')).contains(&u) {
            return None;
        }
        v = v * 10 + u64::from(u - u16::from(b'0'));
    }
    if v < 4294967295 { Some(v as u32) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn array_indices() {
        assert_eq!(
            array_index(&"0".encode_utf16().collect::<Vec<_>>()),
            Some(0)
        );
        assert_eq!(array_index(&"01".encode_utf16().collect::<Vec<_>>()), None);
        assert_eq!(
            array_index(&"4294967294".encode_utf16().collect::<Vec<_>>()),
            Some(4294967294)
        );
        assert_eq!(
            array_index(&"4294967295".encode_utf16().collect::<Vec<_>>()),
            None
        );
        assert_eq!(array_index(&"-1".encode_utf16().collect::<Vec<_>>()), None);
        assert!(matches!(PropertyKey::from("12"), PropertyKey::Index(12)));
        assert!(matches!(PropertyKey::from("1.5"), PropertyKey::String(_)));
    }
}
