//! `Navigator` (HTML §8.9.1) y `NavigatorUAData` (User-Agent Client Hints §3): los mismos
//! datos que da Chrome, con Freedom como producto y como motor (ver
//! [`bf_infra::identity`]). El `User-Agent` sale de los hooks del motor.

use bf_infra::identity;
use bf_js::heap::Gc;
use bf_js::host::HostClass;
use bf_js::object::Object;
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use super::{bom_classes, state};

/// Los slots del objeto `navigator` (valores que deben ser siempre el mismo objeto).
const SLOT_LANGUAGES: usize = 0;
const SLOT_UA_DATA: usize = 1;
const SLOT_PLUGINS: usize = 2;
const SLOT_MIME_TYPES: usize = 3;

pub fn navigator_class() -> HostClass {
    HostClass::new("Navigator")
        .attribute("userAgent", Some(user_agent), None)
        .attribute("appVersion", Some(app_version), None)
        .attribute("appName", Some(app_name), None)
        .attribute("appCodeName", Some(app_code_name), None)
        .attribute("product", Some(product), None)
        .attribute("productSub", Some(product_sub), None)
        .attribute("vendor", Some(vendor), None)
        .attribute("vendorSub", Some(empty), None)
        .attribute("platform", Some(platform), None)
        .attribute("language", Some(language), None)
        .attribute("languages", Some(languages), None)
        .attribute("onLine", Some(yes), None)
        .attribute("cookieEnabled", Some(cookie_enabled), None)
        .attribute("hardwareConcurrency", Some(hardware_concurrency), None)
        .attribute("deviceMemory", Some(device_memory), None)
        .attribute("maxTouchPoints", Some(zero), None)
        .attribute("doNotTrack", Some(null), None)
        .attribute("webdriver", Some(no), None)
        // Freedom no tiene visor de PDF: `plugins` y `mimeTypes` quedan vacíos (HTML §8.9.1.6).
        .attribute("pdfViewerEnabled", Some(no), None)
        .attribute("userAgentData", Some(user_agent_data), None)
        .attribute("plugins", Some(plugins), None)
        .attribute("mimeTypes", Some(mime_types), None)
        .method("javaEnabled", 0, no)
        .method("sendBeacon", 1, yes)
}

pub fn ua_data_class() -> HostClass {
    HostClass::new("NavigatorUAData")
        .attribute("brands", Some(brands), None)
        .attribute("mobile", Some(no), None)
        .attribute("platform", Some(ua_platform), None)
        .method("getHighEntropyValues", 1, get_high_entropy_values)
        .method("toJSON", 0, ua_to_json)
}

/// `PluginArray` y `MimeTypeArray` (HTML §8.9.1.6): colecciones de solo lectura con los
/// visores de PDF.
pub fn plugin_array_class(name: &'static str) -> HostClass {
    HostClass::new(name)
        .attribute("length", Some(collection_length), None)
        .method("item", 1, collection_item)
        .method("namedItem", 1, collection_named_item)
        .method("refresh", 0, undefined)
}

/// El `User-Agent` del navegador.
fn ua(agent: &Agent) -> String {
    agent.host.hooks.user_agent()
}

fn user_agent(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(&ua(agent)))
}

/// `appVersion`: el `User-Agent` sin el `Mozilla/` del principio.
fn app_version(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let ua = ua(agent);
    Ok(Value::str(ua.strip_prefix("Mozilla/").unwrap_or(&ua)))
}

fn app_name(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str("Netscape"))
}

fn app_code_name(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str("Mozilla"))
}

fn product(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str("Gecko"))
}

/// `productSub` (HTML §8.9.1.1): uno de los dos valores de compatibilidad del estándar.
fn product_sub(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str("20030107"))
}

/// `vendor` (HTML §8.9.1.1): la cadena vacía (las otras opciones del estándar son las de
/// Apple y Google).
fn vendor(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(""))
}

fn platform(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(identity::NAVIGATOR_PLATFORM))
}

fn empty(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(""))
}

fn yes(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(true))
}

fn no(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(false))
}

fn zero(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(0.0))
}

fn null(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Null)
}

fn undefined(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Undefined)
}

fn language(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let langs = state(agent).hooks.languages();
    Ok(Value::str(langs.first().map_or("en-US", String::as_str)))
}

/// Un valor guardado en el slot `i` del objeto `this` (o creado con `make` la primera vez).
fn cached(
    agent: &mut Agent,
    this: &Value,
    i: usize,
    make: impl FnOnce(&mut Agent) -> JsResult<Value>,
) -> JsResult<Value> {
    let Value::Object(o) = this else {
        return make(agent);
    };
    if let Some(v) = agent
        .host_slots_mut(*o)
        .and_then(|s| s.get(i).cloned())
        .filter(|v| !v.is_undefined())
    {
        return Ok(v);
    }
    let v = make(agent)?;
    if let Some(slots) = agent.host_slots_mut(*o) {
        while slots.len() <= i {
            slots.push(Value::Undefined);
        }
        slots[i] = v.clone();
    }
    Ok(v)
}

/// Un array congelado de strings.
fn frozen_strings(agent: &mut Agent, items: &[String]) -> JsResult<Value> {
    let values: Vec<Value> = items.iter().map(|s| Value::str(s)).collect();
    let a = agent.create_array_from_list(&values)?;
    agent.set_integrity_level(a, true)?;
    Ok(Value::Object(a))
}

/// `navigator.languages`: el mismo array congelado mientras no cambien los idiomas.
fn languages(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let langs = state(agent).hooks.languages();
    cached(agent, &a.this, SLOT_LANGUAGES, |agent| {
        frozen_strings(agent, &langs)
    })
}

fn cookie_enabled(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(state(agent).hooks.cookies_enabled()))
}

fn hardware_concurrency(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(f64::from(
        state(agent).hooks.hardware_concurrency(),
    )))
}

/// `navigator.deviceMemory` (Device Memory §2.1): GiB redondeado a una potencia de 2, hasta 8.
fn device_memory(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(8.0))
}

fn user_agent_data(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    cached(agent, &a.this, SLOT_UA_DATA, |agent| {
        let class = bom_classes(agent).ua_data;
        Ok(Value::Object(agent.host_create(class, Box::new(()))))
    })
}

/// Un objeto plano congelado con propiedades string.
fn frozen_record(agent: &mut Agent, fields: &[(&str, Value)]) -> JsResult<Gc<Object>> {
    let o = agent.new_plain_object();
    for (k, v) in fields {
        agent.put_data(o, PropertyKey::from(*k), v.clone(), true, true, true);
    }
    agent.set_integrity_level(o, true)?;
    Ok(o)
}

/// Una lista de marcas (`brands` o `fullVersionList`) como array congelado de
/// `{brand, version}`.
fn brand_list(agent: &mut Agent, list: &[(&str, &str)]) -> JsResult<Value> {
    let mut values = Vec::new();
    for &(brand, version) in list {
        let o = frozen_record(
            agent,
            &[
                ("brand", Value::str(brand)),
                ("version", Value::str(version)),
            ],
        )?;
        values.push(Value::Object(o));
    }
    let a = agent.create_array_from_list(&values)?;
    agent.set_integrity_level(a, true)?;
    Ok(Value::Object(a))
}

fn brands(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    cached(agent, &a.this, 0, |agent| {
        brand_list(agent, &identity::brands())
    })
}

fn ua_platform(_: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(identity::UA_PLATFORM))
}

/// `toJSON()`: `{brands, mobile, platform}`.
fn ua_to_json(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let b = brands(agent, a)?;
    let o = agent.new_plain_object();
    agent.put_data(o, PropertyKey::from("brands"), b, true, true, true);
    agent.put_data(
        o,
        PropertyKey::from("mobile"),
        Value::Boolean(false),
        true,
        true,
        true,
    );
    agent.put_data(
        o,
        PropertyKey::from("platform"),
        Value::str(identity::UA_PLATFORM),
        true,
        true,
        true,
    );
    Ok(Value::Object(o))
}

/// `getHighEntropyValues(hints)` (UA-CH §3.3): una promesa con los valores de baja entropía y
/// los pedidos.
fn get_high_entropy_values(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let mut hints: Vec<String> = Vec::new();
    if let Value::Object(list) = a.arg(0) {
        let len = agent.get_str(list, "length")?;
        let n = agent.to_number(&len)? as usize;
        for i in 0..n.min(64) {
            let v = agent.get(list, &PropertyKey::from(i.to_string().as_str()), a.arg(0))?;
            hints.push(super::to_rust_string(agent, &v)?);
        }
    }
    let o = agent.new_plain_object();
    let b = brand_list(agent, &identity::brands())?;
    agent.put_data(o, PropertyKey::from("brands"), b, true, true, true);
    agent.put_data(
        o,
        PropertyKey::from("mobile"),
        Value::Boolean(false),
        true,
        true,
        true,
    );
    agent.put_data(
        o,
        PropertyKey::from("platform"),
        Value::str(identity::UA_PLATFORM),
        true,
        true,
        true,
    );
    for hint in &hints {
        let v = match hint.as_str() {
            "architecture" => Value::str(identity::ARCHITECTURE),
            "bitness" => Value::str(identity::BITNESS),
            "model" => Value::str(""),
            // La versión exacta del sistema no se expone (el estándar permite la cadena vacía).
            "platformVersion" => Value::str(""),
            "uaFullVersion" => Value::str(identity::VERSION),
            "wow64" => Value::Boolean(false),
            "fullVersionList" => brand_list(agent, &identity::full_version_list())?,
            "formFactors" => frozen_strings(agent, &["Desktop".to_owned()])?,
            _ => continue,
        };
        agent.put_data(o, PropertyKey::from(hint.as_str()), v, true, true, true);
    }
    let ctor = agent.intrinsics().promise_ctor;
    agent.promise_resolve(ctor, Value::Object(o))
}

/// `PluginArray`/`MimeTypeArray` con sus entradas como propiedades indexadas y con nombre.
fn collection(agent: &mut Agent, mime: bool) -> JsResult<Value> {
    let c = bom_classes(agent).clone();
    let class = if mime {
        c.mime_type_array
    } else {
        c.plugin_array
    };
    // Sin visor de PDF no hay plugins ni tipos MIME que listar (HTML §8.9.1.6).
    let entries: Vec<(String, Value)> = Vec::new();
    let o = agent.host_create(class, Box::new(entries.len()));
    for (i, (name, v)) in entries.iter().enumerate() {
        agent.put_data(
            o,
            PropertyKey::from(i.to_string().as_str()),
            v.clone(),
            false,
            true,
            true,
        );
        agent.put_data(
            o,
            PropertyKey::from(name.as_str()),
            v.clone(),
            false,
            false,
            true,
        );
    }
    if let Some(slots) = agent.host_slots_mut(o) {
        slots.extend(entries.into_iter().map(|(_, v)| v));
    }
    Ok(Value::Object(o))
}

fn plugins(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    cached(agent, &a.this, SLOT_PLUGINS, |agent| {
        collection(agent, false)
    })
}

fn mime_types(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    cached(agent, &a.this, SLOT_MIME_TYPES, |agent| {
        collection(agent, true)
    })
}

fn collection_entries(agent: &mut Agent, this: &Value) -> Vec<Value> {
    match this {
        Value::Object(o) => agent.host_slots_mut(*o).cloned().unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn collection_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Number(
        collection_entries(agent, &a.this).len() as f64
    ))
}

fn collection_item(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let i = agent.to_number(&a.arg(0))?;
    let entries = collection_entries(agent, &a.this);
    if i.is_finite() && i >= 0.0 {
        return Ok(entries.get(i as usize).cloned().unwrap_or(Value::Null));
    }
    Ok(Value::Null)
}

fn collection_named_item(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = super::to_rust_string(agent, &a.arg(0))?;
    for v in collection_entries(agent, &a.this) {
        if let Value::Object(o) = v {
            for key in ["name", "type"] {
                let k = agent.get_str(o, key)?;
                if !k.is_undefined() && super::to_rust_string(agent, &k)? == name {
                    return Ok(Value::Object(o));
                }
            }
        }
    }
    Ok(Value::Null)
}
