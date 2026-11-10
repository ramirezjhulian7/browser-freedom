//! E4 (en territorio de E2, archivo propio): el namespace `CSS` con `CSS.supports()` (CSS
//! Conditional Rules 3 §8.1, "The CSS namespace, and the supports() function") y
//! `CSS.escape()` (CSSOM §2.1, "serialize an identifier"). Lo usan las bibliotecas de WPT de
//! interpolación (`css/support/interpolation-testcommon.js`).

use bf_css::stylesheet::{Origin, Rule, Stylesheet};
use bf_js::builtins::{FnEntry, methods};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, RealmId};

use crate::dom::to_dom_string;

/// Define `CSS` en el global de `realm`.
pub fn install(agent: &mut Agent, realm: RealmId) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let css = agent.new_object(Some(op));
    const FNS: &[FnEntry] = &[("escape", 1, escape), ("supports", 1, supports)];
    methods(agent, css, FNS);
    let g = agent.realms[realm].global_object;
    agent.put_data(
        g,
        PropertyKey::from("CSS"),
        Value::Object(css),
        true,
        false,
        true,
    );
    agent.current_realm = saved;
}

/// `CSS.supports(property, value)` y `CSS.supports(conditionText)`.
fn supports(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let first = to_dom_string(agent, &a.arg(0))?.to_string();
    if a.args.len() >= 2 {
        let value = to_dom_string(agent, &a.arg(1))?.to_string();
        return Ok(Value::Boolean(supports_declaration(&first, &value)));
    }
    Ok(Value::Boolean(supports_condition(&first)))
}

/// "If property is an ASCII case-insensitive match for any defined CSS property that the UA
/// supports, or is a custom property name string, and value successfully parses according
/// to that property's grammar, return true." (`!important` no es parte del valor.)
pub fn supports_declaration(property: &str, value: &str) -> bool {
    let name = if property.starts_with("--") {
        property.to_owned()
    } else {
        property.to_ascii_lowercase()
    };
    let values = bf_css::syntax::parse_component_value_list(value);
    if values.iter().all(|v| v.is_whitespace()) && !name.starts_with("--") {
        return false;
    }
    // Un `!important` al final no pertenece al valor: con él, el valor no es válido.
    let lower = value.to_ascii_lowercase();
    if lower.trim_end().ends_with("important") && lower.contains('!') {
        return false;
    }
    bf_style::is_supported(&name, &values)
}

/// "If conditionText, parsed and evaluated as a <supports-condition>, would return true,
/// return true. Otherwise, if conditionText, wrapped in parentheses and then parsed and
/// evaluated as a <supports-condition>, would return true, return true."
pub fn supports_condition(text: &str) -> bool {
    let eval = |cond: &str| {
        let sheet = Stylesheet::parse(&format!("@supports {cond} {{}}"), Origin::Author);
        match sheet.rules.as_slice() {
            [Rule::Supports(s)] => s.condition.evaluate(&bf_style::is_supported),
            _ => false,
        }
    };
    eval(text) || eval(&format!("({text})"))
}

/// `CSS.escape(ident)`: "serialize an identifier" (CSSOM §2.1).
fn escape(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let ident = to_dom_string(agent, &a.arg(0))?.to_string();
    Ok(Value::from(serialize_identifier(&ident).as_str()))
}

pub fn serialize_identifier(ident: &str) -> String {
    let chars: Vec<char> = ident.chars().collect();
    let mut out = String::new();
    let hex = |out: &mut String, c: char| out.push_str(&format!("\\{:x} ", c as u32));
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '\0' => out.push('\u{FFFD}'),
            '\u{1}'..='\u{1F}' | '\u{7F}' => hex(&mut out, c),
            '0'..='9' if i == 0 || (i == 1 && chars[0] == '-') => hex(&mut out, c),
            '-' if i == 0 && chars.len() == 1 => out.push_str("\\-"),
            c if c >= '\u{80}' || c == '-' || c == '_' || c.is_ascii_alphanumeric() => out.push(c),
            c => {
                out.push('\\');
                out.push(c);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supports_declarations_and_conditions() {
        assert!(supports_declaration("color", "red"));
        assert!(supports_declaration("COLOR", "red"));
        assert!(!supports_declaration("color", "red !important"));
        assert!(!supports_declaration("color", "10px"));
        assert!(!supports_declaration("colour", "red"));
        assert!(supports_declaration("--x", "anything"));
        assert!(supports_condition("(color: red)"));
        assert!(supports_condition("color: red"));
        assert!(supports_condition("(color: red) and (display: block)"));
        assert!(!supports_condition("(color: 10px)"));
        assert!(!supports_condition("not (color: red)"));
    }

    #[test]
    fn escapes_identifiers() {
        assert_eq!(serialize_identifier("a b"), "a\\ b");
        assert_eq!(serialize_identifier("1a"), "\\31 a");
        assert_eq!(serialize_identifier("-1"), "-\\31 ");
        assert_eq!(serialize_identifier("-"), "\\-");
        assert_eq!(serialize_identifier("a\0"), "a\u{FFFD}");
        assert_eq!(serialize_identifier("é_-x"), "é_-x");
    }
}
