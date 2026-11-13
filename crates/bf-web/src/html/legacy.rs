//! E1: miembros de HTML que faltaban en `Document` y `HTMLElement`.
//!
//! - `Document` (HTML §3.1.3, §6.10 "editing APIs" y §16.3.4 "obsolete"): `domain`,
//!   `designMode`, `execCommand` y los `queryCommand*` (sin editor: no hay comandos
//!   habilitados), y `clear()`/`captureEvents()`/`releaseEvents()`, que no hacen nada.
//! - `HTMLElement`: el atributo `popover` y `showPopover()`/`hidePopover()`/`togglePopover()`
//!   (§6.12, sin capa superior: la *popover visibility state* la guarda el DOM, ver
//!   `bf_dom::Document::set_popover_showing`, y la usa `:popover-open`),
//!   `autocapitalize`/`autocorrect` (§6.8.6-7), `writingSuggestions` (§6.8.8) y
//!   `headingOffset`/`headingReset` (§4.3.11, niveles de encabezado).

use std::collections::HashSet;

use bf_dom::DomException;
use bf_infra::DomString;
use bf_js::host::webidl;
use bf_js::string::JsString;
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use super::elements::{attr_value, remove_attr, set_attr};
use crate::dom::{this_node, throw_dom};
use crate::{DocIndex, NodeRef, WebContextHandle};

fn js_str(s: &str) -> Value {
    Value::String(JsString::from(s))
}

/// Los documentos con `designMode` en `on`.
#[derive(Debug, Default)]
struct LegacyState {
    design_mode: HashSet<DocIndex>,
}

fn legacy_state(agent: &mut Agent) -> &mut LegacyState {
    let realm = WebContextHandle::current(agent).realm;
    if agent.realm_data::<LegacyState>(realm).is_none() {
        agent.set_realm_data(realm, LegacyState::default());
    }
    agent
        .realm_data_mut::<LegacyState>(realm)
        .expect("LegacyState")
}

// --- Document ---

/// `document.domain` (§7.5.2): el host del origen del documento (vacío si es opaco).
pub fn get_domain(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let url = crate::with_doc(agent, n.doc, |d| d.url.clone());
    Ok(js_str(&url.hostname()))
}

/// El setter: solo el mismo host (sin `document.domain` relajado entre subdominios, que los
/// navegadores ya deshabilitan por defecto con origin-keyed agent clusters).
pub fn set_domain(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = webidl::to_dom_string(agent, &a.arg(0))?.to_string_lossy();
    let url = crate::with_doc(agent, n.doc, |d| d.url.clone());
    let host = url.hostname();
    if !host.is_empty() && host.eq_ignore_ascii_case(&v) {
        Ok(Value::Undefined)
    } else {
        throw_dom(agent, DomException::SecurityError)
    }
}

/// `designMode` (§6.10.3): `"on"` u `"off"`.
pub fn get_design_mode(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let on = legacy_state(agent).design_mode.contains(&n.doc);
    Ok(js_str(if on { "on" } else { "off" }))
}

pub fn set_design_mode(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = webidl::to_dom_string(agent, &a.arg(0))?.to_string_lossy();
    match v.to_ascii_lowercase().as_str() {
        "on" => {
            legacy_state(agent).design_mode.insert(n.doc);
        }
        "off" => {
            legacy_state(agent).design_mode.remove(&n.doc);
        }
        _ => {}
    }
    Ok(Value::Undefined)
}

/// `execCommand(commandId, showUI, value)`: sin editor, ningún comando está habilitado.
pub fn exec_command(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_node(agent, a)?;
    crate::dom::events::required(agent, a, 1, "execCommand")?;
    webidl::to_dom_string(agent, &a.arg(0))?;
    Ok(Value::Boolean(false))
}

/// `queryCommandEnabled`/`Indeterm`/`State`/`Supported(commandId)`: `false`.
pub fn query_command_false(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_node(agent, a)?;
    crate::dom::events::required(agent, a, 1, "queryCommand")?;
    webidl::to_dom_string(agent, &a.arg(0))?;
    Ok(Value::Boolean(false))
}

/// `queryCommandValue(commandId)`: `""`.
pub fn query_command_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_node(agent, a)?;
    crate::dom::events::required(agent, a, 1, "queryCommandValue")?;
    webidl::to_dom_string(agent, &a.arg(0))?;
    Ok(js_str(""))
}

/// `clear()`, `captureEvents()`, `releaseEvents()` (§16.3.4): no hacen nada.
pub fn noop(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_node(agent, a)?;
    Ok(Value::Undefined)
}

// --- HTMLElement: popover ---

/// E3: ¿`n` tiene `popover` y está cerrado? (La hoja UA de HTML §15.3.1 los oculta con
/// `[popover]:not(:popover-open):not(dialog[open]) { display: none }`.)
pub fn is_closed_popover(agent: &Agent, n: NodeRef) -> bool {
    popover_state(agent, n).is_some() && !is_popover_showing(agent, n)
}

/// E5: ¿Está `n` en la *popover visibility state* `showing` (§6.12)?
fn is_popover_showing(agent: &Agent, n: NodeRef) -> bool {
    crate::with_doc(agent, n.doc, |d| d.is_popover_showing(n.node))
}

/// E5: cambia la *popover visibility state* de `n` en su documento (sube la versión si
/// cambia, para que el estilo recalcule `:popover-open`).
fn set_popover_showing(agent: &mut Agent, n: NodeRef, showing: bool) {
    crate::with_doc_mut(agent, n.doc, |d| d.set_popover_showing(n.node, showing));
}

/// El estado del atributo `popover` (§6.12): `auto` (vacío o `auto`), `manual`, `hint`, o
/// `manual` si el valor no es válido; `None` sin el atributo.
fn popover_state(agent: &Agent, n: NodeRef) -> Option<&'static str> {
    let v = attr_value(agent, n, "popover")?
        .to_string_lossy()
        .to_ascii_lowercase();
    Some(match v.as_str() {
        "" | "auto" => "auto",
        "hint" => "hint",
        _ => "manual",
    })
}

pub fn get_popover(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(popover_state(agent, n).map_or(Value::Null, js_str))
}

pub fn set_popover(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = a.arg(0);
    if v.is_null() {
        remove_attr(agent, n, "popover");
    } else {
        let s = webidl::to_dom_string(agent, &v)?;
        set_attr(agent, n, "popover", DomString::from_utf16(s.units()))?;
    }
    Ok(Value::Undefined)
}

/// "Check popover validity" (§6.12.2): sin `popover`, `NotSupportedError`; desconectado,
/// `InvalidStateError`.
fn check_popover(agent: &mut Agent, n: NodeRef) -> JsResult<()> {
    if popover_state(agent, n).is_none() {
        return throw_dom(agent, DomException::NotSupportedError);
    }
    if !crate::with_doc(agent, n.doc, |d| d.is_connected(n.node)) {
        return throw_dom(agent, DomException::InvalidStateError);
    }
    Ok(())
}

pub fn show_popover(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    check_popover(agent, n)?;
    set_popover_showing(agent, n, true);
    Ok(Value::Undefined)
}

pub fn hide_popover(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    check_popover(agent, n)?;
    set_popover_showing(agent, n, false);
    Ok(Value::Undefined)
}

/// `togglePopover(force)`: `force` es un booleano o un diccionario con `force`; devuelve si
/// quedó abierto.
pub fn toggle_popover(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let force = match a.arg(0) {
        Value::Undefined | Value::Null => None,
        Value::Object(o) if !agent.is_callable(&Value::Object(o)) => {
            match agent.get_str(o, "force")? {
                Value::Undefined => None,
                v => Some(webidl::to_boolean(agent, &v)),
            }
        }
        v => Some(webidl::to_boolean(agent, &v)),
    };
    check_popover(agent, n)?;
    let open = force.unwrap_or(!is_popover_showing(agent, n));
    set_popover_showing(agent, n, open);
    Ok(Value::Boolean(open))
}

// --- HTMLElement: autocapitalize, autocorrect, writingSuggestions, headings ---

/// `autocapitalize` (§6.8.6): el estado del atributo (`none`, `sentences`, `words`,
/// `characters`), o `""` sin él o con un valor desconocido.
pub fn get_autocapitalize(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = attr_value(agent, n, "autocapitalize")
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    Ok(js_str(match v.as_str() {
        "off" | "none" => "none",
        "on" | "sentences" => "sentences",
        "words" => "words",
        "characters" => "characters",
        _ => "",
    }))
}

fn set_string(agent: &mut Agent, a: &CallArgs<'_>, attr: &str) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = webidl::to_dom_string(agent, &a.arg(0))?;
    set_attr(agent, n, attr, DomString::from_utf16(s.units()))?;
    Ok(Value::Undefined)
}

pub fn set_autocapitalize(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    set_string(agent, a, "autocapitalize")
}

/// `autocorrect` (§6.8.7): `true` salvo con `autocorrect=off`.
pub fn get_autocorrect(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let off = attr_value(agent, n, "autocorrect")
        .is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case("off"));
    Ok(Value::Boolean(!off))
}

pub fn set_autocorrect(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let on = webidl::to_boolean(agent, &a.arg(0));
    set_attr(
        agent,
        n,
        "autocorrect",
        DomString::from(if on { "on" } else { "off" }),
    )?;
    Ok(Value::Undefined)
}

/// `writingSuggestions` (§6.8.8): `"false"` con `writingsuggestions=false`; si no, `"true"`.
pub fn get_writing_suggestions(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let off = attr_value(agent, n, "writingsuggestions")
        .is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case("false"));
    Ok(js_str(if off { "false" } else { "true" }))
}

pub fn set_writing_suggestions(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    set_string(agent, a, "writingsuggestions")
}

/// `headingOffset` (§4.3.11): el entero no negativo de `headingoffset`, como máximo 8.
pub fn get_heading_offset(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = attr_value(agent, n, "headingoffset")
        .and_then(|s| s.to_string_lossy().trim().parse::<u32>().ok())
        .map_or(0, |x| x.min(8));
    Ok(Value::Number(f64::from(v)))
}

pub fn set_heading_offset(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = webidl::to_unsigned_long(agent, &a.arg(0))?;
    set_attr(
        agent,
        n,
        "headingoffset",
        DomString::from(v.to_string().as_str()),
    )?;
    Ok(Value::Undefined)
}

/// `headingReset` (atributo booleano `headingreset`).
pub fn get_heading_reset(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Boolean(
        attr_value(agent, n, "headingreset").is_some(),
    ))
}

pub fn set_heading_reset(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    if webidl::to_boolean(agent, &a.arg(0)) {
        set_attr(agent, n, "headingreset", DomString::new())?;
    } else {
        remove_attr(agent, n, "headingreset");
    }
    Ok(Value::Undefined)
}
