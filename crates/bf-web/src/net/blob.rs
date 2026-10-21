//! File API: `Blob` (§3), `File` (§4), `FileReader` (§6) y las URLs `blob:`
//! (`URL.createObjectURL`/`revokeObjectURL`, §8), guardadas en el almacén de `bf-net`.

use std::sync::Arc;

use bf_dom::events::EventTarget;
use bf_js::builtins::web::encoding::{buffer_source_bytes, new_array_buffer, new_uint8_array};
use bf_js::host::{HostClass, HostClassId, TaskSource};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, RealmId};

use super::body::{decode_with, settled, usv_utf8, utf8_decode};
use super::headers::collect_iterable;
use super::xhr::{fire_progress, new_target};
use super::{classes, dom_error, state};
use crate::dom::events::{TargetRef, with_event_handlers};

/// Los datos de un `Blob` (y, si es un `File`, su nombre y fecha).
#[derive(Debug, Clone)]
pub struct BlobData {
    pub bytes: Arc<Vec<u8>>,
    /// `type`: ya en minúsculas, o vacío.
    pub mime: String,
    /// `(name, lastModified)` de un `File`.
    pub file: Option<(String, f64)>,
}

/// Los datos de un `Blob`/`File`.
pub fn data(agent: &Agent, v: &Value) -> Option<BlobData> {
    agent.host_unwrap::<BlobData>(v).cloned()
}

/// Un `Blob` (o `File`, si `d.file` lo dice) nuevo.
pub fn create(agent: &mut Agent, d: BlobData) -> Value {
    let c = classes(agent);
    let class = if d.file.is_some() { c.file } else { c.blob };
    Value::Object(agent.host_create(class, Box::new(d)))
}

pub fn blob_class() -> HostClass {
    HostClass::new("Blob")
        .constructor(blob_ctor, 0)
        .attribute("size", Some(size), None)
        .attribute("type", Some(blob_type), None)
        .method("slice", 0, slice)
        .method("text", 0, text)
        .method("arrayBuffer", 0, array_buffer)
        .method("bytes", 0, bytes)
}

pub fn file_class(blob: HostClassId) -> HostClass {
    HostClass::new("File")
        .parent(blob)
        .constructor(file_ctor, 2)
        .attribute("name", Some(file_name), None)
        .attribute("lastModified", Some(last_modified), None)
        .attribute("webkitRelativePath", Some(relative_path), None)
}

/// El `type` de `BlobPropertyBag` (File API §3.1): vacío si tiene caracteres fuera de
/// U+0020–U+007E; si no, en minúsculas.
fn normalize_type(t: &str) -> String {
    if t.chars().any(|c| !(' '..='~').contains(&c)) {
        String::new()
    } else {
        t.to_ascii_lowercase()
    }
}

/// Las opciones `type` y `endings` de un `BlobPropertyBag`.
fn bag(agent: &mut Agent, v: &Value) -> JsResult<(String, bool, Option<f64>)> {
    let mut t = String::new();
    let mut native = false;
    let mut last = None;
    match v {
        Value::Undefined | Value::Null => {}
        Value::Object(o) => {
            // Orden de los miembros del diccionario: endings, lastModified, type.
            let e = agent.get(*o, &PropertyKey::from("endings"), v.clone())?;
            if !e.is_undefined() {
                match agent.to_string(&e)?.to_string().as_str() {
                    "transparent" => {}
                    "native" => native = true,
                    other => {
                        return agent.type_error(&format!(
                            "The provided value '{other}' is not a valid enum value of type EndingType."
                        ));
                    }
                }
            }
            let lm = agent.get(*o, &PropertyKey::from("lastModified"), v.clone())?;
            if !lm.is_undefined() {
                let n = agent.to_number(&lm)?;
                // `long long`: ToNumber y truncado.
                last = Some(if n.is_finite() { n.trunc() } else { 0.0 });
            }
            let ty = agent.get(*o, &PropertyKey::from("type"), v.clone())?;
            if !ty.is_undefined() {
                t = normalize_type(&agent.to_string(&ty)?.to_string());
            }
        }
        _ => return agent.type_error("The provided value is not of type 'BlobPropertyBag'"),
    }
    Ok((t, native, last))
}

/// "Process blob parts" (File API §3.1.1).
fn process_parts(agent: &mut Agent, parts: &Value, native: bool) -> JsResult<Vec<u8>> {
    let mut out = Vec::new();
    match parts {
        Value::Undefined => return Ok(out),
        Value::Object(o) => {
            let k = PropertyKey::Symbol(agent.well_known.iterator);
            let m = agent.get(*o, &k, parts.clone())?;
            if !agent.is_callable(&m) {
                return agent.type_error(
                    "Failed to construct 'Blob': The provided value cannot be converted to a sequence.",
                );
            }
            for p in collect_iterable(agent, parts, &m)? {
                if let Some(b) = data(agent, &p) {
                    out.extend_from_slice(&b.bytes);
                } else if let Some(bytes) = buffer_source_bytes(agent, &p) {
                    out.extend_from_slice(&bytes);
                } else {
                    let mut s = usv_utf8(agent, &p)?;
                    if native {
                        s = native_endings(&s);
                    }
                    out.extend_from_slice(s.as_bytes());
                }
            }
        }
        _ => {
            return agent.type_error(
                "Failed to construct 'Blob': The provided value cannot be converted to a sequence.",
            );
        }
    }
    Ok(out)
}

/// "Convert line endings to native" (File API §3.1.1).
fn native_endings(s: &str) -> String {
    let nl = if cfg!(windows) { "\r\n" } else { "\n" };
    s.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', nl)
}

fn blob_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Illegal constructor");
    };
    let (mime, native, _) = bag(agent, &a.arg(1))?;
    let bytes = process_parts(agent, &a.arg(0), native)?;
    let class = classes(agent).blob;
    let d = BlobData {
        bytes: Arc::new(bytes),
        mime,
        file: None,
    };
    Ok(Value::Object(agent.host_create_from_new_target(
        nt,
        class,
        None,
        Box::new(d),
    )?))
}

fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_millis() as f64)
}

fn file_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Illegal constructor");
    };
    if a.args.len() < 2 {
        return agent.type_error(&format!(
            "Failed to construct 'File': 2 arguments required, but only {} present.",
            a.args.len()
        ));
    }
    let parts = a.arg(0);
    let name = usv_utf8(agent, &a.arg(1))?;
    let (mime, native, last) = bag(agent, &a.arg(2))?;
    let bytes = process_parts(agent, &parts, native)?;
    let class = classes(agent).file;
    let d = BlobData {
        bytes: Arc::new(bytes),
        mime,
        file: Some((name, last.unwrap_or_else(now_ms))),
    };
    Ok(Value::Object(agent.host_create_from_new_target(
        nt,
        class,
        None,
        Box::new(d),
    )?))
}

fn this_blob(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<BlobData> {
    match data(agent, &a.this) {
        Some(d) => Ok(d),
        None => agent.type_error("Illegal invocation"),
    }
}

fn size(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(this_blob(agent, a)?.bytes.len() as f64))
}

fn blob_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&this_blob(agent, a)?.mime))
}

/// Un índice relativo de `slice()` (File API §3.3.1): negativo cuenta desde el final.
fn relative(agent: &mut Agent, v: &Value, size: usize, default: usize) -> JsResult<usize> {
    if v.is_undefined() {
        return Ok(default);
    }
    let n = agent.to_number(v)?;
    // `long long` (WebIDL): truncado.
    let n = if n.is_finite() {
        n.trunc()
    } else if n > 0.0 {
        f64::MAX
    } else {
        0.0
    };
    let size = size as f64;
    let r = if n < 0.0 {
        (size + n).max(0.0)
    } else {
        n.min(size)
    };
    Ok(r as usize)
}

fn slice(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_blob(agent, a)?;
    let len = d.bytes.len();
    let start = relative(agent, &a.arg(0), len, 0)?;
    let end = relative(agent, &a.arg(1), len, len)?;
    let mime = if a.arg(2).is_undefined() {
        String::new()
    } else {
        normalize_type(&agent.to_string(&a.arg(2))?.to_string())
    };
    let bytes = if start < end {
        d.bytes[start..end].to_vec()
    } else {
        Vec::new()
    };
    Ok(create(
        agent,
        BlobData {
            bytes: Arc::new(bytes),
            mime,
            file: None,
        },
    ))
}

fn text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_blob(agent, a)?;
    settled(agent, Ok(Value::str(&utf8_decode(&d.bytes))))
}

fn array_buffer(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_blob(agent, a)?;
    let b = new_array_buffer(agent, d.bytes.to_vec());
    settled(agent, Ok(Value::Object(b)))
}

fn bytes(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_blob(agent, a)?;
    let b = new_uint8_array(agent, d.bytes.to_vec());
    settled(agent, Ok(Value::Object(b)))
}

fn file_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_blob(agent, a)?;
    Ok(Value::str(d.file.as_ref().map_or("", |f| f.0.as_str())))
}

fn last_modified(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let d = this_blob(agent, a)?;
    Ok(Value::Number(d.file.as_ref().map_or(0.0, |f| f.1)))
}

fn relative_path(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_blob(agent, a)?;
    Ok(Value::str(""))
}

// --- URL.createObjectURL / revokeObjectURL (File API §8.2) ---

/// Agrega `createObjectURL` y `revokeObjectURL` al `URL` de `realm`.
pub fn install_object_urls(agent: &mut Agent, realm: RealmId) {
    let g = agent.realms[realm].global_object;
    let Ok(Value::Object(url)) = agent.get(g, &PropertyKey::from("URL"), Value::Object(g)) else {
        return;
    };
    bf_js::builtins::method(agent, url, "createObjectURL", 1, create_object_url);
    bf_js::builtins::method(agent, url, "revokeObjectURL", 1, revoke_object_url);
}

fn create_object_url(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(d) = data(agent, &a.arg(0)) else {
        return agent.type_error(
            "Failed to execute 'createObjectURL' on 'URL': Overload resolution failed.",
        );
    };
    let origin = super::document_origin(agent);
    Ok(Value::str(&bf_net::blob::register(
        &origin, d.bytes, &d.mime,
    )))
}

fn revoke_object_url(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = usv_utf8(agent, &a.arg(0))?;
    // Solo las del mismo origen (File API §8.2.2).
    if let Ok(u) = bf_infra::Url::parse(&s, None)
        && u.scheme == "blob"
        && s.starts_with(&format!("blob:{}/", super::document_origin(agent)))
    {
        bf_net::blob::revoke(&s);
    }
    Ok(Value::Undefined)
}

// --- FileReader (File API §6) ---

/// El estado de un `FileReader` (por id de target).
#[derive(Debug, Clone, Default)]
pub struct ReaderState {
    /// `EMPTY` 0, `LOADING` 1, `DONE` 2.
    pub ready: u8,
    /// Sube con cada lectura o `abort()` (para descartar las tareas viejas).
    pub generation: u64,
}

pub fn file_reader_class(event_target: HostClassId) -> HostClass {
    with_event_handlers(
        HostClass::new("FileReader")
            .parent(event_target)
            .constructor(reader_ctor, 0)
            .method("readAsArrayBuffer", 1, read_as_array_buffer)
            .method("readAsBinaryString", 1, read_as_binary_string)
            .method("readAsText", 1, read_as_text)
            .method("readAsDataURL", 1, read_as_data_url)
            .method("abort", 0, reader_abort)
            .constant("EMPTY", 0.0)
            .constant("LOADING", 1.0)
            .constant("DONE", 2.0)
            .attribute("readyState", Some(reader_ready_state), None)
            .attribute("result", Some(reader_result), None)
            .attribute("error", Some(reader_error), None),
        &["loadstart", "progress", "load", "abort", "error", "loadend"],
    )
}

fn reader_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Illegal constructor");
    };
    let class = classes(agent).file_reader;
    let (t, o) = new_target(agent, class, Some(nt))?;
    if let Some(s) = agent.host_slots_mut(o) {
        *s = vec![Value::Null, Value::Null];
    }
    state(agent).readers.insert(t.id(), ReaderState::default());
    Ok(Value::Object(o))
}

trait TargetId {
    fn id(&self) -> u64;
}

impl TargetId for TargetRef {
    fn id(&self) -> u64 {
        match self.target {
            EventTarget::Object(id) => id,
            _ => 0,
        }
    }
}

fn this_reader(
    agent: &mut Agent,
    a: &CallArgs<'_>,
) -> JsResult<(TargetRef, bf_js::heap::Gc<bf_js::object::Object>)> {
    match (agent.host_unwrap::<TargetRef>(&a.this).copied(), &a.this) {
        (Some(t), Value::Object(o))
            if agent.host_is_instance(&a.this, classes(agent).file_reader) =>
        {
            Ok((t, *o))
        }
        _ => agent.type_error("Illegal invocation"),
    }
}

fn reader_state(agent: &mut Agent, t: TargetRef) -> ReaderState {
    state(agent)
        .readers
        .get(&t.id())
        .cloned()
        .unwrap_or_default()
}

fn set_slot(agent: &mut Agent, o: bf_js::heap::Gc<bf_js::object::Object>, i: usize, v: Value) {
    if let Some(s) = agent.host_slots_mut(o) {
        while s.len() <= i {
            s.push(Value::Null);
        }
        s[i] = v;
    }
}

fn reader_ready_state(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (t, _) = this_reader(agent, a)?;
    Ok(Value::Number(f64::from(reader_state(agent, t).ready)))
}

fn reader_result(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (_, o) = this_reader(agent, a)?;
    Ok(agent
        .host_slots_mut(o)
        .and_then(|s| s.first().cloned())
        .unwrap_or(Value::Null))
}

fn reader_error(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (_, o) = this_reader(agent, a)?;
    Ok(agent
        .host_slots_mut(o)
        .and_then(|s| s.get(1).cloned())
        .unwrap_or(Value::Null))
}

#[derive(Debug, Clone)]
enum ReadAs {
    ArrayBuffer,
    BinaryString,
    Text(Option<String>),
    DataUrl,
}

/// La base64 de `bytes` (RFC 4648 §4).
fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// "Package data" de FileReader (File API §6.4).
fn reader_package(agent: &mut Agent, d: &BlobData, how: &ReadAs) -> Value {
    match how {
        ReadAs::ArrayBuffer => Value::Object(new_array_buffer(agent, d.bytes.to_vec())),
        ReadAs::BinaryString => {
            let s: String = d.bytes.iter().map(|b| char::from(*b)).collect();
            Value::str(&s)
        }
        ReadAs::Text(label) => {
            // BOM primero; luego la etiqueta, el charset del tipo y UTF-8.
            let b = &d.bytes;
            let s = if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
                utf8_decode(b)
            } else if b.starts_with(&[0xFE, 0xFF]) || b.starts_with(&[0xFF, 0xFE]) {
                bf_infra::encoding::decode(b, bf_infra::encoding::Encoding::Utf8).0
            } else {
                let charset = label
                    .clone()
                    .filter(|l| bf_infra::encoding::Encoding::for_label(l).is_some())
                    .or_else(|| super::body::mime_charset(&d.mime));
                decode_with(b, charset.as_deref())
            };
            Value::str(&s)
        }
        ReadAs::DataUrl => {
            let mime = if d.mime.is_empty() {
                "application/octet-stream"
            } else {
                d.mime.as_str()
            };
            Value::str(&format!("data:{mime};base64,{}", base64(&d.bytes)))
        }
    }
}

/// "Read operation" (File API §6.3).
fn read(agent: &mut Agent, a: &CallArgs<'_>, how: ReadAs) -> JsResult<Value> {
    let (t, o) = this_reader(agent, a)?;
    let Some(d) = data(agent, &a.arg(0)) else {
        return agent.type_error("The provided value is not of type 'Blob'.");
    };
    let st = reader_state(agent, t);
    if st.ready == 1 {
        return dom_error(
            agent,
            "InvalidStateError",
            "The object is already busy reading Blobs.",
        );
    }
    let generation = st.generation + 1;
    state(agent).readers.insert(
        t.id(),
        ReaderState {
            ready: 1,
            generation,
        },
    );
    set_slot(agent, o, 0, Value::Null);
    set_slot(agent, o, 1, Value::Null);
    let pin = agent.host_pin(Value::Object(o));
    let total = d.bytes.len() as f64;
    // loadstart en una tarea; el resultado, progress, load y loadend en la siguiente.
    agent.queue_task(
        TaskSource::Other,
        Box::new(move |agent: &mut Agent| {
            if reader_state(agent, t).generation != generation {
                return;
            }
            fire_progress(agent, t, "loadstart", false, 0.0, 0.0);
            agent.queue_task(
                TaskSource::Other,
                Box::new(move |agent: &mut Agent| {
                    let Some(Value::Object(o)) = agent.host_unpin(pin) else {
                        return;
                    };
                    if reader_state(agent, t).generation != generation {
                        return;
                    }
                    fire_progress(agent, t, "progress", true, total, total);
                    if reader_state(agent, t).generation != generation {
                        return;
                    }
                    let v = reader_package(agent, &d, &how);
                    set_slot(agent, o, 0, v);
                    if let Some(s) = state(agent).readers.get_mut(&t.id()) {
                        s.ready = 2;
                    }
                    fire_progress(agent, t, "load", true, total, total);
                    if reader_state(agent, t).ready != 1 {
                        fire_progress(agent, t, "loadend", true, total, total);
                    }
                }),
            );
        }),
    );
    Ok(Value::Undefined)
}

fn read_as_array_buffer(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    read(agent, a, ReadAs::ArrayBuffer)
}

fn read_as_binary_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    read(agent, a, ReadAs::BinaryString)
}

fn read_as_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let label = if a.arg(1).is_undefined() {
        None
    } else {
        Some(agent.to_string(&a.arg(1))?.to_string())
    };
    read(agent, a, ReadAs::Text(label))
}

fn read_as_data_url(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    read(agent, a, ReadAs::DataUrl)
}

/// `abort()` (File API §6.2.3).
fn reader_abort(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (t, o) = this_reader(agent, a)?;
    let st = reader_state(agent, t);
    if st.ready != 1 {
        set_slot(agent, o, 0, Value::Null);
        return Ok(Value::Undefined);
    }
    state(agent).readers.insert(
        t.id(),
        ReaderState {
            ready: 2,
            generation: st.generation + 1,
        },
    );
    set_slot(agent, o, 0, Value::Null);
    fire_progress(agent, t, "abort", false, 0.0, 0.0);
    if reader_state(agent, t).ready != 1 {
        fire_progress(agent, t, "loadend", false, 0.0, 0.0);
    }
    Ok(Value::Undefined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(base64(b"hola"), "aG9sYQ==");
        assert_eq!(base64(b""), "");
        assert_eq!(normalize_type("Text/Plain"), "text/plain");
        assert_eq!(normalize_type("a\u{1}b"), "");
    }
}
