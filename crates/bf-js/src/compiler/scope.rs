//! Resolución de ámbitos (la pasada previa a la generación de código).
//!
//! Construye el árbol de scopes del programa con las declaraciones de cada uno según la
//! semántica estática del spec (VarDeclaredNames, LexicallyScopedDeclarations, BoundNames,
//! ContainsExpression, IsSimpleParameterList, las funciones de bloque de B.3.2) y resuelve cada
//! referencia para saber qué bindings captura una clausura. Con eso decide dónde vive cada
//! binding: en un slot del frame (local, no capturado) o en un slot de un [`Env`] del heap
//! (capturado, o visible desde un `eval` directo). Las referencias que atraviesan un `with` o
//! el var scope de una función con `eval` directo no estricto se resuelven dinámicamente.
//!
//! [`Env`]: crate::vm::env::Env

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::ast::*;
use crate::compiler::bytecode::CodeKind;
use crate::string::Atom;
use crate::vm::env::{BindingKind, ScopeLayout};

pub type ScopeId = usize;
pub type FuncId = usize;

/// Qué nodo origina un scope (o una función): span y una etiqueta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeKey {
    pub start: u32,
    pub end: u32,
    pub tag: Tag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tag {
    Program,
    FuncName,
    Function,
    Params,
    Vars,
    Lex,
    Block,
    Switch,
    ForHead,
    ForTdz,
    ForIter,
    Catch,
    Class,
    With,
    FieldInit,
    StaticBlock,
}

pub fn key(span: Span, tag: Tag) -> NodeKey {
    NodeKey {
        start: span.start,
        end: span.end,
        tag,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    /// El top-level de un script: sus declaraciones son globales (por nombre).
    Global,
    /// El top-level de un `eval`: sus `var` (no estricto) van al varEnv del llamador.
    EvalTop,
    /// El top-level de un módulo: su Module Environment Record (§9.1.1.5); todos sus
    /// bindings viven en el environment (otros módulos los importan) (E1).
    Module,
    FuncName,
    Function,
    Params,
    Vars,
    Lex,
    Block,
    Catch {
        simple: bool,
    },
    Class,
    With,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BKind {
    Var,
    Let,
    Const,
    /// Declaración `class` (binding externo, como `let`).
    Class,
    /// El binding interno de la clase (inmutable).
    ClassInner,
    /// El nombre de una expresión de función.
    FunctionName,
    Param,
    CatchParam,
    Arguments,
    /// Declaración de función a nivel de función (var).
    Function,
    /// Declaración de función en un bloque (léxica).
    BlockFunction,
    Private,
    Using,
    /// Un binding de import indirecto (CreateImportBinding, §9.1.1.5.5) (E1).
    Import,
}

impl BKind {
    pub fn is_lexical(self) -> bool {
        matches!(
            self,
            BKind::Let
                | BKind::Const
                | BKind::Class
                | BKind::ClassInner
                | BKind::Using
                | BKind::Import
        )
    }

    pub fn is_const(self) -> bool {
        matches!(
            self,
            BKind::Const | BKind::ClassInner | BKind::Using | BKind::Import
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loc {
    Local(u32),
    Env(u32),
}

#[derive(Debug, Clone)]
pub struct Binding {
    pub name: Atom,
    pub kind: BKind,
    pub captured: bool,
    pub loc: Loc,
}

#[derive(Debug, Clone)]
pub struct Scope {
    pub kind: ScopeKind,
    pub parent: Option<ScopeId>,
    pub func: FuncId,
    pub bindings: Vec<Binding>,
    /// Un `eval` directo no estricto puede agregar bindings aquí.
    pub eval_barrier: bool,
    /// Un `eval` directo ve los bindings de este scope.
    pub contains_eval: bool,
    pub materialized: bool,
    pub layout: Rc<ScopeLayout>,
}

impl Scope {
    pub fn find(&self, name: &str) -> Option<usize> {
        self.bindings.iter().rposition(|b| b.name.as_str() == name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgsMode {
    None,
    Unmapped,
    Mapped,
}

#[derive(Debug, Clone)]
pub struct FuncInfo {
    pub kind: CodeKind,
    pub strict: bool,
    pub is_generator: bool,
    pub is_async: bool,
    pub parent: Option<FuncId>,
    pub this_captured: bool,
    pub new_target_captured: bool,
    pub function_captured: bool,
    /// ¿Hay un `eval` directo en el código propio o en flechas anidadas?
    pub has_eval: bool,
    /// Un `eval` directo no estricto en el código propio.
    pub sloppy_eval: bool,
    pub uses_arguments: bool,
    pub arguments: ArgsMode,
    pub simple_params: bool,
    pub has_param_expressions: bool,
    pub name_scope: Option<ScopeId>,
    pub func_scope: ScopeId,
    pub params_scope: ScopeId,
    pub vars_scope: ScopeId,
    pub lex_scope: ScopeId,
    pub num_locals: u32,
    pub has_with: bool,
}

impl FuncInfo {
    pub fn is_arrow(&self) -> bool {
        self.kind == CodeKind::Arrow
    }

    /// ¿Tiene `this` propio (no léxico)?
    pub fn has_own_this(&self) -> bool {
        !self.is_arrow()
    }

    /// ¿El Function Environment guarda `this`/`new.target`/la función?
    pub fn needs_function_env(&self) -> bool {
        self.has_own_this()
            && (self.this_captured
                || self.new_target_captured
                || self.function_captured
                || self.has_eval)
            && !matches!(self.kind, CodeKind::Script | CodeKind::Eval)
    }
}

/// Resultado de resolver un nombre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// En el scope `scope`, binding `index`.
    Static { scope: ScopeId, index: usize },
    /// Búsqueda dinámica desde el environment actual.
    Dynamic,
    /// Directo al Global Environment.
    Global,
}

#[derive(Debug, Default)]
pub struct ScopeTree {
    pub scopes: Vec<Scope>,
    pub funcs: Vec<FuncInfo>,
    pub scope_of: HashMap<NodeKey, ScopeId>,
    pub func_of: HashMap<NodeKey, FuncId>,
    /// Funciones de bloque elevadas (B.3.2.1) en funciones: nodo de la declaración.
    pub hoisted: HashSet<NodeKey>,
    /// Candidatas de B.3.2.2/B.3.2.3 (top-level de script o eval): la decisión es dinámica.
    pub dynamic_hoist: HashSet<NodeKey>,
    pub annex_b_names: Vec<Atom>,
    /// El código top-level: script o eval (los nombres no resueltos son dinámicos en eval).
    pub is_eval: bool,
    /// El código top-level es un módulo (E1).
    pub is_module: bool,
}

impl ScopeTree {
    pub fn scope(&self, k: NodeKey) -> Option<ScopeId> {
        self.scope_of.get(&k).copied()
    }

    pub fn func(&self, k: NodeKey) -> FuncId {
        self.func_of[&k]
    }

    /// Resuelve `name` desde el scope `from` (GetIdentifierReference estático).
    pub fn resolve(&self, from: ScopeId, name: &str) -> Resolution {
        let mut dynamic = false;
        let mut s = Some(from);
        while let Some(id) = s {
            let scope = &self.scopes[id];
            if scope.kind == ScopeKind::With {
                dynamic = true;
            }
            if let Some(i) = scope.find(name) {
                return if dynamic {
                    Resolution::Dynamic
                } else {
                    Resolution::Static {
                        scope: id,
                        index: i,
                    }
                };
            }
            if scope.eval_barrier {
                dynamic = true;
            }
            s = scope.parent;
        }
        if dynamic || self.is_eval {
            Resolution::Dynamic
        } else {
            Resolution::Global
        }
    }

    /// El scope donde se encuentra el binding si la resolución dinámica lo encontraría
    /// (para marcarlo como capturado).
    fn find_any(&self, from: ScopeId, name: &str) -> Option<(ScopeId, usize)> {
        let mut s = Some(from);
        while let Some(id) = s {
            if let Some(i) = self.scopes[id].find(name) {
                return Some((id, i));
            }
            s = self.scopes[id].parent;
        }
        None
    }

    /// Cantidad de environments materializados entre `from` (inclusive) y `to` (exclusive).
    pub fn hops(&self, from: ScopeId, to: ScopeId) -> u32 {
        let mut n = 0;
        let mut s = from;
        while s != to {
            if self.scopes[s].materialized {
                n += 1;
            }
            s = self.scopes[s].parent.expect("scope ancestro");
        }
        n
    }
}

/// Las declaraciones de una lista de sentencias.
#[derive(Default)]
struct Decls {
    /// VarDeclaredNames (sin funciones de top-level).
    vars: Vec<Atom>,
    /// Funciones de top-level (var-scoped).
    functions: Vec<Atom>,
    /// Léxicas de top-level: (nombre, clase).
    lexical: Vec<(Atom, BKind)>,
}

/// BoundNames de un patrón.
pub fn bound_names(p: &Pat, out: &mut Vec<Atom>) {
    match p {
        Pat::Ident(i) => out.push(i.name.clone()),
        Pat::Object { properties, .. } => {
            for prop in properties {
                match prop {
                    ObjectPatProp::Property { value, .. } => bound_names(value, out),
                    ObjectPatProp::Rest { argument, .. } => bound_names(argument, out),
                }
            }
        }
        Pat::Array { elements, .. } => {
            for e in elements.iter().flatten() {
                bound_names(e, out);
            }
        }
        Pat::Rest { argument, .. } => bound_names(argument, out),
        Pat::Assign { left, .. } => bound_names(left, out),
        Pat::Expr(_) => {}
    }
}

fn decl_bound_names(d: &VariableDeclaration) -> Vec<Atom> {
    let mut out = Vec::new();
    for dec in &d.declarations {
        bound_names(&dec.id, &mut out);
    }
    out
}

fn var_kind_to_bkind(k: VarKind) -> BKind {
    match k {
        VarKind::Var => BKind::Var,
        VarKind::Let => BKind::Let,
        VarKind::Const => BKind::Const,
        VarKind::Using | VarKind::AwaitUsing => BKind::Using,
    }
}

/// VarDeclaredNames de una sentencia (sin entrar a funciones).
fn collect_vars(s: &Stmt, out: &mut Vec<Atom>) {
    match s {
        Stmt::Variable(d) if d.kind == VarKind::Var => out.extend(decl_bound_names(d)),
        Stmt::Block(b) => {
            for s in &b.body {
                collect_vars(s, out);
            }
        }
        Stmt::If {
            consequent,
            alternate,
            ..
        } => {
            collect_vars(consequent, out);
            if let Some(a) = alternate {
                collect_vars(a, out);
            }
        }
        Stmt::While { body, .. } | Stmt::DoWhile { body, .. } | Stmt::With { body, .. } => {
            collect_vars(body, out)
        }
        Stmt::Labeled { body, .. } => collect_vars(body, out),
        Stmt::For { init, body, .. } => {
            if let Some(ForInit::Variable(d)) = init
                && d.kind == VarKind::Var
            {
                out.extend(decl_bound_names(d));
            }
            collect_vars(body, out);
        }
        Stmt::ForIn { left, body, .. } | Stmt::ForOf { left, body, .. } => {
            if let ForHead::Variable(d) = left
                && d.kind == VarKind::Var
            {
                out.extend(decl_bound_names(d));
            }
            collect_vars(body, out);
        }
        Stmt::Try {
            block,
            handler,
            finalizer,
            ..
        } => {
            for s in &block.body {
                collect_vars(s, out);
            }
            if let Some(h) = handler {
                for s in &h.body.body {
                    collect_vars(s, out);
                }
            }
            if let Some(f) = finalizer {
                for s in &f.body {
                    collect_vars(s, out);
                }
            }
        }
        Stmt::Switch { cases, .. } => {
            for c in cases {
                for s in &c.consequent {
                    collect_vars(s, out);
                }
            }
        }
        Stmt::ExportNamed(e) => {
            if let Some(d) = &e.declaration {
                collect_vars(d, out);
            }
        }
        _ => {}
    }
}

/// La función de un LabelledStatement (`a: b: function f() {}`).
fn labelled_function(s: &Stmt) -> Option<&Function> {
    match s {
        Stmt::Labeled { body, .. } => match &**body {
            Stmt::Function(f) => Some(f),
            other => labelled_function(other),
        },
        _ => None,
    }
}

/// Las declaraciones de top-level de una lista (cuerpo de función, script o eval).
fn top_level_decls(body: &[Stmt]) -> Decls {
    let mut d = Decls::default();
    for s in body {
        match s {
            Stmt::Function(f) => {
                if let Some(id) = &f.id {
                    d.functions.push(id.name.clone());
                }
            }
            Stmt::Variable(v) if v.kind != VarKind::Var => {
                for n in decl_bound_names(v) {
                    d.lexical.push((n, var_kind_to_bkind(v.kind)));
                }
            }
            Stmt::Class(c) => {
                if let Some(id) = &c.id {
                    d.lexical.push((id.name.clone(), BKind::Class));
                }
            }
            _ => {
                if let Some(f) = labelled_function(s) {
                    if let Some(id) = &f.id {
                        d.functions.push(id.name.clone());
                    }
                } else {
                    collect_vars(s, &mut d.vars);
                }
            }
        }
    }
    d
}

/// LexicallyScopedDeclarations de un bloque: (nombre, clase).
fn block_decls(body: &[Stmt]) -> Vec<(Atom, BKind)> {
    let mut out = Vec::new();
    for s in body {
        match s {
            Stmt::Function(f) => {
                if let Some(id) = &f.id {
                    out.push((id.name.clone(), BKind::BlockFunction));
                }
            }
            Stmt::Variable(v) if v.kind != VarKind::Var => {
                for n in decl_bound_names(v) {
                    out.push((n, var_kind_to_bkind(v.kind)));
                }
            }
            Stmt::Class(c) => {
                if let Some(id) = &c.id {
                    out.push((id.name.clone(), BKind::Class));
                }
            }
            _ => {
                if let Some(f) = labelled_function(s)
                    && let Some(id) = &f.id
                {
                    out.push((id.name.clone(), BKind::BlockFunction));
                }
            }
        }
    }
    out
}

/// ¿Algún patrón de parámetros contiene expresiones (ContainsExpression)?
fn pat_has_expression(p: &Pat) -> bool {
    match p {
        Pat::Ident(_) => false,
        Pat::Assign { .. } => true,
        Pat::Rest { argument, .. } => pat_has_expression(argument),
        Pat::Array { elements, .. } => elements.iter().flatten().any(pat_has_expression),
        Pat::Object { properties, .. } => properties.iter().any(|p| match p {
            ObjectPatProp::Property { key, value, .. } => {
                matches!(key, PropKey::Computed(_)) || pat_has_expression(value)
            }
            ObjectPatProp::Rest { argument, .. } => pat_has_expression(argument),
        }),
        Pat::Expr(_) => true,
    }
}

/// ¿El cuerpo empieza con la directiva `"use strict"`?
pub fn has_use_strict(body: &[Stmt]) -> bool {
    for s in body {
        match s {
            Stmt::Expression {
                directive: Some(d), ..
            } => {
                if d == "use strict" {
                    return true;
                }
            }
            _ => return false,
        }
    }
    false
}

/// Contexto del analizador para el código top-level.
pub struct TopContext {
    pub is_eval: bool,
    pub strict: bool,
}

pub struct Analyzer {
    pub tree: ScopeTree,
    cur: ScopeId,
    func: FuncId,
    /// Dentro de una clase (herencia y claves calculadas): código estricto.
    force_strict: bool,
}

/// Información de la función que se analiza (lo que el llamador conoce).
pub struct FnSpec<'a> {
    pub kind: CodeKind,
    pub key: NodeKey,
    pub name_binding: Option<&'a Ident>,
    pub params: &'a [Pat],
    pub body: FnBody<'a>,
    pub is_generator: bool,
    pub is_async: bool,
    /// ¿El código que contiene la función es estricto (o es una clase)?
    pub outer_strict: bool,
}

pub enum FnBody<'a> {
    Block(&'a [Stmt]),
    Expr(&'a Expr),
}

impl Analyzer {
    /// Analiza un script o un `eval`.
    pub fn analyze_program(program: &Program, ctx: &TopContext) -> ScopeTree {
        let strict = ctx.strict || has_use_strict(&program.body);
        let mut a = Analyzer {
            tree: ScopeTree {
                is_eval: ctx.is_eval,
                ..ScopeTree::default()
            },
            cur: 0,
            func: 0,
            force_strict: false,
        };
        let kind = if ctx.is_eval {
            CodeKind::Eval
        } else {
            CodeKind::Script
        };
        a.tree.funcs.push(FuncInfo {
            kind,
            strict,
            is_generator: false,
            is_async: false,
            parent: None,
            this_captured: false,
            new_target_captured: false,
            function_captured: false,
            has_eval: false,
            sloppy_eval: false,
            uses_arguments: false,
            arguments: ArgsMode::None,
            simple_params: true,
            has_param_expressions: false,
            name_scope: None,
            func_scope: 0,
            params_scope: 0,
            vars_scope: 0,
            lex_scope: 0,
            num_locals: 0,
            has_with: false,
        });
        let scope_kind = if ctx.is_eval {
            ScopeKind::EvalTop
        } else {
            ScopeKind::Global
        };
        let top = a.new_scope(scope_kind, None);
        a.tree.scope_of.insert(key(program.span, Tag::Program), top);
        a.tree.func_of.insert(key(program.span, Tag::Program), 0);
        a.cur = top;
        let decls = top_level_decls(&program.body);
        // Los bindings del top-level: en un script son todos globales (por nombre); en un eval,
        // los léxicos (y en modo estricto también los var) son del environment propio.
        if ctx.is_eval {
            for (n, k) in &decls.lexical {
                a.declare(top, n.clone(), *k);
            }
            if strict {
                for n in decls.vars.iter().chain(decls.functions.iter()) {
                    a.declare(top, n.clone(), BKind::Var);
                }
            }
        }
        // B.3.2.2 / B.3.2.3: las candidatas se deciden al ejecutar.
        if !strict {
            let top_lex: HashSet<Atom> = decls.lexical.iter().map(|(n, _)| n.clone()).collect();
            let mut found = Vec::new();
            for s in &program.body {
                annex_b_candidates(
                    s,
                    &mut vec![LexLevel::plain(top_lex.clone())],
                    &mut found,
                    false,
                );
            }
            for (k, name) in found {
                a.tree.dynamic_hoist.insert(k);
                if !a.tree.annex_b_names.contains(&name) {
                    a.tree.annex_b_names.push(name);
                }
            }
        }
        for s in &program.body {
            a.stmt(s);
        }
        a.finish();
        a.tree
    }

    fn new_scope(&mut self, kind: ScopeKind, parent: Option<ScopeId>) -> ScopeId {
        self.tree.scopes.push(Scope {
            kind,
            parent,
            func: self.func,
            bindings: Vec::new(),
            eval_barrier: false,
            contains_eval: false,
            materialized: false,
            layout: Rc::new(ScopeLayout::default()),
        });
        self.tree.scopes.len() - 1
    }

    fn declare(&mut self, scope: ScopeId, name: Atom, kind: BKind) -> usize {
        let s = &mut self.tree.scopes[scope];
        if let Some(i) = s.find(&name) {
            // Redeclaraciones válidas (var + var, función + var, parámetros duplicados): un
            // solo binding. Una función reemplaza la clase de un var/parámetro.
            if kind == BKind::Function || kind == BKind::BlockFunction {
                s.bindings[i].kind = kind;
            }
            return i;
        }
        s.bindings.push(Binding {
            name,
            kind,
            captured: false,
            loc: Loc::Local(0),
        });
        s.bindings.len() - 1
    }

    fn enter(&mut self, kind: ScopeKind, k: NodeKey) -> ScopeId {
        let id = self.new_scope(kind, Some(self.cur));
        self.tree.scope_of.insert(k, id);
        self.cur = id;
        id
    }

    fn info(&mut self) -> &mut FuncInfo {
        let f = self.func;
        &mut self.tree.funcs[f]
    }

    fn strict(&self) -> bool {
        self.force_strict || self.tree.funcs[self.func].strict
    }

    // --- Referencias ---

    fn reference(&mut self, name: &str) {
        if name == "arguments" {
            // `arguments` en una flecha es el de la función que la contiene.
            let mut f = self.func;
            while self.tree.funcs[f].is_arrow() {
                match self.tree.funcs[f].parent {
                    Some(p) => f = p,
                    None => break,
                }
            }
            self.tree.funcs[f].uses_arguments = true;
        }
        if let Some((scope, index)) = self.tree.find_any(self.cur, name) {
            let crosses = self.tree.scopes[scope].func != self.func;
            let dynamic = matches!(self.tree.resolve(self.cur, name), Resolution::Dynamic);
            if crosses || dynamic {
                self.tree.scopes[scope].bindings[index].captured = true;
            }
        }
    }

    /// `this`, `new.target` o `super` desde el código actual: si se usa desde una flecha,
    /// la función que lo provee lo guarda en su environment.
    fn use_ctx(&mut self, what: u8) {
        let mut f = self.func;
        let mut crossed = false;
        while self.tree.funcs[f].is_arrow() {
            match self.tree.funcs[f].parent {
                Some(p) => {
                    f = p;
                    crossed = true;
                }
                None => break,
            }
        }
        if crossed {
            let info = &mut self.tree.funcs[f];
            match what {
                0 => info.this_captured = true,
                1 => info.new_target_captured = true,
                _ => info.function_captured = true,
            }
        }
    }

    fn direct_eval(&mut self) {
        let mut s = Some(self.cur);
        while let Some(id) = s {
            self.tree.scopes[id].contains_eval = true;
            s = self.tree.scopes[id].parent;
        }
        let strict = self.strict();
        let mut f = Some(self.func);
        let mut first = true;
        while let Some(id) = f {
            let info = &mut self.tree.funcs[id];
            info.has_eval = true;
            if first && !strict {
                info.sloppy_eval = true;
            }
            first = false;
            if info.has_own_this() {
                info.this_captured = true;
                info.new_target_captured = true;
                info.function_captured = true;
                info.uses_arguments = true;
                break;
            }
            f = info.parent;
        }
        if !strict {
            // Las `var` del eval van al VariableEnvironment: el var scope de la función o,
            // durante los parámetros, su Function Environment.
            let target = self.var_env_scope();
            self.tree.scopes[target].eval_barrier = true;
        }
    }

    /// El scope que hace de VariableEnvironment en la posición actual.
    fn var_env_scope(&self) -> ScopeId {
        let info = &self.tree.funcs[self.func];
        if matches!(info.kind, CodeKind::Script | CodeKind::Eval) {
            return info.vars_scope;
        }
        // ¿Estamos en los parámetros (antes del var scope)?
        let mut s = Some(self.cur);
        while let Some(id) = s {
            if id == info.vars_scope {
                return info.vars_scope;
            }
            if id == info.func_scope {
                return info.func_scope;
            }
            s = self.tree.scopes[id].parent;
        }
        info.vars_scope
    }

    // --- Funciones ---

    pub fn function(&mut self, spec: &FnSpec<'_>) -> FuncId {
        let saved = (self.cur, self.func);
        let body_stmts: &[Stmt] = match spec.body {
            FnBody::Block(b) => b,
            FnBody::Expr(_) => &[],
        };
        let strict = spec.outer_strict
            || has_use_strict(body_stmts)
            || matches!(spec.kind, CodeKind::ClassConstructor { .. });
        let simple_params = spec.params.iter().all(|p| matches!(p, Pat::Ident(_)));
        let has_param_expressions = spec.params.iter().any(pat_has_expression);
        let fid = self.tree.funcs.len();
        self.tree.funcs.push(FuncInfo {
            kind: spec.kind,
            strict,
            is_generator: spec.is_generator,
            is_async: spec.is_async,
            parent: Some(self.func),
            this_captured: false,
            new_target_captured: false,
            function_captured: false,
            has_eval: false,
            sloppy_eval: false,
            uses_arguments: false,
            arguments: ArgsMode::None,
            simple_params,
            has_param_expressions,
            name_scope: None,
            func_scope: 0,
            params_scope: 0,
            vars_scope: 0,
            lex_scope: 0,
            num_locals: 0,
            has_with: false,
        });
        self.tree.func_of.insert(spec.key, fid);
        // El nombre de una expresión de función (funcEnv) vive afuera de la función.
        if let Some(id) = spec.name_binding {
            let ns = self.enter(
                ScopeKind::FuncName,
                NodeKey {
                    tag: Tag::FuncName,
                    ..spec.key
                },
            );
            self.declare(ns, id.name.clone(), BKind::FunctionName);
            self.tree.funcs[fid].name_scope = Some(ns);
        }
        self.func = fid;
        let fscope = self.enter(
            ScopeKind::Function,
            NodeKey {
                tag: Tag::Function,
                ..spec.key
            },
        );
        let pscope = if strict || !has_param_expressions {
            fscope
        } else {
            self.enter(
                ScopeKind::Params,
                NodeKey {
                    tag: Tag::Params,
                    ..spec.key
                },
            )
        };
        let mut param_names = Vec::new();
        for p in spec.params {
            bound_names(p, &mut param_names);
        }
        for n in &param_names {
            self.declare(pscope, n.clone(), BKind::Param);
        }
        let decls = top_level_decls(body_stmts);
        // `arguments` (§10.2.11 pasos 15–18).
        let args_possible = !matches!(
            spec.kind,
            CodeKind::Arrow | CodeKind::FieldInit | CodeKind::StaticBlock
        ) && !param_names.iter().any(|n| n == "arguments")
            && (has_param_expressions
                || !(decls.functions.iter().any(|n| n == "arguments")
                    || decls.lexical.iter().any(|(n, _)| n == "arguments")));
        if args_possible {
            self.declare(pscope, Atom::new("arguments"), BKind::Arguments);
        }
        let vscope = if has_param_expressions {
            self.enter(
                ScopeKind::Vars,
                NodeKey {
                    tag: Tag::Vars,
                    ..spec.key
                },
            )
        } else {
            pscope
        };
        for n in decls.vars.iter().chain(decls.functions.iter()) {
            // En un var scope separado, cada var tiene su binding (se inicializa con el valor
            // del parámetro del mismo nombre).
            self.declare(vscope, n.clone(), BKind::Var);
        }
        for n in &decls.functions {
            self.declare(vscope, n.clone(), BKind::Function);
        }
        let lscope = if strict {
            vscope
        } else {
            self.enter(
                ScopeKind::Lex,
                NodeKey {
                    tag: Tag::Lex,
                    ..spec.key
                },
            )
        };
        for (n, k) in &decls.lexical {
            self.declare(lscope, n.clone(), *k);
        }
        {
            let info = &mut self.tree.funcs[fid];
            info.func_scope = fscope;
            info.params_scope = pscope;
            info.vars_scope = vscope;
            info.lex_scope = lscope;
        }
        // B.3.2.1: funciones en bloques elevadas a var (solo funciones no estrictas).
        if !strict && matches!(spec.body, FnBody::Block(_)) {
            let top_lex: HashSet<Atom> = decls.lexical.iter().map(|(n, _)| n.clone()).collect();
            let mut found = Vec::new();
            for s in body_stmts {
                annex_b_candidates(
                    s,
                    &mut vec![LexLevel::plain(top_lex.clone())],
                    &mut found,
                    false,
                );
            }
            for (k, name) in found {
                if param_names.contains(&name) {
                    continue;
                }
                self.tree.hoisted.insert(k);
                if self.tree.scopes[vscope].find(&name).is_none() && name != "arguments" {
                    self.declare(vscope, name, BKind::Var);
                }
            }
        }
        // Los parámetros se evalúan en su scope (con `cur` en pscope).
        self.cur = pscope;
        for p in spec.params {
            self.pattern(p, true);
        }
        self.cur = lscope;
        match spec.body {
            FnBody::Block(b) => {
                for s in b {
                    self.stmt(s);
                }
            }
            FnBody::Expr(e) => self.expr(e),
        }
        let info = &mut self.tree.funcs[fid];
        if args_possible && (info.uses_arguments || info.has_eval) {
            info.arguments = if strict || !simple_params {
                ArgsMode::Unmapped
            } else {
                ArgsMode::Mapped
            };
        }
        (self.cur, self.func) = saved;
        fid
    }

    fn function_node(&mut self, f: &Function, kind: CodeKind, expr_name: bool) -> FuncId {
        let outer_strict = self.strict();
        self.function(&FnSpec {
            kind,
            key: key(f.span, Tag::Function),
            name_binding: if expr_name { f.id.as_ref() } else { None },
            params: &f.params,
            body: FnBody::Block(&f.body.body),
            is_generator: f.is_generator,
            is_async: f.is_async,
            outer_strict,
        })
    }

    fn arrow(&mut self, a: &Arrow) {
        let outer_strict = self.strict();
        let body = match &a.body {
            ArrowBody::Expr(e) => FnBody::Expr(e),
            ArrowBody::Block(b) => FnBody::Block(&b.body),
        };
        self.function(&FnSpec {
            kind: CodeKind::Arrow,
            key: key(a.span, Tag::Function),
            name_binding: None,
            params: &a.params,
            body,
            is_generator: false,
            is_async: a.is_async,
            outer_strict,
        });
    }

    fn class(&mut self, c: &Class) {
        let saved = self.cur;
        let cs = self.enter(ScopeKind::Class, key(c.span, Tag::Class));
        if let Some(id) = &c.id {
            self.declare(cs, id.name.clone(), BKind::ClassInner);
        }
        for e in &c.body {
            let k = match e {
                ClassElement::Method {
                    key: PropKey::Private(id),
                    ..
                }
                | ClassElement::Property {
                    key: PropKey::Private(id),
                    ..
                } => Some(id),
                _ => None,
            };
            if let Some(id) = k {
                let i = self.declare(cs, Atom::new(&format!("#{}", id.name)), BKind::Private);
                self.tree.scopes[cs].bindings[i].captured = true;
            }
        }
        // La herencia se evalúa en el classEnv (con el nombre en TDZ).
        // Todo el código de la clase es estricto (§11.2.2).
        let saved_strict = std::mem::replace(&mut self.force_strict, true);
        if let Some(sup) = &c.super_class {
            self.expr(sup);
        }
        let derived = c.super_class.is_some();
        for e in &c.body {
            match e {
                ClassElement::Method {
                    key: k,
                    value,
                    kind,
                    ..
                } => {
                    self.prop_key(k);
                    let ck = if *kind == MethodKind::Constructor {
                        CodeKind::ClassConstructor { derived }
                    } else {
                        CodeKind::Method
                    };
                    self.function(&FnSpec {
                        kind: ck,
                        key: key(value.span, Tag::Function),
                        name_binding: None,
                        params: &value.params,
                        body: FnBody::Block(&value.body.body),
                        is_generator: value.is_generator,
                        is_async: value.is_async,
                        outer_strict: true,
                    });
                }
                ClassElement::Property {
                    span,
                    key: k,
                    value,
                    ..
                } => {
                    self.prop_key(k);
                    if let Some(v) = value {
                        self.function(&FnSpec {
                            kind: CodeKind::FieldInit,
                            key: key(*span, Tag::FieldInit),
                            name_binding: None,
                            params: &[],
                            body: FnBody::Expr(v),
                            is_generator: false,
                            is_async: false,
                            outer_strict: true,
                        });
                    }
                }
                ClassElement::StaticBlock(b) => {
                    self.function(&FnSpec {
                        kind: CodeKind::StaticBlock,
                        key: key(b.span, Tag::StaticBlock),
                        name_binding: None,
                        params: &[],
                        body: FnBody::Block(&b.body),
                        is_generator: false,
                        is_async: false,
                        outer_strict: true,
                    });
                }
            }
        }
        self.force_strict = saved_strict;
        self.cur = saved;
    }

    fn prop_key(&mut self, k: &PropKey) {
        if let PropKey::Computed(e) = k {
            self.expr(e);
        }
    }

    // --- Sentencias ---

    fn block_scope(&mut self, body: &[Stmt], k: NodeKey) -> Option<ScopeId> {
        let decls = block_decls(body);
        if decls.is_empty() {
            return None;
        }
        let id = self.enter(ScopeKind::Block, k);
        for (n, kind) in decls {
            self.declare(id, n, kind);
        }
        Some(id)
    }

    fn stmts_in_block(&mut self, body: &[Stmt], k: NodeKey) {
        let saved = self.cur;
        self.block_scope(body, k);
        for s in body {
            self.stmt(s);
        }
        self.cur = saved;
    }

    /// Una sentencia en posición de cuerpo (`if`, etiquetas): una declaración de función ahí
    /// (B.3.3) se trata como si estuviera en un bloque propio.
    fn sub_stmt(&mut self, s: &Stmt) {
        if let Stmt::Function(f) = s {
            self.stmts_in_block(std::slice::from_ref(s), key(f.span, Tag::Block));
        } else {
            self.stmt(s);
        }
    }

    pub fn stmt(&mut self, s: &Stmt) {
        if let Stmt::ForOf { is_await: true, .. } = s {
            self.note_await();
        }
        match s {
            Stmt::Expression { expression, .. } => self.expr(expression),
            Stmt::Block(b) => self.stmts_in_block(&b.body, key(b.span, Tag::Block)),
            Stmt::Empty(_) | Stmt::Debugger(_) => {}
            Stmt::With { object, body, span } => {
                self.expr(object);
                let saved = self.cur;
                self.enter(ScopeKind::With, key(*span, Tag::With));
                self.info().has_with = true;
                self.stmt(body);
                self.cur = saved;
            }
            Stmt::Return { argument, .. } => {
                if let Some(e) = argument {
                    self.expr(e);
                }
            }
            Stmt::Labeled { body, .. } => self.sub_stmt(body),
            Stmt::Break { .. } | Stmt::Continue { .. } => {}
            Stmt::If {
                test,
                consequent,
                alternate,
                ..
            } => {
                self.expr(test);
                self.sub_stmt(consequent);
                if let Some(a) = alternate {
                    self.sub_stmt(a);
                }
            }
            Stmt::Switch {
                discriminant,
                cases,
                span,
            } => {
                self.expr(discriminant);
                let saved = self.cur;
                let mut decls = Vec::new();
                for c in cases {
                    decls.extend(block_decls(&c.consequent));
                }
                if !decls.is_empty() {
                    let id = self.enter(ScopeKind::Block, key(*span, Tag::Switch));
                    for (n, k) in decls {
                        self.declare(id, n, k);
                    }
                }
                for c in cases {
                    if let Some(t) = &c.test {
                        self.expr(t);
                    }
                    for s in &c.consequent {
                        self.stmt(s);
                    }
                }
                self.cur = saved;
            }
            Stmt::Throw { argument, .. } => self.expr(argument),
            Stmt::Try {
                block,
                handler,
                finalizer,
                ..
            } => {
                self.stmts_in_block(&block.body, key(block.span, Tag::Block));
                if let Some(h) = handler {
                    let saved = self.cur;
                    if let Some(p) = &h.param {
                        let simple = matches!(p, Pat::Ident(_));
                        let id = self.enter(ScopeKind::Catch { simple }, key(h.span, Tag::Catch));
                        let mut names = Vec::new();
                        bound_names(p, &mut names);
                        for n in names {
                            self.declare(id, n, BKind::CatchParam);
                        }
                        self.pattern(p, true);
                    }
                    self.stmts_in_block(&h.body.body, key(h.body.span, Tag::Block));
                    self.cur = saved;
                }
                if let Some(f) = finalizer {
                    self.stmts_in_block(&f.body, key(f.span, Tag::Block));
                }
            }
            Stmt::While { test, body, .. } => {
                self.expr(test);
                self.stmt(body);
            }
            Stmt::DoWhile { body, test, .. } => {
                self.stmt(body);
                self.expr(test);
            }
            Stmt::For {
                init,
                test,
                update,
                body,
                span,
            } => {
                let saved = self.cur;
                if let Some(ForInit::Variable(d)) = init
                    && d.kind != VarKind::Var
                {
                    let id = self.enter(ScopeKind::Block, key(*span, Tag::ForHead));
                    for n in decl_bound_names(d) {
                        self.declare(id, n, var_kind_to_bkind(d.kind));
                    }
                }
                match init {
                    Some(ForInit::Variable(d)) => self.var_decl(d),
                    Some(ForInit::Expr(e)) => self.expr(e),
                    None => {}
                }
                if let Some(t) = test {
                    self.expr(t);
                }
                if let Some(u) = update {
                    self.expr(u);
                }
                self.stmt(body);
                self.cur = saved;
            }
            Stmt::ForIn {
                left,
                right,
                body,
                span,
            }
            | Stmt::ForOf {
                left,
                right,
                body,
                span,
                ..
            } => {
                let saved = self.cur;
                let lexical = match left {
                    ForHead::Variable(d) if d.kind != VarKind::Var => Some(d),
                    _ => None,
                };
                if let Some(d) = lexical {
                    let id = self.enter(ScopeKind::Block, key(*span, Tag::ForTdz));
                    for n in decl_bound_names(d) {
                        self.declare(id, n, var_kind_to_bkind(d.kind));
                    }
                    self.expr(right);
                    self.cur = saved;
                    let id = self.enter(ScopeKind::Block, key(*span, Tag::ForIter));
                    for n in decl_bound_names(d) {
                        self.declare(id, n, var_kind_to_bkind(d.kind));
                    }
                    self.var_decl(d);
                } else {
                    self.expr(right);
                    match left {
                        ForHead::Variable(d) => self.var_decl(d),
                        ForHead::Pattern(p) => self.pattern(p, false),
                    }
                }
                self.stmt(body);
                self.cur = saved;
            }
            Stmt::Function(f) => {
                self.function_node(f, CodeKind::Normal, false);
            }
            Stmt::Variable(d) => self.var_decl(d),
            Stmt::Class(c) => self.class(c),
            Stmt::Import(_) | Stmt::ExportAll { .. } => {}
            Stmt::ExportNamed(e) => {
                if let Some(d) = &e.declaration {
                    self.stmt(d);
                }
            }
            Stmt::ExportDefault { declaration, .. } => match declaration {
                ExportDefaultKind::Function(f) => {
                    self.function_node(f, CodeKind::Normal, false);
                }
                ExportDefaultKind::Class(c) => self.class(c),
                ExportDefaultKind::Expr(e) => self.expr(e),
            },
        }
    }

    fn var_decl(&mut self, d: &VariableDeclaration) {
        if d.kind == VarKind::AwaitUsing {
            self.note_await();
        }
        for dec in &d.declarations {
            self.pattern(&dec.id, true);
            if let Some(init) = &dec.init {
                self.expr(init);
            }
        }
    }

    /// Un patrón: los identificadores son referencias (destinos de asignación o bindings ya
    /// declarados), más las expresiones que contenga.
    fn pattern(&mut self, p: &Pat, _binding: bool) {
        match p {
            Pat::Ident(i) => self.reference(&i.name),
            Pat::Object { properties, .. } => {
                for prop in properties {
                    match prop {
                        ObjectPatProp::Property { key: k, value, .. } => {
                            self.prop_key(k);
                            self.pattern(value, _binding);
                        }
                        ObjectPatProp::Rest { argument, .. } => self.pattern(argument, _binding),
                    }
                }
            }
            Pat::Array { elements, .. } => {
                for e in elements.iter().flatten() {
                    self.pattern(e, _binding);
                }
            }
            Pat::Rest { argument, .. } => self.pattern(argument, _binding),
            Pat::Assign { left, right, .. } => {
                self.pattern(left, _binding);
                self.expr(right);
            }
            Pat::Expr(e) => self.expr(e),
        }
    }

    // --- Expresiones ---

    pub fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Ident(i) => self.reference(&i.name),
            Expr::Literal(_) => {}
            Expr::This(_) => self.use_ctx(0),
            Expr::Super(_) => {
                self.use_ctx(0);
                self.use_ctx(2);
            }
            Expr::Array { elements, .. } => {
                for el in elements.iter().flatten() {
                    self.expr(&el.expr);
                }
            }
            Expr::Object { properties, .. } => {
                for p in properties {
                    match p {
                        ObjectMember::Property { key: k, value, .. } => {
                            self.prop_key(k);
                            match value {
                                PropValue::Expr(e) => self.expr(e),
                                PropValue::Method(f) => {
                                    self.function_node(f, CodeKind::Method, false);
                                }
                                PropValue::CoverInit(i, e) => {
                                    self.reference(&i.name);
                                    self.expr(e);
                                }
                            }
                        }
                        ObjectMember::Spread { argument, .. } => self.expr(argument),
                    }
                }
            }
            Expr::Function(f) => {
                self.function_node(f, CodeKind::Normal, true);
            }
            Expr::Arrow(a) => self.arrow(a),
            Expr::Class(c) => self.class(c),
            Expr::Template(t) => {
                for e in &t.expressions {
                    self.expr(e);
                }
            }
            Expr::TaggedTemplate { tag, quasi, .. } => {
                self.expr(tag);
                for e in &quasi.expressions {
                    self.expr(e);
                }
            }
            Expr::Unary { argument, .. } | Expr::Update { argument, .. } => self.expr(argument),
            Expr::Binary { left, right, .. } | Expr::Logical { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            Expr::Assign { left, right, .. } => {
                self.pattern(left, false);
                self.expr(right);
            }
            Expr::Conditional {
                test,
                consequent,
                alternate,
                ..
            } => {
                self.expr(test);
                self.expr(consequent);
                self.expr(alternate);
            }
            Expr::Call {
                callee,
                arguments,
                optional,
                ..
            } => {
                if let Expr::Ident(i) = &**callee
                    && i.name == "eval"
                    && !*optional
                {
                    self.direct_eval();
                }
                if let Expr::Super(_) = &**callee {
                    self.use_ctx(0);
                    self.use_ctx(1);
                    self.use_ctx(2);
                } else {
                    self.expr(callee);
                }
                for a in arguments {
                    self.expr(&a.expr);
                }
            }
            Expr::New {
                callee, arguments, ..
            } => {
                self.expr(callee);
                for a in arguments {
                    self.expr(&a.expr);
                }
            }
            Expr::Sequence { expressions, .. } => {
                for e in expressions {
                    self.expr(e);
                }
            }
            Expr::Member {
                object, property, ..
            } => {
                self.expr(object);
                match property {
                    MemberProp::Computed(e) => self.expr(e),
                    MemberProp::Private(id) => self.reference(&format!("#{}", id.name)),
                    MemberProp::Ident(_) => {}
                }
            }
            Expr::Chain { expression, .. } | Expr::Paren { expression, .. } => {
                self.expr(expression)
            }
            Expr::Yield { argument, .. } => {
                if let Some(a) = argument {
                    self.expr(a);
                }
            }
            Expr::Await { argument, .. } => {
                self.note_await();
                self.expr(argument);
            }
            Expr::MetaProperty { meta, .. } => {
                if meta.name == "new" {
                    self.use_ctx(1);
                }
            }
            Expr::Import {
                source, options, ..
            } => {
                self.expr(source);
                if let Some(o) = options {
                    self.expr(o);
                }
            }
            Expr::PrivateName(id) => self.reference(&format!("#{}", id.name)),
        }
    }

    // --- Ubicaciones ---

    fn finish(&mut self) {
        for f in 0..self.tree.funcs.len() {
            self.tree.funcs[f].num_locals = 0;
        }
        for sid in 0..self.tree.scopes.len() {
            let fid = self.tree.scopes[sid].func;
            let info = self.tree.funcs[fid].clone();
            let scope = &self.tree.scopes[sid];
            let mapped_params = info.arguments == ArgsMode::Mapped && sid == info.params_scope;
            let mut env_count = 0u32;
            let mut layout = ScopeLayout {
                simple_catch: matches!(scope.kind, ScopeKind::Catch { .. }),
                ..ScopeLayout::default()
            };
            let mut locs = Vec::new();
            let mut locals = info.num_locals;
            for b in &scope.bindings {
                let in_env = b.captured
                    || scope.kind == ScopeKind::Module
                    || scope.contains_eval
                    || (mapped_params && b.kind == BKind::Param)
                    || b.kind == BKind::Private;
                if in_env {
                    layout.names.push(b.name.clone());
                    layout.kinds.push(match b.kind {
                        BKind::Const | BKind::ClassInner | BKind::Using => BindingKind::Const,
                        BKind::FunctionName => BindingKind::FunctionName,
                        _ => BindingKind::Mutable,
                    });
                    layout.initialized.push(
                        matches!(b.kind, BKind::Var | BKind::Function)
                            && !matches!(scope.kind, ScopeKind::Global),
                    );
                    locs.push(Loc::Env(env_count));
                    env_count += 1;
                } else {
                    locs.push(Loc::Local(locals));
                    locals += 1;
                }
            }
            let is_func_scope = sid == info.func_scope;
            let top = matches!(scope.kind, ScopeKind::Global | ScopeKind::EvalTop);
            let materialized = env_count > 0
                || scope.kind == ScopeKind::With
                || scope.kind == ScopeKind::Module
                || (scope.eval_barrier && !top)
                || (is_func_scope && info.needs_function_env());
            let scope = &mut self.tree.scopes[sid];
            for (b, l) in scope.bindings.iter_mut().zip(locs) {
                b.loc = l;
            }
            scope.materialized = materialized;
            scope.layout = Rc::new(layout);
            self.tree.funcs[fid].num_locals = locals;
        }
    }
}

/// Los nombres léxicos de un scope que encierra a una función de bloque: `all` (todos, para
/// los scopes externos) y `own` (sin las funciones de bloque, para el bloque de la propia
/// función, que puede repetirlas por B.3.2.4).
#[derive(Clone, Default)]
struct LexLevel {
    all: HashSet<Atom>,
    own: HashSet<Atom>,
}

impl LexLevel {
    fn plain(names: HashSet<Atom>) -> LexLevel {
        LexLevel {
            all: names.clone(),
            own: names,
        }
    }
}

/// Busca funciones de bloque candidatas a B.3.2: `lex` es la pila de nombres léxicos de los
/// scopes que encierran la posición.
fn annex_b_candidates(
    s: &Stmt,
    lex: &mut Vec<LexLevel>,
    out: &mut Vec<(NodeKey, Atom)>,
    in_block: bool,
) {
    let conflicts = |lex: &Vec<LexLevel>, name: &Atom| {
        let n = lex.len();
        lex.iter().enumerate().any(|(i, l)| {
            if i + 1 == n {
                l.own.contains(name)
            } else {
                l.all.contains(name)
            }
        })
    };
    match s {
        Stmt::Function(f) if in_block => {
            if let Some(id) = &f.id
                && !f.is_generator
                && !f.is_async
                && id.name != "arguments"
                && !conflicts(lex, &id.name)
            {
                out.push((key(f.span, Tag::Function), id.name.clone()));
            }
        }
        Stmt::Block(b) => block_candidates(&b.body, lex, out),
        Stmt::If {
            consequent,
            alternate,
            ..
        } => {
            for c in std::iter::once(&**consequent).chain(alternate.as_deref()) {
                if let Stmt::Function(_) = c {
                    // B.3.3: como si estuviera en un bloque propio.
                    lex.push(LexLevel::default());
                    annex_b_candidates(c, lex, out, true);
                    lex.pop();
                } else {
                    annex_b_candidates(c, lex, out, false);
                }
            }
        }
        Stmt::Labeled { body, .. } => annex_b_candidates(body, lex, out, false),
        Stmt::While { body, .. } | Stmt::DoWhile { body, .. } | Stmt::With { body, .. } => {
            annex_b_candidates(body, lex, out, false)
        }
        Stmt::For { init, body, .. } => {
            let mut names = HashSet::new();
            if let Some(ForInit::Variable(d)) = init
                && d.kind != VarKind::Var
            {
                names.extend(decl_bound_names(d));
            }
            lex.push(LexLevel::plain(names));
            annex_b_candidates(body, lex, out, false);
            lex.pop();
        }
        Stmt::ForIn { left, body, .. } | Stmt::ForOf { left, body, .. } => {
            let mut names = HashSet::new();
            if let ForHead::Variable(d) = left
                && d.kind != VarKind::Var
            {
                names.extend(decl_bound_names(d));
            }
            lex.push(LexLevel::plain(names));
            annex_b_candidates(body, lex, out, false);
            lex.pop();
        }
        Stmt::Try {
            block,
            handler,
            finalizer,
            ..
        } => {
            block_candidates(&block.body, lex, out);
            if let Some(h) = handler {
                let mut names = HashSet::new();
                if let Some(p) = &h.param
                    && !matches!(p, Pat::Ident(_))
                {
                    let mut v = Vec::new();
                    bound_names(p, &mut v);
                    names.extend(v);
                }
                lex.push(LexLevel::plain(names));
                block_candidates(&h.body.body, lex, out);
                lex.pop();
            }
            if let Some(f) = finalizer {
                block_candidates(&f.body, lex, out);
            }
        }
        Stmt::Switch { cases, .. } => {
            let mut all_stmts = Vec::new();
            let mut decls = Vec::new();
            for c in cases {
                decls.extend(block_decls(&c.consequent));
                all_stmts.extend(c.consequent.iter());
            }
            lex.push(level_of(decls));
            for s in all_stmts {
                annex_b_candidates(s, lex, out, true);
            }
            lex.pop();
        }
        _ => {}
    }
}

fn level_of(decls: Vec<(Atom, BKind)>) -> LexLevel {
    let mut l = LexLevel::default();
    for (n, k) in decls {
        if k != BKind::BlockFunction {
            l.own.insert(n.clone());
        }
        l.all.insert(n);
    }
    l
}

fn block_candidates(body: &[Stmt], lex: &mut Vec<LexLevel>, out: &mut Vec<(NodeKey, Atom)>) {
    lex.push(level_of(block_decls(body)));
    for s in body {
        annex_b_candidates(s, lex, out, true);
    }
    lex.pop();
}

// --- Módulos (E1) ---

/// Las declaraciones del top-level de un módulo (§16.2.1.6 VarScopedDeclarations y
/// LexicallyScopedDeclarations de ModuleItemList): las de las sentencias `export` incluidas.
/// En un módulo las funciones del top-level son léxicas, pero se instancian (inicializadas)
/// en InitializeEnvironment.
pub fn module_decls(body: &[Stmt]) -> (Vec<Atom>, Vec<Atom>, Vec<(Atom, BKind)>) {
    let mut vars = Vec::new();
    let mut functions = Vec::new();
    let mut lexical = Vec::new();
    let item = |s: &Stmt,
                vars: &mut Vec<Atom>,
                functions: &mut Vec<Atom>,
                lexical: &mut Vec<(Atom, BKind)>| {
        let d = top_level_decls(std::slice::from_ref(s));
        vars.extend(d.vars);
        functions.extend(d.functions);
        lexical.extend(d.lexical);
    };
    for s in body {
        match s {
            Stmt::ExportNamed(e) => {
                if let Some(d) = &e.declaration {
                    item(d, &mut vars, &mut functions, &mut lexical);
                }
            }
            Stmt::ExportDefault { declaration, .. } => match declaration {
                ExportDefaultKind::Function(f) => functions.push(
                    f.id.as_ref()
                        .map_or_else(|| Atom::new("*default*"), |i| i.name.clone()),
                ),
                ExportDefaultKind::Class(c) => lexical.push((
                    c.id.as_ref()
                        .map_or_else(|| Atom::new("*default*"), |i| i.name.clone()),
                    BKind::Class,
                )),
                ExportDefaultKind::Expr(_) => lexical.push((Atom::new("*default*"), BKind::Let)),
            },
            Stmt::Import(_) | Stmt::ExportAll { .. } => {}
            other => item(other, &mut vars, &mut functions, &mut lexical),
        }
    }
    (vars, functions, lexical)
}

impl Analyzer {
    /// Analiza un módulo (§16.2): código estricto con un único scope top-level (el Module
    /// Environment Record), donde se declaran también los bindings de import.
    pub fn analyze_module(program: &Program) -> ScopeTree {
        let mut a = Analyzer {
            tree: ScopeTree {
                is_module: true,
                ..ScopeTree::default()
            },
            cur: 0,
            func: 0,
            force_strict: false,
        };
        a.tree.funcs.push(FuncInfo {
            kind: CodeKind::Script,
            strict: true,
            is_generator: false,
            is_async: false,
            parent: None,
            this_captured: false,
            new_target_captured: false,
            function_captured: false,
            has_eval: false,
            sloppy_eval: false,
            uses_arguments: false,
            arguments: ArgsMode::None,
            simple_params: true,
            has_param_expressions: false,
            name_scope: None,
            func_scope: 0,
            params_scope: 0,
            vars_scope: 0,
            lex_scope: 0,
            num_locals: 0,
            has_with: false,
        });
        let top = a.new_scope(ScopeKind::Module, None);
        a.tree.scope_of.insert(key(program.span, Tag::Program), top);
        a.tree.func_of.insert(key(program.span, Tag::Program), 0);
        a.cur = top;
        // Los bindings de import (§16.2.1.6.4 InitializeEnvironment, paso 7).
        for s in &program.body {
            if let Stmt::Import(i) = s {
                for spec in &i.specifiers {
                    match spec {
                        ImportSpecifier::Namespace { local, .. } => {
                            a.declare(top, local.name.clone(), BKind::Const);
                        }
                        ImportSpecifier::Default { local, .. }
                        | ImportSpecifier::Named { local, .. } => {
                            a.declare(top, local.name.clone(), BKind::Import);
                        }
                    }
                }
            }
        }
        let (vars, functions, lexical) = module_decls(&program.body);
        for n in vars {
            a.declare(top, n, BKind::Var);
        }
        for n in functions {
            a.declare(top, n, BKind::Function);
        }
        for (n, k) in lexical {
            a.declare(top, n, k);
        }
        for s in &program.body {
            a.stmt(s);
        }
        a.finish();
        a.tree
    }

    /// `await` en el código propio del top-level de un módulo: HasTLA (§16.2.1.6.1 ParseModule,
    /// ContainsAwait).
    fn note_await(&mut self) {
        if self.tree.is_module && self.func == 0 {
            self.tree.funcs[0].is_async = true;
        }
    }
}
