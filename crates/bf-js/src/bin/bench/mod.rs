//! El benchmark de bf-js (`bf-bench` y `bf-js bench`): corre los programas de
//! `crates/bf-js/bench/*.js` (u otro directorio con `--dir`) en un agente nuevo cada uno, toma
//! el mejor de N corridas y muestra los tiempos.
//!
//! `[--filter texto] [--runs N] [--save archivo.tsv [--merge-min]] [--compare archivo.tsv]
//! [--dir directorio] [--gc]`. Con `--compare` muestra el speedup respecto de una línea base
//! guardada con `--save` y su media geométrica. Cada programa valida su resultado con
//! `check(valor, esperado)`; un resultado distinto es un error. Con `--merge-min`, `--save`
//! conserva el mínimo entre lo que ya tenía el archivo y lo nuevo (para alternar dos binarios
//! en una máquina con carga y comparar los mejores tiempos de cada uno). Con `--gc` escribe en
//! stderr las estadísticas del recolector de cada programa.

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

/// Una corrida: el tiempo en ms y las estadísticas del GC, o el error.
fn run_once(src: &str, name: &str) -> Result<(f64, String), String> {
    let mut rt = Runtime::new(Box::new(DefaultHooks));
    let realm = rt.main_realm();
    rt.eval_script(realm, CHECK, "check.js")
        .map_err(|e| e.message)?;
    let t = Instant::now();
    rt.eval_script(realm, src, name).map_err(|e| e.message)?;
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    Ok((ms, format!("{:?}", rt.agent.gc_stats())))
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

/// Corre el benchmark con los argumentos de la línea de comandos (sin el nombre del
/// programa), en un hilo con pila grande (la VM usa la pila del hilo para las llamadas).
pub fn main(args: Vec<String>) -> ExitCode {
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(move || real_main(args))
        .expect("hilo")
        .join()
        .unwrap_or(ExitCode::FAILURE)
}

fn real_main(args: Vec<String>) -> ExitCode {
    let opt = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1).cloned())
    };
    let filter = opt("--filter");
    let runs: usize = opt("--runs").and_then(|r| r.parse().ok()).unwrap_or(3);
    let base = opt("--compare").map(|p| load_tsv(&p));
    let gc = args.iter().any(|a| a == "--gc");
    let dir = opt("--dir").map_or_else(bench_dir, PathBuf::from);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("crates/bf-js/bench")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "js"))
        .filter(|p| {
            filter.as_deref().is_none_or(|f| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().contains(f))
            })
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
        let name = f
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let src = std::fs::read_to_string(f).unwrap_or_default();
        let mut best = f64::INFINITY;
        let mut err = None;
        let mut gc_stats = String::new();
        for _ in 0..runs {
            match run_once(&src, &format!("{name}.js")) {
                Ok((ms, stats)) => {
                    best = best.min(ms);
                    gc_stats = stats;
                }
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
        if gc {
            eprintln!("{name}: {gc_stats}");
        }
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
        if args.iter().any(|a| a == "--merge-min") {
            for (k, v) in load_tsv(&path) {
                let e = results.entry(k).or_insert(v);
                *e = e.min(v);
            }
        }
        let text: String = results
            .iter()
            .map(|(k, v)| format!("{k}\t{v:.3}\n"))
            .collect();
        if let Err(e) = std::fs::write(&path, text) {
            eprintln!("no se pudo escribir {path}: {e}");
            return ExitCode::FAILURE;
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
