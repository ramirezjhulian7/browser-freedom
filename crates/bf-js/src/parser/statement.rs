//! Sentencias y declaraciones (ECMA-262 §14 y Annex B.3): listas de sentencias con su prólogo
//! de directivas, `let` contextual (§14.3.1, la restricción de `let [` de §14.5), `using` y
//! `await using` (§14.3.1), los `for` (§14.7.4–5, `for await`, cabeceras léxicas y las
//! excepciones de B.3.5), `switch`, `try`/`catch` (B.3.4), etiquetas, `break`/`continue` y las
//! funciones en bloques y en `if` de Annex B.3.2–3.

use super::function::FnDecl;
use super::{
    Bind, DestructuringErrors, Label, LabelKind, PResult, Parser, SCOPE_SIMPLE_CATCH, SCOPE_SWITCH,
    SCOPE_TOP,
};
use crate::ast::{
    BlockStmt, CatchClause, Expr, ForHead, ForInit, LitValue, Literal, Pat, Stmt, SwitchCase,
    VarKind, VariableDeclaration, VariableDeclarator,
};
use crate::lexer::Tok;

/// El contexto de una sentencia que no admite declaraciones (`if`, cuerpo de un bucle,
/// etiqueta…): las letras son como en acorn (`"if"`, `"label"`, `"do"`…).
type Context = Option<String>;

/// Una directiva del prólogo: su texto crudo y la posición de un escape octal, si tiene.
type Directive = (String, Option<usize>);

impl Parser {
    /// El cuerpo de un Script o Module.
    pub fn parse_top_level(&mut self) -> PResult<Vec<Stmt>> {
        let mut body = Vec::new();
        let mut prologue = true;
        let mut octal_in_prologue = None;
        while self.tok.tok != Tok::Eof {
            let (stmt, directive) = self.parse_directive_or_statement(prologue, true)?;
            if prologue {
                match &directive {
                    Some((raw, octal)) => {
                        if octal.is_some() && octal_in_prologue.is_none() {
                            octal_in_prologue = *octal;
                        }
                        if is_use_strict(raw) {
                            if let Some(p) = octal_in_prologue {
                                return self.raise(
                                    p,
                                    "Octal escape sequences are not allowed in strict mode.",
                                );
                            }
                            self.strict = true;
                        }
                    }
                    None => prologue = false,
                }
            }
            body.push(stmt);
        }
        Ok(body)
    }

    /// Las sentencias hasta `}` (que se consume). Con `directives`, analiza el prólogo y
    /// devuelve si tuvo `"use strict"` (que activa el modo estricto desde ahí).
    pub fn parse_statement_list_until_brace(
        &mut self,
        directives: bool,
    ) -> PResult<(Vec<Stmt>, bool)> {
        let mut body = Vec::new();
        let mut prologue = directives;
        let mut use_strict = false;
        let mut octal_in_prologue = None;
        while !self.is_punct("}") {
            if self.tok.tok == Tok::Eof {
                return self.unexpected();
            }
            let (stmt, directive) = self.parse_directive_or_statement(prologue, false)?;
            if prologue {
                match &directive {
                    Some((raw, octal)) => {
                        if octal.is_some() && octal_in_prologue.is_none() {
                            octal_in_prologue = *octal;
                        }
                        if is_use_strict(raw) {
                            if let Some(p) = octal_in_prologue {
                                return self.raise(
                                    p,
                                    "Octal escape sequences are not allowed in strict mode.",
                                );
                            }
                            use_strict = true;
                            self.strict = true;
                        }
                    }
                    None => prologue = false,
                }
            }
            body.push(stmt);
        }
        self.next()?;
        Ok((body, use_strict))
    }

    /// Una sentencia; en el prólogo, si es una directiva, su texto y si tiene escapes octales.
    fn parse_directive_or_statement(
        &mut self,
        prologue: bool,
        top_level: bool,
    ) -> PResult<(Stmt, Option<Directive>)> {
        let candidate = match (&self.tok.tok, prologue) {
            (Tok::String { octal, .. }, true) => Some((
                self.lex.slice(self.tok.start, self.tok.end),
                *octal,
                self.tok.start,
                self.tok.end,
            )),
            _ => None,
        };
        let mut stmt = self.parse_statement(None, top_level)?;
        if let Some((raw, octal, s, e)) = candidate
            && let Stmt::Expression {
                expression:
                    Expr::Literal(Literal {
                        value: LitValue::String(_),
                        span,
                        ..
                    }),
                directive,
                ..
            } = &mut stmt
            && *span == self.lex.span(s, e)
        {
            let inner = raw[1..raw.len() - 1].to_owned();
            *directive = Some(inner);
            return Ok((stmt, Some((raw, octal))));
        }
        Ok((stmt, None))
    }

    /// ¿`let` empieza una declaración aquí? (§14.3.1 y la restricción de ExpressionStatement.)
    fn is_let(&mut self, context: bool) -> PResult<bool> {
        if !self.is_kw("let") {
            return Ok(false);
        }
        let next = self.peek()?;
        match &next.tok {
            Tok::Punct("[") => Ok(true),
            _ if context => Ok(false),
            Tok::Punct("{") => Ok(true),
            Tok::Name(n) => Ok(next.escaped || (*n != "in" && *n != "instanceof")),
            _ => Ok(false),
        }
    }

    /// `async function` sin LineTerminator en medio.
    fn is_async_function(&mut self) -> PResult<bool> {
        if !self.is_kw("async") {
            return Ok(false);
        }
        let next = self.peek()?;
        Ok(matches!(&next.tok, Tok::Name(n) if *n == "function")
            && !next.escaped
            && !next.nl_before)
    }

    /// `using` seguido de un BindingIdentifier en la misma línea.
    fn is_using(&mut self, for_head: bool) -> PResult<bool> {
        if !self.is_kw("using") {
            return Ok(false);
        }
        let next = self.peek()?;
        if next.nl_before {
            return Ok(false);
        }
        Ok(match &next.tok {
            // `for (using of ...)`: `using` es el destino (§14.7.5, lookahead ≠ using of).
            // `for (using of = ...;;)` sí es una declaración.
            Tok::Name(n) if for_head && *n == "of" && !next.escaped => {
                let (_, third) = self.peek2()?;
                third.tok == Tok::Punct("=")
            }
            Tok::Name(n) => next.escaped || (*n != "in" && *n != "instanceof"),
            _ => false,
        })
    }

    /// `await using x` (las tres palabras en la misma línea).
    fn is_await_using(&mut self) -> PResult<bool> {
        if !self.is_kw("await") || !self.can_await() {
            return Ok(false);
        }
        let (a, b) = self.peek2()?;
        Ok(matches!(&a.tok, Tok::Name(n) if *n == "using")
            && !a.escaped
            && !a.nl_before
            && !b.nl_before
            && matches!(&b.tok, Tok::Name(n) if b.escaped || (*n != "in" && *n != "instanceof")))
    }

    /// `using` solo donde el spec lo permite (§14.3.1.1).
    fn check_using_allowed(&self, pos: usize) -> PResult<()> {
        let scope = self.scope();
        if !self.module && scope.flags & SCOPE_TOP != 0 {
            return self.raise(
                pos,
                "Using declarations are not allowed at the top level of a script",
            );
        }
        if scope.flags & SCOPE_SWITCH != 0 {
            return self.raise(
                pos,
                "Using declarations are not allowed directly in a case clause",
            );
        }
        Ok(())
    }

    /// Statement o Declaration. `context` no vacío: solo una Statement (sin declaraciones).
    pub fn parse_statement(&mut self, context: Option<&str>, top_level: bool) -> PResult<Stmt> {
        self.check_stack()?;
        let start = self.tok.start;
        let ctx = context.map(str::to_owned);
        if self.is_let(context.is_some())? {
            if context.is_some() {
                return self.unexpected();
            }
            self.next()?;
            let decl = self.parse_var(start, VarKind::Let, false)?;
            self.semicolon()?;
            return Ok(Stmt::Variable(decl));
        }
        let Tok::Name(word) = self.tok.tok.clone() else {
            return self.parse_statement_punct(start, ctx);
        };
        if self.tok.escaped {
            return self.parse_expression_or_labeled(start, ctx);
        }
        match word.as_str() {
            "break" | "continue" => self.parse_break_continue(start, word.as_str() == "break"),
            "debugger" => {
                self.next()?;
                self.semicolon()?;
                Ok(Stmt::Debugger(self.span_from(start)))
            }
            "do" => self.parse_do_while(start),
            "for" => self.parse_for(start),
            "function" => {
                // Una función como cuerpo de un `if` o de una etiqueta (B.3.3, B.3.2), solo
                // fuera de código estricto.
                if let Some(c) = context
                    && (self.strict || (c != "if" && c != "label"))
                {
                    return self.unexpected();
                }
                self.next()?;
                let decl = match context {
                    Some("if") => FnDecl::Hanging,
                    Some(_) => FnDecl::Labeled,
                    None => FnDecl::Statement { nullable_id: false },
                };
                let f = self.parse_function_declaration(start, decl, false)?;
                Ok(Stmt::Function(Box::new(f)))
            }
            "class" => {
                if context.is_some() {
                    return self.unexpected();
                }
                let c = self.parse_class(start, true, false)?;
                Ok(Stmt::Class(Box::new(c)))
            }
            "if" => self.parse_if(start),
            "return" => {
                if !self.in_function() {
                    return self.raise(start, "Illegal return statement");
                }
                self.next()?;
                let argument = if self.eat_punct(";")? || self.can_insert_semicolon() {
                    None
                } else {
                    let e = self.parse_expression(false, None)?;
                    self.semicolon()?;
                    Some(e)
                };
                Ok(Stmt::Return {
                    span: self.span_from(start),
                    argument,
                })
            }
            "switch" => self.parse_switch(start),
            "throw" => {
                self.next()?;
                if self.tok.nl_before {
                    return self.raise(self.prev_end, "Illegal newline after throw");
                }
                let argument = self.parse_expression(false, None)?;
                self.semicolon()?;
                Ok(Stmt::Throw {
                    span: self.span_from(start),
                    argument,
                })
            }
            "try" => self.parse_try(start),
            "const" | "var" => {
                let kind = if word.as_str() == "var" {
                    VarKind::Var
                } else {
                    VarKind::Const
                };
                if context.is_some() && kind != VarKind::Var {
                    return self.unexpected();
                }
                self.next()?;
                let decl = self.parse_var(start, kind, false)?;
                self.semicolon()?;
                Ok(Stmt::Variable(decl))
            }
            "while" => {
                self.next()?;
                let test = self.parse_paren_expression()?;
                self.labels.push(Label {
                    name: None,
                    kind: LabelKind::Loop,
                    statement_start: 0,
                });
                let body = self.parse_statement(Some("while"), false)?;
                self.labels.pop();
                Ok(Stmt::While {
                    span: self.span_from(start),
                    test,
                    body: Box::new(body),
                })
            }
            "with" => {
                if self.strict {
                    return self.raise(start, "Strict mode code may not include a with statement");
                }
                self.next()?;
                let object = self.parse_paren_expression()?;
                let body = self.parse_statement(Some("with"), false)?;
                Ok(Stmt::With {
                    span: self.span_from(start),
                    object,
                    body: Box::new(body),
                })
            }
            "import" => {
                let next = self.peek()?;
                if matches!(next.tok, Tok::Punct("(" | ".")) {
                    return self.parse_expression_or_labeled(start, ctx);
                }
                if !top_level || !self.module {
                    return self.raise(start, "Cannot use import statement outside a module");
                }
                self.parse_import(start)
            }
            "export" => {
                if !top_level || !self.module {
                    return self.raise(start, "Unexpected token 'export'");
                }
                self.parse_export(start)
            }
            _ => {
                if self.is_async_function()? {
                    if context.is_some() {
                        return self.unexpected();
                    }
                    self.next()?;
                    self.next()?;
                    let f = self.parse_function_declaration(
                        start,
                        FnDecl::Statement { nullable_id: false },
                        true,
                    )?;
                    return Ok(Stmt::Function(Box::new(f)));
                }
                if context.is_none() && self.is_using(false)? {
                    self.check_using_allowed(start)?;
                    self.next()?;
                    let decl = self.parse_var(start, VarKind::Using, false)?;
                    self.semicolon()?;
                    return Ok(Stmt::Variable(decl));
                }
                if context.is_none() && self.is_await_using()? {
                    self.check_using_allowed(start)?;
                    self.next()?;
                    self.next()?;
                    let decl = self.parse_var(start, VarKind::AwaitUsing, false)?;
                    self.semicolon()?;
                    return Ok(Stmt::Variable(decl));
                }
                self.parse_expression_or_labeled(start, ctx)
            }
        }
    }

    fn parse_statement_punct(&mut self, start: usize, ctx: Context) -> PResult<Stmt> {
        if self.is_punct("{") {
            return Ok(Stmt::Block(self.parse_block(true)?));
        }
        if self.is_punct(";") {
            self.next()?;
            return Ok(Stmt::Empty(self.span_from(start)));
        }
        self.parse_expression_or_labeled(start, ctx)
    }

    fn parse_expression_or_labeled(&mut self, start: usize, ctx: Context) -> PResult<Stmt> {
        let is_name = matches!(self.tok.tok, Tok::Name(_));
        let expr = self.parse_expression(false, None)?;
        if is_name
            && let Expr::Ident(id) = &expr
            && self.is_punct(":")
        {
            let id = id.clone();
            self.next()?;
            return self.parse_labeled(start, id, ctx);
        }
        self.semicolon()?;
        Ok(Stmt::Expression {
            span: self.span_from(start),
            expression: expr,
            directive: None,
        })
    }

    fn parse_labeled(
        &mut self,
        start: usize,
        label: crate::ast::Ident,
        ctx: Context,
    ) -> PResult<Stmt> {
        if self
            .labels
            .iter()
            .any(|l| l.name.as_ref() == Some(&label.name))
        {
            return self.raise(
                self.lex_pos(label.span.start),
                format!("Label '{}' has already been declared", label.name),
            );
        }
        let kind = if self.is_kw("for") || self.is_kw("while") || self.is_kw("do") {
            LabelKind::Loop
        } else if self.is_kw("switch") {
            LabelKind::Switch
        } else {
            LabelKind::Other
        };
        for l in self.labels.iter_mut().rev() {
            if l.statement_start == start {
                l.statement_start = self.tok.start;
                l.kind = kind;
            } else {
                break;
            }
        }
        self.labels.push(Label {
            name: Some(label.name.clone()),
            kind,
            statement_start: self.tok.start,
        });
        let context = match &ctx {
            Some(c) if c.contains("label") => c.clone(),
            Some(c) => format!("{c}label"),
            None => "label".to_owned(),
        };
        let body = self.parse_statement(Some(&context), false)?;
        self.labels.pop();
        Ok(Stmt::Labeled {
            span: self.span_from(start),
            label,
            body: Box::new(body),
        })
    }

    fn parse_break_continue(&mut self, start: usize, is_break: bool) -> PResult<Stmt> {
        self.next()?;
        let label = if self.eat_punct(";")? || self.can_insert_semicolon() {
            None
        } else if matches!(self.tok.tok, Tok::Name(_)) {
            let l = self.parse_ident(false)?;
            self.semicolon()?;
            Some(l)
        } else {
            return self.unexpected();
        };
        let ok = self.labels.iter().any(|lab| match &label {
            None => lab.kind != LabelKind::Other && (is_break || lab.kind == LabelKind::Loop),
            Some(l) => {
                lab.name.as_ref() == Some(&l.name) && (is_break || lab.kind == LabelKind::Loop)
            }
        });
        if !ok {
            let word = if is_break { "break" } else { "continue" };
            return self.raise(
                start,
                match &label {
                    Some(l) if is_break => format!("Undefined label '{}'", l.name),
                    Some(l) => format!(
                        "Illegal continue statement: '{}' does not denote an iteration statement",
                        l.name
                    ),
                    None => format!("Illegal {word} statement"),
                },
            );
        }
        let span = self.span_from(start);
        Ok(if is_break {
            Stmt::Break { span, label }
        } else {
            Stmt::Continue { span, label }
        })
    }

    fn parse_do_while(&mut self, start: usize) -> PResult<Stmt> {
        self.next()?;
        self.labels.push(Label {
            name: None,
            kind: LabelKind::Loop,
            statement_start: 0,
        });
        let body = self.parse_statement(Some("do"), false)?;
        self.labels.pop();
        self.expect_kw("while")?;
        let test = self.parse_paren_expression()?;
        // `;` opcional después de do-while (§12.10.1, regla del `)`).
        self.eat_punct(";")?;
        Ok(Stmt::DoWhile {
            span: self.span_from(start),
            body: Box::new(body),
            test,
        })
    }

    fn parse_if(&mut self, start: usize) -> PResult<Stmt> {
        self.next()?;
        let test = self.parse_paren_expression()?;
        let consequent = self.parse_statement(Some("if"), false)?;
        let alternate = if self.eat_kw("else")? {
            Some(Box::new(self.parse_statement(Some("if"), false)?))
        } else {
            None
        };
        Ok(Stmt::If {
            span: self.span_from(start),
            test,
            consequent: Box::new(consequent),
            alternate,
        })
    }

    pub fn parse_paren_expression(&mut self) -> PResult<Expr> {
        self.expect_punct("(")?;
        let e = self.parse_expression(false, None)?;
        self.expect_punct(")")?;
        Ok(e)
    }

    fn parse_switch(&mut self, start: usize) -> PResult<Stmt> {
        self.next()?;
        let discriminant = self.parse_paren_expression()?;
        self.expect_punct("{")?;
        self.labels.push(Label {
            name: None,
            kind: LabelKind::Switch,
            statement_start: 0,
        });
        self.enter_scope(SCOPE_SWITCH);
        let mut cases: Vec<SwitchCase> = Vec::new();
        let mut saw_default = false;
        while !self.is_punct("}") {
            if self.is_kw("case") || self.is_kw("default") {
                let case_start = self.tok.start;
                let is_case = self.is_kw("case");
                self.next()?;
                let test = if is_case {
                    Some(self.parse_expression(false, None)?)
                } else {
                    if saw_default {
                        return self.raise(
                            case_start,
                            "More than one default clause in switch statement",
                        );
                    }
                    saw_default = true;
                    None
                };
                self.expect_punct(":")?;
                cases.push(SwitchCase {
                    span: self.span_from(case_start),
                    test,
                    consequent: Vec::new(),
                });
            } else {
                let Some(cur) = cases.len().checked_sub(1) else {
                    return self.unexpected();
                };
                let stmt = self.parse_statement(None, false)?;
                cases[cur].consequent.push(stmt);
                cases[cur].span = self.span_from(self.lex_pos(cases[cur].span.start));
            }
        }
        self.next()?;
        self.exit_scope();
        self.labels.pop();
        Ok(Stmt::Switch {
            span: self.span_from(start),
            discriminant,
            cases,
        })
    }

    fn parse_try(&mut self, start: usize) -> PResult<Stmt> {
        self.next()?;
        let block = self.parse_block(true)?;
        let mut handler = None;
        if self.is_kw("catch") {
            let cstart = self.tok.start;
            self.next()?;
            let param = if self.eat_punct("(")? {
                let param = self.parse_binding_atom()?;
                let simple = matches!(param, Pat::Ident(_));
                self.enter_scope(if simple { SCOPE_SIMPLE_CATCH } else { 0 });
                self.check_lval_pattern(
                    &param,
                    if simple {
                        Bind::SimpleCatch
                    } else {
                        Bind::Lexical
                    },
                    &mut Some(&mut std::collections::HashSet::new()),
                )?;
                self.expect_punct(")")?;
                Some(param)
            } else {
                self.enter_scope(0);
                None
            };
            // El bloque del catch comparte el scope del parámetro (B.3.4).
            let bstart = self.tok.start;
            self.expect_punct("{")?;
            let (body, _) = self.parse_statement_list_until_brace(false)?;
            let body = BlockStmt {
                span: self.span_from(bstart),
                body,
            };
            self.exit_scope();
            handler = Some(CatchClause {
                span: self.span_from(cstart),
                param,
                body,
            });
        }
        let finalizer = if self.eat_kw("finally")? {
            Some(self.parse_block(true)?)
        } else {
            None
        };
        if handler.is_none() && finalizer.is_none() {
            return self.raise(start, "Missing catch or finally after try");
        }
        Ok(Stmt::Try {
            span: self.span_from(start),
            block,
            handler,
            finalizer,
        })
    }

    /// Block (`{` actual).
    pub fn parse_block(&mut self, new_scope: bool) -> PResult<BlockStmt> {
        let start = self.tok.start;
        self.expect_punct("{")?;
        if new_scope {
            self.enter_scope(0);
        }
        let (body, _) = self.parse_statement_list_until_brace(false)?;
        if new_scope {
            self.exit_scope();
        }
        Ok(BlockStmt {
            span: self.span_from(start),
            body,
        })
    }

    // --- Declaraciones de variables ---

    /// La lista de declaradores (después de `var`/`let`/`const`/`using`). `is_for`: en una
    /// cabecera de `for` (`~In`, y el inicializador puede faltar antes de `in`/`of`).
    pub fn parse_var(
        &mut self,
        start: usize,
        kind: VarKind,
        is_for: bool,
    ) -> PResult<VariableDeclaration> {
        let mut decls = Vec::new();
        loop {
            let dstart = self.tok.start;
            let id = if matches!(kind, VarKind::Using | VarKind::AwaitUsing) {
                // Sin patrones (BindingList[~Pattern]).
                if !self.is_name_token() {
                    return self.unexpected();
                }
                Pat::Ident(self.parse_ident(false)?)
            } else {
                self.parse_binding_atom()?
            };
            let bind = if kind == VarKind::Var {
                Bind::Var
            } else {
                Bind::Lexical
            };
            self.check_lval_pattern(&id, bind, &mut None)?;
            let for_in_of = is_for && (self.is_kw("in") || self.is_kw("of"));
            let init = if self.eat_punct("=")? {
                Some(self.parse_maybe_assign(is_for, None)?)
            } else {
                let needs_init =
                    matches!(kind, VarKind::Const | VarKind::Using | VarKind::AwaitUsing)
                        || !matches!(id, Pat::Ident(_));
                if needs_init && !for_in_of {
                    return if matches!(id, Pat::Ident(_)) {
                        self.raise(
                            self.tok.start,
                            format!("Missing initializer in {} declaration", kind.as_str()),
                        )
                    } else {
                        self.raise(
                            self.tok.start,
                            "Missing initializer in destructuring declaration",
                        )
                    };
                }
                None
            };
            decls.push(VariableDeclarator {
                span: self.span_from(dstart),
                id,
                init,
            });
            if !self.eat_punct(",")? {
                break;
            }
        }
        Ok(VariableDeclaration {
            span: self.span_from(start),
            kind,
            declarations: decls,
        })
    }

    // --- for (§14.7.4, §14.7.5) ---

    fn parse_for(&mut self, start: usize) -> PResult<Stmt> {
        self.next()?;
        let await_at = if self.can_await() && self.is_kw("await") {
            let p = self.tok.start;
            self.next()?;
            Some(p)
        } else {
            None
        };
        self.labels.push(Label {
            name: None,
            kind: LabelKind::Loop,
            statement_start: 0,
        });
        self.enter_scope(0);
        self.expect_punct("(")?;
        if self.is_punct(";") {
            if let Some(p) = await_at {
                return self.unexpected_at(p);
            }
            return self.parse_for_rest(start, None);
        }
        let is_let = self.is_let(false)?;
        let decl_kind = if self.is_kw("var") {
            Some(VarKind::Var)
        } else if self.is_kw("const") {
            Some(VarKind::Const)
        } else if is_let {
            Some(VarKind::Let)
        } else if self.is_using(true)? {
            Some(VarKind::Using)
        } else if self.is_await_using()? {
            Some(VarKind::AwaitUsing)
        } else {
            None
        };
        if let Some(kind) = decl_kind {
            let init_start = self.tok.start;
            self.next()?;
            if kind == VarKind::AwaitUsing {
                self.next()?;
            }
            let init = self.parse_var(init_start, kind, true)?;
            if (self.is_kw("in") || self.is_kw("of")) && init.declarations.len() == 1 {
                let is_in = self.is_kw("in");
                if is_in {
                    if let Some(p) = await_at {
                        return self.unexpected_at(p);
                    }
                    if matches!(kind, VarKind::Using | VarKind::AwaitUsing) {
                        return self
                            .raise(init_start, "Using declarations are not allowed in for-in");
                    }
                }
                return self.parse_for_in(start, ForHead::Variable(init), await_at.is_some());
            }
            if let Some(p) = await_at {
                return self.unexpected_at(p);
            }
            return self.parse_for_rest(start, Some(ForInit::Variable(init)));
        }
        let starts_with_let = self.is_kw("let");
        let init_start = self.tok.start;
        let escaped = self.tok.escaped;
        let mut errs = DestructuringErrors::default();
        let init = if await_at.is_some() {
            self.parse_lhs_for_await(&mut errs)?
        } else {
            self.parse_expression_for_init(&mut errs)?
        };
        let is_of = self.is_kw("of");
        if self.is_kw("in") || is_of {
            if let Some(p) = await_at {
                if self.is_kw("in") {
                    return self.unexpected_at(p);
                }
            } else if is_of
                && !escaped
                && self.lex_pos(init.span().start) == init_start
                && matches!(&init, Expr::Ident(i) if i.name == "async")
            {
                // `for (async of x)` (§14.7.5: lookahead ≠ async of).
                return self.raise(
                    init_start,
                    "The left-hand side of a for-of loop may not be 'async'.",
                );
            }
            if starts_with_let && is_of {
                return self.raise(
                    init_start,
                    "The left-hand side of a for-of loop may not start with let.",
                );
            }
            // El destino es un LeftHandSideExpression, no una asignación (`for (a = 0 in o)`).
            if matches!(init, Expr::Assign { .. }) {
                return self.raise(init_start, "Invalid left-hand side in for-loop");
            }
            let pat = if self.is_web_compat_call(&init) {
                Pat::Expr(Box::new(init))
            } else {
                self.pattern_from_expr(init, false, Some(&errs))?
            };
            self.check_lval_pattern(&pat, Bind::None, &mut None)?;
            return self.parse_for_in(start, ForHead::Pattern(pat), await_at.is_some());
        }
        self.check_expression_errors(&errs, true)?;
        if let Some(p) = await_at {
            return self.unexpected_at(p);
        }
        self.parse_for_rest(start, Some(ForInit::Expr(init)))
    }

    fn parse_expression_for_init(&mut self, errs: &mut DestructuringErrors) -> PResult<Expr> {
        self.parse_expression(true, Some(errs))
    }

    /// El LeftHandSideExpression de un `for await (... of`.
    fn parse_lhs_for_await(&mut self, errs: &mut DestructuringErrors) -> PResult<Expr> {
        self.potential_arrow_at = Some(self.tok.start);
        self.potential_arrow_in_for_await = true;
        self.parse_expr_subscripts(Some(errs), true)
    }

    /// `for (init; test; update) body` (después de `init`).
    fn parse_for_rest(&mut self, start: usize, init: Option<ForInit>) -> PResult<Stmt> {
        self.expect_punct(";")?;
        let test = if self.is_punct(";") {
            None
        } else {
            Some(self.parse_expression(false, None)?)
        };
        self.expect_punct(";")?;
        let update = if self.is_punct(")") {
            None
        } else {
            Some(self.parse_expression(false, None)?)
        };
        self.expect_punct(")")?;
        let body = self.parse_statement(Some("for"), false)?;
        self.exit_scope();
        self.labels.pop();
        Ok(Stmt::For {
            span: self.span_from(start),
            init,
            test,
            update,
            body: Box::new(body),
        })
    }

    /// `for (left in/of right) body` (el token actual es `in` u `of`).
    fn parse_for_in(&mut self, start: usize, left: ForHead, is_await: bool) -> PResult<Stmt> {
        let is_in = self.is_kw("in");
        if let ForHead::Variable(v) = &left
            && let Some(d) = v.declarations.first()
            && d.init.is_some()
        {
            // B.3.5: `for (var x = 1 in o)` en código no estricto con un identificador.
            let allowed =
                is_in && !self.strict && v.kind == VarKind::Var && matches!(d.id, Pat::Ident(_));
            if !allowed {
                return self.raise(
                    self.lex_pos(v.span.start),
                    format!(
                        "{} loop variable declaration may not have an initializer.",
                        if is_in { "for-in" } else { "for-of" }
                    ),
                );
            }
        }
        self.next()?;
        let right = if is_in {
            self.parse_expression(false, None)?
        } else {
            self.parse_maybe_assign(false, None)?
        };
        self.expect_punct(")")?;
        let body = self.parse_statement(Some("for"), false)?;
        self.exit_scope();
        self.labels.pop();
        let span = self.span_from(start);
        Ok(if is_in {
            Stmt::ForIn {
                span,
                left,
                right,
                body: Box::new(body),
            }
        } else {
            Stmt::ForOf {
                span,
                left,
                right,
                body: Box::new(body),
                is_await,
            }
        })
    }
}

fn is_use_strict(raw: &str) -> bool {
    raw == "\"use strict\"" || raw == "'use strict'"
}
