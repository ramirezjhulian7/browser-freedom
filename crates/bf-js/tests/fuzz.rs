//! Fuzz básico: 1000 mutaciones aleatorias (deterministas) de los programas de
//! `test262-parser-tests/pass`; el parser debe terminar sin pánico con cualquier entrada
//! (aceptándola o con un SyntaxError).
//!
//! `fuzz_run` (E8) además **ejecuta**: muta los tests de la muestra de test262
//! (`tests/data/test262-sample.txt`, con el harness delante) y los corre con presupuesto de
//! instrucciones; ningún programa de página puede hacer entrar en pánico al motor (un
//! `unreachable!` alcanzable tiene que ser un error de JavaScript).
//!
//! `cargo test -p bf-js --release --test fuzz -- --ignored --nocapture`
//! (`BF_FUZZ_N` cambia la cantidad de mutaciones y `BF_FUZZ_SEED`, la semilla).

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use bf_js::vm::{Agent, AgentOptions};
use bf_js::{ParseOptions, parse_module, parse_script};

/// xorshift64*.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Fragmentos que suelen tocar casos borde del lexer y del parser.
const PIECES: &[&str] = &[
    "(", ")", "[", "]", "{", "}", "=>", "...", "?.", "??", "**", "=", ",", ";", "\n", "/", "/=",
    "`", "${", "'", "\"", "\\", "\\u{", "#", "#x", "async", "await", "yield", "let", "of", "in",
    "function", "class", "static", "get", "set", "super", "new", "import", "export", "default",
    "using", "0x", "0b1n", "1_", ".5", "08", "/a/u", "/[a-/v", "<!--", "-->", "/*", "*/",
    "\u{2028}", "\u{FEFF}", "\u{200D}", "𝒜",
];

fn mutate(src: &str, rng: &mut Rng) -> String {
    let mut chars: Vec<char> = src.chars().collect();
    for _ in 0..1 + rng.below(4) {
        let at = rng.below(chars.len() + 1);
        match rng.below(4) {
            0 if !chars.is_empty() => {
                let end = (at + 1 + rng.below(8)).min(chars.len());
                chars.drain(at.min(end)..end);
            }
            1 => {
                let piece = PIECES[rng.below(PIECES.len())];
                for (k, c) in piece.chars().enumerate() {
                    chars.insert((at + k).min(chars.len()), c);
                }
            }
            2 if !chars.is_empty() => {
                // Duplica un tramo (anidamientos, repeticiones).
                let start = rng.below(chars.len());
                let end = (start + 1 + rng.below(16)).min(chars.len());
                let slice: Vec<char> = chars[start..end].to_vec();
                for (k, c) in slice.into_iter().enumerate() {
                    chars.insert((at + k).min(chars.len()), c);
                }
            }
            _ if !chars.is_empty() => {
                let i = rng.below(chars.len());
                chars[i] = char::from_u32(rng.below(0x80) as u32).unwrap_or('x');
            }
            _ => {}
        }
    }
    chars.into_iter().collect()
}

#[test]
#[ignore]
fn fuzz_mutations() {
    let n: usize = std::env::var("BF_FUZZ_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000);
    let seed: u64 = std::env::var("BF_FUZZ_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0x5EED_1234_ABCD_0001);
    let dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/test262-parser-tests/pass");
    let mut sources: Vec<(String, bool)> = fs::read_dir(&dir)
        .expect("git submodule update --init")
        .map(|e| e.unwrap().path())
        .map(|p| {
            let module = p.to_string_lossy().ends_with(".module.js");
            (fs::read_to_string(&p).unwrap_or_default(), module)
        })
        .collect();
    sources.sort();
    let handle = std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || {
            let mut rng = Rng(seed);
            let (mut accepted, mut rejected) = (0, 0);
            for i in 0..n {
                let (src, module) = &sources[rng.below(sources.len())];
                let input = mutate(src, &mut rng);
                let opts = ParseOptions::default();
                let result = std::panic::catch_unwind(|| {
                    if *module {
                        parse_module(&input, &opts).is_ok()
                    } else {
                        parse_script(&input, &opts).is_ok()
                    }
                });
                match result {
                    Ok(true) => accepted += 1,
                    Ok(false) => rejected += 1,
                    Err(_) => panic!("pánico con la mutación {i}:\n{input}"),
                }
            }
            println!("{n} mutaciones: {accepted} aceptadas, {rejected} rechazadas, 0 pánicos");
        })
        .expect("hilo");
    handle.join().expect("sin pánicos");
}

/// Los `includes: [a.js, b.js]` del frontmatter (solo la forma en línea).
fn includes(src: &str) -> Vec<String> {
    src.lines()
        .find_map(|l| l.trim().strip_prefix("includes:"))
        .map(|rest| {
            rest.trim()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Corre un programa en un agente nuevo con los límites del runner de test262 (más chicos).
fn run_program(src: &str) {
    let mut agent = Agent::new(AgentOptions {
        max_stack: 32 << 20,
        max_frames: 5_000,
        instruction_budget: Some(2_000_000),
        deadline: Some(Instant::now() + Duration::from_secs(2)),
    });
    agent.reset_stack_base();
    agent.printer = Box::new(|_| {});
    let realm = agent.current_realm;
    bf_js::builtins::host::install_test262(&mut agent, realm);
    let _ = agent.eval_script(src, "fuzz.js");
    let _ = agent.run_jobs();
}

#[test]
#[ignore]
fn fuzz_run() {
    let n: usize = std::env::var("BF_FUZZ_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000);
    let seed: u64 = std::env::var("BF_FUZZ_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0x5EED_1234_ABCD_0002);
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.join("../../tests/test262");
    let index = fs::read_to_string(manifest.join("tests/data/test262-sample.txt"))
        .expect("tests/data/test262-sample.txt");
    let harness = |f: &str| fs::read_to_string(root.join("harness").join(f)).unwrap_or_default();
    let base = format!("{}\n{}\n", harness("assert.js"), harness("sta.js"));
    let mut programs: Vec<(String, String)> = Vec::new();
    for rel in index
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let Ok(src) = fs::read_to_string(root.join("test").join(rel)) else {
            continue;
        };
        // Los módulos, los `raw` y los asíncronos necesitan otro arnés.
        if src.contains("flags: [module")
            || src.contains("raw]")
            || src.contains("async]")
            || src.contains("CanBlockIsFalse")
        {
            continue;
        }
        let mut prelude = base.clone();
        for inc in includes(&src) {
            prelude.push_str(&harness(&inc));
            prelude.push('\n');
        }
        programs.push((prelude, src));
    }
    assert!(!programs.is_empty(), "falta tests/test262");
    let handle = std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(move || {
            let mut rng = Rng(seed);
            let mut panics: Vec<(usize, String, String)> = Vec::new();
            for i in 0..n {
                let (prelude, src) = &programs[rng.below(programs.len())];
                // Una de cada cuatro sin mutar: el programa válido llega más lejos.
                let body = if rng.below(4) == 0 {
                    src.clone()
                } else {
                    mutate(src, &mut rng)
                };
                let input = format!("{prelude}{body}");
                let r =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_program(&input)));
                if let Err(p) = r {
                    let msg = p
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                        .unwrap_or_default();
                    panics.push((i, msg, body));
                }
            }
            for (i, msg, body) in &panics {
                println!("== pánico con la ejecución {i}: {msg}\n{body}\n");
            }
            println!("{n} ejecuciones, {} pánicos", panics.len());
            panics.len()
        })
        .expect("hilo");
    assert_eq!(
        handle.join().expect("hilo del fuzz"),
        0,
        "pánicos al ejecutar"
    );
}
