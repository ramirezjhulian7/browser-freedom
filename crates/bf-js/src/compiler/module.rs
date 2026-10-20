//! Compilación de módulos: lo que ParseModule (§16.2.1.6.1) guarda en el Source Text Module
//! Record (`[[RequestedModules]]`, `[[ImportEntries]]`, `[[LocalExportEntries]]`,
//! `[[IndirectExportEntries]]`, `[[StarExportEntries]]`, `[[HasTLA]]`) más el código del
//! cuerpo y las funciones que InitializeEnvironment (§16.2.1.6.4) instancia.
//!
//! El top-level del módulo es un único scope ([`ScopeKind::Module`]) cuyos bindings viven
//! todos en el Module Environment Record: los importan otros módulos y el namespace.

use std::rc::Rc;

use super::bytecode::*;
use super::scope::*;
use super::{Codegen, FnState, NameSource, Target};
use crate::ast::*;
use crate::string::{Atom, JsString};
use crate::vm::env::ScopeLayout;

/// ModuleRequest Record (§16.2.1.3.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleRequest {
    pub specifier: JsString,
    /// `[[Attributes]]` (clave, valor), en el orden del código.
    pub attributes: Vec<(JsString, JsString)>,
}

impl ModuleRequest {
    /// ModuleRequestsEqual (§16.2.1.3.2): el mismo especificador y los mismos atributos, en
    /// cualquier orden.
    pub fn equals(&self, other: &ModuleRequest) -> bool {
        self.specifier == other.specifier
            && self.attributes.len() == other.attributes.len()
            && self
                .attributes
                .iter()
                .all(|a| other.attributes.iter().any(|b| a == b))
    }

    /// El valor del atributo `type`, si lo hay.
    pub fn type_attribute(&self) -> Option<&JsString> {
        self.attributes
            .iter()
            .find(|(k, _)| k.to_string_lossy() == "type")
            .map(|(_, v)| v)
    }
}

/// `[[ImportName]]` de un ImportEntry Record (§16.2.1.6, tabla 61).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportName {
    Name(JsString),
    NamespaceObject,
}

/// ImportEntry Record.
#[derive(Debug, Clone)]
pub struct ImportEntry {
    /// Índice en `requested`.
    pub request: usize,
    pub import_name: ImportName,
    pub local_name: Atom,
}

/// `[[ImportName]]` de un ExportEntry Record (§16.2.1.6, tabla 62).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportImportName {
    Name(JsString),
    All,
    AllButDefault,
}

/// ExportEntry Record.
#[derive(Debug, Clone)]
pub struct ExportEntry {
    pub export_name: Option<JsString>,
    /// Índice en `requested`.
    pub request: Option<usize>,
    pub import_name: Option<ExportImportName>,
    pub local_name: Option<Atom>,
}

/// Un módulo compilado.
#[derive(Debug)]
pub struct ModuleCode {
    /// El cuerpo (ExecuteModule, §16.2.1.6.5): corre con el Module Environment Record como
    /// LexicalEnvironment.
    pub code: Rc<FunctionCode>,
    pub requested: Vec<ModuleRequest>,
    pub imports: Vec<ImportEntry>,
    pub local_exports: Vec<ExportEntry>,
    pub indirect_exports: Vec<ExportEntry>,
    pub star_exports: Vec<ExportEntry>,
    /// El layout del Module Environment Record.
    pub layout: Rc<ScopeLayout>,
    /// Las declaraciones de función del top-level: (slot, constante `Function` de `code`).
    pub functions: Vec<(u32, u32)>,
    pub has_tla: bool,
}

/// El StringValue de un ModuleExportName.
pub fn export_name(n: &ModuleExportName) -> JsString {
    match n {
        ModuleExportName::Ident(i) => i.name.to_js_string(),
        ModuleExportName::String(l) => match &l.value {
            LitValue::String(s) => s.clone(),
            _ => JsString::default(),
        },
    }
}

fn literal_string(l: &Literal) -> JsString {
    match &l.value {
        LitValue::String(s) => s.clone(),
        _ => JsString::default(),
    }
}

/// El ModuleRequest de un FromClause con sus WithClause (§16.2.1.3.1 ModuleRequests).
fn request_of(source: &Literal, attributes: &[ImportAttribute]) -> ModuleRequest {
    ModuleRequest {
        specifier: literal_string(source),
        attributes: attributes
            .iter()
            .map(|a| (export_name(&a.key), literal_string(&a.value)))
            .collect(),
    }
}

/// El índice de `r` en `requested` (agregándolo si no está: ModuleRequests no repite).
fn request_index(requested: &mut Vec<ModuleRequest>, r: ModuleRequest) -> usize {
    match requested.iter().position(|x| x.equals(&r)) {
        Some(i) => i,
        None => {
            requested.push(r);
            requested.len() - 1
        }
    }
}

fn default_name() -> Atom {
    Atom::new("*default*")
}

/// ParseModule (§16.2.1.6.1) de un programa ya parseado como Module.
pub fn compile_module(program: &Program, source: Rc<Source>) -> ModuleCode {
    let tree = Analyzer::analyze_module(program);
    let mut cg = Codegen {
        tree: &tree,
        source,
        fs: Vec::new(),
        method_src: None,
    };
    let (code, functions) = cg.module_body(program);
    let top = tree
        .scope(key(program.span, Tag::Program))
        .expect("scope del módulo");
    let layout = tree.scopes[top].layout.clone();

    let mut requested = Vec::new();
    let mut imports = Vec::new();
    // ExportEntries (§16.2.3.2) antes de clasificarlas.
    let mut exports: Vec<ExportEntry> = Vec::new();
    let local = |name: Atom, export: JsString| ExportEntry {
        export_name: Some(export),
        request: None,
        import_name: None,
        local_name: Some(name),
    };
    for s in &program.body {
        match s {
            Stmt::Import(i) => {
                let r = request_index(&mut requested, request_of(&i.source, &i.attributes));
                for spec in &i.specifiers {
                    let (import_name, local_name) = match spec {
                        ImportSpecifier::Default { local, .. } => {
                            (ImportName::Name(JsString::from("default")), local)
                        }
                        ImportSpecifier::Namespace { local, .. } => {
                            (ImportName::NamespaceObject, local)
                        }
                        ImportSpecifier::Named {
                            imported, local, ..
                        } => (ImportName::Name(export_name(imported)), local),
                    };
                    imports.push(ImportEntry {
                        request: r,
                        import_name,
                        local_name: local_name.name.clone(),
                    });
                }
            }
            Stmt::ExportAll {
                exported,
                source,
                attributes,
                ..
            } => {
                let r = request_index(&mut requested, request_of(source, attributes));
                exports.push(match exported {
                    // export * from M
                    None => ExportEntry {
                        export_name: None,
                        request: Some(r),
                        import_name: Some(ExportImportName::AllButDefault),
                        local_name: None,
                    },
                    // export * as ns from M
                    Some(n) => ExportEntry {
                        export_name: Some(export_name(n)),
                        request: Some(r),
                        import_name: Some(ExportImportName::All),
                        local_name: None,
                    },
                });
            }
            Stmt::ExportNamed(e) => {
                if let Some(d) = &e.declaration {
                    let mut names = Vec::new();
                    match d {
                        Stmt::Variable(v) => {
                            for dec in &v.declarations {
                                bound_names(&dec.id, &mut names);
                            }
                        }
                        Stmt::Function(f) => names.extend(f.id.as_ref().map(|i| i.name.clone())),
                        Stmt::Class(c) => names.extend(c.id.as_ref().map(|i| i.name.clone())),
                        _ => {}
                    }
                    for n in names {
                        let ex = n.to_js_string();
                        exports.push(local(n, ex));
                    }
                } else if let Some(src) = &e.source {
                    let r = request_index(&mut requested, request_of(src, &e.attributes));
                    for spec in &e.specifiers {
                        exports.push(ExportEntry {
                            export_name: Some(export_name(&spec.exported)),
                            request: Some(r),
                            import_name: Some(ExportImportName::Name(export_name(&spec.local))),
                            local_name: None,
                        });
                    }
                } else {
                    for spec in &e.specifiers {
                        let local_name = match &spec.local {
                            ModuleExportName::Ident(i) => i.name.clone(),
                            ModuleExportName::String(l) => {
                                Atom::new(&literal_string(l).to_string_lossy())
                            }
                        };
                        exports.push(local(local_name, export_name(&spec.exported)));
                    }
                }
            }
            Stmt::ExportDefault { declaration, .. } => {
                let n = match declaration {
                    ExportDefaultKind::Function(f) => f.id.as_ref().map(|i| i.name.clone()),
                    ExportDefaultKind::Class(c) => c.id.as_ref().map(|i| i.name.clone()),
                    ExportDefaultKind::Expr(_) => None,
                };
                exports.push(local(
                    n.unwrap_or_else(default_name),
                    JsString::from("default"),
                ));
            }
            _ => {}
        }
    }

    // ParseModule pasos 10–11: clasificación de las ExportEntries.
    let mut local_exports = Vec::new();
    let mut indirect_exports = Vec::new();
    let mut star_exports = Vec::new();
    for ee in exports {
        match ee.request {
            None => {
                let ln = ee.local_name.clone().expect("LocalName");
                match imports.iter().find(|ie: &&ImportEntry| ie.local_name == ln) {
                    None => local_exports.push(ee),
                    Some(ie) => match &ie.import_name {
                        // La re-exportación de un namespace importado es un re-export
                        // indirecto de todo el módulo (ImportName `all`): resuelve al
                        // namespace de ese módulo y no es ambiguo si se repite (ES2025).
                        ImportName::NamespaceObject => indirect_exports.push(ExportEntry {
                            export_name: ee.export_name,
                            request: Some(ie.request),
                            import_name: Some(ExportImportName::All),
                            local_name: None,
                        }),
                        ImportName::Name(n) => indirect_exports.push(ExportEntry {
                            export_name: ee.export_name,
                            request: Some(ie.request),
                            import_name: Some(ExportImportName::Name(n.clone())),
                            local_name: None,
                        }),
                    },
                }
            }
            Some(_) => {
                if ee.import_name == Some(ExportImportName::AllButDefault) {
                    star_exports.push(ee);
                } else {
                    indirect_exports.push(ee);
                }
            }
        }
    }
    let has_tla = code.is_async;
    ModuleCode {
        code: Rc::new(code),
        requested,
        imports,
        local_exports,
        indirect_exports,
        star_exports,
        layout,
        functions,
        has_tla,
    }
}

impl<'t> Codegen<'t> {
    /// El cuerpo del módulo. Las declaraciones de función del top-level no generan código:
    /// las instancia InitializeEnvironment (se devuelven como (slot, constante)).
    fn module_body(&mut self, program: &Program) -> (FunctionCode, Vec<(u32, u32)>) {
        let top = self
            .tree
            .scope(key(program.span, Tag::Program))
            .expect("scope del módulo");
        let locals = self.tree.funcs[0].num_locals;
        self.fs
            .push(FnState::new(0, CodeKind::Script, true, top, locals));
        // InitializeEnvironment paso 24 (las funciones de LexicallyScopedDeclarations).
        let mut functions = Vec::new();
        for s in &program.body {
            let (f, name) = match s {
                Stmt::Function(f) => (&**f, None),
                Stmt::ExportNamed(e) => match &e.declaration {
                    Some(Stmt::Function(f)) => (&**f, None),
                    _ => continue,
                },
                Stmt::ExportDefault {
                    declaration: ExportDefaultKind::Function(f),
                    ..
                } => (
                    &**f,
                    // §16.2.3.7: `export default function () {}` se llama "default".
                    f.id.is_none()
                        .then(|| NameSource::Const(JsString::from("default"))),
                ),
                _ => continue,
            };
            let binding = f.id.as_ref().map_or_else(default_name, |i| i.name.clone());
            let idx = self.function(f, CodeKind::Normal, name);
            match self.lookup(&binding) {
                Target::Env { slot, .. } => functions.push((slot, idx)),
                other => unreachable!("binding de módulo fuera del environment: {other:?}"),
            }
        }
        for s in &program.body {
            match s {
                Stmt::Function(_) | Stmt::Import(_) | Stmt::ExportAll { .. } => {}
                Stmt::ExportNamed(e) => {
                    if let Some(d) = &e.declaration
                        && !matches!(d, Stmt::Function(_))
                    {
                        self.stmt(d);
                    }
                }
                Stmt::ExportDefault { declaration, .. } => match declaration {
                    ExportDefaultKind::Function(_) => {}
                    ExportDefaultKind::Class(c) => {
                        // §16.2.3.7: la clase anónima se llama "default".
                        let name =
                            c.id.is_none()
                                .then(|| NameSource::Const(JsString::from("default")));
                        self.class(c, name);
                        let binding = c.id.as_ref().map_or_else(default_name, |i| i.name.clone());
                        self.emit_init_lexical(&binding);
                    }
                    ExportDefaultKind::Expr(e) => {
                        // §16.2.3.7: NamedEvaluation con "default" si es una definición de
                        // función anónima; después InitializeBoundName("*default*").
                        self.expr_named(e, Some(NameSource::Const(JsString::from("default"))));
                        self.emit_init_lexical("*default*");
                    }
                },
                other => self.stmt(other),
            }
        }
        self.emit(Op::Undefined);
        self.emit(Op::Return);
        let code = self.finish_fn(JsString::from("<module>"), program.span, 0);
        (code, functions)
    }
}
