//! Runner de test262: `test262 <dir-test262> [--out docs/test262-status.md] [--filter texto]
//! [--threads N] [--results archivo.tsv] [--list-failures]`.
//!
//! Ejecuta cada test según INTERPRETING.md: los includes del frontmatter después de
//! `assert.js` y `sta.js` (`doneprintHandle.js` si es `async`), un realm nuevo por ejecución
//! con `$262` y `print`, dos ejecuciones sin flags (no estricta y con `"use strict";`),
//! `negative` de fase `parse` y `runtime`. Los módulos (`module`, fase `resolution`) son del
//! paso 5 y se cuentan como `skipped`; las únicas otras omisiones son las propuestas de
//! `UNSUPPORTED_FEATURES` (las del parsing más `nonextensible-applies-to-private`). Un test pasa a `timeout` si agota el
//! presupuesto de instrucciones o el tiempo.
//!
//! Escribe la sección "Ejecución" de `docs/test262-status.md` (tabla por directorio y los 30
//! errores más frecuentes) y el detalle por test en `target/test262/results.tsv`.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bf_js::compiler::bytecode::FunctionCode;
use bf_js::value::Value;
use bf_js::vm::{Agent, AgentOptions, Throw};

/// Propuestas que todavía no están en ECMA-262 (las mismas del parsing).
const UNSUPPORTED_FEATURES: &[(&str, &str)] = &[
    ("decorators", "stage 3: `@dec` en clases"),
    ("source-phase-imports", "stage 3: `import source x from`"),
    (
        "source-phase-imports-module-source",
        "stage 3: `import source`",
    ),
    ("import-defer", "stage 3: `import defer * as ns from`"),
    ("export-defer", "stage 2.7: `export defer`"),
    (
        "error-stack-accessor",
        "stage 3: `Error.prototype.stack` como accessor; el motor usa la propiedad propia no estándar `stack` de los errores",
    ),
    (
        "nonextensible-applies-to-private",
        "stage 3: PrivateFieldAdd lanza TypeError en objetos no extensibles; ECMA-262 todavía lo permite y sus tests contradicen el spec vigente",
    ),
];

/// Presupuesto de instrucciones por ejecución.
const INSTRUCTION_BUDGET: u64 = 1_500_000_000;
const TIME_LIMIT: Duration = Duration::from_secs(10);

#[derive(Default, Debug)]
struct Meta {
    includes: Vec<String>,
    flags: Vec<String>,
    features: Vec<String>,
    negative_phase: Option<String>,
    negative_type: Option<String>,
}

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
    // Los terminadores de línea pueden ser CR solos (tests de normalización).
    let yaml = src[start + 5..start + end]
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let lines: Vec<&str> = yaml.lines().collect();
    let mut in_negative = false;
    for (i, line) in lines.iter().enumerate() {
        let indented = line.starts_with(' ') || line.starts_with('\t');
        let t = line.trim();
        if !indented {
            in_negative = t.starts_with("negative:");
        }
        if !indented {
            if let Some(rest) = t.strip_prefix("flags:") {
                meta.flags = yaml_list(&lines, i, rest);
            } else if let Some(rest) = t.strip_prefix("features:") {
                meta.features = yaml_list(&lines, i, rest);
            } else if let Some(rest) = t.strip_prefix("includes:") {
                meta.includes = yaml_list(&lines, i, rest);
            }
        } else if in_negative {
            if let Some(p) = t.strip_prefix("phase:") {
                meta.negative_phase = Some(p.trim().to_owned());
            } else if let Some(ty) = t.strip_prefix("type:") {
                meta.negative_type = Some(ty.trim().to_owned());
            }
        }
    }
    meta
}

#[derive(Debug, Clone, PartialEq)]
enum Status {
    Passed,
    Failed(String),
    Skipped(String),
    Timeout,
}

thread_local! {
    /// Los archivos del harness compilados (el bytecode no depende del realm).
    static HARNESS: RefCell<HashMap<String, Rc<FunctionCode>>> = RefCell::new(HashMap::new());
}

fn harness_code(agent: &mut Agent, root: &Path, name: &str) -> Result<Rc<FunctionCode>, String> {
    if let Some(c) = HARNESS.with(|h| h.borrow().get(name).cloned()) {
        return Ok(c);
    }
    let text = fs::read_to_string(root.join("harness").join(name))
        .map_err(|e| format!("harness {name}: {e}"))?;
    let code = agent
        .compile_script_text(&text, name)
        .map_err(|_| format!("harness {name}: SyntaxError"))?;
    HARNESS.with(|h| h.borrow_mut().insert(name.to_owned(), code.clone()));
    Ok(code)
}

/// El nombre del constructor de un valor lanzado (`e.constructor.name`).
fn thrown_name(agent: &mut Agent, v: &Value) -> String {
    let Value::Object(o) = v else {
        return format!("{v:?}");
    };
    let ctor = agent.get_str(*o, "constructor").unwrap_or_default();
    match ctor {
        Value::Object(c) => match agent.get_str(c, "name") {
            Ok(Value::String(s)) => s.to_string_lossy(),
            _ => "?".to_owned(),
        },
        _ => "?".to_owned(),
    }
}

fn describe(agent: &mut Agent, v: &Value) -> String {
    let name = thrown_name(agent, v);
    let msg = match v {
        Value::Object(o) => match agent.get_str(*o, "message") {
            Ok(Value::String(s)) => s.to_string_lossy(),
            _ => String::new(),
        },
        other => agent
            .to_string(other)
            .map(|s| s.to_string_lossy())
            .unwrap_or_default(),
    };
    format!("{name}: {msg}")
}

/// Una ejecución del test en un realm nuevo.
fn run_once(root: &Path, meta: &Meta, src: &str, strict: bool) -> Status {
    let output = Rc::new(RefCell::new(String::new()));
    let mut agent = Agent::new(AgentOptions {
        max_stack: 160 << 20,
        max_frames: 20_000,
        instruction_budget: Some(INSTRUCTION_BUDGET),
        deadline: Some(Instant::now() + TIME_LIMIT),
    });
    agent.reset_stack_base();
    agent.can_block = meta.flags.iter().any(|f| f == "CanBlockIsTrue"); // E3: AgentCanSuspend
    if let Some(t) = std::env::var("BF_GC_THRESHOLD")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        agent.heap.set_threshold(t);
    }
    let out2 = output.clone();
    agent.printer = Box::new(move |s| {
        let mut o = out2.borrow_mut();
        o.push_str(s);
        o.push('\n');
    });
    let realm = agent.current_realm;
    bf_js::builtins::host::install_test262(&mut agent, realm);
    let raw = meta.flags.iter().any(|f| f == "raw");
    let is_async = meta.flags.iter().any(|f| f == "async");
    if !raw {
        let mut files = vec!["assert.js".to_owned(), "sta.js".to_owned()];
        if is_async {
            files.push("doneprintHandle.js".to_owned());
        }
        files.extend(meta.includes.iter().cloned());
        for f in files {
            let code = match harness_code(&mut agent, root, &f) {
                Ok(c) => c,
                Err(e) => return Status::Failed(e),
            };
            if let Err(e) = agent.script_evaluation(code) {
                return match e {
                    Throw::Terminate(_) => Status::Timeout,
                    Throw::Value(v) => {
                        let d = describe(&mut agent, &v);
                        Status::Failed(format!("harness {f}: {d}"))
                    }
                };
            }
        }
    }
    let text = if strict {
        format!("\"use strict\";\n{src}")
    } else {
        src.to_owned()
    };
    let phase = meta.negative_phase.as_deref();
    let code = agent.compile_script_text(&text, "test.js");
    let result = match code {
        Err(Throw::Value(v)) => {
            if phase == Some("parse") {
                let n = thrown_name(&mut agent, &v);
                return if Some(n.as_str()) == meta.negative_type.as_deref() {
                    Status::Passed
                } else {
                    Status::Failed(format!(
                        "se esperaba {:?} al parsear, fue {n}",
                        meta.negative_type
                    ))
                };
            }
            let d = describe(&mut agent, &v);
            return Status::Failed(format!("parse: {d}"));
        }
        Err(Throw::Terminate(_)) => return Status::Timeout,
        Ok(code) => {
            if phase == Some("parse") {
                return Status::Failed("se parseó (negative: parse)".into());
            }
            // E3: RunJobs al terminar el script (la cola de promesas).
            agent
                .script_evaluation(code)
                .and_then(|v| agent.run_jobs().map(|_| v))
        }
    };
    let internal = agent.internal_errors.first().cloned();
    match result {
        Err(Throw::Terminate(_)) => Status::Timeout,
        Err(Throw::Value(v)) => {
            if phase == Some("runtime") {
                let n = thrown_name(&mut agent, &v);
                if Some(n.as_str()) == meta.negative_type.as_deref() && internal.is_none() {
                    return Status::Passed;
                }
            }
            if let Some(i) = internal {
                return Status::Failed(format!("InternalError: {i}"));
            }
            Status::Failed(describe(&mut agent, &v))
        }
        Ok(_) => {
            if let Some(i) = internal {
                return Status::Failed(format!("InternalError: {i}"));
            }
            if phase == Some("runtime") {
                return Status::Failed(format!(
                    "se esperaba {} y no se lanzó nada",
                    meta.negative_type.clone().unwrap_or_default()
                ));
            }
            if is_async {
                let out = output.borrow();
                if out.contains("Test262:AsyncTestComplete")
                    && !out.contains("Test262:AsyncTestFailure")
                {
                    return Status::Passed;
                }
                let line = out
                    .lines()
                    .find(|l| l.starts_with("Test262:AsyncTestFailure"))
                    .unwrap_or("async: $DONE nunca se llamó");
                return Status::Failed(line.to_owned());
            }
            Status::Passed
        }
    }
}

fn run_test(root: &Path, path: &Path) -> Status {
    let Ok(bytes) = fs::read(path) else {
        return Status::Failed("no se pudo leer".into());
    };
    let src = String::from_utf8_lossy(&bytes).into_owned();
    let meta = metadata(&src);
    if let Some(f) = meta
        .features
        .iter()
        .find(|f| UNSUPPORTED_FEATURES.iter().any(|(u, _)| u == f))
    {
        return Status::Skipped(format!("feature {f}"));
    }
    if meta.flags.iter().any(|f| f == "module")
        || meta.negative_phase.as_deref() == Some("resolution")
    {
        return Status::Skipped("module (paso 5)".into());
    }
    let has = |f: &str| meta.flags.iter().any(|x| x == f);
    let modes: &[bool] = if has("raw") || has("noStrict") {
        &[false]
    } else if has("onlyStrict") {
        &[true]
    } else {
        &[false, true]
    };
    for &strict in modes {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_once(root, &meta, &src, strict)
        }));
        let status = match r {
            Ok(s) => s,
            Err(p) => {
                let msg = p
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                    .unwrap_or_default();
                Status::Failed(format!("pánico: {msg}"))
            }
        };
        match status {
            Status::Passed => {}
            Status::Failed(m) => {
                let mode = if strict { "strict" } else { "sloppy" };
                return Status::Failed(format!("[{mode}] {m}"));
            }
            other => return other,
        }
    }
    Status::Passed
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

#[derive(Default, Clone, Copy)]
struct Row {
    passed: usize,
    failed: usize,
    skipped: usize,
    timeout: usize,
}

impl Row {
    fn add(&mut self, o: &Row) {
        self.passed += o.passed;
        self.failed += o.failed;
        self.skipped += o.skipped;
        self.timeout += o.timeout;
    }

    fn pct(&self) -> f64 {
        let ran = self.passed + self.failed + self.timeout;
        if ran == 0 {
            100.0
        } else {
            self.passed as f64 * 100.0 / ran as f64
        }
    }
}

/// Normaliza un mensaje de error para agruparlo.
fn normalize(msg: &str) -> String {
    let m = msg.split_once("] ").map_or(msg, |(_, r)| r);
    let m = m.lines().next().unwrap_or("");
    let mut out = String::new();
    let mut chars = m.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '«' => {
                for d in chars.by_ref() {
                    if d == '»' {
                        break;
                    }
                }
                out.push_str("«…»");
            }
            '0'..='9' => {
                while chars.peek().is_some_and(|d| d.is_ascii_digit()) {
                    chars.next();
                }
                out.push('N');
            }
            _ => out.push(c),
        }
    }
    if out.len() > 140 {
        let mut cut = 140;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        out.push('…');
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut root = PathBuf::from("tests/test262");
    let mut out_doc: Option<PathBuf> = None;
    let mut filter: Option<String> = None;
    let mut threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut results_path = PathBuf::from("target/test262/results.tsv");
    let mut list_failures = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--out" => {
                i += 1;
                out_doc = args.get(i).map(PathBuf::from);
            }
            "--filter" => {
                i += 1;
                filter = args.get(i).cloned();
            }
            "--threads" => {
                i += 1;
                threads = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(threads);
            }
            "--results" => {
                i += 1;
                if let Some(p) = args.get(i) {
                    results_path = PathBuf::from(p);
                }
            }
            "--list-failures" => list_failures = true,
            other => root = PathBuf::from(other),
        }
        i += 1;
    }
    let test_dir = root.join("test");
    if !test_dir.exists() {
        eprintln!(
            "falta {}: correr tools/fetch-test262.sh o tools/fetch-test262.ps1",
            test_dir.display()
        );
        std::process::exit(2);
    }
    let mut files = Vec::new();
    collect(&test_dir, &mut files);
    files.sort();
    let rels: Vec<String> = files
        .iter()
        .map(|p| {
            p.strip_prefix(&test_dir)
                .unwrap_or(p)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    let selected: Vec<usize> = (0..files.len())
        .filter(|&i| filter.as_ref().is_none_or(|f| rels[i].contains(f.as_str())))
        .collect();
    let started = Instant::now();
    let results: Mutex<Vec<(usize, Status)>> = Mutex::new(Vec::with_capacity(selected.len()));
    let next = Mutex::new(0usize);
    let done = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..threads {
            std::thread::Builder::new()
                .stack_size(256 << 20)
                .spawn_scoped(s, || {
                    loop {
                        let k = {
                            let mut n = next.lock().unwrap();
                            let k = *n;
                            *n += 1;
                            k
                        };
                        let Some(&idx) = selected.get(k) else { break };
                        let status = run_test(&root, &files[idx]);
                        results.lock().unwrap().push((idx, status));
                        let d = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                        if d.is_multiple_of(2000) {
                            eprintln!(
                                "{d}/{} ({:.0} s)",
                                selected.len(),
                                started.elapsed().as_secs_f64()
                            );
                        }
                    }
                })
                .expect("hilo");
        }
    });
    let elapsed = started.elapsed();
    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|(i, _)| *i);
    let mut rows: BTreeMap<String, Row> = BTreeMap::new();
    let mut errors: HashMap<String, usize> = HashMap::new();
    let mut tsv = String::from("test\tstatus\tmessage\n");
    for (idx, st) in &results {
        let rel = &rels[*idx];
        let row = rows.entry(group(rel)).or_default();
        let (name, msg) = match st {
            Status::Passed => {
                row.passed += 1;
                ("passed", String::new())
            }
            Status::Failed(m) => {
                row.failed += 1;
                *errors.entry(normalize(m)).or_default() += 1;
                ("failed", m.clone())
            }
            Status::Skipped(m) => {
                row.skipped += 1;
                ("skipped", m.clone())
            }
            Status::Timeout => {
                row.timeout += 1;
                ("timeout", String::new())
            }
        };
        tsv.push_str(&format!(
            "{rel}\t{name}\t{}\n",
            msg.replace(['\t', '\n'], " ")
        ));
        if list_failures && name == "failed" {
            println!("FALLO {rel}: {msg}");
        }
    }
    if let Some(dir) = results_path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(&results_path, tsv);
    let mut total = Row::default();
    for r in rows.values() {
        total.add(r);
    }
    // language/ e annexB/language/ agregados.
    let mut language = Row::default();
    let mut annexb_lang = Row::default();
    for (g, r) in &rows {
        if g.starts_with("language/") {
            language.add(r);
        }
        if g == "annexB/language" {
            annexb_lang.add(r);
        }
    }
    let mut table = String::from(
        "| Directorio | passed | failed | skipped | timeout | % |\n|---|---:|---:|---:|---:|---:|\n",
    );
    for (name, r) in &rows {
        table.push_str(&format!(
            "| {name} | {} | {} | {} | {} | {:.1}% |\n",
            r.passed,
            r.failed,
            r.skipped,
            r.timeout,
            r.pct()
        ));
    }
    table.push_str(&format!(
        "| **language/ (total)** | {} | {} | {} | {} | {:.1}% |\n",
        language.passed,
        language.failed,
        language.skipped,
        language.timeout,
        language.pct()
    ));
    table.push_str(&format!(
        "| **Total** | {} | {} | {} | {} | {:.1}% |\n",
        total.passed,
        total.failed,
        total.skipped,
        total.timeout,
        total.pct()
    ));
    let mut top: Vec<(String, usize)> = errors.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut top_md = String::from("| # | tests | error |\n|---:|---:|---|\n");
    for (i, (m, n)) in top.iter().take(30).enumerate() {
        top_md.push_str(&format!(
            "| {} | {n} | `{}` |\n",
            i + 1,
            m.replace('|', "\\|").replace('`', "'")
        ));
    }
    println!("{table}");
    println!("{top_md}");
    println!(
        "language/: {:.2}%  annexB/language: {:.2}%  total: {:.2}%  ({} tests en {:.0} s)",
        language.pct(),
        annexb_lang.pct(),
        total.pct(),
        results.len(),
        elapsed.as_secs_f64()
    );
    if let Some(doc) = out_doc {
        write_section(
            &doc,
            &root,
            &table,
            &top_md,
            elapsed,
            results.len(),
            language.pct(),
        );
    }
}

/// Reemplaza (o agrega) la sección "Ejecución" del documento de estado.
fn write_section(
    doc: &Path,
    root: &Path,
    table: &str,
    top: &str,
    elapsed: Duration,
    n: usize,
    language: f64,
) {
    let commit = fs::read_to_string(root.join("../../tools/test262-commit.txt"))
        .or_else(|_| fs::read_to_string("tools/test262-commit.txt"))
        .unwrap_or_default();
    let date = today();
    let features: String = UNSUPPORTED_FEATURES
        .iter()
        .map(|(f, why)| format!("  - `{f}`: {why}.\n"))
        .collect();
    let section = format!(
        "## Ejecución\n\n\
         Generado por `cargo run --release -p bf-js --bin test262 -- tests/test262 --out docs/test262-status.md`.\n\n\
         - Commit de test262: `{}`; fecha: {date}; {n} archivos en {:.0} s.\n\
         - Cada test corre en un realm nuevo con `assert.js`, `sta.js` y sus `includes`; sin flags, dos \
         veces (no estricto y con `\"use strict\";`). Pasa solo si pasan todas sus ejecuciones.\n\
         - `skipped`: módulos (paso 5) y las propuestas de `UNSUPPORTED_FEATURES`. Todo lo demás que \
         falla cuenta como `failed`, incluido lo que todavía no está implementado; `timeout`: \
         presupuesto de instrucciones o 10 s. Las propuestas omitidas:\n\
         {features}\
         - Detalle por test: `target/test262/results.tsv`.\n\
         - `language/` (referencia de regresión de `tests/test262_run.rs`): {language:.2}%\n\n\
         {table}\n\
         ### Los 30 errores más frecuentes\n\n\
         {top}",
        commit.trim(),
        elapsed.as_secs_f64(),
    );
    let current = fs::read_to_string(doc).unwrap_or_else(|_| "# Estado de test262\n".to_owned());
    let new = replace_section(&current, "## Ejecución", &section);
    let _ = fs::write(doc, new);
}

/// Reemplaza la sección que empieza con `heading` (hasta la próxima `## `) o la agrega al
/// final; las demás secciones (la "Parsing" de `test262_parse`) quedan como estaban.
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

/// La fecha UTC de hoy (AAAA-MM-DD) sin dependencias.
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = (secs / 86400) as i64;
    // Algoritmo civil_from_days (Howard Hinnant).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}
