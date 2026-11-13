//! E4: caché de hojas de estilo ya parseadas, por proceso.
//!
//! Parsear una hoja depende solo de sus bytes, del encoding de respaldo (CSS Syntax 3 §3.2:
//! BOM, `@charset` o el del documento) y de su URL (las `url()` se resuelven contra ella).
//! El resultado no lleva estado del documento: es un `Stylesheet` (`Send + Sync`) que se puede
//! parsear en otro hilo, por ejemplo durante la precarga mientras se valida el documento
//! ([`warm`]), y compartir. Los `@import` no se resuelven aquí sino al usar la hoja
//! (`loader::load_sheet`), porque dependen del cargador del documento.
//!
//! La clave es la URL final de la hoja y un hash de sus bytes: el cargador de recursos no
//! expone el `ETag` ni el `Last-Modified`, y el hash del cuerpo es el validador que queda
//! (dos respuestas con los mismos bytes dan la misma hoja). Quien precarga y quien carga
//! calculan la misma clave con [`SheetKey::new`].

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use bf_css::stylesheet::{Origin, Stylesheet};
use bf_infra::Url;
use bf_infra::encoding::Encoding;

/// Hojas guardadas como máximo (las más viejas salen primero).
const CAPACITY: usize = 256;

/// La identidad de una hoja: URL final y hash (FNV-1a de 64 bits) y largo de sus bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SheetKey {
    pub url: String,
    pub body: u64,
    pub len: usize,
}

impl SheetKey {
    pub fn new(url: &Url, bytes: &[u8]) -> SheetKey {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        SheetKey {
            url: url.serialize(true),
            body: h,
            len: bytes.len(),
        }
    }
}

/// Lo del documento que cambia el resultado del parseo: el encoding de respaldo (CSS Syntax
/// §3.2) y el modo quirks (hoy no cambia el parseo de `bf-css`, que deja las quirks de
/// valores para la cascada, pero va en la clave para que una hoja de un documento quirks
/// nunca se use en uno que no lo es si eso cambia).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SheetContext {
    pub quirks: bool,
    pub encoding: Encoding,
}

impl std::hash::Hash for SheetContext {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.quirks.hash(state);
        self.encoding.name().hash(state);
    }
}

struct Cache {
    sheets: HashMap<(SheetKey, SheetContext), Arc<Stylesheet>>,
    order: VecDeque<(SheetKey, SheetContext)>,
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(Cache {
            sheets: HashMap::new(),
            order: VecDeque::new(),
        })
    })
}

/// Cuántas hojas salieron de la caché y cuántas se parsearon, y el tiempo de parsearlas.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct SheetStats {
    pub hits: u32,
    pub parsed: u32,
    pub parse_time: Duration,
}

fn stats() -> &'static Mutex<SheetStats> {
    static STATS: OnceLock<Mutex<SheetStats>> = OnceLock::new();
    STATS.get_or_init(|| Mutex::new(SheetStats::default()))
}

/// Lo acumulado desde la última llamada (para la traza `perf hojas`).
pub fn take_stats() -> SheetStats {
    std::mem::take(
        &mut *stats()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// La hoja de `key` en el contexto `ctx`, si ya está parseada.
pub fn get(key: &SheetKey, ctx: &SheetContext) -> Option<Arc<Stylesheet>> {
    let found = cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .sheets
        .get(&(key.clone(), *ctx))
        .cloned();
    if found.is_some() {
        stats()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .hits += 1;
    }
    found
}

/// La hoja de los bytes `bytes` servidos en `url`: de la caché, o parseada (y guardada).
/// Se puede llamar desde cualquier hilo.
pub fn warm(url: &Url, bytes: &[u8], ctx: &SheetContext) -> Arc<Stylesheet> {
    let key = SheetKey::new(url, bytes);
    if let Some(sheet) = get(&key, ctx) {
        return sheet;
    }
    let start = Instant::now();
    let sheet = Arc::new(parse(url, bytes, ctx));
    {
        let mut s = stats()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        s.parsed += 1;
        s.parse_time += start.elapsed();
    }
    let mut c = cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let entry = (key, *ctx);
    if !c.sheets.contains_key(&entry) {
        if c.order.len() >= CAPACITY
            && let Some(old) = c.order.pop_front()
        {
            c.sheets.remove(&old);
        }
        c.order.push_back(entry.clone());
        c.sheets.insert(entry, sheet.clone());
    }
    sheet
}

/// Decodifica, parsea y resuelve las `url()` de la hoja contra `url` (sin los `@import`).
fn parse(url: &Url, bytes: &[u8], ctx: &SheetContext) -> Stylesheet {
    let mut sheet = Stylesheet::parse(
        &crate::loader::decode_css(bytes, ctx.encoding),
        Origin::Author,
    );
    sheet.rewrite_urls(&mut |u| Url::parse(u, Some(url)).ok().map(|u| u.href()));
    sheet
}
