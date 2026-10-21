//! Eventos (DOM §2): `Event` y sus subclases como datos, `EventTarget` (nodos, `Window` y
//! objetos no-nodo como `AbortSignal`), `addEventListener`/`removeEventListener` y el dispatch
//! de §2.9 (sin shadow trees: no hay retargeting ni slots).
//!
//! Los listeners nativos son closures de Rust; los de script son un handle opaco que el motor
//! JavaScript resuelve con [`ScriptHost`]. La *activation behavior* de `a`, `area`, `input` y
//! `button` (y la *legacy-pre-activation* de checkbox y radio) se ejecuta aquí; las acciones
//! que salen del DOM (seguir un hipervínculo, enviar o resetear un formulario) se dejan en
//! [`Document::take_activations`] para el navegador.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use bf_infra::DomString;

use crate::{Document, DomException, NodeData, NodeId};

/// Un `EventTarget`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventTarget {
    Node(NodeId),
    /// El `Window` del documento (el padre de `Document` en el event path).
    Window,
    /// Un objeto no-nodo (`AbortSignal`, ...), identificado por el documento.
    Object(u64),
}

/// `Event.eventPhase`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u16)]
pub enum EventPhase {
    #[default]
    None = 0,
    Capturing = 1,
    AtTarget = 2,
    Bubbling = 3,
}

/// Datos de `MouseEvent` (UI Events §5.3).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MouseEventData {
    pub screen_x: f64,
    pub screen_y: f64,
    pub client_x: f64,
    pub client_y: f64,
    pub ctrl_key: bool,
    pub shift_key: bool,
    pub alt_key: bool,
    pub meta_key: bool,
    pub button: i16,
    pub buttons: u16,
    pub related_target: Option<EventTarget>,
    /// `UIEvent.detail` (cantidad de clics).
    pub detail: i32,
}

/// Datos de `KeyboardEvent` (UI Events §5.6).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KeyboardEventData {
    pub key: DomString,
    pub code: DomString,
    pub location: u32,
    pub ctrl_key: bool,
    pub shift_key: bool,
    pub alt_key: bool,
    pub meta_key: bool,
    pub repeat: bool,
    pub is_composing: bool,
}

/// Datos de `InputEvent` (UI Events §5.5).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InputEventData {
    pub data: Option<DomString>,
    pub is_composing: bool,
    pub input_type: DomString,
}

/// La interfaz del evento y sus datos propios.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum EventKind {
    #[default]
    Event,
    /// `CustomEvent`: `detail` es un valor de script (handle) o nulo.
    Custom {
        detail: Option<u64>,
    },
    /// `UIEvent`.
    Ui {
        detail: i32,
    },
    Mouse(MouseEventData),
    Keyboard(KeyboardEventData),
    /// `FocusEvent`.
    Focus {
        related_target: Option<EventTarget>,
    },
    Input(InputEventData),
}

/// `EventInit`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EventInit {
    pub bubbles: bool,
    pub cancelable: bool,
    pub composed: bool,
}

/// Una entrada del event path (§2.9).
#[derive(Debug, Clone, PartialEq)]
struct PathEntry {
    invocation_target: EventTarget,
    shadow_adjusted_target: Option<EventTarget>,
}

/// `Event`.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub type_: DomString,
    pub kind: EventKind,
    pub target: Option<EventTarget>,
    pub current_target: Option<EventTarget>,
    pub event_phase: EventPhase,
    pub bubbles: bool,
    pub cancelable: bool,
    pub composed: bool,
    pub is_trusted: bool,
    /// Milisegundos desde el origen de tiempo del proceso.
    pub time_stamp: f64,
    stop_propagation: bool,
    stop_immediate_propagation: bool,
    canceled: bool,
    in_passive_listener: bool,
    initialized: bool,
    dispatch: bool,
    path: Vec<PathEntry>,
}

fn now_ms() -> f64 {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

impl Event {
    /// `new Event(type, eventInitDict)` (y las subclases con `kind`).
    pub fn new(type_: impl Into<DomString>, init: EventInit) -> Event {
        Event {
            type_: type_.into(),
            kind: EventKind::Event,
            target: None,
            current_target: None,
            event_phase: EventPhase::None,
            bubbles: init.bubbles,
            cancelable: init.cancelable,
            composed: init.composed,
            is_trusted: false,
            time_stamp: now_ms(),
            stop_propagation: false,
            stop_immediate_propagation: false,
            canceled: false,
            in_passive_listener: false,
            initialized: true,
            dispatch: false,
            path: Vec::new(),
        }
    }

    /// Un evento de la interfaz `kind`.
    pub fn with_kind(type_: impl Into<DomString>, init: EventInit, kind: EventKind) -> Event {
        let mut e = Event::new(type_, init);
        e.kind = kind;
        e
    }

    /// `document.createEvent(...)`: sin inicializar hasta `initEvent`.
    pub fn uninitialized(kind: EventKind) -> Event {
        let mut e = Event::new("", EventInit::default());
        e.kind = kind;
        e.initialized = false;
        e
    }

    /// `initEvent(type, bubbles, cancelable)`.
    pub fn init_event(&mut self, type_: impl Into<DomString>, bubbles: bool, cancelable: bool) {
        if self.dispatch {
            return;
        }
        self.initialized = true;
        self.stop_propagation = false;
        self.stop_immediate_propagation = false;
        self.canceled = false;
        self.is_trusted = false;
        self.target = None;
        self.type_ = type_.into();
        self.bubbles = bubbles;
        self.cancelable = cancelable;
    }

    pub fn stop_propagation(&mut self) {
        self.stop_propagation = true;
    }

    pub fn stop_immediate_propagation(&mut self) {
        self.stop_propagation = true;
        self.stop_immediate_propagation = true;
    }

    /// `cancelBubble`.
    pub fn cancel_bubble(&self) -> bool {
        self.stop_propagation
    }

    pub fn set_cancel_bubble(&mut self, value: bool) {
        if value {
            self.stop_propagation = true;
        }
    }

    /// "Set the canceled flag".
    fn set_canceled(&mut self) {
        if self.cancelable && !self.in_passive_listener {
            self.canceled = true;
        }
    }

    pub fn prevent_default(&mut self) {
        self.set_canceled();
    }

    pub fn default_prevented(&self) -> bool {
        self.canceled
    }

    /// `returnValue`.
    pub fn return_value(&self) -> bool {
        !self.canceled
    }

    pub fn set_return_value(&mut self, value: bool) {
        if !value {
            self.set_canceled();
        }
    }

    /// ¿Está en dispatch?
    pub fn is_dispatching(&self) -> bool {
        self.dispatch
    }

    /// `composedPath()` (§2.2): sin shadow trees, los invocation targets del path en orden.
    pub fn composed_path(&self) -> Vec<EventTarget> {
        if self.path.is_empty() || self.current_target.is_none() {
            return Vec::new();
        }
        self.path.iter().map(|p| p.invocation_target).collect()
    }

    /// ¿Es un `MouseEvent`?
    pub fn is_mouse_event(&self) -> bool {
        matches!(self.kind, EventKind::Mouse(_))
    }
}

/// Callback de un listener nativo.
pub type NativeListener = Arc<dyn Fn(&mut Document, &mut Event) + Send + Sync>;

/// El callback de un event listener.
#[derive(Clone)]
pub enum Listener {
    Native(NativeListener),
    /// Un callback de JavaScript (handle del motor).
    Script(u64),
}

impl Listener {
    fn same(&self, other: &Listener) -> bool {
        match (self, other) {
            (Listener::Native(a), Listener::Native(b)) => Arc::ptr_eq(a, b),
            (Listener::Script(a), Listener::Script(b)) => a == b,
            _ => false,
        }
    }
}

impl fmt::Debug for Listener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Listener::Native(_) => f.write_str("Native(..)"),
            Listener::Script(h) => write!(f, "Script({h})"),
        }
    }
}

/// `AddEventListenerOptions` (`passive: None` es el valor por defecto según el tipo).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListenerOptions {
    pub capture: bool,
    pub passive: Option<bool>,
    pub once: bool,
}

#[derive(Debug, Clone)]
struct ListenerEntry {
    type_: DomString,
    callback: Listener,
    capture: bool,
    passive: bool,
    once: bool,
    /// El flag *removed*; compartido con las copias de la lista que hace el dispatch.
    removed: Arc<AtomicBool>,
}

/// Listeners por target.
#[derive(Debug, Clone, Default)]
pub(crate) struct Listeners {
    map: HashMap<EventTarget, Vec<ListenerEntry>>,
    next_object: u64,
    activations: Vec<Activation>,
    /// E1: los `details` cuyo atributo `open` apareció o desapareció (HTML §4.11.1), con su
    /// estado nuevo.
    details_toggles: Vec<(NodeId, bool)>,
}

/// Una acción de *activation behavior* que debe completar el navegador.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activation {
    /// "Follow the hyperlink" de un `a`/`area` con `href`.
    FollowHyperlink(NodeId),
    /// Enviar el formulario del submitter.
    Submit(NodeId),
    /// Resetear el formulario del botón.
    Reset(NodeId),
    /// E1: un `label` reenvía el click a su control (HTML §4.10.4).
    Label(NodeId),
    /// E1: el `summary` de un `details` abre o cierra su `details` (HTML §4.11.2).
    Summary(NodeId),
}

/// Ejecuta los listeners de script.
pub trait ScriptHost {
    /// Llama al callback `handle` con `event`; devuelve `true` si lanzó una excepción.
    fn call_listener(&mut self, doc: &mut Document, handle: u64, event: &mut Event) -> bool;

    /// E3: antes de leer los listeners de `target` para `type_` (paso 6 de "invoke"): el host
    /// registra el event handler del atributo de contenido si todavía no lo hizo (HTML
    /// §8.1.8.1, los atributos `on…` se compilan perezosamente).
    fn before_invoke(&mut self, _doc: &mut Document, _target: EventTarget, _type_: &DomString) {}
}

/// Sin motor de script: los listeners de script no hacen nada.
pub struct NoScript;

impl ScriptHost for NoScript {
    fn call_listener(&mut self, _: &mut Document, _: u64, _: &mut Event) -> bool {
        false
    }
}

/// Tipos con passive por defecto en `Window`, el documento, su document element y su body.
const PASSIVE_TYPES: [&str; 4] = ["touchstart", "touchmove", "wheel", "mousewheel"];

impl Document {
    /// "Erase all event listeners and handlers" de cada nodo del documento y del `Window`
    /// (`document.open()`, HTML §8.4.1 pasos 9-10).
    pub fn erase_all_event_listeners(&mut self) {
        self.listeners
            .map
            .retain(|target, _| matches!(target, EventTarget::Object(_)));
    }

    /// Un nuevo `EventTarget` no-nodo (`new EventTarget()`, `AbortSignal`, ...).
    pub fn create_event_target(&mut self) -> EventTarget {
        self.listeners.next_object += 1;
        EventTarget::Object(self.listeners.next_object)
    }

    /// "Default passive value".
    fn default_passive(&self, type_: &DomString, target: EventTarget) -> bool {
        if !PASSIVE_TYPES.iter().any(|t| *type_ == *t) {
            return false;
        }
        match target {
            EventTarget::Window => true,
            EventTarget::Node(n) => {
                n == self.root() || Some(n) == self.document_element() || Some(n) == self.body()
            }
            EventTarget::Object(_) => false,
        }
    }

    /// `addEventListener(type, callback, options)` ("add an event listener").
    pub fn add_event_listener(
        &mut self,
        target: EventTarget,
        type_: impl Into<DomString>,
        callback: Listener,
        options: ListenerOptions,
    ) {
        let type_ = type_.into();
        let passive = options
            .passive
            .unwrap_or_else(|| self.default_passive(&type_, target));
        let list = self.listeners.map.entry(target).or_default();
        if list
            .iter()
            .any(|l| l.type_ == type_ && l.capture == options.capture && l.callback.same(&callback))
        {
            return;
        }
        list.push(ListenerEntry {
            type_,
            callback,
            capture: options.capture,
            passive,
            once: options.once,
            removed: Arc::new(AtomicBool::new(false)),
        });
    }

    /// `removeEventListener(type, callback, options)`.
    pub fn remove_event_listener(
        &mut self,
        target: EventTarget,
        type_: &DomString,
        callback: &Listener,
        capture: bool,
    ) {
        if let Some(list) = self.listeners.map.get_mut(&target)
            && let Some(i) = list.iter().position(|l| {
                l.type_ == *type_ && l.capture == capture && l.callback.same(callback)
            })
        {
            list[i].removed.store(true, Ordering::Relaxed);
            list.remove(i);
        }
    }

    /// Acciones de activación pendientes.
    pub fn take_activations(&mut self) -> Vec<Activation> {
        std::mem::take(&mut self.listeners.activations)
    }

    /// E1: los cambios de `open` de los `details` pendientes de su evento `toggle`.
    pub fn take_details_toggles(&mut self) -> Vec<(NodeId, bool)> {
        std::mem::take(&mut self.listeners.details_toggles)
    }

    /// E1: "the summary for its parent details" (HTML §4.11.2): el primer hijo `summary` de
    /// un `details`.
    pub fn is_summary_for_parent_details(&self, n: NodeId) -> bool {
        if !self.element(n).is_some_and(|e| e.is_html("summary")) {
            return false;
        }
        let Some(p) = self.parent(n) else {
            return false;
        };
        self.element(p).is_some_and(|e| e.is_html("details"))
            && self
                .children(p)
                .find(|&c| self.element(c).is_some_and(|e| e.is_html("summary")))
                == Some(n)
    }

    /// E1: ¿Es `n` contenido interactivo (HTML §3.2.5.2.7)?
    fn is_interactive_content(&self, n: NodeId) -> bool {
        let Some(e) = self.element(n) else {
            return false;
        };
        let local = e.name.to_string();
        if e.ns != bf_infra::Namespace::Html {
            return false;
        }
        match local.as_str() {
            "a" => e.attr("href").is_some(),
            "audio" | "video" => e.attr("controls").is_some(),
            "img" => e.attr("usemap").is_some(),
            "input" => !e
                .attr("type")
                .is_some_and(|t| t.eq_ignore_ascii_case("hidden")),
            "button" | "details" | "embed" | "iframe" | "label" | "select" | "textarea" => true,
            _ => false,
        }
    }

    /// E1: el *activation behavior* de `label`: nada si el click fue en un descendiente
    /// interactivo (o dentro de uno).
    fn label_activation(&mut self, n: NodeId, event: &Event) {
        if let Some(EventTarget::Node(t)) = event.target {
            let mut cur = Some(t);
            while let Some(c) = cur {
                if c == n {
                    break;
                }
                if self.is_interactive_content(c) {
                    return;
                }
                cur = self.parent(c);
            }
        }
        self.listeners.activations.push(Activation::Label(n));
    }

    /// E1: anota un cambio de presencia del atributo `open` de un `details`.
    pub(crate) fn details_open_changed(&mut self, el: NodeId, was_present: bool) {
        if !self.element(el).is_some_and(|e| e.is_html("details")) {
            return;
        }
        let now = self.element(el).is_some_and(|e| e.attr("open").is_some());
        if now != was_present {
            self.listeners.details_toggles.push((el, now));
        }
    }

    /// "Get the parent" de un target (§2.9).
    fn event_parent(&self, target: EventTarget, event: &Event) -> Option<EventTarget> {
        match target {
            EventTarget::Node(n) => match &self.get(n).data {
                // El padre del documento es su global, salvo para `load`.
                NodeData::Document if n == self.root() => {
                    (event.type_ != "load").then_some(EventTarget::Window)
                }
                _ => self.parent(n).map(EventTarget::Node),
            },
            EventTarget::Window | EventTarget::Object(_) => None,
        }
    }

    /// `dispatchEvent(event)`: el dispatch de un evento creado por script.
    pub fn dispatch_event_checked(
        &mut self,
        target: EventTarget,
        event: &mut Event,
        host: &mut dyn ScriptHost,
    ) -> Result<bool, DomException> {
        if event.dispatch || !event.initialized {
            return Err(DomException::InvalidStateError);
        }
        event.is_trusted = false;
        Ok(self.dispatch(target, event, host))
    }

    /// "Fire an event": un evento de confianza con `init`.
    pub fn fire_event(
        &mut self,
        target: EventTarget,
        type_: &str,
        init: EventInit,
        kind: EventKind,
        host: &mut dyn ScriptHost,
    ) -> bool {
        let mut event = Event::with_kind(type_, init, kind);
        event.is_trusted = true;
        self.dispatch(target, &mut event, host)
    }

    /// Dispatch sin motor de script.
    pub fn dispatch_event(&mut self, target: EventTarget, event: &mut Event) -> bool {
        self.dispatch(target, event, &mut NoScript)
    }

    /// "Dispatch" (§2.9). Devuelve `false` si el evento se canceló.
    pub fn dispatch(
        &mut self,
        target: EventTarget,
        event: &mut Event,
        host: &mut dyn ScriptHost,
    ) -> bool {
        self.dispatch_with_override(target, event, host, None)
    }

    /// Dispara `load` en el `Window` con el *legacy target override flag* (HTML §13.2.7): el
    /// `target` que ven los listeners es el documento.
    pub fn fire_window_load(&mut self, host: &mut dyn ScriptHost) -> bool {
        let mut event = Event::new("load", EventInit::default());
        event.is_trusted = true;
        let root = EventTarget::Node(self.root());
        self.dispatch_with_override(EventTarget::Window, &mut event, host, Some(root))
    }

    /// "Dispatch" con un *target override* opcional (§2.9, pasos 1 a 5.2).
    fn dispatch_with_override(
        &mut self,
        target: EventTarget,
        event: &mut Event,
        host: &mut dyn ScriptHost,
        target_override: Option<EventTarget>,
    ) -> bool {
        // 1.
        event.dispatch = true;
        // 3.
        let mut activation_target: Option<NodeId> = None;
        // 5.2.
        event.path.push(PathEntry {
            invocation_target: target,
            shadow_adjusted_target: Some(target_override.unwrap_or(target)),
        });
        // 5.3-5.4.
        let is_activation_event = event.is_mouse_event() && event.type_ == "click";
        if is_activation_event
            && let EventTarget::Node(n) = target
            && self.has_activation_behavior(n)
        {
            activation_target = Some(n);
        }
        // 5.7-5.8.
        let mut parent = self.event_parent(target, event);
        while let Some(p) = parent {
            if let EventTarget::Window = p {
                event.path.push(PathEntry {
                    invocation_target: p,
                    shadow_adjusted_target: None,
                });
            } else {
                if is_activation_event
                    && event.bubbles
                    && activation_target.is_none()
                    && let EventTarget::Node(n) = p
                    && self.has_activation_behavior(n)
                {
                    activation_target = Some(n);
                }
                event.path.push(PathEntry {
                    invocation_target: p,
                    shadow_adjusted_target: None,
                });
            }
            parent = self.event_parent(p, event);
        }
        // 5.11.
        let legacy_state = activation_target.map(|a| self.legacy_pre_activation(a));
        // 5.12: captura, del final al principio.
        let path = event.path.clone();
        for i in (0..path.len()).rev() {
            event.event_phase = if path[i].shadow_adjusted_target.is_some() {
                EventPhase::AtTarget
            } else {
                EventPhase::Capturing
            };
            self.invoke(i, event, Phase::Capturing, host);
        }
        // 5.13: burbujeo.
        for (i, entry) in path.iter().enumerate() {
            if entry.shadow_adjusted_target.is_some() {
                event.event_phase = EventPhase::AtTarget;
            } else {
                if !event.bubbles {
                    continue;
                }
                event.event_phase = EventPhase::Bubbling;
            }
            self.invoke(i, event, Phase::Bubbling, host);
        }
        // 6-9.
        event.event_phase = EventPhase::None;
        event.current_target = None;
        event.path.clear();
        event.dispatch = false;
        event.stop_propagation = false;
        event.stop_immediate_propagation = false;
        // 11.
        if let Some(a) = activation_target {
            if !event.canceled {
                self.activation_behavior(a, event, host);
            } else if let Some(Some(old)) = legacy_state {
                self.legacy_canceled_activation(a, old);
            }
        }
        !event.canceled
    }

    /// "Invoke" (§2.9) para la entrada `i` del path.
    fn invoke(&mut self, i: usize, event: &mut Event, phase: Phase, host: &mut dyn ScriptHost) {
        // 1. El target es el de la última entrada con shadow-adjusted target hasta `i`.
        event.target = event.path[..=i]
            .iter()
            .rev()
            .find_map(|p| p.shadow_adjusted_target);
        // 4.
        if event.stop_propagation {
            return;
        }
        // 5-6.
        let current = event.path[i].invocation_target;
        event.current_target = Some(current);
        host.before_invoke(self, current, &event.type_); // E3
        let listeners = self
            .listeners
            .map
            .get(&current)
            .cloned()
            .unwrap_or_default();
        // 7.
        let found = self.inner_invoke(event, &listeners, phase, host);
        // 8. Tipos heredados con prefijo `webkit` para eventos de confianza.
        if !found && event.is_trusted {
            let legacy = match event.type_.as_lossy_str() {
                "animationend" => "webkitAnimationEnd",
                "animationiteration" => "webkitAnimationIteration",
                "animationstart" => "webkitAnimationStart",
                "transitionend" => "webkitTransitionEnd",
                _ => return,
            };
            let original = std::mem::replace(&mut event.type_, legacy.into());
            self.inner_invoke(event, &listeners, phase, host);
            event.type_ = original;
        }
    }

    /// "Inner invoke" (§2.9).
    fn inner_invoke(
        &mut self,
        event: &mut Event,
        listeners: &[ListenerEntry],
        phase: Phase,
        host: &mut dyn ScriptHost,
    ) -> bool {
        let mut found = false;
        for l in listeners {
            if l.removed.load(Ordering::Relaxed) {
                continue;
            }
            if event.type_ != l.type_ {
                continue;
            }
            found = true;
            if (phase == Phase::Capturing && !l.capture) || (phase == Phase::Bubbling && l.capture)
            {
                continue;
            }
            if l.once
                && let Some(target) = event.current_target
            {
                self.remove_event_listener(target, &l.type_, &l.callback, l.capture);
            }
            if l.passive {
                event.in_passive_listener = true;
            }
            match &l.callback {
                Listener::Native(f) => f(self, event),
                Listener::Script(h) => {
                    // Una excepción se reporta y el dispatch sigue.
                    let _ = host.call_listener(self, *h, event);
                }
            }
            event.in_passive_listener = false;
            if event.stop_immediate_propagation {
                break;
            }
        }
        found
    }

    // --- Activation behavior (HTML) ---

    /// ¿Tiene `n` activation behavior?
    pub fn has_activation_behavior(&self, n: NodeId) -> bool {
        let Some(e) = self.element(n) else {
            return false;
        };
        // E1: sin `href` no son hipervínculos ni targets de activación (como en los
        // navegadores; el `label` que los contiene sí se activa).
        if e.is_html("a") || e.is_html("area") {
            return e.attr("href").is_some();
        }
        // E1: `label` y el `summary` de un `details`.
        if e.is_html("label") {
            return true;
        }
        if e.is_html("summary") {
            return self.is_summary_for_parent_details(n);
        }
        if e.is_html("input") {
            let t = e.attr("type").unwrap_or("").to_ascii_lowercase();
            return matches!(
                t.as_str(),
                "checkbox" | "radio" | "submit" | "image" | "reset" | "button" | "file" | "color"
            );
        }
        e.is_html("button")
    }

    /// "Actually disabled" (HTML §4.10.18.5): atributo `disabled` o un `fieldset` deshabilitado
    /// ancestro (salvo dentro de su primera `legend`).
    pub fn is_disabled(&self, n: NodeId) -> bool {
        if self
            .element(n)
            .is_some_and(|e| e.attr("disabled").is_some())
        {
            return true;
        }
        let mut child = n;
        let mut cur = self.parent(n);
        while let Some(p) = cur {
            if let Some(e) = self.element(p)
                && e.is_html("fieldset")
                && e.attr("disabled").is_some()
            {
                let first_legend = self
                    .children(p)
                    .find(|&c| self.element(c).is_some_and(|x| x.is_html("legend")));
                if first_legend != Some(child) {
                    return true;
                }
            }
            child = p;
            cur = self.parent(p);
        }
        false
    }

    fn input_type(&self, n: NodeId) -> String {
        self.element(n)
            .and_then(|e| e.attr("type"))
            .unwrap_or("")
            .to_ascii_lowercase()
    }

    /// "Legacy-pre-activation behavior" de checkbox y radio; devuelve el estado anterior
    /// (checkedness y, para radio, el radio del grupo que estaba marcado).
    fn legacy_pre_activation(&mut self, n: NodeId) -> Option<(bool, Option<NodeId>)> {
        if !self.element(n).is_some_and(|e| e.is_html("input")) {
            return None;
        }
        match self.input_type(n).as_str() {
            "checkbox" => {
                let old = self.checkedness(n);
                self.set_checkedness(n, !old);
                Some((old, None))
            }
            "radio" => {
                let old = self.checkedness(n);
                let previous = self
                    .radio_group(n)
                    .into_iter()
                    .find(|&r| self.checkedness(r));
                self.set_checkedness(n, true);
                Some((old, previous))
            }
            _ => None,
        }
    }

    /// "Legacy-canceled-activation behavior": restaura el estado.
    fn legacy_canceled_activation(&mut self, n: NodeId, (old, previous): (bool, Option<NodeId>)) {
        match self.input_type(n).as_str() {
            "checkbox" => self.set_checkedness(n, old),
            "radio" => {
                if let Some(p) = previous {
                    self.set_checkedness(p, true);
                } else {
                    self.set_checkedness(n, old);
                }
            }
            _ => {}
        }
    }

    /// "Activation behavior" de `a`, `area`, `input` y `button`.
    fn activation_behavior(&mut self, n: NodeId, event: &Event, host: &mut dyn ScriptHost) {
        let Some(e) = self.element(n) else {
            return;
        };
        // E1.
        if e.is_html("label") {
            self.label_activation(n, event);
            return;
        }
        if e.is_html("summary") {
            self.listeners.activations.push(Activation::Summary(n));
            return;
        }
        if e.is_html("a") || e.is_html("area") {
            if e.attr("href").is_some() {
                self.listeners
                    .activations
                    .push(Activation::FollowHyperlink(n));
            }
            return;
        }
        if e.is_html("input") {
            let kind = self.input_type(n);
            // "If element is not mutable and is not in the Checkbox state and is not in the
            // Radio state, then return."
            if !matches!(kind.as_str(), "checkbox" | "radio") && self.is_disabled(n) {
                return;
            }
            match kind.as_str() {
                "checkbox" | "radio" => {
                    // Si está conectado: `input` y `change` (HTML §4.10.5.1.15/16).
                    if self.is_connected(n) {
                        let init = EventInit {
                            bubbles: true,
                            cancelable: false,
                            composed: true,
                        };
                        self.fire_event(
                            EventTarget::Node(n),
                            "input",
                            init,
                            EventKind::Event,
                            host,
                        );
                        let init = EventInit {
                            bubbles: true,
                            ..EventInit::default()
                        };
                        self.fire_event(
                            EventTarget::Node(n),
                            "change",
                            init,
                            EventKind::Event,
                            host,
                        );
                    }
                }
                "submit" | "image" => self.listeners.activations.push(Activation::Submit(n)),
                "reset" => self.listeners.activations.push(Activation::Reset(n)),
                _ => {}
            }
            return;
        }
        if e.is_html("button") {
            if self.is_disabled(n) {
                return;
            }
            let e = self.element(n).expect("elemento");
            match e.attr("type").map(str::to_ascii_lowercase).as_deref() {
                Some("reset") => self.listeners.activations.push(Activation::Reset(n)),
                Some("button") => {}
                _ => self.listeners.activations.push(Activation::Submit(n)),
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Capturing,
    Bubbling,
}

// E3: mover los listeners de un nodo a otro documento (adopción entre arenas en bf-web).
impl Document {
    /// Mueve los listeners de `from` (en este documento) a `to` en `other`.
    pub fn move_listeners(&mut self, from: EventTarget, other: &mut Document, to: EventTarget) {
        if let Some(list) = self.listeners.map.remove(&from) {
            other.listeners.map.entry(to).or_default().extend(list);
        }
    }
}

// E3: el estado de un evento visto después de su dispatch (para los wrappers de bf-web).
impl Event {
    /// Una copia con lo que deja el final del dispatch (§2.9 pasos 9 y 12): sin las banderas
    /// de dispatch y de propagación, fase `NONE`, sin `currentTarget` y sin path.
    pub fn after_dispatch_copy(&self) -> Event {
        let mut e = self.clone();
        e.dispatch = false;
        e.stop_propagation = false;
        e.stop_immediate_propagation = false;
        e.in_passive_listener = false;
        e.event_phase = EventPhase::None;
        e.current_target = None;
        e.path.clear();
        e
    }
}
