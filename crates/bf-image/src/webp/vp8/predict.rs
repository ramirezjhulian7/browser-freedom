//! Predicción intra (RFC 6386 §12): 16×16 y croma 8×8 (DC/V/H/TM) y los diez modos de
//! subbloque 4×4. Trabajan sobre un búfer con paso [`BPS`]: la fila de arriba del bloque está
//! en `off - BPS` y la columna izquierda en `off - 1`, ya rellenas con los bordes (127 arriba
//! y 129 a la izquierda fuera del cuadro).

pub(super) const BPS: usize = 32;

/// Modos de 16×16 y de croma.
pub(super) const DC_PRED: u8 = 0;
pub(super) const TM_PRED: u8 = 1;
pub(super) const V_PRED: u8 = 2;
pub(super) const H_PRED: u8 = 3;

/// Modos de subbloque 4×4 (la numeración de `intra_bmode` de §11.2).
pub(super) const B_DC_PRED: u8 = 0;
pub(super) const B_TM_PRED: u8 = 1;
pub(super) const B_VE_PRED: u8 = 2;
pub(super) const B_HE_PRED: u8 = 3;
pub(super) const B_LD_PRED: u8 = 4;
pub(super) const B_RD_PRED: u8 = 5;
pub(super) const B_VR_PRED: u8 = 6;
pub(super) const B_VL_PRED: u8 = 7;
pub(super) const B_HD_PRED: u8 = 8;
pub(super) const B_HU_PRED: u8 = 9;

fn clip(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

fn avg2(a: u8, b: u8) -> u8 {
    ((u16::from(a) + u16::from(b) + 1) >> 1) as u8
}

fn avg3(a: u8, b: u8, c: u8) -> u8 {
    ((u16::from(a) + 2 * u16::from(b) + u16::from(c) + 2) >> 2) as u8
}

fn fill(buf: &mut [u8], off: usize, size: usize, v: u8) {
    for y in 0..size {
        buf[off + y * BPS..off + y * BPS + size].fill(v);
    }
}

/// Predicción de un bloque de `size` × `size` (16 para luma, 8 para croma). `has_top` y
/// `has_left` indican si el macrobloque tiene vecinos dentro del cuadro (solo afecta a DC).
pub(super) fn predict_block(
    buf: &mut [u8],
    off: usize,
    size: usize,
    mode: u8,
    has_top: bool,
    has_left: bool,
) {
    match mode {
        V_PRED => {
            for y in 0..size {
                buf.copy_within(off - BPS..off - BPS + size, off + y * BPS);
            }
        }
        H_PRED => {
            for y in 0..size {
                let l = buf[off + y * BPS - 1];
                buf[off + y * BPS..off + y * BPS + size].fill(l);
            }
        }
        TM_PRED => true_motion(buf, off, size),
        _ => {
            let shift = size.trailing_zeros();
            let top: u32 = buf[off - BPS..off - BPS + size]
                .iter()
                .map(|&v| u32::from(v))
                .sum();
            let left: u32 = (0..size).map(|y| u32::from(buf[off + y * BPS - 1])).sum();
            let dc = match (has_top, has_left) {
                (true, true) => (top + left + size as u32) >> (shift + 1),
                (true, false) => (top + (size as u32 >> 1)) >> shift,
                (false, true) => (left + (size as u32 >> 1)) >> shift,
                (false, false) => 128,
            };
            fill(buf, off, size, dc as u8);
        }
    }
}

fn true_motion(buf: &mut [u8], off: usize, size: usize) {
    let tl = i32::from(buf[off - BPS - 1]);
    for y in 0..size {
        let l = i32::from(buf[off + y * BPS - 1]) - tl;
        for x in 0..size {
            let t = i32::from(buf[off - BPS + x]);
            buf[off + y * BPS + x] = clip(t + l);
        }
    }
}

/// Predicción de un subbloque 4×4. Arriba se leen 8 píxeles (los 4 de arriba a la derecha
/// incluidos).
pub(super) fn predict_subblock(buf: &mut [u8], off: usize, mode: u8) {
    let top = off - BPS;
    let a = |i: usize| buf[top + i];
    // A..H: fila de arriba; X: esquina; I..L: columna izquierda.
    let (ta, tb, tc, td, te, tf, tg, th) = (a(0), a(1), a(2), a(3), a(4), a(5), a(6), a(7));
    let tx = buf[top - 1];
    let (li, lj, lk, ll) = (
        buf[off - 1],
        buf[off + BPS - 1],
        buf[off + 2 * BPS - 1],
        buf[off + 3 * BPS - 1],
    );
    let mut d = [[0u8; 4]; 4]; // d[y][x]
    match mode {
        B_DC_PRED => {
            let s: u32 = [ta, tb, tc, td, li, lj, lk, ll]
                .iter()
                .map(|&v| u32::from(v))
                .sum();
            d = [[((s + 4) >> 3) as u8; 4]; 4];
        }
        B_TM_PRED => {
            true_motion(buf, off, 4);
            return;
        }
        B_VE_PRED => {
            let row = [
                avg3(tx, ta, tb),
                avg3(ta, tb, tc),
                avg3(tb, tc, td),
                avg3(tc, td, te),
            ];
            d = [row; 4];
        }
        B_HE_PRED => {
            let rows = [
                avg3(tx, li, lj),
                avg3(li, lj, lk),
                avg3(lj, lk, ll),
                avg3(lk, ll, ll),
            ];
            for (y, &v) in rows.iter().enumerate() {
                d[y] = [v; 4];
            }
        }
        B_LD_PRED => {
            let e = [
                avg3(ta, tb, tc),
                avg3(tb, tc, td),
                avg3(tc, td, te),
                avg3(td, te, tf),
                avg3(te, tf, tg),
                avg3(tf, tg, th),
                avg3(tg, th, th),
            ];
            for (y, row) in d.iter_mut().enumerate() {
                for (x, v) in row.iter_mut().enumerate() {
                    *v = e[x + y];
                }
            }
        }
        B_RD_PRED => {
            // Borde de abajo a la izquierda hacia arriba a la derecha.
            let edge = [ll, lk, lj, li, tx, ta, tb, tc, td];
            for (y, row) in d.iter_mut().enumerate() {
                for (x, v) in row.iter_mut().enumerate() {
                    let c = 4 - y + x;
                    *v = avg3(edge[c - 1], edge[c], edge[c + 1]);
                }
            }
        }
        B_VR_PRED => {
            d[0][0] = avg2(tx, ta);
            d[2][1] = d[0][0];
            d[0][1] = avg2(ta, tb);
            d[2][2] = d[0][1];
            d[0][2] = avg2(tb, tc);
            d[2][3] = d[0][2];
            d[0][3] = avg2(tc, td);
            d[3][0] = avg3(lk, lj, li);
            d[2][0] = avg3(lj, li, tx);
            d[1][0] = avg3(li, tx, ta);
            d[3][1] = d[1][0];
            d[1][1] = avg3(tx, ta, tb);
            d[3][2] = d[1][1];
            d[1][2] = avg3(ta, tb, tc);
            d[3][3] = d[1][2];
            d[1][3] = avg3(tb, tc, td);
        }
        B_VL_PRED => {
            d[0][0] = avg2(ta, tb);
            d[0][1] = avg2(tb, tc);
            d[2][0] = d[0][1];
            d[0][2] = avg2(tc, td);
            d[2][1] = d[0][2];
            d[0][3] = avg2(td, te);
            d[2][2] = d[0][3];
            d[1][0] = avg3(ta, tb, tc);
            d[1][1] = avg3(tb, tc, td);
            d[3][0] = d[1][1];
            d[1][2] = avg3(tc, td, te);
            d[3][1] = d[1][2];
            d[1][3] = avg3(td, te, tf);
            d[3][2] = d[1][3];
            // Los dos últimos no siguen el patrón (§12.3).
            d[2][3] = avg3(te, tf, tg);
            d[3][3] = avg3(tf, tg, th);
        }
        B_HD_PRED => {
            d[0][0] = avg2(li, tx);
            d[1][2] = d[0][0];
            d[1][0] = avg2(lj, li);
            d[2][2] = d[1][0];
            d[2][0] = avg2(lk, lj);
            d[3][2] = d[2][0];
            d[3][0] = avg2(ll, lk);
            d[0][3] = avg3(ta, tb, tc);
            d[0][2] = avg3(tx, ta, tb);
            d[0][1] = avg3(li, tx, ta);
            d[1][3] = d[0][1];
            d[1][1] = avg3(lj, li, tx);
            d[2][3] = d[1][1];
            d[2][1] = avg3(lk, lj, li);
            d[3][3] = d[2][1];
            d[3][1] = avg3(ll, lk, lj);
        }
        _ => {
            // B_HU_PRED
            d[0][0] = avg2(li, lj);
            d[0][2] = avg2(lj, lk);
            d[1][0] = d[0][2];
            d[1][2] = avg2(lk, ll);
            d[2][0] = d[1][2];
            d[0][1] = avg3(li, lj, lk);
            d[0][3] = avg3(lj, lk, ll);
            d[1][1] = d[0][3];
            d[1][3] = avg3(lk, ll, ll);
            d[2][1] = d[1][3];
            d[2][2] = ll;
            d[2][3] = ll;
            d[3] = [ll; 4];
        }
    }
    for (y, row) in d.iter().enumerate() {
        buf[off + y * BPS..off + y * BPS + 4].copy_from_slice(row);
    }
}
