//! `CSSStyleDeclaration` (CSSOM §6.7.3) y `ElementCSSInlineStyle.style` (CSSOM §6.7.5).
//!
//! Una declaración tiene un dueño: el atributo `style` de un elemento (se re-parsea en cada
//! acceso y se reescribe en cada cambio, así queda viva), una regla de una hoja, o un
//! elemento cuyo estilo computado expone (solo lectura).

use bf_css::selectors::PseudoElement;
use bf_dom::DomException;
use bf_js::host::HostClass;
use bf_js::object::ObjectKind;
use bf_js::object::{Prop, PropValue};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, RealmId};

use super::declaration::{DeclBlock, normalize_name};
use super::sheets::RuleRef;
use super::value::{camel_case, supported_names};
use super::{KeyKind, classes, computed, node_key, set_indexed, sheets};
use crate::dom::{mutated, this_node, throw_dom};
use crate::{NodeRef, with_doc, with_doc_mut};

/// El dueño de una declaración.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclOwner {
    /// El atributo `style` de un elemento.
    Inline(NodeRef),
    /// El estilo computado de un elemento (o de uno de sus pseudo-elementos).
    Computed {
        node: NodeRef,
        pseudo: Option<PseudoElement>,
    },
    /// Una regla de una hoja (`CSSStyleRule.style`, `CSSFontFaceRule.style`…).
    Rule(RuleRef),
}

pub fn class() -> HostClass {
    HostClass::new("CSSStyleDeclaration")
        .attribute("cssText", Some(get_css_text), Some(set_css_text))
        .attribute("length", Some(length), None)
        .method("item", 1, item)
        .method("getPropertyValue", 1, get_property_value)
        .method("getPropertyPriority", 1, get_property_priority)
        .method("setProperty", 2, set_property)
        .method("removeProperty", 1, remove_property)
        .attribute("parentRule", Some(parent_rule), None)
        .attribute("cssFloat", Some(get_css_float), Some(set_css_float))
}

/// `partial interface Element { [SameObject, PutForwards=cssText] readonly attribute
/// CSSStyleDeclaration style; }` (CSSOM §6.7.5, mixin `ElementCSSInlineStyle`).
pub fn element_partial() -> HostClass {
    HostClass::new("Element").attribute("style", Some(element_style), Some(set_element_style))
}

/// El wrapper de una declaración (con identidad para las de los elementos y las reglas).
pub fn wrap(agent: &mut Agent, owner: DeclOwner) -> Value {
    let class = classes(agent).style_declaration;
    let realm = agent.current_realm;
    let o = match owner {
        DeclOwner::Inline(n) => agent.host_wrap(
            realm,
            node_key(KeyKind::InlineStyle, n),
            class,
            Box::new(owner),
        ),
        DeclOwner::Rule(r) => agent.host_wrap(
            realm,
            super::key(KeyKind::RuleStyle, u64::from(r.rule)),
            class,
            Box::new(owner),
        ),
        DeclOwner::Computed { .. } => agent.host_create(class, Box::new(owner)),
    };
    refresh_indices(agent, o, owner);
    Value::Object(o)
}

fn refresh_indices(agent: &mut Agent, o: bf_js::heap::Gc<bf_js::object::Object>, owner: DeclOwner) {
    let names = item_names(agent, owner);
    let items: Vec<Value> = names.iter().map(|n| Value::str(n)).collect();
    set_indexed(agent, o, &items);
}

fn this_owner(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<DeclOwner> {
    match agent.host_unwrap::<DeclOwner>(&a.this) {
        Some(o) => Ok(*o),
        None => agent.type_error("Illegal invocation"),
    }
}

/// El bloque de una declaración no computada.
pub fn read_block(agent: &mut Agent, owner: DeclOwner) -> DeclBlock {
    match owner {
        DeclOwner::Inline(n) => {
            let text = with_doc(agent, n.doc, |d| {
                d.element(n.node)
                    .and_then(|e| e.attr("style").map(str::to_owned))
            });
            text.map_or_else(DeclBlock::default, |t| DeclBlock::parse(&t))
        }
        DeclOwner::Rule(r) => sheets::rule_block(agent, r).unwrap_or_default(),
        DeclOwner::Computed { .. } => DeclBlock::default(),
    }
}

/// Guarda el bloque en su dueño ("update style attribute for the CSS declaration block",
/// CSSOM §6.7.5, o la regla).
fn write_block(agent: &mut Agent, owner: DeclOwner, block: DeclBlock) {
    match owner {
        DeclOwner::Inline(n) => {
            let text = block.css_text();
            let _ = with_doc_mut(agent, n.doc, |d| {
                d.set_attribute(n.node, &"style".into(), text.as_str().into())
            });
            mutated(agent);
        }
        DeclOwner::Rule(r) => {
            sheets::set_rule_block(agent, r, block);
            super::sheets_changed(agent);
        }
        DeclOwner::Computed { .. } => {}
    }
}

/// Los nombres de `item()` (en orden de declaración, o los longhands ordenados en una
/// declaración computada).
fn item_names(agent: &mut Agent, owner: DeclOwner) -> Vec<String> {
    match owner {
        DeclOwner::Computed { node, pseudo } => computed::item_names(agent, node, pseudo),
        _ => read_block(agent, owner).names(),
    }
}

fn readonly<T>(agent: &mut Agent) -> JsResult<T> {
    throw_dom(agent, DomException::NoModificationAllowedError)
}

/// `[LegacyNullToEmptyString] CSSOMString`.
fn null_to_empty(agent: &mut Agent, v: &Value) -> JsResult<String> {
    if v.is_null() {
        return Ok(String::new());
    }
    Ok(agent.to_string(v)?.to_string())
}

fn str_arg(agent: &mut Agent, v: &Value) -> JsResult<String> {
    Ok(agent.to_string(v)?.to_string())
}

/// `getPropertyValue` para un dueño cualquiera.
pub fn property_value(agent: &mut Agent, owner: DeclOwner, name: &str) -> String {
    match owner {
        DeclOwner::Computed { node, pseudo } => {
            computed::property_value(agent, node, pseudo, &normalize_name(name))
        }
        _ => read_block(agent, owner).get_property_value(name),
    }
}

/// `setProperty` para un dueño cualquiera.
pub fn apply_set_property(
    agent: &mut Agent,
    this: &Value,
    owner: DeclOwner,
    name: &str,
    value: &str,
    priority: &str,
) -> JsResult<()> {
    if matches!(owner, DeclOwner::Computed { .. }) {
        return readonly(agent);
    }
    let mut block = read_block(agent, owner);
    if block.set_property(name, value, priority) {
        write_block(agent, owner, block);
        if let Value::Object(o) = this {
            refresh_indices(agent, *o, owner);
        }
    }
    Ok(())
}

fn get_css_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let owner = this_owner(agent, a)?;
    if matches!(owner, DeclOwner::Computed { .. }) {
        return Ok(Value::str(""));
    }
    Ok(Value::str(&read_block(agent, owner).css_text()))
}

fn set_css_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let owner = this_owner(agent, a)?;
    if matches!(owner, DeclOwner::Computed { .. }) {
        return readonly(agent);
    }
    let text = null_to_empty(agent, &a.arg(0))?;
    write_block(agent, owner, DeclBlock::parse(&text));
    if let Value::Object(o) = &a.this {
        refresh_indices(agent, *o, owner);
    }
    Ok(Value::Undefined)
}

fn length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let owner = this_owner(agent, a)?;
    Ok(Value::Number(item_names(agent, owner).len() as f64))
}

fn item(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let owner = this_owner(agent, a)?;
    let i = bf_js::host::webidl::to_unsigned_long(agent, &a.arg(0))? as usize;
    let names = item_names(agent, owner);
    Ok(Value::str(names.get(i).map_or("", String::as_str)))
}

fn get_property_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let owner = this_owner(agent, a)?;
    let name = str_arg(agent, &a.arg(0))?;
    Ok(Value::str(&property_value(agent, owner, &name)))
}

fn get_property_priority(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let owner = this_owner(agent, a)?;
    let name = str_arg(agent, &a.arg(0))?;
    if matches!(owner, DeclOwner::Computed { .. }) {
        return Ok(Value::str(""));
    }
    Ok(Value::str(
        read_block(agent, owner).get_property_priority(&name),
    ))
}

fn set_property(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let owner = this_owner(agent, a)?;
    if matches!(owner, DeclOwner::Computed { .. }) {
        return readonly(agent);
    }
    let name = str_arg(agent, &a.arg(0))?;
    let value = null_to_empty(agent, &a.arg(1))?;
    let priority = if a.arg(2).is_undefined() {
        String::new()
    } else {
        null_to_empty(agent, &a.arg(2))?
    };
    apply_set_property(agent, &a.this, owner, &name, &value, &priority)?;
    Ok(Value::Undefined)
}

fn remove_property(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let owner = this_owner(agent, a)?;
    if matches!(owner, DeclOwner::Computed { .. }) {
        return readonly(agent);
    }
    let name = str_arg(agent, &a.arg(0))?;
    let mut block = read_block(agent, owner);
    let (old, changed) = block.remove_property(&name);
    if changed {
        write_block(agent, owner, block);
        if let Value::Object(o) = &a.this {
            refresh_indices(agent, *o, owner);
        }
    }
    Ok(Value::str(&old))
}

fn parent_rule(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    match this_owner(agent, a)? {
        DeclOwner::Rule(r) => Ok(sheets::wrap_rule(agent, r)),
        _ => Ok(Value::Null),
    }
}

fn get_css_float(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let owner = this_owner(agent, a)?;
    Ok(Value::str(&property_value(agent, owner, "float")))
}

fn set_css_float(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let owner = this_owner(agent, a)?;
    let value = null_to_empty(agent, &a.arg(0))?;
    apply_set_property(agent, &a.this, owner, "float", &value, "")?;
    Ok(Value::Undefined)
}

fn element_style(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    Ok(wrap(agent, DeclOwner::Inline(n)))
}

fn set_element_style(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let text = null_to_empty(agent, &a.arg(0))?;
    write_block(agent, DeclOwner::Inline(n), DeclBlock::parse(&text));
    Ok(Value::Undefined)
}

// --- Los atributos de cada propiedad (CSSOM §6.7.1) ---

/// El nombre de la propiedad que guarda el accessor que se está ejecutando.
fn accessor_property(agent: &Agent, a: &CallArgs<'_>) -> String {
    match &agent.heap[a.callee].kind {
        ObjectKind::Builtin(b) => match b.slots.first() {
            Some(Value::String(s)) => s.to_string(),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

fn property_getter(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let owner = this_owner(agent, a)?;
    let name = accessor_property(agent, a);
    Ok(Value::str(&property_value(agent, owner, &name)))
}

fn property_setter(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let owner = this_owner(agent, a)?;
    let name = accessor_property(agent, a);
    let value = null_to_empty(agent, &a.arg(0))?;
    apply_set_property(agent, &a.this, owner, &name, &value, "")?;
    Ok(Value::Undefined)
}

/// Define en `CSSStyleDeclaration.prototype` de `realm` los atributos `camelCase`,
/// `webkitCase` y con guiones de cada propiedad soportada.
pub fn define_property_accessors(agent: &mut Agent, realm: RealmId) {
    let class = classes(agent).style_declaration;
    let proto = agent.host_class_objects(realm, class).prototype;
    if agent.heap[proto]
        .props
        .contains(&PropertyKey::from("backgroundColor"))
    {
        return;
    }
    let saved = agent.current_realm;
    agent.current_realm = realm;
    for &name in supported_names() {
        let mut attrs = vec![camel_case(name, false)];
        if let Some(rest) = name.strip_prefix("-webkit-") {
            attrs.push(camel_case(&format!("webkit-{rest}"), false));
        }
        if name.contains('-') {
            attrs.push(name.to_owned());
        }
        for attr in attrs {
            let get = agent.create_builtin_with_slots(
                property_getter,
                0,
                &format!("get {attr}"),
                vec![Value::str(name)],
            );
            let set = agent.create_builtin_with_slots(
                property_setter,
                1,
                &format!("set {attr}"),
                vec![Value::str(name)],
            );
            agent.heap[proto].props.insert(
                PropertyKey::from(attr.as_str()),
                Prop {
                    value: PropValue::Accessor {
                        get: Some(get),
                        set: Some(set),
                    },
                    enumerable: true,
                    configurable: true,
                },
            );
        }
    }
    agent.current_realm = saved;
}
