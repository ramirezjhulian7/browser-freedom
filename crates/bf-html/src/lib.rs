//! Parser HTML conforme a WHATWG: tokenizador (13.2.5) y tree builder (13.2.6).

#![forbid(unsafe_code)]

pub mod tokenizer;
pub mod tree_builder;
pub mod xml;

pub use tree_builder::{FragmentContext, ParseOptions, parse_document, parse_fragment};
pub use xml::parse_xml_document;
