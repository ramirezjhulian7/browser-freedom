//! SVG como imagen (E7): rasterizado de `bf-svg` al tamaño pedido, `<img>` con SVG,
//! `mask-image` con un icono SVG (como los de Wikipedia) y SVG en línea en HTML.

use bf_css::media::MediaContext;
use bf_css::selectors::MatchContext;
use bf_html::{ParseOptions, parse_document};
use bf_infra::{FileLoader, Url};
use bf_layout::{Size, TestFont, layout_document_with_images};
use bf_paint::images::rasterize_svg;
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
