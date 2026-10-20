//! Validación de la sintaxis de los patrones de RegExp (ECMA-262 §22.2.1 *Patterns* y sus
//! early errors, más la gramática extendida de Annex B.1.2 para los patrones sin `u`/`v`). Solo
//! la gramática: la compilación y la ejecución del regex son de otra etapa.
//!
//! Sin `u`/`v` el patrón se interpreta como code units UTF-16 (un carácter astral son dos
//! unidades, lo que importa en los rangos de clases); con `u`/`v`, como code points.

use crate::lexer::unicode_tables::{BINARY_PROPERTIES, GENERAL_CATEGORY_VALUES, SCRIPT_VALUES};
use crate::lexer::{is_id_part, is_id_start};

/// Propiedades de strings (§22.2.2.9, tabla "Binary Unicode properties of strings"): solo con
/// `v`.
const STRING_PROPERTIES: &[&str] = &[
    "Basic_Emoji",
    "Emoji_Keycap_Sequence",
    "RGI_Emoji_Modifier_Sequence",
    "RGI_Emoji_Flag_Sequence",
    "RGI_Emoji_Tag_Sequence",
    "RGI_Emoji_ZWJ_Sequence",
    "RGI_Emoji",
];

/// Valida las flags (§13.2.7.1 y §22.2.3.1): solo `dgimsuvy`, sin repetir, y no `u` con `v`.
pub fn validate_flags(flags: &str) -> Result<(), String> {
    let mut seen = String::new();
    for c in flags.chars() {
        if !"dgimsuvy".contains(c) || seen.contains(c) {
            return Err(format!("Invalid regular expression flags '{flags}'"));
        }
        seen.push(c);
    }
    if seen.contains('u') && seen.contains('v') {
        return Err(format!("Invalid regular expression flags '{flags}'"));
    }
    Ok(())
}

/// Valida un patrón con sus flags (`IsValidRegularExpressionLiteral`, §13.2.7.2).
pub fn validate(pattern: &str, flags: &str) -> Result<(), String> {
    validate_flags(flags)?;
    let u = flags.contains('u');
    let v = flags.contains('v');
    let unicode = u || v;
    let src: Vec<u32> = if unicode {
        pattern.chars().map(|c| c as u32).collect()
    } else {
        pattern.encode_utf16().map(u32::from).collect()
    };
    let mut p = Parser {
        s: src,
        i: 0,
        unicode,
        v,
        named: unicode,
        groups: 0,
        names: Vec::new(),
        backref_names: Vec::new(),
        max_backref: 0,
        path: Vec::new(),
        next_disjunction: 0,
        stack_base: crate::parser::stack_position(),
        last_property_strings: false,
    };
    // Sin `u`, la gramática se reinterpreta con [+NamedCaptureGroups] si el patrón tiene un
    // GroupName (§22.2.3.4, ParsePattern).
    if !unicode {
        p.named = has_group_name(&p.s);
    }
    p.pattern()
        .map_err(|m| format!("Invalid regular expression: /{pattern}/{flags}: {m}"))
}

/// ¿El patrón (sin `u`) tiene un `(?<nombre>`?
fn has_group_name(s: &[u32]) -> bool {
    let mut i = 0;
    let mut in_class = false;
    while i < s.len() {
        let c = s[i];
        if c == '\\' as u32 {
            i += 2;
            continue;
        }
        if in_class {
            if c == ']' as u32 {
                in_class = false;
            }
        } else if c == '[' as u32 {
            in_class = true;
        } else if c == '(' as u32
            && s.get(i + 1) == Some(&('?' as u32))
            && s.get(i + 2) == Some(&('<' as u32))
            && !matches!(s.get(i + 3), Some(&c) if c == '=' as u32 || c == '!' as u32)
        {
            return true;
        }
        i += 1;
    }
    false
}

/// Un elemento de una clase sin `v`: un carácter o una clase de caracteres (`\d`).
#[derive(Clone, Copy, PartialEq)]
enum ClassAtom {
    Char(u32),
    Class,
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
    groups: u32,
    /// Grupos con nombre y su "camino" de alternativas (para MightBothParticipate).
    names: Vec<(String, Vec<(u32, u32)>)>,
    backref_names: Vec<String>,
    max_backref: u32,
    /// Alternativas en curso: (disyunción, índice de la alternativa).
    path: Vec<(u32, u32)>,
    next_disjunction: u32,
    stack_base: usize,
    /// El último `\p{…}` era de una propiedad de strings.
    last_property_strings: bool,
}

type R<T> = Result<T, String>;

const SYNTAX: &str = "^$\\.*+?()[]{}|";

fn ch(c: char) -> u32 {
    c as u32
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

    fn pattern(&mut self) -> R<()> {
        self.disjunction()?;
        if self.i < self.s.len() {
            return Err(if self.is(')') {
                "Unmatched ')'".into()
            } else {
                "Unexpected character".into()
            });
        }
        // Early errors de Pattern (§22.2.1.1).
        for name in &self.backref_names {
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
        Ok(())
    }

    fn disjunction(&mut self) -> R<()> {
        // Anidamiento de grupos y clases: el mismo límite de pila que el parser.
        if self
            .stack_base
            .saturating_sub(crate::parser::stack_position())
            > 256 * 1024
        {
            return Err("Maximum nesting depth exceeded".into());
        }
        let id = self.next_disjunction;
        self.next_disjunction += 1;
        let mut alt = 0;
        loop {
            self.path.push((id, alt));
            let r = self.alternative();
            self.path.pop();
            r?;
            if !self.eat('|') {
                return Ok(());
            }
            alt += 1;
        }
    }

    fn alternative(&mut self) -> R<()> {
        while let Some(c) = self.peek() {
            if c == ch('|') || c == ch(')') {
                break;
            }
            self.term()?;
        }
        Ok(())
    }

    fn term(&mut self) -> R<()> {
        // Aserciones.
        if self.eat('^') || self.eat('$') {
            return self.no_quantifier();
        }
        if self.looking_at("\\b") || self.looking_at("\\B") {
            self.i += 2;
            return self.no_quantifier();
        }
        if self.looking_at("(?=") || self.looking_at("(?!") {
            self.i += 3;
            self.disjunction()?;
            if !self.eat(')') {
                return Err("Unterminated group".into());
            }
            // QuantifiableAssertion solo sin `u` (B.1.2).
            if self.unicode {
                return self.no_quantifier();
            }
            return self.quantifier();
        }
        if self.looking_at("(?<=") || self.looking_at("(?<!") {
            self.i += 4;
            self.disjunction()?;
            if !self.eat(')') {
                return Err("Unterminated group".into());
            }
            return self.no_quantifier();
        }
        self.atom()?;
        self.quantifier()
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

    fn quantifier(&mut self) -> R<()> {
        let q = match self.peek() {
            Some(c) if c == ch('*') || c == ch('+') || c == ch('?') => {
                self.i += 1;
                true
            }
            Some(c) if c == ch('{') => match self.braced_quantifier_len() {
                Some((len, min, max)) => {
                    if max.is_some_and(|m| m < min) {
                        return Err("numbers out of order in {} quantifier".into());
                    }
                    self.i += len;
                    true
                }
                None if self.unicode => return Err("Incomplete quantifier".into()),
                // Sin `u`, un `{` que no forma un cuantificador es un carácter.
                None => false,
            },
            _ => false,
        };
        if q {
            self.eat('?');
            self.no_quantifier()?;
        }
        Ok(())
    }

    fn atom(&mut self) -> R<()> {
        let Some(c) = self.peek() else {
            return Ok(());
        };
        match char::from_u32(c).unwrap_or('\0') {
            '.' => {
                self.i += 1;
                Ok(())
            }
            '(' => self.group(),
            '[' => {
                self.i += 1;
                if self.v {
                    self.class_v_body()?;
                } else {
                    self.class_body()?;
                }
                Ok(())
            }
            '\\' => {
                self.i += 1;
                self.atom_escape()
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
                Ok(())
            }
            '}' | ']' if self.unicode => Err("Lone quantifier brackets".into()),
            ')' | '|' => Ok(()),
            _ => {
                self.i += 1;
                Ok(())
            }
        }
    }

    fn group(&mut self) -> R<()> {
        self.i += 1;
        if self.eat('?') {
            if self.eat(':') {
            } else if self.is('<') {
                self.i += 1;
                let name = self.group_name()?;
                self.groups += 1;
                self.names.push((name, self.path.clone()));
            } else {
                self.modifiers()?;
            }
        } else {
            self.groups += 1;
        }
        self.disjunction()?;
        if !self.eat(')') {
            return Err("Unterminated group".into());
        }
        Ok(())
    }

    /// `(?ims-ims:` (RegExp modifiers, §22.2.1.1).
    fn modifiers(&mut self) -> R<()> {
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
        Ok(())
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
    fn atom_escape(&mut self) -> R<()> {
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
            if self.unicode {
                self.max_backref = self.max_backref.max(n);
                return Ok(());
            }
            // Sin `u`: si no hay tantos grupos, es un escape octal o de identidad (B.1.2).
            // Los grupos se cuentan al final; aquí se acepta siempre.
            let _ = start;
            return Ok(());
        }
        if c == ch('k') {
            self.i += 1;
            if self.named {
                if !self.eat('<') {
                    return Err("Invalid named reference".into());
                }
                let name = self.group_name()?;
                self.backref_names.push(name);
            }
            return Ok(());
        }
        if self.class_escape_class()? {
            return Ok(());
        }
        self.character_escape(false).map(|_| ())
    }

    /// `\d \D \s \S \w \W \p{…} \P{…}` (después de `\`). Devuelve si lo era; con `v`, si
    /// `\p` es de una propiedad de strings, el error si no corresponde lo da quien llama.
    fn class_escape_class(&mut self) -> R<bool> {
        let Some(c) = self.peek().and_then(char::from_u32) else {
            return Ok(false);
        };
        match c {
            'd' | 'D' | 's' | 'S' | 'w' | 'W' => {
                self.i += 1;
                Ok(true)
            }
            'p' | 'P' if self.unicode => {
                self.i += 1;
                let strings = self.property_escape()?;
                if strings && (c == 'P' || !self.v) {
                    return Err("Invalid property name".into());
                }
                self.last_property_strings = strings;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// `{Name}` o `{Name=Value}` (después de `\p`). Devuelve si es una propiedad de strings.
    fn property_escape(&mut self) -> R<bool> {
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
        match value {
            Some(v) => {
                let ok = match name.as_str() {
                    "General_Category" | "gc" => GENERAL_CATEGORY_VALUES.contains(&v.as_str()),
                    "Script" | "sc" | "Script_Extensions" | "scx" => {
                        SCRIPT_VALUES.contains(&v.as_str())
                    }
                    _ => false,
                };
                if !ok {
                    return Err("Invalid property name".into());
                }
                Ok(false)
            }
            None => {
                if GENERAL_CATEGORY_VALUES.contains(&name.as_str())
                    || BINARY_PROPERTIES.contains(&name.as_str())
                {
                    Ok(false)
                } else if STRING_PROPERTIES.contains(&name.as_str()) {
                    Ok(true)
                } else {
                    Err("Invalid property name".into())
                }
            }
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

    fn class_body(&mut self) -> R<()> {
        self.eat('^');
        loop {
            match self.peek() {
                None => return Err("Unterminated character class".into()),
                Some(c) if c == ch(']') => {
                    self.i += 1;
                    return Ok(());
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
                    }
                    // `[\d-a]`: error con `u`; sin `u`, `-` es un carácter (B.1.2).
                    _ if self.unicode => return Err("Invalid character class".into()),
                    _ => {}
                }
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
        if self.class_escape_class()? {
            return Ok(ClassAtom::Class);
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

    /// Después de `[`. Devuelve si la clase puede contener strings (MayContainStrings).
    fn class_v_body(&mut self) -> R<bool> {
        if self
            .stack_base
            .saturating_sub(crate::parser::stack_position())
            > 256 * 1024
        {
            return Err("Maximum nesting depth exceeded".into());
        }
        let negated = self.eat('^');
        let strings = self.class_set_expression()?;
        if !self.eat(']') {
            return Err("Unterminated character class".into());
        }
        if negated && strings {
            return Err("Negated character class may contain strings".into());
        }
        Ok(strings)
    }

    fn reserved_double(&self) -> bool {
        let (Some(a), Some(b)) = (self.peek(), self.peek_at(1)) else {
            return false;
        };
        a == b && "&!#$%*+,.:;<=>?@^`~".chars().any(|p| ch(p) == a)
    }

    /// ClassUnion, ClassIntersection o ClassSubtraction.
    fn class_set_expression(&mut self) -> R<bool> {
        if self.is(']') {
            return Ok(false);
        }
        let (first, first_is_range, first_strings) = self.class_set_operand_or_range()?;
        let _ = first;
        if self.looking_at("&&") || self.looking_at("--") {
            if first_is_range {
                return Err("Invalid set operation in character class".into());
            }
            let op = if self.looking_at("&&") { "&&" } else { "--" };
            let mut strings = first_strings;
            while self.looking_at(op) {
                self.i += 2;
                if op == "&&" && self.is('&') {
                    return Err("Invalid set operation in character class".into());
                }
                let (_, range, s) = self.class_set_operand_or_range()?;
                if range {
                    return Err("Invalid set operation in character class".into());
                }
                // Intersección: puede tener strings si todos los operandos pueden;
                // resta: si el primero puede.
                if op == "&&" {
                    strings = strings && s;
                }
            }
            if !self.is(']') {
                return Err("Invalid set operation in character class".into());
            }
            return Ok(strings);
        }
        // Unión.
        let mut strings = first_strings;
        while !self.is(']') {
            if self.peek().is_none() {
                return Err("Unterminated character class".into());
            }
            if self.looking_at("&&") || self.looking_at("--") {
                return Err("Invalid set operation in character class".into());
            }
            let (_, _, s) = self.class_set_operand_or_range()?;
            strings |= s;
        }
        Ok(strings)
    }

    /// Un ClassSetOperand o un ClassSetRange: (carácter si lo es, si es rango, si puede tener
    /// strings).
    fn class_set_operand_or_range(&mut self) -> R<(Option<u32>, bool, bool)> {
        let (c, strings) = self.class_set_operand()?;
        if let Some(a) = c
            && self.is('-')
            && self.peek_at(1) != Some(ch('-'))
        {
            self.i += 1;
            let (b, _) = self.class_set_operand()?;
            let Some(b) = b else {
                return Err("Invalid character class".into());
            };
            if a > b {
                return Err("Range out of order in character class".into());
            }
            return Ok((None, true, false));
        }
        Ok((c, false, strings))
    }

    /// ClassSetOperand: (el carácter si es un ClassSetCharacter, si puede tener strings).
    fn class_set_operand(&mut self) -> R<(Option<u32>, bool)> {
        let Some(c) = self.peek() else {
            return Err("Unterminated character class".into());
        };
        if c == ch('[') {
            self.i += 1;
            let s = self.class_v_body()?;
            return Ok((None, s));
        }
        if c == ch('\\') {
            self.i += 1;
            if self.looking_at("q{") {
                self.i += 2;
                return Ok((None, self.class_string_disjunction()?));
            }
            self.last_property_strings = false;
            if self.class_escape_class()? {
                return Ok((None, self.last_property_strings));
            }
            let Some(n) = self.peek().and_then(char::from_u32) else {
                return Err("\\ at end of pattern".into());
            };
            // ClassSetReservedPunctuator escapado y `\b`.
            if "&-!#%,:;<=>@`~".contains(n) {
                self.i += 1;
                return Ok((Some(n as u32), false));
            }
            if n == 'b' {
                self.i += 1;
                return Ok((Some(0x08), false));
            }
            return Ok((Some(self.character_escape(false)?), false));
        }
        if self.reserved_double() {
            return Err("Invalid set operation in character class".into());
        }
        let chr = char::from_u32(c).unwrap_or('\0');
        if "()[]{}/-|".contains(chr) {
            return Err("Invalid character in character class".into());
        }
        self.i += 1;
        Ok((Some(c), false))
    }

    /// `\q{a|bc|}` (después de `{`). Devuelve si algún ClassString no tiene exactamente un
    /// carácter (MayContainStrings).
    fn class_string_disjunction(&mut self) -> R<bool> {
        let mut strings = false;
        let mut len = 0;
        loop {
            let Some(c) = self.peek() else {
                return Err("Unterminated class string disjunction".into());
            };
            if c == ch('}') {
                self.i += 1;
                if len != 1 {
                    strings = true;
                }
                return Ok(strings);
            }
            if c == ch('|') {
                self.i += 1;
                if len != 1 {
                    strings = true;
                }
                len = 0;
                continue;
            }
            if c == ch('\\') {
                self.i += 1;
                let Some(n) = self.peek().and_then(char::from_u32) else {
                    return Err("\\ at end of pattern".into());
                };
                if "&-!#%,:;<=>@`~".contains(n) || n == 'b' {
                    self.i += 1;
                } else {
                    self.character_escape(false)?;
                }
            } else {
                if self.reserved_double() {
                    return Err("Invalid set operation in character class".into());
                }
                let chr = char::from_u32(c).unwrap_or('\0');
                if "()[]{}/-|".contains(chr) {
                    return Err("Invalid character in character class".into());
                }
                self.i += 1;
            }
            len += 1;
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
        ] {
            assert!(validate(p, f).is_err(), "/{p}/{f} debería fallar");
        }
    }
}
