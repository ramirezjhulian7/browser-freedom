//! El parser rechaza el anidamiento excesivo con un SyntaxError, sin desbordar la pila.

use bf_js::{ParseOptions, parse_script};

#[test]
fn deep_nesting_is_an_error_not_a_crash() {
    for (open, close) in [
        ("(", ")"),
        ("[", "]"),
        ("x={a:", "}"),
        ("!", ""),
        ("-", ""),
        ("a?b:", ""),
    ] {
        let src = format!("{}1{}", open.repeat(100_000), close.repeat(100_000));
        let err = parse_script(&src, &ParseOptions::default()).unwrap_err();
        assert!(err.message.contains("nesting"), "{open}: {}", err.message);
    }
    let blocks = format!("{}{}", "{".repeat(100_000), "}".repeat(100_000));
    assert!(parse_script(&blocks, &ParseOptions::default()).is_err());
    let re = format!("/{}a{}/", "(".repeat(100_000), ")".repeat(100_000));
    assert!(parse_script(&re, &ParseOptions::default()).is_err());
}

#[test]
fn reasonable_nesting_parses() {
    let src = format!("{}1{}", "(".repeat(15), ")".repeat(15));
    parse_script(&src, &ParseOptions::default()).unwrap();
}
