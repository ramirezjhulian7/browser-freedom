//! Intl.ListFormat (ECMA-402 §13) con los `listPatterns` de CLDR de `en` y `es`.

use crate::builtins::intl::{
    IntlData, cldr, create_intl_object, get_string_option, intl_constructor, intl_data, locale,
};
use crate::builtins::web::str_value;
use crate::heap::Gc;
use crate::object::Object;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

/// Los slots de un Intl.ListFormat (§13.4).
#[derive(Debug, Clone)]
pub struct ListFormat {
    pub locale: String,
    pub kind: String,
    pub style: String,
    /// [start, middle, end, 2] del patrón de CLDR que corresponde a tipo y estilo.
    pub patterns: [&'static str; 4],
}

pub fn init(agent: &mut Agent, realm: RealmId, intl: Gc<Object>) {
    let (c, p) = intl_constructor(
        agent,
        realm,
        intl,
        "ListFormat",
        "Intl.ListFormat.prototype",
        0,
        constructor,
    );
    crate::builtins::method(agent, c, "supportedLocalesOf", 1, supported_locales_of);
    crate::builtins::method(agent, p, "format", 1, format);
    crate::builtins::method(agent, p, "formatToParts", 1, format_to_parts);
    crate::builtins::method(agent, p, "resolvedOptions", 0, resolved_options);
}

/// GetOptionsObject (§9.2.10): undefined es un objeto sin prototipo; lo que no es objeto,
/// TypeError.
pub(crate) fn get_options_object(agent: &mut Agent, options: &Value) -> JsResult<Gc<Object>> {
    match options {
        Value::Undefined => Ok(agent.new_object(None)),
        Value::Object(o) => Ok(*o),
        _ => agent.type_error("Options must be an object"),
    }
}

/// Intl.ListFormat ( [ locales [ , options ] ] ) (§13.1.1).
fn constructor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor Intl.ListFormat requires 'new'");
    };
    let obj = create_intl_object(
        agent,
        nt,
        "Intl.ListFormat.prototype",
        IntlData::ListFormat(Box::new(ListFormat {
            locale: String::new(),
            kind: String::new(),
            style: String::new(),
            patterns: [""; 4],
        })),
    )?;
    let requested = locale::canonicalize_locale_list(agent, &a.arg(0))?;
    let options = get_options_object(agent, &a.arg(1))?;
    get_string_option(
        agent,
        options,
        "localeMatcher",
        &["lookup", "best fit"],
        Some("best fit"),
    )?;
    let r = locale::resolve_locale(&requested, &[], &[], &|_, _| Vec::new());
    let kind = get_string_option(
        agent,
        options,
        "type",
        &["conjunction", "disjunction", "unit"],
        Some("conjunction"),
    )?
    .unwrap_or_default();
    let style = get_string_option(
        agent,
        options,
        "style",
        &["long", "short", "narrow"],
        Some("long"),
    )?
    .unwrap_or_default();
    let patterns = patterns_for(&r.data_locale, &kind, &style);
    if let Some(IntlData::ListFormat(lf)) = crate::builtins::intl::intl_data_mut(agent, obj) {
        **lf = ListFormat {
            locale: r.locale,
            kind,
            style,
            patterns,
        };
    }
    Ok(Value::Object(obj))
}

/// Los patrones de CLDR para el locale, el tipo y el estilo.
fn patterns_for(locale: &str, kind: &str, style: &str) -> [&'static str; 4] {
    let lang = locale.split('-').next().unwrap_or("en");
    let base = match kind {
        "disjunction" => "or",
        "unit" => "unit",
        _ => "standard",
    };
    let key = match style {
        "short" => format!("{base}-short"),
        "narrow" => format!("{base}-narrow"),
        _ => base.to_owned(),
    };
    let find = |l: &str| {
        cldr::LIST_PATTERNS
            .iter()
            .find(|(pl, k, _)| *pl == l && *k == key)
            .map(|(_, _, p)| *p)
    };
    find(lang)
        .or_else(|| find("en"))
        .unwrap_or(["{0}, {1}", "{0}, {1}", "{0}, {1}", "{0}, {1}"])
}

fn this_list_format(agent: &mut Agent, this: &Value, method: &str) -> JsResult<ListFormat> {
    match intl_data(agent, this) {
        Some(IntlData::ListFormat(l)) => Ok((**l).clone()),
        _ => agent.type_error(&format!(
            "Method Intl.ListFormat.prototype.{method} called on incompatible receiver"
        )),
    }
}

/// StringListFromIterable ( iterable ) (§13.5.x): los strings de un iterable; un valor que
/// no es string cierra el iterador con un TypeError.
pub(crate) fn string_list_from_iterable(
    agent: &mut Agent,
    iterable: &Value,
) -> JsResult<Vec<String>> {
    if iterable.is_undefined() {
        return Ok(Vec::new());
    }
    let (it, next) = agent.get_iterator(iterable)?;
    let mut list = Vec::new();
    while let Some(v) = agent.iterator_step_value(&it, &next)? {
        match &v {
            Value::String(s) => list.push(s.to_string_lossy()),
            _ => {
                let err = agent.type_error::<()>("Iterable yielded a value which is not a string");
                agent.iterator_close_on_throw(&it);
                err?;
            }
        }
    }
    Ok(list)
}

/// Una parte de la salida: (es elemento, texto).
type Part = (bool, String);

/// Aplica un patrón `…{0}…{1}…` a dos listas de partes.
fn apply(pattern: &str, first: Vec<Part>, second: Vec<Part>) -> Vec<Part> {
    let mut out = Vec::new();
    let (i0, i1) = (pattern.find("{0}"), pattern.find("{1}"));
    let (Some(i0), Some(i1)) = (i0, i1) else {
        return [first, second].concat();
    };
    let (a, b, ia, ib) = if i0 < i1 {
        (first, second, i0, i1)
    } else {
        (second, first, i1, i0)
    };
    let lit = |s: &str, out: &mut Vec<Part>| {
        if !s.is_empty() {
            out.push((false, s.to_owned()));
        }
    };
    lit(&pattern[..ia], &mut out);
    out.extend(a);
    lit(&pattern[ia + 3..ib], &mut out);
    out.extend(b);
    lit(&pattern[ib + 3..], &mut out);
    out
}

/// CreatePartsFromList ( listFormat, list ) (§13.5.2).
fn create_parts(lf: &ListFormat, list: &[String]) -> Vec<Part> {
    let el = |s: &String| vec![(true, s.clone())];
    let [start, middle, end, two] = lf.patterns;
    match list.len() {
        0 => Vec::new(),
        1 => el(&list[0]),
        2 => apply(two, el(&list[0]), el(&list[1])),
        n => {
            let mut tail = apply(end, el(&list[n - 2]), el(&list[n - 1]));
            for i in (1..n - 2).rev() {
                tail = apply(middle, el(&list[i]), tail);
            }
            apply(start, el(&list[0]), tail)
        }
    }
}

/// Intl.ListFormat.prototype.format ( list ) (§13.3.3).
fn format(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let lf = this_list_format(agent, &a.this, "format")?;
    let list = string_list_from_iterable(agent, &a.arg(0))?;
    let s: String = create_parts(&lf, &list)
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    Ok(str_value(&s))
}

/// Intl.ListFormat.prototype.formatToParts ( list ) (§13.3.4).
fn format_to_parts(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let lf = this_list_format(agent, &a.this, "formatToParts")?;
    let list = string_list_from_iterable(agent, &a.arg(0))?;
    let mut vals = Vec::new();
    for (is_el, t) in create_parts(&lf, &list) {
        let o = agent.new_plain_object();
        let kind = if is_el { "element" } else { "literal" };
        agent.create_data_property_or_throw(o, PropertyKey::from("type"), str_value(kind))?;
        agent.create_data_property_or_throw(o, PropertyKey::from("value"), str_value(&t))?;
        vals.push(Value::Object(o));
    }
    Ok(Value::Object(agent.create_array_from_list(&vals)?))
}

/// Intl.ListFormat.prototype.resolvedOptions ( ) (§13.3.5).
fn resolved_options(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let lf = this_list_format(agent, &a.this, "resolvedOptions")?;
    let o = agent.new_plain_object();
    for (k, v) in [
        ("locale", &lf.locale),
        ("type", &lf.kind),
        ("style", &lf.style),
    ] {
        agent.create_data_property_or_throw(o, PropertyKey::from(k), str_value(v))?;
    }
    Ok(Value::Object(o))
}

/// Intl.ListFormat.supportedLocalesOf ( locales [ , options ] ) (§13.2.2).
fn supported_locales_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let requested = locale::canonicalize_locale_list(agent, &a.arg(0))?;
    locale::supported_locales(agent, &requested, &a.arg(1))
}
