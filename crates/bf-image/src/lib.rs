//! Decodificación de imágenes: sniffing de firmas (MIME Sniffing §6.1) y PNG, JPEG y GIF
//! (primer cuadro) a RGBA8 premultiplicado.
//!
//! TODO(roadmap): SVG, WebP, AVIF, ICO/BMP y animación de GIF/APNG.

#![forbid(unsafe_code)]

use std::fmt;
use std::io::Cursor;

/// Imagen decodificada: RGBA8 con alfa premultiplicado, filas de arriba a abajo.
#[derive(Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl fmt::Debug for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Image({}×{})", self.width, self.height)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageError {
    /// Formato reconocido pero sin decodificador (o desconocido).
    Unsupported(&'static str),
    Decode(String),
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageError::Unsupported(t) => write!(f, "formato de imagen no soportado: {t}"),
            ImageError::Decode(e) => write!(f, "imagen inválida: {e}"),
        }
    }
}

impl std::error::Error for ImageError {}

/// Contadores de decodificación de un documento: qué se decodificó, qué formatos no tienen
/// decodificador todavía y qué falló.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecodeStats {
    pub decoded: usize,
    /// Formato (tipo MIME, o `"desconocido"`) → cantidad de imágenes sin decodificador.
    pub unsupported: std::collections::BTreeMap<&'static str, usize>,
    /// Imágenes de un formato soportado que no se pudieron decodificar.
    pub failed: usize,
}

impl DecodeStats {
    /// Registra el resultado de un [`decode`].
    pub fn record(&mut self, result: &Result<Image, ImageError>) {
        match result {
            Ok(_) => self.decoded += 1,
            Err(ImageError::Unsupported(kind)) => *self.unsupported.entry(kind).or_default() += 1,
            Err(ImageError::Decode(_)) => self.failed += 1,
        }
    }
}

/// Límite de píxeles decodificados (protege la memoria).
const MAX_PIXELS: u64 = 1 << 26;

/// "Image type pattern matching algorithm" (MIME Sniffing §6.1).
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    let starts = |p: &[u8]| bytes.starts_with(p);
    if starts(&[0x00, 0x00, 0x01, 0x00]) || starts(&[0x00, 0x00, 0x02, 0x00]) {
        Some("image/x-icon")
    } else if starts(b"BM") {
        Some("image/bmp")
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 14 && &bytes[0..4] == b"RIFF" && &bytes[8..14] == b"WEBPVP" {
        Some("image/webp")
    } else if starts(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if starts(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.len() >= 12 && &bytes[4..12] == b"ftypavif" {
        Some("image/avif")
    } else {
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_lowercase();
        head.contains("<svg").then_some("image/svg+xml")
    }
}

fn premultiply(rgba: &mut [u8]) {
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = u16::from(px[3]);
        if a != 255 {
            for c in &mut px[..3] {
                *c = ((u16::from(*c) * a + 127) / 255) as u8;
            }
        }
    }
}

fn check_size(w: u32, h: u32) -> Result<(), ImageError> {
    if u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(ImageError::Decode("demasiado grande".into()));
    }
    Ok(())
}

fn decode_png(bytes: &[u8]) -> Result<Image, ImageError> {
    let err = |e: png::DecodingError| ImageError::Decode(e.to_string());
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(err)?;
    let (w, h) = (reader.info().width, reader.info().height);
    check_size(w, h)?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(err)?;
    let data = &buf[..info.buffer_size()];
    let n = (w * h) as usize;
    let mut rgba = Vec::with_capacity(n * 4);
    match info.color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(data),
        png::ColorType::Rgb => {
            for p in data.as_chunks::<3>().0 {
                rgba.extend_from_slice(&[p[0], p[1], p[2], 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for p in data.as_chunks::<2>().0 {
                rgba.extend_from_slice(&[p[0], p[0], p[0], p[1]]);
            }
        }
        png::ColorType::Grayscale => {
            for &g in data {
                rgba.extend_from_slice(&[g, g, g, 255]);
            }
        }
        png::ColorType::Indexed => return Err(ImageError::Decode("paleta sin expandir".into())),
    }
    rgba.truncate(n * 4);
    rgba.resize(n * 4, 0);
    premultiply(&mut rgba);
    Ok(Image {
        width: w,
        height: h,
        rgba,
    })
}

fn decode_jpeg(bytes: &[u8]) -> Result<Image, ImageError> {
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;
    let options = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGBA)
        .set_max_width(1 << 14)
        .set_max_height(1 << 14);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(bytes, options);
    let rgba = decoder
        .decode()
        .map_err(|e| ImageError::Decode(format!("{e:?}")))?;
    let info = decoder
        .info()
        .ok_or_else(|| ImageError::Decode("sin cabecera".into()))?;
    let (w, h) = (u32::from(info.width), u32::from(info.height));
    check_size(w, h)?;
    if rgba.len() != (w * h * 4) as usize {
        return Err(ImageError::Decode("tamaño inesperado".into()));
    }
    Ok(Image {
        width: w,
        height: h,
        rgba,
    })
}

/// GIF: el primer cuadro compuesto sobre el lienzo lógico (transparente).
fn decode_gif(bytes: &[u8]) -> Result<Image, ImageError> {
    let err = |e: gif::DecodingError| ImageError::Decode(e.to_string());
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = options.read_info(Cursor::new(bytes)).map_err(err)?;
    let (w, h) = (u32::from(decoder.width()), u32::from(decoder.height()));
    check_size(w, h)?;
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    if let Some(frame) = decoder.read_next_frame().map_err(err)? {
        let (fx, fy) = (u32::from(frame.left), u32::from(frame.top));
        let fw = u32::from(frame.width);
        for (i, px) in frame.buffer.as_chunks::<4>().0.iter().enumerate() {
            let (x, y) = (fx + i as u32 % fw.max(1), fy + i as u32 / fw.max(1));
            if x < w && y < h && px[3] != 0 {
                let o = ((y * w + x) * 4) as usize;
                rgba[o..o + 4].copy_from_slice(px);
            }
        }
    }
    premultiply(&mut rgba);
    Ok(Image {
        width: w,
        height: h,
        rgba,
    })
}

/// Decodifica `bytes`. La firma manda sobre `mime_hint` (como en los navegadores).
pub fn decode(bytes: &[u8], mime_hint: Option<&str>) -> Result<Image, ImageError> {
    let kind = sniff(bytes).or_else(|| {
        mime_hint.map(|m| match m.split(';').next().unwrap_or("").trim() {
            "image/png" => "image/png",
            "image/jpeg" | "image/jpg" => "image/jpeg",
            "image/gif" => "image/gif",
            "image/svg+xml" => "image/svg+xml",
            _ => "unknown",
        })
    });
    match kind {
        Some("image/png") => decode_png(bytes),
        Some("image/jpeg") => decode_jpeg(bytes),
        Some("image/gif") => decode_gif(bytes),
        Some(other) => Err(ImageError::Unsupported(other)),
        None => Err(ImageError::Unsupported("desconocido")),
    }
}
