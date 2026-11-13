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
use bf_js::host::webidl;
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

// --- La carga de los `<img>` desconectados (§4.8.4.3.5) ---

/// Las peticiones de imagen en curso de la ventana: la última de cada elemento (una nueva
/// aborta la anterior, §4.8.4.3.5 paso 13: solo la vigente dispara eventos).
#[derive(Default)]
struct ImageRequests {
    next: u64,
    current: HashMap<NodeRef, u64>,
}

/// Cambió el atributo `name` de `n` (lo llaman `setAttribute` y la reflexión de `src`).
/// Un `<img>` fuera del documento carga su imagen ("relevant mutations": se puso `src`).
pub fn img_attribute_changed(agent: &mut Agent, n: NodeRef, name: &str) {
    // E2: también `srcset` y `sizes`, que eligen la fuente (§4.8.4.3.5 "relevant mutations").
    if !["src", "srcset", "sizes"]
        .iter()
        .any(|a| name.eq_ignore_ascii_case(a))
    {
        return;
    }
    let disconnected_img = with_doc(agent, n.doc, |d| {
        d.element(n.node).is_some_and(|e| e.is_html("img")) && !d.is_connected(n.node)
    });
    if disconnected_img {
        update_image_data(agent, n);
    }
}

/// *Update the image data* (§4.8.4.3.5) con la fuente elegida por `srcset`/`<picture>`: sin fuente o
/// que no es una URL, `error`; si no, la imagen se busca y se decodifica, y según el
/// resultado se dispara `load` o `error`.
fn update_image_data(agent: &mut Agent, n: NodeRef) {
    let realm = WebContextHandle::current(agent).realm;
    let mut reqs = agent
        .take_realm_data::<ImageRequests>(realm)
        .unwrap_or_default();
    reqs.next += 1;
    let id = reqs.next;
    reqs.current.insert(n, id);
    agent.set_realm_data(realm, reqs);
    // El objeto del elemento vive (con sus `onload`/`onerror`) mientras se carga, aunque el
    // script ya no lo referencie.
    let wrapper = wrap_node(agent, n);
    let pin = agent.host_pin(wrapper);
    // E2: la fuente que eligen `srcset`/`sizes` (o `src`).
    let url = super::misc::selected_source(agent, n).map(|(u, _)| u);
    let Some(url) = url else {
        agent.queue_task(
            bf_js::host::TaskSource::DomManipulation,
            Box::new(move |agent: &mut Agent| {
                let saved = agent.current_realm;
                agent.current_realm = realm;
                finish(agent, n, id, pin, None);
                agent.current_realm = saved;
            }),
        );
        return;
    };
    let key = url.serialize(false);
    let started = crate::bom::performance_observer::now_ms(agent);
    let req = crate::net::NetRequest {
        url,
        options: bf_net::RequestOptions::default(),
    };
    crate::net::start(
        agent,
        req,
        Box::new(move |agent: &mut Agent, result: crate::net::NetResult| {
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
            let size = result.ok().and_then(|r| image_size(&r));
            finish(agent, n, id, pin, size.map(|s| (key, s)));
        }),
    );
}

/// El tamaño natural de la imagen de la respuesta, o `None` si no es una imagen que el motor
/// sepa decodificar ([`bf_image::decode`]: PNG, JPEG, GIF, BMP, ICO y WebP). Un SVG cuenta
/// como cargado (sin tamaño conocido; `bf-image` no lo rasteriza). AVIF da `error`: es lo que
/// las detecciones de formato (`new Image()` con un `data:` de prueba) deben ver para servir
/// un formato que el motor pinta.
fn image_size(r: &crate::net::NetResponse) -> Option<(u32, u32)> {
    let mime = r.headers.get("content-type");
    match bf_image::decode(&r.body, mime.as_deref()) {
        Ok(img) => Some((img.width, img.height)),
        Err(bf_image::ImageError::Unsupported("image/svg+xml")) => Some((0, 0)),
        Err(_) => None,
    }
}

/// Termina la petición `id` de `n`: si sigue vigente, guarda el tamaño y dispara `load`
/// (`loaded` con la URL y el tamaño) o `error`.
fn finish(
    agent: &mut Agent,
    n: NodeRef,
    id: u64,
    pin: bf_js::host::HostHandle,
    loaded: Option<(String, (u32, u32))>,
) {
    agent.host_unpin(pin);
    let realm = WebContextHandle::current(agent).realm;
    let current = agent
        .realm_data_mut::<ImageRequests>(realm)
        .is_some_and(|r| {
            let current = r.current.get(&n) == Some(&id);
            if current {
                r.current.remove(&n);
            }
            current
        });
    if !current {
        return;
    }
    let event = match loaded {
        Some((url, size)) => {
            if size != (0, 0) {
                let mut sizes = agent
                    .take_realm_data::<super::misc::ImageSizes>(realm)
                    .unwrap_or_default();
                sizes.0.insert(url, size);
                agent.set_realm_data(realm, sizes);
            }
            "load"
        }
        None => "error",
    };
    crate::dom::events::fire_event(
        agent,
        n.doc,
        EventTarget::Node(n.node),
        event,
        EventInit::default(),
        EventKind::Event,
    );
}
