//! Intl.Collator (ECMA-402 §10) y CompareStrings con una colación de varios niveles al
//! estilo del UCA (UTS #10) para el alfabeto latino: primario (la letra base, sin acentos ni
//! mayúsculas), secundario (los diacríticos, tras NFD), terciario (mayúsculas/minúsculas con
//! `caseFirst`), con comparación numérica (`numeric`) e `ignorePunctuation`.

use std::cmp::Ordering;

use unicode_normalization::UnicodeNormalization;

use crate::builtins::intl::{
    IntlData, coerce_options_to_object, create_intl_object, get_boolean_option, get_string_option,
    intl_constructor, intl_data, intl_data_mut, is_type_sequence, locale,
};
use crate::builtins::web::str_value;
use crate::heap::Gc;
use crate::object::Object;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

/// Los slots de un Intl.Collator.
#[derive(Debug, Clone)]
pub struct Collator {
    pub locale: String,
    pub usage: String,
    pub sensitivity: String,
    pub ignore_punctuation: bool,
    pub collation: String,
    pub numeric: bool,
    pub case_first: String,
    pub bound_compare: Option<Gc<Object>>,
}

pub fn init(agent: &mut Agent, realm: RealmId, intl: Gc<Object>) {
    let (c, p) = intl_constructor(
        agent,
        realm,
        intl,
        "Collator",
        "Intl.Collator.prototype",
        0,
        constructor,
    );
    crate::builtins::web::registry_set(agent, realm, "Intl.Collator", c);
    crate::builtins::method(agent, c, "supportedLocalesOf", 1, supported_locales_of);
    crate::builtins::accessor(
        agent,
        p,
        PropertyKey::from("compare"),
        "compare",
        Some(get_compare),
        None,
    );
    crate::builtins::method(agent, p, "resolvedOptions", 0, resolved_options);
}

/// Intl.Collator ( [ locales [ , options ] ] ) (§10.1.1).
fn constructor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let nt = a.new_target.unwrap_or(a.callee);
    let requested = locale::canonicalize_locale_list(agent, &a.arg(0))?;
    let options = coerce_options_to_object(agent, &a.arg(1))?;
    let usage = get_string_option(agent, options, "usage", &["sort", "search"], Some("sort"))?
        .unwrap_or_default();
    get_string_option(
        agent,
        options,
        "localeMatcher",
        &["lookup", "best fit"],
        Some("best fit"),
    )?;
    let collation = get_string_option(agent, options, "collation", &[], None)?;
    if let Some(c) = &collation
        && !is_type_sequence(c)
    {
        return agent.range_error(&format!("Invalid collation : {c}"));
    }
    let numeric = get_boolean_option(agent, options, "numeric")?.map(|b| b.to_string());
    let case_first = get_string_option(
        agent,
        options,
        "caseFirst",
        &["upper", "lower", "false"],
        None,
    )?;
    let r = locale::resolve_locale(
        &requested,
        &[("co", collation), ("kf", case_first), ("kn", numeric)],
        &["co", "kf", "kn"],
        &|key, _| match key {
            "kn" => vec!["false".into(), "true".into()],
            "kf" => vec!["false".into(), "lower".into(), "upper".into()],
            _ => Vec::new(),
        },
    );
    let sensitivity = get_string_option(
        agent,
        options,
        "sensitivity",
        &["base", "accent", "case", "variant"],
        None,
    )?
    .unwrap_or_else(|| "variant".into());
    let ignore_punctuation =
        get_boolean_option(agent, options, "ignorePunctuation")?.unwrap_or(false);
    let c = Collator {
        locale: r.locale.clone(),
        usage,
        sensitivity,
        ignore_punctuation,
        collation: r.get("co").unwrap_or("default").to_owned(),
        numeric: r.get("kn") == Some("true"),
        case_first: r.get("kf").unwrap_or("false").to_owned(),
        bound_compare: None,
    };
    let o = create_intl_object(
        agent,
        nt,
        "Intl.Collator.prototype",
        IntlData::Collator(Box::new(c)),
    )?;
    Ok(Value::Object(o))
}

fn this_collator(agent: &mut Agent, this: &Value, method: &str) -> JsResult<Gc<Object>> {
    match (this, intl_data(agent, this)) {
        (Value::Object(o), Some(IntlData::Collator(_))) => Ok(*o),
        _ => agent.type_error(&format!(
            "Method Intl.Collator.prototype.{method} called on incompatible receiver"
        )),
    }
}

fn collator_of(agent: &Agent, o: Gc<Object>) -> Collator {
    match intl_data(agent, &Value::Object(o)) {
        Some(IntlData::Collator(c)) => (**c).clone(),
        _ => unreachable!("Collator"),
    }
}

/// get Intl.Collator.prototype.compare (§10.3.3).
fn get_compare(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_collator(agent, &a.this, "compare")?;
    if let Some(IntlData::Collator(c)) = intl_data(agent, &Value::Object(o))
        && let Some(f) = c.bound_compare
    {
        return Ok(Value::Object(f));
    }
    let f = agent.create_builtin_with_slots(compare_function, 2, "", vec![Value::Object(o)]);
    if let Some(IntlData::Collator(c)) = intl_data_mut(agent, o) {
        c.bound_compare = Some(f);
    }
    Ok(Value::Object(f))
}

/// Collator Compare Functions (§10.3.3.1).
fn compare_function(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(Value::Object(o)) = crate::builtins::slots(agent, a.callee).into_iter().next() else {
        return Ok(Value::Number(0.0));
    };
    let c = collator_of(agent, o);
    let x = agent.to_string(&a.arg(0))?.to_string_lossy();
    let y = agent.to_string(&a.arg(1))?.to_string_lossy();
    Ok(Value::Number(match compare_strings(&c, &x, &y) {
        Ordering::Less => -1.0,
        Ordering::Equal => 0.0,
        Ordering::Greater => 1.0,
    }))
}

/// Un elemento de colación: (primario, secundario, terciario).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Primary {
    /// Espacios y puntuación, por code point (antes que todo lo demás).
    Punct(u32),
    /// Un número (con `numeric`): valor como dígitos sin ceros a la izquierda.
    Num(String),
    Digit(u32),
    Letter(u32),
}

impl PartialOrd for Primary {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Primary {
    fn cmp(&self, other: &Self) -> Ordering {
        let rank = |p: &Primary| match p {
            Primary::Punct(_) => 0,
            Primary::Num(_) | Primary::Digit(_) => 1,
            Primary::Letter(_) => 2,
        };
        match (self, other) {
            (Primary::Punct(a), Primary::Punct(b))
            | (Primary::Digit(a), Primary::Digit(b))
            | (Primary::Letter(a), Primary::Letter(b)) => a.cmp(b),
            (Primary::Num(a), Primary::Num(b)) => a.len().cmp(&b.len()).then(a.cmp(b)),
            (Primary::Num(a), Primary::Digit(b)) => {
                let bs = char::from_u32(*b)
                    .map(|c| c.to_string())
                    .unwrap_or_default();
                a.len().cmp(&1).then(a.as_str().cmp(bs.as_str()))
            }
            (Primary::Digit(_), Primary::Num(_)) => other.cmp(self).reverse(),
            _ => rank(self).cmp(&rank(other)),
        }
    }
}

struct Element {
    primary: Primary,
    accents: Vec<char>,
    upper: bool,
}

fn is_punctuation(c: char) -> bool {
    c.is_whitespace()
        || (c.is_ascii_punctuation())
        || matches!(c, '\u{A0}'..='\u{BF}' | '\u{2000}'..='\u{206F}')
}

fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x300..=0x36F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

/// Los elementos de colación de un string (tras NFD).
fn elements(c: &Collator, s: &str) -> Vec<Element> {
    let chars: Vec<char> = s.nfd().collect();
    let mut out: Vec<Element> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if is_mark(ch) {
            if let Some(last) = out.last_mut() {
                last.accents.push(ch);
            }
            i += 1;
            continue;
        }
        if c.ignore_punctuation && is_punctuation(ch) {
            i += 1;
            continue;
        }
        if c.numeric && ch.is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            let digits: String = chars[start..i].iter().collect();
            let trimmed = digits.trim_start_matches('0');
            out.push(Element {
                primary: Primary::Num(if trimmed.is_empty() {
                    "0".into()
                } else {
                    trimmed.into()
                }),
                accents: Vec::new(),
                upper: false,
            });
            continue;
        }
        let lower: char = ch.to_lowercase().next().unwrap_or(ch);
        let upper = ch != lower;
        let primary = if ch.is_numeric() {
            Primary::Digit(ch.to_digit(10).map_or(ch as u32, |d| u32::from(b'0') + d))
        } else if is_punctuation(ch) || !ch.is_alphanumeric() {
            Primary::Punct(ch as u32)
        } else {
            // `ß` ordena como "ss".
            if lower == 'ß' {
                for _ in 0..2 {
                    out.push(Element {
                        primary: Primary::Letter('s' as u32),
                        accents: Vec::new(),
                        upper: false,
                    });
                }
                i += 1;
                continue;
            }
            Primary::Letter(lower as u32)
        };
        out.push(Element {
            primary,
            accents: Vec::new(),
            upper,
        });
        i += 1;
    }
    out
}

/// CompareStrings ( collator, x, y ) (§10.3.3.2).
pub fn compare_strings(c: &Collator, x: &str, y: &str) -> Ordering {
    let a = elements(c, x);
    let b = elements(c, y);
    // Primario.
    let pa: Vec<&Primary> = a.iter().map(|e| &e.primary).collect();
    let pb: Vec<&Primary> = b.iter().map(|e| &e.primary).collect();
    let o = pa.cmp(&pb);
    if o != Ordering::Equal {
        return o;
    }
    let accents = c.sensitivity == "accent" || c.sensitivity == "variant";
    let case = c.sensitivity == "case" || c.sensitivity == "variant";
    if accents {
        let sa: Vec<&Vec<char>> = a.iter().map(|e| &e.accents).collect();
        let sb: Vec<&Vec<char>> = b.iter().map(|e| &e.accents).collect();
        let o = sa.cmp(&sb);
        if o != Ordering::Equal {
            return o;
        }
    }
    if case {
        let upper_first = c.case_first == "upper";
        for (ea, eb) in a.iter().zip(&b) {
            if ea.upper != eb.upper {
                return if ea.upper == upper_first {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }
        }
    }
    if c.sensitivity == "variant" && !c.ignore_punctuation && !c.numeric {
        // Desempate final: los strings canónicamente equivalentes son iguales.
        let na: String = x.nfd().collect();
        let nb: String = y.nfd().collect();
        return na.cmp(&nb);
    }
    Ordering::Equal
}

/// Intl.Collator.prototype.resolvedOptions ( ) (§10.3.4).
fn resolved_options(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_collator(agent, &a.this, "resolvedOptions")?;
    let c = collator_of(agent, o);
    let r = agent.new_plain_object();
    let set = |agent: &mut Agent, k: &str, v: Value| {
        agent.create_data_property_or_throw(r, PropertyKey::from(k), v)
    };
    set(agent, "locale", str_value(&c.locale))?;
    set(agent, "usage", str_value(&c.usage))?;
    set(agent, "sensitivity", str_value(&c.sensitivity))?;
    set(
        agent,
        "ignorePunctuation",
        Value::Boolean(c.ignore_punctuation),
    )?;
    set(agent, "collation", str_value(&c.collation))?;
    set(agent, "numeric", Value::Boolean(c.numeric))?;
    set(agent, "caseFirst", str_value(&c.case_first))?;
    Ok(Value::Object(r))
}

fn supported_locales_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let requested = locale::canonicalize_locale_list(agent, &a.arg(0))?;
    locale::supported_locales(agent, &requested, &a.arg(1))
}

/// Un Collator para String.prototype.localeCompare.
pub fn create_for(agent: &mut Agent, locales: &Value, options: &Value) -> JsResult<Collator> {
    let ctor = crate::builtins::web::registry_get(agent, agent.current_realm, "Intl.Collator")
        .expect("Intl.Collator");
    let v = agent.construct(ctor, &[locales.clone(), options.clone()], None)?;
    match intl_data(agent, &v) {
        Some(IntlData::Collator(c)) => Ok((**c).clone()),
        _ => unreachable!("Collator"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(sensitivity: &str, numeric: bool) -> Collator {
        Collator {
            locale: "en".into(),
            usage: "sort".into(),
            sensitivity: sensitivity.into(),
            ignore_punctuation: false,
            collation: "default".into(),
            numeric,
            case_first: "false".into(),
            bound_compare: None,
        }
    }

    #[test]
    fn ordering() {
        let v = col("variant", false);
        assert_eq!(compare_strings(&v, "a", "b"), Ordering::Less);
        assert_eq!(compare_strings(&v, "a", "A"), Ordering::Less);
        assert_eq!(compare_strings(&v, "a", "á"), Ordering::Less);
        assert_eq!(compare_strings(&v, "á", "b"), Ordering::Less);
        assert_eq!(compare_strings(&v, "\u{e9}", "e\u{301}"), Ordering::Equal);
        let base = col("base", false);
        assert_eq!(compare_strings(&base, "a", "Á"), Ordering::Equal);
        let accent = col("accent", false);
        assert_eq!(compare_strings(&accent, "a", "A"), Ordering::Equal);
        assert_ne!(compare_strings(&accent, "a", "á"), Ordering::Equal);
        let n = col("variant", true);
        assert_eq!(compare_strings(&n, "2", "10"), Ordering::Less);
        assert_eq!(compare_strings(&v, "2", "10"), Ordering::Greater);
    }
}
