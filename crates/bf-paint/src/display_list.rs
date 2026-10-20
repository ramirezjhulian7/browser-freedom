//! Lista de visualización: las operaciones de pintura en orden, en coordenadas de página (px
//! CSS). El constructor ([`crate::builder`]) la arma desde el árbol de fragmentos siguiendo
//! CSS 2.2 Apéndice E; el rasterizador ([`crate::raster`]) la ejecuta.

use std::sync::Arc;

use bf_layout::fragment::{Edges, Rect};
use bf_layout::{FontKey, Outline, Widget};
use bf_style::properties::types::{BorderStyle, TextDecorationStyle};
use bf_style::values::color::Rgba;

/// Radios (horizontal, vertical) de las esquinas: arriba-izquierda, arriba-derecha,
/// abajo-derecha y abajo-izquierda.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Radii(pub [(f32, f32); 4]);

impl Radii {
    pub fn is_zero(&self) -> bool {
        self.0.iter().all(|&(h, v)| h <= 0.0 || v <= 0.0)
    }

    /// Escala los radios para que los de un mismo lado no se solapen (CSS Backgrounds 3
    /// §5.5).
    pub fn fit(mut self, rect: &Rect) -> Radii {
        let [tl, tr, br, bl] = self.0;
        let ratio = |len: f32, sum: f32| if sum > 0.0 { len / sum } else { f32::INFINITY };
        let f = ratio(rect.width, tl.0 + tr.0)
            .min(ratio(rect.width, bl.0 + br.0))
            .min(ratio(rect.height, tl.1 + bl.1))
            .min(ratio(rect.height, tr.1 + br.1));
        if f < 1.0 {
            for r in &mut self.0 {
                r.0 *= f;
                r.1 *= f;
            }
        }
        self
    }

    /// Radios de la curva interior (padding box) dados los anchos de borde (§5.6).
    pub fn shrink(&self, e: &Edges) -> Radii {
        let [tl, tr, br, bl] = self.0;
        let s = |r: f32, w: f32| (r - w).max(0.0);
        Radii([
            (s(tl.0, e.left), s(tl.1, e.top)),
            (s(tr.0, e.right), s(tr.1, e.top)),
            (s(br.0, e.right), s(br.1, e.bottom)),
            (s(bl.0, e.left), s(bl.1, e.bottom)),
        ])
    }
}

/// Un glifo a dibujar, relativo al origen del texto (la baseline).
#[derive(Debug, Clone)]
pub struct GlyphInstance {
    pub id: u16,
    pub x: f32,
    pub y: f32,
    pub outline: Option<Arc<Outline>>,
}

#[derive(Debug, Clone)]
pub enum DisplayItem {
    /// Relleno (fondos). `radii` cero es un rectángulo.
    Rect {
        rect: Rect,
        color: Rgba,
        radii: Radii,
    },
    /// Bordes de una border box. Orden de lados: arriba, derecha, abajo, izquierda.
    Border {
        rect: Rect,
        widths: Edges,
        colors: [Rgba; 4],
        styles: [BorderStyle; 4],
        radii: Radii,
    },
    /// Texto de una sola cara.
    Text {
        glyphs: Vec<GlyphInstance>,
        font: FontKey,
        size: f32,
        color: Rgba,
        origin: (f32, f32),
        /// Escalas verticales del contorno sobre y bajo la baseline (ver
        /// `FontProvider::vertical_fit`).
        y_fit: (f32, f32),
    },
    /// Una imagen escalada a `rect` (contenido de `<img>`, marcadores).
    Image {
        rect: Rect,
        image: Arc<tiny_skia::Pixmap>,
    },
    /// Una capa de `background-image`: el área pintada (con sus radios), el primer tile y cómo
    /// se repite en cada eje. `xs`/`ys` son las posiciones de los tiles en los ejes que no se
    /// repiten indefinidamente (`no-repeat`, `space`); `None` = se repite en todo el área.
    BackgroundImage {
        area: Rect,
        radii: Radii,
        tile: Rect,
        xs: Option<Vec<f32>>,
        ys: Option<Vec<f32>>,
        image: Arc<tiny_skia::Pixmap>,
    },
    /// Parte nativa de un control de formulario en `rect`; `color` es el del texto del
    /// control.
    Widget {
        rect: Rect,
        widget: Widget,
        color: Rgba,
    },
    /// Un trazo vectorial en px CSS (iconos de la interfaz): relleno opcional y contorno de
    /// `stroke.0` px con extremos y uniones redondeados.
    Path {
        ops: Vec<PathOp>,
        fill: Option<Rgba>,
        stroke: Option<(f32, Rgba)>,
    },
    /// Decoración de texto horizontal: `rect` es la banda de la línea.
    Line {
        rect: Rect,
        color: Rgba,
        style: TextDecorationStyle,
    },
    PushClip {
        rect: Rect,
        radii: Radii,
    },
    PopClip,
    PushOpacity(f32),
    PopOpacity,
    PushTransform {
        dx: f32,
        dy: f32,
    },
    PopTransform,
    /// Contenido de una caja con `position: fixed`: se pinta respecto del viewport, sin el
    /// desplazamiento del scroll.
    PushFixed,
    PopFixed,
}

/// Comando de un [`DisplayItem::Path`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathOp {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    QuadTo(f32, f32, f32, f32),
    CubicTo(f32, f32, f32, f32, f32, f32),
    Close,
}

#[derive(Debug, Clone, Default)]
pub struct DisplayList {
    pub items: Vec<DisplayItem>,
}

impl DisplayList {
    pub fn push(&mut self, item: DisplayItem) {
        self.items.push(item);
    }
}
