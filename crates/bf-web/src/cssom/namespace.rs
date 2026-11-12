//! El namespace `CSS` (CSSOM §7.1, CSS Conditional 3 §8.1): `CSS.escape()` y
//! `CSS.supports()`.

use bf_css::stylesheet::{Origin, Rule, Stylesheet};
use bf_css::syntax::parse_declaration;
use bf_js::string::JsString;
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::{CallArgs, NativeFn};
use bf_js::vm::{Agent, JsResult, RealmId};

/// Instala `CSS` en el global de `realm`.
pub fn install(agent: &mut Agent, realm: RealmId) {
    let g = agent.realms[realm].global_object;
    if agent
        .has_own_property(g, &PropertyKey::from("CSS"))
        .unwrap_or(false)
    {
        return;
    }
    let saved = agent.current_realm;
    agent.current_realm = realm;
    // Un namespace object (WebIDL §3.13): prototipo %Object.prototype%.
    let op = agent.realms[realm].intrinsics.object_prototype;
    let ns = agent.new_object(Some(op));
    bf_js::builtins::to_string_tag(agent, ns, "CSS");
    const OPS: &[(&str, u32, NativeFn)] = &[("escape", 1, escape), ("supports", 1, supports)];
    for &(name, len, f) in OPS {
        bf_js::builtins::method(agent, ns, name, len, f);
    }
    agent.put_data(
        g,
        PropertyKey::from("CSS"),
        Value::Object(ns),
        true,
        false,
        true,
    );
    agent.current_realm = saved;
}

/// `@@iterator` = `%Array.prototype.values%` en las interfaces con getter indexado del
/// CSSOM (WebIDL §3.7.6): `[...style]`, `for (const r of sheet.cssRules)`…
pub fn install_indexed_iterators(agent: &mut Agent, realm: RealmId) {
    let g = agent.realms[realm].global_object;
    let ap = agent.realms[realm].intrinsics.array_prototype;
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let values = agent
        .get(ap, &PropertyKey::from("values"), Value::Object(ap))
        .unwrap_or(Value::Undefined);
    let iter = PropertyKey::Symbol(agent.well_known.iterator);
    let proto_key = agent.keys.prototype.clone();
    for name in [
        "CSSStyleDeclaration",
        "CSSRuleList",
        "StyleSheetList",
        "MediaList",
    ] {
        let Ok(Value::Object(iface)) = agent.get(g, &PropertyKey::from(name), Value::Object(g))
        else {
            continue;
        };
        let Ok(Value::Object(proto)) = agent.get(iface, &proto_key, Value::Object(iface)) else {
            continue;
        };
        if !agent.has_own_property(proto, &iter).unwrap_or(true) {
            agent.put_data(proto, iter.clone(), values.clone(), true, false, true);
        }
    }
    agent.current_realm = saved;
}

/// `CSS.escape(ident)`: "serialize an identifier" (CSSOM §2.1).
fn escape(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    if a.is_empty() {
        return agent.type_error("CSS.escape: 1 argument required, but only 0 present.");
    }
    let s = agent.to_string(&a.arg(0))?;
    Ok(Value::String(JsString::from_units(escape_units(s.units()))))
}

/// "Serialize an identifier" sobre unidades UTF-16, como [`super::value::write_ident`]: los
/// surrogates sueltos (≥ U+0080) pasan tal cual.
fn escape_units(s: &[u16]) -> Vec<u16> {
    const DASH: u16 = b'-' as u16;
    let mut out = Vec::with_capacity(s.len());
    for (i, &c) in s.iter().enumerate() {
        let digit = (0x30..=0x39).contains(&c);
        let first_digit = digit && (i == 0 || (i == 1 && s[0] == DASH));
        if c == 0 {
            out.push(0xFFFD);
        } else if (0x1..=0x1f).contains(&c) || c == 0x7f || first_digit {
            out.extend(format!("\\{c:x} ").encode_utf16());
        } else if i == 0 && c == DASH && s.len() == 1 {
            out.extend("\\-".encode_utf16());
        } else if c >= 0x80
            || c == DASH
            || c == u16::from(b'_')
            || u8::try_from(c).is_ok_and(|b| b.is_ascii_alphanumeric())
        {
            out.push(c);
        } else {
            out.push(u16::from(b'\\'));
            out.push(c);
        }
    }
    out
}

/// `CSS.supports(property, value)` y `CSS.supports(conditionText)` (CSS Conditional 3 §8.1).
fn supports(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    if a.is_empty() {
        return agent.type_error("CSS.supports: 1 argument required, but only 0 present.");
    }
    let first = agent.to_string(&a.arg(0))?.to_string();
    if a.len() >= 2 {
        let value = agent.to_string(&a.arg(1))?.to_string();
        return Ok(Value::Boolean(supports_declaration(&first, &value)));
    }
    Ok(Value::Boolean(supports_condition(&first)))
}

/// ¿`property: value` es una declaración válida? Las propiedades se comparan sin distinguir
/// mayúsculas, salvo las personalizadas; `!important` no es parte del valor.
fn supports_declaration(property: &str, value: &str) -> bool {
    let name = if property.starts_with("--") {
        property.to_owned()
    } else {
        property.to_ascii_lowercase()
    };
    if name.is_empty() || name.contains(|c: char| c == ':' || c.is_whitespace()) {
        return false;
    }
    let Ok(decl) = parse_declaration(&format!("x:{value}")) else {
        return false;
    };
    if decl.important {
        return false;
    }
    if name.starts_with("--") {
        return name.len() > 2;
    }
    bf_style::cascade::is_supported(&name, &decl.value)
}

/// ¿`text` es una `<supports-condition>` que se cumple? Si no parsea, se prueba entre
/// paréntesis (para `CSS.supports("color: red")`).
fn supports_condition(text: &str) -> bool {
    if text.contains(['{', '}', ';']) {
        return false;
    }
    let eval = |cond: &str| -> Option<bool> {
        let sheet = Stylesheet::parse(&format!("@supports {cond} {{}}"), Origin::Author);
        match sheet.rules.first() {
            Some(Rule::Supports(s)) => Some(s.condition.evaluate(&bf_style::cascade::is_supported)),
            _ => None,
        }
    };
    eval(text)
        .or_else(|| eval(&format!("({text})")))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape() {
        let e = |s: &str| {
            String::from_utf16(&escape_units(&s.encode_utf16().collect::<Vec<_>>())).unwrap()
        };
        assert_eq!(e("0a"), "\\30 a");
        assert_eq!(e("-"), "\\-");
        assert_eq!(e("-1"), "-\\31 ");
        assert_eq!(e("a b"), "a\\ b");
        assert_eq!(e("\u{1}\0"), "\\1 \u{FFFD}");
        assert_eq!(escape_units(&[0xDF06]), vec![0xDF06]);
    }

    #[test]
    fn declarations() {
        assert!(supports_declaration("color", "red"));
        assert!(supports_declaration("COLOR", "red"));
        assert!(!supports_declaration("color", "red !important"));
        assert!(!supports_declaration("color", "nope nope"));
        assert!(!supports_declaration("colour", "red"));
        assert!(supports_declaration("--x", "anything"));
        assert!(!supports_declaration("--", "anything"));
    }

    #[test]
    fn conditions() {
        assert!(supports_condition("(color: red)"));
        assert!(supports_condition("color: red"));
        assert!(supports_condition("(color: red) and (display: block)"));
        assert!(!supports_condition("(color: red) and (display: nope)"));
        assert!(supports_condition("not (color: nope)"));
        assert!(!supports_condition("color: red; display: block"));
    }
}
