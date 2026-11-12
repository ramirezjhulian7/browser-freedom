//! Transformadas inversas de VP8 (RFC 6386 §14.3 y §14.4).

/// `cos(π/8)·√2 − 1` y `sin(π/8)·√2` en punto fijo de 16 bits (§14.3).
const COS_SQRT2_MINUS1: i64 = 20091;
const SIN_SQRT2: i64 = 35468;

fn mul_cos(a: i32) -> i32 {
    a + ((i64::from(a) * COS_SQRT2_MINUS1) >> 16) as i32
}

fn mul_sin(a: i32) -> i32 {
    ((i64::from(a) * SIN_SQRT2) >> 16) as i32
}

/// DCT inversa de un bloque 4×4 (coeficientes en orden de filas) sumada a la predicción en
/// `dst[off..]` con paso `stride`.
pub(super) fn idct_add(coeffs: &[i32], dst: &mut [u8], off: usize, stride: usize) {
    let mut tmp = [0i32; 16];
    // Pasada vertical.
    for i in 0..4 {
        let (c0, c4, c8, c12) = (coeffs[i], coeffs[4 + i], coeffs[8 + i], coeffs[12 + i]);
        let a = c0 + c8;
        let b = c0 - c8;
        let c = mul_sin(c4) - mul_cos(c12);
        let d = mul_cos(c4) + mul_sin(c12);
        tmp[i] = a + d;
        tmp[12 + i] = a - d;
        tmp[4 + i] = b + c;
        tmp[8 + i] = b - c;
    }
    // Pasada horizontal, con redondeo.
    for y in 0..4 {
        let row = &tmp[y * 4..y * 4 + 4];
        let a = row[0] + row[2];
        let b = row[0] - row[2];
        let c = mul_sin(row[1]) - mul_cos(row[3]);
        let d = mul_cos(row[1]) + mul_sin(row[3]);
        let out = [a + d, b + c, b - c, a - d];
        let p = off + y * stride;
        for (x, v) in out.iter().enumerate() {
            let px = &mut dst[p + x];
            *px = (i32::from(*px) + ((v + 4) >> 3)).clamp(0, 255) as u8;
        }
    }
}

/// Walsh-Hadamard inversa (§14.4): de los 16 coeficientes del bloque Y2 a los DC de los 16
/// subbloques de luma, en orden de filas.
pub(super) fn iwht(input: &[i32; 16]) -> [i32; 16] {
    let mut tmp = [0i32; 16];
    for i in 0..4 {
        let a1 = input[i] + input[12 + i];
        let b1 = input[4 + i] + input[8 + i];
        let c1 = input[4 + i] - input[8 + i];
        let d1 = input[i] - input[12 + i];
        tmp[i] = a1 + b1;
        tmp[4 + i] = c1 + d1;
        tmp[8 + i] = a1 - b1;
        tmp[12 + i] = d1 - c1;
    }
    let mut out = [0i32; 16];
    for y in 0..4 {
        let r = &tmp[y * 4..y * 4 + 4];
        let a1 = r[0] + r[3];
        let b1 = r[1] + r[2];
        let c1 = r[1] - r[2];
        let d1 = r[0] - r[3];
        out[y * 4] = (a1 + b1 + 3) >> 3;
        out[y * 4 + 1] = (c1 + d1 + 3) >> 3;
        out[y * 4 + 2] = (a1 - b1 + 3) >> 3;
        out[y * 4 + 3] = (d1 - c1 + 3) >> 3;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_only() {
        let mut c = [0i32; 16];
        c[0] = 100;
        let mut dst = [10u8; 16];
        idct_add(&c, &mut dst, 0, 4);
        assert!(dst.iter().all(|&v| v == 10 + ((100 + 4) >> 3)));
        let mut y2 = [0i32; 16];
        y2[0] = 80;
        assert!(iwht(&y2).iter().all(|&v| v == (80 + 3) >> 3));
    }
}
