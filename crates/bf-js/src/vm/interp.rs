//! El intérprete: ejecuta el frame del tope de la pila hasta que retorna, se suspende
//! (generadores) o lanza una excepción que nadie atrapa en el frame.
//!
//! Las excepciones y los `return` se desenrollan por la pila de handlers del frame: un
//! handler `catch` recibe la excepción en la pila; uno `finally` recibe la completion en sus
//! dos slots (clase y valor) y al terminar el bloque `EndFinally` la continúa.

use std::rc::Rc;

use crate::compiler::bytecode::*;
use crate::heap::Gc;
use crate::object::{Object, ObjectKind, PrivateElement, PropertyDescriptor};
use crate::string::{Atom, JsString};
use crate::value::{PropertyKey, Value};
use crate::vm::env::{Env, EnvKind, FunctionEnvData};
use crate::vm::frame::{Frame, Handler, HandlerKind};
use crate::vm::function::{ClassData, ClassField, Closure, FieldName};
use crate::vm::{Agent, ErrorType, JsResult, Throw};

/// Cómo se reanuda un frame.
#[derive(Debug, Clone)]
pub enum Resume {
    Normal,
    Next(Value),
    Throw(Value),
    Return(Value),
}

/// Cómo terminó (o se suspendió) un frame.
#[derive(Debug)]
pub enum FrameExit {
    Return(Value),
    /// Valor y si se devuelve sin envolver (`yield*`).
    Yield(Value, bool),
    GeneratorStart(Gc<Object>),
    /// `Await(value)` (§27.7.5.3): el frame se suspende hasta que la promesa se asiente.
    Await(Value),
}

enum Flow {
    Next,
    Return(Value),
    Yield(Value, bool),
    GenStart(Gc<Object>),
    Await(Value),
}

macro_rules! frame {
    ($s:ident) => {
        $s.frames.last_mut().expect("frame")
    };
}

impl Agent {
    pub(crate) fn run(&mut self, resume: Resume) -> JsResult<FrameExit> {
        let base = self.frames.len();
        let r = self.run_loop(resume);
        if r.is_err() {
            // Una salida anticipada (terminación) con llamadas en línea pendientes.
            while self.frames.len() > base {
                self.pop_inline_frame();
            }
        }
        r
    }

    /// Saca el frame de una llamada en línea (E3) y restaura el realm del llamador.
    fn pop_inline_frame(&mut self) {
        let f = self.frames.pop().expect("frame");
        self.current_realm = f.caller_realm;
        self.recycle_frame(f);
    }

    fn run_loop(&mut self, resume: Resume) -> JsResult<FrameExit> {
        let mut pending: Option<JsResult<Flow>> = None;
        if !matches!(resume, Resume::Normal) {
            let f = frame!(self);
            let prev = f.pc.checked_sub(1).map(|p| f.code.ops[p]);
            if let Some(Op::YieldRaw {
                kind_slot,
                value_slot,
            }) = prev
            {
                let (k, v) = match resume {
                    Resume::Next(v) => (COMPLETION_NORMAL, v),
                    Resume::Throw(v) => (COMPLETION_THROW, v),
                    Resume::Return(v) => (COMPLETION_RETURN, v),
                    Resume::Normal => unreachable!(),
                };
                f.locals[kind_slot as usize] = Some(Value::Number(f64::from(k)));
                f.locals[value_slot as usize] = Some(v);
            } else {
                match resume {
                    Resume::Next(v) => f.stack.push(v),
                    Resume::Throw(e) => pending = Some(Err(Throw::Value(e))),
                    Resume::Return(v) => pending = Some(Ok(Flow::Return(v))),
                    Resume::Normal => {}
                }
            }
        }
        self.safepoint()?;
        loop {
            let r = match pending.take() {
                Some(r) => r,
                None => {
                    // Una ráfaga de instrucciones que solo tocan el frame (sin llamar a
                    // `step`); la primera que necesita el agente va por `step`.
                    let mut n = 0u64;
                    let mut back = false;
                    {
                        let realm = &self.realms[self.current_realm];
                        let cx = FastCx {
                            genv: realm.global_env,
                            object_proto: realm.intrinsics.object_prototype,
                            array_proto: realm.intrinsics.array_prototype,
                        };
                        let heap = &mut self.heap;
                        let f = self.frames.last_mut().expect("frame");
                        loop {
                            let op = f.code.ops[f.pc];
                            f.pc += 1;
                            match fast_op(f, heap, &cx, op) {
                                Fast::Done => n += 1,
                                Fast::BackJump => {
                                    n += 1;
                                    back = true;
                                    break;
                                }
                                Fast::Slow => {
                                    f.pc -= 1;
                                    break;
                                }
                            }
                        }
                    }
                    self.instructions += n;
                    if back {
                        self.safepoint()?;
                        continue;
                    }
                    self.step()
                }
            };
            match r {
                Ok(Flow::Next) => {}
                Ok(Flow::Return(v)) => {
                    if !self.unwind_return(&v) {
                        if !frame!(self).inline {
                            return Ok(FrameExit::Return(v));
                        }
                        // La vuelta de una llamada en línea: el resultado al llamador.
                        self.pop_inline_frame();
                        self.push(v);
                        self.safepoint()?;
                    }
                }
                Ok(Flow::Yield(v, raw)) => return Ok(FrameExit::Yield(v, raw)),
                Ok(Flow::GenStart(g)) => return Ok(FrameExit::GeneratorStart(g)),
                Ok(Flow::Await(v)) => return Ok(FrameExit::Await(v)),
                Err(Throw::Value(e)) => {
                    // Se busca un handler en el frame actual y, si es una llamada en línea,
                    // en los de abajo.
                    let mut handled = false;
                    loop {
                        if self.unwind_throw(&e) {
                            handled = true;
                            break;
                        }
                        if !frame!(self).inline {
                            break;
                        }
                        self.pop_inline_frame();
                    }
                    if !handled {
                        return Err(Throw::Value(e));
                    }
                }
                Err(t) => return Err(t),
            }
        }
    }

    fn restore_handler(&mut self, h: &Handler) {
        let f = frame!(self);
        f.stack.truncate(h.stack_len as usize);
        f.refs.truncate(h.refs_len as usize);
        f.env = h.env;
        f.pc = h.pc as usize;
    }

    fn unwind_throw(&mut self, e: &Value) -> bool {
        let Some(h) = frame!(self).handlers.pop() else {
            return false;
        };
        self.restore_handler(&h);
        let f = frame!(self);
        match h.kind {
            HandlerKind::Catch => f.stack.push(e.clone()),
            HandlerKind::Finally {
                kind_slot,
                value_slot,
            } => {
                f.locals[kind_slot as usize] = Some(Value::Number(f64::from(COMPLETION_THROW)));
                f.locals[value_slot as usize] = Some(e.clone());
            }
        }
        true
    }

    fn unwind_return(&mut self, v: &Value) -> bool {
        loop {
            let Some(h) = frame!(self).handlers.pop() else {
                return false;
            };
            if let HandlerKind::Finally {
                kind_slot,
                value_slot,
            } = h.kind
            {
                self.restore_handler(&h);
                let f = frame!(self);
                f.locals[kind_slot as usize] = Some(Value::Number(f64::from(COMPLETION_RETURN)));
                f.locals[value_slot as usize] = Some(v.clone());
                return true;
            }
        }
    }

    // --- Ayudas ---

    fn pop(&mut self) -> Value {
        frame!(self).stack.pop().expect("pila vacía")
    }

    fn push(&mut self, v: Value) {
        frame!(self).stack.push(v);
    }

    /// Los dos valores del tope (`a` debajo, `b` arriba) si los dos son `Number`.
    #[inline]
    fn top_numbers(&mut self) -> Option<(f64, f64)> {
        let s = &frame!(self).stack;
        match s.len().checked_sub(2).map(|i| (&s[i], &s[i + 1])) {
            Some((Value::Number(a), Value::Number(b))) => Some((*a, *b)),
            _ => None,
        }
    }

    /// Reemplaza los dos valores del tope por `v`.
    #[inline]
    fn replace_top2(&mut self, v: Value) {
        let s = &mut frame!(self).stack;
        s.pop();
        if let Some(top) = s.last_mut() {
            *top = v;
        }
    }

    fn peek(&self, depth: usize) -> &Value {
        let s = &self.frames.last().expect("frame").stack;
        &s[s.len() - 1 - depth]
    }

    fn name_const(&self, i: u32) -> (Atom, PropertyKey) {
        match &self.frames.last().expect("frame").code.consts[i as usize] {
            Constant::Name(a, k) => (a.clone(), k.clone()),
            other => unreachable!("se esperaba un nombre: {other:?}"),
        }
    }

    fn string_const(&self, i: u32) -> JsString {
        match &self.frames.last().expect("frame").code.consts[i as usize] {
            Constant::String(s) => s.clone(),
            Constant::Name(a, _) => a.to_js_string(),
            other => unreachable!("se esperaba un string: {other:?}"),
        }
    }

    fn env_at(&self, hops: u32) -> Gc<Env> {
        let mut e = self.frames.last().expect("frame").env;
        for _ in 0..hops {
            e = self.heap[e].outer.expect("environment externo");
        }
        e
    }

    fn strict_code(&self) -> bool {
        self.frames.last().expect("frame").code.strict
    }

    fn local(&self, slot: u32) -> Value {
        self.frames.last().expect("frame").locals[slot as usize]
            .clone()
            .unwrap_or_default()
    }

    /// El environment del contexto (Function Environment) según la ubicación.
    fn ctx_env(&self, loc: CtxLoc) -> Option<Gc<Env>> {
        match loc {
            CtxLoc::Frame => None,
            CtxLoc::Env(h) => Some(self.env_at(h)),
            CtxLoc::Dynamic => {
                let start = self.frames.last().expect("frame").env;
                Some(self.get_this_environment(start))
            }
        }
    }

    fn get_ctx(&mut self, what: CtxWhat, loc: CtxLoc) -> JsResult<Value> {
        let env = self.ctx_env(loc);
        let Some(env) = env else {
            let f = self.frames.last().expect("frame");
            return match what {
                CtxWhat::This => match &f.this {
                    Some(v) => Ok(v.clone()),
                    None => self.reference_error(
                        "Must call super constructor in derived class before accessing 'this' or returning from derived constructor",
                    ),
                },
                CtxWhat::NewTarget => Ok(f.new_target.into()),
                CtxWhat::Function => Ok(f.callee.into()),
            };
        };
        match &self.heap[env].kind {
            EnvKind::Declarative(d) => {
                // Module Environment Record (§9.1.1.5.3): `this` es `undefined` (E1).
                let Some(fd) = d.function.as_ref() else {
                    return Ok(Value::Undefined);
                };
                match what {
                    CtxWhat::This => match &fd.this {
                        Some(v) => Ok(v.clone()),
                        None => self.reference_error(
                            "Must call super constructor in derived class before accessing 'this' or returning from derived constructor",
                        ),
                    },
                    CtxWhat::NewTarget => Ok(fd.new_target.into()),
                    CtxWhat::Function => Ok(Value::Object(fd.function)),
                }
            }
            EnvKind::Global(g) => match what {
                CtxWhat::This => Ok(Value::Object(g.this)),
                _ => Ok(Value::Undefined),
            },
            EnvKind::Object { .. } => Ok(Value::Undefined),
        }
    }

    // --- Instrucciones ---

    fn step(&mut self) -> JsResult<Flow> {
        self.instructions += 1;
        let f = frame!(self);
        let op = f.code.ops[f.pc];
        f.pc += 1;
        match op {
            Op::Pop => {
                self.pop();
            }
            Op::Dup => {
                let v = self.peek(0).clone();
                self.push(v);
            }
            Op::Dup2 => {
                let a = self.peek(1).clone();
                let b = self.peek(0).clone();
                self.push(a);
                self.push(b);
            }
            Op::Dup3 => {
                let a = self.peek(2).clone();
                let b = self.peek(1).clone();
                let c = self.peek(0).clone();
                self.push(a);
                self.push(b);
                self.push(c);
            }
            Op::Swap => {
                let s = &mut frame!(self).stack;
                let n = s.len();
                s.swap(n - 1, n - 2);
            }
            Op::Over => {
                let v = self.peek(1).clone();
                self.push(v);
            }
            Op::Pull(n) => {
                let s = &mut frame!(self).stack;
                let i = s.len() - 1 - n as usize;
                let v = s.remove(i);
                s.push(v);
            }
            Op::Bury(n) => {
                let s = &mut frame!(self).stack;
                let v = s.pop().expect("pila");
                let i = s.len() - n as usize;
                s.insert(i, v);
            }
            Op::Undefined => self.push(Value::Undefined),
            Op::Null => self.push(Value::Null),
            Op::True => self.push(Value::Boolean(true)),
            Op::False => self.push(Value::Boolean(false)),
            Op::Int(i) => self.push(Value::Number(f64::from(i))),
            Op::Const(i) => {
                let v = match &frame!(self).code.consts[i as usize] {
                    Constant::Number(n) => Value::Number(*n),
                    Constant::String(s) => Value::String(s.clone()),
                    Constant::Name(a, _) => Value::String(a.to_js_string()),
                    other => unreachable!("constante {other:?}"),
                };
                self.push(v);
            }
            Op::BigInt(i) => {
                let raw = match &frame!(self).code.consts[i as usize] {
                    Constant::BigInt(r) => r.clone(),
                    _ => unreachable!("bigint"),
                };
                let b = crate::bigint::BigInt::from_literal(&raw).unwrap_or_default();
                let g = self.heap.alloc(b);
                self.push(Value::BigInt(g));
            }
            // E2: RegExpCreate para el literal (§13.2.7.3).
            Op::RegExp(i) => {
                let (pattern, flags) = match &frame!(self).code.consts[i as usize] {
                    Constant::RegExp { pattern, flags } => (pattern.clone(), flags.clone()),
                    _ => unreachable!("regexp"),
                };
                let v = crate::builtins::regexp::regexp_create_literal(self, &pattern, &flags)?;
                self.push(v);
            }

            // --- Bindings ---
            Op::GetLocal(s) => {
                let v = self.local(s);
                self.push(v);
            }
            Op::GetLocalChecked(s, n) => match frame!(self).locals[s as usize].clone() {
                Some(v) => self.push(v),
                None => return self.tdz_error(n),
            },
            Op::SetLocal(s) => {
                let v = self.peek(0).clone();
                frame!(self).locals[s as usize] = Some(v);
            }
            Op::SetLocalChecked(s, n) => {
                if frame!(self).locals[s as usize].is_none() {
                    return self.tdz_error(n);
                }
                let v = self.peek(0).clone();
                frame!(self).locals[s as usize] = Some(v);
            }
            Op::InitLocal(s) => {
                let v = self.pop();
                frame!(self).locals[s as usize] = Some(v);
            }
            Op::ClearLocal(s) => frame!(self).locals[s as usize] = None,
            Op::ConstAssignLocal(s, n) => {
                if frame!(self).locals[s as usize].is_none() {
                    return self.tdz_error(n);
                }
                let (name, _) = self.name_const(n);
                return self.type_error(&format!("Assignment to constant variable '{name}'"));
            }
            Op::GetEnv(h, s) => {
                let e = self.env_at(h);
                let v = self.heap[e].decl().expect("declarativo").slots[s as usize]
                    .clone()
                    .unwrap_or_default();
                self.push(v);
            }
            Op::GetEnvChecked(h, s, n) => {
                let e = self.env_at(h);
                match self.heap[e].decl().expect("declarativo").slots[s as usize].clone() {
                    Some(v) => self.push(v),
                    // Un binding de import indirecto (E1, §9.1.1.5.1).
                    None => match self.module_link_value(e, s) {
                        Some(Some(v)) => self.push(v),
                        _ => return self.tdz_error(n),
                    },
                }
            }
            Op::SetEnv(h, s) => {
                let e = self.env_at(h);
                let v = self.peek(0).clone();
                self.heap[e].decl_mut().expect("declarativo").slots[s as usize] = Some(v);
            }
            Op::SetEnvChecked(h, s, n) => {
                let e = self.env_at(h);
                if self.heap[e].decl().expect("declarativo").slots[s as usize].is_none() {
                    return self.tdz_error(n);
                }
                let v = self.peek(0).clone();
                self.heap[e].decl_mut().expect("declarativo").slots[s as usize] = Some(v);
            }
            Op::InitEnv(h, s) => {
                let e = self.env_at(h);
                let v = self.pop();
                self.heap[e].decl_mut().expect("declarativo").slots[s as usize] = Some(v);
            }
            Op::ConstAssignEnv(h, s, n) => {
                let e = self.env_at(h);
                if self.heap[e].decl().expect("declarativo").slots[s as usize].is_none()
                    && !matches!(self.module_link_value(e, s), Some(Some(_)))
                {
                    return self.tdz_error(n);
                }
                let (name, _) = self.name_const(n);
                return self.type_error(&format!("Assignment to constant variable '{name}'"));
            }
            Op::GetGlobal(n) | Op::GetGlobalForCall(n)
                if let Some(r) = {
                    let code = frame!(self).code.clone();
                    self.ic_get_global(&code, n)
                } =>
            {
                // Inline cache (E3) de una propiedad propia del objeto global.
                let v = r?;
                self.push(v);
                if matches!(op, Op::GetGlobalForCall(_)) {
                    self.push(Value::Undefined);
                }
            }
            Op::GetName(n) | Op::GetGlobal(n) => {
                let start = match op {
                    Op::GetGlobal(_) => self.global_env(),
                    _ => frame!(self).env,
                };
                let (name, key) = self.name_const(n);
                match self.resolve_binding_from(start, &name, &key)? {
                    Some(env) => {
                        let strict = self.strict_code();
                        let v = self.env_get_binding_value(env, &name, &key, strict)?;
                        if matches!(op, Op::GetGlobal(_)) {
                            let code = frame!(self).code.clone();
                            self.ic_fill_global(&code, n, &name, &key);
                        }
                        self.push(v);
                    }
                    None => return self.reference_error(&format!("{name} is not defined")),
                }
            }
            Op::TypeofName(n) | Op::TypeofGlobal(n) => {
                let start = match op {
                    Op::TypeofGlobal(_) => self.global_env(),
                    _ => frame!(self).env,
                };
                let (name, key) = self.name_const(n);
                let v = match self.resolve_binding_from(start, &name, &key)? {
                    Some(env) => {
                        let strict = self.strict_code();
                        self.env_get_binding_value(env, &name, &key, strict)?
                    }
                    None => Value::Undefined,
                };
                let t = self.type_of(&v);
                self.push(Value::str(t));
            }
            Op::ResolveRef(n) | Op::ResolveGlobalRef(n) => {
                let start = match op {
                    Op::ResolveGlobalRef(_) => self.global_env(),
                    _ => frame!(self).env,
                };
                let (name, key) = self.name_const(n);
                let r = self.resolve_binding_from(start, &name, &key)?;
                frame!(self).refs.push(r);
            }
            Op::GetRefValue(n) => {
                let r = *frame!(self).refs.last().expect("referencia");
                let (name, key) = self.name_const(n);
                match r {
                    Some(env) => {
                        let strict = self.strict_code();
                        let v = self.env_get_binding_value(env, &name, &key, strict)?;
                        self.push(v);
                    }
                    None => return self.reference_error(&format!("{name} is not defined")),
                }
            }
            Op::PutRef(n, strict) => {
                let r = frame!(self).refs.pop().expect("referencia");
                let (name, key) = self.name_const(n);
                let v = self.peek(0).clone();
                match r {
                    Some(env) => self.env_set_mutable_binding(env, &name, &key, v, strict)?,
                    None => {
                        if strict {
                            return self.reference_error(&format!("{name} is not defined"));
                        }
                        let g = self.realm().global_object;
                        self.set_or_throw(g, key, v, false)?;
                    }
                }
            }
            Op::InitRef(n) => {
                let r = frame!(self).refs.pop().expect("referencia");
                let (name, key) = self.name_const(n);
                let v = self.pop();
                match r {
                    Some(env) => self.env_initialize_binding(env, &name, &key, v)?,
                    None => {
                        let g = self.realm().global_object;
                        self.set_or_throw(g, key, v, false)?;
                    }
                }
            }
            Op::PopRef => {
                frame!(self).refs.pop();
            }
            Op::DeleteName(n) => {
                let (name, key) = self.name_const(n);
                let start = frame!(self).env;
                let r = match self.resolve_binding_from(start, &name, &key)? {
                    Some(env) => self.env_delete_binding(env, &name, &key)?,
                    None => true,
                };
                self.push(Value::Boolean(r));
            }
            Op::GetNameForCall(n) | Op::GetGlobalForCall(n) => {
                let start = match op {
                    Op::GetGlobalForCall(_) => self.global_env(),
                    _ => frame!(self).env,
                };
                let (name, key) = self.name_const(n);
                match self.resolve_binding_from(start, &name, &key)? {
                    Some(env) => {
                        let strict = self.strict_code();
                        let v = self.env_get_binding_value(env, &name, &key, strict)?;
                        let this = self.env_with_base_object(env);
                        if matches!(op, Op::GetGlobalForCall(_)) {
                            let code = frame!(self).code.clone();
                            self.ic_fill_global(&code, n, &name, &key);
                        }
                        self.push(v);
                        self.push(this);
                    }
                    None => return self.reference_error(&format!("{name} is not defined")),
                }
            }
            Op::GetCallee => {
                let c = frame!(self).callee;
                self.push(c.into());
            }
            Op::PushScope(c) => {
                let layout = match &frame!(self).code.consts[c as usize] {
                    Constant::Layout(l) => l.clone(),
                    _ => unreachable!("layout"),
                };
                let outer = frame!(self).env;
                let e = self.heap.alloc(Env::declarative(Some(outer), layout));
                frame!(self).env = e;
            }
            Op::PushFunctionScope(c) => {
                let layout = match &frame!(self).code.consts[c as usize] {
                    Constant::Layout(l) => l.clone(),
                    _ => unreachable!("layout"),
                };
                let f = frame!(self);
                let outer = f.env;
                let data = FunctionEnvData {
                    this: f.this.clone(),
                    function: f.callee.expect("función"),
                    new_target: f.new_target,
                };
                let mut env = Env::declarative(Some(outer), layout);
                if let Some(d) = env.decl_mut() {
                    d.function = Some(Box::new(data));
                }
                let e = self.heap.alloc(env);
                let f = frame!(self);
                f.env = e;
                f.function_env = Some(e);
            }
            Op::MarkVarEnv => {
                let f = frame!(self);
                f.var_env = Some(f.env);
            }
            Op::PopScope => {
                let e = frame!(self).env;
                frame!(self).env = self.heap[e].outer.expect("environment externo");
            }
            Op::CopyScope => {
                let e = frame!(self).env;
                let copy = self.heap[e].clone();
                let n = self.heap.alloc(copy);
                frame!(self).env = n;
            }
            Op::PushWith => {
                let v = self.pop();
                let o = self.to_object(&v)?;
                let outer = frame!(self).env;
                let e = self.heap.alloc(Env {
                    outer: Some(outer),
                    kind: EnvKind::Object {
                        object: o,
                        with: true,
                    },
                });
                frame!(self).env = e;
            }

            // --- Contexto ---
            Op::Ctx(what, loc) => {
                let v = self.get_ctx(what, loc)?;
                self.push(v);
            }
            Op::SuperCallFinish(loc) => {
                let result = self.peek(0).clone();
                match self.ctx_env(loc) {
                    None => {
                        if frame!(self).this.is_some() {
                            return self
                                .reference_error("Super constructor may only be called once");
                        }
                        frame!(self).this = Some(result.clone());
                    }
                    Some(env) => {
                        let d = self.heap[env]
                            .decl_mut()
                            .and_then(|d| d.function.as_mut())
                            .expect("Function Environment");
                        if d.this.is_some() {
                            return self
                                .reference_error("Super constructor may only be called once");
                        }
                        d.this = Some(result.clone());
                    }
                }
                let func = self.get_ctx(CtxWhat::Function, loc)?;
                if let (Value::Object(r), Value::Object(f)) = (&result, &func) {
                    self.initialize_instance_elements(*r, *f)?;
                }
            }
            Op::SuperConstructor => {
                let f = self.pop();
                let p = match f {
                    Value::Object(o) => self.get_prototype_of(o)?,
                    _ => None,
                };
                self.push(p.map_or(Value::Null, Value::Object));
            }
            Op::SuperBase => {
                let home = self.pop();
                let p = match home {
                    Value::Object(o) => {
                        self.get_prototype_of(o)?.map_or(Value::Null, Value::Object)
                    }
                    _ => Value::Undefined,
                };
                self.push(p);
            }
            Op::GetSuperElem => {
                let base = self.pop();
                let key = self.pop();
                let this = self.pop();
                let key = self.to_property_key(&key)?;
                let o = self.to_object(&base)?;
                let v = self.get(o, &key, this)?;
                self.push(v);
            }
            Op::SetSuperElem(strict) => {
                let v = self.pop();
                let base = self.pop();
                let key = self.pop();
                let this = self.pop();
                let key = self.to_property_key(&key)?;
                let o = self.to_object(&base)?;
                let ok = self.set(o, key.clone(), v.clone(), this)?;
                if !ok && strict {
                    return self
                        .type_error(&format!("Cannot assign to read only property '{key}'"));
                }
                self.push(v);
            }
            Op::HomeObject => {
                let f = self.pop();
                let h = match f {
                    Value::Object(o) => match &self.heap[o].kind {
                        ObjectKind::Function(c) => c.home_object.into(),
                        _ => Value::Undefined,
                    },
                    _ => Value::Undefined,
                };
                self.push(h);
            }

            // --- Operadores ---
            Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Div
            | Op::Mod
            | Op::Exp
            | Op::Shl
            | Op::Shr
            | Op::UShr
            | Op::BitAnd
            | Op::BitOr
            | Op::BitXor => {
                // Ruta rápida: Number × Number (el resultado de §6.1.6.1 sin conversiones).
                if let Some((a, b)) = self.top_numbers() {
                    let v = match op {
                        Op::Add => a + b,
                        Op::Sub => a - b,
                        Op::Mul => a * b,
                        Op::Div => a / b,
                        Op::Mod => crate::abstract_ops::number::remainder(a, b),
                        Op::Exp => crate::abstract_ops::number::exponentiate(a, b),
                        Op::Shl => f64::from(crate::abstract_ops::conversion::to_int32(a).wrapping_shl(crate::abstract_ops::conversion::to_uint32(b) & 31)),
                        Op::Shr => f64::from(crate::abstract_ops::conversion::to_int32(a) >> (crate::abstract_ops::conversion::to_uint32(b) & 31)),
                        Op::UShr => f64::from(crate::abstract_ops::conversion::to_uint32(a) >> (crate::abstract_ops::conversion::to_uint32(b) & 31)),
                        Op::BitAnd => f64::from(crate::abstract_ops::conversion::to_int32(a) & crate::abstract_ops::conversion::to_int32(b)),
                        Op::BitOr => f64::from(crate::abstract_ops::conversion::to_int32(a) | crate::abstract_ops::conversion::to_int32(b)),
                        _ => f64::from(crate::abstract_ops::conversion::to_int32(a) ^ crate::abstract_ops::conversion::to_int32(b)),
                    };
                    self.replace_top2(Value::Number(v));
                    return Ok(Flow::Next);
                }
                let r = self.pop();
                let l = self.pop();
                let v = self.apply_binary(op, l, r)?;
                self.push(v);
            }
            Op::Lt | Op::Gt | Op::Le | Op::Ge => {
                if let Some((a, b)) = self.top_numbers() {
                    let v = match op {
                        Op::Lt => a < b,
                        Op::Gt => a > b,
                        Op::Le => a <= b,
                        _ => a >= b,
                    };
                    self.replace_top2(Value::Boolean(v));
                    return Ok(Flow::Next);
                }
                let r = self.pop();
                let l = self.pop();
                let v = match op {
                    Op::Lt => self.is_less_than(&l, &r, true)? == Some(true),
                    Op::Gt => self.is_less_than(&r, &l, false)? == Some(true),
                    Op::Le => self.is_less_than(&r, &l, false)? == Some(false),
                    _ => self.is_less_than(&l, &r, true)? == Some(false),
                };
                self.push(Value::Boolean(v));
            }
            Op::Eq | Op::Ne => {
                let r = self.pop();
                let l = self.pop();
                let eq = self.is_loosely_equal(&l, &r)?;
                self.push(Value::Boolean(if op == Op::Eq { eq } else { !eq }));
            }
            Op::StrictEq | Op::StrictNe => {
                if let Some((a, b)) = self.top_numbers() {
                    let eq = a == b;
                    self.replace_top2(Value::Boolean(if op == Op::StrictEq { eq } else { !eq }));
                    return Ok(Flow::Next);
                }
                let r = self.pop();
                let l = self.pop();
                let eq = self.is_strictly_equal(&l, &r);
                self.push(Value::Boolean(if op == Op::StrictEq { eq } else { !eq }));
            }
            Op::In => {
                let r = self.pop();
                let l = self.pop();
                let Value::Object(o) = r else {
                    return self.type_error(
                        "Cannot use 'in' operator to search for a key in a non-object",
                    );
                };
                let k = self.to_property_key(&l)?;
                let v = self.has_property(o, &k)?;
                self.push(Value::Boolean(v));
            }
            Op::InstanceOf => {
                let r = self.pop();
                let l = self.pop();
                let v = self.instanceof_operator(&l, &r)?;
                self.push(Value::Boolean(v));
            }
            Op::PrivateIn => {
                let o = self.pop();
                let name = self.pop();
                let Value::Object(o) = o else {
                    return self.type_error(
                        "Cannot use 'in' operator to search for a private field in a non-object",
                    );
                };
                let Value::Symbol(n) = name else {
                    unreachable!("nombre privado")
                };
                let has = self.heap[o].private_find(n).is_some();
                self.push(Value::Boolean(has));
            }
            Op::Neg => {
                let v = self.pop();
                match self.to_numeric(&v)? {
                    Value::Number(n) => self.push(Value::Number(-n)),
                    // E2
                    Value::BigInt(b) => {
                        let r = crate::builtins::bigint::unary_minus(self, b);
                        self.push(r);
                    }
                    _ => unreachable!("ToNumeric"),
                }
            }
            Op::Plus => {
                let v = self.pop();
                let n = self.to_number(&v)?;
                self.push(Value::Number(n));
            }
            Op::Not => {
                let v = self.pop();
                let b = crate::abstract_ops::conversion::to_boolean(self, &v);
                self.push(Value::Boolean(!b));
            }
            Op::BitNot => {
                let v = self.pop();
                match self.to_numeric(&v)? {
                    Value::Number(n) => {
                        let i = crate::abstract_ops::conversion::to_int32(n);
                        self.push(Value::Number(f64::from(!i)));
                    }
                    // E2
                    Value::BigInt(b) => {
                        let r = crate::builtins::bigint::bitwise_not(self, b);
                        self.push(r);
                    }
                    _ => unreachable!("ToNumeric"),
                }
            }
            Op::Typeof => {
                let v = self.pop();
                let t = self.type_of(&v);
                self.push(Value::str(t));
            }
            Op::ToNumeric => {
                let v = self.pop();
                let n = self.to_numeric(&v)?;
                self.push(n);
            }
            Op::Inc | Op::Dec => {
                let v = self.pop();
                match v {
                    Value::Number(n) => {
                        self.push(Value::Number(if op == Op::Inc { n + 1.0 } else { n - 1.0 }))
                    }
                    // E2
                    Value::BigInt(b) => {
                        let r = crate::builtins::bigint::increment(
                            self,
                            b,
                            if op == Op::Inc { 1 } else { -1 },
                        );
                        self.push(r);
                    }
                    _ => return self.internal_error("++/-- sobre un valor no numérico"),
                }
            }
            Op::ToPropertyKey => {
                let v = self.pop();
                let k = self.to_property_key(&v)?;
                self.push(k.to_value());
            }
            Op::ToPropertyKeyAt(d) => {
                let v = self.peek(d as usize).clone();
                let k = self.to_property_key(&v)?.to_value();
                let st = &mut frame!(self).stack;
                let i = st.len() - 1 - d as usize;
                st[i] = k;
            }
            Op::ToStringOp => {
                let v = self.pop();
                let s = self.to_string(&v)?;
                self.push(Value::String(s));
            }
            Op::CheckCoercible(d) => {
                let v = self.peek(d as usize).clone();
                if v.is_nullish() {
                    return self.type_error(&format!(
                        "Cannot set properties of {}",
                        if v.is_null() { "null" } else { "undefined" }
                    ));
                }
            }
            Op::PrepareElem => {
                let key = self.pop();
                let obj = self.peek(0).clone();
                if obj.is_nullish() {
                    return self.nullish_access(&obj, &key);
                }
                let k = self.to_property_key(&key)?;
                self.push(k.to_value());
            }

            // --- Propiedades ---
            Op::GetProp(n) | Op::GetPropKeep(n) => {
                let obj = if matches!(op, Op::GetProp(_)) {
                    self.pop()
                } else {
                    self.peek(0).clone()
                };
                let code = frame!(self).code.clone();
                let key = name_key(&code, n);
                // Inline cache (E3): la disposición ya vista en este sitio. Para un primitivo
                // (salvo `length` de un string) se busca en su prototipo.
                let target = match &obj {
                    Value::Object(o) => Some(*o),
                    Value::String(_) if *key != self.keys.length => {
                        Some(self.intrinsics().string_prototype)
                    }
                    Value::Number(_) => Some(self.intrinsics().number_prototype),
                    _ => None,
                };
                let v = match target {
                    Some(o) => match self.ic_get(&code, n, o, key, &obj) {
                        Some(r) => r?,
                        None => {
                            let v = self.get_value_of(&obj, key)?;
                            self.ic_fill_get(&code, n, o, key);
                            v
                        }
                    },
                    None => self.get_value_of(&obj, key)?,
                };
                self.push(v);
            }
            Op::GetElem | Op::GetElemKeep => {
                let key = self.pop();
                let obj = if op == Op::GetElem {
                    self.pop()
                } else {
                    self.peek(0).clone()
                };
                if obj.is_nullish() {
                    return self.nullish_access(&obj, &key);
                }
                let k = self.to_property_key(&key)?;
                let v = self.get_value_of(&obj, &k)?;
                self.push(v);
            }
            Op::SetProp(n, strict) => {
                let v = self.pop();
                let obj = self.pop();
                let code = frame!(self).code.clone();
                let key = name_key(&code, n);
                match obj {
                    // Inline cache (E3): escribir o agregar con la disposición ya vista.
                    Value::Object(o) => {
                        if !self.ic_set(&code, n, o, key, &v) {
                            let before = self.ic_before_set(o);
                            self.put_value_of(&obj, key.clone(), v.clone(), strict)?;
                            self.ic_fill_set(&code, n, o, key, before);
                        }
                    }
                    _ => self.put_value_of(&obj, key.clone(), v.clone(), strict)?,
                }
                self.push(v);
            }
            Op::SetElem(strict) => {
                let v = self.pop();
                let key = self.pop();
                let obj = self.pop();
                if obj.is_nullish() {
                    return self.nullish_access(&obj, &key);
                }
                let k = self.to_property_key(&key)?;
                self.put_value_of(&obj, k, v.clone(), strict)?;
                self.push(v);
            }
            Op::DeleteProp(n, strict) => {
                let obj = self.pop();
                let (_, key) = self.name_const(n);
                let o = self.to_object(&obj)?;
                let ok = self.delete(o, &key)?;
                if !ok && strict {
                    return self.type_error(&format!("Cannot delete property '{key}'"));
                }
                self.push(Value::Boolean(ok));
            }
            Op::DeleteElem(strict) => {
                let key = self.pop();
                let obj = self.pop();
                let o = self.to_object(&obj)?;
                let k = self.to_property_key(&key)?;
                let ok = self.delete(o, &k)?;
                if !ok && strict {
                    return self.type_error(&format!("Cannot delete property '{k}'"));
                }
                self.push(Value::Boolean(ok));
            }
            Op::InitProp(n) => {
                let v = self.pop();
                let obj = self.peek(0).as_object().expect("objeto literal");
                let code = frame!(self).code.clone();
                // Inline cache (E3): la transición ya vista en este literal.
                if !self.ic_init(&code, n, obj, &v) {
                    let key = name_key(&code, n);
                    let before = self.heap[obj].props.shape().cloned();
                    self.create_data_property_or_throw(obj, key.clone(), v)?;
                    self.ic_fill_init(&code, n, obj, key, before);
                }
            }
            Op::InitElem => {
                let v = self.pop();
                let key = self.pop();
                let obj = self.peek(0).as_object().expect("objeto literal");
                let k = self.to_property_key(&key)?;
                self.create_data_property_or_throw(obj, k, v)?;
            }
            Op::InitProto => {
                let v = self.pop();
                let obj = self.peek(0).as_object().expect("objeto literal");
                match v {
                    Value::Object(p) => {
                        self.set_prototype_of(obj, Some(p))?;
                    }
                    Value::Null => {
                        self.set_prototype_of(obj, None)?;
                    }
                    _ => {}
                }
            }
            Op::InitAccessor {
                name,
                setter,
                computed,
            } => {
                let f = self.pop().as_object().expect("función");
                let key = if computed {
                    let k = self.pop();
                    self.to_property_key(&k)?
                } else {
                    self.name_const(name).1
                };
                let obj = self.peek(0).as_object().expect("objeto literal");
                let desc = if setter {
                    PropertyDescriptor {
                        set: Some(Some(f)),
                        enumerable: Some(true),
                        configurable: Some(true),
                        ..PropertyDescriptor::default()
                    }
                } else {
                    PropertyDescriptor {
                        get: Some(Some(f)),
                        enumerable: Some(true),
                        configurable: Some(true),
                        ..PropertyDescriptor::default()
                    }
                };
                self.define_property_or_throw(obj, key, desc)?;
            }
            Op::CopyDataProperties => {
                let src = self.pop();
                let obj = self.peek(0).as_object().expect("objeto literal");
                self.copy_data_properties(obj, &src, &[])?;
            }
            Op::CopyRest(n) => {
                let mut keys = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    let k = self.pop();
                    keys.push(self.to_property_key(&k)?);
                }
                let src = self.pop();
                let rest = self.new_plain_object();
                self.push(Value::Object(rest));
                self.copy_data_properties(rest, &src, &keys)?;
            }
            Op::PrivateGet => {
                let obj = self.pop();
                let name = self.pop();
                let Value::Symbol(n) = name else {
                    unreachable!("nombre privado")
                };
                let v = self.private_get(&obj, n)?;
                self.push(v);
            }
            Op::PrivateSet => {
                let v = self.pop();
                let obj = self.pop();
                let name = self.pop();
                let Value::Symbol(n) = name else {
                    unreachable!("nombre privado")
                };
                self.private_set(&obj, n, v.clone())?;
                self.push(v);
            }
            Op::NewObject => {
                let o = self.new_plain_object();
                self.push(Value::Object(o));
            }
            Op::NewArray => {
                let a = self.array_create(0, None)?;
                self.push(Value::Object(a));
            }
            Op::ArrayPush => {
                let v = self.pop();
                let a = self.peek(0).as_object().expect("array");
                if let ObjectKind::Array(d) = &mut self.heap[a].kind {
                    d.dense.push(Some(v));
                    d.length += 1;
                }
            }
            Op::ArrayHole => {
                let a = self.peek(0).as_object().expect("array");
                if let ObjectKind::Array(d) = &mut self.heap[a].kind {
                    d.dense.push(None);
                    d.length += 1;
                }
            }
            Op::ArraySpread => {
                let iterable = self.pop();
                let a = self.peek(0).as_object().expect("array");
                let values = self.iterable_to_list(&iterable)?;
                if let ObjectKind::Array(d) = &mut self.heap[a].kind {
                    d.length += values.len() as u32;
                    d.dense.extend(values.into_iter().map(Some));
                }
            }

            // --- Llamadas ---
            Op::Call(argc, t) => {
                let n = argc as usize;
                let len = frame!(self).stack.len();
                let base = len - n - 2;
                let callee = frame!(self).stack[base].clone();
                if !self.is_callable(&callee) {
                    return self.not_callable(t, &callee, "a function");
                }
                let closure = matches!(&callee, Value::Object(o)
                    if matches!(self.heap[*o].kind, ObjectKind::Function(_)));
                let r = if closure {
                    // Una función ECMAScript recibe los argumentos sin copiarlos (E3): salen de
                    // la pila y pasan a su frame (no hay recolección hasta que el frame entra en
                    // la pila de frames).
                    let s = &mut frame!(self).stack;
                    let args = s.split_off(base + 2);
                    let this = s.pop().expect("this");
                    s.pop();
                    // Una función simple corre en este mismo bucle (sin recursión de Rust).
                    if let Value::Object(fo) = callee
                        && let Some(frame) = self.inline_frame(fo, this.clone(), &args)?
                    {
                        let mut frame = frame;
                        frame.args = args;
                        self.frames.push(frame);
                        return Ok(Flow::Next);
                    }
                    self.call_internal_vec(&callee, this, args)?
                } else {
                    let (this, args) = {
                        let s = &frame!(self).stack;
                        (s[base + 1].clone(), s[base + 2..].to_vec())
                    };
                    let r = self.call_internal(&callee, this, &args)?;
                    frame!(self).stack.truncate(base);
                    r
                };
                self.push(r);
                self.safepoint()?;
            }
            Op::CallSpread(t) => {
                let arr = self.peek(0).clone();
                let this = self.peek(1).clone();
                let callee = self.peek(2).clone();
                if !self.is_callable(&callee) {
                    return self.not_callable(t, &callee, "a function");
                }
                let args = self.array_values(&arr);
                let r = self.call_internal(&callee, this, &args)?;
                let s = &mut frame!(self).stack;
                let n = s.len();
                s.truncate(n - 3);
                self.push(r);
                self.safepoint()?;
            }
            Op::CallEval(argc, flags) => {
                let n = argc as usize;
                let len = frame!(self).stack.len();
                let base = len - n - 2;
                let (callee, this, args) = {
                    let s = &frame!(self).stack;
                    (s[base].clone(), s[base + 1].clone(), s[base + 2..].to_vec())
                };
                let r = self.call_maybe_eval(&callee, this, &args, flags)?;
                frame!(self).stack.truncate(base);
                self.push(r);
            }
            Op::CallEvalSpread(flags) => {
                let arr = self.peek(0).clone();
                let this = self.peek(1).clone();
                let callee = self.peek(2).clone();
                let args = self.array_values(&arr);
                let r = self.call_maybe_eval(&callee, this, &args, flags)?;
                let s = &mut frame!(self).stack;
                let n = s.len();
                s.truncate(n - 3);
                self.push(r);
            }
            Op::New(argc, t) => {
                let n = argc as usize;
                let len = frame!(self).stack.len();
                let base = len - n - 1;
                let (callee, args) = {
                    let s = &frame!(self).stack;
                    (s[base].clone(), s[base + 1..].to_vec())
                };
                if !self.is_constructor(&callee) {
                    return self.not_callable(t, &callee, "a constructor");
                }
                let r = self.new_operator(&callee, &args)?;
                frame!(self).stack.truncate(base);
                self.push(r);
                self.safepoint()?;
            }
            Op::NewSpread(t) => {
                let arr = self.peek(0).clone();
                let callee = self.peek(1).clone();
                if !self.is_constructor(&callee) {
                    return self.not_callable(t, &callee, "a constructor");
                }
                let args = self.array_values(&arr);
                let r = self.new_operator(&callee, &args)?;
                let s = &mut frame!(self).stack;
                let n = s.len();
                s.truncate(n - 2);
                self.push(r);
            }
            Op::SuperConstruct(argc) => {
                let n = argc as usize;
                let len = frame!(self).stack.len();
                let base = len - n - 2;
                let (nt, ctor, args) = {
                    let s = &frame!(self).stack;
                    (s[base].clone(), s[base + 1].clone(), s[base + 2..].to_vec())
                };
                let r = self.super_construct(&nt, &ctor, &args)?;
                frame!(self).stack.truncate(base);
                self.push(r);
            }
            Op::SuperConstructSpread => {
                let arr = self.peek(0).clone();
                let ctor = self.peek(1).clone();
                let nt = self.peek(2).clone();
                let args = self.array_values(&arr);
                let r = self.super_construct(&nt, &ctor, &args)?;
                let s = &mut frame!(self).stack;
                let n = s.len();
                s.truncate(n - 3);
                self.push(r);
            }
            Op::SuperConstructForward => {
                let ctor = self.peek(0).clone();
                let nt = self.peek(1).clone();
                let args = frame!(self).args.clone();
                let r = self.super_construct(&nt, &ctor, &args)?;
                let s = &mut frame!(self).stack;
                let n = s.len();
                s.truncate(n - 2);
                self.push(r);
            }
            Op::FieldName => {
                let callee = frame!(self).callee;
                let v = match callee.map(|c| &self.heap[c].kind) {
                    Some(ObjectKind::Function(c)) => match &c.field_name {
                        Some(FieldName::Key(k)) => k.to_value(),
                        Some(FieldName::Private(s)) => Value::Symbol(*s),
                        None => Value::str(""),
                    },
                    _ => Value::str(""),
                };
                self.push(v);
            }
            Op::TemplateObject(site) => {
                let site = match &frame!(self).code.consts[site as usize] {
                    Constant::Template(t) => t.clone(),
                    _ => unreachable!("template"),
                };
                let t = self.get_template_object(&site)?;
                self.push(Value::Object(t));
            }
            Op::Closure { code, name, home } => {
                let code = match &frame!(self).code.consts[code as usize] {
                    Constant::Function(c) => c.clone(),
                    _ => unreachable!("función"),
                };
                let home_obj = if home > 0 {
                    self.peek(home as usize - 1).as_object()
                } else {
                    None
                };
                let name = match name {
                    FnName::Code => None,
                    FnName::Stack(prefix) => {
                        let k = self.peek(0).clone();
                        Some(self.function_name_from_key(&k, prefix)?)
                    }
                    FnName::Local(slot) => {
                        let k = self.local(slot);
                        Some(self.function_name_from_key(&k, NamePrefix::None)?)
                    }
                };
                let env = frame!(self).env;
                let f = self.make_closure(code, env, home_obj, name);
                self.push(Value::Object(f));
            }

            // --- Control ---
            Op::Jump(t) => {
                let f = frame!(self);
                let back = (t as usize) < f.pc;
                f.pc = t as usize;
                if back {
                    self.safepoint()?;
                }
            }
            Op::JumpIfTrue(t) | Op::JumpIfFalse(t) => {
                let v = self.pop();
                let b = crate::abstract_ops::conversion::to_boolean(self, &v);
                if b == matches!(op, Op::JumpIfTrue(_)) {
                    let f = frame!(self);
                    let back = (t as usize) < f.pc;
                    f.pc = t as usize;
                    if back {
                        self.safepoint()?;
                    }
                }
            }
            Op::JumpIfTrueKeep(t) | Op::JumpIfFalseKeep(t) => {
                let v = self.peek(0).clone();
                let b = crate::abstract_ops::conversion::to_boolean(self, &v);
                if b == matches!(op, Op::JumpIfTrueKeep(_)) {
                    frame!(self).pc = t as usize;
                } else {
                    self.pop();
                }
            }
            Op::JumpIfNotNullishKeep(t) => {
                if self.peek(0).is_nullish() {
                    self.pop();
                } else {
                    frame!(self).pc = t as usize;
                }
            }
            Op::JumpIfNotUndefined(t) => {
                if !self.peek(0).is_undefined() {
                    frame!(self).pc = t as usize;
                }
            }
            Op::JumpIfNullishChain { target, extra } => {
                if self.peek(extra as usize).is_nullish() {
                    let s = &mut frame!(self).stack;
                    let n = s.len();
                    s.truncate(n - 1 - extra as usize);
                    s.push(Value::Undefined);
                    frame!(self).pc = target as usize;
                }
            }
            Op::Throw => {
                let v = self.pop();
                return Err(Throw::Value(v));
            }
            Op::ThrowError(kind, msg) => {
                let m = self.string_const(msg).to_string_lossy();
                let t = match kind {
                    ErrorKind::TypeError => ErrorType::TypeError,
                    ErrorKind::ReferenceError => ErrorType::ReferenceError,
                    ErrorKind::SyntaxError => ErrorType::SyntaxError,
                    ErrorKind::RangeError => ErrorType::RangeError,
                };
                return Err(self.error(t, &m));
            }
            Op::Unsupported(msg) => {
                let m = self.string_const(msg).to_string_lossy();
                return self.internal_error(&m);
            }
            Op::PushCatch(t) => {
                let f = frame!(self);
                let h = Handler {
                    kind: HandlerKind::Catch,
                    pc: t,
                    stack_len: f.stack.len() as u32,
                    refs_len: f.refs.len() as u32,
                    env: f.env,
                };
                f.handlers.push(h);
            }
            Op::PushFinally {
                target,
                kind_slot,
                value_slot,
            } => {
                let f = frame!(self);
                let h = Handler {
                    kind: HandlerKind::Finally {
                        kind_slot,
                        value_slot,
                    },
                    pc: target,
                    stack_len: f.stack.len() as u32,
                    refs_len: f.refs.len() as u32,
                    env: f.env,
                };
                f.handlers.push(h);
            }
            Op::PopHandler => {
                frame!(self).handlers.pop();
            }
            Op::EndFinally {
                kind_slot,
                value_slot,
                table,
            } => {
                let kind = match self.local(kind_slot) {
                    Value::Number(n) => n as u32,
                    _ => COMPLETION_NORMAL,
                };
                let value = self.local(value_slot);
                match kind {
                    COMPLETION_NORMAL => {}
                    COMPLETION_THROW => return Err(Throw::Value(value)),
                    COMPLETION_RETURN => return Ok(Flow::Return(value)),
                    k => {
                        let t = match &frame!(self).code.consts[table as usize] {
                            Constant::FinallyTable(t) => {
                                t.iter().find(|(kk, _)| *kk == k).map(|(_, pc)| *pc)
                            }
                            _ => None,
                        };
                        frame!(self).pc = t.expect("continuación de finally") as usize;
                    }
                }
            }
            Op::Return => {
                let v = self.pop();
                return Ok(Flow::Return(v));
            }
            Op::Debugger => {}

            // --- Iteración ---
            Op::GetIterator(slot) => {
                let obj = self.pop();
                let (it, next) = self.get_iterator(&obj)?;
                let f = frame!(self);
                f.locals[slot as usize] = Some(it);
                f.locals[slot as usize + 1] = Some(next);
                f.locals[slot as usize + 2] = Some(Value::Boolean(false));
            }
            Op::IterStep { slot, done } => match self.iter_step_slot(slot)? {
                Some(v) => self.push(v),
                None => frame!(self).pc = done as usize,
            },
            Op::IterValue(slot) => {
                let v = if self.local(slot + 2).is_undefined()
                    || matches!(self.local(slot + 2), Value::Boolean(true))
                {
                    Value::Undefined
                } else {
                    self.iter_step_slot(slot)?.unwrap_or_default()
                };
                self.push(v);
            }
            Op::IterRest(slot) => {
                let mut values = Vec::new();
                while !matches!(self.local(slot + 2), Value::Boolean(true)) {
                    match self.iter_step_slot(slot)? {
                        Some(v) => values.push(v),
                        None => break,
                    }
                }
                let a = self.create_array_from_list(&values)?;
                self.push(Value::Object(a));
            }
            Op::IterClose(slot) => {
                if !matches!(self.local(slot + 2), Value::Boolean(true)) {
                    let it = self.local(slot);
                    self.iterator_close(&it)?;
                }
            }
            Op::IterCleanup {
                slot,
                kind_slot,
                value_slot,
            } => {
                let kind = match self.local(kind_slot) {
                    Value::Number(n) => n as u32,
                    _ => COMPLETION_NORMAL,
                };
                let value = self.local(value_slot);
                let done = matches!(self.local(slot + 2), Value::Boolean(true));
                let it = self.local(slot);
                if kind == COMPLETION_THROW {
                    if !done {
                        self.iterator_close_on_throw(&it);
                    }
                    return Err(Throw::Value(value));
                }
                if !done {
                    self.iterator_close(&it)?;
                }
                return Ok(Flow::Return(value));
            }
            Op::ForInPrepare(slot) => {
                let v = self.pop();
                let it = if v.is_nullish() {
                    Value::Undefined
                } else {
                    let o = self.to_object(&v)?;
                    Value::Object(self.create_for_in_iterator(o))
                };
                frame!(self).locals[slot as usize] = Some(it);
            }
            Op::ForInNext { slot, done } => {
                let it = self.local(slot);
                let next = match it {
                    Value::Object(o) => self.for_in_next(o)?,
                    _ => None,
                };
                match next {
                    Some(k) => self.push(k),
                    None => frame!(self).pc = done as usize,
                }
            }

            // --- Generadores ---
            Op::GeneratorStart => {
                let callee = frame!(self).callee.expect("función generadora");
                let g = crate::vm::generator::create_generator(self, callee)?;
                return Ok(Flow::GenStart(g));
            }
            Op::Yield => {
                let v = self.pop();
                return Ok(Flow::Yield(v, false));
            }
            Op::YieldRaw { .. } => {
                let v = self.pop();
                return Ok(Flow::Yield(v, true));
            }
            Op::YieldStarStep {
                slot,
                kind_slot,
                value_slot,
                done,
            } => return self.yield_star_step(slot, kind_slot, value_slot, done),
            // E3
            Op::Await => {
                let v = self.pop();
                return Ok(Flow::Await(v));
            }
            Op::GetAsyncIterator(slot) => {
                let obj = self.pop();
                let (it, next) = self.get_async_iterator(&obj)?;
                let f = frame!(self);
                f.locals[slot as usize] = Some(it);
                f.locals[slot as usize + 1] = Some(next);
                f.locals[slot as usize + 2] = Some(Value::Boolean(false));
            }
            Op::AsyncIterNext(slot) => {
                let it = self.local(slot);
                let next = self.local(slot + 1);
                let r = self.call(&next, it, &[]);
                if r.is_err() {
                    frame!(self).locals[slot as usize + 2] = Some(Value::Boolean(true));
                }
                self.push(r?);
            }
            Op::AsyncIterResult { slot, done } => return self.async_iter_result(slot, done),
            Op::AsyncIterClose {
                slot,
                kind_slot,
                skip,
            } => return self.async_iter_close(slot, kind_slot, skip),
            Op::AsyncIterCloseCheck { kind_slot } => {
                let r = self.pop();
                if !self.completion_is_throw(kind_slot) && !r.is_object() {
                    return self.type_error("iterator.return() did not return an object");
                }
            }
            Op::AsyncIterCloseCatch { kind_slot } => {
                let e = self.pop();
                if !self.completion_is_throw(kind_slot) {
                    return Err(Throw::Value(e));
                }
            }
            Op::AsyncYieldStarCall {
                slot,
                kind_slot,
                value_slot,
            } => return self.async_yield_star_call(slot, kind_slot, value_slot),
            Op::AsyncYieldStarResult { slot, done } => {
                return self.async_yield_star_result(slot, done);
            }

            // --- Funciones ---
            Op::CreateArguments(table) => {
                let o = match table {
                    None => self.create_unmapped_arguments(),
                    Some(t) => {
                        let map = match &frame!(self).code.consts[t as usize] {
                            Constant::ParamMap(m) => m.clone(),
                            _ => unreachable!("tabla de parámetros"),
                        };
                        self.create_mapped_arguments(&map)
                    }
                };
                self.push(Value::Object(o));
            }
            Op::GetArg(i) => {
                let v = frame!(self)
                    .args
                    .get(i as usize)
                    .cloned()
                    .unwrap_or_default();
                self.push(v);
            }
            Op::RestArgs(i) => {
                let rest: Vec<Value> = frame!(self).args.iter().skip(i as usize).cloned().collect();
                let a = self.create_array_from_list(&rest)?;
                self.push(Value::Object(a));
            }

            // --- Clases ---
            Op::ClassHeritage(has) => {
                let (proto_parent, ctor_parent) = if has {
                    let sup = self.pop();
                    match sup {
                        Value::Null => (
                            Value::Null,
                            Value::Object(self.intrinsics().function_prototype),
                        ),
                        Value::Object(o) if self.heap[o].is_constructor() => {
                            let k = self.keys.prototype.clone();
                            let pp = self.get(o, &k, Value::Object(o))?;
                            if !matches!(pp, Value::Object(_) | Value::Null) {
                                return self.type_error(
                                    "Class extends value does not have valid prototype property",
                                );
                            }
                            (pp, Value::Object(o))
                        }
                        _ => {
                            return self
                                .type_error("Class extends value is not a constructor or null");
                        }
                    }
                } else {
                    let i = self.intrinsics();
                    (
                        Value::Object(i.object_prototype),
                        Value::Object(i.function_prototype),
                    )
                };
                self.push(proto_parent);
                self.push(ctor_parent);
            }
            Op::ClassCreate { code, name } => {
                let ctor_parent = self.pop().as_object();
                let proto_parent = self.pop().as_object();
                let code = match &frame!(self).code.consts[code as usize] {
                    Constant::Function(c) => c.clone(),
                    _ => unreachable!("función"),
                };
                let name = match name {
                    FnName::Code => Value::String(code.name.clone()),
                    FnName::Local(slot) => {
                        let k = self.local(slot);
                        self.function_name_from_key(&k, NamePrefix::None)?
                    }
                    FnName::Stack(p) => {
                        let k = self.peek(0).clone();
                        self.function_name_from_key(&k, p)?
                    }
                };
                let env = frame!(self).env;
                let (f, proto) = self.class_create(code, env, proto_parent, ctor_parent, name);
                self.push(Value::Object(f));
                self.push(Value::Object(proto));
            }
            Op::DefineMethod {
                kind,
                is_static,
                name,
            } => {
                let f = self.pop().as_object().expect("método");
                let key = match name {
                    Some(n) => self.name_const(n).1,
                    None => {
                        let k = self.pop();
                        self.to_property_key(&k)?
                    }
                };
                let target = if is_static {
                    self.peek(1)
                } else {
                    self.peek(0)
                }
                .as_object()
                .expect("objetivo");
                let desc = match kind {
                    MethodKind::Method => {
                        PropertyDescriptor::data(Value::Object(f), true, false, true)
                    }
                    MethodKind::Getter => PropertyDescriptor {
                        get: Some(Some(f)),
                        enumerable: Some(false),
                        configurable: Some(true),
                        ..PropertyDescriptor::default()
                    },
                    MethodKind::Setter => PropertyDescriptor {
                        set: Some(Some(f)),
                        enumerable: Some(false),
                        configurable: Some(true),
                        ..PropertyDescriptor::default()
                    },
                };
                self.define_property_or_throw(target, key, desc)?;
            }
            Op::AddField {
                private, has_init, ..
            } => {
                let init = if has_init {
                    self.pop().as_object()
                } else {
                    None
                };
                let key = self.pop();
                let ctor = self.peek(1).as_object().expect("constructor");
                let name = if private {
                    let Value::Symbol(s) = key else {
                        unreachable!("privado")
                    };
                    FieldName::Private(s)
                } else {
                    FieldName::Key(self.to_property_key(&key)?)
                };
                if let Some(i) = init
                    && let ObjectKind::Function(c) = &mut self.heap[i].kind
                {
                    c.field_name = Some(name.clone());
                }
                if let ObjectKind::Function(c) = &mut self.heap[ctor].kind {
                    c.class
                        .get_or_insert_with(|| Box::new(ClassData::default()))
                        .fields
                        .push(ClassField {
                            name,
                            initializer: init,
                        });
                }
            }
            Op::AddPrivateMethod(kind) => {
                let f = self.pop().as_object().expect("método");
                let name = self.pop();
                let Value::Symbol(n) = name else {
                    unreachable!("privado")
                };
                let ctor = self.peek(1).as_object().expect("constructor");
                if let ObjectKind::Function(c) = &mut self.heap[ctor].kind {
                    let class = c
                        .class
                        .get_or_insert_with(|| Box::new(ClassData::default()));
                    add_private_element(&mut class.private_methods, n, kind, f);
                }
            }
            Op::AddPrivateMethodTo(kind) => {
                let f = self.pop().as_object().expect("método");
                let name = self.pop();
                let Value::Symbol(n) = name else {
                    unreachable!("privado")
                };
                let obj = self.peek(0).as_object().expect("objeto");
                let existing = self.heap[obj].private.iter().position(|(x, _)| *x == n);
                match (existing, kind) {
                    (Some(i), MethodKind::Getter | MethodKind::Setter)
                        if matches!(
                            self.heap[obj].private[i].1,
                            PrivateElement::Accessor { .. }
                        ) =>
                    {
                        if let PrivateElement::Accessor { get, set } =
                            &mut self.heap[obj].private[i].1
                        {
                            if kind == MethodKind::Getter {
                                *get = Some(f);
                            } else {
                                *set = Some(f);
                            }
                        }
                    }
                    _ => {
                        let e = match kind {
                            MethodKind::Method => PrivateElement::Method(f),
                            MethodKind::Getter => PrivateElement::Accessor {
                                get: Some(f),
                                set: None,
                            },
                            MethodKind::Setter => PrivateElement::Accessor {
                                get: None,
                                set: Some(f),
                            },
                        };
                        self.private_method_or_accessor_add(obj, n, e)?;
                    }
                }
            }
            Op::DefineStaticField { private, has_init } => {
                let init = if has_init {
                    self.pop().as_object()
                } else {
                    None
                };
                let key = self.pop();
                let obj = self.peek(0).as_object().expect("constructor");
                let name = if private {
                    let Value::Symbol(s) = key else {
                        unreachable!("privado")
                    };
                    FieldName::Private(s)
                } else {
                    FieldName::Key(self.to_property_key(&key)?)
                };
                if let Some(i) = init
                    && let ObjectKind::Function(c) = &mut self.heap[i].kind
                {
                    c.field_name = Some(name.clone());
                }
                self.define_field(
                    obj,
                    &ClassField {
                        name,
                        initializer: init,
                    },
                )?;
            }
            Op::CallStaticBlock => {
                let f = self.pop();
                let obj = self.peek(0).clone();
                self.call(&f, obj, &[])?;
            }
            Op::NewPrivateName(d) => {
                let desc = self.string_const(d);
                let s = self.heap.alloc(crate::value::Symbol {
                    description: Some(desc),
                    private: true,
                });
                self.push(Value::Symbol(s));
            }

            // --- Declaraciones ---
            Op::DeclareFunction(n) => {
                let v = self.pop();
                let (name, key) = self.name_const(n);
                self.declare_function_binding(&name, &key, v)?;
            }
            Op::DeclareVar(n) => {
                let (name, key) = self.name_const(n);
                self.declare_var_binding(&name, &key)?;
            }
            Op::DeclareGlobalLex { name, constant } => {
                let (name, _) = self.name_const(name);
                let g = self.global_env();
                if let Some(ge) = self.heap[g].global_mut() {
                    ge.decl.insert(name, (None, !constant));
                }
            }
            Op::InitGlobalLex(n) => {
                let v = self.pop();
                let (name, _) = self.name_const(n);
                let g = self.global_env();
                if let Some(ge) = self.heap[g].global_mut()
                    && let Some(cell) = ge.decl.get_mut(&name)
                {
                    cell.0 = Some(v);
                }
            }
            Op::SetVarEnv(n) => {
                let v = self.pop();
                let (name, key) = self.name_const(n);
                let env = frame!(self).var_env.unwrap_or_else(|| self.global_env());
                self.env_set_mutable_binding(env, &name, &key, v, false)?;
            }
            Op::IsHoisted(n) => {
                let (name, _) = self.name_const(n);
                let h = frame!(self)
                    .hoisted
                    .as_ref()
                    .is_some_and(|s| s.contains(&name));
                self.push(Value::Boolean(h));
            }
            // E1
            Op::ImportMeta => {
                let v = self.import_meta()?;
                self.push(v);
            }
            Op::ImportCall => {
                let options = self.pop();
                let specifier = self.pop();
                let v = self.evaluate_import_call(specifier, options)?;
                self.push(v);
            }
        }
        Ok(Flow::Next)
    }

    /// TypeError "x is not a function/constructor" con el texto del callee.
    fn not_callable<T>(&mut self, t: u32, callee: &Value, what: &str) -> JsResult<T> {
        let name = if t == u32::MAX {
            self.describe(callee)
        } else {
            self.string_const(t).to_string_lossy()
        };
        self.type_error(&format!("{name} is not {what}"))
    }

    fn tdz_error<T>(&mut self, n: u32) -> JsResult<T> {
        let (name, _) = self.name_const(n);
        self.reference_error(&format!("Cannot access '{name}' before initialization"))
    }

    fn nullish_access<T>(&mut self, obj: &Value, key: &Value) -> JsResult<T> {
        let what = if obj.is_null() { "null" } else { "undefined" };
        let k = match key {
            Value::String(s) => s.to_string_lossy(),
            Value::Number(n) => crate::abstract_ops::number::number_to_string(*n).to_string_lossy(),
            _ => String::from("?"),
        };
        self.type_error(&format!("Cannot read properties of {what} (reading '{k}')"))
    }

    /// El nombre de una función a partir de una clave de propiedad (SetFunctionName).
    fn function_name_from_key(&mut self, key: &Value, prefix: NamePrefix) -> JsResult<Value> {
        let base = match key {
            Value::Symbol(s) => {
                let sym = self.heap[*s].clone();
                match (sym.private, sym.description) {
                    (true, d) => d.unwrap_or_default(),
                    (false, Some(d)) => {
                        let mut u = vec![u16::from(b'[')];
                        u.extend_from_slice(d.units());
                        u.push(u16::from(b']'));
                        JsString::from_units(u)
                    }
                    (false, None) => JsString::default(),
                }
            }
            other => self.to_string(other)?,
        };
        let p = match prefix {
            NamePrefix::None => return Ok(Value::String(base)),
            NamePrefix::Get => "get ",
            NamePrefix::Set => "set ",
        };
        let mut u: Vec<u16> = p.encode_utf16().collect();
        u.extend_from_slice(base.units());
        Ok(Value::String(JsString::from_units(u)))
    }

    /// Los valores de un array denso (los argumentos de una llamada con spread).
    fn array_values(&self, arr: &Value) -> Vec<Value> {
        match arr {
            Value::Object(o) => match &self.heap[*o].kind {
                ObjectKind::Array(d) => d
                    .dense
                    .iter()
                    .map(|v| v.clone().unwrap_or_default())
                    .collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    /// El operador `new` (§13.3.5.1.1 EvaluateNew, pasos 5–7).
    fn new_operator(&mut self, callee: &Value, args: &[Value]) -> JsResult<Value> {
        match callee {
            Value::Object(o) if self.heap[*o].is_constructor() => {
                let o = *o;
                self.construct_internal(o, args, o)
            }
            _ => {
                let d = self.describe(callee);
                self.type_error(&format!("{d} is not a constructor"))
            }
        }
    }

    /// SuperCall (§13.3.7.1) pasos 3–6.
    fn super_construct(&mut self, nt: &Value, ctor: &Value, args: &[Value]) -> JsResult<Value> {
        let Value::Object(c) = ctor else {
            return self.type_error("Super constructor is not a constructor");
        };
        if !self.heap[*c].is_constructor() {
            return self.type_error("Super constructor is not a constructor");
        }
        let nt = nt.as_object().unwrap_or(*c);
        self.construct_internal(*c, args, nt)
    }

    /// Un `eval(...)` que puede ser directo (§13.3.6.1 paso 6).
    fn call_maybe_eval(
        &mut self,
        callee: &Value,
        this: Value,
        args: &[Value],
        flags: u32,
    ) -> JsResult<Value> {
        let is_eval = matches!(callee, Value::Object(o) if *o == self.intrinsics().eval);
        if !is_eval {
            let r = self.call_internal(callee, this, args)?;
            return Ok(r);
        }
        let Some(x) = args.first() else {
            return Ok(Value::Undefined);
        };
        self.perform_eval(x, true, crate::compiler::EvalFlags::from_bits(flags))
    }

    /// Un paso de un IteratorRecord guardado en los slots: el valor, o `None` si terminó.
    fn iter_step_slot(&mut self, slot: u32) -> JsResult<Option<Value>> {
        let it = self.local(slot);
        let next = self.local(slot + 1);
        let r = self.iterator_step_value(&it, &next);
        match r {
            Ok(Some(v)) => Ok(Some(v)),
            Ok(None) => {
                frame!(self).locals[slot as usize + 2] = Some(Value::Boolean(true));
                Ok(None)
            }
            Err(e) => {
                frame!(self).locals[slot as usize + 2] = Some(Value::Boolean(true));
                Err(e)
            }
        }
    }

    /// Un paso de `yield*` (§15.5.5, YieldExpression : yield * AssignmentExpression).
    fn yield_star_step(&mut self, slot: u32, ks: u32, vs: u32, done: u32) -> JsResult<Flow> {
        let it = self.local(slot);
        let next = self.local(slot + 1);
        let kind = match self.local(ks) {
            Value::Number(n) => n as u32,
            _ => COMPLETION_NORMAL,
        };
        let received = self.local(vs);
        let inner = match kind {
            COMPLETION_NORMAL => self.call(&next, it.clone(), &[received])?,
            COMPLETION_THROW => {
                let Value::Object(io) = &it else {
                    return self.type_error("iterator is not an object");
                };
                let k = self.keys.throw.clone();
                let throw = self.get_method_of(&Value::Object(*io), &k)?;
                match throw {
                    Some(t) => self.call(&t, it.clone(), &[received])?,
                    None => {
                        // Sin `throw`: se cierra el iterador y TypeError (paso 7.b.iii).
                        self.iterator_close(&it)?;
                        return self.type_error("The iterator does not provide a 'throw' method");
                    }
                }
            }
            _ => {
                let k = self.keys.return_.clone();
                let ret = self.get_method_of(&it, &k)?;
                match ret {
                    None => return Ok(Flow::Return(received)),
                    Some(r) => {
                        let res = self.call(&r, it.clone(), &[received])?;
                        let Value::Object(ro) = res else {
                            return self.type_error("iterator result is not an object");
                        };
                        if self.iterator_complete(ro)? {
                            let v = self.iterator_value(ro)?;
                            return Ok(Flow::Return(v));
                        }
                        self.push(Value::Object(ro));
                        return Ok(Flow::Next);
                    }
                }
            }
        };
        let Value::Object(io) = inner else {
            return self.type_error("iterator result is not an object");
        };
        if self.iterator_complete(io)? {
            let v = self.iterator_value(io)?;
            self.push(v);
            frame!(self).pc = done as usize;
            return Ok(Flow::Next);
        }
        self.push(Value::Object(io));
        Ok(Flow::Next)
    }

    /// GetTemplateObject (§13.2.8.4).
    fn get_template_object(&mut self, site: &Rc<TemplateSite>) -> JsResult<Gc<Object>> {
        if let Some(o) = self.realm().template_map.get(&site.id) {
            return Ok(*o);
        }
        let n = site.raw.len();
        let template = self.array_create(n as u64, None)?;
        let raw = self.array_create(n as u64, None)?;
        for i in 0..n {
            let cooked = site.cooked[i]
                .clone()
                .map_or(Value::Undefined, Value::String);
            self.define_property_or_throw(
                template,
                PropertyKey::from(i as u32),
                PropertyDescriptor::data(cooked, false, true, false),
            )?;
            self.define_property_or_throw(
                raw,
                PropertyKey::from(i as u32),
                PropertyDescriptor::data(Value::String(site.raw[i].clone()), false, true, false),
            )?;
        }
        self.set_integrity_level(raw, true)?;
        self.define_property_or_throw(
            template,
            PropertyKey::from("raw"),
            PropertyDescriptor::data(Value::Object(raw), false, false, false),
        )?;
        self.set_integrity_level(template, true)?;
        let realm = self.current_realm;
        self.realms[realm].template_map.insert(site.id, template);
        Ok(template)
    }

    /// Crea el constructor de una clase y su prototipo (§15.7.14 pasos 8–18).
    fn class_create(
        &mut self,
        code: Rc<FunctionCode>,
        env: Gc<Env>,
        proto_parent: Option<Gc<Object>>,
        ctor_parent: Option<Gc<Object>>,
        name: Value,
    ) -> (Gc<Object>, Gc<Object>) {
        let proto = self.new_object(proto_parent);
        let realm = self.current_realm;
        let length = f64::from(code.length);
        let f = self.heap.alloc(Object::new(
            ctor_parent,
            ObjectKind::Function(Box::new(Closure {
                code,
                env,
                home_object: Some(proto),
                realm,
                is_constructor: true,
                class: None,
                field_name: None,
            })),
        ));
        let k = self.keys.length.clone();
        self.put_data(f, k, Value::Number(length), false, false, true);
        self.set_function_name_value(f, name);
        let k = self.keys.prototype.clone();
        self.put_data(f, k, Value::Object(proto), false, false, false);
        let k = self.keys.constructor.clone();
        self.put_data(proto, k, Value::Object(f), true, false, true);
        (f, proto)
    }

    /// El binding de una declaración de función de script o eval (§16.1.7 paso 16,
    /// §19.2.1.3 paso 17).
    fn declare_function_binding(
        &mut self,
        name: &Atom,
        key: &PropertyKey,
        v: Value,
    ) -> JsResult<()> {
        let f = self.frames.last().expect("frame");
        let is_eval = f.code.kind == CodeKind::Eval;
        let var_env = f.var_env;
        match (is_eval, var_env) {
            (false, _) | (true, None) => {
                let g = self.global_env();
                self.global_create_function_binding(g, name, v, is_eval)
            }
            (true, Some(env)) => {
                if self.heap[env].global().is_some() {
                    return self.global_create_function_binding(env, name, v, true);
                }
                if self.env_has_binding(env, name, key)? {
                    self.env_set_mutable_binding(env, name, key, v, false)
                } else {
                    if let Some(d) = self.heap[env].decl_mut() {
                        d.extra.push((name.clone(), Some(v)));
                    }
                    Ok(())
                }
            }
        }
    }

    /// El binding de un `var` de script o eval.
    fn declare_var_binding(&mut self, name: &Atom, key: &PropertyKey) -> JsResult<()> {
        let f = self.frames.last().expect("frame");
        let is_eval = f.code.kind == CodeKind::Eval;
        let var_env = f.var_env;
        match (is_eval, var_env) {
            (false, _) | (true, None) => {
                let g = self.global_env();
                self.global_create_var_binding(g, name, is_eval)
            }
            (true, Some(env)) => {
                if self.heap[env].global().is_some() {
                    return self.global_create_var_binding(env, name, true);
                }
                if !self.env_has_binding(env, name, key)?
                    && let Some(d) = self.heap[env].decl_mut()
                {
                    d.extra.push((name.clone(), Some(Value::Undefined)));
                }
                Ok(())
            }
        }
    }
}

/// Agrega un método privado a la lista (combinando getter y setter del mismo nombre).
fn add_private_element(
    list: &mut Vec<(Gc<crate::value::Symbol>, PrivateElement)>,
    name: Gc<crate::value::Symbol>,
    kind: MethodKind,
    f: Gc<Object>,
) {
    if let Some((_, PrivateElement::Accessor { get, set })) =
        list.iter_mut().find(|(n, _)| *n == name)
    {
        match kind {
            MethodKind::Getter => *get = Some(f),
            MethodKind::Setter => *set = Some(f),
            MethodKind::Method => {}
        }
        return;
    }
    let e = match kind {
        MethodKind::Method => PrivateElement::Method(f),
        MethodKind::Getter => PrivateElement::Accessor {
            get: Some(f),
            set: None,
        },
        MethodKind::Setter => PrivateElement::Accessor {
            get: None,
            set: Some(f),
        },
    };
    list.push((name, e));
}

// --- E3: iteración asíncrona (`for await`, `yield*` en generadores async) ---

/// Los modos de `yield*` asíncrono entre `AsyncYieldStarCall` y `AsyncYieldStarResult`.
const YS_NEXT: i32 = 0;
const YS_THROW: i32 = 1;
const YS_RETURN: i32 = 2;
/// Sin método `return`: el valor recibido se espera y se retorna (§15.5.5 paso 6.c.iii).
const YS_RETURN_DIRECT: i32 = 3;
/// Sin método `throw`: se cerró el iterador y queda lanzar TypeError (paso 6.b.iii).
const YS_CLOSE: i32 = 4;

impl Agent {
    /// ¿La completion guardada en `kind_slot` es throw? (`u32::MAX`: completion normal.)
    fn completion_is_throw(&self, kind_slot: u32) -> bool {
        kind_slot != u32::MAX
            && matches!(self.local(kind_slot), Value::Number(n) if n as u32 == COMPLETION_THROW)
    }

    /// `for await`: el resultado ya esperado de `next()` (§14.7.5.7 pasos 6.c–f).
    fn async_iter_result(&mut self, slot: u32, done: u32) -> JsResult<Flow> {
        let r = self.pop();
        let Value::Object(ro) = r else {
            frame!(self).locals[slot as usize + 2] = Some(Value::Boolean(true));
            return self.type_error("Iterator result is not an object");
        };
        let complete = self.iterator_complete(ro);
        if !matches!(complete, Ok(false)) {
            frame!(self).locals[slot as usize + 2] = Some(Value::Boolean(true));
        }
        if complete? {
            frame!(self).pc = done as usize;
            return Ok(Flow::Next);
        }
        let v = self.iterator_value(ro);
        if v.is_err() {
            frame!(self).locals[slot as usize + 2] = Some(Value::Boolean(true));
        }
        self.push(v?);
        Ok(Flow::Next)
    }

    /// AsyncIteratorClose (§7.4.13) hasta el `Await` de `return()`.
    fn async_iter_close(&mut self, slot: u32, kind_slot: u32, skip: u32) -> JsResult<Flow> {
        let throw = self.completion_is_throw(kind_slot);
        if matches!(self.local(slot + 2), Value::Boolean(true)) {
            frame!(self).pc = skip as usize;
            return Ok(Flow::Next);
        }
        let it = self.local(slot);
        let k = self.keys.return_.clone();
        let ret = match self.get_method_of(&it, &k) {
            Ok(r) => r,
            Err(Throw::Value(_)) if throw => None,
            Err(e) => return Err(e),
        };
        let Some(ret) = ret else {
            frame!(self).pc = skip as usize;
            return Ok(Flow::Next);
        };
        match self.call(&ret, it, &[]) {
            Ok(r) => {
                self.push(r);
                Ok(Flow::Next)
            }
            Err(Throw::Value(_)) if throw => {
                frame!(self).pc = skip as usize;
                Ok(Flow::Next)
            }
            Err(e) => Err(e),
        }
    }

    /// `yield*` asíncrono (§15.5.5, pasos 6.a.i, 6.b.ii.1, 6.b.iii, 6.c.ii–iv): la llamada
    /// al iterador interno según la completion recibida; el valor se espera a continuación.
    fn async_yield_star_call(&mut self, slot: u32, ks: u32, vs: u32) -> JsResult<Flow> {
        let it = self.local(slot);
        let next = self.local(slot + 1);
        let kind = match self.local(ks) {
            Value::Number(n) => n as u32,
            _ => COMPLETION_NORMAL,
        };
        let received = self.local(vs);
        let (mode, v) = match kind {
            COMPLETION_NORMAL => (YS_NEXT, self.call(&next, it, &[received])?),
            COMPLETION_THROW => {
                let k = self.keys.throw.clone();
                match self.get_method_of(&it, &k)? {
                    Some(t) => (YS_THROW, self.call(&t, it, &[received])?),
                    None => {
                        // AsyncIteratorClose(iteratorRecord, NormalCompletion(empty)).
                        let k = self.keys.return_.clone();
                        match self.get_method_of(&it, &k)? {
                            None => {
                                return self
                                    .type_error("The iterator does not provide a 'throw' method");
                            }
                            Some(r) => (YS_CLOSE, self.call(&r, it, &[])?),
                        }
                    }
                }
            }
            _ => {
                let k = self.keys.return_.clone();
                match self.get_method_of(&it, &k)? {
                    None => (YS_RETURN_DIRECT, received),
                    Some(r) => (YS_RETURN, self.call(&r, it, &[received])?),
                }
            }
        };
        self.push(Value::Number(f64::from(mode)));
        self.push(v);
        Ok(Flow::Next)
    }

    /// `yield*` asíncrono después del `Await`: el resultado del iterador interno.
    fn async_yield_star_result(&mut self, _slot: u32, done: u32) -> JsResult<Flow> {
        let r = self.pop();
        let mode = match self.pop() {
            Value::Number(n) => n as i32,
            _ => YS_NEXT,
        };
        if mode == YS_RETURN_DIRECT {
            return Ok(Flow::Return(r));
        }
        let Value::Object(ro) = r else {
            return self.type_error("iterator result is not an object");
        };
        if mode == YS_CLOSE {
            return self.type_error("The iterator does not provide a 'throw' method");
        }
        let complete = self.iterator_complete(ro)?;
        let v = self.iterator_value(ro)?;
        if complete {
            if mode == YS_RETURN {
                return Ok(Flow::Return(v));
            }
            self.push(v);
            frame!(self).pc = done as usize;
            return Ok(Flow::Next);
        }
        let _ = YS_THROW;
        self.push(v);
        Ok(Flow::Next)
    }
}

/// El resultado de [`fast_op`].
enum Fast {
    /// Hecho.
    Done,
    /// Hecho; fue un salto hacia atrás (hay que pasar por el safepoint).
    BackJump,
    /// La instrucción necesita el agente: la resuelve el `match` general de `step`.
    Slow,
}

/// Valor de verdad de los primitivos (ToBoolean, §7.1.2); `None` para objetos y BigInt (los
/// objetos con `[[IsHTMLDDA]]` y los BigInt los resuelve el agente).
#[inline]
fn primitive_truthiness(v: &Value) -> Option<bool> {
    match v {
        Value::Undefined | Value::Null => Some(false),
        Value::Boolean(b) => Some(*b),
        Value::Number(n) => Some(!(*n == 0.0 || n.is_nan())),
        Value::String(s) => Some(!s.is_empty()),
        Value::Symbol(_) => Some(true),
        Value::Object(_) | Value::BigInt(_) => None,
    }
}

/// Las instrucciones más frecuentes que no necesitan el agente (locales, constantes, pila,
/// saltos y aritmética Number × Number). Deja el frame igual si devuelve [`Fast::Slow`].
#[inline]
fn fast_op(f: &mut Frame, heap: &mut crate::heap::Heap, cx: &FastCx, op: Op) -> Fast {
    let genv = cx.genv;
    use crate::abstract_ops::conversion::{to_int32, to_uint32};
    macro_rules! jump {
        ($t:expr) => {{
            let t = $t as usize;
            let back = t < f.pc;
            f.pc = t;
            if back { Fast::BackJump } else { Fast::Done }
        }};
    }
    match op {
        Op::GetLocal(s) => {
            let v = f.locals[s as usize].clone().unwrap_or_default();
            f.stack.push(v);
        }
        Op::GetLocalChecked(s, _) => match &f.locals[s as usize] {
            Some(v) => {
                let v = v.clone();
                f.stack.push(v);
            }
            None => return Fast::Slow,
        },
        Op::SetLocal(s) => {
            let v = f.stack.last().expect("pila").clone();
            f.locals[s as usize] = Some(v);
        }
        Op::InitLocal(s) => {
            let v = f.stack.pop().expect("pila");
            f.locals[s as usize] = Some(v);
        }
        Op::Pop => {
            f.stack.pop();
        }
        Op::Dup => {
            let v = f.stack.last().expect("pila").clone();
            f.stack.push(v);
        }
        Op::GetArg(i) => {
            let v = f.args.get(i as usize).cloned().unwrap_or_default();
            f.stack.push(v);
        }
        Op::Ctx(CtxWhat::This, CtxLoc::Frame) => match &f.this {
            Some(v) => {
                let v = v.clone();
                f.stack.push(v);
            }
            None => return Fast::Slow,
        },
        Op::ToNumeric => {
            if !matches!(f.stack.last(), Some(Value::Number(_))) {
                return Fast::Slow;
            }
        }
        Op::SetLocalChecked(s, _) => {
            if f.locals[s as usize].is_none() {
                return Fast::Slow;
            }
            let v = f.stack.last().expect("pila").clone();
            f.locals[s as usize] = Some(v);
        }
        Op::Swap => {
            let n = f.stack.len();
            f.stack.swap(n - 1, n - 2);
        }
        Op::GetEnv(h, s) | Op::GetEnvChecked(h, s, _) => {
            let Some(v) = env_slot(heap, f.env, h, s).and_then(|v| v.clone()) else {
                return Fast::Slow;
            };
            f.stack.push(v);
        }
        Op::SetEnv(h, s) | Op::SetEnvChecked(h, s, _) | Op::InitEnv(h, s) => {
            let e = env_at_heap(heap, f.env, h);
            let Some(d) = heap[e].decl_mut() else {
                return Fast::Slow;
            };
            let slot = &mut d.slots[s as usize];
            if matches!(op, Op::SetEnvChecked(..)) && slot.is_none() {
                return Fast::Slow;
            }
            *slot = Some(if matches!(op, Op::InitEnv(..)) {
                f.stack.pop().expect("pila")
            } else {
                f.stack.last().expect("pila").clone()
            });
        }
        Op::GetElem | Op::GetElemKeep => {
            let n = f.stack.len();
            let v = match (&f.stack[n - 2], &f.stack[n - 1]) {
                (Value::Object(o), Value::Number(i)) => match dense_index(heap, *o, *i) {
                    Some(v) => v,
                    None => return Fast::Slow,
                },
                _ => return Fast::Slow,
            };
            f.stack.pop();
            if op == Op::GetElem {
                *f.stack.last_mut().expect("pila") = v;
            } else {
                f.stack.push(v);
            }
        }
        Op::SetElem(_) => {
            let n = f.stack.len();
            let (Value::Object(o), Value::Number(i)) = (&f.stack[n - 3], &f.stack[n - 2]) else {
                return Fast::Slow;
            };
            let (o, i) = (*o, *i);
            let ObjectKind::Array(d) = &mut heap[o].kind else {
                return Fast::Slow;
            };
            let idx = i as usize;
            if i < 0.0 || i.fract() != 0.0 || idx >= d.dense.len() || d.dense[idx].is_none() {
                return Fast::Slow;
            }
            let v = f.stack.pop().expect("pila");
            d.dense[idx] = Some(v.clone());
            f.stack.truncate(n - 3);
            f.stack.push(v);
        }
        Op::GetProp(c) | Op::GetPropKeep(c) => {
            let Some(Value::Object(o)) = f.stack.last() else {
                return Fast::Slow;
            };
            let o = *o;
            let Some(v) = fast_ic_get(heap, &f.code, c, o) else {
                return Fast::Slow;
            };
            if op == Op::GetProp(c) {
                *f.stack.last_mut().expect("pila") = v;
            } else {
                f.stack.push(v);
            }
        }
        Op::SetProp(c, _) => {
            let n = f.stack.len();
            let Value::Object(o) = &f.stack[n - 2] else {
                return Fast::Slow;
            };
            let o = *o;
            if !fast_ic_set(heap, &f.code, c, o, &f.stack[n - 1]) {
                return Fast::Slow;
            }
            let v = f.stack.pop().expect("pila");
            *f.stack.last_mut().expect("pila") = v;
        }
        Op::GetGlobal(c) | Op::GetGlobalForCall(c) => {
            let Some(v) = fast_ic_global(heap, &f.code, c, genv) else {
                return Fast::Slow;
            };
            f.stack.push(v);
            if matches!(op, Op::GetGlobalForCall(_)) {
                f.stack.push(Value::Undefined);
            }
        }
        // Literales: crear un objeto no recolecta (la recolección es en los safepoints).
        Op::NewObject => {
            let o = heap.alloc(Object::ordinary(Some(cx.object_proto)));
            f.stack.push(Value::Object(o));
        }
        Op::NewArray => {
            let a = heap.alloc(Object::new(
                Some(cx.array_proto),
                ObjectKind::Array(crate::object::ArrayData {
                    dense: Vec::new(),
                    length: 0,
                    length_writable: true,
                }),
            ));
            f.stack.push(Value::Object(a));
        }
        Op::ArrayPush => {
            let v = f.stack.pop().expect("pila");
            let Some(Value::Object(a)) = f.stack.last() else {
                f.stack.push(v);
                return Fast::Slow;
            };
            if let ObjectKind::Array(d) = &mut heap[*a].kind {
                d.dense.push(Some(v));
                d.length += 1;
            } else {
                f.stack.push(v);
                return Fast::Slow;
            }
        }
        Op::InitProp(c) => {
            let n = f.stack.len();
            let Value::Object(o) = &f.stack[n - 2] else {
                return Fast::Slow;
            };
            let o = *o;
            if !fast_ic_init(heap, &f.code, c, o, &f.stack[n - 1]) {
                return Fast::Slow;
            }
            f.stack.pop();
        }
        Op::Undefined => f.stack.push(Value::Undefined),
        Op::Null => f.stack.push(Value::Null),
        Op::True => f.stack.push(Value::Boolean(true)),
        Op::False => f.stack.push(Value::Boolean(false)),
        Op::Int(i) => f.stack.push(Value::Number(f64::from(i))),
        Op::Const(i) => match &f.code.consts[i as usize] {
            Constant::Number(n) => {
                let n = *n;
                f.stack.push(Value::Number(n));
            }
            Constant::String(s) => {
                let s = s.clone();
                f.stack.push(Value::String(s));
            }
            _ => return Fast::Slow,
        },
        Op::Jump(t) => return jump!(t),
        Op::JumpIfTrue(t) | Op::JumpIfFalse(t) => {
            let Some(b) = f.stack.last().and_then(primitive_truthiness) else {
                return Fast::Slow;
            };
            f.stack.pop();
            if b == matches!(op, Op::JumpIfTrue(_)) {
                return jump!(t);
            }
        }
        Op::JumpIfTrueKeep(t) | Op::JumpIfFalseKeep(t) => {
            let Some(b) = f.stack.last().and_then(primitive_truthiness) else {
                return Fast::Slow;
            };
            // Salta sin sacar; si no salta, saca.
            if b == matches!(op, Op::JumpIfTrueKeep(_)) {
                return jump!(t);
            }
            f.stack.pop();
        }
        Op::Inc | Op::Dec => match f.stack.last_mut() {
            Some(Value::Number(n)) => {
                *n = if op == Op::Inc { *n + 1.0 } else { *n - 1.0 };
            }
            _ => return Fast::Slow,
        },
        Op::Not => {
            let Some(b) = f.stack.last().and_then(primitive_truthiness) else {
                return Fast::Slow;
            };
            *f.stack.last_mut().expect("pila") = Value::Boolean(!b);
        }
        Op::Add
        | Op::Sub
        | Op::Mul
        | Op::Div
        | Op::Mod
        | Op::Shl
        | Op::Shr
        | Op::UShr
        | Op::BitAnd
        | Op::BitOr
        | Op::BitXor
        | Op::Lt
        | Op::Gt
        | Op::Le
        | Op::Ge
        | Op::StrictEq
        | Op::StrictNe => {
            let n = f.stack.len();
            let (a, b) = match (f.stack.get(n.wrapping_sub(2)), f.stack.get(n.wrapping_sub(1))) {
                (Some(Value::Number(a)), Some(Value::Number(b))) => (*a, *b),
                // Concatenación de primitivos sin conversiones con efectos (§13.15.3).
                (Some(l), Some(r)) if op == Op::Add => {
                    let Some(v) = fast_concat(l, r) else {
                        return Fast::Slow;
                    };
                    f.stack.pop();
                    *f.stack.last_mut().expect("pila") = v;
                    return Fast::Done;
                }
                _ => return Fast::Slow,
            };
            let v = match op {
                Op::Add => Value::Number(a + b),
                Op::Sub => Value::Number(a - b),
                Op::Mul => Value::Number(a * b),
                Op::Div => Value::Number(a / b),
                Op::Mod => Value::Number(crate::abstract_ops::number::remainder(a, b)),
                Op::Shl => Value::Number(f64::from(to_int32(a).wrapping_shl(to_uint32(b) & 31))),
                Op::Shr => Value::Number(f64::from(to_int32(a) >> (to_uint32(b) & 31))),
                Op::UShr => Value::Number(f64::from(to_uint32(a) >> (to_uint32(b) & 31))),
                Op::BitAnd => Value::Number(f64::from(to_int32(a) & to_int32(b))),
                Op::BitOr => Value::Number(f64::from(to_int32(a) | to_int32(b))),
                Op::BitXor => Value::Number(f64::from(to_int32(a) ^ to_int32(b))),
                Op::Lt => Value::Boolean(a < b),
                Op::Gt => Value::Boolean(a > b),
                Op::Le => Value::Boolean(a <= b),
                Op::Ge => Value::Boolean(a >= b),
                Op::StrictEq => Value::Boolean(a == b),
                _ => Value::Boolean(a != b),
            };
            f.stack.pop();
            *f.stack.last_mut().expect("pila") = v;
        }
        _ => return Fast::Slow,
    }
    Fast::Done
}

/// La clave de la constante de nombre `n` (prestada del código).
#[inline]
fn name_key(code: &FunctionCode, n: u32) -> &PropertyKey {
    match &code.consts[n as usize] {
        Constant::Name(_, k) => k,
        other => unreachable!("se esperaba un nombre: {other:?}"),
    }
}

/// El environment a `hops` saltos de `env` (solo con el heap).
#[inline]
fn env_at_heap(heap: &crate::heap::Heap, mut env: Gc<Env>, hops: u32) -> Gc<Env> {
    for _ in 0..hops {
        env = heap[env].outer.expect("environment externo");
    }
    env
}

/// El slot `s` del environment declarativo a `hops` saltos.
#[inline]
fn env_slot(heap: &crate::heap::Heap, env: Gc<Env>, hops: u32, s: u32) -> Option<&Option<Value>> {
    let e = env_at_heap(heap, env, hops);
    heap[e].decl().map(|d| &d.slots[s as usize])
}

/// El elemento `i` de un Array denso (un índice entero dentro del vector, sin hueco).
#[inline]
fn dense_index(heap: &crate::heap::Heap, o: Gc<Object>, i: f64) -> Option<Value> {
    let ObjectKind::Array(d) = &heap[o].kind else {
        return None;
    };
    if i < 0.0 || i.fract() != 0.0 {
        return None;
    }
    d.dense.get(i as usize)?.clone()
}

/// ¿El `[[Get]]`/`[[Set]]` con nombre de `o` es el ordinario sin mirar el agente (los objetos
/// host van por `step`)?
#[inline]
fn fast_named(heap: &crate::heap::Heap, o: Gc<Object>, key: &PropertyKey) -> bool {
    match &heap[o].kind {
        ObjectKind::Ordinary | ObjectKind::Function(_) | ObjectKind::Builtin(_) | ObjectKind::Error => true,
        ObjectKind::Array(_) => !matches!(key, PropertyKey::String(s) if s.units() == [108, 101, 110, 103, 116, 104]),
        _ => false,
    }
}

/// Un acierto de la inline cache de `GetProp` con una propiedad de datos (sin el agente).
#[inline]
fn fast_ic_get(heap: &crate::heap::Heap, code: &FunctionCode, c: u32, o: Gc<Object>) -> Option<Value> {
    use crate::vm::ic::Ic;
    let key = name_key(code, c);
    if !fast_named(heap, o, key) {
        return None;
    }
    let shape = heap[o].props.shape_id();
    code.ics
        .with(c, |site| {
            site.ways.iter().find_map(|w| match w {
                Ic::Own { shape: s, slot } if *s == shape => match heap[o].props.slot(*slot as usize) {
                    Some(crate::object::Prop {
                        value: crate::object::PropValue::Data { value, .. },
                        ..
                    }) => Some(value.clone()),
                    _ => None,
                },
                Ic::Missing {
                    shape: s,
                    proto,
                    proto_shape,
                } if *s == shape
                    && heap[o].proto == Some(*proto)
                    && heap[*proto].proto.is_none()
                    && fast_named(heap, *proto, key)
                    && heap[*proto].props.shape_id() == *proto_shape =>
                {
                    Some(Value::Undefined)
                }
                Ic::Proto {
                    shape: s,
                    proto,
                    proto_shape,
                    slot,
                } if *s == shape
                    && heap[o].proto == Some(*proto)
                    && fast_named(heap, *proto, key)
                    && heap[*proto].props.shape_id() == *proto_shape =>
                {
                    match heap[*proto].props.slot(*slot as usize) {
                        Some(crate::object::Prop {
                            value: crate::object::PropValue::Data { value, .. },
                            ..
                        }) => Some(value.clone()),
                        _ => None,
                    }
                }
                _ => None,
            })
        })
        .flatten()
}

/// Un acierto de la inline cache de `SetProp` (propiedad de datos escribible, o la
/// transición que la agrega) sin el agente.
#[inline]
fn fast_ic_set(heap: &mut crate::heap::Heap, code: &FunctionCode, c: u32, o: Gc<Object>, v: &Value) -> bool {
    use crate::vm::ic::Ic;
    let key = name_key(code, c);
    if !fast_named(heap, o, key) {
        return false;
    }
    let shape = heap[o].props.shape_id();
    enum Hit {
        Own(u32),
        Add(Option<Gc<Object>>, u64, Rc<crate::object::Shape>),
    }
    let hit = code.ics.with(c, |site| {
        site.ways.iter().find_map(|w| match w {
            Ic::Own { shape: s, slot } if *s == shape => Some(Hit::Own(*slot)),
            Ic::Add { from, proto, epoch, to } if *from == shape => Some(Hit::Add(*proto, *epoch, to.clone())),
            _ => None,
        })
    });
    match hit.flatten() {
        Some(Hit::Own(slot)) => match heap[o].props.slot_mut(slot as usize) {
            Some(crate::object::Prop {
                value: crate::object::PropValue::Data { value, writable: true },
                ..
            }) => {
                *value = v.clone();
                true
            }
            _ => false,
        },
        Some(Hit::Add(proto, epoch, to)) => {
            let obj = &mut heap[o];
            if obj.proto != proto || !obj.extensible || epoch != crate::object::setter_epoch() {
                return false;
            }
            obj.props
                .push_with_shape(to, crate::object::Prop::data(v.clone(), true, true, true));
            true
        }
        None => false,
    }
}

/// Un acierto de la inline cache de `GetGlobal` con una propiedad de datos del objeto global.
#[inline]
fn fast_ic_global(heap: &crate::heap::Heap, code: &FunctionCode, c: u32, genv: Gc<Env>) -> Option<Value> {
    use crate::vm::ic::Ic;
    let g = heap[genv].global()?;
    let (object, decl_len) = (g.object, g.decl.len() as u32);
    let props = &heap[object].props;
    let shape = props.shape_id();
    code.ics
        .with(c, |site| {
            site.ways.iter().find_map(|w| match w {
                Ic::Global {
                    shape: s,
                    slot,
                    decl_len: d,
                } if *s == shape && *d == decl_len => match props.slot(*slot as usize) {
                    Some(crate::object::Prop {
                        value: crate::object::PropValue::Data { value, .. },
                        ..
                    }) => Some(value.clone()),
                    _ => None,
                },
                _ => None,
            })
        })
        .flatten()
}

/// `l + r` con un string y un string o un número (ToString del número, §6.1.6.1.20).
#[inline]
fn fast_concat(l: &Value, r: &Value) -> Option<Value> {
    use crate::abstract_ops::number::number_to_string;
    let (a, b) = match (l, r) {
        (Value::String(a), Value::String(b)) => (a.clone(), b.clone()),
        (Value::String(a), Value::Number(n)) => (a.clone(), number_to_string(*n)),
        (Value::Number(n), Value::String(b)) => (number_to_string(*n), b.clone()),
        _ => return None,
    };
    if a.is_empty() {
        return Some(Value::String(b));
    }
    if b.is_empty() {
        return Some(Value::String(a));
    }
    let mut u = Vec::with_capacity(a.len() + b.len());
    u.extend_from_slice(a.units());
    u.extend_from_slice(b.units());
    Some(Value::String(JsString::from_units(u)))
}

/// Lo que la ruta rápida necesita del realm.
struct FastCx {
    genv: Gc<Env>,
    object_proto: Gc<Object>,
    array_proto: Gc<Object>,
}

/// Un acierto de la inline cache de `InitProp` (la transición del literal).
#[inline]
fn fast_ic_init(heap: &mut crate::heap::Heap, code: &FunctionCode, c: u32, o: Gc<Object>, v: &Value) -> bool {
    use crate::vm::ic::Ic;
    let obj = &heap[o];
    if !matches!(obj.kind, ObjectKind::Ordinary) || !obj.extensible {
        return false;
    }
    let shape = obj.props.shape_id();
    let to = code.ics.with(c, |site| {
        site.ways.iter().find_map(|w| match w {
            Ic::Init { from, to } if *from == shape => Some(to.clone()),
            _ => None,
        })
    });
    let Some(Some(to)) = to else {
        return false;
    };
    heap[o]
        .props
        .push_with_shape(to, crate::object::Prop::data(v.clone(), true, true, true));
    true
}
