//! Performance Timeline (nivel 2) y User Timing (nivel 3): `PerformanceEntry`,
//! `PerformanceMark`, `PerformanceMeasure`, `PerformanceObserver`,
//! `PerformanceObserverEntryList`, `PerformanceResourceTiming` y
//! `PerformanceNavigationTiming`, más `performance.getEntries*`, `mark`, `measure`,
//! `clearMarks` y `clearMeasures`. **Dueño: E3** (el resto de `performance` es de E4, en
//! [`super::performance`]; estas operaciones reemplazan sus versiones vacías).
//!
//! Tipos soportados: `mark`, `measure`, `navigation` (una entrada con los instantes que el
//! motor conoce, [`super::performance::Timing`]) y `resource` (las imágenes que carga
//! `new Image()` y lo que se anote con [`add_resource_entry`]). Los callbacks de los
//! observers corren en una tarea (*queue the PerformanceObserver task*), nunca dentro de la
//! llamada que creó la entrada.

use bf_js::host::{HostClass, HostClassId, HostHandle, TaskSource};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::{CallArgs, NativeFn};
use bf_js::vm::{Agent, JsResult, RealmId, Throw};

use super::{BomState, bom_classes, state};
use crate::WebContextHandle;

/// Los tipos de entrada soportados, en orden alfabético (`supportedEntryTypes`).
const SUPPORTED: [&str; 4] = ["mark", "measure", "navigation", "resource"];

/// Los atributos de `PerformanceTiming` (un `mark` no puede llamarse así; un `measure` los
/// acepta como extremos, User Timing §4.2 *convert a name to a timestamp*).
const TIMING_NAMES: [&str; 21] = [
    "navigationStart",
    "unloadEventStart",
    "unloadEventEnd",
    "redirectStart",
    "redirectEnd",
    "fetchStart",
    "domainLookupStart",
    "domainLookupEnd",
    "connectStart",
    "connectEnd",
    "secureConnectionStart",
    "requestStart",
    "responseStart",
    "responseEnd",
    "domLoading",
    "domInteractive",
    "domContentLoadedEventStart",
    "domContentLoadedEventEnd",
    "domComplete",
    "loadEventStart",
    "loadEventEnd",
];

/// Los ids de las clases (dato del agente).
#[derive(Debug, Clone, Copy)]
struct TimelineClasses {
    mark: HostClassId,
    measure: HostClassId,
    resource: HostClassId,
    navigation: HostClassId,
    observer: HostClassId,
    entry_list: HostClassId,
}

fn classes(agent: &Agent) -> TimelineClasses {
    *agent
        .embedder_data::<TimelineClasses>()
        .expect("performance_observer::register no se llamó")
}

/// Los instantes de un `PerformanceResourceTiming` (y de navegación), en ms desde el
/// `timeOrigin`.
#[derive(Debug, Clone, Default)]
struct ResourceTimes {
    initiator_type: String,
    fetch_start: f64,
    response_start: f64,
    response_end: f64,
    body_size: f64,
    // Solo navegación.
    dom_interactive: f64,
    dom_content_loaded: f64,
    dom_complete: f64,
    load_event: f64,
}

/// El payload de un `PerformanceEntry` (el `detail` de marks y measures va en el slot 0).
#[derive(Debug, Clone)]
struct Entry {
    name: String,
    entry_type: &'static str,
    start: f64,
    duration: f64,
    times: Option<ResourceTimes>,
}

/// El payload de un `PerformanceObserver` (el callback va en el slot 0).
#[derive(Debug, Clone, Copy)]
struct ObserverData {
    id: u64,
}

/// El payload de un `PerformanceObserverEntryList`: sus entradas (en el slot 0, un array).
struct EntryListData;

/// *Observer type* (§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObserverType {
    Single,
    Multiple,
}

/// Un observer registrado (*registered performance observer*).
struct Registered {
    id: u64,
    /// El objeto, fijado mientras está registrado.
    object: HostHandle,
    types: Vec<String>,
    /// El *observer buffer*.
    buffer: Vec<HostHandle>,
    /// `droppedEntriesCount` para el próximo callback (se pidió `buffered`).
    dropped: Option<u32>,
}

/// El estado de la línea de tiempo de una ventana (dato del realm).
#[derive(Default)]
struct Timeline {
    /// Marks y measures (el *performance entry buffer*), en orden de creación.
    entries: Vec<(HostHandle, Entry)>,
    /// El *resource timing buffer*.
    resources: Vec<(HostHandle, Entry)>,
    navigation: Option<HostHandle>,
    observers: Vec<Registered>,
    /// El tipo de cada observer creado (aunque no esté registrado).
    types: std::collections::HashMap<u64, ObserverType>,
    next_observer: u64,
    /// *Performance observer task queued flag*.
    task_queued: bool,
    supported: Option<HostHandle>,
}

fn timeline(agent: &mut Agent) -> &mut Timeline {
    let realm = WebContextHandle::current(agent).realm;
    if agent.realm_data::<Timeline>(realm).is_none() {
        agent.set_realm_data(realm, Timeline::default());
    }
    agent
        .realm_data_mut::<Timeline>(realm)
        .expect("Timeline recién puesto")
}

/// Registra las clases (la primera vez) y reemplaza las operaciones de la línea de tiempo de
/// `Performance` (lo llama [`super::register`]).
pub fn register(agent: &mut Agent, _realm: RealmId) {
    if agent.embedder_data::<TimelineClasses>().is_some() {
        return;
    }
    let entry = agent.register_host_class(entry_class());
    let mark_class = agent.register_host_class(
        HostClass::new("PerformanceMark")
            .parent(entry)
            .constructor(mark_ctor, 1)
            .attribute("detail", Some(detail), None),
    );
    let measure_class = agent.register_host_class(
        HostClass::new("PerformanceMeasure")
            .parent(entry)
            .attribute("detail", Some(detail), None),
    );
    let resource = agent.register_host_class(resource_class(entry));
    let navigation = agent.register_host_class(navigation_class(resource));
    let observer = agent.register_host_class(
        HostClass::new("PerformanceObserver")
            .constructor(observer_ctor, 1)
            .method("observe", 0, observe)
            .method("disconnect", 0, disconnect)
            .method("takeRecords", 0, take_records)
            .static_attribute("supportedEntryTypes", Some(supported_entry_types), None),
    );
    let entry_list = agent.register_host_class(
        HostClass::new("PerformanceObserverEntryList")
            .method("getEntries", 0, list_get_entries)
            .method("getEntriesByType", 1, list_get_entries_by_type)
            .method("getEntriesByName", 1, list_get_entries_by_name),
    );
    agent.set_embedder_data(TimelineClasses {
        mark: mark_class,
        measure: measure_class,
        resource,
        navigation,
        observer,
        entry_list,
    });
    // `partial interface Performance` (Performance Timeline §4, User Timing §3): las
    // definiciones nuevas van después de las vacías de `performance.rs` y las tapan.
    let performance = bom_classes(agent).performance;
    agent.host_extend_class(
        performance,
        HostClass::new("Performance")
            .method("getEntries", 0, get_entries)
            .method("getEntriesByType", 1, get_entries_by_type)
            .method("getEntriesByName", 1, get_entries_by_name)
            .method("mark", 1, mark)
            .method("measure", 1, measure)
            .method("clearMarks", 0, clear_marks)
            .method("clearMeasures", 0, clear_measures)
            .method("clearResourceTimings", 0, clear_resource_timings),
    );
}

fn entry_class() -> HostClass {
    HostClass::new("PerformanceEntry")
        .attribute("name", Some(entry_name), None)
        .attribute("entryType", Some(entry_type), None)
        .attribute("startTime", Some(entry_start), None)
        .attribute("duration", Some(entry_duration), None)
        .method("toJSON", 0, entry_to_json)
}

/// Define un getter por atributo numérico y la tabla `(nombre, getter, valor)` que usan las
/// clases y `toJSON`.
macro_rules! time_fields {
    ($table:ident, $($name:literal => $getter:ident : |$t:pat_param| $e:expr),* $(,)?) => {
        $(
            fn $getter(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
                let v = agent
                    .host_unwrap::<Entry>(&a.this)
                    .and_then(|e| e.times.as_ref())
                    .map_or(0.0, |$t: &ResourceTimes| $e);
                Ok(Value::Number(v))
            }
        )*
        const $table: &[TimeField] = &[$(($name, $getter, |$t: &ResourceTimes| $e)),*];
    };
}

/// Un atributo numérico: nombre, getter y cómo sale de [`ResourceTimes`].
type TimeField = (&'static str, NativeFn, fn(&ResourceTimes) -> f64);

// Resource Timing §4.2: los que el motor conoce; sin redirecciones ni conexión aparte.
time_fields!(
    RESOURCE_FIELDS,
    "workerStart" => r_worker_start: |_| 0.0,
    "redirectStart" => r_redirect_start: |_| 0.0,
    "redirectEnd" => r_redirect_end: |_| 0.0,
    "fetchStart" => r_fetch_start: |t| t.fetch_start,
    "domainLookupStart" => r_dns_start: |t| t.fetch_start,
    "domainLookupEnd" => r_dns_end: |t| t.fetch_start,
    "connectStart" => r_connect_start: |t| t.fetch_start,
    "connectEnd" => r_connect_end: |t| t.fetch_start,
    "secureConnectionStart" => r_secure_start: |_| 0.0,
    "requestStart" => r_request_start: |t| t.fetch_start,
    "firstInterimResponseStart" => r_interim_start: |_| 0.0,
    "responseStart" => r_response_start: |t| t.response_start,
    "responseEnd" => r_response_end: |t| t.response_end,
    "transferSize" => r_transfer_size: |t| t.body_size,
    "encodedBodySize" => r_encoded_size: |t| t.body_size,
    "decodedBodySize" => r_decoded_size: |t| t.body_size,
    "responseStatus" => r_status: |_| 200.0,
);

// Navigation Timing 2 §3.
time_fields!(
    NAVIGATION_FIELDS,
    "unloadEventStart" => n_unload_start: |_| 0.0,
    "unloadEventEnd" => n_unload_end: |_| 0.0,
    "domInteractive" => n_dom_interactive: |t| t.dom_interactive,
    "domContentLoadedEventStart" => n_dcl_start: |t| t.dom_content_loaded,
    "domContentLoadedEventEnd" => n_dcl_end: |t| t.dom_content_loaded,
    "domComplete" => n_dom_complete: |t| t.dom_complete,
    "loadEventStart" => n_load_start: |t| t.load_event,
    "loadEventEnd" => n_load_end: |t| t.load_event,
    "redirectCount" => n_redirect_count: |_| 0.0,
);

fn resource_class(entry: HostClassId) -> HostClass {
    let mut c = HostClass::new("PerformanceResourceTiming")
        .parent(entry)
        .attribute("initiatorType", Some(initiator_type), None)
        .attribute("nextHopProtocol", Some(empty_string), None)
        .attribute("deliveryType", Some(empty_string), None)
        .attribute("contentType", Some(empty_string), None)
        .attribute("renderBlockingStatus", Some(non_blocking), None)
        .attribute("serverTiming", Some(empty_array), None);
    for &(name, getter, _) in RESOURCE_FIELDS {
        c = c.attribute(name, Some(getter), None);
    }
    c.method("toJSON", 0, entry_to_json)
}

fn navigation_class(resource: HostClassId) -> HostClass {
    let mut c = HostClass::new("PerformanceNavigationTiming")
        .parent(resource)
        .attribute("type", Some(navigation_type), None);
    for &(name, getter, _) in NAVIGATION_FIELDS {
        c = c.attribute(name, Some(getter), None);
    }
    c.method("toJSON", 0, entry_to_json)
}

// --- Relojes ---

/// `performance.now()` (el reloj de [`super::performance`]): ms desde el `timeOrigin`.
fn now(agent: &mut Agent) -> f64 {
    let t = agent.host.hooks.now() - state(agent).now_origin;
    (t * 10.0).floor() / 10.0
}

/// Un instante de [`super::performance::Timing`] (época Unix) relativo al `timeOrigin`; 0 si
/// todavía no ocurrió.
fn relative(s: &BomState, t: f64) -> f64 {
    if t == 0.0 {
        0.0
    } else {
        (t - s.time_origin).max(0.0)
    }
}

// --- PerformanceEntry ---

fn entry_of(agent: &mut Agent, v: &Value) -> JsResult<Entry> {
    match agent.host_unwrap::<Entry>(v) {
        Some(e) => Ok(e.clone()),
        None => agent.type_error("Illegal invocation"),
    }
}

fn entry_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&entry_of(agent, &a.this)?.name))
}

fn entry_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(entry_of(agent, &a.this)?.entry_type))
}

fn entry_start(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(entry_of(agent, &a.this)?.start))
}

fn entry_duration(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(entry_of(agent, &a.this)?.duration))
}

fn detail(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    entry_of(agent, &a.this)?;
    let Value::Object(o) = &a.this else {
        return Ok(Value::Null);
    };
    Ok(agent
        .host_slots_mut(*o)
        .and_then(|s| s.first().cloned())
        .unwrap_or(Value::Null))
}

fn initiator_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let e = entry_of(agent, &a.this)?;
    Ok(Value::str(
        e.times.as_ref().map_or("", |t| t.initiator_type.as_str()),
    ))
}

fn navigation_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    entry_of(agent, &a.this)?;
    Ok(Value::str("navigate"))
}

fn empty_string(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(""))
}

fn non_blocking(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str("non-blocking"))
}

fn empty_array(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Object(agent.create_array_from_list(&[])?))
}

/// `toJSON()` (WebIDL *default toJSON steps*): los atributos de la interfaz y sus ancestros.
fn entry_to_json(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let e = entry_of(agent, &a.this)?;
    let o = agent.new_plain_object();
    let put = |agent: &mut Agent, k: &str, v: Value| {
        agent.put_data(o, PropertyKey::from(k), v, true, true, true);
    };
    put(agent, "name", Value::str(&e.name));
    put(agent, "entryType", Value::str(e.entry_type));
    put(agent, "startTime", Value::Number(e.start));
    put(agent, "duration", Value::Number(e.duration));
    if matches!(e.entry_type, "mark" | "measure") {
        let d = detail(agent, a)?;
        put(agent, "detail", d);
    }
    if let Some(t) = &e.times {
        put(agent, "initiatorType", Value::str(&t.initiator_type));
        put(agent, "nextHopProtocol", Value::str(""));
        for &(name, _, f) in RESOURCE_FIELDS {
            put(agent, name, Value::Number(f(t)));
        }
        if e.entry_type == "navigation" {
            for &(name, _, f) in NAVIGATION_FIELDS {
                put(agent, name, Value::Number(f(t)));
            }
            put(agent, "type", Value::str("navigate"));
        }
    }
    Ok(Value::Object(o))
}

/// Crea el objeto de una entrada (con `detail` en el slot 0).
fn create_entry(agent: &mut Agent, class: HostClassId, e: Entry, detail: Option<Value>) -> Value {
    let o = agent.host_create(class, Box::new(e));
    if let Some(d) = detail
        && let Some(slots) = agent.host_slots_mut(o)
    {
        slots.clear();
        slots.push(d);
    }
    Value::Object(o)
}

/// La entrada de navegación de la ventana (Navigation Timing 2 §3), creada la primera vez
/// que se pide. Sus instantes se leen al crearla: los que no ocurrieron quedan en 0.
fn navigation_entry(agent: &mut Agent) -> Option<Value> {
    if let Some(h) = timeline(agent).navigation
        && let Some(v) = agent.host_pinned(h)
    {
        // Se rehace mientras la carga no terminó, para que lleve los instantes nuevos.
        let done = agent
            .host_unwrap::<Entry>(&v)
            .and_then(|e| e.times.as_ref())
            .is_some_and(|t| t.load_event > 0.0);
        if done {
            return Some(v);
        }
    }
    let realm = WebContextHandle::current(agent).realm;
    let s = agent.realm_data::<BomState>(realm)?;
    let t = s.timing;
    let times = ResourceTimes {
        initiator_type: "navigation".into(),
        fetch_start: relative(s, t.fetch_start),
        response_start: relative(s, t.response_end),
        response_end: relative(s, t.response_end),
        body_size: 0.0,
        dom_interactive: relative(s, t.dom_interactive),
        dom_content_loaded: relative(s, t.dom_content_loaded),
        dom_complete: relative(s, t.dom_complete),
        load_event: relative(s, t.load_event),
    };
    let url = WebContextHandle::current(agent).get(agent).url.href();
    let e = Entry {
        name: url,
        entry_type: "navigation",
        start: 0.0,
        duration: times.load_event,
        times: Some(times),
    };
    let class = classes(agent).navigation;
    let v = create_entry(agent, class, e, None);
    let h = agent.host_pin(v.clone());
    if let Some(old) = timeline(agent).navigation.replace(h) {
        agent.host_unpin(old);
    }
    Some(v)
}

/// Las entradas de los buffers de la ventana que cumplen `keep`, en orden cronológico
/// (*filter buffer map by name and type*, Performance Timeline §6).
fn buffered_entries(agent: &mut Agent, keep: &dyn Fn(&Entry) -> bool) -> Vec<Value> {
    let mut out: Vec<(f64, Value)> = Vec::new();
    if let Some(nav) = navigation_entry(agent)
        && let Some(e) = agent.host_unwrap::<Entry>(&nav)
        && keep(e)
    {
        out.push((e.start, nav));
    }
    let tl = timeline(agent);
    let handles: Vec<(f64, HostHandle)> = tl
        .entries
        .iter()
        .chain(tl.resources.iter())
        .filter(|(_, e)| keep(e))
        .map(|(h, e)| (e.start, *h))
        .collect();
    for (start, h) in handles {
        if let Some(v) = agent.host_pinned(h) {
            out.push((start, v));
        }
    }
    // Orden estable por `startTime`.
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out.into_iter().map(|(_, v)| v).collect()
}

fn array(agent: &mut Agent, values: &[Value]) -> JsResult<Value> {
    Ok(Value::Object(agent.create_array_from_list(values)?))
}

fn get_entries(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let v = buffered_entries(agent, &|_| true);
    array(agent, &v)
}

fn get_entries_by_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let ty = agent.to_string(&a.arg(0))?.to_string_lossy();
    let v = buffered_entries(agent, &|e| e.entry_type == ty);
    array(agent, &v)
}

fn get_entries_by_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = agent.to_string(&a.arg(0))?.to_string_lossy();
    let ty = if a.arg(1).is_undefined() {
        None
    } else {
        Some(agent.to_string(&a.arg(1))?.to_string_lossy())
    };
    let v = buffered_entries(agent, &|e| {
        e.name == name && ty.as_deref().is_none_or(|t| e.entry_type == t)
    });
    array(agent, &v)
}

// --- User Timing ---

/// Un miembro de un diccionario (`undefined` si `dict` no es un objeto).
fn member(agent: &mut Agent, dict: &Value, name: &str) -> JsResult<Value> {
    match dict {
        Value::Object(o) => agent.get(*o, &PropertyKey::from(name), dict.clone()),
        _ => Ok(Value::Undefined),
    }
}

/// El `detail` de un mark o measure: un clon estructurado (`null` si no se dio).
fn clone_detail(agent: &mut Agent, v: Value) -> JsResult<Value> {
    if v.is_undefined() || v.is_null() {
        return Ok(Value::Null);
    }
    crate::workers::ports::clone_within_agent(agent, &v, &[])
}

/// Los pasos de `new PerformanceMark(markName, markOptions)` (User Timing §4.1).
fn new_mark(agent: &mut Agent, a: &CallArgs<'_>, method: Option<&str>) -> JsResult<Value> {
    let fail = |m: &str| match method {
        Some(op) => format!("Failed to execute '{op}' on 'Performance': {m}"),
        None => format!("Failed to construct 'PerformanceMark': {m}"),
    };
    let name = agent.to_string(&a.arg(0))?.to_string_lossy();
    let options = a.arg(1);
    if !matches!(options, Value::Undefined | Value::Null | Value::Object(_)) {
        return agent.type_error(&fail(
            "The provided value is not of type 'PerformanceMarkOptions'.",
        ));
    }
    // 1. En una ventana, los nombres de `PerformanceTiming` están reservados.
    if TIMING_NAMES.contains(&name.as_str()) {
        let msg = fail(&format!(
            "'{name}' is part of the PerformanceTiming interface, and cannot be used as a mark name."
        ));
        return crate::net::dom_error(agent, "SyntaxError", &msg);
    }
    let detail_v = member(agent, &options, "detail")?;
    let start_v = member(agent, &options, "startTime")?;
    // 3-4. `startTime` (no negativo) o ahora.
    let start = if start_v.is_undefined() {
        now(agent)
    } else {
        let t = agent.to_number(&start_v)?;
        if !t.is_finite() {
            return agent.type_error(&fail("The provided double value is non-finite."));
        }
        if t < 0.0 {
            return agent.type_error(&fail(&format!(
                "'{name}' cannot have a negative start time."
            )));
        }
        t
    };
    // 5. `detail`.
    let d = clone_detail(agent, detail_v)?;
    let e = Entry {
        name,
        entry_type: "mark",
        start,
        duration: 0.0,
        times: None,
    };
    let class = classes(agent).mark;
    let v = match a.new_target {
        Some(nt) => {
            let o = agent.host_create_from_new_target(nt, class, None, Box::new(e))?;
            if let Some(slots) = agent.host_slots_mut(o) {
                slots.clear();
                slots.push(d);
            }
            Value::Object(o)
        }
        None => create_entry(agent, class, e, Some(d)),
    };
    Ok(v)
}

fn mark_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    new_mark(agent, a, None)
}

/// `performance.mark(markName, markOptions)` (User Timing §3.1.1).
fn mark(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let args = CallArgs {
        this: a.this.clone(),
        args: a.args,
        new_target: None,
        callee: a.callee,
    };
    let v = new_mark(agent, &args, Some("mark"))?;
    add_to_buffer(agent, v.clone(), false);
    Ok(v)
}

/// *Convert a mark to a timestamp* (User Timing §4.2): un nombre de mark (el último con ese
/// nombre) o de `PerformanceTiming`, o un número no negativo.
fn mark_timestamp(agent: &mut Agent, v: &Value, what: &str) -> JsResult<f64> {
    let fail = |m: &str| format!("Failed to execute 'measure' on 'Performance': {m}");
    if let Value::Number(t) = v {
        if *t < 0.0 {
            return agent.type_error(&fail(&format!("'{what}' cannot be negative.")));
        }
        return Ok(*t);
    }
    let name = agent.to_string(v)?.to_string_lossy();
    if let Some(i) = TIMING_NAMES.iter().position(|n| *n == name) {
        let (t, start) = {
            let s = state(agent);
            let f = timing_field(&s.timing, i);
            (f, s.timing.navigation_start)
        };
        if i != 0 && t == 0.0 {
            let msg = fail(&format!(
                "'{name}' is empty: either the event hasn't happened yet, or it would provide cross-origin timing information."
            ));
            return crate::net::dom_error(agent, "InvalidAccessError", &msg);
        }
        return Ok((t - start).max(0.0));
    }
    let found = timeline(agent)
        .entries
        .iter()
        .rev()
        .find(|(_, e)| e.entry_type == "mark" && e.name == name)
        .map(|(_, e)| e.start);
    match found {
        Some(t) => Ok(t),
        None => {
            let msg = fail(&format!("The mark '{name}' does not exist."));
            crate::net::dom_error(agent, "SyntaxError", &msg)
        }
    }
}

/// El valor del atributo `i` de [`TIMING_NAMES`] (época Unix).
fn timing_field(t: &super::performance::Timing, i: usize) -> f64 {
    match TIMING_NAMES[i] {
        "navigationStart" => t.navigation_start,
        "unloadEventStart"
        | "unloadEventEnd"
        | "redirectStart"
        | "redirectEnd"
        | "secureConnectionStart" => 0.0,
        "responseStart" | "responseEnd" => t.response_end,
        "domLoading" => t.dom_loading,
        "domInteractive" => t.dom_interactive,
        "domContentLoadedEventStart" | "domContentLoadedEventEnd" => t.dom_content_loaded,
        "domComplete" => t.dom_complete,
        "loadEventStart" | "loadEventEnd" => t.load_event,
        _ => t.fetch_start,
    }
}

/// `performance.measure(measureName, startOrMeasureOptions, endMark)` (User Timing §3.1.3).
fn measure(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let fail = |m: &str| format!("Failed to execute 'measure' on 'Performance': {m}");
    let name = agent.to_string(&a.arg(0))?.to_string_lossy();
    let start_or_options = a.arg(1);
    let end_mark = a.arg(2);
    // Un diccionario `PerformanceMeasureOptions` con algún miembro.
    let (mut start_m, mut end_m, mut duration_m, mut detail_m) = (
        Value::Undefined,
        Value::Undefined,
        Value::Undefined,
        Value::Undefined,
    );
    let is_options = matches!(start_or_options, Value::Object(_));
    if is_options {
        detail_m = member(agent, &start_or_options, "detail")?;
        duration_m = member(agent, &start_or_options, "duration")?;
        end_m = member(agent, &start_or_options, "end")?;
        start_m = member(agent, &start_or_options, "start")?;
    }
    let non_empty = is_options
        && [&start_m, &end_m, &duration_m, &detail_m]
            .iter()
            .any(|v| !v.is_undefined());
    // 1.
    if non_empty {
        if !end_mark.is_undefined() {
            return agent.type_error(&fail(
                "If a non-empty PerformanceMeasureOptions object was passed, |end_mark| must not be passed.",
            ));
        }
        if start_m.is_undefined() && end_m.is_undefined() {
            return agent.type_error(&fail(
                "If a non-empty PerformanceMeasureOptions object was passed, at least one of its 'start' or 'end' members must be present.",
            ));
        }
        if !start_m.is_undefined() && !end_m.is_undefined() && !duration_m.is_undefined() {
            return agent.type_error(&fail(
                "If a non-empty PerformanceMeasureOptions object was passed, it must not have all of its 'start', 'duration', and 'end' members present.",
            ));
        }
    }
    let duration = if duration_m.is_undefined() {
        None
    } else {
        let d = agent.to_number(&duration_m)?;
        if d < 0.0 {
            return agent.type_error(&fail("'duration' cannot be negative."));
        }
        Some(d)
    };
    // 2. El final.
    let end = if !end_mark.is_undefined() {
        mark_timestamp(agent, &end_mark, "end")?
    } else if !end_m.is_undefined() {
        mark_timestamp(agent, &end_m, "end")?
    } else if let (false, Some(d)) = (start_m.is_undefined(), duration) {
        let s = mark_timestamp(agent, &start_m, "start")?;
        s + d
    } else {
        now(agent)
    };
    // 3. El comienzo.
    let start = if !start_m.is_undefined() {
        mark_timestamp(agent, &start_m, "start")?
    } else if let (Some(d), false) = (duration, end_m.is_undefined()) {
        end - d
    } else if !is_options && !start_or_options.is_undefined() {
        mark_timestamp(agent, &start_or_options, "start")?
    } else {
        0.0
    };
    let d = clone_detail(agent, detail_m)?;
    let e = Entry {
        name,
        entry_type: "measure",
        start,
        duration: end - start,
        times: None,
    };
    let class = classes(agent).measure;
    let v = create_entry(agent, class, e, Some(d));
    add_to_buffer(agent, v.clone(), false);
    Ok(v)
}

fn clear_by(agent: &mut Agent, a: &CallArgs<'_>, ty: &str) -> JsResult<Value> {
    let name = if a.arg(0).is_undefined() {
        None
    } else {
        Some(agent.to_string(&a.arg(0))?.to_string_lossy())
    };
    let tl = timeline(agent);
    let mut removed = Vec::new();
    tl.entries.retain(|(h, e)| {
        let gone = e.entry_type == ty && name.as_deref().is_none_or(|n| e.name == n);
        if gone {
            removed.push(*h);
        }
        !gone
    });
    for h in removed {
        agent.host_unpin(h);
    }
    Ok(Value::Undefined)
}

fn clear_marks(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    clear_by(agent, a, "mark")
}

fn clear_measures(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    clear_by(agent, a, "measure")
}

fn clear_resource_timings(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let removed = std::mem::take(&mut timeline(agent).resources);
    for (h, _) in removed {
        agent.host_unpin(h);
    }
    Ok(Value::Undefined)
}

/// El tope del *resource timing buffer* (Resource Timing §4.4, `setResourceTimingBufferSize`
/// no se implementa todavía).
const RESOURCE_BUFFER_SIZE: usize = 250;

/// Agrega `v` a su buffer y la reparte a los observers (*queue a PerformanceEntry*,
/// Performance Timeline §6).
fn add_to_buffer(agent: &mut Agent, v: Value, resource: bool) {
    let Some(e) = agent.host_unwrap::<Entry>(&v).cloned() else {
        return;
    };
    let h = agent.host_pin(v.clone());
    let tl = timeline(agent);
    if resource {
        if tl.resources.len() < RESOURCE_BUFFER_SIZE {
            tl.resources.push((h, e.clone()));
        } else {
            agent.host_unpin(h);
        }
    } else {
        tl.entries.push((h, e.clone()));
    }
    queue_entry(agent, &v, e.entry_type);
}

/// *Queue a PerformanceEntry*: a cada observer registrado interesado en `ty` le llega la
/// entrada y se encola la tarea de los observers.
fn queue_entry(agent: &mut Agent, v: &Value, ty: &str) {
    let interested: Vec<usize> = timeline(agent)
        .observers
        .iter()
        .enumerate()
        .filter(|(_, o)| o.types.iter().any(|t| t == ty))
        .map(|(i, _)| i)
        .collect();
    if interested.is_empty() {
        return;
    }
    for i in interested {
        let h = agent.host_pin(v.clone());
        timeline(agent).observers[i].buffer.push(h);
    }
    queue_observer_task(agent);
}

/// *Queue the PerformanceObserver task* (§6): una sola tarea pendiente a la vez.
fn queue_observer_task(agent: &mut Agent) {
    let tl = timeline(agent);
    if tl.task_queued {
        return;
    }
    tl.task_queued = true;
    let realm = WebContextHandle::current(agent).realm;
    agent.queue_task(
        TaskSource::Other,
        Box::new(move |agent: &mut Agent| {
            let saved = agent.current_realm;
            agent.current_realm = realm;
            deliver(agent);
            agent.current_realm = saved;
        }),
    );
}

/// La tarea: a cada observer con entradas, su callback con
/// `(PerformanceObserverEntryList, observer, {droppedEntriesCount})`.
fn deliver(agent: &mut Agent) {
    timeline(agent).task_queued = false;
    let ids: Vec<u64> = timeline(agent).observers.iter().map(|o| o.id).collect();
    for id in ids {
        let Some(o) = timeline(agent).observers.iter_mut().find(|o| o.id == id) else {
            continue;
        };
        if o.buffer.is_empty() {
            continue;
        }
        let buffer = std::mem::take(&mut o.buffer);
        let dropped = o.dropped.take();
        let object = o.object;
        let entries: Vec<Value> = buffer
            .into_iter()
            .filter_map(|h| agent.host_unpin(h))
            .collect();
        let Some(observer) = agent.host_pinned(object) else {
            continue;
        };
        let Value::Object(oo) = &observer else {
            continue;
        };
        let callback = agent
            .host_slots_mut(*oo)
            .and_then(|s| s.first().cloned())
            .unwrap_or_default();
        let list = match entry_list(agent, &entries) {
            Ok(l) => l,
            Err(_) => continue,
        };
        let options = agent.new_plain_object();
        if let Some(n) = dropped {
            agent.put_data(
                options,
                PropertyKey::from("droppedEntriesCount"),
                Value::Number(f64::from(n)),
                true,
                true,
                true,
            );
        }
        let r = agent.call(
            &callback,
            observer.clone(),
            &[list, observer.clone(), Value::Object(options)],
        );
        if let Err(Throw::Value(e)) = r {
            agent.report_exception(&e);
        }
    }
}

// --- PerformanceObserver ---

/// `new PerformanceObserver(callback)` (§5.1).
fn observer_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error(
            "Failed to construct 'PerformanceObserver': Please use the 'new' operator, this DOM object constructor cannot be called as a function.",
        );
    };
    let callback = a.arg(0);
    if !agent.is_callable(&callback) {
        return agent.type_error(
            "Failed to construct 'PerformanceObserver': The callback provided as parameter 1 is not a function.",
        );
    }
    let tl = timeline(agent);
    tl.next_observer += 1;
    let id = tl.next_observer;
    let class = classes(agent).observer;
    let o = agent.host_create_from_new_target(nt, class, None, Box::new(ObserverData { id }))?;
    if let Some(slots) = agent.host_slots_mut(o) {
        slots.clear();
        slots.push(callback);
    }
    Ok(Value::Object(o))
}

fn observer_id(agent: &mut Agent, this: &Value, op: &str) -> JsResult<u64> {
    match agent.host_unwrap::<ObserverData>(this) {
        Some(d) => Ok(d.id),
        None => agent.type_error(&format!(
            "Failed to execute '{op}' on 'PerformanceObserver': Illegal invocation"
        )),
    }
}

/// `observe(options)` (§5.3).
fn observe(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let fail = |m: &str| format!("Failed to execute 'observe' on 'PerformanceObserver': {m}");
    let id = observer_id(agent, &a.this, "observe")?;
    let options = a.arg(0);
    if !matches!(options, Value::Undefined | Value::Null | Value::Object(_)) {
        return agent.type_error(&fail(
            "The provided value is not of type 'PerformanceObserverInit'.",
        ));
    }
    // Los miembros en orden lexicográfico (WebIDL §3.2.17).
    let buffered = member(agent, &options, "buffered")?;
    let duration_threshold = member(agent, &options, "durationThreshold")?;
    let entry_types = member(agent, &options, "entryTypes")?;
    let ty = member(agent, &options, "type")?;
    let buffered_present = !buffered.is_undefined();
    let buffered = bf_js::host::webidl::to_boolean(agent, &buffered);
    let entry_types = if entry_types.is_undefined() {
        None
    } else {
        Some(bf_js::host::webidl::to_sequence(
            agent,
            &entry_types,
            |agent, v| Ok(agent.to_string(v)?.to_string_lossy()),
        )?)
    };
    let ty = if ty.is_undefined() {
        None
    } else {
        Some(agent.to_string(&ty)?.to_string_lossy())
    };
    // 3-4.
    if entry_types.is_none() && ty.is_none() {
        return agent.type_error(&fail(
            "An observe() call must include either entryTypes or type arguments.",
        ));
    }
    if entry_types.is_some()
        && (ty.is_some() || buffered_present || !duration_threshold.is_undefined())
    {
        return agent.type_error(&fail(
            "An observe() call must not include both entryTypes and type arguments.",
        ));
    }
    // 5-6. El tipo del observer no cambia una vez fijado.
    let wanted = if entry_types.is_some() {
        ObserverType::Multiple
    } else {
        ObserverType::Single
    };
    let fixed = *timeline(agent).types.entry(id).or_insert(wanted);
    if fixed != wanted {
        let msg = fail(if fixed == ObserverType::Single {
            "This PerformanceObserver has performed observe({type:...}, therefore it cannot perform observe({entryTypes:...})"
        } else {
            "This PerformanceObserver has performed observe({entryTypes:...}, therefore it cannot perform observe({type:...})"
        });
        return crate::net::dom_error(agent, "InvalidModificationError", &msg);
    }
    let supported = |t: &String| SUPPORTED.contains(&t.as_str());
    let new_types: Vec<String> = match (&entry_types, &ty) {
        (Some(list), _) => list.iter().filter(|t| supported(t)).cloned().collect(),
        (None, Some(t)) if supported(t) => vec![t.clone()],
        _ => Vec::new(),
    };
    // 7.1-7.2: sin tipos soportados, no se hace nada (los navegadores avisan en la consola).
    if new_types.is_empty() {
        return Ok(Value::Undefined);
    }
    // El observer queda registrado (y su objeto, fijado mientras lo esté).
    let registered = timeline(agent).observers.iter().any(|o| o.id == id);
    if !registered {
        let object = agent.host_pin(a.this.clone());
        timeline(agent).observers.push(Registered {
            id,
            object,
            types: Vec::new(),
            buffer: Vec::new(),
            dropped: None,
        });
    }
    let reg = timeline(agent)
        .observers
        .iter_mut()
        .find(|o| o.id == id)
        .expect("registrado");
    match wanted {
        ObserverType::Multiple => reg.types = new_types.clone(),
        ObserverType::Single => {
            let t = new_types[0].clone();
            if !reg.types.contains(&t) {
                reg.types.push(t);
            }
        }
    }
    // 7.3.5: `buffered` copia las entradas ya guardadas de ese tipo.
    if wanted == ObserverType::Single && buffered {
        let t = new_types[0].clone();
        let entries = buffered_entries(agent, &|e| e.entry_type == t);
        let handles: Vec<HostHandle> = entries.into_iter().map(|v| agent.host_pin(v)).collect();
        let reg = timeline(agent)
            .observers
            .iter_mut()
            .find(|o| o.id == id)
            .expect("registrado");
        reg.dropped = Some(0);
        let any = !handles.is_empty();
        reg.buffer.extend(handles);
        if any {
            queue_observer_task(agent);
        }
    }
    Ok(Value::Undefined)
}

/// `disconnect()` (§5.4): deja de observar y vacía su buffer.
fn disconnect(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = observer_id(agent, &a.this, "disconnect")?;
    let tl = timeline(agent);
    let Some(i) = tl.observers.iter().position(|o| o.id == id) else {
        return Ok(Value::Undefined);
    };
    let reg = tl.observers.remove(i);
    agent.host_unpin(reg.object);
    for h in reg.buffer {
        agent.host_unpin(h);
    }
    Ok(Value::Undefined)
}

/// `takeRecords()` (§5.5): el buffer del observer, que queda vacío.
fn take_records(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = observer_id(agent, &a.this, "takeRecords")?;
    let buffer = timeline(agent)
        .observers
        .iter_mut()
        .find(|o| o.id == id)
        .map(|o| std::mem::take(&mut o.buffer))
        .unwrap_or_default();
    let entries: Vec<Value> = buffer
        .into_iter()
        .filter_map(|h| agent.host_unpin(h))
        .collect();
    array(agent, &entries)
}

/// `PerformanceObserver.supportedEntryTypes` (§5.2): siempre el mismo array congelado.
fn supported_entry_types(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    if let Some(h) = timeline(agent).supported
        && let Some(v) = agent.host_pinned(h)
    {
        return Ok(v);
    }
    let values: Vec<Value> = SUPPORTED.iter().map(|t| Value::str(t)).collect();
    let arr = agent.create_array_from_list(&values)?;
    agent.set_integrity_level(arr, true)?;
    let v = Value::Object(arr);
    let h = agent.host_pin(v.clone());
    timeline(agent).supported = Some(h);
    Ok(v)
}

// --- PerformanceObserverEntryList ---

fn entry_list(agent: &mut Agent, entries: &[Value]) -> JsResult<Value> {
    let class = classes(agent).entry_list;
    let arr = agent.create_array_from_list(entries)?;
    let o = agent.host_create(class, Box::new(EntryListData));
    if let Some(slots) = agent.host_slots_mut(o) {
        slots.clear();
        slots.push(Value::Object(arr));
    }
    Ok(Value::Object(o))
}

/// Las entradas de la lista que cumplen `keep`, en orden cronológico.
fn list_filter(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    keep: &dyn Fn(&Entry) -> bool,
) -> JsResult<Value> {
    if agent.host_unwrap::<EntryListData>(&a.this).is_none() {
        return agent.type_error("Illegal invocation");
    }
    let Value::Object(o) = &a.this else {
        return agent.type_error("Illegal invocation");
    };
    let arr = agent
        .host_slots_mut(*o)
        .and_then(|s| s.first().cloned())
        .unwrap_or_default();
    let Value::Object(arr) = arr else {
        return array(agent, &[]);
    };
    let len = agent.length_of_array_like(arr)?;
    let mut out: Vec<(f64, Value)> = Vec::new();
    for i in 0..len as u32 {
        let v = agent.get(arr, &PropertyKey::from(i), Value::Object(arr))?;
        if let Some(e) = agent.host_unwrap::<Entry>(&v)
            && keep(e)
        {
            out.push((e.start, v));
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    let values: Vec<Value> = out.into_iter().map(|(_, v)| v).collect();
    array(agent, &values)
}

fn list_get_entries(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    list_filter(agent, a, &|_| true)
}

fn list_get_entries_by_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let ty = agent.to_string(&a.arg(0))?.to_string_lossy();
    list_filter(agent, a, &|e| e.entry_type == ty)
}

fn list_get_entries_by_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = agent.to_string(&a.arg(0))?.to_string_lossy();
    let ty = if a.arg(1).is_undefined() {
        None
    } else {
        Some(agent.to_string(&a.arg(1))?.to_string_lossy())
    };
    list_filter(agent, a, &|e| {
        e.name == name && ty.as_deref().is_none_or(|t| e.entry_type == t)
    })
}

// --- Resource Timing ---

/// Anota un recurso cargado por la ventana en curso (Resource Timing §4.5, *mark resource
/// timing*): `url`, quién lo pidió (`img`, `fetch`, `xmlhttprequest`…), cuándo empezó y
/// terminó (en el reloj de `performance.now()`, ver [`now_ms`]) y el tamaño del cuerpo.
pub fn add_resource_entry(
    agent: &mut Agent,
    url: &str,
    initiator: &str,
    start: f64,
    end: f64,
    body_size: usize,
) {
    if agent.embedder_data::<TimelineClasses>().is_none() {
        return;
    }
    let realm = WebContextHandle::current(agent).realm;
    if agent.realm_data::<BomState>(realm).is_none() {
        return;
    }
    let times = ResourceTimes {
        initiator_type: initiator.to_owned(),
        fetch_start: start,
        response_start: end,
        response_end: end,
        body_size: body_size as f64,
        ..ResourceTimes::default()
    };
    let e = Entry {
        name: url.to_owned(),
        entry_type: "resource",
        start,
        duration: (end - start).max(0.0),
        times: Some(times),
    };
    let class = classes(agent).resource;
    let v = create_entry(agent, class, e, None);
    add_to_buffer(agent, v, true);
}

/// `performance.now()` de la ventana en curso (`None` sin BOM, p. ej. en un worker).
pub fn now_ms(agent: &mut Agent) -> Option<f64> {
    let realm = WebContextHandle::current(agent).realm;
    agent.realm_data::<BomState>(realm)?;
    Some(now(agent))
}
