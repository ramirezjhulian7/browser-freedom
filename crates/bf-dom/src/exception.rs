//! `DOMException` (WebIDL §3.14.3): los nombres de error que usa el DOM, con su mensaje de la
//! tabla de nombres de error de WebIDL y su `code` heredado. `TypeError` no es un
//! `DOMException` pero algunos métodos del DOM lo lanzan; se incluye aquí para que los
//! bindings lo traduzcan.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DomException {
    IndexSizeError,
    HierarchyRequestError,
    WrongDocumentError,
    InvalidCharacterError,
    NoModificationAllowedError,
    NotFoundError,
    NotSupportedError,
    InUseAttributeError,
    InvalidStateError,
    SyntaxError,
    InvalidModificationError,
    NamespaceError,
    InvalidAccessError,
    TypeMismatchError,
    SecurityError,
    AbortError,
    TimeoutError,
    InvalidNodeTypeError,
    DataCloneError,
    /// `TypeError` de ECMAScript.
    TypeError,
}

impl DomException {
    /// El `name` del `DOMException`.
    pub fn name(self) -> &'static str {
        match self {
            DomException::IndexSizeError => "IndexSizeError",
            DomException::HierarchyRequestError => "HierarchyRequestError",
            DomException::WrongDocumentError => "WrongDocumentError",
            DomException::InvalidCharacterError => "InvalidCharacterError",
            DomException::NoModificationAllowedError => "NoModificationAllowedError",
            DomException::NotFoundError => "NotFoundError",
            DomException::NotSupportedError => "NotSupportedError",
            DomException::InUseAttributeError => "InUseAttributeError",
            DomException::InvalidStateError => "InvalidStateError",
            DomException::SyntaxError => "SyntaxError",
            DomException::InvalidModificationError => "InvalidModificationError",
            DomException::NamespaceError => "NamespaceError",
            DomException::InvalidAccessError => "InvalidAccessError",
            DomException::TypeMismatchError => "TypeMismatchError",
            DomException::SecurityError => "SecurityError",
            DomException::AbortError => "AbortError",
            DomException::TimeoutError => "TimeoutError",
            DomException::InvalidNodeTypeError => "InvalidNodeTypeError",
            DomException::DataCloneError => "DataCloneError",
            DomException::TypeError => "TypeError",
        }
    }

    /// La descripción de la tabla de WebIDL.
    pub fn message(self) -> &'static str {
        match self {
            DomException::IndexSizeError => "The index is not in the allowed range.",
            DomException::HierarchyRequestError => {
                "The operation would yield an incorrect node tree."
            }
            DomException::WrongDocumentError => "The object is in the wrong document.",
            DomException::InvalidCharacterError => "The string contains invalid characters.",
            DomException::NoModificationAllowedError => "The object can not be modified.",
            DomException::NotFoundError => "The object can not be found here.",
            DomException::NotSupportedError => "The operation is not supported.",
            DomException::InUseAttributeError => "The attribute is in use by another element.",
            DomException::InvalidStateError => "The object is in an invalid state.",
            DomException::SyntaxError => "The string did not match the expected pattern.",
            DomException::InvalidModificationError => "The object can not be modified in this way.",
            DomException::NamespaceError => "The operation is not allowed by Namespaces in XML.",
            DomException::InvalidAccessError => {
                "The object does not support the operation or argument."
            }
            DomException::TypeMismatchError => {
                "The type of the object does not match the expected type."
            }
            DomException::SecurityError => "The operation is insecure.",
            DomException::AbortError => "The operation was aborted.",
            DomException::TimeoutError => "The operation timed out.",
            DomException::InvalidNodeTypeError => {
                "The supplied node is incorrect or has an incorrect ancestor for this operation."
            }
            DomException::DataCloneError => "The object can not be cloned.",
            DomException::TypeError => "Type error.",
        }
    }

    /// El `code` heredado (0 si el nombre no tiene uno).
    pub fn code(self) -> u16 {
        match self {
            DomException::IndexSizeError => 1,
            DomException::HierarchyRequestError => 3,
            DomException::WrongDocumentError => 4,
            DomException::InvalidCharacterError => 5,
            DomException::NoModificationAllowedError => 7,
            DomException::NotFoundError => 8,
            DomException::NotSupportedError => 9,
            DomException::InUseAttributeError => 10,
            DomException::InvalidStateError => 11,
            DomException::SyntaxError => 12,
            DomException::InvalidModificationError => 13,
            DomException::NamespaceError => 14,
            DomException::InvalidAccessError => 15,
            DomException::TypeMismatchError => 17,
            DomException::SecurityError => 18,
            DomException::AbortError => 20,
            DomException::TimeoutError => 23,
            DomException::InvalidNodeTypeError => 24,
            DomException::DataCloneError => 25,
            DomException::TypeError => 0,
        }
    }
}

impl fmt::Display for DomException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.name(), self.message())
    }
}

impl std::error::Error for DomException {}
