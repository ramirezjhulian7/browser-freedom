//! Math (§21.3): en esta etapa solo `Math.pow`, que usa `propertyHelper.js` del harness de
//! test262; el resto del objeto llega con los built-ins del paso 3.

use crate::abstract_ops::number::exponentiate;
use crate::builtins::{method, to_string_tag};
use crate::value::Value;
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

pub fn init(agent: &mut Agent, realm: RealmId) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    let m = agent.new_object(Some(op));
    method(agent, m, "pow", 2, pow);
    to_string_tag(agent, m, "Math");
    agent.realms[realm].intrinsics.math = m;
}

/// Math.pow (§21.3.2.26).
fn pow(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let b = agent.to_number(&a.arg(0))?;
    let e = agent.to_number(&a.arg(1))?;
    Ok(Value::Number(exponentiate(b, e)))
}
