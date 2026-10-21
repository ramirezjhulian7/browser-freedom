//! `History` (HTML §7.4.3): el historial de sesión del documento. Las entradas creadas en el
//! documento (`pushState`, navegación a un fragmento) viven aquí; ir más atrás de la primera o
//! más adelante de la última es una traversal del navegador ([`super::BomHooks::traverse`]).
//!
//! El estado de cada entrada se guarda tal cual (fijado con `host_pin`), sin el clonado
//! estructurado de `StructuredSerializeForStorage`.

use bf_infra::Url;
use bf_js::host::{HostClass, HostHandle};
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use super::{state, state_mut, to_rust_string, window_url};
use crate::WebContextHandle;

/// Una entrada del historial creada en el documento.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub url: Url,
    /// El *serialized state* (`history.state`).
    pub state: Option<HostHandle>,
}

/// Las entradas del historial de sesión que pertenecen al documento.
#[derive(Debug, Clone)]
pub struct SessionHistory {
    pub entries: Vec<HistoryEntry>,
    pub index: usize,
    /// `history.scrollRestoration` es "manual".
    pub manual_scroll: bool,
    /// Se agregaron entradas: las siguientes del navegador ya no existen.
    pub pushed: bool,
}

impl SessionHistory {
    pub fn new(url: &Url) -> SessionHistory {
        SessionHistory {
            entries: vec![HistoryEntry {
                url: url.clone(),
                state: None,
            }],
            index: 0,
            manual_scroll: false,
            pushed: false,
        }
    }
}

pub fn history_class() -> HostClass {
    HostClass::new("History")
        .attribute("length", Some(length), None)
        .attribute(
            "scrollRestoration",
            Some(scroll_restoration),
            Some(set_scroll_restoration),
        )
        .attribute("state", Some(get_state), None)
        .method("go", 0, go)
        .method("back", 0, back)
        .method("forward", 0, forward)
        .method("pushState", 2, push_state)
        .method("replaceState", 2, replace_state)
}

/// Cambia la URL del documento (y su URL base si era la del documento).
fn set_document_url(agent: &mut Agent, url: Url) {
    let ctx = WebContextHandle::current(agent).get_mut(agent);
    if ctx.base == ctx.url {
        ctx.base = url.clone();
    }
    ctx.url = url.clone();
    ctx.document_mut().url = url;
}

/// Una navegación dentro del documento a `url`: entrada nueva (`push`) o la actual cambia.
pub fn update_url(agent: &mut Agent, url: Url, push: bool) {
    add_entry(agent, url, None, push);
}

fn add_entry(agent: &mut Agent, url: Url, data: Option<HostHandle>, push: bool) {
    let h = &mut state_mut(agent).history;
    let dropped: Vec<HostHandle> = if push {
        let mut out: Vec<HostHandle> = h
            .entries
            .drain(h.index + 1..)
            .filter_map(|e| e.state)
            .collect();
        h.entries.push(HistoryEntry {
            url: url.clone(),
            state: data,
        });
        h.index = h.entries.len() - 1;
        h.pushed = true;
        out.shrink_to_fit();
        out
    } else {
        let e = &mut h.entries[h.index];
        let old = std::mem::replace(&mut e.state, data);
        e.url = url.clone();
        old.into_iter().collect()
    };
    for d in dropped {
        agent.host_unpin(d);
    }
    set_document_url(agent, url);
}

fn length(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let s = state(agent);
    let (before, after) = s.hooks.session_history();
    let after = if s.history.pushed { 0 } else { after };
    let n = before.saturating_sub(1) as usize + s.history.entries.len() + after as usize;
    Ok(Value::Number(n as f64))
}

fn scroll_restoration(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let manual = state(agent).history.manual_scroll;
    Ok(Value::str(if manual { "manual" } else { "auto" }))
}

/// Un valor del enum `ScrollRestoration`; otro valor se ignora (WebIDL §3.2.20).
fn set_scroll_restoration(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = to_rust_string(agent, &a.arg(0))?;
    match v.as_str() {
        "auto" => state_mut(agent).history.manual_scroll = false,
        "manual" => state_mut(agent).history.manual_scroll = true,
        _ => {}
    }
    Ok(Value::Undefined)
}

fn get_state(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let h = &state(agent).history;
    let handle = h.entries[h.index].state;
    Ok(handle
        .and_then(|h| agent.host_pinned(h))
        .unwrap_or(Value::Null))
}

/// `history.go(delta)` (HTML §7.4.3, "delta traverse").
fn go(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let delta = if a.arg(0).is_undefined() {
        0.0
    } else {
        agent.to_number(&a.arg(0))?
    };
    let delta = if delta.is_finite() { delta as i64 } else { 0 };
    traverse(agent, delta);
    Ok(Value::Undefined)
}

fn back(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    traverse(agent, -1);
    Ok(Value::Undefined)
}

fn forward(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    traverse(agent, 1);
    Ok(Value::Undefined)
}

fn traverse(agent: &mut Agent, delta: i64) {
    if delta == 0 {
        state_mut(agent).hooks.reload();
        return;
    }
    let h = &state(agent).history;
    let target = h.index as i64 + delta;
    let len = h.entries.len() as i64;
    if (0..len).contains(&target) {
        // Una traversal dentro del documento: `popstate` (y `hashchange` si cambió el
        // fragmento).
        let old = window_url(agent);
        let entry = h.entries[target as usize].clone();
        state_mut(agent).history.index = target as usize;
        set_document_url(agent, entry.url.clone());
        state_mut(agent)
            .hooks
            .same_document_navigation(&entry.url, true);
        let data = entry
            .state
            .and_then(|s| agent.host_pinned(s))
            .unwrap_or(Value::Null);
        super::script::queue_popstate(agent, data);
        if old.fragment != entry.url.fragment {
            super::script::queue_window_event(agent, "hashchange");
        }
        return;
    }
    let outside = if target < 0 {
        target
    } else {
        target - (len - 1)
    };
    state_mut(agent).hooks.traverse(outside as i32);
}

/// "Shared history push/replace state steps" (HTML §7.4.3).
fn push_or_replace(agent: &mut Agent, a: &CallArgs<'_>, push: bool) -> JsResult<Value> {
    let data = a.arg(0);
    let current = window_url(agent);
    let new_url = match a.arg(2) {
        v if v.is_nullish() => current.clone(),
        v => {
            let s = to_rust_string(agent, &v)?;
            match Url::parse(&s, Some(&current)) {
                Ok(u) => u,
                Err(_) => return crate::dom::throw_dom(agent, bf_dom::DomException::SecurityError),
            }
        }
    };
    // "Can have its URL rewritten" (HTML §7.4.3).
    let rewritable = new_url.scheme == current.scheme
        && new_url.username == current.username
        && new_url.password == current.password
        && new_url.host == current.host
        && new_url.port == current.port
        && (matches!(current.scheme.as_str(), "http" | "https")
            || (current.scheme == "file" && new_url.path_string() == current.path_string())
            || new_url.serialize(true) == current.serialize(true));
    if !rewritable {
        return crate::dom::throw_dom(agent, bf_dom::DomException::SecurityError);
    }
    let handle = if data.is_nullish() {
        None
    } else {
        Some(agent.host_pin(data))
    };
    add_entry(agent, new_url.clone(), handle, push);
    state_mut(agent)
        .hooks
        .same_document_navigation(&new_url, !push);
    Ok(Value::Undefined)
}

fn push_state(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    push_or_replace(agent, a, true)
}

fn replace_state(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    push_or_replace(agent, a, false)
}
