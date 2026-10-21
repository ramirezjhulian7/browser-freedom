//! Módulos ECMAScript (§16.2) con un loader en memoria: bindings vivos, ciclos, namespaces,
//! `export default`, errores de Link y de evaluación, módulos JSON e `import.meta`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use bf_js::vm::module::ModuleLoader;
use bf_js::vm::{Agent, AgentOptions, Throw};

/// Los módulos `mem:/<nombre>`; `./x` desde cualquier módulo es `mem:/x`.
struct MemLoader(HashMap<String, String>);

impl ModuleLoader for MemLoader {
    fn resolve(&self, specifier: &str, _referrer: Option<&str>) -> Result<String, String> {
        let name = specifier
            .strip_prefix("./")
            .ok_or("especificador no relativo")?;
        Ok(format!("mem:/{name}"))
    }

    fn load(&self, url: &str) -> Result<String, String> {
        self.0
            .get(url)
            .cloned()
            .ok_or_else(|| format!("no existe {url}"))
    }
}

/// Corre `main.js` y devuelve lo impreso, o el error ("Nombre: mensaje").
fn run(files: &[(&str, &str)]) -> Result<String, String> {
    let mut agent = Agent::new(AgentOptions::default());
    let out = Rc::new(RefCell::new(String::new()));
    let o2 = out.clone();
    agent.printer = Box::new(move |s| {
        o2.borrow_mut().push_str(s);
        o2.borrow_mut().push('\n');
    });
    let realm = agent.current_realm;
    bf_js::builtins::host::install_test262(&mut agent, realm);
    let map = files
        .iter()
        .map(|(n, s)| (format!("mem:/{n}"), (*s).to_owned()))
        .collect();
    agent.set_module_loader(Rc::new(MemLoader(map)));
    match agent.run_module("mem:/main.js") {
        Ok(_) => Ok(out.borrow().clone()),
        Err(Throw::Value(v)) => {
            let name = match &v {
                bf_js::value::Value::Object(o) => agent
                    .get_str(*o, "name")
                    .ok()
                    .and_then(|n| agent.to_string(&n).ok())
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default(),
                _ => String::new(),
            };
            let msg = agent
                .to_string(&v)
                .map(|s| s.to_string_lossy())
                .unwrap_or_default();
            Err(format!("{name}|{msg}"))
        }
        Err(Throw::Terminate(t)) => Err(t),
    }
}

#[test]
fn live_bindings_and_cycles() {
    let out = run(&[
        (
            "main.js",
            "import { count, inc } from './counter.js';
             import { fromB } from './b.js';
             print(count); inc(); inc(); print(count);
             print(fromB());
             export function hoisted() { return 'hoisted'; }",
        ),
        (
            "counter.js",
            "export let count = 0; export function inc() { count++; }",
        ),
        // Ciclo: b importa a main; las funciones de main ya están instanciadas (Link).
        (
            "b.js",
            "import { hoisted } from './main.js';
             export function fromB() { return hoisted(); }
             print('b:' + typeof hoisted);",
        ),
    ])
    .unwrap();
    assert_eq!(out, "b:function\n0\n2\nhoisted\n");
}

#[test]
fn namespace_object() {
    let out = run(&[
        (
            "main.js",
            "import * as ns from './lib.js';
             print(Object.prototype.toString.call(ns));
             print(Reflect.ownKeys(ns).map(String).join());
             print(Object.isExtensible(ns), Object.getPrototypeOf(ns));
             const d = Object.getOwnPropertyDescriptor(ns, 'b');
             print(d.value, d.writable, d.enumerable, d.configurable);
             print(Reflect.deleteProperty(ns, 'b'), Reflect.deleteProperty(ns, 'nope'),
                   Reflect.set(ns, 'b', 1));
             print((0, ns.default)(), ns.C.name, ns.self === ns);",
        ),
        (
            "lib.js",
            "export var b = 2, a = 1;
             export default function () { return 'default:' + this; }
             export class C {}
             export * as self from './lib.js';",
        ),
    ])
    .unwrap();
    assert_eq!(
        out,
        "[object Module]\nC,a,b,default,self,Symbol(Symbol.toStringTag)\nfalse null\n2 true true false\nfalse true false\ndefault:undefined C true\n"
    );
}

#[test]
fn default_export_names() {
    let out = run(&[
        (
            "main.js",
            "import f from './f.js'; import C from './c.js'; import x from './x.js';
             print(f.name, C.name, x.name);",
        ),
        ("f.js", "export default function () {}"),
        ("c.js", "export default class {}"),
        ("x.js", "export default (function () {});"),
    ])
    .unwrap();
    assert_eq!(out, "default default default\n");
}

#[test]
fn tdz_through_namespace_and_import() {
    let out = run(&[
        ("main.js", "import './a.js';"),
        (
            "a.js",
            "import * as ns from './b.js';
             import { later } from './b.js';
             export const early = 1;
             try { ns.later; } catch (e) { print('ns:' + e.constructor.name); }
             try { later; } catch (e) { print('import:' + e.constructor.name); }",
        ),
        // a importa b (ciclo): b se evalúa primero y `early` de a todavía está en TDZ.
        (
            "b.js",
            "import { early } from './a.js'; export let later = early;",
        ),
    ]);
    assert_eq!(
        out.unwrap_err(),
        "ReferenceError|ReferenceError: Cannot access 'early' before initialization"
    );
}

#[test]
fn link_errors() {
    let missing = run(&[
        ("main.js", "import { nope } from './lib.js';"),
        ("lib.js", "export const yes = 1;"),
    ]);
    assert!(missing.unwrap_err().starts_with("SyntaxError|"));
    // Dos `export *` con el mismo nombre desde módulos distintos: ambiguo.
    let ambiguous = run(&[
        ("main.js", "import { x } from './star.js';"),
        ("star.js", "export * from './p.js'; export * from './q.js';"),
        ("p.js", "export const x = 1;"),
        ("q.js", "export const x = 2;"),
    ]);
    assert!(ambiguous.unwrap_err().contains("conflicting star exports"));
    // Ambiguo pero sin pedirlo: no es error, y el namespace lo omite.
    let out = run(&[
        (
            "main.js",
            "import * as ns from './star.js'; print('x' in ns, ns.y);",
        ),
        ("star.js", "export * from './p.js'; export * from './q.js';"),
        ("p.js", "export const x = 1, y = 3;"),
        ("q.js", "export const x = 2;"),
    ])
    .unwrap();
    assert_eq!(out, "false 3\n");
}

#[test]
fn evaluation_error_is_cached() {
    let out = run(&[
        ("main.js", "import './c.js';"),
        ("c.js", "import './bad.js'; print('nunca');"),
        ("bad.js", "throw new RangeError('malo');"),
    ]);
    assert_eq!(out.unwrap_err(), "RangeError|RangeError: malo");
}

#[test]
fn json_modules_and_import_meta() {
    let out = run(&[
        (
            "main.js",
            "import data from './d.json' with { type: 'json' };
             import again from './d.json' with { type: 'json' };
             print(data.list.join(), data === again);
             print(import.meta.url, Object.getPrototypeOf(import.meta), import.meta === import.meta);
             print(this);",
        ),
        ("d.json", r#"{ "list": [1, 2, 3] }"#),
    ])
    .unwrap();
    assert_eq!(out, "1,2,3 true\nmem:/main.js null true\nundefined\n");
    let bad = run(&[
        ("main.js", "import x from './a.js' with { nope: 'json' };"),
        ("a.js", ""),
    ]);
    assert!(bad.unwrap_err().starts_with("SyntaxError|"));
}

#[test]
fn import_bindings_are_immutable() {
    let out = run(&[
        (
            "main.js",
            "import { v } from './v.js';
             try { v = 2; } catch (e) { print(e.constructor.name); }
             print(v);",
        ),
        ("v.js", "export let v = 1;"),
    ])
    .unwrap();
    assert_eq!(out, "TypeError\n1\n");
}

#[test]
fn top_level_await_order() {
    // a espera; b (sin TLA) depende de a y se ejecuta cuando a termina; main al final.
    let out = run(&[
        (
            "main.js",
            "import './b.js'; import { log } from './log.js'; log.push('main'); print(log.join());",
        ),
        ("log.js", "export const log = [];"),
        (
            "a.js",
            "import { log } from './log.js';
             log.push('a1'); await Promise.resolve(); log.push('a2');
             export const a = 1;",
        ),
        (
            "b.js",
            "import { log } from './log.js'; import { a } from './a.js'; log.push('b' + a);",
        ),
    ])
    .unwrap();
    assert_eq!(out, "a1,a2,b1,main\n");
}

#[test]
fn top_level_await_rejection() {
    let out = run(&[
        ("main.js", "import './bad.js'; print('nunca');"),
        ("bad.js", "await null; throw new TypeError('tarde');"),
    ]);
    assert_eq!(out.unwrap_err(), "TypeError|TypeError: tarde");
}

#[test]
fn dynamic_import() {
    let out = run(&[
        (
            "main.js",
            "const p = import('./lib.js');
             print(p instanceof Promise);
             const ns = await p;
             print(ns.x, ns === await import('./lib.js'));
             const j = await import('./d.json', { with: { type: 'json' } });
             print(j.default.ok);
             try { await import('./nope.js'); } catch (e) { print(e.constructor.name); }
             try { await import('./lib.js', { with: { bad: 'x' } }); } catch (e) { print(e.constructor.name); }
             print(await eval(\"import('./lib.js')\") === ns);",
        ),
        ("lib.js", "export const x = 42;"),
        ("d.json", r#"{ "ok": true }"#),
    ])
    .unwrap();
    assert_eq!(out, "true\n42 true\ntrue\nTypeError\nTypeError\ntrue\n");
}
