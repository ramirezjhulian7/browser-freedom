//! La interfaz `WebSocket` (WebSockets Standard) y `CloseEvent`, sobre el cliente RFC 6455
//! de `bf-net` (`bf_net::websocket`): la conexión corre en su hilo y los eventos vuelven por
//! [`super::listen`] en tareas *networking*.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use bf_dom::events::{Event, EventInit, EventTarget};
use bf_infra::Url;
use bf_js::builtins::web::encoding::{buffer_source_bytes, new_array_buffer};
use bf_js::host::{HostClass, HostClassId, HostHandle};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};
use bf_net::websocket::{WsClient, WsCommand, WsEvent, WsOptions};

use super::body::usv_utf8;
use super::channels::{dispatch, fire_message};
use super::xhr::{fire_simple, new_target};
use super::{NetEvent, classes, dom_error, state};
use crate::dom::events::{EventData, EventIface, TargetRef, with_event_handlers};

/// El estado de un `WebSocket` (por id de target).
pub struct SocketState {
    /// `CONNECTING` 0, `OPEN` 1, `CLOSING` 2, `CLOSED` 3.
    pub ready: u8,
    pub url: Url,
    pub protocol: String,
    pub extensions: String,
    pub buffered: f64,
    pub binary_type_blob: bool,
    client: Option<WsClient>,
    /// `close()` antes de abrirse: se falla la conexión (error + close 1006).
    failed_connecting: bool,
    listener: u64,
    pin: Option<HostHandle>,
}

pub fn websocket_class(event_target: HostClassId) -> HostClass {
    with_event_handlers(
        HostClass::new("WebSocket")
            .parent(event_target)
            .constructor(ws_ctor, 1)
            .constant("CONNECTING", 0.0)
            .constant("OPEN", 1.0)
            .constant("CLOSING", 2.0)
            .constant("CLOSED", 3.0)
            .attribute("url", Some(ws_url), None)
            .attribute("readyState", Some(ws_ready_state), None)
            .attribute("bufferedAmount", Some(ws_buffered), None)
            .attribute("extensions", Some(ws_extensions), None)
            .attribute("protocol", Some(ws_protocol), None)
            .method("close", 0, ws_close)
            .attribute("binaryType", Some(ws_binary_type), Some(ws_set_binary_type))
            .method("send", 1, ws_send),
        &["open", "error", "close", "message"],
    )
}

pub fn close_event_class(event: HostClassId) -> HostClass {
    HostClass::new("CloseEvent")
        .parent(event)
        .constructor(close_event_ctor, 1)
        .attribute("wasClean", Some(ce_was_clean), None)
        .attribute("code", Some(ce_code), None)
        .attribute("reason", Some(ce_reason), None)
}

type Obj = bf_js::heap::Gc<bf_js::object::Object>;

fn target_id(t: TargetRef) -> u64 {
    match t.target {
        EventTarget::Object(id) => id,
        _ => 0,
    }
}

/// El URL de la conexión con el esquema HTTP equivalente (para cookies y bad ports).
fn http_url(url: &Url) -> Url {
    let mut u = url.clone();
    u.scheme = if url.scheme == "wss" {
        "https".into()
    } else {
        "http".into()
    };
    u
}

/// `new WebSocket(url, protocols)` (WebSockets §3.1).
fn ws_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Illegal constructor");
    };
    if a.args.is_empty() {
        return agent.type_error(
            "Failed to construct 'WebSocket': 1 argument required, but only 0 present.",
        );
    }
    let s = usv_utf8(agent, &a.arg(0))?;
    let base = super::base_url(agent);
    let Ok(mut url) = Url::parse(&s, Some(&base)) else {
        return dom_error(
            agent,
            "SyntaxError",
            &format!("Failed to construct 'WebSocket': The URL '{s}' is invalid."),
        );
    };
    match url.scheme.as_str() {
        "http" => url.scheme = "ws".into(),
        "https" => url.scheme = "wss".into(),
        "ws" | "wss" => {}
        other => {
            return dom_error(
                agent,
                "SyntaxError",
                &format!(
                    "Failed to construct 'WebSocket': The URL's scheme must be either 'http', 'https', 'ws', or 'wss'. '{other}' is not allowed."
                ),
            );
        }
    }
    if url.fragment.is_some() {
        return dom_error(
            agent,
            "SyntaxError",
            "Failed to construct 'WebSocket': The URL contains a fragment identifier.",
        );
    }
    // Los subprotocolos: un string o una secuencia, sin repetidos y con la sintaxis de token.
    let mut protocols = Vec::new();
    let p = a.arg(1);
    if !p.is_undefined() {
        let iter = match &p {
            Value::Object(o) => {
                let k = PropertyKey::Symbol(agent.well_known.iterator);
                let m = agent.get(*o, &k, p.clone())?;
                agent.is_callable(&m).then_some(m)
            }
            _ => None,
        };
        let items = match iter {
            Some(m) => super::headers::collect_iterable(agent, &p, &m)?,
            None => vec![p.clone()],
        };
        for it in items {
            let v = agent.to_string(&it)?.to_string();
            if !super::headers::is_token(&v) || protocols.contains(&v) {
                return dom_error(
                    agent,
                    "SyntaxError",
                    &format!("Failed to construct 'WebSocket': The subprotocol '{v}' is invalid."),
                );
            }
            protocols.push(v);
        }
    }
    let class = classes(agent).websocket;
    let (t, o) = new_target(agent, class, Some(nt))?;
    let id = target_id(t);
    let pin = agent.host_pin(Value::Object(o));
    let origin = super::document_origin(agent);
    let backend = super::current_backend(agent);
    let cookie = backend.cookie_header(&http_url(&url));
    let sink = super::listen(
        agent,
        Box::new(move |agent: &mut Agent, e: NetEvent| {
            if let NetEvent::Ws(e) = e {
                on_event(agent, t, e);
            }
        }),
    );
    state(agent).sockets.insert(
        id,
        SocketState {
            ready: 0,
            url: url.clone(),
            protocol: String::new(),
            extensions: String::new(),
            buffered: 0.0,
            binary_type_blob: true,
            client: None,
            failed_connecting: false,
            listener: sink.id(),
            pin: Some(pin),
        },
    );
    if super::is_bad_port(&http_url(&url)) || !backend.network() {
        // Sin red (runner) o puerto prohibido: falla la conexión de forma asíncrona.
        sink.push(NetEvent::Ws(WsEvent::Error("connection refused".into())));
        sink.push(NetEvent::Ws(WsEvent::Closed {
            code: 1006,
            reason: String::new(),
            clean: false,
        }));
        return Ok(Value::Object(o));
    }
    let client = bf_net::websocket::open(
        &url,
        WsOptions {
            protocols,
            origin,
            cookie,
        },
        Box::new(move |e| sink.push(NetEvent::Ws(e))),
    );
    if let Some(s) = state(agent).sockets.get_mut(&id) {
        s.client = Some(client);
    }
    let _ = backend;
    Ok(Value::Object(o))
}

/// Los eventos de la conexión (WebSockets §4: "the WebSocket connection is established",
/// "a WebSocket message has been received", "the WebSocket connection is closed").
fn on_event(agent: &mut Agent, t: TargetRef, e: WsEvent) {
    let id = target_id(t);
    match e {
        WsEvent::Open {
            protocol,
            extensions,
            set_cookies,
        } => {
            let (url, fail) = match state(agent).sockets.get_mut(&id) {
                Some(s) => {
                    if s.failed_connecting {
                        if let Some(c) = &s.client {
                            c.send(WsCommand::Close(None, String::new()));
                        }
                        (s.url.clone(), true)
                    } else {
                        s.ready = 1;
                        s.protocol = protocol;
                        s.extensions = extensions;
                        (s.url.clone(), false)
                    }
                }
                None => return,
            };
            if !set_cookies.is_empty() {
                super::current_backend(agent).store_cookies(&http_url(&url), &set_cookies);
            }
            if !fail {
                fire_simple(agent, t, "open");
            }
        }
        WsEvent::Text(s) => {
            let (open, origin) = socket_origin(agent, id);
            if open {
                fire_message(agent, t, "message", Value::str(&s), &origin, "", Vec::new());
            }
        }
        WsEvent::Binary(b) => {
            let (open, origin) = socket_origin(agent, id);
            if !open {
                return;
            }
            let blob = state(agent)
                .sockets
                .get(&id)
                .is_some_and(|s| s.binary_type_blob);
            let data = if blob {
                super::blob::create(
                    agent,
                    super::blob::BlobData {
                        bytes: Arc::new(b),
                        mime: String::new(),
                        file: None,
                    },
                )
            } else {
                Value::Object(new_array_buffer(agent, b))
            };
            fire_message(agent, t, "message", data, &origin, "", Vec::new());
        }
        WsEvent::Sent(n) => {
            if let Some(s) = state(agent).sockets.get_mut(&id) {
                s.buffered = (s.buffered - n as f64).max(0.0);
            }
        }
        WsEvent::Error(_) => {
            let closed = state(agent).sockets.get(&id).is_none_or(|s| s.ready == 3);
            if !closed {
                if let Some(s) = state(agent).sockets.get_mut(&id) {
                    s.ready = 3;
                }
                fire_simple(agent, t, "error");
            }
        }
        WsEvent::Closed {
            code,
            reason,
            clean,
        } => {
            let Some(s) = state(agent).sockets.get_mut(&id) else {
                return;
            };
            let (code, reason, clean, error_first) = if s.failed_connecting {
                (1006, String::new(), false, true)
            } else {
                (code, reason, clean, false)
            };
            let already_errored = s.ready == 3;
            s.ready = 3;
            s.client = None;
            let listener = s.listener;
            let pin = s.pin.take();
            // Un cierre anormal (1006) avisa con `error` antes del `close`, como los navegadores.
            if (error_first || (!clean && code == 1006)) && !already_errored {
                fire_simple(agent, t, "error");
            }
            fire_close(agent, t, clean, code, &reason);
            super::unlisten(agent, listener);
            if let Some(p) = pin {
                agent.host_unpin(p);
            }
        }
    }
}

/// ¿Está abierto? y el origen serializado del URL (el `origin` de los `MessageEvent`).
fn socket_origin(agent: &mut Agent, id: u64) -> (bool, String) {
    match state(agent).sockets.get(&id) {
        Some(s) => (
            s.ready == 1,
            http_url(&s.url)
                .origin()
                .to_string()
                .replacen("http", "ws", 1),
        ),
        None => (false, String::new()),
    }
}

/// Dispara un `CloseEvent` de confianza.
fn fire_close(agent: &mut Agent, t: TargetRef, clean: bool, code: u16, reason: &str) {
    let class = classes(agent).close_event;
    let mut ev = Event::new("close", EventInit::default());
    ev.is_trusted = true;
    let o = agent.host_create(
        class,
        Box::new(EventData {
            cell: Rc::new(RefCell::new(ev)),
            iface: EventIface::Event,
            doc: Cell::new(0),
        }),
    );
    if let Some(s) = agent.host_slots_mut(o) {
        *s = vec![
            Value::Boolean(clean),
            Value::Number(f64::from(code)),
            Value::str(reason),
        ];
    }
    dispatch(agent, t, o);
}

fn this_ws(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<(TargetRef, u64)> {
    let class = classes(agent).websocket;
    match agent.host_unwrap::<TargetRef>(&a.this).copied() {
        Some(t) if agent.host_is_instance(&a.this, class) => Ok((t, target_id(t))),
        _ => agent.type_error("Illegal invocation"),
    }
}

fn with_ws<R>(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    f: impl FnOnce(&SocketState) -> R,
) -> JsResult<R> {
    let (_, id) = this_ws(agent, a)?;
    match state(agent).sockets.get(&id) {
        Some(s) => Ok(f(s)),
        None => agent.type_error("Illegal invocation"),
    }
}

fn ws_url(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let u = with_ws(agent, a, |s| s.url.href())?;
    Ok(Value::str(&u))
}

fn ws_ready_state(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(f64::from(with_ws(agent, a, |s| s.ready)?)))
}

fn ws_buffered(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(with_ws(agent, a, |s| s.buffered)?))
}

fn ws_extensions(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = with_ws(agent, a, |s| s.extensions.clone())?;
    Ok(Value::str(&v))
}

fn ws_protocol(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = with_ws(agent, a, |s| s.protocol.clone())?;
    Ok(Value::str(&v))
}

fn ws_binary_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let blob = with_ws(agent, a, |s| s.binary_type_blob)?;
    Ok(Value::str(if blob { "blob" } else { "arraybuffer" }))
}

fn ws_set_binary_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (_, id) = this_ws(agent, a)?;
    let v = agent.to_string(&a.arg(0))?.to_string();
    let blob = match v.as_str() {
        "blob" => true,
        "arraybuffer" => false,
        _ => return Ok(Value::Undefined),
    };
    if let Some(s) = state(agent).sockets.get_mut(&id) {
        s.binary_type_blob = blob;
    }
    Ok(Value::Undefined)
}

/// `close(code, reason)` (WebSockets §3.1).
fn ws_close(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (_, id) = this_ws(agent, a)?;
    let code = if a.arg(0).is_undefined() {
        None
    } else {
        let n = agent.to_number(&a.arg(0))?;
        // `[Clamp] unsigned short`.
        let c = if n.is_nan() {
            0.0
        } else {
            n.clamp(0.0, 65535.0).round_ties_even()
        };
        let c = c as u16;
        if c != 1000 && !(3000..=4999).contains(&c) {
            return dom_error(
                agent,
                "InvalidAccessError",
                &format!(
                    "Failed to execute 'close' on 'WebSocket': The close code must be either 1000, or between 3000 and 4999. {c} is neither."
                ),
            );
        }
        Some(c)
    };
    let reason = if a.arg(1).is_undefined() {
        String::new()
    } else {
        usv_utf8(agent, &a.arg(1))?
    };
    if reason.len() > 123 {
        return dom_error(
            agent,
            "SyntaxError",
            "Failed to execute 'close' on 'WebSocket': The close reason must not be greater than 123 UTF-8 bytes.",
        );
    }
    let Some(s) = state(agent).sockets.get_mut(&id) else {
        return Ok(Value::Undefined);
    };
    match s.ready {
        2 | 3 => {}
        0 => {
            // "Fail the WebSocket connection".
            s.failed_connecting = true;
            s.ready = 2;
            if let Some(c) = &s.client {
                c.send(WsCommand::Close(None, String::new()));
            }
        }
        _ => {
            s.ready = 2;
            let code = code.or((!reason.is_empty()).then_some(1000)).or(Some(1000));
            if let Some(c) = &s.client {
                c.send(WsCommand::Close(code, reason));
            }
        }
    }
    Ok(Value::Undefined)
}

/// `send(data)` (WebSockets §3.1).
fn ws_send(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (_, id) = this_ws(agent, a)?;
    let ready = state(agent).sockets.get(&id).map_or(3, |s| s.ready);
    if ready == 0 {
        return dom_error(
            agent,
            "InvalidStateError",
            "Failed to execute 'send' on 'WebSocket': Still in CONNECTING state.",
        );
    }
    let data = a.arg(0);
    let cmd = if let Some(b) = super::blob::data(agent, &data) {
        WsCommand::Binary(b.bytes.to_vec())
    } else if let Some(bytes) = buffer_source_bytes(agent, &data) {
        WsCommand::Binary(bytes)
    } else {
        WsCommand::Text(usv_utf8(agent, &data)?)
    };
    let len = match &cmd {
        WsCommand::Text(s) => s.len(),
        WsCommand::Binary(b) => b.len(),
        WsCommand::Close(..) => 0,
    };
    if let Some(s) = state(agent).sockets.get_mut(&id) {
        s.buffered += len as f64;
        if s.ready == 1
            && let Some(c) = &s.client
        {
            c.send(cmd);
        }
    }
    Ok(Value::Undefined)
}

// --- CloseEvent (WebSockets §5) ---

fn close_event_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Illegal constructor");
    };
    if a.args.is_empty() {
        return agent.type_error(
            "Failed to construct 'CloseEvent': 1 argument required, but only 0 present.",
        );
    }
    let type_ = crate::dom::to_dom_string(agent, &a.arg(0))?;
    let d = a.arg(1);
    let get = |agent: &mut Agent, n: &str| -> JsResult<Value> {
        match &d {
            Value::Object(o) => agent.get(*o, &PropertyKey::from(n), d.clone()),
            Value::Undefined | Value::Null => Ok(Value::Undefined),
            _ => agent.type_error("The provided value is not of type 'CloseEventInit'"),
        }
    };
    let bubbles = get(agent, "bubbles")?;
    let bubbles = bf_js::host::webidl::to_boolean(agent, &bubbles);
    let cancelable = get(agent, "cancelable")?;
    let cancelable = bf_js::host::webidl::to_boolean(agent, &cancelable);
    let composed = get(agent, "composed")?;
    let composed = bf_js::host::webidl::to_boolean(agent, &composed);
    let code = get(agent, "code")?;
    let code = if code.is_undefined() {
        0.0
    } else {
        let n = agent.to_number(&code)?;
        if n.is_finite() {
            n.trunc().rem_euclid(65536.0)
        } else {
            0.0
        }
    };
    let reason = get(agent, "reason")?;
    let reason = if reason.is_undefined() {
        String::new()
    } else {
        usv_utf8(agent, &reason)?
    };
    let was_clean = get(agent, "wasClean")?;
    let was_clean = bf_js::host::webidl::to_boolean(agent, &was_clean);
    let class = classes(agent).close_event;
    let o = agent.host_create_from_new_target(
        nt,
        class,
        None,
        Box::new(EventData {
            cell: Rc::new(RefCell::new(Event::new(
                type_,
                EventInit {
                    bubbles,
                    cancelable,
                    composed,
                },
            ))),
            iface: EventIface::Event,
            doc: Cell::new(0),
        }),
    )?;
    if let Some(s) = agent.host_slots_mut(o) {
        *s = vec![
            Value::Boolean(was_clean),
            Value::Number(code),
            Value::str(&reason),
        ];
    }
    Ok(Value::Object(o))
}

fn ce_slot(agent: &mut Agent, a: &CallArgs<'_>, i: usize) -> JsResult<Value> {
    let class = classes(agent).close_event;
    if !agent.host_is_instance(&a.this, class) {
        return agent.type_error("Illegal invocation");
    }
    let Value::Object(o) = &a.this else {
        return agent.type_error("Illegal invocation");
    };
    let o: Obj = *o;
    Ok(agent
        .host_slots_mut(o)
        .and_then(|s| s.get(i).cloned())
        .unwrap_or(Value::Undefined))
}

fn ce_was_clean(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    ce_slot(agent, a, 0)
}

fn ce_code(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    ce_slot(agent, a, 1)
}

fn ce_reason(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    ce_slot(agent, a, 2)
}
