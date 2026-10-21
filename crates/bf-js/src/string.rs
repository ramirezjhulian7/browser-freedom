//! Strings de ECMAScript (ECMA-262 §6.1.4): secuencias de code units UTF-16, que pueden tener
//! surrogates sueltos. [`JsString`] es el valor de los literales y (en el intérprete) de los
//! strings en tiempo de ejecución; [`Atom`] es un nombre internado (identificadores y claves de
//! propiedades), que siempre es Unicode válido.

use std::cell::RefCell;
use std::collections::HashSet;
use std::fmt;
use std::rc::Rc;

/// Un string de ECMAScript: code units UTF-16 compartidos.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct JsString(Rc<[u16]>);

impl JsString {
    pub fn from_units(units: Vec<u16>) -> JsString {
        JsString(units.into())
    }

    /// `a` seguido de `b`, con una sola reserva de memoria (E3).
    pub fn concat(a: &[u16], b: &[u16]) -> JsString {
        JsString(a.iter().chain(b).copied().collect())
    }

    pub fn units(&self) -> &[u16] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// ¿Es Unicode bien formado (sin surrogates sueltos)? (`IsStringWellFormedUnicode`, §7.2.9.)
    pub fn is_well_formed(&self) -> bool {
        char::decode_utf16(self.0.iter().copied()).all(|r| r.is_ok())
    }

    /// Conversión a `String` con U+FFFD en lugar de los surrogates sueltos.
    pub fn to_string_lossy(&self) -> String {
        char::decode_utf16(self.0.iter().copied())
            .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect()
    }

    /// El string si es Unicode válido.
    pub fn to_str(&self) -> Option<String> {
        String::from_utf16(&self.0).ok()
    }
}

impl From<&str> for JsString {
    fn from(s: &str) -> JsString {
        JsString(s.encode_utf16().collect::<Vec<_>>().into())
    }
}

impl fmt::Display for JsString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string_lossy())
    }
}

impl fmt::Debug for JsString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.to_string_lossy())
    }
}

thread_local! {
    static INTERNED: RefCell<HashSet<Rc<str>>> = RefCell::new(HashSet::new());
}

/// Un nombre internado: dos `Atom` con el mismo texto comparten la memoria.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Atom(Rc<str>);

impl Atom {
    pub fn new(s: &str) -> Atom {
        INTERNED.with(|set| {
            let mut set = set.borrow_mut();
            if let Some(existing) = set.get(s) {
                return Atom(existing.clone());
            }
            let rc: Rc<str> = Rc::from(s);
            set.insert(rc.clone());
            Atom(rc)
        })
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn to_js_string(&self) -> JsString {
        JsString::from(self.as_str())
    }
}

impl std::ops::Deref for Atom {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl PartialEq<str> for Atom {
    fn eq(&self, other: &str) -> bool {
        &*self.0 == other
    }
}

impl PartialEq<&str> for Atom {
    fn eq(&self, other: &&str) -> bool {
        &*self.0 == *other
    }
}

impl fmt::Display for Atom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Atom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", &*self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_and_surrogates() {
        let s = JsString::from("a😀");
        assert_eq!(s.units(), &[0x61, 0xD83D, 0xDE00]);
        assert!(s.is_well_formed());
        let lone = JsString::from_units(vec![0x61, 0xD800]);
        assert!(!lone.is_well_formed());
        assert_eq!(lone.to_string(), "a\u{FFFD}");
        assert!(JsString::from("b") > JsString::from("a"));
    }

    #[test]
    fn atoms_are_interned() {
        let a = Atom::new("foo");
        let b = Atom::new(&String::from("foo"));
        assert!(Rc::ptr_eq(&a.0, &b.0));
        assert_eq!(a, "foo");
    }
}
