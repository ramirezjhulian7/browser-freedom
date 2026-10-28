//! HPACK (RFC 7541): compresión de cabeceras de HTTP/2. Codificador y decodificador con la
//! tabla estática (apéndice A), la tabla dinámica (§2.3.2, §4) y el código de Huffman
//! (§5.2, apéndice B).

use std::collections::VecDeque;
use std::sync::OnceLock;

/// Un error de compresión (RFC 9113 §4.3: es un `COMPRESSION_ERROR` de la conexión).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HpackError(pub &'static str);

impl std::fmt::Display for HpackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HPACK: {}", self.0)
    }
}

impl std::error::Error for HpackError {}

/// Un campo: nombre y valor como bytes (los valores de HTTP no son siempre UTF-8).
pub type Field = (Vec<u8>, Vec<u8>);

/// Apéndice A: la tabla estática (índices 1 a 61).
pub const STATIC_TABLE: [(&str, &str); 61] = [
    (":authority", ""),
    (":method", "GET"),
    (":method", "POST"),
    (":path", "/"),
    (":path", "/index.html"),
    (":scheme", "http"),
    (":scheme", "https"),
    (":status", "200"),
    (":status", "204"),
    (":status", "206"),
    (":status", "304"),
    (":status", "400"),
    (":status", "404"),
    (":status", "500"),
    ("accept-charset", ""),
    ("accept-encoding", "gzip, deflate"),
    ("accept-language", ""),
    ("accept-ranges", ""),
    ("accept", ""),
    ("access-control-allow-origin", ""),
    ("age", ""),
    ("allow", ""),
    ("authorization", ""),
    ("cache-control", ""),
    ("content-disposition", ""),
    ("content-encoding", ""),
    ("content-language", ""),
    ("content-length", ""),
    ("content-location", ""),
    ("content-range", ""),
    ("content-type", ""),
    ("cookie", ""),
    ("date", ""),
    ("etag", ""),
    ("expect", ""),
    ("expires", ""),
    ("from", ""),
    ("host", ""),
    ("if-match", ""),
    ("if-modified-since", ""),
    ("if-none-match", ""),
    ("if-range", ""),
    ("if-unmodified-since", ""),
    ("last-modified", ""),
    ("link", ""),
    ("location", ""),
    ("max-forwards", ""),
    ("proxy-authenticate", ""),
    ("proxy-authorization", ""),
    ("range", ""),
    ("referer", ""),
    ("refresh", ""),
    ("retry-after", ""),
    ("server", ""),
    ("set-cookie", ""),
    ("strict-transport-security", ""),
    ("transfer-encoding", ""),
    ("user-agent", ""),
    ("vary", ""),
    ("via", ""),
    ("www-authenticate", ""),
];

/// Apéndice B: la longitud en bits del código de cada símbolo (0–255 y EOS = 256). El código
/// es canónico (dentro de cada longitud, los códigos crecen con el símbolo), así que los
/// códigos salen de las longitudes.
const HUFFMAN_LENGTHS: [u8; 257] = [
    13, 23, 28, 28, 28, 28, 28, 28, 28, 24, 30, 28, 28, 30, 28, 28, // 0
    28, 28, 28, 28, 28, 28, 30, 28, 28, 28, 28, 28, 28, 28, 28, 28, // 16
    6, 10, 10, 12, 13, 6, 8, 11, 10, 10, 8, 11, 8, 6, 6, 6, // 32 ' '
    5, 5, 5, 6, 6, 6, 6, 6, 6, 6, 7, 8, 15, 6, 12, 10, // 48 '0'
    13, 6, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, // 64 '@'
    7, 7, 7, 7, 7, 7, 7, 7, 8, 7, 8, 13, 19, 13, 14, 6, // 80 'P'
    15, 5, 6, 5, 6, 5, 6, 6, 6, 5, 7, 7, 6, 6, 6, 5, // 96 '`'
    6, 7, 6, 5, 5, 6, 7, 7, 7, 7, 7, 15, 11, 14, 13, 28, // 112 'p'
    20, 22, 20, 20, 22, 22, 22, 23, 22, 23, 23, 23, 23, 23, 24, 23, // 128
    24, 24, 22, 23, 24, 23, 23, 23, 23, 21, 22, 23, 22, 23, 23, 24, // 144
    22, 21, 20, 22, 22, 23, 23, 21, 23, 22, 22, 24, 21, 22, 23, 23, // 160
    21, 21, 22, 21, 23, 22, 23, 23, 20, 22, 22, 22, 23, 22, 22, 23, // 176
    26, 26, 20, 19, 22, 23, 22, 25, 26, 26, 26, 27, 27, 26, 24, 25, // 192
    19, 21, 26, 27, 27, 26, 27, 24, 21, 21, 26, 26, 28, 27, 27, 27, // 208
    20, 24, 20, 21, 22, 21, 21, 23, 22, 22, 25, 25, 24, 24, 26, 23, // 224
    26, 27, 26, 26, 27, 27, 27, 27, 27, 28, 27, 27, 27, 27, 27, 26, // 240
    30, // EOS
];

const EOS: u16 = 256;

/// Los códigos de Huffman (valor alineado a la derecha, longitud) de cada símbolo.
fn huffman_codes() -> &'static [(u32, u8); 257] {
    static CODES: OnceLock<[(u32, u8); 257]> = OnceLock::new();
    CODES.get_or_init(|| {
        let mut order: Vec<u16> = (0..=EOS).collect();
        order.sort_by_key(|&s| (HUFFMAN_LENGTHS[usize::from(s)], s));
        let mut codes = [(0u32, 0u8); 257];
        let mut code = 0u32;
        let mut prev_len = HUFFMAN_LENGTHS[usize::from(order[0])];
        for (i, &s) in order.iter().enumerate() {
            let len = HUFFMAN_LENGTHS[usize::from(s)];
            if i > 0 {
                code = (code + 1) << (len - prev_len);
            }
            prev_len = len;
            codes[usize::from(s)] = (code, len);
        }
        codes
    })
}

/// Árbol de decodificación: nodos internos con dos hijos; las hojas guardan el símbolo.
#[derive(Clone, Copy)]
enum Node {
    Inner([u16; 2]),
    Leaf(u16),
}

fn huffman_tree() -> &'static [Node] {
    static TREE: OnceLock<Vec<Node>> = OnceLock::new();
    TREE.get_or_init(|| {
        let mut nodes = vec![Node::Inner([0, 0])];
        for (sym, &(code, len)) in huffman_codes().iter().enumerate() {
            let mut at = 0usize;
            for bit in (0..len).rev() {
                let b = ((code >> bit) & 1) as usize;
                let last = bit == 0;
                let Node::Inner(children) = nodes[at] else {
                    unreachable!("el código de Huffman es libre de prefijos")
                };
                if children[b] == 0 {
                    let next = nodes.len();
                    nodes.push(if last {
                        Node::Leaf(sym as u16)
                    } else {
                        Node::Inner([0, 0])
                    });
                    if let Node::Inner(c) = &mut nodes[at] {
                        c[b] = next as u16;
                    }
                    at = next;
                } else {
                    at = usize::from(children[b]);
                }
            }
        }
        nodes
    })
}

/// §5.2: codifica `data` con el código de Huffman, rellenando con el prefijo de EOS (unos).
pub fn huffman_encode(data: &[u8]) -> Vec<u8> {
    let codes = huffman_codes();
    let mut out = Vec::with_capacity(data.len());
    let mut acc = 0u64;
    let mut bits = 0u32;
    for &b in data {
        let (code, len) = codes[usize::from(b)];
        acc = (acc << len) | u64::from(code);
        bits += u32::from(len);
        while bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
        acc &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(((acc << (8 - bits)) | ((1 << (8 - bits)) - 1)) as u8);
    }
    out
}

/// Bytes que ocupa `data` codificado con Huffman.
pub fn huffman_len(data: &[u8]) -> usize {
    let codes = huffman_codes();
    let bits: usize = data
        .iter()
        .map(|&b| usize::from(codes[usize::from(b)].1))
        .sum();
    bits.div_ceil(8)
}

/// §5.2: decodifica. Es un error un EOS en los datos, un relleno de más de 7 bits o uno que
/// no sea el prefijo de EOS.
pub fn huffman_decode(data: &[u8]) -> Result<Vec<u8>, HpackError> {
    let tree = huffman_tree();
    let mut out = Vec::with_capacity(data.len() * 8 / 5);
    let mut at = 0usize;
    // Bits leídos desde el último símbolo, y si todos fueron unos.
    let mut pending = 0u32;
    let mut all_ones = true;
    for &byte in data {
        for bit in (0..8).rev() {
            let b = usize::from((byte >> bit) & 1);
            pending += 1;
            all_ones &= b == 1;
            let Node::Inner(children) = tree[at] else {
                unreachable!()
            };
            at = usize::from(children[b]);
            match tree[at] {
                Node::Leaf(EOS) => return Err(HpackError("EOS en una cadena Huffman")),
                Node::Leaf(sym) => {
                    out.push(sym as u8);
                    at = 0;
                    pending = 0;
                    all_ones = true;
                }
                Node::Inner(_) => {}
            }
        }
    }
    if pending > 7 || !all_ones {
        return Err(HpackError("relleno Huffman inválido"));
    }
    Ok(out)
}

/// §5.1: un entero con prefijo de `n` bits; `first` lleva los bits altos del primer byte.
pub fn encode_int(out: &mut Vec<u8>, value: usize, n: u8, first: u8) {
    let max = (1usize << n) - 1;
    if value < max {
        out.push(first | value as u8);
        return;
    }
    out.push(first | max as u8);
    let mut v = value - max;
    while v >= 128 {
        out.push((v % 128 + 128) as u8);
        v /= 128;
    }
    out.push(v as u8);
}

/// §5.1: lee un entero con prefijo de `n` bits desde `buf[*pos]`.
pub fn decode_int(buf: &[u8], pos: &mut usize, n: u8) -> Result<usize, HpackError> {
    let max = (1usize << n) - 1;
    let first = *buf.get(*pos).ok_or(HpackError("entero truncado"))?;
    *pos += 1;
    let mut value = usize::from(first) & max;
    if value < max {
        return Ok(value);
    }
    let mut shift = 0u32;
    loop {
        let b = *buf.get(*pos).ok_or(HpackError("entero truncado"))?;
        *pos += 1;
        // Más de 4 bytes de continuación pasa de 2^28: ningún valor legítimo llega ahí.
        if shift > 28 {
            return Err(HpackError("entero demasiado grande"));
        }
        value += usize::from(b & 0x7F) << shift;
        shift += 7;
        if b & 0x80 == 0 {
            return Ok(value);
        }
    }
}

/// §5.2: un literal de cadena, con Huffman si `huffman`.
fn encode_string(out: &mut Vec<u8>, s: &[u8], huffman: bool) {
    if huffman {
        let h = huffman_encode(s);
        encode_int(out, h.len(), 7, 0x80);
        out.extend_from_slice(&h);
    } else {
        encode_int(out, s.len(), 7, 0);
        out.extend_from_slice(s);
    }
}

fn decode_string(buf: &[u8], pos: &mut usize) -> Result<Vec<u8>, HpackError> {
    let huffman = buf.get(*pos).ok_or(HpackError("cadena truncada"))? & 0x80 != 0;
    let len = decode_int(buf, pos, 7)?;
    let end = pos
        .checked_add(len)
        .filter(|&e| e <= buf.len())
        .ok_or(HpackError("cadena truncada"))?;
    let raw = &buf[*pos..end];
    *pos = end;
    if huffman {
        huffman_decode(raw)
    } else {
        Ok(raw.to_vec())
    }
}

/// §4: la tabla dinámica. La entrada más nueva va al principio (índice 62).
#[derive(Debug, Clone)]
pub struct DynamicTable {
    entries: VecDeque<Field>,
    size: usize,
    max_size: usize,
}

impl DynamicTable {
    pub fn new(max_size: usize) -> DynamicTable {
        DynamicTable {
            entries: VecDeque::new(),
            size: 0,
            max_size,
        }
    }

    /// §4.1: el tamaño de una entrada es el de sus octetos más 32.
    fn entry_size(f: &Field) -> usize {
        f.0.len() + f.1.len() + 32
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn max_size(&self) -> usize {
        self.max_size
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// §4.3: cambio de tamaño máximo, desalojando lo que no cabe.
    pub fn set_max_size(&mut self, max: usize) {
        self.max_size = max;
        self.evict(0);
    }

    fn evict(&mut self, room: usize) {
        while self.size + room > self.max_size {
            let Some(old) = self.entries.pop_back() else {
                break;
            };
            self.size -= Self::entry_size(&old);
        }
    }

    /// §4.4: una entrada más grande que la tabla la vacía y no se añade.
    pub fn insert(&mut self, f: Field) {
        let size = Self::entry_size(&f);
        self.evict(size);
        if size <= self.max_size {
            self.size += size;
            self.entries.push_front(f);
        }
    }

    /// Entrada con índice HPACK `index` (estática o dinámica; §2.3.3).
    fn get(&self, index: usize) -> Result<Field, HpackError> {
        match index {
            0 => Err(HpackError("índice 0")),
            1..=61 => {
                let (n, v) = STATIC_TABLE[index - 1];
                Ok((n.as_bytes().to_vec(), v.as_bytes().to_vec()))
            }
            _ => self
                .entries
                .get(index - 62)
                .cloned()
                .ok_or(HpackError("índice fuera de la tabla")),
        }
    }
}

/// El decodificador de una conexión (un bloque de campos tras otro, en orden).
#[derive(Debug, Clone)]
pub struct Decoder {
    pub table: DynamicTable,
    /// El tope que anunciamos con `SETTINGS_HEADER_TABLE_SIZE` (§4.2).
    pub max_allowed: usize,
    /// Tope de la lista de campos decodificada (`SETTINGS_MAX_HEADER_LIST_SIZE`).
    pub max_list_size: usize,
}

impl Default for Decoder {
    fn default() -> Self {
        Decoder::new(4096)
    }
}

impl Decoder {
    pub fn new(max_table: usize) -> Decoder {
        Decoder {
            table: DynamicTable::new(max_table),
            max_allowed: max_table,
            max_list_size: 1 << 20,
        }
    }

    /// §3.2: decodifica un bloque de campos completo.
    pub fn decode(&mut self, block: &[u8]) -> Result<Vec<Field>, HpackError> {
        let mut out = Vec::new();
        let mut pos = 0;
        let mut list_size = 0usize;
        let mut seen_field = false;
        while pos < block.len() {
            let b = block[pos];
            let field = if b & 0x80 != 0 {
                // §6.1: campo indexado.
                let index = decode_int(block, &mut pos, 7)?;
                self.table.get(index)?
            } else if b & 0xC0 == 0x40 {
                // §6.2.1: literal con indexado incremental.
                let f = self.literal(block, &mut pos, 6)?;
                self.table.insert(f.clone());
                f
            } else if b & 0xE0 == 0x20 {
                // §6.3: cambio de tamaño de la tabla, solo al principio del bloque (§4.2).
                if seen_field {
                    return Err(HpackError("cambio de tamaño de tabla tras un campo"));
                }
                let size = decode_int(block, &mut pos, 5)?;
                if size > self.max_allowed {
                    return Err(HpackError("tamaño de tabla por encima del permitido"));
                }
                self.table.set_max_size(size);
                continue;
            } else {
                // §6.2.2 (0000) y §6.2.3 (0001): literal sin indexar / nunca indexado.
                self.literal(block, &mut pos, 4)?
            };
            seen_field = true;
            list_size += field.0.len() + field.1.len() + 32;
            if list_size > self.max_list_size {
                return Err(HpackError("lista de campos demasiado grande"));
            }
            out.push(field);
        }
        Ok(out)
    }

    fn literal(&self, block: &[u8], pos: &mut usize, n: u8) -> Result<Field, HpackError> {
        let index = decode_int(block, pos, n)?;
        let name = if index == 0 {
            decode_string(block, pos)?
        } else {
            self.table.get(index)?.0
        };
        let value = decode_string(block, pos)?;
        Ok((name, value))
    }
}

/// Cómo se representa un campo literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indexing {
    /// §6.2.1: se añade a la tabla dinámica.
    Incremental,
    /// §6.2.2.
    Without,
    /// §6.2.3: los intermediarios tampoco lo indexan (credenciales).
    Never,
}

/// El codificador de una conexión.
#[derive(Debug, Clone)]
pub struct Encoder {
    pub table: DynamicTable,
    /// Cambio de tamaño pendiente de anunciar al principio del próximo bloque (§4.2): el
    /// mínimo y el final, por si hubo más de un cambio entre bloques.
    pending_resize: Option<(usize, usize)>,
    /// ¿Se usa Huffman cuando acorta la cadena?
    pub huffman: bool,
}

impl Default for Encoder {
    fn default() -> Self {
        Encoder::new(4096)
    }
}

impl Encoder {
    pub fn new(max_table: usize) -> Encoder {
        Encoder {
            table: DynamicTable::new(max_table),
            pending_resize: None,
            huffman: true,
        }
    }

    /// El par cambió `SETTINGS_HEADER_TABLE_SIZE`: la tabla se ajusta y el cambio se anuncia
    /// en el próximo bloque.
    pub fn set_max_size(&mut self, max: usize) {
        let min = self.pending_resize.map_or(max, |(m, _)| m.min(max));
        self.pending_resize = Some((min, max));
        self.table.set_max_size(max);
    }

    /// Busca `(name, value)`: el índice de una coincidencia completa o el de solo el nombre.
    /// Primero la tabla estática, luego la dinámica (como en los ejemplos del apéndice C).
    fn find(&self, name: &[u8], value: &[u8]) -> (Option<usize>, Option<usize>) {
        let mut name_match = None;
        for (i, (n, v)) in STATIC_TABLE.iter().enumerate() {
            if n.as_bytes() == name {
                if v.as_bytes() == value {
                    return (Some(i + 1), None);
                }
                name_match.get_or_insert(i + 1);
            }
        }
        for (i, (n, v)) in self.table.entries.iter().enumerate() {
            if n == name {
                if v == value {
                    return (Some(i + 62), None);
                }
                name_match.get_or_insert(i + 62);
            }
        }
        (None, name_match)
    }

    /// Codifica un bloque. Los campos sensibles (`authorization`, `cookie` cortas) van como
    /// nunca indexados (§7.1.3); el resto con indexado incremental.
    pub fn encode<'a>(
        &mut self,
        fields: impl IntoIterator<Item = (&'a [u8], &'a [u8])>,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some((min, last)) = self.pending_resize.take() {
            if min < last {
                encode_int(&mut out, min, 5, 0x20);
            }
            encode_int(&mut out, last, 5, 0x20);
        }
        for (name, value) in fields {
            let sensitive = name == b"authorization"
                || name == b"proxy-authorization"
                || (name == b"cookie" && value.len() < 20);
            let mode = if sensitive {
                Indexing::Never
            } else {
                Indexing::Incremental
            };
            self.encode_field(&mut out, name, value, mode);
        }
        out
    }

    /// Codifica un campo con la representación `mode` (si no hay coincidencia completa).
    pub fn encode_field(&mut self, out: &mut Vec<u8>, name: &[u8], value: &[u8], mode: Indexing) {
        let (full, name_index) = self.find(name, value);
        if mode != Indexing::Never
            && let Some(i) = full
        {
            encode_int(out, i, 7, 0x80);
            return;
        }
        let name_index = name_index.or(full);
        let (n, first) = match mode {
            Indexing::Incremental => (6, 0x40),
            Indexing::Without => (4, 0x00),
            Indexing::Never => (4, 0x10),
        };
        match name_index {
            Some(i) => encode_int(out, i, n, first),
            None => {
                out.push(first);
                self.string(out, name);
            }
        }
        self.string(out, value);
        if mode == Indexing::Incremental {
            self.table.insert((name.to_vec(), value.to_vec()));
        }
    }

    /// Con Huffman si no alarga la cadena (C.6.2 lo usa para "307", que ocupa lo mismo).
    fn string(&self, out: &mut Vec<u8>, s: &[u8]) {
        encode_string(
            out,
            s,
            self.huffman && !s.is_empty() && huffman_len(s) <= s.len(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn fields(list: &[(&str, &str)]) -> Vec<Field> {
        list.iter()
            .map(|(n, v)| (n.as_bytes().to_vec(), v.as_bytes().to_vec()))
            .collect()
    }

    fn table(t: &DynamicTable) -> Vec<(String, String)> {
        t.entries
            .iter()
            .map(|(n, v)| {
                (
                    String::from_utf8(n.clone()).unwrap(),
                    String::from_utf8(v.clone()).unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn huffman_code_is_the_rfc_one() {
        // Apéndice B: la suma de Kraft es exactamente 1 y algunos códigos tal cual.
        let kraft: u128 = HUFFMAN_LENGTHS.iter().map(|&l| 1u128 << (30 - l)).sum();
        assert_eq!(kraft, 1 << 30);
        let codes = huffman_codes();
        let expect: [(usize, u32, u8); 16] = [
            (0, 0x1ff8, 13),
            (1, 0x7fffd8, 23),
            (9, 0xffffea, 24),
            (10, 0x3ffffffc, 30),
            (b' ' as usize, 0x14, 6),
            (b'0' as usize, 0x0, 5),
            (b':' as usize, 0x5c, 7),
            (b'<' as usize, 0x7ffc, 15),
            (b'X' as usize, 0xfc, 8),
            (b'\\' as usize, 0x7fff0, 19),
            (b'z' as usize, 0x7b, 7),
            (127, 0xffffffc, 28),
            (128, 0xfffe6, 20),
            (200, 0x3ffffe2, 26),
            (255, 0x3ffffee, 26),
            (256, 0x3fffffff, 30),
        ];
        for (sym, code, len) in expect {
            assert_eq!(codes[sym], (code, len), "símbolo {sym}");
        }
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(huffman_decode(&huffman_encode(&all)).unwrap(), all);
        // Relleno de 8 bits o con ceros: error.
        assert!(huffman_decode(&[0xFF]).is_err());
        assert!(huffman_decode(&[0x00]).is_err());
    }

    #[test]
    fn integers() {
        // C.1.1–C.1.3.
        let mut out = Vec::new();
        encode_int(&mut out, 10, 5, 0);
        assert_eq!(out, [0x0A]);
        out.clear();
        encode_int(&mut out, 1337, 5, 0);
        assert_eq!(out, [0x1F, 0x9A, 0x0A]);
        out.clear();
        encode_int(&mut out, 42, 8, 0);
        assert_eq!(out, [0x2A]);
        let mut pos = 0;
        assert_eq!(decode_int(&[0x1F, 0x9A, 0x0A], &mut pos, 5).unwrap(), 1337);
        assert_eq!(pos, 3);
        assert!(decode_int(&[0x1F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01], &mut 0, 5).is_err());
    }

    #[test]
    fn c2_field_representations() {
        // C.2.1: literal con indexado.
        let mut d = Decoder::default();
        let block = hex("400a 6375 7374 6f6d 2d6b 6579 0d63 7573 746f 6d2d 6865 6164 6572");
        assert_eq!(
            d.decode(&block).unwrap(),
            fields(&[("custom-key", "custom-header")])
        );
        assert_eq!(d.table.size(), 55);
        let mut e = Encoder::new(4096);
        e.huffman = false;
        assert_eq!(
            e.encode([(&b"custom-key"[..], &b"custom-header"[..])]),
            block
        );
        // C.2.2: literal sin indexado.
        let mut d = Decoder::default();
        let block = hex("040c 2f73 616d 706c 652f 7061 7468");
        assert_eq!(
            d.decode(&block).unwrap(),
            fields(&[(":path", "/sample/path")])
        );
        assert!(d.table.is_empty());
        let mut out = Vec::new();
        e.encode_field(&mut out, b":path", b"/sample/path", Indexing::Without);
        assert_eq!(out, block);
        // C.2.3: nunca indexado.
        let mut d = Decoder::default();
        let block = hex("1008 7061 7373 776f 7264 0673 6563 7265 74");
        assert_eq!(d.decode(&block).unwrap(), fields(&[("password", "secret")]));
        assert!(d.table.is_empty());
        let mut out = Vec::new();
        e.encode_field(&mut out, b"password", b"secret", Indexing::Never);
        assert_eq!(out, block);
        // C.2.4: indexado.
        let mut d = Decoder::default();
        assert_eq!(d.decode(&[0x82]).unwrap(), fields(&[(":method", "GET")]));
    }

    /// Un bloque del apéndice C: hex, la lista que codifica, la tabla dinámica resultante y
    /// su tamaño.
    type Step<'a> = (
        &'a str,
        &'a [(&'a str, &'a str)],
        &'a [(&'a str, &'a str)],
        usize,
    );

    /// Decodifica y codifica una secuencia de bloques con la misma pareja de contextos,
    /// comprobando la tabla tras cada uno.
    fn sequence(max_table: usize, huffman: bool, steps: &[Step<'_>]) {
        let mut d = Decoder::new(max_table);
        let mut e = Encoder::new(max_table);
        e.huffman = huffman;
        for (i, (block, list, dyn_table, size)) in steps.iter().enumerate() {
            let block = hex(block);
            assert_eq!(d.decode(&block).unwrap(), fields(list), "bloque {i}");
            let expected: Vec<(String, String)> = dyn_table
                .iter()
                .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
                .collect();
            assert_eq!(table(&d.table), expected, "tabla {i}");
            assert_eq!(d.table.size(), *size, "tamaño {i}");
            let encoded = e.encode(list.iter().map(|(n, v)| (n.as_bytes(), v.as_bytes())));
            assert_eq!(encoded, block, "codificación {i}");
        }
    }

    const REQ1: &[(&str, &str)] = &[
        (":method", "GET"),
        (":scheme", "http"),
        (":path", "/"),
        (":authority", "www.example.com"),
    ];
    const REQ2: &[(&str, &str)] = &[
        (":method", "GET"),
        (":scheme", "http"),
        (":path", "/"),
        (":authority", "www.example.com"),
        ("cache-control", "no-cache"),
    ];
    const REQ3: &[(&str, &str)] = &[
        (":method", "GET"),
        (":scheme", "https"),
        (":path", "/index.html"),
        (":authority", "www.example.com"),
        ("custom-key", "custom-value"),
    ];
    const REQ_T1: &[(&str, &str)] = &[(":authority", "www.example.com")];
    const REQ_T2: &[(&str, &str)] = &[
        ("cache-control", "no-cache"),
        (":authority", "www.example.com"),
    ];
    const REQ_T3: &[(&str, &str)] = &[
        ("custom-key", "custom-value"),
        ("cache-control", "no-cache"),
        (":authority", "www.example.com"),
    ];

    #[test]
    fn c3_requests_without_huffman() {
        sequence(
            4096,
            false,
            &[
                (
                    "8286 8441 0f77 7777 2e65 7861 6d70 6c65 2e63 6f6d",
                    REQ1,
                    REQ_T1,
                    57,
                ),
                ("8286 84be 5808 6e6f 2d63 6163 6865", REQ2, REQ_T2, 110),
                (
                    "8287 85bf 400a 6375 7374 6f6d 2d6b 6579 0c63 7573 746f 6d2d 7661 6c75 65",
                    REQ3,
                    REQ_T3,
                    164,
                ),
            ],
        );
    }

    #[test]
    fn c4_requests_with_huffman() {
        sequence(
            4096,
            true,
            &[
                (
                    "8286 8441 8cf1 e3c2 e5f2 3a6b a0ab 90f4 ff",
                    REQ1,
                    REQ_T1,
                    57,
                ),
                ("8286 84be 5886 a8eb 1064 9cbf", REQ2, REQ_T2, 110),
                (
                    "8287 85bf 4088 25a8 49e9 5ba9 7d7f 8925 a849 e95b b8e8 b4bf",
                    REQ3,
                    REQ_T3,
                    164,
                ),
            ],
        );
    }

    const RES1: &[(&str, &str)] = &[
        (":status", "302"),
        ("cache-control", "private"),
        ("date", "Mon, 21 Oct 2013 20:13:21 GMT"),
        ("location", "https://www.example.com"),
    ];
    const RES2: &[(&str, &str)] = &[
        (":status", "307"),
        ("cache-control", "private"),
        ("date", "Mon, 21 Oct 2013 20:13:21 GMT"),
        ("location", "https://www.example.com"),
    ];
    const RES3: &[(&str, &str)] = &[
        (":status", "200"),
        ("cache-control", "private"),
        ("date", "Mon, 21 Oct 2013 20:13:22 GMT"),
        ("location", "https://www.example.com"),
        ("content-encoding", "gzip"),
        (
            "set-cookie",
            "foo=ASDJKHQKBZXOQWEOPIUAXQWEOIU; max-age=3600; version=1",
        ),
    ];
    const RES_T1: &[(&str, &str)] = &[
        ("location", "https://www.example.com"),
        ("date", "Mon, 21 Oct 2013 20:13:21 GMT"),
        ("cache-control", "private"),
        (":status", "302"),
    ];
    const RES_T2: &[(&str, &str)] = &[
        (":status", "307"),
        ("location", "https://www.example.com"),
        ("date", "Mon, 21 Oct 2013 20:13:21 GMT"),
        ("cache-control", "private"),
    ];
    const RES_T3: &[(&str, &str)] = &[
        (
            "set-cookie",
            "foo=ASDJKHQKBZXOQWEOPIUAXQWEOIU; max-age=3600; version=1",
        ),
        ("content-encoding", "gzip"),
        ("date", "Mon, 21 Oct 2013 20:13:22 GMT"),
    ];

    #[test]
    fn c5_responses_without_huffman() {
        sequence(
            256,
            false,
            &[
                (
                    "4803 3330 3258 0770 7269 7661 7465 611d 4d6f 6e2c 2032 3120 4f63 7420 \
                     3230 3133 2032 303a 3133 3a32 3120 474d 546e 1768 7474 7073 3a2f 2f77 \
                     7777 2e65 7861 6d70 6c65 2e63 6f6d",
                    RES1,
                    RES_T1,
                    222,
                ),
                ("4803 3330 37c1 c0bf", RES2, RES_T2, 222),
                (
                    "88c1 611d 4d6f 6e2c 2032 3120 4f63 7420 3230 3133 2032 303a 3133 3a32 \
                     3220 474d 54c0 5a04 677a 6970 7738 666f 6f3d 4153 444a 4b48 514b 425a \
                     584f 5157 454f 5049 5541 5851 5745 4f49 553b 206d 6178 2d61 6765 3d33 \
                     3630 303b 2076 6572 7369 6f6e 3d31",
                    RES3,
                    RES_T3,
                    215,
                ),
            ],
        );
    }

    #[test]
    fn c6_responses_with_huffman() {
        sequence(
            256,
            true,
            &[
                (
                    "4882 6402 5885 aec3 771a 4b61 96d0 7abe 9410 54d4 44a8 2005 9504 0b81 \
                     66e0 82a6 2d1b ff6e 919d 29ad 1718 63c7 8f0b 97c8 e9ae 82ae 43d3",
                    RES1,
                    RES_T1,
                    222,
                ),
                ("4883 640e ffc1 c0bf", RES2, RES_T2, 222),
                (
                    "88c1 6196 d07a be94 1054 d444 a820 0595 040b 8166 e084 a62d 1bff c05a \
                     839b d9ab 77ad 94e7 821d d7f2 e6c7 b335 dfdf cd5b 3960 d5af 2708 7f36 \
                     72c1 ab27 0fb5 291f 9587 3160 65c0 03ed 4ee5 b106 3d50 07",
                    RES3,
                    RES_T3,
                    215,
                ),
            ],
        );
    }

    #[test]
    fn table_size_updates() {
        let mut e = Encoder::new(4096);
        e.encode([(&b"a"[..], &b"1"[..])]);
        e.set_max_size(0);
        e.set_max_size(100);
        let block = e.encode([(&b"b"[..], &b"2"[..])]);
        // Dos cambios: al mínimo (0) y al final (100), al principio del bloque.
        assert_eq!(&block[..3], &[0x20, 0x3F, 0x45]);
        let mut d = Decoder::new(4096);
        d.decode(&hex("4001 6101 31")).unwrap();
        assert_eq!(d.decode(&block).unwrap(), fields(&[("b", "2")]));
        assert_eq!(table(&d.table), vec![("b".into(), "2".into())]);
        // Un cambio después de un campo, o por encima del tope anunciado: error.
        assert!(d.decode(&[0x82, 0x20]).is_err());
        assert!(Decoder::new(100).decode(&[0x3F, 0xE1, 0x1F]).is_err());
        // Índices fuera de la tabla.
        assert!(Decoder::new(4096).decode(&[0x80]).is_err());
        assert!(Decoder::new(4096).decode(&[0xBE]).is_err());
    }
}
