//! WHATWG Encoding Standard: etiquetas, BOM y los decodificadores y encoders de todos los
//! encodings: UTF-8, UTF-16LE/BE, los single-byte (tablas generadas con
//! `tools/gen_encodings.py`), los multibyte (GBK, gb18030, Big5, EUC-JP, ISO-2022-JP,
//! Shift_JIS y EUC-KR, §10–§13), `x-user-defined` y `replacement`. Las máquinas de estado
//! están en [`crate::encoding_codec`]: [`Decoder`] y [`Encoder`].

pub use crate::encoding_codec::{DecodeError, Decoder, Encoder};
use crate::encoding_tables::{LABELS, SINGLE_BYTE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    /// Índice en la tabla de encodings single-byte.
    SingleByte(usize),
    XUserDefined,
    Replacement,
    /// GBK (§10.1).
    Gbk,
    /// gb18030 (§10.2).
    Gb18030,
    /// Big5 (§11.1).
    Big5,
    /// EUC-JP (§12.1).
    EucJp,
    /// ISO-2022-JP (§12.2).
    Iso2022Jp,
    /// Shift_JIS (§12.3).
    ShiftJis,
    /// EUC-KR (§13.1).
    EucKr,
}

/// Modo de error de un encoder (§4.5): `Fatal` corta en el primer code point no
/// representable; `Html` lo escribe como `&#N;`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeErrorMode {
    Fatal,
    Html,
}

pub const WINDOWS_1252: &str = "windows-1252";

impl Encoding {
    /// "Get an encoding" (§4.2): la etiqueta sin whitespace ASCII y sin distinguir mayúsculas.
    pub fn for_label(label: &str) -> Option<Encoding> {
        let label = label
            .trim_matches(|c: char| matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' '))
            .to_ascii_lowercase();
        let i = LABELS
            .binary_search_by(|(l, _)| (*l).cmp(label.as_str()))
            .ok()?;
        Some(Encoding::for_name(LABELS[i].1))
    }

    fn for_name(name: &'static str) -> Encoding {
        match name {
            "UTF-8" => Encoding::Utf8,
            "UTF-16LE" => Encoding::Utf16Le,
            "UTF-16BE" => Encoding::Utf16Be,
            "x-user-defined" => Encoding::XUserDefined,
            "replacement" => Encoding::Replacement,
            "GBK" => Encoding::Gbk,
            "gb18030" => Encoding::Gb18030,
            "Big5" => Encoding::Big5,
            "EUC-JP" => Encoding::EucJp,
            "ISO-2022-JP" => Encoding::Iso2022Jp,
            "Shift_JIS" => Encoding::ShiftJis,
            "EUC-KR" => Encoding::EucKr,
            _ => Encoding::SingleByte(
                SINGLE_BYTE
                    .iter()
                    .position(|(n, _)| *n == name)
                    .expect("encodings.json y SINGLE_BYTE coinciden"),
            ),
        }
    }

    pub fn windows_1252() -> Encoding {
        Encoding::for_name(WINDOWS_1252)
    }

    pub fn name(&self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Utf16Le => "UTF-16LE",
            Encoding::Utf16Be => "UTF-16BE",
            Encoding::SingleByte(i) => SINGLE_BYTE[*i].0,
            Encoding::XUserDefined => "x-user-defined",
            Encoding::Replacement => "replacement",
            Encoding::Gbk => "GBK",
            Encoding::Gb18030 => "gb18030",
            Encoding::Big5 => "Big5",
            Encoding::EucJp => "EUC-JP",
            Encoding::Iso2022Jp => "ISO-2022-JP",
            Encoding::ShiftJis => "Shift_JIS",
            Encoding::EucKr => "EUC-KR",
        }
    }

    /// ¿Hay decodificador propio? Todos los encodings del spec lo tienen.
    pub fn is_supported(&self) -> bool {
        true
    }

    /// Un decodificador nuevo de este encoding.
    pub fn new_decoder(&self) -> Decoder {
        Decoder::new(*self)
    }

    /// Un encoder nuevo del output encoding de este encoding.
    pub fn new_encoder(&self) -> Encoder {
        Encoder::new(*self)
    }

    /// Decodifica `bytes` (sin mirar el BOM; ver [`decode`]), reemplazando los errores por
    /// U+FFFD.
    pub fn decode_without_bom(&self, bytes: &[u8]) -> String {
        match self {
            Encoding::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
            Encoding::Utf16Le | Encoding::Utf16Be => {
                let le = *self == Encoding::Utf16Le;
                let pairs = bytes.as_chunks::<2>();
                let units: Vec<u16> = pairs
                    .0
                    .iter()
                    .map(|&b| {
                        if le {
                            u16::from_le_bytes(b)
                        } else {
                            u16::from_be_bytes(b)
                        }
                    })
                    .collect();
                let mut s = String::from_utf16_lossy(&units);
                if !pairs.1.is_empty() {
                    s.push('\u{FFFD}');
                }
                s
            }
            Encoding::SingleByte(i) => {
                let table = &SINGLE_BYTE[*i].1;
                bytes
                    .iter()
                    .map(|&b| {
                        if b < 0x80 {
                            char::from(b)
                        } else {
                            match table[usize::from(b - 0x80)] {
                                0 => '\u{FFFD}',
                                cp => char::from_u32(u32::from(cp)).unwrap_or('\u{FFFD}'),
                            }
                        }
                    })
                    .collect()
            }
            Encoding::XUserDefined => bytes
                .iter()
                .map(|&b| {
                    if b < 0x80 {
                        char::from(b)
                    } else {
                        char::from_u32(0xF780 + u32::from(b) - 0x80).unwrap_or('\u{FFFD}')
                    }
                })
                .collect(),
            Encoding::Replacement => {
                if bytes.is_empty() {
                    String::new()
                } else {
                    "\u{FFFD}".to_owned()
                }
            }
            _ => {
                let mut out = String::with_capacity(bytes.len());
                // Sin `fatal` no hay errores que propagar.
                let _ = self.new_decoder().decode(bytes, true, false, &mut out);
                out
            }
        }
    }

    /// "Get an output encoding" (§4.3): UTF-16 y replacement codifican como UTF-8.
    pub fn output_encoding(&self) -> Encoding {
        match self {
            Encoding::Utf16Le | Encoding::Utf16Be | Encoding::Replacement => Encoding::Utf8,
            e => *e,
        }
    }

    /// ¿Hay encoder? Todos los output encodings lo tienen.
    pub fn can_encode(&self) -> bool {
        true
    }

    /// Bytes de `c` en el encoding de salida (con un encoder nuevo, así que ISO-2022-JP
    /// incluye sus secuencias de escape), o `None` si no es representable.
    pub fn encode_char(&self, c: char) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        match self.new_encoder().encode_or_fail(&mut std::iter::once(c), &mut out) {
            None => Some(out),
            Some(_) => None,
        }
    }

    /// "Encode" (§6) de `s` con el modo de error dado; con `Fatal`, `Err` lleva el primer
    /// code point no representable.
    pub fn encode(&self, s: &str, mode: EncodeErrorMode) -> Result<Vec<u8>, u32> {
        let mut encoder = self.new_encoder();
        let mut out = Vec::with_capacity(s.len());
        let mut input = s.chars();
        loop {
            match encoder.encode_or_fail(&mut input, &mut out) {
                None => return Ok(out),
                Some(cp) if mode == EncodeErrorMode::Fatal => return Err(cp),
                // Modo html: "&#N;" se antepone a la entrada y pasa por el encoder (§3,
                // "process an item"); todo es ASCII, así que basta con codificarlo aparte
                // con la misma instancia.
                Some(cp) => {
                    let r = encoder.encode_items(&mut format!("&#{cp};").chars(), false, &mut out);
                    debug_assert!(r.is_none(), "las referencias de carácter son ASCII");
                }
            }
        }
    }

    /// "Percent-encode after encoding" (URL §1.3) por partes: cada trozo de bytes codificados
    /// seguido del code point no representable que lo cortó (el último, sin ninguno).
    pub fn encode_segments(&self, s: &str) -> Vec<(Vec<u8>, Option<u32>)> {
        let mut encoder = self.new_encoder();
        let mut input = s.chars();
        let mut segments = Vec::new();
        loop {
            let mut out = Vec::new();
            let error = encoder.encode_or_fail(&mut input, &mut out);
            segments.push((out, error));
            if error.is_none() {
                return segments;
            }
        }
    }

    /// Codifica `s` con el modo de error HTML: lo no representable pasa a `&#N;`.
    pub fn encode_for_url(&self, s: &str) -> Vec<u8> {
        self.encode(s, EncodeErrorMode::Html)
            .expect("el modo html no falla")
    }
}

/// BOM sniffing (§4.1): el encoding del BOM y su longitud.
pub fn sniff_bom(bytes: &[u8]) -> Option<(Encoding, usize)> {
    match bytes {
        [0xEF, 0xBB, 0xBF, ..] => Some((Encoding::Utf8, 3)),
        [0xFE, 0xFF, ..] => Some((Encoding::Utf16Be, 2)),
        [0xFF, 0xFE, ..] => Some((Encoding::Utf16Le, 2)),
        _ => None,
    }
}

/// "Decode" (§6): el BOM manda sobre `fallback`. Devuelve el texto y el encoding usado.
pub fn decode(bytes: &[u8], fallback: Encoding) -> (String, Encoding) {
    match sniff_bom(bytes) {
        Some((e, n)) => (e.decode_without_bom(&bytes[n..]), e),
        None => (fallback.decode_without_bom(bytes), fallback),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_decoders() {
        assert_eq!(
            Encoding::for_label(" Latin1 ").map(|e| e.name()),
            Some("windows-1252")
        );
        assert_eq!(Encoding::for_label("utf8"), Some(Encoding::Utf8));
        assert_eq!(
            Encoding::for_label("sjis").map(|e| e.name()),
            Some("Shift_JIS")
        );
        assert!(Encoding::for_label("nope").is_none());
        let w = Encoding::windows_1252();
        assert_eq!(w.decode_without_bom(&[0x80, b'a', 0x81]), "€a\u{81}");
        assert_eq!(w.encode_for_url("€ñ\u{2020}☃"), b"\x80\xF1\x86&#9731;");
        let koi = Encoding::for_label("koi8-r").unwrap();
        assert_eq!(koi.decode_without_bom(&[0xC1]), "а");
        assert_eq!(decode(b"\xFF\xFEh\0", Encoding::Utf8).0, "h");
    }
}
