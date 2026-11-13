//! Cache API (Service Workers, https://w3c.github.io/ServiceWorker/): `Cache` (§5.4),
//! `CacheStorage` (§5.5) y el atributo `caches` de `WindowOrWorkerGlobalScope` (§5.3.1).
//! **Dueño: E3.**
//!
//! Las interfaces están en `cache.js` (evaluado una vez por realm, como IndexedDB); aquí están
//! las funciones nativas que usa y el almacén.
//!
//! # El almacén: global al proceso
//!
//! Las páginas y los service workers son agentes en hilos distintos y tienen que ver las
//! mismas cachés, así que el "name to cache map" (§2.6 "relevant name to cache map") no vive
//! en ningún agente: está en un `static` con un `Mutex`, indexado por [`StorageKey`]
//! (partición del proceso, origen). Cada operación (incluido todo un "Batch Cache
//! Operations") corre con el candado tomado, así que es atómica para los demás agentes.
//!
//! Una caché se identifica por un número; el mapa de nombres apunta a esos números. `delete()`
//! quita el nombre pero conserva los datos en memoria (§5.5.4 nota: los objetos `Cache` que ya
//! la referencian siguen funcionando); esos datos huérfanos no se guardan en disco y se
//! liberan al terminar el proceso.
//!
//! # En disco (formato propio)
//!
//! Solo para la partición 0 (la de las pestañas del shell: las demás son números de un
//! proceso, sin sentido entre ejecuciones) y si el realm tiene directorio de almacenamiento
//! ([`crate::storage::root_dir`]): `<root>/<origen>/caches/` con
//!
//! - `index`: texto, `bf-caches 1` y una línea `<id> <nombre en UTF-8 hexadecimal>` por caché
//!   en orden de creación;
//! - `<id>.meta`: binario (`BFC1`, número de entradas y por entrada la petición y la
//!   respuesta, sin el cuerpo);
//! - `<id>-<n>.body`: el cuerpo de una respuesta (se escribe una vez; los que ya no se usan se
//!   borran al guardar la caché).
//!
//! Toda escritura es atómica (archivo temporal + `rename`). Se carga entero, de forma
//! perezosa, la primera vez que algún agente del proceso usa ese [`StorageKey`].

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bf_infra::Url;
use bf_js::host::TaskSource;
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::{CallArgs, NativeFn};
use bf_js::vm::{Agent, JsResult, RealmId};

use crate::WebContextHandle;
use crate::net::body::Body;
use crate::net::fetch::{RequestData, ResponseData};
use crate::net::headers::{self, Guard, HeaderList};

const CACHE_JS: &str = include_str!("cache.js");

/// La clave del almacén: (partición del proceso, origen serializado).
pub type StorageKey = (u64, String);

/// Una petición guardada (Fetch §2.2.5, lo que se conserva de ella).
#[derive(Debug, Clone)]
struct StoredRequest {
    method: String,
    url: String,
    headers: HeaderList,
    mode: String,
    credentials: String,
    cache: String,
    redirect: String,
    referrer: String,
    referrer_policy: String,
    integrity: String,
}

/// Una respuesta guardada (Fetch §2.2.6), con el cuerpo completo.
#[derive(Debug, Clone)]
struct StoredResponse {
    type_: String,
    url_list: Vec<String>,
    redirected: bool,
    status: u16,
    status_text: String,
    headers: HeaderList,
    body: Option<Arc<Vec<u8>>>,
    /// El archivo `<id>-<n>.body` del cuerpo (0: todavía no está en disco).
    body_file: u64,
}

/// Un elemento de la "request response list" (§2.6).
#[derive(Debug, Clone)]
struct Entry {
    req: StoredRequest,
    res: StoredResponse,
}

/// El "name to cache map" de un [`StorageKey`] y las cachés a las que apunta.
#[derive(Debug, Default)]
struct CacheStorageData {
    /// Directorio en disco (`None`: solo en memoria).
    dir: Option<PathBuf>,
    /// Nombre → id, en orden de creación (`keys()`).
    names: Vec<(String, u64)>,
    /// Las cachés (también las huérfanas de un `delete()`).
    caches: HashMap<u64, Vec<Entry>>,
    next_id: u64,
    next_body: u64,
}

static STORE: Mutex<BTreeMap<StorageKey, CacheStorageData>> = Mutex::new(BTreeMap::new());

/// `CacheQueryOptions` (§5.4).
#[derive(Debug, Clone, Copy, Default)]
struct QueryOptions {
    ignore_search: bool,
    ignore_method: bool,
    ignore_vary: bool,
}

/// Instala `Cache`, `CacheStorage` y `caches` en el global de `realm` (una ventana o un
/// worker), solo si es un contexto seguro (§5.3.1 y §5.4/§5.5: `[SecureContext]`).
pub fn install(agent: &mut Agent, realm: RealmId) {
    let url = WebContextHandle { realm }.get(agent).url.clone();
    if !crate::workers::isolation::is_potentially_trustworthy(&url) {
        return;
    }
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let native = agent.new_plain_object();
    // E1: fijado mientras dura: crear los métodos y compilar `cache.js` pueden recolectar, y
    // nada más lo enraíza (el mismo fallo "Gc liberado" que en `storage/idb.rs`).
    let native_pin = agent.host_pin(Value::Object(native));
    let fns: [(&str, u32, NativeFn); 14] = [
        ("queueTask", 1, queue_task),
        ("opaque", 0, opaque),
        ("isRequest", 1, is_request),
        ("isResponse", 1, is_response),
        ("method", 1, method),
        ("check", 3, check),
        ("names", 0, names),
        ("entries", 0, entries),
        ("lookup", 1, lookup),
        ("open", 1, open),
        ("remove", 1, remove),
        ("matchAll", 5, match_all),
        ("keys", 5, keys),
        ("batch", 2, batch),
    ];
    for (name, len, f) in fns {
        bf_js::builtins::method(agent, native, name, len, f);
    }
    let g = Value::Object(agent.realms[realm].global_object);
    if let Ok(f) = agent.eval_script(CACHE_JS, "bf-web:cache.js") {
        let _ = agent.call(&f, Value::Undefined, &[g, Value::Object(native)]);
    }
    agent.host_unpin(native_pin);
    agent.current_realm = saved;
}

fn lock() -> std::sync::MutexGuard<'static, BTreeMap<StorageKey, CacheStorageData>> {
    STORE.lock().unwrap_or_else(|e| e.into_inner())
}

/// El [`StorageKey`] del realm en curso.
fn storage_key(agent: &mut Agent) -> StorageKey {
    (
        crate::workers::partition(agent),
        crate::net::document_origin(agent),
    )
}

/// Corre `f` con el almacén del realm en curso (lo carga del disco la primera vez).
fn with_store<R>(agent: &mut Agent, f: impl FnOnce(&mut CacheStorageData) -> R) -> R {
    let key = storage_key(agent);
    let mut store = lock();
    if !store.contains_key(&key) {
        let realm = WebContextHandle::current(agent).realm;
        let dir = if key.0 == 0 {
            crate::storage::root_dir(agent, realm).map(|r| {
                r.join(crate::storage::backend::origin_dir_name(&key.1))
                    .join("caches")
            })
        } else {
            None
        };
        store.insert(key.clone(), CacheStorageData::load(dir));
    }
    f(store.get_mut(&key).expect("cargado"))
}

// --- Funciones nativas ---

fn str_arg(agent: &mut Agent, v: &Value) -> JsResult<String> {
    Ok(agent.to_string(v)?.to_string())
}

fn id_arg(v: &Value) -> u64 {
    v.as_number().unwrap_or(0.0) as u64
}

fn flag(v: &Value) -> bool {
    matches!(v, Value::Boolean(true))
}

/// `queueTask(f)`: una tarea (los pasos "queue a task" tras los "in parallel" de §5.4/§5.5).
fn queue_task(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    agent.queue_callback_task(TaskSource::Other, a.arg(0), Value::Undefined, Vec::new());
    Ok(Value::Undefined)
}

/// ¿El origen es opaco? (`SecurityError` en `CacheStorage`.)
fn opaque(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(crate::net::document_origin(agent) == "null"))
}

fn is_request(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(
        agent.host_unwrap::<RequestData>(&a.arg(0)).is_some(),
    ))
}

fn is_response(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(
        agent.host_unwrap::<ResponseData>(&a.arg(0)).is_some(),
    ))
}

/// El método de un `Request` (sin pasar por getters que el autor pueda cambiar).
fn method(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(agent
        .host_unwrap::<RequestData>(&a.arg(0))
        .map_or(Value::Null, |r| Value::str(&r.method)))
}

/// La lista de cabeceras del objeto `Headers` del slot 0 de un `Request`/`Response`.
fn header_list(agent: &mut Agent, v: &Value) -> HeaderList {
    let Value::Object(o) = v else {
        return HeaderList::default();
    };
    let h = agent
        .host_slots_mut(*o)
        .and_then(|s| s.first().cloned())
        .unwrap_or(Value::Null);
    headers::data(agent, &h).map(|d| d.list).unwrap_or_default()
}

fn stored_request(agent: &mut Agent, v: &Value) -> Option<StoredRequest> {
    let d = agent.host_unwrap::<RequestData>(v)?.clone();
    let headers = header_list(agent, v);
    Some(StoredRequest {
        method: d.method,
        url: d.url.href(),
        headers,
        mode: d.mode,
        credentials: d.credentials,
        cache: d.cache,
        redirect: d.redirect,
        referrer: d.referrer,
        referrer_policy: d.referrer_policy,
        integrity: d.integrity,
    })
}

fn stored_response(agent: &mut Agent, v: &Value, body: &Value) -> Option<StoredResponse> {
    let d = agent.host_unwrap::<ResponseData>(v)?.clone();
    let headers = header_list(agent, v);
    let body = if body.is_null() || body.is_undefined() {
        None
    } else {
        Some(Arc::new(
            bf_js::builtins::web::encoding::buffer_source_bytes(agent, body).unwrap_or_default(),
        ))
    };
    Some(StoredResponse {
        type_: d.type_,
        url_list: d.url.iter().map(Url::href).collect(),
        redirected: d.redirected,
        status: d.status,
        status_text: d.status_text,
        headers,
        body,
        body_file: 0,
    })
}

/// Los valores de `Vary` ("getting, decoding, and splitting", Fetch §2.2.2).
fn vary_fields(list: &HeaderList) -> Vec<String> {
    list.get("vary")
        .map(|v| {
            v.split(',')
                .map(|s| s.trim_matches([' ', '\t']).to_owned())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn is_http(url: &str) -> bool {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// `check(kind, request, response)`: el mensaje del `TypeError` o `null`.
///
/// - `0`: la petición de `put()`/`addAll()` (§5.4.4 paso 3, §5.4.5 paso 3): esquema
///   `http(s)` y método `GET`.
/// - `1`: la respuesta de `put()` (§5.4.5 pasos 5–6): no `206` ni `Vary: *`.
/// - `2`: la respuesta de un fetch de `addAll()` (§5.4.4 "processResponse"): no de tipo
///   `error`, con estado ok, no `206` ni `Vary: *`.
fn check(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let kind = a.arg(0).as_number().unwrap_or(0.0) as u8;
    let msg = if kind == 0 {
        match agent.host_unwrap::<RequestData>(&a.arg(1)) {
            None => Some("Request expected"),
            Some(r) if r.url.scheme != "http" && r.url.scheme != "https" => {
                Some("Request scheme must be 'http' or 'https'")
            }
            Some(r) if r.method != "GET" => Some("Request method must be 'GET'"),
            Some(_) => None,
        }
    } else {
        let resp = a.arg(2);
        match agent.host_unwrap::<ResponseData>(&resp).cloned() {
            None => Some("Response expected"),
            Some(r) if kind == 2 && r.type_ == "error" => Some("Network error"),
            Some(r) if kind == 2 && !(200..=299).contains(&r.status) => {
                Some("Request failed: the response status is not ok")
            }
            Some(r) if r.status == 206 => Some("Partial response (status code 206) is unsupported"),
            Some(_)
                if vary_fields(&header_list(agent, &resp))
                    .iter()
                    .any(|f| f == "*") =>
            {
                Some("Vary header contains *")
            }
            Some(_) => None,
        }
    };
    Ok(msg.map_or(Value::Null, Value::str))
}

/// `names()`: los nombres de las cachés en orden de creación (§5.5.5 `keys()`).
fn names(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let list = with_store(agent, |s| {
        s.names
            .iter()
            .map(|(n, _)| Value::str(n))
            .collect::<Vec<_>>()
    });
    Ok(Value::Object(agent.create_array_from_list(&list)?))
}

/// `entries()`: `[[nombre, id], ...]` (para `CacheStorage.match()` sin `cacheName`).
fn entries(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let list = with_store(agent, |s| s.names.clone());
    let mut out = Vec::with_capacity(list.len());
    for (n, id) in list {
        let pair = agent.create_array_from_list(&[Value::str(&n), Value::Number(id as f64)])?;
        out.push(Value::Object(pair));
    }
    Ok(Value::Object(agent.create_array_from_list(&out)?))
}

/// `lookup(name)`: el id de la caché `name`, o `-1` (§5.5.2 `has()`).
fn lookup(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = str_arg(agent, &a.arg(0))?;
    let id = with_store(agent, |s| s.id_of(&name));
    Ok(Value::Number(id.map_or(-1.0, |i| i as f64)))
}

/// `open(name)`: el id de la caché `name`, que se crea si no existe (§5.5.3).
fn open(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = str_arg(agent, &a.arg(0))?;
    let id = with_store(agent, |s| {
        if let Some(id) = s.id_of(&name) {
            return id;
        }
        let id = s.next_id;
        s.next_id += 1;
        s.names.push((name, id));
        s.caches.insert(id, Vec::new());
        s.save_index();
        id
    });
    Ok(Value::Number(id as f64))
}

/// `remove(name)`: quita `name` del mapa (§5.5.4); `true` si existía. Los datos quedan
/// en memoria para los objetos `Cache` que ya la tienen.
fn remove(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = str_arg(agent, &a.arg(0))?;
    let found = with_store(agent, |s| {
        let Some(i) = s.names.iter().position(|(n, _)| *n == name) else {
            return false;
        };
        let (_, id) = s.names.remove(i);
        s.save_index();
        s.remove_files(id);
        true
    });
    Ok(Value::Boolean(found))
}

/// `matchAll(id, request | null, ignoreSearch, ignoreMethod, ignoreVary)`: `Response`s
/// nuevos (guard "immutable") de las entradas que coinciden (§5.4.2), o de todas.
fn match_all(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let found = query_args(agent, a)?;
    let mut out = Vec::with_capacity(found.len());
    for e in &found {
        out.push(response_object(agent, &e.res)?);
    }
    Ok(Value::Object(agent.create_array_from_list(&out)?))
}

/// `keys(id, request | null, ignoreSearch, ignoreMethod, ignoreVary)`: `Request`s nuevos
/// (guard "immutable") de las entradas que coinciden (§5.4.7), o de todas, en orden.
fn keys(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let found = query_args(agent, a)?;
    let mut out = Vec::with_capacity(found.len());
    for e in &found {
        out.push(request_object(agent, &e.req)?);
    }
    Ok(Value::Object(agent.create_array_from_list(&out)?))
}

/// Copia de las entradas de la caché `a[0]` que coinciden con la petición `a[1]` ("Query
/// Cache"), o de todas si es `null`.
fn query_args(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Vec<Entry>> {
    let id = id_arg(&a.arg(0));
    let query = if a.arg(1).is_null() || a.arg(1).is_undefined() {
        None
    } else {
        match stored_request(agent, &a.arg(1)) {
            Some(r) => Some(r),
            None => return agent.type_error("Request expected"),
        }
    };
    let opts = QueryOptions {
        ignore_search: flag(&a.arg(2)),
        ignore_method: flag(&a.arg(3)),
        ignore_vary: flag(&a.arg(4)),
    };
    Ok(with_store(agent, |s| {
        let cache = s.caches.entry(id).or_default();
        match &query {
            None => cache.clone(),
            Some(q) => query_cache(q, opts, cache)
                .into_iter()
                .map(|i| cache[i].clone())
                .collect(),
        }
    }))
}

/// Una operación de "Batch Cache Operations" (§2.6 "cache batch operation").
struct Operation {
    request: StoredRequest,
    /// `Some` en un "put".
    response: Option<StoredResponse>,
    /// `None` en un "put" (sus opciones son nulas).
    options: Option<QueryOptions>,
}

/// `batch(id, ops)`: corre "Batch Cache Operations" sobre la caché `id`. Cada operación es
/// `["put", request, response, bytes | null]` o `["delete", request, null, null,
/// ignoreSearch, ignoreMethod, ignoreVary]`. Devuelve cuántas entradas quitaron los
/// "delete", o `"Nombre: mensaje"` si falló (y la caché queda como estaba).
fn batch(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = id_arg(&a.arg(0));
    let list = agent.create_list_from_array_like(&a.arg(1), false)?;
    let mut ops = Vec::with_capacity(list.len());
    for op in list {
        let Value::Object(o) = op else {
            return agent.type_error("operation expected");
        };
        let mut f = Vec::with_capacity(7);
        for i in 0..7u32 {
            f.push(agent.get(o, &PropertyKey::from(i), op.clone())?);
        }
        let kind = str_arg(agent, &f[0])?;
        let Some(request) = stored_request(agent, &f[1]) else {
            return Ok(Value::str("TypeError: Request expected"));
        };
        let (response, options) = match kind.as_str() {
            "put" => {
                let Some(r) = stored_response(agent, &f[2], &f[3]) else {
                    return Ok(Value::str("TypeError: Response expected"));
                };
                (Some(r), None)
            }
            "delete" => (
                None,
                Some(QueryOptions {
                    ignore_search: flag(&f[4]),
                    ignore_method: flag(&f[5]),
                    ignore_vary: flag(&f[6]),
                }),
            ),
            // §2.6 "Batch Cache Operations" paso 4.2.1.
            _ => return Ok(Value::str("TypeError: Invalid operation type")),
        };
        ops.push(Operation {
            request,
            response,
            options,
        });
    }
    let r = with_store(agent, |s| {
        let cache = s.caches.entry(id).or_default();
        let r = batch_operations(cache, ops);
        if r.is_ok() {
            s.save_cache(id);
        }
        r
    });
    Ok(match r {
        Ok(n) => Value::Number(n as f64),
        Err(e) => Value::str(&e),
    })
}

// --- Algoritmos (§2.6 / apéndice "Cache" del spec) ---

/// La URL serializada sin fragmento (y sin query con `ignoreSearch`) para comparar: en una
/// URL serializada el primer `#` empieza el fragmento y el primer `?` la query.
fn url_for_match(href: &str, ignore_search: bool) -> &str {
    let mut end = href.find('#').unwrap_or(href.len());
    if ignore_search && let Some(q) = href[..end].find('?') {
        end = q;
    }
    &href[..end]
}

/// "Request Matches Cached Item" (requestQuery, request, response, options).
///
/// El paso 1 mira el método de `requestQuery`: las peticiones guardadas son siempre `GET`
/// (`put` lo exige), así que es la única lectura en la que el paso tiene efecto.
fn request_matches(
    query: &StoredRequest,
    request: &StoredRequest,
    response: Option<&StoredResponse>,
    opts: QueryOptions,
) -> bool {
    // 1.
    if !opts.ignore_method && query.method != "GET" {
        return false;
    }
    // 2–5: las URL, excluyendo fragmentos (y la query con ignoreSearch).
    if url_for_match(&query.url, opts.ignore_search)
        != url_for_match(&request.url, opts.ignore_search)
    {
        return false;
    }
    // 6.
    let Some(response) = response else {
        return true;
    };
    if opts.ignore_vary || !response.headers.contains("vary") {
        return true;
    }
    // 7–8: `*` nunca coincide; cada cabecera nombrada debe tener el mismo valor combinado.
    for field in vary_fields(&response.headers) {
        if field == "*" || request.headers.get(&field) != query.headers.get(&field) {
            return false;
        }
    }
    true
}

/// "Query Cache" (requestQuery, options, targetStorage): los índices que coinciden.
fn query_cache(query: &StoredRequest, opts: QueryOptions, storage: &[Entry]) -> Vec<usize> {
    storage
        .iter()
        .enumerate()
        .filter(|(_, e)| request_matches(query, &e.req, Some(&e.res), opts))
        .map(|(i, _)| i)
        .collect()
}

fn remove_indices(cache: &mut Vec<Entry>, idx: &[usize]) {
    for &i in idx.iter().rev() {
        cache.remove(i);
    }
}

/// "Batch Cache Operations" (operations): todo o nada. Devuelve cuántas entradas quitaron
/// las operaciones "delete"; en un error la caché vuelve a su copia de respaldo.
fn batch_operations(cache: &mut Vec<Entry>, ops: Vec<Operation>) -> Result<usize, String> {
    // 2. backupCache.
    let backup = cache.clone();
    // 3. addedItems.
    let mut added: Vec<Entry> = Vec::new();
    let mut removed = 0;
    let run = || -> Result<(), String> {
        for op in ops {
            let opts = op.options.unwrap_or_default();
            // 4.2.3: dos operaciones del mismo lote sobre la misma entrada.
            if !query_cache(&op.request, opts, &added).is_empty() {
                return Err("InvalidStateError: duplicate requests in the batch".into());
            }
            match op.response {
                None => {
                    // 4.2.5: "delete".
                    let idx = query_cache(&op.request, opts, cache);
                    removed += idx.len();
                    remove_indices(cache, &idx);
                }
                Some(response) => {
                    // 4.2.6: "put".
                    if !is_http(&op.request.url) {
                        return Err("TypeError: Request scheme must be 'http' or 'https'".into());
                    }
                    if op.request.method != "GET" {
                        return Err("TypeError: Request method must be 'GET'".into());
                    }
                    let idx = query_cache(&op.request, QueryOptions::default(), cache);
                    remove_indices(cache, &idx);
                    let entry = Entry {
                        req: op.request,
                        res: response,
                    };
                    cache.push(entry.clone());
                    added.push(entry);
                }
            }
        }
        Ok(())
    };
    match run() {
        Ok(()) => Ok(removed),
        Err(e) => {
            // 5: se restaura la copia.
            *cache = backup;
            Err(e)
        }
    }
}

// --- Objetos de script ---

/// Un `Response` nuevo para una respuesta guardada, con guard "immutable" (§5.4.2 paso
/// 5.5: "creating a Response object, given response, "immutable"").
fn response_object(agent: &mut Agent, r: &StoredResponse) -> JsResult<Value> {
    let d = ResponseData {
        type_: r.type_.clone(),
        url: r.url_list.last().and_then(|u| Url::parse(u, None).ok()),
        redirected: r.redirected,
        status: r.status,
        status_text: r.status_text.clone(),
        body: Body {
            source: r.body.clone(),
            used: false,
            stream_only: false,
        },
    };
    crate::net::fetch::make_response(agent, None, d, r.headers.clone(), Guard::Immutable, None)
}

/// Un `Request` nuevo para una petición guardada, con guard "immutable" (§5.4.7) y una
/// señal que nunca se aborta.
fn request_object(agent: &mut Agent, r: &StoredRequest) -> JsResult<Value> {
    let Ok(url) = Url::parse(&r.url, None) else {
        return agent.type_error("Invalid cached URL");
    };
    let d = RequestData {
        method: r.method.clone(),
        url,
        body: Body::default(),
        mode: r.mode.clone(),
        credentials: r.credentials.clone(),
        cache: r.cache.clone(),
        redirect: r.redirect.clone(),
        referrer: r.referrer.clone(),
        referrer_policy: r.referrer_policy.clone(),
        integrity: r.integrity.clone(),
        keepalive: false,
        priority: "auto".into(),
    };
    let class = crate::net::classes(agent).request;
    let o = agent.host_create(class, Box::new(d));
    let h = headers::create(agent, r.headers.clone(), Guard::Immutable);
    let signal = new_signal(agent)?;
    if let Some(slots) = agent.host_slots_mut(o) {
        *slots = vec![h, signal, Value::Null];
    }
    Ok(Value::Object(o))
}

/// `new AbortController().signal` del realm en curso.
fn new_signal(agent: &mut Agent) -> JsResult<Value> {
    let g = agent.realms[agent.current_realm].global_object;
    let ctor = agent.get(g, &PropertyKey::from("AbortController"), Value::Object(g))?;
    let Value::Object(c) = ctor else {
        return Ok(Value::Null);
    };
    if !agent.is_callable(&ctor) {
        return Ok(Value::Null);
    }
    let ctrl = agent.construct(c, &[], None)?;
    let Value::Object(co) = ctrl else {
        return Ok(Value::Null);
    };
    agent.get(co, &PropertyKey::from("signal"), ctrl.clone())
}

// --- Persistencia ---

const INDEX_MAGIC: &str = "bf-caches 1";
const META_MAGIC: &[u8; 4] = b"BFC1";

impl CacheStorageData {
    fn id_of(&self, name: &str) -> Option<u64> {
        self.names
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, id)| *id)
    }

    /// Lee el almacén de `dir` (vacío si no hay nada o el formato no se entiende).
    fn load(dir: Option<PathBuf>) -> CacheStorageData {
        let mut s = CacheStorageData {
            dir,
            next_id: 1,
            next_body: 1,
            ..Default::default()
        };
        let Some(dir) = s.dir.clone() else {
            return s;
        };
        let Ok(index) = fs::read_to_string(dir.join("index")) else {
            return s;
        };
        let mut lines = index.lines();
        if lines.next() != Some(INDEX_MAGIC) {
            return s;
        }
        for line in lines {
            let Some((id, hex)) = line.split_once(' ') else {
                continue;
            };
            let (Ok(id), Some(name)) = (id.parse::<u64>(), from_hex(hex)) else {
                continue;
            };
            let entries = read_meta(&dir, id).unwrap_or_default();
            for e in &entries {
                s.next_body = s.next_body.max(e.res.body_file + 1);
            }
            s.next_id = s.next_id.max(id + 1);
            s.names.push((name, id));
            s.caches.insert(id, entries);
        }
        s
    }

    /// Guarda el índice (los nombres en orden).
    fn save_index(&self) {
        let Some(dir) = &self.dir else {
            return;
        };
        let mut text = String::from(INDEX_MAGIC);
        text.push('\n');
        for (name, id) in &self.names {
            text.push_str(&format!("{id} {}\n", to_hex(name.as_bytes())));
        }
        let _ = fs::create_dir_all(dir).and_then(|()| write_atomic(dir, "index", text.as_bytes()));
    }

    /// Guarda la caché `id` (si tiene nombre): cuerpos nuevos, metadatos y limpieza de los
    /// cuerpos que ya no usa.
    fn save_cache(&mut self, id: u64) {
        let Some(dir) = self.dir.clone() else {
            return;
        };
        if !self.names.iter().any(|(_, i)| *i == id) {
            return;
        }
        if fs::create_dir_all(&dir).is_err() {
            return;
        }
        let mut next_body = self.next_body;
        let Some(entries) = self.caches.get_mut(&id) else {
            return;
        };
        for e in entries.iter_mut() {
            if let Some(body) = &e.res.body
                && e.res.body_file == 0
            {
                let n = next_body;
                next_body += 1;
                if write_atomic(&dir, &format!("{id}-{n}.body"), body).is_ok() {
                    e.res.body_file = n;
                }
            }
        }
        self.next_body = next_body;
        let meta = encode_meta(entries);
        if write_atomic(&dir, &format!("{id}.meta"), &meta).is_err() {
            return;
        }
        let used: HashSet<u64> = entries.iter().map(|e| e.res.body_file).collect();
        remove_bodies(&dir, id, |n| !used.contains(&n));
    }

    /// Borra del disco los archivos de la caché `id`.
    fn remove_files(&self, id: u64) {
        let Some(dir) = &self.dir else {
            return;
        };
        let _ = fs::remove_file(dir.join(format!("{id}.meta")));
        remove_bodies(dir, id, |_| true);
    }
}

/// Borra los `<id>-<n>.body` de `dir` para los que `doomed(n)`.
fn remove_bodies(dir: &Path, id: u64, doomed: impl Fn(u64) -> bool) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    let prefix = format!("{id}-");
    for f in rd.flatten() {
        let name = f.file_name().to_string_lossy().into_owned();
        if let Some(n) = name
            .strip_prefix(&prefix)
            .and_then(|r| r.strip_suffix(".body"))
            .and_then(|n| n.parse::<u64>().ok())
            && doomed(n)
        {
            let _ = fs::remove_file(f.path());
        }
    }
}

/// Escritura atómica: `<name>.tmp` y `rename` encima de `<name>`.
fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = dir.join(format!("{name}.tmp"));
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, dir.join(name))
}

fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn from_hex(s: &str) -> Option<String> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let bytes = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    String::from_utf8(bytes).ok()
}

fn put_u32(out: &mut Vec<u8>, n: usize) {
    out.extend_from_slice(&(n as u32).to_le_bytes());
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    put_u32(out, s.len());
    out.extend_from_slice(s.as_bytes());
}

fn put_headers(out: &mut Vec<u8>, h: &HeaderList) {
    put_u32(out, h.0.len());
    for (n, v) in &h.0 {
        put_str(out, n);
        put_str(out, v);
    }
}

/// `<id>.meta`: `BFC1`, u32 entradas y por entrada (u32 LE + bytes para cada cadena):
/// método, URL, cabeceras, mode, credentials, cache, redirect, referrer, referrerPolicy,
/// integrity; tipo, lista de URL, redirected (u8), status (u16), statusText, cabeceras y
/// cuerpo (u8 0 = nulo; u8 1 + u64 = archivo).
fn encode_meta(entries: &[Entry]) -> Vec<u8> {
    let mut out = META_MAGIC.to_vec();
    put_u32(&mut out, entries.len());
    for e in entries {
        let q = &e.req;
        put_str(&mut out, &q.method);
        put_str(&mut out, &q.url);
        put_headers(&mut out, &q.headers);
        for s in [
            &q.mode,
            &q.credentials,
            &q.cache,
            &q.redirect,
            &q.referrer,
            &q.referrer_policy,
            &q.integrity,
        ] {
            put_str(&mut out, s);
        }
        let r = &e.res;
        put_str(&mut out, &r.type_);
        put_u32(&mut out, r.url_list.len());
        for u in &r.url_list {
            put_str(&mut out, u);
        }
        out.push(u8::from(r.redirected));
        out.extend_from_slice(&r.status.to_le_bytes());
        put_str(&mut out, &r.status_text);
        put_headers(&mut out, &r.headers);
        if r.body.is_some() && r.body_file != 0 {
            out.push(1);
            out.extend_from_slice(&r.body_file.to_le_bytes());
        } else {
            out.push(0);
        }
    }
    out
}

/// Un lector de `<id>.meta`.
struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<usize> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?) as usize)
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn str(&mut self) -> Option<String> {
        let n = self.u32()?;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
    fn headers(&mut self) -> Option<HeaderList> {
        let n = self.u32()?;
        let mut v = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            v.push((self.str()?, self.str()?));
        }
        Some(HeaderList(v))
    }
}

fn read_meta(dir: &Path, id: u64) -> Option<Vec<Entry>> {
    let bytes = fs::read(dir.join(format!("{id}.meta"))).ok()?;
    let mut r = Reader(&bytes);
    if r.take(4)? != META_MAGIC {
        return None;
    }
    let n = r.u32()?;
    let mut out = Vec::with_capacity(n.min(4096));
    for _ in 0..n {
        let req = StoredRequest {
            method: r.str()?,
            url: r.str()?,
            headers: r.headers()?,
            mode: r.str()?,
            credentials: r.str()?,
            cache: r.str()?,
            redirect: r.str()?,
            referrer: r.str()?,
            referrer_policy: r.str()?,
            integrity: r.str()?,
        };
        let type_ = r.str()?;
        let urls = r.u32()?;
        let mut url_list = Vec::with_capacity(urls.min(64));
        for _ in 0..urls {
            url_list.push(r.str()?);
        }
        let redirected = r.u8()? != 0;
        let status = r.u16()?;
        let status_text = r.str()?;
        let headers = r.headers()?;
        let (body, body_file) = if r.u8()? == 1 {
            let f = r.u64()?;
            let b = fs::read(dir.join(format!("{id}-{f}.body"))).ok()?;
            (Some(Arc::new(b)), f)
        } else {
            (None, 0)
        };
        out.push(Entry {
            req,
            res: StoredResponse {
                type_,
                url_list,
                redirected,
                status,
                status_text,
                headers,
                body,
                body_file,
            },
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(url: &str, headers: &[(&str, &str)]) -> StoredRequest {
        StoredRequest {
            method: "GET".into(),
            url: url.into(),
            headers: HeaderList(
                headers
                    .iter()
                    .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
                    .collect(),
            ),
            mode: "cors".into(),
            credentials: "same-origin".into(),
            cache: "default".into(),
            redirect: "follow".into(),
            referrer: "about:client".into(),
            referrer_policy: String::new(),
            integrity: String::new(),
        }
    }

    fn res(body: &str, headers: &[(&str, &str)]) -> StoredResponse {
        StoredResponse {
            type_: "basic".into(),
            url_list: vec!["https://a.test/x".into()],
            redirected: false,
            status: 200,
            status_text: "OK".into(),
            headers: req("", headers).headers,
            body: Some(Arc::new(body.as_bytes().to_vec())),
            body_file: 0,
        }
    }

    #[test]
    fn url_match_ignores_fragment_and_optionally_query() {
        assert_eq!(url_for_match("https://a/b?c#d", false), "https://a/b?c");
        assert_eq!(url_for_match("https://a/b?c#d", true), "https://a/b");
        assert_eq!(url_for_match("https://a/b#d?x", true), "https://a/b");
    }

    #[test]
    fn vary_star_never_matches() {
        let q = req("https://a.test/x", &[]);
        let r = res("x", &[("Vary", "*")]);
        assert!(!request_matches(&q, &q, Some(&r), QueryOptions::default()));
        let o = QueryOptions {
            ignore_vary: true,
            ..Default::default()
        };
        assert!(request_matches(&q, &q, Some(&r), o));
    }

    #[test]
    fn batch_rolls_back_on_duplicates() {
        let mut cache = vec![Entry {
            req: req("https://a.test/old", &[]),
            res: res("old", &[]),
        }];
        let op = |u: &str| Operation {
            request: req(u, &[]),
            response: Some(res("n", &[])),
            options: None,
        };
        let r = batch_operations(
            &mut cache,
            vec![op("https://a.test/1"), op("https://a.test/1")],
        );
        assert!(r.unwrap_err().starts_with("InvalidStateError"));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn persists_and_loads() {
        let dir = std::env::temp_dir().join(format!(
            "bf-cache-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        let mut s = CacheStorageData::load(Some(dir.clone()));
        s.names.push(("v1".into(), 7));
        s.next_id = 8;
        s.caches.insert(
            7,
            vec![Entry {
                req: req("https://a.test/x", &[("Accept", "text/plain")]),
                res: res("hola", &[("Content-Type", "text/plain")]),
            }],
        );
        s.save_index();
        s.save_cache(7);
        // Un reemplazo deja un solo cuerpo en disco.
        s.caches.get_mut(&7).expect("7")[0].res = res("adiós", &[]);
        s.save_cache(7);
        let t = CacheStorageData::load(Some(dir.clone()));
        assert_eq!(t.names, vec![("v1".to_owned(), 7)]);
        assert_eq!(t.next_id, 8);
        let e = &t.caches[&7][0];
        assert_eq!(e.req.headers.get("accept").as_deref(), Some("text/plain"));
        assert_eq!(
            e.res.body.as_deref().map(Vec::as_slice),
            Some("adiós".as_bytes())
        );
        let bodies = fs::read_dir(&dir)
            .expect("dir")
            .flatten()
            .filter(|f| f.file_name().to_string_lossy().ends_with(".body"))
            .count();
        assert_eq!(bodies, 1);
        t.remove_files(7);
        let _ = fs::remove_dir_all(&dir);
    }
}
