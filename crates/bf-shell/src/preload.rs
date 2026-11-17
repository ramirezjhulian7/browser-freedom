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
//!
//! E3: además, con todo leído, el mismo hilo adelanta el trabajo de CPU de las hojas y los
//! scripts guardados (frescos o no):
//! 4. parsea las hojas en [`bf_style::sheet_cache`] ([`bf_style::sheet_cache::warm`], con la
//!    clave de la carga: URL y hash de los bytes, modo quirks y encoding del documento);
//! 5. compila los scripts clásicos en [`bf_js::code_cache`] ([`bf_js::code_cache::warm`], con
//!    la URL y el texto con que los evalúa la ventana).
//!
//! Las dos cachés son funciones puras de su clave (los bytes o el texto van en ella): lo
//! parseado o compilado aquí solo lo encuentra una carga que recibe exactamente esos bytes,
//! que daría el mismo resultado al parsearlos ella; por eso también valen las copias que ya
//! no están frescas y que la carga validará. No se ejecuta nada (un error de sintaxis
//! queda para la evaluación, ECMA-262 §16.1.5) y no sale ninguna petición. Con otra respuesta
//! que no sea un 304 el hilo se detiene. Y [`precompile_window`] compila en otro hilo, una vez
//! por proceso, el JavaScript propio de la ventana (`bf_web::precompile_window_scripts`).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Once};
use std::thread::JoinHandle;
use std::time::Instant;

use bf_html::tokenizer::{State, Token, Tokenizer};
use bf_infra::encoding::Encoding;
use bf_infra::{Destination, ResourceLoader, Url};
use bf_net::cache::{HttpCache, Staged};
use bf_net::{MimeType, NetLoader};

/// Cuántas URLs mira el escáner como mucho.
const MAX_URLS: usize = 64;

/// La pila de los hilos que parsean y compilan (el parser de JavaScript mide la que usa).
const STACK: usize = 32 << 20;

/// Lo que encuentra el escáner: hojas de estilo, scripts e imágenes, en orden de documento.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub url: Url,
    pub kind: Kind,
    /// El `charset` de un `<script>`, si nombra un encoding (HTML §4.12.1.1, *prepare the
    /// script element*, paso 21; si no, el del documento).
    pub charset: Option<Encoding>,
    /// E3: el `nonce` de un `<script>` (CSP 3 §6.7.1.1, para pedirlo como lo pedirá el parser).
    pub nonce: Option<String>,
}

/// E3: lo que encuentra [`scan_document`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Scan {
    pub found: Vec<Found>,
    /// ¿Trae un `<meta http-equiv="Content-Security-Policy">`? Esa política solo se conoce
    /// cuando el parser inserta el `<meta>` (CSP 3 §4.2.1): hasta entonces no se puede saber
    /// qué pedidos permitirá.
    pub meta_csp: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Style,
    /// Un script clásico (`<script src>` o `<link rel=preload as=script>`).
    Script,
    /// Un script módulo (`<script type=module>` o `<link rel=modulepreload>`): un Module
    /// Record del realm (ECMA-262 §16.2.1.6), que no pasa por la caché de código.
    Module,
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
    scan_document(html, document).found
}

/// E3: como [`scan`], y además si el documento trae CSP en un `<meta>` ([`Scan::meta_csp`]).
pub fn scan_document(html: &str, document: &Url) -> Scan {
    let mut tok = Tokenizer::new(html);
    let mut base: Option<Url> = None;
    let mut out: Vec<Found> = Vec::new();
    let mut meta_csp = false;
    let push = |out: &mut Vec<Found>,
                base: &Option<Url>,
                href: &str,
                kind: Kind,
                charset: Option<Encoding>,
                nonce: Option<String>| {
        let href = href.trim_matches(|c: char| c.is_ascii_whitespace());
        if href.is_empty() {
            return;
        }
        if let Ok(url) = Url::parse(href, Some(base.as_ref().unwrap_or(document)))
            && matches!(url.scheme.as_str(), "http" | "https")
            && !out.iter().any(|f| f.url == url)
        {
            out.push(Found {
                url,
                kind,
                charset,
                nonce,
            });
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
                } else if rels.contains(&"modulepreload") {
                    Some(Kind::Module)
                } else if rels.contains(&"preload") && as_ == "script" {
                    Some(Kind::Script)
                } else if rels.contains(&"preload") && as_ == "style" {
                    Some(Kind::Style)
                } else {
                    None
                };
                if let (Some(kind), Some(href)) = (kind, attr("href")) {
                    push(&mut out, &base, href, kind, None, None);
                }
            }
            "script" => {
                let ty = attr("type").unwrap_or("").trim().to_ascii_lowercase();
                let kind = if ty == "module" {
                    Some(Kind::Module)
                } else if ty.is_empty() || ty == "text/javascript" || ty == "application/javascript"
                {
                    Some(Kind::Script)
                } else {
                    None
                };
                if let (Some(kind), Some(src)) = (kind, attr("src")) {
                    let charset = attr("charset").and_then(Encoding::for_label);
                    let nonce = attr("nonce").map(str::to_owned);
                    push(&mut out, &base, src, kind, charset, nonce);
                }
            }
            "img" => {
                if let Some(src) = attr("src") {
                    push(&mut out, &base, src, Kind::Image, None, None);
                }
            }
            "meta" => {
                meta_csp |= attr("http-equiv")
                    .is_some_and(|v| v.trim().eq_ignore_ascii_case("content-security-policy"));
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
    Scan {
        found: out,
        meta_csp,
    }
}

/// E3: el modo del documento según su DOCTYPE (HTML §13.2.6.4.1, el modo de inserción
/// "initial"), solo cuando no depende de las listas de identificadores públicos:
/// `Some(false)` con el DOCTYPE de HTML (`<!DOCTYPE html>`, sin identificador público y sin
/// identificador de sistema o con `about:legacy-compat`) y `Some(true)` sin DOCTYPE (lo
/// primero que no es espacio ni comentario es otra cosa: quirks). Con otro DOCTYPE, `None`:
/// la precarga no adivina, y no parsea las hojas (el modo va en su clave).
pub fn document_quirks(html: &str) -> Option<bool> {
    let mut tok = Tokenizer::new(html);
    while let Some(token) = tok.next_token() {
        match token {
            Token::Character('\t' | '\n' | '\x0C' | '\r' | ' ') | Token::Comment(_) => {}
            Token::Doctype(d) => {
                let html = !d.force_quirks
                    && d.name.as_deref() == Some("html")
                    && d.public_id.is_none()
                    && d.system_id
                        .as_deref()
                        .is_none_or(|s| s == "about:legacy-compat");
                return html.then_some(false);
            }
            _ => return Some(true),
        }
    }
    Some(true)
}

/// E3: una hoja o un script guardado que parsear o compilar después de leer todo del disco.
struct Work {
    url: Url,
    kind: Kind,
    charset: Option<Encoding>,
    bytes: Vec<u8>,
    validator: Option<String>,
}

/// E3: compila en otro hilo el JavaScript propio de la ventana en la caché de código del
/// proceso (`bf_web::precompile_window_scripts`), para que instalar la ventana después de la
/// respuesta solo tenga que copiarlo. Una vez por proceso: lo compilado no depende de la
/// página. No espera al hilo; si la ventana se instala antes de que termine, la instalación
/// compila lo que falte, como antes.
pub fn precompile_window() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("freedom-ventana".into())
            .stack_size(STACK)
            .spawn(|| {
                let start = Instant::now();
                bf_web::precompile_window_scripts();
                if std::env::var_os("BF_TRACE").is_some() {
                    eprintln!(
                        "perf precarga: JavaScript propio de la ventana precompilado en {:.1} ms",
                        start.elapsed().as_secs_f64() * 1000.0
                    );
                }
            });
    });
}

/// E3: hilos de la descarga especulativa ([`speculative_fetch`]).
const SPECULATIVE_THREADS: usize = 6;

/// E3: la descarga especulativa de HTML §13.2.8 (*speculative fetch*) con la respuesta del
/// documento ya en la mano: otro hilo recorre `html` con el escáner y pide en paralelo, con
/// `loader` (el del documento, que junta las peticiones iguales y guarda lo que llega), las
/// hojas y los scripts que nombra. El parser se detiene en cada `<script src>` hasta tenerlo;
/// sin esto, los pedía de uno en uno (más de 1 s en 10 de 27 sitios del barrido de E3).
///
/// - Los scripts se piden con su `nonce` e insertados por el parser, como los pedirá el
///   parser (CSP 3 §6.7.1.1), y las hojas como las pide el pintado.
/// - Con CSP en un `<meta>` no se pide nada: esa política aún no se conoce.
/// - No se ejecuta nada. Lo pedido y no usado (un script que otro quita antes de llegar a él)
///   lo permite §13.2.8.
/// - `BF_NO_PRECARGA=1` lo apaga.
pub fn speculative_fetch(loader: Arc<dyn ResourceLoader>, html: String, document: Url) {
    if std::env::var_os("BF_NO_PRECARGA").is_some() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("freedom-especulativa".into())
        .spawn(move || {
            let start = Instant::now();
            let scan = scan_document(&html, &document);
            drop(html);
            if scan.meta_csp {
                return;
            }
            let queue: VecDeque<Found> = scan
                .found
                .into_iter()
                .filter(|f| f.kind != Kind::Image)
                .collect();
            let total = queue.len();
            let queue = Arc::new(Mutex::new(queue));
            let workers: Vec<_> = (0..SPECULATIVE_THREADS.min(total))
                .filter_map(|_| {
                    let (queue, loader) = (queue.clone(), loader.clone());
                    std::thread::Builder::new()
                        .name("freedom-especulativa".into())
                        .spawn(move || {
                            loop {
                                let next =
                                    queue.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
                                let Some(f) = next else { break };
                                match f.kind {
                                    Kind::Style => {
                                        loader.load_for(&f.url, Destination::Style);
                                    }
                                    Kind::Script | Kind::Module => {
                                        loader.load_script(&f.url, f.nonce.as_deref(), true);
                                    }
                                    Kind::Image => {}
                                }
                            }
                        })
                        .ok()
                })
                .collect();
            for w in workers {
                let _ = w.join();
            }
            if std::env::var_os("BF_TRACE").is_some() {
                eprintln!(
                    "perf especulativa: {total} hojas y scripts pedidos en paralelo, listos en \
                     {:.1} ms",
                    start.elapsed().as_secs_f64() * 1000.0
                );
            }
        });
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
            .stack_size(STACK)
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
    let started = Instant::now();
    let (encoding, _) = bf_net::html_encoding_with_source(&body, mime.as_ref());
    // El encoding con que la carga decodifica el documento (`load::fetch_document_with`):
    // el de `document.characterSet`, el de respaldo de sus hojas y scripts.
    let (text, encoding) = bf_infra::encoding::decode(&body, encoding);
    let found = scan(&text, document);
    let quirks = document_quirks(&text);
    shared.lock().unwrap_or_else(|e| e.into_inner()).found = found.len();
    let refused = || shared.lock().unwrap_or_else(|e| e.into_inner()).decision == Some(false);
    let mut work: Vec<Work> = Vec::new();
    for f in found {
        if refused() {
            break;
        }
        let Some(staged) = cache.stage(&f.url, true) else {
            continue;
        };
        // E3: las hojas y los scripts clásicos de una respuesta correcta se parsean o compilan
        // después (los demás estados no los usa la carga como hoja o script). También los que
        // ya no están frescos: lo parseado o compilado va con sus bytes en la clave, así que
        // solo lo encuentra una carga que reciba esos mismos bytes, por ejemplo tras validarlos
        // con un 304 (RFC 9111 §4.3.4) o al servirlos dentro de `stale-while-revalidate`
        // (RFC 5861 §3). La respuesta guardada en sí no se usa: a la caché en memoria solo
        // entra lo fresco, como antes.
        if matches!(f.kind, Kind::Style | Kind::Script)
            && let Some(r) = staged.response()
            && (200..300).contains(&r.status)
        {
            work.push(Work {
                url: f.url.clone(),
                kind: f.kind,
                charset: f.charset,
                bytes: r.body.clone(),
                validator: staged.validator().map(str::to_owned),
            });
        }
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
    let cpu = Instant::now();
    let (mut sheets, mut scripts) = (0, 0);
    for w in work {
        if refused() {
            break;
        }
        match w.kind {
            // La clave con que la carga la busca (`bf_style::loader::load_sheet`): la URL final
            // (una respuesta 2xx guardada no redirige), los bytes, el modo y el encoding del
            // documento.
            Kind::Style => {
                if let Some(quirks) = quirks {
                    let ctx = bf_style::sheet_cache::SheetContext { quirks, encoding };
                    bf_style::sheet_cache::warm(&w.url, &w.bytes, &ctx);
                    sheets += 1;
                }
            }
            // El nombre y el texto con que la ventana lo evalúa (`bf_web::bom::script`): la URL
            // de la respuesta y los bytes decodificados con el `charset` o el del documento.
            Kind::Script => {
                let (text, _) = bf_infra::encoding::decode(&w.bytes, w.charset.unwrap_or(encoding));
                bf_js::code_cache::warm(&w.url.href(), w.validator.as_deref(), &text);
                scripts += 1;
            }
            Kind::Module | Kind::Image => {}
        }
    }
    if std::env::var_os("BF_TRACE").is_some() {
        eprintln!(
            "perf precarga: {sheets} hojas parseadas y {scripts} scripts compilados en {:.1} ms, \
             listos a los {:.1} ms de empezar",
            cpu.elapsed().as_secs_f64() * 1000.0,
            started.elapsed().as_secs_f64() * 1000.0
        );
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
                ("https://sitio.test/wiki/m.js".to_owned(), Kind::Module),
                ("https://sitio.test/c.js".to_owned(), Kind::Module),
                ("https://sitio.test/d.css".to_owned(), Kind::Style),
                ("https://sitio.test/e.png".to_owned(), Kind::Image),
            ]
        );
    }

    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// El script de `/b.js`: más largo que `code_cache::MIN_SOURCE_LEN`, para que se guarde.
    fn big_script() -> String {
        let mut s = String::from("var total = 0;\n");
        while s.len() < 2 * bf_js::code_cache::MIN_SOURCE_LEN {
            s.push_str("total = total + 1; // relleno para pasar del mínimo de la caché\n");
        }
        s
    }

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
                let script = big_script();
                let (status, headers, body): (&str, &str, &str) = match path.as_str() {
                    "/" if conditional && !changed.load(Ordering::SeqCst) => {
                        ("304 Not Modified", "ETag: \"v1\"\r\n", "")
                    }
                    "/" => (
                        "200 OK",
                        "Content-Type: text/html; charset=utf-8\r\n\
                         Cache-Control: max-age=0, must-revalidate\r\nETag: \"v1\"\r\n",
                        "<!doctype html><link rel=stylesheet href=/a.css>\
                         <link rel=stylesheet href=/c.css><script src=/b.js></script>",
                    ),
                    "/c.css" => ("200 OK", "Content-Type: text/css\r\n", "p{}"),
                    "/a.css" => (
                        "200 OK",
                        "Content-Type: text/css\r\nCache-Control: max-age=600\r\n",
                        "p { color: green }",
                    ),
                    "/b.js" => (
                        "200 OK",
                        "Content-Type: text/javascript\r\nCache-Control: max-age=600\r\n\
                         ETag: \"s1\"\r\n",
                        &script,
                    ),
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
        // E3: las hojas guardadas quedaron parseadas con la clave de la carga (documento sin
        // quirks, UTF-8), también `c.css`, que no está fresca (su clave lleva sus bytes), y el
        // script compilado con su URL. Una hoja con otros bytes no está.
        let ctx = bf_style::sheet_cache::SheetContext {
            quirks: false,
            encoding: Encoding::Utf8,
        };
        let sheet = |p: &str, body: &[u8]| {
            let u = url(&format!("http://127.0.0.1:{port}{p}"));
            bf_style::sheet_cache::get(&bf_style::sheet_cache::SheetKey::new(&u, body), &ctx)
        };
        assert!(sheet("/a.css", b"p { color: green }").is_some());
        assert!(sheet("/c.css", b"p{}").is_some());
        assert!(sheet("/c.css", b"p{ }").is_none());
        assert!(bf_js::code_cache::is_warm(
            &format!("http://127.0.0.1:{port}/b.js"),
            &big_script()
        ));
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

    /// E3: el modo del documento, solo en los casos seguros (HTML §13.2.6.4.1).
    #[test]
    fn quirks_from_the_doctype() {
        assert_eq!(document_quirks("<!DOCTYPE html><p>"), Some(false));
        assert_eq!(document_quirks("\n<!-- c --> <!doctype HTML>"), Some(false));
        assert_eq!(
            document_quirks("<!DOCTYPE html SYSTEM \"about:legacy-compat\">"),
            Some(false)
        );
        assert_eq!(document_quirks("<html><p>"), Some(true));
        assert_eq!(document_quirks(""), Some(true));
        assert_eq!(document_quirks("<!DOCTYPE foo>"), None);
        assert_eq!(
            document_quirks(
                "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Transitional//EN\" \
                 \"http://www.w3.org/TR/xhtml1/DTD/xhtml1-transitional.dtd\">"
            ),
            None
        );
    }

    /// El `charset` de un `<script>` va con lo encontrado.
    #[test]
    fn scanner_keeps_the_script_charset() {
        let doc = url("https://sitio.test/");
        let got: Vec<Option<Encoding>> = scan(
            "<script src=a.js charset=windows-1252></script><script src=b.js charset=nada>\
             </script>",
            &doc,
        )
        .into_iter()
        .map(|f| f.charset)
        .collect();
        assert_eq!(got, [Encoding::for_label("windows-1252"), None]);
        assert!(got[0].is_some());
    }

    /// E3: el `nonce` de los scripts y si hay CSP en un `<meta>` (para la descarga
    /// especulativa).
    #[test]
    fn scanner_reports_nonces_and_meta_csp() {
        let doc = url("https://sitio.test/");
        let s = scan_document(
            "<meta http-equiv=Content-Security-Policy content=\"script-src 'nonce-a'\">\
             <script nonce=a src=a.js></script><script src=b.js></script>",
            &doc,
        );
        assert!(s.meta_csp);
        let nonces: Vec<Option<&str>> = s.found.iter().map(|f| f.nonce.as_deref()).collect();
        assert_eq!(nonces, [Some("a"), None]);
        assert!(!scan_document("<meta http-equiv=refresh content=5>", &doc).meta_csp);
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
