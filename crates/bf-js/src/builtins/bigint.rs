//! BigInt (§21.2): el constructor, `BigInt.asIntN`/`asUintN`, BigInt.prototype, las
//! conversiones ToBigInt / ToBigInt64 / ToBigUint64 (§7.1.13–7.1.16) y los operadores del
//! tipo para la VM (§6.1.6.2, con los errores de división por cero, exponente negativo,
//! `>>>` y tamaño máximo).

use crate::abstract_ops::conversion::Hint;
use crate::bigint::BigInt;
use crate::builtins::{FnEntry, constructor, methods};
use crate::compiler::bytecode::Op;
use crate::heap::Gc;
use crate::object::ObjectKind;
use crate::value::Value;
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

pub fn init(agent: &mut Agent, realm: RealmId) {
    // %BigInt.prototype% (con su @@toStringTag) ya existe: se completa.
    let proto = agent.realms[realm].intrinsics.bigint_prototype;
    let ctor = constructor(agent, "BigInt", 1, bigint_ctor, proto);
    agent.realms[realm].intrinsics.bigint_ctor = ctor;
    crate::builtins::register_global(agent, realm, "BigInt", ctor);
    const STATICS: &[FnEntry] = &[("asIntN", 2, as_int_n), ("asUintN", 2, as_uint_n)];
    methods(agent, ctor, STATICS);
    const PROTO: &[FnEntry] = &[
        ("toLocaleString", 0, to_locale_string),
        ("toString", 0, to_string),
        ("valueOf", 0, value_of),
    ];
    methods(agent, proto, PROTO);
}

fn alloc(agent: &mut Agent, b: BigInt) -> Value {
    Value::BigInt(agent.heap.alloc(b))
}

fn too_big<T>(agent: &mut Agent) -> JsResult<T> {
    agent.range_error("Maximum BigInt size exceeded")
}

impl Agent {
    /// ToBigInt ( argument ) (§7.1.13).
    pub fn to_bigint(&mut self, v: &Value) -> JsResult<Gc<BigInt>> {
        let prim = self.to_primitive(v, Hint::Number)?;
        match prim {
            Value::BigInt(b) => Ok(b),
            Value::Boolean(b) => Ok(self.heap.alloc(BigInt::from_u64(u64::from(b)))),
            Value::String(s) => match s.to_str().and_then(|t| BigInt::from_string(&t)) {
                Some(b) => Ok(self.heap.alloc(b)),
                None => self.syntax_error(&format!(
                    "Cannot convert {} to a BigInt",
                    s.to_string_lossy()
                )),
            },
            Value::Undefined | Value::Null => {
                self.type_error(&format!("Cannot convert {} to a BigInt", prim.type_name()))
            }
            Value::Number(n) => self.type_error(&format!(
                "Cannot convert {} to a BigInt",
                crate::abstract_ops::number::number_to_std_string(n)
            )),
            Value::Symbol(_) => self.type_error("Cannot convert a Symbol value to a BigInt"),
            Value::Object(_) => unreachable!("ToPrimitive devolvió un objeto"),
        }
    }

    /// ToBigInt64 ( argument ) (§7.1.15).
    pub fn to_big_int64(&mut self, v: &Value) -> JsResult<i64> {
        let b = self.to_bigint(v)?;
        Ok(self.heap[b].to_u64_wrapping() as i64)
    }

    /// ToBigUint64 ( argument ) (§7.1.16).
    pub fn to_big_uint64(&mut self, v: &Value) -> JsResult<u64> {
        let b = self.to_bigint(v)?;
        Ok(self.heap[b].to_u64_wrapping())
    }
}

/// NumberToBigInt ( number ) (§21.2.1.1.1).
fn number_to_bigint(agent: &mut Agent, n: f64) -> JsResult<Value> {
    if !n.is_finite() || n.fract() != 0.0 {
        return agent.range_error(&format!(
            "The number {} cannot be converted to a BigInt because it is not an integer",
            crate::abstract_ops::number::number_to_std_string(n)
        ));
    }
    Ok(alloc(agent, BigInt::from_integer_f64(n)))
}

/// BigInt ( value ) (§21.2.1.1).
fn bigint_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    if a.new_target.is_some() {
        return agent.type_error("BigInt is not a constructor");
    }
    let prim = agent.to_primitive(&a.arg(0), Hint::Number)?;
    if let Value::Number(n) = prim {
        return number_to_bigint(agent, n);
    }
    Ok(Value::BigInt(agent.to_bigint(&prim)?))
}

/// BigInt.asIntN ( bits, bigint ) (§21.2.2.1).
fn as_int_n(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let bits = agent.to_index(&a.arg(0))?;
    let b = agent.to_bigint(&a.arg(1))?;
    match agent.heap[b].as_int_n(bits) {
        Some(r) => Ok(alloc(agent, r)),
        None => too_big(agent),
    }
}

/// BigInt.asUintN ( bits, bigint ) (§21.2.2.2).
fn as_uint_n(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let bits = agent.to_index(&a.arg(0))?;
    let b = agent.to_bigint(&a.arg(1))?;
    match agent.heap[b].as_uint_n(bits) {
        Some(r) => Ok(alloc(agent, r)),
        None => too_big(agent),
    }
}

/// ThisBigIntValue ( value ) (§21.2.3.4.1).
fn this_bigint(agent: &mut Agent, v: &Value, method: &str) -> JsResult<Gc<BigInt>> {
    match v {
        Value::BigInt(b) => Ok(*b),
        Value::Object(o) => match agent.heap[*o].kind {
            ObjectKind::BigInt(b) => Ok(b),
            _ => agent.type_error(&format!(
                "BigInt.prototype.{method} requires that 'this' be a BigInt"
            )),
        },
        _ => agent.type_error(&format!(
            "BigInt.prototype.{method} requires that 'this' be a BigInt"
        )),
    }
}

/// BigInt.prototype.toString ( [ radix ] ) (§21.2.3.3).
fn to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let x = this_bigint(agent, &a.this, "toString")?;
    let radix = a.arg(0);
    let r = if radix.is_undefined() {
        10.0
    } else {
        agent.to_integer_or_infinity(&radix)?
    };
    if !(2.0..=36.0).contains(&r) {
        return agent.range_error("toString() radix must be between 2 and 36");
    }
    Ok(Value::str(&agent.heap[x].to_string_radix(r as u32)))
}

/// BigInt.prototype.toLocaleString ( [ reserved1 [ , reserved2 ] ] ) (§21.2.3.2), sin
/// ECMA-402: como `toString()`.
fn to_locale_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let x = this_bigint(agent, &a.this, "toLocaleString")?;
    let d = crate::builtins::intl::decimal::Decimal::from_bigint(&agent.heap[x]);
    crate::builtins::intl::number_to_locale_string(
        agent,
        crate::builtins::intl::number_format_value::Value::BigInt(d),
        &a.arg(0),
        &a.arg(1),
    )
}

/// BigInt.prototype.valueOf ( ) (§21.2.3.4).
fn value_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::BigInt(this_bigint(agent, &a.this, "valueOf")?))
}

// --- Operadores (§6.1.6.2) ---

/// Un operador binario entre dos BigInt (la rama BigInt de
/// ApplyStringOrNumericBinaryOperator, §13.15.3).
pub fn binary(agent: &mut Agent, op: Op, a: Gc<BigInt>, b: Gc<BigInt>) -> JsResult<Value> {
    const TOO_BIG: &str = "Maximum BigInt size exceeded";
    const DIV_ZERO: &str = "Division by zero";
    let r: Result<BigInt, (bool, &str)> = {
        let (x, y) = (&agent.heap[a], &agent.heap[b]);
        match op {
            Op::Add => Ok(x.add(y)),
            Op::Sub => Ok(x.sub(y)),
            Op::Mul => Ok(x.mul(y)),
            Op::Div => x.div(y).ok_or((true, DIV_ZERO)),
            Op::Mod => x.rem(y).ok_or((true, DIV_ZERO)),
            Op::Exp if y.negative => Err((true, "Exponent must be non-negative")),
            Op::Exp => x.pow(y).ok_or((true, TOO_BIG)),
            Op::Shl => x.shl(y).ok_or((true, TOO_BIG)),
            Op::Shr => x.shr(y).ok_or((true, TOO_BIG)),
            Op::UShr => Err((
                false,
                "BigInts have no unsigned right shift, use >> instead",
            )),
            Op::BitAnd => Ok(x.bitand(y)),
            Op::BitOr => Ok(x.bitor(y)),
            Op::BitXor => Ok(x.bitxor(y)),
            _ => unreachable!("operador {op:?}"),
        }
    };
    match r {
        Ok(r) => Ok(alloc(agent, r)),
        Err((true, m)) => agent.range_error(m),
        Err((false, m)) => agent.type_error(m),
    }
}

/// BigInt::unaryMinus (§6.1.6.2.1).
pub fn unary_minus(agent: &mut Agent, a: Gc<BigInt>) -> Value {
    let r = agent.heap[a].neg();
    alloc(agent, r)
}

/// BigInt::bitwiseNOT (§6.1.6.2.2).
pub fn bitwise_not(agent: &mut Agent, a: Gc<BigInt>) -> Value {
    let r = agent.heap[a].bitnot();
    alloc(agent, r)
}

/// `++`/`--` sobre un BigInt (§13.4.2.1 y siguientes: BigInt::add con 1ℤ).
pub fn increment(agent: &mut Agent, a: Gc<BigInt>, delta: i64) -> Value {
    let r = agent.heap[a].add(&BigInt::from_i64(delta));
    alloc(agent, r)
}
