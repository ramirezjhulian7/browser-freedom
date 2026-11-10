//! Cache API (`bf_web::workers::cache`, Service Workers §5.4 `Cache` y §5.5 `CacheStorage`):
//! `caches` en contextos seguros, `open`/`has`/`keys`/`delete`, `put`/`match` con
//! `CacheQueryOptions` y `Vary`, `add`/`addAll` contra la red, los errores de `put` y el
//! almacén compartido entre agentes de la misma partición.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bf_js::host::{ConsoleLevel, HostHooks};
use bf_web::net::{NetBackend, NetRequest, NetResponse, NetResult};
use bf_web::{DocumentOwner, Runtime, Url, WebContext, WebContextHandle};

/// Los archivos de la página, en memoria (`blob:` y `data:` van al `Fetcher` del proceso).
struct Files(HashMap<String, String>);

impl NetBackend for Files {
    fn fetch(&self, req: &NetRequest) -> NetResult {
        if matches!(req.url.scheme.as_str(), "blob" | "data") {
            return NetBackend::fetch(&bf_net::Fetcher::new(), req);
        }
        let path = req.url.path_string();
        let Some(text) = self.0.get(&path) else {
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
        // Como los `.headers` de WPT: `<ruta>.headers` con líneas `Nombre: valor`.
        if let Some(extra) = self.0.get(&format!("{path}.headers")) {
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

/// Los hooks de la página: la consola (incluida la de los workers) queda anotada.
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
    function fail(e) { finish('ERR ' + (e && e.name) + ': ' + (e && e.message)); }";

const PAGE_URL: &str = "https://cache.test/dir/page.html";

impl Page {
    fn new(files: &[(&str, &str)]) -> Page {
        Page::at(files, PAGE_URL, bf_web::workers::new_partition())
    }

    fn at(files: &[(&str, &str)], url: &str, partition: u64) -> Page {
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
        let map = files
            .iter()
            .map(|(p, t)| ((*p).to_owned(), (*t).to_owned()))
            .collect();
        bf_web::net::set_backend(&mut rt.agent, realm, Arc::new(Files(map)), false);
        // Los tests corren en paralelo con el mismo origen: cada página, su partición (y con
        // ella su almacén de cachés).
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
        self.exec(&format!(
            "done = false; result = null; try {{ {src} }} catch (e) {{ fail(e); }}"
        ));
        let deadline = Instant::now() + Duration::from_secs(20);
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

macro_rules! c {
    ($name:ident, [$($file:expr => $text:expr),* $(,)?], $src:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let mut p = Page::new(&[$(($file, $text)),*]);
            assert_eq!(p.run(concat!("(async () => { ", $src, " })().then(finish, fail);")), $expected);
        }
    };
}

// --- Exposición ---

c!(
    caches_in_secure_context,
    [],
    "const c = await caches.open('a');
     let illegal = '';
     try { new Cache(); } catch (e) { illegal += e.name; }
     try { new CacheStorage(); } catch (e) { illegal += ' ' + e.name; }
     return [typeof caches, caches === self.caches, caches instanceof CacheStorage,
             c instanceof Cache, String(caches), String(c), illegal,
             Object.keys(CacheStorage.prototype).sort().join()].join('|');",
    "object|true|true|true|[object CacheStorage]|[object Cache]|TypeError TypeError|delete,has,keys,match,open"
);

#[test]
fn no_caches_on_http() {
    let mut p = Page::at(
        &[],
        "http://cache.test/dir/page.html",
        bf_web::workers::new_partition(),
    );
    assert_eq!(
        p.eval_string(
            "[typeof caches, typeof Cache, typeof CacheStorage, 'caches' in self].join()"
        ),
        "undefined,undefined,undefined,false"
    );
}

// --- CacheStorage ---

c!(
    open_has_keys_delete,
    [],
    "const before = await caches.has('v1');
     const a = await caches.open('v1');
     const b = await caches.open('v1');
     await caches.open('v2');
     await caches.open('v0');
     const keys = await caches.keys();
     const del = await caches.delete('v2');
     const del2 = await caches.delete('v2');
     return [before, a === b, await caches.has('v1'), keys.join(), del, del2,
             (await caches.keys()).join(), await caches.has('v2')].join('|');",
    "false|false|true|v1,v2,v0|true|false|v1,v0|false"
);

c!(
    deleted_cache_object_still_works,
    [],
    "const c = await caches.open('doomed');
     await c.put('https://cache.test/a', new Response('a'));
     await caches.delete('doomed');
     await c.put('https://cache.test/b', new Response('b'));
     const r = await c.match('https://cache.test/b');
     const fresh = await caches.open('doomed');
     return [await r.text(), (await c.keys()).length, (await fresh.keys()).length].join('|');",
    "b|2|0"
);

c!(
    storage_match_with_and_without_cache_name,
    [],
    "const a = await caches.open('a');
     const b = await caches.open('b');
     await b.put('https://cache.test/x', new Response('from b'));
     await a.put('https://cache.test/y', new Response('from a'));
     const any = await caches.match('https://cache.test/x');
     const inA = await caches.match('https://cache.test/x', { cacheName: 'a' });
     const inB = await caches.match('https://cache.test/x', { cacheName: 'b' });
     const none = await caches.match('https://cache.test/x', { cacheName: 'nope' });
     const y = await caches.match(new Request('https://cache.test/y'));
     return [await any.text(), inA, await inB.text(), none, await y.text()].join('|');",
    "from b||from b||from a"
);

// --- put / match ---

c!(
    put_and_match_text,
    [],
    "const c = await caches.open('t');
     const res = new Response('hola', { status: 201, statusText: 'Made', headers: { 'X-A': '1', 'Content-Type': 'text/plain' } });
     const r = await c.put(new Request('https://cache.test/a#frag'), res);
     const m1 = await c.match('https://cache.test/a');
     const m2 = await c.match('https://cache.test/a');
     let immutable = '';
     try { m1.headers.set('X-B', '2'); } catch (e) { immutable = e.name; }
     return [r, res.bodyUsed, m1 === m2, m1 instanceof Response, m1.status, m1.statusText,
             m1.headers.get('x-a'), m1.type, immutable, await m1.text(), await m2.text(),
             await c.match('https://cache.test/b'), await c.match('https://cache.test/a?q')].join('|');",
    "|true|false|true|201|Made|1|default|TypeError|hola|hola||"
);

c!(
    put_replaces_same_url,
    [],
    "const c = await caches.open('t');
     await c.put('https://cache.test/a', new Response('one'));
     await c.put('https://cache.test/a', new Response('two'));
     const all = await c.matchAll();
     return [all.length, await all[0].text()].join('|');",
    "1|two"
);

c!(
    ignore_search,
    [],
    "const c = await caches.open('s');
     await c.put('https://cache.test/a?x=1', new Response('x1'));
     await c.put('https://cache.test/a?x=2', new Response('x2'));
     const exact = await c.matchAll('https://cache.test/a?x=2');
     const loose = await c.matchAll('https://cache.test/a', { ignoreSearch: true });
     const none = await c.match('https://cache.test/a');
     return [exact.length, await exact[0].text(), loose.length, none].join('|');",
    "1|x2|2|"
);

c!(
    ignore_method,
    [],
    "const c = await caches.open('m');
     await c.put('https://cache.test/a', new Response('a'));
     const post = new Request('https://cache.test/a', { method: 'POST', body: 'x' });
     const strict = await c.match(post);
     const loose = await c.match(post, { ignoreMethod: true });
     const del = await c.delete(post);
     const keys = await c.keys(post);
     const keysLoose = await c.keys(post, { ignoreMethod: true });
     return [strict, await loose.text(), del, keys.length, keysLoose.length].join('|');",
    "|a|false|0|1"
);

c!(
    vary_and_ignore_vary,
    [],
    "const c = await caches.open('v');
     const req = new Request('https://cache.test/v', { headers: { 'Accept-Language': 'es' } });
     await c.put(req, new Response('es', { headers: { Vary: 'Accept-Language' } }));
     const en = new Request('https://cache.test/v', { headers: { 'Accept-Language': 'en' } });
     const es = new Request('https://cache.test/v', { headers: { 'Accept-Language': 'es' } });
     const missEn = await c.match(en);
     const hitEs = await c.match(es);
     const loose = await c.match(en, { ignoreVary: true });
     // Otro put con otra variante no reemplaza la primera.
     await c.put(en, new Response('en', { headers: { Vary: 'Accept-Language' } }));
     const all = await c.matchAll();
     return [missEn, await hitEs.text(), await loose.text(), all.length].join('|');",
    "|es|es|2"
);

c!(
    match_all_and_keys,
    [],
    "const c = await caches.open('k');
     await c.put('https://cache.test/1', new Response('1'));
     await c.put('https://cache.test/2?q', new Response('2'));
     await c.put('https://cache.test/3', new Response('3', { headers: { 'X-N': '3' } }));
     const all = await c.matchAll();
     const texts = await Promise.all(all.map(r => r.text()));
     const keys = await c.keys();
     const one = await c.keys('https://cache.test/2', { ignoreSearch: true });
     let frozen = Object.isFrozen(keys);
     let immutable = '';
     try { keys[0].headers.set('X', '1'); } catch (e) { immutable = e.name; }
     return [texts.join(), keys.map(k => k.url).join(), keys[0] instanceof Request, keys[0].method,
             one.map(k => k.url).join(), frozen, immutable, keys[0].signal instanceof AbortSignal,
             (await c.keys()) !== keys].join('|');",
    "1,2,3|https://cache.test/1,https://cache.test/2?q,https://cache.test/3|true|GET|https://cache.test/2?q|true|TypeError|true|true"
);

c!(
    cache_delete,
    [],
    "const c = await caches.open('d');
     await c.put('https://cache.test/a?1', new Response('a'));
     await c.put('https://cache.test/a?2', new Response('b'));
     await c.put('https://cache.test/c', new Response('c'));
     const miss = await c.delete('https://cache.test/a');
     const hit = await c.delete('https://cache.test/a', { ignoreSearch: true });
     return [miss, hit, (await c.keys()).map(k => k.url).join()].join('|');",
    "false|true|https://cache.test/c"
);

// --- Errores de put ---

c!(
    put_errors,
    [],
    "const c = await caches.open('e');
     const out = [];
     const t = async (f) => { try { await f(); out.push('ok'); } catch (e) { out.push(e.name); } };
     await t(() => c.put(new Request('https://cache.test/p', { method: 'POST', body: 'x' }), new Response('x')));
     await t(() => c.put('data:text/plain,hola', new Response('x')));
     await t(() => c.put('https://cache.test/206', new Response('x', { status: 206 })));
     await t(() => c.put('https://cache.test/star', new Response('x', { headers: { Vary: 'Accept, *' } })));
     const used = new Response('x'); await used.text();
     await t(() => c.put('https://cache.test/used', used));
     await t(() => c.put('https://cache.test/notresp', 'x'));
     await t(() => c.put('https://cache.test/ok', new Response(null, { status: 204 })));
     const p = c.put('https://cache.test/sync', new Response('x', { status: 206 }));
     out.push(p instanceof Promise);
     await p.catch(() => {});
     return out.join() + '|' + (await c.keys()).map(k => k.url).join();",
    "TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,ok,true|https://cache.test/ok"
);

// --- add / addAll ---

c!(
    add_and_add_all,
    [
        "/dir/a.txt" => "texto a",
        "/dir/b.txt" => "texto b",
        "/dir/c.txt" => "texto c",
    ],
    "const c = await caches.open('net');
     const r = await c.add('a.txt');
     await c.addAll(['b.txt', new Request('c.txt')]);
     const m = await c.match('https://cache.test/dir/b.txt');
     const keys = await c.keys();
     return [r, keys.map(k => k.url.slice(-5)).join(), await m.text(), m.status, m.type,
             m.headers.get('content-type'), m.url].join('|');",
    "|a.txt,b.txt,c.txt|texto b|200|basic|text/plain|https://cache.test/dir/b.txt"
);

c!(
    add_all_rejects_and_stores_nothing,
    ["/dir/a.txt" => "texto a"],
    "const c = await caches.open('net');
     let e1 = '', e2 = '', e3 = '', e4 = '';
     try { await c.addAll(['a.txt', 'missing.txt']); } catch (e) { e1 = e.name; }
     try { await c.add('missing.txt'); } catch (e) { e2 = e.name; }
     try { await c.addAll(['a.txt', 'a.txt']); } catch (e) { e3 = e.name; }
     try { await c.add(new Request('a.txt', { method: 'POST', body: 'x' })); } catch (e) { e4 = e.name; }
     return [e1, e2, e3, e4, (await c.keys()).length].join('|');",
    "TypeError|TypeError|InvalidStateError|TypeError|0"
);

// --- Compartido entre agentes ---

#[test]
fn same_partition_shares_caches_other_partitions_do_not() {
    let part = bf_web::workers::new_partition();
    let mut a = Page::at(&[], PAGE_URL, part);
    assert_eq!(
        a.run(
            "caches.open('shared').then(c => c.put('https://cache.test/s', new Response('de A')))
             .then(() => finish('ok'), fail);"
        ),
        "ok"
    );
    let mut b = Page::at(&[], "https://cache.test/otra.html", part);
    assert_eq!(
        b.run(
            "Promise.all([caches.keys(), caches.match('https://cache.test/s')])
             .then(([k, r]) => r.text().then(t => finish(k.join() + '|' + t)), fail);"
        ),
        "shared|de A"
    );
    let mut c = Page::at(&[], PAGE_URL, bf_web::workers::new_partition());
    assert_eq!(
        c.run(
            "Promise.all([caches.keys(), caches.match('https://cache.test/s')])
             .then(([k, r]) => finish(k.length + '|' + r), fail);"
        ),
        "0|undefined"
    );
    // Otro origen en la misma partición tampoco la ve.
    let mut d = Page::at(&[], "https://otro.test/", part);
    assert_eq!(
        d.run("caches.keys().then(k => finish(k.length), fail);"),
        "0"
    );
}

// --- Workers ---

#[test]
fn caches_in_dedicated_worker() {
    let mut p = Page::new(&[(
        "/dir/w.js",
        "if (typeof caches === 'undefined') { postMessage('sin caches'); } else {
           caches.open('w')
             .then(c => c.put('https://cache.test/w', new Response('del worker')).then(() => c.match('https://cache.test/w')))
             .then(r => r.text())
             .then(t => postMessage(t + '|' + (self.caches === caches)), e => postMessage('ERR ' + e));
         }",
    )]);
    assert_eq!(
        p.run(
            "const w = new Worker('w.js');
             w.onmessage = e => caches.match('https://cache.test/w', { cacheName: 'w' })
               .then(r => r ? r.text() : 'nada')
               .then(t => finish(e.data + '|' + t), fail);"
        ),
        "del worker|true|del worker"
    );
}
