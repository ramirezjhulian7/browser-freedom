//! Tokenizador HTML conforme a WHATWG HTML 13.2.5.
//!
//! La máquina de estados está en [`states`]; este módulo contiene el flujo de entrada
//! (13.2.3.5), el estado interno del tokenizador y las operaciones auxiliares que usan los
//! estados (crear, emitir y modificar tokens).

mod error;
pub(crate) mod named_entities;
mod states;
mod token;

use std::cell::Cell;
use std::collections::VecDeque;

pub use error::ParseError;
pub use states::State;
pub use token::{Doctype, ProcessingInstruction, Tag, Token};

/// U+FFFD REPLACEMENT CHARACTER.
const REPLACEMENT_CHAR: char = '\u{FFFD}';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TagKind {
    Start,
    End,
}

pub struct Tokenizer {
    /// Flujo de entrada ya preprocesado (newlines normalizados), en code points: puede tener
    /// surrogates sueltos (`document.write` con strings de JavaScript).
    input: Vec<u32>,
    /// Índice del *next input character*.
    pos: usize,
    /// El *current input character*; `None` representa EOF. Un surrogate suelto aparece aquí
    /// como U+FFFD (todos los estados lo tratan como "anything else"); su valor real queda en
    /// `current_raw`.
    current: Option<char>,
    /// El code point real del *current input character*.
    current_raw: Option<u32>,
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
    /// La entrada está completa: al final de `input` viene el EOF (el *explicit "EOF"
    /// character* de un parser creado por script, o el fin de los datos de red).
    closed: bool,
    /// Límite de consumo: el *insertion point* cuando el parser procesa lo que escribió
    /// `document.write()` (13.2.3.5, §8.4.3). Más allá no hay que leer, ni es EOF.
    limit: Option<usize>,
    /// Un CR al final del último trozo agregado: si el siguiente empieza con LF, se descarta.
    pending_cr: bool,
    /// El paso actual se topó con el final de la entrada disponible sin que sea EOF: su
    /// resultado no vale y se deshace (ver [`Tokenizer::next_token`]).
    starved: Cell<bool>,
}

/// Estado de la máquina al empezar a buscar un token, para deshacer los pasos que se toparon
/// con el final de la entrada disponible.
struct Snapshot {
    pos: usize,
    current: Option<char>,
    current_raw: Option<u32>,
    reconsume: bool,
    checked: usize,
    state: State,
    return_state: State,
    temp: String,
    char_ref_code: u32,
    tag_kind: TagKind,
    tag: Tag,
    attr_active: bool,
    attr_duplicate: bool,
    attr_name: String,
    attr_value: String,
    comment: String,
    doctype: Doctype,
    pi: ProcessingInstruction,
    last_start_tag: Option<String>,
    errors: usize,
}

impl Tokenizer {
    pub fn new(input: &str) -> Self {
        Self::from_code_points(input.chars().map(u32::from))
    }

    /// Tokenizador sobre code points arbitrarios, surrogates sueltos incluidos (13.2.3.5).
    pub fn from_code_points(input: impl IntoIterator<Item = u32>) -> Self {
        Tokenizer {
            input: preprocess(input),
            pos: 0,
            current: None,
            current_raw: None,
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
            closed: true,
            limit: None,
            pending_cr: false,
            starved: Cell::new(false),
        }
    }

    /// Tokenizador sobre una entrada que llega por partes ([`Tokenizer::append`]) y termina con
    /// [`Tokenizer::close`].
    pub fn streaming() -> Self {
        let mut t = Self::from_code_points([]);
        t.closed = false;
        t
    }

    /// Agrega code points al final de la entrada (datos de red).
    pub fn append(&mut self, input: impl IntoIterator<Item = u32>) {
        let mut chunk = input.into_iter().peekable();
        if std::mem::take(&mut self.pending_cr) && chunk.peek() == Some(&0x0A) {
            chunk.next();
        }
        let chunk: Vec<u32> = chunk.collect();
        self.pending_cr = chunk.last() == Some(&0x0D);
        self.input.extend(preprocess(chunk));
    }

    /// Inserta code points en la posición `at` de la entrada ("insert input into the input
    /// stream just before the insertion point"); devuelve cuántos quedaron tras normalizar.
    pub fn insert(&mut self, at: usize, input: impl IntoIterator<Item = u32>) -> usize {
        debug_assert!(at >= self.pos && at <= self.input.len());
        let chunk = preprocess(input);
        let len = chunk.len();
        self.input.splice(at..at, chunk);
        self.checked = self.checked.min(at);
        len
    }

    /// Fin de la entrada: lo que sigue a lo ya agregado es EOF.
    pub fn close(&mut self) {
        self.closed = true;
        self.pending_cr = false;
    }

    /// ¿Ya llegó el fin de la entrada ([`Tokenizer::close`])?
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Fija (o quita) el punto hasta el que se puede consumir.
    pub fn set_limit(&mut self, limit: Option<usize>) {
        self.limit = limit;
    }

    /// Longitud actual de la entrada (en code points ya preprocesados).
    pub fn len(&self) -> usize {
        self.input.len()
    }

    /// ¿La entrada está vacía?
    pub fn is_empty(&self) -> bool {
        self.input.is_empty()
    }

    /// ¿Ya emitió el token de EOF?
    pub fn eof_emitted(&self) -> bool {
        self.eof_emitted
    }

    /// Fin de lo que se puede leer ahora.
    fn end(&self) -> usize {
        self.limit
            .map_or(self.input.len(), |l| l.min(self.input.len()))
    }

    /// ¿El final de lo legible es el EOF de verdad?
    fn at_real_end(&self) -> bool {
        self.closed && self.limit.is_none()
    }

    /// Lee el code point `i` de la entrada legible; más allá del final devuelve `None` y, si no
    /// es EOF, marca el paso como incompleto.
    pub(crate) fn peek_at(&self, i: usize) -> Option<u32> {
        if i < self.end() {
            Some(self.input[i])
        } else {
            if !self.at_real_end() {
                self.starved.set(true);
            }
            None
        }
    }

    /// La entrada legible desde `start`, y si después de ella viene el EOF de verdad.
    pub(crate) fn readable_from(&self, start: usize) -> (&[u32], bool) {
        (&self.input[start..self.end()], self.at_real_end())
    }

    /// Marca el paso actual como incompleto (necesita entrada que todavía no llegó).
    pub(crate) fn mark_starved(&self) {
        self.starved.set(true);
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            pos: self.pos,
            current: self.current,
            current_raw: self.current_raw,
            reconsume: self.reconsume,
            checked: self.checked,
            state: self.state,
            return_state: self.return_state,
            temp: self.temp.clone(),
            char_ref_code: self.char_ref_code,
            tag_kind: self.tag_kind,
            tag: self.tag.clone(),
            attr_active: self.attr_active,
            attr_duplicate: self.attr_duplicate,
            attr_name: self.attr_name.clone(),
            attr_value: self.attr_value.clone(),
            comment: self.comment.clone(),
            doctype: self.doctype.clone(),
            pi: self.pi.clone(),
            last_start_tag: self.last_start_tag.clone(),
            errors: self.errors.len(),
        }
    }

    fn restore(&mut self, s: Snapshot) {
        self.pos = s.pos;
        self.current = s.current;
        self.current_raw = s.current_raw;
        self.reconsume = s.reconsume;
        self.checked = s.checked;
        self.state = s.state;
        self.return_state = s.return_state;
        self.temp = s.temp;
        self.char_ref_code = s.char_ref_code;
        self.tag_kind = s.tag_kind;
        self.tag = s.tag;
        self.attr_active = s.attr_active;
        self.attr_duplicate = s.attr_duplicate;
        self.attr_name = s.attr_name;
        self.attr_value = s.attr_value;
        self.comment = s.comment;
        self.doctype = s.doctype;
        self.pi = s.pi;
        self.last_start_tag = s.last_start_tag;
        self.errors.truncate(s.errors);
        self.queue.clear();
        self.eof_emitted = false;
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

    /// Devuelve el siguiente token, o `None` después de haber devuelto `Token::Eof` o cuando
    /// la entrada disponible (hasta el límite) no alcanza para completar uno: en ese caso el
    /// estado queda como antes de empezarlo, y se retoma al llegar más entrada.
    pub fn next_token(&mut self) -> Option<Token> {
        if let Some(token) = self.queue.pop_front() {
            return Some(token);
        }
        if self.eof_emitted {
            return None;
        }
        if self.at_real_end() {
            while self.queue.is_empty() {
                self.step();
            }
            return self.queue.pop_front();
        }
        let snapshot = self.snapshot();
        while self.queue.is_empty() {
            self.step();
            if self.starved.replace(false) {
                self.restore(snapshot);
                return None;
            }
        }
        self.queue.pop_front()
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
        let raw = self.peek_at(self.pos);
        if let Some(cp) = raw {
            if self.pos >= self.checked {
                self.checked = self.pos + 1;
                self.check_input_character(cp, self.pos);
            }
            self.pos += 1;
        }
        let c = raw.map(|cp| char::from_u32(cp).unwrap_or(REPLACEMENT_CHAR));
        self.current = c;
        self.current_raw = raw;
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
            .all(|(i, expected)| match self.peek_at(i) {
                Some(cp) => {
                    cp == u32::from(expected)
                        || (ignore_ascii_case
                            && char::from_u32(cp)
                                .is_some_and(|c| c.eq_ignore_ascii_case(&expected)))
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
    fn check_input_character(&mut self, cp: u32, offset: usize) {
        if (0xD800..=0xDFFF).contains(&cp) {
            self.errors.push(ParseError {
                code: "surrogate-in-input-stream",
                offset,
            });
            return;
        }
        let is_control = cp <= 0x1F || (0x7F..=0x9F).contains(&cp);
        let is_ascii_whitespace = matches!(cp, 0x09 | 0x0A | 0x0C | 0x0D | 0x20);
        if is_control && !is_ascii_whitespace && cp != 0 {
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

    /// Emite el *current input character* `c`: si en la entrada era un surrogate suelto, como
    /// [`Token::LoneSurrogate`] con su valor real.
    fn emit_current(&mut self, c: char) {
        match self.current_raw {
            Some(cp) if c == REPLACEMENT_CHAR && (0xD800..=0xDFFF).contains(&cp) => {
                self.emit(Token::LoneSurrogate(cp as u16));
            }
            _ => self.emit_char(c),
        }
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
fn preprocess(input: impl IntoIterator<Item = u32>) -> Vec<u32> {
    let mut out = Vec::new();
    let mut chars = input.into_iter().peekable();
    while let Some(c) = chars.next() {
        if c == 0x0D {
            if chars.peek() == Some(&0x0A) {
                chars.next();
            }
            out.push(0x0A);
        } else {
            out.push(c);
        }
    }
    out
}

/// Busca el identificador más largo de la tabla de named character references que sea prefijo
/// de `input`. Devuelve su longitud (en caracteres) y el texto al que se expande, y si se acabó
/// `input` con identificadores aún posibles (con más entrada, el resultado podría cambiar).
///
/// La tabla está ordenada, así que se mantiene el rango `[lo, hi)` de entradas que comparten el
/// prefijo leído hasta ahora y se estrecha carácter a carácter: O(len · log n).
fn match_named_entity(input: &[u32]) -> (Option<(usize, &'static str)>, bool) {
    let table = named_entities::NAMED_ENTITIES;
    let (mut lo, mut hi) = (0, table.len());
    let mut best = None;
    for (k, &c) in input.iter().enumerate() {
        // Todos los identificadores son ASCII.
        let Ok(byte) = u8::try_from(c) else {
            return (best, false);
        };
        let range = &table[lo..hi];
        // Dentro del rango todas las entradas empiezan con el prefijo de longitud `k`; la
        // única entrada de longitud `k` (si existe) va primero.
        let byte_at = |name: &str| name.as_bytes().get(k).copied();
        let start = range.partition_point(|(name, _)| byte_at(name).is_none_or(|b| b < byte));
        let end = range.partition_point(|(name, _)| byte_at(name).is_none_or(|b| b <= byte));
        (lo, hi) = (lo + start, lo + end);
        if lo == hi {
            return (best, false);
        }
        let (name, value) = table[lo];
        if name.len() == k + 1 {
            best = Some((k + 1, value));
        }
    }
    (best, true)
}

/// <https://infra.spec.whatwg.org/#noncharacter>
fn is_noncharacter(cp: u32) -> bool {
    (0xFDD0..=0xFDEF).contains(&cp) || (cp & 0xFFFE) == 0xFFFE
}
