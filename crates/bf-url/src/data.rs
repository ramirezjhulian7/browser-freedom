//! `data:` URLs (Fetch Standard §4.2 "data: URL processor") y forgiving-base64 (Infra §4.6).

use crate::Url;
use crate::percent;

/// Resultado de procesar una URL `data:`: tipo MIME serializado y cuerpo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataUrl {
    pub mime_type: String,
    pub body: Vec<u8>,
}

const ASCII_WHITESPACE: [char; 5] = ['\t', '\n', '\x0C', '\r', ' '];

/// Forgiving-base64 decode (Infra §4.6). `None` = falla.
pub fn forgiving_base64_decode(input: &str) -> Option<Vec<u8>> {
    let mut data: Vec<u8> = input
        .bytes()
        .filter(|b| !matches!(b, b'\t' | b'\n' | b'\x0C' | b'\r' | b' '))
        .collect();
    if data.len().is_multiple_of(4) {
        if data.ends_with(b"==") {
            data.truncate(data.len() - 2);
        } else if data.ends_with(b"=") {
            data.truncate(data.len() - 1);
        }
    }
    if data.len() % 4 == 1 {
        return None;
    }
    let value = |b: u8| -> Option<u32> {
        Some(match b {
            b'A'..=b'Z' => u32::from(b - b'A'),
            b'a'..=b'z' => u32::from(b - b'a') + 26,
            b'0'..=b'9' => u32::from(b - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    };
    let mut out = Vec::with_capacity(data.len() * 3 / 4);
    let (mut buffer, mut bits) = (0u32, 0);
    for b in data {
        buffer = (buffer << 6) | value(b)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Data: URL processor. `None` si la URL no tiene coma (falla del spec).
pub fn process(url: &Url) -> Option<DataUrl> {
    if url.scheme != "data" {
        return None;
    }
    let serialized = url.serialize(true);
    let input = &serialized["data:".len()..];
    let comma = input.find(',')?;
    let mut mime = input[..comma].trim_matches(ASCII_WHITESPACE).to_owned();
    let encoded_body = &input[comma + 1..];
    let mut body = percent::decode(encoded_body.as_bytes());
    // `;base64` al final (con espacios opcionales antes y después del `;`).
    let trimmed = mime.trim_end_matches(ASCII_WHITESPACE);
    if let Some(semi) = trimmed.rfind(';') {
        let tail = trimmed[semi + 1..].trim_start_matches(ASCII_WHITESPACE);
        if tail.eq_ignore_ascii_case("base64") {
            let text = String::from_utf8_lossy(&body).into_owned();
            body = forgiving_base64_decode(&text)?;
            mime = trimmed[..semi].to_owned();
        }
    }
    if mime.starts_with(';') {
        mime.insert_str(0, "text/plain");
    }
    let mime_type =
        parse_mime_type(&mime).unwrap_or_else(|| "text/plain;charset=US-ASCII".to_owned());
    Some(DataUrl { mime_type, body })
}

/// Parser de MIME type (MIME Sniffing §4.4), simplificado: `tipo/subtipo` en minúsculas más
/// los parámetros tal cual. `None` si no es válido.
pub fn parse_mime_type(input: &str) -> Option<String> {
    let input = input.trim_matches(|c: char| matches!(c, '\t' | '\n' | '\r' | ' ' | '\x0C'));
    let (essence, params) = match input.find(';') {
        Some(i) => (&input[..i], &input[i..]),
        None => (input, ""),
    };
    let (t, s) = essence.split_once('/')?;
    let token = |x: &str| {
        !x.is_empty()
            && x.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
    };
    let s = s.trim_end_matches(ASCII_WHITESPACE);
    if !token(t) || !token(s) {
        return None;
    }
    let mut out = format!("{}/{}", t.to_ascii_lowercase(), s.to_ascii_lowercase());
    for p in params.split(';').skip(1) {
        let p = p.trim_start_matches(ASCII_WHITESPACE);
        if let Some((name, value)) = p.split_once('=')
            && token(name)
            && !value.is_empty()
        {
            out.push(';');
            out.push_str(&name.to_ascii_lowercase());
            out.push('=');
            out.push_str(value);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_urls() {
        let u = Url::parse("data:text/plain;base64,SGVsbG8=", None).unwrap();
        let d = process(&u).unwrap();
        assert_eq!(d.body, b"Hello");
        assert_eq!(d.mime_type, "text/plain");
        let u = Url::parse("data:,a%20b", None).unwrap();
        let d = process(&u).unwrap();
        assert_eq!(d.body, b"a b");
        assert_eq!(d.mime_type, "text/plain;charset=US-ASCII");
        let u = Url::parse("data:image/PNG;charset=x,x", None).unwrap();
        assert_eq!(process(&u).unwrap().mime_type, "image/png;charset=x");
        assert_eq!(forgiving_base64_decode("YQ"), Some(b"a".to_vec()));
        assert_eq!(forgiving_base64_decode("Y"), None);
    }
}
