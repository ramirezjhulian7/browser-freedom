//! Parser XML: XML 1.0 (quinta edición) con Namespaces in XML 1.0, para documentos XHTML y XML
//! (`.xht`, `application/xhtml+xml`, `DOMParser` con `text/xml`).
//!
//! Comprueba la buena formación (XML §2.1 y las WFC) y la de namespaces (Namespaces in XML
//! §3 a §6), y se detiene en el primer error fatal: [`parse_xml`] lo devuelve,
//! [`parse_xml_partial`] además conserva el árbol construido hasta ahí (lo que muestra una
//! navegación) y [`parsererror_document`] arma el documento de error de `DOMParser` (HTML
//! §8.5.1, el elemento `parsererror`).
//!
//! - Prólogo: XML declaration (solo al principio), comentarios, processing instructions y un
//!   DOCTYPE con su identificador externo y su subset interno. Del subset interno se usan las
//!   declaraciones de entidades generales internas; `ELEMENT`, `ATTLIST` y `NOTATION` se
//!   reconocen y se saltan, y las entidades externas no se cargan (HTML §14.2).
//! - Para los DOCTYPE con los public ids de HTML §14.2 (XHTML y MathML) las referencias
//!   nombradas de HTML están definidas, como si se hubiera leído su DTD.
//! - Referencias: las cinco predefinidas, las numéricas y las entidades internas (su texto de
//!   reemplazo se parsea como contenido y debe estar balanceado). Una entidad no declarada es
//!   error salvo que haya subset externo o referencias a parameter entities (WFC: Entity
//!   Declared).
//! - Atributos: normalización de XML §3.3.3. `xml:space` y `xml:lang` quedan como atributos
//!   del namespace XML (el DOM no cambia el whitespace por ellos).
//! - Fin de línea: CRLF y CR pasan a LF antes de parsear (XML §2.11).

use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use bf_dom::{Attribute, Document, Element, NodeData, NodeId, QuirksMode};
use bf_infra::Namespace;

use crate::tokenizer::named_entities::NAMED_ENTITIES;

/// Namespace del elemento `parsererror` de `DOMParser` (HTML §8.5.1).
pub const PARSERERROR_NAMESPACE: &str = "http://www.mozilla.org/newlayout/xml/parsererror.xml";

const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";

/// Public ids cuyas DTD definen las referencias nombradas de HTML (HTML §14.2).
const HTML_ENTITY_PUBLIC_IDS: &[&str] = &[
    "-//W3C//DTD XHTML 1.0 Transitional//EN",
    "-//W3C//DTD XHTML 1.1//EN",
    "-//W3C//DTD XHTML 1.0 Strict//EN",
    "-//W3C//DTD XHTML 1.0 Frameset//EN",
    "-//W3C//DTD XHTML Basic 1.0//EN",
    "-//W3C//DTD XHTML 1.1 plus MathML 2.0//EN",
    "-//W3C//DTD XHTML 1.1 plus MathML 2.0 plus SVG 1.1//EN",
    "-//W3C//DTD MathML 2.0//EN",
    "-//WAPFORUM//DTD XHTML Mobile 1.0//EN",
];

/// Límite de texto expandido por entidades (contra la expansión exponencial).
const EXPANSION_LIMIT: usize = 1 << 22;
/// Profundidad máxima de entidades anidadas.
const ENTITY_DEPTH_LIMIT: usize = 64;

/// Un error fatal de buena formación, con su posición en la entrada (1-based; la columna en
/// code points).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlError {
    pub message: String,
    pub line: usize,
    pub column: usize,
}

impl fmt::Display for XmlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} (line {}, column {})",
            self.message, self.line, self.column
        )
    }
}

impl std::error::Error for XmlError {}

type R<T> = Result<T, XmlError>;

/// `Char` (XML §2.2).
fn is_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..)
}

/// `NameStartChar` (XML §2.3).
fn is_name_start(c: char) -> bool {
    matches!(c,
        ':' | 'A'..='Z' | '_' | 'a'..='z' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}'
        | '\u{F8}'..='\u{2FF}' | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}'
        | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}'
        | '\u{3001}'..='\u{D7FF}' | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}'
        | '\u{10000}'..='\u{EFFFF}')
}

/// `NameChar` (XML §2.3).
fn is_name_char(c: char) -> bool {
    is_name_start(c)
        || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

/// `S` (XML §2.3).
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(is_name_start) && chars.all(is_name_char)
}

/// `NCName` (Namespaces in XML §3).
fn is_ncname(s: &str) -> bool {
    is_name(s) && !s.contains(':')
}

/// `PubidChar` (XML §2.3).
fn is_pubid_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || " \n-'()+,./:=?;!*#@$_%".contains(c)
}

fn predefined(name: &str) -> Option<&'static str> {
    Some(match name {
        "lt" => "<",
        "gt" => ">",
        "amp" => "&",
        "quot" => "\"",
        "apos" => "'",
        _ => return None,
    })
}

fn html_entity(name: &str) -> Option<&'static str> {
    let key = format!("{name};");
    NAMED_ENTITIES
        .binary_search_by(|(n, _)| (*n).cmp(key.as_str()))
        .ok()
        .map(|i| NAMED_ENTITIES[i].1)
}

/// El carácter de una referencia numérica sin `&#` ni `;` (`x41` o `65`), si es un `Char`
/// (WFC: Legal Character).
fn char_ref(body: &str) -> Option<char> {
    let (digits, radix) = match body.strip_prefix('x') {
        Some(hex) => (hex, 16),
        None => (body, 10),
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    u32::from_str_radix(digits, radix)
        .ok()
        .and_then(char::from_u32)
        .filter(|&c| is_char(c))
}

#[derive(Clone)]
enum Entity {
    /// Texto de reemplazo (con las referencias numéricas ya resueltas, XML §4.5).
    Internal(Rc<str>),
    /// Entidad externa analizada: no se carga.
    External,
    /// Entidad no analizada (`NDATA`): no se puede referenciar en el contenido.
    Unparsed,
}

enum Reference {
    Char(char),
    Entity(String),
}

/// Una entrada que se está leyendo: el documento o el texto de reemplazo de una entidad.
struct Frame {
    text: Rc<str>,
    pos: usize,
    /// La entidad (para detectar recursión); `None` para el documento.
    entity: Option<String>,
    /// Elementos abiertos al empezar la entidad: debe terminar con los mismos.
    depth: usize,
}

/// Un elemento abierto con sus declaraciones de namespace (`""` es el default namespace).
struct Open {
    node: NodeId,
    qname: String,
    scope: Vec<(String, String)>,
}

struct Parser {
    frames: Vec<Frame>,
    /// El primer carácter no permitido de la entrada (la entrada se corta ahí y el error sale
    /// cuando el parser llega a él).
    illegal: Option<XmlError>,
    doc: Document,
    stack: Vec<Open>,
    entities: HashMap<String, Entity>,
    /// DOCTYPE con un public id de HTML §14.2.
    html_entities: bool,
    /// Subset externo o referencias a parameter entities: una entidad no declarada no es
    /// error de buena formación.
    undeclared_ok: bool,
    seen_doctype: bool,
    expanded: usize,
    /// E10: los `link` XHTML insertados, en orden ([`parse_xml_partial_with`]).
    links: Vec<NodeId>,
}

impl Parser {
    fn new(input: &str) -> Parser {
        let input = input.strip_prefix('\u{FEFF}').unwrap_or(input);
        let input = input.replace("\r\n", "\n").replace('\r', "\n");
        let mut doc = Document::new_xml();
        doc.mode = QuirksMode::NoQuirks;
        let mut parser = Parser {
            frames: Vec::new(),
            illegal: None,
            doc,
            stack: Vec::new(),
            entities: HashMap::new(),
            html_entities: false,
            undeclared_ok: false,
            seen_doctype: false,
            expanded: 0,
            links: Vec::new(),
        };
        let text: Rc<str> = match input.char_indices().find(|&(_, c)| !is_char(c)) {
            Some((i, c)) => {
                let (line, column) = position(&input, i);
                parser.illegal = Some(XmlError {
                    message: format!("illegal character U+{:04X}", u32::from(c)),
                    line,
                    column,
                });
                input[..i].into()
            }
            None => input.into(),
        };
        parser.frames.push(Frame {
            text,
            pos: 0,
            entity: None,
            depth: 0,
        });
        parser
    }

    // --- Lectura ---

    fn rest(&self) -> &str {
        let f = self.frames.last().expect("siempre hay un frame");
        &f.text[f.pos..]
    }

    fn at_end(&self) -> bool {
        self.rest().is_empty()
    }

    fn advance(&mut self, n: usize) {
        self.frames.last_mut().expect("siempre hay un frame").pos += n;
    }

    fn eat(&mut self, s: &str) -> bool {
        let found = self.rest().starts_with(s);
        if found {
            self.advance(s.len());
        }
        found
    }

    fn skip_space(&mut self) -> bool {
        let r = self.rest();
        let n = r.len() - r.trim_start_matches(is_space).len();
        self.advance(n);
        n > 0
    }

    fn require_space(&mut self, context: &str) -> R<()> {
        if self.skip_space() {
            Ok(())
        } else {
            self.fail(format!("expected whitespace {context}"))
        }
    }

    fn error(&self, message: impl Into<String>) -> XmlError {
        let main = &self.frames[0];
        if self.frames.len() == 1
            && main.pos >= main.text.len()
            && let Some(e) = &self.illegal
        {
            return e.clone();
        }
        let (line, column) = position(&main.text, main.pos);
        XmlError {
            message: message.into(),
            line,
            column,
        }
    }

    fn fail<T>(&self, message: impl Into<String>) -> R<T> {
        Err(self.error(message))
    }

    fn name(&mut self) -> R<String> {
        let r = self.rest();
        if !r.starts_with(is_name_start) {
            return self.fail("expected a name");
        }
        let end = r
            .char_indices()
            .find(|&(_, c)| !is_name_char(c))
            .map_or(r.len(), |(i, _)| i);
        let name = r[..end].to_owned();
        self.advance(end);
        Ok(name)
    }

    /// Un literal entre comillas simples o dobles (sin interpretar).
    fn quoted(&mut self) -> R<String> {
        let r = self.rest();
        let Some(q) = r.chars().next().filter(|&c| c == '"' || c == '\'') else {
            return self.fail("expected a quoted literal");
        };
        let Some(end) = r[1..].find(q) else {
            return self.fail("unterminated literal");
        };
        let value = r[1..1 + end].to_owned();
        self.advance(end + 2);
        Ok(value)
    }

    /// `&…;` en el contenido (en `&`).
    fn reference(&mut self) -> R<Reference> {
        let r = &self.rest()[1..];
        let Some(end) = r.find(';') else {
            return self.fail("unterminated reference");
        };
        let body = &r[..end];
        let reference = if let Some(num) = body.strip_prefix('#') {
            match char_ref(num) {
                Some(c) => Reference::Char(c),
                None => return self.fail(format!("invalid character reference &{body};")),
            }
        } else if is_name(body) {
            Reference::Entity(body.to_owned())
        } else {
            return self.fail("invalid entity reference");
        };
        self.advance(end + 2);
        Ok(reference)
    }

    // --- Árbol ---

    fn current(&self) -> NodeId {
        self.stack.last().map_or(self.doc.root(), |o| o.node)
    }

    fn append(&mut self, data: NodeData) {
        let node = self.doc.create(data);
        let parent = self.current();
        self.doc.append(parent, node);
    }

    fn append_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let parent = self.current();
        if let Some(last) = self.doc.get(parent).last_child()
            && let NodeData::Text(t) = &mut self.doc.get_mut(last).data
        {
            t.push_str(text);
            return;
        }
        let node = self.doc.create(NodeData::Text(text.into()));
        self.doc.append(parent, node);
    }

    /// El namespace de `prefix` (`""` = default) con las declaraciones de `scope` (las del
    /// elemento que se está abriendo) y las de los elementos abiertos.
    fn resolve<'s>(&'s self, scope: &'s [(String, String)], prefix: &str) -> Option<&'s str> {
        if prefix == "xml" {
            return Some(XML_NAMESPACE);
        }
        scope
            .iter()
            .chain(self.stack.iter().rev().flat_map(|o| o.scope.iter()))
            .find(|(p, _)| p == prefix)
            .map(|(_, uri)| uri.as_str())
            .filter(|uri| !uri.is_empty())
    }

    fn split_qname<'q>(&self, qname: &'q str) -> R<(Option<&'q str>, &'q str)> {
        match qname.split_once(':') {
            None => Ok((None, qname)),
            Some((p, l)) if is_ncname(p) && is_ncname(l) => Ok((Some(p), l)),
            Some(_) => self.fail(format!("invalid qualified name {qname:?}")),
        }
    }

    // --- Prólogo ---

    fn run(&mut self) -> R<()> {
        if self.rest().starts_with("<?xml") && self.rest()[5..].starts_with(is_space) {
            self.xml_decl()?;
        }
        self.misc(true)?;
        if self.at_end() {
            return self.fail("no root element");
        }
        if self.rest().starts_with("<!DOCTYPE") {
            return self.fail("misplaced DOCTYPE");
        }
        if !self.rest().starts_with('<') {
            return self.fail("text is not allowed before the root element");
        }
        if !self.start_tag()? {
            self.content()?;
        }
        self.misc(false)?;
        if !self.at_end() {
            return self.fail("junk after the root element");
        }
        match &self.illegal {
            Some(e) => Err(e.clone()),
            None => Ok(()),
        }
    }

    /// `XMLDecl` (XML §2.8).
    fn xml_decl(&mut self) -> R<()> {
        self.advance(5);
        let mut seen = 0;
        loop {
            let space = self.skip_space();
            if self.eat("?>") {
                break;
            }
            if self.at_end() {
                return self.fail("unterminated XML declaration");
            }
            if !space {
                return self.fail("expected whitespace in the XML declaration");
            }
            let name = self.name()?;
            self.skip_space();
            if !self.eat("=") {
                return self.fail("expected '=' in the XML declaration");
            }
            self.skip_space();
            let value = self.quoted()?;
            let (order, ok) = match name.as_str() {
                "version" => (
                    1,
                    value
                        .strip_prefix("1.")
                        .is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit())),
                ),
                "encoding" => (
                    2,
                    value.starts_with(|c: char| c.is_ascii_alphabetic())
                        && value
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')),
                ),
                "standalone" => (3, value == "yes" || value == "no"),
                _ => (0, false),
            };
            if order <= seen || (seen == 0 && order != 1) || !ok {
                return self.fail(format!("invalid XML declaration ({name}=\"{value}\")"));
            }
            seen = order;
        }
        if seen == 0 {
            return self.fail("XML declaration without version");
        }
        Ok(())
    }

    /// `Misc*` antes (con el DOCTYPE) o después del elemento raíz.
    fn misc(&mut self, prolog: bool) -> R<()> {
        loop {
            self.skip_space();
            let r = self.rest();
            if r.starts_with("<!--") {
                let text = self.comment()?;
                self.append(NodeData::Comment(text.into()));
            } else if r.starts_with("<?") {
                self.pi(true)?;
            } else if prolog && !self.seen_doctype && r.starts_with("<!DOCTYPE") {
                self.doctype()?;
            } else {
                return Ok(());
            }
        }
    }

    /// `Comment` (XML §2.5), en `<!--`.
    fn comment(&mut self) -> R<String> {
        self.advance(4);
        let r = self.rest();
        let Some(i) = r.find("--") else {
            return self.fail("unterminated comment");
        };
        if !r[i..].starts_with("-->") {
            return self.fail("'--' is not allowed inside a comment");
        }
        let text = r[..i].to_owned();
        self.advance(i + 3);
        Ok(text)
    }

    /// `PI` (XML §2.6), en `<?`. Fuera del DTD se agrega al árbol.
    fn pi(&mut self, keep: bool) -> R<()> {
        self.advance(2);
        let target = self.name()?;
        if target.eq_ignore_ascii_case("xml") {
            return self.fail("the processing instruction target 'xml' is reserved");
        }
        if target.contains(':') {
            return self.fail("a processing instruction target cannot contain ':'");
        }
        let data = if self.eat("?>") {
            String::new()
        } else {
            self.require_space("after the processing instruction target")?;
            let r = self.rest();
            let Some(end) = r.find("?>") else {
                return self.fail("unterminated processing instruction");
            };
            let data = r[..end].to_owned();
            self.advance(end + 2);
            data
        };
        if keep {
            self.append(NodeData::ProcessingInstruction {
                target: target.into(),
                data: data.into(),
            });
        }
        Ok(())
    }

    /// `doctypedecl` (XML §2.8), en `<!DOCTYPE`.
    fn doctype(&mut self) -> R<()> {
        self.seen_doctype = true;
        self.advance("<!DOCTYPE".len());
        self.require_space("after '<!DOCTYPE'")?;
        let name = self.name()?;
        let space = self.skip_space();
        let mut public_id = None;
        let mut system_id = None;
        if space && self.rest().starts_with("PUBLIC") {
            self.advance(6);
            self.require_space("after 'PUBLIC'")?;
            let public = self.quoted()?;
            if !public.chars().all(is_pubid_char) {
                return self.fail("invalid character in the public identifier");
            }
            public_id = Some(public);
            self.require_space("before the system identifier")?;
            system_id = Some(self.quoted()?);
            self.skip_space();
        } else if space && self.rest().starts_with("SYSTEM") {
            self.advance(6);
            self.require_space("after 'SYSTEM'")?;
            system_id = Some(self.quoted()?);
            self.skip_space();
        }
        if self.eat("[") {
            self.internal_subset()?;
            self.skip_space();
        }
        if !self.eat(">") {
            return self.fail("expected '>' at the end of the DOCTYPE");
        }
        self.html_entities = public_id
            .as_deref()
            .is_some_and(|p| HTML_ENTITY_PUBLIC_IDS.contains(&p));
        if system_id.is_some() {
            self.undeclared_ok = true;
        }
        self.append(NodeData::Doctype {
            name: name.into(),
            public_id: public_id.unwrap_or_default().into(),
            system_id: system_id.unwrap_or_default().into(),
        });
        Ok(())
    }

    /// `intSubset` (XML §2.8), después de `[`; consume el `]`.
    fn internal_subset(&mut self) -> R<()> {
        loop {
            self.skip_space();
            if self.eat("]") {
                return Ok(());
            }
            let r = self.rest();
            if r.is_empty() {
                return self.fail("unterminated internal subset");
            } else if r.starts_with("<!ENTITY") {
                self.entity_decl()?;
            } else if r.starts_with("<!--") {
                self.comment()?;
            } else if r.starts_with("<?") {
                self.pi(false)?;
            } else if ["<!ELEMENT", "<!ATTLIST", "<!NOTATION"]
                .iter()
                .any(|d| r.starts_with(d))
            {
                self.skip_declaration()?;
            } else if r.starts_with('%') {
                self.advance(1);
                self.name()?;
                if !self.eat(";") {
                    return self.fail("expected ';' after a parameter entity reference");
                }
                self.undeclared_ok = true;
            } else {
                return self.fail("invalid markup in the internal subset");
            }
        }
    }

    /// Salta una declaración de markup hasta su `>` (fuera de literales).
    fn skip_declaration(&mut self) -> R<()> {
        let mut quote = None;
        let mut end = None;
        for (i, c) in self.rest().char_indices() {
            match (quote, c) {
                (Some(q), c) if c == q => quote = None,
                (Some(_), _) => {}
                (None, '"' | '\'') => quote = Some(c),
                (None, '>') => {
                    end = Some(i);
                    break;
                }
                _ => {}
            }
        }
        match end {
            Some(i) => {
                self.advance(i + 1);
                Ok(())
            }
            None => self.fail("unterminated markup declaration"),
        }
    }

    /// `EntityDecl` (XML §4.2), en `<!ENTITY`.
    fn entity_decl(&mut self) -> R<()> {
        self.advance("<!ENTITY".len());
        self.require_space("after '<!ENTITY'")?;
        let parameter = self.eat("%");
        if parameter {
            self.require_space("after '%'")?;
        }
        let name = self.name()?;
        if name.contains(':') {
            return self.fail("an entity name cannot contain ':'");
        }
        self.require_space("after the entity name")?;
        let r = self.rest();
        let mut entity = if r.starts_with(['"', '\'']) {
            let raw = self.quoted()?;
            Entity::Internal(self.entity_value(&raw)?.into())
        } else if r.starts_with("SYSTEM") {
            self.advance(6);
            self.require_space("after 'SYSTEM'")?;
            self.quoted()?;
            Entity::External
        } else if r.starts_with("PUBLIC") {
            self.advance(6);
            self.require_space("after 'PUBLIC'")?;
            self.quoted()?;
            self.require_space("before the system identifier")?;
            self.quoted()?;
            Entity::External
        } else {
            return self.fail("expected an entity value or an external identifier");
        };
        let space = self.skip_space();
        if matches!(entity, Entity::External) && space && self.rest().starts_with("NDATA") {
            if parameter {
                return self.fail("a parameter entity cannot be unparsed");
            }
            self.advance(5);
            self.require_space("after 'NDATA'")?;
            self.name()?;
            self.skip_space();
            entity = Entity::Unparsed;
        }
        if !self.eat(">") {
            return self.fail("expected '>' at the end of the entity declaration");
        }
        // La primera declaración manda (XML §4.2).
        if !parameter {
            self.entities.entry(name).or_insert(entity);
        }
        Ok(())
    }

    /// El texto de reemplazo de un `EntityValue` (XML §4.5): las referencias numéricas se
    /// resuelven y las de entidades generales quedan tal cual.
    fn entity_value(&self, raw: &str) -> R<String> {
        let mut out = String::with_capacity(raw.len());
        let mut rest = raw;
        while let Some(i) = rest.find(['&', '%']) {
            out.push_str(&rest[..i]);
            if rest[i..].starts_with('%') {
                return self.fail(
                    "parameter entity references are not allowed inside markup declarations \
                     of the internal subset",
                );
            }
            let tail = &rest[i + 1..];
            let Some(end) = tail.find(';') else {
                return self.fail("unterminated reference in an entity value");
            };
            let body = &tail[..end];
            if let Some(num) = body.strip_prefix('#') {
                match char_ref(num) {
                    Some(c) => out.push(c),
                    None => return self.fail(format!("invalid character reference &{body};")),
                }
            } else if is_name(body) {
                out.push('&');
                out.push_str(body);
                out.push(';');
            } else {
                return self.fail("invalid entity reference in an entity value");
            }
            rest = &tail[end + 1..];
        }
        out.push_str(rest);
        Ok(out)
    }

    /// El valor de una entidad no declarada (`None`: se omite) o el error de WFC: Entity
    /// Declared.
    fn undeclared(&self, name: &str) -> R<Option<&'static str>> {
        if self.html_entities
            && let Some(v) = html_entity(name)
        {
            return Ok(Some(v));
        }
        if self.undeclared_ok {
            Ok(None)
        } else {
            self.fail(format!("undefined entity &{name};"))
        }
    }

    fn count_expansion(&mut self, len: usize) -> R<()> {
        self.expanded += len;
        if self.expanded > EXPANSION_LIMIT {
            return self.fail("entity expansion limit exceeded");
        }
        Ok(())
    }

    // --- Elementos ---

    /// `STag` o `EmptyElemTag` (XML §3.1), en `<`. Devuelve si el elemento quedó cerrado.
    fn start_tag(&mut self) -> R<bool> {
        self.advance(1);
        let qname = self.name()?;
        let mut raw_attrs: Vec<(String, String)> = Vec::new();
        let empty = loop {
            let space = self.skip_space();
            if self.eat("/>") {
                break true;
            }
            if self.eat(">") {
                break false;
            }
            if self.at_end() {
                return self.fail(format!("unterminated start tag <{qname}>"));
            }
            if !space {
                return self.fail("expected whitespace before an attribute");
            }
            let name = self.name()?;
            self.skip_space();
            if !self.eat("=") {
                return self.fail(format!("expected '=' after the attribute {name}"));
            }
            self.skip_space();
            let value = self.attribute_value()?;
            if raw_attrs.iter().any(|(n, _)| *n == name) {
                return self.fail(format!("duplicate attribute {name}"));
            }
            raw_attrs.push((name, value));
        };

        // Declaraciones de namespace (Namespaces in XML §3, NSC: Reserved Prefixes and
        // Namespace Names, NSC: No Prefix Undeclaring).
        let mut scope = Vec::new();
        for (name, value) in &raw_attrs {
            let prefix = if name == "xmlns" {
                ""
            } else if let Some(p) = name.strip_prefix("xmlns:") {
                p
            } else {
                continue;
            };
            let reserved = value == XML_NAMESPACE || value == XMLNS_NAMESPACE;
            let ok = match prefix {
                "xmlns" => false,
                "xml" => value == XML_NAMESPACE,
                "" => !reserved,
                p => is_ncname(p) && !reserved && !value.is_empty(),
            };
            if !ok {
                return self.fail(format!("invalid namespace declaration {name}=\"{value}\""));
            }
            scope.push((prefix.to_owned(), value.clone()));
        }

        let (prefix, local) = self.split_qname(&qname)?;
        let ns = match prefix {
            Some("xmlns") => {
                return self.fail("an element name cannot use the 'xmlns' prefix");
            }
            Some(p) => match self.resolve(&scope, p) {
                Some(uri) => Namespace::from_uri(uri).unwrap_or(Namespace::Null),
                None => return self.fail(format!("unbound namespace prefix '{p}'")),
            },
            None => self
                .resolve(&scope, "")
                .and_then(Namespace::from_uri)
                .unwrap_or(Namespace::Null),
        };

        let mut attrs: Vec<Attribute> = Vec::with_capacity(raw_attrs.len());
        for (name, value) in &raw_attrs {
            let (aprefix, alocal) = self.split_qname(name)?;
            let ans = match aprefix {
                None if alocal == "xmlns" => Some(Namespace::XmlNs),
                None => None,
                Some("xmlns") => Some(Namespace::XmlNs),
                Some(p) => match self.resolve(&scope, p) {
                    Some(uri) => Namespace::from_uri(uri),
                    None => return self.fail(format!("unbound namespace prefix '{p}'")),
                },
            };
            // NSC: Attributes Unique (por nombre expandido).
            if attrs.iter().any(|a| a.ns == ans && a.name == alocal) {
                return self.fail(format!("duplicate attribute {name}"));
            }
            attrs.push(Attribute {
                ns: ans,
                prefix: aprefix.map(Into::into),
                name: alocal.into(),
                value: value.as_str().into(),
            });
        }

        let node = self.doc.create(NodeData::Element(Element {
            ns,
            prefix: prefix.map(Into::into),
            name: local.into(),
            attrs,
            template_contents: None,
        }));
        let parent = self.current();
        self.doc.append(parent, node);
        if self.doc.element(node).is_some_and(|e| e.is_html("link")) {
            self.links.push(node);
        }
        if !empty {
            self.stack.push(Open { node, qname, scope });
        }
        Ok(empty)
    }

    /// `AttValue` normalizado (XML §3.3.3).
    fn attribute_value(&mut self) -> R<String> {
        let raw = self.quoted()?;
        let mut out = String::with_capacity(raw.len());
        let mut open = Vec::new();
        self.normalize_attribute(&raw, &mut out, &mut open)?;
        Ok(out)
    }

    fn normalize_attribute(
        &mut self,
        raw: &str,
        out: &mut String,
        open: &mut Vec<String>,
    ) -> R<()> {
        let mut i = 0;
        while let Some(c) = raw[i..].chars().next() {
            match c {
                '<' => return self.fail("'<' is not allowed in an attribute value"),
                '&' => {
                    let tail = &raw[i + 1..];
                    let Some(end) = tail.find(';') else {
                        return self.fail("unterminated reference in an attribute value");
                    };
                    let body = &tail[..end];
                    if let Some(num) = body.strip_prefix('#') {
                        match char_ref(num) {
                            Some(c) => out.push(c),
                            None => {
                                return self.fail(format!("invalid character reference &{body};"));
                            }
                        }
                    } else if !is_name(body) {
                        return self.fail("invalid entity reference in an attribute value");
                    } else if let Some(v) = predefined(body) {
                        out.push_str(v);
                    } else {
                        match self.entities.get(body).cloned() {
                            Some(Entity::Internal(text)) => {
                                if open.iter().any(|n| n == body)
                                    || open.len() >= ENTITY_DEPTH_LIMIT
                                {
                                    return self
                                        .fail(format!("recursive entity reference &{body};"));
                                }
                                self.count_expansion(text.len())?;
                                open.push(body.to_owned());
                                self.normalize_attribute(&text, out, open)?;
                                open.pop();
                            }
                            Some(_) => {
                                return self.fail(format!(
                                    "reference to the external entity &{body}; in an attribute \
                                     value"
                                ));
                            }
                            None => {
                                if let Some(v) = self.undeclared(body)? {
                                    out.push_str(v);
                                }
                            }
                        }
                    }
                    i += end + 2;
                }
                '\t' | '\n' | '\r' => {
                    out.push(' ');
                    i += 1;
                }
                c => {
                    out.push(c);
                    i += c.len_utf8();
                }
            }
        }
        Ok(())
    }

    /// `ETag` (XML §3.1), en `</`.
    fn end_tag(&mut self) -> R<()> {
        self.advance(2);
        let name = self.name()?;
        self.skip_space();
        if !self.eat(">") {
            return self.fail(format!("expected '>' at the end of </{name}"));
        }
        let open = self.stack.last().expect("hay un elemento abierto");
        if open.qname != name {
            return self.fail(format!(
                "mismatched end tag: expected </{}>, found </{name}>",
                open.qname
            ));
        }
        if self.stack.len() <= self.frames.last().map_or(0, |f| f.depth) {
            return self.fail("an end tag in an entity must close an element opened in it");
        }
        self.stack.pop();
        Ok(())
    }

    /// `content` (XML §3.1) hasta cerrar el elemento raíz.
    fn content(&mut self) -> R<()> {
        while !self.stack.is_empty() {
            if self.at_end() {
                if self.frames.len() > 1 {
                    let depth = self.frames.last().map_or(0, |f| f.depth);
                    if self.stack.len() != depth {
                        return self.fail("an entity must close the elements it opens");
                    }
                    self.frames.pop();
                    continue;
                }
                let open = self
                    .stack
                    .last()
                    .map(|o| o.qname.clone())
                    .unwrap_or_default();
                return self.fail(format!("unclosed element <{open}>"));
            }
            let r = self.rest();
            if r.starts_with("</") {
                self.end_tag()?;
            } else if r.starts_with("<!--") {
                let text = self.comment()?;
                self.append(NodeData::Comment(text.into()));
            } else if r.starts_with("<![CDATA[") {
                self.advance(9);
                let r = self.rest();
                let Some(end) = r.find("]]>") else {
                    return self.fail("unterminated CDATA section");
                };
                let text = r[..end].to_owned();
                self.advance(end + 3);
                self.append(NodeData::CdataSection(text.into()));
            } else if r.starts_with("<?") {
                self.pi(true)?;
            } else if r.starts_with('<') {
                self.start_tag()?;
            } else if r.starts_with('&') {
                match self.reference()? {
                    Reference::Char(c) => self.append_text(c.encode_utf8(&mut [0; 4])),
                    Reference::Entity(name) => self.content_entity(name)?,
                }
            } else {
                let end = r.find(['<', '&']).unwrap_or(r.len());
                if r[..end].contains("]]>") {
                    return self.fail("']]>' is not allowed in text");
                }
                let text = r[..end].to_owned();
                self.advance(end);
                self.append_text(&text);
            }
        }
        Ok(())
    }

    fn content_entity(&mut self, name: String) -> R<()> {
        if let Some(v) = predefined(&name) {
            self.append_text(v);
            return Ok(());
        }
        match self.entities.get(&name).cloned() {
            Some(Entity::Internal(text)) => {
                if self
                    .frames
                    .iter()
                    .any(|f| f.entity.as_deref() == Some(name.as_str()))
                    || self.frames.len() > ENTITY_DEPTH_LIMIT
                {
                    return self.fail(format!("recursive entity reference &{name};"));
                }
                self.count_expansion(text.len())?;
                let depth = self.stack.len();
                self.frames.push(Frame {
                    text,
                    pos: 0,
                    entity: Some(name),
                    depth,
                });
            }
            // Las entidades externas no se cargan (HTML §14.2).
            Some(Entity::External) => {}
            Some(Entity::Unparsed) => {
                return self.fail(format!("reference to the unparsed entity &{name};"));
            }
            None => {
                if let Some(v) = self.undeclared(&name)? {
                    self.append_text(v);
                }
            }
        }
        Ok(())
    }
}

/// Línea y columna (1-based, en code points) del byte `offset` de `text`.
fn position(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let column = before
        .rfind('\n')
        .map_or(before, |i| &before[i + 1..])
        .chars()
        .count()
        + 1;
    (line, column)
}

/// Parsea un documento XML: el árbol construido hasta el primer error fatal (si lo hay) y el
/// error.
pub fn parse_xml_partial(input: &str) -> (Document, Option<XmlError>) {
    parse_xml_partial_with(input, &mut |_, _| {})
}

/// [`parse_xml_partial`] avisando al host de cada elemento `link` del namespace HTML que el
/// parser insertó en el documento, en orden de documento: es cuando el `link` "becomes
/// browsing-context connected" (HTML §4.6.7) y el host busca su recurso (y después dispara
/// `load`/`error`). El parser XML no corre scripts ni cede el control, así que el aviso llega
/// al terminar de parsear, con el `link` conectado y con sus atributos; también los `link`
/// insertados antes de un error de buena formación.
pub fn parse_xml_partial_with(
    input: &str,
    on_link: &mut dyn FnMut(&mut Document, NodeId),
) -> (Document, Option<XmlError>) {
    let mut parser = Parser::new(input);
    let result = parser.run();
    let mut doc = parser.doc;
    for link in parser.links {
        on_link(&mut doc, link);
    }
    (doc, result.err())
}

/// Parsea un documento XML bien formado.
pub fn parse_xml(input: &str) -> Result<Document, XmlError> {
    match parse_xml_partial(input) {
        (doc, None) => Ok(doc),
        (_, Some(e)) => Err(e),
    }
}

/// Parsea un documento XHTML o XML para mostrarlo: ante un error de buena formación queda lo
/// parseado hasta el error.
pub fn parse_xml_document(input: &str) -> Document {
    parse_xml_partial(input).0
}

/// El documento de `DOMParser` para una entrada mal formada (HTML §8.5.1): solo un elemento
/// `parsererror` en [`PARSERERROR_NAMESPACE`], con la descripción del error como texto.
pub fn parsererror_document(error: &XmlError) -> Document {
    let mut doc = Document::new_xml();
    doc.mode = QuirksMode::NoQuirks;
    let ns = Namespace::from_uri(PARSERERROR_NAMESPACE).unwrap_or(Namespace::Null);
    let root = doc.create(NodeData::Element(Element::new(ns, "parsererror")));
    let text = doc.create(NodeData::Text(format!("XML parsing error: {error}").into()));
    doc.append(root, text);
    let document = doc.root();
    doc.append(document, root);
    doc
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

    #[test]
    fn positions() {
        assert_eq!(position("ab\ncd", 4), (2, 2));
        assert_eq!(position("ñx", 2), (1, 2));
    }
}
