//! Tabla de casos de Selectors 4: parseo, matching contra documentos parseados con
//! `bf_html::parse_document`, especificidad y estado dinámico. Los resultados de matching son
//! los `id` de los elementos que coinciden (solo se consideran elementos con `id`), en orden
//! de documento.

use bf_css::media::MediaContext;
use bf_css::selectors::{
    MatchContext, NamespaceMap, Specificity, matches, matching_specificity, parse_selector_list,
};
use bf_css::stylesheet::{Origin, Stylesheet, matching_declarations};
use bf_css::syntax::{ComponentValue, TokenKind};
use bf_dom::{Document, NodeId};
use bf_html::{ParseOptions, parse_document};

const DOC: &str = r#"<!DOCTYPE html>
<div id=root class="a b">
  <p id=p1 class=x lang=en-US>one</p>
  <p id=p2 class="x y">two <span id=s1>s</span></p>
  <ul id=list>
    <li id=l1 class=item>1</li>
    <li id=l2>2</li>
    <li id=l3 class=item>3</li>
    <li id=l4 class=item>4</li>
  </ul>
  <a id=link href="https://example.com/page.html" hreflang=EN>x</a>
  <a id=nolink>y</a>
  <input id=i1 type=checkbox checked disabled>
  <input id=i2 type=text required placeholder=hi>
  <input id=i3 readonly>
  <textarea id=t1></textarea>
  <section id=empty1></section>
  <section id=empty2> <!-- c --> </section>
  <section id=nonempty>x</section>
  <div id=d1 data-x="foo-bar baz" title="Hello World"><em id=e1>em</em></div>
  <fieldset id=fs disabled><legend id=lg><input id=i4></legend><input id=i5></fieldset>
  <div lang=fr id=fr><p id=frp>bonjour</p><p id=de lang=de-Latn-DE>hallo</p></div>
  <div id=rtl dir=rtl><span id=rtlspan>x</span></div>
  <svg id=svg1><rect id="rect1"/><foreignObject id=fo></foreignObject></svg>
  <my-el id=custom></my-el>
  <select id=sel><option id=o1>a</option><option id=o2 selected>b</option><optgroup id=og disabled><option id=o3>c</option></optgroup></select>
</div>"#;

fn doc(html: &str) -> Document {
    parse_document(html, &ParseOptions::default()).0
}

fn elements_with_id(doc: &Document) -> Vec<(NodeId, String)> {
    doc.inclusive_descendants(doc.root())
        .into_iter()
        .filter_map(|n| {
            doc.element(n)
                .and_then(|e| e.attr("id"))
                .map(|id| (n, id.to_owned()))
        })
        .collect()
}

fn select_in(doc: &Document, selector: &str, ctx: &MatchContext, ns: &NamespaceMap) -> Vec<String> {
    let list = parse_selector_list(selector, ns)
        .unwrap_or_else(|_| panic!("selector inválido: {selector}"));
    elements_with_id(doc)
        .into_iter()
        .filter(|(n, _)| list.0.iter().any(|s| matches(s, doc, *n, ctx)))
        .map(|(_, id)| id)
        .collect()
}

#[test]
fn matching_table() {
    let d = doc(DOC);
    let ctx = MatchContext::for_document(&d);
    let ns = NamespaceMap::default();
    let cases: &[(&str, &[&str])] = &[
        ("p", &["p1", "p2", "frp", "de"]),
        ("#p1", &["p1"]),
        ("#p\\31", &["p1"]),
        (".x", &["p1", "p2"]),
        (".x.y", &["p2"]),
        (".a.b", &["root"]),
        ("div.a > #p1", &["p1"]),
        ("p.x:not(.y)", &["p1"]),
        ("div > p", &["p1", "p2", "frp", "de"]),
        ("#root > ul > li", &["l1", "l2", "l3", "l4"]),
        ("li + li", &["l2", "l3", "l4"]),
        ("#l1 ~ li", &["l2", "l3", "l4"]),
        ("#list li:first-child", &["l1"]),
        ("li:last-child", &["l4"]),
        ("li:nth-child(2n+1)", &["l1", "l3"]),
        ("li:nth-child(even)", &["l2", "l4"]),
        ("li:nth-last-child(1)", &["l4"]),
        ("li:nth-child(2 of .item)", &["l3"]),
        ("li:nth-child(-n+2 of .item)", &["l1", "l3"]),
        ("li:nth-last-child(1 of .item)", &["l4"]),
        ("p:nth-of-type(2)", &["p2", "de"]),
        ("p:first-of-type", &["p1", "frp"]),
        ("em:only-child", &["e1"]),
        ("em:only-of-type", &["e1"]),
        ("span:only-child", &["s1", "rtlspan"]),
        // Selectors 4: los nodos de texto con solo whitespace y los comentarios no cuentan.
        ("section:empty", &["empty1", "empty2"]),
        ("[data-x]", &["d1"]),
        ("[DATA-X]", &["d1"]),
        ("[data-x=\"foo-bar baz\"]", &["d1"]),
        ("[data-x~=baz]", &["d1"]),
        ("[data-x|=foo]", &["d1"]),
        ("[data-x^=foo]", &["d1"]),
        ("[data-x$=baz]", &["d1"]),
        ("[data-x*=\"bar b\"]", &["d1"]),
        ("[data-x~=\"\"]", &[]),
        ("[title=\"hello world\" i]", &["d1"]),
        ("[title=\"hello world\"]", &[]),
        ("[title=\"Hello World\" s]", &["d1"]),
        // `hreflang` es uno de los atributos HTML con valor case-insensitive.
        ("[hreflang=en]", &["link"]),
        ("[hreflang=en s]", &[]),
        ("a:link", &["link"]),
        ("a:any-link", &["link"]),
        ("a:not(:any-link)", &["nolink"]),
        (":checked", &["i1", "o2"]),
        ("input:disabled", &["i1", "i5"]),
        ("input:enabled", &["i2", "i3", "i4"]),
        (":required", &["i2"]),
        ("input:optional", &["i1", "i3", "i4", "i5"]),
        ("input:read-only", &["i1", "i3", "i5"]),
        (":read-write", &["i2", "t1", "i4"]),
        (":placeholder-shown", &["i2"]),
        (":lang(en)", &["p1"]),
        (":lang(fr)", &["fr", "frp"]),
        (":lang(de-DE)", &["de"]),
        (":lang(\"*-Latn\")", &["de"]),
        (":lang(en, fr)", &["p1", "fr", "frp"]),
        (":dir(rtl)", &["rtl", "rtlspan"]),
        ("rect", &["rect1"]),
        ("foreignObject", &["fo"]),
        // Los nombres de elementos SVG distinguen mayúsculas.
        ("foreignobject", &[]),
        ("my-el:defined", &[]),
        ("div:defined#root", &["root"]),
        (":is(#p1, #l2)", &["p1", "l2"]),
        (":is(ul, section) > li", &["l1", "l2", "l3", "l4"]),
        // Forgiving: el selector desconocido se descarta.
        (":is(.x, :unknown-pseudo)", &["p1", "p2"]),
        (":where(.y)", &["p2"]),
        ("div:has(> em)", &["d1"]),
        ("div:has(p)", &["root", "fr"]),
        ("li:has(+ .item)", &["l2", "l3"]),
        ("li:has(~ #l4)", &["l1", "l2", "l3"]),
        ("ul:has(li.item:nth-child(3))", &["list"]),
        ("div:not(:has(p, em))", &["rtl"]),
        ("#root :is(p, li):not(.x, .item)", &["l2", "frp", "de"]),
        (":not(div) > span", &["s1"]),
        ("option:checked", &["o2"]),
        ("option:disabled", &["o3"]),
        ("optgroup:disabled", &["og"]),
        ("select > option", &["o1", "o2"]),
        ("#fs input", &["i4", "i5"]),
        ("legend > input", &["i4"]),
        // Un selector con pseudo-elemento no coincide con el elemento.
        ("p::before", &[]),
        ("*|p", &["p1", "p2", "frp", "de"]),
        ("|p", &[]),
        ("P", &["p1", "p2", "frp", "de"]),
        // `&` fuera de una regla anidada equivale a `:scope` (aquí, `:root`).
        ("& p", &["p1", "p2", "frp", "de"]),
    ];
    let mut failures = Vec::new();
    for (selector, expected) in cases {
        let actual = select_in(&d, selector, &ctx, &ns);
        if actual != *expected {
            failures.push(format!(
                "{selector}: esperado {expected:?}, obtenido {actual:?}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} casos fallaron:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(cases.len() >= 60);
}

#[test]
fn invalid_selectors() {
    let ns = NamespaceMap::default();
    let invalid = [
        "",
        " ",
        "p >",
        "> p",
        "p ++ a",
        "#1a",
        ".",
        "[",
        "[a=]",
        "[a=b c]",
        "[a=b x]",
        ":unknown",
        ":not()",
        ":not(::before)",
        ":not(.a, :unknown)",
        ":has(:has(a))",
        ":has(::before)",
        "a::before b",
        "a::before.x",
        "::nope",
        ":nth-child(foo)",
        ":nth-child(2n of)",
        ":nth-child(2n+)",
        "ns|p",
        "p, ",
        ",p",
        "p,, a",
        "a:lang()",
        "a:dir(up)",
        "li:nth-child(2n of ::before)",
    ];
    for selector in invalid {
        assert!(
            parse_selector_list(selector, &ns).is_err(),
            "debería ser inválido: {selector:?}"
        );
    }
    // Válidos aunque raros.
    for selector in [
        ":is()",
        ":where()",
        "a:before",
        "*|*",
        "|*",
        "[a|=b]",
        ":is(:unknown)",
    ] {
        assert!(
            parse_selector_list(selector, &ns).is_ok(),
            "debería ser válido: {selector:?}"
        );
    }
}

#[test]
fn specificity() {
    let ns = NamespaceMap::default();
    let cases: &[(&str, (u32, u32, u32))] = &[
        ("*", (0, 0, 0)),
        ("li", (0, 0, 1)),
        ("ul li", (0, 0, 2)),
        ("ul ol+li", (0, 0, 3)),
        ("h1 + *[rel=up]", (0, 1, 1)),
        ("ul ol li.red", (0, 1, 3)),
        ("li.red.level", (0, 2, 1)),
        ("#x34y", (1, 0, 0)),
        ("#s12:not(FOO)", (1, 0, 1)),
        (".foo :is(.bar, #baz)", (1, 1, 0)),
        (":is(em, #foo)", (1, 0, 0)),
        (".qux:where(em, #foo#bar#baz)", (0, 1, 0)),
        (":where(#a#b) .c", (0, 1, 0)),
        (":nth-child(even of li, .item)", (0, 2, 0)),
        ("li:nth-child(2n+1)", (0, 1, 1)),
        (":not(em, strong#foo)", (1, 0, 1)),
        (":has(> .a, #b)", (1, 0, 0)),
        ("a::before", (0, 0, 2)),
        ("a:before", (0, 0, 2)),
        (":is(.a, :unknown)", (0, 1, 0)),
        (":is()", (0, 0, 0)),
        ("#a#a", (2, 0, 0)),
    ];
    for (selector, (a, b, c)) in cases {
        let list = parse_selector_list(selector, &ns).unwrap();
        assert_eq!(
            list.0[0].specificity(),
            Specificity(*a, *b, *c),
            "especificidad de {selector}"
        );
    }
    // Para una lista, la del selector más específico que coincide.
    let d = doc(DOC);
    let ctx = MatchContext::for_document(&d);
    let root = elements_with_id(&d)
        .into_iter()
        .find(|(_, id)| id == "root")
        .unwrap()
        .0;
    let list = parse_selector_list(".a, #root, span", &ns).unwrap();
    assert_eq!(
        matching_specificity(&list, &d, root, None, &ctx),
        Some(Specificity(1, 0, 0))
    );
    let list = parse_selector_list("span, em", &ns).unwrap();
    assert_eq!(matching_specificity(&list, &d, root, None, &ctx), None);
}

fn node_by_id(doc: &Document, id: &str) -> NodeId {
    elements_with_id(doc)
        .into_iter()
        .find(|(_, i)| i == id)
        .unwrap_or_else(|| panic!("no hay #{id}"))
        .0
}

#[test]
fn dynamic_state_and_scope() {
    let d = doc(DOC);
    let ns = NamespaceMap::default();
    let mut ctx = MatchContext::for_document(&d);
    ctx.hover.insert(node_by_id(&d, "d1"));
    ctx.focus = Some(node_by_id(&d, "i2"));
    ctx.target = Some(node_by_id(&d, "p2"));
    ctx.visited_links.insert(node_by_id(&d, "link"));
    ctx.scope = Some(node_by_id(&d, "list"));
    assert_eq!(select_in(&d, "div:hover", &ctx, &ns), ["d1"]);
    assert_eq!(select_in(&d, ":focus", &ctx, &ns), ["i2"]);
    assert_eq!(
        select_in(&d, ":focus-visible", &ctx, &ns),
        Vec::<String>::new()
    );
    assert_eq!(select_in(&d, ":focus-within", &ctx, &ns), ["root", "i2"]);
    assert_eq!(select_in(&d, ":target", &ctx, &ns), ["p2"]);
    assert_eq!(select_in(&d, ":visited", &ctx, &ns), ["link"]);
    assert_eq!(select_in(&d, ":link", &ctx, &ns), Vec::<String>::new());
    assert_eq!(
        select_in(&d, ":scope > li", &ctx, &ns),
        ["l1", "l2", "l3", "l4"]
    );
    assert_eq!(select_in(&d, ":active", &ctx, &ns), Vec::<String>::new());
    ctx.focus_visible = true;
    assert_eq!(select_in(&d, ":focus-visible", &ctx, &ns), ["i2"]);
}

#[test]
fn root_quirks_and_namespaces() {
    let d = doc("<!DOCTYPE html><html id=h><body id=b class=Foo><p id=p class=Bar></p>");
    let ctx = MatchContext::for_document(&d);
    let ns = NamespaceMap::default();
    assert_eq!(select_in(&d, ":root", &ctx, &ns), ["h"]);
    assert_eq!(select_in(&d, ":root > body", &ctx, &ns), ["b"]);
    assert_eq!(select_in(&d, ".bar", &ctx, &ns), Vec::<String>::new());

    // Sin DOCTYPE: quirks mode, clases e ids sin distinguir mayúsculas.
    let q = doc("<html id=h><body id=b class=Foo><p id=P class=Bar></p>");
    assert_eq!(q.mode, bf_dom::QuirksMode::Quirks);
    let qctx = MatchContext::for_document(&q);
    assert_eq!(select_in(&q, ".bar", &qctx, &ns), ["P"]);
    assert_eq!(select_in(&q, "#p", &qctx, &ns), ["P"]);

    // @namespace.
    let s = doc("<!DOCTYPE html><p id=p></p><svg><a id=svga></a></svg><a id=htmla></a>");
    let sctx = MatchContext::for_document(&s);
    let mut map = NamespaceMap::default();
    map.prefixes
        .insert("svg".into(), "http://www.w3.org/2000/svg".into());
    map.default = Some("http://www.w3.org/1999/xhtml".into());
    assert_eq!(select_in(&s, "svg|a", &sctx, &map), ["svga"]);
    assert_eq!(select_in(&s, "a", &sctx, &map), ["htmla"]);
    assert_eq!(select_in(&s, "*|a", &sctx, &map), ["svga", "htmla"]);
    assert!(parse_selector_list("xlink|a", &map).is_err());
}

#[test]
fn collects_declarations_through_layers_media_supports_and_nesting() {
    let d = doc(DOC);
    let ctx = MatchContext::for_document(&d);
    let css = r"
        @layer a, b;
        @layer b { #p1 { color: blue } }
        @layer a { #p1 { color: red } }
        p { margin: 0 }
        @media print { p { padding: 1px } }
        @media screen and (min-width: 100px) { p { border: 0 } }
        @supports (display: grid) { .x { display: grid } }
        @supports (display: nope) { .x { display: none } }
        .x { & + .x { gap: 1px } }
    ";
    let sheet = Stylesheet::parse(css, Origin::Author);
    let supports = |name: &str, value: &[ComponentValue]| {
        name == "display"
            && matches!(value, [v] if matches!(v.token(), Some(TokenKind::Ident(i)) if i == "grid"))
    };
    let media = MediaContext::default();
    let collect = |id: &str| {
        matching_declarations(
            &d,
            node_by_id(&d, id),
            None,
            &[&sheet],
            &media,
            &supports,
            &ctx,
        )
    };

    let p1 = collect("p1");
    let names: Vec<&str> = p1.iter().map(|m| m.declaration.name.as_str()).collect();
    assert_eq!(names, ["color", "color", "margin", "border", "display"]);
    let layer = |i: usize| p1[i].layer_index;
    // Orden de capas: a < b < sin capa.
    assert!(layer(1) < layer(0), "a antes que b");
    assert!(layer(0) < layer(2), "b antes que los estilos sin capa");
    assert_eq!(p1[0].specificity, Specificity(1, 0, 0));
    assert!(p1.windows(2).all(|w| w[0].source_order < w[1].source_order));

    let p2: Vec<String> = collect("p2")
        .iter()
        .map(|m| m.declaration.name.clone())
        .collect();
    assert_eq!(p2, ["margin", "border", "display", "gap"]);
}
