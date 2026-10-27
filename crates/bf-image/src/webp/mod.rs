//! WebP: el contenedor RIFF ("WebP Container Specification") con `VP8 ` (con pérdida, RFC
//! 6386), `VP8L` (sin pérdida, RFC 9649) o el formato extendido `VP8X` con el canal `ALPH`.
//! `ICCP`, `EXIF`, `XMP ` y los chunks desconocidos se ignoran. De una animación
//! (`ANIM`/`ANMF`) se da el primer fotograma compuesto sobre el lienzo transparente.

mod alpha;
mod vp8;
mod vp8l;

use crate::{Image, ImageError, check_size, invalid, premultiply};

/// Bandera de animación de `VP8X`.
const ANIMATION_FLAG: u8 = 0x02;

struct Chunk<'a> {
    fourcc: [u8; 4],
    data: &'a [u8],
}

fn le24(b: &[u8]) -> u32 {
    u32::from(b[0]) | (u32::from(b[1]) << 8) | (u32::from(b[2]) << 16)
}

/// Recorre los chunks de `data` (cabecera de 8 bytes y relleno a tamaño par).
fn chunks(mut data: &[u8]) -> impl Iterator<Item = Result<Chunk<'_>, ImageError>> {
    std::iter::from_fn(move || {
        if data.is_empty() {
            return None;
        }
        let Some(head) = data.get(..8) else {
            data = &[];
            return Some(Err(invalid("chunk WebP truncado")));
        };
        let fourcc = [head[0], head[1], head[2], head[3]];
        let size = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
        let Some(payload) = size.checked_add(8).and_then(|end| data.get(8..end)) else {
            data = &[];
            return Some(Err(invalid("chunk WebP truncado")));
        };
        let next = (8 + size + (size & 1)).min(data.len());
        data = &data[next..];
        Some(Ok(Chunk {
            fourcc,
            data: payload,
        }))
    })
}

/// El contenido del RIFF después de `WEBP`.
fn riff_body(bytes: &[u8]) -> Result<&[u8], ImageError> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return Err(invalid("sin firma RIFF/WEBP"));
    }
    let size = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    if size < 12 {
        return Err(invalid("RIFF demasiado pequeño"));
    }
    size.checked_add(8)
        .and_then(|end| bytes.get(12..end))
        .ok_or_else(|| invalid("WebP truncado"))
}

pub fn decode(bytes: &[u8]) -> Result<Image, ImageError> {
    let body = riff_body(bytes)?;
    let mut it = chunks(body);
    let first = it.next().ok_or_else(|| invalid("WebP vacío"))??;
    let (width, height, mut rgba) = match &first.fourcc {
        b"VP8 " | b"VP8L" => decode_image(&first, None)?,
        b"VP8X" => decode_extended(first.data, it)?,
        _ => return Err(invalid("chunk inicial de WebP desconocido")),
    };
    premultiply(&mut rgba);
    Ok(Image {
        width,
        height,
        rgba,
    })
}

/// Decodifica un chunk `VP8 ` (con su `ALPH`, si lo hay) o `VP8L` a RGBA sin premultiplicar.
fn decode_image(chunk: &Chunk, alph: Option<&[u8]>) -> Result<(u32, u32, Vec<u8>), ImageError> {
    if &chunk.fourcc == b"VP8L" {
        let (w, h, argb) = vp8l::decode(chunk.data)?;
        let mut rgba = Vec::with_capacity(argb.len() * 4);
        for p in argb {
            let [b, g, r, a] = p.to_le_bytes();
            rgba.extend_from_slice(&[r, g, b, a]);
        }
        return Ok((w, h, rgba));
    }
    let frame = vp8::decode(chunk.data)?;
    let alpha = alph
        .map(|a| alpha::decode(a, frame.width, frame.height))
        .transpose()?;
    let rgba = frame.to_rgba(alpha.as_deref());
    Ok((frame.width as u32, frame.height as u32, rgba))
}

/// Formato extendido: `VP8X` seguido de los demás chunks.
fn decode_extended<'a>(
    header: &[u8],
    rest: impl Iterator<Item = Result<Chunk<'a>, ImageError>>,
) -> Result<(u32, u32, Vec<u8>), ImageError> {
    let header = header.get(..10).ok_or_else(|| invalid("VP8X truncado"))?;
    let animated = header[0] & ANIMATION_FLAG != 0;
    let cw = le24(&header[4..7]) + 1;
    let ch = le24(&header[7..10]) + 1;
    check_size(cw, ch)?;
    let mut alph = None;
    for chunk in rest {
        let chunk = chunk?;
        match &chunk.fourcc {
            b"ANMF" if animated => return first_frame(chunk.data, cw, ch),
            b"ALPH" if !animated && alph.is_none() => alph = Some(chunk.data),
            b"VP8 " | b"VP8L" if !animated => {
                let (w, h, rgba) = decode_image(&chunk, alph)?;
                if (w, h) != (cw, ch) {
                    return Err(invalid("imagen de tamaño distinto al lienzo"));
                }
                return Ok((w, h, rgba));
            }
            _ => {}
        }
    }
    Err(invalid("WebP sin imagen"))
}

/// El primer fotograma (`ANMF`) puesto en su posición sobre el lienzo transparente. Sobre un
/// lienzo vacío, la mezcla alfa y el reemplazo dan lo mismo.
fn first_frame(data: &[u8], cw: u32, ch: u32) -> Result<(u32, u32, Vec<u8>), ImageError> {
    let head = data.get(..16).ok_or_else(|| invalid("ANMF truncado"))?;
    let x = le24(&head[0..3]) as usize * 2;
    let y = le24(&head[3..6]) as usize * 2;
    let fw = le24(&head[6..9]) + 1;
    let fh = le24(&head[9..12]) + 1;
    if x + fw as usize > cw as usize || y + fh as usize > ch as usize {
        return Err(invalid("fotograma fuera del lienzo"));
    }
    let mut alph = None;
    for chunk in chunks(&data[16..]) {
        let chunk = chunk?;
        match &chunk.fourcc {
            b"ALPH" if alph.is_none() => alph = Some(chunk.data),
            b"VP8 " | b"VP8L" => {
                let (w, h, rgba) = decode_image(&chunk, alph)?;
                if (w, h) != (fw, fh) {
                    return Err(invalid("fotograma de tamaño distinto al declarado"));
                }
                let (canvas_w, w) = (cw as usize, w as usize);
                let mut canvas = vec![0u8; canvas_w * ch as usize * 4];
                for (row, src) in rgba.chunks_exact(w * 4).enumerate() {
                    let start = ((y + row) * canvas_w + x) * 4;
                    canvas[start..start + w * 4].copy_from_slice(src);
                }
                return Ok((cw, ch, canvas));
            }
            _ => {}
        }
    }
    Err(invalid("ANMF sin imagen"))
}

/// El contenido del primer chunk `VP8 ` (para las pruebas del decodificador VP8).
#[cfg(test)]
fn find_vp8(bytes: &[u8]) -> Option<&[u8]> {
    fn search(data: &[u8]) -> Option<&[u8]> {
        for chunk in chunks(data) {
            let chunk = chunk.ok()?;
            match &chunk.fourcc {
                b"VP8 " => return Some(chunk.data),
                b"ANMF" => return search(chunk.data.get(16..)?),
                _ => {}
            }
        }
        None
    }
    search(riff_body(bytes).ok()?)
}
