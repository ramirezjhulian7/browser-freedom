//! Módulos (ECMA-262 §16.2): `import` y `export` en todas sus formas, los atributos de import
//! (`with { type: "json" }`) y los early errors de §16.2.1.1 (exports duplicados, nombres
//! exportados sin declarar, strings que no son Unicode bien formado).

use std::collections::HashSet;

use super::function::FnDecl;
use super::{Bind, PResult, Parser};
use crate::ast::{
    ExportDefaultKind, ExportNamed, ExportSpecifier, ImportAttribute, ImportDeclaration,
    ImportSpecifier, LitValue, Literal, ModuleExportName, Stmt, VarKind,
};
use crate::lexer::Tok;

impl Parser {
    /// `ModuleExportName`: IdentifierName o un string bien formado.
    fn parse_module_export_name(&mut self) -> PResult<ModuleExportName> {
        if let Tok::String { value, .. } = &self.tok.tok {
            if !value.is_well_formed() {
                return self.raise(
                    self.tok.start,
                    "Invalid module export name: contains unpaired surrogate",
                );
            }
            return Ok(ModuleExportName::String(self.parse_string_literal()?));
        }
        Ok(ModuleExportName::Ident(self.parse_ident(true)?))
    }

    fn export_name_string(name: &ModuleExportName) -> String {
        match name {
            ModuleExportName::Ident(i) => i.name.to_string(),
            ModuleExportName::String(Literal {
                value: LitValue::String(s),
                ..
            }) => s.to_string_lossy(),
            ModuleExportName::String(l) => l.raw.clone(),
        }
    }

    fn check_export(&mut self, name: &str, pos: usize) -> PResult<()> {
        if !self.exports.insert(name.to_owned()) {
            return self.raise(pos, format!("Duplicate export of '{name}'"));
        }
        Ok(())
    }

    fn module_source(&mut self) -> PResult<Literal> {
        if !matches!(self.tok.tok, Tok::String { .. }) {
            return self.unexpected();
        }
        self.parse_string_literal()
    }

    /// `with { key: "value", ... }` (§16.2.2, WithClause).
    fn parse_with_clause(&mut self) -> PResult<Vec<ImportAttribute>> {
        if !self.is_kw("with") {
            return Ok(Vec::new());
        }
        self.next()?;
        self.expect_punct("{")?;
        let mut attrs = Vec::new();
        let mut keys = HashSet::new();
        let mut first = true;
        while !self.eat_punct("}")? {
            if !first {
                self.expect_punct(",")?;
                if self.eat_punct("}")? {
                    break;
                }
            }
            first = false;
            let start = self.tok.start;
            let key = if matches!(self.tok.tok, Tok::String { .. }) {
                ModuleExportName::String(self.parse_string_literal()?)
            } else {
                ModuleExportName::Ident(self.parse_ident(true)?)
            };
            let k = Self::export_name_string(&key);
            if !keys.insert(k.clone()) {
                return self.raise(start, format!("Import attribute has duplicate key '{k}'"));
            }
            self.expect_punct(":")?;
            if !matches!(self.tok.tok, Tok::String { .. }) {
                return self.unexpected();
            }
            let value = self.parse_string_literal()?;
            attrs.push(ImportAttribute {
                span: self.span_from(start),
                key,
                value,
            });
        }
        Ok(attrs)
    }

    /// ImportDeclaration (el token actual es `import`).
    pub fn parse_import(&mut self, start: usize) -> PResult<Stmt> {
        self.next()?;
        let mut specifiers = Vec::new();
        let clause = !matches!(self.tok.tok, Tok::String { .. });
        if clause {
            // ImportClause.
            if self.is_name_token() && !self.is_punct("{") {
                let s = self.tok.start;
                let local = self.parse_ident(false)?;
                self.check_lval_ident(&local, Bind::Lexical, &mut None)?;
                specifiers.push(ImportSpecifier::Default {
                    span: self.span_from(s),
                    local,
                });
                if !self.eat_punct(",")? {
                    return self.finish_import(start, specifiers, true);
                }
            }
            if self.is_punct("*") {
                let s = self.tok.start;
                self.next()?;
                self.expect_kw("as")?;
                let local = self.parse_ident(false)?;
                self.check_lval_ident(&local, Bind::Lexical, &mut None)?;
                specifiers.push(ImportSpecifier::Namespace {
                    span: self.span_from(s),
                    local,
                });
            } else if self.eat_punct("{")? {
                let mut first = true;
                while !self.eat_punct("}")? {
                    if !first {
                        self.expect_punct(",")?;
                        if self.eat_punct("}")? {
                            break;
                        }
                    }
                    first = false;
                    let s = self.tok.start;
                    let imported = self.parse_module_export_name()?;
                    let local = if self.eat_kw("as")? {
                        self.parse_ident(false)?
                    } else {
                        match &imported {
                            ModuleExportName::Ident(id) => {
                                self.check_unreserved(&id.name, self.lex_pos(id.span.start))?;
                                id.clone()
                            }
                            ModuleExportName::String(_) => return self.unexpected(),
                        }
                    };
                    self.check_lval_ident(&local, Bind::Lexical, &mut None)?;
                    specifiers.push(ImportSpecifier::Named {
                        span: self.span_from(s),
                        imported,
                        local,
                    });
                }
            } else {
                return self.unexpected();
            }
        }
        self.finish_import(start, specifiers, clause)
    }

    fn finish_import(
        &mut self,
        start: usize,
        specifiers: Vec<ImportSpecifier>,
        clause: bool,
    ) -> PResult<Stmt> {
        if clause {
            self.expect_kw("from")?;
        }
        let source = self.module_source()?;
        let attributes = self.parse_with_clause()?;
        self.semicolon()?;
        Ok(Stmt::Import(Box::new(ImportDeclaration {
            span: self.span_from(start),
            specifiers,
            source,
            attributes,
        })))
    }

    /// ExportDeclaration (el token actual es `export`).
    pub fn parse_export(&mut self, start: usize) -> PResult<Stmt> {
        self.next()?;
        // export * [as name] from "m"
        if self.eat_punct("*")? {
            let exported = if self.eat_kw("as")? {
                let pos = self.tok.start;
                let name = self.parse_module_export_name()?;
                self.check_export(&Self::export_name_string(&name), pos)?;
                Some(name)
            } else {
                None
            };
            self.expect_kw("from")?;
            let source = self.module_source()?;
            let attributes = self.parse_with_clause()?;
            self.semicolon()?;
            return Ok(Stmt::ExportAll {
                span: self.span_from(start),
                exported,
                source,
                attributes,
            });
        }
        // export default ...
        if self.is_kw("default") {
            self.check_export("default", self.tok.start)?;
            self.next()?;
            let dstart = self.tok.start;
            let is_async = self.is_async_function_decl()?;
            let declaration = if self.is_kw("function") || is_async {
                if is_async {
                    self.next()?;
                }
                self.next()?;
                let f = self.parse_function_declaration(
                    dstart,
                    FnDecl::Statement { nullable_id: true },
                    is_async,
                )?;
                ExportDefaultKind::Function(Box::new(f))
            } else if self.is_kw("class") {
                ExportDefaultKind::Class(Box::new(self.parse_class(dstart, true, true)?))
            } else {
                let e = self.parse_maybe_assign(false, None)?;
                self.semicolon()?;
                ExportDefaultKind::Expr(e)
            };
            return Ok(Stmt::ExportDefault {
                span: self.span_from(start),
                declaration,
            });
        }
        // export var|let|const|function|class|async function
        if self.is_kw("var")
            || self.is_kw("const")
            || self.is_kw("function")
            || self.is_kw("class")
            || self.is_kw("let")
            || self.is_async_function_decl()?
        {
            let stmt = self.parse_statement(None, true)?;
            match &stmt {
                Stmt::Variable(v) => {
                    for d in &v.declarations {
                        let mut names = Vec::new();
                        super::lval::bound_names(&d.id, &mut names);
                        for n in names {
                            let pos = self.lex_pos(n.span.start);
                            self.check_export(&n.name, pos)?;
                        }
                    }
                    if v.kind == VarKind::Using || v.kind == VarKind::AwaitUsing {
                        return self.raise(start, "Using declarations cannot be exported");
                    }
                }
                Stmt::Function(f) => {
                    if let Some(id) = &f.id {
                        let pos = self.lex_pos(id.span.start);
                        self.check_export(&id.name, pos)?;
                    }
                }
                Stmt::Class(c) => {
                    if let Some(id) = &c.id {
                        let pos = self.lex_pos(id.span.start);
                        self.check_export(&id.name, pos)?;
                    }
                }
                _ => return self.unexpected_at(start),
            }
            return Ok(Stmt::ExportNamed(Box::new(ExportNamed {
                span: self.span_from(start),
                declaration: Some(stmt),
                specifiers: Vec::new(),
                source: None,
                attributes: Vec::new(),
            })));
        }
        // export { a, b as c } [from "m"]
        self.expect_punct("{")?;
        let mut specifiers = Vec::new();
        let mut first = true;
        while !self.eat_punct("}")? {
            if !first {
                self.expect_punct(",")?;
                if self.eat_punct("}")? {
                    break;
                }
            }
            first = false;
            let s = self.tok.start;
            let local = self.parse_module_export_name()?;
            let exported = if self.eat_kw("as")? {
                self.parse_module_export_name()?
            } else {
                local.clone()
            };
            let pos = match &exported {
                ModuleExportName::Ident(i) => self.lex_pos(i.span.start),
                ModuleExportName::String(l) => self.lex_pos(l.span.start),
            };
            self.check_export(&Self::export_name_string(&exported), pos)?;
            specifiers.push(ExportSpecifier {
                span: self.span_from(s),
                local,
                exported,
            });
        }
        let (source, attributes) = if self.eat_kw("from")? {
            let source = self.module_source()?;
            (Some(source), self.parse_with_clause()?)
        } else {
            for spec in &specifiers {
                match &spec.local {
                    ModuleExportName::String(l) => {
                        return self.raise(
                            self.lex_pos(l.span.start),
                            "A string literal cannot be used as an exported binding without `from`",
                        );
                    }
                    ModuleExportName::Ident(id) => {
                        let pos = self.lex_pos(id.span.start);
                        self.check_unreserved(&id.name, pos)?;
                        let top = &self.scopes[0];
                        if !top.lexical.contains(&id.name)
                            && !top.var.contains(&id.name)
                            && !top.functions.contains(&id.name)
                        {
                            self.undefined_exports.push((id.name.clone(), pos));
                        }
                    }
                }
            }
            (None, Vec::new())
        };
        self.semicolon()?;
        Ok(Stmt::ExportNamed(Box::new(ExportNamed {
            span: self.span_from(start),
            declaration: None,
            specifiers,
            source,
            attributes,
        })))
    }

    /// `async function` sin LineTerminator en medio (como `is_async_function`).
    fn is_async_function_decl(&mut self) -> PResult<bool> {
        if !self.is_kw("async") {
            return Ok(false);
        }
        let next = self.peek()?;
        Ok(matches!(&next.tok, Tok::Name(n) if *n == "function")
            && !next.escaped
            && !next.nl_before)
    }
}
