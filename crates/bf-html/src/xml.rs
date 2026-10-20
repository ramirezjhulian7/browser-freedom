//! Parser XML para documentos XHTML (`.xht`, `application/xhtml+xml`).
//!
//! Cubre lo que usan los documentos XHTML reales: prólogo, DOCTYPE (con subset interno, que se
//! ignora), elementos con namespaces por prefijo, autocierre, atributos, CDATA, comentarios,
//! processing instructions, y referencias de entidad: las cinco de XML, las numéricas y las
//! nombradas de HTML (los navegadores las resuelven para los DOCTYPE de XHTML sin cargar la
//! DTD). No valida: ante un error de buena formación sigue con lo que pueda en vez de mostrar
//! la página de error de los navegadores.
//!
//! El shell lo elige por el `Content-Type` de la respuesta (XML que no es HTML) o, sin él, por
//! la extensión del archivo.

use std::collections::HashMap;

use bf_dom::{Attribute, Document, Element, NodeData, NodeId, QuirksMode};
use bf_infra::Namespace;

use crate::tokenizer::named_entities::NAMED_ENTITIES;

/// El namespace de un URI; los desconocidos son `Namespace::Other`. El namespace ausente se
/// trata como XHTML (los elementos sin namespace todavía no tienen representación propia).
fn namespace_of(uri: &str) -> Namespace {
    Namespace::from_uri(uri).unwrap_or(Namespace::Html)
}

fn entity(name: &str) -> Option<String> {
    match name {
        "lt" => return Some("<".into()),
        "gt" => return Some(">".into()),
        "amp" => return Some("&".into()),
        "quot" => return Some("\"".into()),
        "apos" => return Some("'".into()),
        _ => {}
    }
    if let Some(num) = name.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse().ok()?,
        };
        return char::from_u32(code).map(String::from);
    }
    let key = format!("{name};");
    NAMED_ENTITIES
        .binary_search_by(|(n, _)| (*n).cmp(key.as_str()))
        .ok()
        .map(|i| NAMED_ENTITIES[i].1.to_owned())
}

/// Texto con las referencias de entidad resueltas (las desconocidas quedan literales).
fn decode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find(';') {
            Some(end) if end > 0 && end < 40 => match entity(&after[..end]) {
                Some(v) => {
                    out.push_str(&v);
                    rest = &after[end + 1..];
                }
                None => {
                    out.push('&');
                    rest = after;
                }
            },
            _ => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

struct Parser<'a> {
    input: &'a str,
    pos: usize,
    doc: Document,
    /// Elementos abiertos con sus declaraciones de namespace.
    stack: Vec<(NodeId, String, HashMap<String, String>)>,
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        &self.input[self.pos..]
    }

    fn current(&self) -> NodeId {
        self.stack.last().map_or(self.doc.root(), |(n, _, _)| *n)
    }

    fn lookup(&self, prefix: &str) -> Option<&str> {
        self.stack
            .iter()
            .rev()
            .find_map(|(_, _, scope)| scope.get(prefix).map(String::as_str))
    }

    fn append_text(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        let parent = self.current();
        if parent == self.doc.root() {
            // Texto fuera del elemento raíz: solo whitespace en un documento bien formado.
            return;
        }
        if let Some(last) = self.doc.get(parent).last_child()
            && let NodeData::Text(t) = &mut self.doc.get_mut(last).data
        {
            t.push_str(&text);
            return;
        }
        let node = self.doc.create(NodeData::Text(text.into()));
        self.doc.append(parent, node);
    }

    /// Hasta `end` (excluido), avanzando después de él; o hasta el final.
    fn take_until(&mut self, end: &str) -> &'a str {
        let rest = self.rest();
        match rest.find(end) {
            Some(i) => {
                self.pos += i + end.len();
                &rest[..i]
            }
            None => {
                self.pos = self.input.len();
                rest
            }
        }
    }

    fn doctype(&mut self) {
        // `<!DOCTYPE name PUBLIC "pub" "sys" [subset]>`
        let start = self.pos;
        let mut depth = 0;
        let mut quote: Option<char> = None;
        let mut end = self.input.len();
        for (i, c) in self.rest().char_indices() {
            match (quote, c) {
                (Some(q), c) if c == q => quote = None,
                (Some(_), _) => {}
                (None, '"' | '\'') => quote = Some(c),
                (None, '[') => depth += 1,
                (None, ']') => depth -= 1,
                (None, '>') if depth <= 0 => {
                    end = start + i;
                    break;
                }
                _ => {}
            }
        }
        let body = &self.input[start + "<!DOCTYPE".len()..end];
        self.pos = (end + 1).min(self.input.len());
        let mut words = body.split_ascii_whitespace();
        let name = words.next().unwrap_or("").to_owned();
        let quoted: Vec<&str> = body.split(['"', '\'']).skip(1).step_by(2).collect();
        let (public_id, system_id) = if body.contains("PUBLIC") {
            (quoted.first(), quoted.get(1))
        } else {
            (None, quoted.first())
        };
        let node = self.doc.create(NodeData::Doctype {
            name: name.into(),
            public_id: public_id
                .map_or_else(String::new, |s| (*s).to_owned())
                .into(),
            system_id: system_id
                .map_or_else(String::new, |s| (*s).to_owned())
                .into(),
        });
        let root = self.doc.root();
        self.doc.append(root, node);
    }

    fn name(&mut self) -> &'a str {
        let rest = self.rest();
        let end = rest
            .find(|c: char| c.is_whitespace() || matches!(c, '/' | '>' | '=' | '<'))
            .unwrap_or(rest.len());
        self.pos += end;
        &rest[..end]
    }

    fn skip_ws(&mut self) {
        let rest = self.rest();
        self.pos += rest.len() - rest.trim_start().len();
    }

    fn start_tag(&mut self) {
        self.pos += 1;
        let qname = self.name().to_owned();
        let mut raw_attrs: Vec<(String, String)> = Vec::new();
        let mut self_closing = false;
        loop {
            self.skip_ws();
            let rest = self.rest();
            if rest.is_empty() {
                break;
            }
            if rest.starts_with("/>") {
                self.pos += 2;
                self_closing = true;
                break;
            }
            if rest.starts_with('>') {
                self.pos += 1;
                break;
            }
            let name = self.name().to_owned();
            if name.is_empty() {
                // Carácter inesperado: se salta.
                self.pos += rest.chars().next().map_or(1, char::len_utf8);
                continue;
            }
            self.skip_ws();
            let mut value = String::new();
            if self.rest().starts_with('=') {
                self.pos += 1;
                self.skip_ws();
                let q = self.rest().chars().next();
                if let Some(q @ ('"' | '\'')) = q {
                    self.pos += 1;
                    value = decode(self.take_until(&q.to_string()));
                } else {
                    value = decode(self.name());
                }
            }
            // Normalización de valores de atributo (XML §3.3.3).
            let value = value.replace(['\t', '\n', '\r'], " ");
            raw_attrs.push((name, value));
        }
        let mut scope = HashMap::new();
        for (n, v) in &raw_attrs {
            if n == "xmlns" {
                scope.insert(String::new(), v.clone());
            } else if let Some(p) = n.strip_prefix("xmlns:") {
                scope.insert(p.to_owned(), v.clone());
            }
        }
        let (prefix, local) = match qname.split_once(':') {
            Some((p, l)) => (p.to_owned(), l.to_owned()),
            None => (String::new(), qname.clone()),
        };
        self.stack.push((self.current(), String::new(), scope));
        let ns = namespace_of(self.lookup(&prefix).unwrap_or(""));
        let attrs = raw_attrs
            .into_iter()
            .map(|(n, value)| {
                let (prefix, name, ns) = match n.split_once(':') {
                    Some(("xmlns", l)) => (
                        Some("xmlns".to_owned()),
                        l.to_owned(),
                        Some(Namespace::XmlNs),
                    ),
                    Some(("xml", l)) => {
                        (Some("xml".to_owned()), l.to_owned(), Some(Namespace::Xml))
                    }
                    Some((p, l)) => (
                        Some(p.to_owned()),
                        l.to_owned(),
                        self.lookup(p).map(namespace_of),
                    ),
                    None if n == "xmlns" => (None, n, Some(Namespace::XmlNs)),
                    None => (None, n, None),
                };
                Attribute {
                    ns,
                    prefix: prefix.map(Into::into),
                    name: name.into(),
                    value: value.into(),
                }
            })
            .collect();
        let (parent, _, scope) = self.stack.pop().expect("recién apilado");
        let node = self.doc.create(NodeData::Element(Element {
            ns,
            prefix: (!prefix.is_empty()).then(|| prefix.as_str().into()),
            name: local.into(),
            attrs,
            template_contents: None,
        }));
        self.doc.append(parent, node);
        if !self_closing {
            self.stack.push((node, qname, scope));
        }
    }

    fn end_tag(&mut self) {
        self.pos += 2;
        let name = self.name().to_owned();
        self.take_until(">");
        // Cierra hasta el elemento con ese nombre (si está abierto).
        if let Some(i) = self.stack.iter().rposition(|(_, n, _)| *n == name) {
            self.stack.truncate(i);
        }
    }

    fn run(mut self) -> Document {
        while self.pos < self.input.len() {
            let rest = self.rest();
            if rest.starts_with("<!--") {
                self.pos += 4;
                let text = self.take_until("-->").to_owned();
                let node = self.doc.create(NodeData::Comment(text.into()));
                let parent = self.current();
                self.doc.append(parent, node);
            } else if rest.starts_with("<![CDATA[") {
                self.pos += 9;
                let text = self.take_until("]]>").to_owned();
                self.append_text(text);
            } else if rest.starts_with("<!DOCTYPE") {
                self.doctype();
            } else if rest.starts_with("<?") {
                self.pos += 2;
                self.take_until("?>");
            } else if rest.starts_with("</") {
                self.end_tag();
            } else if rest.starts_with('<')
                && rest[1..].starts_with(|c: char| c.is_alphabetic() || c == '_')
            {
                self.start_tag();
            } else {
                let first = rest.chars().next().map_or(1, char::len_utf8);
                let end = rest[first..].find('<').map_or(rest.len(), |i| i + first);
                self.pos += end;
                let text = decode(&rest[..end]);
                self.append_text(text);
            }
        }
        self.doc
    }
}

/// Parsea un documento XHTML.
pub fn parse_xml_document(input: &str) -> Document {
    let input = input.replace("\r\n", "\n").replace('\r', "\n");
    let mut doc = Document::new();
    doc.mode = QuirksMode::NoQuirks;
    let parser = Parser {
        input: &input,
        pos: 0,
        doc,
        stack: Vec::new(),
    };
    parser.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xhtml() {
        let doc = parse_xml_document(
            "<?xml version=\"1.0\"?>\n<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Strict//EN\" \"x.dtd\">\n\
             <html xmlns=\"http://www.w3.org/1999/xhtml\"><head><style><![CDATA[ p { color: red } ]]></style></head>\
             <body><div id=\"a\"/><p class='x'>a&amp;b&nbsp;&#x41;&lt;</p>\
             <svg:svg xmlns:svg=\"http://www.w3.org/2000/svg\"><svg:rect width=\"1\"/></svg:svg></body></html>",
        );
        assert_eq!(
            bf_dom::test_serialize(&doc, doc.root()),
            "| <!DOCTYPE html \"-//W3C//DTD XHTML 1.0 Strict//EN\" \"x.dtd\">\n\
             | <html>\n|   xmlns xmlns=\"http://www.w3.org/1999/xhtml\"\n|   <head>\n|     <style>\n|       \" p { color: red } \"\n\
             |   <body>\n|     <div>\n|       id=\"a\"\n|     <p>\n|       class=\"x\"\n|       \"a&b\u{a0}A<\"\n\
             |     <svg svg>\n|       xmlns svg=\"http://www.w3.org/2000/svg\"\n|       <svg rect>\n|         width=\"1\""
        );
    }
}
