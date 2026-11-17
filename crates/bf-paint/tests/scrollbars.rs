//! E2: las barras de desplazamiento de los scroll containers (`bf_paint::scrollbars`). Una caja
//! que no se ve (`visibility: hidden`, CSS 2 §11.2, o dentro de un `opacity: 0`) no pinta sus
//! barras: el caso de los menús desplegables cerrados de Wikipedia.

use bf_css::media::MediaContext;
use bf_css::selectors::MatchContext;
use bf_html::{ParseOptions, parse_document};
use bf_layout::{Size, TestFont, layout_document};
use bf_paint::display_list::DisplayItem;
use bf_paint::scrollbars::scrollbars;
use bf_style::values::color::Rgba;

/// Cuántas barras (rectángulos) pinta la página.
fn bars(html: &str) -> usize {
    let (doc, _) = parse_document(html, &ParseOptions::default());
    let ctx = MatchContext::for_document(&doc);
    let media = MediaContext {
        width: 800.0,
        height: 600.0,
        ..MediaContext::default()
    };
    let styles = bf_style::style_document(&doc, &[], &media, &ctx);
    let tree = layout_document(
        &doc,
        &styles,
        Size {
            width: 800.0,
            height: 600.0,
        },
        &TestFont,
    );
    scrollbars(&tree, Rgba::BLACK)
        .items
        .iter()
        .filter(|i| matches!(i, DisplayItem::Rect { .. }))
        .count()
}

#[test]
fn hidden_scroll_containers_paint_no_bars() {
    let menu = |extra: &str| {
        format!(
            "<!doctype html><div style='overflow: hidden auto; height: 0; padding: 16px; \
             width: 200px; {extra}'><div style='height: 300px'></div></div>"
        )
    };
    // Visible y desbordando: una barra vertical.
    assert_eq!(bars(&menu("")), 1);
    // Cerrado como en Vector: ninguna.
    assert_eq!(bars(&menu("visibility: hidden")), 0);
    assert_eq!(bars(&menu("opacity: 0")), 0);
    let html = "<!doctype html><div style='opacity: 0'><div style='overflow: auto; height: 20px; \
                width: 200px'><div style='height: 300px'></div></div></div>";
    assert_eq!(bars(html), 0);
}
