//! Public Suffix List (<https://publicsuffix.org/list/>): el sufijo público y el dominio
//! registrable de un host, con el algoritmo de la lista (reglas normales, comodines `*.` y
//! excepciones `!`; sin coincidencias vale la regla `*`). Lo usan las cookies (`SameSite`, el
//! atributo `Domain`) y todo lo que necesite saber si dos hosts son del mismo *site*.
//!
//! Los datos están en `crates/bf-infra/data/public_suffix_list.txt`, generado por
//! `tools/gen_psl.py` (reglas en ASCII, con la sección privada tras `!private`).

use std::collections::HashSet;
use std::sync::OnceLock;

const DATA: &str = include_str!("../../bf-infra/data/public_suffix_list.txt");

struct List {
    /// Reglas normales y comodines (`*.ck`), tal cual.
    rules: HashSet<&'static str>,
    /// Excepciones, sin el `!`.
    exceptions: HashSet<&'static str>,
}

fn list() -> &'static List {
    static LIST: OnceLock<List> = OnceLock::new();
    LIST.get_or_init(|| {
        let mut rules = HashSet::new();
        let mut exceptions = HashSet::new();
        for line in DATA.lines() {
            match line {
                "" | "!private" => {}
                l => match l.strip_prefix('!') {
                    Some(e) => {
                        exceptions.insert(e);
                    }
                    None => {
                        rules.insert(l);
                    }
                },
            }
        }
        List { rules, exceptions }
    })
}

/// Las etiquetas de un host de dominio en minúsculas, o `None` si no es un dominio (IP,
/// vacío, punto inicial, etiquetas vacías). Un punto final se ignora.
fn labels(host: &str) -> Option<Vec<String>> {
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty() || host.starts_with('[') || host.parse::<std::net::Ipv4Addr>().is_ok() {
        return None;
    }
    let labels: Vec<String> = host.split('.').map(str::to_ascii_lowercase).collect();
    if labels.iter().any(String::is_empty) {
        return None;
    }
    Some(labels)
}

/// Cuántas etiquetas finales de `labels` forman el sufijo público.
fn suffix_len(labels: &[String]) -> usize {
    let list = list();
    let n = labels.len();
    // De la coincidencia más larga a la más corta: la primera que aparece es la que manda
    // (una excepción gana a su comodín, que es una etiqueta más corto que ella).
    for i in 0..n {
        let candidate = labels[i..].join(".");
        if list.exceptions.contains(candidate.as_str()) {
            // La regla de excepción sin su etiqueta de la izquierda.
            return n - i - 1;
        }
        if list.rules.contains(candidate.as_str()) {
            return n - i;
        }
        if i + 1 < n {
            let wildcard = format!("*.{}", labels[i + 1..].join("."));
            if list.rules.contains(wildcard.as_str()) {
                return n - i;
            }
        }
    }
    // La regla por defecto `*`: la última etiqueta.
    1
}

/// El sufijo público de `host` (ASCII, como lo da el parser de URLs): `co.uk` para
/// `www.example.co.uk`. `None` si `host` no es un dominio.
pub fn public_suffix(host: &str) -> Option<String> {
    let labels = labels(host)?;
    let n = suffix_len(&labels);
    Some(labels[labels.len() - n..].join("."))
}

/// El dominio registrable (*registrable domain* del URL Standard §3.2, "eTLD+1"): el sufijo
/// público más una etiqueta. `None` si `host` es un sufijo público, una IP o no es un
/// dominio. `registrable_domain("www.example.co.uk") == Some("example.co.uk")`.
pub fn registrable_domain(host: &str) -> Option<String> {
    let labels = labels(host)?;
    let n = suffix_len(&labels);
    if labels.len() <= n {
        return None;
    }
    Some(labels[labels.len() - n - 1..].join("."))
}

/// ¿Es `host` exactamente un sufijo público? (RFC 6265bis §5.7: un `Domain` así se rechaza.)
pub fn is_public_suffix(host: &str) -> bool {
    labels(host).is_some_and(|l| suffix_len(&l) == l.len())
}

/// ¿Son `a` y `b` del mismo *site* (HTML §7.1.1, *same site* sin esquema)? Mismo dominio
/// registrable, o el mismo host si no lo tienen (IPs, sufijos).
pub fn same_site(a: &str, b: &str) -> bool {
    match (registrable_domain(a), registrable_domain(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a.eq_ignore_ascii_case(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `checkPublicSuffix` de `tests/test_psl.txt` de la lista (las versiones en ASCII de los
    /// casos IDN, que es como llegan los hosts del parser de URLs).
    #[test]
    fn psl_test_vectors() {
        let cases: &[(&str, Option<&str>)] = &[
            ("COM", None),
            ("example.COM", Some("example.com")),
            ("WwW.example.COM", Some("example.com")),
            (".com", None),
            (".example", None),
            (".example.com", None),
            (".example.example", None),
            ("example", None),
            ("example.example", Some("example.example")),
            ("b.example.example", Some("example.example")),
            ("a.b.example.example", Some("example.example")),
            ("biz", None),
            ("domain.biz", Some("domain.biz")),
            ("b.domain.biz", Some("domain.biz")),
            ("a.b.domain.biz", Some("domain.biz")),
            ("com", None),
            ("example.com", Some("example.com")),
            ("b.example.com", Some("example.com")),
            ("a.b.example.com", Some("example.com")),
            ("uk.com", None),
            ("example.uk.com", Some("example.uk.com")),
            ("b.example.uk.com", Some("example.uk.com")),
            ("a.b.example.uk.com", Some("example.uk.com")),
            ("test.ac", Some("test.ac")),
            ("mm", None),
            ("c.mm", None),
            ("b.c.mm", Some("b.c.mm")),
            ("a.b.c.mm", Some("b.c.mm")),
            ("jp", None),
            ("test.jp", Some("test.jp")),
            ("www.test.jp", Some("test.jp")),
            ("ac.jp", None),
            ("test.ac.jp", Some("test.ac.jp")),
            ("www.test.ac.jp", Some("test.ac.jp")),
            ("kyoto.jp", None),
            ("test.kyoto.jp", Some("test.kyoto.jp")),
            ("ide.kyoto.jp", None),
            ("b.ide.kyoto.jp", Some("b.ide.kyoto.jp")),
            ("a.b.ide.kyoto.jp", Some("b.ide.kyoto.jp")),
            ("c.kobe.jp", None),
            ("b.c.kobe.jp", Some("b.c.kobe.jp")),
            ("a.b.c.kobe.jp", Some("b.c.kobe.jp")),
            ("city.kobe.jp", Some("city.kobe.jp")),
            ("www.city.kobe.jp", Some("city.kobe.jp")),
            ("ck", None),
            ("test.ck", None),
            ("b.test.ck", Some("b.test.ck")),
            ("a.b.test.ck", Some("b.test.ck")),
            ("www.ck", Some("www.ck")),
            ("www.www.ck", Some("www.ck")),
            ("us", None),
            ("test.us", Some("test.us")),
            ("www.test.us", Some("test.us")),
            ("ak.us", None),
            ("test.ak.us", Some("test.ak.us")),
            ("www.test.ak.us", Some("test.ak.us")),
            ("k12.ak.us", None),
            ("test.k12.ak.us", Some("test.k12.ak.us")),
            ("www.test.k12.ak.us", Some("test.k12.ak.us")),
            ("xn--85x722f.com.cn", Some("xn--85x722f.com.cn")),
            (
                "xn--85x722f.xn--55qx5d.cn",
                Some("xn--85x722f.xn--55qx5d.cn"),
            ),
            (
                "www.xn--85x722f.xn--55qx5d.cn",
                Some("xn--85x722f.xn--55qx5d.cn"),
            ),
            ("shishi.xn--55qx5d.cn", Some("shishi.xn--55qx5d.cn")),
            ("xn--55qx5d.cn", None),
            ("xn--85x722f.xn--fiqs8s", Some("xn--85x722f.xn--fiqs8s")),
            ("www.xn--85x722f.xn--fiqs8s", Some("xn--85x722f.xn--fiqs8s")),
            ("shishi.xn--fiqs8s", Some("shishi.xn--fiqs8s")),
            ("xn--fiqs8s", None),
        ];
        for (host, expected) in cases {
            assert_eq!(
                registrable_domain(host).as_deref(),
                *expected,
                "registrable_domain({host})"
            );
        }
    }

    #[test]
    fn suffixes_ips_and_private_rules() {
        assert_eq!(public_suffix("www.example.co.uk").as_deref(), Some("co.uk"));
        assert_eq!(public_suffix("localhost").as_deref(), Some("localhost"));
        assert!(is_public_suffix("co.uk") && is_public_suffix("github.io"));
        assert!(!is_public_suffix("example.com"));
        // Sección privada: cada usuario de github.io es un site distinto.
        assert_eq!(
            registrable_domain("a.b.github.io").as_deref(),
            Some("b.github.io")
        );
        assert!(!same_site("alice.github.io", "bob.github.io"));
        assert!(same_site("www.example.com", "static.example.com"));
        assert_eq!(registrable_domain("127.0.0.1"), None);
        assert_eq!(registrable_domain("[::1]"), None);
        assert!(same_site("127.0.0.1", "127.0.0.1"));
        assert_eq!(
            registrable_domain("example.com.").as_deref(),
            Some("example.com")
        );
    }
}
