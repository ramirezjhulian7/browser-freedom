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
    fonts.system_face(g.first()?.font)
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
/// `test` da un directorio propio a cada test: los tests corren en paralelo y copiar
/// `font.ttf` al mismo directorio mientras otro lo lee falla de forma intermitente.
fn ahem_fixture(test: &str) -> Option<(PathBuf, String)> {
    let dir = std::env::temp_dir().join(format!("bf-font-{}-{test}", std::process::id()));
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
    let Some((dir, family)) = ahem_fixture("local-file") else {
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
    let Some((dir, family)) = ahem_fixture("unicode-range") else {
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

/// Los clústeres de `shape` indexan el texto original en bytes UTF-8, también cuando hay
/// caracteres de control multibyte (U+0085, U+009D) que el shaper sustituye por espacios.
#[test]
fn clusters_are_char_boundaries() {
    let Some(db) = system() else { return };
    let fonts = Fonts::new(db);
    let family = generic("sans-serif");
    let req = request(&family, 400.0, false);
    let text = "x\u{9d}y é\u{85}z 日本";
    let glyphs = fonts.shape(text, &req);
    assert!(!glyphs.is_empty());
    for g in &glyphs {
        assert!(
            text.is_char_boundary(g.cluster),
            "cluster {} no es límite de carácter en {text:?}",
            g.cluster
        );
    }
}

/// E9: `shape_dir` da los glifos de un run RTL en orden visual (clústeres de mayor a menor) y
/// cada clúster es un límite de carácter, también con controles que ocupan 2 bytes.
#[test]
fn shape_dir_rtl_runs() {
    let Some(db) = system() else { return };
    let fonts = Fonts::new(db);
    let family = generic("sans-serif");
    let req = request(&family, 400.0, false);
    for text in [
        "שלום עולם",
        "مرحبا بالعالم",
        "ab\u{85}cd",
        "אב\u{9d}גד",
        "مرحبا שלום",
    ] {
        for rtl in [false, true] {
            let glyphs = fonts.shape_dir(text, &req, rtl);
            assert!(!glyphs.is_empty(), "{text:?}");
            assert!(
                glyphs.iter().all(|g| text.is_char_boundary(g.cluster)),
                "{text:?} rtl={rtl}: {glyphs:?}"
            );
            let ordered = glyphs.windows(2).all(|w| {
                if rtl {
                    w[0].cluster >= w[1].cluster
                } else {
                    w[0].cluster <= w[1].cluster
                }
            });
            assert!(ordered, "{text:?} rtl={rtl}: {glyphs:?}");
        }
    }
    // Las dos direcciones ocupan lo mismo y no se mezclan en la caché.
    let width = |g: &[bf_layout::Glyph]| g.iter().map(|g| g.advance).sum::<f32>();
    let ltr = fonts.shape_dir("שלום", &req, false);
    let rtl = fonts.shape_dir("שלום", &req, true);
    assert!((width(&ltr) - width(&rtl)).abs() < 0.01);
    assert_ne!(
        ltr.first().map(|g| g.cluster),
        rtl.first().map(|g| g.cluster)
    );
}

/// En macOS, las genéricas resuelven a las fuentes de Apple que traen solo nombres de la
/// plataforma Macintosh (Helvetica, Menlo, Times, Courier, Apple Color Emoji).
#[cfg(target_os = "macos")]
#[test]
fn macos_system_families() {
    let Some(db) = system() else { return };
    for family in [
        "Helvetica",
        "Times",
        "Menlo",
        "Courier",
        "Apple Color Emoji",
    ] {
        assert!(!db.family(family).is_empty(), "falta la familia {family}");
    }
    for generic in [
        "serif",
        "sans-serif",
        "monospace",
        "system-ui",
        "ui-monospace",
    ] {
        let first = bf_font::matching::generic_candidates(generic)
            .iter()
            .find(|f| !db.family(f).is_empty());
        assert!(first.is_some(), "ninguna familia para {generic}");
    }
}

/// E5: `system-ui` en macOS es `.SF NS` (`SFNS.ttf`, variable con 54 tuplas de `gvar` por
/// glifo: sin la característica `gvar-alloc` de `ttf-parser` no daba contornos), con un peso por
/// instancia del eje `wght`: la negrita tiene otro contorno.
#[cfg(target_os = "macos")]
#[test]
fn macos_system_ui_is_sf() {
    let Some(db) = system() else { return };
    if db.family(".SF NS").is_empty() {
        eprintln!("aviso: no está .SF NS; se salta el test");
        return;
    }
    let fonts = Fonts::new(db.clone());
    let family = generic("system-ui");
    let regular = face_of(&fonts, "A", &request(&family, 400.0, false)).expect("cara regular");
    let bold = face_of(&fonts, "A", &request(&family, 700.0, false)).expect("cara negrita");
    assert!(db.faces()[regular].families.iter().any(|f| f == ".sf ns"));
    assert_eq!(db.faces()[regular].wght, Some(400.0));
    assert_eq!(db.faces()[bold].wght, Some(700.0));
    let glyph = |req: &FontRequest<'_>| fonts.shape("A", req)[0];
    let (gr, gb) = (
        glyph(&request(&family, 400.0, false)),
        glyph(&request(&family, 700.0, false)),
    );
    let (or, ob) = (
        fonts.outline(gr.font, gr.id).expect("contorno regular"),
        fonts.outline(gb.font, gb.id).expect("contorno negrita"),
    );
    assert!(!or.commands.is_empty());
    assert_ne!(or.commands, ob.commands, "la negrita usa el eje wght");
    // HVAR: la negrita avanza más.
    assert!(gb.advance > gr.advance);
}

/// E5: Apple Color Emoji solo tiene mapas de bits (`sbix`); las secuencias (tono de piel,
/// bandera, ZWJ, keycap) quedan en un solo glifo de esa cara.
#[cfg(target_os = "macos")]
#[test]
fn macos_color_emoji() {
    let Some(db) = system() else { return };
    if db.family("Apple Color Emoji").is_empty() {
        eprintln!("aviso: no está Apple Color Emoji; se salta el test");
        return;
    }
    let fonts = Fonts::new(db.clone());
    let family = generic("sans-serif");
    let req = request(&family, 400.0, false);
    for text in [
        "😀",
        "👍🏽",
        "🇪🇸",
        "👨\u{200D}👩\u{200D}👧",
        "1\u{FE0F}\u{20E3}",
    ] {
        let glyphs = fonts.shape(text, &req);
        assert_eq!(glyphs.len(), 1, "{text:?}: {glyphs:?}");
        let g = glyphs[0];
        assert!(
            db.faces()[fonts.system_face(g.font).expect("cara del sistema")]
                .families
                .iter()
                .any(|f| f == "apple color emoji"),
            "{text:?}"
        );
        let o = fonts.outline(g.font, g.id).expect("glifo de emoji");
        assert!(!o.bitmaps.is_empty(), "{text:?} sin mapas de bits");
        assert!(o.bitmaps.windows(2).all(|w| w[0].ppem < w[1].ppem));
        assert!(o.bitmaps.iter().all(|b| b.png.starts_with(b"\x89PNG")));
        assert!(g.advance > 10.0, "{text:?}: avance {}", g.advance);
    }
}

/// E5: el fallback por carácter elige, dentro de la familia, la cara del peso pedido (CSS
/// Fonts 4 §5.2): un texto chino en negrita no mezcla Hiragino W3 con W7.
#[cfg(target_os = "macos")]
#[test]
fn macos_cjk_single_weight() {
    let Some(db) = system() else { return };
    if db.family("Hiragino Sans").len() < 2 {
        eprintln!("aviso: no está Hiragino Sans con varios pesos; se salta el test");
        return;
    }
    let fonts = Fonts::new(db.clone());
    let family = generic("sans-serif");
    let text = "我们在使用简体中文测试字体的粗细";
    for (weight, range) in [(700.0, 600.0..=700.0), (400.0, 300.0..=400.0)] {
        for g in fonts.shape(text, &request(&family, weight, false)) {
            let face = &db.faces()[fonts.system_face(g.font).expect("cara del sistema")];
            assert!(
                face.families.iter().any(|f| f.starts_with("hiragino")),
                "{weight}: {:?}",
                face.families.first()
            );
            assert!(
                range.contains(&face.weight),
                "{weight}: cara de peso {}",
                face.weight
            );
        }
    }
}

/// E5: `-apple-system` y `BlinkMacSystemFont` resuelven como `system-ui` en macOS, y las
/// genéricas `ui-*` a las fuentes de Apple con su nombre real.
#[cfg(target_os = "macos")]
#[test]
fn macos_system_font_aliases() {
    let Some(db) = system() else { return };
    if db.family(".SF NS").is_empty() {
        eprintln!("aviso: no está .SF NS; se salta el test");
        return;
    }
    let fonts = Fonts::new(db.clone());
    let family_of = |f: &FontFamily| {
        let i = face_of(&fonts, "a", &request(f, 400.0, false)).expect("cara");
        db.faces()[i].families[0].clone()
    };
    for name in ["-apple-system", "BlinkMacSystemFont"] {
        let list = FontFamily(vec![
            FamilyName::Named(name.into()),
            FamilyName::Generic("serif".into()),
        ]);
        assert_eq!(family_of(&list), ".sf ns", "{name}");
    }
    assert_eq!(family_of(&generic("ui-sans-serif")), ".sf ns");
    if !db.family(".SF NS Rounded").is_empty() {
        assert_eq!(family_of(&generic("ui-rounded")), ".sf ns rounded");
    }
}

/// E5: `font-optical-sizing: auto` (CSS Fonts 4 §7.4): SF a 12 px y a 72 px usa distinto
/// `opsz`, así que el mismo glifo tiene otro contorno (escalado a la misma em).
#[cfg(target_os = "macos")]
#[test]
fn macos_optical_size() {
    let Some(db) = system() else { return };
    if db.family(".SF NS").is_empty() {
        eprintln!("aviso: no está .SF NS; se salta el test");
        return;
    }
    let fonts = Fonts::new(db);
    let family = generic("system-ui");
    let at = |size: f32| {
        let req = FontRequest {
            family: &family,
            size,
            weight: 400.0,
            italic: false,
        };
        let g = fonts.shape("a", &req)[0];
        fonts
            .outline(g.font, g.id)
            .expect("contorno")
            .commands
            .clone()
    };
    assert_ne!(at(12.0), at(72.0));
    // Dentro del mismo tamaño, la misma clave y el mismo contorno.
    assert_eq!(at(12.0), at(12.0));
}

/// E5: un tramo chino sin `lang` sale de una sola familia cuando una la cubre entera.
#[cfg(target_os = "macos")]
#[test]
fn macos_cjk_single_family() {
    let Some(db) = system() else { return };
    if db.family("Hiragino Sans GB").is_empty() {
        eprintln!("aviso: no está Hiragino Sans GB; se salta el test");
        return;
    }
    let fonts = Fonts::new(db.clone());
    let family = generic("sans-serif");
    let glyphs = fonts.shape("我们在使用简体中文测试", &request(&family, 400.0, false));
    let families: std::collections::BTreeSet<String> = glyphs
        .iter()
        .map(|g| db.faces()[fonts.system_face(g.font).expect("cara")].families[0].clone())
        .collect();
    assert_eq!(families.len(), 1, "{families:?}");
}
