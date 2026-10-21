//! Intl (ECMA-402): el objeto `Intl` (`getCanonicalLocales`, `supportedValuesOf`),
//! `Intl.NumberFormat`, `Intl.DateTimeFormat`, `Intl.Collator` e `Intl.PluralRules`, con datos
//! de CLDR 48 para `en` y `es` ([`cldr`], generado por `tools/gen_intl_data.py`), y las
//! operaciones comunes de §9.2 (GetOption y compañía).

pub mod cldr;
pub mod collator;
pub mod date_time_format;
pub mod decimal;
pub mod locale;
pub mod number_format;
pub mod plural_rules;

use crate::builtins::web::{WebData, str_value};
use crate::heap::{Gc, Trace, Tracer};
use crate::object::Object;
use crate::value::Value;
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

/// Los slots de los objetos de Intl.
#[derive(Debug)]
pub enum IntlData {
    NumberFormat(Box<number_format::NumberFormat>),
    DateTimeFormat(Box<date_time_format::DateTimeFormat>),
    Collator(Box<collator::Collator>),
    PluralRules(Box<plural_rules::PluralRules>),
}

impl Trace for IntlData {
    fn trace(&self, t: &mut Tracer) {
        match self {
            IntlData::NumberFormat(n) => t.opt(n.bound_format),
            IntlData::DateTimeFormat(d) => t.opt(d.bound_format),
            IntlData::Collator(c) => t.opt(c.bound_compare),
            IntlData::PluralRules(_) => {}
        }
    }
}

pub fn init(agent: &mut Agent, realm: RealmId) {
    let op_proto = agent.realms[realm].intrinsics.object_prototype;
    let intl = agent.new_object(Some(op_proto));
    crate::builtins::to_string_tag(agent, intl, "Intl");
    crate::builtins::method(agent, intl, "getCanonicalLocales", 1, get_canonical_locales);
    crate::builtins::method(agent, intl, "supportedValuesOf", 1, supported_values_of);
    crate::builtins::web::registry_set(agent, realm, "Intl", intl);
    crate::builtins::register_global(agent, realm, "Intl", intl);
    number_format::init(agent, realm, intl);
    date_time_format::init(agent, realm, intl);
    collator::init(agent, realm, intl);
    plural_rules::init(agent, realm, intl);
}

/// Los slots de Intl de `v`.
pub(crate) fn intl_data<'a>(agent: &'a Agent, v: &Value) -> Option<&'a IntlData> {
    match crate::builtins::web::data_of(agent, v) {
        Some(WebData::Intl(i)) => Some(i),
        _ => None,
    }
}

pub(crate) fn intl_data_mut(agent: &mut Agent, o: Gc<Object>) -> Option<&mut IntlData> {
    match crate::builtins::web::data_of_mut(agent, o) {
        Some(WebData::Intl(i)) => Some(i),
        _ => None,
    }
}

/// Un objeto de Intl con el prototipo de `new_target` (`proto_name` del registro si no).
pub(crate) fn create_intl_object(
    agent: &mut Agent,
    new_target: Gc<Object>,
    proto_name: &str,
    data: IntlData,
) -> JsResult<Gc<Object>> {
    crate::builtins::web::create_from(agent, new_target, proto_name, WebData::Intl(Box::new(data)))
}

/// Un constructor de Intl: `name` en el objeto `Intl`, con su prototipo registrado.
pub(crate) fn intl_constructor(
    agent: &mut Agent,
    realm: RealmId,
    intl: Gc<Object>,
    name: &'static str,
    proto_name: &'static str,
    length: u32,
    f: crate::vm::function::NativeFn,
) -> (Gc<Object>, Gc<Object>) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    let proto = agent.new_object(Some(op));
    let c = crate::builtins::constructor(agent, name, length, f, proto);
    let tag = format!("Intl.{name}");
    crate::builtins::to_string_tag(agent, proto, &tag);
    crate::builtins::value_prop(agent, intl, name, Value::Object(c));
    crate::builtins::web::registry_set(agent, realm, proto_name, proto);
    (c, proto)
}

// --- GetOption y compañía (§9.2.10–§9.2.15) ---

/// CoerceOptionsToObject (§9.2.11).
pub(crate) fn coerce_options_to_object(agent: &mut Agent, options: &Value) -> JsResult<Gc<Object>> {
    match options {
        Value::Undefined => Ok(agent.new_object(None)),
        v => agent.to_object(v),
    }
}

/// GetOption (§9.2.12) con tipo string: `None` si es undefined y no hay valor por defecto.
pub(crate) fn get_string_option(
    agent: &mut Agent,
    options: Gc<Object>,
    property: &str,
    values: &[&str],
    default: Option<&str>,
) -> JsResult<Option<String>> {
    let v = agent.get_str(options, property)?;
    if v.is_undefined() {
        return Ok(default.map(str::to_owned));
    }
    let s = agent.to_string(&v)?.to_string_lossy();
    if !values.is_empty() && !values.contains(&s.as_str()) {
        return agent.range_error(&format!("Value {s} out of range for {property} option"));
    }
    Ok(Some(s))
}

/// GetOption (§9.2.12) con tipo boolean.
pub(crate) fn get_boolean_option(
    agent: &mut Agent,
    options: Gc<Object>,
    property: &str,
) -> JsResult<Option<bool>> {
    let v = agent.get_str(options, property)?;
    if v.is_undefined() {
        return Ok(None);
    }
    Ok(Some(crate::abstract_ops::conversion::to_boolean(agent, &v)))
}

/// DefaultNumberOption (§9.2.15).
pub(crate) fn default_number_option(
    agent: &mut Agent,
    value: &Value,
    minimum: f64,
    maximum: f64,
    fallback: Option<f64>,
    property: &str,
) -> JsResult<Option<f64>> {
    if value.is_undefined() {
        return Ok(fallback);
    }
    let n = agent.to_number(value)?;
    if n.is_nan() || n < minimum || n > maximum {
        return agent.range_error(&format!("{property} value is out of range."));
    }
    Ok(Some(n.floor()))
}

/// GetNumberOption (§9.2.14).
pub(crate) fn get_number_option(
    agent: &mut Agent,
    options: Gc<Object>,
    property: &str,
    minimum: f64,
    maximum: f64,
    fallback: Option<f64>,
) -> JsResult<Option<f64>> {
    let v = agent.get_str(options, property)?;
    default_number_option(agent, &v, minimum, maximum, fallback, property)
}

/// `numberingSystem`/`calendar`: `(3*8alphanum) *("-" (3*8alphanum))` (type de UTS #35).
pub(crate) fn is_type_sequence(s: &str) -> bool {
    !s.is_empty()
        && s.split('-')
            .all(|p| (3..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// Los sistemas de numeración con dígitos simples (los que se pueden usar).
pub(crate) fn numbering_systems() -> Vec<String> {
    let mut v: Vec<String> = cldr::NUMBERING_SYSTEMS
        .iter()
        .map(|(n, _)| (*n).to_owned())
        .collect();
    v.sort();
    v
}

/// Los dígitos de un sistema de numeración.
pub(crate) fn numbering_system_digits(nu: &str) -> Option<Vec<char>> {
    cldr::NUMBERING_SYSTEMS
        .iter()
        .find(|(n, _)| *n == nu)
        .map(|(_, d)| d.chars().collect())
}

/// Un time zone IANA válido: (identificador con la grafía de la base, identificador
/// primario). GetAvailableNamedTimeZoneIdentifier (ECMA-262 §21.4.1.23).
pub(crate) fn time_zone_identifier(name: &str) -> Option<(&'static str, &'static str)> {
    let lower = name.to_ascii_lowercase();
    cldr::TIME_ZONES
        .binary_search_by(|(k, _)| k.to_ascii_lowercase().cmp(&lower))
        .ok()
        .map(|i| cldr::TIME_ZONES[i])
}

// --- Intl ---

/// Intl.getCanonicalLocales ( locales ) (§8.3.1).
fn get_canonical_locales(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let list = locale::canonicalize_locale_list(agent, &a.arg(0))?;
    let vals: Vec<Value> = list.iter().map(|s| str_value(s)).collect();
    Ok(Value::Object(agent.create_array_from_list(&vals)?))
}

/// Intl.supportedValuesOf ( key ) (§8.3.2).
fn supported_values_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let key = agent.to_string(&a.arg(0))?.to_string_lossy();
    let list: Vec<String> = match key.as_str() {
        "calendar" => vec!["gregory".into(), "iso8601".into()],
        "collation" => Vec::new(),
        "currency" => {
            let mut v: Vec<String> = locale::data_for("en")
                .numbers
                .currencies
                .iter()
                .map(|c| c.0.to_owned())
                .filter(|c| c.len() == 3 && c.bytes().all(|b| b.is_ascii_uppercase()))
                .collect();
            v.sort();
            v.dedup();
            v
        }
        "numberingSystem" => numbering_systems(),
        "timeZone" => {
            let mut v: Vec<String> = cldr::TIME_ZONES
                .iter()
                .filter(|(n, p)| n == p)
                .map(|(n, _)| (*n).to_owned())
                .collect();
            v.sort();
            v.dedup();
            v
        }
        "unit" => {
            let mut v: Vec<String> = number_format::SANCTIONED_UNITS
                .iter()
                .map(|u| (*u).to_owned())
                .collect();
            v.sort();
            v
        }
        _ => return agent.range_error(&format!("Invalid key : {key}")),
    };
    let vals: Vec<Value> = list.iter().map(|s| str_value(s)).collect();
    Ok(Value::Object(agent.create_array_from_list(&vals)?))
}

// --- Los métodos `toLocale*` de ECMA-402 (§19, §20) ---

/// Number.prototype.toLocaleString ( [ locales [ , options ] ] ) (§19.2.1).
pub fn number_to_locale_string(
    agent: &mut Agent,
    x: number_format_value::Value,
    locales: &Value,
    options: &Value,
) -> JsResult<Value> {
    let nf = number_format::create_for(agent, locales, options)?;
    let mv = match x {
        number_format_value::Value::Number(n) => number_format::from_number(n),
        number_format_value::Value::BigInt(b) => decimal::MathValue::Finite(b),
    };
    Ok(str_value(&number_format::format_to_string(&nf, &mv)))
}

/// Los valores que formatea Number/BigInt.prototype.toLocaleString.
pub mod number_format_value {
    pub enum Value {
        Number(f64),
        BigInt(super::decimal::Decimal),
    }
}

/// String.prototype.localeCompare ( that [ , locales [ , options ] ] ) (§19.1.1).
pub fn locale_compare(
    agent: &mut Agent,
    s: &str,
    that: &str,
    locales: &Value,
    options: &Value,
) -> JsResult<Value> {
    let c = collator::create_for(agent, locales, options)?;
    Ok(Value::Number(
        match collator::compare_strings(&c, s, that) {
            std::cmp::Ordering::Less => -1.0,
            std::cmp::Ordering::Equal => 0.0,
            std::cmp::Ordering::Greater => 1.0,
        },
    ))
}

/// TransformCase ( S, locales, targetCase ) (§19.1.2.1): `tr`/`az` (I con y sin punto) y
/// `lt` (i con punto combinante) tienen reglas propias; el resto, las por defecto.
fn transform_case(agent: &mut Agent, a: &CallArgs<'_>, upper: bool) -> JsResult<Value> {
    agent.require_object_coercible(&a.this)?;
    let s = agent.to_string(&a.this)?;
    let requested = locale::canonicalize_locale_list(agent, &a.arg(0))?;
    let lang = requested
        .first()
        .map(|l| l.split('-').next().unwrap_or("").to_owned())
        .unwrap_or_default();
    let text = s.to_string_lossy();
    let out: String = match (lang.as_str(), upper) {
        ("tr" | "az", true) => text
            .chars()
            .flat_map(|c| match c {
                'i' => vec!['\u{130}'],
                c => c.to_uppercase().collect(),
            })
            .collect(),
        ("tr" | "az", false) => {
            let replaced = text.replace("I\u{307}", "i");
            replaced
                .chars()
                .flat_map(|c| match c {
                    'I' => vec!['\u{131}'],
                    '\u{130}' => vec!['i'],
                    c => c.to_lowercase().collect(),
                })
                .collect()
        }
        ("lt", true) => text
            .replace("i\u{307}", "i")
            .replace("j\u{307}", "j")
            .to_uppercase(),
        _ => {
            if s.is_well_formed() {
                if upper {
                    text.to_uppercase()
                } else {
                    text.to_lowercase()
                }
            } else {
                // Con surrogates sueltos: unidad a unidad, sin tocar los sueltos.
                let mut out: Vec<u16> = Vec::new();
                for r in char::decode_utf16(s.units().iter().copied()) {
                    match r {
                        Ok(c) => {
                            let t: String = if upper {
                                c.to_uppercase().collect()
                            } else {
                                c.to_lowercase().collect()
                            };
                            out.extend(t.encode_utf16());
                        }
                        Err(e) => out.push(e.unpaired_surrogate()),
                    }
                }
                return Ok(Value::String(crate::string::JsString::from_units(out)));
            }
        }
    };
    Ok(str_value(&out))
}

/// String.prototype.toLocaleLowerCase ( [ locales ] ) (§19.1.2).
pub fn to_locale_lower_case(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    transform_case(agent, a, false)
}

/// String.prototype.toLocaleUpperCase ( [ locales ] ) (§19.1.3).
pub fn to_locale_upper_case(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    transform_case(agent, a, true)
}
