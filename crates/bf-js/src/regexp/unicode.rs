//! Conjuntos de caracteres (CharSet, §22.2.2.1 y §22.2.2.9), Canonicalize (§22.2.2.7.3) y las
//! propiedades Unicode de `\p{…}` (UnicodeMatchProperty / UnicodeMatchPropertyValue,
//! §22.2.2.9.7–8) sobre las tablas generadas de [`super::unicode_tables`].

use std::collections::BTreeSet;
use std::sync::OnceLock;

use super::unicode_tables as t;

pub const MAX_CODE_POINT: u32 = 0x10FFFF;
pub const MAX_CODE_UNIT: u32 = 0xFFFF;

/// Un conjunto de caracteres sueltos: rangos cerrados, disjuntos, no adyacentes y ordenados.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CharSet {
    ranges: Vec<(u32, u32)>,
}

impl CharSet {
    pub fn new() -> CharSet {
        CharSet::default()
    }

    pub fn from_ranges(rs: &[(u32, u32)]) -> CharSet {
        let mut s = CharSet {
            ranges: rs.to_vec(),
        };
        s.normalize();
        s
    }

    pub fn single(c: u32) -> CharSet {
        CharSet {
            ranges: vec![(c, c)],
        }
    }

    pub fn range(a: u32, b: u32) -> CharSet {
        CharSet {
            ranges: vec![(a, b)],
        }
    }

    pub fn ranges(&self) -> &[(u32, u32)] {
        &self.ranges
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// Cantidad de caracteres.
    pub fn len(&self) -> u64 {
        self.ranges.iter().map(|&(a, b)| u64::from(b - a) + 1).sum()
    }

    /// El único carácter, si tiene exactamente uno.
    pub fn as_single(&self) -> Option<u32> {
        match self.ranges.as_slice() {
            [(a, b)] if a == b => Some(*a),
            _ => None,
        }
    }

    fn normalize(&mut self) {
        self.ranges.sort_unstable();
        let mut out: Vec<(u32, u32)> = Vec::with_capacity(self.ranges.len());
        for &(a, b) in &self.ranges {
            if let Some(last) = out.last_mut()
                && a <= last.1.saturating_add(1)
            {
                last.1 = last.1.max(b);
                continue;
            }
            out.push((a, b));
        }
        self.ranges = out;
    }

    pub fn add(&mut self, c: u32) {
        self.add_range(c, c);
    }

    pub fn add_range(&mut self, a: u32, b: u32) {
        self.ranges.push((a, b));
        self.normalize();
    }

    pub fn add_set(&mut self, other: &CharSet) {
        self.ranges.extend_from_slice(&other.ranges);
        self.normalize();
    }

    pub fn contains(&self, c: u32) -> bool {
        let r = &self.ranges;
        // Búsqueda binaria del último rango que empieza en o antes de `c`.
        let i = r.partition_point(|&(a, _)| a <= c);
        i > 0 && c <= r[i - 1].1
    }

    pub fn union(&self, other: &CharSet) -> CharSet {
        let mut s = self.clone();
        s.add_set(other);
        s
    }

    pub fn intersect(&self, other: &CharSet) -> CharSet {
        let (a, b) = (&self.ranges, &other.ranges);
        let mut out = Vec::new();
        let (mut i, mut j) = (0, 0);
        while i < a.len() && j < b.len() {
            let lo = a[i].0.max(b[j].0);
            let hi = a[i].1.min(b[j].1);
            if lo <= hi {
                out.push((lo, hi));
            }
            if a[i].1 < b[j].1 {
                i += 1;
            } else {
                j += 1;
            }
        }
        CharSet { ranges: out }
    }

    /// El complemento dentro de `[0, max]`.
    pub fn complement(&self, max: u32) -> CharSet {
        let mut out = Vec::new();
        let mut next = 0u32;
        for &(a, b) in &self.ranges {
            if a > max {
                break;
            }
            if a > next {
                out.push((next, a - 1));
            }
            next = b.saturating_add(1);
        }
        if next <= max && (self.ranges.last().is_none_or(|l| l.1 < max)) {
            out.push((next, max));
        }
        CharSet { ranges: out }
    }

    pub fn subtract(&self, other: &CharSet) -> CharSet {
        self.intersect(&other.complement(u32::MAX - 1))
    }
}

/// Un CharSet del modo `v`: caracteres sueltos y strings (de longitud distinta de 1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClassSet {
    pub chars: CharSet,
    pub strings: BTreeSet<Vec<u32>>,
}

impl ClassSet {
    pub fn from_chars(chars: CharSet) -> ClassSet {
        ClassSet {
            chars,
            strings: BTreeSet::new(),
        }
    }

    /// Agrega un string (los de un carácter van a `chars`).
    pub fn add_string(&mut self, s: Vec<u32>) {
        if s.len() == 1 {
            self.chars.add(s[0]);
        } else {
            self.strings.insert(s);
        }
    }

    pub fn union(&self, o: &ClassSet) -> ClassSet {
        ClassSet {
            chars: self.chars.union(&o.chars),
            strings: self.strings.union(&o.strings).cloned().collect(),
        }
    }

    pub fn intersect(&self, o: &ClassSet) -> ClassSet {
        ClassSet {
            chars: self.chars.intersect(&o.chars),
            strings: self.strings.intersection(&o.strings).cloned().collect(),
        }
    }

    pub fn subtract(&self, o: &ClassSet) -> ClassSet {
        ClassSet {
            chars: self.chars.subtract(&o.chars),
            strings: self.strings.difference(&o.strings).cloned().collect(),
        }
    }
}

// --- Case folding y Canonicalize ---

/// scf(c) (Simple Case Folding de CaseFolding.txt).
pub fn simple_fold(c: u32) -> u32 {
    let tab = t::SIMPLE_CASE_FOLDING;
    match tab.binary_search_by_key(&c, |&(a, _)| a) {
        Ok(i) => tab[i].1,
        Err(_) => c,
    }
}

/// toUppercase de un code unit con las reglas de Canonicalize sin `u`/`v`.
fn non_unicode_upper(c: u32) -> u32 {
    if c > 0xFFFF {
        return c;
    }
    let tab = t::NON_UNICODE_UPPER;
    match tab.binary_search_by_key(&(c as u16), |&(a, _)| a) {
        Ok(i) => u32::from(tab[i].1),
        Err(_) => c,
    }
}

/// Canonicalize ( rer, ch ) (§22.2.2.7.3).
pub fn canonicalize(c: u32, unicode: bool, ignore_case: bool) -> u32 {
    if !ignore_case {
        c
    } else if unicode {
        simple_fold(c)
    } else {
        non_unicode_upper(c)
    }
}

/// La clausura de `a` bajo Canonicalize: `a ∪ { Canonicalize(x) | x ∈ a }`. Como
/// Canonicalize es idempotente, `Canonicalize(ch) ∈ clausura` equivale a "existe un miembro
/// de `a` cuyo Canonicalize es Canonicalize(ch)" (CharacterSetMatcher, §22.2.2.7.1).
pub fn canonical_closure(a: &CharSet, unicode: bool) -> CharSet {
    let mut extra = Vec::new();
    if unicode {
        for &(x, y) in t::SIMPLE_CASE_FOLDING {
            if a.contains(x) {
                extra.push((y, y));
            }
        }
    } else {
        for &(x, y) in t::NON_UNICODE_UPPER {
            if a.contains(u32::from(x)) {
                extra.push((u32::from(y), u32::from(y)));
            }
        }
    }
    if extra.is_empty() {
        return a.clone();
    }
    a.union(&CharSet::from_ranges(&extra))
}

/// Los code points con una asignación de Simple Case Folding (scf(c) ≠ c).
fn folding_sources() -> &'static CharSet {
    static S: OnceLock<CharSet> = OnceLock::new();
    S.get_or_init(|| {
        let rs: Vec<(u32, u32)> = t::SIMPLE_CASE_FOLDING
            .iter()
            .map(|&(a, _)| (a, a))
            .collect();
        CharSet::from_ranges(&rs)
    })
}

/// MaybeSimpleCaseFolding sobre caracteres sueltos: `{ scf(c) | c ∈ a }`.
pub fn simple_fold_set(a: &CharSet) -> CharSet {
    let src = folding_sources();
    let mut out = a.subtract(src);
    let mut extra = Vec::new();
    for &(x, y) in t::SIMPLE_CASE_FOLDING {
        if a.contains(x) {
            extra.push((y, y));
        }
    }
    if !extra.is_empty() {
        out.add_set(&CharSet::from_ranges(&extra));
    }
    out
}

/// MaybeSimpleCaseFolding ( rer, A ) (§22.2.2.9.5): solo con `v` e `i`.
pub fn maybe_simple_case_folding(a: ClassSet, v: bool, ignore_case: bool) -> ClassSet {
    if !v || !ignore_case {
        return a;
    }
    ClassSet {
        chars: simple_fold_set(&a.chars),
        strings: a
            .strings
            .into_iter()
            .map(|s| s.into_iter().map(simple_fold).collect())
            .collect(),
    }
}

/// AllCharacters ( rer ) (§22.2.2.9.4).
pub fn all_characters(unicode: bool, v: bool, ignore_case: bool) -> CharSet {
    if v && ignore_case {
        CharSet::range(0, MAX_CODE_POINT).subtract(folding_sources())
    } else if unicode {
        CharSet::range(0, MAX_CODE_POINT)
    } else {
        CharSet::range(0, MAX_CODE_UNIT)
    }
}

/// CharacterComplement ( rer, S ) (§22.2.2.9.6).
pub fn character_complement(s: &CharSet, unicode: bool, v: bool, ignore_case: bool) -> CharSet {
    all_characters(unicode, v, ignore_case).subtract(s)
}

/// WordCharacters ( rer ) (§22.2.2.9.3): `[A-Za-z0-9_]` y, con `u`/`v` e `i`, los caracteres
/// cuyo Canonicalize está entre ellos (U+017F, U+212A).
pub fn word_characters(unicode: bool, ignore_case: bool) -> CharSet {
    let mut basic = CharSet::from_ranges(&[
        (u32::from(b'0'), u32::from(b'9')),
        (u32::from(b'A'), u32::from(b'Z')),
        (u32::from(b'_'), u32::from(b'_')),
        (u32::from(b'a'), u32::from(b'z')),
    ]);
    if unicode && ignore_case {
        let mut extra = Vec::new();
        for &(x, y) in t::SIMPLE_CASE_FOLDING {
            if basic.contains(y) && !basic.contains(x) {
                extra.push((x, x));
            }
        }
        basic.add_set(&CharSet::from_ranges(&extra));
    }
    basic
}

/// `\d`.
pub fn digits() -> CharSet {
    CharSet::range(u32::from(b'0'), u32::from(b'9'))
}

/// `\s`: WhiteSpace (§12.2) y LineTerminator (§12.3).
pub fn white_space() -> CharSet {
    let mut s = CharSet::from_ranges(&[
        (0x09, 0x0D),
        (0x20, 0x20),
        (0xA0, 0xA0),
        (0x1680, 0x1680),
        (0x2028, 0x2029),
        (0x202F, 0x202F),
        (0x205F, 0x205F),
        (0x3000, 0x3000),
        (0xFEFF, 0xFEFF),
    ]);
    // Space_Separator (Zs).
    if let Some(zs) = general_category("Zs") {
        s.add_set(&CharSet::from_ranges(zs));
    }
    s
}

pub fn is_line_terminator(c: u32) -> bool {
    matches!(c, 0x0A | 0x0D | 0x2028 | 0x2029)
}

// --- Propiedades ---

fn general_category(value: &str) -> Option<&'static [(u32, u32)]> {
    t::GENERAL_CATEGORY
        .iter()
        .find(|(names, _)| names.contains(&value))
        .map(|(_, r)| *r)
}

/// El resultado de `\p{…}`: caracteres, o caracteres y strings (propiedades de strings).
pub enum Property {
    Chars(CharSet),
    Strings(ClassSet),
}

/// Resuelve `\p{name}` o `\p{name=value}` (UnicodeMatchProperty y UnicodeMatchPropertyValue:
/// los nombres canónicos y los alias de PropertyAliases.txt / PropertyValueAliases.txt, sin
/// loose matching). `strings` habilita las propiedades de strings (modo `v`).
pub fn lookup_property(name: &str, value: Option<&str>, strings: bool) -> Option<Property> {
    match value {
        Some(v) => {
            let rs = match name {
                "General_Category" | "gc" => general_category(v)?,
                "Script" | "sc" => t::SCRIPT.iter().find(|(names, _, _)| names.contains(&v))?.1,
                "Script_Extensions" | "scx" => {
                    t::SCRIPT.iter().find(|(names, _, _)| names.contains(&v))?.2
                }
                _ => return None,
            };
            Some(Property::Chars(CharSet::from_ranges(rs)))
        }
        None => {
            if let Some(rs) = general_category(name) {
                return Some(Property::Chars(CharSet::from_ranges(rs)));
            }
            if let Some((_, rs)) = t::BINARY.iter().find(|(names, _)| names.contains(&name)) {
                return Some(Property::Chars(CharSet::from_ranges(rs)));
            }
            if strings
                && let Some((_, singles, strs)) =
                    t::STRING_PROPERTIES.iter().find(|(n, _, _)| *n == name)
            {
                let mut set = ClassSet::from_chars(CharSet::from_ranges(singles));
                for s in *strs {
                    set.add_string(s.chars().map(|c| c as u32).collect());
                }
                return Some(Property::Strings(set));
            }
            None
        }
    }
}

/// ¿`name` es una propiedad de strings? (Para el early error de `\P{…}` y `[^…]`.)
pub fn is_string_property(name: &str) -> bool {
    t::STRING_PROPERTIES.iter().any(|(n, _, _)| *n == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_operations() {
        let a = CharSet::from_ranges(&[(1, 5), (10, 20)]);
        let b = CharSet::from_ranges(&[(4, 12)]);
        assert_eq!(a.union(&b).ranges(), &[(1, 20)]);
        assert_eq!(a.intersect(&b).ranges(), &[(4, 5), (10, 12)]);
        assert_eq!(a.subtract(&b).ranges(), &[(1, 3), (13, 20)]);
        assert_eq!(a.complement(25).ranges(), &[(0, 0), (6, 9), (21, 25)]);
        assert!(a.contains(15) && !a.contains(7) && !a.contains(21));
        assert_eq!(CharSet::range(0, 10).complement(10).ranges(), &[]);
    }

    #[test]
    fn canonicalize_rules() {
        assert_eq!(canonicalize('a' as u32, false, true), 'A' as u32);
        // ſ → S tiene un resultado ASCII desde fuera de ASCII: queda ſ.
        assert_eq!(canonicalize(0x17F, false, true), 0x17F);
        // ß → "SS": longitud distinta de 1.
        assert_eq!(canonicalize(0xDF, false, true), 0xDF);
        assert_eq!(canonicalize('A' as u32, true, true), 'a' as u32);
        assert_eq!(canonicalize(0x17F, true, true), 's' as u32);
        assert_eq!(canonicalize(0x212A, true, true), 'k' as u32);
        // Idempotencia.
        for &(_, y) in t::SIMPLE_CASE_FOLDING {
            assert_eq!(simple_fold(y), y, "scf no idempotente en {y:X}");
        }
        for &(_, y) in t::NON_UNICODE_UPPER {
            let y = u32::from(y);
            assert_eq!(
                non_unicode_upper(y),
                y,
                "toUppercase no idempotente en {y:X}"
            );
        }
    }

    #[test]
    fn properties() {
        let Some(Property::Chars(l)) = lookup_property("L", None, false) else {
            panic!()
        };
        assert!(l.contains('a' as u32) && !l.contains('1' as u32));
        let Some(Property::Chars(g)) = lookup_property("Script", Some("Greek"), false) else {
            panic!()
        };
        assert!(g.contains(0x3B1));
        assert!(lookup_property("RGI_Emoji", None, false).is_none());
        assert!(matches!(
            lookup_property("RGI_Emoji", None, true),
            Some(Property::Strings(_))
        ));
        assert!(word_characters(true, true).contains(0x17F));
        assert!(!word_characters(false, true).contains(0x17F));
    }
}
