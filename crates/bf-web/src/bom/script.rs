//! La ejecución de los scripts del documento (HTML §4.12.1 y §8.1) y `document.write()`,
//! `writeln()`, `open()` y `close()` (§8.4).
//!
//! Mientras se parsea, el dueño del documento es el [`Parser`], guardado en el
//! [`crate::WebContext`] como [`DocumentOwner::Parser`]. Cada llamada al parser (feed, finish,
//! write…) lo saca del contexto, y [`WebScripts::execute`] lo vuelve a poner mientras corre el
//! script: así las funciones nativas (el DOM, `document.write`) siempre lo encuentran ahí, y
//! `document.write` puede volver a entrar al parser (scripts anidados).
//!
//! Los scripts externos se buscan en el momento con el [`ResourceLoader`] del documento (el
//! hilo del documento puede bloquearse): los bloqueantes corren en orden, los `defer` al
//! terminar el parseo y los `async` como tareas. Los módulos cargan su grafo con
//! [`WebModuleLoader`] (red e *import maps*).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bf_dom::script::ScriptType;
use bf_dom::{Document, NodeId};
use bf_html::{ParseOptions, Parser, ScriptDisposition};
use bf_infra::encoding::{self, Encoding};
use bf_infra::{DomString, ResourceLoader, Url};
use bf_js::host::{ConsoleLevel, TaskSource};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::module::ModuleLoader;
use bf_js::vm::{Agent, JsResult, RealmId, Throw};

use super::document::this_document;
use crate::{DocumentOwner, WebContextHandle};

/// Cuánto puede correr un script antes de que el motor lo corte.
pub const SCRIPT_DEADLINE: Duration = Duration::from_secs(10);

/// Las opciones del parser de los documentos con scripts.
fn parse_options() -> ParseOptions {
    ParseOptions {
        scripting: true,
        iframe_srcdoc: false,
    }
}

/// Un *import map* (HTML §8.1.5.3): especificador (o prefijo terminado en `/`) → URL.
#[derive(Debug, Clone, Default)]
pub struct ImportMap {
    pub imports: Vec<(String, Url)>,
}

impl ImportMap {
    /// "Resolve an imports match" (HTML §8.1.5.3.6): la entrada exacta o el prefijo más largo.
    pub fn resolve(&self, specifier: &str) -> Option<Url> {
        if let Some((_, url)) = self.imports.iter().find(|(k, _)| k == specifier) {
            return Some(url.clone());
        }
        let (key, url) = self
            .imports
            .iter()
            .filter(|(k, _)| k.ends_with('/') && specifier.starts_with(k.as_str()))
            .max_by_key(|(k, _)| k.len())?;
        Url::parse(&specifier[key.len()..], Some(url)).ok()
    }
}

/// El cargador de módulos del documento: resuelve con el import map y la URL del que importa
/// (HTML §8.1.5.5 "resolve a module specifier") y lee con el `ResourceLoader`.
pub struct WebModuleLoader {
    loader: Arc<dyn ResourceLoader>,
    base: Rc<RefCell<Url>>,
    import_map: Rc<RefCell<ImportMap>>,
}

impl ModuleLoader for WebModuleLoader {
    fn resolve(&self, specifier: &str, referrer: Option<&str>) -> Result<String, String> {
        let base = referrer
            .and_then(|r| Url::parse(r, None).ok())
            .unwrap_or_else(|| self.base.borrow().clone());
        if let Some(url) = self.import_map.borrow().resolve(specifier) {
            return Ok(url.href());
        }
        let url_like = specifier.starts_with('/')
            || specifier.starts_with("./")
            || specifier.starts_with("../");
        let url = if url_like {
            Url::parse(specifier, Some(&base)).ok()
        } else {
            Url::parse(specifier, None).ok()
        };
        url.map(|u| u.href()).ok_or_else(|| {
            format!(
                "Failed to resolve module specifier \"{specifier}\": relative references must \
                 start with \"/\", \"./\" or \"../\""
            )
        })
    }

    fn load(&self, url: &str) -> Result<String, String> {
        let parsed = Url::parse(url, None).map_err(|_| format!("URL inválida: {url}"))?;
        let res = self
            .loader
            .load(&parsed)
            .ok_or_else(|| format!("no se pudo cargar {url}"))?;
        Ok(encoding::decode(&res.bytes, Encoding::Utf8).0)
    }
}

/// Un script externo ya buscado.
struct Fetched {
    source: String,
    url: String,
}

/// El estado de la ejecución de scripts de una ventana (dato del realm).
pub struct ScriptEnv {
    loader: Arc<dyn ResourceLoader>,
    base: Rc<RefCell<Url>>,
    import_map: Rc<RefCell<ImportMap>>,
    fetched: HashMap<NodeId, Fetched>,
    inline_modules: usize,
    /// Scripts ejecutados y cuántos terminaron con una excepción.
    pub executed: usize,
    pub failed: usize,
}

/// Prepara `realm` (ya con su [`crate::WebContext`]) para ejecutar los scripts del documento
/// en `url`, que carga sus recursos con `loader`.
pub fn install_scripting(
    agent: &mut Agent,
    realm: RealmId,
    url: &Url,
    loader: Arc<dyn ResourceLoader>,
) {
    let base = Rc::new(RefCell::new(url.clone()));
    let import_map = Rc::new(RefCell::new(ImportMap::default()));
    agent.set_module_loader(Rc::new(WebModuleLoader {
        loader: loader.clone(),
        base: base.clone(),
        import_map: import_map.clone(),
    }));
    agent.set_realm_data(
        realm,
        ScriptEnv {
            loader,
            base,
            import_map,
            fetched: HashMap::new(),
            inline_modules: 0,
            executed: 0,
            failed: 0,
        },
    );
}

/// El estado de scripts de `realm`.
pub fn script_env(agent: &Agent, realm: RealmId) -> Option<&ScriptEnv> {
    agent.realm_data::<ScriptEnv>(realm)
}

fn env_mut(agent: &mut Agent, realm: RealmId) -> Option<&mut ScriptEnv> {
    agent.realm_data_mut::<ScriptEnv>(realm)
}

/// Saca el parser del documento de la ventana (lo crea sobre el documento si no había); el
/// contexto queda con un documento vacío hasta [`put_parser`].
fn take_parser(agent: &mut Agent, realm: RealmId) -> Parser {
    let ctx = WebContextHandle { realm }.get_mut(agent);
    let owner = std::mem::replace(
        &mut ctx.documents[0],
        DocumentOwner::Document(Box::default()),
    );
    match owner {
        DocumentOwner::Parser(p) => *p,
        DocumentOwner::Document(d) => Parser::from_document(*d, &parse_options()),
    }
}

/// Devuelve el parser al contexto.
fn put_parser(agent: &mut Agent, realm: RealmId, parser: Parser) {
    let ctx = WebContextHandle { realm }.get_mut(agent);
    ctx.documents[0] = DocumentOwner::Parser(Box::new(parser));
}

/// Corre `f` con el parser de la ventana y un [`WebScripts`].
pub fn with_parser<R>(
    agent: &mut Agent,
    realm: RealmId,
    f: impl FnOnce(&mut Parser, &mut WebScripts<'_>) -> R,
) -> R {
    let mut parser = take_parser(agent, realm);
    let r = {
        let mut host = WebScripts { agent, realm };
        f(&mut parser, &mut host)
    };
    put_parser(agent, realm, parser);
    r
}

/// Empieza a parsear el documento de la ventana de `realm` (en `url`): el contexto pasa a
/// tener un parser de red vacío. El evento `load` espera a [`resources_loaded`].
pub fn begin_parse(agent: &mut Agent, realm: RealmId, url: &Url) {
    let mut parser = Parser::new(&parse_options());
    parser.delay_load_event();
    parser.document_mut().url = url.clone();
    let ctx = WebContextHandle { realm }.get_mut(agent);
    ctx.url = url.clone();
    ctx.base = url.clone();
    ctx.documents[0] = DocumentOwner::Parser(Box::new(parser));
    if let Some(env) = env_mut(agent, realm) {
        *env.base.borrow_mut() = url.clone();
    }
}

/// Datos de red para el parser de la ventana.
pub fn feed(agent: &mut Agent, realm: RealmId, text: &str) {
    with_parser(agent, realm, |p, host| p.feed(text, host));
}

/// Fin de los datos de red: parsea el resto y corre "the end" (los scripts externos ya
/// están, así que termina con `load`).
pub fn finish(agent: &mut Agent, realm: RealmId) {
    with_parser(agent, realm, |p, host| {
        p.finish(host);
        p.run_tasks(host);
    });
    super::performance::document_loaded(agent, realm);
}

/// Las tareas pendientes del parser (scripts async ya listos, "the end").
pub fn run_parser_tasks(agent: &mut Agent, realm: RealmId) {
    with_parser(agent, realm, |p, host| p.run_tasks(host));
}

/// El [`bf_html::ScriptHost`] de la ventana: busca y ejecuta los scripts con el motor.
pub struct WebScripts<'a> {
    pub agent: &'a mut Agent,
    pub realm: RealmId,
}

impl WebScripts<'_> {
    fn console_error(&mut self, text: String) {
        self.agent.host.hooks.console(ConsoleLevel::Error, &text);
    }

    fn fetch(&mut self, url: &Url, charset: &str) -> Option<Fetched> {
        let loader = script_env(self.agent, self.realm)?.loader.clone();
        let res = loader.load(url)?;
        let fallback = Encoding::for_label(charset).unwrap_or(Encoding::Utf8);
        let (source, _) = encoding::decode(&res.bytes, fallback);
        Some(Fetched {
            source,
            url: res.url.href(),
        })
    }

    /// Corre `f` con el límite de tiempo de un script y el realm de la ventana.
    fn guarded<T>(&mut self, f: impl FnOnce(&mut Agent) -> T) -> T {
        let agent = &mut *self.agent;
        let saved_deadline = agent.options.deadline;
        let saved_realm = agent.current_realm;
        if agent.frames.is_empty() {
            agent.options.deadline = Some(Instant::now() + SCRIPT_DEADLINE);
        }
        agent.current_realm = self.realm;
        let r = f(agent);
        agent.current_realm = saved_realm;
        agent.options.deadline = saved_deadline;
        r
    }

    /// Una excepción sin atrapar de un script: a la consola ("report an exception").
    fn report(&mut self, t: Throw) {
        if let Some(env) = env_mut(self.agent, self.realm) {
            env.failed += 1;
        }
        match t {
            Throw::Value(v) => self.agent.report_exception(&v),
            Throw::Terminate(reason) => self.console_error(format!("Script terminado: {reason}")),
        }
    }

    /// "Clean up after running script": con la pila vacía, los microtasks.
    fn clean_up(&mut self) {
        if self.agent.frames.is_empty()
            && let Err(t) = self.agent.perform_microtask_checkpoint()
        {
            self.report(t);
        }
    }

    /// Ejecuta un script clásico.
    pub fn run_classic(&mut self, source: &str, url: &str) {
        if let Some(env) = env_mut(self.agent, self.realm) {
            env.executed += 1;
        }
        let r = self.guarded(|agent| {
            agent
                .compile_script_text(source, url)
                .and_then(|code| agent.script_evaluation(code))
        });
        if let Err(t) = r {
            self.report(t);
        }
        self.clean_up();
    }

    /// Ejecuta un módulo (su grafo se carga en el momento).
    pub fn run_module(&mut self, source: &str, url: &str) {
        if let Some(env) = env_mut(self.agent, self.realm) {
            env.executed += 1;
        }
        let r = self.guarded(|agent| {
            let m = agent.parse_module_record(url, source)?;
            agent.load_requested_modules(m)?;
            agent.link_module(m)?;
            let p = agent.evaluate_module(m)?;
            agent.run_jobs()?;
            match agent.promise_outcome(p) {
                Some(Err(v)) => Err(Throw::Value(v)),
                _ => Ok(()),
            }
        });
        if let Err(t) = r {
            self.report(t);
        }
        self.clean_up();
    }

    /// "Parse an import map string" (HTML §8.1.5.3.2): solo `imports`.
    pub fn register_import_map(&mut self, text: &str, base: &Url) {
        let r = self.guarded(|agent| -> JsResult<Vec<(String, String)>> {
            let json = crate::bom::script::json_parse(agent, text)?;
            let Value::Object(map) = json else {
                return Ok(Vec::new());
            };
            let imports = agent.get_str(map, "imports")?;
            let Value::Object(imports) = imports else {
                return Ok(Vec::new());
            };
            let keys = agent.own_property_keys(imports)?;
            let mut out = Vec::new();
            for k in keys {
                if let PropertyKey::String(name) = &k {
                    let v = agent.get(imports, &k, Value::Object(imports))?;
                    if let Value::String(s) = v {
                        out.push((name.to_string_lossy(), s.to_string_lossy()));
                    }
                }
            }
            Ok(out)
        });
        match r {
            Ok(entries) => {
                let Some(env) = env_mut(self.agent, self.realm) else {
                    return;
                };
                let mut map = env.import_map.borrow_mut();
                for (k, v) in entries {
                    if let Ok(url) = Url::parse(&v, Some(base)) {
                        map.imports.push((k, url));
                    }
                }
            }
            Err(t) => self.report(t),
        }
    }
}

/// `JSON.parse(text)` con el `JSON` del realm.
fn json_parse(agent: &mut Agent, text: &str) -> JsResult<Value> {
    let g = agent.realms[agent.current_realm].global_object;
    let json = agent.get_str(g, "JSON")?;
    let Value::Object(json) = json else {
        return agent.type_error("JSON no está definido");
    };
    let parse = agent.get_str(json, "parse")?;
    agent.call(&parse, Value::Object(json), &[Value::str(text)])
}

/// Presta `doc` al contexto (como `window.document`) mientras corre el listener `handle`, que
/// llama el DOM de E3 (dueño de `addEventListener`).
fn lend_and_invoke(
    agent: &mut Agent,
    realm: RealmId,
    doc: &mut Document,
    handle: u64,
    event: &mut bf_dom::events::Event,
) -> bool {
    let real = std::mem::take(doc);
    let saved = std::mem::replace(
        &mut WebContextHandle { realm }.get_mut(agent).documents[0],
        DocumentOwner::Document(Box::new(real)),
    );
    let saved_realm = agent.current_realm;
    agent.current_realm = realm;
    let threw = crate::dom::events::invoke_listener(agent, handle, event);
    agent.current_realm = saved_realm;
    let back = std::mem::replace(
        &mut WebContextHandle { realm }.get_mut(agent).documents[0],
        saved,
    );
    if let DocumentOwner::Document(d) = back {
        *doc = *d;
    }
    threw
}

impl bf_dom::events::ScriptHost for WebScripts<'_> {
    /// Los listeners de JavaScript (del DOM de E3): el documento vuelve al contexto mientras
    /// corre el listener (el parser lo tiene prestado durante `feed`).
    fn call_listener(
        &mut self,
        doc: &mut Document,
        handle: u64,
        event: &mut bf_dom::events::Event,
    ) -> bool {
        lend_and_invoke(self.agent, self.realm, doc, handle, event)
    }
}

/// El host de un despacho que empieza fuera de un script (un evento del usuario): el
/// documento de la ventana sale del contexto mientras se despacha y vuelve para cada
/// listener.
struct LendHost<'a> {
    agent: &'a mut Agent,
    realm: RealmId,
}

impl bf_dom::events::ScriptHost for LendHost<'_> {
    fn call_listener(
        &mut self,
        doc: &mut Document,
        handle: u64,
        event: &mut bf_dom::events::Event,
    ) -> bool {
        lend_and_invoke(self.agent, self.realm, doc, handle, event)
    }
}

/// Despacha un evento de confianza (`isTrusted`: lo produjo el navegador) en `target` del
/// documento de la ventana de `realm`. Devuelve `false` si se canceló.
pub fn dispatch_trusted(
    agent: &mut Agent,
    realm: RealmId,
    target: bf_dom::events::EventTarget,
    event: &mut bf_dom::events::Event,
) -> bool {
    event.is_trusted = true;
    let mut owner = std::mem::replace(
        &mut WebContextHandle { realm }.get_mut(agent).documents[0],
        DocumentOwner::Document(Box::default()),
    );
    let mut host = LendHost { agent, realm };
    let r = owner.doc_mut().dispatch(target, event, &mut host);
    WebContextHandle { realm }.get_mut(host.agent).documents[0] = owner;
    if host.agent.frames.is_empty()
        && let Err(Throw::Value(e)) = host.agent.perform_microtask_checkpoint()
    {
        host.agent.report_exception(&e);
    }
    r
}

impl bf_html::ScriptHost for WebScripts<'_> {
    fn prepare(&mut self, doc: &mut Parser, script: NodeId) -> ScriptDisposition {
        let Some(info) = doc.document().script_info(script).cloned() else {
            return ScriptDisposition::Failed;
        };
        let Some(url) = info.url.clone() else {
            return ScriptDisposition::Ready;
        };
        match self.fetch(&url, info.encoding) {
            Some(f) => {
                if let Some(env) = env_mut(self.agent, self.realm) {
                    env.fetched.insert(script, f);
                }
                ScriptDisposition::Ready
            }
            None => {
                self.console_error(format!("Failed to load resource: {}", url.href()));
                ScriptDisposition::Failed
            }
        }
    }

    fn execute(&mut self, doc: &mut Parser, script: NodeId) {
        let Some(info) = doc.document().script_info(script).cloned() else {
            return;
        };
        let page = doc.document().url.href();
        let fetched = env_mut(self.agent, self.realm).and_then(|e| e.fetched.remove(&script));
        // El parser vuelve al contexto mientras corre el script.
        let placeholder = Parser::new(&parse_options());
        let parser = std::mem::replace(doc, placeholder);
        put_parser(self.agent, self.realm, parser);
        match info.type_ {
            ScriptType::Classic => match fetched {
                Some(f) => self.run_classic(&f.source, &f.url),
                None => self.run_classic(info.source_text.as_lossy_str(), &page),
            },
            ScriptType::Module => match fetched {
                Some(f) => self.run_module(&f.source, &f.url),
                None => {
                    let n = env_mut(self.agent, self.realm).map_or(0, |e| {
                        e.inline_modules += 1;
                        e.inline_modules
                    });
                    let url = format!("{page}#module-{n}");
                    self.run_module(info.source_text.as_lossy_str(), &url);
                }
            },
            ScriptType::ImportMap => {
                self.register_import_map(info.source_text.as_lossy_str(), &info.base_url);
            }
        }
        *doc = take_parser(self.agent, self.realm);
    }
}

// --- document.write/writeln/open/close (§8.4) ---

/// Los argumentos de `write(...text)` concatenados.
fn concat_args(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<DomString> {
    let mut out = DomString::new();
    for v in a.args {
        out.push_dom_str(&crate::dom::to_dom_string(agent, v)?);
    }
    Ok(out)
}

/// Una operación del parser de la ventana desde una función nativa; las excepciones del DOM
/// se lanzan.
fn parser_op(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    op: impl FnOnce(&mut Parser, &mut WebScripts<'_>) -> Result<(), bf_dom::DomException>,
) -> JsResult<Value> {
    let n = this_document(agent, a)?;
    if n.doc != 0 {
        // Un documento sin navegador (creado por script): no tiene parser.
        return crate::dom::throw_dom(agent, bf_dom::DomException::InvalidStateError);
    }
    let realm = WebContextHandle::current(agent).realm;
    let r = with_parser(agent, realm, op);
    WebContextHandle::current(agent).get_mut(agent).mutated();
    match r {
        Ok(()) => Ok(Value::Undefined),
        Err(e) => crate::dom::throw_dom(agent, e),
    }
}

/// `document.write(...text)`.
pub fn document_write(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let text = concat_args(agent, a)?;
    parser_op(agent, a, |p, host| p.write(host, &text))
}

/// `document.writeln(...text)`.
pub fn document_writeln(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let text = concat_args(agent, a)?;
    parser_op(agent, a, |p, host| p.writeln(host, &text))
}

/// `document.open()`: devuelve el documento.
pub fn document_open(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    parser_op(agent, a, |p, host| p.open(host))?;
    Ok(a.this.clone())
}

/// `document.close()`.
pub fn document_close(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    parser_op(agent, a, |p, host| {
        p.close(host)?;
        p.run_tasks(host);
        Ok(())
    })
}

// --- Eventos de la ventana ---

/// Encola una tarea que dispara un evento simple en la ventana (`hashchange`).
pub fn queue_window_event(agent: &mut Agent, type_: &'static str) {
    let realm = WebContextHandle::current(agent).realm;
    agent.queue_task(
        TaskSource::DomManipulation,
        Box::new(move |agent: &mut Agent| fire_with_props(agent, realm, type_, false, Vec::new())),
    );
}

/// Encola `popstate` con `state`.
pub fn queue_popstate(agent: &mut Agent, state: Value) {
    let realm = WebContextHandle::current(agent).realm;
    let pin = agent.host_pin(state);
    agent.queue_task(
        TaskSource::DomManipulation,
        Box::new(move |agent: &mut Agent| {
            let state = agent.host_unpin(pin).unwrap_or(Value::Null);
            fire_with_props(agent, realm, "popstate", false, vec![("state", state)]);
        }),
    );
}

/// Dispara en la ventana un evento con datos propios (`state` de `popstate`, `key`/`newValue`
/// de `storage`): un `Event` con esas propiedades, despachado con `dispatchEvent` (los
/// `PopStateEvent`, `HashChangeEvent` y `StorageEvent` de la plataforma todavía no existen).
fn fire_with_props(
    agent: &mut Agent,
    realm: RealmId,
    type_: &str,
    bubbles: bool,
    props: Vec<(&str, Value)>,
) {
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let g = agent.realms[realm].global_object;
    let r = (|| -> JsResult<()> {
        let ctor = agent.get_str(g, "Event")?;
        let Value::Object(ctor_obj) = ctor else {
            return Ok(());
        };
        let init = agent.new_plain_object();
        agent.put_data(
            init,
            PropertyKey::from("bubbles"),
            Value::Boolean(bubbles),
            true,
            true,
            true,
        );
        let ev = agent.construct(ctor_obj, &[Value::str(type_), Value::Object(init)], None)?;
        if let Value::Object(e) = &ev {
            for (k, v) in props {
                agent.put_data(*e, PropertyKey::from(k), v, false, true, true);
            }
        }
        let dispatch = agent.get_str(g, "dispatchEvent")?;
        agent.call(&dispatch, Value::Object(g), &[ev])?;
        Ok(())
    })();
    if let Err(Throw::Value(e)) = r {
        agent.report_exception(&e);
    }
    if agent.frames.is_empty() {
        let _ = agent.perform_microtask_checkpoint();
    }
    agent.current_realm = saved;
}

/// Un evento simple de confianza en `target` del documento de la ventana.
fn fire_simple(
    agent: &mut Agent,
    realm: RealmId,
    target: bf_dom::events::EventTarget,
    type_: &str,
    bubbles: bool,
    cancelable: bool,
) -> bool {
    let mut ev = bf_dom::events::Event::new(
        type_,
        bf_dom::events::EventInit {
            bubbles,
            cancelable,
            composed: false,
        },
    );
    dispatch_trusted(agent, realm, target, &mut ev)
}

/// El navegador terminó de cargar las imágenes y hojas del documento: `load` (13.2.7 pasos 8
/// y 9) si nada más lo demora.
pub fn resources_loaded(agent: &mut Agent, realm: RealmId) {
    // `loadEventStart` se marca antes de disparar `load` (Navigation Timing §4.1).
    super::performance::load_finished(agent, realm);
    with_parser(agent, realm, |p, host| p.release_load_event(host));
}

/// "Unload a document" (HTML §7.4.2.4): `beforeunload` (su cancelación no detiene la
/// navegación: no hay diálogo de confirmación), `pagehide` y `unload` en la ventana.
pub fn unload_document(agent: &mut Agent, realm: RealmId) {
    use bf_dom::events::EventTarget;
    fire_simple(
        agent,
        realm,
        EventTarget::Window,
        "beforeunload",
        false,
        true,
    );
    fire_simple(agent, realm, EventTarget::Window, "pagehide", false, false);
    fire_simple(agent, realm, EventTarget::Window, "unload", false, false);
}

/// La pestaña se mostró u ocultó: `visibilityState` y `visibilitychange` (HTML §6.2.1).
pub fn set_visibility(agent: &mut Agent, realm: RealmId, visible: bool) {
    let changed = agent
        .realm_data_mut::<super::BomState>(realm)
        .is_some_and(|s| std::mem::replace(&mut s.visible, visible) != visible);
    if !changed {
        return;
    }
    let root = WebContextHandle { realm }.get(agent).document().root();
    fire_simple(
        agent,
        realm,
        bf_dom::events::EventTarget::Node(root),
        "visibilitychange",
        true,
        false,
    );
}

/// Otra ventana cambió el `localStorage` de `origin`: si es el de esta, el área se vuelve a
/// leer y la ventana recibe `storage` (HTML §12.2.1).
pub fn storage_event(
    agent: &mut Agent,
    realm: RealmId,
    origin: &str,
    key: Option<&str>,
    old: Option<&str>,
    new: Option<&str>,
    url: &str,
) {
    let window = WebContextHandle { realm }.get(agent).url.clone();
    if super::origin_of(&window) != origin {
        return;
    }
    if let Some(s) = agent.realm_data_mut::<super::BomState>(realm) {
        s.local_storage = None;
    }
    let opt = |v: Option<&str>| v.map_or(Value::Null, Value::str);
    let area = agent
        .realm_data::<super::BomState>(realm)
        .and_then(|s| s.objects.local_storage)
        .and_then(|h| agent.host_pinned(h))
        .unwrap_or(Value::Null);
    let props = vec![
        ("key", opt(key)),
        ("oldValue", opt(old)),
        ("newValue", opt(new)),
        ("url", Value::str(url)),
        ("storageArea", area),
    ];
    fire_with_props(agent, realm, "storage", false, props);
}
