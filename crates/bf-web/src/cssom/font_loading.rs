//! CSS Font Loading 3 (https://drafts.csswg.org/css-font-loading/): `FontFace`,
//! `FontFaceSet`, `FontFaceSetLoadEvent` y `document.fonts`. **E5.**
//!
//! Las interfaces están en `font_loading.js` (evaluado una vez por realm, como Cache e
//! IndexedDB); aquí están sus funciones nativas:
//!
//! - `parseLonghand(propiedad, valor)` y `parseFont(texto)`: validan descriptores y el
//!   argumento de `load()`/`check()` con los parsers de CSS del motor (CSSOM, `expand`); `null`
//!   si no es válido, si es una palabra clave global o si usa `var()` (§3.5.1).
//! - `parseSrc(texto, base)`: el valor de `src` como lista `url:<absoluta>` / `local:<nombre>`
//!   (con el parser de `@font-face` de `bf-font`).
//! - `fetchFont(lista)`: descarga (con el cargador de la página, de forma síncrona en la tarea
//!   del script) la primera fuente de la lista que sea un formato de fuente.
//! - `setFaces(css)`: deja en el documento las caras añadidas al conjunto desde scripts como
//!   un bloque `@font-face`, marcado con [`bf_style::loader::SCRIPT_FONT_FACES`], entre los
//!   textos adoptados de la raíz del documento. `document_stylesheets` lo lee, así que la
//!   maqueta de la ventana y la de `bf-paint` usan esas caras como las de cualquier hoja (y
//!   cambiarlo sube la versión del documento, lo que pide una maqueta nueva).
//!
//! # Cuándo se resuelve `ready`
//!
//! Las fuentes de `@font-face` se cargan de forma síncrona en la maqueta que las usa (`Fonts::
//! load_web`), así que no hay cargas de fondo que esperar: `ready` se resuelve en una tarea
//! cuando no queda ninguna cara del conjunto cargando y el documento ya disparó `load` (CSS
//! Font Loading 3 §4.2, "pending on the environment"). La maqueta siguiente, la que se pinta,
//! ya usa las caras.

use bf_css::syntax::{parse_component_value_list, parse_declaration_list};
use bf_infra::{ResourceLoader, Url};
use bf_js::host::TaskSource;
use bf_js::value::Value;
use bf_js::vm::function::{CallArgs, NativeFn};
use bf_js::vm::{Agent, JsResult, RealmId};
use bf_style::loader::SCRIPT_FONT_FACES;
use bf_style::properties::CssWide;

use crate::WebContextHandle;

const FONT_LOADING_JS: &str = include_str!("font_loading.js");

/// Evalúa `font_loading.js` en `realm` (la ventana) con sus funciones nativas.
pub fn install(agent: &mut Agent, realm: RealmId) {
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let native = agent.new_plain_object();
    let fns: [(&str, u32, NativeFn); 6] = [
        ("queueTask", 1, queue_task),
        ("parseLonghand", 2, parse_longhand),
        ("parseFont", 1, parse_font),
        ("parseSrc", 2, parse_src),
        ("fetchFont", 1, fetch_font),
        ("setFaces", 1, set_faces),
    ];
    for (name, len, f) in fns {
        bf_js::builtins::method(agent, native, name, len, f);
    }
    let g = Value::Object(agent.realms[realm].global_object);
    if let Ok(f) = agent.eval_script(FONT_LOADING_JS, "bf-web:font_loading.js") {
        let _ = agent.call(&f, Value::Undefined, &[g, Value::Object(native)]);
    }
    agent.current_realm = saved;
}

fn string_arg(agent: &mut Agent, a: &CallArgs<'_>, i: usize) -> JsResult<String> {
    Ok(agent.to_string(&a.arg(i))?.to_string())
}

fn queue_task(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    agent.queue_callback_task(TaskSource::Other, a.arg(0), Value::Undefined, Vec::new());
    Ok(Value::Undefined)
}

/// El valor de la longhand `name` serializado, o `None` si `value` no es válido para ella, es
/// una palabra clave global o usa `var()`.
fn longhand(name: &str, value: &str) -> Option<String> {
    let values = parse_component_value_list(value);
    if CssWide::parse(bf_style::values::trim(&values)).is_some()
        || bf_style::cascade::contains_var(&values)
    {
        return None;
    }
    super::declaration::expand(name, &values, None)
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v)
}

fn parse_longhand(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let name = string_arg(agent, a, 0)?;
    let value = string_arg(agent, a, 1)?;
    Ok(longhand(&name, &value).map_or(Value::Null, |v| Value::str(&v)))
}

/// Los nombres de familia de un valor de `font-family` serializado, sin comillas.
fn family_names(list: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = list.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(_), '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => quote = Some(c),
            (None, ',') => {
                out.push(cur.split_whitespace().collect::<Vec<_>>().join(" "));
                cur.clear();
            }
            (None, c) => cur.push(c),
        }
    }
    out.push(cur.split_whitespace().collect::<Vec<_>>().join(" "));
    out.retain(|n| !n.is_empty());
    out
}

/// `parseFont(texto)`: las familias del shorthand `font` (§3.5.1, "find the matching font
/// faces", paso 1), o `null`.
fn parse_font(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let text = string_arg(agent, a, 0)?;
    let values = parse_component_value_list(&text);
    if CssWide::parse(bf_style::values::trim(&values)).is_some()
        || bf_style::cascade::contains_var(&values)
    {
        return Ok(Value::Null);
    }
    let expanded = super::declaration::expand("font", &values, None);
    let Some((_, families)) = expanded.iter().find(|(n, _)| n == "font-family") else {
        return Ok(Value::Null);
    };
    let names: Vec<Value> = family_names(families)
        .iter()
        .map(|n| Value::str(n))
        .collect();
    Ok(Value::Object(agent.create_array_from_list(&names)?))
}

/// `parseSrc(texto, base)`: las fuentes de un valor de `src` (CSS Fonts 4 §4.3) con las URLs
/// resueltas contra `base`, o `null` si no hay ninguna válida.
fn parse_src(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let text = string_arg(agent, a, 0)?;
    let base = string_arg(agent, a, 1)?;
    let base = Url::parse(&base, None)
        .ok()
        .or_else(|| Some(WebContextHandle::current(agent).get(agent).base.clone()));
    let decls = parse_declaration_list(&format!("font-family: x; src: {text}"));
    let Some(rule) = bf_font::parse_font_face(&decls) else {
        return Ok(Value::Null);
    };
    let mut out = Vec::new();
    for s in rule.sources {
        match s {
            bf_font::Source::Url { url, .. } => {
                if let Ok(u) = Url::parse(&url, base.as_ref()) {
                    out.push(Value::str(&format!("url:{}", u.serialize(false))));
                }
            }
            bf_font::Source::Local(name) => out.push(Value::str(&format!("local:{name}"))),
        }
    }
    if out.is_empty() {
        return Ok(Value::Null);
    }
    Ok(Value::Object(agent.create_array_from_list(&out)?))
}

/// ¿Empiezan los datos como un formato de fuente (TrueType, OpenType, colección, WOFF, WOFF2)?
fn is_font_data(data: &[u8]) -> bool {
    matches!(
        data.get(0..4),
        Some([0, 1, 0, 0] | b"OTTO" | b"true" | b"ttcf" | b"wOFF" | b"wOF2")
    )
}

/// `fetchFont(lista)`: ¿se puede cargar alguna de las fuentes (`url:`/`local:`)?
fn fetch_font(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Value::Object(list) = a.arg(0) else {
        return Ok(Value::Boolean(false));
    };
    let len = agent.get_str(list, "length")?.as_number().unwrap_or(0.0) as usize;
    let realm = WebContextHandle::current(agent).realm;
    let loader = agent
        .realm_data::<crate::html::scripting::PageResources>(realm)
        .map(|r| r.loader.clone());
    for i in 0..len {
        let item = agent.get_str(list, &i.to_string())?;
        let item = agent.to_string(&item)?.to_string();
        if let Some(url) = item.strip_prefix("url:") {
            let Ok(url) = Url::parse(url, None) else {
                continue;
            };
            // Las URL `data:` no necesitan cargador.
            let data = match &loader {
                Some(l) => l.load(&url).map(|r| r.bytes),
                None => bf_infra::FileLoader::default().load(&url).map(|r| r.bytes),
            };
            if data.is_some_and(|d| is_font_data(&d)) {
                return Ok(Value::Boolean(true));
            }
        } else if let Some(name) = item.strip_prefix("local:")
            && bf_font::FontDb::system().by_name(name).is_some()
        {
            return Ok(Value::Boolean(true));
        }
    }
    Ok(Value::Boolean(false))
}

/// `setFaces(css)`: el bloque `@font-face` de las caras añadidas desde scripts (vacío: ninguna).
fn set_faces(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let css = string_arg(agent, a, 0)?;
    let ctx = WebContextHandle::current(agent).get_mut(agent);
    let doc = ctx.document_mut();
    let root = doc.root();
    let current = doc.adopted_style_texts(root).to_vec();
    let mut texts: Vec<String> = current
        .iter()
        .filter(|t| !t.starts_with(SCRIPT_FONT_FACES))
        .cloned()
        .collect();
    if !css.is_empty() {
        texts.push(format!("{SCRIPT_FONT_FACES}\n{css}"));
    }
    if texts != current {
        doc.set_adopted_style_texts(root, texts);
    }
    Ok(Value::Undefined)
}

#[cfg(test)]
mod tests {
    use super::family_names;

    #[test]
    fn families() {
        assert_eq!(
            family_names("\"Foo Bar\", serif, 'a\\'b'"),
            vec!["Foo Bar", "serif", "a'b"]
        );
        assert_eq!(family_names("Ahem"), vec!["Ahem"]);
    }
}
