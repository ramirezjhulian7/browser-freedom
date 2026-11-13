//! Las interfaces HTML con algoritmo propio fuera de los formularios y las tablas:
//! `HTMLHyperlinkElementUtils` (§4.6.10, en `HTMLAnchorElement` y `HTMLAreaElement`),
//! `HTMLImageElement` (§4.8.4), `HTMLScriptElement` (§4.12.1), `HTMLTemplateElement`
//! (§4.12.3), `HTMLIFrameElement` (§4.8.5), `HTMLCanvasElement` (§4.12.5), `HTMLTitleElement`
//! (§4.2.2), `HTMLBaseElement` (§4.2.3), y `click()`/`focus()`/`blur()` de `HTMLElement`.

use std::collections::HashMap;

use bf_dom::events::{EventInit, EventKind, EventTarget, MouseEventData};
use bf_infra::{DomString, Url};
use bf_js::host::{HostClass, webidl};
use bf_js::string::JsString;
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use super::elements::{attr_value, html_local_name, parse_url, set_attr};
use super::scripting::fire_event;
use crate::dom::{from_dom_string, mutated, this_node, wrap_node};
use crate::{NodeRef, WebContextHandle, with_doc, with_doc_mut};

fn js_str(s: &str) -> Value {
    Value::String(JsString::from(s))
}

/// Las operaciones y atributos propios de cada interfaz de este módulo.
pub fn extend(name: &str, class: HostClass) -> HostClass {
    match name {
        "HTMLAnchorElement" | "HTMLAreaElement" => {
            let class = hyperlink_utils(class);
            if name == "HTMLAnchorElement" {
                class.attribute("text", Some(text_content), Some(set_text_content))
            } else {
                class
            }
        }
        "HTMLImageElement" => class
            .attribute("width", Some(img_width), Some(img_set_width))
            .attribute("height", Some(img_height), Some(img_set_height))
            .attribute("naturalWidth", Some(img_natural_width), None)
            .attribute("naturalHeight", Some(img_natural_height), None)
            .attribute("complete", Some(img_complete), None)
            .attribute("currentSrc", Some(img_current_src), None)
            .attribute("x", Some(zero), None)
            .attribute("y", Some(zero), None)
            .method("decode", 0, img_decode),
        "HTMLScriptElement" => class.attribute("text", Some(text_content), Some(set_text_content)),
        "HTMLTitleElement" => class.attribute("text", Some(text_content), Some(set_text_content)),
        "HTMLTemplateElement" => class.attribute("content", Some(template_content), None),
        "HTMLIFrameElement" | "HTMLFrameElement" | "HTMLObjectElement" | "HTMLEmbedElement" => {
            let class = class.method("getSVGDocument", 0, null);
            if name == "HTMLEmbedElement" {
                class
            } else {
                class
                    .attribute("contentDocument", Some(null), None)
                    .attribute("contentWindow", Some(null), None)
            }
        }
        "HTMLCanvasElement" => super::canvas::extend_canvas(class),
        "HTMLSlotElement" => crate::dom::shadow::extend_slot(class),
        "HTMLBaseElement" => class.attribute("href", Some(base_href), Some(set_base_href)),
        // WindowEventHandlers (HTML §8.1.8.2): los handlers de `body`/`frameset` son los de la
        // ventana (los handlers de E3 los reenvían a `window`).
        "HTMLBodyElement" | "HTMLFrameSetElement" => crate::dom::events::with_event_handlers(
            class,
            crate::dom::events::WINDOW_EVENT_HANDLERS,
        ),
        _ => class,
    }
}

fn null(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Null)
}

fn zero(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(0.0))
}

/// El *child text content* (`text` de `a`, `script` y `title`).
fn text_content(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let name = html_local_name(agent, n).unwrap_or_default();
    // `a.text` es el textContent (todos los descendientes); `script.text` y `title.text`,
    // el child text content.
    let s = with_doc(agent, n.doc, |d| {
        if name == "a" {
            DomString::from(d.text_content(n.node).as_str())
        } else {
            d.child_text_content(n.node)
        }
    });
    Ok(from_dom_string(&s))
}

/// "String replace all" con el valor (los setters de `text`).
fn set_text_content(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = webidl::to_dom_string(agent, &a.arg(0))?;
    let s = DomString::from_utf16(s.units());
    with_doc_mut(agent, n.doc, |d| {
        let node = (!s.is_empty()).then(|| d.create_text_node(s));
        d.replace_all(node, n.node);
    });
    mutated(agent);
    Ok(Value::Undefined)
}

// --- HTMLHyperlinkElementUtils (§4.6.10) ---

fn hyperlink_utils(class: HostClass) -> HostClass {
    class
        .attribute("href", Some(href), Some(set_href))
        .attribute("origin", Some(origin), None)
        .attribute("protocol", Some(protocol), Some(set_protocol))
        .attribute("username", Some(username), Some(set_username))
        .attribute("password", Some(password), Some(set_password))
        .attribute("host", Some(host), Some(set_host))
        .attribute("hostname", Some(hostname), Some(set_hostname))
        .attribute("port", Some(port), Some(set_port))
        .attribute("pathname", Some(pathname), Some(set_pathname))
        .attribute("search", Some(search), Some(set_search))
        .attribute("hash", Some(hash), Some(set_hash))
        .method("toString", 0, href)
}

/// "Reinitialize url": el `href` parseado contra la base del documento.
fn link_url(agent: &Agent, n: NodeRef) -> Option<Url> {
    let href = attr_value(agent, n, "href")?;
    parse_url(agent, n, &href.to_string_lossy())
}

/// "Update href": el atributo con la URL serializada.
fn update_href(agent: &mut Agent, n: NodeRef, url: &Url) -> JsResult<()> {
    set_attr(
        agent,
        n,
        "href",
        DomString::from(url.serialize(false).as_str()),
    )
}

fn href(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    match link_url(agent, n) {
        Some(u) => Ok(js_str(&u.serialize(false))),
        None => {
            Ok(attr_value(agent, n, "href").map_or_else(|| js_str(""), |v| from_dom_string(&v)))
        }
    }
}

fn set_href(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = webidl::to_dom_string(agent, &a.arg(0))?;
    set_attr(agent, n, "href", DomString::from_utf16(v.units()))?;
    Ok(Value::Undefined)
}

fn origin(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(js_str(
        &link_url(agent, n)
            .map(|u| u.origin().to_string())
            .unwrap_or_default(),
    ))
}

fn url_getter(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    null: &str,
    f: fn(&Url) -> String,
) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(js_str(
        &link_url(agent, n).map_or_else(|| null.to_owned(), |u| f(&u)),
    ))
}

/// Los setters: si hay URL, el setter de la API URL y "update href" (sin URL no hacen nada).
fn url_setter(agent: &mut Agent, a: &CallArgs<'_>, f: fn(&mut Url, &str)) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = webidl::to_dom_string(agent, &a.arg(0))?.to_string_lossy();
    if let Some(mut u) = link_url(agent, n) {
        f(&mut u, &v);
        update_href(agent, n, &u)?;
    }
    Ok(Value::Undefined)
}

fn protocol(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    url_getter(agent, a, ":", Url::protocol)
}
fn set_protocol(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    url_setter(agent, a, |u, v| u.set_protocol(v))
}
fn username(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    url_getter(agent, a, "", |u| u.username.clone())
}
fn set_username(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    url_setter(agent, a, |u, v| u.set_username(v))
}
fn password(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    url_getter(agent, a, "", |u| u.password.clone())
}
fn set_password(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    url_setter(agent, a, |u, v| u.set_password(v))
}
fn host(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    url_getter(agent, a, "", Url::host_string)
}
fn set_host(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    url_setter(agent, a, |u, v| u.set_host(v))
}
fn hostname(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    url_getter(agent, a, "", Url::hostname)
}
fn set_hostname(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    url_setter(agent, a, |u, v| u.set_hostname(v))
}
fn port(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    url_getter(agent, a, "", Url::port_string)
}
fn set_port(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    url_setter(agent, a, |u, v| u.set_port(v))
}
fn pathname(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    url_getter(agent, a, "", Url::path_string)
}
fn set_pathname(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    url_setter(agent, a, |u, v| u.set_pathname(v))
}
fn search(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    url_getter(agent, a, "", Url::search)
}
fn set_search(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    url_setter(agent, a, |u, v| u.set_search(v))
}
fn hash(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    url_getter(agent, a, "", Url::hash)
}
fn set_hash(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    url_setter(agent, a, |u, v| u.set_hash(v))
}

// --- HTMLBaseElement (§4.2.3) ---

/// `base.href`: el atributo parseado contra la *fallback base URL* del documento (la URL
/// del documento), o el atributo tal cual si no parsea.
fn base_href(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let doc_url = with_doc(agent, n.doc, |d| d.url.clone());
    let v = attr_value(agent, n, "href")
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    match Url::parse(&v, Some(&doc_url)) {
        Ok(u) => Ok(js_str(&u.serialize(false))),
        Err(_) => Ok(js_str(&v)),
    }
}

fn set_base_href(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = webidl::to_dom_string(agent, &a.arg(0))?;
    set_attr(agent, n, "href", DomString::from_utf16(v.units()))?;
    Ok(Value::Undefined)
}

// --- HTMLImageElement (§4.8.4) ---

/// Los tamaños naturales de las imágenes de la página que el navegador ya decodificó (URL →
/// (ancho, alto)); los agrega el shell con `agent.set_realm_data`.
#[derive(Debug, Default, Clone)]
pub struct ImageSizes(pub HashMap<String, (u32, u32)>);

/// E2: la fuente que eligen `srcset`, `sizes` y `<picture>` (HTML §4.8.4.3, "select an image
/// source", en `bf_image::source`) con el viewport y la densidad de la ventana, resuelta contra
/// la base del documento, y su densidad.
pub(crate) fn selected_source(agent: &mut Agent, n: NodeRef) -> Option<(Url, f64)> {
    let m = crate::cssom::window_metrics(agent);
    let ctx = bf_css::media::MediaContext {
        width: f64::from(m.viewport.width),
        height: f64::from(m.viewport.height),
        device_pixel_ratio: f64::from(m.dpr),
        ..bf_css::media::MediaContext::default()
    };
    let chosen = with_doc(agent, n.doc, |d| {
        bf_image::source::select_image_source(d, n.node, &ctx)
    })?;
    // Sin `trim`: el parser de URL ya quita los C0 y espacios de los extremos, y un NBSP es
    // parte de la URL.
    let url = parse_url(agent, n, &chosen.url)?;
    Some((url, chosen.density))
}

/// El tamaño natural corregido por densidad (§4.8.4.4): los píxeles de la fuente elegida
/// divididos por su densidad (un `2x` mide la mitad).
fn natural_size(agent: &mut Agent, n: NodeRef) -> Option<(u32, u32)> {
    let (url, density) = selected_source(agent, n)?;
    let realm = WebContextHandle::current(agent).realm;
    let (w, h) = agent
        .realm_data::<ImageSizes>(realm)?
        .0
        .get(&url.serialize(false))
        .copied()?;
    // Una densidad infinita (un `w` con `sizes` 0) da 0×0.
    if density.is_infinite() && density > 0.0 {
        return Some((0, 0));
    }
    if !(density > 0.0 && density.is_finite()) || density == 1.0 {
        return Some((w, h));
    }
    let corrected = |v: u32| (f64::from(v) / density).round() as u32;
    Some((corrected(w), corrected(h)))
}

/// `width`/`height` (§4.8.4.4): el tamaño renderizado; sin layout, el atributo si es un
/// entero no negativo válido y si no el tamaño natural (0 sin imagen).
fn img_dimension(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    name: &str,
    natural: usize,
) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let attr = attr_value(agent, n, name)
        .and_then(|s| super::reflect::parse_non_negative_integer(&s.to_string_lossy()))
        .filter(|v| *v <= 2147483647);
    let v = match attr {
        Some(v) => v as f64,
        None => {
            natural_size(agent, n).map_or(0.0, |(w, h)| f64::from(if natural == 0 { w } else { h }))
        }
    };
    Ok(Value::Number(v))
}

fn img_set_dimension(agent: &mut Agent, a: &CallArgs<'_>, name: &str) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = webidl::to_unsigned_long(agent, &a.arg(0))?;
    let v = if v <= 2147483647 { v } else { 0 };
    set_attr(agent, n, name, DomString::from(v.to_string().as_str()))?;
    Ok(Value::Undefined)
}

fn img_width(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    img_dimension(agent, a, "width", 0)
}
fn img_height(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    img_dimension(agent, a, "height", 1)
}
fn img_set_width(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    img_set_dimension(agent, a, "width")
}
fn img_set_height(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    img_set_dimension(agent, a, "height")
}

fn img_natural_width(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Number(
        natural_size(agent, n).map_or(0.0, |(w, _)| f64::from(w)),
    ))
}

fn img_natural_height(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Number(
        natural_size(agent, n).map_or(0.0, |(_, h)| f64::from(h)),
    ))
}

/// `complete` (§4.8.4.4): sin `src` (y sin `srcset`), o con `src` vacío, o con la imagen ya
/// decodificada.
fn img_complete(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    // E2: con petición, `complete` es "no está pendiente" (disponible o rota).
    if let Some(state) = super::legacy_factories::image_state(agent, n) {
        // Pendiente o aplazada (`loading=lazy`, sin pedir todavía): no está completa.
        return Ok(Value::Boolean(!matches!(
            state,
            super::legacy_factories::ImageState::Pending
                | super::legacy_factories::ImageState::Deferred
        )));
    }
    let src = attr_value(agent, n, "src");
    let srcset = attr_value(agent, n, "srcset");
    let done = match (&src, &srcset) {
        (None, None) => true,
        (Some(s), None) if s.is_empty() => true,
        _ => natural_size(agent, n).is_some(),
    };
    Ok(Value::Boolean(done))
}

/// El tamaño natural (corregido por densidad) de la imagen de `n`, si se conoce.
pub(crate) fn natural_size_of(agent: &mut Agent, n: NodeRef) -> Option<(u32, u32)> {
    natural_size(agent, n)
}

/// ¿Se conoce el tamaño natural de la imagen de `n`?
pub(crate) fn has_natural_size(agent: &mut Agent, n: NodeRef) -> bool {
    natural_size(agent, n).is_some()
}

/// `currentSrc` (§4.8.4.3): la URL de la fuente elegida ([`selected_source`]).
fn img_current_src(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = selected_source(agent, n)
        .map(|(u, _)| u.serialize(false))
        .unwrap_or_default();
    Ok(js_str(&s))
}

/// `decode()` (§4.8.4.4): espera a la petición actual (E2, `legacy_factories::img_decode`).
fn img_decode(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(super::legacy_factories::img_decode(agent, n))
}

// --- HTMLTemplateElement (§4.12.3) ---

fn template_content(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let c = with_doc(agent, n.doc, |d| {
        d.element(n.node).and_then(|e| e.template_contents)
    });
    match c {
        Some(c) => Ok(wrap_node(agent, NodeRef::new(n.doc, c))),
        None => Ok(Value::Null),
    }
}

// --- click(), focus(), blur() (§6.4.2, §6.6) ---

/// ¿Es un control de formulario deshabilitado? ("click()" no hace nada.)
fn is_disabled_control(agent: &Agent, n: NodeRef) -> bool {
    // E1: un *form-associated custom element* deshabilitado (atributo o `fieldset`).
    if super::forms::is_face(agent, n) {
        return with_doc(agent, n.doc, |d| d.is_disabled(n.node));
    }
    matches!(
        html_local_name(agent, n).as_deref(),
        Some("button" | "input" | "select" | "textarea" | "optgroup" | "option" | "fieldset")
    ) && attr_value(agent, n, "disabled").is_some()
}

/// "click()" (§6.4.2): dispara un `click` sintético (PointerEvent con `bubbles`,
/// `cancelable` y `composed`), salvo en un control deshabilitado o durante otro click.
pub fn click(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    click_element(agent, n);
    Ok(Value::Undefined)
}

/// Los pasos de "click()" sobre `n` (también los usa el `label` que reenvía su click).
pub fn click_element(agent: &mut Agent, n: NodeRef) {
    if is_disabled_control(agent, n) {
        return;
    }
    let realm = WebContextHandle::current(agent).realm;
    let in_progress = agent
        .realm_data::<ClickState>(realm)
        .is_some_and(|s| s.0.contains(&n));
    if in_progress {
        return;
    }
    let mut s = agent
        .take_realm_data::<ClickState>(realm)
        .unwrap_or_default();
    s.0.push(n);
    agent.set_realm_data(realm, s);
    let init = EventInit {
        bubbles: true,
        cancelable: true,
        composed: true,
    };
    // La *activation behavior* (y la *legacy-pre-activation* de checkbox y radio) la hace
    // el dispatch de `bf-dom` (DOM §2.9).
    fire_event(
        agent,
        n.doc,
        EventTarget::Node(n.node),
        "click",
        init,
        EventKind::Mouse(MouseEventData::default()),
    );
    // Las acciones que el dispatch dejó en cola (enlaces, envíos, label, summary).
    super::activation::run_activations(agent, n.doc);
    if let Some(s) = agent.realm_data_mut::<ClickState>(realm) {
        s.0.retain(|x| *x != n);
    }
}

/// Los elementos con el *click in progress flag*.
#[derive(Debug, Default)]
struct ClickState(Vec<NodeRef>);

/// El elemento con el foco del documento (§6.6.2 "focused area of the document").
#[derive(Debug, Default, Clone, Copy)]
pub struct Focus(pub Option<NodeRef>);

/// El elemento enfocado de la ventana actual (para `document.activeElement`).
pub fn focused_element(agent: &Agent) -> Option<NodeRef> {
    let realm = WebContextHandle::current(agent).realm;
    agent.realm_data::<Focus>(realm).and_then(|f| f.0)
}

/// ¿Puede tener el foco? ("focusable area": los focusables por defecto y los que tienen
/// `tabindex`, conectados y no deshabilitados.)
fn is_focusable(agent: &Agent, n: NodeRef) -> bool {
    if is_disabled_control(agent, n) || !with_doc(agent, n.doc, |d| d.is_connected(n.node)) {
        return false;
    }
    if attr_value(agent, n, "tabindex")
        .and_then(|s| super::reflect::parse_integer(&s.to_string_lossy()))
        .is_some()
    {
        return true;
    }
    match html_local_name(agent, n).as_deref() {
        Some("a" | "area") => attr_value(agent, n, "href").is_some(),
        Some("button" | "select" | "textarea" | "iframe" | "summary") => true,
        Some("input") => attr_value(agent, n, "type")
            .is_none_or(|t| !t.to_string_lossy().eq_ignore_ascii_case("hidden")),
        _ => false,
    }
}

fn focus_event(agent: &mut Agent, n: NodeRef, type_: &str, related: Option<NodeRef>) {
    let bubbles = type_ == "focusin" || type_ == "focusout";
    let init = EventInit {
        bubbles,
        cancelable: false,
        composed: true,
    };
    // E1: el foco puede pasar entre documentos (uno de `DOMParser` conectado); un
    // `relatedTarget` de otro documento no se resuelve en la arena de `n` (DOM §2.5 lo
    // retargetearía igual a sí mismo, pero aquí solo cabe su `NodeId`).
    let related_target = related
        .filter(|r| r.doc == n.doc)
        .map(|r| EventTarget::Node(r.node));
    fire_event(
        agent,
        n.doc,
        EventTarget::Node(n.node),
        type_,
        init,
        EventKind::Focus { related_target },
    );
}

/// "Focusing steps" (§6.6.4), sin el *focus chain* de documentos anidados.
pub fn focus(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let mut n = this_node(agent, a)?;
    // E1: un shadow host con `delegatesFocus` pasa el foco al primer elemento enfocable de
    // su shadow tree (HTML §6.6.4 "focus delegate"); si el foco ya está dentro, se queda.
    if delegates_focus(agent, n) {
        let realm = WebContextHandle::current(agent).realm;
        let current = agent.realm_data::<Focus>(realm).and_then(|f| f.0);
        if current.is_some_and(|c| {
            c.doc == n.doc
                && with_doc(agent, n.doc, |d| {
                    d.is_shadow_including_inclusive_ancestor(n.node, c.node)
                })
        }) {
            return Ok(Value::Undefined);
        }
        match focus_delegate(agent, n) {
            Some(d) => n = d,
            None => return Ok(Value::Undefined),
        }
    }
    if !is_focusable(agent, n) {
        return Ok(Value::Undefined);
    }
    let realm = WebContextHandle::current(agent).realm;
    let old = agent.realm_data::<Focus>(realm).and_then(|f| f.0);
    if old == Some(n) {
        return Ok(Value::Undefined);
    }
    if let Some(o) = old {
        focus_event(agent, o, "blur", Some(n));
        focus_event(agent, o, "focusout", Some(n));
    }
    agent.set_realm_data(realm, Focus(Some(n)));
    focus_event(agent, n, "focus", old);
    focus_event(agent, n, "focusin", old);
    Ok(Value::Undefined)
}

/// E1: ¿Es un shadow host con `delegatesFocus`?
fn delegates_focus(agent: &Agent, n: NodeRef) -> bool {
    with_doc(agent, n.doc, |d| {
        d.shadow_root(n.node)
            .and_then(|r| d.shadow_root_data(r))
            .is_some_and(|s| s.delegates_focus)
    })
}

/// E1: el *focus delegate* de un host: el primer enfocable de su shadow tree en orden
/// shadow-including (los hosts anidados con `delegatesFocus` delegan a su vez).
fn focus_delegate(agent: &Agent, host: NodeRef) -> Option<NodeRef> {
    let root = with_doc(agent, host.doc, |d| d.shadow_root(host.node))?;
    let nodes = with_doc(agent, host.doc, |d| {
        d.shadow_including_inclusive_descendants(root)
    });
    for n in nodes.into_iter().skip(1) {
        let r = NodeRef::new(host.doc, n);
        if with_doc(agent, host.doc, |d| d.element(n).is_none()) {
            continue;
        }
        if delegates_focus(agent, r) {
            if let Some(d) = focus_delegate(agent, r) {
                return Some(d);
            }
            continue;
        }
        if is_focusable(agent, r) {
            return Some(r);
        }
    }
    None
}

/// "Unfocusing steps" (§6.6.4).
pub fn blur(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let mut n = this_node(agent, a)?;
    let realm = WebContextHandle::current(agent).realm;
    // E1: paso 1: un host con `delegatesFocus` cuyo shadow tree tiene el foco pasa a ser el
    // área enfocada.
    let current = agent.realm_data::<Focus>(realm).and_then(|f| f.0);
    if let Some(c) = current
        && delegates_focus(agent, n)
        && c.doc == n.doc
        && with_doc(agent, n.doc, |d| {
            d.is_shadow_including_inclusive_ancestor(n.node, c.node)
        })
    {
        n = c;
    }
    if agent.realm_data::<Focus>(realm).and_then(|f| f.0) != Some(n) {
        return Ok(Value::Undefined);
    }
    agent.set_realm_data(realm, Focus(None));
    focus_event(agent, n, "blur", None);
    focus_event(agent, n, "focusout", None);
    Ok(Value::Undefined)
}

/// Saca el foco de `n` si lo tenía (p. ej. al quitarlo del documento).
pub fn drop_focus(agent: &mut Agent, n: NodeRef) {
    let realm = WebContextHandle::current(agent).realm;
    if agent.realm_data::<Focus>(realm).and_then(|f| f.0) == Some(n) {
        agent.set_realm_data(realm, Focus(None));
    }
}
