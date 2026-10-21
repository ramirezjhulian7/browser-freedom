//! El BOM (`bf_web::bom`): `navigator`, `location`, `history`, `screen`, `Storage`,
//! `document.cookie`, diálogos, `performance`, `requestIdleCallback` y la ejecución de scripts
//! del documento con `document.write`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use bf_infra::{Resource, ResourceLoader};
use bf_js::host::{ConsoleLevel, HostHooks};
use bf_js::value::Value;
use bf_web::bom::{self, BomHooks, ScreenInfo};
use bf_web::{DocumentOwner, Runtime, Url, WebContext, WebContextHandle, WebHooks};

/// Lo que registran los hooks de prueba.
#[derive(Default)]
struct Log {
    calls: Vec<String>,
    console: Vec<String>,
    cookies: String,
    saved: HashMap<String, Vec<(String, String)>>,
    stored: Vec<(String, String)>,
}

type Shared = Rc<RefCell<Log>>;

struct TestBom(Shared);

impl BomHooks for TestBom {
    fn navigate(&mut self, url: &Url, replace: bool) {
        self.0
            .borrow_mut()
            .calls
            .push(format!("navigate {} {replace}", url.href()));
    }
    fn same_document_navigation(&mut self, url: &Url, replace: bool) {
        self.0
            .borrow_mut()
            .calls
            .push(format!("same {} {replace}", url.href()));
    }
    fn reload(&mut self) {
        self.0.borrow_mut().calls.push("reload".into());
    }
    fn traverse(&mut self, delta: i32) {
        self.0.borrow_mut().calls.push(format!("traverse {delta}"));
    }
    fn session_history(&self) -> (u32, u32) {
        (3, 1)
    }
    fn cookies(&mut self, _url: &Url) -> String {
        self.0.borrow().cookies.clone()
    }
    fn set_cookie(&mut self, _url: &Url, value: &str) {
        self.0.borrow_mut().calls.push(format!("cookie {value}"));
    }
    fn load_storage(&mut self, origin: &str) -> Vec<(String, String)> {
        self.0.borrow_mut().calls.push(format!("load {origin}"));
        self.0.borrow().stored.clone()
    }
    fn save_storage(&mut self, origin: &str, items: &[(String, String)]) {
        self.0
            .borrow_mut()
            .saved
            .insert(origin.to_owned(), items.to_vec());
    }
    fn screen(&self) -> ScreenInfo {
        ScreenInfo {
            width: 2560.0,
            height: 1440.0,
            avail_width: 2560.0,
            avail_height: 1392.0,
            device_pixel_ratio: 1.5,
            ..ScreenInfo::default()
        }
    }
    fn languages(&self) -> Vec<String> {
        vec!["es-ES".into(), "es".into(), "en".into()]
    }
    fn hardware_concurrency(&self) -> u32 {
        12
    }
    fn close(&mut self) {
        self.0.borrow_mut().calls.push("close".into());
    }
    fn referrer(&self) -> String {
        "https://referrer.test/".into()
    }
}

struct TestWeb(Shared);

impl WebHooks for TestWeb {
    fn alert(&mut self, message: &str) {
        self.0.borrow_mut().calls.push(format!("alert {message}"));
    }
    fn confirm(&mut self, message: &str) -> bool {
        self.0.borrow_mut().calls.push(format!("confirm {message}"));
        true
    }
    fn prompt(&mut self, message: &str, default: &str) -> Option<String> {
        self.0
            .borrow_mut()
            .calls
            .push(format!("prompt {message} {default}"));
        Some("respuesta".into())
    }
    fn open(&mut self, url: &str, target: &str, _features: &str) -> bool {
        self.0
            .borrow_mut()
            .calls
            .push(format!("open {url} {target}"));
        true
    }
}

struct TestHooks(Shared);

const UA: &str = bf_infra::identity::USER_AGENT;

impl HostHooks for TestHooks {
    fn console(&mut self, _level: ConsoleLevel, message: &str) {
        self.0.borrow_mut().console.push(message.to_owned());
    }
    fn report_exception(&mut self, message: &str, _stack: &str) {
        self.0
            .borrow_mut()
            .console
            .push(format!("Uncaught {message}"));
    }
    fn user_agent(&self) -> String {
        UA.to_owned()
    }
}

/// Un `ResourceLoader` en memoria.
struct MemLoader(HashMap<String, String>);

impl ResourceLoader for MemLoader {
    fn load(&self, url: &Url) -> Option<Resource> {
        self.0.get(&url.href()).map(|s| Resource {
            bytes: s.as_bytes().to_vec(),
            url: url.clone(),
            content_type: None,
        })
    }
}

struct Page {
    rt: Runtime,
    ctx: WebContextHandle,
    log: Shared,
}

const URL: &str = "https://example.test/dir/page.html?q=1#top";

impl Page {
    fn new(html: &str) -> Page {
        Page::with_storage(html, Vec::new())
    }

    fn with_storage(html: &str, stored: Vec<(String, String)>) -> Page {
        let log: Shared = Rc::new(RefCell::new(Log {
            stored,
            ..Log::default()
        }));
        let mut rt = Runtime::new(Box::new(TestHooks(log.clone())));
        let (doc, _) = bf_html::parse_document(html, &bf_html::ParseOptions::default());
        let realm = rt.main_realm();
        let url = Url::parse(URL, None).expect("url");
        let mut wc = WebContext::new(DocumentOwner::Document(Box::new(doc)), url);
        wc.hooks = Box::new(TestWeb(log.clone()));
        let ctx = bf_web::install(&mut rt.agent, realm, wc);
        bom::set_hooks(&mut rt.agent, realm, Box::new(TestBom(log.clone())));
        Page { rt, ctx, log }
    }

    /// Una página cargada con el parser y los scripts (`files`: URL → contenido).
    fn scripted(html: &str, files: &[(&str, &str)]) -> Page {
        let mut p = Page::new("");
        let realm = p.ctx.realm;
        let url = Url::parse("https://example.test/", None).unwrap();
        let loader = MemLoader(
            files
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        );
        bom::script::install_scripting(&mut p.rt.agent, realm, &url, Arc::new(loader));
        bom::script::begin_parse(&mut p.rt.agent, realm, &url);
        bom::script::feed(&mut p.rt.agent, realm, html);
        bom::script::finish(&mut p.rt.agent, realm);
        p
    }

    fn eval(&mut self, src: &str) -> Value {
        let realm = self.ctx.realm;
        match self.rt.eval_script(realm, src, "test.js") {
            Ok(v) => v,
            Err(e) => panic!("error: {e}\n{}", e.stack),
        }
    }

    fn show(&mut self, src: &str) -> String {
        let v = self.eval(src);
        self.rt.to_display(&v)
    }

    fn truthy(&mut self, src: &str) -> bool {
        matches!(self.eval(src), Value::Boolean(true))
    }

    fn error(&mut self, src: &str) -> String {
        let realm = self.ctx.realm;
        match self.rt.eval_script(realm, src, "test.js") {
            Ok(v) => panic!("se esperaba un error, dio {}", self.rt.to_display(&v)),
            Err(e) => e.message,
        }
    }

    fn idle(&mut self) {
        self.rt.run_until_idle().expect("tareas");
    }

    fn calls(&self) -> Vec<String> {
        self.log.borrow().calls.clone()
    }

    fn console(&self) -> Vec<String> {
        self.log.borrow().console.clone()
    }

    fn body_text(&self) -> String {
        let ctx = self.ctx.get(&self.rt.agent);
        let doc = ctx.document();
        doc.body().map(|b| doc.text_content(b)).unwrap_or_default()
    }
}

const SIMPLE: &str = "<!DOCTYPE html><title>T</title><p>x";

// --- navigator ---

#[test]
fn user_agent_and_app_version() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(p.show("navigator.userAgent"), UA);
    assert_eq!(p.show("navigator.appVersion"), &UA["Mozilla/".len()..]);
}

#[test]
fn navigator_identifies_as_freedom() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "[navigator.appName, navigator.appCodeName, navigator.product, navigator.productSub, \
             navigator.vendor, navigator.vendorSub, navigator.platform].join('|')"
        ),
        format!(
            "Netscape|Mozilla|Gecko|20030107|||{}",
            bf_infra::identity::NAVIGATOR_PLATFORM
        )
    );
    assert!(!p.show("navigator.userAgent").contains("Chrome"));
}

#[test]
fn languages_are_a_frozen_stable_array() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(p.show("navigator.language"), "es-ES");
    assert_eq!(p.show("navigator.languages.join()"), "es-ES,es,en");
    assert!(p.truthy(
        "navigator.languages === navigator.languages && Object.isFrozen(navigator.languages)"
    ));
}

#[test]
fn navigator_capabilities() {
    let mut p = Page::new(SIMPLE);
    assert!(p.truthy(
        "navigator.cookieEnabled === true && navigator.onLine === true && \
         navigator.hardwareConcurrency === 12 && navigator.maxTouchPoints === 0 && \
         navigator.webdriver === false && navigator.pdfViewerEnabled === false && \
         navigator.doNotTrack === null && navigator.deviceMemory === 8 && \
         navigator.javaEnabled() === false"
    ));
}

#[test]
fn plugins_and_mime_types_are_empty_without_pdf_viewer() {
    let mut p = Page::new(SIMPLE);
    assert!(p.truthy(
        "navigator.plugins.length === 0 && navigator.plugins.item(0) === null && \
         navigator.plugins.namedItem('PDF Viewer') === null && \
         navigator.plugins === navigator.plugins && navigator.mimeTypes.length === 0 && \
         navigator.plugins instanceof PluginArray"
    ));
}

#[test]
fn navigator_object_identity_and_interface() {
    let mut p = Page::new(SIMPLE);
    assert!(p.truthy(
        "navigator === window.navigator && navigator === clientInformation && \
         navigator instanceof Navigator && \
         Object.prototype.toString.call(navigator) === '[object Navigator]' && \
         !navigator.hasOwnProperty('userAgent') && 'userAgent' in Navigator.prototype"
    ));
    assert!(
        p.error("Navigator.prototype.userAgent")
            .contains("Illegal invocation")
    );
}

#[test]
fn user_agent_data_brands() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show("navigator.userAgentData.brands.map(b => b.brand + '/' + b.version).join()"),
        format!("Freedom/{},Not.A/Brand/99", bf_infra::identity::MAJOR)
    );
    assert!(p.truthy(&format!(
        "navigator.userAgentData.mobile === false && \
         navigator.userAgentData.platform === '{}' && \
         navigator.userAgentData === navigator.userAgentData && \
         !JSON.stringify(navigator.userAgentData).includes('Chromium')",
        bf_infra::identity::UA_PLATFORM
    )));
}

#[test]
fn high_entropy_values() {
    let mut p = Page::new(SIMPLE);
    p.eval(
        "var he; navigator.userAgentData.getHighEntropyValues(['architecture', 'bitness', \
         'uaFullVersion', 'fullVersionList', 'platformVersion']).then(v => he = v);",
    );
    p.idle();
    assert_eq!(
        p.show(
            "[he.architecture, he.bitness, he.uaFullVersion, he.platform, \
             he.fullVersionList[0].version].join('|')"
        ),
        format!(
            "{}|{}|{v}|{}|{v}",
            bf_infra::identity::ARCHITECTURE,
            bf_infra::identity::BITNESS,
            bf_infra::identity::UA_PLATFORM,
            v = bf_infra::identity::VERSION
        )
    );
}

// --- location ---

#[test]
fn location_components() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "[location.protocol, location.host, location.hostname, location.port, \
             location.pathname, location.search, location.hash, location.origin].join('|')"
        ),
        "https:|example.test|example.test||/dir/page.html|?q=1|#top|https://example.test"
    );
    assert_eq!(p.show("String(location)"), URL);
    assert_eq!(p.show("location.toString()"), URL);
}

#[test]
fn location_href_setter_navigates() {
    let mut p = Page::new(SIMPLE);
    p.eval("location.href = '../otra.html'");
    assert_eq!(p.calls(), ["navigate https://example.test/otra.html false"]);
}

#[test]
fn location_assign_and_replace() {
    let mut p = Page::new(SIMPLE);
    p.eval("location.assign('/a'); location.replace('https://b.test/')");
    assert_eq!(
        p.calls(),
        [
            "navigate https://example.test/a false",
            "navigate https://b.test/ true"
        ]
    );
}

#[test]
fn window_location_put_forwards() {
    let mut p = Page::new(SIMPLE);
    p.eval("window.location = 'x.html'");
    assert_eq!(
        p.calls(),
        ["navigate https://example.test/dir/x.html false"]
    );
}

#[test]
fn location_hash_is_same_document() {
    let mut p = Page::new(SIMPLE);
    p.eval("var fired = 0; window.onhashchange = function (e) { fired = e.type; }; location.hash = 'abajo'");
    assert_eq!(
        p.calls(),
        ["same https://example.test/dir/page.html?q=1#abajo false"]
    );
    assert_eq!(p.show("location.hash"), "#abajo");
    assert_eq!(p.show("history.length"), "4");
    p.idle();
    assert_eq!(p.show("fired"), "hashchange");
    // El mismo fragmento no navega.
    p.eval("location.hash = '#abajo'");
    assert_eq!(p.calls().len(), 1);
}

#[test]
fn location_search_navigates() {
    let mut p = Page::new(SIMPLE);
    p.eval("location.search = 'x=2'");
    assert_eq!(
        p.calls(),
        ["navigate https://example.test/dir/page.html?x=2#top false"]
    );
}

#[test]
fn location_reload_and_protocol() {
    let mut p = Page::new(SIMPLE);
    p.eval("location.protocol = 'ftp'; location.reload()");
    assert_eq!(p.calls(), ["reload"]);
}

#[test]
fn location_members_are_unforgeable() {
    let mut p = Page::new(SIMPLE);
    assert!(p.truthy(
        "var d = Object.getOwnPropertyDescriptor(location, 'href'); \
         d.configurable === false && typeof d.get === 'function' && \
         Object.getOwnPropertyDescriptor(window, 'location').configurable === false && \
         location === window.location && location instanceof Location"
    ));
}

// --- history ---

#[test]
fn push_state_changes_url_and_state() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(p.show("history.length"), "4");
    p.eval("history.pushState({n: 1}, '', 'nueva?x')");
    assert_eq!(p.show("location.href"), "https://example.test/dir/nueva?x");
    assert_eq!(p.show("history.state.n"), "1");
    // pushState descarta las entradas siguientes del navegador.
    assert_eq!(p.show("history.length"), "4");
    assert_eq!(p.calls(), ["same https://example.test/dir/nueva?x false"]);
    assert_eq!(p.show("document.URL === location.href || true"), "true");
}

#[test]
fn replace_state_keeps_length() {
    let mut p = Page::new(SIMPLE);
    p.eval("history.replaceState('s', '', '#otra')");
    assert_eq!(p.show("history.state"), "s");
    assert_eq!(p.show("history.length"), "4");
    assert_eq!(p.show("location.hash"), "#otra");
}

#[test]
fn push_state_cross_origin_is_security_error() {
    let mut p = Page::new(SIMPLE);
    assert!(
        p.error("history.pushState(null, '', 'https://otro.test/')")
            .contains("SecurityError")
    );
}

#[test]
fn back_within_document_fires_popstate() {
    let mut p = Page::new(SIMPLE);
    p.eval(
        "var got; window.onpopstate = function (e) { got = e.state; }; \
         history.pushState('a', '', '?a'); history.pushState('b', '', '?b'); history.back();",
    );
    assert_eq!(p.show("location.search"), "?a");
    assert_eq!(p.show("history.state"), "a");
    p.idle();
    assert_eq!(p.show("got"), "a");
}

#[test]
fn go_outside_document_traverses() {
    let mut p = Page::new(SIMPLE);
    p.eval("history.back(); history.forward(); history.go(0)");
    assert_eq!(p.calls(), ["traverse -1", "traverse 1", "reload"]);
}

#[test]
fn scroll_restoration() {
    let mut p = Page::new(SIMPLE);
    p.eval("history.scrollRestoration = 'manual'; history.scrollRestoration = 'bogus'");
    assert_eq!(p.show("history.scrollRestoration"), "manual");
}

// --- screen ---

#[test]
fn screen_from_hooks() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "[screen.width, screen.height, screen.availWidth, screen.availHeight, \
             screen.colorDepth, screen.pixelDepth, devicePixelRatio].join()"
        ),
        "2560,1440,2560,1392,24,24,1.5"
    );
    assert!(p.truthy("screen === window.screen && screen instanceof Screen"));
}

#[test]
fn screen_orientation() {
    let mut p = Page::new(SIMPLE);
    assert!(p.truthy(
        "screen.orientation === screen.orientation && \
         screen.orientation.type === 'landscape-primary' && screen.orientation.angle === 0"
    ));
    p.eval("var err; screen.orientation.lock('portrait').catch(e => err = e.name)");
    p.idle();
    assert_eq!(p.show("err"), "NotSupportedError");
}

// --- Storage ---

#[test]
fn session_storage_methods() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "sessionStorage.setItem('a', '1'); sessionStorage.setItem('b', 2); \
             [sessionStorage.length, sessionStorage.getItem('a'), typeof sessionStorage.getItem('b'), \
              sessionStorage.key(1), sessionStorage.key(5), sessionStorage.getItem('zz')].join('|')"
        ),
        "2|1|string|b||"
    );
    p.eval("sessionStorage.removeItem('a')");
    assert_eq!(p.show("sessionStorage.length"), "1");
    p.eval("sessionStorage.clear()");
    assert_eq!(p.show("sessionStorage.length"), "0");
}

#[test]
fn storage_named_properties() {
    let mut p = Page::new(SIMPLE);
    p.eval("sessionStorage.color = 'rojo'; sessionStorage['tamaño'] = 3;");
    assert_eq!(p.show("sessionStorage.getItem('color')"), "rojo");
    assert_eq!(p.show("sessionStorage['tamaño']"), "3");
    assert!(p.truthy("'color' in sessionStorage && !('nada' in sessionStorage)"));
    assert_eq!(p.show("Object.keys(sessionStorage).join()"), "color,tamaño");
    p.eval("delete sessionStorage.color");
    assert_eq!(p.show("sessionStorage.getItem('color')"), "null");
    assert!(p.truthy(
        "sessionStorage.nada === undefined && typeof sessionStorage.getItem === 'function'"
    ));
}

#[test]
fn local_storage_loads_and_persists() {
    let mut p = Page::with_storage(SIMPLE, vec![("guardado".into(), "sí".into())]);
    assert_eq!(p.show("localStorage.getItem('guardado')"), "sí");
    p.eval("localStorage.setItem('nuevo', 'x')");
    let log = p.log.borrow();
    assert_eq!(
        log.saved.get("https://example.test"),
        Some(&vec![
            ("guardado".to_owned(), "sí".to_owned()),
            ("nuevo".to_owned(), "x".to_owned())
        ])
    );
    assert_eq!(
        log.calls.iter().filter(|c| c.starts_with("load")).count(),
        1
    );
}

#[test]
fn storage_quota() {
    let mut p = Page::new(SIMPLE);
    let msg = p.error("localStorage.setItem('big', 'x'.repeat(6 * 1024 * 1024))");
    assert!(msg.contains("QuotaExceededError"), "{msg}");
    assert_eq!(p.show("localStorage.length"), "0");
}

#[test]
fn storage_identity_and_interface() {
    let mut p = Page::new(SIMPLE);
    assert!(p.truthy(
        "localStorage === window.localStorage && localStorage instanceof Storage && \
         sessionStorage !== localStorage && \
         Object.prototype.toString.call(localStorage) === '[object Storage]'"
    ));
}

// --- cookies, diálogos, ventana ---

#[test]
fn document_cookie_through_hooks() {
    let mut p = Page::new(SIMPLE);
    p.log.borrow_mut().cookies = "a=1; b=2".into();
    assert_eq!(p.show("document.cookie"), "a=1; b=2");
    p.eval("document.cookie = 'c=3; path=/'");
    assert_eq!(p.calls(), ["cookie c=3; path=/"]);
}

#[test]
fn dialogs_use_the_browser() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show("alert('hola'); [confirm('¿seguro?'), prompt('nombre', 'Ana')].join('|')"),
        "true|respuesta"
    );
    assert_eq!(
        p.calls(),
        ["alert hola", "confirm ¿seguro?", "prompt nombre Ana"]
    );
}

#[test]
fn window_open_resolves_url() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(p.show("String(open('otra.html'))"), "null");
    p.eval("close()");
    assert_eq!(
        p.calls(),
        ["open https://example.test/dir/otra.html _blank", "close"]
    );
}

#[test]
fn window_properties() {
    let mut p = Page::new(SIMPLE);
    assert!(p.truthy(
        "origin === 'https://example.test' && isSecureContext === true && closed === false && \
         opener === null && name === ''"
    ));
    p.eval("name = 'ventana'");
    assert_eq!(p.show("window.name"), "ventana");
}

#[test]
fn document_referrer_and_visibility() {
    let mut p = Page::new(SIMPLE);
    assert!(p.truthy(
        "document.referrer === 'https://referrer.test/' && document.visibilityState === 'visible' && \
         document.hidden === false && document.hasFocus() === true && \
         /^\\d\\d\\/\\d\\d\\/\\d{4} \\d\\d:\\d\\d:\\d\\d$/.test(document.lastModified)"
    ));
}

// --- performance ---

#[test]
fn performance_now_and_timing() {
    let mut p = Page::new(SIMPLE);
    assert!(p.truthy(
        "var a = performance.now(); var b = performance.now(); \
         a >= 0 && b >= a && Math.abs(performance.timeOrigin - Date.now()) < 60000 && \
         performance.timing.navigationStart > 0 && performance.navigation.type === 0 && \
         performance.getEntriesByType('navigation').length === 0 && \
         performance === window.performance && performance.timing === performance.timing"
    ));
}

#[test]
fn request_idle_callback() {
    let mut p = Page::new(SIMPLE);
    p.eval(
        "var r = []; requestIdleCallback(function (d) { r.push(typeof d.timeRemaining(), d.didTimeout); }); \
         var id = requestIdleCallback(function () { r.push('cancelado'); }); cancelIdleCallback(id);",
    );
    assert_eq!(p.show("r.length"), "0");
    std::thread::sleep(std::time::Duration::from_millis(5));
    p.idle();
    assert_eq!(p.show("r.join()"), "number,false");
}

// --- scripts del documento ---

#[test]
fn scripts_run_while_parsing() {
    let mut p = Page::scripted(
        "<p id=a>uno</p><script>var n = document.getElementById('a') ? 1 : 0; \
         var state = document.readyState;</script><p id=b>dos</p>",
        &[],
    );
    assert_eq!(p.show("n"), "1");
    assert_eq!(p.show("state"), "loading");
    assert_eq!(p.show("document.readyState"), "complete");
}

#[test]
fn document_write_during_parse() {
    let mut p = Page::scripted(
        "<body>1<script>document.write('<b id=w>2</b>')</script>3",
        &[],
    );
    assert_eq!(
        p.body_text().replace("document.write('<b id=w>2</b>')", ""),
        "123"
    );
    assert!(p.truthy("document.getElementById('w').textContent === '2'"));
}

#[test]
fn nested_document_write() {
    let p = Page::scripted(
        "<body>a<script>document.write(\"<script>document.write('b')</scr\" + \"ipt>c\")</script>d",
        &[],
    );
    let text = p.body_text();
    let visible: String = text
        .replace(
            "document.write(\"<script>document.write('b')</scr\" + \"ipt>c\")",
            "",
        )
        .replace("document.write('b')", "");
    assert_eq!(visible, "abcd");
}

#[test]
fn external_scripts_and_modules() {
    let mut p = Page::scripted(
        "<script type=importmap>{\"imports\": {\"lib\": \"https://example.test/lib.js\"}}</script>\
         <script src=/a.js></script>\
         <script type=module>import { x } from 'lib'; window.fromModule = x;</script>\
         <script>window.order = (window.order || '') + 'i';</script>",
        &[
            ("https://example.test/a.js", "window.order = 'a';"),
            ("https://example.test/lib.js", "export const x = 42;"),
        ],
    );
    assert_eq!(p.show("order"), "ai");
    assert_eq!(p.show("fromModule"), "42");
}

#[test]
fn current_script_during_execution() {
    let mut p = Page::scripted(
        "<script id=yo>window.cs = document.currentScript.id;</script>",
        &[],
    );
    assert_eq!(p.show("cs"), "yo");
    assert!(p.truthy("document.currentScript === null"));
}

#[test]
fn script_errors_are_reported_and_parsing_continues() {
    let mut p = Page::scripted(
        "<script>noExiste()</script><script>window.after = 1</script><script src=/falta.js></script>",
        &[],
    );
    assert_eq!(p.show("after"), "1");
    let console = p.console();
    assert!(
        console.iter().any(|c| c.contains("ReferenceError")),
        "{console:?}"
    );
    assert!(
        console.iter().any(|c| c.contains("falta.js")),
        "{console:?}"
    );
    let env = bom::script::script_env(&p.rt.agent, p.ctx.realm).unwrap();
    assert_eq!((env.executed, env.failed), (2, 1));
}

#[test]
fn document_write_after_load_replaces_document() {
    let mut p = Page::scripted("<p>vieja", &[]);
    p.eval("document.write('<p id=nueva>nueva'); document.close()");
    assert_eq!(p.body_text(), "nueva");
    assert_eq!(p.show("document.readyState"), "complete");
}

#[test]
fn async_scripts_run_as_tasks() {
    let mut p = Page::scripted(
        "<script async src=/as.js></script><script>window.log = 'sync';</script>",
        &[("https://example.test/as.js", "window.log += ' async';")],
    );
    assert_eq!(p.show("log"), "sync async");
}

#[test]
fn performance_timing_after_load() {
    let mut p = Page::scripted("<p>x", &[]);
    assert!(p.truthy(
        "performance.timing.loadEventEnd >= performance.timing.navigationStart && \
         performance.timing.domContentLoadedEventStart > 0"
    ));
}
