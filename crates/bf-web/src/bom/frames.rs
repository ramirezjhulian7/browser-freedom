//! `<iframe>` (HTML §4.8.5): contextos de navegación anidados en el mismo agente (el mismo
//! hilo que el documento de la pestaña). Cada marco tiene su realm, su [`crate::WebContext`]
//! con su documento, su BOM y sus scripts.
//!
//! - [`sync_frames`] (lo llama el navegador tras parsear y cuando cambia el DOM) crea los
//!   marcos de los `<iframe>` conectados, los navega según `srcdoc`/`src` y descarta los de
//!   los elementos que ya no están; `contentWindow` crea el marco en el acto si hace falta
//!   (con su `about:blank` inicial).
//! - `contentWindow`, `contentDocument` (solo del mismo origen), `window.parent`, `top`,
//!   `length`, `frameElement` y `window[i]`.
//! - `sandbox` básico: sin `allow-scripts` no corren scripts; sin `allow-same-origin`, el
//!   origen es opaco (y `contentDocument` es `null`).
//!
//! Las navegaciones de un marco (`location.href` dentro del iframe) cargan el marco, no la
//! pestaña.
//!
//! E4: entre orígenes distintos, `contentWindow`, `parent`, `top`, `window[i]` y el `source`
//! de los mensajes dan un `WindowProxy` restringido (HTML §7.2.3.1 y §7.2.1.3): solo las
//! propiedades de "CrossOriginProperties" (`postMessage`, `location`, `parent`…); el resto
//! lanza `SecurityError`. `postMessage` (HTML §9.3.3) clona el mensaje, comprueba
//! `targetOrigin` y dispara un `MessageEvent` con `origin` y `source`. [`frame_views`] da al
//! navegador los documentos de los marcos para pintarlos dentro de su `<iframe>`.

use std::sync::Arc;

use bf_dom::events::{Event, EventInit, EventTarget};
use bf_dom::{Document, NodeId};
use bf_infra::encoding::{self, Encoding};
use bf_infra::{DomString, Namespace, ResourceLoader, Url};
use bf_js::host::{HostHandle, TaskSource};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, RealmId};

use super::{BomHooks, ScreenInfo};
use crate::{DocumentOwner, NodeRef, WebContext, WebContextHandle, WebHooks};

/// Cuántos niveles de marcos anidados se cargan (un iframe que se carga a sí mismo).
const MAX_DEPTH: usize = 8;

/// Un marco.
#[derive(Debug, Clone)]
pub struct Frame {
    pub realm: RealmId,
    /// El realm de la ventana que lo contiene.
    pub parent: RealmId,
    /// El `<iframe>` en el documento del padre.
    pub element: NodeId,
    /// Qué se cargó (para notar cambios de `src`/`srcdoc`/`sandbox`).
    signature: String,
    /// Origen opaco (`sandbox` sin `allow-same-origin`).
    pub opaque: bool,
    /// Navegación pendiente pedida desde el propio marco (`location.href = …`).
    pending: Option<(Url, bool)>,
}

/// Los marcos del agente (dato del embebedor).
#[derive(Debug, Default)]
pub struct Frames {
    pub list: Vec<Frame>,
}

fn frames(agent: &Agent) -> Option<&Frames> {
    agent.embedder_data::<Frames>()
}

fn frames_mut(agent: &mut Agent) -> &mut Frames {
    if agent.embedder_data::<Frames>().is_none() {
        agent.set_embedder_data(Frames::default());
    }
    agent.embedder_data_mut::<Frames>().expect("Frames")
}

/// El marco de `realm`, si es un marco.
pub fn frame_of(agent: &Agent, realm: RealmId) -> Option<&Frame> {
    frames(agent)?.list.iter().find(|f| f.realm == realm)
}

/// La ventana que contiene a `realm` (`None` para la de la pestaña).
pub fn parent_of(agent: &Agent, realm: RealmId) -> Option<RealmId> {
    frame_of(agent, realm).map(|f| f.parent)
}

/// La ventana de más arriba.
pub fn top_of(agent: &Agent, mut realm: RealmId) -> RealmId {
    let mut depth = 0;
    while let Some(p) = parent_of(agent, realm) {
        realm = p;
        depth += 1;
        if depth > 64 {
            break;
        }
    }
    realm
}

/// Los marcos hijos de `realm`, en el orden de sus `<iframe>` en el documento.
pub fn children_of(agent: &Agent, realm: RealmId) -> Vec<RealmId> {
    let Some(ctx) = agent.realm_data::<WebContext>(realm) else {
        return Vec::new();
    };
    let doc = ctx.document();
    iframes(doc)
        .into_iter()
        .filter_map(|el| {
            frames(agent)?
                .list
                .iter()
                .find(|f| f.parent == realm && f.element == el)
                .map(|f| f.realm)
        })
        .collect()
}

/// Los `<iframe>` conectados de `doc`, en orden de documento.
fn iframes(doc: &Document) -> Vec<NodeId> {
    doc.inclusive_descendants(doc.root())
        .into_iter()
        .filter(|&n| doc.element(n).is_some_and(|e| e.is_html("iframe")))
        .collect()
}

fn depth_of(agent: &Agent, realm: RealmId) -> usize {
    let mut d = 0;
    let mut r = realm;
    while let Some(p) = parent_of(agent, r) {
        d += 1;
        r = p;
        if d > MAX_DEPTH {
            break;
        }
    }
    d
}

/// Los tokens de `sandbox`: `None` sin el atributo.
fn sandbox_tokens(doc: &Document, el: NodeId) -> Option<Vec<String>> {
    let v = doc.element(el)?.attr("sandbox")?;
    Some(
        v.split_ascii_whitespace()
            .map(str::to_ascii_lowercase)
            .collect(),
    )
}

/// `srcdoc`/`src`/`sandbox` del elemento, para notar cambios.
fn signature(doc: &Document, el: NodeId) -> String {
    let e = doc.element(el);
    let a = |n: &str| e.and_then(|e| e.attr(n)).unwrap_or("\u{1}").to_owned();
    format!("{}\u{0}{}\u{0}{}", a("srcdoc"), a("src"), a("sandbox"))
}

/// Qué cargar en un marco.
enum Target {
    Blank,
    Srcdoc(String),
    Url(Url),
}

/// "Process the iframe attributes" (HTML §4.8.5): `srcdoc`, si no `src`, si no
/// `about:blank`.
fn target_of(doc: &Document, el: NodeId, base: &Url) -> Target {
    let Some(e) = doc.element(el) else {
        return Target::Blank;
    };
    if let Some(srcdoc) = e.attr("srcdoc") {
        return Target::Srcdoc(srcdoc.to_owned());
    }
    match e.attr("src").map(str::trim).filter(|s| !s.is_empty()) {
        Some(src) => match Url::parse(src, Some(base)) {
            Ok(u) if u.href() != "about:blank" => Target::Url(u),
            _ => Target::Blank,
        },
        None => Target::Blank,
    }
}

/// Los hooks del BOM de un marco: lo del navegador (cookies, almacenamiento, pantalla,
/// diálogos) se delega en los de la pestaña; la navegación es del marco.
struct FrameHooks {
    inner: Box<dyn BomHooks>,
    realm: RealmId,
    /// Navegaciones pedidas por el marco (las procesa [`sync_frames`]).
    requests: std::rc::Rc<std::cell::RefCell<Vec<(RealmId, Url, bool)>>>,
}

impl BomHooks for FrameHooks {
    fn navigate(&mut self, url: &Url, replace: bool) {
        self.requests
            .borrow_mut()
            .push((self.realm, url.clone(), replace));
    }
    fn cookies(&mut self, url: &Url) -> String {
        self.inner.cookies(url)
    }
    fn set_cookie(&mut self, url: &Url, value: &str) {
        self.inner.set_cookie(url, value);
    }
    fn cookies_enabled(&self) -> bool {
        self.inner.cookies_enabled()
    }
    fn load_storage(&mut self, origin: &str) -> Vec<(String, String)> {
        self.inner.load_storage(origin)
    }
    fn save_storage(&mut self, origin: &str, items: &[(String, String)]) {
        self.inner.save_storage(origin, items);
    }
    fn storage_changed(
        &mut self,
        origin: &str,
        key: Option<&str>,
        old: Option<&str>,
        new: Option<&str>,
        url: &str,
    ) {
        self.inner.storage_changed(origin, key, old, new, url);
    }
    fn screen(&self) -> ScreenInfo {
        self.inner.screen()
    }
    fn languages(&self) -> Vec<String> {
        self.inner.languages()
    }
    fn hardware_concurrency(&self) -> u32 {
        self.inner.hardware_concurrency()
    }
    fn child_hooks(&self) -> Option<(Box<dyn BomHooks>, Box<dyn WebHooks>)> {
        self.inner.child_hooks()
    }
}

/// Las navegaciones pedidas desde los marcos (compartidas por todos los [`FrameHooks`]).
#[derive(Debug, Default)]
struct FrameRequests(std::rc::Rc<std::cell::RefCell<Vec<(RealmId, Url, bool)>>>);

fn frame_requests(agent: &mut Agent) -> std::rc::Rc<std::cell::RefCell<Vec<(RealmId, Url, bool)>>> {
    if agent.embedder_data::<FrameRequests>().is_none() {
        agent.set_embedder_data(FrameRequests::default());
    }
    agent
        .embedder_data::<FrameRequests>()
        .expect("FrameRequests")
        .0
        .clone()
}

/// Crea el marco del `<iframe>` `element` de `parent`, con su `about:blank` inicial.
fn create_frame(agent: &mut Agent, parent: RealmId, element: NodeId) -> RealmId {
    let realm = agent.create_realm();
    bf_js::host::event_loop::install_globals(agent, realm);
    let parent_url = WebContextHandle { realm: parent }.get(agent).url.clone();
    let blank = Url::parse("about:blank", None).expect("about:blank");
    let (mut doc, _) = bf_html::parse_document("", &bf_html::ParseOptions::default());
    doc.url = blank.clone();
    let mut ctx = WebContext::new(DocumentOwner::Document(Box::new(doc)), blank.clone());
    // `about:blank` hereda la URL base del que lo crea (HTML §2.4.1).
    ctx.base = parent_url;
    let hooks = agent
        .realm_data::<super::BomState>(parent)
        .and_then(|s| s.hooks.child_hooks());
    let (bom, web) = match hooks {
        Some((b, w)) => (Some(b), Some(w)),
        None => (None, None),
    };
    if let Some(w) = web {
        ctx.hooks = w;
    }
    crate::install(agent, realm, ctx);
    let requests = frame_requests(agent);
    let inner: Box<dyn BomHooks> = bom.unwrap_or_else(|| Box::new(super::NoBomHooks));
    super::set_hooks(
        agent,
        realm,
        Box::new(FrameHooks {
            inner,
            realm,
            requests,
        }),
    );
    // La red de `fetch`/XHR es la de la pestaña.
    if let Some(net) = super::script::net_of(agent, parent) {
        super::script::install_net(agent, realm, net);
    }
    if let Some(loader) =
        super::script::script_env(agent, parent).map(super::script::ScriptEnv::loader)
    {
        super::script::install_frame_scripting(agent, realm, &blank, loader);
    }
    frames_mut(agent).list.push(Frame {
        realm,
        parent,
        element,
        signature: String::new(),
        opaque: false,
        pending: None,
    });
    realm
}

/// Lee el recurso de `url` con el loader de la ventana `realm` (UTF-8 si no hay BOM).
fn fetch(agent: &Agent, realm: RealmId, url: &Url) -> Option<(String, Url)> {
    let loader: Arc<dyn ResourceLoader> = super::script::script_env(agent, realm)?.loader();
    let res = loader.load(url)?;
    let (text, _) = encoding::decode(&res.bytes, Encoding::Utf8);
    Some((text, res.url))
}

/// Navega el marco `realm` a `target` y, al terminar, dispara `load` en su `<iframe>`.
fn navigate(agent: &mut Agent, realm: RealmId, target: Target) {
    let Some(frame) = frame_of(agent, realm).cloned() else {
        return;
    };
    let (parent, element) = (frame.parent, frame.element);
    let parent_doc_sandbox = {
        let ctx = WebContextHandle { realm: parent }.get(agent);
        sandbox_tokens(ctx.document(), element)
    };
    let scripts = parent_doc_sandbox
        .as_ref()
        .is_none_or(|t| t.iter().any(|t| t == "allow-scripts"));
    let opaque = parent_doc_sandbox
        .as_ref()
        .is_some_and(|t| !t.iter().any(|t| t == "allow-same-origin"));
    if let Some(f) = frames_mut(agent).list.iter_mut().find(|f| f.realm == realm) {
        f.opaque = opaque;
    }
    let deep = depth_of(agent, realm) > MAX_DEPTH;
    let parent_base = WebContextHandle { realm: parent }.get(agent).base.clone();
    match target {
        Target::Blank => {}
        Target::Srcdoc(text) => {
            let url = Url::parse("about:srcdoc", None).expect("about:srcdoc");
            // `about:srcdoc` toma la URL base del padre (HTML §2.4.1).
            load_document(
                agent,
                realm,
                &url,
                Some(parent_base),
                &text,
                scripts && !deep,
            );
        }
        Target::Url(url) if deep => {
            let _ = url;
        }
        Target::Url(url) => match fetch(agent, parent, &url) {
            Some((text, final_url)) => {
                load_document(agent, realm, &final_url, None, &text, scripts);
            }
            None => {
                agent.host.hooks.console(
                    bf_js::host::ConsoleLevel::Error,
                    &format!("No se pudo cargar el marco {}", url.href()),
                );
            }
        },
    }
    // El `load` del `<iframe>` (HTML §4.8.5 "iframe load event steps").
    let mut ev = Event::new("load", EventInit::default());
    super::script::dispatch_trusted(agent, parent, EventTarget::Node(element), &mut ev);
}

/// Parsea `text` como el documento del marco `realm` en `url`.
fn load_document(
    agent: &mut Agent,
    realm: RealmId,
    url: &Url,
    base: Option<Url>,
    text: &str,
    scripts: bool,
) {
    super::script::begin_parse_with(agent, realm, url, scripts);
    if let Some(b) = base {
        super::script::set_base(agent, realm, &b);
    }
    super::script::feed(agent, realm, text);
    super::script::finish(agent, realm);
    super::script::resources_loaded(agent, realm);
}

/// Pone al día los marcos de la ventana `realm` y de sus marcos: crea los de los `<iframe>`
/// nuevos, vuelve a navegar los que cambiaron `src`/`srcdoc`, atiende las navegaciones
/// pedidas desde los marcos y olvida los de los elementos que ya no están.
pub fn sync_frames(agent: &mut Agent, realm: RealmId) {
    // Navegaciones pedidas desde dentro de los marcos.
    let requested: Vec<(RealmId, Url, bool)> =
        frame_requests(agent).borrow_mut().drain(..).collect();
    for (r, url, replace) in requested {
        if let Some(f) = frames_mut(agent).list.iter_mut().find(|f| f.realm == r) {
            f.pending = Some((url, replace));
        }
    }
    sync_level(agent, realm, 0);
}

fn sync_level(agent: &mut Agent, realm: RealmId, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    let Some(ctx) = agent.realm_data::<WebContext>(realm) else {
        return;
    };
    let doc = ctx.document();
    let base = ctx.base.clone();
    let elements: Vec<(NodeId, String, Target)> = iframes(doc)
        .into_iter()
        .map(|el| (el, signature(doc, el), target_of(doc, el, &base)))
        .collect();
    // Los marcos cuyo `<iframe>` ya no está.
    let alive: Vec<NodeId> = elements.iter().map(|(e, ..)| *e).collect();
    let gone: Vec<RealmId> = frames(agent)
        .map(|f| {
            f.list
                .iter()
                .filter(|f| f.parent == realm && !alive.contains(&f.element))
                .map(|f| f.realm)
                .collect()
        })
        .unwrap_or_default();
    for g in gone {
        remove_frame(agent, g);
    }
    for (el, sig, target) in elements {
        let existing = frames(agent).and_then(|f| {
            f.list
                .iter()
                .find(|f| f.parent == realm && f.element == el)
                .cloned()
        });
        let child = match existing {
            Some(f) => {
                if let Some((url, _)) = f.pending.clone() {
                    set_pending(agent, f.realm, None);
                    navigate(agent, f.realm, Target::Url(url));
                } else if f.signature != sig {
                    set_signature(agent, f.realm, &sig);
                    navigate(agent, f.realm, target);
                }
                f.realm
            }
            None => {
                let child = create_frame(agent, realm, el);
                set_signature(agent, child, &sig);
                navigate(agent, child, target);
                child
            }
        };
        sync_level(agent, child, depth + 1);
    }
    index_children(agent, realm);
}

fn set_signature(agent: &mut Agent, realm: RealmId, sig: &str) {
    if let Some(f) = frames_mut(agent).list.iter_mut().find(|f| f.realm == realm) {
        f.signature = sig.to_owned();
    }
}

fn set_pending(agent: &mut Agent, realm: RealmId, p: Option<(Url, bool)>) {
    if let Some(f) = frames_mut(agent).list.iter_mut().find(|f| f.realm == realm) {
        f.pending = p;
    }
}

/// Olvida un marco y los suyos (su `unload` primero).
fn remove_frame(agent: &mut Agent, realm: RealmId) {
    let kids: Vec<RealmId> = frames(agent)
        .map(|f| {
            f.list
                .iter()
                .filter(|f| f.parent == realm)
                .map(|f| f.realm)
                .collect()
        })
        .unwrap_or_default();
    for k in kids {
        remove_frame(agent, k);
    }
    super::script::unload_document(agent, realm);
    frames_mut(agent).list.retain(|f| f.realm != realm);
}

/// `window[i]` (HTML §7.2.3.3, las propiedades indexadas del `WindowProxy`): propiedades
/// propias del global con los marcos hijos.
fn index_children(agent: &mut Agent, realm: RealmId) {
    let kids = children_of(agent, realm);
    let g = agent.realms[realm].global_object;
    let mut i = 0;
    loop {
        let key = PropertyKey::from(i.to_string().as_str());
        match kids.get(i) {
            Some(&k) => {
                let w = window_proxy(agent, realm, k);
                agent.put_data(g, key, w, false, true, true);
            }
            None => {
                if agent.heap[g].props.get(&key).is_none() {
                    break;
                }
                agent.heap[g].props.remove(&key);
            }
        }
        i += 1;
    }
}

/// ¿Las ventanas `a` y `b` son del mismo origen? (Los marcos con `sandbox` sin
/// `allow-same-origin` tienen origen opaco.)
pub fn same_origin(agent: &Agent, a: RealmId, b: RealmId) -> bool {
    if a == b {
        return true;
    }
    let (oa, ob) = (origin_string(agent, a), origin_string(agent, b));
    oa != "null" && oa == ob
}

/// La serialización del origen de la ventana `realm` (HTML §7.1.1): `null` si es opaco;
/// `about:blank` y `about:srcdoc` heredan el del padre.
pub fn origin_string(agent: &Agent, realm: RealmId) -> String {
    let mut r = realm;
    for _ in 0..=MAX_DEPTH {
        if frame_of(agent, r).is_some_and(|f| f.opaque) {
            return "null".to_owned();
        }
        let Some(ctx) = agent.realm_data::<WebContext>(r) else {
            return "null".to_owned();
        };
        match parent_of(agent, r) {
            Some(p) if ctx.url.scheme == "about" => r = p,
            _ => return super::origin_of(&ctx.url),
        }
    }
    "null".to_owned()
}

// --- Las propiedades de JavaScript ---

/// El marco del `<iframe>` de `this` (lo crea si todavía no existe).
fn frame_for_this(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Option<RealmId>> {
    let n = crate::dom::this_node(agent, a)?;
    let realm = WebContextHandle::current(agent).realm;
    if n.doc != 0 {
        return Ok(None);
    }
    let connected = WebContextHandle { realm }
        .get(agent)
        .document()
        .is_connected(n.node);
    if !connected {
        return Ok(None);
    }
    let existing = frames(agent).and_then(|f| {
        f.list
            .iter()
            .find(|f| f.parent == realm && f.element == n.node)
            .map(|f| f.realm)
    });
    if let Some(r) = existing {
        return Ok(Some(r));
    }
    // "When an iframe element is inserted into a document": el marco nace con su
    // `about:blank`; el resto lo navega `sync_frames`.
    let child = create_frame(agent, realm, n.node);
    index_children(agent, realm);
    Ok(Some(child))
}

/// `iframe.contentWindow` (HTML §4.8.5).
fn content_window(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let viewer = WebContextHandle::current(agent).realm;
    Ok(match frame_for_this(agent, a)? {
        Some(r) => window_proxy(agent, viewer, r),
        None => Value::Null,
    })
}

/// `iframe.contentDocument`: solo si el documento del marco es del mismo origen.
fn content_document(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let caller = WebContextHandle::current(agent).realm;
    let Some(r) = frame_for_this(agent, a)? else {
        return Ok(Value::Null);
    };
    if !same_origin(agent, caller, r) {
        return Ok(Value::Null);
    }
    Ok(wrap_document(agent, r))
}

/// El `Document` de la ventana `realm` (su wrapper es de ese realm).
fn wrap_document(agent: &mut Agent, realm: RealmId) -> Value {
    let root = WebContextHandle { realm }.get(agent).document().root();
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let v = crate::dom::wrap_node(agent, NodeRef::new(0, root));
    agent.current_realm = saved;
    v
}

/// `window.parent`.
pub fn window_parent(agent: &mut Agent) -> Value {
    let realm = WebContextHandle::current(agent).realm;
    let p = parent_of(agent, realm).unwrap_or(realm);
    window_proxy(agent, realm, p)
}

/// `window.top`.
pub fn window_top(agent: &mut Agent) -> Value {
    let realm = WebContextHandle::current(agent).realm;
    let t = top_of(agent, realm);
    window_proxy(agent, realm, t)
}

/// `window.length`: cuántos marcos hijos tiene.
pub fn window_length(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let realm = WebContextHandle::current(agent).realm;
    Ok(Value::Number(children_of(agent, realm).len() as f64))
}

/// `window.frameElement` (HTML §7.2.3.2): el `<iframe>` si el padre es del mismo origen.
pub fn frame_element(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let realm = WebContextHandle::current(agent).realm;
    let Some(f) = frame_of(agent, realm).cloned() else {
        return Ok(Value::Null);
    };
    if !same_origin(agent, realm, f.parent) {
        return Ok(Value::Null);
    }
    let saved = agent.current_realm;
    agent.current_realm = f.parent;
    let v = crate::dom::wrap_node(agent, NodeRef::new(0, f.element));
    agent.current_realm = saved;
    Ok(v)
}

/// Agrega `contentWindow` y `contentDocument` reales a `HTMLIFrameElement` en `realm`.
pub fn install(agent: &mut Agent, realm: RealmId) {
    let class = crate::dom::classes(agent)
        .element_classes
        .get(&(Namespace::Html, DomString::from("iframe")))
        .copied();
    let Some(class) = class else {
        return;
    };
    let proto = agent.host_class_objects(realm, class).prototype;
    super::accessor(agent, proto, "contentWindow", content_window, None);
    super::accessor(agent, proto, "contentDocument", content_document, None);
}

// --- E4: el navegador (pintado y eventos de los marcos) ---

/// Un marco visto por el navegador: su `<iframe>`, su ventana y una copia de su documento.
#[derive(Debug, Clone)]
pub struct FrameView {
    pub element: NodeId,
    pub realm: RealmId,
    pub doc: Document,
    pub base: Url,
    pub children: Vec<FrameView>,
}

/// Los marcos de la ventana `realm` (y los suyos), en orden de documento.
pub fn frame_views(agent: &Agent, realm: RealmId) -> Vec<FrameView> {
    frame_views_at(agent, realm, 0)
}

fn frame_views_at(agent: &Agent, realm: RealmId, depth: usize) -> Vec<FrameView> {
    if depth > MAX_DEPTH {
        return Vec::new();
    }
    children_of(agent, realm)
        .into_iter()
        .filter_map(|r| {
            let f = frame_of(agent, r)?;
            let ctx = agent.realm_data::<WebContext>(r)?;
            Some(FrameView {
                element: f.element,
                realm: r,
                doc: ctx.document().clone(),
                base: ctx.base.clone(),
                children: frame_views_at(agent, r, depth + 1),
            })
        })
        .collect()
}

/// La ventana a la que lleva `path` (los `<iframe>`, de afuera hacia adentro) desde `realm`.
pub fn frame_at_path(agent: &Agent, realm: RealmId, path: &[NodeId]) -> Option<RealmId> {
    let mut r = realm;
    for &el in path {
        r = frames(agent)?
            .list
            .iter()
            .find(|f| f.parent == r && f.element == el)?
            .realm;
    }
    Some(r)
}

/// Navega el marco `realm` a `url` (un enlace dentro del marco, HTML §7.4.2.2 con el propio
/// marco como navegable).
pub fn navigate_frame(agent: &mut Agent, realm: RealmId, url: Url) {
    set_pending(agent, realm, Some((url, false)));
    let top = top_of(agent, realm);
    sync_frames(agent, top);
}

// --- E4: WindowProxy entre orígenes (HTML §7.2.3.1, §7.2.1.3) ---

/// "CrossOriginProperties" de `Window` (HTML §7.2.1.3.1); además, los índices de los marcos.
const CROSS_ORIGIN: [&str; 13] = [
    "window",
    "self",
    "location",
    "close",
    "closed",
    "focus",
    "blur",
    "frames",
    "length",
    "top",
    "opener",
    "parent",
    "postMessage",
];

/// Las vistas restringidas ya creadas: quién mira, qué ventana, el `Proxy` y su objeto
/// interior (el mismo objeto cada vez, como el `WindowProxy`).
#[derive(Debug, Default)]
struct CrossOriginViews(Vec<CrossOriginView>);

#[derive(Debug, Clone, Copy)]
struct CrossOriginView {
    viewer: RealmId,
    target: RealmId,
    proxy: HostHandle,
    inner: HostHandle,
}

fn views(agent: &mut Agent) -> &mut CrossOriginViews {
    if agent.embedder_data::<CrossOriginViews>().is_none() {
        agent.set_embedder_data(CrossOriginViews::default());
    }
    agent
        .embedder_data_mut::<CrossOriginViews>()
        .expect("CrossOriginViews")
}

/// La vista de la que `v` es el `Proxy` o el objeto interior.
fn view_of(agent: &Agent, v: &Value) -> Option<CrossOriginView> {
    let Value::Object(o) = v else {
        return None;
    };
    let is = |h: HostHandle| matches!(agent.host_pinned(h), Some(Value::Object(x)) if x == *o);
    agent
        .embedder_data::<CrossOriginViews>()?
        .0
        .iter()
        .find(|w| is(w.proxy) || is(w.inner))
        .copied()
}

/// El `WindowProxy` de la ventana `target` visto desde la ventana `viewer`: el global si son
/// del mismo origen; si no, un objeto que solo deja usar las propiedades entre orígenes.
pub fn window_proxy(agent: &mut Agent, viewer: RealmId, target: RealmId) -> Value {
    if same_origin(agent, viewer, target) {
        return Value::Object(agent.realms[target].global_object);
    }
    let existing = agent.embedder_data::<CrossOriginViews>().and_then(|v| {
        v.0.iter()
            .find(|w| w.viewer == viewer && w.target == target)
            .copied()
    });
    if let Some(w) = existing
        && let Some(p) = agent.host_pinned(w.proxy)
    {
        return p;
    }
    let saved = agent.current_realm;
    agent.current_realm = viewer;
    let proxy = cross_origin_object(agent, viewer, target);
    agent.current_realm = saved;
    proxy.unwrap_or(Value::Null)
}

fn cross_origin_object(agent: &mut Agent, viewer: RealmId, target: RealmId) -> JsResult<Value> {
    let inner = agent.new_object(None);
    for name in ["window", "self", "frames"] {
        super::accessor(agent, inner, name, xo_self, None);
    }
    super::accessor(agent, inner, "parent", xo_parent, None);
    super::accessor(agent, inner, "top", xo_top, None);
    super::accessor(agent, inner, "length", xo_length, None);
    super::accessor(agent, inner, "closed", xo_false, None);
    super::accessor(agent, inner, "opener", xo_null, None);
    super::accessor(
        agent,
        inner,
        "location",
        xo_location,
        Some(xo_set_location),
    );
    super::method(agent, inner, "postMessage", 1, xo_post_message);
    for name in ["close", "focus", "blur"] {
        super::method(agent, inner, name, 0, xo_noop);
    }
    let handler = agent.new_plain_object();
    super::method(agent, handler, "get", 3, xo_trap_get);
    super::method(agent, handler, "set", 4, xo_trap_set);
    super::method(agent, handler, "getPrototypeOf", 1, xo_trap_proto);
    super::method(agent, handler, "defineProperty", 3, xo_trap_deny);
    super::method(agent, handler, "deleteProperty", 2, xo_trap_deny);
    let g = agent.realms[viewer].global_object;
    let Value::Object(ctor) = agent.get(g, &PropertyKey::from("Proxy"), Value::Object(g))? else {
        return Ok(Value::Null);
    };
    let proxy = agent.construct(
        ctor,
        &[Value::Object(inner), Value::Object(handler)],
        None,
    )?;
    let view = CrossOriginView {
        viewer,
        target,
        proxy: agent.host_pin(proxy.clone()),
        inner: agent.host_pin(Value::Object(inner)),
    };
    views(agent).0.push(view);
    Ok(proxy)
}

/// `SecurityError` (HTML §7.2.3.1: acceso a una propiedad que no es entre orígenes).
fn security_error<T>(agent: &mut Agent, what: &str) -> JsResult<T> {
    crate::net::dom_error(
        agent,
        "SecurityError",
        &format!("Blocked a frame from accessing a cross-origin frame ({what})."),
    )
}

/// `[[Get]]` (HTML §7.2.3.7 "CrossOriginGet"): solo las propiedades entre orígenes y los
/// índices de los marcos; los símbolos y `then` dan `undefined` (§7.2.3.7 paso 2,
/// "CrossOriginPropertyFallback").
fn xo_trap_get(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let target = a.arg(0);
    let key = agent.to_property_key(&a.arg(1))?;
    let Some(view) = view_of(agent, &target) else {
        return Ok(Value::Undefined);
    };
    match &key {
        PropertyKey::Index(i) => {
            let kids = children_of(agent, view.target);
            match kids.get(*i as usize) {
                Some(&k) => Ok(window_proxy(agent, view.viewer, k)),
                None => security_error(agent, &i.to_string()),
            }
        }
        PropertyKey::Symbol(_) => Ok(Value::Undefined),
        PropertyKey::String(s) => {
            let name = s.to_string();
            if CROSS_ORIGIN.contains(&name.as_str()) {
                let Value::Object(o) = target else {
                    return Ok(Value::Undefined);
                };
                agent.get(o, &key, a.arg(2))
            } else if name == "then" {
                Ok(Value::Undefined)
            } else {
                security_error(agent, &name)
            }
        }
    }
}

/// `[[Set]]` (HTML §7.2.3.8 "CrossOriginSet"): solo `location`.
fn xo_trap_set(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let key = agent.to_property_key(&a.arg(1))?;
    if key.to_string() == "location" {
        return match location_target(agent, &a.arg(0)) {
            Some(v) => {
                xo_navigate(agent, v, &a.arg(2), false)?;
                Ok(Value::from_bool(true))
            }
            None => Ok(Value::from_bool(false)),
        };
    }
    let what = key.to_string();
    security_error(agent, &what)
}

/// `[[GetPrototypeOf]]` de un objeto entre orígenes: `null` (HTML §7.2.3.2).
fn xo_trap_proto(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Null)
}

/// `[[DefineOwnProperty]]` y `[[Delete]]`: `SecurityError` (HTML §7.2.3.5, §7.2.3.9).
fn xo_trap_deny(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let key = agent.to_property_key(&a.arg(1))?;
    let what = key.to_string();
    security_error(agent, &what)
}

fn location_target(agent: &Agent, v: &Value) -> Option<CrossOriginView> {
    view_of(agent, v)
}

fn xo_self(_: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(a.this.clone())
}

fn xo_parent(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(v) = view_of(agent, &a.this) else {
        return Ok(Value::Null);
    };
    let p = parent_of(agent, v.target).unwrap_or(v.target);
    Ok(window_proxy(agent, v.viewer, p))
}

fn xo_top(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(v) = view_of(agent, &a.this) else {
        return Ok(Value::Null);
    };
    let t = top_of(agent, v.target);
    Ok(window_proxy(agent, v.viewer, t))
}

fn xo_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = view_of(agent, &a.this).map_or(0, |v| children_of(agent, v.target).len());
    Ok(Value::Number(n as f64))
}

fn xo_false(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::from_bool(false))
}

fn xo_null(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Null)
}

fn xo_noop(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Undefined)
}

/// Las `Location` entre orígenes ya creadas: el objeto y la vista de su ventana.
#[derive(Debug, Default)]
struct CrossOriginLocations(Vec<(HostHandle, CrossOriginView)>);

/// `location` entre orígenes (HTML §7.10.6): solo `href` (de escritura) y `replace()`, que
/// navegan la otra ventana.
fn xo_location(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(view) = view_of(agent, &a.this) else {
        return Ok(Value::Null);
    };
    let existing = agent
        .embedder_data::<CrossOriginLocations>()
        .and_then(|l| {
            l.0.iter()
                .find(|(_, v)| v.viewer == view.viewer && v.target == view.target)
                .map(|(h, _)| *h)
        });
    if let Some(v) = existing.and_then(|h| agent.host_pinned(h)) {
        return Ok(v);
    }
    let loc = agent.new_object(None);
    super::accessor(agent, loc, "href", xo_href_denied, Some(xo_set_href));
    super::method(agent, loc, "replace", 1, xo_replace);
    let h = agent.host_pin(Value::Object(loc));
    if agent.embedder_data::<CrossOriginLocations>().is_none() {
        agent.set_embedder_data(CrossOriginLocations::default());
    }
    if let Some(l) = agent.embedder_data_mut::<CrossOriginLocations>() {
        l.0.push((h, view));
    }
    Ok(Value::Object(loc))
}

/// La ventana de una `Location` entre orígenes.
fn location_view(agent: &Agent, this: &Value) -> Option<CrossOriginView> {
    let Value::Object(o) = this else {
        return None;
    };
    agent
        .embedder_data::<CrossOriginLocations>()?
        .0
        .iter()
        .find(|(h, _)| matches!(agent.host_pinned(*h), Some(Value::Object(x)) if x == *o))
        .map(|(_, v)| *v)
}

fn xo_href_denied(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    security_error(agent, "href")
}

/// Navega la ventana de la vista `view` a `href` (relativa a la URL base de quien llama).
fn xo_navigate(
    agent: &mut Agent,
    view: CrossOriginView,
    href: &Value,
    replace: bool,
) -> JsResult<Value> {
    let href = super::to_rust_string(agent, href)?;
    let base = WebContextHandle::current(agent).get(agent).base.clone();
    let Ok(url) = Url::parse(href.trim(), Some(&base)) else {
        return crate::net::dom_error(
            agent,
            "SyntaxError",
            &format!("'{href}' is not a valid URL."),
        );
    };
    if let Some(s) = agent.realm_data_mut::<super::BomState>(view.target) {
        s.hooks.navigate(&url, replace);
    }
    Ok(Value::Undefined)
}

fn xo_set_href(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    match location_view(agent, &a.this) {
        Some(v) => xo_navigate(agent, v, &a.arg(0), false),
        None => Ok(Value::Undefined),
    }
}

fn xo_replace(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    match location_view(agent, &a.this) {
        Some(v) => xo_navigate(agent, v, &a.arg(0), true),
        None => Ok(Value::Undefined),
    }
}

/// `otherWindow.location = url` entre orígenes (`[PutForwards=href]`).
fn xo_set_location(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    match view_of(agent, &a.this) {
        Some(v) => xo_navigate(agent, v, &a.arg(0), false),
        None => Ok(Value::Undefined),
    }
}

fn xo_post_message(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(v) = view_of(agent, &a.this) else {
        return Ok(Value::Undefined);
    };
    post_to(agent, v.target, a)
}

// --- E4: postMessage (HTML §9.3.3) ---

/// `window.postMessage(message, targetOrigin, transfer)` o `postMessage(message, options)`
/// sobre el global de una ventana (de este realm o de otro del mismo origen).
pub fn post_message(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let target = match &a.this {
        Value::Object(o) => (0..agent.realms.len())
            .find(|&r| agent.realms[r].global_object == *o)
            .unwrap_or(agent.current_realm),
        _ => agent.current_realm,
    };
    post_to(agent, target, a)
}

/// "Window post message steps" (HTML §9.3.3).
fn post_to(agent: &mut Agent, target: RealmId, a: &CallArgs<'_>) -> JsResult<Value> {
    // La ventana que llama (el "incumbent settings object"): la del script que corre.
    let incumbent = agent
        .frames
        .last()
        .map_or(agent.current_realm, |f| f.realm);
    let (target_origin, transfer) = match a.arg(1) {
        Value::Object(o) => {
            let t = agent.get(o, &PropertyKey::from("targetOrigin"), Value::Object(o))?;
            let t = match t {
                Value::Undefined => "/".to_owned(),
                v => super::to_rust_string(agent, &v)?,
            };
            let tr = agent.get(o, &PropertyKey::from("transfer"), Value::Object(o))?;
            (t, tr)
        }
        Value::Undefined if a.len() < 2 => ("/".to_owned(), Value::Undefined),
        v => (super::to_rust_string(agent, &v)?, a.arg(2)),
    };
    // Pasos 3–4: "/" es el origen de quien llama, "*" cualquiera; si no, una URL.
    let wanted = match target_origin.as_str() {
        "/" => Some(origin_string(agent, incumbent)),
        "*" => None,
        s => match Url::parse(s, None) {
            Ok(u) => Some(super::origin_of(&u)),
            Err(_) => {
                return crate::net::dom_error(
                    agent,
                    "SyntaxError",
                    &format!("Invalid target origin '{s}' in a call to 'postMessage'."),
                );
            }
        },
    };
    // Paso 5: StructuredSerializeWithTransfer (el clon se crea en el realm de destino).
    let g = agent.realms[target].global_object;
    let sc = agent.get(g, &PropertyKey::from("structuredClone"), Value::Object(g))?;
    let opts = agent.new_plain_object();
    if !matches!(transfer, Value::Undefined | Value::Null) {
        agent.put_data(
            opts,
            PropertyKey::from("transfer"),
            transfer,
            true,
            true,
            true,
        );
    }
    let data = agent.call(&sc, Value::Undefined, &[a.arg(0), Value::Object(opts)])?;
    let origin = origin_string(agent, incumbent);
    let data = agent.host_pin(data);
    agent.queue_task(
        TaskSource::PostedMessage,
        Box::new(move |agent: &mut Agent| {
            let data = agent.host_unpin(data).unwrap_or_default();
            if agent.realm_data::<WebContext>(target).is_none() {
                return;
            }
            // El origen del destino ya no es el pedido (navegó): no se entrega.
            if wanted
                .as_ref()
                .is_some_and(|w| *w != origin_string(agent, target))
            {
                return;
            }
            let saved = agent.current_realm;
            agent.current_realm = target;
            let source = window_proxy(agent, target, incumbent);
            let ports = match agent.create_array_from_list(&[]) {
                Ok(arr) => {
                    let _ = agent.set_integrity_level(arr, true);
                    Value::Object(arr)
                }
                Err(_) => Value::Null,
            };
            let ev =
                crate::net::channels::create_message_event(agent, "message", data, &origin, "", ports);
            if let Some(s) = agent.host_slots_mut(ev)
                && let Some(slot) = s.get_mut(6)
            {
                *slot = source;
            }
            let pin = agent.host_pin(Value::Object(ev));
            let _ =
                crate::dom::events::dispatch_js_event(agent, 0, EventTarget::Window, ev, false);
            agent.host_unpin(pin);
            agent.current_realm = saved;
        }),
    );
    Ok(Value::Undefined)
}
