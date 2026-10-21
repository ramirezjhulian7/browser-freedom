//! `DOMParser` (HTML §8.5.1): `parseFromString(string, type)` crea un documento nuevo en el
//! [`crate::WebContext`] con el parser HTML (scripting deshabilitado) o el XML de `bf-html`.

use bf_js::host::HostClass;
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use crate::dom::{classes, to_dom_string, wrap_node};
use crate::{NodeRef, WebContextHandle};

/// El payload de un `DOMParser` (no tiene estado).
#[derive(Debug, Clone, Copy)]
pub struct DomParserData;

pub fn dom_parser_class() -> HostClass {
    HostClass::new("DOMParser")
        .constructor(ctor, 0)
        .method("parseFromString", 2, parse_from_string)
}

fn ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let class = classes(agent).dom_parser;
    let nt = a.new_target.expect("new.target");
    let o = agent.host_create_from_new_target(nt, class, None, Box::new(DomParserData))?;
    Ok(Value::Object(o))
}

/// `parseFromString(string, type)` (HTML §8.5.1.1).
fn parse_from_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    if agent.host_unwrap::<DomParserData>(&a.this).is_none() {
        return agent.type_error("Illegal invocation");
    }
    let text = to_dom_string(agent, &a.arg(0))?.to_string();
    let type_ = to_dom_string(agent, &a.arg(1))?.to_string();
    let mut doc = match type_.as_str() {
        "text/html" => {
            let opts = bf_html::ParseOptions {
                scripting: false,
                iframe_srcdoc: false,
            };
            let (mut d, _) = bf_html::parse_document(&text, &opts);
            d.content_type = "text/html".into();
            d
        }
        "text/xml" | "application/xml" | "application/xhtml+xml" | "image/svg+xml" => {
            let mut d = bf_html::parse_xml_document(&text);
            d.is_html = false;
            d.content_type = type_.as_str().into();
            d
        }
        _ => {
            return agent.type_error(&format!(
                "Failed to execute 'parseFromString' on 'DOMParser': The provided value \
                 '{type_}' is not a valid enum value of type DOMParserSupportedType."
            ));
        }
    };
    // La URL es la del documento asociado a la ventana (paso 3).
    let ctx = WebContextHandle::current(agent);
    doc.url = ctx.get(agent).document().url.clone();
    let i = ctx.get_mut(agent).add_document(doc);
    let root = ctx.get(agent).doc(i).root();
    Ok(wrap_node(agent, NodeRef::new(i, root)))
}
