//! Errores de parseo (WHATWG HTML 13.2.2).

/// Un error de parseo. Los errores de parseo no detienen el tokenizador.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError {
    /// Código exacto del spec, p. ej. `"eof-in-tag"`.
    pub code: &'static str,
    /// Índice (en code points) en la entrada ya preprocesada del carácter que causó el
    /// error, o la longitud de la entrada si ocurrió en EOF.
    pub offset: usize,
}
