//! BMP (DIB de Windows): cabeceras `BITMAPCOREHEADER`, `BITMAPINFOHEADER` y sus versiones
//! V2–V5 y OS/2 2.x; 1, 2, 4, 8, 16, 24 y 32 bits por píxel; `BI_RGB`, `BI_RLE8`, `BI_RLE4`,
//! `BI_BITFIELDS` y `BI_ALPHABITFIELDS`; PNG y JPEG embebidos (`BI_PNG`, `BI_JPEG`). También
//! decodifica el DIB de los iconos ([`decode_dib`]), con su máscara AND.

use crate::{Image, ImageError, check_size, invalid, premultiply};

fn u16_at(b: &[u8], o: usize) -> Result<u16, ImageError> {
    b.get(o..o + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| invalid("BMP truncado"))
}

fn u32_at(b: &[u8], o: usize) -> Result<u32, ImageError> {
    b.get(o..o + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| invalid("BMP truncado"))
}

pub fn decode(bytes: &[u8]) -> Result<Image, ImageError> {
    if !bytes.starts_with(b"BM") {
        return Err(invalid("sin firma BM"));
    }
    let off = u32_at(bytes, 10)? as usize;
    let dib = bytes.get(14..).ok_or_else(|| invalid("BMP truncado"))?;
    decode_dib(dib, off.checked_sub(14), false)
}

/// Máscara de un canal: desplazamiento y cantidad de bits.
#[derive(Clone, Copy)]
struct Mask {
    mask: u32,
    shift: u32,
    bits: u32,
}

impl Mask {
    fn new(mask: u32) -> Mask {
        if mask == 0 {
            return Mask {
                mask,
                shift: 0,
                bits: 0,
            };
        }
        let shift = mask.trailing_zeros();
        Mask {
            mask,
            shift,
            bits: (mask >> shift).trailing_ones(),
        }
    }

    /// El canal escalado a 8 bits.
    fn get(&self, px: u32) -> u8 {
        if self.bits == 0 {
            return 0;
        }
        let v = (px & self.mask) >> self.shift;
        let max = (1u64 << self.bits.min(32)) - 1;
        ((u64::from(v) * 255 + max / 2) / max) as u8
    }
}

/// Decodifica un DIB que empieza en su cabecera. `pixels`: posición de los píxeles relativa
/// a `dib` (la del archivo BMP); `None` = justo después de la paleta. `ico`: la altura es la
/// del XOR más la máscara AND, que viene después.
pub(crate) fn decode_dib(
    dib: &[u8],
    pixels: Option<usize>,
    ico: bool,
) -> Result<Image, ImageError> {
    let header = u32_at(dib, 0)? as usize;
    let (width, mut height, bpp, mut compression, palette_entry) = if header == 12 {
        let w = i32::from(u16_at(dib, 4)?);
        let h = i32::from(u16_at(dib, 6)? as i16);
        (w, h, u16_at(dib, 10)?, 0, 3)
    } else if header >= 16 {
        let w = u32_at(dib, 4)? as i32;
        let h = u32_at(dib, 8)? as i32;
        let comp = if header >= 20 { u32_at(dib, 16)? } else { 0 };
        (w, h, u16_at(dib, 14)?, comp, 4)
    } else {
        return Err(invalid("cabecera DIB desconocida"));
    };
    // OS/2 2.x: la compresión 3 es Huffman 1D y la 4 es RLE24 (sin soporte).
    if header == 64 && (compression == 3 || compression == 4) {
        return Err(invalid("compresión OS/2 sin soporte"));
    }
    if ico {
        height /= 2;
    }
    let top_down = height < 0;
    let (w, h) = (width.unsigned_abs(), height.unsigned_abs());
    if width <= 0 {
        return Err(invalid("ancho inválido"));
    }
    check_size(w, h)?;
    if compression == 4 || compression == 5 {
        // BI_JPEG / BI_PNG: la imagen embebida tal cual.
        let start = pixels.unwrap_or(header);
        let data = dib.get(start..).ok_or_else(|| invalid("BMP truncado"))?;
        return if compression == 5 {
            crate::png::decode(data)
        } else {
            crate::jpeg::decode(data)
        };
    }
    // Máscaras de canal: en la cabecera (V2+) o, con `INFOHEADER`, justo después.
    let mut masks_end = header;
    let masks = if compression == 3 || compression == 6 {
        let n = if compression == 6 { 4 } else { 3 };
        let at = if header >= 52 { 40 } else { header };
        if header < 52 {
            masks_end = header + 4 * n;
        }
        let a = if n == 4 || header >= 56 {
            u32_at(dib, at + 12).unwrap_or(0)
        } else {
            0
        };
        Some([
            u32_at(dib, at)?,
            u32_at(dib, at + 4)?,
            u32_at(dib, at + 8)?,
            a,
        ])
    } else {
        None
    };
    if compression == 6 {
        compression = 3;
    }
    // V3+ con `BI_RGB`: la máscara alfa de la cabecera cuenta en 32 bpp.
    let header_alpha = if header >= 56 && compression == 0 && bpp == 32 {
        u32_at(dib, 52).unwrap_or(0)
    } else {
        0
    };
    let palette_len = if bpp <= 8 {
        let used = if header >= 36 { u32_at(dib, 32)? } else { 0 };
        let max = 1u32 << bpp;
        if used == 0 || used > max { max } else { used }
    } else {
        0
    } as usize;
    let mut palette = Vec::with_capacity(palette_len);
    for i in 0..palette_len {
        let o = masks_end + i * palette_entry;
        match dib.get(o..o + 3) {
            Some(p) => palette.push([p[2], p[1], p[0], 255]),
            None => break,
        }
    }
    let start = pixels.unwrap_or(masks_end + palette_len * palette_entry);
    let data = dib.get(start..).ok_or_else(|| invalid("BMP truncado"))?;
    let stride = ((w as usize * bpp as usize).div_ceil(32)) * 4;
    // Una cabecera que promete mucho más de lo que trae es un archivo dañado: no se reserva
    // un lienzo enorme para unos pocos bytes (RLE expande hasta 255 píxeles por par).
    let promised = if compression == 1 || compression == 2 {
        w as usize * h as usize / 128
    } else {
        stride * h as usize / 4
    };
    if promised > data.len() + 4096 {
        return Err(invalid("BMP truncado"));
    }
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    // Fila de archivo → fila de la imagen.
    let row_of = |r: u32| if top_down { r } else { h - 1 - r };
    let pal = |i: usize| palette.get(i).copied().unwrap_or([0, 0, 0, 255]);
    let mut has_alpha = false;
    match (compression, bpp) {
        (1, 8) | (2, 4) => rle(data, bpp, w, h, &palette, top_down, &mut rgba)?,
        (0, 1 | 2 | 4 | 8) => {
            for r in 0..h {
                let Some(row) = data.get(r as usize * stride..) else {
                    break;
                };
                let y = row_of(r);
                for x in 0..w {
                    let bit = x as usize * bpp as usize;
                    let Some(&byte) = row.get(bit / 8) else {
                        break;
                    };
                    let idx = (u16::from(byte) >> (8 - bpp as usize - bit % 8)) & ((1 << bpp) - 1);
                    let o = ((y * w + x) * 4) as usize;
                    rgba[o..o + 4].copy_from_slice(&pal(idx as usize));
                }
            }
        }
        (0 | 3, 16 | 24 | 32) => {
            let [mr, mg, mb, ma] = match masks {
                Some(m) => m.map(Mask::new),
                None if bpp == 16 => [0x7C00, 0x03E0, 0x001F, 0].map(Mask::new),
                None => [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, header_alpha].map(Mask::new),
            };
            let ma = if ma.mask == 0 && ico && bpp == 32 {
                // Los iconos de 32 bpp llevan alfa en el byte alto.
                Mask::new(0xFF00_0000)
            } else {
                ma
            };
            let bytes_pp = bpp as usize / 8;
            for r in 0..h {
                let y = row_of(r);
                for x in 0..w {
                    let o = r as usize * stride + x as usize * bytes_pp;
                    let Some(p) = data.get(o..o + bytes_pp) else {
                        break;
                    };
                    let px = match bytes_pp {
                        2 => u32::from(u16::from_le_bytes([p[0], p[1]])),
                        3 => u32::from_le_bytes([p[0], p[1], p[2], 0]),
                        _ => u32::from_le_bytes([p[0], p[1], p[2], p[3]]),
                    };
                    let a = if ma.mask == 0 { 255 } else { ma.get(px) };
                    has_alpha |= ma.mask != 0 && a != 0;
                    let d = ((y * w + x) * 4) as usize;
                    rgba[d..d + 4].copy_from_slice(&[mr.get(px), mg.get(px), mb.get(px), a]);
                }
            }
            // Alfa toda en cero: la imagen es opaca (así lo tratan los navegadores).
            if ma.mask != 0 && !has_alpha {
                for px in rgba.as_chunks_mut::<4>().0 {
                    px[3] = 255;
                }
            }
        }
        _ => return Err(invalid("combinación de bpp y compresión sin soporte")),
    }
    if ico && !has_alpha {
        // Máscara AND de 1 bpp a continuación de los píxeles: 1 = transparente.
        let xor_len = if compression == 0 {
            stride * h as usize
        } else {
            data.len()
        };
        let and_stride = (w as usize).div_ceil(32) * 4;
        if let Some(mask) = data.get(xor_len..) {
            for r in 0..h {
                let y = row_of(r);
                for x in 0..w {
                    let Some(&byte) = mask.get(r as usize * and_stride + x as usize / 8) else {
                        break;
                    };
                    if byte & (0x80 >> (x % 8)) != 0 {
                        let o = ((y * w + x) * 4) as usize;
                        rgba[o..o + 4].fill(0);
                    }
                }
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

/// `BI_RLE8` / `BI_RLE4`. Los píxeles que el flujo no toca quedan transparentes.
fn rle(
    data: &[u8],
    bpp: u16,
    w: u32,
    h: u32,
    palette: &[[u8; 4]],
    top_down: bool,
    rgba: &mut [u8],
) -> Result<(), ImageError> {
    let (mut x, mut r) = (0u32, 0u32);
    let mut i = 0;
    let put = |x: u32, r: u32, idx: u8, rgba: &mut [u8]| {
        if x < w && r < h {
            let y = if top_down { r } else { h - 1 - r };
            let o = ((y * w + x) * 4) as usize;
            let c = palette.get(idx as usize).copied().unwrap_or([0, 0, 0, 255]);
            rgba[o..o + 4].copy_from_slice(&c);
        }
    };
    while i + 1 < data.len() && r < h {
        let (n, v) = (data[i], data[i + 1]);
        i += 2;
        if n > 0 {
            for k in 0..n {
                let idx = if bpp == 8 {
                    v
                } else if k % 2 == 0 {
                    v >> 4
                } else {
                    v & 0x0F
                };
                put(x, r, idx, rgba);
                x += 1;
            }
            continue;
        }
        match v {
            0 => {
                x = 0;
                r += 1;
            }
            1 => break,
            2 => {
                let (dx, dy) = (
                    *data.get(i).unwrap_or(&0) as u32,
                    *data.get(i + 1).unwrap_or(&0) as u32,
                );
                i += 2;
                x += dx;
                r += dy;
            }
            count => {
                // Tramo absoluto, alineado a 16 bits.
                let bytes = if bpp == 8 {
                    count as usize
                } else {
                    (count as usize).div_ceil(2)
                };
                let run = data
                    .get(i..i + bytes)
                    .ok_or_else(|| invalid("RLE truncado"))?;
                for k in 0..count as usize {
                    let idx = if bpp == 8 {
                        run[k]
                    } else if k % 2 == 0 {
                        run[k / 2] >> 4
                    } else {
                        run[k / 2] & 0x0F
                    };
                    put(x, r, idx, rgba);
                    x += 1;
                }
                i += bytes + bytes % 2;
            }
        }
    }
    Ok(())
}
