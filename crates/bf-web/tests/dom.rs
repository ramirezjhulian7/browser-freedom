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
        "[object Element],[object Text],[object DocumentType]"
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
    let mut p = Page::new(SIMPLE).with_html_element();
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
        "<!DOCTYPE html><body><div id=a onclick=\"window.hit = this.id + ':' + event.type\">\
         </div></body>",
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
    assert!(p.error("document.createEvent('Nope')").contains("NotSupportedError"));
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
        self.0.borrow_mut().push(format!("prompt:{message}:{default}"));
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
        "alert:hola|confirm:¿?|prompt:n:d|open:x.html:_blank"
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
    assert!(p.error("document.querySelector('##')").contains("SyntaxError"));
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
    assert!(p.error("a.insertAdjacentText('nope', 'x')").contains("SyntaxError"));
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
    assert!(p.error("document.createElementNS(null, 'a:b')").contains("NamespaceError"));
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
    assert!(p.error("document.adoptNode(document)").contains("NotSupportedError"));
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
