//! Destinos de asignación y patrones de binding: la reinterpretación de una expresión como
//! patrón (ECMA-262 §13.15.5, *Destructuring Assignment*, y las cover grammars de §13.2.5 y
//! §15.3), los early errors de los destinos (`AssignmentTargetType`, §8.6.4) y de los
//! BindingIdentifier (§13.1.1), y el parseo directo de los patrones de binding (§14.3.3).

use std::collections::HashSet;

use super::{Bind, DestructuringErrors, PResult, Parser, STRICT_RESERVED};
use crate::ast::{Expr, Ident, MemberProp, ObjectMember, ObjectPatProp, Pat, PropKind, PropValue};
use crate::lexer::Tok;
use crate::string::Atom;

impl Parser {
    /// Errores que solo valen si la expresión no se reinterpreta como patrón.
    pub fn check_expression_errors(
        &self,
        errs: &DestructuringErrors,
        and_throw: bool,
    ) -> PResult<bool> {
        if !and_throw {
            return Ok(errs.shorthand_assign.is_some() || errs.double_proto.is_some());
        }
        if let Some(p) = errs.shorthand_assign {
            return self.raise(p, "Invalid shorthand property initializer");
        }
        if let Some(p) = errs.double_proto {
            return self.raise(
                p,
                "Duplicate __proto__ fields are not allowed in object literals",
            );
        }
        Ok(false)
    }

    /// Errores que invalidan la reinterpretación como patrón.
    pub fn check_pattern_errors(&self, errs: &DestructuringErrors) -> PResult<()> {
        if let Some(p) = errs.trailing_comma {
            return self.raise(p, "Comma is not permitted after the rest element");
        }
        Ok(())
    }

    /// Reinterpreta `expr` como destino (AssignmentPattern, o patrón de binding si
    /// `binding`).
    pub fn pattern_from_expr(
        &mut self,
        expr: Expr,
        binding: bool,
        errs: Option<&DestructuringErrors>,
    ) -> PResult<Pat> {
        match expr {
            Expr::Ident(id) => {
                if self.in_async() && id.name == "await" {
                    return self.raise(
                        self.lex_pos(id.span.start),
                        "Cannot use 'await' as identifier inside an async function",
                    );
                }
                Ok(Pat::Ident(id))
            }
            Expr::Object { span, properties } => {
                if let Some(e) = errs {
                    self.check_pattern_errors(e)?;
                }
                let n = properties.len();
                let mut out = Vec::with_capacity(n);
                for (i, prop) in properties.into_iter().enumerate() {
                    match prop {
                        ObjectMember::Property {
                            span,
                            key,
                            value,
                            kind,
                            method,
                            shorthand,
                        } => {
                            if kind != PropKind::Init || method {
                                return self.raise(
                                    self.lex_pos(span.start),
                                    "Invalid destructuring assignment target",
                                );
                            }
                            let value = match value {
                                PropValue::Expr(e) => self.pattern_from_expr(e, binding, None)?,
                                PropValue::CoverInit(id, default) => Pat::Assign {
                                    span,
                                    left: Box::new(Pat::Ident(id)),
                                    right: default,
                                },
                                PropValue::Method(_) => {
                                    return self.raise(
                                        self.lex_pos(span.start),
                                        "Invalid destructuring assignment target",
                                    );
                                }
                            };
                            out.push(ObjectPatProp::Property {
                                span,
                                key,
                                value,
                                shorthand,
                            });
                        }
                        ObjectMember::Spread { span, argument } => {
                            if i + 1 != n {
                                return self.raise(
                                    self.lex_pos(span.start),
                                    "Rest element must be last element",
                                );
                            }
                            let arg = self.pattern_from_expr(argument, binding, None)?;
                            // `{...{a}} = x` y `{...[a]} = x` (§13.15.1).
                            if matches!(
                                arg,
                                Pat::Object { .. } | Pat::Array { .. } | Pat::Assign { .. }
                            ) {
                                return self.raise(
                                    self.lex_pos(arg.span().start),
                                    "`...` must be followed by an assignable reference in assignment contexts",
                                );
                            }
                            out.push(ObjectPatProp::Rest {
                                span,
                                argument: arg,
                            });
                        }
                    }
                }
                Ok(Pat::Object {
                    span,
                    properties: out,
                })
            }
            Expr::Array { span, elements } => {
                if let Some(e) = errs {
                    self.check_pattern_errors(e)?;
                }
                let n = elements.len();
                let mut out = Vec::with_capacity(n);
                for (i, el) in elements.into_iter().enumerate() {
                    match el {
                        None => out.push(None),
                        Some(item) => match item.spread {
                            Some(spread) => {
                                if i + 1 != n {
                                    return self.raise(
                                        self.lex_pos(spread.start),
                                        "Rest element must be last element",
                                    );
                                }
                                let arg = self.pattern_from_expr(item.expr, binding, None)?;
                                if matches!(arg, Pat::Assign { .. }) {
                                    return self.raise(
                                        self.lex_pos(arg.span().start),
                                        "Rest elements cannot have a default value",
                                    );
                                }
                                let span = crate::ast::Span::new(spread.start, arg.span().end);
                                out.push(Some(Pat::Rest {
                                    span,
                                    argument: Box::new(arg),
                                }));
                            }
                            None => {
                                out.push(Some(self.pattern_from_expr(item.expr, binding, None)?))
                            }
                        },
                    }
                }
                Ok(Pat::Array {
                    span,
                    elements: out,
                })
            }
            Expr::Assign {
                span,
                operator,
                left,
                right,
            } => {
                if operator != "=" {
                    return self.raise(
                        self.lex_pos(span.start),
                        "Only '=' operator can be used for specifying default value.",
                    );
                }
                Ok(Pat::Assign { span, left, right })
            }
            Expr::Paren { span, expression } => {
                if binding {
                    return self.raise(
                        self.lex_pos(span.start),
                        "Invalid destructuring assignment target",
                    );
                }
                let inner = *expression;
                if matches!(inner, Expr::Object { .. } | Expr::Array { .. }) {
                    return self.raise(
                        self.lex_pos(span.start),
                        "Invalid destructuring assignment target",
                    );
                }
                match self.pattern_from_expr(inner, false, None)? {
                    Pat::Expr(e) => Ok(Pat::Expr(Box::new(Expr::Paren {
                        span,
                        expression: e,
                    }))),
                    Pat::Ident(id) => Ok(Pat::Expr(Box::new(Expr::Paren {
                        span,
                        expression: Box::new(Expr::Ident(id)),
                    }))),
                    _ => self.raise(
                        self.lex_pos(span.start),
                        "Invalid destructuring assignment target",
                    ),
                }
            }
            Expr::Member {
                span,
                optional: false,
                ..
            } if !binding => {
                let _ = span;
                Ok(Pat::Expr(Box::new(expr)))
            }
            Expr::Chain { span, .. } => self.raise(
                self.lex_pos(span.start),
                "Invalid left-hand side in assignment: optional chaining",
            ),
            other => self.raise(
                self.lex_pos(other.span().start),
                if binding {
                    "Invalid destructuring assignment target"
                } else {
                    "Invalid left-hand side in assignment"
                },
            ),
        }
    }

    /// Posición (índice de code point) de un offset UTF-16 de un nodo.
    pub fn lex_pos(&self, offset: u32) -> usize {
        self.lex.pos_of_offset(offset)
    }

    /// ¿Una llamada que fuera de código estricto es un destino `~web-compat~` (B.3.6, Runtime
    /// Errors for Function Call Assignment Targets: un ReferenceError al ejecutar)?
    pub fn is_web_compat_call(&self, expr: &Expr) -> bool {
        if self.strict {
            return false;
        }
        match expr {
            Expr::Call {
                optional: false,
                callee,
                ..
            } => !matches!(**callee, Expr::Super(_)),
            Expr::Paren { expression, .. } => self.is_web_compat_call(expression),
            _ => false,
        }
    }

    /// Un destino simple (`AssignmentTargetType` simple): `++x`, `x += 1`, `x &&= y`. Con
    /// `allow_call`, también una llamada `~web-compat~` (no en las asignaciones lógicas).
    pub fn check_simple_target(&self, expr: &Expr, what: &str, allow_call: bool) -> PResult<()> {
        if allow_call && self.is_web_compat_call(expr) {
            return Ok(());
        }
        match expr {
            Expr::Ident(id) => self.check_strict_assign(&id.name, id.span.start, false),
            Expr::Member {
                optional: false, ..
            } => Ok(()),
            Expr::Paren { expression, .. } => {
                self.check_simple_target(expression, what, allow_call)
            }
            other => self.raise(
                self.lex_pos(other.span().start),
                format!("Invalid left-hand side expression in {what}"),
            ),
        }
    }

    /// `eval`/`arguments` (y las reservadas de código estricto) como destino o binding.
    fn check_strict_assign(&self, name: &Atom, offset: u32, binding: bool) -> PResult<()> {
        if self.strict {
            let n = name.as_str();
            if n == "eval" || n == "arguments" {
                return self.raise(
                    self.lex_pos(offset),
                    if binding {
                        format!("Binding '{n}' in strict mode")
                    } else {
                        "Unexpected eval or arguments in strict mode".to_owned()
                    },
                );
            }
            if STRICT_RESERVED.contains(&n) {
                return self.raise(
                    self.lex_pos(offset),
                    format!("Unexpected strict mode reserved word '{n}'"),
                );
            }
        }
        Ok(())
    }

    /// Verifica (y declara, según `bind`) un patrón. `clashes` detecta nombres repetidos.
    pub fn check_lval_pattern(
        &mut self,
        pat: &Pat,
        bind: Bind,
        clashes: &mut Option<&mut HashSet<Atom>>,
    ) -> PResult<()> {
        match pat {
            Pat::Ident(id) => self.check_lval_ident(id, bind, clashes),
            Pat::Object { properties, .. } => {
                for p in properties {
                    match p {
                        ObjectPatProp::Property { value, .. } => {
                            self.check_lval_pattern(value, bind, clashes)?;
                        }
                        ObjectPatProp::Rest { argument, .. } => {
                            self.check_lval_pattern(argument, bind, clashes)?;
                        }
                    }
                }
                Ok(())
            }
            Pat::Array { elements, .. } => {
                for p in elements.iter().flatten() {
                    self.check_lval_pattern(p, bind, clashes)?;
                }
                Ok(())
            }
            Pat::Rest { argument, .. } => self.check_lval_pattern(argument, bind, clashes),
            Pat::Assign { left, .. } => self.check_lval_pattern(left, bind, clashes),
            Pat::Expr(e) => {
                if bind != Bind::None {
                    return self.raise(
                        self.lex_pos(e.span().start),
                        "Invalid destructuring assignment target",
                    );
                }
                self.check_assign_expr(e)
            }
        }
    }

    fn check_assign_expr(&self, e: &Expr) -> PResult<()> {
        match e {
            Expr::Ident(id) => self.check_strict_assign(&id.name, id.span.start, false),
            Expr::Paren { expression, .. } => self.check_assign_expr(expression),
            _ => Ok(()),
        }
    }

    pub fn check_lval_ident(
        &mut self,
        id: &Ident,
        bind: Bind,
        clashes: &mut Option<&mut HashSet<Atom>>,
    ) -> PResult<()> {
        let binding = bind != Bind::None;
        self.check_strict_assign(&id.name, id.span.start, binding)?;
        if binding {
            let pos = self.lex_pos(id.span.start);
            if bind == Bind::Lexical && id.name == "let" {
                return self.raise(pos, "let is disallowed as a lexically bound name");
            }
            if let Some(set) = clashes
                && !set.insert(id.name.clone())
            {
                return self.raise(pos, "Duplicate parameter name not allowed in this context");
            }
            if bind != Bind::Outside {
                self.declare_name(&id.name, bind, pos)?;
            }
        }
        Ok(())
    }

    // --- Patrones de binding (§14.3.3) ---

    /// BindingIdentifier o BindingPattern.
    pub fn parse_binding_atom(&mut self) -> PResult<Pat> {
        self.check_stack()?;
        if self.is_punct("[") {
            return self.parse_binding_array();
        }
        if self.is_punct("{") {
            return self.parse_binding_object();
        }
        Ok(Pat::Ident(self.parse_ident(false)?))
    }

    fn parse_binding_array(&mut self) -> PResult<Pat> {
        let start = self.tok.start;
        self.next()?;
        let mut elements = Vec::new();
        loop {
            if self.eat_punct("]")? {
                break;
            }
            if self.is_punct(",") {
                self.next()?;
                elements.push(None);
                continue;
            }
            if self.is_punct("...") {
                let rest = self.parse_rest_binding()?;
                elements.push(Some(rest));
                if self.is_punct(",") {
                    return self.raise(
                        self.tok.start,
                        "Comma is not permitted after the rest element",
                    );
                }
                self.expect_punct("]")?;
                break;
            }
            let el = self.parse_binding_element()?;
            elements.push(Some(el));
            if !self.is_punct("]") {
                self.expect_punct(",")?;
            }
        }
        Ok(Pat::Array {
            span: self.span_from(start),
            elements,
        })
    }

    fn parse_binding_object(&mut self) -> PResult<Pat> {
        let start = self.tok.start;
        self.next()?;
        let mut properties = Vec::new();
        let mut first = true;
        while !self.eat_punct("}")? {
            if !first {
                self.expect_punct(",")?;
                if self.eat_punct("}")? {
                    break;
                }
            }
            first = false;
            let pstart = self.tok.start;
            if self.eat_punct("...")? {
                let argument = Pat::Ident(self.parse_ident(false)?);
                if self.is_punct(",") {
                    return self.raise(
                        self.tok.start,
                        "Comma is not permitted after the rest element",
                    );
                }
                properties.push(ObjectPatProp::Rest {
                    span: self.span_from(pstart),
                    argument,
                });
                continue;
            }
            let escaped = self.tok.escaped;
            let (key, computed) = self.parse_property_name()?;
            if self.eat_punct(":")? {
                let value = self.parse_binding_element()?;
                properties.push(ObjectPatProp::Property {
                    span: self.span_from(pstart),
                    key,
                    value,
                    shorthand: false,
                });
                continue;
            }
            // Shorthand: la clave tiene que ser un BindingIdentifier.
            let crate::ast::PropKey::Ident(id) = &key else {
                return self.unexpected();
            };
            if computed || escaped && false {
                return self.unexpected();
            }
            let id = id.clone();
            self.check_unreserved(&id.name, self.lex_pos(id.span.start))?;
            if id.name == "await" && self.await_ident_pos.is_none() {
                self.await_ident_pos = Some(self.lex_pos(id.span.start));
            }
            let value = self.parse_maybe_default(pstart, Pat::Ident(id))?;
            properties.push(ObjectPatProp::Property {
                span: self.span_from(pstart),
                key,
                value,
                shorthand: true,
            });
        }
        Ok(Pat::Object {
            span: self.span_from(start),
            properties,
        })
    }

    /// BindingElement: un patrón con un inicializador opcional.
    pub fn parse_binding_element(&mut self) -> PResult<Pat> {
        let start = self.tok.start;
        let left = self.parse_binding_atom()?;
        self.parse_maybe_default(start, left)
    }

    pub fn parse_maybe_default(&mut self, start: usize, left: Pat) -> PResult<Pat> {
        if !self.eat_punct("=")? {
            return Ok(left);
        }
        let right = self.parse_maybe_assign(false, None)?;
        Ok(Pat::Assign {
            span: self.span_from(start),
            left: Box::new(left),
            right: Box::new(right),
        })
    }

    /// `...BindingIdentifier` o `...BindingPattern`.
    pub fn parse_rest_binding(&mut self) -> PResult<Pat> {
        let start = self.tok.start;
        self.expect_punct("...")?;
        let argument = self.parse_binding_atom()?;
        if self.is_punct("=") {
            return self.raise(
                self.tok.start,
                "Rest parameter may not have a default initializer",
            );
        }
        Ok(Pat::Rest {
            span: self.span_from(start),
            argument: Box::new(argument),
        })
    }

    /// FormalParameters entre paréntesis (después de `(`, hasta `)` incluido).
    pub fn parse_binding_list_params(&mut self) -> PResult<Vec<Pat>> {
        let mut params = Vec::new();
        let mut first = true;
        while !self.eat_punct(")")? {
            if !first {
                self.expect_punct(",")?;
                if self.eat_punct(")")? {
                    break;
                }
            }
            first = false;
            if self.is_punct("...") {
                params.push(self.parse_rest_binding()?);
                if self.is_punct(",") {
                    return self.raise(
                        self.tok.start,
                        "Rest parameter must be last formal parameter",
                    );
                }
                self.expect_punct(")")?;
                break;
            }
            params.push(self.parse_binding_element()?);
        }
        Ok(params)
    }

    /// ¿Un identificador o una palabra (para los casos donde se acepta un nombre)?
    pub fn is_name_token(&self) -> bool {
        matches!(self.tok.tok, Tok::Name(_))
    }
}

/// Los nombres ligados por un patrón, en orden.
pub fn bound_names(pat: &Pat, out: &mut Vec<Ident>) {
    match pat {
        Pat::Ident(id) => out.push(id.clone()),
        Pat::Object { properties, .. } => {
            for p in properties {
                match p {
                    ObjectPatProp::Property { value, .. } => bound_names(value, out),
                    ObjectPatProp::Rest { argument, .. } => bound_names(argument, out),
                }
            }
        }
        Pat::Array { elements, .. } => {
            for p in elements.iter().flatten() {
                bound_names(p, out);
            }
        }
        Pat::Rest { argument, .. } => bound_names(argument, out),
        Pat::Assign { left, .. } => bound_names(left, out),
        Pat::Expr(_) => {}
    }
}

/// ¿Expresión de acceso a un campo privado (`a.#x`, `a?.#x`, entre paréntesis)?
pub fn is_private_field_access(e: &Expr) -> bool {
    match e {
        Expr::Member { property, .. } => matches!(property, MemberProp::Private(_)),
        Expr::Chain { expression, .. } | Expr::Paren { expression, .. } => {
            is_private_field_access(expression)
        }
        _ => false,
    }
}

/// ¿Un identificador, quizá entre paréntesis (`delete x` en estricto)?
pub fn is_local_variable_access(e: &Expr) -> bool {
    match e {
        Expr::Ident(_) => true,
        Expr::Paren { expression, .. } => is_local_variable_access(expression),
        _ => false,
    }
}
