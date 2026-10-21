//! Operaciones abstractas (ECMA-262 §7): conversiones, pruebas y comparaciones, operaciones
//! sobre objetos e iteradores, y los operadores binarios (§13.15.3).

pub mod conversion;
pub mod iterator;
pub mod number;
pub mod operations;
pub mod testing;

use crate::compiler::bytecode::Op;
use crate::string::JsString;
use crate::value::Value;
use crate::vm::{Agent, JsResult};
use conversion::{Hint, to_int32, to_uint32};

impl Agent {
    /// ApplyStringOrNumericBinaryOperator (§13.15.3).
    pub fn apply_binary(&mut self, op: Op, l: Value, r: Value) -> JsResult<Value> {
        if op == Op::Add {
            let lp = self.to_primitive(&l, Hint::Default)?;
            let rp = self.to_primitive(&r, Hint::Default)?;
            if matches!(lp, Value::String(_)) || matches!(rp, Value::String(_)) {
                let ls = self.to_string(&lp)?;
                let rs = self.to_string(&rp)?;
                // Un resultado largo es un rope (E3).
                return match JsString::concat_strings(&ls, &rs) {
                    Some(s) => Ok(Value::String(s)),
                    None => self.range_error("Invalid string length"),
                };
            }
            return self.numeric_op(op, &lp, &rp);
        }
        self.numeric_op(op, &l, &r)
    }

    fn numeric_op(&mut self, op: Op, l: &Value, r: &Value) -> JsResult<Value> {
        let ln = self.to_numeric(l)?;
        let rn = self.to_numeric(r)?;
        let (a, b) = match (&ln, &rn) {
            (Value::Number(a), Value::Number(b)) => (*a, *b),
            // E2
            (Value::BigInt(a), Value::BigInt(b)) => {
                return crate::builtins::bigint::binary(self, op, *a, *b);
            }
            _ => {
                return self
                    .type_error("Cannot mix BigInt and other types, use explicit conversions");
            }
        };
        Ok(Value::Number(match op {
            Op::Add => a + b,
            Op::Sub => a - b,
            Op::Mul => a * b,
            Op::Div => a / b,
            Op::Mod => number::remainder(a, b),
            Op::Exp => number::exponentiate(a, b),
            // Number::leftShift / signedRightShift / unsignedRightShift (§6.1.6.1.9–11).
            Op::Shl => f64::from(to_int32(a).wrapping_shl(to_uint32(b) & 31)),
            Op::Shr => f64::from(to_int32(a) >> (to_uint32(b) & 31)),
            Op::UShr => f64::from(to_uint32(a) >> (to_uint32(b) & 31)),
            Op::BitAnd => f64::from(to_int32(a) & to_int32(b)),
            Op::BitOr => f64::from(to_int32(a) | to_int32(b)),
            Op::BitXor => f64::from(to_int32(a) ^ to_int32(b)),
            _ => unreachable!("operador {op:?}"),
        }))
    }
}
