//! Medición provisional de WPT `dom/nodes` y `dom/events` con testharness.js (el runner de
//! verdad es el de E1 en `bf_web::html`). Ejecuta los `script` de cada test en orden sobre el
//! documento ya parseado, dispara `load` y corre el event loop hasta que testharness termina.
//!
//! `cargo test -p bf-web --test wpt_dom -- --ignored --nocapture` (con `WPT_FILTER=texto`
//! para limitar los archivos).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bf_dom::events::{EventInit, EventKind, EventTarget};
use bf_js::host::DefaultHooks;
use bf_js::value::Value;
use bf_web::{DocumentOwner, Runtime, Url, WebContext};

fn wpt_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/wpt")
}

/// El resultado de un archivo: (pasaron, total, error del harness).
fn run_file(path: &Path) -> (usize, usize, Option<String>) {
    let html = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return (0, 0, Some(e.to_string())),
    };
    let rel = path
        .strip_prefix(wpt_root())
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    let (doc, _) = bf_html::parse_document(&html, &bf_html::ParseOptions::default());
    let mut rt = Runtime::new(Box::new(DefaultHooks));
    let realm = rt.main_realm();
    let url = Url::parse(&format!("http://web-platform.test:8000/{rel}"), None).expect("url");
    let ctx = bf_web::install(
        &mut rt.agent,
        realm,
        WebContext::new(DocumentOwner::Document(Box::new(doc)), url),
    );
    // Los `script` en orden de documento.
    let scripts: Vec<(Option<String>, String)> = {
        let d = ctx.get(&rt.agent).document();
        d.descendants(d.root())
            .into_iter()
            .filter_map(|n| {
                let e = d.element(n)?;
                if !e.is_html("script") {
                    return None;
                }
                Some((
                    e.attr("src").map(str::to_owned),
                    d.child_text_content(n).to_string(),
                ))
            })
            .collect()
    };
    let prelude = "if (typeof location === 'undefined') { \
        var location = { href: document.URL, search: '', hash: '', \
          pathname: new URL(document.URL).pathname, protocol: 'http:', \
          host: 'web-platform.test:8000', hostname: 'web-platform.test', port: '8000', \
          origin: 'http://web-platform.test:8000', reload: function () {} }; }";
    let _ = rt.eval_script(realm, prelude, "prelude.js");
    let dir = path.parent().expect("dir").to_path_buf();
    for (src, text) in scripts {
        let (code, name) = match src {
            Some(s) if s.ends_with("testharnessreport.js") => (
                "add_completion_callback(function (tests, status) { \
                   window.__wpt = { status: status.status, message: status.message, \
                     tests: tests.map(function (t) { return [t.name, t.status, t.message]; }) }; });"
                    .to_owned(),
                s,
            ),
            Some(s) => {
                let file = if let Some(abs) = s.strip_prefix('/') {
                    wpt_root().join(abs)
                } else {
                    dir.join(&s)
                };
                match std::fs::read_to_string(&file) {
                    Ok(c) => (c, s),
                    Err(_) => continue,
                }
            }
            None => (text, rel.clone()),
        };
        if let Err(e) = rt.eval_script(realm, &code, &name)
            && name.ends_with("testharness.js")
        {
            return (0, 0, Some(format!("testharness.js: {}", e.message)));
        }
    }
    // `load` en la ventana (testharness espera a que cargue la página).
    bf_web::dom::events::fire_event(
        &mut rt.agent,
        0,
        EventTarget::Window,
        "load",
        EventInit::default(),
        EventKind::Event,
    );
    let start = Instant::now();
    let limit = Duration::from_secs(10);
    loop {
        let _ = rt.run_until_idle();
        let g = rt.global(realm);
        if let Ok(Value::Object(_)) = rt.get(g, "__wpt") {
            break;
        }
        match rt.next_wake() {
            Some(_) if start.elapsed() < limit => std::thread::sleep(Duration::from_millis(1)),
            _ => break,
        }
    }
    let g = rt.global(realm);
    let Ok(Value::Object(r)) = rt.get(g, "__wpt") else {
        return (
            0,
            0,
            Some("sin resultado (timeout o harness incompleto)".into()),
        );
    };
    let Ok(Value::Object(tests)) = rt.get(r, "tests") else {
        return (0, 0, Some("sin tests".into()));
    };
    let n = match rt.get(tests, "length") {
        Ok(Value::Number(n)) => n as usize,
        _ => 0,
    };
    let mut pass = 0;
    let verbose = std::env::var("WPT_VERBOSE").is_ok();
    for i in 0..n {
        let Ok(Value::Object(t)) = rt.get(tests, &i.to_string()) else {
            continue;
        };
        let status = match rt.get(t, "1") {
            Ok(Value::Number(s)) => s as i32,
            _ => -1,
        };
        if status == 0 {
            pass += 1;
        } else if verbose {
            let name = rt
                .get(t, "0")
                .map(|v| rt.to_display(&v))
                .unwrap_or_default();
            let msg = rt
                .get(t, "2")
                .map(|v| rt.to_display(&v))
                .unwrap_or_default();
            println!("    FAIL {name}: {msg}");
        }
    }
    let harness = match rt.get(r, "status") {
        Ok(Value::Number(s)) if s != 0.0 => {
            let m = rt
                .get(r, "message")
                .map(|v| rt.to_display(&v))
                .unwrap_or_default();
            Some(format!("harness status {s}: {m}"))
        }
        _ => None,
    };
    (pass, n, harness)
}

fn run_dir(sub: &str) -> (usize, usize) {
    let filter = std::env::var("WPT_FILTER").ok();
    let mut files: Vec<PathBuf> = std::fs::read_dir(wpt_root().join(sub))
        .expect("dir de WPT")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            (name.ends_with(".html") || name.ends_with(".xhtml"))
                && !name.contains("-ref.")
                && !name.contains("manual")
                && filter.as_deref().is_none_or(|f| name.contains(f))
        })
        .collect();
    files.sort();
    let (mut pass, mut total) = (0, 0);
    for f in files {
        let name = f
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_owned();
        let t0 = Instant::now();
        // Un hilo con pila grande por archivo (el intérprete en debug usa mucha pila).
        let file = f.clone();
        let r = std::thread::Builder::new()
            .stack_size(512 << 20)
            .spawn(move || run_file(&file))
            .expect("hilo")
            .join();
        let (p, n, err) = r.unwrap_or_else(|_| (0, 0, Some("panic".into())));
        pass += p;
        total += n;
        println!(
            "{name}: {p}/{n}{} ({} ms)",
            err.map(|e| format!("  [{e}]")).unwrap_or_default(),
            t0.elapsed().as_millis()
        );
    }
    println!("== {sub}: {pass}/{total}");
    (pass, total)
}

#[test]
#[ignore]
fn wpt_dom_nodes() {
    run_dir("dom/nodes");
}

#[test]
#[ignore]
fn wpt_dom_events() {
    run_dir("dom/events");
}
