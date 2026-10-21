//! WPT `dom/nodes` y `dom/events` con el runner de testharness.js de `bf_web::html` (E1):
//! cuenta los subtests que pasan por directorio.
//!
//! `cargo test --release -p bf-web --test wpt_dom -- --ignored --nocapture` (con
//! `WPT_FILTER=texto` para limitar los archivos y `WPT_VERBOSE=1` para ver los fallos).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bf_web::html::testharness::{SubStatus, is_test_file, run_doc, test_docs};

fn wpt_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/wpt")
}

fn run_dir(sub: &str) -> (usize, usize) {
    let root = wpt_root();
    let filter = std::env::var("WPT_FILTER").ok();
    let verbose = std::env::var("WPT_VERBOSE").is_ok();
    let mut files: Vec<String> = std::fs::read_dir(root.join(sub))
        .expect("dir de WPT")
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str().map(|n| format!("{sub}/{n}")))
        .filter(|rel| is_test_file(rel) && filter.as_deref().is_none_or(|f| rel.contains(f)))
        .collect();
    files.sort();
    let (mut pass, mut total) = (0, 0);
    for rel in files {
        for doc in test_docs(&root, &rel) {
            let t0 = Instant::now();
            let root = root.clone();
            // Un hilo con pila grande por documento (el intérprete en debug usa mucha pila).
            let r = std::thread::Builder::new()
                .stack_size(512 << 20)
                .spawn(move || run_doc(&root, &doc, Duration::from_secs(10)))
                .expect("hilo")
                .join();
            let Ok(r) = r else {
                println!("{rel}: pánico");
                continue;
            };
            let p = r.subtests.iter().filter(|s| s.1 == SubStatus::Pass).count();
            pass += p;
            total += r.subtests.len();
            println!(
                "{}: {p}/{} [{}] ({} ms)",
                r.path,
                r.subtests.len(),
                r.harness.label(),
                t0.elapsed().as_millis()
            );
            if verbose {
                for (name, status, msg) in &r.subtests {
                    if *status != SubStatus::Pass {
                        println!("    {} {name}: {msg}", status.label());
                    }
                }
            }
        }
    }
    println!("== {sub}: {pass}/{total}");
    (pass, total)
}

#[test]
#[ignore]
fn wpt_dom_nodes() {
    run_dir("dom/nodes");
}

#[test]
#[ignore]
fn wpt_dom_events() {
    run_dir("dom/events");
}
