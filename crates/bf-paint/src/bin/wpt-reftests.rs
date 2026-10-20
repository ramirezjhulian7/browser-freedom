//! Corre los reftests de WPT: `cargo run --release -p bf-paint --bin wpt-reftests -- [filtro]`.
//! Con filtro solo corre las rutas que lo contienen y no actualiza `docs/wpt-status.md`.

#![forbid(unsafe_code)]

fn main() {
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let filter = std::env::args().nth(1);
    let results =
        bf_paint::reftest::run_and_report(&workspace, filter.as_deref(), filter.is_none());
    if filter.is_some() {
        for r in &results {
            println!("{}\t{}\t{}", r.status.name(), r.test, r.detail);
        }
    }
}
