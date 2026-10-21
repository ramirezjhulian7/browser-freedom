//! XMLHttpRequest Standard: `XMLHttpRequestEventTarget`, `XMLHttpRequest` (§3),
//! `XMLHttpRequestUpload` y `ProgressEvent` (§7). También las utilidades para disparar
//! eventos de progreso en un `EventTarget` de `bf-dom` (las usa `FileReader`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use bf_dom::events::{Event, EventInit, EventTarget};
use bf_infra::Url;
use bf_js::host::{HostClass, HostClassId, HostHandle, TimerHandler, TimerId};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, Throw};

use super::body::{self, decode_with, mime_charset, mime_essence};
use super::headers::{self, HeaderList};
use super::{NetRequest, NetResult, classes, dom_error, state};
use crate::dom::events::{
    EventData, EventIface, TargetRef, dispatch_js_event, object_key, with_event_handlers,
};
use crate::{NodeRef, WebContextHandle};

// --- Eventos ---

/// Un `EventTarget` nuevo de `bf-dom` envuelto en un objeto de `class` (con identidad).
pub fn new_target(
    agent: &mut Agent,
    class: HostClassId,
    nt: Option<bf_js::heap::Gc<bf_js::object::Object>>,
) -> JsResult<(TargetRef, bf_js::heap::Gc<bf_js::object::Object>)> {
    let t = crate::with_doc_mut(agent, 0, bf_dom::Document::create_event_target);
    let EventTarget::Object(id) = t else {
        unreachable!("target no-nodo")
    };
    let tr = TargetRef { doc: 0, target: t };
    let key = object_key(0, id);
    let o = match nt {
        Some(nt) => agent.host_create_from_new_target(nt, class, Some(key), Box::new(tr))?,
        None => {
            let realm = agent.current_realm;
            agent.host_wrap(realm, key, class, Box::new(tr))
        }
    };
    Ok((tr, o))
}

fn target_id(t: TargetRef) -> u64 {
    match t.target {
        EventTarget::Object(id) => id,
        _ => 0,
    }
}

fn dispatch(agent: &mut Agent, t: TargetRef, o: bf_js::heap::Gc<bf_js::object::Object>) {
    let pin = agent.host_pin(Value::Object(o));
    let _ = dispatch_js_event(agent, t.doc, t.target, o, false);
    agent.host_unpin(pin);
}

fn trusted(type_: &str) -> Rc<RefCell<Event>> {
    let mut ev = Event::new(type_, EventInit::default());
    ev.is_trusted = true;
    Rc::new(RefCell::new(ev))
}

/// "Fire a progress event" (XHR §3.7.7): un `ProgressEvent` de confianza en `t`.
pub fn fire_progress(
    agent: &mut Agent,
    t: TargetRef,
    type_: &str,
    computable: bool,
    loaded: f64,
    total: f64,
) {
    let class = classes(agent).progress_event;
    let o = agent.host_create(
        class,
        Box::new(EventData {
            cell: trusted(type_),
            iface: EventIface::Event,
            doc: Cell::new(t.doc),
        }),
    );
    if let Some(s) = agent.host_slots_mut(o) {
        *s = vec![
            Value::Boolean(computable),
            Value::Number(loaded),
            Value::Number(total),
        ];
    }
    dispatch(agent, t, o);
}

/// "Fire an event" (DOM §2.10): un `Event` de confianza en `t`.
pub fn fire_simple(agent: &mut Agent, t: TargetRef, type_: &str) {
    let o =
        crate::dom::events::create_event_object(agent, EventIface::Event, trusted(type_), t.doc);
    dispatch(agent, t, o);
}

// --- ProgressEvent (XHR §7) ---

pub fn progress_event_class(event: HostClassId) -> HostClass {
    HostClass::new("ProgressEvent")
        .parent(event)
        .constructor(progress_event_ctor, 1)
        .attribute("lengthComputable", Some(pe_length_computable), None)
        .attribute("loaded", Some(pe_loaded), None)
        .attribute("total", Some(pe_total), None)
}

fn dict_member(agent: &mut Agent, d: &Value, name: &str) -> JsResult<Value> {
    match d {
        Value::Object(o) => agent.get(*o, &PropertyKey::from(name), d.clone()),
        Value::Undefined | Value::Null => Ok(Value::Undefined),
        _ => agent.type_error("The provided value is not of type 'ProgressEventInit'"),
    }
}

/// `unsigned long long` (WebIDL §3.2.4.10): ToNumber, truncado y módulo 2^64.
fn to_ull(agent: &mut Agent, v: &Value) -> JsResult<f64> {
    if v.is_undefined() {
        return Ok(0.0);
    }
    let n = agent.to_number(v)?;
    if !n.is_finite() {
        return Ok(0.0);
    }
    Ok(n.trunc().rem_euclid(18_446_744_073_709_551_616.0))
}

fn progress_event_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Illegal constructor");
    };
    if a.args.is_empty() {
        return agent.type_error(
            "Failed to construct 'ProgressEvent': 1 argument required, but only 0 present.",
        );
    }
    let type_ = crate::dom::to_dom_string(agent, &a.arg(0))?;
    let d = a.arg(1);
    let flag = |agent: &mut Agent, n: &str| -> JsResult<bool> {
        let v = dict_member(agent, &d, n)?;
        Ok(bf_js::host::webidl::to_boolean(agent, &v))
    };
    let bubbles = flag(agent, "bubbles")?;
    let cancelable = flag(agent, "cancelable")?;
    let composed = flag(agent, "composed")?;
    let computable = flag(agent, "lengthComputable")?;
    let loaded = dict_member(agent, &d, "loaded")?;
    let loaded = to_ull(agent, &loaded)?;
    let total = dict_member(agent, &d, "total")?;
    let total = to_ull(agent, &total)?;
    let ev = Event::new(
        type_,
        EventInit {
            bubbles,
            cancelable,
            composed,
        },
    );
    let class = classes(agent).progress_event;
    let o = agent.host_create_from_new_target(
        nt,
        class,
        None,
        Box::new(EventData {
            cell: Rc::new(RefCell::new(ev)),
            iface: EventIface::Event,
            doc: Cell::new(0),
        }),
    )?;
    if let Some(s) = agent.host_slots_mut(o) {
        *s = vec![
            Value::Boolean(computable),
            Value::Number(loaded),
            Value::Number(total),
        ];
    }
    Ok(Value::Object(o))
}

fn pe_slot(agent: &mut Agent, a: &CallArgs<'_>, i: usize) -> JsResult<Value> {
    if agent.host_unwrap::<EventData>(&a.this).is_none() {
        return agent.type_error("Illegal invocation");
    }
    let Value::Object(o) = &a.this else {
        return agent.type_error("Illegal invocation");
    };
    Ok(agent
        .host_slots_mut(*o)
        .and_then(|s| s.get(i).cloned())
        .unwrap_or(Value::Number(0.0)))
}

fn pe_length_computable(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    pe_slot(agent, a, 0)
}

fn pe_loaded(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    pe_slot(agent, a, 1)
}

fn pe_total(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    pe_slot(agent, a, 2)
}

// --- XMLHttpRequest ---

/// La respuesta de un XMLHttpRequest (ya filtrada).
#[derive(Debug, Clone)]
pub struct XhrResponse {
    pub url: Option<Url>,
    pub status: u16,
    pub status_text: String,
    pub headers: HeaderList,
    pub body: Vec<u8>,
}

/// El estado de un XMLHttpRequest (XHR §3).
#[derive(Debug, Clone, Default)]
pub struct XhrState {
    /// `UNSENT` 0 … `DONE` 4.
    pub ready: u8,
    pub send_flag: bool,
    pub sync: bool,
    pub method: String,
    pub url: Option<Url>,
    pub headers: HeaderList,
    pub timeout: u32,
    pub with_credentials: bool,
    pub upload_complete: bool,
    /// ¿Hay cuerpo? (para los eventos del upload).
    pub has_body: bool,
    pub body_len: f64,
    /// `None` = error de red / sin respuesta.
    pub response: Option<XhrResponse>,
    pub response_type: String,
    pub override_mime: Option<String>,
    /// Sube con cada `open()`, `abort()` o fin de la petición (descarta los resultados viejos).
    pub generation: u64,
    pub net: Option<u64>,
    pub timer: Option<TimerId>,
    pub timed_out: bool,
    /// El wrapper fijado mientras hay una petición asíncrona en vuelo.
    pub pin: Option<HostHandle>,
}

pub fn xhr_event_target_class(event_target: HostClassId) -> HostClass {
    with_event_handlers(
        HostClass::new("XMLHttpRequestEventTarget").parent(event_target),
        &[
            "loadstart",
            "progress",
            "abort",
            "error",
            "load",
            "timeout",
            "loadend",
        ],
    )
}

pub fn upload_class(xhr_event_target: HostClassId) -> HostClass {
    HostClass::new("XMLHttpRequestUpload").parent(xhr_event_target)
}

pub fn xhr_class(xhr_event_target: HostClassId) -> HostClass {
    with_event_handlers(
        HostClass::new("XMLHttpRequest")
            .parent(xhr_event_target)
            .constructor(xhr_ctor, 0)
            .constant("UNSENT", 0.0)
            .constant("OPENED", 1.0)
            .constant("HEADERS_RECEIVED", 2.0)
            .constant("LOADING", 3.0)
            .constant("DONE", 4.0)
            .attribute("readyState", Some(ready_state), None)
            .method("open", 2, open)
            .method("setRequestHeader", 2, set_request_header)
            .attribute("timeout", Some(get_timeout), Some(set_timeout))
            .attribute(
                "withCredentials",
                Some(get_with_credentials),
                Some(set_with_credentials),
            )
            .attribute("upload", Some(upload), None)
            .method("send", 0, send)
            .method("abort", 0, abort)
            .attribute("responseURL", Some(response_url), None)
            .attribute("status", Some(status), None)
            .attribute("statusText", Some(status_text), None)
            .method("getResponseHeader", 1, get_response_header)
            .method("getAllResponseHeaders", 0, get_all_response_headers)
            .method("overrideMimeType", 1, override_mime_type)
            .attribute(
                "responseType",
                Some(get_response_type),
                Some(set_response_type),
            )
            .attribute("response", Some(response), None)
            .attribute("responseText", Some(response_text), None)
            .attribute("responseXML", Some(response_xml), None),
        &["readystatechange"],
    )
}

fn xhr_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Illegal constructor");
    };
    let class = classes(agent).xhr;
    let (t, o) = new_target(agent, class, Some(nt))?;
    if let Some(s) = agent.host_slots_mut(o) {
        *s = vec![Value::Null, Value::Null];
    }
    state(agent).xhrs.insert(target_id(t), XhrState::default());
    Ok(Value::Object(o))
}

type Obj = bf_js::heap::Gc<bf_js::object::Object>;

fn this_xhr(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<(TargetRef, Obj)> {
    let class = classes(agent).xhr;
    match (agent.host_unwrap::<TargetRef>(&a.this).copied(), &a.this) {
        (Some(t), Value::Object(o)) if agent.host_is_instance(&a.this, class) => Ok((t, *o)),
        _ => agent.type_error("Illegal invocation"),
    }
}

fn st(agent: &mut Agent, t: TargetRef) -> XhrState {
    state(agent)
        .xhrs
        .get(&target_id(t))
        .cloned()
        .unwrap_or_default()
}

fn with_st<R>(agent: &mut Agent, t: TargetRef, f: impl FnOnce(&mut XhrState) -> R) -> R {
    let s = state(agent).xhrs.entry(target_id(t)).or_default();
    f(s)
}

fn set_slot(agent: &mut Agent, o: Obj, i: usize, v: Value) {
    if let Some(s) = agent.host_slots_mut(o) {
        while s.len() <= i {
            s.push(Value::Null);
        }
        s[i] = v;
    }
}

fn get_slot(agent: &mut Agent, o: Obj, i: usize) -> Value {
    agent
        .host_slots_mut(o)
        .and_then(|s| s.get(i).cloned())
        .unwrap_or(Value::Null)
}

fn ready_state(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (t, _) = this_xhr(agent, a)?;
    Ok(Value::Number(f64::from(st(agent, t).ready)))
}

/// "Terminate" la petición en curso: descarta su resultado y su temporizador.
fn terminate(agent: &mut Agent, t: TargetRef) {
    let (net, timer, pin) = with_st(agent, t, |s| {
        s.generation += 1;
        (s.net.take(), s.timer.take(), s.pin.take())
    });
    if let Some(n) = net {
        super::cancel(agent, n);
    }
    if let Some(tm) = timer {
        agent.clear_timer(tm);
    }
    if let Some(p) = pin {
        agent.host_unpin(p);
    }
}

/// `open(method, url, async, username, password)` (XHR §3.5.1).
fn open(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (t, o) = this_xhr(agent, a)?;
    let method = headers::byte_string(agent, &a.arg(0))?;
    let url_s = body::usv_utf8(agent, &a.arg(1))?;
    let is_async = if a.args.len() < 3 {
        true
    } else {
        bf_js::host::webidl::to_boolean(agent, &a.arg(2))
    };
    let username = if a.arg(3).is_nullish() {
        None
    } else {
        Some(body::usv_utf8(agent, &a.arg(3))?)
    };
    let password = if a.arg(4).is_nullish() {
        None
    } else {
        Some(body::usv_utf8(agent, &a.arg(4))?)
    };
    if !headers::is_token(&method) {
        return dom_error(
            agent,
            "SyntaxError",
            &format!("'{method}' is not a valid HTTP method."),
        );
    }
    if headers::is_forbidden_method(&method) {
        return dom_error(
            agent,
            "SecurityError",
            &format!("'{method}' HTTP method is unsupported."),
        );
    }
    let method = {
        let mut m = method.clone();
        for k in ["DELETE", "GET", "HEAD", "OPTIONS", "POST", "PUT"] {
            if method.eq_ignore_ascii_case(k) {
                m = k.to_owned();
            }
        }
        m
    };
    let base = super::base_url(agent);
    let Ok(mut url) = Url::parse(&url_s, Some(&base)) else {
        return dom_error(agent, "SyntaxError", &format!("Invalid URL: {url_s}"));
    };
    if url.host.is_some() {
        if let Some(u) = username {
            url.username = u;
        }
        if let Some(p) = password {
            url.password = p;
        }
    }
    let cur = st(agent, t);
    if !is_async && (cur.timeout != 0 || !cur.response_type.is_empty()) {
        return dom_error(
            agent,
            "InvalidAccessError",
            "Synchronous requests from a document must not set a timeout or a responseType.",
        );
    }
    terminate(agent, t);
    let fire = with_st(agent, t, |s| {
        s.send_flag = false;
        s.sync = !is_async;
        s.method = method;
        s.url = Some(url);
        s.headers = HeaderList::default();
        s.response = None;
        s.timed_out = false;
        s.override_mime = None;
        if s.ready != 1 {
            s.ready = 1;
            true
        } else {
            false
        }
    });
    set_slot(agent, o, 0, Value::Null);
    if fire {
        fire_simple(agent, t, "readystatechange");
    }
    Ok(Value::Undefined)
}

/// `setRequestHeader(name, value)` (XHR §3.5.2).
fn set_request_header(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (t, _) = this_xhr(agent, a)?;
    let name = headers::byte_string(agent, &a.arg(0))?;
    let value = headers::normalize(&headers::byte_string(agent, &a.arg(1))?);
    let s = st(agent, t);
    if s.ready != 1 || s.send_flag {
        return dom_error(
            agent,
            "InvalidStateError",
            "The object's state must be OPENED.",
        );
    }
    if !headers::is_token(&name) {
        return dom_error(
            agent,
            "SyntaxError",
            &format!("'{name}' is not a valid HTTP header field name."),
        );
    }
    if !headers::is_value(&value) {
        return dom_error(
            agent,
            "SyntaxError",
            &format!("'{value}' is not a valid HTTP header field value."),
        );
    }
    if headers::is_forbidden_request_header(&name, &value) {
        return Ok(Value::Undefined);
    }
    with_st(agent, t, |s| s.headers.combine(&name, &value));
    Ok(Value::Undefined)
}

fn get_timeout(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (t, _) = this_xhr(agent, a)?;
    Ok(Value::Number(f64::from(st(agent, t).timeout)))
}

fn set_timeout(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (t, _) = this_xhr(agent, a)?;
    let v = bf_js::host::webidl::to_unsigned_long(agent, &a.arg(0))?;
    if st(agent, t).sync {
        return dom_error(
            agent,
            "InvalidAccessError",
            "Timeouts are not supported for synchronous requests made from a document.",
        );
    }
    with_st(agent, t, |s| s.timeout = v);
    Ok(Value::Undefined)
}

fn get_with_credentials(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (t, _) = this_xhr(agent, a)?;
    Ok(Value::Boolean(st(agent, t).with_credentials))
}

fn set_with_credentials(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (t, _) = this_xhr(agent, a)?;
    let v = bf_js::host::webidl::to_boolean(agent, &a.arg(0));
    let s = st(agent, t);
    if s.ready > 1 || s.send_flag {
        return dom_error(
            agent,
            "InvalidStateError",
            "The value may only be set if the object's state is UNSENT or OPENED.",
        );
    }
    with_st(agent, t, |s| s.with_credentials = v);
    Ok(Value::Undefined)
}

/// `upload` (siempre el mismo `XMLHttpRequestUpload`).
fn upload(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (_, o) = this_xhr(agent, a)?;
    let cur = get_slot(agent, o, 1);
    if !cur.is_null() {
        return Ok(cur);
    }
    let class = classes(agent).xhr_upload;
    let (_, u) = new_target(agent, class, None)?;
    set_slot(agent, o, 1, Value::Object(u));
    Ok(Value::Object(u))
}

fn upload_target(agent: &mut Agent, o: Obj) -> Option<TargetRef> {
    let u = get_slot(agent, o, 1);
    agent.host_unwrap::<TargetRef>(&u).copied()
}

/// `send(body)` (XHR §3.5.6).
fn send(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (t, o) = this_xhr(agent, a)?;
    let s = st(agent, t);
    if s.ready != 1 {
        return dom_error(
            agent,
            "InvalidStateError",
            "The object's state must be OPENED.",
        );
    }
    if s.send_flag {
        return dom_error(
            agent,
            "InvalidStateError",
            "The object's send() method has already been called.",
        );
    }
    let body_arg = a.arg(0);
    let mut headers_list = s.headers.clone();
    let mut body = None;
    if !matches!(s.method.as_str(), "GET" | "HEAD") && !body_arg.is_nullish() {
        let is_string = matches!(body_arg, Value::String(_));
        let (bytes, ct) = if let Some(n) = crate::dom::unwrap_node(agent, &body_arg)
            && crate::with_doc(agent, n.doc, |d| d.root() == n.node)
        {
            // Un Document: su serialización HTML (XHR §3.5.6 paso 4.1).
            let html = crate::with_doc(agent, n.doc, |d| {
                bf_html::dom::serialize_children(d, d.root(), false)
                    .as_lossy_str()
                    .to_string()
            });
            (
                html.into_bytes(),
                Some("text/html;charset=UTF-8".to_owned()),
            )
        } else {
            body::extract(agent, &body_arg)?
        };
        match headers_list.get("content-type") {
            None => {
                if let Some(ct) = ct {
                    headers_list.append("Content-Type", &ct);
                }
            }
            Some(orig) if is_string => {
                // Con un string, el charset del autor pasa a ser UTF-8.
                if let Some((essence, params)) = body::parse_mime(&orig)
                    && params
                        .iter()
                        .any(|(k, v)| k == "charset" && !v.eq_ignore_ascii_case("UTF-8"))
                {
                    let mut out = essence;
                    for (k, v) in params {
                        out.push(';');
                        out.push_str(&k);
                        out.push('=');
                        out.push_str(if k == "charset" { "UTF-8" } else { &v });
                    }
                    headers_list.set("Content-Type", &out);
                }
            }
            Some(_) => {}
        }
        body = Some(bytes);
    }
    let has_body = body.is_some();
    let body_len = body.as_ref().map_or(0.0, |b| b.len() as f64);
    let Some(url) = s.url.clone() else {
        return dom_error(
            agent,
            "InvalidStateError",
            "The object's state must be OPENED.",
        );
    };
    let same = super::same_origin(agent, &url);
    let credentials = s.with_credentials || same;
    let mut net_headers = bf_net::Headers::default();
    for (n, v) in &headers_list.0 {
        net_headers.push(n, v);
    }
    if !same || !matches!(s.method.as_str(), "GET" | "HEAD") {
        net_headers.push("Origin", &super::document_origin(agent));
    }
    let req = NetRequest {
        url: url.clone(),
        options: bf_net::RequestOptions {
            method: s.method.clone(),
            headers: net_headers,
            body,
            redirect: bf_net::RedirectMode::Follow,
            credentials,
            referrer: Some(super::base_url(agent)),
        },
    };
    let generation = with_st(agent, t, |x| {
        x.upload_complete = !has_body;
        x.has_body = has_body;
        x.body_len = body_len;
        x.timed_out = false;
        x.response = None;
        x.generation
    });
    if s.sync {
        let r = super::fetch_blocking(agent, &req);
        with_st(agent, t, |x| x.send_flag = true);
        let r = filter(agent, &url, s.with_credentials, r);
        match r {
            Ok(res) => {
                with_st(agent, t, |x| {
                    x.upload_complete = true;
                    x.response = Some(res);
                });
                end_of_body(agent, t, o, true);
            }
            Err(_) => {
                request_error(agent, t, o, "error");
                return dom_error(
                    agent,
                    "NetworkError",
                    &format!("Failed to load '{}'.", url.href()),
                );
            }
        }
        return Ok(Value::Undefined);
    }
    with_st(agent, t, |x| x.send_flag = true);
    let pin = agent.host_pin(Value::Object(o));
    with_st(agent, t, |x| x.pin = Some(pin));
    fire_progress(agent, t, "loadstart", false, 0.0, 0.0);
    if has_body && let Some(u) = upload_target(agent, o) {
        fire_progress(agent, u, "loadstart", true, 0.0, body_len);
    }
    let cur = st(agent, t);
    if cur.ready != 1 || !cur.send_flag || cur.generation != generation {
        return Ok(Value::Undefined);
    }
    let with_credentials = s.with_credentials;
    let net = super::start(
        agent,
        req,
        Box::new(move |agent: &mut Agent, result: NetResult| {
            if st(agent, t).generation != generation {
                return;
            }
            with_st(agent, t, |x| x.net = None);
            let r = filter(agent, &url, with_credentials, result);
            process_response(agent, t, o, generation, r);
        }),
    );
    with_st(agent, t, |x| x.net = Some(net));
    if cur.timeout > 0 {
        let cb = agent.create_builtin_with_slots(
            on_timeout,
            0,
            "",
            vec![
                Value::Number(target_id(t) as f64),
                Value::Number(generation as f64),
                Value::Object(o),
            ],
        );
        let realm = WebContextHandle::current(agent).realm;
        let timer = agent.set_timer(
            realm,
            TimerHandler::Callback(Value::Object(cb)),
            f64::from(cur.timeout),
            Vec::new(),
            false,
        );
        with_st(agent, t, |x| x.timer = Some(timer));
    }
    Ok(Value::Undefined)
}

/// La respuesta filtrada (basic o CORS); `Err` si es un error de red o falla CORS.
fn filter(
    agent: &mut Agent,
    url: &Url,
    with_credentials: bool,
    r: NetResult,
) -> Result<XhrResponse, String> {
    let res = r?;
    let final_url = res.url_list.last().cloned().unwrap_or_else(|| url.clone());
    let same = super::same_origin(agent, url) && super::same_origin(agent, &final_url);
    let headers = if same {
        super::basic_filter(&res.headers)
    } else {
        let origin = super::document_origin(agent);
        if !super::cors_check(&origin, &res.headers, with_credentials) {
            return Err("CORS".into());
        }
        super::cors_filter(&res.headers)
    };
    Ok(XhrResponse {
        url: Some(final_url),
        status: res.status,
        status_text: res.status_text,
        headers,
        body: res.body,
    })
}

/// El temporizador de `timeout`.
fn on_timeout(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let slots = match &agent.heap[a.callee].kind {
        bf_js::object::ObjectKind::Builtin(b) => b.slots.clone(),
        _ => return Ok(Value::Undefined),
    };
    let id = slots.first().and_then(Value::as_number).unwrap_or(0.0) as u64;
    let generation = slots.get(1).and_then(Value::as_number).unwrap_or(0.0) as u64;
    let Some(Value::Object(o)) = slots.get(2).cloned() else {
        return Ok(Value::Undefined);
    };
    let t = TargetRef {
        doc: 0,
        target: EventTarget::Object(id),
    };
    let s = st(agent, t);
    if s.generation != generation || !s.send_flag {
        return Ok(Value::Undefined);
    }
    with_st(agent, t, |x| {
        x.timed_out = true;
        x.timer = None;
    });
    if let Some(n) = s.net {
        super::cancel(agent, n);
    }
    request_error(agent, t, o, "timeout");
    Ok(Value::Undefined)
}

/// "Process response" y "process response end-of-body" (XHR §3.5.6), con el cuerpo entero.
fn process_response(
    agent: &mut Agent,
    t: TargetRef,
    o: Obj,
    generation: u64,
    r: Result<XhrResponse, String>,
) {
    let res = match r {
        Ok(res) => res,
        Err(_) => {
            request_error(agent, t, o, "error");
            return;
        }
    };
    let s = st(agent, t);
    // El upload terminó.
    if !s.upload_complete {
        with_st(agent, t, |x| x.upload_complete = true);
        if s.has_body
            && let Some(u) = upload_target(agent, o)
        {
            fire_progress(agent, u, "progress", true, s.body_len, s.body_len);
            fire_progress(agent, u, "load", true, s.body_len, s.body_len);
            fire_progress(agent, u, "loadend", true, s.body_len, s.body_len);
        }
        if st(agent, t).generation != generation {
            return;
        }
    }
    let empty = res.body.is_empty();
    with_st(agent, t, |x| {
        x.response = Some(res);
        x.ready = 2;
    });
    fire_simple(agent, t, "readystatechange");
    if st(agent, t).generation != generation || st(agent, t).ready != 2 {
        return;
    }
    if !empty {
        with_st(agent, t, |x| x.ready = 3);
        fire_simple(agent, t, "readystatechange");
        if st(agent, t).generation != generation {
            return;
        }
    }
    end_of_body(agent, t, o, false);
}

/// "Handle response end-of-body" (XHR §3.5.6).
fn end_of_body(agent: &mut Agent, t: TargetRef, _o: Obj, sync: bool) {
    let s = st(agent, t);
    let (loaded, total, computable) = match &s.response {
        Some(r) => {
            let len = r
                .headers
                .get("content-length")
                .and_then(|v| v.trim().parse::<f64>().ok());
            (r.body.len() as f64, len.unwrap_or(0.0), len.is_some())
        }
        None => (0.0, 0.0, false),
    };
    let generation = s.generation;
    if !sync {
        fire_progress(agent, t, "progress", computable, loaded, total);
        if st(agent, t).generation != generation {
            return;
        }
    }
    let pin = with_st(agent, t, |x| {
        x.ready = 4;
        x.send_flag = false;
        if let Some(tm) = x.timer.take() {
            Some((tm, x.pin.take()))
        } else {
            Some((TimerId::default(), x.pin.take()))
        }
    });
    if let Some((tm, _)) = &pin
        && *tm != TimerId::default()
    {
        agent.clear_timer(*tm);
    }
    fire_simple(agent, t, "readystatechange");
    if st(agent, t).ready == 4 {
        fire_progress(agent, t, "load", computable, loaded, total);
        fire_progress(agent, t, "loadend", computable, loaded, total);
    }
    if let Some((_, Some(p))) = pin {
        agent.host_unpin(p);
    }
}

/// "Request error steps" (XHR §3.5.6) con `event` (`error`, `abort` o `timeout`).
fn request_error(agent: &mut Agent, t: TargetRef, o: Obj, event: &str) {
    let (net, timer, pin, upload_done, has_body, sync) = with_st(agent, t, |x| {
        x.ready = 4;
        x.send_flag = false;
        x.response = None;
        x.generation += 1;
        let u = x.upload_complete;
        x.upload_complete = true;
        (
            x.net.take(),
            x.timer.take(),
            x.pin.take(),
            u,
            x.has_body,
            x.sync,
        )
    });
    if let Some(n) = net {
        super::cancel(agent, n);
    }
    if let Some(tm) = timer {
        agent.clear_timer(tm);
    }
    if sync {
        if let Some(p) = pin {
            agent.host_unpin(p);
        }
        return;
    }
    fire_simple(agent, t, "readystatechange");
    if !upload_done
        && has_body
        && let Some(u) = upload_target(agent, o)
    {
        fire_progress(agent, u, event, false, 0.0, 0.0);
        fire_progress(agent, u, "loadend", false, 0.0, 0.0);
    }
    fire_progress(agent, t, event, false, 0.0, 0.0);
    fire_progress(agent, t, "loadend", false, 0.0, 0.0);
    if let Some(p) = pin {
        agent.host_unpin(p);
    }
}

/// `abort()` (XHR §3.5.7).
fn abort(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (t, o) = this_xhr(agent, a)?;
    let s = st(agent, t);
    if (s.ready == 1 && s.send_flag) || s.ready == 2 || s.ready == 3 {
        let sync = s.sync;
        request_error(agent, t, o, "abort");
        if sync {
            return dom_error(agent, "AbortError", "The operation was aborted.");
        }
    }
    let s = st(agent, t);
    if s.ready == 4 {
        with_st(agent, t, |x| {
            x.ready = 0;
            x.response = None;
        });
    }
    Ok(Value::Undefined)
}

fn response_url(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (t, _) = this_xhr(agent, a)?;
    let u = st(agent, t).response.and_then(|r| r.url);
    Ok(Value::str(&u.map_or_else(String::new, |mut u| {
        u.fragment = None;
        u.href()
    })))
}

fn status(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (t, _) = this_xhr(agent, a)?;
    Ok(Value::Number(f64::from(
        st(agent, t).response.map_or(0, |r| r.status),
    )))
}

fn status_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (t, _) = this_xhr(agent, a)?;
    Ok(Value::str(
        &st(agent, t)
            .response
            .map_or_else(String::new, |r| r.status_text),
    ))
}

fn get_response_header(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (t, _) = this_xhr(agent, a)?;
    let name = headers::byte_string(agent, &a.arg(0))?;
    let v = st(agent, t).response.and_then(|r| r.headers.get(&name));
    Ok(v.map_or(Value::Null, |v| Value::str(&v)))
}

/// `getAllResponseHeaders()` (XHR §3.6.5): "sort and combine", `name: value\r\n`.
fn get_all_response_headers(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (t, _) = this_xhr(agent, a)?;
    let mut out = String::new();
    if let Some(r) = st(agent, t).response {
        for (n, v) in r.headers.sort_and_combine() {
            out.push_str(&n);
            out.push_str(": ");
            out.push_str(&v);
            out.push_str("\r\n");
        }
    }
    Ok(Value::str(&out))
}

/// `overrideMimeType(mime)` (XHR §3.6.7).
fn override_mime_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (t, _) = this_xhr(agent, a)?;
    let m = agent.to_string(&a.arg(0))?.to_string();
    if st(agent, t).ready >= 3 {
        return dom_error(
            agent,
            "InvalidStateError",
            "MimeType cannot be overridden when the state is LOADING or DONE.",
        );
    }
    let v = if body::parse_mime(&m).is_some() {
        m
    } else {
        "application/octet-stream".into()
    };
    with_st(agent, t, |x| x.override_mime = Some(v));
    Ok(Value::Undefined)
}

fn get_response_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (t, _) = this_xhr(agent, a)?;
    Ok(Value::str(&st(agent, t).response_type))
}

fn set_response_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (t, _) = this_xhr(agent, a)?;
    let v = agent.to_string(&a.arg(0))?.to_string();
    // Un valor que no es del enum se ignora (WebIDL §3.2.18 para atributos).
    if !matches!(
        v.as_str(),
        "" | "arraybuffer" | "blob" | "document" | "json" | "text"
    ) {
        return Ok(Value::Undefined);
    }
    let s = st(agent, t);
    if s.ready >= 3 {
        return dom_error(
            agent,
            "InvalidStateError",
            "The response type cannot be set if the object's state is LOADING or DONE.",
        );
    }
    if s.sync && s.ready >= 1 {
        return dom_error(
            agent,
            "InvalidAccessError",
            "The response type cannot be changed for synchronous requests made from a document.",
        );
    }
    with_st(agent, t, |x| x.response_type = v);
    Ok(Value::Undefined)
}

/// El "final MIME type" (XHR §3.6.8): el override o el `Content-Type` de la respuesta.
fn final_mime(s: &XhrState) -> String {
    s.override_mime
        .clone()
        .or_else(|| {
            s.response
                .as_ref()
                .and_then(|r| r.headers.get("content-type"))
        })
        .unwrap_or_else(|| "text/xml".into())
}

/// "Text response" (XHR §3.6.9).
fn text_response(s: &XhrState) -> String {
    let Some(r) = &s.response else {
        return String::new();
    };
    let mime = final_mime(s);
    let charset = s
        .override_mime
        .as_deref()
        .and_then(mime_charset)
        .or_else(|| mime_charset(&mime));
    decode_with(&r.body, charset.as_deref())
}

/// `response` (XHR §3.6.10).
fn response(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (t, o) = this_xhr(agent, a)?;
    let s = st(agent, t);
    match s.response_type.as_str() {
        "" | "text" => {
            if s.ready < 3 {
                return Ok(Value::str(""));
            }
            Ok(Value::str(&text_response(&s)))
        }
        kind => {
            if s.ready != 4 {
                return Ok(Value::Null);
            }
            let cached = get_slot(agent, o, 0);
            if !cached.is_null() {
                return Ok(cached);
            }
            let Some(r) = s.response.clone() else {
                return Ok(Value::Null);
            };
            let v = match kind {
                "arraybuffer" => Value::Object(bf_js::builtins::web::encoding::new_array_buffer(
                    agent, r.body,
                )),
                "blob" => {
                    let mime = final_mime(&s).to_ascii_lowercase();
                    super::blob::create(
                        agent,
                        super::blob::BlobData {
                            bytes: std::sync::Arc::new(r.body),
                            mime,
                            file: None,
                        },
                    )
                }
                "json" => match body::parse_json(agent, &body::utf8_decode(&r.body)) {
                    Ok(v) => v,
                    Err(Throw::Value(_)) => Value::Null,
                    Err(t) => return Err(t),
                },
                _ => document_response(agent, &s)?,
            };
            set_slot(agent, o, 0, v.clone());
            Ok(v)
        }
    }
}

/// "Document response" (XHR §3.6.11): solo HTML (no hay parser XML todavía).
fn document_response(agent: &mut Agent, s: &XhrState) -> JsResult<Value> {
    let Some(r) = &s.response else {
        return Ok(Value::Null);
    };
    let mime = final_mime(s);
    if mime_essence(&mime).as_deref() != Some("text/html") || s.response_type != "document" {
        return Ok(Value::Null);
    }
    let charset = s
        .override_mime
        .as_deref()
        .and_then(mime_charset)
        .or_else(|| mime_charset(&mime));
    let text = decode_with(&r.body, charset.as_deref());
    let (mut doc, _) = bf_html::parse_document(
        &text,
        &bf_html::ParseOptions {
            scripting: false,
            iframe_srcdoc: false,
        },
    );
    if let Some(u) = &r.url {
        doc.url = u.clone();
    }
    doc.content_type = "text/html".into();
    let root = doc.root();
    let idx = WebContextHandle::current(agent)
        .get_mut(agent)
        .add_document(doc);
    Ok(crate::dom::wrap_node(agent, NodeRef::new(idx, root)))
}

fn response_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (t, _) = this_xhr(agent, a)?;
    let s = st(agent, t);
    if !matches!(s.response_type.as_str(), "" | "text") {
        return dom_error(
            agent,
            "InvalidStateError",
            "The value is only accessible if the object's 'responseType' is '' or 'text'.",
        );
    }
    if s.ready < 3 {
        return Ok(Value::str(""));
    }
    Ok(Value::str(&text_response(&s)))
}

fn response_xml(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (t, o) = this_xhr(agent, a)?;
    let s = st(agent, t);
    if !matches!(s.response_type.as_str(), "" | "document") {
        return dom_error(
            agent,
            "InvalidStateError",
            "The value is only accessible if the object's 'responseType' is '' or 'document'.",
        );
    }
    if s.ready != 4 || s.response_type.is_empty() {
        return Ok(Value::Null);
    }
    let cached = get_slot(agent, o, 0);
    if !cached.is_null() {
        return Ok(cached);
    }
    let v = document_response(agent, &s)?;
    set_slot(agent, o, 0, v.clone());
    Ok(v)
}
