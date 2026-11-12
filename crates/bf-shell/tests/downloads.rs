//! Las descargas (`src/downloads.rs`): qué respuesta se descarga, el nombre del archivo
//! (RFC 6266 / RFC 8187), nombres seguros, el registro en el perfil y la descarga en su hilo
//! contra `file:` y un servidor HTTP local. Los módulos se incluyen por ruta porque `bf-shell`
//! es solo un binario.

#![forbid(unsafe_code)]
#![allow(dead_code)]

#[path = "../src/downloads.rs"]
mod downloads;
#[path = "../src/profile.rs"]
mod profile;

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use bf_url::Url;
use downloads::{Downloads, State};

fn url(s: &str) -> Url {
    Url::parse(s, None).expect("URL válida")
}

/// Una carpeta vacía y propia de cada prueba.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "freedom-test-descargas-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("carpeta temporal");
    dir
}

/// El registro global en memoria y la carpeta de descargas en una temporal (compartida por
/// las pruebas de este archivo: los nombres no chocan).
fn global_setup() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        profile::set_ephemeral();
        let dir = temp_dir("global");
        downloads::set_dir(dir.clone());
        dir
    })
    .clone()
}

/// Espera a que la descarga `id` termine (hasta 20 s).
fn wait(id: u64) -> downloads::Download {
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        let d = downloads::global().get(id).cloned().expect("descarga");
        if d.state != State::Running || Instant::now() > until {
            return d;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn sizes() {
    assert_eq!(downloads::human_size(0), "0 B");
    assert_eq!(downloads::human_size(1023), "1023 B");
    assert_eq!(downloads::human_size(1536), "1,5 KB");
    assert_eq!(downloads::human_size(4 * 1024 * 1024), "4,0 MB");
}

#[test]
fn what_downloads() {
    use downloads::should_download as dl;
    // RFC 6266 §4.2: el tipo de disposición sin distinguir mayúsculas.
    assert!(dl(Some("text/html"), Some("attachment")));
    assert!(dl(None, Some("Attachment; filename=a.txt")));
    assert!(!dl(Some("text/html"), Some("inline")));
    assert!(!dl(
        Some("text/html"),
        Some("inline; filename=\"attachment\"")
    ));
    // Tipos que el motor no muestra como documento.
    assert!(dl(Some("application/zip"), None));
    assert!(dl(Some("application/octet-stream"), None));
    assert!(dl(Some("application/pdf"), None));
    assert!(dl(Some("video/mp4"), None));
    // Lo que sí se muestra.
    assert!(!dl(Some("text/html; charset=utf-8"), None));
    assert!(!dl(Some("text/plain"), None));
    assert!(!dl(Some("application/xhtml+xml"), None));
    assert!(!dl(Some("image/svg+xml"), None));
    assert!(!dl(Some("image/png"), None));
    assert!(!dl(Some("application/json"), None));
    assert!(!dl(Some("application/ld+json"), None));
    assert!(!dl(None, None));
    assert!(!dl(Some(""), None));
}

#[test]
fn by_extension() {
    use downloads::looks_like_download as dl;
    assert!(dl(&url("https://example.com/files/setup.EXE")));
    assert!(dl(&url("https://example.com/a/b/libro.pdf?x=1")));
    assert!(dl(&url("http://example.com/archivo.tar.gz")));
    assert!(!dl(&url("https://example.com/")));
    assert!(!dl(&url("https://example.com/index.html")));
    assert!(!dl(&url("https://example.com/zip")));
    assert!(!dl(&url("https://example.com/.zip")));
    assert!(!dl(&url("https://example.com/pdf/")));
    // Solo http(s): un archivo local se abre.
    assert!(!dl(&url("file:///C:/x/a.zip")));
}

#[test]
fn disposition_names() {
    use downloads::disposition_filename as name;
    assert_eq!(
        name("attachment; filename=\"informe final.pdf\"").as_deref(),
        Some("informe final.pdf")
    );
    assert_eq!(
        name("attachment; filename=plano.txt").as_deref(),
        Some("plano.txt")
    );
    // RFC 6266 §4.3: `filename*` (RFC 8187) gana sobre `filename`, en cualquier orden.
    assert_eq!(
        name("attachment; filename=\"a.pdf\"; filename*=UTF-8''informe%20a%C3%B1o.pdf").as_deref(),
        Some("informe año.pdf")
    );
    assert_eq!(
        name("attachment; filename*=utf-8'es'caf%C3%A9.txt; filename=\"cafe.txt\"").as_deref(),
        Some("café.txt")
    );
    assert_eq!(
        name("attachment; filename*=iso-8859-1''ni%F1o.txt").as_deref(),
        Some("niño.txt")
    );
    // `;` y comillas escapadas dentro de comillas.
    assert_eq!(
        name("attachment; filename=\"a;b \\\"c\\\".txt\"").as_deref(),
        Some("a;b _c_.txt")
    );
    // Sin carpetas: solo el último segmento.
    assert_eq!(
        name("attachment; filename=\"../../Windows/win.ini\"").as_deref(),
        Some("win.ini")
    );
    assert_eq!(
        name("attachment; filename=\"..\\\\x\\\\y.bat\"").as_deref(),
        Some("y.bat")
    );
    assert_eq!(name("attachment").as_deref(), None);
    assert_eq!(name("attachment; filename=\"\"").as_deref(), None);
}

#[test]
fn safe_names() {
    use downloads::sanitize;
    assert_eq!(sanitize("a:b?c*.txt"), "a_b_c_.txt");
    assert_eq!(sanitize(" .oculto. "), "oculto");
    assert_eq!(sanitize("CON"), "_CON");
    assert_eq!(sanitize("nul.txt"), "_nul.txt");
    assert_eq!(sanitize("com1.log"), "_com1.log");
    assert_eq!(sanitize("COM10.log"), "COM10.log");
    assert_eq!(sanitize("console.txt"), "console.txt");
    assert_eq!(sanitize("x/y\\z.txt"), "z.txt");
    assert_eq!(sanitize("a\u{7}b"), "a_b");
    assert_eq!(sanitize(&"n".repeat(300)).chars().count(), 200);
}

#[test]
fn names_for_responses() {
    use downloads::filename_for as name;
    assert_eq!(
        name(
            &url("https://e.com/d/informe"),
            None,
            Some("application/pdf")
        ),
        "informe.pdf"
    );
    assert_eq!(
        name(
            &url("https://e.com/d/caf%C3%A9.zip"),
            None,
            Some("application/zip")
        ),
        "café.zip"
    );
    assert_eq!(
        name(
            &url("https://e.com/get?id=1"),
            Some("attachment; filename=\"datos.csv\""),
            Some("text/csv")
        ),
        "datos.csv"
    );
    assert_eq!(name(&url("https://e.com/"), None, None), "descarga");
    assert_eq!(
        name(
            &url("https://e.com/"),
            None,
            Some("text/plain; charset=utf-8")
        ),
        "descarga.txt"
    );
}

#[test]
fn free_paths() {
    let dir = temp_dir("unique");
    assert_eq!(downloads::unique_path(&dir, "a.txt"), dir.join("a.txt"));
    std::fs::write(dir.join("a.txt"), b"1").unwrap();
    assert_eq!(downloads::unique_path(&dir, "a.txt"), dir.join("a (1).txt"));
    // Una descarga en curso (`.part`) también ocupa el nombre.
    std::fs::write(dir.join("a (1).txt.part"), b"").unwrap();
    assert_eq!(downloads::unique_path(&dir, "a.txt"), dir.join("a (2).txt"));
    std::fs::write(dir.join("LEEME"), b"").unwrap();
    assert_eq!(downloads::unique_path(&dir, "LEEME"), dir.join("LEEME (1)"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn record_survives_reopen() {
    let dir = temp_dir("registro");
    let file = dir.join("downloads");
    let mut d = Downloads::open(file.clone());
    let (a, _) = d.begin("https://e.com/a.zip", dir.join("a.zip"), 100);
    d.progress(a, 10, Some(40));
    assert_eq!(d.get(a).unwrap().fraction(), Some(0.25));
    assert_eq!(d.get(a).unwrap().status(), "10 B de 40 B");
    d.progress(a, 40, None);
    d.finish(a, State::Done);
    let (b, _) = d.begin("https://e.com/b\tc.zip", dir.join("b.zip"), 200);
    d.finish(b, State::Failed("HTTP 404 Not Found".to_owned()));
    // Una en curso no se guarda.
    let (c, cancel) = d.begin("https://e.com/c.zip", dir.join("c.zip"), 300);
    assert_eq!(d.running(), 1);
    d.cancel(c);
    assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));

    let again = Downloads::open(file.clone());
    let list = again.list();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].url, "https://e.com/b\tc.zip");
    assert_eq!(
        list[0].state,
        State::Failed("HTTP 404 Not Found".to_owned())
    );
    assert_eq!(list[1].state, State::Done);
    assert_eq!(list[1].received, 40);
    assert_eq!(list[1].name(), "a.zip");
    assert_eq!(list[1].started, 100);
    assert_eq!(list[1].status(), "Listo · 40 B");
    // Los ids siguen después de los recordados.
    let mut again = again;
    let (next, _) = again.begin("https://e.com/d.zip", dir.join("d.zip"), 400);
    assert!(next > b);
    again.forget(a);
    assert!(again.get(a).is_none());
    // `forget` no quita una en curso.
    again.forget(next);
    assert!(again.get(next).is_some());
    again.clear_finished();
    assert_eq!(again.list().len(), 1);
    assert!(Downloads::open(file).list().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn save_received_bytes() {
    let dir = global_setup();
    let path = downloads::save_bytes("https://e.com/x", "guardado.bin", b"hola").unwrap();
    assert_eq!(path, dir.join("guardado.bin"));
    assert_eq!(std::fs::read(&path).unwrap(), b"hola");
    let g = downloads::global();
    let d = g.list().into_iter().find(|d| d.path == path).unwrap();
    assert_eq!(d.state, State::Done);
    assert_eq!(d.received, 4);
}

fn net() -> Arc<bf_net::NetLoader> {
    Arc::new(bf_net::NetLoader::new())
}

#[test]
fn download_local_file() {
    let dir = global_setup();
    let src = temp_dir("origen").join("origen.txt");
    std::fs::write(&src, "contenido ñ").unwrap();
    let src_url = Url::parse(
        &format!("file:///{}", src.display().to_string().replace('\\', "/")),
        None,
    )
    .unwrap();
    let id = downloads::start(&src_url, Some("copia.txt"), net(), Arc::new(|| {}));
    let d = wait(id);
    assert_eq!(d.state, State::Done, "{d:?}");
    assert_eq!(d.path, dir.join("copia.txt"));
    assert_eq!(std::fs::read_to_string(&d.path).unwrap(), "contenido ñ");
    assert!(!dir.join("copia.txt.part").exists());
}

/// Un servidor HTTP de una sola respuesta: `head` y luego cada trozo de `chunks`, con una
/// pausa de `gap` entre ellos.
fn serve_once(head: String, chunks: Vec<Vec<u8>>, gap: Duration) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                break;
            }
        }
        let mut stream = stream;
        let _ = stream.write_all(head.as_bytes());
        for c in chunks {
            std::thread::sleep(gap);
            if stream.write_all(&c).is_err() {
                return;
            }
            let _ = stream.flush();
        }
    });
    format!("http://{addr}")
}

#[test]
fn download_over_http() {
    let dir = global_setup();
    let body: Vec<u8> = (0..30_000u32).map(|i| (i % 251) as u8).collect();
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let chunks = body.chunks(10_000).map(<[u8]>::to_vec).collect();
    let base = serve_once(head, chunks, Duration::from_millis(30));
    let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let w = woken.clone();
    let id = downloads::start(
        &url(&format!("{base}/paquete.zip")),
        None,
        net(),
        Arc::new(move || {
            w.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }),
    );
    let d = wait(id);
    assert_eq!(d.state, State::Done, "{d:?}");
    assert_eq!(d.name(), "paquete.zip");
    assert_eq!(d.total, Some(30_000));
    assert_eq!(d.received, 30_000);
    assert_eq!(std::fs::read(dir.join("paquete.zip")).unwrap(), body);
    // Al menos el aviso de la cabecera y el del final.
    assert!(woken.load(std::sync::atomic::Ordering::Relaxed) >= 2);
}

#[test]
fn http_error_fails() {
    let dir = global_setup();
    let head =
        "HTTP/1.1 404 Not Found\r\nContent-Length: 2\r\nConnection: close\r\n\r\nno".to_owned();
    let base = serve_once(head, Vec::new(), Duration::ZERO);
    let id = downloads::start(
        &url(&format!("{base}/falta.zip")),
        None,
        net(),
        Arc::new(|| {}),
    );
    let d = wait(id);
    assert_eq!(d.state, State::Failed("HTTP 404 Not Found".to_owned()));
    assert!(!dir.join("falta.zip").exists());
    assert!(!dir.join("falta.zip.part").exists());
}

#[test]
fn cancel_stops_and_removes_part() {
    let dir = global_setup();
    let head = "HTTP/1.1 200 OK\r\nContent-Length: 100000\r\nConnection: close\r\n\r\n".to_owned();
    let chunks = vec![vec![b'x'; 1000]; 100];
    let base = serve_once(head, chunks, Duration::from_millis(50));
    let id = downloads::start(
        &url(&format!("{base}/grande.iso")),
        None,
        net(),
        Arc::new(|| {}),
    );
    let until = Instant::now() + Duration::from_secs(20);
    while downloads::global().get(id).unwrap().received == 0 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(dir.join("grande.iso.part").exists());
    downloads::global().cancel(id);
    let d = wait(id);
    assert_eq!(d.state, State::Cancelled);
    assert!(d.received < 100_000);
    assert!(!dir.join("grande.iso").exists());
    assert!(!dir.join("grande.iso.part").exists());
}
