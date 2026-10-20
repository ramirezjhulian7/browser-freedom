//! Módulos ECMAScript (§16.2.1): Module Records, el module map del host, la carga
//! (LoadRequestedModules), Link, Evaluate, GetModuleNamespace e `import.meta`.
//!
//! La carga es síncrona: el [`ModuleLoader`] del host resuelve y lee cada módulo en el
//! momento en que InnerModuleLoading lo pide, así que HostLoadImportedModule termina antes de
//! volver (lo permite §16.2.1.8: el host puede completar la carga de forma síncrona).
//!
//! Los bindings de import son indirectos (CreateImportBinding, §9.1.1.5.5): el slot del
//! Module Environment Record que importa guarda (módulo, slot) del binding exportado, y cada
//! lectura va al environment de ese módulo (el binding es "vivo").

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::compiler::bytecode::{Constant, Source};
use crate::compiler::module::{
    ExportImportName, ImportName, ModuleCode, ModuleRequest, compile_module,
};
use crate::heap::{Gc, Trace, Tracer};
use crate::object::Object;
use crate::parser::ParseOptions;
use crate::string::{Atom, JsString};
use crate::value::{PropertyKey, Value};
use crate::vm::env::{BindingKind, Env, ModuleEnvData, ScopeLayout};
use crate::vm::frame::Frame;
use crate::vm::{Agent, JsResult, RealmId, Throw};

pub type ModuleId = usize;

/// Los hooks del host para la carga de módulos (HostLoadImportedModule, §16.2.1.8).
pub trait ModuleLoader {
    /// La URL del módulo `specifier` pedido desde el módulo `referrer` (`None`: desde un
    /// script o el punto de entrada).
    fn resolve(&self, specifier: &str, referrer: Option<&str>) -> Result<String, String>;
    /// El código fuente del módulo `url`.
    fn load(&self, url: &str) -> Result<String, String>;
}

/// Un [`ModuleLoader`] sobre el sistema de archivos: las URL son `file:///…` y los
/// especificadores relativos (`./`, `../`, `/`) se resuelven contra el módulo que importa.
#[derive(Debug, Default, Clone, Copy)]
pub struct FileModuleLoader;

impl FileModuleLoader {
    /// La URL `file:` de una ruta (relativa al directorio actual si no es absoluta).
    pub fn path_to_url(path: &Path) -> String {
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|d| d.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        };
        let s = normalize_path(&abs.to_string_lossy().replace('\\', "/"));
        if s.starts_with('/') {
            format!("file://{s}")
        } else {
            format!("file:///{s}")
        }
    }

    /// La ruta de una URL `file:`.
    pub fn url_to_path(url: &str) -> Option<PathBuf> {
        let rest = url.strip_prefix("file://")?;
        // `file:///C:/x` → `C:/x`; `file:///home/x` → `/home/x`.
        let bytes = rest.as_bytes();
        let p = if bytes.len() > 3 && bytes[0] == b'/' && bytes[2] == b':' {
            &rest[1..]
        } else {
            rest
        };
        Some(PathBuf::from(p))
    }
}

/// Resuelve `.` y `..` de una ruta con `/`.
fn normalize_path(p: &str) -> String {
    let absolute = p.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                // No se sube por encima de la raíz (ni de la unidad `C:`).
                if parts.last().is_some_and(|l| !l.ends_with(':')) {
                    parts.pop();
                }
            }
            s => parts.push(s),
        }
    }
    let joined = parts.join("/");
    if absolute {
        format!("/{joined}")
    } else {
        joined
    }
}

impl ModuleLoader for FileModuleLoader {
    fn resolve(&self, specifier: &str, referrer: Option<&str>) -> Result<String, String> {
        if specifier.starts_with("file://") {
            let path = FileModuleLoader::url_to_path(specifier)
                .ok_or_else(|| format!("URL inválida: {specifier}"))?;
            return Ok(FileModuleLoader::path_to_url(&path));
        }
        if !(specifier.starts_with("./")
            || specifier.starts_with("../")
            || specifier.starts_with('/'))
        {
            return Err(format!(
                "no se puede resolver el especificador '{specifier}'"
            ));
        }
        let base = match referrer.and_then(FileModuleLoader::url_to_path) {
            Some(p) => p.parent().map(Path::to_path_buf).unwrap_or_default(),
            None => std::env::current_dir().unwrap_or_default(),
        };
        let path = if specifier.starts_with('/') {
            PathBuf::from(specifier)
        } else {
            base.join(specifier)
        };
        Ok(FileModuleLoader::path_to_url(&path))
    }

    fn load(&self, url: &str) -> Result<String, String> {
        let path =
            FileModuleLoader::url_to_path(url).ok_or_else(|| format!("URL inválida: {url}"))?;
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// `[[Status]]` de un Cyclic Module Record (§16.2.1.5, tabla 63).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleStatus {
    New,
    Unlinked,
    Linking,
    Linked,
    Evaluating,
    EvaluatingAsync,
    Evaluated,
}

pub enum ModuleKind {
    /// Source Text Module Record (§16.2.1.6).
    SourceText(Rc<ModuleCode>),
    /// Synthetic Module Record (§16.2.1.7) con un único export `default`: los módulos JSON
    /// (CreateDefaultExportSyntheticModule, §16.2.1.7.4).
    Synthetic { default: Value },
}

/// Un Module Record (§16.2.1.4) con los campos de Cyclic Module Record (§16.2.1.5).
pub struct ModuleRecord {
    pub realm: RealmId,
    /// `[[HostDefined]]`: la URL (también `import.meta.url`).
    pub url: String,
    pub kind: ModuleKind,
    pub environment: Option<Gc<Env>>,
    pub namespace: Option<Gc<Object>>,
    /// `[[ImportMeta]]`.
    pub import_meta: Option<Gc<Object>>,
    pub status: ModuleStatus,
    pub evaluation_error: Option<Value>,
    pub dfs_index: usize,
    pub dfs_ancestor_index: usize,
    /// `[[LoadedModules]]`, por índice de `[[RequestedModules]]`.
    pub loaded: Vec<Option<ModuleId>>,
    pub cycle_root: Option<ModuleId>,
}

impl ModuleRecord {
    fn code(&self) -> Option<Rc<ModuleCode>> {
        match &self.kind {
            ModuleKind::SourceText(c) => Some(c.clone()),
            ModuleKind::Synthetic { .. } => None,
        }
    }

    fn is_cyclic(&self) -> bool {
        matches!(self.kind, ModuleKind::SourceText(_))
    }
}

/// El estado de módulos del agente: los records, el module map y el loader del host.
#[derive(Default)]
pub struct ModuleHost {
    pub records: Vec<ModuleRecord>,
    /// (realm, URL, ¿JSON?) → módulo: HostLoadImportedModule devuelve el mismo Module
    /// Record para la misma petición (§16.2.1.8).
    map: HashMap<(RealmId, String, bool), ModuleId>,
    pub loader: Option<Rc<dyn ModuleLoader>>,
}

impl Trace for ModuleHost {
    fn trace(&self, t: &mut Tracer) {
        for r in &self.records {
            t.opt(r.environment);
            t.opt(r.namespace);
            t.opt(r.import_meta);
            t.opt_value(&r.evaluation_error);
            if let ModuleKind::Synthetic { default } = &r.kind {
                t.value(default);
            }
        }
    }
}

/// El nombre de un binding resuelto: `[[BindingName]]` de un ResolvedBinding Record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingName {
    Name(Atom),
    Namespace,
}

/// El resultado de ResolveExport (§16.2.1.6.3): `null`, `ambiguous` o un ResolvedBinding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Null,
    Ambiguous,
    Binding(ModuleId, BindingName),
}

/// Lo que mira una propiedad del namespace: un binding (módulo, slot) o otro namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NsTarget {
    Binding(ModuleId, u32),
    Namespace(ModuleId),
}

fn js(s: &str) -> JsString {
    JsString::from(s)
}

impl Agent {
    fn module_parse_options(&self) -> ParseOptions {
        ParseOptions {
            max_stack: self.options.max_stack.max(512 * 1024),
            ..ParseOptions::default()
        }
    }

    /// Instala el loader del host.
    pub fn set_module_loader(&mut self, loader: Rc<dyn ModuleLoader>) {
        self.modules.loader = Some(loader);
    }

    pub fn module(&self, m: ModuleId) -> &ModuleRecord {
        &self.modules.records[m]
    }

    /// ParseModule (§16.2.1.6.1) del texto de `url` en el realm actual, y su registro en el
    /// module map.
    pub fn parse_module_record(&mut self, url: &str, text: &str) -> JsResult<ModuleId> {
        let opts = self.module_parse_options();
        let program = match crate::parse_module(text, &opts) {
            Ok(p) => p,
            Err(e) => return self.syntax_error(&e.message),
        };
        let id = self.modules.records.len();
        let mut source = Source::new(url, text);
        source.module = Some(id);
        let code = compile_module(&program, Rc::new(source));
        let n = code.requested.len();
        self.push_module(url, ModuleKind::SourceText(Rc::new(code)), n, false)
    }

    /// ParseJSONModule (§16.2.1.7.3): un Synthetic Module Record cuyo `default` es el
    /// resultado de ParseJSON.
    pub fn parse_json_module(&mut self, url: &str, text: &str) -> JsResult<ModuleId> {
        let units: Vec<u16> = text.encode_utf16().collect();
        let value = crate::builtins::json::parse_json(self, &units)?;
        self.push_module(url, ModuleKind::Synthetic { default: value }, 0, true)
    }

    fn push_module(
        &mut self,
        url: &str,
        kind: ModuleKind,
        requests: usize,
        json: bool,
    ) -> JsResult<ModuleId> {
        let id = self.modules.records.len();
        let realm = self.current_realm;
        let status = if json {
            ModuleStatus::Unlinked
        } else {
            ModuleStatus::New
        };
        self.modules.records.push(ModuleRecord {
            realm,
            url: url.to_owned(),
            kind,
            environment: None,
            namespace: None,
            import_meta: None,
            status,
            evaluation_error: None,
            dfs_index: 0,
            dfs_ancestor_index: 0,
            loaded: vec![None; requests],
            cycle_root: None,
        });
        self.modules.map.insert((realm, url.to_owned(), json), id);
        Ok(id)
    }

    /// HostLoadImportedModule (§16.2.1.8) + FinishLoadingImportedModule (§16.2.1.9): el
    /// Module Record de `request` pedido desde `referrer` (síncrono).
    pub fn host_load_imported_module(
        &mut self,
        referrer: Option<ModuleId>,
        request: &ModuleRequest,
    ) -> JsResult<ModuleId> {
        let Some(loader) = self.modules.loader.clone() else {
            return self.type_error("No module loader");
        };
        let specifier = request.specifier.to_string_lossy();
        let referrer_url = referrer.map(|r| self.modules.records[r].url.clone());
        let url = match loader.resolve(&specifier, referrer_url.as_deref()) {
            Ok(u) => u,
            Err(e) => return self.type_error(&format!("Cannot resolve module: {e}")),
        };
        // El atributo `type` elige la clase de módulo: sin él, JavaScript; "json", JSON
        // (ES2025 §16.2.1.8 nota y la propuesta de módulos JSON).
        let json = match request.type_attribute() {
            None => false,
            Some(t) if t.to_string_lossy() == "json" => true,
            Some(t) => {
                let t = t.to_string_lossy();
                return self.type_error(&format!("Unsupported module type '{t}'"));
            }
        };
        let realm = self.current_realm;
        if let Some(&id) = self.modules.map.get(&(realm, url.clone(), json)) {
            return Ok(id);
        }
        let text = match loader.load(&url) {
            Ok(t) => t,
            Err(e) => return self.type_error(&format!("Cannot load module: {e}")),
        };
        if json {
            self.parse_json_module(&url, &text)
        } else {
            self.parse_module_record(&url, &text)
        }
    }

    /// LoadRequestedModules (§16.2.1.5.1): carga el grafo de `root` (síncrono).
    pub fn load_requested_modules(&mut self, root: ModuleId) -> JsResult<()> {
        let mut visited = Vec::new();
        self.inner_module_loading(root, &mut visited)?;
        // §16.2.1.5.1.1 paso 4: los visitados pasan a `unlinked`.
        for m in visited {
            if self.modules.records[m].status == ModuleStatus::New {
                self.modules.records[m].status = ModuleStatus::Unlinked;
            }
        }
        Ok(())
    }

    /// InnerModuleLoading (§16.2.1.5.1.1).
    fn inner_module_loading(&mut self, m: ModuleId, visited: &mut Vec<ModuleId>) -> JsResult<()> {
        let rec = &self.modules.records[m];
        let Some(code) = rec.code() else {
            return Ok(());
        };
        if rec.status != ModuleStatus::New || visited.contains(&m) {
            return Ok(());
        }
        visited.push(m);
        for (i, request) in code.requested.iter().enumerate() {
            // AllImportAttributesSupported (§16.2.1.5.1.1 paso 2.c.ii): el host solo entiende
            // `type`.
            if let Some((k, _)) = request
                .attributes
                .iter()
                .find(|(k, _)| k.to_string_lossy() != "type")
            {
                let k = k.to_string_lossy();
                return self.syntax_error(&format!("Unsupported import attribute '{k}'"));
            }
            let dep = match self.modules.records[m].loaded[i] {
                Some(d) => d,
                None => {
                    let d = self.host_load_imported_module(Some(m), request)?;
                    self.modules.records[m].loaded[i] = Some(d);
                    d
                }
            };
            self.inner_module_loading(dep, visited)?;
        }
        Ok(())
    }

    /// GetImportedModule (§16.2.1.9): el módulo cargado para la petición `i` (`[[LoadedModules]]`).
    fn imported(&self, m: ModuleId, i: usize) -> ModuleId {
        self.modules.records[m].loaded[i].expect("módulo cargado")
    }

    // --- Link (§16.2.1.5.2) ---

    /// Link() de un Cyclic Module Record (§16.2.1.5.2).
    pub fn link_module(&mut self, m: ModuleId) -> JsResult<()> {
        let mut stack = Vec::new();
        if let Err(e) = self.inner_module_linking(m, &mut stack, 0) {
            for s in stack {
                let r = &mut self.modules.records[s];
                r.status = ModuleStatus::Unlinked;
                r.environment = None;
            }
            return Err(e);
        }
        Ok(())
    }

    /// InnerModuleLinking (§16.2.1.5.2.1).
    fn inner_module_linking(
        &mut self,
        m: ModuleId,
        stack: &mut Vec<ModuleId>,
        mut index: usize,
    ) -> JsResult<usize> {
        if !self.modules.records[m].is_cyclic() {
            // Synthetic Module Record: Link() es InitializeEnvironment (§16.2.1.7.2.2).
            if self.modules.records[m].environment.is_none() {
                self.synthetic_initialize_environment(m);
            }
            return Ok(index);
        }
        match self.modules.records[m].status {
            ModuleStatus::Linking
            | ModuleStatus::Linked
            | ModuleStatus::EvaluatingAsync
            | ModuleStatus::Evaluated => return Ok(index),
            ModuleStatus::Unlinked => {}
            _ => return self.internal_error("Link de un módulo sin cargar"),
        }
        {
            let r = &mut self.modules.records[m];
            r.status = ModuleStatus::Linking;
            r.dfs_index = index;
            r.dfs_ancestor_index = index;
        }
        index += 1;
        stack.push(m);
        let n = self.modules.records[m].loaded.len();
        for i in 0..n {
            let required = self.imported(m, i);
            index = self.inner_module_linking(required, stack, index)?;
            let req = &self.modules.records[required];
            if req.is_cyclic() && req.status == ModuleStatus::Linking {
                let a = req.dfs_ancestor_index;
                let r = &mut self.modules.records[m];
                r.dfs_ancestor_index = r.dfs_ancestor_index.min(a);
            }
        }
        self.initialize_environment(m)?;
        let r = &self.modules.records[m];
        if r.dfs_ancestor_index == r.dfs_index {
            loop {
                let required = stack.pop().expect("pila de Link");
                self.modules.records[required].status = ModuleStatus::Linked;
                if required == m {
                    break;
                }
            }
        }
        Ok(index)
    }

    /// NewModuleEnvironment (§9.1.2.6) con el layout del módulo.
    fn new_module_environment(&mut self, realm: RealmId, layout: Rc<ScopeLayout>) -> Gc<Env> {
        let genv = self.realms[realm].global_env;
        let n = layout.names.len();
        let mut env = Env::declarative(Some(genv), layout);
        if let Some(d) = env.decl_mut() {
            d.module = Some(Box::new(ModuleEnvData {
                links: vec![None; n],
            }));
        }
        self.heap.alloc(env)
    }

    /// InitializeEnvironment de un Synthetic Module Record (§16.2.1.7.2): un binding mutable
    /// por export, inicializado en `undefined`.
    fn synthetic_initialize_environment(&mut self, m: ModuleId) {
        let layout = Rc::new(ScopeLayout {
            names: vec![Atom::new("default")],
            kinds: vec![BindingKind::Mutable],
            initialized: vec![true],
            simple_catch: false,
        });
        let realm = self.modules.records[m].realm;
        let env = self.new_module_environment(realm, layout);
        let r = &mut self.modules.records[m];
        r.environment = Some(env);
        r.status = ModuleStatus::Linked;
    }

    /// El SyntaxError de un import o re-export que no se resuelve.
    fn unresolvable<T>(&mut self, name: &JsString, r: &Resolution, url: &str) -> JsResult<T> {
        let name = name.to_string_lossy();
        if *r == Resolution::Ambiguous {
            self.syntax_error(&format!(
                "The requested module '{url}' contains conflicting star exports for name '{name}'"
            ))
        } else {
            self.syntax_error(&format!(
                "The requested module '{url}' does not provide an export named '{name}'"
            ))
        }
    }

    /// InitializeEnvironment de un Source Text Module Record (§16.2.1.6.4).
    fn initialize_environment(&mut self, m: ModuleId) -> JsResult<()> {
        let code = self.modules.records[m]
            .code()
            .expect("Source Text Module Record");
        // Pasos 1–2: los re-exports indirectos deben resolverse.
        for e in &code.indirect_exports {
            let name = e.export_name.clone().expect("ExportName");
            let r = self.resolve_export(m, &name, &mut Vec::new());
            if !matches!(r, Resolution::Binding(..)) {
                let url = self.modules.records[m].url.clone();
                return self.unresolvable(&name, &r, &url);
            }
        }
        // Pasos 4–6.
        let realm = self.modules.records[m].realm;
        let env = self.new_module_environment(realm, code.layout.clone());
        self.modules.records[m].environment = Some(env);
        // Paso 7: los imports.
        for ie in &code.imports {
            let imported = self.imported(m, ie.request);
            let slot = code
                .layout
                .position(&ie.local_name)
                .expect("binding de import") as u32;
            match &ie.import_name {
                ImportName::NamespaceObject => {
                    let ns = self.get_module_namespace(imported);
                    self.set_module_slot(env, slot, Value::Object(ns));
                }
                ImportName::Name(n) => {
                    let r = self.resolve_export(imported, n, &mut Vec::new());
                    match r {
                        Resolution::Binding(tm, BindingName::Namespace) => {
                            let ns = self.get_module_namespace(tm);
                            self.set_module_slot(env, slot, Value::Object(ns));
                        }
                        Resolution::Binding(tm, BindingName::Name(bn)) => {
                            let ts = self.module_binding_slot(tm, &bn);
                            if let Some(md) =
                                self.heap[env].decl_mut().and_then(|d| d.module.as_mut())
                            {
                                md.links[slot as usize] = Some((tm, ts));
                            }
                        }
                        other => {
                            let url = self.modules.records[imported].url.clone();
                            return self.unresolvable(n, &other, &url);
                        }
                    }
                }
            }
        }
        // Pasos 21–24: los `var` empiezan en `undefined` (el layout) y las funciones se
        // instancian ahora (InstantiateFunctionObject).
        let saved = self.current_realm;
        self.current_realm = realm;
        for &(slot, idx) in &code.functions {
            let Constant::Function(fc) = &code.code.consts[idx as usize] else {
                unreachable!("constante de función");
            };
            let f = self.make_closure(fc.clone(), env, None, None);
            self.set_module_slot(env, slot, Value::Object(f));
        }
        self.current_realm = saved;
        Ok(())
    }

    fn set_module_slot(&mut self, env: Gc<Env>, slot: u32, v: Value) {
        if let Some(d) = self.heap[env].decl_mut() {
            d.slots[slot as usize] = Some(v);
        }
    }

    /// El slot del binding `name` en el environment del módulo `m`.
    fn module_binding_slot(&self, m: ModuleId, name: &Atom) -> u32 {
        match &self.modules.records[m].kind {
            ModuleKind::SourceText(c) => c.layout.position(name).expect("binding exportado") as u32,
            ModuleKind::Synthetic { .. } => 0,
        }
    }

    // --- ResolveExport, GetExportedNames, GetModuleNamespace ---

    /// ResolveExport(exportName, resolveSet) (§16.2.1.6.3; Synthetic §16.2.1.7.2).
    pub fn resolve_export(
        &self,
        m: ModuleId,
        name: &JsString,
        resolve_set: &mut Vec<(ModuleId, JsString)>,
    ) -> Resolution {
        let rec = &self.modules.records[m];
        let Some(code) = rec.code() else {
            return if name.to_string_lossy() == "default" {
                Resolution::Binding(m, BindingName::Name(Atom::new("default")))
            } else {
                Resolution::Null
            };
        };
        // Pasos 2–3: un pedido circular.
        if resolve_set.iter().any(|(rm, rn)| *rm == m && rn == name) {
            return Resolution::Null;
        }
        resolve_set.push((m, name.clone()));
        for e in &code.local_exports {
            if e.export_name.as_ref() == Some(name) {
                return Resolution::Binding(
                    m,
                    BindingName::Name(e.local_name.clone().expect("LocalName")),
                );
            }
        }
        for e in &code.indirect_exports {
            if e.export_name.as_ref() == Some(name) {
                let Some(im) = rec.loaded[e.request.expect("ModuleRequest")] else {
                    return Resolution::Null;
                };
                return match e.import_name.as_ref().expect("ImportName") {
                    ExportImportName::All => Resolution::Binding(im, BindingName::Namespace),
                    ExportImportName::Name(n) => self.resolve_export(im, n, resolve_set),
                    ExportImportName::AllButDefault => Resolution::Null,
                };
            }
        }
        // Paso 6: `export *` no provee `default`.
        if name.to_string_lossy() == "default" {
            return Resolution::Null;
        }
        let mut star: Option<(ModuleId, BindingName)> = None;
        for e in &code.star_exports {
            let Some(im) = rec.loaded[e.request.expect("ModuleRequest")] else {
                continue;
            };
            match self.resolve_export(im, name, resolve_set) {
                Resolution::Ambiguous => return Resolution::Ambiguous,
                Resolution::Null => {}
                Resolution::Binding(rm, rb) => match &star {
                    None => star = Some((rm, rb)),
                    Some((sm, sb)) => {
                        if rm != *sm || rb != *sb {
                            return Resolution::Ambiguous;
                        }
                    }
                },
            }
        }
        match star {
            Some((sm, sb)) => Resolution::Binding(sm, sb),
            None => Resolution::Null,
        }
    }

    /// GetExportedNames(exportStarSet) (§16.2.1.6.2; Synthetic §16.2.1.7.2).
    pub fn get_exported_names(&self, m: ModuleId, star_set: &mut Vec<ModuleId>) -> Vec<JsString> {
        let rec = &self.modules.records[m];
        let Some(code) = rec.code() else {
            return vec![js("default")];
        };
        if star_set.contains(&m) {
            return Vec::new();
        }
        star_set.push(m);
        let mut names: Vec<JsString> = Vec::new();
        for e in code
            .local_exports
            .iter()
            .chain(code.indirect_exports.iter())
        {
            names.push(e.export_name.clone().expect("ExportName"));
        }
        for e in &code.star_exports {
            let Some(im) = rec.loaded[e.request.expect("ModuleRequest")] else {
                continue;
            };
            for n in self.get_exported_names(im, star_set) {
                if n.to_string_lossy() != "default" && !names.contains(&n) {
                    names.push(n);
                }
            }
        }
        names
    }

    /// GetModuleNamespace (§16.2.1.10).
    pub fn get_module_namespace(&mut self, m: ModuleId) -> Gc<Object> {
        if let Some(ns) = self.modules.records[m].namespace {
            return ns;
        }
        let names = self.get_exported_names(m, &mut Vec::new());
        let mut exports: Vec<(JsString, NsTarget)> = Vec::new();
        for n in names {
            match self.resolve_export(m, &n, &mut Vec::new()) {
                Resolution::Binding(rm, BindingName::Namespace) => {
                    exports.push((n, NsTarget::Namespace(rm)));
                }
                Resolution::Binding(rm, BindingName::Name(bn)) => {
                    let slot = self.module_binding_slot(rm, &bn);
                    exports.push((n, NsTarget::Binding(rm, slot)));
                }
                _ => {}
            }
        }
        // ModuleNamespaceCreate (§10.4.6.12): `[[Exports]]` ordenado por code units.
        exports.sort_by(|a, b| a.0.units().cmp(b.0.units()));
        let ns = crate::builtins::module_namespace::create(self, m, exports);
        self.modules.records[m].namespace = Some(ns);
        ns
    }

    /// El valor del binding indirecto del slot `slot` de `env`, si lo es: `Some(None)` si el
    /// binding exportado aún no se inicializó.
    pub(crate) fn module_link_value(&self, env: Gc<Env>, slot: u32) -> Option<Option<Value>> {
        let d = self.heap[env].decl()?;
        let (tm, ts) = (*d.module.as_ref()?.links.get(slot as usize)?)?;
        Some(self.module_slot_value(tm, ts))
    }

    /// El valor del slot `slot` del environment del módulo `m` (`None`: sin inicializar).
    pub(crate) fn module_slot_value(&self, m: ModuleId, slot: u32) -> Option<Value> {
        let env = self.modules.records.get(m)?.environment?;
        self.heap[env].decl()?.slots.get(slot as usize)?.clone()
    }

    // --- Evaluate (§16.2.1.5.3) ---

    /// Evaluate() de un Cyclic Module Record (§16.2.1.5.3). Sin Promise todavía, el
    /// resultado es la completion (la promesa que devolvería, ya resuelta o rechazada).
    pub fn evaluate_module(&mut self, m: ModuleId) -> JsResult<()> {
        let mut module = m;
        match self.modules.records[m].status {
            ModuleStatus::Linked => {}
            ModuleStatus::EvaluatingAsync | ModuleStatus::Evaluated => {
                module = self.modules.records[m].cycle_root.unwrap_or(m);
                return match self.modules.records[module].evaluation_error.clone() {
                    Some(e) => Err(Throw::Value(e)),
                    None => Ok(()),
                };
            }
            _ => return self.internal_error("Evaluate de un módulo sin enlazar"),
        }
        let mut stack = Vec::new();
        match self.inner_module_evaluation(module, &mut stack, 0) {
            Ok(_) => Ok(()),
            Err(e) => {
                if let Throw::Value(v) = &e {
                    for s in stack {
                        let r = &mut self.modules.records[s];
                        r.status = ModuleStatus::Evaluated;
                        r.evaluation_error = Some(v.clone());
                    }
                }
                Err(e)
            }
        }
    }

    /// InnerModuleEvaluation (§16.2.1.5.3.1).
    fn inner_module_evaluation(
        &mut self,
        m: ModuleId,
        stack: &mut Vec<ModuleId>,
        mut index: usize,
    ) -> JsResult<usize> {
        if !self.modules.records[m].is_cyclic() {
            // Synthetic: SetSyntheticModuleExport("default", json) (§16.2.1.7.4).
            if self.modules.records[m].status != ModuleStatus::Evaluated {
                if let ModuleKind::Synthetic { default } = &self.modules.records[m].kind {
                    let v = default.clone();
                    if let Some(env) = self.modules.records[m].environment {
                        self.set_module_slot(env, 0, v);
                    }
                }
                self.modules.records[m].status = ModuleStatus::Evaluated;
            }
            return Ok(index);
        }
        match self.modules.records[m].status {
            ModuleStatus::EvaluatingAsync | ModuleStatus::Evaluated => {
                return match self.modules.records[m].evaluation_error.clone() {
                    Some(e) => Err(Throw::Value(e)),
                    None => Ok(index),
                };
            }
            ModuleStatus::Evaluating => return Ok(index),
            ModuleStatus::Linked => {}
            _ => return self.internal_error("Evaluate de un módulo sin enlazar"),
        }
        {
            let r = &mut self.modules.records[m];
            r.status = ModuleStatus::Evaluating;
            r.dfs_index = index;
            r.dfs_ancestor_index = index;
        }
        index += 1;
        stack.push(m);
        let n = self.modules.records[m].loaded.len();
        for i in 0..n {
            let required = self.imported(m, i);
            index = self.inner_module_evaluation(required, stack, index)?;
            let req = &self.modules.records[required];
            if req.is_cyclic() {
                if req.status == ModuleStatus::Evaluating {
                    let a = req.dfs_ancestor_index;
                    let r = &mut self.modules.records[m];
                    r.dfs_ancestor_index = r.dfs_ancestor_index.min(a);
                } else {
                    let root = req.cycle_root.unwrap_or(required);
                    if let Some(e) = self.modules.records[root].evaluation_error.clone() {
                        return Err(Throw::Value(e));
                    }
                }
            }
        }
        let code = self.modules.records[m]
            .code()
            .expect("Source Text Module Record");
        if code.has_tla {
            // ExecuteAsyncModule necesita Promise (llega con E3).
            return self.internal_error("top-level await (requiere Promise)");
        }
        self.execute_module(m, &code)?;
        let r = &self.modules.records[m];
        if r.dfs_ancestor_index == r.dfs_index {
            loop {
                let required = stack.pop().expect("pila de Evaluate");
                let r = &mut self.modules.records[required];
                r.status = ModuleStatus::Evaluated;
                r.cycle_root = Some(m);
                if required == m {
                    break;
                }
            }
        }
        Ok(index)
    }

    /// ExecuteModule (§16.2.1.6.5): el cuerpo con el Module Environment Record como
    /// LexicalEnvironment y VariableEnvironment, y `this` = `undefined`.
    fn execute_module(&mut self, m: ModuleId, code: &ModuleCode) -> JsResult<()> {
        let rec = &self.modules.records[m];
        let env = rec.environment.expect("environment del módulo");
        let mut frame = Frame::new(code.code.clone(), env, rec.realm);
        frame.this = Some(Value::Undefined);
        frame.var_env = Some(env);
        self.run_frame(frame)?;
        Ok(())
    }

    // --- import.meta ---

    /// `import.meta` (§13.3.12.1): el objeto del módulo activo, creado la primera vez con
    /// las propiedades de HostGetImportMetaProperties (`url`).
    pub(crate) fn import_meta(&mut self) -> JsResult<Value> {
        let Some(m) = self.frames.last().and_then(|f| f.code.source.module) else {
            return self.syntax_error("Cannot use 'import.meta' outside a module");
        };
        if let Some(o) = self.modules.records[m].import_meta {
            return Ok(Value::Object(o));
        }
        let o = self.new_object(None);
        let url = js(&self.modules.records[m].url);
        self.create_data_property_or_throw(o, PropertyKey::from("url"), Value::String(url))?;
        self.modules.records[m].import_meta = Some(o);
        Ok(Value::Object(o))
    }

    // --- Host ---

    /// Carga (con el loader instalado), enlaza y evalúa el módulo `url`.
    pub fn run_module(&mut self, url: &str) -> JsResult<ModuleId> {
        let Some(loader) = self.modules.loader.clone() else {
            return self.type_error("No module loader");
        };
        let text = match loader.load(url) {
            Ok(t) => t,
            Err(e) => return self.type_error(&format!("Cannot load module: {e}")),
        };
        let m = self.parse_module_record(url, &text)?;
        self.load_requested_modules(m)?;
        self.link_module(m)?;
        self.evaluate_module(m)?;
        Ok(m)
    }
}
