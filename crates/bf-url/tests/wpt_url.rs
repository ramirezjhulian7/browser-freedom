//! WPT `url/resources`: urltestdata.json, setters_tests.json, toascii.json y
//! percent-encoding.json, con la misma lógica que los `.js` de WPT. Resumen por archivo
//! `passed / stale / skipped / failed` (`-- --nocapture`).
//!
//! - `stale`: casos cuya expectativa contradice el spec vigente (lista `STALE`, con motivo).
//! - `skipped`: casos de `percent-encoding.json` en encodings multibyte (roadmap).

use std::path::PathBuf;

use bf_url::Url;
use serde_json::Value;

fn resources() -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/wpt/url/resources");
    dir.is_dir().then_some(dir)
}

fn load(name: &str) -> Option<Value> {
    let text = std::fs::read_to_string(resources()?.join(name)).ok()?;
    serde_json::from_str(&text).ok()
}

/// (archivo, identificador del caso, motivo).
const STALE: &[(&str, &str, &str)] = &[];

#[derive(Default)]
struct Tally {
    passed: usize,
    stale: usize,
    skipped: usize,
    failed: Vec<String>,
}

impl Tally {
    fn check(&mut self, file: &str, id: &str, ok: bool, detail: impl FnOnce() -> String) {
        if ok {
            self.passed += 1;
        } else if STALE.iter().any(|(f, i, _)| *f == file && *i == id) {
            self.stale += 1;
        } else {
            self.failed.push(format!("{id}: {}", detail()));
        }
    }
}

fn getter(url: &Url, name: &str) -> String {
    match name {
        "href" => url.href(),
        "origin" => url.origin().to_string(),
        "protocol" => url.protocol(),
        "username" => url.username.clone(),
        "password" => url.password.clone(),
        "host" => url.host_string(),
        "hostname" => url.hostname(),
        "port" => url.port_string(),
        "pathname" => url.path_string(),
        "search" => url.search(),
        "hash" => url.hash(),
        _ => panic!("getter desconocido {name}"),
    }
}

fn setter(url: &mut Url, name: &str, value: &str) {
    match name {
        "href" => {
            let _ = url.set_href(value);
        }
        "protocol" => url.set_protocol(value),
        "username" => url.set_username(value),
        "password" => url.set_password(value),
        "host" => url.set_host(value),
        "hostname" => url.set_hostname(value),
        "port" => url.set_port(value),
        "pathname" => url.set_pathname(value),
        "search" => url.set_search(value),
        "hash" => url.set_hash(value),
        _ => panic!("setter desconocido {name}"),
    }
}

const FIELDS: &[&str] = &[
    "href", "origin", "protocol", "username", "password", "host", "hostname", "port", "pathname",
    "search", "hash",
];

fn urltestdata(t: &mut Tally) {
    let Some(Value::Array(cases)) = load("urltestdata.json") else {
        return;
    };
    for case in cases.iter().filter_map(Value::as_object) {
        let input = case["input"].as_str().unwrap_or_default();
        let base_str = case.get("base").and_then(Value::as_str);
        let id = format!("{input:?} base {base_str:?}");
        let base = match base_str {
            Some(b) => match Url::parse(b, None) {
                Ok(u) => Some(u),
                Err(_) => {
                    // Con una base inválida, el test espera falla.
                    t.check("urltestdata", &id, case.get("failure").is_some(), || {
                        "base inválida".into()
                    });
                    continue;
                }
            },
            None => None,
        };
        let result = Url::parse(input, base.as_ref());
        if case.get("failure").and_then(Value::as_bool) == Some(true) {
            t.check("urltestdata", &id, result.is_err(), || {
                format!(
                    "debía fallar, dio {}",
                    result.as_ref().map(Url::href).unwrap_or_default()
                )
            });
            continue;
        }
        let Ok(url) = result else {
            t.check("urltestdata", &id, false, || "falló el parseo".into());
            continue;
        };
        let mut diffs = Vec::new();
        for f in FIELDS {
            if let Some(expected) = case.get(*f).and_then(Value::as_str) {
                let got = getter(&url, f);
                if got != expected {
                    diffs.push(format!("{f}: {got:?} ≠ {expected:?}"));
                }
            }
        }
        t.check("urltestdata", &id, diffs.is_empty(), || diffs.join(", "));
    }
}

fn setters(t: &mut Tally) {
    let Some(Value::Object(groups)) = load("setters_tests.json") else {
        return;
    };
    for (name, cases) in &groups {
        if name == "comment" {
            continue;
        }
        for case in cases.as_array().into_iter().flatten() {
            let href = case["href"].as_str().unwrap_or_default();
            let value = case["new_value"].as_str().unwrap_or_default();
            let id = format!("{name} {href:?} ← {value:?}");
            let Ok(mut url) = Url::parse(href, None) else {
                t.check("setters", &id, false, || "href inicial inválido".into());
                continue;
            };
            setter(&mut url, name, value);
            let mut diffs = Vec::new();
            for (f, expected) in case["expected"].as_object().into_iter().flatten() {
                let got = getter(&url, f);
                if Some(got.as_str()) != expected.as_str() {
                    diffs.push(format!("{f}: {got:?} ≠ {expected}"));
                }
            }
            t.check("setters", &id, diffs.is_empty(), || diffs.join(", "));
        }
    }
}

fn toascii(t: &mut Tally) {
    let Some(Value::Array(cases)) = load("toascii.json") else {
        return;
    };
    for case in cases.iter().filter_map(Value::as_object) {
        let input = case["input"].as_str().unwrap_or_default();
        let output = case["output"].as_str();
        let id = format!("{input:?}");
        // new URL("https://" + input + "/x")
        let parsed = Url::parse(&format!("https://{input}/x"), None);
        let ok = match (output, &parsed) {
            (None, r) => r.is_err(),
            (Some(o), Ok(u)) => {
                u.host_string() == o
                    && u.hostname() == o
                    && u.path_string() == "/x"
                    && u.href() == format!("https://{o}/x")
            }
            (Some(_), Err(_)) => false,
        };
        t.check("toascii", &id, ok, || {
            format!("{:?} ≠ {output:?}", parsed.as_ref().map(Url::hostname))
        });
        // url.host = input / url.hostname = input sobre "https://x/x".
        for setter_name in ["host", "hostname"] {
            let mut u = Url::parse("https://x/x", None).expect("válida");
            setter(&mut u, setter_name, input);
            let got = getter(&u, setter_name);
            let expected = output.unwrap_or("x");
            t.check(
                "toascii",
                &format!("{id} .{setter_name}"),
                got == expected,
                || format!("{got:?} ≠ {expected:?}"),
            );
        }
    }
}

fn percent_encoding(t: &mut Tally) {
    let Some(Value::Array(cases)) = load("percent-encoding.json") else {
        return;
    };
    for case in cases.iter().filter_map(Value::as_object) {
        let input = case["input"].as_str().unwrap_or_default();
        let outputs = case["output"].as_object().expect("output");
        let utf8 = outputs["utf-8"].as_str().unwrap_or_default();
        for (encoding, expected) in outputs {
            let id = format!("{input:?} {encoding}");
            let encoder =
                bf_infra::encoding::Encoding::for_label(encoding).filter(|e| e.can_encode());
            let Some(encoder) = encoder else {
                t.skipped += 1;
                continue;
            };
            let href = format!("https://doesnotmatter.invalid/?{input}#{input}");
            let encode = |c: char| encoder.encode_char(c);
            let url = Url::parse_with_encoding(&href, None, &encode);
            let ok = url.as_ref().is_ok_and(|u| {
                u.hash() == format!("#{utf8}")
                    && u.search() == format!("?{}", expected.as_str().unwrap_or_default())
            });
            t.check("percent-encoding", &id, ok, || {
                format!("{:?}", url.as_ref().map(|u| (u.search(), u.hash())))
            });
        }
    }
}

/// (archivo, función que lo corre).
type Run = (&'static str, fn(&mut Tally));

#[test]
fn wpt_url() {
    if resources().is_none() {
        eprintln!("aviso: falta tests/wpt/url/resources (tools/fetch-wpt); se salta");
        return;
    }
    let mut total = Tally::default();
    println!(
        "{:<24}{:>8}{:>8}{:>9}{:>8}",
        "archivo", "passed", "stale", "skipped", "failed"
    );
    let runs: [Run; 4] = [
        ("urltestdata.json", urltestdata),
        ("setters_tests.json", setters),
        ("toascii.json", toascii),
        ("percent-encoding.json", percent_encoding),
    ];
    let mut failures = Vec::new();
    for (name, run) in runs {
        let mut t = Tally::default();
        run(&mut t);
        println!(
            "{name:<24}{:>8}{:>8}{:>9}{:>8}",
            t.passed,
            t.stale,
            t.skipped,
            t.failed.len()
        );
        total.passed += t.passed;
        total.stale += t.stale;
        total.skipped += t.skipped;
        failures.extend(t.failed.iter().map(|f| format!("{name}: {f}")));
    }
    println!(
        "{:<24}{:>8}{:>8}{:>9}{:>8}",
        "TOTAL",
        total.passed,
        total.stale,
        total.skipped,
        failures.len()
    );
    for f in failures.iter().take(60) {
        println!("  {f}");
    }
    assert!(failures.is_empty(), "{} casos fallaron", failures.len());
}
