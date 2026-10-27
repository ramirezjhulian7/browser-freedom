//! Workers (`bf_web::workers`): `Worker`, `SharedWorker`, sus globales, los mensajes entre
//! hilos (structured clone y transferencia), los `MessagePort` entre agentes y la memoria
//! compartida con `Atomics`.

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
        let ct = if path.ends_with(".js") || path.ends_with(".mjs") {
            "text/javascript"
        } else {
            "text/plain"
        };
        headers.append("Content-Type", ct);
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

impl Page {
    fn new(files: &[(&str, &str)]) -> Page {
        Page::with_isolation(files, false)
    }

    fn with_isolation(files: &[(&str, &str)], isolated: bool) -> Page {
        let console = Arc::new(Mutex::new(Vec::new()));
        let mut rt = Runtime::new(Box::new(Hooks(console.clone())));
        let (doc, _) =
            bf_html::parse_document("<!DOCTYPE html><p>x", &bf_html::ParseOptions::default());
        let realm = rt.main_realm();
        let url = Url::parse("http://workers.test/dir/page.html", None).expect("url");
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
        if isolated {
            rt.agent
                .set_realm_data(realm, bf_web::workers::Isolation(true));
        }
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
        self.exec(&format!("try {{ {src} }} catch (e) {{ fail(e); }}"));
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

macro_rules! w {
    ($name:ident, [$($file:expr => $text:expr),* $(,)?], $src:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let mut p = Page::new(&[$(($file, $text)),*]);
            assert_eq!(p.run($src), $expected);
        }
    };
}

// --- Worker: mensajes ---

w!(
    echo,
    ["/dir/echo.js" => "onmessage = e => postMessage(e.data + '!');"],
    "var w = new Worker('echo.js'); w.onmessage = e => finish(e.data + ' ' + (e instanceof MessageEvent) + ' ' + e.origin + '|'); w.postMessage('hola');",
    "hola! true |"
);

w!(
    messages_before_load_are_queued_in_order,
    ["/dir/echo.js" => "onmessage = e => postMessage(e.data);"],
    "var w = new Worker('echo.js'); var got = []; w.onmessage = e => { got.push(e.data); if (got.length == 3) finish(got.join()); }; w.postMessage(1); w.postMessage(2); w.postMessage(3);",
    "1,2,3"
);

w!(
    worker_posts_on_start,
    ["/dir/hi.js" => "postMessage('listo');"],
    "new Worker('hi.js').onmessage = e => finish(e.data);",
    "listo"
);

w!(
    add_event_listener_on_worker,
    ["/dir/hi.js" => "postMessage('x');"],
    "new Worker('hi.js').addEventListener('message', e => finish('listener ' + e.data));",
    "listener x"
);

w!(
    worker_add_event_listener_on_global,
    ["/dir/w.js" => "self.addEventListener('message', e => self.postMessage(e.data * 2));"],
    "var w = new Worker('w.js'); w.onmessage = e => finish(e.data); w.postMessage(21);",
    "42"
);

// --- El global ---

w!(
    global_scope_chain,
    ["/dir/g.js" => "postMessage([self instanceof DedicatedWorkerGlobalScope, self instanceof WorkerGlobalScope, self instanceof EventTarget, self === globalThis, Object.prototype.toString.call(self)].join());"],
    "new Worker('g.js').onmessage = e => finish(e.data);",
    "true,true,true,true,[object DedicatedWorkerGlobalScope]"
);

w!(
    window_only_interfaces_are_absent,
    ["/dir/g.js" => "postMessage([typeof window, typeof document, typeof Node, typeof Element, typeof Window, typeof SharedWorker].join());"],
    "new Worker('g.js').onmessage = e => finish(e.data);",
    "undefined,undefined,undefined,undefined,undefined,undefined"
);

w!(
    worker_interfaces_are_present,
    ["/dir/g.js" => "postMessage([typeof EventTarget, typeof Event, typeof MessageEvent, typeof ErrorEvent, typeof DOMException, typeof fetch, typeof XMLHttpRequest, typeof Blob, typeof MessageChannel, typeof Worker, typeof URL, typeof TextEncoder, typeof setTimeout, typeof importScripts, typeof structuredClone].join());"],
    "new Worker('g.js').onmessage = e => finish(e.data);",
    "function,function,function,function,function,function,function,function,function,function,function,function,function,function,function"
);

w!(
    worker_location,
    ["/dir/sub/l.js" => "postMessage([location.href, location.pathname, location.origin, location.protocol, location.host, location.search, location.hash, String(location), location instanceof WorkerLocation].join(' '));"],
    "new Worker('sub/l.js?a=1#f').onmessage = e => finish(e.data);",
    "http://workers.test/dir/sub/l.js?a=1#f /dir/sub/l.js http://workers.test http: workers.test ?a=1 #f http://workers.test/dir/sub/l.js?a=1#f true"
);

w!(
    worker_navigator_matches_page,
    ["/dir/n.js" => "postMessage([navigator.userAgent, navigator instanceof WorkerNavigator, navigator.onLine, navigator.hardwareConcurrency > 0].join('|'));"],
    "new Worker('n.js').onmessage = e => finish(e.data == [navigator.userAgent, true, true, true].join('|'));",
    "true"
);

w!(
    worker_name_option,
    ["/dir/n.js" => "postMessage(self.name + '|' + typeof self.name);"],
    "new Worker('n.js', {name: 'trabajador'}).onmessage = e => finish(e.data);",
    "trabajador|string"
);

w!(
    self_is_replaceable,
    ["/dir/s.js" => "var g = self; self = 5; postMessage.call(g, String(g.self));"],
    "new Worker('s.js').onmessage = e => finish(e.data);",
    "5"
);

// --- importScripts ---

w!(
    import_scripts_in_order,
    [
        "/dir/a.js" => "var order = ['a'];",
        "/dir/b.js" => "order.push('b');",
        "/dir/main.js" => "importScripts('a.js', 'b.js'); order.push('main'); postMessage(order.join());",
    ],
    "new Worker('main.js').onmessage = e => finish(e.data);",
    "a,b,main"
);

w!(
    import_scripts_network_error,
    ["/dir/main.js" => "try { importScripts('nope.js'); postMessage('no'); } catch (e) { postMessage(e.name + ' ' + (e instanceof DOMException)); }"],
    "new Worker('main.js').onmessage = e => finish(e.data);",
    "NetworkError true"
);

w!(
    import_scripts_bad_url,
    ["/dir/main.js" => "try { importScripts('http://[bad'); } catch (e) { postMessage(e.name); }"],
    "new Worker('main.js').onmessage = e => finish(e.data);",
    "SyntaxError"
);

w!(
    import_scripts_rethrows,
    [
        "/dir/boom.js" => "throw new RangeError('boom');",
        "/dir/main.js" => "try { importScripts('boom.js'); } catch (e) { postMessage(e.name + ': ' + e.message); }",
    ],
    "new Worker('main.js').onmessage = e => finish(e.data);",
    "RangeError: boom"
);

// --- Módulos ---

w!(
    module_worker_imports,
    [
        "/dir/dep.mjs" => "export const x = 40;",
        "/dir/m.mjs" => "import { x } from './dep.mjs'; postMessage(x + 2);",
    ],
    "new Worker('m.mjs', {type: 'module'}).onmessage = e => finish(e.data);",
    "42"
);

w!(
    module_worker_rejects_import_scripts,
    ["/dir/m.mjs" => "try { importScripts('x.js'); } catch (e) { postMessage(e.name); }"],
    "new Worker('m.mjs', {type: 'module'}).onmessage = e => finish(e.data);",
    "TypeError"
);

// --- Structured clone ---

w!(
    clone_graph,
    ["/dir/c.js" => "onmessage = e => { var d = e.data; postMessage([d.self === d, d.m.get('k'), d.s.has(2), d.d.getTime(), d.re.source, d.err instanceof TypeError, d.err.message, d.arr.length, 1 in d.arr, typeof d.big].join()); };"],
    "var o = {m: new Map([['k', 'v']]), s: new Set([2]), d: new Date(7), re: /x+/g, err: new TypeError('t'), arr: [1, , 3], big: 5n}; o.self = o; var w = new Worker('c.js'); w.onmessage = e => finish(e.data); w.postMessage(o);",
    "true,v,true,7,x+,true,t,3,false,bigint"
);

w!(
    clone_back_to_page,
    ["/dir/c.js" => "postMessage({a: [1, 2], b: {c: 'd'}, u: new Uint8Array([9, 8])});"],
    "new Worker('c.js').onmessage = e => finish(JSON.stringify(e.data.a) + e.data.b.c + e.data.u[1] + (e.data.u instanceof Uint8Array));",
    "[1,2]d8true"
);

w!(
    clone_function_throws_in_page,
    ["/dir/c.js" => ""],
    "var w = new Worker('c.js'); try { w.postMessage(function () {}); finish('no'); } catch (e) { finish(e.name + ' ' + (e instanceof DOMException)); }",
    "DataCloneError true"
);

w!(
    clone_error_in_worker_is_catchable,
    ["/dir/c.js" => "try { postMessage(Symbol()); } catch (e) { postMessage(e.name); }"],
    "new Worker('c.js').onmessage = e => finish(e.data);",
    "DataCloneError"
);

w!(
    transfer_array_buffer,
    ["/dir/t.js" => "onmessage = e => postMessage(e.data.byteLength + ':' + new Uint8Array(e.data)[2]);"],
    "var b = new Uint8Array([1, 2, 3]).buffer; var w = new Worker('t.js'); w.onmessage = e => finish(e.data + ' ' + b.byteLength + ' ' + b.detached); w.postMessage(b, [b]);",
    "3:3 0 true"
);

w!(
    transfer_with_options_object,
    ["/dir/t.js" => "onmessage = e => postMessage(e.data.byteLength);"],
    "var b = new ArrayBuffer(16); var w = new Worker('t.js'); w.onmessage = e => finish(e.data + ' ' + b.byteLength); w.postMessage(b, {transfer: [b]});",
    "16 0"
);

w!(
    duplicate_transfer_throws,
    ["/dir/t.js" => ""],
    "var b = new ArrayBuffer(1); var w = new Worker('t.js'); try { w.postMessage(b, [b, b]); finish('no'); } catch (e) { finish(e.name); }",
    "DataCloneError"
);

w!(
    blob_crosses_threads,
    ["/dir/b.js" => "onmessage = async e => postMessage(e.data.type + ' ' + e.data.size + ' ' + await e.data.text() + ' ' + (e.data instanceof Blob));"],
    "var w = new Worker('b.js'); w.onmessage = e => finish(e.data); w.postMessage(new Blob(['hola'], {type: 'text/plain'}));",
    "text/plain 4 hola true"
);

w!(
    shared_array_buffer_needs_isolation,
    ["/dir/s.js" => ""],
    "var w = new Worker('s.js'); try { w.postMessage(new SharedArrayBuffer(4)); finish('no'); } catch (e) { finish(e.name); }",
    "DataCloneError"
);

#[test]
fn shared_memory_and_atomics_between_threads() {
    let mut p = Page::with_isolation(
        &[(
            "/dir/a.js",
            "onmessage = e => { var ia = new Int32Array(e.data); Atomics.add(ia, 1, 5); \
             postMessage('esperando'); var r = Atomics.wait(ia, 0, 0, 10000); \
             postMessage(r + ' ' + Atomics.load(ia, 0) + ' ' + crossOriginIsolated); };",
        )],
        true,
    );
    let r = p.run(
        "var sab = new SharedArrayBuffer(8); var ia = new Int32Array(sab); var w = new Worker('a.js'); \
         w.onmessage = e => { if (e.data == 'esperando') { var t = setInterval(() => { Atomics.store(ia, 0, 7); \
         if (Atomics.notify(ia, 0) == 1) clearInterval(t); }, 1); } else finish(e.data + ' ' + Atomics.load(ia, 1)); }; \
         w.postMessage(sab);",
    );
    assert_eq!(r, "ok 7 true 5");
}

#[test]
fn wait_async_in_page_woken_by_worker() {
    let mut p = Page::with_isolation(
        &[(
            "/dir/n.js",
            "onmessage = e => { var ia = new Int32Array(e.data); Atomics.store(ia, 0, 1); \
             var n = 0; var t = setInterval(() => { n = Atomics.notify(ia, 0); if (n) { clearInterval(t); } }, 1); };",
        )],
        true,
    );
    let r = p.run(
        "var ia = new Int32Array(new SharedArrayBuffer(4)); var res = Atomics.waitAsync(ia, 0, 0); \
         res.value.then(v => finish(res.async + ' ' + v)); new Worker('n.js').postMessage(ia.buffer);",
    );
    assert_eq!(r, "true ok");
}

// --- Errores ---

w!(
    uncaught_error_reaches_worker_object,
    ["/dir/e.js" => "\n\nthrow new Error('falla');"],
    "new Worker('e.js').onerror = e => { e.preventDefault(); finish([e instanceof ErrorEvent, /falla/.test(e.message), /e\\.js$/.test(e.filename), e.lineno > 0].join()); };",
    "true,true,true,true"
);

w!(
    error_handled_in_worker_does_not_propagate,
    ["/dir/e.js" => "self.onerror = () => { postMessage('atrapado'); return true; }; setTimeout(() => { throw new Error('x'); }, 0);"],
    "var w = new Worker('e.js'); w.onerror = () => finish('propagó'); w.onmessage = e => setTimeout(() => finish(e.data), 50);",
    "atrapado"
);

w!(
    global_onerror_arguments,
    ["/dir/e.js" => "self.onerror = (msg, file, line, col, err) => { postMessage([typeof msg, typeof file, typeof line, err instanceof TypeError].join()); return true; }; null.x;"],
    "new Worker('e.js').onmessage = e => finish(e.data);",
    "string,string,number,true"
);

w!(
    load_error_is_simple_event,
    [],
    "new Worker('no-existe.js').onerror = e => finish(e.constructor.name + ' ' + e.type + ' ' + (e instanceof ErrorEvent));",
    "Event error false"
);

w!(
    syntax_error_in_worker_script,
    ["/dir/s.js" => "var = ;"],
    "new Worker('s.js').onerror = e => { e.preventDefault(); finish(e.type + ' ' + /SyntaxError/.test(e.message)); };",
    "error true"
);

w!(
    unhandled_rejection_in_worker,
    ["/dir/r.js" => "self.onunhandledrejection = e => postMessage('rechazo ' + e.reason); Promise.reject('motivo');"],
    "new Worker('r.js').onmessage = e => finish(e.data);",
    "rechazo motivo"
);

// --- Constructor ---

w!(
    bad_url_is_syntax_error,
    [],
    "try { new Worker('http://[x'); finish('no'); } catch (e) { finish(e.name + ' ' + (e instanceof DOMException)); }",
    "SyntaxError true"
);

w!(
    bad_type_is_type_error,
    [],
    "try { new Worker('a.js', {type: 'otro'}); finish('no'); } catch (e) { finish(e.name); }",
    "TypeError"
);

w!(
    needs_new,
    [],
    "try { Worker('a.js'); finish('no'); } catch (e) { finish(e.name); }",
    "TypeError"
);

w!(
    data_url_worker,
    [],
    "new Worker('data:text/javascript,postMessage(location.protocol)').onmessage = e => finish(e.data);",
    "data:"
);

w!(
    blob_url_worker,
    [],
    "var u = URL.createObjectURL(new Blob(['postMessage(6 * 7)'], {type: 'text/javascript'})); new Worker(u).onmessage = e => finish(e.data);",
    "42"
);

// --- Ciclo de vida ---

w!(
    terminate_stops_messages,
    ["/dir/t.js" => "setInterval(() => postMessage('tic'), 1);"],
    "var w = new Worker('t.js'); var n = 0; w.onmessage = () => { n++; if (n == 1) { w.terminate(); setTimeout(() => finish('mensajes ' + n), 100); } };",
    "mensajes 1"
);

w!(
    close_discards_later_messages,
    ["/dir/c.js" => "onmessage = e => { postMessage('antes ' + e.data); close(); postMessage('igual sale'); };"],
    "var w = new Worker('c.js'); var got = []; w.onmessage = e => { got.push(e.data); if (got.length == 2) setTimeout(() => finish(got.join('|')), 150); }; w.postMessage(1); w.postMessage(2);",
    "antes 1|igual sale"
);

w!(
    timers_in_worker,
    ["/dir/t.js" => "var n = 0; var id = setInterval(() => { if (++n == 3) { clearInterval(id); setTimeout(() => postMessage('n=' + n), 5); } }, 1);"],
    "new Worker('t.js').onmessage = e => finish(e.data);",
    "n=3"
);

w!(
    microtasks_in_worker,
    ["/dir/m.js" => "var o = []; Promise.resolve().then(() => o.push('p')); queueMicrotask(() => o.push('q')); o.push('s'); setTimeout(() => postMessage(o.join()), 0);"],
    "new Worker('m.js').onmessage = e => finish(e.data);",
    "s,p,q"
);

w!(
    nested_worker,
    [
        "/dir/inner.js" => "postMessage('adentro ' + self.name);",
        "/dir/outer.js" => "var w = new Worker('inner.js', {name: 'hijo'}); w.onmessage = e => postMessage('afuera: ' + e.data);",
    ],
    "new Worker('outer.js').onmessage = e => finish(e.data);",
    "afuera: adentro hijo"
);

w!(
    console_is_forwarded,
    ["/dir/c.js" => "console.log('desde el worker'); postMessage('ok');"],
    "new Worker('c.js').onmessage = e => setTimeout(() => finish(e.data), 20);",
    "ok"
);

// --- Red en el worker ---

w!(
    fetch_in_worker,
    [
        "/dir/data.txt" => "contenido",
        "/dir/f.js" => "fetch('data.txt').then(r => r.text()).then(t => postMessage(t));",
    ],
    "new Worker('f.js').onmessage = e => finish(e.data);",
    "contenido"
);

w!(
    xhr_in_worker,
    [
        "/dir/data.txt" => "xhr",
        "/dir/x.js" => "var x = new XMLHttpRequest(); x.open('GET', 'data.txt'); x.onload = () => postMessage(x.status + ' ' + x.responseText); x.send();",
    ],
    "new Worker('x.js').onmessage = e => finish(e.data);",
    "200 xhr"
);

// --- MessagePort entre hilos ---

w!(
    message_port_to_worker,
    ["/dir/p.js" => "onmessage = e => { var port = e.ports[0]; port.onmessage = m => port.postMessage('eco ' + m.data); };"],
    "var w = new Worker('p.js'); var ch = new MessageChannel(); w.postMessage('toma', [ch.port2]); ch.port1.onmessage = e => finish(e.data); ch.port1.postMessage('hola');",
    "eco hola"
);

w!(
    message_port_from_worker,
    ["/dir/p.js" => "var ch = new MessageChannel(); postMessage('puerto', [ch.port1]); ch.port2.onmessage = e => ch.port2.postMessage(e.data * 3);"],
    "new Worker('p.js').onmessage = e => { var port = e.ports[0]; port.onmessage = m => finish(m.data); port.postMessage(14); };",
    "42"
);

w!(
    messages_wait_for_start,
    ["/dir/p.js" => "onmessage = e => { var port = e.ports[0]; port.postMessage('a'); port.postMessage('b'); };"],
    "var w = new Worker('p.js'); var ch = new MessageChannel(); w.postMessage(0, [ch.port2]); var got = []; setTimeout(() => { ch.port1.addEventListener('message', e => { got.push(e.data); if (got.length == 2) finish(got.join()); }); ch.port1.start(); }, 50);",
    "a,b"
);

w!(
    port_between_two_workers,
    [
        "/dir/a.js" => "onmessage = e => e.ports[0].postMessage('de a');",
        "/dir/b.js" => "onmessage = e => { e.ports[0].onmessage = m => postMessage('b recibió ' + m.data); };",
    ],
    "var ch = new MessageChannel(); var b = new Worker('b.js'); b.onmessage = e => finish(e.data); b.postMessage(0, [ch.port2]); new Worker('a.js').postMessage(0, [ch.port1]);",
    "b recibió de a"
);

w!(
    transferred_port_is_neutered,
    ["/dir/p.js" => ""],
    "var ch = new MessageChannel(); new Worker('p.js').postMessage(0, [ch.port1]); try { new Worker('p.js').postMessage(0, [ch.port1]); finish('no'); } catch (e) { finish(e.name); }",
    "DataCloneError"
);

// --- SharedWorker ---

w!(
    shared_worker_connect,
    ["/dir/sw.js" => "var n = 0; onconnect = e => { n++; var p = e.ports[0]; p.postMessage('conexión ' + n + ' ' + (self instanceof SharedWorkerGlobalScope) + ' ' + (e.source === undefined || e.source === null || e.source === p)); };"],
    "var s = new SharedWorker('sw.js'); s.port.onmessage = e => finish(e.data + ' ' + (s.port instanceof MessagePort));",
    "conexión 1 true true true"
);

w!(
    shared_worker_is_shared,
    ["/dir/sw2.js" => "var n = 0; onconnect = e => { n++; e.ports[0].postMessage(n); };"],
    "var a = new SharedWorker('sw2.js', 'mismo'); var b = new SharedWorker('sw2.js', {name: 'mismo'}); var got = []; function f(e) { got.push(e.data); if (got.length == 2) finish(got.sort().join()); } a.port.onmessage = f; b.port.onmessage = f;",
    "1,2"
);

w!(
    shared_worker_names_are_separate,
    ["/dir/sw3.js" => "var n = 0; onconnect = e => { n++; e.ports[0].postMessage(self.name + n); };"],
    "var a = new SharedWorker('sw3.js', 'x'); var b = new SharedWorker('sw3.js', 'y'); var got = []; function f(e) { got.push(e.data); if (got.length == 2) finish(got.sort().join()); } a.port.onmessage = f; b.port.onmessage = f;",
    "x1,y1"
);

w!(
    shared_worker_port_messages,
    ["/dir/sw4.js" => "onconnect = e => { var p = e.ports[0]; p.onmessage = m => p.postMessage(m.data.toUpperCase()); };"],
    "var s = new SharedWorker('sw4.js'); s.port.onmessage = e => finish(e.data); s.port.postMessage('hola');",
    "HOLA"
);
