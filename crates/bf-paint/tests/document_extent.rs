//! E2: la extensión del documento (`document_extent`, lo que se puede desplazar) recorta cada
//! caja con el `overflow` que de verdad la recorta (CSS 2.2 §11.1.1, CSS Overflow 3 §2.2).
//! El caso de asimetrix.co: un `absolute` centrado con `left: 50%` y `transform` dentro de un
//! `relative overflow: hidden` ensanchaba el documento y la página se desplazaba en horizontal.

use bf_css::media::MediaContext;
use bf_css::selectors::MatchContext;
use bf_html::{ParseOptions, parse_document};
use bf_layout::{Size, TestFont, layout_document};
use bf_paint::document_extent;
use bf_style::style_document;

/// (ancho, alto) de la extensión del documento en un viewport de 800×600.
fn extent(html: &str) -> (f32, f32) {
    let (doc, _) = parse_document(html, &ParseOptions::default());
    let ctx = MatchContext::for_document(&doc);
    let media = MediaContext {
        width: 800.0,
        height: 600.0,
        ..MediaContext::default()
    };
    let styles = style_document(&doc, &[], &media, &ctx);
    let tree = layout_document(
        &doc,
        &styles,
        Size {
            width: 800.0,
            height: 600.0,
        },
        &TestFont,
    );
    let r = document_extent(&tree);
    (r.width, r.height)
}

#[test]
fn absolute_inside_its_clipping_containing_block_is_clipped() {
    // El bloque contenedor del `absolute` es la caja que recorta: no cuenta lo que sobresale.
    let html = "<!doctype html><style>body { margin: 0 }</style>\
        <div style='position: relative; overflow: hidden; width: 700px; height: 100px'>\
        <div style='position: absolute; left: 50%; top: 50%; width: 100%; height: 100%; \
        transform: translate(-50%, -50%)'></div></div>";
    assert_eq!(extent(html), (800.0, 600.0));
    // Lo mismo con un `relative` y con una caja en flujo más ancha.
    let html = "<!doctype html><style>body { margin: 0 }</style>\
        <div style='overflow: hidden; width: 700px'>\
        <div style='position: relative; left: 500px; width: 700px; height: 10px'></div>\
        <div style='width: 2000px; height: 10px'></div></div>";
    assert_eq!(extent(html), (800.0, 600.0));
}

#[test]
fn absolute_escaping_a_static_clip_still_counts() {
    // El bloque contenedor está fuera de la caja que recorta (es el bloque inicial): el
    // `overflow` no lo recorta y el documento crece hasta él.
    let html = "<!doctype html><style>body { margin: 0 }</style>\
        <div style='overflow: hidden; width: 300px; height: 100px'>\
        <div style='position: absolute; left: 900px; width: 100px; height: 10px'></div></div>";
    assert_eq!(extent(html), (1000.0, 600.0));
    // Un `fixed` con el viewport como bloque contenedor no se mueve al desplazar: no agranda
    // el documento (CSS Position 3 §2.1, CSS Overflow 3 §2.2), ni siquiera escondido a la
    // derecha con `transform` (el menú lateral de DuckDuckGo).
    let html = "<!doctype html><style>body { margin: 0 }</style>\
        <div style='overflow: hidden; width: 300px; height: 100px; position: relative'>\
        <div style='position: fixed; top: 900px; width: 10px; height: 10px'></div></div>";
    assert_eq!(extent(html), (800.0, 600.0));
    let html = "<!doctype html><style>body { margin: 0 }</style>\
        <div style='position: fixed; right: 0; top: 0; width: 260px; height: 100%; \
        transform: translateX(100%)'></div>";
    assert_eq!(extent(html), (800.0, 600.0));
    // Con un antepasado transformado, ese es su bloque contenedor: cuenta, recortado por él.
    let html = "<!doctype html><style>body { margin: 0 }</style>\
        <div style='overflow: hidden; width: 300px; height: 100px; transform: scale(1)'>\
        <div style='position: fixed; top: 900px; width: 10px; height: 10px'></div></div>";
    assert_eq!(extent(html), (800.0, 600.0));
    let html = "<!doctype html><style>body { margin: 0 }</style>\
        <div style='width: 300px; height: 100px; transform: scale(1)'>\
        <div style='position: fixed; top: 900px; width: 10px; height: 10px'></div></div>";
    assert_eq!(extent(html), (800.0, 910.0));
}

/// CSS Overflow 3 §2.2: cuentan los border boxes, y el margin box solo de los ítems de flex y
/// grid. El caso de El País: una caja de 334 px cuyo margen derecho llegaba a 1298.
#[test]
fn margins_count_only_for_flex_and_grid_items() {
    let html = "<!doctype html><style>body { margin: 0 }</style>\
        <div style='width: 334px; height: 10px; margin-right: 964px'></div>";
    assert_eq!(extent(html), (800.0, 600.0));
    let html = "<!doctype html><style>body { margin: 0 }</style>\
        <div style='display: flex'><div style='width: 334px; height: 10px; \
        margin-right: 964px; flex: none'></div></div>";
    assert_eq!(extent(html), (1298.0, 600.0));
    // La raíz sigue contando su margin box: el margen inferior que colapsa a través de ella.
    let html = "<!doctype html><style>body { margin: 0 }</style>\
        <div style='height: 600px'></div><p style='margin: 0 0 50px'>x</p>";
    let (_, h) = extent(html);
    assert!(h > 650.0, "{h}");
}

/// CSS Overflow 3 §2.2: el desbordamiento desplazable cuenta las cajas transformadas, donde
/// quedan después de su `transform`.
#[test]
fn transforms_move_the_extent() {
    let doc = |body: &str| format!("<!doctype html><style>body {{ margin: 0 }}</style>{body}");
    // Una traslación que saca la caja del viewport lo agranda.
    let html = doc("<div style='width: 100px; height: 10px; transform: translateX(900px)'></div>");
    assert_eq!(extent(&html), (1000.0, 600.0));
    // Una caja ancha encogida (o devuelta con una traslación) ya no lo agranda.
    let html = doc(
        "<div style='width: 2000px; height: 10px; transform-origin: 0 0; \
         transform: scale(0.25)'></div>",
    );
    assert_eq!(extent(&html), (800.0, 600.0));
    let html =
        doc("<div style='width: 1500px; height: 10px; transform: translateX(-1000px)'></div>");
    assert_eq!(extent(&html), (800.0, 600.0));
    // Los hijos van con la caja, y las transformaciones anidadas se componen.
    let html = doc(
        "<div style='width: 10px; height: 10px; transform: translateY(700px)'>\
         <div style='width: 10px; height: 50px; transform: translateY(100px)'></div></div>",
    );
    assert_eq!(extent(&html), (800.0, 850.0));
    // Lo transformado sigue recortado por un `overflow` de fuera.
    let html = doc(
        "<div style='overflow: hidden; width: 300px; height: 100px'>\
         <div style='width: 10px; height: 10px; transform: translateX(900px)'></div></div>",
    );
    assert_eq!(extent(&html), (800.0, 600.0));
}
