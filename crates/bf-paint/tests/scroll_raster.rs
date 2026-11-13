//! E4: el raster en tiles de la página (`raster::render_tiled`), en uno o varios hilos, y el
//! desplazamiento incremental (`raster::scroll_frame`) dan exactamente los mismos píxeles que
//! el raster en tiles entero en la misma posición: con elementos `fixed` y `sticky`, un scroll
//! container anidado, capas con opacidad, sombras desenfocadas, texto y transformaciones.
//! Contra el raster de una pasada (`render_scrolled`) solo cambia el antialiasing de las curvas
//! que cruzan el borde de un tile; el último test lo acota.

use std::sync::Arc;

use bf_font::FontDb;
use bf_html::{ParseOptions, parse_document};
use bf_infra::Url;
use bf_infra::encoding::Encoding;
use bf_layout::Size;
use bf_paint::raster::{render_tiled, scroll_frame, set_raster_threads};
use bf_paint::{PageContext, Pixmap, prepare_page_with_frames, render_scrolled};
use bf_style::values::color::Rgba;

const VIEWPORT: Size = Size {
    width: 320.0,
    height: 240.0,
};

/// Una página larga con todo lo que el scroll trata aparte.
const PAGE: &str = "<!doctype html><style>
  body { margin: 0; font: 16px sans-serif; }
  .fixed { position: fixed; top: 0; left: 0; right: 0; height: 30px; background: #258;
           color: white; box-shadow: 0 2px 6px rgba(0,0,0,.5); }
  .sticky { position: sticky; top: 40px; height: 24px; background: #fc3; opacity: .8; }
  .scroller { height: 120px; overflow: auto; border: 2px solid #333; margin: 10px; }
  .scroller div { height: 40px; background: linear-gradient(#f88, #88f); margin: 4px; }
  .faded { opacity: .5; background: #3a3; height: 60px; margin: 8px; border-radius: 12px; }
  .shadow { text-shadow: 2px 2px 3px #000; box-shadow: 4px 4px 10px #555; margin: 16px;
            padding: 8px; }
  p { margin: 6px 12px; }
</style>
<div class=fixed>Arriba fijo</div>
<div style='height: 50px'></div>
<div class=sticky>Pegajoso</div>
<div class=scroller><div></div><div></div><div></div><div></div></div>
<div class=faded>capa con opacidad</div>
<div class=shadow>Texto con sombra desenfocada</div>
<p>Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor.</p>
<p>Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip.</p>
<div class=faded style='transform: scale(1.1) translate(5px, 3px)'>escalada</div>
<div class=faded style='transform: rotate(3deg)'>girada</div>
<div style='width: 200px; height: 200px; border-radius: 50%;
            background: radial-gradient(#f00, #00f)'></div>
<div style='height: 400px; background: repeating-linear-gradient(45deg, #eee 0 10px, #ccc 10px 20px)'></div>
<p>Final de la página.</p>";

fn page(html: &str) -> bf_paint::Page {
    let (doc, _) = parse_document(html, &ParseOptions::default());
    let base = Url::parse("about:blank", None).unwrap();
    let ctx = PageContext {
        base: &base,
        encoding: Encoding::Utf8,
        loader: Arc::new(bf_infra::FileLoader::new()),
        fonts: FontDb::system(),
        extra: &[],
        text_zoom: 1.0,
        device_pixel_ratio: 1.0,
    };
    prepare_page_with_frames(&doc, &ctx, VIEWPORT, &[], &[])
}

/// Dónde difieren dos cuadros (para el mensaje del test).
fn diff(a: &Pixmap, b: &Pixmap) -> Option<String> {
    if a.width() != b.width() || a.height() != b.height() {
        return Some(format!(
            "tamaños {}x{} y {}x{}",
            a.width(),
            a.height(),
            b.width(),
            b.height()
        ));
    }
    let w = a.width() as usize;
    let bad: Vec<usize> = a
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.data().as_chunks::<4>().0)
        .enumerate()
        .filter(|(_, (x, y))| x != y)
        .map(|(i, _)| i)
        .collect();
    let first = bad.first()?;
    Some(format!(
        "{} píxeles distintos; el primero en ({}, {})",
        bad.len(),
        first % w,
        first / w
    ))
}

fn tiled(p: &bf_paint::Page, scale: f32, scroll: (f32, f32), threads: usize) -> Pixmap {
    set_raster_threads(threads);
    render_tiled(&p.display_list, VIEWPORT, scale, Rgba::WHITE, scroll)
        .unwrap_or_else(|| panic!("sin raster en tiles en {scroll:?} a escala {scale}"))
}

#[test]
fn incremental_scroll_matches_tiled_raster() {
    let p = page(PAGE);
    for scale in [1.0f32, 2.0] {
        let mut scroll = (0.0f32, 0.0f32);
        let mut frame = tiled(&p, scale, scroll, 1);
        set_raster_threads(3);
        // Hacia abajo y hacia arriba, con pasos de varios tamaños.
        for dy in [
            7.0, 40.0, 23.5, 120.0, 1.0, -33.0, -80.0, 65.0, -12.5, 200.0, -64.0,
        ] {
            let next = (0.0, (scroll.1 + dy).max(0.0));
            // A escala 1 medio px CSS no es un px de dispositivo entero: no hay tiles, y desde
            // un cuadro así no hay incremental.
            if device_fraction(next.1, scale) {
                assert!(
                    render_tiled(&p.display_list, VIEWPORT, scale, Rgba::WHITE, next).is_none()
                );
                frame = render_scrolled(&p.display_list, VIEWPORT, scale, Rgba::WHITE, next);
                scroll = next;
                continue;
            }
            let full = tiled(&p, scale, next, 1);
            let fast = scroll_frame(
                &p.display_list,
                &frame,
                scroll,
                VIEWPORT,
                scale,
                Rgba::WHITE,
                next,
            );
            if device_fraction(scroll.1, scale) || (next.1 - scroll.1).abs() >= VIEWPORT.height {
                assert!(fast.is_none(), "{scroll:?} → {next:?}");
            } else {
                let fast = fast.unwrap_or_else(|| panic!("sin incremental {scroll:?} → {next:?}"));
                if let Some(d) = diff(&fast, &full) {
                    panic!("scroll {next:?} a escala {scale}: {d}");
                }
            }
            frame = full;
            scroll = next;
        }
    }
}

fn device_fraction(v: f32, scale: f32) -> bool {
    (v * scale).fract() != 0.0
}

#[test]
fn parallel_tiles_match_sequential_tiles() {
    let pages = [
        PAGE,
        "<!doctype html><body style='margin:0'><h1>Título</h1><p style='columns: 2'>uno dos \
         tres cuatro cinco seis siete ocho nueve diez once doce trece catorce</p>\
         <div style='width: 200px; height: 200px; border-radius: 50%; \
         background: radial-gradient(#f00, #00f)'></div>",
    ];
    for html in pages {
        let p = page(html);
        for scale in [1.0f32, 2.0] {
            for scroll in [(0.0, 0.0), (0.0, 37.0)] {
                let one = tiled(&p, scale, scroll, 1);
                for threads in [2, 3, 4, 8] {
                    let many = tiled(&p, scale, scroll, threads);
                    if let Some(d) = diff(&many, &one) {
                        panic!("{threads} hilos, escala {scale}, scroll {scroll:?}: {d}");
                    }
                }
            }
        }
    }
}

/// Contra el raster de una pasada: lo que no son curvas que cruzan el borde de un tile sale
/// igual, y donde cambia es solo antialiasing (pocos píxeles, diferencia acotada).
#[test]
fn tiles_match_single_pass_up_to_antialiasing() {
    let rects = page(
        "<!doctype html><body style='margin:0'>\
         <div style='height: 300px; background: linear-gradient(#f00, #00f)'></div>\
         <div style='height: 100px; background: #3a3; opacity: .5'></div>\
         <div style='height: 100px; border: 4px solid #333'></div>",
    );
    for scale in [1.0f32, 2.0] {
        for scroll in [(0.0, 0.0), (0.0, 37.0), (0.0, 150.0)] {
            let one = render_scrolled(&rects.display_list, VIEWPORT, scale, Rgba::WHITE, scroll);
            assert!(diff(&tiled(&rects, scale, scroll, 2), &one).is_none());
        }
    }
    let p = page(PAGE);
    for scale in [1.0f32, 2.0] {
        let one = render_scrolled(&p.display_list, VIEWPORT, scale, Rgba::WHITE, (0.0, 37.0));
        let t = tiled(&p, scale, (0.0, 37.0), 2);
        let (mut n, mut max) = (0usize, 0i32);
        for (a, b) in one.data().iter().zip(t.data()) {
            let d = (i32::from(*a) - i32::from(*b)).abs();
            if d > 0 {
                n += 1;
                max = max.max(d);
            }
        }
        let total = one.data().len();
        println!("escala {scale}: {n} canales distintos de {total}, diferencia máxima {max}");
        assert!(
            n * 1000 < total,
            "demasiados canales distintos: {n} de {total}"
        );
    }
}

/// Lo que no se puede hacer en tiles o por desplazamiento incremental se rechaza.
#[test]
fn incremental_scroll_falls_back() {
    let p = page(PAGE);
    let frame = tiled(&p, 1.0, (0.0, 0.0), 1);
    let at = |prev: (f32, f32), next: (f32, f32), scale: f32| {
        scroll_frame(
            &p.display_list,
            &frame,
            prev,
            VIEWPORT,
            scale,
            Rgba::WHITE,
            next,
        )
    };
    assert!(
        at((0.0, 0.0), (0.0, 0.3), 1.0).is_none(),
        "px de dispositivo no enteros"
    );
    assert!(at((0.0, 0.0), (5.0, 0.0), 1.0).is_none(), "horizontal");
    assert!(
        at((0.0, 0.0), (0.0, 500.0), 1.0).is_none(),
        "más de un cuadro"
    );
    assert!(at((0.0, 0.0), (0.0, 10.0), 2.0).is_none(), "otra escala");
    let filtered = page("<div style='filter: blur(2px); height: 50px'></div>");
    assert!(
        render_tiled(
            &filtered.display_list,
            VIEWPORT,
            1.0,
            Rgba::WHITE,
            (0.0, 0.0)
        )
        .is_none(),
        "un filter"
    );
}

/// `render_over` (solo la caja que cubren los ítems) da los mismos píxeles que la capa entera
/// del cuadro compuesta con `draw_pixmap`: barras redondeadas con recorte y opacidad, texto,
/// un scroll fraccionario y una lista con transformación (que se pinta entera).
#[test]
fn render_over_matches_full_layer() {
    use bf_layout::fragment::Rect;
    use bf_paint::display_list::{DisplayItem, DisplayList, Radii};
    use bf_paint::raster::render_over;
    use tiny_skia::{PixmapPaint, Transform};

    let gray = Rgba {
        r: 0.3,
        g: 0.3,
        b: 0.35,
        a: 0.55,
    };
    let bar = |x: f32, y: f32, w: f32, h: f32| DisplayItem::Rect {
        rect: Rect::new(x, y, w, h),
        color: gray,
        radii: Radii([(3.0, 3.0); 4]),
    };
    let mut bars = DisplayList::default();
    bars.push(DisplayItem::PushClip {
        rect: Rect::new(10.0, 40.0, 300.0, 120.0),
        radii: Radii::default(),
    });
    bars.push(bar(300.5, 52.25, 6.0, 40.0));
    bars.push(DisplayItem::PushOpacity(0.7));
    bars.push(bar(12.0, 150.0, 120.0, 6.0));
    bars.push(DisplayItem::PopOpacity);
    bars.push(DisplayItem::PopClip);
    bars.push(bar(-20.0, 230.0, 80.0, 30.0)); // corta el borde del cuadro
    let mut moved = bars.clone();
    moved.push(DisplayItem::PushTransform { dx: 5.0, dy: 7.0 });
    moved.push(bar(40.0, 20.0, 30.0, 30.0));
    moved.push(DisplayItem::PopTransform);
    let text = page("<p style='margin: 30px'>Texto <b>resaltado</b> encima de la página</p>");
    for list in [&bars, &moved, &text.display_list, &DisplayList::default()] {
        for scale in [1.0f32, 2.0] {
            for scroll in [(0.0, 0.0), (0.0, 13.0), (3.0, 7.25)] {
                let mut base = Pixmap::new(400, 600).unwrap();
                base.fill(tiny_skia::Color::from_rgba8(200, 220, 240, 255));
                let at = (4.0, 31.0 * scale);
                let mut want = base.clone();
                let layer = render_scrolled(list, VIEWPORT, scale, Rgba::TRANSPARENT, scroll);
                want.draw_pixmap(
                    at.0 as i32,
                    at.1 as i32,
                    layer.as_ref(),
                    &PixmapPaint::default(),
                    Transform::identity(),
                    None,
                );
                let mut got = base;
                render_over(list, &mut got, at, VIEWPORT, scale, scroll);
                if let Some(d) = diff(&got, &want) {
                    panic!("escala {scale}, scroll {scroll:?}: {d}");
                }
            }
        }
    }
}
