//! Conversiones de tipo (§7.1).

use crate::abstract_ops::number::number_to_string;
use crate::heap::Gc;
use crate::object::{Object, ObjectKind};
use crate::string::JsString;
use crate::value::{PropertyKey, Value};
use crate::vm::{Agent, JsResult};

/// El hint de ToPrimitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    Default,
    String,
    Number,
}

/// ToBoolean (§7.1.2).
pub fn to_boolean(agent: &Agent, v: &Value) -> bool {
    match v {
        Value::Undefined | Value::Null => false,
        Value::Boolean(b) => *b,
        Value::Number(n) => !(*n == 0.0 || n.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Symbol(_) => true,
        // B.3.6.1: los objetos con [[IsHTMLDDA]] son falsy.
        Value::Object(o) => !matches!(agent.heap[*o].kind, ObjectKind::HtmlDda),
        Value::BigInt(b) => !agent.heap[*b].is_zero(),
    }
}

/// Quita StrWhiteSpaceChar (WhiteSpace y LineTerminator) de los extremos.
pub fn trim_js_whitespace(s: &str) -> &str {
    s.trim_matches(|c: char| crate::lexer::is_whitespace(c) || crate::lexer::is_line_terminator(c))
}

fn is_str_whitespace(u: u16) -> bool {
    match char::from_u32(u32::from(u)) {
        Some(c) => crate::lexer::is_whitespace(c) || crate::lexer::is_line_terminator(c),
        None => false,
    }
}

/// Los code units sin StrWhiteSpaceChar en los extremos.
pub fn trim_units(s: &[u16]) -> &[u16] {
    let start = s
        .iter()
        .position(|&u| !is_str_whitespace(u))
        .unwrap_or(s.len());
    let end = s
        .iter()
        .rposition(|&u| !is_str_whitespace(u))
        .map_or(start, |e| e + 1);
    &s[start..end.max(start)]
}

/// StringToNumber (§7.1.4.1.1) con la gramática StringNumericLiteral.
pub fn string_to_number(s: &[u16]) -> f64 {
    let t = trim_units(s);
    if t.is_empty() {
        return 0.0;
    }
    let Ok(text) = String::from_utf16(t) else {
        return f64::NAN;
    };
    let b = text.as_bytes();
    // NonDecimalIntegerLiteral (sin signo ni separadores).
    if b.len() > 2 && b[0] == b'0' {
        let radix = match b[1] {
            b'x' | b'X' => Some(16),
            b'o' | b'O' => Some(8),
            b'b' | b'B' => Some(2),
            _ => None,
        };
        if let Some(radix) = radix {
            let digits = &text[2..];
            if !digits.chars().all(|c| c.is_digit(radix)) {
                return f64::NAN;
            }
            return crate::lexer::number::radix_to_f64(digits, radix);
        }
    }
    let (sign, rest) = match b[0] {
        b'+' => (1.0, &text[1..]),
        b'-' => (-1.0, &text[1..]),
        _ => (1.0, &text[..]),
    };
    if rest == "Infinity" {
        return sign * f64::INFINITY;
    }
    // StrUnsignedDecimalLiteral: dígitos, punto, exponente (sin separadores).
    let rb = rest.as_bytes();
    let mut i = 0;
    let mut int_digits = 0;
    while i < rb.len() && rb[i].is_ascii_digit() {
        i += 1;
        int_digits += 1;
    }
    let mut frac_digits = 0;
    if i < rb.len() && rb[i] == b'.' {
        i += 1;
        while i < rb.len() && rb[i].is_ascii_digit() {
            i += 1;
            frac_digits += 1;
        }
    }
    if int_digits == 0 && frac_digits == 0 {
        return f64::NAN;
    }
    if i < rb.len() && (rb[i] == b'e' || rb[i] == b'E') {
        i += 1;
        if i < rb.len() && (rb[i] == b'+' || rb[i] == b'-') {
            i += 1;
        }
        let start = i;
        while i < rb.len() && rb[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return f64::NAN;
        }
    }
    if i != rb.len() {
        return f64::NAN;
    }
    let mut lit = rest.to_owned();
    if lit.starts_with('.') {
        lit.insert(0, '0');
    }
    if lit.ends_with('.') {
        lit.push('0');
    }
    let lit = lit.replace(".e", ".0e").replace(".E", ".0E");
    sign * lit.parse::<f64>().unwrap_or(f64::NAN)
}

/// ToInt32 (§7.1.6) sobre un Number.
pub fn to_int32(n: f64) -> i32 {
    to_uint32(n) as i32
}

/// ToUint32 (§7.1.7).
pub fn to_uint32(n: f64) -> u32 {
    if !n.is_finite() || n == 0.0 {
        return 0;
    }
    let i = n.trunc();
    let m = i.rem_euclid(4294967296.0);
    m as u32
}

/// ToInt16 (§7.1.8).
pub fn to_int16(n: f64) -> i16 {
    to_uint32(n) as u16 as i16
}

/// ToUint16 (§7.1.9).
pub fn to_uint16(n: f64) -> u16 {
    to_uint32(n) as u16
}

/// ToInt8 (§7.1.10).
pub fn to_int8(n: f64) -> i8 {
    to_uint32(n) as u8 as i8
}

/// ToUint8 (§7.1.11).
pub fn to_uint8(n: f64) -> u8 {
    to_uint32(n) as u8
}

/// ToUint8Clamp (§7.1.12).
pub fn to_uint8_clamp(n: f64) -> u8 {
    if n.is_nan() || n <= 0.0 {
        return 0;
    }
    if n >= 255.0 {
        return 255;
    }
    let f = n.floor();
    if f + 0.5 < n {
        return (f + 1.0) as u8;
    }
    if n < f + 0.5 {
        return f as u8;
    }
    if f as u8 % 2 == 1 {
        f as u8 + 1
    } else {
        f as u8
    }
}

/// ToIntegerOrInfinity sobre un Number (§7.1.5 pasos 2–6).
pub fn integer_or_infinity(n: f64) -> f64 {
    if n.is_nan() || n == 0.0 {
        return 0.0;
    }
    if n.is_infinite() {
        return n;
    }
    let t = n.trunc();
    if t == 0.0 { 0.0 } else { t }
}

impl Agent {
    /// ToPrimitive (§7.1.1).
    pub fn to_primitive(&mut self, v: &Value, hint: Hint) -> JsResult<Value> {
        let Value::Object(o) = v else {
            return Ok(v.clone());
        };
        let key = PropertyKey::Symbol(self.well_known.to_primitive);
        let exotic = self.get_method_of(v, &key)?;
        if let Some(f) = exotic {
            let h = match hint {
                Hint::Default => "default",
                Hint::String => "string",
                Hint::Number => "number",
            };
            let r = self.call(&f, v.clone(), &[Value::str(h)])?;
            if r.is_object() {
                return self.type_error("Cannot convert object to primitive value");
            }
            return Ok(r);
        }
        let hint = if hint == Hint::String {
            Hint::String
        } else {
            Hint::Number
        };
        self.ordinary_to_primitive(*o, hint)
    }

    /// OrdinaryToPrimitive (§7.1.1.1).
    pub fn ordinary_to_primitive(&mut self, o: Gc<Object>, hint: Hint) -> JsResult<Value> {
        let names = if hint == Hint::String {
            [self.keys.to_string.clone(), self.keys.value_of.clone()]
        } else {
            [self.keys.value_of.clone(), self.keys.to_string.clone()]
        };
        for name in names {
            let m = self.get(o, &name, Value::Object(o))?;
            if self.is_callable(&m) {
                let r = self.call(&m, Value::Object(o), &[])?;
                if !r.is_object() {
                    return Ok(r);
                }
            }
        }
        self.type_error("Cannot convert object to primitive value")
    }

    /// ToNumeric (§7.1.3): un Number o un BigInt.
    pub fn to_numeric(&mut self, v: &Value) -> JsResult<Value> {
        let p = self.to_primitive(v, Hint::Number)?;
        if let Value::BigInt(_) = p {
            return Ok(p);
        }
        Ok(Value::Number(self.to_number(&p)?))
    }

    /// ToNumber (§7.1.4).
    pub fn to_number(&mut self, v: &Value) -> JsResult<f64> {
        match v {
            Value::Undefined => Ok(f64::NAN),
            Value::Null => Ok(0.0),
            Value::Boolean(b) => Ok(if *b { 1.0 } else { 0.0 }),
            Value::Number(n) => Ok(*n),
            Value::String(s) => Ok(string_to_number(s.units())),
            Value::Symbol(_) => self.type_error("Cannot convert a Symbol value to a number"),
            Value::BigInt(_) => self.type_error("Cannot convert a BigInt value to a number"),
            Value::Object(_) => {
                let p = self.to_primitive(v, Hint::Number)?;
                self.to_number(&p)
            }
        }
    }

    /// ToIntegerOrInfinity (§7.1.5).
    pub fn to_integer_or_infinity(&mut self, v: &Value) -> JsResult<f64> {
        let n = self.to_number(v)?;
        Ok(integer_or_infinity(n))
    }

    pub fn to_int32(&mut self, v: &Value) -> JsResult<i32> {
        Ok(to_int32(self.to_number(v)?))
    }

    pub fn to_uint32(&mut self, v: &Value) -> JsResult<u32> {
        Ok(to_uint32(self.to_number(v)?))
    }

    /// ToString (§7.1.17).
    pub fn to_string(&mut self, v: &Value) -> JsResult<JsString> {
        match v {
            Value::Undefined => Ok(JsString::from("undefined")),
            Value::Null => Ok(JsString::from("null")),
            Value::Boolean(b) => Ok(JsString::from(if *b { "true" } else { "false" })),
            Value::Number(n) => Ok(number_to_string(*n)),
            Value::String(s) => Ok(s.clone()),
            Value::Symbol(_) => self.type_error("Cannot convert a Symbol value to a string"),
            Value::BigInt(b) => Ok(JsString::from(self.heap[*b].to_string_radix(10).as_str())),
            Value::Object(_) => {
                let p = self.to_primitive(v, Hint::String)?;
                self.to_string(&p)
            }
        }
    }

    /// ToObject (§7.1.18).
    pub fn to_object(&mut self, v: &Value) -> JsResult<Gc<Object>> {
        let i = self.intrinsics();
        let (proto, kind) = match v {
            Value::Object(o) => return Ok(*o),
            Value::Undefined | Value::Null => {
                return self.type_error(&format!(
                    "Cannot convert {} to object",
                    if v.is_null() { "null" } else { "undefined" }
                ));
            }
            Value::Boolean(b) => (i.boolean_prototype, ObjectKind::Boolean(*b)),
            Value::Number(n) => (i.number_prototype, ObjectKind::Number(*n)),
            Value::String(s) => (i.string_prototype, ObjectKind::String(s.clone())),
            Value::Symbol(s) => (i.symbol_prototype, ObjectKind::Symbol(*s)),
            Value::BigInt(b) => (i.bigint_prototype, ObjectKind::BigInt(*b)),
        };
        let o = self.heap.alloc(Object::new(Some(proto), kind));
        if let Value::String(s) = v {
            // El `length` de un String exótico (§10.4.3.4 StringCreate).
            let k = self.keys.length.clone();
            self.put_data(o, k, Value::Number(s.len() as f64), false, false, false);
        }
        Ok(o)
    }

    /// ToPropertyKey (§7.1.19).
    pub fn to_property_key(&mut self, v: &Value) -> JsResult<PropertyKey> {
        match v {
            Value::String(s) => Ok(PropertyKey::from_js_string(s.clone())),
            Value::Symbol(s) => Ok(PropertyKey::Symbol(*s)),
            Value::Number(n) => Ok(PropertyKey::from_number(*n)),
            _ => {
                let p = self.to_primitive(v, Hint::String)?;
                if let Value::Symbol(s) = p {
                    return Ok(PropertyKey::Symbol(s));
                }
                let s = self.to_string(&p)?;
                Ok(PropertyKey::from_js_string(s))
            }
        }
    }

    /// ToLength (§7.1.20).
    pub fn to_length(&mut self, v: &Value) -> JsResult<f64> {
        let len = self.to_integer_or_infinity(v)?;
        if len <= 0.0 {
            return Ok(0.0);
        }
        Ok(len.min(9007199254740991.0))
    }

    /// ToIndex (§7.1.22).
    pub fn to_index(&mut self, v: &Value) -> JsResult<u64> {
        let i = self.to_integer_or_infinity(v)?;
        if !(0.0..=9007199254740991.0).contains(&i) {
            return self.range_error("Invalid index");
        }
        Ok(i as u64)
    }

    /// CanonicalNumericIndexString (§7.1.21).
    pub fn canonical_numeric_index_string(&self, s: &JsString) -> Option<f64> {
        if s.units() == [u16::from(b'-'), u16::from(b'0')] {
            return Some(-0.0);
        }
        let n = string_to_number(s.units());
        if number_to_string(n) == *s {
            Some(n)
        } else {
            None
        }
    }

    /// `typeof` (§13.5.3).
    pub fn type_of(&self, v: &Value) -> &'static str {
        match v {
            Value::Undefined => "undefined",
            Value::Null => "object",
            Value::Boolean(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Symbol(_) => "symbol",
            Value::BigInt(_) => "bigint",
            Value::Object(o) => {
                if matches!(self.heap[*o].kind, ObjectKind::HtmlDda) {
                    // B.3.6.3
                    "undefined"
                } else if self.heap[*o].is_callable() {
                    "function"
                } else {
                    "object"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> f64 {
        string_to_number(&s.encode_utf16().collect::<Vec<_>>())
    }

    #[test]
    fn string_numeric_literal() {
        assert_eq!(n(""), 0.0);
        assert_eq!(n("  12  "), 12.0);
        assert_eq!(n("0x1F"), 31.0);
        assert_eq!(n("0b11"), 3.0);
        assert_eq!(n("0o17"), 15.0);
        assert!(n("-0x1").is_nan());
        assert_eq!(n("-Infinity"), f64::NEG_INFINITY);
        assert!(n("infinity").is_nan());
        assert_eq!(n(".5"), 0.5);
        assert_eq!(n("5."), 5.0);
        assert_eq!(n("1e3"), 1000.0);
        assert!(n("1_000").is_nan());
        assert!(n("1e").is_nan());
        assert!(n(".").is_nan());
        assert_eq!(n("\u{FEFF}\u{2028}7\u{3000}"), 7.0);
        assert!(n("inf").is_nan());
        assert!(n("NaN").is_nan());
        assert_eq!(to_int32(4294967296.0 + 5.0), 5);
        assert_eq!(to_int32(-1.0), -1);
        assert_eq!(to_uint32(-1.0), 4294967295);
        assert_eq!(to_uint8_clamp(2.5), 2);
        assert_eq!(to_uint8_clamp(3.5), 4);
    }
}
