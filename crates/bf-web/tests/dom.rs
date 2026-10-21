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
