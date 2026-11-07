//! El zoom de texto del navegador (`MediaContext::text_zoom`, E11): escala los tamaños de
//! fuente que no dependen del padre (palabras clave, longitudes absolutas, el inicial del
//! root) y deja que lo relativo (`em`, `%`, `rem`, `smaller`, `calc()`) parta de ellos, sin
//! escalar dos veces. Las demás longitudes en `px` no cambian.

use bf_css::media::MediaContext;
use bf_css::selectors::{MatchContext, NamespaceMap, matches, parse_selector_list};
use bf_dom::{Document, NodeId};
use bf_html::{ParseOptions, parse_document};
use bf_style::style_document;

fn computed(html: &str, zoom: f64, selector: &str, property: &str) -> String {
    let (doc, _) = parse_document(html, &ParseOptions::default());
    let ctx = MatchContext::for_document(&doc);
    let media = MediaContext {
        text_zoom: zoom,
        ..MediaContext::default()
    };
    let map = style_document(&doc, &[], &media, &ctx);
    let node = find(&doc, selector);
    map[&(node, None)]
        .get_css(property)
        .unwrap_or_else(|| panic!("propiedad desconocida {property}"))
}

fn find(doc: &Document, selector: &str) -> NodeId {
    let list = parse_selector_list(selector, &NamespaceMap::default()).expect("selector válido");
    let ctx = MatchContext::for_document(doc);
    doc.inclusive_descendants(doc.root())
        .into_iter()
        .find(|&n| list.0.iter().any(|s| matches(s, doc, n, &ctx)))
        .unwrap_or_else(|| panic!("ningún elemento coincide con {selector}"))
}

const PAGE: &str = "<!DOCTYPE html><style>
  #px { font-size: 20px }
  #pt { font-size: 12pt }
  #kw { font-size: large }
  #em { font-size: 2em }
  #pc { font-size: 50% }
  #rem { font-size: 1.5rem }
  #calc { font-size: calc(1em + 4px) }
  #sm { font-size: smaller }
  #init { font-size: initial }
  #box { width: 100px; padding-left: 1em }
</style>
<p id=plain>a</p>
<h1>t</h1>
<div id=px><span id=em>b</span><span id=pc>c</span></div>
<p id=pt>d</p><p id=kw>e</p><p id=rem>f</p><p id=calc>g</p><p id=sm>h</p>
<div id=px2 style='font-size: 10px'><i id=init>i</i></div>
<div id=box>j</div>";

#[test]
fn without_zoom_nothing_changes() {
    assert_eq!(computed(PAGE, 1.0, "#plain", "font-size"), "16px");
    assert_eq!(computed(PAGE, 1.0, "#px", "font-size"), "20px");
    assert_eq!(computed(PAGE, 1.0, "#em", "font-size"), "40px");
    assert_eq!(computed(PAGE, 1.0, "h1", "font-size"), "32px");
}

#[test]
fn absolute_sizes_scale() {
    let z = |sel: &str| computed(PAGE, 1.5, sel, "font-size");
    // El inicial del root (16px) y lo que hereda.
    assert_eq!(z("html"), "24px");
    assert_eq!(z("#plain"), "24px");
    // Longitudes absolutas y palabras clave.
    assert_eq!(z("#px"), "30px");
    assert_eq!(z("#pt"), "24px");
    assert_eq!(z("#kw"), "28.8px");
    // Un `font-size` en el atributo `style` también.
    assert_eq!(z("#px2"), "15px");
    // `initial` es `medium`.
    assert_eq!(z("#init"), "24px");
}

#[test]
fn relative_sizes_scale_once() {
    let z = |sel: &str| computed(PAGE, 1.5, sel, "font-size");
    // Relativos al padre ya escalado: no se escalan otra vez.
    assert_eq!(z("#em"), "60px");
    assert_eq!(z("#pc"), "15px");
    assert_eq!(z("h1"), "48px");
    // `rem`: el root ya escalado.
    assert_eq!(z("#rem"), "36px");
    // `calc()` mixto: relativo, parte del padre escalado (24px + 4px).
    assert_eq!(z("#calc"), "28px");
    assert_eq!(z("#sm"), "20px");
}

#[test]
fn other_lengths_do_not_scale() {
    // El zoom de texto no agranda las cajas: `width` en px queda igual; lo que está en `em`
    // sigue a la fuente.
    assert_eq!(computed(PAGE, 2.0, "#box", "width"), "100px");
    assert_eq!(computed(PAGE, 2.0, "#box", "padding-left"), "32px");
}
