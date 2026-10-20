//! `bf-js dump-ast <archivo.js> [--module] [--check] [--no-locations]`: imprime el AST como
//! JSON ESTree; con `--check` solo valida (código de salida 0 si parsea, 1 si no).
//!
//! `bf-js run <archivo.js>`: ejecuta un script con `print` (y `$262`); una excepción no
//! atrapada se imprime en stderr y sale con código 1.

use std::process::ExitCode;

use bf_js::estree::{Options, to_json};
use bf_js::vm::{Agent, AgentOptions, Throw};
use bf_js::{ParseOptions, parse_module, parse_script};

const USAGE: &str = "uso: bf-js dump-ast <archivo.js> [--module] [--check] [--no-locations]\n     bf-js run <archivo.js>";

fn main() -> ExitCode {
    // El parser y la VM corren en un hilo con pila grande: el anidamiento permitido depende
    // de ella.
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(run)
        .expect("hilo")
        .join()
        .unwrap_or(ExitCode::FAILURE)
}

fn read(path: &str) -> Option<String> {
    match std::fs::read(path) {
        Ok(b) => Some(String::from_utf8_lossy(&b).into_owned()),
        Err(e) => {
            eprintln!("no se pudo leer {path}: {e}");
            None
        }
    }
}

fn run() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [cmd, path, flags @ ..] = args.as_slice() else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    match cmd.as_str() {
        "dump-ast" => dump_ast(path, flags),
        "run" => {
            if flags.iter().any(|f| f == "--module") {
                eprintln!("--module: los módulos llegan en el paso 5");
                return ExitCode::FAILURE;
            }
            run_script(path)
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn run_script(path: &str) -> ExitCode {
    let Some(src) = read(path) else {
        return ExitCode::FAILURE;
    };
    let mut agent = Agent::new(AgentOptions {
        max_stack: 192 << 20,
        ..AgentOptions::default()
    });
    agent.reset_stack_base();
    let realm = agent.current_realm;
    bf_js::builtins::host::install_test262(&mut agent, realm);
    match agent.eval_script(&src, path) {
        Ok(_) => ExitCode::SUCCESS,
        Err(Throw::Value(v)) => {
            let msg = describe_exception(&mut agent, &v);
            eprintln!("Uncaught {msg}");
            ExitCode::FAILURE
        }
        Err(Throw::Terminate(r)) => {
            eprintln!("terminado: {r}");
            ExitCode::FAILURE
        }
    }
}

/// `String(e)` y, si tiene, la propiedad `stack`.
fn describe_exception(agent: &mut Agent, v: &bf_js::value::Value) -> String {
    let s = match agent.to_string(v) {
        Ok(s) => s.to_string_lossy(),
        Err(_) => format!("{v:?}"),
    };
    if let bf_js::value::Value::Object(o) = v
        && let Ok(bf_js::value::Value::String(st)) = agent.get_str(*o, "stack")
    {
        return format!("{s}\n{}", st.to_string_lossy());
    }
    s
}

fn dump_ast(path: &str, flags: &[String]) -> ExitCode {
    let module = flags.iter().any(|f| f == "--module") || path.ends_with(".mjs");
    let check = flags.iter().any(|f| f == "--check");
    let locations = !flags.iter().any(|f| f == "--no-locations");
    let Some(src) = read(path) else {
        return ExitCode::FAILURE;
    };
    let opts = ParseOptions {
        max_stack: 192 << 20,
        ..ParseOptions::default()
    };
    let result = if module {
        parse_module(&src, &opts)
    } else {
        parse_script(&src, &opts)
    };
    match result {
        Ok(program) => {
            if !check {
                println!(
                    "{}",
                    to_json(
                        &program,
                        &Options {
                            locations,
                            raw: true
                        }
                    )
                );
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{path}:{}:{}: {}", e.line, e.column, e.message);
            ExitCode::FAILURE
        }
    }
}
