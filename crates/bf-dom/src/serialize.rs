//! Serialización del árbol en el formato de las pruebas de tree construction de html5lib/WPT
//! (`html/syntax/parsing/resources/README.md`).

use std::cmp::Ordering;
use std::fmt::Write;

use crate::{Document, NodeData, NodeId};
use bf_infra::Namespace;

/// Serializa los hijos de `root` (no `root` mismo): una línea `| ` por nodo, con dos espacios de
/// indentación por nivel. Para un documento se pasa `doc.root()`; para un fragmento, el
/// elemento `html` raíz del parseo de fragmento. El resultado no termina en salto de línea.
pub fn test_serialize(doc: &Document, root: NodeId) -> String {
    let mut out = String::new();
    for child in doc.children(root) {
        node(doc, child, 0, &mut out);
    }
    out.pop();
    out
}

/// Abre una línea con su indentación. Cada línea se cierra con `\n`.
fn line(out: &mut String, depth: usize) {
    out.push_str("| ");
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn node(doc: &Document, id: NodeId, depth: usize, out: &mut String) {
    line(out, depth);
    match &doc.get(id).data {
        NodeData::Document | NodeData::DocumentFragment => out.push_str("#fragment\n"),
        NodeData::Doctype {
            name,
            public_id,
            system_id,
        } => {
            if public_id.is_empty() && system_id.is_empty() {
                let _ = writeln!(out, "<!DOCTYPE {name}>");
            } else {
                let _ = writeln!(out, "<!DOCTYPE {name} \"{public_id}\" \"{system_id}\">");
            }
        }
        NodeData::Text(text) => {
            let _ = writeln!(out, "\"{text}\"");
        }
        NodeData::Comment(data) => {
            let _ = writeln!(out, "<!-- {data} -->");
        }
        NodeData::ProcessingInstruction { target, data } => {
            let _ = writeln!(out, "<?{target} {data}?>");
        }
        NodeData::Element(element) => {
            let _ = writeln!(out, "<{}{}>", element_prefix(element.ns), element.name);
            let mut attrs: Vec<(String, String)> = element
                .attrs
                .iter()
                .map(|a| {
                    (
                        format!("{}{}", attr_prefix(a.ns), a.name),
                        a.value.to_string(),
                    )
                })
                .collect();
            attrs.sort_by(|a, b| utf16_cmp(&a.0, &b.0));
            for (name, value) in attrs {
                line(out, depth + 1);
                let _ = writeln!(out, "{name}=\"{value}\"");
            }
            if let Some(contents) = element.template_contents {
                line(out, depth + 1);
                out.push_str("content\n");
                for child in doc.children(contents) {
                    node(doc, child, depth + 2, out);
                }
            }
        }
    }
    for child in doc.children(id) {
        node(doc, child, depth + 1, out);
    }
}

fn element_prefix(ns: Namespace) -> &'static str {
    match ns {
        Namespace::Svg => "svg ",
        Namespace::MathMl => "math ",
        _ => "",
    }
}

fn attr_prefix(ns: Option<Namespace>) -> &'static str {
    match ns {
        Some(Namespace::XLink) => "xlink ",
        Some(Namespace::Xml) => "xml ",
        Some(Namespace::XmlNs) => "xmlns ",
        _ => "",
    }
}

/// Orden lexicográfico por code units UTF-16, como exige el formato de la suite.
fn utf16_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}
