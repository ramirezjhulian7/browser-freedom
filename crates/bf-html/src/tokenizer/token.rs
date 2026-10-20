//! Tokens emitidos por el tokenizador (WHATWG HTML 13.2.5).

/// Un token de salida del tokenizador.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Doctype(Doctype),
    StartTag(Tag),
    EndTag(Tag),
    Comment(String),
    ProcessingInstruction(ProcessingInstruction),
    Character(char),
    Eof,
}

/// Token de start tag o end tag.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    pub self_closing: bool,
    pub attrs: Vec<(String, String)>,
}

/// Token DOCTYPE. `None` representa el estado "missing" del spec, distinto del string vacío.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Doctype {
    pub name: Option<String>,
    pub public_id: Option<String>,
    pub system_id: Option<String>,
    pub force_quirks: bool,
}

/// Token de processing instruction (`<?target data?>`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessingInstruction {
    pub target: String,
    pub data: String,
}
