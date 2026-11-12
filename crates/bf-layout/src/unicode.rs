//! E9: propiedades Unicode por carácter para el texto internacional (UCD 17.0.0): Script
//! (UAX #24) y Vertical_Orientation (UAX #50). Las tablas las genera
//! `tools/gen_unicode_text.py`.

use crate::unicode_tables::{SCRIPT, SCRIPT_NAMES, VERTICAL_ORIENTATION};

fn lookup<T: Copy>(table: &[(u32, u32, T)], c: char, default: T) -> T {
    let cp = c as u32;
    match table.binary_search_by(|&(a, b, _)| {
        if cp < a {
            std::cmp::Ordering::Greater
        } else if cp > b {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Equal
        }
    }) {
        Ok(i) => table[i].2,
        Err(_) => default,
    }
}

/// Un valor de la propiedad Script.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Script(u8);

impl Script {
    pub const UNKNOWN: Script = Script(0);

    /// Valor largo de la UCD (`"Arabic"`, `"Han"`…).
    pub fn name(self) -> &'static str {
        SCRIPT_NAMES[self.0 as usize].0
    }

    /// Etiqueta ISO 15924 (`"Arab"`, `"Hani"`…).
    pub fn tag(self) -> &'static str {
        SCRIPT_NAMES[self.0 as usize].1
    }

    /// `Common`, `Inherited` o `Unknown`: toman el script del texto que los rodea (UAX #24
    /// §5.1, resolución de Script_Extensions simplificada).
    pub fn is_neutral(self) -> bool {
        matches!(self.tag(), "Zyyy" | "Zinh" | "Zzzz")
    }
}

/// Script de un carácter.
pub fn script(c: char) -> Script {
    Script(lookup(SCRIPT, c, 0))
}

/// Runs de script de `text` (offsets en bytes): los caracteres neutros (`Common`,
/// `Inherited`) se unen al run anterior, o al siguiente si están al principio.
pub fn script_runs(text: &str) -> Vec<(usize, usize, Script)> {
    let mut runs: Vec<(usize, usize, Script)> = Vec::new();
    for (i, c) in text.char_indices() {
        let end = i + c.len_utf8();
        let s = script(c);
        match runs.last_mut() {
            Some(last) if s.is_neutral() || s == last.2 => last.1 = end,
            Some(last) if last.2.is_neutral() => {
                last.1 = end;
                last.2 = s;
            }
            _ => runs.push((i, end, s)),
        }
    }
    runs
}

/// Vertical_Orientation (UAX #50 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalOrientation {
    /// Rotado 90° en sentido horario.
    R,
    /// Vertical (sin rotar).
    U,
    /// Transformado (forma vertical propia) o, si no la hay, vertical.
    Tu,
    /// Transformado o, si no la hay, rotado.
    Tr,
}

/// Vertical_Orientation de un carácter.
pub fn vertical_orientation(c: char) -> VerticalOrientation {
    match lookup(VERTICAL_ORIENTATION, c, 0) {
        1 => VerticalOrientation::U,
        2 => VerticalOrientation::Tu,
        3 => VerticalOrientation::Tr,
        _ => VerticalOrientation::R,
    }
}

/// ¿El carácter se pone vertical con `text-orientation: mixed` (CSS Writing Modes 4 §5.1:
/// U y Tu verticales; R y Tr rotados)?
pub fn upright_in_mixed(c: char) -> bool {
    matches!(
        vertical_orientation(c),
        VerticalOrientation::U | VerticalOrientation::Tu
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts() {
        assert_eq!(script('a').tag(), "Latn");
        assert_eq!(script('\u{627}').tag(), "Arab");
        assert_eq!(script('\u{5D0}').tag(), "Hebr");
        assert_eq!(script('日').tag(), "Hani");
        assert!(script(' ').is_neutral());
        let runs = script_runs("ab \u{627}\u{628} 12");
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].2.tag(), "Latn");
        assert_eq!(runs[1].2.tag(), "Arab");
    }

    #[test]
    fn vertical() {
        assert_eq!(vertical_orientation('a'), VerticalOrientation::R);
        assert_eq!(vertical_orientation('日'), VerticalOrientation::U);
        assert!(upright_in_mixed('あ'));
        assert!(!upright_in_mixed('1'));
    }
}
