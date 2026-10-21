//! Objetos host, API de incrustación y bucle de eventos (`bf_js::host`): clases WebIDL con
//! herencia y accessors, identidad de wrappers frente al GC, temporizadores, microtasks
//! frente a tareas, errores y conversiones.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use bf_js::heap::Gc;
use bf_js::host::webidl;
use bf_js::host::{
    ConsoleLevel, HostClass, HostClassId, HostHooks, HostKey, RejectionOperation, TaskSource,
};
use bf_js::object::Object;
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};
use bf_js::{JsError, Runtime};

// --- Hooks de prueba: reloj manual y registro de la consola y los errores ---

#[derive(Default)]
struct Shared {
    now: f64,
    log: Vec<String>,
    errors: Vec<String>,
    rejections: usize,
    handled: usize,
    tasks_queued: usize,
}

struct TestHooks(Rc<RefCell<Shared>>);

impl HostHooks for TestHooks {
    fn now(&mut self) -> f64 {
        self.0.borrow().now
    }
    fn console(&mut self, level: ConsoleLevel, message: &str) {
        self.0.borrow_mut().log.push(format!("{level:?}:{message}"));
    }
    fn report_exception(&mut self, message: &str, _stack: &str) {
        self.0.borrow_mut().errors.push(message.to_owned());
    }
    fn promise_rejection(&mut self, _p: Gc<Object>, op: RejectionOperation) {
        let mut s = self.0.borrow_mut();
        match op {
            RejectionOperation::Reject => s.rejections += 1,
            RejectionOperation::Handle => s.handled += 1,
        }
    }
    fn task_queued(&mut self, _source: TaskSource) {
        self.0.borrow_mut().tasks_queued += 1;
    }
}

struct Env {
    rt: Runtime,
    shared: Rc<RefCell<Shared>>,
}

impl Env {
    fn new() -> Env {
        let shared = Rc::new(RefCell::new(Shared::default()));
        let rt = Runtime::new(Box::new(TestHooks(shared.clone())));
        Env { rt, shared }
    }

    fn realm(&self) -> usize {
        self.rt.main_realm()
    }

    fn eval(&mut self, src: &str) -> Value {
        let r = self.realm();
        match self.rt.eval_script(r, src, "test.js") {
            Ok(v) => v,
            Err(e) => panic!("error: {e}\n{}", e.stack),
        }
    }

    fn eval_err(&mut self, src: &str) -> JsError {
        let r = self.realm();
        self.rt
            .eval_script(r, src, "test.js")
            .expect_err("se esperaba un error")
    }

    fn show(&mut self, src: &str) -> String {
        let v = self.eval(src);
        self.rt.to_display(&v)
    }

    fn truthy(&mut self, src: &str) -> bool {
        let v = self.eval(src);
        matches!(v, Value::Boolean(true))
    }

    /// Avanza el reloj y corre lo que quede listo.
    fn advance(&mut self, ms: f64) {
        self.shared.borrow_mut().now += ms;
        self.rt.run_until_idle().expect("bucle");
    }

    fn log(&self) -> Vec<String> {
        self.shared.borrow().log.clone()
    }
}

// --- Un DOM mínimo de prueba ---

#[derive(Debug)]
struct NodeData {
    id: u64,
    name: String,
    title: String,
}

thread_local! {
    static CLASSES: RefCell<HashMap<&'static str, HostClassId>> = RefCell::new(HashMap::new());
}

fn class(name: &str) -> HostClassId {
    CLASSES.with(|c| c.borrow()[name])
}

fn node<'a>(agent: &'a Agent, v: &Value) -> &'a NodeData {
    agent.host_unwrap::<NodeData>(v).expect("NodeData")
}

fn node_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&node(agent, &a.this).name))
}

fn node_get_id(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(node(agent, &a.this).id as f64))
}

fn element_title(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&node(agent, &a.this).title))
}

fn element_set_title(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = webidl::to_rust_string(agent, &a.arg(0))?;
    if let Some(n) = agent.host_unwrap_mut::<NodeData>(&a.this) {
        n.title = s;
    }
    Ok(Value::Undefined)
}

fn element_get_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&format!("<{}>", node(agent, &a.this).name)))
}

/// `sumLongs(sequence<long>)`: prueba de conversiones.
fn element_sum_longs(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let v = webidl::to_sequence(agent, &a.arg(0), webidl::to_long)?;
    Ok(Value::Number(v.iter().map(|x| f64::from(*x)).sum()))
}

/// `toLong(any)`.
fn static_to_long(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(f64::from(webidl::to_long(agent, &a.arg(0))?)))
}

/// `toUnsignedLong(any)`.
fn static_to_ulong(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(f64::from(webidl::to_unsigned_long(
        agent,
        &a.arg(0),
    )?)))
}

/// `Node.create(id, name)`: un wrapper con identidad.
fn static_create(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = webidl::to_unsigned_long(agent, &a.arg(0))? as u64;
    let name = webidl::to_rust_string(agent, &a.arg(1))?;
    let realm = agent.current_realm;
    let o = agent.host_wrap(
        realm,
        HostKey(id),
        class("HTMLDivElement"),
        Box::new(NodeData {
            id,
            name,
            title: String::new(),
        }),
    );
    Ok(Value::Object(o))
}

/// `[LegacyUnforgeable] readonly attribute DOMString kind`.
fn unforgeable_kind(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str("unforgeable"))
}

#[derive(Debug)]
struct PointData {
    x: f64,
    y: f64,
}

fn point_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let x = webidl::to_unrestricted_double(agent, &a.arg(0))?;
    let y = webidl::to_unrestricted_double(agent, &a.arg(1))?;
    let nt = a.new_target.expect("new.target");
    let o = agent.host_create_from_new_target(
        nt,
        class("Point"),
        None,
        Box::new(PointData { x, y }),
    )?;
    Ok(Value::Object(o))
}

fn point_x(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(
        agent.host_unwrap::<PointData>(&a.this).expect("Point").x,
    ))
}

fn point_y(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(
        agent.host_unwrap::<PointData>(&a.this).expect("Point").y,
    ))
}

fn point_has_instance(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    // Cualquier objeto con `x` e `y` numéricos "es" un Point (prueba del hook).
    let v = a.arg(0);
    let Value::Object(o) = v else {
        return Ok(Value::Boolean(false));
    };
    let x = agent.get(o, &"x".into(), v.clone())?;
    Ok(Value::Boolean(matches!(x, Value::Number(_))))
}

/// Registra Node → Element → HTMLElement → HTMLDivElement y Point.
fn dom_env() -> Env {
    let mut env = Env::new();
    let node = env.rt.register_class(
        HostClass::new("Node")
            .attribute("nodeName", Some(node_name), None)
            .method("getId", 0, node_get_id)
            .constant("ELEMENT_NODE", 1.0)
            .constant("TEXT_NODE", 3.0)
            .static_method("create", 2, static_create)
            .static_method("toLong", 1, static_to_long)
            .static_method("toUnsignedLong", 1, static_to_ulong),
    );
    let element = env.rt.register_class(
        HostClass::new("Element")
            .parent(node)
            .attribute("title", Some(element_title), Some(element_set_title))
            .method("getName", 0, element_get_name)
            .method("sumLongs", 1, element_sum_longs)
            .unforgeable("kind", Some(unforgeable_kind), None),
    );
    let html = env
        .rt
        .register_class(HostClass::new("HTMLElement").parent(element));
    let div = env
        .rt
        .register_class(HostClass::new("HTMLDivElement").parent(html));
    let point = env.rt.register_class(
        HostClass::new("Point")
            .constructor(point_ctor, 2)
            .attribute("x", Some(point_x), None)
            .attribute("y", Some(point_y), None)
            .has_instance(point_has_instance),
    );
    CLASSES.with(|c| {
        let mut c = c.borrow_mut();
        c.insert("Node", node);
        c.insert("Element", element);
        c.insert("HTMLElement", html);
        c.insert("HTMLDivElement", div);
        c.insert("Point", point);
    });
    env
}

// --- Clases host ---

#[test]
fn interface_objects_are_global_functions() {
    let mut e = dom_env();
    assert_eq!(
        e.show("typeof Node + ' ' + typeof HTMLDivElement"),
        "function function"
    );
}

#[test]
fn interface_without_constructor_is_illegal() {
    let mut e = dom_env();
    let err = e.eval_err("new Node()");
    assert!(
        err.message.contains("Illegal constructor"),
        "{}",
        err.message
    );
}

#[test]
fn interface_call_requires_new() {
    let mut e = dom_env();
    let err = e.eval_err("Point(1, 2)");
    assert!(err.message.starts_with("TypeError"), "{}", err.message);
}

#[test]
fn prototype_chain_follows_inheritance() {
    let mut e = dom_env();
    assert!(e.truthy(
        "Object.getPrototypeOf(HTMLDivElement.prototype) === HTMLElement.prototype && \
         Object.getPrototypeOf(HTMLElement.prototype) === Element.prototype && \
         Object.getPrototypeOf(Element.prototype) === Node.prototype && \
         Object.getPrototypeOf(Node.prototype) === Object.prototype"
    ));
}

#[test]
fn interface_objects_inherit_from_parent_interface() {
    let mut e = dom_env();
    assert!(e.truthy(
        "Object.getPrototypeOf(HTMLElement) === Element && \
         Object.getPrototypeOf(Node) === Function.prototype"
    ));
}

#[test]
fn to_string_tag_of_instances_and_prototypes() {
    let mut e = dom_env();
    assert_eq!(
        e.show("Object.prototype.toString.call(Node.create(1, 'div'))"),
        "[object HTMLDivElement]"
    );
    assert_eq!(
        e.show("Object.prototype.toString.call(Element.prototype)"),
        "[object Element]"
    );
}

#[test]
fn instanceof_host_classes() {
    let mut e = dom_env();
    assert!(e.truthy(
        "var d = Node.create(1, 'div'); d instanceof Node && d instanceof Element && \
         d instanceof HTMLElement && d instanceof HTMLDivElement"
    ));
    assert!(e.truthy("!(Node.create(2, 'p') instanceof Array)"));
}

#[test]
fn prototype_constructor_and_prototype_descriptors() {
    let mut e = dom_env();
    assert!(e.truthy(
        "var d = Object.getOwnPropertyDescriptor(Element, 'prototype'); \
         !d.writable && !d.enumerable && !d.configurable && \
         Element.prototype.constructor === Element"
    ));
}

#[test]
fn operations_are_enumerable_methods() {
    let mut e = dom_env();
    assert!(e.truthy(
        "var d = Object.getOwnPropertyDescriptor(Element.prototype, 'getName'); \
         d.writable && d.enumerable && d.configurable && typeof d.value === 'function'"
    ));
}

#[test]
fn operation_length_and_name() {
    let mut e = dom_env();
    assert_eq!(
        e.show("Element.prototype.sumLongs.length + ':' + Element.prototype.sumLongs.name"),
        "1:sumLongs"
    );
}

#[test]
fn attributes_are_accessors_on_the_prototype() {
    let mut e = dom_env();
    assert!(e.truthy(
        "var d = Object.getOwnPropertyDescriptor(Element.prototype, 'title'); \
         typeof d.get === 'function' && typeof d.set === 'function' && d.enumerable && \
         d.configurable && d.get.name === 'get title' && d.set.name === 'set title' && \
         d.set.length === 1"
    ));
}

#[test]
fn readonly_attribute_has_no_setter() {
    let mut e = dom_env();
    assert!(
        e.truthy("Object.getOwnPropertyDescriptor(Node.prototype, 'nodeName').set === undefined")
    );
    let err = e.eval_err("'use strict'; Node.create(3, 'a').nodeName = 'x';");
    assert!(err.message.starts_with("TypeError"), "{}", err.message);
    assert_eq!(
        e.show("var n = Node.create(4, 'span'); n.nodeName = 'x'; n.nodeName"),
        "span"
    );
}

#[test]
fn setter_updates_the_payload() {
    let mut e = dom_env();
    let v = e.eval("var t = Node.create(5, 'div'); t.title = 42; t");
    assert_eq!(e.rt.unwrap::<NodeData>(&v).expect("payload").title, "42");
    assert_eq!(e.show("t.title"), "42");
}

#[test]
fn brand_check_on_operations() {
    let mut e = dom_env();
    let err = e.eval_err("Element.prototype.getName.call({})");
    assert!(
        err.message.contains("Illegal invocation"),
        "{}",
        err.message
    );
}

#[test]
fn brand_check_on_getters_of_the_prototype() {
    let mut e = dom_env();
    let err = e.eval_err("Element.prototype.title");
    assert!(
        err.message.contains("Illegal invocation"),
        "{}",
        err.message
    );
}

#[test]
fn brand_check_rejects_unrelated_host_class() {
    let mut e = dom_env();
    let err = e.eval_err("Element.prototype.getName.call(new Point(1, 2))");
    assert!(
        err.message.contains("Illegal invocation"),
        "{}",
        err.message
    );
}

#[test]
fn inherited_members_work_on_subclasses() {
    let mut e = dom_env();
    assert_eq!(
        e.show("var d = Node.create(9, 'div'); d.getName() + d.getId() + d.nodeName"),
        "<div>9div"
    );
}

#[test]
fn constants_on_interface_and_prototype() {
    let mut e = dom_env();
    assert!(e.truthy(
        "Node.ELEMENT_NODE === 1 && Node.prototype.TEXT_NODE === 3 && \
         !Object.getOwnPropertyDescriptor(Node, 'ELEMENT_NODE').writable && \
         Node.create(1, 'x').ELEMENT_NODE === 1"
    ));
}

#[test]
fn static_methods_skip_the_brand_check() {
    let mut e = dom_env();
    assert_eq!(e.show("Node.toLong(-1.9)"), "-1");
}

#[test]
fn unforgeable_attributes_are_own_and_non_configurable() {
    let mut e = dom_env();
    assert!(e.truthy(
        "var d = Node.create(11, 'div'); \
         var p = Object.getOwnPropertyDescriptor(d, 'kind'); \
         p && !p.configurable && d.kind === 'unforgeable' && !('kind' in Element.prototype)"
    ));
    let err = e.eval_err("'use strict'; delete Node.create(11, 'div').kind");
    assert!(err.message.starts_with("TypeError"), "{}", err.message);
}

#[test]
fn constructible_host_class() {
    let mut e = dom_env();
    assert_eq!(
        e.show("var p = new Point(3, 4); p.x * p.y + ':' + Point.length"),
        "12:2"
    );
}

#[test]
fn host_classes_can_be_subclassed() {
    let mut e = dom_env();
    assert!(e.truthy(
        "class P3 extends Point { get z() { return 7; } } var q = new P3(1, 2); \
         q instanceof P3 && q.x === 1 && q.z === 7 && \
         Object.getPrototypeOf(q) === P3.prototype"
    ));
}

#[test]
fn custom_has_instance() {
    let mut e = dom_env();
    assert!(e.truthy("({ x: 1 }) instanceof Point && !({}) instanceof Point === false"));
}

// --- Identidad de wrappers ---

#[test]
fn wrap_returns_the_same_object_for_a_key() {
    let mut e = dom_env();
    assert!(e.truthy("Node.create(20, 'a') === Node.create(20, 'a')"));
}

#[test]
fn identity_survives_gc_while_referenced() {
    let mut e = dom_env();
    e.eval("var keep = Node.create(21, 'div'); keep.expando = 'sí';");
    e.rt.gc();
    assert!(e.truthy("Node.create(21, 'div') === keep && keep.expando === 'sí'"));
}

#[test]
fn unreachable_wrappers_are_collected() {
    let mut e = dom_env();
    e.eval("(function () { var w = Node.create(22, 'div'); w.expando = 1; })();");
    let realm = e.realm();
    assert!(e.rt.lookup(realm, HostKey(22)).is_some());
    e.rt.gc();
    assert!(e.rt.lookup(realm, HostKey(22)).is_none());
    assert!(e.truthy("Node.create(22, 'div').expando === undefined"));
}

#[test]
fn pinned_wrappers_survive_gc() {
    let mut e = dom_env();
    let v = e.eval("var w = Node.create(23, 'div'); w.mark = 5; w");
    let h = e.rt.pin(v);
    e.eval("w = null;");
    e.rt.gc();
    assert!(e.truthy("Node.create(23, 'div').mark === 5"));
    e.rt.unpin(h);
    e.eval("void 0;");
    e.rt.gc();
    let realm = e.realm();
    assert!(e.rt.lookup(realm, HostKey(23)).is_none());
}

#[test]
fn unwrap_checks_the_payload_type() {
    let mut e = dom_env();
    let v = e.eval("Node.create(24, 'div')");
    assert!(e.rt.unwrap::<NodeData>(&v).is_some());
    assert!(e.rt.unwrap::<PointData>(&v).is_none());
    let plain = e.eval("({})");
    assert!(e.rt.unwrap::<NodeData>(&plain).is_none());
}

#[test]
fn wrap_from_rust_with_lookup() {
    let mut e = dom_env();
    let realm = e.realm();
    let o = e.rt.wrap(
        realm,
        HostKey(25),
        class("HTMLDivElement"),
        Box::new(NodeData {
            id: 25,
            name: "section".into(),
            title: String::new(),
        }),
    );
    assert_eq!(e.rt.lookup(realm, HostKey(25)), Some(o));
    let g = e.rt.global(realm);
    e.rt.set(g, "fromRust", Value::Object(o)).expect("set");
    assert_eq!(e.show("fromRust.nodeName"), "section");
}

#[test]
fn each_realm_has_its_own_interfaces_and_wrappers() {
    let mut e = dom_env();
    let r2 = e.rt.create_realm();
    let other =
        e.rt.eval_script(r2, "Node.create(30, 'div')", "r2.js")
            .expect("r2");
    let g = e.rt.global(e.realm());
    e.rt.set(g, "other", other).expect("set");
    assert!(e.truthy(
        "var mine = Node.create(30, 'div'); mine !== other && !(other instanceof Node) && \
         Object.prototype.toString.call(other) === '[object HTMLDivElement]'"
    ));
}

// --- Bucle de eventos ---

#[test]
fn timers_run_in_timeout_order() {
    let mut e = dom_env();
    e.eval(
        "var order = []; setTimeout(() => order.push(20), 20); \
         setTimeout(() => order.push(10), 10); setTimeout(() => order.push(0), 0);",
    );
    e.advance(0.0);
    e.advance(10.0);
    e.advance(10.0);
    assert_eq!(e.show("order.join()"), "0,10,20");
}

#[test]
fn timers_with_equal_timeout_are_fifo() {
    let mut e = dom_env();
    e.eval("var o = []; for (let i = 0; i < 5; i++) setTimeout(() => o.push(i), 5);");
    e.advance(5.0);
    assert_eq!(e.show("o.join()"), "0,1,2,3,4");
}

#[test]
fn timers_do_not_fire_early() {
    let mut e = dom_env();
    e.eval("var fired = false; setTimeout(() => { fired = true; }, 50);");
    e.advance(49.0);
    assert!(e.truthy("!fired"));
    e.advance(1.0);
    assert!(e.truthy("fired"));
}

#[test]
fn clear_timeout_cancels() {
    let mut e = dom_env();
    e.eval("var hit = 0; var id = setTimeout(() => hit++, 1); clearTimeout(id);");
    e.advance(5.0);
    assert!(e.truthy("hit === 0 && typeof id === 'number' && id > 0"));
}

#[test]
fn set_interval_repeats_until_cleared() {
    let mut e = dom_env();
    e.eval("var n = 0; var iv = setInterval(() => { if (++n === 3) clearInterval(iv); }, 10);");
    for _ in 0..6 {
        e.advance(10.0);
    }
    assert_eq!(e.show("n"), "3");
}

#[test]
fn timer_arguments_and_this() {
    let mut e = dom_env();
    e.eval(
        "var got; setTimeout(function (a, b) { got = [a, b, this === globalThis]; }, 0, 1, 'x');",
    );
    e.advance(0.0);
    assert_eq!(e.show("got.join()"), "1,x,true");
}

#[test]
fn timer_with_string_handler() {
    let mut e = dom_env();
    e.eval("var s = 0; setTimeout('s = 41 + 1', 0);");
    e.advance(0.0);
    assert_eq!(e.show("s"), "42");
}

#[test]
fn nested_timers_are_clamped_to_4ms() {
    let mut e = dom_env();
    e.eval(
        "var depth = 0; function f() { depth++; if (depth < 10) setTimeout(f, 0); } \
         setTimeout(f, 0);",
    );
    // Sin avanzar el reloj solo corren los niveles sin la cota de 4 ms.
    e.advance(0.0);
    let d = e.eval("depth");
    let shallow: f64 = e.rt.convert(&d).expect("n");
    assert!(shallow < 10.0, "depth = {shallow}");
    for _ in 0..10 {
        e.advance(4.0);
    }
    assert_eq!(e.show("depth"), "10");
}

#[test]
fn microtasks_run_before_the_next_task() {
    let mut e = dom_env();
    e.eval(
        "var log = []; setTimeout(() => log.push('task'), 0); \
         Promise.resolve().then(() => log.push('micro')); log.push('sync');",
    );
    assert_eq!(e.show("log.join()"), "sync,micro");
    e.advance(0.0);
    assert_eq!(e.show("log.join()"), "sync,micro,task");
}

#[test]
fn microtasks_queued_by_a_task_run_before_the_following_task() {
    let mut e = dom_env();
    e.eval(
        "var log = []; \
         setTimeout(() => { log.push('t1'); queueMicrotask(() => log.push('m1')); }, 0); \
         setTimeout(() => log.push('t2'), 0);",
    );
    e.advance(0.0);
    assert_eq!(e.show("log.join()"), "t1,m1,t2");
}

#[test]
fn queue_microtask_order_with_promises() {
    let mut e = dom_env();
    e.eval(
        "var log = []; queueMicrotask(() => log.push(1)); \
         Promise.resolve().then(() => log.push(2)); queueMicrotask(() => log.push(3));",
    );
    assert_eq!(e.show("log.join()"), "1,2,3");
}

#[test]
fn queue_microtask_rejects_non_callables() {
    let mut e = dom_env();
    let err = e.eval_err("queueMicrotask(1)");
    assert!(err.message.starts_with("TypeError"), "{}", err.message);
}

#[test]
fn microtask_errors_are_reported_and_the_queue_continues() {
    let mut e = dom_env();
    e.eval("var after = false; queueMicrotask(() => { throw new Error('boom'); }); queueMicrotask(() => { after = true; });");
    assert!(e.truthy("after"));
    assert_eq!(e.shared.borrow().errors, vec!["Error: boom".to_owned()]);
}

#[test]
fn task_errors_are_reported_and_the_loop_continues() {
    let mut e = dom_env();
    e.eval(
        "var ok = false; setTimeout(() => { throw new TypeError('x'); }, 0); \
         setTimeout(() => { ok = true; }, 0);",
    );
    e.advance(0.0);
    assert!(e.truthy("ok"));
    assert_eq!(e.shared.borrow().errors, vec!["TypeError: x".to_owned()]);
}

#[test]
fn next_wake_reports_the_earliest_timer() {
    let mut e = dom_env();
    assert_eq!(e.rt.next_wake(), None);
    e.eval("setTimeout(() => {}, 30); setTimeout(() => {}, 12);");
    assert_eq!(e.rt.next_wake(), Some(12.0));
}

#[test]
fn animation_frames_run_with_the_timestamp_and_can_be_cancelled() {
    let mut e = dom_env();
    e.eval(
        "var ts = []; requestAnimationFrame(t => ts.push(t)); \
         var c = requestAnimationFrame(() => ts.push('no')); cancelAnimationFrame(c); \
         requestAnimationFrame(t => Promise.resolve().then(() => ts.push('micro')));",
    );
    assert!(e.rt.agent.has_animation_frame_callbacks());
    e.rt.run_animation_frames(16.5).expect("raf");
    assert_eq!(e.show("ts.join()"), "16.5,micro");
}

#[test]
fn native_tasks_from_rust() {
    let mut e = dom_env();
    let realm = e.realm();
    e.rt.agent.queue_task(
        TaskSource::Networking,
        Box::new(move |agent: &mut Agent| {
            let g = agent.realms[realm].global_object;
            agent
                .set_prop(g, "fromTask".into(), Value::str("hecho"))
                .expect("set");
        }),
    );
    assert_eq!(e.shared.borrow().tasks_queued, 1);
    e.rt.run_until_idle().expect("bucle");
    assert_eq!(e.show("fromTask"), "hecho");
}

#[test]
fn callback_tasks_in_fifo_order_across_sources() {
    let mut e = dom_env();
    let push = e.eval("var seen = []; (function (x) { seen.push(x); })");
    e.rt.agent.queue_callback_task(
        TaskSource::UserInteraction,
        push.clone(),
        Value::Undefined,
        vec![Value::str("a")],
    );
    e.rt.agent.queue_callback_task(
        TaskSource::DomManipulation,
        push.clone(),
        Value::Undefined,
        vec![Value::str("b")],
    );
    e.rt.agent.queue_callback_task(
        TaskSource::UserInteraction,
        push,
        Value::Undefined,
        vec![Value::str("c")],
    );
    assert_eq!(e.rt.run_until_idle().expect("bucle"), 3);
    assert_eq!(e.show("seen.join()"), "a,b,c");
}

// --- Errores y conversiones ---

#[test]
fn script_errors_carry_message_and_stack() {
    let mut e = dom_env();
    let err = e.eval_err("function f() { null.x; }\nf();");
    assert!(err.message.starts_with("TypeError"), "{}", err.message);
    assert!(err.stack.contains("test.js"), "{}", err.stack);
    assert!(err.value.is_some());
}

#[test]
fn syntax_errors_are_js_errors() {
    let mut e = dom_env();
    let err = e.eval_err("var = ;");
    assert!(err.message.starts_with("SyntaxError"), "{}", err.message);
}

#[test]
fn thrown_primitives_are_displayed() {
    let mut e = dom_env();
    let err = e.eval_err("throw 42");
    assert_eq!(err.message, "42");
}

#[test]
fn call_and_conversions() {
    let mut e = dom_env();
    let f = e.eval("(function (a, b) { return a + b; })");
    let a = e.rt.value(2.5);
    let b = e.rt.value(4i32);
    let r = e.rt.call(&f, Value::Undefined, &[a, b]).expect("call");
    let n: f64 = e.rt.convert(&r).expect("f64");
    assert_eq!(n, 6.5);
    let s: String = e.rt.convert(&Value::Boolean(true)).expect("string");
    assert_eq!(s, "true");
    let o: Option<i32> = e.rt.convert(&Value::Null).expect("option");
    assert_eq!(o, None);
    let arr = e.rt.value(vec![1i32, 2, 3]);
    let g = e.rt.global(e.realm());
    e.rt.set(g, "arr", arr).expect("set");
    assert_eq!(e.show("arr.length + arr[2]"), "6");
}

#[test]
fn display_formats_values_without_running_code() {
    let mut e = dom_env();
    assert_eq!(
        e.show("[1, 'a', [2, {b: null}], undefined]"),
        "[1, \"a\", [2, {b: null}], undefined]"
    );
    assert_eq!(
        e.show("({ get g() { throw 1; }, v: 1n })"),
        "{g: [Getter/Setter], v: 1n}"
    );
    assert_eq!(e.show("new RangeError('mal')"), "RangeError: mal");
    assert_eq!(e.show("Node.create(40, 'b')"), "[object HTMLDivElement]");
    assert_eq!(e.show("(function hola() {})"), "ƒ hola()");
}

#[test]
fn console_uses_the_hooks_and_the_formatter() {
    let mut e = dom_env();
    e.eval("console.log('%s tiene %d años', 'Ana', 41.9, {a: 1}); console.warn('w'); console.assert(false, 'x');");
    assert_eq!(
        e.log(),
        vec![
            "Log:Ana tiene 41 años {a: 1}".to_owned(),
            "Warn:w".to_owned(),
            "Error:Assertion failed: x".to_owned(),
        ]
    );
}

#[test]
fn rejection_tracker_reaches_the_hooks() {
    let mut e = dom_env();
    e.eval("var p = Promise.reject(1); p.catch(() => {});");
    let s = e.shared.borrow();
    assert_eq!((s.rejections, s.handled), (1, 1));
}

#[test]
fn webidl_long_and_unsigned_long() {
    let mut e = dom_env();
    assert_eq!(
        e.show("[Node.toLong(2 ** 31), Node.toLong(NaN), Node.toUnsignedLong(-1)].join()"),
        "-2147483648,0,4294967295"
    );
}

#[test]
fn webidl_sequence_conversion() {
    let mut e = dom_env();
    assert_eq!(
        e.show("Node.create(41, 'x').sumLongs(new Set([1, '2', 3.7]))"),
        "6"
    );
    let err = e.eval_err("Node.create(41, 'x').sumLongs(5)");
    assert!(err.message.starts_with("TypeError"), "{}", err.message);
}

#[test]
fn eval_script_in_another_realm() {
    let mut e = dom_env();
    let r2 = e.rt.create_realm();
    let v =
        e.rt.eval_script(r2, "typeof setTimeout + typeof HTMLElement", "r2.js")
            .expect("r2");
    assert_eq!(e.rt.to_display(&v), "functionfunction");
}

#[test]
fn gc_stress_with_tasks_wrappers_and_microtasks() {
    let mut e = dom_env();
    e.rt.agent.heap.set_threshold(50);
    e.eval(
        "var total = 0; var keep = []; \
         for (let i = 0; i < 200; i++) { \
           const n = Node.create(1000 + (i % 20), 'div'); n.count = (n.count || 0) + 1; \
           if (i % 7 === 0) keep.push(n); \
           setTimeout((x) => { total += x; Promise.resolve(x).then(v => { total += v; }); }, i % 5, i); \
           queueMicrotask(() => { total += 0; }); \
         }",
    );
    for _ in 0..6 {
        e.advance(1.0);
    }
    assert_eq!(e.show("total"), (2 * (0..200).sum::<i32>()).to_string());
    assert!(e.truthy("keep.every(n => n === Node.create(n.getId(), 'div'))"));
}
