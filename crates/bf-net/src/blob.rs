//! El almacén de URLs `blob:` (File API §8, "blob URL store") del proceso: lo llenan
//! `URL.createObjectURL()` y lo consultan el `Fetcher` y los cargadores de recursos. (E2)

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use bf_infra::Url;

/// Una entrada del almacén: los bytes del `Blob`, su tipo y el origen que la creó.
#[derive(Debug, Clone)]
pub struct BlobEntry {
    pub bytes: Arc<Vec<u8>>,
    pub mime: String,
    pub origin: String,
}

fn store() -> &'static Mutex<HashMap<String, BlobEntry>> {
    static STORE: OnceLock<Mutex<HashMap<String, BlobEntry>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Un UUID v4 (RFC 4122) con la entropía de las claves de `RandomState`.
fn uuid() -> String {
    let mut bytes = [0u8; 16];
    for chunk in bytes.chunks_mut(8) {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(0x9e37_79b9_7f4a_7c15);
        chunk.copy_from_slice(&h.finish().to_le_bytes());
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let h: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        h[0..4].concat(),
        h[4..6].concat(),
        h[6..8].concat(),
        h[8..10].concat(),
        h[10..16].concat()
    )
}

/// "Generate a new blob URL" y lo agrega al almacén (File API §8.3).
pub fn register(origin: &str, bytes: Arc<Vec<u8>>, mime: &str) -> String {
    let serialized = if origin.is_empty() || origin == "null" {
        "null".to_owned()
    } else {
        origin.to_owned()
    };
    let url = format!("blob:{serialized}/{}", uuid());
    store().lock().unwrap_or_else(|e| e.into_inner()).insert(
        url.clone(),
        BlobEntry {
            bytes,
            mime: mime.to_owned(),
            origin: origin.to_owned(),
        },
    );
    url
}

/// "Remove an entry from the blob URL store".
pub fn revoke(url: &str) {
    store()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(url);
}

/// "Resolve a blob URL" (sin el fragmento).
pub fn resolve(url: &Url) -> Option<BlobEntry> {
    let mut u = url.clone();
    u.fragment = None;
    store()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&u.href())
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_resolve() {
        let url = register("https://a.test", Arc::new(b"hola".to_vec()), "text/plain");
        assert!(url.starts_with("blob:https://a.test/"));
        assert_eq!(url.len(), "blob:https://a.test/".len() + 36);
        let parsed = Url::parse(&format!("{url}#frag"), None).expect("url");
        let e = resolve(&parsed).expect("entrada");
        assert_eq!(e.bytes.as_slice(), b"hola");
        revoke(&url);
        assert!(resolve(&parsed).is_none());
    }
}
