//! `XMLSerializer` y la serialización XML (DOM Parsing and Serialization §3.2 "XML
//! serialization"; §3.3 la interfaz `XMLSerializer`). **E1.**
//!
//! [`xml_serialize`] es "produce an XML serialization" de un nodo; `serializeToString` la
//! llama sin *require well-formed* y `innerHTML`/`outerHTML` de los documentos XML con él
//! (§2 "fragment serializing algorithm"). Los errores son `InvalidStateError`.

use std::collections::HashMap;

use bf_dom::{Document, DomString, Namespace, NodeData, NodeId};
use bf_js::host::HostClass;
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use crate::dom::{from_dom_string, unwrap_node};
use crate::with_doc;

const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";
const HTML_NS: &str = "http://www.w3.org/1999/xhtml";

/// Los elementos vacíos de §3.2.1.1 paso 14.
const VOID: &[&str] = &[
    "area", "base", "basefont", "bgsound", "br", "col", "embed", "frame", "hr", "img", "input",
    "keygen", "link", "menuitem", "meta", "param", "source", "track", "wbr",
];

/// Un namespace del algoritmo: `None` es el valor nulo.
type Ns = Option<String>;

/// El error de serialización (se lanza como `InvalidStateError`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotWellFormed(pub &'static str);

/// El *namespace prefix map* (§3.2.1.1): namespace → lista de prefijos, en orden.
#[derive(Debug, Clone, Default)]
struct PrefixMap(HashMap<Ns, Vec<String>>);

impl PrefixMap {
    /// "Retrieving a preferred prefix string": el prefijo preferido si está en la lista del
    /// namespace; si no, el último de la lista.
    fn preferred(&self, preferred: Option<&str>, ns: &Ns) -> Option<String> {
        let list = self.0.get(ns)?;
        if let Some(p) = preferred
            && list.iter().any(|x| x == p)
        {
            return Some(p.to_owned());
        }
        list.last().cloned()
    }

    /// "Found": ¿`prefix` está en la lista de `ns`?
    fn found(&self, prefix: &str, ns: &Ns) -> bool {
        self.0
            .get(ns)
            .is_some_and(|l| l.iter().any(|x| x == prefix))
    }

    /// "Add" `prefix` a la lista de `ns`.
    fn add(&mut self, prefix: &str, ns: &Ns) {
        self.0
            .entry(ns.clone())
            .or_default()
            .push(prefix.to_owned());
    }
}

struct Serializer<'a> {
    doc: &'a Document,
    well_formed: bool,
    prefix_index: u32,
    out: DomString,
}

fn element_ns(ns: &Namespace) -> Ns {
    match ns {
        Namespace::Null => None,
        other => Some(other.uri().to_owned()),
    }
}

fn attr_ns(ns: &Option<Namespace>) -> Ns {
    ns.as_ref().and_then(element_ns)
}

/// "Produce an XML serialization" de `node` (§3.2): el nodo con su contenido.
pub fn xml_serialize(
    doc: &Document,
    node: NodeId,
    well_formed: bool,
) -> Result<DomString, NotWellFormed> {
    let mut s = Serializer::new(doc, well_formed);
    s.node(node, &None, &mut PrefixMap::initial())?;
    Ok(s.out)
}

/// La serialización XML de los hijos de `node` (el `innerHTML` de un documento XML).
pub fn xml_serialize_children(
    doc: &Document,
    node: NodeId,
    well_formed: bool,
) -> Result<DomString, NotWellFormed> {
    let mut s = Serializer::new(doc, well_formed);
    let mut map = PrefixMap::initial();
    let parent = match doc.element(node) {
        Some(e) if e.ns == Namespace::Html && e.name == "template" => {
            e.template_contents.unwrap_or(node)
        }
        _ => node,
    };
    for c in doc.child_nodes(parent) {
        s.node(c, &None, &mut map)?;
    }
    Ok(s.out)
}

impl PrefixMap {
    /// El mapa inicial: `xml` para el namespace XML.
    fn initial() -> PrefixMap {
        let mut m = PrefixMap::default();
        m.add("xml", &Some(XML_NS.to_owned()));
        m
    }
}

impl<'a> Serializer<'a> {
    fn new(doc: &'a Document, well_formed: bool) -> Self {
        Serializer {
            doc,
            well_formed,
            prefix_index: 1,
            out: DomString::new(),
        }
    }

    fn push(&mut self, s: &str) {
        self.out.push_str(s);
    }

    fn fail<T>(&self, why: &'static str) -> Result<T, NotWellFormed> {
        Err(NotWellFormed(why))
    }

    /// "XML serialization algorithm" (§3.2.1).
    fn node(&mut self, node: NodeId, ns: &Ns, map: &mut PrefixMap) -> Result<(), NotWellFormed> {
        let doc = self.doc;
        match &doc.get(node).data {
            NodeData::Element(_) => self.element(node, ns, map),
            NodeData::Document => {
                // §3.2.1.2.
                if self.well_formed && doc.document_element().is_none() {
                    return self.fail("the document has no document element");
                }
                for c in doc.child_nodes(node) {
                    self.node(c, ns, map)?;
                }
                Ok(())
            }
            NodeData::DocumentFragment => {
                for c in doc.child_nodes(node) {
                    self.node(c, ns, map)?;
                }
                Ok(())
            }
            NodeData::Comment(data) => {
                // §3.2.1.3.
                if self.well_formed
                    && (!is_xml_chars(data) || has(data, "--") || ends_with_dash(data))
                {
                    return self.fail("the comment is not well-formed");
                }
                self.push("<!--");
                self.out.push_dom(data);
                self.push("-->");
                Ok(())
            }
            NodeData::Text(data) => {
                // §3.2.1.4.
                if self.well_formed && !is_xml_chars(data) {
                    return self.fail("the text contains characters that are not XML Char");
                }
                escape(&mut self.out, data, false);
                Ok(())
            }
            NodeData::CdataSection(data) => {
                if self.well_formed && has(data, "]]>") {
                    return self.fail("the CDATA section contains ]]>");
                }
                self.push("<![CDATA[");
                self.out.push_dom(data);
                self.push("]]>");
                Ok(())
            }
            NodeData::Doctype {
                name,
                public_id,
                system_id,
            } => {
                // §3.2.1.6.
                if self.well_formed && !public_id.code_points().all(is_pubid_char) {
                    return self.fail("the public id is not well-formed");
                }
                if self.well_formed && has(system_id, "\"") && has(system_id, "'") {
                    return self.fail("the system id contains both quote characters");
                }
                self.push("<!DOCTYPE ");
                self.out.push_dom(name);
                if !public_id.is_empty() {
                    self.push(" PUBLIC \"");
                    self.out.push_dom(public_id);
                    self.push("\"");
                }
                if !system_id.is_empty() && public_id.is_empty() {
                    self.push(" SYSTEM");
                }
                if !system_id.is_empty() {
                    self.push(" \"");
                    self.out.push_dom(system_id);
                    self.push("\"");
                }
                self.push(">");
                Ok(())
            }
            NodeData::ProcessingInstruction { target, data } => {
                // §3.2.1.7.
                if self.well_formed
                    && (has(target, ":") || target.to_string_lossy().eq_ignore_ascii_case("xml"))
                {
                    return self.fail("the processing instruction target is not well-formed");
                }
                if self.well_formed && (!is_xml_chars(data) || has(data, "?>")) {
                    return self.fail("the processing instruction data is not well-formed");
                }
                self.push("<?");
                self.out.push_dom(target);
                self.push(" ");
                self.out.push_dom(data);
                self.push("?>");
                Ok(())
            }
            // §3.2.1: un `Attr` se serializa como el string vacío.
            NodeData::Attr(_) => Ok(()),
        }
    }

    /// "XML serializing an Element node" (§3.2.1.1).
    fn element(
        &mut self,
        node: NodeId,
        namespace: &Ns,
        parent_map: &PrefixMap,
    ) -> Result<(), NotWellFormed> {
        let doc = self.doc;
        let e = doc.element(node).expect("element");
        let local = e.name.to_string_lossy().into_owned();
        // 1.
        if self.well_formed && (local.contains(':') || !is_xml_name(&local)) {
            return self.fail("the element's local name is not well-formed");
        }
        // 2-7.
        self.push("<");
        let mut ignore_ns_definition = false;
        let mut map = parent_map.clone();
        let mut local_prefixes: HashMap<String, Ns> = HashMap::new();
        // 8.
        let local_default = record_namespace_information(e, &mut map, &mut local_prefixes);
        // 9-10.
        let mut inherited = namespace.clone();
        let ns = element_ns(&e.ns);
        let qualified;
        // 11.
        if inherited == ns {
            if local_default.is_some() {
                ignore_ns_definition = true;
            }
            qualified = if ns.as_deref() == Some(XML_NS) {
                format!("xml:{local}")
            } else {
                local.clone()
            };
            self.push(&qualified);
        } else {
            // 12.
            let mut prefix = e.prefix.as_ref().map(|p| p.to_string_lossy().into_owned());
            let mut candidate = map.preferred(prefix.as_deref(), &ns);
            if prefix.as_deref() == Some("xmlns") {
                if self.well_formed {
                    return self.fail("an element cannot have the xmlns prefix");
                }
                candidate = prefix.clone();
            }
            if let Some(c) = candidate {
                // 12.4.
                qualified = format!("{c}:{local}");
                if let Some(d) = &local_default
                    && d.as_deref() != Some(XML_NS)
                {
                    inherited = d.clone().filter(|s| !s.is_empty());
                }
                self.push(&qualified);
            } else if let Some(p) = prefix.take() {
                // 12.5.
                let p = if local_prefixes.contains_key(&p) {
                    self.generate_prefix(&mut map, &ns)
                } else {
                    p
                };
                map.add(&p, &ns);
                qualified = format!("{p}:{local}");
                self.push(&qualified);
                self.push(" xmlns:");
                self.push(&p);
                self.push("=\"");
                self.attribute_value(ns.as_deref())?;
                self.push("\"");
                if let Some(d) = &local_default {
                    inherited = d.clone().filter(|s| !s.is_empty());
                }
            } else if local_default.is_none() || local_default.as_ref() != Some(&ns) {
                // 12.6.
                ignore_ns_definition = true;
                qualified = local.clone();
                inherited = ns.clone();
                self.push(&qualified);
                self.push(" xmlns=\"");
                self.attribute_value(ns.as_deref())?;
                self.push("\"");
            } else {
                // 12.7.
                qualified = local.clone();
                inherited = ns.clone();
                self.push(&qualified);
            }
        }
        // 13.
        self.attributes(e, &mut map, &local_prefixes, ignore_ns_definition)?;
        // 14-16.
        let children = doc.child_nodes(node);
        let is_html = ns.as_deref() == Some(HTML_NS);
        let mut skip_end_tag = false;
        if is_html && children.is_empty() && VOID.contains(&local.as_str()) {
            self.push(" /");
            skip_end_tag = true;
        }
        if !is_html && children.is_empty() {
            self.push("/");
            skip_end_tag = true;
        }
        self.push(">");
        // 17.
        if skip_end_tag {
            return Ok(());
        }
        // 18.
        let children = match e.template_contents {
            Some(t) if is_html && local == "template" => doc.child_nodes(t),
            _ => children,
        };
        for c in children {
            self.node(c, &inherited, &mut map)?;
        }
        // 19.
        self.push("</");
        self.push(&qualified);
        self.push(">");
        Ok(())
    }

    /// "Generating a prefix".
    fn generate_prefix(&mut self, map: &mut PrefixMap, ns: &Ns) -> String {
        let p = format!("ns{}", self.prefix_index);
        self.prefix_index += 1;
        map.add(&p, ns);
        p
    }

    /// "XML serialization of the attributes" (§3.2.1.1.1).
    fn attributes(
        &mut self,
        e: &bf_dom::Element,
        map: &mut PrefixMap,
        local_prefixes: &HashMap<String, Ns>,
        ignore_ns_definition: bool,
    ) -> Result<(), NotWellFormed> {
        let mut seen: Vec<(Ns, String)> = Vec::new();
        for attr in &e.attrs {
            let attr_ns = attr_ns(&attr.ns);
            let local = attr.name.to_string_lossy().into_owned();
            let attr_prefix = attr
                .prefix
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned());
            let value = attr.value.to_string_lossy();
            // 3.1-3.2.
            let key = (attr_ns.clone(), local.clone());
            if self.well_formed && seen.contains(&key) {
                return self.fail("duplicate attribute");
            }
            seen.push(key);
            // Un `xmlns` sin namespace (de `setAttribute`) no es una declaración: con *require
            // well-formed* es un error (paso 3.8) y, sin él, se omite como hacen los
            // navegadores (WPT `XMLSerializer-serializeToString.html`, "Drop inconsistent
            // xmlns").
            if attr_ns.is_none() && local == "xmlns" {
                if self.well_formed {
                    return self.fail("an attribute named xmlns must be in the XMLNS namespace");
                }
                continue;
            }
            // 3.3-3.5.
            let mut candidate = None;
            if let Some(ans) = &attr_ns {
                candidate = map.preferred(attr_prefix.as_deref(), &attr_ns);
                if ans == XMLNS_NS {
                    // 3.5.2.1.
                    let redundant = value == XML_NS
                        || (attr_prefix.is_none() && ignore_ns_definition)
                        || (attr_prefix.is_some()
                            && local_prefixes.get(&local).is_none_or(|v| {
                                v.as_deref() != Some(value.as_ref())
                                    && !(v.is_none() && value.is_empty())
                            }));
                    if redundant {
                        continue;
                    }
                    if self.well_formed && value == XMLNS_NS {
                        return self.fail("the xmlns namespace cannot be declared");
                    }
                    if self.well_formed && attr_prefix.is_some() && value.is_empty() {
                        return self.fail("a namespace prefix cannot be undeclared");
                    }
                    if attr_prefix.as_deref() == Some("xmlns") {
                        candidate = Some("xmlns".to_owned());
                    }
                } else if candidate.is_none() {
                    // 3.5.3.
                    let c = self.generate_prefix(map, &attr_ns);
                    self.push(" xmlns:");
                    self.push(&c);
                    self.push("=\"");
                    self.attribute_value(Some(ans))?;
                    self.push("\"");
                    candidate = Some(c);
                }
            }
            // 3.6-3.9.
            self.push(" ");
            if let Some(c) = &candidate {
                self.push(c);
                self.push(":");
            }
            if self.well_formed
                && (local.contains(':')
                    || !is_xml_name(&local)
                    || (local == "xmlns" && attr_ns.is_none()))
            {
                return self.fail("the attribute's local name is not well-formed");
            }
            self.push(&local);
            self.push("=\"");
            let v = attr.value.clone();
            if self.well_formed && !is_xml_chars(&v) {
                return self.fail("the attribute value contains characters that are not XML Char");
            }
            escape(&mut self.out, &v, true);
            self.push("\"");
        }
        Ok(())
    }

    /// "Serializing an attribute value" de un namespace (`None` es el string vacío).
    fn attribute_value(&mut self, v: Option<&str>) -> Result<(), NotWellFormed> {
        let v = DomString::from(v.unwrap_or(""));
        if self.well_formed && !is_xml_chars(&v) {
            return self.fail("the attribute value contains characters that are not XML Char");
        }
        escape(&mut self.out, &v, true);
        Ok(())
    }
}

/// "Recording the namespace information" (§3.2.1.1.2): llena `map` y `local_prefixes` con
/// las declaraciones de `e` y devuelve el valor de su `xmlns` (si lo tiene).
fn record_namespace_information(
    e: &bf_dom::Element,
    map: &mut PrefixMap,
    local_prefixes: &mut HashMap<String, Ns>,
) -> Option<Ns> {
    let mut default_ns = None;
    for attr in &e.attrs {
        if attr_ns(&attr.ns).as_deref() != Some(XMLNS_NS) {
            continue;
        }
        let value = attr.value.to_string_lossy().into_owned();
        match &attr.prefix {
            None => default_ns = Some(Some(value)),
            Some(_) => {
                let prefix = attr.name.to_string_lossy().into_owned();
                if value == XML_NS {
                    continue;
                }
                let ns = Some(value).filter(|s| !s.is_empty());
                if map.found(&prefix, &ns) {
                    continue;
                }
                map.add(&prefix, &ns);
                local_prefixes.insert(prefix, ns);
            }
        }
    }
    default_ns
}

fn has(s: &DomString, needle: &str) -> bool {
    s.to_string_lossy().contains(needle)
}

fn ends_with_dash(s: &DomString) -> bool {
    s.code_points().last() == Some('-' as u32)
}

/// ¿Todos los code points casan con la producción `Char` de XML 1.0?
fn is_xml_chars(s: &DomString) -> bool {
    s.code_points().all(
        |c| matches!(c, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF),
    )
}

/// `PubidChar` de XML 1.0.
fn is_pubid_char(c: u32) -> bool {
    matches!(c, 0x20 | 0xD | 0xA)
        || char::from_u32(c)
            .is_some_and(|c| c.is_ascii_alphanumeric() || "-'()+,./:=?;!*#@$_%".contains(c))
}

fn is_name_start_char(c: char) -> bool {
    matches!(c, ':' | 'A'..='Z' | '_' | 'a'..='z'
        | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}'
        | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}' | '\u{10000}'..='\u{EFFFF}')
}

fn is_name_char(c: char) -> bool {
    is_name_start_char(c)
        || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

/// La producción `Name` de XML 1.0.
fn is_xml_name(s: &str) -> bool {
    let mut it = s.chars();
    it.next().is_some_and(is_name_start_char) && it.all(is_name_char)
}

/// El escape de los datos (`&`, `<`, `>`) o de un valor de atributo (además `"` y, para
/// conservar el valor al volver a parsear, tabulador y saltos de línea).
fn escape(out: &mut DomString, s: &DomString, attribute: bool) {
    for cp in s.code_points() {
        match cp {
            0x26 => out.push_str("&amp;"),
            0x3C => out.push_str("&lt;"),
            0x3E => out.push_str("&gt;"),
            0x22 if attribute => out.push_str("&quot;"),
            0x9 if attribute => out.push_str("&#x9;"),
            0xA if attribute => out.push_str("&#xA;"),
            0xD if attribute => out.push_str("&#xD;"),
            _ => out.push_code_point(cp),
        }
    }
}

trait PushDom {
    fn push_dom(&mut self, s: &DomString);
}

impl PushDom for DomString {
    fn push_dom(&mut self, s: &DomString) {
        for cp in s.code_points() {
            self.push_code_point(cp);
        }
    }
}

// --- La interfaz `XMLSerializer` (§3.3) ---

/// El payload de un `XMLSerializer` (sin estado).
#[derive(Debug, Clone, Copy)]
pub struct XmlSerializerData;

/// La clase registrada (una vez por agente).
#[derive(Debug, Clone, Copy)]
struct XmlSerializerClass(bf_js::host::HostClassId);

/// Registra `XMLSerializer`.
pub fn register(agent: &mut Agent) {
    if agent.embedder_data::<XmlSerializerClass>().is_some() {
        return;
    }
    let class = agent.register_host_class(xml_serializer_class());
    agent.set_embedder_data(XmlSerializerClass(class));
}

pub fn xml_serializer_class() -> HostClass {
    HostClass::new("XMLSerializer").constructor(ctor, 0).method(
        "serializeToString",
        1,
        serialize_to_string,
    )
}

fn ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let class = agent
        .embedder_data::<XmlSerializerClass>()
        .expect("XMLSerializer")
        .0;
    let nt = a.new_target.expect("new.target");
    let o = agent.host_create_from_new_target(nt, class, None, Box::new(XmlSerializerData))?;
    Ok(Value::Object(o))
}

/// `serializeToString(root)`: "produce an XML serialization" de `root` sin *require
/// well-formed*.
fn serialize_to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    if agent.host_unwrap::<XmlSerializerData>(&a.this).is_none() {
        return agent.type_error("Illegal invocation");
    }
    let Some(n) = unwrap_node(agent, &a.arg(0)) else {
        return agent.type_error(
            "Failed to execute 'serializeToString' on 'XMLSerializer': parameter 1 is not of \
             type 'Node'.",
        );
    };
    let r = with_doc(agent, n.doc, |d| xml_serialize(d, n.node, false));
    match r {
        Ok(s) => Ok(from_dom_string(&s)),
        Err(NotWellFormed(why)) => crate::net::dom_error(agent, "InvalidStateError", why),
    }
}
