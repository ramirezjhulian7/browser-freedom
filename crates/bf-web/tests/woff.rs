//! E2: fuentes WOFF 1.0 y WOFF 2.0 (W3C, "WOFF File Format 1.0" y "WOFF File Format 2.0").
//!
//! Los archivos se generan aquí a partir de Ahem (`tests/wpt/fonts/Ahem.ttf`), con los
//! descompresores reales que registra el navegador (los de `bf-net`):
//! - WOFF 1.0 con tablas comprimidas con zlib (un deflate mínimo escrito aquí: Huffman fijo y
//!   repeticiones a distancia 1) y tablas sin comprimir: el sfnt rearmado tiene las mismas
//!   tablas, byte a byte, que el original.
//! - WOFF 2.0 sin transformar (transformación nula de `glyf`/`loca`, §5.1): idéntico tabla a
//!   tabla. El flujo Brotli usa meta-bloques sin comprimir (RFC 7932 §9.2).
//! - WOFF 2.0 con `glyf`/`loca` (§5.1 a §5.3) y `hmtx` (§5.4) transformadas: las demás tablas,
//!   idénticas; `hmtx`, idéntica; cada glifo, con el mismo contorno, caja y avance.
//! - Un archivo dañado (cortado o con bytes cambiados) se rechaza sin pánico.

use std::path::PathBuf;

fn ahem() -> Option<Vec<u8>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/wpt/fonts/Ahem.ttf");
    match std::fs::read(&path) {
        Ok(d) => Some(d),
        Err(_) => {
            eprintln!("aviso: sin Ahem ({}), test omitido", path.display());
            None
        }
    }
}

fn setup() {
    bf_web::cssom::font_loading::register_font_decompressors();
}

fn be16(d: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([d[at], d[at + 1]])
}

fn be32(d: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]])
}

/// Las tablas de un sfnt: (etiqueta, suma de comprobación, datos), en el orden del directorio.
/// Una tabla de un sfnt: etiqueta, suma de comprobación y datos.
type Table = ([u8; 4], u32, Vec<u8>);

fn tables(sfnt: &[u8]) -> Vec<Table> {
    let n = usize::from(be16(sfnt, 4));
    (0..n)
        .map(|i| {
            let e = 12 + 16 * i;
            let off = be32(sfnt, e + 8) as usize;
            let len = be32(sfnt, e + 12) as usize;
            (
                sfnt[e..e + 4].try_into().unwrap(),
                be32(sfnt, e + 4),
                sfnt[off..off + len].to_vec(),
            )
        })
        .collect()
}

fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |s, c| {
        let mut w = [0u8; 4];
        w[..c.len()].copy_from_slice(c);
        s.wrapping_add(u32::from_be_bytes(w))
    })
}

/// Comprueba la cabecera y el directorio del sfnt rearmado (OpenType, "Table Directory"):
/// tablas en orden, alineadas a 4 bytes con ceros, y sumas de comprobación correctas.
fn check_sfnt_layout(sfnt: &[u8]) {
    let n = be16(sfnt, 4);
    let sel = be16(sfnt, 8);
    assert_eq!(be16(sfnt, 6), (1 << sel) * 16, "searchRange");
    assert!((1u32 << sel) <= u32::from(n) && (2u32 << sel) > u32::from(n));
    assert_eq!(be16(sfnt, 10), n * 16 - be16(sfnt, 6), "rangeShift");
    let mut prev: Option<[u8; 4]> = None;
    for i in 0..usize::from(n) {
        let e = 12 + 16 * i;
        let tag: [u8; 4] = sfnt[e..e + 4].try_into().unwrap();
        assert!(prev.is_none_or(|p| p < tag), "orden de etiquetas");
        prev = Some(tag);
        let off = be32(sfnt, e + 8) as usize;
        let len = be32(sfnt, e + 12) as usize;
        assert_eq!(off % 4, 0, "{}: sin alinear", String::from_utf8_lossy(&tag));
        let mut data = sfnt[off..off + len].to_vec();
        if &tag == b"head" {
            data[8..12].fill(0);
        }
        assert_eq!(be32(sfnt, e + 4), checksum(&data));
        let pad_end = off + len.div_ceil(4) * 4;
        assert!(sfnt[off + len..pad_end].iter().all(|&b| b == 0), "relleno");
    }
    assert_eq!(sfnt.len() % 4, 0);
    // `head.checkSumAdjustment`: la suma de toda la fuente da 0xB1B0AFBA.
    assert_eq!(checksum(sfnt), 0xB1B0_AFBA);
}

// --- zlib mínimo (RFC 1950/1951): Huffman fijo, literales y repeticiones a distancia 1 ---

struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn new() -> Bits {
        Bits {
            out: Vec::new(),
            acc: 0,
            n: 0,
        }
    }
    /// `count` bits de `v`, el menos significativo primero.
    fn put(&mut self, v: u32, count: u32) {
        self.acc |= u64::from(v) << self.n;
        self.n += count;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }
    /// Un código de Huffman (se escribe del bit más significativo al menos).
    fn code(&mut self, code: u32, len: u32) {
        let rev = code.reverse_bits() >> (32 - len);
        self.put(rev, len);
    }
    fn align(&mut self) {
        if self.n > 0 {
            self.put(0, 8 - self.n);
        }
    }
    fn finish(mut self) -> Vec<u8> {
        self.align();
        self.out
    }
}

fn fixed_symbol(b: &mut Bits, sym: u32) {
    match sym {
        0..=143 => b.code(0x30 + sym, 8),
        144..=255 => b.code(0x190 + sym - 144, 9),
        256..=279 => b.code(sym - 256, 7),
        _ => b.code(0xC0 + sym - 280, 8),
    }
}

const LEN_BASE: [u32; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u32; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut b = Bits::new();
    b.put(0x78, 8);
    b.put(0x01, 8);
    b.put(1, 1); // BFINAL
    b.put(1, 2); // BTYPE = 01, Huffman fijo
    let mut i = 0;
    while i < data.len() {
        let mut run = 0;
        while i > 0 && i + run < data.len() && run < 258 && data[i + run] == data[i - 1] {
            run += 1;
        }
        if run >= 3 {
            let k = LEN_BASE.iter().rposition(|&l| l <= run as u32).unwrap();
            fixed_symbol(&mut b, 257 + k as u32);
            b.put(run as u32 - LEN_BASE[k], LEN_EXTRA[k]);
            b.code(0, 5); // distancia 1
            i += run;
        } else {
            fixed_symbol(&mut b, u32::from(data[i]));
            i += 1;
        }
    }
    fixed_symbol(&mut b, 256);
    let mut out = b.finish();
    let (mut s1, mut s2) = (1u32, 0u32);
    for &x in data {
        s1 = (s1 + u32::from(x)) % 65521;
        s2 = (s2 + s1) % 65521;
    }
    out.extend_from_slice(&((s2 << 16) | s1).to_be_bytes());
    out
}

// --- Brotli con meta-bloques sin comprimir (RFC 7932 §9.2) ---

fn brotli_stored(data: &[u8]) -> Vec<u8> {
    let mut b = Bits::new();
    b.put(0, 1); // WBITS = 16
    for chunk in data.chunks(65536) {
        b.put(0, 1); // ISLAST
        b.put(0, 2); // MNIBBLES = 4
        b.put(chunk.len() as u32 - 1, 16);
        b.put(1, 1); // ISUNCOMPRESSED
        b.align();
        b.out.extend_from_slice(chunk);
    }
    b.put(1, 1); // ISLAST
    b.put(1, 1); // ISLASTEMPTY
    b.finish()
}

// --- Escritura de WOFF ---

fn woff1(sfnt: &[u8]) -> (Vec<u8>, usize) {
    let tabs = tables(sfnt);
    let mut blobs = Vec::new();
    let mut compressed = 0;
    for (_, _, data) in &tabs {
        let z = zlib(data);
        // §5: si comprimir no ahorra, la tabla va tal cual.
        if z.len() < data.len() {
            compressed += 1;
            blobs.push(z);
        } else {
            blobs.push(data.clone());
        }
    }
    let header = 44 + 20 * tabs.len();
    let mut out = vec![0u8; header];
    let mut dir = Vec::new();
    for ((tag, sum, data), blob) in tabs.iter().zip(&blobs) {
        dir.extend_from_slice(tag);
        dir.extend_from_slice(&(out.len() as u32).to_be_bytes());
        dir.extend_from_slice(&(blob.len() as u32).to_be_bytes());
        dir.extend_from_slice(&(data.len() as u32).to_be_bytes());
        dir.extend_from_slice(&sum.to_be_bytes());
        out.extend_from_slice(blob);
        out.resize(out.len().div_ceil(4) * 4, 0);
    }
    out[44..header].copy_from_slice(&dir);
    out[0..4].copy_from_slice(b"wOFF");
    out[4..8].copy_from_slice(&sfnt[0..4]);
    let len = out.len() as u32;
    out[8..12].copy_from_slice(&len.to_be_bytes());
    out[12..14].copy_from_slice(&(tabs.len() as u16).to_be_bytes());
    out[16..20].copy_from_slice(&(sfnt.len() as u32).to_be_bytes());
    (out, compressed)
}

fn base128(out: &mut Vec<u8>, v: u32) {
    let mut groups = vec![(v & 0x7F) as u8];
    let mut v = v >> 7;
    while v > 0 {
        groups.push((v & 0x7F) as u8 | 0x80);
        v >>= 7;
    }
    groups.reverse();
    out.extend_from_slice(&groups);
}

fn u255(out: &mut Vec<u8>, v: u16) {
    match v {
        0..=252 => out.push(v as u8),
        253..=505 => out.extend_from_slice(&[255, (v - 253) as u8]),
        506..=761 => out.extend_from_slice(&[254, (v - 506) as u8]),
        _ => {
            out.push(253);
            out.extend_from_slice(&v.to_be_bytes());
        }
    }
}

/// Una tabla del directorio de WOFF 2.0: etiqueta, versión de transformación, longitud
/// original y datos que van al flujo (transformados o no).
struct W2Table {
    tag: [u8; 4],
    version: u8,
    orig: usize,
    data: Vec<u8>,
}

const KNOWN: [&[u8; 4]; 12] = [
    b"cmap", b"head", b"hhea", b"hmtx", b"maxp", b"name", b"OS/2", b"post", b"cvt ", b"fpgm",
    b"glyf", b"loca",
];

fn woff2(flavor: &[u8], tabs: Vec<W2Table>, transformed_len: impl Fn(&W2Table) -> bool) -> Vec<u8> {
    let mut dir = Vec::new();
    let mut stream = Vec::new();
    for t in &tabs {
        let known = KNOWN.iter().position(|k| **k == t.tag);
        dir.push(known.map_or(63, |k| k as u8) | (t.version << 6));
        if known.is_none() {
            dir.extend_from_slice(&t.tag);
        }
        base128(&mut dir, t.orig as u32);
        if transformed_len(t) {
            base128(&mut dir, t.data.len() as u32);
        }
        stream.extend_from_slice(&t.data);
    }
    let compressed = brotli_stored(&stream);
    let mut out = vec![0u8; 48];
    out.extend_from_slice(&dir);
    out.extend_from_slice(&compressed);
    out.resize(out.len().div_ceil(4) * 4, 0);
    out[0..4].copy_from_slice(b"wOF2");
    out[4..8].copy_from_slice(flavor);
    let len = out.len() as u32;
    out[8..12].copy_from_slice(&len.to_be_bytes());
    out[12..14].copy_from_slice(&(tabs.len() as u16).to_be_bytes());
    let total: usize =
        12 + 16 * tabs.len() + tabs.iter().map(|t| t.orig.div_ceil(4) * 4).sum::<usize>();
    out[16..20].copy_from_slice(&(total as u32).to_be_bytes());
    out[20..24].copy_from_slice(&(compressed.len() as u32).to_be_bytes());
    out
}

/// Un punto de un glifo simple de TrueType: coordenadas absolutas y si está sobre la curva.
struct Glyph {
    contours: i16,
    bbox: [i16; 4],
    end_points: Vec<u16>,
    instructions: Vec<u8>,
    points: Vec<(i32, i32, bool)>,
}

/// Lee los glifos simples de `glyf`/`loca` (`None` en los vacíos).
fn read_glyphs(glyf: &[u8], loca: &[u8], long: bool, n: usize) -> Vec<Option<Glyph>> {
    let off = |i: usize| {
        if long {
            be32(loca, 4 * i) as usize
        } else {
            2 * usize::from(be16(loca, 2 * i))
        }
    };
    (0..n)
        .map(|i| {
            let (s, e) = (off(i), off(i + 1));
            if s == e {
                return None;
            }
            let g = &glyf[s..e];
            let contours = be16(g, 0) as i16;
            assert!(contours > 0, "Ahem solo tiene glifos simples");
            let bbox = [0, 1, 2, 3].map(|k| be16(g, 2 + 2 * k) as i16);
            let mut p = 10;
            let end_points: Vec<u16> = (0..contours)
                .map(|_| {
                    p += 2;
                    be16(g, p - 2)
                })
                .collect();
            let n_points = usize::from(*end_points.last().unwrap()) + 1;
            let ilen = usize::from(be16(g, p));
            let instructions = g[p + 2..p + 2 + ilen].to_vec();
            p += 2 + ilen;
            let mut flags = Vec::new();
            while flags.len() < n_points {
                let f = g[p];
                p += 1;
                flags.push(f);
                if f & 8 != 0 {
                    let r = g[p];
                    p += 1;
                    flags.extend(std::iter::repeat_n(f, usize::from(r)));
                }
            }
            let mut coord = |short: u8, same: u8| {
                let mut v = 0i32;
                flags
                    .iter()
                    .map(|&f| {
                        if f & short != 0 {
                            let d = i32::from(g[p]);
                            p += 1;
                            v += if f & same != 0 { d } else { -d };
                        } else if f & same == 0 {
                            v += i32::from(be16(g, p) as i16);
                            p += 2;
                        }
                        v
                    })
                    .collect::<Vec<_>>()
            };
            let xs = coord(0x02, 0x10);
            let ys = coord(0x04, 0x20);
            let points = (0..n_points)
                .map(|k| (xs[k], ys[k], flags[k] & 1 != 0))
                .collect();
            Some(Glyph {
                contours,
                bbox,
                end_points,
                instructions,
                points,
            })
        })
        .collect()
}

/// Un desplazamiento como tripleta (WOFF 2.0 §5.2, la codificación de la implementación de
/// referencia): la bandera y sus bytes.
fn triplet(dx: i32, dy: i32, on: bool, flags: &mut Vec<u8>, glyphs: &mut Vec<u8>) {
    let (ax, ay) = (dx.unsigned_abs(), dy.unsigned_abs());
    let xs = u32::from(dx >= 0);
    let ys = u32::from(dy >= 0);
    let xy = xs + 2 * ys;
    let flag = if dx == 0 && ay < 1280 {
        glyphs.push(ay as u8);
        ((ay & 0xF00) >> 7) + ys
    } else if dy == 0 && ax < 1280 {
        glyphs.push(ax as u8);
        10 + ((ax & 0xF00) >> 7) + xs
    } else if ax < 65 && ay < 65 {
        glyphs.push((((ax - 1) & 0xF) << 4 | ((ay - 1) & 0xF)) as u8);
        20 + ((ax - 1) & 0x30) + (((ay - 1) & 0x30) >> 2) + xy
    } else if ax < 769 && ay < 769 {
        glyphs.extend_from_slice(&[(ax - 1) as u8, (ay - 1) as u8]);
        84 + 12 * (((ax - 1) & 0x300) >> 8) + (((ay - 1) & 0x300) >> 6) + xy
    } else if ax < 4096 && ay < 4096 {
        glyphs.extend_from_slice(&[(ax >> 4) as u8, ((ax & 0xF) << 4 | ay >> 8) as u8, ay as u8]);
        120 + xy
    } else {
        glyphs.extend_from_slice(&(ax as u16).to_be_bytes());
        glyphs.extend_from_slice(&(ay as u16).to_be_bytes());
        124 + xy
    };
    flags.push(flag as u8 | if on { 0 } else { 0x80 });
}

/// La tabla `glyf` transformada (§5.1). Para probar las dos ramas, la caja va explícita en los
/// glifos impares y se calcula de los puntos en los pares.
fn transform_glyf(glyphs: &[Option<Glyph>], index_format: u16) -> Vec<u8> {
    let n = glyphs.len();
    let (mut n_contours, mut n_points, mut flags, mut glyph_stream) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut bitmap = vec![0u8; 4 * n.div_ceil(32)];
    let (mut bboxes, mut instructions) = (Vec::new(), Vec::new());
    for (i, g) in glyphs.iter().enumerate() {
        let Some(g) = g else {
            n_contours.extend_from_slice(&0i16.to_be_bytes());
            continue;
        };
        n_contours.extend_from_slice(&g.contours.to_be_bytes());
        let mut start = 0u16;
        for &e in &g.end_points {
            u255(&mut n_points, e + 1 - start);
            start = e + 1;
        }
        let (mut px, mut py) = (0, 0);
        for &(x, y, on) in &g.points {
            triplet(x - px, y - py, on, &mut flags, &mut glyph_stream);
            (px, py) = (x, y);
        }
        u255(&mut glyph_stream, g.instructions.len() as u16);
        instructions.extend_from_slice(&g.instructions);
        if i % 2 == 1 {
            bitmap[i >> 3] |= 0x80 >> (i & 7);
            for v in g.bbox {
                bboxes.extend_from_slice(&v.to_be_bytes());
            }
        }
    }
    let mut bbox = bitmap;
    bbox.extend_from_slice(&bboxes);
    let streams = [
        n_contours,
        n_points,
        flags,
        glyph_stream,
        Vec::new(),
        bbox,
        instructions,
    ];
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&(n as u16).to_be_bytes());
    out.extend_from_slice(&index_format.to_be_bytes());
    for s in &streams {
        out.extend_from_slice(&(s.len() as u32).to_be_bytes());
    }
    for s in &streams {
        out.extend_from_slice(s);
    }
    out
}

fn face(sfnt: Vec<u8>) -> bf_font::LoadedFace {
    bf_font::LoadedFace::new(std::sync::Arc::new(sfnt), 0).expect("la fuente rearmada se lee")
}

#[test]
fn woff1_rebuilds_the_same_tables() {
    setup();
    let Some(ttf) = ahem() else { return };
    let (woff, compressed) = woff1(&ttf);
    assert!(
        compressed >= 3,
        "el test debe comprimir varias tablas ({compressed})"
    );
    assert!(woff.len() < ttf.len());
    let sfnt = bf_font::woff::decode(&woff).expect("WOFF 1.0 válido");
    check_sfnt_layout(&sfnt);
    assert_eq!(&sfnt[0..4], &ttf[0..4]);
    assert_eq!(tables(&sfnt), tables(&ttf));
    face(sfnt);
}

fn ahem_parts(ttf: &[u8]) -> (Vec<Table>, Vec<u8>, Vec<u8>) {
    let tabs = tables(ttf);
    let get = |t: &[u8; 4]| tabs.iter().find(|x| &x.0 == t).unwrap().2.clone();
    (tabs.clone(), get(b"glyf"), get(b"loca"))
}

#[test]
fn woff2_null_transform_rebuilds_the_same_tables() {
    setup();
    let Some(ttf) = ahem() else { return };
    let (tabs, _, _) = ahem_parts(&ttf);
    let w2 = tabs
        .iter()
        .map(|(tag, _, data)| W2Table {
            tag: *tag,
            // §5.1: la versión 3 de `glyf`/`loca` es la transformación nula.
            version: if tag == b"glyf" || tag == b"loca" {
                3
            } else {
                0
            },
            orig: data.len(),
            data: data.clone(),
        })
        .collect();
    let woff = woff2(&ttf[0..4], w2, |_| false);
    let sfnt = bf_font::woff::decode(&woff).expect("WOFF 2.0 válido");
    check_sfnt_layout(&sfnt);
    assert_eq!(tables(&sfnt), tables(&ttf));
    face(sfnt);
}

#[test]
fn woff2_transformed_glyf_loca_and_hmtx() {
    setup();
    let Some(ttf) = ahem() else { return };
    let (tabs, glyf, loca) = ahem_parts(&ttf);
    let get = |t: &[u8; 4]| tabs.iter().find(|x| &x.0 == t).unwrap().2.clone();
    let head = get(b"head");
    let index_format = be16(&head, 50);
    let maxp = get(b"maxp");
    let n = usize::from(be16(&maxp, 4));
    let hhea = get(b"hhea");
    let n_metrics = usize::from(be16(&hhea, 34));
    let hmtx = get(b"hmtx");
    let glyphs = read_glyphs(&glyf, &loca, index_format == 1, n);
    // §5.4: se omiten los `leftSideBearing` si todos son el `xMin` del glifo (0 si está vacío).
    let lsb = |i: usize| {
        if i < n_metrics {
            be16(&hmtx, 4 * i + 2) as i16
        } else {
            be16(&hmtx, 4 * n_metrics + 2 * (i - n_metrics)) as i16
        }
    };
    let x_min = |i: usize| glyphs[i].as_ref().map_or(0, |g| g.bbox[0]);
    assert!(
        (0..n).all(|i| lsb(i) == x_min(i)),
        "Ahem debería admitir la transformación"
    );
    let mut thmtx = vec![if n_metrics < n { 3 } else { 1 }];
    for i in 0..n_metrics {
        thmtx.extend_from_slice(&hmtx[4 * i..4 * i + 2]);
    }
    let w2 = tabs
        .iter()
        .map(|(tag, _, data)| match tag {
            b"glyf" => W2Table {
                tag: *tag,
                version: 0,
                orig: data.len(),
                data: transform_glyf(&glyphs, index_format),
            },
            b"loca" => W2Table {
                tag: *tag,
                version: 0,
                orig: data.len(),
                data: Vec::new(),
            },
            b"hmtx" => W2Table {
                tag: *tag,
                version: 1,
                orig: data.len(),
                data: thmtx.clone(),
            },
            _ => W2Table {
                tag: *tag,
                version: 0,
                orig: data.len(),
                data: data.clone(),
            },
        })
        .collect();
    let woff = woff2(&ttf[0..4], w2, |t| {
        (t.version == 0 && (&t.tag == b"glyf" || &t.tag == b"loca")) || (&t.tag == b"hmtx")
    });
    let sfnt = bf_font::woff::decode(&woff).expect("WOFF 2.0 válido");
    check_sfnt_layout(&sfnt);
    let got = tables(&sfnt);
    let want = tables(&ttf);
    assert_eq!(
        got.iter().map(|t| t.0).collect::<Vec<_>>(),
        want.iter().map(|t| t.0).collect::<Vec<_>>()
    );
    for (g, w) in got.iter().zip(&want) {
        if &g.0 == b"head" {
            // `checkSumAdjustment` depende de los bytes de `glyf`, que cambian (§5.3: el
            // rearmado no tiene por qué ser idéntico byte a byte); el resto, igual.
            assert_eq!((g.1, &g.2[..8], &g.2[12..]), (w.1, &w.2[..8], &w.2[12..]));
        } else if &g.0 != b"glyf" && &g.0 != b"loca" {
            assert_eq!(g, w, "{}", String::from_utf8_lossy(&g.0));
        }
    }
    let new_glyf = &got.iter().find(|t| &t.0 == b"glyf").unwrap().2;
    let new_loca = &got.iter().find(|t| &t.0 == b"loca").unwrap().2;
    assert_eq!(new_loca.len(), loca.len());
    let rebuilt = read_glyphs(new_glyf, new_loca, index_format == 1, n);
    for (i, (a, b)) in rebuilt.iter().zip(&glyphs).enumerate() {
        match (a, b) {
            (None, None) => {}
            (Some(a), Some(b)) => {
                assert_eq!(a.contours, b.contours, "glifo {i}");
                assert_eq!(a.bbox, b.bbox, "glifo {i}");
                assert_eq!(a.end_points, b.end_points, "glifo {i}");
                assert_eq!(a.instructions, b.instructions, "glifo {i}");
                assert_eq!(a.points, b.points, "glifo {i}");
            }
            _ => panic!("glifo {i}: vacío en uno y no en el otro"),
        }
    }
    // Y para quien pinta: los mismos contornos.
    let (orig, new) = (face(ttf.clone()), face(sfnt));
    for g in 0..n as u16 {
        assert_eq!(orig.outline(g), new.outline(g), "contorno del glifo {g}");
    }
}

/// Un generador determinista para los bytes cambiados.
fn lcg(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *seed >> 33
}

#[test]
fn damaged_woff_is_rejected_without_panic() {
    setup();
    let Some(ttf) = ahem() else { return };
    let (w1, _) = woff1(&ttf);
    let (tabs, glyf, loca) = ahem_parts(&ttf);
    let head = &tabs.iter().find(|t| &t.0 == b"head").unwrap().2;
    let n = usize::from(be16(&tabs.iter().find(|t| &t.0 == b"maxp").unwrap().2, 4));
    let glyphs = read_glyphs(&glyf, &loca, be16(head, 50) == 1, n);
    let w2 = woff2(
        &ttf[0..4],
        tabs.iter()
            .map(|(tag, _, data)| W2Table {
                tag: *tag,
                version: 0,
                orig: data.len(),
                data: match tag {
                    b"glyf" => transform_glyf(&glyphs, be16(head, 50)),
                    b"loca" => Vec::new(),
                    _ => data.clone(),
                },
            })
            .collect(),
        |t| &t.tag == b"glyf" || &t.tag == b"loca",
    );
    assert!(bf_font::woff::decode(&w2).is_some());
    for file in [&w1, &w2] {
        // Cortado en cualquier punto: rechazado (la longitud de la cabecera ya no cuadra).
        for cut in (0..file.len()).step_by(7) {
            assert!(
                bf_font::woff::decode(&file[..cut]).is_none(),
                "cortado en {cut}"
            );
        }
        // Con bytes cambiados: lo que sea, sin pánico.
        let mut seed = 7;
        for _ in 0..2000 {
            let mut d = file.clone();
            for _ in 0..1 + lcg(&mut seed) % 4 {
                let at = (lcg(&mut seed) as usize) % d.len();
                d[at] = lcg(&mut seed) as u8;
            }
            if let Some(sfnt) = bf_font::woff::decode(&d) {
                let _ = bf_font::LoadedFace::new(std::sync::Arc::new(sfnt), 0);
            }
        }
    }
    // Un WOFF que anuncia tablas descomunales: rechazado sin reservar esa memoria.
    let mut big = w1.clone();
    big[44 + 12..44 + 16].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(bf_font::woff::decode(&big).is_none());
}

/// Las fuentes WOFF 2.0 de WPT (con `glyf`/`loca` transformadas por un codificador real) se
/// rearman y se leen: cada glifo con datos tiene contorno.
#[test]
fn wpt_woff2_fonts_decode() {
    setup();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/wpt");
    let files = [
        "fonts/noto/NotoSansMongolian-regular.woff2",
        "fonts/noto/NotoNaskhArabic-regular.woff2",
        "fonts/hasubi-mono/HasubiMono-Regular.woff2",
        "css/css-fonts/variations/resources/FontStyleTest-slnt-VF.woff2",
        "fonts/noto/noto-sans-v8-latin-regular.woff",
        "fonts/Revalia.woff",
    ];
    for f in files {
        let Ok(data) = std::fs::read(root.join(f)) else {
            eprintln!("aviso: sin {f}, omitida");
            continue;
        };
        let sfnt = bf_font::woff::decode(&data).unwrap_or_else(|| panic!("{f}"));
        check_sfnt_layout(&sfnt);
        let lf = face(sfnt);
        let face = lf.face().expect(f);
        let n = face.number_of_glyphs();
        assert!(n > 10, "{f}");
        let mut with_outline = 0;
        for g in 0..n {
            if lf.outline(g).is_some() {
                with_outline += 1;
            }
        }
        assert!(
            with_outline > n / 2,
            "{f}: {with_outline} de {n} glifos con contorno"
        );
    }
}
