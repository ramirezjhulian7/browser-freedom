//! `bf-bench [--filter texto] [--runs N] [--save archivo.tsv] [--compare archivo.tsv]`: corre
//! los programas de `crates/bf-js/bench/*.js` en un agente nuevo cada uno, toma el mejor de N
//! corridas y muestra los tiempos. Con `--compare` muestra el speedup respecto de una línea
//! base guardada con `--save` y su media geométrica. Cada programa valida su resultado con
//! `check(valor, esperado)`; un resultado distinto es un error.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use bf_js::host::{DefaultHooks, Runtime};

const CHECK: &str = "var __bench_ok = true; function check(v, e) { if (v !== e) { \
                     __bench_ok = false; throw new Error('resultado ' + v + ', se esperaba ' + e); } }";

fn bench_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bench")
}

/// Una corrida: el tiempo en ms, o el error.
fn run_once(src: &str, name: &str) -> Result<f64, String> {
    let mut rt = Runtime::new(Box::new(DefaultHooks));
    let realm = rt.main_realm();
    rt.eval_script(realm, CHECK, "check.js").map_err(|e| e.message)?;
    let t = Instant::now();
    rt.eval_script(realm, src, name).map_err(|e| e.message)?;
    Ok(t.elapsed().as_secs_f64() * 1000.0)
}

fn load_tsv(path: &str) -> BTreeMap<String, f64> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    text.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once('\t')?;
            Some((k.to_owned(), v.parse().ok()?))
        })
        .collect()
}

fn main() -> ExitCode {
    // La VM usa la pila del hilo para las llamadas: un hilo con pila grande.
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(real_main)
        .expect("hilo")
        .join()
        .unwrap_or(ExitCode::FAILURE)
}

fn real_main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opt = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1).cloned())
    };
    let filter = opt("--filter");
    let runs: usize = opt("--runs").and_then(|r| r.parse().ok()).unwrap_or(3);
    let base = opt("--compare").map(|p| load_tsv(&p));
    let mut files: Vec<PathBuf> = std::fs::read_dir(bench_dir())
        .expect("crates/bf-js/bench")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "js"))
        .filter(|p| {
            filter
                .as_deref()
                .is_none_or(|f| p.file_name().is_some_and(|n| n.to_string_lossy().contains(f)))
        })
        .collect();
    files.sort();
    let mut results = BTreeMap::new();
    let mut failed = false;
    let mut log_sum = 0.0;
    let mut compared = 0;
    println!("| benchmark | ms | base ms | speedup |");
    println!("|---|---:|---:|---:|");
    for f in &files {
        let name = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let src = std::fs::read_to_string(f).unwrap_or_default();
        let mut best = f64::INFINITY;
        let mut err = None;
        for _ in 0..runs {
            match run_once(&src, &format!("{name}.js")) {
                Ok(ms) => best = best.min(ms),
                Err(e) => {
                    err = Some(e);
                    break;
                }
            }
        }
        if let Some(e) = err {
            println!("| {name} | ERROR: {e} | | |");
            failed = true;
            continue;
        }
        results.insert(name.clone(), best);
        match base.as_ref().and_then(|b| b.get(&name)) {
            Some(&b) => {
                let speedup = b / best;
                log_sum += speedup.ln();
                compared += 1;
                println!("| {name} | {best:.1} | {b:.1} | {speedup:.2}× |");
            }
            None => println!("| {name} | {best:.1} | | |"),
        }
    }
    if compared > 0 {
        println!(
            "\nmedia geométrica del speedup ({compared} benchmarks): {:.2}×",
            (log_sum / f64::from(compared)).exp()
        );
    }
    if let Some(path) = opt("--save") {
        let text: String = results.iter().map(|(k, v)| format!("{k}\t{v:.3}\n")).collect();
        if let Err(e) = std::fs::write(&path, text) {
            eprintln!("no se pudo escribir {path}: {e}");
            return ExitCode::FAILURE;
        }
    }
    if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}
