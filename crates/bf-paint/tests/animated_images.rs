//! Imágenes animadas en una página (E7): `PageImages` guarda los fotogramas de un APNG y da el
//! visible en cada instante; sin animaciones no hay próximo cambio que esperar.

use std::time::Duration;

use bf_css::media::MediaContext;
use bf_css::selectors::MatchContext;
use bf_html::{ParseOptions, parse_document};
use bf_infra::{FileLoader, Url};
use bf_paint::load_images;
use bf_style::style_document;

fn images_for(html: &str) -> bf_paint::PageImages {
    let (doc, _) = parse_document(html, &ParseOptions::default());
    let media = MediaContext::default();
    let ctx = MatchContext::for_document(&doc);
    let styles = style_document(&doc, &[], &media, &ctx);
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../bf-image/tests/vectors/");
    let base = Url::from_file_path(&std::path::absolute(dir).unwrap()).unwrap();
    load_images(&doc, &styles, &base, &FileLoader::new())
}

#[test]
fn apng_frames_by_time() {
    let images = images_for("<img src='anim/three.apng'>");
    assert!(images.errors.is_empty(), "{:?}", images.errors);
    assert!(images.has_animations());
    let url = images.by_url.keys().next().unwrap().clone();
    let f0 = images.frame_at(&url, Duration::ZERO).unwrap();
    let f1 = images.frame_at(&url, Duration::from_millis(150)).unwrap();
    let f3 = images.frame_at(&url, Duration::from_millis(300)).unwrap();
    assert_ne!(f0.data(), f1.data());
    // Sin fin: a los 300 ms vuelve al primero.
    assert_eq!(f0.data(), f3.data());
    // La imagen fija (para el layout y el primer cuadro) es el primer fotograma.
    assert_eq!(images.for_url(&url).unwrap().data(), f0.data());
    assert_eq!(
        images.next_frame_change(Duration::from_millis(120)),
        Some(Duration::from_millis(80))
    );
}

#[test]
fn still_images_need_no_frames() {
    let images = images_for("<img src='bmp/bgr24.png'>");
    assert!(!images.has_animations());
    assert_eq!(images.next_frame_change(Duration::ZERO), None);
}
