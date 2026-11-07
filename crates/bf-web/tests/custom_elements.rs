//! Custom elements (HTML §4.13) desde JavaScript (E1): `customElements.define`/`get`/
//! `getName`/`whenDefined`/`upgrade`, los constructores HTML, el upgrade al insertar y al
//! definir, las reacciones, los customized built-ins con `is`, `ElementInternals`, `:defined`,
//! clonado, adopción y el parser.

use std::collections::HashMap;
use std::sync::Arc;

use bf_infra::{Resource, ResourceLoader};
use bf_js::host::DefaultHooks;
use bf_web::bom;
use bf_web::{DocumentOwner, Runtime, Url, WebContext, WebContextHandle};

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
}

impl Page {
    fn new(body: &str) -> Page {
        let html = format!("<!DOCTYPE html><html><head></head><body>{body}</body></html>");
        let mut rt = Runtime::new(Box::new(DefaultHooks));
        let (mut doc, _) = bf_html::parse_document(&html, &bf_html::ParseOptions::default());
        let realm = rt.main_realm();
        let url = Url::parse("https://example.com/page.html", None).expect("url");
        doc.url = url.clone();
        let ctx = bf_web::install(
            &mut rt.agent,
            realm,
            WebContext::new(DocumentOwner::Document(Box::new(doc)), url),
        );
        Page { rt, ctx }
    }

    /// Una página cargada por el parser con sus scripts.
    fn scripted(html: &str) -> Page {
        let mut p = Page::new("");
        let realm = p.ctx.realm;
        let url = Url::parse("https://example.com/", None).expect("url");
        bom::script::install_scripting(
            &mut p.rt.agent,
            realm,
            &url,
            Arc::new(MemLoader(HashMap::new())),
        );
        bom::script::begin_parse(&mut p.rt.agent, realm, &url);
        bom::script::feed(&mut p.rt.agent, realm, html);
        bom::script::finish(&mut p.rt.agent, realm);
        bom::script::resources_loaded(&mut p.rt.agent, realm);
        p
    }

    fn show(&mut self, src: &str) -> String {
        let realm = self.ctx.realm;
        let r = match self.rt.eval_script(realm, src, "test.js") {
            Ok(v) => self.rt.to_display(&v),
            Err(e) => format!("Error: {}", e.message),
        };
        self.rt.run_until_idle().expect("tareas");
        r
    }
}

#[test]
fn define_get_get_name_and_errors() {
    let mut p = Page::new("");
    assert_eq!(
        p.show(
            "class A extends HTMLElement {}; customElements.define('x-a', A); \
             [customElements.get('x-a') === A, customElements.getName(A), \
              customElements.get('x-b'), customElements.getName(class {}), \
              customElements instanceof CustomElementRegistry].join()"
        ),
        "true,x-a,,,true"
    );
    for (src, err) in [
        (
            "customElements.define('nodash', class extends HTMLElement {})",
            "SyntaxError",
        ),
        (
            "customElements.define('x-a', class extends HTMLElement {})",
            "NotSupportedError",
        ),
        ("customElements.define('x-c', A)", "NotSupportedError"),
        ("customElements.define('x-d', 1)", "TypeError"),
        (
            "customElements.define('x-e', class extends HTMLElement {}, {extends: 'x-f'})",
            "NotSupportedError",
        ),
    ] {
        assert_eq!(
            p.show(&format!("try {{ {src}; 'ok' }} catch (e) {{ e.name }}")),
            err,
            "{src}"
        );
    }
}

#[test]
fn html_constructor_and_create_element() {
    let mut p = Page::new("");
    assert_eq!(
        p.show(
            "class B extends HTMLElement { constructor() { super(); this.hecho = 1; } }; \
             customElements.define('x-b', B); \
             var a = new B(), b = document.createElement('x-b'); \
             [a instanceof B, a.localName, a.hecho, b instanceof B, b.hecho, \
              a.matches(':defined'), document.createElement('x-z').matches(':defined')].join()"
        ),
        "true,x-b,1,true,1,true,false"
    );
    assert_eq!(
        p.show("try { new HTMLElement() } catch (e) { e.name }"),
        "TypeError"
    );
    assert_eq!(
        p.show("try { new (class extends HTMLElement {})() } catch (e) { e.name }"),
        "TypeError"
    );
}

#[test]
fn upgrade_on_define_and_on_insert() {
    let mut p = Page::new("<x-up id=u a=1></x-up>");
    assert_eq!(
        p.show(
            "var log = []; var antes = u.matches(':defined'); \
             var fuera = document.createElement('x-up'); \
             class U extends HTMLElement { \
               static get observedAttributes() { return ['a']; } \
               constructor() { super(); log.push('ctor'); } \
               connectedCallback() { log.push('conn'); } \
               disconnectedCallback() { log.push('disc'); } \
               attributeChangedCallback(n, o, v) { log.push(n + ':' + o + ':' + v); } \
             }; \
             customElements.define('x-up', U); \
             [antes, u instanceof U, fuera instanceof U, log.join(' ')].join()"
        ),
        "false,true,false,ctor a:null:1 conn"
    );
    assert_eq!(
        p.show(
            "log = []; document.body.appendChild(fuera); u.setAttribute('a', '2'); \
             u.setAttribute('b', 'x'); u.remove(); \
             [fuera instanceof U, log.join(' ')].join()"
        ),
        "true,ctor conn a:1:2 disc"
    );
    assert_eq!(
        p.show(
            "var d = document.createElement('div'); d.innerHTML = '<x-up></x-up>'; \
             var antes = d.firstChild instanceof U; customElements.upgrade(d); \
             [antes, d.firstChild instanceof U].join()"
        ),
        "false,true"
    );
}

#[test]
fn when_defined() {
    let mut p = Page::new("");
    assert_eq!(
        p.show(
            "var r = []; customElements.whenDefined('x-w').then(c => r.push(c === W)); \
             var W = class extends HTMLElement {}; customElements.define('x-w', W); 0"
        ),
        "0"
    );
    assert_eq!(p.show("r.join()"), "true");
    assert_eq!(
        p.show("var e; customElements.whenDefined('nope').catch(x => e = x.name); 0"),
        "0"
    );
    assert_eq!(p.show("e"), "SyntaxError");
}

#[test]
fn customized_built_in() {
    let mut p = Page::new("");
    assert_eq!(
        p.show(
            "class P extends HTMLParagraphElement { constructor() { super(); this.ok = 1; } }; \
             customElements.define('x-p', P, {extends: 'p'}); \
             var a = document.createElement('p', {is: 'x-p'}), b = new P(); \
             [a instanceof P, a.ok, a.localName, b instanceof P, b.localName, \
              b.outerHTML].join()"
        ),
        "true,1,p,true,p,<p is=\"x-p\"></p>"
    );
}

#[test]
fn clone_and_adopt() {
    let mut p = Page::new("");
    assert_eq!(
        p.show(
            "var log = []; class C extends HTMLElement { \
               constructor() { super(); log.push('ctor'); } \
               adoptedCallback() { log.push('adopted'); } }; \
             customElements.define('x-c', C); \
             var c = document.createElement('x-c'); var k = c.cloneNode(); \
             var doc = document.implementation.createHTMLDocument(''); doc.adoptNode(c); \
             [k instanceof C, log.join(' ')].join()"
        ),
        "true,ctor ctor adopted"
    );
}

#[test]
fn element_internals_and_states() {
    let mut p = Page::new("");
    assert_eq!(
        p.show(
            "var i; class I extends HTMLElement { constructor() { super(); \
               i = this.attachInternals(); } }; \
             customElements.define('x-i', I); var e = new I(); \
             i.states.add('on'); \
             [i instanceof ElementInternals, i.states.has('on'), i.states.size, \
              (function () { try { e.attachInternals() } catch (x) { return x.name } })(), \
              (function () { try { document.createElement('div').attachInternals() } \
               catch (x) { return x.name } })()].join()"
        ),
        "true,true,1,NotSupportedError,NotSupportedError"
    );
}

#[test]
fn parser_creates_and_upgrades() {
    let mut p = Page::scripted(
        "<script>var log = []; customElements.define('x-q', class extends HTMLElement { \
           constructor() { super(); log.push('ctor'); } \
           connectedCallback() { log.push('conn'); } });</script>\
         <x-q></x-q><script>log.push('script');</script>",
    );
    assert_eq!(p.show("log.join(' ')"), "ctor conn script");
}
