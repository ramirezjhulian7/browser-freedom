//! Reftests de WPT (ver `bf_paint::reftest`). Ignorado por defecto porque tarda:
//! `cargo test -p bf-paint --test wpt_reftests -- --ignored --nocapture`. No falla por los
//! resultados: escribe `docs/wpt-status.md` y deja los fallos en `target/reftests/`.

#[test]
#[ignore = "lento: correr con --ignored"]
fn wpt_reftests() {
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if !workspace.join("tests/wpt/css").is_dir() {
        eprintln!("aviso: falta tests/wpt/css (tools/fetch-wpt); se salta");
        return;
    }
    let results = bf_paint::reftest::run_and_report(&workspace, None, true);
    assert!(!results.is_empty());
}
