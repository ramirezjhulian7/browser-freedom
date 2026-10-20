//! Expresiones (ECMA-262 §13): precedencia de operadores, cadenas opcionales, la cover grammar
//! de los paréntesis y las flechas (§13.2.5, §15.3), `new`/`super`/`import`, templates,
//! literales de objeto (con `CoverInitializedName` y `__proto__` duplicado, B.3.1),
//! `yield`/`await` y sus early errors.

use super::lval::{is_local_variable_access, is_private_field_access};
use super::{DestructuringErrors, PResult, Parser};
use crate::ast::{
    Expr, ExprOrSpread, Ident, LitValue, Literal, MemberProp, ObjectMember, Pat, PropKey, PropKind,
    PropValue, Span, Template, TemplateElement,
};
use crate::lexer::Tok;
use crate::string::Atom;

/// Un elemento entre paréntesis antes de saber si es una lista de parámetros.
enum ParenItem {
    Expr(Expr),
    Rest(Pat),
}

impl Parser {
    // --- Expression, AssignmentExpression ---

    /// Expression (con comas). `for_init`: `~In` (no se acepta `in` como operador).
    pub fn parse_expression(
        &mut self,
        for_init: bool,
        mut errs: Option<&mut DestructuringErrors>,
    ) -> PResult<Expr> {
        let start = self.tok.start;
        let expr = self.parse_maybe_assign(for_init, errs.as_deref_mut())?;
        if !self.is_punct(",") {
            return Ok(expr);
        }
        let mut expressions = vec![expr];
        while self.eat_punct(",")? {
            expressions.push(self.parse_maybe_assign(for_init, errs.as_deref_mut())?);
        }
        Ok(Expr::Sequence {
            span: self.span_from(start),
            expressions,
        })
    }

    /// AssignmentExpression.
    pub fn parse_maybe_assign(
        &mut self,
        for_init: bool,
        errs: Option<&mut DestructuringErrors>,
    ) -> PResult<Expr> {
        self.parse_maybe_assign_ex(for_init, errs, false)
    }

    pub fn parse_maybe_assign_ex(
        &mut self,
        for_init: bool,
        errs: Option<&mut DestructuringErrors>,
        for_await: bool,
    ) -> PResult<Expr> {
        self.check_stack()?;
        if self.is_kw("yield") && self.in_generator() {
            return self.parse_yield(for_init);
        }
        let mut own = DestructuringErrors::default();
        let own_errors = errs.is_none();
        let errs: &mut DestructuringErrors = match errs {
            Some(e) => e,
            None => &mut own,
        };
        let old_trailing = errs.trailing_comma.take();
        let old_double_proto = errs.double_proto;
        let start = self.tok.start;
        if self.is_punct("(") || self.is_name_token() {
            self.potential_arrow_at = Some(self.tok.start);
            self.potential_arrow_in_for_await = for_await;
        }
        let left = self.parse_maybe_conditional(for_init, errs)?;
        if let Tok::Punct(op) = self.tok.tok
            && is_assign_op(op)
        {
            let target = if op == "=" && self.is_web_compat_call(&left) {
                Pat::Expr(Box::new(left))
            } else if op == "=" {
                let pat = self.pattern_from_expr(left, false, Some(errs))?;
                if !own_errors {
                    errs.trailing_comma = None;
                    errs.double_proto = None;
                }
                if errs.shorthand_assign.is_some_and(|p| p >= start) {
                    errs.shorthand_assign = None;
                }
                self.check_lval_pattern(&pat, super::Bind::None, &mut None)?;
                pat
            } else {
                // Asignación compuesta y lógica: solo destinos simples.
                self.check_expression_errors(errs, true)?;
                let logical = matches!(op, "&&=" | "||=" | "??=");
                self.check_simple_target(&left, "assignment", !logical)?;
                Pat::Expr(Box::new(left))
            };
            self.next()?;
            let right = self.parse_maybe_assign(for_init, None)?;
            if old_double_proto.is_some() {
                errs.double_proto = old_double_proto;
            }
            return Ok(Expr::Assign {
                span: self.span_from(start),
                operator: op,
                left: Box::new(target),
                right: Box::new(right),
            });
        }
        if own_errors {
            self.check_expression_errors(errs, true)?;
        }
        if old_trailing.is_some() {
            errs.trailing_comma = old_trailing;
        }
        Ok(left)
    }

    /// ConditionalExpression.
    fn parse_maybe_conditional(
        &mut self,
        for_init: bool,
        errs: &mut DestructuringErrors,
    ) -> PResult<Expr> {
        let start = self.tok.start;
        let expr = self.parse_expr_ops(for_init, errs)?;
        if self.check_expression_errors(errs, false)? {
            return Ok(expr);
        }
        // Una flecha es una AssignmentExpression completa.
        if matches!(expr, Expr::Arrow(_)) && self.lex_pos(expr.span().start) == start {
            return Ok(expr);
        }
        if self.eat_punct("?")? {
            let consequent = self.parse_maybe_assign(false, None)?;
            self.expect_punct(":")?;
            let alternate = self.parse_maybe_assign(for_init, None)?;
            return Ok(Expr::Conditional {
                span: self.span_from(start),
                test: Box::new(expr),
                consequent: Box::new(consequent),
                alternate: Box::new(alternate),
            });
        }
        Ok(expr)
    }

    fn parse_expr_ops(&mut self, for_init: bool, errs: &mut DestructuringErrors) -> PResult<Expr> {
        let start = self.tok.start;
        let expr = self.parse_maybe_unary(Some(errs), false, false, for_init)?;
        if self.check_expression_errors(errs, false)? {
            return Ok(expr);
        }
        if matches!(expr, Expr::Arrow(_)) && self.lex_pos(expr.span().start) == start {
            return Ok(expr);
        }
        self.parse_expr_op(expr, start, -1, for_init)
    }

    /// Precedencia del operador binario actual (`??` y `||` tienen la misma).
    fn binop(&self, for_init: bool) -> Option<(i32, &'static str)> {
        match &self.tok.tok {
            Tok::Punct(p) => Some((
                match *p {
                    "??" | "||" => 1,
                    "&&" => 2,
                    "|" => 3,
                    "^" => 4,
                    "&" => 5,
                    "==" | "!=" | "===" | "!==" => 6,
                    "<" | ">" | "<=" | ">=" => 7,
                    "<<" | ">>" | ">>>" => 8,
                    "+" | "-" => 9,
                    "*" | "/" | "%" => 10,
                    _ => return None,
                },
                p,
            )),
            Tok::Name(n) if !self.tok.escaped && *n == "instanceof" => Some((7, "instanceof")),
            Tok::Name(n) if !self.tok.escaped && *n == "in" && !for_init => Some((7, "in")),
            _ => None,
        }
    }

    fn parse_expr_op(
        &mut self,
        left: Expr,
        left_start: usize,
        min_prec: i32,
        for_init: bool,
    ) -> PResult<Expr> {
        let Some((mut prec, op)) = self.binop(for_init) else {
            return Ok(left);
        };
        if prec <= min_prec {
            return Ok(left);
        }
        if matches!(left, Expr::PrivateName(_)) && op != "in" {
            return self.unexpected_at(self.lex_pos(left.span().start));
        }
        let logical = op == "||" || op == "&&";
        let coalesce = op == "??";
        if coalesce {
            // `??` no se mezcla con `||`/`&&`: el lado derecho se parsea con la precedencia
            // de `&&` para detectar la mezcla.
            prec = 2;
        }
        self.next()?;
        let right_start = self.tok.start;
        let right = self.parse_maybe_unary(None, false, false, for_init)?;
        let right = self.parse_expr_op(right, right_start, prec, for_init)?;
        if matches!(right, Expr::PrivateName(_)) {
            return self.raise(
                self.lex_pos(right.span().start),
                "Private identifier can only be left side of binary expression",
            );
        }
        let span = self.span_from(left_start);
        let node = if logical || coalesce {
            Expr::Logical {
                span,
                operator: op,
                left: Box::new(left),
                right: Box::new(right),
            }
        } else {
            Expr::Binary {
                span,
                operator: op,
                left: Box::new(left),
                right: Box::new(right),
            }
        };
        if (logical && self.is_punct("??"))
            || (coalesce && (self.is_punct("||") || self.is_punct("&&")))
        {
            return self.raise(
                self.tok.start,
                "Logical expressions and coalesce expressions cannot be mixed. Wrap either by parentheses",
            );
        }
        self.parse_expr_op(node, left_start, min_prec, for_init)
    }

    /// UnaryExpression, UpdateExpression y ExponentiationExpression.
    pub fn parse_maybe_unary(
        &mut self,
        errs: Option<&mut DestructuringErrors>,
        mut saw_unary: bool,
        inc_dec: bool,
        for_init: bool,
    ) -> PResult<Expr> {
        self.check_stack()?;
        let start = self.tok.start;
        let expr;
        let prefix = match &self.tok.tok {
            Tok::Punct(p @ ("!" | "~" | "+" | "-" | "++" | "--")) => Some(*p),
            Tok::Name(n)
                if !self.tok.escaped && matches!(n.as_str(), "typeof" | "void" | "delete") =>
            {
                Some(match n.as_str() {
                    "typeof" => "typeof",
                    "void" => "void",
                    _ => "delete",
                })
            }
            _ => None,
        };
        if self.is_kw("await") && self.can_await() {
            expr = self.parse_await(for_init)?;
            saw_unary = true;
        } else if let Some(op) = prefix {
            let update = op == "++" || op == "--";
            self.next()?;
            let argument = self.parse_maybe_unary(None, true, update, for_init)?;
            if let Some(e) = errs {
                self.check_expression_errors(e, true)?;
            }
            if update {
                self.check_simple_target(&argument, "prefix operation", true)?;
            } else if op == "delete" && self.strict && is_local_variable_access(&argument) {
                return self.raise(start, "Delete of an unqualified identifier in strict mode.");
            } else if op == "delete" && is_private_field_access(&argument) {
                return self.raise(start, "Private fields can not be deleted");
            } else {
                saw_unary = true;
            }
            let span = self.span_from(start);
            expr = if update {
                Expr::Update {
                    span,
                    operator: op,
                    prefix: true,
                    argument: Box::new(argument),
                }
            } else {
                Expr::Unary {
                    span,
                    operator: op,
                    argument: Box::new(argument),
                }
            };
        } else if !saw_unary && matches!(self.tok.tok, Tok::Private(_)) {
            if for_init || self.private_stack.is_empty() {
                return self.unexpected();
            }
            let id = self.parse_private_ident()?;
            if !self.is_kw("in") {
                return self.unexpected();
            }
            expr = Expr::PrivateName(id);
        } else {
            let mut errs = errs;
            let mut e = self.parse_expr_subscripts(errs.as_deref_mut(), for_init)?;
            if let Some(er) = errs.as_deref()
                && self.check_expression_errors(er, false)?
            {
                return Ok(e);
            }
            while let Tok::Punct(op @ ("++" | "--")) = self.tok.tok {
                if self.can_insert_semicolon() {
                    break;
                }
                self.check_simple_target(&e, "postfix operation", true)?;
                self.next()?;
                e = Expr::Update {
                    span: self.span_from(start),
                    operator: op,
                    prefix: false,
                    argument: Box::new(e),
                };
            }
            expr = e;
        }
        if !inc_dec && self.is_punct("**") {
            if saw_unary {
                return self.raise(
                    self.tok.start,
                    "Unary operator used immediately before exponentiation expression. Parenthesis must be used to disambiguate operator precedence",
                );
            }
            self.next()?;
            let right = self.parse_maybe_unary(None, false, false, for_init)?;
            return Ok(Expr::Binary {
                span: self.span_from(start),
                operator: "**",
                left: Box::new(expr),
                right: Box::new(right),
            });
        }
        Ok(expr)
    }

    // --- LeftHandSideExpression ---

    pub fn parse_expr_subscripts(
        &mut self,
        errs: Option<&mut DestructuringErrors>,
        for_init: bool,
    ) -> PResult<Expr> {
        let start = self.tok.start;
        let mut errs = errs;
        let expr = self.parse_expr_atom(errs.as_deref_mut(), for_init, false)?;
        // El cuerpo de una flecha se parsea hasta el final: no admite subíndices.
        if matches!(expr, Expr::Arrow(_)) {
            return Ok(expr);
        }
        let result = self.parse_subscripts(expr, start, false, for_init)?;
        if let Some(e) = errs
            && matches!(result, Expr::Member { .. })
        {
            let rs = self.lex_pos(result.span().start);
            if e.trailing_comma.is_some_and(|p| p >= rs) {
                e.trailing_comma = None;
            }
        }
        Ok(result)
    }

    pub fn parse_subscripts(
        &mut self,
        mut base: Expr,
        start: usize,
        no_calls: bool,
        for_init: bool,
    ) -> PResult<Expr> {
        let maybe_async_arrow = matches!(&base, Expr::Ident(id) if id.name == "async")
            && self.prev_end == self.lex_pos(base.span().end)
            && !self.can_insert_semicolon()
            && base.span().end - base.span().start == 5
            && self.potential_arrow_at == Some(start);
        let mut optional_chained = false;
        loop {
            let (element, changed, optional) = self.parse_subscript(
                base,
                start,
                no_calls,
                maybe_async_arrow,
                optional_chained,
                for_init,
            )?;
            if optional {
                optional_chained = true;
            }
            if !changed || matches!(element, Expr::Arrow(_)) {
                if optional_chained {
                    return Ok(Expr::Chain {
                        span: self.span_from(start),
                        expression: Box::new(element),
                    });
                }
                return Ok(element);
            }
            base = element;
        }
    }

    /// Un subíndice. Devuelve (expresión, si se agregó algo, si era `?.`).
    fn parse_subscript(
        &mut self,
        base: Expr,
        start: usize,
        no_calls: bool,
        maybe_async_arrow: bool,
        optional_chained: bool,
        for_init: bool,
    ) -> PResult<(Expr, bool, bool)> {
        let optional = self.eat_punct("?.")?;
        if no_calls && optional {
            return self.raise(
                self.prev_start,
                "Invalid optional chain from new expression",
            );
        }
        let computed = self.eat_punct("[")?;
        if computed
            || (optional && !self.is_punct("(") && !matches!(self.tok.tok, Tok::Template { .. }))
            || self.eat_punct(".")?
        {
            let property = if computed {
                let e = self.parse_expression(false, None)?;
                self.expect_punct("]")?;
                MemberProp::Computed(Box::new(e))
            } else if matches!(self.tok.tok, Tok::Private(_)) && !matches!(base, Expr::Super(_)) {
                MemberProp::Private(self.parse_private_ident()?)
            } else {
                MemberProp::Ident(self.parse_ident(true)?)
            };
            let node = Expr::Member {
                span: self.span_from(start),
                object: Box::new(base),
                property,
                optional,
            };
            return Ok((node, true, optional));
        }
        if !no_calls && self.is_punct("(") {
            self.next()?;
            let mut errs = DestructuringErrors::default();
            let old_yield = self.yield_pos.take();
            let old_await = self.await_pos.take();
            let old_await_ident = self.await_ident_pos.take();
            let args = self.parse_expr_list(")", true, false, Some(&mut errs))?;
            if maybe_async_arrow && !optional && !self.can_insert_semicolon() && self.is_punct("=>")
            {
                self.check_pattern_errors(&errs)?;
                self.check_yield_await_in_default_params()?;
                if let Some(p) = self.await_ident_pos {
                    return self.raise(
                        p,
                        "Cannot use 'await' as identifier inside an async function",
                    );
                }
                self.yield_pos = old_yield;
                self.await_pos = old_await;
                self.await_ident_pos = old_await_ident;
                self.next()?;
                let mut params = Vec::new();
                for a in args {
                    let a = a.expect("sin huecos");
                    params.push(self.param_from_arg(a)?);
                }
                let arrow = self.parse_arrow_expression(start, params, true, for_init)?;
                return Ok((arrow, true, false));
            }
            self.check_expression_errors(&errs, true)?;
            self.yield_pos = old_yield.or(self.yield_pos);
            self.await_pos = old_await.or(self.await_pos);
            self.await_ident_pos = old_await_ident.or(self.await_ident_pos);
            let node = Expr::Call {
                span: self.span_from(start),
                callee: Box::new(base),
                arguments: args.into_iter().map(|a| a.expect("sin huecos")).collect(),
                optional,
            };
            return Ok((node, true, optional));
        }
        if matches!(self.tok.tok, Tok::Template { .. }) {
            if optional || optional_chained {
                return self.raise(self.tok.start, "Invalid tagged template on optional chain");
            }
            let quasi = self.parse_template(true)?;
            let node = Expr::TaggedTemplate {
                span: self.span_from(start),
                tag: Box::new(base),
                quasi,
            };
            return Ok((node, true, false));
        }
        Ok((base, false, false))
    }

    /// Un argumento de una lista entre paréntesis convertido en parámetro.
    fn param_from_arg(&mut self, a: ExprOrSpread) -> PResult<Pat> {
        match a.spread {
            Some(spread) => {
                let arg = self.pattern_from_expr(a.expr, true, None)?;
                if matches!(arg, Pat::Assign { .. }) {
                    return self.raise(
                        self.lex_pos(arg.span().start),
                        "Rest parameter may not have a default initializer",
                    );
                }
                Ok(Pat::Rest {
                    span: Span::new(spread.start, arg.span().end),
                    argument: Box::new(arg),
                })
            }
            None => self.pattern_from_expr(a.expr, true, None),
        }
    }

    // --- PrimaryExpression ---

    pub fn parse_expr_atom(
        &mut self,
        errs: Option<&mut DestructuringErrors>,
        for_init: bool,
        for_new: bool,
    ) -> PResult<Expr> {
        let start = self.tok.start;
        let can_be_arrow = self.potential_arrow_at == Some(start);
        match self.tok.tok.clone() {
            Tok::Name(name) if !self.tok.escaped => match name.as_str() {
                "this" => {
                    self.next()?;
                    return Ok(Expr::This(self.span_from(start)));
                }
                "super" => return self.parse_super(start),
                "null" | "true" | "false" => {
                    self.next()?;
                    let value = match name.as_str() {
                        "null" => LitValue::Null,
                        "true" => LitValue::Bool(true),
                        _ => LitValue::Bool(false),
                    };
                    return Ok(Expr::Literal(Literal {
                        span: self.span_from(start),
                        value,
                        raw: name.to_string(),
                    }));
                }
                "function" => {
                    self.next()?;
                    let f = self.parse_function_expression(start, false, for_init)?;
                    return Ok(Expr::Function(Box::new(f)));
                }
                "class" => {
                    let c = self.parse_class(start, false, false)?;
                    return Ok(Expr::Class(Box::new(c)));
                }
                "new" => return self.parse_new(),
                "import" => return self.parse_import_expr(for_new),
                _ => {}
            },
            Tok::Number {
                value,
                legacy_octal,
            } => {
                if legacy_octal && self.strict {
                    return self.raise(start, "Octal literals are not allowed in strict mode.");
                }
                let raw = self.lex.slice(self.tok.start, self.tok.end);
                self.next()?;
                return Ok(Expr::Literal(Literal {
                    span: self.span_from(start),
                    value: LitValue::Number(value),
                    raw,
                }));
            }
            Tok::BigInt(digits) => {
                let raw = self.lex.slice(self.tok.start, self.tok.end);
                self.next()?;
                return Ok(Expr::Literal(Literal {
                    span: self.span_from(start),
                    value: LitValue::BigInt(digits),
                    raw,
                }));
            }
            Tok::String { .. } => return Ok(Expr::Literal(self.parse_string_literal()?)),
            Tok::Template { .. } => return Ok(Expr::Template(self.parse_template(false)?)),
            Tok::Punct("/" | "/=") => {
                let tok = self.tok.clone();
                self.tok = self
                    .lex
                    .rescan_regex(&tok)
                    .map_err(|e| self.error_at(e.pos, e.message))?;
                let Tok::Regex { pattern, flags } = self.tok.tok.clone() else {
                    unreachable!("rescan_regex devuelve un regex");
                };
                if let Err(m) = crate::regexp::validate(&pattern, &flags) {
                    return self.raise(start, m);
                }
                let raw = self.lex.slice(self.tok.start, self.tok.end);
                self.next()?;
                return Ok(Expr::Literal(Literal {
                    span: self.span_from(start),
                    value: LitValue::Regex { pattern, flags },
                    raw,
                }));
            }
            Tok::Punct("(") => return self.parse_paren_and_distinguish(can_be_arrow, for_init),
            Tok::Punct("[") => {
                self.next()?;
                let elements = self.parse_expr_list("]", true, true, errs)?;
                return Ok(Expr::Array {
                    span: self.span_from(start),
                    elements,
                });
            }
            Tok::Punct("{") => return self.parse_object(errs),
            Tok::Name(_) => {}
            _ => return self.unexpected(),
        }
        // IdentifierReference, `async function`, flechas.
        let escaped = self.tok.escaped;
        let id = self.parse_ident(false)?;
        if !escaped && id.name == "async" && !self.can_insert_semicolon() && self.is_kw("function")
        {
            self.next()?;
            let f = self.parse_function_expression(start, true, for_init)?;
            return Ok(Expr::Function(Box::new(f)));
        }
        if can_be_arrow && !self.can_insert_semicolon() {
            if self.is_punct("=>") {
                self.next()?;
                return self.parse_arrow_expression(start, vec![Pat::Ident(id)], false, for_init);
            }
            if !escaped
                && id.name == "async"
                && self.is_name_token()
                && (!self.potential_arrow_in_for_await || !self.is_kw("of"))
            {
                let param = self.parse_ident(false)?;
                if self.can_insert_semicolon() || !self.is_punct("=>") {
                    return self.unexpected();
                }
                self.next()?;
                return self.parse_arrow_expression(start, vec![Pat::Ident(param)], true, for_init);
            }
        }
        Ok(Expr::Ident(id))
    }

    fn parse_super(&mut self, start: usize) -> PResult<Expr> {
        if !self.allow_super() {
            return self.raise(start, "'super' keyword unexpected here");
        }
        self.next()?;
        if self.is_punct("(") && !self.allow_direct_super() {
            return self.raise(start, "'super' keyword unexpected here");
        }
        if !self.is_punct(".") && !self.is_punct("[") && !self.is_punct("(") {
            return self.raise(start, "'super' keyword unexpected here");
        }
        Ok(Expr::Super(self.span_from(start)))
    }

    /// Un literal string (con el early error de los octales en estricto).
    pub fn parse_string_literal(&mut self) -> PResult<Literal> {
        let Tok::String { value, octal } = self.tok.tok.clone() else {
            return self.unexpected();
        };
        if let Some(p) = octal
            && self.strict
        {
            return self.raise(p, "Octal escape sequences are not allowed in strict mode.");
        }
        let start = self.tok.start;
        let raw = self.lex.slice(self.tok.start, self.tok.end);
        self.next()?;
        Ok(Literal {
            span: self.span_from(start),
            value: LitValue::String(value),
            raw,
        })
    }

    /// `( ... )`: expresión entre paréntesis o parámetros de una flecha (§13.2.5).
    fn parse_paren_and_distinguish(&mut self, can_be_arrow: bool, for_init: bool) -> PResult<Expr> {
        let start = self.tok.start;
        self.next()?;
        let mut items: Vec<ParenItem> = Vec::new();
        let mut first = true;
        let mut last_is_comma = false;
        let mut spread_start = None;
        let mut errs = DestructuringErrors::default();
        let old_yield = self.yield_pos.take();
        let old_await = self.await_pos.take();
        // `await_ident_pos` no se reinicia: los `await` en los parámetros se ven igual.
        while !self.is_punct(")") {
            if first {
                first = false;
            } else {
                self.expect_punct(",")?;
            }
            if self.is_punct(")") {
                last_is_comma = true;
                break;
            }
            if self.is_punct("...") {
                spread_start = Some(self.tok.start);
                items.push(ParenItem::Rest(self.parse_rest_binding()?));
                if self.is_punct(",") {
                    return self.raise(
                        self.tok.start,
                        "Comma is not permitted after the rest element",
                    );
                }
                break;
            }
            items.push(ParenItem::Expr(
                self.parse_maybe_assign(false, Some(&mut errs))?,
            ));
        }
        self.expect_punct(")")?;
        if can_be_arrow && !self.can_insert_semicolon() && self.is_punct("=>") {
            self.check_pattern_errors(&errs)?;
            self.check_yield_await_in_default_params()?;
            self.yield_pos = old_yield;
            self.await_pos = old_await;
            self.next()?;
            let mut params = Vec::new();
            for item in items {
                params.push(match item {
                    ParenItem::Expr(e) => self.pattern_from_expr(e, true, None)?,
                    ParenItem::Rest(p) => p,
                });
            }
            return self.parse_arrow_expression(start, params, false, for_init);
        }
        if items.is_empty() || last_is_comma {
            return self.unexpected_at(self.prev_start);
        }
        if let Some(p) = spread_start {
            return self.unexpected_at(p);
        }
        self.check_expression_errors(&errs, true)?;
        self.yield_pos = old_yield.or(self.yield_pos);
        self.await_pos = old_await.or(self.await_pos);
        let mut exprs: Vec<Expr> = items
            .into_iter()
            .map(|i| match i {
                ParenItem::Expr(e) => e,
                ParenItem::Rest(_) => unreachable!("rest ya rechazado"),
            })
            .collect();
        let inner = if exprs.len() > 1 {
            let s = exprs[0].span().start;
            let e = exprs[exprs.len() - 1].span().end;
            Expr::Sequence {
                span: Span::new(s, e),
                expressions: exprs,
            }
        } else {
            exprs.remove(0)
        };
        Ok(Expr::Paren {
            span: self.span_from(start),
            expression: Box::new(inner),
        })
    }

    /// `new.target`, `new X(...)` (§13.3.5).
    fn parse_new(&mut self) -> PResult<Expr> {
        let start = self.tok.start;
        self.next()?;
        if self.is_punct(".") {
            let meta = Ident {
                span: self.span_from(start),
                name: Atom::new("new"),
            };
            self.next()?;
            let escaped = self.tok.escaped;
            let property = self.parse_ident(true)?;
            if property.name != "target" || escaped {
                return self.raise(
                    self.lex_pos(property.span.start),
                    "The only valid meta property for new is 'new.target'",
                );
            }
            if !self.allow_new_target() {
                return self.raise(start, "new.target expression is not allowed here");
            }
            return Ok(Expr::MetaProperty {
                span: self.span_from(start),
                meta,
                property,
            });
        }
        let callee_start = self.tok.start;
        let atom = self.parse_expr_atom(None, false, true)?;
        // `new super()`: SuperCall no es un MemberExpression (§13.3.5).
        if matches!(atom, Expr::Super(_)) && self.is_punct("(") {
            return self.raise(callee_start, "'super' keyword unexpected here");
        }
        let callee = self.parse_subscripts(atom, callee_start, true, false)?;
        let arguments = if self.eat_punct("(")? {
            self.parse_expr_list(")", true, false, None)?
                .into_iter()
                .map(|a| a.expect("sin huecos"))
                .collect()
        } else {
            Vec::new()
        };
        Ok(Expr::New {
            span: self.span_from(start),
            callee: Box::new(callee),
            arguments,
        })
    }

    /// `import(...)` e `import.meta` (§13.3.10, §13.3.12).
    fn parse_import_expr(&mut self, for_new: bool) -> PResult<Expr> {
        let start = self.tok.start;
        let meta = Ident {
            span: self.lex.span(self.tok.start, self.tok.end),
            name: Atom::new("import"),
        };
        self.next()?;
        if self.is_punct("(") && !for_new {
            self.next()?;
            let source = self.parse_maybe_assign(false, None)?;
            let mut options = None;
            if self.eat_punct(",")? && !self.is_punct(")") {
                options = Some(Box::new(self.parse_maybe_assign(false, None)?));
                self.eat_punct(",")?;
            }
            self.expect_punct(")")?;
            return Ok(Expr::Import {
                span: self.span_from(start),
                source: Box::new(source),
                options,
            });
        }
        if self.is_punct(".") {
            self.next()?;
            let escaped = self.tok.escaped;
            let property = self.parse_ident(true)?;
            if property.name != "meta" || escaped {
                return self.raise(
                    self.lex_pos(property.span.start),
                    "The only valid meta property for import is 'import.meta'",
                );
            }
            if !self.module {
                return self.raise(start, "Cannot use 'import.meta' outside a module");
            }
            return Ok(Expr::MetaProperty {
                span: self.span_from(start),
                meta,
                property,
            });
        }
        if for_new {
            return self.raise(start, "Cannot use new with import");
        }
        self.unexpected()
    }

    /// Un TemplateLiteral; el token actual es su primera parte.
    pub fn parse_template(&mut self, tagged: bool) -> PResult<Template> {
        let start = self.tok.start;
        let mut quasis = Vec::new();
        let mut expressions = Vec::new();
        loop {
            let Tok::Template {
                cooked,
                raw,
                tail,
                invalid_escape,
            } = self.tok.tok.clone()
            else {
                return self.unexpected();
            };
            if let Some((pos, msg)) = invalid_escape
                && !tagged
            {
                return self.raise(pos, msg);
            }
            // El elemento sin los delimitadores (`` ` ``/`}` y `` ` ``/`${`).
            let el_start = self.tok.start + 1;
            let el_end = if tail {
                self.tok.end - 1
            } else {
                self.tok.end - 2
            };
            quasis.push(TemplateElement {
                span: self.lex.span(el_start, el_end.max(el_start)),
                tail,
                cooked,
                raw,
            });
            self.next()?;
            if tail {
                break;
            }
            expressions.push(self.parse_expression(false, None)?);
            if !self.is_punct("}") {
                return self.unexpected();
            }
            let close = self.tok.clone();
            self.tok = self
                .lex
                .rescan_template(&close)
                .map_err(|e| self.error_at(e.pos, e.message))?;
        }
        Ok(Template {
            span: self.span_from(start),
            quasis,
            expressions,
        })
    }

    // --- Literales de objeto (§13.2.5) ---

    fn parse_object(&mut self, errs: Option<&mut DestructuringErrors>) -> PResult<Expr> {
        let start = self.tok.start;
        self.next()?;
        let mut properties = Vec::new();
        let mut has_proto = false;
        let mut first = true;
        let mut errs = errs;
        while !self.eat_punct("}")? {
            if !first {
                self.expect_punct(",")?;
                if self.eat_punct("}")? {
                    break;
                }
            }
            first = false;
            let prop = self.parse_property(errs.as_deref_mut())?;
            // `__proto__` duplicado (B.3.1): error salvo en un patrón.
            if let ObjectMember::Property {
                key,
                kind: PropKind::Init,
                method: false,
                shorthand: false,
                ..
            } = &prop
                && !matches!(key, PropKey::Computed(_))
                && key.static_name().as_deref() == Some("__proto__")
            {
                if has_proto {
                    let pos = match key {
                        PropKey::Ident(i) => i.span.start,
                        PropKey::Literal(l) => l.span.start,
                        _ => 0,
                    };
                    let pos = self.lex_pos(pos);
                    match errs.as_deref_mut() {
                        Some(e) => {
                            if e.double_proto.is_none() {
                                e.double_proto = Some(pos);
                            }
                        }
                        None => {
                            return self.raise(
                                pos,
                                "Duplicate __proto__ fields are not allowed in object literals",
                            );
                        }
                    }
                }
                has_proto = true;
            }
            properties.push(prop);
        }
        Ok(Expr::Object {
            span: self.span_from(start),
            properties,
        })
    }

    fn parse_property(&mut self, errs: Option<&mut DestructuringErrors>) -> PResult<ObjectMember> {
        let start = self.tok.start;
        let mut errs = errs;
        if self.eat_punct("...")? {
            let argument = self.parse_maybe_assign(false, errs.as_deref_mut())?;
            if self.is_punct(",")
                && let Some(e) = errs.as_deref_mut()
                && e.trailing_comma.is_none()
            {
                e.trailing_comma = Some(self.tok.start);
            }
            return Ok(ObjectMember::Spread {
                span: self.span_from(start),
                argument,
            });
        }
        let mut is_generator = self.eat_punct("*")?;
        let mut is_async = false;
        let escaped = self.tok.escaped;
        let (mut key, mut computed) = self.parse_property_name()?;
        if !is_generator
            && !escaped
            && !computed
            && matches!(&key, PropKey::Ident(i) if i.name == "async")
            && !self.tok.nl_before
            && (self.is_name_token()
                || matches!(
                    self.tok.tok,
                    Tok::Number { .. } | Tok::BigInt(_) | Tok::String { .. }
                )
                || self.is_punct("[")
                || self.is_punct("*"))
        {
            is_async = true;
            is_generator = self.eat_punct("*")?;
            (key, computed) = self.parse_property_name()?;
        }
        // Valor.
        if (is_generator || is_async) && self.is_punct(":") {
            return self.unexpected();
        }
        if self.eat_punct(":")? {
            let value = self.parse_maybe_assign(false, errs)?;
            return Ok(ObjectMember::Property {
                span: self.span_from(start),
                key,
                value: PropValue::Expr(value),
                kind: PropKind::Init,
                method: false,
                shorthand: false,
            });
        }
        if self.is_punct("(") {
            let f = self.parse_method(start, is_generator, is_async, false)?;
            return Ok(ObjectMember::Property {
                span: self.span_from(start),
                key,
                value: PropValue::Method(Box::new(f)),
                kind: PropKind::Init,
                method: true,
                shorthand: false,
            });
        }
        if !escaped
            && !computed
            && matches!(&key, PropKey::Ident(i) if i.name == "get" || i.name == "set")
            && !self.is_punct(",")
            && !self.is_punct("}")
            && !self.is_punct("=")
        {
            if is_generator || is_async {
                return self.unexpected();
            }
            let kind = if matches!(&key, PropKey::Ident(i) if i.name == "get") {
                PropKind::Get
            } else {
                PropKind::Set
            };
            let (key, _) = self.parse_property_name()?;
            let f = self.parse_method(start, false, false, false)?;
            self.check_accessor_params(kind, &f)?;
            return Ok(ObjectMember::Property {
                span: self.span_from(start),
                key,
                value: PropValue::Method(Box::new(f)),
                kind,
                method: false,
                shorthand: false,
            });
        }
        // Shorthand: `{a}` o `{a = 1}` (solo en patrones).
        let PropKey::Ident(id) = &key else {
            return self.unexpected();
        };
        if computed || is_generator || is_async {
            return self.unexpected();
        }
        let id = id.clone();
        self.check_unreserved(&id.name, self.lex_pos(id.span.start))?;
        if escaped {
            // Un shorthand con escapes no puede ser una palabra reservada (ya verificado).
        }
        if id.name == "await" && self.await_ident_pos.is_none() {
            self.await_ident_pos = Some(self.lex_pos(id.span.start));
        }
        if self.is_punct("=")
            && let Some(e) = errs
        {
            if e.shorthand_assign.is_none() {
                e.shorthand_assign = Some(self.tok.start);
            }
            self.next()?;
            let default = self.parse_maybe_assign(false, None)?;
            return Ok(ObjectMember::Property {
                span: self.span_from(start),
                key,
                value: PropValue::CoverInit(id, Box::new(default)),
                kind: PropKind::Init,
                method: false,
                shorthand: true,
            });
        }
        Ok(ObjectMember::Property {
            span: self.span_from(start),
            key,
            value: PropValue::Expr(Expr::Ident(id)),
            kind: PropKind::Init,
            method: false,
            shorthand: true,
        })
    }

    /// Getters sin parámetros; setters con exactamente uno que no sea rest (§15.4.1).
    pub fn check_accessor_params(&self, kind: PropKind, f: &crate::ast::Function) -> PResult<()> {
        let pos = self.lex_pos(f.span.start);
        match kind {
            PropKind::Get if !f.params.is_empty() => {
                self.raise(pos, "Getter must not have any formal parameters.")
            }
            PropKind::Set if f.params.len() != 1 => {
                self.raise(pos, "Setter must have exactly one formal parameter.")
            }
            PropKind::Set if matches!(f.params[0], Pat::Rest { .. }) => {
                self.raise(pos, "Setter function argument must not be a rest parameter")
            }
            _ => Ok(()),
        }
    }

    /// PropertyName: IdentifierName, string, número o `[expr]`. Devuelve si es computada.
    pub fn parse_property_name(&mut self) -> PResult<(PropKey, bool)> {
        if self.eat_punct("[")? {
            let e = self.parse_maybe_assign(false, None)?;
            self.expect_punct("]")?;
            return Ok((PropKey::Computed(Box::new(e)), true));
        }
        match self.tok.tok.clone() {
            Tok::Number {
                value,
                legacy_octal,
            } => {
                if legacy_octal && self.strict {
                    return self.raise(
                        self.tok.start,
                        "Octal literals are not allowed in strict mode.",
                    );
                }
                let start = self.tok.start;
                let raw = self.lex.slice(self.tok.start, self.tok.end);
                self.next()?;
                Ok((
                    PropKey::Literal(Literal {
                        span: self.span_from(start),
                        value: LitValue::Number(value),
                        raw,
                    }),
                    false,
                ))
            }
            Tok::BigInt(d) => {
                let start = self.tok.start;
                let raw = self.lex.slice(self.tok.start, self.tok.end);
                self.next()?;
                Ok((
                    PropKey::Literal(Literal {
                        span: self.span_from(start),
                        value: LitValue::BigInt(d),
                        raw,
                    }),
                    false,
                ))
            }
            Tok::String { .. } => Ok((PropKey::Literal(self.parse_string_literal()?), false)),
            Tok::Name(_) => Ok((PropKey::Ident(self.parse_ident(true)?), false)),
            _ => self.unexpected(),
        }
    }

    /// `elementos , ... close` (después de la apertura). Con `allow_empty`, huecos (arrays).
    pub fn parse_expr_list(
        &mut self,
        close: &str,
        allow_trailing_comma: bool,
        allow_empty: bool,
        errs: Option<&mut DestructuringErrors>,
    ) -> PResult<Vec<Option<ExprOrSpread>>> {
        let mut out = Vec::new();
        let mut first = true;
        let mut errs = errs;
        while !self.eat_punct(close)? {
            if !first {
                self.expect_punct(",")?;
                if allow_trailing_comma && self.eat_punct(close)? {
                    break;
                }
            }
            first = false;
            if allow_empty && self.is_punct(",") {
                out.push(None);
                continue;
            }
            if self.is_punct("...") {
                let spread = self.lex.span(self.tok.start, self.tok.end);
                self.next()?;
                let expr = self.parse_maybe_assign(false, errs.as_deref_mut())?;
                if self.is_punct(",")
                    && let Some(e) = errs.as_deref_mut()
                    && e.trailing_comma.is_none()
                {
                    e.trailing_comma = Some(self.tok.start);
                }
                out.push(Some(ExprOrSpread {
                    spread: Some(spread),
                    expr,
                }));
                continue;
            }
            let expr = self.parse_maybe_assign(false, errs.as_deref_mut())?;
            out.push(Some(ExprOrSpread { spread: None, expr }));
        }
        Ok(out)
    }

    // --- Identificadores ---

    /// Un identificador. `liberal`: un IdentifierName (palabras reservadas incluidas), para
    /// nombres de propiedades.
    pub fn parse_ident(&mut self, liberal: bool) -> PResult<Ident> {
        let Tok::Name(name) = self.tok.tok.clone() else {
            return self.unexpected();
        };
        let start = self.tok.start;
        if !liberal {
            self.check_unreserved(&name, start)?;
            if name == "await" && self.await_ident_pos.is_none() {
                self.await_ident_pos = Some(start);
            }
        }
        self.next()?;
        Ok(Ident {
            span: self.span_from(start),
            name,
        })
    }

    /// `#nombre`, registrado para verificar que la clase lo declara (§15.7.1).
    pub fn parse_private_ident(&mut self) -> PResult<Ident> {
        let Tok::Private(name) = self.tok.tok.clone() else {
            return self.unexpected();
        };
        let start = self.tok.start;
        self.next()?;
        match self.private_stack.last_mut() {
            Some(class) => class.used.push((name.clone(), start)),
            None => {
                return self.raise(
                    start,
                    format!("Private field '#{name}' must be declared in an enclosing class"),
                );
            }
        }
        Ok(Ident {
            span: self.span_from(start),
            name,
        })
    }

    // --- yield, await ---

    fn parse_yield(&mut self, for_init: bool) -> PResult<Expr> {
        let start = self.tok.start;
        if self.yield_pos.is_none() {
            self.yield_pos = Some(start);
        }
        self.next()?;
        if self.is_punct(";")
            || self.can_insert_semicolon()
            || (!self.is_punct("*") && !self.starts_expr())
        {
            return Ok(Expr::Yield {
                span: self.span_from(start),
                argument: None,
                delegate: false,
            });
        }
        let delegate = self.eat_punct("*")?;
        let argument = self.parse_maybe_assign(for_init, None)?;
        Ok(Expr::Yield {
            span: self.span_from(start),
            argument: Some(Box::new(argument)),
            delegate,
        })
    }

    /// ¿El token actual puede empezar una expresión?
    pub fn starts_expr(&self) -> bool {
        match &self.tok.tok {
            Tok::Eof => false,
            Tok::Punct(p) => matches!(
                *p,
                "(" | "[" | "{" | "+" | "-" | "!" | "~" | "++" | "--" | "/" | "/=" | "..."
            ),
            Tok::Name(n) => {
                self.tok.escaped
                    || !matches!(
                        n.as_str(),
                        "in" | "instanceof"
                            | "case"
                            | "default"
                            | "else"
                            | "catch"
                            | "finally"
                            | "extends"
                            | "while"
                            | "break"
                            | "continue"
                            | "do"
                            | "for"
                            | "if"
                            | "return"
                            | "switch"
                            | "throw"
                            | "try"
                            | "var"
                            | "const"
                            | "with"
                            | "debugger"
                            | "enum"
                            | "export"
                    )
            }
            _ => true,
        }
    }

    fn parse_await(&mut self, for_init: bool) -> PResult<Expr> {
        let start = self.tok.start;
        if self.await_pos.is_none() {
            self.await_pos = Some(start);
        }
        self.next()?;
        let argument = self.parse_maybe_unary(None, true, false, for_init)?;
        Ok(Expr::Await {
            span: self.span_from(start),
            argument: Box::new(argument),
        })
    }

    /// `yield`/`await` en los inicializadores de los parámetros (§15.5.1, §15.8.1).
    pub fn check_yield_await_in_default_params(&self) -> PResult<()> {
        if let Some(y) = self.yield_pos
            && self.await_pos.is_none_or(|a| y < a)
        {
            return self.raise(y, "Yield expression not allowed in formal parameter");
        }
        if let Some(a) = self.await_pos {
            return self.raise(
                a,
                "Illegal await-expression in formal parameters of async function",
            );
        }
        Ok(())
    }
}

fn is_assign_op(op: &str) -> bool {
    matches!(
        op,
        "=" | "+="
            | "-="
            | "*="
            | "/="
            | "%="
            | "**="
            | "<<="
            | ">>="
            | ">>>="
            | "&="
            | "|="
            | "^="
            | "&&="
            | "||="
            | "??="
    )
}
