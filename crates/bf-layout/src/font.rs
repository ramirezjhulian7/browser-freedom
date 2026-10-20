//! Fuentes abstractas: el layout solo conoce métricas y glifos posicionados. `bf-font`
//! implementa [`FontProvider`] con fuentes reales; [`TestFont`] da resultados deterministas
//! para los tests.

use std::sync::Arc;

use bf_style::ComputedStyle;
use bf_style::properties::types::{FontFamily, FontStyle};

/// Lo que el layout pide a una fuente.
#[derive(Debug, Clone, Copy)]
pub struct FontRequest<'a> {
    pub family: &'a FontFamily,
    /// Tamaño en px.
    pub size: f32,
    pub weight: f32,
    pub italic: bool,
}

/// La petición de fuente de un estilo computado.
pub fn font_request(style: &ComputedStyle) -> FontRequest<'_> {
    FontRequest {
        family: &style.font_family,
        size: style.font_size_px(),
        weight: style.font_weight.value(),
        italic: !matches!(style.font_style, FontStyle::Normal),
    }
}

/// Métricas en px para el tamaño pedido. Los offsets de las decoraciones se miden desde la
/// baseline: `underline_offset` hacia abajo y `strikeout_offset` hacia arriba.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontMetrics {
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
    pub x_height: f32,
    pub underline_offset: f32,
    pub underline_thickness: f32,
    pub strikeout_offset: f32,
    pub strikeout_thickness: f32,
}

impl FontMetrics {
    /// `line-height: normal`.
    pub fn normal_line_height(&self) -> f32 {
        self.ascent + self.descent + self.line_gap
    }
}

/// Identifica una cara concreta dentro de un [`FontProvider`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct FontKey(pub u32);

/// Un glifo ya posicionado: avance y desplazamientos en px, el índice (en bytes) del
/// carácter de origen, el id del glifo y la cara que lo dibuja.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    pub advance: f32,
    pub cluster: usize,
    pub id: u16,
    pub font: FontKey,
    /// Desplazamiento horizontal respecto de la posición de la pluma.
    pub x_offset: f32,
    /// Desplazamiento vertical hacia arriba respecto de la baseline.
    pub y_offset: f32,
}

impl Glyph {
    /// Glifo sin desplazamientos.
    pub fn simple(advance: f32, cluster: usize, id: u16, font: FontKey) -> Self {
        Glyph {
            advance,
            cluster,
            id,
            font,
            x_offset: 0.0,
            y_offset: 0.0,
        }
    }
}

/// Comando de un contorno, en unidades de la fuente (y hacia arriba).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathCommand {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    QuadTo(f32, f32, f32, f32),
    CurveTo(f32, f32, f32, f32, f32, f32),
    Close,
}

/// Contorno de un glifo.
#[derive(Debug, Clone, PartialEq)]
pub struct Outline {
    pub units_per_em: f32,
    pub commands: Vec<PathCommand>,
}

pub trait FontProvider {
    fn metrics(&self, request: &FontRequest<'_>) -> FontMetrics;
    fn shape(&self, text: &str, request: &FontRequest<'_>) -> Vec<Glyph>;

    /// Contorno del glifo `glyph` de la cara `font` (`None` si no tiene, como el espacio).
    fn outline(&self, font: FontKey, glyph: u16) -> Option<Arc<Outline>> {
        let _ = (font, glyph);
        None
    }

    /// Escalas verticales (sobre y bajo la baseline) que llevan el ascent y el descent de la
    /// cara a los valores redondeados que usa el layout, para que los contornos coincidan con
    /// las cajas.
    ///
    /// No es comportamiento de ningún spec: es una aproximación propia al hinting de los
    /// navegadores (que ajustan los contornos a la grilla de píxeles). Se puede desactivar con
    /// [`FontProvider::vertical_fit_enabled`].
    fn vertical_fit(&self, font: FontKey, size: f32) -> (f32, f32) {
        let _ = (font, size);
        (1.0, 1.0)
    }

    /// ¿Se aplica [`FontProvider::vertical_fit`] al pintar?
    fn vertical_fit_enabled(&self) -> bool {
        true
    }
}

/// Fuente de prueba: cada carácter avanza 0.5em; ascent 0.8em, descent 0.2em, sin line gap.
/// No tiene contornos.
#[derive(Debug, Clone, Copy, Default)]
pub struct TestFont;

impl FontProvider for TestFont {
    fn metrics(&self, request: &FontRequest<'_>) -> FontMetrics {
        let s = request.size;
        FontMetrics {
            ascent: 0.8 * s,
            descent: 0.2 * s,
            line_gap: 0.0,
            x_height: 0.5 * s,
            underline_offset: 0.1 * s,
            underline_thickness: 0.05 * s,
            strikeout_offset: 0.25 * s,
            strikeout_thickness: 0.05 * s,
        }
    }

    fn shape(&self, text: &str, request: &FontRequest<'_>) -> Vec<Glyph> {
        text.char_indices()
            .map(|(cluster, c)| Glyph::simple(0.5 * request.size, cluster, c as u16, FontKey(0)))
            .collect()
    }
}

/// Adapta un [`FontProvider`] a las métricas que necesita el cómputo de `ex` y `ch`.
pub struct StyleMetrics<'a>(pub &'a dyn FontProvider);

impl bf_style::FontMetricsSource for StyleMetrics<'_> {
    fn ex_ch(&self, style: &ComputedStyle) -> (f32, f32) {
        let request = font_request(style);
        let x_height = self.0.metrics(&request).x_height;
        let zero = self.0.shape("0", &request).iter().map(|g| g.advance).sum();
        (x_height, zero)
    }
}
