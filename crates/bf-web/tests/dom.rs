//! El puente JS↔DOM (`bf-web`): `Window` como global, wrappers de nodos con identidad y
//! las interfaces del DOM sobre `bf-dom`.

use bf_js::host::DefaultHooks;
use bf_js::value::Value;
use bf_web::{DocumentOwner, Runtime, Url, WebContext, WebContextHandle};

struct Page {
    rt: Runtime,
    ctx: WebContextHandle,
}

impl Page {
    fn new(html: &str) -> Page {
        let mut rt = Runtime::new(Box::new(DefaultHooks));
        let (doc, _) = bf_html::parse_document(html, &bf_html::ParseOptions::default());
        let realm = rt.main_realm();
        let url = Url::parse("https://example.com/dir/page.html", None).expect("url");
        let ctx = bf_web::install(
            &mut rt.agent,
            realm,
            WebContext::new(DocumentOwner::Document(Box::new(doc)), url),
        );
        Page { rt, ctx }
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
}

const SIMPLE: &str = "<!DOCTYPE html><html><head><title>T</title></head><body>\
                      <div id=a class='x y'><p>uno</p><p>dos</p></div></body></html>";

// --- Window ---

#[test]
fn global_is_the_window() {
    let mut p = Page::new(SIMPLE);
    assert!(p.truthy(
        "window === globalThis && self === window && top === window && \
         Object.prototype.toString.call(window) === '[object Window]'"
    ));
}

#[test]
fn window_prototype_chain() {
    let mut p = Page::new(SIMPLE);
    assert!(p.truthy(
        "var wp = Object.getPrototypeOf(window); var props = Object.getPrototypeOf(wp); \
         wp === Window.prototype && \
         Object.prototype.toString.call(props) === '[object WindowProperties]' && \
         Object.getPrototypeOf(props) === EventTarget.prototype && \
         Object.getPrototypeOf(EventTarget.prototype) === Object.prototype"
    ));
}

#[test]
fn window_keeps_the_builtins() {
    let mut p = Page::new(SIMPLE);
    assert!(p.truthy(
        "typeof Array === 'function' && typeof setTimeout === 'function' && window.Math === Math"
    ));
}

#[test]
fn window_and_document_are_unforgeable() {
    let mut p = Page::new(SIMPLE);
    assert!(p.truthy(
        "!Object.getOwnPropertyDescriptor(window, 'document').configurable && \
         !Object.getOwnPropertyDescriptor(window, 'window').configurable"
    ));
}

// --- Nodos ---

#[test]
fn document_wrapper() {
    let mut p = Page::new(SIMPLE);
    assert!(p.truthy(
        "document instanceof Document && document instanceof Node && \
         document.nodeType === 9 && document.nodeName === '#document'"
    ));
}

#[test]
fn wrapper_identity() {
    let mut p = Page::new(SIMPLE);
    assert!(p.truthy(
        "document.body === document.body && document.getElementById('a') === \
         document.body.firstChild && document === document.documentElement.parentNode"
    ));
}

#[test]
fn wrapper_identity_survives_gc() {
    let mut p = Page::new(SIMPLE);
    p.eval("var a = document.getElementById('a'); a.expando = 7;");
    p.rt.gc();
    assert!(p.truthy("document.getElementById('a').expando === 7"));
}

#[test]
fn element_basics() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show("var a = document.getElementById('a'); [a.tagName, a.localName, a.id, a.className, a.namespaceURI].join()"),
        "DIV,div,a,x y,http://www.w3.org/1999/xhtml"
    );
}

#[test]
fn tree_navigation() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show("var a = document.getElementById('a'); a.firstChild.textContent + a.lastChild.textContent + a.firstChild.nextSibling.previousSibling.textContent"),
        "unodosuno"
    );
}

#[test]
fn create_and_append() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show("var s = document.createElement('SPAN'); s.textContent = 'tres'; document.getElementById('a').appendChild(s); [s.tagName, s.parentNode.id, document.getElementById('a').textContent].join()"),
        "SPAN,a,unodostres"
    );
    let dirty = p.ctx.get(&p.rt.agent).pending_dirty;
    assert!(dirty);
}

#[test]
fn hierarchy_errors_are_dom_exceptions() {
    let mut p = Page::new(SIMPLE);
    assert!(p.truthy(
        "try { document.body.appendChild(document); false } catch (e) { \
         e instanceof DOMException && e.name === 'HierarchyRequestError' && e.code === 3 && \
         e instanceof Error }"
    ));
}

#[test]
fn attributes() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show("var a = document.getElementById('a'); a.setAttribute('data-x', 1); [a.getAttribute('data-x'), a.hasAttribute('data-x'), a.getAttribute('nope')].join()"),
        "1,true,"
    );
    assert!(
        p.error("document.body.setAttribute('a b', 'x')")
            .contains("InvalidCharacterError")
    );
}

#[test]
fn text_and_comment_constructors() {
    let mut p = Page::new(SIMPLE);
    assert!(p.truthy(
        "var t = new Text('hola'); var c = new Comment('c'); t.data === 'hola' && \
         t instanceof CharacterData && c.nodeType === 8 && t.ownerDocument === document"
    ));
}

#[test]
fn illegal_invocation_and_constructor() {
    let mut p = Page::new(SIMPLE);
    assert!(
        p.error("Node.prototype.appendChild.call({}, document.body)")
            .contains("Illegal invocation")
    );
    assert!(p.error("new Node()").contains("Illegal constructor"));
}

#[test]
fn class_strings() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show("[document.body, document.body.firstChild.firstChild.firstChild, document.doctype || document.firstChild].map(x => Object.prototype.toString.call(x)).join()"),
        "[object Element],[object Text],[object DocumentType]"
    );
}
