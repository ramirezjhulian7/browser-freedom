//! test262-parser-tests (`tests/test262-parser-tests`, submódulo):
//! - `pass/` y `pass-explicit/` deben parsear (como módulo si el nombre termina en
//!   `.module.js`) y dar el mismo AST (ESTree sin posiciones ni `raw`: el corpus cambia los
//!   paréntesis, los puntos y comas y la forma de los literales);
//! - `fail/` y `early/` deben fallar.
//!
//! El corpus es de ES2017: los casos que el spec actual cambió están en [`STALE`] con su
//! motivo.

use std::fs;
use std::path::{Path, PathBuf};

use bf_js::estree::{Options, to_json};
use bf_js::{ParseOptions, parse_module, parse_script};

/// Casos cuya expectativa ya no corresponde al spec actual: (carpeta/archivo, motivo).
const STALE: &[(&str, &str)] = &[
    (
        "pass/0339fa95c78c11bd.js",
        "pass-explicit cambia un literal (`1` por `0`): no es el mismo programa",
    ),
    (
        "pass/2179895ec5cc6276.js",
        "pass-explicit cambia un literal (`1` por `0`): no es el mismo programa",
    ),
    (
        "pass/8c80f7ee04352eba.js",
        "pass-explicit cambia un literal (`1` por `0`): no es el mismo programa",
    ),
    (
        "pass/0b4d61559ccce0f9.js",
        "pass-explicit cambia un literal (`1` por `0`): no es el mismo programa",
    ),
    (
        "pass/5829d742ab805866.js",
        "pass-explicit cambia un literal (`1` por `0`): no es el mismo programa",
    ),
    (
        "pass/7788d3c1e1247da9.js",
        "pass-explicit cambia un literal (`1` por `0`): no es el mismo programa",
    ),
    (
        "pass/82c827ccaecbe22b.js",
        "pass-explicit cambia un literal (`1` por `0`): no es el mismo programa",
    ),
    (
        "pass/988e362ed9ddcac5.js",
        "pass-explicit cambia un literal (`1` por `0`): no es el mismo programa",
    ),
    (
        "pass/e71c1d5f0b6b833c.js",
        "pass-explicit cambia un literal (`1` por `0`): no es el mismo programa",
    ),
    (
        "pass/441a92357939904a.js",
        "pass-explicit cambia un literal (`1` por `0`): no es el mismo programa",
    ),
    (
        "pass/598a5cedba92154d.js",
        "pass-explicit cambia un literal (`1` por `0`): no es el mismo programa",
    ),
    (
        "pass/a8a03a88237c4e8f.js",
        "pass-explicit cambia un literal (`1` por `0`): no es el mismo programa",
    ),
    (
        "pass/ad06370e34811a6a.js",
        "pass-explicit cambia un literal (`1` por `0`): no es el mismo programa",
    ),
    (
        "pass/0f88c334715d2489.js",
        "pass-explicit cambia un literal (`1` por `0`): no es el mismo programa",
    ),
    (
        "pass/7dab6e55461806c9.js",
        "pass-explicit cambia un literal (`1` por `0`): no es el mismo programa",
    ),
    (
        "pass/cb211fadccb029c7.js",
        "pass-explicit cambia un literal (`1` por `0`): no es el mismo programa",
    ),
    (
        "pass/ce968fcdf3a1987c.js",
        "pass-explicit cambia un literal (`1` por `0`): no es el mismo programa",
    ),
    (
        "pass/1093d98f5fc0758d.js",
        "pass-explicit renombra identificadores (`a` por `x`): no es el mismo programa",
    ),
    (
        "pass/15d9592709b947a0.js",
        "pass-explicit renombra identificadores (`a` por `x`): no es el mismo programa",
    ),
    (
        "pass/247a3a57e8176ebd.js",
        "pass-explicit renombra identificadores (`a` por `x`): no es el mismo programa",
    ),
    (
        "pass/47f974d6fc52e3e4.js",
        "pass-explicit renombra identificadores (`a` por `x`): no es el mismo programa",
    ),
    (
        "pass/4e1a0da46ca45afe.js",
        "pass-explicit renombra identificadores (`a` por `x`): no es el mismo programa",
    ),
    (
        "pass/72d79750e81ef03d.js",
        "pass-explicit renombra identificadores (`a` por `x`): no es el mismo programa",
    ),
    (
        "pass/96f5d93be9a54573.js",
        "pass-explicit renombra identificadores (`a` por `x`): no es el mismo programa",
    ),
    (
        "pass/9bcae7c7f00b4e3c.js",
        "pass-explicit renombra identificadores (`a` por `x`): no es el mismo programa",
    ),
    (
        "pass/db3c01738aaf0b92.js",
        "pass-explicit renombra identificadores (`a` por `x`): no es el mismo programa",
    ),
    (
        "pass/e1387fe892984e2b.js",
        "pass-explicit renombra identificadores (`a` por `x`): no es el mismo programa",
    ),
    (
        "pass/e8ea384458526db0.js",
        "pass-explicit renombra identificadores (`a` por `x`): no es el mismo programa",
    ),
    (
        "pass/589dc8ad3b9aa28f.js",
        "pass-explicit convierte un argumento `(a, b, c)` en varios argumentos",
    ),
    (
        "pass/87a9b0d1d80812cc.js",
        "pass-explicit convierte un argumento `(a, b, c)` en varios argumentos",
    ),
    (
        "pass/0426f15dac46e92d.js",
        "pass-explicit cambia la clave '1e2000' por Infinity (otra propiedad)",
    ),
    (
        "pass/7b72d7b43bedc895.js",
        "pass-explicit cambia la clave '0x20' por 0 (otra propiedad)",
    ),
    (
        "pass/b62c6dd890bef675.js",
        "pass-explicit cambia la clave '0x20' por 0 (otra propiedad)",
    ),
    (
        "pass/b0fdc038ee292aba.js",
        "`09.5` es 9.5 (NonOctalDecimalIntegerLiteral con fracción, §12.9.3); pass-explicit dice `9`",
    ),
    (
        "fail/0d5e450f1da8a92a.js",
        r"`\8` y `\9` son válidos fuera de código estricto desde ES2021 (NonOctalDecimalEscapeSequence, §12.9.4)",
    ),
    (
        "fail/748656edbfb2d0bb.js",
        r"`\8` y `\9` son válidos fuera de código estricto desde ES2021 (NonOctalDecimalEscapeSequence, §12.9.4)",
    ),
    (
        "fail/79f882da06f88c9f.js",
        r"`\8` y `\9` son válidos fuera de código estricto desde ES2021 (NonOctalDecimalEscapeSequence, §12.9.4)",
    ),
    (
        "fail/92b6af54adef3624.js",
        r"`\8` y `\9` son válidos fuera de código estricto desde ES2021 (NonOctalDecimalEscapeSequence, §12.9.4)",
    ),
    (
        "fail/98204d734f8c72b3.js",
        "campos de clase, válidos desde ES2022 (§15.7)",
    ),
    (
        "fail/ef81b93cf9bdb4ec.js",
        "campos de clase, válidos desde ES2022 (§15.7)",
    ),
    (
        "fail/c060a3014ad24dfd.js",
        "U+2B81E es ID_Start en la UCD actual",
    ),
    (
        "fail/a8beb1480f385441.js",
        "`func() = 4` fuera de código estricto es un destino ~web-compat~: ReferenceError al ejecutar, no SyntaxError (Annex B, Runtime Errors for Function Call Assignment Targets)",
    ),
    (
        "fail/e3fbcf63d7e43ead.js",
        "`for (var x = 1 in o)` es válido fuera de código estricto (B.3.5)",
    ),
    (
        "early/0f5f47108da5c34e.js",
        "B.3.4 actual: `var` puede redeclarar un parámetro de catch simple, también en for-of (test262 annexB/language/statements/try/catch-redeclared-for-of-var.js)",
    ),
    (
        "early/12a74c60f52a60de.js",
        "declaraciones de función duplicadas en un bloque no estricto (B.3.2.4)",
    ),
    (
        "early/1aff49273f3e3a98.js",
        "declaraciones de función duplicadas en un bloque no estricto (B.3.2.4)",
    ),
    (
        "early/be7329119eaa3d47.js",
        "declaraciones de función duplicadas en un bloque no estricto (B.3.2.4)",
    ),
    (
        "early/ec31fa5e521c5df4.js",
        "declaraciones de función duplicadas en un bloque no estricto (B.3.2.4)",
    ),
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/test262-parser-tests")
}

fn parse(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    let src = String::from_utf8_lossy(&bytes);
    let name = path.file_name().unwrap().to_string_lossy();
    // Pila grande (ver `test262_parser_tests`): en debug los frames del parser son grandes.
    let opts = ParseOptions {
        max_stack: 192 << 20,
        ..ParseOptions::default()
    };
    let program = if name.ends_with(".module.js") {
        parse_module(&src, &opts)
    } else {
        parse_script(&src, &opts)
    }
    .map_err(|e| e.to_string())?;
    let json = to_json(
        &program,
        &Options {
            locations: false,
            raw: false,
        },
    );
    let mut value: serde_json::Value = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    canonicalize(&mut value);
    Ok(value.to_string())
}

/// Lo que el corpus considera equivalente: los paréntesis de agrupación anidan las
/// secuencias (`(a, b), c`) y las claves no computadas pueden escribirse como identificador,
/// string o número (`{"a": 1}` y `{a: 1}`).
fn canonicalize(v: &mut serde_json::Value) {
    use serde_json::Value;
    match v {
        Value::Array(items) => items.iter_mut().for_each(canonicalize),
        Value::Object(map) => {
            for (_, child) in map.iter_mut() {
                canonicalize(child);
            }
            let ty = map
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            if ty == "SequenceExpression"
                && let Some(Value::Array(exprs)) = map.get_mut("expressions")
            {
                let mut flat = Vec::new();
                for e in exprs.drain(..) {
                    match e {
                        Value::Object(mut inner)
                            if inner.get("type").and_then(Value::as_str)
                                == Some("SequenceExpression") =>
                        {
                            if let Some(Value::Array(xs)) = inner.remove("expressions") {
                                flat.extend(xs);
                            }
                        }
                        other => flat.push(other),
                    }
                }
                *exprs = flat;
            }
            // La diferencia `' '` / `(' ')` solo es si la sentencia es una directiva.
            map.remove("directive");
            let has_key = matches!(
                ty.as_str(),
                "Property" | "MethodDefinition" | "PropertyDefinition"
            );
            if has_key
                && map.get("computed") == Some(&Value::Bool(false))
                && let Some(key) = map.get_mut("key")
            {
                let name = match key.get("type").and_then(Value::as_str) {
                    Some("Identifier") => key.get("name").cloned(),
                    Some("Literal") => match key.get("value") {
                        Some(Value::Number(n)) => {
                            n.as_f64().map(|f| Value::String(js_number_to_string(f)))
                        }
                        Some(Value::Null) => Some(Value::String("Infinity".into())),
                        other => other.cloned(),
                    },
                    _ => None,
                };
                if let Some(name) = name {
                    *key = serde_json::json!({"type": "Key", "name": name});
                }
            }
        }
        _ => {}
    }
}

/// Number::toString para las claves numéricas habituales del corpus.
fn js_number_to_string(f: f64) -> String {
    if f == f.trunc() && f.abs() < 1e21 {
        format!("{f:.0}")
    } else {
        format!("{f}")
    }
}

#[derive(Default)]
struct Tally {
    passed: usize,
    stale: usize,
    failed: Vec<String>,
}

fn files(dir: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(root().join(dir))
        .unwrap_or_else(|e| {
            panic!("falta tests/test262-parser-tests/{dir} ({e}): git submodule update --init")
        })
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "js"))
        .collect();
    v.sort();
    v
}

fn name(path: &Path) -> String {
    path.file_name().unwrap().to_string_lossy().into_owned()
}

fn stale(dir: &str, path: &Path) -> bool {
    let key = format!("{dir}/{}", path.file_name().unwrap().to_string_lossy());
    STALE.iter().any(|(k, _)| *k == key)
}

#[test]
fn test262_parser_tests() {
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(run)
        .expect("hilo")
        .join()
        .expect("el harness no debe entrar en pánico");
}

fn run() {
    let mut rows = Vec::new();
    // pass + pass-explicit.
    let mut t = Tally::default();
    for path in files("pass") {
        if stale("pass", &path) {
            t.stale += 1;
            continue;
        }
        let explicit = root().join("pass-explicit").join(path.file_name().unwrap());
        match (parse(&path), parse(&explicit)) {
            (Ok(a), Ok(b)) if a == b => t.passed += 1,
            (Ok(a), Ok(b)) => {
                let i = a
                    .bytes()
                    .zip(b.bytes())
                    .position(|(x, y)| x != y)
                    .unwrap_or(0);
                let from = i.saturating_sub(60);
                t.failed.push(format!(
                    "pass/{}: AST distinto de pass-explicit
    {}
    {}",
                    name(&path),
                    a.get(from..(i + 60).min(a.len())).unwrap_or(""),
                    b.get(from..(i + 60).min(b.len())).unwrap_or("")
                ));
            }
            (Err(e), _) => t.failed.push(format!("pass/{}: {e}", name(&path))),
            (_, Err(e)) => t
                .failed
                .push(format!("pass-explicit/{}: {e}", name(&explicit))),
        }
    }
    rows.push(("pass + pass-explicit", t));
    for dir in ["fail", "early"] {
        let mut t = Tally::default();
        for path in files(dir) {
            if stale(dir, &path) {
                t.stale += 1;
                continue;
            }
            match parse(&path) {
                Err(_) => t.passed += 1,
                Ok(_) => t.failed.push(format!("{dir}/{}: se aceptó", name(&path))),
            }
        }
        rows.push((dir, t));
    }
    println!(
        "{:<22} {:>7} {:>6} {:>7}",
        "carpeta", "passed", "stale", "failed"
    );
    let mut failures = Vec::new();
    for (name, t) in &rows {
        println!(
            "{:<22} {:>7} {:>6} {:>7}",
            name,
            t.passed,
            t.stale,
            t.failed.len()
        );
        failures.extend(t.failed.iter().cloned());
    }
    for f in failures.iter().take(200) {
        println!("  {f}");
    }
    assert!(failures.is_empty(), "{} fallos", failures.len());
}
