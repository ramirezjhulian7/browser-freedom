//! Las hojas de estilo del CSSOM (CSSOM §6.1–§6.4): `StyleSheet`, `CSSStyleSheet`,
//! `StyleSheetList`, `MediaList`, `CSSRuleList` y las subclases de `CSSRule`.
//!
//! El [`SheetStore`] guarda el modelo de cada hoja (sus reglas con identidad estable). Las
//! hojas de `<style>` se sincronizan con el DOM al acceder: si el texto cambió, la hoja se
//! reemplaza por una nueva (CSSOM §6.1, "the associated CSS style sheet"). Los cambios
//! hechos desde el CSSOM (`insertRule`, `style.setProperty` de una regla…) marcan la hoja
//! como modificada y la cascada usa entonces su serialización.

use std::collections::HashMap;

use bf_css::media::{MediaContext, parse_media_query_list};
use bf_css::selectors::parse_selector_list;
use bf_css::stylesheet::{LayerRule, Origin, Rule, Stylesheet, component_values_to_string};
use bf_dom::{DomException, Namespace};
use bf_js::host::{HostClass, HostClassId};
use bf_js::value::Value;
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult};

use super::declaration::DeclBlock;
use super::style_decl::{DeclOwner, wrap as wrap_decl};
use super::value::{number, write_string};
use super::{KeyKind, classes, key, set_indexed, sheets_changed, state};
use crate::dom::{this_node, throw_dom, wrap_node};
use crate::{NodeRef, WebContextHandle, with_doc};

pub type SheetId = u32;
pub type RuleId = u32;

/// Una regla de una hoja.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuleRef {
    pub sheet: SheetId,
    pub rule: RuleId,
}

#[derive(Debug, Clone)]
pub struct CssRule {
    pub id: RuleId,
    pub kind: RuleKind,
}

#[derive(Debug, Clone)]
pub enum RuleKind {
    Style {
        selector: String,
        block: DeclBlock,
    },
    Media {
        media: String,
        rules: Vec<CssRule>,
    },
    Supports {
        condition: String,
        rules: Vec<CssRule>,
    },
    Import {
        href: String,
        media: String,
        layer: Option<String>,
        supports: Option<String>,
    },
    FontFace {
        block: DeclBlock,
    },
    Page {
        selector: String,
        block: DeclBlock,
    },
    Namespace {
        prefix: String,
        uri: String,
    },
    Keyframes {
        name: String,
        rules: Vec<CssRule>,
    },
    Keyframe {
        key: String,
        block: DeclBlock,
    },
    LayerBlock {
        name: String,
        rules: Vec<CssRule>,
    },
    LayerStatement {
        names: Vec<String>,
    },
}

impl RuleKind {
    fn children(&self) -> Option<&Vec<CssRule>> {
        match self {
            RuleKind::Media { rules, .. }
            | RuleKind::Supports { rules, .. }
            | RuleKind::Keyframes { rules, .. }
            | RuleKind::LayerBlock { rules, .. } => Some(rules),
            _ => None,
        }
    }

    fn children_mut(&mut self) -> Option<&mut Vec<CssRule>> {
        match self {
            RuleKind::Media { rules, .. }
            | RuleKind::Supports { rules, .. }
            | RuleKind::Keyframes { rules, .. }
            | RuleKind::LayerBlock { rules, .. } => Some(rules),
            _ => None,
        }
    }

    fn block_mut(&mut self) -> Option<&mut DeclBlock> {
        match self {
            RuleKind::Style { block, .. }
            | RuleKind::FontFace { block }
            | RuleKind::Page { block, .. }
            | RuleKind::Keyframe { block, .. } => Some(block),
            _ => None,
        }
    }
}

/// Una hoja de estilo.
#[derive(Debug, Clone, Default)]
pub struct Sheet {
    pub owner: Option<NodeRef>,
    /// El texto de la hoja de un `<style>` (para detectar cambios).
    pub source: Option<String>,
    pub href: Option<String>,
    pub title: Option<String>,
    pub media: String,
    pub disabled: bool,
    pub rules: Vec<CssRule>,
    pub modified: bool,
    pub constructed: bool,
}

/// Las hojas de una ventana.
#[derive(Debug, Default)]
pub struct SheetStore {
    pub sheets: HashMap<SheetId, Sheet>,
    by_owner: HashMap<NodeRef, SheetId>,
    next_sheet: SheetId,
    next_rule: RuleId,
    /// Las reglas quitadas de su hoja (sus wrappers siguen vivos).
    detached: HashMap<RuleId, CssRule>,
}

impl SheetStore {
    fn new_sheet(&mut self, sheet: Sheet) -> SheetId {
        self.next_sheet += 1;
        self.sheets.insert(self.next_sheet, sheet);
        self.next_sheet
    }

    /// Convierte las reglas parseadas de `bf-css` al modelo del CSSOM.
    fn convert(&mut self, rules: &[Rule]) -> Vec<CssRule> {
        let mut out = Vec::new();
        for r in rules {
            let kind = match r {
                Rule::Style(s) => RuleKind::Style {
                    selector: s.selectors.to_string(),
                    block: DeclBlock::from_declarations(&s.declarations),
                },
                Rule::Media(m) => RuleKind::Media {
                    media: media_text(&unrange(&m.queries.to_string())),
                    rules: self.convert(&m.rules),
                },
                Rule::Supports(s) => RuleKind::Supports {
                    condition: s.condition.to_string(),
                    rules: self.convert(&s.rules),
                },
                Rule::Import(i) => RuleKind::Import {
                    href: i.url.clone(),
                    media: i.media.to_string(),
                    layer: i.layer.as_ref().map(layer_name),
                    supports: i.supports.as_ref().map(ToString::to_string),
                },
                Rule::FontFace(d) => RuleKind::FontFace {
                    block: DeclBlock::from_descriptors(d),
                },
                Rule::Page(p) => RuleKind::Page {
                    selector: component_values_to_string(&p.selectors).trim().to_owned(),
                    block: DeclBlock::from_descriptors(&p.declarations),
                },
                Rule::Namespace(n) => RuleKind::Namespace {
                    prefix: n.prefix.clone().unwrap_or_default(),
                    uri: n.url.clone(),
                },
                Rule::Keyframes(k) => {
                    let mut frames = Vec::new();
                    for f in &k.keyframes {
                        self.next_rule += 1;
                        frames.push(CssRule {
                            id: self.next_rule,
                            kind: RuleKind::Keyframe {
                                key: keyframe_key(&f.offsets),
                                block: DeclBlock::from_declarations(&f.declarations),
                            },
                        });
                    }
                    RuleKind::Keyframes {
                        name: k.name.clone(),
                        rules: frames,
                    }
                }
                Rule::Layer(LayerRule::Block { name, rules }) => RuleKind::LayerBlock {
                    name: layer_name(name),
                    rules: self.convert(rules),
                },
                Rule::Layer(LayerRule::Statement(names)) => RuleKind::LayerStatement {
                    names: names.iter().map(layer_name).collect(),
                },
                Rule::Unknown(_) => continue,
            };
            self.next_rule += 1;
            out.push(CssRule {
                id: self.next_rule,
                kind,
            });
        }
        out
    }

    fn parse_rules(&mut self, text: &str) -> Vec<CssRule> {
        let sheet = Stylesheet::parse(text, Origin::Author);
        self.convert(&sheet.rules)
    }

    /// La regla `r` (en su hoja o entre las quitadas).
    pub fn rule(&self, r: RuleRef) -> Option<&CssRule> {
        self.sheets
            .get(&r.sheet)
            .and_then(|s| find(&s.rules, r.rule))
            .or_else(|| self.detached.get(&r.rule))
    }

    fn rule_mut(&mut self, r: RuleRef) -> Option<&mut CssRule> {
        let in_sheet = self
            .sheets
            .get(&r.sheet)
            .is_some_and(|s| find(&s.rules, r.rule).is_some());
        if in_sheet {
            let s = self.sheets.get_mut(&r.sheet)?;
            find_mut(&mut s.rules, r.rule)
        } else {
            self.detached.get_mut(&r.rule)
        }
    }

    /// ¿Sigue `r` en su hoja?
    fn attached(&self, r: RuleRef) -> bool {
        self.sheets
            .get(&r.sheet)
            .is_some_and(|s| find(&s.rules, r.rule).is_some())
    }

    /// La regla padre de `r` (`None` si está en el nivel superior o quitada).
    fn parent(&self, r: RuleRef) -> Option<RuleId> {
        let s = self.sheets.get(&r.sheet)?;
        parent_of(&s.rules, r.rule, None).flatten()
    }

    /// La lista de reglas de `owner`.
    fn list(&self, owner: ListOwner) -> Option<&Vec<CssRule>> {
        match owner {
            ListOwner::Sheet(s) => self.sheets.get(&s).map(|s| &s.rules),
            ListOwner::Rule(r) => self.rule(r).and_then(|r| r.kind.children()),
        }
    }

    fn list_mut(&mut self, owner: ListOwner) -> Option<&mut Vec<CssRule>> {
        match owner {
            ListOwner::Sheet(s) => self.sheets.get_mut(&s).map(|s| &mut s.rules),
            ListOwner::Rule(r) => self.rule_mut(r).and_then(|r| r.kind.children_mut()),
        }
    }

    fn mark_modified(&mut self, sheet: SheetId) {
        if let Some(s) = self.sheets.get_mut(&sheet) {
            s.modified = true;
        }
    }
}

fn find(rules: &[CssRule], id: RuleId) -> Option<&CssRule> {
    for r in rules {
        if r.id == id {
            return Some(r);
        }
        if let Some(c) = r.kind.children().and_then(|c| find(c, id)) {
            return Some(c);
        }
    }
    None
}

fn find_mut(rules: &mut [CssRule], id: RuleId) -> Option<&mut CssRule> {
    for r in rules {
        if r.id == id {
            return Some(r);
        }
        if let Some(c) = r.kind.children_mut().and_then(|c| find_mut(c, id)) {
            return Some(c);
        }
    }
    None
}

/// `Some(padre)` si `id` está en `rules` (con `padre` = la regla que contiene la lista).
fn parent_of(rules: &[CssRule], id: RuleId, parent: Option<RuleId>) -> Option<Option<RuleId>> {
    for r in rules {
        if r.id == id {
            return Some(parent);
        }
        if let Some(p) = r.kind.children().and_then(|c| parent_of(c, id, Some(r.id))) {
            return Some(p);
        }
    }
    None
}

fn layer_name(n: &bf_css::stylesheet::LayerName) -> String {
    if n.0.iter().any(|s| s.starts_with('\0')) {
        String::new()
    } else {
        n.to_string()
    }
}

fn keyframe_key(offsets: &[f64]) -> String {
    offsets
        .iter()
        .map(|o| format!("{}%", number(o * 100.0)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Normaliza una lista de media queries (`""` si está vacía).
fn media_text(text: &str) -> String {
    if text.trim().is_empty() {
        return String::new();
    }
    super::value::media_list_text(text)
}

/// `bf-css` guarda `min-width: X` como `width >= X`: se vuelve a la forma con prefijo (la que
/// escriben los autores y serializan los navegadores).
fn unrange(text: &str) -> String {
    let mut out = text.to_owned();
    for f in [
        "width",
        "height",
        "device-width",
        "device-height",
        "aspect-ratio",
        "device-aspect-ratio",
        "resolution",
        "color",
        "color-index",
        "monochrome",
    ] {
        for (op, prefix) in [(">=", "min"), ("<=", "max")] {
            let pat = format!("({f} {op} ");
            out = out.replace(&pat, &format!("({prefix}-{f}: "));
        }
    }
    out
}

// --- Serialización (CSSOM §6.4) ---

/// "Serialize a CSS rule".
pub fn rule_text(rule: &CssRule) -> String {
    let block = |selector: &str, b: &DeclBlock| {
        let t = b.css_text();
        if t.is_empty() {
            format!("{selector} {{ }}")
        } else {
            format!("{selector} {{ {t} }}")
        }
    };
    let group = |head: String, rules: &[CssRule]| {
        let mut s = format!("{head} {{");
        for r in rules {
            s.push_str("\n  ");
            s.push_str(&rule_text(r).replace('\n', "\n  "));
        }
        s.push_str("\n}");
        s
    };
    match &rule.kind {
        RuleKind::Style { selector, block: b } => block(selector, b),
        RuleKind::Media { media, rules } => group(format!("@media {media}"), rules),
        RuleKind::Supports { condition, rules } => group(format!("@supports {condition}"), rules),
        RuleKind::Import {
            href,
            media,
            layer,
            supports,
        } => {
            let mut s = String::from("@import url(");
            write_string(&mut s, href);
            s.push(')');
            match layer.as_deref() {
                Some("") => s.push_str(" layer"),
                Some(l) => s.push_str(&format!(" layer({l})")),
                None => {}
            }
            if let Some(c) = supports {
                s.push_str(&format!(" supports({c})"));
            }
            if !media.is_empty() {
                s.push(' ');
                s.push_str(media);
            }
            s.push(';');
            s
        }
        RuleKind::FontFace { block: b } => block("@font-face", b),
        RuleKind::Page { selector, block: b } => {
            if selector.is_empty() {
                block("@page", b)
            } else {
                block(&format!("@page {selector}"), b)
            }
        }
        RuleKind::Namespace { prefix, uri } => {
            let mut s = String::from("@namespace ");
            if !prefix.is_empty() {
                s.push_str(prefix);
                s.push(' ');
            }
            s.push_str("url(");
            write_string(&mut s, uri);
            s.push_str(");");
            s
        }
        RuleKind::Keyframes { name, rules } => group(format!("@keyframes {name}"), rules),
        RuleKind::Keyframe { key, block: b } => block(key, b),
        RuleKind::LayerBlock { name, rules } => {
            if name.is_empty() {
                group("@layer".into(), rules)
            } else {
                group(format!("@layer {name}"), rules)
            }
        }
        RuleKind::LayerStatement { names } => format!("@layer {};", names.join(", ")),
    }
}

fn sheet_text(sheet: &Sheet) -> String {
    sheet
        .rules
        .iter()
        .map(rule_text)
        .collect::<Vec<_>>()
        .join("\n")
}

// --- Sincronización con el DOM ---

fn is_css_type(t: Option<&str>) -> bool {
    t.is_none_or(|t| t.is_empty() || t.eq_ignore_ascii_case("text/css"))
}

/// Las hojas del documento de la ventana en orden de árbol (CSSOM §6.2, "document or shadow
/// root CSS style sheets"), creando o reemplazando las de los `<style>` cuyo texto cambió.
pub fn sync(agent: &mut Agent) -> Vec<SheetId> {
    struct Found {
        owner: NodeRef,
        source: Option<String>,
        href: Option<String>,
        title: Option<String>,
        media: String,
        alternate: bool,
    }
    let found: Vec<Found> = with_doc(agent, 0, |d| {
        let mut out = Vec::new();
        for node in d.inclusive_descendants(d.root()) {
            let Some(el) = d.element(node) else { continue };
            if el.ns != Namespace::Html {
                continue;
            }
            let media = el.attr("media").unwrap_or("").to_owned();
            let title = el.attr("title").map(str::to_owned);
            if el.is_html("style") {
                if !is_css_type(el.attr("type")) {
                    continue;
                }
                out.push(Found {
                    owner: NodeRef::new(0, node),
                    source: Some(d.child_text_content(node).as_lossy_str().to_string()),
                    href: None,
                    title,
                    media,
                    alternate: false,
                });
            } else if el.is_html("link") {
                let rel = el.attr("rel").unwrap_or("");
                let has = |t: &str| {
                    rel.split_ascii_whitespace()
                        .any(|x| x.eq_ignore_ascii_case(t))
                };
                if !has("stylesheet") || !is_css_type(el.attr("type")) {
                    continue;
                }
                let Some(href) = el.attr("href") else {
                    continue;
                };
                out.push(Found {
                    owner: NodeRef::new(0, node),
                    source: None,
                    href: Some(href.to_owned()),
                    title,
                    media,
                    alternate: has("alternate"),
                });
            }
        }
        out
    });
    let base = WebContextHandle::current(agent).get(agent).base.clone();
    let st = &mut state(agent).sheets;
    let mut ids = Vec::new();
    let mut live = std::collections::HashSet::new();
    for f in found {
        live.insert(f.owner);
        let href = f
            .href
            .map(|h| bf_infra::Url::parse(&h, Some(&base)).map_or(h, |u| u.serialize(false)));
        let reuse = st.by_owner.get(&f.owner).copied().filter(|id| {
            st.sheets
                .get(id)
                .is_some_and(|s| s.source == f.source && s.href == href)
        });
        let id = match reuse {
            Some(id) => id,
            None => {
                let rules = f
                    .source
                    .as_deref()
                    .map_or_else(Vec::new, |t| st.parse_rules(t));
                let id = st.new_sheet(Sheet {
                    owner: Some(f.owner),
                    source: f.source,
                    href,
                    rules,
                    disabled: f.alternate,
                    ..Sheet::default()
                });
                if let Some(old) = st.by_owner.insert(f.owner, id)
                    && let Some(s) = st.sheets.get_mut(&old)
                {
                    s.owner = None;
                }
                id
            }
        };
        if let Some(s) = st.sheets.get_mut(&id) {
            s.title = f.title;
            s.media = media_text(&f.media);
        }
        ids.push(id);
    }
    // Las hojas cuyo dueño salió del documento pierden el dueño.
    let gone: Vec<NodeRef> = st
        .by_owner
        .keys()
        .filter(|n| !live.contains(n))
        .copied()
        .collect();
    for n in gone {
        if let Some(id) = st.by_owner.remove(&n)
            && let Some(s) = st.sheets.get_mut(&id)
        {
            s.owner = None;
        }
    }
    ids
}

/// Las hojas de autor del documento para la cascada (con los cambios del CSSOM).
pub fn document_sheets(agent: &mut Agent, media: &MediaContext) -> Vec<Stylesheet> {
    let ids = sync(agent);
    let st = &state(agent).sheets;
    let mut out = Vec::new();
    for id in ids {
        let Some(s) = st.sheets.get(&id) else {
            continue;
        };
        if s.disabled || (!s.media.is_empty() && !parse_media_query_list(&s.media).evaluate(media))
        {
            continue;
        }
        let text = if s.modified {
            sheet_text(s)
        } else if let Some(src) = &s.source {
            src.clone()
        } else {
            continue;
        };
        out.push(Stylesheet::parse(&text, Origin::Author));
    }
    out
}

// --- Acceso desde las declaraciones ---

/// El bloque de declaraciones de una regla.
pub fn rule_block(agent: &mut Agent, r: RuleRef) -> Option<DeclBlock> {
    let st = &state(agent).sheets;
    match &st.rule(r)?.kind {
        RuleKind::Style { block, .. }
        | RuleKind::FontFace { block }
        | RuleKind::Page { block, .. }
        | RuleKind::Keyframe { block, .. } => Some(block.clone()),
        _ => None,
    }
}

/// Reemplaza el bloque de una regla.
pub fn set_rule_block(agent: &mut Agent, r: RuleRef, block: DeclBlock) {
    let st = &mut state(agent).sheets;
    if let Some(b) = st.rule_mut(r).and_then(|x| x.kind.block_mut()) {
        *b = block;
    }
    st.mark_modified(r.sheet);
}

// --- Clases ---

/// Los ids de las clases de reglas.
#[derive(Debug, Clone)]
pub struct RuleClasses {
    pub rule: HostClassId,
    pub style: HostClassId,
    pub grouping: HostClassId,
    pub condition: HostClassId,
    pub media: HostClassId,
    pub supports: HostClassId,
    pub import: HostClassId,
    pub font_face: HostClassId,
    pub page: HostClassId,
    pub namespace: HostClassId,
    pub keyframes: HostClassId,
    pub keyframe: HostClassId,
    pub layer_block: HostClassId,
    pub layer_statement: HostClassId,
}

/// El payload de `StyleSheet`/`CSSStyleSheet`.
#[derive(Debug, Clone, Copy)]
struct SheetPayload(SheetId);

/// El dueño de una `CSSRuleList`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListOwner {
    Sheet(SheetId),
    Rule(RuleRef),
}

/// El dueño de una `MediaList`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaOwner {
    Sheet(SheetId),
    Rule(RuleRef),
}

/// El payload de `StyleSheetList`.
#[derive(Debug, Clone, Copy)]
struct SheetListPayload;

pub fn register_classes(
    agent: &mut Agent,
) -> (
    HostClassId,
    HostClassId,
    HostClassId,
    HostClassId,
    HostClassId,
    RuleClasses,
) {
    let style_sheet = agent.register_host_class(
        HostClass::new("StyleSheet")
            .attribute("type", Some(sheet_type), None)
            .attribute("href", Some(sheet_href), None)
            .attribute("ownerNode", Some(sheet_owner_node), None)
            .attribute("parentStyleSheet", Some(sheet_parent), None)
            .attribute("title", Some(sheet_title), None)
            .attribute("media", Some(sheet_media), Some(set_sheet_media))
            .attribute("disabled", Some(sheet_disabled), Some(set_sheet_disabled)),
    );
    let css_style_sheet = agent.register_host_class(
        HostClass::new("CSSStyleSheet")
            .parent(style_sheet)
            .constructor(sheet_constructor, 0)
            .attribute("ownerRule", Some(sheet_owner_rule), None)
            .attribute("cssRules", Some(sheet_css_rules), None)
            .method("insertRule", 1, sheet_insert_rule)
            .method("deleteRule", 1, sheet_delete_rule)
            .method("replace", 1, sheet_replace)
            .method("replaceSync", 1, sheet_replace_sync)
            .attribute("rules", Some(sheet_css_rules), None)
            .method("addRule", 0, sheet_add_rule)
            .method("removeRule", 0, sheet_remove_rule),
    );
    let style_sheet_list = agent.register_host_class(
        HostClass::new("StyleSheetList")
            .method("item", 1, sheet_list_item)
            .attribute("length", Some(sheet_list_length), None),
    );
    let media_list = agent.register_host_class(
        HostClass::new("MediaList")
            .attribute("mediaText", Some(media_text_get), Some(media_text_set))
            .attribute("length", Some(media_length), None)
            .method("item", 1, media_item)
            .method("appendMedium", 1, media_append)
            .method("deleteMedium", 1, media_delete)
            .method("toString", 0, media_text_get),
    );
    let rule_list = agent.register_host_class(
        HostClass::new("CSSRuleList")
            .method("item", 1, rule_list_item)
            .attribute("length", Some(rule_list_length), None),
    );
    let rule = agent.register_host_class(
        HostClass::new("CSSRule")
            .attribute("cssText", Some(rule_css_text), Some(rule_set_css_text))
            .attribute("parentRule", Some(rule_parent_rule), None)
            .attribute("parentStyleSheet", Some(rule_parent_sheet), None)
            .attribute("type", Some(rule_type), None)
            .constant("STYLE_RULE", 1.0)
            .constant("CHARSET_RULE", 2.0)
            .constant("IMPORT_RULE", 3.0)
            .constant("MEDIA_RULE", 4.0)
            .constant("FONT_FACE_RULE", 5.0)
            .constant("PAGE_RULE", 6.0)
            .constant("KEYFRAMES_RULE", 7.0)
            .constant("KEYFRAME_RULE", 8.0)
            .constant("MARGIN_RULE", 9.0)
            .constant("NAMESPACE_RULE", 10.0)
            .constant("COUNTER_STYLE_RULE", 11.0)
            .constant("SUPPORTS_RULE", 12.0)
            .constant("FONT_FEATURE_VALUES_RULE", 14.0),
    );
    let style = agent.register_host_class(
        HostClass::new("CSSStyleRule")
            .parent(rule)
            .attribute(
                "selectorText",
                Some(rule_selector_text),
                Some(rule_set_selector_text),
            )
            .attribute("style", Some(rule_style), Some(rule_set_style)),
    );
    let grouping = agent.register_host_class(
        HostClass::new("CSSGroupingRule")
            .parent(rule)
            .attribute("cssRules", Some(rule_css_rules), None)
            .method("insertRule", 1, rule_insert_rule)
            .method("deleteRule", 1, rule_delete_rule),
    );
    let condition = agent.register_host_class(
        HostClass::new("CSSConditionRule")
            .parent(grouping)
            .attribute("conditionText", Some(rule_condition_text), None),
    );
    let media =
        agent.register_host_class(HostClass::new("CSSMediaRule").parent(condition).attribute(
            "media",
            Some(rule_media),
            Some(set_rule_media),
        ));
    let supports = agent.register_host_class(HostClass::new("CSSSupportsRule").parent(condition));
    let import = agent.register_host_class(
        HostClass::new("CSSImportRule")
            .parent(rule)
            .attribute("href", Some(import_href), None)
            .attribute("media", Some(rule_media), Some(set_rule_media))
            .attribute("styleSheet", Some(import_style_sheet), None)
            .attribute("layerName", Some(import_layer_name), None)
            .attribute("supportsText", Some(import_supports_text), None),
    );
    let font_face = agent.register_host_class(
        HostClass::new("CSSFontFaceRule")
            .parent(rule)
            .attribute("style", Some(rule_style), None),
    );
    let page = agent.register_host_class(
        HostClass::new("CSSPageRule")
            .parent(grouping)
            .attribute(
                "selectorText",
                Some(rule_selector_text),
                Some(rule_set_selector_text),
            )
            .attribute("style", Some(rule_style), Some(rule_set_style)),
    );
    let namespace = agent.register_host_class(
        HostClass::new("CSSNamespaceRule")
            .parent(rule)
            .attribute("namespaceURI", Some(namespace_uri), None)
            .attribute("prefix", Some(namespace_prefix), None),
    );
    let keyframes = agent.register_host_class(
        HostClass::new("CSSKeyframesRule")
            .parent(rule)
            .attribute("name", Some(keyframes_name), Some(set_keyframes_name))
            .attribute("cssRules", Some(rule_css_rules), None)
            .attribute("length", Some(keyframes_length), None)
            .method("findRule", 1, keyframes_find_rule)
            .method("appendRule", 1, keyframes_append_rule)
            .method("deleteRule", 1, keyframes_delete_rule),
    );
    let keyframe = agent.register_host_class(
        HostClass::new("CSSKeyframeRule")
            .parent(rule)
            .attribute(
                "keyText",
                Some(keyframe_key_text),
                Some(set_keyframe_key_text),
            )
            .attribute("style", Some(rule_style), Some(rule_set_style)),
    );
    let layer_block = agent.register_host_class(
        HostClass::new("CSSLayerBlockRule")
            .parent(grouping)
            .attribute("name", Some(layer_block_name), None),
    );
    let layer_statement = agent.register_host_class(
        HostClass::new("CSSLayerStatementRule")
            .parent(rule)
            .attribute("nameList", Some(layer_name_list), None),
    );
    (
        style_sheet,
        css_style_sheet,
        style_sheet_list,
        media_list,
        rule_list,
        RuleClasses {
            rule,
            style,
            grouping,
            condition,
            media,
            supports,
            import,
            font_face,
            page,
            namespace,
            keyframes,
            keyframe,
            layer_block,
            layer_statement,
        },
    )
}

/// `partial interface Document { readonly attribute StyleSheetList styleSheets; }`
/// (CSSOM §6.3, `DocumentOrShadowRoot`).
pub fn document_partial() -> HostClass {
    HostClass::new("Document").attribute("styleSheets", Some(document_style_sheets), None)
}

/// `LinkStyle.sheet` (CSSOM §6.3) para `<style>` y `<link>`.
pub fn element_partial() -> HostClass {
    HostClass::new("Element").attribute("sheet", Some(element_sheet), None)
}

// --- Wrappers ---

/// El wrapper de una hoja.
pub fn wrap_sheet(agent: &mut Agent, id: SheetId) -> Value {
    let class = classes(agent).css_style_sheet;
    let realm = agent.current_realm;
    Value::Object(agent.host_wrap(
        realm,
        key(KeyKind::Sheet, u64::from(id)),
        class,
        Box::new(SheetPayload(id)),
    ))
}

/// El wrapper de una regla.
pub fn wrap_rule(agent: &mut Agent, r: RuleRef) -> Value {
    let c = classes(agent).rules.clone();
    let class = match state(agent).sheets.rule(r).map(|x| &x.kind) {
        Some(RuleKind::Style { .. }) => c.style,
        Some(RuleKind::Media { .. }) => c.media,
        Some(RuleKind::Supports { .. }) => c.supports,
        Some(RuleKind::Import { .. }) => c.import,
        Some(RuleKind::FontFace { .. }) => c.font_face,
        Some(RuleKind::Page { .. }) => c.page,
        Some(RuleKind::Namespace { .. }) => c.namespace,
        Some(RuleKind::Keyframes { .. }) => c.keyframes,
        Some(RuleKind::Keyframe { .. }) => c.keyframe,
        Some(RuleKind::LayerBlock { .. }) => c.layer_block,
        Some(RuleKind::LayerStatement { .. }) => c.layer_statement,
        None => return Value::Null,
    };
    let realm = agent.current_realm;
    Value::Object(agent.host_wrap(
        realm,
        key(KeyKind::Rule, u64::from(r.rule)),
        class,
        Box::new(r),
    ))
}

fn list_key(owner: ListOwner) -> bf_js::host::HostKey {
    match owner {
        ListOwner::Sheet(s) => key(KeyKind::SheetRules, u64::from(s)),
        ListOwner::Rule(r) => key(KeyKind::RuleRules, u64::from(r.rule)),
    }
}

fn list_items(agent: &mut Agent, owner: ListOwner) -> Vec<Value> {
    let sheet = match owner {
        ListOwner::Sheet(s) => s,
        ListOwner::Rule(r) => r.sheet,
    };
    let ids: Vec<RuleId> = state(agent)
        .sheets
        .list(owner)
        .map(|l| l.iter().map(|r| r.id).collect())
        .unwrap_or_default();
    ids.into_iter()
        .map(|rule| wrap_rule(agent, RuleRef { sheet, rule }))
        .collect()
}

/// El wrapper de una `CSSRuleList` (con sus índices al día).
fn wrap_rule_list(agent: &mut Agent, owner: ListOwner) -> Value {
    let class = classes(agent).rule_list;
    let realm = agent.current_realm;
    let o = agent.host_wrap(realm, list_key(owner), class, Box::new(owner));
    let items = list_items(agent, owner);
    set_indexed(agent, o, &items);
    Value::Object(o)
}

/// Actualiza los índices de la `CSSRuleList` de `owner` si JavaScript la tiene.
fn refresh_rule_list(agent: &mut Agent, owner: ListOwner) {
    let realm = agent.current_realm;
    if let Some(o) = agent.host_lookup(realm, list_key(owner)) {
        let items = list_items(agent, owner);
        set_indexed(agent, o, &items);
    }
}

fn this_sheet(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<SheetId> {
    match agent.host_unwrap::<SheetPayload>(&a.this) {
        Some(s) => Ok(s.0),
        None => agent.type_error("Illegal invocation"),
    }
}

fn this_rule(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<RuleRef> {
    match agent.host_unwrap::<RuleRef>(&a.this) {
        Some(r) => Ok(*r),
        None => agent.type_error("Illegal invocation"),
    }
}

fn str_arg(agent: &mut Agent, v: &Value) -> JsResult<String> {
    Ok(agent.to_string(v)?.to_string())
}

fn opt_str(s: Option<&str>) -> Value {
    s.map_or(Value::Null, Value::str)
}

fn with_sheet<R>(agent: &mut Agent, id: SheetId, f: impl FnOnce(&Sheet) -> R) -> Option<R> {
    state(agent).sheets.sheets.get(&id).map(f)
}

// --- StyleSheet ---

fn sheet_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_sheet(agent, a)?;
    Ok(Value::str("text/css"))
}

fn sheet_href(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_sheet(agent, a)?;
    let h = with_sheet(agent, id, |s| s.href.clone()).flatten();
    Ok(opt_str(h.as_deref()))
}

fn sheet_owner_node(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_sheet(agent, a)?;
    sync(agent);
    match with_sheet(agent, id, |s| s.owner).flatten() {
        Some(n) => Ok(wrap_node(agent, n)),
        None => Ok(Value::Null),
    }
}

fn sheet_parent(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_sheet(agent, a)?;
    Ok(Value::Null)
}

fn sheet_title(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_sheet(agent, a)?;
    let t = with_sheet(agent, id, |s| s.title.clone()).flatten();
    Ok(opt_str(t.as_deref().filter(|t| !t.is_empty())))
}

fn wrap_media_list(agent: &mut Agent, owner: MediaOwner) -> Value {
    let class = classes(agent).media_list;
    let realm = agent.current_realm;
    let k = match owner {
        MediaOwner::Sheet(s) => key(KeyKind::SheetMedia, u64::from(s)),
        MediaOwner::Rule(r) => key(KeyKind::RuleMedia, u64::from(r.rule)),
    };
    let o = agent.host_wrap(realm, k, class, Box::new(owner));
    let items: Vec<Value> = media_items(agent, owner)
        .iter()
        .map(|m| Value::str(m))
        .collect();
    set_indexed(agent, o, &items);
    Value::Object(o)
}

fn sheet_media(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_sheet(agent, a)?;
    Ok(wrap_media_list(agent, MediaOwner::Sheet(id)))
}

/// `[PutForwards=mediaText]`.
fn set_sheet_media(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_sheet(agent, a)?;
    let text = str_arg(agent, &a.arg(0))?;
    set_media(agent, MediaOwner::Sheet(id), &text);
    Ok(Value::Undefined)
}

fn sheet_disabled(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_sheet(agent, a)?;
    Ok(Value::Boolean(
        with_sheet(agent, id, |s| s.disabled).unwrap_or(false),
    ))
}

fn set_sheet_disabled(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_sheet(agent, a)?;
    let v = bf_js::host::webidl::to_boolean(agent, &a.arg(0));
    if let Some(s) = state(agent).sheets.sheets.get_mut(&id)
        && s.disabled != v
    {
        s.disabled = v;
        sheets_changed(agent);
    }
    Ok(Value::Undefined)
}

// --- CSSStyleSheet ---

/// `new CSSStyleSheet(options)` (CSSOM §6.1.2, constructed style sheets).
fn sheet_constructor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Illegal constructor");
    };
    let opts = a.arg(0);
    let mut media = String::new();
    let mut disabled = false;
    if let Value::Object(o) = opts {
        let m = agent.get(o, &"media".into(), opts.clone())?;
        if !m.is_undefined() {
            media = media_text(&str_arg(agent, &m)?);
        }
        let d = agent.get(o, &"disabled".into(), opts.clone())?;
        disabled = bf_js::host::webidl::to_boolean(agent, &d);
    } else if !opts.is_nullish() {
        return agent.type_error("CSSStyleSheetInit must be an object");
    }
    let id = state(agent).sheets.new_sheet(Sheet {
        media,
        disabled,
        constructed: true,
        ..Sheet::default()
    });
    let class = classes(agent).css_style_sheet;
    let o = agent.host_create_from_new_target(
        nt,
        class,
        Some(key(KeyKind::Sheet, u64::from(id))),
        Box::new(SheetPayload(id)),
    )?;
    Ok(Value::Object(o))
}

fn sheet_owner_rule(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_sheet(agent, a)?;
    Ok(Value::Null)
}

fn sheet_css_rules(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_sheet(agent, a)?;
    Ok(wrap_rule_list(agent, ListOwner::Sheet(id)))
}

/// Parsea el texto de una regla para `insertRule` ("parse a CSS rule", CSSOM §6.6.3).
fn parse_one_rule(agent: &mut Agent, text: &str) -> Result<CssRule, DomException> {
    if bf_css::syntax::parse_rule(text).is_err() {
        return Err(DomException::SyntaxError);
    }
    let mut rules = state(agent).sheets.parse_rules(text);
    if rules.len() != 1 {
        return Err(DomException::SyntaxError);
    }
    Ok(rules.remove(0))
}

/// "Insert a CSS rule" (CSSOM §6.6.3).
fn insert_rule(
    agent: &mut Agent,
    owner: ListOwner,
    text: &str,
    index: usize,
    nested: bool,
) -> JsResult<Value> {
    let rule = match parse_one_rule(agent, text) {
        Ok(r) => r,
        Err(e) => return throw_dom(agent, e),
    };
    let sheet = match owner {
        ListOwner::Sheet(s) => s,
        ListOwner::Rule(r) => r.sheet,
    };
    let check = {
        let st = &state(agent).sheets;
        let Some(list) = st.list(owner) else {
            return throw_dom(agent, DomException::InvalidStateError);
        };
        if index > list.len() {
            Err(DomException::IndexSizeError)
        } else {
            constraint(list, &rule, index, nested)
        }
    };
    if let Err(e) = check {
        return throw_dom(agent, e);
    }
    let st = &mut state(agent).sheets;
    if let Some(list) = st.list_mut(owner) {
        list.insert(index, rule);
    }
    st.mark_modified(sheet);
    sheets_changed(agent);
    refresh_rule_list(agent, owner);
    Ok(Value::Number(index as f64))
}

/// Las restricciones de CSS al lugar de una regla (`@import` y `@namespace` primero; nada
/// de eso dentro de una regla de grupo).
fn constraint(
    list: &[CssRule],
    rule: &CssRule,
    index: usize,
    nested: bool,
) -> Result<(), DomException> {
    let is_import = |r: &CssRule| matches!(r.kind, RuleKind::Import { .. });
    let is_ns = |r: &CssRule| matches!(r.kind, RuleKind::Namespace { .. });
    let is_stmt = |r: &CssRule| matches!(r.kind, RuleKind::LayerStatement { .. });
    if nested && (is_import(rule) || is_ns(rule)) {
        return Err(DomException::HierarchyRequestError);
    }
    if is_import(rule) {
        if list[..index].iter().any(|r| !is_import(r) && !is_stmt(r)) {
            return Err(DomException::HierarchyRequestError);
        }
    } else if is_ns(rule) {
        if list
            .iter()
            .any(|r| !is_import(r) && !is_ns(r) && !is_stmt(r))
        {
            return Err(DomException::InvalidStateError);
        }
        if list[index..].iter().any(is_import) {
            return Err(DomException::HierarchyRequestError);
        }
    } else if list[index..].iter().any(|r| is_import(r) || is_ns(r)) {
        return Err(DomException::HierarchyRequestError);
    }
    Ok(())
}

/// "Remove a CSS rule" (CSSOM §6.6.3).
fn delete_rule(agent: &mut Agent, owner: ListOwner, index: usize) -> JsResult<Value> {
    let sheet = match owner {
        ListOwner::Sheet(s) => s,
        ListOwner::Rule(r) => r.sheet,
    };
    let check = {
        let st = &state(agent).sheets;
        match st.list(owner) {
            None => Err(DomException::InvalidStateError),
            Some(list) if index >= list.len() => Err(DomException::IndexSizeError),
            Some(list) => {
                let ns = matches!(list[index].kind, RuleKind::Namespace { .. });
                let others = list.iter().any(|r| {
                    !matches!(r.kind, RuleKind::Namespace { .. } | RuleKind::Import { .. })
                });
                if ns && others {
                    Err(DomException::InvalidStateError)
                } else {
                    Ok(())
                }
            }
        }
    };
    if let Err(e) = check {
        return throw_dom(agent, e);
    }
    let st = &mut state(agent).sheets;
    if let Some(list) = st.list_mut(owner) {
        let r = list.remove(index);
        st.detached.insert(r.id, r);
    }
    st.mark_modified(sheet);
    sheets_changed(agent);
    refresh_rule_list(agent, owner);
    Ok(Value::Undefined)
}

fn index_arg(agent: &mut Agent, v: &Value) -> JsResult<usize> {
    Ok(bf_js::host::webidl::to_unsigned_long(agent, v)? as usize)
}

fn sheet_insert_rule(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_sheet(agent, a)?;
    let text = str_arg(agent, &a.arg(0))?;
    let index = index_arg(agent, &a.arg(1))?;
    insert_rule(agent, ListOwner::Sheet(id), &text, index, false)
}

fn sheet_delete_rule(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_sheet(agent, a)?;
    let index = index_arg(agent, &a.arg(0))?;
    delete_rule(agent, ListOwner::Sheet(id), index)
}

/// `addRule(selector, style, index)` (CSSOM §6.1.2, legacy): siempre devuelve -1.
fn sheet_add_rule(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_sheet(agent, a)?;
    let selector = str_arg(agent, &a.arg(0))?;
    let style = str_arg(agent, &a.arg(1))?;
    let mut text = format!("{selector} {{ ");
    if !style.is_empty() {
        text.push_str(&style);
        text.push(' ');
    }
    text.push('}');
    let index = if a.arg(2).is_undefined() {
        with_sheet(agent, id, |s| s.rules.len()).unwrap_or(0)
    } else {
        index_arg(agent, &a.arg(2))?
    };
    insert_rule(agent, ListOwner::Sheet(id), &text, index, false)?;
    Ok(Value::Number(-1.0))
}

fn sheet_remove_rule(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_sheet(agent, a)?;
    let index = index_arg(agent, &a.arg(0))?;
    delete_rule(agent, ListOwner::Sheet(id), index)
}

/// `replaceSync(text)`: solo para hojas construidas; los `@import` se ignoran.
fn replace_rules(agent: &mut Agent, id: SheetId, text: &str) -> JsResult<()> {
    let constructed = with_sheet(agent, id, |s| s.constructed).unwrap_or(false);
    if !constructed {
        return throw_dom(agent, DomException::NotSupportedError);
    }
    let st = &mut state(agent).sheets;
    let mut rules = st.parse_rules(text);
    rules.retain(|r| !matches!(r.kind, RuleKind::Import { .. }));
    if let Some(s) = st.sheets.get_mut(&id) {
        s.rules = rules;
        s.modified = true;
    }
    sheets_changed(agent);
    refresh_rule_list(agent, ListOwner::Sheet(id));
    Ok(())
}

fn sheet_replace_sync(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_sheet(agent, a)?;
    let text = str_arg(agent, &a.arg(0))?;
    replace_rules(agent, id, &text)?;
    Ok(Value::Undefined)
}

fn sheet_replace(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = this_sheet(agent, a)?;
    let text = str_arg(agent, &a.arg(0))?;
    let promise = Value::Object(agent.intrinsics().promise_ctor);
    let cap = agent.new_promise_capability(&promise)?;
    match replace_rules(agent, id, &text) {
        Ok(()) => agent.capability_resolve(&cap, a.this.clone())?,
        Err(e) => {
            agent.if_abrupt_reject(e, &cap)?;
        }
    }
    Ok(cap.promise())
}

// --- StyleSheetList ---

fn document_style_sheets(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let class = classes(agent).style_sheet_list;
    let realm = agent.current_realm;
    let o = agent.host_wrap(
        realm,
        key(KeyKind::SheetList, u64::from(n.doc)),
        class,
        Box::new(SheetListPayload),
    );
    let items = sheet_list(agent, n.doc);
    set_indexed(agent, o, &items);
    Ok(Value::Object(o))
}

fn sheet_list(agent: &mut Agent, doc: u32) -> Vec<Value> {
    if doc != 0 {
        return Vec::new();
    }
    let ids = sync(agent);
    ids.into_iter().map(|id| wrap_sheet(agent, id)).collect()
}

fn sheet_list_item(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let i = index_arg(agent, &a.arg(0))?;
    if let Value::Object(o) = &a.this {
        let items = sheet_list(agent, 0);
        set_indexed(agent, *o, &items);
        return Ok(items.get(i).cloned().unwrap_or(Value::Null));
    }
    Ok(Value::Null)
}

fn sheet_list_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let items = sheet_list(agent, 0);
    if let Value::Object(o) = &a.this {
        set_indexed(agent, *o, &items);
    }
    Ok(Value::Number(items.len() as f64))
}

fn element_sheet(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let n = this_node(agent, a)?;
    let is_style = with_doc(agent, n.doc, |d| {
        d.element(n.node)
            .is_some_and(|e| e.is_html("style") || e.is_html("link"))
    });
    if !is_style || n.doc != 0 {
        return Ok(Value::Null);
    }
    sync(agent);
    let id = state(agent).sheets.by_owner.get(&n).copied();
    Ok(id.map_or(Value::Null, |id| wrap_sheet(agent, id)))
}

// --- MediaList (CSSOM §4.2) ---

fn this_media(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<MediaOwner> {
    match agent.host_unwrap::<MediaOwner>(&a.this) {
        Some(m) => Ok(*m),
        None => agent.type_error("Illegal invocation"),
    }
}

fn media_of(agent: &mut Agent, owner: MediaOwner) -> String {
    let st = &state(agent).sheets;
    match owner {
        MediaOwner::Sheet(s) => st.sheets.get(&s).map(|s| s.media.clone()),
        MediaOwner::Rule(r) => match st.rule(r).map(|x| &x.kind) {
            Some(RuleKind::Media { media, .. } | RuleKind::Import { media, .. }) => {
                Some(media.clone())
            }
            _ => None,
        },
    }
    .unwrap_or_default()
}

fn media_items(agent: &mut Agent, owner: MediaOwner) -> Vec<String> {
    let text = media_of(agent, owner);
    if text.is_empty() {
        return Vec::new();
    }
    text.split(", ").map(str::to_owned).collect()
}

fn set_media(agent: &mut Agent, owner: MediaOwner, text: &str) {
    let text = media_text(text);
    let st = &mut state(agent).sheets;
    match owner {
        MediaOwner::Sheet(s) => {
            if let Some(s) = st.sheets.get_mut(&s) {
                s.media = text;
            }
        }
        MediaOwner::Rule(r) => {
            if let Some(RuleKind::Media { media, .. } | RuleKind::Import { media, .. }) =
                st.rule_mut(r).map(|x| &mut x.kind)
            {
                *media = text;
            }
            st.mark_modified(r.sheet);
        }
    }
    sheets_changed(agent);
}

fn media_text_get(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let owner = this_media(agent, a)?;
    Ok(Value::str(&media_of(agent, owner)))
}

fn media_text_set(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let owner = this_media(agent, a)?;
    let text = if a.arg(0).is_null() {
        String::new()
    } else {
        str_arg(agent, &a.arg(0))?
    };
    set_media(agent, owner, &text);
    refresh_media(agent, a, owner);
    Ok(Value::Undefined)
}

fn refresh_media(agent: &mut Agent, a: &CallArgs<'_>, owner: MediaOwner) {
    if let Value::Object(o) = &a.this {
        let items: Vec<Value> = media_items(agent, owner)
            .iter()
            .map(|m| Value::str(m))
            .collect();
        set_indexed(agent, *o, &items);
    }
}

fn media_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let owner = this_media(agent, a)?;
    Ok(Value::Number(media_items(agent, owner).len() as f64))
}

fn media_item(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let owner = this_media(agent, a)?;
    let i = index_arg(agent, &a.arg(0))?;
    Ok(media_items(agent, owner)
        .get(i)
        .map_or(Value::Null, |m| Value::str(m)))
}

/// `appendMedium(medium)` (CSSOM §4.2).
fn media_append(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let owner = this_media(agent, a)?;
    let medium = str_arg(agent, &a.arg(0))?;
    let parsed = parse_media_query_list(&medium);
    if parsed.0.len() != 1 {
        return Ok(Value::Undefined);
    }
    let m = parsed.0[0].to_string();
    let mut items = media_items(agent, owner);
    if !items.contains(&m) {
        items.push(m);
        set_media(agent, owner, &items.join(", "));
        refresh_media(agent, a, owner);
    }
    Ok(Value::Undefined)
}

/// `deleteMedium(medium)` (CSSOM §4.2).
fn media_delete(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let owner = this_media(agent, a)?;
    let medium = str_arg(agent, &a.arg(0))?;
    let parsed = parse_media_query_list(&medium);
    if parsed.0.len() != 1 {
        return Ok(Value::Undefined);
    }
    let m = parsed.0[0].to_string();
    let mut items = media_items(agent, owner);
    let before = items.len();
    items.retain(|x| *x != m);
    if items.len() == before {
        return throw_dom(agent, DomException::NotFoundError);
    }
    set_media(agent, owner, &items.join(", "));
    refresh_media(agent, a, owner);
    Ok(Value::Undefined)
}

// --- CSSRuleList ---

fn this_list(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<ListOwner> {
    match agent.host_unwrap::<ListOwner>(&a.this) {
        Some(l) => Ok(*l),
        None => agent.type_error("Illegal invocation"),
    }
}

fn rule_list_item(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let owner = this_list(agent, a)?;
    let i = index_arg(agent, &a.arg(0))?;
    let items = list_items(agent, owner);
    Ok(items.get(i).cloned().unwrap_or(Value::Null))
}

fn rule_list_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let owner = this_list(agent, a)?;
    let n = state(agent).sheets.list(owner).map_or(0, Vec::len);
    Ok(Value::Number(n as f64))
}

// --- CSSRule ---

fn rule_css_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    let t = state(agent)
        .sheets
        .rule(r)
        .map(rule_text)
        .unwrap_or_default();
    Ok(Value::str(&t))
}

/// El setter de `cssText` no hace nada (CSSOM §6.4.1).
fn rule_set_css_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_rule(agent, a)?;
    Ok(Value::Undefined)
}

fn rule_parent_rule(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    match state(agent).sheets.parent(r) {
        Some(p) => Ok(wrap_rule(
            agent,
            RuleRef {
                sheet: r.sheet,
                rule: p,
            },
        )),
        None => Ok(Value::Null),
    }
}

fn rule_parent_sheet(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    if state(agent).sheets.attached(r) {
        Ok(wrap_sheet(agent, r.sheet))
    } else {
        Ok(Value::Null)
    }
}

fn rule_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    let t = match state(agent).sheets.rule(r).map(|x| &x.kind) {
        Some(RuleKind::Style { .. }) => 1,
        Some(RuleKind::Import { .. }) => 3,
        Some(RuleKind::Media { .. }) => 4,
        Some(RuleKind::FontFace { .. }) => 5,
        Some(RuleKind::Page { .. }) => 6,
        Some(RuleKind::Keyframes { .. }) => 7,
        Some(RuleKind::Keyframe { .. }) => 8,
        Some(RuleKind::Namespace { .. }) => 10,
        Some(RuleKind::Supports { .. }) => 12,
        _ => 0,
    };
    Ok(Value::Number(f64::from(t)))
}

fn rule_selector_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    let s = match state(agent).sheets.rule(r).map(|x| &x.kind) {
        Some(RuleKind::Style { selector, .. } | RuleKind::Page { selector, .. }) => {
            selector.clone()
        }
        _ => String::new(),
    };
    Ok(Value::str(&s))
}

/// El setter de `selectorText` (CSSOM §6.4.3): ignora los selectores inválidos.
fn rule_set_selector_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    let text = str_arg(agent, &a.arg(0))?;
    let is_page = matches!(
        state(agent).sheets.rule(r).map(|x| &x.kind),
        Some(RuleKind::Page { .. })
    );
    let new = if is_page {
        Some(text.trim().to_owned())
    } else {
        parse_selector_list(&text, &Default::default())
            .ok()
            .map(|l| l.to_string())
    };
    if let Some(new) = new {
        let st = &mut state(agent).sheets;
        if let Some(RuleKind::Style { selector, .. } | RuleKind::Page { selector, .. }) =
            st.rule_mut(r).map(|x| &mut x.kind)
        {
            *selector = new;
        }
        st.mark_modified(r.sheet);
        sheets_changed(agent);
    }
    Ok(Value::Undefined)
}

fn rule_style(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    Ok(wrap_decl(agent, DeclOwner::Rule(r)))
}

/// `[PutForwards=cssText]`.
fn rule_set_style(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    let text = str_arg(agent, &a.arg(0))?;
    set_rule_block(agent, r, DeclBlock::parse(&text));
    sheets_changed(agent);
    Ok(Value::Undefined)
}

fn rule_css_rules(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    Ok(wrap_rule_list(agent, ListOwner::Rule(r)))
}

fn rule_insert_rule(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    let text = str_arg(agent, &a.arg(0))?;
    let index = index_arg(agent, &a.arg(1))?;
    insert_rule(agent, ListOwner::Rule(r), &text, index, true)
}

fn rule_delete_rule(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    let index = index_arg(agent, &a.arg(0))?;
    delete_rule(agent, ListOwner::Rule(r), index)
}

fn rule_condition_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    let s = match state(agent).sheets.rule(r).map(|x| &x.kind) {
        Some(RuleKind::Media { media, .. }) => media.clone(),
        Some(RuleKind::Supports { condition, .. }) => condition.clone(),
        _ => String::new(),
    };
    Ok(Value::str(&s))
}

fn rule_media(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    Ok(wrap_media_list(agent, MediaOwner::Rule(r)))
}

fn set_rule_media(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    let text = str_arg(agent, &a.arg(0))?;
    set_media(agent, MediaOwner::Rule(r), &text);
    Ok(Value::Undefined)
}

fn import_field(agent: &mut Agent, a: &CallArgs<'_>, which: u8) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    let v = match state(agent).sheets.rule(r).map(|x| &x.kind) {
        Some(RuleKind::Import {
            href,
            layer,
            supports,
            ..
        }) => match which {
            0 => Some(href.clone()),
            1 => layer.clone(),
            _ => supports.clone(),
        },
        _ => None,
    };
    Ok(opt_str(v.as_deref()))
}

fn import_href(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    import_field(agent, a, 0)
}

fn import_layer_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    import_field(agent, a, 1)
}

fn import_supports_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    import_field(agent, a, 2)
}

/// Las hojas importadas no se cargan: `styleSheet` es `null`.
fn import_style_sheet(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_rule(agent, a)?;
    Ok(Value::Null)
}

fn namespace_field(agent: &mut Agent, a: &CallArgs<'_>, uri: bool) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    let v = match state(agent).sheets.rule(r).map(|x| &x.kind) {
        Some(RuleKind::Namespace { prefix, uri: u }) => {
            if uri {
                u.clone()
            } else {
                prefix.clone()
            }
        }
        _ => String::new(),
    };
    Ok(Value::str(&v))
}

fn namespace_uri(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    namespace_field(agent, a, true)
}

fn namespace_prefix(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    namespace_field(agent, a, false)
}

// --- @keyframes (CSS Animations §6) ---

fn keyframes_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    let v = match state(agent).sheets.rule(r).map(|x| &x.kind) {
        Some(RuleKind::Keyframes { name, .. }) => name.clone(),
        _ => String::new(),
    };
    Ok(Value::str(&v))
}

fn set_keyframes_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    let v = str_arg(agent, &a.arg(0))?;
    let st = &mut state(agent).sheets;
    if let Some(RuleKind::Keyframes { name, .. }) = st.rule_mut(r).map(|x| &mut x.kind) {
        *name = v;
    }
    st.mark_modified(r.sheet);
    sheets_changed(agent);
    Ok(Value::Undefined)
}

fn keyframes_length(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    let n = state(agent)
        .sheets
        .list(ListOwner::Rule(r))
        .map_or(0, Vec::len);
    Ok(Value::Number(n as f64))
}

/// La clave normalizada de un selector de keyframe (`from` → `0%`).
fn normalize_key(text: &str) -> String {
    text.split(',')
        .map(|k| match k.trim().to_ascii_lowercase().as_str() {
            "from" => "0%".to_owned(),
            "to" => "100%".to_owned(),
            other => {
                let n = other
                    .trim_end_matches('%')
                    .parse::<f64>()
                    .unwrap_or(f64::NAN);
                format!("{}%", number(n))
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn keyframes_find_rule(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    let k = normalize_key(&str_arg(agent, &a.arg(0))?);
    let found = state(agent).sheets.list(ListOwner::Rule(r)).and_then(|l| {
        l.iter()
            .rev()
            .find(|x| matches!(&x.kind, RuleKind::Keyframe { key, .. } if *key == k))
            .map(|x| x.id)
    });
    Ok(found.map_or(Value::Null, |id| {
        wrap_rule(
            agent,
            RuleRef {
                sheet: r.sheet,
                rule: id,
            },
        )
    }))
}

fn keyframes_append_rule(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    let text = str_arg(agent, &a.arg(0))?;
    let st = &mut state(agent).sheets;
    let mut parsed = st.parse_rules(&format!("@keyframes x {{ {text} }}"));
    let frame = match parsed.pop() {
        Some(CssRule {
            kind: RuleKind::Keyframes { mut rules, .. },
            ..
        }) if rules.len() == 1 => rules.remove(0),
        _ => return Ok(Value::Undefined),
    };
    if let Some(list) = st.list_mut(ListOwner::Rule(r)) {
        list.push(frame);
    }
    st.mark_modified(r.sheet);
    sheets_changed(agent);
    refresh_rule_list(agent, ListOwner::Rule(r));
    Ok(Value::Undefined)
}

fn keyframes_delete_rule(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    let k = normalize_key(&str_arg(agent, &a.arg(0))?);
    let st = &mut state(agent).sheets;
    if let Some(list) = st.list_mut(ListOwner::Rule(r))
        && let Some(i) = list
            .iter()
            .rposition(|x| matches!(&x.kind, RuleKind::Keyframe { key, .. } if *key == k))
    {
        let removed = list.remove(i);
        st.detached.insert(removed.id, removed);
        st.mark_modified(r.sheet);
        sheets_changed(agent);
        refresh_rule_list(agent, ListOwner::Rule(r));
    }
    Ok(Value::Undefined)
}

fn keyframe_key_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    let v = match state(agent).sheets.rule(r).map(|x| &x.kind) {
        Some(RuleKind::Keyframe { key, .. }) => key.clone(),
        _ => String::new(),
    };
    Ok(Value::str(&v))
}

fn set_keyframe_key_text(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    let text = str_arg(agent, &a.arg(0))?;
    let k = normalize_key(&text);
    if k.contains("NaN") {
        return throw_dom(agent, DomException::SyntaxError);
    }
    let st = &mut state(agent).sheets;
    if let Some(RuleKind::Keyframe { key, .. }) = st.rule_mut(r).map(|x| &mut x.kind) {
        *key = k;
    }
    st.mark_modified(r.sheet);
    sheets_changed(agent);
    Ok(Value::Undefined)
}

fn layer_block_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    let v = match state(agent).sheets.rule(r).map(|x| &x.kind) {
        Some(RuleKind::LayerBlock { name, .. }) => name.clone(),
        _ => String::new(),
    };
    Ok(Value::str(&v))
}

fn layer_name_list(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_rule(agent, a)?;
    let names = match state(agent).sheets.rule(r).map(|x| &x.kind) {
        Some(RuleKind::LayerStatement { names }) => names.clone(),
        _ => Vec::new(),
    };
    let vals: Vec<Value> = names.iter().map(|n| Value::str(n)).collect();
    let arr = agent.create_array_from_list(&vals)?;
    agent.set_integrity_level(arr, true)?;
    Ok(Value::Object(arr))
}
