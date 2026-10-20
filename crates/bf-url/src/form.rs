//! `application/x-www-form-urlencoded` (URL Standard §5): parser y serializador, la base de
//! `URLSearchParams`.

use crate::percent::{self, EncodeSet};

/// Parser (§5.1): pares (nombre, valor).
pub fn parse(input: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for sequence in input.split(|&b| b == b'&') {
        if sequence.is_empty() {
            continue;
        }
        let (name, value) = match sequence.iter().position(|&b| b == b'=') {
            Some(i) => (&sequence[..i], &sequence[i + 1..]),
            None => (sequence, &[][..]),
        };
        let plus = |s: &[u8]| -> Vec<u8> {
            s.iter()
                .map(|&b| if b == b'+' { b' ' } else { b })
                .collect()
        };
        let decode = |s: &[u8]| String::from_utf8_lossy(&percent::decode(&plus(s))).into_owned();
        out.push((decode(name), decode(value)));
    }
    out
}

/// Serializador (§5.2) con UTF-8.
pub fn serialize(pairs: &[(String, String)]) -> String {
    let mut out = String::new();
    for (i, (name, value)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        out.push_str(&percent::encode_bytes(
            name.as_bytes(),
            EncodeSet::FormUrlencoded,
            true,
        ));
        out.push('=');
        out.push_str(&percent::encode_bytes(
            value.as_bytes(),
            EncodeSet::FormUrlencoded,
            true,
        ));
    }
    out
}

/// Equivalente mínimo de `URLSearchParams`: lista ordenada de pares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchParams(pub Vec<(String, String)>);

impl SearchParams {
    pub fn parse(query: &str) -> SearchParams {
        SearchParams(parse(query.strip_prefix('?').unwrap_or(query).as_bytes()))
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn get_all(&self, name: &str) -> Vec<&str> {
        self.0
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    pub fn append(&mut self, name: &str, value: &str) {
        self.0.push((name.to_owned(), value.to_owned()));
    }

    /// Reemplaza el primer par con ese nombre y elimina el resto.
    pub fn set(&mut self, name: &str, value: &str) {
        let mut found = false;
        self.0.retain_mut(|(n, v)| {
            if n != name {
                return true;
            }
            if found {
                return false;
            }
            found = true;
            *v = value.to_owned();
            true
        });
        if !found {
            self.append(name, value);
        }
    }

    pub fn delete(&mut self, name: &str) {
        self.0.retain(|(n, _)| n != name);
    }

    /// Orden estable por nombre en unidades UTF-16 (§6.2 `sort()`).
    pub fn sort(&mut self) {
        self.0
            .sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
    }
}

impl std::fmt::Display for SearchParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&serialize(&self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let p = SearchParams::parse("?a=1&b=x+y&a=%C3%B1&c");
        assert_eq!(p.get("b"), Some("x y"));
        assert_eq!(p.get_all("a"), ["1", "ñ"]);
        assert_eq!(p.to_string(), "a=1&b=x+y&a=%C3%B1&c=");
        let mut p = p;
        p.set("a", "z");
        p.sort();
        assert_eq!(p.to_string(), "a=z&b=x+y&c=");
    }
}
