//! Lexer de ECMAScript (ECMA-262 §12 *ECMAScript Language: Lexical Grammar*, más los
//! comentarios HTML de Annex B.1.1).
//!
//! El código fuente se recorre como secuencia de code points; las posiciones de los tokens son
//! índices de code point y [`Lexer::span`] los convierte a code units UTF-16. El objetivo
//! léxico lo elige el parser (§12, "goal symbols"): por defecto se escanea
//! `InputElementDiv`/`InputElementTemplateTail`; cuando el parser espera una expresión y ve
//! `/` o `/=`, pide [`Lexer::rescan_regex`], y al cerrar una sustitución de template con `}`,
//! [`Lexer::rescan_template`].

mod number;
pub mod unicode_tables;

use crate::ast::Span;
use crate::string::{Atom, JsString};

/// Los puntuadores (§12.8), del más largo al más corto para el "longest match".
const PUNCTUATORS: &[&str] = &[
    ">>>=", "...", "===", "!==", "**=", "<<=", ">>=", ">>>", "&&=", "||=", "??=", "=>", "==",
    "!=", "<=", ">=", "&&", "||", "??", "?.", "++", "--", "+=", "-=", "*=", "%=", "&=", "|=",
    "^=", "/=", "<<", ">>", "**", "{", "}", "(", ")", "[", "]", ";", ",", "<", ">", "+", "-",
    "*", "%", "&", "|", "^", "!", "~", "?", ":", "=", ".", "/",
];

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Eof,
    /// Un IdentifierName (identificador o palabra reservada; el parser decide).
    Name(Atom),
    /// `#nombre`.
    Private(Atom),
    Punct(&'static str),
    Number {
        value: f64,
        /// `010` o `08`: prohibido en código estricto (§12.9.3.1).
        legacy_octal: bool,
    },
    BigInt(String),
    String {
        value: JsString,
        /// Posición del primer escape octal legado o `\8`/`\9` (prohibidos en estricto).
        octal: Option<usize>,
    },
    Template {
        cooked: Option<JsString>,
        raw: String,
        /// `true` en `TemplateTail` o `NoSubstitutionTemplate`.
        tail: bool,
        /// El primer escape inválido (error si el template no tiene tag).
        invalid_escape: Option<(usize, &'static str)>,
    },
    Regex {
        pattern: String,
        flags: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub tok: Tok,
    /// Índices de code point.
    pub start: usize,
    pub end: usize,
    /// Hubo un LineTerminator entre el token anterior y éste.
    pub nl_before: bool,
    /// El IdentifierName tenía escapes `\u`: no puede ser una palabra clave.
    pub escaped: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LexError {
    pub message: String,
    pub pos: usize,
}

type LResult<T> = Result<T, LexError>;

fn in_ranges(c: char, ranges: &[(u32, u32)]) -> bool {
    let c = c as u32;
    ranges
        .binary_search_by(|&(a, b)| {
            if c < a {
                std::cmp::Ordering::Greater
            } else if c > b {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// IdentifierStartChar (§12.7): `ID_Start`, `$` y `_`.
pub fn is_id_start(c: char) -> bool {
    c.is_ascii_alphabetic()
        || c == '$'
        || c == '_'
        || (!c.is_ascii() && in_ranges(c, unicode_tables::ID_START))
}

/// IdentifierPartChar (§12.7): `ID_Continue`, `$`, ZWNJ y ZWJ.
pub fn is_id_part(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || c == '$'
        || c == '_'
        || c == '\u{200C}'
        || c == '\u{200D}'
        || (!c.is_ascii() && in_ranges(c, unicode_tables::ID_CONTINUE))
}

/// WhiteSpace (§12.2): TAB, VT, FF, ZWNBSP y `Zs`.
pub fn is_whitespace(c: char) -> bool {
    matches!(c, '\t' | '\u{B}' | '\u{C}' | ' ' | '\u{A0}' | '\u{FEFF}')
        || (!c.is_ascii() && in_ranges(c, unicode_tables::SPACE_SEPARATOR))
}

/// LineTerminator (§12.3).
pub fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

fn hex_value(c: char) -> Option<u32> {
    c.to_digit(16)
}

pub struct Lexer {
    chars: Vec<char>,
    /// Offset UTF-16 de cada code point (y uno más al final).
    offsets: Vec<u32>,
    pos: usize,
    module: bool,
}

impl Lexer {
    pub fn new(src: &str, module: bool) -> Lexer {
        let chars: Vec<char> = src.chars().collect();
        let mut offsets = Vec::with_capacity(chars.len() + 1);
        let mut o = 0u32;
        for c in &chars {
            offsets.push(o);
            o += c.len_utf16() as u32;
        }
        offsets.push(o);
        Lexer {
            chars,
            offsets,
            pos: 0,
            module,
        }
    }

    pub fn span(&self, start: usize, end: usize) -> Span {
        Span::new(self.offsets[start], self.offsets[end.min(self.chars.len())])
    }

    pub fn offset(&self, pos: usize) -> u32 {
        self.offsets[pos.min(self.chars.len())]
    }

    /// Línea y columna (desde 1, columna en code points) de una posición.
    pub fn line_col(&self, pos: usize) -> (u32, u32) {
        let mut line = 1;
        let mut col = 1;
        let mut i = 0;
        while i < pos.min(self.chars.len()) {
            let c = self.chars[i];
            if c == '\r' && self.chars.get(i + 1) == Some(&'\n') {
                i += 1;
            }
            if is_line_terminator(c) {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
            i += 1;
        }
        (line, col)
    }

    /// El texto fuente entre dos posiciones.
    pub fn slice(&self, start: usize, end: usize) -> String {
        self.chars[start..end.min(self.chars.len())].iter().collect()
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.chars.get(self.pos + n).copied()
    }

    fn starts_with(&self, s: &str) -> bool {
        let mut i = self.pos;
        for c in s.chars() {
            if self.chars.get(i) != Some(&c) {
                return false;
            }
            i += 1;
        }
        true
    }

    fn err<T>(&self, pos: usize, message: impl Into<String>) -> LResult<T> {
        Err(LexError {
            message: message.into(),
            pos,
        })
    }

    /// Salta espacios, terminadores y comentarios; devuelve si hubo un LineTerminator.
    fn skip_trivia(&mut self) -> LResult<bool> {
        let mut newline = false;
        // Hashbang (§12.5), solo al principio del código.
        if self.pos == 0 && self.starts_with("#!") {
            while let Some(c) = self.peek() {
                if is_line_terminator(c) {
                    break;
                }
                self.pos += 1;
            }
        }
        while let Some(c) = self.peek() {
            if is_whitespace(c) {
                self.pos += 1;
            } else if is_line_terminator(c) {
                newline = true;
                self.pos += 1;
            } else if c == '/' && self.peek_at(1) == Some('/') {
                self.skip_line();
            } else if c == '/' && self.peek_at(1) == Some('*') {
                let start = self.pos;
                self.pos += 2;
                loop {
                    match self.peek() {
                        None => return self.err(start, "Unterminated comment"),
                        Some('*') if self.peek_at(1) == Some('/') => {
                            self.pos += 2;
                            break;
                        }
                        Some(c) => {
                            if is_line_terminator(c) {
                                newline = true;
                            }
                            self.pos += 1;
                        }
                    }
                }
            } else if !self.module && self.starts_with("<!--") {
                // SingleLineHTMLOpenComment (B.1.1).
                self.skip_line();
            } else if !self.module && newline && self.starts_with("-->") {
                // SingleLineHTMLCloseComment: solo después de un LineTerminator.
                self.skip_line();
            } else {
                break;
            }
        }
        Ok(newline)
    }

    fn skip_line(&mut self) {
        while let Some(c) = self.peek() {
            if is_line_terminator(c) {
                break;
            }
            self.pos += 1;
        }
    }

    /// El siguiente token con el objetivo `InputElementDiv`.
    pub fn next_token(&mut self) -> LResult<Token> {
        let nl_before = self.skip_trivia()?;
        let start = self.pos;
        let Some(c) = self.peek() else {
            return Ok(Token {
                tok: Tok::Eof,
                start,
                end: start,
                nl_before,
                escaped: false,
            });
        };
        let mut escaped = false;
        let tok = if is_id_start(c) || c == '\\' {
            let (name, esc) = self.identifier_name()?;
            escaped = esc;
            Tok::Name(Atom::new(&name))
        } else if c == '#' {
            self.pos += 1;
            match self.peek() {
                Some(c) if is_id_start(c) || c == '\\' => {
                    let (name, esc) = self.identifier_name()?;
                    escaped = esc;
                    Tok::Private(Atom::new(&name))
                }
                _ => return self.err(start, "Invalid or unexpected token"),
            }
        } else if c.is_ascii_digit() || (c == '.' && self.peek_at(1).is_some_and(|d| d.is_ascii_digit())) {
            self.number()?
        } else if c == '"' || c == '\'' {
            self.string(c)?
        } else if c == '`' {
            self.pos += 1;
            self.template_chars(start)?
        } else {
            let mut found = None;
            for p in PUNCTUATORS {
                if self.starts_with(p) {
                    // `?.` seguido de un dígito es `?` y un número (`a?.5:b`).
                    if *p == "?." && self.peek_at(2).is_some_and(|d| d.is_ascii_digit()) {
                        continue;
                    }
                    found = Some(*p);
                    break;
                }
            }
            match found {
                Some(p) => {
                    self.pos += p.chars().count();
                    Tok::Punct(p)
                }
                None => return self.err(start, "Invalid or unexpected token"),
            }
        };
        Ok(Token {
            tok,
            start,
            end: self.pos,
            nl_before,
            escaped,
        })
    }

    /// IdentifierName con escapes `\uXXXX`/`\u{...}` (§12.7). Devuelve el nombre y si tenía
    /// escapes.
    fn identifier_name(&mut self) -> LResult<(String, bool)> {
        let mut name = String::new();
        let mut escaped = false;
        let mut first = true;
        loop {
            let Some(c) = self.peek() else { break };
            let ch = if c == '\\' {
                let at = self.pos;
                if self.peek_at(1) != Some('u') {
                    return self.err(at, "Invalid Unicode escape sequence");
                }
                self.pos += 2;
                let cp = self.unicode_escape_body(at)?;
                let Some(ch) = char::from_u32(cp) else {
                    return self.err(at, "Invalid Unicode escape sequence");
                };
                let ok = if first { is_id_start(ch) } else { is_id_part(ch) };
                if !ok {
                    return self.err(at, "Invalid Unicode escape sequence");
                }
                escaped = true;
                ch
            } else if (first && is_id_start(c)) || (!first && is_id_part(c)) {
                self.pos += 1;
                c
            } else {
                break;
            };
            name.push(ch);
            first = false;
        }
        Ok((name, escaped))
    }

    /// Lo que sigue a `\u`: `XXXX` o `{X...}` (≤ 10FFFF).
    fn unicode_escape_body(&mut self, at: usize) -> LResult<u32> {
        if self.peek() == Some('{') {
            self.pos += 1;
            let mut value: u32 = 0;
            let mut digits = 0;
            while let Some(h) = self.peek().and_then(hex_value) {
                value = value.saturating_mul(16).saturating_add(h);
                if value > 0x10FFFF {
                    return self.err(at, "Undefined Unicode code-point");
                }
                digits += 1;
                self.pos += 1;
            }
            if digits == 0 || self.peek() != Some('}') {
                return self.err(at, "Invalid Unicode escape sequence");
            }
            self.pos += 1;
            Ok(value)
        } else {
            let mut value = 0;
            for _ in 0..4 {
                let Some(h) = self.peek().and_then(hex_value) else {
                    return self.err(at, "Invalid Unicode escape sequence");
                };
                value = value * 16 + h;
                self.pos += 1;
            }
            Ok(value)
        }
    }

    fn number(&mut self) -> LResult<Tok> {
        let start = self.pos;
        let tok = number::scan(&self.chars, &mut self.pos).map_err(|(pos, m)| LexError {
            message: m.to_owned(),
            pos,
        })?;
        // §12.9.3: "The SourceCharacter immediately following a NumericLiteral must not be
        // an IdentifierStart or DecimalDigit."
        if let Some(c) = self.peek()
            && (is_id_start(c) || c.is_ascii_digit() || c == '\\')
        {
            return self.err(start, "Invalid or unexpected token");
        }
        Ok(tok)
    }

    /// Un escape después de `\` en un string o template. Devuelve las unidades (vacío para
    /// LineContinuation) o el error de un escape inválido.
    fn escape(&mut self, units: &mut Vec<u16>, template: bool) -> Result<EscapeKind, (usize, &'static str)> {
        let at = self.pos - 1;
        let Some(c) = self.peek() else {
            return Err((at, "Invalid or unexpected token"));
        };
        self.pos += 1;
        let simple = |u: u16, units: &mut Vec<u16>| {
            units.push(u);
            Ok(EscapeKind::Normal)
        };
        match c {
            'b' => simple(0x08, units),
            't' => simple(0x09, units),
            'n' => simple(0x0A, units),
            'v' => simple(0x0B, units),
            'f' => simple(0x0C, units),
            'r' => simple(0x0D, units),
            '\r' => {
                if self.peek() == Some('\n') {
                    self.pos += 1;
                }
                Ok(EscapeKind::Normal)
            }
            '\n' | '\u{2028}' | '\u{2029}' => Ok(EscapeKind::Normal),
            'x' => {
                let (Some(a), Some(b)) = (
                    self.peek().and_then(hex_value),
                    self.peek_at(1).and_then(hex_value),
                ) else {
                    return Err((at, "Invalid hexadecimal escape sequence"));
                };
                self.pos += 2;
                units.push((a * 16 + b) as u16);
                Ok(EscapeKind::Normal)
            }
            'u' => {
                let save = self.pos;
                match self.unicode_escape_body(at) {
                    Ok(cp) => {
                        push_code_point(units, cp);
                        Ok(EscapeKind::Normal)
                    }
                    Err(e) => {
                        self.pos = save;
                        let msg = if e.message.contains("code-point") {
                            "Undefined Unicode code-point"
                        } else {
                            "Invalid Unicode escape sequence"
                        };
                        Err((at, msg))
                    }
                }
            }
            '0' if !self.peek().is_some_and(|d| d.is_ascii_digit()) => {
                units.push(0);
                Ok(EscapeKind::Normal)
            }
            '0'..='7' => {
                if template {
                    return Err((at, "Octal escape sequences are not allowed in template strings"));
                }
                // LegacyOctalEscapeSequence (B.1.2): hasta tres dígitos, valor ≤ 255.
                let mut value = c.to_digit(8).unwrap_or(0);
                let max_len = if c <= '3' { 3 } else { 2 };
                let mut len = 1;
                while len < max_len {
                    match self.peek().and_then(|d| d.to_digit(8)) {
                        Some(d) => {
                            value = value * 8 + d;
                            self.pos += 1;
                            len += 1;
                        }
                        None => break,
                    }
                }
                units.push(value as u16);
                Ok(EscapeKind::Octal)
            }
            '8' | '9' => {
                if template {
                    return Err((at, "\\8 and \\9 are not allowed in template strings"));
                }
                units.push(c as u16);
                Ok(EscapeKind::Octal)
            }
            c => {
                push_code_point(units, c as u32);
                Ok(EscapeKind::Normal)
            }
        }
    }

    fn string(&mut self, quote: char) -> LResult<Tok> {
        let start = self.pos;
        self.pos += 1;
        let mut units = Vec::new();
        let mut octal = None;
        loop {
            let Some(c) = self.peek() else {
                return self.err(start, "Invalid or unexpected token");
            };
            if c == quote {
                self.pos += 1;
                break;
            }
            if c == '\n' || c == '\r' {
                return self.err(start, "Invalid or unexpected token");
            }
            self.pos += 1;
            if c == '\\' {
                let at = self.pos - 1;
                match self.escape(&mut units, false) {
                    Ok(EscapeKind::Octal) => {
                        octal.get_or_insert(at);
                    }
                    Ok(EscapeKind::Normal) => {}
                    Err((pos, m)) => return self.err(pos, m),
                }
            } else {
                push_code_point(&mut units, c as u32);
            }
        }
        Ok(Tok::String {
            value: JsString::from_units(units),
            octal,
        })
    }

    /// TemplateCharacters hasta `` ` `` o `${` (después de `` ` `` o de `}`).
    fn template_chars(&mut self, start: usize) -> LResult<Tok> {
        let mut units = Vec::new();
        let mut raw = String::new();
        let mut invalid = None;
        loop {
            let Some(c) = self.peek() else {
                return self.err(start, "Unterminated template literal");
            };
            if c == '`' {
                self.pos += 1;
                return Ok(self.template_tok(units, raw, true, invalid));
            }
            if c == '$' && self.peek_at(1) == Some('{') {
                self.pos += 2;
                return Ok(self.template_tok(units, raw, false, invalid));
            }
            self.pos += 1;
            if c == '\\' {
                let from = self.pos;
                let result = self.escape(&mut units, true);
                if let Err(e) = result {
                    invalid.get_or_insert(e);
                    // NotEscapeSequence: el raw sigue hasta donde llega el escape inválido.
                    self.skip_invalid_escape(from);
                }
                raw.push('\\');
                // CR y CRLF se normalizan a LF también en el raw (§12.9.6, TRV).
                let slice = &self.chars[from..self.pos];
                for (i, &ch) in slice.iter().enumerate() {
                    if ch == '\r' {
                        if slice.get(i + 1) != Some(&'\n') {
                            raw.push('\n');
                        }
                    } else {
                        raw.push(ch);
                    }
                }
            } else if c == '\r' {
                // TRV/TV: CR y CRLF se normalizan a LF.
                if self.peek() == Some('\n') {
                    self.pos += 1;
                }
                units.push(0x0A);
                raw.push('\n');
            } else {
                push_code_point(&mut units, c as u32);
                raw.push(c);
            }
        }
    }

    /// Avanza sobre lo que forma un NotEscapeSequence (§12.9.6) desde `from` (justo después
    /// de `\`).
    fn skip_invalid_escape(&mut self, from: usize) {
        self.pos = from;
        match self.peek() {
            Some('x') => {
                self.pos += 1;
                if self.peek().and_then(hex_value).is_some() {
                    self.pos += 1;
                }
            }
            Some('u') => {
                self.pos += 1;
                if self.peek() == Some('{') {
                    self.pos += 1;
                    while self.peek().and_then(hex_value).is_some() {
                        self.pos += 1;
                    }
                } else {
                    for _ in 0..3 {
                        if self.peek().and_then(hex_value).is_some() {
                            self.pos += 1;
                        } else {
                            break;
                        }
                    }
                }
            }
            Some(_) => self.pos += 1,
            None => {}
        }
    }

    fn template_tok(
        &self,
        units: Vec<u16>,
        raw: String,
        tail: bool,
        invalid: Option<(usize, &'static str)>,
    ) -> Tok {
        Tok::Template {
            cooked: if invalid.is_some() {
                None
            } else {
                Some(JsString::from_units(units))
            },
            raw,
            tail,
            invalid_escape: invalid,
        }
    }

    /// Re-escanea desde `tok` (que es `/` o `/=`) como RegularExpressionLiteral (§12.9.5).
    pub fn rescan_regex(&mut self, tok: &Token) -> LResult<Token> {
        self.pos = tok.start + 1;
        let mut pattern = String::new();
        let mut in_class = false;
        loop {
            let Some(c) = self.peek() else {
                return self.err(tok.start, "Invalid regular expression: missing /");
            };
            if is_line_terminator(c) {
                return self.err(tok.start, "Invalid regular expression: missing /");
            }
            self.pos += 1;
            if c == '\\' {
                pattern.push(c);
                match self.peek() {
                    Some(n) if !is_line_terminator(n) => {
                        pattern.push(n);
                        self.pos += 1;
                    }
                    _ => return self.err(tok.start, "Invalid regular expression: missing /"),
                }
                continue;
            }
            match c {
                '[' => in_class = true,
                ']' => in_class = false,
                '/' if !in_class => break,
                _ => {}
            }
            pattern.push(c);
        }
        let mut flags = String::new();
        while let Some(c) = self.peek() {
            if c == '\\' {
                return self.err(self.pos, "Invalid regular expression flags");
            }
            if !is_id_part(c) {
                break;
            }
            flags.push(c);
            self.pos += 1;
        }
        Ok(Token {
            tok: Tok::Regex { pattern, flags },
            start: tok.start,
            end: self.pos,
            nl_before: tok.nl_before,
            escaped: false,
        })
    }

    /// Re-escanea desde `tok` (un `}` que cierra una sustitución) como `TemplateMiddle` o
    /// `TemplateTail`.
    pub fn rescan_template(&mut self, tok: &Token) -> LResult<Token> {
        self.pos = tok.start + 1;
        let t = self.template_chars(tok.start)?;
        Ok(Token {
            tok: t,
            start: tok.start,
            end: self.pos,
            nl_before: tok.nl_before,
            escaped: false,
        })
    }

    /// Posición actual (para guardar y restaurar en la lectura anticipada).
    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn set_position(&mut self, pos: usize) {
        self.pos = pos;
    }
}

enum EscapeKind {
    Normal,
    Octal,
}

fn push_code_point(units: &mut Vec<u16>, cp: u32) {
    if cp >= 0x10000 {
        let v = cp - 0x10000;
        units.push(0xD800 + (v >> 10) as u16);
        units.push(0xDC00 + (v & 0x3FF) as u16);
    } else {
        units.push(cp as u16);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Tok> {
        let mut l = Lexer::new(src, false);
        let mut out = Vec::new();
        loop {
            let t = l.next_token().unwrap();
            if t.tok == Tok::Eof {
                return out;
            }
            out.push(t.tok);
        }
    }

    #[test]
    fn unicode_identifier_tables() {
        for w in unicode_tables::ID_START.windows(2) {
            assert!(w[0].1 < w[1].0, "ID_Start ordenado y sin solapes");
        }
        for w in unicode_tables::ID_CONTINUE.windows(2) {
            assert!(w[0].1 < w[1].0);
        }
        assert!(is_id_start('a') && is_id_start('ñ') && is_id_start('ℵ'));
        assert!(!is_id_start('·') && is_id_part('\u{B7}'));
        // ZWNJ/ZWJ: IdentifierPart pero no IdentifierStart (§12.7).
        assert!(is_id_part('\u{200C}') && is_id_part('\u{200D}'));
        assert!(!is_id_start('\u{200C}'));
        assert!(is_whitespace('\u{3000}') && is_whitespace('\u{FEFF}'));
    }

    #[test]
    fn basic_tokens() {
        assert_eq!(
            toks("a >>>= b?.c ?? .5"),
            vec![
                Tok::Name(Atom::new("a")),
                Tok::Punct(">>>="),
                Tok::Name(Atom::new("b")),
                Tok::Punct("?."),
                Tok::Name(Atom::new("c")),
                Tok::Punct("??"),
                Tok::Number {
                    value: 0.5,
                    legacy_octal: false
                },
            ]
        );
        assert_eq!(toks("x?.5:y")[1], Tok::Punct("?"));
        let s = &toks(r#"'\x41\u{1F600}\101'"#)[0];
        let Tok::String { value, octal } = s else {
            panic!()
        };
        assert_eq!(value.to_string(), "A😀A");
        assert_eq!(*octal, Some(9));
    }

    #[test]
    fn html_comments() {
        assert_eq!(toks("a <!-- b\nc").len(), 2);
        assert_eq!(toks("a\n--> b\nc").len(), 2);
        assert_eq!(toks("a --> b").len(), 4);
        let mut m = Lexer::new("a <!-- b", true);
        m.next_token().unwrap();
        assert_eq!(m.next_token().unwrap().tok, Tok::Punct("<"));
    }
}
