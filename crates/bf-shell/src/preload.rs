//! E6: calentar la caché mientras se valida el documento.
//!
//! Con una copia del documento en la caché que hay que validar (un 304 de Wikipedia cuesta
//! TCP + TLS + 1 RTT), el hilo de la pestaña no tiene nada que hacer hasta la respuesta. En
//! paralelo a la petición, [`Warmup`]:
//!
//! 1. lee del disco el cuerpo guardado del documento ([`bf_net::cache::HttpCache::stored_body`]);
//! 2. lo recorre con un escáner de precarga ([`scan`]): los pasos de HTML §13.2.8 *speculative
//!    HTML parsing* que eligen qué pedir ("speculative fetch": `<base href>` para la URL base,
//!    `<link rel=stylesheet>`, `<script src>` e `<img src>`), sin construir un DOM ni ejecutar
//!    nada;
//! 3. de cada hoja, script e imagen lee del disco lo guardado, sin usarlo
//!    ([`bf_net::cache::HttpCache::stage`]), y se queda con lo fresco (RFC 9111 §4.2).
//!
//! No sale ninguna petición: solo se lee la caché. Lo leído entra en la caché en memoria solo
//! si el documento se validó con un 304 (RFC 9111 §4.3.4: la respuesta guardada no se usa
//! hasta validarla; [`Warmup::finish`]). Con otra respuesta se descarta. Después, la carga pide
//! esos recursos como siempre y la caché decide con sus reglas: la diferencia es que ya no
//! leen del disco en el camino de la primera pintura.

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use bf_html::tokenizer::{State, Token, Tokenizer};
use bf_infra::Url;
use bf_net::cache::{HttpCache, Staged};
use bf_net::{MimeType, NetLoader};

/// Cuántas URLs mira el escáner como mucho.
const MAX_URLS: usize = 64;

/// Lo que encuentra el escáner: hojas de estilo, scripts e imágenes, en orden de documento.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub url: Url,
    pub kind: Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Style,
    Script,
    /// Las imágenes no bloquean el renderizado, pero el pintado del shell las lee antes de la
    /// primera maqueta (`bf_paint::images::load_images_in`).
    Image,
}

/// El escáner de precarga (HTML §13.2.8, "speculative fetch" de los *speculative mock
/// elements*): tokeniza `html` como el parser (con los estados de texto de `script`, `style`,
/// `textarea`…), toma el primer `<base href>` como URL base (HTML §4.2.3) y devuelve las
/// `<link rel=stylesheet href>` (no las `alternate`), las `<link rel=preload|modulepreload>`
/// de hojas y scripts, los `<script src>` de JavaScript (clásico o módulo) y los `<img src>`.
pub fn scan(html: &str, document: &Url) -> Vec<Found> {
    let mut tok = Tokenizer::new(html);
    let mut base: Option<Url> = None;
    let mut out: Vec<Found> = Vec::new();
    let push = |out: &mut Vec<Found>, base: &Option<Url>, href: &str, kind: Kind| {
        let href = href.trim_matches(|c: char| c.is_ascii_whitespace());
        if href.is_empty() {
            return;
        }
        if let Ok(url) = Url::parse(href, Some(base.as_ref().unwrap_or(document)))
            && matches!(url.scheme.as_str(), "http" | "https")
            && !out.iter().any(|f| f.url == url)
        {
            out.push(Found { url, kind });
        }
    };
    while let Some(token) = tok.next_token() {
        if out.len() >= MAX_URLS {
            break;
        }
        let tag = match token {
            Token::StartTag(t) => t,
            Token::Eof => break,
            _ => continue,
        };
        let attr = |name: &str| {
            tag.attrs
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
        };
        match tag.name.as_str() {
            "base" if base.is_none() => {
                if let Some(href) = attr("href") {
                    base = Url::parse(href.trim(), Some(document)).ok();
                }
            }
            "link" => {
                let rel = attr("rel").unwrap_or("").to_ascii_lowercase();
                let rels: Vec<&str> = rel.split_ascii_whitespace().collect();
                let as_ = attr("as").unwrap_or("").to_ascii_lowercase();
                let kind = if rels.contains(&"stylesheet") && !rels.contains(&"alternate") {
                    Some(Kind::Style)
                } else if rels.contains(&"modulepreload")
                    || (rels.contains(&"preload") && as_ == "script")
                {
                    Some(Kind::Script)
                } else if rels.contains(&"preload") && as_ == "style" {
                    Some(Kind::Style)
                } else {
                    None
                };
                if let (Some(kind), Some(href)) = (kind, attr("href")) {
                    push(&mut out, &base, href, kind);
                }
            }
            "script" => {
                let ty = attr("type").unwrap_or("").trim().to_ascii_lowercase();
                let js = ty.is_empty()
                    || ty == "module"
                    || ty == "text/javascript"
                    || ty == "application/javascript";
                if js && let Some(src) = attr("src") {
                    push(&mut out, &base, src, Kind::Script);
                }
            }
            "img" => {
                if let Some(src) = attr("src") {
                    push(&mut out, &base, src, Kind::Image);
                }
            }
            _ => {}
        }
        // Los elementos de texto: el tokenizador cambia de estado como lo pide el tree builder
        // (HTML §13.2.6.4.4 y §13.2.6.4.7).
        let state = match tag.name.as_str() {
            "script" => Some(State::ScriptData),
            "style" | "xmp" | "iframe" | "noembed" | "noframes" | "noscript" => {
                Some(State::Rawtext)
            }
            "textarea" | "title" => Some(State::Rcdata),
            "plaintext" => break,
            _ => None,
        };
        if let Some(s) = state
            && !tag.self_closing
        {
            tok.set_state(s);
            tok.set_last_start_tag(&tag.name);
        }
    }
    out
}

/// Lo que comparten la carga y el hilo de la precarga.
#[derive(Default)]
struct Shared {
    /// `Some(true)`: el documento se validó con un 304 y lo leído se puede adoptar;
    /// `Some(false)`: no se usa nada.
    decision: Option<bool>,
    /// Lo fresco leído antes de la decisión.
    staged: Vec<Staged>,
    found: usize,
    ready: usize,
    done: bool,
}

/// La precarga de un documento (ver el módulo).
pub struct Warmup {
    cache: HttpCache,
    shared: Arc<Mutex<Shared>>,
    _thread: JoinHandle<()>,
}

/// Lo que llevaba la precarga al llegar la respuesta del documento.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WarmReport {
    /// Hojas y scripts que encontró el escáner.
    pub found: usize,
    /// Los que estaban frescos y quedaron listos en memoria (hasta ese momento).
    pub ready: usize,
    /// ¿Había terminado?
    pub done: bool,
}

impl Warmup {
    /// Empieza la precarga de `url` si la red tiene caché HTTP. Lo hace en otro hilo: no
    /// espera a nada.
    pub fn start(net: &Arc<NetLoader>, url: &Url) -> Option<Warmup> {
        if !matches!(url.scheme.as_str(), "http" | "https") {
            return None;
        }
        let cache = net.fetcher.cache.clone()?;
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (c, s, document) = (cache.clone(), shared.clone(), url.clone());
        let thread = std::thread::Builder::new()
            .name("freedom-precarga".into())
            .spawn(move || warm(&c, &s, &document))
            .ok()?;
        Some(Warmup {
            cache,
            shared,
            _thread: thread,
        })
    }

    /// Espera a que el hilo termine (tests).
    #[cfg(test)]
    fn wait(&self) {
        let start = std::time::Instant::now();
        while !self.shared.lock().unwrap().done && start.elapsed().as_secs() < 10 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// La respuesta del documento llegó: con `validated` (un 304 de la misma URL), lo fresco
    /// leído hasta ahora entra en la caché en memoria y lo que siga leyendo el hilo también;
    /// si no, se descarta. No espera al hilo.
    pub fn finish(self, validated: bool) -> WarmReport {
        let mut s = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        s.decision = Some(validated);
        let staged = std::mem::take(&mut s.staged);
        if validated {
            for st in staged {
                self.cache.adopt(st);
            }
        }
        WarmReport {
            found: s.found,
            ready: s.ready,
            done: s.done,
        }
    }
}

/// El hilo de la precarga.
fn warm(cache: &HttpCache, shared: &Mutex<Shared>, document: &Url) {
    let finish = || shared.lock().unwrap_or_else(|e| e.into_inner()).done = true;
    // Las peticiones de navegación y de subrecursos del shell van con credenciales: la clave
    // de la caché es la de `credentials = true` (`Fetcher::fetch`).
    let Some((body, content_type)) = cache.stored_body(document, true) else {
        return finish();
    };
    let mime = content_type.as_deref().and_then(MimeType::parse);
    if mime.as_ref().is_some_and(|m| !m.is_html()) {
        return finish();
    }
    let (encoding, _) = bf_net::html_encoding_with_source(&body, mime.as_ref());
    let (text, _) = bf_infra::encoding::decode(&body, encoding);
    let found = scan(&text, document);
    shared.lock().unwrap_or_else(|e| e.into_inner()).found = found.len();
    for f in found {
        if shared.lock().unwrap_or_else(|e| e.into_inner()).decision == Some(false) {
            break;
        }
        let Some(staged) = cache.stage(&f.url, true) else {
            continue;
        };
        if !staged.fresh {
            continue;
        }
        let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
        match s.decision {
            Some(true) => cache.adopt(staged),
            Some(false) => break,
            None => s.staged.push(staged),
        }
        s.ready += 1;
    }
    finish();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s, None).unwrap()
    }

    #[test]
    fn scanner_finds_styles_and_scripts() {
        let doc = url("https://sitio.test/wiki/a");
        let html = "<!doctype html><head><title><link rel=stylesheet href=/no></title>\
            <link rel='stylesheet' href='/a.css'>\
            <link rel='alternate stylesheet' href='/alt.css'>\
            <link rel=icon href=/i.png>\
            <script>var s = '<script src=/no.js></script>';</script>\
            <script src=/b.js async></script>\
            <script type=text/template src=/plantilla></script>\
            <script type=module src=m.js></script>\
            <link rel=modulepreload href=/c.js>\
            <link rel=preload as=style href=/d.css>\
            <link rel=stylesheet href=/a.css>\
            <style>@import '/no.css';</style>\
            </head><body><textarea><script src=/no2.js></script></textarea>\
            <script src=data:text/javascript,1></script><img src=/e.png alt=x></body>";
        let got: Vec<(String, Kind)> = scan(html, &doc)
            .into_iter()
            .map(|f| (f.url.href(), f.kind))
            .collect();
        assert_eq!(
            got,
            [
                ("https://sitio.test/a.css".to_owned(), Kind::Style),
                ("https://sitio.test/b.js".to_owned(), Kind::Script),
                ("https://sitio.test/wiki/m.js".to_owned(), Kind::Script),
                ("https://sitio.test/c.js".to_owned(), Kind::Script),
                ("https://sitio.test/d.css".to_owned(), Kind::Style),
                ("https://sitio.test/e.png".to_owned(), Kind::Image),
            ]
        );
    }

    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// Un servidor con `/` (HTML que valida con `ETag`, o que cambia si `changed`), `/a.css` y
    /// `/b.js` frescos diez minutos y `/c.css` sin frescura. Cuenta las peticiones.
    fn server(changed: Arc<AtomicBool>) -> (u16, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut r = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                let _ = r.read_line(&mut line);
                let mut conditional = false;
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                        break;
                    }
                    conditional |= h.to_ascii_lowercase().starts_with("if-none-match");
                }
                seen.fetch_add(1, Ordering::SeqCst);
                let path = line.split(' ').nth(1).unwrap_or("/").to_owned();
                let (status, headers, body): (&str, &str, &str) = match path.as_str() {
                    "/" if conditional && !changed.load(Ordering::SeqCst) => {
                        ("304 Not Modified", "ETag: \"v1\"\r\n", "")
                    }
                    "/" => (
                        "200 OK",
                        "Content-Type: text/html\r\nCache-Control: max-age=0, must-revalidate\r\n\
                         ETag: \"v1\"\r\n",
                        "<!doctype html><link rel=stylesheet href=/a.css>\
                         <link rel=stylesheet href=/c.css><script src=/b.js></script>",
                    ),
                    "/c.css" => ("200 OK", "Content-Type: text/css\r\n", "p{}"),
                    _ => (
                        "200 OK",
                        "Content-Type: text/plain\r\nCache-Control: max-age=600\r\n",
                        "x",
                    ),
                };
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\n\
                         Connection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                );
            }
        });
        (port, count)
    }

    fn net(dir: &std::path::Path) -> Arc<NetLoader> {
        Arc::new(NetLoader::with_cache(HttpCache::on_disk(dir)))
    }

    /// Con la copia guardada: la precarga encuentra las dos hojas y el script, deja listos los
    /// frescos (no `c.css`) sin pedir nada, y con el 304 entran en la caché en memoria. Si el
    /// documento cambió (200), no entra nada.
    #[test]
    fn warmup_waits_for_the_304() {
        let dir = std::env::temp_dir().join(format!("freedom-precarga-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let changed = Arc::new(AtomicBool::new(false));
        let (port, count) = server(changed.clone());
        let page = url(&format!("http://127.0.0.1:{port}/"));
        {
            let n = net(&dir);
            for p in ["/", "/a.css", "/b.js", "/c.css"] {
                let u = url(&format!("http://127.0.0.1:{port}{p}"));
                n.fetcher
                    .fetch(&u, &bf_net::FetchOptions::default())
                    .unwrap();
            }
            n.fetcher.cache.as_ref().unwrap().flush();
        }
        assert_eq!(count.load(Ordering::SeqCst), 4);

        // Otro proceso: nada en memoria.
        let n = net(&dir);
        let cache = n.fetcher.cache.clone().unwrap();
        let w = Warmup::start(&n, &page).expect("con caché");
        w.wait();
        assert_eq!(count.load(Ordering::SeqCst), 4, "la precarga no pide nada");
        assert!(cache.len() <= 1, "nada en memoria antes del 304");
        let r = n
            .fetcher
            .fetch(&page, &bf_net::FetchOptions::document())
            .unwrap();
        assert_eq!(r.cache, bf_net::CacheUse::Revalidated);
        let report = w.finish(true);
        assert_eq!((report.found, report.ready, report.done), (3, 2, true));
        // El documento y las dos frescas.
        assert_eq!(cache.len(), 3);
        assert_eq!(count.load(Ordering::SeqCst), 5);

        // El documento cambió: lo leído no entra.
        changed.store(true, Ordering::SeqCst);
        let n = net(&dir);
        let cache = n.fetcher.cache.clone().unwrap();
        let w = Warmup::start(&n, &page).expect("con caché");
        w.wait();
        let r = n
            .fetcher
            .fetch(&page, &bf_net::FetchOptions::document())
            .unwrap();
        assert_eq!(r.cache, bf_net::CacheUse::Network);
        let report = w.finish(false);
        assert_eq!(report.ready, 2);
        assert_eq!(cache.len(), 1, "solo el documento nuevo");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Cuánto cuestan el escáner, el tokenizador solo y el parser entero sobre un HTML real
    /// (para el diseño del parseo especulativo): `BF_HTML=<archivo> cargo test --release -p
    /// bf-shell --bin freedom parse_costs -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn parse_costs() {
        let Some(path) = std::env::var_os("BF_HTML") else {
            return;
        };
        let html = std::fs::read_to_string(path).unwrap();
        let doc = url("https://es.wikipedia.org/wiki/Navegador_web");
        let best = |f: &dyn Fn()| {
            (0..5)
                .map(|_| {
                    let t = std::time::Instant::now();
                    f();
                    t.elapsed()
                })
                .min()
                .unwrap()
        };
        let scan_t = best(&|| {
            std::hint::black_box(scan(&html, &doc));
        });
        let tok_t = best(&|| {
            let mut t = Tokenizer::new(&html);
            let mut n = 0usize;
            while let Some(tok) = t.next_token() {
                if tok == Token::Eof {
                    break;
                }
                n += 1;
            }
            std::hint::black_box(n);
        });
        let parse_t = best(&|| {
            std::hint::black_box(bf_html::parse_document(
                &html,
                &bf_html::ParseOptions::default(),
            ));
        });
        eprintln!(
            "{} bytes: escáner {:?} · tokenizador {:?} · parser entero {:?} · encontrados {}",
            html.len(),
            scan_t,
            tok_t,
            parse_t,
            scan(&html, &doc).len()
        );
    }

    /// El primer `<base href>` cambia la URL base de lo que sigue (HTML §4.2.3).
    #[test]
    fn scanner_uses_the_first_base() {
        let doc = url("https://sitio.test/x/y");
        let html = "<link rel=stylesheet href=antes.css><base href=https://cdn.test/r/>\
            <base href=https://otro.test/><script src=s.js></script>";
        let got: Vec<String> = scan(html, &doc).into_iter().map(|f| f.url.href()).collect();
        assert_eq!(
            got,
            ["https://sitio.test/x/antes.css", "https://cdn.test/r/s.js"]
        );
    }
}
