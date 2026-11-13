//! El puente JS↔DOM (`bf-web`): `Window` como global, wrappers de nodos con identidad y
//! las interfaces del DOM sobre `bf-dom`.

use std::cell::RefCell;
use std::rc::Rc;

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

    /// Registra un `HTMLElement` mínimo con GlobalEventHandlers (la clase real es de E1).
    fn with_html_element(mut self) -> Page {
        let agent = &mut self.rt.agent;
        let element = bf_web::dom::classes(agent).element;
        let class = bf_web::dom::events::with_event_handlers(
            bf_web::HostClass::new("HTMLElement").parent(element),
            bf_web::dom::events::GLOBAL_EVENT_HANDLERS,
        );
        let id = agent.register_host_class(class);
        bf_web::dom::set_html_default_class(agent, id);
        self
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
        // E1: `body` tiene su interfaz HTML (HTMLBodyElement).
        "[object HTMLBodyElement],[object Text],[object DocumentType]"
    );
}

// --- Eventos ---

#[test]
fn event_constructor_and_attributes() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var e = new Event('foo', {bubbles: true, cancelable: true}); \
             [e.type, e.bubbles, e.cancelable, e.composed, e.eventPhase, e.isTrusted, \
              e.defaultPrevented, e.target, e.timeStamp >= 0].join()"
        ),
        "foo,true,true,false,0,false,false,,true"
    );
    assert!(p.truthy(
        "Event.NONE === 0 && Event.CAPTURING_PHASE === 1 && Event.AT_TARGET === 2 && \
         Event.BUBBLING_PHASE === 3"
    ));
    assert!(p.error("Event('x')").contains("new"));
}

#[test]
fn add_event_listener_and_dispatch() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var log = []; var a = document.getElementById('a'); var pp = a.firstChild; \
             a.addEventListener('x', function (e) { log.push('a:' + e.eventPhase + ':' + (this === a)); }); \
             a.addEventListener('x', function (e) { log.push('cap:' + e.eventPhase); }, true); \
             pp.addEventListener('x', function (e) { log.push('p:' + e.eventPhase + ':' + (e.target === pp)); }); \
             var r = pp.dispatchEvent(new Event('x', {bubbles: true})); \
             log.join() + '|' + r"
        ),
        "cap:1,p:2:true,a:3:true|true"
    );
}

#[test]
fn prevent_default_and_once() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var n = 0; var t = new EventTarget(); \
             t.addEventListener('y', function (e) { n++; e.preventDefault(); }, {once: true}); \
             var r1 = t.dispatchEvent(new Event('y', {cancelable: true})); \
             var r2 = t.dispatchEvent(new Event('y', {cancelable: true})); \
             [n, r1, r2].join()"
        ),
        "1,false,true"
    );
}

#[test]
fn handle_event_objects_and_remove() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var log = []; var o = { handleEvent: function (e) { log.push(this === o); } }; \
             document.addEventListener('z', o); document.dispatchEvent(new Event('z')); \
             document.removeEventListener('z', o); document.dispatchEvent(new Event('z')); \
             log.join()"
        ),
        "true"
    );
}

#[test]
fn stop_propagation() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var log = []; var a = document.getElementById('a'); \
             a.firstChild.addEventListener('s', function (e) { log.push(1); e.stopPropagation(); }); \
             a.firstChild.addEventListener('s', function (e) { log.push(2); }); \
             a.addEventListener('s', function (e) { log.push(3); }); \
             a.firstChild.dispatchEvent(new Event('s', {bubbles: true})); log.join()"
        ),
        "1,2"
    );
}

#[test]
fn dispatch_already_dispatched_throws() {
    let mut p = Page::new(SIMPLE);
    assert!(p.truthy(
        "var e = new Event('q'); var ok = false; \
         document.addEventListener('q', function () { \
           try { document.dispatchEvent(e); } catch (x) { ok = x.name === 'InvalidStateError'; } }); \
         document.dispatchEvent(e); ok"
    ));
}

#[test]
fn window_receives_bubbling_events() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var log = []; window.addEventListener('b', function (e) { \
               log.push(e.currentTarget === window, e.eventPhase); }); \
             document.body.dispatchEvent(new Event('b', {bubbles: true})); log.join()"
        ),
        "true,3"
    );
}

#[test]
fn custom_event_detail() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var d; document.addEventListener('c', function (e) { d = e.detail.v; }); \
             document.dispatchEvent(new CustomEvent('c', {detail: {v: 42}})); \
             d + ',' + (new CustomEvent('c') instanceof Event) + ',' + new CustomEvent('c').detail"
        ),
        "42,true,null"
    );
}

#[test]
fn mouse_and_keyboard_events() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var m = new MouseEvent('click', {clientX: 3, clientY: 4, button: 1, ctrlKey: true}); \
             var k = new KeyboardEvent('keydown', {key: 'a', code: 'KeyA', shiftKey: true}); \
             [m.clientX, m.clientY, m.button, m.ctrlKey, m.getModifierState('Control'), \
              m instanceof UIEvent, k.key, k.code, k.shiftKey, k instanceof UIEvent, \
              KeyboardEvent.DOM_KEY_LOCATION_LEFT].join()"
        ),
        "3,4,1,true,true,true,a,KeyA,true,true,1"
    );
}

#[test]
fn listener_exception_reported_to_onerror() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var msg = null, after = false; \
             window.onerror = function (m, src, l, c, err) { msg = err.message; }; \
             document.addEventListener('t', function () { throw new Error('boom'); }); \
             document.addEventListener('t', function () { after = true; }); \
             document.dispatchEvent(new Event('t')); msg + ',' + after"
        ),
        "boom,true"
    );
}

#[test]
fn error_event_listener_on_window() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var got; window.addEventListener('error', function (e) { \
               got = e instanceof ErrorEvent && e.error.message; }); \
             document.addEventListener('t', function () { throw new TypeError('bad'); }); \
             document.dispatchEvent(new Event('t')); got"
        ),
        "bad"
    );
}

#[test]
fn event_handler_idl_attribute() {
    // `x-a` usa la clase por defecto (el `HTMLElement` del test, con los handlers).
    let mut p = Page::new("<!DOCTYPE html><body><x-a id=a></x-a></body>").with_html_element();
    assert_eq!(
        p.show(
            "var n = 0; var a = document.getElementById('a'); \
             a.onclick = function (e) { n++; return false; }; \
             var r = a.dispatchEvent(new Event('click', {cancelable: true})); \
             var f = typeof a.onclick; a.onclick = null; a.dispatchEvent(new Event('click')); \
             [n, r, f, a.onclick].join()"
        ),
        "1,false,function,"
    );
}

#[test]
fn event_handler_content_attribute() {
    let mut p = Page::new(
        "<!DOCTYPE html><body><x-a id=a onclick=\"window.hit = this.id + ':' + event.type\">\
         </x-a></body>",
    )
    .with_html_element();
    assert_eq!(
        p.show(
            "var a = document.getElementById('a'); a.dispatchEvent(new Event('click')); \
             window.hit + ',' + typeof a.onclick"
        ),
        "a:click,function"
    );
    assert_eq!(
        p.show(
            "a.setAttribute('onclick', 'window.hit = 2'); a.dispatchEvent(new Event('click')); \
             window.hit"
        ),
        "2"
    );
}

#[test]
fn passive_listener_cannot_prevent_default() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var t = new EventTarget(); \
             t.addEventListener('p', function (e) { e.preventDefault(); }, {passive: true}); \
             var e = new Event('p', {cancelable: true}); [t.dispatchEvent(e), e.defaultPrevented].join()"
        ),
        "true,false"
    );
}

#[test]
fn init_event_and_create_event() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var e = document.createEvent('Event'); e.initEvent('w', true, true); \
             var c = document.createEvent('CustomEvent'); \
             [e.type, e.bubbles, e.cancelable, c instanceof CustomEvent, \
              document.createEvent('mouseevents') instanceof MouseEvent].join()"
        ),
        "w,true,true,true,true"
    );
    assert!(
        p.error("document.createEvent('Nope')")
            .contains("NotSupportedError")
    );
}

#[test]
fn composed_path() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var path; var a = document.getElementById('a'); \
             a.addEventListener('cp', function (e) { path = e.composedPath().map(function (n) { \
               return n === window ? 'W' : n.nodeName; }).join(' '); }); \
             a.firstChild.dispatchEvent(new Event('cp', {bubbles: true})); path"
        ),
        "P DIV BODY HTML #document W"
    );
}

// --- AbortSignal ---

#[test]
fn abort_controller() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var c = new AbortController(); var s = c.signal; var log = []; \
             s.addEventListener('abort', function (e) { log.push('abort:' + (e.target === s)); }); \
             s.onabort = function () { log.push('on'); }; \
             var before = s.aborted; c.abort('why'); c.abort('again'); \
             [before, s.aborted, s.reason, log.join('/'), c.signal === s].join()"
        ),
        "false,true,why,abort:true/on,true"
    );
    assert!(p.truthy(
        "var s2 = AbortSignal.abort(); s2.aborted && s2.reason instanceof DOMException && \
         s2.reason.name === 'AbortError'"
    ));
    assert!(p.truthy(
        "var ok = false; try { s2.throwIfAborted(); } catch (e) { ok = e === s2.reason; } ok"
    ));
}

#[test]
fn abort_signal_removes_listener() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var n = 0; var c = new AbortController(); var t = new EventTarget(); \
             t.addEventListener('e', function () { n++; }, {signal: c.signal}); \
             t.dispatchEvent(new Event('e')); c.abort(); t.dispatchEvent(new Event('e')); n"
        ),
        "1"
    );
}

#[test]
fn abort_signal_any() {
    let mut p = Page::new(SIMPLE);
    assert!(p.truthy(
        "var c = new AbortController(); var s = AbortSignal.any([c.signal]); \
         var b = s.aborted; c.abort('r'); !b && s.aborted && s.reason === 'r'"
    ));
}

// --- Colecciones ---

#[test]
fn child_nodes_is_live() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var a = document.getElementById('a'); var cn = a.childNodes; var l1 = cn.length; \
             a.appendChild(document.createTextNode('t')); \
             [l1, cn.length, cn === a.childNodes, cn[0].nodeName, cn.item(2).nodeName, cn[9], \
              cn instanceof NodeList, Object.keys(cn).join(':')].join()"
        ),
        "2,3,true,P,#text,,true,0:1:2"
    );
}

#[test]
fn node_list_iteration() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var out = []; \
             for (var n of document.getElementById('a').childNodes) out.push(n.textContent); \
             document.getElementById('a').childNodes.forEach(function (n, i) { out.push(i); }); \
             out.join()"
        ),
        "uno,dos,0,1"
    );
}

#[test]
fn html_collection_named_and_live() {
    let mut p = Page::new("<!DOCTYPE html><body><p id=x>1</p><p name=y>2</p></body>");
    assert_eq!(
        p.show(
            "var ps = document.getElementsByTagName('p'); var l = ps.length; \
             document.body.appendChild(document.createElement('p')); \
             [l, ps.length, ps.x.textContent, ps.namedItem('y').textContent, ps.item(0).id, \
              ps instanceof HTMLCollection, 'x' in ps].join()"
        ),
        "2,3,1,2,x,true,true"
    );
}

#[test]
fn class_list() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var a = document.getElementById('a'); var cl = a.classList; \
             cl.add('z'); cl.remove('x'); var t = cl.toggle('w'); var c = cl.contains('y'); \
             cl.replace('y', 'v'); \
             [a.className, cl.length, t, c, cl[0], cl.value, cl === a.classList].join('|')"
        ),
        "v z w|3|true|true|v|v z w|true"
    );
    assert!(
        p.error("document.getElementById('a').classList.add('a b')")
            .contains("InvalidCharacterError")
    );
    assert!(
        p.error("document.getElementById('a').classList.add('')")
            .contains("SyntaxError")
    );
}

#[test]
fn named_node_map() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var a = document.getElementById('a'); var m = a.attributes; \
             [m.length, m[0].name, m.getNamedItem('class').value, m.id.value, m.item(1).name, \
              m instanceof NamedNodeMap].join()"
        ),
        "2,id,x y,a,class,true"
    );
}

// --- MutationObserver ---

#[test]
fn mutation_observer_child_list() {
    let mut p = Page::new(SIMPLE);
    p.eval(
        "var recs = []; var a = document.getElementById('a'); \
         var mo = new MutationObserver(function (rs, o) { \
           recs.push(rs.length, rs[0].type, rs[0].addedNodes.length, o === mo); }); \
         mo.observe(a, {childList: true}); a.appendChild(document.createElement('span'));",
    );
    assert_eq!(p.show("recs.join()"), "1,childList,1,true");
}

#[test]
fn mutation_observer_attributes_and_take_records() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var a = document.getElementById('a'); var mo = new MutationObserver(function () {}); \
             mo.observe(a, {attributes: true, attributeOldValue: true}); a.setAttribute('id', 'b'); \
             var r = mo.takeRecords(); mo.disconnect(); a.setAttribute('id', 'c'); \
             [r.length, r[0].type, r[0].attributeName, r[0].oldValue, mo.takeRecords().length].join()"
        ),
        "1,attributes,id,a,0"
    );
}

#[test]
fn mutation_observer_microtask_order() {
    let mut p = Page::new(SIMPLE);
    p.eval(
        "var log = []; var a = document.getElementById('a'); \
         new MutationObserver(function () { log.push('mo'); }).observe(a, {childList: true}); \
         Promise.resolve().then(function () { log.push('p'); }); \
         a.textContent = ''; log.push('sync');",
    );
    assert_eq!(p.show("log.join()"), "sync,p,mo");
}

// --- Traversal ---

#[test]
fn tree_walker() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var w = document.createTreeWalker(document.body, NodeFilter.SHOW_ELEMENT); var out = []; \
             while (w.nextNode()) out.push(w.currentNode.nodeName); \
             w.currentNode = document.body; out.push(w.firstChild().nodeName, w.lastChild().nodeName, \
             w.previousSibling().nodeName, w.parentNode().nodeName); out.join()"
        ),
        "DIV,P,P,DIV,P,P,DIV"
    );
}

#[test]
fn node_iterator_with_filter() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var it = document.createNodeIterator(document.getElementById('a'), NodeFilter.SHOW_TEXT, \
               { acceptNode: function (n) { \
                   return n.data === 'dos' ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_SKIP; } }); \
             var n = it.nextNode(); [n.data, it.nextNode(), it.previousNode().data, it.whatToShow].join()"
        ),
        "dos,,dos,4"
    );
}

// --- DOMImplementation ---

#[test]
fn dom_implementation() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var impl = document.implementation; var d = impl.createHTMLDocument('hola'); \
             var dt = impl.createDocumentType('svg', 'p', 's'); \
             var x = impl.createDocument('http://www.w3.org/2000/svg', 'svg', null); \
             [impl === document.implementation, d.title, d.body.nodeName, d.doctype.name, \
              dt.name, dt.publicId, dt.systemId, x.documentElement.namespaceURI, \
              impl.hasFeature()].join()"
        ),
        "true,hola,BODY,html,svg,p,s,http://www.w3.org/2000/svg,true"
    );
}

// --- Hooks del navegador ---

/// Hooks que anotan cada llamada.
struct Recorder(Rc<RefCell<Vec<String>>>);

impl bf_web::WebHooks for Recorder {
    fn alert(&mut self, message: &str) {
        self.0.borrow_mut().push(format!("alert:{message}"));
    }
    fn confirm(&mut self, message: &str) -> bool {
        self.0.borrow_mut().push(format!("confirm:{message}"));
        true
    }
    fn prompt(&mut self, message: &str, default: &str) -> Option<String> {
        self.0
            .borrow_mut()
            .push(format!("prompt:{message}:{default}"));
        Some("respuesta".into())
    }
    fn open(&mut self, url: &str, target: &str, _features: &str) -> bool {
        self.0.borrow_mut().push(format!("open:{url}:{target}"));
        true
    }
    fn resource_needed(&mut self, url: &str, kind: bf_web::ResourceKind) {
        self.0.borrow_mut().push(format!("resource:{kind:?}:{url}"));
    }
    fn dirty(&mut self) {
        self.0.borrow_mut().push("dirty".into());
    }
}

fn recording(p: &mut Page) -> Rc<RefCell<Vec<String>>> {
    let log = Rc::new(RefCell::new(Vec::new()));
    p.ctx.get_mut(&mut p.rt.agent).hooks = Box::new(Recorder(log.clone()));
    log
}

#[test]
fn dialogs_go_to_hooks() {
    let mut p = Page::new(SIMPLE);
    let log = recording(&mut p);
    assert_eq!(
        p.show("alert('hola'); [confirm('¿?'), prompt('n', 'd'), window.open('x.html')].join()"),
        "true,respuesta,"
    );
    assert_eq!(
        log.borrow().join("|"),
        "alert:hola|confirm:¿?|prompt:n:d|open:https://example.com/dir/x.html:_blank"
    );
}

#[test]
fn mutations_mark_dirty() {
    let mut p = Page::new(SIMPLE);
    let log = recording(&mut p);
    p.eval("document.body.appendChild(document.createElement('i'))");
    p.eval("document.body.setAttribute('x', '1')");
    // `dirty` solo se avisa una vez hasta que el shell vuelva a maquetar.
    assert_eq!(log.borrow().join("|"), "dirty");
    let ctx = p.ctx.get(&p.rt.agent);
    assert!(ctx.pending_dirty);
    assert_eq!(ctx.mutations, 2);
}

#[test]
fn inserted_images_and_stylesheets_need_resources() {
    let mut p = Page::new(SIMPLE);
    let log = recording(&mut p);
    p.eval(
        "var i = document.createElement('img'); i.src = 'a.png'; i.setAttribute('src', 'a.png'); \
         document.body.appendChild(i); \
         var l = document.createElement('link'); l.setAttribute('rel', 'stylesheet'); \
         l.setAttribute('href', '/s.css'); document.head.appendChild(l); \
         var detached = document.createElement('img'); detached.setAttribute('src', 'no.png');",
    );
    let log = log.borrow();
    let resources: Vec<_> = log.iter().filter(|l| l.starts_with("resource")).collect();
    assert_eq!(
        resources,
        [
            "resource:Image:https://example.com/dir/a.png",
            "resource:Stylesheet:https://example.com/s.css"
        ]
    );
}

#[test]
fn inserted_inline_script_runs() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var s = document.createElement('script'); \
             s.textContent = 'window.ran = (window.ran || 0) + 1; window.cur = document.currentScript === s;'; \
             document.body.appendChild(s); document.body.appendChild(s); \
             [window.ran, window.cur, document.currentScript].join()"
        ),
        "1,true,"
    );
}

#[test]
fn script_needs_connection_and_text() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "window.n = 0; var s = document.createElement('script'); \
             var f = document.createDocumentFragment(); f.appendChild(s); \
             s.textContent = 'window.n++'; var before = window.n; \
             document.body.appendChild(f); [before, window.n].join()"
        ),
        "0,1"
    );
    // Un script vacío conectado se ejecuta cuando recibe texto.
    assert_eq!(
        p.show(
            "var e = document.createElement('script'); document.body.appendChild(e); \
             e.appendChild(document.createTextNode('window.n += 10')); window.n"
        ),
        "11"
    );
}

#[test]
fn inner_html_scripts_do_not_run() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "window.x = 0; document.body.innerHTML = '<script>window.x = 1<\\/script><b>hi</b>'; \
             [window.x, document.body.lastChild.nodeName].join()"
        ),
        "0,B"
    );
}

#[test]
fn external_script_is_a_resource() {
    let mut p = Page::new(SIMPLE);
    let log = recording(&mut p);
    p.eval("var s = document.createElement('script'); s.setAttribute('src', 'lib.js'); document.body.appendChild(s)");
    assert!(
        log.borrow()
            .iter()
            .any(|l| l == "resource:Script:https://example.com/dir/lib.js")
    );
}

#[test]
fn script_errors_are_reported() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var m; window.addEventListener('error', function (e) { m = e.message; }); \
             var s = document.createElement('script'); s.textContent = 'throw new RangeError(\"x\")'; \
             document.body.appendChild(s); m"
        ),
        "Uncaught RangeError: x"
    );
}

// --- Árbol: más del core ---

#[test]
fn parent_node_mixin() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var a = document.getElementById('a'); a.prepend('x', document.createElement('i')); \
             a.append('z'); \
             [a.childNodes.length, a.firstChild.data, a.children.length, a.firstElementChild.nodeName, \
              a.lastElementChild.nodeName, a.childElementCount, a.children === a.children, \
              a.lastChild.data].join()"
        ),
        "5,x,3,I,P,3,true,z"
    );
    assert_eq!(
        p.show("a.replaceChildren('solo'); a.childNodes.length + ':' + a.textContent"),
        "1:solo"
    );
}

#[test]
fn child_node_mixin() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var a = document.getElementById('a'); var p1 = a.firstChild; \
             p1.before('b'); p1.after(document.createElement('hr')); \
             a.lastChild.replaceWith('fin'); p1.remove(); \
             Array.prototype.map.call(a.childNodes, function (n) { return n.nodeName; }).join()"
        ),
        "#text,HR,#text"
    );
}

#[test]
fn element_siblings() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var ps = document.querySelectorAll('p'); \
             [ps.length, ps[0].nextElementSibling === ps[1], ps[1].previousElementSibling === ps[0], \
              ps[0].previousElementSibling, ps instanceof NodeList].join()"
        ),
        "2,true,true,,true"
    );
}

#[test]
fn selectors() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var a = document.querySelector('div.x'); \
             [a.id, a.matches('#a.y'), a.querySelector('p:last-child').textContent, \
              a.firstChild.closest('div').id, a.firstChild.closest('span'), \
              document.querySelectorAll('div > p').length, a.webkitMatchesSelector('div')].join()"
        ),
        "a,true,dos,a,,2,true"
    );
    assert!(
        p.error("document.querySelector('##')")
            .contains("SyntaxError")
    );
}

#[test]
fn get_elements_by() {
    let mut p = Page::new(
        "<!DOCTYPE html><body><p class='a b'>1</p><p class=b name=n>2</p><span class=a>3</span></body>",
    );
    assert_eq!(
        p.show(
            "[document.getElementsByClassName('a').length, document.getElementsByClassName('b a').length, \
              document.getElementsByTagName('*').length, document.body.getElementsByTagName('p').length, \
              document.getElementsByTagNameNS('http://www.w3.org/1999/xhtml', 'span').length, \
              document.getElementsByName('n')[0].textContent].join()"
        ),
        "2,1,6,2,1,2"
    );
}

#[test]
fn inner_and_outer_html() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var a = document.getElementById('a'); var before = a.innerHTML; \
             a.innerHTML = '<b>x</b>&amp;'; var after = a.innerHTML; \
             a.firstChild.outerHTML = '<i>y</i>'; \
             a.insertAdjacentHTML('beforeend', '<u>z</u>'); \
             [before, after, a.innerHTML, a.outerHTML.slice(0, 22)].join('|')"
        ),
        "<p>uno</p><p>dos</p>|<b>x</b>&amp;|<i>y</i>&amp;<u>z</u>|<div id=\"a\" class=\"x y"
    );
}

#[test]
fn insert_adjacent() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var a = document.getElementById('a'); \
             a.insertAdjacentElement('afterbegin', document.createElement('em')); \
             a.insertAdjacentText('beforeend', 't'); \
             [a.firstChild.nodeName, a.lastChild.data, \
              a.insertAdjacentElement('beforebegin', document.createElement('hr')).nodeName, \
              a.previousSibling.nodeName].join()"
        ),
        "EM,t,HR,HR"
    );
    assert!(
        p.error("a.insertAdjacentText('nope', 'x')")
            .contains("SyntaxError")
    );
}

#[test]
fn namespaced_attributes() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var e = document.createElementNS('http://www.w3.org/2000/svg', 'svg:rect'); \
             e.setAttributeNS('http://www.w3.org/1999/xlink', 'xlink:href', '#a'); \
             [e.prefix, e.localName, e.tagName, e.namespaceURI, \
              e.getAttributeNS('http://www.w3.org/1999/xlink', 'href'), \
              e.hasAttributeNS('http://www.w3.org/1999/xlink', 'href'), e.getAttributeNames().join(' ')].join()"
        ),
        "svg,rect,svg:rect,http://www.w3.org/2000/svg,#a,true,xlink:href"
    );
    assert!(
        p.error("document.createElementNS(null, 'a:b')")
            .contains("NamespaceError")
    );
}

#[test]
fn toggle_attribute_and_attr_nodes() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var e = document.createElement('div'); var t1 = e.toggleAttribute('hidden'); \
             var t2 = e.toggleAttribute('hidden', true); var t3 = e.toggleAttribute('hidden'); \
             var at = document.createAttribute('title'); at.value = 'v'; e.setAttributeNode(at); \
             [t1, t2, t3, e.getAttribute('title'), at.ownerElement === e, \
              e.getAttributeNode('title') === at, e.removeAttributeNode(at) === at, \
              e.hasAttribute('title')].join()"
        ),
        "true,true,false,v,true,true,true,false"
    );
}

#[test]
fn text_split_and_whole_text() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var t = document.createTextNode('abcdef'); document.body.appendChild(t); \
             var r = t.splitText(2); [t.data, r.data, t.nextSibling === r, t.wholeText].join()"
        ),
        "ab,cdef,true,abcdef"
    );
    assert!(p.error("t.splitText(99)").contains("IndexSizeError"));
}

#[test]
fn character_data_methods() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var c = document.createComment('hola'); c.appendData('!'); c.insertData(0, '¡'); \
             c.replaceData(1, 4, 'chao'); c.deleteData(5, 1); \
             [c.data, c.length, c.substringData(1, 2), c.nodeValue].join()"
        ),
        "¡chao,5,ch,¡chao"
    );
}

// --- Documentos ---

#[test]
fn document_properties() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "[document.URL, document.documentURI, document.compatMode, document.characterSet, \
              document.contentType, document.readyState, document.defaultView === window, \
              document.title, document.doctype.name, document.visibilityState, document.hidden, \
              document.hasFocus(), document.activeElement === document.body].join()"
        ),
        "https://example.com/dir/page.html,https://example.com/dir/page.html,CSS1Compat,UTF-8,\
         text/html,complete,true,T,html,visible,false,true,true"
    );
    assert_eq!(p.show("document.title = 'Nuevo'; document.title"), "Nuevo");
}

#[test]
fn adopt_node_between_documents() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var d = document.implementation.createHTMLDocument('x'); var a = document.getElementById('a'); \
             var n = 0; a.addEventListener('k', function () { n++; }); \
             var r = d.adoptNode(a); \
             [r === a, a.ownerDocument === d, a.parentNode, document.getElementById('a'), \
              a.firstChild.textContent, a.dispatchEvent(new Event('k')), n].join()"
        ),
        "true,true,,,uno,true,1"
    );
    assert!(
        p.error("document.adoptNode(document)")
            .contains("NotSupportedError")
    );
}

#[test]
fn append_from_other_document() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var d = document.implementation.createHTMLDocument('x'); var e = d.createElement('span'); \
             e.textContent = 'ajeno'; document.body.appendChild(e); \
             [e.ownerDocument === document, e.parentNode === document.body, \
              document.body.lastChild === e, e.textContent].join()"
        ),
        "true,true,true,ajeno"
    );
}

#[test]
fn import_node() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var d = document.implementation.createHTMLDocument('x'); var a = document.getElementById('a'); \
             var deep = d.importNode(a, true); var shallow = d.importNode(a); \
             [deep === a, deep.ownerDocument === d, deep.childNodes.length, shallow.childNodes.length, \
              a.ownerDocument === document, deep.id].join()"
        ),
        "false,true,2,0,true,a"
    );
}

#[test]
fn clone_document() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var c = document.cloneNode(true); var s = document.cloneNode(); \
             [c === document, c.nodeType, c.getElementById('a').id, s.childNodes.length, \
              c.getElementById('a').ownerDocument === c].join()"
        ),
        "false,9,a,0,true"
    );
}

#[test]
fn new_document_constructor() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var d = new Document(); [d.nodeType, d.contentType, d.URL, d.childNodes.length, \
              d.documentElement, d instanceof Document].join()"
        ),
        "9,application/xml,about:blank,0,,true"
    );
}

#[test]
fn document_write_while_parsing() {
    let html = "<!DOCTYPE html><body><p id=a>x</p>";
    let mut rt = Runtime::new(Box::new(DefaultHooks));
    let realm = rt.main_realm();
    let mut parser = bf_web::Parser::new(&bf_html::ParseOptions {
        scripting: true,
        iframe_srcdoc: false,
        ..bf_html::ParseOptions::default()
    });
    parser.feed(html, &mut bf_dom::events::NoScript);
    let url = Url::parse("https://example.com/", None).expect("url");
    let ctx = bf_web::install(
        &mut rt.agent,
        realm,
        WebContext::new(DocumentOwner::Parser(Box::new(parser)), url),
    );
    let mut p = Page { rt, ctx };
    assert_eq!(
        p.show(
            "document.write('<b id=w>escrito</b>'); document.writeln('<i>2</i>'); \
             [document.getElementById('w').textContent, document.getElementsByTagName('i').length, \
              document.body.lastChild.data === '\\n'].join()"
        ),
        "escrito,1,true"
    );
}

#[test]
fn document_write_in_xml_throws() {
    let mut p = Page::new(SIMPLE);
    assert!(
        p.error("new Document().write('x')")
            .contains("InvalidStateError")
    );
}

// --- Window ---

#[test]
fn window_members() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "window.name = 'w'; [name, window.closed, window.length, window.opener, \
              window.frameElement, top === window, parent === window, frames === window, \
              typeof window.print, window.event].join()"
        ),
        "w,false,0,,,true,true,true,function,"
    );
}

#[test]
fn window_event_during_dispatch() {
    let mut p = Page::new(SIMPLE);
    assert!(p.truthy(
        "var e = new Event('we'); var seen; \
         document.addEventListener('we', function () { seen = window.event; }); \
         document.dispatchEvent(e); seen === e && window.event === undefined"
    ));
}

#[test]
fn window_handlers_and_event_target() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var n = 0; window.onfoo = 1; window.onmessage = function () { n++; }; \
             window.dispatchEvent(new Event('message')); \
             [n, typeof window.onload, window instanceof EventTarget, \
              Object.getPrototypeOf(Window.prototype)[Symbol.toStringTag]].join()"
        ),
        "1,object,true,WindowProperties"
    );
}

#[test]
fn post_message_is_async() {
    let mut p = Page::new(SIMPLE);
    p.eval(
        "window.got = []; window.addEventListener('message', function (e) { got.push(e.data); }); \
         postMessage('m1', '*'); got.push('sync');",
    );
    p.rt.run_until_idle().expect("tareas");
    assert_eq!(p.show("got.join()"), "sync,m1");
}

#[test]
fn unhandled_rejection_event() {
    let mut p = Page::new(SIMPLE);
    p.eval(
        "window.log = []; window.addEventListener('unhandledrejection', function (e) { \
           log.push(e.reason, e.promise instanceof Promise, e instanceof PromiseRejectionEvent); \
           e.preventDefault(); }); \
         Promise.reject('r1'); Promise.reject('r2').catch(function () {});",
    );
    p.rt.run_until_idle().expect("tareas");
    assert_eq!(p.show("log.join()"), "r1,true,true");
}

#[test]
fn event_target_subclass() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "class T extends EventTarget { constructor() { super(); this.k = 1; } } \
             var t = new T(); var got; t.addEventListener('x', function (e) { got = e.target === t; }); \
             t.dispatchEvent(new Event('x')); [t instanceof EventTarget, t.k, got].join()"
        ),
        "true,1,true"
    );
}

#[test]
fn mutation_observer_subtree_and_character_data() {
    let mut p = Page::new(SIMPLE);
    p.eval(
        "window.types = []; var a = document.getElementById('a'); \
         new MutationObserver(function (rs) { rs.forEach(function (r) { \
           types.push(r.type + ':' + (r.oldValue === null ? '-' : r.oldValue)); }); }) \
           .observe(a, {subtree: true, characterData: true, characterDataOldValue: true, childList: true}); \
         a.firstChild.firstChild.data = 'UNO'; a.lastChild.remove();",
    );
    assert_eq!(p.show("types.join()"), "characterData:uno,childList:-");
}

// --- Parte 2: más eventos, documentos, Window [Global], Range y Selection ---

#[test]
fn dictionary_event_interfaces() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var h = new HashChangeEvent('hashchange', {oldURL: 'a', newURL: 'b'}); \
             var m = new MessageEvent('message', {data: {x: 1}, origin: 'o'}); \
             var s = new StorageEvent('storage', {key: 'k'}); \
             var w = new WheelEvent('wheel', {deltaY: 3, clientX: 2}); \
             var ps = new PopStateEvent('popstate', {state: 5}); \
             [h.oldURL, h.newURL, m.data.x, m.origin, m.lastEventId, m.ports.length, s.key, \
              s.oldValue, w.deltaY, w.clientX, w instanceof MouseEvent, WheelEvent.DOM_DELTA_LINE, \
              ps.state, new CompositionEvent('c', {data: 'd'}).data, \
              new AnimationEvent('a', {animationName: 'n'}).animationName, \
              new TransitionEvent('t', {propertyName: 'p'}).propertyName].join()"
        ),
        "a,b,1,o,,0,k,,3,2,true,1,5,d,n,p"
    );
    assert_eq!(
        p.show(
            "var e = document.createEvent('MessageEvent'); \
             e.initMessageEvent('message', true, false, 'datos', 'origen'); \
             [e instanceof MessageEvent, e.type, e.bubbles, e.data, e.origin].join()"
        ),
        "true,message,true,datos,origen"
    );
}

#[test]
fn document_interfaces_and_dom_parser() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var x = new DOMParser().parseFromString('<a xmlns=\"urn:x\"><b/></a>', 'application/xml'); \
             var h = new DOMParser().parseFromString('<p id=q>hola', 'text/html'); \
             [document instanceof HTMLDocument, x instanceof XMLDocument, x.documentElement.namespaceURI, \
              x.contentType, h instanceof HTMLDocument, h.getElementById('q').textContent, \
              new Document() instanceof XMLDocument, \
              document.implementation.createDocument(null, 'r') instanceof XMLDocument].join()"
        ),
        // `DOMParser` con un tipo XML crea un `Document`, no un `XMLDocument` (HTML §8.5.1;
        // WPT `domparsing/DOMParser-parseFromString-xml.html`).
        "true,false,urn:x,application/xml,true,hola,false,true"
    );
    assert!(
        p.error("new DOMParser().parseFromString('x', 'text/plain')")
            .contains("enum")
    );
}

/// E10: `Document.rootElement` (SVG 2): el `svg` raíz o `null`.
#[test]
fn document_root_element() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var s = new DOMParser().parseFromString('<svg xmlns=\"http://www.w3.org/2000/svg\"/>', 'image/svg+xml'); \
             var g = new DOMParser().parseFromString('<g xmlns=\"http://www.w3.org/2000/svg\"/>', 'image/svg+xml'); \
             [s.rootElement === s.documentElement, g.rootElement, document.rootElement, \
              'rootElement' in Document.prototype].join()"
        ),
        "true,,,true"
    );
}

#[test]
fn window_members_are_own_properties() {
    let mut p = Page::new(SIMPLE);
    assert!(p.truthy(
        "Object.prototype.hasOwnProperty.call(window, 'event') && \
         Object.prototype.hasOwnProperty.call(window, 'onload') && \
         Object.prototype.hasOwnProperty.call(window, 'postMessage') && \
         !Object.prototype.hasOwnProperty.call(Window.prototype, 'onload') && \
         typeof Window.prototype.constructor === 'function'"
    ));
}

#[test]
fn content_handlers_run_in_native_dispatch() {
    // Los handlers de atributo de contenido se registran en cualquier dispatch de bf-dom
    // (aquí, el `change` de la activación de un checkbox).
    let mut p = Page::new(
        "<!DOCTYPE html><body><input id=c type=checkbox onchange=\"window.changed = this.checked\">\
         </body>",
    );
    assert_eq!(
        p.show(
            "document.getElementById('c').dispatchEvent(new MouseEvent('click', {bubbles: true})); \
             window.changed"
        ),
        "true"
    );
}

#[test]
fn nested_events_get_their_own_wrapper() {
    let mut p = Page::new("<!DOCTYPE html><body><input id=r type=radio></body>");
    assert_eq!(
        p.show(
            "var r = document.getElementById('r'); var click = new MouseEvent('click'); var seen; \
             r.addEventListener('change', function (e) { seen = [e === click, e.type, click.eventPhase]; }); \
             r.dispatchEvent(click); seen.join()"
        ),
        "false,change,0"
    );
}

#[test]
fn range_basics() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var a = document.getElementById('a'); var r = document.createRange(); \
             r.setStart(a.firstChild.firstChild, 1); r.setEnd(a.lastChild.firstChild, 2); \
             [r.toString(), r.collapsed, r.commonAncestorContainer === a, \
              r.startContainer.data, r.startOffset, r.endOffset, \
              r.isPointInRange(a.lastChild, 0), r.comparePoint(a, 0), r.intersectsNode(a.firstChild), \
              r instanceof AbstractRange].join()"
        ),
        "nodo,false,true,uno,1,2,true,-1,true,true"
    );
    assert!(
        p.error("r.setStart(document.doctype, 0)")
            .contains("InvalidNodeTypeError")
    );
    assert!(p.error("r.setStart(a, 9)").contains("IndexSizeError"));
}

#[test]
fn range_is_live() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var a = document.getElementById('a'); var r = new Range(); r.setStart(a, 1); r.setEnd(a, 2); \
             a.insertBefore(document.createElement('i'), a.firstChild); var s1 = r.startOffset; \
             a.removeChild(a.lastChild); var e = r.endContainer === a && r.endOffset; \
             var t = document.createTextNode('abcdef'); document.body.appendChild(t); \
             var r2 = new Range(); r2.setStart(t, 4); r2.setEnd(t, 5); var n = t.splitText(2); \
             [s1, e, r2.startContainer === n, r2.startOffset, r2.endOffset].join()"
        ),
        "2,2,true,2,3"
    );
}

#[test]
fn range_mutations() {
    let mut p = Page::new("<!DOCTYPE html><body><p id=p>uno <b>dos</b> tres</p></body>");
    assert_eq!(
        p.show(
            "var p = document.getElementById('p'); var r = document.createRange(); \
             r.setStart(p.firstChild, 2); r.setEnd(p.lastChild, 3); \
             var c = r.cloneContents(); var cloned = c.textContent; \
             var f = r.extractContents(); \
             [cloned, f.childNodes.length, p.textContent, r.collapsed].join('|')"
        ),
        "o dos tr|3|unes|true"
    );
    assert_eq!(
        p.show(
            "var r = document.createRange(); r.selectNodeContents(p); \
             r.setStart(p.firstChild, 2); r.collapse(true); \
             r.insertNode(document.createElement('hr')); \
             var s = document.createElement('span'); r.selectNode(p.childNodes[1]); \
             r.surroundContents(s); \
             [p.innerHTML, r.startContainer === p].join('|')"
        ),
        "un<span><hr></span>es|true"
    );
    assert_eq!(
        p.show("var r = document.createRange(); r.selectNodeContents(p); r.deleteContents(); p.childNodes.length"),
        "0"
    );
}

#[test]
fn static_range() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var a = document.getElementById('a'); \
             var s = new StaticRange({startContainer: a, startOffset: 0, endContainer: a, endOffset: 1}); \
             [s.startContainer === a, s.endOffset, s.collapsed, s instanceof AbstractRange].join()"
        ),
        "true,1,false,true"
    );
    assert!(
        p.error(
            "new StaticRange({startContainer: document.doctype, startOffset: 0, \
             endContainer: document, endOffset: 0})"
        )
        .contains("InvalidNodeTypeError")
    );
}

#[test]
fn selection() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "var sel = getSelection(); var a = document.getElementById('a'); \
             var before = [sel.rangeCount, sel.type, sel.anchorNode]; \
             sel.setBaseAndExtent(a.lastChild.firstChild, 2, a.firstChild.firstChild, 1); \
             var mid = [sel.rangeCount, sel.type, sel.direction, sel.anchorNode.data, sel.focusOffset, \
                        sel.toString(), sel.containsNode(a.firstChild, true)]; \
             sel.collapseToStart(); \
             [before.join(), mid.join(), sel.isCollapsed, sel.type, document.getSelection() === sel].join('|')"
        ),
        "0,None,|1,Range,backward,dos,1,nodo,true|true|Caret|true"
    );
    assert_eq!(
        p.show(
            "var r = document.createRange(); r.selectNodeContents(a); sel.removeAllRanges(); \
             sel.addRange(r); [sel.getRangeAt(0) === r, sel.toString()].join()"
        ),
        "true,unodos"
    );
    assert!(
        p.error("sel.removeAllRanges(); sel.getRangeAt(0)")
            .contains("IndexSizeError")
    );
}

#[test]
fn dirty_without_mutated_call() {
    let mut p = Page::new(SIMPLE);
    assert!(!p.ctx.get_mut(&mut p.rt.agent).take_dirty());
    // Un cambio directo del documento (sin pasar por `mutated()`) también pide re-render.
    let ctx = p.ctx.get_mut(&mut p.rt.agent);
    let body = ctx.document().body().expect("body");
    ctx.document_mut()
        .set_attribute(body, &"x".into(), "1".into())
        .expect("atributo");
    assert!(ctx.take_dirty());
    assert!(!ctx.take_dirty());
}

#[test]
fn document_write_after_load_opens() {
    let mut p = Page::new(SIMPLE);
    assert_eq!(
        p.show(
            "document.write('<p id=n>nuevo</p>'); document.close(); \
             [document.getElementById('n') && document.getElementById('n').textContent, \
              document.getElementById('a')].join()"
        ),
        "nuevo,"
    );
}

// E1: rutas que mezclaban `NodeId` de dos documentos del mismo contexto (antes: pánico de índice
// en `bf-dom` `Document::get`, o el nodo equivocado). Los documentos creados son más chicos que
// el de la ventana, así que un `NodeId` de esta en su arena se sale de rango.
#[test]
fn nodes_of_two_documents_are_not_mixed() {
    let mut p = Page::new(SIMPLE);
    let r = p.show(
        "(function () { \
           var out = []; \
           var d = document.implementation.createHTMLDocument(''); \
           function err(f) { try { f(); return 'ok'; } catch (e) { return e.name; } } \
           /* table.caption con un elemento de otro documento: se adopta (DOM §4.2.3). */ \
           var t = d.createElement('table'); d.body.appendChild(t); \
           var c = document.createElement('caption'); \
           out.push(err(function () { t.caption = c; })); \
           out.push(t.caption === c, c.ownerDocument === d); \
           var big = document.createElement('caption'); \
           for (var i = 0; i < 50; i++) document.body.appendChild(document.createElement('i')); \
           var t2 = document.createElement('table'); document.body.appendChild(t2); \
           t2.tHead = d.createElement('thead'); out.push(t2.tHead.ownerDocument === document); \
           /* relatedTarget de otro documento: no se resuelve en la arena equivocada. */ \
           var m = new MouseEvent('mouseover', {relatedTarget: d.body}); \
           out.push(m.relatedTarget === d.body); \
           var seen = 'none'; \
           document.body.addEventListener('mouseover', function (e) { seen = String(e.relatedTarget); }); \
           out.push(err(function () { document.body.dispatchEvent(m); }), seen); \
           var m2 = new MouseEvent('mouseout', {relatedTarget: document.getElementById('a')}); \
           d.body.addEventListener('mouseout', function (e) { seen = String(e.relatedTarget); }); \
           out.push(err(function () { d.body.dispatchEvent(m2); }), seen); \
           /* Re-despachar en otro documento un evento en curso: InvalidStateError y el \
              target sigue siendo el del primero. */ \
           var inner = 'none', tgt = false; \
           document.body.addEventListener('ping', function (e) { \
             inner = err(function () { d.body.dispatchEvent(e); }); tgt = e.target === document.body; }); \
           document.body.dispatchEvent(new Event('ping')); \
           out.push(inner, tgt); \
           /* FormData con un submitter de otro documento: NotFoundError. */ \
           var f = document.createElement('form'); document.body.appendChild(f); \
           var b = d.createElement('button'); d.body.appendChild(b); \
           out.push(err(function () { new FormData(f, b); })); \
           return out.join(); \
         })()",
    );
    assert_eq!(
        r,
        "ok,true,true,true,true,ok,null,ok,null,InvalidStateError,true,NotFoundError"
    );
}

// E1 (ronda 2 en macOS): `before()`/`after()`/`replaceWith()` con el propio nodo entre los
// argumentos (§4.2.8, "viable sibling"): el nodo no se quita antes de calcular dónde insertar.
#[test]
fn child_node_mutators_with_the_node_itself() {
    let mut p = Page::new(SIMPLE);
    let r = p.show(
        "(function () { \
           function fresh() { \
             var parent = document.createElement('div'); \
             parent.append('text', document.createElement('test'), document.createElement('x')); \
             return [parent, parent.childNodes[1], parent.childNodes[2]]; \
           } \
           var out = [], t; \
           t = fresh(); t[1].before(t[1]); out.push(t[0].innerHTML); \
           t = fresh(); t[1].after(t[2], t[1]); out.push(t[0].innerHTML); \
           t = fresh(); t[1].replaceWith(t[2], t[1]); out.push(t[0].innerHTML); \
           var lone = document.createElement('div'), c = document.createElement('c'); \
           lone.appendChild(c); \
           var mo = new MutationObserver(function () {}); mo.observe(lone, {childList: true}); \
           lone.replaceChildren(c); out.push(mo.takeRecords().length, lone.firstChild === c); \
           return out.join('|'); \
         })()",
    );
    assert_eq!(
        r,
        "text<test></test><x></x>|text<x></x><test></test>|text<x></x><test></test>|1|true"
    );
}

// E1: `createDocument(ns, qn, doctype)` agrega el mismo doctype (adoptado), no una copia; los
// `Attr` con wrapper siguen a su elemento al adoptarlo; `isEqualNode` compara entre documentos;
// los argumentos obligatorios de `createDocument`, `insertBefore` y `CharacterData`.
#[test]
fn documents_doctypes_and_attrs_across_documents() {
    let mut p = Page::new(SIMPLE);
    let r = p.show(
        "(function () { \
           var out = []; \
           function err(f) { try { f(); return 'ok'; } catch (e) { return e.name; } } \
           var dt = document.implementation.createDocumentType('foo', '', ''); \
           var x = document.implementation.createDocument(null, 'foo', dt); \
           out.push(x.doctype === dt, dt.ownerDocument === x, x.firstChild === dt, \
                    x.documentElement.localName); \
           var d = document.implementation.createHTMLDocument(''); \
           var div = d.createElement('div'); div.setAttribute('data-a', '1'); \
           var attr = div.getAttributeNode('data-a'); \
           document.adoptNode(div); \
           out.push(attr.ownerDocument === document, div.getAttributeNode('data-a') === attr); \
           var a1 = document.implementation.createDocument('', ''); \
           var a2 = document.implementation.createDocument('', ''); \
           out.push(a1.isEqualNode(a2), document.createElement('p').isEqualNode(d.createElement('p'))); \
           out.push(err(function () { document.implementation.createDocument(); }), \
                    err(function () { document.body.insertBefore(document.createTextNode('t')); }), \
                    err(function () { document.createTextNode('t').appendData(); }), \
                    err(function () { document.createTextNode('t').substringData(); })); \
           var other = d.createElement('span'); \
           out.push(err(function () { document.createTextNode('t').insertBefore(document.createElement('i'), other); })); \
           return out.join(); \
         })()",
    );
    assert_eq!(
        r,
        "true,true,true,foo,true,true,true,true,TypeError,TypeError,TypeError,TypeError,\
         HierarchyRequestError"
    );
}

// E1: `moveBefore()` (DOM §4.2.6, "move" §4.2.3): mueve sin removing/insertion steps (un
// script movido no corre; el foco se queda), con dos mutation records, y lanza en los casos de
// la spec (otra raíz, ancestro, hijo ajeno, tipo).
#[test]
fn move_before_moves_without_reinserting() {
    let mut p = Page::new(SIMPLE);
    let r = p.show(
        "(function () { \
           var out = []; \
           function err(f) { try { f(); return 'ok'; } catch (e) { return e.name; } } \
           var a = document.getElementById('a'), p1 = a.children[0], p2 = a.children[1]; \
           var mo = new MutationObserver(function () {}); \
           mo.observe(a, {childList: true}); \
           a.moveBefore(p2, p1); \
           var recs = mo.takeRecords(); \
           out.push(a.firstElementChild === p2, recs.length, recs[0].removedNodes[0] === p2, \
                    recs[1].addedNodes[0] === p2); \
           a.moveBefore(p1, p1); out.push(a.lastElementChild === p1); \
           out.push(err(function () { a.moveBefore(document.createElement('i'), null); }), \
                    err(function () { p1.moveBefore(a, null); }), \
                    err(function () { a.moveBefore(p1, document.body); }), \
                    err(function () { document.moveBefore(document.createTextNode('t'), null); }), \
                    err(function () { a.moveBefore(p1); })); \
           window.ran = 0; \
           var s = document.createElement('script'); s.textContent = 'ran++'; \
           a.appendChild(s); document.body.moveBefore(s, null); \
           out.push(ran, document.body.lastChild === s); \
           var log = []; \
           customElements.define('x-m', class extends HTMLElement { \
             connectedCallback() { log.push('c'); } disconnectedCallback() { log.push('d'); } \
             connectedMoveCallback() { log.push('m'); } }); \
           customElements.define('x-n', class extends HTMLElement { \
             connectedCallback() { log.push('c'); } disconnectedCallback() { log.push('d'); } }); \
           var m = document.createElement('x-m'), n = document.createElement('x-n'); \
           a.append(m, n); log.push('|'); \
           document.body.moveBefore(m, null); document.body.moveBefore(n, null); \
           out.push(log.join('')); \
           return out.join(); \
         })()",
    );
    assert_eq!(
        r,
        "true,2,true,true,true,HierarchyRequestError,HierarchyRequestError,NotFoundError,\
         HierarchyRequestError,TypeError,1,true,cc|mdc"
    );
}

// E1: `HTMLElement.dataset` y `DOMStringMap` (HTML §3.2.6.6): los atributos `data-*` en
// camelCase, `[SameObject]`, setter, deleter, enumeración y `[LegacyOverrideBuiltIns]`.
#[test]
fn dataset_maps_data_attributes() {
    let mut p = Page::new(SIMPLE);
    let r = p.show(
        "(function () { \
           var out = []; \
           function err(f) { try { f(); return 'ok'; } catch (e) { return e.name; } } \
           var e = document.createElement('div'); \
           e.setAttribute('data-foo-bar', '1'); e.setAttribute('data-x', '2'); \
           /* en HTML el nombre pasa a minúsculas: `data-upper`. */ \
           e.setAttribute('data-Upper', 'no'); e.setAttribute('title', 't'); \
           var ds = e.dataset; \
           out.push(ds === e.dataset, ds instanceof DOMStringMap, ds.fooBar, ds.x, ds.upper); \
           out.push(Object.keys(ds).join('+'), 'fooBar' in ds, 'nope' in ds); \
           ds.newThing = 7; out.push(e.getAttribute('data-new-thing')); \
           delete ds.x; out.push(e.hasAttribute('data-x')); \
           out.push(err(function () { ds['a-b'] = 1; })); \
           e.setAttribute('data-to-string', 'ts'); out.push(ds.toString); \
           e.removeAttribute('data-to-string'); out.push(typeof ds.toString); \
           return out.join(); \
         })()",
    );
    assert_eq!(
        r,
        "true,true,1,2,no,fooBar+x+upper,true,false,7,false,SyntaxError,ts,function"
    );
}

// E1 (ronda 3): la API de validación de restricciones (HTML §4.10.20.3): `validity` es el
// mismo `ValidityState`, con los estados de `required`, `type=email`, `pattern`, `min`/`max`,
// `step` y el error propio; `labels`, `list`, `valueAsDate`, `stepUp`/`stepDown` y
// `setRangeText`.
#[test]
fn constraint_validation_and_input_members() {
    let mut p = Page::new(SIMPLE);
    let r = p.show(
        "(function () { \
           var out = []; \
           function err(f) { try { f(); return 'ok'; } catch (e) { return e.name; } } \
           var i = document.createElement('input'); i.required = true; \
           out.push(i.validity === i.validity, i.validity instanceof ValidityState, \
                    i.validity.valueMissing, i.validity.valid, i.checkValidity()); \
           i.value = 'x'; out.push(i.validity.valid); \
           i.type = 'email'; i.value = 'nope'; out.push(i.validity.typeMismatch); \
           i.value = 'a@b.c'; out.push(i.validity.valid); \
           i.type = 'text'; i.pattern = '[0-9]+'; i.value = 'abc'; \
           out.push(i.validity.patternMismatch); i.value = '123'; out.push(i.validity.valid); \
           var n = document.createElement('input'); n.type = 'number'; n.min = '2'; n.max = '10'; \
           n.step = '2'; n.value = '1'; out.push(n.validity.rangeUnderflow); \
           n.value = '5'; out.push(n.validity.stepMismatch); \
           n.value = '4'; n.stepUp(); out.push(n.value); n.stepDown(2); out.push(n.value); \
           i.setCustomValidity('mal'); out.push(i.validity.customError, i.validationMessage); \
           out.push(err(function () { i.setCustomValidity(); })); \
           var l = document.createElement('label'); l.htmlFor = 'q'; \
           var q = document.createElement('input'); q.id = 'q'; \
           document.body.append(l, q); out.push(q.labels.length, q.labels[0] === l); \
           var dl = document.createElement('datalist'); dl.id = 'dl'; document.body.append(dl); \
           q.setAttribute('list', 'dl'); out.push(q.list === dl); \
           var d = document.createElement('input'); d.type = 'date'; d.value = '2020-02-29'; \
           out.push(d.valueAsDate.getTime() === Date.UTC(2020, 1, 29)); \
           d.valueAsDate = new Date(Date.UTC(1999, 11, 31)); out.push(d.value); \
           var w = document.createElement('input'); w.type = 'week'; \
           w.valueAsDate = new Date('2019-12-09T00:00:00.000Z'); out.push(w.value); \
           var t = document.createElement('input'); t.value = 'abcdef'; \
           t.setSelectionRange(1, 3); t.setRangeText('XY', 1, 3, 'select'); \
           out.push(t.value, t.selectionStart, t.selectionEnd); \
           out.push(err(function () { t.setRangeText(); }), \
                    err(function () { t.setRangeText('x', 3, 1); })); \
           return out.join(); \
         })()",
    );
    assert_eq!(
        r,
        "true,true,true,false,false,true,true,true,true,true,true,true,6,2,true,mal,\
         TypeError,1,true,true,true,1999-12-31,2019-W50,aXYdef,1,3,TypeError,IndexSizeError"
    );
}

// E1: `HTMLMediaElement` sin reproducción, `TimeRanges`, y lo que faltaba de `Document`
// (`designMode`, `execCommand`, `queryCommand*`, `domain`) y de `HTMLElement` (popover,
// `autocapitalize`, `headingOffset`).
#[test]
fn media_and_legacy_members() {
    let mut p = Page::new(SIMPLE);
    let r = p.show(
        "(function () { \
           var out = []; \
           function err(f) { try { f(); return 'ok'; } catch (e) { return e.name; } } \
           var v = document.createElement('video'); \
           out.push(v.networkState, v.readyState, v.paused, isNaN(v.duration), \
                    v.buffered instanceof TimeRanges, v.buffered.length, v.canPlayType('video/mp4'), \
                    HTMLMediaElement.HAVE_ENOUGH_DATA, err(function () { v.buffered.start(0); })); \
           v.volume = 0.5; out.push(v.volume, err(function () { v.volume = 2; })); \
           v.appendChild(document.createElement('source')); out.push(v.networkState); \
           out.push(v.play() instanceof Promise); \
           out.push(document.designMode); document.designMode = 'on'; out.push(document.designMode); \
           out.push(document.execCommand('bold'), document.queryCommandSupported('bold'), \
                    document.queryCommandValue('bold'), document.domain, \
                    err(function () { document.domain = 'otro.test'; })); \
           var e = document.createElement('div'); \
           out.push(e.popover, err(function () { e.showPopover(); })); \
           e.popover = 'nada'; out.push(e.popover); document.body.append(e); \
           out.push(e.togglePopover(), e.togglePopover(), e.togglePopover({force: true})); \
           e.setAttribute('autocapitalize', 'on'); out.push(e.autocapitalize); \
           e.setAttribute('headingoffset', '12'); out.push(e.headingOffset); \
           return out.join(); \
         })()",
    );
    assert_eq!(
        r,
        "0,0,true,true,true,0,,4,IndexSizeError,0.5,IndexSizeError,3,true,off,on,false,false,,\
         example.com,SecurityError,,NotSupportedError,manual,true,false,true,sentences,8"
    );
}
