//! Carga de recursos (hojas de estilo, fuentes, imágenes) por URL. `FileLoader` atiende `file:`
//! y `data:`, y puede montar un prefijo de URL sobre un directorio (el harness de WPT sirve
//! `tests/wpt` como `http://web-platform.test/`). La red (`http:`/`https:`) está en `bf-net`.

use std::path::PathBuf;

use bf_url::Url;

/// Un recurso cargado.
#[derive(Debug, Clone)]
pub struct Resource {
    pub bytes: Vec<u8>,
    /// URL final (después de redirecciones): base de las referencias que contenga.
    pub url: Url,
    /// `Content-Type` (o el MIME de una URL `data:`), si se conoce.
    pub content_type: Option<String>,
}

pub trait ResourceLoader: Send + Sync {
    /// Contenido de `url`, o `None` si no se pudo cargar.
    fn load(&self, url: &Url) -> Option<Resource>;
}

/// Carga desde el sistema de archivos (`file:`), URLs `data:` y prefijos montados.
#[derive(Debug, Clone, Default)]
pub struct FileLoader {
    mounts: Vec<(String, PathBuf)>,
}

impl FileLoader {
    pub fn new() -> Self {
        FileLoader::default()
    }

    /// Sirve las URLs que empiezan con `prefix` (p. ej. `http://web-platform.test/`) desde
    /// `dir`.
    pub fn mount(mut self, prefix: &Url, dir: PathBuf) -> Self {
        self.mounts.push((prefix.serialize(true), dir));
        self
    }
}

/// `data:`, `file:` o un prefijo montado.
pub fn load_local(url: &Url, mounts: &[(String, PathBuf)]) -> Option<Resource> {
    if url.scheme == "data" {
        let d = bf_url::data::process(url)?;
        return Some(Resource {
            bytes: d.body,
            url: url.clone(),
            content_type: Some(d.mime_type),
        });
    }
    let href = url.serialize(true);
    let href = href.split('?').next().unwrap_or(&href);
    for (prefix, dir) in mounts {
        if let Some(rest) = href.strip_prefix(prefix.as_str()) {
            let decoded = bf_url::percent::decode(rest.as_bytes());
            let path = dir.join(String::from_utf8_lossy(&decoded).as_ref());
            let bytes = std::fs::read(&path).ok()?;
            return Some(Resource {
                bytes,
                url: url.clone(),
                content_type: None,
            });
        }
    }
    let path = url.to_file_path()?;
    let bytes = std::fs::read(path).ok()?;
    Some(Resource {
        bytes,
        url: url.clone(),
        content_type: None,
    })
}

impl ResourceLoader for FileLoader {
    fn load(&self, url: &Url) -> Option<Resource> {
        load_local(url, &self.mounts)
    }
}

/// URL de un argumento de línea de comandos: una URL absoluta, o una ruta (relativa al
/// directorio actual) que pasa a `file:`.
pub fn url_from_arg(arg: &str) -> Option<Url> {
    // Una letra de unidad (`C:\x`) no es un esquema.
    let looks_like_drive = arg.len() >= 2 && arg.as_bytes()[1] == b':';
    if !looks_like_drive && let Ok(u) = Url::parse(arg, None) {
        return Some(u);
    }
    let path = std::path::absolute(arg).ok()?;
    Url::from_file_path(&path).ok()
}

/// Decodifica texto de un recurso: BOM (UTF-8, UTF-16LE/BE) y, si no hay, UTF-8.
pub fn decode_text(bytes: &[u8]) -> String {
    crate::encoding::decode(bytes, crate::encoding::Encoding::Utf8).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_loading() {
        let dir = std::env::temp_dir().join(format!("bf-infra-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/a b.css"), "x").unwrap();
        let file = Url::from_file_path(&dir.join("sub/a b.css")).unwrap();
        assert_eq!(FileLoader::new().load(&file).unwrap().bytes, b"x");
        let prefix = Url::parse("http://web-platform.test/", None).unwrap();
        let loader = FileLoader::new().mount(&prefix, dir.clone());
        let mounted = Url::parse("http://web-platform.test/sub/a%20b.css?x", None).unwrap();
        assert_eq!(loader.load(&mounted).unwrap().bytes, b"x");
        let data = Url::parse("data:text/css,p%7B%7D", None).unwrap();
        assert_eq!(loader.load(&data).unwrap().bytes, b"p{}");
        assert!(url_from_arg("C:\\x\\y.html").is_some_and(|u| u.scheme == "file"));
        assert_eq!(url_from_arg("https://example.com").unwrap().scheme, "https");
    }

    #[test]
    fn decoding() {
        assert_eq!(decode_text(b"\xEF\xBB\xBFa"), "a");
        assert_eq!(decode_text(&[0xFF, 0xFE, b'h', 0, b'i', 0]), "hi");
        assert_eq!(decode_text("ñ".as_bytes()), "ñ");
    }
}
