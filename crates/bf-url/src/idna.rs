//! IDNA: UTS #46 (Unicode IDNA Compatibility Processing) con los parámetros del URL Standard
//! (§3.3 "domain to ASCII": CheckHyphens=false, CheckBidi=true, CheckJoiners=true,
//! UseSTD3ASCIIRules=false, Transitional_Processing=false, VerifyDnsLength=false) y Punycode
//! (RFC 3492). Las tablas se generan con `tools/gen_idna.py`.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::{canonical_combining_class, is_combining_mark};

use crate::tables::{BIDI, IDNA, IDNA_MAPPING, JOINING};

// --- Punycode (RFC 3492) ---

const BASE: u32 = 36;
const T_MIN: u32 = 1;
const T_MAX: u32 = 26;
const SKEW: u32 = 38;
const DAMP: u32 = 700;
const INITIAL_BIAS: u32 = 72;
const INITIAL_N: u32 = 128;

fn adapt(mut delta: u32, num_points: u32, first: bool) -> u32 {
    delta /= if first { DAMP } else { 2 };
    delta += delta / num_points;
    let mut k = 0;
    while delta > ((BASE - T_MIN) * T_MAX) / 2 {
        delta /= BASE - T_MIN;
        k += BASE;
    }
    k + (BASE - T_MIN + 1) * delta / (delta + SKEW)
}

fn digit(d: u32) -> char {
    char::from_u32(if d < 26 {
        d + u32::from(b'a')
    } else {
        d - 26 + u32::from(b'0')
    })
    .unwrap_or('a')
}

fn digit_value(c: char) -> Option<u32> {
    match c {
        'a'..='z' => Some(c as u32 - 'a' as u32),
        'A'..='Z' => Some(c as u32 - 'A' as u32),
        '0'..='9' => Some(c as u32 - '0' as u32 + 26),
        _ => None,
    }
}

/// Codifica `input` en Punycode (sin el prefijo `xn--`).
pub fn punycode_encode(input: &[char]) -> Option<String> {
    let mut out: String = input.iter().filter(|c| c.is_ascii()).collect();
    let basic = out.len() as u32;
    let mut handled = basic;
    if basic > 0 {
        out.push('-');
    }
    let (mut n, mut delta, mut bias) = (INITIAL_N, 0u32, INITIAL_BIAS);
    while (handled as usize) < input.len() {
        let m = input.iter().map(|&c| c as u32).filter(|&c| c >= n).min()?;
        delta = delta.checked_add((m - n).checked_mul(handled + 1)?)?;
        n = m;
        for &c in input {
            let c = c as u32;
            if c < n {
                delta = delta.checked_add(1)?;
            }
            if c == n {
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = if k <= bias {
                        T_MIN
                    } else if k >= bias + T_MAX {
                        T_MAX
                    } else {
                        k - bias
                    };
                    if q < t {
                        break;
                    }
                    out.push(digit(t + (q - t) % (BASE - t)));
                    q = (q - t) / (BASE - t);
                    k += BASE;
                }
                out.push(digit(q));
                bias = adapt(delta, handled + 1, handled == basic);
                delta = 0;
                handled += 1;
            }
        }
        delta = delta.checked_add(1)?;
        n = n.checked_add(1)?;
    }
    Some(out)
}

/// Decodifica Punycode (sin el prefijo `xn--`).
pub fn punycode_decode(input: &str) -> Option<Vec<char>> {
    let (basic, rest) = match input.rfind('-') {
        Some(i) => (&input[..i], &input[i + 1..]),
        None => ("", input),
    };
    if !basic.is_ascii() {
        return None;
    }
    let mut out: Vec<char> = basic.chars().collect();
    let (mut n, mut i, mut bias) = (INITIAL_N, 0u32, INITIAL_BIAS);
    let mut chars = rest.chars().peekable();
    while chars.peek().is_some() {
        let old_i = i;
        let mut w = 1u32;
        let mut k = BASE;
        loop {
            let d = digit_value(chars.next()?)?;
            i = i.checked_add(d.checked_mul(w)?)?;
            let t = if k <= bias {
                T_MIN
            } else if k >= bias + T_MAX {
                T_MAX
            } else {
                k - bias
            };
            if d < t {
                break;
            }
            w = w.checked_mul(BASE - t)?;
            k += BASE;
        }
        let len = out.len() as u32 + 1;
        bias = adapt(i - old_i, len, old_i == 0);
        n = n.checked_add(i / len)?;
        i %= len;
        out.insert(i as usize, char::from_u32(n)?);
        i += 1;
    }
    Some(out)
}

// --- Tablas ---

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Valid,
    Mapped,
    Deviation,
    Disallowed,
    Ignored,
}

fn lookup(c: char) -> (Status, &'static str) {
    let cp = c as u32;
    let i = IDNA.partition_point(|e| e.1 < cp);
    let Some(&(start, _, status, off, len)) = IDNA.get(i).filter(|e| e.0 <= cp) else {
        return (Status::Disallowed, "");
    };
    let _ = start;
    let status = match status {
        0 => Status::Valid,
        1 => Status::Mapped,
        2 => Status::Deviation,
        3 => Status::Disallowed,
        _ => Status::Ignored,
    };
    let off = off as usize;
    (status, &IDNA_MAPPING[off..off + usize::from(len)])
}

fn range_value(table: &[(u32, u32, u8)], c: char, default: u8) -> u8 {
    let cp = c as u32;
    let i = table.partition_point(|e| e.1 < cp);
    table.get(i).filter(|e| e.0 <= cp).map_or(default, |e| e.2)
}

// Índices de `BIDI` en tables.rs.
const L: u8 = 0;
const R: u8 = 1;
const AL: u8 = 2;
const AN: u8 = 3;
const EN: u8 = 4;
const ES: u8 = 5;
const CS: u8 = 6;
const ET: u8 = 7;
const ON: u8 = 8;
const BN: u8 = 9;
const NSM: u8 = 10;

fn bidi(c: char) -> u8 {
    range_value(BIDI, c, L)
}

// Joining_Type en tables.rs.
const JT_D: u8 = 1;
const JT_L: u8 = 2;
const JT_R: u8 = 3;
const JT_T: u8 = 4;

fn joining(c: char) -> u8 {
    range_value(JOINING, c, 0)
}

const VIRAMA: u8 = 9;

/// RFC 5892 Apéndice A.1 y A.2 (ZWNJ y ZWJ).
fn check_joiners(label: &[char]) -> bool {
    for (i, &c) in label.iter().enumerate() {
        if c != '\u{200C}' && c != '\u{200D}' {
            continue;
        }
        if i > 0 && canonical_combining_class(label[i - 1]) == VIRAMA {
            continue;
        }
        if c == '\u{200D}' {
            return false;
        }
        // (Joining_Type:{L,D})(Joining_Type:T)*‌(Joining_Type:T)*(Joining_Type:{R,D})
        let before = label[..i].iter().rev().find(|&&x| joining(x) != JT_T);
        let after = label[i + 1..].iter().find(|&&x| joining(x) != JT_T);
        let ok_before = before.is_some_and(|&x| matches!(joining(x), JT_L | JT_D));
        let ok_after = after.is_some_and(|&x| matches!(joining(x), JT_R | JT_D));
        if !(ok_before && ok_after) {
            return false;
        }
    }
    true
}

/// RFC 5893 §2, reglas 1 a 6.
fn check_bidi_label(label: &[char]) -> bool {
    let Some(&first) = label.first() else {
        return true;
    };
    let classes: Vec<u8> = label.iter().map(|&c| bidi(c)).collect();
    let last_non_nsm = classes.iter().rev().find(|&&c| c != NSM).copied();
    match bidi(first) {
        R | AL => {
            // Regla 2, 3 y 4.
            if !classes
                .iter()
                .all(|&c| matches!(c, R | AL | AN | EN | ES | CS | ET | ON | BN | NSM))
            {
                return false;
            }
            if !matches!(last_non_nsm, Some(R | AL | EN | AN)) {
                return false;
            }
            !(classes.contains(&EN) && classes.contains(&AN))
        }
        L => {
            // Reglas 5 y 6.
            classes
                .iter()
                .all(|&c| matches!(c, L | EN | ES | CS | ET | ON | BN | NSM))
                && matches!(last_non_nsm, Some(L | EN))
        }
        _ => false,
    }
}

/// Criterios de validez de UTS #46 §4.1 (con los parámetros del URL Standard).
fn valid_label(label: &[char]) -> bool {
    if label.is_empty() {
        return true;
    }
    let s: String = label.iter().collect();
    if s.nfc().ne(label.iter().copied()) {
        return false;
    }
    // CheckHyphens=false: solo se prohíbe el prefijo "xn--".
    if has_xn_prefix(&s) {
        return false;
    }
    if label.contains(&'.') || is_combining_mark(label[0]) {
        return false;
    }
    if !label
        .iter()
        .all(|&c| matches!(lookup(c).0, Status::Valid | Status::Deviation))
    {
        return false;
    }
    check_joiners(label)
}

/// ¿Empieza con `xn--` (sin distinguir mayúsculas)?
pub fn has_xn_prefix(s: &str) -> bool {
    s.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("xn--"))
}

/// UTS #46 ToASCII. `None` si hubo algún error.
pub fn to_ascii(domain: &str) -> Option<String> {
    // 1. Mapeo (no transicional).
    let mut error = false;
    let mut mapped = String::with_capacity(domain.len());
    for c in domain.chars() {
        let (status, target) = lookup(c);
        match status {
            Status::Valid | Status::Deviation => mapped.push(c),
            Status::Mapped => mapped.push_str(target),
            Status::Ignored => {}
            Status::Disallowed => {
                error = true;
                mapped.push(c);
            }
        }
    }
    // 2. Normalización NFC.
    let normalized: String = mapped.nfc().collect();
    // 3 y 4. Etiquetas: decodificación de `xn--` y validación.
    let mut labels: Vec<Vec<char>> = Vec::new();
    for label in normalized.split('.') {
        if has_xn_prefix(label) {
            if !label.is_ascii() {
                error = true;
                labels.push(label.chars().collect());
                continue;
            }
            match punycode_decode(&label[4..]) {
                Some(decoded) if !decoded.is_empty() && !decoded.iter().all(char::is_ascii) => {
                    if !valid_label(&decoded) {
                        error = true;
                    }
                    labels.push(decoded);
                }
                _ => {
                    error = true;
                    labels.push(label.chars().collect());
                }
            }
        } else {
            let chars: Vec<char> = label.chars().collect();
            if !valid_label(&chars) {
                error = true;
            }
            labels.push(chars);
        }
    }
    // CheckBidi: si es un "Bidi domain name" (alguna etiqueta tiene R, AL o AN).
    let bidi_domain = labels
        .iter()
        .flatten()
        .any(|&c| matches!(bidi(c), R | AL | AN));
    if bidi_domain && !labels.iter().all(|l| check_bidi_label(l)) {
        error = true;
    }
    if error {
        return None;
    }
    // 5. Codificación.
    let mut out = String::with_capacity(normalized.len());
    for (i, label) in labels.iter().enumerate() {
        if i > 0 {
            out.push('.');
        }
        if label.iter().all(char::is_ascii) {
            out.extend(label.iter());
        } else {
            out.push_str("xn--");
            out.push_str(&punycode_encode(label)?);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn punycode() {
        let s: Vec<char> = "münchen".chars().collect();
        assert_eq!(punycode_encode(&s).as_deref(), Some("mnchen-3ya"));
        assert_eq!(punycode_decode("mnchen-3ya"), Some(s));
        assert_eq!(
            to_ascii("Bücher.example").as_deref(),
            Some("xn--bcher-kva.example")
        );
        assert_eq!(to_ascii("faß.de").as_deref(), Some("xn--fa-hia.de"));
        assert_eq!(to_ascii("a\u{200D}b"), None);
    }
}
