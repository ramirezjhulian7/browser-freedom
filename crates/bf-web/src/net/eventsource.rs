//! `EventSource` (HTML §9.2): la conexión `text/event-stream` en un hilo (con
//! `bf_net::streaming`), el parser (§9.2.6), la entrega de eventos (§9.2.7) y la
//! reconexión con `retry` y `Last-Event-ID` (§9.2.3).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bf_dom::events::EventTarget;
use bf_infra::Url;
use bf_js::host::{HostClass, HostClassId, HostHandle, TimerHandler, TimerId};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use super::body::usv_utf8;
use super::channels::fire_message;
use super::headers::HeaderList;
use super::xhr::{fire_simple, new_target};
use super::{EventSink, NetEvent, NetRequest, classes, dom_error, state};
use crate::WebContextHandle;
use crate::dom::events::{TargetRef, with_event_handlers};

/// El tiempo de reconexión por defecto (§9.2.3, "implementation-defined"): 3 s, como los
/// navegadores.
const DEFAULT_RETRY_MS: f64 = 3000.0;

/// El estado de un `EventSource` (por id de target).
pub struct SourceState {
    /// `CONNECTING` 0, `OPEN` 1, `CLOSED` 2.
    pub ready: u8,
    pub url: Url,
    pub with_credentials: bool,
    /// El origen de la respuesta (el `origin` de los `MessageEvent`).
    origin: String,
    last_event_id: String,
    retry_ms: f64,
    // El parser.
    pending: Vec<u8>,
    started: bool,
    last_was_cr: bool,
    line: String,
    data: String,
    event_type: String,
    id_buffer: String,
    cancel: Option<Arc<AtomicBool>>,
    timer: Option<TimerId>,
    listener: u64,
    pin: Option<HostHandle>,
}

pub fn event_source_class(event_target: HostClassId) -> HostClass {
    with_event_handlers(
        HostClass::new("EventSource")
            .parent(event_target)
            .constructor(es_ctor, 1)
            .attribute("url", Some(es_url), None)
            .attribute("withCredentials", Some(es_with_credentials), None)
            .constant("CONNECTING", 0.0)
            .constant("OPEN", 1.0)
            .constant("CLOSED", 2.0)
            .attribute("readyState", Some(es_ready_state), None)
            .method("close", 0, es_close),
        &["open", "message", "error"],
    )
}

fn target_id(t: TargetRef) -> u64 {
    match t.target {
        EventTarget::Object(id) => id,
        _ => 0,
    }
}

/// `new EventSource(url, eventSourceInitDict)` (§9.2.2).
fn es_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Illegal constructor");
    };
    if a.args.is_empty() {
        return agent.type_error(
            "Failed to construct 'EventSource': 1 argument required, but only 0 present.",
        );
    }
    let s = usv_utf8(agent, &a.arg(0))?;
    let init = a.arg(1);
    let with_credentials = match &init {
        Value::Object(o) => {
            let v = agent.get(*o, &PropertyKey::from("withCredentials"), init.clone())?;
            bf_js::host::webidl::to_boolean(agent, &v)
        }
        Value::Undefined | Value::Null => false,
        _ => return agent.type_error("The provided value is not of type 'EventSourceInit'"),
    };
    let base = super::base_url(agent);
    let Ok(url) = Url::parse(&s, Some(&base)) else {
        return dom_error(
            agent,
            "SyntaxError",
            &format!(
                "Failed to construct 'EventSource': Cannot open an EventSource to '{s}'. The URL is invalid."
            ),
        );
    };
    let class = classes(agent).event_source;
    let (t, o) = new_target(agent, class, Some(nt))?;
    let id = target_id(t);
    let pin = agent.host_pin(Value::Object(o));
    let sink = super::listen(
        agent,
        Box::new(move |agent: &mut Agent, e: NetEvent| on_event(agent, t, e)),
    );
    state(agent).sources.insert(
        id,
        SourceState {
            ready: 0,
            url,
            with_credentials,
            origin: String::new(),
            last_event_id: String::new(),
            retry_ms: DEFAULT_RETRY_MS,
            pending: Vec::new(),
            started: false,
            last_was_cr: false,
            line: String::new(),
            data: String::new(),
            event_type: String::new(),
            id_buffer: String::new(),
            cancel: None,
            timer: None,
            listener: sink.id(),
            pin: Some(pin),
        },
    );
    connect(agent, id, sink);
    Ok(Value::Object(o))
}

/// Hace la petición (§9.2.2 pasos 8–14): en un hilo, entregando la respuesta por partes.
fn connect(agent: &mut Agent, id: u64, sink: EventSink) {
    let Some(s) = state(agent).sources.get_mut(&id) else {
        return;
    };
    // Una conexión nueva: el parser empieza de cero (§9.2.6).
    s.pending.clear();
    s.started = false;
    s.last_was_cr = false;
    s.line.clear();
    s.data.clear();
    s.event_type.clear();
    let url = s.url.clone();
    let last_id = s.last_event_id.clone();
    let with_credentials = s.with_credentials;
    let cancel = Arc::new(AtomicBool::new(false));
    s.cancel = Some(cancel.clone());
    let same = super::same_origin(agent, &url);
    let credentials = with_credentials || same;
    let mut headers = bf_net::Headers::default();
    headers.push("Accept", "text/event-stream");
    headers.push("Cache-Control", "no-cache");
    if !last_id.is_empty() {
        headers.push("Last-Event-ID", &last_id);
    }
    if !same {
        headers.push("Origin", &super::document_origin(agent));
    }
    let backend = super::current_backend(agent);
    let cookie = if credentials {
        backend.cookie_header(&url)
    } else {
        None
    };
    let req = NetRequest {
        url: url.clone(),
        options: bf_net::RequestOptions {
            method: "GET".into(),
            headers,
            body: None,
            redirect: bf_net::RedirectMode::Follow,
            credentials,
            referrer: Some(super::base_url(agent)),
        },
    };
    if super::is_bad_port(&url) {
        sink.push(NetEvent::End(Err("bad port".into())));
        return;
    }
    if !backend.network() || !matches!(url.scheme.as_str(), "http" | "https") {
        // Sin red por partes (runner, data:): la respuesta entera de una vez.
        let r = backend.fetch(&req);
        match r {
            Ok(res) => {
                sink.push(NetEvent::Head {
                    url: res.url_list.last().cloned().unwrap_or(url),
                    status: res.status,
                    headers: res.headers,
                });
                sink.push(NetEvent::Chunk(res.body));
                sink.push(NetEvent::End(Ok(())));
            }
            Err(e) => sink.push(NetEvent::End(Err(e))),
        }
        return;
    }
    struct Sink {
        out: EventSink,
        cancel: Arc<AtomicBool>,
    }
    impl bf_net::streaming::StreamSink for Sink {
        fn head(&mut self, h: &bf_net::streaming::StreamHead) -> bool {
            self.out.push(NetEvent::Head {
                url: h.url.clone(),
                status: h.status,
                headers: HeaderList(h.headers.0.clone()),
            });
            !self.cancel.load(Ordering::Relaxed)
        }
        fn chunk(&mut self, bytes: &[u8]) -> bool {
            if self.cancel.load(Ordering::Relaxed) {
                return false;
            }
            self.out.push(NetEvent::Chunk(bytes.to_vec()));
            true
        }
    }
    let backend2 = backend.clone();
    std::thread::spawn(move || {
        let mut s = Sink {
            out: sink.clone(),
            cancel: cancel.clone(),
        };
        let r =
            bf_net::streaming::fetch_streaming(&req.url, &req.options, cookie.as_deref(), &mut s);
        let _ = backend2;
        if !cancel.load(Ordering::Relaxed) {
            sink.push(NetEvent::End(r.map_err(|e| e.to_string())));
        }
    });
}

fn on_event(agent: &mut Agent, t: TargetRef, e: NetEvent) {
    let id = target_id(t);
    let ready = state(agent).sources.get(&id).map_or(2, |s| s.ready);
    if ready == 2 {
        return;
    }
    match e {
        NetEvent::Head {
            url,
            status,
            headers,
        } => {
            let set_cookies: Vec<String> = headers
                .0
                .iter()
                .filter(|(n, _)| n.eq_ignore_ascii_case("set-cookie"))
                .map(|(_, v)| v.clone())
                .collect();
            if !set_cookies.is_empty() {
                super::current_backend(agent).store_cookies(&url, &set_cookies);
            }
            let ok_type = headers
                .get("content-type")
                .and_then(|c| super::body::mime_essence(&c))
                .is_some_and(|e| e == "text/event-stream");
            let with_credentials = state(agent)
                .sources
                .get(&id)
                .is_some_and(|s| s.with_credentials);
            let cors_ok = super::same_origin(agent, &url) || {
                let origin = super::document_origin(agent);
                super::cors_check(&origin, &headers, with_credentials)
            };
            if status != 200 || !ok_type || !cors_ok {
                fail(agent, t);
                return;
            }
            if let Some(s) = state(agent).sources.get_mut(&id) {
                s.ready = 1;
                s.origin = url.origin().to_string();
            }
            fire_simple(agent, t, "open");
        }
        NetEvent::Chunk(bytes) => parse(agent, t, &bytes),
        NetEvent::End(_) => reestablish(agent, t),
        NetEvent::Ws(_) => {}
    }
}

/// "Fail the connection" (§9.2.3): CLOSED y `error`, sin reconectar.
fn fail(agent: &mut Agent, t: TargetRef) {
    let id = target_id(t);
    let (listener, pin) = match state(agent).sources.get_mut(&id) {
        Some(s) => {
            s.ready = 2;
            if let Some(c) = s.cancel.take() {
                c.store(true, Ordering::Relaxed);
            }
            (s.listener, s.pin.take())
        }
        None => return,
    };
    fire_simple(agent, t, "error");
    super::unlisten(agent, listener);
    if let Some(p) = pin {
        agent.host_unpin(p);
    }
}

/// "Reestablish the connection" (§9.2.3): CONNECTING, `error` y reintento tras `retry`.
fn reestablish(agent: &mut Agent, t: TargetRef) {
    let id = target_id(t);
    let retry = match state(agent).sources.get_mut(&id) {
        Some(s) if s.ready != 2 => {
            s.ready = 0;
            s.retry_ms
        }
        _ => return,
    };
    fire_simple(agent, t, "error");
    if state(agent).sources.get(&id).is_none_or(|s| s.ready != 0) {
        return;
    }
    let realm = WebContextHandle::current(agent).realm;
    let f = agent.create_builtin_with_slots(retry_tick, 0, "", vec![Value::Number(id as f64)]);
    let timer = agent.set_timer(
        realm,
        TimerHandler::Callback(Value::Object(f)),
        retry,
        Vec::new(),
        false,
    );
    if let Some(s) = state(agent).sources.get_mut(&id) {
        s.timer = Some(timer);
    }
}

fn retry_tick(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = match &agent.heap[a.callee].kind {
        bf_js::object::ObjectKind::Builtin(b) => {
            b.slots.first().and_then(Value::as_number).unwrap_or(0.0) as u64
        }
        _ => return Ok(Value::Undefined),
    };
    let t = TargetRef {
        doc: 0,
        target: EventTarget::Object(id),
    };
    let listener = match state(agent).sources.get_mut(&id) {
        Some(s) if s.ready == 0 => {
            s.timer = None;
            s.listener
        }
        _ => return Ok(Value::Undefined),
    };
    super::unlisten(agent, listener);
    let sink = super::listen(
        agent,
        Box::new(move |agent: &mut Agent, e: NetEvent| on_event(agent, t, e)),
    );
    if let Some(s) = state(agent).sources.get_mut(&id) {
        s.listener = sink.id();
    }
    connect(agent, id, sink);
    Ok(Value::Undefined)
}

/// El parser de `text/event-stream` (§9.2.6) sobre `bytes` (UTF-8 por partes).
fn parse(agent: &mut Agent, t: TargetRef, bytes: &[u8]) {
    let id = target_id(t);
    let text = {
        let Some(s) = state(agent).sources.get_mut(&id) else {
            return;
        };
        s.pending.extend_from_slice(bytes);
        // Lo que se puede decodificar ya (sin partir una secuencia UTF-8).
        let valid = match std::str::from_utf8(&s.pending) {
            Ok(_) => s.pending.len(),
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            Err(_) => s.pending.len(),
        };
        let chunk: Vec<u8> = s.pending.drain(..valid).collect();
        let mut text = String::from_utf8_lossy(&chunk).into_owned();
        if !s.started && !text.is_empty() {
            s.started = true;
            if let Some(rest) = text.strip_prefix('\u{FEFF}') {
                text = rest.to_owned();
            }
        }
        text
    };
    for c in text.chars() {
        let mut complete = None;
        {
            let Some(s) = state(agent).sources.get_mut(&id) else {
                return;
            };
            match c {
                '\n' if s.last_was_cr => s.last_was_cr = false,
                '\r' | '\n' => {
                    s.last_was_cr = c == '\r';
                    complete = Some(std::mem::take(&mut s.line));
                }
                _ => {
                    s.last_was_cr = false;
                    s.line.push(c);
                }
            }
        }
        if let Some(line) = complete {
            process_line(agent, t, &line);
        }
    }
}

/// Una línea del stream (§9.2.6).
fn process_line(agent: &mut Agent, t: TargetRef, line: &str) {
    let id = target_id(t);
    if line.is_empty() {
        dispatch(agent, t);
        return;
    }
    if line.starts_with(':') {
        return;
    }
    let (field, value) = match line.split_once(':') {
        Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
        None => (line, ""),
    };
    let Some(s) = state(agent).sources.get_mut(&id) else {
        return;
    };
    match field {
        "event" => s.event_type = value.to_owned(),
        "data" => {
            s.data.push_str(value);
            s.data.push('\n');
        }
        "id" => {
            if !value.contains('\0') {
                s.id_buffer = value.to_owned();
            }
        }
        "retry" => {
            if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
                if let Ok(ms) = value.parse::<f64>() {
                    s.retry_ms = ms;
                }
            }
        }
        _ => {}
    }
}

/// "Dispatch the event" (§9.2.6).
fn dispatch(agent: &mut Agent, t: TargetRef) {
    let id = target_id(t);
    let (data, event_type, last_id, origin) = {
        let Some(s) = state(agent).sources.get_mut(&id) else {
            return;
        };
        s.last_event_id = s.id_buffer.clone();
        if s.data.is_empty() {
            s.event_type.clear();
            return;
        }
        let mut data = std::mem::take(&mut s.data);
        if data.ends_with('\n') {
            data.pop();
        }
        let ty = std::mem::take(&mut s.event_type);
        let ty = if ty.is_empty() {
            "message".to_owned()
        } else {
            ty
        };
        (data, ty, s.last_event_id.clone(), s.origin.clone())
    };
    if state(agent).sources.get(&id).is_some_and(|s| s.ready == 2) {
        return;
    }
    fire_message(
        agent,
        t,
        &event_type,
        Value::str(&data),
        &origin,
        &last_id,
        Vec::new(),
    );
}

fn this_es(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<u64> {
    let class = classes(agent).event_source;
    match agent.host_unwrap::<TargetRef>(&a.this).copied() {
        Some(t) if agent.host_is_instance(&a.this, class) => Ok(target_id(t)),
        _ => agent.type_error("Illegal invocation"),
    }
}

fn es_url(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_es(agent, a)?;
    let u = state(agent)
        .sources
        .get(&id)
        .map(|s| s.url.href())
        .unwrap_or_default();
    Ok(Value::str(&u))
}

fn es_with_credentials(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_es(agent, a)?;
    Ok(Value::Boolean(
        state(agent)
            .sources
            .get(&id)
            .is_some_and(|s| s.with_credentials),
    ))
}

fn es_ready_state(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_es(agent, a)?;
    Ok(Value::Number(f64::from(
        state(agent).sources.get(&id).map_or(2, |s| s.ready),
    )))
}

/// `close()` (§9.2.2).
fn es_close(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_es(agent, a)?;
    let (listener, timer, pin) = match state(agent).sources.get_mut(&id) {
        Some(s) => {
            s.ready = 2;
            if let Some(c) = s.cancel.take() {
                c.store(true, Ordering::Relaxed);
            }
            (s.listener, s.timer.take(), s.pin.take())
        }
        None => return Ok(Value::Undefined),
    };
    super::unlisten(agent, listener);
    if let Some(t) = timer {
        agent.clear_timer(t);
    }
    if let Some(p) = pin {
        agent.host_unpin(p);
    }
    Ok(Value::Undefined)
}
