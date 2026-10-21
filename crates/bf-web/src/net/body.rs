//! Los cuerpos (Fetch §5.1 "body mixin" y §5.2 "BodyInit"): extraerlos de un valor de script
//! y consumirlos (`text()`, `json()`, `arrayBuffer()`, `bytes()`, `blob()`, `formData()`).
//! También los tipos MIME (MIME Sniffing §4.4, simplificado).

use std::sync::Arc;

use bf_js::builtins::web::encoding::{buffer_source_bytes, new_array_buffer, new_uint8_array};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::{Agent, JsResult};

use super::blob::{self, BlobData};
use super::formdata::{self, Entry};

/// Un cuerpo: los bytes (ya completos) y si ya se leyó.
#[derive(Debug, Clone, Default)]
pub struct Body {
    /// `None` = cuerpo nulo.
    pub source: Option<Arc<Vec<u8>>>,
    pub used: bool,
}

impl Body {
    pub fn new(bytes: Option<Vec<u8>>) -> Body {
        Body {
            source: bytes.map(Arc::new),
            used: false,
        }
    }
}

/// "Parse a MIME type" (simplificado): `(essence, params)` en minúsculas, o `None`.
pub fn parse_mime(value: &str) -> Option<(String, Vec<(String, String)>)> {
    let mut parts = value.split(';');
    let essence = parts.next()?.trim().to_ascii_lowercase();
    let (t, st) = essence.split_once('/')?;
    let tok = |s: &str| super::headers::is_token(s);
    if !tok(t) || !tok(st) {
        return None;
    }
    let mut params = Vec::new();
    for p in parts {
        if let Some((k, v)) = p.split_once('=') {
            let k = k.trim().to_ascii_lowercase();
            let v = v.trim().trim_matches('"').to_owned();
            if !k.is_empty() && !params.iter().any(|(x, _)| *x == k) {
                params.push((k, v));
            }
        }
    }
    Some((essence, params))
}

/// La esencia (`type/subtype`) de un `Content-Type`.
pub fn mime_essence(value: &str) -> Option<String> {
    parse_mime(value).map(|(e, _)| e)
}

/// El `charset` de un `Content-Type`.
pub fn mime_charset(value: &str) -> Option<String> {
    parse_mime(value)?
        .1
        .into_iter()
        .find(|(k, _)| k == "charset")
        .map(|(_, v)| v)
}

/// "UTF-8 decode" (Encoding §6): sin BOM, con U+FFFD para lo inválido.
pub fn utf8_decode(bytes: &[u8]) -> String {
    let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8_lossy(b).into_owned()
}

/// Decodifica con la etiqueta `charset` (UTF-8 si falta o no se conoce).
pub fn decode_with(bytes: &[u8], charset: Option<&str>) -> String {
    match charset.and_then(bf_infra::encoding::Encoding::for_label) {
        Some(enc) if enc != bf_infra::encoding::Encoding::Utf8 => {
            let (s, _) = bf_infra::encoding::decode(bytes, enc);
            s
        }
        _ => utf8_decode(bytes),
    }
}

/// Un `String` de JavaScript como `USVString` en UTF-8 (los surrogates sueltos → U+FFFD).
pub fn usv_utf8(agent: &mut Agent, v: &Value) -> JsResult<String> {
    let s = agent.to_string(v)?;
    Ok(String::from_utf16_lossy(s.units()))
}

/// "Extract a body" (Fetch §5.2): los bytes y el `Content-Type` que implica `init`.
pub fn extract(agent: &mut Agent, init: &Value) -> JsResult<(Vec<u8>, Option<String>)> {
    if let Some(b) = blob::data(agent, init) {
        let t = (!b.mime.is_empty()).then(|| b.mime.clone());
        return Ok((b.bytes.to_vec(), t));
    }
    if let Some(bytes) = buffer_source_bytes(agent, init) {
        return Ok((bytes, None));
    }
    if let Some(fd) = formdata::entries_of(agent, init) {
        let boundary = boundary();
        let body = formdata::multipart(&fd, &boundary);
        return Ok((
            body,
            Some(format!("multipart/form-data; boundary={boundary}")),
        ));
    }
    if let Some(list) = search_params_list(agent, init) {
        let body = bf_url::form::serialize(&list);
        return Ok((
            body.into_bytes(),
            Some("application/x-www-form-urlencoded;charset=UTF-8".into()),
        ));
    }
    let s = usv_utf8(agent, init)?;
    Ok((s.into_bytes(), Some("text/plain;charset=UTF-8".into())))
}

/// La lista de un `URLSearchParams` (de los built-ins de `bf-js`).
pub fn search_params_list(agent: &Agent, v: &Value) -> Option<Vec<(String, String)>> {
    let Value::Object(o) = v else { return None };
    match &agent.heap[*o].kind {
        bf_js::object::ObjectKind::Web(w) => match &**w {
            bf_js::builtins::web::WebData::SearchParams(p) => Some(p.list.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Un boundary de `multipart/form-data` (HTML §4.10.21.8).
pub fn boundary() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut h = RandomState::new().build_hasher();
    h.write_u8(1);
    format!("----FreedomFormBoundary{:016x}", h.finish())
}

/// Lo que se lee de un cuerpo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsumeKind {
    Text,
    Json,
    ArrayBuffer,
    Bytes,
    Blob,
    FormData,
}

/// "Package data" (Fetch §5.1): el valor de script de `bytes` según `kind` (`mime` es el
/// `Content-Type`).
pub fn package(
    agent: &mut Agent,
    bytes: &[u8],
    kind: ConsumeKind,
    mime: Option<&str>,
) -> JsResult<Value> {
    match kind {
        ConsumeKind::Text => Ok(Value::str(&utf8_decode(bytes))),
        ConsumeKind::Json => parse_json(agent, &utf8_decode(bytes)),
        ConsumeKind::ArrayBuffer => Ok(Value::Object(new_array_buffer(agent, bytes.to_vec()))),
        ConsumeKind::Bytes => Ok(Value::Object(new_uint8_array(agent, bytes.to_vec()))),
        ConsumeKind::Blob => {
            let t = mime.map(str::to_ascii_lowercase).unwrap_or_default();
            Ok(blob::create(
                agent,
                BlobData {
                    bytes: Arc::new(bytes.to_vec()),
                    mime: t,
                    file: None,
                },
            ))
        }
        ConsumeKind::FormData => {
            let essence = mime.and_then(mime_essence);
            let entries = match essence.as_deref() {
                Some("multipart/form-data") => {
                    let b = mime
                        .and_then(|m| parse_mime(m))
                        .and_then(|(_, p)| p.into_iter().find(|(k, _)| k == "boundary"))
                        .map(|(_, v)| v);
                    match b.and_then(|b| formdata::parse_multipart(bytes, &b)) {
                        Some(e) => e,
                        None => return agent.type_error("Failed to fetch: invalid multipart body"),
                    }
                }
                Some("application/x-www-form-urlencoded") => bf_url::form::parse(bytes)
                    .into_iter()
                    .map(|(k, v)| (k, Entry::String(v)))
                    .collect(),
                _ => {
                    return agent.type_error("Could not parse content as FormData.");
                }
            };
            Ok(formdata::create(agent, entries))
        }
    }
}

/// `JSON.parse(text)` con el `JSON` intrínseco del realm.
pub fn parse_json(agent: &mut Agent, text: &str) -> JsResult<Value> {
    let realm = agent.current_realm;
    let g = agent.realms[realm].global_object;
    let json = agent.get(g, &PropertyKey::from("JSON"), Value::Object(g))?;
    let Value::Object(j) = json else {
        return agent.type_error("JSON no está disponible");
    };
    let parse = agent.get(j, &PropertyKey::from("parse"), json.clone())?;
    agent.call(&parse, json, &[Value::str(text)])
}

/// `JSON.stringify(v)` (para `Response.json()`); `None` si da `undefined`.
pub fn stringify_json(agent: &mut Agent, v: &Value) -> JsResult<Option<String>> {
    let realm = agent.current_realm;
    let g = agent.realms[realm].global_object;
    let json = agent.get(g, &PropertyKey::from("JSON"), Value::Object(g))?;
    let Value::Object(j) = json else {
        return agent.type_error("JSON no está disponible");
    };
    let f = agent.get(j, &PropertyKey::from("stringify"), json.clone())?;
    let r = agent.call(&f, json, &[v.clone()])?;
    if r.is_undefined() {
        return Ok(None);
    }
    Ok(Some(usv_utf8(agent, &r)?))
}

/// Una promesa ya resuelta con `v` (o rechazada con el error de `r`).
pub fn settled(agent: &mut Agent, r: JsResult<Value>) -> JsResult<Value> {
    let p = agent.new_internal_promise();
    match r {
        Ok(v) => agent.resolve_promise(p, v)?,
        Err(bf_js::vm::Throw::Value(e)) => agent.reject_promise(p, e),
        Err(t) => return Err(t),
    }
    Ok(Value::Object(p))
}

/// "Consume body" (Fetch §5.1): una promesa con el cuerpo leído; rechazada con TypeError si
/// ya se leyó.
pub fn consume(
    agent: &mut Agent,
    body: &mut Body,
    kind: ConsumeKind,
    mime: Option<&str>,
) -> JsResult<Value> {
    if body.used {
        let e = agent.type_error::<Value>("Body has already been consumed.");
        return settled(agent, e);
    }
    let bytes = body.source.clone().unwrap_or_default();
    if body.source.is_some() {
        body.used = true;
    }
    let r = package(agent, &bytes, kind, mime);
    settled(agent, r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime() {
        assert_eq!(
            parse_mime("Text/HTML; Charset=\"utf-8\"; a=b"),
            Some((
                "text/html".into(),
                vec![("charset".into(), "utf-8".into()), ("a".into(), "b".into())]
            ))
        );
        assert_eq!(mime_essence("nope"), None);
        assert_eq!(
            mime_charset("text/plain;charset=windows-1252").as_deref(),
            Some("windows-1252")
        );
        assert_eq!(utf8_decode(b"\xEF\xBB\xBFhi"), "hi");
    }
}
