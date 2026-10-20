//! Interfaz `Document` (DOM §4.5) y las partes de HTML §3.1 que la extienden (`head`, `body`,
//! `title`), más el "element interface" de HTML para los bindings ([`HtmlElementKind`]).

use std::collections::HashMap;

use bf_infra::{DomString, Namespace};

use crate::names::{
    NameContext, matches_xml_name, valid_attribute_local_name, valid_doctype_name,
    valid_element_local_name, validate_and_extract,
};
use crate::{AttrData, Document, DomException, Element, NodeData, NodeId, QuirksMode};

/// La interfaz HTML de un elemento según su local name (HTML, tabla "Element interfaces" del
/// índice y §3.2.2 *element interface*), para que los bindings sepan qué exponer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HtmlElementKind {
    Anchor,
    Area,
    Audio,
    Base,
    Body,
    BR,
    Button,
    Canvas,
    Data,
    DataList,
    Details,
    Dialog,
    Directory,
    Div,
    DList,
    Embed,
    FieldSet,
    Font,
    Form,
    Frame,
    FrameSet,
    Head,
    Heading,
    HR,
    Html,
    IFrame,
    Image,
    Input,
    Label,
    Legend,
    LI,
    Link,
    Map,
    Marquee,
    Menu,
    Meta,
    Meter,
    Mod,
    Object,
    OList,
    OptGroup,
    Option,
    Output,
    Paragraph,
    Param,
    Picture,
    Pre,
    Progress,
    Quote,
    Script,
    Search,
    Select,
    SelectedContent,
    Slot,
    Source,
    Span,
    Style,
    Table,
    TableCaption,
    TableCell,
    TableCol,
    TableRow,
    TableSection,
    Template,
    TextArea,
    Time,
    Title,
    Track,
    UList,
    Video,
    /// `HTMLElement` a secas (elementos sin interfaz propia y custom elements válidos).
    Element,
    /// `HTMLUnknownElement`.
    Unknown,
}

/// ¿Es un "valid custom element name" (HTML §4.13.2)? Aproximación: empieza con una letra
/// ASCII minúscula, contiene `-`, sin mayúsculas ASCII y no es un nombre reservado.
fn is_valid_custom_element_name(name: &str) -> bool {
    const RESERVED: [&str; 8] = [
        "annotation-xml",
        "color-profile",
        "font-face",
        "font-face-src",
        "font-face-uri",
        "font-face-format",
        "font-face-name",
        "missing-glyph",
    ];
    name.starts_with(|c: char| c.is_ascii_lowercase())
        && name.contains('-')
        && !name.chars().any(|c| c.is_ascii_uppercase())
        && !RESERVED.contains(&name)
}

/// El "element interface" de un local name del namespace HTML.
pub fn html_element_kind(local_name: &str) -> HtmlElementKind {
    use HtmlElementKind as K;
    match local_name {
        "a" => K::Anchor,
        "area" => K::Area,
        "audio" => K::Audio,
        "base" => K::Base,
        "blockquote" | "q" => K::Quote,
        "body" => K::Body,
        "br" => K::BR,
        "button" => K::Button,
        "canvas" => K::Canvas,
        "caption" => K::TableCaption,
        "col" | "colgroup" => K::TableCol,
        "data" => K::Data,
        "datalist" => K::DataList,
        "del" | "ins" => K::Mod,
        "details" => K::Details,
        "dialog" => K::Dialog,
        "dir" => K::Directory,
        "div" => K::Div,
        "dl" => K::DList,
        "embed" => K::Embed,
        "fieldset" => K::FieldSet,
        "font" => K::Font,
        "form" => K::Form,
        "frame" => K::Frame,
        "frameset" => K::FrameSet,
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => K::Heading,
        "head" => K::Head,
        "hr" => K::HR,
        "html" => K::Html,
        "iframe" => K::IFrame,
        "img" => K::Image,
        "input" => K::Input,
        "label" => K::Label,
        "legend" => K::Legend,
        "li" => K::LI,
        "link" => K::Link,
        "map" => K::Map,
        "marquee" => K::Marquee,
        "menu" => K::Menu,
        "meta" => K::Meta,
        "meter" => K::Meter,
        "object" => K::Object,
        "ol" => K::OList,
        "optgroup" => K::OptGroup,
        "option" => K::Option,
        "output" => K::Output,
        "p" => K::Paragraph,
        "param" => K::Param,
        "picture" => K::Picture,
        "pre" | "listing" | "xmp" => K::Pre,
        "progress" => K::Progress,
        "script" => K::Script,
        "select" => K::Select,
        "selectedcontent" => K::SelectedContent,
        "slot" => K::Slot,
        "source" => K::Source,
        "span" => K::Span,
        "style" => K::Style,
        "table" => K::Table,
        "tbody" | "thead" | "tfoot" => K::TableSection,
        "td" | "th" => K::TableCell,
        "template" => K::Template,
        "textarea" => K::TextArea,
        "time" => K::Time,
        "title" => K::Title,
        "tr" => K::TableRow,
        "track" => K::Track,
        "ul" => K::UList,
        "video" => K::Video,
        // Obsoletos sin interfaz (HTML §16.2): HTMLUnknownElement.
        "applet" | "bgsound" | "blink" | "isindex" | "keygen" | "multicol" | "nextid"
        | "spacer" => K::Unknown,
        "abbr" | "acronym" | "address" | "article" | "aside" | "b" | "basefont" | "bdi" | "bdo"
        | "big" | "center" | "cite" | "code" | "dd" | "dfn" | "dt" | "em" | "figcaption"
        | "figure" | "footer" | "header" | "hgroup" | "i" | "kbd" | "main" | "mark" | "nav"
        | "nobr" | "noembed" | "noframes" | "noscript" | "plaintext" | "rb" | "rp" | "rt"
        | "rtc" | "ruby" | "s" | "samp" | "section" | "small" | "strike" | "strong" | "sub"
        | "summary" | "sup" | "tt" | "u" | "var" | "wbr" => K::Element,
        "search" => K::Element,
        name if is_valid_custom_element_name(name) => K::Element,
        _ => K::Unknown,
    }
}

impl Document {
    /// La interfaz HTML del elemento `el` (`None` si no es del namespace HTML).
    pub fn html_kind(&self, el: NodeId) -> Option<HtmlElementKind> {
        let e = self.element(el)?;
        (e.ns == Namespace::Html).then(|| html_element_kind(&e.name))
    }

    // --- Fábricas ---

    /// "Create an element" (sin custom elements).
    pub fn create_element_raw(
        &mut self,
        ns: Namespace,
        prefix: Option<DomString>,
        local: DomString,
    ) -> NodeId {
        let mut e = Element::new(ns, local);
        e.prefix = prefix;
        self.create(NodeData::Element(e))
    }

    /// `Document.createElement(localName)`.
    pub fn create_element(&mut self, local_name: &DomString) -> Result<NodeId, DomException> {
        if !valid_element_local_name(local_name) {
            return Err(DomException::InvalidCharacterError);
        }
        let local = if self.is_html {
            local_name.ascii_lowercase()
        } else {
            local_name.clone()
        };
        let ns = if self.is_html || self.content_type == "application/xhtml+xml" {
            Namespace::Html
        } else {
            Namespace::Null
        };
        Ok(self.create_element_raw(ns, None, local))
    }

    /// `Document.createElementNS(namespace, qualifiedName)`.
    pub fn create_element_ns(
        &mut self,
        namespace: Option<&str>,
        qualified_name: &DomString,
    ) -> Result<NodeId, DomException> {
        let (ns, prefix, local) =
            validate_and_extract(namespace, qualified_name, NameContext::Element)?;
        Ok(self.create_element_raw(ns.unwrap_or(Namespace::Null), prefix, local))
    }

    /// `Document.createDocumentFragment()`.
    pub fn create_document_fragment(&mut self) -> NodeId {
        self.create(NodeData::DocumentFragment)
    }

    /// `Document.createTextNode(data)`.
    pub fn create_text_node(&mut self, data: DomString) -> NodeId {
        self.create(NodeData::Text(data))
    }

    /// `Document.createCDATASection(data)`.
    pub fn create_cdata_section(&mut self, data: DomString) -> Result<NodeId, DomException> {
        if self.is_html {
            return Err(DomException::NotSupportedError);
        }
        if data.contains("]]>") {
            return Err(DomException::InvalidCharacterError);
        }
        Ok(self.create(NodeData::CdataSection(data)))
    }

    /// `Document.createComment(data)`.
    pub fn create_comment(&mut self, data: DomString) -> NodeId {
        self.create(NodeData::Comment(data))
    }

    /// `Document.createProcessingInstruction(target, data)`.
    pub fn create_processing_instruction(
        &mut self,
        target: DomString,
        data: DomString,
    ) -> Result<NodeId, DomException> {
        if !matches_xml_name(&target) || data.contains("?>") {
            return Err(DomException::InvalidCharacterError);
        }
        Ok(self.create(NodeData::ProcessingInstruction { target, data }))
    }

    /// `Document.createAttribute(localName)`.
    pub fn create_attribute(&mut self, local_name: &DomString) -> Result<NodeId, DomException> {
        if !valid_attribute_local_name(local_name) {
            return Err(DomException::InvalidCharacterError);
        }
        let local = if self.is_html {
            local_name.ascii_lowercase()
        } else {
            local_name.clone()
        };
        Ok(self.create(NodeData::Attr(AttrData {
            ns: None,
            prefix: None,
            local_name: local,
            value: DomString::new(),
            owner: None,
        })))
    }

    /// `Document.createAttributeNS(namespace, qualifiedName)`.
    pub fn create_attribute_ns(
        &mut self,
        namespace: Option<&str>,
        qualified_name: &DomString,
    ) -> Result<NodeId, DomException> {
        let (ns, prefix, local) =
            validate_and_extract(namespace, qualified_name, NameContext::Attribute)?;
        Ok(self.create(NodeData::Attr(AttrData {
            ns,
            prefix,
            local_name: local,
            value: DomString::new(),
            owner: None,
        })))
    }

    /// `DOMImplementation.createDocumentType(name, publicId, systemId)`.
    pub fn create_document_type(
        &mut self,
        name: DomString,
        public_id: DomString,
        system_id: DomString,
    ) -> Result<NodeId, DomException> {
        if !valid_doctype_name(&name) {
            return Err(DomException::InvalidCharacterError);
        }
        Ok(self.create(NodeData::Doctype {
            name,
            public_id,
            system_id,
        }))
    }

    /// `Document.importNode(node, subtree)`.
    pub fn import_node(&mut self, node: NodeId, subtree: bool) -> Result<NodeId, DomException> {
        if matches!(self.get(node).data, NodeData::Document) {
            return Err(DomException::NotSupportedError);
        }
        Ok(self.clone_node(node, subtree))
    }

    /// `Document.adoptNode(node)`.
    pub fn adopt_node(&mut self, node: NodeId) -> Result<NodeId, DomException> {
        if matches!(self.get(node).data, NodeData::Document) {
            return Err(DomException::NotSupportedError);
        }
        // Un fragmento que es template contents (con host) no se adopta.
        if self.template_host(node).is_some() {
            return Ok(node);
        }
        self.adopt(node);
        Ok(node)
    }

    // --- Consultas ---

    /// `Document.documentElement`.
    pub fn document_element(&self) -> Option<NodeId> {
        self.children(self.root())
            .find(|&c| self.element(c).is_some())
    }

    /// `Document.doctype`.
    pub fn doctype(&self) -> Option<NodeId> {
        self.children(self.root())
            .find(|&c| matches!(self.get(c).data, NodeData::Doctype { .. }))
    }

    /// "The html element": el document element si es un `html` HTML.
    pub fn html_element(&self) -> Option<NodeId> {
        self.document_element()
            .filter(|&e| self.element(e).is_some_and(|x| x.is_html("html")))
    }

    /// `Document.head`: el primer hijo `head` del html element.
    pub fn head(&self) -> Option<NodeId> {
        let html = self.html_element()?;
        self.children(html)
            .find(|&c| self.element(c).is_some_and(|e| e.is_html("head")))
    }

    /// `Document.body`: el primer hijo `body` o `frameset` del html element.
    pub fn body(&self) -> Option<NodeId> {
        let html = self.html_element()?;
        self.children(html).find(|&c| {
            self.element(c)
                .is_some_and(|e| e.is_html("body") || e.is_html("frameset"))
        })
    }

    /// "The title element": el primer `title` HTML del documento en tree order.
    pub fn title_element(&self) -> Option<NodeId> {
        self.descendants(self.root())
            .into_iter()
            .find(|&n| self.element(n).is_some_and(|e| e.is_html("title")))
    }

    /// Getter de `Document.title` (HTML §3.1.3).
    pub fn title(&self) -> DomString {
        let svg_root = self.document_element().filter(|&e| {
            self.element(e)
                .is_some_and(|x| x.ns == Namespace::Svg && x.name == "svg")
        });
        let value = match svg_root {
            Some(svg) => self
                .children(svg)
                .find(|&c| {
                    self.element(c)
                        .is_some_and(|e| e.ns == Namespace::Svg && e.name == "title")
                })
                .map(|t| self.child_text_content(t)),
            None => self.title_element().map(|t| self.child_text_content(t)),
        }
        .unwrap_or_default();
        // "Strip and collapse ASCII whitespace".
        let collapsed: Vec<&str> = value
            .split(['\t', '\n', '\x0C', '\r', ' '])
            .filter(|s| !s.is_empty())
            .collect();
        collapsed.join(" ").into()
    }

    /// Setter de `Document.title` (HTML §3.1.3).
    pub fn set_title(&mut self, value: DomString) {
        let Some(root) = self.document_element() else {
            return;
        };
        let (ns, name) = {
            let e = self.element(root).expect("elemento");
            (e.ns, e.name.clone())
        };
        let element = if ns == Namespace::Svg && name == "svg" {
            let existing = self.children(root).find(|&c| {
                self.element(c)
                    .is_some_and(|e| e.ns == Namespace::Svg && e.name == "title")
            });
            match existing {
                Some(t) => t,
                None => {
                    let t = self.create_element_raw(Namespace::Svg, None, "title".into());
                    let first = self.get(root).first_child();
                    self.insert(t, root, first, false);
                    t
                }
            }
        } else if ns == Namespace::Html {
            match self.title_element() {
                Some(t) => t,
                None => {
                    let Some(head) = self.head() else {
                        return;
                    };
                    let t = self.create_element_raw(Namespace::Html, None, "title".into());
                    self.insert(t, head, None, false);
                    t
                }
            }
        } else {
            return;
        };
        self.string_replace_all(&value, element);
    }

    /// `Document.getElementById(elementId)` (el primero en tree order). El mapa id → elemento
    /// se reconstruye cuando cambia el contador de mutaciones.
    pub fn get_element_by_id(&self, id: &DomString) -> Option<NodeId> {
        if id.is_empty() {
            return None;
        }
        self.element_by_id_in(self.root(), id)
    }

    /// `getElementById` sobre el subárbol de `root` (`DocumentFragment.getElementById`).
    pub fn element_by_id_in(&self, root: NodeId, id: &DomString) -> Option<NodeId> {
        if root != self.root() {
            return self.descendants(root).into_iter().find(|&n| {
                self.element(n).is_some_and(|e| {
                    e.attrs
                        .iter()
                        .any(|a| a.ns.is_none() && a.name == "id" && a.value == *id)
                })
            });
        }
        let mut cache = self.id_cache.0.lock().unwrap_or_else(|e| e.into_inner());
        let stale = cache.as_ref().is_none_or(|(v, _)| *v != self.version);
        if stale {
            let mut map: HashMap<DomString, NodeId> = HashMap::new();
            for n in self.descendants(self.root()) {
                if let Some(e) = self.element(n)
                    && let Some(a) = e
                        .attrs
                        .iter()
                        .find(|a| a.ns.is_none() && a.name == "id" && !a.value.is_empty())
                {
                    map.entry(a.value.clone()).or_insert(n);
                }
            }
            *cache = Some((self.version, map));
        }
        cache.as_ref().and_then(|(_, m)| m.get(id).copied())
    }

    /// `Document.URL` / `documentURI`.
    pub fn document_url(&self) -> DomString {
        self.url.href().into()
    }

    /// `Document.compatMode`.
    pub fn compat_mode(&self) -> &'static str {
        if self.mode == QuirksMode::Quirks {
            "BackCompat"
        } else {
            "CSS1Compat"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_kinds() {
        assert_eq!(html_element_kind("a"), HtmlElementKind::Anchor);
        assert_eq!(html_element_kind("thead"), HtmlElementKind::TableSection);
        assert_eq!(html_element_kind("my-widget"), HtmlElementKind::Element);
        assert_eq!(html_element_kind("blink"), HtmlElementKind::Unknown);
        assert_eq!(html_element_kind("foo"), HtmlElementKind::Unknown);
        assert_eq!(html_element_kind("section"), HtmlElementKind::Element);
    }
}
