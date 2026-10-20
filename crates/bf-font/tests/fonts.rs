//! Fuentes reales: escaneo del sistema, matching de genéricas y pesos, fallback por `cmap`,
//! `@font-face` con un archivo local y contornos. Los tests que dependen de fuentes del
//! sistema se saltan (con aviso) si no hay ninguna.

use std::path::PathBuf;
use std::sync::Arc;

use bf_css::stylesheet::{Origin, Rule, Stylesheet};
use bf_font::{FaceStyle, FontDb, Fonts};
use bf_infra::{FileLoader, Url};
use bf_layout::{FontKey, FontProvider, FontRequest};
use bf_style::properties::types::{FamilyName, FontFamily};

fn system() -> Option<Arc<FontDb>> {
    let db = FontDb::system();
    if db.faces().is_empty() {
        eprintln!("aviso: no hay fuentes del sistema; se salta el test");
        return None;
    }
    Some(db)
}

fn request(family: &FontFamily, weight: f32, italic: bool) -> FontRequest<'_> {
    FontRequest {
        family,
        size: 16.0,
        weight,
        italic,
    }
}

fn generic(name: &str) -> FontFamily {
    FontFamily(vec![FamilyName::Generic(name.into())])
}

fn named(name: &str) -> FontFamily {
    FontFamily(vec![FamilyName::Named(name.into())])
}

/// La cara del sistema que dibuja el primer glifo de `text`.
fn face_of(fonts: &Fonts, text: &str, req: &FontRequest<'_>) -> Option<usize> {
    let g = fonts.shape(text, req);
    let k = g.first()?.font.0;
    (k & (1 << 31) == 0 && (k as usize) < fonts.db().faces().len()).then_some(k as usize)
}

#[test]
fn system_database() {
    let Some(db) = system() else { return };
    let some = db
        .faces()
        .iter()
        .position(|f| f.families.iter().any(|n| !n.is_empty()))
        .expect("alguna cara con nombre");
    let face = db.load(some).expect("la cara se carga");
    assert!(face.units_per_em >= 16.0);
    assert!(face.ascender > 0.0);
}

#[test]
fn generic_families() {
    let Some(db) = system() else { return };
    let fonts = Fonts::new(db.clone());
    for g in ["serif", "sans-serif", "monospace"] {
        let family = generic(g);
        let req = request(&family, 400.0, false);
        let Some(i) = face_of(&fonts, "a", &req) else {
            eprintln!("aviso: ninguna fuente para {g}");
            continue;
        };
        let candidates: Vec<String> = bf_font::matching::generic_candidates(g)
            .iter()
            .map(|c| c.to_lowercase())
            .collect();
        let face = &db.faces()[i];
        if candidates.iter().any(|c| !db.family(c).is_empty()) {
            assert!(
                face.families.iter().any(|f| candidates.contains(f)),
                "{g} → {:?}",
                face.families
            );
            assert_eq!(face.style, FaceStyle::Normal);
        }
    }
    // Monoespaciada: todos los caracteres avanzan lo mismo.
    let mono = generic("monospace");
    let g = fonts.shape("iW", &request(&mono, 400.0, false));
    assert!((g[0].advance - g[1].advance).abs() < 0.01);
}

#[test]
fn weight_and_style_matching() {
    let Some(db) = system() else { return };
    let fonts = Fonts::new(db.clone());
    let Some(name) = ["Arial", "DejaVu Sans", "Liberation Sans", "Helvetica"]
        .into_iter()
        .find(|n| db.family(n).len() >= 2)
    else {
        eprintln!("aviso: no hay una familia con variantes; se salta");
        return;
    };
    let family = named(name);
    let regular = face_of(&fonts, "a", &request(&family, 400.0, false)).unwrap();
    let bold = face_of(&fonts, "a", &request(&family, 700.0, false)).unwrap();
    assert_eq!(db.faces()[regular].weight, 400.0);
    assert!(db.faces()[bold].weight >= 600.0);
    let italic = face_of(&fonts, "a", &request(&family, 400.0, true)).unwrap();
    assert_ne!(db.faces()[italic].style, FaceStyle::Normal);
}

/// Directorio con Ahem: el de WPT si está descargado, o una fuente del sistema copiada.
fn ahem_fixture() -> Option<(PathBuf, String)> {
    let dir = std::env::temp_dir().join(format!("bf-font-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let wpt = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/wpt/fonts/Ahem.ttf");
    let (src, family) = if wpt.is_file() {
        (wpt, "Ahem".to_owned())
    } else {
        let db = FontDb::system();
        let f = db.faces().iter().find(|f| {
            f.path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("ttf"))
        })?;
        (f.path.clone(), "Copia".to_owned())
    };
    std::fs::copy(&src, dir.join("font.ttf")).ok()?;
    Some((dir, family))
}

fn font_face_fonts(css: &str, db: Arc<FontDb>) -> Fonts {
    let sheet = Stylesheet::parse(css, Origin::Author);
    let rules: Vec<_> = sheet
        .rules
        .iter()
        .filter_map(|r| match r {
            Rule::FontFace(d) => Some(d.as_slice()),
            _ => None,
        })
        .collect();
    let mut fonts = Fonts::new(db);
    fonts.add_font_faces(&rules, Arc::new(FileLoader::default()));
    fonts
}

#[test]
fn font_face_local_file() {
    let Some((dir, family)) = ahem_fixture() else {
        eprintln!("aviso: sin fuente para el test de @font-face");
        return;
    };
    let url = Url::from_file_path(&dir.join("font.ttf")).unwrap().href();
    let css =
        format!("@font-face {{ font-family: Test; src: url(\"{url}\") format(\"truetype\"); }}");
    let fonts = font_face_fonts(&css, Arc::new(FontDb::default()));
    let f = named("Test");
    let req = request(&f, 400.0, false);
    let glyphs = fonts.shape("XpX", &req);
    assert_eq!(glyphs.len(), 3);
    assert!(
        glyphs.iter().all(|g| g.font.0 & (1 << 31) != 0),
        "{glyphs:?}"
    );
    let outline = fonts
        .outline(glyphs[0].font, glyphs[0].id)
        .expect("contorno de X");
    assert!(!outline.commands.is_empty());
    if family == "Ahem" {
        // Ahem: cada glifo es un cuadrado de 1em; ascent 0.8em, descent 0.2em. Las métricas
        // verticales se redondean a px enteros, como en los navegadores (12.8 → 13, 3.2 → 3).
        let m = fonts.metrics(&req);
        assert_eq!((m.ascent, m.descent, m.line_gap), (13.0, 3.0, 0.0), "{m:?}");
        assert!((m.x_height - 12.8).abs() < 0.01, "{m:?}");
        assert!(glyphs.iter().all(|g| (g.advance - 16.0).abs() < 0.01));
    }
    // Una familia @font-face que no carga cae en las siguientes de la lista.
    let fonts = font_face_fonts(
        "@font-face { font-family: Rota; src: url(no-existe.ttf) }",
        Arc::new(FontDb::default()),
    );
    let f = named("Rota");
    assert_eq!(
        fonts.shape("a", &request(&f, 400.0, false))[0].font,
        FontKey(u32::MAX)
    );
}

#[test]
fn unicode_range_and_cmap_fallback() {
    let Some((dir, family)) = ahem_fixture() else {
        return;
    };
    if family != "Ahem" {
        return;
    }
    let url = Url::from_file_path(&dir.join("font.ttf")).unwrap().href();
    let css = format!(
        "@font-face {{ font-family: Test; src: url(\"{url}\"); unicode-range: U+0041-005A; }}"
    );
    let Some(db) = system() else { return };
    let fonts = font_face_fonts(&css, db);
    let f = named("Test");
    let req = request(&f, 400.0, false);
    // Mayúsculas con Ahem; minúsculas (fuera del unicode-range) y un ideograma con fallback.
    let glyphs = fonts.shape("Aa中", &req);
    let web = |k: FontKey| k.0 & (1 << 31) != 0 && k.0 != u32::MAX;
    assert!(web(glyphs[0].font));
    assert!(!web(glyphs[1].font));
    let last = glyphs.last().unwrap();
    if last.font != FontKey(u32::MAX) {
        assert!(!web(last.font));
        assert_ne!(last.id, 0, "el fallback encontró un glifo real");
    }
}
