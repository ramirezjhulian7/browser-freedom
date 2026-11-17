//! E2: WOFF 1.0 y WOFF 2.0 (W3C, "WOFF File Format"): de vuelta a un sfnt (TrueType u
//! OpenType) que `LoadedFace` sabe leer.
//!
//! La descompresión (zlib para WOFF 1.0, Brotli para WOFF 2.0) llega de fuera con
//! [`set_decompressors`]: `bf-font` no depende de esas bibliotecas y quien ya las tiene (el
//! navegador, por `bf-net`) las registra. Sin descompresor registrado, una fuente comprimida se
//! descarta como antes.
//!
//! - WOFF 1.0 (§3 a §5): cabecera, directorio de tablas, cada tabla comprimida con zlib o sin
//!   comprimir.
//! - WOFF 2.0: cabecera y directorio con etiquetas conocidas y `UIntBase128` (§3 y §4), un solo
//!   flujo comprimido con Brotli (§5), la transformación de `glyf`/`loca` (§5.1 a §5.3) y la de
//!   `hmtx` (§5.4), y las colecciones (`ttcf`, §4.2), que dan una colección OpenType.
//!
//! El sfnt se rearma con las tablas en orden de etiqueta, cada una alineada a 4 bytes con
//! relleno de ceros, y sus sumas de comprobación (OpenType, "Calculating checksums").
//! Cualquier dato inconsistente (desplazamientos fuera del archivo, tamaños imposibles) devuelve
//! `None`, nunca un pánico.

use std::sync::OnceLock;

/// Un descompresor: los bytes comprimidos y el tamaño esperado del resultado.
pub type Decompressor = fn(&[u8], usize) -> Option<Vec<u8>>;

static ZLIB: OnceLock<Decompressor> = OnceLock::new();
static BROTLI: OnceLock<Decompressor> = OnceLock::new();

/// Registra los descompresores de zlib (WOFF 1.0) y Brotli (WOFF 2.0). La primera vez vale; las
/// siguientes no cambian nada.
pub fn set_decompressors(zlib: Decompressor, brotli: Decompressor) {
    let _ = ZLIB.set(zlib);
    let _ = BROTLI.set(brotli);
}

/// ¿Hay descompresores registrados (se pueden usar WOFF y WOFF2)?
pub fn supported() -> bool {
    ZLIB.get().is_some() && BROTLI.get().is_some()
}

/// Límite de tamaño de una fuente descomprimida (protege de archivos que piden memoria absurda).
const MAX_SFNT: usize = 64 << 20;

/// Si `data` es un WOFF o un WOFF2, el sfnt que contiene con los descompresores registrados;
/// `None` si no es ninguno de los dos, si no hay descompresor o si está dañado.
pub fn decode(data: &[u8]) -> Option<Vec<u8>> {
    match data.get(0..4)? {
        b"wOFF" => decode_woff(data, *ZLIB.get()?),
        b"wOF2" => decode_woff2(data, *BROTLI.get()?),
        _ => None,
    }
}

// --- Lectura ---

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Reader<'a> {
        Reader { data, pos: 0 }
    }
    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let b = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(b)
    }
    fn u8(&mut self) -> Option<u8> {
        self.bytes(1).map(|b| b[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.bytes(2).map(|b| u16::from_be_bytes([b[0], b[1]]))
    }
    fn i16(&mut self) -> Option<i16> {
        self.u16().map(|v| v as i16)
    }
    fn u32(&mut self) -> Option<u32> {
        self.bytes(4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    /// `UIntBase128` (WOFF 2.0 §3): hasta 5 bytes, sin ceros iniciales ni desbordamiento.
    fn base128(&mut self) -> Option<u32> {
        let mut acc: u32 = 0;
        for i in 0..5 {
            let b = self.u8()?;
            if i == 0 && b == 0x80 {
                return None;
            }
            if acc & 0xFE00_0000 != 0 {
                return None;
            }
            acc = (acc << 7) | u32::from(b & 0x7F);
            if b & 0x80 == 0 {
                return Some(acc);
            }
        }
        None
    }
    /// `255UInt16` (WOFF 2.0 §3).
    fn u255(&mut self) -> Option<u16> {
        match self.u8()? {
            253 => self.u16(),
            255 => Some(u16::from(self.u8()?) + 253),
            254 => Some(u16::from(self.u8()?) + 506),
            c => Some(u16::from(c)),
        }
    }
}

// --- sfnt ---

/// La suma de comprobación de una tabla (OpenType: suma de `u32` big-endian, con el final
/// rellenado con ceros).
fn checksum(data: &[u8]) -> u32 {
    let mut sum: u32 = 0;
    for chunk in data.chunks(4) {
        let mut w = [0u8; 4];
        w[..chunk.len()].copy_from_slice(chunk);
        sum = sum.wrapping_add(u32::from_be_bytes(w));
    }
    sum
}

/// La suma de una tabla para el directorio. La de `head` se calcula con `checkSumAdjustment`
/// (bytes 8 a 11) a cero (OpenType, `head`).
fn table_checksum(tag: &[u8; 4], data: &[u8]) -> u32 {
    let sum = checksum(data);
    match data.get(8..12) {
        Some(adj) if tag == b"head" => {
            sum.wrapping_sub(u32::from_be_bytes([adj[0], adj[1], adj[2], adj[3]]))
        }
        _ => sum,
    }
}

/// La cabecera de un sfnt (OpenType, "Table Directory"): versión, `numTables`, `searchRange`,
/// `entrySelector` y `rangeShift`.
fn sfnt_header(out: &mut Vec<u8>, flavor: u32, n: usize) -> Option<()> {
    let n = u16::try_from(n).ok()?;
    let mut entry_selector: u16 = 0;
    while (1u32 << (entry_selector + 1)) <= u32::from(n) {
        entry_selector += 1;
    }
    let search_range = (1u16 << entry_selector).wrapping_mul(16);
    let range_shift = n.wrapping_mul(16).wrapping_sub(search_range);
    out.extend_from_slice(&flavor.to_be_bytes());
    for v in [n, search_range, entry_selector, range_shift] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    Some(())
}

/// Una entrada del directorio de tablas.
fn sfnt_entry(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8], offset: usize) -> Option<()> {
    out.extend_from_slice(tag);
    out.extend_from_slice(&table_checksum(tag, data).to_be_bytes());
    out.extend_from_slice(&u32::try_from(offset).ok()?.to_be_bytes());
    out.extend_from_slice(&u32::try_from(data.len()).ok()?.to_be_bytes());
    Some(())
}

/// Los datos de las tablas, cada una alineada a 4 bytes con ceros.
fn push_padded(out: &mut Vec<u8>, data: &[u8]) {
    out.extend_from_slice(data);
    out.resize(out.len().div_ceil(4) * 4, 0);
}

/// ¿Etiquetas en orden estricto (sin repetidas)?
fn sorted_unique<'a>(mut tags: impl Iterator<Item = &'a [u8; 4]>) -> bool {
    let Some(mut prev) = tags.next() else {
        return true;
    };
    tags.all(|t| {
        let ok = prev < t;
        prev = t;
        ok
    })
}

/// Un sfnt con `tables` (etiqueta, datos), en orden de etiqueta y alineadas a 4 bytes, con
/// `head.checkSumAdjustment` calculado (0xB1B0AFBA menos la suma de toda la fuente).
fn build_sfnt(flavor: u32, mut tables: Vec<([u8; 4], Vec<u8>)>) -> Option<Vec<u8>> {
    tables.sort_by_key(|t| t.0);
    if !sorted_unique(tables.iter().map(|t| &t.0)) {
        return None;
    }
    let mut out = Vec::new();
    sfnt_header(&mut out, flavor, tables.len())?;
    let mut offset = 12 + 16 * tables.len();
    let mut head_at = None;
    for (tag, data) in &tables {
        if tag == b"head" {
            head_at = Some(offset);
        }
        sfnt_entry(&mut out, tag, data, offset)?;
        offset = offset.checked_add(data.len().div_ceil(4) * 4)?;
        if offset > MAX_SFNT {
            return None;
        }
    }
    for (_, data) in &tables {
        push_padded(&mut out, data);
    }
    if let Some(at) = head_at {
        out.get_mut(at + 8..at + 12)?.fill(0);
        let adjustment = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
        out.get_mut(at + 8..at + 12)?
            .copy_from_slice(&adjustment.to_be_bytes());
    }
    Some(out)
}

/// Una colección (`ttcf`, OpenType "Font Collections", versión 1.0): un directorio por fuente
/// (`fonts`: su versión sfnt y los índices de sus tablas) y cada tabla una sola vez, aunque la
/// compartan varias fuentes.
fn build_ttc(fonts: &[(u32, Vec<usize>)], tables: &[([u8; 4], Vec<u8>)]) -> Option<Vec<u8>> {
    let header = 12 + 4 * fonts.len();
    let dirs: usize = fonts.iter().map(|(_, idx)| 12 + 16 * idx.len()).sum();
    let mut used = vec![false; tables.len()];
    for (_, idx) in fonts {
        for &i in idx {
            *used.get_mut(i)? = true;
        }
    }
    let mut table_at = vec![0usize; tables.len()];
    let mut offset = header + dirs;
    for (i, (_, data)) in tables.iter().enumerate() {
        if used[i] {
            table_at[i] = offset;
            offset = offset.checked_add(data.len().div_ceil(4) * 4)?;
            if offset > MAX_SFNT {
                return None;
            }
        }
    }
    let mut out = Vec::with_capacity(offset);
    out.extend_from_slice(b"ttcf");
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    out.extend_from_slice(&u32::try_from(fonts.len()).ok()?.to_be_bytes());
    let mut dir_at = header;
    for (_, idx) in fonts {
        out.extend_from_slice(&u32::try_from(dir_at).ok()?.to_be_bytes());
        dir_at += 12 + 16 * idx.len();
    }
    for (flavor, idx) in fonts {
        let mut idx = idx.clone();
        idx.sort_by_key(|&i| tables[i].0);
        if !sorted_unique(idx.iter().map(|&i| &tables[i].0)) {
            return None;
        }
        sfnt_header(&mut out, *flavor, idx.len())?;
        for &i in &idx {
            sfnt_entry(&mut out, &tables[i].0, &tables[i].1, table_at[i])?;
        }
    }
    for (i, (_, data)) in tables.iter().enumerate() {
        if used[i] {
            push_padded(&mut out, data);
        }
    }
    Some(out)
}

// --- WOFF 1.0 ---

/// WOFF 1.0 (§3 a §5).
pub fn decode_woff(data: &[u8], zlib: Decompressor) -> Option<Vec<u8>> {
    let mut r = Reader::new(data);
    if r.bytes(4)? != b"wOFF" {
        return None;
    }
    let flavor = r.u32()?;
    let length = r.u32()? as usize;
    let num_tables = r.u16()?;
    let _reserved = r.u16()?;
    let total_sfnt = r.u32()? as usize;
    if length != data.len() || num_tables == 0 || total_sfnt > MAX_SFNT {
        return None;
    }
    r.bytes(24)?; // versiones, metadatos y datos privados: no hacen falta para el sfnt.
    let mut tables = Vec::with_capacity(usize::from(num_tables));
    for _ in 0..num_tables {
        let tag: [u8; 4] = r.bytes(4)?.try_into().ok()?;
        let offset = r.u32()? as usize;
        let comp = r.u32()? as usize;
        let orig = r.u32()? as usize;
        let _orig_checksum = r.u32()?;
        if orig > MAX_SFNT || comp > orig {
            return None;
        }
        let raw = data.get(offset..offset.checked_add(comp)?)?;
        // §5: si `compLength` es menor que `origLength`, la tabla va comprimida con zlib.
        let table = if comp < orig {
            let t = zlib(raw, orig)?;
            if t.len() != orig {
                return None;
            }
            t
        } else {
            raw.to_vec()
        };
        tables.push((tag, table));
    }
    build_sfnt(flavor, tables)
}

// --- WOFF 2.0 ---

/// Las etiquetas conocidas del directorio de WOFF 2.0 (§4.1, "Known Table Tags").
const KNOWN_TAGS: [&[u8; 4]; 63] = [
    b"cmap", b"head", b"hhea", b"hmtx", b"maxp", b"name", b"OS/2", b"post", b"cvt ", b"fpgm",
    b"glyf", b"loca", b"prep", b"CFF ", b"VORG", b"EBDT", b"EBLC", b"gasp", b"hdmx", b"kern",
    b"LTSH", b"PCLT", b"VDMX", b"vhea", b"vmtx", b"BASE", b"GDEF", b"GPOS", b"GSUB", b"EBSC",
    b"JSTF", b"MATH", b"CBDT", b"CBLC", b"COLR", b"CPAL", b"SVG ", b"sbix", b"acnt", b"avar",
    b"bdat", b"bloc", b"bsln", b"cvar", b"fdsc", b"feat", b"fmtx", b"fvar", b"gvar", b"hsty",
    b"just", b"lcar", b"mort", b"morx", b"opbd", b"prop", b"trak", b"Zapf", b"Silf", b"Glat",
    b"Gloc", b"Feat", b"Sill",
];

struct Entry {
    tag: [u8; 4],
    transformed: bool,
    orig: usize,
    /// Lo que ocupa la tabla en el flujo descomprimido.
    stored: usize,
}

/// WOFF 2.0 (§3 a §5).
pub fn decode_woff2(data: &[u8], brotli: Decompressor) -> Option<Vec<u8>> {
    let mut r = Reader::new(data);
    if r.bytes(4)? != b"wOF2" {
        return None;
    }
    let flavor = r.u32()?;
    let length = r.u32()? as usize;
    let num_tables = r.u16()?;
    let _reserved = r.u16()?;
    let total_sfnt = r.u32()? as usize;
    let total_compressed = r.u32()? as usize;
    r.bytes(24)?; // versiones, metadatos y datos privados.
    if length != data.len() || num_tables == 0 || total_sfnt > MAX_SFNT {
        return None;
    }
    let collection = flavor == u32::from_be_bytes(*b"ttcf");
    let mut entries = Vec::with_capacity(usize::from(num_tables));
    for _ in 0..num_tables {
        let flags = r.u8()?;
        let tag = match flags & 0x3F {
            63 => r.bytes(4)?.try_into().ok()?,
            i => *KNOWN_TAGS[usize::from(i)],
        };
        let version = flags >> 6;
        let orig = r.base128()? as usize;
        // §4.1: `glyf`/`loca` se transforman con la versión 0 (la 3 es la transformación nula);
        // las demás tablas, con cualquier versión distinta de 0 (solo `hmtx` tiene la 1).
        let transformed = if &tag == b"glyf" || &tag == b"loca" {
            version == 0
        } else {
            version != 0
        };
        let stored = if transformed {
            r.base128()? as usize
        } else {
            orig
        };
        if orig > MAX_SFNT || stored > MAX_SFNT {
            return None;
        }
        entries.push(Entry {
            tag,
            transformed,
            orig,
            stored,
        });
    }
    // §4.2: en una colección, tras el directorio va `CollectionHeader`: la versión, el número
    // de fuentes y, de cada una, su número de tablas, su versión sfnt y los índices (en el
    // directorio) de sus tablas. Sin colección, una sola fuente con todas las tablas.
    let fonts: Vec<(u32, Vec<usize>)> = if collection {
        let version = r.u32()?;
        if version != 0x0001_0000 && version != 0x0002_0000 {
            return None;
        }
        let num_fonts = r.u255()?;
        if num_fonts == 0 {
            return None;
        }
        let mut fonts = Vec::with_capacity(usize::from(num_fonts));
        for _ in 0..num_fonts {
            let n = r.u255()?;
            let font_flavor = r.u32()?;
            let mut idx = Vec::with_capacity(usize::from(n));
            for _ in 0..n {
                let i = usize::from(r.u255()?);
                if i >= entries.len() {
                    return None;
                }
                idx.push(i);
            }
            fonts.push((font_flavor, idx));
        }
        fonts
    } else {
        vec![(flavor, (0..entries.len()).collect())]
    };
    let compressed = r.bytes(total_compressed)?;
    let expected: usize = entries.iter().map(|e| e.stored).sum();
    if expected > MAX_SFNT {
        return None;
    }
    let stream = brotli(compressed, expected)?;
    // §5: el flujo descomprimido mide justo la suma de las tablas.
    if stream.len() != expected {
        return None;
    }
    let mut pos = 0;
    let mut raw: Vec<&[u8]> = Vec::with_capacity(entries.len());
    for e in &entries {
        raw.push(stream.get(pos..pos + e.stored)?);
        pos += e.stored;
    }
    let mut built: Vec<Option<Vec<u8>>> = vec![None; entries.len()];
    let mut x_mins: Vec<Option<Vec<Option<i16>>>> = vec![None; entries.len()];
    for (_, idx) in &fonts {
        let find = |tag: &[u8; 4]| idx.iter().copied().find(|&i| &entries[i].tag == tag);
        let glyf = find(b"glyf");
        let loca = find(b"loca");
        // `glyf` y `loca` se transforman las dos o ninguna (§5.1); en una colección, la `loca`
        // de una `glyf` transformada es la entrada siguiente del directorio (§4.2).
        let glyf_transformed = glyf.is_some_and(|g| entries[g].transformed);
        if glyf_transformed != loca.is_some_and(|l| entries[l].transformed) {
            return None;
        }
        if let (true, Some(g), Some(l)) = (glyf_transformed, glyf, loca) {
            if collection && l != g + 1 {
                return None;
            }
            if built[g].is_none() {
                let rebuilt = rebuild_glyf(raw[g])?;
                if rebuilt.loca.len() != entries[l].orig || entries[l].stored != 0 {
                    return None;
                }
                built[g] = Some(rebuilt.glyf);
                built[l] = Some(rebuilt.loca);
                x_mins[g] = Some(rebuilt.x_mins);
            }
        }
        for &i in idx {
            if built[i].is_some() {
                continue;
            }
            let e = &entries[i];
            if &e.tag == b"hmtx" && e.transformed {
                let hhea = raw[find(b"hhea")?];
                let maxp = raw[find(b"maxp")?];
                let glyph_x_mins = x_mins[glyf?].as_ref()?;
                // Como `glyf` (§5.1, "MUST NOT reject" si no cuadra con `origLength`), la tabla
                // rearmada vale aunque mida distinto de lo anunciado; `loca`, en cambio, sí
                // tiene que cuadrar (§5.3).
                built[i] = Some(rebuild_hmtx(raw[i], hhea, maxp, glyph_x_mins)?);
            } else if e.transformed {
                // Una transformación desconocida.
                return None;
            } else {
                built[i] = Some(raw[i].to_vec());
            }
        }
    }
    let tables: Vec<([u8; 4], Vec<u8>)> = entries
        .iter()
        .zip(built)
        .map(|(e, t)| (e.tag, t.unwrap_or_default()))
        .collect();
    if collection {
        build_ttc(&fonts, &tables)
    } else {
        build_sfnt(flavor, tables)
    }
}

struct RebuiltGlyf {
    glyf: Vec<u8>,
    loca: Vec<u8>,
    /// El `xMin` de cada glifo (`None` para los vacíos), para `hmtx` (§5.4).
    x_mins: Vec<Option<i16>>,
}

// Banderas de los glifos simples de TrueType (`glyf`, "Simple Glyph Description").
const ON_CURVE: u8 = 0x01;
const X_SHORT: u8 = 0x02;
const Y_SHORT: u8 = 0x04;
const X_SAME_OR_POSITIVE: u8 = 0x10;
const Y_SAME_OR_POSITIVE: u8 = 0x20;
const OVERLAP_SIMPLE: u8 = 0x40;

// Banderas de los componentes de los glifos compuestos.
const ARG_1_AND_2_ARE_WORDS: u16 = 0x0001;
const WE_HAVE_A_SCALE: u16 = 0x0008;
const MORE_COMPONENTS: u16 = 0x0020;
const WE_HAVE_AN_X_AND_Y_SCALE: u16 = 0x0040;
const WE_HAVE_A_TWO_BY_TWO: u16 = 0x0080;
const WE_HAVE_INSTRUCTIONS: u16 = 0x0100;

/// Un desplazamiento de un punto según su bandera (§5.2, "Decoding of variable-length X and Y
/// coordinates").
fn triplet(flag: u8, glyphs: &mut Reader<'_>) -> Option<(i32, i32)> {
    let with_sign = |f: u8, v: i32| if f & 1 != 0 { v } else { -v };
    let f = flag & 0x7F;
    Some(if f < 10 {
        let b = i32::from(glyphs.u8()?);
        (0, with_sign(f, (i32::from(f & 14) << 7) + b))
    } else if f < 20 {
        let b = i32::from(glyphs.u8()?);
        (with_sign(f, (i32::from((f - 10) & 14) << 7) + b), 0)
    } else if f < 84 {
        let b0 = i32::from(f - 20);
        let b1 = i32::from(glyphs.u8()?);
        (
            with_sign(f, 1 + (b0 & 0x30) + (b1 >> 4)),
            with_sign(f >> 1, 1 + ((b0 & 0x0C) << 2) + (b1 & 0x0F)),
        )
    } else if f < 120 {
        let b0 = i32::from(f - 84);
        let b = glyphs.bytes(2)?;
        (
            with_sign(f, 1 + ((b0 / 12) << 8) + i32::from(b[0])),
            with_sign(f >> 1, 1 + (((b0 % 12) >> 2) << 8) + i32::from(b[1])),
        )
    } else if f < 124 {
        let b = glyphs.bytes(3)?;
        let b2 = i32::from(b[1]);
        (
            with_sign(f, (i32::from(b[0]) << 4) + (b2 >> 4)),
            with_sign(f >> 1, ((b2 & 0x0F) << 8) + i32::from(b[2])),
        )
    } else {
        let b = glyphs.bytes(4)?;
        (
            with_sign(f, (i32::from(b[0]) << 8) + i32::from(b[1])),
            with_sign(f >> 1, (i32::from(b[2]) << 8) + i32::from(b[3])),
        )
    })
}

fn push_i16(out: &mut Vec<u8>, v: i16) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn push_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// La tabla `glyf` transformada (§5.1) de vuelta a `glyf` y `loca` (§5.3).
fn rebuild_glyf(data: &[u8]) -> Option<RebuiltGlyf> {
    let mut h = Reader::new(data);
    let _reserved = h.u16()?;
    let option_flags = h.u16()?;
    let num_glyphs = usize::from(h.u16()?);
    let index_format = h.u16()?;
    let mut sizes = [0usize; 7];
    for s in &mut sizes {
        *s = h.u32()? as usize;
    }
    let mut streams = Vec::with_capacity(7);
    for s in sizes {
        streams.push(h.bytes(s)?);
    }
    let overlap = if option_flags & 1 != 0 {
        Some(h.bytes(num_glyphs.div_ceil(8))?)
    } else {
        None
    };
    let mut n_contours = Reader::new(streams[0]);
    let mut n_points = Reader::new(streams[1]);
    let mut flags = Reader::new(streams[2]);
    let mut glyphs = Reader::new(streams[3]);
    let mut composite = Reader::new(streams[4]);
    let bbox_stream = streams[5];
    let bitmap_len = 4 * num_glyphs.div_ceil(32);
    let bbox_bitmap = bbox_stream.get(..bitmap_len)?;
    let mut bbox = Reader::new(bbox_stream.get(bitmap_len..)?);
    let mut instructions = Reader::new(streams[6]);
    let has_bbox = |i: usize| bbox_bitmap[i >> 3] & (0x80 >> (i & 7)) != 0;

    let mut glyf = Vec::new();
    let mut offsets = Vec::with_capacity(num_glyphs + 1);
    let mut x_mins = Vec::with_capacity(num_glyphs);
    for i in 0..num_glyphs {
        offsets.push(glyf.len());
        let contours = n_contours.i16()?;
        if contours == 0 {
            // Vacío: sin datos (y sin caja explícita).
            if has_bbox(i) {
                return None;
            }
            x_mins.push(None);
            continue;
        }
        if contours < 0 {
            // Compuesto: los componentes tal cual y la caja explícita.
            if !has_bbox(i) {
                return None;
            }
            let start = composite.pos;
            let mut has_instructions = false;
            loop {
                let f = composite.u16()?;
                let _glyph = composite.u16()?;
                let mut n = if f & ARG_1_AND_2_ARE_WORDS != 0 { 4 } else { 2 };
                if f & WE_HAVE_A_SCALE != 0 {
                    n += 2;
                } else if f & WE_HAVE_AN_X_AND_Y_SCALE != 0 {
                    n += 4;
                } else if f & WE_HAVE_A_TWO_BY_TWO != 0 {
                    n += 8;
                }
                composite.bytes(n)?;
                has_instructions |= f & WE_HAVE_INSTRUCTIONS != 0;
                if f & MORE_COMPONENTS == 0 {
                    break;
                }
            }
            let components = &composite.data[start..composite.pos];
            push_i16(&mut glyf, -1);
            let b = bbox.bytes(8)?;
            x_mins.push(Some(i16::from_be_bytes([b[0], b[1]])));
            glyf.extend_from_slice(b);
            glyf.extend_from_slice(components);
            if has_instructions {
                let len = glyphs.u255()?;
                push_u16(&mut glyf, len);
                glyf.extend_from_slice(instructions.bytes(usize::from(len))?);
            }
        } else {
            // Simple.
            let contours = contours as usize;
            let mut end_points = Vec::with_capacity(contours);
            let mut total: usize = 0;
            for _ in 0..contours {
                total = total.checked_add(usize::from(n_points.u255()?))?;
                end_points.push(u16::try_from(total.checked_sub(1)?).ok()?);
            }
            let point_flags = flags.bytes(total)?;
            let mut points = Vec::with_capacity(total);
            let (mut x, mut y) = (0i32, 0i32);
            for &f in point_flags {
                let (dx, dy) = triplet(f, &mut glyphs)?;
                x += dx;
                y += dy;
                points.push((x, y, f & 0x80 == 0));
            }
            let instruction_len = glyphs.u255()?;
            let code = instructions.bytes(usize::from(instruction_len))?;
            let (x_min, y_min, x_max, y_max) = if has_bbox(i) {
                let b = bbox.bytes(8)?;
                let v = |k: usize| i32::from(i16::from_be_bytes([b[k], b[k + 1]]));
                (v(0), v(2), v(4), v(6))
            } else {
                points.iter().fold(
                    (i32::MAX, i32::MAX, i32::MIN, i32::MIN),
                    |(a, b, c, d), &(x, y, _)| (a.min(x), b.min(y), c.max(x), d.max(y)),
                )
            };
            let clamp = |v: i32| i16::try_from(v).ok();
            let (x_min, y_min, x_max, y_max) =
                (clamp(x_min)?, clamp(y_min)?, clamp(x_max)?, clamp(y_max)?);
            x_mins.push(Some(x_min));
            push_i16(&mut glyf, i16::try_from(contours).ok()?);
            for v in [x_min, y_min, x_max, y_max] {
                push_i16(&mut glyf, v);
            }
            for e in &end_points {
                push_u16(&mut glyf, *e);
            }
            push_u16(&mut glyf, instruction_len);
            glyf.extend_from_slice(code);
            // Banderas y coordenadas en el formato de TrueType (sin repeticiones).
            let overlap_first = overlap.is_some_and(|o| o[i >> 3] & (0x80 >> (i & 7)) != 0);
            let mut xs = Vec::new();
            let mut ys = Vec::new();
            let (mut px, mut py) = (0i32, 0i32);
            for (k, &(x, y, on)) in points.iter().enumerate() {
                let mut f = if on { ON_CURVE } else { 0 };
                if k == 0 && overlap_first {
                    f |= OVERLAP_SIMPLE;
                }
                let (dx, dy) = (x - px, y - py);
                (px, py) = (x, y);
                if dx == 0 {
                    f |= X_SAME_OR_POSITIVE;
                } else if (-255..=255).contains(&dx) {
                    f |= X_SHORT;
                    if dx > 0 {
                        f |= X_SAME_OR_POSITIVE;
                    }
                    xs.push(dx.unsigned_abs() as u8);
                } else {
                    xs.extend_from_slice(&i16::try_from(dx).ok()?.to_be_bytes());
                }
                if dy == 0 {
                    f |= Y_SAME_OR_POSITIVE;
                } else if (-255..=255).contains(&dy) {
                    f |= Y_SHORT;
                    if dy > 0 {
                        f |= Y_SAME_OR_POSITIVE;
                    }
                    ys.push(dy.unsigned_abs() as u8);
                } else {
                    ys.extend_from_slice(&i16::try_from(dy).ok()?.to_be_bytes());
                }
                glyf.push(f);
            }
            glyf.extend_from_slice(&xs);
            glyf.extend_from_slice(&ys);
        }
        // Cada glifo alineado a 4 bytes (vale para `loca` corta y larga).
        glyf.resize(glyf.len().div_ceil(4) * 4, 0);
        if glyf.len() > MAX_SFNT {
            return None;
        }
    }
    offsets.push(glyf.len());
    let mut loca = Vec::with_capacity(offsets.len() * 4);
    for o in offsets {
        if index_format == 0 {
            push_u16(&mut loca, u16::try_from(o / 2).ok()?);
        } else {
            loca.extend_from_slice(&u32::try_from(o).ok()?.to_be_bytes());
        }
    }
    Some(RebuiltGlyf { glyf, loca, x_mins })
}

/// La tabla `hmtx` transformada (§5.4) de vuelta a `hmtx`: los `leftSideBearing` que faltan
/// son el `xMin` de cada glifo (0 en los vacíos).
fn rebuild_hmtx(data: &[u8], hhea: &[u8], maxp: &[u8], x_mins: &[Option<i16>]) -> Option<Vec<u8>> {
    let n_metrics = usize::from(u16::from_be_bytes(hhea.get(34..36)?.try_into().ok()?));
    let num_glyphs = usize::from(u16::from_be_bytes(maxp.get(4..6)?.try_into().ok()?));
    if n_metrics == 0 || n_metrics > num_glyphs || x_mins.len() != num_glyphs {
        return None;
    }
    let mut r = Reader::new(data);
    let flags = r.u8()?;
    // §5.4: los bits 2 a 7 están reservados y alguno de los dos primeros tiene que estar (si
    // no, la tabla no debería ir transformada).
    if flags & 0xFC != 0 || flags & 3 == 0 {
        return None;
    }
    let mut advances = Vec::with_capacity(n_metrics);
    for _ in 0..n_metrics {
        advances.push(r.u16()?);
    }
    let lsb = |i: usize, r: &mut Reader<'_>, omitted: bool| -> Option<i16> {
        if omitted {
            Some(x_mins[i].unwrap_or(0))
        } else {
            r.i16()
        }
    };
    let mut out = Vec::with_capacity(4 * n_metrics + 2 * (num_glyphs - n_metrics));
    let mut lsbs = Vec::with_capacity(num_glyphs);
    for i in 0..n_metrics {
        lsbs.push(lsb(i, &mut r, flags & 1 != 0)?);
    }
    for i in n_metrics..num_glyphs {
        lsbs.push(lsb(i, &mut r, flags & 2 != 0)?);
    }
    for i in 0..n_metrics {
        push_u16(&mut out, advances[i]);
        push_i16(&mut out, lsbs[i]);
    }
    for v in &lsbs[n_metrics..] {
        push_i16(&mut out, *v);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base128() {
        // WOFF 2.0 §3: los ejemplos y los casos que hay que rechazar.
        let read = |b: &[u8]| Reader::new(b).base128();
        assert_eq!(read(&[0x3F]), Some(63));
        assert_eq!(read(&[0x81, 0x00]), Some(128));
        assert_eq!(read(&[0x8F, 0xFF, 0xFF, 0xFF, 0x7F]), Some(u32::MAX));
        assert_eq!(read(&[0x80, 0x01]), None, "cero inicial");
        assert_eq!(read(&[0x90, 0x80, 0x80, 0x80, 0x00]), None, "desborda");
        assert_eq!(
            read(&[0x81, 0x81, 0x81, 0x81, 0x81, 0x01]),
            None,
            "más de 5 bytes"
        );
        assert_eq!(read(&[0x81]), None, "cortado");
    }

    #[test]
    fn uint255() {
        let read = |b: &[u8]| Reader::new(b).u255();
        assert_eq!(read(&[252]), Some(252));
        assert_eq!(read(&[255, 0]), Some(253));
        assert_eq!(read(&[254, 0]), Some(506));
        assert_eq!(read(&[253, 0x01, 0x2C]), Some(300));
    }

    #[test]
    fn without_decompressor_or_magic_nothing() {
        assert_eq!(decode(b"OTTO\0\0\0\0"), None);
        assert_eq!(decode(b"wO"), None);
        let never: Decompressor = |_, _| None;
        assert_eq!(decode_woff(b"wOFF", never), None);
        assert_eq!(decode_woff2(b"wOF2", never), None);
    }
}
