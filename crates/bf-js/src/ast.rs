//! Árbol de sintaxis de ECMAScript con la forma de ESTree (nombres de nodos y campos), más
//! [`Expr::Paren`] para los paréntesis (que importan a la semántica: `(a) = 1`, nombres de
//! funciones anónimas, `delete (x)`); el serializador ESTree ([`crate::estree`]) los omite.
//! Cada nodo lleva su [`Span`] en code units UTF-16 del código fuente.

use crate::string::{Atom, JsString};

/// Rango `[start, end)` en code units UTF-16.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: u32, end: u32) -> Span {
        Span { start, end }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceType {
    Script,
    Module,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub span: Span,
    pub source_type: SourceType,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ident {
    pub span: Span,
    pub name: Atom,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LitValue {
    Null,
    Bool(bool),
    Number(f64),
    String(JsString),
    /// Dígitos tal como aparecen (sin `n` ni separadores).
    BigInt(String),
    Regex { pattern: String, flags: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Literal {
    pub span: Span,
    pub value: LitValue,
    /// Texto original del literal.
    pub raw: String,
}

// --- Sentencias ---

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Expression {
        span: Span,
        expression: Expr,
        /// Para las directivas del prólogo (`"use strict"`): su texto crudo sin comillas.
        directive: Option<String>,
    },
    Block(BlockStmt),
    Empty(Span),
    Debugger(Span),
    With {
        span: Span,
        object: Expr,
        body: Box<Stmt>,
    },
    Return {
        span: Span,
        argument: Option<Expr>,
    },
    Labeled {
        span: Span,
        label: Ident,
        body: Box<Stmt>,
    },
    Break {
        span: Span,
        label: Option<Ident>,
    },
    Continue {
        span: Span,
        label: Option<Ident>,
    },
    If {
        span: Span,
        test: Expr,
        consequent: Box<Stmt>,
        alternate: Option<Box<Stmt>>,
    },
    Switch {
        span: Span,
        discriminant: Expr,
        cases: Vec<SwitchCase>,
    },
    Throw {
        span: Span,
        argument: Expr,
    },
    Try {
        span: Span,
        block: BlockStmt,
        handler: Option<CatchClause>,
        finalizer: Option<BlockStmt>,
    },
    While {
        span: Span,
        test: Expr,
        body: Box<Stmt>,
    },
    DoWhile {
        span: Span,
        body: Box<Stmt>,
        test: Expr,
    },
    For {
        span: Span,
        init: Option<ForInit>,
        test: Option<Expr>,
        update: Option<Expr>,
        body: Box<Stmt>,
    },
    ForIn {
        span: Span,
        left: ForHead,
        right: Expr,
        body: Box<Stmt>,
    },
    ForOf {
        span: Span,
        left: ForHead,
        right: Expr,
        body: Box<Stmt>,
        is_await: bool,
    },
    Function(Box<Function>),
    Variable(VariableDeclaration),
    Class(Box<Class>),
    Import(Box<ImportDeclaration>),
    ExportNamed(Box<ExportNamed>),
    ExportDefault {
        span: Span,
        declaration: ExportDefaultKind,
    },
    ExportAll {
        span: Span,
        exported: Option<ModuleExportName>,
        source: Literal,
        attributes: Vec<ImportAttribute>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockStmt {
    pub span: Span,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SwitchCase {
    pub span: Span,
    pub test: Option<Expr>,
    pub consequent: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CatchClause {
    pub span: Span,
    pub param: Option<Pat>,
    pub body: BlockStmt,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ForInit {
    Variable(VariableDeclaration),
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ForHead {
    Variable(VariableDeclaration),
    Pattern(Pat),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarKind {
    Var,
    Let,
    Const,
    Using,
    AwaitUsing,
}

impl VarKind {
    pub fn as_str(self) -> &'static str {
        match self {
            VarKind::Var => "var",
            VarKind::Let => "let",
            VarKind::Const => "const",
            VarKind::Using => "using",
            VarKind::AwaitUsing => "await using",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct VariableDeclaration {
    pub span: Span,
    pub kind: VarKind,
    pub declarations: Vec<VariableDeclarator>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VariableDeclarator {
    pub span: Span,
    pub id: Pat,
    pub init: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub span: Span,
    pub id: Option<Ident>,
    pub params: Vec<Pat>,
    pub body: BlockStmt,
    pub is_generator: bool,
    pub is_async: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArrowBody {
    Expr(Box<Expr>),
    Block(BlockStmt),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Arrow {
    pub span: Span,
    pub params: Vec<Pat>,
    pub body: ArrowBody,
    pub is_async: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Class {
    pub span: Span,
    pub id: Option<Ident>,
    pub super_class: Option<Expr>,
    pub body_span: Span,
    pub body: Vec<ClassElement>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodKind {
    Constructor,
    Method,
    Get,
    Set,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClassElement {
    Method {
        span: Span,
        key: PropKey,
        value: Box<Function>,
        kind: MethodKind,
        is_static: bool,
    },
    Property {
        span: Span,
        key: PropKey,
        value: Option<Expr>,
        is_static: bool,
    },
    StaticBlock(BlockStmt),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModuleExportName {
    Ident(Ident),
    String(Literal),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportAttribute {
    pub span: Span,
    pub key: ModuleExportName,
    pub value: Literal,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ImportSpecifier {
    Default {
        span: Span,
        local: Ident,
    },
    Namespace {
        span: Span,
        local: Ident,
    },
    Named {
        span: Span,
        imported: ModuleExportName,
        local: Ident,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportDeclaration {
    pub span: Span,
    pub specifiers: Vec<ImportSpecifier>,
    pub source: Literal,
    pub attributes: Vec<ImportAttribute>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportSpecifier {
    pub span: Span,
    pub local: ModuleExportName,
    pub exported: ModuleExportName,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportNamed {
    pub span: Span,
    pub declaration: Option<Stmt>,
    pub specifiers: Vec<ExportSpecifier>,
    pub source: Option<Literal>,
    pub attributes: Vec<ImportAttribute>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExportDefaultKind {
    Function(Box<Function>),
    Class(Box<Class>),
    Expr(Expr),
}

// --- Expresiones ---

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Ident(Ident),
    Literal(Literal),
    This(Span),
    Super(Span),
    Array {
        span: Span,
        elements: Vec<Option<ExprOrSpread>>,
    },
    Object {
        span: Span,
        properties: Vec<ObjectMember>,
    },
    Function(Box<Function>),
    Arrow(Box<Arrow>),
    Class(Box<Class>),
    Template(Template),
    TaggedTemplate {
        span: Span,
        tag: Box<Expr>,
        quasi: Template,
    },
    Unary {
        span: Span,
        operator: &'static str,
        argument: Box<Expr>,
    },
    Update {
        span: Span,
        operator: &'static str,
        prefix: bool,
        argument: Box<Expr>,
    },
    Binary {
        span: Span,
        operator: &'static str,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Logical {
        span: Span,
        operator: &'static str,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Assign {
        span: Span,
        operator: &'static str,
        left: Box<Pat>,
        right: Box<Expr>,
    },
    Conditional {
        span: Span,
        test: Box<Expr>,
        consequent: Box<Expr>,
        alternate: Box<Expr>,
    },
    Call {
        span: Span,
        callee: Box<Expr>,
        arguments: Vec<ExprOrSpread>,
        optional: bool,
    },
    New {
        span: Span,
        callee: Box<Expr>,
        arguments: Vec<ExprOrSpread>,
    },
    Sequence {
        span: Span,
        expressions: Vec<Expr>,
    },
    Member {
        span: Span,
        object: Box<Expr>,
        property: MemberProp,
        optional: bool,
    },
    /// Una cadena opcional (`a?.b.c`): el `ChainExpression` de ESTree.
    Chain {
        span: Span,
        expression: Box<Expr>,
    },
    Yield {
        span: Span,
        argument: Option<Box<Expr>>,
        delegate: bool,
    },
    Await {
        span: Span,
        argument: Box<Expr>,
    },
    MetaProperty {
        span: Span,
        meta: Ident,
        property: Ident,
    },
    /// `import(source[, options])`.
    Import {
        span: Span,
        source: Box<Expr>,
        options: Option<Box<Expr>>,
    },
    /// `#x` a la izquierda de `in`.
    PrivateName(Ident),
    Paren {
        span: Span,
        expression: Box<Expr>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum MemberProp {
    Ident(Ident),
    Private(Ident),
    Computed(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExprOrSpread {
    pub spread: Option<Span>,
    pub expr: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    pub span: Span,
    pub quasis: Vec<TemplateElement>,
    pub expressions: Vec<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TemplateElement {
    pub span: Span,
    pub tail: bool,
    /// `None` si tiene un escape inválido (solo en templates con tag).
    pub cooked: Option<JsString>,
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PropKey {
    Ident(Ident),
    /// String o número.
    Literal(Literal),
    Computed(Box<Expr>),
    Private(Ident),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropKind {
    Init,
    Get,
    Set,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ObjectMember {
    Property {
        span: Span,
        key: PropKey,
        value: PropValue,
        kind: PropKind,
        method: bool,
        shorthand: bool,
    },
    Spread {
        span: Span,
        argument: Expr,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum PropValue {
    Expr(Expr),
    Method(Box<Function>),
    /// `{a = 1}` (CoverInitializedName): solo válido si el objeto se reinterpreta como
    /// patrón.
    CoverInit(Ident, Box<Expr>),
}

// --- Patrones ---

#[derive(Debug, Clone, PartialEq)]
pub enum Pat {
    Ident(Ident),
    Object {
        span: Span,
        properties: Vec<ObjectPatProp>,
    },
    Array {
        span: Span,
        elements: Vec<Option<Pat>>,
    },
    Rest {
        span: Span,
        argument: Box<Pat>,
    },
    Assign {
        span: Span,
        left: Box<Pat>,
        right: Box<Expr>,
    },
    /// Destino de asignación que no es un binding (`a.b`, `a[0]`, `(a)`).
    Expr(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ObjectPatProp {
    Property {
        span: Span,
        key: PropKey,
        value: Pat,
        shorthand: bool,
    },
    Rest {
        span: Span,
        argument: Pat,
    },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Ident(i) | Expr::PrivateName(i) => i.span,
            Expr::Literal(l) => l.span,
            Expr::This(s) | Expr::Super(s) => *s,
            Expr::Function(f) => f.span,
            Expr::Arrow(a) => a.span,
            Expr::Class(c) => c.span,
            Expr::Template(t) => t.span,
            Expr::Array { span, .. }
            | Expr::Object { span, .. }
            | Expr::TaggedTemplate { span, .. }
            | Expr::Unary { span, .. }
            | Expr::Update { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Logical { span, .. }
            | Expr::Assign { span, .. }
            | Expr::Conditional { span, .. }
            | Expr::Call { span, .. }
            | Expr::New { span, .. }
            | Expr::Sequence { span, .. }
            | Expr::Member { span, .. }
            | Expr::Chain { span, .. }
            | Expr::Yield { span, .. }
            | Expr::Await { span, .. }
            | Expr::MetaProperty { span, .. }
            | Expr::Import { span, .. }
            | Expr::Paren { span, .. } => *span,
        }
    }
}

impl Pat {
    pub fn span(&self) -> Span {
        match self {
            Pat::Ident(i) => i.span,
            Pat::Object { span, .. }
            | Pat::Array { span, .. }
            | Pat::Rest { span, .. }
            | Pat::Assign { span, .. } => *span,
            Pat::Expr(e) => e.span(),
        }
    }
}

impl Stmt {
    pub fn span(&self) -> Span {
        match self {
            Stmt::Block(b) => b.span,
            Stmt::Empty(s) | Stmt::Debugger(s) => *s,
            Stmt::Function(f) => f.span,
            Stmt::Variable(v) => v.span,
            Stmt::Class(c) => c.span,
            Stmt::Import(i) => i.span,
            Stmt::ExportNamed(e) => e.span,
            Stmt::Expression { span, .. }
            | Stmt::With { span, .. }
            | Stmt::Return { span, .. }
            | Stmt::Labeled { span, .. }
            | Stmt::Break { span, .. }
            | Stmt::Continue { span, .. }
            | Stmt::If { span, .. }
            | Stmt::Switch { span, .. }
            | Stmt::Throw { span, .. }
            | Stmt::Try { span, .. }
            | Stmt::While { span, .. }
            | Stmt::DoWhile { span, .. }
            | Stmt::For { span, .. }
            | Stmt::ForIn { span, .. }
            | Stmt::ForOf { span, .. }
            | Stmt::ExportDefault { span, .. }
            | Stmt::ExportAll { span, .. } => *span,
        }
    }
}

impl PropKey {
    /// El nombre estático de la clave (identificador o string; los números no), para
    /// detectar `__proto__`, `constructor` y `prototype`.
    pub fn static_name(&self) -> Option<String> {
        match self {
            PropKey::Ident(i) => Some(i.name.to_string()),
            PropKey::Literal(Literal {
                value: LitValue::String(s),
                ..
            }) => Some(s.to_string_lossy()),
            _ => None,
        }
    }
}
