//! Autenticación HTTP (RFC 9110 §11): los retos de `WWW-Authenticate` / `Proxy-Authenticate`,
//! los esquemas Basic (RFC 7617) y Digest (RFC 7616, MD5 y SHA-256, con y sin `-sess`) y la
//! caché de credenciales por origen y *realm* que usa el [`crate::Fetcher`].
//!
//! # API para el embebedor (el shell)
//!
//! Ante un 401 (o 407) con un reto que entiende, el `Fetcher` busca credenciales en este orden:
//! las ya guardadas para el origen y el *realm*, las de la URL (`https://usuario:clave@host/`) y,
//! por último, [`crate::Fetcher::authenticator`]: una función que recibe un [`AuthPrompt`] (URL,
//! *realm*, esquema y si las credenciales anteriores fallaron) y devuelve las [`Credentials`]
//! o `None` (cancelar: la respuesta es el 401). Con credenciales, repite la petición; si el
//! servidor las acepta, quedan guardadas y las siguientes peticiones al mismo espacio de
//! protección (§11.5: el mismo origen y las rutas bajo la del primer 401) las mandan sin
//! esperar otro reto.
//!
//! Sin `authenticator`, el 401 llega al llamador; sus retos se leen con [`challenges`] y, con
//! credenciales del usuario, [`crate::Fetcher::set_credentials`] las guarda para reintentar.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use bf_url::Url;

use crate::http::Headers;

/// Un usuario y su contraseña.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

impl Credentials {
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Credentials {
        Credentials {
            username: username.into(),
            password: password.into(),
        }
    }

    /// Las de la URL (`usuario:clave@`), sin el *percent-encoding*; `None` si no tiene.
    pub fn from_url(url: &Url) -> Option<Credentials> {
        if url.username.is_empty() && url.password.is_empty() {
            return None;
        }
        let decode =
            |s: &str| String::from_utf8_lossy(&bf_url::percent::decode(s.as_bytes())).into_owned();
        Some(Credentials::new(
            decode(&url.username),
            decode(&url.password),
        ))
    }
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username)
            .field("password", &"…")
            .finish()
    }
}

/// Un reto (§11.3): el esquema (en minúsculas) y sus parámetros (nombres en minúsculas, valores
/// sin comillas) o su `token68`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    pub scheme: String,
    pub params: Vec<(String, String)>,
    pub token68: Option<String>,
}

impl Challenge {
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// §11.5: el *realm* (`None` si no trae).
    pub fn realm(&self) -> Option<&str> {
        self.param("realm")
    }
}

/// Lo que se le pregunta al embebedor.
#[derive(Debug, Clone)]
pub struct AuthPrompt {
    /// La URL que pidió autenticación (sin credenciales).
    pub url: Url,
    /// `true` para un 407 (`Proxy-Authenticate`).
    pub proxy: bool,
    pub realm: Option<String>,
    /// El reto elegido (`basic` o `digest`).
    pub scheme: String,
    /// Todos los retos de la respuesta.
    pub challenges: Vec<Challenge>,
    /// Las credenciales anteriores (guardadas, de la URL o del embebedor) fallaron.
    pub retry: bool,
}

/// La función que aporta credenciales (ver la documentación del módulo).
pub type Authenticator = dyn Fn(&AuthPrompt) -> Option<Credentials> + Send + Sync;

// --- Retos ---

fn is_tchar(c: char) -> bool {
    c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c)
}

/// Los retos de las cabeceras `name` (`www-authenticate` o `proxy-authenticate`) de una
/// respuesta (§11.6.1, §11.7.1).
pub fn challenges(headers: &Headers, name: &str) -> Vec<Challenge> {
    let mut out = Vec::new();
    for value in headers.get_all(name) {
        out.extend(parse_challenges(value));
    }
    out
}

/// §11.2–11.3: `challenge = auth-scheme [ 1*SP ( token68 / #auth-param ) ]`, varios separados
/// por comas. Un elemento de la lista que empieza con `token SP` abre un reto nuevo; uno que es
/// `nombre=valor` es un parámetro del reto en curso.
pub fn parse_challenges(value: &str) -> Vec<Challenge> {
    let mut out: Vec<Challenge> = Vec::new();
    for element in split_quoted(value) {
        let element = element.trim();
        if element.is_empty() {
            continue;
        }
        let scheme_len = element
            .find(|c: char| !is_tchar(c))
            .unwrap_or(element.len());
        let rest = &element[scheme_len..];
        let starts_param = rest.trim_start().starts_with('=');
        if scheme_len > 0 && !starts_param {
            // Un esquema nuevo, quizá con su primer parámetro o un token68.
            let mut c = Challenge {
                scheme: element[..scheme_len].to_ascii_lowercase(),
                params: Vec::new(),
                token68: None,
            };
            let rest = rest.trim_start();
            if !rest.is_empty() {
                match parse_param(rest) {
                    Some(p) => c.params.push(p),
                    None => c.token68 = Some(rest.to_owned()),
                }
            }
            out.push(c);
        } else if let Some(p) = parse_param(element)
            && let Some(c) = out.last_mut()
        {
            c.params.push(p);
        }
    }
    out
}

/// `nombre = ( token / quoted-string )`; `None` si no es un parámetro (un token68 como
/// `abc==` tiene `=` al final, sin valor).
fn parse_param(s: &str) -> Option<(String, String)> {
    let (name, value) = s.split_once('=')?;
    let name = name.trim();
    if name.is_empty() || !name.chars().all(is_tchar) {
        return None;
    }
    let value = value.trim();
    if value.is_empty() || value.starts_with('=') {
        return None;
    }
    let value = if let Some(inner) = value.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => out.extend(chars.next()),
                '"' => break,
                c => out.push(c),
            }
        }
        out
    } else {
        value.to_owned()
    };
    Some((name.to_ascii_lowercase(), value))
}

/// Parte por comas fuera de comillas.
fn split_quoted(v: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut quoted, mut escaped) = (0, false, false);
    for (i, c) in v.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ',' if !quoted => {
                out.push(&v[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&v[start..]);
    out
}

/// El reto que se va a contestar: Digest con SHA-256, Digest con MD5 y Basic, en ese orden
/// (RFC 7616 §3.7: el más fuerte que se entienda).
pub fn choose(challenges: &[Challenge]) -> Option<&Challenge> {
    let rank = |c: &Challenge| match c.scheme.as_str() {
        "digest" => match Algorithm::parse(c.param("algorithm").unwrap_or("MD5")) {
            Some((Algorithm::Sha256, _)) => Some(3),
            Some((Algorithm::Md5, _)) => Some(2),
            None => None,
        },
        "basic" => Some(1),
        _ => None,
    };
    challenges
        .iter()
        .filter_map(|c| rank(c).map(|r| (r, c)))
        .max_by_key(|(r, _)| *r)
        .map(|(_, c)| c)
}

// --- Basic (RFC 7617) ---

/// §2: `Basic base64(usuario ":" clave)`, en UTF-8 (§2.1).
pub fn basic(creds: &Credentials) -> String {
    format!(
        "Basic {}",
        crate::websocket::base64(format!("{}:{}", creds.username, creds.password).as_bytes())
    )
}

// --- Digest (RFC 7616) ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algorithm {
    Md5,
    Sha256,
}

impl Algorithm {
    /// §3.3: `MD5`, `SHA-256` y sus variantes `-sess` (el `bool`).
    fn parse(s: &str) -> Option<(Algorithm, bool)> {
        match s.to_ascii_uppercase().as_str() {
            "MD5" => Some((Algorithm::Md5, false)),
            "MD5-SESS" => Some((Algorithm::Md5, true)),
            "SHA-256" => Some((Algorithm::Sha256, false)),
            "SHA-256-SESS" => Some((Algorithm::Sha256, true)),
            _ => None,
        }
    }

    fn hash(self, data: &str) -> String {
        let bytes = match self {
            Algorithm::Md5 => md5(data.as_bytes()).to_vec(),
            Algorithm::Sha256 => sha256(data.as_bytes()).to_vec(),
        };
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// El estado de Digest con un servidor: el reto y el contador de usos del `nonce` (§3.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestState {
    pub realm: String,
    pub nonce: String,
    pub opaque: Option<String>,
    pub algorithm: String,
    /// `qop=auth` (no se usa `auth-int`).
    pub qop: bool,
    pub userhash: bool,
    pub nc: u32,
}

impl DigestState {
    /// Del reto; `None` si el algoritmo no se entiende, falta el `nonce` o solo ofrece
    /// `auth-int`.
    pub fn from_challenge(c: &Challenge) -> Option<DigestState> {
        let algorithm = c.param("algorithm").unwrap_or("MD5");
        Algorithm::parse(algorithm)?;
        let qop = match c.param("qop") {
            None => false,
            Some(q) if q.split(',').any(|q| q.trim().eq_ignore_ascii_case("auth")) => true,
            Some(_) => return None,
        };
        Some(DigestState {
            realm: c.realm().unwrap_or("").to_owned(),
            nonce: c.param("nonce")?.to_owned(),
            opaque: c.param("opaque").map(str::to_owned),
            algorithm: algorithm.to_owned(),
            qop,
            userhash: c
                .param("userhash")
                .is_some_and(|v| v.eq_ignore_ascii_case("true")),
            nc: 0,
        })
    }

    /// §3.4: el valor de `Authorization` para `method` y `uri` (el *request-target*), con el
    /// siguiente `nc` y `cnonce`.
    pub fn authorization(
        &mut self,
        creds: &Credentials,
        method: &str,
        uri: &str,
        cnonce: &str,
    ) -> String {
        self.nc += 1;
        let (alg, sess) = Algorithm::parse(&self.algorithm).unwrap_or((Algorithm::Md5, false));
        let nc = format!("{:08x}", self.nc);
        let mut ha1 = alg.hash(&format!(
            "{}:{}:{}",
            creds.username, self.realm, creds.password
        ));
        if sess {
            ha1 = alg.hash(&format!("{ha1}:{}:{cnonce}", self.nonce));
        }
        let ha2 = alg.hash(&format!("{method}:{uri}"));
        let response = if self.qop {
            alg.hash(&format!("{ha1}:{}:{nc}:{cnonce}:auth:{ha2}", self.nonce))
        } else {
            alg.hash(&format!("{ha1}:{}:{ha2}", self.nonce))
        };
        // §3.4.4: con `userhash`, el usuario va como H(usuario ":" realm).
        let username = if self.userhash {
            alg.hash(&format!("{}:{}", creds.username, self.realm))
        } else {
            creds.username.clone()
        };
        let q = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let mut out = format!(
            "Digest username=\"{}\", realm=\"{}\", uri=\"{}\", algorithm={}, nonce=\"{}\"",
            q(&username),
            q(&self.realm),
            q(uri),
            self.algorithm,
            q(&self.nonce)
        );
        if self.qop {
            out.push_str(&format!(", nc={nc}, cnonce=\"{}\", qop=auth", q(cnonce)));
        }
        out.push_str(&format!(", response=\"{response}\""));
        if let Some(o) = &self.opaque {
            out.push_str(&format!(", opaque=\"{}\"", q(o)));
        }
        if self.userhash {
            out.push_str(", userhash=true");
        }
        out
    }
}

/// Un `cnonce` (§3.4): 128 bits impredecibles para el servidor, en hexadecimal.
pub fn cnonce() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut out = String::new();
    for _ in 0..2 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

// --- Caché de credenciales ---

/// Cómo se contesta en un espacio de protección.
#[derive(Debug, Clone)]
enum Scheme {
    Basic,
    Digest(DigestState),
    /// Credenciales del embebedor todavía sin usar: se mandan cuando llegue el reto.
    Pending,
}

#[derive(Debug, Clone)]
struct Entry {
    /// El origen (`scheme://host:puerto`).
    origin: String,
    proxy: bool,
    realm: String,
    /// RFC 7617 §2.2: las rutas que empiezan con este prefijo están en el espacio.
    prefix: String,
    creds: Credentials,
    scheme: Scheme,
}

/// Las credenciales aceptadas, por origen y *realm*.
#[derive(Debug, Default)]
pub struct AuthCache {
    entries: Mutex<Vec<Entry>>,
}

/// El directorio de la ruta (hasta la última `/`).
fn directory(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..=i].to_owned(),
        None => "/".to_owned(),
    }
}

pub(crate) fn origin_of(url: &Url) -> String {
    url.origin().to_string()
}

impl AuthCache {
    /// Credenciales para `origin` y `realm`, si las hay.
    pub fn credentials(&self, origin: &str, proxy: bool, realm: &str) -> Option<Credentials> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries
            .iter()
            .find(|e| e.origin == origin && e.proxy == proxy && e.realm == realm)
            .map(|e| e.creds.clone())
    }

    /// Guarda (o reemplaza) las credenciales de un espacio y cómo contestar en él sin esperar
    /// otro reto: con `digest`, Digest con ese estado; si no, Basic.
    pub(crate) fn store(
        &self,
        url: &Url,
        proxy: bool,
        realm: &str,
        creds: Credentials,
        digest: Option<DigestState>,
    ) {
        let origin = origin_of(url);
        let prefix = directory(&url.path_string());
        let scheme = match digest {
            Some(d) => Scheme::Digest(d),
            None => Scheme::Basic,
        };
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|e| !(e.origin == origin && e.proxy == proxy && e.realm == realm));
        entries.push(Entry {
            origin,
            proxy,
            realm: realm.to_owned(),
            prefix,
            creds,
            scheme,
        });
    }

    /// Credenciales para el *realm* de `url`, para contestar al próximo reto (no se mandan
    /// antes).
    pub(crate) fn remember(&self, url: &Url, proxy: bool, realm: &str, creds: Credentials) {
        self.store(url, proxy, realm, creds, None);
        let origin = origin_of(url);
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = entries
            .iter_mut()
            .find(|e| e.origin == origin && e.proxy == proxy && e.realm == realm)
        {
            e.scheme = Scheme::Pending;
        }
    }

    /// Olvida las credenciales de un espacio (el servidor las rechazó).
    pub(crate) fn forget(&self, url: &Url, proxy: bool, realm: &str) {
        let origin = origin_of(url);
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|e| !(e.origin == origin && e.proxy == proxy && e.realm == realm));
    }

    /// RFC 7617 §2.2 / RFC 7616 §3.6: la cabecera `Authorization` para mandar sin esperar el
    /// reto, si `url` está bajo el espacio de unas credenciales guardadas, y su *realm*.
    pub(crate) fn preemptive(&self, url: &Url, method: &str, target: &str) -> Option<String> {
        let origin = origin_of(url);
        let path = url.path_string();
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let e = entries
            .iter_mut()
            .filter(|e| !e.proxy && e.origin == origin && path.starts_with(&e.prefix))
            .max_by_key(|e| e.prefix.len())?;
        match &mut e.scheme {
            Scheme::Basic => Some(basic(&e.creds)),
            Scheme::Digest(d) => Some(d.authorization(&e.creds, method, target, &cnonce())),
            Scheme::Pending => None,
        }
    }
}

// --- MD5 (RFC 1321) y SHA-256 (FIPS 180-4) ---

/// MD5 (RFC 1321 §3).
pub fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    // K[i] = floor(2^32 × |sin(i + 1)|).
    let k: Vec<u32> = (0..64)
        .map(|i| ((i as f64 + 1.0).sin().abs() * 4_294_967_296.0) as u32)
        .collect();
    let mut st = [0x6745_2301u32, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_le_bytes());
    for block in msg.chunks(64) {
        let m: Vec<u32> = block
            .chunks(4)
            .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
            .collect();
        let [mut a, mut b, mut c, mut d] = st;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        st[0] = st[0].wrapping_add(a);
        st[1] = st[1].wrapping_add(b);
        st[2] = st[2].wrapping_add(c);
        st[3] = st[3].wrapping_add(d);
    }
    let mut out = [0u8; 16];
    for (i, w) in st.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    out
}

/// SHA-256 (FIPS 180-4 §6.2).
pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h = [
        0x6a09e667u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for block in msg.chunks(64) {
        let mut w = [0u32; 64];
        for (i, c) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(v);
        }
    }
    let mut out = [0u8; 32];
    for (i, w) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&w.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn hashes() {
        // RFC 1321 §A.5.
        assert_eq!(hex(&md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex(&md5(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            hex(&md5(
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"
            )),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
        // FIPS 180-4 (ejemplos de NIST).
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn parses_challenges() {
        let c = parse_challenges(
            "Newauth realm=\"apps\", type=1, title=\"Login to \\\"apps\\\"\", Basic realm=\"simple\"",
        );
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].scheme, "newauth");
        assert_eq!(c[0].param("title"), Some("Login to \"apps\""));
        assert_eq!(c[0].param("type"), Some("1"));
        assert_eq!(
            (c[1].scheme.as_str(), c[1].realm()),
            ("basic", Some("simple"))
        );
        let c = parse_challenges("Negotiate abc123==, Digest realm=\"r, con coma\", nonce=x");
        assert_eq!(c[0].token68.as_deref(), Some("abc123=="));
        assert_eq!(c[1].realm(), Some("r, con coma"));
        assert_eq!(c[1].param("nonce"), Some("x"));
        assert_eq!(parse_challenges("Basic")[0].params, []);
    }

    #[test]
    fn chooses_the_strongest() {
        let c = parse_challenges(
            "Basic realm=a, Digest realm=a, nonce=n, algorithm=MD5, \
             Digest realm=a, nonce=n, algorithm=SHA-256, Digest realm=a, nonce=n, algorithm=X",
        );
        assert_eq!(choose(&c).unwrap().param("algorithm"), Some("SHA-256"));
        assert_eq!(choose(&c[..1]).unwrap().scheme, "basic");
        assert!(choose(&parse_challenges("Bearer realm=x")).is_none());
    }

    #[test]
    fn basic_header() {
        // RFC 7617 §2.
        let creds = Credentials::new("Aladdin", "open sesame");
        assert_eq!(basic(&creds), "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==");
        let u = Url::parse("http://us%C3%A9r:p%40ss@example.com/", None).unwrap();
        assert_eq!(
            Credentials::from_url(&u),
            Some(Credentials::new("usér", "p@ss"))
        );
    }

    /// RFC 7616 §3.9.1: el ejemplo con SHA-256 y con MD5.
    #[test]
    fn digest_example() {
        let creds = Credentials::new("Mufasa", "Circle of Life");
        let challenge = |alg: &str| {
            parse_challenges(&format!(
                "Digest realm=\"http-auth@example.org\", qop=\"auth, auth-int\", \
                 algorithm={alg}, nonce=\"7ypf/xlj9XXwfDPEoM4URrv/xwf94BcCAzFZH4GiTo0v\", \
                 opaque=\"FQhe/qaU925kfnzjCev0ciny7QMkPqMAFRtzCUYo5tdS\""
            ))
            .remove(0)
        };
        let cnonce = "f2/wE4q74E6zIJEtWaHKaf5wv/H5QzzpXusqGemxURZJ";
        for (alg, response) in [
            (
                "SHA-256",
                "753927fa0e85d155564e2e272a28d1802ca10daf4496794697cf8db5856cb6c1",
            ),
            ("MD5", "8ca523f5e9506fed4657c9700eebdbec"),
        ] {
            let mut d = DigestState::from_challenge(&challenge(alg)).unwrap();
            let h = d.authorization(&creds, "GET", "/dir/index.html", cnonce);
            assert!(h.contains(&format!("response=\"{response}\"")), "{h}");
            assert!(h.contains("nc=00000001, cnonce=\""), "{h}");
            assert!(h.contains("opaque=\"FQhe/"), "{h}");
            // El siguiente uso del mismo `nonce` lleva nc=2.
            let h = d.authorization(&creds, "GET", "/dir/index.html", cnonce);
            assert!(h.contains("nc=00000002"));
        }
        // Solo `auth-int`: no se contesta.
        let only_int = parse_challenges("Digest realm=r, nonce=n, qop=auth-int").remove(0);
        assert!(DigestState::from_challenge(&only_int).is_none());
    }

    #[test]
    fn cache_spaces() {
        let cache = AuthCache::default();
        let u = Url::parse("https://example.com/docs/a.html", None).unwrap();
        let creds = Credentials::new("u", "p");
        cache.store(&u, false, "r", creds.clone(), None);
        let under = Url::parse("https://example.com/docs/sub/b", None).unwrap();
        let outside = Url::parse("https://example.com/other", None).unwrap();
        assert_eq!(
            cache.preemptive(&under, "GET", "/docs/sub/b").as_deref(),
            Some("Basic dTpw")
        );
        assert_eq!(cache.preemptive(&outside, "GET", "/other"), None);
        assert_eq!(
            cache.credentials("https://example.com", false, "r"),
            Some(creds)
        );
        cache.forget(&u, false, "r");
        assert_eq!(cache.preemptive(&under, "GET", "/docs/sub/b"), None);
    }
}
