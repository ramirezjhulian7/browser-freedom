//! %GeneratorFunction% (§27.3), %GeneratorFunction.prototype% y %GeneratorPrototype%
//! (§27.5).

use crate::builtins::{FnEntry, methods, to_string_tag};
use crate::value::Value;
use crate::vm::eval::DynamicKind;
use crate::vm::function::CallArgs;
use crate::vm::generator::generator_resume;
use crate::vm::interp::Resume;
use crate::vm::{Agent, JsResult, RealmId};

pub fn init(agent: &mut Agent, realm: RealmId) {
    let i = agent.realms[realm].intrinsics.clone();
    // %GeneratorFunction.prototype% (§27.3.3).
    let gfp = agent.new_object(Some(i.function_prototype));
    // %GeneratorPrototype% (§27.5.1).
    let gp = agent.new_object(Some(i.iterator_prototype));
    const PROTO: &[FnEntry] = &[
        ("next", 1, next),
        ("return", 1, return_),
        ("throw", 1, throw),
    ];
    methods(agent, gp, PROTO);
    to_string_tag(agent, gp, "Generator");
    let k = agent.keys.constructor.clone();
    agent.put_data(gp, k, Value::Object(gfp), false, false, true);
    let k = agent.keys.prototype.clone();
    agent.put_data(gfp, k, Value::Object(gp), false, false, true);
    to_string_tag(agent, gfp, "GeneratorFunction");
    // %GeneratorFunction% (§27.3.1).
    let gf = agent.create_builtin_function(generator_function, 1, "GeneratorFunction", true);
    agent.heap[gf].proto = Some(i.function_ctor);
    let k = agent.keys.prototype.clone();
    agent.put_data(gf, k, Value::Object(gfp), false, false, false);
    let k = agent.keys.constructor.clone();
    agent.put_data(gfp, k, Value::Object(gf), false, false, true);
    let ints = &mut agent.realms[realm].intrinsics;
    ints.generator_function = gf;
    ints.generator_function_prototype = gfp;
    ints.generator_prototype = gp;
}

fn generator_function(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    agent.create_dynamic_function(a.callee, a.new_target, DynamicKind::Generator, a.args)
}

/// %GeneratorPrototype%.next (§27.5.1.2).
fn next(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    generator_resume(agent, &a.this, Resume::Next(a.arg(0)))
}

/// %GeneratorPrototype%.return (§27.5.1.3).
fn return_(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    generator_resume(agent, &a.this, Resume::Return(a.arg(0)))
}

/// %GeneratorPrototype%.throw (§27.5.1.4).
fn throw(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    generator_resume(agent, &a.this, Resume::Throw(a.arg(0)))
}
