//! E4: animaciones y transiciones CSS de la ventana (CSS Animations 1, CSS Transitions 1).
//!
//! El [`Animator`] de `bf-style` vive como dato del realm ([`AnimationState`]). Cada cálculo de
//! los estilos del documento ([`crate::cssom::current_styles`], que usan `getComputedStyle` y la
//! geometría) es un "style change event" (CSS Transitions 1 §3): [`animate`] le pasa al
//! `Animator` los estilos de la cascada y la hora de la línea de tiempo del documento
//! ([`document_time`], Web Animations 1 §4.5 "document timeline") y devuelve los estilos
//! con los valores animados. "Update the rendering" (HTML §8.1.7.3, paso "update animations
//! and send events") es [`update_and_send_events`]: recalcula los estilos y despacha los
//! `AnimationEvent`/`TransitionEvent` encolados (CSS Animations 1 §4.2, CSS Transitions 1 §6),
//! en orden de su hora.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use bf_css::stylesheet::Stylesheet;
use bf_dom::events::{Event, EventInit, EventTarget};
use bf_js::string::JsString;
use bf_js::value::Value;
use bf_js::vm::{Agent, RealmId, Throw};
use bf_style::StyleMap;
use bf_style::animation::{AnimationEvent, Animator, UpdateInput, keyframes_rules};

use crate::WebContextHandle;
use crate::dom::events::EventIface;

/// Las animaciones del documento de una ventana.
#[derive(Default)]
pub struct AnimationState {
    pub animator: Animator,
    /// La última actualización: los estilos de la cascada, la hora y los mostrados.
    pub(crate) last: Option<(Arc<StyleMap>, f64, Arc<StyleMap>)>,
    /// `(Document::version(), generación de las hojas)` del último "update the rendering".
    seen: Option<(u64, u64)>,
    /// La hora de la línea de tiempo del documento (ms desde el `timeOrigin`), fija durante un
    /// cuadro ([`document_time`]).
    pub(crate) time: Option<f64>,
    /// `document.timeline` (ver [`super::web_animations`]).
    pub(crate) timeline: Option<bf_js::host::HostHandle>,
    /// Las promesas `finished` pendientes (fijadas) de cada animación.
    pub(crate) finished_waiting: Vec<(u64, bf_js::host::HostHandle)>,
}

/// El estado de animaciones de la ventana en curso (se crea la primera vez).
pub(crate) fn state_mut(agent: &mut Agent) -> &mut AnimationState {
    let realm = WebContextHandle::current(agent).realm;
    if agent.realm_data::<AnimationState>(realm).is_none() {
        agent.set_realm_data(realm, AnimationState::default());
    }
    agent
        .realm_data_mut::<AnimationState>(realm)
        .expect("AnimationState recién creado")
}

/// La hora actual de la línea de tiempo del documento de la ventana en curso (ms).
///
/// Web Animations 1 §4.5: la hora de la línea de tiempo solo avanza en "update animations and
/// send events" (cada cuadro, [`update_and_send_events`]), así que durante un cuadro todas las
/// consultas ven la misma hora y los estilos animados (y la maqueta que depende de ellos) se
/// calculan una sola vez. Sin nada animándose el navegador no corre cuadros; entonces la hora
/// sigue al reloj (`performance.now()`), para que una transición que arranca en reposo empiece
/// en la hora del cambio de estilo y no en la del último cuadro.
pub fn document_time(agent: &mut Agent) -> f64 {
    let now = super::performance::now_ms(agent);
    let st = state_mut(agent);
    match st.time {
        Some(t) if st.animator.is_active() => t,
        _ => {
            st.time = Some(now);
            now
        }
    }
}

/// El "style change event": los estilos de la cascada `base` con las animaciones y
/// transiciones de la ventana en curso aplicadas en la hora actual.
pub fn animate(agent: &mut Agent, base: Arc<StyleMap>) -> Arc<StyleMap> {
    let realm = WebContextHandle::current(agent).realm;
    let now = document_time(agent);
    let st = state_mut(agent);
    // Sin nada en curso (solo efectos retenidos: en pausa o con relleno), el resultado no
    // depende de la hora.
    if let Some((b, t, shown)) = &st.last
        && Arc::ptr_eq(b, &base)
        && (*t == now || !st.animator.is_active())
    {
        return shown.clone();
    }
    if !st.animator.needed(&base) {
        st.animator.note_styles(&base);
        st.last = Some((base.clone(), now, base.clone()));
        return base;
    }
    let mut animator = std::mem::take(&mut st.animator);
    let media = crate::cssom::media_context(agent);
    let sheets = crate::cssom::sheets::document_sheets(agent, &media);
    let refs: Vec<&Stylesheet> = sheets.iter().collect();
    let keyframes = keyframes_rules(&refs, &media);
    let mut styles = (*base).clone();
    {
        let doc = WebContextHandle { realm }.get(agent).document();
        animator.update(
            &mut styles,
            &UpdateInput {
                doc,
                keyframes: &keyframes,
                now,
            },
        );
    }
    let shown = Arc::new(styles);
    if let Some(st) = agent.realm_data_mut::<AnimationState>(realm) {
        st.animator = animator;
        st.last = Some((base, now, shown.clone()));
    }
    shown
}

/// ¿La ventana `realm` necesita un cuadro ("update the rendering") aunque no haya callbacks
/// de `requestAnimationFrame`? Sí si hay animaciones o transiciones en curso o eventos
/// pendientes y, con `after_mutation`, si el documento cambió desde el último cuadro (puede
/// arrancar una transición; el navegador no lo necesita porque re-maqueta tras cada cambio).
pub fn wants_frame(agent: &Agent, realm: RealmId, after_mutation: bool) -> bool {
    let Some(ctx) = agent.realm_data::<crate::WebContext>(realm) else {
        return false;
    };
    let version = (
        ctx.document().version(),
        crate::cssom::sheets_generation(agent, realm),
    );
    match agent.realm_data::<AnimationState>(realm) {
        Some(st) => {
            st.animator.is_active()
                || st.animator.has_events()
                || (after_mutation && st.seen != Some(version))
        }
        None => after_mutation,
    }
}

/// ¿Hay animaciones o transiciones en curso en la ventana `realm`? (Mientras las haya, cada
/// cuadro muestra estilos nuevos.)
pub fn is_animating(agent: &Agent, realm: RealmId) -> bool {
    agent
        .realm_data::<AnimationState>(realm)
        .is_some_and(|st| st.animator.is_active())
}

/// Los estilos de la cascada `base` del documento de la ventana `realm` con sus animaciones y
/// transiciones en la hora de la línea de tiempo (para pintar: el navegador maqueta con estos).
pub fn animate_in(agent: &mut Agent, realm: RealmId, base: Arc<StyleMap>) -> Arc<StyleMap> {
    if agent.realm_data::<crate::WebContext>(realm).is_none() {
        return base;
    }
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let shown = animate(agent, base);
    agent.current_realm = saved;
    shown
}

/// "Update animations and send events" (Web Animations 1 §4.6; HTML §8.1.7.3) en la ventana
/// `realm`: el "style change event" del cuadro y los eventos de animación pendientes.
pub fn update_and_send_events(agent: &mut Agent, realm: RealmId) {
    let saved = agent.current_realm;
    agent.current_realm = realm;
    if agent.realm_data::<crate::WebContext>(realm).is_some() {
        // La hora del cuadro (Web Animations 1 §4.5).
        let now = super::performance::now_ms(agent);
        state_mut(agent).time = Some(now);
        let _ = crate::cssom::current_styles(agent);
        // Las promesas `finished` de las que terminaron (Web Animations 1 §4.4.18).
        super::web_animations::resolve_finished(agent, realm, now);
        let version = (
            WebContextHandle { realm }.get(agent).document().version(),
            crate::cssom::sheets_generation(agent, realm),
        );
        let events = match agent.realm_data_mut::<AnimationState>(realm) {
            Some(st) => {
                st.seen = Some(version);
                st.animator.take_events()
            }
            None => Vec::new(),
        };
        for e in events {
            dispatch(agent, realm, &e);
        }
    }
    agent.current_realm = saved;
}

/// Despacha un `AnimationEvent` o `TransitionEvent` de confianza (burbujea, no cancelable).
fn dispatch(agent: &mut Agent, realm: RealmId, e: &AnimationEvent) {
    let connected = WebContextHandle { realm }
        .get(agent)
        .document()
        .is_connected(e.target.0);
    if !connected {
        return;
    }
    let animation = e.kind.starts_with("animation");
    let classes = crate::dom::classes(agent);
    let class = if animation {
        classes.animation_event
    } else {
        classes.transition_event
    };
    let mut event = Event::new(
        e.kind,
        EventInit {
            bubbles: true,
            cancelable: false,
            composed: false,
        },
    );
    event.is_trusted = true;
    let o = crate::dom::events::create_event_object(
        agent,
        EventIface::Class(class),
        Rc::new(RefCell::new(event)),
        0,
    );
    let pseudo = e
        .target
        .1
        .map_or(String::new(), |p| format!("::{}", p.name()));
    let name = Value::from(JsString::from(e.name.as_str()));
    let elapsed = Value::Number(e.elapsed);
    let pseudo = Value::from(JsString::from(pseudo.as_str()));
    // Los slots de `plain_ctor`: los dos de `Event` y los miembros del diccionario en orden.
    let members = if animation {
        [name, elapsed, pseudo]
    } else {
        [elapsed, name, pseudo]
    };
    if let Some(s) = agent.host_slots_mut(o) {
        *s = vec![Value::Null, Value::Undefined];
        s.extend(members);
    }
    let pin = agent.host_pin(Value::Object(o));
    let target = EventTarget::Node(e.target.0);
    let _ = crate::dom::events::dispatch_js_event(agent, 0, target, o, false);
    agent.host_unpin(pin);
    if agent.frames.is_empty()
        && let Err(Throw::Value(err)) = agent.perform_microtask_checkpoint()
    {
        agent.report_exception(&err);
    }
}
