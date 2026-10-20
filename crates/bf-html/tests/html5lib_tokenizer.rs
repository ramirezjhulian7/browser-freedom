//! Corre las pruebas de tokenizador de html5lib-tests (`tests/html5lib-tests/tokenizer`).
//!
//! Formato: <https://github.com/html5lib/html5lib-tests/blob/master/tokenizer/README.md>.

use std::fs;
use std::path::PathBuf;

use bf_html::tokenizer::{State, Token, Tokenizer};
use serde_json::{Map, Value};

/// Archivos que no se corren.
const SKIPPED_FILES: &[(&str, &str)] = &[
    (
        "pendingSpecChanges.test",
        "su único test (`<!---- >`) contradice el comment end state actual",
    ),
    (
        "xmlViolation.test",
        "modo xmlViolation (coerción a infoset XML), que no se implementa",
    ),
];

/// Tests cuya expectativa contradice el spec actual. Cada entrada lleva justificación.
/// Se siguen ejecutando: si alguno pasa, la entrada sobra y el harness falla para avisarlo.
const UPSTREAM_STALE: &[(
    &str, /* archivo */
    &str, /* description */
    &str, /* motivo */
)] = &[
    (
        "domjs.test",
        "CR in bogus comment state",
        "espera unexpected-question-mark-instead-of-tag-name, eliminado del spec al introducir PIs (html5lib-tests c777c40 no actualizó domjs.test)",
    ),
    (
        "domjs.test",
        "CRLF in bogus comment state",
        "espera unexpected-question-mark-instead-of-tag-name, eliminado del spec al introducir PIs (html5lib-tests c777c40 no actualizó domjs.test)",
    ),
    (
        "domjs.test",
        "CRLFLF in bogus comment state",
        "espera unexpected-question-mark-instead-of-tag-name, eliminado del spec al introducir PIs (html5lib-tests c777c40 no actualizó domjs.test)",
    ),
];

#[derive(Default)]
struct Stats {
    passed: usize,
    /// Tests de `UPSTREAM_STALE` (fallan como se espera).
    stale: usize,
    /// Tests con surrogates sueltos, que no son representables en un `&str` de Rust.
    unrepresentable: usize,
    failed: usize,
}

#[test]
fn html5lib_tokenizer() {
    let dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/html5lib-tests/tokenizer");
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| {
            panic!(
                "no se pudo leer {}: {e}. ¿Falta `git submodule update --init`?",
                dir.display()
            )
        })
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "test"))
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "no hay archivos .test en {}",
        dir.display()
    );

    let mut failures = Vec::new();
    let mut total = Stats::default();
    println!(
        "{:<30} {:>7} {:>6} {:>16} {:>7}",
        "archivo", "passed", "stale", "unrepresentable", "failed"
    );
    for path in &files {
        let file_name = path.file_name().unwrap().to_str().unwrap();
        if let Some((_, reason)) = SKIPPED_FILES.iter().find(|(name, _)| *name == file_name) {
            println!("{file_name:<30} (archivo omitido: {reason})");
            continue;
        }
        let json: Value = serde_json::from_str(&fs::read_to_string(path).unwrap())
            .unwrap_or_else(|e| panic!("{file_name}: JSON inválido: {e}"));
        let mut stats = Stats::default();
        for test in json["tests"].as_array().unwrap() {
            run_test(file_name, test, &mut stats, &mut failures);
        }
        println!(
            "{file_name:<30} {:>7} {:>6} {:>16} {:>7}",
            stats.passed, stats.stale, stats.unrepresentable, stats.failed
        );
        total.passed += stats.passed;
        total.stale += stats.stale;
        total.unrepresentable += stats.unrepresentable;
        total.failed += stats.failed;
    }
    println!(
        "{:<30} {:>7} {:>6} {:>16} {:>7}",
        "TOTAL", total.passed, total.stale, total.unrepresentable, total.failed
    );

    for failure in failures.iter().take(50) {
        println!("\n{failure}");
    }
    assert_eq!(
        total.failed, 0,
        "{} tests de html5lib fallaron",
        total.failed
    );
}

fn run_test(file: &str, test: &Value, stats: &mut Stats, failures: &mut Vec<String>) {
    let description = test["description"].as_str().unwrap();
    let double_escaped = test["doubleEscaped"].as_bool().unwrap_or(false);
    let raw_input = test["input"].as_str().unwrap();
    let stale = UPSTREAM_STALE
        .iter()
        .any(|&(f, d, _)| f == file && d == description);
    let (Some(input), Some(expected)) = (
        unescape_if(double_escaped, raw_input),
        normalize_expected(&test["output"], double_escaped),
    ) else {
        stats.unrepresentable += 1;
        return;
    };

    let mut expected_errors: Option<Vec<String>> = test.get("errors").map(|errors| {
        errors
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["code"].as_str().unwrap().to_owned())
            .collect()
    });
    if let Some(errors) = &mut expected_errors {
        errors.sort();
    }

    let last_start_tag = test["lastStartTag"].as_str();
    let initial_states: Vec<&str> = match test.get("initialStates") {
        Some(states) => states
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap())
            .collect(),
        None => vec!["Data state"],
    };

    for state_name in initial_states {
        let (actual, mut actual_errors) = tokenize(&input, parse_state(state_name), last_start_tag);
        actual_errors.sort();
        let tokens_ok = actual == expected;
        let errors_ok = expected_errors.as_ref().is_none_or(|e| *e == actual_errors);
        if stale && !(tokens_ok && errors_ok) {
            stats.stale += 1;
        } else if stale {
            stats.failed += 1;
            failures.push(format!(
                "FALLO {file}: {description} [{state_name}]\n  el test ya pasa: quitarlo de UPSTREAM_STALE"
            ));
        } else if tokens_ok && errors_ok {
            stats.passed += 1;
        } else {
            stats.failed += 1;
            failures.push(format!(
                "FALLO {file}: {description} [{state_name}]\n  input:            {input:?}\n  esperado:         {}\n  obtenido:         {}\n  errores esperados: {expected_errors:?}\n  errores obtenidos: {actual_errors:?}",
                Value::Array(expected.clone()),
                Value::Array(actual),
            ));
        }
    }
}

fn parse_state(name: &str) -> State {
    match name {
        "Data state" => State::Data,
        "PLAINTEXT state" => State::Plaintext,
        "RCDATA state" => State::Rcdata,
        "RAWTEXT state" => State::Rawtext,
        "Script data state" => State::ScriptData,
        "CDATA section state" => State::CdataSection,
        other => panic!("estado inicial desconocido: {other}"),
    }
}

fn tokenize(input: &str, state: State, last_start_tag: Option<&str>) -> (Vec<Value>, Vec<String>) {
    let mut tokenizer = Tokenizer::new(input);
    tokenizer.set_state(state);
    if let Some(name) = last_start_tag {
        tokenizer.set_last_start_tag(name);
    }
    let mut out = Vec::new();
    while let Some(token) = tokenizer.next_token() {
        let value = match token {
            Token::Character(c) => {
                Value::from(vec![Value::from("Character"), Value::from(c.to_string())])
            }
            Token::Doctype(d) => Value::from(vec![
                Value::from("DOCTYPE"),
                Value::from(d.name),
                Value::from(d.public_id),
                Value::from(d.system_id),
                Value::from(!d.force_quirks),
            ]),
            Token::StartTag(tag) => {
                let attrs: Map<String, Value> = tag
                    .attrs
                    .into_iter()
                    .map(|(k, v)| (k, Value::from(v)))
                    .collect();
                let mut v = vec![
                    Value::from("StartTag"),
                    Value::from(tag.name),
                    Value::Object(attrs),
                ];
                if tag.self_closing {
                    v.push(Value::from(true));
                }
                Value::from(v)
            }
            Token::EndTag(tag) => Value::from(vec![Value::from("EndTag"), Value::from(tag.name)]),
            Token::Comment(data) => Value::from(vec![Value::from("Comment"), Value::from(data)]),
            Token::ProcessingInstruction(pi) => Value::from(vec![
                Value::from("ProcessingInstruction"),
                Value::from(pi.target),
                Value::from(pi.data),
            ]),
            Token::Eof => continue,
        };
        out.push(value);
    }
    let errors = tokenizer
        .errors()
        .iter()
        .map(|e| e.code.to_owned())
        .collect();
    (coalesce_characters(out), errors)
}

/// Des-escapa el output esperado (si aplica) y concatena los `Character` adyacentes.
/// Devuelve `None` si contiene surrogates sueltos.
fn normalize_expected(output: &Value, double_escaped: bool) -> Option<Vec<Value>> {
    fn unescape_value(v: &Value, double_escaped: bool) -> Option<Value> {
        Some(match v {
            Value::String(s) => Value::String(unescape_if(double_escaped, s)?),
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item| unescape_value(item, double_escaped))
                    .collect::<Option<_>>()?,
            ),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| {
                        Some((
                            unescape_if(double_escaped, k)?,
                            unescape_value(v, double_escaped)?,
                        ))
                    })
                    .collect::<Option<_>>()?,
            ),
            other => other.clone(),
        })
    }
    let tokens = output
        .as_array()
        .unwrap()
        .iter()
        .map(|t| unescape_value(t, double_escaped))
        .collect::<Option<Vec<_>>>()?;
    Some(coalesce_characters(tokens))
}

fn coalesce_characters(tokens: Vec<Value>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::with_capacity(tokens.len());
    for token in tokens {
        if token[0] == "Character"
            && let Some(last) = out.last_mut().filter(|last| last[0] == "Character")
        {
            let joined = format!(
                "{}{}",
                last[1].as_str().unwrap(),
                token[1].as_str().unwrap()
            );
            last[1] = Value::String(joined);
            continue;
        }
        out.push(token);
    }
    out
}

fn unescape_if(double_escaped: bool, s: &str) -> Option<String> {
    if double_escaped {
        unescape(s)
    } else {
        Some(s.to_owned())
    }
}

/// Convierte cada secuencia `\uHHHH` en su code point; combina pares de surrogates.
/// Devuelve `None` si hay un surrogate suelto.
fn unescape(s: &str) -> Option<String> {
    let mut units: Vec<u16> = Vec::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("\\u") {
        units.extend(rest[..i].encode_utf16());
        let hex = &rest[i + 2..i + 6];
        units.push(u16::from_str_radix(hex, 16).expect("\\u inválido en test doubleEscaped"));
        rest = &rest[i + 6..];
    }
    units.extend(rest.encode_utf16());
    String::from_utf16(&units).ok()
}
