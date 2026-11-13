//! Caché HTTP privada (RFC 9111), en memoria y opcionalmente en disco.
//!
//! [`HttpCache::fetch`] es el paso *HTTP-network-or-cache fetch* de Fetch (§4.5) para un salto:
//! busca una respuesta guardada para la petición (clave por URL y credenciales, variantes por
//! `Vary`, §4.1), la sirve si está fresca (§4.2), la valida con `If-None-Match` /
//! `If-Modified-Since` si caducó (§4.3), y si no va a la red con la función que le pasa quien
//! llama. Después guarda lo que se pueda guardar (§3), actualiza lo validado con un 304
//! (§4.3.4) e invalida tras métodos inseguros (§4.4). También sirve rangos desde una respuesta
//! completa o parcial guardada, combina las parciales y completa una parcial pidiendo lo que
//! falta (§3.3–3.4). Entiende `immutable` (RFC 8246) y `stale-while-revalidate` (RFC 5861): lo
//! segundo deja [`Revalidation`]s pendientes que quien llama hace en segundo plano.

use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use bf_url::Url;

use crate::http::Headers;

/// El *cache mode* de una petición (Fetch §2.2.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CacheMode {
    #[default]
    Default,
    NoStore,
    Reload,
    NoCache,
    ForceCache,
    OnlyIfCached,
}

impl CacheMode {
    /// El valor de `RequestInit.cache`.
    pub fn parse(s: &str) -> Option<CacheMode> {
        Some(match s {
            "default" => CacheMode::Default,
            "no-store" => CacheMode::NoStore,
            "reload" => CacheMode::Reload,
            "no-cache" => CacheMode::NoCache,
            "force-cache" => CacheMode::ForceCache,
            "only-if-cached" => CacheMode::OnlyIfCached,
            _ => return None,
        })
    }
}

/// Qué hizo la caché con una petición.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CacheUse {
    /// La respuesta vino de la red (y quizá se guardó).
    #[default]
    Network,
    /// Servida de la caché, sin red.
    Hit,
    /// Validada con la red: el servidor respondió 304 y se sirvió la guardada.
    Revalidated,
    /// `only-if-cached` sin nada guardado: 504 sin red.
    Unavailable,
}

/// La petición tal como se mandaría a la red.
#[derive(Debug, Clone)]
pub struct CacheRequest<'a> {
    pub method: &'a str,
    pub url: &'a Url,
    /// Todas las cabeceras de la petición (las de `Vary` y las directivas salen de aquí).
    pub headers: &'a Headers,
    pub mode: CacheMode,
    /// ¿Lleva credenciales? Las respuestas con y sin credenciales se guardan aparte.
    pub credentials: bool,
}

/// Una respuesta, ya sin `Content-Encoding` aplicado al cuerpo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheResponse {
    pub status: u16,
    pub status_text: String,
    pub headers: Headers,
    pub body: Vec<u8>,
}

// --- Directivas y fechas ---

/// Las directivas de `Cache-Control` (RFC 9111 §5.2), en minúsculas y sin comillas.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Directives(Vec<(String, Option<String>)>);

impl Directives {
    pub fn of(headers: &Headers) -> Directives {
        let mut out = Vec::new();
        for value in headers.get_all("cache-control") {
            for part in split_list(value) {
                let (name, arg) = match part.split_once('=') {
                    Some((n, v)) => (n.trim(), Some(unquote(v.trim()))),
                    None => (part.trim(), None),
                };
                if !name.is_empty() {
                    out.push((name.to_ascii_lowercase(), arg));
                }
            }
        }
        Directives(out)
    }

    pub fn has(&self, name: &str) -> bool {
        self.0.iter().any(|(n, _)| n == name)
    }

    /// El argumento en segundos de la primera aparición de `name` (§1.2.2: delta-seconds).
    /// `Some(None)` si está pero el valor no es válido.
    pub fn seconds(&self, name: &str) -> Option<Option<u64>> {
        let (_, v) = self.0.iter().find(|(n, _)| n == name)?;
        Some(v.as_deref().and_then(|v| {
            if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) {
                // §1.2.2: lo que desborde se toma como 2^31.
                Some(v.parse().unwrap_or(1 << 31))
            } else {
                None
            }
        }))
    }
}

/// Parte una lista de cabecera por comas que no estén entre comillas.
fn split_list(v: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut quoted, mut escaped) = (0, false, false);
    for (i, c) in v.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ',' if !quoted => {
                out.push(v[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(v[start..].trim());
    out.retain(|s| !s.is_empty());
    out
}

fn unquote(v: &str) -> String {
    match v.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        Some(inner) => {
            let mut out = String::new();
            let mut chars = inner.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    out.extend(chars.next());
                } else {
                    out.push(c);
                }
            }
            out
        }
        None => v.to_owned(),
    }
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

/// Días desde 1970-01-01 de una fecha civil (algoritmo de Howard Hinnant).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// RFC 9110 §5.6.7: `HTTP-date` (IMF-fixdate, y los obsoletos RFC 850 y asctime). Devuelve
/// segundos desde 1970.
pub fn parse_http_date(s: &str) -> Option<i64> {
    let s = s.trim();
    let tokens: Vec<&str> = s.split([' ', ',', '-']).filter(|t| !t.is_empty()).collect();
    let month = |t: &str| {
        MONTHS
            .iter()
            .position(|m| t.eq_ignore_ascii_case(m))
            .map(|i| i as u32 + 1)
    };
    let time = |t: &str| -> Option<(u32, u32, u32)> {
        let mut p = t.split(':');
        let h = p.next()?.parse().ok()?;
        let mi = p.next()?.parse().ok()?;
        let se = p.next()?.parse().ok()?;
        (p.next().is_none() && h < 24 && mi < 60 && se <= 60).then_some((h, mi, se))
    };
    let num = |t: &str| t.parse::<u32>().ok();
    let (day, mon, year, hms) = match tokens.as_slice() {
        // Sun, 06 Nov 1994 08:49:37 GMT  y  Sunday, 06-Nov-94 08:49:37 GMT
        [_, d, m, y, t, gmt] if gmt.eq_ignore_ascii_case("GMT") => {
            let mut year = i64::from(num(y)?);
            if y.len() == 2 {
                // RFC 9110 §5.6.7: un año de dos cifras a más de 50 años en el futuro es del
                // siglo pasado.
                year += if year < 70 { 2000 } else { 1900 };
            }
            (num(d)?, month(m)?, year, time(t)?)
        }
        // Sun Nov  6 08:49:37 1994
        [_, m, d, t, y] => (num(d)?, month(m)?, i64::from(num(y)?), time(t)?),
        _ => return None,
    };
    if !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(year, mon, day);
    Some(days * 86_400 + i64::from(hms.0) * 3600 + i64::from(hms.1) * 60 + i64::from(hms.2))
}

/// Segundos (con fracción) desde 1970.
fn secs(t: SystemTime) -> f64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    }
}

/// RFC 9110 §15.1: los códigos que se pueden guardar con frescura heurística.
fn heuristically_cacheable(status: u16) -> bool {
    matches!(
        status,
        200 | 203 | 204 | 206 | 300 | 301 | 308 | 404 | 405 | 410 | 414 | 501
    )
}

/// RFC 9110 §9.2.1.
fn is_safe(method: &str) -> bool {
    matches!(method, "GET" | "HEAD" | "OPTIONS" | "TRACE")
}

// --- Entradas ---

/// Una respuesta guardada (§3) con lo necesario para calcular su edad (§4.2.3).
#[derive(Debug, Clone, PartialEq)]
struct Entry {
    response: CacheResponse,
    /// Los valores de las cabeceras de la petición que nombra `Vary` (§4.1).
    vary: Vec<(String, Option<String>)>,
    request_time: f64,
    response_time: f64,
    /// Para las respuestas parciales (206): el rango guardado y el largo total.
    range: Option<(u64, u64, Option<u64>)>,
    /// El `Range` de la petición que produjo una respuesta parcial.
    request_range: Option<String>,
}

impl Entry {
    fn directives(&self) -> Directives {
        Directives::of(&self.response.headers)
    }

    fn date(&self) -> f64 {
        self.response
            .headers
            .get("date")
            .and_then(parse_http_date)
            .map_or(self.response_time, |d| d as f64)
    }

    /// §4.2.3: `current_age`.
    fn age(&self, now: f64) -> f64 {
        let age_value = self
            .response
            .headers
            .get("age")
            .and_then(|a| a.trim().parse::<u64>().ok())
            .unwrap_or(0) as f64;
        let apparent_age = (self.response_time - self.date()).max(0.0);
        let response_delay = (self.response_time - self.request_time).max(0.0);
        let corrected_age_value = age_value + response_delay;
        let corrected_initial_age = apparent_age.max(corrected_age_value);
        let resident_time = (now - self.response_time).max(0.0);
        corrected_initial_age + resident_time
    }

    /// §4.2.1 (caché privada: `s-maxage` no cuenta) y §4.2.2 (heurística: el 10 % del
    /// tiempo desde `Last-Modified`).
    fn freshness_lifetime(&self) -> f64 {
        let cc = self.directives();
        if let Some(max_age) = cc.seconds("max-age") {
            return max_age.map_or(0.0, |s| s as f64);
        }
        let mut expires = self.response.headers.get_all("expires");
        if let Some(e) = expires.next() {
            // §5.3: una fecha inválida ("0") es una fecha en el pasado.
            return parse_http_date(e).map_or(0.0, |e| (e as f64 - self.date()).max(0.0));
        }
        if heuristically_cacheable(self.response.status) || cc.has("public") {
            let lm = self
                .response
                .headers
                .get("last-modified")
                .and_then(parse_http_date);
            if let Some(lm) = lm {
                return ((self.date() - lm as f64) / 10.0).max(0.0);
            }
        }
        0.0
    }

    fn validators(&self) -> (Option<&str>, Option<&str>) {
        let h = &self.response.headers;
        (h.get("etag"), h.get("last-modified"))
    }

    /// §4.1: ¿coinciden las cabeceras nombradas por `Vary`?
    fn matches(&self, request: &Headers) -> bool {
        self.vary
            .iter()
            .all(|(name, value)| &vary_value(request, name) == value)
    }
}

/// El valor de una cabecera de la petición para comparar por `Vary` (§4.1: se combinan las
/// líneas y se normaliza el espacio). `Cookie` se normaliza además ordenando sus pares, que
/// §4.1 permite cuando el orden no tiene significado: RFC 6265bis §5.8.3 deja el orden al agente
/// y pide a los servidores no depender de él (§4.2.2). Sin esto, una cookie de sesión que vuelve
/// a llegar en otra sesión cambia el orden y ninguna variante guardada coincide.
fn vary_value(headers: &Headers, name: &str) -> Option<String> {
    let values: Vec<&str> = headers.get_all(name).map(str::trim).collect();
    if values.is_empty() {
        return None;
    }
    if name.eq_ignore_ascii_case("cookie") {
        let mut pairs: Vec<&str> = values
            .iter()
            .flat_map(|v| v.split(';'))
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        pairs.sort_unstable();
        return Some(pairs.join("; "));
    }
    Some(values.join(", "))
}

/// Las cabeceras que nombra el `Vary` de una respuesta; `None` si es `*` (§4.1: nunca
/// coincide).
fn vary_names(headers: &Headers) -> Option<Vec<String>> {
    let names = headers.list("vary");
    if names.iter().any(|n| n == "*") {
        return None;
    }
    Some(names)
}

/// `bytes a-b/len` o `bytes a-b/*` (RFC 9110 §14.4).
fn parse_content_range(v: &str) -> Option<(u64, u64, Option<u64>)> {
    let rest = v.trim().strip_prefix("bytes")?.trim_start();
    let (range, len) = rest.split_once('/')?;
    let (a, b) = range.split_once('-')?;
    let (a, b) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    let len = match len.trim() {
        "*" => None,
        l => Some(l.parse().ok()?),
    };
    (a <= b && len.is_none_or(|l| b < l)).then_some((a, b, len))
}

// --- La caché ---

/// Topes de tamaño (bytes de cuerpo) en memoria y en disco.
const MEMORY_LIMIT: usize = 64 << 20;
const DISK_LIMIT: u64 = 512 << 20;
/// Una respuesta más grande que esto no se guarda.
const ENTRY_LIMIT: usize = 16 << 20;

#[derive(Default)]
struct Memory {
    entries: HashMap<String, (Vec<Entry>, u64)>,
    bytes: usize,
    tick: u64,
}

type Clock = Box<dyn Fn() -> SystemTime + Send + Sync>;

/// La caché HTTP de un [`crate::Fetcher`]. Clonarla da otra referencia a la misma caché (para
/// revalidar en segundo plano).
#[derive(Clone)]
pub struct HttpCache {
    inner: Arc<Inner>,
}

struct Inner {
    memory: Mutex<Memory>,
    /// Carpeta de la caché en disco (`None`: solo memoria).
    dir: Option<PathBuf>,
    clock: Clock,
    /// RFC 5861 §3: revalidaciones por hacer en segundo plano, y las claves en curso.
    revalidations: Mutex<Vec<Revalidation>>,
    in_flight: Mutex<HashSet<String>>,
    /// E6: el hilo que guarda en disco, en orden, las actualizaciones por 304 (§4.3.4) fuera
    /// del camino de la respuesta.
    writer: Mutex<Option<(Sender<Job>, JoinHandle<()>)>>,
}

/// Un trabajo del hilo de escritura.
enum Job {
    Write {
        dir: PathBuf,
        key: String,
        entries: Vec<Entry>,
    },
    /// Avisa cuando todo lo anterior está escrito.
    Flush(Sender<()>),
}

impl Drop for Inner {
    /// Lo pendiente de escribir se escribe antes de soltar la caché.
    fn drop(&mut self) {
        let writer = self
            .writer
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some((tx, handle)) = writer {
            drop(tx);
            let _ = handle.join();
        }
    }
}

/// E6: una respuesta guardada leída del disco para la precarga (HTML §13.2.8, *speculative
/// HTML parsing*) y todavía sin usar: no entra en la caché en memoria hasta
/// [`HttpCache::adopt`], que se llama solo si el documento que la nombra se validó con un 304
/// (RFC 9111 §4.3.4: lo guardado no se usa hasta validarlo).
#[derive(Debug)]
pub struct Staged {
    key: String,
    entries: Vec<Entry>,
    /// ¿Está fresca (§4.2) para una petición sin directivas?
    pub fresh: bool,
}

impl Staged {
    /// El cuerpo de la respuesta más reciente y su `Content-Type`.
    pub fn body(&self) -> Option<(&[u8], Option<&str>)> {
        let e = self
            .entries
            .iter()
            .filter(|e| e.range.is_none())
            .max_by(|a, b| a.response_time.total_cmp(&b.response_time))?;
        Some((&e.response.body, e.response.headers.get("content-type")))
    }
}

/// Una revalidación en segundo plano (RFC 5861 §3): se sirvió una respuesta caducada dentro de
/// su `stale-while-revalidate` y hay que validarla con [`HttpCache::revalidate`].
#[derive(Debug, Clone)]
pub struct Revalidation {
    pub url: Url,
    /// Las cabeceras de la petición que la sirvió (las de `Vary`, las cookies).
    pub headers: Headers,
    pub credentials: bool,
    key: String,
}

impl Default for HttpCache {
    fn default() -> Self {
        HttpCache::in_memory()
    }
}

impl std::fmt::Debug for HttpCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpCache")
            .field("dir", &self.inner.dir)
            .finish()
    }
}

/// Dónde guarda Freedom la caché en disco: `%LOCALAPPDATA%\Freedom\cache` en Windows,
/// `~/Library/Caches/Freedom` en macOS y `$XDG_CACHE_HOME/freedom` (o `~/.cache/freedom`) en
/// el resto.
pub fn default_dir() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        return std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Caches/Freedom"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return Some(PathBuf::from(local).join("Freedom").join("cache"));
    }
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        return Some(PathBuf::from(xdg).join("freedom"));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache").join("freedom"))
}

impl HttpCache {
    pub fn in_memory() -> HttpCache {
        HttpCache::new(None)
    }

    /// Una caché en memoria respaldada por `dir` en disco.
    pub fn on_disk(dir: impl Into<PathBuf>) -> HttpCache {
        HttpCache::new(Some(dir.into()))
    }

    fn new(dir: Option<PathBuf>) -> HttpCache {
        HttpCache {
            inner: Arc::new(Inner {
                memory: Mutex::default(),
                dir,
                clock: Box::new(SystemTime::now),
                revalidations: Mutex::default(),
                in_flight: Mutex::default(),
                writer: Mutex::default(),
            }),
        }
    }

    /// Con otro reloj (para tests). Antes de clonarla: si ya está compartida, no cambia.
    pub fn with_clock(mut self, clock: impl Fn() -> SystemTime + Send + Sync + 'static) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.clock = Box::new(clock);
        }
        self
    }

    pub fn dir(&self) -> Option<&Path> {
        self.inner.dir.as_deref()
    }

    fn now(&self) -> f64 {
        secs((self.inner.clock)())
    }

    /// La clave de una URL: sin fragmento, y separada por credenciales.
    fn key(url: &Url, credentials: bool) -> String {
        let mut u = url.clone();
        u.fragment = None;
        format!("{}{}", if credentials { "c " } else { "a " }, u.href())
    }

    /// Vacía la caché (memoria y disco).
    pub fn clear(&self) {
        *self.inner.memory.lock().unwrap_or_else(|e| e.into_inner()) = Memory::default();
        if let Some(dir) = &self.inner.dir
            && let Ok(list) = std::fs::read_dir(dir)
        {
            for f in list.flatten() {
                let _ = std::fs::remove_file(f.path());
            }
        }
    }

    /// Cuántas URLs hay guardadas en memoria.
    pub fn len(&self) -> usize {
        self.inner
            .memory
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn load(&self, key: &str) -> Vec<Entry> {
        let mut mem = self.inner.memory.lock().unwrap_or_else(|e| e.into_inner());
        mem.tick += 1;
        let tick = mem.tick;
        if let Some((entries, used)) = mem.entries.get_mut(key) {
            *used = tick;
            return entries.clone();
        }
        drop(mem);
        let Some(dir) = &self.inner.dir else {
            return Vec::new();
        };
        let started = std::time::Instant::now();
        let entries = disk::read(&disk::path(dir, key), key).unwrap_or_default();
        use crate::timing::{Disk, disk_add, disk_op};
        disk_op(Disk::Reads, Disk::ReadNanos, started.elapsed());
        let bytes: usize = entries.iter().map(|e| e.response.body.len()).sum();
        disk_add(Disk::ReadBytes, bytes as u64);
        if !entries.is_empty() {
            self.remember(key, entries.clone(), false);
        }
        entries
    }

    /// E6: como [`HttpCache::remember`] con `persist`, pero el disco se escribe en el hilo de
    /// escritura (las actualizaciones por 304: la memoria ya tiene lo nuevo y lo guardado en
    /// disco sigue siendo válido mientras tanto).
    fn remember_later(&self, key: &str, entries: Vec<Entry>) {
        // `BF_NO_PRECARGA=1`: como antes, en el momento (para comparar en el mismo binario).
        if std::env::var_os("BF_NO_PRECARGA").is_some() {
            self.remember(key, entries, true);
            return;
        }
        if let Some(dir) = &self.inner.dir
            && !entries.is_empty()
        {
            let job = Job::Write {
                dir: dir.clone(),
                key: key.to_owned(),
                entries: entries.clone(),
            };
            self.write_job(job);
        }
        self.remember(key, entries, false);
    }

    /// Manda `job` al hilo de escritura (lo crea la primera vez).
    fn write_job(&self, job: Job) {
        let mut writer = self.inner.writer.lock().unwrap_or_else(|e| e.into_inner());
        if writer.is_none() {
            let (tx, rx) = mpsc::channel::<Job>();
            let handle = std::thread::Builder::new()
                .name("bf-net-cache-disco".into())
                .spawn(move || {
                    for job in rx {
                        match job {
                            Job::Write { dir, key, entries } => {
                                let started = std::time::Instant::now();
                                let path = disk::path(&dir, &key);
                                if disk::write(&dir, &path, &key, &entries).is_ok() {
                                    disk::trim(&dir, DISK_LIMIT);
                                }
                                use crate::timing::{Disk, disk_op};
                                disk_op(Disk::BgWrites, Disk::BgWriteNanos, started.elapsed());
                            }
                            Job::Flush(ack) => {
                                let _ = ack.send(());
                            }
                        }
                    }
                });
            match handle {
                Ok(h) => *writer = Some((tx, h)),
                // Sin hilo: se escribe aquí.
                Err(_) => {
                    if let Job::Write { dir, key, entries } = job {
                        let path = disk::path(&dir, &key);
                        let _ = disk::write(&dir, &path, &key, &entries);
                    }
                    return;
                }
            }
        }
        if let Some((tx, _)) = writer.as_ref() {
            let _ = tx.send(job);
        }
    }

    /// Espera a que el hilo de escritura guarde en disco todo lo pendiente.
    pub fn flush(&self) {
        let (ack, done) = mpsc::channel();
        let pending = self
            .inner
            .writer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|(tx, _)| tx.send(Job::Flush(ack)).is_ok());
        if pending {
            let _ = done.recv();
        }
    }

    /// E6: lee del disco lo guardado para `url` sin meterlo en la memoria ni usarlo (ver
    /// [`Staged`]). `None` si ya está en memoria (no hace falta) o no hay nada.
    pub fn stage(&self, url: &Url, credentials: bool) -> Option<Staged> {
        let key = Self::key(url, credentials);
        if self
            .inner
            .memory
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .contains_key(&key)
        {
            return None;
        }
        let dir = self.inner.dir.as_ref()?;
        let started = std::time::Instant::now();
        let entries = disk::read(&disk::path(dir, &key), &key)?;
        use crate::timing::{Disk, disk_op};
        disk_op(Disk::WarmReads, Disk::WarmNanos, started.elapsed());
        let now = self.now();
        let fresh = entries
            .iter()
            .any(|e| e.range.is_none() && usable(e, &Directives::default(), now));
        (!entries.is_empty()).then_some(Staged {
            key,
            entries,
            fresh,
        })
    }

    /// E6: el cuerpo guardado para `url` (el de la respuesta completa más reciente) y su
    /// `Content-Type`, de la memoria o del disco, sin usarlo como respuesta ni cambiar la
    /// caché: para que el escáner de precarga lea el documento mientras se valida.
    pub fn stored_body(&self, url: &Url, credentials: bool) -> Option<(Vec<u8>, Option<String>)> {
        let key = Self::key(url, credentials);
        let from_memory = self
            .inner
            .memory
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .get(&key)
            .map(|(e, _)| e.clone());
        let entries = match from_memory {
            Some(e) => e,
            None => {
                let dir = self.inner.dir.as_ref()?;
                let started = std::time::Instant::now();
                let e = disk::read(&disk::path(dir, &key), &key)?;
                use crate::timing::{Disk, disk_op};
                disk_op(Disk::WarmReads, Disk::WarmNanos, started.elapsed());
                e
            }
        };
        let staged = Staged {
            key,
            entries,
            fresh: false,
        };
        let (body, ct) = staged.body()?;
        Some((body.to_vec(), ct.map(str::to_owned)))
    }

    /// E6: pone en la memoria de la caché lo leído con [`HttpCache::stage`], si la memoria no
    /// tiene ya esa clave (lo que se haya guardado después es más nuevo). Desde ahí se sirve
    /// con las reglas de siempre (§4), como si se hubiera leído del disco en ese momento.
    pub fn adopt(&self, staged: Staged) {
        let present = self
            .inner
            .memory
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .contains_key(&staged.key);
        if !present {
            self.remember(&staged.key, staged.entries, false);
        }
    }

    /// Guarda las entradas de `key` (en memoria y, con `persist`, en disco).
    fn remember(&self, key: &str, entries: Vec<Entry>, persist: bool) {
        if persist && let Some(dir) = &self.inner.dir {
            let started = std::time::Instant::now();
            let path = disk::path(dir, key);
            if entries.is_empty() {
                let _ = std::fs::remove_file(&path);
            } else if disk::write(dir, &path, key, &entries).is_ok() {
                disk::trim(dir, DISK_LIMIT);
            }
            use crate::timing::{Disk, disk_op};
            disk_op(Disk::Writes, Disk::WriteNanos, started.elapsed());
        }
        let mut mem = self.inner.memory.lock().unwrap_or_else(|e| e.into_inner());
        let size = |v: &Vec<Entry>| v.iter().map(|e| e.response.body.len()).sum::<usize>();
        if let Some((old, _)) = mem.entries.remove(key) {
            mem.bytes -= size(&old);
        }
        if entries.is_empty() {
            return;
        }
        mem.tick += 1;
        let tick = mem.tick;
        mem.bytes += size(&entries);
        mem.entries.insert(key.to_owned(), (entries, tick));
        // Fuera de memoria lo usado hace más tiempo (sigue en disco si lo hay).
        while mem.bytes > MEMORY_LIMIT && mem.entries.len() > 1 {
            let oldest = mem
                .entries
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(k, _)| k.clone());
            let Some(k) = oldest else { break };
            if let Some((old, _)) = mem.entries.remove(&k) {
                mem.bytes -= size(&old);
            }
        }
    }

    /// §4.4: quita todo lo guardado para `url` (con y sin credenciales).
    pub fn invalidate(&self, url: &Url) {
        for credentials in [false, true] {
            let key = Self::key(url, credentials);
            let stored = !self.load(&key).is_empty();
            if stored {
                self.remember(&key, Vec::new(), true);
            }
        }
    }

    /// El salto *HTTP-network-or-cache fetch*: sirve de la caché o llama a `network` con las
    /// cabeceras a mandar (las de la petición más las que añaden el modo y la validación).
    /// Si completar una parcial guardada (§3.4) da otro 206 que no se puede combinar, ese 206
    /// es la respuesta: [`HttpCache::fetch_mut`] repite en cambio la petición entera.
    pub fn fetch<E>(
        &self,
        req: &CacheRequest<'_>,
        network: impl FnOnce(&Headers) -> Result<CacheResponse, E>,
    ) -> Result<(CacheResponse, CacheUse), E> {
        let mut network = Some(network);
        self.fetch_inner(req, &mut |h| network.take().map(|n| n(h)))
    }

    /// Como [`HttpCache::fetch`], con una `network` que se puede llamar dos veces.
    pub fn fetch_mut<E>(
        &self,
        req: &CacheRequest<'_>,
        mut network: impl FnMut(&Headers) -> Result<CacheResponse, E>,
    ) -> Result<(CacheResponse, CacheUse), E> {
        self.fetch_inner(req, &mut |h| Some(network(h)))
    }

    /// Las revalidaciones en segundo plano pendientes (RFC 5861 §3); quien las toma las hace
    /// con [`HttpCache::revalidate`].
    pub fn take_revalidations(&self) -> Vec<Revalidation> {
        std::mem::take(
            &mut *self
                .inner
                .revalidations
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        )
    }

    /// Valida (con `no-cache`: §4.3.1) la respuesta de `rv` con `network` y guarda lo que
    /// llegue: un 304 la refresca, un 200 la reemplaza.
    pub fn revalidate<E>(
        &self,
        rv: &Revalidation,
        network: impl FnOnce(&Headers) -> Result<CacheResponse, E>,
    ) -> Result<CacheUse, E> {
        let req = CacheRequest {
            method: "GET",
            url: &rv.url,
            headers: &rv.headers,
            mode: CacheMode::NoCache,
            credentials: rv.credentials,
        };
        let result = self.fetch(&req, network);
        self.inner
            .in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&rv.key);
        result.map(|(_, used)| used)
    }

    /// Deja pendiente la revalidación de `key`, si no hay una en curso.
    fn queue_revalidation(&self, key: &str, req: &CacheRequest<'_>) {
        let new = self
            .inner
            .in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_owned());
        if new {
            self.inner
                .revalidations
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(Revalidation {
                    url: req.url.clone(),
                    headers: req.headers.clone(),
                    credentials: req.credentials,
                    key: key.to_owned(),
                });
        }
    }

    /// `network` devuelve `None` si ya no se puede llamar otra vez.
    #[allow(clippy::type_complexity)]
    fn fetch_inner<E>(
        &self,
        req: &CacheRequest<'_>,
        network: &mut dyn FnMut(&Headers) -> Option<Result<CacheResponse, E>>,
    ) -> Result<(CacheResponse, CacheUse), E> {
        let method = req.method.to_ascii_uppercase();
        let mut headers = req.headers.clone();
        let mut mode = req.mode;
        // Fetch §4.5 paso 8.18: una petición condicional del autor no usa la caché.
        if mode == CacheMode::Default
            && [
                "if-modified-since",
                "if-none-match",
                "if-unmodified-since",
                "if-match",
                "if-range",
            ]
            .iter()
            .any(|h| headers.get(h).is_some())
        {
            mode = CacheMode::NoStore;
        }
        // Fetch §4.5 pasos 8.19–8.20: las cabeceras que anuncian el modo.
        let has_cc = headers.get("cache-control").is_some();
        if mode == CacheMode::NoCache && !has_cc {
            headers.push("Cache-Control", "max-age=0");
        }
        if matches!(mode, CacheMode::NoStore | CacheMode::Reload) {
            if headers.get("pragma").is_none() {
                headers.push("Pragma", "no-cache");
            }
            if !has_cc {
                headers.push("Cache-Control", "no-cache");
            }
        }
        let request_cc = Directives::of(req.headers);
        // §5.4: `Pragma: no-cache` sin `Cache-Control` es un `no-cache`.
        let request_no_cache = request_cc.has("no-cache")
            || (!has_cc && req.headers.list("pragma").iter().any(|p| p == "no-cache"));
        let key = Self::key(req.url, req.credentials);
        let range = (method == "GET")
            .then(|| headers.get("range"))
            .flatten()
            .and_then(crate::parse_range);
        let only_if_cached = mode == CacheMode::OnlyIfCached || request_cc.has("only-if-cached");

        // Lo guardado que sirve para esta petición (§4: misma URL, `Vary`, rango), la más
        // reciente primero.
        let readable = matches!(method.as_str(), "GET" | "HEAD")
            && !matches!(mode, CacheMode::NoStore | CacheMode::Reload)
            && !request_cc.has("no-store");
        let mut validating: Option<Entry> = None;
        // §4.3.1: guardadas que `Vary` no deja elegir (§4.1), validadas para que el servidor
        // elija una con un 304 (p. ej. `Vary: Cookie` cuando cambia alguna cookie de sesión).
        let mut unselected: Vec<Entry> = Vec::new();
        // §3.4: una parcial guardada desde el byte 0 se completa pidiendo lo que falta; si no
        // sale, se repite la petición con `plain_headers`.
        let mut completing: Option<Entry> = None;
        let plain_headers = headers.clone();
        if readable {
            let stored = self.load(&key);
            let now = self.now();
            let mut candidates: Vec<Entry> = stored
                .iter()
                .filter(|e| e.matches(&headers))
                .filter(|e| serve_range(e, range, req.headers.get("range")).is_some())
                .cloned()
                .collect();
            candidates.sort_by(|a, b| b.response_time.total_cmp(&a.response_time));
            if candidates.is_empty()
                && range.is_none()
                && method == "GET"
                && !only_if_cached
                && mode != CacheMode::NoCache
                && !request_no_cache
            {
                completing = stored
                    .iter()
                    .filter(|e| e.matches(&headers) && e.range.is_some_and(|(s, _, _)| s == 0))
                    .filter(|e| usable(e, &request_cc, now))
                    .max_by(|a, b| a.response_time.total_cmp(&b.response_time))
                    .cloned();
                if let Some((_, end, _)) = completing.as_ref().and_then(|e| e.range) {
                    headers.push("Range", &format!("bytes={}-", end + 1));
                    // RFC 9110 §13.1.5: con validador fuerte, el resto solo si no cambió.
                    if let Some(v) = completing
                        .as_ref()
                        .and_then(|e| strong_validator(&e.response.headers))
                    {
                        headers.push("If-Range", &v);
                    }
                }
            }
            if completing.is_none() && candidates.is_empty() && range.is_none() && !only_if_cached {
                unselected = stored
                    .into_iter()
                    .filter(|e| {
                        let (etag, lm) = e.validators();
                        e.range.is_none() && (etag.is_some() || lm.is_some())
                    })
                    .collect();
                unselected.sort_by(|a, b| b.response_time.total_cmp(&a.response_time));
                let mut etags: Vec<&str> = Vec::new();
                for etag in unselected.iter().filter_map(|e| e.validators().0) {
                    if !etags.contains(&etag) {
                        etags.push(etag);
                    }
                }
                if !etags.is_empty() {
                    headers.push("If-None-Match", &etags.join(", "));
                }
                if let Some(lm) = unselected.iter().find_map(|e| e.validators().1) {
                    headers.push("If-Modified-Since", lm);
                }
            }
            if let Some(entry) = candidates.into_iter().next() {
                if matches!(mode, CacheMode::ForceCache | CacheMode::OnlyIfCached) {
                    return Ok((self.serve(&entry, &method, range, req, now), CacheUse::Hit));
                }
                if mode != CacheMode::NoCache
                    && !request_no_cache
                    && usable(&entry, &request_cc, now)
                {
                    return Ok((self.serve(&entry, &method, range, req, now), CacheUse::Hit));
                }
                // RFC 5861 §3: caducada hace poco: se sirve y se valida en segundo plano.
                if mode == CacheMode::Default
                    && !request_no_cache
                    && entry.range.is_none()
                    && stale_while_revalidate(&entry, &request_cc, now)
                {
                    self.queue_revalidation(&key, req);
                    return Ok((self.serve(&entry, &method, range, req, now), CacheUse::Hit));
                }
                if only_if_cached {
                    return Ok((gateway_timeout(), CacheUse::Unavailable));
                }
                // §4.3.1: validación con los validadores guardados (solo respuestas completas).
                let (etag, last_modified) = entry.validators();
                if entry.range.is_none() && (etag.is_some() || last_modified.is_some()) {
                    if let Some(etag) = etag {
                        headers.push("If-None-Match", etag);
                    }
                    if let Some(lm) = last_modified {
                        headers.push("If-Modified-Since", lm);
                    }
                    validating = Some(entry);
                }
            }
        }
        if only_if_cached && validating.is_none() {
            return Ok((gateway_timeout(), CacheUse::Unavailable));
        }

        let mut request_time = self.now();
        let mut response = network(&headers).expect("la primera llamada a la red")?;
        let mut response_time = self.now();

        // §3.4: el resto de una parcial. Un 206 que se combina con ella la completa; si no, se
        // pide la representación entera (si `network` se puede llamar otra vez).
        if let Some(partial) = completing {
            if let Some(full) = self.complete(&key, partial, &response, request_time, response_time)
            {
                let served = self.serve(&full, &method, None, req, response_time);
                return Ok((served, CacheUse::Network));
            }
            if response.status == 206 {
                request_time = self.now();
                if let Some(r) = network(&plain_headers) {
                    response = r?;
                    response_time = self.now();
                }
            }
        }

        // §4.3.3–4.3.4: un 304 a nuestra validación actualiza y sirve lo guardado.
        if response.status == 304
            && let Some(mut entry) = validating
        {
            update_headers(&mut entry.response.headers, &response.headers);
            entry.request_time = request_time;
            entry.response_time = response_time;
            let served = self.serve(&entry, &method, None, req, response_time);
            if self.storable(&method, &entry.response, req, mode, false) {
                self.store_validated(&key, entry);
            }
            return Ok((served, CacheUse::Revalidated));
        }
        if response.status == 304
            && let Some(mut entry) = select_for_update(unselected, &response.headers)
        {
            let old_vary = std::mem::take(&mut entry.vary);
            update_headers(&mut entry.response.headers, &response.headers);
            entry.request_time = request_time;
            entry.response_time = response_time;
            let served = self.serve(&entry, &method, None, req, response_time);
            // La elegida pasa a ser la variante de esta petición (sus cabeceras de `Vary`).
            if let Some(names) = vary_names(&entry.response.headers)
                && self.storable(&method, &entry.response, req, mode, false)
            {
                let mut entries = self.load(&key);
                entries.retain(|e| e.range.is_some() || e.vary != old_vary);
                self.remember(&key, entries, false);
                entry.vary = names
                    .iter()
                    .map(|n| (n.clone(), vary_value(req.headers, n)))
                    .collect();
                // E6: el disco, en el hilo de escritura (una sola vez, con las dos cosas).
                self.store_validated(&key, entry);
            }
            return Ok((served, CacheUse::Revalidated));
        }

        // §4.4: un método inseguro con éxito invalida la URL y las de `Location` y
        // `Content-Location` del mismo origen.
        if !is_safe(&method) && (200..400).contains(&response.status) {
            self.invalidate(req.url);
            for h in ["location", "content-location"] {
                if let Some(v) = response.headers.get(h)
                    && let Ok(u) = Url::parse(v, Some(req.url))
                    && u.origin() == req.url.origin()
                {
                    self.invalidate(&u);
                }
            }
        }

        // §3: guardar. RFC 9110 §9.3.3: la respuesta a un POST con frescura explícita y
        // `Content-Location` igual a la URL vale como la de un GET a esa URL.
        let post_as_get = method == "POST"
            && response
                .headers
                .get("content-location")
                .and_then(|v| Url::parse(v, Some(req.url)).ok())
                .is_some_and(|u| {
                    let mut a = u.clone();
                    let mut b = req.url.clone();
                    a.fragment = None;
                    b.fragment = None;
                    a.href() == b.href()
                });
        if (method == "GET" || post_as_get)
            && self.storable(&method, &response, req, mode, post_as_get)
            && let Some(names) = vary_names(&response.headers)
        {
            let vary = names
                .iter()
                .map(|n| (n.clone(), vary_value(req.headers, n)))
                .collect();
            let range = if response.status == 206 {
                response
                    .headers
                    .get("content-range")
                    .and_then(parse_content_range)
            } else {
                None
            };
            let mut stored = response.clone();
            let range = range.and_then(|r| normalize_partial(r, &mut stored.body));
            if response.status != 206 || range.is_some() {
                self.store(
                    &key,
                    Entry {
                        response: stored,
                        vary,
                        request_time,
                        response_time,
                        range,
                        request_range: req.headers.get("range").map(str::to_owned),
                    },
                );
            }
        }
        Ok((response, CacheUse::Network))
    }

    /// §3: ¿se puede guardar `response`?
    fn storable(
        &self,
        method: &str,
        response: &CacheResponse,
        req: &CacheRequest<'_>,
        mode: CacheMode,
        post_as_get: bool,
    ) -> bool {
        if mode == CacheMode::NoStore || response.body.len() > ENTRY_LIMIT {
            return false;
        }
        let rq = Directives::of(req.headers);
        let rs = Directives::of(&response.headers);
        if rq.has("no-store") || rs.has("no-store") {
            return false;
        }
        // Final y entendida (§3: 206 solo con rango; 304 nunca se guarda tal cual).
        if response.status < 200 || response.status == 304 {
            return false;
        }
        // §3.5: con `Authorization`, solo si la respuesta lo permite explícitamente.
        if req.headers.get("authorization").is_some()
            && !(rs.has("public") || rs.has("must-revalidate") || rs.has("s-maxage"))
        {
            return false;
        }
        let explicit = rs.has("max-age") || response.headers.get("expires").is_some();
        if post_as_get {
            return method == "POST" && explicit;
        }
        explicit
            || rs.has("public")
            || rs.has("private")
            || heuristically_cacheable(response.status)
    }

    /// §3.4: combina `partial` con el 206 `response` a la petición que la completaba. Con la
    /// representación entera, la guarda y la devuelve; si no, guarda lo combinado (si se pudo).
    fn complete(
        &self,
        key: &str,
        partial: Entry,
        response: &CacheResponse,
        request_time: f64,
        response_time: f64,
    ) -> Option<Entry> {
        if response.status != 206 || Directives::of(&response.headers).has("no-store") {
            return None;
        }
        let mut new = Entry {
            response: response.clone(),
            vary: partial.vary.clone(),
            request_time,
            response_time,
            range: None,
            request_range: None,
        };
        new.range = response
            .headers
            .get("content-range")
            .and_then(parse_content_range)
            .and_then(|r| normalize_partial(r, &mut new.response.body));
        if !mergeable(&partial, &new) {
            return None;
        }
        let merged = merge(&partial, &new);
        let complete = merged.range.is_none();
        self.store(key, merged.clone());
        complete.then_some(merged)
    }

    /// Guarda `entry` en lugar de las variantes con los mismos valores de `Vary`. Una parcial
    /// se combina con las guardadas que puede (§3.4); una completa reemplaza a las parciales.
    fn store(&self, key: &str, entry: Entry) {
        let entries = self.merged(key, entry);
        self.remember(key, entries, true);
    }

    /// E6: [`HttpCache::store`] de una respuesta validada con un 304 (§4.3.4): el disco se
    /// pone al día en el hilo de escritura.
    fn store_validated(&self, key: &str, entry: Entry) {
        let entries = self.merged(key, entry);
        self.remember_later(key, entries);
    }

    /// Las entradas de `key` con `entry` agregada (y las que reemplaza, fuera).
    fn merged(&self, key: &str, mut entry: Entry) -> Vec<Entry> {
        let mut entries = self.load(key);
        if entry.range.is_some() {
            while let Some(i) = entries
                .iter()
                .position(|e| e.vary == entry.vary && mergeable(e, &entry))
            {
                let old = entries.remove(i);
                entry = merge(&old, &entry);
                if entry.range.is_none() {
                    break;
                }
            }
        }
        entries.retain(|e| {
            e.vary != entry.vary
                || match (entry.range, e.range) {
                    (None, _) => false,
                    (Some(_), None) => true,
                    (Some(a), Some(b)) => a != b,
                }
        });
        entries.push(entry);
        // Pocas variantes por URL: las más viejas se van.
        if entries.len() > 8 {
            entries.sort_by(|a, b| b.response_time.total_cmp(&a.response_time));
            entries.truncate(8);
        }
        entries
    }

    /// La respuesta a servir desde `entry`: con `Age` (§5.1), sin cuerpo para HEAD y
    /// recortada al rango pedido.
    fn serve(
        &self,
        entry: &Entry,
        method: &str,
        range: Option<(Option<u64>, Option<u64>)>,
        req: &CacheRequest<'_>,
        now: f64,
    ) -> CacheResponse {
        let mut r = serve_range(entry, range, req.headers.get("range"))
            .unwrap_or_else(|| entry.response.clone());
        let age = entry.age(now).floor() as u64;
        r.headers.0.retain(|(n, _)| !n.eq_ignore_ascii_case("age"));
        r.headers.push("Age", &age.to_string());
        if method == "HEAD" {
            r.body.clear();
        }
        r
    }
}

/// La respuesta de `entry` para el rango pedido; `None` si no lo cubre. Sin rango pedido,
/// solo valen las respuestas completas; una parcial con el mismo `Range` se sirve tal cual.
fn serve_range(
    entry: &Entry,
    range: Option<(Option<u64>, Option<u64>)>,
    raw_range: Option<&str>,
) -> Option<CacheResponse> {
    let Some((start, end)) = range else {
        return entry.range.is_none().then(|| entry.response.clone());
    };
    let (have_start, have_end, total) = match entry.range {
        Some(r) => r,
        None => {
            let len = entry.response.body.len() as u64;
            if len == 0 {
                return None;
            }
            (0, len - 1, Some(len))
        }
    };
    // Una parcial pedida con el mismo `Range` que la produjo.
    if entry.range.is_some() && raw_range.is_some() && entry.request_range.as_deref() == raw_range {
        return Some(entry.response.clone());
    }
    let (from, to) = match (start, end) {
        (Some(s), e) => (s, e.unwrap_or(total.map_or(have_end, |t| t - 1))),
        (None, Some(suffix)) => {
            let total = total?;
            (total.saturating_sub(suffix), total - 1)
        }
        (None, None) => return None,
    };
    let to = to.min(total.map_or(have_end, |t| t - 1));
    if from < have_start || to > have_end || from > to {
        return None;
    }
    let offset = (from - have_start) as usize;
    let len = (to - from + 1) as usize;
    let body = entry.response.body.get(offset..offset + len)?.to_vec();
    let mut headers = entry.response.headers.clone();
    headers.0.retain(|(n, _)| {
        !n.eq_ignore_ascii_case("content-range") && !n.eq_ignore_ascii_case("content-length")
    });
    let total = total.map_or("*".to_owned(), |t| t.to_string());
    headers.push("Content-Range", &format!("bytes {from}-{to}/{total}"));
    headers.push("Content-Length", &body.len().to_string());
    Some(CacheResponse {
        status: 206,
        status_text: "Partial Content".into(),
        headers,
        body,
    })
}

/// §4.2 y §5.2.1: ¿se puede servir sin validar?
fn usable(entry: &Entry, request: &Directives, now: f64) -> bool {
    let response = entry.directives();
    if response.has("no-cache") {
        return false;
    }
    let age = entry.age(now);
    let lifetime = entry.freshness_lifetime();
    // RFC 8246 §2: `immutable` fresca no se valida aunque se pida `max-age=0` (una recarga);
    // `no-cache` en la petición (recarga forzada) sí valida.
    let immutable = response.has("immutable") && lifetime > age;
    if let Some(max_age) = request.seconds("max-age")
        && !immutable
    {
        match max_age {
            // `max-age=0`: siempre a validar (la edad se mide en segundos enteros).
            Some(0) | None => return false,
            Some(m) if age > m as f64 => return false,
            _ => {}
        }
    }
    if let Some(min_fresh) = request.seconds("min-fresh")
        && lifetime - age < min_fresh.unwrap_or(0) as f64
    {
        return false;
    }
    if lifetime > age {
        return true;
    }
    // §5.2.1.2: `max-stale` admite una respuesta caducada (no si la respuesta lo prohíbe).
    if response.has("must-revalidate") {
        return false;
    }
    match request.seconds("max-stale") {
        Some(Some(s)) => age - lifetime <= s as f64,
        Some(None) => true,
        None => false,
    }
}

/// §4.3.4: cuál de las respuestas `stored` (la más reciente primero) actualiza un 304 que
/// contesta a sus validadores: la del mismo `ETag` (comparación débil, RFC 9110 §8.8.3.2) o, sin
/// él, la más reciente con el mismo `Last-Modified`. Si el 304 no trae validadores o no coincide
/// con ninguna, la más reciente: solo pudo contestar a los validadores que mandamos, todos
/// sacados de `stored`, y la petición no era condicional, así que un 304 no puede llegar al
/// cliente (lo mismo que hace la validación de la variante elegida).
fn select_for_update(mut stored: Vec<Entry>, fresh: &Headers) -> Option<Entry> {
    let weak = |t: &str| t.trim().trim_start_matches("W/").to_owned();
    let position = if let Some(etag) = fresh.get("etag") {
        stored
            .iter()
            .position(|e| e.validators().0.is_some_and(|t| weak(t) == weak(etag)))
    } else if let Some(lm) = fresh.get("last-modified") {
        stored
            .iter()
            .position(|e| e.validators().1.is_some_and(|l| l.trim() == lm.trim()))
    } else {
        None
    };
    let position = position.unwrap_or(0);
    (position < stored.len()).then(|| stored.swap_remove(position))
}

/// RFC 5861 §3: ¿se puede servir `entry`, caducada, mientras se valida en segundo plano? Si
/// lleva menos de `stale-while-revalidate` segundos caducada, la respuesta no exige validar
/// (`no-cache`, `must-revalidate`: RFC 9111 §5.2.2.2 manda sobre la extensión) y la petición no
/// pide frescura (`max-age`, `min-fresh`).
fn stale_while_revalidate(entry: &Entry, request: &Directives, now: f64) -> bool {
    let response = entry.directives();
    let Some(Some(window)) = response.seconds("stale-while-revalidate") else {
        return false;
    };
    if response.has("no-cache")
        || response.has("must-revalidate")
        || request.has("max-age")
        || request.has("min-fresh")
    {
        return false;
    }
    let stale_for = entry.age(now) - entry.freshness_lifetime();
    stale_for >= 0.0 && stale_for <= window as f64
}

/// RFC 9110 §8.8.1: el validador fuerte de una respuesta: su `ETag` si no es débil o, sin
/// `ETag`, un `Last-Modified` al menos 60 s anterior a su `Date` (§8.8.2.2).
fn strong_validator(h: &Headers) -> Option<String> {
    if let Some(tag) = h.get("etag") {
        let tag = tag.trim();
        return (!tag.starts_with("W/")).then(|| tag.to_owned());
    }
    let lm = h.get("last-modified")?;
    let modified = parse_http_date(lm)?;
    let date = parse_http_date(h.get("date")?)?;
    (date - modified >= 60).then(|| lm.trim().to_owned())
}

/// El rango que de verdad trae un 206 (RFC 9110 §14.4): un cuerpo más largo que su
/// `Content-Range` se recorta; uno más corto cubre menos y, si decía llegar hasta el final, lo
/// que trae es el final. `None` sin cuerpo.
fn normalize_partial(
    (start, end, total): (u64, u64, Option<u64>),
    body: &mut Vec<u8>,
) -> Option<(u64, u64, Option<u64>)> {
    let len = body.len() as u64;
    if len == 0 || end < start {
        return None;
    }
    let declared = end - start + 1;
    if len >= declared {
        body.truncate(declared as usize);
        return Some((start, end, total));
    }
    let real_end = start + len - 1;
    let total = if total == Some(end + 1) {
        Some(real_end + 1)
    } else {
        total
    };
    Some((start, real_end, total))
}

/// §3.4: dos parciales se combinan si comparten un validador fuerte (y el largo total) y sus
/// rangos se tocan.
fn mergeable(old: &Entry, new: &Entry) -> bool {
    let (Some((s1, e1, t1)), Some((s2, e2, t2))) = (old.range, new.range) else {
        return false;
    };
    let validator = strong_validator(&new.response.headers);
    validator.is_some()
        && validator == strong_validator(&old.response.headers)
        && (t1.is_none() || t2.is_none() || t1 == t2)
        && s2 <= e1 + 1
        && s1 <= e2 + 1
}

/// La combinación de dos parciales [`mergeable`]: los bytes de las dos (los de `new` mandan) y
/// las cabeceras de `new` (§3.4, como en un 304: §3.2). Si cubre la representación entera, es
/// un 200 completo.
fn merge(old: &Entry, new: &Entry) -> Entry {
    let ((s1, e1, t1), (s2, e2, t2)) = (
        old.range.unwrap_or((0, 0, None)),
        new.range.unwrap_or((0, 0, None)),
    );
    let (start, end, total) = (s1.min(s2), e1.max(e2), t2.or(t1));
    let mut body = vec![0u8; (end - start + 1) as usize];
    for (part, from) in [(old, s1), (new, s2)] {
        let at = (from - start) as usize;
        let len = part.response.body.len().min(body.len() - at);
        body[at..at + len].copy_from_slice(&part.response.body[..len]);
    }
    let mut headers = new.response.headers.clone();
    headers.0.retain(|(n, _)| {
        !n.eq_ignore_ascii_case("content-range") && !n.eq_ignore_ascii_case("content-length")
    });
    headers.push("Content-Length", &body.len().to_string());
    let complete = start == 0 && total == Some(end + 1);
    let (status, status_text, range) = if complete {
        (200, "OK", None)
    } else {
        let shown = total.map_or("*".to_owned(), |t| t.to_string());
        headers.push("Content-Range", &format!("bytes {start}-{end}/{shown}"));
        (206, "Partial Content", Some((start, end, total)))
    };
    Entry {
        response: CacheResponse {
            status,
            status_text: status_text.into(),
            headers,
            body,
        },
        vary: new.vary.clone(),
        request_time: new.request_time,
        response_time: new.response_time,
        range,
        request_range: None,
    }
}

/// §3.2: un 304 reemplaza las cabeceras guardadas que trae (salvo las de conexión y
/// `Content-Length`).
fn update_headers(stored: &mut Headers, fresh: &Headers) {
    const KEEP: [&str; 7] = [
        "content-length",
        "connection",
        "keep-alive",
        "proxy-connection",
        "transfer-encoding",
        "upgrade",
        "te",
    ];
    let names: Vec<String> = fresh
        .0
        .iter()
        .map(|(n, _)| n.to_ascii_lowercase())
        .filter(|n| !KEEP.contains(&n.as_str()))
        .collect();
    stored
        .0
        .retain(|(n, _)| !names.contains(&n.to_ascii_lowercase()));
    for (n, v) in &fresh.0 {
        if !KEEP.contains(&n.to_ascii_lowercase().as_str()) {
            stored.push(n, v);
        }
    }
}

/// §5.2.1.7: `only-if-cached` sin respuesta guardada.
fn gateway_timeout() -> CacheResponse {
    let mut headers = Headers::default();
    headers.push("Content-Length", "0");
    CacheResponse {
        status: 504,
        status_text: "Gateway Timeout".into(),
        headers,
        body: Vec::new(),
    }
}

/// La caché en disco: un archivo por clave (todas sus variantes), con nombre por hash.
mod disk {
    use super::*;

    /// FNV-1a de 64 bits.
    fn hash(s: &str) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for b in s.bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h
    }

    pub fn path(dir: &Path, key: &str) -> PathBuf {
        dir.join(format!("{:016x}.fc", hash(key)))
    }

    const MAGIC: &str = "FREEDOM-CACHE 1";

    pub fn write(dir: &Path, path: &Path, key: &str, entries: &[Entry]) -> io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let mut out = Vec::new();
        let line = |out: &mut Vec<u8>, s: &str| {
            out.extend_from_slice(s.as_bytes());
            out.push(b'\n');
        };
        line(&mut out, MAGIC);
        line(&mut out, key);
        line(&mut out, &entries.len().to_string());
        for e in entries {
            let r = &e.response;
            line(&mut out, &format!("{} {}", r.status, r.status_text));
            line(&mut out, &format!("{} {}", e.request_time, e.response_time));
            let range = match e.range {
                Some((a, b, Some(t))) => format!("{a} {b} {t}"),
                Some((a, b, None)) => format!("{a} {b} *"),
                None => "-".into(),
            };
            line(&mut out, &range);
            match &e.request_range {
                Some(r) => line(&mut out, &format!("+{r}")),
                None => line(&mut out, "-"),
            }
            line(&mut out, &e.vary.len().to_string());
            for (n, v) in &e.vary {
                line(&mut out, n);
                match v {
                    Some(v) => line(&mut out, &format!("+{v}")),
                    None => line(&mut out, "-"),
                }
            }
            line(&mut out, &r.headers.0.len().to_string());
            for (n, v) in &r.headers.0 {
                line(&mut out, &format!("{n}: {v}"));
            }
            line(&mut out, &r.body.len().to_string());
            out.extend_from_slice(&r.body);
            out.push(b'\n');
        }
        // Escritura atómica: a un temporal y luego se renombra.
        let tmp = path.with_extension("tmp");
        std::fs::File::create(&tmp)?.write_all(&out)?;
        std::fs::rename(&tmp, path)
    }

    /// Lee las entradas de `key`; `None` si el archivo no está o no es válido.
    pub fn read(path: &Path, key: &str) -> Option<Vec<Entry>> {
        let mut data = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .read_to_end(&mut data)
            .ok()?;
        let mut pos = 0usize;
        let next_line = |pos: &mut usize| -> Option<String> {
            let end = data[*pos..].iter().position(|&b| b == b'\n')? + *pos;
            let s = String::from_utf8(data[*pos..end].to_vec()).ok()?;
            *pos = end + 1;
            Some(s)
        };
        if next_line(&mut pos)? != MAGIC || next_line(&mut pos)? != key {
            return None;
        }
        let count: usize = next_line(&mut pos)?.parse().ok()?;
        let mut entries = Vec::new();
        for _ in 0..count {
            let status_line = next_line(&mut pos)?;
            let (status, status_text) = status_line.split_once(' ').unwrap_or((&status_line, ""));
            let times = next_line(&mut pos)?;
            let (a, b) = times.split_once(' ')?;
            let range = match next_line(&mut pos)?.as_str() {
                "-" => None,
                l => {
                    let p: Vec<&str> = l.split(' ').collect();
                    let total = match *p.get(2)? {
                        "*" => None,
                        t => Some(t.parse().ok()?),
                    };
                    Some((p.first()?.parse().ok()?, p.get(1)?.parse().ok()?, total))
                }
            };
            let request_range = next_line(&mut pos)?.strip_prefix('+').map(str::to_owned);
            let nvary: usize = next_line(&mut pos)?.parse().ok()?;
            let mut vary = Vec::new();
            for _ in 0..nvary {
                let n = next_line(&mut pos)?;
                let v = next_line(&mut pos)?;
                vary.push((n, v.strip_prefix('+').map(str::to_owned)));
            }
            let nheaders: usize = next_line(&mut pos)?.parse().ok()?;
            let mut headers = Headers::default();
            for _ in 0..nheaders {
                let h = next_line(&mut pos)?;
                let (n, v) = h.split_once(": ")?;
                headers.push(n, v);
            }
            let len: usize = next_line(&mut pos)?.parse().ok()?;
            let body = data.get(pos..pos + len)?.to_vec();
            pos += len + 1;
            entries.push(Entry {
                response: CacheResponse {
                    status: status.parse().ok()?,
                    status_text: status_text.to_owned(),
                    headers,
                    body,
                },
                vary,
                request_time: a.parse().ok()?,
                response_time: b.parse().ok()?,
                range,
                request_range,
            });
        }
        Some(entries)
    }

    /// Si la carpeta pasa de `limit` bytes, borra los archivos usados hace más tiempo hasta
    /// quedar en el 80 %.
    pub fn trim(dir: &Path, limit: u64) {
        let Ok(list) = std::fs::read_dir(dir) else {
            return;
        };
        let mut files: Vec<(SystemTime, u64, PathBuf)> = list
            .flatten()
            .filter_map(|f| {
                let m = f.metadata().ok()?;
                Some((m.modified().ok()?, m.len(), f.path()))
            })
            .collect();
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        if total <= limit {
            return;
        }
        files.sort_by_key(|f| f.0);
        for (_, len, path) in files {
            if total <= limit / 5 * 4 {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total -= len;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    #[test]
    fn http_dates() {
        let t = 784_111_777;
        assert_eq!(parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT"), Some(t));
        assert_eq!(parse_http_date("Sunday, 06-Nov-94 08:49:37 GMT"), Some(t));
        assert_eq!(parse_http_date("Sun Nov  6 08:49:37 1994"), Some(t));
        assert_eq!(parse_http_date("0"), None);
        assert_eq!(parse_http_date("Sun, 32 Nov 1994 08:49:37 GMT"), None);
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
    }

    #[test]
    fn directives() {
        let mut h = Headers::default();
        h.push(
            "Cache-Control",
            "max-age=\"60\", No-Cache=\"Set-Cookie, X\"",
        );
        h.push("cache-control", "public, max-age=10, max-stale");
        let d = Directives::of(&h);
        assert_eq!(d.seconds("max-age"), Some(Some(60)));
        assert!(d.has("no-cache") && d.has("public"));
        assert_eq!(d.seconds("max-stale"), Some(None));
        assert_eq!(d.seconds("min-fresh"), None);
    }

    /// Una caché con reloj manual y un "servidor" que cuenta peticiones.
    struct Rig {
        cache: HttpCache,
        clock: Arc<AtomicU64>,
        url: Url,
    }

    fn rig(dir: Option<&Path>) -> Rig {
        let clock = Arc::new(AtomicU64::new(1_700_000_000));
        let c = clock.clone();
        let cache = match dir {
            Some(d) => HttpCache::on_disk(d),
            None => HttpCache::in_memory(),
        }
        .with_clock(move || UNIX_EPOCH + Duration::from_secs(c.load(Ordering::SeqCst)));
        Rig {
            cache,
            clock,
            url: Url::parse("https://example.com/a.css#x", None).unwrap(),
        }
    }

    impl Rig {
        fn advance(&self, s: u64) {
            self.clock.fetch_add(s, Ordering::SeqCst);
        }

        fn get(
            &self,
            req_headers: &[(&str, &str)],
            mode: CacheMode,
            respond: &[(&str, &str)],
            status: u16,
        ) -> (CacheResponse, CacheUse, Headers) {
            let body: &[u8] = if status == 304 { b"" } else { b"cuerpo" };
            self.get_body(req_headers, mode, respond, status, body)
        }

        /// Como `get`, con el cuerpo que responde el servidor.
        fn get_body(
            &self,
            req_headers: &[(&str, &str)],
            mode: CacheMode,
            respond: &[(&str, &str)],
            status: u16,
            body: &[u8],
        ) -> (CacheResponse, CacheUse, Headers) {
            let mut h = Headers::default();
            for (n, v) in req_headers {
                h.push(n, v);
            }
            let req = CacheRequest {
                method: "GET",
                url: &self.url,
                headers: &h,
                mode,
                credentials: true,
            };
            let mut sent = Headers::default();
            let (r, u) = self
                .cache
                .fetch(&req, |hh| {
                    sent = hh.clone();
                    let mut headers = Headers::default();
                    for (n, v) in respond {
                        headers.push(n, v);
                    }
                    Ok::<_, ()>(CacheResponse {
                        status,
                        status_text: String::new(),
                        headers,
                        body: body.to_vec(),
                    })
                })
                .unwrap();
            (r, u, sent)
        }
    }

    #[test]
    fn freshness_and_validation() {
        let r = rig(None);
        let d = Default::default();
        let (_, u, _) = r.get(
            &[],
            d,
            &[("Cache-Control", "max-age=60"), ("ETag", "\"v1\"")],
            200,
        );
        assert_eq!(u, CacheUse::Network);
        r.advance(30);
        let (res, u, _) = r.get(&[], d, &[], 500);
        assert_eq!((u, res.status), (CacheUse::Hit, 200));
        assert_eq!(res.headers.get("age"), Some("30"));
        r.advance(31);
        // Caducada: se valida con el ETag y el 304 actualiza las cabeceras.
        let (res, u, sent) = r.get(&[], d, &[("Cache-Control", "max-age=100"), ("X", "2")], 304);
        assert_eq!(sent.get("if-none-match"), Some("\"v1\""));
        assert_eq!(
            (u, res.status, res.headers.get("x")),
            (CacheUse::Revalidated, 200, Some("2"))
        );
        assert_eq!(res.body, b"cuerpo");
        let (_, u, _) = r.get(&[], d, &[], 500);
        assert_eq!(u, CacheUse::Hit);
        // Modos de Fetch.
        let (_, u, sent) = r.get(&[], CacheMode::NoCache, &[], 304);
        assert_eq!(
            (u, sent.get("cache-control")),
            (CacheUse::Revalidated, Some("max-age=0"))
        );
        let (_, u, sent) = r.get(
            &[],
            CacheMode::Reload,
            &[("Cache-Control", "max-age=5")],
            200,
        );
        assert_eq!(
            (u, sent.get("pragma")),
            (CacheUse::Network, Some("no-cache"))
        );
        r.advance(100);
        let (_, u, _) = r.get(&[], CacheMode::ForceCache, &[], 500);
        assert_eq!(u, CacheUse::Hit);
        let (_, u, _) = r.get(&[("Cache-Control", "max-stale=200")], d, &[], 500);
        assert_eq!(u, CacheUse::Hit);
        r.cache.clear();
        let (res, u, _) = r.get(&[], CacheMode::OnlyIfCached, &[], 200);
        assert_eq!((u, res.status), (CacheUse::Unavailable, 504));
    }

    #[test]
    fn not_stored() {
        let r = rig(None);
        let d = Default::default();
        r.get(&[], d, &[("Cache-Control", "max-age=60, no-store")], 200);
        r.get(
            &[("Authorization", "x")],
            d,
            &[("Cache-Control", "max-age=60")],
            200,
        );
        r.get(
            &[],
            d,
            &[("Cache-Control", "max-age=60"), ("Vary", "*")],
            200,
        );
        r.get(&[], d, &[], 299);
        assert!(r.cache.is_empty());
        // Expires inválido: guardada pero caducada.
        r.get(&[], d, &[("Expires", "0")], 200);
        let (_, u, _) = r.get(&[], d, &[], 200);
        assert_eq!(u, CacheUse::Network);
    }

    #[test]
    fn heuristic_and_vary() {
        let r = rig(None);
        let d = Default::default();
        // Last-Modified hace 1000 s: fresca 100 s.
        let lm = "Tue, 14 Nov 2023 21:56:40 GMT";
        assert_eq!(parse_http_date(lm), Some(1_700_000_000 - 1000));
        let date = "Tue, 14 Nov 2023 22:13:20 GMT";
        r.get(
            &[("Accept", "a")],
            d,
            &[("Last-Modified", lm), ("Date", date), ("Vary", "Accept")],
            200,
        );
        r.advance(50);
        assert_eq!(r.get(&[("Accept", "a")], d, &[], 500).1, CacheUse::Hit);
        // Las otras variantes también llevan `Vary: Accept`: una guardada sin `Vary` casaría
        // con cualquier petición (§4.1) y, por más reciente, ganaría a la de `Accept: a`.
        let vary = [("Vary", "Accept")];
        assert_eq!(
            r.get(&[("Accept", "b")], d, &vary, 200).1,
            CacheUse::Network
        );
        assert_eq!(r.get(&[], d, &vary, 200).1, CacheUse::Network);
        r.advance(60);
        let (_, u, sent) = r.get(&[("Accept", "a")], d, &[], 304);
        assert_eq!(
            (u, sent.get("if-modified-since")),
            (CacheUse::Revalidated, Some(lm))
        );
    }

    /// `Vary: Cookie` con los mismos pares en otro orden coincide (§4.1, normalización); con otro
    /// valor, no.
    #[test]
    fn vary_cookie_ignores_pair_order() {
        let r = rig(None);
        let d = Default::default();
        let vary = [
            ("Cache-Control", "max-age=0, must-revalidate"),
            ("Last-Modified", "Tue, 14 Nov 2023 21:56:40 GMT"),
            ("Vary", "Cookie"),
        ];
        r.get(&[("Cookie", "a=1; b=2; GeoIP=x")], d, &vary, 200);
        let (_, u, sent) = r.get(&[("Cookie", "GeoIP=x;b=2; a=1")], d, &[], 304);
        assert_eq!(u, CacheUse::Revalidated);
        assert!(sent.get("if-modified-since").is_some());
        // Otra cookie: ninguna variante coincide; se valida igual (§4.3.1) y un 200 se guarda
        // como variante nueva.
        let (_, u, sent) = r.get(&[("Cookie", "a=1; b=3")], d, &vary, 200);
        assert_eq!(u, CacheUse::Network);
        assert!(sent.get("if-modified-since").is_some());
        assert_eq!(r.cache.load(&HttpCache::key(&r.url, true)).len(), 2);
    }

    /// §4.3.1 y §4.3.4: sin variante que coincida por `Vary`, la petición lleva los validadores
    /// de las guardadas; el 304 elige la del mismo `ETag`, la sirve y pasa a ser la variante de
    /// las cabeceras nuevas (no se acumulan variantes). Es el caso de los gadgets de Wikipedia,
    /// con `Vary: Cookie` y cookies `WMF-Last-Access` que cambian cada día.
    #[test]
    fn vary_mismatch_validates_stored_variants() {
        let r = rig(None);
        let d = Default::default();
        let respond = |etag| {
            [
                ("Cache-Control", "private, max-age=0, must-revalidate"),
                ("ETag", etag),
                ("Vary", "Cookie"),
            ]
        };
        r.get(&[("Cookie", "a=1")], d, &respond("\"v1\""), 200);
        r.advance(1);
        r.get(&[("Cookie", "a=2")], d, &respond("W/\"v2\""), 200);
        r.advance(1);
        let (res, u, sent) = r.get(&[("Cookie", "a=3")], d, &[("ETag", "\"v2\"")], 304);
        assert_eq!(sent.get("if-none-match"), Some("W/\"v2\", \"v1\""));
        assert_eq!((u, res.status), (CacheUse::Revalidated, 200));
        assert_eq!(res.body, b"cuerpo");
        let stored = r.cache.load(&HttpCache::key(&r.url, true));
        assert_eq!(stored.len(), 2);
        // La de `a=2` ahora es la de `a=3`: coincide y se valida sola.
        let (_, u, sent) = r.get(&[("Cookie", "a=3")], d, &[], 304);
        assert_eq!(u, CacheUse::Revalidated);
        assert_eq!(sent.get("if-none-match"), Some("\"v2\""));
        // Solo con `Last-Modified` (como `action=raw` de MediaWiki).
        let r = rig(None);
        let lm = "Tue, 14 Nov 2023 21:56:40 GMT";
        let vary = [
            (
                "Cache-Control",
                "private, s-maxage=0, max-age=0, must-revalidate",
            ),
            ("Last-Modified", lm),
            ("Vary", "Accept-Encoding,Cookie"),
        ];
        r.get(&[("Cookie", "WMF-Last-Access=10-Nov-2026")], d, &vary, 200);
        let (res, u, sent) = r.get(&[("Cookie", "WMF-Last-Access=11-Nov-2026")], d, &[], 304);
        assert_eq!(sent.get("if-modified-since"), Some(lm));
        assert_eq!(
            (u, res.body.as_slice()),
            (CacheUse::Revalidated, &b"cuerpo"[..])
        );
        assert_eq!(r.cache.load(&HttpCache::key(&r.url, true)).len(), 1);
        // `only-if-cached` sin variante que coincida sigue siendo un 504.
        let (res, u, _) = r.get(&[("Cookie", "x=1")], CacheMode::OnlyIfCached, &[], 200);
        assert_eq!((u, res.status), (CacheUse::Unavailable, 504));
    }

    #[test]
    fn ranges() {
        let r = rig(None);
        let d = Default::default();
        let (res, _, _) = r.get(&[], d, &[("Cache-Control", "max-age=60")], 200);
        assert_eq!(res.body, b"cuerpo");
        let (res, u, _) = r.get(&[("Range", "bytes=1-2")], d, &[], 500);
        assert_eq!(
            (u, res.status, &res.body[..]),
            (CacheUse::Hit, 206, &b"ue"[..])
        );
        assert_eq!(res.headers.get("content-range"), Some("bytes 1-2/6"));
        let (res, _, _) = r.get(&[("Range", "bytes=-2")], d, &[], 500);
        assert_eq!(res.body, b"po");
    }

    /// WPT `partial.any.js`: una parcial que dice `4-9/10` pero trae 5 bytes cubre 4–8, y ahí
    /// termina la representación; de ella salen los rangos menores.
    #[test]
    fn partial_responses_serve_smaller_ranges() {
        let r = rig(None);
        let d = Default::default();
        let partial = [
            ("Cache-Control", "max-age=3600"),
            ("Content-Range", "bytes 4-9/10"),
        ];
        r.get_body(&[("Range", "bytes=-5")], d, &partial, 206, b"01234");
        for (range, body) in [
            ("bytes=-5", &b"01234"[..]),
            ("bytes=6-8", b"234"),
            ("bytes=6-", b"234"),
            ("bytes=-1", b"4"),
        ] {
            let (res, u, _) = r.get(&[("Range", range)], d, &[], 500);
            assert_eq!(
                (u, res.status, res.body.as_slice()),
                (CacheUse::Hit, 206, body),
                "{range}"
            );
        }
    }

    /// §3.4: una parcial desde 0 se completa pidiendo el resto (`Range: bytes=5-`); con
    /// validador fuerte va con `If-Range`, y el 206 que llega se combina en un 200 completo.
    #[test]
    fn partial_is_completed() {
        let d = Default::default();
        // Sin validador: el servidor manda la entera (WPT "stores partial content and
        // completes it"), que reemplaza a la parcial.
        let r = rig(None);
        r.get_body(
            &[("Range", "bytes=0-4")],
            d,
            &[
                ("Cache-Control", "max-age=3600"),
                ("Content-Range", "bytes 0-4/10"),
            ],
            206,
            b"01234",
        );
        let full = [("Cache-Control", "max-age=3600")];
        let (res, u, sent) = r.get_body(&[], d, &full, 200, b"0123456789");
        assert_eq!(
            (sent.get("range"), sent.get("if-range")),
            (Some("bytes=5-"), None)
        );
        assert_eq!((u, res.status), (CacheUse::Network, 200));
        assert_eq!(r.get(&[], d, &[], 500).1, CacheUse::Hit);
        assert_eq!(r.cache.load(&HttpCache::key(&r.url, true)).len(), 1);
        // Con ETag: `If-Range`, y el 206 del resto se combina.
        let r = rig(None);
        let tagged = |cr| {
            [
                ("Cache-Control", "max-age=3600"),
                ("ETag", "\"e\""),
                ("Content-Range", cr),
            ]
        };
        r.get_body(
            &[("Range", "bytes=0-4")],
            d,
            &tagged("bytes 0-4/10"),
            206,
            b"01234",
        );
        let (res, u, sent) = r.get_body(&[], d, &tagged("bytes 5-9/10"), 206, b"56789");
        assert_eq!(
            (sent.get("range"), sent.get("if-range")),
            (Some("bytes=5-"), Some("\"e\""))
        );
        assert_eq!(
            (u, res.status, res.body.as_slice()),
            (CacheUse::Network, 200, &b"0123456789"[..])
        );
        assert_eq!(res.headers.get("content-range"), None);
        let (res, u, _) = r.get(&[], d, &[], 500);
        assert_eq!(
            (u, res.body.as_slice()),
            (CacheUse::Hit, &b"0123456789"[..])
        );
    }

    /// Completar una parcial sin validador da otro 206 que no se puede combinar: `fetch_mut`
    /// pide entonces la representación entera.
    #[test]
    fn uncombinable_completion_asks_for_the_whole() {
        let r = rig(None);
        let d = Default::default();
        r.get_body(
            &[("Range", "bytes=0-4")],
            d,
            &[
                ("Cache-Control", "max-age=3600"),
                ("Content-Range", "bytes 0-4/10"),
            ],
            206,
            b"01234",
        );
        let none = Headers::default();
        let req = CacheRequest {
            method: "GET",
            url: &r.url,
            headers: &none,
            mode: d,
            credentials: true,
        };
        let mut calls = Vec::new();
        let (res, u) = r
            .cache
            .fetch_mut(&req, |sent| {
                calls.push(sent.get("range").map(str::to_owned));
                let mut headers = Headers::default();
                let (status, body) = if calls.len() == 1 {
                    headers.push("Content-Range", "bytes 5-9/10");
                    (206, &b"56789"[..])
                } else {
                    (200, &b"0123456789"[..])
                };
                Ok::<_, ()>(CacheResponse {
                    status,
                    status_text: String::new(),
                    headers,
                    body: body.to_vec(),
                })
            })
            .unwrap();
        assert_eq!(calls, [Some("bytes=5-".to_owned()), None]);
        assert_eq!(
            (u, res.status, res.body.as_slice()),
            (CacheUse::Network, 200, &b"0123456789"[..])
        );
    }

    /// §3.4: dos 206 que se tocan y comparten un validador fuerte se guardan como una sola
    /// parcial; si cubren todo, es la respuesta completa. Con validador débil no se combinan.
    #[test]
    fn partials_are_combined() {
        let r = rig(None);
        let d = Default::default();
        let h = |cr| {
            [
                ("Cache-Control", "max-age=3600"),
                ("ETag", "\"e\""),
                ("Content-Range", cr),
            ]
        };
        r.get_body(
            &[("Range", "bytes=0-3")],
            d,
            &h("bytes 0-3/10"),
            206,
            b"0123",
        );
        r.get_body(
            &[("Range", "bytes=4-6")],
            d,
            &h("bytes 4-6/10"),
            206,
            b"456",
        );
        let stored = r.cache.load(&HttpCache::key(&r.url, true));
        assert_eq!((stored.len(), stored[0].range), (1, Some((0, 6, Some(10)))));
        let (res, u, _) = r.get(&[("Range", "bytes=2-5")], d, &[], 500);
        assert_eq!((u, res.body.as_slice()), (CacheUse::Hit, &b"2345"[..]));
        r.get_body(&[("Range", "bytes=7-")], d, &h("bytes 7-9/10"), 206, b"789");
        let (res, u, _) = r.get(&[], d, &[], 500);
        assert_eq!(
            (u, res.status, res.body.as_slice()),
            (CacheUse::Hit, 200, &b"0123456789"[..])
        );
        let r = rig(None);
        let weak = |cr| {
            [
                ("Cache-Control", "max-age=3600"),
                ("ETag", "W/\"e\""),
                ("Content-Range", cr),
            ]
        };
        r.get_body(
            &[("Range", "bytes=0-3")],
            d,
            &weak("bytes 0-3/10"),
            206,
            b"0123",
        );
        r.get_body(
            &[("Range", "bytes=4-6")],
            d,
            &weak("bytes 4-6/10"),
            206,
            b"456",
        );
        assert_eq!(r.cache.load(&HttpCache::key(&r.url, true)).len(), 2);
    }

    /// RFC 8246: fresca e `immutable`, una recarga (`max-age=0`) no la valida; `no-cache` (la
    /// recarga forzada) sí, y caducada se valida como cualquiera.
    #[test]
    fn immutable_is_not_revalidated_on_reload() {
        let r = rig(None);
        let d = Default::default();
        r.get(
            &[],
            d,
            &[
                ("Cache-Control", "max-age=100, immutable"),
                ("ETag", "\"i\""),
            ],
            200,
        );
        let reload = [("Cache-Control", "max-age=0")];
        assert_eq!(r.get(&reload, d, &[], 500).1, CacheUse::Hit);
        let force = [("Cache-Control", "no-cache")];
        assert_eq!(r.get(&force, d, &[], 304).1, CacheUse::Revalidated);
        r.advance(200);
        assert_eq!(r.get(&reload, d, &[], 304).1, CacheUse::Revalidated);
    }

    /// RFC 5861 §3: caducada dentro de la ventana se sirve y deja una revalidación pendiente
    /// (una sola mientras está en curso); el 304 la refresca. Fuera de la ventana, o con
    /// `must-revalidate`, se valida antes de servir.
    #[test]
    fn stale_while_revalidate_queues_a_revalidation() {
        let r = rig(None);
        let d = Default::default();
        r.get(
            &[],
            d,
            &[
                ("Cache-Control", "max-age=10, stale-while-revalidate=30"),
                ("ETag", "\"s\""),
            ],
            200,
        );
        r.advance(20);
        assert_eq!(r.get(&[], d, &[], 500).1, CacheUse::Hit);
        assert_eq!(r.get(&[], d, &[], 500).1, CacheUse::Hit);
        let pending = r.cache.take_revalidations();
        assert_eq!(pending.len(), 1);
        let mut sent = Headers::default();
        let used = r
            .cache
            .revalidate(&pending[0], |h| {
                sent = h.clone();
                Ok::<_, ()>(CacheResponse {
                    status: 304,
                    status_text: String::new(),
                    headers: Headers::default(),
                    body: Vec::new(),
                })
            })
            .unwrap();
        assert_eq!(used, CacheUse::Revalidated);
        assert_eq!(sent.get("if-none-match"), Some("\"s\""));
        assert_eq!(r.get(&[], d, &[], 500).1, CacheUse::Hit);
        assert!(r.cache.take_revalidations().is_empty());
        r.advance(60);
        assert_eq!(r.get(&[], d, &[], 304).1, CacheUse::Revalidated);
        let r = rig(None);
        r.get(
            &[],
            d,
            &[
                (
                    "Cache-Control",
                    "max-age=10, stale-while-revalidate=30, must-revalidate",
                ),
                ("ETag", "\"m\""),
            ],
            200,
        );
        r.advance(20);
        assert_eq!(r.get(&[], d, &[], 304).1, CacheUse::Revalidated);
    }

    /// E6: lo que actualiza un 304 se escribe en disco en el hilo de la caché; `flush` (o
    /// soltar la caché) lo deja escrito.
    #[test]
    fn validated_update_reaches_the_disk() {
        let dir = std::env::temp_dir().join(format!("freedom-cache-304-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let r = rig(Some(&dir));
        let d = CacheMode::Default;
        r.get(
            &[],
            d,
            &[("Cache-Control", "max-age=0"), ("ETag", "\"v\"")],
            200,
        );
        r.advance(5);
        let (_, used, sent) = r.get(&[], d, &[("Cache-Control", "max-age=600")], 304);
        assert_eq!(used, CacheUse::Revalidated);
        assert_eq!(sent.get("if-none-match"), Some("\"v\""));
        r.cache.flush();
        // Otra caché sobre la misma carpeta (otro proceso): fresca por el 304, sin red.
        let r2 = rig(Some(&dir));
        r2.advance(10);
        let (res, used, _) = r2.get(&[], d, &[], 500);
        assert_eq!((used, &res.body[..]), (CacheUse::Hit, &b"cuerpo"[..]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// E6: `stage` lee del disco sin tocar la memoria; `adopt` la pone solo si no hay nada.
    #[test]
    fn stage_and_adopt() {
        let dir = std::env::temp_dir().join(format!("freedom-cache-stage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        {
            let r = rig(Some(&dir));
            r.get(
                &[],
                CacheMode::Default,
                &[("Cache-Control", "max-age=60")],
                200,
            );
        }
        let r = rig(Some(&dir));
        assert!(r.cache.is_empty());
        let staged = r.cache.stage(&r.url, true).expect("en disco");
        assert!(staged.fresh);
        assert_eq!(staged.body().map(|(b, _)| b), Some(&b"cuerpo"[..]));
        assert!(r.cache.is_empty(), "stage no usa ni guarda en memoria");
        assert!(r.cache.stage(&r.url, false).is_none(), "otra clave");
        assert_eq!(
            r.cache.stored_body(&r.url, true).map(|(b, _)| b),
            Some(b"cuerpo".to_vec())
        );
        assert!(r.cache.is_empty());
        r.cache.adopt(staged);
        assert_eq!(r.cache.len(), 1);
        // En memoria: `stage` ya no hace falta.
        assert!(r.cache.stage(&r.url, true).is_none());
        // Caducada: se lee, pero no está fresca.
        r.advance(120);
        let r2 = rig(Some(&dir));
        r2.advance(120);
        assert!(!r2.cache.stage(&r2.url, true).expect("en disco").fresh);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disk_round_trip() {
        let dir = std::env::temp_dir().join(format!("freedom-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        {
            let r = rig(Some(&dir));
            r.get(
                &[("Accept", "x")],
                Default::default(),
                &[("Cache-Control", "max-age=60"), ("Vary", "Accept")],
                200,
            );
        }
        let r = rig(Some(&dir));
        r.advance(10);
        let (res, u, _) = r.get(&[("Accept", "x")], Default::default(), &[], 500);
        assert_eq!((u, &res.body[..]), (CacheUse::Hit, &b"cuerpo"[..]));
        r.cache.invalidate(&r.url);
        let r = rig(Some(&dir));
        assert_eq!(
            r.get(&[("Accept", "x")], Default::default(), &[], 200).1,
            CacheUse::Network
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
