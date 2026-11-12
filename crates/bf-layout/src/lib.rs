//! Layout: modelo de cajas, flujo de bloques e inline, floats y posicionamiento (CSS 2.2,
//! CSS Display 3, CSS Box 3 y CSS Text 3 a nivel básico). Las fuentes se abstraen con
//! [`font::FontProvider`].

#![forbid(unsafe_code)]

pub mod bidi; // E9: UAX #9
mod block;
pub mod box_tree;
mod clamp; // E12
pub mod controls;
mod flex;
pub mod float;
pub mod font;
pub mod fragment;
mod grid; // E4
mod inline;
pub mod linebreak; // E9: UAX #14
mod positioned;
pub mod scroll; // E12
mod table; // E4
mod text_overflow; // E12
pub mod unicode; // E9: Script y Vertical_Orientation
mod unicode_tables; // E9: generado por tools/gen_unicode_text.py
pub mod writing_mode; // E9: CSS Writing Modes 4 (flujos verticales)

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
        // E9: con cajas verticales, cada caja se maqueta en el marco de su flujo; una raíz
        // vertical, con el viewport traspuesto, y el resultado vuelve a coordenadas físicas.
        if writing_mode::has_vertical(root) {
            let frame = writing_mode::Frame::of(&root.style);
            let tree = writing_mode::logical_tree(root);
            let logical_viewport = if frame.is_vertical() {
                Size {
                    width: viewport.height,
                    height: viewport.width,
                }
            } else {
                viewport
            };
            let mut layout = block::Layout::new(fonts, logical_viewport);
            layout.frame = frame;
            let mut f = block::layout_root(&mut layout, &tree);
            writing_mode::fragment_to_physical(&mut f, frame, (0.0, 0.0), viewport.width);
            return f;
        }
        let mut layout = block::Layout::new(fonts, viewport);
        block::layout_root(&mut layout, root)
    });
    let mut tree = FragmentTree {
        root: fragment,
        viewport,
        report,
        scroll: scroll::ScrollState::default(),
    };
    scroll::init(&mut tree, doc); // E12
    tree
}

/// Volcado del árbol de fragmentos: una línea por fragmento, indentada dos espacios por nivel.
pub fn dump(doc: &Document, tree: &FragmentTree) -> String {
    fragment::dump(doc, tree)
}
