//! Tree builder HTML conforme a WHATWG HTML 13.2.6 (tree construction) y 13.4 (parseo de
//! fragmentos).
//!
//! Este módulo contiene el estado del parser (13.2.4), el *tree construction dispatcher* y los
//! algoritmos compartidos (13.2.6.1 a 13.2.6.3). Las reglas de cada insertion mode están en
//! [`modes`] e [`in_body`]; las de contenido foráneo en [`foreign`].
//!
//! Partes del spec que no aplican todavía y se omiten a propósito: el *parser sanitizer
//! configuration* (siempre null), declarative shadow roots y content patching (`allow
//! declarative shadow roots` es false, así que `<template shadowrootmode>` y `<template for>`
//! se insertan como templates normales), custom elements, form owners, el parser
//! especulativo y la ejecución de scripts.

mod foreign;
mod in_body;
mod modes;
mod quirks;

use std::collections::HashMap;

use bf_dom::{Attribute, Document, Element, NodeData, NodeId, QuirksMode};
use bf_infra::{DomString, Namespace};

use crate::tokenizer::{self, ParseError, ProcessingInstruction, State, Token, Tokenizer};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParseOptions {
    /// Scripting habilitado (scripting mode Normal/Inert). Con `false` el modo es Disabled y
    /// `<noscript>` se parsea como contenido normal.
    pub scripting: bool,
    /// El documento es un iframe srcdoc document (nunca entra en quirks mode).
    pub iframe_srcdoc: bool,
}

/// Elemento de contexto para [`parse_fragment`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FragmentContext<'a> {
    pub ns: Namespace,
    pub name: &'a str,
}

/// Parsea un documento completo.
pub fn parse_document(input: &str, opts: &ParseOptions) -> (Document, Vec<ParseError>) {
    let mut builder = TreeBuilder::new(input, opts);
    builder.run();
    builder.finish()
}

/// HTML fragment parsing algorithm (13.4). El resultado es un documento cuyo único hijo es el
/// elemento `html` raíz; los nodos del fragmento son los hijos de ese elemento.
pub fn parse_fragment(
    input: &str,
    context: FragmentContext<'_>,
    opts: &ParseOptions,
) -> (Document, Vec<ParseError>) {
    let mut builder = TreeBuilder::new(input, opts);
    builder.setup_fragment(context);
    builder.run();
    builder.finish()
}

/// Insertion modes (13.2.4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Initial,
    BeforeHtml,
    BeforeHead,
    InHead,
    InHeadNoscript,
    AfterHead,
    InBody,
    Text,
    InTable,
    InTableText,
    InCaption,
    InColumnGroup,
    InTableBody,
    InRow,
    InCell,
    InTemplate,
    AfterBody,
    InFrameset,
    AfterFrameset,
    AfterAfterBody,
    AfterAfterFrameset,
}

/// Start/end tag tal como lo ve el tree builder: los atributos ya pueden tener namespace
/// (después de "adjust foreign attributes").
#[derive(Debug, Clone, PartialEq, Eq)]
struct TagToken {
    name: DomString,
    self_closing: bool,
    attrs: Vec<Attribute>,
}

impl TagToken {
    /// Start tag sintético sin atributos ("insert an HTML element for a "head" start tag...").
    fn named(name: &str) -> Self {
        TagToken {
            name: name.into(),
            self_closing: false,
            attrs: Vec::new(),
        }
    }

    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|a| a.ns.is_none() && a.name == name)
            .map(|a| a.value.as_lossy_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Doctype(tokenizer::Doctype),
    Start(TagToken),
    End(TagToken),
    Comment(DomString),
    Pi(ProcessingInstruction),
    Char(char),
    Eof,
}

impl Tok {
    fn from_token(token: Token) -> Tok {
        let tag = |t: tokenizer::Tag| TagToken {
            name: t.name.into(),
            self_closing: t.self_closing,
            attrs: t
                .attrs
                .into_iter()
                .map(|(name, value)| Attribute::new(name, value))
                .collect(),
        };
        match token {
            Token::Doctype(d) => Tok::Doctype(d),
            Token::StartTag(t) => Tok::Start(tag(t)),
            Token::EndTag(t) => Tok::End(tag(t)),
            Token::Comment(c) => Tok::Comment(c.into()),
            Token::ProcessingInstruction(pi) => Tok::Pi(pi),
            Token::Character(c) => Tok::Char(c),
            // El surrogate real queda en `TreeBuilder::surrogate` (ver `run`).
            Token::LoneSurrogate(_) => Tok::Char('\u{FFFD}'),
            Token::Eof => Tok::Eof,
        }
    }
}

/// Resultado de procesar un token en un insertion mode.
#[must_use]
enum Flow {
    Done,
    /// "Reprocess the token": vuelve a pasar por el dispatcher con el insertion mode actual.
    Reprocess(Tok),
}

/// Entrada de la list of active formatting elements (13.2.4.3).
#[derive(Debug, Clone)]
enum Formatting {
    Marker,
    Element(NodeId, TagToken),
}

/// Insertion location: (target parent, reference child).
#[derive(Debug, Clone, Copy)]
struct Loc {
    parent: NodeId,
    before: Option<NodeId>,
}

/// Código genérico para los errores de tree construction (el spec no les asigna códigos).
const TREE_ERROR: &str = "tree-construction-error";

struct TreeBuilder {
    tokenizer: Tokenizer,
    /// El surrogate suelto del token de carácter en curso (llega a los modos como U+FFFD).
    surrogate: Option<u16>,
    doc: Document,
    mode: Mode,
    original_mode: Mode,
    template_modes: Vec<Mode>,
    open: Vec<NodeId>,
    formatting: Vec<Formatting>,
    head: Option<NodeId>,
    form: Option<NodeId>,
    frameset_ok: bool,
    foster_parenting: bool,
    scripting: bool,
    iframe_srcdoc: bool,
    context: Option<NodeId>,
    pending_table_chars: String,
    /// "If the next token is a U+000A LINE FEED (LF) character token, then ignore that token".
    skip_next_lf: bool,
    /// template contents → template, para "host-including inclusive ancestor".
    template_hosts: HashMap<NodeId, NodeId>,
    stopped: bool,
    errors: Vec<ParseError>,
}

impl TreeBuilder {
    fn new(input: &str, opts: &ParseOptions) -> Self {
        TreeBuilder {
            tokenizer: Tokenizer::new(input),
            doc: Document::new(),
            mode: Mode::Initial,
            original_mode: Mode::Initial,
            template_modes: Vec::new(),
            open: Vec::new(),
            formatting: Vec::new(),
            head: None,
            form: None,
            frameset_ok: true,
            foster_parenting: false,
            scripting: opts.scripting,
            iframe_srcdoc: opts.iframe_srcdoc,
            context: None,
            pending_table_chars: String::new(),
            skip_next_lf: false,
            surrogate: None,
            template_hosts: HashMap::new(),
            stopped: false,
            errors: Vec::new(),
        }
    }

    /// Pasos de 13.4 posteriores a crear el parser.
    fn setup_fragment(&mut self, context: FragmentContext<'_>) {
        let context_el = self.create_element(Element {
            ns: context.ns,
            prefix: None,
            name: context.name.into(),
            attrs: Vec::new(),
            template_contents: None,
        });
        self.context = Some(context_el);
        if context.ns == Namespace::Html {
            let state = match context.name {
                "title" | "textarea" => Some(State::Rcdata),
                "style" | "xmp" | "iframe" | "noembed" | "noframes" => Some(State::Rawtext),
                "script" => Some(State::ScriptData),
                "noscript" if self.scripting => Some(State::Rawtext),
                "plaintext" => Some(State::Plaintext),
                _ => None,
            };
            if let Some(state) = state {
                self.tokenizer.set_state(state);
            }
        }
        let root = self.create_element(Element {
            ns: Namespace::Html,
            prefix: None,
            name: "html".into(),
            attrs: Vec::new(),
            template_contents: None,
        });
        let doc_root = self.doc.root();
        self.doc.append(doc_root, root);
        self.open.push(root);
        // La insertion target redirection map[root] → fragment se modela insertando en `root`:
        // los hijos de `root` son el fragmento.
        if self.is_html(context_el, "template") {
            self.template_modes.push(Mode::InTemplate);
        }
        self.reset_insertion_mode();
        if self.is_html(context_el, "form") {
            self.form = Some(context_el);
        }
    }

    fn run(&mut self) {
        while !self.stopped {
            let cdata = self
                .adjusted_current_node()
                .is_some_and(|n| self.elem(n).ns != Namespace::Html);
            self.tokenizer.set_cdata_allowed(cdata);
            let Some(token) = self.tokenizer.next_token() else {
                break;
            };
            self.surrogate = match token {
                Token::LoneSurrogate(u) => Some(u),
                _ => None,
            };
            let tok = Tok::from_token(token);
            if std::mem::take(&mut self.skip_next_lf) && tok == Tok::Char('\n') {
                continue;
            }
            self.process_token(tok);
        }
        // 13.2.7 The end: "Pop all the nodes off the stack of open elements." Después el event
        // loop corre los microtasks pendientes.
        self.truncate_open(0);
        self.doc.perform_microtask_checkpoint();
    }

    fn finish(mut self) -> (Document, Vec<ParseError>) {
        let mut errors = self.tokenizer.errors().to_vec();
        errors.append(&mut self.errors);
        errors.sort_by_key(|e| e.offset);
        (self.doc, errors)
    }

    fn process_token(&mut self, mut tok: Tok) {
        loop {
            match self.dispatch(tok) {
                Flow::Done => return,
                Flow::Reprocess(t) => tok = t,
            }
        }
    }

    /// Tree construction dispatcher (13.2.6).
    fn dispatch(&mut self, tok: Tok) -> Flow {
        if self.use_html_rules(&tok) {
            self.process_in(self.mode, tok)
        } else {
            self.foreign_content(tok)
        }
    }

    fn use_html_rules(&self, tok: &Tok) -> bool {
        let Some(acn) = self.adjusted_current_node() else {
            return true;
        };
        let el = self.elem(acn);
        if el.ns == Namespace::Html {
            return true;
        }
        let mathml_text_ip = is_mathml_text_integration_point(el);
        match tok {
            Tok::Start(t) if mathml_text_ip && t.name != "mglyph" && t.name != "malignmark" => true,
            Tok::Char(_) if mathml_text_ip => true,
            Tok::Start(t)
                if el.ns == Namespace::MathMl && el.name == "annotation-xml" && t.name == "svg" =>
            {
                true
            }
            Tok::Start(_) | Tok::Char(_) if is_html_integration_point(el) => true,
            Tok::Eof => true,
            _ => false,
        }
    }

    /// Procesa el token con las reglas del insertion mode `mode` (sin cambiar `self.mode`).
    fn process_in(&mut self, mode: Mode, tok: Tok) -> Flow {
        match mode {
            Mode::Initial => self.initial(tok),
            Mode::BeforeHtml => self.before_html(tok),
            Mode::BeforeHead => self.before_head(tok),
            Mode::InHead => self.in_head(tok),
            Mode::InHeadNoscript => self.in_head_noscript(tok),
            Mode::AfterHead => self.after_head(tok),
            Mode::InBody => self.in_body(tok),
            Mode::Text => self.text(tok),
            Mode::InTable => self.in_table(tok),
            Mode::InTableText => self.in_table_text(tok),
            Mode::InCaption => self.in_caption(tok),
            Mode::InColumnGroup => self.in_column_group(tok),
            Mode::InTableBody => self.in_table_body(tok),
            Mode::InRow => self.in_row(tok),
            Mode::InCell => self.in_cell(tok),
            Mode::InTemplate => self.in_template(tok),
            Mode::AfterBody => self.after_body(tok),
            Mode::InFrameset => self.in_frameset(tok),
            Mode::AfterFrameset => self.after_frameset(tok),
            Mode::AfterAfterBody => self.after_after_body(tok),
            Mode::AfterAfterFrameset => self.after_after_frameset(tok),
        }
    }

    fn error(&mut self) {
        self.errors.push(ParseError {
            code: TREE_ERROR,
            offset: self.tokenizer.offset(),
        });
    }

    // --- Consultas sobre nodos ---

    fn elem(&self, id: NodeId) -> &Element {
        self.doc.element(id).expect("se esperaba un elemento")
    }

    fn is_html(&self, id: NodeId, name: &str) -> bool {
        self.doc.element(id).is_some_and(|e| e.is_html(name))
    }

    fn is_html_one_of(&self, id: NodeId, names: &[&str]) -> bool {
        self.doc
            .element(id)
            .is_some_and(|e| e.ns == Namespace::Html && names.contains(&e.name.as_lossy_str()))
    }

    fn current(&self) -> NodeId {
        *self.open.last().expect("stack of open elements vacía")
    }

    /// 13.2.4.2 adjusted current node.
    fn adjusted_current_node(&self) -> Option<NodeId> {
        match self.context {
            Some(context) if self.open.len() == 1 => Some(context),
            _ => self.open.last().copied(),
        }
    }

    fn current_is_html(&self, name: &str) -> bool {
        self.open.last().is_some_and(|&n| self.is_html(n, name))
    }

    fn current_is_html_one_of(&self, names: &[&str]) -> bool {
        self.open
            .last()
            .is_some_and(|&n| self.is_html_one_of(n, names))
    }

    fn stack_has_template(&self) -> bool {
        self.open.iter().any(|&n| self.is_html(n, "template"))
    }

    /// 13.2.4.4 "parsing template contents".
    fn parsing_template_contents(&self) -> bool {
        self.stack_has_template() || self.context.is_some_and(|c| self.is_html(c, "template"))
    }

    // --- Scopes (13.2.4.2) ---

    /// "Has an element target node in a specific scope": `target` reconoce el nodo buscado y
    /// `kind` define la lista de tipos que cortan la búsqueda.
    fn in_scope(&self, target: impl Fn(NodeId) -> bool, kind: Scope) -> bool {
        for &node in self.open.iter().rev() {
            if target(node) {
                return true;
            }
            if kind.is_boundary(self.elem(node)) {
                return false;
            }
        }
        false
    }

    fn has_in_scope(&self, name: &str, kind: Scope) -> bool {
        self.in_scope(|n| self.is_html(n, name), kind)
    }

    fn has_node_in_scope(&self, node: NodeId) -> bool {
        self.in_scope(|n| n == node, Scope::Default)
    }

    // --- Stack of open elements ---

    /// Saca el current node. Todo lo que sale de la stack pasa por aquí (o por
    /// `remove_from_stack`) para correr el hook de `option` (4.10.10).
    fn pop(&mut self) -> NodeId {
        let node = self.open.pop().expect("stack of open elements vacía");
        self.doc.option_popped(node);
        node
    }

    /// Saca elementos hasta que la stack tenga `len` elementos.
    fn truncate_open(&mut self, len: usize) {
        while self.open.len() > len {
            self.pop();
        }
    }

    /// Pop elements until an element matching `pred` has been popped.
    fn pop_until(&mut self, pred: impl Fn(&Self, NodeId) -> bool) {
        while !self.open.is_empty() {
            let node = self.pop();
            if pred(self, node) {
                return;
            }
        }
    }

    fn pop_until_html(&mut self, name: &str) {
        self.pop_until(|tb, n| tb.is_html(n, name));
    }

    fn remove_from_stack(&mut self, node: NodeId) {
        if let Some(i) = self.open.iter().rposition(|&n| n == node) {
            self.open.remove(i);
            self.doc.option_popped(node);
        }
    }

    /// 13.2.6.3 generate implied end tags, excepto `except` si no es `None`.
    fn generate_implied_end_tags(&mut self, except: Option<&str>) {
        const IMPLIED: &[&str] = &[
            "dd", "dt", "li", "optgroup", "option", "p", "rb", "rp", "rt", "rtc",
        ];
        while let Some(&node) = self.open.last() {
            let el = self.elem(node);
            if el.ns == Namespace::Html
                && IMPLIED.contains(&el.name.as_lossy_str())
                && except != Some(el.name.as_lossy_str())
            {
                self.pop();
            } else {
                return;
            }
        }
    }

    /// 13.2.6.3 generate all implied end tags thoroughly.
    fn generate_implied_end_tags_thoroughly(&mut self) {
        const THOROUGH: &[&str] = &[
            "caption", "colgroup", "dd", "dt", "li", "optgroup", "option", "p", "rb", "rp", "rt",
            "rtc", "tbody", "td", "tfoot", "th", "thead", "tr",
        ];
        while self.current_is_html_one_of(THOROUGH) {
            self.pop();
        }
    }

    /// 13.2.4.1 reset the insertion mode appropriately.
    fn reset_insertion_mode(&mut self) {
        let mut index = self.open.len() - 1;
        loop {
            let mut node = self.open[index];
            let last = index == 0;
            if last && let Some(context) = self.context {
                node = context;
            }
            let el = self.elem(node);
            if el.ns == Namespace::Html {
                let mode = match el.name.as_lossy_str() {
                    "td" | "th" if !last => Some(Mode::InCell),
                    "tr" => Some(Mode::InRow),
                    "tbody" | "thead" | "tfoot" => Some(Mode::InTableBody),
                    "caption" => Some(Mode::InCaption),
                    "colgroup" => Some(Mode::InColumnGroup),
                    "table" => Some(Mode::InTable),
                    "template" => Some(*self.template_modes.last().expect("template mode")),
                    "head" if !last => Some(Mode::InHead),
                    "body" => Some(Mode::InBody),
                    "frameset" => Some(Mode::InFrameset),
                    "html" => Some(if self.head.is_none() {
                        Mode::BeforeHead
                    } else {
                        Mode::AfterHead
                    }),
                    _ => None,
                };
                if let Some(mode) = mode {
                    self.mode = mode;
                    return;
                }
            }
            if last {
                self.mode = Mode::InBody;
                return;
            }
            index -= 1;
        }
    }

    // --- List of active formatting elements (13.2.4.3) ---

    fn push_marker(&mut self) {
        self.formatting.push(Formatting::Marker);
    }

    fn formatting_index(&self, node: NodeId) -> Option<usize> {
        self.formatting
            .iter()
            .rposition(|e| matches!(e, Formatting::Element(n, _) if *n == node))
    }

    /// Push onto the list of active formatting elements (con la cláusula del Arca de Noé).
    fn push_formatting(&mut self, node: NodeId, tag: TagToken) {
        let el = self.elem(node);
        let mut same = Vec::new();
        for (i, entry) in self.formatting.iter().enumerate().rev() {
            match entry {
                Formatting::Marker => break,
                Formatting::Element(other, other_tag) => {
                    let other_el = self.elem(*other);
                    if other_el.name == el.name
                        && other_el.ns == el.ns
                        && same_attributes(&other_tag.attrs, &tag.attrs)
                    {
                        same.push(i);
                    }
                }
            }
        }
        if same.len() >= 3 {
            // `same` está en orden inverso: el último es el más antiguo.
            let earliest = *same.last().expect("al menos tres");
            self.formatting.remove(earliest);
        }
        self.formatting.push(Formatting::Element(node, tag));
    }

    fn is_open(&self, node: NodeId) -> bool {
        self.open.contains(&node)
    }

    /// Reconstruct the active formatting elements.
    fn reconstruct_formatting(&mut self) {
        let Some(last) = self.formatting.last() else {
            return;
        };
        match last {
            Formatting::Marker => return,
            Formatting::Element(n, _) if self.is_open(*n) => return,
            Formatting::Element(..) => {}
        }
        let mut index = self.formatting.len() - 1;
        // Rewind.
        while index > 0 {
            let open_or_marker = match &self.formatting[index - 1] {
                Formatting::Marker => true,
                Formatting::Element(n, _) => self.is_open(*n),
            };
            if open_or_marker {
                break;
            }
            index -= 1;
        }
        // Advance / create.
        while index < self.formatting.len() {
            let Formatting::Element(_, tag) = &self.formatting[index] else {
                unreachable!("las entradas posteriores al rewind no son markers");
            };
            let tag = tag.clone();
            let new = self.insert_html(&tag);
            self.formatting[index] = Formatting::Element(new, tag);
            index += 1;
        }
    }

    /// Clear the list of active formatting elements up to the last marker.
    fn clear_formatting_to_last_marker(&mut self) {
        while let Some(entry) = self.formatting.pop() {
            if matches!(entry, Formatting::Marker) {
                return;
            }
        }
    }

    // --- Creación e inserción de nodos (13.2.6.1) ---

    fn create_element(&mut self, element: Element) -> NodeId {
        let id = self.doc.create(NodeData::Element(element));
        if let Some(contents) = self.elem(id).template_contents {
            self.template_hosts.insert(contents, id);
        }
        id
    }

    /// Create an element for a token. `intended_parent` solo importa para custom elements y
    /// form owners, que aún no existen.
    fn create_element_for_token(&mut self, tag: &TagToken, ns: Namespace) -> NodeId {
        self.create_element(Element {
            ns,
            prefix: None,
            name: tag.name.clone(),
            attrs: tag.attrs.clone(),
            template_contents: None,
        })
    }

    /// Appropriate place for inserting a node.
    fn appropriate_place(&self, override_target: Option<NodeId>) -> Loc {
        let mut parent = override_target.unwrap_or_else(|| self.current());
        let mut before = None;
        if self.foster_parenting
            && self.is_html_one_of(parent, &["table", "tbody", "tfoot", "thead", "tr"])
        {
            let last = self
                .open
                .iter()
                .rposition(|&n| self.is_html_one_of(n, &["template", "table"]));
            match last {
                None => {
                    return Loc {
                        parent: self.open[0],
                        before: None,
                    };
                }
                Some(i) => {
                    let last = self.open[i];
                    if self.is_html(last, "template") {
                        parent = last;
                    } else if let Some(table_parent) = self.doc.parent(last) {
                        parent = table_parent;
                        before = Some(last);
                    } else {
                        parent = self.open[i - 1];
                    }
                }
            }
        }
        if let Some(contents) = self.doc.element(parent).and_then(|e| {
            if e.is_html("template") {
                e.template_contents
            } else {
                None
            }
        }) {
            // La insertion target de un template (content patching) siempre es null aquí.
            return Loc {
                parent: contents,
                before: None,
            };
        }
        Loc { parent, before }
    }

    fn is_host_including_inclusive_ancestor(&self, ancestor: NodeId, node: NodeId) -> bool {
        let mut current = Some(node);
        while let Some(n) = current {
            if n == ancestor {
                return true;
            }
            current = self
                .doc
                .parent(n)
                .or_else(|| self.template_hosts.get(&n).copied());
        }
        false
    }

    fn has_element_child(&self, node: NodeId) -> bool {
        self.doc
            .children(node)
            .any(|c| self.doc.element(c).is_some())
    }

    /// Insert an element at the adjusted insertion location.
    fn insert_element_at(&mut self, element: NodeId, location: Option<Loc>) {
        let loc = location.unwrap_or_else(|| self.appropriate_place(None));
        let parent_is_document = matches!(self.doc.get(loc.parent).data, NodeData::Document);
        if self.doc.parent(element).is_some()
            || self.is_host_including_inclusive_ancestor(element, loc.parent)
            || (parent_is_document && self.has_element_child(loc.parent))
        {
            return;
        }
        self.doc.insert_before(loc.parent, element, loc.before);
    }

    /// Insert a foreign element.
    fn insert_foreign(&mut self, tag: &TagToken, ns: Namespace) -> NodeId {
        let element = self.create_element_for_token(tag, ns);
        self.insert_element_at(element, None);
        self.open.push(element);
        element
    }

    /// Insert an HTML element.
    fn insert_html(&mut self, tag: &TagToken) -> NodeId {
        self.insert_foreign(tag, Namespace::Html)
    }

    /// Insert a character (acepta varios caracteres a la vez).
    fn insert_chars(&mut self, data: &str) {
        let loc = self.appropriate_place(None);
        if matches!(self.doc.get(loc.parent).data, NodeData::Document) {
            return;
        }
        let previous = match loc.before {
            Some(before) => self.doc.get(before).prev_sibling(),
            None => self.doc.get(loc.parent).last_child(),
        };
        if let Some(prev) = previous
            && let NodeData::Text(text) = &mut self.doc.get_mut(prev).data
        {
            text.push_str(data);
            return;
        }
        let text = self.doc.create(NodeData::Text(data.into()));
        self.doc.insert_before(loc.parent, text, loc.before);
    }

    fn insert_char(&mut self, c: char) {
        // Un surrogate suelto de la entrada llega como U+FFFD: se inserta su valor real.
        if c == '\u{FFFD}'
            && let Some(u) = self.surrogate.take()
        {
            self.insert_code_unit(u);
            return;
        }
        let mut buf = [0; 4];
        self.insert_chars(c.encode_utf8(&mut buf));
    }

    /// Insert a character, para un surrogate suelto.
    fn insert_code_unit(&mut self, u: u16) {
        let loc = self.appropriate_place(None);
        if matches!(self.doc.get(loc.parent).data, NodeData::Document) {
            return;
        }
        let previous = match loc.before {
            Some(before) => self.doc.get(before).prev_sibling(),
            None => self.doc.get(loc.parent).last_child(),
        };
        if let Some(prev) = previous
            && let NodeData::Text(text) = &mut self.doc.get_mut(prev).data
        {
            text.push_code_unit(u);
            return;
        }
        let text = self.doc.create(NodeData::Text(DomString::from_utf16(&[u])));
        self.doc.insert_before(loc.parent, text, loc.before);
    }

    fn insert_comment(&mut self, data: DomString, location: Option<Loc>) {
        let loc = location.unwrap_or_else(|| self.appropriate_place(None));
        let comment = self.doc.create(NodeData::Comment(data));
        self.doc.insert_before(loc.parent, comment, loc.before);
    }

    fn insert_processing_instruction(&mut self, pi: ProcessingInstruction, location: Option<Loc>) {
        let loc = location.unwrap_or_else(|| self.appropriate_place(None));
        let node = self.doc.create(NodeData::ProcessingInstruction {
            target: pi.target.into(),
            data: pi.data.into(),
        });
        self.doc.insert_before(loc.parent, node, loc.before);
    }

    fn document_loc(&self) -> Loc {
        Loc {
            parent: self.doc.root(),
            before: None,
        }
    }

    // --- 13.2.6.2 Parsing elements that contain only text ---

    fn generic_text_element(&mut self, tag: &TagToken, state: State) {
        self.insert_html(tag);
        self.tokenizer.set_state(state);
        self.original_mode = self.mode;
        self.mode = Mode::Text;
    }

    fn set_quirks_mode(&mut self, mode: QuirksMode) {
        self.doc.mode = mode;
    }
}

/// Tipos de scope de 13.2.4.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Default,
    ListItem,
    Button,
    Table,
}

impl Scope {
    fn is_boundary(self, el: &Element) -> bool {
        let html =
            |names: &[&str]| el.ns == Namespace::Html && names.contains(&el.name.as_lossy_str());
        let default = || {
            html(&[
                "applet", "caption", "html", "table", "td", "th", "marquee", "object", "select",
                "template",
            ]) || (el.ns == Namespace::MathMl
                && matches!(
                    el.name.as_lossy_str(),
                    "mi" | "mo" | "mn" | "ms" | "mtext" | "annotation-xml"
                ))
                || (el.ns == Namespace::Svg
                    && matches!(el.name.as_lossy_str(), "foreignObject" | "desc" | "title"))
        };
        match self {
            Scope::Default => default(),
            Scope::ListItem => default() || html(&["ol", "ul"]),
            Scope::Button => default() || html(&["button"]),
            Scope::Table => html(&["html", "table", "template"]),
        }
    }
}

/// Categoría *special* de 13.2.4.2.
fn is_special(el: &Element) -> bool {
    match el.ns {
        Namespace::Html => matches!(
            el.name.as_lossy_str(),
            "address"
                | "applet"
                | "area"
                | "article"
                | "aside"
                | "base"
                | "basefont"
                | "bgsound"
                | "blockquote"
                | "body"
                | "br"
                | "button"
                | "caption"
                | "center"
                | "col"
                | "colgroup"
                | "dd"
                | "details"
                | "dir"
                | "div"
                | "dl"
                | "dt"
                | "embed"
                | "fieldset"
                | "figcaption"
                | "figure"
                | "footer"
                | "form"
                | "frame"
                | "frameset"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "head"
                | "header"
                | "hgroup"
                | "hr"
                | "html"
                | "iframe"
                | "img"
                | "input"
                | "keygen"
                | "li"
                | "link"
                | "listing"
                | "main"
                | "marquee"
                | "menu"
                | "meta"
                | "nav"
                | "noembed"
                | "noframes"
                | "noscript"
                | "object"
                | "ol"
                | "p"
                | "param"
                | "plaintext"
                | "pre"
                | "script"
                | "search"
                | "section"
                | "select"
                | "source"
                | "style"
                | "summary"
                | "table"
                | "tbody"
                | "td"
                | "template"
                | "textarea"
                | "tfoot"
                | "th"
                | "thead"
                | "title"
                | "tr"
                | "track"
                | "ul"
                | "wbr"
                | "xmp"
        ),
        Namespace::MathMl => matches!(
            el.name.as_lossy_str(),
            "mi" | "mo" | "mn" | "ms" | "mtext" | "annotation-xml"
        ),
        Namespace::Svg => matches!(el.name.as_lossy_str(), "foreignObject" | "desc" | "title"),
        _ => false,
    }
}

/// MathML text integration point (13.2.6).
fn is_mathml_text_integration_point(el: &Element) -> bool {
    el.ns == Namespace::MathMl
        && matches!(el.name.as_lossy_str(), "mi" | "mo" | "mn" | "ms" | "mtext")
}

/// HTML integration point (13.2.6). Los atributos del elemento son los de su start tag.
fn is_html_integration_point(el: &Element) -> bool {
    match el.ns {
        Namespace::MathMl if el.name == "annotation-xml" => el.attr("encoding").is_some_and(|v| {
            v.eq_ignore_ascii_case("text/html") || v.eq_ignore_ascii_case("application/xhtml+xml")
        }),
        Namespace::Svg => matches!(el.name.as_lossy_str(), "foreignObject" | "desc" | "title"),
        _ => false,
    }
}

/// ¿Los dos tokens tienen los mismos atributos (nombre, namespace y valor), sin importar el
/// orden?
fn same_attributes(a: &[Attribute], b: &[Attribute]) -> bool {
    a.len() == b.len()
        && a.iter().all(|x| {
            b.iter()
                .any(|y| x.name == y.name && x.ns == y.ns && x.value == y.value)
        })
}

fn is_ascii_whitespace(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' ')
}
