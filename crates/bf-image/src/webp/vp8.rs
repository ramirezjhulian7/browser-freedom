//! VP8: el formato con pérdida (RFC 6386). WebP solo usa cuadros clave, así que no hay
//! predicción inter: encabezado de cuadro (§9, §19.2), modos intra por macrobloque (§11,
//! §19.3), tokens de coeficientes (§13), cuantización (§14.1), transformadas inversas (§14),
//! predicción intra (§12) y filtro de bucle (§15).

mod bool_decoder;
mod idct;
mod loop_filter;
mod predict;
mod tables;
mod yuv;

use crate::{ImageError, check_size, invalid};
use bool_decoder::BoolDecoder;
use predict::{
    B_DC_PRED, B_HD_PRED, B_HE_PRED, B_HU_PRED, B_LD_PRED, B_RD_PRED, B_TM_PRED, B_VE_PRED,
    B_VL_PRED, B_VR_PRED, BPS, DC_PRED, H_PRED, TM_PRED, V_PRED,
};
use tables::{
    AC_QUANT, BANDS, CAT3456, COEFF_UPDATE_PROBS, DC_QUANT, DEFAULT_COEFF_PROBS, KF_BMODE_PROBS,
    ZIGZAG,
};

/// Un cuadro decodificado en YUV 4:2:0, con planos rellenos hasta múltiplos de 16.
pub(super) struct Frame {
    pub width: usize,
    pub height: usize,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    pub y_stride: usize,
    pub uv_stride: usize,
}

impl Frame {
    /// RGBA sin premultiplicar, con el plano alfa opcional.
    pub(super) fn to_rgba(&self, alpha: Option<&[u8]>) -> Vec<u8> {
        yuv::to_rgba(self, alpha)
    }
}

type Probs = [[[[u8; 11]; 3]; 8]; 4];

/// Pasos de cuantización `[DC, AC]` de cada tipo de bloque.
#[derive(Clone, Copy, Default)]
struct Quant {
    y1: [i32; 2],
    y2: [i32; 2],
    uv: [i32; 2],
}

/// Parámetros del filtro de bucle de un macrobloque.
#[derive(Clone, Copy, Default)]
struct MbFilter {
    level: i32,
    interior: i32,
    hev: i32,
    inner: bool,
}

struct Header {
    update_map: bool,
    segment_probs: [u8; 3],
    quant: [Quant; 4],
    /// Filtro por segmento y por "es B_PRED".
    filters: [[MbFilter; 2]; 4],
    filter_level: i32,
    simple_filter: bool,
    num_partitions: usize,
    probs: Box<Probs>,
    skip_prob: Option<u8>,
    /// Posición de algunos bits entre los leídos, para recodificar el encabezado en las
    /// pruebas: `segment_feature_mode`, `filter_type` y `loop_filter_adj_enable`.
    #[cfg(test)]
    marks: [usize; 3],
}

/// Decodifica el contenido de un chunk `VP8 ` (cuadro clave, §9.1).
pub(super) fn decode(data: &[u8]) -> Result<Frame, ImageError> {
    decode_parts(data).map(|(frame, ..)| frame)
}

/// Como [`decode`], pero da también la partición 0 ya leída y el encabezado.
fn decode_parts(data: &[u8]) -> Result<(Frame, BoolDecoder<'_>, Header), ImageError> {
    let head = data.get(..10).ok_or_else(|| invalid("VP8 truncado"))?;
    let tag = u32::from(head[0]) | (u32::from(head[1]) << 8) | (u32::from(head[2]) << 16);
    let key_frame = tag & 1 == 0;
    let version = (tag >> 1) & 7;
    let show = (tag >> 4) & 1;
    let part0_size = (tag >> 5) as usize;
    if !key_frame {
        return Err(invalid("VP8 sin cuadro clave"));
    }
    if version > 3 {
        return Err(invalid("versión de VP8 desconocida"));
    }
    if show == 0 {
        return Err(invalid("cuadro VP8 no visible"));
    }
    if head[3..6] != [0x9d, 0x01, 0x2a] {
        return Err(invalid("código de inicio VP8 inválido"));
    }
    // El escalado (2 bits altos) es solo una pista para el que muestra la imagen.
    let width = u32::from(u16::from_le_bytes([head[6], head[7]]) & 0x3fff);
    let height = u32::from(u16::from_le_bytes([head[8], head[9]]) & 0x3fff);
    check_size(width, height)?;
    let rest = &data[10..];
    let part0 = rest
        .get(..part0_size)
        .ok_or_else(|| invalid("partición VP8 truncada"))?;
    let mut br = BoolDecoder::new(part0);
    let hdr = parse_header(&mut br)?;
    if br.eof() {
        return Err(invalid("encabezado VP8 truncado"));
    }

    // Particiones de tokens (§9.5).
    let tokens = &rest[part0_size..];
    let n = hdr.num_partitions;
    let sizes_len = 3 * (n - 1);
    if tokens.len() < sizes_len {
        return Err(invalid("particiones VP8 truncadas"));
    }
    let mut parts = Vec::with_capacity(n);
    let mut start = sizes_len;
    for i in 0..n - 1 {
        let s = &tokens[3 * i..3 * i + 3];
        let size = usize::from(s[0]) | (usize::from(s[1]) << 8) | (usize::from(s[2]) << 16);
        let size = size.min(tokens.len() - start);
        parts.push(BoolDecoder::new(&tokens[start..start + size]));
        start += size;
    }
    if start >= tokens.len() {
        return Err(invalid("partición VP8 vacía"));
    }
    parts.push(BoolDecoder::new(&tokens[start..]));

    let frame = decode_macroblocks(&mut br, &mut parts, &hdr, width as usize, height as usize)?;
    Ok((frame, br, hdr))
}

/// Encabezado del cuadro (§9.3–§9.11, §19.2).
fn parse_header(br: &mut BoolDecoder) -> Result<Header, ImageError> {
    let _color_space = br.flag();
    let _clamping_type = br.flag();

    // Segmentación (§9.3). En un cuadro clave los datos parten de cero en modo delta.
    let segmentation = br.flag();
    #[cfg(test)]
    let mut marks = [usize::MAX; 3];
    let mut update_map = false;
    let mut absolute = false;
    let mut seg_quant = [0i32; 4];
    let mut seg_lf = [0i32; 4];
    let mut segment_probs = [255u8; 3];
    if segmentation {
        update_map = br.flag();
        if br.flag() {
            #[cfg(test)]
            {
                marks[0] = br.log.len();
            }
            absolute = br.flag();
            for q in &mut seg_quant {
                *q = if br.flag() { br.signed(7) } else { 0 };
            }
            for l in &mut seg_lf {
                *l = if br.flag() { br.signed(6) } else { 0 };
            }
        }
        if update_map {
            for p in &mut segment_probs {
                *p = if br.flag() { br.literal(8) as u8 } else { 255 };
            }
        }
    }

    // Filtro de bucle (§9.6) y sus ajustes por referencia y modo (§9.7).
    #[cfg(test)]
    {
        marks[1] = br.log.len();
    }
    let simple_filter = br.flag();
    let filter_level = br.literal(6) as i32;
    let sharpness = br.literal(3) as i32;
    let mut ref_lf_delta = [0i32; 4];
    let mut mode_lf_delta = [0i32; 4];
    #[cfg(test)]
    {
        marks[2] = br.log.len();
    }
    let lf_adjust = br.flag();
    if lf_adjust && br.flag() {
        for d in &mut ref_lf_delta {
            if br.flag() {
                *d = br.signed(6);
            }
        }
        for d in &mut mode_lf_delta {
            if br.flag() {
                *d = br.signed(6);
            }
        }
    }

    let num_partitions = 1 << br.literal(2);

    // Cuantización (§9.6, §14.1).
    let base_q = br.literal(7) as i32;
    let mut delta = || if br.flag() { br.signed(4) } else { 0 };
    let (y1_dc, y2_dc, y2_ac, uv_dc, uv_ac) = (delta(), delta(), delta(), delta(), delta());
    let mut quant = [Quant::default(); 4];
    for (s, q) in quant.iter_mut().enumerate() {
        let base = if !segmentation {
            base_q
        } else if absolute {
            seg_quant[s]
        } else {
            base_q + seg_quant[s]
        }
        .clamp(0, 127);
        let dc = |d: i32| i32::from(DC_QUANT[(base + d).clamp(0, 127) as usize]);
        let ac = |d: i32| i32::from(AC_QUANT[(base + d).clamp(0, 127) as usize]);
        *q = Quant {
            y1: [dc(y1_dc), ac(0)],
            y2: [dc(y2_dc) * 2, (ac(y2_ac) * 155 / 100).max(8)],
            uv: [dc(uv_dc).min(132), ac(uv_ac)],
        };
    }

    // Niveles de filtro por segmento y modo (§9.6, §15.1).
    let mut filters = [[MbFilter::default(); 2]; 4];
    for (s, f) in filters.iter_mut().enumerate() {
        let mut base = filter_level;
        if segmentation {
            base = if absolute {
                seg_lf[s]
            } else {
                base + seg_lf[s]
            };
            base = base.clamp(0, 63);
        }
        for (i4x4, mf) in f.iter_mut().enumerate() {
            let mut level = base;
            if lf_adjust {
                // Cuadro intra: referencia 0; el ajuste de modo 0 es el de B_PRED.
                level += ref_lf_delta[0];
                if i4x4 == 1 {
                    level += mode_lf_delta[0];
                }
                level = level.clamp(0, 63);
            }
            let mut interior = level;
            if sharpness > 0 {
                interior >>= if sharpness > 4 { 2 } else { 1 };
                interior = interior.min(9 - sharpness);
            }
            *mf = MbFilter {
                level,
                interior: interior.max(1),
                hev: if level >= 40 {
                    2
                } else if level >= 15 {
                    1
                } else {
                    0
                },
                inner: i4x4 == 1,
            };
        }
    }

    let _refresh_entropy_probs = br.flag();

    // Actualización de las probabilidades de los tokens (§13.4).
    let mut probs = Box::new(DEFAULT_COEFF_PROBS);
    for (t, bands) in probs.iter_mut().enumerate() {
        for (b, ctxs) in bands.iter_mut().enumerate() {
            for (c, nodes) in ctxs.iter_mut().enumerate() {
                for (p, prob) in nodes.iter_mut().enumerate() {
                    if br.get(COEFF_UPDATE_PROBS[t][b][c][p]) {
                        *prob = br.literal(8) as u8;
                    }
                }
            }
        }
    }
    let skip_prob = if br.flag() {
        Some(br.literal(8) as u8)
    } else {
        None
    };

    Ok(Header {
        update_map,
        segment_probs,
        quant,
        filters,
        filter_level,
        simple_filter,
        num_partitions,
        probs,
        skip_prob,
        #[cfg(test)]
        marks,
    })
}

/// `intra_bmode` con el árbol `bmode_tree` (§11.2).
fn read_bmode(br: &mut BoolDecoder, p: &[u8; 9]) -> u8 {
    if !br.get(p[0]) {
        B_DC_PRED
    } else if !br.get(p[1]) {
        B_TM_PRED
    } else if !br.get(p[2]) {
        B_VE_PRED
    } else if !br.get(p[3]) {
        if !br.get(p[4]) {
            B_HE_PRED
        } else if !br.get(p[5]) {
            B_RD_PRED
        } else {
            B_VR_PRED
        }
    } else if !br.get(p[6]) {
        B_LD_PRED
    } else if !br.get(p[7]) {
        B_VL_PRED
    } else if !br.get(p[8]) {
        B_HD_PRED
    } else {
        B_HU_PRED
    }
}

/// Valor de un token grande (`DCT_2` en adelante, §13.2).
fn large_value(br: &mut BoolDecoder, p: &[u8; 11]) -> i32 {
    if !br.get(p[3]) {
        if !br.get(p[4]) {
            2
        } else {
            3 + i32::from(br.get(p[5]))
        }
    } else if !br.get(p[6]) {
        if !br.get(p[7]) {
            5 + i32::from(br.get(159))
        } else {
            7 + 2 * i32::from(br.get(165)) + i32::from(br.get(145))
        }
    } else {
        let bit1 = usize::from(br.get(p[8]));
        let bit0 = usize::from(br.get(p[9 + bit1]));
        let cat = 2 * bit1 + bit0;
        let mut v = 0;
        for &prob in CAT3456[cat] {
            v = 2 * v + i32::from(br.get(prob));
        }
        v + 3 + (8 << cat)
    }
}

/// Tokens de un bloque de 16 coeficientes desde la posición `first` (§13.2–§13.3).
/// Devuelve la posición siguiente al último token leído (`first` si el bloque está vacío).
fn get_coeffs(
    br: &mut BoolDecoder,
    probs: &[[[u8; 11]; 3]; 8],
    ctx: usize,
    dq: [i32; 2],
    first: usize,
    out: &mut [i32],
) -> usize {
    let mut n = first;
    let mut p = &probs[BANDS[n]][ctx];
    while n < 16 {
        if !br.get(p[0]) {
            return n; // fin de bloque
        }
        while !br.get(p[1]) {
            n += 1;
            if n == 16 {
                return 16;
            }
            p = &probs[BANDS[n]][0];
        }
        let (v, next_ctx) = if !br.get(p[2]) {
            (1, 1)
        } else {
            (large_value(br, p), 2)
        };
        let v = if br.get(128) { -v } else { v };
        // Como el decodificador de referencia, el coeficiente se guarda en 16 bits.
        out[ZIGZAG[n]] = (v * dq[usize::from(n > 0)]) as i16 as i32;
        n += 1;
        p = &probs[BANDS[n]][next_ctx];
    }
    16
}

/// Contextos de "bloque con coeficientes" de un lado de los macrobloques: 4 de luma, 2 de U,
/// 2 de V y el de Y2.
type NzCtx = [u8; 9];

/// Lee los coeficientes de un macrobloque (§13). Devuelve si hubo alguno distinto de cero.
fn parse_residuals(
    br: &mut BoolDecoder,
    probs: &Probs,
    q: &Quant,
    is_i4x4: bool,
    top: &mut NzCtx,
    left: &mut NzCtx,
    coeffs: &mut [i32; 384],
) -> bool {
    let mut any = false;
    let (first, y_type) = if is_i4x4 {
        (0, 3)
    } else {
        let mut dc = [0i32; 16];
        let ctx = usize::from(top[8] + left[8]);
        let nz = get_coeffs(br, &probs[1], ctx, q.y2, 0, &mut dc);
        top[8] = u8::from(nz > 0);
        left[8] = top[8];
        any |= nz > 0;
        for (i, v) in idct::iwht(&dc).iter().enumerate() {
            coeffs[i * 16] = *v as i16 as i32;
        }
        (1, 0)
    };
    let (top_y, left_y) = (&mut top[..4], &mut left[..4]);
    for (y, ly) in left_y.iter_mut().enumerate() {
        let mut l = *ly;
        for (x, tx) in top_y.iter_mut().enumerate() {
            let ctx = usize::from(l + *tx);
            let b = (y * 4 + x) * 16;
            let nz = get_coeffs(br, &probs[y_type], ctx, q.y1, first, &mut coeffs[b..b + 16]);
            l = u8::from(nz > first);
            *tx = l;
            any |= nz > first;
        }
        *ly = l;
    }
    for ch in 0..2 {
        for y in 0..2 {
            let mut l = left[4 + ch * 2 + y];
            for x in 0..2 {
                let ctx = usize::from(l + top[4 + ch * 2 + x]);
                let b = 256 + ch * 64 + (y * 2 + x) * 16;
                let nz = get_coeffs(br, &probs[2], ctx, q.uv, 0, &mut coeffs[b..b + 16]);
                l = u8::from(nz > 0);
                top[4 + ch * 2 + x] = l;
                any |= nz > 0;
            }
            left[4 + ch * 2 + y] = l;
        }
    }
    any
}

/// Origen del bloque dentro del búfer de trabajo: una fila y una columna de borde antes.
const WS_OFF: usize = BPS + 1;

/// Decodifica todos los macrobloques y aplica el filtro de bucle.
fn decode_macroblocks(
    br: &mut BoolDecoder,
    parts: &mut [BoolDecoder],
    hdr: &Header,
    width: usize,
    height: usize,
) -> Result<Frame, ImageError> {
    let mb_w = width.div_ceil(16);
    let mb_h = height.div_ceil(16);
    let ys = mb_w * 16;
    let uvs = mb_w * 8;
    let mut yp = vec![0u8; ys * mb_h * 16];
    let mut up = vec![0u8; uvs * mb_h * 8];
    let mut vp = vec![0u8; uvs * mb_h * 8];
    let mut intra_top = vec![B_DC_PRED; mb_w * 4];
    let mut nz_top = vec![NzCtx::default(); mb_w];
    let mut filters = vec![MbFilter::default(); mb_w * mb_h];
    let mut ws = [0u8; BPS * 17];
    let mut ws_u = [0u8; BPS * 9];
    let mut ws_v = [0u8; BPS * 9];
    let mut coeffs = [0i32; 384];
    let num_parts = parts.len();

    for mb_y in 0..mb_h {
        let mut intra_left = [B_DC_PRED; 4];
        let mut nz_left = NzCtx::default();
        let tb = &mut parts[mb_y % num_parts];
        for mb_x in 0..mb_w {
            // Encabezado del macrobloque (§19.3).
            let segment = if hdr.update_map {
                let p = &hdr.segment_probs;
                if !br.get(p[0]) {
                    usize::from(br.get(p[1]))
                } else {
                    2 + usize::from(br.get(p[2]))
                }
            } else {
                0
            };
            let skip = hdr.skip_prob.is_some_and(|p| br.get(p));
            let is_i4x4 = !br.get(145);
            let mut ymode = DC_PRED;
            let mut bmodes = [B_DC_PRED; 16];
            let top_modes = &mut intra_top[mb_x * 4..mb_x * 4 + 4];
            if !is_i4x4 {
                ymode = if br.get(156) {
                    if br.get(128) { TM_PRED } else { H_PRED }
                } else if br.get(163) {
                    V_PRED
                } else {
                    DC_PRED
                };
                // Los contextos de los subbloques usan el modo equivalente (§11.3).
                top_modes.fill(ymode);
                intra_left.fill(ymode);
            } else {
                for y in 0..4 {
                    let mut left = intra_left[y];
                    for x in 0..4 {
                        let probs = &KF_BMODE_PROBS[usize::from(top_modes[x])][usize::from(left)];
                        left = read_bmode(br, probs);
                        bmodes[y * 4 + x] = left;
                        top_modes[x] = left;
                    }
                    intra_left[y] = left;
                }
            }
            let uv_mode = if !br.get(142) {
                DC_PRED
            } else if !br.get(114) {
                V_PRED
            } else if !br.get(183) {
                H_PRED
            } else {
                TM_PRED
            };

            // Coeficientes.
            coeffs.fill(0);
            let has_coeffs = if skip {
                let dc = (nz_top[mb_x][8], nz_left[8]);
                nz_top[mb_x] = NzCtx::default();
                nz_left = NzCtx::default();
                if is_i4x4 {
                    // Sin Y2, su contexto se conserva.
                    (nz_top[mb_x][8], nz_left[8]) = dc;
                }
                false
            } else {
                parse_residuals(
                    tb,
                    &hdr.probs,
                    &hdr.quant[segment],
                    is_i4x4,
                    &mut nz_top[mb_x],
                    &mut nz_left,
                    &mut coeffs,
                )
            };
            if tb.eof() {
                return Err(invalid("partición de tokens VP8 truncada"));
            }

            // Reconstrucción de luma en el búfer de trabajo.
            let px = mb_x * 16;
            let py = mb_y * 16;
            if mb_y == 0 {
                ws[WS_OFF - BPS - 1..WS_OFF - BPS + 20].fill(127);
            } else {
                let above = (py - 1) * ys;
                ws[WS_OFF - BPS - 1] = if mb_x == 0 { 129 } else { yp[above + px - 1] };
                ws[WS_OFF - BPS..WS_OFF - BPS + 16]
                    .copy_from_slice(&yp[above + px..above + px + 16]);
                if mb_x + 1 < mb_w {
                    ws[WS_OFF - BPS + 16..WS_OFF - BPS + 20]
                        .copy_from_slice(&yp[above + px + 16..above + px + 20]);
                } else {
                    let last = yp[above + px + 15];
                    ws[WS_OFF - BPS + 16..WS_OFF - BPS + 20].fill(last);
                }
            }
            for y in 0..16 {
                ws[WS_OFF + y * BPS - 1] = if mb_x == 0 {
                    129
                } else {
                    yp[(py + y) * ys + px - 1]
                };
            }
            // Los subbloques de la columna derecha usan los píxeles de arriba a la derecha
            // del macrobloque (§12.3).
            for r in [3, 7, 11] {
                ws.copy_within(WS_OFF - BPS + 16..WS_OFF - BPS + 20, WS_OFF + r * BPS + 16);
            }
            if is_i4x4 {
                for (sb, &mode) in bmodes.iter().enumerate() {
                    let off = WS_OFF + (sb / 4) * 4 * BPS + (sb % 4) * 4;
                    predict::predict_subblock(&mut ws, off, mode);
                    add_residual(&coeffs[sb * 16..sb * 16 + 16], &mut ws, off);
                }
            } else {
                predict::predict_block(&mut ws, WS_OFF, 16, ymode, mb_y > 0, mb_x > 0);
                for sb in 0..16 {
                    let off = WS_OFF + (sb / 4) * 4 * BPS + (sb % 4) * 4;
                    add_residual(&coeffs[sb * 16..sb * 16 + 16], &mut ws, off);
                }
            }
            for y in 0..16 {
                yp[(py + y) * ys + px..(py + y) * ys + px + 16]
                    .copy_from_slice(&ws[WS_OFF + y * BPS..WS_OFF + y * BPS + 16]);
            }

            // Croma.
            let (cx, cy) = (mb_x * 8, mb_y * 8);
            for (ch, (plane, wsc)) in [(&mut up, &mut ws_u), (&mut vp, &mut ws_v)]
                .into_iter()
                .enumerate()
            {
                if mb_y == 0 {
                    wsc[WS_OFF - BPS - 1..WS_OFF - BPS + 8].fill(127);
                } else {
                    let above = (cy - 1) * uvs;
                    wsc[WS_OFF - BPS - 1] = if mb_x == 0 {
                        129
                    } else {
                        plane[above + cx - 1]
                    };
                    wsc[WS_OFF - BPS..WS_OFF - BPS + 8]
                        .copy_from_slice(&plane[above + cx..above + cx + 8]);
                }
                for y in 0..8 {
                    wsc[WS_OFF + y * BPS - 1] = if mb_x == 0 {
                        129
                    } else {
                        plane[(cy + y) * uvs + cx - 1]
                    };
                }
                predict::predict_block(wsc, WS_OFF, 8, uv_mode, mb_y > 0, mb_x > 0);
                for sb in 0..4 {
                    let off = WS_OFF + (sb / 2) * 4 * BPS + (sb % 2) * 4;
                    let b = 256 + ch * 64 + sb * 16;
                    add_residual(&coeffs[b..b + 16], wsc, off);
                }
                for y in 0..8 {
                    plane[(cy + y) * uvs + cx..(cy + y) * uvs + cx + 8]
                        .copy_from_slice(&wsc[WS_OFF + y * BPS..WS_OFF + y * BPS + 8]);
                }
            }

            let mut f = hdr.filters[segment][usize::from(is_i4x4)];
            f.inner |= has_coeffs;
            filters[mb_y * mb_w + mb_x] = f;
        }
        if br.eof() {
            return Err(invalid("partición 0 de VP8 truncada"));
        }
    }

    if hdr.filter_level > 0 {
        for mb_y in 0..mb_h {
            for mb_x in 0..mb_w {
                filter_macroblock(
                    &mut yp,
                    &mut up,
                    &mut vp,
                    ys,
                    uvs,
                    mb_x,
                    mb_y,
                    &filters[mb_y * mb_w + mb_x],
                    hdr.simple_filter,
                );
            }
        }
    }

    Ok(Frame {
        width,
        height,
        y: yp,
        u: up,
        v: vp,
        y_stride: ys,
        uv_stride: uvs,
    })
}

/// Suma la DCT inversa del bloque a la predicción (nada si no tiene coeficientes).
fn add_residual(coeffs: &[i32], buf: &mut [u8], off: usize) {
    if coeffs.iter().any(|&c| c != 0) {
        idct::idct_add(coeffs, buf, off, BPS);
    }
}

/// Filtro de bucle de un macrobloque (§15): borde izquierdo, bordes verticales internos,
/// borde superior y bordes horizontales internos.
#[allow(clippy::too_many_arguments)]
fn filter_macroblock(
    yp: &mut [u8],
    up: &mut [u8],
    vp: &mut [u8],
    ys: usize,
    uvs: usize,
    mb_x: usize,
    mb_y: usize,
    f: &MbFilter,
    simple: bool,
) {
    if f.level == 0 {
        return;
    }
    let mb_limit = (f.level + 2) * 2 + f.interior;
    let sub_limit = f.level * 2 + f.interior;
    let y0 = mb_y * 16 * ys + mb_x * 16;
    if simple {
        if mb_x > 0 {
            loop_filter::simple(yp, y0, 1, ys, 16, mb_limit);
        }
        if f.inner {
            for k in 1..4 {
                loop_filter::simple(yp, y0 + 4 * k, 1, ys, 16, sub_limit);
            }
        }
        if mb_y > 0 {
            loop_filter::simple(yp, y0, ys, 1, 16, mb_limit);
        }
        if f.inner {
            for k in 1..4 {
                loop_filter::simple(yp, y0 + 4 * k * ys, ys, 1, 16, sub_limit);
            }
        }
        return;
    }
    let mb = loop_filter::Params {
        edge: mb_limit,
        interior: f.interior,
        hev: f.hev,
    };
    let sub = loop_filter::Params {
        edge: sub_limit,
        ..mb
    };
    let c0 = mb_y * 8 * uvs + mb_x * 8;
    if mb_x > 0 {
        loop_filter::macroblock_edge(yp, y0, 1, ys, 16, mb);
        loop_filter::macroblock_edge(up, c0, 1, uvs, 8, mb);
        loop_filter::macroblock_edge(vp, c0, 1, uvs, 8, mb);
    }
    if f.inner {
        for k in 1..4 {
            loop_filter::subblock_edge(yp, y0 + 4 * k, 1, ys, 16, sub);
        }
        loop_filter::subblock_edge(up, c0 + 4, 1, uvs, 8, sub);
        loop_filter::subblock_edge(vp, c0 + 4, 1, uvs, 8, sub);
    }
    if mb_y > 0 {
        loop_filter::macroblock_edge(yp, y0, ys, 1, 16, mb);
        loop_filter::macroblock_edge(up, c0, uvs, 1, 8, mb);
        loop_filter::macroblock_edge(vp, c0, uvs, 1, 8, mb);
    }
    if f.inner {
        for k in 1..4 {
            loop_filter::subblock_edge(yp, y0 + 4 * k * ys, ys, 1, 16, sub);
        }
        loop_filter::subblock_edge(up, c0 + 4 * uvs, uvs, 1, 8, sub);
        loop_filter::subblock_edge(vp, c0 + 4 * uvs, uvs, 1, 8, sub);
    }
}

#[cfg(test)]
mod tests {
    use super::bool_decoder::encoder::Encoder;
    use std::path::PathBuf;

    fn vectors_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/webp")
    }

    /// Recodifica la partición 0 de un vector tras editar sus bits leídos (las particiones
    /// de tokens no cambian) y escribe el resultado como un WebP simple.
    fn reencode(src: &str, dst: &str, edit: impl FnOnce(&mut Vec<(u8, bool)>, &super::Header)) {
        let src = std::fs::read(vectors_dir().join(src)).unwrap();
        let payload = super::super::find_vp8(&src).unwrap();
        let (_, br, hdr) = super::decode_parts(payload).unwrap();
        let mut log = br.log;
        edit(&mut log, &hdr);
        let mut enc = Encoder::new();
        for (prob, bit) in log {
            enc.put(prob, bit);
        }
        let part0 = enc.finish();
        let tag = u32::from_le_bytes([payload[0], payload[1], payload[2], 0]);
        let old_size = (tag >> 5) as usize;
        let tag = (tag & 0x1f) | ((part0.len() as u32) << 5);
        let mut vp8 = tag.to_le_bytes()[..3].to_vec();
        vp8.extend_from_slice(&payload[3..10]);
        vp8.extend_from_slice(&part0);
        vp8.extend_from_slice(&payload[10 + old_size..]);
        let padded = vp8.len() + vp8.len() % 2;
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(12 + padded as u32).to_le_bytes());
        out.extend_from_slice(b"WEBPVP8 ");
        out.extend_from_slice(&(vp8.len() as u32).to_le_bytes());
        out.extend_from_slice(&vp8);
        out.resize(8 + 12 + padded, 0);
        std::fs::write(vectors_dir().join(dst), out).unwrap();
    }

    /// Bits de un campo `L(n)` con signo (prob. 128), como en el encabezado.
    fn signed_bits(v: i32, n: u32) -> Vec<(u8, bool)> {
        let mut bits = vec![(128, true)];
        for i in (0..n).rev() {
            bits.push((128, (v.unsigned_abs() >> i) & 1 == 1));
        }
        bits.push((128, v < 0));
        bits
    }

    /// Genera los vectores derivados que libwebp (vía ffmpeg) no produce; sus referencias
    /// (`.yuv`, `.png`) salen de decodificarlos con ffmpeg. Se ejecuta a mano:
    /// `cargo test -p bf-image generate_derived -- --ignored`.
    ///
    /// - `ly_simple_filter.webp`: `ly_mandel_photo.webp` con el filtro simple (§15.2) en vez
    ///   del normal (bit `filter_type` invertido).
    /// - `ly_deltas.webp`: `ly_grad_q50.webp` con los datos de segmento en modo delta y con
    ///   `mode_ref_lf_delta_update`: `ref_frame` intra +8 y modo B_PRED −3 (§9.3, §9.7).
    #[test]
    #[ignore]
    fn generate_derived_vectors() {
        reencode(
            "ly_mandel_photo.webp",
            "ly_simple_filter.webp",
            |log, hdr| {
                assert!(!hdr.simple_filter && hdr.filter_level > 0);
                log[hdr.marks[1]].1 = true;
            },
        );
        reencode("ly_grad_q50.webp", "ly_deltas.webp", |log, hdr| {
            assert!(log[hdr.marks[0]].1 && !log[hdr.marks[2]].1);
            log[hdr.marks[0]].1 = false;
            let mut adj = vec![(128, true), (128, true)];
            adj.extend(signed_bits(8, 6));
            adj.extend([(128, false); 3]);
            adj.extend(signed_bits(-3, 6));
            adj.extend([(128, false); 3]);
            log.splice(hdr.marks[2]..=hdr.marks[2], adj);
        });
    }

    /// El VP8 es exacto: los planos Y, U y V coinciden con los de ffmpeg (`.yuv`, 4:2:0 crudo).
    #[test]
    fn planes_match_reference() {
        let dir = vectors_dir();
        let mut checked = 0;
        let mut failed = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "yuv") {
                continue;
            }
            let webp = std::fs::read(path.with_extension("webp")).unwrap();
            let reference = std::fs::read(&path).unwrap();
            let payload = super::super::find_vp8(&webp).unwrap();
            let f = super::decode(payload).unwrap();
            let (w, h) = (f.width, f.height);
            let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
            let mut ours = Vec::new();
            for y in 0..h {
                ours.extend_from_slice(&f.y[y * f.y_stride..y * f.y_stride + w]);
            }
            for plane in [&f.u, &f.v] {
                for y in 0..ch {
                    ours.extend_from_slice(&plane[y * f.uv_stride..y * f.uv_stride + cw]);
                }
            }
            let name = path.file_name().unwrap().to_string_lossy();
            assert_eq!(ours.len(), reference.len(), "{name}");
            let diffs = ours.iter().zip(&reference).filter(|(a, b)| a != b).count();
            let ny = w * h;
            let dy = ours[..ny]
                .iter()
                .zip(&reference)
                .filter(|(a, b)| a != b)
                .count();
            println!("{name}: {diffs} muestras distintas ({dy} de luma)");
            if diffs != 0 {
                failed.push(name.into_owned());
            }
            checked += 1;
        }
        assert!(checked > 0);
        assert!(failed.is_empty(), "{failed:?}");
    }
}
