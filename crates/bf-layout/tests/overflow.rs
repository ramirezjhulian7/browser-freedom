//! E12: `text-overflow` (CSS Overflow 3 §5) y el borde de recorte con `overflow-clip-margin`
//! (CSS Overflow 3 §3.2) sobre el árbol de fragmentos (viewport 800×600, `TestFont`: cada glifo
//! avanza medio em, 8px con el tamaño de letra por omisión).

use bf_css::media::MediaContext;
use bf_css::selectors::MatchContext;
use bf_dom::{Document, NodeId};
use bf_html::{ParseOptions, parse_document};
use bf_layout::fragment::{Fragment, FragmentKind, Rect};
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

/// Los textos (con su x y su ancho) de las líneas de `#t`, en orden.
fn texts(html: &str) -> Vec<(String, f32, f32)> {
    fn walk(f: &Fragment, out: &mut Vec<(String, f32, f32)>) {
        for c in &f.children {
            if let (FragmentKind::Text, Some((t, _))) = (&c.kind, &c.text) {
                out.push((t.clone(), c.rect.x, c.rect.width));
            }
            walk(c, out);
        }
    }
    let (doc, tree) = layout(html);
    let mut out = Vec::new();
    walk(&frag(&tree, by_id(&doc, "t")), &mut out);
    out
}

fn page(style: &str, body: &str) -> String {
    format!(
        "<!doctype html><style>body{{margin:0}}\
         #t{{width:100px;white-space:nowrap;{style}}}</style><div id=t>{body}</div>"
    )
}

fn joined(t: &[(String, f32, f32)]) -> String {
    t.iter().map(|(s, _, _)| s.as_str()).collect()
}

#[test]
fn ellipsis_hides_clusters_until_it_fits() {
    // En 100px caben 12 glifos de 8px; con la elipsis (8px) quedan 11.
    let t = texts(&page(
        "overflow:hidden;text-overflow:ellipsis",
        "abcdefghijklmnop",
    ));
    assert_eq!(joined(&t), "abcdefghijk\u{2026}");
    let (last, x, w) = t.last().unwrap();
    assert_eq!((last.as_str(), *x, *w), ("\u{2026}", 88.0, 8.0));
}

#[test]
fn ellipsis_needs_inline_axis_clipping_and_overflow() {
    // `overflow: visible`: no aplica.
    let long = "abcdefghijklmnop";
    let t = texts(&page("text-overflow:ellipsis", long));
    assert_eq!(joined(&t), long);
    // `text-overflow: clip`: el contenido sigue entero (lo recorta la pintura).
    let t = texts(&page("overflow:hidden", long));
    assert_eq!(joined(&t), long);
    // Sin desborde no hay elipsis.
    let t = texts(&page("overflow:hidden;text-overflow:ellipsis", "abc"));
    assert_eq!(joined(&t), "abc");
    // Recortar solo en vertical no basta: el eje inline es el que cuenta.
    let t = texts(&page("overflow-y:clip;text-overflow:ellipsis", long));
    assert_eq!(joined(&t), long);
}

#[test]
fn ellipsis_string_and_first_unit_kept() {
    // Cadena propia (CSS Overflow 4 §5.1): "--" mide 16px, quedan 10 glifos.
    let t = texts(&page(
        "overflow:clip;text-overflow:\"--\"",
        "abcdefghijklmnop",
    ));
    assert_eq!(joined(&t), "abcdefghij--");
    // El primer carácter de la línea no se oculta aunque la elipsis no quepa.
    let t = texts(&page(
        "width:10px;overflow:hidden;text-overflow:ellipsis",
        "abc",
    ));
    assert_eq!(joined(&t), "a\u{2026}");
}

#[test]
fn ellipsis_shortens_cut_inline_boxes() {
    let html = "<!doctype html><style>body{margin:0}\
        #t{width:100px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}\
        #i{padding-right:20px}</style><div id=t>ab<span id=i>cdefghijklmn</span>op</div>";
    let (doc, tree) = layout(html);
    let i = frag(&tree, by_id(&doc, "i"));
    // Quedan "ab" + "cdefghijk" (88px); el `span` termina en su texto, sin su padding del
    // final.
    assert_eq!(i.rect.x, 16.0);
    assert_eq!(i.rect.right(), 88.0);
    assert_eq!(i.padding.right, 0.0);
}

fn clip_of(style: &str) -> Rect {
    let html = format!(
        "<!doctype html><style>body{{margin:0}}\
         #c{{width:100px;height:100px;padding:10px;border:5px solid;{style}}}</style>\
         <div id=c><div style='height:300px'></div></div>"
    );
    let (doc, tree) = layout(&html);
    frag(&tree, by_id(&doc, "c")).clip.expect("recorte")
}

#[test]
fn clip_edge_with_overflow_clip_margin() {
    // Padding box: (5,5) 120×120.
    assert_eq!(clip_of("overflow:clip"), Rect::new(5.0, 5.0, 120.0, 120.0));
    assert_eq!(
        clip_of("overflow:clip;overflow-clip-margin:10px"),
        Rect::new(-5.0, -5.0, 140.0, 140.0)
    );
    assert_eq!(
        clip_of("overflow:clip;overflow-clip-margin:content-box"),
        Rect::new(15.0, 15.0, 100.0, 100.0)
    );
    assert_eq!(
        clip_of("overflow:clip;overflow-clip-margin:border-box 5px"),
        Rect::new(-5.0, -5.0, 140.0, 140.0)
    );
    assert_eq!(
        clip_of("overflow:clip;overflow-clip-margin:padding-box -10px"),
        Rect::new(15.0, 15.0, 100.0, 100.0)
    );
    // `contain: paint` recorta como `overflow: clip` (CSS Containment 2 §3.4).
    assert_eq!(
        clip_of("contain:paint;overflow-clip-margin:10px"),
        Rect::new(-5.0, -5.0, 140.0, 140.0)
    );
}

#[test]
fn clip_edge_of_scroll_containers_stays_inside_padding_box() {
    let padding = Rect::new(5.0, 5.0, 120.0, 120.0);
    assert_eq!(
        clip_of("overflow:hidden;overflow-clip-margin:10px"),
        padding
    );
    assert_eq!(
        clip_of("overflow:hidden;overflow-clip-margin:border-box"),
        padding
    );
    assert_eq!(
        clip_of("overflow:auto;overflow-clip-margin:border-box -1000px"),
        padding
    );
    assert_eq!(
        clip_of("overflow:hidden;overflow-clip-margin:content-box"),
        Rect::new(15.0, 15.0, 100.0, 100.0)
    );
    assert_eq!(
        clip_of("overflow:hidden;overflow-clip-margin:-10px"),
        Rect::new(15.0, 15.0, 100.0, 100.0)
    );
}

#[test]
fn clip_only_in_the_clipped_axis() {
    let half = scroll::UNCLIPPED_HALF;
    let x = clip_of("overflow-x:clip;overflow-clip-margin:10px");
    assert_eq!((x.x, x.width), (-5.0, 140.0));
    assert_eq!((x.y, x.height), (-half, 2.0 * half));
    let y = clip_of("overflow-y:clip");
    assert_eq!((y.y, y.height), (5.0, 120.0));
    assert_eq!((y.x, y.width), (-half, 2.0 * half));
    // Con el otro eje `hidden`, `clip` se comporta como `hidden` y recortan los dos.
    assert_eq!(
        clip_of("overflow-x:clip;overflow-y:hidden"),
        Rect::new(5.0, 5.0, 120.0, 120.0)
    );
}
