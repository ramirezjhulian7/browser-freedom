//! Tipos comunes del motor: el texto del DOM ([`DomString`], WTF-8), strings internados
//! ([`Atom`]), namespaces y utilidades compartidas.

#![forbid(unsafe_code)]

mod atom;
mod dom_string;

pub use atom::Atom;
pub use dom_string::DomString;

/// Namespaces de <https://infra.spec.whatwg.org/#namespaces>; `Other` es cualquier otro
/// namespace URI (no vacío) de un documento XML.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Namespace {
    Html,
    Svg,
    MathMl,
    XLink,
    Xml,
    XmlNs,
    Other(Atom),
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
            Namespace::Other(a) => a.as_str(),
        }
    }

    /// El namespace de un URI (`None` para el string vacío, que es "sin namespace").
    pub fn from_uri(uri: &str) -> Option<Namespace> {
        Some(match uri {
            "" => return None,
            "http://www.w3.org/1999/xhtml" => Namespace::Html,
            "http://www.w3.org/2000/svg" => Namespace::Svg,
            "http://www.w3.org/1998/Math/MathML" => Namespace::MathMl,
            "http://www.w3.org/1999/xlink" => Namespace::XLink,
            "http://www.w3.org/XML/1998/namespace" => Namespace::Xml,
            "http://www.w3.org/2000/xmlns/" => Namespace::XmlNs,
            other => Namespace::Other(Atom::new(other)),
        })
    }
}

pub mod encoding;
mod encoding_tables;
pub mod resource;

pub use bf_url::Url;
pub use resource::{FileLoader, Resource, ResourceLoader, decode_text, url_from_arg};
