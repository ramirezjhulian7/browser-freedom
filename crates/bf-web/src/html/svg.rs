//! E1: las interfaces del DOM de SVG (SVG 2): `SVGElement` (§4.4), `SVGGraphicsElement` (§4.5),
//! `SVGGeometryElement` (§4.6), `SVGSVGElement` (§5.11) y las de cada elemento. Los elementos
//! del namespace SVG son instancias de la suya (`dom::class_for`); los que el motor no conoce,
//! de `SVGElement` (§4.4: "unknown elements in the SVG namespace").
//!
//! Lo que hay de cada interfaz es lo que usan los sitios para detectar SVG y para medirlo:
//! - `SVGElement`: `className` (un `SVGAnimatedString`), `ownerSVGElement`, `viewportElement`,
//!   y de `HTMLOrSVGElement` (HTML §3.2.6) `dataset`, `tabIndex`, `focus()` y `blur()`, con los
//!   manejadores de `GlobalEventHandlers`. `style` viene de `Element` (CSSOM).
//! - `SVGGraphicsElement`: `getBBox()` (aquí, la caja de la maqueta), `getCTM()` y
//!   `getScreenCTM()` (la identidad: el motor todavía no compone las transformaciones de SVG).
//! - `SVGSVGElement`: `createSVGPoint()`, `createSVGMatrix()`, `createSVGRect()` (§5.11.2).
//!
//! No están las propiedades animadas de longitud (`x.baseVal` y compañía) ni `SVGLength`.

use bf_infra::Namespace;
use bf_js::host::{HostClass, HostClassId};
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use crate::dom::{self, this_node, wrap_opt};
use crate::{NodeRef, with_doc};

/// Las clases de SVG (una vez por agente).
#[derive(Debug, Clone, Copy)]
struct SvgClasses {
    animated_string: HostClassId,
}

/// Los elementos SVG y su interfaz, con la interfaz padre: `(local name, interfaz, padre)`. Los
/// padres que no son elementos (`SVGGradientElement`, `SVGTextContentElement`…) están antes.
const ELEMENTS: &[(&str, &str, &str)] = &[
    ("svg", "SVGSVGElement", "SVGGraphicsElement"),
    ("g", "SVGGElement", "SVGGraphicsElement"),
    ("defs", "SVGDefsElement", "SVGGraphicsElement"),
    ("use", "SVGUseElement", "SVGGraphicsElement"),
    ("image", "SVGImageElement", "SVGGraphicsElement"),
    ("switch", "SVGSwitchElement", "SVGGraphicsElement"),
    ("symbol", "SVGSymbolElement", "SVGGraphicsElement"),
    ("a", "SVGAElement", "SVGGraphicsElement"),
    (
        "foreignObject",
        "SVGForeignObjectElement",
        "SVGGraphicsElement",
    ),
    ("path", "SVGPathElement", "SVGGeometryElement"),
    ("rect", "SVGRectElement", "SVGGeometryElement"),
    ("circle", "SVGCircleElement", "SVGGeometryElement"),
    ("ellipse", "SVGEllipseElement", "SVGGeometryElement"),
    ("line", "SVGLineElement", "SVGGeometryElement"),
    ("polyline", "SVGPolylineElement", "SVGGeometryElement"),
    ("polygon", "SVGPolygonElement", "SVGGeometryElement"),
    ("text", "SVGTextElement", "SVGTextPositioningElement"),
    ("tspan", "SVGTSpanElement", "SVGTextPositioningElement"),
    ("textPath", "SVGTextPathElement", "SVGTextContentElement"),
    (
        "linearGradient",
        "SVGLinearGradientElement",
        "SVGGradientElement",
    ),
    (
        "radialGradient",
        "SVGRadialGradientElement",
        "SVGGradientElement",
    ),
    ("stop", "SVGStopElement", "SVGElement"),
    ("pattern", "SVGPatternElement", "SVGElement"),
    ("clipPath", "SVGClipPathElement", "SVGElement"),
    ("mask", "SVGMaskElement", "SVGElement"),
    ("marker", "SVGMarkerElement", "SVGElement"),
    ("filter", "SVGFilterElement", "SVGElement"),
    ("title", "SVGTitleElement", "SVGElement"),
    ("desc", "SVGDescElement", "SVGElement"),
    ("metadata", "SVGMetadataElement", "SVGElement"),
    ("style", "SVGStyleElement", "SVGElement"),
    ("script", "SVGScriptElement", "SVGElement"),
    ("view", "SVGViewElement", "SVGElement"),
];

/// Las interfaces intermedias: `(interfaz, padre)`, en orden de dependencia.
const ABSTRACT: &[(&str, &str)] = &[
    ("SVGGeometryElement", "SVGGraphicsElement"),
    ("SVGTextContentElement", "SVGGraphicsElement"),
    ("SVGTextPositioningElement", "SVGTextContentElement"),
    ("SVGGradientElement", "SVGElement"),
];

/// Registra las interfaces (la primera vez) y la clase de cada elemento SVG.
pub fn register(agent: &mut Agent) {
    if agent.embedder_data::<SvgClasses>().is_some() {
        return;
    }
    let element = dom::classes(agent).element;
    let mut ids: std::collections::HashMap<&'static str, HostClassId> =
        std::collections::HashMap::new();
    let svg_element = agent.register_host_class(svg_element_class(element));
    ids.insert("SVGElement", svg_element);
    let graphics = agent.register_host_class(
        HostClass::new("SVGGraphicsElement")
            .parent(svg_element)
            .method("getBBox", 0, get_bbox)
            .method("getCTM", 0, identity_matrix)
            .method("getScreenCTM", 0, identity_matrix),
    );
    ids.insert("SVGGraphicsElement", graphics);
    for (name, parent) in ABSTRACT {
        let p = ids[parent];
        let id = agent.register_host_class(HostClass::new(name).parent(p));
        ids.insert(name, id);
    }
    for (local, name, parent) in ELEMENTS {
        let p = ids[parent];
        let mut class = HostClass::new(name).parent(p);
        if *local == "svg" {
            class = class
                .method("createSVGPoint", 0, create_svg_point)
                .method("createSVGMatrix", 0, identity_matrix)
                .method("createSVGRect", 0, create_svg_rect);
        }
        let id = agent.register_host_class(class);
        ids.insert(name, id);
        dom::set_element_class(agent, Namespace::Svg, local, id);
    }
    dom::set_svg_default_class(agent, svg_element);
    let animated_string = agent.register_host_class(
        HostClass::new("SVGAnimatedString")
            .attribute("baseVal", Some(get_base_val), Some(set_base_val))
            .attribute("animVal", Some(get_base_val), None),
    );
    agent.set_embedder_data(SvgClasses { animated_string });
}

fn svg_element_class(element: HostClassId) -> HostClass {
    crate::dom::events::with_event_handlers(
        HostClass::new("SVGElement"),
        crate::dom::events::GLOBAL_EVENT_HANDLERS,
    )
    .parent(element)
    .attribute("className", Some(get_class_name), None)
    .attribute("ownerSVGElement", Some(owner_svg_element), None)
    .attribute("viewportElement", Some(owner_svg_element), None)
    // `HTMLOrSVGElement` (HTML §3.2.6).
    .attribute("dataset", Some(super::dataset::get_dataset), None)
    .attribute(
        "tabIndex",
        Some(super::elements::get_tab_index),
        Some(super::elements::set_tab_index),
    )
    .method("focus", 0, super::misc::focus)
    .method("blur", 0, super::misc::blur)
}

/// `className` (§4.4): un `SVGAnimatedString` que refleja el atributo `class`.
fn get_class_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let Some(c) = agent.embedder_data::<SvgClasses>().copied() else {
        return Ok(Value::Undefined);
    };
    Ok(Value::Object(
        agent.host_create(c.animated_string, Box::new(n)),
    ))
}

fn get_base_val(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(n) = agent.host_unwrap::<NodeRef>(&a.this).copied() else {
        return agent.type_error("Illegal invocation");
    };
    let v = with_doc(agent, n.doc, |d| {
        d.element(n.node)
            .and_then(|e| e.attr("class"))
            .unwrap_or("")
            .to_owned()
    });
    Ok(Value::str(&v))
}

fn set_base_val(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(n) = agent.host_unwrap::<NodeRef>(&a.this).copied() else {
        return agent.type_error("Illegal invocation");
    };
    let v = agent.to_string(&a.arg(0))?.to_string();
    super::elements::set_attr(agent, n, "class", v.as_str().into())?;
    Ok(Value::Undefined)
}

/// `ownerSVGElement` (§4.4): el `<svg>` antecesor más cercano (`null` en el más externo).
fn owner_svg_element(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let owner = with_doc(agent, n.doc, |d| {
        let mut cur = d.parent(n.node);
        while let Some(p) = cur {
            if d.element(p)
                .is_some_and(|e| e.ns == Namespace::Svg && &*e.name == "svg")
            {
                return Some(p);
            }
            cur = d.parent(p);
        }
        None
    });
    Ok(wrap_opt(agent, n.doc, owner))
}

/// `getBBox()` (§4.5): la caja de la maqueta del elemento (en coordenadas del viewport: el
/// motor no compone todavía las transformaciones de SVG).
fn get_bbox(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let (x, y, width, height) = crate::cssom::view::client_rect(agent, n).unwrap_or_default();
    Ok(crate::cssom::view::new_rect(
        agent,
        crate::cssom::view::RectData {
            x: f64::from(x),
            y: f64::from(y),
            width: f64::from(width),
            height: f64::from(height),
        },
    ))
}

/// Un objeto nuevo con el constructor global `name` del realm en curso.
fn construct_global(agent: &mut Agent, name: &str, args: &[Value]) -> JsResult<Value> {
    let g = agent.realms[agent.current_realm].global_object;
    let ctor = agent.get(g, &PropertyKey::from(name), Value::Object(g))?;
    let Value::Object(c) = ctor else {
        return agent.type_error(&format!("{name} is not a constructor"));
    };
    agent.construct(c, args, None)
}

/// `getCTM()`, `getScreenCTM()` y `createSVGMatrix()`: un `DOMMatrix` identidad.
fn identity_matrix(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    construct_global(agent, "DOMMatrix", &[])
}

/// `createSVGPoint()` (§5.11.2): un `DOMPoint` (0, 0).
fn create_svg_point(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    construct_global(agent, "DOMPoint", &[])
}

/// `createSVGRect()` (§5.11.2): un `DOMRect` (0, 0, 0, 0).
fn create_svg_rect(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    construct_global(agent, "DOMRect", &[])
}
