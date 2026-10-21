//! Fetch Standard §5.4–§5.6: `Request`, `Response` y `fetch()`; y `navigator.sendBeacon`
//! (Beacon §3.1).
//!
//! CORS simplificado: mismo origen (respuesta "basic"), `no-cors` (respuesta opaca) y `cors`
//! con `Access-Control-Allow-Origin` comprobado (sin preflight). `cache` se acepta pero no hay
//! caché HTTP (todo es `no-store`).

use bf_infra::Url;
use bf_js::host::{HostClass, HostHandle};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, RealmId, Throw};

use super::body::{self, Body, ConsumeKind};
use super::headers::{self, Guard, HeaderList, HeadersData};
use super::{NetRequest, NetResult, classes, state};

// --- Request ---

/// Los datos de un `Request` (Fetch §2.2.5). Los `Headers` y el `AbortSignal` van en los
/// slots 0 y 1 del wrapper.
#[derive(Debug, Clone)]
pub struct RequestData {
    pub method: String,
    pub url: Url,
    pub body: Body,
    pub mode: String,
    pub credentials: String,
    pub cache: String,
    pub redirect: String,
    /// `"about:client"`, `""` (no-referrer) o una URL.
    pub referrer: String,
    pub referrer_policy: String,
    pub integrity: String,
    pub keepalive: bool,
    pub priority: String,
}

/// Los datos de un `Response` (Fetch §2.2.6). Los `Headers` van en el slot 0.
#[derive(Debug, Clone)]
pub struct ResponseData {
    /// `basic`, `cors`, `default`, `error`, `opaque`, `opaqueredirect`.
    pub type_: String,
    pub url: Option<Url>,
    pub redirected: bool,
    pub status: u16,
    pub status_text: String,
    pub body: Body,
}

pub fn request_class() -> HostClass {
    HostClass::new("Request")
        .constructor(request_ctor, 1)
        .attribute("method", Some(req_method), None)
        .attribute("url", Some(req_url), None)
        .attribute("headers", Some(slot0), None)
        .attribute("destination", Some(req_destination), None)
        .attribute("referrer", Some(req_referrer), None)
        .attribute("referrerPolicy", Some(req_referrer_policy), None)
        .attribute("mode", Some(req_mode), None)
        .attribute("credentials", Some(req_credentials), None)
        .attribute("cache", Some(req_cache), None)
        .attribute("redirect", Some(req_redirect), None)
        .attribute("integrity", Some(req_integrity), None)
        .attribute("keepalive", Some(req_keepalive), None)
        .attribute("isReloadNavigation", Some(false_attr), None)
        .attribute("isHistoryNavigation", Some(false_attr), None)
        .attribute("signal", Some(slot1), None)
        .attribute("duplex", Some(req_duplex), None)
        .method("clone", 0, req_clone)
        .attribute("body", Some(body_attr), None)
        .attribute("bodyUsed", Some(body_used), None)
        .method("arrayBuffer", 0, body_array_buffer)
        .method("blob", 0, body_blob)
        .method("bytes", 0, body_bytes)
        .method("formData", 0, body_form_data)
        .method("json", 0, body_json)
        .method("text", 0, body_text)
}

pub fn response_class() -> HostClass {
    HostClass::new("Response")
        .constructor(response_ctor, 0)
        .static_method("error", 0, response_error)
        .static_method("redirect", 1, response_redirect)
        .static_method("json", 1, response_json)
        .attribute("type", Some(res_type), None)
        .attribute("url", Some(res_url), None)
        .attribute("redirected", Some(res_redirected), None)
        .attribute("status", Some(res_status), None)
        .attribute("ok", Some(res_ok), None)
        .attribute("statusText", Some(res_status_text), None)
        .attribute("headers", Some(slot0), None)
        .method("clone", 0, res_clone)
        .attribute("body", Some(body_attr), None)
        .attribute("bodyUsed", Some(body_used), None)
        .method("arrayBuffer", 0, body_array_buffer)
        .method("blob", 0, body_blob)
        .method("bytes", 0, body_bytes)
        .method("formData", 0, body_form_data)
        .method("json", 0, body_json)
        .method("text", 0, body_text)
}

/// `partial interface mixin WindowOrWorkerGlobalScope { Promise<Response> fetch(...); }`.
pub fn window_partial() -> HostClass {
    HostClass::new("Window").method("fetch", 1, fetch)
}

fn slot(agent: &mut Agent, this: &Value, i: usize) -> Value {
    match this {
        Value::Object(o) => agent
            .host_slots_mut(*o)
            .and_then(|s| s.get(i).cloned())
            .unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

fn slot0(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(slot(agent, &a.this, 0))
}

fn slot1(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(slot(agent, &a.this, 1))
}

fn false_attr(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(false))
}

fn this_req(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<RequestData> {
    match agent.host_unwrap::<RequestData>(&a.this) {
        Some(r) => Ok(r.clone()),
        None => agent.type_error("Illegal invocation"),
    }
}

fn this_res(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<ResponseData> {
    match agent.host_unwrap::<ResponseData>(&a.this) {
        Some(r) => Ok(r.clone()),
        None => agent.type_error("Illegal invocation"),
    }
}

/// La lista de cabeceras de un objeto con `Headers` en el slot 0.
fn header_list(agent: &mut Agent, this: &Value) -> HeaderList {
    let h = slot(agent, this, 0);
    headers::data(agent, &h).map(|d| d.list).unwrap_or_default()
}

/// Un miembro de un diccionario: `None` si es `undefined`.
fn member(agent: &mut Agent, dict: &Value, name: &str) -> JsResult<Option<Value>> {
    match dict {
        Value::Object(o) => {
            let v = agent.get(*o, &PropertyKey::from(name), dict.clone())?;
            Ok((!v.is_undefined()).then_some(v))
        }
        _ => Ok(None),
    }
}

/// Un miembro enumerado (TypeError si el valor no es uno de `allowed`).
fn enum_member(
    agent: &mut Agent,
    dict: &Value,
    name: &str,
    ty: &str,
    allowed: &[&str],
) -> JsResult<Option<String>> {
    let Some(v) = member(agent, dict, name)? else {
        return Ok(None);
    };
    let s = agent.to_string(&v)?.to_string();
    if !allowed.contains(&s.as_str()) {
        return agent.type_error(&format!(
            "Failed to construct 'Request': The provided value '{s}' is not a valid enum value of type {ty}."
        ));
    }
    Ok(Some(s))
}

/// "Normalize a method" (Fetch §2.2.1).
fn normalize_method(m: &str) -> String {
    for k in ["DELETE", "GET", "HEAD", "OPTIONS", "POST", "PUT"] {
        if m.eq_ignore_ascii_case(k) {
            return k.to_owned();
        }
    }
    m.to_owned()
}

/// Un `AbortSignal` que nunca se aborta (`new AbortController().signal`).
fn new_signal(agent: &mut Agent) -> JsResult<Value> {
    let realm = agent.current_realm;
    let g = agent.realms[realm].global_object;
    let ctor = agent.get(g, &PropertyKey::from("AbortController"), Value::Object(g))?;
    if !agent.is_callable(&ctor) {
        return Ok(Value::Null);
    }
    let Value::Object(c) = ctor else {
        return Ok(Value::Null);
    };
    let ctrl = agent.construct(c, &[], None)?;
    let Value::Object(co) = ctrl else {
        return Ok(Value::Null);
    };
    agent.get(co, &PropertyKey::from("signal"), ctrl.clone())
}

/// ¿Es `v` un `AbortSignal`?
fn is_signal(agent: &Agent, v: &Value) -> bool {
    let c = crate::dom::classes(agent).abort_signal;
    agent.host_is_instance(v, c)
}

/// Crea el wrapper de un `Request` con sus `Headers` y su señal.
fn make_request(
    agent: &mut Agent,
    nt: Option<bf_js::heap::Gc<bf_js::object::Object>>,
    d: RequestData,
    headers: HeadersData,
    signal: Value,
) -> JsResult<Value> {
    let class = classes(agent).request;
    let o = match nt {
        Some(nt) => agent.host_create_from_new_target(nt, class, None, Box::new(d))?,
        None => agent.host_create(class, Box::new(d)),
    };
    let h = headers::create(agent, headers.list, headers.guard);
    if let Some(s) = agent.host_slots_mut(o) {
        *s = vec![h, signal];
    }
    Ok(Value::Object(o))
}

/// `new Request(input, init)` (Fetch §5.4).
fn request_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Illegal constructor");
    };
    let (r, h, s) = build_request(agent, &a.arg(0), &a.arg(1))?;
    make_request(agent, Some(nt), r, h, s)
}

/// Los pasos del constructor de `Request`: los datos, sus cabeceras y su señal.
fn build_request(
    agent: &mut Agent,
    input: &Value,
    init: &Value,
) -> JsResult<(RequestData, HeadersData, Value)> {
    if !matches!(init, Value::Undefined | Value::Null | Value::Object(_)) {
        return agent.type_error(
            "Failed to construct 'Request': The provided value is not of type 'RequestInit'.",
        );
    }
    let base = super::base_url(agent);
    let input_req = agent.host_unwrap::<RequestData>(input).cloned();
    let (mut r, mut signal, input_headers) = match &input_req {
        Some(ir) => {
            let h = header_list(agent, input);
            let s = slot(agent, input, 1);
            (ir.clone(), s, Some(h))
        }
        None => {
            let s = body::usv_utf8(agent, input)?;
            let url = match Url::parse(&s, Some(&base)) {
                Ok(u) => u,
                Err(_) => {
                    return agent.type_error(&format!(
                        "Failed to construct 'Request': Failed to parse URL from {s}"
                    ));
                }
            };
            if !url.username.is_empty() || !url.password.is_empty() {
                return agent.type_error(&format!(
                    "Failed to construct 'Request': Request cannot be constructed from a URL that includes credentials: {s}"
                ));
            }
            (
                RequestData {
                    method: "GET".into(),
                    url,
                    body: Body::default(),
                    mode: "cors".into(),
                    credentials: "same-origin".into(),
                    cache: "default".into(),
                    redirect: "follow".into(),
                    referrer: "about:client".into(),
                    referrer_policy: String::new(),
                    integrity: String::new(),
                    keepalive: false,
                    priority: "auto".into(),
                },
                Value::Null,
                None,
            )
        }
    };
    // Los miembros del diccionario en orden alfabético (WebIDL §3.2.17).
    let body_init = member(agent, init, "body")?;
    let cache = enum_member(
        agent,
        init,
        "cache",
        "RequestCache",
        &[
            "default",
            "no-store",
            "reload",
            "no-cache",
            "force-cache",
            "only-if-cached",
        ],
    )?;
    let credentials = enum_member(
        agent,
        init,
        "credentials",
        "RequestCredentials",
        &["omit", "same-origin", "include"],
    )?;
    let duplex = enum_member(agent, init, "duplex", "RequestDuplex", &["half"])?;
    let headers_init = member(agent, init, "headers")?;
    let integrity = match member(agent, init, "integrity")? {
        Some(v) => Some(agent.to_string(&v)?.to_string()),
        None => None,
    };
    let keepalive = match member(agent, init, "keepalive")? {
        Some(v) => Some(bf_js::host::webidl::to_boolean(agent, &v)),
        None => None,
    };
    let method = match member(agent, init, "method")? {
        Some(v) => Some(headers::byte_string(agent, &v)?),
        None => None,
    };
    let mode = enum_member(
        agent,
        init,
        "mode",
        "RequestMode",
        &["navigate", "same-origin", "no-cors", "cors"],
    )?;
    let priority = enum_member(
        agent,
        init,
        "priority",
        "RequestPriority",
        &["high", "low", "auto"],
    )?;
    let redirect = enum_member(
        agent,
        init,
        "redirect",
        "RequestRedirect",
        &["follow", "error", "manual"],
    )?;
    let referrer = match member(agent, init, "referrer")? {
        Some(v) => Some(body::usv_utf8(agent, &v)?),
        None => None,
    };
    let referrer_policy = enum_member(
        agent,
        init,
        "referrerPolicy",
        "ReferrerPolicy",
        &[
            "",
            "no-referrer",
            "no-referrer-when-downgrade",
            "same-origin",
            "origin",
            "strict-origin",
            "origin-when-cross-origin",
            "strict-origin-when-cross-origin",
            "unsafe-url",
        ],
    )?;
    let signal_init = match init {
        Value::Object(o) => {
            let k = PropertyKey::from("signal");
            let has = agent.has_property(*o, &k)?;
            if has {
                Some(agent.get(*o, &k, init.clone())?)
            } else {
                None
            }
        }
        _ => None,
    };
    if let Some(w) = member(agent, init, "window")?
        && !w.is_null()
    {
        return agent.type_error("Failed to construct 'Request': 'window' must be null.");
    }
    let _ = duplex;
    let init_nonempty =
        matches!(init, Value::Object(o) if !agent.own_property_keys(*o)?.is_empty());
    if init_nonempty && r.mode == "navigate" {
        r.mode = "same-origin".into();
    }
    if let Some(rf) = referrer {
        if rf.is_empty() {
            r.referrer = String::new();
        } else {
            match Url::parse(&rf, Some(&base)) {
                Ok(u) => {
                    r.referrer = if u.scheme == "about" && u.path_string() == "client" {
                        "about:client".into()
                    } else if u.origin().to_string() != super::document_origin(agent) {
                        "about:client".into()
                    } else {
                        u.href()
                    };
                }
                Err(_) => {
                    return agent.type_error(&format!(
                        "Failed to construct 'Request': The referrer '{rf}' is not a valid URL."
                    ));
                }
            }
        }
    }
    if let Some(p) = referrer_policy {
        r.referrer_policy = p;
    }
    if let Some(m) = mode {
        if m == "navigate" {
            return agent.type_error("Failed to construct 'Request': Cannot construct a Request with a RequestInit whose mode member is set as 'navigate'.");
        }
        r.mode = m;
    }
    if let Some(c) = credentials {
        r.credentials = c;
    }
    if let Some(c) = cache {
        r.cache = c;
    }
    if r.cache == "only-if-cached" && r.mode != "same-origin" {
        return agent.type_error("Failed to construct 'Request': 'only-if-cached' can be set only with 'same-origin' mode");
    }
    if let Some(rd) = redirect {
        r.redirect = rd;
    }
    if let Some(i) = integrity {
        r.integrity = i;
    }
    if let Some(k) = keepalive {
        r.keepalive = k;
    }
    if let Some(p) = priority {
        r.priority = p;
    }
    if let Some(m) = method {
        if !headers::is_token(&m) {
            return agent.type_error(&format!(
                "Failed to construct 'Request': '{m}' is not a valid HTTP method."
            ));
        }
        if headers::is_forbidden_method(&m) {
            return agent.type_error(&format!(
                "Failed to construct 'Request': '{m}' HTTP method is unsupported."
            ));
        }
        r.method = normalize_method(&m);
    }
    if let Some(s) = signal_init {
        if s.is_null() {
            signal = Value::Null;
        } else if is_signal(agent, &s) {
            signal = s;
        } else {
            return agent.type_error(
                "Failed to construct 'Request': member signal is not of type AbortSignal.",
            );
        }
    }
    if signal.is_null() {
        signal = new_signal(agent)?;
    }
    // Las cabeceras.
    let guard = if r.mode == "no-cors" {
        if !matches!(r.method.as_str(), "GET" | "HEAD" | "POST") {
            return agent.type_error(&format!(
                "Failed to construct 'Request': '{}' is unsupported in no-cors mode.",
                r.method
            ));
        }
        Guard::RequestNoCors
    } else {
        Guard::Request
    };
    let mut hd = HeadersData {
        list: HeaderList::default(),
        guard,
    };
    match &headers_init {
        Some(h) => headers::fill(agent, &mut hd, h)?,
        None => {
            if let Some(list) = input_headers {
                for (n, v) in list.0 {
                    headers::append_to(agent, &mut hd, &n, &v)?;
                }
            }
        }
    }
    // El cuerpo.
    let input_body = input_req.as_ref().map(|ir| ir.body.clone());
    let has_init_body = body_init.as_ref().is_some_and(|b| !b.is_null());
    if (has_init_body || input_body.as_ref().is_some_and(|b| b.source.is_some()))
        && matches!(r.method.as_str(), "GET" | "HEAD")
    {
        return agent.type_error(
            "Failed to construct 'Request': Request with GET/HEAD method cannot have body.",
        );
    }
    if let Some(b) = &input_body
        && b.used
        && (has_init_body || b.source.is_some())
    {
        return agent.type_error("Failed to construct 'Request': Cannot construct a Request with a Request object that has already been used.");
    }
    if has_init_body {
        let (bytes, ct) = body::extract(agent, body_init.as_ref().expect("body"))?;
        if let Some(ct) = ct
            && !hd.list.contains("content-type")
        {
            hd.list.append("Content-Type", &ct);
        }
        r.body = Body::new(Some(bytes));
    } else if let Some(b) = input_body {
        r.body = Body {
            source: b.source,
            used: false,
        };
        // El cuerpo pasa al nuevo Request: el de entrada queda usado.
        if r.body.source.is_some()
            && let Some(ir) = agent.host_unwrap_mut::<RequestData>(input)
        {
            ir.body.used = true;
        }
    }
    Ok((r, hd, signal))
}

fn req_method(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&this_req(agent, a)?.method))
}

fn req_url(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&this_req(agent, a)?.url.href()))
}

fn req_destination(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_req(agent, a)?;
    Ok(Value::str(""))
}

fn req_referrer(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&this_req(agent, a)?.referrer))
}

fn req_referrer_policy(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&this_req(agent, a)?.referrer_policy))
}

fn req_mode(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&this_req(agent, a)?.mode))
}

fn req_credentials(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&this_req(agent, a)?.credentials))
}

fn req_cache(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&this_req(agent, a)?.cache))
}

fn req_redirect(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&this_req(agent, a)?.redirect))
}

fn req_integrity(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&this_req(agent, a)?.integrity))
}

fn req_keepalive(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(this_req(agent, a)?.keepalive))
}

fn req_duplex(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_req(agent, a)?;
    Ok(Value::str("half"))
}

fn req_clone(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_req(agent, a)?;
    if r.body.used {
        return agent
            .type_error("Failed to execute 'clone' on 'Request': Request body is already used");
    }
    let h = header_list(agent, &a.this);
    let hv = slot(agent, &a.this, 0);
    let guard = headers::data(agent, &hv).map_or(Guard::Request, |d| d.guard);
    let signal = slot(agent, &a.this, 1);
    make_request(agent, None, r, HeadersData { list: h, guard }, signal)
}

// --- Body mixin (Fetch §5.1) ---

/// El cuerpo y el `Content-Type` de `this` (un `Request` o un `Response`).
fn with_body<R>(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    f: impl FnOnce(&mut Agent, &mut Body, Option<String>) -> JsResult<R>,
) -> JsResult<R> {
    let mime = header_list(agent, &a.this).mime_type();
    let mut b = if let Some(r) = agent.host_unwrap::<RequestData>(&a.this) {
        r.body.clone()
    } else if let Some(r) = agent.host_unwrap::<ResponseData>(&a.this) {
        r.body.clone()
    } else {
        return agent.type_error("Illegal invocation");
    };
    let out = f(agent, &mut b, mime)?;
    if let Some(r) = agent.host_unwrap_mut::<RequestData>(&a.this) {
        r.body = b;
    } else if let Some(r) = agent.host_unwrap_mut::<ResponseData>(&a.this) {
        r.body = b;
    }
    Ok(out)
}

/// `body`: `null` (no hay Streams todavía; ver la documentación del módulo).
fn body_attr(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    with_body(agent, a, |_, _, _| Ok(Value::Null))
}

fn body_used(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    with_body(agent, a, |_, b, _| Ok(Value::Boolean(b.used)))
}

fn consume(agent: &mut Agent, a: &CallArgs<'_>, kind: ConsumeKind) -> JsResult<Value> {
    with_body(agent, a, |agent, b, mime| {
        body::consume(agent, b, kind, mime.as_deref())
    })
}

fn body_array_buffer(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    consume(agent, a, ConsumeKind::ArrayBuffer)
}

fn body_blob(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    consume(agent, a, ConsumeKind::Blob)
}

fn body_bytes(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    consume(agent, a, ConsumeKind::Bytes)
}

fn body_form_data(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    consume(agent, a, ConsumeKind::FormData)
}

fn body_json(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    consume(agent, a, ConsumeKind::Json)
}

fn body_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    consume(agent, a, ConsumeKind::Text)
}

// --- Response ---

/// Crea el wrapper de un `Response`.
pub fn make_response(
    agent: &mut Agent,
    nt: Option<bf_js::heap::Gc<bf_js::object::Object>>,
    d: ResponseData,
    list: HeaderList,
    guard: Guard,
) -> JsResult<Value> {
    let class = classes(agent).response;
    let o = match nt {
        Some(nt) => agent.host_create_from_new_target(nt, class, None, Box::new(d))?,
        None => agent.host_create(class, Box::new(d)),
    };
    let h = headers::create(agent, list, guard);
    if let Some(s) = agent.host_slots_mut(o) {
        *s = vec![h];
    }
    Ok(Value::Object(o))
}

/// "reason-phrase" (RFC 9112 §4): HTAB, SP, VCHAR y obs-text.
fn is_reason_phrase(s: &str) -> bool {
    s.chars().all(|c| {
        let b = c as u32;
        b == 9 || b == 0x20 || (0x21..=0x7E).contains(&b) || (0x80..=0xFF).contains(&b)
    })
}

/// "Initialize a response" (Fetch §5.5).
fn init_response(
    agent: &mut Agent,
    init: &Value,
    body: Option<(Vec<u8>, Option<String>)>,
) -> JsResult<(ResponseData, HeaderList)> {
    if !matches!(init, Value::Undefined | Value::Null | Value::Object(_)) {
        return agent.type_error(
            "Failed to construct 'Response': The provided value is not of type 'ResponseInit'.",
        );
    }
    let headers_init = member(agent, init, "headers")?;
    let status = match member(agent, init, "status")? {
        Some(v) => {
            let n = agent.to_number(&v)?;
            // `unsigned short` (WebIDL §3.2.4.3).
            let n = if n.is_finite() {
                n.trunc().rem_euclid(65536.0)
            } else {
                0.0
            };
            n as u16
        }
        None => 200,
    };
    let status_text = match member(agent, init, "statusText")? {
        Some(v) => headers::byte_string(agent, &v)?,
        None => String::new(),
    };
    if !(200..=599).contains(&status) {
        return agent.range_error(&format!(
            "Failed to construct 'Response': The status provided ({status}) is outside the range [200, 599]."
        ));
    }
    if !is_reason_phrase(&status_text) {
        return agent.type_error("Failed to construct 'Response': Invalid statusText");
    }
    let mut hd = HeadersData {
        list: HeaderList::default(),
        guard: Guard::Response,
    };
    if let Some(h) = &headers_init {
        headers::fill(agent, &mut hd, h)?;
    }
    let mut d = ResponseData {
        type_: "default".into(),
        url: None,
        redirected: false,
        status,
        status_text,
        body: Body::default(),
    };
    if let Some((bytes, ct)) = body {
        if matches!(status, 101 | 103 | 204 | 205 | 304) {
            return agent.type_error(
                "Failed to construct 'Response': Response with null body status cannot have body",
            );
        }
        d.body = Body::new(Some(bytes));
        if let Some(ct) = ct
            && !hd.list.contains("content-type")
        {
            hd.list.append("Content-Type", &ct);
        }
    }
    Ok((d, hd.list))
}

fn response_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Illegal constructor");
    };
    let b = a.arg(0);
    let body = if b.is_nullish() {
        None
    } else {
        Some(body::extract(agent, &b)?)
    };
    let (d, list) = init_response(agent, &a.arg(1), body)?;
    make_response(agent, Some(nt), d, list, Guard::Response)
}

/// `Response.error()`.
fn response_error(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let d = ResponseData {
        type_: "error".into(),
        url: None,
        redirected: false,
        status: 0,
        status_text: String::new(),
        body: Body::default(),
    };
    make_response(agent, None, d, HeaderList::default(), Guard::Immutable)
}

/// `Response.redirect(url, status)`.
fn response_redirect(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = body::usv_utf8(agent, &a.arg(0))?;
    let base = super::base_url(agent);
    let Ok(url) = Url::parse(&s, Some(&base)) else {
        return agent.type_error(&format!(
            "Failed to execute 'redirect' on 'Response': Invalid URL {s}"
        ));
    };
    let status = if a.arg(1).is_undefined() {
        302
    } else {
        let n = agent.to_number(&a.arg(1))?;
        if n.is_finite() {
            n.trunc().rem_euclid(65536.0) as u16
        } else {
            0
        }
    };
    if !matches!(status, 301 | 302 | 303 | 307 | 308) {
        return agent
            .range_error("Failed to execute 'redirect' on 'Response': Invalid status code");
    }
    let mut list = HeaderList::default();
    list.append("Location", &url.href());
    let d = ResponseData {
        type_: "default".into(),
        url: None,
        redirected: false,
        status,
        status_text: String::new(),
        body: Body::default(),
    };
    make_response(agent, None, d, list, Guard::Immutable)
}

/// `Response.json(data, init)`.
fn response_json(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(text) = body::stringify_json(agent, &a.arg(0))? else {
        return agent.type_error(
            "Failed to execute 'json' on 'Response': The data is not JSON serializable",
        );
    };
    let (d, list) = init_response(
        agent,
        &a.arg(1),
        Some((text.into_bytes(), Some("application/json".into()))),
    )?;
    make_response(agent, None, d, list, Guard::Response)
}

fn res_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&this_res(agent, a)?.type_))
}

fn res_url(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_res(agent, a)?;
    Ok(Value::str(&d.url.map_or_else(String::new, |mut u| {
        u.fragment = None;
        u.href()
    })))
}

fn res_redirected(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(this_res(agent, a)?.redirected))
}

fn res_status(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(f64::from(this_res(agent, a)?.status)))
}

fn res_ok(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_res(agent, a)?.status;
    Ok(Value::Boolean((200..300).contains(&s)))
}

fn res_status_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&this_res(agent, a)?.status_text))
}

fn res_clone(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_res(agent, a)?;
    if d.body.used {
        return agent
            .type_error("Failed to execute 'clone' on 'Response': Response body is already used");
    }
    let list = header_list(agent, &a.this);
    let hv = slot(agent, &a.this, 0);
    let guard = headers::data(agent, &hv).map_or(Guard::Response, |h| h.guard);
    make_response(agent, None, d, list, guard)
}

// --- fetch() (Fetch §5.6) ---

/// Una llamada a `fetch()` en curso.
pub(crate) struct FetchCall {
    promise: HostHandle,
    net: Option<u64>,
    signal: Option<HostHandle>,
}

thread_local! {
    static CALLS: std::cell::RefCell<std::collections::HashMap<u64, FetchCall>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    static NEXT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// La razón de un `AbortSignal` (`signal.reason`, o un `AbortError`).
fn abort_reason(agent: &mut Agent, signal: &Value) -> Value {
    if let Value::Object(o) = signal
        && let Ok(r) = agent.get(*o, &PropertyKey::from("reason"), signal.clone())
        && !r.is_undefined()
    {
        return r;
    }
    crate::dom::exception::create(agent, "AbortError", "The operation was aborted.")
}

fn signal_aborted(agent: &mut Agent, signal: &Value) -> bool {
    match signal {
        Value::Object(o) => agent
            .get(*o, &PropertyKey::from("aborted"), signal.clone())
            .is_ok_and(|v| matches!(v, Value::Boolean(true))),
        _ => false,
    }
}

fn reject(agent: &mut Agent, p: bf_js::heap::Gc<bf_js::object::Object>, e: Value) {
    agent.reject_promise(p, e);
}

/// Termina una llamada: saca su promesa y suelta la señal.
fn finish_call(agent: &mut Agent, id: u64) -> Option<bf_js::heap::Gc<bf_js::object::Object>> {
    let call = CALLS.with(|c| c.borrow_mut().remove(&id))?;
    if let Some(s) = call.signal {
        agent.host_unpin(s);
    }
    match agent.host_unpin(call.promise) {
        Some(Value::Object(p)) => Some(p),
        _ => None,
    }
}

/// El listener de `abort` de la señal de un `fetch()` ("abort the fetch() call").
fn on_abort(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = match &agent.heap[a.callee].kind {
        bf_js::object::ObjectKind::Builtin(b) => {
            b.slots.first().and_then(Value::as_number).unwrap_or(0.0) as u64
        }
        _ => 0,
    };
    let net = CALLS.with(|c| c.borrow().get(&id).and_then(|x| x.net));
    let signal = CALLS.with(|c| c.borrow().get(&id).and_then(|x| x.signal));
    let sig = signal
        .and_then(|h| agent.host_pinned(h))
        .unwrap_or(Value::Null);
    let reason = abort_reason(agent, &sig);
    if let Some(p) = finish_call(agent, id) {
        if let Some(n) = net {
            super::cancel(agent, n);
        }
        reject(agent, p, reason);
    }
    Ok(Value::Undefined)
}

/// La petición de red de un `Request` (y si sus cookies van).
pub fn net_request(agent: &mut Agent, r: &RequestData, list: &HeaderList) -> NetRequest {
    let same = super::same_origin(agent, &r.url);
    let credentials = match r.credentials.as_str() {
        "omit" => false,
        "include" => true,
        _ => same,
    };
    let mut headers = bf_net::Headers::default();
    for (n, v) in &list.0 {
        headers.push(n, v);
    }
    if !same && r.mode == "cors" {
        headers.push("Origin", &super::document_origin(agent));
    }
    let referrer = match r.referrer.as_str() {
        "" => None,
        "about:client" => Some(super::base_url(agent)),
        u => Url::parse(u, None).ok(),
    };
    NetRequest {
        url: r.url.clone(),
        options: bf_net::RequestOptions {
            method: r.method.clone(),
            headers,
            body: r.body.source.as_ref().map(|b| b.to_vec()),
            redirect: match r.redirect.as_str() {
                "error" => bf_net::RedirectMode::Error,
                "manual" => bf_net::RedirectMode::Manual,
                _ => bf_net::RedirectMode::Follow,
            },
            credentials,
            referrer,
        },
    }
}

/// Convierte el resultado de la red en un `Response` según el modo (o un error de red).
pub fn to_response(
    agent: &mut Agent,
    r: &RequestData,
    result: NetResult,
) -> Result<(ResponseData, HeaderList), String> {
    let res = result?;
    let final_url = res
        .url_list
        .last()
        .cloned()
        .unwrap_or_else(|| r.url.clone());
    let same = super::same_origin(agent, &final_url) && super::same_origin(agent, &r.url);
    let credentials = r.credentials == "include";
    let redirected = res.url_list.len() > 1;
    if r.redirect == "manual" && matches!(res.status, 301 | 302 | 303 | 307 | 308) {
        return Ok((
            ResponseData {
                type_: "opaqueredirect".into(),
                url: Some(final_url),
                redirected: false,
                status: 0,
                status_text: String::new(),
                body: Body::default(),
            },
            HeaderList::default(),
        ));
    }
    if same {
        let body = (!matches!(res.status, 101 | 103 | 204 | 205 | 304)).then_some(res.body);
        return Ok((
            ResponseData {
                type_: "basic".into(),
                url: Some(final_url),
                redirected,
                status: res.status,
                status_text: res.status_text,
                body: Body::new(body),
            },
            super::basic_filter(&res.headers),
        ));
    }
    match r.mode.as_str() {
        "same-origin" => Err("Failed to fetch: cross-origin request in same-origin mode".into()),
        "no-cors" => Ok((
            ResponseData {
                type_: "opaque".into(),
                url: None,
                redirected: false,
                status: 0,
                status_text: String::new(),
                body: Body::default(),
            },
            HeaderList::default(),
        )),
        _ => {
            let origin = super::document_origin(agent);
            if !super::cors_check(&origin, &res.headers, credentials) {
                return Err("Failed to fetch: CORS check failed".into());
            }
            Ok((
                ResponseData {
                    type_: "cors".into(),
                    url: Some(final_url),
                    redirected,
                    status: res.status,
                    status_text: res.status_text,
                    body: Body::new(Some(res.body)),
                },
                super::cors_filter(&res.headers),
            ))
        }
    }
}

/// `fetch(input, init)` (Fetch §5.6).
fn fetch(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let p = agent.new_internal_promise();
    let built = build_request(agent, &a.arg(0), &a.arg(1));
    let (r, hd, signal) = match built {
        Ok(x) => x,
        Err(Throw::Value(e)) => {
            agent.reject_promise(p, e);
            return Ok(Value::Object(p));
        }
        Err(t) => return Err(t),
    };
    // El Request de entrada queda usado si tenía cuerpo (Fetch §5.6 paso 2 vía el
    // constructor).
    if signal_aborted(agent, &signal) {
        let reason = abort_reason(agent, &signal);
        agent.reject_promise(p, reason);
        return Ok(Value::Object(p));
    }
    if !matches!(
        r.url.scheme.as_str(),
        "http" | "https" | "data" | "blob" | "file" | "about"
    ) {
        let e = agent
            .type_error::<Value>("Failed to fetch: unsupported URL scheme")
            .err();
        if let Some(Throw::Value(e)) = e {
            agent.reject_promise(p, e);
        }
        return Ok(Value::Object(p));
    }
    let id = NEXT.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    let promise = agent.host_pin(Value::Object(p));
    let sig_pin = (!signal.is_null()).then(|| agent.host_pin(signal.clone()));
    CALLS.with(|c| {
        c.borrow_mut().insert(
            id,
            FetchCall {
                promise,
                net: None,
                signal: sig_pin,
            },
        );
    });
    if let Value::Object(so) = &signal {
        let listener =
            agent.create_builtin_with_slots(on_abort, 1, "", vec![Value::Number(id as f64)]);
        let add = agent.get(*so, &PropertyKey::from("addEventListener"), signal.clone())?;
        if agent.is_callable(&add) {
            agent.call(
                &add,
                signal.clone(),
                &[Value::str("abort"), Value::Object(listener)],
            )?;
        }
    }
    let req = net_request(agent, &r, &hd.list);
    let rd = r.clone();
    let net = super::start(
        agent,
        req,
        Box::new(move |agent: &mut Agent, result: NetResult| {
            let Some(p) = finish_call(agent, id) else {
                return;
            };
            match to_response(agent, &rd, result) {
                Ok((d, list)) => match make_response(agent, None, d, list, Guard::Immutable) {
                    Ok(v) => {
                        let _ = agent.resolve_promise(p, v);
                    }
                    Err(Throw::Value(e)) => reject(agent, p, e),
                    Err(_) => {}
                },
                Err(msg) => {
                    if let Err(Throw::Value(e)) = agent.type_error::<Value>(&msg) {
                        reject(agent, p, e);
                    }
                }
            }
        }),
    );
    CALLS.with(|c| {
        if let Some(call) = c.borrow_mut().get_mut(&id) {
            call.net = Some(net);
        }
    });
    let _ = state(agent);
    Ok(Value::Object(p))
}

// --- navigator.sendBeacon (Beacon §3.1) ---

/// Agrega `sendBeacon` a `Navigator.prototype` de `realm` (si existe).
pub fn install_send_beacon(agent: &mut Agent, realm: RealmId) {
    let g = agent.realms[realm].global_object;
    let Ok(Value::Object(nav)) = agent.get(g, &PropertyKey::from("Navigator"), Value::Object(g))
    else {
        return;
    };
    let kp = agent.keys.prototype.clone();
    let Ok(Value::Object(proto)) = agent.get(nav, &kp, Value::Object(nav)) else {
        return;
    };
    bf_js::builtins::method(agent, proto, "sendBeacon", 1, send_beacon);
}

fn send_beacon(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = body::usv_utf8(agent, &a.arg(0))?;
    let base = super::base_url(agent);
    let Ok(url) = Url::parse(&s, Some(&base)) else {
        return agent.type_error(&format!(
            "Failed to execute 'sendBeacon' on 'Navigator': Invalid URL '{s}'"
        ));
    };
    if !matches!(url.scheme.as_str(), "http" | "https") {
        return agent.type_error("Failed to execute 'sendBeacon' on 'Navigator': Beacons are only supported over HTTP(S).");
    }
    let data = a.arg(1);
    let (bytes, ct) = if data.is_nullish() {
        (Vec::new(), None)
    } else {
        body::extract(agent, &data)?
    };
    // La cuota de keepalive (Fetch §4.1): 64 KiB.
    if bytes.len() > 65536 {
        return Ok(Value::Boolean(false));
    }
    let mut headers = bf_net::Headers::default();
    if let Some(ct) = ct {
        headers.push("Content-Type", &ct);
    }
    let same = super::same_origin(agent, &url);
    if !same {
        headers.push("Origin", &super::document_origin(agent));
    }
    let req = NetRequest {
        url,
        options: bf_net::RequestOptions {
            method: "POST".into(),
            headers,
            body: Some(bytes),
            redirect: bf_net::RedirectMode::Follow,
            credentials: true,
            referrer: Some(base),
        },
    };
    super::start(agent, req, Box::new(|_, _| {}));
    Ok(Value::Boolean(true))
}
