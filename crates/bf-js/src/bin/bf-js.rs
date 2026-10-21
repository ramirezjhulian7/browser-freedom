//! `bf-js dump-ast <archivo.js> [--module] [--check] [--no-locations]`: imprime el AST como
//! JSON ESTree; con `--check` solo valida (código de salida 0 si parsea, 1 si no).
//!
//! `bf-js run <archivo.js> [--module]`: ejecuta un script (o, con `--module` o `.mjs`, un
//! módulo con sus imports desde el sistema de archivos) con `print` (y `$262`); una
//! excepción no atrapada se imprime en stderr y sale con código 1.
//!
//! `bf-js bench [opciones]`: el benchmark de la VM (ver `bench/mod.rs`).

use std::process::ExitCode;

use bf_js::estree::{Options, to_json};
use bf_js::vm::{Agent, AgentOptions, Throw};
use bf_js::{ParseOptions, parse_module, parse_script};

mod bench;

const USAGE: &str = "uso: bf-js dump-ast <archivo.js> [--module] [--check] [--no-locations]\n     bf-js run <archivo.js> [--module]\n     bf-js bench [--filter texto] [--runs N] [--compare base.tsv] [--save x.tsv] [--gc]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "bench") {
        return bench::main(args[1..].to_vec());
    }
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
        "dump-bytecode" => dump_bytecode(path),
        "run" => {
            let module = flags.iter().any(|f| f == "--module") || path.ends_with(".mjs");
            run_script(path, module)
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn run_script(path: &str, module: bool) -> ExitCode {
    let src = if module {
        String::new()
    } else {
        match read(path) {
            Some(s) => s,
            None => return ExitCode::FAILURE,
        }
    };
    let mut agent = Agent::new(AgentOptions {
        max_stack: 192 << 20,
        ..AgentOptions::default()
    });
    agent.reset_stack_base();
    let realm = agent.current_realm;
    bf_js::builtins::host::install_test262(&mut agent, realm);
    // E3: los timers, `queueMicrotask`, `requestAnimationFrame` y `console` (hooks por defecto).
    bf_js::host::event_loop::install_globals(&mut agent, realm);
    let result = if module {
        // E1: módulos ESM con FileModuleLoader.
        use bf_js::vm::module::FileModuleLoader;
        agent.set_module_loader(std::rc::Rc::new(FileModuleLoader));
        let url = FileModuleLoader::path_to_url(std::path::Path::new(path));
        agent
            .run_module(&url)
            .map(|_| bf_js::value::Value::Undefined)
    } else {
        agent.eval_script(&src, path)
    };
    // E3: RunJobs al terminar el script.
    // y luego el bucle de eventos hasta que no quede nada pendiente.
    let result = result
        .and_then(|v| agent.run_jobs().map(|_| v))
        .and_then(|v| run_event_loop(&mut agent).map(|_| v));
    match result {
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

/// E3: corre las tareas y los temporizadores hasta que no quede nada pendiente (durmiendo
/// hasta el próximo vencimiento).
fn run_event_loop(agent: &mut Agent) -> Result<(), Throw> {
    loop {
        agent.run_until_idle()?;
        let Some(wake) = agent.next_wake() else {
            return Ok(());
        };
        let now = agent.host.hooks.now();
        if wake > now {
            std::thread::sleep(std::time::Duration::from_secs_f64((wake - now) / 1000.0));
        }
    }
}

/// `dump-bytecode`: las instrucciones de cada función del script (para estudiar el
/// rendimiento de la VM).
fn dump_bytecode(path: &str) -> ExitCode {
    let Some(src) = read(path) else {
        return ExitCode::FAILURE;
    };
    let mut agent = Agent::new(AgentOptions::default());
    let code = match agent.compile_script_text(&src, path) {
        Ok(c) => c,
        Err(_) => {
            eprintln!("error de sintaxis");
            return ExitCode::FAILURE;
        }
    };
    fn dump(code: &bf_js::compiler::bytecode::FunctionCode, depth: usize) {
        let pad = "  ".repeat(depth);
        println!(
            "{pad}== {} (locals {})",
            code.name.to_string_lossy(),
            code.num_locals
        );
        for (i, op) in code.ops.iter().enumerate() {
            println!("{pad}{i:5} {op:?}");
        }
        for c in &code.consts {
            if let bf_js::compiler::bytecode::Constant::Function(f) = c {
                dump(f, depth + 1);
            }
        }
    }
    dump(&code, 0);
    ExitCode::SUCCESS
}
