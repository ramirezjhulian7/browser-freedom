//! E4: gradientes (CSS Images 3 §3), `box-shadow` (CSS Backgrounds 3 §7) y `outline` (CSS UI 4
//! §5) rasterizados de punta a punta (HTML → estilo → layout → lista → píxeles).

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

fn paint(html: &str) -> Pixmap {
    let (doc, _) = parse_document(html, &ParseOptions::default());
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

#[test]
fn gradiente_lineal_de_rojo_a_azul() {
    let p = paint(
        "<!DOCTYPE html><style>body{margin:0}</style>\
         <div style='width:100px;height:100px;background:linear-gradient(red, blue)'></div>",
    );
    assert!(
        near(px(&p, 50, 0), [255, 0, 0, 255], 4),
        "{:?}",
        px(&p, 50, 0)
    );
    assert!(
        near(px(&p, 50, 99), [0, 0, 255, 255], 4),
        "{:?}",
        px(&p, 50, 99)
    );
    // En el centro, morado (interpolación en sRGB).
    assert!(
        near(px(&p, 50, 50), [126, 0, 129, 255], 4),
        "{:?}",
        px(&p, 50, 50)
    );
}

#[test]
fn gradiente_con_transparent_interpola_premultiplicado() {
    // `transparent` es negro transparente: sin premultiplicar el centro sería grisáceo.
    let p = paint(
        "<!DOCTYPE html><style>body{margin:0}</style>\
         <div style='width:100px;height:100px;background:linear-gradient(to right, red, transparent)'></div>",
    );
    let c = px(&p, 50, 50);
    assert!(c[0] == 255 && c[1] > 120 && c[1] == c[2], "{c:?}");
}

#[test]
fn gradiente_repetido_y_paradas_duras() {
    let p = paint(
        "<!DOCTYPE html><style>body{margin:0}</style>\
         <div style='width:100px;height:20px;background:repeating-linear-gradient(to right, lime 0 10px, blue 10px 20px)'></div>",
    );
    assert_eq!(px(&p, 5, 10), [0, 255, 0, 255]);
    assert_eq!(px(&p, 15, 10), [0, 0, 255, 255]);
    assert_eq!(px(&p, 85, 10), [0, 255, 0, 255]);
    assert_eq!(px(&p, 95, 10), [0, 0, 255, 255]);
}

#[test]
fn gradiente_radial_y_conico() {
    let p = paint(
        "<!DOCTYPE html><style>body{margin:0} div{width:100px;height:100px}</style>\
         <div style='background:radial-gradient(circle closest-side, lime 50%, blue 50%)'></div>\
         <div style='background:conic-gradient(lime 25%, blue 25%)'></div>",
    );
    assert_eq!(px(&p, 50, 50), [0, 255, 0, 255]);
    assert_eq!(px(&p, 5, 5), [0, 0, 255, 255]);
    // Cónico (centro en (50, 150)): el primer cuarto, arriba a la derecha, es verde.
    assert_eq!(px(&p, 75, 125), [0, 255, 0, 255]);
    assert_eq!(px(&p, 75, 175), [0, 0, 255, 255]);
    assert_eq!(px(&p, 25, 125), [0, 0, 255, 255]);
}

#[test]
fn sombra_sin_desenfoque_cubre_el_rectangulo_desplazado() {
    let p = paint(
        "<!DOCTYPE html><style>body{margin:0}</style>\
         <div style='margin:20px;width:40px;height:40px;box-shadow:10px 30px black'></div>",
    );
    // La sombra ocupa (30..70, 50..90), fuera de la caja (20..60, 20..60).
    assert_eq!(px(&p, 65, 85), [0, 0, 0, 255]);
    assert_eq!(px(&p, 30, 89), [0, 0, 0, 255]);
    assert_eq!(px(&p, 69, 61), [0, 0, 0, 255]);
    assert_eq!(px(&p, 70, 85), [255, 255, 255, 255]);
    assert_eq!(px(&p, 65, 90), [255, 255, 255, 255]);
    assert_eq!(px(&p, 29, 85), [255, 255, 255, 255]);
    // Debajo de la caja (transparente) no se pinta.
    assert_eq!(px(&p, 40, 55), [255, 255, 255, 255]);
}

#[test]
fn sombra_desenfocada_e_interior() {
    let p = paint(
        "<!DOCTYPE html><style>body{margin:0}</style>\
         <div style='margin:50px;width:50px;height:50px;box-shadow:0 0 20px black'></div>\
         <div style='margin:0 50px;width:50px;height:50px;box-shadow:inset 0 10px blue'></div>",
    );
    // Fuera de la caja, la sombra se desvanece con la distancia.
    let near_edge = px(&p, 102, 75)[0];
    let far = px(&p, 115, 75)[0];
    assert!(near_edge < far && far < 255, "{near_edge} {far}");
    // Interior: banda azul de 10px arriba, dentro de la padding box.
    assert_eq!(px(&p, 75, 155), [0, 0, 255, 255]);
    assert_eq!(px(&p, 75, 165), [255, 255, 255, 255]);
}

#[test]
fn outline_con_offset_fuera_del_borde() {
    let p = paint(
        "<!DOCTYPE html><style>body{margin:0}</style>\
         <div style='margin:30px;width:40px;height:40px;outline:5px solid lime;outline-offset:5px'></div>",
    );
    // Caja 30..70; offset 5 → outline de 20..25 y 75..80.
    assert_eq!(px(&p, 22, 50), [0, 255, 0, 255]);
    assert_eq!(px(&p, 77, 50), [0, 255, 0, 255]);
    assert_eq!(px(&p, 50, 22), [0, 255, 0, 255]);
    assert_eq!(px(&p, 27, 50), [255, 255, 255, 255]);
    assert_eq!(px(&p, 18, 50), [255, 255, 255, 255]);
}

#[test]
fn outline_se_pinta_sobre_los_hermanos_siguientes() {
    // Apéndice E, paso 10: el outline va al final del stacking context.
    let p = paint(
        "<!DOCTYPE html><style>body{margin:0}</style>\
         <div style='width:40px;height:40px;outline:10px solid lime'></div>\
         <div style='width:40px;height:40px;background:blue'></div>",
    );
    assert_eq!(px(&p, 20, 45), [0, 255, 0, 255]);
}

#[test]
fn grupo_de_sombra_desenfoca_y_desplaza() {
    use bf_layout::fragment::Rect;
    use bf_paint::{DisplayItem, DisplayList, Radii};
    let mut dl = DisplayList::default();
    dl.push(DisplayItem::PushShadow {
        bounds: Rect::new(60.0, 50.0, 40.0, 40.0),
        blur: 8.0,
    });
    dl.push(DisplayItem::PushTransform { dx: 10.0, dy: 0.0 });
    dl.push(DisplayItem::Rect {
        rect: Rect::new(50.0, 50.0, 40.0, 40.0),
        color: Rgba::BLACK,
        radii: Radii::default(),
    });
    dl.push(DisplayItem::PopTransform);
    dl.push(DisplayItem::PopShadow);
    let p = render(&dl, VIEWPORT, 1.0, Rgba::WHITE);
    // Centro desplazado: opaco; borde: a medias; lejos: intacto.
    assert!(px(&p, 80, 70)[0] < 10, "{:?}", px(&p, 80, 70));
    let edge = px(&p, 60, 70)[0];
    assert!(edge > 90 && edge < 165, "{edge}");
    assert_eq!(px(&p, 40, 70), [255, 255, 255, 255]);
}
