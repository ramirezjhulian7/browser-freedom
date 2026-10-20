//! Layout: modelo de cajas, flujo de bloques e inline, floats y posicionamiento (CSS 2.2,
//! CSS Display 3, CSS Box 3 y CSS Text 3 a nivel básico). Las fuentes se abstraen con
//! [`font::FontProvider`].

#![forbid(unsafe_code)]

mod block;
pub mod box_tree;
pub mod controls;
mod flex;
pub mod float;
pub mod font;
pub mod fragment;
mod inline;
mod positioned;

use bf_dom::Document;
use bf_style::StyleMap;

pub use box_tree::{ImageSizes, ImageState, LayoutReport};
pub use font::{
    FontKey, FontMetrics, FontProvider, FontRequest, Glyph, Outline, PathCommand, StyleMetrics,
    TestFont, font_request,
};
pub use fragment::{Fragment, FragmentKind, FragmentTree, MeterLevel, Rect, Size, Widget};

/// Layout del documento estilado en un viewport de `viewport` px.
pub fn layout_document(
    doc: &Document,
    styles: &StyleMap,
    viewport: Size,
    fonts: &dyn FontProvider,
) -> FragmentTree {
    layout_document_with_images(doc, styles, viewport, fonts, &ImageSizes::new())
}

/// Como [`layout_document`], con el estado de las imágenes del documento.
pub fn layout_document_with_images(
    doc: &Document,
    styles: &StyleMap,
    viewport: Size,
    fonts: &dyn FontProvider,
    images: &ImageSizes,
) -> FragmentTree {
    let (root, report) = box_tree::build_box_tree_report(doc, styles, images);
    let fragment = root.as_ref().map(|root| {
        let mut layout = block::Layout::new(fonts, viewport);
        block::layout_root(&mut layout, root)
    });
    FragmentTree {
        root: fragment,
        viewport,
        report,
    }
}

/// Volcado del árbol de fragmentos: una línea por fragmento, indentada dos espacios por nivel.
pub fn dump(doc: &Document, tree: &FragmentTree) -> String {
    fragment::dump(doc, tree)
}
