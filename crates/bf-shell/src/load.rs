//! Carga de documentos: fetch, detección del encoding (HTML §13.2.3) y parseo HTML o XML según
//! el `Content-Type` (o la extensión, para archivos). Las URLs `about:` son páginas internas
//! ([`crate::internal`]) y los errores de carga se muestran como `about:error`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bf_dom::{Document, NodeId};
use bf_font::FontDb;
use bf_infra::encoding::{self, Encoding};
use bf_infra::{Resource, ResourceLoader, Url, url_from_arg};
use bf_layout::Size;
use bf_net::{
    EncodingSource, FetchError, FetchOptions, MimeType, NetLoader, html_encoding_with_source,
};

use crate::internal;
use crate::theme::Theme;

/// Los subrecursos de un documento (hojas, imágenes, fuentes) en memoria, para que volver a
/// hacer estilo y layout (al redimensionar o hacer zoom) no los pida de nuevo a la red.
///
/// TODO(http-cache): reemplazar por la caché HTTP de `bf-net` (RFC 9111).
pub struct CachedLoader {
    pub net: Arc<NetLoader>,
    entries: Mutex<HashMap<String, Option<Resource>>>,
}

impl CachedLoader {
    pub fn new(net: Arc<NetLoader>) -> CachedLoader {
        CachedLoader {
            net,
            entries: Mutex::new(HashMap::new()),
        }
    }
}

impl ResourceLoader for CachedLoader {
    fn load(&self, url: &Url) -> Option<Resource> {
        let key = url.href();
        if let Some(hit) = self
            .entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        {
            return hit.clone();
        }
        let result = self.net.load(url);
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, result.clone());
        result
    }
}

/// Por qué no se pudo cargar un documento.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Dns,
    Tls,
    Timeout,
    Connection,
    Http(u16),
    NotFound,
    InvalidUrl,
    UnsupportedScheme,
    TooManyRedirects,
    UnknownInternal,
    /// Un pánico del motor con este documento.
    Engine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadError {
    pub kind: ErrorKind,
    pub message: String,
}

impl LoadError {
    fn from_fetch(e: &FetchError) -> LoadError {
        let kind = match e {
            FetchError::Dns(_) => ErrorKind::Dns,
            FetchError::Tls(_) => ErrorKind::Tls,
            FetchError::Timeout => ErrorKind::Timeout,
            FetchError::Io(_) => ErrorKind::Connection,
            FetchError::NotFound => ErrorKind::NotFound,
            FetchError::InvalidUrl => ErrorKind::InvalidUrl,
            FetchError::UnsupportedScheme(_) => ErrorKind::UnsupportedScheme,
            FetchError::TooManyRedirects => ErrorKind::TooManyRedirects,
        };
        LoadError {
            kind,
            message: e.to_string(),
        }
    }
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Lo que se sabe de la respuesta que produjo el documento (para el panel del motor).
#[derive(Debug, Clone, Default)]
pub struct Meta {
    /// Código y texto HTTP (`None` en `file:`, `data:` y las páginas internas).
    pub status: Option<(u16, String)>,
    pub redirects: usize,
    pub encoding_source: Option<EncodingSource>,
    /// Tamaño del documento recibido (después de descomprimir).
    pub bytes: usize,
    pub content_type: Option<String>,
    /// Página generada por el navegador (`about:`).
    pub internal: bool,
    /// Si es una página de error, el error.
    pub error: Option<LoadError>,
}

#[derive(Clone)]
pub struct Loaded {
    pub doc: Document,
    /// URL del documento: la final, después de redirecciones.
    pub url: Url,
    /// URL base del documento (HTML §2.4.1: la del primer `<base href>` o la del documento),
    /// contra la que se resuelven enlaces y subrecursos.
    pub base: Url,
    pub encoding: Encoding,
    pub loader: Arc<CachedLoader>,
    pub fetch: Duration,
    pub parse: Duration,
    pub meta: Meta,
}

impl Loaded {
    pub fn net(&self) -> &NetLoader {
        &self.loader.net
    }

    /// Título del documento (HTML §3.1.3, `document.title`): el texto del primer `<title>` con
    /// el white space ASCII colapsado.
    pub fn title(&self) -> Option<String> {
        let doc = &self.doc;
        let title = doc
            .inclusive_descendants(doc.root())
            .into_iter()
            .find(|&n| doc.element(n).is_some_and(|e| e.is_html("title")))?;
        let text = doc
            .text_content(title)
            .split_ascii_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        (!text.is_empty()).then_some(text)
    }

    /// Cookies que se mandarían a la URL del documento.
    pub fn cookies(&self) -> usize {
        self.net()
            .fetcher
            .cookies
            .cookie_header(&self.url)
            .map_or(0, |h| h.split(';').count())
    }

    /// Un documento generado por el navegador a partir de `html`.
    pub fn generated(url: &Url, html: &str, net: Arc<NetLoader>, meta: Meta) -> Loaded {
        let start = Instant::now();
        let (doc, _) = bf_html::parse_document(html, &bf_html::ParseOptions::default());
        Loaded {
            doc,
            url: url.clone(),
            base: url.clone(),
            encoding: Encoding::Utf8,
            loader: Arc::new(CachedLoader::new(net)),
            fetch: Duration::ZERO,
            parse: start.elapsed(),
            meta: Meta {
                internal: true,
                bytes: html.len(),
                ..meta
            },
        }
    }
}

/// Lo que necesita una carga además de la URL: la red (conexiones y cookies), las fuentes y
/// el viewport (para `about:engine`, que maqueta otra página), el historial reciente (para
/// `about:home`) y el tema de las páginas internas.
#[derive(Clone)]
pub struct LoadContext {
    pub net: Arc<NetLoader>,
    pub fonts: Arc<FontDb>,
    pub viewport: Size,
    /// (título, URL) de las páginas visitadas, la más reciente primero.
    pub recent: Vec<(String, String)>,
    pub theme: Theme,
    /// Cuerpo `POST` y su `Content-Type` (envío de un formulario).
    pub post: Option<(Vec<u8>, String)>,
}

impl LoadContext {
    /// El contexto de la línea de comandos: red nueva, fuentes del sistema, 1024×768.
    pub fn cli() -> LoadContext {
        LoadContext {
            net: Arc::new(NetLoader::new()),
            fonts: FontDb::system(),
            viewport: Size {
                width: 1024.0,
                height: 768.0,
            },
            recent: Vec::new(),
            theme: Theme::for_system(None),
            post: None,
        }
    }
}

/// Carga para la línea de comandos: las páginas internas se generan; un error de red es un
/// error del comando.
pub fn load(arg: &str) -> Result<Loaded, String> {
    let url = url_from_arg(arg).ok_or_else(|| format!("URL o ruta inválida: {arg}"))?;
    if url.scheme == "about" {
        return Ok(internal::page(&url, &LoadContext::cli()));
    }
    fetch_document(&url, Arc::new(NetLoader::new()))
        .map_err(|e| format!("no se pudo cargar {url}: {e}"))
}

/// Carga para la ventana: nunca falla; los errores se muestran como `about:error`.
pub fn load_with(url: &Url, ctx: &LoadContext) -> Loaded {
    if url.scheme == "about" {
        return internal::page(url, ctx);
    }
    match fetch_document_with(url, ctx.net.clone(), ctx.post.clone()) {
        // Un 4xx/5xx sin cuerpo: la página de error interna (con cuerpo, se muestra el del
        // servidor, como hacen los navegadores).
        Ok(l) if l.meta.bytes == 0 && l.meta.status.as_ref().is_some_and(|s| s.0 >= 400) => {
            let (code, text) = l.meta.status.clone().unwrap_or_default();
            let err = LoadError {
                kind: ErrorKind::Http(code),
                message: format!("HTTP {code} {text}"),
            };
            internal::error_page(&l.url, &err, ctx)
        }
        Ok(l) => l,
        Err(e) => internal::error_page(url, &e, ctx),
    }
}

/// La URL base de `doc` (HTML §2.4.1, *document base URL*): el `href` del primer `<base>`
/// que lo tiene, resuelto contra la URL del documento; si no hay, la del documento.
pub fn document_base_url(doc: &Document, url: &Url) -> Url {
    doc.inclusive_descendants(doc.root())
        .into_iter()
        .filter_map(|n| doc.element(n))
        .find(|e| e.is_html("base") && e.attr("href").is_some())
        .and_then(|e| Url::parse(e.attr("href")?, Some(url)).ok())
        .filter(|u| !matches!(u.scheme.as_str(), "data" | "javascript"))
        .unwrap_or_else(|| url.clone())
}

/// Fetch y parseo de `url` con `net` (que conserva conexiones y cookies entre cargas).
pub fn fetch_document(url: &Url, net: Arc<NetLoader>) -> Result<Loaded, LoadError> {
    fetch_document_with(url, net, None)
}

/// Como [`fetch_document`], con un cuerpo `POST` (envío de formularios): `(bytes, Content-Type)`.
pub fn fetch_document_with(
    url: &Url,
    net: Arc<NetLoader>,
    post: Option<(Vec<u8>, String)>,
) -> Result<Loaded, LoadError> {
    let start = Instant::now();
    let mut options = FetchOptions::document();
    if let Some(body) = post {
        options.method = bf_net::Method::Post;
        options.body = Some(body);
    }
    let response = net
        .fetcher
        .fetch(url, &options)
        .map_err(|e| LoadError::from_fetch(&e))?;
    if !response.ok() {
        eprintln!(
            "aviso: {} respondió {} {}",
            response.url, response.status, response.status_text
        );
    }
    let fetch = start.elapsed();
    let start = Instant::now();
    let content_type = response.content_type();
    let path = response.url.path_string().to_ascii_lowercase();
    let xml = match &content_type {
        Some(ct) => ct.is_xml() && !ct.is_html(),
        None => path.ends_with(".xht") || path.ends_with(".xhtml") || path.ends_with(".svg"),
    };
    let (doc, encoding, source) = if xml {
        // XML: BOM o UTF-8 (sin prescan de <meta>).
        let (text, enc) = encoding::decode(&response.body, Encoding::Utf8);
        let source = if bf_infra::encoding::sniff_bom(&response.body).is_some() {
            EncodingSource::Bom
        } else {
            EncodingSource::Default
        };
        (bf_html::parse_xml_document(&text), enc, source)
    } else {
        let ct = content_type
            .as_ref()
            .filter(|ct| ct.is_html())
            .cloned()
            .or_else(|| MimeType::parse("text/html"));
        let (enc, source) = html_encoding_with_source(&response.body, ct.as_ref());
        if !enc.is_supported() {
            eprintln!(
                "aviso: el encoding {} todavía no tiene decodificador; se usa windows-1252",
                enc.name()
            );
        }
        let (text, enc) = encoding::decode(&response.body, enc);
        let (doc, _) = bf_html::parse_document(&text, &bf_html::ParseOptions::default());
        (doc, enc, source)
    };
    let base = document_base_url(&doc, &response.url);
    let http = matches!(response.url.scheme.as_str(), "http" | "https");
    let meta = Meta {
        status: http.then(|| (response.status, response.status_text.clone())),
        redirects: response.redirects,
        encoding_source: Some(source),
        bytes: response.body.len(),
        content_type: response.headers.get("content-type").map(str::to_owned),
        internal: false,
        error: None,
    };
    Ok(Loaded {
        doc,
        url: response.url,
        base,
        encoding,
        loader: Arc::new(CachedLoader::new(net)),
        fetch,
        parse: start.elapsed(),
        meta,
    })
}

/// El elemento indicado por el fragmento `fragment` (HTML §7.4.6.3, *find a potential
/// indicated element*): el de ese `id` o, si no hay, el primer `<a>` con ese `name`.
pub fn indicated_element(doc: &Document, fragment: &str) -> Option<NodeId> {
    let decoded =
        String::from_utf8_lossy(&bf_url::percent::decode(fragment.as_bytes())).into_owned();
    let all = doc.inclusive_descendants(doc.root());
    let by = |name: &str, value: &str| {
        all.iter().copied().find(|&n| {
            doc.element(n)
                .is_some_and(|e| e.attr(name) == Some(value) && (name == "id" || e.is_html("a")))
        })
    };
    by("id", fragment)
        .or_else(|| by("name", fragment))
        .or_else(|| by("id", &decoded))
        .or_else(|| by("name", &decoded))
}
