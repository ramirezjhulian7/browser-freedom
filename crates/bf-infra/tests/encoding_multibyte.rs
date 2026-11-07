//! Decodificadores y encoders multibyte del Encoding Standard (§10–§13): cada pointer de los
//! índices de `data/indexes.json` decodifica a su code point y los encoders vuelven al primer
//! pointer (o al que dicta cada "index … pointer" del §5), más vectores de los algoritmos.

use bf_infra::encoding::{EncodeErrorMode, Encoding};
use serde_json::Value;

fn indexes() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join("indexes.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn index(v: &Value, name: &str) -> Vec<Option<u32>> {
    v[name]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_u64().map(|c| c as u32))
        .collect()
}

fn enc(label: &str) -> Encoding {
    Encoding::for_label(label).unwrap()
}

fn s(cps: &[u32]) -> String {
    cps.iter().map(|&c| char::from_u32(c).unwrap()).collect()
}

fn fatal(e: Encoding, text: &str) -> Result<Vec<u8>, u32> {
    e.encode(text, EncodeErrorMode::Fatal)
}

/// Los primeros pointers de todos los code points de `idx`.
fn first_pointers(
    idx: &[Option<u32>],
    skip: impl Fn(usize) -> bool,
) -> std::collections::HashMap<u32, usize> {
    let mut map = std::collections::HashMap::new();
    for (p, c) in idx.iter().enumerate() {
        if let Some(c) = c
            && !skip(p)
        {
            map.entry(*c).or_insert(p);
        }
    }
    map
}

#[test]
fn labels_resolve_to_multibyte_encodings() {
    for (label, name) in [
        ("gbk", "GBK"),
        ("gb2312", "GBK"),
        ("gb18030", "gb18030"),
        ("big5-hkscs", "Big5"),
        ("euc-jp", "EUC-JP"),
        ("csiso2022jp", "ISO-2022-JP"),
        ("sjis", "Shift_JIS"),
        ("ks_c_5601-1987", "EUC-KR"),
    ] {
        assert_eq!(enc(label).name(), name);
    }
}

#[test]
fn euc_kr_matches_index() {
    let v = indexes();
    let idx = index(&v, "euc-kr");
    let firsts = first_pointers(&idx, |_| false);
    let e = enc("euc-kr");
    for (p, cp) in idx.iter().enumerate() {
        let bytes = [(p / 190 + 0x81) as u8, (p % 190 + 0x41) as u8];
        let got = e.decode_without_bom(&bytes);
        match cp {
            Some(cp) => {
                assert_eq!(got, s(&[*cp]), "pointer {p}");
                let back = firsts[cp];
                let expected = vec![(back / 190 + 0x81) as u8, (back % 190 + 0x41) as u8];
                assert_eq!(fatal(e, &got), Ok(expected), "U+{cp:04X}");
            }
            None => {
                // §13.1.1: si el trail es ASCII se reprocesa.
                let trail = bytes[1];
                let expected = if trail < 0x80 {
                    format!("\u{FFFD}{}", trail as char)
                } else {
                    "\u{FFFD}".to_owned()
                };
                assert_eq!(got, expected, "pointer {p}");
            }
        }
    }
}

#[test]
fn gb18030_matches_index_and_ranges() {
    let v = indexes();
    let idx = index(&v, "gb18030");
    let gb = enc("gb18030");
    let gbk = enc("gbk");
    let firsts = first_pointers(&idx, |_| false);
    let pua = [
        0xE78D, 0xE78E, 0xE78F, 0xE790, 0xE791, 0xE792, 0xE793, 0xE794, 0xE795, 0xE796, 0xE81E,
        0xE826, 0xE82B, 0xE82C, 0xE832, 0xE843, 0xE854, 0xE864,
    ];
    for (p, cp) in idx.iter().enumerate() {
        let trail = p % 190;
        let offset = if trail < 0x3F { 0x40 } else { 0x41 };
        let bytes = vec![(p / 190 + 0x81) as u8, (trail + offset) as u8];
        let Some(cp) = cp else {
            continue;
        };
        assert_eq!(gb.decode_without_bom(&bytes), s(&[*cp]), "pointer {p}");
        assert_eq!(gbk.decode_without_bom(&bytes), s(&[*cp]), "pointer {p}");
        if firsts[cp] == p && !pua.contains(cp) {
            assert_eq!(fatal(gb, &s(&[*cp])), Ok(bytes.clone()), "U+{cp:04X}");
            if *cp != 0x20AC {
                assert_eq!(fatal(gbk, &s(&[*cp])), Ok(bytes), "U+{cp:04X}");
            }
        }
    }
    // §10.2.1 y §5 "index gb18030 ranges code point".
    assert_eq!(gb.decode_without_bom(&[0x80]), "\u{20AC}");
    assert_eq!(gb.decode_without_bom(b"\x81\x30\x81\x30"), "\u{80}");
    assert_eq!(gb.decode_without_bom(b"\x90\x30\x81\x30"), "\u{10000}");
    assert_eq!(gb.decode_without_bom(b"\xE3\x32\x9A\x35"), "\u{10FFFF}");
    assert_eq!(gb.decode_without_bom(b"\xE3\x32\x9A\x36"), "\u{FFFD}");
    assert_eq!(gb.decode_without_bom(b"\x81\x35\xF4\x37"), "\u{E7C7}");
    assert_eq!(gb.decode_without_bom(b"\x84\x31\xA4\x39"), "\u{FFFF}");
    assert_eq!(gb.decode_without_bom(b"\x84\x31\xA5\x30"), "\u{FFFD}");
    // Secuencias cortadas: los bytes se reprocesan.
    assert_eq!(
        gb.decode_without_bom(b"\x81\x30\x81\x7F"),
        "\u{FFFD}0\u{FFFD}\u{7F}"
    );
    assert_eq!(gb.decode_without_bom(b"\x81\x30a"), "\u{FFFD}0a");
    assert_eq!(gb.decode_without_bom(b"\x81a"), "\u{4E64}");
    assert_eq!(gb.decode_without_bom(b"\x81\x7F"), "\u{FFFD}\u{7F}");
    assert_eq!(gb.decode_without_bom(b"\x81"), "\u{FFFD}");
    assert_eq!(gb.decode_without_bom(b"\xFF"), "\u{FFFD}");
    // §10.2.2.
    assert_eq!(fatal(gb, "\u{20AC}"), Ok(vec![0xA2, 0xE3]));
    assert_eq!(fatal(gbk, "\u{20AC}"), Ok(vec![0x80]));
    assert_eq!(fatal(gb, "\u{E5E5}"), Err(0xE5E5));
    assert_eq!(fatal(gb, "\u{E7C7}"), Ok(vec![0x81, 0x35, 0xF4, 0x37]));
    assert_eq!(fatal(gb, "\u{80}"), Ok(vec![0x81, 0x30, 0x81, 0x30]));
    assert_eq!(fatal(gb, "\u{10000}"), Ok(vec![0x90, 0x30, 0x81, 0x30]));
    assert_eq!(fatal(gb, "\u{10FFFF}"), Ok(vec![0xE3, 0x32, 0x9A, 0x35]));
    assert_eq!(fatal(gb, "\u{E78D}"), Ok(vec![0xA6, 0xD9]));
    assert_eq!(fatal(gbk, "\u{E864}"), Ok(vec![0xFE, 0xA0]));
    assert_eq!(fatal(gbk, "\u{80}"), Err(0x80));
    // Cada code point de los rangos vuelve por el mismo camino.
    for cp in (0x80..0xD800).chain(0xE000..0x10000).step_by(7) {
        let c = char::from_u32(cp).unwrap();
        let bytes = fatal(gb, &c.to_string());
        let Ok(bytes) = bytes else {
            assert_eq!(cp, 0xE5E5);
            continue;
        };
        let round = gb.decode_without_bom(&bytes);
        if !pua.contains(&cp) {
            assert_eq!(round, c.to_string(), "U+{cp:04X} {bytes:02X?}");
        }
    }
}

#[test]
fn big5_matches_index() {
    let v = indexes();
    let idx = index(&v, "big5");
    let e = enc("big5");
    let last = [0x2550, 0x255E, 0x2561, 0x256A, 0x5341, 0x5345];
    let min = (0xA1 - 0x81) * 157;
    let firsts = first_pointers(&idx, |q| q < min);
    for (p, cp) in idx.iter().enumerate() {
        let trail = p % 157;
        let offset = if trail < 0x3F { 0x40 } else { 0x62 };
        let bytes = vec![(p / 157 + 0x81) as u8, (trail + offset) as u8];
        let got = e.decode_without_bom(&bytes);
        match p {
            1133 => assert_eq!(got, "\u{CA}\u{304}"),
            1135 => assert_eq!(got, "\u{CA}\u{30C}"),
            1164 => assert_eq!(got, "\u{EA}\u{304}"),
            1166 => assert_eq!(got, "\u{EA}\u{30C}"),
            _ => match cp {
                Some(cp) => {
                    assert_eq!(got, s(&[*cp]), "pointer {p}");
                    let expected = if last.contains(cp) {
                        idx.iter().rposition(|c| *c == Some(*cp))
                    } else {
                        firsts.get(cp).copied()
                    };
                    let encoded = fatal(e, &got);
                    match expected {
                        Some(q) => {
                            let t = q % 157;
                            let o = if t < 0x3F { 0x40 } else { 0x62 };
                            assert_eq!(
                                encoded,
                                Ok(vec![(q / 157 + 0x81) as u8, (t + o) as u8]),
                                "U+{cp:04X}"
                            );
                        }
                        None => assert_eq!(encoded, Err(*cp), "U+{cp:04X}"),
                    }
                }
                None => assert!(got.starts_with('\u{FFFD}'), "pointer {p}"),
            },
        }
    }
    assert_eq!(fatal(e, "\u{2550}"), Ok(vec![0xF9, 0xF9]));
    assert_eq!(fatal(e, "\u{5341}"), Ok(vec![0xA4, 0x51]));
    assert_eq!(e.decode_without_bom(b"\x81\x40"), "\u{FFFD}@");
    assert_eq!(e.decode_without_bom(b"\x81\xA1"), "\u{FFFD}");
    assert_eq!(e.decode_without_bom(b"\x87\x40"), "\u{43F0}");
    assert_eq!(e.decode_without_bom(b"\xFE\x40a\xA1"), "\u{9442}a\u{FFFD}");
}

#[test]
fn jis_encodings_match_index() {
    let v = indexes();
    let jis0208 = index(&v, "jis0208");
    let jis0212 = index(&v, "jis0212");
    let euc = enc("euc-jp");
    let sjis = enc("shift_jis");
    let iso = enc("iso-2022-jp");
    let firsts = first_pointers(&jis0208, |_| false);
    let sjis_firsts = first_pointers(&jis0208, |q| (8272..=8835).contains(&q));
    for (p, cp) in jis0208.iter().enumerate() {
        let Some(cp) = *cp else {
            continue;
        };
        let text = s(&[cp]);
        // EUC-JP (§12.1).
        let eb = vec![(p / 94 + 0xA1) as u8, (p % 94 + 0xA1) as u8];
        if p < 94 * 94 {
            assert_eq!(euc.decode_without_bom(&eb), text, "EUC-JP pointer {p}");
            // ISO-2022-JP (§12.2).
            let ib = [
                &[0x1B, 0x24, 0x42][..],
                &[(p / 94 + 0x21) as u8, (p % 94 + 0x21) as u8],
                &[0x1B, 0x28, 0x42],
            ]
            .concat();
            assert_eq!(iso.decode_without_bom(&ib), text, "ISO-2022-JP pointer {p}");
        }
        // Shift_JIS (§12.3).
        let lead = p / 188;
        let lead_offset = if lead < 0x1F { 0x81 } else { 0xC1 };
        let trail = p % 188;
        let offset = if trail < 0x3F { 0x40 } else { 0x41 };
        let sb = vec![(lead + lead_offset) as u8, (trail + offset) as u8];
        if !(8836..=10715).contains(&p) {
            assert_eq!(sjis.decode_without_bom(&sb), text, "Shift_JIS pointer {p}");
        }
        if matches!(cp, 0xA5 | 0x203E | 0xFF61..=0xFF9F) {
            continue;
        }
        if firsts[&cp] == p {
            assert_eq!(fatal(euc, &text), Ok(eb), "EUC-JP U+{cp:04X}");
        }
        if sjis_firsts.get(&cp) == Some(&p) {
            assert_eq!(fatal(sjis, &text), Ok(sb), "Shift_JIS U+{cp:04X}");
        }
    }
    for (p, cp) in jis0212.iter().enumerate() {
        let bytes = [0x8F, (p / 94 + 0xA1) as u8, (p % 94 + 0xA1) as u8];
        let got = euc.decode_without_bom(&bytes);
        match cp {
            Some(cp) => assert_eq!(got, s(&[*cp]), "jis0212 pointer {p}"),
            None => assert_eq!(got, "\u{FFFD}", "jis0212 pointer {p}"),
        }
    }
    // Shift_JIS: área de uso privado (§12.3.1, paso 3.5) y bytes sueltos.
    assert_eq!(sjis.decode_without_bom(b"\xF0\x40"), "\u{E000}");
    assert_eq!(
        sjis.decode_without_bom(b"\x80\xA1\xDF"),
        "\u{80}\u{FF61}\u{FF9F}"
    );
    assert_eq!(
        fatal(sjis, "\u{A5}\u{203E}\u{2212}"),
        Ok(b"\x5C\x7E\x81\x7C".to_vec())
    );
    assert_eq!(fatal(sjis, "\u{E000}"), Err(0xE000));
    // EUC-JP: katakana de media anchura.
    assert_eq!(
        euc.decode_without_bom(b"\x8E\xA1\x8E\xE0"),
        "\u{FF61}\u{FFFD}"
    );
    assert_eq!(fatal(euc, "\u{FF61}\u{A5}"), Ok(b"\x8E\xA1\x5C".to_vec()));
}

#[test]
fn iso_2022_jp_states() {
    let e = enc("iso-2022-jp");
    // §12.2.1.
    assert_eq!(
        e.decode_without_bom(b"a\x1B(J\x5C\x7E\x1B(Bb"),
        "a\u{A5}\u{203E}b"
    );
    assert_eq!(e.decode_without_bom(b"\x1B(I\x21\x5F"), "\u{FF61}\u{FF9F}");
    assert_eq!(e.decode_without_bom(b"\x1B$B\x30\x21\x1B(B"), "\u{4E9C}");
    // Dos escapes seguidos sin salida entre ellos: error.
    assert_eq!(e.decode_without_bom(b"\x1B(B\x1B(Ba"), "\u{FFFD}a");
    // Escape inválido: se reprocesan los bytes.
    assert_eq!(e.decode_without_bom(b"\x1B(Xa"), "\u{FFFD}(Xa");
    assert_eq!(e.decode_without_bom(b"\x1B"), "\u{FFFD}");
    assert_eq!(e.decode_without_bom(b"\x1B$"), "\u{FFFD}$");
    // Trail cortado por el final.
    assert_eq!(e.decode_without_bom(b"\x1B$B\x30"), "\u{FFFD}");
    assert_eq!(e.decode_without_bom(b"\x0E"), "\u{FFFD}");
    // §12.2.2.
    assert_eq!(fatal(e, "\u{4E9C}a"), Ok(b"\x1B$B\x30\x21\x1B(Ba".to_vec()));
    assert_eq!(
        fatal(e, "\u{A5}a\u{A5}"),
        Ok(b"\x1B(J\x5Ca\x5C\x1B(B".to_vec())
    );
    assert_eq!(fatal(e, "\u{FF61}"), Ok(b"\x1B$B\x21\x23\x1B(B".to_vec()));
    assert_eq!(fatal(e, "\u{0E}"), Err(0xFFFD));
    assert_eq!(fatal(e, "\u{E000}"), Err(0xE000));
    assert_eq!(
        e.encode("\u{4E9C}\u{E000}", EncodeErrorMode::Html),
        Ok(b"\x1B$B\x30\x21\x1B(B&#57344;".to_vec())
    );
}

#[test]
fn streaming_and_fatal() {
    // Un carácter de cuatro bytes partido en trozos.
    let mut d = enc("gb18030").new_decoder();
    let mut out = String::new();
    for chunk in [&b"a\x90"[..], b"\x30", b"\x81", b"\x30b"] {
        d.decode(chunk, false, false, &mut out).unwrap();
    }
    d.decode(&[], true, false, &mut out).unwrap();
    assert_eq!(out, "a\u{10000}b");
    let mut d = enc("shift_jis").new_decoder();
    let mut out = String::new();
    assert!(d.decode(b"a\xFF", true, true, &mut out).is_err());
    assert_eq!(out, "a");
    // Modo html del encoder.
    assert_eq!(
        enc("euc-kr").encode("a\u{1F600}", EncodeErrorMode::Html),
        Ok(b"a&#128512;".to_vec())
    );
    assert_eq!(
        enc("big5").encode_segments("a\u{E000}b"),
        vec![(b"a".to_vec(), Some(0xE000)), (b"b".to_vec(), None)]
    );
}
