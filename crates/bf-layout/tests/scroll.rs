//! Scroll containers (CSS Overflow 3 §3) y `position: sticky` (CSS Position 3 §3.4) sobre el
//! árbol de fragmentos (viewport 800×600, `TestFont`: cada glifo mide un em).

use bf_css::media::MediaContext;
use bf_css::selectors::MatchContext;
use bf_dom::{Document, NodeId};
use bf_html::{ParseOptions, parse_document};
use bf_layout::fragment::Fragment;
use bf_layout::{FragmentTree, Size, TestFont, layout_document, scroll};
use bf_style::style_document;

fn layout(html: &str) -> (Document, FragmentTree) {
    let (doc, _) = parse_document(html, &ParseOptions::default());
    let ctx = MatchContext::for_document(&doc);
    let media = MediaContext {
        width: 800.0,
        height: 600.0,
        ..MediaContext::default()
    };
    let styles = style_document(&doc, &[], &media, &ctx);
    let tree = layout_document(
        &doc,
        &styles,
        Size {
            width: 800.0,
            height: 600.0,
        },
        &TestFont,
    );
    (doc, tree)
}

fn by_id(doc: &Document, id: &str) -> NodeId {
    doc.inclusive_descendants(doc.root())
        .into_iter()
        .find(|&n| doc.element(n).is_some_and(|e| e.attr("id") == Some(id)))
        .unwrap_or_else(|| panic!("#{id}"))
}

fn frag(tree: &FragmentTree, node: NodeId) -> Fragment {
    fn walk(f: &Fragment, node: NodeId) -> Option<Fragment> {
        if f.node == Some(node) && !f.anonymous {
            return Some(f.clone());
        }
        f.children.iter().find_map(|c| walk(c, node))
    }
    walk(tree.root.as_ref().unwrap(), node).expect("fragmento")
}

const SCROLLER: &str = "<!doctype html><style>body{margin:0}\
    #s{overflow:auto;width:100px;height:100px;padding:10px;border:5px solid}\
    #c{height:300px;width:50px;margin-bottom:7px}</style>\
    <div id=s><div id=c></div></div>";

#[test]
fn scrollable_overflow_includes_end_padding_and_margin() {
    let (doc, tree) = layout(SCROLLER);
    let s = by_id(&doc, "s");
    let sb = tree.scroll.boxes.get(&s).expect("scroll container");
    assert_eq!((sb.client.width, sb.client.height), (120.0, 120.0));
    // 10 de padding + 300 de contenido + 7 de margen + 10 de padding del final.
    assert_eq!(sb.scroll_size.height, 327.0);
    assert_eq!(sb.scroll_size.width, 120.0);
    assert_eq!(sb.max_offset(), (0.0, 207.0));
    assert_eq!(sb.user, (true, true));
}

#[test]
fn set_offset_translates_content_and_clamps() {
    let (doc, mut tree) = layout(SCROLLER);
    let (s, c) = (by_id(&doc, "s"), by_id(&doc, "c"));
    let y0 = frag(&tree, c).rect.y;
    assert_eq!(
        scroll::set_scroll_offset(&mut tree, s, (0.0, 50.0)),
        Some((0.0, 50.0))
    );
    assert_eq!(frag(&tree, c).rect.y, y0 - 50.0);
    // El contenedor no se mueve.
    assert_eq!(frag(&tree, s).rect.y, 0.0);
    assert_eq!(
        scroll::set_scroll_offset(&mut tree, s, (30.0, 1e6)),
        Some((0.0, 207.0))
    );
    assert_eq!(frag(&tree, c).rect.y, y0 - 207.0);
    assert_eq!(
        scroll::set_scroll_offset(&mut tree, s, (0.0, -5.0)),
        Some((0.0, 0.0))
    );
    assert_eq!(frag(&tree, c).rect.y, y0);
    // Lo que no es scroll container no se desplaza.
    assert_eq!(scroll::set_scroll_offset(&mut tree, c, (0.0, 5.0)), None);
}

#[test]
fn wheel_scrolls_innermost_container_that_can_move() {
    let html = "<!doctype html><style>body{margin:0}div{overflow:auto}\
        #outer{height:100px}#inner{height:50px}#tall{height:80px}</style>\
        <div id=outer><div id=inner><div id=tall></div></div><div style='height:300px'></div></div>";
    let (doc, mut tree) = layout(html);
    let (outer, inner, tall) = (
        by_id(&doc, "outer"),
        by_id(&doc, "inner"),
        by_id(&doc, "tall"),
    );
    assert_eq!(
        scroll::scroll_at(&mut tree, tall, (0.0, 20.0)),
        Some((inner, (0.0, 20.0)))
    );
    // `inner` llega a su máximo (30): la rueda pasa a `outer`.
    assert_eq!(
        scroll::scroll_at(&mut tree, tall, (0.0, 20.0)),
        Some((inner, (0.0, 30.0)))
    );
    assert_eq!(
        scroll::scroll_at(&mut tree, tall, (0.0, 20.0)),
        Some((outer, (0.0, 20.0)))
    );
    // Hacia arriba vuelve a moverse el interno.
    assert_eq!(
        scroll::scroll_at(&mut tree, tall, (0.0, -10.0)),
        Some((inner, (0.0, 20.0)))
    );
    assert_eq!(scroll::scroll_parent(&tree, tall), Some(inner));
}

#[test]
fn overflow_hidden_is_not_user_scrollable() {
    let html = "<!doctype html><style>body{margin:0}#h{overflow:hidden;height:50px}</style>\
        <div id=h><div id=t style='height:200px'></div></div>";
    let (doc, mut tree) = layout(html);
    let (h, t) = (by_id(&doc, "h"), by_id(&doc, "t"));
    assert_eq!(scroll::scroll_at(&mut tree, t, (0.0, 20.0)), None);
    // Por programa, sí (CSS Overflow 3 §3.1).
    assert_eq!(
        scroll::set_scroll_offset(&mut tree, h, (0.0, 20.0)),
        Some((0.0, 20.0))
    );
}

#[test]
fn root_and_body_propagate_overflow_to_viewport() {
    let html = "<!doctype html><style>body{overflow:hidden;height:10px;margin:0}</style>\
        <body><div style='height:300px'></div>";
    let (doc, tree) = layout(html);
    let body = doc.body().unwrap();
    assert!(!tree.scroll.boxes.contains_key(&body));
    assert!(frag(&tree, body).clip.is_none());
}

#[test]
fn textarea_is_a_scroll_container() {
    let text = "a\n".repeat(30);
    let html = format!("<!doctype html><textarea id=t rows=2>{text}</textarea>");
    let (doc, mut tree) = layout(&html);
    let t = by_id(&doc, "t");
    let sb = *tree.scroll.boxes.get(&t).expect("textarea");
    assert!(sb.max_offset().1 > 0.0);
    assert_eq!(
        scroll::scroll_at(&mut tree, t, (0.0, 16.0)),
        Some((t, (0.0, 16.0)))
    );
}

#[test]
fn sticky_top_in_viewport_and_container_limits() {
    let html = "<!doctype html><style>body{margin:0}\
        #cb{height:400px;margin-top:100px}\
        #st{position:sticky;top:10px;height:50px}</style>\
        <div id=cb><div id=st></div></div><div style='height:2000px'></div>";
    let (doc, mut tree) = layout(html);
    let st = by_id(&doc, "st");
    assert_eq!(frag(&tree, st).rect.y, 100.0);
    assert!(tree.scroll.viewport_sticky);
    // Con la página desplazada 200 px la caja queda a 10 px del borde del viewport.
    assert!(scroll::page_scrolled(&mut tree, (0.0, 200.0)));
    assert_eq!(frag(&tree, st).rect.y, 210.0);
    // No sale de su containing block (100 + 400 - 50 = 450).
    scroll::page_scrolled(&mut tree, (0.0, 1000.0));
    assert_eq!(frag(&tree, st).rect.y, 450.0);
    scroll::page_scrolled(&mut tree, (0.0, 0.0));
    assert_eq!(frag(&tree, st).rect.y, 100.0);
}

#[test]
fn sticky_inside_scroll_container() {
    let html = "<!doctype html><style>body{margin:0}\
        #s{overflow:auto;height:100px}\
        #st{position:sticky;top:0;height:20px}</style>\
        <div id=s><div style='height:30px'></div><div id=st></div>\
        <div style='height:500px'></div></div>";
    let (doc, mut tree) = layout(html);
    let (s, st) = (by_id(&doc, "s"), by_id(&doc, "st"));
    assert_eq!(frag(&tree, st).rect.y, 30.0);
    scroll::set_scroll_offset(&mut tree, s, (0.0, 100.0));
    assert_eq!(frag(&tree, st).rect.y, 0.0);
    // El scroll de la página no afecta a una caja pegada a otro contenedor.
    assert!(!tree.scroll.viewport_sticky);
    scroll::set_scroll_offset(&mut tree, s, (0.0, 10.0));
    assert_eq!(frag(&tree, st).rect.y, 20.0);
}

/// CSS Transforms 1 §3: un elemento transformado es el containing block (su padding box) de
/// sus descendientes absolutos y fijos, aunque sea `static`.
#[test]
fn transform_contains_absolute_and_fixed_descendants() {
    let (doc, tree) = layout(
        "<!doctype html><style>body{margin:0}\
         #t{transform:translateX(0);margin:40px 0 0 30px;width:200px;height:100px;\
         padding:5px;border:3px solid}\
         #a{position:absolute;left:10px;top:20px;width:10px;height:10px}\
         #f{position:fixed;right:0;bottom:0;width:10px;height:10px}\
         #n{position:fixed;left:0;top:0;width:10px;height:10px}</style>\
         <div id=t><div id=a></div><div><div id=f></div></div></div><div id=n></div>",
    );
    // Padding box de #t: (33, 43) con 210×110.
    let a = frag(&tree, by_id(&doc, "a"));
    assert_eq!((a.rect.x, a.rect.y), (43.0, 63.0));
    let f = frag(&tree, by_id(&doc, "f"));
    assert_eq!((f.rect.x, f.rect.y), (233.0, 143.0));
    // Ya no está fijado al viewport.
    assert_eq!(
        f.style.position,
        bf_style::properties::types::Position::Absolute
    );
    // Un fijo fuera del elemento transformado sigue contra el viewport.
    let n = frag(&tree, by_id(&doc, "n"));
    assert_eq!((n.rect.x, n.rect.y), (0.0, 0.0));
    assert_eq!(
        n.style.position,
        bf_style::properties::types::Position::Fixed
    );
}

fn texts(f: &Fragment, out: &mut Vec<String>) {
    if let Some((t, _)) = &f.text {
        out.push(t.clone());
    }
    for c in &f.children {
        texts(c, out);
    }
}

/// CSS Overflow 4 §5: `line-clamp: 2` corta tras la segunda línea, la caja mide dos líneas y
/// la última termina en "…".
#[test]
fn line_clamp_counts_lines_and_inserts_ellipsis() {
    let (doc, tree) = layout(
        "<!doctype html><style>body{margin:0}\
         #c{line-clamp:2;font:10px/20px x;width:300px}</style>\
         <div id=c>aaa<br>bbb<br>ccc</div><div id=n>x</div>",
    );
    let c = frag(&tree, by_id(&doc, "c"));
    assert_eq!(c.rect.height, 40.0);
    let mut t = Vec::new();
    texts(&c, &mut t);
    assert_eq!(t.join("|"), "aaa|bbb|\u{2026}");
    // El siguiente bloque sube.
    assert_eq!(frag(&tree, by_id(&doc, "n")).rect.y, 40.0);
}

/// `line-clamp: auto` corta en la última línea que cabe en `max-height`; la heredada de WebKit
/// solo actúa con `display: -webkit-box` y `-webkit-box-orient: vertical`.
#[test]
fn line_clamp_auto_and_webkit_legacy() {
    let (doc, tree) = layout(
        "<!doctype html><style>body{margin:0}\
         div{font:10px/20px x;width:60px}\
         #a{line-clamp:auto;max-height:50px}\
         #w{display:-webkit-box;-webkit-box-orient:vertical;-webkit-line-clamp:1}\
         #h{display:-webkit-box;-webkit-line-clamp:1}</style>\
         <div id=a>a<br>b<br>c<br>d</div><div id=w>a<br>b</div><div id=h>a<br>b</div>",
    );
    assert_eq!(frag(&tree, by_id(&doc, "a")).rect.height, 40.0);
    assert_eq!(frag(&tree, by_id(&doc, "w")).rect.height, 20.0);
    assert_eq!(frag(&tree, by_id(&doc, "h")).rect.height, 40.0);
}
