//! `getComputedStyle()` (CSSOM §9) y los valores resueltos (CSSOM §9, "resolved value"):
//! el valor computado de `bf-style` serializado, salvo las propiedades de dimensión, cuyo
//! valor resuelto es el usado (en `px`) cuando el elemento está maquetado.

use bf_css::selectors::PseudoElement;
use bf_js::host::HostClass;
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};
use bf_layout::Fragment;
use bf_style::ComputedStyle;
use bf_style::properties::types::{BoxSizing, Display, Position};

use super::declaration::DeclBlock;
use super::style_decl::{DeclOwner, wrap};
use super::value::{Property, number, property, sorted_longhands};
use super::{current_layout, current_styles, view};
use crate::NodeRef;
use crate::dom::unwrap_node;

/// `partial interface Window { [NewObject] CSSStyleDeclaration getComputedStyle(Element
/// elt, optional CSSOMString? pseudoElt); }` (CSSOM §9).
pub fn window_partial() -> HostClass {
    HostClass::new("Window").method("getComputedStyle", 1, get_computed_style)
}

/// El pseudo-elemento de `getComputedStyle(elt, pseudoElt)`: `Err(())` si es inválido.
fn parse_pseudo(s: &str) -> Result<Option<PseudoElement>, ()> {
    if s.is_empty() || !s.starts_with(':') {
        return Ok(None);
    }
    let lower = s.to_ascii_lowercase();
    let (name, legacy) = match lower.strip_prefix("::") {
        Some(n) => (n, false),
        None => (&lower[1..], true),
    };
    let p = match name {
        "before" => PseudoElement::Before,
        "after" => PseudoElement::After,
        "first-line" => PseudoElement::FirstLine,
        "first-letter" => PseudoElement::FirstLetter,
        "marker" if !legacy => PseudoElement::Marker,
        "placeholder" if !legacy => PseudoElement::Placeholder,
        "selection" if !legacy => PseudoElement::Selection,
        _ => return Err(()),
    };
    Ok(Some(p))
}

fn get_computed_style(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(node) = unwrap_node(agent, &a.arg(0)) else {
        return agent.type_error(
            "Failed to execute 'getComputedStyle' on 'Window': parameter 1 is not of type 'Element'.",
        );
    };
    let is_element = crate::with_doc(agent, node.doc, |d| d.element(node.node).is_some());
    if !is_element {
        return agent.type_error(
            "Failed to execute 'getComputedStyle' on 'Window': parameter 1 is not of type 'Element'.",
        );
    }
    let pseudo_arg = a.arg(1);
    let pseudo = if pseudo_arg.is_nullish() {
        None
    } else {
        let s = agent.to_string(&pseudo_arg)?.to_string();
        // Un pseudo-elemento inválido da una declaración vacía (sin estilo).
        parse_pseudo(&s).unwrap_or(Some(PseudoElement::Selection))
    };
    Ok(wrap(agent, DeclOwner::Computed { node, pseudo }))
}

/// El estilo computado de `node` (y `pseudo`), si el elemento está en el documento de la
/// ventana.
fn style_of(
    agent: &mut Agent,
    node: NodeRef,
    pseudo: Option<PseudoElement>,
) -> Option<std::sync::Arc<ComputedStyle>> {
    if node.doc != 0 {
        return None;
    }
    let connected = crate::with_doc(agent, 0, |d| d.is_connected(node.node));
    if !connected {
        return None;
    }
    let styles = current_styles(agent);
    styles.get(&(node.node, pseudo)).cloned()
}

/// Los nombres de una declaración computada: los longhands en orden lexicográfico y luego
/// las custom properties.
pub fn item_names(agent: &mut Agent, node: NodeRef, pseudo: Option<PseudoElement>) -> Vec<String> {
    let Some(style) = style_of(agent, node, pseudo) else {
        return Vec::new();
    };
    let mut names: Vec<String> = sorted_longhands()
        .iter()
        .map(|id| id.name().to_owned())
        .collect();
    let mut custom: Vec<String> = style.custom.keys().cloned().collect();
    custom.sort();
    names.extend(custom);
    names
}

/// El valor resuelto de `name` para `node`.
pub fn property_value(
    agent: &mut Agent,
    node: NodeRef,
    pseudo: Option<PseudoElement>,
    name: &str,
) -> String {
    let Some(style) = style_of(agent, node, pseudo) else {
        return String::new();
    };
    match property(name) {
        Some(Property::Custom) => style.get_css(name).unwrap_or_default(),
        Some(Property::Longhand(id)) => resolved(agent, node, pseudo, &style, id.name()),
        Some(Property::Shorthand(sh)) => {
            let mut block = DeclBlock::default();
            for l in sh.longhands {
                let v = resolved(agent, node, pseudo, &style, l.name());
                block.set_declaration(l.name(), v, false);
            }
            block.get_property_value(sh.name)
        }
        None => String::new(),
    }
}

fn px(v: f32) -> String {
    format!("{}px", number(f64::from(v)))
}

/// El valor resuelto de un longhand (CSSOM §9).
fn resolved(
    agent: &mut Agent,
    node: NodeRef,
    pseudo: Option<PseudoElement>,
    style: &ComputedStyle,
    name: &str,
) -> String {
    let computed = style.get_css(name).unwrap_or_default();
    let layout_dependent = matches!(
        name,
        "width"
            | "height"
            | "margin-top"
            | "margin-right"
            | "margin-bottom"
            | "margin-left"
            | "padding-top"
            | "padding-right"
            | "padding-bottom"
            | "padding-left"
            | "top"
            | "right"
            | "bottom"
            | "left"
    );
    if !layout_dependent || pseudo.is_some() || style.display == Display::None {
        return computed;
    }
    let tree = current_layout(agent);
    let frags = view::element_fragments(&tree, node.node);
    let Some(f) = frags.first().copied() else {
        return computed;
    };
    let cb = crate::with_doc(agent, 0, |d| {
        view::containing_block_rect(d, &tree, node.node)
    });
    used_value(f, style, name, cb).unwrap_or(computed)
}

fn used_value(
    f: &Fragment,
    style: &ComputedStyle,
    name: &str,
    cb: Option<bf_layout::Rect>,
) -> Option<String> {
    let inline_box = matches!(f.kind, bf_layout::FragmentKind::Inline);
    Some(match name {
        "width" | "height" => {
            // `width`/`height` no se aplican a las cajas inline no reemplazadas.
            if inline_box {
                return None;
            }
            let content = f.content_rect();
            let border_box = style.box_sizing == BoxSizing::BorderBox;
            if name == "width" {
                px(if border_box {
                    f.rect.width
                } else {
                    content.width
                })
            } else {
                px(if border_box {
                    f.rect.height
                } else {
                    content.height
                })
            }
        }
        "margin-top" => px(f.margin.top),
        "margin-right" => px(f.margin.right),
        "margin-bottom" => px(f.margin.bottom),
        "margin-left" => px(f.margin.left),
        "padding-top" => px(f.padding.top),
        "padding-right" => px(f.padding.right),
        "padding-bottom" => px(f.padding.bottom),
        "padding-left" => px(f.padding.left),
        "top" | "right" | "bottom" | "left" => {
            let computed = style.get_css(name)?;
            if style.position == Position::Static {
                return Some(computed);
            }
            if computed.ends_with("px") {
                return Some(computed);
            }
            // Las cajas posicionadas: la distancia usada al bloque contenedor.
            let cb = cb?;
            let r = f.margin_rect();
            match name {
                "top" => px(r.y - cb.y),
                "left" => px(r.x - cb.x),
                "bottom" => px(cb.bottom() - r.bottom()),
                _ => px(cb.right() - r.right()),
            }
        }
        _ => return None,
    })
}
