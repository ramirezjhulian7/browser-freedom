//! Tokenizador HTML conforme a WHATWG HTML 13.2.5.
//!
//! La máquina de estados está en [`states`]; este módulo contiene el flujo de entrada
//! (13.2.3.5), el estado interno del tokenizador y las operaciones auxiliares que usan los
//! estados (crear, emitir y modificar tokens).

mod error;
pub(crate) mod named_entities;
mod states;
mod token;

use std::collections::VecDeque;

pub use error::ParseError;
pub use states::State;
pub use token::{Doctype, ProcessingInstruction, Tag, Token};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TagKind {
    Start,
    End,
}

pub struct Tokenizer {
    /// Flujo de entrada ya preprocesado (newlines normalizados).
    input: Vec<char>,
    /// Índice del *next input character*.
    pos: usize,
    /// El *current input character*; `None` representa EOF.
    current: Option<char>,
    /// Si es `true`, el próximo consumo devuelve `current` sin avanzar.
    reconsume: bool,
    /// Primer índice que aún no pasó por la validación de 13.2.3.5.
    checked: usize,
    state: State,
    /// El *return state* de los estados de character reference.
    return_state: State,
    /// El *temporary buffer*.
    temp: String,
    /// El *character reference code*.
    char_ref_code: u32,
    tag_kind: TagKind,
    tag: Tag,
    /// Hay un atributo actual (aún no agregado a `tag.attrs`).
    attr_active: bool,
    /// El atributo actual es duplicado y se descartará.
    attr_duplicate: bool,
    attr_name: String,
    attr_value: String,
    comment: String,
    doctype: Doctype,
    pi: ProcessingInstruction,
    last_start_tag: Option<String>,
    cdata_allowed: bool,
    queue: VecDeque<Token>,
    eof_emitted: bool,
    errors: Vec<ParseError>,
}

impl Tokenizer {
    pub fn new(input: &str) -> Self {
        // TODO(bf-dom): input como code points para soportar surrogates de document.write
        Tokenizer {
            input: preprocess(input),
            pos: 0,
            current: None,
            reconsume: false,
            checked: 0,
            state: State::Data,
            return_state: State::Data,
            temp: String::new(),
            char_ref_code: 0,
            tag_kind: TagKind::Start,
            tag: Tag::default(),
            attr_active: false,
            attr_duplicate: false,
            attr_name: String::new(),
            attr_value: String::new(),
            comment: String::new(),
            doctype: Doctype::default(),
            pi: ProcessingInstruction::default(),
            last_start_tag: None,
            cdata_allowed: false,
            queue: VecDeque::new(),
            eof_emitted: false,
            errors: Vec::new(),
        }
    }

    /// Cambia el estado del tokenizador (lo usa el tree builder, p. ej. al ver `<script>`).
    pub fn set_state(&mut self, s: State) {
        self.state = s;
    }

    /// Fija el nombre del último start tag emitido, usado para el *appropriate end tag token*.
    pub fn set_last_start_tag(&mut self, name: &str) {
        self.last_start_tag = Some(name.to_owned());
    }

    /// Indica si existe un *adjusted current node* que no es un elemento del namespace HTML.
    /// Solo en ese caso `<![CDATA[` abre una sección CDATA (13.2.5.42). Lo fija el tree builder.
    pub fn set_cdata_allowed(&mut self, allowed: bool) {
        self.cdata_allowed = allowed;
    }

    /// Devuelve el siguiente token, o `None` después de haber devuelto `Token::Eof`.
    pub fn next_token(&mut self) -> Option<Token> {
        loop {
            if let Some(token) = self.queue.pop_front() {
                return Some(token);
            }
            if self.eof_emitted {
                return None;
            }
            self.step();
        }
    }

    pub fn errors(&self) -> &[ParseError] {
        &self.errors
    }

    /// Índice (en code points de la entrada preprocesada) del *next input character*.
    pub fn offset(&self) -> usize {
        self.pos
    }

    // --- Flujo de entrada ---

    /// Consume el *next input character* (o devuelve el actual si hay que reconsumir).
    fn consume(&mut self) -> Option<char> {
        if self.reconsume {
            self.reconsume = false;
            return self.current;
        }
        let c = self.input.get(self.pos).copied();
        if let Some(ch) = c {
            if self.pos >= self.checked {
                self.checked = self.pos + 1;
                self.check_input_character(ch, self.pos);
            }
            self.pos += 1;
        }
        self.current = c;
        c
    }

    /// Cambia a `state` y reconsume el carácter actual en él.
    fn reconsume_in(&mut self, state: State) {
        self.reconsume = true;
        self.state = state;
    }

    /// ¿Los próximos caracteres (sin consumir) son exactamente `s`?
    fn next_chars_are(&self, s: &str, ignore_ascii_case: bool) -> bool {
        debug_assert!(!self.reconsume);
        self.chars_match_at(self.pos, s, ignore_ascii_case)
    }

    fn chars_match_at(&self, start: usize, s: &str, ignore_ascii_case: bool) -> bool {
        (start..)
            .zip(s.chars())
            .all(|(i, expected)| match self.input.get(i) {
                Some(&c) => {
                    c == expected || (ignore_ascii_case && c.eq_ignore_ascii_case(&expected))
                }
                None => false,
            })
    }

    fn consume_n(&mut self, n: usize) {
        for _ in 0..n {
            self.consume();
        }
    }

    /// Validaciones de 13.2.3.5 "Preprocessing the input stream".
    fn check_input_character(&mut self, c: char, offset: usize) {
        let cp = u32::from(c);
        // Los surrogates no son representables en `char`, así que nunca aparecen aquí.
        let is_control = cp <= 0x1F || (0x7F..=0x9F).contains(&cp);
        let is_ascii_whitespace = matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' ');
        if is_control && !is_ascii_whitespace && c != '\0' {
            self.errors.push(ParseError {
                code: "control-character-in-input-stream",
                offset,
            });
        } else if is_noncharacter(cp) {
            self.errors.push(ParseError {
                code: "noncharacter-in-input-stream",
                offset,
            });
        }
    }

    fn error(&mut self, code: &'static str) {
        let offset = if self.current.is_some() {
            self.pos - 1
        } else {
            self.pos
        };
        self.errors.push(ParseError { code, offset });
    }

    // --- Emisión ---

    fn emit(&mut self, token: Token) {
        if token == Token::Eof {
            self.eof_emitted = true;
        }
        self.queue.push_back(token);
    }

    fn emit_char(&mut self, c: char) {
        self.emit(Token::Character(c));
    }

    fn emit_eof(&mut self) {
        self.emit(Token::Eof);
    }

    /// Emite '<', '/' y los caracteres del temporary buffer.
    fn emit_less_than_solidus_and_temp(&mut self) {
        self.emit_char('<');
        self.emit_char('/');
        let temp = std::mem::take(&mut self.temp);
        for c in temp.chars() {
            self.emit_char(c);
        }
        self.temp = temp;
    }

    // --- Character references ---

    /// "Consumed as part of an attribute": el return state es un estado de valor de atributo.
    fn char_ref_in_attribute(&self) -> bool {
        matches!(
            self.return_state,
            State::AttributeValueDoubleQuoted
                | State::AttributeValueSingleQuoted
                | State::AttributeValueUnquoted
        )
    }

    /// "Flush code points consumed as a character reference".
    fn flush_char_ref(&mut self) {
        if self.char_ref_in_attribute() {
            self.attr_value.push_str(&self.temp);
        } else {
            let temp = std::mem::take(&mut self.temp);
            for c in temp.chars() {
                self.emit_char(c);
            }
            self.temp = temp;
        }
    }

    // --- Tags y atributos ---

    fn create_tag(&mut self, kind: TagKind) {
        self.tag_kind = kind;
        self.tag = Tag::default();
        self.attr_active = false;
    }

    fn is_appropriate_end_tag(&self) -> bool {
        self.tag_kind == TagKind::End && self.last_start_tag.as_deref() == Some(&self.tag.name)
    }

    fn start_attribute(&mut self) {
        self.finish_attribute();
        self.attr_active = true;
        self.attr_duplicate = false;
        self.attr_name.clear();
        self.attr_value.clear();
    }

    /// Al salir del attribute name state: descarta el atributo si su nombre está repetido.
    fn check_duplicate_attribute(&mut self) {
        if self
            .tag
            .attrs
            .iter()
            .any(|(name, _)| *name == self.attr_name)
        {
            self.error("duplicate-attribute");
            self.attr_duplicate = true;
        }
    }

    fn finish_attribute(&mut self) {
        if self.attr_active {
            self.attr_active = false;
            if !self.attr_duplicate {
                let name = std::mem::take(&mut self.attr_name);
                let value = std::mem::take(&mut self.attr_value);
                self.tag.attrs.push((name, value));
            }
        }
    }

    fn emit_tag(&mut self) {
        self.finish_attribute();
        let tag = std::mem::take(&mut self.tag);
        match self.tag_kind {
            TagKind::Start => {
                self.last_start_tag = Some(tag.name.clone());
                self.emit(Token::StartTag(tag));
            }
            TagKind::End => {
                if !tag.attrs.is_empty() {
                    self.error("end-tag-with-attributes");
                }
                if tag.self_closing {
                    self.error("end-tag-with-trailing-solidus");
                }
                self.emit(Token::EndTag(tag));
            }
        }
    }

    // --- Comentarios, DOCTYPE y processing instructions ---

    fn emit_comment(&mut self) {
        let data = std::mem::take(&mut self.comment);
        self.emit(Token::Comment(data));
    }

    /// "Convert the temporary buffer to a comment".
    fn convert_temp_to_comment(&mut self) {
        self.comment.clear();
        self.comment.push('?');
        self.comment.push_str(&self.temp);
    }

    fn create_doctype(&mut self) {
        self.doctype = Doctype::default();
    }

    fn emit_doctype(&mut self) {
        let doctype = std::mem::take(&mut self.doctype);
        self.emit(Token::Doctype(doctype));
    }

    fn emit_processing_instruction(&mut self) {
        let pi = std::mem::take(&mut self.pi);
        self.emit(Token::ProcessingInstruction(pi));
    }
}

/// Normaliza newlines: CRLF y CR sueltos se convierten en LF (13.2.3.5).
fn preprocess(input: &str) -> Vec<char> {
    let mut out = Vec::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

/// Busca el identificador más largo de la tabla de named character references que sea prefijo
/// de `input`. Devuelve su longitud (en caracteres) y el texto al que se expande.
///
/// La tabla está ordenada, así que se mantiene el rango `[lo, hi)` de entradas que comparten el
/// prefijo leído hasta ahora y se estrecha carácter a carácter: O(len · log n).
fn match_named_entity(input: &[char]) -> Option<(usize, &'static str)> {
    let table = named_entities::NAMED_ENTITIES;
    let (mut lo, mut hi) = (0, table.len());
    let mut best = None;
    for (k, &c) in input.iter().enumerate() {
        // Todos los identificadores son ASCII.
        let Ok(byte) = u8::try_from(c) else { break };
        let range = &table[lo..hi];
        // Dentro del rango todas las entradas empiezan con el prefijo de longitud `k`; la
        // única entrada de longitud `k` (si existe) va primero.
        let byte_at = |name: &str| name.as_bytes().get(k).copied();
        let start = range.partition_point(|(name, _)| byte_at(name).is_none_or(|b| b < byte));
        let end = range.partition_point(|(name, _)| byte_at(name).is_none_or(|b| b <= byte));
        (lo, hi) = (lo + start, lo + end);
        if lo == hi {
            break;
        }
        let (name, value) = table[lo];
        if name.len() == k + 1 {
            best = Some((k + 1, value));
        }
    }
    best
}

/// <https://infra.spec.whatwg.org/#noncharacter>
fn is_noncharacter(cp: u32) -> bool {
    (0xFDD0..=0xFDEF).contains(&cp) || (cp & 0xFFFE) == 0xFFFE
}
