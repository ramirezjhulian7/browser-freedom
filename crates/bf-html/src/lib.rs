//! Parser HTML conforme a WHATWG: tokenizador (13.2.5) y tree builder (13.2.6).

#![forbid(unsafe_code)]

pub mod dom; // E4: serialización HTML, innerHTML/outerHTML/insertAdjacentHTML
pub mod tokenizer;
pub mod tree_builder;
pub mod xml;

pub use tree_builder::script::{Parser, ParserStatus, ScriptDisposition, ScriptHost};
pub use tree_builder::{FragmentContext, ParseOptions, parse_document, parse_fragment};
pub use xml::{
    PARSERERROR_NAMESPACE, XmlError, parse_xml, parse_xml_document,
    parse_xml_document_with_error_report, parse_xml_inert, parse_xml_partial,
    parse_xml_partial_with, parsererror_document,
};
