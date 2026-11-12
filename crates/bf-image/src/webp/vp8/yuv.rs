//! VP8 → RGBA como los decodificadores de referencia (libwebp): upsampling "fancy" del croma
//! 4:2:0 (interpolación 9-3-3-1 entre las cuatro muestras de croma vecinas) y conversión
//! YUV → RGB BT.601 de rango limitado en punto fijo de 14 bits.

use super::Frame;

fn mult_hi(v: i32, coeff: i32) -> i32 {
    (v * coeff) >> 8
}

/// Recorta un valor con 6 bits de fracción a 0..=255.
fn clip8(v: i32) -> u8 {
    const MASK: i32 = (256 << 6) - 1;
    if v & !MASK == 0 {
        (v >> 6) as u8
    } else if v < 0 {
        0
    } else {
        255
    }
}

fn yuv_to_rgb(y: u8, u: u8, v: u8) -> [u8; 3] {
    let (y, u, v) = (i32::from(y), i32::from(u), i32::from(v));
    let luma = mult_hi(y, 19077);
    [
        clip8(luma + mult_hi(v, 26149) - 14234),
        clip8(luma - mult_hi(u, 6419) - mult_hi(v, 13320) + 8708),
        clip8(luma + mult_hi(u, 33050) - 17685),
    ]
}

/// Croma a resolución completa de las filas de luma entre las filas de croma `t` (arriba) y
/// `c` (abajo): `top` es la fila más cercana a `t` y `bottom` la más cercana a `c`.
fn upsample_pair(t: &[u8], c: &[u8], top: &mut [u8], bottom: &mut [u8]) {
    let len = top.len();
    let (mut tl, mut l) = (u32::from(t[0]), u32::from(c[0]));
    top[0] = ((3 * tl + l + 2) >> 2) as u8;
    bottom[0] = ((3 * l + tl + 2) >> 2) as u8;
    for x in 1..=(len - 1) >> 1 {
        let (tt, cc) = (u32::from(t[x]), u32::from(c[x]));
        let avg = tl + tt + l + cc + 8;
        let diag_12 = (avg + 2 * (tt + l)) >> 3;
        let diag_03 = (avg + 2 * (tl + cc)) >> 3;
        top[2 * x - 1] = ((diag_12 + tl) >> 1) as u8;
        top[2 * x] = ((diag_03 + tt) >> 1) as u8;
        bottom[2 * x - 1] = ((diag_03 + l) >> 1) as u8;
        bottom[2 * x] = ((diag_12 + cc) >> 1) as u8;
        tl = tt;
        l = cc;
    }
    if len & 1 == 0 {
        top[len - 1] = ((3 * tl + l + 2) >> 2) as u8;
        bottom[len - 1] = ((3 * l + tl + 2) >> 2) as u8;
    }
}

/// Convierte el cuadro a RGBA (sin premultiplicar); `alpha` es el plano alfa, si lo hay.
pub(super) fn to_rgba(f: &Frame, alpha: Option<&[u8]>) -> Vec<u8> {
    let (w, h) = (f.width, f.height);
    let mut out = vec![0u8; w * h * 4];
    let mut bufs = vec![0u8; w * 4];
    let (tu, rest) = bufs.split_at_mut(w);
    let (tv, rest) = rest.split_at_mut(w);
    let (bu, bv) = rest.split_at_mut(w);
    let uv_row = |k: usize| k * f.uv_stride..k * f.uv_stride + w.div_ceil(2);
    let mut emit = |row: usize, u: &[u8], v: &[u8]| {
        let y = &f.y[row * f.y_stride..row * f.y_stride + w];
        let dst = &mut out[row * w * 4..(row + 1) * w * 4];
        for (x, px) in dst.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let [r, g, b] = yuv_to_rgb(y[x], u[x], v[x]);
            px[0] = r;
            px[1] = g;
            px[2] = b;
            px[3] = alpha.map_or(255, |a| a[row * w + x]);
        }
    };
    // Primera fila: la croma se refleja en el borde.
    let r0 = uv_row(0);
    upsample_pair(&f.u[r0.clone()], &f.u[r0.clone()], tu, bu);
    upsample_pair(&f.v[r0.clone()], &f.v[r0], tv, bv);
    emit(0, tu, tv);
    for k in 1..=h / 2 {
        let (top, bottom) = (2 * k - 1, 2 * k);
        let rt = uv_row(k - 1);
        let rc = if bottom < h { uv_row(k) } else { rt.clone() };
        upsample_pair(&f.u[rt.clone()], &f.u[rc.clone()], tu, bu);
        upsample_pair(&f.v[rt], &f.v[rc], tv, bv);
        emit(top, tu, tv);
        if bottom < h {
            emit(bottom, bu, bv);
        }
    }
    out
}
