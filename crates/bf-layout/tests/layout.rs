//! Casos de layout: `tests/cases/NNN-nombre.html`, cada uno con un bloque
//! `<!-- expect ... -->` al final con el volcado esperado (viewport 800×600, `TestFont`).
//!
//! Con la variable de entorno `BF_LAYOUT_PRINT=1` se imprime el volcado obtenido de cada caso.

use std::fs;
use std::path::PathBuf;

use bf_css::media::MediaContext;
use bf_css::selectors::MatchContext;
use bf_html::{ParseOptions, parse_document};
use bf_layout::{Size, TestFont, dump, layout_document};
use bf_style::style_document;

fn layout(html: &str) -> String {
    let (doc, _) = parse_document(html, &ParseOptions::default());
    let ctx = MatchContext::for_document(&doc);
    let media = MediaContext {
        width: 800.0,
        height: 600.0,
        ..MediaContext::default()
    };
    let styles = style_document(&doc, &[], &media, &ctx);
    let tree = layout_document(
        &doc,
        &styles,
        Size {
            width: 800.0,
            height: 600.0,
        },
        &TestFont,
    );
    dump(&doc, &tree)
}

#[test]
fn layout_cases() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/cases");
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("tests/cases")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "html"))
        .collect();
    files.sort();
    let print = std::env::var_os("BF_LAYOUT_PRINT").is_some();
    let mut failures = Vec::new();
    let mut count = 0;
    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        // Sin depender de cómo git extrajo el caso (`core.autocrlf`).
        let content = fs::read_to_string(path).unwrap().replace("\r\n", "\n");
        let actual = layout(&content);
        if print {
            println!("===== {name}\n{actual}");
        }
        let Some(start) = content.find("<!-- expect") else {
            failures.push(format!("{name}: falta el bloque <!-- expect -->"));
            continue;
        };
        let expected_block = &content[start + "<!-- expect".len()..];
        let end = expected_block
            .find("-->")
            .expect("cierre del bloque expect");
        let expected = expected_block[..end].trim();
        count += 1;
        if actual.trim() != expected {
            failures.push(format!(
                "FALLO {name}\n--- esperado\n{expected}\n--- obtenido\n{}",
                actual.trim()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} casos fallaron:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    assert!(count >= 40, "solo hay {count} casos");
}
