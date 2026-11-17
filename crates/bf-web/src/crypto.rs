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

pub mod aes; // E1: AES (FIPS 197) y los modos CBC, CTR y GCM
pub mod sha; // E1: SHA con estado incremental, para HMAC y PBKDF2

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
    sha::Hmac::new(h, key).mac(data)
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

/// `[[usages]]`, en el orden del enum `KeyUsage` (§13.3; E1: con los cuatro de "Modern
/// Algorithms in the Web Cryptography API", que ningún algoritmo de aquí admite: dan
/// `SyntaxError` y no `TypeError`).
const USAGES: [&str; 12] = [
    "encrypt",
    "decrypt",
    "sign",
    "verify",
    "deriveKey",
    "deriveBits",
    "wrapKey",
    "unwrapKey",
    "encapsulateKey",
    "encapsulateBits",
    "decapsulateKey",
    "decapsulateBits",
];

/// El algoritmo de una clave (todas son secretas, `[[type]]` "secret").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyKind {
    Hmac,
    AesCbc,
    AesCtr,
    AesGcm,
    Pbkdf2,
    Hkdf,
}

impl KeyKind {
    const ALL: [KeyKind; 6] = [
        KeyKind::Hmac,
        KeyKind::AesCbc,
        KeyKind::AesCtr,
        KeyKind::AesGcm,
        KeyKind::Pbkdf2,
        KeyKind::Hkdf,
    ];

    /// El nombre registrado (§18.4.4: se compara sin distinguir mayúsculas).
    fn name(self) -> &'static str {
        match self {
            KeyKind::Hmac => "HMAC",
            KeyKind::AesCbc => "AES-CBC",
            KeyKind::AesCtr => "AES-CTR",
            KeyKind::AesGcm => "AES-GCM",
            KeyKind::Pbkdf2 => "PBKDF2",
            KeyKind::Hkdf => "HKDF",
        }
    }

    /// El `alg` de la JWK de una clave AES de `bits` bits (RFC 7518 §4.4, §5.2; Web Crypto
    /// §28.x.4: `A128CBC`, `A256GCM`…).
    fn aes_jwk_alg(self, bits: u32) -> String {
        let mode = match self {
            KeyKind::AesCbc => "CBC",
            KeyKind::AesCtr => "CTR",
            _ => "GCM",
        };
        format!("A{bits}{mode}")
    }
}

/// Las operaciones de §18.4 ("registered algorithms" por operación).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    ImportKey,
    GenerateKey,
    Encrypt,
    Decrypt,
    Derive,
    GetKeyLength,
}

/// Lo que guarda un `CryptoKey` (§13). `hash` solo cuenta en HMAC; `length` es la longitud de
/// la clave en bits (HMAC y AES).
#[derive(Debug, Clone)]
struct KeyData {
    kind: KeyKind,
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
                    .method("encrypt", 3, subtle_encrypt)
                    .method("decrypt", 3, subtle_decrypt)
                    .method("deriveBits", 3, subtle_derive_bits)
                    .method("deriveKey", 5, subtle_derive_key)
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

/// E1: "normalize an algorithm" (§18.4.4) para `op`: el algoritmo registrado con ese nombre
/// para esa operación, o `NotSupportedError`.
fn normalize_kind(agent: &mut Agent, alg: &Value, op: Op) -> JsResult<KeyKind> {
    let name = algorithm_name(agent, alg)?;
    let kind = KeyKind::ALL
        .into_iter()
        .find(|k| k.name().eq_ignore_ascii_case(&name));
    let supported = kind.is_some_and(|k| match op {
        Op::ImportKey => true,
        Op::GenerateKey | Op::GetKeyLength => !matches!(k, KeyKind::Pbkdf2 | KeyKind::Hkdf),
        Op::Encrypt | Op::Decrypt => {
            matches!(k, KeyKind::AesCbc | KeyKind::AesCtr | KeyKind::AesGcm)
        }
        Op::Derive => matches!(k, KeyKind::Pbkdf2 | KeyKind::Hkdf),
    });
    match kind {
        Some(k) if supported => Ok(k),
        _ => not_supported(agent, &name),
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

/// El material de una clave a importar: bytes (`raw`, o lo que sale de `deriveKey`) o una JWK.
enum Material {
    Raw(Vec<u8>),
    Jwk(Value),
}

/// `importKey(format, keyData, algorithm, extractable, keyUsages)` (§14.3.9; HMAC §29.6, AES
/// §28.2.4/§28.3.4/§28.4.4, PBKDF2 §31.4, HKDF §32.4).
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
        let kind = normalize_kind(agent, &alg, Op::ImportKey)?;
        let material = match format.as_str() {
            "raw" => Material::Raw(bytes(agent, &data, "keyData")?),
            "jwk" => {
                if !matches!(data, Value::Object(_)) {
                    return agent.type_error(
                        "Failed to execute 'importKey' on 'SubtleCrypto': keyData is not a JsonWebKey.",
                    );
                }
                Material::Jwk(data.clone())
            }
            _ => return not_supported(agent, &format!("{} {format}", kind.name())),
        };
        let k = import_key(agent, kind, &alg, material, extractable, usages)?;
        Ok(new_key(agent, k))
    })
}

/// Los pasos de "import key" de cada algoritmo, y el último de §14.3.9: una clave secreta sin
/// usos es `SyntaxError`.
fn import_key(
    agent: &mut Agent,
    kind: KeyKind,
    alg: &Value,
    material: Material,
    extractable: bool,
    usages: Vec<&'static str>,
) -> JsResult<KeyData> {
    let allowed: &[&str] = match kind {
        KeyKind::Hmac => &["sign", "verify"],
        KeyKind::AesCbc | KeyKind::AesCtr | KeyKind::AesGcm => {
            &["encrypt", "decrypt", "wrapKey", "unwrapKey"]
        }
        KeyKind::Pbkdf2 | KeyKind::Hkdf => &["deriveKey", "deriveBits"],
    };
    if usages.iter().any(|u| !allowed.contains(u)) {
        return dom_error(
            agent,
            "SyntaxError",
            "Cannot create a key using the specified key usages.",
        );
    }
    let (hash, length, key) = match kind {
        KeyKind::Hmac => {
            let hash = hmac_hash(agent, alg)?;
            let length = hmac_length(agent, alg)?;
            let key = match material {
                Material::Raw(b) => b,
                Material::Jwk(j) => {
                    let (key, jalg) = jwk_secret(agent, &j, extractable, &usages, "sig")?;
                    if jalg.is_some_and(|a| a != hash.jwk_alg()) {
                        return dom_error(
                            agent,
                            "DataError",
                            "The JWK \"alg\" member does not match the hash.",
                        );
                    }
                    key
                }
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
            (hash, length, key)
        }
        KeyKind::AesCbc | KeyKind::AesCtr | KeyKind::AesGcm => {
            let (key, jalg) = match material {
                Material::Raw(b) => (b, None),
                Material::Jwk(j) => jwk_secret(agent, &j, extractable, &usages, "enc")?,
            };
            if !matches!(key.len(), 16 | 24 | 32) {
                return dom_error(
                    agent,
                    "DataError",
                    "AES key data must be 128, 192 or 256 bits.",
                );
            }
            if jalg.is_some_and(|a| a != kind.aes_jwk_alg(key.len() as u32 * 8)) {
                return dom_error(
                    agent,
                    "DataError",
                    "The JWK \"alg\" member does not match the key.",
                );
            }
            (Hash::Sha256, key.len() as u32 * 8, key)
        }
        KeyKind::Pbkdf2 | KeyKind::Hkdf => {
            let Material::Raw(key) = material else {
                return not_supported(agent, &format!("{} jwk", kind.name()));
            };
            if extractable {
                return dom_error(
                    agent,
                    "SyntaxError",
                    "Keys for key derivation cannot be extractable.",
                );
            }
            (Hash::Sha256, 0, key)
        }
    };
    if usages.is_empty() {
        return dom_error(
            agent,
            "SyntaxError",
            "Usages cannot be empty when creating a key.",
        );
    }
    Ok(KeyData {
        kind,
        hash,
        length,
        bytes: key,
        extractable,
        usages,
    })
}

/// La clave de una JWK secreta (`kty` "oct"; §29.6, §28.x.4) y su `alg`, si tiene.
fn jwk_secret(
    agent: &mut Agent,
    jwk: &Value,
    extractable: bool,
    usages: &[&str],
    use_value: &str,
) -> JsResult<(Vec<u8>, Option<String>)> {
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
    let alg = field(agent, "alg")?;
    if !usages.is_empty()
        && let Some(u) = field(agent, "use")?
        && u != use_value
    {
        return dom_error(
            agent,
            "DataError",
            &format!("The JWK \"use\" member was not \"{use_value}\"."),
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
    Ok((key, alg))
}

/// `exportKey(format, key)` (§14.3.10; HMAC §29.7, AES §28.x.5).
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
        if matches!(k.kind, KeyKind::Pbkdf2 | KeyKind::Hkdf) {
            return not_supported(agent, &format!("{} export", k.kind.name()));
        }
        if !k.extractable {
            return dom_error(agent, "InvalidAccessError", "The key is not extractable.");
        }
        match format.as_str() {
            "raw" => Ok(array_buffer(agent, k.bytes)),
            "jwk" => {
                let o = agent.new_plain_object();
                let ops: Vec<Value> = k.usages.iter().map(|u| Value::str(u)).collect();
                let ops = Value::Object(agent.create_array_from_list(&ops)?);
                let alg = match k.kind {
                    KeyKind::Hmac => k.hash.jwk_alg().to_owned(),
                    kind => kind.aes_jwk_alg(k.length),
                };
                let entries = [
                    ("kty", Value::str("oct")),
                    ("k", Value::str(&base64url_encode(&k.bytes))),
                    ("alg", Value::str(&alg)),
                    ("key_ops", ops),
                    ("ext", Value::Boolean(k.extractable)),
                ];
                for (name, v) in entries {
                    agent.create_data_property_or_throw(o, PropertyKey::from(name), v)?;
                }
                Ok(Value::Object(o))
            }
            _ => not_supported(agent, &format!("{} {format}", k.kind.name())),
        }
    })
}

/// `generateKey(algorithm, extractable, keyUsages)` (§14.3.6; HMAC §29.5, AES §28.x.3).
fn subtle_generate_key(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (alg, extractable, uses) = (a.arg(0), a.arg(1), a.arg(2));
    promise(agent, |agent| {
        let extractable = bf_js::abstract_ops::conversion::to_boolean(agent, &extractable);
        let usages = usages(agent, &uses)?;
        let kind = normalize_kind(agent, &alg, Op::GenerateKey)?;
        let (hash, length) = match kind {
            KeyKind::Hmac => {
                let hash = hmac_hash(agent, &alg)?;
                let length = match hmac_length(agent, &alg)? {
                    Some(0) => {
                        return dom_error(
                            agent,
                            "OperationError",
                            "HMAC key length must not be zero.",
                        );
                    }
                    Some(l) => l,
                    None => hash.block_len() as u32 * 8,
                };
                (hash, length)
            }
            _ => (Hash::Sha256, aes_length(agent, &alg)?),
        };
        let allowed: &[&str] = if kind == KeyKind::Hmac {
            &["sign", "verify"]
        } else {
            &["encrypt", "decrypt", "wrapKey", "unwrapKey"]
        };
        if usages.iter().any(|u| !allowed.contains(u)) {
            return dom_error(
                agent,
                "SyntaxError",
                "Cannot create a key using the specified key usages.",
            );
        }
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
                kind,
                hash,
                length,
                bytes: key,
                extractable,
                usages,
            },
        ))
    })
}

/// El miembro `length` de `AesKeyGenParams`/`AesDerivedKeyParams` (§28.x.3, "get key length"):
/// obligatorio (`TypeError`) y de 128, 192 o 256 (`OperationError`).
fn aes_length(agent: &mut Agent, alg: &Value) -> JsResult<u32> {
    let Some(l) = hmac_length(agent, alg)? else {
        return agent.type_error(
            "Failed to execute on 'SubtleCrypto': required member length is undefined.",
        );
    };
    if !matches!(l, 128 | 192 | 256) {
        return dom_error(
            agent,
            "OperationError",
            "AES key length must be 128, 192 or 256 bits.",
        );
    }
    Ok(l)
}

/// Los pasos comunes de las operaciones con una clave (§14.3.1 a §14.3.8), después de
/// normalizar el algoritmo y copiar los datos: el algoritmo tiene que ser el de la clave y la
/// clave tiene que admitir el uso (`InvalidAccessError`).
fn key_for(agent: &mut Agent, key: &Value, kind: KeyKind, usage: &str) -> JsResult<KeyData> {
    let k = arg_key(agent, key)?;
    if k.kind != kind {
        return dom_error(
            agent,
            "InvalidAccessError",
            &format!("The key is not a {} key.", kind.name()),
        );
    }
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
        // §14.3.3: primero se normaliza el algoritmo (puede correr código de la página, p. ej.
        // un getter de `name`) y después se copian los bytes de `data`.
        normalize_hmac(agent, &alg)?;
        let data = bytes(agent, &data, "data")?;
        let k = key_for(agent, &key, KeyKind::Hmac, "sign")?;
        let mac = hmac(k.hash, &k.bytes, &data);
        Ok(array_buffer(agent, mac))
    })
}

/// `verify(algorithm, key, signature, data)` (§14.3.4; HMAC, §29.3).
fn subtle_verify(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (alg, key, sig, data) = (a.arg(0), a.arg(1), a.arg(2), a.arg(3));
    promise(agent, |agent| {
        // §14.3.4: normalizar, y después copiar la firma y los datos.
        normalize_hmac(agent, &alg)?;
        let sig = bytes(agent, &sig, "signature")?;
        let data = bytes(agent, &data, "data")?;
        let k = key_for(agent, &key, KeyKind::Hmac, "verify")?;
        let mac = hmac(k.hash, &k.bytes, &data);
        Ok(Value::Boolean(equal(&mac, &sig)))
    })
}

/// Un miembro obligatorio de un diccionario de parámetros (`TypeError` si falta).
fn required(agent: &mut Agent, alg: &Value, name: &str) -> JsResult<Value> {
    let v = member(agent, alg, name)?;
    if matches!(v, Value::Undefined) {
        return agent.type_error(&format!(
            "Failed to execute on 'SubtleCrypto': required member {name} is undefined."
        ));
    }
    Ok(v)
}

/// Un número de un diccionario como entero sin signo de hasta `max` (`[EnforceRange]`).
fn enforce_range(agent: &mut Agent, v: &Value, max: f64) -> JsResult<u32> {
    let n = agent.to_number(v)?;
    if !n.is_finite() || n.trunc() < 0.0 || n.trunc() > max {
        return agent.type_error("Failed to execute on 'SubtleCrypto': value is out of range.");
    }
    Ok(n.trunc() as u32)
}

/// Los parámetros de un cifrado AES ya normalizados (§28.2.1 `AesCtrParams`, §28.3.1
/// `AesCbcParams`, §28.4.1 `AesGcmParams`).
enum AesParams {
    Cbc {
        iv: Vec<u8>,
    },
    Ctr {
        counter: Vec<u8>,
        length: u32,
    },
    Gcm {
        iv: Vec<u8>,
        aad: Vec<u8>,
        tag_len: u32,
    },
}

fn aes_params(agent: &mut Agent, kind: KeyKind, alg: &Value) -> JsResult<AesParams> {
    Ok(match kind {
        KeyKind::AesCbc => {
            let iv = required(agent, alg, "iv")?;
            AesParams::Cbc {
                iv: bytes(agent, &iv, "iv")?,
            }
        }
        KeyKind::AesCtr => {
            let counter = required(agent, alg, "counter")?;
            let counter = bytes(agent, &counter, "counter")?;
            let length = required(agent, alg, "length")?;
            AesParams::Ctr {
                counter,
                length: enforce_range(agent, &length, 255.0)?,
            }
        }
        _ => {
            let iv = required(agent, alg, "iv")?;
            let iv = bytes(agent, &iv, "iv")?;
            let aad = match member(agent, alg, "additionalData")? {
                Value::Undefined => Vec::new(),
                v => bytes(agent, &v, "additionalData")?,
            };
            let tag_len = match member(agent, alg, "tagLength")? {
                Value::Undefined => 128,
                v => enforce_range(agent, &v, 255.0)?,
            };
            AesParams::Gcm { iv, aad, tag_len }
        }
    })
}

fn operation_error<T>(agent: &mut Agent, what: &str) -> JsResult<T> {
    dom_error(agent, "OperationError", what)
}

/// `encrypt(algorithm, key, data)` (§14.3.1) y `decrypt` (§14.3.2), con AES-CBC, AES-CTR y
/// AES-GCM (§28).
fn subtle_crypt(agent: &mut Agent, a: &CallArgs<'_>, encrypt: bool) -> JsResult<Value> {
    let (alg, key, data) = (a.arg(0), a.arg(1), a.arg(2));
    promise(agent, |agent| {
        let op = if encrypt { Op::Encrypt } else { Op::Decrypt };
        let kind = normalize_kind(agent, &alg, op)?;
        let params = aes_params(agent, kind, &alg)?;
        // Los datos se copian después de normalizar (§14.3.1, pasos 2 a 4).
        let data = bytes(agent, &data, "data")?;
        let usage = if encrypt { "encrypt" } else { "decrypt" };
        let k = key_for(agent, &key, kind, usage)?;
        let Some(aes) = aes::Aes::new(&k.bytes) else {
            return operation_error(agent, "Invalid AES key.");
        };
        let out = match params {
            AesParams::Cbc { iv } => {
                if iv.len() != 16 {
                    return operation_error(agent, "The iv must be 16 bytes.");
                }
                if encrypt {
                    aes::cbc_encrypt(&aes, &iv, &data)
                } else {
                    aes::cbc_decrypt(&aes, &iv, &data)
                }
            }
            AesParams::Ctr { counter, length } => {
                if counter.len() != 16 || length == 0 || length > 128 {
                    return operation_error(agent, "Invalid AES-CTR counter or length.");
                }
                aes::ctr(&aes, &counter, length, &data)
            }
            AesParams::Gcm { iv, aad, tag_len } => {
                if !matches!(tag_len, 32 | 64 | 96 | 104 | 112 | 120 | 128) {
                    return operation_error(agent, "Invalid AES-GCM tag length.");
                }
                if iv.is_empty() {
                    return operation_error(agent, "The iv must not be empty.");
                }
                let t = (tag_len / 8) as usize;
                if encrypt {
                    Some(aes::gcm_encrypt(&aes, &iv, &aad, &data, t))
                } else {
                    aes::gcm_decrypt(&aes, &iv, &aad, &data, t)
                }
            }
        };
        match out {
            Some(o) => Ok(array_buffer(agent, o)),
            None => operation_error(
                agent,
                "The operation failed for an operation-specific reason.",
            ),
        }
    })
}

fn subtle_encrypt(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    subtle_crypt(agent, a, true)
}

fn subtle_decrypt(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    subtle_crypt(agent, a, false)
}

/// PBKDF2 (RFC 8018 §5.2) con HMAC.
pub fn pbkdf2(h: Hash, password: &[u8], salt: &[u8], iterations: u32, len: usize) -> Vec<u8> {
    let hlen = digest(h, &[]).len();
    let prf = sha::Hmac::new(h, password);
    let mut out = Vec::with_capacity(len.div_ceil(hlen) * hlen);
    for i in 1..=len.div_ceil(hlen) as u32 {
        let mut s = salt.to_vec();
        s.extend_from_slice(&i.to_be_bytes());
        let mut u = prf.mac(&s);
        let mut t = u.clone();
        for _ in 1..iterations {
            u = prf.mac(&u);
            for (x, y) in t.iter_mut().zip(&u) {
                *x ^= y;
            }
        }
        out.extend_from_slice(&t);
    }
    out.truncate(len);
    out
}

/// HKDF (RFC 5869 §2.2 y §2.3) con HMAC: `None` si `len` pasa de 255 bloques.
pub fn hkdf(h: Hash, ikm: &[u8], salt: &[u8], info: &[u8], len: usize) -> Option<Vec<u8>> {
    let hlen = digest(h, &[]).len();
    if len > 255 * hlen {
        return None;
    }
    let salt = if salt.is_empty() {
        vec![0u8; hlen]
    } else {
        salt.to_vec()
    };
    let prk = hmac(h, &salt, ikm);
    let mut out = Vec::with_capacity(len + hlen);
    let mut t: Vec<u8> = Vec::new();
    let mut i = 1u8;
    while out.len() < len {
        let mut m = t.clone();
        m.extend_from_slice(info);
        m.push(i);
        t = hmac(h, &prk, &m);
        out.extend_from_slice(&t);
        i = i.wrapping_add(1);
    }
    out.truncate(len);
    Some(out)
}

/// Los parámetros normalizados de PBKDF2 (§31.1 `Pbkdf2Params`) o HKDF (§32.1 `HkdfParams`).
struct DeriveParams {
    hash: Hash,
    salt: Vec<u8>,
    /// Las iteraciones de PBKDF2 o el `info` de HKDF.
    iterations: u32,
    info: Vec<u8>,
}

fn derive_params(agent: &mut Agent, kind: KeyKind, alg: &Value) -> JsResult<DeriveParams> {
    let h = required(agent, alg, "hash")?;
    let hash = normalize_hash(agent, &h)?;
    let salt = required(agent, alg, "salt")?;
    let salt = bytes(agent, &salt, "salt")?;
    let (iterations, info) = if kind == KeyKind::Pbkdf2 {
        let it = required(agent, alg, "iterations")?;
        (enforce_range(agent, &it, f64::from(u32::MAX))?, Vec::new())
    } else {
        let info = required(agent, alg, "info")?;
        (0, bytes(agent, &info, "info")?)
    };
    Ok(DeriveParams {
        hash,
        salt,
        iterations,
        info,
    })
}

/// Los bits de `deriveBits`/`deriveKey` con PBKDF2 (§31.3) o HKDF (§32.3): `length` en bits,
/// múltiplo de 8 (0 da un buffer vacío).
fn derive(
    agent: &mut Agent,
    kind: KeyKind,
    p: &DeriveParams,
    key: &KeyData,
    length: Option<u32>,
) -> JsResult<Vec<u8>> {
    let Some(length) = length.filter(|l| l % 8 == 0) else {
        return operation_error(agent, "The length must be a multiple of 8.");
    };
    let len = (length / 8) as usize;
    if kind == KeyKind::Pbkdf2 {
        if p.iterations == 0 {
            return operation_error(agent, "PBKDF2 iterations must not be zero.");
        }
        return Ok(pbkdf2(p.hash, &key.bytes, &p.salt, p.iterations, len));
    }
    match hkdf(p.hash, &key.bytes, &p.salt, &p.info, len) {
        Some(b) => Ok(b),
        None => operation_error(agent, "HKDF length is too large."),
    }
}

/// La longitud de `deriveBits` (`optional [EnforceRange] unsigned long? length = null`, §14.2):
/// `null` o sin dar es `None`; NaN, infinitos o fuera de rango son `TypeError`.
fn derive_length(agent: &mut Agent, v: &Value) -> JsResult<Option<u32>> {
    if v.is_nullish() {
        return Ok(None);
    }
    enforce_range(agent, v, f64::from(u32::MAX)).map(Some)
}

/// `deriveBits(algorithm, baseKey, length)` (§14.3.8).
fn subtle_derive_bits(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (alg, key, length) = (a.arg(0), a.arg(1), a.arg(2));
    promise(agent, |agent| {
        let kind = normalize_kind(agent, &alg, Op::Derive)?;
        let params = derive_params(agent, kind, &alg)?;
        let length = derive_length(agent, &length)?;
        let k = key_for(agent, &key, kind, "deriveBits")?;
        let out = derive(agent, kind, &params, &k, length)?;
        Ok(array_buffer(agent, out))
    })
}

/// `deriveKey(algorithm, baseKey, derivedKeyType, extractable, keyUsages)` (§14.3.7): los bits
/// de la longitud de la clave destino ("get key length": AES §28.x.6, HMAC §29.8), importados
/// como `raw`.
fn subtle_derive_key(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (alg, key, target, extractable, uses) = (a.arg(0), a.arg(1), a.arg(2), a.arg(3), a.arg(4));
    promise(agent, |agent| {
        let extractable = bf_js::abstract_ops::conversion::to_boolean(agent, &extractable);
        let usages = usages(agent, &uses)?;
        let kind = normalize_kind(agent, &alg, Op::Derive)?;
        let params = derive_params(agent, kind, &alg)?;
        let target_kind = normalize_kind(agent, &target, Op::GetKeyLength)?;
        let length = match target_kind {
            KeyKind::Hmac => {
                let hash = hmac_hash(agent, &target)?;
                match hmac_length(agent, &target)? {
                    Some(0) => {
                        return dom_error(agent, "TypeError", "HMAC key length must not be zero.");
                    }
                    Some(l) => l,
                    None => hash.block_len() as u32 * 8,
                }
            }
            _ => aes_length(agent, &target)?,
        };
        let k = key_for(agent, &key, kind, "deriveKey")?;
        let bits = derive(agent, kind, &params, &k, Some(length))?;
        let derived = import_key(
            agent,
            target_kind,
            &target,
            Material::Raw(bits),
            extractable,
            usages,
        )?;
        Ok(new_key(agent, derived))
    })
}

/// `wrapKey` y `unwrapKey`: ningún algoritmo registrado para ellas, así que "normalize an
/// algorithm" (§18.4.4) da `NotSupportedError`.
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

/// `algorithm` (§13.5): un `HmacKeyAlgorithm`, `AesKeyAlgorithm` o `KeyAlgorithm`, el mismo
/// objeto cada vez (slot 0).
fn key_algorithm(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let k = this_key(agent, a)?;
    cached(agent, a, 0, |agent| {
        let o = agent.new_plain_object();
        agent.create_data_property_or_throw(
            o,
            PropertyKey::from("name"),
            Value::str(k.kind.name()),
        )?;
        match k.kind {
            KeyKind::Hmac => {
                let hash = agent.new_plain_object();
                agent.create_data_property_or_throw(
                    hash,
                    PropertyKey::from("name"),
                    Value::str(k.hash.name()),
                )?;
                agent.create_data_property_or_throw(
                    o,
                    PropertyKey::from("hash"),
                    Value::Object(hash),
                )?;
                agent.create_data_property_or_throw(
                    o,
                    PropertyKey::from("length"),
                    Value::Number(f64::from(k.length)),
                )?;
            }
            KeyKind::AesCbc | KeyKind::AesCtr | KeyKind::AesGcm => {
                agent.create_data_property_or_throw(
                    o,
                    PropertyKey::from("length"),
                    Value::Number(f64::from(k.length)),
                )?;
            }
            KeyKind::Pbkdf2 | KeyKind::Hkdf => {}
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

    /// RFC 6070 §2 (PBKDF2-HMAC-SHA1) y RFC 5869 apéndice A, casos 1 y 3 (HKDF-SHA256).
    #[test]
    fn pbkdf2_rfc_6070_and_hkdf_rfc_5869_vectors() {
        assert_eq!(
            hex(&pbkdf2(Hash::Sha1, b"password", b"salt", 1, 20)),
            "0c60c80f961f0e71f3a9b524af6012062fe037a6"
        );
        assert_eq!(
            hex(&pbkdf2(Hash::Sha1, b"password", b"salt", 4096, 20)),
            "4b007901b765489abead49d926f721d065a429c1"
        );
        assert_eq!(
            hex(&pbkdf2(
                Hash::Sha1,
                b"passwordPASSWORDpassword",
                b"saltSALTsaltSALTsaltSALTsaltSALTsalt",
                4096,
                25
            )),
            "3d2eec4fe41c849b80c8d83662c0e44a8b291a964cf2f07038"
        );
        let ikm = [0x0bu8; 22];
        assert_eq!(
            hex(&hkdf(
                Hash::Sha256,
                &ikm,
                &unhex("000102030405060708090a0b0c"),
                &unhex("f0f1f2f3f4f5f6f7f8f9"),
                42
            )
            .unwrap()),
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
        );
        assert_eq!(
            hex(&hkdf(Hash::Sha256, &ikm, &[], &[], 42).unwrap()),
            "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8"
        );
        assert!(hkdf(Hash::Sha256, &ikm, &[], &[], 255 * 32 + 1).is_none());
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
