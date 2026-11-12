//! E6: WPT `fetch/http-cache` (la caché HTTP de `bf-net`, RFC 9111) con el runner de
//! testharness.js. Ignorado: necesita `tests/wpt` (`tools/fetch-wpt`). Imprime cada subtest
//! que no pasa y el total.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bf_web::html::testharness::{Harness, SubStatus, doc_limit, is_test_file, run_doc, test_docs};

#[test]
#[ignore]
fn wpt_fetch_http_cache() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/wpt");
    let dir = root.join("fetch/http-cache");
    assert!(
        dir.exists(),
        "falta tests/wpt/fetch/http-cache: tools/fetch-wpt"
    );
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .expect("fetch/http-cache")
        .flatten()
        .filter_map(|e| {
            let rel = e.path().strip_prefix(&root).ok()?.to_owned();
            let rel = rel.to_string_lossy().replace('\\', "/");
            is_test_file(&rel).then_some(rel)
        })
        .collect();
    files.sort();
    let (mut pass, mut total) = (0, 0);
    let mut crashes = Vec::new();
    for rel in &files {
        for doc in test_docs(Path::new(&root), rel) {
            let r = run_doc(&root, &doc, doc_limit(&doc, Duration::from_secs(30)));
            let p = r
                .subtests
                .iter()
                .filter(|(_, s, _)| *s == SubStatus::Pass)
                .count();
            println!("{}: {p}/{} ({:?})", r.path, r.subtests.len(), r.harness);
            for (name, s, msg) in &r.subtests {
                if *s != SubStatus::Pass {
                    println!("  {s:?} {name}: {msg}");
                }
            }
            pass += p;
            total += r.subtests.len();
            if let Harness::Crash(m) = &r.harness {
                crashes.push(format!("{}: {m}", r.path));
            }
        }
    }
    println!(
        "fetch/http-cache: {pass} pass / {total} ({:.1} %)",
        100.0 * pass as f64 / total.max(1) as f64
    );
    assert!(crashes.is_empty(), "pánicos:\n{}", crashes.join("\n"));
}
