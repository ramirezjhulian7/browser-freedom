//! `DomString`: el texto del DOM, una secuencia de code units UTF-16 (DOM Standard, WebIDL
//! `DOMString`) que puede contener surrogates sueltos. Se guarda en **WTF-8**
//! (<https://simonsapin.github.io/wtf-8/>): un superconjunto de UTF-8 en el que un surrogate
//! suelto se codifica como su code point de 3 bytes y un par de surrogates siempre se combina
//! en el code point suplementario.
//!
//! Representación: `Utf8(String)` cuando no hay surrogates sueltos (el caso común) y
//! `Wtf8 { bytes, lossy }` cuando hay al menos uno; `lossy` es el mismo texto con cada
//! surrogate suelto cambiado por U+FFFD (que también ocupa 3 bytes, así que las posiciones en
//! bytes coinciden). La igualdad y el hash dependen solo de los code units.
//!
//! - [`DomString::as_str`] da el `&str` exacto, o `None` si hay surrogates sueltos.
//! - `Deref<Target = str>` da la **vista con U+FFFD**: es lo que usan el estilo, el layout y la
//!   pintura (para ellos un surrogate suelto se muestra como U+FFFD). Los algoritmos del DOM
//!   trabajan con code units ([`DomString::code_units`], [`DomString::substring_utf16`], ...).

use std::borrow::Cow;
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;

#[derive(Clone)]
enum Repr {
    Utf8(String),
    /// WTF-8 bien formado con al menos un surrogate suelto, y su vista con U+FFFD.
    Wtf8 {
        bytes: Vec<u8>,
        lossy: String,
    },
}

/// Texto del DOM (ver el módulo).
#[derive(Clone)]
pub struct DomString(Repr);

fn is_surrogate(cp: u32) -> bool {
    (0xD800..=0xDFFF).contains(&cp)
}

/// Codifica un code point (surrogates incluidos) como WTF-8.
fn encode_cp(cp: u32, out: &mut Vec<u8>) {
    match cp {
        0..=0x7F => out.push(cp as u8),
        0x80..=0x7FF => {
            out.push(0xC0 | (cp >> 6) as u8);
            out.push(0x80 | (cp & 0x3F) as u8);
        }
        0x800..=0xFFFF => {
            out.push(0xE0 | (cp >> 12) as u8);
            out.push(0x80 | ((cp >> 6) & 0x3F) as u8);
            out.push(0x80 | (cp & 0x3F) as u8);
        }
        _ => {
            out.push(0xF0 | (cp >> 18) as u8);
            out.push(0x80 | ((cp >> 12) & 0x3F) as u8);
            out.push(0x80 | ((cp >> 6) & 0x3F) as u8);
            out.push(0x80 | (cp & 0x3F) as u8);
        }
    }
}

/// Decodifica WTF-8 bien formado en code points.
struct CodePoints<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Iterator for CodePoints<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        let b = &self.bytes[self.pos..];
        let first = *b.first()?;
        let (cp, n) = match first {
            0..=0x7F => (u32::from(first), 1),
            0xC0..=0xDF => ((u32::from(first & 0x1F) << 6) | u32::from(b[1] & 0x3F), 2),
            0xE0..=0xEF => (
                (u32::from(first & 0x0F) << 12)
                    | (u32::from(b[1] & 0x3F) << 6)
                    | u32::from(b[2] & 0x3F),
                3,
            ),
            _ => (
                (u32::from(first & 0x07) << 18)
                    | (u32::from(b[1] & 0x3F) << 12)
                    | (u32::from(b[2] & 0x3F) << 6)
                    | u32::from(b[3] & 0x3F),
                4,
            ),
        };
        self.pos += n;
        Some(cp)
    }
}

impl DomString {
    pub fn new() -> Self {
        DomString(Repr::Utf8(String::new()))
    }

    /// Desde code units UTF-16 (los pares válidos se combinan; los sueltos se conservan).
    pub fn from_utf16(units: &[u16]) -> Self {
        let mut s = DomString::new();
        for &u in units {
            s.push_code_unit(u);
        }
        s
    }

    /// Desde code points (los surrogates se aceptan y se combinan si forman pares).
    pub fn from_code_points(cps: impl IntoIterator<Item = u32>) -> Self {
        let mut s = DomString::new();
        for cp in cps {
            s.push_code_point(cp);
        }
        s
    }

    fn bytes(&self) -> &[u8] {
        match &self.0 {
            Repr::Utf8(s) => s.as_bytes(),
            Repr::Wtf8 { bytes, .. } => bytes,
        }
    }

    fn lossy(&self) -> &str {
        match &self.0 {
            Repr::Utf8(s) => s,
            Repr::Wtf8 { lossy, .. } => lossy,
        }
    }

    /// El texto como `&str`, o `None` si contiene surrogates sueltos.
    pub fn as_str(&self) -> Option<&str> {
        match &self.0 {
            Repr::Utf8(s) => Some(s),
            Repr::Wtf8 { .. } => None,
        }
    }

    /// La vista `&str` con U+FFFD en lugar de los surrogates sueltos (la misma que `Deref`).
    pub fn as_lossy_str(&self) -> &str {
        self.lossy()
    }

    /// El texto con cada surrogate suelto reemplazado por U+FFFD.
    pub fn to_string_lossy(&self) -> Cow<'_, str> {
        Cow::Borrowed(self.lossy())
    }

    /// Como [`Self::to_string_lossy`], consumiendo el valor.
    pub fn into_string_lossy(self) -> String {
        match self.0 {
            Repr::Utf8(s) => s,
            Repr::Wtf8 { lossy, .. } => lossy,
        }
    }

    /// ¿Contiene surrogates sueltos?
    pub fn has_lone_surrogates(&self) -> bool {
        matches!(self.0, Repr::Wtf8 { .. })
    }

    /// Code points (con los surrogates sueltos como tales).
    pub fn code_points(&self) -> impl Iterator<Item = u32> + '_ {
        CodePoints {
            bytes: self.bytes(),
            pos: 0,
        }
    }

    /// Code units UTF-16.
    pub fn code_units(&self) -> impl Iterator<Item = u16> + '_ {
        self.code_points().flat_map(|cp| {
            if cp >= 0x10000 {
                let v = cp - 0x10000;
                [
                    Some(0xD800 | (v >> 10) as u16),
                    Some(0xDC00 | (v & 0x3FF) as u16),
                ]
            } else {
                [Some(cp as u16), None]
            }
            .into_iter()
            .flatten()
        })
    }

    pub fn to_utf16(&self) -> Vec<u16> {
        self.code_units().collect()
    }

    /// Longitud en code units UTF-16 (la `length` del DOM).
    pub fn len_utf16(&self) -> usize {
        self.code_points()
            .map(|cp| if cp >= 0x10000 { 2 } else { 1 })
            .sum()
    }

    /// Longitud en bytes WTF-8 (igual a la de la vista `&str`).
    pub fn len(&self) -> usize {
        self.bytes().len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes().is_empty()
    }

    pub fn clear(&mut self) {
        self.0 = Repr::Utf8(String::new());
    }

    pub fn push(&mut self, c: char) {
        match &mut self.0 {
            Repr::Utf8(s) => s.push(c),
            Repr::Wtf8 { bytes, lossy } => {
                encode_cp(u32::from(c), bytes);
                lossy.push(c);
            }
        }
    }

    pub fn push_str(&mut self, s: &str) {
        match &mut self.0 {
            Repr::Utf8(own) => own.push_str(s),
            Repr::Wtf8 { bytes, lossy } => {
                bytes.extend_from_slice(s.as_bytes());
                lossy.push_str(s);
            }
        }
    }

    /// Agrega un code point; un trail surrogate tras un lead surrogate suelto se combina en el
    /// code point suplementario (concatenación WTF-8).
    pub fn push_code_point(&mut self, cp: u32) {
        if let Some(c) = char::from_u32(cp) {
            self.push(c);
            return;
        }
        if !is_surrogate(cp) {
            self.push('\u{FFFD}');
            return;
        }
        let mut bytes = match std::mem::replace(&mut self.0, Repr::Utf8(String::new())) {
            Repr::Utf8(s) => s.into_bytes(),
            Repr::Wtf8 { bytes, .. } => bytes,
        };
        let n = bytes.len();
        if (0xDC00..=0xDFFF).contains(&cp)
            && n >= 3
            && bytes[n - 3] == 0xED
            && (0xA0..=0xAF).contains(&bytes[n - 2])
        {
            let lead =
                0xD000 | (u32::from(bytes[n - 2] & 0x3F) << 6) | u32::from(bytes[n - 1] & 0x3F);
            bytes.truncate(n - 3);
            encode_cp(
                0x10000 + ((lead - 0xD800) << 10) + (cp - 0xDC00),
                &mut bytes,
            );
        } else {
            encode_cp(cp, &mut bytes);
        }
        self.0 = Self::repr_from_wtf8(bytes);
    }

    /// La representación de bytes WTF-8 bien formados.
    fn repr_from_wtf8(bytes: Vec<u8>) -> Repr {
        if std::str::from_utf8(&bytes).is_ok() {
            return Repr::Utf8(String::from_utf8(bytes).expect("UTF-8 ya validado"));
        }
        let lossy = CodePoints {
            bytes: &bytes,
            pos: 0,
        }
        .map(|cp| char::from_u32(cp).unwrap_or('\u{FFFD}'))
        .collect();
        Repr::Wtf8 { bytes, lossy }
    }

    pub fn push_code_unit(&mut self, u: u16) {
        self.push_code_point(u32::from(u));
    }

    /// Concatena otro `DomString` (combinando un surrogate suelto final con uno inicial).
    pub fn push_dom_str(&mut self, other: &DomString) {
        match &other.0 {
            Repr::Utf8(s) => self.push_str(s),
            Repr::Wtf8 { .. } => {
                for cp in other.code_points() {
                    self.push_code_point(cp);
                }
            }
        }
    }

    /// Subcadena por code units UTF-16 `[start, end)`; un par partido queda suelto.
    pub fn substring_utf16(&self, start: usize, end: usize) -> DomString {
        let units: Vec<u16> = self
            .code_units()
            .skip(start)
            .take(end.saturating_sub(start))
            .collect();
        DomString::from_utf16(&units)
    }

    /// ASCII lowercase (Infra): solo cambia A-Z.
    pub fn make_ascii_lowercase(&mut self) {
        match &mut self.0 {
            Repr::Utf8(s) => s.make_ascii_lowercase(),
            Repr::Wtf8 { bytes, lossy } => {
                bytes.make_ascii_lowercase();
                lossy.make_ascii_lowercase();
            }
        }
    }

    /// ASCII lowercase como `DomString`.
    pub fn ascii_lowercase(&self) -> DomString {
        let mut s = self.clone();
        s.make_ascii_lowercase();
        s
    }

    /// ASCII uppercase como `DomString`.
    pub fn ascii_uppercase(&self) -> DomString {
        let mut s = self.clone();
        match &mut s.0 {
            Repr::Utf8(own) => own.make_ascii_uppercase(),
            Repr::Wtf8 { bytes, lossy } => {
                bytes.make_ascii_uppercase();
                lossy.make_ascii_uppercase();
            }
        }
        s
    }
}

impl Deref for DomString {
    type Target = str;

    /// La vista con U+FFFD (ver el módulo).
    fn deref(&self) -> &str {
        self.lossy()
    }
}

impl PartialEq for DomString {
    fn eq(&self, other: &Self) -> bool {
        self.bytes() == other.bytes()
    }
}

impl Eq for DomString {}

impl Hash for DomString {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.bytes().hash(state);
    }
}

impl Default for DomString {
    fn default() -> Self {
        DomString::new()
    }
}

impl fmt::Display for DomString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.lossy())
    }
}

impl fmt::Debug for DomString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Repr::Utf8(s) => fmt::Debug::fmt(s, f),
            Repr::Wtf8 { .. } => {
                f.write_str("\"")?;
                for cp in self.code_points() {
                    match char::from_u32(cp) {
                        Some(c) => write!(f, "{}", c.escape_debug())?,
                        None => write!(f, "\\u{{{cp:x}}}")?,
                    }
                }
                f.write_str("\"")
            }
        }
    }
}

/// Orden por code units UTF-16 (el del DOM), distinto del orden por code points.
impl Ord for DomString {
    fn cmp(&self, other: &Self) -> Ordering {
        self.code_units().cmp(other.code_units())
    }
}

impl PartialOrd for DomString {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl From<&str> for DomString {
    fn from(s: &str) -> Self {
        DomString(Repr::Utf8(s.to_owned()))
    }
}

impl From<String> for DomString {
    fn from(s: String) -> Self {
        DomString(Repr::Utf8(s))
    }
}

impl From<&String> for DomString {
    fn from(s: &String) -> Self {
        DomString(Repr::Utf8(s.clone()))
    }
}

impl From<Cow<'_, str>> for DomString {
    fn from(s: Cow<'_, str>) -> Self {
        DomString(Repr::Utf8(s.into_owned()))
    }
}

impl From<char> for DomString {
    fn from(c: char) -> Self {
        DomString(Repr::Utf8(c.to_string()))
    }
}

impl From<&DomString> for DomString {
    fn from(s: &DomString) -> Self {
        s.clone()
    }
}

impl FromIterator<char> for DomString {
    fn from_iter<I: IntoIterator<Item = char>>(iter: I) -> Self {
        DomString(Repr::Utf8(iter.into_iter().collect()))
    }
}

impl PartialEq<str> for DomString {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == Some(other)
    }
}

impl PartialEq<&str> for DomString {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == Some(*other)
    }
}

impl PartialEq<String> for DomString {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == Some(other.as_str())
    }
}

impl PartialEq<DomString> for str {
    fn eq(&self, other: &DomString) -> bool {
        other == self
    }
}

impl PartialEq<DomString> for &str {
    fn eq(&self, other: &DomString) -> bool {
        other == *self
    }
}

impl PartialEq<DomString> for String {
    fn eq(&self, other: &DomString) -> bool {
        other == self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_round_trip() {
        let units = [0x61, 0xD800, 0x62, 0xD83D, 0xDE00, 0xDFFF];
        let s = DomString::from_utf16(&units);
        assert!(s.has_lone_surrogates());
        assert_eq!(s.to_utf16(), units);
        assert_eq!(s.len_utf16(), 6);
        assert_eq!(s.as_str(), None);
        assert_eq!(&*s, "a\u{FFFD}b😀\u{FFFD}");
        assert_eq!(s.len(), "a\u{FFFD}b😀\u{FFFD}".len());
        assert_eq!(format!("{s}"), "a\u{FFFD}b😀\u{FFFD}");
        assert_ne!(s, DomString::from("a\u{FFFD}b😀\u{FFFD}"));
    }

    #[test]
    fn surrogate_pairs_join() {
        let mut s = DomString::from("x");
        s.push_code_unit(0xD83D);
        assert!(s.has_lone_surrogates());
        s.push_code_unit(0xDE00);
        assert!(!s.has_lone_surrogates());
        assert_eq!(s, "x😀");
        let mut a = DomString::from_utf16(&[0xD83D]);
        a.push_dom_str(&DomString::from_utf16(&[0xDE00]));
        assert_eq!(a, "😀");
    }

    #[test]
    fn order_by_code_units() {
        // U+FF61 < U+1F600 por code points, pero 0xFF61 > 0xD83D por code units.
        let a = DomString::from("\u{FF61}");
        let b = DomString::from("😀");
        assert_eq!(a.cmp(&b), Ordering::Greater);
        assert_eq!(DomString::from("a"), "a");
        assert_eq!(
            DomString::from("a😀b").substring_utf16(1, 2).to_utf16(),
            [0xD83D]
        );
    }
}
