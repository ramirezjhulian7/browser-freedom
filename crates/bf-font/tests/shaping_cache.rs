//! E5: las caras y los planes de rustybuzz guardados (solo para la base del sistema) dan los
//! mismos glifos, bit a bit, que prepararlos en cada run (el camino de una base que no es la
//! del sistema).

use std::sync::Arc;

use bf_font::db::system_font_dirs;
use bf_font::{FontDb, Fonts};
use bf_layout::{FontProvider, FontRequest};
use bf_style::properties::types::{FamilyName, FontFamily};

const TEXTS: &[&str] = &[
    "Navegador web: AVATAR, Te, To, fi, ffl, office; «Wikipedia», 1/2 — 3,14 %",
    "The quick brown fox jumps over the lazy dog.",
    "مرحبا بالعالم، هذا نص عربي",
    "שלום עולם, זה טקסט בעברית",
    "中文 日本語の文章 한국어 텍스트",
    "हिन्दी पाठ, संयुक्ताक्षर क्ष त्र",
    "Ελληνικά, Русский текст, Հայերեն",
    "emoji 😀👍🏽 e\u{301} n\u{303}",
    "\u{A0}espacio\u{A0}duro\ttab",
];

#[test]
fn cached_faces_shape_the_same() {
    let cached = Fonts::system();
    let fresh = Fonts::new(Arc::new(FontDb::scan(&system_font_dirs())));
    let families = [
        FontFamily(vec![FamilyName::Generic("sans-serif".into())]),
        FontFamily(vec![FamilyName::Generic("serif".into())]),
        FontFamily(vec![FamilyName::Generic("monospace".into())]),
        FontFamily(vec![FamilyName::Named("Helvetica".into())]),
    ];
    let mut glyphs = 0;
    for family in &families {
        for (size, weight, italic) in [(13.0, 400.0, false), (16.7, 700.0, true)] {
            let request = FontRequest {
                family,
                size,
                weight,
                italic,
            };
            for text in TEXTS {
                for rtl in [None, Some(false), Some(true)] {
                    let (a, b) = match rtl {
                        None => (cached.shape(text, &request), fresh.shape(text, &request)),
                        Some(rtl) => (
                            cached.shape_dir(text, &request, rtl),
                            fresh.shape_dir(text, &request, rtl),
                        ),
                    };
                    assert_eq!(
                        format!("{a:?}"),
                        format!("{b:?}"),
                        "{text:?} {rtl:?} {family:?}"
                    );
                    glyphs += a.len();
                }
            }
        }
    }
    assert!(glyphs > 1000);
}
