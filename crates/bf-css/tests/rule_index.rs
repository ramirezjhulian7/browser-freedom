//! E4: el índice de reglas de la cascada (`RuleIndex`) da exactamente las mismas declaraciones
//! que el recorrido completo (`matching_declarations_in`) en cada nodo: el mismo orden, origen,
//! capa, especificidad y orden de aparición (CSS Cascade 5 §6), también para los
//! pseudoelementos (el índice los agrupa aparte) y con el filtro de ancestros
//! (`AncestorBloom`): combinadores de hermanos entre ancestros, mayúsculas y selectores que
//! no coinciden por un ancestro que falta.

use bf_css::media::MediaContext;
use bf_css::selectors::{MatchContext, PseudoElement, ShadowMatch};
use bf_css::stylesheet::{AncestorBloom, Origin, RuleIndex, Stylesheet, matching_declarations_in};
use bf_dom::Document;
use bf_html::{ParseOptions, parse_document};

const UA: &str = "
    p, div { display: block }
    * { color: black }
    li::marker { color: gray }
    [hidden] { display: none }
";

const AUTHOR: &str = "
    @layer base, tema;
    @layer base { .a { margin: 1px } #x { margin: 2px } }
    @layer tema { DIV.A { padding: 3px } }
    #X { border: 1px solid }
    .B, p > span.c { color: red }
    div .c:is(.d, #y) { color: blue }
    :where(.a) span { color: green }
    span::before { content: 'x' }
    p:first-child + div { width: 5px }
    @media (min-width: 100px) { .a { height: 1px } }
    @media (max-width: 10px) { .a { height: 9px } }
    @supports (display: grid) { .c { display: grid } }
    @supports (nada: nada) { .c { display: table } }
    .n { color: navy; & > span { color: teal } .m & { color: olive } }
    svg rect.r { fill: red }
    *|p { line-height: 2 }
    [data-k] { order: 1 }
    #y.c.d { z-index: 4 }
    .a p + div span { order: 2 }
    #x > p ~ div { order: 3 }
    ul li { order: 4 }
    DIV#X SPAN { order: 5 }
    .A .C { order: 6 }
    .nada span, .a span { order: 7 }
    .nada span { order: 8 }
    div p > span::before { content: 'y' }
";

const HTML: &str = "<div class='a b' id=x><p>uno<span class='c d' id=y>dos</span></p>\
    <div class=A>tres</div><p class=n><span>cuatro</span></p><div class='m'><p class=n>x</p></div>\
    <ul><li>cinco</li></ul><span data-k hidden>seis</span>\
    <svg><rect class=r /></svg></div>";

fn check(doc: &Document) {
    let ua = Stylesheet::parse(UA, Origin::UserAgent);
    let author = Stylesheet::parse(AUTHOR, Origin::Author);
    let sheets = [&ua, &author];
    let media = MediaContext::default();
    // `@supports (display: grid)` sí, `@supports (nada: nada)` no.
    let supports_grid =
        |name: &str, _: &[bf_css::syntax::ComponentValue]| name.eq_ignore_ascii_case("display");
    let ctx = MatchContext::for_document(doc);
    let shadow = ShadowMatch::default();
    let index = RuleIndex::new(&sheets, &media, &supports_grid);
    let mut compared = 0;
    for node in doc.inclusive_descendants(doc.root()) {
        for pseudo in [
            None,
            Some(PseudoElement::Before),
            Some(PseudoElement::Marker),
        ] {
            let full = matching_declarations_in(
                doc,
                node,
                pseudo,
                &sheets,
                &media,
                &supports_grid,
                &ctx,
                shadow,
            );
            let fast = index.matching(doc, node, pseudo, &ctx, shadow);
            assert_eq!(full, fast, "nodo {node:?} {pseudo:?}");
            // Con el filtro de ancestros (E4), lo mismo.
            let bloom = AncestorBloom::of_ancestors(doc, node);
            let filtered = index.matching_filtered(doc, node, pseudo, &ctx, shadow, Some(&bloom));
            assert_eq!(full, filtered, "con filtro: nodo {node:?} {pseudo:?}");
            compared += full.len();
        }
    }
    assert!(
        compared > 20,
        "el documento debe coincidir con varias reglas"
    );
}

#[test]
fn index_matches_the_full_walk() {
    let (doc, _) = parse_document(&format!("<!doctype html>{HTML}"), &ParseOptions::default());
    check(&doc);
}

/// En modo quirks los id y las clases no distinguen mayúsculas: `#X` y `.B` coinciden.
#[test]
fn index_matches_the_full_walk_in_quirks_mode() {
    let (doc, _) = parse_document(HTML, &ParseOptions::default());
    assert_eq!(doc.mode, bf_dom::QuirksMode::Quirks);
    check(&doc);
}
