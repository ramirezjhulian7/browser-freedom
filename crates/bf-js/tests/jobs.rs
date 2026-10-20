//! La cola de jobs (§9.5) y las funciones async: el orden de los microtasks y el drenado
//! con `Agent::run_jobs`.

use std::cell::RefCell;
use std::rc::Rc;

use bf_js::vm::{Agent, AgentOptions};

/// Corre `src` y la cola de jobs; devuelve lo impreso con `print`.
fn run(src: &str) -> String {
    let out = Rc::new(RefCell::new(String::new()));
    let mut agent = Agent::new(AgentOptions::default());
    agent.reset_stack_base();
    let o = out.clone();
    agent.printer = Box::new(move |s| {
        o.borrow_mut().push_str(s);
        o.borrow_mut().push('\n');
    });
    let realm = agent.current_realm;
    bf_js::builtins::host::install_print(&mut agent, realm);
    agent.eval_script(src, "test.js").expect("script");
    agent.run_jobs().expect("jobs");
    out.take()
}

#[test]
fn microtask_order() {
    let src = r#"
        var log = [];
        Promise.resolve().then(() => log.push("a")).then(() => log.push("c"));
        Promise.resolve().then(() => log.push("b"));
        (async () => { log.push("s"); await null; log.push("t"); })();
        log.push("sync");
        Promise.resolve().then(() => Promise.resolve().then(() => print(log.join())));
    "#;
    assert_eq!(run(src), "s,sync,a,b,t,c\n");
}

#[test]
fn async_generators_and_for_await() {
    let src = r#"
        async function* g() { try { yield 1; yield Promise.resolve(2); } finally { print("fin"); } }
        (async () => {
            var r = [];
            for await (var v of g()) { r.push(v); }
            for await (var w of [Promise.resolve(3), 4]) { r.push(w); }
            print(r.join());
        })();
    "#;
    assert_eq!(run(src), "fin\n1,2,3,4\n");
}

#[test]
fn unhandled_rejections_are_tracked() {
    let mut agent = Agent::new(AgentOptions::default());
    agent.reset_stack_base();
    agent
        .eval_script(
            "Promise.reject(1); var p = Promise.reject(2); p.catch(() => {});",
            "t.js",
        )
        .expect("script");
    agent.run_jobs().expect("jobs");
    assert_eq!(agent.unhandled_rejections(), 1);
}
