//! Operaciones de prueba y comparación (§7.2).

use std::cmp::Ordering;

use crate::abstract_ops::conversion::Hint;
use crate::bigint::BigInt;
use crate::heap::Gc;
use crate::object::{Object, ObjectKind};
use crate::value::Value;
use crate::vm::{Agent, JsResult};

impl Agent {
    /// RequireObjectCoercible (§7.2.1).
    pub fn require_object_coercible(&mut self, v: &Value) -> JsResult<()> {
        if v.is_nullish() {
            return self.type_error(&format!(
                "Cannot convert {} to object",
                if v.is_null() { "null" } else { "undefined" }
            ));
        }
        Ok(())
    }

    /// IsArray (§7.2.2) (sin Proxy todavía).
    pub fn is_array(&mut self, v: &Value) -> JsResult<bool> {
        match v {
            Value::Object(o) => match &self.heap[*o].kind {
                // Un Proxy es array si su target lo es (paso 3).
                ObjectKind::Proxy(p) => match p.target {
                    Some(t) => self.is_array(&Value::Object(t)),
                    None => self.type_error("Cannot perform 'IsArray' on a revoked proxy"),
                },
                _ => Ok(self.heap[*o].is_array()),
            },
            _ => Ok(false),
        }
    }

    /// IsCallable (§7.2.3).
    pub fn is_callable(&self, v: &Value) -> bool {
        matches!(v, Value::Object(o) if self.heap[*o].is_callable())
    }

    /// IsConstructor (§7.2.4).
    pub fn is_constructor(&self, v: &Value) -> bool {
        matches!(v, Value::Object(o) if self.heap[*o].is_constructor())
    }

    /// IsRegExp (§7.2.8), mínimo: sin RegExp todavía solo mira `@@match`.
    pub fn is_regexp(&mut self, v: &Value) -> JsResult<bool> {
        let Value::Object(o) = v else {
            return Ok(false);
        };
        let key = crate::value::PropertyKey::Symbol(self.well_known.match_);
        let m = self.get(*o, &key, v.clone())?;
        if !m.is_undefined() {
            return Ok(crate::abstract_ops::conversion::to_boolean(self, &m));
        }
        // E2: paso 4, `[[RegExpMatcher]]`.
        Ok(crate::builtins::regexp::has_regexp_matcher(self, *o))
    }

    /// SameValue (§7.2.10).
    pub fn same_value(&self, x: &Value, y: &Value) -> bool {
        match (x, y) {
            (Value::Number(a), Value::Number(b)) => {
                if a.is_nan() && b.is_nan() {
                    return true;
                }
                a == b && a.is_sign_negative() == b.is_sign_negative()
            }
            _ => self.same_value_non_number(x, y),
        }
    }

    /// SameValueZero (§7.2.11).
    pub fn same_value_zero(&self, x: &Value, y: &Value) -> bool {
        match (x, y) {
            (Value::Number(a), Value::Number(b)) => (a.is_nan() && b.is_nan()) || a == b,
            _ => self.same_value_non_number(x, y),
        }
    }

    /// SameValueNonNumber (§7.2.12).
    pub fn same_value_non_number(&self, x: &Value, y: &Value) -> bool {
        match (x, y) {
            (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => true,
            (Value::Boolean(a), Value::Boolean(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Symbol(a), Value::Symbol(b)) => a == b,
            (Value::Object(a), Value::Object(b)) => a == b,
            (Value::BigInt(a), Value::BigInt(b)) => self.heap[*a] == self.heap[*b],
            _ => false,
        }
    }

    /// IsStrictlyEqual (§7.2.16).
    pub fn is_strictly_equal(&self, x: &Value, y: &Value) -> bool {
        match (x, y) {
            (Value::Number(a), Value::Number(b)) => a == b,
            _ => self.same_value_non_number(x, y),
        }
    }

    /// IsLooselyEqual (§7.2.15).
    pub fn is_loosely_equal(&mut self, x: &Value, y: &Value) -> JsResult<bool> {
        // B.3.6.2: [[IsHTMLDDA]] == undefined/null.
        let dda = |v: &Value| matches!(v, Value::Object(o) if matches!(self.heap[*o].kind, ObjectKind::HtmlDda));
        if (dda(x) && y.is_nullish()) || (dda(y) && x.is_nullish()) {
            return Ok(true);
        }
        match (x, y) {
            (Value::Number(_), Value::Number(_))
            | (Value::String(_), Value::String(_))
            | (Value::Boolean(_), Value::Boolean(_))
            | (Value::Symbol(_), Value::Symbol(_))
            | (Value::Object(_), Value::Object(_))
            | (Value::BigInt(_), Value::BigInt(_))
            | (Value::Undefined, Value::Undefined)
            | (Value::Null, Value::Null) => Ok(self.is_strictly_equal(x, y)),
            (Value::Undefined, Value::Null) | (Value::Null, Value::Undefined) => Ok(true),
            (Value::Number(a), Value::String(s)) => {
                Ok(*a == crate::abstract_ops::conversion::string_to_number(s.units()))
            }
            (Value::String(s), Value::Number(b)) => {
                Ok(crate::abstract_ops::conversion::string_to_number(s.units()) == *b)
            }
            (Value::BigInt(b), Value::String(s)) | (Value::String(s), Value::BigInt(b)) => {
                let Some(text) = s.to_str() else {
                    return Ok(false);
                };
                match BigInt::from_string(&text) {
                    Some(n) => Ok(n == self.heap[*b]),
                    None => Ok(false),
                }
            }
            (Value::Boolean(b), _) => {
                let n = Value::Number(if *b { 1.0 } else { 0.0 });
                self.is_loosely_equal(&n, y)
            }
            (_, Value::Boolean(b)) => {
                let n = Value::Number(if *b { 1.0 } else { 0.0 });
                self.is_loosely_equal(x, &n)
            }
            (
                Value::String(_) | Value::Number(_) | Value::BigInt(_) | Value::Symbol(_),
                Value::Object(_),
            ) => {
                let p = self.to_primitive(y, Hint::Default)?;
                self.is_loosely_equal(x, &p)
            }
            (
                Value::Object(_),
                Value::String(_) | Value::Number(_) | Value::BigInt(_) | Value::Symbol(_),
            ) => {
                let p = self.to_primitive(x, Hint::Default)?;
                self.is_loosely_equal(&p, y)
            }
            (Value::BigInt(a), Value::Number(n)) | (Value::Number(n), Value::BigInt(a)) => {
                if !n.is_finite() {
                    return Ok(false);
                }
                Ok(self.heap[*a].compare_f64(*n) == Ordering::Equal)
            }
            _ => Ok(false),
        }
    }

    /// IsLessThan (§7.2.14): `Some(bool)` o `None` (undefined, con NaN).
    pub fn is_less_than(
        &mut self,
        x: &Value,
        y: &Value,
        left_first: bool,
    ) -> JsResult<Option<bool>> {
        let (px, py) = if left_first {
            let px = self.to_primitive(x, Hint::Number)?;
            let py = self.to_primitive(y, Hint::Number)?;
            (px, py)
        } else {
            let py = self.to_primitive(y, Hint::Number)?;
            let px = self.to_primitive(x, Hint::Number)?;
            (px, py)
        };
        if let (Value::String(a), Value::String(b)) = (&px, &py) {
            return Ok(Some(a.units() < b.units()));
        }
        if let (Value::BigInt(a), Value::String(b)) = (&px, &py) {
            let Some(n) = b.to_str().and_then(|t| BigInt::from_string(&t)) else {
                return Ok(None);
            };
            return Ok(Some(self.heap[*a].compare(&n) == Ordering::Less));
        }
        if let (Value::String(a), Value::BigInt(b)) = (&px, &py) {
            let Some(n) = a.to_str().and_then(|t| BigInt::from_string(&t)) else {
                return Ok(None);
            };
            return Ok(Some(n.compare(&self.heap[*b]) == Ordering::Less));
        }
        let nx = self.to_numeric(&px)?;
        let ny = self.to_numeric(&py)?;
        match (&nx, &ny) {
            (Value::Number(a), Value::Number(b)) => {
                if a.is_nan() || b.is_nan() {
                    Ok(None)
                } else {
                    Ok(Some(a < b))
                }
            }
            (Value::BigInt(a), Value::BigInt(b)) => Ok(Some(
                self.heap[*a].compare(&self.heap[*b]) == Ordering::Less,
            )),
            (Value::BigInt(a), Value::Number(b)) => {
                if b.is_nan() {
                    return Ok(None);
                }
                Ok(Some(self.heap[*a].compare_f64(*b) == Ordering::Less))
            }
            (Value::Number(a), Value::BigInt(b)) => {
                if a.is_nan() {
                    return Ok(None);
                }
                Ok(Some(self.heap[*b].compare_f64(*a) == Ordering::Greater))
            }
            _ => Ok(None),
        }
    }

    /// IsIntegralNumber (§7.2.6 en ES2022; hoy "is an integral Number").
    pub fn is_integral_number(&self, v: &Value) -> bool {
        matches!(v, Value::Number(n) if n.is_finite() && n.trunc() == *n)
    }

    /// InstanceofOperator (§13.10.2).
    pub fn instanceof_operator(&mut self, v: &Value, target: &Value) -> JsResult<bool> {
        let Value::Object(t) = target else {
            return self.type_error("Right-hand side of 'instanceof' is not an object");
        };
        let key = crate::value::PropertyKey::Symbol(self.well_known.has_instance);
        let handler = self.get_method_of(target, &key)?;
        if let Some(h) = handler {
            let r = self.call(&h, target.clone(), std::slice::from_ref(v))?;
            return Ok(crate::abstract_ops::conversion::to_boolean(self, &r));
        }
        if !self.heap[*t].is_callable() {
            return self.type_error("Right-hand side of 'instanceof' is not callable");
        }
        self.ordinary_has_instance(target, v)
    }

    /// OrdinaryHasInstance (§7.3.22).
    pub fn ordinary_has_instance(&mut self, c: &Value, o: &Value) -> JsResult<bool> {
        let Value::Object(cf) = c else {
            return Ok(false);
        };
        if !self.heap[*cf].is_callable() {
            return Ok(false);
        }
        if let ObjectKind::Bound(b) = &self.heap[*cf].kind {
            let t = Value::Object(b.target);
            return self.instanceof_operator(o, &t);
        }
        let Value::Object(mut obj) = o.clone() else {
            return Ok(false);
        };
        let k = self.keys.prototype.clone();
        let p = self.get(*cf, &k, c.clone())?;
        let Value::Object(p) = p else {
            return self.type_error("Function has non-object prototype in instanceof check");
        };
        loop {
            match self.get_prototype_of(obj)? {
                None => return Ok(false),
                Some(x) if x == p => return Ok(true),
                Some(x) => obj = x,
            }
        }
    }

    /// ¿`o` tiene `[[ErrorData]]`? (Error.prototype.toString y amigos.)
    pub fn is_error_object(&self, o: Gc<Object>) -> bool {
        matches!(self.heap[o].kind, ObjectKind::Error)
    }
}
