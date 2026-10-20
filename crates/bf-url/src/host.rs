//! Hosts (URL Standard §3): dominios, IPv4, IPv6, hosts opacos y el host vacío.

use std::fmt;

use crate::idna;
use crate::percent::{self, EncodeSet};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Host {
    Domain(String),
    Ipv4(u32),
    Ipv6([u16; 8]),
    Opaque(String),
    Empty,
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Host::Domain(d) | Host::Opaque(d) => f.write_str(d),
            Host::Empty => Ok(()),
            Host::Ipv4(a) => write!(
                f,
                "{}.{}.{}.{}",
                a >> 24,
                (a >> 16) & 0xFF,
                (a >> 8) & 0xFF,
                a & 0xFF
            ),
            Host::Ipv6(pieces) => {
                f.write_str("[")?;
                serialize_ipv6(pieces, f)?;
                f.write_str("]")
            }
        }
    }
}

/// §3.7: la primera secuencia más larga (de 2 o más) de piezas en cero se comprime.
fn serialize_ipv6(pieces: &[u16; 8], f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let mut compress: Option<(usize, usize)> = None;
    let mut i = 0;
    while i < 8 {
        if pieces[i] == 0 {
            let start = i;
            while i < 8 && pieces[i] == 0 {
                i += 1;
            }
            let len = i - start;
            if len > 1 && compress.is_none_or(|(_, l)| len > l) {
                compress = Some((start, len));
            }
        } else {
            i += 1;
        }
    }
    let mut ignore0 = false;
    for (i, piece) in pieces.iter().enumerate() {
        if ignore0 && *piece == 0 {
            continue;
        }
        ignore0 = false;
        if let Some((start, _)) = compress
            && start == i
        {
            f.write_str(if i == 0 { "::" } else { ":" })?;
            ignore0 = true;
            continue;
        }
        write!(f, "{piece:x}")?;
        if i != 7 {
            f.write_str(":")?;
        }
    }
    Ok(())
}

fn is_forbidden_host(c: char) -> bool {
    matches!(
        c,
        '\0' | '\t'
            | '\n'
            | '\r'
            | ' '
            | '#'
            | '/'
            | ':'
            | '<'
            | '>'
            | '?'
            | '@'
            | '['
            | '\\'
            | ']'
            | '^'
            | '|'
    )
}

fn is_forbidden_domain(c: char) -> bool {
    is_forbidden_host(c) || (c as u32) <= 0x1F || c == '%' || c == '\u{7F}'
}

/// Host parser (§3.5). `opaque` = el esquema no es especial.
pub fn parse_host(input: &str, opaque: bool) -> Option<Host> {
    if let Some(rest) = input.strip_prefix('[') {
        let inner = rest.strip_suffix(']')?;
        return parse_ipv6(inner).map(Host::Ipv6);
    }
    if opaque {
        if input.chars().any(is_forbidden_host) {
            return None;
        }
        // El opaque host vacío es el empty host.
        if input.is_empty() {
            return Some(Host::Empty);
        }
        return Some(Host::Opaque(percent::utf8_encode(
            input,
            EncodeSet::C0Control,
        )));
    }
    let decoded = percent::decode(input.as_bytes());
    let domain = String::from_utf8_lossy(&decoded);
    let ascii = domain_to_ascii(&domain)?;
    if ascii.chars().any(is_forbidden_domain) {
        return None;
    }
    if ends_in_number(&ascii) {
        return parse_ipv4(&ascii).map(Host::Ipv4);
    }
    Some(Host::Domain(ascii))
}

/// Domain to ASCII (§3.3) con `beStrict = false`. Un dominio ASCII solo se pasa a
/// minúsculas: sus etiquetas `xn--` no se decodifican ni validan (WPT toascii.json, 2026:
/// "Invalid Punycode" en ASCII se conserva).
pub fn domain_to_ascii(domain: &str) -> Option<String> {
    let result = if domain.is_ascii() {
        domain.to_ascii_lowercase()
    } else {
        idna::to_ascii(domain)?
    };
    (!result.is_empty()).then_some(result)
}

/// §3.5 "ends in a number".
fn ends_in_number(input: &str) -> bool {
    let mut parts: Vec<&str> = input.split('.').collect();
    if parts.last() == Some(&"") {
        if parts.len() == 1 {
            return false;
        }
        parts.pop();
    }
    let last = parts.last().copied().unwrap_or("");
    if !last.is_empty() && last.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    parse_ipv4_number(last).is_some()
}

/// IPv4 number parser (§3.5): decimal, octal (`0…`) o hexadecimal (`0x…`).
fn parse_ipv4_number(mut input: &str) -> Option<u64> {
    if input.is_empty() {
        return None;
    }
    let mut radix = 10;
    if input.len() >= 2 && (input.starts_with("0x") || input.starts_with("0X")) {
        input = &input[2..];
        radix = 16;
    } else if input.len() >= 2 && input.starts_with('0') {
        input = &input[1..];
        radix = 8;
    }
    if input.is_empty() {
        return Some(0);
    }
    if !input.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    // Saturado: lo que no cabe en u64 es demasiado grande igual.
    Some(input.chars().fold(0u64, |acc, c| {
        acc.saturating_mul(u64::from(radix))
            .saturating_add(u64::from(c.to_digit(radix).unwrap_or(0)))
    }))
}

/// IPv4 parser (§3.5).
fn parse_ipv4(input: &str) -> Option<u32> {
    let mut parts: Vec<&str> = input.split('.').collect();
    if parts.last() == Some(&"") && parts.len() > 1 {
        parts.pop();
    }
    if parts.len() > 4 {
        return None;
    }
    let numbers: Vec<u64> = parts
        .iter()
        .map(|p| parse_ipv4_number(p))
        .collect::<Option<_>>()?;
    let (last, rest) = numbers.split_last()?;
    if rest.iter().any(|&n| n > 255) {
        return None;
    }
    if *last >= 256u64.pow(5 - numbers.len() as u32) {
        return None;
    }
    let mut ipv4 = *last;
    for (i, &n) in rest.iter().enumerate() {
        ipv4 += n * 256u64.pow(3 - i as u32);
    }
    u32::try_from(ipv4).ok()
}

/// IPv6 parser (§3.5).
fn parse_ipv6(input: &str) -> Option<[u16; 8]> {
    let s: Vec<char> = input.chars().collect();
    let at = |p: usize| s.get(p).copied();
    let mut address = [0u16; 8];
    let mut piece = 0usize;
    let mut compress: Option<usize> = None;
    let mut p = 0usize;
    if at(p) == Some(':') {
        if at(p + 1) != Some(':') {
            return None;
        }
        p += 2;
        piece += 1;
        compress = Some(piece);
    }
    while at(p).is_some() {
        if piece == 8 {
            return None;
        }
        if at(p) == Some(':') {
            if compress.is_some() {
                return None;
            }
            p += 1;
            piece += 1;
            compress = Some(piece);
            continue;
        }
        let (mut value, mut length) = (0u32, 0);
        while length < 4
            && let Some(d) = at(p).and_then(|c| c.to_digit(16))
        {
            value = value * 16 + d;
            p += 1;
            length += 1;
        }
        if at(p) == Some('.') {
            if length == 0 {
                return None;
            }
            p -= length;
            if piece > 6 {
                return None;
            }
            let mut numbers_seen = 0;
            while at(p).is_some() {
                let mut ipv4_piece: Option<u32> = None;
                if numbers_seen > 0 {
                    if at(p) == Some('.') && numbers_seen < 4 {
                        p += 1;
                    } else {
                        return None;
                    }
                }
                at(p)?.to_digit(10)?;
                while let Some(n) = at(p).and_then(|c| c.to_digit(10)) {
                    ipv4_piece = match ipv4_piece {
                        None => Some(n),
                        Some(0) => return None,
                        Some(v) => Some(v * 10 + n),
                    };
                    if ipv4_piece? > 255 {
                        return None;
                    }
                    p += 1;
                }
                address[piece] = (u32::from(address[piece]) * 0x100 + ipv4_piece?) as u16;
                numbers_seen += 1;
                if numbers_seen == 2 || numbers_seen == 4 {
                    piece += 1;
                }
            }
            if numbers_seen != 4 {
                return None;
            }
            break;
        } else if at(p) == Some(':') {
            p += 1;
            at(p)?;
        } else if at(p).is_some() {
            return None;
        }
        address[piece] = value as u16;
        piece += 1;
    }
    if let Some(c) = compress {
        let mut swaps = piece - c;
        piece = 7;
        while piece != 0 && swaps > 0 {
            address.swap(piece, c + swaps - 1);
            piece -= 1;
            swaps -= 1;
        }
    } else if piece != 8 {
        return None;
    }
    Some(address)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts() {
        let h = |s: &str| parse_host(s, false).map(|h| h.to_string());
        assert_eq!(h("EXAMPLE.com").as_deref(), Some("example.com"));
        assert_eq!(h("0x7f.1").as_deref(), Some("127.0.0.1"));
        assert_eq!(h("[::1]").as_deref(), Some("[::1]"));
        assert_eq!(h("[1:0:0:2::3:0]").as_deref(), Some("[1::2:0:0:3:0]"));
        assert_eq!(h("[::127.0.0.1]").as_deref(), Some("[::7f00:1]"));
        assert_eq!(h("1.2.3.4.5"), None);
        assert_eq!(h("a b"), None);
    }
}
