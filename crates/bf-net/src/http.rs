//! HTTP/1.1 (RFC 9112): serialización de requests y parser de responses (status line,
//! cabeceras, cuerpo con `Content-Length`, `chunked` o hasta EOF) y descompresión (RFC 9110
//! §8.4: gzip, deflate y br).

use std::io::{self, BufRead, Read, Write};

/// Cabeceras en orden, con los nombres como llegaron (la búsqueda no distingue mayúsculas).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headers(pub Vec<(String, String)>);

impl Headers {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn get_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.0
            .iter()
            .filter(move |(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Valores de una cabecera de lista (separados por comas, de todas sus apariciones).
    pub fn list(&self, name: &str) -> Vec<String> {
        self.get_all(name)
            .flat_map(|v| v.split(','))
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect()
    }

    pub fn push(&mut self, name: &str, value: &str) {
        self.0.push((name.to_owned(), value.to_owned()));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub version: (u8, u8),
    pub status: u16,
    pub reason: String,
    pub headers: Headers,
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_owned())
}

/// Una línea sin el `\r\n` (o `\n` solo). `None` en EOF.
fn read_line(r: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    let n = r.read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    if buf.len() > 64 * 1024 {
        return Err(invalid("línea demasiado larga"));
    }
    while matches!(buf.last(), Some(b'\n' | b'\r')) {
        buf.pop();
    }
    // Las cabeceras son ISO-8859-1 en la práctica (RFC 9110 §5.5).
    Ok(Some(buf.iter().map(|&b| char::from(b)).collect()))
}

/// Status line y cabeceras. Tolera `\n` solo, líneas vacías antes de la status line,
/// cabeceras repetidas y obs-fold (líneas que empiezan con espacio).
pub fn read_head(r: &mut impl BufRead) -> io::Result<Head> {
    let mut line = loop {
        match read_line(r)? {
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "sin respuesta",
                ));
            }
            Some(l) if l.trim().is_empty() => continue,
            Some(l) => break l,
        }
    };
    if !line.starts_with("HTTP/") {
        return Err(invalid("status line inválida"));
    }
    let rest = line.split_off(5);
    let mut parts = rest.splitn(3, ' ');
    let version = parts.next().unwrap_or("1.1");
    let (major, minor) = version.split_once('.').unwrap_or((version, "0"));
    let version = (major.parse().unwrap_or(1), minor.parse().unwrap_or(1));
    let status: u16 = parts
        .next()
        .and_then(|s| s.trim().parse().ok())
        .ok_or_else(|| invalid("código de estado inválido"))?;
    let reason = parts.next().unwrap_or("").trim().to_owned();
    let mut headers = Headers::default();
    while let Some(l) = read_line(r)? {
        if l.is_empty() {
            break;
        }
        if l.starts_with([' ', '\t']) {
            if let Some(last) = headers.0.last_mut() {
                last.1.push(' ');
                last.1.push_str(l.trim());
            }
            continue;
        }
        if let Some((name, value)) = l.split_once(':') {
            let name = name.trim();
            if !name.is_empty() {
                headers.push(name, value.trim());
            }
        }
        if headers.0.len() > 1000 {
            return Err(invalid("demasiadas cabeceras"));
        }
    }
    Ok(Head {
        version,
        status,
        reason,
        headers,
    })
}

/// Cómo termina el cuerpo de una response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    None,
    Length(u64),
    Chunked,
    UntilEof,
}

/// RFC 9112 §6.3.
pub fn body_kind(head: &Head, request_was_head: bool) -> BodyKind {
    if request_was_head || (100..200).contains(&head.status) || matches!(head.status, 204 | 304) {
        return BodyKind::None;
    }
    if head
        .headers
        .list("transfer-encoding")
        .last()
        .map(String::as_str)
        == Some("chunked")
    {
        return BodyKind::Chunked;
    }
    if let Some(len) = head
        .headers
        .get_all("content-length")
        .filter_map(|v| v.split(',').next()?.trim().parse::<u64>().ok())
        .next()
    {
        return BodyKind::Length(len);
    }
    BodyKind::UntilEof
}

/// Lee el cuerpo. Los trailers del chunked se descartan.
pub fn read_body(r: &mut impl BufRead, kind: BodyKind, limit: u64) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    match kind {
        BodyKind::None => {}
        BodyKind::Length(n) => {
            if n > limit {
                return Err(invalid("cuerpo demasiado grande"));
            }
            out.resize(n as usize, 0);
            r.read_exact(&mut out)?;
        }
        BodyKind::UntilEof => {
            r.take(limit).read_to_end(&mut out)?;
        }
        BodyKind::Chunked => loop {
            let line = read_line(r)?.ok_or_else(|| invalid("chunked truncado"))?;
            let size_str = line.split(';').next().unwrap_or("").trim();
            let size = u64::from_str_radix(size_str, 16)
                .map_err(|_| invalid("tamaño de chunk inválido"))?;
            if size == 0 {
                // Trailers hasta la línea vacía.
                while let Some(l) = read_line(r)? {
                    if l.is_empty() {
                        break;
                    }
                }
                break;
            }
            if out.len() as u64 + size > limit {
                return Err(invalid("cuerpo demasiado grande"));
            }
            let start = out.len();
            out.resize(start + size as usize, 0);
            r.read_exact(&mut out[start..])?;
            // CRLF tras los datos.
            let _ = read_line(r)?;
        },
    }
    Ok(out)
}

/// Deshace `Content-Encoding` (las codificaciones se aplicaron en orden; se deshacen al
/// revés). Una codificación desconocida deja el cuerpo como está.
pub fn decode_content(body: Vec<u8>, encodings: &[String]) -> io::Result<Vec<u8>> {
    let mut data = body;
    for enc in encodings.iter().rev() {
        let mut out = Vec::new();
        match enc.as_str() {
            "gzip" | "x-gzip" => {
                flate2::read::MultiGzDecoder::new(&data[..]).read_to_end(&mut out)?;
            }
            "deflate" => {
                // "deflate" es zlib (RFC 9110 §8.4.1.2), pero hay servidores que mandan deflate
                // crudo.
                if flate2::read::ZlibDecoder::new(&data[..])
                    .read_to_end(&mut out)
                    .is_err()
                {
                    out.clear();
                    flate2::read::DeflateDecoder::new(&data[..]).read_to_end(&mut out)?;
                }
            }
            "br" => {
                brotli::Decompressor::new(&data[..], 4096).read_to_end(&mut out)?;
            }
            "identity" => continue,
            _ => return Ok(data),
        }
        data = out;
    }
    Ok(data)
}

/// Escribe un request HTTP/1.1.
pub fn write_request(
    w: &mut impl Write,
    method: &str,
    target: &str,
    headers: &Headers,
    body: Option<&[u8]>,
) -> io::Result<()> {
    let mut head = format!("{method} {target} HTTP/1.1\r\n");
    for (n, v) in &headers.0 {
        head.push_str(n);
        head.push_str(": ");
        head.push_str(v);
        head.push_str("\r\n");
    }
    if let Some(b) = body {
        head.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    head.push_str("\r\n");
    w.write_all(head.as_bytes())?;
    if let Some(b) = body {
        w.write_all(b)?;
    }
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heads_and_bodies() {
        let raw = b"\r\nHTTP/1.1 200 OK\nContent-Type: text/html\nX-A: 1\n  2\nset-cookie: a=1\nSet-Cookie: b=2\nTransfer-Encoding: gzip, chunked\n\n";
        let mut r = &raw[..];
        let head = read_head(&mut r).unwrap();
        assert_eq!(head.status, 200);
        assert_eq!(head.headers.get("x-a"), Some("1 2"));
        assert_eq!(head.headers.get_all("set-cookie").count(), 2);
        assert_eq!(body_kind(&head, false), BodyKind::Chunked);
        let chunked = b"4;ext=1\r\nWiki\r\n5\r\npedia\r\n0\r\nTrailer: x\r\n\r\n";
        let mut r = &chunked[..];
        assert_eq!(
            read_body(&mut r, BodyKind::Chunked, 1 << 20).unwrap(),
            b"Wikipedia"
        );
        let head = read_head(&mut &b"HTTP/1.0 304 Not Modified\r\n\r\n"[..]).unwrap();
        assert_eq!(body_kind(&head, false), BodyKind::None);
    }

    #[test]
    fn content_encodings() {
        use std::io::Write as _;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(b"hola").unwrap();
        let data = gz.finish().unwrap();
        assert_eq!(decode_content(data, &["gzip".into()]).unwrap(), b"hola");
        let mut z = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(b"raw").unwrap();
        assert_eq!(
            decode_content(z.finish().unwrap(), &["deflate".into()]).unwrap(),
            b"raw"
        );
        let mut br = Vec::new();
        {
            let mut w = brotli::CompressorWriter::new(&mut br, 4096, 5, 22);
            w.write_all(b"brotli").unwrap();
        }
        assert_eq!(decode_content(br, &["br".into()]).unwrap(), b"brotli");
    }
}
