//! Regresión de la ejecución de test262 (`#[ignore]`, lento): corre el runner sobre
//! `language/` y falla si el porcentaje baja del guardado en `docs/test262-status.md`
//! (sección "Ejecución", línea "referencia de regresión").
//!
//! `cargo test -p bf-js --release --test test262_run -- --ignored --nocapture`

use std::path::PathBuf;
use std::process::Command;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// El número que sigue a `marker` y termina en `%`.
fn percent_after(text: &str, marker: &str) -> Option<f64> {
    let i = text.find(marker)? + marker.len();
    let rest = &text[i..];
    let end = rest.find('%')?;
    rest[..end]
        .trim()
        .trim_start_matches(':')
        .trim()
        .parse()
        .ok()
}

#[test]
#[ignore]
fn test262_language_no_regression() {
    let root = workspace().join("tests/test262");
    assert!(
        root.join("test").exists(),
        "falta tests/test262: correr tools/fetch-test262.sh o tools/fetch-test262.ps1"
    );
    let doc = std::fs::read_to_string(workspace().join("docs/test262-status.md"))
        .expect("docs/test262-status.md");
    let reference = percent_after(&doc, "(referencia de regresión de `tests/test262_run.rs`):")
        .expect("la sección Ejecución no tiene la referencia de regresión");
    let results = std::env::temp_dir().join("bf-js-test262-run.tsv");
    let out = Command::new(env!("CARGO_BIN_EXE_test262"))
        .arg(&root)
        .args(["--filter", "language/", "--results"])
        .arg(&results)
        .output()
        .expect("ejecutar el runner");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let current = percent_after(&stdout, "language/:").expect("salida del runner");
    println!("language/: {current:.2}% (referencia {reference:.2}%)");
    assert!(
        current + 0.005 >= reference,
        "regresión en language/: {current:.2}% < {reference:.2}%"
    );
}
