//! Intl.RelativeTimeFormat (ECMA-402 §18) con los `dateFields` de CLDR de `en` y `es`.

use crate::builtins::intl::decimal::Decimal;
use crate::builtins::intl::number_format::{self, NumberFormat, format_numeric_to_string};
use crate::builtins::intl::plural_rules::{Operands, select_category};
use crate::builtins::intl::{
    IntlData, cldr, coerce_options_to_object, create_intl_object, get_string_option,
    intl_constructor, intl_data, intl_data_mut, is_type_sequence, locale, numbering_systems,
};
use crate::builtins::web::str_value;
use crate::heap::Gc;
use crate::object::Object;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

/// Los slots de un Intl.RelativeTimeFormat (§18.4).
#[derive(Debug, Clone)]
pub struct RelativeTimeFormat {
    pub locale: String,
    pub data_locale: String,
    pub style: String,
    pub numeric: String,
    pub numbering_system: String,
    pub number_format: Option<Box<NumberFormat>>,
}

pub fn init(agent: &mut Agent, realm: RealmId, intl: Gc<Object>) {
    let (c, p) = intl_constructor(
        agent,
        realm,
        intl,
        "RelativeTimeFormat",
        "Intl.RelativeTimeFormat.prototype",
        0,
        constructor,
    );
    crate::builtins::method(agent, c, "supportedLocalesOf", 1, supported_locales_of);
    crate::builtins::method(agent, p, "format", 2, format);
    crate::builtins::method(agent, p, "formatToParts", 2, format_to_parts);
    crate::builtins::method(agent, p, "resolvedOptions", 0, resolved_options);
}

/// Intl.RelativeTimeFormat ( [ locales [ , options ] ] ) (§18.1.1).
fn constructor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor Intl.RelativeTimeFormat requires 'new'");
    };
    let obj = create_intl_object(
        agent,
        nt,
        "Intl.RelativeTimeFormat.prototype",
        IntlData::RelativeTimeFormat(Box::new(RelativeTimeFormat {
            locale: String::new(),
            data_locale: String::new(),
            style: String::new(),
            numeric: String::new(),
            numbering_system: String::new(),
            number_format: None,
        })),
    )?;
    let requested = locale::canonicalize_locale_list(agent, &a.arg(0))?;
    let options = coerce_options_to_object(agent, &a.arg(1))?;
    get_string_option(
        agent,
        options,
        "localeMatcher",
        &["lookup", "best fit"],
        Some("best fit"),
    )?;
    let nu = get_string_option(agent, options, "numberingSystem", &[], None)?;
    if let Some(n) = &nu
        && !is_type_sequence(n)
    {
        return agent.range_error(&format!("Invalid numberingSystem : {n}"));
    }
    let r = locale::resolve_locale(&requested, &[("nu", nu)], &["nu"], &|key, _| {
        if key == "nu" {
            let mut v = vec!["latn".to_owned()];
            v.extend(numbering_systems().into_iter().filter(|n| n != "latn"));
            v
        } else {
            Vec::new()
        }
    });
    let style = get_string_option(
        agent,
        options,
        "style",
        &["long", "short", "narrow"],
        Some("long"),
    )?
    .unwrap_or_default();
    let numeric = get_string_option(
        agent,
        options,
        "numeric",
        &["always", "auto"],
        Some("always"),
    )?
    .unwrap_or_default();
    let numbering_system = r.get("nu").unwrap_or("latn").to_owned();
    // §18.1.1 pasos 19-20: un Intl.NumberFormat con el locale y el sistema de numeración.
    let nf_options = agent.new_object(None);
    agent.create_data_property_or_throw(
        nf_options,
        PropertyKey::from("numberingSystem"),
        str_value(&numbering_system),
    )?;
    let nf = number_format::create_for(agent, &str_value(&r.locale), &Value::Object(nf_options))?;
    if let Some(IntlData::RelativeTimeFormat(rtf)) = intl_data_mut(agent, obj) {
        **rtf = RelativeTimeFormat {
            locale: r.locale,
            data_locale: r.data_locale,
            style,
            numeric,
            numbering_system,
            number_format: Some(Box::new(nf)),
        };
    }
    Ok(Value::Object(obj))
}

fn this_rtf(agent: &mut Agent, this: &Value, method: &str) -> JsResult<RelativeTimeFormat> {
    match intl_data(agent, this) {
        Some(IntlData::RelativeTimeFormat(r)) => Ok((**r).clone()),
        _ => agent.type_error(&format!(
            "Method Intl.RelativeTimeFormat.prototype.{method} called on incompatible receiver"
        )),
    }
}

/// SingularRelativeTimeUnit ( unit ) (§18.5.1).
fn singular_unit(unit: &str) -> Option<&'static str> {
    const UNITS: [&str; 8] = [
        "second", "minute", "hour", "day", "week", "month", "quarter", "year",
    ];
    UNITS
        .iter()
        .find(|u| unit == **u || unit.strip_suffix('s') == Some(**u))
        .copied()
}

/// Una parte: (tipo, valor, con la unidad).
type Part = (&'static str, String, bool);

/// PartitionRelativeTimePattern ( relativeTimeFormat, value, unit ) (§18.5.2).
fn partition(
    agent: &mut Agent,
    rtf: &RelativeTimeFormat,
    a: &CallArgs<'_>,
) -> JsResult<(Vec<Part>, &'static str)> {
    let value = agent.to_number(&a.arg(0))?;
    let unit_s = agent.to_string(&a.arg(1))?.to_string_lossy();
    if !value.is_finite() {
        return agent.range_error("Invalid value: must be finite");
    }
    let Some(unit) = singular_unit(&unit_s) else {
        return agent.range_error(&format!("Invalid unit argument '{unit_s}'"));
    };
    let lang = rtf.data_locale.split('-').next().unwrap_or("en");
    let field = match rtf.style.as_str() {
        "short" => format!("{unit}-short"),
        "narrow" => format!("{unit}-narrow"),
        _ => unit.to_owned(),
    };
    let find = |l: &str| {
        cldr::RELATIVE_FIELDS
            .iter()
            .find(|(fl, f, ..)| *fl == l && *f == field)
    };
    let Some(&(_, _, relative, future, past)) = find(lang).or_else(|| find("en")) else {
        return agent.range_error("Missing locale data");
    };
    // numeric "auto": los nombres de -1, 0, 1… (`-0` es "0").
    if rtf.numeric == "auto"
        && value.fract() == 0.0
        && value.abs() <= 127.0
        && let Some((_, s)) = relative.iter().find(|(n, _)| f64::from(*n) == value)
    {
        return Ok((vec![("literal", (*s).to_owned(), false)], unit));
    }
    let is_past = value < 0.0 || (value == 0.0 && value.is_sign_negative());
    let abs = value.abs();
    let patterns = if is_past { past } else { future };
    let nf = rtf.number_format.as_deref().expect("NumberFormat");
    let fv = number_format::partition(nf, &number_format::from_number(abs));
    let formatted = format_numeric_to_string(&nf.digits, &Decimal::from_f64(abs));
    let category = select_category(
        &rtf.locale,
        false,
        &Operands::from_str(&formatted.string, 0),
    );
    let pattern = patterns
        .iter()
        .find(|(c, _)| *c == category)
        .or_else(|| patterns.iter().find(|(c, _)| *c == "other"))
        .map_or("{0}", |(_, p)| *p);
    // MakePartsList ( pattern, unit, parts ) (§18.5.3).
    let mut out = Vec::new();
    let (before, after) = pattern.split_once("{0}").unwrap_or((pattern, ""));
    if !before.is_empty() {
        out.push(("literal", before.to_owned(), false));
    }
    for (t, v) in fv {
        out.push((t, v, true));
    }
    if !after.is_empty() {
        out.push(("literal", after.to_owned(), false));
    }
    Ok((out, unit))
}

/// Intl.RelativeTimeFormat.prototype.format ( value, unit ) (§18.3.3).
fn format(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let rtf = this_rtf(agent, &a.this, "format")?;
    let (parts, _) = partition(agent, &rtf, a)?;
    let s: String = parts.into_iter().map(|(_, v, _)| v).collect();
    Ok(str_value(&s))
}

/// Intl.RelativeTimeFormat.prototype.formatToParts ( value, unit ) (§18.3.4).
fn format_to_parts(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let rtf = this_rtf(agent, &a.this, "formatToParts")?;
    let (parts, unit) = partition(agent, &rtf, a)?;
    let mut vals = Vec::new();
    for (t, v, with_unit) in parts {
        let o = agent.new_plain_object();
        agent.create_data_property_or_throw(o, PropertyKey::from("type"), str_value(t))?;
        agent.create_data_property_or_throw(o, PropertyKey::from("value"), str_value(&v))?;
        if with_unit {
            agent.create_data_property_or_throw(o, PropertyKey::from("unit"), str_value(unit))?;
        }
        vals.push(Value::Object(o));
    }
    Ok(Value::Object(agent.create_array_from_list(&vals)?))
}

/// Intl.RelativeTimeFormat.prototype.resolvedOptions ( ) (§18.3.5).
fn resolved_options(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let rtf = this_rtf(agent, &a.this, "resolvedOptions")?;
    let o = agent.new_plain_object();
    for (k, v) in [
        ("locale", &rtf.locale),
        ("style", &rtf.style),
        ("numeric", &rtf.numeric),
        ("numberingSystem", &rtf.numbering_system),
    ] {
        agent.create_data_property_or_throw(o, PropertyKey::from(k), str_value(v))?;
    }
    Ok(Value::Object(o))
}

/// Intl.RelativeTimeFormat.supportedLocalesOf ( locales [ , options ] ) (§18.2.2).
fn supported_locales_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let requested = locale::canonicalize_locale_list(agent, &a.arg(0))?;
    locale::supported_locales(agent, &requested, &a.arg(1))
}
