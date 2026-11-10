//! Caché HTTP privada (RFC 9111), en memoria y opcionalmente en disco.
//!
//! [`HttpCache::fetch`] es el paso *HTTP-network-or-cache fetch* de Fetch (§4.5) para un salto:
//! busca una respuesta guardada para la petición (clave por URL y credenciales, variantes por
//! `Vary`, §4.1), la sirve si está fresca (§4.2), la valida con `If-None-Match` /
//! `If-Modified-Since` si caducó (§4.3), y si no va a la red con la función que le pasa quien
//! llama. Después guarda lo que se pueda guardar (§3), actualiza lo validado con un 304
//! (§4.3.4) e invalida tras métodos inseguros (§4.4). También sirve rangos (§3.4) desde una
//! respuesta completa o parcial guardada.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
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

/// La caché HTTP de un [`crate::Fetcher`].
pub struct HttpCache {
    memory: Mutex<Memory>,
    /// Carpeta de la caché en disco (`None`: solo memoria).
    dir: Option<PathBuf>,
    clock: Clock,
}

impl Default for HttpCache {
    fn default() -> Self {
        HttpCache::in_memory()
    }
}

impl std::fmt::Debug for HttpCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpCache").field("dir", &self.dir).finish()
    }
}

/// Dónde guarda Freedom la caché en disco: `%LOCALAPPDATA%\Freedom\cache` en Windows y
/// `$XDG_CACHE_HOME/freedom` (o `~/.cache/freedom`) en el resto.
pub fn default_dir() -> Option<PathBuf> {
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
        HttpCache {
            memory: Mutex::default(),
            dir: None,
            clock: Box::new(SystemTime::now),
        }
    }

    /// Una caché en memoria respaldada por `dir` en disco.
    pub fn on_disk(dir: impl Into<PathBuf>) -> HttpCache {
        HttpCache {
            dir: Some(dir.into()),
            ..HttpCache::in_memory()
        }
    }

    /// Con otro reloj (para tests).
    pub fn with_clock(mut self, clock: impl Fn() -> SystemTime + Send + Sync + 'static) -> Self {
        self.clock = Box::new(clock);
        self
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    fn now(&self) -> f64 {
        secs((self.clock)())
    }

    /// La clave de una URL: sin fragmento, y separada por credenciales.
    fn key(url: &Url, credentials: bool) -> String {
        let mut u = url.clone();
        u.fragment = None;
        format!("{}{}", if credentials { "c " } else { "a " }, u.href())
    }

    /// Vacía la caché (memoria y disco).
    pub fn clear(&self) {
        *self.memory.lock().unwrap_or_else(|e| e.into_inner()) = Memory::default();
        if let Some(dir) = &self.dir
            && let Ok(list) = std::fs::read_dir(dir)
        {
            for f in list.flatten() {
                let _ = std::fs::remove_file(f.path());
            }
        }
    }

    /// Cuántas URLs hay guardadas en memoria.
    pub fn len(&self) -> usize {
        self.memory
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn load(&self, key: &str) -> Vec<Entry> {
        let mut mem = self.memory.lock().unwrap_or_else(|e| e.into_inner());
        mem.tick += 1;
        let tick = mem.tick;
        if let Some((entries, used)) = mem.entries.get_mut(key) {
            *used = tick;
            return entries.clone();
        }
        drop(mem);
        let Some(dir) = &self.dir else {
            return Vec::new();
        };
        let entries = disk::read(&disk::path(dir, key), key).unwrap_or_default();
        if !entries.is_empty() {
            self.remember(key, entries.clone(), false);
        }
        entries
    }

    /// Guarda las entradas de `key` (en memoria y, con `persist`, en disco).
    fn remember(&self, key: &str, entries: Vec<Entry>, persist: bool) {
        if persist && let Some(dir) = &self.dir {
            let path = disk::path(dir, key);
            if entries.is_empty() {
                let _ = std::fs::remove_file(&path);
            } else if disk::write(dir, &path, key, &entries).is_ok() {
                disk::trim(dir, DISK_LIMIT);
            }
        }
        let mut mem = self.memory.lock().unwrap_or_else(|e| e.into_inner());
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
    pub fn fetch<E>(
        &self,
        req: &CacheRequest<'_>,
        network: impl FnOnce(&Headers) -> Result<CacheResponse, E>,
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
        if readable {
            let mut candidates: Vec<Entry> = self
                .load(&key)
                .into_iter()
                .filter(|e| e.matches(&headers))
                .filter(|e| serve_range(e, range, req.headers.get("range")).is_some())
                .collect();
            candidates.sort_by(|a, b| b.response_time.total_cmp(&a.response_time));
            if let Some(entry) = candidates.into_iter().next() {
                let now = self.now();
                if matches!(mode, CacheMode::ForceCache | CacheMode::OnlyIfCached) {
                    return Ok((self.serve(&entry, &method, range, req, now), CacheUse::Hit));
                }
                if mode != CacheMode::NoCache
                    && !request_no_cache
                    && usable(&entry, &request_cc, now)
                {
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

        let request_time = self.now();
        let response = network(&headers)?;
        let response_time = self.now();

        // §4.3.3–4.3.4: un 304 a nuestra validación actualiza y sirve lo guardado.
        if response.status == 304
            && let Some(mut entry) = validating
        {
            update_headers(&mut entry.response.headers, &response.headers);
            entry.request_time = request_time;
            entry.response_time = response_time;
            let served = self.serve(&entry, &method, None, req, response_time);
            if self.storable(&method, &entry.response, req, mode, false) {
                self.store(&key, entry);
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
            if response.status != 206 || range.is_some() {
                self.store(
                    &key,
                    Entry {
                        response: response.clone(),
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

    /// Guarda `entry` en lugar de las variantes con los mismos valores de `Vary`.
    fn store(&self, key: &str, entry: Entry) {
        let mut entries = self.load(key);
        entries.retain(|e| {
            e.vary != entry.vary
                || (e.range.is_some() != entry.range.is_some())
                || (e.range.is_some() && e.range != entry.range)
        });
        entries.push(entry);
        // Pocas variantes por URL: las más viejas se van.
        if entries.len() > 8 {
            entries.sort_by(|a, b| b.response_time.total_cmp(&a.response_time));
            entries.truncate(8);
        }
        self.remember(key, entries, true);
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
    if let Some(max_age) = request.seconds("max-age") {
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
                        body: if status == 304 {
                            vec![]
                        } else {
                            b"cuerpo".to_vec()
                        },
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
        let (_, u, sent) = r.get(&[("Cookie", "a=1; b=3")], d, &vary, 200);
        assert_eq!(u, CacheUse::Network);
        assert!(sent.get("if-modified-since").is_none());
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
