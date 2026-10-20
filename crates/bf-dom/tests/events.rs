//! Tests de eventos transcritos de WPT (`tests/wpt/dom/events/`).
//!
//! Los árboles se arman a mano (en `bf-dom` no hay parser). `window` es
//! `EventTarget::Window`, `new EventTarget()` es `doc.create_event_target()`,
//! `document.createEvent("Event")` + `initEvent` es `Event::uninitialized` + `init_event`,
//! `new MouseEvent("click")` es `Event::with_kind(.., EventKind::Mouse(..))` y `el.click()`
//! es el dispatch de un `MouseEvent` `click` que burbujea y es cancelable.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use bf_dom::events::{
    Activation, Event, EventInit, EventKind, EventPhase, EventTarget, Listener, ListenerOptions,
    MouseEventData, NoScript, ScriptHost,
};
use bf_dom::{Document, DomException, DomString, NodeId};

// --- Utilidades ---

fn s(x: &str) -> DomString {
    DomString::from(x)
}

fn native(f: impl Fn(&mut Document, &mut Event) + Send + Sync + 'static) -> Listener {
    Listener::Native(Arc::new(f))
}

fn cap(capture: bool) -> ListenerOptions {
    ListenerOptions {
        capture,
        ..ListenerOptions::default()
    }
}

fn node(n: NodeId) -> EventTarget {
    EventTarget::Node(n)
}

type Log<T> = Arc<Mutex<Vec<T>>>;

fn log<T>() -> Log<T> {
    Arc::new(Mutex::new(Vec::new()))
}

fn take<T>(l: &Log<T>) -> Vec<T> {
    std::mem::take(&mut *l.lock().unwrap())
}

/// Crea `<tag>` y lo agrega a `parent`.
fn el(doc: &mut Document, parent: NodeId, tag: &str) -> NodeId {
    let e = doc.create_element(&s(tag)).unwrap();
    doc.append_child(parent, e).unwrap();
    e
}

/// Crea `<tag>` sin padre.
fn detached(doc: &mut Document, tag: &str) -> NodeId {
    doc.create_element(&s(tag)).unwrap()
}

fn set(doc: &mut Document, e: NodeId, name: &str, value: &str) {
    doc.set_attribute(e, &s(name), s(value)).unwrap();
}

/// `document.createEvent("Event"); e.initEvent(type, bubbles, cancelable)`.
fn init_event(type_: &str, bubbles: bool, cancelable: bool) -> Event {
    let mut e = Event::uninitialized(EventKind::Event);
    e.init_event(type_, bubbles, cancelable);
    e
}

/// `new Event(type, {bubbles, cancelable})`.
fn new_event(type_: &str, bubbles: bool, cancelable: bool) -> Event {
    Event::new(
        type_,
        EventInit {
            bubbles,
            cancelable,
            composed: false,
        },
    )
}

/// `new MouseEvent(type, {bubbles, cancelable})`.
fn mouse(type_: &str, bubbles: bool, cancelable: bool) -> Event {
    Event::with_kind(
        type_,
        EventInit {
            bubbles,
            cancelable,
            composed: false,
        },
        EventKind::Mouse(MouseEventData::default()),
    )
}

/// `el.click()` (HTML): un `MouseEvent` `click` sintético que burbujea y es cancelable.
fn click(doc: &mut Document, e: NodeId) -> bool {
    let mut ev = Event::with_kind(
        "click",
        EventInit {
            bubbles: true,
            cancelable: true,
            composed: true,
        },
        EventKind::Mouse(MouseEventData::default()),
    );
    doc.dispatch_event(node(e), &mut ev)
}

/// El documento de la mayoría de los tests de dispatch:
///
/// ```html
/// <table id="table"><tbody id="table-body">
///   <tr id="table-row"><td id="table-cell">Shady Grove</td><td>Aeolian</td></tr>
///   <tr id="parent"><td id="target">Over the river, Charlie</td><td>Dorian</td></tr>
/// </tbody></table>
/// ```
struct Tabla {
    doc: Document,
    html: NodeId,
    body: NodeId,
    table: NodeId,
    tbody: NodeId,
    row: NodeId,
    parent: NodeId,
    target: NodeId,
}

impl Tabla {
    fn new() -> Tabla {
        let mut doc = Document::new();
        let root = doc.root();
        let html = el(&mut doc, root, "html");
        el(&mut doc, html, "head");
        let body = el(&mut doc, html, "body");
        let table = el(&mut doc, body, "table");
        let tbody = el(&mut doc, table, "tbody");
        let row = el(&mut doc, tbody, "tr");
        let cell = el(&mut doc, row, "td");
        let t = doc.create_text_node(s("Shady Grove"));
        doc.append_child(cell, t).unwrap();
        el(&mut doc, row, "td");
        let parent = el(&mut doc, tbody, "tr");
        let target = el(&mut doc, parent, "td");
        let t = doc.create_text_node(s("Over the river, Charlie"));
        doc.append_child(target, t).unwrap();
        el(&mut doc, parent, "td");
        Tabla {
            doc,
            html,
            body,
            table,
            tbody,
            row,
            parent,
            target,
        }
    }

    /// `document`.
    fn document(&self) -> EventTarget {
        node(self.doc.root())
    }

    /// `[window, document, html, body, table, tbody, parent, target]`.
    fn path(&self) -> Vec<EventTarget> {
        vec![
            EventTarget::Window,
            self.document(),
            node(self.html),
            node(self.body),
            node(self.table),
            node(self.tbody),
            node(self.parent),
            node(self.target),
        ]
    }
}

/// Registra el mismo listener (captura y burbujeo) en `targets` y devuelve el log de
/// `(currentTarget, eventPhase)`.
fn registrar_cadena(
    doc: &mut Document,
    targets: &[EventTarget],
    type_: &str,
) -> Log<(EventTarget, EventPhase)> {
    let l = log();
    let l2 = l.clone();
    let listener = native(move |_, ev| {
        l2.lock()
            .unwrap()
            .push((ev.current_target.unwrap(), ev.event_phase));
    });
    for &t in targets {
        doc.add_event_listener(t, type_, listener.clone(), cap(true));
        doc.add_event_listener(t, type_, listener.clone(), cap(false));
    }
    l
}

fn separar(v: Vec<(EventTarget, EventPhase)>) -> (Vec<EventTarget>, Vec<EventPhase>) {
    v.into_iter().unzip()
}

use EventPhase::{AtTarget, Bubbling, Capturing};

// --- Event-dispatch-bubbles-false.html ---

// WPT: Event-dispatch-bubbles-false.html — "In window.document with click event"
#[test]
fn bubbles_false_click_en_documento() {
    let mut t = Tabla::new();
    let path = t.path();
    let l = registrar_cadena(&mut t.doc, &path, "click");
    let mut evt = init_event("click", false, true);
    t.doc.dispatch_event(node(t.target), &mut evt);
    let (targets, phases) = separar(take(&l));
    let mut esperado = path.clone();
    esperado.push(node(t.target));
    assert_eq!(targets, esperado);
    let mut fases = vec![Capturing; 7];
    fases.extend([AtTarget, AtTarget]);
    assert_eq!(phases, fases);
}

// WPT: Event-dispatch-bubbles-false.html — "In window.document with load event"
// (para `load` el padre del documento no es `window`).
#[test]
fn bubbles_false_load_sin_window() {
    let mut t = Tabla::new();
    let path = t.path();
    let l = registrar_cadena(&mut t.doc, &path, "load");
    let mut evt = init_event("load", false, true);
    t.doc.dispatch_event(node(t.target), &mut evt);
    let (targets, phases) = separar(take(&l));
    let mut esperado = path[1..].to_vec();
    esperado.push(node(t.target));
    assert_eq!(targets, esperado);
    let mut fases = vec![Capturing; 6];
    fases.extend([AtTarget, AtTarget]);
    assert_eq!(phases, fases);
}

// --- Event-dispatch-bubbles-true.html ---

// WPT: Event-dispatch-bubbles-true.html — "In window.document with click event"
#[test]
fn bubbles_true_click_en_documento() {
    let mut t = Tabla::new();
    let path = t.path();
    let l = registrar_cadena(&mut t.doc, &path, "click");
    let mut evt = init_event("click", true, true);
    t.doc.dispatch_event(node(t.target), &mut evt);
    let (targets, phases) = separar(take(&l));
    let mut esperado = path.clone();
    esperado.extend(path.iter().rev());
    assert_eq!(targets, esperado);
    let mut fases = vec![Capturing; 7];
    fases.extend([AtTarget, AtTarget]);
    fases.extend([Bubbling; 7]);
    assert_eq!(phases, fases);
}

// WPT: Event-dispatch-bubbles-true.html — "In window.document with load event"
#[test]
fn bubbles_true_load_sin_window() {
    let mut t = Tabla::new();
    let path = t.path()[1..].to_vec();
    let l = registrar_cadena(&mut t.doc, &path, "load");
    let mut evt = init_event("load", true, true);
    t.doc.dispatch_event(node(t.target), &mut evt);
    let (targets, phases) = separar(take(&l));
    let mut esperado = path.clone();
    esperado.extend(path.iter().rev());
    assert_eq!(targets, esperado);
    let mut fases = vec![Capturing; 6];
    fases.extend([AtTarget, AtTarget]);
    fases.extend([Bubbling; 6]);
    assert_eq!(phases, fases);
}

// --- Event-dispatch-bubble-canceled.html / Event-dispatch-propagation-stopped.html ---

// WPT: Event-dispatch-bubble-canceled.html — "Setting cancelBubble=true prior to dispatchEvent()"
#[test]
fn cancel_bubble_antes_del_dispatch() {
    let mut t = Tabla::new();
    let path = t.path();
    let l = registrar_cadena(&mut t.doc, &path, "foo");
    let mut evt = init_event("foo", true, true);
    evt.set_cancel_bubble(true);
    t.doc.dispatch_event(node(t.target), &mut evt);
    assert!(take(&l).is_empty());
}

// WPT: Event-dispatch-propagation-stopped.html — "Calling stopPropagation() prior to dispatchEvent()"
#[test]
fn stop_propagation_antes_del_dispatch() {
    let mut t = Tabla::new();
    let path = t.path();
    let l = registrar_cadena(&mut t.doc, &path, "foo");
    let mut evt = init_event("foo", true, true);
    evt.stop_propagation();
    t.doc.dispatch_event(node(t.target), &mut evt);
    assert!(take(&l).is_empty());
}

// --- Event-dispatch-click.html ---

/// `<div id=dump>` en el body de un documento.
fn dump() -> (Document, NodeId) {
    let mut doc = Document::new();
    let root = doc.root();
    let html = el(&mut doc, root, "html");
    let body = el(&mut doc, html, "body");
    let dump = el(&mut doc, body, "div");
    (doc, dump)
}

/// Registra un listener de `type_` en `n` que anota la checkedness de `input`.
fn anotar_checked(doc: &mut Document, n: NodeId, input: NodeId, type_: &str) -> Log<bool> {
    let l = log();
    let l2 = l.clone();
    doc.add_event_listener(
        node(n),
        type_,
        native(move |doc, _| l2.lock().unwrap().push(doc.checkedness(input))),
        cap(false),
    );
    l
}

// WPT: Event-dispatch-click.html — "basic with click()" y "basic with dispatchEvent()"
#[test]
fn click_checkbox_basico() {
    for por_click in [true, false] {
        let (mut doc, dump) = dump();
        let input = el(&mut doc, dump, "input");
        set(&mut doc, input, "type", "checkbox");
        let l = anotar_checked(&mut doc, input, input, "click");
        if por_click {
            click(&mut doc, input);
        } else {
            doc.dispatch_event(node(input), &mut mouse("click", true, false));
        }
        assert_eq!(take(&l), vec![true]);
        assert!(doc.checkedness(input));
    }
}

// WPT: Event-dispatch-click.html — "basic with wrong event class"
#[test]
fn click_checkbox_con_event_no_mouse() {
    let (mut doc, dump) = dump();
    let input = el(&mut doc, dump, "input");
    set(&mut doc, input, "type", "checkbox");
    let l = anotar_checked(&mut doc, input, input, "click");
    doc.dispatch_event(node(input), &mut new_event("click", true, false));
    assert_eq!(take(&l), vec![false]);
    assert!(!doc.checkedness(input));
}

// WPT: Event-dispatch-click.html — "look at parents only when event bubbles" y
// "look at parents when event bubbles"
#[test]
fn click_busca_en_ancestros_solo_si_burbujea() {
    let (mut doc, dump) = dump();
    let input = el(&mut doc, dump, "input");
    set(&mut doc, input, "type", "checkbox");
    let child = doc.create_text_node(s("does not matter"));
    doc.append_child(input, child).unwrap();
    doc.dispatch_event(node(child), &mut mouse("click", false, false));
    assert!(!doc.checkedness(input));

    let l = anotar_checked(&mut doc, input, input, "click");
    doc.dispatch_event(node(child), &mut mouse("click", true, false));
    assert_eq!(take(&l), vec![true]);
    assert!(doc.checkedness(input));
}

// WPT: Event-dispatch-click.html — "pick the first with activation behavior <input type=checkbox>"
// y "pick the first with activation behavior <input type=radio>"
#[test]
fn click_elige_el_primero_con_activation_behavior() {
    for tipo in ["checkbox", "radio"] {
        let (mut doc, dump) = dump();
        let input = el(&mut doc, dump, "input");
        set(&mut doc, input, "type", tipo);
        let child = el(&mut doc, input, "input");
        set(&mut doc, child, "type", tipo);
        let li = anotar_checked(&mut doc, input, input, "click");
        let lc = anotar_checked(&mut doc, child, child, "click");
        doc.dispatch_event(node(child), &mut mouse("click", true, false));
        assert_eq!(take(&lc), vec![true], "{tipo}: el hijo se pre-activa");
        assert_eq!(take(&li), vec![false], "{tipo}: el padre no se pre-activa");
        assert!(!doc.checkedness(input));
    }
}

// WPT: Event-dispatch-click.html — "pick the first with activation behavior <a href>"
#[test]
fn click_elige_el_primer_a_href() {
    let (mut doc, dump) = dump();
    let link = el(&mut doc, dump, "a");
    set(&mut doc, link, "href", "javascript:hrefComplete('link')");
    let child = el(&mut doc, link, "a");
    set(&mut doc, child, "href", "javascript:hrefComplete('child')");
    doc.take_activations();
    doc.dispatch_event(node(child), &mut mouse("click", true, false));
    assert_eq!(
        doc.take_activations(),
        vec![Activation::FollowHyperlink(child)]
    );
}

// WPT: Event-dispatch-click.html — "event state during post-click handling"
// (el estado del evento `click` se observa al terminar el dispatch, cuando ya corrieron
// `input` y `change`).
#[test]
fn click_estado_del_evento_tras_el_dispatch() {
    let (mut doc, dump) = dump();
    let input = el(&mut doc, dump, "input");
    set(&mut doc, input, "type", "checkbox");
    let l = log();
    for tipo in ["click", "input", "change"] {
        let l2 = l.clone();
        doc.add_event_listener(
            node(input),
            tipo,
            native(move |_, ev| {
                l2.lock()
                    .unwrap()
                    .push((ev.type_.to_string(), ev.bubbles, ev.is_trusted))
            }),
            cap(false),
        );
    }
    let mut click_event = mouse("click", false, false);
    doc.dispatch_event(node(input), &mut click_event);
    assert_eq!(
        take(&l),
        vec![
            ("click".to_string(), false, false),
            ("input".to_string(), true, true),
            ("change".to_string(), true, true),
        ]
    );
    assert!(!click_event.default_prevented());
    assert!(click_event.return_value());
    assert_eq!(click_event.event_phase, EventPhase::None);
    assert_eq!(click_event.current_target, None);
    assert_eq!(click_event.target, Some(node(input)));
    assert!(click_event.composed_path().is_empty());
}

// WPT: Event-dispatch-click.html — "disconnected checkbox should be checked" / "disconnected
// radio should be checked" (y las variantes con `dispatchEvent(new MouseEvent('click'))`)
#[test]
fn click_checkbox_y_radio_desconectados() {
    for tipo in ["checkbox", "radio"] {
        for por_click in [true, false] {
            let mut doc = Document::new();
            let input = detached(&mut doc, "input");
            set(&mut doc, input, "type", tipo);
            let l = anotar_checked(&mut doc, input, input, "click");
            if por_click {
                click(&mut doc, input);
            } else {
                doc.dispatch_event(node(input), &mut mouse("click", false, false));
            }
            assert_eq!(take(&l), vec![true], "{tipo}");
        }
    }
}

// WPT: Event-dispatch-click.html — "disabled checkbox should be checked from
// dispatchEvent(new MouseEvent("click"))", la variante radio y "disabled ... should fire onclick"
#[test]
fn click_checkbox_y_radio_deshabilitados() {
    for tipo in ["checkbox", "radio"] {
        let mut doc = Document::new();
        let input = detached(&mut doc, "input");
        set(&mut doc, input, "type", tipo);
        set(&mut doc, input, "disabled", "");
        let l = anotar_checked(&mut doc, input, input, "click");
        doc.dispatch_event(node(input), &mut mouse("click", false, false));
        assert_eq!(take(&l), vec![true], "{tipo}: onclick se dispara");
        assert!(doc.checkedness(input), "{tipo}");
    }
}

// WPT: Event-dispatch-click.html — "disabled checkbox should get legacy-canceled-activation
// behavior" (y radio)
#[test]
fn click_legacy_canceled_activation_en_listener() {
    for tipo in ["checkbox", "radio"] {
        let mut doc = Document::new();
        let input = detached(&mut doc, "input");
        set(&mut doc, input, "type", tipo);
        set(&mut doc, input, "disabled", "");
        let l = log();
        let l2 = l.clone();
        doc.add_event_listener(
            node(input),
            "click",
            native(move |doc, ev| {
                l2.lock().unwrap().push(doc.checkedness(input));
                ev.prevent_default();
            }),
            cap(false),
        );
        let ok = doc.dispatch_event(node(input), &mut mouse("click", false, true));
        assert!(!ok);
        assert_eq!(
            take(&l),
            vec![true],
            "{tipo}: pre-activado durante el listener"
        );
        assert!(!doc.checkedness(input), "{tipo}: restaurado al cancelar");
    }
}

// WPT: Event-dispatch-click.html — "disabled checkbox should get legacy-canceled-activation
// behavior 2" (y radio): `preventDefault()` antes del dispatch.
#[test]
fn click_legacy_canceled_activation_antes_del_dispatch() {
    for tipo in ["checkbox", "radio"] {
        let mut doc = Document::new();
        let input = detached(&mut doc, "input");
        set(&mut doc, input, "type", tipo);
        set(&mut doc, input, "disabled", "");
        let mut ev = mouse("click", false, true);
        ev.prevent_default();
        doc.dispatch_event(node(input), &mut ev);
        assert!(!doc.checkedness(input), "{tipo}");
    }
}

// WPT: Event-dispatch-click.html — "disabling ${type} in onclick listener shouldn't suppress
// ${handler}" (oninput y onchange)
#[test]
fn click_deshabilitar_en_onclick_no_suprime_input_ni_change() {
    for tipo in ["checkbox", "radio"] {
        let (mut doc, dump) = dump();
        let input = el(&mut doc, dump, "input");
        set(&mut doc, input, "type", tipo);
        doc.add_event_listener(
            node(input),
            "click",
            native(move |doc, _| {
                doc.set_attribute(input, &s("disabled"), s("")).unwrap();
            }),
            cap(false),
        );
        let li = anotar_checked(&mut doc, input, input, "input");
        let lc = anotar_checked(&mut doc, input, input, "change");
        click(&mut doc, input);
        assert_eq!(take(&li), vec![true], "{tipo}: oninput");
        assert_eq!(take(&lc), vec![true], "{tipo}: onchange");
    }
}

// WPT: Event-dispatch-click.html — "disabled submit button should not activate"
#[test]
fn click_submit_deshabilitado_no_activa() {
    let (mut doc, dump) = dump();
    let form = el(&mut doc, dump, "form");
    let input = el(&mut doc, form, "input");
    set(&mut doc, input, "type", "submit");
    set(&mut doc, input, "disabled", "");
    doc.take_activations();
    doc.dispatch_event(node(input), &mut mouse("click", false, true));
    assert_eq!(doc.take_activations(), vec![]);
}

// WPT: Event-dispatch-click.html — "submit button should not activate if the event listener
// disables it" y "submit button that morphed from checkbox should not activate"
#[test]
fn click_submit_deshabilitado_en_listener_no_activa() {
    for morph in [false, true] {
        let (mut doc, dump) = dump();
        let form = el(&mut doc, dump, "form");
        let input = el(&mut doc, form, "input");
        if !morph {
            set(&mut doc, input, "type", "submit");
        }
        doc.add_event_listener(
            node(input),
            "click",
            native(move |doc, _| {
                if morph {
                    doc.set_attribute(input, &s("type"), s("submit")).unwrap();
                }
                doc.set_attribute(input, &s("disabled"), s("")).unwrap();
            }),
            cap(false),
        );
        doc.take_activations();
        click(&mut doc, input);
        assert_eq!(doc.take_activations(), vec![], "morph = {morph}");
    }
}

// --- Event-dispatch-detached-click.html ---

// WPT: Event-dispatch-detached-click.html — "Click event can be dispatched to an element that
// is not in the document."
#[test]
fn click_en_elemento_desconectado() {
    let mut doc = Document::new();
    let target = detached(&mut doc, "somerandomelement");
    let l = log();
    let l2 = l.clone();
    doc.add_event_listener(
        node(target),
        "click",
        native(move |_, ev| l2.lock().unwrap().push(ev.target)),
        cap(true),
    );
    let mut e = init_event("click", true, true);
    doc.dispatch_event(node(target), &mut e);
    assert_eq!(take(&l), vec![Some(node(target))]);
}

// --- Event-dispatch-handlers-changed.html ---

// WPT: Event-dispatch-handlers-changed.html — "Dispatch additional events inside an event listener"
// (agregar/quitar listeners durante el dispatch: la lista se copia en cada invoke).
#[test]
fn handlers_cambiados_durante_el_dispatch() {
    let mut t = Tabla::new();
    let targets = t.path();
    let l: Log<(EventTarget, usize)> = log();
    let handlers: Arc<OnceLock<Vec<Listener>>> = Arc::new(OnceLock::new());
    let foo: Arc<Mutex<HashMap<EventTarget, u32>>> = Arc::new(Mutex::new(HashMap::new()));
    let hacer = |i: usize| {
        let (l, handlers, foo) = (l.clone(), handlers.clone(), foo.clone());
        native(move |doc, ev| {
            let ct = ev.current_target.unwrap();
            l.lock().unwrap().push((ct, i));
            let hs = handlers.get().unwrap();
            let mut foo = foo.lock().unwrap();
            if ev.event_phase != Bubbling && foo.get(&ct) != Some(&1) {
                doc.remove_event_listener(ct, &s("bar"), &hs[0], true);
                doc.add_event_listener(ct, "bar", hs[2].clone(), cap(true));
                foo.insert(ct, 1);
            }
            if ev.event_phase != Capturing && foo.get(&ct) != Some(&3) {
                doc.remove_event_listener(ct, &s("bar"), &hs[0], false);
                doc.add_event_listener(ct, "bar", hs[3].clone(), cap(false));
                foo.insert(ct, 3);
            }
        })
    };
    let hs: Vec<Listener> = (0..4).map(hacer).collect();
    let _ = handlers.set(hs.clone());
    for &tg in &targets {
        t.doc
            .add_event_listener(tg, "bar", hs[0].clone(), cap(true));
        t.doc
            .add_event_listener(tg, "bar", hs[1].clone(), cap(false));
    }
    let mut evt = init_event("bar", true, true);
    t.doc.dispatch_event(node(t.target), &mut evt);

    let (actual_targets, actual_listeners): (Vec<_>, Vec<_>) = take(&l).into_iter().unzip();
    let mut esperado = targets.clone();
    esperado.push(node(t.target));
    esperado.extend(targets.iter().rev());
    assert_eq!(actual_targets, esperado);
    assert_eq!(
        actual_listeners,
        vec![0, 0, 0, 0, 0, 0, 0, 0, 1, 3, 1, 1, 1, 1, 1, 1, 1]
    );
}

// --- Event-dispatch-multiple-cancelBubble.html / Event-dispatch-multiple-stopPropagation.html ---

/// `<div id=parent><input id=target type=hidden></div>` en el body.
fn multiple_dispatch(con_cancel_bubble: bool) {
    let (mut doc, _) = dump();
    let body = doc.body().unwrap();
    let parent = el(&mut doc, body, "div");
    let target = el(&mut doc, parent, "input");
    set(&mut doc, target, "type", "hidden");
    let l = log();
    let l2 = l.clone();
    let test_event = native(move |_, ev| {
        let ct = ev.current_target.unwrap();
        l2.lock().unwrap().push(ct);
        if ct == node(parent) {
            if con_cancel_bubble {
                ev.set_cancel_bubble(true);
            } else {
                ev.stop_propagation();
            }
        }
    });
    let document = node(doc.root());
    for tg in [node(target), node(parent), document, EventTarget::Window] {
        doc.add_event_listener(tg, "foo", test_event.clone(), cap(false));
    }
    let mut evt = init_event("foo", true, true);
    doc.dispatch_event(node(target), &mut evt);
    assert_eq!(take(&l), vec![node(target), node(parent)]);
    doc.dispatch_event(node(parent), &mut evt);
    assert_eq!(take(&l), vec![node(parent)]);
    doc.dispatch_event(document, &mut evt);
    assert_eq!(take(&l), vec![document, EventTarget::Window]);
}

// WPT: Event-dispatch-multiple-cancelBubble.html — "Multiple dispatchEvent() and cancelBubble"
#[test]
fn multiples_dispatch_con_cancel_bubble() {
    multiple_dispatch(true);
}

// WPT: Event-dispatch-multiple-stopPropagation.html — "Multiple dispatchEvent() and stopPropagation()"
#[test]
fn multiples_dispatch_con_stop_propagation() {
    multiple_dispatch(false);
}

// --- Event-dispatch-omitted-capture.html ---

// WPT: Event-dispatch-omitted-capture.html — "EventTarget.addEventListener with the capture
// argument omitted"
#[test]
fn capture_omitido() {
    let mut t = Tabla::new();
    let mut targets = t.path();
    targets.reverse();
    let l = log();
    let l2 = l.clone();
    let test_event = native(move |_, ev| {
        l2.lock()
            .unwrap()
            .push((ev.current_target.unwrap(), ev.event_phase))
    });
    for &tg in &targets {
        t.doc
            .add_event_listener(tg, "foo", test_event.clone(), ListenerOptions::default());
    }
    let mut evt = init_event("foo", true, true);
    t.doc.dispatch_event(node(t.target), &mut evt);
    for &tg in &targets {
        t.doc
            .remove_event_listener(tg, &s("foo"), &test_event, false);
    }
    t.doc.dispatch_event(node(t.target), &mut evt);
    let (actual_targets, phases) = separar(take(&l));
    assert_eq!(actual_targets, targets);
    let mut fases = vec![AtTarget];
    fases.extend([Bubbling; 7]);
    assert_eq!(phases, fases);
}

// --- Event-dispatch-order.html / Event-dispatch-order-at-target.html ---

// WPT: Event-dispatch-order.html — "Event phases order"
#[test]
fn orden_de_fases() {
    let (mut doc, dump) = dump();
    let parent = el(&mut doc, dump, "div");
    let child = el(&mut doc, parent, "div");
    let l = log();
    for (tg, n, capture) in [(parent, 1, true), (child, 2, false), (parent, 3, false)] {
        let l2 = l.clone();
        doc.add_event_listener(
            node(tg),
            "click",
            native(move |_, _| l2.lock().unwrap().push(n)),
            cap(capture),
        );
    }
    doc.dispatch_event(node(child), &mut new_event("click", true, false));
    assert_eq!(take(&l), vec![1, 2, 3]);
}

// WPT: Event-dispatch-order-at-target.html — "Listeners are invoked in correct order (AT_TARGET phase)"
#[test]
fn orden_en_at_target() {
    let mut doc = Document::new();
    let e = detached(&mut doc, "div");
    let l = log();
    for (nombre, capture) in [("bubbling", false), ("capturing", true)] {
        let l2 = l.clone();
        doc.add_event_listener(
            node(e),
            "click",
            native(move |_, ev| l2.lock().unwrap().push((nombre, ev.event_phase))),
            cap(capture),
        );
    }
    for bubbles in [true, false] {
        doc.dispatch_event(node(e), &mut new_event("click", bubbles, false));
        assert_eq!(
            take(&l),
            vec![("capturing", AtTarget), ("bubbling", AtTarget)],
            "bubbles: {bubbles}"
        );
    }
}

// --- Event-dispatch-redispatch.html / EventTarget-dispatchEvent.html ---

// WPT: Event-dispatch-redispatch.html — "Redispatching event which is being dispatched" y
// EventTarget-dispatchEvent.html — "If the event's dispatch flag is set, an InvalidStateError
// must be thrown."
#[test]
fn redispatch_durante_el_dispatch_es_invalid_state() {
    let mut doc = Document::new();
    let target = detached(&mut doc, "div");
    let l = log();
    let l2 = l.clone();
    doc.add_event_listener(
        node(target),
        "type",
        native(move |doc, ev| {
            let root = doc.root();
            let a = doc.dispatch_event_checked(node(target), ev, &mut NoScript);
            let b = doc.dispatch_event_checked(node(root), ev, &mut NoScript);
            l2.lock().unwrap().extend([a, b]);
        }),
        cap(false),
    );
    let mut e = init_event("type", false, false);
    assert_eq!(
        doc.dispatch_event_checked(node(target), &mut e, &mut NoScript),
        Ok(true)
    );
    assert_eq!(
        take(&l),
        vec![
            Err(DomException::InvalidStateError),
            Err(DomException::InvalidStateError)
        ]
    );
}

// WPT: Event-dispatch-redispatch.html — "Redispatching DOMContentLoaded event after being
// dispatched" (un evento de confianza redespachado por script deja de serlo).
#[test]
fn redispatch_quita_is_trusted() {
    let mut doc = Document::new();
    let button = detached(&mut doc, "button");
    let l = log();
    let l2 = l.clone();
    doc.add_event_listener(
        node(button),
        "DOMContentLoaded",
        native(move |_, ev| l2.lock().unwrap().push(ev.is_trusted)),
        cap(false),
    );
    let mut ev = new_event("DOMContentLoaded", true, false);
    ev.is_trusted = true;
    doc.dispatch_event_checked(node(button), &mut ev, &mut NoScript)
        .unwrap();
    assert_eq!(take(&l), vec![false]);
    assert!(!ev.is_trusted);
}

// WPT: EventTarget-dispatchEvent.html — "If the event's initialized flag is not set, an
// InvalidStateError must be thrown (Event)."
#[test]
fn dispatch_de_evento_no_inicializado() {
    let mut doc = Document::new();
    let mut e = Event::uninitialized(EventKind::Event);
    assert_eq!(e.type_, "");
    let root = doc.root();
    assert_eq!(
        doc.dispatch_event_checked(node(root), &mut e, &mut NoScript),
        Err(DomException::InvalidStateError)
    );
}

/// Un host de script donde el listener `1` lanza una excepción.
struct HostQueLanza(Vec<&'static str>);

impl ScriptHost for HostQueLanza {
    fn call_listener(&mut self, _: &mut Document, h: u64, _: &mut Event) -> bool {
        if h == 1 {
            self.0.push("First");
            true
        } else {
            self.0.push("Second");
            false
        }
    }
}

// WPT: EventTarget-dispatchEvent.html — "Exceptions from event listeners must not be propagated."
#[test]
fn excepciones_de_listeners_no_se_propagan() {
    let mut doc = Document::new();
    let target = detached(&mut doc, "div");
    doc.add_event_listener(node(target), "type", Listener::Script(1), cap(false));
    doc.add_event_listener(node(target), "type", Listener::Script(2), cap(false));
    let mut e = init_event("type", false, false);
    let mut host = HostQueLanza(Vec::new());
    assert_eq!(
        doc.dispatch_event_checked(node(target), &mut e, &mut host),
        Ok(true)
    );
    assert_eq!(host.0, vec!["First", "Second"]);
}

// WPT: EventTarget-dispatchEvent.html — "Event listeners added during dispatch should be called"
#[test]
fn listeners_agregados_durante_el_dispatch() {
    let mut doc = Document::new();
    let outerb = detached(&mut doc, "b");
    let middleb = el(&mut doc, outerb, "b");
    let innerb = el(&mut doc, middleb, "b");
    let l = log();
    let l2 = l.clone();
    doc.add_event_listener(
        node(outerb),
        "x",
        native(move |doc, _| {
            let l3 = l2.clone();
            doc.add_event_listener(
                node(middleb),
                "x",
                native(move |_, _| l3.lock().unwrap().push("middle")),
                cap(true),
            );
            l2.lock().unwrap().push("outer");
        }),
        cap(true),
    );
    doc.dispatch_event(node(innerb), &mut new_event("x", false, false));
    assert_eq!(take(&l), vec!["outer", "middle"]);
}

// WPT: EventTarget-dispatchEvent.html — "Capturing event listeners should be called before
// non-capturing ones"
#[test]
fn captura_antes_que_burbujeo_en_el_target() {
    let mut doc = Document::new();
    let b = detached(&mut doc, "b");
    let l = log();
    for (n, capture) in [(1, true), (2, false), (3, true)] {
        let l2 = l.clone();
        doc.add_event_listener(
            node(b),
            "x",
            native(move |_, _| l2.lock().unwrap().push(n)),
            cap(capture),
        );
    }
    doc.dispatch_event(node(b), &mut new_event("x", false, false));
    assert_eq!(take(&l), vec![1, 3, 2]);
}

// --- Event-dispatch-reenter.html ---

// WPT: Event-dispatch-reenter.html — "Dispatch additional events inside an event listener"
#[test]
fn dispatch_reentrante() {
    let mut t = Tabla::new();
    let targets = t.path();
    let (table, target) = (t.table, t.target);
    let l = log();
    let l2 = l.clone();
    let test_event = native(move |doc, evt| {
        let ct = evt.current_target.unwrap();
        l2.lock().unwrap().push((ct, evt.type_.to_string()));
        if ct == node(table) && evt.type_ == "foo" {
            let mut e = init_event("bar", true, true);
            doc.dispatch_event(node(target), &mut e);
        }
    });
    for &tg in &targets {
        t.doc
            .add_event_listener(tg, "foo", test_event.clone(), cap(true));
        t.doc
            .add_event_listener(tg, "bar", test_event.clone(), cap(false));
    }
    let mut evt = init_event("foo", false, true);
    t.doc.dispatch_event(node(t.target), &mut evt);

    let (actual_targets, types): (Vec<_>, Vec<_>) = take(&l).into_iter().unzip();
    let [window, document, html, body, table, tbody, parent, target] = targets[..] else {
        unreachable!()
    };
    assert_eq!(
        actual_targets,
        vec![
            window, document, html, body, table, target, parent, tbody, table, body, html,
            document, window, tbody, parent, target
        ]
    );
    let mut tipos = vec!["foo"; 5];
    tipos.extend(["bar"; 8]);
    tipos.extend(["foo"; 3]);
    assert_eq!(types, tipos);
}

// --- Event-dispatch-single-activation-behavior.html ---

// WPT: Event-dispatch-single-activation-behavior.html — "When clicking child X of parent Y, only
// child should be activated." (subconjunto: `a`, `input` submit/reset/checkbox y `button`; sin
// `details`, `label` ni el envío real del formulario: se miran las activaciones pendientes).
#[test]
fn una_sola_activation_behavior() {
    // (target, padre) → activación esperada del target.
    type Armar = fn(&mut Document, NodeId) -> (NodeId, Option<Activation>);
    let crear_a: Armar = |doc, p| {
        let a = el(doc, p, "a");
        set(doc, a, "href", "#link");
        (a, Some(Activation::FollowHyperlink(a)))
    };
    let crear_submit: Armar = |doc, p| {
        let i = el(doc, p, "input");
        set(doc, i, "type", "submit");
        (i, Some(Activation::Submit(i)))
    };
    let crear_reset: Armar = |doc, p| {
        let b = el(doc, p, "button");
        set(doc, b, "type", "reset");
        (b, Some(Activation::Reset(b)))
    };
    let crear_checkbox: Armar = |doc, p| {
        let i = el(doc, p, "input");
        set(doc, i, "type", "checkbox");
        (i, None)
    };
    let crear_area: Armar = |doc, p| {
        let a = el(doc, p, "area");
        set(doc, a, "href", "#link");
        (a, Some(Activation::FollowHyperlink(a)))
    };
    let todos = [
        crear_a,
        crear_submit,
        crear_reset,
        crear_checkbox,
        crear_area,
    ];
    for (i, crear_target) in todos.iter().enumerate() {
        for (j, crear_padre) in todos.iter().enumerate() {
            if i == j {
                continue;
            }
            let (mut doc, dump) = dump();
            let form = el(&mut doc, dump, "form");
            let (padre, _) = crear_padre(&mut doc, form);
            let (target, esperado) = crear_target(&mut doc, padre);
            doc.take_activations();
            click(&mut doc, target);
            assert_eq!(
                doc.take_activations(),
                esperado.into_iter().collect::<Vec<_>>(),
                "target {i}, padre {j}"
            );
            if j == 3 {
                assert!(!doc.checkedness(padre), "el checkbox padre no se activa");
            }
            if i == 3 {
                assert!(doc.checkedness(target), "el checkbox target se activa");
            }
        }
    }
}

// --- Event-dispatch-target-moved.html / Event-dispatch-target-removed.html ---

fn path_fijo(mover: bool) {
    let mut t = Tabla::new();
    let targets = t.path();
    let (parent, target, row) = (t.parent, t.target, t.row);
    let l = log();
    let l2 = l.clone();
    let test_event = native(move |doc, evt| {
        if doc.parent(target) == Some(parent) {
            let n = doc.remove_child(parent, target).unwrap();
            if mover {
                doc.append_child(row, n).unwrap();
            }
        }
        l2.lock()
            .unwrap()
            .push((evt.current_target.unwrap(), evt.event_phase));
    });
    for &tg in &targets {
        t.doc
            .add_event_listener(tg, "foo", test_event.clone(), cap(true));
        t.doc
            .add_event_listener(tg, "foo", test_event.clone(), cap(false));
    }
    let mut evt = init_event("foo", true, true);
    t.doc.dispatch_event(node(t.target), &mut evt);
    let (actual, phases) = separar(take(&l));
    let mut esperado = targets.clone();
    esperado.extend(targets.iter().rev());
    assert_eq!(actual, esperado);
    let mut fases = vec![Capturing; 7];
    fases.extend([AtTarget, AtTarget]);
    fases.extend([Bubbling; 7]);
    assert_eq!(phases, fases);
}

// WPT: Event-dispatch-target-moved.html — "Event propagation path when an element in it is moved
// within the DOM"
#[test]
fn path_no_cambia_si_el_target_se_mueve() {
    path_fijo(true);
}

// WPT: Event-dispatch-target-removed.html — "Event propagation path when an element in it is
// removed from the DOM"
#[test]
fn path_no_cambia_si_el_target_se_quita() {
    path_fijo(false);
}

// --- Event-constructors.any.js ---

// WPT: Event-constructors.any.js — subtests 4 y 5 (`new Event("")` y `new Event("test")`)
#[test]
fn constructor_valores_por_defecto() {
    for tipo in ["", "test"] {
        let ev = Event::new(tipo, EventInit::default());
        assert_eq!(ev.type_, tipo);
        assert_eq!(ev.target, None);
        assert_eq!(ev.current_target, None);
        assert_eq!(ev.event_phase, EventPhase::None);
        assert!(!ev.bubbles);
        assert!(!ev.cancelable);
        assert!(!ev.default_prevented());
        assert!(ev.return_value());
        assert!(!ev.is_trusted);
        assert!(ev.time_stamp >= 0.0);
    }
}

// WPT: Event-constructors.any.js — subtests con diccionario (`{bubbles: true, cancelable:
// false}`, `{cancelable: true}`, `{}`) y `new CustomEvent("$", {detail: 54, cancelable: true})`
#[test]
fn constructor_con_diccionario() {
    let ev = new_event("I am an event", true, false);
    assert_eq!(ev.type_, "I am an event");
    assert!(ev.bubbles);
    assert!(!ev.cancelable);

    let ev = new_event("@", false, true);
    assert_eq!(ev.type_, "@");
    assert!(!ev.bubbles);
    assert!(ev.cancelable);

    let ev = Event::new("Xx", EventInit::default());
    assert_eq!(ev.type_, "Xx");
    assert!(!ev.bubbles);
    assert!(!ev.cancelable);

    let ev = Event::with_kind(
        "$",
        EventInit {
            cancelable: true,
            ..EventInit::default()
        },
        EventKind::Custom { detail: Some(54) },
    );
    assert_eq!(ev.type_, "$");
    assert!(!ev.bubbles);
    assert!(ev.cancelable);
    assert_eq!(ev.kind, EventKind::Custom { detail: Some(54) });
}

// --- Event-initEvent.html ---

// WPT: Event-initEvent.html — "Properties of initEvent(type, bubbles, cancelable)" (las 4
// combinaciones)
#[test]
fn init_event_propiedades() {
    for bubbles in [true, false] {
        for cancelable in [true, false] {
            let e = init_event("type", bubbles, cancelable);
            assert!(!e.default_prevented());
            assert!(e.return_value());
            assert!(!e.is_trusted);
            assert_eq!(e.target, None);
            assert_eq!(e.type_, "type");
            assert_eq!(e.bubbles, bubbles);
            assert_eq!(e.cancelable, cancelable);
        }
    }
}

// WPT: Event-initEvent.html — "Calling initEvent multiple times (getting type)." y "(not getting type)."
#[test]
fn init_event_multiples_veces() {
    let mut e = init_event("type 1", true, false);
    assert_eq!(e.type_, "type 1");
    assert!(e.bubbles);
    assert!(!e.cancelable);
    e.init_event("type 2", false, true);
    assert_eq!(e.type_, "type 2");
    assert!(!e.bubbles);
    assert!(e.cancelable);
}

// WPT: Event-initEvent.html — "Calling initEvent must not have an effect during dispatching." y
// "Calling initEvent during propagation."
#[test]
fn init_event_durante_el_dispatch_no_tiene_efecto() {
    let mut doc = Document::new();
    let target = detached(&mut doc, "div");
    let l = log();
    let l2 = l.clone();
    doc.add_event_listener(
        node(target),
        "type",
        native(move |_, e| {
            e.init_event("fail", true, true);
            l2.lock()
                .unwrap()
                .push((e.type_.to_string(), e.bubbles, e.cancelable));
        }),
        cap(false),
    );
    let mut e = init_event("type", false, false);
    assert!(doc.dispatch_event(node(target), &mut e));
    assert_eq!(take(&l), vec![("type".to_string(), false, false)]);
}

// WPT: Event-initEvent.html — "Calling initEvent must unset the stop propagation flag." y
// "... the stop immediate propagation flag."
#[test]
fn init_event_borra_los_flags_de_propagacion() {
    for inmediata in [false, true] {
        let mut doc = Document::new();
        let target = detached(&mut doc, "div");
        let mut e = Event::uninitialized(EventKind::Event);
        if inmediata {
            e.stop_immediate_propagation();
        } else {
            e.stop_propagation();
        }
        e.init_event("type", false, false);
        assert!(!e.cancel_bubble());
        let l = log();
        let l2 = l.clone();
        doc.add_event_listener(
            node(target),
            "type",
            native(move |_, _| l2.lock().unwrap().push(())),
            cap(false),
        );
        assert!(doc.dispatch_event(node(target), &mut e));
        assert_eq!(take(&l).len(), 1, "inmediata = {inmediata}");
    }
}

// --- Event-defaultPrevented.html / Event-defaultPrevented-after-dispatch.html ---

// WPT: Event-defaultPrevented.html — los 8 subtests en secuencia sobre el mismo evento
#[test]
fn default_prevented() {
    let mut ev = Event::uninitialized(EventKind::Event);
    assert!(!ev.default_prevented());
    ev.init_event("foo", true, false);
    assert!(ev.bubbles);
    assert!(!ev.cancelable);
    assert!(!ev.default_prevented());
    // preventDefault() no cambia nada si no es cancelable.
    ev.prevent_default();
    assert!(!ev.cancelable);
    assert!(!ev.default_prevented());
    // returnValue = false tampoco.
    ev.set_return_value(false);
    assert!(!ev.cancelable);
    assert!(!ev.default_prevented());
    ev.init_event("foo", true, true);
    assert!(ev.cancelable);
    assert!(!ev.default_prevented());
    ev.prevent_default();
    assert!(ev.cancelable);
    assert!(ev.default_prevented());
    ev.init_event("foo", true, true);
    ev.set_return_value(false);
    assert!(ev.default_prevented());
    // initEvent borra defaultPrevented.
    ev.init_event("foo", true, true);
    assert!(ev.bubbles);
    assert!(ev.cancelable);
    assert!(!ev.default_prevented());
}

// WPT: Event-defaultPrevented-after-dispatch.html — "Default prevention via preventDefault" y
// "Default prevention via returnValue"
#[test]
fn default_prevented_persiste_tras_el_dispatch() {
    for por_return_value in [false, true] {
        let mut doc = Document::new();
        let target = detached(&mut doc, "input");
        set(&mut doc, target, "type", "hidden");
        let l = log();
        let l2 = l.clone();
        doc.add_event_listener(
            node(target),
            "foo",
            native(move |_, e| {
                if por_return_value {
                    e.set_return_value(false);
                } else {
                    e.prevent_default();
                }
                l2.lock().unwrap().push(e.default_prevented());
            }),
            cap(true),
        );
        let mut evt = init_event("foo", true, true);
        doc.dispatch_event(node(target), &mut evt);
        assert_eq!(take(&l), vec![true], "durante el dispatch");
        assert!(evt.default_prevented(), "después del dispatch");
        assert_eq!(evt.target, Some(node(target)));
    }
}

// --- Event-cancelBubble.html ---

// WPT: Event-cancelBubble.html — "cancelBubble must be false when an event is initially created.",
// "Initializing an event must set cancelBubble to false.", "stopPropagation() must set
// cancelBubble to true.", "stopImmediatePropagation() must set cancelBubble to true." y
// "Event.cancelBubble=false must have no effect."
#[test]
fn cancel_bubble_propiedades() {
    let e = Event::uninitialized(EventKind::Event);
    assert!(!e.cancel_bubble());

    for bubbles in [true, false] {
        let mut one = Event::uninitialized(EventKind::Event);
        one.set_cancel_bubble(true);
        one.init_event("foo", bubbles, false);
        assert!(!one.cancel_bubble());
        one.set_cancel_bubble(true);
        one.init_event("foo", bubbles, false);
        assert!(!one.cancel_bubble());
    }

    let mut e = Event::uninitialized(EventKind::Event);
    e.stop_propagation();
    assert!(e.cancel_bubble());

    let mut e = Event::uninitialized(EventKind::Event);
    e.stop_immediate_propagation();
    assert!(e.cancel_bubble());

    let mut one = Event::uninitialized(EventKind::Event);
    one.stop_propagation();
    one.set_cancel_bubble(false);
    assert!(one.cancel_bubble());
}

/// `<div id=outer><div id=middle><div id=inner></div></div></div>`.
fn anidados() -> (Document, NodeId, NodeId, NodeId) {
    let (mut doc, dump) = dump();
    let outer = el(&mut doc, dump, "div");
    let middle = el(&mut doc, outer, "div");
    let inner = el(&mut doc, middle, "div");
    (doc, outer, middle, inner)
}

// WPT: Event-cancelBubble.html — "Event.cancelBubble=false must have no effect during event propagation."
#[test]
fn cancel_bubble_false_durante_la_propagacion() {
    let (mut doc, outer, middle, inner) = anidados();
    let l = log();
    let l2 = l.clone();
    doc.add_event_listener(
        node(outer),
        "barbaz",
        native(move |_, _| l2.lock().unwrap().push("outer")),
        cap(false),
    );
    doc.add_event_listener(
        node(middle),
        "barbaz",
        native(|_, e| {
            e.set_cancel_bubble(true);
            e.set_cancel_bubble(false);
        }),
        cap(false),
    );
    let mut e = init_event("barbaz", true, false);
    doc.dispatch_event(node(inner), &mut e);
    assert!(take(&l).is_empty());
}

// WPT: Event-cancelBubble.html — "cancelBubble must be false after an event has been dispatched."
#[test]
fn cancel_bubble_false_tras_el_dispatch() {
    let (mut doc, _) = dump();
    let body = doc.body().unwrap();
    doc.add_event_listener(
        node(body),
        "foobar",
        native(|_, e| e.stop_propagation()),
        cap(false),
    );
    let mut e = init_event("foobar", true, true);
    doc.dispatch_event(node(body), &mut e);
    assert!(!e.cancel_bubble());
}

// WPT: Event-cancelBubble.html — "Event.cancelBubble=true must set the stop propagation flag."
#[test]
fn cancel_bubble_true_detiene_la_propagacion() {
    let (mut doc, outer, middle, inner) = anidados();
    let l = log();
    let stopper = native(|_, e| e.set_cancel_bubble(true));
    // Burbujeo.
    doc.add_event_listener(node(middle), "bar", stopper.clone(), cap(false));
    let l2 = l.clone();
    doc.add_event_listener(
        node(outer),
        "bar",
        native(move |_, _| l2.lock().unwrap().push("listenerOne")),
        cap(false),
    );
    doc.dispatch_event(node(inner), &mut init_event("bar", true, false));
    // Captura.
    doc.add_event_listener(node(outer), "qux", stopper, cap(true));
    let l2 = l.clone();
    doc.add_event_listener(
        node(middle),
        "qux",
        native(move |_, _| l2.lock().unwrap().push("listenerTwo")),
        cap(true),
    );
    doc.dispatch_event(node(inner), &mut init_event("qux", false, false));
    assert!(take(&l).is_empty());
}

// --- Event-stopPropagation-cancel-bubbling.html / Event-stopImmediatePropagation.html ---

// WPT: Event-stopPropagation-cancel-bubbling.html — (único subtest) stopPropagation en el
// listener de captura cancela el de burbujeo del mismo target.
#[test]
fn stop_propagation_en_captura_cancela_burbujeo_en_el_target() {
    let mut doc = Document::new();
    let element = detached(&mut doc, "div");
    doc.add_event_listener(
        node(element),
        "click",
        native(|_, e| e.stop_propagation()),
        cap(true),
    );
    let l = log();
    let l2 = l.clone();
    doc.add_event_listener(
        node(element),
        "click",
        native(move |_, _| l2.lock().unwrap().push(())),
        cap(false),
    );
    doc.dispatch_event(node(element), &mut mouse("click", true, true));
    assert!(take(&l).is_empty());
}

// WPT: Event-stopImmediatePropagation.html — (único subtest)
#[test]
fn stop_immediate_propagation() {
    let mut doc = Document::new();
    let target = detached(&mut doc, "div");
    let l = log();
    let l2 = l.clone();
    doc.add_event_listener(
        node(target),
        "test",
        native(move |_, e| {
            e.stop_immediate_propagation();
            l2.lock().unwrap().push(e.cancel_bubble());
        }),
        cap(false),
    );
    let l2 = l.clone();
    doc.add_event_listener(
        node(target),
        "test",
        native(move |_, _| l2.lock().unwrap().push(false)),
        cap(false),
    );
    doc.dispatch_event(node(target), &mut new_event("test", false, false));
    assert_eq!(take(&l), vec![true]);
}

// --- EventTarget-add-remove-listener.any.js ---

// WPT: EventTarget-add-remove-listener.any.js — "Removing an event listener without explicit
// capture arg should succeed"
#[test]
fn quitar_listener_sin_capture_explicito() {
    let mut doc = Document::new();
    let et = doc.create_event_target();
    let listener = native(|_, e| e.prevent_default());
    doc.add_event_listener(et, "x", listener.clone(), cap(false));
    assert!(!doc.dispatch_event(et, &mut new_event("x", false, true)));
    doc.remove_event_listener(et, &s("x"), &listener, false);
    assert!(doc.dispatch_event(et, &mut new_event("x", false, true)));
}

// --- AddEventListenerOptions-once.any.js ---

fn once() -> ListenerOptions {
    ListenerOptions {
        once: true,
        ..ListenerOptions::default()
    }
}

// WPT: AddEventListenerOptions-once.any.js — "Once listener should be invoked only once"
#[test]
fn once_se_invoca_una_vez() {
    let mut doc = Document::new();
    let et = doc.create_event_target();
    let l = log();
    let (l1, l2) = (l.clone(), l.clone());
    doc.add_event_listener(
        et,
        "test",
        native(move |_, _| l1.lock().unwrap().push("once")),
        once(),
    );
    let normal = native(move |_, _| l2.lock().unwrap().push("normal"));
    doc.add_event_listener(et, "test", normal.clone(), ListenerOptions::default());
    doc.dispatch_event(et, &mut new_event("test", false, false));
    assert_eq!(take(&l), vec!["once", "normal"]);
    doc.dispatch_event(et, &mut new_event("test", false, false));
    assert_eq!(take(&l), vec!["normal"]);
    doc.remove_event_listener(et, &s("test"), &normal, false);
}

// WPT: AddEventListenerOptions-once.any.js — "Once listener should be invoked only once even if
// the event is nested"
#[test]
fn once_con_dispatch_anidado() {
    let mut doc = Document::new();
    let et = doc.create_event_target();
    let count = Arc::new(Mutex::new(0));
    let c = count.clone();
    doc.add_event_listener(
        et,
        "test",
        native(move |doc, _| {
            let n = {
                let mut c = c.lock().unwrap();
                *c += 1;
                *c
            };
            if n == 1 {
                doc.dispatch_event(et, &mut new_event("test", false, false));
            }
        }),
        once(),
    );
    doc.dispatch_event(et, &mut new_event("test", false, false));
    assert_eq!(*count.lock().unwrap(), 1);

    *count.lock().unwrap() = 0;
    let slot: Arc<OnceLock<Listener>> = Arc::new(OnceLock::new());
    let (c, s2) = (count.clone(), slot.clone());
    let handler2 = native(move |doc, _| {
        let n = {
            let mut c = c.lock().unwrap();
            *c += 1;
            *c
        };
        if n == 1 {
            doc.add_event_listener(et, "test", s2.get().unwrap().clone(), once());
        }
        if n <= 2 {
            doc.dispatch_event(et, &mut new_event("test", false, false));
        }
    });
    let _ = slot.set(handler2.clone());
    doc.add_event_listener(et, "test", handler2, once());
    doc.dispatch_event(et, &mut new_event("test", false, false));
    assert_eq!(*count.lock().unwrap(), 2);
}

// WPT: AddEventListenerOptions-once.any.js — "Once listener should be added / removed like
// normal listeners"
#[test]
fn once_se_agrega_y_quita_como_los_normales() {
    let mut doc = Document::new();
    let et = doc.create_event_target();
    let l = log();
    let l2 = l.clone();
    let handler = native(move |_, _| l2.lock().unwrap().push(()));
    doc.add_event_listener(et, "test", handler.clone(), once());
    doc.add_event_listener(et, "test", handler.clone(), ListenerOptions::default());
    doc.dispatch_event(et, &mut new_event("test", false, false));
    assert_eq!(take(&l).len(), 1, "se agrega una sola vez");
    doc.dispatch_event(et, &mut new_event("test", false, false));
    assert_eq!(take(&l).len(), 0, "era un listener once");
    doc.add_event_listener(et, "test", handler.clone(), once());
    doc.remove_event_listener(et, &s("test"), &handler, false);
    doc.dispatch_event(et, &mut new_event("test", false, false));
    assert_eq!(take(&l).len(), 0, "se quitó");
}

// WPT: AddEventListenerOptions-once.any.js — "Multiple once listeners should be invoked even if
// the stopImmediatePropagation is set"
#[test]
fn varios_once_con_stop_immediate_propagation() {
    let mut doc = Document::new();
    let et = doc.create_event_target();
    let l = log();
    for _ in 0..4 {
        let l2 = l.clone();
        doc.add_event_listener(
            et,
            "test",
            native(move |_, e| {
                l2.lock().unwrap().push(());
                e.stop_immediate_propagation();
            }),
            once(),
        );
    }
    for _ in 0..4 {
        doc.dispatch_event(et, &mut new_event("test", false, false));
    }
    assert_eq!(take(&l).len(), 4);
}

// --- AddEventListenerOptions-passive.any.js ---

fn passive(capture: bool, passive: Option<bool>) -> ListenerOptions {
    ListenerOptions {
        capture,
        passive,
        once: false,
    }
}

/// `testPassiveValue` / `testPassiveValueOnReturnValue`.
fn probar_passive(
    doc: &mut Document,
    et: EventTarget,
    opciones: ListenerOptions,
    esperado: bool,
    por_return_value: bool,
) {
    let l = log();
    let l2 = l.clone();
    let handler = native(move |_, e| {
        assert!(!e.default_prevented(), "defaultPrevented prematuro");
        if por_return_value {
            e.set_return_value(false);
        } else {
            e.prevent_default();
        }
        l2.lock().unwrap().push(e.default_prevented());
    });
    doc.add_event_listener(et, "test", handler.clone(), opciones);
    let uncanceled = doc.dispatch_event(et, &mut new_event("test", true, true));
    assert_eq!(take(&l), vec![esperado], "{opciones:?}");
    assert_eq!(uncanceled, !esperado, "{opciones:?}");
    doc.remove_event_listener(et, &s("test"), &handler, opciones.capture);
}

// WPT: AddEventListenerOptions-passive.any.js — "preventDefault should be ignored if-and-only-if
// the passive option is true" y "returnValue should be ignored if-and-only-if the passive option
// is true"
#[test]
fn passive_ignora_prevent_default() {
    for por_return_value in [false, true] {
        let mut doc = Document::new();
        for (p, esperado) in [(None, true), (Some(false), true), (Some(true), false)] {
            let et = doc.create_event_target();
            probar_passive(&mut doc, et, passive(false, p), esperado, por_return_value);
        }
    }
}

// WPT: AddEventListenerOptions-passive.any.js — "passive behavior of one listener should be
// unaffected by the presence of other listeners"
#[test]
fn passive_no_afecta_a_otros_listeners() {
    for (p, esperado) in [(None, true), (Some(false), true), (Some(true), false)] {
        let mut doc = Document::new();
        let et = doc.create_event_target();
        let l = log();
        let (l1, l2) = (l.clone(), l.clone());
        let dummy1 = native(move |_, _| l1.lock().unwrap().push(1));
        let dummy2 = native(move |_, _| l2.lock().unwrap().push(2));
        doc.add_event_listener(et, "test", dummy1.clone(), passive(false, Some(true)));
        doc.add_event_listener(et, "test", dummy2.clone(), ListenerOptions::default());
        probar_passive(&mut doc, et, passive(false, p), esperado, false);
        assert_eq!(take(&l), vec![1, 2]);
        doc.remove_event_listener(et, &s("test"), &dummy1, false);
        doc.remove_event_listener(et, &s("test"), &dummy2, false);
    }
}

// WPT: AddEventListenerOptions-passive.any.js — "Equivalence of option values"
#[test]
fn passive_equivalencia_de_opciones() {
    let casos = [
        (passive(true, None), passive(false, Some(false)), false),
        (passive(true, None), passive(false, Some(true)), false),
        (passive(false, None), passive(false, Some(false)), true),
        (
            passive(false, Some(true)),
            passive(false, Some(false)),
            true,
        ),
        (passive(false, None), passive(false, Some(true)), true),
        (passive(true, Some(false)), passive(true, Some(true)), true),
    ];
    for (a, b, iguales) in casos {
        let mut doc = Document::new();
        let et = doc.create_event_target();
        let l = log();
        let l2 = l.clone();
        let handler = native(move |_, _| l2.lock().unwrap().push(()));
        doc.add_event_listener(et, "test", handler.clone(), a);
        doc.add_event_listener(et, "test", handler.clone(), b);
        doc.dispatch_event(et, &mut new_event("test", true, false));
        assert_eq!(
            take(&l).len(),
            if iguales { 1 } else { 2 },
            "{a:?} vs {b:?}"
        );
    }
}

// WPT: passive-by-default.html — "${eventName} listener is passive by default for ..." (window,
// document, documentElement, body y un div; touchstart/touchmove/wheel/mousewheel/touchend)
#[test]
fn passive_por_defecto() {
    let (mut doc, dump) = dump();
    let div = el(&mut doc, dump, "div");
    let root = doc.root();
    let html = doc.document_element().unwrap();
    let body = doc.body().unwrap();
    let pasivos = [EventTarget::Window, node(root), node(html), node(body)];
    let tipos = [
        ("touchstart", true),
        ("touchmove", true),
        ("wheel", true),
        ("mousewheel", true),
        ("touchend", false),
    ];
    for (tipo, pasivo_por_tipo) in tipos {
        for tg in pasivos.iter().copied().chain([node(div)]) {
            let por_defecto = pasivo_por_tipo && tg != node(div);
            for (p, esperado_pasivo) in [
                (None, por_defecto),
                (Some(false), false),
                (Some(true), true),
            ] {
                let l = log();
                let l2 = l.clone();
                let handler = native(move |_, e| {
                    e.prevent_default();
                    l2.lock().unwrap().push(e.default_prevented());
                });
                doc.add_event_listener(tg, tipo, handler.clone(), passive(false, p));
                let ret = doc.dispatch_event(tg, &mut new_event(tipo, false, true));
                doc.remove_event_listener(tg, &s(tipo), &handler, false);
                assert_eq!(take(&l), vec![!esperado_pasivo], "{tipo} {tg:?} {p:?}");
                assert_eq!(ret, esperado_pasivo, "{tipo} {tg:?} {p:?}");
            }
        }
    }
}

// --- EventListenerOptions-capture.html ---

// WPT: EventListenerOptions-capture.html — "Capture boolean should be honored correctly" y
// "Capture option should be honored correctly" (valores ya convertidos a booleano)
#[test]
fn capture_se_respeta() {
    for (capture, fase) in [(true, Capturing), (false, Bubbling)] {
        let mut t = Tabla::new();
        let document = t.document();
        let l = log();
        let l2 = l.clone();
        let handler = native(move |_, e| l2.lock().unwrap().push(e.event_phase));
        t.doc
            .add_event_listener(document, "test", handler.clone(), cap(capture));
        t.doc
            .dispatch_event(node(t.body), &mut new_event("test", true, false));
        t.doc
            .remove_event_listener(document, &s("test"), &handler, capture);
        t.doc
            .dispatch_event(node(t.body), &mut new_event("test", true, false));
        assert_eq!(take(&l), vec![fase]);
    }
}

// WPT: EventListenerOptions-capture.html — "Equivalence of option values" (add y remove con el
// mismo/distinto capture)
#[test]
fn capture_equivalencia_add_remove() {
    for (add, remove, iguales) in [
        (false, false, true),
        (true, true, true),
        (true, false, false),
        (false, true, false),
    ] {
        let mut t = Tabla::new();
        let document = t.document();
        let l = log();
        let l2 = l.clone();
        let handler = native(move |_, _| l2.lock().unwrap().push(()));
        t.doc
            .add_event_listener(document, "test", handler.clone(), cap(add));
        t.doc
            .remove_event_listener(document, &s("test"), &handler, remove);
        t.doc
            .dispatch_event(node(t.body), &mut new_event("test", true, false));
        assert_eq!(take(&l).is_empty(), iguales, "add {add}, remove {remove}");
    }
}

// --- Event-timestamp-high-resolution.html ---

// WPT: Event-timestamp-high-resolution.html — "Constructed MouseEvent timestamp should be high
// resolution and have the same time origin as performance.now()" (adaptado: sin
// `performance.now()`, se comprueba que el reloj es monótono y de alta resolución).
#[test]
fn timestamp_monotono() {
    let a = mouse("test", false, false);
    std::thread::sleep(std::time::Duration::from_millis(2));
    let b = new_event("test", false, false);
    assert!(b.time_stamp > a.time_stamp);
    assert!(b.time_stamp - a.time_stamp >= 1.0);
    assert!(b.time_stamp > 0.0);
}
