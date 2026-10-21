//! Generación de código de las expresiones (§13), con el orden de evaluación de las
//! referencias del spec: en `a[k] = v` primero `a` y `k`, después `v`, y la conversión de la
//! clave y la escritura al final (PutValue); en las asignaciones compuestas la clave se
//! convierte una sola vez (GetValue la deja en la referencia).

use std::rc::Rc;

use crate::ast::*;
use crate::compiler::bytecode::*;
use crate::compiler::scope::{Tag, key};
use crate::compiler::{Codegen, FnBodyRef, NameSource, Target};
use crate::string::JsString;

/// Una referencia evaluada (sus operandos ya están en la pila).
#[derive(Debug, Clone)]
pub(crate) enum RefKind {
    Binding(Target),
    /// [obj]
    Prop(u32),
    /// [obj key]
    Elem,
    /// [name obj]
    Private,
    /// [this key base]
    Super,
}

impl RefKind {
    /// Cuántos operandos tiene en la pila.
    pub fn operands(&self) -> u8 {
        match self {
            RefKind::Binding(_) => 0,
            RefKind::Prop(_) => 1,
            RefKind::Elem | RefKind::Private => 2,
            RefKind::Super => 3,
        }
    }
}

pub(crate) fn strip_parens(e: &Expr) -> &Expr {
    match e {
        Expr::Paren { expression, .. } => strip_parens(expression),
        other => other,
    }
}

fn binary_op(op: &str) -> Option<Op> {
    Some(match op {
        "+" => Op::Add,
        "-" => Op::Sub,
        "*" => Op::Mul,
        "/" => Op::Div,
        "%" => Op::Mod,
        "**" => Op::Exp,
        "<<" => Op::Shl,
        ">>" => Op::Shr,
        ">>>" => Op::UShr,
        "&" => Op::BitAnd,
        "|" => Op::BitOr,
        "^" => Op::BitXor,
        "<" => Op::Lt,
        ">" => Op::Gt,
        "<=" => Op::Le,
        ">=" => Op::Ge,
        "==" => Op::Eq,
        "!=" => Op::Ne,
        "===" => Op::StrictEq,
        "!==" => Op::StrictNe,
        "in" => Op::In,
        "instanceof" => Op::InstanceOf,
        _ => return None,
    })
}

impl<'t> Codegen<'t> {
    pub(crate) fn expr(&mut self, e: &Expr) {
        self.expr_named(e, None);
    }

    /// Evalúa `e`; si es una definición de función anónima, con NamedEvaluation (§8.4.5).
    pub(crate) fn expr_named(&mut self, e: &Expr, name: Option<NameSource>) {
        self.pos(e.span().start);
        match e {
            Expr::Ident(i) => {
                let t = self.lookup(&i.name);
                self.emit_get(&t);
            }
            Expr::Literal(l) => self.literal(l),
            Expr::This(_) => self.emit_ctx(CtxWhat::This),
            Expr::Super(_) => {
                self.throw_error(ErrorKind::SyntaxError, "'super' keyword unexpected")
            }
            Expr::Array { elements, .. } => {
                self.emit(Op::NewArray);
                for el in elements {
                    match el {
                        None => self.emit(Op::ArrayHole),
                        Some(ExprOrSpread {
                            spread: Some(_),
                            expr,
                        }) => {
                            self.expr(expr);
                            self.emit(Op::ArraySpread);
                        }
                        Some(ExprOrSpread { spread: None, expr }) => {
                            self.expr(expr);
                            self.emit(Op::ArrayPush);
                        }
                    }
                }
            }
            Expr::Object { properties, .. } => self.object_literal(properties),
            Expr::Function(f) => self.function_expr(f, if f.id.is_none() { name } else { None }),
            Expr::Arrow(a) => self.arrow_expr(a, name),
            Expr::Class(c) => {
                let n = if c.id.is_none() { name } else { None };
                self.class(c, n);
            }
            Expr::Template(t) => self.template(t),
            Expr::TaggedTemplate { tag, quasi, .. } => self.tagged_template(tag, quasi),
            Expr::Unary {
                operator, argument, ..
            } => self.unary(operator, argument),
            Expr::Update {
                operator,
                prefix,
                argument,
                ..
            } => self.update(operator == &"++", *prefix, argument),
            Expr::Binary {
                operator,
                left,
                right,
                ..
            } => {
                if *operator == "in"
                    && let Expr::PrivateName(id) = &**left
                {
                    self.private_name(&id.name);
                    self.expr(right);
                    self.emit(Op::PrivateIn);
                    return;
                }
                self.expr(left);
                self.expr(right);
                self.pos(e.span().start);
                self.emit(binary_op(operator).expect("operador binario"));
            }
            Expr::Logical {
                operator,
                left,
                right,
                ..
            } => {
                self.expr(left);
                let end = self.new_label();
                let op = match *operator {
                    "&&" => Op::JumpIfFalseKeep(0),
                    "||" => Op::JumpIfTrueKeep(0),
                    _ => Op::JumpIfNotNullishKeep(0),
                };
                self.emit_to(op, end);
                self.expr(right);
                self.bind(end);
            }
            Expr::Assign {
                operator,
                left,
                right,
                ..
            } => self.assign(operator, left, right),
            Expr::Conditional {
                test,
                consequent,
                alternate,
                ..
            } => {
                self.expr(test);
                let else_l = self.new_label();
                let end = self.new_label();
                self.emit_to(Op::JumpIfFalse(0), else_l);
                self.expr(consequent);
                self.jump(end);
                self.bind(else_l);
                self.expr(alternate);
                self.bind(end);
            }
            Expr::Call {
                callee,
                arguments,
                optional,
                span,
            } => self.call(callee, arguments, *optional, *span),
            Expr::New {
                callee, arguments, ..
            } => {
                self.expr(callee);
                let spread = self.arguments(arguments);
                self.pos(e.span().start);
                let t = self.callee_text(callee);
                match spread {
                    None => self.emit(Op::NewSpread(t)),
                    Some(n) => self.emit(Op::New(n, t)),
                }
            }
            Expr::Sequence { expressions, .. } => {
                for (i, x) in expressions.iter().enumerate() {
                    self.expr(x);
                    if i + 1 < expressions.len() {
                        self.emit(Op::Pop);
                    }
                }
            }
            Expr::Member {
                object,
                property,
                optional,
                ..
            } => {
                self.member(object, property, *optional, false);
            }
            Expr::Chain { expression, .. } => {
                let short = self.new_label();
                self.st().chain_short.push(short);
                self.expr(expression);
                self.st().chain_short.pop();
                self.bind(short);
            }
            Expr::Paren { expression, .. } => self.expr_named(expression, name),
            Expr::Yield {
                argument, delegate, ..
            } => {
                match argument {
                    Some(a) => self.expr(a),
                    None => self.emit(Op::Undefined),
                }
                if *delegate {
                    if self.in_async_generator() {
                        self.async_yield_star();
                    } else {
                        self.yield_star();
                    }
                } else {
                    // En un generador async: AsyncGeneratorYield(? Await(value)) (§15.5.5).
                    if self.in_async_generator() {
                        self.emit(Op::Await);
                    }
                    self.emit(Op::Yield);
                }
            }
            Expr::Await { argument, .. } => {
                self.expr(argument);
                self.emit(Op::Await);
            }
            Expr::MetaProperty { meta, .. } => {
                if meta.name == "new" {
                    self.emit_ctx(CtxWhat::NewTarget);
                } else {
                    self.emit(Op::ImportMeta); // E1
                }
            }
            Expr::Import {
                source, options, ..
            } => {
                // E1: EvaluateImportCall (§13.3.10.2).
                self.expr(source);
                match options {
                    Some(o) => self.expr(o),
                    None => self.emit(Op::Undefined),
                }
                self.emit(Op::ImportCall);
            }
            Expr::PrivateName(_) => {
                self.throw_error(ErrorKind::SyntaxError, "Unexpected private name");
            }
        }
    }

    fn literal(&mut self, l: &Literal) {
        match &l.value {
            LitValue::Null => self.emit(Op::Null),
            LitValue::Bool(true) => self.emit(Op::True),
            LitValue::Bool(false) => self.emit(Op::False),
            LitValue::Number(n) => self.number(*n),
            LitValue::String(s) => {
                let i = self.string(s.clone());
                self.emit(Op::Const(i));
            }
            LitValue::BigInt(raw) => {
                let i = self.add_const(Constant::BigInt(raw.clone()));
                self.emit(Op::BigInt(i));
            }
            LitValue::Regex { pattern, flags } => {
                let i = self.add_const(Constant::RegExp {
                    pattern: JsString::from(pattern.as_str()),
                    flags: JsString::from(flags.as_str()),
                });
                self.emit(Op::RegExp(i));
            }
        }
    }

    /// Empuja el Private Name `#name` (resuelto en las clases que lo encierran).
    pub(crate) fn private_name(&mut self, name: &str) {
        let full = format!("#{name}");
        let t = self.lookup_private(&full);
        self.emit_get(&t);
    }

    fn lookup_private(&self, full: &str) -> Target {
        let from = self.scope_id();
        let hidden = &self.sti().private_hidden;
        let mut s = Some(from);
        while let Some(id) = s {
            let scope = &self.tree.scopes[id];
            if !hidden.contains(&id)
                && let Some(i) = scope.find(full)
            {
                let b = &scope.bindings[i];
                if let crate::compiler::scope::Loc::Env(slot) = b.loc {
                    return Target::Env {
                        hops: self.tree.hops(from, id),
                        slot,
                        kind: b.kind,
                        name: b.name.clone(),
                        check: false,
                    };
                }
            }
            s = scope.parent;
        }
        // Código de eval: búsqueda dinámica.
        Target::Dynamic(crate::string::Atom::new(full))
    }

    // --- Funciones ---

    fn closure_name(&mut self, name: &Option<NameSource>) -> (JsString, FnName) {
        match name {
            None => (JsString::default(), FnName::Code),
            Some(NameSource::Const(s)) => (s.clone(), FnName::Code),
            Some(NameSource::Stack(p)) => (JsString::default(), FnName::Stack(*p)),
            Some(NameSource::FieldName) => {
                self.emit(Op::FieldName);
                (JsString::default(), FnName::Stack(NamePrefix::None))
            }
        }
    }

    fn function_expr(&mut self, f: &Function, name: Option<NameSource>) {
        let field = matches!(name, Some(NameSource::FieldName));
        let (static_name, fname) = self.closure_name(&name);
        let static_name = match &f.id {
            Some(id) => JsString::from(id.name.as_str()),
            None => static_name,
        };
        let fid = self.tree.func(key(f.span, Tag::Function));
        let code = self.compile_function_body(
            fid,
            crate::compiler::bytecode::CodeKind::Normal,
            &f.params,
            FnBodyRef::Block(&f.body.body),
            static_name,
            f.span,
        );
        let idx = self.add_const(Constant::Function(code));
        // El funcEnv con el nombre (§15.2.5 InstantiateOrdinaryFunctionExpression).
        let name_scope = self.tree.funcs[fid].name_scope;
        let materialized = name_scope.is_some_and(|s| self.tree.scopes[s].materialized);
        if let (Some(ns), true) = (name_scope, materialized) {
            let layout = self.tree.scopes[ns].layout.clone();
            let c = self.add_const(Constant::Layout(layout));
            self.emit(Op::PushScope(c));
            self.emit(Op::Closure {
                code: idx,
                name: fname,
                home: 0,
            });
            self.emit(Op::Dup);
            self.emit(Op::InitEnv(0, 0));
            self.emit(Op::PopScope);
        } else {
            self.emit(Op::Closure {
                code: idx,
                name: fname,
                home: 0,
            });
        }
        if field {
            self.emit(Op::Swap);
            self.emit(Op::Pop);
        }
    }

    fn arrow_expr(&mut self, a: &Arrow, name: Option<NameSource>) {
        let field = matches!(name, Some(NameSource::FieldName));
        let (static_name, fname) = self.closure_name(&name);
        let fid = self.tree.func(key(a.span, Tag::Function));
        let body = match &a.body {
            ArrowBody::Expr(e) => FnBodyRef::Expr(e),
            ArrowBody::Block(b) => FnBodyRef::Block(&b.body),
        };
        let code = self.compile_function_body(
            fid,
            crate::compiler::bytecode::CodeKind::Arrow,
            &a.params,
            body,
            static_name,
            a.span,
        );
        let idx = self.add_const(Constant::Function(code));
        self.emit(Op::Closure {
            code: idx,
            name: fname,
            home: 0,
        });
        if field {
            self.emit(Op::Swap);
            self.emit(Op::Pop);
        }
    }

    // --- Templates ---

    fn template(&mut self, t: &Template) {
        let first = t.quasis[0].cooked.clone().unwrap_or_default();
        let i = self.string(first);
        self.emit(Op::Const(i));
        for (k, e) in t.expressions.iter().enumerate() {
            self.expr(e);
            self.emit(Op::ToStringOp);
            self.emit(Op::Add);
            let s = t.quasis[k + 1].cooked.clone().unwrap_or_default();
            if !s.is_empty() {
                let i = self.string(s);
                self.emit(Op::Const(i));
                self.emit(Op::Add);
            }
        }
    }

    fn template_site(&mut self, t: &Template) -> u32 {
        let site = TemplateSite {
            id: next_template_site(),
            cooked: t.quasis.iter().map(|q| q.cooked.clone()).collect(),
            raw: t
                .quasis
                .iter()
                .map(|q| JsString::from(q.raw.as_str()))
                .collect(),
        };
        self.add_const(Constant::Template(Rc::new(site)))
    }

    fn tagged_template(&mut self, tag: &Expr, quasi: &Template) {
        self.callee(tag);
        let site = self.template_site(quasi);
        self.emit(Op::TemplateObject(site));
        for e in &quasi.expressions {
            self.expr(e);
        }
        self.pos(tag.span().start);
        let t = self.callee_text(tag);
        self.emit(Op::Call(quasi.expressions.len() as u32 + 1, t));
    }

    // --- Operadores ---

    fn unary(&mut self, op: &str, arg: &Expr) {
        match op {
            "-" => {
                self.expr(arg);
                self.emit(Op::Neg);
            }
            "+" => {
                self.expr(arg);
                self.emit(Op::Plus);
            }
            "!" => {
                self.expr(arg);
                self.emit(Op::Not);
            }
            "~" => {
                self.expr(arg);
                self.emit(Op::BitNot);
            }
            "void" => {
                self.expr(arg);
                self.emit(Op::Pop);
                self.emit(Op::Undefined);
            }
            "typeof" => {
                if let Expr::Ident(i) = strip_parens(arg) {
                    match self.lookup(&i.name) {
                        Target::Dynamic(n) => {
                            let n = self.name(&n);
                            self.emit(Op::TypeofName(n));
                            return;
                        }
                        Target::Global(n) => {
                            let n = self.name(&n);
                            self.emit(Op::TypeofGlobal(n));
                            return;
                        }
                        t => self.emit_get(&t),
                    }
                } else {
                    self.expr(arg);
                }
                self.emit(Op::Typeof);
            }
            "delete" => self.delete(arg),
            _ => unreachable!("operador unario {op}"),
        }
    }

    fn delete(&mut self, arg: &Expr) {
        let strict = self.strict();
        match arg {
            Expr::Paren { expression, .. } => self.delete(expression),
            Expr::Ident(i) => match self.lookup(&i.name) {
                Target::Dynamic(n) | Target::Global(n) => {
                    let n = self.name(&n);
                    self.emit(Op::DeleteName(n));
                }
                // Los bindings declarativos no se pueden borrar.
                _ => self.emit(Op::False),
            },
            Expr::Member {
                object, property, ..
            } => {
                if let Expr::Super(_) = &**object {
                    // §13.5.1.2: la referencia se evalúa y después ReferenceError.
                    self.emit_ctx(CtxWhat::This);
                    self.emit(Op::Pop);
                    if let MemberProp::Computed(k) = property {
                        self.expr(k);
                        self.emit(Op::Pop);
                    }
                    self.throw_error(
                        ErrorKind::ReferenceError,
                        "Unsupported reference to 'super'",
                    );
                    self.emit(Op::Undefined);
                    return;
                }
                self.expr(object);
                match property {
                    MemberProp::Ident(id) => {
                        let n = self.name(&id.name);
                        self.emit(Op::DeleteProp(n, strict));
                    }
                    MemberProp::Computed(k) => {
                        self.expr(k);
                        self.emit(Op::DeleteElem(strict));
                    }
                    MemberProp::Private(_) => unreachable!("delete de privado (early error)"),
                }
            }
            Expr::Chain { expression, .. } => {
                let short = self.new_label();
                let end = self.new_label();
                self.st().chain_short.push(short);
                match &**expression {
                    Expr::Member {
                        object,
                        property,
                        optional,
                        ..
                    } => {
                        self.expr(object);
                        if *optional {
                            self.optional_check(0);
                        }
                        match property {
                            MemberProp::Ident(id) => {
                                let n = self.name(&id.name);
                                self.emit(Op::DeleteProp(n, strict));
                            }
                            MemberProp::Computed(k) => {
                                self.expr(k);
                                self.emit(Op::DeleteElem(strict));
                            }
                            MemberProp::Private(_) => unreachable!("delete de privado"),
                        }
                    }
                    other => {
                        self.expr(other);
                        self.emit(Op::Pop);
                        self.emit(Op::True);
                    }
                }
                self.st().chain_short.pop();
                self.jump(end);
                self.bind(short);
                self.emit(Op::Pop);
                self.emit(Op::True);
                self.bind(end);
            }
            other => {
                self.expr(other);
                self.emit(Op::Pop);
                self.emit(Op::True);
            }
        }
    }

    /// Cortocircuito de `?.`: el valor a profundidad `extra` es la base.
    pub(crate) fn optional_check(&mut self, extra: u8) {
        let short = *self.sti().chain_short.last().expect("cadena opcional");
        self.emit_to(Op::JumpIfNullishChain { target: 0, extra }, short);
    }

    // --- Referencias ---

    /// Evalúa los operandos de una referencia (destino de asignación).
    pub(crate) fn emit_ref(&mut self, target: &Expr) -> RefKind {
        match target {
            Expr::Paren { expression, .. } => self.emit_ref(expression),
            Expr::Ident(i) => {
                let t = self.lookup(&i.name);
                self.emit_prepare_set(&t);
                RefKind::Binding(t)
            }
            Expr::Member {
                object, property, ..
            } => {
                if let Expr::Super(_) = &**object {
                    self.emit_ctx(CtxWhat::This);
                    match property {
                        MemberProp::Ident(id) => self.push_str(&id.name),
                        MemberProp::Computed(k) => self.expr(k),
                        MemberProp::Private(_) => unreachable!("super.#x (early error)"),
                    }
                    self.emit_ctx(CtxWhat::Function);
                    self.emit(Op::HomeObject);
                    self.emit(Op::SuperBase);
                    return RefKind::Super;
                }
                self.expr(object);
                match property {
                    MemberProp::Ident(id) => RefKind::Prop(self.name(&id.name)),
                    MemberProp::Computed(k) => {
                        // ToObject y ToPropertyKey quedan para GetValue/PutValue.
                        self.expr(k);
                        RefKind::Elem
                    }
                    MemberProp::Private(id) => {
                        self.private_name(&id.name);
                        self.emit(Op::Swap);
                        RefKind::Private
                    }
                }
            }
            Expr::Call { .. } => {
                // B.3.6 (web-compat): `f() = 1` evalúa la llamada y lanza ReferenceError.
                self.expr(target);
                self.emit(Op::Pop);
                self.throw_error(
                    ErrorKind::ReferenceError,
                    "Invalid left-hand side in assignment",
                );
                RefKind::Binding(Target::Callee)
            }
            _ => {
                self.throw_error(ErrorKind::ReferenceError, "Invalid assignment target");
                RefKind::Binding(Target::Callee)
            }
        }
    }

    /// GetValue de la referencia (los operandos quedan): [ops] → [ops valor].
    pub(crate) fn emit_ref_get(&mut self, r: &RefKind) {
        match r {
            RefKind::Binding(t) => match t {
                Target::Dynamic(n) | Target::Global(n) => {
                    let n = self.name(n);
                    self.emit(Op::GetRefValue(n));
                }
                t => self.emit_get(t),
            },
            RefKind::Prop(n) => {
                self.emit(Op::Dup);
                self.emit(Op::GetProp(*n));
            }
            RefKind::Elem => {
                self.emit(Op::PrepareElem);
                self.emit(Op::Dup2);
                self.emit(Op::GetElem);
            }
            RefKind::Private => {
                self.emit(Op::Dup2);
                self.emit(Op::PrivateGet);
            }
            RefKind::Super => {
                // La clave se convierte una sola vez (queda en la referencia).
                self.emit(Op::ToPropertyKeyAt(1));
                self.emit(Op::Dup3);
                self.emit(Op::GetSuperElem);
            }
        }
    }

    /// PutValue: [ops valor] → [valor].
    pub(crate) fn emit_ref_put(&mut self, r: &RefKind) {
        let strict = self.strict();
        match r {
            RefKind::Binding(t) => self.emit_set(t),
            RefKind::Prop(n) => self.emit(Op::SetProp(*n, strict)),
            RefKind::Elem => self.emit(Op::SetElem(strict)),
            RefKind::Private => self.emit(Op::PrivateSet),
            RefKind::Super => self.emit(Op::SetSuperElem(strict)),
        }
    }

    /// Descarta los operandos de una referencia que quedaron debajo del valor.
    fn drop_ref_operands(&mut self, r: &RefKind) {
        let n = r.operands();
        if n > 0 {
            self.emit(Op::Bury(n));
            for _ in 0..n {
                self.emit(Op::Pop);
            }
        }
        if let RefKind::Binding(Target::Dynamic(_) | Target::Global(_)) = r {
            self.emit(Op::PopRef);
        }
    }

    fn assign(&mut self, op: &str, left: &Pat, right: &Expr) {
        // Destructuring.
        if matches!(left, Pat::Object { .. } | Pat::Array { .. }) {
            self.expr(right);
            self.emit(Op::Dup);
            self.pattern_binding(left, crate::compiler::BindMode::Assign);
            return;
        }
        let target = match left {
            Pat::Ident(i) => Expr::Ident(i.clone()),
            Pat::Expr(e) => (**e).clone(),
            _ => unreachable!("destino de asignación"),
        };
        // IsIdentifierRef: un identificador sin paréntesis.
        let is_ident = matches!(&target, Expr::Ident(_));
        let name = match &target {
            Expr::Ident(i) => Some(NameSource::Const(i.name.to_js_string())),
            _ => None,
        };
        let r = self.emit_ref(&target);
        match op {
            "=" => {
                if is_ident {
                    self.expr_named(right, name);
                } else {
                    self.expr(right);
                }
                self.emit_ref_put(&r);
            }
            "&&=" | "||=" | "??=" => {
                self.emit_ref_get(&r);
                let short = self.new_label();
                let end = self.new_label();
                let jump = match op {
                    "&&=" => Op::JumpIfFalseKeep(0),
                    "||=" => Op::JumpIfTrueKeep(0),
                    _ => Op::JumpIfNotNullishKeep(0),
                };
                self.emit_to(jump, short);
                if is_ident {
                    self.expr_named(right, name);
                } else {
                    self.expr(right);
                }
                self.emit_ref_put(&r);
                self.jump(end);
                self.bind(short);
                self.drop_ref_operands(&r);
                self.bind(end);
            }
            _ => {
                self.emit_ref_get(&r);
                self.expr(right);
                let bin = &op[..op.len() - 1];
                self.emit(binary_op(bin).expect("operador compuesto"));
                self.emit_ref_put(&r);
            }
        }
    }

    fn update(&mut self, inc: bool, prefix: bool, arg: &Expr) {
        let r = self.emit_ref(arg);
        self.emit_ref_get(&r);
        self.emit(Op::ToNumeric);
        if !prefix {
            self.emit(Op::Dup);
            let n = r.operands();
            if n > 0 {
                self.emit(Op::Bury(n + 1));
            }
        }
        self.emit(if inc { Op::Inc } else { Op::Dec });
        self.emit_ref_put(&r);
        if !prefix {
            self.emit(Op::Pop);
        }
    }

    // --- Miembros y llamadas ---

    /// `obj.prop`: con `keep`, deja [obj valor] (para llamadas con `this`).
    fn member(&mut self, object: &Expr, property: &MemberProp, optional: bool, keep: bool) {
        if let Expr::Super(_) = object {
            // [this] [key] [home → base]
            self.emit_ctx(CtxWhat::This);
            if keep {
                self.emit(Op::Dup);
            }
            match property {
                MemberProp::Ident(id) => self.push_str(&id.name),
                MemberProp::Computed(k) => self.expr(k),
                MemberProp::Private(_) => unreachable!("super.#x"),
            }
            self.emit_ctx(CtxWhat::Function);
            self.emit(Op::HomeObject);
            self.emit(Op::SuperBase);
            self.emit(Op::GetSuperElem);
            return;
        }
        self.expr(object);
        if optional {
            self.optional_check(0);
        }
        match property {
            MemberProp::Ident(id) => {
                let n = self.name(&id.name);
                self.emit(if keep {
                    Op::GetPropKeep(n)
                } else {
                    Op::GetProp(n)
                });
            }
            MemberProp::Computed(k) => {
                self.expr(k);
                self.emit(if keep { Op::GetElemKeep } else { Op::GetElem });
            }
            MemberProp::Private(id) => {
                if keep {
                    self.emit(Op::Dup);
                }
                self.private_name(&id.name);
                self.emit(Op::Swap);
                self.emit(Op::PrivateGet);
            }
        }
    }

    /// Evalúa el callee dejando [f this].
    fn callee(&mut self, callee: &Expr) {
        match callee {
            Expr::Paren { expression, .. }
                if matches!(
                    strip_parens(expression),
                    Expr::Member { .. } | Expr::Ident(_) | Expr::Chain { .. }
                ) =>
            {
                self.callee(strip_parens(expression))
            }
            Expr::Member {
                object,
                property,
                optional,
                ..
            } => {
                self.member(object, property, *optional, true);
                self.emit(Op::Swap);
            }
            Expr::Ident(i) => match self.lookup(&i.name) {
                Target::Dynamic(n) => {
                    let n = self.name(&n);
                    self.emit(Op::GetNameForCall(n));
                }
                Target::Global(n) => {
                    let n = self.name(&n);
                    self.emit(Op::GetGlobalForCall(n));
                }
                t => {
                    self.emit_get(&t);
                    self.emit(Op::Undefined);
                }
            },
            Expr::Chain { expression, .. } => {
                // `(a?.b)()`: la referencia conserva `this`.
                let short = self.new_label();
                let end = self.new_label();
                self.st().chain_short.push(short);
                self.callee(expression);
                self.st().chain_short.pop();
                self.jump(end);
                self.bind(short);
                self.emit(Op::Undefined);
                self.bind(end);
            }
            other => {
                self.expr(other);
                self.emit(Op::Undefined);
            }
        }
    }

    /// Los argumentos: `Some(n)` si son simples (n en la pila), `None` si hay spread (un
    /// array en la pila).
    fn arguments(&mut self, args: &[ExprOrSpread]) -> Option<u32> {
        if args.iter().any(|a| a.spread.is_some()) {
            self.emit(Op::NewArray);
            for a in args {
                self.expr(&a.expr);
                self.emit(if a.spread.is_some() {
                    Op::ArraySpread
                } else {
                    Op::ArrayPush
                });
            }
            None
        } else {
            for a in args {
                self.expr(&a.expr);
            }
            Some(args.len() as u32)
        }
    }

    fn call(&mut self, callee: &Expr, args: &[ExprOrSpread], optional: bool, span: Span) {
        if let Expr::Super(_) = callee {
            self.emit_ctx(CtxWhat::NewTarget);
            self.emit_ctx(CtxWhat::Function);
            self.emit(Op::SuperConstructor);
            let spread = self.arguments(args);
            self.pos(span.start);
            match spread {
                Some(n) => self.emit(Op::SuperConstruct(n)),
                None => self.emit(Op::SuperConstructSpread),
            }
            let loc = self.ctx_loc();
            self.emit(Op::SuperCallFinish(loc));
            return;
        }
        // `eval?.(x)` no es un eval directo (§13.3.6.1).
        let direct_eval = !optional && matches!(callee, Expr::Ident(i) if i.name == "eval");
        self.callee(callee);
        if optional {
            self.optional_check(1);
        }
        let spread = self.arguments(args);
        self.pos(span.start);
        if direct_eval {
            let flags = self.eval_flags().to_bits();
            match spread {
                Some(n) => self.emit(Op::CallEval(n, flags)),
                None => self.emit(Op::CallEvalSpread(flags)),
            }
            return;
        }
        let t = self.callee_text(callee);
        match spread {
            Some(n) => self.emit(Op::Call(n, t)),
            None => self.emit(Op::CallSpread(t)),
        }
    }

    /// El texto fuente del callee (para "x is not a function").
    fn callee_text(&mut self, callee: &Expr) -> u32 {
        let sp = callee.span();
        let text = self.source.slice(sp);
        if text.len() > 60 {
            return u32::MAX;
        }
        self.string(text)
    }

    // --- Literales de objeto ---

    /// La clave estática de una propiedad (identificador, string o número).
    pub(crate) fn static_key(k: &PropKey) -> Option<JsString> {
        match k {
            PropKey::Ident(i) => Some(i.name.to_js_string()),
            PropKey::Literal(l) => match &l.value {
                LitValue::String(s) => Some(s.clone()),
                LitValue::Number(n) => Some(crate::abstract_ops::number::number_to_string(*n)),
                LitValue::BigInt(raw) => Some(JsString::from(
                    crate::bigint::BigInt::from_literal(raw)
                        .map(|b| b.to_string_radix(10))
                        .unwrap_or_default()
                        .as_str(),
                )),
                _ => None,
            },
            _ => None,
        }
    }

    fn object_literal(&mut self, props: &[ObjectMember]) {
        self.emit(Op::NewObject);
        for p in props {
            match p {
                ObjectMember::Spread { argument, .. } => {
                    self.expr(argument);
                    self.emit(Op::CopyDataProperties);
                }
                ObjectMember::Property {
                    span,
                    key: k,
                    value,
                    kind,
                    method,
                    shorthand,
                } => {
                    // El texto de un método es su MethodDefinition entera (§20.2.3.5).
                    if matches!(value, PropValue::Method(_)) {
                        self.method_src = Some(*span);
                    }
                    let skey = Self::static_key(k);
                    // `__proto__: v` (B.3.1): no calculada, no shorthand, no método.
                    if let (Some(s), PropKind::Init, false, false, PropValue::Expr(v)) =
                        (&skey, kind, method, shorthand, value)
                        && !matches!(k, PropKey::Computed(_))
                        && s.to_string_lossy() == "__proto__"
                    {
                        self.expr(v);
                        self.emit(Op::InitProto);
                        continue;
                    }
                    match &skey {
                        Some(s) => {
                            let n = self.name(&s.to_string_lossy());
                            // Las claves con surrogates sueltos usan la forma calculada.
                            if s.to_str().is_none() {
                                let i = self.string(s.clone());
                                self.emit(Op::Const(i));
                                self.object_value(value, *kind, None);
                                continue;
                            }
                            match kind {
                                PropKind::Init => {
                                    let name = Some(NameSource::Const(s.clone()));
                                    match value {
                                        PropValue::Expr(v) => self.expr_named(v, name),
                                        PropValue::Method(f) => {
                                            self.method_closure(f, name, 1);
                                        }
                                        PropValue::CoverInit(..) => unreachable!("CoverInit"),
                                    }
                                    self.emit(Op::InitProp(n));
                                }
                                PropKind::Get | PropKind::Set => {
                                    let prefix = if *kind == PropKind::Get {
                                        "get "
                                    } else {
                                        "set "
                                    };
                                    let mut full = JsString::from(prefix).units().to_vec();
                                    full.extend_from_slice(s.units());
                                    let PropValue::Method(f) = value else {
                                        unreachable!("accessor sin función")
                                    };
                                    self.method_closure(
                                        f,
                                        Some(NameSource::Const(JsString::from_units(full))),
                                        1,
                                    );
                                    self.emit(Op::InitAccessor {
                                        name: n,
                                        setter: *kind == PropKind::Set,
                                        computed: false,
                                    });
                                }
                            }
                        }
                        None => {
                            let PropKey::Computed(ke) = k else {
                                unreachable!("clave no calculada")
                            };
                            self.expr(ke);
                            self.emit(Op::ToPropertyKey);
                            self.object_value(value, *kind, None);
                        }
                    }
                }
            }
        }
    }

    /// [obj key] → obj: el valor de una propiedad con clave en la pila.
    fn object_value(&mut self, value: &PropValue, kind: PropKind, _n: Option<u32>) {
        match kind {
            PropKind::Init => {
                match value {
                    PropValue::Expr(v) => {
                        self.expr_named(v, Some(NameSource::Stack(NamePrefix::None)))
                    }
                    PropValue::Method(f) => {
                        self.method_closure(f, Some(NameSource::Stack(NamePrefix::None)), 2)
                    }
                    PropValue::CoverInit(..) => unreachable!("CoverInit"),
                }
                self.emit(Op::InitElem);
            }
            PropKind::Get | PropKind::Set => {
                let PropValue::Method(f) = value else {
                    unreachable!("accessor sin función")
                };
                let prefix = if kind == PropKind::Get {
                    NamePrefix::Get
                } else {
                    NamePrefix::Set
                };
                self.method_closure(f, Some(NameSource::Stack(prefix)), 2);
                self.emit(Op::InitAccessor {
                    name: 0,
                    setter: kind == PropKind::Set,
                    computed: true,
                });
            }
        }
    }

    /// Un método con HomeObject a profundidad `home - 1` de la pila.
    pub(crate) fn method_closure(&mut self, f: &Function, name: Option<NameSource>, home: u8) {
        let (static_name, fname) = self.closure_name(&name);
        let fid = self.tree.func(key(f.span, Tag::Function));
        let src = self.method_src.take().unwrap_or(f.span);
        let code = self.compile_function_body(
            fid,
            crate::compiler::bytecode::CodeKind::Method,
            &f.params,
            FnBodyRef::Block(&f.body.body),
            static_name,
            src,
        );
        let idx = self.add_const(Constant::Function(code));
        self.emit(Op::Closure {
            code: idx,
            name: fname,
            home,
        });
    }

    // --- Generadores ---

    fn yield_star(&mut self) {
        let slot = self.alloc_temp();
        self.alloc_temp();
        self.alloc_temp();
        let ks = self.alloc_temp();
        let vs = self.alloc_temp();
        self.emit(Op::GetIterator(slot));
        self.emit(Op::Int(COMPLETION_NORMAL as i32));
        self.emit(Op::InitLocal(ks));
        self.emit(Op::Undefined);
        self.emit(Op::InitLocal(vs));
        let top = self.new_label();
        let done = self.new_label();
        self.bind(top);
        self.emit_to(
            Op::YieldStarStep {
                slot,
                kind_slot: ks,
                value_slot: vs,
                done: 0,
            },
            done,
        );
        self.emit(Op::YieldRaw {
            kind_slot: ks,
            value_slot: vs,
        });
        self.jump(top);
        self.bind(done);
        self.free_temp(slot);
    }

    /// `yield*` en un generador async (§15.5.5, generatorKind async): cada llamada al
    /// iterador interno se espera con `Await` y los valores se ceden con
    /// AsyncGeneratorYield sin otro `Await` (`YieldRaw`). (E3)
    fn async_yield_star(&mut self) {
        let slot = self.alloc_temp();
        self.alloc_temp();
        self.alloc_temp();
        let ks = self.alloc_temp();
        let vs = self.alloc_temp();
        self.emit(Op::GetAsyncIterator(slot));
        self.emit(Op::Int(COMPLETION_NORMAL as i32));
        self.emit(Op::InitLocal(ks));
        self.emit(Op::Undefined);
        self.emit(Op::InitLocal(vs));
        let top = self.new_label();
        let done = self.new_label();
        self.bind(top);
        self.emit(Op::AsyncYieldStarCall {
            slot,
            kind_slot: ks,
            value_slot: vs,
        });
        self.emit(Op::Await);
        self.emit_to(Op::AsyncYieldStarResult { slot, done: 0 }, done);
        self.emit(Op::YieldRaw {
            kind_slot: ks,
            value_slot: vs,
        });
        self.jump(top);
        self.bind(done);
        self.free_temp(slot);
    }
}
