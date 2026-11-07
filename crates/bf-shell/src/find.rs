//! Buscar en la página (`Ctrl+F`) y la comparación de texto que usan también el historial y
//! los marcadores.
//!
//! La comparación es por equivalencia canónica (Unicode §3.7, UAX #15): texto y búsqueda se
//! llevan a NFD carácter a carácter (dos cadenas son NFC-iguales si y solo si son NFD-iguales)
//! y se pasan a minúsculas, así "Ñandú", "ñandú" y "n\u{303}andu\u{301}" coinciden. Si la
//! búsqueda no lleva diacríticos, tampoco se distinguen en el texto ("arbol" encuentra
//! "árbol"), como hacen los navegadores por omisión; con diacríticos se respetan ("árbol" no
//! encuentra "arbol").
//!
//! Descomponer carácter a carácter guarda para cada carácter plegado el carácter original del
//! que sale, y así una coincidencia vuelve a las posiciones de la página para resaltarla.

use unicode_normalization::char::{decompose_canonical, is_combining_mark};

/// Pliega `c` (NFD, minúsculas y, sin `marks`, sin marcas combinantes) en `out`.
pub fn fold_char(c: char, marks: bool, out: &mut Vec<char>) {
    decompose_canonical(c, |d| {
        if !marks && is_combining_mark(d) {
            return;
        }
        out.extend(d.to_lowercase());
    });
}

/// ¿La búsqueda lleva diacríticos (marcas combinantes en NFD)?
pub fn has_marks(query: &str) -> bool {
    let mut any = false;
    for c in query.chars() {
        decompose_canonical(c, |d| any |= is_combining_mark(d));
    }
    any
}

/// `s` plegado para comparar con una búsqueda que lleva (o no) diacríticos.
pub fn fold(s: &str, marks: bool) -> Vec<char> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        fold_char(c, marks, &mut out);
    }
    out
}

/// La búsqueda plegada: sin espacios en los extremos y los internos colapsados a uno.
pub fn fold_query(query: &str) -> Vec<char> {
    let marks = has_marks(query);
    let words: Vec<&str> = query.split_whitespace().collect();
    fold(&words.join(" "), marks)
}

/// ¿`haystack` contiene la búsqueda `query` (con las reglas del módulo)? Una búsqueda vacía
/// está en todo.
pub fn contains(haystack: &str, query: &str) -> bool {
    let needle = fold_query(query);
    if needle.is_empty() {
        return true;
    }
    let hay = fold(haystack, has_marks(query));
    hay.windows(needle.len()).any(|w| w == needle.as_slice())
}

/// Una posición en el texto de la página: el fragmento y el carácter dentro de él (como
/// `window::select::Point`).
pub type Pos = (usize, usize);

/// Un fragmento de texto de la página: sus caracteres y su line box.
pub struct Run<'a> {
    pub chars: &'a [char],
    pub line: usize,
}

/// Las coincidencias de `query` en `runs`, en orden: desde el primer carácter hasta el final
/// (exclusivo). Entre fragmentos de líneas distintas se cuenta un espacio (el salto de línea
/// suave), así una frase partida en dos líneas se encuentra.
pub fn find_all(runs: &[Run<'_>], query: &str) -> Vec<(Pos, Pos)> {
    let needle = fold_query(query);
    if needle.is_empty() {
        return Vec::new();
    }
    let marks = has_marks(query);
    // El texto plegado y, por cada carácter, de qué carácter original sale (`None` para los
    // espacios agregados entre líneas).
    let mut hay: Vec<char> = Vec::new();
    let mut origin: Vec<Option<Pos>> = Vec::new();
    let mut prev_line = None;
    for (i, run) in runs.iter().enumerate() {
        if prev_line.is_some_and(|l| l != run.line)
            && hay.last().is_some_and(|c| !c.is_whitespace())
        {
            hay.push(' ');
            origin.push(None);
        }
        prev_line = Some(run.line);
        for (j, &c) in run.chars.iter().enumerate() {
            if c.is_whitespace() {
                // Espacios colapsados: la página ya los colapsa, pero el nbsp y los
                // tabuladores de un `pre` cuentan como uno.
                if hay.last() == Some(&' ') {
                    continue;
                }
                hay.push(' ');
            } else {
                fold_char(c, marks, &mut hay);
            }
            origin.resize(hay.len(), Some((i, j)));
        }
    }
    let mut out = Vec::new();
    let n = needle.len();
    let mut k = 0;
    while k + n <= hay.len() {
        if hay[k..k + n] == needle[..] {
            let start = origin[k..k + n].iter().flatten().next();
            let end = origin[k..k + n].iter().rev().flatten().next();
            if let (Some(&s), Some(&(ei, ej))) = (start, end) {
                // La coincidencia no corta un carácter original a medias: termina después de
                // todo lo que sale de su último carácter.
                out.push((s, (ei, ej + 1)));
            }
            k += n;
        } else {
            k += 1;
        }
    }
    out
}

/// El estado de la barra de búsqueda de una pestaña: lo buscado, las coincidencias y cuál es
/// la actual.
#[derive(Debug, Clone, Default)]
pub struct FindState {
    pub query: String,
    pub matches: Vec<(Pos, Pos)>,
    pub current: Option<usize>,
}

impl FindState {
    /// Busca de nuevo `query` en `runs`; la actual pasa a la primera coincidencia en o después
    /// de `from` (la que estaba, para seguir donde iba al escribir).
    pub fn update(&mut self, runs: &[Run<'_>], from: Option<Pos>) {
        self.matches = find_all(runs, &self.query);
        self.current = if self.matches.is_empty() {
            None
        } else {
            let from = from.unwrap_or((0, 0));
            Some(
                self.matches
                    .iter()
                    .position(|(s, _)| *s >= from)
                    .unwrap_or(0),
            )
        };
    }

    /// Pasa a la coincidencia siguiente (`forward`) o anterior, dando la vuelta.
    pub fn step(&mut self, forward: bool) {
        let n = self.matches.len();
        if n == 0 {
            self.current = None;
            return;
        }
        self.current = Some(match self.current {
            None if forward => 0,
            None => n - 1,
            Some(i) if forward => (i + 1) % n,
            Some(i) => (i + n - 1) % n,
        });
    }

    /// "3 de 17", "Sin resultados" o vacío si no se busca nada.
    pub fn counter(&self) -> String {
        if self.query.trim().is_empty() {
            return String::new();
        }
        match self.current {
            Some(i) => format!("{} de {}", i + 1, self.matches.len()),
            None => "Sin resultados".to_owned(),
        }
    }

    /// La coincidencia actual.
    pub fn current_match(&self) -> Option<(Pos, Pos)> {
        self.matches.get(self.current?).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(parts: &[(&str, usize)]) -> (Vec<Vec<char>>, Vec<usize>) {
        (
            parts.iter().map(|(t, _)| t.chars().collect()).collect(),
            parts.iter().map(|(_, l)| *l).collect(),
        )
    }

    fn find(parts: &[(&str, usize)], q: &str) -> Vec<(Pos, Pos)> {
        let (chars, lines) = runs(parts);
        let r: Vec<Run> = chars
            .iter()
            .zip(&lines)
            .map(|(c, &line)| Run {
                chars: c,
                line,
            })
            .collect();
        find_all(&r, q)
    }

    #[test]
    fn case_and_canonical_equivalence() {
        // "Ñandú" precompuesto en la página; la búsqueda descompuesta y en mayúsculas.
        assert_eq!(
            find(&[("El Ñandú corre", 1)], "N\u{303}ANDU\u{301}"),
            vec![((0, 3), (0, 8))]
        );
        // Al revés: la página descompuesta, la búsqueda precompuesta.
        assert_eq!(
            find(&[("n\u{303}andu\u{301}", 1)], "ñandú"),
            vec![((0, 0), (0, 7))]
        );
    }

    #[test]
    fn diacritics_only_when_asked() {
        let page = [("Árbol y arbol", 1)];
        assert_eq!(find(&page, "arbol").len(), 2);
        assert_eq!(find(&page, "árbol"), vec![((0, 0), (0, 5))]);
    }

    #[test]
    fn across_runs_and_lines() {
        // "Navegador web" partido en dos fragmentos de la misma línea y luego en dos líneas.
        let page = [("Un nave", 1), ("gador ", 1), ("web", 2)];
        assert_eq!(find(&page, "navegador web"), vec![((0, 3), (2, 3))]);
        assert_eq!(find(&page, "  NAVEGADOR   WEB "), vec![((0, 3), (2, 3))]);
    }

    #[test]
    fn expanding_lowercase_maps_back() {
        // 'İ' (U+0130) pasa a "i̇" (dos caracteres): la coincidencia cubre el carácter entero.
        assert_eq!(find(&[("xİy", 1)], "i"), vec![((0, 1), (0, 2))]);
    }

    #[test]
    fn state_steps_and_counter() {
        let (chars, _) = runs(&[("a b a b a", 1)]);
        let r = [Run {
            chars: &chars[0],
            line: 1,
        }];
        let mut s = FindState {
            query: "A".into(),
            ..FindState::default()
        };
        s.update(&r, Some((0, 3)));
        assert_eq!(s.counter(), "2 de 3");
        s.step(true);
        s.step(true);
        assert_eq!(s.counter(), "1 de 3");
        s.step(false);
        assert_eq!(s.current_match(), Some(((0, 8), (0, 9))));
        s.query = "zeta".into();
        s.update(&r, None);
        assert_eq!(s.counter(), "Sin resultados");
    }

    #[test]
    fn contains_folds() {
        assert!(contains("Navegador web — Wikipedia", "NAVEGADOR"));
        assert!(contains("Canción", "cancion"));
        assert!(!contains("Cancion", "canción"));
        assert!(contains("x", "  "));
    }
}
