//! Decodificación de imágenes: sniffing de firmas (MIME Sniffing §6.1) y los formatos bitmap a
//! RGBA8 premultiplicado. PNG, JPEG y GIF usan los códecs permitidos (`png`, `zune-jpeg`,
//! `gif`); BMP, ICO y WebP (VP8L, VP8 y `ALPH`) son decodificadores propios escritos desde sus
//! specs. [`decode_frames`] da los fotogramas de GIF y APNG animados.
//!
//! El SVG no es un bitmap: `bf-svg` lo interpreta y `bf-paint` lo rasteriza al tamaño pedido.
//!
//! [`decode_frames_shared`] decodifica cada imagen una vez por proceso (la piden `bf-paint` y
//! `bf-web`). [`source`] elige el recurso de un `<img>` por `srcset`, `sizes` y `<picture>` (HTML §4.8.4.3).
//!
//! TODO(roadmap): AVIF (diseño en `docs/pendiente/e7.md`).

#![forbid(unsafe_code)]

mod anim;
mod bmp;
mod gif;
mod ico;
mod jpeg;
mod png;
mod shared;
pub mod source;
mod webp;

use std::fmt;

pub use anim::{Blend, Dispose, Frame, ImageFrames};
pub use shared::{decode_count, decode_frames_shared};

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

/// Error de decodificación con un mensaje fijo.
pub(crate) fn invalid(msg: &str) -> ImageError {
    ImageError::Decode(msg.to_owned())
}

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
    /// Registra el resultado de un [`decode`] (o de cualquier decodificación que devuelva
    /// [`ImageError`]).
    pub fn record<T>(&mut self, result: &Result<T, ImageError>) {
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
    } else if is_svg(bytes) {
        Some("image/svg+xml")
    } else {
        None
    }
}

/// ¿Parece un documento SVG? (`<svg` en los primeros 512 bytes, tras un posible prólogo XML.)
pub fn is_svg(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_lowercase();
    head.contains("<svg")
}

/// El tipo de la imagen: la firma manda sobre `mime_hint` (como en los navegadores).
pub fn kind(bytes: &[u8], mime_hint: Option<&str>) -> Option<&'static str> {
    sniff(bytes).or_else(|| {
        mime_hint.map(|m| match m.split(';').next().unwrap_or("").trim() {
            "image/png" | "image/apng" => "image/png",
            "image/jpeg" | "image/jpg" => "image/jpeg",
            "image/gif" => "image/gif",
            "image/webp" => "image/webp",
            "image/bmp" | "image/x-bmp" | "image/x-ms-bmp" => "image/bmp",
            "image/x-icon" | "image/vnd.microsoft.icon" => "image/x-icon",
            "image/svg+xml" => "image/svg+xml",
            _ => "unknown",
        })
    })
}

pub(crate) fn premultiply(rgba: &mut [u8]) {
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = u16::from(px[3]);
        if a != 255 {
            for c in &mut px[..3] {
                *c = ((u16::from(*c) * a + 127) / 255) as u8;
            }
        }
    }
}

pub(crate) fn check_size(w: u32, h: u32) -> Result<(), ImageError> {
    if w == 0 || h == 0 {
        return Err(invalid("tamaño cero"));
    }
    if u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(invalid("demasiado grande"));
    }
    Ok(())
}

/// Decodifica `bytes` (el primer fotograma, si es animada). La firma manda sobre `mime_hint`.
pub fn decode(bytes: &[u8], mime_hint: Option<&str>) -> Result<Image, ImageError> {
    shared::count_decode();
    decode_first(bytes, mime_hint)
}

fn decode_first(bytes: &[u8], mime_hint: Option<&str>) -> Result<Image, ImageError> {
    match kind(bytes, mime_hint) {
        Some("image/png") => png::decode(bytes),
        Some("image/jpeg") => jpeg::decode(bytes),
        Some("image/gif") => gif::decode(bytes),
        Some("image/bmp") => bmp::decode(bytes),
        Some("image/x-icon") => ico::decode(bytes),
        Some("image/webp") => webp::decode(bytes),
        Some(other) => Err(ImageError::Unsupported(other)),
        None => Err(ImageError::Unsupported("desconocido")),
    }
}

/// Decodifica todos los fotogramas de `bytes`, compuestos sobre el lienzo. Los formatos sin
/// animación (o un PNG sin `acTL`) dan un solo fotograma.
pub fn decode_frames(bytes: &[u8], mime_hint: Option<&str>) -> Result<ImageFrames, ImageError> {
    shared::count_decode();
    match kind(bytes, mime_hint) {
        Some("image/png") => png::decode_frames(bytes),
        Some("image/gif") => gif::decode_frames(bytes),
        _ => decode_first(bytes, mime_hint).map(ImageFrames::still),
    }
}
