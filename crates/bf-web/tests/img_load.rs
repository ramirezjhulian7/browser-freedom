//! E2: "update the image data" (HTML §4.8.4.3.5) de los `<img>` del documento: los que crea el
//! parser, los que inserta un script y los que cambian `src` desde un script piden su imagen,
//! disparan `load` o `error` en una tarea, demoran el `load` de la ventana y dejan `complete`,
//! `naturalWidth`/`naturalHeight` y `decode()` según el resultado.

use std::sync::Arc;

use bf_js::host::DefaultHooks;
use bf_web::{DocumentOwner, Runtime, Url, WebContext, WebContextHandle, bom};

/// Un GIF de 1×1 (el del caso mínimo de E6).
const GIF_1X1: &str =
    "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";

/// Un PNG de 3×2.
const PNG_3X2: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAMAAAACCAYAAACddGYaAAAAEUlEQVR4nGP4z8DwH4YZkDkAm34L9XKwuTwAAAAASUVORK5CYII=";

struct Page {
    rt: Runtime,
    ctx: WebContextHandle,
}

impl Page {
    /// Una página cargada con el parser y sus scripts, como en la pestaña: después de
    /// `resources_loaded` (lo que el navegador carga) el bucle de eventos corre hasta vaciarse.
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
        // Los marcos, como el hilo de la pestaña (`bf-shell`, `Engine::parse`).
        bom::frames::sync_frames(&mut rt.agent, realm);
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

    fn idle(&mut self) {
        self.rt.run_until_idle().expect("tareas");
    }
}

/// El caso mínimo de E6: un script asigna `src` a un `<img>` del documento.
#[test]
fn script_assigned_src_fires_load() {
    let mut p = Page::load(&format!(
        "<script>var log = [];</script><img id=i>\
         <script>i.onload = () => log.push('ok ' + i.naturalWidth + ' ' + i.complete); \
         log.push('antes ' + i.complete); i.src = '{GIF_1X1}'; log.push('después ' + i.complete);</script>"
    ));
    assert_eq!(
        p.show("log.join('|')"),
        "antes true|después false|ok 1 true"
    );
}

/// Un `src` que no es una imagen dispara `error`; uno vacío también; sin `src` ni `srcset`,
/// ninguno de los dos.
#[test]
fn invalid_src_fires_error() {
    let mut p = Page::load(
        "<script>var log = [];</script>\
         <img id=a src='data:text/plain,hola' onload=\"log.push('load a')\" onerror=\"log.push('error a')\">\
         <img id=b><img id=c>\
         <script>b.onerror = () => log.push('error b ' + b.complete + ' ' + b.naturalWidth); \
         b.src = 'data:text/plain,x'; c.onerror = () => log.push('error c'); c.src = '';</script>",
    );
    assert_eq!(
        p.show("log.slice().sort().join('|')"),
        "error a|error b true 0|error c"
    );
}

/// Las imágenes del markup cargan y demoran el `load` de la ventana: en `onload`,
/// `naturalWidth` ya tiene el tamaño de la imagen.
#[test]
fn parser_images_delay_window_load() {
    let mut p = Page::load(&format!(
        "<script>var log = []; addEventListener('load', () => log.push('window ' + \
         document.getElementById('p').naturalWidth + 'x' + document.getElementById('p').naturalHeight));</script>\
         <img id=p src='{PNG_3X2}' onload=\"log.push('img')\">"
    ));
    assert_eq!(p.show("log.join('|')"), "img|window 3x2");
    assert_eq!(p.show("document.getElementById('p').complete"), "true");
}

/// Los `<img>` de `innerHTML` y de `appendChild` también cargan.
#[test]
fn inserted_images_load() {
    let mut p = Page::load("<div id=d></div>");
    p.show(&format!(
        "var log = []; d.innerHTML = \"<img id=m src='{PNG_3X2}' onload='log.push(\\\"markup\\\")'>\"; \
         var e = document.createElement('img'); e.onload = () => log.push('createElement'); \
         e.src = '{GIF_1X1}'; d.appendChild(e);"
    ));
    p.idle();
    assert_eq!(
        p.show("log.slice().sort().join('|')"),
        "createElement|markup"
    );
}

/// `decode()` se cumple con una imagen buena y se rechaza con `EncodingError` con una rota o
/// si la petición cambia antes de terminar.
#[test]
fn decode_promise() {
    let mut p = Page::load("<img id=g><img id=r><img id=x>");
    p.show(&format!(
        "var log = []; g.src = '{PNG_3X2}'; r.src = 'data:text/plain,no'; x.src = '{GIF_1X1}'; \
         g.decode().then(() => log.push('g ok'), e => log.push('g ' + e.name)); \
         r.decode().then(() => log.push('r ok'), e => log.push('r ' + e.name)); \
         x.decode().then(() => log.push('x ok'), e => log.push('x ' + e.name)); \
         x.src = '{PNG_3X2}';"
    ));
    p.idle();
    assert_eq!(
        p.show("log.slice().sort().join('|')"),
        "g ok|r EncodingError|x EncodingError"
    );
    // Ya cargada: se cumple.
    p.show("g.decode().then(() => log.push('g otra vez'))");
    p.idle();
    assert_eq!(p.show("log.includes('g otra vez')"), "true");
}

/// Los `<link>` del parser también disparan su evento antes que la ventana: una hoja (aquí
/// `error`: el cargador de la página no sirve `data:` para hojas) y un `preload` (que con
/// `onload="this.rel='stylesheet'"` es como cargan CSS muchos sitios).
#[test]
fn parser_links_fire_load() {
    let mut p = Page::load(&format!(
        "<script>var log = []; addEventListener('load', () => log.push('window'));</script>\
         <link rel=stylesheet href='data:text/css,p%7Bcolor:red%7D' onload=\"log.push('css load')\" \
         onerror=\"log.push('css error')\">\
         <link rel=preload as=image href='{GIF_1X1}' onload=\"log.push('preload')\">\
         <link rel=preload as=style href='data:text/css,a%7B%7D' \
         onload=\"log.push('swap'); this.rel='stylesheet'\">"
    ));
    let log = p.show("log.join('|')");
    assert!(log.ends_with("window"), "{log}");
    for e in ["css error", "preload", "swap"] {
        assert!(log.contains(e), "{e}: {log}");
    }
    // El `preload` que pasó a `stylesheet` ya es una hoja del documento.
    assert_eq!(p.show("document.styleSheets.length"), "2");
}

/// HTML §7.4.6.3 ("iframe load event steps"): el `load` del `<iframe>` sale después del
/// `load` de su documento, que espera a sus imágenes; y el `load` de la ventana, después.
#[test]
fn iframe_load_waits_for_its_document() {
    let mut p = Page::load(&format!(
        "<script>var log = []; addEventListener('load', () => log.push('window'));</script>\
         <iframe srcdoc=\"<img src='{GIF_1X1}' onload='parent.log.push(1)'>\" \
         onload=\"log.push(2)\"></iframe>"
    ));
    assert_eq!(p.show("log.join()"), "1,2,window");
}

/// `loading=lazy` (HTML §4.8.4.3.9): la imagen cerca del viewport se pide; la que está muy
/// abajo espera (sin `complete`) hasta que el desplazamiento la acerca; una sin caja
/// (`display: none`) no se pide nunca.
#[test]
fn lazy_images_wait_for_the_viewport() {
    let mut p = Page::load(&format!(
        "<script>var log = [];</script><body style='margin:0'>\
         <img id=cerca loading=lazy src='{GIF_1X1}' onload=\"log.push('cerca')\">\
         <div style='height:10000px'></div>\
         <img id=lejos loading=lazy src='{PNG_3X2}' onload=\"log.push('lejos')\">\
         <img id=oculta loading=lazy style='display:none' src='{PNG_3X2}' \
         onload=\"log.push('oculta')\">"
    ));
    assert_eq!(p.show("log.join()"), "cerca");
    assert_eq!(
        p.show("[lejos.complete, lejos.naturalWidth].join()"),
        "false,0"
    );
    p.show("scrollTo(0, 9500)");
    let realm = p.ctx.realm;
    bom::frames::sync_frames(&mut p.rt.agent, realm);
    p.idle();
    assert_eq!(p.show("log.join()"), "cerca,lejos");
    assert_eq!(p.show("lejos.naturalWidth"), "3");
}
