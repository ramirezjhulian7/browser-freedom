//! Casos HTML + CSS → valor computado esperado. Cubre la hoja UA, presentational hints,
//! cascada entre orígenes, `!important`, capas, keywords globales, `var()`, `calc()`,
//! shorthands y quirks mode.

use bf_css::media::MediaContext;
use bf_css::selectors::{MatchContext, NamespaceMap, PseudoElement, matches, parse_selector_list};
use bf_dom::{Document, NodeId};
use bf_html::{ParseOptions, parse_document};
use bf_style::{StyleMap, style_document};

fn styled(html: &str) -> (Document, StyleMap) {
    let (doc, _) = parse_document(html, &ParseOptions::default());
    let ctx = MatchContext::for_document(&doc);
    let map = style_document(&doc, &[], &MediaContext::default(), &ctx);
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

fn computed(html: &str, selector: &str, property: &str) -> String {
    let (doc, map) = styled(html);
    let node = find(&doc, selector);
    map[&(node, None)]
        .get_css(property)
        .unwrap_or_else(|| panic!("propiedad desconocida {property}"))
}

const STD: &str = "<!DOCTYPE html>";

#[test]
fn computed_values() {
    let cases: &[(&str, &str, &str, &str)] = &[
        // --- Hoja UA (HTML §15) ---
        ("<div hidden>x</div>", "div", "display", "none"),
        ("<h1>t</h1>", "h1", "font-size", "32px"),
        ("<h2>t</h2>", "h2", "font-size", "24px"),
        ("<h1>t</h1>", "h1", "margin-top", "21.44px"),
        ("<h1>t</h1>", "h1", "font-weight", "700"),
        ("<p>x</p>", "p", "margin-top", "16px"),
        ("<p>x</p>", "p", "display", "block"),
        ("<b>x</b>", "b", "font-weight", "700"),
        ("<b><strong>x</strong></b>", "strong", "font-weight", "900"),
        ("<em>x</em>", "em", "font-style", "italic"),
        ("<pre>x</pre>", "pre", "white-space", "pre"),
        ("<pre>x</pre>", "pre", "font-family", "monospace"),
        ("<ul><li>x</ul>", "li", "display", "list-item"),
        ("<ul><li>x</ul>", "ul", "padding-left", "40px"),
        (
            "<ul><li><ul id=in><li>y</ul></ul>",
            "#in",
            "margin-top",
            "0px",
        ),
        (
            "<ul><li><ul id=in><li>y</ul></ul>",
            "#in",
            "list-style-type",
            "circle",
        ),
        ("<ol><li>x</ol>", "ol", "list-style-type", "decimal"),
        ("<table><tr><td>x</table>", "table", "display", "table"),
        ("<table><tr><td>x</table>", "td", "padding-top", "1px"),
        ("<a href=x>l</a>", "a", "color", "rgb(0, 0, 238)"),
        ("<a href=x>l</a>", "a", "text-decoration-line", "underline"),
        ("<a>l</a>", "a", "text-decoration-line", "none"),
        ("<title>t</title>", "head", "display", "none"),
        ("<span>x</span>", "span", "display", "inline"),
        ("<hr>", "hr", "border-top-width", "1px"),
        ("<hr>", "hr", "border-top-style", "inset"),
        ("<hr>", "hr", "color", "rgb(128, 128, 128)"),
        ("<input type=hidden>", "input", "display", "none"),
        (
            "<blockquote>x</blockquote>",
            "blockquote",
            "margin-left",
            "40px",
        ),
        ("<small>x</small>", "small", "font-size", "13.333px"),
        ("<big>x</big>", "big", "font-size", "19.2px"),
        ("<sub>x</sub>", "sub", "vertical-align", "sub"),
        ("<html>", "html", "display", "block"),
        // --- Presentational hints ---
        ("<body>", "body", "margin-left", "8px"),
        ("<body marginwidth=20>", "body", "margin-left", "20px"),
        ("<body marginwidth=20>", "body", "margin-top", "8px"),
        ("<body text=red>", "body", "color", "rgb(255, 0, 0)"),
        ("<div align=center>x</div>", "div", "text-align", "center"),
        (
            "<font color=red>x</font>",
            "font",
            "color",
            "rgb(255, 0, 0)",
        ),
        ("<font size=7>x</font>", "font", "font-size", "48px"),
        (
            "<font face=Arial>x</font>",
            "font",
            "font-family",
            "\"Arial\"",
        ),
        ("<img width=100 height=50>", "img", "width", "100px"),
        ("<img width=50%>", "img", "width", "50%"),
        (
            "<table border=2><tr><td>x</table>",
            "table",
            "border-top-width",
            "2px",
        ),
        (
            "<table><tr><td bgcolor=#00ff00>x</table>",
            "td",
            "background-color",
            "rgb(0, 255, 0)",
        ),
        (
            "<table cellpadding=5><tr><td>x</table>",
            "td",
            "padding-left",
            "5px",
        ),
        ("<p align=right>x</p>", "p", "text-align", "right"),
        // Un hint (especificidad 0) pierde contra cualquier regla de autor que coincida.
        (
            "<style>div{text-align:left}</style><div align=center>x</div>",
            "div",
            "text-align",
            "left",
        ),
        (
            "<style>*{text-align:left}</style><div align=center>x</div>",
            "div",
            "text-align",
            "left",
        ),
        // --- Cascada ---
        (
            "<style>#a{color:red} div{color:blue}</style><div id=a>x</div>",
            "div",
            "color",
            "rgb(255, 0, 0)",
        ),
        (
            "<style>.x{color:red} .x{color:blue}</style><div class=x>x</div>",
            "div",
            "color",
            "rgb(0, 0, 255)",
        ),
        (
            "<style>div{color:red !important} #a{color:blue}</style><div id=a>x</div>",
            "div",
            "color",
            "rgb(255, 0, 0)",
        ),
        (
            "<style>#a{color:red}</style><div id=a style='color:green'>x</div>",
            "div",
            "color",
            "rgb(0, 128, 0)",
        ),
        (
            "<style>#a{color:red !important}</style><div id=a style='color:green'>x</div>",
            "div",
            "color",
            "rgb(255, 0, 0)",
        ),
        (
            "<style>#a{color:red !important}</style><div id=a style='color:green !important'>x</div>",
            "div",
            "color",
            "rgb(0, 128, 0)",
        ),
        // `!important` de la UA gana a `!important` de autor.
        (
            "<style>input{display:block !important}</style><input type=hidden>",
            "input",
            "display",
            "none",
        ),
        (
            "<style>@layer a, b; @layer b { div{color:red} } @layer a { div{color:blue} }</style><div>x</div>",
            "div",
            "color",
            "rgb(255, 0, 0)",
        ),
        (
            "<style>@layer a { #x{color:red} } div{color:blue}</style><div id=x>x</div>",
            "div",
            "color",
            "rgb(0, 0, 255)",
        ),
        (
            "<style>@layer a { div{color:red !important} } div{color:blue !important}</style><div>x</div>",
            "div",
            "color",
            "rgb(255, 0, 0)",
        ),
        (
            "<style>@layer a { @layer b { div{color:red} } div{color:blue} }</style><div>x</div>",
            "div",
            "color",
            "rgb(0, 0, 255)",
        ),
        // --- Keywords globales ---
        (
            "<style>div{border-top:5px solid} span{border-top-width:inherit; border-top-style:solid}</style><div><span>x</span></div>",
            "span",
            "border-top-width",
            "5px",
        ),
        (
            "<style>div{color:red} span{color:unset}</style><div><span>x</span></div>",
            "span",
            "color",
            "rgb(255, 0, 0)",
        ),
        (
            "<style>div{margin-left:5px} span{margin-left:inherit}</style><div><span>x</span></div>",
            "span",
            "margin-left",
            "5px",
        ),
        (
            "<style>div{margin-left:5px} span{margin-left:unset}</style><div><span>x</span></div>",
            "span",
            "margin-left",
            "0px",
        ),
        (
            "<style>div{color:red} span{color:initial}</style><div><span>x</span></div>",
            "span",
            "color",
            "rgb(0, 0, 0)",
        ),
        (
            "<style>p{margin-top:5px} p{margin-top:revert}</style><p>x</p>",
            "p",
            "margin-top",
            "16px",
        ),
        (
            "<style>div{display:revert}</style><div>x</div>",
            "div",
            "display",
            "block",
        ),
        (
            "<style>@layer base { p { margin-top: 3px } } @layer top { p { margin-top: revert-layer } }</style><p>x</p>",
            "p",
            "margin-top",
            "3px",
        ),
        (
            "<style>@layer top { p { margin-top: revert-layer } }</style><p>x</p>",
            "p",
            "margin-top",
            "16px",
        ),
        // --- Custom properties y var() ---
        (
            "<style>:root{--c: red} div{color:var(--c)}</style><div>x</div>",
            "div",
            "color",
            "rgb(255, 0, 0)",
        ),
        (
            "<style>div{color:var(--missing, blue)}</style><div>x</div>",
            "div",
            "color",
            "rgb(0, 0, 255)",
        ),
        (
            "<style>div{--a: var(--b); --b: var(--a); color: var(--a, green)}</style><div>x</div>",
            "div",
            "color",
            "rgb(0, 128, 0)",
        ),
        (
            "<style>div{color:red} span{color: var(--nope)}</style><div><span>x</span></div>",
            "span",
            "color",
            "rgb(255, 0, 0)",
        ),
        (
            "<style>div{--m: 1px 2px; margin: var(--m)}</style><div>x</div>",
            "div",
            "margin-right",
            "2px",
        ),
        (
            "<style>div{--x: 7px} span{width:var(--x)}</style><div><span>x</span></div>",
            "span",
            "width",
            "7px",
        ),
        (
            "<style>div{--x: 7px}</style><div><span>x</span></div>",
            "span",
            "--x",
            "7px",
        ),
        (
            "<style>div{--a: 1px; --b: calc(var(--a) * 3); width: var(--b)}</style><div>x</div>",
            "div",
            "width",
            "3px",
        ),
        // --- calc() y unidades ---
        (
            "<style>div{width: calc(100px + 2em)}</style><div>x</div>",
            "div",
            "width",
            "132px",
        ),
        (
            "<style>div{width: calc(50% - 10px)}</style><div>x</div>",
            "div",
            "width",
            "calc(50% + -10px)",
        ),
        (
            "<style>div{width: min(10px, 2em)}</style><div>x</div>",
            "div",
            "width",
            "10px",
        ),
        (
            "<style>div{font-size: 20px; padding-left: 2em}</style><div>x</div>",
            "div",
            "padding-left",
            "40px",
        ),
        (
            "<style>html{font-size: 10px} div{width: 3rem}</style><div>x</div>",
            "div",
            "width",
            "30px",
        ),
        (
            "<style>div{font-size:20px} span{font-size:150%}</style><div><span>x</span></div>",
            "span",
            "font-size",
            "30px",
        ),
        (
            "<style>div{font-size:20px} span{font-size:2em}</style><div><span>x</span></div>",
            "span",
            "font-size",
            "40px",
        ),
        (
            "<style>div{line-height: 20px; height: 2lh}</style><div>x</div>",
            "div",
            "height",
            "40px",
        ),
        (
            "<style>div{width: 10vw}</style><div>x</div>",
            "div",
            "width",
            "128px",
        ),
        (
            "<style>div{line-height: 150%; font-size: 10px}</style><div>x</div>",
            "div",
            "line-height",
            "15px",
        ),
        (
            "<style>div{line-height: 1.5}</style><div>x</div>",
            "div",
            "line-height",
            "1.5",
        ),
        (
            "<style>div{font-size: x-large}</style><div>x</div>",
            "div",
            "font-size",
            "24px",
        ),
        // --- Shorthands ---
        (
            "<style>div{margin: 1px 2px 3px 4px}</style><div>x</div>",
            "div",
            "margin-left",
            "4px",
        ),
        (
            "<style>div{border: 1px solid red}</style><div>x</div>",
            "div",
            "border-left-color",
            "rgb(255, 0, 0)",
        ),
        (
            "<style>div{font: italic bold 12px/30px Georgia, serif}</style><div>x</div>",
            "div",
            "line-height",
            "30px",
        ),
        (
            "<style>div{font: italic bold 12px/30px Georgia, serif}</style><div>x</div>",
            "div",
            "font-family",
            "\"Georgia\", serif",
        ),
        (
            "<style>div{font-weight:900; font: 12px serif}</style><div>x</div>",
            "div",
            "font-weight",
            "400",
        ),
        (
            "<style>div{font: 12px system-ui}</style><div>x</div>",
            "div",
            "font-family",
            "system-ui",
        ),
        (
            "<style>div{color: notacolor; width: 10px}</style><div>x</div>",
            "div",
            "width",
            "10px",
        ),
        (
            "<style>div{color: notacolor; width: 10px}</style><div>x</div>",
            "div",
            "color",
            "rgb(0, 0, 0)",
        ),
        (
            "<style>div{margin-inline-start: 7px}</style><div>x</div>",
            "div",
            "margin-left",
            "7px",
        ),
        (
            "<style>div{padding-block: 1px 2px}</style><div>x</div>",
            "div",
            "padding-bottom",
            "2px",
        ),
        (
            "<style>div{inset: 1px auto}</style><div>x</div>",
            "div",
            "right",
            "auto",
        ),
        (
            "<style>div{overflow: hidden scroll}</style><div>x</div>",
            "div",
            "overflow-y",
            "scroll",
        ),
        (
            "<style>div{flex: 2 3 10px}</style><div>x</div>",
            "div",
            "flex-shrink",
            "3",
        ),
        (
            "<style>div{list-style: square inside}</style><div>x</div>",
            "div",
            "list-style-position",
            "inside",
        ),
        (
            "<style>div{text-decoration: underline wavy}</style><div>x</div>",
            "div",
            "text-decoration-style",
            "wavy",
        ),
        (
            "<style>div{background: url(a.png) red}</style><div>x</div>",
            "div",
            "background-color",
            "rgb(255, 0, 0)",
        ),
        // --- Cómputo ---
        // El ancho computado no depende de `border-style` (CSS Backgrounds 3, resolución de
        // 2023); el usado es 0 con `none`.
        (
            "<style>div{border-top-width:5px}</style><div>x</div>",
            "div",
            "border-top-width",
            "5px",
        ),
        (
            "<style>body{border-width:2em} p{border-width:inherit}</style><p>x</p>",
            "p",
            "border-left-width",
            "32px",
        ),
        (
            "<style>span{float:left}</style><span>x</span>",
            "span",
            "display",
            "block",
        ),
        (
            "<style>span{position:absolute; float:left}</style><span>x</span>",
            "span",
            "float",
            "none",
        ),
        (
            "<style>span{display:inline-flex; position:fixed}</style><span>x</span>",
            "span",
            "display",
            "flex",
        ),
        (
            "<style>div{color:red; border-top-color: currentcolor}</style><div>x</div>",
            "div",
            "border-top-color",
            "currentcolor",
        ),
        (
            "<style>div{color:red} span{color:currentcolor}</style><div><span>x</span></div>",
            "span",
            "color",
            "rgb(255, 0, 0)",
        ),
        (
            "<style>div{opacity: 2}</style><div>x</div>",
            "div",
            "opacity",
            "1",
        ),
        (
            "<style>div{letter-spacing: 1em}</style><div>x</div>",
            "div",
            "letter-spacing",
            "16px",
        ),
        (
            "<ul style='text-align:right'><li>x</ul>",
            "li",
            "text-align",
            "right",
        ),
        ("<div dir=rtl><p>x</p></div>", "p", "direction", "rtl"),
        // --- Media y @supports ---
        (
            "<style media=print>div{color:red}</style><div>x</div>",
            "div",
            "color",
            "rgb(0, 0, 0)",
        ),
        (
            "<style>@media (min-width: 100px){div{color:red}}</style><div>x</div>",
            "div",
            "color",
            "rgb(255, 0, 0)",
        ),
        (
            "<style>@supports (display: flex){div{color:red}}</style><div>x</div>",
            "div",
            "color",
            "rgb(255, 0, 0)",
        ),
        (
            "<style>@supports (display: nonsense){div{color:red}}</style><div>x</div>",
            "div",
            "color",
            "rgb(0, 0, 0)",
        ),
        (
            "<style>div { color: blue; & > span { color: red } }</style><div><span>x</span></div>",
            "span",
            "color",
            "rgb(255, 0, 0)",
        ),
    ];
    let mut failures = Vec::new();
    for (html, selector, property, expected) in cases {
        let html = format!("{STD}{html}");
        let actual = computed(&html, selector, property);
        if actual != *expected {
            failures.push(format!(
                "{html} | {selector} {{ {property} }}: esperado {expected:?}, obtenido {actual:?}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} casos fallaron:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(cases.len() >= 50);
}

#[test]
fn quirks_mode() {
    // §15.3.9: en quirks mode el primer hijo con márgenes por defecto del body no tiene
    // margin-top.
    assert_eq!(computed("<body><p>x</p>", "p", "margin-top"), "0px");
    assert_eq!(
        computed("<!DOCTYPE html><body><p>x</p>", "p", "margin-top"),
        "16px"
    );
    // Bloques "in quirks mode" de §15.3.3.
    assert_eq!(computed("<form></form>", "form", "margin-bottom"), "16px");
    assert_eq!(
        computed("<!DOCTYPE html><form></form>", "form", "margin-bottom"),
        "0px"
    );
}

#[test]
fn pseudo_elements() {
    let (doc, map) = styled(
        "<!DOCTYPE html><style>p::before{content:'x' attr(id); color: red} p::after{color:blue}</style><p id=a>t</p><ol><li>i</ol>",
    );
    let p = find(&doc, "p");
    let before = &map[&(p, Some(PseudoElement::Before))];
    assert_eq!(before.get_css("content").as_deref(), Some("\"x\" attr(id)"));
    assert_eq!(before.get_css("color").as_deref(), Some("rgb(255, 0, 0)"));
    // Sin `content`, `::after` no se genera.
    assert!(!map.contains_key(&(p, Some(PseudoElement::After))));
    let li = find(&doc, "li");
    assert!(map.contains_key(&(li, Some(PseudoElement::Marker))));
}

#[test]
fn supports_callback() {
    use bf_css::syntax::parse_component_value_list;
    let v = |s: &str| parse_component_value_list(s);
    assert!(bf_style::is_supported("display", &v("flex")));
    assert!(bf_style::is_supported("margin", &v("1px auto")));
    assert!(!bf_style::is_supported("display", &v("nope")));
    assert!(!bf_style::is_supported("nope", &v("1px")));
    assert!(bf_style::is_supported("color", &v("var(--x)")));
    assert!(bf_style::is_supported("--anything", &v("{}")));
}
