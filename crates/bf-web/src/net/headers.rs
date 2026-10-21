//! Las listas de cabeceras (Fetch §2.2.2) y la interfaz `Headers` (Fetch §5.2).
//!
//! Los valores son *byte strings*: se guardan como `String` donde cada `char` es un byte
//! (U+0000–U+00FF), igual que los ve JavaScript.

use bf_js::host::HostClass;
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use super::{classes, iterator_from_pairs};

/// Una lista de cabeceras: pares nombre/valor en orden, con nombres repetidos.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HeaderList(pub Vec<(String, String)>);

/// El guard de un objeto `Headers` (Fetch §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guard {
    Immutable,
    Request,
    RequestNoCors,
    Response,
    None,
}

/// "HTTP whitespace" (Fetch §2.2).
fn is_http_ws(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | ' ')
}

/// "Normalize" un valor de cabecera: sin espacios HTTP a los lados.
pub fn normalize(v: &str) -> String {
    v.trim_matches(is_http_ws).to_owned()
}

/// ¿Es `s` un `token` de HTTP (RFC 9110 §5.6.2)?
pub fn is_token(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c)
        })
}

/// "Header value" (Fetch §2.2.2): sin NUL, LF ni CR, ni espacios a los lados.
pub fn is_value(v: &str) -> bool {
    !v.contains(['\0', '\n', '\r'])
        && !v.starts_with(is_http_ws)
        && !v.ends_with(is_http_ws)
        && v.chars().all(|c| (c as u32) <= 0xFF)
}

/// "Forbidden request-header" (Fetch §2.2.2).
pub fn is_forbidden_request_header(name: &str, value: &str) -> bool {
    let n = name.to_ascii_lowercase();
    const NAMES: &[&str] = &[
        "accept-charset",
        "accept-encoding",
        "access-control-request-headers",
        "access-control-request-method",
        "connection",
        "content-length",
        "cookie",
        "cookie2",
        "date",
        "dnt",
        "expect",
        "host",
        "keep-alive",
        "origin",
        "referer",
        "set-cookie",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
        "via",
    ];
    if NAMES.contains(&n.as_str()) || n.starts_with("proxy-") || n.starts_with("sec-") {
        return true;
    }
    if matches!(
        n.as_str(),
        "x-http-method" | "x-http-method-override" | "x-method-override"
    ) {
        return value
            .split(',')
            .map(|m| m.trim_matches(is_http_ws))
            .any(is_forbidden_method);
    }
    false
}

/// "Forbidden response-header name".
pub fn is_forbidden_response_header(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "set-cookie" || n == "set-cookie2"
}

/// "Forbidden method" (Fetch §2.2.1).
pub fn is_forbidden_method(m: &str) -> bool {
    ["CONNECT", "TRACE", "TRACK"]
        .iter()
        .any(|f| f.eq_ignore_ascii_case(m))
}

/// "CORS-unsafe request-header byte".
fn is_cors_unsafe_byte(c: char) -> bool {
    let b = c as u32;
    (b < 0x20 && b != 0x09)
        || matches!(
            c,
            '"' | '(' | ')' | ':' | '<' | '>' | '?' | '@' | '[' | '\\' | ']' | '{' | '}' | '\u{7f}'
        )
}

/// "CORS-safelisted request-header" (Fetch §2.2.2).
pub fn is_cors_safelisted(name: &str, value: &str) -> bool {
    if value.len() > 128 {
        return false;
    }
    match name.to_ascii_lowercase().as_str() {
        "accept" => !value.chars().any(is_cors_unsafe_byte),
        "accept-language" | "content-language" => value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || " *,-.;=".contains(c)),
        "content-type" => {
            if value.chars().any(is_cors_unsafe_byte) {
                return false;
            }
            let essence = super::body::mime_essence(value);
            matches!(
                essence.as_deref(),
                Some("application/x-www-form-urlencoded" | "multipart/form-data" | "text/plain")
            )
        }
        "range" => {
            let Some(rest) = value.strip_prefix("bytes=") else {
                return false;
            };
            let Some((a, b)) = rest.split_once('-') else {
                return false;
            };
            let digits = |s: &str| s.chars().all(|c| c.is_ascii_digit());
            !a.is_empty()
                && digits(a)
                && digits(b)
                && (b.is_empty() || a.parse::<u64>().ok() <= b.parse::<u64>().ok())
        }
        _ => false,
    }
}

/// "No-CORS-safelisted request-header name".
fn is_no_cors_safelisted_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "accept" | "accept-language" | "content-language" | "content-type"
    )
}

/// "Privileged no-CORS request-header name".
fn is_privileged_no_cors(name: &str) -> bool {
    name.eq_ignore_ascii_case("range")
}

impl HeaderList {
    /// "Contains" (insensible a mayúsculas).
    pub fn contains(&self, name: &str) -> bool {
        self.0.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
    }

    /// "Get": los valores de `name` unidos con `", "`.
    pub fn get(&self, name: &str) -> Option<String> {
        let vals: Vec<&str> = self
            .0
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .collect();
        if vals.is_empty() {
            None
        } else {
            Some(vals.join(", "))
        }
    }

    /// "Append": un nombre ya presente reusa su capitalización.
    pub fn append(&mut self, name: &str, value: &str) {
        let name = self
            .0
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map_or_else(|| name.to_owned(), |(n, _)| n.clone());
        self.0.push((name, value.to_owned()));
    }

    pub fn delete(&mut self, name: &str) {
        self.0.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
    }

    /// "Set": reemplaza el primero y quita los demás.
    pub fn set(&mut self, name: &str, value: &str) {
        if let Some(i) = self.0.iter().position(|(n, _)| n.eq_ignore_ascii_case(name)) {
            self.0[i].1 = value.to_owned();
            let mut k = 0;
            self.0.retain(|(n, _)| {
                let keep = k <= i || !n.eq_ignore_ascii_case(name);
                k += 1;
                keep
            });
        } else {
            self.0.push((name.to_owned(), value.to_owned()));
        }
    }

    /// "Combine": agrega `value` al valor existente separado por `", "`.
    pub fn combine(&mut self, name: &str, value: &str) {
        if let Some((_, v)) = self.0.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
            v.push_str(", ");
            v.push_str(value);
        } else {
            self.0.push((name.to_owned(), value.to_owned()));
        }
    }

    /// "Sort and combine" (Fetch §2.2.2): nombres en minúsculas y ordenados; `set-cookie`
    /// queda separado.
    pub fn sort_and_combine(&self) -> Vec<(String, String)> {
        let mut names: Vec<String> = self.0.iter().map(|(n, _)| n.to_ascii_lowercase()).collect();
        names.sort();
        names.dedup();
        let mut out = Vec::new();
        for n in names {
            if n == "set-cookie" {
                for (k, v) in &self.0 {
                    if k.eq_ignore_ascii_case(&n) {
                        out.push((n.clone(), v.clone()));
                    }
                }
            } else if let Some(v) = self.get(&n) {
                out.push((n, v));
            }
        }
        out
    }

    /// "Extract a MIME type" (Fetch §3.1.8): el `Content-Type` combinado, o `None`.
    pub fn mime_type(&self) -> Option<String> {
        self.get("content-type")
    }
}

// --- La interfaz Headers ---

/// El payload de un `Headers`.
#[derive(Debug, Clone)]
pub struct HeadersData {
    pub list: HeaderList,
    pub guard: Guard,
}

pub fn class() -> HostClass {
    HostClass::new("Headers")
        .constructor(constructor, 0)
        .method("append", 2, append)
        .method("delete", 1, delete)
        .method("get", 1, get)
        .method("getSetCookie", 0, get_set_cookie)
        .method("has", 1, has)
        .method("set", 2, set)
        .method("forEach", 1, for_each)
        .method("keys", 0, keys)
        .method("values", 0, values)
        .method("entries", 0, entries)
}

/// Un `Headers` nuevo con `list` y `guard`.
pub fn create(agent: &mut Agent, list: HeaderList, guard: Guard) -> Value {
    let class = classes(agent).headers;
    Value::Object(agent.host_create(class, Box::new(HeadersData { list, guard })))
}

/// Los datos de un `Headers`.
pub fn data(agent: &Agent, v: &Value) -> Option<HeadersData> {
    agent.host_unwrap::<HeadersData>(v).cloned()
}

fn this_data<'a>(agent: &'a mut Agent, a: &CallArgs<'_>) -> JsResult<&'a mut HeadersData> {
    if agent.host_unwrap::<HeadersData>(&a.this).is_none() {
        return agent.type_error("Illegal invocation");
    }
    Ok(agent
        .host_unwrap_mut::<HeadersData>(&a.this)
        .expect("Headers"))
}

/// `ByteString` (WebIDL §3.2.11): TypeError si hay un code unit mayor que 0xFF.
pub fn byte_string(agent: &mut Agent, v: &Value) -> JsResult<String> {
    let s = agent.to_string(v)?;
    let mut out = String::new();
    for u in s.units() {
        if *u > 0xFF {
            return agent.type_error("Cannot convert argument to a ByteString because the character exceeds 255");
        }
        out.push(char::from(*u as u8));
    }
    Ok(out)
}

/// "Validate" (Fetch §5.2): `Ok(false)` si la operación se ignora en silencio.
fn validate(agent: &mut Agent, d: &HeadersData, name: &str, value: &str) -> JsResult<bool> {
    if !is_token(name) {
        return agent.type_error(&format!("Invalid header name: '{name}'"));
    }
    if !is_value(value) {
        return agent.type_error(&format!("Invalid header value for '{name}'"));
    }
    match d.guard {
        Guard::Immutable => agent.type_error("Headers are immutable"),
        Guard::Request if is_forbidden_request_header(name, value) => Ok(false),
        Guard::Response if is_forbidden_response_header(name) => Ok(false),
        _ => Ok(true),
    }
}

/// "Append" a un `Headers` (Fetch §5.2).
pub fn append_to(agent: &mut Agent, d: &mut HeadersData, name: &str, value: &str) -> JsResult<()> {
    let value = normalize(value);
    if !validate(agent, d, name, &value)? {
        return Ok(());
    }
    if d.guard == Guard::RequestNoCors {
        let combined = match d.list.get(name) {
            Some(t) => format!("{t}, {value}"),
            None => value.clone(),
        };
        if !is_cors_safelisted(name, &combined) {
            return Ok(());
        }
    }
    d.list.append(name, &value);
    if d.guard == Guard::RequestNoCors {
        remove_privileged(&mut d.list);
    }
    Ok(())
}

/// "Remove privileged no-CORS request-headers".
fn remove_privileged(list: &mut HeaderList) {
    list.0.retain(|(n, _)| !is_privileged_no_cors(n));
}

/// "Fill" (Fetch §5.2) desde `HeadersInit`: secuencia de pares o record.
pub fn fill(agent: &mut Agent, d: &mut HeadersData, init: &Value) -> JsResult<()> {
    let Value::Object(o) = init else {
        return agent.type_error("Failed to construct 'Headers': The provided value is not of type '(record<ByteString, ByteString> or sequence<sequence<ByteString>>)'");
    };
    let o = *o;
    // Un `Headers` como init: copia su lista.
    if let Some(other) = data(agent, init) {
        for (n, v) in other.list.0 {
            append_to(agent, d, &n, &v)?;
        }
        return Ok(());
    }
    let iter_key = PropertyKey::Symbol(agent.well_known.iterator);
    let method = agent.get(o, &iter_key, init.clone())?;
    if !method.is_nullish() {
        if !agent.is_callable(&method) {
            return agent.type_error("@@iterator is not callable");
        }
        let pairs = collect_iterable(agent, init, &method)?;
        for p in pairs {
            let items = match &p {
                Value::Object(_) => {
                    let m = agent.get_method_iterator(&p)?;
                    collect_iterable(agent, &p, &m)?
                }
                _ => return agent.type_error("Each header pair must be an iterable"),
            };
            if items.len() != 2 {
                return agent.type_error("Each header pair must be a sequence of two values");
            }
            let n = byte_string(agent, &items[0])?;
            let v = byte_string(agent, &items[1])?;
            append_to(agent, d, &n, &v)?;
        }
        return Ok(());
    }
    // record<ByteString, ByteString>: las claves propias enumerables en orden.
    let keys = agent.own_property_keys(o)?;
    for k in keys {
        let PropertyKey::String(_) | PropertyKey::Index(_) = &k else {
            continue;
        };
        let Some(desc) = agent.get_own_property(o, &k)? else {
            continue;
        };
        if desc.enumerable != Some(true) {
            continue;
        }
        let kv = k.to_value(agent);
        let n = byte_string(agent, &kv)?;
        let v = agent.get(o, &k, init.clone())?;
        let v = byte_string(agent, &v)?;
        append_to(agent, d, &n, &v)?;
    }
    Ok(())
}

/// Los valores de un iterable (para `sequence<T>`).
pub fn collect_iterable(agent: &mut Agent, v: &Value, method: &Value) -> JsResult<Vec<Value>> {
    let iter = agent.call(method, v.clone(), &[])?;
    let Value::Object(it) = iter else {
        return agent.type_error("Result of the Symbol.iterator method is not an object");
    };
    let next = agent.get(it, &PropertyKey::from("next"), iter.clone())?;
    let mut out = Vec::new();
    loop {
        let r = agent.call(&next, iter.clone(), &[])?;
        let Value::Object(ro) = r else {
            return agent.type_error("Iterator result is not an object");
        };
        let done = agent.get(ro, &PropertyKey::from("done"), r.clone())?;
        if bf_js::host::webidl::to_boolean(agent, &done) {
            break;
        }
        out.push(agent.get(ro, &PropertyKey::from("value"), r.clone())?);
    }
    Ok(out)
}

fn constructor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Illegal constructor");
    };
    let mut d = HeadersData {
        list: HeaderList::default(),
        guard: Guard::None,
    };
    let init = a.arg(0);
    if !init.is_undefined() {
        fill(agent, &mut d, &init)?;
    }
    let class = classes(agent).headers;
    Ok(Value::Object(agent.host_create_from_new_target(
        nt,
        class,
        None,
        Box::new(d),
    )?))
}

fn append(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = byte_string(agent, &a.arg(0))?;
    let value = byte_string(agent, &a.arg(1))?;
    let mut d = this_data(agent, a)?.clone();
    append_to(agent, &mut d, &name, &value)?;
    *this_data(agent, a)? = d;
    Ok(Value::Undefined)
}

fn delete(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = byte_string(agent, &a.arg(0))?;
    let d = this_data(agent, a)?.clone();
    if !is_token(&name) {
        return agent.type_error(&format!("Invalid header name: '{name}'"));
    }
    match d.guard {
        Guard::Immutable => return agent.type_error("Headers are immutable"),
        Guard::Request if is_forbidden_request_header(&name, "") => return Ok(Value::Undefined),
        Guard::RequestNoCors
            if !is_no_cors_safelisted_name(&name) && !is_privileged_no_cors(&name) =>
        {
            return Ok(Value::Undefined);
        }
        Guard::Response if is_forbidden_response_header(&name) => return Ok(Value::Undefined),
        _ => {}
    }
    let d = this_data(agent, a)?;
    d.list.delete(&name);
    if d.guard == Guard::RequestNoCors {
        remove_privileged(&mut d.list);
    }
    Ok(Value::Undefined)
}

fn get(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = byte_string(agent, &a.arg(0))?;
    if !is_token(&name) {
        return agent.type_error(&format!("Invalid header name: '{name}'"));
    }
    let d = this_data(agent, a)?;
    Ok(d.list.get(&name).map_or(Value::Null, |v| Value::str(&v)))
}

fn get_set_cookie(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_data(agent, a)?.clone();
    let vals: Vec<Value> = d
        .list
        .0
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, v)| Value::str(v))
        .collect();
    Ok(Value::Object(agent.create_array_from_list(&vals)?))
}

fn has(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = byte_string(agent, &a.arg(0))?;
    if !is_token(&name) {
        return agent.type_error(&format!("Invalid header name: '{name}'"));
    }
    let d = this_data(agent, a)?;
    Ok(Value::Boolean(d.list.contains(&name)))
}

fn set(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = byte_string(agent, &a.arg(0))?;
    let value = normalize(&byte_string(agent, &a.arg(1))?);
    let d = this_data(agent, a)?.clone();
    if !validate(agent, &d, &name, &value)? {
        return Ok(Value::Undefined);
    }
    if d.guard == Guard::RequestNoCors && !is_cors_safelisted(&name, &value) {
        return Ok(Value::Undefined);
    }
    let d = this_data(agent, a)?;
    d.list.set(&name, &value);
    if d.guard == Guard::RequestNoCors {
        remove_privileged(&mut d.list);
    }
    Ok(Value::Undefined)
}

fn pairs(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Vec<(String, String)>> {
    Ok(this_data(agent, a)?.list.sort_and_combine())
}

fn for_each(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let cb = a.arg(0);
    if !agent.is_callable(&cb) {
        return agent.type_error("Headers.forEach: callback is not a function");
    }
    // Se recalcula en cada paso: el callback puede cambiar la lista.
    let mut i = 0;
    loop {
        let ps = pairs(agent, a)?;
        let Some((n, v)) = ps.get(i).cloned() else {
            break;
        };
        agent.call(&cb, a.arg(1), &[Value::str(&v), Value::str(&n), a.this.clone()])?;
        i += 1;
    }
    Ok(Value::Undefined)
}

fn keys(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let ps = pairs(agent, a)?;
    iterator_from_pairs(agent, "Headers Iterator", ps, 0)
}

fn values(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let ps = pairs(agent, a)?;
    iterator_from_pairs(agent, "Headers Iterator", ps, 1)
}

fn entries(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let ps = pairs(agent, a)?;
    iterator_from_pairs(agent, "Headers Iterator", ps, 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list() {
        let mut l = HeaderList::default();
        l.append("A", "1");
        l.append("b", "2");
        l.append("a", "3");
        l.append("Set-Cookie", "x");
        l.append("set-cookie", "y");
        assert_eq!(l.get("a").as_deref(), Some("1, 3"));
        assert_eq!(l.0[2].0, "A");
        assert_eq!(
            l.sort_and_combine(),
            vec![
                ("a".to_owned(), "1, 3".to_owned()),
                ("b".to_owned(), "2".to_owned()),
                ("set-cookie".to_owned(), "x".to_owned()),
                ("set-cookie".to_owned(), "y".to_owned()),
            ]
        );
        l.set("a", "9");
        assert_eq!(l.get("A").as_deref(), Some("9"));
        assert!(is_forbidden_request_header("Sec-Fetch-Mode", ""));
        assert!(is_forbidden_request_header("X-HTTP-Method-Override", "trace"));
        assert!(!is_forbidden_request_header("X-Foo", ""));
        assert!(is_cors_safelisted("content-type", "text/plain;charset=utf-8"));
        assert!(!is_cors_safelisted("content-type", "application/json"));
        assert_eq!(normalize("  a b \t"), "a b");
    }
}
