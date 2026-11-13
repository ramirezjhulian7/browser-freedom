//! Las funciones constructoras heredadas de HTML (`[LegacyFactoryFunction]`, WebIDL §3.7.2):
//! `new Image(width, height)` (HTML §4.8.3), `new Audio(src)` (§4.8.9) y
//! `new Option(text, value, defaultSelected, selected)` (§4.10.10). **Dueño: E3.**
//!
//! También la carga de las imágenes que no están en el documento (la "precarga" de
//! `new Image()`, §4.8.4.3.5 *update the image data*): al poner `src` en un `<img>`
//! desconectado se busca la imagen con la red de la ventana ([`crate::net::start`], la misma
//! que `fetch`) y se dispara `load` (con el tamaño natural en [`super::misc::ImageSizes`]) o
//! `error` en una tarea. Las imágenes conectadas las carga el navegador
//! ([`crate::WebHooks::resource_needed`]).

use std::collections::HashMap;

use bf_dom::events::{EventInit, EventKind, EventTarget};
use bf_infra::DomString;
use bf_js::host::{HostHandle, webidl};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, RealmId};

use super::elements::{class_named, set_attr};
use crate::dom::{mutated, wrap_node};
use crate::{NodeRef, WebContextHandle, with_doc, with_doc_mut};

const HTML_NS: &str = "http://www.w3.org/1999/xhtml";

/// Las tres funciones: nombre, interfaz cuyo `prototype` comparten y la función.
const FACTORIES: [(&str, &str, bf_js::vm::function::NativeFn); 3] = [
    ("Image", "HTMLImageElement", image_ctor),
    ("Audio", "HTMLAudioElement", audio_ctor),
    ("Option", "HTMLOptionElement", option_ctor),
];

/// Define `Image`, `Audio` y `Option` en el global de `realm` (lo llama
/// [`super::register`], después de registrar las interfaces).
pub fn register(agent: &mut Agent, realm: RealmId) {
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let g = agent.realms[realm].global_object;
    for (name, interface, f) in FACTORIES {
        let Some(class) = class_named(agent, interface) else {
            continue;
        };
        let proto = agent.host_class_objects(realm, class).prototype;
        // WebIDL §3.7.2: `length` es el del overload más corto (0 en los tres), `name` el de
        // la función y `prototype` el objeto prototipo de la interfaz, no escribible.
        let ctor = agent.create_builtin_function(f, 0, name, true);
        agent.put_data(
            ctor,
            PropertyKey::from("prototype"),
            Value::Object(proto),
            false,
            false,
            false,
        );
        // Como las interfaces: escribible, no enumerable y configurable.
        agent.put_data(
            g,
            PropertyKey::from(name),
            Value::Object(ctor),
            true,
            false,
            true,
        );
    }
    agent.current_realm = saved;
}

/// Los pasos comunes: exige `new` y crea un elemento HTML `local` en el documento asociado
/// al global en curso (WebIDL §3.7.2; HTML §4.8.3 paso 1).
fn construct(agent: &mut Agent, a: &CallArgs<'_>, name: &str, local: &str) -> JsResult<NodeRef> {
    if a.new_target.is_none() {
        return agent.type_error(&format!(
            "Failed to construct '{name}': Please use the 'new' operator, this DOM object \
             constructor cannot be called as a function."
        ));
    }
    let local = DomString::from(local);
    let node = with_doc_mut(agent, 0, |d| d.create_element_ns(Some(HTML_NS), &local))
        .or_else(|e| crate::dom::throw_dom_in(agent, e, name, "constructor"))?;
    Ok(NodeRef::new(0, node))
}

/// `new Image(width, height)` (§4.8.3): un `img` con los atributos `width`/`height` si se
/// dieron.
fn image_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let width = (!a.arg(0).is_undefined())
        .then(|| webidl::to_unsigned_long(agent, &a.arg(0)))
        .transpose()?;
    let height = (!a.arg(1).is_undefined())
        .then(|| webidl::to_unsigned_long(agent, &a.arg(1)))
        .transpose()?;
    let n = construct(agent, a, "Image", "img")?;
    for (attr, v) in [("width", width), ("height", height)] {
        if let Some(v) = v {
            set_attr(agent, n, attr, DomString::from(v.to_string().as_str()))?;
        }
    }
    Ok(wrap_node(agent, n))
}

/// `new Audio(src)` (§4.8.9): un `audio` con `preload="auto"` y, si se dio, `src`.
fn audio_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let src = (!a.arg(0).is_undefined())
        .then(|| webidl::to_dom_string(agent, &a.arg(0)))
        .transpose()?;
    let n = construct(agent, a, "Audio", "audio")?;
    set_attr(agent, n, "preload", DomString::from("auto"))?;
    if let Some(src) = src {
        set_attr(agent, n, "src", DomString::from_utf16(src.units()))?;
    }
    Ok(wrap_node(agent, n))
}

/// `new Option(text, value, defaultSelected, selected)` (§4.10.10).
fn option_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let text = if a.arg(0).is_undefined() {
        DomString::new()
    } else {
        let s = webidl::to_dom_string(agent, &a.arg(0))?;
        DomString::from_utf16(s.units())
    };
    let value = (!a.arg(1).is_undefined())
        .then(|| webidl::to_dom_string(agent, &a.arg(1)))
        .transpose()?;
    let default_selected = webidl::to_boolean(agent, &a.arg(2));
    let selected = webidl::to_boolean(agent, &a.arg(3));
    let n = construct(agent, a, "Option", "option")?;
    // 3. Un `Text` hijo con `text` si no es vacío.
    if !text.is_empty() {
        let r = with_doc_mut(agent, 0, |d| {
            let t = d.create_text_node(text);
            d.append_child(n.node, t)
        });
        if let Err(e) = r {
            return crate::dom::throw_dom_in(agent, e, "Option", "constructor");
        }
    }
    // 4. `value`.
    if let Some(v) = value {
        set_attr(agent, n, "value", DomString::from_utf16(v.units()))?;
    }
    // 5. `defaultSelected`: el atributo `selected`.
    if default_selected {
        set_attr(agent, n, "selected", DomString::new())?;
    }
    // 6. La selectedness es `selected` (aunque `defaultSelected` sea `true`).
    with_doc_mut(agent, 0, |d| d.set_option_selected(n.node, selected));
    mutated(agent);
    Ok(wrap_node(agent, n))
}

// --- "Update the image data" de los `<img>` (HTML §4.8.4.3.5) ---

/// El estado de la petición actual de un `<img>` (§4.8.4.3.1, "image request state"): `Pending`
/// mientras se busca y decodifica, y después `Available` ("completely available") o `Broken`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageState {
    Pending,
    Available,
    Broken,
    /// E2: `loading=lazy` lejos del viewport: todavía no se pidió (HTML §4.8.4.3.9).
    Deferred,
}

/// "Lazy load root margin" (HTML §4.8.4.3.9; la cifra la decide el navegador): se piden las
/// imágenes perezosas a menos de esto del viewport, como hace Chrome con buena red.
const LAZY_MARGIN: f32 = 1250.0;

/// Las peticiones de imagen de la ventana: la última de cada elemento (una nueva aborta la
/// anterior, §4.8.4.3.5 paso 13: solo la vigente dispara eventos), su estado y las promesas
/// de `decode()` que esperan a que termine.
#[derive(Default)]
struct ImageRequests {
    next: u64,
    current: HashMap<NodeRef, u64>,
    state: HashMap<NodeRef, ImageState>,
    /// La URL de la petición en curso de cada elemento.
    urls: HashMap<NodeRef, String>,
    decodes: HashMap<NodeRef, Vec<HostHandle>>,
    /// E2: los `<img loading=lazy>` aplazados, que [`check_lazy_images`] pide al acercarse al
    /// viewport.
    lazy: Vec<NodeRef>,
}

/// El estado de la petición actual de `n` (`None` si nunca se pidió su imagen).
pub fn image_state(agent: &Agent, n: NodeRef) -> Option<ImageState> {
    let realm = WebContextHandle::current(agent).realm;
    agent
        .realm_data::<ImageRequests>(realm)?
        .state
        .get(&n)
        .copied()
}

/// ¿Se pide la imagen de `n`? Los `<img>` fuera del documento (`new Image()`) y los del
/// documento de la ventana; los de documentos sin browsing context (`DOMParser`,
/// `createHTMLDocument`) no son *fully active* y no cargan (§4.8.4.3.5 paso 1).
fn loads_images(agent: &Agent, n: NodeRef) -> bool {
    with_doc(agent, n.doc, |d| {
        d.element(n.node).is_some_and(|e| e.is_html("img"))
            && (n.doc == 0 || !d.is_connected(n.node))
    })
}

/// Cambió el atributo `name` de `n` (lo llaman `setAttribute` y la reflexión de `src`):
/// `src`, `srcset` y `sizes` son "relevant mutations" (§4.8.4.3.5) y vuelven a pedir la imagen.
pub fn img_attribute_changed(agent: &mut Agent, n: NodeRef, name: &str) {
    let is = |list: &[&str]| list.iter().any(|a| name.eq_ignore_ascii_case(a));
    if is(&["src", "srcset", "sizes"]) && loads_images(agent, n) {
        update_image_data(agent, n);
    }
    // Un `<source>` de un `<picture>` cambia `srcset`, `sizes`, `media` o `type`: los `<img>`
    // que lo siguen en el `<picture>` vuelven a elegir (§4.8.4.3.5, "relevant mutations").
    if is(&["srcset", "sizes", "media", "type"]) {
        let imgs = with_doc(agent, n.doc, |d| {
            let is_source = d.element(n.node).is_some_and(|e| e.is_html("source"));
            let Some(picture) = d
                .parent(n.node)
                .filter(|&p| is_source && d.element(p).is_some_and(|e| e.is_html("picture")))
            else {
                return Vec::new();
            };
            d.children(picture)
                .skip_while(|&c| c != n.node)
                .filter(|&c| d.element(c).is_some_and(|e| e.is_html("img")))
                .collect::<Vec<_>>()
        });
        for img in imgs {
            let img = NodeRef::new(n.doc, img);
            if loads_images(agent, img) {
                update_image_data(agent, img);
            }
        }
    }
}

/// E2: un `<img>` creado con sus atributos (por el parser, `innerHTML` o `cloneNode`) quedó
/// conectado al documento: si todavía no pidió su imagen, la pide ("when an img element is
/// created", §4.8.4.3.5). Uno que ya la pidió (un `src` asignado antes de insertarlo) no
/// vuelve a empezar.
pub fn img_created(agent: &mut Agent, n: NodeRef) {
    if image_state(agent, n).is_none() && loads_images(agent, n) {
        update_image_data(agent, n);
    }
}

/// Corre `f` con `doc` (el documento de la ventana `realm`, que el parser tiene prestado)
/// de vuelta en el contexto, como `window.document`: para lo que el parser avisa al insertar
/// un elemento (un `<img>`, un `<link>`).
pub(crate) fn with_lent_document(
    agent: &mut Agent,
    realm: RealmId,
    doc: &mut bf_dom::Document,
    f: impl FnOnce(&mut Agent),
) {
    let real = std::mem::take(doc);
    let saved = std::mem::replace(
        &mut WebContextHandle { realm }.get_mut(agent).documents[0],
        crate::DocumentOwner::Document(Box::new(real)),
    );
    let saved_realm = agent.current_realm;
    agent.current_realm = realm;
    f(agent);
    agent.current_realm = saved_realm;
    let back = std::mem::replace(
        &mut WebContextHandle { realm }.get_mut(agent).documents[0],
        saved,
    );
    *doc = match back {
        crate::DocumentOwner::Document(d) => *d,
        crate::DocumentOwner::Parser(p) => p.into_document().0,
    };
}

/// Marca (o desmarca) que la imagen de `n` demora el evento `load` del documento (§4.8.4.3.5:
/// la búsqueda "delays the load event"; el parser lo mira en el paso 8 de "the end"). Solo los
/// `<img>` conectados al documento de la ventana.
pub(crate) fn delay_load(agent: &mut Agent, n: NodeRef, on: bool) {
    if n.doc != 0 {
        return;
    }
    with_doc_mut(agent, n.doc, |d| {
        if on && !d.is_connected(n.node) {
            return;
        }
        d.script_state_mut(n.node).delaying_load_event = on;
    });
}

/// Deja de demorar el `load` del documento por `n` si ya no tiene ninguna petición en curso
/// (una que terminó pudo haber sido reemplazada por otra que sigue).
fn release_delay(agent: &mut Agent, realm: RealmId, n: NodeRef) {
    let busy = agent
        .realm_data::<ImageRequests>(realm)
        .is_some_and(|r| r.current.contains_key(&n));
    if !busy {
        delay_load(agent, n, false);
    }
    resume_load(agent, realm);
}

/// Vuelve a evaluar "the end" del parser de la ventana (si sigue siendo un parser): la última
/// imagen que demoraba `load` terminó.
pub(crate) fn resume_load(agent: &mut Agent, realm: RealmId) {
    let parsing = matches!(
        WebContextHandle { realm }.get(agent).documents[0],
        crate::DocumentOwner::Parser(_)
    );
    if !parsing {
        return;
    }
    // Con el host que corresponde: el de la ventana (`bom::script`, en el navegador) o el de
    // las páginas sin ventana (`html::scripting`, el runner de testharness); en el realm de esa
    // ventana (puede llegar desde la tarea de un marco).
    let saved = agent.current_realm;
    agent.current_realm = realm;
    if crate::bom::script::script_env(agent, realm).is_some() {
        crate::bom::script::with_parser(agent, realm, |p, host| p.run_tasks(host));
    } else {
        super::scripting::with_parser(agent, realm, |p, host| p.run_tasks(host));
    }
    agent.current_realm = saved;
    // Si es un marco y su documento acaba de terminar, el `load` de su `<iframe>`.
    crate::bom::frames::frame_document_loaded(agent, realm);
}

/// Rechaza con `EncodingError` las promesas de `decode()` de `n` (cambió la petición o se
/// rompió) o las cumple (`ok`).
fn settle_decodes(agent: &mut Agent, n: NodeRef, ok: bool) {
    let realm = WebContextHandle::current(agent).realm;
    let waiting = agent
        .realm_data_mut::<ImageRequests>(realm)
        .and_then(|r| r.decodes.remove(&n))
        .unwrap_or_default();
    for h in waiting {
        let Some(Value::Object(p)) = agent.host_unpin(h) else {
            continue;
        };
        if ok {
            agent.fulfill_promise(p, Value::Undefined);
        } else {
            let e = crate::dom::exception::create(
                agent,
                "EncodingError",
                "The source image cannot be decoded.",
            );
            agent.reject_promise(p, e);
        }
    }
}

/// `decode()` (§4.8.4.4): una promesa que se cumple cuando la imagen está decodificada y se
/// rechaza con `EncodingError` si está rota o si su petición cambia antes.
pub fn img_decode(agent: &mut Agent, n: NodeRef) -> Value {
    // Un `<img>` que todavía no pidió su imagen (por ejemplo, metido en un `<picture>` fuera
    // del documento) la pide ahora.
    if image_state(agent, n).is_none() && loads_images(agent, n) {
        update_image_data(agent, n);
    }
    let p = agent.new_internal_promise();
    let ok = match image_state(agent, n) {
        // Pendiente o aplazada (`loading=lazy`): espera a que termine.
        Some(ImageState::Pending | ImageState::Deferred) => {
            let h = agent.host_pin(Value::Object(p));
            let realm = WebContextHandle::current(agent).realm;
            let mut reqs = agent
                .take_realm_data::<ImageRequests>(realm)
                .unwrap_or_default();
            reqs.decodes.entry(n).or_default().push(h);
            agent.set_realm_data(realm, reqs);
            return Value::Object(p);
        }
        Some(ImageState::Available) => true,
        Some(ImageState::Broken) => false,
        // Sin petición (un documento sin browsing context): lo que se sepa de su tamaño.
        None => super::misc::has_natural_size(agent, n),
    };
    if ok {
        agent.fulfill_promise(p, Value::Undefined);
    } else {
        let e = crate::dom::exception::create(
            agent,
            "EncodingError",
            "The source image cannot be decoded.",
        );
        agent.reject_promise(p, e);
    }
    Value::Object(p)
}

/// *Update the image data* (§4.8.4.3.5) con la fuente elegida por `srcset`/`<picture>`:
/// - sin fuente: la petición queda rota y, si hay `src` o `srcset`, se dispara `error`;
/// - una imagen ya en la lista de disponibles (§4.8.4.3.2): disponible al momento y `load`
///   en una tarea;
/// - si no, se busca y se decodifica (demorando el `load` del documento) y según el
///   resultado se dispara `load` o `error`.
pub fn update_image_data(agent: &mut Agent, n: NodeRef) {
    update_image_data_with(agent, n, None);
}

/// "Will lazy load element steps" (HTML §4.8.4.3.9): `loading=lazy` en un `<img>` conectado
/// del documento de la ventana, con scripts (si no, las imágenes perezosas cargan como las
/// demás).
fn will_lazy_load(agent: &Agent, n: NodeRef) -> bool {
    let realm = WebContextHandle::current(agent).realm;
    n.doc == 0
        && !crate::bom::frames::scripting_disabled(agent, realm)
        && with_doc(agent, n.doc, |d| {
            d.is_connected(n.node)
                && d.element(n.node)
                    .and_then(|e| e.attr("loading"))
                    .is_some_and(|v| v.trim().eq_ignore_ascii_case("lazy"))
        })
}

/// E2: pide las imágenes `loading=lazy` aplazadas de la ventana `realm` cuya caja está a menos
/// de [`LAZY_MARGIN`] del viewport ("lazy load intersection observer", HTML §4.8.4.3.9). Las
/// que ya no son perezosas (cambió `loading`) también; las que no se pintan (`display: none`)
/// siguen esperando. Lo llama `bom::frames::sync_frames`, el punto en que el bucle de eventos
/// queda sin trabajo.
pub fn check_lazy_images(agent: &mut Agent, realm: RealmId) {
    let waiting = agent
        .realm_data::<ImageRequests>(realm)
        .map(|r| r.lazy.clone())
        .unwrap_or_default();
    if waiting.is_empty() {
        return;
    }
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let viewport = crate::cssom::window_metrics(agent).viewport;
    let mut ready = Vec::new();
    for n in waiting {
        let near = !will_lazy_load(agent, n)
            // Una caja vacía (la imagen sin cargar mide 0×0) también cuenta; sin caja, no.
            || crate::cssom::view::client_rect(agent, n).is_some_and(|(x, y, w, h)| {
                x + w >= -LAZY_MARGIN
                    && y + h >= -LAZY_MARGIN
                    && x <= viewport.width + LAZY_MARGIN
                    && y <= viewport.height + LAZY_MARGIN
            });
        if near {
            ready.push(n);
        }
    }
    if let Some(r) = agent.realm_data_mut::<ImageRequests>(realm) {
        r.lazy.retain(|n| !ready.contains(n));
    }
    for n in ready {
        if image_state(agent, n) == Some(ImageState::Deferred) {
            // Con la URL que se resolvió al aplazarla: la base del documento de entonces.
            let url = agent
                .realm_data::<ImageRequests>(realm)
                .and_then(|r| r.urls.get(&n).cloned())
                .and_then(|u| bf_infra::Url::parse(&u, None).ok());
            update_image_data_with(agent, n, Some(url));
        }
    }
    agent.current_realm = saved;
}

/// [`update_image_data`]. `resume`: una imagen perezosa que se pide ahora, con la URL que se
/// resolvió al aplazarla (`None` dentro: la vuelve a elegir); sin `resume`, una imagen
/// perezosa se aplaza.
fn update_image_data_with(agent: &mut Agent, n: NodeRef, resume: Option<Option<bf_infra::Url>>) {
    let realm = WebContextHandle::current(agent).realm;
    let may_defer = resume.is_none();
    let url = match resume.flatten() {
        Some(u) => Some(u),
        None => super::misc::selected_source(agent, n).map(|(u, _)| u),
    };
    let key = url.as_ref().map(|u| u.serialize(false));
    // La misma URL que la petición que sigue en curso: no se empieza otra (§4.8.4.3.5,
    // "if … urlString is the same as the pending request's current URL, return").
    let same_pending = agent.realm_data::<ImageRequests>(realm).is_some_and(|r| {
        r.state.get(&n) == Some(&ImageState::Pending)
            && r.current.contains_key(&n)
            && key.is_some()
            && r.urls.get(&n) == key.as_ref()
    });
    if same_pending {
        return;
    }
    // `loading=lazy`: sin pedirla (ni demorar `load`) hasta que se acerque al viewport. La
    // petición anterior, si la había, queda abortada.
    // Una que ya está en la lista de imágenes disponibles no espera (el paso 7 va antes).
    let available = key.as_ref().is_some_and(|k| {
        agent
            .realm_data::<super::misc::ImageSizes>(realm)
            .is_some_and(|s| s.0.contains_key(k))
    });
    if may_defer && !available && key.is_some() && will_lazy_load(agent, n) {
        let mut reqs = agent
            .take_realm_data::<ImageRequests>(realm)
            .unwrap_or_default();
        reqs.current.remove(&n);
        reqs.state.insert(n, ImageState::Deferred);
        if let Some(k) = &key {
            reqs.urls.insert(n, k.clone());
        }
        if !reqs.lazy.contains(&n) {
            reqs.lazy.push(n);
        }
        agent.set_realm_data(realm, reqs);
        return;
    }
    // Una petición nueva: los `decode()` de la anterior se rechazan.
    settle_decodes(agent, n, false);
    let mut reqs = agent
        .take_realm_data::<ImageRequests>(realm)
        .unwrap_or_default();
    reqs.next += 1;
    let id = reqs.next;
    reqs.current.insert(n, id);
    reqs.state.insert(n, ImageState::Pending);
    match &key {
        Some(k) => reqs.urls.insert(n, k.clone()),
        None => reqs.urls.remove(&n),
    };
    agent.set_realm_data(realm, reqs);
    // El objeto del elemento vive (con sus `onload`/`onerror`) mientras se carga, aunque el
    // script ya no lo referencie.
    let wrapper = wrap_node(agent, n);
    let pin = agent.host_pin(wrapper);
    let Some(url) = url else {
        let (has_source, nothing) = with_doc(agent, n.doc, |d| {
            let e = d.element(n.node);
            let src = e.and_then(|e| e.attr("src"));
            let srcset = e.and_then(|e| e.attr("srcset"));
            // Un `srcset` vacío (sin `src`) no dispara `error` (WPT `null-image-source`).
            (
                src.is_some() || srcset.is_some_and(|s| !s.trim().is_empty()),
                srcset.is_none() && src.is_none_or(str::is_empty),
            )
        });
        // Sin `src` (o vacío) ni `srcset`, la petición se rompe ya (paso 4); una URL que no
        // se puede parsear, después, en la tarea (§4.8.4.3.5 pasos 9 y 11).
        queue_finish_with(
            agent,
            realm,
            n,
            (id, pin),
            Outcome::Broken { event: has_source },
            nothing,
        );
        return;
    };
    let key = url.serialize(false);
    // Ya en la lista de imágenes disponibles: sin buscarla otra vez.
    let known = agent
        .realm_data::<super::misc::ImageSizes>(realm)
        .and_then(|s| s.0.get(&key).copied());
    if let Some(size) = known {
        queue_finish(agent, realm, n, id, pin, Outcome::Loaded(key, size));
        return;
    }
    // E6: Mixed Content (§4.1: se pide por `https:`) y CSP `img-src` (CSP 3 §6.1.6, con
    // `default-src` de respaldo): una petición bloqueada es un error de red, y la imagen queda
    // rota con `error` (HTML §4.8.4.3.5, "update the image data", paso 26).
    let Some(url) = crate::security::check_request(
        agent,
        realm,
        None,
        &url,
        bf_infra::Destination::Image,
        None,
        Some(n),
    ) else {
        queue_finish(agent, realm, n, id, pin, Outcome::Broken { event: true });
        return;
    };
    delay_load(agent, n, true);
    let started = crate::bom::performance_observer::now_ms(agent);
    let req = crate::net::NetRequest {
        url,
        // E6: el *destination* `image` (Fetch §2.2.5, HTML §4.8.4.3.5 "update the image data").
        options: bf_net::RequestOptions {
            destination: Some(bf_infra::Destination::Image),
            ..Default::default()
        },
    };
    crate::net::start(
        agent,
        req,
        Box::new(move |agent: &mut Agent, result: crate::net::NetResult| {
            // La respuesta llega en el realm de la petición (el de un marco no es el de la
            // ventana que corre el bucle de eventos).
            let saved = agent.current_realm;
            agent.current_realm = realm;
            // Resource Timing: una entrada `img` por respuesta (no por error de red).
            if let (Some(start), Ok(r)) = (started, &result) {
                let end = crate::bom::performance_observer::now_ms(agent).unwrap_or(start);
                crate::bom::performance_observer::add_resource_entry(
                    agent,
                    &key,
                    "img",
                    start,
                    end,
                    r.body.len(),
                );
            }
            let outcome = match result.ok().and_then(|r| image_size(&r)) {
                Some(size) => Outcome::Loaded(key, size),
                None => Outcome::Broken { event: true },
            };
            finish(agent, n, id, pin, outcome);
            release_delay(agent, realm, n);
            agent.current_realm = saved;
        }),
    );
}

/// E2: los `<img>` del documento de la ventana con su imagen ya disponible y su tamaño
/// natural corregido por densidad, para maquetar sin el navegador (el layout de `cssom` sin
/// proveedor, como en el runner de testharness).
pub fn layout_image_sizes(agent: &mut Agent) -> bf_layout::ImageSizes {
    let realm = WebContextHandle::current(agent).realm;
    let available: Vec<NodeRef> = agent
        .realm_data::<ImageRequests>(realm)
        .map(|r| {
            r.state
                .iter()
                .filter(|(n, s)| n.doc == 0 && **s == ImageState::Available)
                .map(|(n, _)| *n)
                .collect()
        })
        .unwrap_or_default();
    let mut out = bf_layout::ImageSizes::new();
    for n in available {
        if let Some((w, h)) = super::misc::natural_size_of(agent, n) {
            out.insert(
                n.node,
                bf_layout::ImageState::Loaded(bf_layout::Size {
                    width: w as f32,
                    height: h as f32,
                }),
            );
        }
    }
    out
}

/// Cómo terminó una petición.
enum Outcome {
    /// Decodificada: URL y tamaño en píxeles (0×0 para un SVG, sin tamaño conocido aquí).
    Loaded(String, (u32, u32)),
    /// Rota; `event`: si se dispara `error`.
    Broken { event: bool },
}

/// [`finish`] en una tarea del event loop (§4.8.4.3.5: "queue an element task").
fn queue_finish(
    agent: &mut Agent,
    realm: RealmId,
    n: NodeRef,
    id: u64,
    pin: HostHandle,
    outcome: Outcome,
) {
    queue_finish_with(agent, realm, n, (id, pin), outcome, true);
}

/// [`queue_finish`]; con `now`, el estado cambia ya (`complete` y `naturalWidth` síncronos);
/// si no, en la tarea. La tarea también demora el `load` del documento (si no, el parser lo
/// dispararía antes que el evento del elemento).
fn queue_finish_with(
    agent: &mut Agent,
    realm: RealmId,
    n: NodeRef,
    (id, pin): (u64, HostHandle),
    outcome: Outcome,
    now: bool,
) {
    if now {
        set_state(agent, realm, n, id, &outcome);
    }
    delay_load(agent, n, true);
    agent.queue_task(
        bf_js::host::TaskSource::DomManipulation,
        Box::new(move |agent: &mut Agent| {
            let saved = agent.current_realm;
            agent.current_realm = realm;
            finish(agent, n, id, pin, outcome);
            release_delay(agent, realm, n);
            agent.current_realm = saved;
        }),
    );
}

/// Si `id` sigue siendo la petición de `n`, deja su estado y su tamaño según `outcome`.
fn set_state(agent: &mut Agent, realm: RealmId, n: NodeRef, id: u64, outcome: &Outcome) -> bool {
    let current = agent
        .realm_data_mut::<ImageRequests>(realm)
        .is_some_and(|r| {
            let current = r.current.get(&n) == Some(&id);
            if current {
                let state = match outcome {
                    Outcome::Loaded(..) => ImageState::Available,
                    Outcome::Broken { .. } => ImageState::Broken,
                };
                r.state.insert(n, state);
            }
            current
        });
    if current
        && let Outcome::Loaded(url, size) = outcome
        && *size != (0, 0)
    {
        let mut sizes = agent
            .take_realm_data::<super::misc::ImageSizes>(realm)
            .unwrap_or_default();
        sizes.0.insert(url.clone(), *size);
        agent.set_realm_data(realm, sizes);
    }
    current
}

/// El tamaño natural de la imagen de la respuesta, o `None` si no es una imagen que el motor
/// sepa decodificar ([`bf_image::decode`]: PNG, JPEG, GIF, BMP, ICO y WebP). Un SVG (E2) da su
/// ancho y alto naturales (`bf-svg`, SVG 2 §8.2: lo que falte, por la proporción del
/// `viewBox`); sin ninguno de los dos cuenta como cargado sin tamaño (0×0). AVIF da `error`:
/// es lo que las detecciones de formato (`new Image()` con un `data:` de prueba) deben ver para
/// servir un formato que el motor pinta.
fn image_size(r: &crate::net::NetResponse) -> Option<(u32, u32)> {
    let mime = r.headers.get("content-type");
    // E2: la caché del proceso, compartida con `bf-paint` (que la pinta): una decodificación.
    match bf_image::decode_frames_shared(&r.body, mime.as_deref()) {
        Ok(img) => Some((img.width, img.height)),
        Err(bf_image::ImageError::Unsupported("image/svg+xml")) => {
            let svg = bf_svg::Svg::parse_bytes(&r.body).ok()?;
            let i = svg.intrinsic();
            let ratio = i.ratio.filter(|r| *r > 0.0 && r.is_finite());
            let size = match (i.width, i.height, ratio) {
                (Some(w), Some(h), _) => (w, h),
                (Some(w), None, Some(r)) => (w, w / r),
                (None, Some(h), Some(r)) => (h * r, h),
                _ => (0.0, 0.0),
            };
            let px = |v: f32| v.max(0.0).round() as u32;
            Some((px(size.0), px(size.1)))
        }
        Err(_) => None,
    }
}

/// Termina la petición `id` de `n`: si sigue vigente, deja el estado y el tamaño, dispara
/// `load` o `error` y resuelve los `decode()` que esperaban.
fn finish(agent: &mut Agent, n: NodeRef, id: u64, pin: HostHandle, outcome: Outcome) {
    agent.host_unpin(pin);
    let realm = WebContextHandle::current(agent).realm;
    if !set_state(agent, realm, n, id, &outcome) {
        return;
    }
    if let Some(r) = agent.realm_data_mut::<ImageRequests>(realm) {
        r.current.remove(&n);
    }
    let event = match outcome {
        Outcome::Loaded(..) => Some("load"),
        Outcome::Broken { event } => event.then_some("error"),
    };
    settle_decodes(agent, n, matches!(outcome, Outcome::Loaded(..)));
    // La geometría guardada (`getBoundingClientRect`, `clientWidth`) cambia con el tamaño.
    crate::cssom::invalidate_layout(agent);
    if let Some(event) = event {
        crate::dom::events::fire_event(
            agent,
            n.doc,
            EventTarget::Node(n.node),
            event,
            EventInit::default(),
            EventKind::Event,
        );
    }
}
