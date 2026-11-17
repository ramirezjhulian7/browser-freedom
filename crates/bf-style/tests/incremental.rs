//! E2: la cascada incremental tras cambios de atributo
//! (`bf_style::restyle_after_attribute_changes`) da exactamente el mismo mapa que la cascada
//! completa del documento ya cambiado (también cuando desaparece el `::before` de un
//! descendiente), y se niega (`None`) cuando no puede asegurarlo.

use bf_css::media::MediaContext;
use bf_css::selectors::MatchContext;
use bf_css::stylesheet::Stylesheet;
use bf_dom::{Document, DomString, NodeId};
use bf_html::{ParseOptions, parse_document};
use bf_style::{
    StyleMap, document_stylesheets, restyle_after_attribute_changes, style_document_with_sheets,
};

/// Combinadores de descendiente, hijo y hermanos, selectores de atributo, `:not()`, `:is()`,
/// `:nth-child()` sin `of`, herencia (`em`, `color`, variables), `::before` con `attr()`,
/// `::marker`, `:dir()`, `:lang()` y atributos con presentational hints.
const HTML: &str = "<!doctype html><html lang=es><style>
  body { font-size: 16px }
  .big { font-size: 2em } .big span { color: teal }
  #menu > li { margin: 2px } #menu > li.on { color: red; --c: 5px }
  li { padding: var(--c, 1px) }
  .on + li { text-decoration: underline } .on ~ li em { font-style: normal }
  [data-k='1'] { order: 3 } [data-k='2'] .x { order: 4 }
  p:not(.q) { margin: 7px } :is(.a, .b) > i { color: green }
  li:nth-child(2n) { background-color: silver }
  q[data-t]::before { content: attr(data-t) } ol li::marker { color: gray }
  :dir(rtl) { text-indent: 3px } :lang(en) { quotes: '<' '>' }
  [hidden] { display: none } .hide > * { visibility: hidden }
  .noq q::before { content: none }
</style>
<body>
<div id=a1><span id=s1>uno</span><p id=p1>dos <span id=s2>tres</span></p></div>
<ul id=menu><li id=l1>a</li><li id=l2>b <em>e</em></li><li id=l3>c <em>f</em></li><li id=l4>d</li></ul>
<ol><li id=o1>x</li><li id=o2>y</li></ol>
<div id=d2 data-k=0><b class=x id=x1>x</b><i id=i1>i</i><q id=q1>q</q></div>
<table id=t1 border=0><tr><td id=c1>1</td><td id=c2 bgcolor=red>2</td></tr></table>
<font id=f1 color=blue>f <span id=s3>g</span></font>
<div id=w><section id=sec1><p id=p2>p</p></section></div>
</body></html>";

fn id(doc: &Document, s: &str) -> NodeId {
    doc.get_element_by_id(&DomString::from(s))
        .unwrap_or_else(|| panic!("#{s}"))
}

fn set(doc: &mut Document, el: &str, name: &str, value: &str) {
    let el = id(doc, el);
    doc.set_attribute(el, &DomString::from(name), DomString::from(value))
        .unwrap();
}

fn unset(doc: &mut Document, el: &str, name: &str) {
    let el = id(doc, el);
    doc.remove_attribute(el, &DomString::from(name));
}

struct Env {
    media: MediaContext,
}

impl Env {
    fn sheets(&self, doc: &Document) -> Vec<Stylesheet> {
        document_stylesheets(doc, &self.media, None)
    }

    fn full(&self, doc: &Document) -> StyleMap {
        let ctx = MatchContext::for_document(doc);
        style_document_with_sheets(doc, &self.sheets(doc), &[], &self.media, &ctx, None)
    }

    fn incremental(&self, doc: &Document, previous: &StyleMap, since: u64) -> Option<StyleMap> {
        let changed = doc.attribute_changes_since(since)?;
        let ctx = MatchContext::for_document(doc);
        restyle_after_attribute_changes(
            doc,
            &self.sheets(doc),
            &[],
            &self.media,
            &ctx,
            None,
            previous,
            &changed,
        )
    }
}

fn assert_same(full: &StyleMap, inc: &StyleMap, step: &str) {
    for (key, style) in full {
        assert_eq!(Some(style), inc.get(key), "{step}: {key:?}");
    }
    assert_eq!(full.len(), inc.len(), "{step}: claves de más");
}

type Step = (&'static str, fn(&mut Document));

#[test]
fn incremental_matches_full_cascade() {
    let env = Env {
        media: MediaContext::default(),
    };
    let (mut doc, _) = parse_document(HTML, &ParseOptions::default());
    let steps: [Step; 15] = [
        ("clase heredada", |d| set(d, "a1", "class", "big")),
        ("clase con hermanos", |d| set(d, "l1", "class", "on")),
        ("clase en otro hermano", |d| {
            unset(d, "l1", "class");
            set(d, "l3", "class", "on");
        }),
        ("atributo con descendientes", |d| {
            set(d, "d2", "data-k", "2")
        }),
        ("style", |d| {
            set(d, "s1", "style", "color: rgb(1, 2, 3); margin: 1em")
        }),
        ("not", |d| set(d, "p1", "class", "q")),
        ("is", |d| set(d, "d2", "class", "b")),
        ("attr() en ::before", |d| set(d, "q1", "data-t", "«")),
        ("dir y lang", |d| {
            set(d, "w", "dir", "rtl");
            set(d, "sec1", "lang", "en");
        }),
        ("hidden", |d| set(d, "l2", "hidden", "")),
        ("hints", |d| {
            set(d, "c2", "bgcolor", "blue");
            set(d, "f1", "color", "green");
            set(d, "t1", "border", "3");
        }),
        ("varios en un lote", |d| {
            set(d, "a1", "class", "hide");
            set(d, "s2", "style", "font-size: 20px");
            set(d, "o1", "style", "color: red");
            unset(d, "l2", "hidden");
        }),
        ("y vuelta atrás", |d| {
            unset(d, "a1", "class");
            unset(d, "s1", "style");
            set(d, "d2", "data-k", "1");
        }),
        ("sin cambios de estilo", |d| set(d, "p2", "title", "nada")),
        ("desaparece un ::before de un descendiente", |d| {
            set(d, "d2", "class", "noq")
        }),
    ];
    // Cada paso parte del resultado incremental del anterior: los errores no se esconden.
    let mut previous = env.full(&doc);
    for (name, step) in steps {
        let since = doc.version();
        step(&mut doc);
        let inc = env
            .incremental(&doc, &previous, since)
            .unwrap_or_else(|| panic!("{name}: debería poder ser incremental"));
        let full = env.full(&doc);
        assert_same(&full, &inc, name);
        previous = inc;
    }
}

#[test]
fn incremental_refuses_when_it_cannot_be_sure() {
    let env = Env {
        media: MediaContext::default(),
    };
    let (mut doc, _) = parse_document(HTML, &ParseOptions::default());
    let previous = env.full(&doc);

    // Un cambio del árbol entre medias: `attribute_changes_since` no lo cubre.
    let since = doc.version();
    set(&mut doc, "s1", "class", "x");
    let p2 = id(&doc, "p2");
    doc.remove(p2);
    assert!(doc.attribute_changes_since(since).is_none());

    // La raíz cambia (`rem`, y todo cuelga de ella).
    let (mut doc, _) = parse_document(HTML, &ParseOptions::default());
    let since = doc.version();
    let root = doc.document_element().unwrap();
    doc.set_attribute(root, &DomString::from("class"), DomString::from("t"))
        .unwrap();
    assert!(env.incremental(&doc, &previous, since).is_none());

    // `<style>` decide qué hojas aplican; un control de formulario, estados de otros.
    for html in [
        "<!doctype html><style id=st>p { color: red }</style><p>x</p>",
        "<!doctype html><form><input id=st type=radio name=g><input type=radio name=g></form>",
    ] {
        let (mut doc, _) = parse_document(html, &ParseOptions::default());
        let previous = env.full(&doc);
        let since = doc.version();
        set(&mut doc, "st", "media", "print");
        assert!(env.incremental(&doc, &previous, since).is_none(), "{html}");
    }

    // Hojas con `:has()` o `:nth-child(… of S)`: un atributo cambia a antepasados o hermanos.
    for sheet in [
        "div:has(.on) { color: red }",
        "li:nth-child(2 of .on) { color: red }",
    ] {
        let html = format!(
            "<!doctype html><style>{sheet}</style><div><ul><li id=a>a</li><li>b</li></ul></div>"
        );
        let (mut doc, _) = parse_document(&html, &ParseOptions::default());
        let previous = env.full(&doc);
        let since = doc.version();
        set(&mut doc, "a", "class", "on");
        assert!(env.incremental(&doc, &previous, since).is_none(), "{sheet}");
    }

    // Una versión de la que ya no queda registro (más cambios que los que se guardan).
    let (mut doc, _) = parse_document(HTML, &ParseOptions::default());
    let since = doc.version();
    for i in 0..10_000 {
        set(&mut doc, "s1", "data-n", &i.to_string());
    }
    assert!(doc.attribute_changes_since(since).is_none());
    let recent = doc.version() - 10;
    assert_eq!(
        doc.attribute_changes_since(recent),
        Some(vec![id(&doc, "s1")])
    );
}
