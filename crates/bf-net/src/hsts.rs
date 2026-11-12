//! HTTP Strict Transport Security (RFC 6797).
//!
//! [`HstsStore::note`] procesa la cabecera `Strict-Transport-Security` de una respuesta segura
//! (§8.1) y [`HstsStore::upgrade`] reescribe a `https:` las URLs `http:` de los *Known HSTS
//! Hosts* (§8.3), antes de abrir la conexión (Fetch §4.1, "main fetch", paso 7). Opcionalmente
//! se guarda en un archivo de texto (una línea por host: `host expira includeSubDomains`).

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use bf_url::{Host, Url};

/// Una política válida de `Strict-Transport-Security` (§6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    pub max_age: u64,
    pub include_subdomains: bool,
}

/// §6.1: `directive *( ";" [ directive ] )`, `directive = name [ "=" value ]`, con `value`
/// token o cadena entre comillas. `max-age` es obligatoria; una directiva repetida invalida la
/// cabecera (§6.1 regla 2); las desconocidas se ignoran (regla 4). Los nombres no distinguen
/// mayúsculas.
pub fn parse(value: &str) -> Option<Policy> {
    let mut max_age = None;
    let mut include_subdomains = false;
    let mut seen: Vec<String> = Vec::new();
    for directive in value.split(';') {
        let directive = directive.trim_matches([' ', '\t']);
        if directive.is_empty() {
            continue;
        }
        let (name, val) = match directive.split_once('=') {
            Some((n, v)) => (
                n.trim_matches([' ', '\t']),
                Some(v.trim_matches([' ', '\t'])),
            ),
            None => (directive, None),
        };
        if name.is_empty() || !name.bytes().all(is_tchar) {
            return None;
        }
        let name = name.to_ascii_lowercase();
        if seen.contains(&name) {
            return None;
        }
        seen.push(name.clone());
        let val = match val {
            Some(v) if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') => {
                Some(unquote(&v[1..v.len() - 1])?)
            }
            Some(v) if v.bytes().all(is_tchar) && !v.is_empty() => Some(v.to_owned()),
            Some(_) => return None,
            None => None,
        };
        match name.as_str() {
            // §6.1.1: `max-age = delta-seconds` (1*DIGIT); un valor enorme se recorta.
            "max-age" => {
                let v = val?;
                if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                max_age = Some(v.parse::<u64>().unwrap_or(u64::MAX));
            }
            // §6.1.2: sin valor.
            "includesubdomains" => {
                if val.is_some() {
                    return None;
                }
                include_subdomains = true;
            }
            _ => {}
        }
    }
    Some(Policy {
        max_age: max_age?,
        include_subdomains,
    })
}

fn is_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

/// RFC 9110 §5.6.4: `quoted-string` con `quoted-pair`.
fn unquote(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push(chars.next()?),
            '"' => return None,
            c => out.push(c),
        }
    }
    Some(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    /// Segundos desde la época Unix.
    expires: i64,
    include_subdomains: bool,
}

/// La lista de *Known HSTS Hosts* de un agente (§5.4).
#[derive(Default)]
pub struct HstsStore {
    hosts: Mutex<HashMap<String, Entry>>,
    file: Option<PathBuf>,
}

/// §8.1.1 y §8.3: el host como dominio (ASCII en minúsculas, sin el punto final), o `None` si
/// es una IP o no hay host.
fn domain(url: &Url) -> Option<String> {
    match url.host.as_ref()? {
        Host::Domain(d) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            (!d.is_empty()).then_some(d)
        }
        _ => None,
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl HstsStore {
    pub fn new() -> HstsStore {
        HstsStore::default()
    }

    /// Una lista que se lee de `file` (si existe) y se guarda ahí cada vez que cambia.
    pub fn on_disk(file: impl Into<PathBuf>) -> HstsStore {
        let file = file.into();
        let mut hosts = HashMap::new();
        if let Ok(text) = std::fs::read_to_string(&file) {
            let t = now();
            for line in text.lines() {
                let mut parts = line.split(' ');
                let (Some(host), Some(expires), sub) = (parts.next(), parts.next(), parts.next())
                else {
                    continue;
                };
                let Ok(expires) = expires.parse::<i64>() else {
                    continue;
                };
                if expires > t && !host.is_empty() {
                    hosts.insert(
                        host.to_owned(),
                        Entry {
                            expires,
                            include_subdomains: sub == Some("1"),
                        },
                    );
                }
            }
        }
        HstsStore {
            hosts: Mutex::new(hosts),
            file: Some(file),
        }
    }

    fn save(&self, hosts: &HashMap<String, Entry>) -> io::Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        let mut text = String::new();
        let mut list: Vec<_> = hosts.iter().collect();
        list.sort_by(|a, b| a.0.cmp(b.0));
        for (host, e) in list {
            text.push_str(&format!(
                "{host} {} {}\n",
                e.expires,
                u8::from(e.include_subdomains)
            ));
        }
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = file.with_extension("tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, file)
    }

    /// §8.1: procesa la cabecera `Strict-Transport-Security` (la primera, si hay varias) de una
    /// respuesta a `url`. Solo cuenta sobre transporte seguro sin errores (§8.1, §7.2) y con un
    /// host que no sea una IP (§8.1.1). `max-age=0` borra el host (§6.1.1).
    pub fn note(&self, url: &Url, header: &str) {
        if url.scheme != "https" {
            return;
        }
        let (Some(host), Some(policy)) = (domain(url), parse(header)) else {
            return;
        };
        let mut hosts = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        let before = hosts.get(&host).copied();
        if policy.max_age == 0 {
            hosts.remove(&host);
        } else {
            let expires = now().saturating_add(i64::try_from(policy.max_age).unwrap_or(i64::MAX));
            hosts.insert(
                host.clone(),
                Entry {
                    expires,
                    include_subdomains: policy.include_subdomains,
                },
            );
        }
        if hosts.get(&host).copied() != before {
            let _ = self.save(&hosts);
        }
    }

    /// §8.2: ¿es `host` un *Known HSTS Host*? Coincidencia exacta (*congruent match*) o un
    /// superdominio con `includeSubDomains` (*superdomain match*). Los caducados no cuentan.
    pub fn is_known(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        let hosts = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        let t = now();
        let live = |h: &str| hosts.get(h).filter(|e| e.expires > t);
        if live(&host).is_some() {
            return true;
        }
        let mut rest = host.as_str();
        while let Some((_, parent)) = rest.split_once('.') {
            if live(parent).is_some_and(|e| e.include_subdomains) {
                return true;
            }
            rest = parent;
        }
        false
    }

    /// §8.3: si `url` es `http:` y su host es un *Known HSTS Host*, la misma URL en `https:`; el
    /// puerto 80 explícito pasa a 443 y cualquier otro se conserva (paso 5).
    pub fn upgrade(&self, url: &Url) -> Option<Url> {
        if url.scheme != "http" || !self.is_known(&domain(url)?) {
            return None;
        }
        let mut up = url.clone();
        up.scheme = "https".into();
        if up.port == Some(80) {
            up.port = None;
        }
        Some(up)
    }

    pub fn len(&self) -> usize {
        self.hosts.lock().unwrap_or_else(|e| e.into_inner()).len()
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
    fn parses_directives() {
        let p = |s| parse(s);
        assert_eq!(
            p("max-age=31536000; includeSubDomains"),
            Some(Policy {
                max_age: 31536000,
                include_subdomains: true
            })
        );
        assert_eq!(
            p("  MAX-AGE=\"10\" ;; preload; foo=bar"),
            Some(Policy {
                max_age: 10,
                include_subdomains: false
            })
        );
        assert_eq!(p("max-age=0").map(|p| p.max_age), Some(0));
        assert_eq!(p("includeSubDomains"), None);
        assert_eq!(p("max-age=1; max-age=2"), None);
        assert_eq!(p("max-age=1; includeSubDomains; includesubdomains"), None);
        assert_eq!(p("max-age=-1"), None);
        assert_eq!(p("max-age=1.5"), None);
        assert_eq!(p("max-age="), None);
        assert_eq!(p("max-age=1; includeSubDomains=1"), None);
        assert_eq!(
            p("max-age=99999999999999999999999").map(|p| p.max_age),
            Some(u64::MAX)
        );
    }

    #[test]
    fn notes_and_upgrades() {
        let s = HstsStore::new();
        // Solo sobre https y con un dominio.
        s.note(&url("http://a.example/"), "max-age=100");
        s.note(&url("https://192.0.2.1/"), "max-age=100");
        s.note(&url("https://[::1]/"), "max-age=100");
        assert!(s.is_empty());

        s.note(&url("https://A.Example./"), "max-age=100");
        assert_eq!(
            s.upgrade(&url("http://a.example/x?y#z")).unwrap().href(),
            "https://a.example/x?y#z"
        );
        assert_eq!(
            s.upgrade(&url("http://a.example:80/")).unwrap().href(),
            "https://a.example/"
        );
        assert_eq!(
            s.upgrade(&url("http://a.example:8080/")).unwrap().href(),
            "https://a.example:8080/"
        );
        assert!(s.upgrade(&url("https://a.example/")).is_none());
        assert!(s.upgrade(&url("http://b.a.example/")).is_none());
        assert!(s.upgrade(&url("http://example/")).is_none());

        s.note(&url("https://a.example/"), "max-age=100; includeSubDomains");
        assert!(s.upgrade(&url("http://b.a.example/")).is_some());
        assert!(s.upgrade(&url("http://c.b.a.example/")).is_some());
        assert!(s.upgrade(&url("http://xa.example/")).is_none());

        // Una cabecera no válida no cambia nada; max-age=0 borra.
        s.note(&url("https://a.example/"), "max-age=1; max-age=2");
        assert!(s.is_known("a.example"));
        s.note(&url("https://a.example/"), "max-age=0");
        assert!(!s.is_known("a.example"));
        assert!(s.upgrade(&url("http://a.example/")).is_none());
    }

    #[test]
    fn persists_to_a_file() {
        let dir = std::env::temp_dir().join(format!("bf-hsts-{}", std::process::id()));
        let file = dir.join("hsts.txt");
        let _ = std::fs::remove_file(&file);
        {
            let s = HstsStore::on_disk(&file);
            s.note(
                &url("https://a.example/"),
                "max-age=1000; includeSubDomains",
            );
            s.note(&url("https://b.example/"), "max-age=1000");
        }
        let s = HstsStore::on_disk(&file);
        assert_eq!(s.len(), 2);
        assert!(s.is_known("x.a.example"));
        assert!(!s.is_known("x.b.example"));
        s.note(&url("https://b.example/"), "max-age=0");
        assert_eq!(HstsStore::on_disk(&file).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
