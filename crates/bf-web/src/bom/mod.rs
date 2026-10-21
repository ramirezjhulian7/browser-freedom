//! El Browser Object Model: `navigator`, `location`, `history`, `screen`, `localStorage`/
//! `sessionStorage`, `document.cookie` y los diálogos, más la ejecución de los scripts del
//! documento ([`script`]: `document.write()` y compañía). **Dueño: E4.**
//!
//! El estado de cada ventana es un [`BomState`] guardado en el realm (junto al
//! [`crate::WebContext`]); lo que depende del navegador (navegar, el historial de sesión, la
//! jarra de cookies, la persistencia de `localStorage`, la pantalla) pasa por [`BomHooks`].
//! Los objetos de la ventana (`navigator`, `location`, …) se crean una vez y quedan fijados
//! (`host_pin`): son siempre el mismo objeto.

pub mod document;
pub mod frames;
pub mod history;
pub mod location;
pub mod navigator;
pub mod performance;
pub mod screen;
pub mod script;
pub mod storage;

use bf_infra::Url;
use bf_js::host::{HostClassId, HostHandle, HostKey};
use bf_js::object::{Prop, PropValue};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::{CallArgs, NativeFn};
use bf_js::vm::{Agent, JsResult, RealmId};

use crate::WebContextHandle;
use crate::dom::to_dom_string;

/// Lo que el navegador implementa para el BOM. Todo tiene un valor por defecto para un
/// entorno sin interfaz (tests, línea de comandos).
pub trait BomHooks {
    /// Navegar la ventana a `url` (`location.href = …`, `location.assign`); `replace` para
    /// `location.replace` (no agrega una entrada al historial).
    fn navigate(&mut self, _url: &Url, _replace: bool) {}
    /// La URL del documento cambió sin cargar otro (fragmento, `pushState`, traversal dentro
    /// del documento); `replace` si no se agregó una entrada al historial.
    fn same_document_navigation(&mut self, _url: &Url, _replace: bool) {}
    /// `location.reload()`.
    fn reload(&mut self) {}
    /// `history.go(delta)` hacia otro documento.
    fn traverse(&mut self, _delta: i32) {}
    /// Cuántas entradas tiene el historial de sesión de la pestaña hasta el documento actual
    /// inclusive, y cuántas hay después (para `history.length`).
    fn session_history(&self) -> (u32, u32) {
        (1, 0)
    }
    /// `document.cookie`: las cookies de `url` sin las `HttpOnly`.
    fn cookies(&mut self, _url: &Url) -> String {
        String::new()
    }
    /// `document.cookie = value`.
    fn set_cookie(&mut self, _url: &Url, _value: &str) {}
    /// ¿El navegador acepta cookies? (`navigator.cookieEnabled`.)
    fn cookies_enabled(&self) -> bool {
        true
    }
    /// El contenido guardado del `localStorage` de `origin`.
    fn load_storage(&mut self, _origin: &str) -> Vec<(String, String)> {
        Vec::new()
    }
    /// Guarda el `localStorage` de `origin`.
    fn save_storage(&mut self, _origin: &str, _items: &[(String, String)]) {}
    /// Cambió el `localStorage` de `origin` (`key` `None`: `clear()`): el navegador avisa a
    /// las otras ventanas de ese origen ([`script::storage_event`]).
    fn storage_changed(
        &mut self,
        _origin: &str,
        _key: Option<&str>,
        _old: Option<&str>,
        _new: Option<&str>,
        _url: &str,
    ) {
    }
    /// La pantalla (`window.screen`).
    fn screen(&self) -> ScreenInfo {
        ScreenInfo::default()
    }
    /// `navigator.languages` (el primero es `navigator.language`).
    fn languages(&self) -> Vec<String> {
        vec!["en-US".to_owned(), "en".to_owned()]
    }
    /// `navigator.hardwareConcurrency`.
    fn hardware_concurrency(&self) -> u32 {
        4
    }
    /// `window.close()`.
    fn close(&mut self) {}
    /// `window.focus()`.
    fn focus(&mut self) {}
    /// La URL del documento que llevó a este (`document.referrer`).
    fn referrer(&self) -> String {
        String::new()
    }
    /// Los hooks para un marco (`<iframe>`) de esta ventana: los del BOM (cookies,
    /// almacenamiento, pantalla) y los de la ventana (diálogos). `None`: sin navegador.
    fn child_hooks(&self) -> Option<(Box<dyn BomHooks>, Box<dyn crate::WebHooks>)> {
        None
    }
}

/// Hooks sin efectos.
#[derive(Debug, Default)]
pub struct NoBomHooks;

impl BomHooks for NoBomHooks {}

/// Las medidas de la pantalla (CSSOM View §4.3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenInfo {
    pub width: f64,
    pub height: f64,
    pub avail_width: f64,
    pub avail_height: f64,
    pub avail_left: f64,
    pub avail_top: f64,
    pub color_depth: u32,
    pub device_pixel_ratio: f64,
}

impl Default for ScreenInfo {
    fn default() -> Self {
        ScreenInfo {
            width: 1920.0,
            height: 1080.0,
            avail_width: 1920.0,
            avail_height: 1040.0,
            avail_left: 0.0,
            avail_top: 0.0,
            color_depth: 24,
            device_pixel_ratio: 1.0,
        }
    }
}

/// Los ids de las clases del BOM (uno por agente).
#[derive(Debug, Clone)]
pub struct BomClasses {
    pub navigator: HostClassId,
    pub ua_data: HostClassId,
    pub plugin_array: HostClassId,
    pub mime_type_array: HostClassId,
    pub location: HostClassId,
    pub history: HostClassId,
    pub screen: HostClassId,
    pub screen_orientation: HostClassId,
    pub storage: HostClassId,
    pub performance: HostClassId,
    pub performance_timing: HostClassId,
    pub performance_navigation: HostClassId,
    pub idle_deadline: HostClassId,
}

/// Los ids de las clases del BOM del agente.
pub fn bom_classes(agent: &Agent) -> &BomClasses {
    agent
        .embedder_data::<BomClasses>()
        .expect("bf_web::bom::register no se llamó")
}

/// Los objetos de la ventana, fijados para que sean siempre los mismos.
#[derive(Debug, Default, Clone, Copy)]
pub struct BomObjects {
    pub navigator: Option<HostHandle>,
    pub location: Option<HostHandle>,
    pub history: Option<HostHandle>,
    pub screen: Option<HostHandle>,
    pub session_storage: Option<HostHandle>,
    pub local_storage: Option<HostHandle>,
    pub performance: Option<HostHandle>,
}

/// El estado del BOM de una ventana (dato del realm).
pub struct BomState {
    pub hooks: Box<dyn BomHooks>,
    pub objects: BomObjects,
    pub history: history::SessionHistory,
    pub session_storage: storage::StorageArea,
    /// Se carga (de los hooks) la primera vez que se usa.
    pub local_storage: Option<storage::StorageArea>,
    /// `performance.timeOrigin` (ms desde la época Unix).
    pub time_origin: f64,
    /// El `timeOrigin` en el reloj del motor (`HostHooks::now`).
    pub now_origin: f64,
    /// Los instantes de la carga (`performance.timing`).
    pub timing: performance::Timing,
    /// `window.name`.
    pub name: String,
    /// `window.closed`.
    pub closed: bool,
    /// ¿La pestaña está a la vista? (`document.visibilityState`.)
    pub visible: bool,
}

impl BomState {
    pub fn new(hooks: Box<dyn BomHooks>, url: &Url) -> BomState {
        let time_origin = performance::epoch_ms();
        BomState {
            hooks,
            objects: BomObjects::default(),
            history: history::SessionHistory::new(url),
            session_storage: storage::StorageArea::default(),
            local_storage: None,
            time_origin,
            now_origin: 0.0,
            timing: performance::Timing::starting_at(time_origin),
            name: String::new(),
            closed: false,
            visible: true,
        }
    }
}

/// El estado del BOM del realm en curso.
pub fn state(agent: &Agent) -> &BomState {
    let realm = WebContextHandle::current(agent).realm;
    agent
        .realm_data::<BomState>(realm)
        .expect("el realm no tiene BomState")
}

/// El estado del BOM del realm en curso, mutable.
pub fn state_mut(agent: &mut Agent) -> &mut BomState {
    let realm = WebContextHandle::current(agent).realm;
    agent
        .realm_data_mut::<BomState>(realm)
        .expect("el realm no tiene BomState")
}

/// Cambia los hooks del BOM de `realm` (los pone el navegador después de [`crate::install`]).
pub fn set_hooks(agent: &mut Agent, realm: RealmId, hooks: Box<dyn BomHooks>) {
    if let Some(s) = agent.realm_data_mut::<BomState>(realm) {
        s.hooks = hooks;
    }
}

/// Las claves de identidad de los objetos de la ventana (fuera del espacio de los nodos).
fn object_key(n: u64) -> HostKey {
    HostKey(0xFFFF_FFFE_0000_0000 | n)
}

/// Registra las clases del BOM (la primera vez) e instala los objetos de la ventana de
/// `realm` (lo llama [`crate::register_all`]).
pub fn register(agent: &mut Agent, realm: RealmId, ctx: &WebContextHandle) {
    if agent.embedder_data::<BomClasses>().is_none() {
        let et = crate::dom::classes(agent).event_target;
        let classes = BomClasses {
            navigator: agent.register_host_class(navigator::navigator_class()),
            ua_data: agent.register_host_class(navigator::ua_data_class()),
            plugin_array: agent.register_host_class(navigator::plugin_array_class("PluginArray")),
            mime_type_array: agent
                .register_host_class(navigator::plugin_array_class("MimeTypeArray")),
            location: agent.register_host_class(location::location_class()),
            history: agent.register_host_class(history::history_class()),
            screen: agent.register_host_class(screen::screen_class(et)),
            screen_orientation: agent.register_host_class(screen::orientation_class(et)),
            storage: agent.register_host_class(storage::storage_class()),
            performance: agent.register_host_class(performance::performance_class(et)),
            performance_timing: agent.register_host_class(performance::timing_class()),
            performance_navigation: agent.register_host_class(performance::navigation_class()),
            idle_deadline: agent.register_host_class(performance::idle_deadline_class()),
        };
        agent.set_embedder_data(classes);
    }
    let url = ctx.get(agent).url.clone();
    if agent.realm_data::<BomState>(realm).is_none() {
        agent.set_realm_data(realm, BomState::new(Box::new(NoBomHooks), &url));
        performance::set_now_origin(agent, realm);
    }
    let saved = agent.current_realm;
    agent.current_realm = realm;
    install_window(agent, realm);
    document::install(agent, realm);
    frames::install(agent, realm);
    agent.current_realm = saved;
}

/// Crea un objeto de la ventana (con identidad) y lo fija.
fn pinned(agent: &mut Agent, realm: RealmId, n: u64, class: HostClassId) -> HostHandle {
    let o = agent.host_wrap(realm, object_key(n), class, Box::new(()));
    agent.host_pin(Value::Object(o))
}

/// Los objetos y las propiedades propias del `Window` que pone el BOM.
fn install_window(agent: &mut Agent, realm: RealmId) {
    let c = bom_classes(agent).clone();
    let objects = BomObjects {
        navigator: Some(pinned(agent, realm, 1, c.navigator)),
        location: Some(pinned(agent, realm, 2, c.location)),
        history: Some(pinned(agent, realm, 3, c.history)),
        screen: Some(pinned(agent, realm, 4, c.screen)),
        session_storage: Some(storage::create(agent, realm, 5, false)),
        local_storage: Some(storage::create(agent, realm, 6, true)),
        performance: Some(pinned(agent, realm, 7, c.performance)),
    };
    if let Some(s) = agent.realm_data_mut::<BomState>(realm) {
        s.objects = objects;
    }
    let g = agent.realms[realm].global_object;
    // Atributos del `Window` (HTML §7.2): en un `[Global]`, propiedades propias del global.
    accessor(agent, g, "navigator", get_navigator, None);
    accessor(agent, g, "clientInformation", get_navigator, None);
    accessor(agent, g, "history", get_history, None);
    accessor(agent, g, "screen", get_screen, None);
    accessor(agent, g, "sessionStorage", get_session_storage, None);
    accessor(agent, g, "localStorage", get_local_storage, None);
    accessor(agent, g, "performance", get_performance, None);
    accessor(agent, g, "name", get_name, Some(set_name));
    accessor(agent, g, "closed", get_closed, None);
    accessor(agent, g, "opener", get_null, Some(ignore_set));
    accessor(agent, g, "status", get_empty, Some(ignore_set));
    accessor(agent, g, "length", frames::window_length, None);
    accessor(agent, g, "frameElement", frames::frame_element, None);
    accessor(agent, g, "origin", get_origin, None);
    accessor(agent, g, "isSecureContext", get_secure_context, None);
    // `location` es `[LegacyUnforgeable]` y `[PutForwards=href]`.
    let lg = agent.create_builtin_function(get_location, 0, "get location", false);
    let ls = agent.create_builtin_function(set_location, 1, "set location", false);
    agent.heap[g].props.insert(
        PropertyKey::from("location"),
        Prop {
            value: PropValue::Accessor {
                get: Some(lg),
                set: Some(ls),
            },
            enumerable: true,
            configurable: false,
        },
    );
    method(agent, g, "alert", 0, alert);
    method(agent, g, "confirm", 0, confirm);
    method(agent, g, "prompt", 0, prompt);
    method(agent, g, "open", 0, open);
    method(agent, g, "close", 0, close);
    method(agent, g, "focus", 0, focus);
    method(agent, g, "blur", 0, noop);
    method(agent, g, "print", 0, noop);
    method(agent, g, "stop", 0, noop);
    method(
        agent,
        g,
        "requestIdleCallback",
        1,
        performance::request_idle_callback,
    );
    method(
        agent,
        g,
        "cancelIdleCallback",
        1,
        performance::cancel_idle_callback,
    );
}

/// Define un accessor `{enumerable, configurable}` con funciones nativas.
pub(crate) fn accessor(
    agent: &mut Agent,
    target: bf_js::heap::Gc<bf_js::object::Object>,
    name: &str,
    get: NativeFn,
    set: Option<NativeFn>,
) {
    let g = agent.create_builtin_function(get, 0, &format!("get {name}"), false);
    let s = set.map(|f| agent.create_builtin_function(f, 1, &format!("set {name}"), false));
    agent.heap[target].props.insert(
        PropertyKey::from(name),
        Prop {
            value: PropValue::Accessor {
                get: Some(g),
                set: s,
            },
            enumerable: true,
            configurable: true,
        },
    );
}

/// Define un método `{writable, enumerable, configurable}`.
pub(crate) fn method(
    agent: &mut Agent,
    target: bf_js::heap::Gc<bf_js::object::Object>,
    name: &str,
    length: u32,
    f: NativeFn,
) {
    let m = agent.create_builtin_function(f, length, name, false);
    agent.put_data(
        target,
        PropertyKey::from(name),
        Value::Object(m),
        true,
        true,
        true,
    );
}

/// El valor fijado de un objeto de la ventana.
fn pinned_value(agent: &Agent, h: Option<HostHandle>) -> Value {
    h.and_then(|h| agent.host_pinned(h)).unwrap_or(Value::Null)
}

/// Un `DOMString` de JavaScript a `String` (los surrogates sueltos, como U+FFFD).
pub(crate) fn to_rust_string(agent: &mut Agent, v: &Value) -> JsResult<String> {
    Ok(to_dom_string(agent, v)?.as_lossy_str().to_owned())
}

/// La URL de la ventana.
pub(crate) fn window_url(agent: &Agent) -> Url {
    WebContextHandle::current(agent).get(agent).url.clone()
}

/// La serialización del origen de una URL (HTML §7.1.1, "serialization of an origin").
pub(crate) fn origin_of(url: &Url) -> String {
    url.origin().to_string()
}

fn get_navigator(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(pinned_value(agent, state(agent).objects.navigator))
}

fn get_location(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(pinned_value(agent, state(agent).objects.location))
}

/// `window.location = url`: `[PutForwards=href]`.
fn set_location(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let href = to_rust_string(agent, &a.arg(0))?;
    location::navigate_to(agent, &href, false)?;
    Ok(Value::Undefined)
}

fn get_history(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(pinned_value(agent, state(agent).objects.history))
}

fn get_screen(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(pinned_value(agent, state(agent).objects.screen))
}

fn get_session_storage(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(pinned_value(agent, state(agent).objects.session_storage))
}

fn get_local_storage(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    // Un origen opaco no tiene `localStorage` (HTML §12.2.3: SecurityError).
    if origin_of(&window_url(agent)) == "null" {
        return crate::dom::throw_dom(agent, bf_dom::DomException::SecurityError);
    }
    Ok(pinned_value(agent, state(agent).objects.local_storage))
}

fn get_performance(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(pinned_value(agent, state(agent).objects.performance))
}

fn get_name(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&state(agent).name))
}

fn set_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = to_rust_string(agent, &a.arg(0))?;
    state_mut(agent).name = name;
    Ok(Value::Undefined)
}

fn get_closed(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(state(agent).closed))
}

fn get_null(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Null)
}

fn get_empty(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(""))
}

fn ignore_set(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Undefined)
}

fn noop(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Undefined)
}

fn get_origin(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&origin_of(&window_url(agent))))
}

/// "Potentially trustworthy" (Secure Contexts §3.1): `https`, `wss`, `file`, `localhost`.
fn get_secure_context(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let url = window_url(agent);
    let host = url.hostname();
    let secure = matches!(url.scheme.as_str(), "https" | "wss" | "file" | "about")
        || host == "localhost"
        || host.ends_with(".localhost")
        || host == "127.0.0.1"
        || host == "[::1]";
    Ok(Value::Boolean(secure))
}

/// El mensaje de un diálogo: `undefined` es el string vacío.
fn dialog_message(agent: &mut Agent, v: &Value) -> JsResult<String> {
    if v.is_undefined() {
        return Ok(String::new());
    }
    to_rust_string(agent, v)
}

/// `window.alert(message)` (HTML §8.8.1).
fn alert(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let message = dialog_message(agent, &a.arg(0))?;
    WebContextHandle::current(agent)
        .get_mut(agent)
        .hooks
        .alert(&message);
    Ok(Value::Undefined)
}

/// `window.confirm(message)`.
fn confirm(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let message = dialog_message(agent, &a.arg(0))?;
    let ok = WebContextHandle::current(agent)
        .get_mut(agent)
        .hooks
        .confirm(&message);
    Ok(Value::Boolean(ok))
}

/// `window.prompt(message, default)`.
fn prompt(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let message = dialog_message(agent, &a.arg(0))?;
    let default = dialog_message(agent, &a.arg(1))?;
    let r = WebContextHandle::current(agent)
        .get_mut(agent)
        .hooks
        .prompt(&message, &default);
    Ok(r.map_or(Value::Null, |s| Value::str(&s)))
}

/// `window.open(url, target, features)` (HTML §7.2.2.1): el navegador abre una pestaña; no
/// hay `WindowProxy` de otra ventana, así que devuelve `null`.
fn open(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let url = dialog_message(agent, &a.arg(0))?;
    let target = if a.arg(1).is_undefined() {
        "_blank".to_owned()
    } else {
        to_rust_string(agent, &a.arg(1))?
    };
    let features = dialog_message(agent, &a.arg(2))?;
    let base = WebContextHandle::current(agent).get(agent).base.clone();
    let resolved = if url.is_empty() {
        "about:blank".to_owned()
    } else {
        match Url::parse(&url, Some(&base)) {
            Ok(u) => u.href(),
            Err(_) => return crate::dom::throw_dom(agent, bf_dom::DomException::SyntaxError),
        }
    };
    WebContextHandle::current(agent)
        .get_mut(agent)
        .hooks
        .open(&resolved, &target, &features);
    Ok(Value::Null)
}

/// `window.close()`: solo cierra ventanas abiertas por script; el navegador decide.
fn close(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    state_mut(agent).hooks.close();
    Ok(Value::Undefined)
}

fn focus(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    state_mut(agent).hooks.focus();
    Ok(Value::Undefined)
}
