//! E4: compartir estilo entre elementos (mismo padre y mismas declaraciones emparejadas, sin
//! declaraciones propias) da exactamente los mismos valores computados que calcularlos todos.

use bf_css::media::MediaContext;
use bf_css::selectors::MatchContext;
use bf_css::stylesheet::{Origin, Stylesheet};
use bf_html::{ParseOptions, parse_document};
use bf_style::cascade::set_style_sharing;
use bf_style::style_document;

const HTML: &str = "<!doctype html><style>
  ul { font-size: 14px; color: navy } li { margin: 2px } li:nth-child(2n) { color: red }
  li:first-child { font-weight: bold } .a { padding: 1px } .a .b { color: teal }
  p:empty { display: none } td { padding: 3px } tr:last-child td { border: 1px solid }
  div > span + span { text-decoration: underline } [data-x] { order: 2 }
  .v { --c: 3px; margin: var(--c) } li::marker { color: gray } q::before { content: '«' }
</style>
<ul><li>a</li><li>b</li><li>c</li><li class=a>d<span class=b>e</span></li><li>f</li></ul>
<ul><li>a</li><li>b</li></ul>
<div><span>1</span><span>2</span><span>3</span><span style='color: blue'>4</span></div>
<div class=a><p></p><p>x</p><p></p><font color=red>f</font><font color=blue>g</font></div>
<table><tr><td>1</td><td>2</td></tr><tr><td>3</td><td>4</td></tr></table>
<div class=v><i>x</i><i>y</i><q>z</q><q>w</q></div>
<span data-x>1</span><span data-x>2</span>";

#[test]
fn shared_styles_match_computed_styles() {
    let (doc, _) = parse_document(HTML, &ParseOptions::default());
    let sheets: [&Stylesheet; 0] = [];
    let media = MediaContext::default();
    let ctx = MatchContext::for_document(&doc);
    set_style_sharing(Some(false));
    let full = style_document(&doc, &sheets, &media, &ctx);
    set_style_sharing(Some(true));
    let shared = style_document(&doc, &sheets, &media, &ctx);
    set_style_sharing(None);
    assert_eq!(full.len(), shared.len());
    for (key, style) in &full {
        assert_eq!(Some(style), shared.get(key), "{key:?}");
    }
    // Y de verdad se compartió algo (los `li` iguales de la primera lista, por ejemplo).
    let distinct: std::collections::HashSet<_> = shared
        .values()
        .map(|s| std::sync::Arc::as_ptr(s) as usize)
        .collect();
    assert!(distinct.len() < shared.len(), "ningún estilo compartido");
    let _ = Origin::Author;
}

/// E4: la cascada repartida en hilos (`style_document_parallel`) da los mismos estilos que en
/// orden, en un documento con bastantes subárboles para repartir.
#[test]
fn parallel_cascade_matches_sequential() {
    let mut html = String::from(
        "<!doctype html><style>
          section { padding: 1em } section:nth-child(odd) { color: red }
          li { margin: 1px } li:nth-child(3n) { font-size: 1.2em } .k { font-weight: bold }
          ul > li:last-child::after { content: '.' } p:first-of-type { margin: 0 }
          html { font-size: 20px } li { padding: 1rem }
        </style>",
    );
    for i in 0..40 {
        html.push_str(&format!("<section class=s{i}><p>p</p><ul>"));
        for j in 0..12 {
            let k = if (i + j) % 5 == 0 { " class=k" } else { "" };
            html.push_str(&format!("<li{k}>item <b>{j}</b></li>"));
        }
        html.push_str("</ul></section>");
    }
    let (doc, _) = parse_document(&html, &ParseOptions::default());
    let media = MediaContext::default();
    let ctx = MatchContext::for_document(&doc);
    let sheets = bf_style::document_stylesheets(&doc, &media, None);
    let one = bf_style::style_document_with_sheets(&doc, &sheets, &[], &media, &ctx, None);
    for threads in [2, 3, 4, 8] {
        let many =
            bf_style::style_document_parallel(&doc, &sheets, &[], &media, &ctx, None, threads);
        assert_eq!(one.len(), many.len(), "{threads} hilos");
        for (key, style) in &one {
            assert_eq!(Some(style), many.get(key), "{threads} hilos: {key:?}");
        }
    }
}
