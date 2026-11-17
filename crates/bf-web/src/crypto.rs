//! E1: `crypto.subtle` (Web Cryptography API, W3C, "Web Cryptography Level 2"): `SubtleCrypto`
//! y `CryptoKey`, con `digest` (SHA-1, SHA-256, SHA-384 y SHA-512, §30) y HMAC (§29:
//! `importKey`/`exportKey` en `raw` y `jwk`, `generateKey`, `sign` y `verify`).
//!
//! - **Primitivas:** las de `bf-net`, en código propio y sin `unsafe`: SHA-1
//!   (`websocket::sha1`, el saludo de WebSocket), SHA-256 (`auth::sha256`, Digest de HTTP) y
//!   SHA-384/512 (`csp::sha2`, las fuentes de hash de CSP). Aquí solo está HMAC (RFC 2104).
//!   Los vectores oficiales (FIPS 180-4, RFC 2202 y RFC 4231) están en los tests.
//! - **Lo demás** (AES, PBKDF2, HKDF, RSA, ECDSA, ECDH, Ed25519…) no está registrado: el paso
//!   "normalize an algorithm" (§18.4.4) no encuentra el algoritmo y la operación rechaza con
//!   `NotSupportedError`, como pide el estándar.
//! - **Asíncrono:** cada operación devuelve una promesa. El trabajo (unos microsegundos) se hace
//!   antes de devolverla y la promesa se resuelve o rechaza ya: el resultado llega igual en un
//!   job, pero no en una tarea de la *crypto task source* como pide §14.3 ("in parallel" y
//!   "queue a global task").
//! - **`[SecureContext]`** (§14.1): `crypto.subtle` es `undefined` fuera de un contexto seguro
//!   (Secure Contexts §3.1, "potentially trustworthy"). Se decide al leerlo, porque un marco
//!   navega sin cambiar de realm.

use bf_js::host::{HostClass, HostClassId, HostHandle};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, RealmId, Throw};

use crate::WebContextHandle;

// --- Primitivas ---

/// Una función hash de §30.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hash {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

impl Hash {
    /// El nombre registrado (§18.4.4: la comparación es sin distinguir mayúsculas).
    pub fn name(self) -> &'static str {
        match self {
            Hash::Sha1 => "SHA-1",
            Hash::Sha256 => "SHA-256",
            Hash::Sha384 => "SHA-384",
            Hash::Sha512 => "SHA-512",
        }
    }

    fn from_name(name: &str) -> Option<Hash> {
        [Hash::Sha1, Hash::Sha256, Hash::Sha384, Hash::Sha512]
            .into_iter()
            .find(|h| h.name().eq_ignore_ascii_case(name))
    }

    /// El tamaño de bloque en bytes (FIPS 180-4 §1): el de la clave HMAC por omisión.
    fn block_len(self) -> usize {
        match self {
            Hash::Sha1 | Hash::Sha256 => 64,
            Hash::Sha384 | Hash::Sha512 => 128,
        }
    }

    /// El `alg` de una JWK HMAC (RFC 7518 §3.2).
    fn jwk_alg(self) -> &'static str {
        match self {
            Hash::Sha1 => "HS1",
            Hash::Sha256 => "HS256",
            Hash::Sha384 => "HS384",
            Hash::Sha512 => "HS512",
        }
    }
}

/// El digest de `data` (FIPS 180-4).
pub fn digest(h: Hash, data: &[u8]) -> Vec<u8> {
    match h {
        Hash::Sha1 => bf_net::websocket::sha1(data).to_vec(),
        Hash::Sha256 => bf_net::auth::sha256(data).to_vec(),
        Hash::Sha384 => bf_net::csp::sha2(384, data),
        Hash::Sha512 => bf_net::csp::sha2(512, data),
    }
}

/// HMAC (RFC 2104; FIPS 198-1): `H((K' ^ opad) || H((K' ^ ipad) || m))`.
pub fn hmac(h: Hash, key: &[u8], data: &[u8]) -> Vec<u8> {
    let block = h.block_len();
    let mut k = if key.len() > block {
        digest(h, key)
    } else {
        key.to_vec()
    };
    k.resize(block, 0);
    let mut inner: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    inner.extend_from_slice(data);
    let inner = digest(h, &inner);
    let mut outer: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    outer.extend_from_slice(&inner);
    digest(h, &outer)
}

/// Comparación en tiempo constante (para `verify`).
fn equal(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

const B64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// base64url sin relleno (RFC 4648 §5; JWK, RFC 7515 §2).
fn base64url_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        let chars = c.len() + 1;
        for i in 0..chars {
            out.push(B64URL[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

/// base64url sin relleno a bytes (`None` si no es válido).
fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut bits = 0u32;
    let mut n = 0u32;
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for ch in s.bytes() {
        let v = B64URL.iter().position(|&c| c == ch)? as u32;
        bits = (bits << 6) | v;
        n += 6;
        if n >= 8 {
            n -= 8;
            out.push((bits >> n) as u8);
            bits &= (1 << n) - 1;
        }
    }
    // Un carácter suelto (6 bits) no forma un byte.
    (n < 6).then_some(out)
}

// --- Clases ---

/// Las clases de la API (una vez por agente).
#[derive(Debug, Clone, Copy)]
struct CryptoClasses {
    subtle: HostClassId,
    key: HostClassId,
}

/// El `SubtleCrypto` de cada realm (el mismo objeto cada vez).
#[derive(Debug, Default)]
struct SubtleObjects(std::collections::HashMap<RealmId, HostHandle>);

/// `[[usages]]`, en el orden del enum `KeyUsage` (§13.3).
const USAGES: [&str; 8] = [
    "encrypt",
    "decrypt",
    "sign",
    "verify",
    "deriveKey",
    "deriveBits",
    "wrapKey",
    "unwrapKey",
];

/// Lo que guarda un `CryptoKey` (§13): por ahora solo claves HMAC (`[[type]]` "secret").
#[derive(Debug, Clone)]
struct KeyData {
    hash: Hash,
    /// La longitud en bits (`HmacKeyAlgorithm.length`).
    length: u32,
    bytes: Vec<u8>,
    extractable: bool,
    usages: Vec<&'static str>,
}

/// Registra las clases (la primera vez) y añade `subtle` al `Crypto` de `realm`.
pub fn register(agent: &mut Agent, realm: RealmId, _ctx: &WebContextHandle) {
    let classes = match agent.embedder_data::<CryptoClasses>() {
        Some(c) => *c,
        None => {
            let subtle = agent.register_host_class(
                HostClass::new("SubtleCrypto")
                    .method("digest", 2, subtle_digest)
                    .method("importKey", 5, subtle_import_key)
                    .method("exportKey", 2, subtle_export_key)
                    .method("generateKey", 3, subtle_generate_key)
                    .method("sign", 3, subtle_sign)
                    .method("verify", 4, subtle_verify)
                    .method("encrypt", 3, subtle_unsupported)
                    .method("decrypt", 3, subtle_unsupported)
                    .method("deriveBits", 3, subtle_unsupported)
                    .method("deriveKey", 5, subtle_unsupported)
                    .method("wrapKey", 4, subtle_unsupported)
                    .method("unwrapKey", 7, subtle_unsupported),
            );
            let key = agent.register_host_class(
                HostClass::new("CryptoKey")
                    .attribute("type", Some(key_type), None)
                    .attribute("extractable", Some(key_extractable), None)
                    .attribute("algorithm", Some(key_algorithm), None)
                    .attribute("usages", Some(key_usages), None),
            );
            let c = CryptoClasses { subtle, key };
            agent.set_embedder_data(c);
            c
        }
    };
    let _ = classes;
    // `Crypto.prototype.subtle` (§10.1): el `Crypto` lo crea `bf-js` en cada realm.
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let g = agent.realms[realm].global_object;
    if let Some(Value::Object(c)) = agent.realms[realm]
        .globals
        .iter()
        .find(|(n, _)| *n == "crypto")
        .map(|(_, o)| Value::Object(*o))
        && let Some(proto) = agent.heap[c].proto
    {
        crate::bom::accessor(agent, proto, "subtle", get_subtle, None);
    }
    let _ = g;
    agent.current_realm = saved;
}

/// `crypto.subtle`: el `SubtleCrypto` del realm, o `undefined` fuera de un contexto seguro
/// (`[SecureContext]`, §14.1).
fn get_subtle(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let realm = agent.current_realm;
    let Some(ctx) = WebContextHandle { realm }.try_get(agent) else {
        return Ok(Value::Undefined);
    };
    let url = &ctx.url;
    let secure =
        url.scheme == "about" || crate::workers::isolation::is_potentially_trustworthy(url);
    if !secure {
        return Ok(Value::Undefined);
    }
    if let Some(h) = agent
        .embedder_data::<SubtleObjects>()
        .and_then(|m| m.0.get(&realm).copied())
        && let Some(v) = agent.host_pinned(h)
    {
        return Ok(v);
    }
    let Some(classes) = agent.embedder_data::<CryptoClasses>().copied() else {
        return Ok(Value::Undefined);
    };
    let o = Value::Object(agent.host_create(classes.subtle, Box::new(())));
    let h = agent.host_pin(o.clone());
    if agent.embedder_data::<SubtleObjects>().is_none() {
        agent.set_embedder_data(SubtleObjects::default());
    }
    if let Some(m) = agent.embedder_data_mut::<SubtleObjects>() {
        m.0.insert(realm, h);
    }
    Ok(o)
}

// --- Promesas y errores ---

/// La promesa de una operación: `f` hace el trabajo; su error (un `TypeError` de una
/// conversión WebIDL o una `DOMException` de §14.3) rechaza la promesa en vez de lanzarse.
fn promise(agent: &mut Agent, f: impl FnOnce(&mut Agent) -> JsResult<Value>) -> JsResult<Value> {
    let ctor = agent.intrinsics().promise_ctor;
    let cap = agent.new_promise_capability_hidden(ctor)?;
    match f(agent) {
        Ok(v) => agent.capability_resolve(&cap, v)?,
        Err(Throw::Value(e)) => agent.capability_reject(&cap, e)?,
        Err(t) => return Err(t),
    }
    Ok(cap.promise())
}

fn dom_error<T>(agent: &mut Agent, name: &str, message: &str) -> JsResult<T> {
    crate::net::dom_error(agent, name, message)
}

fn not_supported<T>(agent: &mut Agent, what: &str) -> JsResult<T> {
    dom_error(
        agent,
        "NotSupportedError",
        &format!("Algorithm: Unrecognized name ({what})."),
    )
}

/// Los bytes de un `BufferSource` (`TypeError` si no es uno).
fn bytes(agent: &mut Agent, v: &Value, what: &str) -> JsResult<Vec<u8>> {
    match bf_js::builtins::web::encoding::buffer_source_bytes(agent, v) {
        Some(b) => Ok(b),
        None => agent.type_error(&format!(
            "Failed to execute on 'SubtleCrypto': {what} is not of type 'BufferSource'."
        )),
    }
}

fn array_buffer(agent: &mut Agent, data: Vec<u8>) -> Value {
    Value::Object(bf_js::builtins::web::encoding::new_array_buffer(
        agent, data,
    ))
}

// --- "Normalize an algorithm" (§18.4.4) ---

/// El nombre de un `AlgorithmIdentifier` (una cadena o un objeto con `name`).
fn algorithm_name(agent: &mut Agent, alg: &Value) -> JsResult<String> {
    match alg {
        Value::Object(o) => {
            let n = agent.get(*o, &PropertyKey::from("name"), alg.clone())?;
            if matches!(n, Value::Undefined) {
                return agent.type_error(
                    "Failed to execute on 'SubtleCrypto': required member name is undefined.",
                );
            }
            Ok(agent.to_string(&n)?.to_string())
        }
        v => Ok(agent.to_string(v)?.to_string()),
    }
}

/// Un miembro de un diccionario de parámetros (`undefined` si falta).
fn member(agent: &mut Agent, alg: &Value, name: &str) -> JsResult<Value> {
    match alg {
        Value::Object(o) => agent.get(*o, &PropertyKey::from(name), alg.clone()),
        _ => Ok(Value::Undefined),
    }
}

/// El hash de §30 para la operación "digest" (o el miembro `hash` de los parámetros HMAC).
fn normalize_hash(agent: &mut Agent, alg: &Value) -> JsResult<Hash> {
    let name = algorithm_name(agent, alg)?;
    match Hash::from_name(&name) {
        Some(h) => Ok(h),
        None => not_supported(agent, &name),
    }
}

/// HMAC para `importKey`, `generateKey`, `sign` y `verify` (el único algoritmo registrado
/// para ellas); los demás nombres, `NotSupportedError`.
fn normalize_hmac(agent: &mut Agent, alg: &Value) -> JsResult<()> {
    let name = algorithm_name(agent, alg)?;
    if name.eq_ignore_ascii_case("HMAC") {
        Ok(())
    } else {
        not_supported(agent, &name)
    }
}

/// El miembro `hash` (obligatorio) de `HmacImportParams`/`HmacKeyGenParams` (§29.2, §29.4).
fn hmac_hash(agent: &mut Agent, alg: &Value) -> JsResult<Hash> {
    let h = member(agent, alg, "hash")?;
    if matches!(h, Value::Undefined) {
        return agent
            .type_error("Failed to execute on 'SubtleCrypto': required member hash is undefined.");
    }
    normalize_hash(agent, &h)
}

/// El miembro opcional `length` (`[EnforceRange] unsigned long`).
fn hmac_length(agent: &mut Agent, alg: &Value) -> JsResult<Option<u32>> {
    let l = member(agent, alg, "length")?;
    if matches!(l, Value::Undefined) {
        return Ok(None);
    }
    let n = agent.to_number(&l)?;
    if !n.is_finite() || !(0.0..=f64::from(u32::MAX)).contains(&n.trunc()) {
        return agent.type_error("Failed to execute on 'SubtleCrypto': length is out of range.");
    }
    Ok(Some(n.trunc() as u32))
}

/// Un `sequence<KeyUsage>` (`TypeError` con un valor que no es del enum), sin repetidos y en
/// el orden del enum (el "normalized value" de §14.3).
fn usages(agent: &mut Agent, v: &Value) -> JsResult<Vec<&'static str>> {
    let list = agent.iterable_to_list(v)?;
    let mut set = Vec::new();
    for u in list {
        let s = agent.to_string(&u)?.to_string();
        let Some(&known) = USAGES.iter().find(|k| **k == s) else {
            return agent.type_error(&format!(
                "Failed to execute on 'SubtleCrypto': '{s}' is not a valid enum value of type KeyUsage."
            ));
        };
        if !set.contains(&known) {
            set.push(known);
        }
    }
    Ok(USAGES.into_iter().filter(|u| set.contains(u)).collect())
}

fn key_data(agent: &Agent, v: &Value) -> Option<KeyData> {
    agent.host_unwrap::<KeyData>(v).cloned()
}

fn this_key(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<KeyData> {
    match key_data(agent, &a.this) {
        Some(k) => Ok(k),
        None => agent.type_error("Illegal invocation"),
    }
}

/// El `CryptoKey` de un argumento (`TypeError` si no es uno).
fn arg_key(agent: &mut Agent, v: &Value) -> JsResult<KeyData> {
    match key_data(agent, v) {
        Some(k) => Ok(k),
        None => agent.type_error(
            "Failed to execute on 'SubtleCrypto': parameter is not of type 'CryptoKey'.",
        ),
    }
}

/// Crea un `CryptoKey` HMAC.
fn new_key(agent: &mut Agent, k: KeyData) -> Value {
    let Some(classes) = agent.embedder_data::<CryptoClasses>().copied() else {
        return Value::Undefined;
    };
    Value::Object(agent.host_create(classes.key, Box::new(k)))
}

// --- SubtleCrypto ---

/// `digest(algorithm, data)` (§14.3.5; SHA, §30).
fn subtle_digest(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (alg, data) = (a.arg(0), a.arg(1));
    promise(agent, |agent| {
        let h = normalize_hash(agent, &alg)?;
        let data = bytes(agent, &data, "data")?;
        let out = digest(h, &data);
        Ok(array_buffer(agent, out))
    })
}

/// `importKey(format, keyData, algorithm, extractable, keyUsages)` (§14.3.9; HMAC, §29.6).
fn subtle_import_key(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (format, data, alg, extractable, uses) = (a.arg(0), a.arg(1), a.arg(2), a.arg(3), a.arg(4));
    promise(agent, |agent| {
        let format = agent.to_string(&format)?.to_string();
        if !matches!(format.as_str(), "raw" | "spki" | "pkcs8" | "jwk") {
            return agent.type_error(&format!(
                "Failed to execute 'importKey' on 'SubtleCrypto': '{format}' is not a valid enum value of type KeyFormat."
            ));
        }
        let extractable = bf_js::abstract_ops::conversion::to_boolean(agent, &extractable);
        let usages = usages(agent, &uses)?;
        normalize_hmac(agent, &alg)?;
        let hash = hmac_hash(agent, &alg)?;
        let length = hmac_length(agent, &alg)?;
        if usages.iter().any(|u| !matches!(*u, "sign" | "verify")) {
            return dom_error(
                agent,
                "SyntaxError",
                "Cannot create a key using the specified key usages.",
            );
        }
        let key = match format.as_str() {
            "raw" => bytes(agent, &data, "keyData")?,
            "jwk" => jwk_key(agent, &data, hash, extractable, &usages)?,
            _ => return not_supported(agent, &format!("HMAC {format}")),
        };
        let bits = key.len() as u32 * 8;
        if bits == 0 {
            return dom_error(agent, "DataError", "HMAC key data must not be empty.");
        }
        let length = match length {
            Some(l) if l > bits || l <= bits - 8 => {
                return dom_error(
                    agent,
                    "DataError",
                    "The key length does not match the key data.",
                );
            }
            Some(l) => l,
            None => bits,
        };
        if usages.is_empty() {
            return dom_error(
                agent,
                "SyntaxError",
                "Usages cannot be empty when creating a key.",
            );
        }
        Ok(new_key(
            agent,
            KeyData {
                hash,
                length,
                bytes: key,
                extractable,
                usages,
            },
        ))
    })
}

/// La clave de una JWK HMAC (§29.6, "If format is "jwk"").
fn jwk_key(
    agent: &mut Agent,
    jwk: &Value,
    hash: Hash,
    extractable: bool,
    usages: &[&str],
) -> JsResult<Vec<u8>> {
    if !matches!(jwk, Value::Object(_)) {
        return agent.type_error(
            "Failed to execute 'importKey' on 'SubtleCrypto': keyData is not a JsonWebKey.",
        );
    }
    let field = |agent: &mut Agent, name: &str| -> JsResult<Option<String>> {
        let v = member(agent, jwk, name)?;
        if matches!(v, Value::Undefined) {
            return Ok(None);
        }
        Ok(Some(agent.to_string(&v)?.to_string()))
    };
    if field(agent, "kty")?.as_deref() != Some("oct") {
        return dom_error(
            agent,
            "DataError",
            "The JWK \"kty\" member was not \"oct\".",
        );
    }
    let Some(k) = field(agent, "k")? else {
        return dom_error(agent, "DataError", "The JWK \"k\" member was missing.");
    };
    let Some(key) = base64url_decode(&k) else {
        return dom_error(
            agent,
            "DataError",
            "The JWK \"k\" member is not valid base64url.",
        );
    };
    if let Some(alg) = field(agent, "alg")?
        && alg != hash.jwk_alg()
    {
        return dom_error(
            agent,
            "DataError",
            "The JWK \"alg\" member does not match the hash.",
        );
    }
    if !usages.is_empty()
        && let Some(u) = field(agent, "use")?
        && u != "sig"
    {
        return dom_error(
            agent,
            "DataError",
            "The JWK \"use\" member was not \"sig\".",
        );
    }
    let ops = member(agent, jwk, "key_ops")?;
    if !matches!(ops, Value::Undefined) {
        let list = agent.iterable_to_list(&ops)?;
        let mut have = Vec::new();
        for o in list {
            have.push(agent.to_string(&o)?.to_string());
        }
        if usages.iter().any(|u| !have.iter().any(|h| h == u)) {
            return dom_error(
                agent,
                "DataError",
                "The JWK \"key_ops\" member does not include the requested usages.",
            );
        }
    }
    let ext = member(agent, jwk, "ext")?;
    if matches!(ext, Value::Boolean(false)) && extractable {
        return dom_error(
            agent,
            "DataError",
            "The JWK \"ext\" member was false but extractable was requested.",
        );
    }
    Ok(key)
}

/// `exportKey(format, key)` (§14.3.10; HMAC, §29.7).
fn subtle_export_key(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (format, key) = (a.arg(0), a.arg(1));
    promise(agent, |agent| {
        let format = agent.to_string(&format)?.to_string();
        if !matches!(format.as_str(), "raw" | "spki" | "pkcs8" | "jwk") {
            return agent.type_error(&format!(
                "Failed to execute 'exportKey' on 'SubtleCrypto': '{format}' is not a valid enum value of type KeyFormat."
            ));
        }
        let k = arg_key(agent, &key)?;
        if !k.extractable {
            return dom_error(agent, "InvalidAccessError", "The key is not extractable.");
        }
        match format.as_str() {
            "raw" => Ok(array_buffer(agent, k.bytes)),
            "jwk" => {
                let o = agent.new_plain_object();
                let ops: Vec<Value> = k.usages.iter().map(|u| Value::str(u)).collect();
                let ops = Value::Object(agent.create_array_from_list(&ops)?);
                let entries = [
                    ("kty", Value::str("oct")),
                    ("k", Value::str(&base64url_encode(&k.bytes))),
                    ("alg", Value::str(k.hash.jwk_alg())),
                    ("key_ops", ops),
                    ("ext", Value::Boolean(k.extractable)),
                ];
                for (name, v) in entries {
                    agent.create_data_property_or_throw(o, PropertyKey::from(name), v)?;
                }
                Ok(Value::Object(o))
            }
            _ => not_supported(agent, &format!("HMAC {format}")),
        }
    })
}

/// `generateKey(algorithm, extractable, keyUsages)` (§14.3.6; HMAC, §29.5).
fn subtle_generate_key(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (alg, extractable, uses) = (a.arg(0), a.arg(1), a.arg(2));
    promise(agent, |agent| {
        let extractable = bf_js::abstract_ops::conversion::to_boolean(agent, &extractable);
        let usages = usages(agent, &uses)?;
        normalize_hmac(agent, &alg)?;
        let hash = hmac_hash(agent, &alg)?;
        let length = hmac_length(agent, &alg)?;
        if usages.iter().any(|u| !matches!(*u, "sign" | "verify")) {
            return dom_error(
                agent,
                "SyntaxError",
                "Cannot create a key using the specified key usages.",
            );
        }
        let length = match length {
            Some(0) => {
                return dom_error(agent, "OperationError", "HMAC key length must not be zero.");
            }
            Some(l) => l,
            None => hash.block_len() as u32 * 8,
        };
        if usages.is_empty() {
            return dom_error(
                agent,
                "SyntaxError",
                "Usages cannot be empty when creating a key.",
            );
        }
        let mut key = vec![0u8; length.div_ceil(8) as usize];
        bf_js::builtins::web::crypto::random_bytes(&mut key);
        Ok(new_key(
            agent,
            KeyData {
                hash,
                length,
                bytes: key,
                extractable,
                usages,
            },
        ))
    })
}

/// Los pasos comunes de `sign` y `verify` (§14.3.3, §14.3.4): el algoritmo tiene que ser el de
/// la clave y la clave tiene que admitir el uso.
fn key_for(agent: &mut Agent, alg: &Value, key: &Value, usage: &str) -> JsResult<KeyData> {
    normalize_hmac(agent, alg)?;
    let k = arg_key(agent, key)?;
    if !k.usages.contains(&usage) {
        return dom_error(
            agent,
            "InvalidAccessError",
            &format!("The key does not support the '{usage}' operation."),
        );
    }
    Ok(k)
}

/// `sign(algorithm, key, data)` (§14.3.3; HMAC, §29.3).
fn subtle_sign(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (alg, key, data) = (a.arg(0), a.arg(1), a.arg(2));
    promise(agent, |agent| {
        let data = bytes(agent, &data, "data")?;
        let k = key_for(agent, &alg, &key, "sign")?;
        let mac = hmac(k.hash, &k.bytes, &data);
        Ok(array_buffer(agent, mac))
    })
}

/// `verify(algorithm, key, signature, data)` (§14.3.4; HMAC, §29.3).
fn subtle_verify(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (alg, key, sig, data) = (a.arg(0), a.arg(1), a.arg(2), a.arg(3));
    promise(agent, |agent| {
        let sig = bytes(agent, &sig, "signature")?;
        let data = bytes(agent, &data, "data")?;
        let k = key_for(agent, &alg, &key, "verify")?;
        let mac = hmac(k.hash, &k.bytes, &data);
        Ok(Value::Boolean(equal(&mac, &sig)))
    })
}

/// `encrypt`, `decrypt`, `deriveBits`, `deriveKey`, `wrapKey` y `unwrapKey`: ningún algoritmo
/// registrado para ellas, así que "normalize an algorithm" (§18.4.4) da `NotSupportedError`.
fn subtle_unsupported(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let alg = a.arg(0);
    promise(agent, |agent| {
        let name = algorithm_name(agent, &alg)?;
        not_supported(agent, &name)
    })
}

// --- CryptoKey ---

fn key_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_key(agent, a)?;
    Ok(Value::str("secret"))
}

fn key_extractable(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(this_key(agent, a)?.extractable))
}

/// `algorithm` (§13.5): un `HmacKeyAlgorithm`, el mismo objeto cada vez (slot 0).
fn key_algorithm(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let k = this_key(agent, a)?;
    cached(agent, a, 0, |agent| {
        let hash = agent.new_plain_object();
        agent.create_data_property_or_throw(
            hash,
            PropertyKey::from("name"),
            Value::str(k.hash.name()),
        )?;
        let o = agent.new_plain_object();
        let entries = [
            ("name", Value::str("HMAC")),
            ("hash", Value::Object(hash)),
            ("length", Value::Number(f64::from(k.length))),
        ];
        for (name, v) in entries {
            agent.create_data_property_or_throw(o, PropertyKey::from(name), v)?;
        }
        Ok(Value::Object(o))
    })
}

/// `usages` (§13.5): un array, el mismo cada vez (slot 1).
fn key_usages(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let k = this_key(agent, a)?;
    cached(agent, a, 1, |agent| {
        let list: Vec<Value> = k.usages.iter().map(|u| Value::str(u)).collect();
        Ok(Value::Object(agent.create_array_from_list(&list)?))
    })
}

/// El valor del slot `i` de `this`, creado con `make` la primera vez.
fn cached(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    i: usize,
    make: impl FnOnce(&mut Agent) -> JsResult<Value>,
) -> JsResult<Value> {
    let Value::Object(o) = a.this else {
        return Ok(Value::Undefined);
    };
    if let Some(v) = agent.host_slots_mut(o).and_then(|s| s.get(i).cloned())
        && !matches!(v, Value::Undefined)
    {
        return Ok(v);
    }
    let v = make(agent)?;
    if let Some(s) = agent.host_slots_mut(o) {
        if s.len() <= i {
            s.resize(i + 1, Value::Undefined);
        }
        s[i] = v.clone();
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// FIPS 180-4, los ejemplos de NIST ("abc" y el mensaje de 448 bits).
    #[test]
    fn sha_fips_180_4_vectors() {
        let m2 = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        let cases: [(Hash, &[u8], &str); 6] = [
            (
                Hash::Sha1,
                b"abc",
                "a9993e364706816aba3e25717850c26c9cd0d89d",
            ),
            (Hash::Sha1, m2, "84983e441c3bd26ebaae4aa1f95129e5e54670f1"),
            (
                Hash::Sha256,
                b"abc",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                Hash::Sha256,
                m2,
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
            (
                Hash::Sha384,
                b"abc",
                "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed\
                 8086072ba1e7cc2358baeca134c825a7",
            ),
            (
                Hash::Sha512,
                b"abc",
                "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
                 2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
            ),
        ];
        for (h, m, want) in cases {
            assert_eq!(hex(&digest(h, m)), want, "{h:?}");
        }
        assert_eq!(
            hex(&digest(Hash::Sha256, b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    /// RFC 4231 (HMAC-SHA-256/384/512), casos 1, 2, 3, 4, 6 y 7; y RFC 2202 (HMAC-SHA-1),
    /// casos 1 y 2.
    #[test]
    fn hmac_rfc_4231_and_2202_vectors() {
        let key20 = vec![0x0b; 20];
        let aa20 = vec![0xaa; 20];
        let aa131 = vec![0xaa; 131];
        let dd50 = vec![0xdd; 50];
        let key25 = unhex("0102030405060708090a0b0c0d0e0f10111213141516171819");
        let cd50 = vec![0xcd; 50];
        let big1: &[u8] = b"Test Using Larger Than Block-Size Key - Hash Key First";
        let big2: &[u8] = b"This is a test using a larger than block-size key and a larger than block-size data. The key needs to be hashed before being used by the HMAC algorithm.";
        let cases: Vec<(&[u8], &[u8], [&str; 3])> = vec![
            (
                &key20,
                b"Hi There",
                [
                    "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7",
                    "afd03944d84895626b0825f4ab46907f15f9dadbe4101ec682aa034c7cebc59cfaea9ea9076ede7f4af152e8b2fa9cb6",
                    "87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cdedaa833b7d6b8a702038b274eaea3f4e4be9d914eeb61f1702e696c203a126854",
                ],
            ),
            (
                b"Jefe",
                b"what do ya want for nothing?",
                [
                    "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
                    "af45d2e376484031617f78d2b58a6b1b9c7ef464f5a01b47e42ec3736322445e8e2240ca5e69e2c78b3239ecfab21649",
                    "164b7a7bfcf819e2e395fbe73b56e0a387bd64222e831fd610270cd7ea2505549758bf75c05a994a6d034f65f8f0e6fdcaeab1a34d4a6b4b636e070a38bce737",
                ],
            ),
            (
                &aa20,
                &dd50,
                [
                    "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe",
                    "88062608d3e6ad8a0aa2ace014c8a86f0aa635d947ac9febe83ef4e55966144b2a5ab39dc13814b94e3ab6e101a34f27",
                    "fa73b0089d56a284efb0f0756c890be9b1b5dbdd8ee81a3655f83e33b2279d39bf3e848279a722c806b485a47e67c807b946a337bee8942674278859e13292fb",
                ],
            ),
            (
                &key25,
                &cd50,
                [
                    "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b",
                    "3e8a69b7783c25851933ab6290af6ca77a9981480850009cc5577c6e1f573b4e6801dd23c4a7d679ccf8a386c674cffb",
                    "b0ba465637458c6990e5a8c5f61d4af7e576d97ff94b872de76f8050361ee3dba91ca5c11aa25eb4d679275cc5788063a5f19741120c4f2de2adebeb10a298dd",
                ],
            ),
            (
                &aa131,
                big1,
                [
                    "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54",
                    "4ece084485813e9088d2c63a041bc5b44f9ef1012a2b588f3cd11f05033ac4c60c2ef6ab4030fe8296248df163f44952",
                    "80b24263c7c1a3ebb71493c1dd7be8b49b46d1f41b4aeec1121b013783f8f3526b56d037e05f2598bd0fd2215d6a1e5295e64f73f63f0aec8b915a985d786598",
                ],
            ),
            (
                &aa131,
                big2,
                [
                    "9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2",
                    "6617178e941f020d351e2f254e8fd32c602420feb0b8fb9adccebb82461e99c5a678cc31e799176d3860e6110c46523e",
                    "e37b6a775dc87dbaa4dfa9f96e5e3ffddebd71f8867289865df5a32d20cdc944b6022cac3c4982b10d5eeb55c3e4de15134676fb6de0446065c97440fa8c6a58",
                ],
            ),
        ];
        for (key, data, [s256, s384, s512]) in cases {
            assert_eq!(hex(&hmac(Hash::Sha256, key, data)), s256);
            assert_eq!(hex(&hmac(Hash::Sha384, key, data)), s384);
            assert_eq!(hex(&hmac(Hash::Sha512, key, data)), s512);
        }
        assert_eq!(
            hex(&hmac(Hash::Sha1, &key20, b"Hi There")),
            "b617318655057264e28bc0b6fb378c8ef146be00"
        );
        assert_eq!(
            hex(&hmac(Hash::Sha1, b"Jefe", b"what do ya want for nothing?")),
            "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79"
        );
    }

    #[test]
    fn base64url_round_trip() {
        for n in 0..40 {
            let data: Vec<u8> = (0..n as u8).map(|i| i.wrapping_mul(37)).collect();
            assert_eq!(base64url_decode(&base64url_encode(&data)).unwrap(), data);
        }
        assert_eq!(base64url_encode(b"\xfb\xff"), "-_8");
        assert!(base64url_decode("a").is_none());
        assert!(base64url_decode("a+").is_none());
    }
}
