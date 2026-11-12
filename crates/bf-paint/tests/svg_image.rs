//! SVG como imagen (E7): rasterizado de `bf-svg` al tamaño pedido, `<img>` con SVG,
//! `mask-image` con un icono SVG (como los de Wikipedia) y SVG en línea en HTML.

use bf_css::media::MediaContext;
use bf_css::selectors::MatchContext;
use bf_html::{ParseOptions, parse_document};
use bf_infra::{FileLoader, Url};
use bf_layout::{Size, TestFont, layout_document_with_images};
use bf_paint::images::{SvgResources, rasterize_svg, rasterize_svg_with};
use bf_paint::{Pixmap, build_display_list_with_images, load_images, render};
use bf_style::style_document;
use bf_style::values::color::Rgba;
use bf_svg::Svg;

fn raster(src: &str, w: u32, h: u32) -> Pixmap {
    let svg = Svg::parse(src).expect("SVG válido");
    rasterize_svg(&svg.tree(w as f32, h as f32), w, h, None).expect("bitmap")
}

/// (r, g, b, a) sin premultiplicar.
fn px(p: &Pixmap, x: u32, y: u32) -> (u8, u8, u8, u8) {
    let c = p.pixel(x, y).expect("dentro").demultiply();
    (c.red(), c.green(), c.blue(), c.alpha())
}

const SVG_NS: &str = "xmlns='http://www.w3.org/2000/svg'";

#[test]
fn shapes_and_view_box() {
    let p = raster(
        &format!(
            "<svg {SVG_NS} viewBox='0 0 20 20'><rect x='0' y='0' width='10' height='10' fill='red'/><circle cx='15' cy='15' r='4' fill='#00f'/></svg>"
        ),
        40,
        40,
    );
    assert_eq!(px(&p, 5, 5), (255, 0, 0, 255));
    assert_eq!(px(&p, 30, 30), (0, 0, 255, 255));
    assert_eq!(px(&p, 30, 5).3, 0);
    // `preserveAspectRatio` por defecto (xMidYMid meet): un viewBox de 10×20 en 40×40 queda
    // centrado en x ∈ [10, 30).
    let p = raster(
        &format!(
            "<svg {SVG_NS} viewBox='0 0 10 20'><rect width='10' height='20' fill='lime'/></svg>"
        ),
        40,
        40,
    );
    assert_eq!(px(&p, 5, 20).3, 0);
    assert_eq!(px(&p, 20, 20), (0, 255, 0, 255));
    assert_eq!(px(&p, 35, 20).3, 0);
}

#[test]
fn paths_strokes_and_transforms() {
    // Arco: medio círculo relleno; trazo grueso con extremos cuadrados; transformación.
    let p = raster(
        &format!(
            "<svg {SVG_NS} width='40' height='40'>\
             <path d='M0 20 A20 20 0 0 1 40 20 Z' fill='red'/>\
             <line x1='10' y1='35' x2='30' y2='35' stroke='blue' stroke-width='4' stroke-linecap='square'/>\
             <g transform='translate(30 0) scale(2)'><rect width='5' height='5' fill='lime'/></g></svg>"
        ),
        40,
        40,
    );
    assert_eq!(px(&p, 20, 10), (255, 0, 0, 255));
    assert_eq!(px(&p, 2, 2).3, 0);
    // `square` alarga 2 px el trazo por cada extremo.
    assert_eq!(px(&p, 9, 35), (0, 0, 255, 255));
    assert_eq!(px(&p, 31, 35), (0, 0, 255, 255));
    assert_eq!(px(&p, 38, 8), (0, 255, 0, 255));
}

#[test]
fn css_cascade_and_use() {
    // `<style>` gana a los atributos de presentación y `style` a `<style>`; `use` hereda.
    let p = raster(
        &format!(
            "<svg {SVG_NS} xmlns:xlink='http://www.w3.org/1999/xlink' width='30' height='10'>\
             <style>.a {{ fill: lime }} #b {{ fill: red }}</style>\
             <defs><rect id='r' width='10' height='10'/></defs>\
             <rect class='a' width='10' height='10' fill='red'/>\
             <rect id='b' x='10' width='10' height='10' style='fill: blue'/>\
             <use xlink:href='#r' x='20' fill='yellow'/></svg>"
        ),
        30,
        10,
    );
    assert_eq!(px(&p, 5, 5), (0, 255, 0, 255));
    assert_eq!(px(&p, 15, 5), (0, 0, 255, 255));
    assert_eq!(px(&p, 25, 5), (255, 255, 0, 255));
}

#[test]
fn gradients_opacity_and_clip() {
    let p = raster(
        &format!(
            "<svg {SVG_NS} width='100' height='40'>\
             <linearGradient id='g'><stop offset='0' stop-color='red'/><stop offset='1' stop-color='blue'/></linearGradient>\
             <clipPath id='c'><rect x='0' y='20' width='50' height='20'/></clipPath>\
             <rect width='100' height='20' fill='url(#g)'/>\
             <rect y='20' width='100' height='20' fill='black' opacity='0.5' clip-path='url(#c)'/></svg>"
        ),
        100,
        40,
    );
    let (r0, _, b0, _) = px(&p, 1, 10);
    let (r1, _, b1, _) = px(&p, 98, 10);
    assert!(r0 > 240 && b0 < 15, "{r0} {b0}");
    assert!(b1 > 240 && r1 < 15, "{r1} {b1}");
    let (_, _, _, a) = px(&p, 25, 30);
    assert!((126..=129).contains(&a), "{a}");
    assert_eq!(px(&p, 75, 30).3, 0, "fuera del clipPath");
}

#[test]
fn intrinsic_sizes() {
    let s = Svg::parse(&format!(
        "<svg {SVG_NS} width='18' height='18' viewBox='0 0 4 4'/>"
    ))
    .unwrap();
    assert_eq!(s.default_size(), (18.0, 18.0));
    let s = Svg::parse(&format!("<svg {SVG_NS} viewBox='0 0 20 10'/>")).unwrap();
    assert_eq!(s.intrinsic().ratio, Some(2.0));
    assert_eq!(s.default_size(), (300.0, 150.0));
    let s = Svg::parse(&format!("<svg {SVG_NS} width='2in'/>")).unwrap();
    assert_eq!(s.default_size(), (192.0, 150.0));
    assert!(Svg::parse("<html/>").is_err());
}

/// Pinta una página HTML con imágenes de `data:` (20×20 px).
fn page(html: &str) -> Pixmap {
    let (doc, _) = parse_document(html, &ParseOptions::default());
    let viewport = Size {
        width: 20.0,
        height: 20.0,
    };
    let media = MediaContext {
        width: 20.0,
        height: 20.0,
        ..MediaContext::default()
    };
    let ctx = MatchContext::for_document(&doc);
    let styles = style_document(&doc, &[], &media, &ctx);
    let base = Url::parse("http://example.test/", None).unwrap();
    let images = load_images(&doc, &styles, &base, &FileLoader::new());
    assert!(images.errors.is_empty(), "{:?}", images.errors);
    let tree = layout_document_with_images(&doc, &styles, viewport, &TestFont, &images.sizes);
    let dl = build_display_list_with_images(&doc, &tree, &TestFont, &images);
    render(&dl, viewport, 1.0, Rgba::WHITE)
}

/// Un círculo de radio 5 en un viewBox de 20×20, como data URL.
const ICON: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 20 20'%3E%3Ccircle cx='10' cy='10' r='5'/%3E%3C/svg%3E";

#[test]
fn img_with_svg() {
    let p = page(&format!(
        "<!DOCTYPE html><body style='margin:0'><img src=\"{ICON}\" width=20 height=20 style='display:block'>"
    ));
    assert_eq!(px(&p, 10, 10), (0, 0, 0, 255));
    assert_eq!(px(&p, 1, 1), (255, 255, 255, 255));
}

#[test]
fn mask_image_with_svg_icon() {
    // Como `.cdx-icon` de Wikipedia: el color del texto recortado por el icono.
    let p = page(&format!(
        "<!DOCTYPE html><body style='margin:0'><div style=\"width:20px;height:20px;\
         background-color:red;-webkit-mask-image:url(&quot;{ICON}&quot;);mask-image:url(&quot;{ICON}&quot;);\
         mask-size:20px;mask-repeat:no-repeat;mask-position:center\"></div>"
    ));
    assert_eq!(px(&p, 10, 10), (255, 0, 0, 255));
    assert_eq!(
        px(&p, 1, 1),
        (255, 255, 255, 255),
        "fuera del icono no se pinta"
    );
}

#[test]
fn inline_svg_in_html() {
    let p = page(
        "<!DOCTYPE html><style>rect { fill: lime }</style><body style='margin:0'>\
         <svg width=20 height=20 style='display:block'><rect width=10 height=20 fill=red /></svg>",
    );
    assert_eq!(px(&p, 5, 10), (0, 255, 0, 255));
    assert_eq!(px(&p, 15, 10), (255, 255, 255, 255));
}

#[test]
fn markers_on_vertices() {
    // Un cuadrado de 4×4 centrado en cada extremo de la línea; el del final, girado con ella
    // (orient=auto) sigue cubriendo el mismo punto.
    let p = raster(
        &format!(
            "<svg {SVG_NS} width='40' height='40'>\
             <marker id='m' markerWidth='4' markerHeight='4' refX='2' refY='2' orient='auto' markerUnits='userSpaceOnUse'>\
             <rect width='4' height='4' fill='red'/></marker>\
             <polyline points='5,5 35,5 35,35' fill='none' stroke='blue' stroke-width='0.5'\
              marker-start='url(#m)' marker-mid='url(#m)' marker-end='url(#m)'/></svg>"
        ),
        40,
        40,
    );
    for (x, y) in [(5, 5), (35, 5), (35, 35)] {
        assert_eq!(px(&p, x, y), (255, 0, 0, 255), "({x}, {y})");
    }
    assert_eq!(px(&p, 20, 20).3, 0);
}

/// Pinta un SVG de 100×100 con Ahem (cada glifo, un cuadrado de 1em con 0,8em sobre la línea
/// base); `None` sin `tests/wpt/fonts/Ahem.ttf`.
fn raster_ahem(body: &str) -> Option<Pixmap> {
    let ahem =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/wpt/fonts/Ahem.ttf");
    if !ahem.is_file() {
        eprintln!("aviso: falta {}; se salta", ahem.display());
        return None;
    }
    let db = std::sync::Arc::new(bf_font::FontDb::scan_with(&[], &[ahem]));
    let fonts = bf_font::Fonts::new(db);
    let src = format!(
        "<svg {SVG_NS} width='100' height='100' font-family='Ahem' font-size='20'>{body}</svg>"
    );
    let svg = Svg::parse(&src).expect("SVG válido");
    rasterize_svg(&svg.tree(100.0, 100.0), 100, 100, Some(&fonts))
}

/// ¿Está pintado (opaco) el píxel?
fn ink(p: &Pixmap, x: u32, y: u32) -> bool {
    px(p, x, y).3 == 255
}

#[test]
fn text_position_lists_rotate_and_spacing() {
    // Listas de `x`: cada valor abre un chunk.
    let Some(p) = raster_ahem("<text x='0 50' y='20'>XX</text>") else {
        return;
    };
    assert!(ink(&p, 10, 10) && ink(&p, 60, 10));
    assert!(!ink(&p, 30, 10));
    // `dx`/`dy` por carácter.
    let p = raster_ahem("<text y='20' dx='0 30' dy='0 40'>XX</text>").unwrap();
    assert!(ink(&p, 10, 10) && ink(&p, 60, 50));
    assert!(!ink(&p, 30, 10) && !ink(&p, 60, 10));
    // `rotate` gira el glifo alrededor de su origen (90°: la línea base hacia abajo).
    let p = raster_ahem("<text x='20' y='20' rotate='90'>X</text>").unwrap();
    assert!(ink(&p, 30, 30) && !ink(&p, 30, 10));
    // `letter-spacing` detrás de cada carácter.
    let p = raster_ahem("<text y='20' letter-spacing='10'>XX</text>").unwrap();
    assert!(ink(&p, 40, 10) && !ink(&p, 25, 10));
}

#[test]
fn text_dominant_baseline() {
    // `text-before-edge`: la posición es el borde del ascent; `text-after-edge`, el del
    // descent; `central`, el centro de la caja.
    let Some(p) = raster_ahem("<text y='0' dominant-baseline='text-before-edge'>X</text>") else {
        return;
    };
    assert!(ink(&p, 10, 1) && ink(&p, 10, 19) && !ink(&p, 10, 21));
    let p = raster_ahem("<text y='20' dominant-baseline='text-after-edge'>X</text>").unwrap();
    assert!(ink(&p, 10, 1) && ink(&p, 10, 19) && !ink(&p, 10, 21));
    let p = raster_ahem("<text y='10' dominant-baseline='central'>X</text>").unwrap();
    assert!(ink(&p, 10, 1) && ink(&p, 10, 19) && !ink(&p, 10, 21));
}

#[test]
fn text_on_a_path() {
    // Un camino vertical hacia abajo: los glifos van girados 90° a lo largo de él.
    let Some(p) =
        raster_ahem("<path id='p' d='M10,0 V100'/><text><textPath href='#p'>XX</textPath></text>")
    else {
        return;
    };
    assert!(ink(&p, 16, 10) && ink(&p, 16, 30));
    assert!(!ink(&p, 16, 50) && !ink(&p, 3, 10) && !ink(&p, 40, 10));
    // El glifo cuyo punto medio cae fuera del camino no se pinta.
    let p =
        raster_ahem("<path id='p' d='M10,0 V25'/><text><textPath href='#p'>XX</textPath></text>")
            .unwrap();
    assert!(ink(&p, 16, 10) && !ink(&p, 16, 22));
    // `startOffset`, `text-anchor` y lo que sigue al camino (empieza en su final).
    let p = raster_ahem(
        "<path id='p' d='M0,20 H100'/><text><textPath href='#p' startOffset='50%' \
         text-anchor='middle'>XX</textPath><tspan fill='blue'>X</tspan></text>",
    )
    .unwrap();
    assert!(ink(&p, 35, 10) && ink(&p, 65, 10) && !ink(&p, 25, 10) && !ink(&p, 75, 10));
    assert!(!ink(&p, 95, 10));
    // `side="right"`: el camino al revés (de derecha a izquierda, los glifos cabeza abajo).
    let p = raster_ahem(
        "<path id='p' d='M0,50 H100'/><text><textPath href='#p' side='right'>X</textPath></text>",
    )
    .unwrap();
    assert!(ink(&p, 90, 55) && !ink(&p, 90, 45) && !ink(&p, 10, 55));
}

/// Todos los bytes en `%XX` (para meterlos en una URL `data:` dentro de un atributo).
fn pct(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("%{b:02X}")).collect()
}

/// BMP de 2×1 a 24 bits: rojo a la izquierda, azul a la derecha.
fn bmp_red_blue() -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(b"BM");
    b.extend_from_slice(&(14u32 + 40 + 8).to_le_bytes());
    b.extend_from_slice(&[0, 0, 0, 0]);
    b.extend_from_slice(&54u32.to_le_bytes());
    b.extend_from_slice(&40u32.to_le_bytes());
    b.extend_from_slice(&2i32.to_le_bytes());
    b.extend_from_slice(&1i32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&24u16.to_le_bytes());
    b.extend_from_slice(&[0; 24]);
    // Una fila (BGR), rellena a 4 bytes.
    b.extend_from_slice(&[0, 0, 255, 255, 0, 0, 0, 0]);
    b
}

/// Elemento `image` (SVG 2 §12.3) con URLs `data:`: un bitmap estirado con
/// `preserveAspectRatio="none"`, un SVG (por `xlink:href`) con su viewport en el rectángulo
/// del elemento, y `meet` centrando un bitmap.
#[test]
fn image_element_with_data_urls() {
    let inner = pct(format!(
        "<svg {SVG_NS} viewBox='0 0 10 10'><rect width='10' height='10' fill='lime'/></svg>"
    )
    .as_bytes());
    let bmp = pct(&bmp_red_blue());
    let src = format!(
        "<svg {SVG_NS} xmlns:xlink='http://www.w3.org/1999/xlink' width='60' height='20'>\
         <image href='data:image/bmp,{bmp}' width='20' height='10' preserveAspectRatio='none'/>\
         <image xlink:href='data:image/svg+xml,{inner}' x='20' width='20' height='20' preserveAspectRatio='none'/>\
         <image href='data:image/bmp,{bmp}' x='40' width='20' height='20'/>\
         </svg>"
    );
    let svg = Svg::parse(&src).expect("SVG válido");
    assert_eq!(svg.image_hrefs().len(), 2);
    let mut res = SvgResources::default();
    res.load(&svg, None, None);
    let p = rasterize_svg_with(&svg.tree(60.0, 20.0), 60, 20, None, &res).expect("bitmap");
    assert_eq!(px(&p, 2, 5), (255, 0, 0, 255));
    assert_eq!(px(&p, 17, 5), (0, 0, 255, 255));
    assert_eq!(px(&p, 5, 15).3, 0);
    assert_eq!(px(&p, 30, 15), (0, 255, 0, 255));
    // `xMidYMid meet`: 2×1 en 20×20 ocupa y ∈ [5, 15).
    assert_eq!(px(&p, 50, 2).3, 0);
    assert_eq!(px(&p, 42, 10), (255, 0, 0, 255));
    assert_eq!(px(&p, 50, 18).3, 0);
    // Sin recursos no se pinta nada.
    let empty = rasterize_svg(&svg.tree(60.0, 20.0), 60, 20, None).expect("bitmap");
    assert_eq!(px(&empty, 2, 5).3, 0);
}

/// `foreignObject` en un SVG en línea (SVG 2 §23.2): su HTML se maqueta en la caja del
/// elemento (aquí un `div` de 10×10 en (20, 10)), recortado a ella; fuera queda el SVG.
#[test]
fn foreign_object_html_content() {
    let (doc, _) = parse_document(
        "<!DOCTYPE html><style>.box { width: 10px; height: 30px; background: rgb(0, 0, 255) }</style>\
         <body style='margin:0'><svg width=40 height=40 style='display:block'>\
         <rect width=40 height=40 fill=lime />\
         <foreignObject x=20 y=10 width=10 height=10><div class=box></div></foreignObject>\
         </svg>",
        &ParseOptions::default(),
    );
    let base = Url::parse("about:blank", None).unwrap();
    let ctx = bf_paint::PageContext {
        base: &base,
        encoding: bf_infra::encoding::Encoding::Utf8,
        loader: std::sync::Arc::new(FileLoader::new()),
        fonts: std::sync::Arc::new(bf_font::FontDb::from_faces(Vec::new())),
        extra: &[],
        text_zoom: 1.0,
    };
    let viewport = Size {
        width: 40.0,
        height: 40.0,
    };
    let p = bf_paint::render_document(&doc, &ctx, viewport, 1.0);
    assert_eq!(px(&p, 25, 15), (0, 0, 255, 255));
    // Recortado al alto del elemento (el `div` mide 30).
    assert_eq!(px(&p, 25, 25), (0, 255, 0, 255));
    assert_eq!(px(&p, 15, 15), (0, 255, 0, 255));
}

/// Filter Effects 1 §9.13: `feFlood` llena la región del filtro (por defecto, la caja con un
/// 10 % de margen) y nada fuera de ella.
#[test]
fn filter_flood_fills_region() {
    let p = raster(
        &format!(
            "<svg {SVG_NS} width='100' height='100'>\
             <filter id='f'><feFlood flood-color='lime'/></filter>\
             <rect x='10' y='10' width='50' height='50' fill='red' filter='url(#f)'/></svg>"
        ),
        100,
        100,
    );
    assert_eq!(px(&p, 6, 6), (0, 255, 0, 255));
    assert_eq!(px(&p, 63, 63), (0, 255, 0, 255));
    assert_eq!(px(&p, 3, 3).3, 0);
    assert_eq!(px(&p, 67, 67).3, 0);
    // Una referencia a un filtro que no existe se ignora (§5): el elemento se pinta tal cual.
    let p = raster(
        &format!(
            "<svg {SVG_NS} width='100' height='100'>\
             <rect x='10' y='10' width='50' height='50' fill='red' filter='url(#nada)'/></svg>"
        ),
        100,
        100,
    );
    assert_eq!(px(&p, 30, 30), (255, 0, 0, 255));
}

/// §9.18: `feOffset` desplaza en espacio de usuario (aquí escalado ×2 por el `viewBox`).
#[test]
fn filter_offset_moves_content() {
    let p = raster(
        &format!(
            "<svg {SVG_NS} viewBox='0 0 50 50'>\
             <filter id='f' filterUnits='userSpaceOnUse' x='0' y='0' width='50' height='50'>\
             <feOffset dx='20' dy='5'/></filter>\
             <rect width='10' height='10' fill='blue' filter='url(#f)'/></svg>"
        ),
        100,
        100,
    );
    assert_eq!(px(&p, 5, 5).3, 0);
    assert_eq!(px(&p, 45, 15), (0, 0, 255, 255));
    assert_eq!(px(&p, 61, 15).3, 0);
}

/// §9.14: el desenfoque suaviza el borde (alfa intermedio en el borde, opaco en el centro).
#[test]
fn filter_blur_softens_edges() {
    let p = raster(
        &format!(
            "<svg {SVG_NS} width='100' height='100'>\
             <filter id='f' filterUnits='userSpaceOnUse' x='0' y='0' width='100' height='100'>\
             <feGaussianBlur stdDeviation='4'/></filter>\
             <rect x='25' y='25' width='50' height='50' fill='black' filter='url(#f)'/></svg>"
        ),
        100,
        100,
    );
    let edge = px(&p, 25, 50).3;
    assert!((100..=155).contains(&edge), "alfa en el borde: {edge}");
    assert!(px(&p, 50, 50).3 >= 250);
    let outside = px(&p, 20, 50).3;
    assert!(outside > 0 && outside < edge, "alfa fuera: {outside}");
    assert_eq!(px(&p, 5, 50).3, 0);
}

/// §9.6 y §10: `saturate` 0 en sRGB da la luminancia del rojo (0,213 · 255 ≈ 54); en
/// linearRGB (el valor inicial), el mismo gris convertido de vuelta a sRGB (≈ 127).
#[test]
fn filter_color_matrix_and_color_space() {
    let doc = |space: &str| {
        format!(
            "<svg {SVG_NS} width='20' height='20'>\
             <filter id='f' color-interpolation-filters='{space}'>\
             <feColorMatrix type='saturate' values='0'/></filter>\
             <rect width='20' height='20' fill='red' filter='url(#f)'/></svg>"
        )
    };
    let (r, g, b, a) = px(&raster(&doc("sRGB"), 20, 20), 10, 10);
    assert_eq!(a, 255);
    assert!(r.abs_diff(54) <= 2 && r == g && g == b, "{r} {g} {b}");
    let (r, g, b, _) = px(&raster(&doc("linearRGB"), 20, 20), 10, 10);
    assert!(r.abs_diff(127) <= 3 && r == g && g == b, "{r} {g} {b}");
}

/// `feMerge` con una sombra (`SourceAlpha` desplazada) bajo el gráfico, y `feComposite`
/// `in` de un `feFlood` con la forma.
#[test]
fn filter_merge_and_composite() {
    let p = raster(
        &format!(
            "<svg {SVG_NS} width='100' height='100'>\
             <filter id='f' filterUnits='userSpaceOnUse' x='0' y='0' width='100' height='100'>\
             <feOffset in='SourceAlpha' dx='10' dy='10' result='s'/>\
             <feMerge><feMergeNode in='s'/><feMergeNode in='SourceGraphic'/></feMerge>\
             </filter>\
             <rect x='10' y='10' width='30' height='30' fill='lime' filter='url(#f)'/></svg>"
        ),
        100,
        100,
    );
    assert_eq!(px(&p, 20, 20), (0, 255, 0, 255));
    assert_eq!(px(&p, 45, 45), (0, 0, 0, 255));
    assert_eq!(px(&p, 55, 55).3, 0);
    let p = raster(
        &format!(
            "<svg {SVG_NS} width='100' height='100'>\
             <filter id='f' filterUnits='userSpaceOnUse' x='0' y='0' width='100' height='100'>\
             <feFlood flood-color='blue'/><feComposite in2='SourceGraphic' operator='in'/>\
             </filter>\
             <circle cx='50' cy='50' r='20' fill='red' filter='url(#f)'/></svg>"
        ),
        100,
        100,
    );
    assert_eq!(px(&p, 50, 50), (0, 0, 255, 255));
    assert_eq!(px(&p, 10, 10).3, 0);
}
