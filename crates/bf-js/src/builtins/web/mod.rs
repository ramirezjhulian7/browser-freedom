//! Las Web APIs puras de JavaScript (las que no dependen del DOM ni del bucle de eventos):
//! `URL` y `URLSearchParams` (URL Standard §6), `TextEncoder`/`TextDecoder` (Encoding Standard
//! §8), `atob`/`btoa` (HTML §8.3), `structuredClone` (HTML §2.7), `queueMicrotask` (HTML
//! §8.6), `crypto` (Web Crypto §10), `performance` (High Resolution Time §5) y `console`
//! (Console Standard), más `DOMException` (WebIDL §3.14.1), que todas usan.
//!
//! Las interfaces siguen las reglas de WebIDL (§3.7): el constructor exige `new`, los
//! atributos son accessors y las operaciones propiedades de datos, todos enumerables, y cada
//! prototipo tiene su `@@toStringTag`. Los prototipos y constructores de cada realm se guardan
//! en el registro `web_registry` (para GetPrototypeFromConstructor con un newTarget de otro
//! realm).

pub mod base64;
pub mod clone;
pub mod console;
pub mod crypto;
pub mod encoding;
pub mod performance;
pub mod url;

use std::collections::HashMap;
use std::fmt;

use crate::heap::{Gc, Trace, Tracer};
use crate::object::{Object, ObjectKind, Prop, PropValue};
use crate::string::JsString;
use crate::value::{PropertyKey, Value};
use crate::vm::function::{CallArgs, NativeFn};
use crate::vm::{Agent, JsResult, RealmId, Throw};

/// Los internal slots de los objetos de las Web APIs y de Intl.
pub enum WebData {
    /// El registro de prototipos y constructores del realm.
    Registry(Box<HashMap<&'static str, Gc<Object>>>),
    Url(Box<url::UrlData>),
    SearchParams(Box<url::SearchParamsData>),
    SearchParamsIterator(Box<url::SearchParamsIter>),
    TextEncoder,
    TextDecoder(Box<encoding::DecoderData>),
    DomException(Box<DomExceptionData>),
    Console(Box<console::ConsoleState>),
    Intl(Box<crate::builtins::intl::IntlData>),
}

impl fmt::Debug for WebData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            WebData::Registry(_) => "Registry",
            WebData::Url(_) => "URL",
            WebData::SearchParams(_) => "URLSearchParams",
            WebData::SearchParamsIterator(_) => "URLSearchParams Iterator",
            WebData::TextEncoder => "TextEncoder",
            WebData::TextDecoder(_) => "TextDecoder",
            WebData::DomException(_) => "DOMException",
            WebData::Console(_) => "console",
            WebData::Intl(_) => "Intl",
        };
        f.write_str(name)
    }
}

impl Trace for WebData {
    fn trace(&self, t: &mut Tracer) {
        match self {
            WebData::Registry(m) => {
                for o in m.values() {
                    t.gc(*o);
                }
            }
            WebData::Url(u) => t.opt(u.query),
            WebData::SearchParams(p) => t.opt(p.url),
            WebData::SearchParamsIterator(i) => t.gc(i.params),
            WebData::Intl(i) => i.trace(t),
            WebData::TextEncoder
            | WebData::TextDecoder(_)
            | WebData::DomException(_)
            | WebData::Console(_) => {}
        }
    }
}

/// Los slots de un DOMException.
#[derive(Debug)]
pub struct DomExceptionData {
    pub name: JsString,
    pub message: JsString,
}

pub fn init(agent: &mut Agent, realm: RealmId) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    let reg = agent.alloc(Object::new(
        Some(op),
        ObjectKind::Web(Box::new(WebData::Registry(Box::default()))),
    ));
    agent.realms[realm].intrinsics.web_registry = reg;
    init_dom_exception(agent, realm);
    url::init(agent, realm);
    encoding::init(agent, realm);
    base64::init(agent, realm);
    clone::init(agent, realm);
    crypto::init(agent, realm);
    performance::init(agent, realm);
    console::init(agent, realm);
    let f = agent.create_builtin_function(queue_microtask, 1, "queueMicrotask", false);
    crate::builtins::register_global(agent, realm, "queueMicrotask", f);
}

// --- Registro y helpers de WebIDL ---

/// Un prototipo o constructor registrado del realm (`"URL"`, `"URL.prototype"`…).
pub(crate) fn registry_get(agent: &Agent, realm: RealmId, name: &str) -> Option<Gc<Object>> {
    let reg = agent.realms[realm].intrinsics.web_registry;
    match &agent.heap[reg].kind {
        ObjectKind::Web(w) => match &**w {
            WebData::Registry(m) => m.get(name).copied(),
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn registry_set(agent: &mut Agent, realm: RealmId, name: &'static str, o: Gc<Object>) {
    let reg = agent.realms[realm].intrinsics.web_registry;
    if let ObjectKind::Web(w) = &mut agent.heap[reg].kind
        && let WebData::Registry(m) = &mut **w
    {
        m.insert(name, o);
    }
}

/// Una operación de WebIDL: propiedad de datos `{writable, enumerable, configurable}`.
pub(crate) fn op(agent: &mut Agent, target: Gc<Object>, name: &str, length: u32, f: NativeFn) {
    let func = agent.create_builtin_function(f, length, name, false);
    agent.put_data(
        target,
        PropertyKey::from(name),
        Value::Object(func),
        true,
        true,
        true,
    );
}

/// Un atributo de WebIDL: accessor `{enumerable, configurable}`.
pub(crate) fn attr(
    agent: &mut Agent,
    target: Gc<Object>,
    name: &str,
    get: NativeFn,
    set: Option<NativeFn>,
) {
    let g = agent.create_builtin_function(get, 0, &format!("get {name}"), false);
    let s = set.map(|f| agent.create_builtin_function(f, 1, &format!("set {name}"), false));
    agent.heap[target].props.insert(
        PropertyKey::from(name),
        Prop {
            value: PropValue::Accessor {
                get: Some(g),
                set: s,
            },
            enumerable: true,
            configurable: true,
        },
    );
}

/// Una constante de WebIDL: `{writable: false, enumerable: true, configurable: false}`.
pub(crate) fn constant(agent: &mut Agent, target: Gc<Object>, name: &str, v: Value) {
    agent.put_data(target, PropertyKey::from(name), v, false, true, false);
}

/// Crea un interface object (§3.7.1) y su interface prototype object (§3.7.3), registra
/// ambos y el binding global. `parent` es el prototipo del prototipo.
pub(crate) fn interface(
    agent: &mut Agent,
    realm: RealmId,
    name: &'static str,
    proto_name: &'static str,
    length: u32,
    ctor: NativeFn,
    parent: Option<Gc<Object>>,
) -> (Gc<Object>, Gc<Object>) {
    let parent = parent.unwrap_or(agent.realms[realm].intrinsics.object_prototype);
    let proto = agent.new_object(Some(parent));
    let c = crate::builtins::constructor(agent, name, length, ctor, proto);
    crate::builtins::to_string_tag(agent, proto, name);
    registry_set(agent, realm, name, c);
    registry_set(agent, realm, proto_name, proto);
    crate::builtins::register_global(agent, realm, name, c);
    (c, proto)
}

/// Un objeto de la interfaz `proto_name` con los slots `data`, con el prototipo de
/// `new_target` (GetPrototypeFromConstructor, con el registro como valor por defecto).
pub(crate) fn create_from(
    agent: &mut Agent,
    new_target: Gc<Object>,
    proto_name: &str,
    data: WebData,
) -> JsResult<Gc<Object>> {
    let k = agent.keys.prototype.clone();
    let p = agent.get(new_target, &k, Value::Object(new_target))?;
    let proto = match p {
        Value::Object(p) => p,
        _ => {
            let realm = agent.get_function_realm(new_target)?;
            registry_get(agent, realm, proto_name)
                .unwrap_or(agent.realms[realm].intrinsics.object_prototype)
        }
    };
    Ok(agent.alloc(Object::new(Some(proto), ObjectKind::Web(Box::new(data)))))
}

/// Un objeto de la interfaz `proto_name` del realm actual.
pub(crate) fn create(agent: &mut Agent, proto_name: &str, data: WebData) -> Gc<Object> {
    let realm = agent.current_realm;
    let proto = registry_get(agent, realm, proto_name);
    agent.alloc(Object::new(proto, ObjectKind::Web(Box::new(data))))
}

/// Los slots de un objeto de las Web APIs.
pub(crate) fn data_of<'a>(agent: &'a Agent, v: &Value) -> Option<&'a WebData> {
    match v {
        Value::Object(o) => match &agent.heap[*o].kind {
            ObjectKind::Web(w) => Some(w),
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn data_of_mut(agent: &mut Agent, o: Gc<Object>) -> Option<&mut WebData> {
    match &mut agent.heap[o].kind {
        ObjectKind::Web(w) => Some(w),
        _ => None,
    }
}

/// Un constructor de WebIDL llamado sin `new`.
pub(crate) fn require_new(agent: &mut Agent, a: &CallArgs<'_>, name: &str) -> JsResult<Gc<Object>> {
    match a.new_target {
        Some(nt) => Ok(nt),
        None => agent.type_error(&format!(
            "Failed to construct '{name}': Please use the 'new' operator"
        )),
    }
}

/// Los argumentos obligatorios de una operación (WebIDL §3.7.6, paso "If args's size <
/// required").
pub(crate) fn require_args(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    n: usize,
    what: &str,
) -> JsResult<()> {
    if a.len() < n {
        return agent.type_error(&format!(
            "{what}: {n} argument{} required, but only {} present",
            if n == 1 { "" } else { "s" },
            a.len()
        ));
    }
    Ok(())
}

/// La conversión a USVString (WebIDL §3.2.11): ToString y los surrogates sueltos a U+FFFD.
pub(crate) fn usv_string(agent: &mut Agent, v: &Value) -> JsResult<String> {
    Ok(agent.to_string(v)?.to_string_lossy())
}

/// La conversión a DOMString (WebIDL §3.2.10).
pub(crate) fn dom_string(agent: &mut Agent, v: &Value) -> JsResult<JsString> {
    agent.to_string(v)
}

pub(crate) fn str_value(s: &str) -> Value {
    Value::String(JsString::from(s))
}

// --- DOMException (WebIDL §3.14.1) ---

/// Los nombres con código legacy (WebIDL, tabla "DOMException names").
const LEGACY_CODES: &[(&str, &str, u16)] = &[
    ("IndexSizeError", "INDEX_SIZE_ERR", 1),
    ("", "DOMSTRING_SIZE_ERR", 2),
    ("HierarchyRequestError", "HIERARCHY_REQUEST_ERR", 3),
    ("WrongDocumentError", "WRONG_DOCUMENT_ERR", 4),
    ("InvalidCharacterError", "INVALID_CHARACTER_ERR", 5),
    ("", "NO_DATA_ALLOWED_ERR", 6),
    (
        "NoModificationAllowedError",
        "NO_MODIFICATION_ALLOWED_ERR",
        7,
    ),
    ("NotFoundError", "NOT_FOUND_ERR", 8),
    ("NotSupportedError", "NOT_SUPPORTED_ERR", 9),
    ("InUseAttributeError", "INUSE_ATTRIBUTE_ERR", 10),
    ("InvalidStateError", "INVALID_STATE_ERR", 11),
    ("SyntaxError", "SYNTAX_ERR", 12),
    ("InvalidModificationError", "INVALID_MODIFICATION_ERR", 13),
    ("NamespaceError", "NAMESPACE_ERR", 14),
    ("InvalidAccessError", "INVALID_ACCESS_ERR", 15),
    ("", "VALIDATION_ERR", 16),
    ("TypeMismatchError", "TYPE_MISMATCH_ERR", 17),
    ("SecurityError", "SECURITY_ERR", 18),
    ("NetworkError", "NETWORK_ERR", 19),
    ("AbortError", "ABORT_ERR", 20),
    ("URLMismatchError", "URL_MISMATCH_ERR", 21),
    ("QuotaExceededError", "QUOTA_EXCEEDED_ERR", 22),
    ("TimeoutError", "TIMEOUT_ERR", 23),
    ("InvalidNodeTypeError", "INVALID_NODE_TYPE_ERR", 24),
    ("DataCloneError", "DATA_CLONE_ERR", 25),
];

fn init_dom_exception(agent: &mut Agent, realm: RealmId) {
    let ep = agent.realms[realm].intrinsics.error_prototype;
    let (c, p) = interface(
        agent,
        realm,
        "DOMException",
        "DOMException.prototype",
        0,
        dom_exception_ctor,
        Some(ep),
    );
    attr(agent, p, "name", dom_exception_name, None);
    attr(agent, p, "message", dom_exception_message, None);
    attr(agent, p, "code", dom_exception_code, None);
    for &(_, cname, code) in LEGACY_CODES {
        constant(agent, c, cname, Value::Number(f64::from(code)));
        constant(agent, p, cname, Value::Number(f64::from(code)));
    }
}

/// `new DOMException(message = "", name = "Error")`.
fn dom_exception_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let nt = require_new(agent, a, "DOMException")?;
    let message = if a.arg(0).is_undefined() {
        JsString::default()
    } else {
        dom_string(agent, &a.arg(0))?
    };
    let name = if a.arg(1).is_undefined() {
        JsString::from("Error")
    } else {
        dom_string(agent, &a.arg(1))?
    };
    let o = create_from(
        agent,
        nt,
        "DOMException.prototype",
        WebData::DomException(Box::new(DomExceptionData { name, message })),
    )?;
    agent.attach_stack(o);
    Ok(Value::Object(o))
}

fn dom_exception_slots(agent: &mut Agent, this: &Value) -> JsResult<(JsString, JsString)> {
    match data_of(agent, this) {
        Some(WebData::DomException(d)) => Ok((d.name.clone(), d.message.clone())),
        _ => agent.type_error("Illegal invocation"),
    }
}

fn dom_exception_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::String(dom_exception_slots(agent, &a.this)?.0))
}

fn dom_exception_message(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::String(dom_exception_slots(agent, &a.this)?.1))
}

fn dom_exception_code(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (name, _) = dom_exception_slots(agent, &a.this)?;
    let n = name.to_string_lossy();
    let code = LEGACY_CODES
        .iter()
        .find(|(nm, _, _)| !nm.is_empty() && *nm == n)
        .map_or(0, |&(_, _, c)| c);
    Ok(Value::Number(f64::from(code)))
}

/// Crea un DOMException `name` con `message` en el realm actual.
pub fn dom_exception(agent: &mut Agent, name: &str, message: &str) -> Gc<Object> {
    let o = create(
        agent,
        "DOMException.prototype",
        WebData::DomException(Box::new(DomExceptionData {
            name: JsString::from(name),
            message: JsString::from(message),
        })),
    );
    agent.attach_stack(o);
    o
}

/// Lanza un DOMException.
pub fn throw_dom<T>(agent: &mut Agent, name: &str, message: &str) -> JsResult<T> {
    Err(Throw::Value(Value::Object(dom_exception(
        agent, name, message,
    ))))
}

// --- queueMicrotask (HTML §8.6) ---

/// `queueMicrotask(callback)`: encola un microtask que llama a `callback` sin argumentos; una
/// excepción se reporta (HostReportErrors) sin cortar la cola.
fn queue_microtask(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    require_args(agent, a, 1, "Failed to execute 'queueMicrotask'")?;
    let cb = a.arg(0);
    if !agent.is_callable(&cb) {
        return agent.type_error(
            "Failed to execute 'queueMicrotask': parameter 1 is not of type 'Function'",
        );
    }
    let runner = agent.create_builtin_with_slots(run_microtask, 0, "", vec![cb]);
    let realm = agent.current_realm;
    agent.enqueue_job(crate::builtins::promise::Job::Reaction {
        reaction: crate::builtins::promise::Reaction {
            capability: None,
            kind: crate::builtins::promise::ReactionType::Fulfill,
            handler: crate::builtins::promise::Handler::Callback(Value::Object(runner)),
        },
        argument: Value::Undefined,
        realm,
    });
    Ok(Value::Undefined)
}

fn run_microtask(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let cb = crate::builtins::slots(agent, a.callee)
        .into_iter()
        .next()
        .unwrap_or_default();
    match agent.call(&cb, Value::Undefined, &[]) {
        Ok(_) => Ok(Value::Undefined),
        Err(Throw::Value(e)) => {
            agent.job_errors.push(e);
            Ok(Value::Undefined)
        }
        Err(t) => Err(t),
    }
}
