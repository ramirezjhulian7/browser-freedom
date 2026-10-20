//! Matching de selectores contra `bf-dom` (Selectors 4, de derecha a izquierda).

use std::collections::HashSet;

use bf_dom::{Document, Element, NodeData, NodeId, QuirksMode};
use bf_infra::Namespace;

use super::{
    AttrNs, AttrOp, AttrSelector, CaseFlag, Combinator, ComplexSelector, Compound, Dir,
    NsConstraint, PseudoClass, PseudoElement, SelectorList, Simple, Specificity,
};

/// Estado dinámico y del documento que el matching necesita.
#[derive(Debug, Clone, Default)]
pub struct MatchContext {
    /// Modo del documento: en quirks mode las clases e ids se comparan sin distinguir
    /// mayúsculas ASCII.
    pub quirks_mode: QuirksMode,
    /// Elementos en estado `:hover` (el llamador incluye los ancestros del elemento señalado).
    pub hover: HashSet<NodeId>,
    /// Elementos en estado `:active`.
    pub active: HashSet<NodeId>,
    pub focus: Option<NodeId>,
    /// El foco actual debe indicarse visiblemente (`:focus-visible`).
    pub focus_visible: bool,
    pub target: Option<NodeId>,
    /// Links visitados.
    pub visited_links: HashSet<NodeId>,
    /// Scoping root de `:scope`; si es `None`, `:scope` equivale a `:root`.
    pub scope: Option<NodeId>,
}

impl MatchContext {
    /// Contexto sin estado dinámico, con el quirks mode del documento.
    pub fn for_document(doc: &Document) -> Self {
        MatchContext {
            quirks_mode: doc.mode,
            ..MatchContext::default()
        }
    }
}

/// ¿`selector` coincide con el elemento `node`? Los selectores con pseudo-elemento no
/// coinciden con el elemento en sí (ver [`matching_specificity`]).
pub fn matches(
    selector: &ComplexSelector,
    doc: &Document,
    node: NodeId,
    ctx: &MatchContext,
) -> bool {
    selector.pseudo_element.is_none() && Matcher { doc, ctx }.complex(selector, node, None)
}

/// ¿Algún selector de la lista coincide con `node`?
pub fn matches_list(list: &SelectorList, doc: &Document, node: NodeId, ctx: &MatchContext) -> bool {
    list.0.iter().any(|s| matches(s, doc, node, ctx))
}

/// La especificidad efectiva de `list` para `node` (o su pseudo-elemento `pseudo`): la del
/// selector más específico que coincide, o `None` si ninguno coincide.
pub fn matching_specificity(
    list: &SelectorList,
    doc: &Document,
    node: NodeId,
    pseudo: Option<PseudoElement>,
    ctx: &MatchContext,
) -> Option<Specificity> {
    let matcher = Matcher { doc, ctx };
    list.0
        .iter()
        .filter(|s| s.pseudo_element == pseudo && matcher.complex(s, node, None))
        .map(ComplexSelector::specificity)
        .max()
}

struct Matcher<'a> {
    doc: &'a Document,
    ctx: &'a MatchContext,
}

/// Atributos HTML cuyos valores se comparan sin distinguir mayúsculas ASCII
/// (HTML §4.16.2 "Case-sensitivity of selectors").
const CASE_INSENSITIVE_ATTRS: &[&str] = &[
    "accept",
    "accept-charset",
    "align",
    "alink",
    "axis",
    "bgcolor",
    "charset",
    "checked",
    "clear",
    "codetype",
    "color",
    "compact",
    "declare",
    "defer",
    "dir",
    "direction",
    "disabled",
    "enctype",
    "face",
    "frame",
    "hreflang",
    "http-equiv",
    "lang",
    "language",
    "link",
    "media",
    "method",
    "multiple",
    "nohref",
    "noresize",
    "noshade",
    "nowrap",
    "readonly",
    "rel",
    "rev",
    "rules",
    "scope",
    "scrolling",
    "selected",
    "shape",
    "target",
    "text",
    "type",
    "valign",
    "valuetype",
    "vlink",
];

fn is_document_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0C')
}

impl Matcher<'_> {
    fn element(&self, node: NodeId) -> Option<&Element> {
        self.doc.element(node)
    }

    fn parent_element(&self, node: NodeId) -> Option<NodeId> {
        self.doc.parent(node).filter(|&p| self.element(p).is_some())
    }

    fn prev_element(&self, node: NodeId) -> Option<NodeId> {
        let mut current = self.doc.get(node).prev_sibling();
        while let Some(n) = current {
            if self.element(n).is_some() {
                return Some(n);
            }
            current = self.doc.get(n).prev_sibling();
        }
        None
    }

    fn next_element(&self, node: NodeId) -> Option<NodeId> {
        let mut current = self.doc.get(node).next_sibling();
        while let Some(n) = current {
            if self.element(n).is_some() {
                return Some(n);
            }
            current = self.doc.get(n).next_sibling();
        }
        None
    }

    fn descendant_elements(&self, node: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.doc
            .inclusive_descendants(node)
            .into_iter()
            .skip(1)
            .filter(|&n| self.element(n).is_some())
    }

    fn complex(&self, sel: &ComplexSelector, node: NodeId, anchor: Option<NodeId>) -> bool {
        self.element(node).is_some() && self.from(sel, sel.compounds.len() - 1, node, anchor)
    }

    /// ¿`compounds[..=i]` coincide terminando en `node`?
    fn from(&self, sel: &ComplexSelector, i: usize, node: NodeId, anchor: Option<NodeId>) -> bool {
        if !self.compound(&sel.compounds[i], node) {
            return false;
        }
        if i == 0 {
            return match (sel.leading, anchor) {
                (Some(c), Some(anchor)) => self.related(c, anchor, node),
                _ => true,
            };
        }
        match sel.combinators[i - 1] {
            Combinator::Descendant => {
                let mut current = self.parent_element(node);
                while let Some(p) = current {
                    if self.from(sel, i - 1, p, anchor) {
                        return true;
                    }
                    current = self.parent_element(p);
                }
                false
            }
            Combinator::Child => self
                .parent_element(node)
                .is_some_and(|p| self.from(sel, i - 1, p, anchor)),
            Combinator::NextSibling => self
                .prev_element(node)
                .is_some_and(|p| self.from(sel, i - 1, p, anchor)),
            Combinator::SubsequentSibling => {
                let mut current = self.prev_element(node);
                while let Some(p) = current {
                    if self.from(sel, i - 1, p, anchor) {
                        return true;
                    }
                    current = self.prev_element(p);
                }
                false
            }
        }
    }

    /// ¿`node` está relacionado con `anchor` por `c` (anchor a la izquierda)?
    fn related(&self, c: Combinator, anchor: NodeId, node: NodeId) -> bool {
        match c {
            Combinator::Descendant => {
                anchor != node && self.doc.is_inclusive_ancestor(anchor, node)
            }
            Combinator::Child => self.parent_element(node) == Some(anchor),
            Combinator::NextSibling => self.prev_element(node) == Some(anchor),
            Combinator::SubsequentSibling => {
                let mut current = self.prev_element(node);
                while let Some(p) = current {
                    if p == anchor {
                        return true;
                    }
                    current = self.prev_element(p);
                }
                false
            }
        }
    }

    /// `:has()`: busca candidatos solo donde el relative selector puede llegar.
    fn has(&self, list: &SelectorList, anchor: NodeId) -> bool {
        list.0.iter().any(|sel| {
            let leading = sel.leading.unwrap_or(Combinator::Descendant);
            let candidates: Vec<NodeId> = match leading {
                Combinator::Descendant | Combinator::Child => {
                    self.descendant_elements(anchor).collect()
                }
                Combinator::NextSibling | Combinator::SubsequentSibling => {
                    let reaches_down = sel
                        .combinators
                        .iter()
                        .any(|c| matches!(c, Combinator::Descendant | Combinator::Child));
                    // Hermanos siguientes del anchor (y sus descendientes si algún combinator
                    // posterior baja en el árbol).
                    let mut out = Vec::new();
                    let mut sibling = self.next_element(anchor);
                    while let Some(s) = sibling {
                        out.push(s);
                        if reaches_down {
                            out.extend(self.descendant_elements(s));
                        }
                        sibling = self.next_element(s);
                    }
                    out
                }
            };
            candidates
                .into_iter()
                .any(|c| self.complex(sel, c, Some(anchor)))
        })
    }

    fn compound(&self, compound: &Compound, node: NodeId) -> bool {
        compound.iter().all(|simple| self.simple(simple, node))
    }

    fn ns_matches(ns: &NsConstraint, el: &Element) -> bool {
        match ns {
            NsConstraint::Any => true,
            // `|E`: elementos sin namespace.
            NsConstraint::None => el.ns == Namespace::Null,
            NsConstraint::Url(url) => el.ns.uri() == url,
        }
    }

    fn simple(&self, simple: &Simple, node: NodeId) -> bool {
        let Some(el) = self.element(node) else {
            return false;
        };
        let quirks = self.ctx.quirks_mode == QuirksMode::Quirks;
        match simple {
            Simple::Type { ns, name, lower } => {
                Self::ns_matches(ns, el)
                    && if el.ns == Namespace::Html {
                        el.name == *lower
                    } else {
                        el.name == *name
                    }
            }
            Simple::Universal(ns) => Self::ns_matches(ns, el),
            Simple::Id(id) => el.attr("id").is_some_and(|v| {
                if quirks {
                    v.eq_ignore_ascii_case(id)
                } else {
                    v == id
                }
            }),
            Simple::Class(class) => el.attr("class").is_some_and(|v| {
                v.split(is_document_whitespace).any(|c| {
                    if quirks {
                        c.eq_ignore_ascii_case(class)
                    } else {
                        c == class
                    }
                })
            }),
            Simple::Attribute(attr) => self.attribute(attr, el),
            Simple::PseudoClass(pc) => self.pseudo_class(pc, node, el),
            Simple::Nesting => self.scope(node),
        }
    }

    fn attribute(&self, sel: &AttrSelector, el: &Element) -> bool {
        let html = el.ns == Namespace::Html;
        let name = if html {
            sel.name.to_ascii_lowercase()
        } else {
            sel.name.clone()
        };
        el.attrs.iter().any(|attr| {
            let ns_ok = match &sel.ns {
                AttrNs::None => attr.ns.is_none(),
                AttrNs::Any => true,
                AttrNs::Url(url) => attr.ns.is_some_and(|ns| ns.uri() == url),
            };
            if !ns_ok || attr.name != name {
                return false;
            }
            let Some((op, value, flag)) = &sel.matcher else {
                return true;
            };
            let insensitive = match flag {
                CaseFlag::Insensitive => true,
                CaseFlag::Sensitive => false,
                CaseFlag::Default => {
                    html && attr.ns.is_none() && CASE_INSENSITIVE_ATTRS.contains(&name.as_str())
                }
            };
            let (actual, expected) = if insensitive {
                (attr.value.to_ascii_lowercase(), value.to_ascii_lowercase())
            } else {
                (attr.value.to_string(), value.clone())
            };
            match op {
                AttrOp::Equals => actual == expected,
                AttrOp::Includes => {
                    !expected.is_empty()
                        && !expected.contains(is_document_whitespace)
                        && actual.split(is_document_whitespace).any(|w| w == expected)
                }
                AttrOp::DashMatch => {
                    actual == expected || actual.starts_with(&format!("{expected}-"))
                }
                AttrOp::Prefix => !expected.is_empty() && actual.starts_with(&expected),
                AttrOp::Suffix => !expected.is_empty() && actual.ends_with(&expected),
                AttrOp::Substring => !expected.is_empty() && actual.contains(&expected),
            }
        })
    }

    fn matches_any(&self, list: &SelectorList, node: NodeId) -> bool {
        list.0
            .iter()
            .any(|s| s.pseudo_element.is_none() && self.complex(s, node, None))
    }

    /// Índice (desde 1) de `node` entre sus hermanos elementos que cumplen `filter`.
    fn index(&self, node: NodeId, from_end: bool, filter: &dyn Fn(NodeId) -> bool) -> i64 {
        let mut index = 1;
        let mut current = if from_end {
            self.next_element(node)
        } else {
            self.prev_element(node)
        };
        while let Some(n) = current {
            if filter(n) {
                index += 1;
            }
            current = if from_end {
                self.next_element(n)
            } else {
                self.prev_element(n)
            };
        }
        index
    }

    fn same_type(&self, a: NodeId, b: NodeId) -> bool {
        match (self.element(a), self.element(b)) {
            (Some(x), Some(y)) => x.ns == y.ns && x.name == y.name,
            _ => false,
        }
    }

    fn scope(&self, node: NodeId) -> bool {
        match self.ctx.scope {
            Some(scope) => scope == node,
            None => self.doc.parent(node) == Some(self.doc.root()),
        }
    }

    fn pseudo_class(&self, pc: &PseudoClass, node: NodeId, el: &Element) -> bool {
        let html = el.ns == Namespace::Html;
        let is = |name: &str| el.is_html(name);
        match pc {
            PseudoClass::Not(list) => !self.matches_any(list, node),
            PseudoClass::Is(list) | PseudoClass::Where(list) => self.matches_any(list, node),
            PseudoClass::Has(list) => self.has(list, node),
            PseudoClass::NthChild { a, b, of } | PseudoClass::NthLastChild { a, b, of } => {
                let from_end = matches!(pc, PseudoClass::NthLastChild { .. });
                match of {
                    Some(list) => {
                        self.matches_any(list, node)
                            && nth(
                                *a,
                                *b,
                                self.index(node, from_end, &|n| self.matches_any(list, n)),
                            )
                    }
                    None => nth(*a, *b, self.index(node, from_end, &|_| true)),
                }
            }
            PseudoClass::NthOfType { a, b } | PseudoClass::NthLastOfType { a, b } => {
                let from_end = matches!(pc, PseudoClass::NthLastOfType { .. });
                nth(
                    *a,
                    *b,
                    self.index(node, from_end, &|n| self.same_type(n, node)),
                )
            }
            PseudoClass::FirstChild => self.prev_element(node).is_none(),
            PseudoClass::LastChild => self.next_element(node).is_none(),
            PseudoClass::OnlyChild => {
                self.prev_element(node).is_none() && self.next_element(node).is_none()
            }
            PseudoClass::FirstOfType => self.index(node, false, &|n| self.same_type(n, node)) == 1,
            PseudoClass::LastOfType => self.index(node, true, &|n| self.same_type(n, node)) == 1,
            PseudoClass::OnlyOfType => {
                self.index(node, false, &|n| self.same_type(n, node)) == 1
                    && self.index(node, true, &|n| self.same_type(n, node)) == 1
            }
            PseudoClass::Root => self.doc.parent(node) == Some(self.doc.root()),
            PseudoClass::Empty => self
                .doc
                .children(node)
                .all(|c| match &self.doc.get(c).data {
                    NodeData::Element(_) => false,
                    // Selectors 4: se ignoran los nodos de texto con solo document white space.
                    NodeData::Text(t) | NodeData::CdataSection(t) => {
                        t.chars().all(is_document_whitespace)
                    }
                    _ => true,
                }),
            PseudoClass::AnyLink => is_link(el),
            PseudoClass::Link => is_link(el) && !self.ctx.visited_links.contains(&node),
            PseudoClass::Visited => is_link(el) && self.ctx.visited_links.contains(&node),
            PseudoClass::Hover => self.ctx.hover.contains(&node),
            PseudoClass::Active => self.ctx.active.contains(&node),
            PseudoClass::Focus => self.ctx.focus == Some(node),
            PseudoClass::FocusVisible => self.ctx.focus == Some(node) && self.ctx.focus_visible,
            PseudoClass::FocusWithin => self
                .ctx
                .focus
                .is_some_and(|f| self.doc.is_inclusive_ancestor(node, f)),
            PseudoClass::Target => self.ctx.target == Some(node),
            PseudoClass::Checked => {
                (is("input")
                    && el.attr("type").is_some_and(|t| {
                        t.eq_ignore_ascii_case("checkbox") || t.eq_ignore_ascii_case("radio")
                    })
                    && el.attr("checked").is_some())
                    || (is("option") && el.attr("selected").is_some())
            }
            PseudoClass::Disabled => self.is_disabled(node, el),
            PseudoClass::Enabled => {
                html && matches!(
                    el.name.as_lossy_str(),
                    "button" | "input" | "select" | "textarea" | "optgroup" | "option" | "fieldset"
                ) && !self.is_disabled(node, el)
            }
            PseudoClass::Required => {
                (is("input") || is("select") || is("textarea")) && el.attr("required").is_some()
            }
            PseudoClass::Optional => {
                (is("input") || is("select") || is("textarea")) && el.attr("required").is_none()
            }
            PseudoClass::ReadWrite => self.is_read_write(node, el),
            PseudoClass::ReadOnly => !self.is_read_write(node, el),
            PseudoClass::PlaceholderShown => {
                el.attr("placeholder").is_some()
                    && ((is("input") && el.attr("value").is_none_or(str::is_empty))
                        || (is("textarea") && self.doc.children(node).next().is_none()))
            }
            PseudoClass::Lang(ranges) => {
                let lang = self.content_language(node);
                ranges.iter().any(|r| lang_matches(r, lang.as_deref()))
            }
            PseudoClass::Dir(dir) => self.directionality(node) == *dir,
            PseudoClass::Scope => self.scope(node),
            // Sin custom element registry: los custom elements (nombre con '-') no están
            // definidos; el resto de los elementos sí.
            PseudoClass::Defined => !(html && el.name.contains('-')),
            PseudoClass::Heading(levels) => {
                let level = match el.name.as_lossy_str() {
                    "h1" => 1,
                    "h2" => 2,
                    "h3" => 3,
                    "h4" => 4,
                    "h5" => 5,
                    "h6" => 6,
                    _ => 0,
                };
                html && level > 0 && (levels.is_empty() || levels.contains(&level))
            }
        }
    }

    /// "Actually disabled" (HTML §4.16.3) para controles de formulario.
    fn is_disabled(&self, node: NodeId, el: &Element) -> bool {
        if el.ns != Namespace::Html {
            return false;
        }
        match el.name.as_lossy_str() {
            "button" | "input" | "select" | "textarea" | "fieldset" => {
                el.attr("disabled").is_some() || self.in_disabled_fieldset(node)
            }
            "optgroup" => el.attr("disabled").is_some(),
            "option" => {
                el.attr("disabled").is_some()
                    || self
                        .parent_element(node)
                        .and_then(|p| self.element(p))
                        .is_some_and(|p| p.is_html("optgroup") && p.attr("disabled").is_some())
            }
            _ => false,
        }
    }

    /// ¿Es descendiente de un `fieldset` deshabilitado, fuera de su primer `legend`?
    fn in_disabled_fieldset(&self, node: NodeId) -> bool {
        let mut child = node;
        let mut current = self.parent_element(node);
        while let Some(p) = current {
            if let Some(pel) = self.element(p)
                && pel.is_html("fieldset")
                && pel.attr("disabled").is_some()
            {
                let first_legend = self
                    .doc
                    .children(p)
                    .find(|&c| self.element(c).is_some_and(|e| e.is_html("legend")));
                if first_legend != Some(child) {
                    return true;
                }
            }
            child = p;
            current = self.parent_element(p);
        }
        false
    }

    fn is_read_write(&self, node: NodeId, el: &Element) -> bool {
        if el.is_html("input") {
            let ty = el.attr("type").unwrap_or("text").to_ascii_lowercase();
            let mutable_type = !matches!(
                ty.as_str(),
                "hidden"
                    | "checkbox"
                    | "radio"
                    | "file"
                    | "submit"
                    | "image"
                    | "reset"
                    | "button"
                    | "range"
                    | "color"
            );
            return mutable_type && el.attr("readonly").is_none() && !self.is_disabled(node, el);
        }
        if el.is_html("textarea") {
            return el.attr("readonly").is_none() && !self.is_disabled(node, el);
        }
        // Editing hosts y su contenido editable.
        let mut current = Some(node);
        while let Some(n) = current {
            if let Some(e) = self.element(n)
                && e.ns == Namespace::Html
                && let Some(v) = e.attr("contenteditable")
            {
                return v.is_empty()
                    || v.eq_ignore_ascii_case("true")
                    || v.eq_ignore_ascii_case("plaintext-only");
            }
            current = self.parent_element(n);
        }
        false
    }

    /// Content language: el `lang` (o `xml:lang`) más cercano. `Some("")` = no etiquetado.
    fn content_language(&self, node: NodeId) -> Option<String> {
        let mut current = Some(node);
        while let Some(n) = current {
            if let Some(el) = self.element(n) {
                if let Some(attr) = el
                    .attrs
                    .iter()
                    .find(|a| a.ns == Some(Namespace::Xml) && a.name == "lang")
                {
                    return Some(attr.value.to_string());
                }
                if el.ns == Namespace::Html
                    && let Some(v) = el.attr("lang")
                {
                    return Some(v.to_owned());
                }
            }
            current = self.parent_element(n);
        }
        None
    }

    /// Directionality simplificada: el `dir="ltr|rtl"` más cercano; `auto` y la ausencia de
    /// `dir` cuentan como ltr (todavía no hay análisis bidi del texto).
    fn directionality(&self, node: NodeId) -> Dir {
        let mut current = Some(node);
        while let Some(n) = current {
            if let Some(dir) = self.element(n).and_then(|e| e.attr("dir")) {
                if dir.eq_ignore_ascii_case("rtl") {
                    return Dir::Rtl;
                }
                if dir.eq_ignore_ascii_case("ltr") || dir.eq_ignore_ascii_case("auto") {
                    return Dir::Ltr;
                }
            }
            current = self.parent_element(n);
        }
        Dir::Ltr
    }
}

fn is_link(el: &Element) -> bool {
    (el.is_html("a") || el.is_html("area")) && el.attr("href").is_some()
}

/// ¿Existe n ≥ 0 con `a·n + b == index`?
fn nth(a: i32, b: i32, index: i64) -> bool {
    let (a, b) = (i64::from(a), i64::from(b));
    if a == 0 {
        index == b
    } else {
        let diff = index - b;
        diff % a == 0 && diff / a >= 0
    }
}

/// Extended filtering (RFC 4647 §3.3.2), ASCII case-insensitive. `lang` es `None` si el
/// elemento no tiene idioma conocido.
fn lang_matches(range: &str, lang: Option<&str>) -> bool {
    let lang = lang.unwrap_or("");
    if range.is_empty() {
        return lang.is_empty();
    }
    if lang.is_empty() {
        return false;
    }
    let range: Vec<String> = range.split('-').map(str::to_ascii_lowercase).collect();
    let tag: Vec<String> = lang.split('-').map(str::to_ascii_lowercase).collect();
    if range[0] != "*" && range[0] != tag[0] {
        return false;
    }
    let (mut i, mut j) = (1, 1);
    while i < range.len() {
        if range[i] == "*" {
            i += 1;
            continue;
        }
        if j >= tag.len() {
            return false;
        }
        if range[i] == tag[j] {
            i += 1;
            j += 1;
            continue;
        }
        if tag[j].len() == 1 {
            return false;
        }
        j += 1;
    }
    true
}
