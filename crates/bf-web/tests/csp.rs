//! E6: Content Security Policy en el motor web: `img-src` para los `<img>` (CSP 3 §6.1.6, con
//! `default-src` de respaldo) y el evento `securitypolicyviolation` (CSP 3 §5.3) con los datos
//! de `SecurityPolicyViolationEvent` (§5.1).

use std::sync::Arc;

use bf_js::host::DefaultHooks;
use bf_web::{DocumentOwner, Runtime, Url, WebContext, WebContextHandle, bom};

/// Un GIF de 1×1.
const GIF_1X1: &str =
    "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";

struct Page {
    rt: Runtime,
    ctx: WebContextHandle,
}

impl Page {
    /// Una página de `https://example.test/` cargada con el parser y sus scripts (sin
    /// navegador: la política sale de los `<meta>`), con el bucle de eventos hasta vaciarse.
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
}

/// Escucha `securitypolicyviolation` en el documento y anota los datos de cada evento.
const LISTEN: &str = "<script nonce=n>var log = []; \
    document.addEventListener('securitypolicyviolation', e => log.push([\
      e.constructor.name, e.isTrusted, e.target.nodeName || e.target.constructor.name, \
      e.blockedURI, e.effectiveDirective, e.violatedDirective, e.disposition, \
      e.originalPolicy, e.documentURI].join(' | ')));</script>";

/// `img-src 'none'` en un `<meta>`: la imagen no se pide, dispara `error` (no `load`) y
/// `securitypolicyviolation` en el `<img>`, que llega al documento.
#[test]
fn img_src_blocks_images_and_fires_the_event() {
    let mut p = Page::load(&format!(
        "<meta http-equiv=Content-Security-Policy content=\"img-src 'none'; script-src 'nonce-n'\">\
         {LISTEN}<body><script nonce=n>var img = document.createElement('img');\
         img.onload = () => log.push('load'); img.onerror = () => log.push('error');\
         img.src = '{GIF_1X1}'; document.body.appendChild(img);</script>"
    ));
    let log = p.show("log.join('\\n')");
    let lines: Vec<&str> = log.lines().collect();
    assert!(lines.contains(&"error"), "{log}");
    assert!(!lines.contains(&"load"), "{log}");
    assert!(
        lines.contains(
            &"SecurityPolicyViolationEvent | true | IMG | data | img-src | img-src | enforce | \
              img-src 'none'; script-src 'nonce-n' | https://example.test/"
        ),
        "{log}"
    );
    assert_eq!(p.show("img.complete + ' ' + img.naturalWidth"), "true 0");
}

/// Sin `img-src`, se aplica `default-src` (CSP 3 §6.8.3); un `img-src` que permite `data:`
/// deja cargar la imagen sin eventos.
#[test]
fn img_src_falls_back_to_default_src() {
    let mut p = Page::load(&format!(
        "<meta http-equiv=Content-Security-Policy content=\"default-src 'self'; script-src 'nonce-n'\">\
         {LISTEN}<img id=i src='{GIF_1X1}' onload=\"log.push('load')\" onerror=\"log.push('error')\">"
    ));
    // El `onerror` en línea es un manejador (script-src-attr) que no se comprueba todavía.
    let log = p.show("log.join('\\n')");
    assert!(log.contains("error"), "{log}");
    assert!(
        log.contains("| IMG | data | img-src | img-src | enforce |"),
        "{log}"
    );

    let mut p = Page::load(&format!(
        "<meta http-equiv=Content-Security-Policy content=\"default-src 'self'; img-src data:; \
         script-src 'nonce-n'\">\
         {LISTEN}<img id=i src='{GIF_1X1}' onload=\"log.push('load')\" onerror=\"log.push('error')\">"
    ));
    assert_eq!(p.show("log.join('\\n')"), "load");
}

/// Un script en línea sin nonce: el evento va al `<script>` (`blockedURI` `inline`,
/// `script-src-elem`); `eval`, al documento (`eval`, `script-src`).
#[test]
fn inline_script_and_eval_violations() {
    let mut p = Page::load(&format!(
        "<meta http-equiv=Content-Security-Policy content=\"script-src 'nonce-n'\">\
         {LISTEN}<script id=bloqueado>log.push('corrió')</script>\
         <script nonce=n>try {{ eval('1') }} catch (e) {{ log.push(e.name) }}</script>"
    ));
    let log = p.show("log.join('\\n')");
    let lines: Vec<&str> = log.lines().collect();
    assert!(!lines.contains(&"corrió"), "{log}");
    assert!(lines.contains(&"EvalError"), "{log}");
    assert!(
        lines.iter().any(|l| l.contains(
            "| SCRIPT | inline | script-src-elem | script-src-elem | enforce | script-src 'nonce-n' |"
        )),
        "{log}"
    );
    assert!(
        lines.iter().any(|l| l
            .contains("| HTMLDocument | eval | script-src | script-src | enforce |")
            || l.contains("| #document | eval | script-src | script-src | enforce |")),
        "{log}"
    );
}

/// `new SecurityPolicyViolationEvent(type, dict)`: los miembros del diccionario con sus
/// valores por defecto; `disposition` es un enum de WebIDL.
#[test]
fn constructor() {
    let mut p = Page::load("");
    assert_eq!(
        p.show(
            "var e = new SecurityPolicyViolationEvent('securitypolicyviolation', \
             {blockedURI: 'https://a.test/x', violatedDirective: 'img-src', statusCode: 200}); \
             [e.type, e.blockedURI, e.violatedDirective, e.effectiveDirective === '', \
              e.disposition, e.statusCode, e.lineNumber, e.bubbles, e.isTrusted].join(' ')"
        ),
        "securitypolicyviolation https://a.test/x img-src true enforce 200 0 false false"
    );
    assert_eq!(
        p.show(
            "try { new SecurityPolicyViolationEvent('x', {disposition: 'otra'}); 'no' } \
             catch (e) { e.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        p.show("new SecurityPolicyViolationEvent('x', {disposition: 'report'}).disposition"),
        "report"
    );
}
