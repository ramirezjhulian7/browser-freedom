//! Intl.DisplayNames (ECMA-402 §12) con los nombres de CLDR 48 de `en` y `es`
//! ([`cldr::DISPLAY_NAMES`]): idiomas, regiones, scripts, monedas, calendarios y campos de
//! fecha.

use crate::builtins::intl::{
    IntlData, cldr, create_intl_object, get_string_option, intl_constructor, intl_data,
    is_type_sequence, list_format, locale,
};
use crate::builtins::web::str_value;
use crate::heap::Gc;
use crate::object::Object;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

/// Los slots de un Intl.DisplayNames (§12.4).
#[derive(Debug, Clone)]
pub struct DisplayNames {
    pub locale: String,
    pub data_locale: String,
    pub style: String,
    pub kind: String,
    pub fallback: String,
    /// Solo con `type: "language"`.
    pub language_display: Option<String>,
}

pub fn init(agent: &mut Agent, realm: RealmId, intl: Gc<Object>) {
    let (c, p) = intl_constructor(
        agent,
        realm,
        intl,
        "DisplayNames",
        "Intl.DisplayNames.prototype",
        2,
        constructor,
    );
    crate::builtins::method(agent, c, "supportedLocalesOf", 1, supported_locales_of);
    crate::builtins::method(agent, p, "of", 1, of);
    crate::builtins::method(agent, p, "resolvedOptions", 0, resolved_options);
}

/// Intl.DisplayNames ( locales, options ) (§12.1.1).
fn constructor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor Intl.DisplayNames requires 'new'");
    };
    let obj = create_intl_object(
        agent,
        nt,
        "Intl.DisplayNames.prototype",
        IntlData::DisplayNames(Box::new(DisplayNames {
            locale: String::new(),
            data_locale: String::new(),
            style: String::new(),
            kind: String::new(),
            fallback: String::new(),
            language_display: None,
        })),
    )?;
    let requested = locale::canonicalize_locale_list(agent, &a.arg(0))?;
    if a.arg(1).is_undefined() {
        return agent.type_error("Intl.DisplayNames requires an options object");
    }
    let options = list_format::get_options_object(agent, &a.arg(1))?;
    get_string_option(
        agent,
        options,
        "localeMatcher",
        &["lookup", "best fit"],
        Some("best fit"),
    )?;
    let r = locale::resolve_locale(&requested, &[], &[], &|_, _| Vec::new());
    let style = get_string_option(
        agent,
        options,
        "style",
        &["narrow", "short", "long"],
        Some("long"),
    )?
    .unwrap_or_default();
    let Some(kind) = get_string_option(
        agent,
        options,
        "type",
        &[
            "language",
            "region",
            "script",
            "currency",
            "calendar",
            "dateTimeField",
        ],
        None,
    )?
    else {
        return agent.type_error("Intl.DisplayNames requires the type option");
    };
    let fallback = get_string_option(agent, options, "fallback", &["code", "none"], Some("code"))?
        .unwrap_or_default();
    let language_display = get_string_option(
        agent,
        options,
        "languageDisplay",
        &["dialect", "standard"],
        Some("dialect"),
    )?;
    if let Some(IntlData::DisplayNames(d)) = crate::builtins::intl::intl_data_mut(agent, obj) {
        **d = DisplayNames {
            locale: r.locale,
            data_locale: r.data_locale,
            style,
            language_display: if kind == "language" {
                language_display
            } else {
                None
            },
            kind,
            fallback,
        };
    }
    Ok(Value::Object(obj))
}

fn this_display_names(agent: &mut Agent, this: &Value, method: &str) -> JsResult<DisplayNames> {
    match intl_data(agent, this) {
        Some(IntlData::DisplayNames(d)) => Ok((**d).clone()),
        _ => agent.type_error(&format!(
            "Method Intl.DisplayNames.prototype.{method} called on incompatible receiver"
        )),
    }
}

/// Los códigos válidos de `dateTimeField` (IsValidDateTimeFieldCode, §12.5.2) y su campo en
/// los dateFields de CLDR.
const DATE_TIME_FIELDS: [(&str, &str); 12] = [
    ("era", "era"),
    ("year", "year"),
    ("quarter", "quarter"),
    ("month", "month"),
    ("weekOfYear", "week"),
    ("weekday", "weekday"),
    ("day", "day"),
    ("dayPeriod", "dayperiod"),
    ("hour", "hour"),
    ("minute", "minute"),
    ("second", "second"),
    ("timeZoneName", "zone"),
];

/// CanonicalCodeForDisplayNames ( type, code ) (§12.5.1): `None` es RangeError.
fn canonical_code(kind: &str, code: &str) -> Option<String> {
    let alpha = |s: &str| s.bytes().all(|b| b.is_ascii_alphabetic());
    match kind {
        "language" => {
            // unicode_language_id: sin extensiones ni uso privado.
            let t = locale::parse(code)?;
            if t.unicode.is_some()
                || t.transformed.is_some()
                || !t.others.is_empty()
                || !t.private_use.is_empty()
            {
                return None;
            }
            Some(locale::canonicalize(code))
        }
        "region" => {
            let ok = (code.len() == 2 && alpha(code))
                || (code.len() == 3 && code.bytes().all(|b| b.is_ascii_digit()));
            ok.then(|| code.to_ascii_uppercase())
        }
        "script" => (code.len() == 4 && alpha(code)).then(|| {
            let lower = code.to_ascii_lowercase();
            lower[..1].to_ascii_uppercase() + &lower[1..]
        }),
        "calendar" => is_type_sequence(code).then(|| code.to_ascii_lowercase()),
        "dateTimeField" => DATE_TIME_FIELDS
            .iter()
            .any(|(c, _)| *c == code)
            .then(|| code.to_owned()),
        _ => (code.len() == 3 && alpha(code)).then(|| code.to_ascii_uppercase()),
    }
}

/// Un nombre de la tabla de CLDR.
fn name(loc: &str, kind: &str, code: &str) -> Option<&'static str> {
    let lang = loc.split('-').next().unwrap_or("en");
    let find = |l: &str| {
        let key = (l, kind, code);
        cldr::DISPLAY_NAMES
            .binary_search_by(|(a, b, c, _)| (*a, *b, *c).cmp(&key))
            .ok()
            .map(|i| cldr::DISPLAY_NAMES[i].3)
    };
    find(lang).or_else(|| find("en"))
}

/// Un nombre con el estilo pedido: el `-short` de CLDR si lo hay y el estilo no es `long`.
fn styled(dn: &DisplayNames, kind: &str, code: &str) -> Option<&'static str> {
    if dn.style != "long"
        && let Some(n) = name(&dn.data_locale, &format!("{kind}-short"), code)
    {
        return Some(n);
    }
    name(&dn.data_locale, kind, code)
}

/// El nombre de un idioma: el del idioma (o del dialecto, con `languageDisplay: dialect`)
/// y, entre paréntesis, los nombres del script, la región y las variantes que sobran.
fn language_name(dn: &DisplayNames, code: &str) -> Option<String> {
    let t = locale::parse(code)?;
    let dialect = dn.language_display.as_deref() == Some("dialect");
    let mut script = t.script.as_ref().map(|s| {
        let s = s.to_ascii_lowercase();
        s[..1].to_ascii_uppercase() + &s[1..]
    });
    let mut region = t.region.as_ref().map(|r| r.to_ascii_uppercase());
    let mut base = None;
    if dialect {
        let full = [Some(t.language.clone()), script.clone(), region.clone()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("-");
        if let Some(n) = styled(dn, "language", &full) {
            base = Some(n);
            script = None;
            region = None;
        } else if let Some(r) = &region
            && let Some(n) = styled(dn, "language", &format!("{}-{r}", t.language))
        {
            base = Some(n);
            region = None;
        } else if let Some(s) = &script
            && let Some(n) = styled(dn, "language", &format!("{}-{s}", t.language))
        {
            base = Some(n);
            script = None;
        }
    }
    let base = match base {
        Some(b) => b,
        None => styled(dn, "language", &t.language)?,
    };
    let mut extras: Vec<String> = Vec::new();
    if let Some(s) = script {
        extras.push(styled(dn, "script", &s).map_or(s.clone(), str::to_owned));
    }
    if let Some(r) = region {
        extras.push(styled(dn, "region", &r).map_or(r.clone(), str::to_owned));
    }
    extras.extend(t.variants.iter().map(|v| v.to_ascii_uppercase()));
    if extras.is_empty() {
        return Some(base.to_owned());
    }
    let sep = name(&dn.data_locale, "pattern", "localeSeparator").unwrap_or("{0}, {1}");
    let mut joined = extras[0].clone();
    for e in &extras[1..] {
        joined = sep.replace("{0}", &joined).replace("{1}", e);
    }
    let pattern = name(&dn.data_locale, "pattern", "localePattern").unwrap_or("{0} ({1})");
    Some(pattern.replace("{0}", base).replace("{1}", &joined))
}

/// Intl.DisplayNames.prototype.of ( code ) (§12.3.3).
fn of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let dn = this_display_names(agent, &a.this, "of")?;
    let code = agent.to_string(&a.arg(0))?.to_string_lossy();
    let Some(code) = canonical_code(&dn.kind, &code) else {
        return agent.range_error(&format!(
            "invalid code for Intl.DisplayNames.prototype.of: {code}"
        ));
    };
    let found: Option<String> = match dn.kind.as_str() {
        "language" => language_name(&dn, &code),
        "region" => styled(&dn, "region", &code).map(str::to_owned),
        "script" => styled(&dn, "script", &code).map(str::to_owned),
        "calendar" => {
            let cldr_key = match code.as_str() {
                "gregory" => "gregorian",
                "ethioaa" => "ethiopic-amete-alem",
                c => c,
            };
            name(&dn.data_locale, "calendar", cldr_key).map(str::to_owned)
        }
        "dateTimeField" => {
            let field = DATE_TIME_FIELDS
                .iter()
                .find(|(c, _)| *c == code)
                .map_or("", |(_, f)| *f);
            let key = match dn.style.as_str() {
                "long" => field.to_owned(),
                s => format!("{field}-{s}"),
            };
            name(&dn.data_locale, "field", &key)
                .or_else(|| name(&dn.data_locale, "field", field))
                .map(str::to_owned)
        }
        _ => locale::data_for(&dn.data_locale)
            .numbers
            .currencies
            .iter()
            .find(|c| c.0 == code)
            .map(|c| match dn.style.as_str() {
                "long" => c.5,
                "short" => c.1,
                _ => c.2,
            })
            .filter(|n| !n.is_empty())
            .map(str::to_owned),
    };
    Ok(match found {
        Some(n) => str_value(&n),
        None if dn.fallback == "code" => str_value(&code),
        None => Value::Undefined,
    })
}

/// Intl.DisplayNames.prototype.resolvedOptions ( ) (§12.3.4).
fn resolved_options(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let dn = this_display_names(agent, &a.this, "resolvedOptions")?;
    let o = agent.new_plain_object();
    for (k, v) in [
        ("locale", &dn.locale),
        ("style", &dn.style),
        ("type", &dn.kind),
        ("fallback", &dn.fallback),
    ] {
        agent.create_data_property_or_throw(o, PropertyKey::from(k), str_value(v))?;
    }
    if let Some(ld) = &dn.language_display {
        agent.create_data_property_or_throw(
            o,
            PropertyKey::from("languageDisplay"),
            str_value(ld),
        )?;
    }
    Ok(Value::Object(o))
}

/// Intl.DisplayNames.supportedLocalesOf ( locales [ , options ] ) (§12.2.2).
fn supported_locales_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let requested = locale::canonicalize_locale_list(agent, &a.arg(0))?;
    locale::supported_locales(agent, &requested, &a.arg(1))
}
