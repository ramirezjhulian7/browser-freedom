//! Tipo MIME de `Content-Type` y encoding de un documento HTML (HTML §13.2.3: BOM →
//! `charset` del transporte → prescan de `<meta>` en los primeros 1024 bytes → windows-1252).

use bf_infra::encoding::{Encoding, sniff_bom};

/// Tipo MIME parseado (MIME Sniffing §4.4, simplificado).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MimeType {
    /// `tipo/subtipo` en minúsculas.
    pub essence: String,
    pub params: Vec<(String, String)>,
}

impl MimeType {
    pub fn parse(input: &str) -> Option<MimeType> {
        let mut parts = input.split(';');
        let essence = parts.next()?.trim().to_ascii_lowercase();
        let (t, s) = essence.split_once('/')?;
        if t.is_empty() || s.is_empty() || t.contains(char::is_whitespace) {
            return None;
        }
        let params = parts
            .filter_map(|p| {
                let (n, v) = p.split_once('=')?;
                let v = v.trim();
                let v = v
                    .strip_prefix('"')
                    .and_then(|x| x.strip_suffix('"'))
                    .unwrap_or(v);
                Some((n.trim().to_ascii_lowercase(), v.to_owned()))
            })
            .collect();
        Some(MimeType { essence, params })
    }

    pub fn charset(&self) -> Option<&str> {
        self.params
            .iter()
            .find(|(n, _)| n == "charset")
            .map(|(_, v)| v.as_str())
    }

    pub fn is_html(&self) -> bool {
        self.essence == "text/html"
    }

    pub fn is_xml(&self) -> bool {
        self.essence.ends_with("+xml")
            || matches!(self.essence.as_str(), "text/xml" | "application/xml")
    }
}

/// "Extracting a character encoding from a meta element" (HTML §2.5.3 / §13.2.3.3).
pub fn charset_from_content(content: &str) -> Option<Encoding> {
    let lower = content.to_ascii_lowercase();
    let mut pos = 0;
    loop {
        let i = lower[pos..].find("charset")? + pos + "charset".len();
        let rest = lower[i..].trim_start_matches(|c: char| c.is_ascii_whitespace());
        let Some(rest) = rest.strip_prefix('=') else {
            pos = i;
            continue;
        };
        let rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace());
        let value = if let Some(q) = rest.strip_prefix(['"', '\'']) {
            let quote = rest.as_bytes()[0] as char;
            &q[..q.find(quote)?]
        } else {
            let end = rest
                .find(|c: char| c.is_ascii_whitespace() || c == ';')
                .unwrap_or(rest.len());
            &rest[..end]
        };
        return Encoding::for_label(value);
    }
}

/// "Get an attribute" del prescan (§13.2.3.3): (nombre, valor) o `None` al llegar a `>`.
fn get_attribute(bytes: &[u8], pos: &mut usize) -> Option<(String, String)> {
    let at = |p: usize| bytes.get(p).copied();
    while matches!(at(*pos), Some(0x09 | 0x0A | 0x0C | 0x0D | 0x20 | b'/')) {
        *pos += 1;
    }
    if matches!(at(*pos), None | Some(b'>')) {
        return None;
    }
    let mut name = String::new();
    let mut value = String::new();
    loop {
        match at(*pos)? {
            b'=' if !name.is_empty() => {
                *pos += 1;
                break;
            }
            0x09 | 0x0A | 0x0C | 0x0D | 0x20 => {
                while matches!(at(*pos), Some(0x09 | 0x0A | 0x0C | 0x0D | 0x20)) {
                    *pos += 1;
                }
                if at(*pos) != Some(b'=') {
                    return Some((name, value));
                }
                *pos += 1;
                break;
            }
            b'/' | b'>' => return Some((name, value)),
            c => {
                name.push(char::from(c.to_ascii_lowercase()));
                *pos += 1;
            }
        }
    }
    while matches!(at(*pos), Some(0x09 | 0x0A | 0x0C | 0x0D | 0x20)) {
        *pos += 1;
    }
    match at(*pos)? {
        q @ (b'"' | b'\'') => {
            *pos += 1;
            loop {
                let c = at(*pos)?;
                *pos += 1;
                if c == q {
                    return Some((name, value));
                }
                value.push(char::from(c.to_ascii_lowercase()));
            }
        }
        b'>' => Some((name, value)),
        _ => loop {
            match at(*pos) {
                None => return Some((name, value)),
                Some(0x09 | 0x0A | 0x0C | 0x0D | 0x20 | b'>') => return Some((name, value)),
                Some(c) => {
                    value.push(char::from(c.to_ascii_lowercase()));
                    *pos += 1;
                }
            }
        },
    }
}

/// Prescan de los primeros 1024 bytes buscando `<meta charset>` o
/// `<meta http-equiv=content-type content="…charset=…">` (§13.2.3.3).
pub fn prescan(input: &[u8]) -> Option<Encoding> {
    let bytes = &input[..input.len().min(1024)];
    let mut pos = 0;
    let starts = |p: usize, s: &[u8]| {
        bytes
            .get(p..p + s.len())
            .is_some_and(|b| b.eq_ignore_ascii_case(s))
    };
    while pos < bytes.len() {
        if starts(pos, b"<!--") {
            pos += 4;
            while pos < bytes.len() && !starts(pos, b"-->") {
                pos += 1;
            }
            pos += 3;
            continue;
        }
        if starts(pos, b"<meta")
            && matches!(
                bytes.get(pos + 5),
                Some(0x09 | 0x0A | 0x0C | 0x0D | 0x20 | b'/')
            )
        {
            pos += 6;
            let (mut got_pragma, mut need_pragma) = (false, None::<bool>);
            let mut charset: Option<Encoding> = None;
            let mut seen: Vec<String> = Vec::new();
            while let Some((name, value)) = get_attribute(bytes, &mut pos) {
                if seen.contains(&name) {
                    continue;
                }
                seen.push(name.clone());
                match name.as_str() {
                    "http-equiv" if value == "content-type" => got_pragma = true,
                    "content" if charset.is_none() => {
                        if let Some(e) = charset_from_content(&value) {
                            charset = Some(e);
                            need_pragma = Some(true);
                        }
                    }
                    "charset" => {
                        charset = Encoding::for_label(&value);
                        need_pragma = Some(false);
                    }
                    _ => {}
                }
            }
            let ok = match need_pragma {
                None => false,
                Some(true) => got_pragma,
                Some(false) => true,
            };
            if ok && let Some(e) = charset {
                return Some(match e {
                    Encoding::Utf16Le | Encoding::Utf16Be => Encoding::Utf8,
                    Encoding::XUserDefined => Encoding::windows_1252(),
                    e => e,
                });
            }
            continue;
        }
        if bytes[pos] == b'<'
            && (bytes.get(pos + 1).is_some_and(u8::is_ascii_alphabetic)
                || (bytes.get(pos + 1) == Some(&b'/')
                    && bytes.get(pos + 2).is_some_and(u8::is_ascii_alphabetic)))
        {
            // Una etiqueta: se saltan su nombre y sus atributos.
            pos += 1;
            while pos < bytes.len()
                && !matches!(bytes[pos], 0x09 | 0x0A | 0x0C | 0x0D | 0x20 | b'>')
            {
                pos += 1;
            }
            while get_attribute(bytes, &mut pos).is_some() {}
            pos += 1;
            continue;
        }
        if starts(pos, b"<!") || starts(pos, b"</") || starts(pos, b"<?") {
            while pos < bytes.len() && bytes[pos] != b'>' {
                pos += 1;
            }
            pos += 1;
            continue;
        }
        pos += 1;
    }
    None
}

/// Encoding de un documento HTML (§13.2.3.2) y si se lo sacó del BOM.
pub fn html_encoding(bytes: &[u8], content_type: Option<&MimeType>) -> Encoding {
    html_encoding_with_source(bytes, content_type).0
}

/// De dónde salió el encoding de un documento (HTML §13.2.3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodingSource {
    Bom,
    /// El parámetro `charset` del `Content-Type`.
    ContentType,
    /// El prescan de `<meta charset>` / `http-equiv`.
    Meta,
    /// El default del locale.
    Default,
}

impl EncodingSource {
    pub fn name(self) -> &'static str {
        match self {
            EncodingSource::Bom => "BOM",
            EncodingSource::ContentType => "cabecera Content-Type",
            EncodingSource::Meta => "<meta> (prescan)",
            EncodingSource::Default => "default del locale",
        }
    }
}

/// [`html_encoding`] con el paso del algoritmo que lo decidió.
pub fn html_encoding_with_source(
    bytes: &[u8],
    content_type: Option<&MimeType>,
) -> (Encoding, EncodingSource) {
    if let Some((e, _)) = sniff_bom(bytes) {
        return (e, EncodingSource::Bom);
    }
    if let Some(e) = content_type
        .and_then(MimeType::charset)
        .and_then(Encoding::for_label)
    {
        return (e, EncodingSource::ContentType);
    }
    if let Some(e) = prescan(bytes) {
        return (e, EncodingSource::Meta);
    }
    // Default para locales no listados en la tabla de §13.2.3.2 (incluido el español).
    (Encoding::windows_1252(), EncodingSource::Default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prescan_meta() {
        assert_eq!(
            prescan(b"<meta charset=\"ISO-8859-2\">").map(|e| e.name()),
            Some("ISO-8859-2")
        );
        assert_eq!(
            prescan(b"<!-- <meta charset=koi8-r> --><head><meta http-equiv='Content-Type' content='text/html; charset=windows-1251'>")
                .map(|e| e.name()),
            Some("windows-1251")
        );
        // content sin http-equiv no cuenta.
        assert_eq!(prescan(b"<meta content='text/html; charset=koi8-r'>"), None);
        assert_eq!(prescan(b"<meta charset=utf-16le>"), Some(Encoding::Utf8));
        assert_eq!(
            prescan(b"<title a='<meta charset=koi8-r>'></title><meta charset=utf-8>"),
            Some(Encoding::Utf8)
        );
        let ct = MimeType::parse("text/html; charset=\"KOI8-R\"").unwrap();
        assert_eq!(
            html_encoding(b"<meta charset=utf-8>", Some(&ct)).name(),
            "KOI8-R"
        );
        assert_eq!(html_encoding(b"\xEF\xBB\xBFx", Some(&ct)), Encoding::Utf8);
        assert_eq!(html_encoding(b"<p>", None).name(), "windows-1252");
    }
}
