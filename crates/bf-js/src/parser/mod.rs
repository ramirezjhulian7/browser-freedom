//! Parser de ECMAScript (ECMA-262 §13–16 y Annex B): descendente recursivo, con precedencia
//! para los binarios, las cover grammars de §13.2.5 y §13.3, ASI (§12.10) y los early errors de
//! cada producción.
//!
//! Los parámetros gramaticales `[Yield, Await, In, Return]` se modelan con la pila de
//! [`Scope`]s (función, generador, async, flecha, bloque estático, inicializador de campo) y el
//! argumento `for_init` (`~In`). Las cover grammars se parsean como expresiones y se
//! reinterpretan como patrones ([`lval`]); lo que solo es válido en un patrón (`{a = 1}`, una
//! coma después de un rest, `__proto__` duplicado) se registra en [`DestructuringErrors`] y es
//! error si la expresión no termina reinterpretada.
//!
//! El modo estricto se activa al encontrar la directiva `"use strict"`; lo que depende de él y
//! se parseó antes (el nombre y los parámetros de la función, los octales de las directivas
//! anteriores) se verifica al cerrar la función.

mod expression;
mod function;
mod lval;
mod module;
mod statement;

use std::collections::{HashMap, HashSet};

use crate::ast::{Program, SourceType, Span};
use crate::lexer::{Lexer, Tok, Token};
use crate::string::Atom;

#[derive(Debug, Clone, Default)]
pub struct ParseOptions {
    /// Empezar en código estricto (como si el script empezara con `"use strict"`).
    pub strict: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub message: String,
    pub span: Span,
    pub line: u32,
    pub column: u32,
}

impl std::fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SyntaxError: {} ({}:{})", self.message, self.line, self.column)
    }
}

impl std::error::Error for SyntaxError {}

pub(crate) type PResult<T> = Result<T, SyntaxError>;

// Flags de los scopes (como en acorn).
pub(crate) const SCOPE_TOP: u32 = 1;
pub(crate) const SCOPE_FUNCTION: u32 = 2;
pub(crate) const SCOPE_ASYNC: u32 = 4;
pub(crate) const SCOPE_GENERATOR: u32 = 8;
pub(crate) const SCOPE_ARROW: u32 = 16;
pub(crate) const SCOPE_SIMPLE_CATCH: u32 = 32;
pub(crate) const SCOPE_SUPER: u32 = 64;
pub(crate) const SCOPE_DIRECT_SUPER: u32 = 128;
pub(crate) const SCOPE_CLASS_STATIC_BLOCK: u32 = 256;
pub(crate) const SCOPE_CLASS_FIELD_INIT: u32 = 512;
/// En un inicializador de campo: `yield`/`await` del contexto externo no son identificadores.
pub(crate) const SCOPE_OUTER_YIELD: u32 = 1024;
pub(crate) const SCOPE_OUTER_AWAIT: u32 = 2048;
/// El bloque de un `switch` (para `using` en las cláusulas).
pub(crate) const SCOPE_SWITCH: u32 = 4096;
pub(crate) const SCOPE_VAR: u32 =
    SCOPE_TOP | SCOPE_FUNCTION | SCOPE_CLASS_STATIC_BLOCK | SCOPE_CLASS_FIELD_INIT;

pub(crate) fn function_flags(is_async: bool, generator: bool) -> u32 {
    SCOPE_FUNCTION
        | if is_async { SCOPE_ASYNC } else { 0 }
        | if generator { SCOPE_GENERATOR } else { 0 }
}

#[derive(Debug, Default)]
pub(crate) struct Scope {
    pub flags: u32,
    pub var: HashSet<Atom>,
    pub lexical: HashSet<Atom>,
    pub functions: HashSet<Atom>,
    /// El nombre del parámetro de un `catch (e)` simple.
    pub simple_catch: Option<Atom>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bind {
    /// Solo verificación (sin declarar): destinos de asignación.
    None,
    Var,
    Lexical,
    Function,
    SimpleCatch,
    /// El nombre de una función ya declarado afuera (solo verificar).
    Outside,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LabelKind {
    Loop,
    Switch,
    Other,
}

#[derive(Debug, Clone)]
pub(crate) struct Label {
    pub name: Option<Atom>,
    pub kind: LabelKind,
    pub statement_start: usize,
}

/// Lo que solo es válido si la expresión se reinterpreta como patrón (posiciones).
#[derive(Debug, Clone, Default)]
pub(crate) struct DestructuringErrors {
    pub shorthand_assign: Option<usize>,
    pub trailing_comma: Option<usize>,
    pub double_proto: Option<usize>,
}

pub(crate) struct PrivateClass {
    /// Nombre → `"iget"`, `"iset"`, `"sget"`, `"sset"` o `"true"`.
    pub declared: HashMap<Atom, &'static str>,
    pub used: Vec<(Atom, usize)>,
}

/// Palabras reservadas siempre (§12.7.2), salvo `await`/`yield`/`let`/`static`, que dependen
/// del contexto.
pub(crate) const KEYWORDS: &[&str] = &[
    "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete",
    "do", "else", "enum", "export", "extends", "false", "finally", "for", "function", "if",
    "import", "in", "instanceof", "new", "null", "return", "super", "switch", "this", "throw",
    "true", "try", "typeof", "var", "void", "while", "with",
];

/// Reservadas además en código estricto.
pub(crate) const STRICT_RESERVED: &[&str] = &[
    "implements",
    "interface",
    "let",
    "package",
    "private",
    "protected",
    "public",
    "static",
    "yield",
];

pub(crate) struct Parser {
    pub lex: Lexer,
    pub tok: Token,
    pub prev_start: usize,
    pub prev_end: usize,
    pub module: bool,
    pub strict: bool,
    pub scopes: Vec<Scope>,
    pub labels: Vec<Label>,
    pub yield_pos: Option<usize>,
    pub await_pos: Option<usize>,
    pub await_ident_pos: Option<usize>,
    pub potential_arrow_at: Option<usize>,
    pub potential_arrow_in_for_await: bool,
    pub private_stack: Vec<PrivateClass>,
    pub exports: HashSet<String>,
    pub undefined_exports: Vec<(Atom, usize)>,
}

impl Parser {
    pub fn new(src: &str, module: bool, opts: &ParseOptions) -> PResult<Parser> {
        let lex = Lexer::new(src, module);
        let mut p = Parser {
            lex,
            tok: Token {
                tok: Tok::Eof,
                start: 0,
                end: 0,
                nl_before: false,
                escaped: false,
            },
            prev_start: 0,
            prev_end: 0,
            module,
            strict: module || opts.strict,
            scopes: Vec::new(),
            labels: Vec::new(),
            yield_pos: None,
            await_pos: None,
            await_ident_pos: None,
            potential_arrow_at: None,
            potential_arrow_in_for_await: false,
            private_stack: Vec::new(),
            exports: HashSet::new(),
            undefined_exports: Vec::new(),
        };
        p.tok = p.lex.next_token().map_err(|e| p.lex_error(e))?;
        Ok(p)
    }

    pub fn parse_program(mut self) -> PResult<Program> {
        self.enter_scope(SCOPE_TOP);
        let body = self.parse_top_level()?;
        if let Some((name, pos)) = self.undefined_exports.first().cloned() {
            return self.raise(pos, format!("Export '{name}' is not defined"));
        }
        self.exit_scope();
        Ok(Program {
            span: self.lex.span(0, self.lex.len()),
            source_type: if self.module {
                SourceType::Module
            } else {
                SourceType::Script
            },
            body,
        })
    }

    // --- Errores ---

    fn lex_error(&self, e: crate::lexer::LexError) -> SyntaxError {
        self.error_at(e.pos, e.message)
    }

    pub fn error_at(&self, pos: usize, message: impl Into<String>) -> SyntaxError {
        let (line, column) = self.lex.line_col(pos);
        SyntaxError {
            message: message.into(),
            span: self.lex.span(pos, (pos + 1).min(self.lex.len())),
            line,
            column,
        }
    }

    pub fn raise<T>(&self, pos: usize, message: impl Into<String>) -> PResult<T> {
        Err(self.error_at(pos, message))
    }

    pub fn unexpected<T>(&self) -> PResult<T> {
        self.unexpected_at(self.tok.start)
    }

    pub fn unexpected_at<T>(&self, pos: usize) -> PResult<T> {
        if pos == self.tok.start && self.tok.tok == Tok::Eof {
            return self.raise(pos, "Unexpected end of input");
        }
        let text = if pos == self.tok.start {
            self.lex.slice(self.tok.start, self.tok.end)
        } else {
            String::new()
        };
        if text.is_empty() {
            self.raise(pos, "Unexpected token")
        } else {
            self.raise(pos, format!("Unexpected token '{text}'"))
        }
    }

    // --- Tokens ---

    pub fn next(&mut self) -> PResult<()> {
        self.prev_start = self.tok.start;
        self.prev_end = self.tok.end;
        self.tok = self.lex.next_token().map_err(|e| self.lex_error(e))?;
        Ok(())
    }

    /// El token siguiente al actual, sin consumir nada.
    pub fn peek(&mut self) -> PResult<Token> {
        let save = self.lex.position();
        let t = self.lex.next_token().map_err(|e| self.lex_error(e));
        self.lex.set_position(save);
        t
    }

    /// Los dos tokens siguientes al actual.
    pub fn peek2(&mut self) -> PResult<(Token, Token)> {
        let save = self.lex.position();
        let a = self.lex.next_token().map_err(|e| self.lex_error(e));
        let b = self.lex.next_token().map_err(|e| self.lex_error(e));
        self.lex.set_position(save);
        Ok((a?, b?))
    }

    pub fn is_punct(&self, p: &str) -> bool {
        matches!(self.tok.tok, Tok::Punct(q) if q == p)
    }

    pub fn eat_punct(&mut self, p: &str) -> PResult<bool> {
        if self.is_punct(p) {
            self.next()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn expect_punct(&mut self, p: &str) -> PResult<()> {
        if self.eat_punct(p)? {
            Ok(())
        } else {
            self.unexpected()
        }
    }

    /// ¿El token es la palabra `w` sin escapes?
    pub fn is_kw(&self, w: &str) -> bool {
        matches!(&self.tok.tok, Tok::Name(n) if *n == w) && !self.tok.escaped
    }

    pub fn eat_kw(&mut self, w: &str) -> PResult<bool> {
        if self.is_kw(w) {
            self.next()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn expect_kw(&mut self, w: &str) -> PResult<()> {
        if self.eat_kw(w)? {
            Ok(())
        } else {
            self.unexpected()
        }
    }

    pub fn tok_name(&self) -> Option<&Atom> {
        match &self.tok.tok {
            Tok::Name(n) => Some(n),
            _ => None,
        }
    }

    /// ¿Una palabra reservada (palabra clave sin escapes)?
    pub fn is_keyword_token(&self) -> bool {
        self.tok_name()
            .is_some_and(|n| !self.tok.escaped && KEYWORDS.contains(&n.as_str()))
    }

    pub fn can_insert_semicolon(&self) -> bool {
        self.tok.tok == Tok::Eof || self.is_punct("}") || self.tok.nl_before
    }

    /// `;` o una inserción automática (§12.10).
    pub fn semicolon(&mut self) -> PResult<()> {
        if !self.eat_punct(";")? && !self.can_insert_semicolon() {
            return self.unexpected();
        }
        Ok(())
    }

    pub fn span_from(&self, start: usize) -> Span {
        self.lex.span(start, self.prev_end.max(start))
    }

    // --- Scopes (como acorn: `declareName`) ---

    pub fn enter_scope(&mut self, flags: u32) {
        self.scopes.push(Scope {
            flags,
            ..Scope::default()
        });
    }

    pub fn exit_scope(&mut self) {
        self.scopes.pop();
    }

    pub fn scope(&self) -> &Scope {
        self.scopes.last().expect("scope")
    }

    /// ¿Las declaraciones de función de este scope se tratan como `var` (cuerpo de función o
    /// top-level de un script)?
    fn treat_functions_as_var_in(&self, scope: &Scope) -> bool {
        scope.flags & SCOPE_FUNCTION != 0 || (!self.module && scope.flags & SCOPE_TOP != 0)
    }

    pub fn treat_functions_as_var(&self) -> bool {
        self.treat_functions_as_var_in(self.scope())
    }

    pub fn declare_name(&mut self, name: &Atom, bind: Bind, pos: usize) -> PResult<()> {
        let mut redeclared = false;
        match bind {
            Bind::Lexical => {
                let s = self.scope();
                redeclared = s.lexical.contains(name)
                    || s.functions.contains(name)
                    || s.var.contains(name);
                let top = s.flags & SCOPE_TOP != 0;
                self.scopes.last_mut().expect("scope").lexical.insert(name.clone());
                if self.module && top {
                    self.undefined_exports.retain(|(n, _)| n != name);
                }
            }
            Bind::SimpleCatch => {
                let s = self.scopes.last_mut().expect("scope");
                s.lexical.insert(name.clone());
                s.simple_catch = Some(name.clone());
            }
            Bind::Function => {
                let s = self.scope();
                redeclared = if self.treat_functions_as_var() {
                    s.lexical.contains(name)
                } else {
                    s.lexical.contains(name) || s.var.contains(name)
                };
                self.scopes
                    .last_mut()
                    .expect("scope")
                    .functions
                    .insert(name.clone());
            }
            Bind::Var => {
                for i in (0..self.scopes.len()).rev() {
                    let s = &self.scopes[i];
                    let catch_exception = s.flags & SCOPE_SIMPLE_CATCH != 0
                        && s.simple_catch.as_ref() == Some(name);
                    if (s.lexical.contains(name) && !catch_exception)
                        || (!self.treat_functions_as_var_in(s) && s.functions.contains(name))
                    {
                        redeclared = true;
                        break;
                    }
                    let flags = s.flags;
                    self.scopes[i].var.insert(name.clone());
                    if self.module && flags & SCOPE_TOP != 0 {
                        self.undefined_exports.retain(|(n, _)| n != name);
                    }
                    if flags & SCOPE_VAR != 0 {
                        break;
                    }
                }
            }
            Bind::None | Bind::Outside => {}
        }
        if redeclared {
            return self.raise(pos, format!("Identifier '{name}' has already been declared"));
        }
        Ok(())
    }

    fn var_scope(&self) -> &Scope {
        self.scopes
            .iter()
            .rev()
            .find(|s| s.flags & SCOPE_VAR != 0)
            .expect("scope var")
    }

    fn this_scope(&self) -> &Scope {
        self.scopes
            .iter()
            .rev()
            .find(|s| s.flags & SCOPE_VAR != 0 && s.flags & SCOPE_ARROW == 0)
            .expect("scope this")
    }

    pub fn in_function(&self) -> bool {
        self.var_scope().flags & SCOPE_FUNCTION != 0
    }

    pub fn in_generator(&self) -> bool {
        self.var_scope().flags & SCOPE_GENERATOR != 0
    }

    pub fn in_async(&self) -> bool {
        self.var_scope().flags & SCOPE_ASYNC != 0
    }

    pub fn can_await(&self) -> bool {
        for s in self.scopes.iter().rev() {
            if s.flags & (SCOPE_CLASS_STATIC_BLOCK | SCOPE_CLASS_FIELD_INIT) != 0 {
                return false;
            }
            if s.flags & SCOPE_FUNCTION != 0 {
                return s.flags & SCOPE_ASYNC != 0;
            }
        }
        // Top-level await en módulos.
        self.module
    }

    pub fn allow_super(&self) -> bool {
        self.this_scope().flags & (SCOPE_SUPER | SCOPE_CLASS_FIELD_INIT) != 0
    }

    pub fn allow_direct_super(&self) -> bool {
        self.this_scope().flags & SCOPE_DIRECT_SUPER != 0
    }

    pub fn allow_new_target(&self) -> bool {
        self.this_scope().flags
            & (SCOPE_FUNCTION | SCOPE_CLASS_STATIC_BLOCK | SCOPE_CLASS_FIELD_INIT)
            != 0
    }

    pub fn in_class_static_block(&self) -> bool {
        self.var_scope().flags & SCOPE_CLASS_STATIC_BLOCK != 0
    }

    /// ¿`arguments` está prohibido (inicializador de campo o bloque estático, sin una función
    /// no flecha en medio)?
    pub fn arguments_forbidden(&self) -> bool {
        for s in self.scopes.iter().rev() {
            if s.flags & (SCOPE_CLASS_FIELD_INIT | SCOPE_CLASS_STATIC_BLOCK) != 0 {
                return true;
            }
            if s.flags & SCOPE_FUNCTION != 0 && s.flags & SCOPE_ARROW == 0 {
                return false;
            }
        }
        false
    }

    /// Early errors de un IdentifierReference/BindingIdentifier/LabelIdentifier (§13.1.1).
    pub fn check_unreserved(&self, name: &Atom, pos: usize) -> PResult<()> {
        let n = name.as_str();
        if n == "yield" && (self.in_generator() || self.var_scope().flags & SCOPE_OUTER_YIELD != 0)
        {
            return self.raise(pos, "Cannot use 'yield' as identifier inside a generator");
        }
        if n == "await" && (self.in_async() || self.var_scope().flags & SCOPE_OUTER_AWAIT != 0)
        {
            return self.raise(pos, "Cannot use 'await' as identifier inside an async function");
        }
        if n == "arguments" && self.arguments_forbidden() {
            return self.raise(
                pos,
                "'arguments' is not allowed in class field initializer or static initialization block",
            );
        }
        if self.in_class_static_block() && n == "await" {
            return self.raise(pos, "Cannot use 'await' in class static initialization block");
        }
        if KEYWORDS.contains(&n) {
            return self.raise(pos, format!("Unexpected keyword '{n}'"));
        }
        if self.strict && STRICT_RESERVED.contains(&n) {
            return self.raise(pos, format!("Unexpected strict mode reserved word '{n}'"));
        }
        if n == "await" && self.module {
            return self.raise(pos, "Cannot use keyword 'await' outside an async function");
        }
        Ok(())
    }
}
