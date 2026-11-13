//! E5: las cachés de la maqueta (líneas por caja y anchos intrínsecos) no cambian ningún bit
//! del resultado: la misma página maquetada con y sin ellas da el mismo árbol de fragmentos
//! (comparado con `{:?}`, que escribe cada `f32` con todos sus dígitos significativos).

use bf_css::media::MediaContext;
use bf_css::selectors::MatchContext;
use bf_html::{ParseOptions, parse_document};
use bf_layout::{ImageSizes, Size, TestFont, layout_document_with_caches};
use bf_style::style_document;

fn both(html: &str) -> (String, String) {
    let (doc, _) = parse_document(html, &ParseOptions::default());
    let ctx = MatchContext::for_document(&doc);
    let media = MediaContext {
        width: 800.0,
        height: 600.0,
        ..MediaContext::default()
    };
    let styles = style_document(&doc, &[], &media, &ctx);
    let viewport = Size {
        width: 800.0,
        height: 600.0,
    };
    let images = ImageSizes::new();
    let run = |caches| {
        let tree =
            layout_document_with_caches(&doc, &styles, viewport, &TestFont, &images, Some(caches));
        format!("{:?}", tree.root)
    };
    (run(true), run(false))
}

/// Un grid con filas `auto`: cada ítem se maqueta para medir su alto (CSS Grid 2 §12.1) y otra
/// vez en su sitio, con orígenes fraccionarios y distintos de 0 en las dos pasadas (el padding y
/// el borde de los ítems) y de magnitudes muy distintas (el grid empieza muy abajo, donde
/// `(o₀ + a) + (o₁ − o₀)` y `o₁ + a` ya no dan el mismo `f32`); dentro, un flex en columna que
/// también mide. Los ítems no se estiran (`align-items: start`): con alto definido en la segunda
/// pasada, las líneas tendrían otra clave y no se reutilizarían.
#[test]
fn grid_two_passes_same_bits() {
    let html = r#"<!doctype html>
<style>
  body { margin: 7.3px; font: 13.7px/1.37 sans-serif }
  .g { display: grid; grid-template-columns: 1fr 2.3fr; gap: 3.1px 5.7px; padding: 11.13px;
       margin: 1234.567px 0 0 133.33px; align-items: start }
  .g > div { padding: 3.71px 2.93px; border-top: 1.37px solid }
  .f { display: flex; flex-direction: column; padding: 1.7px }
  .r { position: relative; top: 1.3px }
  .a { position: absolute; left: 3.3px }
  .ib { display: inline-block; padding: 2.9px; width: 41.3px }
</style>
<div class="g">
  <div>Uno dos tres cuatro cinco seis siete ocho nueve diez once doce trece.</div>
  <div class="f"><p>Texto <span class="r">relativo</span> y <b>negrita</b> que se corta en
    varias líneas dentro de un flex en columna.</p><p>Otro <span class="a">abs</span> párrafo
    con <span class="ib">bloque en línea con texto</span> al final.</p></div>
  <div style="padding-left: 9.71px">Más texto para la segunda fila del grid con un ancho
    fraccionario.</div>
  <div class="f" style="align-items: center"><span>centrado</span><span>en columna</span></div>
</div>"#;
    let (with, without) = both(html);
    assert_eq!(with, without);
}

/// Flotantes en el contexto de las líneas, dentro de un ítem de grid (dos pasadas) y fuera.
#[test]
fn floats_same_bits() {
    let html = r#"<!doctype html>
<style>
  body { margin: 5.37px; font: 12.3px/1.41 sans-serif }
  .g { display: grid; grid-template-columns: 2fr 1fr; padding: 3.33px }
  .fl { float: left; width: 61.7px; height: 37.9px; margin: 2.1px 4.3px }
  .fr { float: right; width: 23.3px; height: 71.1px }
  p { margin: 3.7px 0 }
</style>
<div class="g">
  <div><div class="fl"></div><div class="fr"></div>
    <p>Primer párrafo junto a dos flotantes, con texto suficiente para varias líneas que
      rodean los dos y bajan cuando se acaban.</p>
    <p>Segundo párrafo, todavía junto al flotante derecho, con <span class="fl" style="height:
      9.1px; width: 12.7px"></span> un flotante dentro de las líneas.</p>
    <p style="clear: left">Tercer párrafo después de despejar los flotantes izquierdos.</p></div>
  <div><p>Columna estrecha con texto que se corta muchas veces en líneas cortas.</p></div>
</div>
<div class="fl"></div>
<p>Fuera del grid, junto a un flotante, en el flujo normal.</p>"#;
    let (with, without) = both(html);
    assert_eq!(with, without);
}

/// Ítems estirados (alto indefinido al medirlos, definido al colocarlos): las líneas sin nada
/// que lea el alto del containing block se reutilizan entre las dos pasadas; las que tienen
/// altos o `top` en porcentaje o `stretch` dentro, no.
#[test]
fn stretched_items_same_bits() {
    let html = r#"<!doctype html>
<style>
  body { margin: 3.17px; font: 14.3px/1.29 sans-serif }
  .g { display: grid; grid-template-columns: 1.3fr 1fr 0.7fr; grid-auto-rows: minmax(57.3px, auto);
       padding: 7.77px; margin-top: 811.3px }
  .g > div { padding: 2.71px }
  .ib { display: inline-block; width: 31.3px; height: 50%; background: red }
  .rel { position: relative; top: 10% }
  .st { display: inline-block; min-height: stretch; width: 12.3px }
  .fx { display: flex; flex-direction: column; height: 151.7px }
  .fx > p { flex: 1; margin: 1.3px }
</style>
<div class="g">
  <div>Texto sin nada que lea el alto, en varias líneas que se cortan dentro de su columna.</div>
  <div>Con <span class="ib"></span> un bloque en línea de alto en porcentaje y más texto.</div>
  <div>Con <span class="rel">un desplazamiento relativo en porcentaje</span> en medio.</div>
  <div>Con <span class="st"></span> un bloque en línea con min-height: stretch.</div>
  <div class="fx"><p>Primer párrafo flexible en una columna de alto fijo.</p><p>Segundo.</p></div>
  <div>Último ítem con texto corto.</div>
</div>"#;
    let (with, without) = both(html);
    assert_eq!(with, without);
}
