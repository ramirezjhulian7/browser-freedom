//! La *activation behavior* de HTML que completa `bf_web::html::activation`: enlaces,
//! envío y reset de formularios (con `submit`, `formdata` e `invalid`), `label` y
//! `summary`/`details` (con `toggle`).

use std::cell::RefCell;
use std::rc::Rc;

use bf_js::host::DefaultHooks;
use bf_web::bom::{self, BomHooks};
use bf_web::{DocumentOwner, Runtime, Url, WebContext, WebContextHandle};

type Log = Rc<RefCell<Vec<String>>>;

struct NavBom(Log);

impl BomHooks for NavBom {
    fn navigate(&mut self, url: &Url, replace: bool) {
        self.0
            .borrow_mut()
            .push(format!("navigate {} {replace}", url.href()));
    }
    fn navigate_post(&mut self, url: &Url, body: Vec<u8>, content_type: &str) {
        self.0.borrow_mut().push(format!(
            "post {} {content_type} {}",
            url.href(),
            String::from_utf8_lossy(&body)
        ));
    }
    fn same_document_navigation(&mut self, url: &Url, _replace: bool) {
        self.0.borrow_mut().push(format!("same {}", url.href()));
    }
}

struct Page {
    rt: Runtime,
    ctx: WebContextHandle,
    log: Log,
}

impl Page {
    fn new(body: &str) -> Page {
        let html =
            format!("<!DOCTYPE html><html><head><title>T</title></head><body>{body}</body></html>");
        let mut rt = Runtime::new(Box::new(DefaultHooks));
        let (mut doc, _) = bf_html::parse_document(&html, &bf_html::ParseOptions::default());
        let realm = rt.main_realm();
        let url = Url::parse("https://example.com/dir/page.html", None).expect("url");
        doc.url = url.clone();
        let ctx = bf_web::install(
            &mut rt.agent,
            realm,
            WebContext::new(DocumentOwner::Document(Box::new(doc)), url),
        );
        let log = Log::default();
        bom::set_hooks(&mut rt.agent, realm, Box::new(NavBom(log.clone())));
        let mut p = Page { rt, ctx, log };
        // Los elementos con `id` como globales (sin el named access de `Window`).
        p.show("for (const e of document.querySelectorAll('[id]')) globalThis[e.id] = e");
        p
    }

    fn show(&mut self, src: &str) -> String {
        let realm = self.ctx.realm;
        match self.rt.eval_script(realm, src, "test.js") {
            Ok(v) => self.rt.to_display(&v),
            Err(e) => format!("Error: {}", e.message),
        }
    }

    fn nav(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.borrow_mut())
    }
}

#[test]
fn links_follow_their_href() {
    let mut p = Page::new(
        "<a id=a href='/x/y?q=1'>a</a><a id=h href='#frag'>h</a><a id=b href=/b target=_blank>b</a>\
         <a id=d href=/d download>d</a><a id=n>sin href</a><span id=s></span>",
    );
    p.show("a.click()");
    assert_eq!(p.nav(), ["navigate https://example.com/x/y?q=1 false"]);
    // Un click en un descendiente sube hasta el enlace.
    p.show("a.appendChild(s); s.click()");
    assert_eq!(p.nav(), ["navigate https://example.com/x/y?q=1 false"]);
    // `preventDefault` cancela la navegación.
    p.show("a.addEventListener('click', e => e.preventDefault()); a.click()");
    assert!(p.nav().is_empty());
    p.show("h.click()");
    assert_eq!(p.nav(), ["same https://example.com/dir/page.html#frag"]);
    // `target=_blank` no navega esta ventana; `download` y sin `href`, nada.
    p.show("b.click(); d.click(); n.click()");
    assert!(p.nav().is_empty());
    // Un `click` despachado por script también activa.
    p.show("h.dispatchEvent(new MouseEvent('click', {bubbles: true, cancelable: true}))");
    assert_eq!(p.nav(), ["same https://example.com/dir/page.html#frag"]);
    // Un `Event` que no es de ratón, no.
    p.show("h.dispatchEvent(new Event('click'))");
    assert!(p.nav().is_empty());
}

#[test]
fn submit_button_submits_the_form() {
    let mut p = Page::new(
        "<form id=f action=/go><input name=q id=q value=v><input type=checkbox name=c checked>\
         <input type=checkbox name=u><textarea name=t id=t>a\nb</textarea>\
         <select name=s><option>uno<option selected value=2>dos</select>\
         <button id=b name=btn value=ok>go</button><input type=submit id=other name=o value=x>\
         </form>",
    );
    let log = p.show(
        "var log = []; f.addEventListener('submit', e => log.push(e.type + ':' + \
         (e instanceof SubmitEvent) + ':' + (e.submitter === b) + ':' + e.isTrusted + ':' + \
         e.cancelable)); \
         f.addEventListener('formdata', e => { log.push(e.constructor.name + ':' + \
         e.formData.get('q')); e.formData.append('extra', '1'); }); \
         q.value = 'x y'; b.click(); log.join()",
    );
    assert_eq!(log, "submit:true:true:true:true,FormDataEvent:x y");
    assert_eq!(
        p.nav(),
        ["navigate https://example.com/go?q=x+y&c=on&t=a%0D%0Ab&s=2&btn=ok&extra=1 false"]
    );
    // Cancelar `submit` no envía.
    p.show("f.onsubmit = e => e.preventDefault(); other.click()");
    assert!(p.nav().is_empty());
}

#[test]
fn post_and_overrides() {
    let mut p = Page::new(
        "<form id=f method=post action=/a><input name=n value='1 2'>\
         <button id=b formaction=/b>b</button><button id=g formmethod=get>g</button>\
         <button id=m formenctype=text/plain>m</button></form>",
    );
    p.show("b.click()");
    assert_eq!(
        p.nav(),
        ["post https://example.com/b application/x-www-form-urlencoded n=1+2"]
    );
    p.show("g.click()");
    assert_eq!(p.nav(), ["navigate https://example.com/a?n=1+2 false"]);
    p.show("m.click()");
    assert_eq!(p.nav(), ["post https://example.com/a text/plain n=1 2\r\n"]);
}

#[test]
fn submit_methods_and_validation() {
    let mut p = Page::new(
        "<form id=f action=/s><input id=r name=r required><input type=submit id=b></form>\
         <form id=g action=/g novalidate><input name=r required></form>",
    );
    // `requestSubmit()` valida: `invalid` en el control y no se envía.
    let log = p.show(
        "var log = []; r.addEventListener('invalid', e => log.push('invalid:' + e.cancelable)); \
         f.addEventListener('submit', e => log.push('submit')); f.requestSubmit(); log.join()",
    );
    assert_eq!(log, "invalid:true");
    assert!(p.nav().is_empty());
    // `submit()` no valida ni dispara eventos.
    assert_eq!(p.show("f.submit(); log.join()"), "invalid:true");
    assert_eq!(p.nav(), ["navigate https://example.com/s?r= false"]);
    // Con valor, `requestSubmit()` envía con `submit`.
    assert_eq!(
        p.show("r.value = 'ok'; f.requestSubmit(); log.join()"),
        "invalid:true,submit"
    );
    assert_eq!(p.nav(), ["navigate https://example.com/s?r=ok false"]);
    // `novalidate`.
    p.show("g.requestSubmit()");
    assert_eq!(p.nav(), ["navigate https://example.com/g?r= false"]);
    // `checkValidity()` también dispara `invalid`.
    assert_eq!(
        p.show("r.value = ''; log = []; [r.checkValidity(), f.checkValidity(), log.join()].join()"),
        "false,false,invalid:true,invalid:true"
    );
    // Un formulario desconectado no se envía.
    p.show("var x = document.createElement('form'); x.action = '/x'; x.submit()");
    assert!(p.nav().is_empty());
}

#[test]
fn reset_button_resets_controls() {
    let mut p = Page::new(
        "<form id=f><input id=i value=a><input type=checkbox id=c checked>\
         <select id=s><option>1<option selected>2</select><textarea id=t>x</textarea>\
         <input type=reset id=r></form>",
    );
    let r = p.show(
        "var log = []; f.addEventListener('reset', e => log.push(e.isTrusted + ':' + e.cancelable)); \
         i.value = 'b'; c.checked = false; s.selectedIndex = 0; t.value = 'y'; r.click(); \
         [log.join(), i.value, c.checked, s.selectedIndex, t.value].join()",
    );
    assert_eq!(r, "true:true,a,true,1,x");
    // Cancelar `reset` no resetea.
    let r = p.show("f.onreset = e => e.preventDefault(); i.value = 'b'; f.reset(); i.value");
    assert_eq!(r, "b");
}

#[test]
fn label_forwards_clicks() {
    let mut p = Page::new(
        "<label id=l>texto <span id=s>x</span><input type=checkbox id=c></label>\
         <label id=l2 for=t>t</label><input id=t><label id=l3><input id=u></label>",
    );
    let r = p.show(
        "var log = []; c.addEventListener('change', () => log.push('change')); \
         c.addEventListener('click', e => log.push('click:' + (e.target === c))); \
         s.click(); [log.join(), c.checked].join()",
    );
    assert_eq!(r, "click:true,change,true");
    // Un click en el propio control no se reenvía otra vez.
    assert_eq!(
        p.show("log = []; c.click(); [log.join(), c.checked].join()"),
        "click:true,change,false"
    );
    // `for` y un control de texto dentro del label.
    let r = p.show(
        "var n = 0; t.addEventListener('click', () => n++); u.addEventListener('click', () => n++); \
         l2.click(); u.click(); n",
    );
    assert_eq!(r, "2");
    // `preventDefault` en el label cancela el reenvío.
    let r = p.show(
        "l.addEventListener('click', e => e.preventDefault()); log = []; s.click(); log.join()",
    );
    assert_eq!(r, "");
}

#[test]
fn summary_toggles_details() {
    let mut p = Page::new(
        "<details id=d><summary id=s>s</summary><summary id=s2>no</summary>contenido</details>",
    );
    let r = p.show(
        "var log = []; d.addEventListener('toggle', e => log.push(e.constructor.name + ':' + \
         e.oldState + '>' + e.newState)); s.click(); d.open",
    );
    assert_eq!(r, "true");
    // El evento va en una tarea.
    assert_eq!(p.show("log.join()"), "");
    p.rt.run_until_idle().expect("tareas");
    assert_eq!(p.show("log.join()"), "ToggleEvent:closed>open");
    // Solo el primer `summary` alterna.
    assert_eq!(p.show("s2.click(); d.open"), "true");
    // Varios cambios antes de la tarea se unen en un evento.
    p.show("log = []; d.open = false; d.open = true; d.removeAttribute('open')");
    p.rt.run_until_idle().expect("tareas");
    assert_eq!(p.show("log.join()"), "ToggleEvent:open>closed");
    assert_eq!(
        p.show("var e = new ToggleEvent('toggle', {oldState: 'a', newState: 'b'}); e.oldState + e.newState"),
        "ab"
    );
}

#[test]
fn form_data_constructor_fires_formdata() {
    let mut p = Page::new("<form id=f><input name=a value=1><input name=b id=b></form>");
    let r = p.show(
        "var log = []; f.addEventListener('formdata', e => { log.push(e.bubbles); \
         try { new FormData(f); } catch (x) { log.push(x.name); } }); \
         b.value = 'dos'; var fd = new FormData(f); [log.join(), fd.get('a'), fd.get('b')].join()",
    );
    assert_eq!(r, "true,InvalidStateError,1,dos");
    assert_eq!(
        p.show("try { new FormDataEvent('formdata') } catch (e) { e.name }"),
        "TypeError"
    );
    assert_eq!(
        p.show("new FormDataEvent('x', {formData: new FormData()}).formData instanceof FormData"),
        "true"
    );
    assert_eq!(
        p.show("new SubmitEvent('submit').submitter === null"),
        "true"
    );
}
