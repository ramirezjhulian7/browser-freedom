//! Strings de ECMAScript (ECMA-262 §6.1.4): secuencias de code units UTF-16, que pueden tener
//! surrogates sueltos. [`JsString`] es el valor de los literales y (en el intérprete) de los
//! strings en tiempo de ejecución; [`Atom`] es un nombre internado (identificadores y claves de
//! propiedades), que siempre es Unicode válido.

use std::cell::{OnceCell, RefCell};
use std::collections::HashSet;
use std::fmt;
use std::mem::ManuallyDrop;
use std::rc::Rc;

/// Un string de ECMAScript: code units UTF-16 compartidos. Una concatenación larga es un
/// *rope* (E3): guarda las dos partes y se aplana una sola vez, la primera vez que se leen sus
/// code units; así `s += x` en un bucle no copia el string entero en cada vuelta.
pub struct JsString(Repr);

impl Clone for JsString {
    #[inline(always)]
    fn clone(&self) -> JsString {
        JsString(match &self.0 {
            Repr::Flat(u) => Repr::Flat(Rc::clone(u)),
            Repr::Rope(r) => Repr::Rope(Rc::clone(r)),
        })
    }
}

enum Repr {
    Flat(Rc<[u16]>),
    Rope(Rc<Rope>),
}

/// Una concatenación pendiente.
struct Rope {
    len: usize,
    /// Las partes, hasta que se aplana. `ManuallyDrop`: las libera [`Rope::drop`] sin
    /// recursión (y el código que libera un `JsString` queda chico y en línea).
    parts: ManuallyDrop<RefCell<Option<(JsString, JsString)>>>,
    flat: OnceCell<Rc<[u16]>>,
}

/// Desde este largo una concatenación es un rope (las cortas se copian).
const ROPE_MIN: usize = 256;

/// El largo máximo de un string (como `String.prototype.padStart`).
pub const MAX_LEN: usize = 1 << 30;

impl Rope {
    /// Los code units, aplanando la primera vez (sin recursión: un rope puede tener miles de
    /// niveles).
    #[inline(never)]
    fn units(&self) -> &[u16] {
        self.flat.get_or_init(|| {
            let mut out = Vec::with_capacity(self.len);
            let mut stack: Vec<JsString> = Vec::new();
            if let Some((a, b)) = self.parts.borrow_mut().take() {
                stack.push(b);
                stack.push(a);
            }
            while let Some(s) = stack.pop() {
                match &s.0 {
                    Repr::Flat(u) => out.extend_from_slice(u),
                    Repr::Rope(r) => match r.flat.get() {
                        Some(u) => out.extend_from_slice(u),
                        None => {
                            if let Some((a, b)) = &*r.parts.borrow() {
                                stack.push(b.clone());
                                stack.push(a.clone());
                            }
                        }
                    },
                }
            }
            out.into()
        })
    }
}

impl Drop for Rope {
    /// Sin recursión: las partes que solo este rope comparte se desarman con una pila.
    fn drop(&mut self) {
        let Some((a, b)) = self.parts.get_mut().take() else {
            return;
        };
        let mut stack = vec![a, b];
        while let Some(s) = stack.pop() {
            if let Repr::Rope(rc) = s.0
                && let Ok(mut r) = Rc::try_unwrap(rc)
                && let Some((a, b)) = r.parts.get_mut().take()
            {
                stack.push(a);
                stack.push(b);
            }
        }
    }
}

impl Default for JsString {
    fn default() -> JsString {
        JsString(Repr::Flat(Rc::from([])))
    }
}

impl PartialEq for JsString {
    fn eq(&self, other: &JsString) -> bool {
        if let (Repr::Flat(a), Repr::Flat(b)) = (&self.0, &other.0)
            && Rc::ptr_eq(a, b)
        {
            return true;
        }
        self.len() == other.len() && self.units() == other.units()
    }
}

impl Eq for JsString {}

impl std::hash::Hash for JsString {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.units().hash(state);
    }
}

impl PartialOrd for JsString {
    fn partial_cmp(&self, other: &JsString) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for JsString {
    fn cmp(&self, other: &JsString) -> std::cmp::Ordering {
        self.units().cmp(other.units())
    }
}

impl JsString {
    pub fn from_units(units: Vec<u16>) -> JsString {
        JsString(Repr::Flat(units.into()))
    }

    /// Una copia de `units` (una sola reserva de memoria).
    pub fn from_slice(units: &[u16]) -> JsString {
        JsString(Repr::Flat(units.into()))
    }

    /// `a` seguido de `b`, con una sola reserva de memoria (E3).
    pub fn concat(a: &[u16], b: &[u16]) -> JsString {
        JsString(Repr::Flat(a.iter().chain(b).copied().collect()))
    }

    /// `a` seguido de `b` sin copiar si el resultado es largo (un rope); `None` si superaría
    /// [`MAX_LEN`] (E3).
    pub fn concat_strings(a: &JsString, b: &JsString) -> Option<JsString> {
        if a.is_empty() {
            return Some(b.clone());
        }
        if b.is_empty() {
            return Some(a.clone());
        }
        let len = a.len() + b.len();
        if len > MAX_LEN {
            return None;
        }
        if len < ROPE_MIN {
            return Some(JsString::concat(a.units(), b.units()));
        }
        Some(JsString(Repr::Rope(Rc::new(Rope {
            len,
            parts: ManuallyDrop::new(RefCell::new(Some((a.clone(), b.clone())))),
            flat: OnceCell::new(),
        }))))
    }

    #[inline]
    pub fn units(&self) -> &[u16] {
        match &self.0 {
            Repr::Flat(u) => u,
            Repr::Rope(r) => r.units(),
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        match &self.0 {
            Repr::Flat(u) => u.len(),
            Repr::Rope(r) => r.len,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// ¿Es Unicode bien formado (sin surrogates sueltos)? (`IsStringWellFormedUnicode`, §7.2.9.)
    pub fn is_well_formed(&self) -> bool {
        char::decode_utf16(self.units().iter().copied()).all(|r| r.is_ok())
    }

    /// Conversión a `String` con U+FFFD en lugar de los surrogates sueltos.
    pub fn to_string_lossy(&self) -> String {
        char::decode_utf16(self.units().iter().copied())
            .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect()
    }

    /// El string si es Unicode válido.
    pub fn to_str(&self) -> Option<String> {
        String::from_utf16(self.units()).ok()
    }
}

impl From<&str> for JsString {
    fn from(s: &str) -> JsString {
        JsString(Repr::Flat(s.encode_utf16().collect::<Vec<_>>().into()))
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
    fn ropes_flatten_lazily_and_compare_by_content() {
        assert_eq!(std::mem::size_of::<JsString>(), 16);
        let piece = JsString::from("x".repeat(300).as_str());
        let mut s = JsString::default();
        for _ in 0..20_000 {
            s = JsString::concat_strings(&s, &piece).expect("largo");
        }
        assert_eq!(s.len(), 300 * 20_000);
        let flat = JsString::from_units(vec![0x78; 300 * 20_000]);
        assert_eq!(s, flat);
        // Un rope profundo se libera sin desbordar la pila.
        let mut deep = JsString::from("y".repeat(ROPE_MIN).as_str());
        for _ in 0..200_000 {
            deep = JsString::concat_strings(&deep, &JsString::from("z")).expect("largo");
        }
        assert_eq!(deep.len(), ROPE_MIN + 200_000);
        drop(deep);
    }

    #[test]
    fn atoms_are_interned() {
        let a = Atom::new("foo");
        let b = Atom::new(&String::from("foo"));
        assert!(Rc::ptr_eq(&a.0, &b.0));
        assert_eq!(a, "foo");
    }
}
