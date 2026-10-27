//! ICO y CUR: el directorio de imágenes y la de más resolución (a igual tamaño, la de más
//! bits por píxel). Cada imagen es un PNG embebido o un DIB con máscara AND.

use crate::{Image, ImageError, invalid};

pub fn decode(bytes: &[u8]) -> Result<Image, ImageError> {
    let u16_at = |o: usize| bytes.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]));
    let u32_at = |o: usize| {
        bytes
            .get(o..o + 4)
            .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    };
    let count = u16_at(4).ok_or_else(|| invalid("ICO truncado"))? as usize;
    let mut best: Option<(u64, u16, usize, usize)> = None;
    for i in 0..count {
        let e = 6 + i * 16;
        let (Some(&w), Some(&h), Some(bpp), Some(len), Some(off)) = (
            bytes.get(e),
            bytes.get(e + 1),
            u16_at(e + 6),
            u32_at(e + 8),
            u32_at(e + 12),
        ) else {
            break;
        };
        let dim = |v: u8| if v == 0 { 256u64 } else { u64::from(v) };
        let area = dim(w) * dim(h);
        if best.is_none_or(|(a, b, _, _)| (area, bpp) > (a, b)) {
            best = Some((area, bpp, off as usize, len as usize));
        }
    }
    let (_, _, off, len) = best.ok_or_else(|| invalid("ICO sin imágenes"))?;
    let data = bytes
        .get(off..off.saturating_add(len).min(bytes.len()))
        .ok_or_else(|| invalid("ICO truncado"))?;
    if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        crate::png::decode(data)
    } else {
        crate::bmp::decode_dib(data, None, true)
    }
}
