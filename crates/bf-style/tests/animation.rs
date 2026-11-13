//! E4: el `Animator` de punta a punta sobre documentos reales: `@keyframes` (CSS Animations
//! 1 §4), transiciones (CSS Transitions 1 §3), el modelo de tiempo (Web Animations 1 §4) y la
//! cola de eventos.

use bf_css::media::MediaContext;
use bf_css::selectors::{MatchContext, NamespaceMap, matches, parse_selector_list};
use bf_css::stylesheet::Stylesheet;
use bf_dom::{Document, DomString, NodeId};
use bf_html::{ParseOptions, parse_document};
use bf_style::StyleMap;
use bf_style::animation::{Animator, UpdateInput, keyframes_rules};
use bf_style::loader::document_stylesheets;

fn find(doc: &Document, selector: &str) -> NodeId {
    let list = parse_selector_list(selector, &NamespaceMap::default()).expect("selector válido");
    let ctx = MatchContext::for_document(doc);
    doc.inclusive_descendants(doc.root())
        .into_iter()
        .find(|&n| list.0.iter().any(|s| matches(s, doc, n, &ctx)))
        .unwrap_or_else(|| panic!("ningún elemento coincide con {selector}"))
}

/// Un documento con su `Animator`: cada [`Page::at`] es un "style change event" con la
/// cascada recién calculada, como `cssom::current_styles` en bf-web.
struct Page {
    doc: Document,
    animator: Animator,
}

impl Page {
    fn new(html: &str) -> Page {
        let (doc, _) = parse_document(html, &ParseOptions::default());
        Page {
            doc,
            animator: Animator::new(),
        }
    }

    fn at(&mut self, now: f64) -> StyleMap {
        let media = MediaContext::default();
        let sheets: Vec<Stylesheet> = document_stylesheets(&self.doc, &media, None);
        let refs: Vec<&Stylesheet> = sheets.iter().collect();
        let keyframes = keyframes_rules(&refs, &media);
        let ctx = MatchContext::for_document(&self.doc);
        let mut styles = bf_style::style_document(&self.doc, &[], &media, &ctx);
        self.animator.update(
            &mut styles,
            &UpdateInput {
                doc: &self.doc,
                keyframes: &keyframes,
                now,
            },
        );
        styles
    }

    fn value(&mut self, now: f64, selector: &str, property: &str) -> String {
        let styles = self.at(now);
        let node = find(&self.doc, selector);
        styles[&(node, None)]
            .get_css(property)
            .unwrap_or_else(|| panic!("propiedad desconocida {property}"))
    }

    fn set(&mut self, selector: &str, name: &str, value: &str) {
        let node = find(&self.doc, selector);
        self.doc
            .set_attribute(node, &DomString::from(name), DomString::from(value))
            .expect("atributo válido");
    }

    fn events(&mut self) -> Vec<(&'static str, String, f64)> {
        self.animator
            .take_events()
            .into_iter()
            .map(|e| (e.kind, e.name, e.elapsed))
            .collect()
    }
}

#[test]
fn keyframes_animate_the_computed_value() {
    let mut p = Page::new(
        "<!doctype html><style>@keyframes fade{from{opacity:0}to{opacity:1}}\
         #a{animation:fade 1s linear}</style><div id=a></div>",
    );
    assert_eq!(p.value(0.0, "#a", "opacity"), "0");
    assert!(p.animator.is_active());
    assert_eq!(p.value(250.0, "#a", "opacity"), "0.25");
    assert_eq!(p.value(500.0, "#a", "opacity"), "0.5");
    // Sin `fill`, al terminar vuelve al valor de la cascada.
    assert_eq!(p.value(1500.0, "#a", "opacity"), "1");
    assert!(!p.animator.is_active());
}

#[test]
fn keyframes_with_offsets_delay_and_fill() {
    let mut p = Page::new(
        "<!doctype html><style>@keyframes w{0%{width:0}50%{width:100px}100%{width:200px}}\
         #a{animation:w 2s linear 1s both;width:7px}</style><div id=a></div>",
    );
    // `fill: backwards` durante el retardo: el primer keyframe.
    assert_eq!(p.value(0.0, "#a", "width"), "0px");
    assert_eq!(p.value(1500.0, "#a", "width"), "50px");
    assert_eq!(p.value(2500.0, "#a", "width"), "150px");
    // `fill: forwards`: el último keyframe se queda.
    assert_eq!(p.value(5000.0, "#a", "width"), "200px");
}

#[test]
fn iterations_and_alternate_direction() {
    let mut p = Page::new(
        "<!doctype html><style>@keyframes m{from{margin-left:0}to{margin-left:100px}}\
         #a{animation:m 1s linear 2 alternate}</style><div id=a></div>",
    );
    assert_eq!(p.value(0.0, "#a", "margin-left"), "0px");
    assert_eq!(p.value(250.0, "#a", "margin-left"), "25px");
    // Segunda iteración, hacia atrás.
    assert_eq!(p.value(1250.0, "#a", "margin-left"), "75px");
}

#[test]
fn animation_events_are_queued_in_order() {
    let mut p = Page::new(
        "<!doctype html><style>@keyframes k{to{opacity:0}}\
         #a{animation:k 1s 2}</style><div id=a></div>",
    );
    p.at(0.0);
    assert_eq!(p.events(), vec![("animationstart", "k".to_string(), 0.0)]);
    p.at(1100.0);
    assert_eq!(
        p.events(),
        vec![("animationiteration", "k".to_string(), 1.0)]
    );
    p.at(2500.0);
    assert_eq!(p.events(), vec![("animationend", "k".to_string(), 2.0)]);
    assert!(!p.animator.has_events());
}

#[test]
fn removing_the_animation_cancels_it() {
    let mut p = Page::new(
        "<!doctype html><style>@keyframes k{to{opacity:0}}\
         .on{animation:k 1s}</style><div id=a class=on></div>",
    );
    p.at(0.0);
    p.events();
    p.set("#a", "class", "");
    assert_eq!(p.value(100.0, "#a", "opacity"), "1");
    // `elapsedTime` del cancel: el tiempo activo al cancelar (CSS Animations 2 §4.2).
    assert_eq!(p.events(), vec![("animationcancel", "k".to_string(), 0.1)]);
}

#[test]
fn transition_interpolates_from_the_before_change_style() {
    let mut p = Page::new(
        "<!doctype html><style>#a{opacity:0;transition:opacity 1s linear}\
         #a.on{opacity:1}</style><div id=a></div>",
    );
    assert_eq!(p.value(0.0, "#a", "opacity"), "0");
    p.set("#a", "class", "on");
    assert_eq!(p.value(100.0, "#a", "opacity"), "0");
    let ev = p.events();
    assert_eq!(
        ev.iter().map(|e| e.0).collect::<Vec<_>>(),
        ["transitionrun", "transitionstart"]
    );
    assert_eq!(ev[0].1, "opacity");
    assert_eq!(p.value(600.0, "#a", "opacity"), "0.5");
    assert_eq!(p.value(1200.0, "#a", "opacity"), "1");
    assert_eq!(
        p.events(),
        vec![("transitionend", "opacity".to_string(), 1.0)]
    );
    assert!(!p.animator.is_active());
}

#[test]
fn transition_delay_and_colors() {
    let mut p = Page::new(
        "<!doctype html><style>#a{background-color:rgb(0,0,0);\
         transition:background-color 1s linear 500ms}\
         #a.on{background-color:rgb(200,100,0)}</style><div id=a></div>",
    );
    p.at(0.0);
    p.set("#a", "class", "on");
    p.at(0.0);
    assert_eq!(
        p.events().iter().map(|e| e.0).collect::<Vec<_>>(),
        ["transitionrun"]
    );
    // Durante el retardo se muestra el valor de partida.
    assert_eq!(p.value(250.0, "#a", "background-color"), "rgb(0, 0, 0)");
    assert_eq!(p.value(1000.0, "#a", "background-color"), "rgb(100, 50, 0)");
    assert_eq!(p.events()[0].0, "transitionstart");
}

#[test]
fn transitions_of_transform_interpolate_per_function() {
    let mut p = Page::new(
        "<!doctype html><style>#a{transform:translateX(0px);transition:transform 1s linear}\
         #a.on{transform:translateX(100px)}</style><div id=a></div>",
    );
    p.at(0.0);
    p.set("#a", "class", "on");
    p.at(0.0);
    assert_eq!(p.value(500.0, "#a", "transform"), "translateX(50px)");
}

#[test]
fn no_transition_without_duration_or_for_discrete_values() {
    let mut p = Page::new(
        "<!doctype html><style>#a{opacity:0;display:block}#a.on{opacity:1;display:flex}\
         #a{transition:display 1s}</style><div id=a></div>",
    );
    p.at(0.0);
    p.set("#a", "class", "on");
    assert_eq!(p.value(100.0, "#a", "opacity"), "1");
    // `display` es discreta: sin `transition-behavior: allow-discrete` no transiciona.
    assert_eq!(p.value(100.0, "#a", "display"), "flex");
    assert!(p.events().is_empty());
    assert!(p.animator.is_empty());
}

#[test]
fn paused_animation_holds_its_time() {
    let mut p = Page::new(
        "<!doctype html><style>@keyframes w{from{width:0}to{width:100px}}\
         #a{animation:w 1s linear}#a.p{animation-play-state:paused}</style><div id=a></div>",
    );
    assert_eq!(p.value(0.0, "#a", "width"), "0px");
    assert_eq!(p.value(300.0, "#a", "width"), "30px");
    p.set("#a", "class", "p");
    assert_eq!(p.value(300.0, "#a", "width"), "30px");
    assert_eq!(p.value(900.0, "#a", "width"), "30px");
}

/// E4: `text-shadow` y `box-shadow` se interpolan sombra a sombra (CSS Backgrounds 3 §7.2):
/// color y longitudes; `none` cuenta como una lista de sombras transparentes de longitud 0.
#[test]
fn shadows_interpolate_per_shadow() {
    let mut p = Page::new(
        "<style>#a { color: black; text-shadow: none; box-shadow: 0px 0px 0px 0px red;\
         transition: text-shadow 1s linear, box-shadow 1s linear; }\
         #a.on { text-shadow: rgb(0, 0, 200) 10px 20px 4px;\
         box-shadow: 10px 10px 10px 4px blue, inset 2px 2px green; }</style><p id=a>x</p>",
    );
    p.at(0.0);
    p.set("#a", "class", "on");
    p.at(0.0);
    assert_eq!(
        p.value(500.0, "#a", "text-shadow"),
        "rgba(0, 0, 200, 0.5) 5px 10px 2px"
    );
    // La lista más corta (una sombra) se completa con una transparente de longitud 0 y el
    // `inset` de la otra.
    assert_eq!(
        p.value(500.0, "#a", "box-shadow"),
        "rgb(128, 0, 128) 5px 5px 5px 2px, inset rgba(0, 128, 0, 0.5) 1px 1px 0px 0px"
    );
}

/// E4: lo que necesita la API de Web Animations: la lista en orden de composición (primero
/// las transiciones), mover una animación desde script (`set_times`) y `cancel()` con su
/// evento.
#[test]
fn animator_api_for_web_animations() {
    let mut p = Page::new(
        "<style>@keyframes k { from { width: 0px } to { width: 100px } }\
         #a { width: 10px; transition: width 1s linear; } #a.on { width: 20px; }\
         #b { width: 30px; animation: k 1s linear; }</style><div id=a></div><div id=b></div>",
    );
    p.at(0.0);
    p.set("#a", "class", "on");
    p.at(0.0);
    p.events();
    let list = p.animator.relevant(&p.doc, 0.0);
    assert_eq!(list.len(), 2);
    assert!(list[0].transition && !list[1].transition);
    assert_eq!(list[0].target.0, find(&p.doc, "#a"));
    let k = list[1].id;
    let mut t = p.animator.times(k).expect("la animación existe");
    assert_eq!(t.name, "k");
    assert_eq!(t.play_state(250.0), "running");
    // Saltar a la mitad.
    t.start_time = Some(250.0 - 500.0);
    p.animator.set_times(k, &t);
    assert_eq!(p.value(250.0, "#b", "width"), "50px");
    // Cancelar: sin efecto, `idle`, `animationcancel` y fuera de la lista.
    p.animator.cancel(k, 250.0);
    assert_eq!(p.value(250.0, "#b", "width"), "30px");
    assert_eq!(p.animator.times(k).unwrap().play_state(250.0), "idle");
    assert_eq!(p.events(), [("animationcancel", "k".to_owned(), 0.5)]);
    assert_eq!(p.animator.relevant(&p.doc, 250.0).len(), 1);
    // La transición termina: sigue respondiendo como "finished".
    let tr = list[0].id;
    p.at(2000.0);
    assert!(p.animator.find(tr).is_none());
    assert_eq!(p.animator.times(tr).unwrap().play_state(2000.0), "finished");
    assert_eq!(p.animator.target_of(tr), Some((find(&p.doc, "#a"), None)));
}
