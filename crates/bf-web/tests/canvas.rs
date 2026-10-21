//! `HTMLCanvasElement` y `CanvasRenderingContext2D` (`bf_web::html::canvas`): los píxeles se
//! comprueban con `getImageData` desde JavaScript.

use std::rc::Rc;

use bf_js::host::DefaultHooks;
use bf_web::html::canvas::{CanvasFonts, snapshot};
use bf_web::{DocumentOwner, Runtime, Url, WebContext, WebContextHandle};

struct Page {
    rt: Runtime,
    ctx: WebContextHandle,
}

impl Page {
    /// Una página con `<canvas id=c width=20 height=20>` y `ctx` = su contexto 2D.
    fn new() -> Page {
        Page::with(
            "<!DOCTYPE html><body><canvas id=c width=20 height=20></canvas>\
                    <canvas id=d width=4 height=4></canvas></body>",
        )
    }

    fn with(html: &str) -> Page {
        let mut rt = Runtime::new(Box::new(DefaultHooks));
        let (mut doc, _) = bf_html::parse_document(html, &bf_html::ParseOptions::default());
        let realm = rt.main_realm();
        let url = Url::parse("https://example.com/", None).expect("url");
        doc.url = url.clone();
        let ctx = bf_web::install(
            &mut rt.agent,
            realm,
            WebContext::new(DocumentOwner::Document(Box::new(doc)), url),
        );
        let mut p = Page { rt, ctx };
        p.show("var c = document.getElementById('c'); var ctx = c.getContext('2d'); 0");
        p
    }

    /// Con la fuente de prueba (cada carácter avanza 0.5em; sin contornos).
    fn with_test_font(self) -> Page {
        let mut p = self;
        let realm = p.ctx.realm;
        p.rt.agent
            .set_realm_data(realm, CanvasFonts(Rc::new(bf_layout::TestFont)));
        p
    }

    fn show(&mut self, src: &str) -> String {
        let realm = self.ctx.realm;
        match self.rt.eval_script(realm, src, "test.js") {
            Ok(v) => self.rt.to_display(&v),
            Err(e) => format!("Error: {}", e.message),
        }
    }

    /// El píxel (x, y) del canvas `c` como "r,g,b,a".
    fn px(&mut self, x: u32, y: u32) -> String {
        self.show(&format!(
            "Array.from(ctx.getImageData({x}, {y}, 1, 1).data).join()"
        ))
    }
}

const RED: &str = "255,0,0,255";
const CLEAR: &str = "0,0,0,0";

#[test]
fn get_context() {
    let mut p = Page::new();
    assert_eq!(
        p.show(
            "[c.getContext('2d') === ctx, c.getContext('webgl'), ctx.canvas === c, \
                Object.prototype.toString.call(ctx)].join()"
        ),
        ",,true,[object CanvasRenderingContext2D]".replacen(",,", "true,,", 1)
    );
}

#[test]
fn fill_rect() {
    let mut p = Page::new();
    p.show("ctx.fillStyle = 'red'; ctx.fillRect(2, 2, 5, 5)");
    assert_eq!(p.px(4, 4), RED);
    assert_eq!(p.px(1, 1), CLEAR);
    assert_eq!(p.px(7, 7), CLEAR);
}

#[test]
fn clear_rect() {
    let mut p = Page::new();
    p.show("ctx.fillStyle = 'red'; ctx.fillRect(0, 0, 20, 20); ctx.clearRect(5, 5, 5, 5)");
    assert_eq!(p.px(6, 6), CLEAR);
    assert_eq!(p.px(2, 2), RED);
}

#[test]
fn stroke_rect() {
    let mut p = Page::new();
    p.show("ctx.strokeStyle = '#00ff00'; ctx.lineWidth = 2; ctx.strokeRect(5, 5, 10, 10)");
    assert_eq!(p.px(5, 10), "0,255,0,255");
    assert_eq!(p.px(10, 10), CLEAR);
}

#[test]
fn style_serialization() {
    let mut p = Page::new();
    assert_eq!(
        p.show(
            "var r = []; ctx.fillStyle = 'red'; r.push(ctx.fillStyle); \
                ctx.fillStyle = 'rgba(0, 0, 255, 0.5)'; r.push(ctx.fillStyle); \
                ctx.fillStyle = 'no-es-un-color'; r.push(ctx.fillStyle); \
                ctx.strokeStyle = 'hsl(120, 100%, 50%)'; r.push(ctx.strokeStyle); r.join('|')"
        ),
        "#ff0000|rgba(0, 0, 255, 0.5)|rgba(0, 0, 255, 0.5)|#00ff00"
    );
}

#[test]
fn global_alpha() {
    let mut p = Page::new();
    p.show("ctx.globalAlpha = 0.5; ctx.globalAlpha = 2; ctx.fillStyle = 'red'; ctx.fillRect(0, 0, 4, 4)");
    assert_eq!(p.show("ctx.globalAlpha"), "0.5");
    assert_eq!(p.px(1, 1), "255,0,0,128");
}

#[test]
fn triangle_path() {
    let mut p = Page::new();
    p.show(
        "ctx.fillStyle = 'blue'; ctx.beginPath(); ctx.moveTo(0, 0); ctx.lineTo(20, 0); \
            ctx.lineTo(0, 20); ctx.closePath(); ctx.fill()",
    );
    assert_eq!(p.px(2, 2), "0,0,255,255");
    assert_eq!(p.px(17, 17), CLEAR);
}

#[test]
fn arc_fill() {
    let mut p = Page::new();
    p.show(
        "ctx.fillStyle = 'red'; ctx.beginPath(); ctx.arc(10, 10, 6, 0, Math.PI * 2); ctx.fill()",
    );
    assert_eq!(p.px(10, 10), RED);
    assert_eq!(p.px(15, 10), RED);
    assert_eq!(p.px(1, 1), CLEAR);
    assert_eq!(
        p.show("try { ctx.arc(0, 0, -1, 0, 1) } catch (e) { e.name }"),
        "IndexSizeError"
    );
}

#[test]
fn stroke_line_width() {
    let mut p = Page::new();
    p.show(
        "ctx.strokeStyle = 'red'; ctx.lineWidth = 4; ctx.beginPath(); ctx.moveTo(0, 10); \
            ctx.lineTo(20, 10); ctx.stroke()",
    );
    assert_eq!(p.px(10, 9), RED);
    assert_eq!(p.px(10, 11), RED);
    assert_eq!(p.px(10, 14), CLEAR);
}

#[test]
fn translate_and_scale() {
    let mut p = Page::new();
    p.show(
        "ctx.fillStyle = 'red'; ctx.translate(10, 10); ctx.scale(2, 2); ctx.fillRect(0, 0, 2, 2)",
    );
    assert_eq!(p.px(13, 13), RED);
    assert_eq!(p.px(9, 9), CLEAR);
    assert_eq!(p.px(14, 14), CLEAR);
}

#[test]
fn rotate() {
    let mut p = Page::new();
    p.show("ctx.fillStyle = 'red'; ctx.translate(10, 10); ctx.rotate(Math.PI / 2); ctx.fillRect(0, 0, 8, 2)");
    // Rotado 90°: el rectángulo va hacia abajo y a la izquierda del origen.
    assert_eq!(p.px(9, 15), RED);
    assert_eq!(p.px(15, 11), CLEAR);
}

#[test]
fn transform_getters() {
    let mut p = Page::new();
    assert_eq!(
        p.show(
            "ctx.setTransform(2, 0, 0, 3, 4, 5); var t = ctx.getTransform(); \
                var a = [t.a, t.d, t.e, t.f].join(); ctx.resetTransform(); \
                a + '|' + ctx.getTransform().a"
        ),
        "2,3,4,5|1"
    );
}

#[test]
fn save_restore() {
    let mut p = Page::new();
    assert_eq!(
        p.show(
            "ctx.fillStyle = 'red'; ctx.save(); ctx.fillStyle = 'blue'; ctx.translate(5, 5); \
                ctx.restore(); [ctx.fillStyle, ctx.getTransform().e].join()"
        ),
        "#ff0000,0"
    );
}

#[test]
fn clip_region() {
    let mut p = Page::new();
    p.show(
        "ctx.beginPath(); ctx.rect(0, 0, 10, 10); ctx.clip(); ctx.fillStyle = 'red'; \
            ctx.fillRect(0, 0, 20, 20)",
    );
    assert_eq!(p.px(5, 5), RED);
    assert_eq!(p.px(15, 15), CLEAR);
}

#[test]
fn linear_gradient() {
    let mut p = Page::new();
    p.show(
        "var g = ctx.createLinearGradient(0, 0, 20, 0); g.addColorStop(0, 'red'); \
            g.addColorStop(1, 'blue'); ctx.fillStyle = g; ctx.fillRect(0, 0, 20, 20)",
    );
    assert_eq!(p.show("ctx.fillStyle === g"), "true");
    let left = p.px(0, 5);
    let right = p.px(19, 5);
    let rgb = |s: &str| -> Vec<u32> { s.split(',').map(|v| v.parse().unwrap()).collect() };
    let (l, r) = (rgb(&left), rgb(&right));
    assert!(l[0] > 230 && l[2] < 25 && l[3] == 255, "{left}");
    assert!(r[0] < 25 && r[2] > 230 && r[3] == 255, "{right}");
    assert_eq!(
        p.show("try { g.addColorStop(2, 'red') } catch (e) { e.name }"),
        "IndexSizeError"
    );
}

#[test]
fn radial_gradient() {
    let mut p = Page::new();
    p.show(
        "var g = ctx.createRadialGradient(10, 10, 0, 10, 10, 10); g.addColorStop(0, '#00ff00'); \
            g.addColorStop(1, '#000000'); ctx.fillStyle = g; ctx.fillRect(0, 0, 20, 20)",
    );
    let center: Vec<u32> = p
        .px(10, 10)
        .split(',')
        .map(|v| v.parse().unwrap())
        .collect();
    assert!(center[1] > 200 && center[0] < 20, "{center:?}");
}

#[test]
fn draw_image_from_canvas() {
    let mut p = Page::new();
    p.show(
        "var d = document.getElementById('d'); var dc = d.getContext('2d'); \
            dc.fillStyle = 'red'; dc.fillRect(0, 0, 4, 4); ctx.drawImage(d, 8, 8)",
    );
    assert_eq!(p.px(9, 9), RED);
    assert_eq!(p.px(13, 13), CLEAR);
    // Escalado (5 argumentos) y recorte (9 argumentos).
    p.show("ctx.clearRect(0, 0, 20, 20); ctx.drawImage(d, 0, 0, 16, 16)");
    assert_eq!(p.px(14, 14), RED);
    p.show("ctx.clearRect(0, 0, 20, 20); ctx.drawImage(d, 0, 0, 2, 2, 10, 10, 4, 4)");
    assert_eq!(p.px(12, 12), RED);
    assert_eq!(p.px(15, 15), CLEAR);
    assert_eq!(
        p.show("try { ctx.drawImage({}, 0, 0) } catch (e) { e.name }"),
        "TypeError"
    );
}

#[test]
fn image_data_round_trip() {
    let mut p = Page::new();
    assert_eq!(
        p.show(
            "var id = ctx.createImageData(2, 2); id.data[0] = 10; id.data[3] = 255; \
                id.data[7] = 255; id.data[4] = 200; ctx.putImageData(id, 3, 3); \
                var back = ctx.getImageData(3, 3, 2, 1); \
                [id.width, id.height, id.data.length, Array.from(back.data).join(' ')].join('|')"
        ),
        "2|2|16|10 0 0 255 200 0 0 255"
    );
}

#[test]
fn image_data_constructor() {
    let mut p = Page::new();
    assert_eq!(
        p.show("var a = new ImageData(2, 3); var b = new ImageData(new Uint8ClampedArray(16), 2); \
                [a.width, a.height, a.data.length, b.height, a.colorSpace, \
                 (function () { try { new ImageData(0, 1) } catch (e) { return e.name } })()].join()"),
        "2,3,24,2,srgb,IndexSizeError"
    );
}

#[test]
fn put_image_data_ignores_state() {
    let mut p = Page::new();
    p.show(
        "ctx.globalAlpha = 0.1; ctx.translate(5, 5); var id = ctx.createImageData(1, 1); \
            id.data.set([255, 0, 0, 255]); ctx.putImageData(id, 0, 0)",
    );
    assert_eq!(p.px(0, 0), RED);
}

#[test]
fn get_image_data_errors() {
    let mut p = Page::new();
    assert_eq!(
        p.show("try { ctx.getImageData(0, 0, 0, 1) } catch (e) { e.name }"),
        "IndexSizeError"
    );
    // Fuera del bitmap: transparente.
    assert_eq!(
        p.show("Array.from(ctx.getImageData(-5, -5, 1, 1).data).join()"),
        CLEAR
    );
}

#[test]
fn width_resets_canvas() {
    let mut p = Page::new();
    p.show("ctx.fillStyle = 'red'; ctx.fillRect(0, 0, 20, 20); c.width = 20");
    assert_eq!(p.px(5, 5), CLEAR);
    assert_eq!(
        p.show("[ctx.fillStyle, c.width, c.height].join()"),
        "#000000,20,20"
    );
    p.show("c.height = 7");
    assert_eq!(p.show("ctx.getImageData(0, 0, 20, 7).data.length"), "560");
}

#[test]
fn to_data_url() {
    let mut p = Page::new();
    assert!(
        p.show("c.toDataURL()")
            .starts_with("data:image/png;base64,iVBORw0KGgo")
    );
    assert_eq!(p.show("c.width = 0; c.toDataURL()"), "data:,");
}

#[test]
fn measure_text_with_test_font() {
    let mut p = Page::new().with_test_font();
    assert_eq!(
        p.show("ctx.font = '20px sans-serif'; var m = ctx.measureText('abcd'); \
                [m.width, m.actualBoundingBoxLeft, m.fontBoundingBoxAscent, m.fontBoundingBoxDescent].join()"),
        "40,0,16,4"
    );
    assert_eq!(
        p.show("ctx.textAlign = 'center'; ctx.measureText('abcd').actualBoundingBoxLeft"),
        "20"
    );
    assert_eq!(
        p.show("ctx.textBaseline = 'top'; ctx.measureText('a').fontBoundingBoxAscent"),
        "32"
    );
}

#[test]
fn font_attribute() {
    let mut p = Page::new();
    assert_eq!(
        p.show(
            "var r = [ctx.font]; ctx.font = 'bold 12px Arial'; r.push(ctx.font); \
                ctx.font = 'italic 2em \"Times New Roman\", serif'; r.push(ctx.font); \
                ctx.font = 'no válido'; r.push(ctx.font); r.join('|')"
        ),
        "10px sans-serif|bold 12px Arial|italic 20px \"Times New Roman\", serif|italic 20px \"Times New Roman\", serif"
    );
}

#[test]
fn fill_text_with_system_font() {
    let mut p = Page::new();
    // Con las fuentes del sistema: algo se pinta (sin fuentes, nada; el test lo admite).
    let has_fonts = !bf_font::FontDb::system().faces().is_empty();
    p.show("ctx.font = '16px sans-serif'; ctx.fillStyle = 'black'; ctx.fillText('Hg', 2, 16)");
    let painted = p.show(
        "Array.from(ctx.getImageData(0, 0, 20, 20).data).some((v, i) => i % 4 == 3 && v > 0)",
    );
    if has_fonts {
        assert_eq!(painted, "true");
    }
    assert_eq!(
        p.show("ctx.measureText('Hg').width > 0"),
        if has_fonts { "true" } else { "false" }
    );
}

#[test]
fn point_in_path_and_dash() {
    let mut p = Page::new();
    assert_eq!(
        p.show(
            "ctx.beginPath(); ctx.rect(5, 5, 10, 10); \
                [ctx.isPointInPath(10, 10), ctx.isPointInPath(1, 1)].join()"
        ),
        "true,false"
    );
    assert_eq!(
        p.show("ctx.setLineDash([1, 2, 3]); ctx.getLineDash().join()"),
        "1,2,3,1,2,3"
    );
}

#[test]
fn composite_destination_out() {
    let mut p = Page::new();
    p.show(
        "ctx.fillStyle = 'red'; ctx.fillRect(0, 0, 20, 20); \
            ctx.globalCompositeOperation = 'destination-out'; ctx.fillRect(0, 0, 10, 10)",
    );
    assert_eq!(p.px(5, 5), CLEAR);
    assert_eq!(p.px(15, 15), RED);
    assert_eq!(
        p.show("ctx.globalCompositeOperation = 'nope'; ctx.globalCompositeOperation"),
        "destination-out"
    );
}

#[test]
fn pattern_from_canvas() {
    let mut p = Page::new();
    p.show(
        "var d = document.getElementById('d'); var dc = d.getContext('2d'); \
            dc.fillStyle = 'red'; dc.fillRect(0, 0, 2, 2); \
            ctx.fillStyle = ctx.createPattern(d, 'repeat'); ctx.fillRect(0, 0, 20, 20)",
    );
    assert_eq!(p.px(4, 4), RED);
    assert_eq!(p.px(6, 6), CLEAR);
}

#[test]
fn snapshot_for_the_browser() {
    let mut p = Page::new();
    p.show("ctx.fillStyle = 'red'; ctx.fillRect(0, 0, 1, 1)");
    let realm = p.ctx.realm;
    let shots = snapshot(&p.rt.agent, realm);
    assert_eq!(shots.len(), 1);
    let (_, v1, pix) = &shots[0];
    assert_eq!((pix.width(), pix.height()), (20, 20));
    assert_eq!(pix.pixel(0, 0).map(|c| c.red()), Some(255));
    p.show("ctx.fillRect(1, 1, 1, 1)");
    let v2 = snapshot(&p.rt.agent, realm)[0].1;
    assert!(v2 > *v1);
}

#[test]
fn round_rect_and_arc_to() {
    let mut p = Page::new();
    p.show("ctx.fillStyle = 'red'; ctx.beginPath(); ctx.roundRect(0, 0, 20, 20, 8); ctx.fill()");
    assert_eq!(p.px(10, 10), RED);
    assert_eq!(p.px(0, 0), CLEAR);
    p.show(
        "ctx.clearRect(0, 0, 20, 20); ctx.strokeStyle = 'red'; ctx.lineWidth = 2; \
            ctx.beginPath(); ctx.moveTo(0, 2); ctx.arcTo(18, 2, 18, 20, 6); ctx.stroke()",
    );
    assert_eq!(p.px(5, 2), RED);
    // `arcTo` termina en la tangente (18, 8): no traza hasta (18, 20).
    let end = p.px(18, 7);
    assert!(
        end.starts_with("255,0,0,") && end[8..].parse::<u32>().unwrap() > 200,
        "{end}"
    );
    assert_eq!(p.px(18, 17), CLEAR);
}

/// El script de huella digital de canvas típico (como el de FingerprintJS) no lanza.
#[test]
fn fingerprint_script() {
    let mut p = Page::new();
    assert_eq!(
        p.show(
            "var cv = document.createElement('canvas'); cv.width = 240; cv.height = 60; \
             var x = cv.getContext('2d'); x.rect(0, 0, 10, 10); x.rect(2, 2, 6, 6); \
             var winding = !x.isPointInPath(5, 5, 'evenodd'); \
             x.textBaseline = 'alphabetic'; x.fillStyle = '#f60'; x.fillRect(100, 1, 62, 20); \
             x.fillStyle = '#069'; x.font = '11pt \"Times New Roman\"'; \
             x.fillText('Cwm fjordbank gly \u{1F603}', 2, 15); \
             x.fillStyle = 'rgba(102, 204, 0, 0.2)'; x.font = '18pt Arial'; \
             x.fillText('Cwm fjordbank gly \u{1F603}', 4, 45); \
             x.globalCompositeOperation = 'multiply'; \
             for (const c of ['#f2f', '#2ff', '#ff2']) { x.fillStyle = c; x.beginPath(); \
               x.arc(50, 50, 40, 0, Math.PI * 2, true); x.closePath(); x.fill(); } \
             x.fillStyle = '#f9c'; x.arc(75, 75, 75, 0, Math.PI * 2, true); \
             x.arc(75, 75, 25, 0, Math.PI * 2, true); x.fill('evenodd'); \
             var data = cv.toDataURL(); [winding, data.length > 100, \
             x.getImageData(0, 0, 1, 1).data.length].join()"
        ),
        "true,true,4"
    );
}
