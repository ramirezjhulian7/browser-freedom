//! E6 y E2: cuatro defectos de CSS de `https://asimetrix.co/` (Next.js con Tailwind v4), con un caso
//! mínimo escrito a mano para cada uno:
//!
//! 1. `@media (scripting)` (Media Queries 5 §8.1): con scripts, la regla UA
//!    `noscript { display: none }` se aplica; sin scripts, no.
//! 2. `::backdrop` y `::file-selector-button` (y los demás pseudo-elementos estándar sin caja
//!    todavía) no invalidan la lista de selectores del *preflight* de Tailwind (Selectors 4
//!    §3.1); uno desconocido sí.
//! 3. El margen por omisión del `<body>` (HTML §15.3.2) va en la hoja UA: un `margin: 0` de
//!    autor dentro de `@layer` le gana; los atributos `marginwidth`/`topmargin`… siguen siendo
//!    hints.
//! 4. Los presentational hints van debajo de toda regla de autor, también de las de `@layer`
//!    (CSS Cascade 5 §6.5), y `width`/`height` dan `aspect-ratio` (HTML §15.4.3).

use bf_css::media::{MediaContext, Scripting};
use bf_css::selectors::{MatchContext, NamespaceMap, PseudoElement, matches, parse_selector_list};
use bf_dom::{Document, NodeId};
use bf_html::{ParseOptions, parse_document};
use bf_style::{StyleMap, style_document};

/// El documento parseado con o sin scripting y sus estilos, con la media feature `scripting`
/// que corresponde al documento (como hace `bf_paint`).
fn styled(html: &str, scripting: bool) -> (Document, StyleMap) {
    let opts = ParseOptions {
        scripting,
        ..ParseOptions::default()
    };
    let (doc, _) = parse_document(html, &opts);
    let media = MediaContext {
        scripting: Scripting::of(doc.scripting),
        ..MediaContext::default()
    };
    let ctx = MatchContext::for_document(&doc);
    let map = style_document(&doc, &[], &media, &ctx);
    (doc, map)
}

fn find(doc: &Document, selector: &str) -> NodeId {
    let list = parse_selector_list(selector, &NamespaceMap::default()).expect("selector válido");
    let ctx = MatchContext::for_document(doc);
    doc.inclusive_descendants(doc.root())
        .into_iter()
        .find(|&n| list.0.iter().any(|s| matches(s, doc, n, &ctx)))
        .unwrap_or_else(|| panic!("ningún elemento coincide con {selector}"))
}

fn get(doc: &Document, map: &StyleMap, selector: &str, property: &str) -> String {
    map[&(find(doc, selector), None)]
        .get_css(property)
        .unwrap_or_else(|| panic!("propiedad desconocida {property}"))
}

/// 1. `@media (scripting)`.
#[test]
fn noscript_follows_the_scripting_feature() {
    let html = "<!doctype html><body><noscript><iframe src=x></iframe></noscript>\
        <style>@media (scripting) { p { color: red } } \
        @media (scripting: none) { p { color: blue } } \
        @media (scripting: enabled) { b { color: green } } \
        @media (scripting: initial-only) { b { color: black } }</style><p>t</p><b>b</b>";
    // Con scripts: el parser deja el contenido como texto y la hoja UA oculta el `noscript`.
    let (doc, map) = styled(html, true);
    assert!(doc.scripting);
    assert_eq!(get(&doc, &map, "noscript", "display"), "none");
    assert_eq!(get(&doc, &map, "p", "color"), "rgb(255, 0, 0)");
    assert_eq!(get(&doc, &map, "b", "color"), "rgb(0, 128, 0)");
    // Sin scripts: el `noscript` se muestra (con su contenido como elementos).
    let (doc, map) = styled(html, false);
    assert!(!doc.scripting);
    assert_eq!(get(&doc, &map, "noscript", "display"), "inline");
    find(&doc, "noscript > iframe");
    assert_eq!(get(&doc, &map, "p", "color"), "rgb(0, 0, 255)");
    // Un documento sin indicación: `scripting: none` por omisión.
    assert_eq!(MediaContext::default().scripting, Scripting::None);
}

/// 2. Pseudo-elementos que antes invalidaban la regla entera.
#[test]
fn preflight_with_backdrop_and_file_selector_button_applies() {
    // El *preflight* de Tailwind v4 (reducido a mano).
    let html = "<!doctype html><style>@layer base { \
          *, :after, :before, ::backdrop { box-sizing: border-box; margin: 0; padding: 0 } \
          ::file-selector-button { margin-right: 4px } } \
        h1, ::details-content, ::view-transition-group(*), ::highlight(marca), \
          ::target-text { color: red } \
        p, ::no-existe { color: blue } \
        p:is(.a, ::no-existe), :user-invalid, :autofill, :-webkit-autofill { color: green } \
        </style><h1>t</h1><p class=a>p</p><ul><li>x</li></ul>";
    let (doc, map) = styled(html, true);
    assert_eq!(get(&doc, &map, "h1", "margin-top"), "0px");
    assert_eq!(get(&doc, &map, "ul", "padding-left"), "0px");
    assert_eq!(get(&doc, &map, "h1", "box-sizing"), "border-box");
    assert_eq!(get(&doc, &map, "h1", "color"), "rgb(255, 0, 0)");
    // Un pseudo-elemento desconocido sigue invalidando la lista entera: `p` no es azul.
    // Dentro de `:is()` la lista es tolerante (Selectors 4 §4.2) y `p.a` coincide; las
    // pseudo-clases sin estado (`:user-invalid`, `:autofill`) se reconocen y no coinciden.
    assert_eq!(get(&doc, &map, "p", "color"), "rgb(0, 128, 0)");
    // Sin caja: no hay estilos de `::backdrop` ni de los demás.
    let backdrop = map
        .keys()
        .filter(|(_, p)| {
            matches!(
                p,
                Some(
                    PseudoElement::Backdrop
                        | PseudoElement::FileSelectorButton
                        | PseudoElement::DetailsContent
                )
            )
        })
        .count();
    assert_eq!(backdrop, 0);
    for s in [
        "::backdrop",
        "::file-selector-button",
        "::details-content",
        "::target-text",
        "::spelling-error",
        "::grammar-error",
        "::view-transition",
        "::view-transition-group(*)",
        "::view-transition-old(nombre)",
        "::highlight(marca)",
        ":user-valid",
        ":fullscreen",
        ":picture-in-picture",
    ] {
        assert!(
            parse_selector_list(s, &NamespaceMap::default()).is_ok(),
            "{s} debería ser válido"
        );
    }
    for s in [
        "::no-existe",
        "::highlight(*)",
        "::view-transition-group()",
        ":no-existe",
    ] {
        assert!(
            parse_selector_list(s, &NamespaceMap::default()).is_err(),
            "{s} debería ser inválido"
        );
    }
}

/// 3. El margen del `<body>`.
#[test]
fn body_margin_is_a_ua_rule() {
    // Sin nada: los 8px de la hoja UA.
    let (doc, map) = styled("<!doctype html><body><h1>t</h1>", true);
    assert_eq!(get(&doc, &map, "body", "margin-left"), "8px");
    assert_eq!(get(&doc, &map, "body", "margin-top"), "8px");
    // El caso mínimo de asimetrix.co: la capa de autor gana a la UA (CSS Cascade 5 §6.1).
    let (doc, map) = styled(
        "<!doctype html><style>@layer base { body, h1 { margin: 0 } }</style><h1>t</h1>",
        true,
    );
    assert_eq!(get(&doc, &map, "body", "margin-left"), "0px");
    assert_eq!(get(&doc, &map, "h1", "margin-top"), "0px");
    // Los atributos siguen siendo hints (HTML §15.3.2): `topmargin` y `marginheight` dan arriba
    // y abajo; `leftmargin` y `marginwidth`, izquierda y derecha. `bottommargin` y
    // `rightmargin` no existen (WPT `body-margin-3a/3b`).
    let (doc, map) = styled(
        "<!doctype html><body marginwidth=3 topmargin=5 bottommargin=7 rightmargin=9>t",
        true,
    );
    assert_eq!(get(&doc, &map, "body", "margin-top"), "5px");
    assert_eq!(get(&doc, &map, "body", "margin-bottom"), "5px");
    assert_eq!(get(&doc, &map, "body", "margin-left"), "3px");
    assert_eq!(get(&doc, &map, "body", "margin-right"), "3px");
    let (doc, map) = styled("<!doctype html><body leftmargin=2>t", true);
    assert_eq!(get(&doc, &map, "body", "margin-left"), "2px");
    assert_eq!(get(&doc, &map, "body", "margin-right"), "2px");
    let (doc, map) = styled(
        "<!doctype html><body rightmargin=100 bottommargin=100>t",
        true,
    );
    assert_eq!(get(&doc, &map, "body", "margin-right"), "8px");
    assert_eq!(get(&doc, &map, "body", "margin-bottom"), "8px");
    // Un hint es de autor, así que le gana a la hoja UA, y una regla de autor le gana al hint
    // (CSS Cascade 5 §6.5: un origen propio entre el de usuario y el de autor).
    let (doc, map) = styled(
        "<!doctype html><style>body { margin-top: 1px }</style><body topmargin=5>t",
        true,
    );
    assert_eq!(get(&doc, &map, "body", "margin-top"), "1px");
}

/// Caso 4: los presentational hints quedan debajo de las reglas de autor en `@layer` (CSS
/// Cascade 5 §6.5: un origen propio entre el de usuario y el de autor). El logo de "Ecosistema"
/// de asimetrix.co: `<img width=2932 height=1368 class="w-full h-auto">` con las utilidades de
/// Tailwind en `@layer utilities`.
#[test]
fn presentational_hints_lose_to_layered_author_rules() {
    let html = "<!doctype html><style>@layer utilities { .w-full { width: 100% } \
        .h-auto { height: auto } } @layer x { #r1 { width: revert-layer } \
        #r2 { width: revert } }</style>\
        <div style=\"width:438px\"><img class=\"w-full h-auto\" width=2932 height=1368>\
        <img id=sin width=2932 height=1368><img id=r1 width=44><img id=r2 width=44>\
        <img id=pct width=50% height=10><iframe class=w-full width=640 height=360></iframe></div>";
    let (doc, map) = styled(html, true);
    assert_eq!(get(&doc, &map, "img.w-full", "width"), "100%");
    assert_eq!(get(&doc, &map, "img.w-full", "height"), "auto");
    assert_eq!(get(&doc, &map, "iframe", "width"), "100%");
    assert_eq!(get(&doc, &map, "iframe", "height"), "360px");
    // Sin regla de autor, el hint se aplica.
    assert_eq!(get(&doc, &map, "#sin", "width"), "2932px");
    // `revert-layer` en la única capa de autor vuelve al hint; `revert` lo salta (para `revert`
    // es del origen de autor) y queda el valor de la hoja UA (WPT
    // `css/css-cascade/presentational-hints-rollback.html`).
    assert_eq!(get(&doc, &map, "#r1", "width"), "44px");
    assert_eq!(get(&doc, &map, "#r2", "width"), "auto");
    // HTML §15.4.3: el par width/height da `aspect-ratio: auto w / h`, salvo con porcentaje.
    let ratio = get(&doc, &map, "#sin", "aspect-ratio");
    assert!(ratio.starts_with("auto 2.14"), "{ratio}");
    assert_eq!(get(&doc, &map, "#pct", "aspect-ratio"), "auto");
    // El estilo compartido: dos elementos iguales sin hints siguen compartiendo, y uno con
    // hints no toma el estilo de su hermano.
    let (doc, map) = styled(
        "<!doctype html><style>@layer u { .w { width: 10px } }</style>\
         <p><img class=w id=a><img class=w id=b><img class=w id=c width=99></p>",
        true,
    );
    assert_eq!(get(&doc, &map, "#a", "width"), "10px");
    assert_eq!(get(&doc, &map, "#b", "width"), "10px");
    assert_eq!(get(&doc, &map, "#c", "width"), "10px");
    assert_eq!(get(&doc, &map, "#c", "aspect-ratio"), "auto");
}
