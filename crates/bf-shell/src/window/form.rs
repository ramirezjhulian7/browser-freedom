//! El cuerpo del envío de un formulario según su `enctype` (HTML §4.10.21.3 paso "Submit as
//! entity body"): `application/x-www-form-urlencoded` (§4.10.21.7), `multipart/form-data`
//! (§4.10.21.8, RFC 7578) y `text/plain` (§4.10.21.9).
//!
//! La lista de entradas la arma [`super::form_data`] (§4.10.21.4); aquí solo se codifica.

use std::sync::atomic::{AtomicU64, Ordering};

/// El valor de una entrada del conjunto de datos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Text(String),
    /// Un `<input type=file>` sin archivos elegidos: §4.10.21.4 pide un `File` de nombre vacío,
    /// tipo `application/octet-stream` y cuerpo vacío (Freedom todavía no elige archivos).
    EmptyFile,
}

/// Una entrada del conjunto de datos (*entry list*).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub value: Value,
}

impl Entry {
    pub fn text(name: &str, value: &str) -> Entry {
        Entry {
            name: name.to_owned(),
            value: Value::Text(value.to_owned()),
        }
    }
}

/// El atributo `enctype` (o `formenctype`) del envío (§4.10.18.6: atributo enumerado; sin
/// valor o con uno desconocido, `urlencoded`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enctype {
    UrlEncoded,
    Multipart,
    TextPlain,
}

impl Enctype {
    pub fn parse(value: Option<&str>) -> Enctype {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("multipart/form-data") => Enctype::Multipart,
            Some("text/plain") => Enctype::TextPlain,
            _ => Enctype::UrlEncoded,
        }
    }
}

/// Cambia cada CR sin LF detrás y cada LF sin CR delante por CRLF (§4.10.21.8 y la conversión
/// a pares nombre-valor de §4.10.21.4).
pub fn normalize_newlines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                out.push_str("\r\n");
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
            }
            '\n' => out.push_str("\r\n"),
            c => out.push(c),
        }
    }
    out
}

/// "Convert to a list of name-value pairs" (§4.10.21.4): un archivo vale su nombre (vacío) y
/// los saltos de línea pasan a CRLF.
fn pairs(entries: &[Entry]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|e| {
            let value = match &e.value {
                Value::Text(v) => normalize_newlines(v),
                Value::EmptyFile => String::new(),
            };
            (normalize_newlines(&e.name), value)
        })
        .collect()
}

/// `application/x-www-form-urlencoded` (§4.10.21.7, con el serializador de URL §5.2 en UTF-8).
pub fn urlencoded(entries: &[Entry]) -> String {
    bf_url::form::serialize(&pairs(entries))
}

/// `text/plain` (§4.10.21.9): `nombre=valor` CRLF por entrada, sin escapar nada.
pub fn text_plain(entries: &[Entry]) -> String {
    pairs(entries)
        .into_iter()
        .map(|(n, v)| format!("{n}={v}\r\n"))
        .collect()
}

/// Un nombre o nombre de archivo dentro de las comillas de `Content-Disposition`: §4.10.21.8
/// escapa LF como `%0A`, CR como `%0D` y `"` como `%22` (como hacen los navegadores).
fn quoted(s: &str) -> String {
    s.replace('\n', "%0A")
        .replace('\r', "%0D")
        .replace('"', "%22")
}

/// `multipart/form-data` (§4.10.21.8, RFC 7578) con el delimitador `boundary`: el cuerpo.
pub fn multipart_with(entries: &[Entry], boundary: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for e in entries {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        let name = quoted(&normalize_newlines(&e.name));
        match &e.value {
            Value::Text(v) => {
                out.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
                );
                out.extend_from_slice(normalize_newlines(v).as_bytes());
            }
            Value::EmptyFile => out.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"\"\r\n\
                     Content-Type: application/octet-stream\r\n\r\n"
                )
                .as_bytes(),
            ),
        }
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    out
}

/// Un delimitador nuevo (RFC 2046 §5.1.1: hasta 70 caracteres que no aparezcan en el cuerpo).
fn boundary(seed: u64) -> String {
    // splitmix64: suficiente para que no se repita; no tiene que ser impredecible.
    let mut x = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut next = || {
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let tail: String = (0..16)
        .map(|_| ALPHABET[(next() % ALPHABET.len() as u64) as usize] as char)
        .collect();
    format!("----FreedomFormBoundary{tail}")
}

/// `multipart/form-data`: el cuerpo y su `Content-Type` con el delimitador.
pub fn multipart(entries: &[Entry]) -> (Vec<u8>, String) {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    loop {
        let b = boundary(nanos ^ COUNTER.fetch_add(1, Ordering::Relaxed).rotate_left(32));
        let contains = entries
            .iter()
            .any(|e| e.name.contains(&b) || matches!(&e.value, Value::Text(v) if v.contains(&b)));
        if !contains {
            let body = multipart_with(entries, &b);
            return (body, format!("multipart/form-data; boundary={b}"));
        }
    }
}

/// El cuerpo y el `Content-Type` de un envío POST con `enctype`.
pub fn body(entries: &[Entry], enctype: Enctype) -> (Vec<u8>, String) {
    match enctype {
        Enctype::UrlEncoded => (
            urlencoded(entries).into_bytes(),
            "application/x-www-form-urlencoded".to_owned(),
        ),
        Enctype::Multipart => multipart(entries),
        Enctype::TextPlain => (
            text_plain(entries).into_bytes(),
            "text/plain;charset=UTF-8".to_owned(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<Entry> {
        vec![
            Entry::text("nombre", "José Ñ & \"amigos\""),
            Entry::text("nota", "uno\ndos\r\ntres\r"),
            Entry::text("con\"comillas", "x"),
            Entry {
                name: "archivo".into(),
                value: Value::EmptyFile,
            },
        ]
    }

    #[test]
    fn enctype_attribute() {
        assert_eq!(Enctype::parse(None), Enctype::UrlEncoded);
        assert_eq!(
            Enctype::parse(Some(" Multipart/Form-Data ")),
            Enctype::Multipart
        );
        assert_eq!(Enctype::parse(Some("text/plain")), Enctype::TextPlain);
        assert_eq!(Enctype::parse(Some("json")), Enctype::UrlEncoded);
    }

    #[test]
    fn urlencoded_normalizes_newlines() {
        assert_eq!(
            urlencoded(&entries()),
            "nombre=Jos%C3%A9+%C3%91+%26+%22amigos%22&nota=uno%0D%0Ados%0D%0Atres%0D%0A\
             &con%22comillas=x&archivo="
        );
    }

    #[test]
    fn text_plain_lines() {
        assert_eq!(
            text_plain(&entries()[..1]),
            "nombre=José Ñ & \"amigos\"\r\n"
        );
    }

    #[test]
    fn multipart_parts() {
        let body = String::from_utf8(multipart_with(&entries(), "XYZ")).unwrap();
        assert_eq!(
            body,
            "--XYZ\r\nContent-Disposition: form-data; name=\"nombre\"\r\n\r\nJosé Ñ & \"amigos\"\r\n\
             --XYZ\r\nContent-Disposition: form-data; name=\"nota\"\r\n\r\nuno\r\ndos\r\ntres\r\n\r\n\
             --XYZ\r\nContent-Disposition: form-data; name=\"con%22comillas\"\r\n\r\nx\r\n\
             --XYZ\r\nContent-Disposition: form-data; name=\"archivo\"; filename=\"\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n\r\n\
             --XYZ--\r\n"
        );
        let (body, ty) = multipart(&entries());
        let b = ty.strip_prefix("multipart/form-data; boundary=").unwrap();
        assert!(b.len() <= 70 && b.starts_with("----FreedomFormBoundary"));
        assert!(
            String::from_utf8(body)
                .unwrap()
                .ends_with(&format!("--{b}--\r\n"))
        );
        assert_ne!(multipart(&entries()).1, ty, "cada envío, otro delimitador");
    }
}
