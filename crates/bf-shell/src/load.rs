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
    EncodingSource, FetchError, FetchOptions, MimeType, NetLoader, ReferrerPolicy,
    html_encoding_with_source,
};

use crate::internal;
use crate::script::{Engine, PageEnv, parse_with_scripts};
use crate::theme::Theme;

/// Los subrecursos de un documento (hojas, imágenes, fuentes) en memoria, para que volver a
/// hacer estilo y layout (al redimensionar o hacer zoom) no los pida de nuevo a la red. Es la
/// memoria del documento; entre documentos y sesiones está la caché HTTP de `bf-net` (RFC 9111).
pub struct CachedLoader {
    pub net: Arc<NetLoader>,
    entries: Mutex<HashMap<String, Option<Resource>>>,
    /// E6: el cliente de los subrecursos (Fetch §2.2.5: el referente `"client"` es la URL del
    /// documento, Referrer Policy §8.3 paso 2) y la política de su *policy container* (HTML
    /// §7.1.2: la de la cabecera `Referrer-Policy`, cambiada luego por `<meta name=referrer>`).
    client: Mutex<Option<(Url, ReferrerPolicy)>>,
}

impl CachedLoader {
    pub fn new(net: Arc<NetLoader>) -> CachedLoader {
        CachedLoader {
            net,
            entries: Mutex::new(HashMap::new()),
            client: Mutex::new(None),
        }
    }

    /// El documento que pide los subrecursos: su URL (el referente) y su política.
    pub fn set_client(&self, url: Url, policy: ReferrerPolicy) {
        *self.client.lock().unwrap_or_else(|e| e.into_inner()) = Some((url, policy));
    }

    /// Cambia la política del documento (HTML §4.2.5.3, `<meta name=referrer>`).
    pub fn set_referrer_policy(&self, policy: ReferrerPolicy) {
        if let Some((_, p)) = self
            .client
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            *p = policy;
        }
    }
}

/// HTML §4.2.5.3 (`<meta name=referrer>`): la política del último `<meta name=referrer>` con un
/// `content` válido (los tokens viejos `never`, `default`, `always` y `origin-when-crossorigin` se
/// traducen); `None` si no hay ninguno.
pub fn meta_referrer_policy(doc: &Document) -> Option<ReferrerPolicy> {
    doc.inclusive_descendants(doc.root())
        .into_iter()
        .filter_map(|n| doc.element(n))
        .filter(|e| {
            e.is_html("meta")
                && e.attr("name")
                    .is_some_and(|n| n.eq_ignore_ascii_case("referrer"))
        })
        .filter_map(|e| {
            let value = e.attr("content")?.to_ascii_lowercase();
            let value = match value.as_str() {
                "" => return None,
                "never" => "no-referrer",
                "default" => "strict-origin-when-cross-origin",
                "always" => "unsafe-url",
                "origin-when-crossorigin" => "origin-when-cross-origin",
                v => v,
            };
            ReferrerPolicy::parse(value)
        })
        .last()
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
        let (referrer, referrer_policy) = self
            .client
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .map_or((None, ReferrerPolicy::Default), |(u, p)| (Some(u), p));
        let options = FetchOptions {
            referrer,
            referrer_policy,
            ..FetchOptions::default()
        };
        let result = self.net.load_with(url, &options);
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
    /// La respuesta no era para mostrar y se guardó como descarga (E11): la ruta del archivo.
    pub download: Option<String>,
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
    /// El contexto de la línea de comandos: red nueva (con la caché HTTP en disco), fuentes del
    /// sistema, 1024×768.
    pub fn cli() -> LoadContext {
        LoadContext {
            net: Arc::new(NetLoader::with_disk_cache()),
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
/// error del comando. Los scripts se ejecutan (la consola va a stderr).
pub fn load(arg: &str) -> Result<Loaded, String> {
    let url = url_from_arg(arg).ok_or_else(|| format!("URL o ruta inválida: {arg}"))?;
    if internal::is_internal(&url) {
        return Ok(internal::page(&url, &LoadContext::cli()));
    }
    let mut engine = Engine::stderr();
    let net = Arc::new(NetLoader::with_disk_cache());
    let env = PageEnv::cli(net.clone());
    let loaded = fetch_document_with(&url, net, None, Some((&mut engine, &env)))
        .map_err(|e| format!("no se pudo cargar {url}: {e}"))?;
    let (executed, failed) = engine.counts();
    if executed > 0 {
        eprintln!("scripts: {executed} ejecutados, {failed} con error");
    }
    Ok(loaded)
}

/// Carga para la ventana: nunca falla; los errores se muestran como `about:error`. Con
/// `engine`, los scripts del documento se ejecutan con él.
pub fn load_with(url: &Url, ctx: &LoadContext, engine: Option<(&mut Engine, &PageEnv)>) -> Loaded {
    if internal::is_internal(url) {
        return internal::page(url, ctx);
    }
    match fetch_document_with(url, ctx.net.clone(), ctx.post.clone(), engine) {
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
    fetch_document_with(url, net, None, None)
}

/// Como [`fetch_document`], con un cuerpo `POST` (envío de formularios): `(bytes, Content-Type)`,
/// y, con `engine`, ejecutando los scripts del documento HTML durante el parseo.
pub fn fetch_document_with(
    url: &Url,
    net: Arc<NetLoader>,
    post: Option<(Vec<u8>, String)>,
    engine: Option<(&mut Engine, &PageEnv)>,
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
    // Lo que no se muestra (`Content-Disposition: attachment`, tipos que el motor no
    // renderiza) se guarda como descarga (E11).
    if let Some(saved) = crate::internal::download_response(&response, &net) {
        return Ok(saved);
    }
    if let Some(image) = crate::internal::image_document(&response, &net) {
        return Ok(image);
    }
    let fetch = start.elapsed();
    let start = Instant::now();
    let loader = Arc::new(CachedLoader::new(net));
    // E6: la política de referente del documento (HTML §7.1.2, *create a policy container from a
    // fetch response*: Referrer Policy §8.1, la cabecera con todas sus líneas).
    let header_policy = response
        .headers
        .get_all("referrer-policy")
        .collect::<Vec<_>>()
        .join(",");
    loader.set_client(
        response.url.clone(),
        ReferrerPolicy::from_header(&header_policy).unwrap_or_default(),
    );
    let content_type = response.content_type();
    let path = response.url.path_string().to_ascii_lowercase();
    let xml = match &content_type {
        Some(ct) => ct.is_xml() && !ct.is_html(),
        None => path.ends_with(".xht") || path.ends_with(".xhtml") || path.ends_with(".svg"),
    };
    let (mut doc, encoding, source) = if xml {
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
        let (text, enc) = encoding::decode(&response.body, enc);
        let doc = match engine {
            Some((engine, env)) => {
                parse_with_scripts(&text, &response.url, engine, loader.clone(), env)
            }
            // E1: un documento de navegación permite declarative shadow roots.
            None => {
                let opts = bf_html::ParseOptions {
                    allow_declarative_shadow_roots: true,
                    ..bf_html::ParseOptions::default()
                };
                bf_html::parse_document(&text, &opts).0
            }
        };
        (doc, enc, source)
    };
    // E10: `document.characterSet` (y el encoding de las URLs del documento).
    doc.character_set = encoding.name().into();
    // E6: `<meta name=referrer>` cambia la política (HTML §4.2.5.3). Se aplica al terminar el
    // parseo: los scripts que se piden durante el parseo van con la de la cabecera.
    if let Some(policy) = meta_referrer_policy(&doc) {
        loader.set_referrer_policy(policy);
    }
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
        download: None,
    };
    Ok(Loaded {
        doc,
        url: response.url,
        base,
        encoding,
        loader,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    type Log = Arc<Mutex<Vec<(String, Option<String>)>>>;

    /// Un servidor HTTP/1.1 mínimo: `/doc-<n>` responde `pages[n]` (cabeceras de más y cuerpo);
    /// cualquier otra ruta, un cuerpo vacío. Anota la ruta y el `Referer` de cada petición.
    fn serve(pages: Vec<(&'static str, &'static str)>) -> (Url, Log) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let log: Log = Arc::default();
        let seen = log.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let path = line.split(' ').nth(1).unwrap_or("").to_owned();
                let mut referer = None;
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                        break;
                    }
                    if let Some((n, v)) = h.split_once(':')
                        && n.eq_ignore_ascii_case("referer")
                    {
                        referer = Some(v.trim().to_owned());
                    }
                }
                seen.lock().unwrap().push((path.clone(), referer));
                let (extra, body) = path
                    .strip_prefix("/doc-")
                    .and_then(|n| n.split('?').next()?.parse::<usize>().ok())
                    .and_then(|n| pages.get(n).copied())
                    .unwrap_or(("", ""));
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nCache-Control: no-store\r\n\
                     Connection: close\r\n{extra}Content-Length: {}\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        let base = Url::parse(&format!("http://127.0.0.1:{port}/"), None).unwrap();
        (base, log)
    }

    /// La política del documento (cabecera `Referrer-Policy`, o `<meta name=referrer>`, que la
    /// cambia) llega a sus subrecursos; sin ninguna, `strict-origin-when-cross-origin`.
    #[test]
    fn subresources_use_the_document_referrer_policy() {
        let (base, log) = serve(vec![
            ("", "<p>sin política"),
            ("Referrer-Policy: no-referrer\r\n", "<p>cabecera"),
            (
                "Referrer-Policy: unsafe-url, no-referrer\r\n",
                "<meta name=Referrer content=ORIGIN>",
            ),
            (
                "",
                "<meta name=referrer content=never><meta name=referrer content=bogus>",
            ),
        ]);
        let net = Arc::new(NetLoader::new());
        let mut referers = Vec::new();
        for n in 0..4 {
            let doc = Url::parse(&format!("doc-{n}?q#f"), Some(&base)).unwrap();
            let loaded = fetch_document(&doc, net.clone()).unwrap();
            let sub = Url::parse(&format!("img-{n}.png"), Some(&base)).unwrap();
            loaded.loader.load(&sub);
            let (path, referer) = log.lock().unwrap().last().unwrap().clone();
            assert_eq!(path, format!("/img-{n}.png"));
            referers.push(referer);
        }
        let origin = base.href();
        assert_eq!(
            referers,
            [
                Some(format!("{origin}doc-0?q")),
                None,
                Some(origin.clone()),
                None,
            ]
        );
    }

    #[test]
    fn meta_referrer_tokens() {
        let policy = |html: &str| {
            let (doc, _) = bf_html::parse_document(html, &bf_html::ParseOptions::default());
            meta_referrer_policy(&doc)
        };
        assert_eq!(
            policy("<meta name=referrer content=always>"),
            Some(ReferrerPolicy::UnsafeUrl)
        );
        assert_eq!(
            policy("<meta name=referrer content=origin-when-crossorigin>"),
            Some(ReferrerPolicy::OriginWhenCrossOrigin)
        );
        assert_eq!(
            policy("<meta name=referrer content=default>"),
            Some(ReferrerPolicy::StrictOriginWhenCrossOrigin)
        );
        assert_eq!(
            policy("<meta name=referrer content=''><meta name=description content=origin>"),
            None
        );
    }
}
