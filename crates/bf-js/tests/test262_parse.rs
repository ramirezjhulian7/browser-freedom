//! Parsing de test262 (`tests/test262`, `tools/fetch-test262`): sin ejecutar nada, cada test
//! debe parsear salvo que su frontmatter diga `negative: { phase: parse }`, en cuyo caso debe
//! fallar. Se respetan los flags `module`, `onlyStrict`, `noStrict` y `raw` (sin flags, el test
//! se parsea en código no estricto y con `"use strict";` delante, como hace el runner). Los
//! `_FIXTURE.js` son módulos auxiliares y no se cuentan.
//!
//! Escribe el resumen por directorio en la sección "Parsing" de `docs/test262-status.md`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Tests `negative: phase: parse` detectados (control del frontmatter).
static NEGATIVE: AtomicUsize = AtomicUsize::new(0);

use bf_js::{ParseOptions, parse_module, parse_script};

/// Propuestas que todavía no están en ECMA-262 y agregan sintaxis: sus tests no se cuentan.
const UNSUPPORTED_FEATURES: &[(&str, &str)] = &[
    ("decorators", "stage 3: `@dec` en clases"),
    ("source-phase-imports", "stage 3: `import source x from`"),
    (
        "source-phase-imports-module-source",
        "stage 3: `import source` (especificador de test262)",
    ),
    ("import-defer", "stage 3: `import defer * as ns from`"),
    ("export-defer", "stage 2.7: `export defer`"),
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/test262")
}

#[derive(Default)]
struct Meta {
    flags: Vec<String>,
    features: Vec<String>,
    negative_parse: bool,
}

/// Una lista YAML en línea (`[a, b]`) o en bloque (`- a`).
fn yaml_list(lines: &[&str], i: usize, rest: &str) -> Vec<String> {
    let rest = rest.trim();
    if let Some(inner) = rest.strip_prefix('[') {
        return inner
            .trim_end_matches(']')
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
    }
    let mut out = Vec::new();
    for l in &lines[i + 1..] {
        let t = l.trim();
        match t.strip_prefix("- ") {
            Some(item) => out.push(item.trim().to_owned()),
            None => break,
        }
    }
    out
}

fn metadata(src: &str) -> Meta {
    let mut meta = Meta::default();
    let Some(start) = src.find("/*---") else {
        return meta;
    };
    let Some(end) = src[start..].find("---*/") else {
        return meta;
    };
    let yaml = &src[start + 5..start + end];
    let lines: Vec<&str> = yaml.lines().collect();
    let mut in_negative = false;
    for (i, line) in lines.iter().enumerate() {
        let indented = line.starts_with(' ') || line.starts_with('\t');
        let t = line.trim();
        if !indented {
            in_negative = t.starts_with("negative:");
        }
        if let Some(rest) = t.strip_prefix("flags:")
            && !indented
        {
            meta.flags = yaml_list(&lines, i, rest);
        } else if let Some(rest) = t.strip_prefix("features:")
            && !indented
        {
            meta.features = yaml_list(&lines, i, rest);
        } else if in_negative && t.starts_with("phase:") && t.ends_with("parse") {
            meta.negative_parse = true;
        }
    }
    meta
}

enum Outcome {
    Passed,
    Skipped,
    Failed(String),
}

fn check(path: &Path) -> Outcome {
    let Ok(bytes) = fs::read(path) else {
        return Outcome::Failed("no se pudo leer".into());
    };
    let src = String::from_utf8_lossy(&bytes).into_owned();
    let meta = metadata(&src);
    if meta
        .features
        .iter()
        .any(|f| UNSUPPORTED_FEATURES.iter().any(|(u, _)| u == f))
    {
        return Outcome::Skipped;
    }
    if meta.negative_parse {
        NEGATIVE.fetch_add(1, Ordering::Relaxed);
    }
    let has = |f: &str| meta.flags.iter().any(|x| x == f);
    // Los hilos tienen 256 MiB de pila: en debug los frames del parser son grandes.
    let opts = ParseOptions {
        max_stack: 192 << 20,
        ..ParseOptions::default()
    };
    let mut runs: Vec<(&str, Result<(), String>)> = Vec::new();
    let res = |r: Result<bf_js::ast::Program, bf_js::SyntaxError>| {
        r.map(|_| ()).map_err(|e| e.to_string())
    };
    if has("module") {
        runs.push(("module", res(parse_module(&src, &opts))));
    } else if has("raw") || has("noStrict") {
        runs.push(("sloppy", res(parse_script(&src, &opts))));
    } else if has("onlyStrict") {
        runs.push((
            "strict",
            res(parse_script(&format!("\"use strict\";\n{src}"), &opts)),
        ));
    } else {
        runs.push(("sloppy", res(parse_script(&src, &opts))));
        runs.push((
            "strict",
            res(parse_script(&format!("\"use strict\";\n{src}"), &opts)),
        ));
    }
    for (mode, r) in runs {
        match (meta.negative_parse, r) {
            (true, Ok(())) => {
                return Outcome::Failed(format!("[{mode}] se aceptó (negative: parse)"));
            }
            (false, Err(e)) => return Outcome::Failed(format!("[{mode}] {e}")),
            _ => {}
        }
    }
    Outcome::Passed
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x == "js")
            && !p.to_string_lossy().contains("_FIXTURE")
        {
            out.push(p);
        }
    }
}

/// `language/expressions`, `built-ins/Array`, `annexB/language`…
fn group(rel: &str) -> String {
    let parts: Vec<&str> = rel.split('/').collect();
    match parts.as_slice() {
        [a, b, _, ..] => format!("{a}/{b}"),
        [a, ..] => (*a).to_owned(),
        [] => String::new(),
    }
}

#[derive(Default, Clone)]
struct Row {
    passed: usize,
    skipped: usize,
    failed: usize,
}

#[test]
fn test262_parse() {
    let test_dir = root().join("test");
    assert!(
        test_dir.exists(),
        "falta tests/test262: correr tools/fetch-test262.sh o tools/fetch-test262.ps1"
    );
    let mut files = Vec::new();
    collect(&test_dir, &mut files);
    files.sort();
    let results: Mutex<Vec<(String, Outcome)>> = Mutex::new(Vec::with_capacity(files.len()));
    let next = Mutex::new(0usize);
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..workers {
            let builder = std::thread::Builder::new().stack_size(256 << 20);
            builder
                .spawn_scoped(s, || {
                    loop {
                        let i = {
                            let mut n = next.lock().unwrap();
                            let i = *n;
                            *n += 1;
                            i
                        };
                        let Some(path) = files.get(i) else { break };
                        let rel = path
                            .strip_prefix(&test_dir)
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', "/");
                        // Un pánico es un fallo del parser, no del harness.
                        let outcome = std::panic::catch_unwind(|| check(path))
                            .unwrap_or_else(|_| Outcome::Failed("pánico del parser".into()));
                        results.lock().unwrap().push((rel, outcome));
                    }
                })
                .expect("hilo del harness");
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by(|a, b| a.0.cmp(&b.0));
    let mut rows: BTreeMap<String, Row> = BTreeMap::new();
    let mut failures = Vec::new();
    for (rel, outcome) in &results {
        let row = rows.entry(group(rel)).or_default();
        match outcome {
            Outcome::Passed => row.passed += 1,
            Outcome::Skipped => row.skipped += 1,
            Outcome::Failed(m) => {
                row.failed += 1;
                failures.push(format!("{rel}: {m}"));
            }
        }
    }
    let total = rows.values().fold(Row::default(), |a, r| Row {
        passed: a.passed + r.passed,
        skipped: a.skipped + r.skipped,
        failed: a.failed + r.failed,
    });
    let pct = |r: &Row| {
        let run = r.passed + r.failed;
        if run == 0 {
            100.0
        } else {
            r.passed as f64 * 100.0 / run as f64
        }
    };
    let mut table =
        String::from("| Directorio | passed | skipped | failed | % |\n|---|---:|---:|---:|---:|\n");
    for (name, r) in &rows {
        table.push_str(&format!(
            "| {name} | {} | {} | {} | {:.2}% |\n",
            r.passed,
            r.skipped,
            r.failed,
            pct(r)
        ));
    }
    table.push_str(&format!(
        "| **Total** | {} | {} | {} | {:.2}% |\n",
        total.passed,
        total.skipped,
        total.failed,
        pct(&total)
    ));
    println!("{table}");
    println!(
        "negative: phase: parse detectados: {}",
        NEGATIVE.load(Ordering::Relaxed)
    );
    if std::env::var_os("BF_TEST262_FAILURES").is_some() {
        for f in &failures {
            println!("FALLO {f}");
        }
    } else {
        for f in failures.iter().take(50) {
            println!("FALLO {f}");
        }
    }
    write_status(&table, total.passed + total.failed);
    assert!(
        pct(&total) >= 99.5,
        "parsing de test262 por debajo de 99.5%: {:.2}%",
        pct(&total)
    );
}

fn write_status(table: &str, ran: usize) {
    let commit = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/test262-commit.txt"),
    )
    .unwrap_or_default();
    let features: String = UNSUPPORTED_FEATURES
        .iter()
        .map(|(f, why)| format!("- `{f}`: {why}.\n"))
        .collect();
    let section = format!(
        "## Parsing\n\n\
         Generado por `cargo test -p bf-js --test test262_parse -- --nocapture`.\n\n\
         - Commit de test262: `{}`\n\
         - {ran} tests parseados (sin ejecutar): deben parsear, salvo los `negative: phase: parse`, \
         que deben fallar. Sin flags, cada test se parsea en código no estricto y con \
         `\"use strict\";` delante; con `module`, como módulo.\n\
         - `skipped`: tests de propuestas que agregan sintaxis y todavía no están en el spec:\n\n\
         {features}\n\
         {table}",
        commit.trim(),
    );
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/test262-status.md");
    let current = fs::read_to_string(&path).unwrap_or_else(|_| "# Estado de test262\n".to_owned());
    let _ = fs::write(path, replace_section(&current, "## Parsing", &section));
}

/// Reemplaza la sección que empieza con `heading` (hasta la próxima `## `) o la agrega al
/// final; las demás secciones (la "Ejecución" del runner) quedan como estaban.
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
