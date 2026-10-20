//! Generación de código de las sentencias (§14).
//!
//! `break`/`continue` se resuelven en compilación con la pila de [`Control`]: al salir de cada
//! estructura se emite su limpieza (cerrar environments, quitar handlers, cerrar iteradores);
//! si en el camino hay un `finally`, el salto se convierte en una completion con una clase
//! propia que el `finally` despacha al terminar (§14.15.3). `return` y las excepciones se
//! desenrollan en tiempo de ejecución por la pila de handlers del frame.
//!
//! En el código de script y de eval se mantiene el valor de completion (UpdateEmpty): las
//! sentencias que producen `undefined` cuando su cuerpo es vacío lo reinician al empezar.

use crate::ast::*;
use crate::compiler::bytecode::*;
use crate::compiler::scope::{Tag, key};
use crate::compiler::{BindMode, Codegen, Control, JumpExit, Label, NameSource, labelled_fn};
use crate::string::JsString;

/// A qué apunta un `break`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BreakKind {
    Loop,
    Switch,
    Label,
}

impl<'t> Codegen<'t> {
    pub(crate) fn statements(&mut self, body: &[Stmt]) {
        for s in body {
            self.stmt(s);
        }
    }

    fn completion_reset(&mut self) {
        if let Some(cv) = self.sti().completion {
            self.emit(Op::Undefined);
            self.emit(Op::InitLocal(cv));
        }
    }

    /// Un bloque con su scope (si declara algo).
    pub(crate) fn block(&mut self, body: &[Stmt], span: Span) {
        let parent = self.scope_id();
        if let Some(id) = self.tree.scope(key(span, Tag::Block)) {
            self.enter_scope(id);
            self.instantiate_block_functions(body);
            self.statements(body);
            self.exit_scope(id, parent);
        } else {
            self.statements(body);
        }
    }

    /// Una sentencia en posición de cuerpo: una declaración de función ahí (B.3.3) es como
    /// si estuviera en un bloque propio.
    fn sub_stmt(&mut self, s: &Stmt) {
        if let Stmt::Function(f) = s {
            self.block(std::slice::from_ref(s), f.span);
        } else {
            self.stmt(s);
        }
    }

    fn push_breakable(&mut self, kind: BreakKind, continue_keep: usize) -> (Label, Label) {
        let labels = std::mem::take(&mut self.st().pending_labels);
        let b = self.new_label();
        let c = self.new_label();
        self.st().controls.push(Control::Breakable {
            labels,
            is_loop: kind == BreakKind::Loop,
            break_label: b,
            continue_label: if kind == BreakKind::Loop {
                Some(c)
            } else {
                None
            },
            continue_keep,
        });
        // El tipo `Switch` se distingue por no tener etiqueta de continue y no ser `Label`.
        if kind == BreakKind::Switch {
            let i = self.sti().controls.len() - 1;
            self.st().switch_marks.push(i);
        }
        (b, c)
    }

    fn pop_breakable(&mut self, kind: BreakKind) {
        let c = self.st().controls.pop();
        debug_assert!(matches!(c, Some(Control::Breakable { .. })));
        if kind == BreakKind::Switch {
            self.st().switch_marks.pop();
        }
    }

    pub(crate) fn stmt(&mut self, s: &Stmt) {
        self.pos(s.span().start);
        match s {
            Stmt::Expression { expression, .. } => {
                self.expr(expression);
                match self.sti().completion {
                    Some(cv) => self.emit(Op::InitLocal(cv)),
                    None => self.emit(Op::Pop),
                }
            }
            Stmt::Block(b) => self.block(&b.body, b.span),
            Stmt::Empty(_) => {}
            Stmt::Debugger(_) => self.emit(Op::Debugger),
            Stmt::With { span, object, body } => {
                self.completion_reset();
                self.expr(object);
                let parent = self.scope_id();
                let id = self
                    .tree
                    .scope(key(*span, Tag::With))
                    .expect("scope de with");
                self.emit(Op::PushWith);
                self.st().controls.push(Control::Scope);
                self.st().scope = id;
                self.stmt(body);
                self.exit_scope(id, parent);
            }
            Stmt::Return { argument, .. } => {
                match argument {
                    Some(e) => {
                        self.expr(e);
                        // §14.10.1: en un generador async, `? Await(exprValue)`. (E3)
                        if self.in_async_generator() {
                            self.emit(Op::Await);
                        }
                    }
                    None => self.emit(Op::Undefined),
                }
                self.emit(Op::Return);
            }
            Stmt::Labeled { .. } => self.labeled(s),
            Stmt::Break { label, .. } => self.break_continue(label.as_ref(), false),
            Stmt::Continue { label, .. } => self.break_continue(label.as_ref(), true),
            Stmt::If {
                test,
                consequent,
                alternate,
                ..
            } => {
                self.completion_reset();
                self.expr(test);
                let else_l = self.new_label();
                let end = self.new_label();
                self.emit_to(Op::JumpIfFalse(0), else_l);
                self.sub_stmt(consequent);
                if let Some(a) = alternate {
                    self.jump(end);
                    self.bind(else_l);
                    self.sub_stmt(a);
                } else {
                    self.bind(else_l);
                }
                self.bind(end);
            }
            Stmt::Switch {
                span,
                discriminant,
                cases,
            } => self.switch(*span, discriminant, cases),
            Stmt::Throw { argument, .. } => {
                self.expr(argument);
                self.emit(Op::Throw);
            }
            Stmt::Try {
                block,
                handler,
                finalizer,
                ..
            } => self.try_stmt(block, handler.as_ref(), finalizer.as_ref()),
            Stmt::While { test, body, .. } => {
                self.completion_reset();
                let (brk, cont) = self.push_breakable(BreakKind::Loop, 0);
                self.bind(cont);
                self.expr(test);
                self.emit_to(Op::JumpIfFalse(0), brk);
                self.stmt(body);
                self.jump(cont);
                self.bind(brk);
                self.pop_breakable(BreakKind::Loop);
            }
            Stmt::DoWhile { body, test, .. } => {
                self.completion_reset();
                let (brk, cont) = self.push_breakable(BreakKind::Loop, 0);
                let top = self.new_label();
                self.bind(top);
                self.stmt(body);
                self.bind(cont);
                self.expr(test);
                self.emit_to(Op::JumpIfTrue(0), top);
                self.bind(brk);
                self.pop_breakable(BreakKind::Loop);
            }
            Stmt::For {
                span,
                init,
                test,
                update,
                body,
            } => self.for_stmt(*span, init.as_ref(), test.as_ref(), update.as_ref(), body),
            Stmt::ForIn {
                span,
                left,
                right,
                body,
            } => self.for_in_of(*span, left, right, body, false, false),
            Stmt::ForOf {
                span,
                left,
                right,
                body,
                is_await,
            } => self.for_in_of(*span, left, right, body, true, *is_await),
            Stmt::Function(f) => self.function_statement(f),
            Stmt::Variable(d) => self.var_declaration(d),
            Stmt::Class(c) => {
                self.class(c, None);
                let name = c.id.as_ref().expect("nombre de clase").name.clone();
                self.emit_init_lexical(&name);
            }
            Stmt::Import(_)
            | Stmt::ExportNamed(_)
            | Stmt::ExportDefault { .. }
            | Stmt::ExportAll { .. } => {
                self.unsupported("módulos (paso 5)");
            }
        }
    }

    pub(crate) fn unsupported(&mut self, what: &str) {
        let i = self.string(JsString::from(what));
        self.emit(Op::Unsupported(i));
    }

    /// La evaluación de una FunctionDeclaration: vacía, salvo las de bloque elevadas por
    /// B.3.2, que copian el binding del bloque al del VariableEnvironment.
    fn function_statement(&mut self, f: &Function) {
        let k = key(f.span, Tag::Function);
        let Some(id) = &f.id else { return };
        let name = id.name.clone();
        if self.tree.hoisted.contains(&k) {
            let t = self.lookup(&name);
            self.emit_get(&t);
            let saved = self.scope_id();
            let vs = self.tree.funcs[self.sti().func].vars_scope;
            self.st().scope = vs;
            let v = self.lookup(&name);
            // La referencia dinámica va en la pila de referencias: el valor ya está arriba.
            self.emit_prepare_set(&v);
            self.emit_set(&v);
            self.emit(Op::Pop);
            self.st().scope = saved;
        } else if self.tree.dynamic_hoist.contains(&k) {
            let skip = self.new_label();
            let n = self.name(&name);
            self.emit(Op::IsHoisted(n));
            self.emit_to(Op::JumpIfFalse(0), skip);
            let t = self.lookup(&name);
            self.emit_get(&t);
            self.emit(Op::SetVarEnv(n));
            self.bind(skip);
        }
    }

    fn labeled(&mut self, s: &Stmt) {
        let mut labels = Vec::new();
        let mut inner = s;
        while let Stmt::Labeled { label, body, .. } = inner {
            labels.push(label.name.clone());
            inner = body;
        }
        let is_loop = matches!(
            inner,
            Stmt::While { .. }
                | Stmt::DoWhile { .. }
                | Stmt::For { .. }
                | Stmt::ForIn { .. }
                | Stmt::ForOf { .. }
        );
        if is_loop {
            self.st().pending_labels.extend(labels);
            self.stmt(inner);
        } else {
            self.st().pending_labels = labels;
            let (brk, _) = self.push_breakable(BreakKind::Label, 0);
            if labelled_fn(s).is_some() {
                self.sub_stmt(inner);
            } else {
                self.stmt(inner);
            }
            self.bind(brk);
            self.pop_breakable(BreakKind::Label);
        }
    }

    fn break_continue(&mut self, label: Option<&Ident>, is_continue: bool) {
        let controls = &self.sti().controls;
        let switches = &self.sti().switch_marks;
        let mut target = None;
        for (i, c) in controls.iter().enumerate().rev() {
            if let Control::Breakable {
                labels, is_loop, ..
            } = c
            {
                let ok = match label {
                    Some(l) => labels.contains(&l.name) && (!is_continue || *is_loop),
                    None => {
                        if is_continue {
                            *is_loop
                        } else {
                            *is_loop || switches.contains(&i)
                        }
                    }
                };
                if ok {
                    target = Some(i);
                    break;
                }
            }
        }
        let target = target.expect("destino de break/continue (validado por el parser)");
        self.emit_unwind(target, is_continue);
    }

    /// Emite la salida hacia el control `target` (sus limpiezas y el salto).
    pub(crate) fn emit_unwind(&mut self, target: usize, is_continue: bool) {
        let keep = match &self.sti().controls[target] {
            Control::Breakable { continue_keep, .. } if is_continue => *continue_keep,
            _ => 0,
        };
        let stop = target + 1 + keep;
        let n = self.sti().controls.len();
        for i in (stop..n).rev() {
            let c = self.sti().controls[i].clone();
            match c {
                Control::Scope => self.emit(Op::PopScope),
                Control::Handler => self.emit(Op::PopHandler),
                Control::Iter(slot) => {
                    self.emit(Op::PopHandler);
                    self.emit(Op::IterClose(slot));
                }
                // E3: AsyncIteratorClose con una completion normal (break/continue).
                Control::AsyncIter(slot) => {
                    self.emit(Op::PopHandler);
                    let skip = self.new_label();
                    self.emit_to(
                        Op::AsyncIterClose {
                            slot,
                            kind_slot: u32::MAX,
                            skip: 0,
                        },
                        skip,
                    );
                    self.emit(Op::Await);
                    self.emit(Op::AsyncIterCloseCheck {
                        kind_slot: u32::MAX,
                    });
                    self.bind(skip);
                }
                Control::Breakable { .. } => {}
                Control::Finally {
                    kind_slot, entry, ..
                } => {
                    let k = self.completion_kind();
                    if let Control::Finally { exits, .. } = &mut self.st().controls[i] {
                        exits.push((
                            k,
                            JumpExit {
                                target,
                                is_continue,
                            },
                        ));
                    }
                    self.emit(Op::PopHandler);
                    self.emit(Op::Int(k as i32));
                    self.emit(Op::InitLocal(kind_slot));
                    self.jump(entry);
                    return;
                }
            }
        }
        let dest = match &self.sti().controls[target] {
            Control::Breakable {
                break_label,
                continue_label,
                ..
            } => {
                if is_continue {
                    continue_label.expect("continue a un bucle")
                } else {
                    *break_label
                }
            }
            _ => unreachable!("destino no es Breakable"),
        };
        self.jump(dest);
    }

    fn switch(&mut self, span: Span, discriminant: &Expr, cases: &[SwitchCase]) {
        self.completion_reset();
        self.expr(discriminant);
        let tmp = self.alloc_temp();
        self.emit(Op::InitLocal(tmp));
        let parent = self.scope_id();
        let scope = self.tree.scope(key(span, Tag::Switch));
        if let Some(id) = scope {
            self.enter_scope(id);
            let all: Vec<Stmt> = cases
                .iter()
                .flat_map(|c| c.consequent.iter().cloned())
                .collect();
            self.instantiate_block_functions(&all);
        }
        let (brk, _) = self.push_breakable(BreakKind::Switch, 0);
        let labels: Vec<Label> = cases.iter().map(|_| self.new_label()).collect();
        let mut default = None;
        for (i, c) in cases.iter().enumerate() {
            match &c.test {
                Some(t) => {
                    self.emit(Op::GetLocal(tmp));
                    self.expr(t);
                    self.emit(Op::StrictEq);
                    self.emit_to(Op::JumpIfTrue(0), labels[i]);
                }
                None => default = Some(labels[i]),
            }
        }
        self.jump(default.unwrap_or(brk));
        for (i, c) in cases.iter().enumerate() {
            self.bind(labels[i]);
            self.statements(&c.consequent);
        }
        self.bind(brk);
        self.pop_breakable(BreakKind::Switch);
        if let Some(id) = scope {
            self.exit_scope(id, parent);
        }
        self.free_temp(tmp);
    }

    fn try_stmt(
        &mut self,
        block: &BlockStmt,
        handler: Option<&CatchClause>,
        finalizer: Option<&BlockStmt>,
    ) {
        self.completion_reset();
        let Some(fin) = finalizer else {
            self.try_catch(block, handler);
            return;
        };
        let ks = self.alloc_temp();
        let vs = self.alloc_temp();
        let entry = self.new_label();
        self.emit_to(
            Op::PushFinally {
                target: 0,
                kind_slot: ks,
                value_slot: vs,
            },
            entry,
        );
        self.st().controls.push(Control::Finally {
            kind_slot: ks,
            entry,
            exits: Vec::new(),
        });
        self.try_catch(block, handler);
        let exits = match self.st().controls.pop() {
            Some(Control::Finally { exits, .. }) => exits,
            _ => unreachable!("control de finally"),
        };
        self.emit(Op::PopHandler);
        self.emit(Op::Int(COMPLETION_NORMAL as i32));
        self.emit(Op::InitLocal(ks));
        self.bind(entry);
        let saved = self.sti().completion.map(|cv| {
            let t = self.alloc_temp();
            self.emit(Op::GetLocal(cv));
            self.emit(Op::InitLocal(t));
            self.emit(Op::Undefined);
            self.emit(Op::InitLocal(cv));
            (cv, t)
        });
        self.block(&fin.body, fin.span);
        if let Some((cv, t)) = saved {
            self.emit(Op::GetLocal(t));
            self.emit(Op::InitLocal(cv));
        }
        let table = self.add_const(Constant::FinallyTable(std::rc::Rc::new(Vec::new())));
        self.emit(Op::EndFinally {
            kind_slot: ks,
            value_slot: vs,
            table,
        });
        let after = self.new_label();
        self.jump(after);
        let mut entries = Vec::new();
        for (k, exit) in exits {
            entries.push((k, self.pc()));
            self.emit_unwind(exit.target, exit.is_continue);
        }
        self.st().consts[table as usize] = Constant::FinallyTable(std::rc::Rc::new(entries));
        self.bind(after);
        if let Some((_, t)) = saved {
            self.free_temp(t);
        }
        self.free_temp(ks);
    }

    fn try_catch(&mut self, block: &BlockStmt, handler: Option<&CatchClause>) {
        let Some(h) = handler else {
            self.block(&block.body, block.span);
            return;
        };
        let catch_l = self.new_label();
        let end = self.new_label();
        self.emit_to(Op::PushCatch(0), catch_l);
        self.st().controls.push(Control::Handler);
        self.block(&block.body, block.span);
        self.st().controls.pop();
        self.emit(Op::PopHandler);
        self.jump(end);
        self.bind(catch_l);
        self.completion_reset();
        match &h.param {
            Some(p) => {
                let parent = self.scope_id();
                let id = self
                    .tree
                    .scope(key(h.span, Tag::Catch))
                    .expect("scope del catch");
                self.enter_scope(id);
                self.pattern_binding(p, BindMode::Lexical);
                self.block(&h.body.body, h.body.span);
                self.exit_scope(id, parent);
            }
            None => {
                self.emit(Op::Pop);
                self.block(&h.body.body, h.body.span);
            }
        }
        self.bind(end);
    }

    fn for_stmt(
        &mut self,
        span: Span,
        init: Option<&ForInit>,
        test: Option<&Expr>,
        update: Option<&Expr>,
        body: &Stmt,
    ) {
        self.completion_reset();
        let parent = self.scope_id();
        let head = self.tree.scope(key(span, Tag::ForHead));
        let mut per_iteration = false;
        if let Some(id) = head {
            self.enter_scope(id);
            if let Some(ForInit::Variable(d)) = init {
                per_iteration = d.kind == VarKind::Let && self.tree.scopes[id].materialized;
            }
        }
        match init {
            Some(ForInit::Variable(d)) => self.var_declaration(d),
            Some(ForInit::Expr(e)) => {
                self.expr(e);
                self.emit(Op::Pop);
            }
            None => {}
        }
        if per_iteration {
            self.emit(Op::CopyScope);
        }
        let (brk, cont) = self.push_breakable(BreakKind::Loop, 0);
        let top = self.new_label();
        self.bind(top);
        if let Some(t) = test {
            self.expr(t);
            self.emit_to(Op::JumpIfFalse(0), brk);
        }
        self.stmt(body);
        self.bind(cont);
        if per_iteration {
            self.emit(Op::CopyScope);
        }
        if let Some(u) = update {
            self.expr(u);
            self.emit(Op::Pop);
        }
        self.jump(top);
        self.bind(brk);
        self.pop_breakable(BreakKind::Loop);
        if let Some(id) = head {
            self.exit_scope(id, parent);
        }
    }

    fn for_in_of(
        &mut self,
        span: Span,
        left: &ForHead,
        right: &Expr,
        body: &Stmt,
        is_of: bool,
        is_await: bool,
    ) {
        self.completion_reset();
        let parent = self.scope_id();
        // B.3.5: `for (var x = init in o)` evalúa el inicializador antes.
        if let ForHead::Variable(d) = left
            && d.kind == VarKind::Var
            && d.declarations[0].init.is_some()
        {
            self.var_declaration(d);
        }
        if let Some(id) = self.tree.scope(key(span, Tag::ForTdz)) {
            self.enter_scope(id);
            self.expr(right);
            self.exit_scope(id, parent);
        } else {
            self.expr(right);
        }
        let slot = self.alloc_temp();
        self.alloc_temp();
        self.alloc_temp();
        if is_await {
            // E3: ForIn/OfHeadEvaluation con iteratorKind async (§14.7.5.6).
            self.emit(Op::GetAsyncIterator(slot));
        } else if is_of {
            self.emit(Op::GetIterator(slot));
        } else {
            self.emit(Op::ForInPrepare(slot));
        }
        let iter_scope = self.tree.scope(key(span, Tag::ForIter));
        let materialized = iter_scope.is_some_and(|id| self.tree.scopes[id].materialized);
        let keep = usize::from(is_of) + usize::from(materialized);
        let (brk, cont) = self.push_breakable(BreakKind::Loop, keep);
        let next = self.new_label();
        self.bind(next);
        if is_await {
            // E3: §14.7.5.7 pasos 6.a–f.
            self.emit(Op::AsyncIterNext(slot));
            self.emit(Op::Await);
            self.emit_to(Op::AsyncIterResult { slot, done: 0 }, brk);
        } else if is_of {
            self.emit_to(Op::IterStep { slot, done: 0 }, brk);
        } else {
            self.emit_to(Op::ForInNext { slot, done: 0 }, brk);
        }
        let cleanup = self.new_label();
        let (ks, vs) = (self.alloc_temp(), self.alloc_temp());
        if is_of {
            self.emit_to(
                Op::PushFinally {
                    target: 0,
                    kind_slot: ks,
                    value_slot: vs,
                },
                cleanup,
            );
            let c = if is_await {
                Control::AsyncIter(slot)
            } else {
                Control::Iter(slot)
            };
            self.st().controls.push(c);
        }
        if let Some(id) = iter_scope {
            self.enter_scope(id);
        }
        match left {
            ForHead::Variable(d) => {
                let mode = if d.kind == VarKind::Var {
                    BindMode::Var
                } else {
                    BindMode::Lexical
                };
                if matches!(d.kind, VarKind::Using | VarKind::AwaitUsing) {
                    self.unsupported("using (explicit-resource-management)");
                }
                self.pattern_binding(&d.declarations[0].id, mode);
            }
            ForHead::Pattern(p) => self.pattern_binding(p, BindMode::Assign),
        }
        self.stmt(body);
        self.bind(cont);
        if let Some(id) = iter_scope {
            self.exit_scope(id, parent);
            // `exit_scope` dejó el scope en el padre; los controles del bucle siguen.
        }
        if is_of {
            self.emit(Op::PopHandler);
            self.st().controls.pop();
        }
        self.jump(next);
        if is_await {
            // E3: AsyncIteratorClose(iteratorRecord, status) (§7.4.13) con la completion
            // abrupta del cuerpo en `ks`/`vs`.
            self.bind(cleanup);
            let after = self.new_label();
            let catch = self.new_label();
            self.emit_to(
                Op::AsyncIterClose {
                    slot,
                    kind_slot: ks,
                    skip: 0,
                },
                after,
            );
            self.emit_to(Op::PushCatch(0), catch);
            self.emit(Op::Await);
            self.emit(Op::PopHandler);
            self.emit(Op::AsyncIterCloseCheck { kind_slot: ks });
            self.jump(after);
            self.bind(catch);
            self.emit(Op::AsyncIterCloseCatch { kind_slot: ks });
            self.bind(after);
            let table = self.add_const(Constant::FinallyTable(std::rc::Rc::new(Vec::new())));
            self.emit(Op::EndFinally {
                kind_slot: ks,
                value_slot: vs,
                table,
            });
        } else if is_of {
            self.bind(cleanup);
            self.emit(Op::IterCleanup {
                slot,
                kind_slot: ks,
                value_slot: vs,
            });
        }
        self.bind(brk);
        self.pop_breakable(BreakKind::Loop);
        self.free_temp(slot);
    }

    pub(crate) fn var_declaration(&mut self, d: &VariableDeclaration) {
        if matches!(d.kind, VarKind::Using | VarKind::AwaitUsing) {
            self.unsupported("using (explicit-resource-management)");
            return;
        }
        let var = d.kind == VarKind::Var;
        for dec in &d.declarations {
            self.pos(dec.span.start);
            match (&dec.id, &dec.init) {
                (Pat::Ident(id), init) => {
                    if var {
                        let Some(init) = init else { continue };
                        let t = self.lookup(&id.name);
                        self.emit_prepare_set(&t);
                        self.expr_named(init, Some(NameSource::Const(id.name.to_js_string())));
                        self.emit_set(&t);
                        self.emit(Op::Pop);
                    } else {
                        match init {
                            Some(init) => self
                                .expr_named(init, Some(NameSource::Const(id.name.to_js_string()))),
                            None => self.emit(Op::Undefined),
                        }
                        self.emit_init_lexical(&id.name);
                    }
                }
                (p, Some(init)) => {
                    self.expr(init);
                    self.pattern_binding(
                        p,
                        if var {
                            BindMode::Var
                        } else {
                            BindMode::Lexical
                        },
                    );
                }
                (_, None) => {}
            }
        }
    }
}
