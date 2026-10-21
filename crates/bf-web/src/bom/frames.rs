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
//! pestaña. El acceso entre orígenes distintos no tiene los `WindowProxy` restringidos del
//! estándar: `contentWindow` da el global del marco.

use std::sync::Arc;

use bf_dom::events::{Event, EventInit, EventTarget};
use bf_dom::{Document, NodeId};
use bf_infra::encoding::{self, Encoding};
use bf_infra::{DomString, Namespace, ResourceLoader, Url};
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
                let w = agent.realms[k].global_object;
                agent.put_data(g, key, Value::Object(w), false, true, true);
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
    let opaque = |r: RealmId| frame_of(agent, r).is_some_and(|f| f.opaque);
    if opaque(a) || opaque(b) {
        return a == b;
    }
    let origin = |r: RealmId| {
        let ctx = WebContextHandle { realm: r }.get(agent);
        // `about:blank` y `about:srcdoc` heredan el origen del padre.
        if ctx.url.scheme == "about" {
            let p = parent_of(agent, r).unwrap_or(r);
            if p != r {
                return super::origin_of(&WebContextHandle { realm: p }.get(agent).url);
            }
        }
        super::origin_of(&ctx.url)
    };
    let (oa, ob) = (origin(a), origin(b));
    oa != "null" && oa == ob
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
    Ok(match frame_for_this(agent, a)? {
        Some(r) => Value::Object(agent.realms[r].global_object),
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
    Value::Object(agent.realms[p].global_object)
}

/// `window.top`.
pub fn window_top(agent: &mut Agent) -> Value {
    let realm = WebContextHandle::current(agent).realm;
    let t = top_of(agent, realm);
    Value::Object(agent.realms[t].global_object)
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
