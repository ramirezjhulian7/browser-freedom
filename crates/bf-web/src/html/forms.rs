//! Los formularios (HTML §4.10): `HTMLInputElement` (el valor con su *dirty value flag* y la
//! sanitización por tipo, checkedness, selección), `HTMLTextAreaElement`, `HTMLSelectElement`,
//! `HTMLOptionElement`, `HTMLButtonElement`, `HTMLFormElement` (`submit`, `requestSubmit`,
//! `reset`, `action`), `HTMLLabelElement`, `HTMLFieldSetElement`, `HTMLOutputElement`,
//! `HTMLLegendElement`, `HTMLMeterElement` y `HTMLProgressElement`.
//!
//! El estado de los controles que no guarda `bf-dom` (el valor, su dirty flag, la selección
//! de texto, `indeterminate`) vive en [`ControlStates`], por nodo.

use std::collections::HashMap;

use bf_dom::events::EventInit;
use bf_dom::{DomException, Namespace, NodeData};
use bf_infra::DomString;
use bf_js::host::{HostClass, HostClassId, webidl};
use bf_js::string::JsString;
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use super::elements::{
    attr_value, html_element_arg, html_local_name, live_collection, parse_url, set_attr,
};
use super::reflect::parse_float;
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
        "HTMLInputElement" => popover_invoker(text_selection(
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
                .attribute("width", Some(zero), Some(set_input_width))
                .attribute("height", Some(zero), Some(set_input_height))
                .attribute(
                    "formAction",
                    Some(form_action_attr),
                    Some(set_form_action_attr),
                )
                // E1: lo que faltaba de HTMLInputElement (§4.10.5).
                .attribute("list", Some(input_list), None)
                .attribute("valueAsDate", Some(value_as_date), Some(set_value_as_date))
                .method("stepUp", 0, step_up)
                .method("stepDown", 0, step_down)
                .method("showPicker", 0, show_picker)
                .attribute("colorSpace", Some(color_space), Some(set_color_space))
                .attribute("alpha", Some(get_alpha), Some(set_alpha))
                .attribute("labels", Some(get_labels), None),
        ))
        .attribute("willValidate", Some(will_validate), None)
        .method("checkValidity", 0, check_validity)
        .method("reportValidity", 0, check_validity)
        .method("setCustomValidity", 1, set_custom_validity)
        .attribute("validationMessage", Some(validation_message), None)
        .attribute("validity", Some(get_validity), None),
        "HTMLTextAreaElement" => text_selection(
            class
                .attribute("value", Some(textarea_value), Some(set_textarea_value))
                .attribute("defaultValue", Some(child_text), Some(set_child_text))
                .attribute("textLength", Some(text_length), None)
                .attribute("type", Some(type_textarea), None)
                .attribute("form", Some(form_owner), None)
                .attribute("autocomplete", Some(autocomplete), Some(set_autocomplete))
                .attribute("labels", Some(get_labels), None),
        )
        .attribute("willValidate", Some(will_validate), None)
        .method("checkValidity", 0, check_validity)
        .method("reportValidity", 0, check_validity)
        .method("setCustomValidity", 1, set_custom_validity)
        .attribute("validationMessage", Some(validation_message), None)
        .attribute("validity", Some(get_validity), None),
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
            .attribute("labels", Some(get_labels), None)
            .attribute("willValidate", Some(will_validate), None)
            .method("checkValidity", 0, check_validity)
            .method("reportValidity", 0, check_validity)
            .method("setCustomValidity", 1, set_custom_validity)
            .attribute("validationMessage", Some(validation_message), None)
            .attribute("validity", Some(get_validity), None)
            .method("item", 1, select_item)
            .attribute("options", Some(select_options_attr), None)
            .attribute("selectedOptions", Some(selected_options_attr), None),
        "HTMLOptionElement" => class
            .attribute("text", Some(option_text), Some(set_child_text_all))
            .attribute("value", Some(option_value), Some(set_option_value))
            .attribute("label", Some(option_label), Some(set_option_label))
            .attribute("selected", Some(option_selected), Some(set_option_selected))
            .attribute("index", Some(option_index), None)
            .attribute("form", Some(option_form), None),
        "HTMLButtonElement" => popover_invoker(class)
            .attribute("form", Some(form_owner), None)
            .attribute("labels", Some(get_labels), None)
            .attribute(
                "formAction",
                Some(form_action_attr),
                Some(set_form_action_attr),
            )
            .attribute("willValidate", Some(will_validate), None)
            .method("checkValidity", 0, check_validity)
            .method("reportValidity", 0, check_validity)
            .method("setCustomValidity", 1, set_custom_validity)
            .attribute("validationMessage", Some(validation_message), None)
            .attribute("validity", Some(get_validity), None),
        "HTMLFormElement" => class
            .attribute("action", Some(form_action), Some(set_form_action))
            .attribute("length", Some(form_length), None)
            .attribute("elements", Some(form_elements_attr), None)
            .method("submit", 0, form_submit)
            .method("requestSubmit", 0, form_request_submit)
            .method("reset", 0, form_reset)
            .method("checkValidity", 0, form_check_validity)
            .method("reportValidity", 0, form_check_validity),
        "HTMLLabelElement" => class
            .attribute("control", Some(label_control), None)
            .attribute("form", Some(label_form), None),
        "HTMLFieldSetElement" => class
            .attribute("type", Some(type_fieldset), None)
            .attribute("elements", Some(fieldset_elements_attr), None)
            .attribute("form", Some(form_owner), None)
            .attribute("willValidate", Some(false_value), None)
            .method("checkValidity", 0, check_validity)
            .method("reportValidity", 0, check_validity)
            .method("setCustomValidity", 1, set_custom_validity)
            .attribute("validationMessage", Some(validation_message), None)
            .attribute("validity", Some(get_validity), None),
        "HTMLOutputElement" => class
            .attribute("type", Some(type_output), None)
            .attribute("value", Some(output_value), Some(set_output_value))
            .attribute(
                "defaultValue",
                Some(output_default),
                Some(set_output_default),
            )
            .attribute("form", Some(form_owner), None)
            .attribute("labels", Some(get_labels), None)
            .attribute("willValidate", Some(false_value), None)
            .method("checkValidity", 0, check_validity)
            .method("reportValidity", 0, check_validity)
            .method("setCustomValidity", 1, set_custom_validity)
            .attribute("validationMessage", Some(validation_message), None)
            .attribute("validity", Some(get_validity), None),
        "HTMLLegendElement" => class.attribute("form", Some(legend_form), None),
        "HTMLObjectElement" => class.attribute("form", Some(form_owner), None),
        "HTMLMeterElement" => class
            .attribute("value", Some(meter_value), Some(set_meter("value")))
            .attribute("min", Some(meter_min), Some(set_meter("min")))
            .attribute("max", Some(meter_max), Some(set_meter("max")))
            .attribute("low", Some(meter_low), Some(set_meter("low")))
            .attribute("high", Some(meter_high), Some(set_meter("high")))
            .attribute("optimum", Some(meter_optimum), Some(set_meter("optimum")))
            .attribute("labels", Some(get_labels), None),
        "HTMLProgressElement" => class
            .attribute("value", Some(progress_value), Some(set_progress_value))
            .attribute("position", Some(progress_position), None)
            .attribute("labels", Some(get_labels), None),
        _ => class,
    }
}

fn zero(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(0.0))
}

/// `width`/`height` de `input` (§4.10.5.1.20): sin imagen, 0; el setter refleja el
/// atributo como `unsigned long`.
fn set_dimension(agent: &mut Agent, a: &CallArgs<'_>, name: &str) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = webidl::to_unsigned_long(agent, &a.arg(0))?;
    let v = if v <= 2147483647 { v } else { 0 };
    set_attr(agent, n, name, DomString::from(v.to_string().as_str()))?;
    Ok(Value::Undefined)
}

fn set_input_width(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    set_dimension(agent, a, "width")
}

fn set_input_height(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    set_dimension(agent, a, "height")
}

/// `formAction` (§4.10.18.6): sin atributo (o vacío), la URL del documento; si no, el
/// atributo parseado (o tal cual si no parsea).
fn form_action_attr(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = attr_value(agent, n, "formaction").map(|s| s.to_string_lossy().into_owned());
    let url = match v.as_deref() {
        None | Some("") => with_doc(agent, n.doc, |d| d.url.serialize(false)),
        Some(s) => parse_url(agent, n, s).map_or_else(|| s.to_owned(), |u| u.serialize(false)),
    };
    Ok(js_str(&url))
}

fn set_form_action_attr(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = webidl::to_dom_string(agent, &a.arg(0))?;
    set_attr(agent, n, "formaction", DomString::from_utf16(v.units()))?;
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
        _ if (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400) => 29,
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
        // E1: §4.10.18.6.
        .method("setRangeText", 1, set_range_text)
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

/// "Set the selection range" (§4.10.18.6). E1: si la selección cambió, se encola la tarea que
/// dispara `select` en el elemento (una sola pendiente por elemento).
fn apply_selection(agent: &mut Agent, n: NodeRef, start: u32, end: u32, dir: &'static str) {
    let len = api_value(agent, n).encode_utf16().count() as u32;
    let end = end.min(len);
    let start = start.min(end);
    let old = selection(agent, n);
    state_mut(agent, n).selection = Some((start, end, dir));
    if old != (start, end, dir) {
        queue_select_event(agent, n);
    }
}

/// E1: los elementos con un `select` encolado.
#[derive(Debug, Default)]
struct PendingSelect(std::collections::HashSet<NodeRef>);

/// E1: "queue an element task on the user interaction task source given the element to fire
/// an event named `select` at the element, with `bubbles` true" (§4.10.18.6).
fn queue_select_event(agent: &mut Agent, n: NodeRef) {
    let realm = WebContextHandle::current(agent).realm;
    if agent.realm_data::<PendingSelect>(realm).is_none() {
        agent.set_realm_data(realm, PendingSelect::default());
    }
    let pending = agent
        .realm_data_mut::<PendingSelect>(realm)
        .expect("PendingSelect");
    if !pending.0.insert(n) {
        return;
    }
    agent.queue_task(
        bf_js::host::TaskSource::UserInteraction,
        Box::new(move |agent: &mut Agent| {
            let saved = agent.current_realm;
            agent.current_realm = realm;
            if let Some(p) = agent.realm_data_mut::<PendingSelect>(realm) {
                p.0.remove(&n);
            }
            let init = EventInit {
                bubbles: true,
                ..EventInit::default()
            };
            super::submission::fire_simple(agent, n, "select", init);
            agent.current_realm = saved;
        }),
    );
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
    crate::dom::events::required(agent, a, 2, "setSelectionRange")?; // E1
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

/// ¿Es un *form-associated custom element* (E1)?
pub(super) fn is_face(agent: &Agent, n: NodeRef) -> bool {
    with_doc(agent, n.doc, |d| {
        d.is_form_associated_custom_element(n.node)
    })
}

/// El *form owner*: el `form` del atributo `form` (si es un id de un `form` del mismo árbol)
/// o el `form` ancestro más cercano.
pub fn form_owner_of(agent: &Agent, n: NodeRef) -> Option<NodeRef> {
    // "Reset the form owner" (HTML §4.10.17.3), paso 3: el atributo `form` solo cuenta si el
    // elemento está conectado; si no, el `form` ancestro.
    if let Some(id) = attr_value(agent, n, "form")
        && with_doc(agent, n.doc, |d| d.is_connected(n.node))
    {
        let f = with_doc(agent, n.doc, |d| {
            d.get_element_by_id(&id)
                .filter(|f| d.element(*f).is_some_and(|e| e.is_html("form")))
        });
        return f.map(|f| NodeRef::new(n.doc, f));
    }
    // E1: el form que asoció el parser (*parser inserted flag*).
    if let Some(f) = with_doc(agent, n.doc, |d| d.parser_form_owner(n.node)) {
        return Some(NodeRef::new(n.doc, f));
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
            (html_local_name(agent, *c).is_some_and(|name| {
                is_form_associated(&name)
                    && name != "img"
                    && !(name == "input" && input_type(agent, *c) == "image")
            }) || is_face(agent, *c))
                && form_owner_of(agent, *c) == Some(form)
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

/// `submit()` (§4.10.21.3, "submitted from submit() method"): sin eventos ni validación.
fn form_submit(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    super::submission::submit(agent, n, n, true);
    Ok(Value::Undefined)
}

/// `requestSubmit(submitter)` (§4.10.18.6): valida el submitter y envía con eventos.
fn form_request_submit(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let submitter = a.arg(0);
    let mut from = n;
    if !matches!(submitter, Value::Undefined | Value::Null) {
        let Some(s) = html_element_arg(agent, &submitter) else {
            return agent.type_error("The specified element is not a submit button.");
        };
        if !super::submission::is_submit_button(agent, s) {
            return agent.type_error("The specified element is not a submit button.");
        }
        if form_owner_of(agent, s) != Some(n) {
            return throw_dom(agent, DomException::NotFoundError);
        }
        from = s;
    }
    super::submission::submit(agent, n, from, false);
    Ok(Value::Undefined)
}

/// `reset()` (§4.10.21.6): dispara `reset` (cancelable) y, si no se canceló, el "reset
/// algorithm" de cada control.
fn form_reset(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    reset_form(agent, n, false);
    Ok(Value::Undefined)
}

/// "Reset" (§4.10.21.6): `reset` (cancelable, de confianza) y el "reset algorithm" de cada
/// control. `from_activation` (botón de reset, fuera de `[CEReactions]`): los
/// `formResetCallback` van a la *backup element queue* y corren en un microtask (§4.13.6).
pub(super) fn reset_form(agent: &mut Agent, form: NodeRef, from_activation: bool) {
    let realm = WebContextHandle::current(agent).realm;
    // "Locked for reset".
    let locked = agent
        .realm_data::<ResetLocks>(realm)
        .is_some_and(|l| l.0.contains(&form));
    if locked {
        return;
    }
    if agent.realm_data::<ResetLocks>(realm).is_none() {
        agent.set_realm_data(realm, ResetLocks::default());
    }
    if let Some(l) = agent.realm_data_mut::<ResetLocks>(realm) {
        l.0.push(form);
    }
    let init = EventInit {
        bubbles: true,
        cancelable: true,
        composed: false,
    };
    if super::submission::fire_simple(agent, form, "reset", init) {
        for c in listed_elements(agent, form) {
            match html_local_name(agent, c).as_deref() {
                Some("input" | "textarea") => {
                    let st = state_mut(agent, c);
                    st.value = None;
                    st.selection = None;
                    with_doc_mut(agent, c.doc, |d| d.reset_form_control(c.node));
                }
                Some("select") => {
                    with_doc_mut(agent, c.doc, |d| d.reset_form_control(c.node));
                }
                // E1: el "reset algorithm" de `output` (§4.10.12): el contenido pasa a ser el
                // *default value* y el *default value override* vuelve a null.
                Some("output") => {
                    let default = state_mut(agent, c)
                        .value
                        .take()
                        .unwrap_or_else(|| with_doc(agent, c.doc, |d| d.text_content(c.node)));
                    let s = DomString::from(default.as_str());
                    with_doc_mut(agent, c.doc, |d| {
                        let node = (!s.is_empty()).then(|| d.create_text_node(s));
                        d.replace_all(node, c.node);
                    });
                }
                // E1: el "reset algorithm" de un FACE encola `formResetCallback`.
                _ if is_face(agent, c) => {
                    super::custom_elements::enqueue_form_reset(agent, c, from_activation);
                }
                _ => {}
            }
        }
        mutated(agent);
    }
    if let Some(l) = agent.realm_data_mut::<ResetLocks>(realm) {
        l.0.retain(|f| *f != form);
    }
}

/// Los formularios con el *locked for reset flag*.
#[derive(Debug, Default)]
struct ResetLocks(Vec<NodeRef>);

// --- Validación (§4.10.20, mínima) ---

/// `willValidate`: los controles no deshabilitados ni de tipo hidden/reset/button.
fn will_validate(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Boolean(will_validate_node(agent, n)))
}

/// ¿Es candidato a la validación de restricciones (§4.10.20.1)?
pub(super) fn will_validate_node(agent: &Agent, n: NodeRef) -> bool {
    let name = html_local_name(agent, n).unwrap_or_default();
    let face = is_face(agent, n);
    if !face && !matches!(name.as_str(), "input" | "textarea" | "select" | "button") {
        return false;
    }
    let in_datalist = with_doc(agent, n.doc, |d| {
        d.inclusive_ancestors(n.node)
            .iter()
            .any(|a| is_html_el(d, *a, "datalist"))
    });
    let barred = in_datalist
        || with_doc(agent, n.doc, |d| d.is_disabled(n.node))
        || (name == "input"
            && matches!(input_type(agent, n).as_str(), "hidden" | "reset" | "button"))
        || (name == "button"
            && attr_value(agent, n, "type").is_some_and(|t| {
                matches!(
                    t.to_string_lossy().to_ascii_lowercase().as_str(),
                    "reset" | "button"
                )
            }))
        || ((face || matches!(name.as_str(), "input" | "textarea"))
            && attr_value(agent, n, "readonly").is_some());
    !barred
}

/// `checkValidity()` (§4.10.20.3): si es candidato y no satisface sus restricciones,
/// dispara `invalid` (cancelable) y devuelve `false`.
fn check_validity(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Boolean(check_node(agent, n)))
}

pub(super) fn check_node(agent: &mut Agent, n: NodeRef) -> bool {
    if !will_validate_node(agent, n) || satisfies_constraints(agent, n) {
        return true;
    }
    let init = EventInit {
        cancelable: true,
        ..EventInit::default()
    };
    super::submission::fire_simple(agent, n, "invalid", init);
    false
}

/// `form.checkValidity()`: valida cada control del formulario ("statically validate").
fn form_check_validity(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let mut valid = true;
    for c in listed_elements(agent, n) {
        if !check_node(agent, c) {
            valid = false;
        }
    }
    Ok(Value::Boolean(valid))
}

/// ¿Satisface sus restricciones? (Los estados `valueMissing` y `customError`.)
/// ¿Satisface sus restricciones (§4.10.20.1)? E1: ningún estado de `ValidityState`.
pub(super) fn satisfies_constraints(agent: &mut Agent, n: NodeRef) -> bool {
    !flags(agent, n).any()
}

// --- Para el envío (`submission`) ---

pub(super) fn input_current_value(agent: &Agent, n: NodeRef) -> String {
    current_value(agent, n)
}

pub(super) fn textarea_value_of(agent: &Agent, n: NodeRef) -> String {
    textarea_api_value(agent, n)
}

pub(super) fn select_options(agent: &Agent, n: NodeRef) -> Vec<NodeRef> {
    options(agent, n)
}

pub(super) fn option_value_str(agent: &Agent, o: NodeRef) -> String {
    option_value_of(agent, o)
}

pub(super) fn labeled_control(agent: &Agent, label: NodeRef) -> Option<NodeRef> {
    label_control_of(agent, label)
}

fn set_custom_validity(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    crate::dom::events::required(agent, a, 1, "setCustomValidity")?; // E1
    let s = webidl::to_dom_string(agent, &a.arg(0))?.to_string_lossy();
    state_mut(agent, n).custom_validity = s;
    Ok(Value::Undefined)
}

fn validation_message(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    // E1: el mensaje del error propio, o uno por estado de validez (§4.10.20.3).
    let m = validation_message_of(agent, n);
    Ok(js_str(&m))
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
                            e.name == "script" && matches!(e.ns, Namespace::Html | Namespace::Svg)
                        })
                });
                if !in_script {
                    s.push_str(&t.to_string());
                }
            }
        }
        s
    });
    raw.split(['\t', '\n', '\x0C', '\r', ' '])
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

pub(super) fn is_labelable(agent: &Agent, n: NodeRef) -> bool {
    if is_face(agent, n) {
        return true;
    }
    match html_local_name(agent, n).as_deref() {
        Some("button" | "meter" | "output" | "progress" | "select" | "textarea") => true,
        Some("input") => input_type(agent, n) != "hidden",
        _ => false,
    }
}

pub(super) fn label_control_of(agent: &Agent, n: NodeRef) -> Option<NodeRef> {
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

// --- Colecciones (sobre el `Document`, sin el agente) ---

fn is_html_el(d: &bf_dom::Document, n: bf_dom::NodeId, name: &str) -> bool {
    d.element(n).is_some_and(|e| e.is_html(name))
}

/// ¿Es un *listed element* (§4.10.2)? (`input type=image` no entra en `elements`.)
fn is_listed_doc(d: &bf_dom::Document, n: bf_dom::NodeId) -> bool {
    // E1: los *form-associated custom elements* (HTML §4.13.7).
    if d.is_form_associated_custom_element(n) {
        return true;
    }
    let Some(e) = d.element(n) else { return false };
    if e.ns != Namespace::Html {
        return false;
    }
    match e.name.to_string().as_str() {
        "button" | "fieldset" | "object" | "output" | "select" | "textarea" => true,
        "input" => !e
            .attr("type")
            .is_some_and(|t| t.eq_ignore_ascii_case("image")),
        _ => false,
    }
}

/// El form owner calculado sobre el documento (como [`form_owner_of`]).
fn form_owner_doc(d: &bf_dom::Document, n: bf_dom::NodeId) -> Option<bf_dom::NodeId> {
    if let Some(id) = d
        .element(n)
        .and_then(|e| e.attr("form"))
        .map(DomString::from)
        && d.is_connected(n)
    {
        return d
            .get_element_by_id(&id)
            .filter(|f| is_html_el(d, *f, "form"));
    }
    if let Some(f) = d.parser_form_owner(n) {
        return Some(f);
    }
    let mut cur = d.parent(n);
    while let Some(p) = cur {
        if is_html_el(d, p, "form") {
            return Some(p);
        }
        cur = d.parent(p);
    }
    None
}

/// `form.elements` (§4.10.3): los listed elements cuyo form owner es el form, en orden de
/// árbol.
fn form_elements_list(d: &bf_dom::Document, form: bf_dom::NodeId) -> Vec<bf_dom::NodeId> {
    let root = *d.inclusive_ancestors(form).last().unwrap_or(&form);
    d.descendants(root)
        .into_iter()
        .filter(|c| is_listed_doc(d, *c) && form_owner_doc(d, *c) == Some(form))
        .collect()
}

/// `fieldset.elements` (§4.10.15): los listed elements descendientes.
fn fieldset_elements_list(d: &bf_dom::Document, fs: bf_dom::NodeId) -> Vec<bf_dom::NodeId> {
    d.descendants(fs)
        .into_iter()
        .filter(|c| *c != fs && is_listed_doc(d, *c))
        .collect()
}

fn select_options_list(d: &bf_dom::Document, s: bf_dom::NodeId) -> Vec<bf_dom::NodeId> {
    d.list_of_options(s)
}

fn selected_options_list(d: &bf_dom::Document, s: bf_dom::NodeId) -> Vec<bf_dom::NodeId> {
    d.list_of_options(s)
        .into_iter()
        .filter(|o| d.option_selectedness(*o))
        .collect()
}

fn form_elements_attr(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    // E1: `form.elements` es un `HTMLFormControlsCollection` (§2.7.2.2).
    let realm = agent.current_realm;
    let key = bf_js::host::HostKey(n.key().0 ^ (5 << 52) | (1 << 62));
    if let Some(o) = agent.host_lookup(realm, key) {
        return Ok(Value::Object(o));
    }
    let class = collection_classes(agent).controls;
    let coll = bf_dom::collections::HtmlCollection::new(
        n.node,
        bf_dom::collections::CollectionFilter::Computed(bf_dom::collections::ComputedList(
            form_elements_list,
        )),
    );
    Ok(Value::Object(agent.host_wrap(
        realm,
        key,
        class,
        Box::new(crate::dom::collections::HtmlCollectionData { doc: n.doc, coll }),
    )))
}

// --- HTMLFormControlsCollection y RadioNodeList (§2.7.2.2, §2.7.2.3), E1 ---

/// Las clases `HTMLFormControlsCollection` y `RadioNodeList`.
#[derive(Debug, Clone, Copy)]
pub struct FormCollectionClasses {
    pub controls: HostClassId,
    pub radio_list: HostClassId,
}

pub fn register_collections(agent: &mut Agent) {
    if agent.embedder_data::<FormCollectionClasses>().is_some() {
        return;
    }
    use crate::dom::collections as c;
    let dc = crate::dom::classes(agent).clone();
    let controls = agent.register_host_class(
        HostClass::new("HTMLFormControlsCollection")
            .parent(dc.html_collection)
            .method("namedItem", 1, fc_named_item)
            .exotic(bf_js::host::HostExotic {
                length: Some(c::hc_len),
                item: Some(c::hc_at),
                named: Some(fc_named),
                names: Some(c::hc_names),
                named_enumerable: false,
                override_builtins: false,
            }),
    );
    let radio_list = agent.register_host_class(
        HostClass::new("RadioNodeList")
            .parent(dc.node_list)
            .attribute("value", Some(rnl_value), Some(rnl_set_value))
            .exotic(bf_js::host::HostExotic {
                length: Some(c::nl_len),
                item: Some(c::nl_at),
                ..bf_js::host::HostExotic::default()
            }),
    );
    agent.set_embedder_data(FormCollectionClasses {
        controls,
        radio_list,
    });
}

fn collection_classes(agent: &Agent) -> FormCollectionClasses {
    *agent
        .embedder_data::<FormCollectionClasses>()
        .expect("FormCollectionClasses")
}

/// `namedItem(name)` de `HTMLFormControlsCollection` (§2.7.2.2): null, el único elemento con
/// ese `id` o `name`, o un `RadioNodeList` con todos ellos.
fn fc_lookup(
    agent: &mut Agent,
    d: &crate::dom::collections::HtmlCollectionData,
    key: &DomString,
) -> Option<Value> {
    if key.is_empty() {
        return None;
    }
    let items = crate::dom::collections::hc_items(agent, d);
    let matches: Vec<bf_dom::NodeId> = with_doc(agent, d.doc, |doc| {
        items
            .into_iter()
            .filter(|&n| {
                doc.element(n).is_some_and(|e| {
                    e.attrs.iter().any(|a| {
                        a.ns.is_none() && (a.name == "id" || a.name == "name") && a.value == *key
                    })
                })
            })
            .collect()
    });
    match matches.len() {
        0 => None,
        1 => Some(wrap_node(agent, NodeRef::new(d.doc, matches[0]))),
        _ => {
            let class = collection_classes(agent).radio_list;
            Some(Value::Object(agent.host_create(
                class,
                Box::new(crate::dom::collections::NodeListData::Static {
                    doc: d.doc,
                    nodes: matches,
                }),
            )))
        }
    }
}

fn fc_named(
    agent: &mut Agent,
    o: bf_js::heap::Gc<bf_js::object::Object>,
    name: &JsString,
) -> Option<Value> {
    let d = agent
        .host_unwrap::<crate::dom::collections::HtmlCollectionData>(&Value::Object(o))
        .cloned()?;
    fc_lookup(agent, &d, &DomString::from_utf16(name.units()))
}

fn fc_named_item(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(d) = agent
        .host_unwrap::<crate::dom::collections::HtmlCollectionData>(&a.this)
        .cloned()
    else {
        return agent.type_error("Illegal invocation");
    };
    let key = webidl::to_dom_string(agent, &a.arg(0))?;
    let key = DomString::from_utf16(key.units());
    Ok(fc_lookup(agent, &d, &key).unwrap_or(Value::Null))
}

/// Los radios (`input type=radio`) de un `RadioNodeList`, en orden.
fn rnl_radios(agent: &mut Agent, this: &Value) -> JsResult<(crate::DocIndex, Vec<bf_dom::NodeId>)> {
    let Some(crate::dom::collections::NodeListData::Static { doc, nodes }) = agent
        .host_unwrap::<crate::dom::collections::NodeListData>(this)
        .cloned()
    else {
        return agent.type_error("Illegal invocation");
    };
    let radios = with_doc(agent, doc, |d| {
        nodes
            .into_iter()
            .filter(|&n| {
                d.element(n).is_some_and(|e| {
                    e.is_html("input")
                        && e.attr("type")
                            .is_some_and(|t| t.eq_ignore_ascii_case("radio"))
                })
            })
            .collect()
    });
    Ok((doc, radios))
}

/// `RadioNodeList.value` (§2.7.2.3): el valor del primer radio marcado ("on" sin atributo
/// `value`), o la cadena vacía.
fn rnl_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (doc, radios) = rnl_radios(agent, &a.this)?;
    let v = with_doc(agent, doc, |d| {
        radios.into_iter().find(|&r| d.checkedness(r)).map(|r| {
            d.element(r)
                .and_then(|e| e.attr("value"))
                .map_or_else(|| "on".to_owned(), |v| v.to_string())
        })
    });
    Ok(js_str(&v.unwrap_or_default()))
}

/// El setter: marca el primer radio cuyo `value` es el nuevo valor (o sin `value`, si es
/// "on").
fn rnl_set_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = webidl::to_dom_string(agent, &a.arg(0))?.to_string_lossy();
    let (doc, radios) = rnl_radios(agent, &a.this)?;
    with_doc_mut(agent, doc, |d| {
        let hit = radios
            .into_iter()
            .find(|&r| match d.element(r).and_then(|e| e.attr("value")) {
                Some(x) => *x == v,
                None => v == "on",
            });
        if let Some(r) = hit {
            d.set_checkedness(r, true);
        }
    });
    mutated(agent);
    Ok(Value::Undefined)
}

fn fieldset_elements_attr(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(live_collection(agent, n, 6, fieldset_elements_list))
}

fn select_options_attr(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(live_collection(agent, n, 7, select_options_list))
}

fn selected_options_attr(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(live_collection(agent, n, 8, selected_options_list))
}

// --- E1: `ValidityState`, `labels` y lo que faltaba de `HTMLInputElement` ---
//
// HTML §4.10.20.3 (la API de validación de restricciones), §4.10.4 (`labels`), §4.10.5.4
// (`list`, `valueAsDate`, `stepUp`/`stepDown`, `showPicker`), §4.10.18.6 (`setRangeText`) y
// §6.12 (PopoverInvokerElement).

/// La clase `ValidityState` (dato del embebedor).
struct ValidityClass(HostClassId);

/// El payload de un `ValidityState`: su control (un tipo propio, para que no pase por nodo).
struct ValidityOf(NodeRef);

/// Los estados de validez de un control ("suffering from…", §4.10.20.1).
#[derive(Debug, Default, Clone, Copy)]
struct Flags {
    value_missing: bool,
    type_mismatch: bool,
    pattern_mismatch: bool,
    too_long: bool,
    too_short: bool,
    range_underflow: bool,
    range_overflow: bool,
    step_mismatch: bool,
    bad_input: bool,
    custom_error: bool,
}

impl Flags {
    fn any(&self) -> bool {
        self.value_missing
            || self.type_mismatch
            || self.pattern_mismatch
            || self.too_long
            || self.too_short
            || self.range_underflow
            || self.range_overflow
            || self.step_mismatch
            || self.bad_input
            || self.custom_error
    }
}

macro_rules! validity_getters {
    ($($fn_name:ident => $field:ident),* $(,)?) => {
        $(
            fn $fn_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
                let n = validity_target(agent, a)?;
                Ok(Value::Boolean(flags(agent, n).$field))
            }
        )*
    };
}

validity_getters! {
    v_value_missing => value_missing,
    v_type_mismatch => type_mismatch,
    v_pattern_mismatch => pattern_mismatch,
    v_too_long => too_long,
    v_too_short => too_short,
    v_range_underflow => range_underflow,
    v_range_overflow => range_overflow,
    v_step_mismatch => step_mismatch,
    v_bad_input => bad_input,
    v_custom_error => custom_error,
}

fn v_valid(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = validity_target(agent, a)?;
    Ok(Value::Boolean(!flags(agent, n).any()))
}

fn validity_target(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<NodeRef> {
    match agent.host_unwrap::<ValidityOf>(&a.this) {
        Some(v) => Ok(v.0),
        None => agent.type_error("Illegal invocation"),
    }
}

/// Registra `ValidityState` (la primera vez).
pub fn register_validity(agent: &mut Agent) {
    if agent.embedder_data::<ValidityClass>().is_some() {
        return;
    }
    let class = HostClass::new("ValidityState")
        .attribute("valueMissing", Some(v_value_missing), None)
        .attribute("typeMismatch", Some(v_type_mismatch), None)
        .attribute("patternMismatch", Some(v_pattern_mismatch), None)
        .attribute("tooLong", Some(v_too_long), None)
        .attribute("tooShort", Some(v_too_short), None)
        .attribute("rangeUnderflow", Some(v_range_underflow), None)
        .attribute("rangeOverflow", Some(v_range_overflow), None)
        .attribute("stepMismatch", Some(v_step_mismatch), None)
        .attribute("badInput", Some(v_bad_input), None)
        .attribute("customError", Some(v_custom_error), None)
        .attribute("valid", Some(v_valid), None);
    let id = agent.register_host_class(class);
    agent.set_embedder_data(ValidityClass(id));
}

/// `validity` (`[SameObject]`): el `ValidityState` del control.
fn get_validity(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let Some(&ValidityClass(class)) = agent.embedder_data::<ValidityClass>() else {
        return Ok(Value::Undefined);
    };
    let realm = WebContextHandle::current(agent).realm;
    let key = bf_js::host::HostKey(n.key().0 | (1 << 63));
    let o = agent.host_wrap(realm, key, class, Box::new(ValidityOf(n)));
    Ok(Value::Object(o))
}

/// Los estados de validez de `n` (sin candidato a la validación, solo el error propio).
fn flags(agent: &mut Agent, n: NodeRef) -> Flags {
    let mut f = Flags {
        custom_error: !state(agent, n).custom_validity.is_empty(),
        ..Flags::default()
    };
    if is_face(agent, n) {
        f.custom_error |= !super::custom_elements::face_is_valid(agent, n);
        return f;
    }
    if !will_validate_node(agent, n) {
        return f;
    }
    let name = html_local_name(agent, n).unwrap_or_default();
    let required = attr_value(agent, n, "required").is_some();
    let readonly = attr_value(agent, n, "readonly").is_some();
    match name.as_str() {
        "input" => {
            let t = input_type(agent, n);
            let v = current_value(agent, n);
            let value_missing_applies = !matches!(
                t.as_str(),
                "range" | "color" | "hidden" | "submit" | "image" | "reset" | "button"
            );
            f.value_missing = required
                && value_missing_applies
                && (!readonly || matches!(t.as_str(), "checkbox" | "radio" | "file"))
                && match t.as_str() {
                    "checkbox" => !with_doc(agent, n.doc, |d| d.checkedness(n.node)),
                    "radio" => !radio_group_checked(agent, n),
                    _ => v.is_empty(),
                };
            if v.is_empty() {
                return f;
            }
            let multiple = attr_value(agent, n, "multiple").is_some();
            let values: Vec<String> = if t == "email" && multiple {
                v.split(',').map(|s| trim_ascii_ws(s).to_owned()).collect()
            } else {
                vec![v.clone()]
            };
            f.type_mismatch = match t.as_str() {
                "email" => values.iter().any(|e| !valid_email(e)),
                "url" => bf_infra::Url::parse(&v, None).is_err(),
                _ => false,
            };
            if matches!(
                t.as_str(),
                "text" | "search" | "url" | "tel" | "email" | "password"
            ) && let Some(p) = attr_value(agent, n, "pattern")
            {
                let p = p.to_string_lossy().into_owned();
                f.pattern_mismatch = values
                    .iter()
                    .any(|x| pattern_matches(agent, &p, x) == Some(false));
            }
            if matches!(t.as_str(), "number" | "range")
                && let Some(x) = parse_float(&v)
            {
                let min =
                    attr_value(agent, n, "min").and_then(|s| parse_float(&s.to_string_lossy()));
                let max =
                    attr_value(agent, n, "max").and_then(|s| parse_float(&s.to_string_lossy()));
                f.range_underflow = min.is_some_and(|m| x < m);
                f.range_overflow = max.is_some_and(|m| x > m);
                if let Some((step, base)) = allowed_step(agent, n) {
                    let q = (x - base) / step;
                    f.step_mismatch = (q - q.round()).abs() > 1e-9 * q.abs().max(1.0);
                }
            }
        }
        "textarea" => {
            f.value_missing = required && !readonly && textarea_api_value(agent, n).is_empty();
        }
        "select" => {
            f.value_missing = required
                && !options(agent, n).iter().any(|o| {
                    with_doc(agent, o.doc, |d| d.option_selectedness(o.node))
                        && !option_value_of(agent, *o).is_empty()
                });
        }
        _ => {}
    }
    f
}

/// ¿Hay un radio marcado en el grupo de `n` (él incluido)?
fn radio_group_checked(agent: &Agent, n: NodeRef) -> bool {
    with_doc(agent, n.doc, |d| {
        d.checkedness(n.node) || d.radio_group(n.node).iter().any(|&r| d.checkedness(r))
    })
}

/// "Valid email address" (HTML §4.10.5.1.5).
fn valid_email(s: &str) -> bool {
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    let local_ok = !local.is_empty()
        && local
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".!#$%&'*+/=?^_`{|}~-".contains(c));
    let label_ok = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !l.starts_with('-')
            && !l.ends_with('-')
    };
    local_ok && !domain.is_empty() && domain.split('.').all(label_ok)
}

/// `pattern` (HTML §4.10.5.3.6): `^(?:pattern)$` con el flag `v`. `None` si no compila (y
/// entonces no hay restricción).
fn pattern_matches(agent: &mut Agent, pattern: &str, value: &str) -> Option<bool> {
    let realm = agent.current_realm;
    let ctor = agent.realms[realm].intrinsics.regexp_ctor;
    let proto = agent.realms[realm].intrinsics.regexp_prototype;
    let src = format!("^(?:{pattern})$");
    let re = agent
        .construct(ctor, &[js_str(&src), js_str("v")], None)
        .ok()?;
    // Fijado mientras se busca `test` (el `RegExp` recién creado no tiene otra raíz).
    let pin = agent.host_pin(re.clone());
    let r = match agent.get_str(proto, "test") {
        Ok(test) => match agent.call(&test, re, &[js_str(value)]) {
            Ok(Value::Boolean(b)) => Some(b),
            _ => None,
        },
        Err(_) => None,
    };
    agent.host_unpin(pin);
    r
}

/// El *allowed value step* y el *step base* (§4.10.5.4) de `number`/`range`: `None` con
/// `step=any`.
fn allowed_step(agent: &Agent, n: NodeRef) -> Option<(f64, f64)> {
    let step = match attr_value(agent, n, "step") {
        Some(s) if s.to_string_lossy().eq_ignore_ascii_case("any") => return None,
        Some(s) => parse_float(&s.to_string_lossy())
            .filter(|x| *x > 0.0)
            .unwrap_or(1.0),
        None => 1.0,
    };
    let base = attr_value(agent, n, "min")
        .and_then(|s| parse_float(&s.to_string_lossy()))
        .or_else(|| attr_value(agent, n, "value").and_then(|s| parse_float(&s.to_string_lossy())))
        .unwrap_or(0.0);
    Some((step, base))
}

/// El mensaje de `validationMessage` (el del error propio, o uno genérico por estado).
fn validation_message_of(agent: &mut Agent, n: NodeRef) -> String {
    if !will_validate_node(agent, n) {
        return String::new();
    }
    let custom = state(agent, n).custom_validity;
    if !custom.is_empty() {
        return custom;
    }
    let f = flags(agent, n);
    let m = if f.value_missing {
        "Please fill out this field."
    } else if f.type_mismatch {
        "Please enter a valid value."
    } else if f.pattern_mismatch {
        "Please match the requested format."
    } else if f.range_underflow || f.range_overflow {
        "Value is out of range."
    } else if f.step_mismatch {
        "Please enter a valid value (step)."
    } else {
        ""
    };
    m.to_owned()
}

/// `labels` (§4.10.4): los `label` del árbol cuyo control es este, en orden; `null` en un
/// `input` hidden.
fn get_labels(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    if html_local_name(agent, n).as_deref() == Some("input") && input_type(agent, n) == "hidden" {
        return Ok(Value::Null);
    }
    let labels: Vec<bf_dom::NodeId> = {
        let candidates = with_doc(agent, n.doc, |d| {
            let root = d.get_root_node(n.node);
            d.descendants(root)
                .into_iter()
                .filter(|&x| is_html_el(d, x, "label"))
                .collect::<Vec<_>>()
        });
        candidates
            .into_iter()
            .filter(|&l| label_control_of(agent, NodeRef::new(n.doc, l)) == Some(n))
            .collect()
    };
    Ok(crate::dom::collections::static_node_list(
        agent, n.doc, labels,
    ))
}

/// `input.list` (§4.10.5.3.9): el `datalist` con el id del atributo `list`, o `null`.
fn input_list(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    if matches!(
        input_type(agent, n).as_str(),
        "hidden" | "checkbox" | "radio" | "file" | "submit" | "image" | "reset" | "button"
    ) {
        return Ok(Value::Null);
    }
    let Some(id) = attr_value(agent, n, "list") else {
        return Ok(Value::Null);
    };
    let e = with_doc(agent, n.doc, |d| {
        d.get_element_by_id(&id)
            .filter(|&e| is_html_el(d, e, "datalist"))
    });
    Ok(wrap_opt(agent, n.doc, e))
}

/// Días desde 1970-01-01 de una fecha del calendario gregoriano proléptico.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// La fecha de un número de días desde 1970-01-01.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// Los ms UTC de un valor `date`, `month`, `week` o `time` (§2.3.5), o `None`.
fn date_value_ms(t: &str, v: &str) -> Option<f64> {
    let num = |s: &str| s.parse::<i64>().ok();
    match t {
        "date" => {
            let mut it = v.splitn(3, '-');
            let (y, m, d) = (num(it.next()?)?, num(it.next()?)?, num(it.next()?)?);
            Some(days_from_civil(y, m, d) as f64 * 86_400_000.0)
        }
        "month" => {
            let (y, m) = v.split_once('-')?;
            Some(days_from_civil(num(y)?, num(m)?, 1) as f64 * 86_400_000.0)
        }
        // El lunes de la semana ISO 8601 (`YYYY-Www`).
        "week" => {
            let (y, w) = v.split_once("-W")?;
            let (y, w) = (num(y)?, num(w)?);
            let jan4 = days_from_civil(y, 1, 4);
            let monday = jan4 - (jan4 + 3).rem_euclid(7);
            Some((monday + (w - 1) * 7) as f64 * 86_400_000.0)
        }
        "time" => {
            let mut it = v.splitn(3, ':');
            let h = num(it.next()?)?;
            let mi = num(it.next()?)?;
            let s: f64 = it.next().map_or(Some(0.0), |s| s.parse().ok())?;
            Some((h * 3_600_000 + mi * 60_000) as f64 + s * 1000.0)
        }
        _ => None,
    }
}

/// El valor `date`/`month`/`time` de unos ms UTC.
fn date_value_string(t: &str, ms: f64) -> String {
    let days = (ms / 86_400_000.0).floor() as i64;
    let (y, m, d) = civil_from_days(days);
    match t {
        "date" => format!("{y:04}-{m:02}-{d:02}"),
        "month" => format!("{y:04}-{m:02}"),
        // La semana ISO 8601: la del jueves de la semana del día.
        "week" => {
            let thursday = days - (days + 3).rem_euclid(7) + 3;
            let (wy, _, _) = civil_from_days(thursday);
            let week = (thursday - days_from_civil(wy, 1, 1)) / 7 + 1;
            format!("{wy:04}-W{week:02}")
        }
        _ => {
            let in_day = ms - days as f64 * 86_400_000.0;
            let total = in_day.round() as i64;
            let (h, mi, s, milli) = (
                total / 3_600_000,
                (total / 60_000) % 60,
                (total / 1000) % 60,
                total % 1000,
            );
            match (s, milli) {
                (0, 0) => format!("{h:02}:{mi:02}"),
                (_, 0) => format!("{h:02}:{mi:02}:{s:02}"),
                _ => format!("{h:02}:{mi:02}:{s:02}.{milli:03}"),
            }
        }
    }
}

/// `valueAsDate` (§4.10.5.4): un `Date` para `date`, `month`, `week` y `time`; `null` si no
/// aplica o
/// el valor no es válido.
fn value_as_date(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let t = input_type(agent, n);
    let Some(ms) = date_value_ms(&t, &current_value(agent, n)) else {
        return Ok(Value::Null);
    };
    let ctor = agent.realms[agent.current_realm].intrinsics.date_ctor;
    agent.construct(ctor, &[Value::Number(ms)], None)
}

fn set_value_as_date(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let t = input_type(agent, n);
    if !matches!(t.as_str(), "date" | "month" | "week" | "time") {
        return throw_dom(agent, DomException::InvalidStateError);
    }
    let v = a.arg(0);
    let ms = match &v {
        Value::Null => None,
        Value::Object(o) => match agent.heap[*o].kind {
            bf_js::object::ObjectKind::Date(t) => Some(t),
            _ => return agent.type_error("The provided value is not a Date."),
        },
        _ => return agent.type_error("The provided value is not a Date."),
    };
    let s = match ms {
        Some(ms) if ms.is_finite() => date_value_string(&t, ms),
        _ => String::new(),
    };
    state_mut(agent, n).value = Some(s);
    Ok(Value::Undefined)
}

/// `stepUp(n)`/`stepDown(n)` (§4.10.5.4), para `number` y `range`.
fn step_by(agent: &mut Agent, a: &CallArgs<'_>, sign: f64) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let count = match a.arg(0) {
        Value::Undefined => 1,
        v => webidl::to_long(agent, &v)?,
    };
    let t = input_type(agent, n);
    if !matches!(t.as_str(), "number" | "range") {
        return throw_dom(agent, DomException::InvalidStateError);
    }
    let Some((step, base)) = allowed_step(agent, n) else {
        return throw_dom(agent, DomException::InvalidStateError);
    };
    let min = attr_value(agent, n, "min").and_then(|s| parse_float(&s.to_string_lossy()));
    let max = attr_value(agent, n, "max").and_then(|s| parse_float(&s.to_string_lossy()));
    if let (Some(lo), Some(hi)) = (min, max)
        && lo > hi
    {
        return Ok(Value::Undefined);
    }
    let value = parse_float(&current_value(agent, n)).unwrap_or(0.0);
    let q = (value - base) / step;
    let mut new = if (q - q.round()).abs() > 1e-9 {
        // Sin alinear: al valor alineado siguiente en la dirección del paso.
        base + if sign > 0.0 { q.ceil() } else { q.floor() } * step
    } else {
        value + sign * f64::from(count) * step
    };
    if let Some(lo) = min
        && new < lo
    {
        new = base + ((lo - base) / step).ceil() * step;
    }
    if let Some(hi) = max
        && new > hi
    {
        new = base + ((hi - base) / step).floor() * step;
    }
    if (sign > 0.0 && new < value) || (sign < 0.0 && new > value) {
        return Ok(Value::Undefined);
    }
    let s = sanitize(&t, &super::reflect::float_to_string(new));
    state_mut(agent, n).value = Some(s);
    Ok(Value::Undefined)
}

fn step_up(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    step_by(agent, a, 1.0)
}

fn step_down(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    step_by(agent, a, -1.0)
}

/// `showPicker()` (§4.10.5.1.20): sin activación de usuario, `NotAllowedError` (y
/// `InvalidStateError` si el control no es mutable).
fn show_picker(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let immutable = with_doc(agent, n.doc, |d| d.is_disabled(n.node))
        || attr_value(agent, n, "readonly").is_some();
    if immutable {
        return throw_dom(agent, DomException::InvalidStateError);
    }
    let e = crate::dom::exception::create(
        agent,
        "NotAllowedError",
        "Failed to execute 'showPicker': requires a user gesture",
    );
    Err(bf_js::vm::Throw::Value(e))
}

/// `setRangeText(replacement[, start, end[, selectMode]])` (§4.10.18.6).
fn set_range_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    crate::dom::events::required(agent, a, 1, "setRangeText")?;
    if !selection_applies(agent, n) {
        return throw_dom(agent, DomException::InvalidStateError);
    }
    if a.args.len() == 2 {
        return agent.type_error("Failed to execute 'setRangeText': 3 arguments required.");
    }
    let replacement: Vec<u16> = webidl::to_dom_string(agent, &a.arg(0))?.units().to_vec();
    let (sel_s, sel_e, _) = selection(agent, n);
    let (start, end, mode) = if a.args.len() >= 3 {
        let s = webidl::to_unsigned_long(agent, &a.arg(1))?;
        let e = webidl::to_unsigned_long(agent, &a.arg(2))?;
        let mode = match a.arg(3) {
            Value::Undefined => "preserve".to_owned(),
            v => webidl::to_dom_string(agent, &v)?.to_string_lossy(),
        };
        if !matches!(mode.as_str(), "select" | "start" | "end" | "preserve") {
            return agent.type_error("The provided value is not a valid SelectionMode.");
        }
        (s, e, mode)
    } else {
        (sel_s, sel_e, "preserve".to_owned())
    };
    if start > end {
        return throw_dom(agent, DomException::IndexSizeError);
    }
    let value: Vec<u16> = api_value(agent, n).encode_utf16().collect();
    let len = value.len() as u32;
    let (start, end) = (start.min(len), end.min(len));
    let mut new = value[..start as usize].to_vec();
    new.extend_from_slice(&replacement);
    new.extend_from_slice(&value[end as usize..]);
    let new_len = replacement.len() as u32;
    state_mut(agent, n).value = Some(String::from_utf16_lossy(&new));
    let new_end = start + new_len;
    let (s, e) = match mode.as_str() {
        "select" => (start, new_end),
        "start" => (start, start),
        "end" => (new_end, new_end),
        _ => {
            let delta = i64::from(new_len) - i64::from(end - start);
            let adjust = |p: u32| -> u32 {
                if p > end {
                    (i64::from(p) + delta) as u32
                } else if p > start {
                    start
                } else {
                    p
                }
            };
            let s = adjust(sel_s);
            let e = if sel_e > end {
                (i64::from(sel_e) + delta) as u32
            } else if sel_e > start {
                new_end
            } else {
                sel_e
            };
            (s, e)
        }
    };
    apply_selection(agent, n, s, e, "none");
    Ok(Value::Undefined)
}

/// `popoverTargetElement` (§6.12, reflejo de elemento de `popovertarget`).
fn popover_target_element(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let Some(id) = attr_value(agent, n, "popovertarget") else {
        return Ok(Value::Null);
    };
    let e = with_doc(agent, n.doc, |d| d.get_element_by_id(&id));
    Ok(wrap_opt(agent, n.doc, e))
}

fn set_popover_target_element(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = a.arg(0);
    if v.is_nullish() {
        super::elements::remove_attr(agent, n, "popovertarget");
        return Ok(Value::Undefined);
    }
    let el = crate::dom::arg_node(agent, &v, "popoverTargetElement")?;
    let id = attr_value(agent, el, "id").unwrap_or_default();
    set_attr(agent, n, "popovertarget", id)?;
    Ok(Value::Undefined)
}

/// Un atributo enumerado (§2.3.3): el valor conocido del atributo, o el de por defecto.
fn enumerated(agent: &Agent, n: NodeRef, attr: &str, known: &[&str], default: &str) -> String {
    let v = attr_value(agent, n, attr)
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if known.contains(&v.as_str()) {
        v
    } else {
        default.to_owned()
    }
}

fn set_string_attr(agent: &mut Agent, a: &CallArgs<'_>, attr: &str) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = webidl::to_dom_string(agent, &a.arg(0))?;
    set_attr(agent, n, attr, DomString::from_utf16(v.units()))?;
    Ok(Value::Undefined)
}

fn popover_target_action(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = enumerated(
        agent,
        n,
        "popovertargetaction",
        &["toggle", "show", "hide"],
        "toggle",
    );
    Ok(js_str(&v))
}

fn set_popover_target_action(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    set_string_attr(agent, a, "popovertargetaction")
}

/// `colorSpace` (§4.10.5.1.15, `colorspace`: `limited-srgb` o `display-p3`).
fn color_space(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = enumerated(
        agent,
        n,
        "colorspace",
        &["limited-srgb", "display-p3"],
        "limited-srgb",
    );
    Ok(js_str(&v))
}

fn set_color_space(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    set_string_attr(agent, a, "colorspace")
}

/// `alpha` (atributo booleano).
fn get_alpha(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Boolean(attr_value(agent, n, "alpha").is_some()))
}

fn set_alpha(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    if webidl::to_boolean(agent, &a.arg(0)) {
        set_attr(agent, n, "alpha", DomString::new())?;
    } else {
        super::elements::remove_attr(agent, n, "alpha");
    }
    Ok(Value::Undefined)
}

/// Lo de PopoverInvokerElement (`button` e `input`).
fn popover_invoker(class: HostClass) -> HostClass {
    class
        .attribute(
            "popoverTargetElement",
            Some(popover_target_element),
            Some(set_popover_target_element),
        )
        .attribute(
            "popoverTargetAction",
            Some(popover_target_action),
            Some(set_popover_target_action),
        )
}
