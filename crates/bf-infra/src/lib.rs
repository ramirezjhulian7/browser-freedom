//! Tipos comunes del motor: interning de strings (`Atom`), `SourceLocation` y utilidades compartidas.

#![forbid(unsafe_code)]

/// Texto del DOM.
///
/// Alias temporal. Cuando exista bf-js pasará a un tipo WTF-8 para admitir surrogates sueltos (ver
/// TODO(bf-dom) en el tokenizador).
pub type DomString = String;

/// Namespaces de <https://infra.spec.whatwg.org/#namespaces>.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Namespace {
    Html,
    Svg,
    MathMl,
    XLink,
    Xml,
    XmlNs,
}

impl Namespace {
    pub fn uri(&self) -> &'static str {
        match self {
            Namespace::Html => "http://www.w3.org/1999/xhtml",
            Namespace::Svg => "http://www.w3.org/2000/svg",
            Namespace::MathMl => "http://www.w3.org/1998/Math/MathML",
            Namespace::XLink => "http://www.w3.org/1999/xlink",
            Namespace::Xml => "http://www.w3.org/XML/1998/namespace",
            Namespace::XmlNs => "http://www.w3.org/2000/xmlns/",
        }
    }
}

pub mod encoding;
mod encoding_tables;
pub mod resource;

pub use bf_url::Url;
pub use resource::{FileLoader, Resource, ResourceLoader, decode_text, url_from_arg};
