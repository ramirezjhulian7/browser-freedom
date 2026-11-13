//! Una cara cargada en memoria: datos, métricas en unidades de la fuente y contornos.

use std::sync::Arc;

use bf_layout::{GlyphBitmap, Outline, PathCommand};
use ttf_parser::{Face, GlyphId, OutlineBuilder, RasterGlyphImage, RasterImageFormat};

/// Etiqueta del eje de peso de las fuentes variables (OpenType, `fvar`).
pub const WGHT: ttf_parser::Tag = ttf_parser::Tag::from_bytes(b"wght");
/// Etiqueta del eje de tamaño óptico (OpenType, `fvar`; CSS Fonts 4 §7.4).
pub const OPSZ: ttf_parser::Tag = ttf_parser::Tag::from_bytes(b"opsz");

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
    /// E5: valor del eje `wght` de una fuente variable (CSS Fonts 4 §2.5: una instancia por
    /// peso); `None` deja los ejes en su valor por defecto.
    pub wght: Option<f32>,
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
            wght: None,
        };
        Some(loaded)
    }

    /// E5: la cara con el eje `wght` en `wght` (ver [`LoadedFace::wght`]).
    pub fn with_wght(mut self, wght: Option<f32>) -> LoadedFace {
        self.wght = wght;
        self
    }

    pub fn face(&self) -> Option<Face<'_>> {
        self.face_at(None)
    }

    /// E5: [`LoadedFace::face`] con el eje `opsz` en `opsz` si se da (`font-optical-sizing:
    /// auto`, CSS Fonts 4 §7.4).
    pub fn face_at(&self, opsz: Option<f32>) -> Option<Face<'_>> {
        let mut face = Face::parse(&self.data, self.index).ok()?;
        if let Some(w) = self.wght {
            face.set_variation(WGHT, w);
        }
        if let Some(o) = opsz {
            face.set_variation(OPSZ, o);
        }
        Some(face)
    }

    /// E5: los ejes fijados de la cara (`wght` de la instancia y el `opsz` dado), para el
    /// shaper.
    pub fn variations(&self, opsz: Option<f32>) -> Vec<(ttf_parser::Tag, f32)> {
        let mut v = Vec::new();
        if let Some(w) = self.wght {
            v.push((WGHT, w));
        }
        if let Some(o) = opsz {
            v.push((OPSZ, o));
        }
        v
    }

    pub fn outline(&self, glyph: u16) -> Option<Outline> {
        self.outline_at(glyph, None)
    }

    /// E5: [`LoadedFace::outline`] con el tamaño óptico `opsz` (ver [`LoadedFace::face_at`]).
    pub fn outline_at(&self, glyph: u16, opsz: Option<f32>) -> Option<Outline> {
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
        let face = self.face_at(opsz)?;
        let mut builder = Builder(Vec::new());
        let has_outline = face.outline_glyph(GlyphId(glyph), &mut builder).is_some();
        let bitmaps = bitmaps(&face, GlyphId(glyph));
        if !has_outline && bitmaps.is_empty() {
            return None;
        }
        Some(Outline {
            units_per_em: self.units_per_em,
            commands: builder.0,
            bitmaps,
        })
    }
}

/// E5: los mapas de bits PNG de un glifo, de menor a mayor tamaño: todos los de la tabla
/// `sbix` (Apple; los gráficos `dupe` los resuelve `ttf-parser`) o, sin ella, el mayor de
/// `CBDT`/`EBDT`. Los demás formatos (máscaras sin comprimir) no se usan.
fn bitmaps(face: &Face<'_>, glyph: GlyphId) -> Vec<GlyphBitmap> {
    let to_bitmap = |img: RasterGlyphImage<'_>| {
        (img.format == RasterImageFormat::PNG).then(|| GlyphBitmap {
            ppem: img.pixels_per_em,
            x: f32::from(img.x),
            y: f32::from(img.y),
            width: img.width,
            height: img.height,
            png: Arc::from(img.data),
        })
    };
    let mut out: Vec<GlyphBitmap> = match face.tables().sbix {
        Some(sbix) => sbix
            .strikes
            .into_iter()
            .filter_map(|strike| strike.get(glyph))
            .filter_map(to_bitmap)
            .collect(),
        None => face
            .glyph_raster_image(glyph, u16::MAX)
            .and_then(to_bitmap)
            .into_iter()
            .collect(),
    };
    out.sort_by_key(|b| b.ppem);
    out.dedup_by_key(|b| b.ppem);
    out
}
