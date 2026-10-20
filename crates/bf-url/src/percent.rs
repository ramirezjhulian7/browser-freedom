//! Percent-encoding (URL Standard §1.3).

/// Conjuntos de percent-encode del spec, de menor a mayor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeSet {
    C0Control,
    Fragment,
    Query,
    SpecialQuery,
    Path,
    Userinfo,
    Component,
    FormUrlencoded,
}

impl EncodeSet {
    /// ¿`c` (un byte o code point) pertenece al conjunto?
    pub fn contains(self, c: u32) -> bool {
        let c0 = !(0x20..=0x7E).contains(&c);
        let ch = char::from_u32(c).unwrap_or('\0');
        let fragment = c0 || matches!(ch, ' ' | '"' | '<' | '>' | '`');
        let query = c0 || matches!(ch, ' ' | '"' | '#' | '<' | '>');
        let special_query = query || ch == '\'';
        let path = query || matches!(ch, '?' | '^' | '`' | '{' | '}');
        let userinfo = path || matches!(ch, '/' | ':' | ';' | '=' | '@' | '[' | '\\' | ']' | '|');
        let component = userinfo || matches!(ch, '$' | '%' | '&' | '+' | ',');
        let form = component || matches!(ch, '!' | '\'' | '(' | ')' | '~');
        match self {
            EncodeSet::C0Control => c0,
            EncodeSet::Fragment => fragment,
            EncodeSet::Query => query,
            EncodeSet::SpecialQuery => special_query,
            EncodeSet::Path => path,
            EncodeSet::Userinfo => userinfo,
            EncodeSet::Component => component,
            EncodeSet::FormUrlencoded => form,
        }
    }
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// Percent-encode de un byte.
pub fn push_byte(out: &mut String, b: u8) {
    out.push('%');
    out.push(HEX[usize::from(b >> 4)] as char);
    out.push(HEX[usize::from(b & 0xF)] as char);
}

/// UTF-8 percent-encode de un code point con `set`.
pub fn utf8_encode_char(out: &mut String, c: char, set: EncodeSet) {
    if set.contains(c as u32) {
        let mut buf = [0u8; 4];
        for &b in c.encode_utf8(&mut buf).as_bytes() {
            push_byte(out, b);
        }
    } else {
        out.push(c);
    }
}

/// UTF-8 percent-encode de una cadena con `set`.
pub fn utf8_encode(input: &str, set: EncodeSet) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        utf8_encode_char(&mut out, c, set);
    }
    out
}

/// Percent-encode de bytes ya codificados (`space_as_plus` para form-urlencoded).
pub fn encode_bytes(bytes: &[u8], set: EncodeSet, space_as_plus: bool) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        if space_as_plus && b == b' ' {
            out.push('+');
        } else if set.contains(u32::from(b)) {
            push_byte(&mut out, b);
        } else {
            out.push(b as char);
        }
    }
    out
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Percent-decode (§1.3): los `%XX` válidos pasan a bytes, el resto queda igual.
pub fn decode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%'
            && let (Some(h), Some(l)) = (
                input.get(i + 1).copied().and_then(hex_value),
                input.get(i + 2).copied().and_then(hex_value),
            )
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(input[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets() {
        assert_eq!(
            utf8_encode("a b\"#<>?^`{}", EncodeSet::Path),
            "a%20b%22%23%3C%3E%3F%5E%60%7B%7D"
        );
        assert_eq!(utf8_encode("ñ", EncodeSet::C0Control), "%C3%B1");
        assert_eq!(decode(b"%41%4g%"), b"A%4g%");
    }
}
