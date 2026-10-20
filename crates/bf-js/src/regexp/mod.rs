//! Motor de expresiones regulares propio (ECMA-262 §22.2): el parser de patrones con sus
//! early errors y Annex B.1.2 ([`parser`]), la compilación a un programa de backtracking
//! ([`compiler`]), su ejecución con las semánticas de §22.2.2 ([`matcher`]) y los conjuntos de
//! caracteres, Canonicalize y las propiedades Unicode ([`unicode`], [`unicode_tables`]).

pub mod compiler;
pub mod matcher;
pub mod parser;
pub mod unicode;
#[rustfmt::skip]
pub mod unicode_tables;

use std::time::Instant;

pub use matcher::{Capture, MatchError};
pub use parser::{Flags, parse_flags, validate, validate_flags};

/// Un patrón compilado (el `[[RegExpMatcher]]` de un objeto RegExp).
#[derive(Debug, Clone)]
pub struct Regex {
    pub flags: Flags,
    pub program: compiler::Program,
    /// El nombre de cada grupo (índice 0 = grupo 1).
    pub group_names: Vec<Option<String>>,
}

impl Regex {
    /// ParsePattern + CompilePattern. El error es el mensaje del SyntaxError.
    pub fn new(pattern: &[u16], flags: &str) -> Result<Regex, String> {
        let f = parse_flags(flags)?;
        let p = parser::parse(pattern, f).map_err(|m| {
            format!(
                "Invalid regular expression: /{}/{flags}: {m}",
                String::from_utf16_lossy(pattern)
            )
        })?;
        let program = compiler::compile(&p, f.either_unicode());
        Ok(Regex {
            flags: f,
            program,
            group_names: p.group_names,
        })
    }

    /// Cantidad de grupos de captura (sin el 0).
    pub fn group_count(&self) -> usize {
        self.program.group_count as usize
    }

    /// El matcher en `index` (code units): las capturas, la 0 es el match completo.
    pub fn match_at(
        &self,
        input: &[u16],
        index: usize,
        deadline: Option<Instant>,
    ) -> Result<Option<Vec<Capture>>, MatchError> {
        matcher::Matcher::new(&self.program, input, deadline).match_at(index)
    }

    /// Busca desde `index` avanzando con AdvanceStringIndex (el bucle de RegExpBuiltinExec,
    /// §22.2.7.2 paso 13, sin `sticky`). Devuelve el índice donde empezó el intento que tuvo
    /// éxito y las capturas.
    pub fn search_from(
        &self,
        input: &[u16],
        mut index: usize,
        sticky: bool,
        deadline: Option<Instant>,
    ) -> Result<Option<(usize, Vec<Capture>)>, MatchError> {
        let mut m = matcher::Matcher::new(&self.program, input, deadline);
        let full_unicode = self.flags.either_unicode();
        let first = self.program.first.as_ref();
        while index <= input.len() {
            // Las posiciones cuyo carácter no puede empezar un match se saltan sin
            // ejecutar el matcher (el resultado es el mismo).
            // (Con `u`/`v`, un índice en medio de un par empieza en el par: no se salta.)
            let mid_pair = full_unicode
                && index > 0
                && index < input.len()
                && (0xDC00..0xE000).contains(&input[index])
                && (0xD800..0xDC00).contains(&input[index - 1]);
            let skip = !mid_pair
                && first.is_some_and(|set| {
                    index >= input.len() || !set.contains(char_at(input, index, full_unicode))
                });
            if !skip && let Some(caps) = m.match_at(index)? {
                return Ok(Some((index, caps)));
            }
            // Con `^` sin Multiline solo el primer intento puede tener éxito.
            if sticky || self.program.anchored {
                return Ok(None);
            }
            index = advance_string_index(input, index, full_unicode);
        }
        Ok(None)
    }
}

/// El carácter (code point con `unicode`) que empieza en `index`.
fn char_at(s: &[u16], index: usize, unicode: bool) -> u32 {
    let u = u32::from(s[index]);
    if unicode
        && (0xD800..0xDC00).contains(&u)
        && let Some(&t) = s.get(index + 1)
        && (0xDC00..0xE000).contains(&t)
    {
        return 0x10000 + ((u - 0xD800) << 10) + (u32::from(t) - 0xDC00);
    }
    u
}

/// AdvanceStringIndex ( S, index, unicode ) (§22.2.7.3).
pub fn advance_string_index(s: &[u16], index: usize, unicode: bool) -> usize {
    if !unicode || index + 1 >= s.len() {
        return index + 1;
    }
    let lead = s[index];
    if (0xD800..0xDC00).contains(&lead) && (0xDC00..0xE000).contains(&s[index + 1]) {
        index + 2
    } else {
        index + 1
    }
}

#[cfg(test)]
mod tests;
