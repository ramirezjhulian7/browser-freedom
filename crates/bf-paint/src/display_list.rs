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
    /// Una capa de `background-image` que es un `<gradient>` (CSS Images 3 §3): como
    /// [`DisplayItem::BackgroundImage`], pero el tile se genera a resolución de dispositivo.
    /// La geometría de `gradient` está en coordenadas del tile (origen en su esquina).
    Gradient {
        area: Rect,
        radii: Radii,
        tile: Rect,
        xs: Option<Vec<f32>>,
        ys: Option<Vec<f32>>,
        gradient: Arc<ResolvedGradient>,
    },
    /// Una sombra de `box-shadow` (CSS Backgrounds 3 §7.1): `rect` y `radii` son la border box
    /// (exterior) o la padding box (`inset`) del elemento, que recortan la sombra.
    BoxShadow {
        rect: Rect,
        radii: Radii,
        shadow: BoxShadowParams,
    },
    /// Abre un grupo que se pinta como sombra desenfocada (`text-shadow`, CSS Text Decoration
    /// 3 §4): lo que sigue hasta [`DisplayItem::PopShadow`] se pinta en una capa aparte (ya
    /// desplazado y en el color de la sombra), se desenfoca con σ = `blur` / 2 y se compone.
    /// `bounds` acota (en px CSS) lo que se pinta dentro del grupo, sin el desenfoque.
    PushShadow {
        bounds: Rect,
        blur: f32,
    },
    PopShadow,
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
    /// Recorte al área que pinta un borde con esos anchos, estilos y radios (`background-clip:
    /// border-area`, CSS Backgrounds 4 §2.1); se cierra con [`DisplayItem::PopClip`].
    PushClipBorder {
        rect: Rect,
        widths: Edges,
        styles: [BorderStyle; 4],
        radii: Radii,
    },
    /// Recorte a la forma de los glifos de `glyphs` (ítems `Text`) (`background-clip: text`,
    /// CSS Backgrounds 4 §2.1); se cierra con [`DisplayItem::PopClip`].
    PushClipText {
        glyphs: Vec<DisplayItem>,
    },
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

/// Parámetros de una sombra de caja ya resueltos a px CSS.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxShadowParams {
    pub offset: (f32, f32),
    pub blur: f32,
    pub spread: f32,
    pub color: Rgba,
    pub inset: bool,
}

/// Forma de un gradiente resuelto, en px CSS relativos al origen del tile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GradientShape {
    /// La línea del gradiente va de `start` (0) a `end` (1) (CSS Images 3 §3.1.1).
    Linear { start: (f32, f32), end: (f32, f32) },
    /// Elipse de radios `radii` centrada en `center`; 1 = la forma final (§3.2.1).
    Radial {
        center: (f32, f32),
        radii: (f32, f32),
    },
    /// Ángulo inicial `from` en grados (0 = arriba, sentido horario) (CSS Images 4 §3.3).
    Conic { center: (f32, f32), from: f32 },
}

/// Una parada ya corregida (CSS Images 3 §3.4.3): posición como fracción de la línea del
/// gradiente (o de la vuelta, en los cónicos), no decreciente.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradientStop {
    pub offset: f32,
    pub color: Rgba,
    /// Indicación de interpolación hacia la parada siguiente, como fracción del tramo
    /// (`None` = punto medio, interpolación lineal).
    pub hint: Option<f32>,
}

/// Un gradiente listo para rasterizar, independiente de `bf_style`.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedGradient {
    pub shape: GradientShape,
    pub stops: Vec<GradientStop>,
    pub repeating: bool,
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
