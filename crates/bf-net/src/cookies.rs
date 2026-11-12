//! Cookies (RFC 6265bis): parser de `Set-Cookie` (§5.6), modelo de almacenamiento (§5.7) y la
//! cabecera `Cookie` (§5.8.3). Jarra en memoria; con [`CookieJar::on_disk`], las cookies
//! persistentes se guardan en un archivo (E6).
//!
//! E6: la lista de sufijos públicos ([`crate::psl`]) da el sitio ([`site_of`]) y rechaza
//! `Domain=` con un sufijo público (§5.7 paso 9).
//!
//! E4: `SameSite` (§5.2, §5.7 y §5.8.3) y el bloqueo de cookies de terceros. El "site for
//! cookies" de una petición es el del documento de nivel superior: el hilo del documento lo
//! declara con [`set_thread_context`] y lo usan las peticiones que salen de ese hilo (el
//! documento, sus subrecursos, sus marcos y `document.cookie`). Una petición sin contexto
//! (otro hilo, la línea de comandos antes de cargar) se trata como del mismo sitio.

use std::cell::RefCell;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use bf_url::{Host, Url};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameSite {
    Strict,
    Lax,
    None,
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub host_only: bool,
    pub path: String,
    /// Segundos Unix; `None` = cookie de sesión.
    pub expires: Option<i64>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: SameSite,
    creation: u64,
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Días desde 1970-01-01 (algoritmo de Howard Hinnant).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Algoritmo de parseo de fechas de cookies (RFC 6265bis §5.1.1).
pub fn parse_cookie_date(input: &str) -> Option<i64> {
    let is_delimiter = |c: char| matches!(c, '\t' | ' '..='/' | ';'..='@' | '['..='`' | '{'..='~');
    let (mut time, mut day, mut month, mut year) = (None, None, None, None);
    for token in input.split(is_delimiter).filter(|t| !t.is_empty()) {
        let digits = |s: &str, min: usize, max: usize| -> Option<(i64, usize)> {
            let n = s.chars().take_while(char::is_ascii_digit).count();
            if n < min || n > max {
                return None;
            }
            Some((s[..n].parse().ok()?, n))
        };
        if time.is_none() {
            let parts: Vec<&str> = token.split(':').collect();
            if parts.len() == 3
                && let (Some((h, _)), Some((m, _)), Some((s, _))) = (
                    digits(parts[0], 1, 2),
                    digits(parts[1], 1, 2),
                    digits(parts[2], 1, 2),
                )
            {
                time = Some((h, m, s));
                continue;
            }
        }
        if day.is_none()
            && let Some((d, n)) = digits(token, 1, 2)
            && !token[n..].starts_with(|c: char| c.is_ascii_digit())
        {
            day = Some(d);
            continue;
        }
        if month.is_none() && token.len() >= 3 {
            let months = [
                "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
            ];
            if let Some(i) = months
                .iter()
                .position(|m| token[..3].eq_ignore_ascii_case(m))
            {
                month = Some(i as i64 + 1);
                continue;
            }
        }
        if year.is_none()
            && let Some((y, n)) = digits(token, 2, 4)
            && !token[n..].starts_with(|c: char| c.is_ascii_digit())
        {
            year = Some(y);
        }
    }
    let (h, mi, s) = time?;
    let (d, m) = (day?, month?);
    let mut y = year?;
    if (70..=99).contains(&y) {
        y += 1900;
    } else if (0..=69).contains(&y) {
        y += 2000;
    }
    if !(1..=31).contains(&d) || y < 1601 || h > 23 || mi > 59 || s > 59 {
        return None;
    }
    Some(days_from_civil(y, m, d) * 86_400 + h * 3600 + mi * 60 + s)
}

/// Default-path (§5.1.4).
fn default_path(url: &Url) -> String {
    let path = url.path_string();
    if !path.starts_with('/') {
        return "/".into();
    }
    match path.rfind('/') {
        Some(0) | None => "/".into(),
        Some(i) => path[..i].to_owned(),
    }
}

/// Domain-match (§5.1.3).
fn domain_match(host: &str, domain: &str, host_is_ip: bool) -> bool {
    host == domain
        || (!host_is_ip
            && host.len() > domain.len()
            && host.ends_with(domain)
            && host.as_bytes()[host.len() - domain.len() - 1] == b'.')
}

/// Path-match (§5.1.4).
fn path_match(request: &str, cookie: &str) -> bool {
    request == cookie
        || (request.starts_with(cookie)
            && (cookie.ends_with('/') || request[cookie.len()..].starts_with('/')))
}

/// El contexto de las peticiones de un hilo para `SameSite` (RFC 6265bis §5.2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CookieContext {
    /// El "site for cookies": el sitio (dominio registrable) del documento de nivel superior.
    pub site_for_cookies: Option<String>,
    /// ¿Es una navegación de nivel superior (la de ese documento)? Las cookies `Lax` viajan
    /// en las navegaciones de nivel superior aunque sean entre sitios.
    pub top_level_navigation: bool,
}

thread_local! {
    static CONTEXT: RefCell<Option<CookieContext>> = const { RefCell::new(None) };
}

/// Declara el contexto de las peticiones que salgan de este hilo (`None`: sin contexto).
pub fn set_thread_context(ctx: Option<CookieContext>) {
    CONTEXT.with(|c| *c.borrow_mut() = ctx);
}

/// El contexto declarado en este hilo.
pub fn thread_context() -> Option<CookieContext> {
    CONTEXT.with(|c| c.borrow().clone())
}

/// El sitio de `url` (HTML §7.1.1 "obtain a site", sin esquema): el dominio registrable según
/// la lista de sufijos públicos (E6, [`crate::psl`]), o el host entero si no tiene (un sufijo
/// público, `localhost`). Las IP son su propio sitio.
pub fn site_of(url: &Url) -> Option<String> {
    let host = match url.host.as_ref()? {
        Host::Domain(d) => d.trim_end_matches('.').to_ascii_lowercase(),
        h @ (Host::Ipv4(_) | Host::Ipv6(_)) => return Some(h.to_string()),
        _ => return None,
    };
    Some(crate::psl::registrable_domain(&host).unwrap_or(host))
}

/// ¿Una petición a `url` es del mismo sitio que el contexto? (§5.2: "same-site" si el dominio
/// registrable del destino es el del "site for cookies".)
fn same_site_request(url: &Url, ctx: Option<&CookieContext>) -> bool {
    match ctx.and_then(|c| c.site_for_cookies.as_deref()) {
        None => true,
        Some(site) => site_of(url).as_deref() == Some(site),
    }
}

#[derive(Debug, Default)]
pub struct CookieJar {
    cookies: Mutex<Vec<Cookie>>,
    counter: Mutex<u64>,
    /// No mandar ni guardar cookies en peticiones entre sitios (opción del navegador; por
    /// defecto `false`).
    block_third_party: AtomicBool,
    /// E6: dónde se guardan las cookies persistentes ([`CookieJar::on_disk`]).
    file: Option<PathBuf>,
}

impl CookieJar {
    fn host_of(url: &Url) -> Option<(String, bool)> {
        match url.host.as_ref()? {
            Host::Domain(d) => Some((d.clone(), false)),
            h @ (Host::Ipv4(_) | Host::Ipv6(_)) => Some((h.to_string(), true)),
            _ => None,
        }
    }

    /// ¿Se bloquean las cookies de terceros?
    pub fn block_third_party(&self) -> bool {
        self.block_third_party.load(Ordering::Relaxed)
    }

    pub fn set_block_third_party(&self, block: bool) {
        self.block_third_party.store(block, Ordering::Relaxed);
    }

    /// ¿Se descarta `cookie`, recibida para `url` en el contexto del hilo? (§5.7: las de
    /// `same-site-flag` distinto de "None" que llegan entre sitios fuera de una navegación de
    /// nivel superior; el bloqueo de terceros es una política del navegador, §7.1.)
    fn rejected_by_site(&self, url: &Url, cookie: &Cookie) -> bool {
        let ctx = thread_context();
        if same_site_request(url, ctx.as_ref()) {
            return false;
        }
        if self.block_third_party() {
            return true;
        }
        cookie.same_site != SameSite::None && !ctx.as_ref().is_some_and(|c| c.top_level_navigation)
    }

    /// Procesa una cabecera `Set-Cookie` recibida para `url` (§5.6 y §5.7).
    pub fn set_cookie(&self, url: &Url, header: &str) {
        let Some(cookie) = self.parse(url, header, now()) else {
            return;
        };
        if self.rejected_by_site(url, &cookie) {
            return;
        }
        let mut jar = self.cookies.lock().unwrap_or_else(|e| e.into_inner());
        let mut cookie = cookie;
        let old = jar.iter().position(|c| {
            c.name == cookie.name && c.domain == cookie.domain && c.path == cookie.path
        });
        let mut persistent = cookie.expires.is_some();
        if let Some(old) = old {
            cookie.creation = jar[old].creation;
            persistent |= jar[old].expires.is_some();
            jar.remove(old);
        }
        // Una cookie ya vencida solo borra la anterior.
        if cookie.expires.is_none_or(|e| e > now()) {
            jar.push(cookie);
        }
        // E6: la copia en disco sigue a las cookies persistentes.
        if persistent && self.file.is_some() {
            let _ = self.write(&jar);
        }
    }

    fn parse(&self, url: &Url, header: &str, now: i64) -> Option<Cookie> {
        // §5.6 paso 1: caracteres de control (salvo tab) invalidan la cookie.
        if header
            .chars()
            .any(|c| (c < ' ' && c != '\t') || c == '\u{7F}')
        {
            return None;
        }
        let (pair, attrs) = header.split_once(';').unwrap_or((header, ""));
        let (name, value) = match pair.split_once('=') {
            Some((n, v)) => (n.trim(), v.trim()),
            None => ("", pair.trim()),
        };
        if (name.is_empty() && value.is_empty()) || name.len() + value.len() > 4096 {
            return None;
        }
        let (host, is_ip) = Self::host_of(url)?;
        let secure_origin = url.scheme == "https";
        let mut cookie = Cookie {
            name: name.to_owned(),
            value: value.to_owned(),
            domain: host.clone(),
            host_only: true,
            path: default_path(url),
            expires: None,
            secure: false,
            http_only: false,
            same_site: SameSite::Default,
            creation: {
                let mut c = self.counter.lock().unwrap_or_else(|e| e.into_inner());
                *c += 1;
                *c
            },
        };
        let (mut max_age, mut expires, mut domain_attr) = (None, None, None);
        for attr in attrs.split(';') {
            let (k, v) = attr.split_once('=').unwrap_or((attr, ""));
            let (k, v) = (k.trim(), v.trim());
            if v.len() > 1024 {
                continue;
            }
            match k.to_ascii_lowercase().as_str() {
                "expires" => expires = parse_cookie_date(v).or(expires),
                "max-age" => {
                    let ok = !v.is_empty()
                        && v.trim_start_matches('-')
                            .bytes()
                            .all(|b| b.is_ascii_digit());
                    if ok {
                        let n: i64 = v.parse().unwrap_or(if v.starts_with('-') {
                            i64::MIN
                        } else {
                            i64::MAX
                        });
                        max_age = Some(if n <= 0 {
                            i64::MIN
                        } else {
                            now.saturating_add(n)
                        });
                    }
                }
                "domain" if !v.is_empty() => {
                    domain_attr = Some(v.trim_start_matches('.').to_ascii_lowercase());
                }
                "path" => {
                    cookie.path = if v.starts_with('/') {
                        v.to_owned()
                    } else {
                        default_path(url)
                    };
                }
                "secure" => cookie.secure = true,
                "httponly" => cookie.http_only = true,
                "samesite" => {
                    cookie.same_site = match v.to_ascii_lowercase().as_str() {
                        "strict" => SameSite::Strict,
                        "lax" => SameSite::Lax,
                        "none" => SameSite::None,
                        _ => SameSite::Default,
                    };
                }
                _ => {}
            }
        }
        cookie.expires = max_age.or(expires);
        // §5.7 paso 9 (E6): un `Domain` que es un sufijo público solo vale si es el propio host
        // (y entonces la cookie es *host-only*); si no, la cookie se ignora.
        if let Some(d) = &domain_attr
            && !is_ip
            && crate::psl::is_public_suffix(d)
        {
            if *d != host.to_ascii_lowercase() {
                return None;
            }
            domain_attr = None;
        }
        if let Some(d) = domain_attr {
            if !domain_match(&host, &d, is_ip) {
                return None;
            }
            cookie.domain = d;
            cookie.host_only = false;
        }
        // §5.7: una cookie Secure solo desde un origen seguro; prefijos __Secure- y __Host-.
        if cookie.secure && !secure_origin {
            return None;
        }
        let lower = cookie.name.to_ascii_lowercase();
        if lower.starts_with("__secure-") && !cookie.secure {
            return None;
        }
        if lower.starts_with("__host-")
            && (!cookie.secure || !cookie.host_only || cookie.path != "/")
        {
            return None;
        }
        if cookie.same_site == SameSite::None && !cookie.secure {
            return None;
        }
        Some(cookie)
    }

    /// `document.cookie` (HTML §3.1.3; RFC 6265 §5.8.3 con la API "non-HTTP"): las cookies
    /// de `url` sin las `HttpOnly`.
    pub fn script_cookies(&self, url: &Url) -> String {
        self.cookie_string(url, false).unwrap_or_default()
    }

    /// `document.cookie = value`: una cookie puesta desde script (RFC 6265 §5.7 con la API
    /// "non-HTTP"): no puede ser `HttpOnly` ni pisar una cookie `HttpOnly`.
    pub fn set_cookie_from_script(&self, url: &Url, value: &str) {
        let Some(cookie) = self.parse(url, value, now()) else {
            return;
        };
        if cookie.http_only {
            return;
        }
        {
            let jar = self.cookies.lock().unwrap_or_else(|e| e.into_inner());
            let shadows_http_only = jar.iter().any(|c| {
                c.http_only
                    && c.name == cookie.name
                    && c.domain == cookie.domain
                    && c.path == cookie.path
            });
            if shadows_http_only {
                return;
            }
        }
        self.set_cookie(url, value);
    }

    /// Valor de la cabecera `Cookie` para un request a `url` (§5.8.3).
    pub fn cookie_header(&self, url: &Url) -> Option<String> {
        self.cookie_string(url, true)
    }

    /// Las cookies que corresponden a `url`; sin `http`, se omiten las `HttpOnly`.
    fn cookie_string(&self, url: &Url, http: bool) -> Option<String> {
        let (host, is_ip) = Self::host_of(url)?;
        let path = url.path_string();
        let secure = url.scheme == "https";
        let ctx = thread_context();
        let same_site = same_site_request(url, ctx.as_ref());
        if !same_site && self.block_third_party() {
            return None;
        }
        // §5.8.3 paso 1: entre sitios, `Strict` nunca viaja; `Lax` (y sin atributo, que se
        // aplica como `Lax`) solo en una navegación de nivel superior por HTTP.
        let lax_ok = http && ctx.as_ref().is_some_and(|c| c.top_level_navigation);
        let t = now();
        let mut jar = self.cookies.lock().unwrap_or_else(|e| e.into_inner());
        jar.retain(|c| c.expires.is_none_or(|e| e > t));
        let mut matching: Vec<&Cookie> = jar
            .iter()
            .filter(|c| {
                let domain_ok = if c.host_only {
                    host == c.domain
                } else {
                    domain_match(&host, &c.domain, is_ip)
                };
                domain_ok
                    && path_match(&path, &c.path)
                    && (!c.secure || secure)
                    && (http || !c.http_only)
                    && (same_site
                        || match c.same_site {
                            SameSite::None => true,
                            SameSite::Strict => false,
                            SameSite::Lax | SameSite::Default => lax_ok,
                        })
            })
            .collect();
        if matching.is_empty() {
            return None;
        }
        matching.sort_by(|a, b| {
            b.path
                .len()
                .cmp(&a.path.len())
                .then(a.creation.cmp(&b.creation))
        });
        let parts: Vec<String> = matching
            .iter()
            .map(|c| {
                if c.name.is_empty() {
                    c.value.clone()
                } else {
                    format!("{}={}", c.name, c.value)
                }
            })
            .collect();
        Some(parts.join("; "))
    }

    /// E6: una jarra que carga las cookies persistentes de `file` (si existe) y las guarda ahí
    /// cada vez que cambia una (§5.7: las de sesión no se guardan; las vencidas no se cargan).
    pub fn on_disk(file: impl Into<PathBuf>) -> CookieJar {
        let file = file.into();
        let mut cookies = Vec::new();
        if let Ok(text) = std::fs::read_to_string(&file) {
            let t = now();
            cookies = text
                .lines()
                .filter_map(decode_line)
                .filter(|c| c.expires.is_some_and(|e| e > t))
                .collect();
            cookies.sort_by_key(|c| c.creation);
        }
        let counter = cookies.iter().map(|c| c.creation).max().unwrap_or(0);
        CookieJar {
            cookies: Mutex::new(cookies),
            counter: Mutex::new(counter),
            block_third_party: AtomicBool::new(false),
            file: Some(file),
        }
    }

    /// E6: guarda ya las cookies persistentes (si la jarra tiene archivo).
    pub fn save(&self) -> io::Result<()> {
        let jar = self.cookies.lock().unwrap_or_else(|e| e.into_inner());
        self.write(&jar)
    }

    fn write(&self, jar: &[Cookie]) -> io::Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        let t = now();
        let mut text = String::new();
        for c in jar.iter().filter(|c| c.expires.is_some_and(|e| e > t)) {
            text.push_str(&encode_line(c));
            text.push('\n');
        }
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = file.with_extension("tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, file)
    }

    pub fn len(&self) -> usize {
        self.cookies.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// E6: una cookie por línea, campos separados por tabuladores y escapados con `%XX`.
fn encode_line(c: &Cookie) -> String {
    let same_site = match c.same_site {
        SameSite::Strict => "strict",
        SameSite::Lax => "lax",
        SameSite::None => "none",
        SameSite::Default => "default",
    };
    [
        escape(&c.name),
        escape(&c.value),
        escape(&c.domain),
        u8::from(c.host_only).to_string(),
        escape(&c.path),
        c.expires.unwrap_or(0).to_string(),
        u8::from(c.secure).to_string(),
        u8::from(c.http_only).to_string(),
        same_site.to_owned(),
        c.creation.to_string(),
    ]
    .join("\t")
}

fn decode_line(line: &str) -> Option<Cookie> {
    let f: Vec<&str> = line.split('\t').collect();
    let [
        name,
        value,
        domain,
        host_only,
        path,
        expires,
        secure,
        http_only,
        same_site,
        creation,
    ] = f[..]
    else {
        return None;
    };
    Some(Cookie {
        name: unescape(name)?,
        value: unescape(value)?,
        domain: unescape(domain)?,
        host_only: host_only == "1",
        path: unescape(path)?,
        expires: Some(expires.parse().ok()?),
        secure: secure == "1",
        http_only: http_only == "1",
        same_site: match same_site {
            "strict" => SameSite::Strict,
            "lax" => SameSite::Lax,
            "none" => SameSite::None,
            _ => SameSite::Default,
        },
        creation: creation.parse().ok()?,
    })
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '%' | '\t' | '\r' | '\n' => out.push_str(&format!("%{:02X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn unescape(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            out.push(u8::from_str_radix(s.get(i + 1..i + 3)?, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_site_and_third_party() {
        // RFC 6265bis §5.2 y §5.8.3: desde a.example, b.test es otro sitio.
        let jar = CookieJar::default();
        let b = url("https://b.test/x");
        jar.set_cookie(&b, "n=1; SameSite=None; Secure");
        jar.set_cookie(&b, "l=2; SameSite=Lax");
        jar.set_cookie(&b, "s=3; SameSite=Strict");
        jar.set_cookie(&b, "d=4");
        let a = site_of(&url("https://www.a.example/"));
        set_thread_context(Some(CookieContext {
            site_for_cookies: a.clone(),
            top_level_navigation: false,
        }));
        assert_eq!(jar.cookie_header(&b).as_deref(), Some("n=1"));
        assert_eq!(jar.script_cookies(&b), "n=1");
        // Una cookie `Lax` que llega en una petición entre sitios no se guarda; `None` sí.
        jar.set_cookie(&b, "l2=5");
        jar.set_cookie(&b, "n2=6; SameSite=None; Secure");
        assert_eq!(jar.cookie_header(&b).as_deref(), Some("n=1; n2=6"));
        // Navegación de nivel superior: `Lax` viaja; `Strict` no.
        set_thread_context(Some(CookieContext {
            site_for_cookies: a,
            top_level_navigation: true,
        }));
        assert_eq!(
            jar.cookie_header(&b).as_deref(),
            Some("n=1; l=2; d=4; n2=6")
        );
        // Bloqueo de terceros: nada entre sitios; el mismo sitio no cambia.
        jar.set_block_third_party(true);
        assert_eq!(jar.cookie_header(&b), None);
        set_thread_context(Some(CookieContext {
            site_for_cookies: site_of(&url("https://sub.b.test/")),
            top_level_navigation: false,
        }));
        assert_eq!(
            jar.cookie_header(&b).as_deref(),
            Some("n=1; l=2; s=3; d=4; n2=6")
        );
        set_thread_context(None);
        assert_eq!(
            site_of(&url("https://a.b.example.co.uk/")).as_deref(),
            Some("example.co.uk")
        );
        assert_eq!(
            site_of(&url("http://127.0.0.1:8000/")).as_deref(),
            Some("127.0.0.1")
        );
    }

    fn url(s: &str) -> Url {
        Url::parse(s, None).unwrap()
    }

    #[test]
    fn dates() {
        assert_eq!(
            parse_cookie_date("Wed, 21 Oct 2015 07:28:00 GMT"),
            Some(1_445_412_480)
        );
        assert_eq!(
            parse_cookie_date("Sun, 06-Nov-94 08:49:37 GMT"),
            Some(784_111_777)
        );
        assert_eq!(parse_cookie_date("nonsense"), None);
    }

    #[test]
    fn jar() {
        let jar = CookieJar::default();
        let u = url("https://www.example.com/a/b/c");
        jar.set_cookie(&u, "a=1");
        jar.set_cookie(
            &u,
            "b=2; Path=/; Domain=.example.com; Secure; HttpOnly; SameSite=Lax",
        );
        jar.set_cookie(&u, "c=3; Domain=other.com");
        jar.set_cookie(&url("http://www.example.com/"), "d=4; Secure");
        jar.set_cookie(&u, "__Host-e=5; Secure; Path=/");
        jar.set_cookie(&u, "__Host-f=6; Secure; Path=/x");
        assert_eq!(jar.len(), 3);
        assert_eq!(
            jar.cookie_header(&url("https://www.example.com/a/b/z"))
                .as_deref(),
            Some("a=1; b=2; __Host-e=5")
        );
        assert_eq!(
            jar.cookie_header(&url("http://sub.example.com/"))
                .as_deref(),
            None,
            "b es Secure"
        );
        assert_eq!(
            jar.cookie_header(&url("https://sub.example.com/"))
                .as_deref(),
            Some("b=2")
        );
        // Max-Age=0 borra.
        jar.set_cookie(&u, "a=1; Max-Age=0");
        assert_eq!(jar.len(), 2);
        jar.set_cookie(
            &u,
            "b=9; Path=/; Domain=example.com; Expires=Thu, 01 Jan 2099 00:00:00 GMT",
        );
        assert!(jar.cookie_header(&u).unwrap().contains("b=9"));
    }

    /// E6: RFC 6265bis §5.7 paso 9 con la lista de sufijos públicos.
    #[test]
    fn public_suffix_domains() {
        let jar = CookieJar::default();
        jar.set_cookie(&url("https://www.example.co.uk/"), "a=1; Domain=co.uk");
        jar.set_cookie(&url("https://www.example.co.uk/"), "b=2; Domain=.UK");
        assert!(jar.is_empty());
        jar.set_cookie(
            &url("https://www.example.co.uk/"),
            "c=3; Domain=example.co.uk",
        );
        assert_eq!(jar.len(), 1);
        // El propio host como Domain: vale, pero solo para ese host.
        let gh = url("https://github.io/");
        jar.set_cookie(&gh, "d=4; Domain=github.io");
        assert_eq!(jar.cookie_header(&gh).as_deref(), Some("d=4"));
        assert_eq!(jar.cookie_header(&url("https://x.github.io/")), None);
        jar.set_cookie(&url("https://x.github.io/"), "e=5; Domain=github.io");
        assert_eq!(jar.cookie_header(&url("https://y.github.io/")), None);
        assert_eq!(
            site_of(&url("https://x.github.io/")).as_deref(),
            Some("x.github.io")
        );
    }

    /// E6: las cookies persistentes sobreviven en disco; las de sesión no.
    #[test]
    fn persists_to_a_file() {
        let dir = std::env::temp_dir().join(format!("bf-cookies-{}", std::process::id()));
        let file = dir.join("cookies.txt");
        let _ = std::fs::remove_file(&file);
        let u = url("https://www.example.com/a/");
        {
            let jar = CookieJar::on_disk(&file);
            jar.set_cookie(&u, "s=session");
            jar.set_cookie(&u, "p=1; Max-Age=1000; HttpOnly; SameSite=Strict; Path=/");
            jar.set_cookie(&u, "q=a%3Bb c; Max-Age=1000; Domain=example.com; Secure");
        }
        let jar = CookieJar::on_disk(&file);
        assert_eq!(jar.len(), 2);
        assert_eq!(
            jar.cookie_header(&url("https://sub.example.com/a/"))
                .as_deref(),
            Some("q=a%3Bb c")
        );
        assert_eq!(jar.script_cookies(&u), "q=a%3Bb c");
        assert_eq!(jar.cookie_header(&u).as_deref(), Some("q=a%3Bb c; p=1"));
        // Borrar una persistente también llega al disco.
        jar.set_cookie(&u, "p=; Max-Age=0; Path=/");
        assert_eq!(CookieJar::on_disk(&file).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
