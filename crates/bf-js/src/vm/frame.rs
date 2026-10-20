//! Los frames del intérprete: un execution context (§9.4) de código ECMAScript.

use std::collections::HashSet;
use std::rc::Rc;

use crate::compiler::bytecode::FunctionCode;
use crate::heap::{Gc, Trace, Tracer};
use crate::object::Object;
use crate::string::Atom;
use crate::value::Value;
use crate::vm::RealmId;
use crate::vm::env::Env;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlerKind {
    Catch,
    /// La completion se guarda en (clase, valor).
    Finally {
        kind_slot: u32,
        value_slot: u32,
    },
}

#[derive(Debug, Clone)]
pub struct Handler {
    pub kind: HandlerKind,
    pub pc: u32,
    pub stack_len: u32,
    pub refs_len: u32,
    pub env: Gc<Env>,
}

#[derive(Debug)]
pub struct Frame {
    pub code: Rc<FunctionCode>,
    pub pc: usize,
    pub stack: Vec<Value>,
    pub locals: Vec<Option<Value>>,
    /// LexicalEnvironment.
    pub env: Gc<Env>,
    /// VariableEnvironment (para `eval` no estricto); `None` = el Global Environment.
    pub var_env: Option<Gc<Env>>,
    /// El Function Environment de esta función, si se materializó.
    pub function_env: Option<Gc<Env>>,
    pub handlers: Vec<Handler>,
    /// Referencias resueltas dinámicamente: el environment donde está el binding (`None` si
    /// no se resolvió).
    pub refs: Vec<Option<Gc<Env>>>,
    /// `None`: sin inicializar (constructor derivado antes de `super()`).
    pub this: Option<Value>,
    pub callee: Option<Gc<Object>>,
    pub new_target: Option<Gc<Object>>,
    pub args: Vec<Value>,
    pub realm: RealmId,
    /// Funciones de bloque elevadas por B.3.2.2/B.3.2.3 en este código.
    pub hoisted: Option<Rc<HashSet<Atom>>>,
}

impl Frame {
    pub fn new(code: Rc<FunctionCode>, env: Gc<Env>, realm: RealmId) -> Frame {
        let n = code.num_locals as usize;
        Frame {
            code,
            pc: 0,
            stack: Vec::with_capacity(16),
            locals: vec![None; n],
            env,
            var_env: None,
            function_env: None,
            handlers: Vec::new(),
            refs: Vec::new(),
            this: Some(Value::Undefined),
            callee: None,
            new_target: None,
            args: Vec::new(),
            realm,
            hoisted: None,
        }
    }
}

impl Trace for Frame {
    fn trace(&self, t: &mut Tracer) {
        for v in &self.stack {
            t.value(v);
        }
        for v in &self.locals {
            t.opt_value(v);
        }
        t.gc(self.env);
        t.opt(self.var_env);
        t.opt(self.function_env);
        for h in &self.handlers {
            t.gc(h.env);
        }
        for r in &self.refs {
            t.opt(*r);
        }
        t.opt_value(&self.this);
        t.opt(self.callee);
        t.opt(self.new_target);
        for a in &self.args {
            t.value(a);
        }
    }
}
