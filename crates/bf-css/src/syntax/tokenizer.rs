//! Tokenizador CSS conforme a CSS Syntax Level 3 §3.3 (preprocesado) y §4 (tokenización).

use std::fmt;

/// Valor numérico de `<number-token>`, `<percentage-token>` y `<dimension-token>`.
#[derive(Debug, Clone, PartialEq)]
pub struct Numeric {
    pub value: f64,
    /// Type flag: `true` = "integer", `false` = "number".
    pub integer: bool,
    /// Sign character: `Some('+')`, `Some('-')` o ausente.
    pub sign: Option<char>,
    /// Texto original del número (sin `%` ni unidad).
    pub repr: String,
}

impl Numeric {
    /// El valor como entero, si el type flag es "integer" (saturado al rango de `i32`).
    pub fn as_integer(&self) -> Option<i32> {
        self.integer
            .then(|| self.value.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Ident(String),
    Function(String),
    AtKeyword(String),
    Hash {
        value: String,
        /// Type flag "id" (si no, "unrestricted").
        id: bool,
    },
    String(String),
    BadString,
    Url(String),
    BadUrl,
    Delim(char),
    Number(Numeric),
    Percentage(Numeric),
    Dimension(Numeric, String),
    UnicodeRange {
        start: u32,
        end: u32,
    },
    Whitespace,
    Cdo,
    Cdc,
    Colon,
    Semicolon,
    Comma,
    LeftBracket,
    RightBracket,
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
}

/// Un token con su posición: offsets en bytes sobre la entrada ya preprocesada.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub start: usize,
    pub end: usize,
}

impl fmt::Display for TokenKind {
    /// Representación aproximada en CSS (para mensajes y volcados; no es la serialización
    /// del spec).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenKind::Ident(v) => write!(f, "{v}"),
            TokenKind::Function(v) => write!(f, "{v}("),
            TokenKind::AtKeyword(v) => write!(f, "@{v}"),
            TokenKind::Hash { value, .. } => write!(f, "#{value}"),
            TokenKind::String(v) => write!(f, "{v:?}"),
            TokenKind::BadString => write!(f, "<bad-string>"),
            TokenKind::Url(v) => write!(f, "url({v})"),
            TokenKind::BadUrl => write!(f, "<bad-url>"),
            TokenKind::Delim(c) => write!(f, "{c}"),
            TokenKind::Number(n) => write!(f, "{}", n.repr),
            TokenKind::Percentage(n) => write!(f, "{}%", n.repr),
            TokenKind::Dimension(n, unit) => write!(f, "{}{unit}", n.repr),
            TokenKind::UnicodeRange { start, end } => write!(f, "U+{start:X}-{end:X}"),
            TokenKind::Whitespace => write!(f, " "),
            TokenKind::Cdo => write!(f, "<!--"),
            TokenKind::Cdc => write!(f, "-->"),
            TokenKind::Colon => write!(f, ":"),
            TokenKind::Semicolon => write!(f, ";"),
            TokenKind::Comma => write!(f, ","),
            TokenKind::LeftBracket => write!(f, "["),
            TokenKind::RightBracket => write!(f, "]"),
            TokenKind::LeftParen => write!(f, "("),
            TokenKind::RightParen => write!(f, ")"),
            TokenKind::LeftBrace => write!(f, "{{"),
            TokenKind::RightBrace => write!(f, "}}"),
        }
    }
}

/// Filter code points (§3.3): CR, FF y CRLF → LF; NULL y surrogates → U+FFFD. Los surrogates
/// no son representables en `&str`.
pub fn filter_code_points(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\x0C' => out.push('\n'),
            '\0' => out.push('\u{FFFD}'),
            c => out.push(c),
        }
    }
    out
}

/// Tokeniza `input`, que debe estar ya filtrado (ver [`filter_code_points`]). No incluye el
/// `<EOF-token>`.
pub fn tokenize(input: &str) -> Vec<Token> {
    tokenize_with(input, false)
}

/// Como [`tokenize`], con el flag *unicode ranges allowed*.
pub fn tokenize_with(input: &str, unicode_ranges_allowed: bool) -> Vec<Token> {
    let mut tokenizer = Tokenizer::new(input, unicode_ranges_allowed);
    let mut tokens = Vec::new();
    while let Some(token) = tokenizer.consume_token() {
        tokens.push(token);
    }
    tokens
}

struct Tokenizer<'a> {
    input: &'a str,
    chars: Vec<char>,
    /// Offset en bytes de cada carácter (más uno final).
    offsets: Vec<usize>,
    pos: usize,
    unicode_ranges_allowed: bool,
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || is_non_ascii_ident(c)
}

fn is_ident_char(c: char) -> bool {
    is_ident_start(c) || c.is_ascii_digit() || c == '-'
}

/// Non-ASCII ident code point.
fn is_non_ascii_ident(c: char) -> bool {
    matches!(c as u32,
        0xB7 | 0xC0..=0xD6 | 0xD8..=0xF6 | 0xF8..=0x37D | 0x37F..=0x1FFF | 0x200C | 0x200D
        | 0x203F | 0x2040 | 0x2070..=0x218F | 0x2C00..=0x2FEF | 0x3001..=0xD7FF
        | 0xF900..=0xFDCF | 0xFDF0..=0xFFFD | 0x10000..)
}

fn is_whitespace(c: char) -> bool {
    matches!(c, '\n' | '\t' | ' ')
}

fn is_non_printable(c: char) -> bool {
    matches!(c, '\0'..='\x08' | '\x0B' | '\x0E'..='\x1F' | '\x7F')
}

/// Check if two code points are a valid escape.
fn is_valid_escape(a: Option<char>, b: Option<char>) -> bool {
    a == Some('\\') && b != Some('\n')
}

/// Check if three code points would start an ident sequence.
fn would_start_ident(a: Option<char>, b: Option<char>, c: Option<char>) -> bool {
    match a {
        Some('-') => b.is_some_and(|b| is_ident_start(b) || b == '-') || is_valid_escape(b, c),
        Some(a) if is_ident_start(a) => true,
        Some('\\') => is_valid_escape(a, b),
        _ => false,
    }
}

/// Check if three code points would start a number.
fn would_start_number(a: Option<char>, b: Option<char>, c: Option<char>) -> bool {
    let digit = |x: Option<char>| x.is_some_and(|x| x.is_ascii_digit());
    match a {
        Some('+' | '-') => digit(b) || (b == Some('.') && digit(c)),
        Some('.') => digit(b),
        Some(a) => a.is_ascii_digit(),
        None => false,
    }
}

impl<'a> Tokenizer<'a> {
    fn new(input: &'a str, unicode_ranges_allowed: bool) -> Self {
        let mut chars = Vec::with_capacity(input.len());
        let mut offsets = Vec::with_capacity(input.len() + 1);
        for (i, c) in input.char_indices() {
            chars.push(c);
            offsets.push(i);
        }
        offsets.push(input.len());
        Tokenizer {
            input,
            chars,
            offsets,
            pos: 0,
            unicode_ranges_allowed,
        }
    }

    /// El carácter `n` posiciones adelante del next input code point (`None` = EOF).
    fn peek(&self, n: usize) -> Option<char> {
        self.chars.get(self.pos + n).copied()
    }

    /// Consume y devuelve el next input code point.
    fn next(&mut self) -> Option<char> {
        let c = self.peek(0);
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn reconsume(&mut self) {
        self.pos -= 1;
    }

    /// Los tres code points empezando en el current input code point (recién consumido).
    fn current_three(&self) -> (Option<char>, Option<char>, Option<char>) {
        (
            self.chars.get(self.pos - 1).copied(),
            self.peek(0),
            self.peek(1),
        )
    }

    fn byte(&self, char_index: usize) -> usize {
        self.offsets[char_index]
    }

    /// Consume a token. `None` representa el `<EOF-token>`.
    fn consume_token(&mut self) -> Option<Token> {
        self.consume_comments();
        let start = self.pos;
        let c = self.next()?;
        let kind = match c {
            c if is_whitespace(c) => {
                while self.peek(0).is_some_and(is_whitespace) {
                    self.next();
                }
                TokenKind::Whitespace
            }
            '"' | '\'' => self.consume_string(c),
            '#' => {
                if self.peek(0).is_some_and(is_ident_char)
                    || is_valid_escape(self.peek(0), self.peek(1))
                {
                    let id = would_start_ident(self.peek(0), self.peek(1), self.peek(2));
                    let value = self.consume_ident_sequence();
                    TokenKind::Hash { value, id }
                } else {
                    TokenKind::Delim('#')
                }
            }
            '(' => TokenKind::LeftParen,
            ')' => TokenKind::RightParen,
            '+' | '.' => {
                let (a, b, c2) = self.current_three();
                if would_start_number(a, b, c2) {
                    self.reconsume();
                    self.consume_numeric(start)
                } else {
                    TokenKind::Delim(c)
                }
            }
            ',' => TokenKind::Comma,
            '-' => {
                let (a, b, c2) = self.current_three();
                if would_start_number(a, b, c2) {
                    self.reconsume();
                    self.consume_numeric(start)
                } else if self.peek(0) == Some('-') && self.peek(1) == Some('>') {
                    self.pos += 2;
                    TokenKind::Cdc
                } else if would_start_ident(a, b, c2) {
                    self.reconsume();
                    self.consume_ident_like()
                } else {
                    TokenKind::Delim('-')
                }
            }
            ':' => TokenKind::Colon,
            ';' => TokenKind::Semicolon,
            '<' => {
                if self.peek(0) == Some('!')
                    && self.peek(1) == Some('-')
                    && self.peek(2) == Some('-')
                {
                    self.pos += 3;
                    TokenKind::Cdo
                } else {
                    TokenKind::Delim('<')
                }
            }
            '@' => {
                if would_start_ident(self.peek(0), self.peek(1), self.peek(2)) {
                    TokenKind::AtKeyword(self.consume_ident_sequence())
                } else {
                    TokenKind::Delim('@')
                }
            }
            '[' => TokenKind::LeftBracket,
            '\\' => {
                if is_valid_escape(Some('\\'), self.peek(0)) {
                    self.reconsume();
                    self.consume_ident_like()
                } else {
                    // Parse error.
                    TokenKind::Delim('\\')
                }
            }
            ']' => TokenKind::RightBracket,
            '{' => TokenKind::LeftBrace,
            '}' => TokenKind::RightBrace,
            c if c.is_ascii_digit() => {
                self.reconsume();
                self.consume_numeric(start)
            }
            'U' | 'u' => {
                let (a, b, c2) = self.current_three();
                if self.unicode_ranges_allowed && would_start_unicode_range(a, b, c2) {
                    self.reconsume();
                    self.consume_unicode_range()
                } else {
                    self.reconsume();
                    self.consume_ident_like()
                }
            }
            c if is_ident_start(c) => {
                self.reconsume();
                self.consume_ident_like()
            }
            c => TokenKind::Delim(c),
        };
        Some(Token {
            kind,
            start: self.byte(start),
            end: self.byte(self.pos),
        })
    }

    /// Consume comments.
    fn consume_comments(&mut self) {
        while self.peek(0) == Some('/') && self.peek(1) == Some('*') {
            self.pos += 2;
            loop {
                match self.next() {
                    None => return, // Parse error.
                    Some('*') if self.peek(0) == Some('/') => {
                        self.pos += 1;
                        break;
                    }
                    Some(_) => {}
                }
            }
        }
    }

    /// Consume a numeric token.
    fn consume_numeric(&mut self, start: usize) -> TokenKind {
        let number = self.consume_number(start);
        if would_start_ident(self.peek(0), self.peek(1), self.peek(2)) {
            let unit = self.consume_ident_sequence();
            TokenKind::Dimension(number, unit)
        } else if self.peek(0) == Some('%') {
            self.next();
            TokenKind::Percentage(number)
        } else {
            TokenKind::Number(number)
        }
    }

    /// Consume an ident-like token.
    fn consume_ident_like(&mut self) -> TokenKind {
        let string = self.consume_ident_sequence();
        if string.eq_ignore_ascii_case("url") && self.peek(0) == Some('(') {
            self.next();
            while self.peek(0).is_some_and(is_whitespace) && self.peek(1).is_some_and(is_whitespace)
            {
                self.next();
            }
            let quote = |c: Option<char>| matches!(c, Some('"' | '\''));
            if quote(self.peek(0))
                || (self.peek(0).is_some_and(is_whitespace) && quote(self.peek(1)))
            {
                TokenKind::Function(string)
            } else {
                self.consume_url()
            }
        } else if self.peek(0) == Some('(') {
            self.next();
            TokenKind::Function(string)
        } else {
            TokenKind::Ident(string)
        }
    }

    /// Consume a string token, terminado por `ending`.
    fn consume_string(&mut self, ending: char) -> TokenKind {
        let mut value = String::new();
        loop {
            match self.next() {
                Some(c) if c == ending => return TokenKind::String(value),
                None => return TokenKind::String(value), // Parse error.
                Some('\n') => {
                    // Parse error.
                    self.reconsume();
                    return TokenKind::BadString;
                }
                Some('\\') => match self.peek(0) {
                    None => {}
                    Some('\n') => {
                        self.next();
                    }
                    Some(_) => value.push(self.consume_escaped()),
                },
                Some(c) => value.push(c),
            }
        }
    }

    /// Consume a url token.
    fn consume_url(&mut self) -> TokenKind {
        let mut value = String::new();
        while self.peek(0).is_some_and(is_whitespace) {
            self.next();
        }
        loop {
            match self.next() {
                Some(')') => return TokenKind::Url(value),
                None => return TokenKind::Url(value), // Parse error.
                Some(c) if is_whitespace(c) => {
                    while self.peek(0).is_some_and(is_whitespace) {
                        self.next();
                    }
                    match self.peek(0) {
                        Some(')') => {
                            self.next();
                            return TokenKind::Url(value);
                        }
                        None => return TokenKind::Url(value), // Parse error.
                        Some(_) => {
                            self.consume_bad_url_remnants();
                            return TokenKind::BadUrl;
                        }
                    }
                }
                Some('"' | '\'' | '(') => {
                    // Parse error.
                    self.consume_bad_url_remnants();
                    return TokenKind::BadUrl;
                }
                Some(c) if is_non_printable(c) => {
                    self.consume_bad_url_remnants();
                    return TokenKind::BadUrl;
                }
                Some('\\') => {
                    if is_valid_escape(Some('\\'), self.peek(0)) {
                        value.push(self.consume_escaped());
                    } else {
                        // Parse error.
                        self.consume_bad_url_remnants();
                        return TokenKind::BadUrl;
                    }
                }
                Some(c) => value.push(c),
            }
        }
    }

    /// Consume an escaped code point (el `\` ya fue consumido).
    fn consume_escaped(&mut self) -> char {
        match self.next() {
            Some(c) if c.is_ascii_hexdigit() => {
                let mut hex = String::from(c);
                while hex.len() < 6 && self.peek(0).is_some_and(|c| c.is_ascii_hexdigit()) {
                    hex.push(self.next().unwrap_or('0'));
                }
                if self.peek(0).is_some_and(is_whitespace) {
                    self.next();
                }
                let value = u32::from_str_radix(&hex, 16).unwrap_or(0);
                if value == 0 || (0xD800..=0xDFFF).contains(&value) || value > 0x10FFFF {
                    '\u{FFFD}'
                } else {
                    char::from_u32(value).unwrap_or('\u{FFFD}')
                }
            }
            None => '\u{FFFD}', // Parse error.
            Some(c) => c,
        }
    }

    /// Consume an ident sequence.
    fn consume_ident_sequence(&mut self) -> String {
        let mut result = String::new();
        loop {
            match self.peek(0) {
                Some(c) if is_ident_char(c) => {
                    self.next();
                    result.push(c);
                }
                Some('\\') if is_valid_escape(Some('\\'), self.peek(1)) => {
                    self.next();
                    result.push(self.consume_escaped());
                }
                _ => return result,
            }
        }
    }

    /// Consume a number. `start` es el índice del primer carácter del número.
    fn consume_number(&mut self, start: usize) -> Numeric {
        let mut integer = true;
        let mut sign = None;
        if let Some(c @ ('+' | '-')) = self.peek(0) {
            self.next();
            sign = Some(c);
        }
        while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
            self.next();
        }
        if self.peek(0) == Some('.') && self.peek(1).is_some_and(|c| c.is_ascii_digit()) {
            self.next();
            while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
                self.next();
            }
            integer = false;
        }
        let number_end = self.pos;
        let mut exponent: Option<i32> = None;
        if matches!(self.peek(0), Some('e' | 'E')) {
            let digit_at = |n: usize| self.peek(n).is_some_and(|c| c.is_ascii_digit());
            let skip = if matches!(self.peek(1), Some('+' | '-')) && digit_at(2) {
                Some(2)
            } else if digit_at(1) {
                Some(1)
            } else {
                None
            };
            if let Some(skip) = skip {
                self.next();
                let negative = skip == 2 && self.next() == Some('-');
                let mut value: i32 = 0;
                while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
                    let d = self.next().and_then(|c| c.to_digit(10)).unwrap_or(0);
                    value = value.saturating_mul(10).saturating_add(d as i32);
                }
                exponent = Some(if negative { -value } else { value });
                integer = false;
            }
        }
        let number_text = &self.input[self.byte(start)..self.byte(number_end)];
        let mut value = number_text.parse::<f64>().unwrap_or(0.0);
        if let Some(exp) = exponent {
            value *= 10f64.powi(exp);
        }
        Numeric {
            value,
            integer,
            sign,
            repr: self.input[self.byte(start)..self.byte(self.pos)].to_owned(),
        }
    }

    /// Consume a unicode-range token.
    fn consume_unicode_range(&mut self) -> TokenKind {
        self.pos += 2;
        let mut first = String::new();
        while first.len() < 6 && self.peek(0).is_some_and(|c| c.is_ascii_hexdigit()) {
            first.push(self.next().unwrap_or('0'));
        }
        while first.len() < 6 && self.peek(0) == Some('?') {
            first.push(self.next().unwrap_or('?'));
        }
        let hex = |s: &str| u32::from_str_radix(s, 16).unwrap_or(0);
        if first.contains('?') {
            return TokenKind::UnicodeRange {
                start: hex(&first.replace('?', "0")),
                end: hex(&first.replace('?', "F")),
            };
        }
        let start = hex(&first);
        if self.peek(0) == Some('-') && self.peek(1).is_some_and(|c| c.is_ascii_hexdigit()) {
            self.next();
            let mut second = String::new();
            while second.len() < 6 && self.peek(0).is_some_and(|c| c.is_ascii_hexdigit()) {
                second.push(self.next().unwrap_or('0'));
            }
            return TokenKind::UnicodeRange {
                start,
                end: hex(&second),
            };
        }
        TokenKind::UnicodeRange { start, end: start }
    }

    /// Consume the remnants of a bad url.
    fn consume_bad_url_remnants(&mut self) {
        loop {
            match self.peek(0) {
                Some(')') => {
                    self.next();
                    return;
                }
                None => return,
                Some('\\') if is_valid_escape(Some('\\'), self.peek(1)) => {
                    self.next();
                    self.consume_escaped();
                }
                Some(_) => {
                    self.next();
                }
            }
        }
    }
}

/// Check if three code points would start a unicode-range.
fn would_start_unicode_range(a: Option<char>, b: Option<char>, c: Option<char>) -> bool {
    matches!(a, Some('U' | 'u'))
        && b == Some('+')
        && c.is_some_and(|c| c == '?' || c.is_ascii_hexdigit())
}
