//! Las propiedades con nombre de `Window` (HTML §7.2.2.3, "named access on the Window
//! object"): el objeto `WindowProperties` del prototipo del global expone por su `id` los
//! elementos HTML del documento (y por su `name` los `embed`, `form`, `img` y `object`). **E1.**

use std::collections::HashMap;

use bf_dom::collections::{CollectionFilter, HtmlCollection};
use bf_dom::{Document, DomString, Namespace, NodeData, NodeId};
use bf_js::heap::Gc;
use bf_js::host::{HostClass, HostClassId, HostExotic};
use bf_js::object::Object;
use bf_js::string::JsString;
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::{Agent, RealmId};

use crate::{NodeRef, WebContextHandle};

/// El payload de `WindowProperties`.
#[derive(Debug, Clone, Copy)]
struct WindowProps {
    realm: RealmId,
}

#[derive(Debug, Clone, Copy)]
struct NamedClasses {
    window_properties: HostClassId,
}

/// Los nombres soportados del documento, por versión.
#[derive(Debug, Default)]
struct NamedCache {
    version: Option<u64>,
    order: Vec<DomString>,
    map: HashMap<DomString, Vec<NodeId>>,
}

/// ¿Expone `e` su atributo `name`?
fn exposes_name(local: &str) -> bool {
    matches!(local, "embed" | "form" | "img" | "object")
}

/// Los elementos con nombre `name` del documento (en tree order): el filtro de la colección
/// que devuelve el getter cuando hay varios.
pub fn window_named_elements(doc: &Document, name: &DomString) -> Vec<NodeId> {
    doc.descendants(doc.root())
        .into_iter()
        .filter(|&n| element_has_name(doc, n, name))
        .collect()
}

fn element_has_name(doc: &Document, n: NodeId, name: &DomString) -> bool {
    let Some(e) = doc.element(n) else {
        return false;
    };
    if e.ns != Namespace::Html || name.is_empty() {
        return false;
    }
    e.attrs.iter().any(|a| {
        a.ns.is_none()
            && a.value == *name
            && (a.name == "id" || (a.name == "name" && exposes_name(&e.name.to_string())))
    })
}

fn build(doc: &Document) -> (Vec<DomString>, HashMap<DomString, Vec<NodeId>>) {
    let mut order = Vec::new();
    let mut map: HashMap<DomString, Vec<NodeId>> = HashMap::new();
    for n in doc.descendants(doc.root()) {
        let NodeData::Element(e) = &doc.get(n).data else {
            continue;
        };
        if e.ns != Namespace::Html {
            continue;
        }
        let local = e.name.to_string();
        for a in &e.attrs {
            if a.ns.is_some() || a.value.is_empty() {
                continue;
            }
            if a.name == "id" || (a.name == "name" && exposes_name(&local)) {
                let list = map.entry(a.value.clone()).or_default();
                if list.is_empty() {
                    order.push(a.value.clone());
                }
                if !list.contains(&n) {
                    list.push(n);
                }
            }
        }
    }
    (order, map)
}

/// Los nodos con nombre `name` (con la caché al día).
fn lookup(
    agent: &mut Agent,
    realm: RealmId,
    name: Option<&DomString>,
) -> (Vec<DomString>, Vec<NodeId>) {
    let ctx = WebContextHandle { realm };
    let Some(wc) = agent.realm_data::<crate::WebContext>(realm) else {
        return (Vec::new(), Vec::new());
    };
    let version = wc.document().version();
    let fresh = agent
        .realm_data::<NamedCache>(realm)
        .is_some_and(|c| c.version == Some(version));
    if !fresh {
        let (order, map) = build(ctx.get(agent).document());
        agent.set_realm_data(
            realm,
            NamedCache {
                version: Some(version),
                order,
                map,
            },
        );
    }
    let cache = agent.realm_data::<NamedCache>(realm).expect("NamedCache");
    match name {
        Some(n) => (Vec::new(), cache.map.get(n).cloned().unwrap_or_default()),
        None => (cache.order.clone(), Vec::new()),
    }
}

fn realm_of(agent: &Agent, o: Gc<Object>) -> Option<RealmId> {
    agent
        .host_unwrap::<WindowProps>(&Value::Object(o))
        .map(|p| p.realm)
}

fn named(agent: &mut Agent, o: Gc<Object>, name: &JsString) -> Option<Value> {
    let realm = realm_of(agent, o)?;
    let name = DomString::from_utf16(name.units());
    let (_, nodes) = lookup(agent, realm, Some(&name));
    if nodes.is_empty() {
        return None;
    }
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let v = if nodes.len() == 1 {
        crate::dom::wrap_node(agent, NodeRef::new(0, nodes[0]))
    } else {
        let root = WebContextHandle { realm }.get(agent).document().root();
        let class = crate::dom::classes(agent).html_collection;
        let coll = HtmlCollection::new(root, CollectionFilter::WindowNamed(name));
        Value::Object(agent.host_create(
            class,
            Box::new(crate::dom::collections::HtmlCollectionData { doc: 0, coll }),
        ))
    };
    agent.current_realm = saved;
    Some(v)
}

fn names(agent: &mut Agent, o: Gc<Object>) -> Vec<JsString> {
    let Some(realm) = realm_of(agent, o) else {
        return Vec::new();
    };
    lookup(agent, realm, None)
        .0
        .into_iter()
        .map(|n| JsString::from_units(n.code_units().collect::<Vec<u16>>()))
        .collect()
}

/// Crea el `WindowProperties` del global de `realm` con prototipo `proto`.
pub fn window_properties(
    agent: &mut Agent,
    realm: RealmId,
    proto: Option<Gc<Object>>,
) -> Gc<Object> {
    if agent.embedder_data::<NamedClasses>().is_none() {
        let window_properties =
            agent.register_host_class(HostClass::new("WindowProperties").exposed(false).exotic(
                HostExotic {
                    named: Some(named),
                    names: Some(names),
                    named_enumerable: false,
                    ..HostExotic::default()
                },
            ));
        agent.set_embedder_data(NamedClasses { window_properties });
    }
    let class = agent
        .embedder_data::<NamedClasses>()
        .expect("NamedClasses")
        .window_properties;
    let o = agent.host_create(class, Box::new(WindowProps { realm }));
    agent.heap[o].proto = proto;
    let tag = PropertyKey::Symbol(agent.well_known.to_string_tag);
    agent.put_data(o, tag, Value::str("WindowProperties"), false, false, true);
    o
}
