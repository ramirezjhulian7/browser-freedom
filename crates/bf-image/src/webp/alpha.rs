//! Canal `ALPH` de las imágenes con pérdida (WebP Container Specification, "Alpha"): sin
//! compresión o comprimido con VP8L (solo el canal verde, sin encabezado de tamaño), con los
//! filtros predictivos horizontal, vertical y de gradiente. El preprocesado (reducción de
//! niveles) es solo una pista y se ignora.

use super::vp8l;
use crate::{ImageError, invalid};

/// Decodifica el canal alfa de `width × height` a partir del contenido del chunk `ALPH`.
pub(super) fn decode(data: &[u8], width: usize, height: usize) -> Result<Vec<u8>, ImageError> {
    let (&header, rest) = data.split_first().ok_or_else(|| invalid("ALPH vacío"))?;
    let compression = header & 3;
    let filter = (header >> 2) & 3;
    let preprocessing = (header >> 4) & 3;
    if header >> 6 != 0 || preprocessing > 1 {
        return Err(invalid("encabezado ALPH inválido"));
    }
    let n = width * height;
    let mut alpha = match compression {
        0 => rest
            .get(..n)
            .ok_or_else(|| invalid("ALPH truncado"))?
            .to_vec(),
        1 => vp8l::decode_headerless(rest, width, height)?
            .iter()
            .map(|&p| (p >> 8) as u8)
            .collect(),
        _ => return Err(invalid("compresión de ALPH desconocida")),
    };
    unfilter(&mut alpha, width, filter);
    Ok(alpha)
}

/// Deshace el filtro predictivo: cada valor es la suma (módulo 256) del residuo y la
/// predicción. El píxel (0, 0) se predice con 0, la primera fila con el de la izquierda y,
/// para los filtros horizontal y de gradiente, la primera columna con el de arriba.
fn unfilter(alpha: &mut [u8], width: usize, filter: u8) {
    if filter == 0 || width == 0 {
        return;
    }
    let mut prev: Option<Vec<u8>> = None;
    for row in alpha.chunks_exact_mut(width) {
        match prev.as_deref() {
            None => {
                let mut pred = 0u8;
                for v in row.iter_mut() {
                    *v = v.wrapping_add(pred);
                    pred = *v;
                }
            }
            Some(top) => match filter {
                1 => {
                    let mut pred = top[0];
                    for v in row.iter_mut() {
                        *v = v.wrapping_add(pred);
                        pred = *v;
                    }
                }
                2 => {
                    for (v, &t) in row.iter_mut().zip(top) {
                        *v = v.wrapping_add(t);
                    }
                }
                _ => {
                    let (mut left, mut top_left) = (top[0], top[0]);
                    for (v, &t) in row.iter_mut().zip(top) {
                        let g =
                            (i32::from(left) + i32::from(t) - i32::from(top_left)).clamp(0, 255);
                        left = v.wrapping_add(g as u8);
                        *v = left;
                        top_left = t;
                    }
                }
            },
        }
        match &mut prev {
            Some(p) => p.copy_from_slice(row),
            None => prev = Some(row.to_vec()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters() {
        // 3×2, residuos con filtro de gradiente.
        let mut a = [10, 1, 1, 5, 0, 2];
        unfilter(&mut a, 3, 3);
        // Fila 0: 10, 11, 12. Fila 1: 15; luego 15+11-10=16 → 16; 16+12-11=17 → 19.
        assert_eq!(a, [10, 11, 12, 15, 16, 19]);
        let mut a = [10, 1, 1, 5, 0, 2];
        unfilter(&mut a, 3, 2);
        assert_eq!(a, [10, 11, 12, 15, 11, 14]);
        let mut a = [10, 1, 1, 5, 0, 2];
        unfilter(&mut a, 3, 1);
        assert_eq!(a, [10, 11, 12, 15, 15, 17]);
    }
}
