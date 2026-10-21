//! Intl.NumberFormat (ECMA-402 §16): las opciones (SetNumberFormatUnitOptions,
//! SetNumberFormatDigitOptions), FormatNumericToString, ComputeExponent,
//! PartitionNumberPattern con notación estándar, científica, de ingeniería y compacta, monedas
//! y unidades, `format`, `formatToParts`, `formatRange(ToParts)` y `resolvedOptions`, más el
//! modo constructor legacy (§4.3 nota 1, `[[FallbackSymbol]]`).

use crate::builtins::intl::cldr;
use crate::builtins::intl::decimal::{
    Decimal, MathValue, RawFormat, RoundingMode, parse_str, to_raw_fixed, to_raw_precision,
};
use crate::builtins::intl::plural_rules::{Operands, select_category};
use crate::builtins::intl::{
    IntlData, coerce_options_to_object, create_intl_object, default_number_option,
    get_number_option, get_string_option, intl_constructor, intl_data, intl_data_mut,
    is_type_sequence, locale, numbering_system_digits, numbering_systems,
};
use crate::builtins::web::str_value;
use crate::heap::Gc;
use crate::object::Object;
use crate::string::JsString;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

/// ECMA-402 §6.6.2, Table 2.
pub const SANCTIONED_UNITS: &[&str] = &[
    "acre",
    "bit",
    "byte",
    "celsius",
    "centimeter",
    "day",
    "degree",
    "fahrenheit",
    "fluid-ounce",
    "foot",
    "gallon",
    "gigabit",
    "gigabyte",
    "gram",
    "hectare",
    "hour",
    "inch",
    "kilobit",
    "kilobyte",
    "kilogram",
    "kilometer",
    "liter",
    "megabit",
    "megabyte",
    "meter",
    "microsecond",
    "mile",
    "mile-scandinavian",
    "milliliter",
    "millimeter",
    "millisecond",
    "minute",
    "month",
    "nanosecond",
    "ounce",
    "percent",
    "petabyte",
    "pound",
    "second",
    "stone",
    "terabit",
    "terabyte",
    "week",
    "yard",
    "year",
];

const ROUNDING_INCREMENTS: &[f64] = &[
    1.0, 2.0, 5.0, 10.0, 20.0, 25.0, 50.0, 100.0, 200.0, 250.0, 500.0, 1000.0, 2000.0, 2500.0,
    5000.0,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundingType {
    FractionDigits,
    SignificantDigits,
    MorePrecision,
    LessPrecision,
}

/// Los slots de dígitos y redondeo (SetNumberFormatDigitOptions, §16.5.? ).
#[derive(Debug, Clone)]
pub struct DigitOptions {
    pub min_int: u32,
    pub min_frac: Option<u32>,
    pub max_frac: Option<u32>,
    pub min_sig: Option<u32>,
    pub max_sig: Option<u32>,
    pub rounding_increment: u32,
    pub rounding_mode: RoundingMode,
    pub rounding_type: RoundingType,
    pub computed_priority: &'static str,
    pub strip_if_integer: bool,
}

impl DigitOptions {
    /// Los campos de dígitos de resolvedOptions (en el orden de Table 16).
    pub fn write_resolved(&self, agent: &mut Agent, o: Gc<Object>) -> JsResult<()> {
        let set = |agent: &mut Agent, k: &str, v: Option<u32>| -> JsResult<()> {
            if let Some(v) = v {
                agent.create_data_property_or_throw(
                    o,
                    PropertyKey::from(k),
                    Value::Number(f64::from(v)),
                )?;
            }
            Ok(())
        };
        set(agent, "minimumIntegerDigits", Some(self.min_int))?;
        set(agent, "minimumFractionDigits", self.min_frac)?;
        set(agent, "maximumFractionDigits", self.max_frac)?;
        set(agent, "minimumSignificantDigits", self.min_sig)?;
        set(agent, "maximumSignificantDigits", self.max_sig)?;
        Ok(())
    }

    /// Los campos de redondeo de resolvedOptions.
    pub fn write_rounding(&self, agent: &mut Agent, o: Gc<Object>) -> JsResult<()> {
        agent.create_data_property_or_throw(
            o,
            PropertyKey::from("roundingIncrement"),
            Value::Number(f64::from(self.rounding_increment)),
        )?;
        agent.create_data_property_or_throw(
            o,
            PropertyKey::from("roundingMode"),
            str_value(self.rounding_mode.name()),
        )?;
        agent.create_data_property_or_throw(
            o,
            PropertyKey::from("roundingPriority"),
            str_value(self.computed_priority),
        )?;
        agent.create_data_property_or_throw(
            o,
            PropertyKey::from("trailingZeroDisplay"),
            str_value(if self.strip_if_integer {
                "stripIfInteger"
            } else {
                "auto"
            }),
        )?;
        Ok(())
    }
}

/// SetNumberFormatDigitOptions ( intlObj, options, mnfdDefault, mxfdDefault, notation ).
pub fn set_digit_options(
    agent: &mut Agent,
    options: Gc<Object>,
    mnfd_default: u32,
    mxfd_default: u32,
    notation: &str,
) -> JsResult<DigitOptions> {
    let mnid = get_number_option(agent, options, "minimumIntegerDigits", 1.0, 21.0, Some(1.0))?
        .unwrap_or(1.0) as u32;
    let mnfd = agent.get_str(options, "minimumFractionDigits")?;
    let mxfd = agent.get_str(options, "maximumFractionDigits")?;
    let mnsd = agent.get_str(options, "minimumSignificantDigits")?;
    let mxsd = agent.get_str(options, "maximumSignificantDigits")?;
    let increment = get_number_option(agent, options, "roundingIncrement", 1.0, 5000.0, Some(1.0))?
        .unwrap_or(1.0);
    if !ROUNDING_INCREMENTS.contains(&increment) {
        return agent.range_error("roundingIncrement value is out of range.");
    }
    let mode = get_string_option(
        agent,
        options,
        "roundingMode",
        &[
            "ceil",
            "floor",
            "expand",
            "trunc",
            "halfCeil",
            "halfFloor",
            "halfExpand",
            "halfTrunc",
            "halfEven",
        ],
        Some("halfExpand"),
    )?
    .unwrap_or_default();
    let priority = get_string_option(
        agent,
        options,
        "roundingPriority",
        &["auto", "morePrecision", "lessPrecision"],
        Some("auto"),
    )?
    .unwrap_or_default();
    let tzd = get_string_option(
        agent,
        options,
        "trailingZeroDisplay",
        &["auto", "stripIfInteger"],
        Some("auto"),
    )?
    .unwrap_or_default();
    let mut mxfd_default = mxfd_default;
    if increment != 1.0 {
        mxfd_default = mnfd_default;
    }
    let mut d = DigitOptions {
        min_int: mnid,
        min_frac: None,
        max_frac: None,
        min_sig: None,
        max_sig: None,
        rounding_increment: increment as u32,
        rounding_mode: RoundingMode::parse(&mode),
        rounding_type: RoundingType::FractionDigits,
        computed_priority: "auto",
        strip_if_integer: tzd == "stripIfInteger",
    };
    let has_sd = !mnsd.is_undefined() || !mxsd.is_undefined();
    let has_fd = !mnfd.is_undefined() || !mxfd.is_undefined();
    let mut need_sd = true;
    let mut need_fd = true;
    if priority == "auto" {
        need_sd = has_sd;
        if need_sd || (!has_fd && notation == "compact") {
            need_fd = false;
        }
    }
    if need_sd {
        if has_sd {
            let min = default_number_option(
                agent,
                &mnsd,
                1.0,
                21.0,
                Some(1.0),
                "minimumSignificantDigits",
            )?
            .unwrap_or(1.0);
            let max = default_number_option(
                agent,
                &mxsd,
                min,
                21.0,
                Some(21.0),
                "maximumSignificantDigits",
            )?
            .unwrap_or(21.0);
            d.min_sig = Some(min as u32);
            d.max_sig = Some(max as u32);
        } else {
            d.min_sig = Some(1);
            d.max_sig = Some(21);
        }
    }
    if need_fd {
        if has_fd {
            let mut min =
                default_number_option(agent, &mnfd, 0.0, 100.0, None, "minimumFractionDigits")?;
            let mut max =
                default_number_option(agent, &mxfd, 0.0, 100.0, None, "maximumFractionDigits")?;
            match (min, max) {
                (None, Some(mx)) => min = Some(f64::from(mnfd_default).min(mx)),
                (Some(mn), None) => max = Some(f64::from(mxfd_default).max(mn)),
                (Some(mn), Some(mx)) if mn > mx => {
                    return agent.range_error("maximumFractionDigits value is out of range.");
                }
                _ => {}
            }
            d.min_frac = min.map(|v| v as u32);
            d.max_frac = max.map(|v| v as u32);
        } else {
            d.min_frac = Some(mnfd_default);
            d.max_frac = Some(mxfd_default);
        }
    }
    if !need_sd && !need_fd {
        d.min_frac = Some(0);
        d.max_frac = Some(0);
        d.min_sig = Some(1);
        d.max_sig = Some(2);
        d.rounding_type = RoundingType::MorePrecision;
        d.computed_priority = "morePrecision";
    } else if priority == "auto" {
        d.rounding_type = if has_sd {
            RoundingType::SignificantDigits
        } else {
            RoundingType::FractionDigits
        };
    } else if priority == "morePrecision" {
        d.rounding_type = RoundingType::MorePrecision;
        d.computed_priority = "morePrecision";
    } else {
        d.rounding_type = RoundingType::LessPrecision;
        d.computed_priority = "lessPrecision";
    }
    if increment != 1.0 {
        if d.rounding_type != RoundingType::FractionDigits {
            return agent.type_error("roundingIncrement requires fraction digit rounding");
        }
        if d.max_frac != d.min_frac {
            return agent.range_error(
                "maximumFractionDigits must be equal to minimumFractionDigits with roundingIncrement",
            );
        }
    }
    Ok(d)
}

/// FormatNumericToString ( intlObject, x ) (§16.5.3): `x` es finito.
pub fn format_numeric_to_string(d: &DigitOptions, x: &Decimal) -> RawFormat {
    let sig = || {
        to_raw_precision(
            x,
            d.min_sig.unwrap_or(1),
            d.max_sig.unwrap_or(21),
            d.rounding_mode,
        )
    };
    let fix = || {
        to_raw_fixed(
            x,
            d.min_frac.unwrap_or(0),
            d.max_frac.unwrap_or(3),
            d.rounding_increment,
            d.rounding_mode,
        )
    };
    let mut r = match d.rounding_type {
        RoundingType::SignificantDigits => sig(),
        RoundingType::FractionDigits => fix(),
        RoundingType::MorePrecision | RoundingType::LessPrecision => {
            let s = sig();
            let f = fix();
            let s_more = s.rounding_magnitude <= f.rounding_magnitude;
            if (d.rounding_type == RoundingType::MorePrecision) == s_more {
                s
            } else {
                f
            }
        }
    };
    let is_integer = r.rounded.exp >= 0 || r.rounded.is_zero();
    if d.strip_if_integer
        && is_integer
        && let Some(i) = r.string.find('.')
    {
        r.string.truncate(i);
    }
    if r.integer_digits < i64::from(d.min_int) {
        let zeros = "0".repeat((i64::from(d.min_int) - r.integer_digits) as usize);
        r.string = zeros + &r.string;
    }
    r.rounded.negative = x.negative;
    r
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Decimal,
    Percent,
    Currency,
    Unit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Grouping {
    Always,
    Auto,
    Min2,
    Off,
}

/// Los slots de un Intl.NumberFormat.
#[derive(Debug, Clone)]
pub struct NumberFormat {
    pub locale: String,
    pub data_locale: String,
    pub numbering_system: String,
    pub style: Style,
    pub currency: Option<String>,
    pub currency_display: String,
    pub currency_sign: String,
    pub unit: Option<String>,
    pub unit_display: String,
    pub notation: String,
    pub compact_display: Option<String>,
    pub use_grouping: Grouping,
    pub sign_display: String,
    pub digits: DigitOptions,
    pub bound_format: Option<Gc<Object>>,
}

pub fn init(agent: &mut Agent, realm: RealmId, intl: Gc<Object>) {
    let (c, p) = intl_constructor(
        agent,
        realm,
        intl,
        "NumberFormat",
        "Intl.NumberFormat.prototype",
        0,
        constructor,
    );
    crate::builtins::web::registry_set(agent, realm, "Intl.NumberFormat", c);
    crate::builtins::method(agent, c, "supportedLocalesOf", 1, supported_locales_of);
    crate::builtins::accessor(
        agent,
        p,
        PropertyKey::from("format"),
        "format",
        Some(get_format),
        None,
    );
    crate::builtins::method(agent, p, "formatToParts", 1, format_to_parts);
    crate::builtins::method(agent, p, "formatRange", 2, format_range);
    crate::builtins::method(agent, p, "formatRangeToParts", 2, format_range_to_parts);
    crate::builtins::method(agent, p, "resolvedOptions", 0, resolved_options);
}

/// IsWellFormedCurrencyCode (§6.3.1).
fn is_currency_code(s: &str) -> bool {
    s.len() == 3 && s.bytes().all(|b| b.is_ascii_alphabetic())
}

/// IsWellFormedUnitIdentifier (§6.6.3).
pub fn is_unit_identifier(s: &str) -> bool {
    if SANCTIONED_UNITS.contains(&s) {
        return true;
    }
    match s.split_once("-per-") {
        Some((a, b)) => SANCTIONED_UNITS.contains(&a) && SANCTIONED_UNITS.contains(&b),
        None => false,
    }
}

/// CurrencyDigits ( currency ) (§16.5.1).
fn currency_digits(c: &str) -> u32 {
    cldr::CURRENCY_DIGITS
        .iter()
        .find(|(k, _)| *k == c)
        .map_or(2, |(_, d)| u32::from(*d))
}

/// El símbolo `[[FallbackSymbol]]` de %Intl% (§4.3 nota 1), guardado en los slots del
/// constructor.
pub(crate) fn fallback_symbol(agent: &mut Agent) -> Gc<crate::value::Symbol> {
    let realm = agent.current_realm;
    let intl = crate::builtins::web::registry_get(agent, realm, "Intl");
    if let Some(i) = intl
        && let Some(PropertyKey::Symbol(s)) = agent.heap[i]
            .props
            .keys()
            .into_iter()
            .find(|k| matches!(k, PropertyKey::Symbol(s) if agent.heap[*s].description.as_ref().is_some_and(|d| d.to_string_lossy() == "IntlLegacyConstructedSymbol") && agent.heap[*s].private))
    {
        return s;
    }
    let s = agent.heap.alloc(crate::value::Symbol {
        description: Some(JsString::from("IntlLegacyConstructedSymbol")),
        private: true,
    });
    if let Some(i) = intl {
        agent.put_data(
            i,
            PropertyKey::Symbol(s),
            Value::Undefined,
            false,
            false,
            false,
        );
    }
    s
}

/// ChainNumberFormat / ChainDateTimeFormat (§4.3 nota 1): llamada sin `new` con un `this`
/// que hereda del prototipo.
pub(crate) fn chain_legacy(
    agent: &mut Agent,
    this: &Value,
    ctor: Gc<Object>,
    created: Gc<Object>,
) -> JsResult<Option<Value>> {
    if !this.is_object() {
        return Ok(None);
    }
    if !agent.ordinary_has_instance(&Value::Object(ctor), this)? {
        return Ok(None);
    }
    let Value::Object(t) = this else {
        return Ok(None);
    };
    let sym = fallback_symbol(agent);
    agent.define_property_or_throw(
        *t,
        PropertyKey::Symbol(sym),
        crate::object::PropertyDescriptor::data(Value::Object(created), false, false, false),
    )?;
    Ok(Some(this.clone()))
}

/// Intl.NumberFormat ( [ locales [ , options ] ] ) (§16.1.1).
fn constructor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let nt = a.new_target.unwrap_or(a.callee);
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
    // SetNumberFormatUnitOptions (§16.1.3).
    let style = get_string_option(
        agent,
        options,
        "style",
        &["decimal", "percent", "currency", "unit"],
        Some("decimal"),
    )?
    .unwrap_or_default();
    let currency = get_string_option(agent, options, "currency", &[], None)?;
    match &currency {
        None if style == "currency" => {
            return agent.type_error("Currency code is required with currency style.");
        }
        Some(c) if !is_currency_code(c) => {
            return agent.range_error(&format!("Invalid currency code : {c}"));
        }
        _ => {}
    }
    let currency_display = get_string_option(
        agent,
        options,
        "currencyDisplay",
        &["code", "symbol", "narrowSymbol", "name"],
        Some("symbol"),
    )?
    .unwrap_or_default();
    let currency_sign = get_string_option(
        agent,
        options,
        "currencySign",
        &["standard", "accounting"],
        Some("standard"),
    )?
    .unwrap_or_default();
    let unit = get_string_option(agent, options, "unit", &[], None)?;
    match &unit {
        None if style == "unit" => return agent.type_error("Unit is required with unit style."),
        Some(u) if !is_unit_identifier(u) => {
            return agent.range_error(&format!("Invalid unit argument : {u}"));
        }
        _ => {}
    }
    let unit_display = get_string_option(
        agent,
        options,
        "unitDisplay",
        &["short", "narrow", "long"],
        Some("short"),
    )?
    .unwrap_or_default();
    let style_v = match style.as_str() {
        "percent" => Style::Percent,
        "currency" => Style::Currency,
        "unit" => Style::Unit,
        _ => Style::Decimal,
    };
    let notation = get_string_option(
        agent,
        options,
        "notation",
        &["standard", "scientific", "engineering", "compact"],
        Some("standard"),
    )?
    .unwrap_or_default();
    let (mnfd, mxfd) = if style_v == Style::Currency && notation == "standard" {
        let c = currency_digits(&currency.clone().unwrap_or_default().to_ascii_uppercase());
        (c, c)
    } else if style_v == Style::Percent {
        (0, 0)
    } else {
        (0, 3)
    };
    let digits = set_digit_options(agent, options, mnfd, mxfd, &notation)?;
    let compact_display = get_string_option(
        agent,
        options,
        "compactDisplay",
        &["short", "long"],
        Some("short"),
    )?;
    let default_grouping = if notation == "compact" {
        "min2"
    } else {
        "auto"
    };
    // GetBooleanOrStringNumberFormatOption (§16.5.?).
    let ug = agent.get_str(options, "useGrouping")?;
    let use_grouping = match ug {
        Value::Undefined => default_grouping.to_owned(),
        Value::Boolean(true) => "always".to_owned(),
        v if !crate::abstract_ops::conversion::to_boolean(agent, &v) => "false".to_owned(),
        v => {
            let s = agent.to_string(&v)?.to_string_lossy();
            if !["min2", "auto", "always", "true", "false"].contains(&s.as_str()) {
                return agent
                    .range_error(&format!("Value {s} out of range for useGrouping option"));
            }
            if s == "true" || s == "false" {
                default_grouping.to_owned()
            } else {
                s
            }
        }
    };
    let use_grouping = match use_grouping.as_str() {
        "always" => Grouping::Always,
        "min2" => Grouping::Min2,
        "auto" => Grouping::Auto,
        _ => Grouping::Off,
    };
    let sign_display = get_string_option(
        agent,
        options,
        "signDisplay",
        &["auto", "never", "always", "exceptZero", "negative"],
        Some("auto"),
    )?
    .unwrap_or_default();
    let nf = NumberFormat {
        locale: r.locale.clone(),
        data_locale: r.data_locale.clone(),
        numbering_system: r.get("nu").unwrap_or("latn").to_owned(),
        style: style_v,
        currency: if style_v == Style::Currency {
            currency.map(|c| c.to_ascii_uppercase())
        } else {
            None
        },
        currency_display,
        currency_sign,
        unit: if style_v == Style::Unit { unit } else { None },
        unit_display,
        compact_display: if notation == "compact" {
            compact_display
        } else {
            None
        },
        notation,
        use_grouping,
        sign_display,
        digits,
        bound_format: None,
    };
    let o = create_intl_object(
        agent,
        nt,
        "Intl.NumberFormat.prototype",
        IntlData::NumberFormat(Box::new(nf)),
    )?;
    if a.new_target.is_none()
        && let Some(v) = chain_legacy(agent, &a.this, a.callee, o)?
    {
        return Ok(v);
    }
    Ok(Value::Object(o))
}

/// UnwrapNumberFormat (§16.5.?): con el `[[FallbackSymbol]]` del modo legacy.
fn unwrap(agent: &mut Agent, v: &Value, method: &str) -> JsResult<Gc<Object>> {
    let Value::Object(o) = v else {
        return agent.type_error(&format!(
            "Method Intl.NumberFormat.prototype.{method} called on incompatible receiver"
        ));
    };
    if matches!(intl_data(agent, v), Some(IntlData::NumberFormat(_))) {
        return Ok(*o);
    }
    let realm = agent.current_realm;
    if let Some(ctor) = crate::builtins::web::registry_get(agent, realm, "Intl.NumberFormat")
        && agent.ordinary_has_instance(&Value::Object(ctor), v)?
    {
        let sym = fallback_symbol(agent);
        let inner = agent.get(*o, &PropertyKey::Symbol(sym), v.clone())?;
        if let Value::Object(i) = inner
            && matches!(intl_data(agent, &inner), Some(IntlData::NumberFormat(_)))
        {
            return Ok(i);
        }
    }
    agent.type_error(&format!(
        "Method Intl.NumberFormat.prototype.{method} called on incompatible receiver"
    ))
}

fn nf_of(agent: &Agent, o: Gc<Object>) -> NumberFormat {
    match intl_data(agent, &Value::Object(o)) {
        Some(IntlData::NumberFormat(n)) => (**n).clone(),
        _ => unreachable!("NumberFormat"),
    }
}

/// ToIntlMathematicalValue ( value ) (§16.5.16).
pub fn to_math_value(agent: &mut Agent, v: &Value) -> JsResult<MathValue> {
    let prim = agent.to_primitive(v, crate::abstract_ops::conversion::Hint::Number)?;
    Ok(match &prim {
        Value::BigInt(b) => MathValue::Finite(Decimal::from_bigint(&agent.heap[*b])),
        Value::String(s) => parse_str(&s.to_string_lossy()),
        _ => {
            let n = agent.to_number(&prim)?;
            from_number(n)
        }
    })
}

pub fn from_number(n: f64) -> MathValue {
    if n.is_nan() {
        MathValue::NaN
    } else if n.is_infinite() {
        MathValue::Infinity(n < 0.0)
    } else {
        MathValue::Finite(Decimal::from_f64(n))
    }
}

/// get Intl.NumberFormat.prototype.format (§16.3.3).
fn get_format(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = unwrap(agent, &a.this, "format")?;
    if let Some(IntlData::NumberFormat(n)) = intl_data(agent, &Value::Object(o))
        && let Some(f) = n.bound_format
    {
        return Ok(Value::Object(f));
    }
    let f = agent.create_builtin_with_slots(number_format_function, 1, "", vec![Value::Object(o)]);
    if let Some(IntlData::NumberFormat(n)) = intl_data_mut(agent, o) {
        n.bound_format = Some(f);
    }
    Ok(Value::Object(f))
}

/// Number Format Functions (§16.5.2).
fn number_format_function(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(Value::Object(o)) = crate::builtins::slots(agent, a.callee).into_iter().next() else {
        return Ok(Value::Undefined);
    };
    let nf = nf_of(agent, o);
    let x = to_math_value(agent, &a.arg(0))?;
    Ok(str_value(&format_to_string(&nf, &x)))
}

/// FormatNumeric (§16.5.?).
pub fn format_to_string(nf: &NumberFormat, x: &MathValue) -> String {
    partition(nf, x).into_iter().map(|(_, v)| v).collect()
}

fn parts_array(agent: &mut Agent, parts: &[(&str, String, Option<&str>)]) -> JsResult<Value> {
    let mut vals = Vec::with_capacity(parts.len());
    for (t, v, source) in parts {
        let o = agent.new_plain_object();
        agent.create_data_property_or_throw(o, PropertyKey::from("type"), str_value(t))?;
        agent.create_data_property_or_throw(o, PropertyKey::from("value"), str_value(v))?;
        if let Some(s) = source {
            agent.create_data_property_or_throw(o, PropertyKey::from("source"), str_value(s))?;
        }
        vals.push(Value::Object(o));
    }
    Ok(Value::Object(agent.create_array_from_list(&vals)?))
}

/// Intl.NumberFormat.prototype.formatToParts ( value ) (§16.3.4).
fn format_to_parts(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_nf(agent, &a.this, "formatToParts")?;
    let nf = nf_of(agent, o);
    let x = to_math_value(agent, &a.arg(0))?;
    let parts: Vec<(&str, String, Option<&str>)> = partition(&nf, &x)
        .into_iter()
        .map(|(t, v)| (t, v, None))
        .collect();
    parts_array(agent, &parts)
}

fn this_nf(agent: &mut Agent, this: &Value, method: &str) -> JsResult<Gc<Object>> {
    match (this, intl_data(agent, this)) {
        (Value::Object(o), Some(IntlData::NumberFormat(_))) => Ok(*o),
        _ => agent.type_error(&format!(
            "Method Intl.NumberFormat.prototype.{method} called on incompatible receiver"
        )),
    }
}

fn range_values(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<(MathValue, MathValue)> {
    let (s, e) = (a.arg(0), a.arg(1));
    if s.is_undefined() || e.is_undefined() {
        return agent.type_error("start and end must not be undefined");
    }
    let x = to_math_value(agent, &s)?;
    let y = to_math_value(agent, &e)?;
    if x == MathValue::NaN || y == MathValue::NaN {
        return agent.range_error("Invalid start or end: NaN");
    }
    Ok((x, y))
}

/// PartitionNumberRangePattern (§16.5.?): las partes con su `source`.
fn range_parts(
    nf: &NumberFormat,
    x: &MathValue,
    y: &MathValue,
) -> Vec<(&'static str, String, Option<&'static str>)> {
    let xp = partition(nf, x);
    let yp = partition(nf, y);
    let xs: String = xp.iter().map(|p| p.1.as_str()).collect();
    let ys: String = yp.iter().map(|p| p.1.as_str()).collect();
    let data = locale::data_for(&nf.data_locale);
    if xs == ys {
        // FormatApproximately: el signo aproximado antes del número.
        let mut out: Vec<(&'static str, String, Option<&'static str>)> = xp
            .into_iter()
            .map(|(t, v)| (t, v, Some("shared")))
            .collect();
        let at = out
            .iter()
            .position(|(t, _, _)| {
                matches!(
                    *t,
                    "integer" | "nan" | "infinity" | "minusSign" | "plusSign" | "currency"
                )
            })
            .unwrap_or(0);
        out.insert(
            at,
            (
                "approximatelySign",
                data.numbers.approximately.to_owned(),
                Some("shared"),
            ),
        );
        return out;
    }
    let mut sep = data
        .numbers
        .range_pattern
        .replace("{0}", "")
        .replace("{1}", "");
    let digit_edge = |s: &str, end: bool| {
        let c = if end {
            s.chars().last()
        } else {
            s.chars().next()
        };
        c.is_some_and(|c| c.is_numeric())
    };
    if !sep.contains(' ') && (!digit_edge(&xs, true) || !digit_edge(&ys, false)) {
        sep = format!(" {sep} ");
    }
    let mut out: Vec<(&'static str, String, Option<&'static str>)> = xp
        .into_iter()
        .map(|(t, v)| (t, v, Some("startRange")))
        .collect();
    out.push(("literal", sep, Some("shared")));
    out.extend(yp.into_iter().map(|(t, v)| (t, v, Some("endRange"))));
    out
}

/// Intl.NumberFormat.prototype.formatRange ( start, end ) (§16.3.5).
fn format_range(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_nf(agent, &a.this, "formatRange")?;
    let nf = nf_of(agent, o);
    let (x, y) = range_values(agent, a)?;
    let s: String = range_parts(&nf, &x, &y).into_iter().map(|p| p.1).collect();
    Ok(str_value(&s))
}

/// Intl.NumberFormat.prototype.formatRangeToParts ( start, end ) (§16.3.6).
fn format_range_to_parts(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_nf(agent, &a.this, "formatRangeToParts")?;
    let nf = nf_of(agent, o);
    let (x, y) = range_values(agent, a)?;
    let parts = range_parts(&nf, &x, &y);
    parts_array(agent, &parts)
}

/// Intl.NumberFormat.prototype.resolvedOptions ( ) (§16.3.7).
fn resolved_options(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = unwrap(agent, &a.this, "resolvedOptions")?;
    let nf = nf_of(agent, o);
    let r = agent.new_plain_object();
    let set = |agent: &mut Agent, k: &str, v: Value| {
        agent.create_data_property_or_throw(r, PropertyKey::from(k), v)
    };
    set(agent, "locale", str_value(&nf.locale))?;
    set(agent, "numberingSystem", str_value(&nf.numbering_system))?;
    let style = match nf.style {
        Style::Decimal => "decimal",
        Style::Percent => "percent",
        Style::Currency => "currency",
        Style::Unit => "unit",
    };
    set(agent, "style", str_value(style))?;
    if let Some(c) = &nf.currency {
        set(agent, "currency", str_value(c))?;
        set(agent, "currencyDisplay", str_value(&nf.currency_display))?;
        set(agent, "currencySign", str_value(&nf.currency_sign))?;
    }
    if let Some(u) = &nf.unit {
        set(agent, "unit", str_value(u))?;
        set(agent, "unitDisplay", str_value(&nf.unit_display))?;
    }
    nf.digits.write_resolved(agent, r)?;
    let ug = match nf.use_grouping {
        Grouping::Always => str_value("always"),
        Grouping::Auto => str_value("auto"),
        Grouping::Min2 => str_value("min2"),
        Grouping::Off => Value::Boolean(false),
    };
    set(agent, "useGrouping", ug)?;
    set(agent, "notation", str_value(&nf.notation))?;
    if let Some(c) = &nf.compact_display {
        set(agent, "compactDisplay", str_value(c))?;
    }
    set(agent, "signDisplay", str_value(&nf.sign_display))?;
    nf.digits.write_rounding(agent, r)?;
    Ok(Value::Object(r))
}

fn supported_locales_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let requested = locale::canonicalize_locale_list(agent, &a.arg(0))?;
    locale::supported_locales(agent, &requested, &a.arg(1))
}

// --- PartitionNumberPattern (§16.5.4) ---

/// Un elemento de un patrón de número.
#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Lit(String),
    Number,
    Currency,
    Percent,
    Sign,
    Unit(String),
    Compact(String),
}

/// Un patrón de CLDR (`¤#,##0.00`, `#,##0 %`, `0K`) en elementos; `None` el subpatrón negativo
/// explícito si hay.
fn parse_pattern(p: &str) -> (Vec<Tok>, Option<Vec<Tok>>) {
    let (pos, neg) = match p.split_once(';') {
        Some((a, b)) => (a, Some(b)),
        None => (p, None),
    };
    (sub_pattern(pos), neg.map(sub_pattern))
}

fn sub_pattern(p: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut lit = String::new();
    let mut in_number = false;
    let mut number_done = false;
    let mut quoted = false;
    let flush = |lit: &mut String, out: &mut Vec<Tok>| {
        if !lit.is_empty() {
            out.push(Tok::Lit(std::mem::take(lit)));
        }
    };
    for c in p.chars() {
        if c == '\'' {
            quoted = !quoted;
            continue;
        }
        if quoted {
            lit.push(c);
            continue;
        }
        match c {
            '#' | '0' | ',' | '.' | '@' if !number_done => {
                if !in_number {
                    flush(&mut lit, &mut out);
                    out.push(Tok::Number);
                    in_number = true;
                }
            }
            _ => {
                if in_number {
                    in_number = false;
                    number_done = true;
                }
                match c {
                    '¤' => {
                        flush(&mut lit, &mut out);
                        out.push(Tok::Currency);
                    }
                    '%' => {
                        flush(&mut lit, &mut out);
                        out.push(Tok::Percent);
                    }
                    '-' => {
                        flush(&mut lit, &mut out);
                        out.push(Tok::Sign);
                    }
                    _ => lit.push(c),
                }
            }
        }
    }
    flush(&mut lit, &mut out);
    out
}

/// Un patrón con `{0}` (unidades, nombres de moneda): el número y el texto como unidad.
fn unit_pattern(p: &str, unit_type: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let (before, after) = p.split_once("{0}").unwrap_or(("", p));
    let push_text = |t: &str, out: &mut Vec<Tok>| {
        if t.is_empty() {
            return;
        }
        let trimmed_start = t.trim_start_matches(char::is_whitespace);
        let lead = &t[..t.len() - trimmed_start.len()];
        let core = trimmed_start.trim_end_matches(char::is_whitespace);
        let trail = &trimmed_start[core.len()..];
        if !lead.is_empty() {
            out.push(Tok::Lit(lead.to_owned()));
        }
        if !core.is_empty() {
            out.push(if unit_type == "currency" {
                Tok::Unit(format!("\u{0}{core}"))
            } else {
                Tok::Unit(core.to_owned())
            });
        }
        if !trail.is_empty() {
            out.push(Tok::Lit(trail.to_owned()));
        }
    };
    push_text(before, &mut out);
    out.push(Tok::Number);
    push_text(after, &mut out);
    out
}

/// ComputeExponentForMagnitude (§16.5.18) para la notación compacta: (exponente, patrón).
fn compact_exponent(nf: &NumberFormat, magnitude: i64) -> i64 {
    let data = &locale::data_for(&nf.data_locale).numbers;
    let table = compact_table(nf, data);
    let max = table.iter().map(|r| i64::from(r.0)).max().unwrap_or(0);
    let m = magnitude.min(max);
    match table.iter().find(|r| i64::from(r.0) == m) {
        Some(&(mag, _, pat)) => {
            if pat == "0" {
                return 0;
            }
            let zeros = pat.chars().filter(|&c| c == '0').count() as i64;
            i64::from(mag) - (zeros - 1)
        }
        None => 0,
    }
}

fn compact_table(nf: &NumberFormat, data: &'static cldr::NumberData) -> cldr::Compact {
    if nf.style == Style::Currency {
        data.compact_currency
    } else if nf.compact_display.as_deref() == Some("long") {
        data.compact_long
    } else {
        data.compact_short
    }
}

/// ComputeExponent ( numberFormat, x ) (§16.5.17).
fn compute_exponent(nf: &NumberFormat, x: &Decimal) -> (i64, i64) {
    if x.is_zero() {
        return (0, 0);
    }
    let magnitude = x.magnitude();
    let for_mag = |m: i64| -> i64 {
        match nf.notation.as_str() {
            "scientific" => m,
            "engineering" => m.div_euclid(3) * 3,
            "compact" => compact_exponent(nf, m),
            _ => 0,
        }
    };
    let exponent = for_mag(magnitude);
    let r = format_numeric_to_string(&nf.digits, &x.scale(-exponent));
    if r.rounded.is_zero() {
        return (exponent, magnitude);
    }
    if r.rounded.magnitude() == magnitude - exponent {
        return (exponent, magnitude);
    }
    (for_mag(magnitude + 1), magnitude + 1)
}

/// PartitionNumberPattern: las partes `(tipo, valor)`.
pub fn partition(nf: &NumberFormat, x: &MathValue) -> Vec<(&'static str, String)> {
    let data = &locale::data_for(&nf.data_locale).numbers;
    // El número: (string formateado, valor redondeado, exponente).
    let (n_str, rounded_neg, rounded_zero, exponent, special) = match x {
        MathValue::NaN => (data.nan.to_owned(), false, false, 0, Some("nan")),
        MathValue::Infinity(neg) => (data.infinity.to_owned(), *neg, false, 0, Some("infinity")),
        MathValue::Finite(d) => {
            let mut v = d.clone();
            if nf.style == Style::Percent {
                v = v.scale(2);
            }
            let (exponent, _) = compute_exponent(nf, &v);
            let v = v.scale(-exponent);
            let r = format_numeric_to_string(&nf.digits, &v);
            (r.string, d.negative, r.rounded.is_zero(), exponent, None)
        }
    };
    // GetNumberFormatPattern: el signo según signDisplay.
    let is_nan = special == Some("nan");
    let sign: Option<bool> = match nf.sign_display.as_str() {
        "never" => None,
        "always" => Some(rounded_neg && !is_nan),
        "exceptZero" => {
            if is_nan || rounded_zero {
                None
            } else {
                Some(rounded_neg)
            }
        }
        "negative" => {
            if rounded_neg && !rounded_zero && !is_nan {
                Some(true)
            } else {
                None
            }
        }
        _ => {
            if rounded_neg && !is_nan {
                Some(true)
            } else {
                None
            }
        }
    };
    let sign = sign.map(|neg| if neg { data.minus } else { data.plus });
    // El patrón del estilo.
    let currency = nf.currency.clone().unwrap_or_default();
    let cur_entry = data.currencies.iter().find(|c| c.0 == currency);
    let currency_text = match nf.currency_display.as_str() {
        "code" => currency.clone(),
        "narrowSymbol" => cur_entry.map_or(currency.clone(), |c| c.2.to_owned()),
        _ => cur_entry.map_or(currency.clone(), |c| c.1.to_owned()),
    };
    let plural_ops = Operands::from_str(&n_str, exponent);
    let category = select_category(&nf.data_locale, false, &plural_ops);
    let compact_text = if nf.notation == "compact" && exponent != 0 {
        let table = compact_table(nf, data);
        let mag = (n_str
            .split('.')
            .next()
            .unwrap_or("")
            .trim_start_matches('-')
            .len() as i64
            - 1
            + exponent)
            .min(table.iter().map(|r| i64::from(r.0)).max().unwrap_or(0));
        table
            .iter()
            .find(|r| i64::from(r.0) == mag && r.1 == category)
            .or_else(|| {
                table
                    .iter()
                    .find(|r| i64::from(r.0) == mag && r.1 == "other")
            })
            .map(|r| r.2)
    } else {
        None
    };
    let (mut toks, negative_toks): (Vec<Tok>, Option<Vec<Tok>>) = match nf.style {
        Style::Decimal | Style::Unit => (vec![Tok::Number], None),
        Style::Percent => parse_pattern(data.percent_pattern),
        Style::Currency => {
            if nf.currency_display == "name" {
                let name = cur_entry.map_or(currency.clone(), |c| {
                    if category == "one" {
                        c.3.to_owned()
                    } else {
                        c.4.to_owned()
                    }
                });
                let pat = if category == "one" {
                    data.currency_unit_one
                } else {
                    data.currency_unit_other
                };
                let pat = pat.replace("{1}", &name);
                (unit_pattern(&pat, "currency"), None)
            } else {
                let accounting = nf.currency_sign == "accounting";
                let alpha = currency_text
                    .chars()
                    .last()
                    .is_some_and(char::is_alphabetic)
                    || currency_text
                        .chars()
                        .next()
                        .is_some_and(char::is_alphabetic);
                let p = match (accounting, alpha) {
                    (true, true) => data.accounting_alpha_pattern,
                    (true, false) => data.accounting_pattern,
                    (false, true) => data.currency_alpha_pattern,
                    (false, false) => data.currency_pattern,
                };
                parse_pattern(p)
            }
        }
    };
    // Compacto: el número del patrón compacto (con su símbolo).
    if let Some(cp) = compact_text {
        let ctoks = sub_pattern(cp);
        let mut expanded = Vec::new();
        for t in &ctoks {
            match t {
                Tok::Number => expanded.push(Tok::Number),
                Tok::Lit(s) => {
                    let core = s.trim();
                    if core.is_empty() {
                        expanded.push(Tok::Lit(s.clone()));
                    } else {
                        let lead = &s[..s.len() - s.trim_start().len()];
                        let trail = &s[s.trim_end().len()..];
                        if !lead.is_empty() {
                            expanded.push(Tok::Lit(lead.to_owned()));
                        }
                        expanded.push(Tok::Compact(core.to_owned()));
                        if !trail.is_empty() {
                            expanded.push(Tok::Lit(trail.to_owned()));
                        }
                    }
                }
                Tok::Currency => expanded.push(Tok::Currency),
                other => expanded.push(other.clone()),
            }
        }
        if nf.style == Style::Currency {
            toks = expanded;
        } else {
            toks = replace_number(&toks, &expanded);
        }
    }
    // Unidades: el patrón de la unidad alrededor del número.
    if nf.style == Style::Unit
        && let Some(u) = &nf.unit
    {
        let utoks = unit_tokens(data, u, &nf.unit_display, category);
        toks = replace_number(&toks, &utoks);
    }
    // El signo: el subpatrón negativo explícito (contable) o el signo antes del patrón.
    let explicit_negative = if compact_text.is_some() || nf.style == Style::Unit {
        None
    } else {
        negative_toks
    };
    let toks: Vec<Tok> = match sign {
        None => toks.into_iter().filter(|t| *t != Tok::Sign).collect(),
        Some(s) if s == data.minus && explicit_negative.is_some() => {
            explicit_negative.unwrap_or_default()
        }
        Some(_) => {
            let mut t = vec![Tok::Sign];
            t.extend(toks.into_iter().filter(|t| *t != Tok::Sign));
            t
        }
    };
    let mut out: Vec<(&'static str, String)> = Vec::new();
    for t in toks {
        match t {
            Tok::Lit(s) => out.push(("literal", s)),
            Tok::Sign => match sign {
                Some(s) if s == data.minus => out.push(("minusSign", s.to_owned())),
                Some(s) => out.push(("plusSign", s.to_owned())),
                None => {}
            },
            Tok::Percent => out.push(("percentSign", data.percent.to_owned())),
            Tok::Currency => out.push(("currency", currency_text.clone())),
            Tok::Unit(u) => match u.strip_prefix('\u{0}') {
                Some(c) => out.push(("currency", c.to_owned())),
                None => out.push(("unit", u)),
            },
            Tok::Compact(c) => out.push(("compact", c)),
            Tok::Number => {
                if let Some(kind) = special {
                    out.push((kind, n_str.clone()));
                } else {
                    number_parts(nf, data, &n_str, exponent, &mut out);
                }
            }
        }
    }
    out
}

fn replace_number(toks: &[Tok], with: &[Tok]) -> Vec<Tok> {
    let mut out = Vec::new();
    for t in toks {
        if *t == Tok::Number {
            out.extend(with.iter().cloned());
        } else {
            out.push(t.clone());
        }
    }
    out
}

/// El patrón de una unidad (simple o `x-per-y`) con la categoría de plural.
fn unit_tokens(data: &cldr::NumberData, unit: &str, display: &str, category: &str) -> Vec<Tok> {
    let find = |u: &str| data.units.iter().find(|e| e.0 == u && e.1 == display);
    let simple = |u: &str| -> String {
        match find(u) {
            Some(e) => {
                if category == "one" {
                    e.2.to_owned()
                } else {
                    e.3.to_owned()
                }
            }
            None => format!("{{0}} {u}"),
        }
    };
    match unit.split_once("-per-") {
        None => unit_pattern(&simple(unit), "unit"),
        Some((num, den)) => {
            let numerator = simple(num);
            let per = find(den).map(|e| e.4).filter(|p| !p.is_empty());
            let combined = match per {
                Some(p) => p.replace("{0}", &numerator),
                None => {
                    let w = match display {
                        "long" => 0,
                        "narrow" => 2,
                        _ => 1,
                    };
                    let den_name = simple(den).replace("{0}", "").trim().to_owned();
                    data.per_patterns[w]
                        .replace("{0}", &numerator)
                        .replace("{1}", &den_name)
                }
            };
            unit_pattern(&combined, "unit")
        }
    }
}

/// Las partes del número (PartitionNotationSubPattern, §16.5.5): enteros con grupos, decimal,
/// fracción y el exponente científico.
fn number_parts(
    nf: &NumberFormat,
    data: &cldr::NumberData,
    n: &str,
    exponent: i64,
    out: &mut Vec<(&'static str, String)>,
) {
    let digits = numbering_system_digits(&nf.numbering_system);
    let translit = |s: &str| -> String {
        match &digits {
            Some(d) if nf.numbering_system != "latn" => s
                .chars()
                .map(|c| c.to_digit(10).map_or(c, |v| d[v as usize]))
                .collect(),
            _ => s.to_owned(),
        }
    };
    let (int, frac) = match n.split_once('.') {
        Some((a, b)) => (a, Some(b)),
        None => (n, None),
    };
    let group = match nf.use_grouping {
        Grouping::Off => false,
        Grouping::Always => int.len() > 3,
        Grouping::Min2 => int.len() >= 5,
        Grouping::Auto => {
            if nf.notation == "compact" {
                int.len() >= 5
            } else {
                int.len() >= 3 + usize::from(data.min_grouping)
            }
        }
    };
    if group {
        let first = int.len() % 3;
        let mut chunks: Vec<&str> = Vec::new();
        if first > 0 {
            chunks.push(&int[..first]);
        }
        let mut i = first;
        while i < int.len() {
            chunks.push(&int[i..i + 3]);
            i += 3;
        }
        for (k, c) in chunks.iter().enumerate() {
            if k > 0 {
                out.push(("group", data.group.to_owned()));
            }
            out.push(("integer", translit(c)));
        }
    } else {
        out.push(("integer", translit(int)));
    }
    if let Some(f) = frac {
        out.push(("decimal", data.decimal.to_owned()));
        out.push(("fraction", translit(f)));
    }
    if nf.notation == "scientific" || nf.notation == "engineering" {
        out.push(("exponentSeparator", data.exponential.to_owned()));
        if exponent < 0 {
            out.push(("exponentMinusSign", data.minus.to_owned()));
        }
        out.push(("exponentInteger", translit(&exponent.abs().to_string())));
    }
}

/// Un NumberFormat por defecto para otros built-ins (Number.prototype.toLocaleString).
pub fn create_for(agent: &mut Agent, locales: &Value, options: &Value) -> JsResult<NumberFormat> {
    let ctor = crate::builtins::web::registry_get(agent, agent.current_realm, "Intl.NumberFormat")
        .expect("Intl.NumberFormat");
    let v = agent.construct(ctor, &[locales.clone(), options.clone()], None)?;
    match intl_data(agent, &v) {
        Some(IntlData::NumberFormat(n)) => Ok((**n).clone()),
        _ => unreachable!("NumberFormat"),
    }
}
