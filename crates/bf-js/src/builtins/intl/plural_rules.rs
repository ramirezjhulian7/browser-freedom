//! Intl.PluralRules (ECMA-402 §17) y el evaluador de las reglas de plurales de CLDR (UTS #35
//! Part 3 §5.1: operandos `n i v w f t e c` y la sintaxis de condiciones).

use crate::builtins::intl::decimal::Decimal;
use crate::builtins::intl::number_format::{
    DigitOptions, format_numeric_to_string, set_digit_options,
};
use crate::builtins::intl::{
    IntlData, cldr, coerce_options_to_object, create_intl_object, get_string_option,
    intl_constructor, intl_data, locale,
};
use crate::builtins::web::str_value;
use crate::heap::Gc;
use crate::object::Object;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

/// Los slots de un Intl.PluralRules.
#[derive(Debug, Clone)]
pub struct PluralRules {
    pub locale: String,
    pub ordinal: bool,
    pub notation: String,
    pub compact_display: Option<String>,
    pub digits: DigitOptions,
}

pub fn init(agent: &mut Agent, realm: RealmId, intl: Gc<Object>) {
    let (c, p) = intl_constructor(
        agent,
        realm,
        intl,
        "PluralRules",
        "Intl.PluralRules.prototype",
        0,
        constructor,
    );
    crate::builtins::method(agent, c, "supportedLocalesOf", 1, supported_locales_of);
    crate::builtins::method(agent, p, "select", 1, select);
    crate::builtins::method(agent, p, "selectRange", 2, select_range);
    crate::builtins::method(agent, p, "resolvedOptions", 0, resolved_options);
}

// --- Reglas de CLDR ---

/// Los operandos de una cantidad (§5.1.1), sacados de su forma decimal formateada.
#[derive(Debug, Clone, Copy, Default)]
pub struct Operands {
    pub n: f64,
    pub i: f64,
    pub v: f64,
    pub w: f64,
    pub f: f64,
    pub t: f64,
    pub e: f64,
}

impl Operands {
    /// Desde un string decimal (`"1.50"`, con signo o no) y el exponente compacto.
    pub fn from_str(s: &str, exponent: i64) -> Operands {
        let s = s.trim_start_matches('-');
        let (int, frac) = s.split_once('.').unwrap_or((s, ""));
        let i: f64 = int.parse().unwrap_or(0.0);
        let trimmed = frac.trim_end_matches('0');
        let f: f64 = if frac.is_empty() {
            0.0
        } else {
            frac.parse().unwrap_or(0.0)
        };
        let t: f64 = if trimmed.is_empty() {
            0.0
        } else {
            trimmed.parse().unwrap_or(0.0)
        };
        let n: f64 = format!("{int}.{}", if frac.is_empty() { "0" } else { frac })
            .parse()
            .unwrap_or(0.0);
        let scale = 10f64.powi(exponent as i32);
        if exponent != 0 {
            return Operands {
                n: n * scale,
                i: (n * scale).trunc(),
                v: 0.0,
                w: 0.0,
                f: 0.0,
                t: 0.0,
                e: exponent as f64,
            };
        }
        Operands {
            n,
            i,
            v: frac.len() as f64,
            w: trimmed.len() as f64,
            f,
            t,
            e: 0.0,
        }
    }

    fn get(&self, c: char) -> f64 {
        match c {
            'n' => self.n,
            'i' => self.i,
            'v' => self.v,
            'w' => self.w,
            'f' => self.f,
            't' => self.t,
            'e' | 'c' => self.e,
            _ => 0.0,
        }
    }
}

/// Evalúa una condición de UTS #35 (`i = 1 and v = 0`, `n % 10 = 2..4 and n % 100 != 12..14`).
fn eval_condition(cond: &str, ops: &Operands) -> bool {
    if cond.trim().is_empty() {
        return true;
    }
    cond.split(" or ")
        .any(|and| and.split(" and ").all(|rel| eval_relation(rel.trim(), ops)))
}

fn eval_relation(rel: &str, ops: &Operands) -> bool {
    let (lhs, rhs, negate) = if let Some((a, b)) = rel.split_once("!=") {
        (a, b, true)
    } else if let Some((a, b)) = rel.split_once('=') {
        (a, b, false)
    } else {
        return false;
    };
    let lhs = lhs.trim();
    let (operand, modulus) = match lhs.split_once('%') {
        Some((o, m)) => (o.trim(), m.trim().parse::<f64>().ok()),
        None => (lhs, None),
    };
    let mut x = ops.get(operand.chars().next().unwrap_or('n'));
    if let Some(m) = modulus {
        x %= m;
    }
    let hit = rhs.split(',').any(|item| {
        let item = item.trim();
        match item.split_once("..") {
            Some((a, b)) => {
                let (a, b) = (
                    a.parse::<f64>().unwrap_or(0.0),
                    b.parse::<f64>().unwrap_or(0.0),
                );
                x.fract() == 0.0 && x >= a && x <= b
            }
            None => item.parse::<f64>().is_ok_and(|v| v == x),
        }
    });
    hit != negate
}

/// PluralRuleSelect (§17.5.3): la categoría de `ops` en `locale`.
pub fn select_category(locale: &str, ordinal: bool, ops: &Operands) -> &'static str {
    let lang = locale.split('-').next().unwrap_or("en");
    let lang = if cldr::PLURAL_RULES.iter().any(|r| r.0 == lang) {
        lang
    } else {
        "en"
    };
    let kind = if ordinal { "ordinal" } else { "cardinal" };
    for &(l, k, cat, cond) in cldr::PLURAL_RULES {
        if l == lang && k == kind && cat != "other" && eval_condition(cond, ops) {
            return cat;
        }
    }
    "other"
}

/// Las categorías de un locale, en el orden de CLDR (`zero one two few many other`).
pub fn categories(locale: &str, ordinal: bool) -> Vec<&'static str> {
    let lang = locale.split('-').next().unwrap_or("en");
    let kind = if ordinal { "ordinal" } else { "cardinal" };
    let mut out = Vec::new();
    for cat in ["zero", "one", "two", "few", "many", "other"] {
        if cldr::PLURAL_RULES
            .iter()
            .any(|&(l, k, c, _)| l == lang && k == kind && c == cat)
        {
            out.push(cat);
        }
    }
    out
}

// --- Intl.PluralRules ---

/// Intl.PluralRules ( [ locales [ , options ] ] ) (§17.1.1).
fn constructor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor Intl.PluralRules requires 'new'");
    };
    let requested = locale::canonicalize_locale_list(agent, &a.arg(0))?;
    let options = coerce_options_to_object(agent, &a.arg(1))?;
    get_string_option(
        agent,
        options,
        "localeMatcher",
        &["lookup", "best fit"],
        Some("best fit"),
    )?;
    let t = get_string_option(
        agent,
        options,
        "type",
        &["cardinal", "ordinal"],
        Some("cardinal"),
    )?;
    let notation = get_string_option(
        agent,
        options,
        "notation",
        &["standard", "scientific", "engineering", "compact"],
        Some("standard"),
    )?
    .unwrap_or_default();
    let compact_display = get_string_option(
        agent,
        options,
        "compactDisplay",
        &["short", "long"],
        Some("short"),
    )?;
    let digits = set_digit_options(agent, options, 0, 3, &notation)?;
    let r = locale::resolve_locale(&requested, &[], &[], &|_, _| Vec::new());
    let o = create_intl_object(
        agent,
        nt,
        "Intl.PluralRules.prototype",
        IntlData::PluralRules(Box::new(PluralRules {
            locale: r.locale,
            ordinal: t.as_deref() == Some("ordinal"),
            compact_display: if notation == "compact" {
                compact_display
            } else {
                None
            },
            notation,
            digits,
        })),
    )?;
    Ok(Value::Object(o))
}

fn this_rules(agent: &mut Agent, this: &Value, method: &str) -> JsResult<PluralRules> {
    match intl_data(agent, this) {
        Some(IntlData::PluralRules(p)) => Ok((**p).clone()),
        _ => agent.type_error(&format!(
            "Method Intl.PluralRules.prototype.{method} called on incompatible receiver"
        )),
    }
}

/// ResolvePlural (§17.5.2): (categoría, string formateado).
fn resolve_plural(pr: &PluralRules, n: f64) -> (&'static str, String) {
    if !n.is_finite() {
        return ("other", String::new());
    }
    let r = format_numeric_to_string(&pr.digits, &Decimal::from_f64(n));
    let ops = Operands::from_str(&r.string, 0);
    (select_category(&pr.locale, pr.ordinal, &ops), r.string)
}

/// Intl.PluralRules.prototype.select ( value ) (§17.3.3).
fn select(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let pr = this_rules(agent, &a.this, "select")?;
    let n = agent.to_number(&a.arg(0))?;
    Ok(str_value(resolve_plural(&pr, n).0))
}

/// Intl.PluralRules.prototype.selectRange ( start, end ) (§17.3.4).
fn select_range(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let pr = this_rules(agent, &a.this, "selectRange")?;
    let (s, e) = (a.arg(0), a.arg(1));
    if s.is_undefined() || e.is_undefined() {
        return agent.type_error("start and end must not be undefined");
    }
    let x = agent.to_number(&s)?;
    let y = agent.to_number(&e)?;
    if x.is_nan() || y.is_nan() {
        return agent.range_error("start and end must be numbers");
    }
    let (xc, xs) = resolve_plural(&pr, x);
    let (yc, ys) = resolve_plural(&pr, y);
    if xs == ys {
        return Ok(str_value(xc));
    }
    // PluralRuleSelectRange: en `en` y `es` el rango toma la categoría del final.
    Ok(str_value(yc))
}

/// Intl.PluralRules.prototype.resolvedOptions ( ) (§17.3.5).
fn resolved_options(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let pr = this_rules(agent, &a.this, "resolvedOptions")?;
    let o = agent.new_plain_object();
    let set = |agent: &mut Agent, k: &str, v: Value| {
        agent.create_data_property_or_throw(o, PropertyKey::from(k), v)
    };
    set(agent, "locale", str_value(&pr.locale))?;
    set(
        agent,
        "type",
        str_value(if pr.ordinal { "ordinal" } else { "cardinal" }),
    )?;
    set(agent, "notation", str_value(&pr.notation))?;
    if let Some(c) = &pr.compact_display {
        set(agent, "compactDisplay", str_value(c))?;
    }
    pr.digits.write_resolved(agent, o)?;
    let cats: Vec<Value> = categories(&pr.locale, pr.ordinal)
        .into_iter()
        .map(str_value)
        .collect();
    let arr = agent.create_array_from_list(&cats)?;
    set(agent, "pluralCategories", Value::Object(arr))?;
    pr.digits.write_rounding(agent, o)?;
    Ok(Value::Object(o))
}

fn supported_locales_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let requested = locale::canonicalize_locale_list(agent, &a.arg(0))?;
    locale::supported_locales(agent, &requested, &a.arg(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules() {
        let c = |l: &str, s: &str| select_category(l, false, &Operands::from_str(s, 0));
        assert_eq!(c("en", "1"), "one");
        assert_eq!(c("en", "1.0"), "other");
        assert_eq!(c("en", "2"), "other");
        assert_eq!(c("es", "1"), "one");
        assert_eq!(c("es", "1000000"), "many");
        let o = |s: &str| select_category("en", true, &Operands::from_str(s, 0));
        assert_eq!(o("1"), "one");
        assert_eq!(o("22"), "two");
        assert_eq!(o("13"), "other");
        assert_eq!(o("103"), "few");
        assert_eq!(categories("en", false), vec!["one", "other"]);
        assert_eq!(categories("es", false), vec!["one", "many", "other"]);
    }
}
