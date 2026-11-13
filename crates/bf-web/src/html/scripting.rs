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

use bf_dom::events::{Event, EventInit, EventKind, EventTarget};
use bf_dom::script::ScriptType;
use bf_dom::{Document, NodeId};
use bf_html::{ParseOptions, Parser, ScriptDisposition, ScriptHost};
use bf_infra::Url;
use bf_infra::resource::ResourceLoader;
use bf_js::vm::{Agent, RealmId, Throw};

use crate::{DocIndex, DocumentOwner, WebContext, WebContextHandle};

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

/// Presta `doc` al contexto (como el documento `index`) mientras corre el listener `handle`.
fn lend_and_invoke(
    agent: &mut Agent,
    realm: RealmId,
    index: DocIndex,
    doc: &mut Document,
    handle: u64,
    event: &mut Event,
) -> bool {
    let real = std::mem::replace(doc, Document::new());
    let saved = std::mem::replace(
        &mut ctx(agent, realm).documents[index as usize],
        DocumentOwner::Document(Box::new(real)),
    );
    // El listener lo llama el DOM (E3), dueño de `addEventListener`.
    let threw = crate::dom::events::invoke_listener(agent, handle, event);
    let back = std::mem::replace(&mut ctx(agent, realm).documents[index as usize], saved);
    match back {
        DocumentOwner::Document(d) => *doc = *d,
        // E1: `document.write`/`open`/`close` en el listener envolvió el documento prestado en
        // un parser (`take_parser`); el documento sigue siendo ese. Antes se perdía y lo que
        // seguía (el dispatch, el parser) corría sobre el documento vacío: pánico de índice
        // con el siguiente `NodeId` (`bf-dom` `Document::get`).
        DocumentOwner::Parser(p) => *doc = p.into_document().0,
    }
    threw
}

impl bf_dom::events::ScriptHost for PageHost<'_> {
    fn call_listener(&mut self, doc: &mut Document, handle: u64, event: &mut Event) -> bool {
        lend_and_invoke(self.agent, self.realm, 0, doc, handle, event)
    }

    // E3: los handlers de atributo de contenido (`onclick="…"`) se registran al despachar.
    fn before_invoke(
        &mut self,
        doc: &mut Document,
        target: EventTarget,
        type_: &bf_dom::DomString,
    ) {
        crate::dom::events::sync_lent(self.agent, self.realm, 0, doc, target, type_);
    }
}

/// El host de un despacho que empieza en una función nativa: el documento `doc` del
/// contexto sale mientras se despacha y vuelve para cada listener.
struct LendHost<'a> {
    agent: &'a mut Agent,
    realm: RealmId,
    doc: DocIndex,
}

impl bf_dom::events::ScriptHost for LendHost<'_> {
    fn call_listener(&mut self, doc: &mut Document, handle: u64, event: &mut Event) -> bool {
        lend_and_invoke(self.agent, self.realm, self.doc, doc, handle, event)
    }

    // E3: los handlers de atributo de contenido (`onclick="…"`) se registran al despachar.
    fn before_invoke(
        &mut self,
        doc: &mut Document,
        target: EventTarget,
        type_: &bf_dom::DomString,
    ) {
        crate::dom::events::sync_lent(self.agent, self.realm, self.doc, doc, target, type_);
    }
}

/// "Fire an event" (DOM §2.10) desde una función nativa: `type_` con `init` y `kind` en
/// `target` del documento `doc`. Los eventos de esta API son sintéticos (`isTrusted` es
/// `false`, como los de `click()`). Devuelve `false` si se canceló.
pub fn fire_event(
    agent: &mut Agent,
    doc: DocIndex,
    target: EventTarget,
    type_: &str,
    init: EventInit,
    kind: EventKind,
) -> bool {
    let realm = WebContextHandle::current(agent).realm;
    let mut owner = std::mem::replace(
        &mut ctx(agent, realm).documents[doc as usize],
        placeholder(),
    );
    let mut event = Event::with_kind(type_, init, kind);
    let mut host = LendHost { agent, realm, doc };
    let r = owner.doc_mut().dispatch(target, &mut event, &mut host);
    ctx(host.agent, realm).documents[doc as usize] = owner;
    r
}

impl ScriptHost for PageHost<'_> {
    // E2: como en la ventana (`bom::script`): los `<link>` del parser cargan su hoja o su
    // `preload`, y los `<img>` piden su imagen; los dos demoran `load`.
    fn link_inserted(&mut self, doc: &mut Parser, link: NodeId) {
        let realm = self.realm;
        super::legacy_factories::with_lent_document(
            self.agent,
            realm,
            doc.document_mut(),
            |agent| crate::cssom::sheets::link_connected(agent, crate::NodeRef::new(0, link)),
        );
    }

    fn img_inserted(&mut self, doc: &mut Parser, img: NodeId) {
        let realm = self.realm;
        super::legacy_factories::with_lent_document(
            self.agent,
            realm,
            doc.document_mut(),
            |agent| super::legacy_factories::img_created(agent, crate::NodeRef::new(0, img)),
        );
    }

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
        // E1: los custom elements que el parser insertó se actualizan antes del script.
        crate::html::custom_elements::process_reactions(self.agent);
        self.run_classic_script(&text, &url);
        let back = std::mem::replace(&mut ctx(self.agent, self.realm).documents[0], saved);
        if let DocumentOwner::Parser(p) = back {
            *doc = *p;
        }
    }

    /// E3: un `script` SVG que cierra el parser (HTML §13.2.6.5, SVG 2 §15.2): su texto o el
    /// recurso de su `href`, como script clásico, con el parser en el contexto.
    fn process_svg_script(&mut self, doc: &mut Parser, script: NodeId) {
        let (url, text) = match bf_html::svg_script_source(doc.document(), script) {
            Some(bf_html::SvgScriptSource::Inline(text)) => {
                (doc.document().url.serialize(false), text)
            }
            Some(bf_html::SvgScriptSource::External(href)) => {
                let base = doc.document().base_url();
                let Ok(url) = Url::parse(&href, Some(&base)) else {
                    return;
                };
                match self.fetch_script(&url, "utf-8") {
                    Some(text) => (url.serialize(false), text),
                    None => return,
                }
            }
            None => return,
        };
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

    /// E3: el punto de control de microtasks antes de preparar cada `script` del parser
    /// (HTML §13.2.6.4.8, paso 1), si la pila de JavaScript está vacía. El parser vuelve al
    /// contexto mientras corren (un `MutationObserver` puede tocar el documento), como en
    /// [`Self::execute`].
    fn microtask_checkpoint(&mut self, doc: &mut Parser) {
        if !self.agent.frames.is_empty() {
            return;
        }
        let real = std::mem::replace(doc, Parser::new(&ParseOptions::default()));
        let saved = std::mem::replace(
            &mut ctx(self.agent, self.realm).documents[0],
            DocumentOwner::Parser(Box::new(real)),
        );
        let saved_realm = self.agent.current_realm;
        self.agent.current_realm = self.realm;
        // Los records de lo que insertó el parser: bf-dom los encola, pero la entrega al
        // `MutationObserver` solo se agenda sola tras una mutación desde script.
        crate::dom::observer::schedule_delivery(self.agent, 0);
        let r = match self.agent.perform_microtask_checkpoint() {
            Err(Throw::Value(e)) => {
                self.agent.report_exception(&e);
                Ok(())
            }
            r => r,
        };
        if let Err(Throw::Terminate(_)) = r
            && let Some(res) = self.resources()
        {
            res.terminated = true;
        }
        self.agent.current_realm = saved_realm;
        let back = std::mem::replace(&mut ctx(self.agent, self.realm).documents[0], saved);
        if let DocumentOwner::Parser(p) = back {
            *doc = *p;
        }
    }
}
