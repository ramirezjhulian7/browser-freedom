//! Decodificación compartida: la misma imagen se decodifica una sola vez en el proceso.
//!
//! En la ventana, cada imagen del documento la piden dos crates: `bf-paint`, para pintarla, y
//! `bf-web`, para sus eventos `load`/`error`, `naturalWidth` y `decode()` (HTML §4.8.4.3.5).
//! Los dos llaman a [`decode_frames_shared`] con los bytes que descargaron, y el primero que
//! llega decodifica; el otro recibe el mismo resultado. La clave es el contenido (longitud y
//! hash de los bytes), no la URL: no acopla los crates a la carga del otro, sirve igual para
//! dos URLs con los mismos bytes y no confunde una URL cuyo contenido cambió.
//!
//! La caché guarda también los errores (un formato sin soporte o un archivo roto no se vuelve
//! a intentar) y tiene un tope de memoria: al pasarlo se olvidan las entradas más viejas.

use std::collections::{HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::{ImageError, ImageFrames, decode_frames};

/// Bytes de píxeles decodificados que guarda la caché como máximo.
const MAX_BYTES: usize = 128 << 20;

/// Decodificaciones de verdad en el proceso ([`crate::decode`], [`crate::decode_frames`] y las
/// que no salieron de la caché), para medir y para tests.
static DECODES: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn count_decode() {
    DECODES.fetch_add(1, Ordering::Relaxed);
}

type Key = (usize, u64);
type Entry = Result<Arc<ImageFrames>, ImageError>;

#[derive(Default)]
struct Cache {
    map: HashMap<Key, Entry>,
    /// Orden de llegada, para olvidar las más viejas.
    order: VecDeque<Key>,
    bytes: usize,
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

fn key(bytes: &[u8], mime_hint: Option<&str>) -> Key {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    // El tipo cuenta (un SVG o un formato desconocido se decide con él).
    mime_hint.hash(&mut h);
    (bytes.len(), h.finish())
}

fn pixel_bytes(e: &Entry) -> usize {
    e.as_ref()
        .map_or(0, |f| f.frames.iter().map(|f| f.image.rgba.len()).sum())
}

/// [`decode_frames`] con la caché del proceso: la misma imagen (mismos bytes y tipo) se
/// decodifica una vez y los siguientes reciben el mismo [`ImageFrames`].
pub fn decode_frames_shared(
    bytes: &[u8],
    mime_hint: Option<&str>,
) -> Result<Arc<ImageFrames>, ImageError> {
    let k = key(bytes, mime_hint);
    if let Some(hit) = cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .map
        .get(&k)
    {
        return hit.clone();
    }
    // Sin el candado: dos hilos con la misma imagen a la vez pueden decodificarla los dos (la
    // segunda entrada pisa a la primera); el caso normal, uno tras otro, decodifica una vez.
    let entry: Entry = decode_frames(bytes, mime_hint).map(Arc::new);
    let size = pixel_bytes(&entry);
    if size <= MAX_BYTES {
        let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = c.map.insert(k, entry.clone()) {
            c.bytes -= pixel_bytes(&old);
        } else {
            c.order.push_back(k);
        }
        c.bytes += size;
        while c.bytes > MAX_BYTES {
            let Some(oldest) = c.order.pop_front() else {
                break;
            };
            if let Some(gone) = c.map.remove(&oldest) {
                c.bytes -= pixel_bytes(&gone);
            }
        }
    }
    entry
}

/// Cuántas imágenes se decodificaron en el proceso (con [`crate::decode`],
/// [`crate::decode_frames`] o [`decode_frames_shared`] sin acierto en la caché).
pub fn decode_count() -> usize {
    DECODES.load(Ordering::Relaxed)
}
