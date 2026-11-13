//! E2: `loading=lazy` en el pintado (HTML §4.8.4.3.9). Quien decide qué imagen perezosa
//! espera es el motor web (`bf-web`, con su margen de cercanía al viewport); lo anota en el
//! documento (`Document::set_image_deferred`) y el pintado no pide las anotadas. Un documento
//! sin motor (sin scripts) no tiene anotaciones: `lazy` no aplica y se piden todas.

use std::sync::{Arc, Mutex};

use bf_dom::{Document, NodeId};
use bf_html::{ParseOptions, parse_document};
use bf_infra::{Resource, ResourceLoader, Url};
use bf_layout::Size;
use bf_layout::fragment::Fragment;
use bf_paint::{PageContext, prepare_page};

/// Un GIF de 1×1.
const GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff!\xf9\x04\x01\
                     \x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x01D\x00;";

/// Sirve el GIF en cualquier URL y anota las rutas pedidas.
#[derive(Default)]
struct Counting(Mutex<Vec<String>>);

impl ResourceLoader for Counting {
    fn load(&self, url: &Url) -> Option<Resource> {
        self.0.lock().unwrap().push(url.path_string());
        Some(Resource {
            bytes: GIF.to_vec(),
            url: url.clone(),
            content_type: Some("image/gif".into()),
        })
    }
}

const PAGE: &str = "<body style='margin:0'>\
    <img id=cerca loading=lazy src='/cerca.gif'>\
    <div style='height:10000px'></div>\
    <img id=lejos loading=lazy src='/lejos.gif' width=40 height=30>";

fn by_id(doc: &Document, id: &str) -> NodeId {
    doc.inclusive_descendants(doc.root())
        .into_iter()
        .find(|&n| doc.element(n).is_some_and(|e| e.attr("id") == Some(id)))
        .unwrap()
}

/// Pinta `doc` y devuelve las rutas pedidas, si `lejos` tiene imagen y el tamaño de su caja.
fn paint(doc: &Document) -> (Vec<String>, bool, (f32, f32)) {
    let loader = Arc::new(Counting::default());
    let base = Url::parse("http://example.test/", None).unwrap();
    let ctx = PageContext {
        base: &base,
        encoding: bf_infra::encoding::Encoding::Utf8,
        loader: loader.clone(),
        fonts: Arc::new(bf_font::FontDb::from_faces(Vec::new())),
        extra: &[],
        text_zoom: 1.0,
        device_pixel_ratio: 1.0,
    };
    let viewport = Size {
        width: 800.0,
        height: 600.0,
    };
    let page = prepare_page(doc, &ctx, viewport);
    let lejos = by_id(doc, "lejos");
    fn find(f: &Fragment, node: NodeId) -> Option<&Fragment> {
        if f.node == Some(node) {
            return Some(f);
        }
        f.children.iter().find_map(|c| find(c, node))
    }
    let r = find(page.tree.root.as_ref().unwrap(), lejos)
        .expect("caja de lejos")
        .content_rect();
    let mut paths = loader.0.lock().unwrap().clone();
    paths.sort();
    (
        paths,
        page.images.for_node(lejos).is_some(),
        (r.width, r.height),
    )
}

#[test]
fn deferred_lazy_image_is_not_requested() {
    let (mut doc, _) = parse_document(PAGE, &ParseOptions::default());
    // Lo que deja el motor web: la cercana ya se pidió y llegó; la lejana espera.
    let lejos = by_id(&doc, "lejos");
    doc.set_image_deferred(lejos, true);
    let (paths, painted, size) = paint(&doc);
    assert_eq!(paths, ["/cerca.gif"]);
    assert!(!painted);
    // Sin imagen, la caja tiene el tamaño de sus atributos.
    assert_eq!(size, (40.0, 30.0));

    // Se acercó y llegó: el motor quita la anotación y el pintado la pide.
    doc.set_image_deferred(lejos, false);
    let (paths, painted, _) = paint(&doc);
    assert_eq!(paths, ["/cerca.gif", "/lejos.gif"]);
    assert!(painted);
}

/// Sin motor no hay anotaciones: un documento sin scripts pide todas las imágenes, también las
/// perezosas (§4.8.4.3.9: "will lazy load element steps" solo con el scripting activo).
#[test]
fn without_scripts_every_image_is_requested() {
    let (doc, _) = parse_document(PAGE, &ParseOptions::default());
    let (paths, painted, size) = paint(&doc);
    assert_eq!(paths, ["/cerca.gif", "/lejos.gif"]);
    assert!(painted);
    assert_eq!(size, (40.0, 30.0));
}
