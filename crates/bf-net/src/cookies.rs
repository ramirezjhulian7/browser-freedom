//! Cookies (RFC 6265bis): parser de `Set-Cookie` (§5.6), modelo de almacenamiento (§5.7) y la
//! cabecera `Cookie` (§5.8.3). Jarra en memoria, sin persistencia ni lista de sufijos públicos.

use std::sync::Mutex;
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

#[derive(Debug, Default)]
pub struct CookieJar {
    cookies: Mutex<Vec<Cookie>>,
    counter: Mutex<u64>,
}

impl CookieJar {
    fn host_of(url: &Url) -> Option<(String, bool)> {
        match url.host.as_ref()? {
            Host::Domain(d) => Some((d.clone(), false)),
            h @ (Host::Ipv4(_) | Host::Ipv6(_)) => Some((h.to_string(), true)),
            _ => None,
        }
    }

    /// Procesa una cabecera `Set-Cookie` recibida para `url` (§5.6 y §5.7).
    pub fn set_cookie(&self, url: &Url, header: &str) {
        let Some(cookie) = self.parse(url, header, now()) else {
            return;
        };
        let mut jar = self.cookies.lock().unwrap_or_else(|e| e.into_inner());
        let mut cookie = cookie;
        let old = jar.iter().position(|c| {
            c.name == cookie.name && c.domain == cookie.domain && c.path == cookie.path
        });
        if let Some(old) = old {
            cookie.creation = jar[old].creation;
            jar.remove(old);
        }
        // Una cookie ya vencida solo borra la anterior.
        if cookie.expires.is_none_or(|e| e > now()) {
            jar.push(cookie);
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

    /// Valor de la cabecera `Cookie` para un request a `url` (§5.8.3).
    pub fn cookie_header(&self, url: &Url) -> Option<String> {
        let (host, is_ip) = Self::host_of(url)?;
        let path = url.path_string();
        let secure = url.scheme == "https";
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
                domain_ok && path_match(&path, &c.path) && (!c.secure || secure)
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

    pub fn len(&self) -> usize {
        self.cookies.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
