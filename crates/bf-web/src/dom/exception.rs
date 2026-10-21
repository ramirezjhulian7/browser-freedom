//! `DOMException` (WebIDL §3.14.1): `name`, `message`, el `code` heredado y las constantes;
//! su prototipo hereda de `%Error.prototype%`.

use bf_dom::DomException;
use bf_js::host::HostClass;
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, RealmId, Throw};

use crate::dom::classes;

/// El payload de un `DOMException`.
#[derive(Debug, Clone)]
pub struct DomExceptionData {
    pub name: String,
    pub message: String,
}

/// Los nombres con `code` heredado (WebIDL §3.14.2, tabla de nombres de error).
const LEGACY: [(&str, &str, u16); 25] = [
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

pub fn dom_exception_class() -> HostClass {
    let mut c = HostClass::new("DOMException")
        .constructor(ctor, 0)
        .attribute("name", Some(get_name), None)
        .attribute("message", Some(get_message), None)
        .attribute("code", Some(get_code), None);
    for (_, constant, code) in LEGACY {
        c = c.constant(constant, f64::from(code));
    }
    c
}

/// En cada realm: `DOMException.prototype` hereda de `%Error.prototype%`.
pub fn finish_realm(agent: &mut Agent, realm: RealmId) {
    let id = classes(agent).dom_exception;
    let proto = agent.host_class_objects(realm, id).prototype;
    let ep = agent.realms[realm].intrinsics.error_prototype;
    agent.heap[proto].proto = Some(ep);
}

fn data(agent: &Agent, v: &Value) -> DomExceptionData {
    agent
        .host_unwrap::<DomExceptionData>(v)
        .cloned()
        .unwrap_or(DomExceptionData {
            name: "Error".into(),
            message: String::new(),
        })
}

/// `new DOMException(message = "", name = "Error")`.
fn ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let message = if a.arg(0).is_undefined() {
        String::new()
    } else {
        agent.to_string(&a.arg(0))?.to_string_lossy()
    };
    let name = if a.arg(1).is_undefined() {
        "Error".to_owned()
    } else {
        agent.to_string(&a.arg(1))?.to_string_lossy()
    };
    let class = classes(agent).dom_exception;
    let nt = a.new_target.expect("new.target");
    let o = agent.host_create_from_new_target(
        nt,
        class,
        None,
        Box::new(DomExceptionData { name, message }),
    )?;
    agent.attach_stack(o);
    Ok(Value::Object(o))
}

fn get_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&data(agent, &a.this).name))
}

fn get_message(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&data(agent, &a.this).message))
}

fn get_code(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = data(agent, &a.this).name;
    let code = LEGACY
        .iter()
        .find(|(n, _, _)| !n.is_empty() && *n == name)
        .map_or(0, |(_, _, c)| *c);
    Ok(Value::Number(f64::from(code)))
}

/// Un `DOMException` nuevo con `name` y `message`.
pub fn create(agent: &mut Agent, name: &str, message: &str) -> Value {
    let class = classes(agent).dom_exception;
    let o = agent.host_create(
        class,
        Box::new(DomExceptionData {
            name: name.to_owned(),
            message: message.to_owned(),
        }),
    );
    agent.attach_stack(o);
    Value::Object(o)
}

/// Lanza `e` (`TypeError` es un error de ECMAScript; el resto, `DOMException`).
pub fn throw<T>(agent: &mut Agent, e: DomException) -> JsResult<T> {
    if e == DomException::TypeError {
        return agent.type_error(e.message());
    }
    let v = create(agent, e.name(), e.message());
    Err(Throw::Value(v))
}
