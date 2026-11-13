//! E2: en la ventana, una imagen del documento se decodifica una sola vez, aunque la pidan los
//! dos lados: `bf-web` (eventos `load`, `naturalWidth`) y `bf-paint` (para pintarla). Los dos
//! pasan por la caché del proceso de `bf-image` (`decode_frames_shared`). Un solo test en este
//! binario: el contador es global.

use std::sync::Arc;

use bf_css::media::MediaContext;
use bf_css::selectors::MatchContext;
use bf_infra::{FileLoader, Url};
use bf_js::host::{DefaultHooks, Runtime};
use bf_web::{DocumentOwner, WebContext, WebContextHandle, bom};

/// Un PNG de 3×2 que no usa ningún otro test de este binario.
const PNG_3X2: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAMAAAACCAYAAACddGYaAAAAEUlEQVR4nGP4z8DwH4YZkDkAm34L9XKwuTwAAAAASUVORK5CYII=";

#[test]
fn document_image_decoded_once() {
    let start = bf_image::decode_count();
    // El lado de los scripts: la página se parsea, el `<img>` pide su imagen y dispara `load`.
    let mut rt = Runtime::new(Box::new(DefaultHooks));
    let realm = rt.main_realm();
    let url = Url::parse("https://example.test/", None).unwrap();
    bf_web::install(
        &mut rt.agent,
        realm,
        WebContext::new(DocumentOwner::Document(Box::default()), url.clone()),
    );
    bom::script::install_scripting(&mut rt.agent, realm, &url, Arc::new(FileLoader::new()));
    bom::script::begin_parse(&mut rt.agent, realm, &url);
    bom::script::feed(
        &mut rt.agent,
        realm,
        &format!(
            "<script>var log = [];</script><img id=i src='{PNG_3X2}' \
             onload=\"log.push(this.naturalWidth)\">"
        ),
    );
    bom::script::finish(&mut rt.agent, realm);
    bom::script::resources_loaded(&mut rt.agent, realm);
    rt.run_until_idle().expect("tareas");
    let log = rt.eval_script(realm, "log.join()", "t.js").expect("log");
    assert_eq!(rt.to_display(&log), "3");
    assert_eq!(
        bf_image::decode_count() - start,
        1,
        "bf-web decodificó una vez"
    );
    // El lado de la pintura, sobre el mismo documento: la imagen ya está decodificada.
    let doc = WebContextHandle { realm }.get(&rt.agent).document().clone();
    let media = MediaContext::default();
    let styles = bf_style::style_document(&doc, &[], &media, &MatchContext::for_document(&doc));
    let images = bf_paint::load_images(&doc, &styles, &url, &FileLoader::new());
    assert!(images.errors.is_empty(), "{:?}", images.errors);
    assert_eq!(images.by_url.len(), 1);
    assert_eq!(
        bf_image::decode_count() - start,
        1,
        "bf-paint no volvió a decodificar"
    );
}
