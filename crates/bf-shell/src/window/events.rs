//! Los eventos del usuario en el hilo del documento (UI Events, HTML §6.6 y §4.10.21): la
//! ventana manda cada entrada ([`UserInput`]) y aquí se despacha al DOM como evento de
//! confianza, con las acciones por defecto que no se cancelaron (seguir un enlace, enviar un
//! formulario, abrir el menú contextual del navegador).
//!
//! La ventana sigue haciendo lo inmediato (caret, selección, scroll); lo que JavaScript puede
//! cancelar lo decide el hilo después del dispatch.

use std::collections::HashSet;
use std::sync::Arc;

use bf_dom::events::{
    Event, EventInit, EventKind, EventTarget, InputEventData, KeyboardEventData, MouseEventData,
    NoScript,
};
use bf_dom::{Document, NodeId};
use bf_infra::Url;

use super::edit::{self, Editable};
use crate::load::Loaded;
use crate::script::{Engine, PageRequest};

/// Crea un evento del puntero: tipo, `bubbles`, `cancelable` y `relatedTarget`.
type MouseEventFactory<'a> = &'a dyn Fn(&str, bool, bool, Option<EventTarget>) -> Event;

/// Las teclas modificadoras.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

/// Qué hizo el puntero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Down,
    Up,
    Click,
    Move,
    ContextMenu,
}

/// Una entrada del usuario sobre la página.
#[derive(Debug, Clone, PartialEq)]
pub enum UserInput {
    /// El puntero en `client` (px CSS del viewport) sobre `target`.
    Mouse {
        kind: MouseKind,
        client: (f32, f32),
        /// Posición en la pantalla (px CSS).
        screen: (f32, f32),
        button: i16,
        buttons: u16,
        mods: Mods,
        /// Cantidad de clics seguidos (`detail`).
        detail: i32,
        target: Option<NodeId>,
    },
    /// La rueda (`wheel`), con el desplazamiento pedido en px CSS.
    Wheel {
        client: (f32, f32),
        delta: (f32, f32),
        mods: Mods,
        target: Option<NodeId>,
    },
    /// Una tecla en el elemento con el foco (o el `body`).
    Key {
        down: bool,
        key: String,
        code: String,
        mods: Mods,
        repeat: bool,
        /// Lo que escribe, si escribe algo (`keypress`).
        text: Option<String>,
    },
    /// El usuario cambió el valor de un campo.
    SetValue {
        node: NodeId,
        kind: Editable,
        value: String,
        /// `inputType` del `InputEvent`.
        input_type: &'static str,
        data: Option<String>,
    },
    /// Envío implícito (Enter en un campo, HTML §4.10.21.2).
    ImplicitSubmit { field: NodeId },
    /// El foco pasó a `node` (o a ninguno).
    Focus(Option<NodeId>),
}

/// El estado de los eventos de una pestaña.
#[derive(Debug, Default)]
pub struct InputState {
    /// El elemento bajo el puntero (`mouseover`/`mouseout`).
    hover: Option<NodeId>,
    /// El elemento con el foco.
    focus: Option<NodeId>,
    /// Campos cambiados desde que tomaron el foco (`change` al perderlo).
    changed: HashSet<NodeId>,
}

impl InputState {
    /// Un documento nuevo: nada bajo el puntero ni con el foco.
    pub fn reset(&mut self) {
        *self = InputState::default();
    }
}

/// El documento de la pestaña: el del motor o, sin motor (páginas internas), el cargado.
struct Target<'a> {
    engine: Option<&'a mut Engine>,
    loaded: &'a mut Arc<Loaded>,
}

impl Target<'_> {
    fn doc(&self) -> &Document {
        match &self.engine {
            Some(e) => e.document().unwrap_or(&self.loaded.doc),
            None => &self.loaded.doc,
        }
    }

    fn doc_mut(&mut self) -> &mut Document {
        match &mut self.engine {
            Some(e) if e.document().is_some() => e.document_mut().expect("documento"),
            _ => &mut Arc::make_mut(self.loaded).doc,
        }
    }

    /// Despacha `event` (de confianza) en `target`; `false` si se canceló.
    fn dispatch(&mut self, target: EventTarget, mut event: Event) -> bool {
        match &mut self.engine {
            Some(e) if e.document().is_some() => e.dispatch(target, &mut event),
            _ => {
                event.is_trusted = true;
                Arc::make_mut(self.loaded)
                    .doc
                    .dispatch(target, &mut event, &mut NoScript)
            }
        }
    }

    /// El documento con los valores que escribió el usuario (para enviar formularios).
    fn form_doc(&self) -> Document {
        match &self.engine {
            Some(e) => e.render_copy().unwrap_or_else(|| self.loaded.doc.clone()),
            None => self.loaded.doc.clone(),
        }
    }
}

fn init(bubbles: bool, cancelable: bool) -> EventInit {
    EventInit {
        bubbles,
        cancelable,
        composed: true,
    }
}

fn node_target(doc: &Document, n: Option<NodeId>) -> EventTarget {
    EventTarget::Node(n.filter(|&n| doc.is_connected(n)).unwrap_or_else(|| {
        doc.body()
            .or_else(|| doc.document_element())
            .unwrap_or(doc.root())
    }))
}

/// El elemento de `n` (los eventos del puntero van a elementos, no a nodos de texto).
fn element_of(doc: &Document, n: Option<NodeId>) -> Option<NodeId> {
    let mut cur = n;
    while let Some(c) = cur {
        if doc.element(c).is_some() {
            return Some(c);
        }
        cur = doc.parent(c);
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn mouse_event(
    type_: &str,
    bubbles: bool,
    cancelable: bool,
    client: (f32, f32),
    screen: (f32, f32),
    button: i16,
    buttons: u16,
    mods: Mods,
    detail: i32,
    related: Option<EventTarget>,
) -> Event {
    Event::with_kind(
        type_,
        init(bubbles, cancelable),
        EventKind::Mouse(MouseEventData {
            screen_x: f64::from(screen.0),
            screen_y: f64::from(screen.1),
            client_x: f64::from(client.0),
            client_y: f64::from(client.1),
            ctrl_key: mods.ctrl,
            shift_key: mods.shift,
            alt_key: mods.alt,
            meta_key: mods.meta,
            button,
            buttons,
            related_target: related,
            detail,
        }),
    )
}

/// Los ancestros inclusivos de `n`.
fn ancestors(doc: &Document, n: Option<NodeId>) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut cur = n;
    while let Some(c) = cur {
        out.push(c);
        cur = doc.parent(c);
    }
    out
}

/// Despacha `input` en el documento de la pestaña y devuelve lo que la ventana debe hacer
/// después (navegar, abrir el menú contextual…).
pub fn handle(
    engine: Option<&mut Engine>,
    loaded: &mut Arc<Loaded>,
    state: &mut InputState,
    input: UserInput,
) -> Vec<PageRequest> {
    let mut t = Target { engine, loaded };
    let mut out = Vec::new();
    match input {
        UserInput::Mouse {
            kind,
            client,
            screen,
            button,
            buttons,
            mods,
            detail,
            target,
        } => {
            let target = element_of(t.doc(), target);
            let tgt = node_target(t.doc(), target);
            let ev = |type_: &str, bubbles: bool, cancelable: bool, related| {
                mouse_event(
                    type_, bubbles, cancelable, client, screen, button, buttons, mods, detail,
                    related,
                )
            };
            match kind {
                MouseKind::Move => {
                    hover(&mut t, state, target, &ev);
                    t.dispatch(tgt, ev("mousemove", true, true, None));
                }
                MouseKind::Down => {
                    hover(&mut t, state, target, &ev);
                    t.dispatch(tgt, ev("mousedown", true, true, None));
                }
                MouseKind::Up => {
                    t.dispatch(tgt, ev("mouseup", true, true, None));
                }
                MouseKind::Click => {
                    // El activation behavior (`bf-dom`) corre si nadie llamó a
                    // `preventDefault`; queda en la cola de activaciones.
                    t.doc_mut().take_activations();
                    t.dispatch(tgt, ev("click", true, true, None));
                    activations(&mut t, mods, &mut out);
                }
                MouseKind::ContextMenu => {
                    if t.dispatch(tgt, ev("contextmenu", true, true, None)) {
                        out.push(PageRequest::ContextMenu);
                    }
                }
            }
        }
        UserInput::Wheel {
            client,
            delta,
            mods,
            target,
        } => {
            let target = element_of(t.doc(), target);
            let tgt = node_target(t.doc(), target);
            // `WheelEvent` como `MouseEvent` (sin `deltaX`/`deltaY` todavía).
            let _ = delta;
            t.dispatch(
                tgt,
                mouse_event("wheel", true, true, client, client, 0, 0, mods, 0, None),
            );
        }
        UserInput::Key {
            down,
            key,
            code,
            mods,
            repeat,
            text,
        } => {
            let tgt = node_target(t.doc(), state.focus);
            let data = KeyboardEventData {
                key: key.as_str().into(),
                code: code.as_str().into(),
                location: 0,
                ctrl_key: mods.ctrl,
                shift_key: mods.shift,
                alt_key: mods.alt,
                meta_key: mods.meta,
                repeat,
                is_composing: false,
            };
            let type_ = if down { "keydown" } else { "keyup" };
            let ok = t.dispatch(
                tgt,
                Event::with_kind(type_, init(true, true), EventKind::Keyboard(data.clone())),
            );
            // `keypress` (en desuso) para las teclas que escriben.
            if down && ok && text.is_some() && !mods.ctrl && !mods.meta {
                t.dispatch(
                    tgt,
                    Event::with_kind("keypress", init(true, true), EventKind::Keyboard(data)),
                );
            }
        }
        UserInput::SetValue {
            node,
            kind,
            value,
            input_type,
            data,
        } => {
            set_value(&mut t, node, kind, &value);
            state.changed.insert(node);
            let tgt = EventTarget::Node(node);
            t.dispatch(
                tgt,
                Event::with_kind(
                    "input",
                    init(true, false),
                    EventKind::Input(InputEventData {
                        data: data.map(|d| d.as_str().into()),
                        is_composing: false,
                        input_type: input_type.into(),
                    }),
                ),
            );
        }
        UserInput::ImplicitSubmit { field } => {
            if let Some(form) = super::form_of(t.doc(), field) {
                submit(&mut t, form, None, &mut out);
            }
        }
        UserInput::Focus(node) => focus(&mut t, state, node),
    }
    out
}

/// `mouseout`/`mouseleave` en lo que deja el puntero y `mouseover`/`mouseenter` en lo nuevo
/// (UI Events §5.3.3).
fn hover(
    t: &mut Target<'_>,
    state: &mut InputState,
    target: Option<NodeId>,
    ev: MouseEventFactory<'_>,
) {
    if state.hover == target {
        return;
    }
    let old = state.hover.filter(|&n| t.doc().is_connected(n));
    state.hover = target;
    let old_chain = ancestors(t.doc(), old);
    let new_chain = ancestors(t.doc(), target);
    let rel = |n: Option<NodeId>| n.map(EventTarget::Node);
    if let Some(o) = old {
        t.dispatch(
            EventTarget::Node(o),
            ev("mouseout", true, true, rel(target)),
        );
        for &n in old_chain.iter().filter(|n| !new_chain.contains(n)) {
            if t.doc().element(n).is_some() {
                t.dispatch(
                    EventTarget::Node(n),
                    ev("mouseleave", false, false, rel(target)),
                );
            }
        }
    }
    if let Some(n) = target {
        t.dispatch(EventTarget::Node(n), ev("mouseover", true, true, rel(old)));
        for &a in new_chain.iter().rev().filter(|a| !old_chain.contains(a)) {
            if t.doc().element(a).is_some() {
                t.dispatch(
                    EventTarget::Node(a),
                    ev("mouseenter", false, false, rel(old)),
                );
            }
        }
    }
}

/// "Focus update steps" (HTML §6.6.4): `change` si el campo cambió, `blur`/`focusout` en el
/// viejo y `focus`/`focusin` en el nuevo.
fn focus(t: &mut Target<'_>, state: &mut InputState, node: Option<NodeId>) {
    if state.focus == node {
        return;
    }
    let old = state.focus.filter(|&n| t.doc().is_connected(n));
    state.focus = node;
    if let Some(e) = &mut t.engine {
        e.set_focus(node);
    }
    let focus_ev = |type_: &str, related: Option<NodeId>| {
        let bubbles = type_ == "focusin" || type_ == "focusout";
        Event::with_kind(
            type_,
            init(bubbles, false),
            EventKind::Focus {
                related_target: related.map(EventTarget::Node),
            },
        )
    };
    if let Some(o) = old {
        if state.changed.remove(&o) {
            t.dispatch(
                EventTarget::Node(o),
                Event::new("change", init(true, false)),
            );
        }
        t.dispatch(EventTarget::Node(o), focus_ev("blur", node));
        t.dispatch(EventTarget::Node(o), focus_ev("focusout", node));
    }
    if let Some(n) = node {
        state.changed.remove(&n);
        t.dispatch(EventTarget::Node(n), focus_ev("focus", old));
        t.dispatch(EventTarget::Node(n), focus_ev("focusin", old));
    }
}

/// Escribe el valor de un campo: el *dirty value* de `HTMLInputElement` (con motor) o, sin
/// motor, el atributo como antes.
fn set_value(t: &mut Target<'_>, node: NodeId, kind: Editable, value: &str) {
    match &mut t.engine {
        Some(e) if e.document().is_some() => e.set_control_value(node, value),
        _ => edit::set_value(&mut Arc::make_mut(t.loaded).doc, node, kind, value),
    }
}

/// Las acciones por defecto que dejó el dispatch (enlaces, envíos).
fn activations(t: &mut Target<'_>, mods: Mods, out: &mut Vec<PageRequest>) {
    for a in t.doc_mut().take_activations() {
        match a {
            bf_dom::events::Activation::FollowHyperlink(n) => {
                let doc = t.doc();
                let Some(href) = doc.element(n).and_then(|e| e.attr("href")) else {
                    continue;
                };
                let base = doc.base_url();
                let Ok(url) = Url::parse(href.trim(), Some(&base)) else {
                    continue;
                };
                let blank = doc
                    .element(n)
                    .and_then(|e| e.attr("target"))
                    .is_some_and(|t| t.eq_ignore_ascii_case("_blank"));
                if mods.ctrl || blank {
                    out.push(PageRequest::Open(url.href()));
                } else {
                    out.push(PageRequest::Navigate {
                        url,
                        replace: false,
                        post: None,
                    });
                }
            }
            bf_dom::events::Activation::Submit(submitter) => {
                if let Some(form) = super::form_of(t.doc(), submitter) {
                    submit(t, form, Some(submitter), out);
                }
            }
            bf_dom::events::Activation::Reset(_) => {}
            // E1: `label` y `summary` los completa el motor (bf-web).
            a @ (bf_dom::events::Activation::Label(_) | bf_dom::events::Activation::Summary(_)) => {
                if let Some(e) = &mut t.engine {
                    e.run_activation(a);
                }
            }
        }
    }
}

/// Envío de un formulario (HTML §4.10.21.3): `submit` cancelable y, si no se canceló, la
/// navegación.
fn submit(t: &mut Target<'_>, form: NodeId, submitter: Option<NodeId>, out: &mut Vec<PageRequest>) {
    let ev = Event::with_kind("submit", init(true, true), EventKind::Event);
    if !t.dispatch(EventTarget::Node(form), ev) {
        return;
    }
    let doc = t.form_doc();
    let base = doc.base_url();
    let url = doc.url.clone();
    let (target, post) = super::form_submission(&doc, &base, &url, form, submitter);
    out.push(PageRequest::Navigate {
        url: target,
        replace: false,
        post,
    });
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::load::Meta;
    use crate::script::{ConsoleEntry, PageEnv};

    struct Page {
        engine: Engine,
        loaded: Arc<Loaded>,
        state: InputState,
        log: Rc<RefCell<Vec<ConsoleEntry>>>,
    }

    impl Page {
        fn new(html: &str) -> Page {
            let log = Rc::new(RefCell::new(Vec::new()));
            let sink = log.clone();
            let mut engine = Engine::new(Box::new(move |e| sink.borrow_mut().push(e)));
            let net = Arc::new(bf_net::NetLoader::new());
            let env = PageEnv::cli(net.clone());
            let url = Url::parse("https://example.test/dir/", None).unwrap();
            let loader = Arc::new(crate::load::CachedLoader::new(net.clone()));
            engine.parse(html, &url, loader, &env);
            let loaded = Arc::new(Loaded::generated(&url, "", net, Meta::default()));
            Page {
                engine,
                loaded,
                state: InputState::default(),
                log,
            }
        }

        fn id(&self, id: &str) -> NodeId {
            self.engine
                .document()
                .unwrap()
                .get_element_by_id(&id.into())
                .unwrap()
        }

        fn send(&mut self, input: UserInput) -> Vec<PageRequest> {
            handle(
                Some(&mut self.engine),
                &mut self.loaded,
                &mut self.state,
                input,
            )
        }

        fn mouse(&mut self, kind: MouseKind, target: &str, mods: Mods) -> Vec<PageRequest> {
            let t = self.id(target);
            self.send(UserInput::Mouse {
                kind,
                client: (10.0, 20.0),
                screen: (10.0, 20.0),
                button: 0,
                buttons: 0,
                mods,
                detail: 1,
                target: Some(t),
            })
        }

        fn eval(&mut self, src: &str) -> String {
            let realm = self.engine.realm;
            let v = self.engine.rt.eval_script(realm, src, "t.js").unwrap();
            self.engine.rt.to_display(&v)
        }

        fn console(&self) -> Vec<String> {
            self.log.borrow().iter().map(|e| e.text.clone()).collect()
        }
    }

    fn navigations(r: &[PageRequest]) -> Vec<String> {
        r.iter()
            .filter_map(|r| match r {
                PageRequest::Navigate { url, post, .. } => {
                    Some(format!("{} {}", url.href(), post.is_some()))
                }
                PageRequest::Open(u) => Some(format!("open {u}")),
                PageRequest::ContextMenu => Some("menu".to_owned()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn click_follows_link_unless_prevented() {
        let mut p = Page::new(
            "<a id=a href=otra.html>x</a><a id=b href=/nunca>y</a>\
             <script>document.getElementById('b').addEventListener('click', function (e) { \
             e.preventDefault(); console.log('cancelado', e.isTrusted, e.clientX, e.detail); });</script>",
        );
        let r = p.mouse(MouseKind::Click, "a", Mods::default());
        assert_eq!(
            navigations(&r),
            ["https://example.test/dir/otra.html false"]
        );
        let r = p.mouse(MouseKind::Click, "b", Mods::default());
        assert!(navigations(&r).is_empty());
        assert_eq!(p.console(), ["cancelado true 10 1"]);
        let ctrl = Mods {
            ctrl: true,
            ..Mods::default()
        };
        let r = p.mouse(MouseKind::Click, "a", ctrl);
        assert_eq!(navigations(&r), ["open https://example.test/dir/otra.html"]);
    }

    #[test]
    fn submit_is_cancelable_and_uses_typed_values() {
        let mut p = Page::new(
            "<form id=f action=/buscar><input id=q name=q><button id=go>ir</button></form>\
             <form id=g action=/no method=post><button id=no>no</button></form>\
             <script>document.getElementById('g').addEventListener('submit', function (e) { e.preventDefault(); });</script>",
        );
        let q = p.id("q");
        p.send(UserInput::SetValue {
            node: q,
            kind: Editable::Input,
            value: "hola mundo".into(),
            input_type: "insertText",
            data: None,
        });
        assert_eq!(p.eval("document.getElementById('q').value"), "hola mundo");
        let r = p.mouse(MouseKind::Click, "go", Mods::default());
        assert_eq!(
            navigations(&r),
            ["https://example.test/buscar?q=hola+mundo false"]
        );
        let r = p.mouse(MouseKind::Click, "no", Mods::default());
        assert!(navigations(&r).is_empty());
        let r = p.send(UserInput::ImplicitSubmit { field: q });
        assert_eq!(navigations(&r).len(), 1);
    }

    #[test]
    fn typing_fires_input_and_change_on_blur() {
        let mut p = Page::new(
            "<input id=a><input id=b><script>var log = []; ['input', 'change', 'focus', 'blur', \
             'focusin', 'focusout'].forEach(function (t) { document.addEventListener(t, function (e) { \
             log.push(t + ':' + e.target.id); }, true); });</script>",
        );
        let (a, b) = (p.id("a"), p.id("b"));
        p.send(UserInput::Focus(Some(a)));
        p.send(UserInput::SetValue {
            node: a,
            kind: Editable::Input,
            value: "x".into(),
            input_type: "insertText",
            data: Some("x".into()),
        });
        p.send(UserInput::Focus(Some(b)));
        assert_eq!(
            p.eval("log.join()"),
            "focus:a,focusin:a,input:a,change:a,blur:a,focusout:a,focus:b,focusin:b"
        );
        // El documento que se maqueta muestra lo escrito.
        let copy = p.engine.render_copy().unwrap();
        assert_eq!(copy.element(a).unwrap().attr("value"), Some("x"));
    }

    #[test]
    fn hover_fires_over_out_enter_leave() {
        let mut p = Page::new(
            "<div id=d><span id=s>x</span></div><p id=p>y</p><script>var log = []; \
             ['mouseover', 'mouseout', 'mouseenter', 'mouseleave', 'mousemove'].forEach(function (t) { \
             document.addEventListener(t, function (e) { log.push(t + ':' + (e.target.id || e.target.nodeName)); }, true); });</script>",
        );
        p.mouse(MouseKind::Move, "s", Mods::default());
        p.mouse(MouseKind::Move, "p", Mods::default());
        assert_eq!(
            p.eval("log.join()"),
            "mouseover:s,mouseenter:HTML,mouseenter:BODY,mouseenter:d,mouseenter:s,mousemove:s,\
             mouseout:s,mouseleave:s,mouseleave:d,mouseover:p,mouseenter:p,mousemove:p"
        );
    }

    #[test]
    fn context_menu_is_cancelable() {
        let mut p = Page::new(
            "<p id=a>a</p><p id=b>b</p><script>document.getElementById('b').addEventListener(\
             'contextmenu', function (e) { e.preventDefault(); });</script>",
        );
        assert_eq!(
            navigations(&p.mouse(MouseKind::ContextMenu, "a", Mods::default())),
            ["menu"]
        );
        assert!(navigations(&p.mouse(MouseKind::ContextMenu, "b", Mods::default())).is_empty());
    }

    #[test]
    fn keyboard_events() {
        let mut p = Page::new(
            "<input id=a><script>var log = []; ['keydown', 'keypress', 'keyup'].forEach(function (t) { \
             document.addEventListener(t, function (e) { log.push(t + ':' + e.key + ':' + e.code + ':' + e.target.id); }); });</script>",
        );
        let a = p.id("a");
        p.send(UserInput::Focus(Some(a)));
        for down in [true, false] {
            p.send(UserInput::Key {
                down,
                key: "q".into(),
                code: "KeyQ".into(),
                mods: Mods::default(),
                repeat: false,
                text: Some("q".into()),
            });
        }
        assert_eq!(
            p.eval("log.join()"),
            "keydown:q:KeyQ:a,keypress:q:KeyQ:a,keyup:q:KeyQ:a"
        );
    }
}
