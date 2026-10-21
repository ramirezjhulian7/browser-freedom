//! El puente entre el motor JavaScript (`bf-js`) y la plataforma web (`bf-dom`, `bf-html`,
//! `bf-css`): las interfaces WebIDL del DOM, HTML, el BOM y el CSSOM expuestas como clases
//! host de `bf_js::host`.
//!
//! # Organización (una persona por área)
//!
//! | Módulo | Contenido | Dueño |
//! |---|---|---|
//! | [`dom`] | `Window` (global), `EventTarget`/`Event`, `Node`, `Element`, `Document`… | E3 |
//! | [`html`] | `HTML*Element` (reflexión de atributos), runner de testharness.js | E1 |
//! | [`bom`] | `navigator`, `location`, `history`, `screen`, storage, cookie, diálogos | E4 |
//! | [`cssom`] | `CSSStyleDeclaration`, `getComputedStyle`, `styleSheets`, geometría | E2 |
//!
//! Cada área expone `pub fn register(agent: &mut Agent, realm: RealmId, ctx: &WebContextHandle)`
//! y [`register_all`] las llama en orden (`dom` primero: las demás heredan de sus clases).
//!
//! # El contexto
//!
//! Cada ventana tiene un [`WebContext`] guardado en su realm (`Agent::set_realm_data`): es
//! dueño de los documentos (el de la ventana es el índice 0; mientras se parsea, el dueño es
//! el `Parser`), la URL, el contador de mutaciones que pide re-render y los hooks del
//! navegador ([`WebHooks`]). Desde una función nativa se llega con
//! [`WebContextHandle::current`] (el realm de la función es el de la ventana).
//!
//! # El patrón de un wrapper (ver `dom::node`)
//!
//! 1. **Clase**: una vez por agente, `agent.register_host_class(HostClass::new("Node")
//!    .parent(event_target).attribute(...).method(...))`; el id va en
//!    [`dom::DomClasses`] (`agent.embedder_data::<DomClasses>()`). Las clases se instancian
//!    solas en cada realm.
//! 2. **Identidad**: un nodo es un [`NodeRef`] (documento + `NodeId`) y su wrapper se obtiene
//!    con [`dom::wrap_node`], que usa `HostKey(NodeRef::key())`: el mismo nodo da siempre el
//!    mismo objeto mientras JavaScript lo alcance. La clase concreta sale del tipo de nodo y,
//!    para los elementos, del mapa de [`dom::DomClasses::element_classes`] (E1 registra ahí
//!    `HTMLDivElement` etc.).
//! 3. **Funciones nativas**: `fn(&mut Agent, &CallArgs) -> JsResult<Value>`. El trampolín
//!    ya verificó que `this` es de la clase; [`dom::this_node`] devuelve el `NodeRef` y
//!    [`with_doc`]/[`with_doc_mut`] dan el `Document`. Toda mutación del árbol llama a
//!    [`WebContext::mutated`] (re-render). Los errores del DOM se lanzan con
//!    [`dom::throw_dom`] (`DOMException`).
//! 4. **Conversiones**: `bf_js::host::webidl` (`to_dom_string`, `to_long`…) y
//!    [`dom::to_dom_string`]/[`dom::from_dom_string`] entre `JsString` y `DomString`.

#![forbid(unsafe_code)]

pub mod bom;
pub mod cssom;
pub mod dom;
pub mod html;
pub mod net; // E2: fetch, XMLHttpRequest, FormData, Blob

pub use bf_dom::{Document, NodeId};
pub use bf_html::Parser;
pub use bf_infra::{DomString, Url};
pub use bf_js::host::{HostClass, HostClassId, HostKey, Runtime};
pub use bf_js::vm::{Agent, JsResult, RealmId};
pub use cssom::LayoutSnapshot;

/// El índice de un documento en [`WebContext::documents`].
pub type DocIndex = u32;

/// Un nodo de uno de los documentos del contexto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeRef {
    pub doc: DocIndex,
    pub node: NodeId,
}

impl NodeRef {
    pub fn new(doc: DocIndex, node: NodeId) -> NodeRef {
        NodeRef { doc, node }
    }

    /// La clave de identidad del wrapper.
    pub fn key(self) -> HostKey {
        HostKey((u64::from(self.doc) << 32) | self.node.index() as u64)
    }
}

/// Quién tiene el documento: el documento mismo o el parser que lo está construyendo.
pub enum DocumentOwner {
    Document(Box<Document>),
    Parser(Box<Parser>),
}

impl DocumentOwner {
    pub fn doc(&self) -> &Document {
        match self {
            DocumentOwner::Document(d) => d,
            DocumentOwner::Parser(p) => p.document(),
        }
    }

    pub fn doc_mut(&mut self) -> &mut Document {
        match self {
            DocumentOwner::Document(d) => d,
            DocumentOwner::Parser(p) => p.document_mut(),
        }
    }
}

/// Qué recurso hace falta cargar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Script,
    Image,
    Stylesheet,
    Other,
}

/// Lo que el navegador (el shell) implementa para la ventana. Todo tiene un valor por
/// defecto para un entorno sin interfaz.
pub trait WebHooks {
    /// `window.alert` (HTML §8.8.1).
    fn alert(&mut self, _message: &str) {}
    /// `window.confirm`.
    fn confirm(&mut self, _message: &str) -> bool {
        false
    }
    /// `window.prompt`: `None` si se cancela.
    fn prompt(&mut self, _message: &str, _default: &str) -> Option<String> {
        None
    }
    /// `window.open`: `true` si el navegador abrió algo.
    fn open(&mut self, _url: &str, _target: &str, _features: &str) -> bool {
        false
    }
    /// Un elemento insertado necesita un recurso (`<img>`, `<link>`, `<script src>`).
    fn resource_needed(&mut self, _url: &str, _kind: ResourceKind) {}
    /// El DOM cambió y hay que volver a maquetar.
    fn dirty(&mut self) {}
}

/// Hooks sin efectos.
#[derive(Debug, Default)]
pub struct NoWebHooks;

impl WebHooks for NoWebHooks {}

/// El estado de una ventana.
pub struct WebContext {
    /// Los documentos de la ventana; el 0 es `window.document` (los demás los crean
    /// `DOMImplementation`, `DOMParser`…).
    pub documents: Vec<DocumentOwner>,
    pub url: Url,
    pub base: Url,
    /// Contador de mutaciones hechas desde JavaScript.
    pub mutations: u64,
    /// ¿Hay que volver a maquetar?
    pub pending_dirty: bool,
    /// La versión del documento de la ventana en el último [`WebContext::take_dirty`]: un
    /// cambio que no pasó por [`WebContext::mutated`] también pide re-render.
    pub rendered_version: u64,
    /// El último layout (lo llena el shell; lo usa el CSSOM View de E2).
    pub layout: Option<LayoutSnapshot>,
    pub hooks: Box<dyn WebHooks>,
}

impl WebContext {
    /// Un contexto con `document` como documento de la ventana (su URL pasa a ser `url`).
    pub fn new(mut document: DocumentOwner, url: Url) -> WebContext {
        document.doc_mut().url = url.clone();
        let rendered_version = document.doc().version();
        WebContext {
            documents: vec![document],
            base: url.clone(),
            url,
            mutations: 0,
            pending_dirty: false,
            rendered_version,
            layout: None,
            hooks: Box::new(NoWebHooks),
        }
    }

    /// `window.document`.
    pub fn document(&self) -> &Document {
        self.documents[0].doc()
    }

    pub fn document_mut(&mut self) -> &mut Document {
        self.documents[0].doc_mut()
    }

    pub fn doc(&self, i: DocIndex) -> &Document {
        self.documents[i as usize].doc()
    }

    pub fn doc_mut(&mut self, i: DocIndex) -> &mut Document {
        self.documents[i as usize].doc_mut()
    }

    /// Agrega un documento y devuelve su índice.
    pub fn add_document(&mut self, doc: Document) -> DocIndex {
        self.documents.push(DocumentOwner::Document(Box::new(doc)));
        (self.documents.len() - 1) as DocIndex
    }

    /// ¿Cambió el DOM desde la última vez? (Y se limpia la marca.) Cubre las mutaciones
    /// registradas con [`WebContext::mutated`] y cualquier cambio del árbol o de atributos.
    pub fn take_dirty(&mut self) -> bool {
        let version = self.document().version();
        let dirty = std::mem::take(&mut self.pending_dirty) || version != self.rendered_version;
        self.rendered_version = version;
        dirty
    }

    /// Registra una mutación del DOM hecha desde JavaScript.
    pub fn mutated(&mut self) {
        self.mutations += 1;
        if !self.pending_dirty {
            self.pending_dirty = true;
            self.hooks.dirty();
        }
    }
}

/// Cómo llega una función nativa a su [`WebContext`]: el realm de la ventana.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebContextHandle {
    pub realm: RealmId,
}

impl WebContextHandle {
    /// El contexto del realm en curso (el de la función nativa que se está ejecutando).
    pub fn current(agent: &Agent) -> WebContextHandle {
        WebContextHandle {
            realm: agent.current_realm,
        }
    }

    pub fn get<'a>(&self, agent: &'a Agent) -> &'a WebContext {
        agent
            .realm_data::<WebContext>(self.realm)
            .expect("el realm no tiene WebContext")
    }

    pub fn get_mut<'a>(&self, agent: &'a mut Agent) -> &'a mut WebContext {
        agent
            .realm_data_mut::<WebContext>(self.realm)
            .expect("el realm no tiene WebContext")
    }
}

/// El `Document` de `doc` en el contexto actual.
pub fn with_doc<R>(agent: &Agent, doc: DocIndex, f: impl FnOnce(&Document) -> R) -> R {
    let ctx = WebContextHandle::current(agent).get(agent);
    f(ctx.doc(doc))
}

/// El `Document` mutable de `doc` en el contexto actual.
pub fn with_doc_mut<R>(agent: &mut Agent, doc: DocIndex, f: impl FnOnce(&mut Document) -> R) -> R {
    let h = WebContextHandle::current(agent);
    f(h.get_mut(agent).doc_mut(doc))
}

/// Instala `ctx` como la ventana de `realm`: registra todas las clases y convierte el objeto
/// global en el `Window`.
pub fn install(agent: &mut Agent, realm: RealmId, ctx: WebContext) -> WebContextHandle {
    agent.set_realm_data(realm, ctx);
    let h = WebContextHandle { realm };
    register_all(agent, realm, &h);
    h
}

/// Registra las clases de todas las áreas en `realm` (cada área agrega su línea).
pub fn register_all(agent: &mut Agent, realm: RealmId, ctx: &WebContextHandle) {
    dom::register(agent, realm, ctx); // E3
    html::register(agent, realm, ctx); // E1
    bom::register(agent, realm, ctx); // E4
    cssom::register(agent, realm, ctx); // E2
    net::register(agent, realm, ctx); // E2
}
