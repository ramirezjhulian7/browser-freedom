//! Corre `tests/css-parsing-tests` (formato en su README.rst).
//!
//! La suite se escribió contra el CR de 2021 de CSS Syntax 3; el parser sigue el Editor's
//! Draft actual. Las diferencias de representación se normalizan así:
//! - En las listas de reglas/declaraciones se omiten los `["error", ...]` de construcciones
//!   descartadas: el spec actual "no devuelve nada" para ellas y no hay nodo de error en el
//!   resultado.
//! - Los números se comparan con tolerancia relativa.
//!
//! Los tests cuya expectativa contradice el spec actual están en `UPSTREAM_STALE`.

use std::fs;
use std::path::PathBuf;

use bf_css::syntax::{
    self, BlockContents, BlockKind, ComponentValue, Declaration, NestedItem, Rule, SyntaxError,
    TokenKind,
};
use serde_json::{Value, json};

const UNICODE_RANGE: &str = "espera <unicode-range-token> en una lista de component values; el ED actual solo lo produce con 'unicode ranges allowed' (descriptor unicode-range, §5.5.11)";
const MATCH_TOKENS: &str = "espera tokens <include-match>/<dash-match>/<prefix-match>/<suffix-match>/<substring-match>/<column>, eliminados del spec (ahora son <delim-token>s)";
const DECL_WHITESPACE: &str =
    "el ED actual descarta el whitespace tras ':' y al final del valor de una declaración (§5.5.6)";
const DECL_LIST_AT_RULES: &str =
    "el ED actual 'parse a list of declarations' consume y descarta las at-rules (§5.4.5)";
const DECL_STOPS_AT_SEMICOLON: &str =
    "el ED actual consume el valor hasta el ';' (stop token); lo que sigue es extra-input (§5.4.7)";

/// (archivo, input, motivo).
const UPSTREAM_STALE: &[(&str, &str, &str)] = &[
    (
        "component_value_list.json",
        "\\- red0 -red --red -\\-red\\ blue 0red -0red \u{0}red _Red .red rêd r\\êd \u{7f}\u{80}\u{81}",
        "U+0080/U+0081 ya no son non-ASCII ident code points (el ED actual usa la lista de nombres de custom elements, §4.2)",
    ),
    (
        "component_value_list.json",
        "u+1 U+10 U+100 U+1000 U+10000 U+100000 U+1000000",
        UNICODE_RANGE,
    ),
    (
        "component_value_list.json",
        "u+? u+1? U+10? U+100? U+1000? U+10000? U+100000?",
        UNICODE_RANGE,
    ),
    (
        "component_value_list.json",
        "u+?? U+1?? U+10?? U+100?? U+1000?? U+10000??",
        UNICODE_RANGE,
    ),
    (
        "component_value_list.json",
        "u+??? U+1??? U+10??? U+100??? U+1000???",
        UNICODE_RANGE,
    ),
    (
        "component_value_list.json",
        "u+???? U+1???? U+10???? U+100????",
        UNICODE_RANGE,
    ),
    (
        "component_value_list.json",
        "u+????? U+1????? U+10?????",
        UNICODE_RANGE,
    ),
    (
        "component_value_list.json",
        "u+?????? U+1??????",
        UNICODE_RANGE,
    ),
    (
        "component_value_list.json",
        "u+1-2 U+100000-2 U+1000000-2 U+10-200000",
        UNICODE_RANGE,
    ),
    (
        "component_value_list.json",
        "ù+12 Ü+12 u +12 U+ 12 U+12 - 20 U+1?2 U+1?-50",
        UNICODE_RANGE,
    ),
    (
        "component_value_list.json",
        "~=|=^=$=*=||<!------> |/**/| ~/**/=",
        MATCH_TOKENS,
    ),
    (
        "component_value_list.json",
        "a:not([href^=http\\:],  [href ^=\t'https\\:'\n]) { color: rgba(0%, 100%, 50%); }",
        MATCH_TOKENS,
    ),
    (
        "declaration_list.json",
        "@import 'foo.css'; a:b; @import 'bar.css'",
        DECL_LIST_AT_RULES,
    ),
    (
        "declaration_list.json",
        "@media screen { div{;}} a:b;; @media print{div{",
        DECL_LIST_AT_RULES,
    ),
    (
        "declaration_list.json",
        "@ media screen { div{;}} a:b;; @media print{div{",
        DECL_LIST_AT_RULES,
    ),
    ("one_declaration.json", "\n/**/ foo: ", DECL_WHITESPACE),
    (
        "one_declaration.json",
        "foo: 9000  !Important",
        DECL_WHITESPACE,
    ),
    (
        "one_declaration.json",
        "foo: 9000  ! /**/\t IMPORTant /**/\u{c}",
        DECL_WHITESPACE,
    ),
    (
        "one_declaration.json",
        "foo: 9000  /* Dotted capital I */!İmportant",
        DECL_WHITESPACE,
    ),
    (
        "one_declaration.json",
        "foo: 9000  !important!",
        DECL_WHITESPACE,
    ),
    (
        "one_declaration.json",
        "foo: 9000  important",
        DECL_WHITESPACE,
    ),
    ("one_declaration.json", "foo:;", DECL_STOPS_AT_SEMICOLON),
    (
        "one_declaration.json",
        "foo:;bar:;",
        DECL_STOPS_AT_SEMICOLON,
    ),
    (
        "rule_list.json",
        " /* CDO/CDC are not special */ <!-- --> {",
        "el ED actual no tiene 'parse a list of rules' (top-level flag); 'parse a stylesheet's contents' descarta <CDO>/<CDC> (§5.5.1)",
    ),
];

fn cv_json(cv: &ComponentValue) -> Value {
    match cv {
        ComponentValue::Token(t) => token_json(&t.kind),
        ComponentValue::Function(f) => {
            let mut v = vec![json!("function"), json!(f.name)];
            v.extend(f.value.iter().map(cv_json));
            Value::Array(v)
        }
        ComponentValue::Block(b) => {
            let name = match b.kind {
                BlockKind::Curly => "{}",
                BlockKind::Square => "[]",
                BlockKind::Paren => "()",
            };
            let mut v = vec![json!(name)];
            v.extend(b.value.iter().map(cv_json));
            Value::Array(v)
        }
    }
}

fn numeric_type(integer: bool) -> &'static str {
    if integer { "integer" } else { "number" }
}

fn token_json(kind: &TokenKind) -> Value {
    match kind {
        TokenKind::Ident(v) => json!(["ident", v]),
        TokenKind::Function(v) => json!(["function", v]),
        TokenKind::AtKeyword(v) => json!(["at-keyword", v]),
        TokenKind::Hash { value, id } => {
            json!(["hash", value, if *id { "id" } else { "unrestricted" }])
        }
        TokenKind::String(v) => json!(["string", v]),
        TokenKind::BadString => json!(["error", "bad-string"]),
        TokenKind::Url(v) => json!(["url", v]),
        TokenKind::BadUrl => json!(["error", "bad-url"]),
        TokenKind::Delim(c) => json!(c.to_string()),
        TokenKind::Number(n) => json!(["number", n.repr, n.value, numeric_type(n.integer)]),
        TokenKind::Percentage(n) => {
            json!(["percentage", n.repr, n.value, numeric_type(n.integer)])
        }
        TokenKind::Dimension(n, unit) => {
            json!(["dimension", n.repr, n.value, numeric_type(n.integer), unit])
        }
        TokenKind::UnicodeRange { start, end } => json!(["unicode-range", start, end]),
        TokenKind::Whitespace => json!(" "),
        TokenKind::Cdo => json!("<!--"),
        TokenKind::Cdc => json!("-->"),
        TokenKind::Colon => json!(":"),
        TokenKind::Semicolon => json!(";"),
        TokenKind::Comma => json!(","),
        TokenKind::RightBracket => json!(["error", "]"]),
        TokenKind::RightParen => json!(["error", ")"]),
        TokenKind::RightBrace => json!(["error", "}"]),
        TokenKind::LeftBracket | TokenKind::LeftParen | TokenKind::LeftBrace => {
            unreachable!("token no preservado")
        }
    }
}

fn values_json(values: &[ComponentValue]) -> Value {
    Value::Array(values.iter().map(cv_json).collect())
}

fn decl_json(d: &Declaration) -> Value {
    json!(["declaration", d.name, values_json(&d.value), d.important])
}

fn rule_json(rule: &Rule) -> Value {
    match rule {
        Rule::Qualified(q) => json!([
            "qualified rule",
            values_json(&q.prelude),
            values_json(&q.block.values)
        ]),
        Rule::At(a) => json!([
            "at-rule",
            a.name,
            values_json(&a.prelude),
            a.block
                .as_ref()
                .map_or(Value::Null, |b| values_json(&b.values))
        ]),
    }
}

fn contents_json(contents: &BlockContents) -> Value {
    let mut out: Vec<Value> = contents.declarations.iter().map(decl_json).collect();
    for item in &contents.rules {
        match item {
            NestedItem::Rule(rule) => out.push(rule_json(rule)),
            NestedItem::Declarations(decls) => out.extend(decls.iter().map(decl_json)),
        }
    }
    Value::Array(out)
}

fn error_json(e: SyntaxError) -> Value {
    let kind = match e {
        SyntaxError::Empty => "empty",
        SyntaxError::Invalid => "invalid",
        SyntaxError::ExtraInput => "extra-input",
    };
    json!(["error", kind])
}

/// Resultado del parser en la forma JSON de la suite.
fn run(file: &str, input: &str) -> Value {
    match file {
        "component_value_list.json" => values_json(&syntax::parse_component_value_list(input)),
        "one_component_value.json" => {
            syntax::parse_component_value(input).map_or_else(error_json, |v| cv_json(&v))
        }
        "declaration_list.json" => Value::Array(
            syntax::parse_declaration_list(input)
                .iter()
                .map(decl_json)
                .collect(),
        ),
        "blocks_contents.json" => contents_json(&syntax::parse_block_contents(input)),
        "one_declaration.json" => {
            syntax::parse_declaration(input).map_or_else(error_json, |d| decl_json(&d))
        }
        "one_rule.json" => syntax::parse_rule(input).map_or_else(error_json, |r| rule_json(&r)),
        "rule_list.json" => Value::Array(
            syntax::parse_rule_list(input)
                .iter()
                .map(rule_json)
                .collect(),
        ),
        "stylesheet.json" => Value::Array(
            syntax::parse_stylesheet(input)
                .iter()
                .map(rule_json)
                .collect(),
        ),
        "An+B.json" => {
            let values = syntax::parse_component_value_list(input);
            syntax::parse_anb(&values).map_or(Value::Null, |(a, b)| json!([a, b]))
        }
        _ => unreachable!(),
    }
}

/// Quita los marcadores `["error", "eof-in-string"]` / `["error", "eof-in-url"]`: la suite
/// representa así el parse error de llegar a EOF dentro de un string o url, pero el token que
/// produce el spec es el `<string-token>`/`<url-token>` sin ningún token adicional.
fn drop_eof_markers(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .filter(|item| {
                    !matches!(
                        item.as_array().map(Vec::as_slice),
                        Some([Value::String(e), Value::String(k)])
                            if e == "error" && (k == "eof-in-string" || k == "eof-in-url")
                    )
                })
                .map(drop_eof_markers)
                .collect(),
        ),
        other => other,
    }
}

/// Quita de una lista de resultados los nodos de error de construcciones descartadas.
fn drop_discarded_errors(file: &str, expected: Value) -> Value {
    let expected = drop_eof_markers(expected);
    let is_list = matches!(
        file,
        "declaration_list.json" | "blocks_contents.json" | "rule_list.json" | "stylesheet.json"
    );
    match expected {
        Value::Array(items) if is_list => Value::Array(
            items
                .into_iter()
                .filter(|item| {
                    !matches!(
                        item.as_array().map(Vec::as_slice),
                        Some([Value::String(e), Value::String(k)])
                            if e == "error" && matches!(k.as_str(), "invalid" | "empty" | "extra-input")
                    )
                })
                .collect(),
        ),
        other => other,
    }
}

fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (
                x.as_f64().unwrap_or(f64::NAN),
                y.as_f64().unwrap_or(f64::NAN),
            );
            x == y || (x - y).abs() <= 1e-6 * x.abs().max(y.abs())
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same(x, y))
        }
        _ => a == b,
    }
}

const FILES: &[&str] = &[
    "component_value_list.json",
    "one_component_value.json",
    "declaration_list.json",
    "blocks_contents.json",
    "one_declaration.json",
    "one_rule.json",
    "rule_list.json",
    "stylesheet.json",
    "An+B.json",
];

#[test]
fn css_parsing_tests() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/css-parsing-tests");
    let mut failures = Vec::new();
    let mut used_stale = 0;
    let (mut total_passed, mut total_stale, mut total_failed) = (0, 0, 0);
    println!(
        "{:<28} {:>7} {:>6} {:>7}",
        "archivo", "passed", "stale", "failed"
    );
    for file in FILES {
        let path = dir.join(file);
        let text = fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "no se pudo leer {}: {e}. ¿Falta `git submodule update --init`?",
                path.display()
            )
        });
        let data: Vec<Value> = serde_json::from_str(&text).unwrap();
        let (mut passed, mut stale, mut failed) = (0, 0, 0);
        for pair in data.chunks(2) {
            let input = pair[0].as_str().unwrap();
            let expected = drop_discarded_errors(file, pair[1].clone());
            let actual = run(file, input);
            let ok = same(&actual, &expected);
            let is_stale = UPSTREAM_STALE
                .iter()
                .any(|(f, i, _)| f == file && *i == input);
            if is_stale {
                used_stale += 1;
                if ok {
                    failed += 1;
                    failures.push(format!(
                        "{file} {input:?}: ya pasa, quitarlo de UPSTREAM_STALE"
                    ));
                } else {
                    stale += 1;
                }
            } else if ok {
                passed += 1;
            } else {
                failed += 1;
                failures.push(format!(
                    "FALLO {file} {input:?}\n  esperado: {expected}\n  obtenido: {actual}"
                ));
            }
        }
        println!("{file:<28} {passed:>7} {stale:>6} {failed:>7}");
        total_passed += passed;
        total_stale += stale;
        total_failed += failed;
    }
    println!(
        "{:<28} {total_passed:>7} {total_stale:>6} {total_failed:>7}",
        "TOTAL"
    );
    for f in failures.iter().take(80) {
        println!("{f}");
    }
    assert_eq!(
        used_stale,
        UPSTREAM_STALE.len(),
        "hay entradas de UPSTREAM_STALE sin test"
    );
    assert_eq!(
        total_failed, 0,
        "{total_failed} tests de css-parsing-tests fallaron"
    );
}
