//! Patrones: BindingInitialization (§8.6.2) y DestructuringAssignmentEvaluation (§13.15.5).
//!
//! El valor a desestructurar está en el tope de la pila y se consume. Los arrays usan el
//! protocolo de iteración con un handler de limpieza que cierra el iterador si algo falla
//! (IteratorClose con la completion abrupta) y se cierra normalmente al terminar si no se
//! agotó. En la asignación, cada destino se evalúa (como referencia) antes de obtener su
//! valor, como pide el spec.

use crate::ast::*;
use crate::compiler::bytecode::*;
use crate::compiler::expr::{RefKind, strip_parens};
use crate::compiler::{BindMode, Codegen, NameSource};

/// Una clave ya procesada de un patrón de objeto (para las exclusiones del rest).
enum KeyRef {
    Const(u32),
    Temp(u32),
}

impl<'t> Codegen<'t> {
    /// Vincula el valor del tope con el patrón.
    pub(crate) fn pattern_binding(&mut self, p: &Pat, mode: BindMode) {
        match p {
            Pat::Ident(id) => self.bind_name(&id.name, mode),
            Pat::Assign { left, right, .. } => {
                let l = self.new_label();
                self.emit_to(Op::JumpIfNotUndefined(0), l);
                self.emit(Op::Pop);
                self.default_value(left, right);
                self.bind(l);
                self.pattern_binding(left, mode);
            }
            Pat::Object { properties, .. } => self.object_pattern(properties, mode),
            Pat::Array { elements, .. } => self.array_pattern(elements, mode),
            Pat::Rest { argument, .. } => self.pattern_binding(argument, mode),
            Pat::Expr(e) => {
                // Destino de asignación con el valor ya calculado (for-in/of): la referencia
                // se evalúa ahora y el valor se pone arriba.
                let r = self.emit_ref(e);
                let n = r.operands();
                if n > 0 {
                    self.emit(Op::Pull(n));
                }
                self.emit_ref_put(&r);
                self.emit(Op::Pop);
            }
        }
    }

    /// El valor por defecto (NamedEvaluation si el destino es un identificador).
    fn default_value(&mut self, target: &Pat, init: &Expr) {
        match target {
            Pat::Ident(id) => {
                self.expr_named(init, Some(NameSource::Const(id.name.to_js_string())))
            }
            Pat::Expr(e) => match strip_parens(e) {
                Expr::Ident(id) if !matches!(&**e, Expr::Paren { .. }) => {
                    self.expr_named(init, Some(NameSource::Const(id.name.to_js_string())))
                }
                _ => self.expr(init),
            },
            _ => self.expr(init),
        }
    }

    fn bind_name(&mut self, name: &str, mode: BindMode) {
        match mode {
            BindMode::Var | BindMode::Assign => {
                let t = self.lookup(name);
                self.emit_prepare_set(&t);
                self.emit_set(&t);
                self.emit(Op::Pop);
            }
            BindMode::Lexical => self.emit_init_lexical(name),
            BindMode::Param => {
                let t = self.lookup(name);
                self.emit_init(&t);
            }
        }
    }

    /// Un destino de asignación simple (identificador o expresión), con su default.
    fn assign_target(p: &Pat) -> Option<(&Pat, Option<&Expr>)> {
        match p {
            Pat::Assign { left, right, .. } => match &**left {
                Pat::Ident(_) | Pat::Expr(_) => Some((left, Some(right))),
                _ => None,
            },
            Pat::Ident(_) | Pat::Expr(_) => Some((p, None)),
            _ => None,
        }
    }

    /// Evalúa la referencia de un destino simple (modo asignación).
    fn target_ref(&mut self, target: &Pat) -> RefKind {
        match target {
            Pat::Ident(id) => {
                let t = self.lookup(&id.name);
                self.emit_prepare_set(&t);
                RefKind::Binding(t)
            }
            Pat::Expr(e) => self.emit_ref(e),
            _ => unreachable!("destino simple"),
        }
    }

    /// Con el valor arriba de los operandos de `r`: aplica el default y escribe.
    fn store_target(&mut self, r: &RefKind, target: &Pat, default: Option<&Expr>) {
        if let Some(d) = default {
            let l = self.new_label();
            self.emit_to(Op::JumpIfNotUndefined(0), l);
            self.emit(Op::Pop);
            self.default_value(target, d);
            self.bind(l);
        }
        self.emit_ref_put(r);
        self.emit(Op::Pop);
    }

    fn object_pattern(&mut self, props: &[ObjectPatProp], mode: BindMode) {
        self.emit(Op::CheckCoercible(0));
        let src = self.alloc_temp();
        self.emit(Op::InitLocal(src));
        let mut keys: Vec<KeyRef> = Vec::new();
        for prop in props {
            match prop {
                ObjectPatProp::Property { key: k, value, .. } => {
                    // La clave.
                    let key_ref = match Self::static_key(k) {
                        Some(s) if s.to_str().is_some() => {
                            KeyRef::Const(self.name(&s.to_string_lossy()))
                        }
                        Some(s) => {
                            let t = self.alloc_temp();
                            let i = self.string(s);
                            self.emit(Op::Const(i));
                            self.emit(Op::InitLocal(t));
                            KeyRef::Temp(t)
                        }
                        None => {
                            let PropKey::Computed(e) = k else {
                                unreachable!("clave de patrón")
                            };
                            self.expr(e);
                            self.emit(Op::ToPropertyKey);
                            let t = self.alloc_temp();
                            self.emit(Op::InitLocal(t));
                            KeyRef::Temp(t)
                        }
                    };
                    let simple = if mode == BindMode::Assign {
                        Self::assign_target(value)
                    } else {
                        None
                    };
                    let r = simple.map(|(t, _)| self.target_ref(t));
                    self.emit(Op::GetLocal(src));
                    match &key_ref {
                        KeyRef::Const(n) => self.emit(Op::GetProp(*n)),
                        KeyRef::Temp(t) => {
                            self.emit(Op::GetLocal(*t));
                            self.emit(Op::GetElem);
                        }
                    }
                    match (r, simple) {
                        (Some(r), Some((t, d))) => self.store_target(&r, t, d),
                        _ => self.pattern_binding(value, mode),
                    }
                    keys.push(key_ref);
                }
                ObjectPatProp::Rest { argument, .. } => {
                    let r = if mode == BindMode::Assign {
                        Self::assign_target(argument).map(|(t, _)| self.target_ref(t))
                    } else {
                        None
                    };
                    self.emit(Op::GetLocal(src));
                    for k in &keys {
                        match k {
                            KeyRef::Const(n) => {
                                let Constant::Name(_, key) = &self.sti().consts[*n as usize] else {
                                    unreachable!("constante de nombre")
                                };
                                let s = key.to_js_string().unwrap_or_default();
                                let i = self.string(s);
                                self.emit(Op::Const(i));
                            }
                            KeyRef::Temp(t) => self.emit(Op::GetLocal(*t)),
                        }
                    }
                    self.emit(Op::CopyRest(keys.len() as u32));
                    match r {
                        Some(r) => {
                            self.emit_ref_put(&r);
                            self.emit(Op::Pop);
                        }
                        None => self.pattern_binding(argument, mode),
                    }
                }
            }
        }
        self.free_temp(src);
    }

    fn array_pattern(&mut self, elements: &[Option<Pat>], mode: BindMode) {
        let slot = self.alloc_temp();
        self.alloc_temp();
        self.alloc_temp();
        let ks = self.alloc_temp();
        let vs = self.alloc_temp();
        self.emit(Op::GetIterator(slot));
        let cleanup = self.new_label();
        let end = self.new_label();
        self.emit_to(
            Op::PushFinally {
                target: 0,
                kind_slot: ks,
                value_slot: vs,
            },
            cleanup,
        );
        for el in elements {
            match el {
                None => {
                    self.emit(Op::IterValue(slot));
                    self.emit(Op::Pop);
                }
                Some(Pat::Rest { argument, .. }) => {
                    let r = if mode == BindMode::Assign {
                        Self::assign_target(argument).map(|(t, _)| self.target_ref(t))
                    } else {
                        None
                    };
                    self.emit(Op::IterRest(slot));
                    match r {
                        Some(r) => {
                            self.emit_ref_put(&r);
                            self.emit(Op::Pop);
                        }
                        None => self.pattern_binding(argument, mode),
                    }
                }
                Some(p) => {
                    let simple = if mode == BindMode::Assign {
                        Self::assign_target(p)
                    } else {
                        None
                    };
                    let r = simple.map(|(t, _)| self.target_ref(t));
                    self.emit(Op::IterValue(slot));
                    match (r, simple) {
                        (Some(r), Some((t, d))) => self.store_target(&r, t, d),
                        _ => self.pattern_binding(p, mode),
                    }
                }
            }
        }
        self.emit(Op::PopHandler);
        self.emit(Op::IterClose(slot));
        self.jump(end);
        self.bind(cleanup);
        self.emit(Op::IterCleanup {
            slot,
            kind_slot: ks,
            value_slot: vs,
        });
        self.bind(end);
        self.free_temp(slot);
    }
}
