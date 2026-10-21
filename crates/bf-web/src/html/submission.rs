//! Envío de formularios (HTML §4.10.21): el algoritmo "submit" con la validación
//! interactiva, `submit` (`SubmitEvent`) y `formdata` (`FormDataEvent`), el "entry list" de
//! los controles, las codificaciones y la navegación.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use bf_dom::events::{Event, EventInit, EventKind, EventTarget};
use bf_infra::{Namespace, Url};
use bf_js::host::{HostClass, HostClassId};
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use super::elements::{attr_value, html_local_name, parse_url};
use super::forms::{self, form_owner_of, input_type};
use crate::dom::events::{
    EventIface, construct_event_vec, create_event_object, ctor_type, dict_get, dispatch_js_event,
    event_init, slot,
};
use crate::net::blob::BlobData;
use crate::net::formdata::{self, Entry, FormDataPayload};
use crate::{NodeRef, WebContextHandle, with_doc};

// --- SubmitEvent, FormDataEvent, ToggleEvent ---

/// Las clases de evento de esta área.
#[derive(Debug, Clone, Copy)]
pub struct FormEventClasses {
    pub submit: HostClassId,
    pub form_data: HostClassId,
    pub toggle: HostClassId,
}

pub fn register(agent: &mut Agent) {
    if agent.embedder_data::<FormEventClasses>().is_some() {
        return;
    }
    let event = crate::dom::classes(agent).event;
    let submit = agent.register_host_class(
        HostClass::new("SubmitEvent")
            .parent(event)
            .constructor(submit_event_ctor, 1)
            .attribute("submitter", Some(slot2), None),
    );
    let form_data = agent.register_host_class(
        HostClass::new("FormDataEvent")
            .parent(event)
            .constructor(form_data_event_ctor, 2)
            .attribute("formData", Some(slot2), None),
    );
    let toggle = agent.register_host_class(
        HostClass::new("ToggleEvent")
            .parent(event)
            .constructor(toggle_event_ctor, 1)
            .attribute("newState", Some(slot2), None)
            .attribute("oldState", Some(slot3), None),
    );
    agent.set_embedder_data(FormEventClasses {
        submit,
        form_data,
        toggle,
    });
}

pub fn event_classes(agent: &Agent) -> FormEventClasses {
    *agent
        .embedder_data::<FormEventClasses>()
        .expect("FormEventClasses")
}

fn slot2(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    slot(agent, a, 2)
}

fn slot3(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    slot(agent, a, 3)
}

/// `new SubmitEvent(type, {submitter})`: `submitter` es un `HTMLElement` o `null`.
fn submit_event_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let type_ = ctor_type(agent, a, "SubmitEvent")?;
    let dict = a.arg(1);
    let init = event_init(agent, &dict)?;
    let submitter = match dict_get(agent, &dict, "submitter")? {
        Value::Undefined | Value::Null => Value::Null,
        v if super::elements::html_element_arg(agent, &v).is_some() => v,
        _ => {
            return agent.type_error(
                "Failed to construct 'SubmitEvent': Failed to read the 'submitter' property \
                 from 'SubmitEventInit': Failed to convert value to 'HTMLElement'.",
            );
        }
    };
    let class = event_classes(agent).submit;
    construct_event_vec(
        agent,
        a,
        EventIface::Class(class),
        Event::new(type_, init),
        vec![Value::Null, Value::Undefined, submitter],
    )
}

/// `new FormDataEvent(type, {formData})`: `formData` es obligatorio.
fn form_data_event_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let type_ = ctor_type(agent, a, "FormDataEvent")?;
    let dict = a.arg(1);
    if !matches!(dict, Value::Object(_)) {
        return agent.type_error(
            "Failed to construct 'FormDataEvent': 2 arguments required, but only 1 present.",
        );
    }
    let init = event_init(agent, &dict)?;
    let fd = dict_get(agent, &dict, "formData")?;
    if fd.is_undefined() {
        return agent.type_error(
            "Failed to construct 'FormDataEvent': Failed to read the 'formData' property from \
             'FormDataEventInit': Required member is undefined.",
        );
    }
    if agent.host_unwrap::<FormDataPayload>(&fd).is_none() {
        return agent.type_error(
            "Failed to construct 'FormDataEvent': Failed to read the 'formData' property from \
             'FormDataEventInit': Failed to convert value to 'FormData'.",
        );
    }
    let class = event_classes(agent).form_data;
    construct_event_vec(
        agent,
        a,
        EventIface::Class(class),
        Event::new(type_, init),
        vec![Value::Null, Value::Undefined, fd],
    )
}

/// `new ToggleEvent(type, {newState, oldState})`.
fn toggle_event_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let type_ = ctor_type(agent, a, "ToggleEvent")?;
    let dict = a.arg(1);
    let init = event_init(agent, &dict)?;
    let mut states = Vec::with_capacity(2);
    for name in ["newState", "oldState"] {
        let v = dict_get(agent, &dict, name)?;
        states.push(if v.is_undefined() {
            Value::str("")
        } else {
            let s = crate::dom::to_dom_string(agent, &v)?;
            crate::dom::from_dom_string(&s)
        });
    }
    let class = event_classes(agent).toggle;
    let mut slots = vec![Value::Null, Value::Undefined];
    slots.extend(states);
    construct_event_vec(
        agent,
        a,
        EventIface::Class(class),
        Event::new(type_, init),
        slots,
    )
}

/// "Fire an event" (DOM §2.10) con una interfaz de esta área: `isTrusted` y los miembros
/// propios en los slots desde el 2. Devuelve `false` si se canceló.
pub fn fire_class_event(
    agent: &mut Agent,
    target: NodeRef,
    class: HostClassId,
    type_: &str,
    init: EventInit,
    members: Vec<Value>,
) -> bool {
    let mut ev = Event::with_kind(type_, init, EventKind::Event);
    ev.is_trusted = true;
    let o = create_event_object(
        agent,
        EventIface::Class(class),
        Rc::new(RefCell::new(ev)),
        target.doc,
    );
    if let Some(s) = agent.host_slots_mut(o) {
        s.extend(members);
    }
    let pin = agent.host_pin(Value::Object(o));
    let r = dispatch_js_event(agent, target.doc, EventTarget::Node(target.node), o, false);
    agent.host_unpin(pin);
    matches!(r, Ok(Ok(true)))
}

/// "Fire an event" simple (`Event`, `isTrusted`) en `target`.
pub fn fire_simple(agent: &mut Agent, target: NodeRef, type_: &str, init: EventInit) -> bool {
    let class = crate::dom::classes(agent).event;
    fire_class_event(agent, target, class, type_, init, Vec::new())
}

// --- Los flags de los formularios ---

/// "Constructing entry list" y "firing submission events" de cada `form` (§4.10.18).
#[derive(Debug, Default)]
struct FormFlags {
    constructing: HashSet<NodeRef>,
    firing: HashSet<NodeRef>,
}

fn flags(agent: &mut Agent) -> &mut FormFlags {
    let realm = WebContextHandle::current(agent).realm;
    if agent.realm_data::<FormFlags>(realm).is_none() {
        agent.set_realm_data(realm, FormFlags::default());
    }
    agent.realm_data_mut::<FormFlags>(realm).expect("FormFlags")
}

// --- Entry list (§4.10.21.4) ---

/// ¿Es un *submittable element* (§4.10.2)?
fn is_submittable(name: &str) -> bool {
    matches!(name, "button" | "input" | "select" | "textarea")
}

/// Los controles *submittable* cuyo form owner es `form`, en orden de árbol.
fn submittable_controls(agent: &Agent, form: NodeRef) -> Vec<NodeRef> {
    let root = with_doc(agent, form.doc, |d| {
        *d.inclusive_ancestors(form.node)
            .last()
            .unwrap_or(&form.node)
    });
    let all = with_doc(agent, form.doc, |d| d.descendants(root));
    all.into_iter()
        .map(|c| NodeRef::new(form.doc, c))
        .filter(|c| {
            html_local_name(agent, *c).is_some_and(|n| is_submittable(&n))
                && form_owner_of(agent, *c) == Some(form)
        })
        .collect()
}

/// ¿Es un *submit button* (§4.10.2)?
pub fn is_submit_button(agent: &Agent, n: NodeRef) -> bool {
    match html_local_name(agent, n).as_deref() {
        Some("button") => !matches!(
            attr_value(agent, n, "type")
                .map(|t| t.to_string_lossy().to_ascii_lowercase())
                .as_deref(),
            Some("reset" | "button")
        ),
        Some("input") => matches!(input_type(agent, n).as_str(), "submit" | "image"),
        _ => false,
    }
}

fn attr_string(agent: &Agent, n: NodeRef, name: &str) -> Option<String> {
    attr_value(agent, n, name).map(|s| s.to_string_lossy().into_owned())
}

/// "Constructing the entry list" (§4.10.21.4) con `formdata`; `None` si ya se estaba
/// construyendo para `form`.
pub fn construct_entry_list(
    agent: &mut Agent,
    form: NodeRef,
    submitter: Option<NodeRef>,
) -> Option<Vec<(String, Entry)>> {
    // 1-2.
    if !flags(agent).constructing.insert(form) {
        return None;
    }
    // 3-5.
    let mut list = Vec::new();
    for field in submittable_controls(agent, form) {
        append_entries(agent, field, submitter, &mut list);
    }
    // 7-8: `formdata` con un FormData sobre la lista; los listeners pueden cambiarla.
    let fd = formdata::create(agent, list);
    let class = event_classes(agent).form_data;
    let init = EventInit {
        bubbles: true,
        ..EventInit::default()
    };
    let pin = agent.host_pin(fd.clone());
    fire_class_event(agent, form, class, "formdata", init, vec![fd.clone()]);
    agent.host_unpin(pin);
    // 9-10.
    flags(agent).constructing.remove(&form);
    Some(formdata::entries_of(agent, &fd).unwrap_or_default())
}

/// Las entradas de un control (paso 5 de "constructing the entry list").
fn append_entries(
    agent: &Agent,
    field: NodeRef,
    submitter: Option<NodeRef>,
    list: &mut Vec<(String, Entry)>,
) {
    let Some(local) = html_local_name(agent, field) else {
        return;
    };
    // 5.1: dentro de un `datalist`, deshabilitado, un botón que no es el submitter, un
    // checkbox o radio sin marcar.
    let skip = with_doc(agent, field.doc, |d| {
        d.is_disabled(field.node)
            || d.inclusive_ancestors(field.node).iter().any(|a| {
                d.element(*a)
                    .is_some_and(|e| e.ns == Namespace::Html && e.name == "datalist")
            })
    });
    if skip {
        return;
    }
    let ty = if local == "input" {
        input_type(agent, field)
    } else {
        String::new()
    };
    let is_button = local == "button"
        || (local == "input" && matches!(ty.as_str(), "submit" | "image" | "reset" | "button"));
    if is_button && submitter != Some(field) {
        return;
    }
    if local == "input"
        && matches!(ty.as_str(), "checkbox" | "radio")
        && !with_doc(agent, field.doc, |d| d.checkedness(field.node))
    {
        return;
    }
    // 5.2: el botón imagen, con las coordenadas.
    if local == "input" && ty == "image" {
        let name = attr_string(agent, field, "name").unwrap_or_default();
        let prefix = if name.is_empty() {
            String::new()
        } else {
            format!("{name}.")
        };
        list.push((format!("{prefix}x"), Entry::String("0".into())));
        list.push((format!("{prefix}y"), Entry::String("0".into())));
        return;
    }
    // 5.4-5.5: sin nombre no se envía.
    let Some(name) = attr_string(agent, field, "name").filter(|n| !n.is_empty()) else {
        return;
    };
    match local.as_str() {
        "select" => {
            for o in forms::select_options(agent, field) {
                let ok = with_doc(agent, o.doc, |d| {
                    d.option_selectedness(o.node) && !d.option_disabled(o.node)
                });
                if ok {
                    list.push((
                        name.clone(),
                        Entry::String(forms::option_value_str(agent, o)),
                    ));
                }
            }
        }
        "input" if matches!(ty.as_str(), "checkbox" | "radio") => {
            let v = attr_string(agent, field, "value").unwrap_or_else(|| "on".into());
            list.push((name, Entry::String(v)));
        }
        "input" if ty == "file" => {
            // Sin archivos elegidos: un `File` vacío sin nombre.
            list.push((
                name,
                Entry::File(BlobData {
                    bytes: Arc::new(Vec::new()),
                    mime: "application/octet-stream".into(),
                    file: Some((String::new(), 0.0)),
                }),
            ));
        }
        "input" if ty == "hidden" && name.eq_ignore_ascii_case("_charset_") => {
            list.push((name, Entry::String("UTF-8".into())));
        }
        "input" => list.push((
            name,
            Entry::String(forms::input_current_value(agent, field)),
        )),
        // El valor con la "textarea wrapping transformation" (§4.10.11): saltos en CRLF.
        "textarea" => list.push((
            name,
            Entry::String(crlf(&forms::textarea_value_of(agent, field))),
        )),
        _ => {
            let v = attr_string(agent, field, "value").unwrap_or_default();
            list.push((name, Entry::String(v)));
        }
    }
    // 5.11: `dirname`.
    let dir_applies = local == "textarea"
        || (local == "input"
            && matches!(
                ty.as_str(),
                "hidden"
                    | "text"
                    | "search"
                    | "url"
                    | "tel"
                    | "email"
                    | "submit"
                    | "reset"
                    | "button"
            ));
    if dir_applies
        && let Some(dirname) = attr_string(agent, field, "dirname").filter(|d| !d.is_empty())
    {
        list.push((dirname, Entry::String("ltr".into())));
    }
}

// --- Validación interactiva (§4.10.20.2) ---

/// "Interactively validate the constraints": dispara `invalid` en los controles inválidos;
/// `true` si son todos válidos.
fn interactively_validate(agent: &mut Agent, form: NodeRef) -> bool {
    let mut valid = true;
    for c in submittable_controls(agent, form) {
        if forms::will_validate_node(agent, c) && !forms::satisfies_constraints(agent, c) {
            valid = false;
            let init = EventInit {
                cancelable: true,
                ..EventInit::default()
            };
            fire_simple(agent, c, "invalid", init);
        }
    }
    valid
}

// --- Submit (§4.10.21.3) ---

/// "A form cannot navigate": desconectado o fuera del documento de la ventana.
fn cannot_navigate(agent: &Agent, form: NodeRef) -> bool {
    form.doc != 0 || !with_doc(agent, form.doc, |d| d.is_connected(form.node))
}

/// El atributo `name` del submitter (`formaction`…) si es un submit button con él, o el del
/// formulario.
fn override_attr(agent: &Agent, form: NodeRef, submitter: NodeRef, name: &str) -> Option<String> {
    if submitter != form
        && is_submit_button(agent, submitter)
        && let Some(v) = attr_string(agent, submitter, &format!("form{name}"))
    {
        return Some(v);
    }
    attr_string(agent, form, name)
}

/// "Submit" `form` desde `submitter` (el propio `form` si no hay botón).
pub fn submit(agent: &mut Agent, form: NodeRef, submitter: NodeRef, from_submit_method: bool) {
    // 1-2.
    if cannot_navigate(agent, form) || flags(agent).constructing.contains(&form) {
        return;
    }
    // 6.
    if !from_submit_method {
        // 6.1-6.2.
        if !flags(agent).firing.insert(form) {
            return;
        }
        // 6.3: el *no-validate state*.
        let no_validate = override_attr(agent, form, submitter, "novalidate").is_some();
        if !no_validate && !interactively_validate(agent, form) {
            flags(agent).firing.remove(&form);
            return;
        }
        // 6.4-6.5.
        let submitter_button = if submitter == form {
            Value::Null
        } else {
            crate::dom::wrap_node(agent, submitter)
        };
        let class = event_classes(agent).submit;
        let init = EventInit {
            bubbles: true,
            cancelable: true,
            composed: false,
        };
        let should_continue =
            fire_class_event(agent, form, class, "submit", init, vec![submitter_button]);
        // 6.6-6.8.
        flags(agent).firing.remove(&form);
        if !should_continue || cannot_navigate(agent, form) {
            return;
        }
    }
    // 8-10.
    let submitter_opt = (submitter != form).then_some(submitter);
    let Some(entries) = construct_entry_list(agent, form, submitter_opt) else {
        return;
    };
    if cannot_navigate(agent, form) {
        return;
    }
    // 11-14: action, method, enctype, target.
    let action = override_attr(agent, form, submitter, "action").unwrap_or_default();
    let base_node = if submitter == form { form } else { submitter };
    let action = if action.trim().is_empty() {
        with_doc(agent, form.doc, |d| d.url.clone())
    } else {
        match parse_url(agent, base_node, action.trim()) {
            Some(u) => u,
            None => return,
        }
    };
    let method = override_attr(agent, form, submitter, "method")
        .map(|m| m.to_ascii_lowercase())
        .unwrap_or_default();
    if method == "dialog" {
        return;
    }
    let post = method == "post";
    let enctype = override_attr(agent, form, submitter, "enctype")
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    let target = override_attr(agent, form, submitter, "target")
        .or_else(|| base_target(agent, form))
        .unwrap_or_default();
    // 20-23: según el esquema y el método.
    let mut url = action;
    let body = match url.scheme.as_str() {
        "javascript" | "mailto" => None,
        _ if !post => {
            url.query = Some(urlencoded(&entries));
            None
        }
        _ => Some(encode_body(&enctype, &entries)),
    };
    navigate(agent, url, body, &target);
}

/// El `target` del primer `base` con atributo `target` (§4.2.3).
pub fn base_target(agent: &Agent, n: NodeRef) -> Option<String> {
    with_doc(agent, n.doc, |d| {
        d.descendants(d.root()).into_iter().find_map(|c| {
            d.element(c)
                .filter(|e| e.is_html("base"))
                .and_then(|e| e.attr("target").map(str::to_owned))
        })
    })
}

/// "Convert to a list of name-value pairs" (§4.10.21.8): los archivos por su nombre y los
/// saltos de línea como CRLF.
fn name_value_pairs(entries: &[(String, Entry)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(n, v)| {
            let v = match v {
                Entry::String(s) => crlf(s),
                Entry::File(f) => f
                    .file
                    .as_ref()
                    .map(|(name, _)| name.clone())
                    .unwrap_or_default(),
            };
            (crlf(n), v)
        })
        .collect()
}

fn crlf(s: &str) -> String {
    s.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r\n")
}

fn urlencoded(entries: &[(String, Entry)]) -> String {
    bf_url::form::serialize(&name_value_pairs(entries))
}

/// El cuerpo de un envío POST y su `Content-Type`.
fn encode_body(enctype: &str, entries: &[(String, Entry)]) -> (Vec<u8>, String) {
    match enctype {
        "multipart/form-data" => {
            let boundary = format!("----freedomFormBoundary{:016x}", boundary_seed());
            let normalized: Vec<(String, Entry)> = entries
                .iter()
                .map(|(n, v)| {
                    let v = match v {
                        Entry::String(s) => Entry::String(crlf(s)),
                        f => f.clone(),
                    };
                    (crlf(n), v)
                })
                .collect();
            (
                formdata::multipart(&normalized, &boundary),
                format!("multipart/form-data; boundary={boundary}"),
            )
        }
        "text/plain" => {
            let mut out = String::new();
            for (n, v) in name_value_pairs(entries) {
                out.push_str(&n);
                out.push('=');
                out.push_str(&v);
                out.push_str("\r\n");
            }
            (out.into_bytes(), "text/plain".into())
        }
        _ => (
            urlencoded(entries).into_bytes(),
            "application/x-www-form-urlencoded".into(),
        ),
    }
}

thread_local! {
    static BOUNDARY: Cell<u64> = const { Cell::new(0x5eed_f00d_cafe_0001) };
}

fn boundary_seed() -> u64 {
    BOUNDARY.with(|b| {
        // xorshift64: distinto en cada envío.
        let mut x = b.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        b.set(x);
        x
    })
}

/// Navega la ventana (o abre otra si `target` no es la propia) a `url`, con `body` si es
/// un POST.
pub fn navigate(agent: &mut Agent, url: Url, body: Option<(Vec<u8>, String)>, target: &str) {
    let target = target.trim();
    let own = target.is_empty()
        || ["_self", "_parent", "_top"]
            .iter()
            .any(|t| target.eq_ignore_ascii_case(t));
    if !own {
        let href = url.href();
        WebContextHandle::current(agent)
            .get_mut(agent)
            .hooks
            .open(&href, target, "");
        return;
    }
    if url.scheme == "javascript" {
        queue_javascript_url(agent, &url);
        return;
    }
    match body {
        Some((bytes, content_type)) => {
            crate::bom::state_mut(agent)
                .hooks
                .navigate_post(&url, bytes, &content_type)
        }
        None => crate::bom::location::navigate_url(agent, url, false),
    }
}

/// "Navigate to a javascript: URL" (HTML §7.4.2.3.4): el script (percent-decoded) corre en
/// una tarea.
fn queue_javascript_url(agent: &mut Agent, url: &Url) {
    let href = url.href();
    let source = percent_decode(href.strip_prefix("javascript:").unwrap_or(""));
    let realm = WebContextHandle::current(agent).realm;
    agent.queue_task(
        bf_js::host::TaskSource::DomManipulation,
        Box::new(move |agent: &mut Agent| {
            crate::bom::script::run_script(agent, realm, &source, &href, false);
        }),
    );
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}
