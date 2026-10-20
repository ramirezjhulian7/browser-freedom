//! Hojas externas: `<link rel="stylesheet">`, `@import` (con `media`, `layer` y `supports`),
//! resolución de URLs relativas a cada hoja y recolección de `@font-face`.

use std::path::PathBuf;

use bf_css::media::MediaContext;
use bf_css::selectors::{MatchContext, NamespaceMap, matches, parse_selector_list};
use bf_html::{ParseOptions, parse_document};
use bf_infra::encoding::Encoding;
use bf_infra::{FileLoader, Url};
use bf_style::{Resources, document_stylesheets, font_face_rules, style_document_with_sheets};

/// Directorio temporal propio del test con los archivos dados.
fn fixture(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bf-style-{name}-{}", std::process::id()));
    for (path, content) in files {
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }
    dir
}

fn computed(dir: &std::path::Path, selector: &str, property: &str) -> String {
    let path = dir.join("index.html");
    let html = std::fs::read_to_string(&path).unwrap();
    let (doc, _) = parse_document(&html, &ParseOptions::default());
    let loader = FileLoader::new();
    let base = Url::from_file_path(&path).unwrap();
    let media = MediaContext::default();
    let sheets = document_stylesheets(
        &doc,
        &media,
        Some(Resources {
            loader: &loader,
            base: &base,
            encoding: Encoding::Utf8,
        }),
    );
    let ctx = MatchContext::for_document(&doc);
    let map = style_document_with_sheets(&doc, &sheets, &[], &media, &ctx, None);
    let list = parse_selector_list(selector, &NamespaceMap::default()).unwrap();
    let node = doc
        .inclusive_descendants(doc.root())
        .into_iter()
        .find(|&n| list.0.iter().any(|s| matches(s, &doc, n, &ctx)))
        .unwrap();
    map[&(node, None)].get_css(property).unwrap()
}

#[test]
fn link_and_import() {
    let dir = fixture(
        "link",
        &[
            (
                "index.html",
                r#"<!DOCTYPE html>
<link rel="stylesheet" href="css/main.css">
<link rel="alternate stylesheet" href="css/alt.css">
<link rel="stylesheet" href="css/print.css" media="print">
<style>@import "css/layered.css" layer(base); #c { color: green }</style>
<p id=a>a</p><p id=b>b</p><p id=c>c</p><p id=d>d</p>"#,
            ),
            (
                "css/main.css",
                "\u{feff}@import url(sub/inner.css);\n@import 'sub/screen.css' screen and (min-width: 10000px);\n#a { color: red }",
            ),
            (
                "css/sub/inner.css",
                "#a { margin-top: 3px; background-image: url(../img/x.png) }",
            ),
            ("css/sub/screen.css", "#a { margin-top: 99px }"),
            ("css/alt.css", "#b { color: red }"),
            ("css/print.css", "#b { color: red }"),
            // Capa `base`: las declaraciones normales sin capa le ganan.
            ("css/layered.css", "#c { color: red } #d { color: blue }"),
        ],
    );
    assert_eq!(computed(&dir, "#a", "color"), "rgb(255, 0, 0)");
    assert_eq!(computed(&dir, "#a", "margin-top"), "3px");
    let image = computed(&dir, "#a", "background-image");
    assert!(
        image.starts_with("url(\"file:///") && image.ends_with("css/img/x.png\")"),
        "URL resuelta contra la hoja importada: {image}"
    );
    assert_eq!(computed(&dir, "#b", "color"), "rgb(0, 0, 0)");
    assert_eq!(computed(&dir, "#c", "color"), "rgb(0, 128, 0)");
    assert_eq!(computed(&dir, "#d", "color"), "rgb(0, 0, 255)");
}

#[test]
fn import_cycles_and_supports() {
    let dir = fixture(
        "cycle",
        &[
            (
                "index.html",
                r#"<!DOCTYPE html><link rel=stylesheet href=a.css><p id=a>a</p><p id=b>b</p>"#,
            ),
            (
                "a.css",
                "@import 'b.css'; @import 'c.css' supports(display: nope); #a { margin-top: 1px }",
            ),
            ("b.css", "@import 'a.css'; #b { margin-top: 2px }"),
            ("c.css", "#b { margin-top: 50px }"),
        ],
    );
    assert_eq!(computed(&dir, "#a", "margin-top"), "1px");
    assert_eq!(computed(&dir, "#b", "margin-top"), "2px");
}

#[test]
fn font_faces() {
    let dir = fixture(
        "fontface",
        &[
            (
                "index.html",
                "<!DOCTYPE html><link rel=stylesheet href=f/fonts.css>",
            ),
            (
                "f/fonts.css",
                "@font-face { font-family: A; src: url(a.ttf) } @media print { @font-face { font-family: B; src: url(b.ttf) } }",
            ),
        ],
    );
    let html = std::fs::read_to_string(dir.join("index.html")).unwrap();
    let (doc, _) = parse_document(&html, &ParseOptions::default());
    let loader = FileLoader::new();
    let base = Url::from_file_path(&dir.join("index.html")).unwrap();
    let media = MediaContext::default();
    let sheets = document_stylesheets(
        &doc,
        &media,
        Some(Resources {
            loader: &loader,
            base: &base,
            encoding: Encoding::Utf8,
        }),
    );
    let refs: Vec<_> = sheets.iter().collect();
    let faces = font_face_rules(&refs, &media);
    assert_eq!(faces.len(), 1);
    let src = faces[0].iter().find(|d| d.name == "src").unwrap();
    let text = bf_css::stylesheet::component_values_to_string(&src.value);
    assert!(
        text.contains("file:///") && text.contains("/f/a.ttf"),
        "{text}"
    );
}

#[test]
fn charset_rule() {
    // `@charset "windows-1252";` decodifica 0xE9 como é; sin él, el entorno (UTF-8) lo
    // reemplaza.
    let dir = std::env::temp_dir().join(format!("bf-style-charset-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("index.html"),
        "<!DOCTYPE html><link rel=stylesheet href=a.css><p id=a>a</p>",
    )
    .unwrap();
    std::fs::write(
        dir.join("a.css"),
        b"@charset \"windows-1252\";\n#a { font-family: \"caf\xE9\" }",
    )
    .unwrap();
    assert_eq!(computed(&dir, "#a", "font-family"), "\"café\"");
}
