//! El motor de script visto desde el parser (`bf_html::ScriptHost`): carga los scripts
//! clásicos externos con el [`ResourceLoader`] de la página y ejecuta los scripts (HTML
//! §4.12.1.1 "prepare the script element", pasos 31–32, y §4.12.1.3 "execute the script
//! element", paso 6, con "run a classic script", §8.1.4.4).
//!
//! # Quién tiene el documento
//!
//! Mientras se parsea, el `Parser` (y con él el `Document`) vive en el [`WebContext`]
//! ([`DocumentOwner::Parser`]): así lo ven las funciones nativas del DOM. Para alimentarlo,
//! [`with_parser`] lo saca del contexto y lo pasa junto con un [`PageHost`]. Cuando el parser
//! le pide al host que ejecute un script, el host lo **devuelve al contexto** durante la
//! ejecución (intercambiándolo con un parser vacío: `feed` no usa su estado mientras presta
//! `&mut self`) y lo recupera al terminar; lo mismo con el `Document` en los listeners.

use std::collections::HashMap;
use std::sync::Arc;

use bf_dom::events::Event;
use bf_dom::script::ScriptType;
use bf_dom::{Document, NodeId};
use bf_html::{ParseOptions, Parser, ScriptDisposition, ScriptHost};
use bf_infra::Url;
use bf_infra::resource::ResourceLoader;
use bf_js::vm::{Agent, RealmId, Throw};

use crate::{DocumentOwner, WebContext};

/// Los recursos de la página: el loader y los scripts externos ya descargados.
pub struct PageResources {
    pub loader: Arc<dyn ResourceLoader>,
    /// Respuestas fijas por URL (p. ej. el `testharnessreport.js` del runner).
    pub overrides: HashMap<String, String>,
    /// El texto de los scripts externos listos, por elemento.
    scripts: HashMap<NodeId, (String, String)>,
    /// La ejecución terminó por el presupuesto o el tiempo del agente.
    pub terminated: bool,
}

impl PageResources {
    pub fn new(loader: Arc<dyn ResourceLoader>) -> PageResources {
        PageResources {
            loader,
            overrides: HashMap::new(),
            scripts: HashMap::new(),
            terminated: false,
        }
    }
}

/// Cómo se llama a un listener de script desde un evento que dispara el parser (DOMContentLoaded,
/// `load`, `error` de los scripts): lo registra el área del DOM, dueña de `addEventListener`,
/// con `agent.set_embedder_data(ListenerInvoker(f))`. `f` devuelve `true` si el listener lanzó.
#[derive(Clone, Copy)]
pub struct ListenerInvoker(pub fn(&mut Agent, u64, &mut Event) -> bool);

/// Un `ScriptHost` sobre el agente: lo crea [`with_parser`].
pub struct PageHost<'a> {
    pub agent: &'a mut Agent,
    pub realm: RealmId,
}

fn ctx(agent: &mut Agent, realm: RealmId) -> &mut WebContext {
    agent
        .realm_data_mut::<WebContext>(realm)
        .expect("el realm no tiene WebContext")
}

fn placeholder() -> DocumentOwner {
    DocumentOwner::Document(Box::new(Document::new()))
}

/// Corre `f` con el parser de la ventana de `realm` (fuera del contexto mientras dura). `None`
/// si el documento ya no tiene parser.
pub fn with_parser<R>(
    agent: &mut Agent,
    realm: RealmId,
    f: impl FnOnce(&mut Parser, &mut PageHost<'_>) -> R,
) -> Option<R> {
    let owner = std::mem::replace(&mut ctx(agent, realm).documents[0], placeholder());
    let mut parser = match owner {
        DocumentOwner::Parser(p) => p,
        other => {
            ctx(agent, realm).documents[0] = other;
            return None;
        }
    };
    let mut host = PageHost { agent, realm };
    let r = f(&mut parser, &mut host);
    ctx(host.agent, realm).documents[0] = DocumentOwner::Parser(parser);
    Some(r)
}

impl PageHost<'_> {
    fn resources(&mut self) -> Option<&mut PageResources> {
        self.agent.realm_data_mut::<PageResources>(self.realm)
    }

    /// Descarga un script clásico externo: el texto decodificado, o `None` si falló.
    fn fetch_script(&mut self, url: &Url, encoding: &'static str) -> Option<String> {
        let href = url.serialize(true);
        let res = self.resources()?;
        if let Some(text) = res.overrides.get(&href) {
            return Some(text.clone());
        }
        let resource = res.loader.load(url)?;
        let enc = bf_infra::encoding::Encoding::for_label(encoding)
            .unwrap_or(bf_infra::encoding::Encoding::Utf8);
        Some(bf_infra::encoding::decode(&resource.bytes, enc).0)
    }

    /// "Run a classic script" (§8.1.4.4): las excepciones se reportan ("report an
    /// exception", §8.1.4.6) y al terminar se hace el punto de control de microtasks.
    pub fn run_classic_script(&mut self, text: &str, url: &str) {
        let saved = self.agent.current_realm;
        self.agent.current_realm = self.realm;
        let r = self
            .agent
            .compile_script_text(text, url)
            .and_then(|code| self.agent.script_evaluation(code));
        let r = match r {
            Ok(_) => self.agent.perform_microtask_checkpoint(),
            Err(Throw::Value(e)) => {
                self.agent.report_exception(&e);
                self.agent.perform_microtask_checkpoint()
            }
            Err(t) => Err(t),
        };
        if let Err(Throw::Terminate(_)) = r
            && let Some(res) = self.resources()
        {
            res.terminated = true;
        }
        self.agent.current_realm = saved;
    }
}

impl bf_dom::events::ScriptHost for PageHost<'_> {
    fn call_listener(&mut self, doc: &mut Document, handle: u64, event: &mut Event) -> bool {
        // El documento vuelve al contexto mientras corre el listener.
        let real = std::mem::replace(doc, Document::new());
        let saved = std::mem::replace(
            &mut ctx(self.agent, self.realm).documents[0],
            DocumentOwner::Document(Box::new(real)),
        );
        let threw = match self.agent.embedder_data::<ListenerInvoker>() {
            Some(ListenerInvoker(f)) => {
                let f = *f;
                f(self.agent, handle, event)
            }
            None => false,
        };
        let back = std::mem::replace(&mut ctx(self.agent, self.realm).documents[0], saved);
        if let DocumentOwner::Document(d) = back {
            *doc = *d;
        }
        threw
    }
}

impl ScriptHost for PageHost<'_> {
    fn prepare(&mut self, doc: &mut Parser, script: NodeId) -> ScriptDisposition {
        let Some(info) = doc.document().script_info(script).cloned() else {
            return ScriptDisposition::Failed;
        };
        if info.type_ != ScriptType::Classic {
            // Los scripts módulo y los import maps todavía no se cargan desde la página.
            return ScriptDisposition::Failed;
        }
        if info.src.is_none() {
            return ScriptDisposition::Ready;
        }
        let Some(url) = info.url else {
            return ScriptDisposition::Failed;
        };
        match self.fetch_script(&url, info.encoding) {
            Some(text) => {
                let href = url.serialize(false);
                if let Some(res) = self.resources() {
                    res.scripts.insert(script, (href, text));
                }
                ScriptDisposition::Ready
            }
            None => ScriptDisposition::Failed,
        }
    }

    fn execute(&mut self, doc: &mut Parser, script: NodeId) {
        let Some(info) = doc.document().script_info(script).cloned() else {
            return;
        };
        let (url, text) = if info.src.is_some() {
            match self.resources().and_then(|r| r.scripts.remove(&script)) {
                Some(s) => s,
                None => return,
            }
        } else {
            (
                doc.document().url.serialize(false),
                info.source_text.to_string(),
            )
        };
        // El parser vuelve al contexto mientras corre el script (ver el módulo).
        let real = std::mem::replace(doc, Parser::new(&ParseOptions::default()));
        let saved = std::mem::replace(
            &mut ctx(self.agent, self.realm).documents[0],
            DocumentOwner::Parser(Box::new(real)),
        );
        self.run_classic_script(&text, &url);
        let back = std::mem::replace(&mut ctx(self.agent, self.realm).documents[0], saved);
        if let DocumentOwner::Parser(p) = back {
            *doc = *p;
        }
    }
}
