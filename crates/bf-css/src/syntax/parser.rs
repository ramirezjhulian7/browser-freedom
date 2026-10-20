//! Parser CSS conforme a CSS Syntax Level 3 §5 (Editor's Draft actual, con la gramática de
//! nesting: los bloques mezclan declaraciones y reglas).
//!
//! El parser opera sobre un *token stream* de [`ComponentValue`]s: al empezar desde texto son
//! todos tokens; los bloques de las reglas se consumen primero como `{}`-block (se conserva su
//! contenido crudo en [`Block::values`]) y luego su contenido se parsea con "consume a block's
//! contents" sobre esos component values. Ambos caminos consumen exactamente los mismos
//! tokens, porque todas las reglas de los algoritmos fuera de `}` y del stop token pasan por
//! "consume a component value".

use super::tokenizer::{Token, TokenKind, filter_code_points, tokenize, tokenize_with};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    /// `{}`-block
    Curly,
    /// `[]`-block
    Square,
    /// `()`-block
    Paren,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub name: String,
    pub value: Vec<ComponentValue>,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SimpleBlock {
    pub kind: BlockKind,
    pub value: Vec<ComponentValue>,
    pub start: usize,
    pub end: usize,
}

/// Component value: un preserved token, una función o un simple block.
#[derive(Debug, Clone, PartialEq)]
pub enum ComponentValue {
    Token(Token),
    Function(Function),
    Block(SimpleBlock),
}

impl ComponentValue {
    pub fn token(&self) -> Option<&TokenKind> {
        match self {
            ComponentValue::Token(t) => Some(&t.kind),
            _ => None,
        }
    }

    pub fn is_whitespace(&self) -> bool {
        matches!(self.token(), Some(TokenKind::Whitespace))
    }

    pub fn start(&self) -> usize {
        match self {
            ComponentValue::Token(t) => t.start,
            ComponentValue::Function(f) => f.start,
            ComponentValue::Block(b) => b.start,
        }
    }

    pub fn end(&self) -> usize {
        match self {
            ComponentValue::Token(t) => t.end,
            ComponentValue::Function(f) => f.end,
            ComponentValue::Block(b) => b.end,
        }
    }

    fn is_open_curly(&self) -> bool {
        matches!(self.token(), Some(TokenKind::LeftBrace))
            || matches!(
                self,
                ComponentValue::Block(SimpleBlock {
                    kind: BlockKind::Curly,
                    ..
                })
            )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Declaration {
    pub name: String,
    pub value: Vec<ComponentValue>,
    pub important: bool,
    /// Texto original del valor; solo se captura para custom properties (`--*`).
    pub original_text: Option<String>,
}

/// Contenido de un `{}`-block de una regla.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BlockContents {
    /// La primera lista de declaraciones del bloque.
    pub declarations: Vec<Declaration>,
    /// El resto, en orden: reglas y listas de declaraciones posteriores a una regla
    /// (*nested declarations rules*).
    pub rules: Vec<NestedItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NestedItem {
    Rule(Rule),
    Declarations(Vec<Declaration>),
}

/// Bloque de una regla: el contenido crudo y su interpretación como block contents.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub values: Vec<ComponentValue>,
    pub contents: BlockContents,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QualifiedRule {
    pub prelude: Vec<ComponentValue>,
    pub block: Block,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AtRule {
    pub name: String,
    pub prelude: Vec<ComponentValue>,
    /// `None` para statement at-rules (terminadas en `;`).
    pub block: Option<Block>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Rule {
    Qualified(QualifiedRule),
    At(AtRule),
}

/// Syntax error de las entry points que devuelven un solo valor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxError {
    /// La entrada estaba vacía (solo whitespace).
    Empty,
    /// No se pudo consumir el constructo pedido.
    Invalid,
    /// Sobró entrada después del constructo.
    ExtraInput,
}

// --- Entry points (§5.4) ---

/// Normalize into a token stream: filtra, tokeniza y envuelve cada token.
fn normalize(input: &str) -> (String, Vec<ComponentValue>) {
    let source = filter_code_points(input);
    let values = tokenize(&source)
        .into_iter()
        .map(ComponentValue::Token)
        .collect();
    (source, values)
}

/// Parse a stylesheet (§5.4.3).
pub fn parse_stylesheet(input: &str) -> Vec<Rule> {
    let (source, values) = normalize(input);
    Stream::new(&values, &source).consume_stylesheet_contents()
}

/// Parse a stylesheet's contents (§5.4.4). También se usa como "parse a list of rules".
pub fn parse_rule_list(input: &str) -> Vec<Rule> {
    parse_stylesheet(input)
}

/// Parse a list of declarations (§5.4.5): las at-rules se consumen y se descartan.
pub fn parse_declaration_list(input: &str) -> Vec<Declaration> {
    let (source, values) = normalize(input);
    let mut s = Stream::new(&values, &source);
    let mut decls = Vec::new();
    while !s.is_empty() {
        s.discard_whitespace();
        match s.next_kind() {
            None => {}
            Some(TokenKind::Semicolon) => s.discard(),
            Some(TokenKind::AtKeyword(_)) => {
                s.consume_at_rule(false);
            }
            Some(_) => {
                if let Some(decl) = s.consume_declaration(false) {
                    decls.push(decl);
                }
            }
        }
    }
    decls
}

/// Parse a rule (§5.4.6).
pub fn parse_rule(input: &str) -> Result<Rule, SyntaxError> {
    let (source, values) = normalize(input);
    let mut s = Stream::new(&values, &source);
    s.discard_whitespace();
    let rule = match s.next_kind() {
        None => return Err(SyntaxError::Empty),
        Some(TokenKind::AtKeyword(_)) => s.consume_at_rule(false).map(Rule::At),
        Some(_) => match s.consume_qualified_rule(None, false) {
            Qualified::Rule(rule) => Some(Rule::Qualified(rule)),
            Qualified::Nothing => None,
        },
    };
    let rule = rule.ok_or(SyntaxError::Invalid)?;
    s.discard_whitespace();
    if s.is_empty() {
        Ok(rule)
    } else {
        Err(SyntaxError::ExtraInput)
    }
}

/// Parse a declaration (§5.4.7).
pub fn parse_declaration(input: &str) -> Result<Declaration, SyntaxError> {
    let (source, values) = normalize(input);
    declaration_from(&values, &source)
}

/// Como [`parse_declaration`], sobre component values ya parseados (p. ej. el contenido de
/// un `()`-block en `@supports`).
pub fn parse_declaration_from(values: &[ComponentValue]) -> Result<Declaration, SyntaxError> {
    declaration_from(values, "")
}

fn declaration_from(values: &[ComponentValue], source: &str) -> Result<Declaration, SyntaxError> {
    let mut s = Stream::new(values, source);
    s.discard_whitespace();
    if s.is_empty() {
        return Err(SyntaxError::Empty);
    }
    let decl = s.consume_declaration(false).ok_or(SyntaxError::Invalid)?;
    s.discard_whitespace();
    if s.is_empty() {
        Ok(decl)
    } else {
        Err(SyntaxError::ExtraInput)
    }
}

/// Consume a block's contents sobre toda la entrada (el contenido de un `style=""`, por
/// ejemplo, o de un bloque ya extraído).
pub fn parse_block_contents(input: &str) -> BlockContents {
    let (source, values) = normalize(input);
    Stream::new(&values, &source).consume_block_contents()
}

/// Igual que [`parse_block_contents`], sobre component values ya parseados.
pub fn parse_block_contents_from(values: &[ComponentValue]) -> BlockContents {
    Stream::new(values, "").consume_block_contents()
}

/// Parse a component value (§5.4.8).
pub fn parse_component_value(input: &str) -> Result<ComponentValue, SyntaxError> {
    let (source, values) = normalize(input);
    let mut s = Stream::new(&values, &source);
    s.discard_whitespace();
    if s.is_empty() {
        return Err(SyntaxError::Empty);
    }
    let value = s.consume_component_value();
    s.discard_whitespace();
    if s.is_empty() {
        Ok(value)
    } else {
        Err(SyntaxError::ExtraInput)
    }
}

/// Parse a list of component values (§5.4.9).
pub fn parse_component_value_list(input: &str) -> Vec<ComponentValue> {
    let (source, values) = normalize(input);
    Stream::new(&values, &source).consume_list_of_component_values(None, false)
}

/// Parse a comma-separated list of component values (§5.4.10).
pub fn parse_comma_separated_list(input: &str) -> Vec<Vec<ComponentValue>> {
    let (source, values) = normalize(input);
    split_commas(&values, &source)
}

/// Parse a comma-separated list of component values sobre component values ya parseados.
pub fn split_comma_separated(values: &[ComponentValue]) -> Vec<Vec<ComponentValue>> {
    split_commas(values, "")
}

fn split_commas(values: &[ComponentValue], source: &str) -> Vec<Vec<ComponentValue>> {
    let mut s = Stream::new(values, source);
    let mut groups = Vec::new();
    while !s.is_empty() {
        groups.push(s.consume_list_of_component_values(Some(&TokenKind::Comma), false));
        s.discard();
    }
    groups
}

// --- Token stream (§5.3) y algoritmos (§5.5) ---

/// Resultado de "consume a qualified rule". A nivel sintáctico toda regla es válida en su
/// contexto, así que el "invalid rule error" no se produce: las reglas inválidas (p. ej. con un
/// selector inválido) las descarta después el modelo de hoja de estilos. El efecto sobre la
/// estructura del bloque es el mismo, porque ambos casos cortan la lista de declaraciones.
enum Qualified {
    Nothing,
    Rule(QualifiedRule),
}

struct Stream<'a> {
    items: &'a [ComponentValue],
    index: usize,
    marks: Vec<usize>,
    /// Texto fuente (ya filtrado) al que apuntan los offsets; vacío si no se conoce.
    source: &'a str,
}

impl<'a> Stream<'a> {
    fn new(items: &'a [ComponentValue], source: &'a str) -> Self {
        Stream {
            items,
            index: 0,
            marks: Vec::new(),
            source,
        }
    }

    fn next(&self) -> Option<&'a ComponentValue> {
        self.items.get(self.index)
    }

    fn next_kind(&self) -> Option<&'a TokenKind> {
        self.next().and_then(ComponentValue::token)
    }

    fn is_empty(&self) -> bool {
        self.index >= self.items.len()
    }

    fn consume(&mut self) -> ComponentValue {
        let item = self.items[self.index].clone();
        self.index += 1;
        item
    }

    fn discard(&mut self) {
        if !self.is_empty() {
            self.index += 1;
        }
    }

    fn mark(&mut self) {
        self.marks.push(self.index);
    }

    fn restore_mark(&mut self) {
        self.index = self.marks.pop().expect("mark");
    }

    fn discard_mark(&mut self) {
        self.marks.pop();
    }

    fn discard_whitespace(&mut self) {
        while self.next().is_some_and(ComponentValue::is_whitespace) {
            self.index += 1;
        }
    }

    fn slice_source(&self, start: usize, end: usize) -> Option<&'a str> {
        self.source.get(start..end)
    }

    /// Consume a stylesheet's contents (§5.5.1).
    fn consume_stylesheet_contents(&mut self) -> Vec<Rule> {
        let mut rules = Vec::new();
        loop {
            match self.next_kind() {
                _ if self.is_empty() => return rules,
                Some(TokenKind::Whitespace | TokenKind::Cdo | TokenKind::Cdc) => self.discard(),
                Some(TokenKind::AtKeyword(_)) => {
                    if let Some(rule) = self.consume_at_rule(false) {
                        rules.push(Rule::At(rule));
                    }
                }
                _ => {
                    if let Qualified::Rule(rule) = self.consume_qualified_rule(None, false) {
                        rules.push(Rule::Qualified(rule));
                    }
                }
            }
        }
    }

    /// Consume an at-rule (§5.5.2).
    fn consume_at_rule(&mut self, nested: bool) -> Option<AtRule> {
        let Some(TokenKind::AtKeyword(name)) = self.next_kind() else {
            unreachable!("consume_at_rule requiere un <at-keyword-token>");
        };
        self.discard();
        let mut rule = AtRule {
            name: name.clone(),
            prelude: Vec::new(),
            block: None,
        };
        loop {
            match self.next() {
                None => return Some(rule),
                Some(item) => match item.token() {
                    Some(TokenKind::Semicolon) => {
                        self.discard();
                        return Some(rule);
                    }
                    Some(TokenKind::RightBrace) => {
                        if nested {
                            return Some(rule);
                        }
                        let token = self.consume();
                        rule.prelude.push(token);
                    }
                    _ if item.is_open_curly() => {
                        rule.block = Some(self.consume_block());
                        return Some(rule);
                    }
                    _ => {
                        let value = self.consume_component_value();
                        rule.prelude.push(value);
                    }
                },
            }
        }
    }

    /// Consume a qualified rule (§5.5.3).
    fn consume_qualified_rule(&mut self, stop: Option<&TokenKind>, nested: bool) -> Qualified {
        let mut prelude = Vec::new();
        loop {
            let Some(item) = self.next() else {
                return Qualified::Nothing;
            };
            match item.token() {
                Some(kind) if Some(kind) == stop => return Qualified::Nothing,
                Some(TokenKind::RightBrace) => {
                    if nested {
                        return Qualified::Nothing;
                    }
                    let token = self.consume();
                    prelude.push(token);
                }
                _ if item.is_open_curly() => {
                    if looks_like_custom_property(&prelude) {
                        if nested {
                            self.consume_bad_declaration_remnants(true);
                        } else {
                            self.consume_block();
                        }
                        return Qualified::Nothing;
                    }
                    let block = self.consume_block();
                    // "Si el rule es válido en el contexto actual": a nivel sintáctico, siempre.
                    return Qualified::Rule(QualifiedRule { prelude, block });
                }
                _ => {
                    let value = self.consume_component_value();
                    prelude.push(value);
                }
            }
        }
    }

    /// Consume a block (§5.5.4): consume el `{}`-block y parsea su contenido.
    fn consume_block(&mut self) -> Block {
        let values = match self.consume_component_value() {
            ComponentValue::Block(block) => block.value,
            _ => unreachable!("consume_block requiere un <{{-token>"),
        };
        let contents = Stream::new(&values, self.source).consume_block_contents();
        Block { values, contents }
    }

    /// Consume a block's contents (§5.5.5).
    fn consume_block_contents(&mut self) -> BlockContents {
        let mut items: Vec<NestedItem> = Vec::new();
        let mut decls: Vec<Declaration> = Vec::new();
        let flush = |items: &mut Vec<NestedItem>, decls: &mut Vec<Declaration>| {
            if !decls.is_empty() {
                items.push(NestedItem::Declarations(std::mem::take(decls)));
            }
        };
        loop {
            match self.next_kind() {
                _ if self.is_empty() => break,
                Some(TokenKind::Whitespace | TokenKind::Semicolon) => self.discard(),
                Some(TokenKind::RightBrace) => break,
                Some(TokenKind::AtKeyword(_)) => {
                    flush(&mut items, &mut decls);
                    if let Some(rule) = self.consume_at_rule(true) {
                        items.push(NestedItem::Rule(Rule::At(rule)));
                    }
                }
                _ => {
                    self.mark();
                    if let Some(decl) = self.consume_declaration(true) {
                        decls.push(decl);
                        self.discard_mark();
                    } else {
                        self.restore_mark();
                        match self.consume_qualified_rule(Some(&TokenKind::Semicolon), true) {
                            Qualified::Nothing => {}
                            Qualified::Rule(rule) => {
                                flush(&mut items, &mut decls);
                                items.push(NestedItem::Rule(Rule::Qualified(rule)));
                            }
                        }
                    }
                }
            }
        }
        // El spec dice solo "Return rules"; sin agregar las declaraciones pendientes se
        // perderían las del final del bloque, así que se agregan (errata evidente).
        flush(&mut items, &mut decls);
        let mut items = items.into_iter();
        let mut contents = BlockContents::default();
        match items.next() {
            Some(NestedItem::Declarations(first)) => contents.declarations = first,
            Some(other) => contents.rules.push(other),
            None => {}
        }
        contents.rules.extend(items);
        contents
    }

    /// Consume a declaration (§5.5.6).
    fn consume_declaration(&mut self, nested: bool) -> Option<Declaration> {
        let Some(TokenKind::Ident(name)) = self.next_kind() else {
            self.consume_bad_declaration_remnants(nested);
            return None;
        };
        self.discard();
        self.discard_whitespace();
        if self.next_kind() != Some(&TokenKind::Colon) {
            self.consume_bad_declaration_remnants(nested);
            return None;
        }
        self.discard();
        self.discard_whitespace();
        let mut value = self.consume_list_of_component_values(Some(&TokenKind::Semicolon), nested);
        let full_span = value
            .first()
            .zip(value.last())
            .map(|(f, l)| (f.start(), l.end()));

        let mut important = false;
        let non_ws: Vec<usize> = (0..value.len())
            .filter(|&i| !value[i].is_whitespace())
            .collect();
        if let [.., bang, imp] = non_ws[..]
            && value[bang].token() == Some(&TokenKind::Delim('!'))
            && matches!(value[imp].token(), Some(TokenKind::Ident(v)) if v.eq_ignore_ascii_case("important"))
        {
            value.remove(imp);
            value.remove(bang);
            important = true;
        }
        while value.last().is_some_and(ComponentValue::is_whitespace) {
            value.pop();
        }

        let mut decl = Declaration {
            name: name.clone(),
            value,
            important,
            original_text: None,
        };
        if decl.name.starts_with("--") {
            let text = match (decl.value.first(), decl.value.last()) {
                (Some(first), Some(last)) => self.slice_source(first.start(), last.end()),
                _ => Some(""),
            };
            decl.original_text = text.map(str::to_owned);
        } else if decl.value.iter().any(|v| {
            matches!(
                v,
                ComponentValue::Block(SimpleBlock {
                    kind: BlockKind::Curly,
                    ..
                })
            )
        }) && decl.value.iter().filter(|v| !v.is_whitespace()).count() > 1
        {
            return None;
        } else if decl.name.eq_ignore_ascii_case("unicode-range")
            && let Some((start, end)) = full_span
            && let Some(text) = self.slice_source(start, end)
        {
            // Consume the value of a unicode-range descriptor (§5.5.11).
            let tokens: Vec<ComponentValue> = tokenize_with(text, true)
                .into_iter()
                .map(ComponentValue::Token)
                .collect();
            decl.value = Stream::new(&tokens, text).consume_list_of_component_values(None, false);
        }
        Some(decl)
    }

    /// Consume the remnants of a bad declaration.
    fn consume_bad_declaration_remnants(&mut self, nested: bool) {
        loop {
            match self.next_kind() {
                _ if self.is_empty() => return,
                Some(TokenKind::Semicolon) => {
                    self.discard();
                    return;
                }
                Some(TokenKind::RightBrace) => {
                    if nested {
                        return;
                    }
                    self.discard();
                }
                _ => {
                    self.consume_component_value();
                }
            }
        }
    }

    /// Consume a list of component values (§5.5.7).
    fn consume_list_of_component_values(
        &mut self,
        stop: Option<&TokenKind>,
        nested: bool,
    ) -> Vec<ComponentValue> {
        let mut values = Vec::new();
        loop {
            match self.next_kind() {
                _ if self.is_empty() => return values,
                Some(kind) if Some(kind) == stop => return values,
                Some(TokenKind::RightBrace) => {
                    if nested {
                        return values;
                    }
                    let token = self.consume();
                    values.push(token);
                }
                _ => values.push(self.consume_component_value()),
            }
        }
    }

    /// Consume a component value (§5.5.8).
    fn consume_component_value(&mut self) -> ComponentValue {
        match self.next_kind() {
            Some(TokenKind::LeftBrace) => self.consume_simple_block(BlockKind::Curly),
            Some(TokenKind::LeftBracket) => self.consume_simple_block(BlockKind::Square),
            Some(TokenKind::LeftParen) => self.consume_simple_block(BlockKind::Paren),
            Some(TokenKind::Function(_)) => self.consume_function(),
            _ => self.consume(),
        }
    }

    /// Consume a simple block (§5.5.9).
    fn consume_simple_block(&mut self, kind: BlockKind) -> ComponentValue {
        let ending = match kind {
            BlockKind::Curly => TokenKind::RightBrace,
            BlockKind::Square => TokenKind::RightBracket,
            BlockKind::Paren => TokenKind::RightParen,
        };
        let open = self.consume();
        let mut block = SimpleBlock {
            kind,
            value: Vec::new(),
            start: open.start(),
            end: open.end(),
        };
        loop {
            match self.next() {
                None => return ComponentValue::Block(block),
                Some(item) if item.token() == Some(&ending) => {
                    block.end = item.end();
                    self.discard();
                    return ComponentValue::Block(block);
                }
                Some(_) => {
                    let value = self.consume_component_value();
                    block.end = value.end();
                    block.value.push(value);
                }
            }
        }
    }

    /// Consume a function (§5.5.10).
    fn consume_function(&mut self) -> ComponentValue {
        let token = self.consume();
        let Some(TokenKind::Function(name)) = token.token() else {
            unreachable!("consume_function requiere un <function-token>");
        };
        let mut function = Function {
            name: name.clone(),
            value: Vec::new(),
            start: token.start(),
            end: token.end(),
        };
        loop {
            match self.next() {
                None => return ComponentValue::Function(function),
                Some(item) if item.token() == Some(&TokenKind::RightParen) => {
                    function.end = item.end();
                    self.discard();
                    return ComponentValue::Function(function);
                }
                Some(_) => {
                    let value = self.consume_component_value();
                    function.end = value.end();
                    function.value.push(value);
                }
            }
        }
    }
}

/// ¿Los dos primeros valores no-whitespace son un ident `--*` y un `:`?
fn looks_like_custom_property(prelude: &[ComponentValue]) -> bool {
    let mut non_ws = prelude.iter().filter(|v| !v.is_whitespace());
    matches!(
        (non_ws.next().and_then(ComponentValue::token), non_ws.next().and_then(ComponentValue::token)),
        (Some(TokenKind::Ident(name)), Some(TokenKind::Colon)) if name.starts_with("--")
    )
}
