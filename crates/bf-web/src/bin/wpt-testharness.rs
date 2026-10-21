//! Runner de los tests de testharness.js de WPT:
//! `wpt-testharness [dir…] [--root tests/wpt] [--out docs/wpt-status.md]
//! [--results target/wpt-testharness/results.tsv] [--threads N] [--filter texto]
//! [--timeout s] [--list-failures]`.
//!
//! Cada `*.html` / `*.any.js` / `*.window.js` de los directorios (relativos a la raíz de WPT)
//! corre en un agente nuevo con el documento del parser, sus scripts y el bucle de eventos
//! hasta el completion callback (ver `bf_web::html::testharness`). Escribe la tabla por
//! directorio en la sección "testharness" de `docs/wpt-status.md` y el detalle por subtest en
//! el TSV.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bf_web::html::testharness::{DocResult, Harness, SubStatus, is_test_file, run_doc, test_docs};

/// Los directorios por defecto.
const DEFAULT_DIRS: &[&str] = &[
    "dom",
    "html/dom",
    "html/semantics/scripting-1",
    "html/webappapis",
    "url",
    "encoding",
    "console",
];

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(root, &p, out);
        } else if let Ok(rel) = p.strip_prefix(root) {
            let rel = rel.to_string_lossy().replace('\\', "/");
            if is_test_file(&rel) {
                out.push(rel);
            }
        }
    }
}

/// `dom/nodes`, `html/dom`, `url`… (los dos primeros componentes del directorio).
fn group(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    match parts.as_slice() {
        [a, b, _, ..] => format!("{a}/{b}"),
        [a, ..] => (*a).to_owned(),
        [] => String::new(),
    }
}

#[derive(Default, Clone)]
struct Row {
    files: usize,
    files_ok: usize,
    pass: usize,
    fail: usize,
    timeout: usize,
    notrun: usize,
}

impl Row {
    fn add(&mut self, r: &DocResult) {
        self.files += 1;
        if r.harness == Harness::Ok {
            self.files_ok += 1;
        }
        for (_, s, _) in &r.subtests {
            match s {
                SubStatus::Pass => self.pass += 1,
                SubStatus::Fail | SubStatus::PreconditionFailed => self.fail += 1,
                SubStatus::Timeout => self.timeout += 1,
                SubStatus::NotRun => self.notrun += 1,
            }
        }
    }

    fn subtests(&self) -> usize {
        self.pass + self.fail + self.timeout + self.notrun
    }

    fn pct(&self) -> f64 {
        let n = self.subtests();
        if n == 0 {
            0.0
        } else {
            self.pass as f64 * 100.0 / n as f64
        }
    }
}

fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut root = PathBuf::from("tests/wpt");
    let mut out_doc: Option<PathBuf> = None;
    let mut results_path = PathBuf::from("target/wpt-testharness/results.tsv");
    let mut threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut filter: Option<String> = None;
    let mut timeout = 10u64;
    let mut list_failures = false;
    let mut dirs: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let next = |i: &mut usize| {
            *i += 1;
            args.get(*i).cloned().unwrap_or_default()
        };
        match args[i].as_str() {
            "--root" => root = PathBuf::from(next(&mut i)),
            "--out" => out_doc = Some(PathBuf::from(next(&mut i))),
            "--results" => results_path = PathBuf::from(next(&mut i)),
            "--threads" => threads = next(&mut i).parse().unwrap_or(threads),
            "--filter" => filter = Some(next(&mut i)),
            "--timeout" => timeout = next(&mut i).parse().unwrap_or(timeout),
            "--list-failures" => list_failures = true,
            d => dirs.push(d.trim_end_matches('/').to_owned()),
        }
        i += 1;
    }
    if dirs.is_empty() {
        dirs = DEFAULT_DIRS.iter().map(|s| (*s).to_owned()).collect();
    }
    let mut files = Vec::new();
    for d in &dirs {
        collect(&root, &root.join(d), &mut files);
    }
    if let Some(f) = &filter {
        files.retain(|p| p.contains(f.as_str()));
    }
    files.sort();
    files.dedup();
    let start = Instant::now();
    let limit = Duration::from_secs(timeout);
    let next = Mutex::new(0usize);
    let results: Mutex<Vec<DocResult>> = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            std::thread::Builder::new()
                .stack_size(256 << 20)
                .spawn_scoped(s, || {
                    loop {
                        let i = {
                            let mut n = next.lock().unwrap();
                            let i = *n;
                            *n += 1;
                            i
                        };
                        let Some(rel) = files.get(i) else { break };
                        for doc in test_docs(&root, rel) {
                            let r = run_doc(&root, &doc, limit);
                            results.lock().unwrap().push(r);
                        }
                    }
                })
                .expect("hilo del runner");
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by(|a, b| a.path.cmp(&b.path));

    // El detalle por subtest.
    if let Some(parent) = results_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut tsv = String::from("file\tharness\tsubtest\tstatus\tmessage\n");
    for r in &results {
        tsv.push_str(&format!(
            "{}\t{}\t\t\t{}\n",
            r.path,
            r.harness.label(),
            clean(r.harness.message())
        ));
        for (name, st, msg) in &r.subtests {
            tsv.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\n",
                r.path,
                r.harness.label(),
                clean(name),
                st.label(),
                clean(msg)
            ));
        }
    }
    let _ = fs::write(&results_path, tsv);

    // La tabla por directorio.
    let mut rows: BTreeMap<String, Row> = BTreeMap::new();
    let mut total = Row::default();
    for r in &results {
        rows.entry(group(&r.path)).or_default().add(r);
        total.add(r);
    }
    let mut table = String::from(
        "| Directorio | archivos | archivos OK | subtests | pass | fail | timeout | notrun | % pass |\n\
         |---|---:|---:|---:|---:|---:|---:|---:|---:|\n",
    );
    let line = |name: &str, r: &Row| {
        format!(
            "| {name} | {} | {} | {} | {} | {} | {} | {} | {:.1}% |\n",
            r.files,
            r.files_ok,
            r.subtests(),
            r.pass,
            r.fail,
            r.timeout,
            r.notrun,
            r.pct()
        )
    };
    for (name, r) in &rows {
        table.push_str(&line(name, r));
    }
    table.push_str(&line("**Total**", &total));
    println!("{table}");
    println!(
        "{} documentos en {:.0} s; subtests: {} pass / {} ({:.1}%)",
        results.len(),
        start.elapsed().as_secs_f64(),
        total.pass,
        total.subtests(),
        total.pct()
    );
    if list_failures {
        for r in &results {
            if r.harness != Harness::Ok {
                println!("{} {}: {}", r.harness.label(), r.path, r.harness.message());
            }
        }
    }
    if let Some(path) = out_doc {
        write_status(&path, &dirs, &table, results.len(), start.elapsed());
    }
}

/// Escribe la sección "testharness" de `docs/wpt-status.md` (las demás quedan).
fn write_status(path: &Path, dirs: &[String], table: &str, docs: usize, elapsed: Duration) {
    let commit = fs::read_to_string("tools/wpt-commit.txt").unwrap_or_default();
    let dirs: Vec<String> = dirs.iter().map(|d| format!("`{d}`")).collect();
    let section = format!(
        "## testharness\n\n\
         Generado por `cargo run --release -p bf-web --bin wpt-testharness -- --out docs/wpt-status.md`.\n\n\
         - Commit de WPT: `{}`; directorios: {}; {docs} documentos en {:.0} s.\n\
         - Cada `*.html` con `testharness.js`, `*.any.js` (variante `window`) y `*.window.js` corre en un \
         agente nuevo: el parser con los scripts clásicos (inline y `src` desde `tests/wpt` montado en \
         `http://web-platform.test:8000/`), el bucle de eventos con reloj virtual y un \
         `testharnessreport.js` propio que pasa los resultados al runner. Límite: 10 s por archivo.\n\
         - `archivos OK`: el harness terminó con estado OK (no implica que pasen todos sus subtests). \
         Detalle por subtest: `target/wpt-testharness/results.tsv`.\n\n\
         {table}",
        commit.trim(),
        dirs.join(", "),
        elapsed.as_secs_f64(),
    );
    let current = fs::read_to_string(path).unwrap_or_else(|_| "# Estado de WPT\n".to_owned());
    let _ = fs::write(path, replace_section(&current, "## testharness", &section));
}

/// Reemplaza la sección que empieza con `heading` (hasta la próxima `## `) o la agrega al
/// final.
fn replace_section(doc: &str, heading: &str, section: &str) -> String {
    let section = format!("{}\n", section.trim_end_matches('\n'));
    match doc.find(heading) {
        Some(start) => {
            let after = start + heading.len();
            match doc[after..].find("\n## ") {
                Some(e) => format!("{}{section}\n{}", &doc[..start], &doc[after + e + 1..]),
                None => format!("{}{section}", &doc[..start]),
            }
        }
        None => format!("{}\n\n{section}", doc.trim_end_matches('\n')),
    }
}
