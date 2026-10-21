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

// E3: 1000 niveles de anidamiento se parsean y corren con la configuración del navegador
// (`bf-shell`: 64 MB de pila de JS en un hilo de 128 MB).
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "la profundidad se garantiza en release (en debug los frames son mucho más grandes)"
)]
fn thousand_levels_parse_and_run() {
    use bf_js::host::{DefaultHooks, Runtime};
    use bf_js::vm::AgentOptions;
    std::thread::Builder::new()
        .stack_size(128 << 20)
        .spawn(|| {
            let n = 1000;
            let cases = [
                format!("{}1{}", "(".repeat(n), ")".repeat(n)),
                format!("{}1{}", "[".repeat(n), "]".repeat(n)),
                format!("{}{}", "{".repeat(n), "}".repeat(n)),
                format!("{}1{}", "x={a:".repeat(n), "}".repeat(n)),
                format!("{}1{}", "(function(){return ".repeat(n), "})()".repeat(n)),
            ];
            let opts = ParseOptions {
                max_stack: 64 << 20,
                ..ParseOptions::default()
            };
            for src in &cases {
                parse_script(src, &opts)
                    .unwrap_or_else(|e| panic!("{}: {}", &src[..20], e.message));
                let mut rt = Runtime::with_options(
                    Box::new(DefaultHooks),
                    AgentOptions {
                        max_stack: 64 << 20,
                        ..AgentOptions::default()
                    },
                );
                let realm = rt.main_realm();
                rt.eval_script(realm, src, "depth.js")
                    .unwrap_or_else(|e| panic!("{}: {}", &src[..20], e.message));
            }
        })
        .expect("hilo")
        .join()
        .expect("sin desbordar la pila");
}

// E3: el frame del intérprete se copia en cada llamada; que no vuelva a crecer sin querer.
#[test]
fn frame_size() {
    let size = std::mem::size_of::<bf_js::vm::frame::Frame>();
    println!("size_of::<Frame>() = {size}");
    assert!(size <= 232, "Frame mide {size} bytes");
}
