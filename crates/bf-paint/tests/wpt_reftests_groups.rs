//! E9: reftests de WPT de unos directorios concretos, para medir mientras se desarrolla sin
//! correr todos los grupos. Ignorado por defecto:
//!
//! `REFTEST_GROUPS=css/CSS2/bidi-text,css/css-text cargo test -p bf-paint --test wpt_reftests_groups -- --ignored --nocapture`
//!
//! `REFTEST_FILTER` (opcional) restringe a las rutas que lo contienen. Imprime la tabla y
//! escribe `target/reftests-groups/results.tsv` (con las imágenes de los fallos al lado). No
//! toca `docs/wpt-status.md`.

#[test]
#[ignore = "lento: correr con --ignored y REFTEST_GROUPS"]
fn wpt_reftests_groups() {
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let Ok(groups) = std::env::var("REFTEST_GROUPS") else {
        eprintln!("aviso: falta REFTEST_GROUPS; se salta");
        return;
    };
    let groups: Vec<&str> = groups.split(',').map(str::trim).collect();
    let filter = std::env::var("REFTEST_FILTER").ok();
    let out = workspace.join("target/reftests-groups");
    let _ = std::fs::remove_dir_all(&out);
    let _ = std::fs::create_dir_all(&out);
    let mut harness =
        bf_paint::reftest::Harness::new(workspace.join("tests/wpt"), Some(out.clone()));
    let results = harness.run(&groups, filter.as_deref(), &|done, total| {
        eprintln!("reftests: {done}/{total}");
    });
    println!("{}", bf_paint::reftest::summary_table(&results, &groups));
    let mut tsv = String::from("test\tstatus\tdetail\n");
    for r in &results {
        tsv.push_str(&format!("{}\t{}\t{}\n", r.test, r.status.name(), r.detail));
    }
    let _ = std::fs::write(out.join("results.tsv"), tsv);
}

/// Volcado del árbol de fragmentos de los archivos de `REFTEST_DUMP` (rutas relativas a
/// `tests/wpt`, separadas por comas):
///
/// `REFTEST_DUMP=css/css-writing-modes/x.xht cargo test -p bf-paint --test wpt_reftests_groups wpt_dump -- --ignored --nocapture`
#[test]
#[ignore = "herramienta de depuración: correr con --ignored y REFTEST_DUMP"]
fn wpt_dump() {
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let Ok(files) = std::env::var("REFTEST_DUMP") else {
        eprintln!("aviso: falta REFTEST_DUMP; se salta");
        return;
    };
    let harness = bf_paint::reftest::Harness::new(workspace.join("tests/wpt"), None);
    for f in files.split(',').map(str::trim) {
        println!("===== {f}");
        println!(
            "{}",
            harness
                .dump(f)
                .unwrap_or_else(|| "(no se pudo leer)".into())
        );
    }
}
