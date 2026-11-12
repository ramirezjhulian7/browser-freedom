//! Service Workers (`bf_web::workers::service`): registro, ciclo de vida, mensajes,
//! `clients`, `FetchEvent` y persistencia en el perfil.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bf_js::host::{ConsoleLevel, HostHooks};
use bf_web::net::{NetBackend, NetRequest, NetResponse, NetResult};
use bf_web::{DocumentOwner, Runtime, Url, WebContext, WebContextHandle};

/// Los archivos del sitio, en memoria. `<ruta>.headers` agrega cabeceras (como en WPT) y
/// cada petición queda anotada (para ver qué salió a la red).
#[derive(Clone)]
struct Files {
    files: Arc<Mutex<HashMap<String, String>>>,
    log: Arc<Mutex<Vec<String>>>,
}

impl NetBackend for Files {
    fn fetch(&self, req: &NetRequest) -> NetResult {
        if matches!(req.url.scheme.as_str(), "blob" | "data") {
            return NetBackend::fetch(&bf_net::Fetcher::new(), req);
        }
        let path = req.url.path_string();
        self.log.lock().unwrap().push(path.clone());
        let files = self.files.lock().unwrap();
        let Some(text) = files.get(&path) else {
            return Ok(NetResponse {
                url_list: vec![req.url.clone()],
                status: 404,
                status_text: "Not Found".into(),
                headers: Default::default(),
                body: Vec::new(),
            });
        };
        let mut headers = bf_web::net::headers::HeaderList::default();
        let ct = if path.ends_with(".js") {
            "text/javascript"
        } else {
            "text/plain"
        };
        headers.append("Content-Type", ct);
        if let Some(extra) = files.get(&format!("{path}.headers")) {
            for line in extra.lines() {
                if let Some((n, v)) = line.split_once(':') {
                    headers.append(n.trim(), v.trim());
                }
            }
        }
        Ok(NetResponse {
            url_list: vec![req.url.clone()],
            status: 200,
            status_text: "OK".into(),
            headers,
            body: text.clone().into_bytes(),
        })
    }
}

struct Hooks(Arc<Mutex<Vec<String>>>);

impl HostHooks for Hooks {
    fn console(&mut self, _level: ConsoleLevel, message: &str) {
        self.0.lock().unwrap().push(message.to_owned());
    }
    fn report_exception(&mut self, message: &str, _stack: &str) {
        self.0.lock().unwrap().push(format!("EXC {message}"));
    }
}

struct Page {
    rt: Runtime,
    ctx: WebContextHandle,
    console: Arc<Mutex<Vec<String>>>,
}

const PRELUDE: &str = "var result = null, done = false, log = [];\
    function finish(v) { if (!done) { result = String(v); done = true; } }\
    function fail(e) { finish('ERR ' + (e && e.name) + ': ' + (e && e.message)); }\
    function waitState(w, s) { return new Promise(r => { if (w.state == s) return r(); w.addEventListener('statechange', () => { if (w.state == s) r(); }); }); }";

impl Page {
    fn new(files: &[(&str, &str)]) -> Page {
        let files = Files {
            files: Arc::new(Mutex::new(
                files
                    .iter()
                    .map(|(p, t)| ((*p).to_owned(), (*t).to_owned()))
                    .collect(),
            )),
            log: Arc::default(),
        };
        Page::build(
            files,
            "https://sw.test/dir/page.html",
            bf_web::workers::new_partition(),
            None,
        )
    }

    fn build(files: Files, url: &str, partition: u64, root: Option<std::path::PathBuf>) -> Page {
        let console = Arc::new(Mutex::new(Vec::new()));
        let mut rt = Runtime::new(Box::new(Hooks(console.clone())));
        let (doc, _) =
            bf_html::parse_document("<!DOCTYPE html><p>x", &bf_html::ParseOptions::default());
        let realm = rt.main_realm();
        let url = Url::parse(url, None).expect("url");
        let ctx = bf_web::install(
            &mut rt.agent,
            realm,
            WebContext::new(DocumentOwner::Document(Box::new(doc)), url),
        );
        if root.is_some() {
            bf_web::storage::set_storage_dir(&mut rt.agent, realm, root);
        }
        bf_web::net::set_backend(&mut rt.agent, realm, Arc::new(files), false);
        bf_web::workers::set_partition(&mut rt.agent, partition);
        let mut p = Page { rt, ctx, console };
        p.exec(PRELUDE);
        p
    }

    fn exec(&mut self, src: &str) {
        let realm = self.ctx.realm;
        if let Err(e) = self.rt.eval_script(realm, src, "test.js") {
            panic!("error: {e}\n{}", e.stack);
        }
    }

    fn eval_string(&mut self, src: &str) -> String {
        let realm = self.ctx.realm;
        match self
            .rt
            .eval_script(realm, &format!("String({src})"), "test.js")
        {
            Ok(v) => self.rt.convert::<String>(&v).expect("string"),
            Err(e) => panic!("error: {e}\n{}", e.stack),
        }
    }

    fn run(&mut self, src: &str) -> String {
        let realm = self.ctx.realm;
        self.exec("done = false; result = null;");
        self.exec(&format!("(async () => {{ {src} }})().catch(fail);"));
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            bf_web::net::pump(&mut self.rt.agent, realm);
            let _ = self.rt.run_until_idle();
            if self.eval_string("done") == "true" {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "tiempo agotado; log = {}; consola = {:?}",
                self.eval_string("JSON.stringify(log)"),
                self.console.lock().unwrap()
            );
            bf_web::workers::wait_for_events(
                &self.rt.agent,
                Instant::now() + Duration::from_millis(5),
            );
        }
        self.eval_string("result")
    }
}

macro_rules! sw {
    ($name:ident, [$($file:expr => $text:expr),* $(,)?], $src:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let mut p = Page::new(&[$(($file, $text)),*]);
            assert_eq!(p.run($src), $expected);
        }
    };
}

const SIMPLE: &str =
    "self.addEventListener('install', e => {}); self.addEventListener('activate', e => {});";

// --- Interfaces y contexto seguro ---

sw!(
    interfaces_in_secure_context,
    [],
    "finish([typeof navigator.serviceWorker, navigator.serviceWorker instanceof ServiceWorkerContainer, navigator.serviceWorker === navigator.serviceWorker, typeof ServiceWorker, typeof ServiceWorkerRegistration, String(navigator.serviceWorker.controller), typeof ExtendableEvent].join());",
    "object,true,true,function,function,null,undefined"
);

#[test]
fn no_service_workers_in_insecure_context() {
    let files = Files {
        files: Arc::default(),
        log: Arc::default(),
    };
    let mut p = Page::build(
        files,
        "http://sw.test/page.html",
        bf_web::workers::new_partition(),
        None,
    );
    assert_eq!(
        p.eval_string("[typeof navigator.serviceWorker, 'serviceWorker' in navigator, typeof ServiceWorker].join()"),
        "undefined,false,undefined"
    );
}

sw!(
    illegal_constructors,
    [],
    "var r = []; for (const C of [ServiceWorker, ServiceWorkerRegistration, ServiceWorkerContainer]) { try { new C(); r.push('no'); } catch (e) { r.push(e.name); } } finish(r.join());",
    "TypeError,TypeError,TypeError"
);

// --- Registro y ciclo de vida ---

sw!(
    register_install_activate,
    ["/dir/sw.js" => SIMPLE],
    "const reg = await navigator.serviceWorker.register('sw.js');\
     log.push(reg.scope, reg.updateViaCache, String(reg.waiting), String(reg.active));\
     const w = reg.installing; log.push(w.scriptURL, w.state);\
     const states = []; w.onstatechange = () => states.push(w.state);\
     await waitState(w, 'activated');\
     finish(log.join('|') + '|' + states.join() + '|' + (reg.active === w) + ',' + reg.installing + ',' + reg.waiting);",
    "https://sw.test/dir/|imports|null|null|https://sw.test/dir/sw.js|installing|installed,activating,activated|true,null,null"
);

sw!(
    updatefound_fires_after_resolution,
    ["/dir/sw.js" => SIMPLE],
    "const reg = await navigator.serviceWorker.register('sw.js', { scope: 'a/' });\
     reg.onupdatefound = () => { log.push('updatefound ' + reg.installing.state); };\
     await waitState(reg.installing, 'activated');\
     finish(log.join() + ' ' + reg.scope);",
    "updatefound installing https://sw.test/dir/a/"
);

sw!(
    wait_until_delays_and_rejection_fails_install,
    ["/dir/ok.js" => "self.oninstall = e => e.waitUntil(new Promise(r => setTimeout(r, 30)));",
     "/dir/bad.js" => "self.oninstall = e => e.waitUntil(Promise.reject(new Error('no')));"],
    "const r1 = await navigator.serviceWorker.register('ok.js', { scope: 'ok/' });\
     await waitState(r1.installing, 'activated');\
     const r2 = await navigator.serviceWorker.register('bad.js', { scope: 'bad/' });\
     const w = r2.installing; await waitState(w, 'redundant');     while (r2.installing) await new Promise(r => setTimeout(r, 5));\
     const g = await navigator.serviceWorker.getRegistration('bad/x');\
     finish([r1.active.state, w.state, String(r2.installing), String(g)].join());",
    "activated,redundant,null,undefined"
);

sw!(
    register_errors,
    ["/dir/text.txt" => "x", "/dir/throws.js" => "throw new Error('boom');",
     "/dir/sub/sw.js" => SIMPLE],
    "const sw = navigator.serviceWorker; const out = [];\
     for (const [u, o] of [['nope.js'], ['text.txt'], ['throws.js'], ['sub/sw.js', { scope: '/dir/' }], ['data:text/javascript,1'], ['sw.js', { scope: 'x%2fy/' }], ['sw.js', { type: 'bad' }]]) {\
       try { await sw.register(u, o); out.push('ok'); } catch (e) { out.push(e.name); } }\
     finish(out.join());",
    "TypeError,SecurityError,TypeError,SecurityError,TypeError,TypeError,TypeError"
);

sw!(
    service_worker_allowed_widens_scope,
    ["/dir/sub/sw.js" => SIMPLE, "/dir/sub/sw.js.headers" => "Service-Worker-Allowed: /dir/"],
    "const reg = await navigator.serviceWorker.register('sub/sw.js', { scope: '/dir/' }); finish(reg.scope);",
    "https://sw.test/dir/"
);

sw!(
    cross_origin_script_is_security_error,
    [],
    "try { await navigator.serviceWorker.register('https://otro.test/sw.js'); finish('ok'); } catch (e) { finish(e.name); }",
    "SecurityError"
);

sw!(
    same_registration_twice_and_registrations_list,
    ["/dir/sw.js" => SIMPLE],
    "const sw = navigator.serviceWorker;\
     const a = await sw.register('sw.js', { scope: 'x/' }); const b = await sw.register('sw.js', { scope: 'x/' });\
     const c = await sw.register('sw.js', { scope: 'y/' });\
     const all = await sw.getRegistrations();\
     const g = await sw.getRegistration('x/page');\
     finish([a === b, a === c, all.length, all[0] === a, all[1] === c, g === a].join());",
    "true,false,2,true,true,true"
);

sw!(
    get_registration_other_origin_is_security_error,
    [],
    "try { await navigator.serviceWorker.getRegistration('https://otro.test/'); finish('ok'); } catch (e) { finish(e.name); }",
    "SecurityError"
);

sw!(
    unregister_clears_registration,
    ["/dir/sw.js" => SIMPLE],
    "const reg = await navigator.serviceWorker.register('sw.js', { scope: 'u/' });\
     const w = reg.installing; await waitState(w, 'activated');\
     const ok = await reg.unregister();\
     await waitState(w, 'redundant');\
     const again = await reg.unregister();\
     const g = await navigator.serviceWorker.getRegistration('u/');\
     finish([ok, again, String(g), String(reg.active), w.state].join());",
    "true,false,undefined,null,redundant"
);

sw!(
    ready_resolves_with_active_registration,
    ["/dir/sw.js" => SIMPLE],
    "navigator.serviceWorker.register('sw.js', { scope: '/dir/' }).catch(fail);\
     const reg = await navigator.serviceWorker.ready;\
     finish(reg.scope + ' ' + (reg.active !== null) + ' ' + ['activating', 'activated'].includes(reg.active.state));",
    "https://sw.test/dir/ true true"
);

#[test]
fn update_with_new_script_and_skip_waiting() {
    let files = Files {
        files: Arc::new(Mutex::new(
            [(
                "/dir/sw.js".to_owned(),
                "self.oninstall = () => self.skipWaiting(); var V = 1;".to_owned(),
            )]
            .into_iter()
            .collect(),
        )),
        log: Arc::default(),
    };
    let mut p = Page::build(
        files.clone(),
        "https://sw.test/dir/page.html",
        bf_web::workers::new_partition(),
        None,
    );
    assert_eq!(
        p.run("window.reg = await navigator.serviceWorker.register('sw.js', { scope: 'up/' });               window.w1 = reg.installing; await waitState(w1, 'activated');               const same = await reg.update(); finish((same === reg) + ',' + reg.installing);"),
        "true,null"
    );
    files.files.lock().unwrap().insert(
        "/dir/sw.js".into(),
        "self.oninstall = () => self.skipWaiting(); var V = 2;".into(),
    );
    assert_eq!(
        p.run("await reg.update(); const w2 = reg.installing;               log.push(w2 !== null && w2 !== w1);               await waitState(w2, 'activated');               finish(log.join() + ',' + w1.state + ',' + (reg.active === w2));"),
        "true,redundant,true"
    );
}

sw!(
    install_event_order_and_global,
    ["/dir/sw.js" => "var seen = [];\
        self.addEventListener('install', e => seen.push(e.constructor.name + ':' + (e instanceof ExtendableEvent) + ':' + e.isTrusted));\
        self.addEventListener('activate', e => seen.push('activate'));\
        self.onmessage = e => e.source.postMessage([seen.join(), self instanceof ServiceWorkerGlobalScope, typeof self.clients, self.registration.scope, self.serviceWorker.scriptURL, typeof XMLHttpRequest, typeof Worker, typeof importScripts].join('|'));"],
    "const reg = await navigator.serviceWorker.register('sw.js', { scope: 'g/' });\
     await waitState(reg.installing, 'activated');\
     navigator.serviceWorker.onmessage = e => finish(e.data + '|' + (e.source === reg.active) + '|' + e.origin);\
     reg.active.postMessage('hola');",
    "ExtendableEvent:true:true,activate|true|object|https://sw.test/dir/g/|https://sw.test/dir/sw.js|undefined|undefined|function|true|https://sw.test"
);

sw!(
    message_event_in_worker,
    ["/dir/sw.js" => "self.onmessage = e => e.waitUntil(Promise.resolve().then(() => e.source.postMessage([e.constructor.name, e.data.n * 2, e.origin, e.source.constructor.name, e.source.type, e.source.url, e.ports.length].join())));"],
    "const reg = await navigator.serviceWorker.register('sw.js', { scope: 'm/' });\
     await waitState(reg.installing, 'activated');\
     navigator.serviceWorker.startMessages();\
     navigator.serviceWorker.addEventListener('message', e => finish(e.data));\
     reg.active.postMessage({ n: 21 });",
    "ExtendableMessageEvent,42,https://sw.test,WindowClient,window,https://sw.test/dir/page.html,0"
);

sw!(
    wait_until_outside_dispatch_throws,
    ["/dir/sw.js" => "var saved; self.oninstall = e => { saved = e; };\
        self.onmessage = e => { let r; try { saved.waitUntil(Promise.resolve()); r = 'no'; } catch (x) { r = x.name; }\
          let u; try { new ExtendableEvent('x').waitUntil(Promise.resolve()); u = 'no'; } catch (x) { u = x.name; }\
          e.source.postMessage(r + ',' + u); };"],
    "const reg = await navigator.serviceWorker.register('sw.js', { scope: 'wu/' });\
     await waitState(reg.installing, 'activated');\
     navigator.serviceWorker.onmessage = e => finish(e.data);\
     reg.active.postMessage(0);",
    "InvalidStateError,InvalidStateError"
);

// --- Clientes ---

sw!(
    claim_sets_controller_and_match_all,
    ["/dir/sw.js" => "self.onactivate = e => e.waitUntil(self.clients.claim());\
        self.onmessage = async e => { const all = await clients.matchAll(); const unc = await clients.matchAll({ includeUncontrolled: true });\
          const c = await clients.get(e.source.id); e.source.postMessage([all.length, unc.length, c.id === e.source.id, String(await clients.get('nada'))].join()); };"],
    "const sw = navigator.serviceWorker; let changed = 0; sw.oncontrollerchange = () => changed++;\
     const reg = await sw.register('sw.js', { scope: '/dir/' });\
     await waitState(reg.installing, 'activated');\
     await new Promise(r => setTimeout(r, 20));\
     log.push(changed, sw.controller === reg.active);\
     sw.onmessage = e => finish(log.join() + '|' + e.data);\
     sw.controller.postMessage('x');",
    "1,true|1,1,true,undefined"
);

sw!(
    claim_before_activation_is_invalid_state,
    ["/dir/sw.js" => "self.oninstall = e => e.waitUntil(clients.claim().then(() => 'ok', x => x.name).then(r => { self.r = r; }));\
        self.onmessage = e => e.source.postMessage(self.r);"],
    "const reg = await navigator.serviceWorker.register('sw.js', { scope: 'c/' });\
     await waitState(reg.installing, 'activated');\
     navigator.serviceWorker.onmessage = e => finish(e.data);\
     reg.active.postMessage(0);",
    "InvalidStateError"
);

// --- FetchEvent ---

const FETCH_SW: &str = "self.onactivate = e => e.waitUntil(clients.claim());\
    self.onfetch = e => {\
      const u = new URL(e.request.url);\
      if (u.pathname.endsWith('/hola')) e.respondWith(new Response('desde el SW ' + e.request.method + ' ' + (e.clientId !== ''), { headers: { 'X-Sw': '1' } }));\
      else if (u.pathname.endsWith('/lento')) e.respondWith(new Promise(r => setTimeout(() => r(new Response('tarde')), 20)));\
      else if (u.pathname.endsWith('/proxy')) e.respondWith(fetch('real.txt'));\
      else if (u.pathname.endsWith('/rechazo')) e.respondWith(Promise.reject(new Error('x')));\
      else if (u.pathname.endsWith('/cancela')) e.preventDefault();\
      else if (u.pathname.endsWith('/nojs')) e.respondWith('no es un Response');\
    };";

fn controlled_page() -> Page {
    let mut p = Page::new(&[("/dir/sw.js", FETCH_SW), ("/dir/real.txt", "de la red")]);
    let r = p.run(
        "const reg = await navigator.serviceWorker.register('sw.js', { scope: '/dir/' });\
         await waitState(reg.installing, 'activated');\
         while (!navigator.serviceWorker.controller) await new Promise(r => setTimeout(r, 5));\
         finish('ok');",
    );
    assert_eq!(r, "ok");
    p
}

#[test]
fn fetch_event_respond_with() {
    let mut p = controlled_page();
    assert_eq!(
        p.run("const r = await fetch('hola', { method: 'POST', body: 'x' }); finish([r.status, r.type, r.url, r.headers.get('x-sw'), await r.text()].join('|'));"),
        "200|basic|https://sw.test/dir/hola|1|desde el SW POST true"
    );
    assert_eq!(
        p.run("finish(await (await fetch('lento')).text());"),
        "tarde"
    );
    assert_eq!(
        p.run("const r = await fetch('proxy'); finish(r.url + ' ' + await r.text());"),
        "https://sw.test/dir/real.txt de la red"
    );
}

#[test]
fn fetch_event_fallback_and_errors() {
    let mut p = controlled_page();
    assert_eq!(
        p.run("const r = await fetch('real.txt'); finish(r.status + ' ' + await r.text());"),
        "200 de la red"
    );
    for path in ["rechazo", "cancela", "nojs"] {
        assert_eq!(
            p.run(&format!(
                "try {{ await fetch('{path}'); finish('ok'); }} catch (e) {{ finish(e.name); }}"
            )),
            "TypeError",
            "{path}"
        );
    }
}

#[test]
fn uncontrolled_page_goes_to_network() {
    let mut p = Page::new(&[
        (
            "/dir/sw.js",
            FETCH_SW
                .replace("self.onactivate = e => e.waitUntil(clients.claim());", "")
                .as_str(),
        ),
        ("/dir/hola", "red"),
    ]);
    assert_eq!(
        p.run("const reg = await navigator.serviceWorker.register('sw.js', { scope: '/dir/' });\
               await waitState(reg.installing, 'activated');\
               const r = await fetch('hola'); finish(String(navigator.serviceWorker.controller) + ' ' + await r.text());"),
        "null red"
    );
}

sw!(
    fetch_event_constructor,
    ["/dir/sw.js" => "self.onmessage = e => { const r = []; const req = new Request('https://sw.test/x');\
        const f = new FetchEvent('fetch', { request: req, clientId: 'c' });\
        r.push(f.request === req, f.clientId, f.resultingClientId === '', f.isTrusted);\
        try { f.respondWith(new Response('')); r.push('no'); } catch (x) { r.push(x.name); }\
        try { new FetchEvent('fetch', {}); r.push('no'); } catch (x) { r.push(x.name); }\
        const m = new ExtendableMessageEvent('message', { data: 3 }); r.push(m.data, m.source, m.ports.length);\
        e.source.postMessage(r.join()); };"],
    "const reg = await navigator.serviceWorker.register('sw.js', { scope: 'fe/' });\
     await waitState(reg.installing, 'activated');\
     navigator.serviceWorker.onmessage = e => finish(e.data);\
     reg.active.postMessage(0);",
    "true,c,true,false,InvalidStateError,TypeError,3,,0"
);

// --- importScripts y persistencia ---

sw!(
    import_scripts_cached_after_install,
    ["/dir/sw.js" => "importScripts('lib.js');\
        self.onmessage = e => { let r; try { importScripts('otro.js'); r = 'no'; } catch (x) { r = x.name; } importScripts('lib.js'); e.source.postMessage(LIB + ',' + r); };",
     "/dir/lib.js" => "var LIB = 'lib';", "/dir/otro.js" => "var OTRO = 1;"],
    "const reg = await navigator.serviceWorker.register('sw.js', { scope: 'i/' });\
     await waitState(reg.installing, 'activated');\
     navigator.serviceWorker.onmessage = e => finish(e.data);\
     reg.active.postMessage(0);",
    "lib,NetworkError"
);

#[test]
fn registration_persists_in_profile() {
    let dir = std::env::temp_dir().join(format!("freedom-sw-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let partition = bf_web::workers::new_partition();
    let files = Files {
        files: Arc::new(Mutex::new(
            [("/dir/sw.js".to_owned(), FETCH_SW.to_owned())]
                .into_iter()
                .collect(),
        )),
        log: Arc::default(),
    };
    {
        let mut p = Page::build(
            files.clone(),
            "https://persist.test/dir/page.html",
            partition,
            Some(dir.clone()),
        );
        assert_eq!(
            p.run("const reg = await navigator.serviceWorker.register('sw.js', { scope: '/dir/' }); await waitState(reg.installing, 'activated'); finish('ok');"),
            "ok"
        );
    }
    assert!(
        dir.join("https_persist.test")
            .join("service_workers")
            .exists()
    );
    // Otra página nueva del mismo origen en otra partición (como otra sesión del perfil:
    // el registro sale del disco): nace controlada y su `fetch` pasa por el service worker
    // guardado sin volver a pedirlo.
    files.log.lock().unwrap().clear();
    let mut p = Page::build(
        files.clone(),
        "https://persist.test/dir/otra.html",
        bf_web::workers::new_partition(),
        Some(dir.clone()),
    );
    assert_eq!(
        p.run("const r = await fetch('hola'); finish((navigator.serviceWorker.controller !== null) + ' ' + await r.text());"),
        "true desde el SW GET true"
    );
    assert!(!files.log.lock().unwrap().iter().any(|p| p == "/dir/sw.js"));
    let _ = std::fs::remove_dir_all(&dir);
}

// --- Service worker + Cache API ---

#[test]
fn service_worker_serves_from_cache() {
    let sw = "self.oninstall = e => e.waitUntil(caches.open('v1').then(c => c.addAll(['datos.txt'])));\
        self.onactivate = e => e.waitUntil(clients.claim());\
        self.onfetch = e => e.respondWith(caches.match(e.request).then(r => r || new Response('sin caché')));";
    let mut p = Page::new(&[("/dir/sw.js", sw), ("/dir/datos.txt", "guardado")]);
    assert_eq!(
        p.run(
            "const reg = await navigator.serviceWorker.register('sw.js', { scope: '/dir/' });\
             await waitState(reg.installing, 'activated');\
             while (!navigator.serviceWorker.controller) await new Promise(r => setTimeout(r, 5));\
             const a = await (await fetch('datos.txt')).text();\
             const b = await (await fetch('otro.txt')).text();\
             const keys = await caches.keys();\
             finish([a, b, keys.join()].join('|'));"
        ),
        "guardado|sin caché|v1"
    );
}

// --- XMLHttpRequest por el service worker ---

#[test]
fn xhr_goes_through_the_service_worker() {
    let mut p = controlled_page();
    let xhr = |path: &str, method: &str| {
        format!(
            "const x = new XMLHttpRequest(); x.open('{method}', '{path}'); \
             x.onload = () => finish([x.status, x.getResponseHeader('x-sw'), x.responseURL, x.responseText].join('|')); \
             x.onerror = () => finish('error ' + x.readyState); x.send('cuerpo');"
        )
    };
    // `respondWith`: la respuesta sale del service worker.
    assert_eq!(
        p.run(&xhr("hola", "POST")),
        "200|1|https://sw.test/dir/hola|desde el SW POST true"
    );
    // Sin `respondWith`: a la red.
    assert_eq!(
        p.run(&xhr("real.txt", "GET")),
        "200||https://sw.test/dir/real.txt|de la red"
    );
    // Una promesa rechazada en `respondWith`: error de red en el XHR.
    assert_eq!(p.run(&xhr("rechazo", "GET")), "error 4");
}
