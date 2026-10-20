//! Corre las pruebas de tree construction de WPT (`html/syntax/parsing/resources/*.dat`).
//!
//! Formato: `tests/wpt/html/syntax/parsing/resources/README.md`. Las pruebas viven en WPT desde
//! html5lib-tests `224991e`; `tools/fetch-wpt` descarga el commit pineado en
//! `tools/wpt-commit.txt`.
//!
//! En este paso no se comparan los errores de parseo (`#errors` / `#new-errors`). Cada
//! ejecución (test × modo de scripting) cuenta por separado.

use std::fs;
use std::path::PathBuf;

use bf_dom::test_serialize;
use bf_html::{FragmentContext, ParseOptions, parse_document, parse_fragment};
use bf_infra::Namespace;

const SECTIONS: &[&str] = &[
    "#description",
    "#data",
    "#errors",
    "#new-errors",
    "#document-fragment",
    "#script-off",
    "#script-on",
    "#document",
];

#[derive(Default)]
struct DatTest {
    line: usize,
    data: Vec<String>,
    fragment: Option<String>,
    scripting: Option<bool>,
    document: Vec<String>,
    has_document: bool,
}

/// Divide un `.dat` en tests. Un test nuevo empieza con `#data` (o `#description`) cuando el
/// anterior ya tiene su `#document`.
fn parse_dat(content: &str) -> Vec<DatTest> {
    let mut tests: Vec<DatTest> = Vec::new();
    let mut section = "";
    for (n, line) in content.split('\n').enumerate() {
        let starts_test =
            matches!(line, "#data" | "#description") && tests.last().is_none_or(|t| t.has_document);
        if starts_test {
            tests.push(DatTest {
                line: n + 1,
                ..DatTest::default()
            });
        }
        let Some(test) = tests.last_mut() else {
            continue;
        };
        // Dentro del #document solo `#data`/`#description` (ya manejados arriba) abren algo
        // nuevo; cualquier otra línea es parte del árbol esperado.
        if SECTIONS.contains(&line) && (starts_test || section != "#document") {
            section = line;
            match line {
                "#script-on" => test.scripting = Some(true),
                "#script-off" => test.scripting = Some(false),
                "#document" => test.has_document = true,
                _ => {}
            }
            continue;
        }
        match section {
            "#data" => test.data.push(line.to_owned()),
            "#document-fragment" if test.fragment.is_none() => {
                test.fragment = Some(line.to_owned());
            }
            "#document" => test.document.push(line.to_owned()),
            _ => {}
        }
    }
    for test in &mut tests {
        // Las líneas en blanco al final del #document son el separador entre tests.
        while test.document.last().is_some_and(String::is_empty) {
            test.document.pop();
        }
    }
    tests
}

fn parse_context(s: &str) -> FragmentContext<'_> {
    if let Some(name) = s.strip_prefix("svg ") {
        FragmentContext {
            ns: Namespace::Svg,
            name,
        }
    } else if let Some(name) = s.strip_prefix("math ") {
        FragmentContext {
            ns: Namespace::MathMl,
            name,
        }
    } else {
        FragmentContext {
            ns: Namespace::Html,
            name: s,
        }
    }
}

fn run(test: &DatTest, scripting: bool) -> String {
    // "#data ... except with the final newline (on the last line) removed".
    let input = test.data.join("\n");
    let opts = ParseOptions {
        scripting,
        iframe_srcdoc: false,
    };
    match &test.fragment {
        Some(context) => {
            let (doc, _errors) = parse_fragment(&input, parse_context(context), &opts);
            let html = doc
                .children(doc.root())
                .next()
                .expect("el parseo de fragmento crea el elemento html");
            test_serialize(&doc, html)
        }
        None => {
            let (doc, _errors) = parse_document(&input, &opts);
            test_serialize(&doc, doc.root())
        }
    }
}

#[derive(Default)]
struct Stats {
    passed: usize,
    /// Tests de `scripted_*.dat`: necesitan ejecutar JS (`document.write`), hasta bf-js.
    skipped: usize,
    failed: usize,
}

#[test]
fn wpt_tree_construction() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/wpt/html/syntax/parsing/resources");
    assert!(
        dir.is_dir(),
        "no existe {}: ejecuta tools/fetch-wpt.ps1 (Windows) o tools/fetch-wpt.sh",
        dir.display()
    );
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "dat"))
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "no hay archivos .dat en {}",
        dir.display()
    );

    let mut total = Stats::default();
    let mut failures = Vec::new();
    println!(
        "{:<45} {:>7} {:>8} {:>7}",
        "archivo", "passed", "skipped", "failed"
    );
    for path in &files {
        let file_name = path.file_name().unwrap().to_str().unwrap();
        let content = fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("{file_name}: no es UTF-8 válido: {e}"));
        let mut stats = Stats::default();
        let needs_js = file_name.starts_with("scripted_");
        for test in parse_dat(&content) {
            // Sin flag, el test corre en ambos modos (README de la suite).
            let modes: &[bool] = match test.scripting {
                Some(true) => &[true],
                Some(false) => &[false],
                None => &[false, true],
            };
            for &scripting in modes {
                if needs_js {
                    stats.skipped += 1;
                    continue;
                }
                let expected = test.document.join("\n");
                let actual = run(&test, scripting);
                if actual == expected {
                    stats.passed += 1;
                } else {
                    stats.failed += 1;
                    failures.push(format!(
                        "FALLO {file_name}:{} (fragmento: {:?}, scripting: {scripting})\n--- input\n{}\n--- esperado\n{expected}\n--- obtenido\n{actual}",
                        test.line,
                        test.fragment,
                        test.data.join("\n"),
                    ));
                }
            }
        }
        println!(
            "{file_name:<45} {:>7} {:>8} {:>7}",
            stats.passed, stats.skipped, stats.failed
        );
        total.passed += stats.passed;
        total.skipped += stats.skipped;
        total.failed += stats.failed;
    }
    println!(
        "{:<45} {:>7} {:>8} {:>7}",
        "TOTAL", total.passed, total.skipped, total.failed
    );
    for failure in failures.iter().take(40) {
        println!("\n{failure}");
    }
    assert_eq!(
        total.failed, 0,
        "{} tests de tree construction fallaron",
        total.failed
    );
}
