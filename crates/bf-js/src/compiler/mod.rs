//! Compilador del AST a bytecode ([`bytecode`]).
//!
//! Primero [`scope`] resuelve los ámbitos de todo el programa (qué binding usa cada
//! referencia, cuáles captura una clausura, dónde vive cada uno); después la generación de
//! código recorre el AST función por función. Las funciones anidadas se compilan al
//! encontrarlas y quedan como constantes de la función que las contiene.
//!
//! El prólogo de cada función implementa FunctionDeclarationInstantiation (§10.2.11) en
//! bytecode: environments de parámetros/variables/léxicos según el spec, el objeto
//! `arguments`, la vinculación de parámetros (con TDZ si hay expresiones), los `var`, las
//! funciones de bloque de B.3.2.1 y las declaraciones de función.

pub mod bytecode;
mod class;
mod expr;
pub mod module; // E1
mod pattern;
pub mod scope;
mod stmt;

use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::*;
use crate::string::{Atom, JsString};
use crate::value::PropertyKey;
use bytecode::*;
use scope::*;

/// Una etiqueta de salto (se resuelve al terminar la función).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Label(usize);

/// Dónde está un binding, para leerlo o escribirlo.
#[derive(Debug, Clone)]
pub(crate) enum Target {
    Local {
        slot: u32,
        kind: BKind,
        name: Atom,
        check: bool,
    },
    Env {
        hops: u32,
        slot: u32,
        kind: BKind,
        name: Atom,
        check: bool,
    },
    /// El nombre de la expresión de función, no capturado: la función activa.
    Callee,
    Dynamic(Atom),
    Global(Atom),
}

/// Las estructuras de control abiertas (para `break`/`continue`).
#[derive(Debug, Clone)]
pub(crate) enum Control {
    Breakable {
        labels: Vec<Atom>,
        is_loop: bool,
        break_label: Label,
        continue_label: Option<Label>,
        /// Cuántos controles encima de este los maneja el destino de `continue`.
        continue_keep: usize,
    },
    Scope,
    Handler,
    /// Un iterador con su handler de limpieza.
    Iter(u32),
    /// Un iterador async de `for await` con su handler de limpieza (E3).
    AsyncIter(u32),
    Finally {
        kind_slot: u32,
        entry: Label,
        exits: Vec<(u32, JumpExit)>,
    },
}

/// Un salto pendiente que atraviesa un `finally`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct JumpExit {
    /// Índice del control destino.
    pub target: usize,
    pub is_continue: bool,
}

/// De dónde sale el nombre en una NamedEvaluation.
#[derive(Debug, Clone)]
pub(crate) enum NameSource {
    Const(JsString),
    /// La clave en el tope de la pila.
    Stack(NamePrefix),
    /// `[[ClassFieldInitializerName]]` de la función activa.
    FieldName,
}

pub(crate) struct FnState {
    pub func: FuncId,
    pub kind: CodeKind,
    pub strict: bool,
    pub ops: Vec<Op>,
    pub consts: Vec<Constant>,
    names: HashMap<Atom, u32>,
    strings: HashMap<JsString, u32>,
    pub next_local: u32,
    pub max_locals: u32,
    pub scope: ScopeId,
    pub controls: Vec<Control>,
    labels: Vec<Option<u32>>,
    patches: Vec<(usize, Label)>,
    positions: Vec<(u32, u32)>,
    /// Slot del valor de completion (código de script y eval).
    pub completion: Option<u32>,
    /// Profundidad de código de clase (estricto).
    pub class_depth: u32,
    /// Scopes de clase cuyos nombres privados no se ven (la herencia).
    pub private_hidden: Vec<ScopeId>,
    /// Destino del cortocircuito de la cadena opcional actual.
    pub chain_short: Vec<Label>,
    /// Etiquetas que se aplican a la próxima sentencia.
    pub pending_labels: Vec<Atom>,
    /// Índices de los controles que son `switch` (destino de `break` sin etiqueta).
    pub switch_marks: Vec<usize>,
    next_completion_kind: u32,
    /// Nombres léxicos del top-level de un script (globales).
    pub global_lex: Vec<Atom>,
}

pub struct Codegen<'t> {
    pub(crate) tree: &'t ScopeTree,
    pub(crate) source: Rc<Source>,
    pub(crate) fs: Vec<FnState>,
    /// El texto fuente del próximo método (su MethodDefinition, para `toString`).
    pub(crate) method_src: Option<Span>,
}

/// Contexto de un `eval` directo (lo que PerformEval pasa al parser y al compilador).
#[derive(Debug, Clone, Copy, Default)]
pub struct EvalFlags {
    pub strict: bool,
    pub new_target: bool,
    pub super_property: bool,
    pub super_call: bool,
    pub class_field: bool,
}

impl EvalFlags {
    pub fn to_bits(self) -> u32 {
        u32::from(self.strict)
            | u32::from(self.new_target) << 1
            | u32::from(self.super_property) << 2
            | u32::from(self.super_call) << 3
            | u32::from(self.class_field) << 4
    }

    pub fn from_bits(b: u32) -> EvalFlags {
        EvalFlags {
            strict: b & 1 != 0,
            new_target: b & 2 != 0,
            super_property: b & 4 != 0,
            super_call: b & 8 != 0,
            class_field: b & 16 != 0,
        }
    }
}

/// Compila un Script (§16.1): el código del top-level, con sus Declarations para
/// GlobalDeclarationInstantiation.
pub fn compile_script(program: &Program, source: Rc<Source>, strict: bool) -> Rc<FunctionCode> {
    let tree = Analyzer::analyze_program(
        program,
        &TopContext {
            is_eval: false,
            strict,
        },
    );
    let mut cg = Codegen {
        tree: &tree,
        source,
        fs: Vec::new(),
        method_src: None,
    };
    cg.top_level(program, CodeKind::Script)
}

/// Compila el código de un `eval` (§19.2.1.1).
pub fn compile_eval(program: &Program, source: Rc<Source>, strict: bool) -> Rc<FunctionCode> {
    let tree = Analyzer::analyze_program(
        program,
        &TopContext {
            is_eval: true,
            strict,
        },
    );
    let mut cg = Codegen {
        tree: &tree,
        source,
        fs: Vec::new(),
        method_src: None,
    };
    cg.top_level(program, CodeKind::Eval)
}

/// Compila la función de CreateDynamicFunction (§20.2.1.1.1): `program` contiene una sola
/// expresión de función (en una sentencia); devuelve su código.
pub fn compile_dynamic_function(program: &Program, source: Rc<Source>) -> Option<Rc<FunctionCode>> {
    let tree = Analyzer::analyze_program(
        program,
        &TopContext {
            is_eval: false,
            strict: false,
        },
    );
    let mut cg = Codegen {
        tree: &tree,
        source,
        fs: Vec::new(),
        method_src: None,
    };
    let f = match program.body.first()? {
        Stmt::Expression { expression, .. } => match expression {
            Expr::Paren { expression, .. } => match &**expression {
                Expr::Function(f) => f,
                _ => return None,
            },
            Expr::Function(f) => f,
            _ => return None,
        },
        _ => return None,
    };
    cg.fs.push(FnState::new(0, CodeKind::Script, false, 0, 0));
    let idx = cg.function(
        f,
        CodeKind::Normal,
        Some(NameSource::Const(JsString::from("anonymous"))),
    );
    let st = cg.fs.pop()?;
    match &st.consts[idx as usize] {
        Constant::Function(code) => Some(code.clone()),
        _ => None,
    }
}

impl FnState {
    fn new(func: FuncId, kind: CodeKind, strict: bool, scope: ScopeId, locals: u32) -> FnState {
        FnState {
            func,
            kind,
            strict,
            ops: Vec::new(),
            consts: Vec::new(),
            names: HashMap::new(),
            strings: HashMap::new(),
            next_local: locals,
            max_locals: locals,
            scope,
            controls: Vec::new(),
            labels: Vec::new(),
            patches: Vec::new(),
            positions: Vec::new(),
            completion: None,
            class_depth: 0,
            private_hidden: Vec::new(),
            chain_short: Vec::new(),
            pending_labels: Vec::new(),
            switch_marks: Vec::new(),
            next_completion_kind: COMPLETION_RETURN + 1,
            global_lex: Vec::new(),
        }
    }
}

/// Fija el destino de una instrucción de salto.
fn patch_target(op: &mut Op, t: u32) {
    match op {
        Op::Jump(x)
        | Op::JumpIfTrue(x)
        | Op::JumpIfFalse(x)
        | Op::JumpIfTrueKeep(x)
        | Op::JumpIfFalseKeep(x)
        | Op::JumpIfNotNullishKeep(x)
        | Op::JumpIfNotUndefined(x)
        | Op::PushCatch(x) => *x = t,
        Op::JumpIfNullishChain { target, .. } | Op::PushFinally { target, .. } => *target = t,
        Op::IterStep { done, .. } | Op::ForInNext { done, .. } | Op::YieldStarStep { done, .. } => {
            *done = t
        }
        // E3
        Op::AsyncIterResult { done, .. } | Op::AsyncYieldStarResult { done, .. } => *done = t,
        Op::AsyncIterClose { skip, .. } => *skip = t,
        _ => panic!("instrucción sin destino: {op:?}"),
    }
}

impl<'t> Codegen<'t> {
    // --- Emisión ---

    pub(crate) fn st(&mut self) -> &mut FnState {
        self.fs.last_mut().expect("función actual")
    }

    pub(crate) fn sti(&self) -> &FnState {
        self.fs.last().expect("función actual")
    }

    pub(crate) fn emit(&mut self, op: Op) {
        self.st().ops.push(op);
    }

    pub(crate) fn pc(&self) -> u32 {
        self.sti().ops.len() as u32
    }

    pub(crate) fn new_label(&mut self) -> Label {
        let st = self.st();
        st.labels.push(None);
        Label(st.labels.len() - 1)
    }

    pub(crate) fn bind(&mut self, l: Label) {
        let pc = self.pc();
        self.st().labels[l.0] = Some(pc);
    }

    /// Emite una instrucción de salto hacia `l`.
    pub(crate) fn emit_to(&mut self, op: Op, l: Label) {
        let st = self.st();
        st.patches.push((st.ops.len(), l));
        st.ops.push(op);
    }

    pub(crate) fn jump(&mut self, l: Label) {
        self.emit_to(Op::Jump(0), l);
    }

    pub(crate) fn pos(&mut self, offset: u32) {
        let pc = self.pc();
        let st = self.st();
        match st.positions.last_mut() {
            Some((p, o)) if *p == pc => *o = offset,
            Some((_, o)) if *o == offset => {}
            _ => st.positions.push((pc, offset)),
        }
    }

    pub(crate) fn add_const(&mut self, c: Constant) -> u32 {
        let st = self.st();
        st.consts.push(c);
        st.consts.len() as u32 - 1
    }

    pub(crate) fn name(&mut self, n: &str) -> u32 {
        let atom = Atom::new(n);
        if let Some(&i) = self.sti().names.get(&atom) {
            return i;
        }
        let key = PropertyKey::from_atom(&atom);
        let i = self.add_const(Constant::Name(atom.clone(), key));
        self.st().names.insert(atom, i);
        i
    }

    pub(crate) fn string(&mut self, s: JsString) -> u32 {
        if let Some(&i) = self.sti().strings.get(&s) {
            return i;
        }
        let i = self.add_const(Constant::String(s.clone()));
        self.st().strings.insert(s, i);
        i
    }

    pub(crate) fn number(&mut self, n: f64) {
        if n.fract() == 0.0 && n.abs() < 1e9 && !(n == 0.0 && n.is_sign_negative()) {
            self.emit(Op::Int(n as i32));
        } else {
            let i = self.add_const(Constant::Number(n));
            self.emit(Op::Const(i));
        }
    }

    pub(crate) fn push_str(&mut self, s: &str) {
        let i = self.string(JsString::from(s));
        self.emit(Op::Const(i));
    }

    pub(crate) fn alloc_temp(&mut self) -> u32 {
        let st = self.st();
        let t = st.next_local;
        st.next_local += 1;
        st.max_locals = st.max_locals.max(st.next_local);
        t
    }

    /// Libera los temporales desde `t` (disciplina de pila).
    pub(crate) fn free_temp(&mut self, t: u32) {
        self.st().next_local = t;
    }

    pub(crate) fn strict(&self) -> bool {
        let st = self.sti();
        st.strict || st.class_depth > 0
    }

    pub(crate) fn completion_kind(&mut self) -> u32 {
        let st = self.st();
        st.next_completion_kind += 1;
        st.next_completion_kind - 1
    }

    pub(crate) fn throw_error(&mut self, kind: ErrorKind, msg: &str) {
        let i = self.string(JsString::from(msg));
        self.emit(Op::ThrowError(kind, i));
    }

    fn finish_fn(&mut self, name: JsString, span: Span, length: u32) -> FunctionCode {
        let mut st = self.fs.pop().expect("función");
        for (at, l) in std::mem::take(&mut st.patches) {
            let t = st.labels[l.0].expect("etiqueta sin ubicar");
            patch_target(&mut st.ops[at], t);
        }
        let info = &self.tree.funcs[st.func];
        let has_prototype =
            match st.kind {
                CodeKind::Normal => true,
                CodeKind::Method => info.is_generator,
                _ => false,
            } && !(st.kind == CodeKind::Normal && info.is_async && !info.is_generator);
        FunctionCode {
            name,
            kind: st.kind,
            strict: st.strict,
            is_generator: info.is_generator,
            is_async: info.is_async,
            ops: st.ops,
            consts: st.consts,
            num_locals: st.max_locals,
            length,
            source: self.source.clone(),
            span,
            positions: st.positions,
            has_prototype,
            decls: None,
            dynamic: info.has_eval || info.has_with,
        }
    }

    // --- Scopes ---

    pub(crate) fn scope_id(&self) -> ScopeId {
        self.sti().scope
    }

    /// Entra a un scope (BlockDeclarationInstantiation): environment si está materializado,
    /// TDZ de los léxicos locales.
    pub(crate) fn enter_scope(&mut self, id: ScopeId) {
        let scope = &self.tree.scopes[id];
        if scope.materialized {
            let layout = scope.layout.clone();
            let c = self.add_const(Constant::Layout(layout));
            self.emit(Op::PushScope(c));
            self.st().controls.push(Control::Scope);
        }
        self.st().scope = id;
        self.clear_lexical_locals(id);
    }

    pub(crate) fn clear_lexical_locals(&mut self, id: ScopeId) {
        let slots: Vec<u32> = self.tree.scopes[id]
            .bindings
            .iter()
            .filter_map(|b| match b.loc {
                Loc::Local(s) if b.kind.is_lexical() => Some(s),
                _ => None,
            })
            .collect();
        for s in slots {
            self.emit(Op::ClearLocal(s));
        }
    }

    /// El span de la MethodDefinition de un elemento de clase: sin el `static` inicial
    /// (ClassElement : static MethodDefinition), saltando espacios y comentarios.
    pub(crate) fn method_definition_span(&self, element: Span, is_static: bool) -> Span {
        if !is_static {
            return element;
        }
        let u = &self.source.units;
        let mut i = element.start as usize + "static".len();
        let end = element.end as usize;
        while i < end {
            let c = u[i];
            if c == u16::from(b'/') && u.get(i + 1) == Some(&u16::from(b'*')) {
                i += 2;
                while i + 1 < end && !(u[i] == u16::from(b'*') && u[i + 1] == u16::from(b'/')) {
                    i += 1;
                }
                i += 2;
            } else if c == u16::from(b'/') && u.get(i + 1) == Some(&u16::from(b'/')) {
                while i < end && !matches!(u[i], 0x0A | 0x0D | 0x2028 | 0x2029) {
                    i += 1;
                }
            } else if char::from_u32(u32::from(c)).is_some_and(|ch| {
                crate::lexer::is_whitespace(ch) || crate::lexer::is_line_terminator(ch)
            }) {
                i += 1;
            } else {
                break;
            }
        }
        Span::new(i.min(end) as u32, element.end)
    }

    /// Sale del scope `id` (volviendo a `parent`).
    pub(crate) fn exit_scope(&mut self, id: ScopeId, parent: ScopeId) {
        if self.tree.scopes[id].materialized {
            self.emit(Op::PopScope);
            let c = self.st().controls.pop();
            debug_assert!(matches!(c, Some(Control::Scope)));
        }
        self.st().scope = parent;
    }

    // --- Bindings ---

    /// La ubicación de `name` desde el scope actual.
    pub(crate) fn lookup(&self, name: &str) -> Target {
        let from = self.scope_id();
        match self.tree.resolve(from, name) {
            Resolution::Static { scope, index } => {
                let s = &self.tree.scopes[scope];
                let b = &s.bindings[index];
                let info = &self.tree.funcs[s.func];
                let check = b.kind.is_lexical()
                    || (b.kind == BKind::Param && info.has_param_expressions)
                    || b.kind == BKind::Private;
                match b.loc {
                    Loc::Local(slot) => {
                        if b.kind == BKind::FunctionName {
                            return Target::Callee;
                        }
                        Target::Local {
                            slot,
                            kind: b.kind,
                            name: b.name.clone(),
                            check,
                        }
                    }
                    Loc::Env(slot) => Target::Env {
                        hops: self.tree.hops(from, scope),
                        slot,
                        kind: b.kind,
                        name: b.name.clone(),
                        check,
                    },
                }
            }
            Resolution::Dynamic => Target::Dynamic(Atom::new(name)),
            Resolution::Global => Target::Global(Atom::new(name)),
        }
    }

    pub(crate) fn emit_get(&mut self, t: &Target) {
        match t {
            Target::Local {
                slot, check, name, ..
            } => {
                if *check {
                    let n = self.name(name);
                    self.emit(Op::GetLocalChecked(*slot, n));
                } else {
                    self.emit(Op::GetLocal(*slot));
                }
            }
            Target::Env {
                hops,
                slot,
                check,
                name,
                ..
            } => {
                if *check {
                    let n = self.name(name);
                    self.emit(Op::GetEnvChecked(*hops, *slot, n));
                } else {
                    self.emit(Op::GetEnv(*hops, *slot));
                }
            }
            Target::Callee => self.emit(Op::GetCallee),
            Target::Dynamic(n) => {
                let n = self.name(n);
                self.emit(Op::GetName(n));
            }
            Target::Global(n) => {
                let n = self.name(n);
                self.emit(Op::GetGlobal(n));
            }
        }
    }

    /// Antes de evaluar el valor de una asignación: ResolveBinding de las referencias
    /// dinámicas.
    pub(crate) fn emit_prepare_set(&mut self, t: &Target) {
        match t {
            Target::Dynamic(n) => {
                let n = self.name(n);
                self.emit(Op::ResolveRef(n));
            }
            Target::Global(n) => {
                let n = self.name(n);
                self.emit(Op::ResolveGlobalRef(n));
            }
            _ => {}
        }
    }

    /// PutValue (el valor queda en la pila). Requiere `emit_prepare_set` antes del valor.
    pub(crate) fn emit_set(&mut self, t: &Target) {
        match t {
            Target::Local {
                slot,
                kind,
                name,
                check,
            } => {
                if kind.is_const() {
                    let n = self.name(name);
                    self.emit(Op::ConstAssignLocal(*slot, n));
                } else if *check {
                    let n = self.name(name);
                    self.emit(Op::SetLocalChecked(*slot, n));
                } else {
                    self.emit(Op::SetLocal(*slot));
                }
            }
            Target::Env {
                hops,
                slot,
                kind,
                name,
                check,
            } => {
                if kind.is_const() {
                    let n = self.name(name);
                    self.emit(Op::ConstAssignEnv(*hops, *slot, n));
                } else if *kind == BKind::FunctionName {
                    if self.strict() {
                        self.throw_error(
                            ErrorKind::TypeError,
                            &format!("Assignment to constant variable '{name}'"),
                        );
                    }
                } else if *check {
                    let n = self.name(name);
                    self.emit(Op::SetEnvChecked(*hops, *slot, n));
                } else {
                    self.emit(Op::SetEnv(*hops, *slot));
                }
            }
            Target::Callee => {
                // Binding inmutable no estricto (§9.1.1.1.5): se ignora salvo en modo estricto.
                if self.strict() {
                    self.throw_error(ErrorKind::TypeError, "Assignment to constant variable");
                }
            }
            Target::Dynamic(n) | Target::Global(n) => {
                let n = self.name(n);
                let strict = self.strict();
                self.emit(Op::PutRef(n, strict));
            }
        }
    }

    /// InitializeBinding (saca el valor).
    pub(crate) fn emit_init(&mut self, t: &Target) {
        match t {
            Target::Local { slot, .. } => self.emit(Op::InitLocal(*slot)),
            Target::Env { hops, slot, .. } => self.emit(Op::InitEnv(*hops, *slot)),
            Target::Callee => self.emit(Op::Pop),
            Target::Dynamic(n) | Target::Global(n) => {
                let n = self.name(n);
                self.emit(Op::InitRef(n));
            }
        }
    }

    /// Inicializa la declaración léxica `name` del scope actual (o del top-level global).
    pub(crate) fn emit_init_lexical(&mut self, name: &str) {
        let t = self.lookup(name);
        match t {
            Target::Dynamic(_) | Target::Global(_) => {
                let n = self.name(name);
                self.emit(Op::InitGlobalLex(n));
            }
            t => self.emit_init(&t),
        }
    }

    // --- Contexto de la función (`this`, `new.target`, la función activa) ---

    /// La función que provee `this` a la posición actual y cómo llegar a su contexto.
    pub(crate) fn ctx_loc(&self) -> CtxLoc {
        let mut f = self.sti().func;
        let mut crossed = false;
        loop {
            let info = &self.tree.funcs[f];
            match info.kind {
                CodeKind::Arrow => {
                    crossed = true;
                    match info.parent {
                        Some(p) => f = p,
                        None => return CtxLoc::Dynamic,
                    }
                }
                CodeKind::Eval => return CtxLoc::Dynamic,
                CodeKind::Script => {
                    return if crossed {
                        CtxLoc::Dynamic
                    } else {
                        CtxLoc::Frame
                    };
                }
                _ => {
                    if info.needs_function_env() {
                        let hops = self.tree.hops(self.scope_id(), info.func_scope);
                        // El Function Environment es el scope `func_scope`: hops cuenta los
                        // materializados hasta él (sin incluirlo).
                        return CtxLoc::Env(hops);
                    }
                    debug_assert!(!crossed, "contexto capturado sin environment");
                    return CtxLoc::Frame;
                }
            }
        }
    }

    /// El código no flecha que encierra la posición actual.
    pub(crate) fn this_func(&self) -> &FuncInfo {
        let mut f = self.sti().func;
        loop {
            let info = &self.tree.funcs[f];
            if info.kind != CodeKind::Arrow {
                return info;
            }
            match info.parent {
                Some(p) => f = p,
                None => return info,
            }
        }
    }

    pub(crate) fn emit_ctx(&mut self, what: CtxWhat) {
        let loc = self.ctx_loc();
        self.emit(Op::Ctx(what, loc));
    }

    /// Las flags de PerformEval para un `eval` directo en la posición actual.
    pub(crate) fn eval_flags(&self) -> EvalFlags {
        let strict = self.strict();
        let info = self.this_func();
        let mut f = EvalFlags {
            strict,
            ..EvalFlags::default()
        };
        match info.kind {
            CodeKind::Normal => f.new_target = true,
            CodeKind::Method | CodeKind::StaticBlock => {
                f.new_target = true;
                f.super_property = true;
            }
            CodeKind::ClassConstructor { derived } => {
                f.new_target = true;
                f.super_property = true;
                f.super_call = derived;
            }
            CodeKind::FieldInit => {
                f.new_target = true;
                f.super_property = true;
                f.class_field = true;
            }
            CodeKind::Script | CodeKind::Eval | CodeKind::Arrow => {}
        }
        f
    }

    // --- Código top-level ---

    fn top_level(&mut self, program: &Program, kind: CodeKind) -> Rc<FunctionCode> {
        let pk = key(program.span, Tag::Program);
        let top = self.tree.scope(pk).expect("scope del programa");
        let info = &self.tree.funcs[0];
        let strict = info.strict;
        self.fs
            .push(FnState::new(0, kind, strict, top, info.num_locals));
        let cv = self.alloc_temp();
        self.st().completion = Some(cv);
        self.emit(Op::Undefined);
        self.emit(Op::InitLocal(cv));
        let mut decls = Declarations {
            strict,
            ..Declarations::default()
        };
        // Las declaraciones de top-level.
        let mut var_names = Vec::new();
        let mut funcs: Vec<&Function> = Vec::new();
        for s in &program.body {
            collect_top_level(s, &mut var_names, &mut funcs);
        }
        let mut lex = Vec::new();
        for s in &program.body {
            match s {
                Stmt::Variable(v) if v.kind != VarKind::Var => {
                    let mut names = Vec::new();
                    for d in &v.declarations {
                        bound_names(&d.id, &mut names);
                    }
                    for n in names {
                        lex.push((n, v.kind == VarKind::Const || v.kind == VarKind::Using));
                    }
                }
                Stmt::Class(c) => {
                    if let Some(id) = &c.id {
                        lex.push((id.name.clone(), false));
                    }
                }
                _ => {}
            }
        }
        // Funciones a instanciar: la última de cada nombre, en orden.
        let mut function_names: Vec<Atom> = Vec::new();
        let mut last: Vec<&Function> = Vec::new();
        for f in funcs.iter().rev() {
            let n =
                f.id.as_ref()
                    .map(|i| i.name.clone())
                    .unwrap_or_else(|| Atom::new(""));
            if !function_names.contains(&n) {
                function_names.insert(0, n);
                last.insert(0, f);
            }
        }
        let var_only: Vec<Atom> = {
            let mut out = Vec::new();
            for n in var_names {
                if !function_names.contains(&n) && !out.contains(&n) {
                    out.push(n);
                }
            }
            out
        };
        decls.var_names = var_only.clone();
        decls.function_names = function_names.clone();
        decls.lex_names = lex.clone();
        decls.annex_b = self.tree.annex_b_names.clone();
        if kind == CodeKind::Eval && self.tree.scopes[top].materialized {
            decls.top_layout = Some(self.tree.scopes[top].layout.clone());
        }
        let global_varenv = kind == CodeKind::Script || !strict;
        if kind == CodeKind::Script {
            self.st().global_lex = lex.iter().map(|(n, _)| n.clone()).collect();
            // GlobalDeclarationInstantiation (§16.1.7), pasos de creación.
            for (n, c) in &lex {
                let ni = self.name(n);
                self.emit(Op::DeclareGlobalLex {
                    name: ni,
                    constant: *c,
                });
            }
        }
        if global_varenv {
            for f in &last {
                let idx = self.function(f, CodeKind::Normal, None);
                self.emit(Op::Closure {
                    code: idx,
                    name: FnName::Code,
                    home: 0,
                });
                let n = self.name(&f.id.as_ref().expect("nombre").name);
                self.emit(Op::DeclareFunction(n));
            }
            for n in &var_only {
                let ni = self.name(n);
                self.emit(Op::DeclareVar(ni));
            }
        } else {
            // Eval estricto: var y funciones son bindings propios (estáticos).
            for n in &var_only {
                let t = self.lookup(n);
                self.emit(Op::Undefined);
                self.emit_init(&t);
            }
            for f in &last {
                let idx = self.function(f, CodeKind::Normal, None);
                self.emit(Op::Closure {
                    code: idx,
                    name: FnName::Code,
                    home: 0,
                });
                let t = self.lookup(&f.id.as_ref().expect("nombre").name);
                self.emit_init(&t);
            }
        }
        // Las funciones de los bloques del top-level se instancian al entrar a cada bloque.
        self.clear_lexical_locals(top);
        self.statements(&program.body);
        self.emit(Op::GetLocal(cv));
        self.emit(Op::Return);
        let name = JsString::from(if kind == CodeKind::Script {
            "<script>"
        } else {
            "<eval>"
        });
        let mut code = self.finish_fn(name, program.span, 0);
        code.decls = Some(Box::new(decls));
        Rc::new(code)
    }

    /// Instancia las declaraciones de función de un bloque (BlockDeclarationInstantiation).
    pub(crate) fn instantiate_block_functions(&mut self, body: &[Stmt]) {
        for s in body {
            let f = match s {
                Stmt::Function(f) => Some(&**f),
                other => labelled_fn(other),
            };
            if let Some(f) = f {
                let idx = self.function(f, CodeKind::Normal, None);
                self.emit(Op::Closure {
                    code: idx,
                    name: FnName::Code,
                    home: 0,
                });
                let name = f.id.as_ref().expect("nombre").name.clone();
                let t = self.lookup(&name);
                match t {
                    Target::Dynamic(_) | Target::Global(_) => {
                        let n = self.name(&name);
                        self.emit(Op::InitGlobalLex(n));
                    }
                    t => self.emit_init(&t),
                }
            }
        }
    }

    // --- Funciones ---

    /// Compila una función (declaración, expresión o método) y devuelve el índice de su
    /// constante en la función actual.
    pub(crate) fn function(
        &mut self,
        f: &Function,
        kind: CodeKind,
        name: Option<NameSource>,
    ) -> u32 {
        let fid = self.tree.func(key(f.span, Tag::Function));
        let static_name = match (&f.id, &name) {
            (Some(id), _) => JsString::from(id.name.as_str()),
            (None, Some(NameSource::Const(s))) => s.clone(),
            _ => JsString::default(),
        };
        let code = self.compile_function_body(
            fid,
            kind,
            &f.params,
            FnBodyRef::Block(&f.body.body),
            static_name,
            f.span,
        );
        self.add_const(Constant::Function(code))
    }

    pub(crate) fn compile_function_body(
        &mut self,
        fid: FuncId,
        kind: CodeKind,
        params: &[Pat],
        body: FnBodyRef<'_>,
        name: JsString,
        span: Span,
    ) -> Rc<FunctionCode> {
        let info = self.tree.funcs[fid].clone();
        let outer_class = self.fs.last().map(|s| s.class_depth).unwrap_or(0);
        let mut st = FnState::new(fid, kind, info.strict, info.func_scope, info.num_locals);
        st.class_depth = if matches!(
            kind,
            CodeKind::ClassConstructor { .. } | CodeKind::FieldInit | CodeKind::StaticBlock
        ) {
            1
        } else {
            0
        };
        let _ = outer_class;
        self.fs.push(st);
        self.prologue(&info, params, body.stmts());
        match body {
            FnBodyRef::Block(b) => {
                self.statements(b);
                self.emit(Op::Undefined);
                self.emit(Op::Return);
            }
            FnBodyRef::Expr(e) => {
                if kind == CodeKind::FieldInit {
                    self.expr_named(e, Some(NameSource::FieldName));
                } else {
                    self.expr(e);
                }
                self.emit(Op::Return);
            }
        }
        let length = expected_argument_count(params);
        Rc::new(self.finish_fn(name, span, length))
    }

    /// FunctionDeclarationInstantiation (§10.2.11).
    fn prologue(&mut self, info: &FuncInfo, params: &[Pat], body: &[Stmt]) {
        let fscope = info.func_scope;
        let tree = self.tree;
        if tree.scopes[fscope].materialized {
            let layout = tree.scopes[fscope].layout.clone();
            let c = self.add_const(Constant::Layout(layout));
            if info.needs_function_env() {
                self.emit(Op::PushFunctionScope(c));
            } else {
                self.emit(Op::PushScope(c));
            }
        }
        if info.sloppy_eval {
            self.emit(Op::MarkVarEnv);
        }
        self.st().scope = fscope;
        let pscope = info.params_scope;
        if pscope != fscope {
            self.enter_scope_no_control(pscope);
        }
        // `arguments`.
        if info.arguments != ArgsMode::None {
            let table = if info.arguments == ArgsMode::Mapped {
                let mut names: Vec<Atom> = Vec::new();
                for p in params {
                    if let Pat::Ident(i) = p {
                        names.push(i.name.clone());
                    }
                }
                let mut map: Vec<Option<u32>> = vec![None; names.len()];
                let mut seen: Vec<&Atom> = Vec::new();
                for i in (0..names.len()).rev() {
                    if seen.contains(&&names[i]) {
                        continue;
                    }
                    seen.push(&names[i]);
                    let s = &tree.scopes[pscope];
                    if let Some(bi) = s.find(&names[i])
                        && let Loc::Env(slot) = s.bindings[bi].loc
                    {
                        map[i] = Some(slot);
                    }
                }
                Some(self.add_const(Constant::ParamMap(Rc::new(map))))
            } else {
                None
            };
            self.emit(Op::CreateArguments(table));
            let t = self.lookup("arguments");
            self.emit_init(&t);
        }
        // Los parámetros.
        let mut has_dups = false;
        {
            let mut names = Vec::new();
            for p in params {
                bound_names(p, &mut names);
            }
            for (i, n) in names.iter().enumerate() {
                if names[..i].contains(n) {
                    has_dups = true;
                }
            }
        }
        for (i, p) in params.iter().enumerate() {
            self.pos(p.span().start);
            match p {
                Pat::Rest { argument, .. } => {
                    self.emit(Op::RestArgs(i as u32));
                    self.pattern_binding(argument, BindMode::Param);
                }
                Pat::Ident(id) if has_dups => {
                    self.emit(Op::GetArg(i as u32));
                    let t = self.lookup(&id.name);
                    self.emit_init(&t);
                }
                p => {
                    self.emit(Op::GetArg(i as u32));
                    self.pattern_binding(p, BindMode::Param);
                }
            }
        }
        // Las variables.
        let vscope = info.vars_scope;
        if vscope != pscope {
            self.enter_scope_no_control(vscope);
            if info.sloppy_eval {
                self.emit(Op::MarkVarEnv);
            }
            let vars: Vec<Binding> = tree.scopes[vscope].bindings.clone();
            let mut param_names = Vec::new();
            for p in params {
                bound_names(p, &mut param_names);
            }
            for b in &vars {
                let from_param = (param_names.contains(&b.name)
                    || (b.name == "arguments" && info.arguments != ArgsMode::None))
                    && b.kind != BKind::Function;
                if from_param {
                    // El valor inicial es el del parámetro del mismo nombre (paso 28.f.i.4).
                    let saved = self.st().scope;
                    self.st().scope = pscope;
                    let t = self.lookup(&b.name);
                    self.emit_get(&t);
                    self.st().scope = saved;
                } else {
                    self.emit(Op::Undefined);
                }
                let t = self.lookup(&b.name);
                self.emit_init(&t);
            }
        } else {
            let vars: Vec<Binding> = tree.scopes[vscope].bindings.clone();
            for b in &vars {
                if matches!(b.kind, BKind::Var | BKind::Function) {
                    let t = self.lookup(&b.name);
                    self.emit(Op::Undefined);
                    self.emit_init(&t);
                }
            }
        }
        let lscope = info.lex_scope;
        if lscope != vscope {
            self.enter_scope_no_control(lscope);
        } else {
            self.clear_lexical_locals(lscope);
        }
        // Las declaraciones de función (la última de cada nombre gana).
        let mut funcs: Vec<&Function> = Vec::new();
        let mut dummy = Vec::new();
        for s in body {
            collect_top_level(s, &mut dummy, &mut funcs);
        }
        for f in funcs {
            let idx = self.function(f, CodeKind::Normal, None);
            self.emit(Op::Closure {
                code: idx,
                name: FnName::Code,
                home: 0,
            });
            let t = self.lookup(&f.id.as_ref().expect("nombre").name);
            self.emit_init(&t);
        }
        // Generadores (§15.5.2) y generadores async (§15.6.2): los errores de la instanciación
        // se lanzan antes de crear el objeto. Las funciones async (§15.8.4) no tienen marca:
        // corren desde el principio y sus errores rechazan la promesa.
        if info.is_generator {
            self.emit(Op::GeneratorStart);
            self.emit(Op::Pop);
        }
    }

    /// ¿El código actual es el de un generador async? (E3)
    pub(crate) fn in_async_generator(&self) -> bool {
        let info = &self.tree.funcs[self.sti().func];
        info.is_async && info.is_generator
    }

    /// Como `enter_scope`, pero sin registrar el control (los scopes del prólogo duran toda
    /// la función).
    fn enter_scope_no_control(&mut self, id: ScopeId) {
        let scope = &self.tree.scopes[id];
        if scope.materialized {
            let layout = scope.layout.clone();
            let c = self.add_const(Constant::Layout(layout));
            self.emit(Op::PushScope(c));
        }
        self.st().scope = id;
        self.clear_lexical_locals(id);
    }
}

/// El cuerpo de una función compilada.
#[derive(Clone, Copy)]
pub(crate) enum FnBodyRef<'a> {
    Block(&'a [Stmt]),
    Expr(&'a Expr),
}

impl<'a> FnBodyRef<'a> {
    fn stmts(&self) -> &'a [Stmt] {
        match self {
            FnBodyRef::Block(b) => b,
            FnBodyRef::Expr(_) => &[],
        }
    }
}

/// ExpectedArgumentCount (§15.1.5): los parámetros antes del primero con valor por defecto o
/// rest.
pub(crate) fn expected_argument_count(params: &[Pat]) -> u32 {
    let mut n = 0;
    for p in params {
        match p {
            Pat::Assign { .. } | Pat::Rest { .. } => break,
            _ => n += 1,
        }
    }
    n
}

pub(crate) fn labelled_fn(s: &Stmt) -> Option<&Function> {
    match s {
        Stmt::Labeled { body, .. } => match &**body {
            Stmt::Function(f) => Some(f),
            other => labelled_fn(other),
        },
        _ => None,
    }
}

/// Las declaraciones var-scoped del top-level: nombres de `var` y funciones (en orden).
pub(crate) fn collect_top_level<'a>(
    s: &'a Stmt,
    vars: &mut Vec<Atom>,
    funcs: &mut Vec<&'a Function>,
) {
    match s {
        Stmt::Function(f) => funcs.push(f),
        _ => {
            if let Some(f) = labelled_fn(s) {
                funcs.push(f);
            } else {
                collect_vars_pub(s, vars);
            }
        }
    }
}

fn collect_vars_pub(s: &Stmt, out: &mut Vec<Atom>) {
    // VarDeclaredNames (sin entrar a funciones), igual que en el análisis.
    match s {
        Stmt::Variable(d) if d.kind == VarKind::Var => {
            for dec in &d.declarations {
                bound_names(&dec.id, out);
            }
        }
        Stmt::Block(b) => {
            for s in &b.body {
                collect_vars_pub(s, out);
            }
        }
        Stmt::If {
            consequent,
            alternate,
            ..
        } => {
            collect_vars_pub(consequent, out);
            if let Some(a) = alternate {
                collect_vars_pub(a, out);
            }
        }
        Stmt::While { body, .. }
        | Stmt::DoWhile { body, .. }
        | Stmt::With { body, .. }
        | Stmt::Labeled { body, .. } => collect_vars_pub(body, out),
        Stmt::For { init, body, .. } => {
            if let Some(ForInit::Variable(d)) = init
                && d.kind == VarKind::Var
            {
                for dec in &d.declarations {
                    bound_names(&dec.id, out);
                }
            }
            collect_vars_pub(body, out);
        }
        Stmt::ForIn { left, body, .. } | Stmt::ForOf { left, body, .. } => {
            if let ForHead::Variable(d) = left
                && d.kind == VarKind::Var
            {
                for dec in &d.declarations {
                    bound_names(&dec.id, out);
                }
            }
            collect_vars_pub(body, out);
        }
        Stmt::Try {
            block,
            handler,
            finalizer,
            ..
        } => {
            for s in &block.body {
                collect_vars_pub(s, out);
            }
            if let Some(h) = handler {
                for s in &h.body.body {
                    collect_vars_pub(s, out);
                }
            }
            if let Some(f) = finalizer {
                for s in &f.body {
                    collect_vars_pub(s, out);
                }
            }
        }
        Stmt::Switch { cases, .. } => {
            for c in cases {
                for s in &c.consequent {
                    collect_vars_pub(s, out);
                }
            }
        }
        _ => {}
    }
}

/// Cómo se vincula un patrón.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BindMode {
    /// `var` (PutValue sobre ResolveBinding).
    Var,
    /// `let`/`const`/`class`/`catch`: InitializeBinding.
    Lexical,
    /// Parámetros: InitializeBinding.
    Param,
    /// Asignación por destructuring (las hojas son referencias).
    Assign,
}
