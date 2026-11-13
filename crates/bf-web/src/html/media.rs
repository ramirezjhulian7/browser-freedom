//! E1: `HTMLMediaElement` (HTML §4.8.11) sin reproducción: el motor no decodifica audio ni
//! video, así que un elemento multimedia se queda en `NETWORK_EMPTY`/`HAVE_NOTHING`, sin
//! duración (`NaN`), en pausa, con rangos `TimeRanges` vacíos, `canPlayType` vacío y `play()`
//! rechazado con `NotSupportedError` (como un navegador sin fuentes soportadas); con `src` o
//! hijos `source`, `networkState` es `NETWORK_NO_SOURCE`. Los atributos
//! de estado que script puede cambiar (`currentTime`, `volume`, `muted`, `playbackRate`,
//! `defaultPlaybackRate`, `preservesPitch`, `srcObject`) se guardan por elemento.
//!
//! También `TimeRanges` (§4.8.11.10) y `MediaError` (§4.8.11.1).

use std::collections::HashMap;

use bf_dom::DomException;
use bf_js::host::{HostClass, HostClassId, HostHandle, webidl};
use bf_js::string::JsString;
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use crate::dom::{this_node, throw_dom};
use crate::{NodeRef, WebContextHandle};

/// El estado de un elemento multimedia.
#[derive(Debug, Clone)]
struct MediaState {
    current_time: f64,
    volume: f64,
    muted: bool,
    playback_rate: f64,
    default_playback_rate: f64,
    preserves_pitch: bool,
    paused: bool,
    src_object: Option<HostHandle>,
}

impl Default for MediaState {
    fn default() -> MediaState {
        MediaState {
            current_time: 0.0,
            volume: 1.0,
            muted: false,
            playback_rate: 1.0,
            default_playback_rate: 1.0,
            preserves_pitch: true,
            paused: true,
            src_object: None,
        }
    }
}

#[derive(Debug, Default)]
struct MediaStates(HashMap<NodeRef, MediaState>);

/// Las clases `TimeRanges` y `MediaError` (dato del embebedor).
struct MediaClasses {
    time_ranges: HostClassId,
}

/// El payload de un `TimeRanges`: sus rangos normalizados.
struct Ranges(Vec<(f64, f64)>);

fn media_state(agent: &Agent, n: NodeRef) -> MediaState {
    let realm = WebContextHandle::current(agent).realm;
    agent
        .realm_data::<MediaStates>(realm)
        .and_then(|s| s.0.get(&n).cloned())
        .unwrap_or_default()
}

fn media_state_mut(agent: &mut Agent, n: NodeRef) -> &mut MediaState {
    let realm = WebContextHandle::current(agent).realm;
    if agent.realm_data::<MediaStates>(realm).is_none() {
        agent.set_realm_data(realm, MediaStates::default());
    }
    agent
        .realm_data_mut::<MediaStates>(realm)
        .expect("MediaStates")
        .0
        .entry(n)
        .or_default()
}

/// Registra `TimeRanges` y `MediaError` (la primera vez).
pub fn register(agent: &mut Agent) {
    if agent.embedder_data::<MediaClasses>().is_some() {
        return;
    }
    let time_ranges = agent.register_host_class(
        HostClass::new("TimeRanges")
            .attribute("length", Some(ranges_length), None)
            .method("start", 1, ranges_start)
            .method("end", 1, ranges_end),
    );
    agent.register_host_class(
        HostClass::new("MediaError")
            .constant("MEDIA_ERR_ABORTED", 1.0)
            .constant("MEDIA_ERR_NETWORK", 2.0)
            .constant("MEDIA_ERR_DECODE", 3.0)
            .constant("MEDIA_ERR_SRC_NOT_SUPPORTED", 4.0)
            .attribute("code", Some(error_code), None)
            .attribute("message", Some(error_message), None),
    );
    agent.set_embedder_data(MediaClasses { time_ranges });
}

/// Lo propio de `HTMLMediaElement`.
pub fn extend(name: &str, class: HostClass) -> HostClass {
    if name != "HTMLMediaElement" {
        return class;
    }
    class
        .constant("NETWORK_EMPTY", 0.0)
        .constant("NETWORK_IDLE", 1.0)
        .constant("NETWORK_LOADING", 2.0)
        .constant("NETWORK_NO_SOURCE", 3.0)
        .constant("HAVE_NOTHING", 0.0)
        .constant("HAVE_METADATA", 1.0)
        .constant("HAVE_CURRENT_DATA", 2.0)
        .constant("HAVE_FUTURE_DATA", 3.0)
        .constant("HAVE_ENOUGH_DATA", 4.0)
        .attribute("error", Some(null), None)
        .attribute("srcObject", Some(src_object), Some(set_src_object))
        .attribute("currentSrc", Some(empty_string), None)
        .attribute("networkState", Some(network_state), None)
        .attribute("buffered", Some(empty_ranges), None)
        .method("load", 0, load)
        .method("canPlayType", 1, can_play_type)
        .attribute("readyState", Some(zero), None)
        .attribute("seeking", Some(false_value), None)
        .attribute("currentTime", Some(current_time), Some(set_current_time))
        .method("fastSeek", 1, fast_seek)
        .attribute("duration", Some(nan), None)
        .method("getStartDate", 0, get_start_date)
        .attribute("paused", Some(paused), None)
        .attribute(
            "defaultPlaybackRate",
            Some(default_playback_rate),
            Some(set_default_playback_rate),
        )
        .attribute("playbackRate", Some(playback_rate), Some(set_playback_rate))
        .attribute(
            "preservesPitch",
            Some(preserves_pitch),
            Some(set_preserves_pitch),
        )
        .attribute("played", Some(empty_ranges), None)
        .attribute("seekable", Some(empty_ranges), None)
        .attribute("ended", Some(false_value), None)
        .method("play", 0, play)
        .method("pause", 0, pause)
        .attribute("volume", Some(volume), Some(set_volume))
        .attribute("muted", Some(muted), Some(set_muted))
        .method("addTextTrack", 1, add_text_track)
}

fn null(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Null)
}

fn zero(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(0.0))
}

/// `networkState`: `NETWORK_EMPTY` sin `src` ni hijos `source`; con alguno, el "resource
/// selection algorithm" termina sin fuente soportada (`NETWORK_NO_SOURCE`, §4.8.11.5), y eso
/// ya vale durante la inserción del `source`, antes de los post-insertion steps.
fn network_state(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let has_source = super::elements::attr_value(agent, n, "src").is_some()
        || crate::with_doc(agent, n.doc, |d| {
            d.children(n.node)
                .any(|c| d.element(c).is_some_and(|e| e.is_html("source")))
        });
    Ok(Value::Number(if has_source { 3.0 } else { 0.0 }))
}

fn nan(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(f64::NAN))
}

fn false_value(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(false))
}

fn empty_string(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::String(JsString::from("")))
}

/// `buffered`, `played`, `seekable`: un `TimeRanges` nuevo y vacío (no hay datos).
fn empty_ranges(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_node(agent, a)?;
    let Some(class) = agent.embedder_data::<MediaClasses>().map(|c| c.time_ranges) else {
        return Ok(Value::Null);
    };
    Ok(Value::Object(
        agent.host_create(class, Box::new(Ranges(Vec::new()))),
    ))
}

fn ranges_of<'a>(agent: &'a Agent, a: &CallArgs<'_>) -> Option<&'a Ranges> {
    agent.host_unwrap::<Ranges>(&a.this)
}

fn ranges_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    match ranges_of(agent, a) {
        Some(r) => Ok(Value::Number(r.0.len() as f64)),
        None => agent.type_error("Illegal invocation"),
    }
}

/// `start(i)`/`end(i)`: `IndexSizeError` fuera de rango (§4.8.11.10).
fn range_bound(agent: &mut Agent, a: &CallArgs<'_>, end: bool) -> JsResult<Value> {
    crate::dom::events::required(agent, a, 1, if end { "end" } else { "start" })?;
    let i = webidl::to_unsigned_long(agent, &a.arg(0))? as usize;
    let r = match ranges_of(agent, a) {
        Some(r) => r.0.get(i).copied(),
        None => return agent.type_error("Illegal invocation"),
    };
    match r {
        Some((s, e)) => Ok(Value::Number(if end { e } else { s })),
        None => throw_dom(agent, DomException::IndexSizeError),
    }
}

fn ranges_start(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    range_bound(agent, a, false)
}

fn ranges_end(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    range_bound(agent, a, true)
}

/// `MediaError.code`/`message`: sin carga de medios no hay instancias (`error` es `null`), así
/// que solo los ve una llamada con otro `this`.
fn error_code(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    agent.type_error("Illegal invocation")
}

fn error_message(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    agent.type_error("Illegal invocation")
}

fn src_object(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let h = media_state(agent, n).src_object;
    Ok(h.and_then(|h| agent.host_pinned(h)).unwrap_or(Value::Null))
}

fn set_src_object(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = a.arg(0);
    let new = if v.is_nullish() {
        None
    } else {
        Some(agent.host_pin(v))
    };
    if let Some(old) = std::mem::replace(&mut media_state_mut(agent, n).src_object, new) {
        agent.host_unpin(old);
    }
    Ok(Value::Undefined)
}

/// `load()`: el "media element load algorithm" sin fuentes: vuelve al estado inicial.
fn load(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let st = media_state_mut(agent, n);
    st.current_time = 0.0;
    st.paused = true;
    st.playback_rate = st.default_playback_rate;
    Ok(Value::Undefined)
}

/// `canPlayType(type)`: ningún tipo se puede reproducir (`""`).
fn can_play_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_node(agent, a)?;
    crate::dom::events::required(agent, a, 1, "canPlayType")?;
    webidl::to_dom_string(agent, &a.arg(0))?;
    Ok(Value::String(JsString::from("")))
}

fn current_time(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Number(media_state(agent, n).current_time))
}

fn set_current_time(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let t = webidl::to_double(agent, &a.arg(0))?;
    media_state_mut(agent, n).current_time = t;
    Ok(Value::Undefined)
}

fn fast_seek(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    crate::dom::events::required(agent, a, 1, "fastSeek")?;
    let t = webidl::to_double(agent, &a.arg(0))?;
    media_state_mut(agent, n).current_time = t;
    Ok(Value::Undefined)
}

/// `getStartDate()`: sin timeline, un `Date` inválido (§4.8.11.6).
fn get_start_date(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_node(agent, a)?;
    let ctor = agent.realms[agent.current_realm].intrinsics.date_ctor;
    agent.construct(ctor, &[Value::Number(f64::NAN)], None)
}

fn paused(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Boolean(media_state(agent, n).paused))
}

macro_rules! rate_accessors {
    ($get:ident, $set:ident, $field:ident) => {
        fn $get(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
            let n = this_node(agent, a)?;
            Ok(Value::Number(media_state(agent, n).$field))
        }

        fn $set(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
            let n = this_node(agent, a)?;
            let v = webidl::to_double(agent, &a.arg(0))?;
            media_state_mut(agent, n).$field = v;
            Ok(Value::Undefined)
        }
    };
}

rate_accessors!(playback_rate, set_playback_rate, playback_rate);
rate_accessors!(
    default_playback_rate,
    set_default_playback_rate,
    default_playback_rate
);

fn preserves_pitch(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Boolean(media_state(agent, n).preserves_pitch))
}

fn set_preserves_pitch(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let b = webidl::to_boolean(agent, &a.arg(0));
    media_state_mut(agent, n).preserves_pitch = b;
    Ok(Value::Undefined)
}

/// `play()`: sin fuentes soportadas, la promesa se rechaza con `NotSupportedError`.
fn play(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_node(agent, a)?;
    let p = agent.new_internal_promise();
    let e = crate::dom::exception::create(
        agent,
        DomException::NotSupportedError.name(),
        "The element has no supported sources.",
    );
    agent.reject_promise(p, e);
    Ok(Value::Object(p))
}

fn pause(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    media_state_mut(agent, n).paused = true;
    Ok(Value::Undefined)
}

fn volume(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Number(media_state(agent, n).volume))
}

/// `volume`: fuera de [0, 1], `IndexSizeError`.
fn set_volume(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let v = webidl::to_double(agent, &a.arg(0))?;
    if !(0.0..=1.0).contains(&v) {
        return throw_dom(agent, DomException::IndexSizeError);
    }
    media_state_mut(agent, n).volume = v;
    Ok(Value::Undefined)
}

fn muted(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(Value::Boolean(media_state(agent, n).muted))
}

fn set_muted(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let b = webidl::to_boolean(agent, &a.arg(0));
    media_state_mut(agent, n).muted = b;
    Ok(Value::Undefined)
}

/// `addTextTrack(kind, label, language)`: valida `kind` (`TextTrackKind`); las pistas de texto
/// todavía no existen (`NotSupportedError`).
fn add_text_track(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_node(agent, a)?;
    crate::dom::events::required(agent, a, 1, "addTextTrack")?;
    let kind = webidl::to_dom_string(agent, &a.arg(0))?.to_string_lossy();
    if !matches!(
        kind.as_str(),
        "subtitles" | "captions" | "descriptions" | "chapters" | "metadata"
    ) {
        return agent.type_error(&format!(
            "The provided value '{kind}' is not a valid enum value of type TextTrackKind."
        ));
    }
    throw_dom(agent, DomException::NotSupportedError)
}
