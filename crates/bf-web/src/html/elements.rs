//! Las clases `HTMLElement` (HTML §3.2.8) y `HTML*Element` (§4, §15.3) y la reflexión de sus
//! atributos de contenido (§2.6.1).
//!
//! Las clases se registran una vez por agente ([`HtmlClasses`]); en cada realm, los
//! atributos reflejados se definen como accessors del prototipo de su interfaz (getter
//! `get <nombre>`, setter `set <nombre>`, enumerables y configurables como cualquier
//! atributo WebIDL). Cada accessor lleva en un slot el índice de su atributo en
//! [`HtmlClasses::reflected`]; el brand check es el de WebIDL: `this` debe implementar la
//! interfaz.

use std::collections::HashMap;

use bf_dom::{DomException, Namespace, NodeData};
use bf_infra::{DomString, Url};
use bf_js::host::{HostClass, HostClassId, webidl};
use bf_js::object::PropertyDescriptor;
use bf_js::string::JsString;
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, RealmId};

use super::reflect::{self, Got, Input, Reflect, SetAction};
use super::table::{Attr, HTML_ELEMENT_ATTRS, INTERFACES, Kind, interface_for};
use crate::dom::{self, from_dom_string, mutated, this_node, throw_dom, unwrap_node};
use crate::{NodeRef, WebContextHandle, with_doc, with_doc_mut};

/// Los ids de las clases HTML del agente.
#[derive(Debug, Clone)]
pub struct HtmlClasses {
    pub html_element: HostClassId,
    pub by_name: HashMap<&'static str, HostClassId>,
    /// Los atributos reflejados: (interfaz, atributo). El índice va en el slot del accessor.
    pub reflected: Vec<(HostClassId, &'static Attr)>,
}

/// Los ids de las clases HTML (`None` antes de registrarlas).
pub fn html_classes(agent: &Agent) -> Option<&HtmlClasses> {
    agent.embedder_data::<HtmlClasses>()
}

/// La clase de una interfaz HTML por nombre.
pub fn class_named(agent: &Agent, name: &str) -> Option<HostClassId> {
    html_classes(agent).and_then(|c| c.by_name.get(name).copied())
}

/// Lo propio de cada interfaz además de la reflexión (los métodos y atributos con
/// algoritmo propio los agregan los módulos de cada familia).
fn extend(name: &str, class: HostClass) -> HostClass {
    let class = super::forms::extend(name, class);
    let class = super::tables::extend(name, class);
    super::misc::extend(name, class)
}

fn html_element_class(element: HostClassId) -> HostClass {
    // HTMLElement incluye GlobalEventHandlers (HTML §8.1.8.2): los manejadores del DOM.
    let class = crate::dom::events::with_event_handlers(
        HostClass::new("HTMLElement"),
        crate::dom::events::GLOBAL_EVENT_HANDLERS,
    );
    class
        .parent(element)
        .attribute("tabIndex", Some(get_tab_index), Some(set_tab_index))
        .attribute("innerText", Some(get_inner_text), Some(set_inner_text))
        .attribute("outerText", Some(get_inner_text), Some(set_outer_text))
        .attribute("accessKeyLabel", Some(access_key_label), None)
        .attribute("draggable", Some(get_draggable), Some(set_draggable))
        .attribute("spellcheck", Some(get_spellcheck), Some(set_spellcheck))
        .attribute("translate", Some(get_translate), Some(set_translate))
        .attribute(
            "contentEditable",
            Some(get_content_editable),
            Some(set_content_editable),
        )
        .attribute("isContentEditable", Some(is_content_editable), None)
        .method("click", 0, super::misc::click)
        .method("focus", 0, super::misc::focus)
        .method("blur", 0, super::misc::blur)
}

/// Registra las clases (la primera vez) y define la reflexión en `realm`.
pub fn register(agent: &mut Agent, realm: RealmId, _ctx: &WebContextHandle) {
    if html_classes(agent).is_none() {
        let element = dom::classes(agent).element;
        let html_element = agent.register_host_class(html_element_class(element));
        let mut by_name: HashMap<&'static str, HostClassId> = HashMap::new();
        by_name.insert("HTMLElement", html_element);
        let mut reflected: Vec<(HostClassId, &'static Attr)> = HTML_ELEMENT_ATTRS
            .iter()
            .map(|a| (html_element, a))
            .collect();
        for iface in INTERFACES {
            let parent = by_name[iface.parent];
            let class = agent.register_host_class(extend(
                iface.name,
                HostClass::new(iface.name).parent(parent),
            ));
            by_name.insert(iface.name, class);
            reflected.extend(iface.attrs.iter().map(|a| (class, a)));
        }
        // La clase de cada elemento HTML por nombre local (los demás: HTMLUnknownElement, o
        // HTMLElement si el nombre es de custom element; eso lo decide `interface_for`).
        for tag in KNOWN_TAGS {
            let class = by_name[interface_for(tag)];
            dom::set_element_class(agent, Namespace::Html, tag, class);
        }
        dom::set_html_default_class(agent, by_name["HTMLUnknownElement"]);
        agent.set_embedder_data(HtmlClasses {
            html_element,
            by_name,
            reflected,
        });
    }
    define_reflection(agent, realm);
    define_document_colors(agent, realm);
}

/// Los atributos obsoletos de `Document` que reflejan los de `body` (HTML §15.3.1
/// "Other elements, attributes and APIs": `fgColor`, `linkColor`, `vlinkColor`,
/// `alinkColor`, `bgColor`, `[LegacyNullToEmptyString]`).
const DOCUMENT_COLORS: [(&str, &str); 5] = [
    ("fgColor", "text"),
    ("linkColor", "link"),
    ("vlinkColor", "vlink"),
    ("alinkColor", "alink"),
    ("bgColor", "bgcolor"),
];

fn define_document_colors(agent: &mut Agent, realm: RealmId) {
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let class = dom::classes(agent).document;
    let proto = agent.host_class_objects(realm, class).prototype;
    for (i, (idl, _)) in DOCUMENT_COLORS.iter().enumerate() {
        let slot = vec![Value::Number(i as f64)];
        let g =
            agent.create_builtin_with_slots(document_color, 0, &format!("get {idl}"), slot.clone());
        let s = agent.create_builtin_with_slots(set_document_color, 1, &format!("set {idl}"), slot);
        let _ = agent.define_own_property(
            proto,
            PropertyKey::from(*idl),
            PropertyDescriptor::accessor(Some(g), Some(s), true, true),
        );
    }
    agent.current_realm = saved;
}

/// El atributo de `body` del accessor en curso y el `body` del documento (si es un
/// elemento `body`).
fn document_color_target(
    agent: &mut Agent,
    a: &CallArgs<'_>,
) -> JsResult<(&'static str, Option<NodeRef>)> {
    let class = dom::classes(agent).document;
    if !agent.host_is_instance(&a.this, class) {
        return agent.type_error("Illegal invocation");
    }
    let i = match bf_js::builtins::slots(agent, a.callee).first() {
        Some(Value::Number(n)) => *n as usize,
        _ => 0,
    };
    let attr = DOCUMENT_COLORS[i.min(DOCUMENT_COLORS.len() - 1)].1;
    let doc = this_node(agent, a)?;
    let body = with_doc(agent, doc.doc, |d| {
        d.body()
            .filter(|b| d.element(*b).is_some_and(|e| e.is_html("body")))
    });
    Ok((attr, body.map(|b| NodeRef::new(doc.doc, b))))
}

fn document_color(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (attr, body) = document_color_target(agent, a)?;
    let v = body.and_then(|b| attr_value(agent, b, attr));
    Ok(v.map_or_else(|| js_str(""), |v| from_dom_string(&v)))
}

fn set_document_color(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (attr, body) = document_color_target(agent, a)?;
    let v = webidl::to_dom_string_null_empty(agent, &a.arg(0))?;
    if let Some(b) = body {
        set_attr(agent, b, attr, DomString::from_utf16(v.units()))?;
    }
    Ok(Value::Undefined)
}

/// Los nombres locales con interfaz propia o `HTMLElement` (ver [`interface_for`]).
const KNOWN_TAGS: &[&str] = &[
    "a",
    "abbr",
    "acronym",
    "address",
    "area",
    "article",
    "aside",
    "audio",
    "b",
    "base",
    "basefont",
    "bdi",
    "bdo",
    "big",
    "blockquote",
    "body",
    "br",
    "button",
    "canvas",
    "caption",
    "center",
    "cite",
    "code",
    "col",
    "colgroup",
    "data",
    "datalist",
    "dd",
    "del",
    "details",
    "dfn",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "em",
    "embed",
    "fieldset",
    "figcaption",
    "figure",
    "font",
    "footer",
    "form",
    "frame",
    "frameset",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hgroup",
    "hr",
    "html",
    "i",
    "iframe",
    "img",
    "input",
    "ins",
    "kbd",
    "label",
    "legend",
    "li",
    "link",
    "listing",
    "main",
    "map",
    "mark",
    "marquee",
    "menu",
    "meta",
    "meter",
    "nav",
    "nobr",
    "noembed",
    "noframes",
    "noscript",
    "object",
    "ol",
    "optgroup",
    "option",
    "output",
    "p",
    "param",
    "picture",
    "plaintext",
    "pre",
    "progress",
    "q",
    "rb",
    "rp",
    "rt",
    "rtc",
    "ruby",
    "s",
    "samp",
    "script",
    "search",
    "section",
    "select",
    "slot",
    "small",
    "source",
    "span",
    "strike",
    "strong",
    "style",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "template",
    "textarea",
    "tfoot",
    "th",
    "thead",
    "time",
    "title",
    "tr",
    "track",
    "tt",
    "u",
    "ul",
    "var",
    "video",
    "wbr",
    "xmp",
];

/// Los accessors de los atributos reflejados en los prototipos de `realm`.
fn define_reflection(agent: &mut Agent, realm: RealmId) {
    let Some(classes) = html_classes(agent).cloned() else {
        return;
    };
    let saved = agent.current_realm;
    agent.current_realm = realm;
    for (i, (class, attr)) in classes.reflected.iter().enumerate() {
        let proto = agent.host_class_objects(realm, *class).prototype;
        let slot = vec![Value::Number(i as f64)];
        let (get, set): (bf_js::vm::function::NativeFn, bf_js::vm::function::NativeFn) =
            match attr.kind {
                Kind::TokenList => (token_list_get, token_list_set),
                _ => (reflect_get, reflect_set),
            };
        let g = agent.create_builtin_with_slots(get, 0, &format!("get {}", attr.idl), slot.clone());
        let s = agent.create_builtin_with_slots(set, 1, &format!("set {}", attr.idl), slot);
        let _ = agent.define_own_property(
            proto,
            PropertyKey::from(attr.idl),
            PropertyDescriptor::accessor(Some(g), Some(s), true, true),
        );
    }
    agent.current_realm = saved;
}

/// El atributo del accessor en curso, con el brand check de WebIDL.
pub(crate) fn reflected_attr(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<&'static Attr> {
    let i = match bf_js::builtins::slots(agent, a.callee).first() {
        Some(Value::Number(n)) => *n as usize,
        _ => usize::MAX,
    };
    let entry = html_classes(agent).and_then(|c| c.reflected.get(i).copied());
    match entry {
        Some((class, attr)) if agent.host_is_instance(&a.this, class) => Ok(attr),
        _ => agent.type_error("Illegal invocation"),
    }
}

/// El valor de un atributo de contenido (`None` si no está).
pub fn attr_value(agent: &Agent, n: NodeRef, name: &str) -> Option<DomString> {
    let name = DomString::from(name);
    with_doc(agent, n.doc, |d| d.get_attribute(n.node, &name))
}

/// Fija un atributo de contenido.
pub fn set_attr(agent: &mut Agent, n: NodeRef, name: &str, value: DomString) -> JsResult<()> {
    let name = DomString::from(name);
    let r = with_doc_mut(agent, n.doc, |d| d.set_attribute(n.node, &name, value));
    match r {
        Ok(()) => {
            mutated(agent);
            Ok(())
        }
        Err(e) => throw_dom(agent, e),
    }
}

/// Quita un atributo de contenido.
pub fn remove_attr(agent: &mut Agent, n: NodeRef, name: &str) {
    let name = DomString::from(name);
    with_doc_mut(agent, n.doc, |d| d.remove_attribute(n.node, &name));
    mutated(agent);
}

/// Parsea `s` contra la base del documento de `n` ("encoding-parsing-and-serializing a URL").
pub fn parse_url(agent: &Agent, n: NodeRef, s: &str) -> Option<Url> {
    let base = with_doc(agent, n.doc, |d| d.base_url());
    Url::parse(s, Some(&base)).ok()
}

fn js_str(s: &str) -> Value {
    Value::String(JsString::from(s))
}

fn got_to_value(g: Got) -> Value {
    match g {
        Got::Str(s) => js_str(&s),
        Got::Null => Value::Null,
        Got::Bool(b) => Value::Boolean(b),
        Got::Num(n) => Value::Number(n),
    }
}

/// El getter de un atributo reflejado (§2.6.1).
fn reflect_get(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let attr = reflected_attr(agent, a)?;
    let n = this_node(agent, a)?;
    let value = attr_value(agent, n, attr.content);
    match attr.kind {
        // Los strings se devuelven tal cual (sin pasar por UTF-8).
        Kind::StrNullEmpty | Kind::R(Reflect::Str) => {
            Ok(value.map_or_else(|| js_str(""), |v| from_dom_string(&v)))
        }
        Kind::R(r) => {
            let v = value.map(|s| s.to_string_lossy().into_owned());
            let base = with_doc(agent, n.doc, |d| d.base_url());
            let resolve = |s: &str| Url::parse(s, Some(&base)).ok().map(|u| u.serialize(false));
            Ok(got_to_value(reflect::get(r, v.as_deref(), &resolve)))
        }
        Kind::TokenList => Ok(Value::Undefined),
    }
}

/// El setter de un atributo reflejado (§2.6.1), con la conversión WebIDL del tipo IDL.
fn reflect_set(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let attr = reflected_attr(agent, a)?;
    let n = this_node(agent, a)?;
    let v = a.arg(0);
    let input = match attr.kind {
        Kind::StrNullEmpty => {
            let s = webidl::to_dom_string_null_empty(agent, &v)?;
            set_attr(agent, n, attr.content, DomString::from_utf16(s.units()))?;
            return Ok(Value::Undefined);
        }
        Kind::R(Reflect::Str | Reflect::Url) => {
            let s = webidl::to_dom_string(agent, &v)?;
            set_attr(agent, n, attr.content, DomString::from_utf16(s.units()))?;
            return Ok(Value::Undefined);
        }
        Kind::R(Reflect::Enum(spec)) if spec.nullable => {
            match webidl::to_nullable_dom_string(agent, &v)? {
                Some(s) => Input::Str(s.to_string_lossy()),
                None => Input::Null,
            }
        }
        Kind::R(Reflect::Enum(_)) => {
            Input::Str(webidl::to_dom_string(agent, &v)?.to_string_lossy())
        }
        Kind::R(Reflect::Bool) => Input::Bool(webidl::to_boolean(agent, &v)),
        Kind::R(Reflect::Long(_) | Reflect::LimitedLong(_)) => {
            Input::Num(f64::from(webidl::to_long(agent, &v)?))
        }
        Kind::R(
            Reflect::ULong(_) | Reflect::LimitedULong { .. } | Reflect::ClampedULong { .. },
        ) => Input::Num(f64::from(webidl::to_unsigned_long(agent, &v)?)),
        Kind::R(Reflect::Double(_) | Reflect::LimitedDouble(_)) => {
            Input::Num(webidl::to_double(agent, &v)?)
        }
        Kind::TokenList => return Ok(Value::Undefined),
    };
    let Kind::R(r) = attr.kind else {
        return Ok(Value::Undefined);
    };
    match reflect::set(r, input) {
        SetAction::Set(s) => set_attr(agent, n, attr.content, DomString::from(s.as_str()))?,
        SetAction::Remove => remove_attr(agent, n, attr.content),
        SetAction::Nothing => {}
        SetAction::IndexSizeError => return throw_dom(agent, DomException::IndexSizeError),
    }
    Ok(Value::Undefined)
}

// --- HTMLElement (§3.2.8, §6) ---

/// El nombre local de un elemento HTML (`None` si no es un elemento HTML).
pub fn html_local_name(agent: &Agent, n: NodeRef) -> Option<String> {
    with_doc(agent, n.doc, |d| match &d.get(n.node).data {
        NodeData::Element(e) if e.ns == Namespace::Html => Some(e.name.to_string()),
        _ => None,
    })
}

/// ¿El elemento es focusable por defecto? (El valor por defecto de `tabIndex`, §6.6.3:
/// 0 para los controles y los enlaces con `href`, -1 para los demás.)
fn focusable_by_default(agent: &Agent, n: NodeRef) -> bool {
    let Some(name) = html_local_name(agent, n) else {
        return false;
    };
    match name.as_str() {
        "a" | "area" => attr_value(agent, n, "href").is_some(),
        "button" | "select" | "textarea" | "iframe" | "embed" | "object" => true,
        "input" => attr_value(agent, n, "type")
            .is_none_or(|t| !t.to_string_lossy().eq_ignore_ascii_case("hidden")),
        "summary" => true,
        _ => false,
    }
}

fn get_tab_index(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = attr_value(agent, n, "tabindex")
        .and_then(|s| reflect::parse_integer(&s.to_string_lossy()))
        .filter(|v| (-2147483648..=2147483647).contains(v));
    let d = if focusable_by_default(agent, n) {
        0
    } else {
        -1
    };
    Ok(Value::Number(v.map_or(f64::from(d), |v| v as f64)))
}

fn set_tab_index(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = webidl::to_long(agent, &a.arg(0))?;
    set_attr(
        agent,
        n,
        "tabindex",
        DomString::from(v.to_string().as_str()),
    )?;
    Ok(Value::Undefined)
}

/// `accessKeyLabel` (§6.7.2): la combinación asignada al primer token de un carácter de
/// `accesskey` (en Windows, Alt + la tecla); vacío si no hay tecla asignada.
fn access_key_label(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let key = attr_value(agent, n, "accesskey").and_then(|v| {
        v.to_string_lossy()
            .split(['\t', '\n', '\x0C', '\r', ' '])
            .find(|t| t.chars().count() == 1)
            .map(str::to_uppercase)
    });
    Ok(js_str(&key.map(|k| format!("Alt+{k}")).unwrap_or_default()))
}

/// `draggable` (§6.11.7): `true`/`false` del atributo; sin él, `true` para `img` y los
/// enlaces con `href`.
fn get_draggable(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = attr_value(agent, n, "draggable").map(|s| s.to_string_lossy().to_ascii_lowercase());
    let r = match v.as_deref() {
        Some("true") => true,
        Some("false") => false,
        _ => match html_local_name(agent, n).as_deref() {
            Some("img") => true,
            Some("a") => attr_value(agent, n, "href").is_some(),
            _ => false,
        },
    };
    Ok(Value::Boolean(r))
}

fn set_draggable(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let b = webidl::to_boolean(agent, &a.arg(0));
    set_attr(
        agent,
        n,
        "draggable",
        DomString::from(if b { "true" } else { "false" }),
    )?;
    Ok(Value::Undefined)
}

/// Los atributos enumerados heredados (`spellcheck`, `translate`): el estado del propio
/// elemento o, si es "inherit", el del padre (`default` en la raíz).
fn inherited_state(
    agent: &Agent,
    n: NodeRef,
    name: &str,
    on: &str,
    off: &str,
    default: bool,
) -> bool {
    let mut cur = Some(n.node);
    while let Some(id) = cur {
        let r = NodeRef::new(n.doc, id);
        if let Some(v) = attr_value(agent, r, name) {
            let v = v.to_string_lossy().to_ascii_lowercase();
            if v == on || (v.is_empty() && !on.is_empty()) {
                return true;
            }
            if v == off {
                return false;
            }
        }
        cur = with_doc(agent, n.doc, |d| d.parent_element(id));
    }
    default
}

fn get_spellcheck(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Boolean(inherited_state(
        agent,
        n,
        "spellcheck",
        "true",
        "false",
        true,
    )))
}

fn set_spellcheck(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let b = webidl::to_boolean(agent, &a.arg(0));
    set_attr(
        agent,
        n,
        "spellcheck",
        DomString::from(if b { "true" } else { "false" }),
    )?;
    Ok(Value::Undefined)
}

fn get_translate(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Boolean(inherited_state(
        agent,
        n,
        "translate",
        "yes",
        "no",
        true,
    )))
}

fn set_translate(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let b = webidl::to_boolean(agent, &a.arg(0));
    set_attr(
        agent,
        n,
        "translate",
        DomString::from(if b { "yes" } else { "no" }),
    )?;
    Ok(Value::Undefined)
}

/// `contentEditable` (§6.8.1): "true", "false", "plaintext-only" o "inherit".
fn get_content_editable(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v =
        attr_value(agent, n, "contenteditable").map(|s| s.to_string_lossy().to_ascii_lowercase());
    Ok(js_str(match v.as_deref() {
        Some("" | "true") => "true",
        Some("false") => "false",
        Some("plaintext-only") => "plaintext-only",
        _ => "inherit",
    }))
}

fn set_content_editable(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = webidl::to_dom_string(agent, &a.arg(0))?.to_string_lossy();
    match v.to_ascii_lowercase().as_str() {
        "inherit" => remove_attr(agent, n, "contenteditable"),
        k @ ("true" | "false" | "plaintext-only") => {
            set_attr(agent, n, "contenteditable", DomString::from(k))?
        }
        _ => return throw_dom(agent, DomException::SyntaxError),
    }
    Ok(Value::Undefined)
}

fn is_content_editable(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let mut cur = Some(n.node);
    while let Some(id) = cur {
        let r = NodeRef::new(n.doc, id);
        if let Some(v) = attr_value(agent, r, "contenteditable") {
            match v.to_string_lossy().to_ascii_lowercase().as_str() {
                "" | "true" | "plaintext-only" => return Ok(Value::Boolean(true)),
                "false" => return Ok(Value::Boolean(false)),
                _ => {}
            }
        }
        cur = with_doc(agent, n.doc, |d| d.parent_element(id));
    }
    Ok(Value::Boolean(false))
}

/// Los elementos cuyo contenido no se renderiza (y no aportan a `innerText`).
const NOT_RENDERED: &[&str] = &[
    "head", "script", "style", "template", "title", "meta", "link", "noscript", "datalist",
    "param", "source", "track", "area", "base", "noembed", "noframes",
];

/// Los elementos de bloque (aportan saltos de línea a `innerText`).
const BLOCKS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "body",
    "center",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "html",
    "legend",
    "li",
    "listing",
    "main",
    "menu",
    "nav",
    "ol",
    "optgroup",
    "option",
    "pre",
    "search",
    "section",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "ul",
    "xmp",
    "caption",
    "plaintext",
];

/// El *rendered text collection* aproximado (§6.13 "The innerText getter steps") sin layout:
/// el texto de los descendientes renderizados con los espacios colapsados, un salto de línea
/// alrededor de los bloques, dos alrededor de `p`, `\n` por `br` y tabuladores entre celdas.
pub fn rendered_text(agent: &Agent, n: NodeRef) -> String {
    #[derive(PartialEq)]
    enum Item {
        Text(String),
        Breaks(usize),
    }
    fn walk(d: &bf_dom::Document, id: bf_dom::NodeId, pre: bool, out: &mut Vec<Item>) {
        match &d.get(id).data {
            NodeData::Text(t) | NodeData::CdataSection(t) => {
                let s = t.to_string();
                if pre {
                    out.push(Item::Text(s));
                } else {
                    let mut c = String::new();
                    let mut ws = false;
                    for ch in s.chars() {
                        if matches!(ch, ' ' | '\t' | '\n' | '\r' | '\x0C') {
                            ws = true;
                        } else {
                            if ws {
                                c.push(' ');
                            }
                            ws = false;
                            c.push(ch);
                        }
                    }
                    if ws {
                        c.push(' ');
                    }
                    out.push(Item::Text(c));
                }
            }
            NodeData::Element(e) => {
                let name = e.name.to_string();
                if e.ns == Namespace::Html && NOT_RENDERED.contains(&name.as_str()) {
                    return;
                }
                if name == "br" {
                    out.push(Item::Text("\n".into()));
                    return;
                }
                let pre = pre || matches!(name.as_str(), "pre" | "listing" | "xmp" | "textarea");
                let block = BLOCKS.contains(&name.as_str());
                let para = name == "p";
                if para {
                    out.push(Item::Breaks(2));
                } else if block {
                    out.push(Item::Breaks(1));
                }
                let mut c = d.first_child(id);
                let mut first_cell = true;
                while let Some(ch) = c {
                    let is_cell = matches!(&d.get(ch).data, NodeData::Element(e)
                        if matches!(e.name.to_string().as_str(), "td" | "th"));
                    if is_cell && !first_cell {
                        out.push(Item::Text("\t".into()));
                    }
                    if is_cell {
                        first_cell = false;
                    }
                    walk(d, ch, pre, out);
                    c = d.next_sibling(ch);
                }
                if para {
                    out.push(Item::Breaks(2));
                } else if block {
                    out.push(Item::Breaks(1));
                }
            }
            _ => {
                let mut c = d.first_child(id);
                while let Some(ch) = c {
                    walk(d, ch, pre, out);
                    c = d.next_sibling(ch);
                }
            }
        }
    }
    with_doc(agent, n.doc, |d| {
        let mut items = Vec::new();
        let mut c = d.first_child(n.node);
        while let Some(ch) = c {
            walk(d, ch, false, &mut items);
            c = d.next_sibling(ch);
        }
        // Los espacios al principio y al final de cada línea se quitan (como en el layout),
        // los saltos pedidos se combinan (el máximo) y no hay saltos al principio ni al final.
        let mut out = String::new();
        let mut pending = 0usize;
        for it in items {
            match it {
                Item::Breaks(k) => pending = pending.max(k),
                Item::Text(t) => {
                    if t.is_empty() {
                        continue;
                    }
                    if !out.is_empty() && pending > 0 {
                        while out.ends_with(' ') {
                            out.pop();
                        }
                        out.push_str(&"\n".repeat(pending));
                    }
                    pending = 0;
                    let t = if out.is_empty() || out.ends_with('\n') || out.ends_with(' ') {
                        t.trim_start_matches(' ').to_owned()
                    } else {
                        t
                    };
                    out.push_str(&t);
                }
            }
        }
        while out.ends_with(' ') {
            out.pop();
        }
        out
    })
}

fn get_inner_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(js_str(&rendered_text(agent, n)))
}

/// "The inner text setter steps" (§6.13): reemplaza los hijos por el texto, con un `br` por
/// cada salto de línea.
fn set_inner_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let text = webidl::to_dom_string_null_empty(agent, &a.arg(0))?.to_string_lossy();
    with_doc_mut(agent, n.doc, |d| {
        let frag = d.create_document_fragment();
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        for (i, line) in normalized.split('\n').enumerate() {
            if i > 0 {
                let br = d.create_element_raw(Namespace::Html, None, DomString::from("br"));
                let _ = d.append_child(frag, br);
            }
            if !line.is_empty() {
                let t = d.create_text_node(DomString::from(line));
                let _ = d.append_child(frag, t);
            }
        }
        d.replace_all(Some(frag), n.node);
    });
    mutated(agent);
    Ok(Value::Undefined)
}

/// "The outer text setter steps" (§6.13): reemplaza el elemento por el texto.
fn set_outer_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let text = webidl::to_dom_string_null_empty(agent, &a.arg(0))?.to_string_lossy();
    let parent = with_doc(agent, n.doc, |d| d.parent(n.node));
    let Some(parent) = parent else {
        return throw_dom(agent, DomException::NoModificationAllowedError);
    };
    let r = with_doc_mut(agent, n.doc, |d| {
        let frag = d.create_document_fragment();
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        for (i, line) in normalized.split('\n').enumerate() {
            if i > 0 {
                let br = d.create_element_raw(Namespace::Html, None, DomString::from("br"));
                let _ = d.append_child(frag, br);
            }
            if !line.is_empty() {
                let t = d.create_text_node(DomString::from(line));
                let _ = d.append_child(frag, t);
            }
        }
        if d.first_child(frag).is_none() {
            let t = d.create_text_node(DomString::from(""));
            let _ = d.append_child(frag, t);
        }
        d.replace_child(parent, frag, n.node).map(|_| ())
    });
    match r {
        Ok(()) => {
            mutated(agent);
            Ok(Value::Undefined)
        }
        Err(e) => throw_dom(agent, e),
    }
}

/// Un nodo de `v` que es un elemento HTML (para los argumentos de las interfaces HTML).
pub fn html_element_arg(agent: &Agent, v: &Value) -> Option<NodeRef> {
    let n = unwrap_node(agent, v)?;
    html_local_name(agent, n).map(|_| n)
}

// --- DOMTokenList reflejados y colecciones vivas ---

/// `[SameObject, PutForwards=value] readonly attribute DOMTokenList` (`relList`, `sandbox`,
/// `sizes`, `htmlFor` de `output`): el `DOMTokenList` del DOM sobre el atributo.
fn token_list_get(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let attr = reflected_attr(agent, a)?;
    let n = this_node(agent, a)?;
    Ok(crate::dom::collections::token_list(agent, n, attr.content))
}

/// `[PutForwards=value]`: asignar el atributo IDL fija el `value` del `DOMTokenList`, es
/// decir, el atributo de contenido.
fn token_list_set(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let attr = reflected_attr(agent, a)?;
    let n = this_node(agent, a)?;
    let v = webidl::to_dom_string(agent, &a.arg(0))?;
    set_attr(agent, n, attr.content, DomString::from_utf16(v.units()))?;
    Ok(Value::Undefined)
}

/// Una `HTMLCollection` viva de `owner` calculada por `list`, siempre el mismo objeto
/// mientras JavaScript la alcance (`[SameObject]`); `which` distingue las colecciones de un
/// mismo elemento.
pub fn live_collection(
    agent: &mut Agent,
    owner: NodeRef,
    which: u64,
    list: fn(&bf_dom::Document, bf_dom::NodeId) -> Vec<bf_dom::NodeId>,
) -> Value {
    let realm = agent.current_realm;
    let key = bf_js::host::HostKey(owner.key().0 ^ ((which & 0xff) << 52) | (1 << 62));
    if let Some(o) = agent.host_lookup(realm, key) {
        return Value::Object(o);
    }
    let class = dom::classes(agent).html_collection;
    let coll = bf_dom::collections::HtmlCollection::new(
        owner.node,
        bf_dom::collections::CollectionFilter::Computed(bf_dom::collections::ComputedList(list)),
    );
    Value::Object(agent.host_wrap(
        realm,
        key,
        class,
        Box::new(crate::dom::collections::HtmlCollectionData {
            doc: owner.doc,
            coll,
        }),
    ))
}
