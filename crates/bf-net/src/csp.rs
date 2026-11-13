//! Content Security Policy Level 3 (W3C): el parseo de las políticas y las decisiones, sin
//! saber nada del navegador. Quien carga recursos o ejecuta scripts pregunta aquí:
//! - ¿se puede pedir esta URL para este *destination*? ([`Policy::request_violation`],
//!   §6.7.2.5 "Does url match source list", con la directiva efectiva de §6.8.1 y la lista de
//!   respaldo de §6.8.3);
//! - ¿se puede ejecutar este script o aplicar este estilo en línea? ([`Policy::inline_violation`],
//!   §6.7.3.3, con `'unsafe-inline'`, `'nonce-…'` y `'sha256-…'`/`'sha384-…'`/`'sha512-…'`);
//! - ¿se puede compilar texto como código (`eval`, `new Function`)? ([`Policy::eval_violation`],
//!   §4.4 "EnsureCSPDoesNotBlockStringCompilation", con `'unsafe-eval'`).
//!
//! Las políticas vienen de la cabecera `Content-Security-Policy` (una lista separada por
//! comas, §2.2.2), de `Content-Security-Policy-Report-Only` (solo informan) y de
//! `<meta http-equiv="Content-Security-Policy">` (§3.3: sin `report-uri`, `frame-ancestors`
//! ni `sandbox`).

use bf_infra::{Destination, Url};

/// Qué se hace con una violación (§2.2 *disposition*).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    Enforce,
    Report,
}

/// De dónde vino la política (§2.2 *source*).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Header,
    Meta,
}

/// Una directiva: nombre en minúsculas y valor (los tokens separados por espacios).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directive {
    pub name: String,
    pub value: Vec<String>,
}

impl Directive {
    /// La directiva como se escribió (para los mensajes de la consola).
    pub fn serialize(&self) -> String {
        if self.value.is_empty() {
            self.name.clone()
        } else {
            format!("{} {}", self.name, self.value.join(" "))
        }
    }
}

/// Una política (§2.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub directives: Vec<Directive>,
    pub disposition: Disposition,
    pub source: Source,
}

/// Una violación: la directiva que no lo permitió y la disposición de su política.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// La directiva que no lo permitió, serializada (`img-src 'none'`).
    pub directive: String,
    /// La directiva efectiva (§6.8.1): `img-src` aunque la que se aplicó fuera `default-src`.
    pub effective: String,
    /// La política entera, serializada (`originalPolicy`, §5.3).
    pub policy: String,
    pub disposition: Disposition,
}

/// Qué se ejecuta o aplica en línea (§6.7.3.3 *type*).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inline {
    /// Un `<script>` en línea.
    Script,
    /// Un atributo manejador de eventos (`onclick="…"`).
    ScriptAttribute,
    /// Un `<style>`.
    Style,
    /// Un atributo `style="…"`.
    StyleAttribute,
}

/// "Parse a serialized CSP list" (§2.2.2): las políticas de una cabecera, separadas por comas.
pub fn parse_list(value: &str, disposition: Disposition) -> Vec<Policy> {
    value
        .split(',')
        .map(|p| parse_policy(p, disposition, Source::Header))
        .filter(|p| !p.directives.is_empty())
        .collect()
}

/// "Parse a serialized CSP" (§2.2.1): directivas separadas por `;`; la primera de cada nombre
/// gana y los nombres que no son ASCII alfanumérico o `-` se ignoran. En una política de
/// `<meta>` se quitan `report-uri`, `frame-ancestors` y `sandbox` (§3.3 paso 5).
pub fn parse_policy(text: &str, disposition: Disposition, source: Source) -> Policy {
    let mut directives: Vec<Directive> = Vec::new();
    for token in text.split(';') {
        let token = token.trim_matches(|c: char| c.is_ascii_whitespace());
        let mut parts = token.split_ascii_whitespace();
        let Some(name) = parts.next() else { continue };
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            continue;
        }
        let name = name.to_ascii_lowercase();
        if directives.iter().any(|d| d.name == name) {
            continue;
        }
        if source == Source::Meta
            && matches!(name.as_str(), "report-uri" | "frame-ancestors" | "sandbox")
        {
            continue;
        }
        directives.push(Directive {
            name,
            value: parts.map(str::to_owned).collect(),
        });
    }
    Policy {
        directives,
        disposition,
        source,
    }
}

/// La directiva efectiva para una petición con `destination` (§6.8.1).
pub fn effective_directive(destination: Destination) -> Option<&'static str> {
    Some(match destination {
        Destination::Script => "script-src-elem",
        Destination::Style => "style-src-elem",
        Destination::Image => "img-src",
        Destination::Font => "font-src",
        Destination::Frame => "frame-src",
        Destination::Audio | Destination::Video => "media-src",
        Destination::Connect => "connect-src",
        Destination::Object => "object-src",
        Destination::Worker => "worker-src",
        Destination::Other => return None,
    })
}

/// La lista de respaldo de una directiva (§6.8.3): la primera que tenga la política se aplica.
fn fallback_list(directive: &str) -> &'static [&'static str] {
    match directive {
        "script-src-elem" => &["script-src-elem", "script-src", "default-src"],
        "script-src-attr" => &["script-src-attr", "script-src", "default-src"],
        "style-src-elem" => &["style-src-elem", "style-src", "default-src"],
        "style-src-attr" => &["style-src-attr", "style-src", "default-src"],
        "worker-src" => &["worker-src", "child-src", "script-src", "default-src"],
        "frame-src" => &["frame-src", "child-src", "default-src"],
        "script-src" => &["script-src", "default-src"],
        "connect-src" => &["connect-src", "default-src"],
        "font-src" => &["font-src", "default-src"],
        "img-src" => &["img-src", "default-src"],
        "media-src" => &["media-src", "default-src"],
        "object-src" => &["object-src", "default-src"],
        "manifest-src" => &["manifest-src", "default-src"],
        _ => &[],
    }
}

fn is_keyword(token: &str, keyword: &str) -> bool {
    token.eq_ignore_ascii_case(keyword)
}

fn nonce_of(token: &str) -> Option<&str> {
    let lower = token.to_ascii_lowercase();
    (lower.starts_with("'nonce-") && token.ends_with('\'') && token.len() > 8)
        .then(|| &token[7..token.len() - 1])
}

/// `'sha256-…'`, `'sha384-…'` o `'sha512-…'`: el algoritmo y el valor en base64.
fn hash_of(token: &str) -> Option<(u32, &str)> {
    let lower = token.to_ascii_lowercase();
    if !token.ends_with('\'') || token.len() < 10 {
        return None;
    }
    let bits = match &lower[..8] {
        "'sha256-" => 256,
        "'sha384-" => 384,
        "'sha512-" => 512,
        _ => return None,
    };
    Some((bits, &token[8..token.len() - 1]))
}

/// "Scheme-part match" (§6.7.2.7): `http` admite `https` y `ws` admite `wss`.
fn scheme_part_match(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
    a == b
        || (a == "http" && b == "https")
        || (a == "ws" && (b == "wss" || b == "http" || b == "https"))
        || (a == "wss" && b == "https")
}

fn is_network_scheme(s: &str) -> bool {
    matches!(s, "http" | "https" | "ws" | "wss" | "ftp")
}

/// "Host-part match" (§6.7.2.8): `*.ejemplo.com` coincide con los subdominios, no con
/// `ejemplo.com`.
fn host_part_match(pattern: &str, host: &str) -> bool {
    let (pattern, host) = (pattern.to_ascii_lowercase(), host.to_ascii_lowercase());
    match pattern.strip_prefix('*') {
        Some(rest) => host.ends_with(&rest),
        None => pattern == host,
    }
}

/// "Port-part match" (§6.7.2.9).
fn port_part_match(port: Option<&str>, url: &Url) -> bool {
    let default = bf_url::default_port(&url.scheme);
    let actual = url.port.or(default);
    match port {
        None => url.port.is_none() || url.port == default,
        Some("*") => true,
        Some(p) => p.parse::<u16>().ok().is_some_and(|p| Some(p) == actual),
    }
}

/// "Path-part match" (§6.7.2.10): sin `/` al final, exacta; con `/`, un prefijo.
fn path_part_match(path: &str, url: &Url) -> bool {
    if path.is_empty() || path == "/" {
        return true;
    }
    let decode =
        |s: &str| String::from_utf8_lossy(&bf_url::percent::decode(s.as_bytes())).into_owned();
    let expr = decode(path);
    let actual = decode(&url.path_string());
    if expr.ends_with('/') {
        actual.starts_with(&expr)
    } else {
        actual == expr
    }
}

/// "Does url match expression in origin" (§6.7.2.6).
fn matches_expression(url: &Url, expr: &str, origin: &Url) -> bool {
    let scheme = url.scheme.as_str();
    if expr == "*" {
        return is_network_scheme(scheme) || scheme == origin.scheme;
    }
    if is_keyword(expr, "'self'") {
        let same = url.origin() == origin.origin();
        // §6.7.2.6 paso 6: el mismo host por un esquema más seguro también es `'self'`.
        let upgrade = url.hostname() == origin.hostname()
            && url.host.is_some()
            && ((origin.scheme == "http" && matches!(scheme, "https" | "ws" | "wss"))
                || (origin.scheme == "https" && scheme == "wss"));
        return same || upgrade;
    }
    if expr.starts_with('\'') {
        // Palabras clave, nonces y hashes no coinciden con URLs.
        return false;
    }
    // scheme-source: `https:`, `data:`, `blob:`…
    if let Some(s) = expr.strip_suffix(':')
        && !s.contains('/')
        && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
    {
        return scheme_part_match(s, scheme);
    }
    // host-source: [esquema "://"] host [":" puerto] [camino].
    let (expr_scheme, rest) = match expr.split_once("://") {
        Some((s, r)) => (Some(s), r),
        None => (None, expr),
    };
    if url.host.is_none() {
        return false;
    }
    match expr_scheme {
        Some(s) if !scheme_part_match(s, scheme) => return false,
        None if !scheme_part_match(&origin.scheme, scheme) => return false,
        _ => {}
    }
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() => (h, Some(p)),
        _ => (authority, None),
    };
    host_part_match(host, &url.hostname())
        && port_part_match(port, url)
        && path_part_match(path, url)
}

/// "Does url match source list in origin" (§6.7.2.5). Una lista vacía o con `'none'` no
/// coincide con nada.
pub fn matches_source_list(url: &Url, list: &[String], origin: &Url) -> bool {
    if list.is_empty() || (list.len() == 1 && is_keyword(&list[0], "'none'")) {
        return false;
    }
    list.iter().any(|e| matches_expression(url, e, origin))
}

/// §6.7.3.2: ¿la lista permite todo lo en línea? (`'unsafe-inline'` sin nonces ni hashes, y
/// para los scripts sin `'strict-dynamic'`).
fn allows_all_inline(list: &[String], kind: Inline) -> bool {
    let script = matches!(kind, Inline::Script | Inline::ScriptAttribute);
    let unsafe_inline = list.iter().any(|t| is_keyword(t, "'unsafe-inline'"));
    let nonce_or_hash = list
        .iter()
        .any(|t| nonce_of(t).is_some() || hash_of(t).is_some());
    let strict_dynamic = script && list.iter().any(|t| is_keyword(t, "'strict-dynamic'"));
    unsafe_inline && !nonce_or_hash && !strict_dynamic
}

/// Normaliza base64url a base64 (§6.7.3.3 paso 4: los dos alfabetos valen).
fn base64_normalized(s: &str) -> String {
    s.replace('-', "+").replace('_', "/")
}

/// SHA-384 y SHA-512 (FIPS 180-4 §6.4), para las fuentes de hash.
fn sha512_family(data: &[u8], iv: [u64; 8], out_len: usize) -> Vec<u8> {
    const K: [u64; 80] = [
        0x428a2f98d728ae22,
        0x7137449123ef65cd,
        0xb5c0fbcfec4d3b2f,
        0xe9b5dba58189dbbc,
        0x3956c25bf348b538,
        0x59f111f1b605d019,
        0x923f82a4af194f9b,
        0xab1c5ed5da6d8118,
        0xd807aa98a3030242,
        0x12835b0145706fbe,
        0x243185be4ee4b28c,
        0x550c7dc3d5ffb4e2,
        0x72be5d74f27b896f,
        0x80deb1fe3b1696b1,
        0x9bdc06a725c71235,
        0xc19bf174cf692694,
        0xe49b69c19ef14ad2,
        0xefbe4786384f25e3,
        0x0fc19dc68b8cd5b5,
        0x240ca1cc77ac9c65,
        0x2de92c6f592b0275,
        0x4a7484aa6ea6e483,
        0x5cb0a9dcbd41fbd4,
        0x76f988da831153b5,
        0x983e5152ee66dfab,
        0xa831c66d2db43210,
        0xb00327c898fb213f,
        0xbf597fc7beef0ee4,
        0xc6e00bf33da88fc2,
        0xd5a79147930aa725,
        0x06ca6351e003826f,
        0x142929670a0e6e70,
        0x27b70a8546d22ffc,
        0x2e1b21385c26c926,
        0x4d2c6dfc5ac42aed,
        0x53380d139d95b3df,
        0x650a73548baf63de,
        0x766a0abb3c77b2a8,
        0x81c2c92e47edaee6,
        0x92722c851482353b,
        0xa2bfe8a14cf10364,
        0xa81a664bbc423001,
        0xc24b8b70d0f89791,
        0xc76c51a30654be30,
        0xd192e819d6ef5218,
        0xd69906245565a910,
        0xf40e35855771202a,
        0x106aa07032bbd1b8,
        0x19a4c116b8d2d0c8,
        0x1e376c085141ab53,
        0x2748774cdf8eeb99,
        0x34b0bcb5e19b48a8,
        0x391c0cb3c5c95a63,
        0x4ed8aa4ae3418acb,
        0x5b9cca4f7763e373,
        0x682e6ff3d6b2b8a3,
        0x748f82ee5defb2fc,
        0x78a5636f43172f60,
        0x84c87814a1f0ab72,
        0x8cc702081a6439ec,
        0x90befffa23631e28,
        0xa4506cebde82bde9,
        0xbef9a3f7b2c67915,
        0xc67178f2e372532b,
        0xca273eceea26619c,
        0xd186b8c721c0c207,
        0xeada7dd6cde0eb1e,
        0xf57d4f7fee6ed178,
        0x06f067aa72176fba,
        0x0a637dc5a2c898a6,
        0x113f9804bef90dae,
        0x1b710b35131c471b,
        0x28db77f523047d84,
        0x32caab7b40c72493,
        0x3c9ebe0a15c9bebc,
        0x431d67c49c100d4c,
        0x4cc5d4becb3e42b6,
        0x597f299cfc657e2a,
        0x5fcb6fab3ad6faec,
        0x6c44198c4a475817,
    ];
    let mut h = iv;
    let mut msg = data.to_vec();
    let bits = (data.len() as u128).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 128 != 112 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for block in msg.chunks(128) {
        let mut w = [0u64; 80];
        for (i, c) in block.chunks(8).enumerate() {
            w[i] = u64::from_be_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]);
        }
        for i in 16..80 {
            let s0 = w[i - 15].rotate_right(1) ^ w[i - 15].rotate_right(8) ^ (w[i - 15] >> 7);
            let s1 = w[i - 2].rotate_right(19) ^ w[i - 2].rotate_right(61) ^ (w[i - 2] >> 6);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..80 {
            let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
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
    h.iter()
        .flat_map(|w| w.to_be_bytes())
        .take(out_len)
        .collect()
}

/// El digest SHA-2 de `data` con `bits` (256, 384 o 512).
pub fn sha2(bits: u32, data: &[u8]) -> Vec<u8> {
    match bits {
        256 => crate::auth::sha256(data).to_vec(),
        384 => sha512_family(
            data,
            [
                0xcbbb9d5dc1059ed8,
                0x629a292a367cd507,
                0x9159015a3070dd17,
                0x152fecd8f70e5939,
                0x67332667ffc00b31,
                0x8eb44a8768581511,
                0xdb0c2e0d64f98fa7,
                0x47b5481dbefa4fa4,
            ],
            48,
        ),
        _ => sha512_family(
            data,
            [
                0x6a09e667f3bcc908,
                0xbb67ae8584caa73b,
                0x3c6ef372fe94f82b,
                0xa54ff53a5f1d36f1,
                0x510e527fade682d1,
                0x9b05688c2b3e6c1f,
                0x1f83d9abfb41bd6b,
                0x5be0cd19137e2179,
            ],
            64,
        ),
    }
}

/// "Does element match source list for type and source" (§6.7.3.3).
pub fn inline_allowed(list: &[String], kind: Inline, nonce: Option<&str>, source: &str) -> bool {
    if allows_all_inline(list, kind) {
        return true;
    }
    let element = matches!(kind, Inline::Script | Inline::Style);
    if element
        && let Some(n) = nonce.filter(|n| !n.is_empty())
        && list.iter().any(|t| nonce_of(t) == Some(n))
    {
        return true;
    }
    // Los atributos solo con `'unsafe-hashes'` (§6.7.3.3 paso 5).
    let hashes_apply = element || list.iter().any(|t| is_keyword(t, "'unsafe-hashes'"));
    if hashes_apply {
        for (bits, value) in list.iter().filter_map(|t| hash_of(t)) {
            let digest = crate::websocket::base64(&sha2(bits, source.as_bytes()));
            if base64_normalized(value) == digest {
                return true;
            }
        }
    }
    false
}

impl Policy {
    /// La directiva que se aplica a `effective` en esta política (§6.8.3), si hay.
    fn directive_for(&self, effective: &str) -> Option<&Directive> {
        fallback_list(effective)
            .iter()
            .find_map(|name| self.directives.iter().find(|d| d.name == *name))
    }

    /// La serialización de la política (§2.2: las directivas separadas por `"; "`).
    pub fn serialize(&self) -> String {
        self.directives
            .iter()
            .map(Directive::serialize)
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn violation(&self, d: &Directive, effective: &str) -> Violation {
        Violation {
            directive: d.serialize(),
            effective: effective.to_owned(),
            policy: self.serialize(),
            disposition: self.disposition,
        }
    }

    /// §6.1 "Should request be blocked by Content Security Policy?" para esta política: una
    /// petición de `url` con `destination` desde un documento de `origin`. `nonce` es el del
    /// elemento que la hace (un `<script src>` o `<link>` con `nonce`), que la permite aunque
    /// la URL no coincida (§6.7.1.1, *pre-request check* de `script-src`/`style-src`).
    pub fn request_violation(
        &self,
        url: &Url,
        destination: Destination,
        origin: &Url,
        nonce: Option<&str>,
    ) -> Option<Violation> {
        self.request_violation_by(url, destination, origin, nonce, true)
    }

    /// Como [`Policy::request_violation`]; `parser_inserted` es falso para un `<script>` que
    /// insertó otro script: con `'strict-dynamic'` (§8.2) la confianza se propaga y se permite.
    pub fn request_violation_by(
        &self,
        url: &Url,
        destination: Destination,
        origin: &Url,
        nonce: Option<&str>,
        parser_inserted: bool,
    ) -> Option<Violation> {
        let effective = effective_directive(destination)?;
        let d = self.directive_for(effective)?;
        if matches!(destination, Destination::Script | Destination::Style)
            && let Some(n) = nonce.filter(|n| !n.is_empty())
            && d.value.iter().any(|t| nonce_of(t) == Some(n))
        {
            return None;
        }
        // `'strict-dynamic'` (§8.2): las fuentes de host y de esquema no cuentan para los
        // scripts que insertó el parser.
        if destination == Destination::Script
            && d.value.iter().any(|t| is_keyword(t, "'strict-dynamic'"))
        {
            return parser_inserted.then(|| self.violation(d, effective));
        }
        (!matches_source_list(url, &d.value, origin)).then(|| self.violation(d, effective))
    }

    /// ¿Esta política impide el script o estilo en línea `source` con el `nonce` de su
    /// elemento?
    pub fn inline_violation(
        &self,
        kind: Inline,
        nonce: Option<&str>,
        source: &str,
    ) -> Option<Violation> {
        self.inline_violation_by(kind, nonce, source, true)
    }

    /// Como [`Policy::inline_violation`]; `parser_inserted` es falso para un `<script>` en
    /// línea que insertó otro script. Con `'strict-dynamic'` (§8.2) ese script hereda la
    /// confianza del que lo insertó y se permite aunque no tenga nonce, como hacen los
    /// navegadores y espera WPT (`script-src/script-src-strict_dynamic_meta_tag.html`).
    pub fn inline_violation_by(
        &self,
        kind: Inline,
        nonce: Option<&str>,
        source: &str,
        parser_inserted: bool,
    ) -> Option<Violation> {
        let effective = match kind {
            Inline::Script => "script-src-elem",
            Inline::ScriptAttribute => "script-src-attr",
            Inline::Style => "style-src-elem",
            Inline::StyleAttribute => "style-src-attr",
        };
        let d = self.directive_for(effective)?;
        if kind == Inline::Script
            && !parser_inserted
            && d.value.iter().any(|t| is_keyword(t, "'strict-dynamic'"))
        {
            return None;
        }
        (!inline_allowed(&d.value, kind, nonce, source)).then(|| self.violation(d, effective))
    }

    /// §4.4: ¿esta política impide compilar texto como código (`eval`, `new Function`,
    /// `setTimeout` con texto)? Solo lo permite `'unsafe-eval'` en `script-src` (o en
    /// `default-src`).
    pub fn eval_violation(&self) -> Option<Violation> {
        let d = self.directive_for("script-src")?;
        let allowed = d.value.iter().any(|t| is_keyword(t, "'unsafe-eval'"));
        (!allowed).then(|| self.violation(d, "script-src"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s, None).unwrap()
    }

    fn policy(s: &str) -> Policy {
        parse_policy(s, Disposition::Enforce, Source::Header)
    }

    #[test]
    fn parsing() {
        let p =
            policy(" Script-Src 'self'  https://a.test ; img-src * ;script-src 'none'; bad_name x");
        assert_eq!(p.directives.len(), 2);
        assert_eq!(p.directives[0].name, "script-src");
        assert_eq!(p.directives[0].value, ["'self'", "https://a.test"]);
        assert_eq!(p.directives[1].serialize(), "img-src *");
        let list = parse_list("default-src 'self', img-src https:", Disposition::Report);
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].disposition, Disposition::Report);
        let meta = parse_policy(
            "script-src 'self'; frame-ancestors 'none'; report-uri /r",
            Disposition::Enforce,
            Source::Meta,
        );
        assert_eq!(meta.directives.len(), 1);
    }

    #[test]
    fn source_lists() {
        let origin = url("https://sitio.test/pagina");
        let m = |u: &str, list: &[&str]| {
            let list: Vec<String> = list.iter().map(|s| (*s).to_owned()).collect();
            matches_source_list(&url(u), &list, &origin)
        };
        let table: &[(&str, &[&str], bool)] = &[
            ("https://sitio.test/a.js", &["'self'"], true),
            ("https://otro.test/a.js", &["'self'"], false),
            ("https://sitio.test:8443/a.js", &["'self'"], false),
            ("https://sitio.test/a.js", &["'none'"], false),
            ("https://sitio.test/a.js", &[], false),
            ("https://cdn.test/a.js", &["*"], true),
            ("data:text/javascript,1", &["*"], false),
            ("data:text/javascript,1", &["data:"], true),
            ("https://x.test/", &["https:"], true),
            ("http://x.test/", &["https:"], false),
            ("https://x.test/", &["http:"], true),
            ("https://a.cdn.test/x", &["*.cdn.test"], true),
            ("https://cdn.test/x", &["*.cdn.test"], false),
            ("https://a.b.cdn.test/x", &["https://*.cdn.test"], true),
            ("http://a.cdn.test/x", &["*.cdn.test"], false),
            ("https://cdn.test:444/x", &["cdn.test"], false),
            ("https://cdn.test:444/x", &["cdn.test:444"], true),
            ("https://cdn.test:444/x", &["cdn.test:*"], true),
            ("https://cdn.test/lib/a.js", &["cdn.test/lib/"], true),
            ("https://cdn.test/otra/a.js", &["cdn.test/lib/"], false),
            ("https://cdn.test/lib/a.js", &["cdn.test/lib/a.js"], true),
            ("https://cdn.test/lib/b.js", &["cdn.test/lib/a.js"], false),
            ("https://CDN.test/x", &["https://cdn.TEST"], true),
            (
                "https://x.test/",
                &["'unsafe-inline'", "'nonce-abc'"],
                false,
            ),
        ];
        for (u, list, want) in table {
            assert_eq!(m(u, list), *want, "{u} {list:?}");
        }
        // `'self'` desde `http:` admite el mismo host por `https:`.
        let http = url("http://sitio.test/");
        assert!(matches_source_list(
            &url("https://sitio.test/a"),
            &["'self'".into()],
            &http
        ));
    }

    #[test]
    fn requests_and_fallbacks() {
        let origin = url("https://sitio.test/");
        let p = policy("default-src 'self'; img-src *; script-src 'self' 'nonce-n1'");
        let v = |u: &str, d, n| p.request_violation(&url(u), d, &origin, n);
        assert!(v("https://sitio.test/a.js", Destination::Script, None).is_none());
        let blocked = v("https://otro.test/a.js", Destination::Script, None).unwrap();
        assert_eq!(blocked.directive, "script-src 'self' 'nonce-n1'");
        assert!(v("https://otro.test/a.js", Destination::Script, Some("n1")).is_none());
        assert!(v("https://otro.test/a.png", Destination::Image, None).is_none());
        // Sin `font-src` ni `connect-src`: `default-src`.
        assert_eq!(
            v("https://otro.test/f.woff", Destination::Font, None)
                .unwrap()
                .directive,
            "default-src 'self'"
        );
        assert!(v("https://otro.test/api", Destination::Connect, None).is_some());
        // `frame-src` cae en `child-src` antes que en `default-src`.
        let p = policy("default-src 'none'; child-src https://marcos.test");
        assert!(
            p.request_violation(
                &url("https://marcos.test/x"),
                Destination::Frame,
                &origin,
                None
            )
            .is_none()
        );
        // `'strict-dynamic'`: los scripts del parser necesitan nonce; los que inserta un
        // script, no.
        let p = policy("script-src 'nonce-n' 'strict-dynamic' https://cdn.test");
        let cdn = url("https://cdn.test/a.js");
        assert!(
            p.request_violation(&cdn, Destination::Script, &origin, None)
                .is_some()
        );
        assert!(
            p.request_violation(&cdn, Destination::Script, &origin, Some("n"))
                .is_none()
        );
        assert!(
            p.request_violation_by(&cdn, Destination::Script, &origin, None, false)
                .is_none()
        );
        // Sin directiva aplicable, todo vale.
        let p = policy("img-src 'none'");
        assert!(
            p.request_violation(
                &url("https://x.test/a.js"),
                Destination::Script,
                &origin,
                None
            )
            .is_none()
        );
    }

    #[test]
    fn inline_scripts_and_styles() {
        let src = "alert(1)";
        let h256 = crate::websocket::base64(&sha2(256, src.as_bytes()));
        let h512 = crate::websocket::base64(&sha2(512, src.as_bytes()));
        let table: &[(&str, Inline, Option<&str>, &str, bool)] = &[
            ("script-src 'self'", Inline::Script, None, src, false),
            (
                "script-src 'unsafe-inline'",
                Inline::Script,
                None,
                src,
                true,
            ),
            (
                "script-src 'unsafe-inline' 'nonce-a'",
                Inline::Script,
                None,
                src,
                false,
            ),
            ("script-src 'nonce-a'", Inline::Script, Some("a"), src, true),
            (
                "script-src 'nonce-a'",
                Inline::Script,
                Some("b"),
                src,
                false,
            ),
            (
                "script-src 'unsafe-inline' 'strict-dynamic'",
                Inline::Script,
                None,
                src,
                false,
            ),
            ("default-src 'self'", Inline::Style, None, "p{}", false),
            (
                "style-src 'unsafe-inline'",
                Inline::Style,
                None,
                "p{}",
                true,
            ),
            (
                "script-src 'nonce-a'",
                Inline::ScriptAttribute,
                Some("a"),
                src,
                false,
            ),
            ("img-src 'none'", Inline::Script, None, src, true),
        ];
        for (p, kind, nonce, source, allowed) in table {
            let v = policy(p).inline_violation(*kind, *nonce, source);
            assert_eq!(v.is_none(), *allowed, "{p} {kind:?} {nonce:?}");
        }
        // `'strict-dynamic'`: el script en línea que insertó otro script corre sin nonce; el
        // del parser, no.
        let strict = policy("script-src 'nonce-a' 'strict-dynamic'");
        assert!(
            strict
                .inline_violation_by(Inline::Script, None, src, false)
                .is_none()
        );
        assert!(
            strict
                .inline_violation_by(Inline::Script, Some("x"), src, true)
                .is_some()
        );
        assert!(
            strict
                .inline_violation_by(Inline::ScriptAttribute, None, src, false)
                .is_some()
        );
        let with_hash = policy(&format!("script-src 'sha256-{h256}'"));
        assert!(
            with_hash
                .inline_violation(Inline::Script, None, src)
                .is_none()
        );
        assert!(
            with_hash
                .inline_violation(Inline::Script, None, "alert(2)")
                .is_some()
        );
        let url_safe = h512.replace('+', "-").replace('/', "_");
        assert!(
            policy(&format!("script-src 'sha512-{url_safe}'"))
                .inline_violation(Inline::Script, None, src)
                .is_none()
        );
        // Los atributos solo aceptan hashes con `'unsafe-hashes'`.
        let attr = policy(&format!("script-src 'sha256-{h256}'"));
        assert!(
            attr.inline_violation(Inline::ScriptAttribute, None, src)
                .is_some()
        );
        let attr = policy(&format!("script-src 'unsafe-hashes' 'sha256-{h256}'"));
        assert!(
            attr.inline_violation(Inline::ScriptAttribute, None, src)
                .is_none()
        );
    }

    #[test]
    fn eval() {
        assert!(policy("script-src 'self'").eval_violation().is_some());
        assert!(
            policy("script-src 'self' 'unsafe-eval'")
                .eval_violation()
                .is_none()
        );
        assert!(policy("default-src 'self'").eval_violation().is_some());
        assert!(policy("img-src 'none'").eval_violation().is_none());
    }

    #[test]
    fn sha2_vectors() {
        // FIPS 180-4, ejemplos con "abc".
        let hex = |b: Vec<u8>| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        assert_eq!(
            hex(sha2(384, b"abc")),
            "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed\
             8086072ba1e7cc2358baeca134c825a7"
        );
        assert_eq!(
            hex(sha2(512, b"abc")),
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
             2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
        assert_eq!(hex(sha2(512, b"")).len(), 128);
    }
}
