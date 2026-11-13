//! E2: `IntersectionObserver` (W3C Intersection Observer) y `ResizeObserver` (W3C Resize
//! Observer) sin navegador: la maqueta es la del CSSOM (viewport de 800×600, fuente de prueba)
//! y cada [`Page::frame`] es el paso de *update the rendering* (HTML §8.1.7.3) que los calcula,
//! seguido del bucle de eventos (las entradas de `IntersectionObserver` llegan en una tarea).

use std::sync::Arc;

use bf_js::host::DefaultHooks;
use bf_web::{DocumentOwner, Runtime, Url, WebContext, WebContextHandle, bom};

struct Page {
    rt: Runtime,
    ctx: WebContextHandle,
}

impl Page {
    fn load(html: &str) -> Page {
        let mut rt = Runtime::new(Box::new(DefaultHooks));
        let realm = rt.main_realm();
        let url = Url::parse("https://example.test/", None).unwrap();
        let ctx = bf_web::install(
            &mut rt.agent,
            realm,
            WebContext::new(DocumentOwner::Document(Box::default()), url.clone()),
        );
        bom::script::install_scripting(
            &mut rt.agent,
            realm,
            &url,
            Arc::new(bf_infra::FileLoader::new()),
        );
        bom::script::begin_parse(&mut rt.agent, realm, &url);
        bom::script::feed(&mut rt.agent, realm, html);
        bom::script::finish(&mut rt.agent, realm);
        bom::script::resources_loaded(&mut rt.agent, realm);
        rt.run_until_idle().expect("tareas");
        Page { rt, ctx }
    }

    fn show(&mut self, src: &str) -> String {
        let realm = self.ctx.realm;
        match self.rt.eval_script(realm, src, "test.js") {
            Ok(v) => self.rt.to_display(&v),
            Err(e) => format!("Error: {}", e.message),
        }
    }

    /// ¿Pide la ventana un cuadro para sus observadores?
    fn wants_frame(&self) -> bool {
        bf_web::bom::animation::wants_frame(&self.rt.agent, self.ctx.realm, false)
    }

    /// Un cuadro: los observadores con la maqueta actual y después las tareas.
    fn frame(&mut self) {
        let realm = self.ctx.realm;
        bf_web::cssom::update_observations(&mut self.rt.agent, realm);
        self.rt.run_until_idle().expect("tareas");
    }
}

/// Lo que registra el callback de un `IntersectionObserver`.
const IO_LOG: &str = "var log = []; function record(entries) { for (const e of entries) \
    log.push([e.target.id, e.isIntersecting, Math.round(e.intersectionRatio * 100) / 100].join(' ')); }";

#[test]
fn interfaces_exist() {
    let mut p = Page::load("<p>hola</p>");
    assert_eq!(
        p.show(
            "[typeof IntersectionObserver, typeof IntersectionObserverEntry, typeof \
             ResizeObserver, typeof ResizeObserverEntry, typeof ResizeObserverSize].join()"
        ),
        "function,function,function,function,function"
    );
}

/// La primera notificación llega aunque nada cambie, con el viewport como `rootBounds`; la
/// siguiente actualización sin cambios no notifica.
#[test]
fn first_notification() {
    let mut p = Page::load(&format!(
        "<body style='margin:0'><div id=a style='width:100px;height:50px'></div>\
         <div id=b style='display:none'></div><script>{IO_LOG}\
         var e0; var io = new IntersectionObserver((es) => {{ e0 = e0 || es[0]; record(es); }});\
         io.observe(a); io.observe(b);</script>"
    ));
    assert!(p.wants_frame());
    p.frame();
    assert_eq!(p.show("log.join('|')"), "a true 1|b false 0");
    assert_eq!(
        p.show(
            "[e0.rootBounds.width, e0.rootBounds.height, e0.boundingClientRect.width, \
             e0.intersectionRect.height, e0.target === a, e0.time > 0].join()"
        ),
        "800,600,100,50,true,true"
    );
    assert!(!p.wants_frame());
    p.frame();
    assert_eq!(p.show("log.length"), "2");
}

/// Los umbrales: una entrada cada vez que la proporción cruza uno.
#[test]
fn thresholds() {
    let mut p = Page::load(&format!(
        "<body style='margin:0'><div style='height:300px'></div>\
         <div id=t style='height:600px'></div><div style='height:2000px'></div><script>{IO_LOG}\
         var io = new IntersectionObserver(record, {{ threshold: [0.75, 0.25, 0.5] }});\
         io.observe(t);</script>"
    ));
    assert_eq!(p.show("io.thresholds.join()"), "0.25,0.5,0.75");
    assert_eq!(p.show("Object.isFrozen(io.thresholds)"), "true");
    p.frame();
    assert_eq!(p.show("log.join('|')"), "t true 0.5");
    // De 0,5 a 0,67 el primer umbral mayor sigue siendo 0,75 (§3.2.8 paso 2.10): nada.
    p.show("scrollTo(0, 100)");
    p.frame();
    assert_eq!(p.show("log.join('|')"), "t true 0.5");
    // Entero: pasa el último umbral.
    p.show("scrollTo(0, 300)");
    p.frame();
    assert_eq!(p.show("log.join('|')"), "t true 0.5|t true 1");
    // De vuelta a 0,2: por debajo del primer umbral la entrada no interseca (como en los
    // navegadores; WPT `initial-observation-with-threshold`).
    p.show("scrollTo(0, 780)");
    p.frame();
    assert_eq!(p.show("log[2]"), "t false 0.2");
    // Fuera del todo: el índice (0) y `isIntersecting` no cambian, no hay entrada.
    p.show("scrollTo(0, 1000)");
    p.frame();
    assert_eq!(p.show("log.length"), "3");
}

/// `rootMargin` agranda el rectángulo de la raíz; los márgenes inválidos son `SyntaxError` y
/// los umbrales fuera de [0, 1], `RangeError`.
#[test]
fn root_margin_and_errors() {
    let mut p = Page::load(&format!(
        "<body style='margin:0'><div style='height:650px'></div>\
         <div id=t style='height:50px'></div><script>{IO_LOG}\
         new IntersectionObserver(record).observe(t);\
         var m = new IntersectionObserver(record, {{ rootMargin: '100px 0px' }}); m.observe(t);</script>"
    ));
    p.frame();
    assert_eq!(p.show("log.join('|')"), "t false 0|t true 1");
    assert_eq!(p.show("m.rootMargin"), "100px 0px 100px 0px");
    assert_eq!(
        p.show(
            "try { new IntersectionObserver(record, { rootMargin: '1em' }) } catch (e) { e.name }"
        ),
        "SyntaxError"
    );
    assert_eq!(
        p.show("try { new IntersectionObserver(record, { threshold: 2 }) } catch (e) { e.name }"),
        "RangeError"
    );
    assert_eq!(
        p.show("try { new IntersectionObserver(1) } catch (e) { e.name }"),
        "TypeError"
    );
}

/// Una raíz con `overflow` recorta a sus descendientes: el objetivo debajo de su borde no
/// interseca hasta que la raíz se desplaza.
#[test]
fn root_with_overflow() {
    let mut p = Page::load(&format!(
        "<body style='margin:0'><div id=r style='width:100px;height:100px;overflow:hidden'>\
         <div style='height:150px'></div><div id=t style='height:50px'></div></div>\
         <script>{IO_LOG} var io = new IntersectionObserver(record, {{ root: r }});\
         io.observe(t); var v = new IntersectionObserver(record); v.observe(t);</script>"
    ));
    p.frame();
    // Con la raíz `r`, fuera; con el viewport, también (lo recorta `r`).
    assert_eq!(p.show("log.join('|')"), "t false 0|t false 0");
    assert_eq!(p.show("io.root === r"), "true");
    p.show("r.scrollTop = 100");
    p.frame();
    assert_eq!(
        p.show("log.join('|')"),
        "t false 0|t false 0|t true 1|t true 1"
    );
}

/// `disconnect()` y `unobserve()`: sin más entradas; `takeRecords()` vacía la cola.
#[test]
fn disconnect_and_take_records() {
    let mut p = Page::load(&format!(
        "<body style='margin:0'><div id=t style='height:50px'></div><script>{IO_LOG}\
         var io = new IntersectionObserver(record); io.observe(t);</script>"
    ));
    p.frame();
    assert_eq!(p.show("log.length"), "1");
    p.show("io.disconnect(); scrollTo(0, 1000)");
    p.frame();
    assert_eq!(p.show("log.length"), "1");
    // `takeRecords()` antes de la entrega: la tarea ya no tiene nada que entregar.
    p.show("io.observe(t)");
    bf_web::cssom::update_observations(&mut p.rt.agent, p.ctx.realm);
    assert_eq!(p.show("io.takeRecords().length"), "1");
    p.rt.run_until_idle().unwrap();
    assert_eq!(p.show("log.length"), "1");
}

/// `ResizeObserver`: la primera observación se notifica (también la de un elemento sin caja),
/// después solo los cambios de la caja observada.
#[test]
fn resize_observer_notifies_changes() {
    let mut p = Page::load(
        "<body style='margin:0'><div id=a style='width:100px;height:40px;padding:5px'></div>\
         <div id=h style='display:none'></div><script>var log = [];\
         var ro = new ResizeObserver((es) => { for (const e of es) log.push([e.target.id, \
         e.contentRect.width, e.contentRect.height, e.contentRect.x, \
         e.borderBoxSize[0].inlineSize, e.contentBoxSize[0].blockSize].join(' ')); });\
         ro.observe(a); ro.observe(h); ro.observe(a, { box: 'border-box' });</script>",
    );
    assert!(p.wants_frame());
    p.frame();
    assert_eq!(p.show("log.join('|')"), "h 0 0 0 0 0|a 100 40 5 110 40");
    p.frame();
    assert_eq!(p.show("log.length"), "2");
    p.show("a.style.width = '200px'");
    p.frame();
    assert_eq!(p.show("log[2]"), "a 200 40 5 210 40");
    assert_eq!(
        p.show("try { ro.observe(a, { box: 'margin-box' }) } catch (e) { e.name }"),
        "TypeError"
    );
}

/// El bucle de `ResizeObserver` (§3.4.1): un callback que vuelve a cambiar el tamaño de su
/// objetivo deja una observación saltada, y la ventana recibe el error de la especificación.
#[test]
fn resize_observer_loop_error() {
    let mut p = Page::load(
        "<body style='margin:0'><div id=a style='width:10px;height:10px'></div><script>\
         var calls = 0, errors = []; addEventListener('error', (e) => errors.push(e.message));\
         new ResizeObserver(() => { calls++; a.style.width = (10 + calls) + 'px'; }).observe(a);\
         </script>",
    );
    p.frame();
    assert_eq!(p.show("calls"), "1");
    assert_eq!(
        p.show("errors.join()"),
        "ResizeObserver loop completed with undelivered notifications."
    );
}
