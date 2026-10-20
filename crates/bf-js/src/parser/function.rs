//! Funciones, métodos, flechas y clases (ECMA-262 §15): parámetros, cuerpos con su prólogo de
//! directivas y los early errors de §15.1–§15.9 (parámetros duplicados, `"use strict"` con
//! parámetros no simples, `yield`/`await` en los parámetros), y las clases de §15.7
//! (constructor único, `prototype` estático, accessors, campos, bloques estáticos y nombres
//! privados).

use std::collections::{HashMap, HashSet};

use super::lval::bound_names;
use super::{
    Bind, PResult, Parser, PrivateClass, SCOPE_ARROW, SCOPE_CLASS_FIELD_INIT,
    SCOPE_CLASS_STATIC_BLOCK, SCOPE_DIRECT_SUPER, SCOPE_OUTER_AWAIT, SCOPE_OUTER_YIELD,
    SCOPE_SUPER, function_flags,
};
use crate::ast::{
    Arrow, ArrowBody, BlockStmt, Class, ClassElement, Expr, Function, Ident, MethodKind, Pat,
    PropKey, PropKind,
};
use crate::lexer::Tok;
use crate::string::Atom;

/// Cómo se declara una función.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FnDecl {
    /// Declaración en una lista de sentencias (se declara su nombre).
    Statement { nullable_id: bool },
    /// `l: function f(){}` (B.3.2): se declara, pero no puede ser un generador.
    Labeled,
    /// `if (x) function f(){}` (B.3.3): no se declara en el scope actual.
    Hanging,
}

fn is_simple_params(params: &[Pat]) -> bool {
    params.iter().all(|p| matches!(p, Pat::Ident(_)))
}

impl Parser {
    /// FunctionDeclaration (después de `function`, y de `async` si `is_async`).
    pub fn parse_function_declaration(
        &mut self,
        start: usize,
        decl: FnDecl,
        is_async: bool,
    ) -> PResult<Function> {
        if self.is_punct("*") && matches!(decl, FnDecl::Hanging | FnDecl::Labeled) {
            return self.unexpected();
        }
        let generator = self.eat_punct("*")?;
        let id = match decl {
            FnDecl::Statement { nullable_id: true } if !self.is_name_token() => None,
            _ => Some(self.parse_ident(false)?),
        };
        if let (Some(id), FnDecl::Statement { .. } | FnDecl::Labeled) = (&id, decl) {
            let bind = if self.strict || generator || is_async {
                if self.treat_functions_as_var() {
                    Bind::Var
                } else {
                    Bind::Lexical
                }
            } else {
                Bind::Function
            };
            self.check_lval_ident(id, bind, &mut None)?;
        }
        self.parse_function_rest(start, id, generator, is_async, false)
    }

    /// FunctionExpression (después de `function`).
    pub fn parse_function_expression(
        &mut self,
        start: usize,
        is_async: bool,
        _for_init: bool,
    ) -> PResult<Function> {
        let generator = self.eat_punct("*")?;
        self.parse_function_rest(start, None, generator, is_async, true)
    }

    fn parse_function_rest(
        &mut self,
        start: usize,
        mut id: Option<Ident>,
        generator: bool,
        is_async: bool,
        expression: bool,
    ) -> PResult<Function> {
        let saved = (
            self.yield_pos.take(),
            self.await_pos.take(),
            self.await_ident_pos.take(),
        );
        self.enter_scope(function_flags(is_async, generator));
        // El nombre de una expresión se liga dentro de la función: `function* yield(){}` es
        // un error.
        if expression && self.is_name_token() {
            id = Some(self.parse_ident(false)?);
        }
        self.expect_punct("(")?;
        let params = self.parse_binding_list_params()?;
        self.check_yield_await_in_default_params()?;
        let body = self.parse_function_body(&params, false, false, id.as_ref())?;
        self.exit_scope();
        (self.yield_pos, self.await_pos, self.await_ident_pos) = saved;
        Ok(Function {
            span: self.span_from(start),
            id,
            params,
            body,
            is_generator: generator,
            is_async,
        })
    }

    /// Un método (objeto o clase), desde `(`.
    pub fn parse_method(
        &mut self,
        start: usize,
        generator: bool,
        is_async: bool,
        allow_direct_super: bool,
    ) -> PResult<Function> {
        let saved = (
            self.yield_pos.take(),
            self.await_pos.take(),
            self.await_ident_pos.take(),
        );
        let fn_start = self.tok.start;
        self.enter_scope(
            function_flags(is_async, generator)
                | SCOPE_SUPER
                | if allow_direct_super {
                    SCOPE_DIRECT_SUPER
                } else {
                    0
                },
        );
        self.expect_punct("(")?;
        let params = self.parse_binding_list_params()?;
        self.check_yield_await_in_default_params()?;
        let body = self.parse_function_body(&params, false, true, None)?;
        self.exit_scope();
        (self.yield_pos, self.await_pos, self.await_ident_pos) = saved;
        let _ = start;
        Ok(Function {
            span: self.span_from(fn_start),
            id: None,
            params,
            body,
            is_generator: generator,
            is_async,
        })
    }

    /// ArrowFunction con sus parámetros ya reinterpretados (después de `=>`).
    pub fn parse_arrow_expression(
        &mut self,
        start: usize,
        params: Vec<Pat>,
        is_async: bool,
        for_init: bool,
    ) -> PResult<Expr> {
        let saved = (
            self.yield_pos.take(),
            self.await_pos.take(),
            self.await_ident_pos.take(),
        );
        self.enter_scope(function_flags(is_async, false) | SCOPE_ARROW);
        let body = if self.is_punct("{") {
            ArrowBody::Block(self.parse_function_body(&params, true, false, None)?)
        } else {
            let e = self.parse_maybe_assign(for_init, None)?;
            self.check_params(&params, false)?;
            ArrowBody::Expr(Box::new(e))
        };
        self.exit_scope();
        (self.yield_pos, self.await_pos, self.await_ident_pos) = saved;
        Ok(Expr::Arrow(Box::new(Arrow {
            span: self.span_from(start),
            params,
            body,
            is_async,
        })))
    }

    /// Declara los parámetros en el scope de la función y verifica los nombres;
    /// `allow_duplicates` solo en funciones normales no estrictas con parámetros simples.
    pub fn check_params(&mut self, params: &[Pat], allow_duplicates: bool) -> PResult<()> {
        let mut seen = HashSet::new();
        for p in params {
            let mut clashes = if allow_duplicates {
                None
            } else {
                Some(&mut seen)
            };
            self.check_lval_pattern(p, Bind::Var, &mut clashes)?;
        }
        Ok(())
    }

    /// FunctionBody entre llaves, con su prólogo de directivas, en el scope de los parámetros.
    pub fn parse_function_body(
        &mut self,
        params: &[Pat],
        is_arrow: bool,
        is_method: bool,
        name: Option<&Ident>,
    ) -> PResult<BlockStmt> {
        let start = self.tok.start;
        let old_strict = self.strict;
        let simple = is_simple_params(params);
        // Los parámetros se declaran antes del cuerpo (para los conflictos con las léxicas);
        // lo que depende del modo estricto se verifica al final.
        for p in params {
            let mut names = Vec::new();
            bound_names(p, &mut names);
            for id in names {
                let pos = self.lex_pos(id.span.start);
                self.declare_name(&id.name, Bind::Var, pos)?;
            }
        }
        let old_labels = std::mem::take(&mut self.labels);
        self.expect_punct("{")?;
        let (body, use_strict) = self.parse_statement_list_until_brace(true)?;
        let span = self.span_from(start);
        if use_strict && !simple {
            return self.raise(
                start,
                "Illegal 'use strict' directive in function with non-simple parameter list",
            );
        }
        // Verificación de los nombres con el modo estricto ya conocido.
        let allow_duplicates = !self.strict && !is_arrow && !is_method && simple;
        let mut seen: HashSet<Atom> = HashSet::new();
        for p in params {
            let mut names = Vec::new();
            bound_names(p, &mut names);
            for id in names {
                let mut clashes = if allow_duplicates {
                    None
                } else {
                    Some(&mut seen)
                };
                self.check_lval_ident(&id, Bind::Outside, &mut clashes)?;
            }
            // `yield`/`await`/`let`... como parámetros en estricto ya los rechaza
            // `check_lval_ident` (reservadas en estricto).
        }
        if self.strict
            && let Some(id) = name
        {
            self.check_lval_ident(id, Bind::Outside, &mut None)?;
        }
        self.labels = old_labels;
        self.strict = old_strict;
        Ok(BlockStmt { span, body })
    }

    // --- Clases (§15.7) ---

    /// ClassDeclaration o ClassExpression (el token actual es `class`).
    pub fn parse_class(
        &mut self,
        start: usize,
        statement: bool,
        nullable_id: bool,
    ) -> PResult<Class> {
        self.next()?;
        // Una clase es código estricto (§11.2.2).
        let old_strict = self.strict;
        self.strict = true;
        let id = if self.is_name_token() && !self.is_kw("extends") {
            let id = self.parse_ident(false)?;
            // BindingIdentifier en código estricto (`class eval {}` es un error).
            let bind = if statement {
                Bind::Lexical
            } else {
                Bind::Outside
            };
            self.check_lval_ident(&id, bind, &mut None)?;
            Some(id)
        } else {
            if statement && !nullable_id {
                return self.unexpected();
            }
            None
        };
        let super_class = if self.eat_kw("extends")? {
            let s = self.tok.start;
            let atom = self.parse_expr_atom(None, false, false)?;
            Some(self.parse_subscripts(atom, s, false, false)?)
        } else {
            None
        };
        let body_start = self.tok.start;
        self.private_stack.push(PrivateClass {
            declared: HashMap::new(),
            used: Vec::new(),
        });
        self.expect_punct("{")?;
        let mut body = Vec::new();
        let mut had_constructor = false;
        while !self.is_punct("}") {
            if self.eat_punct(";")? {
                continue;
            }
            let element = self.parse_class_element(super_class.is_some())?;
            match &element {
                ClassElement::Method {
                    kind: MethodKind::Constructor,
                    span,
                    ..
                } => {
                    if had_constructor {
                        return self.raise(
                            self.lex_pos(span.start),
                            "A class may only have one constructor",
                        );
                    }
                    had_constructor = true;
                }
                ClassElement::Method {
                    key: PropKey::Private(name),
                    kind,
                    is_static,
                    ..
                } => {
                    let tag = match (kind, is_static) {
                        (MethodKind::Get, false) => "iget",
                        (MethodKind::Set, false) => "iset",
                        (MethodKind::Get, true) => "sget",
                        (MethodKind::Set, true) => "sset",
                        _ => "true",
                    };
                    self.declare_private(name, tag)?;
                }
                ClassElement::Property {
                    key: PropKey::Private(name),
                    ..
                } => self.declare_private(name, "true")?,
                _ => {}
            }
            body.push(element);
        }
        self.strict = old_strict;
        self.next()?;
        let body_span = self.span_from(body_start);
        // Los nombres privados usados y no declarados pasan a la clase de afuera.
        let class = self.private_stack.pop().expect("clase");
        for (name, pos) in class.used {
            if !class.declared.contains_key(&name) {
                match self.private_stack.last_mut() {
                    Some(parent) => parent.used.push((name, pos)),
                    None => {
                        return self.raise(
                            pos,
                            format!(
                                "Private field '#{name}' must be declared in an enclosing class"
                            ),
                        );
                    }
                }
            }
        }
        Ok(Class {
            span: self.span_from(start),
            id,
            super_class,
            body_span,
            body,
        })
    }

    /// Declara `#name` en la clase actual (solo se repite un par getter/setter del mismo tipo).
    fn declare_private(&mut self, name: &Ident, tag: &'static str) -> PResult<()> {
        let pos = self.lex_pos(name.span.start);
        let class = self.private_stack.last_mut().expect("clase");
        let curr = class.declared.get(&name.name).copied();
        let ok = match (curr, tag) {
            (None, _) => {
                class.declared.insert(name.name.clone(), tag);
                true
            }
            (Some("iget"), "iset")
            | (Some("iset"), "iget")
            | (Some("sget"), "sset")
            | (Some("sset"), "sget") => {
                class.declared.insert(name.name.clone(), "true");
                true
            }
            _ => false,
        };
        if !ok {
            return self.raise(
                pos,
                format!("Identifier '#{}' has already been declared", name.name),
            );
        }
        Ok(())
    }

    /// ¿Puede empezar un ClassElementName?
    fn is_class_element_name_start(&self) -> bool {
        matches!(
            self.tok.tok,
            Tok::Name(_)
                | Tok::Private(_)
                | Tok::Number { .. }
                | Tok::BigInt(_)
                | Tok::String { .. }
        ) || self.is_punct("[")
    }

    fn parse_class_element(&mut self, derived: bool) -> PResult<ClassElement> {
        let start = self.tok.start;
        let mut key_name: Option<(Atom, usize, usize)> = None;
        let mut is_static = false;
        let mut is_generator = false;
        let mut is_async = false;
        let mut kind = MethodKind::Method;
        if self.is_kw("static") {
            let (s, e) = (self.tok.start, self.tok.end);
            self.next()?;
            if self.is_punct("{") {
                return self.parse_static_block(start);
            }
            if self.is_class_element_name_start() || self.is_punct("*") {
                is_static = true;
            } else {
                key_name = Some((Atom::new("static"), s, e));
            }
        }
        if key_name.is_none() && self.is_kw("async") {
            let (s, e) = (self.tok.start, self.tok.end);
            self.next()?;
            if (self.is_class_element_name_start() || self.is_punct("*"))
                && !self.can_insert_semicolon()
            {
                is_async = true;
            } else {
                key_name = Some((Atom::new("async"), s, e));
            }
        }
        if key_name.is_none() && self.is_punct("*") {
            self.next()?;
            is_generator = true;
        }
        if key_name.is_none()
            && !is_async
            && !is_generator
            && (self.is_kw("get") || self.is_kw("set"))
        {
            let word = if self.is_kw("get") { "get" } else { "set" };
            let (s, e) = (self.tok.start, self.tok.end);
            self.next()?;
            if self.is_class_element_name_start() {
                kind = if word == "get" {
                    MethodKind::Get
                } else {
                    MethodKind::Set
                };
            } else {
                key_name = Some((Atom::new(word), s, e));
            }
        }
        let (key, computed) = match key_name {
            Some((name, s, e)) => (
                PropKey::Ident(Ident {
                    span: self.lex.span(s, e),
                    name,
                }),
                false,
            ),
            None => {
                if let Tok::Private(name) = &self.tok.tok {
                    if *name == "constructor" {
                        return self.raise(
                            self.tok.start,
                            "Classes may not have a private field named '#constructor'",
                        );
                    }
                    (PropKey::Private(self.parse_private_ident_decl()?), false)
                } else {
                    self.parse_property_name()?
                }
            }
        };
        let key_pos = match &key {
            PropKey::Ident(i) | PropKey::Private(i) => self.lex_pos(i.span.start),
            PropKey::Literal(l) => self.lex_pos(l.span.start),
            PropKey::Computed(e) => self.lex_pos(e.span().start),
        };
        let named = |n: &str| !computed && key.static_name().as_deref() == Some(n);
        if self.is_punct("(") || kind != MethodKind::Method || is_generator || is_async {
            let is_constructor = !is_static && named("constructor");
            if is_constructor {
                if kind != MethodKind::Method {
                    return self.raise(key_pos, "Class constructor may not be an accessor");
                }
                if is_generator {
                    return self.raise(key_pos, "Class constructor may not be a generator");
                }
                if is_async {
                    return self.raise(key_pos, "Class constructor may not be an async method");
                }
            } else if is_static && named("prototype") {
                return self.raise(
                    key_pos,
                    "Classes may not have a static property named 'prototype'",
                );
            }
            let value =
                self.parse_method(start, is_generator, is_async, is_constructor && derived)?;
            let pkind = match kind {
                MethodKind::Get => PropKind::Get,
                MethodKind::Set => PropKind::Set,
                _ => PropKind::Init,
            };
            self.check_accessor_params(pkind, &value)?;
            return Ok(ClassElement::Method {
                span: self.span_from(start),
                key,
                value: Box::new(value),
                kind: if is_constructor {
                    MethodKind::Constructor
                } else {
                    kind
                },
                is_static,
            });
        }
        // FieldDefinition.
        if named("constructor") {
            return self.raise(key_pos, "Classes may not have a field named 'constructor'");
        }
        if is_static && named("prototype") {
            return self.raise(
                key_pos,
                "Classes may not have a static property named 'prototype'",
            );
        }
        let value = if self.eat_punct("=")? {
            let outer = if self.in_generator() {
                SCOPE_OUTER_YIELD
            } else {
                0
            } | if self.can_await() || self.in_async() {
                SCOPE_OUTER_AWAIT
            } else {
                0
            };
            let saved = (self.yield_pos.take(), self.await_pos.take());
            self.enter_scope(SCOPE_CLASS_FIELD_INIT | SCOPE_SUPER | outer);
            let old_labels = std::mem::take(&mut self.labels);
            let v = self.parse_maybe_assign(false, None)?;
            self.labels = old_labels;
            self.exit_scope();
            (self.yield_pos, self.await_pos) = saved;
            Some(v)
        } else {
            None
        };
        self.semicolon()?;
        Ok(ClassElement::Property {
            span: self.span_from(start),
            key,
            value,
            is_static,
        })
    }

    /// El nombre privado de un elemento (declaración: no se registra como uso).
    fn parse_private_ident_decl(&mut self) -> PResult<Ident> {
        let Tok::Private(name) = self.tok.tok.clone() else {
            return self.unexpected();
        };
        let start = self.tok.start;
        self.next()?;
        Ok(Ident {
            span: self.span_from(start),
            name,
        })
    }

    /// `static { ... }` (§15.7.1, ClassStaticBlock); el token actual es `{`.
    fn parse_static_block(&mut self, start: usize) -> PResult<ClassElement> {
        let block_start = self.tok.start;
        self.next()?;
        let old_labels = std::mem::take(&mut self.labels);
        let saved = (self.yield_pos.take(), self.await_pos.take());
        self.enter_scope(SCOPE_CLASS_STATIC_BLOCK | SCOPE_SUPER);
        let mut body = Vec::new();
        while !self.is_punct("}") {
            body.push(self.parse_statement(None, false)?);
        }
        self.next()?;
        self.exit_scope();
        (self.yield_pos, self.await_pos) = saved;
        self.labels = old_labels;
        let _ = start;
        Ok(ClassElement::StaticBlock(BlockStmt {
            span: self.span_from(block_start),
            body,
        }))
    }
}
