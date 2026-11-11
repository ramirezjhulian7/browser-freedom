//! Filtro de bucle de VP8 (RFC 6386 §15): el simple (§15.2) y el normal (§15.3), sobre los
//! bordes de macrobloque y de subbloque. Cada función filtra a lo largo de un borde: `pos`
//! es el primer píxel después del borde (`q0`), `step` cruza el borde y `along` lo recorre.

fn clamp128(v: i32) -> i32 {
    v.clamp(-128, 127)
}

fn u2s(v: u8) -> i32 {
    i32::from(v) - 128
}

fn s2u(v: i32) -> u8 {
    (clamp128(v) + 128) as u8
}

/// `common_adjust` (§15.2): ajusta `p0` y `q0`; devuelve el ajuste de `q0`.
fn common_adjust(buf: &mut [u8], pos: usize, step: usize, use_outer_taps: bool) -> i32 {
    let p1 = u2s(buf[pos - 2 * step]);
    let p0 = u2s(buf[pos - step]);
    let q0 = u2s(buf[pos]);
    let q1 = u2s(buf[pos + step]);
    let outer = if use_outer_taps { clamp128(p1 - q1) } else { 0 };
    let a = clamp128(outer + 3 * (q0 - p0));
    let b = clamp128(a + 3) >> 3;
    let a = clamp128(a + 4) >> 3;
    buf[pos] = s2u(q0 - a);
    buf[pos - step] = s2u(p0 + b);
    a
}

/// ¿Supera el borde el umbral? (`simple_segment`, en la forma `2·|p0−q0| + |p1−q1|/2`.)
fn edge_ok(buf: &[u8], pos: usize, step: usize, limit: i32) -> bool {
    let p1 = i32::from(buf[pos - 2 * step]);
    let p0 = i32::from(buf[pos - step]);
    let q0 = i32::from(buf[pos]);
    let q1 = i32::from(buf[pos + step]);
    (p0 - q0).abs() * 2 + ((p1 - q1).abs() >> 1) <= limit
}

/// `filter_yes` (§15.3): el umbral del borde y los del interior.
fn normal_ok(buf: &[u8], pos: usize, step: usize, edge: i32, interior: i32) -> bool {
    if !edge_ok(buf, pos, step, edge) {
        return false;
    }
    let px = |k: isize| i32::from(buf[(pos as isize + k * step as isize) as usize]);
    let (p3, p2, p1, p0) = (px(-4), px(-3), px(-2), px(-1));
    let (q0, q1, q2, q3) = (px(0), px(1), px(2), px(3));
    (p3 - p2).abs() <= interior
        && (p2 - p1).abs() <= interior
        && (p1 - p0).abs() <= interior
        && (q1 - q0).abs() <= interior
        && (q2 - q1).abs() <= interior
        && (q3 - q2).abs() <= interior
}

/// `hev`: ¿varianza alta junto al borde?
fn high_edge_variance(buf: &[u8], pos: usize, step: usize, thresh: i32) -> bool {
    let p1 = i32::from(buf[pos - 2 * step]);
    let p0 = i32::from(buf[pos - step]);
    let q0 = i32::from(buf[pos]);
    let q1 = i32::from(buf[pos + step]);
    (p1 - p0).abs() > thresh || (q1 - q0).abs() > thresh
}

/// Filtro simple de un borde de `len` píxeles (§15.2).
pub(super) fn simple(
    buf: &mut [u8],
    pos: usize,
    step: usize,
    along: usize,
    len: usize,
    limit: i32,
) {
    for i in 0..len {
        let p = pos + i * along;
        if edge_ok(buf, p, step, limit) {
            common_adjust(buf, p, step, true);
        }
    }
}

/// Parámetros del filtro normal de un borde.
#[derive(Clone, Copy)]
pub(super) struct Params {
    pub edge: i32,
    pub interior: i32,
    pub hev: i32,
}

/// Filtro normal de un borde de macrobloque (`MBfilter`, §15.3).
pub(super) fn macroblock_edge(
    buf: &mut [u8],
    pos: usize,
    step: usize,
    along: usize,
    len: usize,
    f: Params,
) {
    for i in 0..len {
        let p = pos + i * along;
        if !normal_ok(buf, p, step, f.edge, f.interior) {
            continue;
        }
        if high_edge_variance(buf, p, step, f.hev) {
            common_adjust(buf, p, step, true);
            continue;
        }
        let p2 = u2s(buf[p - 3 * step]);
        let p1 = u2s(buf[p - 2 * step]);
        let p0 = u2s(buf[p - step]);
        let q0 = u2s(buf[p]);
        let q1 = u2s(buf[p + step]);
        let q2 = u2s(buf[p + 2 * step]);
        let w = clamp128(clamp128(p1 - q1) + 3 * (q0 - p0));
        let a = clamp128((27 * w + 63) >> 7);
        buf[p] = s2u(q0 - a);
        buf[p - step] = s2u(p0 + a);
        let a = clamp128((18 * w + 63) >> 7);
        buf[p + step] = s2u(q1 - a);
        buf[p - 2 * step] = s2u(p1 + a);
        let a = clamp128((9 * w + 63) >> 7);
        buf[p + 2 * step] = s2u(q2 - a);
        buf[p - 3 * step] = s2u(p2 + a);
    }
}

/// Filtro normal de un borde de subbloque (`subblock_filter`, §15.3).
pub(super) fn subblock_edge(
    buf: &mut [u8],
    pos: usize,
    step: usize,
    along: usize,
    len: usize,
    f: Params,
) {
    for i in 0..len {
        let p = pos + i * along;
        if !normal_ok(buf, p, step, f.edge, f.interior) {
            continue;
        }
        let hev = high_edge_variance(buf, p, step, f.hev);
        let p1 = u2s(buf[p - 2 * step]);
        let q1 = u2s(buf[p + step]);
        let a = (common_adjust(buf, p, step, hev) + 1) >> 1;
        if !hev {
            buf[p + step] = s2u(q1 - a);
            buf[p - 2 * step] = s2u(p1 + a);
        }
    }
}
