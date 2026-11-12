//! E7: `border-image` (CSS Backgrounds 3 §6), `filter` (Filter Effects 1 §8), `clip-path`
//! con formas básicas (CSS Masking 1 §5) y `mix-blend-mode` (Compositing and Blending 1 §3),
//! de punta a punta (HTML → estilo → layout → lista → píxeles).

use bf_css::media::MediaContext;
use bf_css::selectors::MatchContext;
use bf_html::{ParseOptions, parse_document};
use bf_layout::{Size, TestFont, layout_document};
use bf_paint::{Pixmap, build_display_list, render};
use bf_style::style_document;
use bf_style::values::color::Rgba;

const VIEWPORT: Size = Size {
    width: 200.0,
    height: 200.0,
};

fn paint(body: &str) -> Pixmap {
    let html = format!("<!DOCTYPE html><style>body{{margin:0}}</style>{body}");
    let (doc, _) = parse_document(&html, &ParseOptions::default());
    let media = MediaContext {
        width: f64::from(VIEWPORT.width),
        height: f64::from(VIEWPORT.height),
        ..MediaContext::default()
    };
    let ctx = MatchContext::for_document(&doc);
    let styles = style_document(&doc, &[], &media, &ctx);
    let tree = layout_document(&doc, &styles, VIEWPORT, &TestFont);
    let dl = build_display_list(&doc, &tree, &TestFont);
    render(&dl, VIEWPORT, 1.0, Rgba::WHITE)
}

fn px(p: &Pixmap, x: u32, y: u32) -> [u8; 4] {
    let c = p.pixel(x, y).expect("dentro del pixmap");
    [c.red(), c.green(), c.blue(), c.alpha()]
}

fn near(a: [u8; 4], b: [u8; 4], tol: u8) -> bool {
    a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= tol)
}

const WHITE: [u8; 4] = [255, 255, 255, 255];
const GREEN: [u8; 4] = [0, 128, 0, 255];

#[track_caller]
fn check(p: &Pixmap, x: u32, y: u32, want: [u8; 4], tol: u8) {
    let got = px(p, x, y);
    assert!(near(got, want, tol), "({x}, {y}): {got:?} ≠ {want:?}");
}

#[test]
fn border_image_reemplaza_el_borde() {
    let p = paint(
        "<div style='width:60px;height:60px;border:10px solid red;\
         border-image:linear-gradient(green, green) 10'></div>",
    );
    check(&p, 5, 40, GREEN, 2);
    check(&p, 75, 75, GREEN, 2);
    // Sin `fill`, el centro no se pinta.
    check(&p, 40, 40, WHITE, 0);
    let p = paint(
        "<div style='width:60px;height:60px;border:10px solid red;\
         border-image:linear-gradient(green, green) 10 fill'></div>",
    );
    check(&p, 40, 40, GREEN, 2);
}

#[test]
fn border_image_outset_y_ancho() {
    // El área crece 10 px por fuera y el ancho de la imagen es 20 px aunque el borde mida 5.
    let p = paint(
        "<div style='margin:20px;width:60px;height:60px;border:5px solid red;\
         border-image:linear-gradient(green, green) 10 / 20px / 10px'></div>",
    );
    check(&p, 12, 50, GREEN, 2);
    check(&p, 28, 50, GREEN, 2);
    check(&p, 50, 50, WHITE, 0);
    check(&p, 5, 50, WHITE, 0);
}

#[test]
fn border_shorthand_reinicia_border_image() {
    let p = paint(
        "<div style='width:60px;height:60px;border-image:linear-gradient(green, green) 10;\
         border:10px solid blue'></div>",
    );
    check(&p, 5, 40, [0, 0, 255, 255], 0);
}

#[test]
fn filtros_de_color() {
    let p = paint("<div style='width:50px;height:50px;background:red;filter:invert(1)'></div>");
    check(&p, 25, 25, [0, 255, 255, 255], 2);
    let p = paint("<div style='width:50px;height:50px;background:red;filter:opacity(0.5)'></div>");
    check(&p, 25, 25, [255, 128, 128, 255], 2);
    let p = paint("<div style='width:50px;height:50px;background:red;filter:grayscale(1)'></div>");
    let c = px(&p, 25, 25);
    assert!(
        c[0] == c[1] && c[1] == c[2] && c[0] > 40 && c[0] < 70,
        "{c:?}"
    );
    // `brightness(0)` deja negro y `hue-rotate(180deg)` de rojo da un cian oscuro.
    let p = paint("<div style='width:50px;height:50px;background:red;filter:brightness(0)'></div>");
    check(&p, 25, 25, [0, 0, 0, 255], 0);
}

#[test]
fn desenfoque_y_sombra() {
    let p = paint(
        "<div style='margin:50px;width:50px;height:50px;background:black;filter:blur(5px)'></div>",
    );
    // El borde queda a medio camino; fuera del cuadrado aparece algo de negro.
    let edge = px(&p, 50, 75);
    assert!(edge[0] > 90 && edge[0] < 170, "{edge:?}");
    let out = px(&p, 46, 75);
    assert!(out[0] < 250 && out[0] > 128, "{out:?}");
    check(&p, 75, 75, [0, 0, 0, 255], 2);
    let p = paint(
        "<div style='width:50px;height:50px;background:green;\
         filter:drop-shadow(20px 20px blue)'></div>",
    );
    check(&p, 25, 25, GREEN, 0);
    check(&p, 60, 60, [0, 0, 255, 255], 0);
    check(&p, 5, 60, WHITE, 0);
}

#[test]
fn clip_path_con_formas() {
    let p = paint(
        "<div style='width:100px;height:100px;background:green;clip-path:circle(50%)'></div>",
    );
    check(&p, 50, 50, GREEN, 0);
    check(&p, 3, 3, WHITE, 0);
    let p = paint(
        "<div style='width:100px;height:100px;background:green;\
         clip-path:inset(10px 20px)'></div>",
    );
    check(&p, 50, 5, WHITE, 0);
    check(&p, 15, 50, WHITE, 0);
    check(&p, 25, 15, GREEN, 0);
    let p = paint(
        "<div style='width:100px;height:100px;background:green;\
         clip-path:polygon(0 0, 100% 0, 0 100%)'></div>",
    );
    check(&p, 10, 10, GREEN, 0);
    check(&p, 90, 90, WHITE, 0);
    let p = paint(
        "<div style='width:100px;height:100px;background:green;\
         clip-path:path(\"M 0 0 L 50 0 L 50 50 Z\")'></div>",
    );
    check(&p, 40, 10, GREEN, 0);
    check(&p, 10, 40, WHITE, 0);
    check(&p, 80, 80, WHITE, 0);
    // Una caja sola recorta a esa caja.
    let p = paint(
        "<div style='width:80px;height:80px;padding:10px;background:green;\
         clip-path:content-box'></div>",
    );
    check(&p, 5, 50, WHITE, 0);
    check(&p, 50, 50, GREEN, 0);
}

#[test]
fn mezcla_con_lo_de_debajo() {
    let p = paint(
        "<div style='width:100px;height:100px;background:blue'>\
         <div style='width:50px;height:50px;background:yellow;mix-blend-mode:multiply'></div>\
         </div>",
    );
    check(&p, 25, 25, [0, 0, 0, 255], 0);
    check(&p, 75, 75, [0, 0, 255, 255], 0);
    let p = paint(
        "<div style='width:50px;height:50px;background:red;mix-blend-mode:difference'></div>",
    );
    check(&p, 25, 25, [0, 255, 255, 255], 0);
    // `isolation: isolate` en el padre: la mezcla no ve el fondo azul de fuera.
    let p = paint(
        "<div style='width:100px;height:100px;background:blue'>\
         <div style='isolation:isolate'>\
         <div style='width:50px;height:50px;background:yellow;mix-blend-mode:multiply'></div>\
         </div></div>",
    );
    check(&p, 25, 25, [255, 255, 0, 255], 0);
}
