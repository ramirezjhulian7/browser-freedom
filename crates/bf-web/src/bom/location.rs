//! `Location` (HTML §7.10.4): los componentes de la URL de la ventana y la navegación. Un
//! cambio de solo el fragmento es una navegación dentro del documento (entrada nueva en el
//! historial y `hashchange`); cualquier otro va al navegador ([`super::BomHooks::navigate`]).

use bf_infra::Url;
use bf_js::host::HostClass;
use bf_js::value::Value;
use bf_js::vm::function::{CallArgs, NativeFn};
use bf_js::vm::{Agent, JsResult};

use super::{state_mut, to_rust_string, window_url};
use crate::WebContextHandle;

pub fn location_class() -> HostClass {
    let attrs: [(&'static str, NativeFn, Option<NativeFn>); 9] = [
        ("href", href, Some(set_href)),
        ("origin", origin, None),
        ("protocol", protocol, Some(set_protocol)),
        ("host", host, Some(set_host)),
        ("hostname", hostname, Some(set_hostname)),
        ("port", port, Some(set_port)),
        ("pathname", pathname, Some(set_pathname)),
        ("search", search, Some(set_search)),
        ("hash", hash, Some(set_hash)),
    ];
    let mut c = HostClass::new("Location");
    // Todos los miembros de `Location` son `[LegacyUnforgeable]` (propiedades propias).
    for (name, get, set) in attrs {
        c = c.unforgeable(name, Some(get), set);
    }
    c.method("assign", 1, assign)
        .method("replace", 1, replace)
        .method("reload", 0, reload)
        .method("toString", 0, href)
}

fn href(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&window_url(agent).href()))
}

fn origin(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&super::origin_of(&window_url(agent))))
}

fn protocol(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&window_url(agent).protocol()))
}

fn host(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&window_url(agent).host_string()))
}

fn hostname(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&window_url(agent).hostname()))
}

fn port(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&window_url(agent).port_string()))
}

fn pathname(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&window_url(agent).path_string()))
}

fn search(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&window_url(agent).search()))
}

fn hash(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&window_url(agent).hash()))
}

/// Resuelve `href` contra la URL base de la ventana y navega (`replace`: sin entrada nueva).
pub fn navigate_to(agent: &mut Agent, href: &str, replace: bool) -> JsResult<()> {
    let base = WebContextHandle::current(agent).get(agent).base.clone();
    match Url::parse(href, Some(&base)) {
        Ok(url) => {
            navigate_url(agent, url, replace);
            Ok(())
        }
        Err(_) => crate::dom::throw_dom(agent, bf_dom::DomException::SyntaxError),
    }
}

/// "Location-object navigate" (HTML §7.10.4.1) a `url`.
pub fn navigate_url(agent: &mut Agent, url: Url, replace: bool) {
    let current = window_url(agent);
    let same_document = current.serialize(true) == url.serialize(true) && url.fragment.is_some();
    if same_document {
        // "Navigate to a fragment" (HTML §7.4.2.3.3).
        let changed = current.fragment != url.fragment;
        super::history::update_url(agent, url.clone(), !replace);
        state_mut(agent)
            .hooks
            .same_document_navigation(&url, replace);
        if changed {
            super::script::queue_window_event(agent, "hashchange");
        }
        return;
    }
    state_mut(agent).hooks.navigate(&url, replace);
}

/// Un setter de componente: aplica `set` a una copia de la URL y navega (HTML §7.10.4.2).
fn set_component(agent: &mut Agent, a: &CallArgs<'_>, set: fn(&mut Url, &str)) -> JsResult<Value> {
    let v = to_rust_string(agent, &a.arg(0))?;
    let mut url = window_url(agent);
    set(&mut url, &v);
    navigate_url(agent, url, false);
    Ok(Value::Undefined)
}

fn set_href(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = to_rust_string(agent, &a.arg(0))?;
    navigate_to(agent, &v, false)?;
    Ok(Value::Undefined)
}

fn set_protocol(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = to_rust_string(agent, &a.arg(0))?;
    let mut url = window_url(agent);
    url.set_protocol(&v);
    // "If copyURL's scheme is not an HTTP(S) scheme, then terminate these steps."
    if !matches!(url.scheme.as_str(), "http" | "https") {
        return Ok(Value::Undefined);
    }
    navigate_url(agent, url, false);
    Ok(Value::Undefined)
}

fn set_host(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    set_component(agent, a, Url::set_host)
}

fn set_hostname(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    set_component(agent, a, Url::set_hostname)
}

fn set_port(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    set_component(agent, a, Url::set_port)
}

fn set_pathname(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    set_component(agent, a, Url::set_pathname)
}

fn set_search(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    set_component(agent, a, Url::set_search)
}

/// `hash`: "If copyURL's fragment is this's url's fragment, then return" (no navega).
fn set_hash(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = to_rust_string(agent, &a.arg(0))?;
    let mut url = window_url(agent);
    let before = url.fragment.clone();
    url.set_hash(&v);
    if url.fragment.is_none() {
        url.fragment = Some(String::new());
    }
    if url.fragment == before {
        return Ok(Value::Undefined);
    }
    navigate_url(agent, url, false);
    Ok(Value::Undefined)
}

fn assign(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = to_rust_string(agent, &a.arg(0))?;
    navigate_to(agent, &v, false)?;
    Ok(Value::Undefined)
}

fn replace(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = to_rust_string(agent, &a.arg(0))?;
    navigate_to(agent, &v, true)?;
    Ok(Value::Undefined)
}

fn reload(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    state_mut(agent).hooks.reload();
    Ok(Value::Undefined)
}
