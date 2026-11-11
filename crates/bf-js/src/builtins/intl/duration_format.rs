//! Intl.DurationFormat (ECMA-402 §13 de la edición 2025): cada unidad se formatea con un
//! Intl.NumberFormat (estilo `unit`, o numérico con `:` entre horas, minutos y segundos) y
//! la lista, con los patrones `unit` de Intl.ListFormat.

use crate::builtins::intl::decimal::{self, MathValue};
use crate::builtins::intl::{
    IntlData, create_intl_object, get_number_option, get_string_option, intl_constructor,
    intl_data, is_type_sequence, list_format, locale, number_format, numbering_systems,
};
use crate::builtins::web::str_value;
use crate::heap::Gc;
use crate::object::Object;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

/// Las unidades en el orden de la tabla 20 (Components of Duration Instances).
const UNITS: [&str; 10] = [
    "years",
    "months",
    "weeks",
    "days",
    "hours",
    "minutes",
    "seconds",
    "milliseconds",
    "microseconds",
    "nanoseconds",
];

/// Los slots de un Intl.DurationFormat (§13.4).
#[derive(Debug, Clone)]
pub struct DurationFormat {
    pub locale: String,
    pub numbering_system: String,
    pub style: String,
    /// [[Style]] y [[Display]] de cada unidad (orden de [`UNITS`]).
    pub units: [(String, String); 10],
    pub fractional_digits: Option<u32>,
}

pub fn init(agent: &mut Agent, realm: RealmId, intl: Gc<Object>) {
    let (c, p) = intl_constructor(
        agent,
        realm,
        intl,
        "DurationFormat",
        "Intl.DurationFormat.prototype",
        0,
        constructor,
    );
    crate::builtins::method(agent, c, "supportedLocalesOf", 1, supported_locales_of);
    crate::builtins::method(agent, p, "format", 1, format);
    crate::builtins::method(agent, p, "formatToParts", 1, format_to_parts);
    crate::builtins::method(agent, p, "resolvedOptions", 0, resolved_options);
}

/// GetDurationUnitOptions ( unit, options, style, unitList, digitalBase, prevStyle,
/// twoDigitHours ) (§13.5.6) con ValidateDurationUnitStyle (§13.5.5).
fn get_duration_unit_options(
    agent: &mut Agent,
    unit: &str,
    options: Gc<Object>,
    style: &str,
    unit_list: &[&str],
    digital_base: &str,
    prev_style: &str,
) -> JsResult<(String, String)> {
    let mut unit_style = get_string_option(agent, options, unit, unit_list, None)?;
    let mut display_default = "always";
    if unit_style.is_none() {
        if style == "digital" {
            unit_style = Some(digital_base.to_owned());
            if !matches!(unit, "hours" | "minutes" | "seconds") {
                display_default = "auto";
            }
        } else if matches!(prev_style, "fractional" | "numeric" | "2-digit") {
            unit_style = Some("numeric".to_owned());
            if !matches!(unit, "minutes" | "seconds") {
                display_default = "auto";
            }
        } else {
            unit_style = Some(style.to_owned());
            display_default = "auto";
        }
    }
    let mut unit_style = unit_style.unwrap_or_default();
    if unit_style == "numeric" && matches!(unit, "milliseconds" | "microseconds" | "nanoseconds") {
        unit_style = "fractional".to_owned();
        display_default = "auto";
    }
    let field = format!("{unit}Display");
    let display = get_string_option(
        agent,
        options,
        &field,
        &["auto", "always"],
        Some(display_default),
    )?
    .unwrap_or_default();
    // ValidateDurationUnitStyle.
    if display == "always" && unit_style == "fractional" {
        return agent.range_error(&format!(
            "{field} can't be \"always\" when {unit} is \"numeric\""
        ));
    }
    if prev_style == "fractional" && unit_style != "fractional" {
        return agent.range_error(&format!("{unit} must be \"numeric\""));
    }
    if matches!(prev_style, "numeric" | "2-digit")
        && !matches!(unit_style.as_str(), "fractional" | "numeric" | "2-digit")
    {
        return agent.range_error(&format!(
            "{unit} must be \"numeric\" or \"2-digit\" after a numeric unit"
        ));
    }
    if matches!(unit, "minutes" | "seconds") && matches!(prev_style, "numeric" | "2-digit") {
        unit_style = "2-digit".to_owned();
    }
    Ok((unit_style, display))
}

/// Intl.DurationFormat ( [ locales [ , options ] ] ) (§13.1.1).
fn constructor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor Intl.DurationFormat requires 'new'");
    };
    let empty = || (String::new(), String::new());
    let obj = create_intl_object(
        agent,
        nt,
        "Intl.DurationFormat.prototype",
        IntlData::DurationFormat(Box::new(DurationFormat {
            locale: String::new(),
            numbering_system: String::new(),
            style: String::new(),
            units: std::array::from_fn(|_| empty()),
            fractional_digits: None,
        })),
    )?;
    let requested = locale::canonicalize_locale_list(agent, &a.arg(0))?;
    let options = list_format::get_options_object(agent, &a.arg(1))?;
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
    let numbering_system = r.get("nu").unwrap_or("latn").to_owned();
    let style = get_string_option(
        agent,
        options,
        "style",
        &["long", "short", "narrow", "digital"],
        Some("short"),
    )?
    .unwrap_or_default();
    let mut units: [(String, String); 10] = std::array::from_fn(|_| empty());
    let mut prev_style = String::new();
    for (i, unit) in UNITS.iter().enumerate() {
        let (list, base): (&[&str], &str) = match i {
            0..=3 => (&["long", "short", "narrow"], "short"),
            4..=6 => (
                &["long", "short", "narrow", "numeric", "2-digit"],
                "numeric",
            ),
            _ => (&["long", "short", "narrow", "numeric"], "numeric"),
        };
        let u = get_duration_unit_options(agent, unit, options, &style, list, base, &prev_style)?;
        if i >= 4 {
            prev_style = u.0.clone();
        }
        units[i] = u;
    }
    let fd = get_number_option(agent, options, "fractionalDigits", 0.0, 9.0, None)?;
    if let Some(IntlData::DurationFormat(d)) = crate::builtins::intl::intl_data_mut(agent, obj) {
        **d = DurationFormat {
            locale: r.locale,
            numbering_system,
            style,
            units,
            fractional_digits: fd.map(|x| x as u32),
        };
    }
    Ok(Value::Object(obj))
}

fn this_duration_format(agent: &mut Agent, this: &Value, method: &str) -> JsResult<DurationFormat> {
    match intl_data(agent, this) {
        Some(IntlData::DurationFormat(d)) => Ok((**d).clone()),
        _ => agent.type_error(&format!(
            "Method Intl.DurationFormat.prototype.{method} called on incompatible receiver"
        )),
    }
}

/// Una parte de la salida: (tipo, valor, unidad en singular).
type Part = (String, String, Option<&'static str>);

/// El valor de los segundos (o ms, µs) con las unidades fraccionarias que le siguen, como
/// decimal exacto (`exponent` es 9, 6 o 3).
fn fractional_value(d: &[i128; 10], unit: usize) -> String {
    let (exponent, mut ns): (u32, i128) = match unit {
        6 => (
            9,
            d[6] * 1_000_000_000 + d[7] * 1_000_000 + d[8] * 1000 + d[9],
        ),
        7 => (6, d[7] * 1_000_000 + d[8] * 1000 + d[9]),
        _ => (3, d[8] * 1000 + d[9]),
    };
    let e = 10i128.pow(exponent);
    let neg = ns < 0;
    if neg {
        ns = -ns;
    }
    let (q, r) = (ns / e, ns % e);
    let sign = if neg { "-" } else { "" };
    format!("{sign}{q}.{r:0width$}", width = exponent as usize)
}

/// PartitionDurationFormatPattern ( durationFormat, duration ) (§13.5.7).
fn partition(agent: &mut Agent, df: &DurationFormat, d: &[f64; 10]) -> JsResult<Vec<Part>> {
    let iv: [i128; 10] = std::array::from_fn(|i| d[i] as i128);
    let negative = d.iter().any(|&v| v < 0.0);
    let mut result: Vec<Vec<Part>> = Vec::new();
    let mut need_separator = false;
    let mut display_negative_sign = true;
    let locale_v = str_value(&df.locale);
    for i in 0..10 {
        let (style, display) = (&df.units[i].0, &df.units[i].1);
        let unit_singular: &'static str = &UNITS[i][..UNITS[i].len() - 1];
        let mut value: MathValue = decimal::parse_str(&iv[i].to_string());
        let mut is_zero = iv[i] == 0;
        let nf_opts = agent.new_object(None);
        let mut done = false;
        if (6..=8).contains(&i) && df.units[i + 1].0 == "fractional" {
            let s = fractional_value(&iv, i);
            is_zero = s
                .trim_start_matches('-')
                .bytes()
                .all(|b| b == b'0' || b == b'.');
            value = decimal::parse_str(&s);
            let max = df.fractional_digits.unwrap_or(9);
            let min = df.fractional_digits.unwrap_or(0);
            set(
                agent,
                nf_opts,
                "maximumFractionDigits",
                Value::Number(f64::from(max)),
            );
            set(
                agent,
                nf_opts,
                "minimumFractionDigits",
                Value::Number(f64::from(min)),
            );
            set(agent, nf_opts, "roundingMode", str_value("trunc"));
            done = true;
        }
        // Los minutos numéricos en cero se muestran si siguen segundos.
        let mut display_required = false;
        if i == 5 && need_separator {
            display_required = df.units[6].1 == "always" || iv[6..].iter().any(|&v| v != 0);
        }
        if !is_zero || display != "auto" || display_required {
            if display_negative_sign {
                display_negative_sign = false;
                if is_zero && negative {
                    value = number_format::from_number(-0.0);
                }
            } else {
                set(agent, nf_opts, "signDisplay", str_value("never"));
            }
            set(
                agent,
                nf_opts,
                "numberingSystem",
                str_value(&df.numbering_system),
            );
            if style == "2-digit" {
                set(agent, nf_opts, "minimumIntegerDigits", Value::Number(2.0));
            }
            if style != "numeric" && style != "2-digit" && style != "fractional" {
                set(agent, nf_opts, "style", str_value("unit"));
                set(agent, nf_opts, "unit", str_value(unit_singular));
                set(agent, nf_opts, "unitDisplay", str_value(style));
            } else {
                set(agent, nf_opts, "useGrouping", Value::Boolean(false));
            }
            let nf = number_format::create_for(agent, &locale_v, &Value::Object(nf_opts))?;
            let mut list = if need_separator {
                let mut l = result.pop().unwrap_or_default();
                l.push(("literal".to_owned(), ":".to_owned(), None));
                l
            } else {
                Vec::new()
            };
            for (t, v) in number_format::partition(&nf, &value) {
                list.push((t.to_owned(), v, Some(unit_singular)));
            }
            if !need_separator && (style == "2-digit" || style == "numeric") {
                need_separator = true;
            }
            result.push(list);
        }
        if done {
            break;
        }
    }
    // La lista de unidades.
    let list_style = if df.style == "digital" {
        "short"
    } else {
        df.style.as_str()
    };
    let lf = list_format::ListFormat {
        locale: df.locale.clone(),
        kind: "unit".to_owned(),
        style: list_style.to_owned(),
        patterns: list_format::patterns_for(&df.locale, "unit", list_style),
    };
    let strings: Vec<String> = result
        .iter()
        .map(|parts| parts.iter().map(|p| p.1.as_str()).collect())
        .collect();
    let mut out = Vec::new();
    let mut groups = result.into_iter();
    for (is_el, text) in list_format::create_parts(&lf, &strings) {
        if is_el {
            out.extend(groups.next().unwrap_or_default());
        } else {
            out.push(("literal".to_owned(), text, None));
        }
    }
    Ok(out)
}

fn set(agent: &mut Agent, o: Gc<Object>, k: &str, v: Value) {
    agent.put_data(o, PropertyKey::from(k), v, true, true, true);
}

/// Intl.DurationFormat.prototype.format ( duration ) (§13.3.3).
fn format(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let df = this_duration_format(agent, &a.this, "format")?;
    let d = agent.to_temporal_duration(&a.arg(0))?;
    let parts = partition(agent, &df, &d.0)?;
    let s: String = parts.into_iter().map(|p| p.1).collect();
    Ok(str_value(&s))
}

/// Intl.DurationFormat.prototype.formatToParts ( duration ) (§13.3.4).
fn format_to_parts(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let df = this_duration_format(agent, &a.this, "formatToParts")?;
    let d = agent.to_temporal_duration(&a.arg(0))?;
    let parts = partition(agent, &df, &d.0)?;
    let mut vals = Vec::new();
    for (t, v, unit) in parts {
        let o = agent.new_plain_object();
        agent.create_data_property_or_throw(o, PropertyKey::from("type"), str_value(&t))?;
        agent.create_data_property_or_throw(o, PropertyKey::from("value"), str_value(&v))?;
        if let Some(u) = unit {
            agent.create_data_property_or_throw(o, PropertyKey::from("unit"), str_value(u))?;
        }
        vals.push(Value::Object(o));
    }
    Ok(Value::Object(agent.create_array_from_list(&vals)?))
}

/// Intl.DurationFormat.prototype.resolvedOptions ( ) (§13.3.5).
fn resolved_options(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let df = this_duration_format(agent, &a.this, "resolvedOptions")?;
    let o = agent.new_plain_object();
    for (k, v) in [
        ("locale", &df.locale),
        ("numberingSystem", &df.numbering_system),
        ("style", &df.style),
    ] {
        agent.create_data_property_or_throw(o, PropertyKey::from(k), str_value(v))?;
    }
    for (i, unit) in UNITS.iter().enumerate() {
        let (style, display) = &df.units[i];
        // "fractional" se informa como "numeric".
        let style = if style == "fractional" {
            "numeric"
        } else {
            style
        };
        agent.create_data_property_or_throw(o, PropertyKey::from(*unit), str_value(style))?;
        let field = format!("{unit}Display");
        agent.create_data_property_or_throw(
            o,
            PropertyKey::from(field.as_str()),
            str_value(display),
        )?;
    }
    if let Some(fd) = df.fractional_digits {
        agent.create_data_property_or_throw(
            o,
            PropertyKey::from("fractionalDigits"),
            Value::Number(f64::from(fd)),
        )?;
    }
    Ok(Value::Object(o))
}

/// Intl.DurationFormat.supportedLocalesOf ( locales [ , options ] ) (§13.2.2).
fn supported_locales_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let requested = locale::canonicalize_locale_list(agent, &a.arg(0))?;
    locale::supported_locales(agent, &requested, &a.arg(1))
}
