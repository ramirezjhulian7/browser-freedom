//! `Atom`: strings internados. Un `Atom` es un índice `Copy` en una tabla global; los strings
//! internados viven hasta el final del proceso (se filtran con `Box::leak`), así que
//! [`Atom::as_str`] devuelve `&'static str`.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Atom(u32);

#[derive(Default)]
struct Interner {
    by_str: HashMap<&'static str, u32>,
    strings: Vec<&'static str>,
}

fn interner() -> &'static Mutex<Interner> {
    static INTERNER: OnceLock<Mutex<Interner>> = OnceLock::new();
    INTERNER.get_or_init(Default::default)
}

impl Atom {
    /// Interna `s`.
    pub fn new(s: &str) -> Atom {
        let mut i = interner().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&id) = i.by_str.get(s) {
            return Atom(id);
        }
        let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
        let id = u32::try_from(i.strings.len()).expect("tabla de atoms llena");
        i.strings.push(leaked);
        i.by_str.insert(leaked, id);
        Atom(id)
    }

    pub fn as_str(self) -> &'static str {
        let i = interner().lock().unwrap_or_else(|e| e.into_inner());
        i.strings[self.0 as usize]
    }
}

impl fmt::Debug for Atom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Atom({:?})", self.as_str())
    }
}

impl fmt::Display for Atom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning() {
        let a = Atom::new("urn:x");
        let b = Atom::new(&String::from("urn:x"));
        assert_eq!(a, b);
        assert_ne!(a, Atom::new("urn:y"));
        assert_eq!(a.as_str(), "urn:x");
    }
}
