//! `FormData` (XHR §5) y el "entry list" de un formulario (HTML §4.10.21.4), con sus
//! serializaciones `multipart/form-data` (HTML §4.10.21.8) y su lectura (Fetch §5.1).

use std::sync::Arc;

use bf_dom::{Document, NodeId};
use bf_js::host::HostClass;
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use super::blob::{self, BlobData};
use super::body::usv_utf8;
use super::{classes, values_iterator};
use crate::dom::unwrap_node;
use crate::with_doc;

/// El valor de una entrada: un string o un `File`.
#[derive(Debug, Clone)]
pub enum Entry {
    String(String),
    File(BlobData),
}

/// El payload de un `FormData`.
#[derive(Debug, Clone, Default)]
pub struct FormDataPayload(pub Vec<(String, Entry)>);

pub fn class() -> HostClass {
    HostClass::new("FormData")
        .constructor(constructor, 0)
        .method("append", 2, append)
        .method("delete", 1, delete)
        .method("get", 1, get)
        .method("getAll", 1, get_all)
        .method("has", 1, has)
        .method("set", 2, set)
        .method("forEach", 1, for_each)
        .method("keys", 0, keys)
        .method("values", 0, values)
        .method("entries", 0, entries)
}

/// Un `FormData` nuevo con `entries`.
pub fn create(agent: &mut Agent, entries: Vec<(String, Entry)>) -> Value {
    let class = classes(agent).form_data;
    Value::Object(agent.host_create(class, Box::new(FormDataPayload(entries))))
}

/// Las entradas de un `FormData` (para extraer un cuerpo).
pub fn entries_of(agent: &Agent, v: &Value) -> Option<Vec<(String, Entry)>> {
    agent.host_unwrap::<FormDataPayload>(v).map(|p| p.0.clone())
}

/// "Constructing the entry list" (HTML §4.10.21.4, sin `dirname` ni archivos elegidos).
pub fn form_entry_list(
    doc: &Document,
    form: NodeId,
    submitter: Option<NodeId>,
) -> Vec<(String, Entry)> {
    let mut out = Vec::new();
    let disabled_by_fieldset = |n: NodeId| {
        let mut cur = doc.parent(n);
        while let Some(p) = cur {
            if p == form {
                return false;
            }
            if doc
                .element(p)
                .is_some_and(|e| e.is_html("fieldset") && e.attr("disabled").is_some())
            {
                return true;
            }
            cur = doc.parent(p);
        }
        false
    };
    for n in doc.inclusive_descendants(form) {
        let Some(el) = doc.element(n) else { continue };
        if el.ns != bf_infra::Namespace::Html
            || el.attr("disabled").is_some()
            || disabled_by_fieldset(n)
        {
            continue;
        }
        let local = el.name.as_str().unwrap_or("").to_owned();
        if !matches!(local.as_str(), "input" | "button" | "select" | "textarea") {
            continue;
        }
        let ty = el.attr("type").unwrap_or("").to_ascii_lowercase();
        let is_button = local == "button"
            || (local == "input" && matches!(ty.as_str(), "submit" | "image" | "reset" | "button"));
        if is_button && submitter != Some(n) {
            continue;
        }
        if local == "input" && matches!(ty.as_str(), "checkbox" | "radio") && !doc.checkedness(n) {
            continue;
        }
        if local == "input" && ty == "image" {
            let name = el.attr("name").unwrap_or("");
            let prefix = if name.is_empty() {
                String::new()
            } else {
                format!("{name}.")
            };
            out.push((format!("{prefix}x"), Entry::String("0".into())));
            out.push((format!("{prefix}y"), Entry::String("0".into())));
            continue;
        }
        let Some(name) = el.attr("name").filter(|s| !s.is_empty()).map(str::to_owned) else {
            continue;
        };
        match local.as_str() {
            "select" => {
                for o in doc.list_of_options(n) {
                    if doc.option_selectedness(o) && !doc.option_disabled(o) {
                        let v = doc
                            .element(o)
                            .and_then(|e| e.attr("value").map(str::to_owned))
                            .unwrap_or_else(|| strip_collapse(&doc.text_content(o)));
                        out.push((name.clone(), Entry::String(v)));
                    }
                }
            }
            "input" if matches!(ty.as_str(), "checkbox" | "radio") => {
                out.push((
                    name,
                    Entry::String(el.attr("value").unwrap_or("on").to_owned()),
                ));
            }
            "input" if ty == "file" => {
                // Sin archivos elegidos: un File vacío llamado "" de tipo application/octet-stream.
                out.push((
                    name,
                    Entry::File(BlobData {
                        bytes: Arc::new(Vec::new()),
                        mime: "application/octet-stream".into(),
                        file: Some((String::new(), 0.0)),
                    }),
                ));
            }
            "textarea" => out.push((
                name,
                Entry::String(normalize_newlines(&doc.text_content(n))),
            )),
            _ => out.push((
                name,
                Entry::String(el.attr("value").unwrap_or("").to_owned()),
            )),
        }
    }
    out
}

/// "Strip and collapse ASCII whitespace".
fn strip_collapse(s: &str) -> String {
    s.split_ascii_whitespace().collect::<Vec<_>>().join(" ")
}

/// Los saltos de línea como CRLF (HTML §4.10.21.8, "convert to a list of name-value pairs").
fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r\n")
}

/// "Escape a name" para `multipart/form-data`.
fn escape(s: &str) -> String {
    s.replace('\n', "%0A")
        .replace('\r', "%0D")
        .replace('"', "%22")
}

/// "multipart/form-data encoding algorithm" (HTML §4.10.21.8).
pub fn multipart(entries: &[(String, Entry)], boundary: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, value) in entries {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        let name = escape(&normalize_newlines(name));
        match value {
            Entry::String(v) => {
                out.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
                );
                out.extend_from_slice(normalize_newlines(v).as_bytes());
            }
            Entry::File(f) => {
                let filename = escape(f.file.as_ref().map_or("blob", |x| x.0.as_str()));
                let mime = if f.mime.is_empty() {
                    "application/octet-stream"
                } else {
                    f.mime.as_str()
                };
                out.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: {mime}\r\n\r\n"
                    )
                    .as_bytes(),
                );
                out.extend_from_slice(&f.bytes);
            }
        }
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    out
}

/// Lee un cuerpo `multipart/form-data` (RFC 7578); `None` si está mal formado.
pub fn parse_multipart(body: &[u8], boundary: &str) -> Option<Vec<(String, Entry)>> {
    let delim = format!("--{boundary}").into_bytes();
    let find = |hay: &[u8], needle: &[u8], from: usize| -> Option<usize> {
        if needle.is_empty() || hay.len() < needle.len() {
            return None;
        }
        (from..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
    };
    let mut pos = find(body, &delim, 0)?;
    let mut out = Vec::new();
    loop {
        pos += delim.len();
        if body.get(pos..pos + 2) == Some(b"--") {
            return Some(out);
        }
        if body.get(pos..pos + 2) != Some(b"\r\n") {
            return None;
        }
        pos += 2;
        let head_end = find(body, b"\r\n\r\n", pos)?;
        let head = String::from_utf8_lossy(&body[pos..head_end]).into_owned();
        let content_start = head_end + 4;
        let mut next_delim = b"\r\n".to_vec();
        next_delim.extend_from_slice(&delim);
        let content_end = find(body, &next_delim, content_start)?;
        let content = &body[content_start..content_end];
        let mut name = None;
        let mut filename = None;
        let mut ctype = None;
        for line in head.split("\r\n") {
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            match k.trim().to_ascii_lowercase().as_str() {
                "content-disposition" => {
                    for part in v.split(';').skip(1) {
                        if let Some((pk, pv)) = part.split_once('=') {
                            let pv = pv.trim().trim_matches('"').to_owned();
                            match pk.trim().to_ascii_lowercase().as_str() {
                                "name" => name = Some(pv),
                                "filename" => filename = Some(pv),
                                _ => {}
                            }
                        }
                    }
                }
                "content-type" => ctype = Some(v.trim().to_owned()),
                _ => {}
            }
        }
        let name = name?;
        let entry = match filename {
            Some(f) => Entry::File(BlobData {
                bytes: Arc::new(content.to_vec()),
                mime: ctype
                    .unwrap_or_else(|| "text/plain".into())
                    .to_ascii_lowercase(),
                file: Some((f, 0.0)),
            }),
            None => Entry::String(super::body::utf8_decode(content)),
        };
        out.push((name, entry));
        pos = content_end + 2;
    }
}

// --- La interfaz ---

fn this_entries<'a>(
    agent: &'a mut Agent,
    a: &CallArgs<'_>,
) -> JsResult<&'a mut Vec<(String, Entry)>> {
    if agent.host_unwrap::<FormDataPayload>(&a.this).is_none() {
        return agent.type_error("Illegal invocation");
    }
    Ok(&mut agent
        .host_unwrap_mut::<FormDataPayload>(&a.this)
        .expect("FormData")
        .0)
}

/// `new FormData(form, submitter)` (XHR §5.1).
fn constructor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Illegal constructor");
    };
    let mut entries = Vec::new();
    let form = a.arg(0);
    if !form.is_undefined() {
        let n = unwrap_node(agent, &form);
        let is_form = n.is_some_and(|n| {
            with_doc(agent, n.doc, |d| {
                d.element(n.node).is_some_and(|e| e.is_html("form"))
            })
        });
        let Some(n) = n.filter(|_| is_form) else {
            return agent.type_error(
                "Failed to construct 'FormData': parameter 1 is not of type 'HTMLFormElement'.",
            );
        };
        let submitter = match a.arg(1) {
            Value::Undefined | Value::Null => None,
            v => {
                let s = unwrap_node(agent, &v);
                let ok = s.is_some_and(|s| {
                    with_doc(agent, s.doc, |d| {
                        d.element(s.node).is_some_and(|e| {
                            e.is_html("button")
                                || (e.is_html("input")
                                    && matches!(
                                        e.attr("type").unwrap_or("").to_ascii_lowercase().as_str(),
                                        "submit" | "image"
                                    ))
                        }) && d.inclusive_ancestors(s.node).contains(&n.node)
                    })
                });
                if !ok {
                    return crate::net::dom_error(
                        agent,
                        "NotFoundError",
                        "The specified element is not a submit button of this form.",
                    );
                }
                s.map(|s| s.node)
            }
        };
        entries = with_doc(agent, n.doc, |d| form_entry_list(d, n.node, submitter));
    }
    let class = classes(agent).form_data;
    Ok(Value::Object(agent.host_create_from_new_target(
        nt,
        class,
        None,
        Box::new(FormDataPayload(entries)),
    )?))
}

/// "Create an entry" (XHR §5): un string, o un `File` (con `filename` o el nombre del `File`,
/// o `"blob"` para un `Blob`).
fn create_entry(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<(String, Entry)> {
    let name = usv_utf8(agent, &a.arg(0))?;
    let v = a.arg(1);
    if let Some(mut b) = blob::data(agent, &v) {
        let filename = if a.args.len() > 2 {
            Some(usv_utf8(agent, &a.arg(2))?)
        } else {
            None
        };
        let fname = filename.unwrap_or_else(|| {
            b.file
                .as_ref()
                .map_or_else(|| "blob".into(), |f| f.0.clone())
        });
        let last = b.file.as_ref().map_or_else(
            || {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0.0, |d| d.as_millis() as f64)
            },
            |f| f.1,
        );
        b.file = Some((fname, last));
        return Ok((name, Entry::File(b)));
    }
    if a.args.len() > 2 {
        return agent
            .type_error("Failed to execute on 'FormData': parameter 2 is not of type 'Blob'.");
    }
    Ok((name, Entry::String(usv_utf8(agent, &v)?)))
}

fn entry_value(agent: &mut Agent, e: &Entry) -> Value {
    match e {
        Entry::String(s) => Value::str(s),
        Entry::File(f) => blob::create(agent, f.clone()),
    }
}

fn append(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let e = create_entry(agent, a)?;
    this_entries(agent, a)?.push(e);
    Ok(Value::Undefined)
}

fn delete(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = usv_utf8(agent, &a.arg(0))?;
    this_entries(agent, a)?.retain(|(n, _)| *n != name);
    Ok(Value::Undefined)
}

fn get(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = usv_utf8(agent, &a.arg(0))?;
    let e = this_entries(agent, a)?
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, e)| e.clone());
    Ok(e.map_or(Value::Null, |e| entry_value(agent, &e)))
}

fn get_all(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = usv_utf8(agent, &a.arg(0))?;
    let es: Vec<Entry> = this_entries(agent, a)?
        .iter()
        .filter(|(n, _)| *n == name)
        .map(|(_, e)| e.clone())
        .collect();
    let vals: Vec<Value> = es.iter().map(|e| entry_value(agent, e)).collect();
    Ok(Value::Object(agent.create_array_from_list(&vals)?))
}

fn has(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = usv_utf8(agent, &a.arg(0))?;
    let r = this_entries(agent, a)?.iter().any(|(n, _)| *n == name);
    Ok(Value::Boolean(r))
}

fn set(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (name, e) = create_entry(agent, a)?;
    let list = this_entries(agent, a)?;
    match list.iter().position(|(n, _)| *n == name) {
        Some(i) => {
            list[i].1 = e;
            let mut k = 0;
            list.retain(|(n, _)| {
                let keep = k <= i || *n != name;
                k += 1;
                keep
            });
        }
        None => list.push((name, e)),
    }
    Ok(Value::Undefined)
}

fn for_each(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let cb = a.arg(0);
    if !agent.is_callable(&cb) {
        return agent.type_error("FormData.forEach: callback is not a function");
    }
    let mut i = 0;
    loop {
        let Some((n, e)) = this_entries(agent, a)?.get(i).cloned() else {
            break;
        };
        let v = entry_value(agent, &e);
        agent.call(&cb, a.arg(1), &[v, Value::str(&n), a.this.clone()])?;
        i += 1;
    }
    Ok(Value::Undefined)
}

fn iter(agent: &mut Agent, a: &CallArgs<'_>, kind: u8) -> JsResult<Value> {
    let list = this_entries(agent, a)?.clone();
    let mut vals = Vec::new();
    for (n, e) in list {
        let v = entry_value(agent, &e);
        vals.push(match kind {
            0 => Value::str(&n),
            1 => v,
            _ => Value::Object(agent.create_array_from_list(&[Value::str(&n), v])?),
        });
    }
    values_iterator(agent, vals)
}

fn keys(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    iter(agent, a, 0)
}

fn values(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    iter(agent, a, 1)
}

pub fn entries(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    iter(agent, a, 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipart_roundtrip() {
        let entries = vec![
            ("a\"b".to_owned(), Entry::String("x\ny".into())),
            (
                "f".to_owned(),
                Entry::File(BlobData {
                    bytes: Arc::new(b"data".to_vec()),
                    mime: "text/plain".into(),
                    file: Some(("n.txt".into(), 0.0)),
                }),
            ),
        ];
        let body = multipart(&entries, "BND");
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("name=\"a%22b\"\r\n\r\nx\r\ny\r\n"));
        assert!(text.contains("filename=\"n.txt\"\r\nContent-Type: text/plain\r\n\r\ndata\r\n"));
        assert!(text.ends_with("--BND--\r\n"));
        let back = parse_multipart(&body, "BND").expect("multipart");
        assert_eq!(back.len(), 2);
        assert!(matches!(&back[0].1, Entry::String(s) if s == "x\r\ny"));
        assert!(matches!(&back[1].1, Entry::File(f) if f.bytes.as_slice() == b"data"));
    }
}
