//! Los formularios (HTML §4.10): `HTMLInputElement` (el valor con su *dirty value flag* y la
//! sanitización por tipo, checkedness, selección), `HTMLTextAreaElement`, `HTMLSelectElement`,
//! `HTMLOptionElement`, `HTMLButtonElement`, `HTMLFormElement` (`submit`, `requestSubmit`,
//! `reset`, `action`), `HTMLLabelElement`, `HTMLFieldSetElement`, `HTMLOutputElement`,
//! `HTMLLegendElement`, `HTMLMeterElement` y `HTMLProgressElement`.
//!
//! El estado de los controles que no guarda `bf-dom` (el valor, su dirty flag, la selección
//! de texto, `indeterminate`) vive en [`ControlStates`], por nodo.

use std::collections::HashMap;

use bf_dom::events::{EventInit, EventKind, EventTarget};
use bf_dom::{DomException, Namespace, NodeData};
use bf_infra::DomString;
use bf_js::host::{HostClass, webidl};
use bf_js::string::JsString;
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use super::elements::{attr_value, html_element_arg, html_local_name, parse_url, set_attr};
use super::reflect::parse_float;
use super::scripting::fire_event;
use crate::dom::{from_dom_string, mutated, this_node, throw_dom, wrap_node, wrap_opt};
use crate::{NodeRef, WebContextHandle, with_doc, with_doc_mut};

fn js_str(s: &str) -> Value {
    Value::String(JsString::from(s))
}

/// El estado de un control.
#[derive(Debug, Clone, Default)]
pub struct ControlState {
    /// El valor si el *dirty value flag* está puesto (sin él, el valor sale del atributo o
    /// del contenido).
    pub value: Option<String>,
    pub indeterminate: bool,
    /// (inicio, fin, dirección) de la selección, en code units.
    pub selection: Option<(u32, u32, &'static str)>,
    pub custom_validity: String,
}

/// Los estados de los controles de la ventana.
#[derive(Debug, Default)]
pub struct ControlStates(pub HashMap<NodeRef, ControlState>);

fn state(agent: &Agent, n: NodeRef) -> ControlState {
    let realm = WebContextHandle::current(agent).realm;
    agent
        .realm_data::<ControlStates>(realm)
        .and_then(|s| s.0.get(&n).cloned())
        .unwrap_or_default()
}

fn state_mut(agent: &mut Agent, n: NodeRef) -> &mut ControlState {
    let realm = WebContextHandle::current(agent).realm;
    if agent.realm_data::<ControlStates>(realm).is_none() {
        agent.set_realm_data(realm, ControlStates::default());
    }
    agent
        .realm_data_mut::<ControlStates>(realm)
        .expect("ControlStates")
        .0
        .entry(n)
        .or_default()
}

/// Las operaciones y atributos propios de cada interfaz de formulario.
pub fn extend(name: &str, class: HostClass) -> HostClass {
    match name {
        "HTMLInputElement" => text_selection(
            class
                .attribute("value", Some(input_value), Some(set_input_value))
                .attribute("checked", Some(input_checked), Some(set_input_checked))
                .attribute(
                    "indeterminate",
                    Some(get_indeterminate),
                    Some(set_indeterminate),
                )
                .attribute("form", Some(form_owner), None)
                .attribute("files", Some(input_files), None)
                .attribute(
                    "valueAsNumber",
                    Some(value_as_number),
                    Some(set_value_as_number),
                )
                .attribute("autocomplete", Some(autocomplete), Some(set_autocomplete))
                .attribute("width", Some(zero), Some(ignore))
                .attribute("height", Some(zero), Some(ignore)),
        )
        .attribute("willValidate", Some(will_validate), None)
        .method("checkValidity", 0, check_validity)
        .method("reportValidity", 0, check_validity)
        .method("setCustomValidity", 1, set_custom_validity)
        .attribute("validationMessage", Some(validation_message), None),
        "HTMLTextAreaElement" => text_selection(
            class
                .attribute("value", Some(textarea_value), Some(set_textarea_value))
                .attribute("defaultValue", Some(child_text), Some(set_child_text))
                .attribute("textLength", Some(text_length), None)
                .attribute("type", Some(type_textarea), None)
                .attribute("form", Some(form_owner), None)
                .attribute("autocomplete", Some(autocomplete), Some(set_autocomplete)),
        )
        .attribute("willValidate", Some(will_validate), None)
        .method("checkValidity", 0, check_validity)
        .method("reportValidity", 0, check_validity)
        .method("setCustomValidity", 1, set_custom_validity)
        .attribute("validationMessage", Some(validation_message), None),
        "HTMLSelectElement" => class
            .attribute("type", Some(select_type), None)
            .attribute("length", Some(select_length), None)
            .attribute(
                "selectedIndex",
                Some(selected_index),
                Some(set_selected_index),
            )
            .attribute("value", Some(select_value), Some(set_select_value))
            .attribute("form", Some(form_owner), None)
            .attribute("autocomplete", Some(autocomplete), Some(set_autocomplete))
            .attribute("willValidate", Some(will_validate), None)
            .method("checkValidity", 0, check_validity)
            .method("reportValidity", 0, check_validity)
            .method("setCustomValidity", 1, set_custom_validity)
            .attribute("validationMessage", Some(validation_message), None)
            .method("item", 1, select_item),
        "HTMLOptionElement" => class
            .attribute("text", Some(option_text), Some(set_child_text_all))
            .attribute("value", Some(option_value), Some(set_option_value))
            .attribute("label", Some(option_label), Some(set_option_label))
            .attribute("selected", Some(option_selected), Some(set_option_selected))
            .attribute("index", Some(option_index), None)
            .attribute("form", Some(option_form), None),
        "HTMLButtonElement" => class
            .attribute("form", Some(form_owner), None)
            .attribute("willValidate", Some(will_validate), None)
            .method("checkValidity", 0, check_validity)
            .method("reportValidity", 0, check_validity)
            .method("setCustomValidity", 1, set_custom_validity)
            .attribute("validationMessage", Some(validation_message), None),
        "HTMLFormElement" => class
            .attribute("action", Some(form_action), Some(set_form_action))
            .attribute("length", Some(form_length), None)
            .method("submit", 0, form_submit)
            .method("requestSubmit", 0, form_request_submit)
            .method("reset", 0, form_reset)
            .method("checkValidity", 0, check_validity)
            .method("reportValidity", 0, check_validity),
        "HTMLLabelElement" => class
            .attribute("control", Some(label_control), None)
            .attribute("form", Some(label_form), None),
        "HTMLFieldSetElement" => class
            .attribute("type", Some(type_fieldset), None)
            .attribute("form", Some(form_owner), None)
            .attribute("willValidate", Some(false_value), None)
            .method("checkValidity", 0, check_validity)
            .method("reportValidity", 0, check_validity)
            .method("setCustomValidity", 1, set_custom_validity)
            .attribute("validationMessage", Some(validation_message), None),
        "HTMLOutputElement" => class
            .attribute("type", Some(type_output), None)
            .attribute("value", Some(output_value), Some(set_output_value))
            .attribute(
                "defaultValue",
                Some(output_default),
                Some(set_output_default),
            )
            .attribute("form", Some(form_owner), None)
            .attribute("willValidate", Some(false_value), None)
            .method("checkValidity", 0, check_validity)
            .method("reportValidity", 0, check_validity)
            .method("setCustomValidity", 1, set_custom_validity)
            .attribute("validationMessage", Some(validation_message), None),
        "HTMLLegendElement" => class.attribute("form", Some(legend_form), None),
        "HTMLObjectElement" => class.attribute("form", Some(form_owner), None),
        "HTMLMeterElement" => class
            .attribute("value", Some(meter_value), Some(set_meter("value")))
            .attribute("min", Some(meter_min), Some(set_meter("min")))
            .attribute("max", Some(meter_max), Some(set_meter("max")))
            .attribute("low", Some(meter_low), Some(set_meter("low")))
            .attribute("high", Some(meter_high), Some(set_meter("high")))
            .attribute("optimum", Some(meter_optimum), Some(set_meter("optimum"))),
        "HTMLProgressElement" => class
            .attribute("value", Some(progress_value), Some(set_progress_value))
            .attribute("position", Some(progress_position), None),
        _ => class,
    }
}

fn zero(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(0.0))
}

fn ignore(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Undefined)
}

fn false_value(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(false))
}

// --- HTMLInputElement (§4.10.5) ---

/// El estado del atributo `type` (palabra clave canónica; "text" si falta o es inválido).
pub fn input_type(agent: &Agent, n: NodeRef) -> String {
    let v = attr_value(agent, n, "type").map(|s| s.to_string_lossy().to_ascii_lowercase());
    match v.as_deref() {
        Some(
            t @ ("hidden" | "text" | "search" | "tel" | "url" | "email" | "password" | "date"
            | "month" | "week" | "time" | "datetime-local" | "number" | "range" | "color"
            | "checkbox" | "radio" | "file" | "submit" | "image" | "reset" | "button"),
        ) => t.to_owned(),
        _ => "text".to_owned(),
    }
}

/// El modo de `value` (§4.10.5.4): value, default, default/on o filename.
fn value_mode(t: &str) -> &'static str {
    match t {
        "hidden" | "submit" | "image" | "reset" | "button" => "default",
        "checkbox" | "radio" => "default/on",
        "file" => "filename",
        _ => "value",
    }
}

fn strip_newlines(s: &str) -> String {
    s.chars().filter(|c| *c != '\n' && *c != '\r').collect()
}

fn trim_ascii_ws(s: &str) -> &str {
    s.trim_matches(|c: char| matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' '))
}

/// "Valid floating-point number" (§2.3.4.3, la sintaxis).
fn is_valid_float(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if b.first() == Some(&b'-') {
        i = 1;
    }
    let start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i - start;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let f = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == f {
            return false;
        }
    } else if int_digits == 0 {
        return false;
    }
    if int_digits == 0 && !s[start..].starts_with('.') {
        return false;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
            i += 1;
        }
        let e = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == e {
            return false;
        }
    }
    i == b.len()
}

/// "Valid simple color" en minúsculas (§2.3.6).
fn simple_color(s: &str) -> Option<String> {
    let b = s.as_bytes();
    (b.len() == 7 && b[0] == b'#' && b[1..].iter().all(u8::is_ascii_hexdigit))
        .then(|| s.to_ascii_lowercase())
}

fn is_digits(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|b| b.is_ascii_digit())
}

fn days_in_month(y: u32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

/// "Valid month string" (§2.3.5.1).
fn valid_month(s: &str) -> bool {
    let Some((y, m)) = s.rsplit_once('-') else {
        return false;
    };
    y.len() >= 4
        && y.bytes().all(|b| b.is_ascii_digit())
        && y.parse::<u32>().is_ok_and(|y| y > 0)
        && is_digits(m, 2)
        && (1..=12).contains(&m.parse::<u32>().unwrap_or(0))
}

/// "Valid date string" (§2.3.5.2).
fn valid_date(s: &str) -> bool {
    let Some((ym, d)) = s.rsplit_once('-') else {
        return false;
    };
    if !valid_month(ym) || !is_digits(d, 2) {
        return false;
    }
    let (y, m) = ym.rsplit_once('-').expect("mes");
    let d: u32 = d.parse().unwrap_or(0);
    d >= 1 && d <= days_in_month(y.parse().unwrap_or(1), m.parse().unwrap_or(1))
}

/// "Valid time string" (§2.3.5.4).
fn valid_time(s: &str) -> bool {
    let parts: Vec<&str> = s.split(':').collect();
    let ok2 = |p: &str, max: u32| is_digits(p, 2) && p.parse::<u32>().is_ok_and(|v| v <= max);
    match parts.as_slice() {
        [h, m] => ok2(h, 23) && ok2(m, 59),
        [h, m, sec] => {
            let (s, frac) = sec.split_once('.').unwrap_or((sec, ""));
            ok2(h, 23)
                && ok2(m, 59)
                && ok2(s, 59)
                && (frac.is_empty() && !sec.ends_with('.')
                    || (1..=3).contains(&frac.len()) && frac.bytes().all(|b| b.is_ascii_digit()))
        }
        _ => false,
    }
}

/// "Valid week string" (§2.3.5.8), sin verificar que la semana 53 exista ese año.
fn valid_week(s: &str) -> bool {
    let Some((y, w)) = s.split_once("-W") else {
        return false;
    };
    y.len() >= 4
        && y.bytes().all(|b| b.is_ascii_digit())
        && is_digits(w, 2)
        && (1..=53).contains(&w.parse::<u32>().unwrap_or(0))
}

/// El *value sanitization algorithm* de cada tipo (§4.10.5.1).
fn sanitize(t: &str, v: &str) -> String {
    match t {
        "text" | "search" | "tel" | "password" => strip_newlines(v),
        "url" | "email" => trim_ascii_ws(&strip_newlines(v)).to_owned(),
        "number" => {
            if is_valid_float(v) {
                v.to_owned()
            } else {
                String::new()
            }
        }
        "range" => {
            // Sin min/max: [0, 100], por defecto 50.
            match parse_float(v).filter(|_| is_valid_float(v)) {
                Some(x) => super::reflect::float_to_string(x.clamp(0.0, 100.0)),
                None => "50".to_owned(),
            }
        }
        "color" => simple_color(v).unwrap_or_else(|| "#000000".to_owned()),
        "date" if !valid_date(v) => String::new(),
        "month" if !valid_month(v) => String::new(),
        "week" if !valid_week(v) => String::new(),
        "time" if !valid_time(v) => String::new(),
        "datetime-local" => match v.split_once(['T', ' ']) {
            Some((d, t)) if valid_date(d) && valid_time(t) => format!("{d}T{t}"),
            _ => String::new(),
        },
        _ => v.to_owned(),
    }
}

/// El valor de un `input` en modo "value": el sucio, o el atributo `value` sanitizado.
fn current_value(agent: &Agent, n: NodeRef) -> String {
    let t = input_type(agent, n);
    let raw = match state(agent, n).value {
        Some(v) => v,
        None => attr_value(agent, n, "value")
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    sanitize(&t, &raw)
}

fn input_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let t = input_type(agent, n);
    let v = match value_mode(&t) {
        "value" => current_value(agent, n),
        "default" => attr_value(agent, n, "value")
            .map(|s| s.to_string())
            .unwrap_or_default(),
        "default/on" => attr_value(agent, n, "value")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "on".to_owned()),
        // `filename`: "C:\fakepath\" + el primer archivo (no hay archivos seleccionados).
        _ => String::new(),
    };
    Ok(js_str(&v))
}

fn set_input_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = webidl::to_dom_string_null_empty(agent, &a.arg(0))?.to_string_lossy();
    let t = input_type(agent, n);
    match value_mode(&t) {
        "value" => {
            let old = current_value(agent, n);
            let new = sanitize(&t, &v);
            let len = new.encode_utf16().count() as u32;
            let st = state_mut(agent, n);
            st.value = Some(new.clone());
            // La selección va al final si el valor cambió (§4.10.5.4).
            if old != new {
                st.selection = Some((len, len, "none"));
            }
        }
        "default" | "default/on" => set_attr(agent, n, "value", DomString::from(v.as_str()))?,
        _ => {
            if !v.is_empty() {
                return throw_dom(agent, DomException::InvalidStateError);
            }
        }
    }
    Ok(Value::Undefined)
}

fn input_checked(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Boolean(with_doc(agent, n.doc, |d| {
        d.checkedness(n.node)
    })))
}

fn set_input_checked(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let b = webidl::to_boolean(agent, &a.arg(0));
    with_doc_mut(agent, n.doc, |d| d.set_checkedness(n.node, b));
    mutated(agent);
    Ok(Value::Undefined)
}

fn get_indeterminate(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Boolean(state(agent, n).indeterminate))
}

fn set_indeterminate(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let b = webidl::to_boolean(agent, &a.arg(0));
    state_mut(agent, n).indeterminate = b;
    Ok(Value::Undefined)
}

/// `files`: `null` salvo en `type=file` (y todavía no hay `FileList`).
fn input_files(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Null)
}

/// `valueAsNumber` (§4.10.5.4): `number` y `range` parsean el valor; los demás tipos dan NaN.
fn value_as_number(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let t = input_type(agent, n);
    let v = match t.as_str() {
        "number" | "range" => parse_float(&current_value(agent, n)).unwrap_or(f64::NAN),
        _ => f64::NAN,
    };
    Ok(Value::Number(v))
}

fn set_value_as_number(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let x = webidl::to_unrestricted_double(agent, &a.arg(0))?;
    if !x.is_finite() {
        return agent.type_error("The value provided is not finite");
    }
    let t = input_type(agent, n);
    if !matches!(t.as_str(), "number" | "range") {
        return throw_dom(agent, DomException::InvalidStateError);
    }
    let s = sanitize(&t, &super::reflect::float_to_string(x));
    state_mut(agent, n).value = Some(s);
    Ok(Value::Undefined)
}

/// `autocomplete` (§4.10.18.7.1): el valor del atributo si es `on`/`off`; vacío si no.
fn autocomplete(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = attr_value(agent, n, "autocomplete")
        .map(|s| trim_ascii_ws(&s.to_string_lossy()).to_ascii_lowercase())
        .unwrap_or_default();
    Ok(js_str(match v.as_str() {
        "on" | "off" => &v,
        _ => "",
    }))
}

fn set_autocomplete(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = webidl::to_dom_string(agent, &a.arg(0))?;
    set_attr(agent, n, "autocomplete", DomString::from_utf16(v.units()))?;
    Ok(Value::Undefined)
}

// --- Selección de texto (§4.10.18.6) ---

/// ¿El tipo de `input` admite la API de selección?
fn selection_applies(agent: &Agent, n: NodeRef) -> bool {
    match html_local_name(agent, n).as_deref() {
        Some("textarea") => true,
        Some("input") => matches!(
            input_type(agent, n).as_str(),
            "text" | "search" | "url" | "tel" | "password"
        ),
        _ => false,
    }
}

fn text_selection(class: HostClass) -> HostClass {
    class
        .attribute("selectionStart", Some(sel_start), Some(set_sel_start))
        .attribute("selectionEnd", Some(sel_end), Some(set_sel_end))
        .attribute(
            "selectionDirection",
            Some(sel_direction),
            Some(set_sel_direction),
        )
        .method("select", 0, select_all)
        .method("setSelectionRange", 2, set_selection_range)
}

/// El valor de la API (para la longitud de la selección).
fn api_value(agent: &Agent, n: NodeRef) -> String {
    if html_local_name(agent, n).as_deref() == Some("textarea") {
        textarea_api_value(agent, n)
    } else {
        current_value(agent, n)
    }
}

fn selection(agent: &Agent, n: NodeRef) -> (u32, u32, &'static str) {
    state(agent, n).selection.unwrap_or_else(|| {
        let len = api_value(agent, n).encode_utf16().count() as u32;
        (len, len, "none")
    })
}

fn sel_start(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    if !selection_applies(agent, n) {
        return Ok(Value::Null);
    }
    Ok(Value::Number(f64::from(selection(agent, n).0)))
}

fn sel_end(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    if !selection_applies(agent, n) {
        return Ok(Value::Null);
    }
    Ok(Value::Number(f64::from(selection(agent, n).1)))
}

fn sel_direction(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    if !selection_applies(agent, n) {
        return Ok(Value::Null);
    }
    Ok(js_str(selection(agent, n).2))
}

/// "Set the selection range" (§4.10.18.6).
fn apply_selection(agent: &mut Agent, n: NodeRef, start: u32, end: u32, dir: &'static str) {
    let len = api_value(agent, n).encode_utf16().count() as u32;
    let end = end.min(len);
    let start = start.min(end);
    state_mut(agent, n).selection = Some((start, end, dir));
}

fn set_sel_start(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    if !selection_applies(agent, n) {
        return throw_dom(agent, DomException::InvalidStateError);
    }
    let s = webidl::to_unsigned_long(agent, &a.arg(0))?;
    let (_, end, dir) = selection(agent, n);
    apply_selection(agent, n, s, end.max(s), dir);
    Ok(Value::Undefined)
}

fn set_sel_end(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    if !selection_applies(agent, n) {
        return throw_dom(agent, DomException::InvalidStateError);
    }
    let e = webidl::to_unsigned_long(agent, &a.arg(0))?;
    let (start, _, dir) = selection(agent, n);
    apply_selection(agent, n, start, e, dir);
    Ok(Value::Undefined)
}

fn direction(s: &str) -> &'static str {
    match s {
        "forward" => "forward",
        "backward" => "backward",
        _ => "none",
    }
}

fn set_sel_direction(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    if !selection_applies(agent, n) {
        return throw_dom(agent, DomException::InvalidStateError);
    }
    let d = webidl::to_dom_string(agent, &a.arg(0))?.to_string_lossy();
    let (start, end, _) = selection(agent, n);
    apply_selection(agent, n, start, end, direction(&d));
    Ok(Value::Undefined)
}

fn select_all(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    if selection_applies(agent, n) {
        let len = api_value(agent, n).encode_utf16().count() as u32;
        apply_selection(agent, n, 0, len, "none");
    }
    Ok(Value::Undefined)
}

fn set_selection_range(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    if !selection_applies(agent, n) {
        return throw_dom(agent, DomException::InvalidStateError);
    }
    let s = webidl::to_unsigned_long(agent, &a.arg(0))?;
    let e = webidl::to_unsigned_long(agent, &a.arg(1))?;
    let d = match a.arg(2) {
        Value::Undefined => String::new(),
        v => webidl::to_dom_string(agent, &v)?.to_string_lossy(),
    };
    apply_selection(agent, n, s, e, direction(&d));
    Ok(Value::Undefined)
}

// --- HTMLTextAreaElement (§4.10.11) ---

/// El *raw value*: el sucio, o el child text content.
fn textarea_raw(agent: &Agent, n: NodeRef) -> String {
    match state(agent, n).value {
        Some(v) => v,
        None => with_doc(agent, n.doc, |d| d.child_text_content(n.node).to_string()),
    }
}

/// El *API value*: el raw value con los saltos de línea normalizados a LF.
fn textarea_api_value(agent: &Agent, n: NodeRef) -> String {
    textarea_raw(agent, n)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

fn textarea_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(js_str(&textarea_api_value(agent, n)))
}

fn set_textarea_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = webidl::to_dom_string_null_empty(agent, &a.arg(0))?.to_string_lossy();
    let old = textarea_api_value(agent, n);
    let new = v.replace("\r\n", "\n").replace('\r', "\n");
    let len = new.encode_utf16().count() as u32;
    let st = state_mut(agent, n);
    st.value = Some(new.clone());
    if old != new {
        st.selection = Some((len, len, "none"));
    }
    Ok(Value::Undefined)
}

fn text_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Number(
        textarea_api_value(agent, n).encode_utf16().count() as f64,
    ))
}

fn child_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = with_doc(agent, n.doc, |d| d.child_text_content(n.node));
    Ok(from_dom_string(&s))
}

/// "String replace all" (los setters de `defaultValue` de `textarea` y `text` de `option`).
fn set_child_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
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

fn set_child_text_all(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    set_child_text(agent, a)
}

fn type_textarea(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(js_str("textarea"))
}

fn type_fieldset(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(js_str("fieldset"))
}

fn type_output(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(js_str("output"))
}

// --- Form owner (§4.10.17.3) ---

/// Los *form-associated elements* que pueden tener form owner.
fn is_form_associated(name: &str) -> bool {
    matches!(
        name,
        "button" | "fieldset" | "input" | "object" | "output" | "select" | "textarea" | "img"
    )
}

/// El *form owner*: el `form` del atributo `form` (si es un id de un `form` del mismo árbol)
/// o el `form` ancestro más cercano.
pub fn form_owner_of(agent: &Agent, n: NodeRef) -> Option<NodeRef> {
    if let Some(id) = attr_value(agent, n, "form") {
        let f = with_doc(agent, n.doc, |d| {
            d.get_element_by_id(&id)
                .filter(|f| d.element(*f).is_some_and(|e| e.is_html("form")))
        });
        return f.map(|f| NodeRef::new(n.doc, f));
    }
    with_doc(agent, n.doc, |d| {
        let mut cur = d.parent(n.node);
        while let Some(p) = cur {
            if d.element(p).is_some_and(|e| e.is_html("form")) {
                return Some(NodeRef::new(n.doc, p));
            }
            cur = d.parent(p);
        }
        None
    })
}

fn form_owner(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    match form_owner_of(agent, n) {
        Some(f) => Ok(wrap_node(agent, f)),
        None => Ok(Value::Null),
    }
}

fn legend_form(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let parent = with_doc(agent, n.doc, |d| {
        d.parent(n.node)
            .filter(|p| d.element(*p).is_some_and(|e| e.is_html("fieldset")))
    });
    match parent.and_then(|p| form_owner_of(agent, NodeRef::new(n.doc, p))) {
        Some(f) => Ok(wrap_node(agent, f)),
        None => Ok(Value::Null),
    }
}

/// Los *listed elements* cuyo form owner es `form`, en orden de árbol.
fn listed_elements(agent: &Agent, form: NodeRef) -> Vec<NodeRef> {
    let root = with_doc(agent, form.doc, |d| {
        d.inclusive_ancestors(form.node).last().copied()
    });
    let Some(root) = root else {
        return Vec::new();
    };
    let candidates = with_doc(agent, form.doc, |d| d.descendants(root));
    candidates
        .into_iter()
        .map(|c| NodeRef::new(form.doc, c))
        .filter(|c| {
            html_local_name(agent, *c).is_some_and(|name| {
                is_form_associated(&name)
                    && name != "img"
                    && !(name == "input" && input_type(agent, *c) == "image")
            }) && form_owner_of(agent, *c) == Some(form)
        })
        .collect()
}

fn form_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Number(listed_elements(agent, n).len() as f64))
}

/// `form.action` (§4.10.18.6): sin atributo (o vacío), la URL del documento.
fn form_action(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = attr_value(agent, n, "action").map(|s| s.to_string_lossy().into_owned());
    let url = match v.as_deref() {
        None | Some("") => with_doc(agent, n.doc, |d| d.url.serialize(false)),
        Some(s) => parse_url(agent, n, s).map_or_else(|| s.to_owned(), |u| u.serialize(false)),
    };
    Ok(js_str(&url))
}

fn set_form_action(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = webidl::to_dom_string(agent, &a.arg(0))?;
    set_attr(agent, n, "action", DomString::from_utf16(v.units()))?;
    Ok(Value::Undefined)
}

/// `submit()` (§4.10.21.3, "submitted from submit() method"): sin eventos; la navegación
/// todavía no se hace desde el motor.
fn form_submit(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Undefined)
}

/// `requestSubmit(submitter)` (§4.10.18.6): valida el submitter y dispara `submit`
/// (cancelable).
fn form_request_submit(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let submitter = a.arg(0);
    if !matches!(submitter, Value::Undefined | Value::Null) {
        let Some(s) = html_element_arg(agent, &submitter) else {
            return agent.type_error("The specified element is not a submit button.");
        };
        let is_submit = match html_local_name(agent, s).as_deref() {
            Some("button") => !matches!(
                attr_value(agent, s, "type")
                    .map(|t| t.to_string_lossy().to_ascii_lowercase())
                    .as_deref(),
                Some("reset" | "button")
            ),
            Some("input") => matches!(input_type(agent, s).as_str(), "submit" | "image"),
            _ => false,
        };
        if !is_submit {
            return agent.type_error("The specified element is not a submit button.");
        }
        if form_owner_of(agent, s) != Some(n) {
            return throw_dom(agent, DomException::NotFoundError);
        }
    }
    submit_form(agent, n);
    Ok(Value::Undefined)
}

/// "Submit" (§4.10.21.3) hasta el evento `submit`.
fn submit_form(agent: &mut Agent, form: NodeRef) {
    let init = EventInit {
        bubbles: true,
        cancelable: true,
        composed: false,
    };
    fire_event(
        agent,
        form.doc,
        EventTarget::Node(form.node),
        "submit",
        init,
        EventKind::Event,
    );
}

/// `reset()` (§4.10.21.6): dispara `reset` (cancelable) y, si no se canceló, el "reset
/// algorithm" de cada control.
fn form_reset(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    reset_form(agent, n);
    Ok(Value::Undefined)
}

fn reset_form(agent: &mut Agent, form: NodeRef) {
    let init = EventInit {
        bubbles: true,
        cancelable: true,
        composed: false,
    };
    if !fire_event(
        agent,
        form.doc,
        EventTarget::Node(form.node),
        "reset",
        init,
        EventKind::Event,
    ) {
        return;
    }
    for c in listed_elements(agent, form) {
        match html_local_name(agent, c).as_deref() {
            Some("input") => {
                let st = state_mut(agent, c);
                st.value = None;
                st.selection = None;
                let checked = attr_value(agent, c, "checked").is_some();
                with_doc_mut(agent, c.doc, |d| d.set_checkedness(c.node, checked));
            }
            Some("textarea") => {
                let st = state_mut(agent, c);
                st.value = None;
                st.selection = None;
            }
            _ => {}
        }
    }
    mutated(agent);
}

// --- Validación (§4.10.20, mínima) ---

/// `willValidate`: los controles no deshabilitados ni de tipo hidden/reset/button.
fn will_validate(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let name = html_local_name(agent, n).unwrap_or_default();
    let barred = attr_value(agent, n, "disabled").is_some()
        || (name == "input"
            && matches!(input_type(agent, n).as_str(), "hidden" | "reset" | "button"))
        || (name == "button"
            && attr_value(agent, n, "type").is_some_and(|t| {
                matches!(
                    t.to_string_lossy().to_ascii_lowercase().as_str(),
                    "reset" | "button"
                )
            }))
        || (matches!(name.as_str(), "input" | "textarea")
            && attr_value(agent, n, "readonly").is_some());
    Ok(Value::Boolean(!barred))
}

/// `checkValidity()` con los estados `valueMissing` y `customError`.
fn check_validity(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let st = state(agent, n);
    if !st.custom_validity.is_empty() {
        return Ok(Value::Boolean(false));
    }
    let name = html_local_name(agent, n).unwrap_or_default();
    let required = attr_value(agent, n, "required").is_some();
    let missing = required
        && match name.as_str() {
            "input" => match input_type(agent, n).as_str() {
                "checkbox" | "radio" => !with_doc(agent, n.doc, |d| d.checkedness(n.node)),
                _ => current_value(agent, n).is_empty(),
            },
            "textarea" => textarea_api_value(agent, n).is_empty(),
            _ => false,
        };
    Ok(Value::Boolean(!missing))
}

fn set_custom_validity(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = webidl::to_dom_string(agent, &a.arg(0))?.to_string_lossy();
    state_mut(agent, n).custom_validity = s;
    Ok(Value::Undefined)
}

fn validation_message(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(js_str(&state(agent, n).custom_validity))
}

// --- HTMLSelectElement y HTMLOptionElement (§4.10.7, §4.10.10) ---

fn options(agent: &Agent, select: NodeRef) -> Vec<NodeRef> {
    with_doc(agent, select.doc, |d| d.list_of_options(select.node))
        .into_iter()
        .map(|o| NodeRef::new(select.doc, o))
        .collect()
}

fn is_multiple(agent: &Agent, n: NodeRef) -> bool {
    attr_value(agent, n, "multiple").is_some()
}

fn select_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(js_str(if is_multiple(agent, n) {
        "select-multiple"
    } else {
        "select-one"
    }))
}

fn select_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Number(options(agent, n).len() as f64))
}

fn selected_index(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let opts = options(agent, n);
    let i = opts
        .iter()
        .position(|o| with_doc(agent, o.doc, |d| d.option_selectedness(o.node)));
    Ok(Value::Number(i.map_or(-1.0, |i| i as f64)))
}

/// Marca la selectedness de `option` (con dirtiness) y, si el `select` no es múltiple,
/// desmarca las demás.
fn select_option(agent: &mut Agent, select: NodeRef, option: Option<NodeRef>) {
    for o in options(agent, select) {
        let on = Some(o) == option;
        with_doc_mut(agent, o.doc, |d| d.set_option_selected(o.node, on));
    }
    mutated(agent);
}

fn set_selected_index(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let i = webidl::to_long(agent, &a.arg(0))?;
    let opts = options(agent, n);
    let target = usize::try_from(i).ok().and_then(|i| opts.get(i).copied());
    select_option(agent, n, target);
    Ok(Value::Undefined)
}

fn select_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let first = options(agent, n)
        .into_iter()
        .find(|o| with_doc(agent, o.doc, |d| d.option_selectedness(o.node)));
    Ok(js_str(
        &first.map(|o| option_value_of(agent, o)).unwrap_or_default(),
    ))
}

fn set_select_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = webidl::to_dom_string(agent, &a.arg(0))?.to_string_lossy();
    let target = options(agent, n)
        .into_iter()
        .find(|o| option_value_of(agent, *o) == v);
    select_option(agent, n, target);
    Ok(Value::Undefined)
}

fn select_item(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let i = webidl::to_unsigned_long(agent, &a.arg(0))? as usize;
    let o = options(agent, n).get(i).copied();
    Ok(wrap_opt(agent, n.doc, o.map(|o| o.node)))
}

/// `option.text` (§4.10.10): el texto de los descendientes (sin los de `script` ni SVG
/// `script`) con el ASCII whitespace quitado y colapsado.
fn option_text_of(agent: &Agent, o: NodeRef) -> String {
    let raw = with_doc(agent, o.doc, |d| {
        let mut s = String::new();
        for id in d.descendants(o.node) {
            if let NodeData::Text(t) = &d.get(id).data {
                let in_script = d.inclusive_ancestors(id).iter().any(|a| {
                    *a != o.node
                        && d.element(*a).is_some_and(|e| {
                            e.name.to_string() == "script"
                                && matches!(e.ns, Namespace::Html | Namespace::Svg)
                        })
                });
                if !in_script {
                    s.push_str(&t.to_string());
                }
            }
        }
        s
    });
    raw.split(|c: char| matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' '))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn option_value_of(agent: &Agent, o: NodeRef) -> String {
    match attr_value(agent, o, "value") {
        Some(v) => v.to_string(),
        None => option_text_of(agent, o),
    }
}

fn option_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(js_str(&option_text_of(agent, n)))
}

fn option_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(js_str(&option_value_of(agent, n)))
}

fn set_option_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = webidl::to_dom_string(agent, &a.arg(0))?;
    set_attr(agent, n, "value", DomString::from_utf16(v.units()))?;
    Ok(Value::Undefined)
}

fn option_label(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = match attr_value(agent, n, "label") {
        Some(v) => v.to_string(),
        None => option_text_of(agent, n),
    };
    Ok(js_str(&v))
}

fn set_option_label(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = webidl::to_dom_string(agent, &a.arg(0))?;
    set_attr(agent, n, "label", DomString::from_utf16(v.units()))?;
    Ok(Value::Undefined)
}

fn option_selected(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Boolean(with_doc(agent, n.doc, |d| {
        d.option_selectedness(n.node)
    })))
}

/// `option.selected = b` (§4.10.10): selectedness con dirtiness y, en un `select` simple,
/// "ask for a reset" de los demás.
fn set_option_selected(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let b = webidl::to_boolean(agent, &a.arg(0));
    with_doc_mut(agent, n.doc, |d| d.set_option_selected(n.node, b));
    mutated(agent);
    Ok(Value::Undefined)
}

fn option_select(agent: &Agent, o: NodeRef) -> Option<NodeRef> {
    with_doc(agent, o.doc, |d| {
        let mut cur = d.parent(o.node);
        while let Some(p) = cur {
            if d.element(p).is_some_and(|e| e.is_html("select")) {
                return Some(NodeRef::new(o.doc, p));
            }
            if !d
                .element(p)
                .is_some_and(|e| e.is_html("optgroup") || e.is_html("datalist"))
            {
                return None;
            }
            cur = d.parent(p);
        }
        None
    })
}

fn option_index(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let i = option_select(agent, n)
        .and_then(|s| options(agent, s).iter().position(|o| *o == n))
        .unwrap_or(0);
    Ok(Value::Number(i as f64))
}

fn option_form(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    match option_select(agent, n).and_then(|s| form_owner_of(agent, s)) {
        Some(f) => Ok(wrap_node(agent, f)),
        None => Ok(Value::Null),
    }
}

// --- HTMLLabelElement (§4.10.4) ---

fn is_labelable(agent: &Agent, n: NodeRef) -> bool {
    match html_local_name(agent, n).as_deref() {
        Some("button" | "meter" | "output" | "progress" | "select" | "textarea") => true,
        Some("input") => input_type(agent, n) != "hidden",
        _ => false,
    }
}

fn label_control_of(agent: &Agent, n: NodeRef) -> Option<NodeRef> {
    if let Some(id) = attr_value(agent, n, "for") {
        let e = with_doc(agent, n.doc, |d| d.get_element_by_id(&id))?;
        let e = NodeRef::new(n.doc, e);
        return is_labelable(agent, e).then_some(e);
    }
    let desc = with_doc(agent, n.doc, |d| d.descendants(n.node));
    desc.into_iter()
        .map(|c| NodeRef::new(n.doc, c))
        .find(|c| is_labelable(agent, *c))
}

fn label_control(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    match label_control_of(agent, n) {
        Some(c) => Ok(wrap_node(agent, c)),
        None => Ok(Value::Null),
    }
}

fn label_form(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    match label_control_of(agent, n).and_then(|c| form_owner_of(agent, c)) {
        Some(f) => Ok(wrap_node(agent, f)),
        None => Ok(Value::Null),
    }
}

// --- HTMLOutputElement (§4.10.12) ---

fn output_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let s = with_doc(agent, n.doc, |d| d.text_content(n.node));
    Ok(js_str(&s))
}

fn set_output_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    // El *default value override* toma el valor actual la primera vez.
    if state(agent, n).value.is_none() {
        let cur = with_doc(agent, n.doc, |d| d.text_content(n.node));
        state_mut(agent, n).value = Some(cur);
    }
    set_child_text(agent, a)
}

fn output_default(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    match state(agent, n).value {
        Some(v) => Ok(js_str(&v)),
        None => {
            let s = with_doc(agent, n.doc, |d| d.text_content(n.node));
            Ok(js_str(&s))
        }
    }
}

fn set_output_default(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    if state(agent, n).value.is_some() {
        let s = webidl::to_dom_string(agent, &a.arg(0))?.to_string_lossy();
        state_mut(agent, n).value = Some(s);
        return Ok(Value::Undefined);
    }
    set_child_text(agent, a)
}

// --- HTMLMeterElement (§4.10.14) y HTMLProgressElement (§4.10.13) ---

fn float_attr(agent: &Agent, n: NodeRef, name: &str) -> Option<f64> {
    attr_value(agent, n, name).and_then(|s| parse_float(&s.to_string_lossy()))
}

/// Los valores de `meter` (§4.10.14: "the minimum value", "the maximum value", …).
fn meter_values(agent: &Agent, n: NodeRef) -> [f64; 6] {
    let min = float_attr(agent, n, "min").unwrap_or(0.0);
    let max = float_attr(agent, n, "max").unwrap_or(1.0).max(min);
    let value = float_attr(agent, n, "value").unwrap_or(0.0).clamp(min, max);
    let low = float_attr(agent, n, "low").unwrap_or(min).clamp(min, max);
    let high = float_attr(agent, n, "high").unwrap_or(max).clamp(low, max);
    let optimum = float_attr(agent, n, "optimum")
        .unwrap_or(min + (max - min) / 2.0)
        .clamp(min, max);
    [value, min, max, low, high, optimum]
}

macro_rules! meter_getter {
    ($name:ident, $i:expr) => {
        fn $name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
            let n = this_node(agent, a)?;
            Ok(Value::Number(meter_values(agent, n)[$i]))
        }
    };
}
meter_getter!(meter_value, 0);
meter_getter!(meter_min, 1);
meter_getter!(meter_max, 2);
meter_getter!(meter_low, 3);
meter_getter!(meter_high, 4);
meter_getter!(meter_optimum, 5);

/// Los setters de `meter` (los atributos `double` reflejados con getter propio).
fn set_meter(name: &'static str) -> bf_js::vm::function::NativeFn {
    match name {
        "value" => |agent, a| set_double_attr(agent, a, "value"),
        "min" => |agent, a| set_double_attr(agent, a, "min"),
        "max" => |agent, a| set_double_attr(agent, a, "max"),
        "low" => |agent, a| set_double_attr(agent, a, "low"),
        "high" => |agent, a| set_double_attr(agent, a, "high"),
        _ => |agent, a| set_double_attr(agent, a, "optimum"),
    }
}

fn set_double_attr(agent: &mut Agent, a: &CallArgs<'_>, name: &str) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let x = webidl::to_double(agent, &a.arg(0))?;
    set_attr(
        agent,
        n,
        name,
        DomString::from(super::reflect::float_to_string(x).as_str()),
    )?;
    Ok(Value::Undefined)
}

/// `progress.value`: el atributo si es un número ≥ 0, acotado a `max`; 0 si es indeterminado.
fn progress_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let max = float_attr(agent, n, "max")
        .filter(|m| *m > 0.0)
        .unwrap_or(1.0);
    let v = float_attr(agent, n, "value").filter(|v| *v >= 0.0);
    Ok(Value::Number(v.map_or(0.0, |v| v.min(max))))
}

fn set_progress_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    set_double_attr(agent, a, "value")
}

/// `progress.position`: -1 si es indeterminado; si no, valor / máximo.
fn progress_position(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let max = float_attr(agent, n, "max")
        .filter(|m| *m > 0.0)
        .unwrap_or(1.0);
    match float_attr(agent, n, "value").filter(|v| *v >= 0.0) {
        None => Ok(Value::Number(-1.0)),
        Some(v) => Ok(Value::Number(v.min(max) / max)),
    }
}
