//! Reftests de WPT que necesitan JavaScript (`src/reftest_js.rs`, roadmap 2.3). Ignorado por
//! defecto porque tarda; con la misma forma que `bf-paint/tests/wpt_reftests_groups.rs`:
//!
//! `REFTEST_GROUPS=fase1 cargo test --release -p bf-shell --test wpt_reftests_js -- --ignored --nocapture`
//!
//! `REFTEST_GROUPS` son directorios de `tests/wpt` separados por comas, `fase1` (los 13 de la
//! fase 1) o, sin la variable, todos los de `bf_paint::reftest::GROUPS`. `REFTEST_FILTER`
//! (opcional) restringe a las rutas que lo contienen y `REFTEST_THREADS` fija los hilos (3 por
//! defecto). Imprime la tabla y escribe `target/reftests-js/results.tsv` con las imágenes de
//! los fallos al lado. No falla por los resultados.

#![forbid(unsafe_code)]

use std::process::Command;

#[test]
#[ignore = "lento: correr con --ignored (y REFTEST_GROUPS)"]
fn wpt_reftests_js() {
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let wpt = workspace.join("tests/wpt");
    if !wpt.join("css").is_dir() {
        eprintln!("aviso: falta tests/wpt/css (tools/fetch-wpt); se salta");
        return;
    }
    // El binario hereda `REFTEST_GROUPS`, `REFTEST_FILTER` y `REFTEST_THREADS`.
    let status = Command::new(env!("CARGO_BIN_EXE_freedom"))
        .arg("wpt-reftests-js")
        .arg("--wpt")
        .arg(&wpt)
        .arg("--out")
        .arg(workspace.join("target/reftests-js"))
        .status()
        .expect("se pudo ejecutar freedom");
    assert!(
        status.success(),
        "freedom wpt-reftests-js terminó con {status}"
    );
}
