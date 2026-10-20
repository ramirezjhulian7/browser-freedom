//! WHATWG Encoding Standard: etiquetas, BOM y decodificadores de UTF-8, UTF-16LE/BE, todos los
//! single-byte (tablas generadas con `tools/gen_encodings.py`), `x-user-defined` y
//! `replacement`, más el encoder que usa el parser de URLs.
//!
//! TODO(roadmap): los multibyte (GBK, gb18030, Big5, EUC-JP, ISO-2022-JP, Shift_JIS, EUC-KR) se
//! reconocen por su etiqueta pero se decodifican como windows-1252.

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
    /// Multibyte sin decodificador todavía (nombre canónico).
    Unsupported(&'static str),
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
            _ => match SINGLE_BYTE.iter().position(|(n, _)| *n == name) {
                Some(i) => Encoding::SingleByte(i),
                None => Encoding::Unsupported(name),
            },
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
            Encoding::Unsupported(n) => n,
        }
    }

    /// ¿Hay decodificador propio? (Los multibyte se decodifican como windows-1252.)
    pub fn is_supported(&self) -> bool {
        !matches!(self, Encoding::Unsupported(_))
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
            Encoding::Unsupported(_) => Encoding::windows_1252().decode_without_bom(bytes),
        }
    }

    /// "Get an output encoding" (§4.3): UTF-16 y replacement codifican como UTF-8.
    pub fn output_encoding(&self) -> Encoding {
        match self {
            Encoding::Utf16Le | Encoding::Utf16Be | Encoding::Replacement => Encoding::Utf8,
            e => *e,
        }
    }

    /// ¿Hay encoder? (UTF-8 y single-byte; los multibyte no.)
    pub fn can_encode(&self) -> bool {
        !matches!(self.output_encoding(), Encoding::Unsupported(_))
    }

    /// Bytes de `c` en el encoding de salida, o `None` si no es representable.
    pub fn encode_char(&self, c: char) -> Option<Vec<u8>> {
        let cp = c as u32;
        match self.output_encoding() {
            Encoding::SingleByte(i) => {
                if cp < 0x80 {
                    return Some(vec![cp as u8]);
                }
                let table = &SINGLE_BYTE[i].1;
                table
                    .iter()
                    .position(|&t| u32::from(t) == cp && t != 0)
                    .map(|b| vec![0x80 + b as u8])
            }
            Encoding::XUserDefined => {
                if cp < 0x80 {
                    Some(vec![cp as u8])
                } else if (0xF780..=0xF7FF).contains(&cp) {
                    Some(vec![(cp - 0xF780 + 0x80) as u8])
                } else {
                    None
                }
            }
            Encoding::Unsupported(_) => None,
            _ => {
                let mut buf = [0u8; 4];
                Some(c.encode_utf8(&mut buf).as_bytes().to_vec())
            }
        }
    }

    /// Codifica `s` con el modo de error HTML: lo no representable pasa a `&#N;` (lo usa el
    /// parser de URLs para el query).
    pub fn encode_for_url(&self, s: &str) -> Vec<u8> {
        match self.output_encoding() {
            Encoding::SingleByte(i) => {
                let table = &SINGLE_BYTE[i].1;
                let mut out = Vec::with_capacity(s.len());
                for c in s.chars() {
                    let cp = c as u32;
                    if cp < 0x80 {
                        out.push(cp as u8);
                    } else if let Some(b) = table.iter().position(|&t| u32::from(t) == cp && t != 0)
                    {
                        out.push(0x80 + b as u8);
                    } else {
                        out.extend_from_slice(format!("&#{cp};").as_bytes());
                    }
                }
                out
            }
            Encoding::XUserDefined => {
                let mut out = Vec::with_capacity(s.len());
                for c in s.chars() {
                    let cp = c as u32;
                    if cp < 0x80 {
                        out.push(cp as u8);
                    } else if (0xF780..=0xF7FF).contains(&cp) {
                        out.push((cp - 0xF780 + 0x80) as u8);
                    } else {
                        out.extend_from_slice(format!("&#{cp};").as_bytes());
                    }
                }
                out
            }
            _ => s.as_bytes().to_vec(),
        }
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
