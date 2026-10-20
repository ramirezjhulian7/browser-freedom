//! Fuzz básico: 1000 mutaciones aleatorias (deterministas) de los programas de
//! `test262-parser-tests/pass`; el parser debe terminar sin pánico con cualquier entrada
//! (aceptándola o con un SyntaxError).
//!
//! `cargo test -p bf-js --release --test fuzz -- --ignored --nocapture`
//! (`BF_FUZZ_N` cambia la cantidad de mutaciones y `BF_FUZZ_SEED`, la semilla).

use std::fs;
use std::path::PathBuf;

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
