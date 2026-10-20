//! Una cara cargada en memoria: datos, métricas en unidades de la fuente y contornos.

use std::sync::Arc;

use bf_layout::{Outline, PathCommand};
use ttf_parser::{Face, GlyphId, OutlineBuilder};

/// Estilo de una cara (CSS Fonts 4 §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaceStyle {
    Normal,
    Italic,
    Oblique,
}

#[derive(Debug)]
pub struct LoadedFace {
    pub data: Arc<Vec<u8>>,
    pub index: u32,
    pub units_per_em: f32,
    /// Por encima de la baseline (positivo).
    pub ascender: f32,
    /// Por debajo de la baseline (positivo).
    pub descender: f32,
    pub line_gap: f32,
    pub x_height: f32,
    /// Posición del subrayado por debajo de la baseline (positivo hacia abajo).
    pub underline_offset: f32,
    pub underline_thickness: f32,
    /// Posición del tachado por encima de la baseline.
    pub strikeout_offset: f32,
    pub strikeout_thickness: f32,
}

impl LoadedFace {
    /// Valida y lee las métricas de la cara `index` de `data` (TrueType, OpenType o
    /// colección).
    pub fn new(data: Arc<Vec<u8>>, index: u32) -> Option<LoadedFace> {
        let face = Face::parse(&data, index).ok()?;
        let upem = f32::from(face.units_per_em());
        let underline = face.underline_metrics();
        let strikeout = face.strikeout_metrics();
        let x_height = face
            .x_height()
            .filter(|&x| x > 0)
            .map_or(upem * 0.5, f32::from);
        let loaded = LoadedFace {
            units_per_em: upem,
            ascender: f32::from(face.ascender()),
            descender: -f32::from(face.descender()),
            line_gap: f32::from(face.line_gap()).max(0.0),
            x_height,
            underline_offset: underline.map_or(upem * 0.1, |m| -f32::from(m.position)),
            underline_thickness: underline
                .map(|m| f32::from(m.thickness))
                .filter(|&t| t > 0.0)
                .unwrap_or(upem / 20.0),
            strikeout_offset: strikeout
                .map(|m| f32::from(m.position))
                .filter(|&p| p > 0.0)
                .unwrap_or(x_height / 2.0),
            strikeout_thickness: strikeout
                .map(|m| f32::from(m.thickness))
                .filter(|&t| t > 0.0)
                .unwrap_or(upem / 20.0),
            data: data.clone(),
            index,
        };
        Some(loaded)
    }

    pub fn face(&self) -> Option<Face<'_>> {
        Face::parse(&self.data, self.index).ok()
    }

    pub fn outline(&self, glyph: u16) -> Option<Outline> {
        struct Builder(Vec<PathCommand>);
        impl OutlineBuilder for Builder {
            fn move_to(&mut self, x: f32, y: f32) {
                self.0.push(PathCommand::MoveTo(x, y));
            }
            fn line_to(&mut self, x: f32, y: f32) {
                self.0.push(PathCommand::LineTo(x, y));
            }
            fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
                self.0.push(PathCommand::QuadTo(x1, y1, x, y));
            }
            fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
                self.0.push(PathCommand::CurveTo(x1, y1, x2, y2, x, y));
            }
            fn close(&mut self) {
                self.0.push(PathCommand::Close);
            }
        }
        let face = self.face()?;
        let mut builder = Builder(Vec::new());
        face.outline_glyph(GlyphId(glyph), &mut builder)?;
        Some(Outline {
            units_per_em: self.units_per_em,
            commands: builder.0,
        })
    }
}
