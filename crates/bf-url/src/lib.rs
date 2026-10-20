//! URLs conformes al URL Living Standard de WHATWG: parser (máquina de estados completa),
//! hosts (dominios con IDNA/UTS #46, IPv4, IPv6, opacos), serialización, origin, setters de la
//! API `URL` y `application/x-www-form-urlencoded`.

#![forbid(unsafe_code)]

pub mod data;
pub mod form;
pub mod host;
pub mod idna;
mod parser;
pub mod percent;
mod tables;

use std::fmt;

pub use host::{Host, domain_to_ascii};
use parser::{State, basic_parse};

/// Path de una URL: opaco (`mailto:x`) o lista de segmentos.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Path {
    Opaque(String),
    List(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Url {
    pub scheme: String,
    pub username: String,
    pub password: String,
    pub host: Option<Host>,
    pub port: Option<u16>,
    pub path: Path,
    pub query: Option<String>,
    pub fragment: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError;

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("URL inválida")
    }
}

impl std::error::Error for ParseError {}

/// Esquemas especiales (§4.1).
pub fn is_special_scheme(s: &str) -> bool {
    matches!(s, "ftp" | "file" | "http" | "https" | "ws" | "wss")
}

/// Puerto por defecto de un esquema especial.
pub fn default_port(s: &str) -> Option<u16> {
    match s {
        "ftp" => Some(21),
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        _ => None,
    }
}

/// Origin (HTML §7.1.1): una tupla (esquema, host, puerto) o un origin opaco.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Origin {
    Tuple(String, Host, Option<u16>),
    Opaque,
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Origin::Opaque => f.write_str("null"),
            Origin::Tuple(scheme, host, port) => {
                write!(f, "{scheme}://{host}")?;
                if let Some(p) = port {
                    write!(f, ":{p}")?;
                }
                Ok(())
            }
        }
    }
}

impl Url {
    /// API `URL(input, base)`: parsea `input` relativo a `base`.
    pub fn parse(input: &str, base: Option<&Url>) -> Result<Url, ParseError> {
        basic_parse(input, base, None, None, None)
            .ok()
            .flatten()
            .ok_or(ParseError)
    }

    /// Como [`Url::parse`] con el encoding del documento para el query: `encode` da los bytes
    /// de cada code point, o `None` si no es representable.
    pub fn parse_with_encoding(
        input: &str,
        base: Option<&Url>,
        encode: &dyn Fn(char) -> Option<Vec<u8>>,
    ) -> Result<Url, ParseError> {
        basic_parse(input, base, None, None, Some(encode))
            .ok()
            .flatten()
            .ok_or(ParseError)
    }

    pub fn has_opaque_path(&self) -> bool {
        matches!(self.path, Path::Opaque(_))
    }

    fn cannot_have_credentials_or_port(&self) -> bool {
        matches!(self.host, None | Some(Host::Empty)) || self.scheme == "file"
    }

    /// Serialización del path (§4.5).
    pub fn path_string(&self) -> String {
        match &self.path {
            Path::Opaque(p) => p.clone(),
            Path::List(segments) => {
                let mut out = String::new();
                for s in segments {
                    out.push('/');
                    out.push_str(s);
                }
                out
            }
        }
    }

    /// URL serializer (§4.5).
    pub fn serialize(&self, exclude_fragment: bool) -> String {
        let mut out = format!("{}:", self.scheme);
        if let Some(host) = &self.host {
            out.push_str("//");
            if self.includes_credentials() {
                out.push_str(&self.username);
                if !self.password.is_empty() {
                    out.push(':');
                    out.push_str(&self.password);
                }
                out.push('@');
            }
            out.push_str(&host.to_string());
            if let Some(p) = self.port {
                out.push_str(&format!(":{p}"));
            }
        } else if let Path::List(segments) = &self.path
            && segments.len() > 1
            && segments[0].is_empty()
        {
            out.push_str("/.");
        }
        out.push_str(&self.path_string());
        if let Some(q) = &self.query {
            out.push('?');
            out.push_str(q);
        }
        if !exclude_fragment && let Some(f) = &self.fragment {
            out.push('#');
            out.push_str(f);
        }
        out
    }

    /// Origin de la URL (§4.7).
    pub fn origin(&self) -> Origin {
        match self.scheme.as_str() {
            "blob" => {
                let inner = Url::parse(&self.path_string(), None);
                match inner {
                    Ok(u) if matches!(u.scheme.as_str(), "http" | "https") => u.origin(),
                    _ => Origin::Opaque,
                }
            }
            "ftp" | "http" | "https" | "ws" | "wss" => Origin::Tuple(
                self.scheme.clone(),
                self.host.clone().unwrap_or(Host::Empty),
                self.port,
            ),
            _ => Origin::Opaque,
        }
    }

    // --- Getters de la API URL ---

    pub fn href(&self) -> String {
        self.serialize(false)
    }

    pub fn protocol(&self) -> String {
        format!("{}:", self.scheme)
    }

    pub fn host_string(&self) -> String {
        match (&self.host, self.port) {
            (None, _) => String::new(),
            (Some(h), None) => h.to_string(),
            (Some(h), Some(p)) => format!("{h}:{p}"),
        }
    }

    pub fn hostname(&self) -> String {
        self.host
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default()
    }

    pub fn port_string(&self) -> String {
        self.port.map(|p| p.to_string()).unwrap_or_default()
    }

    pub fn search(&self) -> String {
        match &self.query {
            Some(q) if !q.is_empty() => format!("?{q}"),
            _ => String::new(),
        }
    }

    pub fn hash(&self) -> String {
        match &self.fragment {
            Some(f) if !f.is_empty() => format!("#{f}"),
            _ => String::new(),
        }
    }

    // --- Setters de la API URL (§6.1) ---

    pub fn set_href(&mut self, value: &str) -> Result<(), ParseError> {
        *self = Url::parse(value, None)?;
        Ok(())
    }

    pub fn set_protocol(&mut self, value: &str) {
        let _ = basic_parse(
            &format!("{value}:"),
            None,
            Some(self),
            Some(State::SchemeStart),
            None,
        );
    }

    pub fn set_username(&mut self, value: &str) {
        if self.cannot_have_credentials_or_port() {
            return;
        }
        self.username = percent::utf8_encode(value, percent::EncodeSet::Userinfo);
    }

    pub fn set_password(&mut self, value: &str) {
        if self.cannot_have_credentials_or_port() {
            return;
        }
        self.password = percent::utf8_encode(value, percent::EncodeSet::Userinfo);
    }

    /// Los setters corren el parser sobre la URL misma: como en el spec, lo que se cambió antes
    /// de una falla se queda (`host = "example.com:"` cambia el host aunque el puerto falle).
    fn with_override(&mut self, value: &str, state: State) {
        let _ = basic_parse(value, None, Some(self), Some(state), None);
    }

    pub fn set_host(&mut self, value: &str) {
        if self.has_opaque_path() {
            return;
        }
        self.with_override(value, State::Host);
    }

    pub fn set_hostname(&mut self, value: &str) {
        if self.has_opaque_path() {
            return;
        }
        self.with_override(value, State::Hostname);
    }

    pub fn set_port(&mut self, value: &str) {
        if self.cannot_have_credentials_or_port() {
            return;
        }
        if value.is_empty() {
            self.port = None;
        } else {
            self.with_override(value, State::Port);
        }
    }

    pub fn set_pathname(&mut self, value: &str) {
        if self.has_opaque_path() {
            return;
        }
        self.path = Path::List(Vec::new());
        self.with_override(value, State::PathStart);
    }

    /// "Potentially strip trailing spaces from an opaque path" (§6.1).
    fn strip_trailing_spaces(&mut self) {
        if self.fragment.is_some() || self.query.is_some() {
            return;
        }
        if let Path::Opaque(p) = &mut self.path {
            let trimmed = p.trim_end_matches(' ').len();
            p.truncate(trimmed);
        }
    }

    pub fn set_search(&mut self, value: &str) {
        if value.is_empty() {
            self.query = None;
            self.strip_trailing_spaces();
            return;
        }
        let input = value.strip_prefix('?').unwrap_or(value);
        self.query = Some(String::new());
        self.with_override(input, State::Query);
    }

    pub fn set_hash(&mut self, value: &str) {
        if value.is_empty() {
            self.fragment = None;
            self.strip_trailing_spaces();
            return;
        }
        let input = value.strip_prefix('#').unwrap_or(value);
        self.fragment = Some(String::new());
        self.with_override(input, State::Fragment);
    }

    /// Path del sistema de archivos de una URL `file:` (`None` para otros esquemas).
    pub fn to_file_path(&self) -> Option<std::path::PathBuf> {
        if self.scheme != "file" {
            return None;
        }
        let Path::List(segments) = &self.path else {
            return None;
        };
        let decoded: Vec<String> = segments
            .iter()
            .map(|s| String::from_utf8_lossy(&percent::decode(s.as_bytes())).into_owned())
            .collect();
        let host = self.hostname();
        let joined = decoded.join("/");
        if cfg!(windows) {
            if !host.is_empty() {
                return Some(format!(r"\\{host}\{}", joined.replace('/', "\\")).into());
            }
            // `file:///C:/x` → `C:/x`.
            Some(joined.into())
        } else {
            Some(format!("/{joined}").into())
        }
    }

    /// URL `file:` de un path absoluto del sistema de archivos.
    pub fn from_file_path(path: &std::path::Path) -> Result<Url, ParseError> {
        let s = path.to_string_lossy().replace('\\', "/");
        let s = if s.starts_with('/') {
            s
        } else {
            format!("/{s}")
        };
        Url::parse(&format!("file://{s}"), None)
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.serialize(false))
    }
}

impl std::str::FromStr for Url {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Url, ParseError> {
        Url::parse(s, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics() {
        let base = Url::parse("http://example.org/foo/bar", None).unwrap();
        let u = Url::parse("../baz?q#f", Some(&base)).unwrap();
        assert_eq!(u.href(), "http://example.org/baz?q#f");
        assert_eq!(u.origin().to_string(), "http://example.org");
        let u = Url::parse("HTTPS://user:pa ss@EXAMPLE.com:443/a/./b/../c", None).unwrap();
        assert_eq!(u.href(), "https://user:pa%20ss@example.com/a/c");
        assert!(Url::parse("http://[::1", None).is_err());
        let f = Url::parse("file:///C:/dir/a%20b.txt", None).unwrap();
        assert_eq!(f.path_string(), "/C:/dir/a%20b.txt");
    }
}
