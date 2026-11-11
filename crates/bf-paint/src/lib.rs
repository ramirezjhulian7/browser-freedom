//! Pintura: lista de visualización (CSS 2.2 Apéndice E) y rasterizado con `tiny-skia`.

#![forbid(unsafe_code)]

pub mod builder;
pub mod display_list;
mod effects; // E4
pub mod images;
pub mod raster;
pub mod reftest;
pub mod scrollbars; // E12

use std::sync::Arc;
use std::time::{Duration, Instant};

use bf_css::media::MediaContext;
use bf_css::selectors::MatchContext;
use bf_css::stylesheet::Stylesheet;
use bf_dom::Document;
use bf_font::{FontDb, Fonts};
use bf_infra::encoding::Encoding;
use bf_infra::{ResourceLoader, Url};
use bf_layout::{FragmentTree, Size};
use bf_style::values::color::Rgba;
use bf_style::{Resources, document_stylesheets, font_face_rules, style_document_with_sheets};

pub use builder::{
    build_display_list, build_display_list_with_frames, build_display_list_with_images,
    document_extent,
};
pub use display_list::{DisplayItem, DisplayList, GlyphInstance, PathOp, Radii};
pub use images::{PageImages, load_images};
pub use raster::{raster_color, render, render_scrolled};
pub use tiny_skia::{Pixmap, PixmapPaint, Transform};

/// Un documento estilado, maquetado y listo para pintar.
pub struct Page {
    pub fonts: Fonts,
    /// Los estilos computados con que se maquetó (el CSSOM de la página los reutiliza).
    pub styles: std::sync::Arc<bf_style::StyleMap>,
    pub tree: FragmentTree,
    pub display_list: DisplayList,
    pub images: PageImages,
    pub timings: PageTimings,
    pub report: EngineReport,
    /// Los documentos de los `<iframe>` maquetados con el tamaño de su caja (E4).
    pub frames: Vec<FramePage>,
}

/// El documento de un `<iframe>` (HTML §4.8.5) que se pinta dentro de su caja (E4).
#[derive(Clone)]
pub struct FrameDoc {
    /// El `<iframe>` en el documento que lo contiene.
    pub node: bf_dom::NodeId,
    pub doc: Document,
    /// URL base del documento del marco.
    pub base: Url,
    pub encoding: Encoding,
    /// Desplazamiento del marco (px CSS).
    pub scroll: (f32, f32),
    /// Sus propios marcos.
    pub frames: Vec<FrameDoc>,
}

/// Un marco ya maquetado (E4).
pub struct FramePage {
    /// El `<iframe>`.
    pub node: bf_dom::NodeId,
    /// Su content box en coordenadas de la página que lo contiene: el viewport del marco.
    pub rect: bf_layout::fragment::Rect,
    pub scroll: (f32, f32),
    pub page: Box<Page>,
}

impl FramePage {
    /// El marco (y su página) bajo `point` (coordenadas de la página que lo contiene), con
    /// el punto en coordenadas del viewport del marco.
    pub fn locate(frames: &[FramePage], point: (f32, f32)) -> Option<(&FramePage, (f32, f32))> {
        frames.iter().rev().find_map(|f| {
            let (x, y) = (point.0 - f.rect.x, point.1 - f.rect.y);
            (x >= 0.0 && y >= 0.0 && x < f.rect.width && y < f.rect.height).then_some((f, (x, y)))
        })
    }
}

impl Page {
    /// Rehace la lista de visualización después de cambiar el árbol sin maquetar de nuevo
    /// (el scroll de un elemento o las cajas `sticky`, E12).
    pub fn repaint(&mut self, doc: &Document) {
        let start = Instant::now();
        self.display_list =
            build_display_list_with_images(doc, &self.tree, &self.fonts, &self.images);
        self.timings.paint = start.elapsed();
    }

    /// Aplica las posiciones de desplazamiento de los elementos y el scroll de la página (para
    /// las cajas `sticky` del viewport) y, si cambió el árbol, rehace la lista. Devuelve si
    /// cambió.
    pub fn apply_scroll(
        &mut self,
        doc: &Document,
        offsets: &[(bf_dom::NodeId, (f32, f32))],
        page_scroll: (f32, f32),
    ) -> bool {
        let boxes = &self.tree.scroll.boxes;
        let moved = offsets
            .iter()
            .any(|(n, o)| boxes.get(n).is_some_and(|b| b.clamp(*o) != b.offset));
        bf_layout::scroll::apply_offsets(&mut self.tree, offsets.iter().copied());
        let sticky = bf_layout::scroll::page_scrolled(&mut self.tree, page_scroll);
        if moved || sticky {
            self.repaint(doc);
        }
        moved || sticky
    }
}

/// Qué hizo el motor con un documento y qué ignoró. Cada contador sale del subsistema que lo
/// decide: `bf-style` (CSS), `bf-layout` (`display` degradados), `bf-image` (formatos) y
/// `bf-font` (fuentes resueltas).
#[derive(Debug, Clone, Default)]
pub struct EngineReport {
    pub style: bf_style::StyleReport,
    pub layout: bf_layout::LayoutReport,
    pub images: bf_image::DecodeStats,
    /// Imágenes que no se pudieron descargar.
    pub images_unreachable: usize,
    /// Elementos `<script>` (no se ejecutan: el motor JavaScript es la fase 2).
    pub scripts: usize,
    pub dom_nodes: usize,
    pub elements: usize,
    pub quirks: bf_dom::QuirksMode,
    /// `font-family` pedido → cara resuelta.
    pub fonts: Vec<(String, String)>,
}

impl EngineReport {
    /// ¿La página usó algo que el motor todavía no soporta?
    pub fn has_unsupported(&self) -> bool {
        self.scripts > 0
            || self.style.ignored() > 0
            || !self.layout.is_empty()
            || !self.images.unsupported.is_empty()
    }
}

/// Duración de cada fase de [`prepare_page`].
#[derive(Debug, Clone, Copy, Default)]
pub struct PageTimings {
    /// Hojas externas, `@font-face` y cascada.
    pub style: Duration,
    /// Carga y decodificación de imágenes.
    pub images: Duration,
    pub layout: Duration,
    /// Construcción de la lista de visualización.
    pub paint: Duration,
}

/// Lo que un documento necesita además de su árbol para pintarse.
#[derive(Clone)]
pub struct PageContext<'a> {
    /// URL base del documento, contra la que se resuelven `<link>`, `@import`, `url()`,
    /// `@font-face` e imágenes.
    pub base: &'a Url,
    /// Encoding del documento (el de las hojas sin BOM ni `@charset`).
    pub encoding: Encoding,
    pub loader: Arc<dyn ResourceLoader>,
    pub fonts: Arc<FontDb>,
    /// Hojas adicionales del llamador (user o UA).
    pub extra: &'a [&'a Stylesheet],
    /// Zoom de texto del navegador (1 = sin zoom; ver `MediaContext::text_zoom`). Los marcos
    /// anidados heredan el del documento.
    pub text_zoom: f32,
}

/// Estilo → layout → lista de visualización.
pub fn prepare_page(doc: &Document, ctx: &PageContext<'_>, viewport: Size) -> Page {
    prepare_page_with_canvases(doc, ctx, viewport, &[])
}

/// [`prepare_page`] con los bitmaps de los `<canvas>` del documento (E1): cada uno se
/// muestra como la imagen `canvas:<id>` del elemento, con su tamaño natural.
pub fn prepare_page_with_canvases(
    doc: &Document,
    ctx: &PageContext<'_>,
    viewport: Size,
    canvases: &[(bf_dom::NodeId, std::sync::Arc<tiny_skia::Pixmap>)],
) -> Page {
    prepare_page_with_frames(doc, ctx, viewport, canvases, &[])
}

/// [`prepare_page_with_canvases`] con los documentos de los `<iframe>` (E4): cada uno se
/// maqueta con el tamaño del content box de su `<iframe>` (su viewport, HTML §4.8.5) y su lista
/// de visualización se pinta dentro, recortada y desplazada.
pub fn prepare_page_with_frames(
    doc: &Document,
    ctx: &PageContext<'_>,
    viewport: Size,
    canvases: &[(bf_dom::NodeId, std::sync::Arc<tiny_skia::Pixmap>)],
    frames: &[FrameDoc],
) -> Page {
    let (base, extra, loader, db) = (ctx.base, ctx.extra, ctx.loader.clone(), ctx.fonts.clone());
    let start = Instant::now();
    let media = MediaContext {
        width: f64::from(viewport.width),
        height: f64::from(viewport.height),
        text_zoom: f64::from(ctx.text_zoom),
        ..MediaContext::default()
    };
    let sheets = document_stylesheets(
        doc,
        &media,
        Some(Resources {
            loader: &*loader,
            base,
            encoding: ctx.encoding,
        }),
    );
    let mut all: Vec<&Stylesheet> = sheets.iter().collect();
    all.extend(extra.iter().copied());
    let mut fonts = Fonts::new(db);
    fonts.add_font_faces(&font_face_rules(&all, &media), loader.clone());
    let match_ctx = MatchContext::for_document(doc);
    let metrics = bf_layout::StyleMetrics(&fonts);
    let styles =
        style_document_with_sheets(doc, &sheets, extra, &media, &match_ctx, Some(&metrics));
    let style = start.elapsed();
    let start = Instant::now();
    let mut images = load_images(doc, &styles, base, &*loader);
    for (node, pixmap) in canvases {
        images.insert_canvas(*node, pixmap.clone());
    }
    let images_time = start.elapsed();
    let start = Instant::now();
    let tree =
        bf_layout::layout_document_with_images(doc, &styles, viewport, &fonts, &images.sizes);
    let layout = start.elapsed();
    let frames = prepare_frames(&tree, ctx, frames);
    let start = Instant::now();
    let lists: std::collections::HashMap<_, _> = frames
        .iter()
        .map(|f| (f.node, (&f.page.display_list, f.scroll)))
        .collect();
    let display_list = build_display_list_with_frames(doc, &tree, &fonts, &images, &lists);
    let nodes = doc.inclusive_descendants(doc.root());
    let report = EngineReport {
        style: bf_style::style_report(doc, &sheets),
        layout: tree.report.clone(),
        images: images.stats.clone(),
        images_unreachable: images.fetch_failed,
        scripts: nodes
            .iter()
            .filter(|&&n| doc.element(n).is_some_and(|e| e.is_html("script")))
            .count(),
        dom_nodes: nodes.len(),
        elements: nodes.iter().filter(|&&n| doc.element(n).is_some()).count(),
        quirks: doc.mode,
        fonts: fonts.used_fonts(),
    };
    Page {
        fonts,
        styles: std::sync::Arc::new(styles),
        tree,
        display_list,
        images,
        report,
        frames,
        timings: PageTimings {
            style,
            images: images_time,
            layout,
            paint: start.elapsed(),
        },
    }
}

/// Maqueta los marcos de `frames` que tienen caja en `tree`.
fn prepare_frames(
    tree: &FragmentTree,
    ctx: &PageContext<'_>,
    frames: &[FrameDoc],
) -> Vec<FramePage> {
    if frames.is_empty() {
        return Vec::new();
    }
    let mut boxes = std::collections::HashMap::new();
    if let Some(root) = &tree.root {
        replaced_boxes(root, &mut boxes);
    }
    frames
        .iter()
        .filter_map(|fd| {
            let rect = *boxes.get(&fd.node)?;
            if rect.width <= 0.0 || rect.height <= 0.0 {
                return None;
            }
            let child = PageContext {
                base: &fd.base,
                encoding: fd.encoding,
                loader: ctx.loader.clone(),
                fonts: ctx.fonts.clone(),
                extra: &[],
                text_zoom: ctx.text_zoom,
            };
            let size = Size {
                width: rect.width,
                height: rect.height,
            };
            let page = prepare_page_with_frames(&fd.doc, &child, size, &[], &fd.frames);
            Some(FramePage {
                node: fd.node,
                rect,
                scroll: fd.scroll,
                page: Box::new(page),
            })
        })
        .collect()
}

/// Content box de cada caja reemplazada del árbol.
fn replaced_boxes(
    f: &bf_layout::fragment::Fragment,
    out: &mut std::collections::HashMap<bf_dom::NodeId, bf_layout::fragment::Rect>,
) {
    if f.kind == bf_layout::fragment::FragmentKind::Replaced
        && let Some(n) = f.node
    {
        out.entry(n).or_insert_with(|| f.content_rect());
    }
    for c in &f.children {
        replaced_boxes(c, out);
    }
}

/// Atajo: [`prepare_page`] y [`render`] sobre fondo blanco.
pub fn render_document(
    doc: &Document,
    ctx: &PageContext<'_>,
    viewport: Size,
    scale: f32,
) -> Pixmap {
    let page = prepare_page(doc, ctx, viewport);
    render(&page.display_list, viewport, scale, Rgba::WHITE)
}
