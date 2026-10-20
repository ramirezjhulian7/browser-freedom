//! Parser de los patrones de RegExp (ECMA-262 §22.2.1 *Patterns* con sus early errors y la
//! gramática extendida de Annex B.1.2 para los patrones sin `u`/`v`). Produce el árbol que
//! compila [`super::compiler`]; los CharSet de las clases se evalúan aquí (CompileToCharSet,
//! §22.2.2.9) con los flags vigentes en cada punto (los modificadores `(?i:…)` cambian
//! IgnoreCase, Multiline y DotAll para su Disjunction).
//!
//! Sin `u`/`v` el patrón se interpreta como code units UTF-16 (un carácter astral son dos
//! unidades, lo que importa en los rangos de clases); con `u`/`v`, como code points.

use super::unicode::{
    CharSet, ClassSet, Property, character_complement, digits, is_string_property, lookup_property,
    maybe_simple_case_folding, white_space, word_characters,
};
use crate::lexer::{is_id_part, is_id_start};

/// Los flags de un RegExp (§22.2.3.1 RegExpInitialize).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flags {
    pub has_indices: bool,
    pub global: bool,
    pub ignore_case: bool,
    pub multiline: bool,
    pub dot_all: bool,
    pub unicode: bool,
    pub unicode_sets: bool,
    pub sticky: bool,
}

impl Flags {
    /// HasEitherUnicodeFlag.
    pub fn either_unicode(&self) -> bool {
        self.unicode || self.unicode_sets
    }
}

/// Valida las flags (§13.2.7.1 y §22.2.3.1): solo `dgimsuvy`, sin repetir, y no `u` con `v`.
pub fn parse_flags(flags: &str) -> Result<Flags, String> {
    let mut f = Flags::default();
    let err = || format!("Invalid regular expression flags '{flags}'");
    for c in flags.chars() {
        let slot = match c {
            'd' => &mut f.has_indices,
            'g' => &mut f.global,
            'i' => &mut f.ignore_case,
            'm' => &mut f.multiline,
            's' => &mut f.dot_all,
            'u' => &mut f.unicode,
            'v' => &mut f.unicode_sets,
            'y' => &mut f.sticky,
            _ => return Err(err()),
        };
        if *slot {
            return Err(err());
        }
        *slot = true;
    }
    if f.unicode && f.unicode_sets {
        return Err(err());
    }
    Ok(f)
}

/// Valida las flags de un literal.
pub fn validate_flags(flags: &str) -> Result<(), String> {
    parse_flags(flags).map(|_| ())
}

/// Valida un patrón con sus flags (`IsValidRegularExpressionLiteral`, §13.2.7.2).
pub fn validate(pattern: &str, flags: &str) -> Result<(), String> {
    let f = parse_flags(flags)?;
    let units: Vec<u16> = pattern.encode_utf16().collect();
    parse(&units, f)
        .map(|_| ())
        .map_err(|m| format!("Invalid regular expression: /{pattern}/{flags}: {m}"))
}

/// Un nodo del patrón. Los flags que afectan a cada nodo (IgnoreCase, Multiline, DotAll) van
/// en el propio nodo.
#[derive(Debug, Clone)]
pub enum Node {
    Empty,
    /// Un carácter (PatternCharacter o un escape de un carácter).
    Char {
        c: u32,
        ic: bool,
    },
    /// `.`
    Dot {
        dot_all: bool,
    },
    /// Una clase o un escape de clase: con `invert`, CharacterSetMatcher invertido; en modo
    /// `v` puede tener strings.
    Set {
        set: ClassSet,
        invert: bool,
        ic: bool,
    },
    Seq(Vec<Node>),
    Alt(Vec<Node>),
    /// Grupo de captura con su índice (desde 1).
    Capture {
        index: u32,
        body: Box<Node>,
    },
    /// `^`
    Start {
        multiline: bool,
    },
    /// `$`
    End {
        multiline: bool,
    },
    /// `\b` (`negate`: `\B`).
    WordBoundary {
        negate: bool,
        ic: bool,
    },
    /// Lookahead / lookbehind.
    Look {
        behind: bool,
        negate: bool,
        body: Box<Node>,
    },
    /// Backreference a los grupos `groups` (varios si el nombre está duplicado).
    BackRef {
        groups: Vec<u32>,
        ic: bool,
    },
    /// Un átomo cuantificado; `first_group`/`group_count` son parenIndex y parenCount.
    Repeat {
        body: Box<Node>,
        min: u64,
        max: Option<u64>,
        greedy: bool,
        first_group: u32,
        group_count: u32,
    },
}

/// Un patrón analizado.
#[derive(Debug, Clone)]
pub struct Pattern {
    pub node: Node,
    /// Cantidad de grupos de captura (sin contar el 0).
    pub group_count: u32,
    /// Nombre de cada grupo (índice 0 = grupo 1).
    pub group_names: Vec<Option<String>>,
}

/// ParsePattern (§22.2.3.4): analiza `pattern` (code units) con `flags`. El error es el
/// mensaje sin el patrón.
pub fn parse(pattern: &[u16], flags: Flags) -> Result<Pattern, String> {
    let unicode = flags.either_unicode();
    let src: Vec<u32> = if unicode {
        decode_code_points(pattern)
    } else {
        pattern.iter().map(|&u| u32::from(u)).collect()
    };
    let total_groups = count_groups(&src, flags.unicode_sets);
    let mut p = Parser {
        s: src,
        i: 0,
        unicode,
        v: flags.unicode_sets,
        named: unicode,
        ic: flags.ignore_case,
        multiline: flags.multiline,
        dot_all: flags.dot_all,
        groups: 0,
        total_groups,
        names: Vec::new(),
        group_names: Vec::new(),
        backrefs: Vec::new(),
        max_backref: 0,
        path: Vec::new(),
        next_disjunction: 0,
        stack_base: crate::parser::stack_position(),
    };
    // Sin `u`, la gramática se reinterpreta con [+NamedCaptureGroups] si el patrón tiene un
    // GroupName (§22.2.3.4, ParsePattern).
    if !unicode {
        p.named = has_group_name(&p.s);
    }
    let node = p.pattern()?;
    Ok(Pattern {
        node,
        group_count: p.groups,
        group_names: p.group_names,
    })
}

/// Los code points de un string UTF-16 (los surrogates sueltos quedan como están).
pub fn decode_code_points(units: &[u16]) -> Vec<u32> {
    let mut out = Vec::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        let u = u32::from(units[i]);
        if (0xD800..0xDC00).contains(&u)
            && let Some(&t) = units.get(i + 1)
            && (0xDC00..0xE000).contains(&u32::from(t))
        {
            out.push(0x10000 + ((u - 0xD800) << 10) + (u32::from(t) - 0xDC00));
            i += 2;
            continue;
        }
        out.push(u);
        i += 1;
    }
    out
}

/// ¿El patrón (sin `u`) tiene un `(?<nombre>`?
fn has_group_name(s: &[u32]) -> bool {
    let mut i = 0;
    let mut in_class = false;
    while i < s.len() {
        let c = s[i];
        if c == ch('\\') {
            i += 2;
            continue;
        }
        if in_class {
            if c == ch(']') {
                in_class = false;
            }
        } else if c == ch('[') {
            in_class = true;
        } else if c == ch('(')
            && s.get(i + 1) == Some(&ch('?'))
            && s.get(i + 2) == Some(&ch('<'))
            && !matches!(s.get(i + 3), Some(&c) if c == ch('=') || c == ch('!'))
        {
            return true;
        }
        i += 1;
    }
    false
}

/// CountLeftCapturingParensWithin: los `(` de grupos de captura fuera de clases y escapes
/// (con `v` las clases se anidan).
fn count_groups(s: &[u32], v: bool) -> u32 {
    let mut i = 0;
    let mut depth = 0u32;
    let mut n = 0;
    while i < s.len() {
        let c = s[i];
        if c == ch('\\') {
            i += 2;
            continue;
        }
        if depth > 0 {
            if c == ch(']') {
                depth -= 1;
            } else if c == ch('[') && v {
                depth += 1;
            }
        } else if c == ch('[') {
            depth = 1;
        } else if c == ch('(')
            && (s.get(i + 1) != Some(&ch('?'))
                || (s.get(i + 2) == Some(&ch('<'))
                    && !matches!(s.get(i + 3), Some(&c) if c == ch('=') || c == ch('!'))))
        {
            n += 1;
        }
        i += 1;
    }
    n
}

struct Parser {
    s: Vec<u32>,
    i: usize,
    /// UnicodeMode (`u` o `v`).
    unicode: bool,
    /// UnicodeSetsMode (`v`).
    v: bool,
    /// NamedCaptureGroups.
    named: bool,
    /// Flags vigentes (cambian con los modificadores).
    ic: bool,
    multiline: bool,
    dot_all: bool,
    groups: u32,
    total_groups: u32,
    /// Grupos con nombre y su "camino" de alternativas (para MightBothParticipate).
    names: Vec<(String, Vec<(u32, u32)>)>,
    group_names: Vec<Option<String>>,
    /// Los nombres de las referencias `\k<…>`, en orden de aparición.
    backrefs: Vec<String>,
    max_backref: u32,
    /// Alternativas en curso: (disyunción, índice de la alternativa).
    path: Vec<(u32, u32)>,
    next_disjunction: u32,
    stack_base: usize,
}

type R<T> = Result<T, String>;

const SYNTAX: &str = "^$\\.*+?()[]{}|";

fn ch(c: char) -> u32 {
    c as u32
}

/// Un elemento de una clase sin `v`: un carácter o una clase de caracteres (`\d`).
enum ClassAtom {
    Char(u32),
    Class(CharSet),
}

/// Un ClassSetOperand: un ClassSetCharacter (que puede empezar un rango) o un conjunto con
/// su MayContainStrings.
enum SetOperand {
    Char(u32),
    Set(ClassSet, bool),
}

impl Parser {
    fn peek(&self) -> Option<u32> {
        self.s.get(self.i).copied()
    }

    fn peek_at(&self, n: usize) -> Option<u32> {
        self.s.get(self.i + n).copied()
    }

    fn is(&self, c: char) -> bool {
        self.peek() == Some(ch(c))
    }

    fn eat(&mut self, c: char) -> bool {
        if self.is(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn looking_at(&self, s: &str) -> bool {
        s.chars()
            .enumerate()
            .all(|(k, c)| self.peek_at(k) == Some(ch(c)))
    }

    fn check_depth(&self) -> R<()> {
        // Anidamiento de grupos y clases: el mismo límite de pila que el parser.
        if self
            .stack_base
            .saturating_sub(crate::parser::stack_position())
            > 256 * 1024
        {
            return Err("Maximum nesting depth exceeded".into());
        }
        Ok(())
    }

    fn pattern(&mut self) -> R<Node> {
        let mut node = self.disjunction()?;
        if self.i < self.s.len() {
            return Err(if self.is(')') {
                "Unmatched ')'".into()
            } else {
                "Unexpected character".into()
            });
        }
        // Early errors de Pattern (§22.2.1.1).
        for name in &self.backrefs {
            if !self.names.iter().any(|(n, _)| n == name) {
                return Err("Invalid named capture referenced".into());
            }
        }
        if self.unicode && self.max_backref > self.groups {
            return Err("Invalid escape".into());
        }
        for (a, (na, pa)) in self.names.iter().enumerate() {
            for (nb, pb) in &self.names[a + 1..] {
                if na == nb && might_both_participate(pa, pb) {
                    return Err("Duplicate capture group name".into());
                }
            }
        }
        let mut k = 0;
        self.fix_named(&mut node, &mut k);
        Ok(node)
    }

    /// Las referencias `\k<nombre>` se crean con `groups` vacío (el grupo puede venir
    /// después); aquí se completan con todos los grupos de ese nombre.
    fn fix_named(&self, node: &mut Node, k: &mut usize) {
        match node {
            Node::BackRef { groups, .. } if groups.is_empty() => {
                let name = &self.backrefs[*k];
                *k += 1;
                for (i, n) in self.group_names.iter().enumerate() {
                    if n.as_deref() == Some(name.as_str()) {
                        groups.push(i as u32 + 1);
                    }
                }
            }
            Node::Seq(v) | Node::Alt(v) => {
                for n in v {
                    self.fix_named(n, k);
                }
            }
            Node::Capture { body, .. } | Node::Look { body, .. } | Node::Repeat { body, .. } => {
                self.fix_named(body, k)
            }
            _ => {}
        }
    }

    fn disjunction(&mut self) -> R<Node> {
        self.check_depth()?;
        let id = self.next_disjunction;
        self.next_disjunction += 1;
        let mut alt = 0;
        let mut alts = Vec::new();
        loop {
            self.path.push((id, alt));
            let r = self.alternative();
            self.path.pop();
            alts.push(r?);
            if !self.eat('|') {
                break;
            }
            alt += 1;
        }
        Ok(if alts.len() == 1 {
            alts.pop().unwrap_or(Node::Empty)
        } else {
            Node::Alt(alts)
        })
    }

    fn alternative(&mut self) -> R<Node> {
        let mut terms = Vec::new();
        while let Some(c) = self.peek() {
            if c == ch('|') || c == ch(')') {
                break;
            }
            if let Some(t) = self.term()? {
                terms.push(t);
            }
        }
        Ok(match terms.len() {
            0 => Node::Empty,
            1 => terms.pop().unwrap_or(Node::Empty),
            _ => Node::Seq(terms),
        })
    }

    fn term(&mut self) -> R<Option<Node>> {
        // Aserciones.
        if self.eat('^') {
            self.no_quantifier()?;
            return Ok(Some(Node::Start {
                multiline: self.multiline,
            }));
        }
        if self.eat('$') {
            self.no_quantifier()?;
            return Ok(Some(Node::End {
                multiline: self.multiline,
            }));
        }
        if self.looking_at("\\b") || self.looking_at("\\B") {
            let negate = self.peek_at(1) == Some(ch('B'));
            self.i += 2;
            self.no_quantifier()?;
            return Ok(Some(Node::WordBoundary {
                negate,
                ic: self.ic,
            }));
        }
        if self.looking_at("(?=") || self.looking_at("(?!") {
            let negate = self.peek_at(2) == Some(ch('!'));
            self.i += 3;
            let first_group = self.groups;
            let body = self.disjunction()?;
            if !self.eat(')') {
                return Err("Unterminated group".into());
            }
            let node = Node::Look {
                behind: false,
                negate,
                body: Box::new(body),
            };
            // QuantifiableAssertion solo sin `u` (B.1.2).
            if self.unicode {
                self.no_quantifier()?;
                return Ok(Some(node));
            }
            return self.quantifier(node, first_group).map(Some);
        }
        if self.looking_at("(?<=") || self.looking_at("(?<!") {
            let negate = self.peek_at(3) == Some(ch('!'));
            self.i += 4;
            let body = self.disjunction()?;
            if !self.eat(')') {
                return Err("Unterminated group".into());
            }
            self.no_quantifier()?;
            return Ok(Some(Node::Look {
                behind: true,
                negate,
                body: Box::new(body),
            }));
        }
        let first_group = self.groups;
        let Some(atom) = self.atom()? else {
            return Ok(None);
        };
        self.quantifier(atom, first_group).map(Some)
    }

    /// Tras algo no cuantificable: un cuantificador es un error.
    fn no_quantifier(&mut self) -> R<()> {
        if matches!(self.peek(), Some(c) if c == ch('*') || c == ch('+') || c == ch('?')) {
            return Err("Nothing to repeat".into());
        }
        if self.is('{') && (self.unicode || self.braced_quantifier_len().is_some()) {
            return Err("Nothing to repeat".into());
        }
        Ok(())
    }

    /// Si en la posición actual hay un `{n}`, `{n,}` o `{n,m}`, su longitud y sus límites.
    fn braced_quantifier_len(&self) -> Option<(usize, u64, Option<u64>)> {
        let mut k = self.i;
        if self.s.get(k) != Some(&ch('{')) {
            return None;
        }
        k += 1;
        let digits = |k: &mut usize| -> Option<u64> {
            let start = *k;
            let mut v: u64 = 0;
            while let Some(&d) = self.s.get(*k) {
                if !(ch('0')..=ch('9')).contains(&d) {
                    break;
                }
                v = v.saturating_mul(10).saturating_add(u64::from(d - ch('0')));
                *k += 1;
            }
            (*k > start).then_some(v)
        };
        let min = digits(&mut k)?;
        let max = if self.s.get(k) == Some(&ch(',')) {
            k += 1;
            if self.s.get(k) == Some(&ch('}')) {
                None
            } else {
                Some(digits(&mut k)?)
            }
        } else {
            Some(min)
        };
        if self.s.get(k) != Some(&ch('}')) {
            return None;
        }
        Some((k + 1 - self.i, min, max))
    }

    /// El cuantificador opcional de `atom` (cuyos grupos empiezan después de `first_group`).
    fn quantifier(&mut self, atom: Node, first_group: u32) -> R<Node> {
        let (min, max) = match self.peek() {
            Some(c) if c == ch('*') => {
                self.i += 1;
                (0, None)
            }
            Some(c) if c == ch('+') => {
                self.i += 1;
                (1, None)
            }
            Some(c) if c == ch('?') => {
                self.i += 1;
                (0, Some(1))
            }
            Some(c) if c == ch('{') => match self.braced_quantifier_len() {
                Some((len, min, max)) => {
                    if max.is_some_and(|m| m < min) {
                        return Err("numbers out of order in {} quantifier".into());
                    }
                    self.i += len;
                    (min, max)
                }
                None if self.unicode => return Err("Incomplete quantifier".into()),
                // Sin `u`, un `{` que no forma un cuantificador es un carácter.
                None => return Ok(atom),
            },
            _ => return Ok(atom),
        };
        let greedy = !self.eat('?');
        self.no_quantifier()?;
        Ok(Node::Repeat {
            body: Box::new(atom),
            min,
            max,
            greedy,
            first_group,
            group_count: self.groups - first_group,
        })
    }

    fn char_node(&self, c: u32) -> Node {
        Node::Char { c, ic: self.ic }
    }

    fn set_node(&self, set: ClassSet, invert: bool) -> Node {
        Node::Set {
            set,
            invert,
            ic: self.ic,
        }
    }

    fn atom(&mut self) -> R<Option<Node>> {
        let Some(c) = self.peek() else {
            return Ok(None);
        };
        match char::from_u32(c).unwrap_or('\0') {
            '.' => {
                self.i += 1;
                Ok(Some(Node::Dot {
                    dot_all: self.dot_all,
                }))
            }
            '(' => self.group().map(Some),
            '[' => {
                self.i += 1;
                if self.v {
                    let (set, _) = self.class_v_body()?;
                    Ok(Some(self.set_node(set, false)))
                } else {
                    let (set, invert) = self.class_body()?;
                    Ok(Some(self.set_node(ClassSet::from_chars(set), invert)))
                }
            }
            '\\' => {
                self.i += 1;
                self.atom_escape().map(Some)
            }
            '*' | '+' | '?' => Err("Nothing to repeat".into()),
            '{' => {
                if self.unicode {
                    return Err("Lone quantifier brackets".into());
                }
                // InvalidBracedQuantifier (B.1.2).
                if self.braced_quantifier_len().is_some() {
                    return Err("Nothing to repeat".into());
                }
                self.i += 1;
                Ok(Some(self.char_node(c)))
            }
            '}' | ']' if self.unicode => Err("Lone quantifier brackets".into()),
            ')' | '|' => Ok(None),
            _ => {
                self.i += 1;
                Ok(Some(self.char_node(c)))
            }
        }
    }

    fn group(&mut self) -> R<Node> {
        self.i += 1;
        if self.eat('?') {
            if self.eat(':') {
                let body = self.disjunction()?;
                if !self.eat(')') {
                    return Err("Unterminated group".into());
                }
                return Ok(body);
            }
            if self.is('<') {
                self.i += 1;
                let name = self.group_name()?;
                self.groups += 1;
                let index = self.groups;
                self.names.push((name.clone(), self.path.clone()));
                self.group_names.push(Some(name));
                let body = self.disjunction()?;
                if !self.eat(')') {
                    return Err("Unterminated group".into());
                }
                return Ok(Node::Capture {
                    index,
                    body: Box::new(body),
                });
            }
            let (add, remove) = self.modifiers()?;
            let saved = (self.ic, self.multiline, self.dot_all);
            for (set, on) in [(&add, true), (&remove, false)] {
                if set.contains('i') {
                    self.ic = on;
                }
                if set.contains('m') {
                    self.multiline = on;
                }
                if set.contains('s') {
                    self.dot_all = on;
                }
            }
            let body = self.disjunction();
            (self.ic, self.multiline, self.dot_all) = saved;
            let body = body?;
            if !self.eat(')') {
                return Err("Unterminated group".into());
            }
            return Ok(body);
        }
        self.groups += 1;
        let index = self.groups;
        self.group_names.push(None);
        let body = self.disjunction()?;
        if !self.eat(')') {
            return Err("Unterminated group".into());
        }
        Ok(Node::Capture {
            index,
            body: Box::new(body),
        })
    }

    /// `(?ims-ims:` (RegExp modifiers, §22.2.1.1): los flags agregados y quitados.
    fn modifiers(&mut self) -> R<(String, String)> {
        let mut add = String::new();
        let mut remove = String::new();
        let mut removing = false;
        loop {
            let Some(c) = self.peek().and_then(char::from_u32) else {
                return Err("Invalid group".into());
            };
            self.i += 1;
            match c {
                'i' | 'm' | 's' => {
                    if add.contains(c) || remove.contains(c) {
                        return Err("Repeated flag in group modifiers".into());
                    }
                    if removing {
                        remove.push(c);
                    } else {
                        add.push(c);
                    }
                }
                '-' if !removing => removing = true,
                ':' => break,
                _ => return Err("Invalid group".into()),
            }
        }
        if removing && add.is_empty() && remove.is_empty() {
            return Err("Invalid group modifiers".into());
        }
        if !removing && add.is_empty() {
            return Err("Invalid group".into());
        }
        Ok((add, remove))
    }

    /// `GroupName :: < RegExpIdentifierName >` (después de `<`).
    fn group_name(&mut self) -> R<String> {
        let mut name = String::new();
        loop {
            let Some(mut c) = self.peek() else {
                return Err("Invalid capture group name".into());
            };
            if c == ch('>') {
                self.i += 1;
                break;
            }
            self.i += 1;
            if c == ch('\\') {
                if !self.eat('u') {
                    return Err("Invalid capture group name".into());
                }
                // RegExpUnicodeEscapeSequence[+UnicodeMode], con pares de surrogates.
                c = self.unicode_escape(true).ok_or("Invalid Unicode escape")?;
            } else if (0xD800..0xDC00).contains(&c)
                && let Some(t) = self.peek()
                && (0xDC00..0xE000).contains(&t)
            {
                self.i += 1;
                c = 0x10000 + ((c - 0xD800) << 10) + (t - 0xDC00);
            }
            let Some(chr) = char::from_u32(c) else {
                return Err("Invalid capture group name".into());
            };
            let ok = if name.is_empty() {
                is_id_start(chr)
            } else {
                is_id_part(chr)
            };
            if !ok {
                return Err("Invalid capture group name".into());
            }
            name.push(chr);
        }
        if name.is_empty() {
            return Err("Invalid capture group name".into());
        }
        Ok(name)
    }

    /// `\u` (después de `u`): `XXXX`, `{X…}` (solo con `brace`) y pares de surrogates
    /// `😀`.
    fn unicode_escape(&mut self, brace: bool) -> Option<u32> {
        let save = self.i;
        if brace && self.eat('{') {
            let mut v: u32 = 0;
            let mut n = 0;
            while let Some(d) = self.peek().and_then(|c| char::from_u32(c)?.to_digit(16)) {
                v = v.saturating_mul(16).saturating_add(d);
                n += 1;
                self.i += 1;
            }
            if n == 0 || v > 0x10FFFF || !self.eat('}') {
                self.i = save;
                return None;
            }
            return Some(v);
        }
        let lead = self.hex4()?;
        if brace && (0xD800..0xDC00).contains(&lead) && self.looking_at("\\u") {
            let at = self.i;
            self.i += 2;
            match self.hex4() {
                Some(t) if (0xDC00..0xE000).contains(&t) => {
                    return Some(0x10000 + ((lead - 0xD800) << 10) + (t - 0xDC00));
                }
                _ => self.i = at,
            }
        }
        Some(lead)
    }

    fn hex4(&mut self) -> Option<u32> {
        let mut v = 0;
        for k in 0..4 {
            let d = self
                .peek_at(k)
                .and_then(|c| char::from_u32(c)?.to_digit(16))?;
            v = v * 16 + d;
        }
        self.i += 4;
        Some(v)
    }

    /// Lo que sigue a `\` fuera de una clase.
    fn atom_escape(&mut self) -> R<Node> {
        let Some(c) = self.peek() else {
            return Err("\\ at end of pattern".into());
        };
        // Referencias numéricas (DecimalEscape).
        if (ch('1')..=ch('9')).contains(&c) {
            let start = self.i;
            let mut n: u32 = 0;
            while let Some(d) = self.peek().filter(|d| (ch('0')..=ch('9')).contains(d)) {
                n = n.saturating_mul(10).saturating_add(d - ch('0'));
                self.i += 1;
            }
            // Sin `u`, si el patrón no tiene tantos grupos es un escape octal o de identidad
            // (B.1.2).
            if self.unicode || n <= self.total_groups {
                self.max_backref = self.max_backref.max(n);
                return Ok(Node::BackRef {
                    groups: vec![n],
                    ic: self.ic,
                });
            }
            self.i = start;
            let c = self.character_escape(false)?;
            return Ok(self.char_node(c));
        }
        if c == ch('k') && self.named {
            self.i += 1;
            if !self.eat('<') {
                return Err("Invalid named reference".into());
            }
            let name = self.group_name()?;
            self.backrefs.push(name);
            return Ok(Node::BackRef {
                groups: Vec::new(),
                ic: self.ic,
            });
        }
        if let Some((set, _)) = self.class_escape_class()? {
            return Ok(self.set_node(set, false));
        }
        let c = self.character_escape(false)?;
        Ok(self.char_node(c))
    }

    /// `\d \D \s \S \w \W \p{…} \P{…}` (después de `\`): CompileToCharSet de
    /// CharacterClassEscape, con MayContainStrings. Una propiedad de strings fuera del modo
    /// `v` o con `\P` es un error.
    fn class_escape_class(&mut self) -> R<Option<(ClassSet, bool)>> {
        let Some(c) = self.peek().and_then(char::from_u32) else {
            return Ok(None);
        };
        let (u, v, ic) = (self.unicode, self.v, self.ic);
        let complement = |s: &CharSet| character_complement(s, u, v, ic);
        let word =
            || maybe_simple_case_folding(ClassSet::from_chars(word_characters(u, ic)), v, ic);
        let set = match c {
            'd' => digits(),
            'D' => complement(&digits()),
            's' => white_space(),
            'S' => complement(&white_space()),
            'w' => word().chars,
            'W' => complement(&word().chars),
            'p' | 'P' if self.unicode => {
                self.i += 1;
                let (prop, strings) = self.property_escape()?;
                if strings && (c == 'P' || !self.v) {
                    return Err("Invalid property name".into());
                }
                let set = maybe_simple_case_folding(prop, v, ic);
                if c == 'P' {
                    return Ok(Some((ClassSet::from_chars(complement(&set.chars)), false)));
                }
                return Ok(Some((set, strings)));
            }
            _ => return Ok(None),
        };
        self.i += 1;
        Ok(Some((ClassSet::from_chars(set), false)))
    }

    /// `{Name}` o `{Name=Value}` (después de `\p`): el conjunto y si es una propiedad de
    /// strings.
    fn property_escape(&mut self) -> R<(ClassSet, bool)> {
        if !self.eat('{') {
            return Err("Invalid property name".into());
        }
        let mut name = String::new();
        let mut value: Option<String> = None;
        loop {
            let Some(c) = self.peek().and_then(char::from_u32) else {
                return Err("Invalid property name".into());
            };
            self.i += 1;
            match c {
                '}' => break,
                '=' if value.is_none() => value = Some(String::new()),
                c if c.is_ascii_alphanumeric() || c == '_' => match &mut value {
                    Some(v) => v.push(c),
                    None => name.push(c),
                },
                _ => return Err("Invalid property name".into()),
            }
        }
        // Los nombres de propiedades de strings se reconocen aunque no haya `v`: el error lo
        // da quien llama.
        let strings_ok = value.is_none() && is_string_property(&name);
        match lookup_property(&name, value.as_deref(), strings_ok) {
            Some(Property::Chars(c)) => Ok((ClassSet::from_chars(c), false)),
            Some(Property::Strings(s)) => Ok((s, true)),
            None => Err("Invalid property name".into()),
        }
    }

    /// CharacterEscape (después de `\`): devuelve el carácter. `in_class` habilita `\b`, `\-`
    /// y `\c` + dígito/`_` (B.1.2).
    fn character_escape(&mut self, in_class: bool) -> R<u32> {
        let Some(c) = self.peek() else {
            return Err("\\ at end of pattern".into());
        };
        let chr = char::from_u32(c).unwrap_or('\0');
        self.i += 1;
        let value = match chr {
            'f' => 0x0C,
            'n' => 0x0A,
            'r' => 0x0D,
            't' => 0x09,
            'v' => 0x0B,
            'b' if in_class => 0x08,
            '-' if in_class && self.unicode => ch('-'),
            'c' => match self.peek().and_then(char::from_u32) {
                Some(l) if l.is_ascii_alphabetic() => {
                    self.i += 1;
                    (l as u32) % 32
                }
                Some(l) if in_class && !self.unicode && (l.is_ascii_digit() || l == '_') => {
                    self.i += 1;
                    (l as u32) % 32
                }
                _ if self.unicode => return Err("Invalid unicode escape".into()),
                // `\c` sin letra: el `\` es un carácter (B.1.2).
                _ => {
                    self.i -= 1;
                    ch('\\')
                }
            },
            '0' if !self
                .peek()
                .is_some_and(|d| (ch('0')..=ch('9')).contains(&d)) =>
            {
                0
            }
            '0'..='7' if !self.unicode => {
                // LegacyOctalEscapeSequence.
                let mut v = c - ch('0');
                let max = if c <= ch('3') { 2 } else { 1 };
                for _ in 0..max {
                    match self.peek().filter(|d| (ch('0')..=ch('7')).contains(d)) {
                        Some(d) => {
                            v = v * 8 + (d - ch('0'));
                            self.i += 1;
                        }
                        None => break,
                    }
                }
                v
            }
            '8' | '9' if !self.unicode => c,
            'x' => {
                let a = self.peek().and_then(|c| char::from_u32(c)?.to_digit(16));
                let b = self
                    .peek_at(1)
                    .and_then(|c| char::from_u32(c)?.to_digit(16));
                match (a, b) {
                    (Some(a), Some(b)) => {
                        self.i += 2;
                        a * 16 + b
                    }
                    _ if self.unicode => return Err("Invalid escape".into()),
                    _ => ch('x'),
                }
            }
            'u' => match self.unicode_escape(self.unicode) {
                Some(v) => v,
                None if self.unicode => return Err("Invalid Unicode escape".into()),
                None => ch('u'),
            },
            _ if self.unicode => {
                // IdentityEscape[+UnicodeMode]: SyntaxCharacter o `/`.
                if SYNTAX.contains(chr) || chr == '/' {
                    c
                } else {
                    return Err("Invalid escape".into());
                }
            }
            // IdentityEscape[~UnicodeMode, +N]: cualquier carácter salvo `c` y `k`.
            'k' if self.named => return Err("Invalid named reference".into()),
            _ => c,
        };
        Ok(value)
    }

    // --- Clases sin `v` ---

    /// Después de `[`: el CharSet y si está invertida (CompileCharacterClass).
    fn class_body(&mut self) -> R<(CharSet, bool)> {
        let invert = self.eat('^');
        let mut set = CharSet::new();
        loop {
            match self.peek() {
                None => return Err("Unterminated character class".into()),
                Some(c) if c == ch(']') => {
                    self.i += 1;
                    return Ok((set, invert));
                }
                _ => {}
            }
            let a = self.class_atom()?;
            if self.is('-') && self.peek_at(1) != Some(ch(']')) && self.peek_at(1).is_some() {
                self.i += 1;
                let b = self.class_atom()?;
                match (a, b) {
                    (ClassAtom::Char(x), ClassAtom::Char(y)) => {
                        if x > y {
                            return Err("Range out of order in character class".into());
                        }
                        set.add_range(x, y);
                    }
                    // `[\d-a]`: error con `u`; sin `u`, `-` es un carácter (B.1.2).
                    _ if self.unicode => return Err("Invalid character class".into()),
                    (a, b) => {
                        for atom in [a, b] {
                            match atom {
                                ClassAtom::Char(x) => set.add(x),
                                ClassAtom::Class(c) => set.add_set(&c),
                            }
                        }
                        set.add(ch('-'));
                    }
                }
                continue;
            }
            match a {
                ClassAtom::Char(x) => set.add(x),
                ClassAtom::Class(c) => set.add_set(&c),
            }
        }
    }

    fn class_atom(&mut self) -> R<ClassAtom> {
        let c = self.peek().ok_or("Unterminated character class")?;
        self.i += 1;
        if c != ch('\\') {
            return Ok(ClassAtom::Char(c));
        }
        let Some(n) = self.peek() else {
            return Err("\\ at end of pattern".into());
        };
        if let Some((set, _)) = self.class_escape_class()? {
            return Ok(ClassAtom::Class(set.chars));
        }
        // `\1` dentro de una clase: error con `u`; sin `u`, octal (B.1.2).
        if self.unicode && (ch('1')..=ch('9')).contains(&n) {
            return Err("Invalid class escape".into());
        }
        if n == ch('k') && !self.named {
            self.i += 1;
            return Ok(ClassAtom::Char(n));
        }
        Ok(ClassAtom::Char(self.character_escape(true)?))
    }

    // --- Clases con `v` (ClassSetExpression) ---

    /// MaybeSimpleCaseFolding con los flags vigentes.
    fn fold(&self, set: ClassSet) -> ClassSet {
        maybe_simple_case_folding(set, self.v, self.ic)
    }

    /// Después de `[` (CharacterClass o NestedClass con `v`): el conjunto (ya complementado
    /// si es `[^…]`, CompileCharacterClass con UnicodeSets) y MayContainStrings.
    fn class_v_body(&mut self) -> R<(ClassSet, bool)> {
        self.check_depth()?;
        let negated = self.eat('^');
        let (set, strings) = self.class_set_expression()?;
        if !self.eat(']') {
            return Err("Unterminated character class".into());
        }
        if negated {
            if strings {
                return Err("Negated character class may contain strings".into());
            }
            let c = character_complement(&set.chars, true, true, self.ic);
            return Ok((ClassSet::from_chars(c), false));
        }
        Ok((set, strings))
    }

    fn reserved_double(&self) -> bool {
        let (Some(a), Some(b)) = (self.peek(), self.peek_at(1)) else {
            return false;
        };
        a == b && "&!#$%*+,.:;<=>?@^`~".chars().any(|p| ch(p) == a)
    }

    /// ClassUnion, ClassIntersection o ClassSubtraction.
    fn class_set_expression(&mut self) -> R<(ClassSet, bool)> {
        if self.is(']') {
            return Ok((ClassSet::default(), false));
        }
        let (first, first_is_range, first_strings) = self.class_set_operand_or_range()?;
        if self.looking_at("&&") || self.looking_at("--") {
            if first_is_range {
                return Err("Invalid set operation in character class".into());
            }
            let intersection = self.looking_at("&&");
            let op = if intersection { "&&" } else { "--" };
            let mut acc = first;
            let mut strings = first_strings;
            while self.looking_at(op) {
                self.i += 2;
                if intersection && self.is('&') {
                    return Err("Invalid set operation in character class".into());
                }
                let (b, range, s) = self.class_set_operand_or_range()?;
                if range {
                    return Err("Invalid set operation in character class".into());
                }
                // Intersección: puede tener strings si todos los operandos pueden; resta: si
                // el primero puede.
                if intersection {
                    acc = acc.intersect(&b);
                    strings = strings && s;
                } else {
                    acc = acc.subtract(&b);
                }
            }
            if !self.is(']') {
                return Err("Invalid set operation in character class".into());
            }
            return Ok((acc, strings));
        }
        // Unión.
        let mut acc = first;
        let mut strings = first_strings;
        while !self.is(']') {
            if self.peek().is_none() {
                return Err("Unterminated character class".into());
            }
            if self.looking_at("&&") || self.looking_at("--") {
                return Err("Invalid set operation in character class".into());
            }
            let (b, _, s) = self.class_set_operand_or_range()?;
            acc = acc.union(&b);
            strings |= s;
        }
        Ok((acc, strings))
    }

    /// Un ClassSetOperand o un ClassSetRange: (conjunto, si es rango, MayContainStrings).
    fn class_set_operand_or_range(&mut self) -> R<(ClassSet, bool, bool)> {
        match self.class_set_operand()? {
            SetOperand::Char(a) => {
                if self.is('-') && self.peek_at(1) != Some(ch('-')) {
                    self.i += 1;
                    let SetOperand::Char(b) = self.class_set_operand()? else {
                        return Err("Invalid character class".into());
                    };
                    if a > b {
                        return Err("Range out of order in character class".into());
                    }
                    let set = self.fold(ClassSet::from_chars(CharSet::range(a, b)));
                    return Ok((set, true, false));
                }
                let set = self.fold(ClassSet::from_chars(CharSet::single(a)));
                Ok((set, false, false))
            }
            SetOperand::Set(s, strings) => Ok((s, false, strings)),
        }
    }

    fn class_set_operand(&mut self) -> R<SetOperand> {
        let Some(c) = self.peek() else {
            return Err("Unterminated character class".into());
        };
        if c == ch('[') {
            self.i += 1;
            let (s, strings) = self.class_v_body()?;
            return Ok(SetOperand::Set(s, strings));
        }
        if c == ch('\\') {
            self.i += 1;
            if self.looking_at("q{") {
                self.i += 2;
                let (s, strings) = self.class_string_disjunction()?;
                return Ok(SetOperand::Set(s, strings));
            }
            if let Some((s, strings)) = self.class_escape_class()? {
                return Ok(SetOperand::Set(s, strings));
            }
            return Ok(SetOperand::Char(self.class_set_escaped_char()?));
        }
        if self.reserved_double() {
            return Err("Invalid set operation in character class".into());
        }
        let chr = char::from_u32(c).unwrap_or('\0');
        if "()[]{}/-|".contains(chr) {
            return Err("Invalid character in character class".into());
        }
        self.i += 1;
        Ok(SetOperand::Char(c))
    }

    /// Después de `\` en un ClassSetCharacter: ClassSetReservedPunctuator, `\b` o
    /// CharacterEscape.
    fn class_set_escaped_char(&mut self) -> R<u32> {
        let Some(n) = self.peek().and_then(char::from_u32) else {
            return Err("\\ at end of pattern".into());
        };
        if "&-!#%,:;<=>@`~".contains(n) {
            self.i += 1;
            return Ok(n as u32);
        }
        if n == 'b' {
            self.i += 1;
            return Ok(0x08);
        }
        self.character_escape(false)
    }

    /// `\q{a|bc|}` (después de `{`): el conjunto (MaybeSimpleCaseFolding) y si algún
    /// ClassString no tiene exactamente un carácter (MayContainStrings).
    fn class_string_disjunction(&mut self) -> R<(ClassSet, bool)> {
        let mut set = ClassSet::default();
        let mut strings = false;
        let mut cur: Vec<u32> = Vec::new();
        loop {
            let Some(c) = self.peek() else {
                return Err("Unterminated class string disjunction".into());
            };
            if c == ch('}') || c == ch('|') {
                self.i += 1;
                if cur.len() != 1 {
                    strings = true;
                }
                set.add_string(std::mem::take(&mut cur));
                if c == ch('}') {
                    return Ok((self.fold(set), strings));
                }
                continue;
            }
            if c == ch('\\') {
                self.i += 1;
                cur.push(self.class_set_escaped_char()?);
            } else {
                if self.reserved_double() {
                    return Err("Invalid set operation in character class".into());
                }
                let chr = char::from_u32(c).unwrap_or('\0');
                if "()[]{}/-|".contains(chr) {
                    return Err("Invalid character in character class".into());
                }
                self.i += 1;
                cur.push(c);
            }
        }
    }
}

/// MightBothParticipate (§22.2.1.1): falso si los grupos están en alternativas distintas de
/// una misma disyunción.
fn might_both_participate(a: &[(u32, u32)], b: &[(u32, u32)]) -> bool {
    for (x, y) in a.iter().zip(b) {
        if x.0 != y.0 {
            return true;
        }
        if x.1 != y.1 {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_patterns() {
        for (p, f) in [
            ("a|b*c{1,2}?", ""),
            ("(?<x>a)\\k<x>", "u"),
            ("(?<x>a)|(?<x>b)", ""),
            ("[\\d-a]", ""),
            ("\\1(a)", ""),
            ("a{", ""),
            ("]", ""),
            ("(?=a)*", ""),
            ("\\p{L}\\P{Script=Greek}\\p{ASCII_Hex_Digit}", "u"),
            ("[\\p{RGI_Emoji}--\\q{a|bc}]", "v"),
            ("[a&&b]", "v"),
            ("(?i:a)(?-s:b)(?m-i:c)", ""),
            ("\\c1", ""),
            ("[\\c1]", ""),
            ("\\u{1F600}", "u"),
            ("(?<𝒜>.)", ""),
            ("[^\\q{a}]", "v"),
            ("[^[\\p{RGI_Emoji}&&a]]", "v"),
        ] {
            assert!(validate(p, f).is_ok(), "/{p}/{f}: {:?}", validate(p, f));
        }
    }

    #[test]
    fn invalid_patterns() {
        for (p, f) in [
            ("a**", ""),
            ("{1}", ""),
            ("(?<x>a)(?<x>b)", ""),
            ("(?<x>a)\\k<y>", ""),
            ("[b-a]", ""),
            ("[\\d-a]", "u"),
            ("\\1", "u"),
            ("a{2,1}", ""),
            ("(?<=a)*", ""),
            ("(?=a)*", "u"),
            ("]", "u"),
            ("\\p{Nope}", "u"),
            ("[^\\p{RGI_Emoji}]", "v"),
            ("\\P{RGI_Emoji}", "v"),
            ("\\p{RGI_Emoji}", "u"),
            ("[a-z&&b]", "v"),
            ("[(]", "v"),
            ("[a&&&b]", "v"),
            ("(?ii:a)", ""),
            ("(?i-i:a)", ""),
            ("(?-:a)", ""),
            ("\\k<a>(?<b>.)", ""),
            ("a", "gg"),
            ("a", "uv"),
            ("(", ""),
            (")", ""),
            ("[", ""),
            ("[^\\q{ab}]", "v"),
            ("[^\\q{}]", "v"),
            ("[^[\\p{RGI_Emoji}--\\p{RGI_Emoji}]]", "v"),
        ] {
            assert!(validate(p, f).is_err(), "/{p}/{f} debería fallar");
        }
    }

    #[test]
    fn legacy_backreferences() {
        let f = Flags::default();
        let units = |s: &str| s.encode_utf16().collect::<Vec<_>>();
        // `\2` con un solo grupo: octal.
        let p = parse(&units("(a)\\2"), f).unwrap();
        let Node::Seq(v) = p.node else { panic!() };
        assert!(matches!(v[1], Node::Char { c: 2, .. }));
        // `\1` antes del grupo: backreference.
        let p = parse(&units("\\1(a)"), f).unwrap();
        let Node::Seq(v) = p.node else { panic!() };
        assert!(matches!(&v[0], Node::BackRef { groups, .. } if groups == &[1]));
    }
}
