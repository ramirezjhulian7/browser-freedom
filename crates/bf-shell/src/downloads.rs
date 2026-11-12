//! Las descargas: lo que no se muestra como página (`Content-Disposition: attachment` o un tipo
//! que el motor no renderiza) y lo que se guarda desde el menú contextual va a la carpeta de
//! descargas del usuario (`%USERPROFILE%\Downloads`), con el progreso en la barra inferior del
//! marco y en `freedom://downloads`.
//!
//! Una descarga nueva corre en su propio hilo con el cuerpo por partes
//! ([`bf_net::streaming::fetch_streaming`]): se escribe en `<nombre>.part` y al terminar se
//! renombra. El registro de las terminadas vive en el archivo `downloads` del perfil
//! ([`crate::profile`]), un registro por línea:
//!
//! ```text
//! # Freedom: descargas v1
//! <id>\t<inicio, s Unix>\t<estado>\t<bytes>\t<URL>\t<archivo>
//! ```
//!
//! con `<estado>` `ok`, `cancelada` o `error:<motivo>`.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use bf_infra::Url;

use crate::profile;

const HEADER: &str = "# Freedom: descargas v1";
/// Descargas que se recuerdan.
const MAX_KEPT: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Running,
    Done,
    Cancelled,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct Download {
    pub id: u64,
    pub url: String,
    /// El archivo final (mientras corre, se escribe en `<archivo>.part`).
    pub path: PathBuf,
    /// Tamaño anunciado (`Content-Length`), si se conoce.
    pub total: Option<u64>,
    pub received: u64,
    pub state: State,
    /// Inicio, en segundos Unix.
    pub started: u64,
    cancel: Arc<AtomicBool>,
}

impl Download {
    /// El nombre del archivo.
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
    }

    /// Fracción descargada (0 a 1) si se conoce el total.
    pub fn fraction(&self) -> Option<f32> {
        let total = self.total.filter(|t| *t > 0)?;
        Some((self.received as f64 / total as f64).clamp(0.0, 1.0) as f32)
    }

    /// "1,2 MB de 4,0 MB", "830 KB", "Listo · 4,0 MB", "Falló: …".
    pub fn status(&self) -> String {
        match &self.state {
            State::Running => match self.total {
                Some(t) => format!("{} de {}", human_size(self.received), human_size(t)),
                None => human_size(self.received),
            },
            State::Done => format!("Listo · {}", human_size(self.received)),
            State::Cancelled => "Cancelada".to_owned(),
            State::Failed(why) => format!("Falló: {why}"),
        }
    }

    fn line(&self) -> String {
        let state = match &self.state {
            State::Done => "ok".to_owned(),
            State::Cancelled => "cancelada".to_owned(),
            State::Failed(why) => format!("error:{why}"),
            State::Running => "cancelada".to_owned(),
        };
        profile::join_record(&[
            &self.id.to_string(),
            &self.started.to_string(),
            &state,
            &self.received.to_string(),
            &self.url,
            &self.path.to_string_lossy(),
        ])
    }

    fn parse(line: &str) -> Option<Download> {
        let f = profile::split_record(line)?;
        let [id, started, state, bytes, url, path] = &f[..] else {
            return None;
        };
        let state = match state.as_str() {
            "ok" => State::Done,
            "cancelada" => State::Cancelled,
            s => State::Failed(s.strip_prefix("error:").unwrap_or(s).to_owned()),
        };
        let received = bytes.parse().ok()?;
        Some(Download {
            id: id.parse().ok()?,
            url: url.clone(),
            path: PathBuf::from(path),
            total: Some(received),
            received,
            state,
            started: started.parse().ok()?,
            cancel: Arc::new(AtomicBool::new(false)),
        })
    }
}

/// `bytes` en B, KB, MB o GB (base 1024, coma decimal).
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit + 1 < UNITS.len() {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[unit]).replace('.', ",")
    }
}

/// ¿La respuesta se descarga en lugar de mostrarse? Con `Content-Disposition: attachment`
/// (RFC 6266 §4.2), o si su tipo no es uno que el motor muestre como documento (HTML, XML,
/// SVG, texto, JSON, imágenes). Sin `Content-Type` se muestra (el motor la trata como HTML).
pub fn should_download(content_type: Option<&str>, disposition: Option<&str>) -> bool {
    if disposition.is_some_and(|d| {
        d.split(';')
            .next()
            .is_some_and(|t| t.trim().eq_ignore_ascii_case("attachment"))
    }) {
        return true;
    }
    let Some(ct) = content_type else {
        return false;
    };
    let essence = ct
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if essence.is_empty() {
        return false;
    }
    let renderable = essence.starts_with("text/")
        || essence.starts_with("image/")
        || essence.ends_with("+xml")
        || essence.ends_with("+json")
        || matches!(
            essence.as_str(),
            "application/xml"
                | "application/xhtml+xml"
                | "application/json"
                | "application/javascript"
                | "application/ecmascript"
                | "application/x-javascript"
        );
    !renderable
}

/// Extensiones de archivo que nunca son una página: un enlace `http(s)` que termina así se
/// descarga directamente (con progreso, sin dejar la página actual).
const DOWNLOAD_EXTENSIONS: &[&str] = &[
    "zip", "7z", "rar", "tar", "gz", "tgz", "bz2", "xz", "zst", "exe", "msi", "msix", "dmg", "pkg",
    "deb", "rpm", "apk", "appimage", "iso", "img", "bin", "pdf", "epub", "doc", "docx", "xls",
    "xlsx", "ppt", "pptx", "odt", "ods", "odp", "mp3", "flac", "wav", "ogg", "m4a", "mp4", "mkv",
    "avi", "mov", "webm", "torrent", "ttf", "otf", "woff", "woff2",
];

/// ¿`url` es un archivo para descargar por su extensión ([`DOWNLOAD_EXTENSIONS`])?
pub fn looks_like_download(url: &Url) -> bool {
    if !matches!(url.scheme.as_str(), "http" | "https") {
        return false;
    }
    let path = url.path_string().to_ascii_lowercase();
    let last = path.rsplit('/').next().unwrap_or("");
    last.rsplit_once('.')
        .is_some_and(|(stem, ext)| !stem.is_empty() && DOWNLOAD_EXTENSIONS.contains(&ext))
}

/// Muestra `path` en el explorador de archivos del sistema: seleccionado en Windows
/// (`explorer /select,`) y en macOS (`open -R`, en el Finder); en Linux se abre su carpeta.
pub fn reveal(path: &Path) {
    let (program, args) = reveal_command(path);
    if let Err(e) = std::process::Command::new(program).args(&args).spawn() {
        eprintln!(
            "aviso: no se pudo abrir la carpeta de {}: {e}",
            path.display()
        );
    }
}

/// El programa y los argumentos de [`reveal`] en esta plataforma. Si el archivo ya no existe
/// (se movió o se borró), se abre su carpeta.
pub fn reveal_command(path: &Path) -> (&'static str, Vec<std::ffi::OsString>) {
    let dir = path.parent().unwrap_or(path);
    if cfg!(windows) {
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(path);
        ("explorer", vec![arg])
    } else if cfg!(target_os = "macos") {
        if path.exists() {
            ("open", vec!["-R".into(), path.into()])
        } else {
            ("open", vec![dir.into()])
        }
    } else {
        ("xdg-open", vec![dir.into()])
    }
}

/// El nombre de archivo de `Content-Disposition` (RFC 6266 §4.3): `filename*` (RFC 8187,
/// `UTF-8''…` con %XX) tiene prioridad sobre `filename`.
pub fn disposition_filename(header: &str) -> Option<String> {
    let mut plain = None;
    let mut extended = None;
    for part in split_params(header).into_iter().skip(1) {
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        if name == "filename*" {
            let mut pieces = value.splitn(3, '\'');
            let (charset, _lang, encoded) = (pieces.next()?, pieces.next(), pieces.next());
            if let Some(encoded) = encoded {
                let bytes = bf_url::percent::decode(encoded.as_bytes());
                let text = if charset.eq_ignore_ascii_case("utf-8") {
                    String::from_utf8_lossy(&bytes).into_owned()
                } else {
                    // ISO-8859-1: cada byte es su punto de código.
                    bytes.iter().map(|&b| char::from(b)).collect()
                };
                extended = Some(text);
            }
        } else if name == "filename" {
            let v = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .map_or_else(
                    || value.to_owned(),
                    |q| q.replace("\\\"", "\"").replace("\\\\", "\\"),
                );
            plain = Some(v);
        }
    }
    extended
        .or(plain)
        .map(|n| sanitize(&n))
        .filter(|n| !n.is_empty())
}

/// Los parámetros separados por `;`, sin cortar dentro de comillas.
fn split_params(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for c in s.chars() {
        match c {
            _ if escaped => {
                escaped = false;
                cur.push(c);
            }
            '\\' if quoted => {
                escaped = true;
                cur.push(c);
            }
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ';' if !quoted => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// Un nombre de archivo seguro: sin carpetas (solo lo que sigue a la última `/` o `\`), sin
/// caracteres reservados ni de control, sin puntos ni espacios en los extremos, y sin los
/// nombres reservados de Windows (`CON`, `NUL`, `COM1`…).
pub fn sanitize(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let mut out: String = base
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    out = out
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .to_owned();
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        out.insert(0, '_');
    }
    out.chars().take(200).collect()
}

/// El nombre para guardar una respuesta: el de `Content-Disposition`, o el último segmento de
/// la URL, o `descarga`; con extensión según el tipo si no tiene.
pub fn filename_for(url: &Url, disposition: Option<&str>, content_type: Option<&str>) -> String {
    let from_url = || {
        let last = url
            .path_string()
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_owned();
        let decoded =
            String::from_utf8_lossy(&bf_url::percent::decode(last.as_bytes())).into_owned();
        Some(sanitize(&decoded)).filter(|n| !n.is_empty())
    };
    let name = disposition
        .and_then(disposition_filename)
        .or_else(from_url)
        .unwrap_or_else(|| "descarga".to_owned());
    if name.contains('.') {
        return name;
    }
    let ext = match content_type
        .and_then(|c| c.split(';').next())
        .map(|c| c.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("application/pdf") => "pdf",
        Some("application/zip") => "zip",
        Some("application/gzip" | "application/x-gzip") => "gz",
        Some("application/json") => "json",
        Some("text/plain") => "txt",
        Some("text/html") => "html",
        Some("text/csv") => "csv",
        Some("image/png") => "png",
        Some("image/jpeg") => "jpg",
        Some("image/gif") => "gif",
        Some("image/webp") => "webp",
        Some("image/svg+xml") => "svg",
        Some("audio/mpeg") => "mp3",
        Some("video/mp4") => "mp4",
        _ => return name,
    };
    format!("{name}.{ext}")
}

/// Una ruta libre para `name` en `dir`: `name`, o `nombre (1).ext`, `nombre (2).ext`…
/// (sin pisar tampoco un `.part` de una descarga en curso).
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let taken = |p: &Path| {
        let mut part = p.as_os_str().to_owned();
        part.push(".part");
        p.exists() || Path::new(&part).exists()
    };
    let first = dir.join(name);
    if !taken(&first) {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (1..10_000)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !taken(p))
        .unwrap_or(first)
}

static DIR_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Fija la carpeta de descargas (sesiones sin ventana y pruebas: no escriben en la del
/// usuario).
pub fn set_dir(dir: PathBuf) {
    *DIR_OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir);
}

/// La carpeta de descargas: la fijada con [`set_dir`], `FREEDOM_DOWNLOADS`, la de XDG en
/// Linux, `Downloads` de la carpeta personal, o la personal.
pub fn downloads_dir() -> PathBuf {
    if let Some(d) = DIR_OVERRIDE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        return d;
    }
    if let Some(d) = std::env::var_os("FREEDOM_DOWNLOADS").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .filter(|h| h.is_dir());
    let xdg = home.as_ref().and_then(|h| {
        let dirs = std::fs::read_to_string(h.join(".config/user-dirs.dirs")).ok()?;
        let line = dirs
            .lines()
            .find_map(|l| l.trim().strip_prefix("XDG_DOWNLOAD_DIR="))?;
        Some(PathBuf::from(
            line.trim_matches('"')
                .replace("$HOME", &h.to_string_lossy()),
        ))
    });
    xdg.into_iter()
        .chain(home.as_ref().map(|h| h.join("Downloads")))
        .find(|d| d.is_dir())
        .or(home)
        .unwrap_or_else(std::env::temp_dir)
}

/// Las descargas de esta sesión y las recordadas.
#[derive(Debug, Default)]
pub struct Downloads {
    path: Option<PathBuf>,
    items: Vec<Download>,
    next_id: u64,
}

impl Downloads {
    pub fn memory() -> Downloads {
        Downloads {
            next_id: 1,
            ..Downloads::default()
        }
    }

    pub fn open(path: PathBuf) -> Downloads {
        let mut d = Downloads {
            path: Some(path),
            next_id: 1,
            ..Downloads::default()
        };
        let text = d
            .path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        for item in text.lines().filter_map(Download::parse) {
            d.next_id = d.next_id.max(item.id + 1);
            d.items.push(item);
        }
        d
    }

    fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let mut out = String::from(HEADER);
        out.push('\n');
        let finished: Vec<&Download> = self
            .items
            .iter()
            .filter(|d| d.state != State::Running)
            .collect();
        for d in finished
            .iter()
            .skip(finished.len().saturating_sub(MAX_KEPT))
        {
            out.push_str(&d.line());
            out.push('\n');
        }
        if let Err(e) = profile::write_atomic(path, &out) {
            eprintln!(
                "aviso: no se pudo guardar la lista de descargas en {}: {e}",
                path.display()
            );
        }
    }

    /// Las descargas, la más reciente primero.
    pub fn list(&self) -> Vec<&Download> {
        self.items.iter().rev().collect()
    }

    pub fn get(&self, id: u64) -> Option<&Download> {
        self.items.iter().find(|d| d.id == id)
    }

    pub fn running(&self) -> usize {
        self.items
            .iter()
            .filter(|d| d.state == State::Running)
            .count()
    }

    /// Agrega una descarga en curso hacia `path`; devuelve su id y su señal de cancelar.
    pub fn begin(&mut self, url: &str, path: PathBuf, now: u64) -> (u64, Arc<AtomicBool>) {
        let id = self.next_id.max(1);
        self.next_id = id + 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.items.push(Download {
            id,
            url: url.to_owned(),
            path,
            total: None,
            received: 0,
            state: State::Running,
            started: now,
            cancel: cancel.clone(),
        });
        (id, cancel)
    }

    /// Avance de la descarga `id`.
    pub fn progress(&mut self, id: u64, received: u64, total: Option<u64>) {
        if let Some(d) = self.items.iter_mut().find(|d| d.id == id) {
            d.received = received;
            if total.is_some() {
                d.total = total;
            }
        }
    }

    /// La descarga `id` terminó con `state`; queda en el registro.
    pub fn finish(&mut self, id: u64, state: State) {
        if let Some(d) = self.items.iter_mut().find(|d| d.id == id) {
            d.state = state;
        }
        self.save();
    }

    /// Pide cancelar la descarga `id` (el hilo corta en el siguiente trozo).
    pub fn cancel(&self, id: u64) {
        if let Some(d) = self.items.iter().find(|d| d.id == id) {
            d.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Quita del registro las que no están en curso.
    pub fn clear_finished(&mut self) {
        self.items.retain(|d| d.state == State::Running);
        self.save();
    }

    /// Quita una del registro (no borra el archivo).
    pub fn forget(&mut self, id: u64) {
        self.items
            .retain(|d| d.id != id || d.state == State::Running);
        self.save();
    }
}

/// Las descargas del navegador (registro en el perfil, o en memoria).
pub fn global() -> MutexGuard<'static, Downloads> {
    static GLOBAL: OnceLock<Mutex<Downloads>> = OnceLock::new();
    GLOBAL
        .get_or_init(|| {
            Mutex::new(match profile::file("downloads") {
                Some(p) => Downloads::open(p),
                None => Downloads::memory(),
            })
        })
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Lo que recibe el cuerpo de una descarga: lo escribe en el `.part` y anota el avance.
struct Sink {
    id: u64,
    file: std::fs::File,
    received: u64,
    total: Option<u64>,
    status: u16,
    cancel: Arc<AtomicBool>,
    error: Option<String>,
    wake: Arc<dyn Fn() + Send + Sync>,
    last_wake: std::time::Instant,
}

impl bf_net::streaming::StreamSink for Sink {
    fn head(&mut self, head: &bf_net::streaming::StreamHead) -> bool {
        self.status = head.status;
        if !(200..300).contains(&head.status) {
            self.error = Some(format!("HTTP {} {}", head.status, head.status_text));
            return false;
        }
        self.total = head
            .headers
            .get("content-length")
            .and_then(|v| v.trim().parse().ok());
        global().progress(self.id, 0, self.total);
        (self.wake)();
        true
    }

    fn chunk(&mut self, bytes: &[u8]) -> bool {
        if self.cancel.load(Ordering::Relaxed) {
            return false;
        }
        if let Err(e) = self.file.write_all(bytes) {
            self.error = Some(e.to_string());
            return false;
        }
        self.received += bytes.len() as u64;
        global().progress(self.id, self.received, None);
        // Como mucho ~10 avisos por segundo a la ventana.
        if self.last_wake.elapsed() >= std::time::Duration::from_millis(100) {
            self.last_wake = std::time::Instant::now();
            (self.wake)();
        }
        true
    }
}

fn part_path(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".part");
    PathBuf::from(p)
}

/// Empieza a descargar `url` (en un hilo) a la carpeta de descargas, con el nombre `name` o
/// el que dé la URL. `wake` se llama cuando hay avance para mostrar. Devuelve el id.
pub fn start(
    url: &Url,
    name: Option<&str>,
    net: Arc<bf_net::NetLoader>,
    wake: Arc<dyn Fn() + Send + Sync>,
) -> u64 {
    let name = name
        .map(sanitize)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| filename_for(url, None, None));
    let dir = downloads_dir();
    let path = unique_path(&dir, &name);
    let (id, cancel) = global().begin(&url.href(), path.clone(), profile::now());
    let url = url.clone();
    std::thread::Builder::new()
        .name(format!("descarga-{id}"))
        .spawn(move || {
            let state = run(id, &url, &path, &net, cancel, wake.clone());
            global().finish(id, state);
            wake();
        })
        .ok();
    id
}

fn run(
    id: u64,
    url: &Url,
    path: &Path,
    net: &bf_net::NetLoader,
    cancel: Arc<AtomicBool>,
    wake: Arc<dyn Fn() + Send + Sync>,
) -> State {
    let part = part_path(path);
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
    let file = match std::fs::File::create(&part) {
        Ok(f) => f,
        Err(e) => return State::Failed(e.to_string()),
    };
    let state = if matches!(url.scheme.as_str(), "http" | "https") {
        let mut sink = Sink {
            id,
            file,
            received: 0,
            total: None,
            status: 0,
            cancel: cancel.clone(),
            error: None,
            wake,
            last_wake: std::time::Instant::now(),
        };
        // Con las cookies del navegador en cada salto (`credentials` por omisión).
        let result = bf_net::streaming::fetch_streaming(
            url,
            &bf_net::RequestOptions::default(),
            Some(&net.fetcher.cookies),
            &mut sink,
        );
        let _ = sink.file.flush();
        if cancel.load(Ordering::Relaxed) {
            State::Cancelled
        } else if let Some(e) = sink.error {
            State::Failed(e)
        } else {
            match result {
                Ok(()) => State::Done,
                Err(e) => State::Failed(e.to_string()),
            }
        }
    } else {
        // `file:`, `data:`, `blob:`: de una vez.
        let mut file = file;
        match bf_infra::ResourceLoader::load(net, url) {
            Some(res) => match file.write_all(&res.bytes) {
                Ok(()) => {
                    let n = res.bytes.len() as u64;
                    global().progress(id, n, Some(n));
                    State::Done
                }
                Err(e) => State::Failed(e.to_string()),
            },
            None => State::Failed("no se pudo leer".to_owned()),
        }
    };
    if state == State::Done {
        if let Err(e) = std::fs::rename(&part, path) {
            return State::Failed(e.to_string());
        }
    } else {
        let _ = std::fs::remove_file(&part);
    }
    state
}

/// Guarda `bytes` ya recibidos (una respuesta que el documento no muestra) como descarga
/// terminada. Devuelve la ruta.
pub fn save_bytes(url: &str, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let path = unique_path(&downloads_dir(), name);
    let (id, _) = global().begin(url, path.clone(), profile::now());
    let result =
        std::fs::create_dir_all(downloads_dir()).and_then(|()| std::fs::write(&path, bytes));
    let mut g = global();
    g.progress(id, bytes.len() as u64, Some(bytes.len() as u64));
    match result {
        Ok(()) => {
            g.finish(id, State::Done);
            Ok(path)
        }
        Err(e) => {
            g.finish(id, State::Failed(e.to_string()));
            Err(e.to_string())
        }
    }
}
