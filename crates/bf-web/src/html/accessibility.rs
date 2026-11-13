//! E1: el rol y el nombre accesibles con el estilo de la página (el cálculo está en
//! `bf_dom::accessibility`; aquí, lo que depende del CSS y del estado de los controles).
//!
//! [`computed_role`] y [`computed_label`] son lo que exponen WebDriver "Get Computed Role" y
//! "Get Computed Label" (y el `testdriver` del runner de WPT); el shell los puede usar para el
//! árbol de accesibilidad.

use std::collections::{HashMap, HashSet};

use bf_css::selectors::PseudoElement;
use bf_dom::NodeId;
use bf_dom::accessibility::{self, A11yEnv};
use bf_js::string::JsString;
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use crate::{DocIndex, NodeRef, WebContextHandle, with_doc};

/// Lo que el cálculo necesita del estilo y de los controles de un documento.
#[derive(Debug, Default, Clone)]
struct StyleEnv {
    display_none: HashSet<NodeId>,
    inline: HashSet<NodeId>,
    visibility_hidden: HashSet<NodeId>,
    before: HashMap<NodeId, String>,
    after: HashMap<NodeId, String>,
    values: HashMap<NodeId, String>,
    transforms: HashMap<NodeId, String>,
}

impl A11yEnv for StyleEnv {
    fn display_none(&self, n: NodeId) -> bool {
        self.display_none.contains(&n)
    }

    fn visibility_hidden(&self, n: NodeId) -> bool {
        self.visibility_hidden.contains(&n)
    }

    fn is_inline(&self, _doc: &bf_dom::Document, n: NodeId) -> bool {
        self.inline.contains(&n)
    }

    fn pseudo_content(&self, n: NodeId, after: bool) -> Option<String> {
        if after {
            self.after.get(&n).cloned()
        } else {
            self.before.get(&n).cloned()
        }
    }

    fn control_value(&self, n: NodeId) -> Option<String> {
        self.values.get(&n).cloned()
    }

    fn text_transform(&self, n: NodeId) -> Option<String> {
        self.transforms.get(&n).cloned()
    }
}

/// El entorno de cada documento con la versión en que se calculó.
#[derive(Debug, Default)]
struct EnvCache(HashMap<DocIndex, (u64, StyleEnv)>);

/// El texto de una lista de `content`: las cadenas y `attr(nombre)` (con `attr` para leer
/// el atributo del elemento); `none`/`normal`, nada. Los contadores no se evalúan.
fn content_text(v: &str, attr: impl Fn(&str) -> Option<String>) -> Option<String> {
    let v = v.trim();
    if v.is_empty() || v == "none" || v == "normal" {
        return None;
    }
    let mut out = String::new();
    let mut chars = v.chars().peekable();
    let mut any = false;
    while let Some(c) = chars.next() {
        if c == 'a' {
            // `attr(nombre)`.
            let rest: String = chars.clone().take(4).collect();
            if rest == "ttr(" {
                for _ in 0..4 {
                    chars.next();
                }
                let mut name = String::new();
                for d in chars.by_ref() {
                    if d == ')' {
                        break;
                    }
                    name.push(d);
                }
                any = true;
                if let Some(v) = attr(name.trim()) {
                    out.push_str(&v);
                }
            }
            continue;
        }
        if c == '"' || c == '\'' {
            any = true;
            let q = c;
            while let Some(d) = chars.next() {
                if d == '\\' {
                    if let Some(e) = chars.next() {
                        out.push(e);
                    }
                } else if d == q {
                    break;
                } else {
                    out.push(d);
                }
            }
        }
    }
    any.then_some(out)
}

/// El entorno de `doc` (de la caché si el documento no cambió).
fn env_for(agent: &mut Agent, doc: DocIndex) -> StyleEnv {
    let version = with_doc(agent, doc, |d| d.version());
    let realm = WebContextHandle::current(agent).realm;
    if let Some((v, env)) = agent
        .realm_data::<EnvCache>(realm)
        .and_then(|c| c.0.get(&doc))
        && *v == version
    {
        return env.clone();
    }
    let elements: Vec<NodeId> = with_doc(agent, doc, |d| {
        let root = d.root();
        d.descendants(root)
            .into_iter()
            .filter(|&n| d.element(n).is_some())
            .collect()
    });
    let mut env = StyleEnv::default();
    for n in elements {
        let r = NodeRef::new(doc, n);
        let display = crate::cssom::computed::property_value(agent, r, None, "display");
        if display == "none" {
            env.display_none.insert(n);
        }
        // `inline-block` y demás cajas atómicas se separan con espacios (como en los
        // navegadores); solo `inline` y `contents` van pegadas.
        if display == "inline" || display == "contents" || display.is_empty() {
            env.inline.insert(n);
        }
        let transform = crate::cssom::computed::property_value(agent, r, None, "text-transform");
        if matches!(transform.as_str(), "uppercase" | "lowercase" | "capitalize") {
            env.transforms.insert(n, transform);
        }
        let visibility = crate::cssom::computed::property_value(agent, r, None, "visibility");
        if visibility == "hidden" || visibility == "collapse" {
            env.visibility_hidden.insert(n);
        }
        for (pseudo, after) in [(PseudoElement::Before, false), (PseudoElement::After, true)] {
            let c = crate::cssom::computed::property_value(agent, r, Some(pseudo), "content");
            let attr_of = |name: &str| {
                super::elements::attr_value(agent, r, name)
                    .map(|v| v.to_string_lossy().into_owned())
            };
            if let Some(t) = content_text(&c, attr_of) {
                if after {
                    env.after.insert(n, t);
                } else {
                    env.before.insert(n, t);
                }
            }
        }
        match super::elements::html_local_name(agent, r).as_deref() {
            Some("input") => {
                env.values
                    .insert(n, super::forms::input_current_value(agent, r));
            }
            Some("textarea") => {
                env.values
                    .insert(n, super::forms::textarea_value_of(agent, r));
            }
            _ => {}
        }
    }
    let version = with_doc(agent, doc, |d| d.version());
    if agent.realm_data::<EnvCache>(realm).is_none() {
        agent.set_realm_data(realm, EnvCache::default());
    }
    if let Some(c) = agent.realm_data_mut::<EnvCache>(realm) {
        c.0.insert(doc, (version, env.clone()));
    }
    env
}

/// El rol calculado de `n` (WAI-ARIA 1.2 §5.4 y HTML-AAM §4).
pub fn computed_role(agent: &mut Agent, n: NodeRef) -> String {
    let env = env_for(agent, n.doc);
    with_doc(agent, n.doc, |d| {
        accessibility::computed_role(d, n.node, &env)
    })
}

/// El nombre accesible de `n` (AccName 1.2).
pub fn computed_label(agent: &mut Agent, n: NodeRef) -> String {
    let env = env_for(agent, n.doc);
    with_doc(agent, n.doc, |d| {
        accessibility::accessible_name(d, n.node, &env)
    })
}

/// La descripción accesible de `n` (AccName 1.2 §4.3, `aria-describedby`).
pub fn computed_description(agent: &mut Agent, n: NodeRef) -> String {
    let env = env_for(agent, n.doc);
    with_doc(agent, n.doc, |d| {
        accessibility::accessible_description(d, n.node, &env)
    })
}

/// `__freedom_computed_role(el)` del runner de WPT (`test_driver.get_computed_role`).
pub fn role_native(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = crate::dom::arg_node(agent, &a.arg(0), "get_computed_role")?;
    let r = computed_role(agent, n);
    Ok(Value::String(JsString::from(r.as_str())))
}

/// `__freedom_computed_label(el)` del runner de WPT (`test_driver.get_computed_label`).
pub fn label_native(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = crate::dom::arg_node(agent, &a.arg(0), "get_computed_label")?;
    let r = computed_label(agent, n);
    Ok(Value::String(JsString::from(r.as_str())))
}

#[cfg(test)]
mod tests {
    use super::content_text;

    #[test]
    fn content_strings() {
        let none = |_: &str| None;
        assert_eq!(content_text("none", none), None);
        assert_eq!(content_text("\"a\" \"b\"", none), Some("ab".into()));
        assert_eq!(content_text("'x\\'y'", none), Some("x'y".into()));
        assert_eq!(content_text("counter(c)", none), None);
        let t = |n: &str| (n == "data-t").then(|| "T".to_owned());
        assert_eq!(
            content_text("\"start \" attr(data-t) \" end\"", t),
            Some("start T end".into())
        );
    }
}
