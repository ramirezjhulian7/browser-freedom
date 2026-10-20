//! ClassDefinitionEvaluation (§15.7.14).
//!
//! Orden: el classEnv con el nombre en TDZ y los Private Names nuevos; la herencia (con los
//! nombres privados de la clase ocultos); el constructor F y el prototipo; cada elemento en
//! orden (métodos definidos, campos de instancia agregados a `[[Fields]]`, claves calculadas
//! evaluadas); la inicialización del nombre interno; los métodos privados estáticos; y al
//! final los campos estáticos y los bloques `static`, en orden de aparición.

use std::rc::Rc;

use crate::ast::*;
use crate::compiler::bytecode::*;
use crate::compiler::scope::{BKind, Loc, Tag, key};
use crate::compiler::{Codegen, FnBodyRef, NameSource};
use crate::string::JsString;

enum StaticElem {
    Field {
        key: u32,
        init: Option<u32>,
        private: bool,
    },
    Block(u32),
}

impl<'t> Codegen<'t> {
    /// Evalúa la clase y deja el constructor en la pila.
    pub(crate) fn class(&mut self, c: &Class, name: Option<NameSource>) {
        let parent = self.scope_id();
        let cs = self
            .tree
            .scope(key(c.span, Tag::Class))
            .expect("scope de clase");
        // El nombre para SetFunctionName.
        let (static_name, fname) = match (&c.id, name) {
            (Some(id), _) => (id.name.to_js_string(), FnName::Code),
            (None, Some(NameSource::Const(s))) => (s, FnName::Code),
            (None, Some(NameSource::Stack(_))) => {
                let t = self.alloc_temp();
                self.emit(Op::Dup);
                self.emit(Op::InitLocal(t));
                (JsString::default(), FnName::Local(t))
            }
            (None, Some(NameSource::FieldName)) => {
                let t = self.alloc_temp();
                self.emit(Op::FieldName);
                self.emit(Op::InitLocal(t));
                (JsString::default(), FnName::Local(t))
            }
            (None, None) => (JsString::default(), FnName::Code),
        };
        self.st().class_depth += 1;
        self.enter_scope(cs);
        // Los Private Names (§15.7.14 paso 6).
        let privates: Vec<(JsString, u32)> = self.tree.scopes[cs]
            .bindings
            .iter()
            .filter(|b| b.kind == BKind::Private)
            .filter_map(|b| match b.loc {
                Loc::Env(slot) => Some((b.name.to_js_string(), slot)),
                Loc::Local(_) => None,
            })
            .collect();
        for (desc, slot) in privates {
            let i = self.string(desc);
            self.emit(Op::NewPrivateName(i));
            self.emit(Op::InitEnv(0, slot));
        }
        // La herencia.
        match &c.super_class {
            Some(h) => {
                self.st().private_hidden.push(cs);
                self.expr(h);
                self.st().private_hidden.pop();
                self.emit(Op::ClassHeritage(true));
            }
            None => self.emit(Op::ClassHeritage(false)),
        }
        // El constructor.
        let derived = c.super_class.is_some();
        let ctor = c.body.iter().find_map(|e| match e {
            ClassElement::Method {
                kind: crate::ast::MethodKind::Constructor,
                value,
                ..
            } => Some(value),
            _ => None,
        });
        let code = match ctor {
            Some(f) => {
                let fid = self.tree.func(key(f.span, Tag::Function));
                self.compile_function_body(
                    fid,
                    CodeKind::ClassConstructor { derived },
                    &f.params,
                    FnBodyRef::Block(&f.body.body),
                    static_name.clone(),
                    c.span,
                )
            }
            None => self.default_constructor(derived, static_name.clone(), c.span),
        };
        let idx = self.add_const(Constant::Function(code));
        self.pos(c.span.start);
        self.emit(Op::ClassCreate {
            code: idx,
            name: fname,
        });
        // [F proto]
        let mut statics: Vec<StaticElem> = Vec::new();
        let mut static_privates: Vec<(&Ident, &Function, crate::ast::MethodKind, Span)> =
            Vec::new();
        for e in &c.body {
            match e {
                ClassElement::Method {
                    kind: crate::ast::MethodKind::Constructor,
                    ..
                } => {}
                ClassElement::Method {
                    span,
                    key: k,
                    value,
                    kind,
                    is_static,
                } => {
                    let src = self.method_definition_span(*span, *is_static);
                    let mk = match kind {
                        crate::ast::MethodKind::Get => bytecode::MethodKind::Getter,
                        crate::ast::MethodKind::Set => bytecode::MethodKind::Setter,
                        _ => bytecode::MethodKind::Method,
                    };
                    if let PropKey::Private(id) = k {
                        if *is_static {
                            static_privates.push((id, value, *kind, src));
                            continue;
                        }
                        self.private_name(&id.name);
                        let n = prefixed(kind, &format!("#{}", id.name));
                        self.method_src = Some(src);
                        self.method_closure(value, Some(NameSource::Const(n)), 2);
                        self.emit(Op::AddPrivateMethod(mk));
                        continue;
                    }
                    match Self::static_key(k) {
                        Some(s) if s.to_str().is_some() => {
                            let n = self.name(&s.to_string_lossy());
                            let full = prefixed_js(kind, &s);
                            self.method_src = Some(src);
                            self.method_closure(
                                value,
                                Some(NameSource::Const(full)),
                                if *is_static { 2 } else { 1 },
                            );
                            self.emit(Op::DefineMethod {
                                kind: mk,
                                is_static: *is_static,
                                name: Some(n),
                            });
                        }
                        other => {
                            match other {
                                Some(s) => {
                                    let i = self.string(s);
                                    self.emit(Op::Const(i));
                                }
                                None => {
                                    let PropKey::Computed(ke) = k else {
                                        unreachable!("clave de método")
                                    };
                                    self.expr(ke);
                                    self.emit(Op::ToPropertyKey);
                                }
                            }
                            let prefix = match kind {
                                crate::ast::MethodKind::Get => NamePrefix::Get,
                                crate::ast::MethodKind::Set => NamePrefix::Set,
                                _ => NamePrefix::None,
                            };
                            self.method_src = Some(src);
                            self.method_closure(
                                value,
                                Some(NameSource::Stack(prefix)),
                                if *is_static { 3 } else { 2 },
                            );
                            self.emit(Op::DefineMethod {
                                kind: mk,
                                is_static: *is_static,
                                name: None,
                            });
                        }
                    }
                }
                ClassElement::Property {
                    span,
                    key: k,
                    value,
                    is_static,
                } => {
                    let private = matches!(k, PropKey::Private(_));
                    match k {
                        PropKey::Private(id) => self.private_name(&id.name),
                        PropKey::Computed(ke) => {
                            self.expr(ke);
                            self.emit(Op::ToPropertyKey);
                        }
                        _ => {
                            let s = Self::static_key(k).unwrap_or_default();
                            let i = self.string(s);
                            self.emit(Op::Const(i));
                        }
                    }
                    // [F proto key]
                    let has_init = value.is_some();
                    if let Some(v) = value {
                        let fid = self.tree.func(key(*span, Tag::FieldInit));
                        let code = self.compile_function_body(
                            fid,
                            CodeKind::FieldInit,
                            &[],
                            FnBodyRef::Expr(v),
                            JsString::default(),
                            v.span(),
                        );
                        let ci = self.add_const(Constant::Function(code));
                        self.emit(Op::Closure {
                            code: ci,
                            name: FnName::Code,
                            home: if *is_static { 3 } else { 2 },
                        });
                    }
                    if *is_static {
                        let init = if has_init {
                            let t = self.alloc_temp();
                            self.emit(Op::InitLocal(t));
                            Some(t)
                        } else {
                            None
                        };
                        let kt = self.alloc_temp();
                        self.emit(Op::InitLocal(kt));
                        statics.push(StaticElem::Field {
                            key: kt,
                            init,
                            private,
                        });
                    } else {
                        self.emit(Op::AddField {
                            name: None,
                            private,
                            has_init,
                        });
                    }
                }
                ClassElement::StaticBlock(b) => {
                    let fid = self.tree.func(key(b.span, Tag::StaticBlock));
                    let code = self.compile_function_body(
                        fid,
                        CodeKind::StaticBlock,
                        &[],
                        FnBodyRef::Block(&b.body),
                        JsString::default(),
                        b.span,
                    );
                    let ci = self.add_const(Constant::Function(code));
                    self.emit(Op::Closure {
                        code: ci,
                        name: FnName::Code,
                        home: 2,
                    });
                    let t = self.alloc_temp();
                    self.emit(Op::InitLocal(t));
                    statics.push(StaticElem::Block(t));
                }
            }
        }
        // El binding interno de la clase.
        if let Some(id) = &c.id {
            self.emit(Op::Over);
            let t = self.lookup(&id.name);
            self.emit_init(&t);
        }
        // Métodos privados estáticos (paso 30).
        for (id, f, kind, src) in static_privates {
            let mk = match kind {
                crate::ast::MethodKind::Get => bytecode::MethodKind::Getter,
                crate::ast::MethodKind::Set => bytecode::MethodKind::Setter,
                _ => bytecode::MethodKind::Method,
            };
            self.emit(Op::Over);
            self.private_name(&id.name);
            let n = prefixed(&kind, &format!("#{}", id.name));
            self.method_src = Some(src);
            self.method_closure(f, Some(NameSource::Const(n)), 2);
            self.emit(Op::AddPrivateMethodTo(mk));
            self.emit(Op::Pop);
        }
        // Campos estáticos y bloques (paso 31).
        for s in statics {
            self.emit(Op::Over);
            match s {
                StaticElem::Field { key, init, private } => {
                    self.emit(Op::GetLocal(key));
                    if let Some(i) = init {
                        self.emit(Op::GetLocal(i));
                    }
                    self.emit(Op::DefineStaticField {
                        private,
                        has_init: init.is_some(),
                    });
                }
                StaticElem::Block(t) => {
                    self.emit(Op::GetLocal(t));
                    self.emit(Op::CallStaticBlock);
                }
            }
            self.emit(Op::Pop);
        }
        self.emit(Op::Pop);
        self.exit_scope(cs, parent);
        self.st().class_depth -= 1;
    }

    /// El constructor por defecto (§15.7.14 paso 14).
    fn default_constructor(
        &mut self,
        derived: bool,
        name: JsString,
        span: Span,
    ) -> Rc<FunctionCode> {
        let ops = if derived {
            vec![
                Op::Ctx(CtxWhat::NewTarget, CtxLoc::Frame),
                Op::Ctx(CtxWhat::Function, CtxLoc::Frame),
                Op::SuperConstructor,
                Op::SuperConstructForward,
                Op::SuperCallFinish(CtxLoc::Frame),
                Op::Pop,
                Op::Undefined,
                Op::Return,
            ]
        } else {
            vec![Op::Undefined, Op::Return]
        };
        Rc::new(FunctionCode {
            name,
            kind: CodeKind::ClassConstructor { derived },
            strict: true,
            is_generator: false,
            is_async: false,
            ops,
            consts: Vec::new(),
            num_locals: 0,
            length: 0,
            source: self.source.clone(),
            span,
            positions: vec![(0, span.start)],
            has_prototype: false,
            decls: None,
            dynamic: false,
        })
    }
}

fn prefixed(kind: &crate::ast::MethodKind, name: &str) -> JsString {
    JsString::from(
        match kind {
            crate::ast::MethodKind::Get => format!("get {name}"),
            crate::ast::MethodKind::Set => format!("set {name}"),
            _ => name.to_owned(),
        }
        .as_str(),
    )
}

fn prefixed_js(kind: &crate::ast::MethodKind, name: &JsString) -> JsString {
    let prefix: &str = match kind {
        crate::ast::MethodKind::Get => "get ",
        crate::ast::MethodKind::Set => "set ",
        _ => "",
    };
    let mut units: Vec<u16> = prefix.encode_utf16().collect();
    units.extend_from_slice(name.units());
    JsString::from_units(units)
}

use crate::compiler::bytecode;
