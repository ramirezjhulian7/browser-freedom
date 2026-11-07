//! Buscar en la página (`src/find.rs`): equivalencia canónica (UAX #15), mayúsculas,
//! diacríticos según la búsqueda y posiciones de vuelta a los fragmentos de la página.

#![forbid(unsafe_code)]
#![allow(dead_code)]

#[path = "../src/find.rs"]
mod find;

use find::{FindState, Run, contains, find_all};

fn chars(s: &str) -> Vec<char> {
    s.chars().collect()
}

#[test]
fn accents_follow_the_query() {
    // Sin diacríticos en la búsqueda, no se distinguen.
    assert!(contains("El árbol de la canción", "arbol"));
    assert!(contains("El árbol de la canción", "CANCION"));
    assert!(contains("ÁRBOL", "arbol"));
    // Con diacríticos, se respetan.
    assert!(contains("El árbol", "árbol"));
    assert!(contains("EL ÁRBOL", "árbol"));
    assert!(!contains("El arbol", "árbol"));
    assert!(!contains("cancion", "canción"));
    // La ñ es n + tilde en NFD: "nandu" la encuentra, "ñandú" no encuentra "nandu".
    assert!(contains("Ñandú", "nandu"));
    assert!(contains("ñandú", "ÑANDÚ"));
    assert!(!contains("nandu", "ñandú"));
    // Búsqueda vacía o solo espacios: está en todo.
    assert!(contains("x", ""));
    assert!(contains("x", "   "));
}

#[test]
fn canonical_equivalence() {
    // NFC en la página, NFD en la búsqueda y al revés.
    assert!(contains("Ñandú", "n\u{303}andu\u{301}"));
    assert!(contains("n\u{303}andu\u{301}", "Ñandú"));
    assert!(!contains("n\u{303}andu", "ñandú"));
    // Espacios repetidos en la búsqueda cuentan como uno.
    assert!(contains("la canción", "la    canción"));
}

#[test]
fn positions_map_back() {
    let a = chars("Árbol, árbol y ARBOL");
    let runs = [Run {
        chars: &a,
        line: 0,
    }];
    let m = find_all(&runs, "arbol");
    assert_eq!(m, vec![((0, 0), (0, 5)), ((0, 7), (0, 12)), ((0, 15), (0, 20))]);
    let m = find_all(&runs, "árbol");
    assert_eq!(m, vec![((0, 0), (0, 5)), ((0, 7), (0, 12))]);
    // Un carácter descompuesto de la página (n + U+0303) entra entero en la coincidencia.
    let b = chars("pin\u{303}a");
    let runs = [Run {
        chars: &b,
        line: 0,
    }];
    assert_eq!(find_all(&runs, "piña"), vec![((0, 0), (0, 5))]);
    assert_eq!(find_all(&runs, "pina"), vec![((0, 0), (0, 5))]);
}

#[test]
fn across_fragments_and_lines() {
    let a = chars("la can");
    let b = chars("ción");
    let c = chars("de cuna");
    // `a` y `b` en la misma línea (un `<b>` en medio de la palabra); `c` en la siguiente.
    let runs = [
        Run {
            chars: &a,
            line: 0,
        },
        Run {
            chars: &b,
            line: 0,
        },
        Run {
            chars: &c,
            line: 1,
        },
    ];
    assert_eq!(find_all(&runs, "cancion"), vec![((0, 3), (1, 4))]);
    // El salto de línea suave cuenta como un espacio.
    assert_eq!(find_all(&runs, "canción de"), vec![((0, 3), (2, 2))]);
    assert!(find_all(&runs, "canciónde").is_empty());
}

#[test]
fn state_counter_and_steps() {
    let a = chars("uno dos uno dos uno");
    let runs = [Run {
        chars: &a,
        line: 0,
    }];
    let mut s = FindState {
        query: "uno".to_owned(),
        ..FindState::default()
    };
    s.update(&runs, None);
    assert_eq!(s.matches.len(), 3);
    assert_eq!(s.current_match(), Some(((0, 0), (0, 3))));
    s.step(true);
    assert_eq!(s.current_match(), Some(((0, 8), (0, 11))));
    s.step(true);
    s.step(true);
    // Da la vuelta.
    assert_eq!(s.current_match(), Some(((0, 0), (0, 3))));
    s.step(false);
    assert_eq!(s.current_match(), Some(((0, 16), (0, 19))));
    // Seguir desde una posición: la primera coincidencia en o después de ella.
    s.update(&runs, Some((0, 5)));
    assert_eq!(s.current_match(), Some(((0, 8), (0, 11))));
    s.query = "nada".to_owned();
    s.update(&runs, None);
    assert!(s.matches.is_empty());
    assert_eq!(s.current_match(), None);
}
