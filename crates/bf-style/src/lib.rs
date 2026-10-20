//! Motor de estilos: cascada, herencia, especificidad y valores computados.

#![forbid(unsafe_code)]

pub mod cascade;
pub mod hints;
pub mod loader;
pub mod properties;
pub mod report;
pub mod values;

pub use cascade::{
    FontMetricsSource, StyleMap, is_supported, style_document, style_document_with_sheets,
};
pub use loader::{Resources, document_stylesheets, font_face_rules};
pub use properties::ComputedStyle;
pub use report::{StyleReport, style_report};
