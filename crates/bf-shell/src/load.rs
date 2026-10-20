//! Carga de un documento desde un argumento de línea de comandos (URL `http(s)`/`file`/`data`
//! o ruta local): fetch, detección del encoding (HTML §13.2.3) y parseo HTML o XML según el
//! `Content-Type` (o la extensión, para archivos).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bf_dom::{Document, NodeId};
use bf_infra::encoding::{self, Encoding};
use bf_infra::{Resource, ResourceLoader, Url, url_from_arg};
use bf_net::{FetchOptions, MimeType, NetLoader, html_encoding};

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
}

pub fn load(arg: &str) -> Result<Loaded, String> {
    let url = url_from_arg(arg).ok_or_else(|| format!("URL o ruta inválida: {arg}"))?;
    load_url(&url, Arc::new(NetLoader::new()))
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
pub fn load_url(url: &Url, net: Arc<NetLoader>) -> Result<Loaded, String> {
    let start = Instant::now();
    let response = net
        .fetcher
        .fetch(url, &FetchOptions::document())
        .map_err(|e| format!("no se pudo cargar {url}: {e}"))?;
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
    let (doc, encoding) = if xml {
        // XML: BOM o UTF-8 (sin prescan de <meta>).
        let (text, enc) = encoding::decode(&response.body, Encoding::Utf8);
        (bf_html::parse_xml_document(&text), enc)
    } else {
        let ct = content_type
            .as_ref()
            .filter(|ct| ct.is_html())
            .cloned()
            .or_else(|| MimeType::parse("text/html"));
        let enc = html_encoding(&response.body, ct.as_ref());
        if !enc.is_supported() {
            eprintln!(
                "aviso: el encoding {} todavía no tiene decodificador; se usa windows-1252",
                enc.name()
            );
        }
        let (text, enc) = encoding::decode(&response.body, enc);
        let (doc, _) = bf_html::parse_document(&text, &bf_html::ParseOptions::default());
        (doc, enc)
    };
    let base = document_base_url(&doc, &response.url);
    Ok(Loaded {
        doc,
        url: response.url,
        base,
        encoding,
        loader: Arc::new(CachedLoader::new(net)),
        fetch,
        parse: start.elapsed(),
    })
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Página de error interna (HTML generado y maquetado por el mismo motor) para `url`.
pub fn error_page(url: &Url, message: &str, net: Arc<NetLoader>) -> Loaded {
    let html = format!(
        "<!DOCTYPE html><html lang=es><title>Error al cargar la página</title>\
         <style>body{{font:16px sans-serif;margin:64px auto;max-width:640px;color:#202124}}\
         h1{{font-size:24px;font-weight:normal}} code{{background:#f1f3f4;padding:2px 4px}}\
         p.detail{{color:#5f6368}}</style>\
         <h1>No se pudo cargar la página</h1><p><code>{}</code></p>\
         <p class=detail>{}</p><p>Revisa la dirección o la conexión y vuelve a intentarlo \
         con <b>F5</b>.</p></html>",
        escape(&url.href()),
        escape(message)
    );
    let (doc, _) = bf_html::parse_document(&html, &bf_html::ParseOptions::default());
    Loaded {
        doc,
        url: url.clone(),
        base: url.clone(),
        encoding: Encoding::Utf8,
        loader: Arc::new(CachedLoader::new(net)),
        fetch: Duration::ZERO,
        parse: Duration::ZERO,
    }
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
