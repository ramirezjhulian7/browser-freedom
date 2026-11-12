//! VP8L: el formato sin pérdida ("WebP Lossless Bitstream Specification", RFC 9649).
//!
//! Lector de bits LSB primero (§3.3), transformaciones (§4), códigos de prefijo normales y
//! simples con el código de longitudes de código (§3.7.2.1), LZ77 con el mapa de distancias 2D
//! (§5.2.2), caché de color (§5.2.3) e imagen de meta-prefijos (§6.2.2).

use crate::{ImageError, check_size, invalid};

/// Firma del encabezado VP8L (§3.1).
const SIGNATURE: u32 = 0x2f;
const NUM_LITERALS: usize = 256;
const NUM_LENGTH_CODES: usize = 24;
const NUM_DISTANCE_CODES: usize = 40;
const MAX_CACHE_BITS: u32 = 11;
/// Orden de las longitudes del código de longitudes de código (§3.7.2.1.2).
const CODE_LENGTH_ORDER: [usize; 19] = [
    17, 18, 0, 1, 2, 3, 4, 5, 16, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
];

/// Mapa de distancias 2D (§5.2.2): `(xi, yi)` de los códigos de distancia 1..=120.
#[rustfmt::skip]
const DISTANCE_MAP: [(i8, i8); 120] = [
    (0, 1), (1, 0), (1, 1), (-1, 1), (0, 2), (2, 0), (1, 2), (-1, 2),
    (2, 1), (-2, 1), (2, 2), (-2, 2), (0, 3), (3, 0), (1, 3), (-1, 3),
    (3, 1), (-3, 1), (2, 3), (-2, 3), (3, 2), (-3, 2), (0, 4), (4, 0),
    (1, 4), (-1, 4), (4, 1), (-4, 1), (3, 3), (-3, 3), (2, 4), (-2, 4),
    (4, 2), (-4, 2), (0, 5), (3, 4), (-3, 4), (4, 3), (-4, 3), (5, 0),
    (1, 5), (-1, 5), (5, 1), (-5, 1), (2, 5), (-2, 5), (5, 2), (-5, 2),
    (4, 4), (-4, 4), (3, 5), (-3, 5), (5, 3), (-5, 3), (0, 6), (6, 0),
    (1, 6), (-1, 6), (6, 1), (-6, 1), (2, 6), (-2, 6), (6, 2), (-6, 2),
    (4, 5), (-4, 5), (5, 4), (-5, 4), (3, 6), (-3, 6), (6, 3), (-6, 3),
    (0, 7), (7, 0), (1, 7), (-1, 7), (5, 5), (-5, 5), (7, 1), (-7, 1),
    (4, 6), (-4, 6), (6, 4), (-6, 4), (2, 7), (-2, 7), (7, 2), (-7, 2),
    (3, 7), (-3, 7), (7, 3), (-7, 3), (5, 6), (-5, 6), (6, 5), (-6, 5),
    (8, 0), (4, 7), (-4, 7), (7, 4), (-7, 4), (8, 1), (8, 2), (6, 6),
    (-6, 6), (8, 3), (5, 7), (-5, 7), (7, 5), (-7, 5), (8, 4), (6, 7),
    (-6, 7), (7, 6), (-7, 6), (8, 5), (7, 7), (-7, 7), (8, 6), (8, 7),
];

fn truncated() -> ImageError {
    invalid("VP8L truncado")
}

/// Decodifica un flujo VP8L completo (con encabezado, §3): `(ancho, alto, píxeles ARGB)`.
pub(super) fn decode(data: &[u8]) -> Result<(u32, u32, Vec<u32>), ImageError> {
    let mut br = BitReader::new(data);
    if br.read(8) != SIGNATURE {
        return Err(invalid("firma VP8L inválida"));
    }
    let width = br.read(14) + 1;
    let height = br.read(14) + 1;
    let _alpha_is_used = br.read(1);
    if br.read(3) != 0 {
        return Err(invalid("versión de VP8L desconocida"));
    }
    check_size(width, height)?;
    let argb = decode_image_stream(&mut br, width as usize, height as usize, true)?;
    Ok((width, height, argb))
}

/// Decodifica un flujo de imagen VP8L sin encabezado de tamaño (el del canal `ALPH`).
pub(super) fn decode_headerless(
    data: &[u8],
    width: usize,
    height: usize,
) -> Result<Vec<u32>, ImageError> {
    let mut br = BitReader::new(data);
    decode_image_stream(&mut br, width, height, true)
}

/// Lector de bits LSB primero (§3.3). Leer más allá del final da ceros y marca `overrun`.
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u64,
    nbits: u32,
    overrun: bool,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> BitReader<'a> {
        BitReader {
            data,
            pos: 0,
            buf: 0,
            nbits: 0,
            overrun: false,
        }
    }

    fn refill(&mut self) {
        while self.nbits <= 56 {
            let Some(&b) = self.data.get(self.pos) else {
                return;
            };
            self.buf |= u64::from(b) << self.nbits;
            self.pos += 1;
            self.nbits += 8;
        }
    }

    /// Los próximos 32 bits sin consumirlos (con ceros tras el final).
    fn peek(&mut self) -> u32 {
        if self.nbits < 32 {
            self.refill();
        }
        self.buf as u32
    }

    fn consume(&mut self, n: u32) {
        if n > self.nbits {
            self.overrun = true;
            self.buf = 0;
            self.nbits = 0;
        } else {
            self.buf >>= n;
            self.nbits -= n;
        }
    }

    /// Lee `n` ≤ 32 bits.
    fn read(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let v = u64::from(self.peek()) & ((1u64 << n) - 1);
        self.consume(n);
        v as u32
    }

    fn check(&self) -> Result<(), ImageError> {
        if self.overrun {
            Err(truncated())
        } else {
            Ok(())
        }
    }
}

/// Entrada de una tabla de decodificación de códigos de prefijo. En la tabla raíz, si
/// `bits > root_bits` es un enlace: `value` es el inicio de la subtabla y `bits - root_bits`
/// su cantidad de bits.
#[derive(Clone, Copy, Default)]
struct Entry {
    bits: u8,
    value: u16,
}

/// Código de prefijo canónico (§3.7.2) decodificado con tablas de dos niveles.
struct Huffman {
    root_bits: u32,
    table: Vec<Entry>,
}

impl Huffman {
    const MAX_ROOT_BITS: u32 = 8;

    /// Construye el código a partir de sus longitudes (0 = símbolo ausente). Un solo símbolo
    /// se codifica con cero bits; si no, el árbol debe estar completo.
    fn build(lengths: &[u8]) -> Result<Huffman, ImageError> {
        let mut count = [0u32; 16];
        let mut single = None;
        for (sym, &len) in lengths.iter().enumerate() {
            if len > 15 {
                return Err(invalid("longitud de código inválida"));
            }
            count[len as usize] += 1;
            if len > 0 {
                single = Some(sym);
            }
        }
        let used = lengths.len() as u32 - count[0];
        match (used, single) {
            (0, _) | (_, None) => return Err(invalid("código de prefijo vacío")),
            (1, Some(sym)) => {
                return Ok(Huffman {
                    root_bits: 0,
                    table: vec![Entry {
                        bits: 0,
                        value: sym as u16,
                    }],
                });
            }
            _ => {}
        }
        // ¿Árbol completo y sin sobresuscripción?
        let mut left: i64 = 1;
        let mut max_len = 0;
        for (len, &c) in count.iter().enumerate().skip(1) {
            left = (left << 1) - i64::from(c);
            if left < 0 {
                return Err(invalid("código de prefijo sobresuscrito"));
            }
            if c > 0 {
                max_len = len as u32;
            }
        }
        if left != 0 {
            return Err(invalid("código de prefijo incompleto"));
        }
        // Códigos canónicos (como en Deflate), invertidos porque se leen LSB primero.
        let mut next = [0u32; 16];
        let mut code = 0u32;
        for len in 1..16 {
            code = (code + count[len - 1] * u32::from(len > 1)) << 1;
            next[len] = code;
        }
        let root_bits = max_len.min(Self::MAX_ROOT_BITS);
        let root_size = 1usize << root_bits;
        let mut codes = vec![0u16; lengths.len()];
        let mut sub_bits = vec![0u8; root_size];
        for (sym, &len) in lengths.iter().enumerate() {
            if len == 0 {
                continue;
            }
            let c = next[len as usize];
            next[len as usize] += 1;
            let rev = (c.reverse_bits() >> (32 - u32::from(len))) as u16;
            codes[sym] = rev;
            if u32::from(len) > root_bits {
                let prefix = usize::from(rev) & (root_size - 1);
                sub_bits[prefix] = sub_bits[prefix].max(len - root_bits as u8);
            }
        }
        let mut table = vec![Entry::default(); root_size];
        let mut offsets = vec![0u16; root_size];
        for (prefix, &sb) in sub_bits.iter().enumerate() {
            if sb > 0 {
                offsets[prefix] = table.len() as u16;
                table[prefix] = Entry {
                    bits: root_bits as u8 + sb,
                    value: table.len() as u16,
                };
                table.resize(table.len() + (1 << sb), Entry::default());
            }
        }
        for (sym, &len) in lengths.iter().enumerate() {
            if len == 0 {
                continue;
            }
            let rev = usize::from(codes[sym]);
            let len = u32::from(len);
            if len <= root_bits {
                let entry = Entry {
                    bits: len as u8,
                    value: sym as u16,
                };
                for i in (rev..root_size).step_by(1 << len) {
                    table[i] = entry;
                }
            } else {
                let prefix = rev & (root_size - 1);
                let sb = usize::from(sub_bits[prefix]);
                let base = usize::from(offsets[prefix]);
                let sub_len = len - root_bits;
                let entry = Entry {
                    bits: sub_len as u8,
                    value: sym as u16,
                };
                for i in ((rev >> root_bits)..(1 << sb)).step_by(1 << sub_len) {
                    table[base + i] = entry;
                }
            }
        }
        Ok(Huffman { root_bits, table })
    }

    fn read(&self, br: &mut BitReader) -> u32 {
        let bits = br.peek();
        let e = self.table[(bits & ((1 << self.root_bits) - 1)) as usize];
        if u32::from(e.bits) <= self.root_bits {
            br.consume(u32::from(e.bits));
            return u32::from(e.value);
        }
        let sub = u32::from(e.bits) - self.root_bits;
        let idx = usize::from(e.value) + ((bits >> self.root_bits) & ((1 << sub) - 1)) as usize;
        let e2 = self.table[idx];
        br.consume(self.root_bits + u32::from(e2.bits));
        u32::from(e2.value)
    }
}

/// Lee un código de prefijo de un alfabeto de `size` símbolos (§3.7.2.1).
fn read_huffman(br: &mut BitReader, size: usize) -> Result<Huffman, ImageError> {
    let mut lengths = vec![0u8; size];
    if br.read(1) == 1 {
        // Código simple: uno o dos símbolos.
        let num_symbols = br.read(1) + 1;
        let first_bits = if br.read(1) == 1 { 8 } else { 1 };
        let s0 = br.read(first_bits) as usize;
        *lengths
            .get_mut(s0)
            .ok_or_else(|| invalid("símbolo fuera del alfabeto"))? = 1;
        if num_symbols == 2 {
            let s1 = br.read(8) as usize;
            *lengths
                .get_mut(s1)
                .ok_or_else(|| invalid("símbolo fuera del alfabeto"))? = 1;
        }
    } else {
        // Código normal: primero el código de longitudes de código.
        let num_code_lengths = 4 + br.read(4) as usize;
        let mut cl_lengths = [0u8; 19];
        for &i in &CODE_LENGTH_ORDER[..num_code_lengths] {
            cl_lengths[i] = br.read(3) as u8;
        }
        br.check()?;
        let cl = Huffman::build(&cl_lengths)?;
        let mut max_symbol = if br.read(1) == 1 {
            let length_nbits = 2 + 2 * br.read(3);
            let m = 2 + br.read(length_nbits) as usize;
            if m > size {
                return Err(invalid("max_symbol mayor que el alfabeto"));
            }
            m
        } else {
            size
        };
        let mut prev = 8u8;
        let mut sym = 0;
        while sym < size {
            if max_symbol == 0 {
                break;
            }
            max_symbol -= 1;
            let code = cl.read(br);
            if code < 16 {
                lengths[sym] = code as u8;
                sym += 1;
                if code != 0 {
                    prev = code as u8;
                }
            } else {
                let (extra, offset, value) = match code {
                    16 => (2, 3, prev),
                    17 => (3, 3, 0),
                    _ => (7, 11, 0),
                };
                let repeat = br.read(extra) as usize + offset;
                let run = lengths
                    .get_mut(sym..sym + repeat)
                    .ok_or_else(|| invalid("repetición de longitudes fuera del alfabeto"))?;
                run.fill(value);
                sym += repeat;
            }
            br.check()?;
        }
    }
    br.check()?;
    Huffman::build(&lengths)
}

/// Grupo de cinco códigos de prefijo (§6.2.2): verde+longitud+caché, rojo, azul, alfa,
/// distancia.
struct Group {
    green: Huffman,
    red: Huffman,
    blue: Huffman,
    alpha: Huffman,
    dist: Huffman,
}

fn read_group(br: &mut BitReader, cache_size: usize) -> Result<Group, ImageError> {
    Ok(Group {
        green: read_huffman(br, NUM_LITERALS + NUM_LENGTH_CODES + cache_size)?,
        red: read_huffman(br, NUM_LITERALS)?,
        blue: read_huffman(br, NUM_LITERALS)?,
        alpha: read_huffman(br, NUM_LITERALS)?,
        dist: read_huffman(br, NUM_DISTANCE_CODES)?,
    })
}

enum Transform {
    /// §4.1: bits del bloque y subimagen de modos.
    Predictor { bits: u32, data: Vec<u32> },
    /// §4.2: bits del bloque y subimagen de elementos de transformación.
    Color { bits: u32, data: Vec<u32> },
    /// §4.3.
    SubtractGreen,
    /// §4.4: paleta y bits de empaquetado; `xsize` es el ancho antes de empaquetar.
    ColorIndexing {
        palette: Vec<u32>,
        width_bits: u32,
        xsize: usize,
    },
}

fn div_round_up(n: usize, bits: u32) -> usize {
    (n + (1 << bits) - 1) >> bits
}

/// Decodifica un flujo de imagen (§5): transformaciones (solo en el nivel principal), caché
/// de color, meta-prefijos y los píxeles codificados con entropía.
fn decode_image_stream(
    br: &mut BitReader,
    xsize: usize,
    ysize: usize,
    is_level0: bool,
) -> Result<Vec<u32>, ImageError> {
    let mut transforms = Vec::new();
    let mut xsize_cur = xsize;
    if is_level0 {
        let mut seen = 0u8;
        while br.read(1) == 1 {
            let ty = br.read(2);
            if seen & (1 << ty) != 0 {
                return Err(invalid("transformación VP8L repetida"));
            }
            seen |= 1 << ty;
            match ty {
                0 | 1 => {
                    let bits = br.read(3) + 2;
                    let data = decode_image_stream(
                        br,
                        div_round_up(xsize_cur, bits),
                        div_round_up(ysize, bits),
                        false,
                    )?;
                    transforms.push(if ty == 0 {
                        Transform::Predictor { bits, data }
                    } else {
                        Transform::Color { bits, data }
                    });
                }
                2 => transforms.push(Transform::SubtractGreen),
                _ => {
                    let size = br.read(8) as usize + 1;
                    let mut palette = decode_image_stream(br, size, 1, false)?;
                    for i in 1..palette.len() {
                        palette[i] = add_pixels(palette[i], palette[i - 1]);
                    }
                    let width_bits = match size {
                        0..=2 => 3,
                        3..=4 => 2,
                        5..=16 => 1,
                        _ => 0,
                    };
                    transforms.push(Transform::ColorIndexing {
                        palette,
                        width_bits,
                        xsize: xsize_cur,
                    });
                    xsize_cur = div_round_up(xsize_cur, width_bits);
                }
            }
            br.check()?;
        }
    }

    let cache_bits = if br.read(1) == 1 {
        let b = br.read(4);
        if !(1..=MAX_CACHE_BITS).contains(&b) {
            return Err(invalid("caché de color inválida"));
        }
        b
    } else {
        0
    };
    let cache_size = if cache_bits > 0 { 1 << cache_bits } else { 0 };

    let meta = if is_level0 && br.read(1) == 1 {
        let bits = br.read(3) + 2;
        let ew = div_round_up(xsize_cur, bits);
        let image = decode_image_stream(br, ew, div_round_up(ysize, bits), false)?;
        Some((image, bits, ew))
    } else {
        None
    };
    br.check()?;
    let num_groups = match &meta {
        Some((image, ..)) => {
            image.iter().map(|&p| (p >> 8) & 0xffff).max().unwrap_or(0) as usize + 1
        }
        None => 1,
    };
    let mut groups = Vec::with_capacity(num_groups.min(1024));
    for _ in 0..num_groups {
        groups.push(read_group(br, cache_size)?);
    }

    let mut data = decode_pixels(
        br,
        xsize_cur,
        ysize,
        &groups,
        meta.as_ref()
            .map(|(img, bits, ew)| (img.as_slice(), *bits, *ew)),
        cache_bits,
    )?;

    for t in transforms.iter().rev() {
        match t {
            Transform::Predictor { bits, data: modes } => {
                inverse_predictor(&mut data, xsize_cur, ysize, *bits, modes);
            }
            Transform::Color { bits, data: elems } => {
                inverse_color(&mut data, xsize_cur, *bits, elems);
            }
            Transform::SubtractGreen => {
                for p in &mut data {
                    let g = (*p >> 8) & 0xff;
                    *p = add_pixels(*p, (g << 16) | g);
                }
            }
            Transform::ColorIndexing {
                palette,
                width_bits,
                xsize,
            } => {
                data =
                    inverse_color_indexing(&data, xsize_cur, *xsize, ysize, *width_bits, palette);
                xsize_cur = *xsize;
            }
        }
    }
    Ok(data)
}

/// Valor de un código de prefijo de longitud o distancia (§5.2.2).
fn prefix_value(br: &mut BitReader, sym: u32) -> usize {
    if sym < 4 {
        return sym as usize + 1;
    }
    let extra = (sym - 2) >> 1;
    let offset = (2 + (sym & 1)) << extra;
    (offset + br.read(extra)) as usize + 1
}

/// Código de distancia → distancia lineal con el mapa 2D (§5.2.2).
fn plane_to_distance(xsize: usize, code: usize) -> usize {
    if code > 120 {
        return code - 120;
    }
    let (xi, yi) = DISTANCE_MAP[code - 1];
    let d = i64::from(yi) * xsize as i64 + i64::from(xi);
    d.max(1) as usize
}

/// Decodifica los píxeles con LZ77, caché de color y los grupos de códigos (§5.2).
fn decode_pixels(
    br: &mut BitReader,
    xsize: usize,
    ysize: usize,
    groups: &[Group],
    meta: Option<(&[u32], u32, usize)>,
    cache_bits: u32,
) -> Result<Vec<u32>, ImageError> {
    let total = xsize * ysize;
    let mut out = vec![0u32; total];
    let mut cache = vec![0u32; if cache_bits > 0 { 1 << cache_bits } else { 0 }];
    let cache_shift = 32 - cache_bits;
    let mut last_cached = 0;
    let mask = match meta {
        Some((_, bits, _)) => (1usize << bits) - 1,
        None => usize::MAX,
    };
    let group_at = |x: usize, y: usize| group_for(groups, meta, x, y);
    let (mut x, mut y) = (0, 0);
    let mut pos = 0;
    let mut group = &groups[0];
    while pos < total {
        if x & mask == 0 {
            group = group_at(x, y);
        }
        let code = group.green.read(br);
        if code < NUM_LITERALS as u32 {
            let r = group.red.read(br);
            let b = group.blue.read(br);
            let a = group.alpha.read(br);
            out[pos] = (a << 24) | (r << 16) | (code << 8) | b;
            pos += 1;
            x += 1;
            if x == xsize {
                x = 0;
                y += 1;
                br.check()?;
            }
        } else if code < (NUM_LITERALS + NUM_LENGTH_CODES) as u32 {
            let len = prefix_value(br, code - NUM_LITERALS as u32);
            let dist_sym = group.dist.read(br);
            let dist = plane_to_distance(xsize, prefix_value(br, dist_sym));
            br.check()?;
            if dist > pos || len > total - pos {
                return Err(invalid("referencia LZ77 fuera de la imagen"));
            }
            for i in pos..pos + len {
                out[i] = out[i - dist];
            }
            pos += len;
            x += len;
            y += x / xsize;
            x %= xsize;
            if pos < total && x & mask != 0 {
                group = group_at(x, y);
            }
        } else {
            let key = (code - (NUM_LITERALS + NUM_LENGTH_CODES) as u32) as usize;
            while last_cached < pos {
                let p = out[last_cached];
                cache[(0x1e35a7bd_u32.wrapping_mul(p) >> cache_shift) as usize] = p;
                last_cached += 1;
            }
            out[pos] = *cache
                .get(key)
                .ok_or_else(|| invalid("índice de caché inválido"))?;
            pos += 1;
            x += 1;
            if x == xsize {
                x = 0;
                y += 1;
                br.check()?;
            }
        }
    }
    br.check()?;
    Ok(out)
}

/// El grupo de códigos del píxel `(x, y)` según la imagen de meta-prefijos (§6.2.2).
fn group_for<'g>(
    groups: &'g [Group],
    meta: Option<(&[u32], u32, usize)>,
    x: usize,
    y: usize,
) -> &'g Group {
    match meta {
        Some((img, bits, ew)) => {
            let idx = img[(y >> bits) * ew + (x >> bits)];
            &groups[((idx >> 8) & 0xffff) as usize]
        }
        None => &groups[0],
    }
}

/// Suma por componente, módulo 256.
fn add_pixels(a: u32, b: u32) -> u32 {
    let ag = (a & 0xff00ff00).wrapping_add(b & 0xff00ff00) & 0xff00ff00;
    let rb = (a & 0x00ff00ff).wrapping_add(b & 0x00ff00ff) & 0x00ff00ff;
    ag | rb
}

/// Promedio por componente, redondeando hacia abajo.
fn average2(a: u32, b: u32) -> u32 {
    (((a ^ b) & 0xfefefefe) >> 1) + (a & b)
}

fn channel(p: u32, shift: u32) -> i32 {
    ((p >> shift) & 0xff) as i32
}

fn select(l: u32, t: u32, tl: u32) -> u32 {
    let mut pl = 0;
    let mut pt = 0;
    for s in [24, 16, 8, 0] {
        let (cl, ct, ctl) = (channel(l, s), channel(t, s), channel(tl, s));
        // Distancia de la estimación `L + T - TL` a cada vecino.
        pl += (ct - ctl).abs();
        pt += (cl - ctl).abs();
    }
    if pl < pt { l } else { t }
}

fn clamp_add_subtract_full(a: u32, b: u32, c: u32) -> u32 {
    let mut out = 0;
    for s in [24, 16, 8, 0] {
        let v = (channel(a, s) + channel(b, s) - channel(c, s)).clamp(0, 255) as u32;
        out |= v << s;
    }
    out
}

fn clamp_add_subtract_half(a: u32, b: u32) -> u32 {
    let mut out = 0;
    for s in [24, 16, 8, 0] {
        let (ca, cb) = (channel(a, s), channel(b, s));
        let v = (ca + (ca - cb) / 2).clamp(0, 255) as u32;
        out |= v << s;
    }
    out
}

/// Transformación predictora inversa (§4.1).
fn inverse_predictor(data: &mut [u32], w: usize, h: usize, bits: u32, modes: &[u32]) {
    if data.is_empty() {
        return;
    }
    data[0] = add_pixels(data[0], 0xff000000);
    for x in 1..w {
        data[x] = add_pixels(data[x], data[x - 1]);
    }
    let bw = div_round_up(w, bits);
    for y in 1..h {
        let row = y * w;
        data[row] = add_pixels(data[row], data[row - w]);
        let mrow = &modes[(y >> bits) * bw..];
        for x in 1..w {
            let i = row + x;
            let mode = (mrow[x >> bits] >> 8) & 0xf;
            let l = data[i - 1];
            let t = data[i - w];
            let pred = match mode {
                1 => l,
                2 => t,
                3 => data[i - w + 1],
                4 => data[i - w - 1],
                5 => average2(average2(l, data[i - w + 1]), t),
                6 => average2(l, data[i - w - 1]),
                7 => average2(l, t),
                8 => average2(data[i - w - 1], t),
                9 => average2(t, data[i - w + 1]),
                10 => average2(average2(l, data[i - w - 1]), average2(t, data[i - w + 1])),
                11 => select(l, t, data[i - w - 1]),
                12 => clamp_add_subtract_full(l, t, data[i - w - 1]),
                13 => clamp_add_subtract_half(average2(l, t), data[i - w - 1]),
                // 0, y 14 y 15 (sin uso) se comportan como 0.
                _ => 0xff000000,
            };
            data[i] = add_pixels(data[i], pred);
        }
    }
}

fn color_delta(t: u32, c: u32) -> i32 {
    (i32::from(t as u8 as i8) * i32::from(c as u8 as i8)) >> 5
}

/// Transformación de color inversa (§4.2).
fn inverse_color(data: &mut [u32], w: usize, bits: u32, elems: &[u32]) {
    let bw = div_round_up(w, bits);
    for (y, row) in data.chunks_exact_mut(w).enumerate() {
        let erow = &elems[(y >> bits) * bw..];
        for (x, p) in row.iter_mut().enumerate() {
            let m = erow[x >> bits];
            let (g2r, g2b, r2b) = (m & 0xff, (m >> 8) & 0xff, (m >> 16) & 0xff);
            let argb = *p;
            let green = (argb >> 8) & 0xff;
            let red = ((((argb >> 16) & 0xff) as i32 + color_delta(g2r, green)) & 0xff) as u32;
            let blue = (((argb & 0xff) as i32 + color_delta(g2b, green) + color_delta(r2b, red))
                & 0xff) as u32;
            *p = (argb & 0xff00ff00) | (red << 16) | blue;
        }
    }
}

/// Transformación de indexado de color inversa (§4.4): desempaqueta y aplica la paleta.
fn inverse_color_indexing(
    data: &[u32],
    packed_w: usize,
    w: usize,
    h: usize,
    width_bits: u32,
    palette: &[u32],
) -> Vec<u32> {
    let lookup = |i: u32| palette.get(i as usize).copied().unwrap_or(0);
    let mut out = vec![0u32; w * h];
    let bits_per_px = 8 >> width_bits;
    let px_mask = (1u32 << width_bits) - 1;
    let val_mask = (1u32 << bits_per_px) - 1;
    for y in 0..h {
        let src = &data[y * packed_w..(y + 1) * packed_w];
        let dst = &mut out[y * w..(y + 1) * w];
        for (x, d) in dst.iter_mut().enumerate() {
            let g = (src[x >> width_bits] >> 8) & 0xff;
            let shift = (x as u32 & px_mask) * bits_per_px;
            *d = lookup((g >> shift) & val_mask);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huffman_two_levels() {
        // Longitudes 1, 2, 3, ..., 14, 15, 15: árbol completo y muy profundo.
        let mut lengths: Vec<u8> = (1..=15).collect();
        lengths.push(15);
        let h = Huffman::build(&lengths).unwrap();
        // Cada símbolo s < 15 tiene el código canónico 1…10 (s unos y un cero), LSB primero.
        for (s, &len) in lengths.iter().enumerate() {
            let len = u32::from(len);
            let code: u32 = if s < 15 {
                (1 << len) - 2
            } else {
                (1 << 15) - 1
            };
            let rev = code.reverse_bits() >> (32 - len);
            let bytes = rev.to_le_bytes();
            let mut br = BitReader::new(&bytes);
            assert_eq!(h.read(&mut br), s as u32);
        }
    }

    #[test]
    fn incomplete_codes_fail() {
        assert!(Huffman::build(&[1, 2, 0]).is_err());
        assert!(Huffman::build(&[1, 1, 1]).is_err());
        assert!(Huffman::build(&[0, 0]).is_err());
        assert!(Huffman::build(&[0, 3]).is_ok());
    }
}
