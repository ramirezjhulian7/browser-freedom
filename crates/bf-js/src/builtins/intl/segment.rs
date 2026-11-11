//! Segmentación de texto de UAX #29 (Unicode 17.0.0): límites de grafemas extendidos
//! (§3.1.1), de palabras (§4.1.1) y de oraciones (§5.1.1), con las tablas de
//! [`super::segment_tables`]. Sin diccionarios: el chino, el japonés o el tailandés se
//! cortan con las reglas por defecto.
//!
//! Trabaja sobre los code points de un string de JS: un surrogate suelto es un code point
//! más (Grapheme_Cluster_Break=Control). Los límites se devuelven como índices de code unit.

use super::segment_tables::{self as t, gcb, incb, sb, wb};

/// La granularidad de un Intl.Segmenter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Granularity {
    Grapheme,
    Word,
    Sentence,
}

impl Granularity {
    pub fn as_str(self) -> &'static str {
        match self {
            Granularity::Grapheme => "grapheme",
            Granularity::Word => "word",
            Granularity::Sentence => "sentence",
        }
    }
}

fn lookup(table: &[(u32, u32, u8)], c: u32) -> u8 {
    match table.binary_search_by(|&(lo, hi, _)| {
        if hi < c {
            std::cmp::Ordering::Less
        } else if lo > c {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    }) {
        Ok(i) => table[i].2,
        Err(_) => 0,
    }
}

fn is_ext_pict(c: u32) -> bool {
    t::EXT_PICT
        .binary_search_by(|&(lo, hi)| {
            if hi < c {
                std::cmp::Ordering::Less
            } else if lo > c {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// Los code points de `units` con el índice de code unit donde empieza cada uno.
pub fn code_points(units: &[u16]) -> Vec<(u32, usize)> {
    let mut out = Vec::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        let u = units[i];
        if (0xD800..0xDC00).contains(&u)
            && let Some(&l) = units.get(i + 1)
            && (0xDC00..0xE000).contains(&l)
        {
            let c = 0x10000 + ((u32::from(u) - 0xD800) << 10) + (u32::from(l) - 0xDC00);
            out.push((c, i));
            i += 2;
            continue;
        }
        out.push((u32::from(u), i));
        i += 1;
    }
    out
}

/// Los límites (en índices de code point, sin 0 ni `len`) de la granularidad pedida.
fn breaks(cps: &[u32], g: Granularity) -> Vec<usize> {
    match g {
        Granularity::Grapheme => grapheme_breaks(cps),
        Granularity::Word => word_breaks(cps),
        Granularity::Sentence => sentence_breaks(cps),
    }
}

/// Los límites de segmento de `units` (en code units), con 0 y `units.len()`.
pub fn boundaries(units: &[u16], g: Granularity) -> Vec<usize> {
    let cps = code_points(units);
    let only: Vec<u32> = cps.iter().map(|&(c, _)| c).collect();
    let mut out = vec![0];
    for b in breaks(&only, g) {
        out.push(cps[b].1);
    }
    if !units.is_empty() {
        out.push(units.len());
    }
    out
}

// --- Grafemas (§3.1.1) ---

/// Los límites de grafemas extendidos (GB1–GB999), en índices de code point.
// Una rama por regla de UAX #29, aunque varias den el mismo resultado.
#[allow(clippy::if_same_then_else)]
pub fn grapheme_breaks(cps: &[u32]) -> Vec<usize> {
    let props: Vec<u8> = cps.iter().map(|&c| lookup(t::GCB, c)).collect();
    let mut out = Vec::new();
    // GB9c: hubo Consonant [Extend Linker]* y al menos un Linker desde la última Consonant.
    let mut incb_consonant = false;
    let mut incb_linker = false;
    // GB11: lo anterior termina en ExtPict Extend* (`ep`) o en ExtPict Extend* ZWJ (`ep_zwj`).
    let mut ep = false;
    let mut ep_zwj = false;
    // GB12/13: cuántos Regional_Indicator seguidos hay antes.
    let mut ri = 0usize;
    for i in 0..cps.len() {
        if i > 0 {
            let (p, c) = (props[i - 1], props[i]);
            let keep = if p == gcb::CR && c == gcb::LF {
                true // GB3
            } else if matches!(p, gcb::CONTROL | gcb::CR | gcb::LF)
                || matches!(c, gcb::CONTROL | gcb::CR | gcb::LF)
            {
                false // GB4, GB5
            } else if p == gcb::L && matches!(c, gcb::L | gcb::V | gcb::LV | gcb::LVT) {
                true // GB6
            } else if matches!(p, gcb::LV | gcb::V) && matches!(c, gcb::V | gcb::T) {
                true // GB7
            } else if matches!(p, gcb::LVT | gcb::T) && c == gcb::T {
                true // GB8
            } else if matches!(c, gcb::EXTEND | gcb::ZWJ | gcb::SPACINGMARK) || p == gcb::PREPEND {
                true // GB9, GB9a, GB9b
            } else if incb_consonant && incb_linker && lookup(t::INCB, cps[i]) == incb::CONSONANT {
                true // GB9c
            } else if ep_zwj && is_ext_pict(cps[i]) {
                true // GB11
            } else {
                // GB12, GB13
                p == gcb::REGIONAL_INDICATOR && c == gcb::REGIONAL_INDICATOR && ri % 2 == 1
            };
            if !keep {
                out.push(i);
            }
        }
        // El estado después de `cps[i]`.
        match lookup(t::INCB, cps[i]) {
            incb::CONSONANT => {
                incb_consonant = true;
                incb_linker = false;
            }
            incb::LINKER if incb_consonant => incb_linker = true,
            incb::EXTEND if incb_consonant => {}
            _ => {
                incb_consonant = false;
                incb_linker = false;
            }
        }
        let c = props[i];
        ep_zwj = ep && c == gcb::ZWJ;
        ep = is_ext_pict(cps[i]) || (ep && c == gcb::EXTEND);
        ri = if c == gcb::REGIONAL_INDICATOR {
            ri + 1
        } else {
            0
        };
    }
    out
}

// --- Palabras (§4.1.1) ---

fn is_ah_letter(p: u8) -> bool {
    p == wb::ALETTER || p == wb::HEBREW_LETTER
}

fn is_mid_num_let_q(p: u8) -> bool {
    p == wb::MIDNUMLET || p == wb::SINGLE_QUOTE
}

fn is_ignorable_wb(p: u8) -> bool {
    matches!(p, wb::EXTEND | wb::FORMAT | wb::ZWJ)
}

/// Los límites de palabras (WB1–WB999), en índices de code point.
// Una rama por regla de UAX #29, aunque varias den el mismo resultado.
#[allow(clippy::if_same_then_else)]
pub fn word_breaks(cps: &[u32]) -> Vec<usize> {
    let props: Vec<u8> = cps.iter().map(|&c| lookup(t::WB, c)).collect();
    let n = props.len();
    // WB4: el code point "efectivo" en cada posición: un Extend/Format/ZWJ se pega al code
    // point anterior salvo después de sot, CR, LF o Newline.
    let mut base: Vec<Option<usize>> = vec![None; n];
    for i in 0..n {
        base[i] = if i > 0
            && is_ignorable_wb(props[i])
            && !matches!(props[i - 1], wb::CR | wb::LF | wb::NEWLINE)
        {
            base[i - 1]
        } else {
            Some(i)
        };
    }
    // El code point efectivo antes de `i` (el de la posición `i - 1`).
    let prev = |i: usize| -> Option<usize> { if i == 0 { None } else { base[i - 1] } };
    // El primer code point no ignorable desde `i` (para mirar hacia adelante).
    let next_from = |i: usize| -> Option<usize> { (i..n).find(|&j| !is_ignorable_wb(props[j])) };
    let mut out = Vec::new();
    for i in 1..n {
        let (rp, c) = (props[i - 1], props[i]);
        // WB3–WB3d con los code points tal cual.
        if rp == wb::CR && c == wb::LF {
            continue; // WB3
        }
        if matches!(rp, wb::NEWLINE | wb::CR | wb::LF) || matches!(c, wb::NEWLINE | wb::CR | wb::LF)
        {
            out.push(i); // WB3a, WB3b
            continue;
        }
        if rp == wb::ZWJ && is_ext_pict(cps[i]) {
            continue; // WB3c
        }
        if rp == wb::WSEGSPACE && c == wb::WSEGSPACE {
            continue; // WB3d
        }
        if is_ignorable_wb(c) {
            continue; // WB4
        }
        let Some(pi) = prev(i) else {
            out.push(i);
            continue;
        };
        let p = props[pi];
        let pp = if pi > 0 {
            base[pi - 1].map(|k| props[k])
        } else {
            None
        };
        let nn = next_from(i + 1).map(|k| props[k]);
        let keep = if is_ah_letter(p) && is_ah_letter(c) {
            true // WB5
        } else if is_ah_letter(p)
            && (c == wb::MIDLETTER || is_mid_num_let_q(c))
            && nn.is_some_and(is_ah_letter)
        {
            true // WB6
        } else if pp.is_some_and(is_ah_letter)
            && (p == wb::MIDLETTER || is_mid_num_let_q(p))
            && is_ah_letter(c)
        {
            true // WB7
        } else if p == wb::HEBREW_LETTER && c == wb::SINGLE_QUOTE {
            true // WB7a
        } else if p == wb::HEBREW_LETTER && c == wb::DOUBLE_QUOTE && nn == Some(wb::HEBREW_LETTER) {
            true // WB7b
        } else if pp == Some(wb::HEBREW_LETTER) && p == wb::DOUBLE_QUOTE && c == wb::HEBREW_LETTER {
            true // WB7c
        } else if (p == wb::NUMERIC && c == wb::NUMERIC)
            || (is_ah_letter(p) && c == wb::NUMERIC)
            || (p == wb::NUMERIC && is_ah_letter(c))
        {
            true // WB8, WB9, WB10
        } else if pp == Some(wb::NUMERIC)
            && (p == wb::MIDNUM || is_mid_num_let_q(p))
            && c == wb::NUMERIC
        {
            true // WB11
        } else if p == wb::NUMERIC
            && (c == wb::MIDNUM || is_mid_num_let_q(c))
            && nn == Some(wb::NUMERIC)
        {
            true // WB12
        } else if p == wb::KATAKANA && c == wb::KATAKANA {
            true // WB13
        } else if (is_ah_letter(p) || matches!(p, wb::NUMERIC | wb::KATAKANA | wb::EXTENDNUMLET))
            && c == wb::EXTENDNUMLET
        {
            true // WB13a
        } else if p == wb::EXTENDNUMLET
            && (is_ah_letter(c) || matches!(c, wb::NUMERIC | wb::KATAKANA))
        {
            true // WB13b
        } else if p == wb::REGIONAL_INDICATOR && c == wb::REGIONAL_INDICATOR {
            // WB15, WB16: un número impar de RI efectivos antes.
            let mut count = 0;
            let mut k = Some(pi);
            while let Some(j) = k {
                if props[j] != wb::REGIONAL_INDICATOR {
                    break;
                }
                count += 1;
                k = if j > 0 { base[j - 1] } else { None };
            }
            count % 2 == 1
        } else {
            false
        };
        if !keep {
            out.push(i);
        }
    }
    out
}

/// Un segmento de palabra es "word-like" si tiene letras, números o ideogramas (lo que ICU
/// marca con los estados `letter`, `number`, `kana` o `ideo`).
pub fn is_word_like(units: &[u16]) -> bool {
    code_points(units).iter().any(|&(c, _)| {
        let p = lookup(t::WB, c);
        if matches!(
            p,
            wb::ALETTER | wb::HEBREW_LETTER | wb::NUMERIC | wb::KATAKANA | wb::EXTENDNUMLET
        ) {
            return p != wb::EXTENDNUMLET || c != 0x5F;
        }
        // Letras sin Word_Break propio: ideogramas, hiragana, thai, lao, khmer, myanmar…
        char::from_u32(c).is_some_and(char::is_alphanumeric)
    })
}

// --- Oraciones (§5.1.1) ---

fn is_para_sep(p: u8) -> bool {
    matches!(p, sb::SEP | sb::CR | sb::LF)
}

fn is_sa_term(p: u8) -> bool {
    p == sb::STERM || p == sb::ATERM
}

/// Los límites de oraciones (SB1–SB998), en índices de code point.
// Una rama por regla de UAX #29, aunque varias den el mismo resultado.
#[allow(clippy::if_same_then_else)]
pub fn sentence_breaks(cps: &[u32]) -> Vec<usize> {
    let raw: Vec<u8> = cps.iter().map(|&c| lookup(t::SB, c)).collect();
    let n = raw.len();
    // SB5: los tokens (code points que no son Extend/Format pegados al anterior).
    let mut tokens: Vec<usize> = Vec::new();
    for i in 0..n {
        if i == 0 || !matches!(raw[i], sb::EXTEND | sb::FORMAT) || is_para_sep(raw[i - 1]) {
            tokens.push(i);
        }
    }
    let tp: Vec<u8> = tokens.iter().map(|&i| raw[i]).collect();
    let mut out = Vec::new();
    for (t, &i) in tokens.iter().enumerate().skip(1) {
        let (p, c) = (tp[t - 1], tp[t]);
        let keep = if raw[i - 1] == sb::CR && raw[i] == sb::LF {
            true // SB3
        } else if is_para_sep(p) {
            false // SB4
        } else if p == sb::ATERM && c == sb::NUMERIC {
            true // SB6
        } else if t >= 2
            && matches!(tp[t - 2], sb::UPPER | sb::LOWER)
            && p == sb::ATERM
            && c == sb::UPPER
        {
            true // SB7
        } else {
            // SATerm Close* Sp* antes de la posición.
            let mut k = t - 1;
            let mut sps = 0;
            while tp[k] == sb::SP && k > 0 {
                k -= 1;
                sps += 1;
            }
            while tp[k] == sb::CLOSE && k > 0 {
                k -= 1;
            }
            let term = is_sa_term(tp[k]).then_some(tp[k]);
            match term {
                None => true, // SB998
                Some(term) => {
                    let sb8 = term == sb::ATERM && {
                        let next = tp[t..].iter().copied().find(|&x| {
                            matches!(x, sb::OLETTER | sb::UPPER | sb::LOWER)
                                || is_para_sep(x)
                                || is_sa_term(x)
                        });
                        next == Some(sb::LOWER)
                    };
                    if sb8 {
                        true // SB8
                    } else if c == sb::SCONTINUE || is_sa_term(c) {
                        true // SB8a
                    } else if sps == 0 && (c == sb::CLOSE || c == sb::SP || is_para_sep(c)) {
                        true // SB9
                    } else {
                        // SB10 / SB11
                        c == sb::SP || is_para_sep(c)
                    }
                }
            }
        };
        if !keep {
            out.push(i);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Corre un archivo de pruebas de la UCD (`÷ 0020 × 0308 ÷`).
    fn run(file: &str, g: Granularity) {
        let path = format!(
            "{}/data/ucd-17.0.0/auxiliary/{file}",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = std::fs::read_to_string(path).expect("datos de la UCD");
        let mut failed = Vec::new();
        let mut total = 0;
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            total += 1;
            let mut cps = Vec::new();
            let mut expected = Vec::new();
            for tok in line.split_whitespace() {
                match tok {
                    "÷" => {
                        if !cps.is_empty() {
                            expected.push(cps.len());
                        }
                    }
                    "×" => {}
                    h => cps.push(u32::from_str_radix(h, 16).expect("hex")),
                }
            }
            expected.retain(|&b| b < cps.len());
            if breaks(&cps, g) != expected {
                failed.push(line.to_owned());
            }
        }
        assert!(
            failed.is_empty(),
            "{file}: {} de {total} fallan, p. ej.\n{}",
            failed.len(),
            failed
                .iter()
                .take(10)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    #[test]
    fn grapheme_break_test() {
        run("GraphemeBreakTest.txt", Granularity::Grapheme);
    }

    #[test]
    fn word_break_test() {
        run("WordBreakTest.txt", Granularity::Word);
    }

    #[test]
    fn sentence_break_test() {
        run("SentenceBreakTest.txt", Granularity::Sentence);
    }

    #[test]
    fn surrogates_and_units() {
        // "a😀b": el emoji es un par de surrogates.
        let s: Vec<u16> = "a\u{1F600}b".encode_utf16().collect();
        assert_eq!(boundaries(&s, Granularity::Grapheme), vec![0, 1, 3, 4]);
        // Un surrogate suelto es su propio grafema.
        assert_eq!(
            boundaries(&[0x61, 0xD800, 0x62], Granularity::Grapheme),
            vec![0, 1, 2, 3]
        );
        assert_eq!(boundaries(&[], Granularity::Word), vec![0]);
    }
}
