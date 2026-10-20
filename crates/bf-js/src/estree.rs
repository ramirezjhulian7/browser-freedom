//! Serialización del AST a JSON con la forma de ESTree (los paréntesis se omiten, como en
//! ESTree). Con `locations`, cada nodo lleva `start` y `end` (code units UTF-16).

use std::fmt::Write as _;

use crate::ast::*;

pub struct Options {
    pub locations: bool,
    /// Incluir el texto `raw` de los literales.
    pub raw: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            locations: true,
            raw: true,
        }
    }
}

struct W<'o> {
    out: String,
    o: &'o Options,
}

fn json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Un string de ECMAScript (con surrogates sueltos como `\uXXXX`).
fn json_units(out: &mut String, units: &[u16]) {
    out.push('"');
    for r in char::decode_utf16(units.iter().copied()) {
        match r {
            Ok(c) => {
                let mut buf = String::new();
                json_str(&mut buf, &c.to_string());
                out.push_str(&buf[1..buf.len() - 1]);
            }
            Err(e) => {
                let _ = write!(out, "\\u{:04x}", e.unpaired_surrogate());
            }
        }
    }
    out.push('"');
}

fn number(n: f64) -> String {
    if n.is_finite() {
        if n == n.trunc() && n.abs() < 1e21 {
            format!("{n:.0}")
        } else {
            format!("{n}")
        }
    } else {
        "null".to_owned()
    }
}

impl W<'_> {
    fn open(&mut self, ty: &str, span: Span) {
        self.out.push_str("{\"type\":");
        json_str(&mut self.out, ty);
        if self.o.locations {
            let _ = write!(self.out, ",\"start\":{},\"end\":{}", span.start, span.end);
        }
    }

    fn key(&mut self, k: &str) {
        self.out.push(',');
        json_str(&mut self.out, k);
        self.out.push(':');
    }

    fn close(&mut self) {
        self.out.push('}');
    }

    fn bool(&mut self, k: &str, v: bool) {
        self.key(k);
        self.out.push_str(if v { "true" } else { "false" });
    }

    fn str(&mut self, k: &str, v: &str) {
        self.key(k);
        json_str(&mut self.out, v);
    }

    fn null(&mut self, k: &str) {
        self.key(k);
        self.out.push_str("null");
    }

    fn list<T>(&mut self, k: &str, items: &[T], f: impl Fn(&mut Self, &T)) {
        self.key(k);
        self.out.push('[');
        for (i, it) in items.iter().enumerate() {
            if i > 0 {
                self.out.push(',');
            }
            f(self, it);
        }
        self.out.push(']');
    }

    fn opt<T>(&mut self, k: &str, v: &Option<T>, f: impl Fn(&mut Self, &T)) {
        match v {
            Some(v) => {
                self.key(k);
                f(self, v);
            }
            None => self.null(k),
        }
    }

    fn ident(&mut self, id: &Ident) {
        self.open("Identifier", id.span);
        self.str("name", &id.name);
        self.close();
    }

    fn private(&mut self, id: &Ident) {
        self.open("PrivateIdentifier", id.span);
        self.str("name", &id.name);
        self.close();
    }

    fn literal(&mut self, l: &Literal) {
        self.open("Literal", l.span);
        self.key("value");
        match &l.value {
            LitValue::Null => self.out.push_str("null"),
            LitValue::Bool(b) => self.out.push_str(if *b { "true" } else { "false" }),
            LitValue::Number(n) => self.out.push_str(&number(*n)),
            LitValue::String(s) => json_units(&mut self.out, s.units()),
            LitValue::BigInt(_) | LitValue::Regex { .. } => self.out.push_str("null"),
        }
        if self.o.raw {
            self.str("raw", &l.raw);
        }
        match &l.value {
            LitValue::BigInt(d) => self.str("bigint", d),
            LitValue::Regex { pattern, flags } => {
                self.key("regex");
                self.out.push_str("{\"pattern\":");
                json_str(&mut self.out, pattern);
                self.out.push_str(",\"flags\":");
                json_str(&mut self.out, flags);
                self.out.push('}');
            }
            _ => {}
        }
        self.close();
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Expression {
                span,
                expression,
                directive,
            } => {
                self.open("ExpressionStatement", *span);
                self.key("expression");
                self.expr(expression);
                if let Some(d) = directive {
                    self.str("directive", d);
                }
                self.close();
            }
            Stmt::Block(b) => self.block(b, "BlockStatement"),
            Stmt::Empty(span) => {
                self.open("EmptyStatement", *span);
                self.close();
            }
            Stmt::Debugger(span) => {
                self.open("DebuggerStatement", *span);
                self.close();
            }
            Stmt::With { span, object, body } => {
                self.open("WithStatement", *span);
                self.key("object");
                self.expr(object);
                self.key("body");
                self.stmt(body);
                self.close();
            }
            Stmt::Return { span, argument } => {
                self.open("ReturnStatement", *span);
                self.opt("argument", argument, |w, e| w.expr(e));
                self.close();
            }
            Stmt::Labeled { span, label, body } => {
                self.open("LabeledStatement", *span);
                self.key("label");
                self.ident(label);
                self.key("body");
                self.stmt(body);
                self.close();
            }
            Stmt::Break { span, label } | Stmt::Continue { span, label } => {
                let ty = if matches!(s, Stmt::Break { .. }) {
                    "BreakStatement"
                } else {
                    "ContinueStatement"
                };
                self.open(ty, *span);
                self.opt("label", label, |w, l| w.ident(l));
                self.close();
            }
            Stmt::If {
                span,
                test,
                consequent,
                alternate,
            } => {
                self.open("IfStatement", *span);
                self.key("test");
                self.expr(test);
                self.key("consequent");
                self.stmt(consequent);
                self.opt("alternate", alternate, |w, a| w.stmt(a));
                self.close();
            }
            Stmt::Switch {
                span,
                discriminant,
                cases,
            } => {
                self.open("SwitchStatement", *span);
                self.key("discriminant");
                self.expr(discriminant);
                self.list("cases", cases, |w, c| {
                    w.open("SwitchCase", c.span);
                    w.opt("test", &c.test, |w, t| w.expr(t));
                    w.list("consequent", &c.consequent, |w, s| w.stmt(s));
                    w.close();
                });
                self.close();
            }
            Stmt::Throw { span, argument } => {
                self.open("ThrowStatement", *span);
                self.key("argument");
                self.expr(argument);
                self.close();
            }
            Stmt::Try {
                span,
                block,
                handler,
                finalizer,
            } => {
                self.open("TryStatement", *span);
                self.key("block");
                self.block(block, "BlockStatement");
                self.opt("handler", handler, |w, h| {
                    w.open("CatchClause", h.span);
                    w.opt("param", &h.param, |w, p| w.pat(p));
                    w.key("body");
                    w.block(&h.body, "BlockStatement");
                    w.close();
                });
                self.opt("finalizer", finalizer, |w, f| w.block(f, "BlockStatement"));
                self.close();
            }
            Stmt::While { span, test, body } => {
                self.open("WhileStatement", *span);
                self.key("test");
                self.expr(test);
                self.key("body");
                self.stmt(body);
                self.close();
            }
            Stmt::DoWhile { span, body, test } => {
                self.open("DoWhileStatement", *span);
                self.key("body");
                self.stmt(body);
                self.key("test");
                self.expr(test);
                self.close();
            }
            Stmt::For {
                span,
                init,
                test,
                update,
                body,
            } => {
                self.open("ForStatement", *span);
                self.opt("init", init, |w, i| match i {
                    ForInit::Variable(v) => w.var_decl(v),
                    ForInit::Expr(e) => w.expr(e),
                });
                self.opt("test", test, |w, e| w.expr(e));
                self.opt("update", update, |w, e| w.expr(e));
                self.key("body");
                self.stmt(body);
                self.close();
            }
            Stmt::ForIn {
                span,
                left,
                right,
                body,
            } => {
                self.open("ForInStatement", *span);
                self.for_head(left);
                self.key("right");
                self.expr(right);
                self.key("body");
                self.stmt(body);
                self.close();
            }
            Stmt::ForOf {
                span,
                left,
                right,
                body,
                is_await,
            } => {
                self.open("ForOfStatement", *span);
                self.bool("await", *is_await);
                self.for_head(left);
                self.key("right");
                self.expr(right);
                self.key("body");
                self.stmt(body);
                self.close();
            }
            Stmt::Function(f) => self.function(f, "FunctionDeclaration"),
            Stmt::Variable(v) => self.var_decl(v),
            Stmt::Class(c) => self.class(c, "ClassDeclaration"),
            Stmt::Import(i) => {
                self.open("ImportDeclaration", i.span);
                self.list("specifiers", &i.specifiers, |w, s| match s {
                    ImportSpecifier::Default { span, local } => {
                        w.open("ImportDefaultSpecifier", *span);
                        w.key("local");
                        w.ident(local);
                        w.close();
                    }
                    ImportSpecifier::Namespace { span, local } => {
                        w.open("ImportNamespaceSpecifier", *span);
                        w.key("local");
                        w.ident(local);
                        w.close();
                    }
                    ImportSpecifier::Named {
                        span,
                        imported,
                        local,
                    } => {
                        w.open("ImportSpecifier", *span);
                        w.key("imported");
                        w.export_name(imported);
                        w.key("local");
                        w.ident(local);
                        w.close();
                    }
                });
                self.key("source");
                self.literal(&i.source);
                self.attributes(&i.attributes);
                self.close();
            }
            Stmt::ExportNamed(e) => {
                self.open("ExportNamedDeclaration", e.span);
                self.opt("declaration", &e.declaration, |w, d| w.stmt(d));
                self.list("specifiers", &e.specifiers, |w, s| {
                    w.open("ExportSpecifier", s.span);
                    w.key("local");
                    w.export_name(&s.local);
                    w.key("exported");
                    w.export_name(&s.exported);
                    w.close();
                });
                self.opt("source", &e.source, |w, s| w.literal(s));
                self.attributes(&e.attributes);
                self.close();
            }
            Stmt::ExportDefault { span, declaration } => {
                self.open("ExportDefaultDeclaration", *span);
                self.key("declaration");
                match declaration {
                    ExportDefaultKind::Function(f) => self.function(f, "FunctionDeclaration"),
                    ExportDefaultKind::Class(c) => self.class(c, "ClassDeclaration"),
                    ExportDefaultKind::Expr(e) => self.expr(e),
                }
                self.close();
            }
            Stmt::ExportAll {
                span,
                exported,
                source,
                attributes,
            } => {
                self.open("ExportAllDeclaration", *span);
                self.opt("exported", exported, |w, e| w.export_name(e));
                self.key("source");
                self.literal(source);
                self.attributes(attributes);
                self.close();
            }
        }
    }

    fn attributes(&mut self, attrs: &[ImportAttribute]) {
        self.list("attributes", attrs, |w, a| {
            w.open("ImportAttribute", a.span);
            w.key("key");
            w.export_name(&a.key);
            w.key("value");
            w.literal(&a.value);
            w.close();
        });
    }

    fn export_name(&mut self, n: &ModuleExportName) {
        match n {
            ModuleExportName::Ident(i) => self.ident(i),
            ModuleExportName::String(l) => self.literal(l),
        }
    }

    fn for_head(&mut self, h: &ForHead) {
        self.key("left");
        match h {
            ForHead::Variable(v) => self.var_decl(v),
            ForHead::Pattern(p) => self.pat(p),
        }
    }

    fn block(&mut self, b: &BlockStmt, ty: &str) {
        self.open(ty, b.span);
        self.list("body", &b.body, |w, s| w.stmt(s));
        self.close();
    }

    fn var_decl(&mut self, v: &VariableDeclaration) {
        self.open("VariableDeclaration", v.span);
        self.list("declarations", &v.declarations, |w, d| {
            w.open("VariableDeclarator", d.span);
            w.key("id");
            w.pat(&d.id);
            w.opt("init", &d.init, |w, e| w.expr(e));
            w.close();
        });
        self.str("kind", v.kind.as_str());
        self.close();
    }

    fn function(&mut self, f: &Function, ty: &str) {
        self.open(ty, f.span);
        self.opt("id", &f.id, |w, i| w.ident(i));
        self.bool("expression", false);
        self.bool("generator", f.is_generator);
        self.bool("async", f.is_async);
        self.list("params", &f.params, |w, p| w.pat(p));
        self.key("body");
        self.block(&f.body, "BlockStatement");
        self.close();
    }

    fn class(&mut self, c: &Class, ty: &str) {
        self.open(ty, c.span);
        self.opt("id", &c.id, |w, i| w.ident(i));
        self.opt("superClass", &c.super_class, |w, e| w.expr(e));
        self.key("body");
        self.open("ClassBody", c.body_span);
        self.list("body", &c.body, |w, el| w.class_element(el));
        self.close();
        self.close();
    }

    fn prop_key(&mut self, k: &PropKey) {
        match k {
            PropKey::Ident(i) => self.ident(i),
            PropKey::Literal(l) => self.literal(l),
            PropKey::Computed(e) => self.expr(e),
            PropKey::Private(i) => self.private(i),
        }
    }

    fn class_element(&mut self, el: &ClassElement) {
        match el {
            ClassElement::Method {
                span,
                key,
                value,
                kind,
                is_static,
            } => {
                self.open("MethodDefinition", *span);
                self.bool("static", *is_static);
                self.bool("computed", matches!(key, PropKey::Computed(_)));
                self.key("key");
                self.prop_key(key);
                self.str(
                    "kind",
                    match kind {
                        MethodKind::Constructor => "constructor",
                        MethodKind::Method => "method",
                        MethodKind::Get => "get",
                        MethodKind::Set => "set",
                    },
                );
                self.key("value");
                self.function(value, "FunctionExpression");
                self.close();
            }
            ClassElement::Property {
                span,
                key,
                value,
                is_static,
            } => {
                self.open("PropertyDefinition", *span);
                self.bool("static", *is_static);
                self.bool("computed", matches!(key, PropKey::Computed(_)));
                self.key("key");
                self.prop_key(key);
                self.opt("value", value, |w, e| w.expr(e));
                self.close();
            }
            ClassElement::StaticBlock(b) => self.block(b, "StaticBlock"),
        }
    }

    fn spread_or(&mut self, e: &ExprOrSpread) {
        match e.spread {
            Some(s) => {
                self.open("SpreadElement", Span::new(s.start, e.expr.span().end));
                self.key("argument");
                self.expr(&e.expr);
                self.close();
            }
            None => self.expr(&e.expr),
        }
    }

    fn template(&mut self, t: &Template) {
        self.open("TemplateLiteral", t.span);
        self.list("expressions", &t.expressions, |w, e| w.expr(e));
        self.list("quasis", &t.quasis, |w, q| {
            w.open("TemplateElement", q.span);
            w.bool("tail", q.tail);
            w.key("value");
            w.out.push_str("{\"raw\":");
            json_str(&mut w.out, &q.raw);
            w.out.push_str(",\"cooked\":");
            match &q.cooked {
                Some(c) => json_units(&mut w.out, c.units()),
                None => w.out.push_str("null"),
            }
            w.out.push('}');
            w.close();
        });
        self.close();
    }

    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Ident(i) => self.ident(i),
            Expr::PrivateName(i) => self.private(i),
            Expr::Literal(l) => self.literal(l),
            Expr::This(s) => {
                self.open("ThisExpression", *s);
                self.close();
            }
            Expr::Super(s) => {
                self.open("Super", *s);
                self.close();
            }
            Expr::Array { span, elements } => {
                self.open("ArrayExpression", *span);
                self.list("elements", elements, |w, el| match el {
                    Some(el) => w.spread_or(el),
                    None => w.out.push_str("null"),
                });
                self.close();
            }
            Expr::Object { span, properties } => {
                self.open("ObjectExpression", *span);
                self.list("properties", properties, |w, p| w.object_member(p));
                self.close();
            }
            Expr::Function(f) => self.function(f, "FunctionExpression"),
            Expr::Arrow(a) => {
                self.open("ArrowFunctionExpression", a.span);
                self.null("id");
                self.bool("expression", matches!(a.body, ArrowBody::Expr(_)));
                self.bool("generator", false);
                self.bool("async", a.is_async);
                self.list("params", &a.params, |w, p| w.pat(p));
                self.key("body");
                match &a.body {
                    ArrowBody::Expr(e) => self.expr(e),
                    ArrowBody::Block(b) => self.block(b, "BlockStatement"),
                }
                self.close();
            }
            Expr::Class(c) => self.class(c, "ClassExpression"),
            Expr::Template(t) => self.template(t),
            Expr::TaggedTemplate { span, tag, quasi } => {
                self.open("TaggedTemplateExpression", *span);
                self.key("tag");
                self.expr(tag);
                self.key("quasi");
                self.template(quasi);
                self.close();
            }
            Expr::Unary {
                span,
                operator,
                argument,
            } => {
                self.open("UnaryExpression", *span);
                self.str("operator", operator);
                self.bool("prefix", true);
                self.key("argument");
                self.expr(argument);
                self.close();
            }
            Expr::Update {
                span,
                operator,
                prefix,
                argument,
            } => {
                self.open("UpdateExpression", *span);
                self.str("operator", operator);
                self.bool("prefix", *prefix);
                self.key("argument");
                self.expr(argument);
                self.close();
            }
            Expr::Binary {
                span,
                operator,
                left,
                right,
            }
            | Expr::Logical {
                span,
                operator,
                left,
                right,
            } => {
                let ty = if matches!(e, Expr::Logical { .. }) {
                    "LogicalExpression"
                } else {
                    "BinaryExpression"
                };
                self.open(ty, *span);
                self.key("left");
                self.expr(left);
                self.str("operator", operator);
                self.key("right");
                self.expr(right);
                self.close();
            }
            Expr::Assign {
                span,
                operator,
                left,
                right,
            } => {
                self.open("AssignmentExpression", *span);
                self.str("operator", operator);
                self.key("left");
                self.pat(left);
                self.key("right");
                self.expr(right);
                self.close();
            }
            Expr::Conditional {
                span,
                test,
                consequent,
                alternate,
            } => {
                self.open("ConditionalExpression", *span);
                self.key("test");
                self.expr(test);
                self.key("consequent");
                self.expr(consequent);
                self.key("alternate");
                self.expr(alternate);
                self.close();
            }
            Expr::Call {
                span,
                callee,
                arguments,
                optional,
            } => {
                self.open("CallExpression", *span);
                self.key("callee");
                self.expr(callee);
                self.list("arguments", arguments, |w, a| w.spread_or(a));
                self.bool("optional", *optional);
                self.close();
            }
            Expr::New {
                span,
                callee,
                arguments,
            } => {
                self.open("NewExpression", *span);
                self.key("callee");
                self.expr(callee);
                self.list("arguments", arguments, |w, a| w.spread_or(a));
                self.close();
            }
            Expr::Sequence { span, expressions } => {
                self.open("SequenceExpression", *span);
                self.list("expressions", expressions, |w, e| w.expr(e));
                self.close();
            }
            Expr::Member {
                span,
                object,
                property,
                optional,
            } => {
                self.open("MemberExpression", *span);
                self.key("object");
                self.expr(object);
                self.key("property");
                match property {
                    MemberProp::Ident(i) => self.ident(i),
                    MemberProp::Private(i) => self.private(i),
                    MemberProp::Computed(e) => self.expr(e),
                }
                self.bool("computed", matches!(property, MemberProp::Computed(_)));
                self.bool("optional", *optional);
                self.close();
            }
            Expr::Chain { span, expression } => {
                self.open("ChainExpression", *span);
                self.key("expression");
                self.expr(expression);
                self.close();
            }
            Expr::Yield {
                span,
                argument,
                delegate,
            } => {
                self.open("YieldExpression", *span);
                self.bool("delegate", *delegate);
                self.opt("argument", argument, |w, a| w.expr(a));
                self.close();
            }
            Expr::Await { span, argument } => {
                self.open("AwaitExpression", *span);
                self.key("argument");
                self.expr(argument);
                self.close();
            }
            Expr::MetaProperty {
                span,
                meta,
                property,
            } => {
                self.open("MetaProperty", *span);
                self.key("meta");
                self.ident(meta);
                self.key("property");
                self.ident(property);
                self.close();
            }
            Expr::Import {
                span,
                source,
                options,
            } => {
                self.open("ImportExpression", *span);
                self.key("source");
                self.expr(source);
                self.opt("options", options, |w, o| w.expr(o));
                self.close();
            }
            Expr::Paren { expression, .. } => self.expr(expression),
        }
    }

    fn object_member(&mut self, p: &ObjectMember) {
        match p {
            ObjectMember::Spread { span, argument } => {
                self.open("SpreadElement", *span);
                self.key("argument");
                self.expr(argument);
                self.close();
            }
            ObjectMember::Property {
                span,
                key,
                value,
                kind,
                method,
                shorthand,
            } => {
                self.open("Property", *span);
                self.bool("method", *method);
                self.bool("shorthand", *shorthand);
                self.bool("computed", matches!(key, PropKey::Computed(_)));
                self.key("key");
                self.prop_key(key);
                self.key("value");
                match value {
                    PropValue::Expr(e) => self.expr(e),
                    PropValue::Method(f) => self.function(f, "FunctionExpression"),
                    PropValue::CoverInit(id, default) => {
                        self.open("AssignmentPattern", *span);
                        self.key("left");
                        self.ident(id);
                        self.key("right");
                        self.expr(default);
                        self.close();
                    }
                }
                self.str(
                    "kind",
                    match kind {
                        PropKind::Init => "init",
                        PropKind::Get => "get",
                        PropKind::Set => "set",
                    },
                );
                self.close();
            }
        }
    }

    fn pat(&mut self, p: &Pat) {
        match p {
            Pat::Ident(i) => self.ident(i),
            Pat::Expr(e) => self.expr(e),
            Pat::Object { span, properties } => {
                self.open("ObjectPattern", *span);
                self.list("properties", properties, |w, p| match p {
                    ObjectPatProp::Property {
                        span,
                        key,
                        value,
                        shorthand,
                    } => {
                        w.open("Property", *span);
                        w.bool("method", false);
                        w.bool("shorthand", *shorthand);
                        w.bool("computed", matches!(key, PropKey::Computed(_)));
                        w.key("key");
                        w.prop_key(key);
                        w.key("value");
                        w.pat(value);
                        w.str("kind", "init");
                        w.close();
                    }
                    ObjectPatProp::Rest { span, argument } => {
                        w.open("RestElement", *span);
                        w.key("argument");
                        w.pat(argument);
                        w.close();
                    }
                });
                self.close();
            }
            Pat::Array { span, elements } => {
                self.open("ArrayPattern", *span);
                self.list("elements", elements, |w, el| match el {
                    Some(p) => w.pat(p),
                    None => w.out.push_str("null"),
                });
                self.close();
            }
            Pat::Rest { span, argument } => {
                self.open("RestElement", *span);
                self.key("argument");
                self.pat(argument);
                self.close();
            }
            Pat::Assign { span, left, right } => {
                self.open("AssignmentPattern", *span);
                self.key("left");
                self.pat(left);
                self.key("right");
                self.expr(right);
                self.close();
            }
        }
    }
}

/// El programa como JSON ESTree.
pub fn to_json(program: &Program, options: &Options) -> String {
    let mut w = W {
        out: String::new(),
        o: options,
    };
    w.open("Program", program.span);
    w.str(
        "sourceType",
        match program.source_type {
            SourceType::Script => "script",
            SourceType::Module => "module",
        },
    );
    w.list("body", &program.body, |w, s| w.stmt(s));
    w.close();
    w.out
}
