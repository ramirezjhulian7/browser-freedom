//! Pintura: lista de visualización (CSS 2.2 Apéndice E) y rasterizado con `tiny-skia`.

#![forbid(unsafe_code)]

pub mod builder;
pub mod display_list;
pub mod images;
pub mod raster;
pub mod reftest;

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

pub use builder::{build_display_list, build_display_list_with_images, document_extent};
pub use display_list::{DisplayItem, DisplayList, GlyphInstance, PathOp, Radii};
pub use images::{PageImages, load_images};
pub use raster::{raster_color, render, render_scrolled};
pub use tiny_skia::{Pixmap, PixmapPaint, Transform};

/// Un documento estilado, maquetado y listo para pintar.
pub struct Page {
    pub fonts: Fonts,
    pub tree: FragmentTree,
    pub display_list: DisplayList,
    pub images: PageImages,
    pub timings: PageTimings,
    pub report: EngineReport,
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
}

/// Estilo → layout → lista de visualización.
pub fn prepare_page(doc: &Document, ctx: &PageContext<'_>, viewport: Size) -> Page {
    let (base, extra, loader, db) = (ctx.base, ctx.extra, ctx.loader.clone(), ctx.fonts.clone());
    let start = Instant::now();
    let media = MediaContext {
        width: f64::from(viewport.width),
        height: f64::from(viewport.height),
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
    let images = load_images(doc, &styles, base, &*loader);
    let images_time = start.elapsed();
    let start = Instant::now();
    let tree =
        bf_layout::layout_document_with_images(doc, &styles, viewport, &fonts, &images.sizes);
    let layout = start.elapsed();
    let start = Instant::now();
    let display_list = build_display_list_with_images(doc, &tree, &fonts, &images);
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
        tree,
        display_list,
        images,
        report,
        timings: PageTimings {
            style,
            images: images_time,
            layout,
            paint: start.elapsed(),
        },
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
