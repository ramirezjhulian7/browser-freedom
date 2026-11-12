//! La cascada con shadow trees (CSS Scoping §3, E1): las hojas de cada árbol, `:host`,
//! `:host()`, `:host-context()`, `::slotted()`, `::part()`, el orden de los contextos y la
//! herencia por el flat tree.

use bf_css::media::MediaContext;
use bf_css::selectors::MatchContext;
use bf_dom::{Document, NodeId};
use bf_html::{ParseOptions, parse_document};
use bf_style::{StyleMap, style_document};

fn styled(html: &str) -> (Document, StyleMap) {
    let opts = ParseOptions {
        allow_declarative_shadow_roots: true,
        ..ParseOptions::default()
    };
    let (doc, _) = parse_document(html, &opts);
    let ctx = MatchContext::for_document(&doc);
    let map = style_document(&doc, &[], &MediaContext::default(), &ctx);
    (doc, map)
}

/// El elemento con ese `id`, buscando también en los shadow trees.
fn by_id(doc: &Document, id: &str) -> NodeId {
    doc.shadow_including_inclusive_descendants(doc.root())
        .into_iter()
        .find(|&n| doc.element(n).and_then(|e| e.attr("id")) == Some(id))
        .unwrap_or_else(|| panic!("sin #{id}"))
}

fn css(doc: &Document, map: &StyleMap, id: &str, property: &str) -> String {
    let n = by_id(doc, id);
    map.get(&(n, None))
        .unwrap_or_else(|| panic!("#{id} sin estilo"))
        .get_css(property)
        .expect("propiedad")
}

const PAGE: &str = "<!DOCTYPE html><style>\
    p { margin-bottom: 3px }\
    span { color: rgb(0, 0, 255) }\
    #h2 { color: rgb(0, 0, 255) }\
    .host::part(label) { color: rgb(0, 128, 0) }\
    </style>\
    <div class=dark><div id=h1 class=host><template shadowrootmode=open><style>\
      :host { display: inline-block; color: rgb(255, 0, 0) }\
      :host(.host) { margin-left: 5px }\
      :host-context(.dark) { margin-right: 6px }\
      ::slotted(span) { font-weight: 700 }\
      p { margin-top: 7px }\
      span { color: rgb(255, 255, 0) }\
      </style><p id=inner part=label>in</p><span id=own>own</span><slot></slot></template>\
      <span id=light>x</span><em id=hidden-slot slot=missing>no</em></div></div>\
    <div id=h2><template shadowrootmode=open><style>:host { color: rgb(255, 0, 0) }\
      :host { margin-left: 9px !important }</style><slot></slot></template></div>";

#[test]
fn shadow_tree_sheets_are_scoped() {
    let (doc, map) = styled(PAGE);
    // Las reglas del documento no entran al shadow tree ni las del shadow tree salen.
    // El margen del `p` interno es el de la hoja UA (16px), no el `3px` del documento.
    assert_eq!(css(&doc, &map, "inner", "margin-bottom"), "16px");
    assert_eq!(css(&doc, &map, "inner", "margin-top"), "7px");
    assert_eq!(css(&doc, &map, "own", "color"), "rgb(255, 255, 0)");
    assert_eq!(css(&doc, &map, "light", "color"), "rgb(0, 0, 255)");
}

#[test]
fn host_selectors() {
    let (doc, map) = styled(PAGE);
    assert_eq!(css(&doc, &map, "h1", "display"), "inline-block");
    assert_eq!(css(&doc, &map, "h1", "color"), "rgb(255, 0, 0)");
    assert_eq!(css(&doc, &map, "h1", "margin-left"), "5px");
    assert_eq!(css(&doc, &map, "h1", "margin-right"), "6px");
    // Para las normales gana el árbol exterior; para `!important`, el interior.
    assert_eq!(css(&doc, &map, "h2", "color"), "rgb(0, 0, 255)");
    assert_eq!(css(&doc, &map, "h2", "margin-left"), "9px");
}

#[test]
fn slotted_and_part() {
    let (doc, map) = styled(PAGE);
    assert_eq!(css(&doc, &map, "light", "font-weight"), "700");
    assert_eq!(css(&doc, &map, "own", "font-weight"), "400");
    assert_eq!(css(&doc, &map, "inner", "color"), "rgb(0, 128, 0)");
    // Un hijo del host sin slot no está en el flat tree: no tiene estilo.
    let hidden = by_id(&doc, "hidden-slot");
    assert!(!map.contains_key(&(hidden, None)));
}

#[test]
fn inheritance_follows_the_flat_tree() {
    let html = "<!DOCTYPE html><div id=host style='color: rgb(1, 2, 3)'>\
        <template shadowrootmode=open><div id=wrap style='color: rgb(9, 9, 9)'><slot></slot></div></template>\
        <span id=light>x</span></div>";
    let (doc, map) = styled(html);
    // El span hereda del slot (dentro de #wrap), no de su padre en el DOM.
    assert_eq!(css(&doc, &map, "light", "color"), "rgb(9, 9, 9)");
}
