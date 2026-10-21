//! Los miembros de `Document` que son del BOM y de la carga (HTML §3.1.3, §3.1.5, §8.4):
//! `cookie`, `referrer`, `lastModified`, `readyState`, `currentScript`, `visibilityState`,
//! `hidden`, `hasFocus()` y `write()`/`writeln()`/`open()`/`close()`. Se agregan al prototipo
//! de `Document` (la clase es de [`crate::dom`]).

use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, RealmId};

use super::{accessor, method, state, state_mut, to_rust_string};
use crate::dom::{classes, unwrap_node, wrap_opt};
use crate::{NodeRef, WebContextHandle, with_doc};

/// Agrega los miembros al prototipo de `Document` de `realm`.
pub fn install(agent: &mut Agent, realm: RealmId) {
    let class = classes(agent).document;
    let proto = agent.host_class_objects(realm, class).prototype;
    accessor(agent, proto, "cookie", get_cookie, Some(set_cookie));
    accessor(agent, proto, "referrer", referrer, None);
    accessor(agent, proto, "lastModified", last_modified, None);
    accessor(agent, proto, "readyState", ready_state, None);
    accessor(agent, proto, "currentScript", current_script, None);
    accessor(agent, proto, "visibilityState", visibility_state, None);
    accessor(agent, proto, "hidden", hidden, None);
    method(agent, proto, "hasFocus", 0, has_focus);
    method(agent, proto, "write", 0, super::script::document_write);
    method(agent, proto, "writeln", 0, super::script::document_writeln);
    method(agent, proto, "open", 0, super::script::document_open);
    method(agent, proto, "close", 0, super::script::document_close);
}

/// El documento de `this` (un `Document` de la ventana o `TypeError`).
pub(crate) fn this_document(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<NodeRef> {
    let document = classes(agent).document;
    match unwrap_node(agent, &a.this) {
        Some(n) if agent.host_is_instance(&a.this, document) => Ok(n),
        _ => agent.type_error("Illegal invocation"),
    }
}

/// ¿Es el documento de la ventana (el que tiene cookies, `readyState` de la carga, …)?
fn is_window_document(n: NodeRef) -> bool {
    n.doc == 0
}

/// `document.cookie` (HTML §3.1.3): el de la ventana; los documentos sin navegador (creados
/// por script) y los de origen opaco devuelven "".
fn get_cookie(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_document(agent, a)?;
    if !is_window_document(n) {
        return Ok(Value::str(""));
    }
    let url = super::window_url(agent);
    if super::origin_of(&url) == "null" {
        return crate::dom::throw_dom(agent, bf_dom::DomException::SecurityError);
    }
    let cookies = state_mut(agent).hooks.cookies(&url);
    Ok(Value::str(&cookies))
}

fn set_cookie(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_document(agent, a)?;
    let value = to_rust_string(agent, &a.arg(0))?;
    if !is_window_document(n) {
        return Ok(Value::Undefined);
    }
    let url = super::window_url(agent);
    if super::origin_of(&url) == "null" {
        return crate::dom::throw_dom(agent, bf_dom::DomException::SecurityError);
    }
    state_mut(agent).hooks.set_cookie(&url, &value);
    Ok(Value::Undefined)
}

fn referrer(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_document(agent, a)?;
    if !is_window_document(n) {
        return Ok(Value::str(""));
    }
    Ok(Value::str(&state(agent).hooks.referrer()))
}

/// `lastModified`: sin cabecera `Last-Modified`, la hora actual (`MM/DD/YYYY hh:mm:ss`, hora
/// local; aquí UTC).
fn last_modified(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let secs = super::performance::epoch_ms() as i64 / 1000;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    Ok(Value::str(&format!(
        "{m:02}/{d:02}/{y:04} {:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )))
}

/// Fecha civil (año, mes, día) de un número de días desde 1970-01-01.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn ready_state(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_document(agent, a)?;
    let r = with_doc(agent, n.doc, |d| d.ready_state().as_str());
    Ok(Value::str(r))
}

fn current_script(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_document(agent, a)?;
    let s = with_doc(agent, n.doc, |d| d.scripts().current_script);
    Ok(wrap_opt(agent, n.doc, s))
}

fn visibility_state(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str("visible"))
}

fn hidden(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(false))
}

fn has_focus(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_document(agent, a)?;
    let _ = WebContextHandle::current(agent);
    Ok(Value::Boolean(is_window_document(n)))
}
