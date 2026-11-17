//! E2: lo que pinta el navegador mientras hay animaciones CSS (`bom::animation::animate_in`,
//! el gancho del pintado, y `update_and_send_events`, el cuadro).
//!
//! El caso de `https://asimetrix.co/`: un titular con `animation: .7s both fade-up` (de
//! `opacity: 0` a 1) se quedaba pintado en el primer fotograma porque (1) lo que se pintaba
//! entre dos cuadros usaba la hora congelada del último cuadro y (2) si el primer cuadro tras el
//! arranque llegaba con la animación ya terminada, no se volvía a pintar.

use std::sync::Arc;
use std::time::Duration;

use bf_css::media::MediaContext;
use bf_css::selectors::MatchContext;
use bf_js::host::DefaultHooks;
use bf_web::bom::animation::{animate_in, is_animating, update_and_send_events};
use bf_web::{DocumentOwner, Runtime, Url, WebContext, WebContextHandle};

const PAGE: &str = "<!doctype html><style>\
    @keyframes fade-up { from { opacity: 0 } to { opacity: 1 } }\
    h1 { animation: 100ms linear both fade-up }\
    </style><h1>Titular</h1>";

struct Page {
    rt: Runtime,
    ctx: WebContextHandle,
}

impl Page {
    fn new() -> Page {
        let mut rt = Runtime::new(Box::new(DefaultHooks));
        let (doc, _) = bf_html::parse_document(PAGE, &bf_html::ParseOptions::default());
        let realm = rt.main_realm();
        let url = Url::parse("https://example.com/", None).expect("url");
        let ctx = bf_web::install(
            &mut rt.agent,
            realm,
            WebContext::new(DocumentOwner::Document(Box::new(doc)), url),
        );
        Page { rt, ctx }
    }

    /// Los estilos de la cascada del documento (lo que el navegador le pasa a `animate_in`).
    fn base(&self) -> Arc<bf_style::StyleMap> {
        let doc = self.ctx.get(&self.rt.agent).document();
        Arc::new(bf_style::style_document(
            doc,
            &[],
            &MediaContext::default(),
            &MatchContext::for_document(doc),
        ))
    }

    /// La opacidad del `<h1>` en `styles`.
    fn opacity(&self, styles: &bf_style::StyleMap) -> f32 {
        let doc = self.ctx.get(&self.rt.agent).document();
        let h1 = doc
            .inclusive_descendants(doc.root())
            .into_iter()
            .find(|&n| doc.element(n).is_some_and(|e| e.is_html("h1")))
            .expect("h1");
        styles[&(h1, None)].opacity.0
    }

    /// Lo que pinta el navegador ahora.
    fn paint(&mut self) -> f32 {
        let base = self.base();
        let shown = animate_in(&mut self.rt.agent, self.ctx.realm, base);
        self.opacity(&shown)
    }
}

/// HTML §8.1.7.3: pintar es actualizar la representación, que corre "update animations"
/// (Web Animations 1 §4.6) antes: lo pintado sin un cuadro de por medio usa la hora de ahora.
#[test]
fn painting_between_frames_uses_the_current_time() {
    let mut p = Page::new();
    // El primer pintado arranca la animación: primer fotograma.
    assert_eq!(p.paint(), 0.0);
    assert!(is_animating(&p.rt.agent, p.ctx.realm));
    // Pasa más que la duración sin ningún cuadro (`update_and_send_events`): el siguiente
    // pintado ya muestra el estado final (`animation-fill-mode: both`).
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(p.paint(), 1.0);
}

/// Si el cuadro que ve terminar la animación es el primero desde que arrancó, el navegador
/// tiene que pintar una vez más (`is_animating` sigue en `true` hasta que pinta).
#[test]
fn an_animation_that_ends_between_frames_is_repainted() {
    let mut p = Page::new();
    assert_eq!(p.paint(), 0.0);
    std::thread::sleep(Duration::from_millis(150));
    let realm = p.ctx.realm;
    update_and_send_events(&mut p.rt.agent, realm);
    assert!(
        is_animating(&p.rt.agent, realm),
        "terminó en este cuadro y todavía no se pintó"
    );
    assert_eq!(p.paint(), 1.0);
    assert!(!is_animating(&p.rt.agent, realm));
}
