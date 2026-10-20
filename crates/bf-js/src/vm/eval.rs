//! Evaluación de scripts (§16.1.6 ScriptEvaluation con GlobalDeclarationInstantiation
//! §16.1.7), `eval` (§19.2.1.1 PerformEval con EvalDeclarationInstantiation §19.2.1.3) y
//! CreateDynamicFunction (§20.2.1.1.1).
//!
//! Las verificaciones de GlobalDeclarationInstantiation y EvalDeclarationInstantiation (las
//! que lanzan antes de crear ningún binding) y las decisiones de B.3.2.2/B.3.2.3 se hacen aquí;
//! la creación de los bindings la hace el prólogo del bytecode.

use std::collections::HashSet;
use std::rc::Rc;

use crate::compiler::bytecode::{CodeKind, Declarations, FunctionCode, Source};
use crate::compiler::{EvalFlags, compile_dynamic_function, compile_eval, compile_script};
use crate::heap::Gc;
use crate::object::Object;
use crate::parser::ParseOptions;
use crate::string::{Atom, JsString};
use crate::value::{PropertyKey, Value};
use crate::vm::env::{Env, EnvKind};
use crate::vm::frame::Frame;
use crate::vm::interp::FrameExit;
use crate::vm::{Agent, JsResult};

/// Las clases de función que crea CreateDynamicFunction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DynamicKind {
    Normal,
    Generator,
    Async,
    AsyncGenerator,
}

impl Agent {
    fn parse_options(&self) -> ParseOptions {
        ParseOptions {
            strict: false,
            max_stack: self.options.max_stack.max(512 * 1024),
            ..ParseOptions::default()
        }
    }

    /// ParseScript (§16.1.5) + ScriptEvaluation (§16.1.6): el valor de completion.
    pub fn eval_script(&mut self, text: &str, name: &str) -> JsResult<Value> {
        let code = self.compile_script_text(text, name)?;
        self.script_evaluation(code)
    }

    /// Compila un script (los errores de parsing son SyntaxError del realm actual).
    pub fn compile_script_text(&mut self, text: &str, name: &str) -> JsResult<Rc<FunctionCode>> {
        let opts = self.parse_options();
        let program = match crate::parse_script(text, &opts) {
            Ok(p) => p,
            Err(e) => return self.syntax_error(&e.message),
        };
        Ok(compile_script(
            &program,
            Rc::new(Source::new(name, text)),
            false,
        ))
    }

    /// ScriptEvaluation (§16.1.6) de un script ya compilado.
    pub fn script_evaluation(&mut self, code: Rc<FunctionCode>) -> JsResult<Value> {
        let genv = self.global_env();
        let decls = code.decls.clone().unwrap_or_default();
        let hoisted = self.global_declaration_checks(genv, &decls)?;
        let realm = self.current_realm;
        let mut frame = Frame::new(code, genv, realm);
        frame.this = Some(Value::Object(
            self.heap[genv].global().expect("global").this,
        ));
        frame.hoisted = Some(Rc::new(hoisted));
        let (exit, _) = self.run_frame(frame)?;
        match exit {
            FrameExit::Return(v) => Ok(v),
            _ => Ok(Value::Undefined),
        }
    }

    /// GlobalDeclarationInstantiation (§16.1.7) pasos 3–11 y B.3.2.2: verifica y decide las
    /// funciones de bloque elevadas (creando sus bindings).
    fn global_declaration_checks(
        &mut self,
        env: Gc<Env>,
        d: &Declarations,
    ) -> JsResult<HashSet<Atom>> {
        // §16.1.7 paso 3 (ES2025 ya no tiene [[VarNames]]: un `var` creado por `eval` es
        // configurable y no impide la declaración léxica).
        for (name, _) in &d.lex_names {
            if self.global_has_lexical_declaration(env, name) {
                return self
                    .syntax_error(&format!("Identifier '{name}' has already been declared"));
            }
            if self.global_has_restricted_property(env, name)? {
                return self
                    .syntax_error(&format!("Identifier '{name}' has already been declared"));
            }
        }
        for name in d.var_names.iter().chain(d.function_names.iter()) {
            if self.global_has_lexical_declaration(env, name) {
                return self
                    .syntax_error(&format!("Identifier '{name}' has already been declared"));
            }
        }
        for name in &d.function_names {
            if !self.global_can_declare_function(env, name)? {
                return self.type_error(&format!("Cannot declare global function '{name}'"));
            }
        }
        for name in &d.var_names {
            if !self.global_can_declare_var(env, name)? {
                return self.type_error(&format!("Cannot declare global variable '{name}'"));
            }
        }
        let mut hoisted = HashSet::new();
        if !d.strict {
            let mut declared: Vec<Atom> = d
                .function_names
                .iter()
                .chain(d.var_names.iter())
                .cloned()
                .collect();
            for f in &d.annex_b {
                if self.global_has_lexical_declaration(env, f) {
                    continue;
                }
                if self.global_can_declare_var(env, f)? {
                    if !declared.contains(f) {
                        self.global_create_var_binding(env, f, false)?;
                        declared.push(f.clone());
                    }
                    hoisted.insert(f.clone());
                }
            }
        }
        Ok(hoisted)
    }

    /// PerformEval (§19.2.1.1).
    pub fn perform_eval(&mut self, x: &Value, direct: bool, flags: EvalFlags) -> JsResult<Value> {
        let Value::String(s) = x else {
            return Ok(x.clone());
        };
        let text = s.to_string_lossy();
        let flags = if direct { flags } else { EvalFlags::default() };
        // El contexto del parser: los nombres privados visibles en los environments.
        let caller_env = if direct {
            self.frames.last().map(|f| f.env)
        } else {
            None
        };
        let mut private_names = Vec::new();
        if let Some(mut e) = caller_env {
            loop {
                if let Some(d) = self.heap[e].decl() {
                    for n in &d.layout.names {
                        if let Some(p) = n.strip_prefix('#') {
                            private_names.push(Atom::new(p));
                        }
                    }
                }
                match self.heap[e].outer {
                    Some(o) => e = o,
                    None => break,
                }
            }
        }
        let opts = ParseOptions {
            strict: flags.strict,
            allow_new_target: flags.new_target,
            allow_super_property: flags.super_property,
            allow_super_call: flags.super_call,
            in_class_field_init: flags.class_field,
            private_names,
            ..self.parse_options()
        };
        let program = match crate::parse_script(&text, &opts) {
            Ok(p) => p,
            Err(e) => return self.syntax_error(&e.message),
        };
        let strict_eval = flags.strict || crate::compiler::scope::has_use_strict(&program.body);
        let code = compile_eval(&program, Rc::new(Source::new("<eval>", &text)), strict_eval);
        let realm = self.current_realm;
        let genv = self.global_env();
        let (outer_env, var_env) = if direct {
            let f = self.frames.last().expect("frame del llamador");
            (f.env, f.var_env.unwrap_or(genv))
        } else {
            (genv, genv)
        };
        let decls = code.decls.clone().unwrap_or_default();
        let hoisted = if strict_eval {
            HashSet::new()
        } else {
            self.eval_declaration_checks(&decls, outer_env, var_env)?
        };
        // El LexicalEnvironment propio del eval (si el compilador lo materializó).
        let top_layout = top_layout_of(&code);
        let env = match top_layout {
            Some(layout) => self.heap.alloc(Env::declarative(Some(outer_env), layout)),
            None => outer_env,
        };
        let mut frame = Frame::new(code, env, realm);
        frame.var_env = if strict_eval { None } else { Some(var_env) };
        frame.this = None;
        frame.hoisted = Some(Rc::new(hoisted));
        let (exit, _) = self.run_frame(frame)?;
        match exit {
            FrameExit::Return(v) => Ok(v),
            _ => Ok(Value::Undefined),
        }
    }

    /// EvalDeclarationInstantiation (§19.2.1.3) pasos 3–11 y B.3.2.3 (código no estricto).
    fn eval_declaration_checks(
        &mut self,
        d: &Declarations,
        outer_env: Gc<Env>,
        var_env: Gc<Env>,
    ) -> JsResult<HashSet<Atom>> {
        let var_global = self.heap[var_env].global().is_some();
        let names: Vec<Atom> = d
            .var_names
            .iter()
            .chain(d.function_names.iter())
            .cloned()
            .collect();
        if var_global {
            for n in &names {
                if self.global_has_lexical_declaration(var_env, n) {
                    return self
                        .syntax_error(&format!("Identifier '{n}' has already been declared"));
                }
            }
        }
        // Los environments entre el del eval y el varEnv (B.3.4: salvo los de un catch).
        let mut e = outer_env;
        while e != var_env {
            if let EnvKind::Declarative(decl) = &self.heap[e].kind {
                let is_catch = decl.layout.simple_catch;
                for n in &names {
                    if decl.find(n).is_some() && !is_catch {
                        return self
                            .syntax_error(&format!("Identifier '{n}' has already been declared"));
                    }
                }
            }
            match self.heap[e].outer {
                Some(o) => e = o,
                None => break,
            }
        }
        if var_global {
            for n in &d.function_names {
                if !self.global_can_declare_function(var_env, n)? {
                    return self.type_error(&format!("Cannot declare global function '{n}'"));
                }
            }
            for n in &d.var_names {
                if !self.global_can_declare_var(var_env, n)? {
                    return self.type_error(&format!("Cannot declare global variable '{n}'"));
                }
            }
        }
        // B.3.2.3
        let mut hoisted = HashSet::new();
        let mut declared = names.clone();
        for f in &d.annex_b {
            let mut binding_exists = false;
            let mut e = outer_env;
            while e != var_env {
                if let EnvKind::Declarative(decl) = &self.heap[e].kind
                    && decl.find(f).is_some()
                    && !decl.layout.simple_catch
                {
                    binding_exists = true;
                }
                match self.heap[e].outer {
                    Some(o) => e = o,
                    None => break,
                }
            }
            let definable = if !binding_exists && var_global {
                if self.global_has_lexical_declaration(var_env, f) {
                    false
                } else {
                    self.global_can_declare_var(var_env, f)?
                }
            } else {
                true
            };
            if !binding_exists && definable {
                if !declared.contains(f) {
                    if var_global {
                        self.global_create_var_binding(var_env, f, true)?;
                    } else {
                        let key = PropertyKey::from_atom(f);
                        if !self.env_has_binding(var_env, f, &key)?
                            && let Some(decl) = self.heap[var_env].decl_mut()
                        {
                            decl.extra.push((f.clone(), Some(Value::Undefined)));
                        }
                    }
                    declared.push(f.clone());
                }
                hoisted.insert(f.clone());
            }
        }
        Ok(hoisted)
    }

    /// CreateDynamicFunction (§20.2.1.1.1).
    pub fn create_dynamic_function(
        &mut self,
        constructor: Gc<Object>,
        new_target: Option<Gc<Object>>,
        kind: DynamicKind,
        args: &[Value],
    ) -> JsResult<Value> {
        let new_target = new_target.unwrap_or(constructor);
        let (params, body) = match args.split_last() {
            None => (&[][..], None),
            Some((b, ps)) => (ps, Some(b)),
        };
        let mut p = String::new();
        for (i, a) in params.iter().enumerate() {
            if i > 0 {
                p.push(',');
            }
            let s = self.to_string(a)?;
            p.push_str(&s.to_string_lossy());
        }
        let body_string = match body {
            Some(b) => format!("\n{}\n", self.to_string(b)?.to_string_lossy()),
            None => "\n\n".to_owned(),
        };
        let prefix = match kind {
            DynamicKind::Normal => "function",
            DynamicKind::Generator => "function*",
            DynamicKind::Async => "async function",
            DynamicKind::AsyncGenerator => "async function*",
        };
        if matches!(kind, DynamicKind::Async | DynamicKind::AsyncGenerator) {
            return self.internal_error("funciones async dinámicas (paso 4)");
        }
        let head = format!("{prefix} anonymous({p}\n) ");
        let source_string = format!("{head}{{{body_string}}}");
        let text = format!("({source_string})");
        let opts = self.parse_options();
        let program = match crate::parse_script(&text, &opts) {
            Ok(p) => p,
            Err(e) => return self.syntax_error(&e.message),
        };
        // Los parámetros y el cuerpo se parsean por separado en el spec: se verifica que el
        // árbol tenga exactamente la forma esperada (nada se "escapó" de su lugar).
        let body_start = (1 + head.encode_utf16().count()) as u32;
        let end = text.encode_utf16().count() as u32 - 1;
        let ok = program.body.len() == 1
            && match &program.body[0] {
                crate::ast::Stmt::Expression { expression, .. } => match expression {
                    crate::ast::Expr::Paren { expression, .. } => match &**expression {
                        crate::ast::Expr::Function(f) => {
                            f.body.span.start == body_start
                                && f.span.end == end
                                && f.span.start == 1
                        }
                        _ => false,
                    },
                    _ => false,
                },
                _ => false,
            };
        if !ok {
            return self.syntax_error("Invalid function parameters or body");
        }
        let Some(code) =
            compile_dynamic_function(&program, Rc::new(Source::new("<anonymous>", &text)))
        else {
            return self.syntax_error("Invalid function");
        };
        let fallback: fn(&crate::vm::realm::Intrinsics) -> Gc<Object> = match kind {
            DynamicKind::Generator => |i| i.generator_function_prototype,
            _ => |i| i.function_prototype,
        };
        let proto = self.get_prototype_from_constructor(new_target, fallback)?;
        let genv = self.global_env();
        let f = self.make_closure(
            code,
            genv,
            None,
            Some(Value::String(JsString::from("anonymous"))),
        );
        self.heap[f].proto = Some(proto);
        Ok(Value::Object(f))
    }
}

/// El layout del scope top-level de un eval si está materializado (lo guarda el compilador
/// en las constantes como el primer Layout marcado).
fn top_layout_of(code: &FunctionCode) -> Option<Rc<crate::vm::env::ScopeLayout>> {
    if code.kind != CodeKind::Eval {
        return None;
    }
    code.decls.as_ref().and_then(|d| d.top_layout.clone())
}
