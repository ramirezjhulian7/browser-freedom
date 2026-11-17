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
//! - Longitudes animadas (§4.6.2 `SVGLength`, §4.6.3 `SVGAnimatedLength`): `x`, `y`, `width`,
//!   `height`, `rx`, `ry`, `cx`, `cy`, `r`, `x1`… de los elementos que las tienen, que reflejan su
//!   atributo. `animVal` es de solo lectura y vale lo mismo que `baseVal` (no hay animaciones
//!   SMIL). Los porcentajes se resuelven contra el `viewBox` (o `width`/`height`) del `<svg>`
//!   antecesor; `em` y `ex`, contra 16 px y 8 px (sin la fuente calculada).

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
    animated_length: HostClassId,
    length: HostClassId,
}

/// Los atributos de longitud de cada elemento (SVG 2: §10.2 `rect`, §10.3 `circle`, §10.4
/// `ellipse`, §10.5 `line`, §5.11 `svg`, §12.3 `image`, §5.6 `use`, §23.2 `foreignObject`, y los
/// de las regiones de `pattern`, `mask` y `filter`).
fn length_attrs(local: &str) -> &'static [&'static str] {
    match local {
        "rect" => &["x", "y", "width", "height", "rx", "ry"],
        "circle" => &["cx", "cy", "r"],
        "ellipse" => &["cx", "cy", "rx", "ry"],
        "line" => &["x1", "y1", "x2", "y2"],
        "svg" | "image" | "use" | "foreignObject" | "pattern" | "mask" | "filter" => {
            &["x", "y", "width", "height"]
        }
        _ => &[],
    }
}

/// Los nombres de atributo de longitud conocidos (su índice entra en la clave de identidad).
const LENGTH_NAMES: [&str; 13] = [
    "x", "y", "width", "height", "rx", "ry", "cx", "cy", "r", "x1", "y1", "x2", "y2",
];

/// El getter de cada atributo de longitud (una función nativa por nombre).
macro_rules! length_getters {
    ($($f:ident => $i:expr),* $(,)?) => {
        $(fn $f(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
            animated_length(agent, a, $i)
        })*
        fn length_getter(name: &str) -> Option<bf_js::vm::function::NativeFn> {
            match LENGTH_NAMES.iter().position(|n| *n == name)? {
                $($i => Some($f),)*
                _ => None,
            }
        }
    };
}
length_getters!(
    len_x => 0, len_y => 1, len_width => 2, len_height => 3, len_rx => 4, len_ry => 5,
    len_cx => 6, len_cy => 7, len_r => 8, len_x1 => 9, len_y1 => 10, len_x2 => 11, len_y2 => 12,
);

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
        for attr in length_attrs(local) {
            if let Some(getter) = length_getter(attr) {
                class = class.attribute(attr, Some(getter), None);
            }
        }
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
    let animated_length = agent.register_host_class(
        HostClass::new("SVGAnimatedLength")
            .attribute("baseVal", Some(get_base_length), None)
            .attribute("animVal", Some(get_anim_length), None),
    );
    let mut length = HostClass::new("SVGLength")
        .attribute("unitType", Some(length_unit_type), None)
        .attribute("value", Some(length_value), Some(set_length_value))
        .attribute(
            "valueInSpecifiedUnits",
            Some(length_specified),
            Some(set_length_specified),
        )
        .attribute(
            "valueAsString",
            Some(length_as_string),
            Some(set_length_as_string),
        )
        .method("newValueSpecifiedUnits", 2, new_value_specified_units)
        .method("convertToSpecifiedUnits", 1, convert_to_specified_units);
    for (i, unit) in UNITS.iter().enumerate() {
        length = length.constant(unit.0, i as f64);
    }
    let length = agent.register_host_class(length);
    agent.set_embedder_data(SvgClasses {
        animated_string,
        animated_length,
        length,
    });
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

// --- Longitudes (§4.6.2 SVGLength, §4.6.3 SVGAnimatedLength) ---

/// Las unidades de `SVGLength`, por `unitType`: la constante y el sufijo.
const UNITS: [(&str, &str); 11] = [
    ("SVG_LENGTHTYPE_UNKNOWN", ""),
    ("SVG_LENGTHTYPE_NUMBER", ""),
    ("SVG_LENGTHTYPE_PERCENTAGE", "%"),
    ("SVG_LENGTHTYPE_EMS", "em"),
    ("SVG_LENGTHTYPE_EXS", "ex"),
    ("SVG_LENGTHTYPE_PX", "px"),
    ("SVG_LENGTHTYPE_CM", "cm"),
    ("SVG_LENGTHTYPE_MM", "mm"),
    ("SVG_LENGTHTYPE_IN", "in"),
    ("SVG_LENGTHTYPE_PT", "pt"),
    ("SVG_LENGTHTYPE_PC", "pc"),
];

/// Una longitud reflejada: el elemento, el atributo (índice en [`LENGTH_NAMES`]) y si es
/// `animVal` (de solo lectura).
#[derive(Debug, Clone, Copy)]
struct LengthRef {
    node: NodeRef,
    attr: usize,
    anim: bool,
}

/// La clave de identidad de los objetos de una longitud (`[SameObject]`): `level` 0 es el
/// `SVGAnimatedLength`, 1 su `baseVal` y 2 su `animVal`.
fn length_key(n: NodeRef, attr: usize, level: u64) -> bf_js::host::HostKey {
    bf_js::host::HostKey(n.key().0 | (1 << 61) | ((attr as u64) << 48) | (level << 44))
}

/// El `SVGAnimatedLength` del atributo `attr` de `this`.
fn animated_length(agent: &mut Agent, a: &CallArgs<'_>, attr: usize) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let Some(c) = agent.embedder_data::<SvgClasses>().copied() else {
        return Ok(Value::Undefined);
    };
    let realm = agent.current_realm;
    let r = LengthRef {
        node: n,
        attr,
        anim: false,
    };
    let o = agent.host_wrap(
        realm,
        length_key(n, attr, 0),
        c.animated_length,
        Box::new(r),
    );
    Ok(Value::Object(o))
}

fn length_object(agent: &mut Agent, a: &CallArgs<'_>, anim: bool) -> JsResult<Value> {
    let Some(r) = agent.host_unwrap::<LengthRef>(&a.this).copied() else {
        return agent.type_error("Illegal invocation");
    };
    let Some(c) = agent.embedder_data::<SvgClasses>().copied() else {
        return Ok(Value::Undefined);
    };
    let realm = agent.current_realm;
    let key = length_key(r.node, r.attr, if anim { 2 } else { 1 });
    let o = agent.host_wrap(realm, key, c.length, Box::new(LengthRef { anim, ..r }));
    Ok(Value::Object(o))
}

fn get_base_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    length_object(agent, a, false)
}

fn get_anim_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    length_object(agent, a, true)
}

fn this_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<LengthRef> {
    match agent.host_unwrap::<LengthRef>(&a.this).copied() {
        Some(r) => Ok(r),
        None => agent.type_error("Illegal invocation"),
    }
}

/// Un valor de longitud de SVG: `<number><unidad>?` (SVG 2 §4.6.2; las unidades sin distinguir
/// mayúsculas). `None` si no es válido.
fn parse_length(s: &str) -> Option<(f64, usize)> {
    let s = s.trim();
    let end = s
        .char_indices()
        .find(|&(i, c)| {
            !(c.is_ascii_digit()
                || c == '.'
                || ((c == '+' || c == '-')
                    && (i == 0 || matches!(s.as_bytes()[i - 1], b'e' | b'E')))
                || ((c == 'e' || c == 'E')
                    && s[i + 1..]
                        .starts_with(|d: char| d.is_ascii_digit() || d == '-' || d == '+')))
        })
        .map_or(s.len(), |(i, _)| i);
    let v: f64 = s[..end].parse().ok()?;
    let unit = s[end..].to_ascii_lowercase();
    let t = if unit.is_empty() {
        1
    } else {
        UNITS.iter().skip(2).position(|u| u.1 == unit)? + 2
    };
    v.is_finite().then_some((v, t))
}

/// El valor actual del atributo: el número en sus unidades y la unidad (`0` sin unidad si falta
/// o no es válido, el valor inicial de §10).
fn current(agent: &mut Agent, r: LengthRef) -> (f64, usize) {
    let name = LENGTH_NAMES[r.attr];
    with_doc(agent, r.node.doc, |d| {
        d.element(r.node.node)
            .and_then(|e| e.attr(name))
            .and_then(parse_length)
            .unwrap_or((0.0, 1))
    })
}

/// La dimensión del viewport contra la que se resuelven los porcentajes de `r` (§8.9: la
/// anchura para las coordenadas horizontales, la altura para las verticales y la diagonal
/// normalizada para `r`): la del `viewBox` del `<svg>` antecesor, o su `width`/`height`.
fn percent_base(agent: &mut Agent, r: LengthRef) -> f64 {
    let name = LENGTH_NAMES[r.attr];
    let (w, h) = with_doc(agent, r.node.doc, |d| {
        let mut cur = d.parent(r.node.node);
        while let Some(p) = cur {
            if let Some(e) = d.element(p)
                && e.ns == Namespace::Svg
                && &*e.name == "svg"
            {
                if let Some(vb) = e.attr("viewBox") {
                    let n: Vec<f64> = vb
                        .split(|c: char| c == ',' || c.is_whitespace())
                        .filter(|s| !s.is_empty())
                        .filter_map(|s| s.parse().ok())
                        .collect();
                    if n.len() == 4 {
                        return (n[2], n[3]);
                    }
                }
                let dim = |k: &str| {
                    e.attr(k)
                        .and_then(parse_length)
                        .filter(|(_, t)| *t == 1 || *t == 5)
                        .map_or(0.0, |(v, _)| v)
                };
                return (dim("width"), dim("height"));
            }
            cur = d.parent(p);
        }
        (0.0, 0.0)
    });
    match name {
        "x" | "width" | "cx" | "rx" | "x1" | "x2" => w,
        "y" | "height" | "cy" | "ry" | "y1" | "y2" => h,
        _ => ((w * w + h * h) / 2.0).sqrt(),
    }
}

/// Cuántas unidades de usuario es una unidad `t` (§4.6.2 y CSS Values §6.2).
fn user_units(agent: &mut Agent, r: LengthRef, t: usize) -> f64 {
    match t {
        2 => percent_base(agent, r) / 100.0,
        3 => 16.0,
        4 => 8.0,
        6 => 96.0 / 2.54,
        7 => 96.0 / 25.4,
        8 => 96.0,
        9 => 96.0 / 72.0,
        10 => 16.0,
        _ => 1.0,
    }
}

/// Escribe el atributo con `v` en la unidad `t` (sin animVal: §4.6.2 "reflect").
fn write(agent: &mut Agent, r: LengthRef, v: f64, t: usize) -> JsResult<()> {
    if r.anim {
        return crate::dom::throw_dom(agent, bf_dom::DomException::NoModificationAllowedError);
    }
    // Con la precisión de `float` (como guardan los navegadores las longitudes de SVG): la
    // representación más corta, sin el ruido de las conversiones (`2cm` → `20mm`, no
    // `19.999999999999996mm`).
    let text = format!("{}{}", v as f32, UNITS[t].1);
    super::elements::set_attr(agent, r.node, LENGTH_NAMES[r.attr], text.as_str().into())
}

fn length_unit_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_length(agent, a)?;
    Ok(Value::Number(current(agent, r).1 as f64))
}

fn length_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_length(agent, a)?;
    let (v, t) = current(agent, r);
    Ok(Value::Number(v * user_units(agent, r, t)))
}

/// `value = x` (en unidades de usuario): se guarda en la unidad que ya tenía.
fn set_length_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_length(agent, a)?;
    let x = bf_js::host::webidl::to_double(agent, &a.arg(0))?;
    let (_, t) = current(agent, r);
    let f = user_units(agent, r, t);
    let (v, t) = if f == 0.0 { (x, 1) } else { (x / f, t) };
    write(agent, r, v, t)?;
    Ok(Value::Undefined)
}

fn length_specified(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_length(agent, a)?;
    Ok(Value::Number(current(agent, r).0))
}

fn set_length_specified(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_length(agent, a)?;
    let x = bf_js::host::webidl::to_double(agent, &a.arg(0))?;
    let (_, t) = current(agent, r);
    write(agent, r, x, t)?;
    Ok(Value::Undefined)
}

fn length_as_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_length(agent, a)?;
    let (v, t) = current(agent, r);
    let num = agent.to_string(&Value::Number(v))?.to_string();
    Ok(Value::str(&format!("{num}{}", UNITS[t].1)))
}

/// `valueAsString = s`: `SyntaxError` si no es una longitud.
fn set_length_as_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_length(agent, a)?;
    let s = agent.to_string(&a.arg(0))?.to_string();
    let Some((v, t)) = parse_length(&s) else {
        return crate::dom::throw_dom(agent, bf_dom::DomException::SyntaxError);
    };
    write(agent, r, v, t)?;
    Ok(Value::Undefined)
}

/// La unidad de los métodos: `NotSupportedError` si no es una de las conocidas (§4.6.2).
fn unit_arg(agent: &mut Agent, v: &Value) -> JsResult<usize> {
    let t = bf_js::host::webidl::to_unsigned_short(agent, v)? as usize;
    if t == 0 || t >= UNITS.len() {
        return crate::dom::throw_dom(agent, bf_dom::DomException::NotSupportedError);
    }
    Ok(t)
}

fn new_value_specified_units(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_length(agent, a)?;
    let t = unit_arg(agent, &a.arg(0))?;
    let v = bf_js::host::webidl::to_double(agent, &a.arg(1))?;
    write(agent, r, v, t)?;
    Ok(Value::Undefined)
}

fn convert_to_specified_units(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_length(agent, a)?;
    let t = unit_arg(agent, &a.arg(0))?;
    let (v, old) = current(agent, r);
    let user = v * user_units(agent, r, old);
    let f = user_units(agent, r, t);
    let nv = if f == 0.0 { 0.0 } else { user / f };
    write(agent, r, nv, t)?;
    Ok(Value::Undefined)
}
