//! Modelo de hoja de estilos: interpreta las reglas genéricas de CSS Syntax como style rules
//! y at-rules (`@media`, `@supports`, `@layer`, `@import`, `@namespace`, `@font-face`,
//! `@keyframes`, `@page`), des-anida CSS Nesting y recolecta las declaraciones que aplican a
//! un elemento. El orden de cascada se aplica en otro paso.

use std::collections::HashMap;
use std::fmt::Write as _;

use bf_dom::{Document, NodeId};

use crate::media::{MediaContext, MediaQueryList, parse_media_query_list_from};
use crate::selectors::{
    MatchContext, NamespaceMap, PseudoClass, PseudoElement, SelectorList, Simple, Specificity,
    matching_specificity, parse_relative_selector_list_from, parse_selector_list_from,
};
use crate::syntax::{
    self, AtRule, Block, BlockKind, ComponentValue, Declaration, NestedItem, TokenKind,
    parse_declaration_from, split_comma_separated,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Origin {
    UserAgent,
    User,
    Author,
}

/// Nombre completo de una cascade layer (`a.b` → `["a", "b"]`). Las capas anónimas usan un
/// segmento que empieza con U+0000, que no puede aparecer en un ident.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LayerName(pub Vec<String>);

impl LayerName {
    fn child(&self, segments: &[String]) -> LayerName {
        LayerName(self.0.iter().chain(segments).cloned().collect())
    }
}

impl std::fmt::Display for LayerName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<String> = self
            .0
            .iter()
            .map(|s| {
                s.strip_prefix('\0')
                    .map_or_else(|| s.clone(), |n| format!("<{n}>"))
            })
            .collect();
        f.write_str(&names.join("."))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
    pub origin: Origin,
    /// Capas en orden de primera aparición (incluidos los prefijos de los nombres anidados).
    pub layer_order: Vec<LayerName>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Rule {
    Style(StyleRule),
    Media(MediaRule),
    Supports(SupportsRule),
    Import(ImportRule),
    FontFace(Vec<Declaration>),
    Layer(LayerRule),
    Keyframes(KeyframesRule),
    Page(PageRule),
    Namespace(NamespaceRule),
    Unknown(AtRule),
}

#[derive(Debug, Clone, PartialEq)]
pub struct StyleRule {
    /// Selectores ya des-anidados: `&` reemplazado por `:is(<padre>)` y los relativos con
    /// `:is(<padre>)` antepuesto.
    pub selectors: SelectorList,
    pub declarations: Vec<Declaration>,
    pub nested: Vec<Rule>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaRule {
    pub queries: MediaQueryList,
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SupportsRule {
    pub condition: SupportsCondition,
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportRule {
    pub url: String,
    /// `None`: sin `layer`; `Some(name)`: capa con nombre o anónima.
    pub layer: Option<LayerName>,
    pub supports: Option<SupportsCondition>,
    pub media: MediaQueryList,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LayerRule {
    /// `@layer a, b.c;`: solo establece el orden.
    Statement(Vec<LayerName>),
    /// `@layer [name] { ... }`. `name` es el nombre completo.
    Block { name: LayerName, rules: Vec<Rule> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeyframesRule {
    pub name: String,
    pub keyframes: Vec<Keyframe>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Keyframe {
    /// Offsets entre 0 y 1 (`from` = 0, `to` = 1).
    pub offsets: Vec<f64>,
    pub declarations: Vec<Declaration>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PageRule {
    pub selectors: Vec<ComponentValue>,
    pub declarations: Vec<Declaration>,
    /// Margin rules (`@top-left { ... }`, ...).
    pub margins: Vec<(String, Vec<Declaration>)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NamespaceRule {
    pub prefix: Option<String>,
    pub url: String,
}

/// Condición de `@supports` (CSS Conditional 3/4).
#[derive(Debug, Clone, PartialEq)]
pub enum SupportsCondition {
    Not(Box<SupportsCondition>),
    And(Vec<SupportsCondition>),
    Or(Vec<SupportsCondition>),
    Declaration {
        name: String,
        value: Vec<ComponentValue>,
    },
    /// `selector(...)`: si el selector es válido.
    Selector(bool),
    /// `<general-enclosed>`: evalúa a false.
    Unknown,
}

/// Callback de soporte de propiedades: `(propiedad, valor) -> bool`.
pub type SupportsFn<'a> = &'a dyn Fn(&str, &[ComponentValue]) -> bool;

impl std::fmt::Display for SupportsCondition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let join = |f: &mut std::fmt::Formatter<'_>, cs: &[SupportsCondition], op: &str| {
            for (i, c) in cs.iter().enumerate() {
                if i > 0 {
                    write!(f, " {op} ")?;
                }
                write!(f, "{c}")?;
            }
            Ok(())
        };
        match self {
            SupportsCondition::Not(c) => write!(f, "not ({c})"),
            SupportsCondition::And(cs) => join(f, cs, "and"),
            SupportsCondition::Or(cs) => join(f, cs, "or"),
            SupportsCondition::Declaration { name, value } => {
                write!(f, "({name}: {})", component_values_to_string(value))
            }
            SupportsCondition::Selector(valid) => write!(
                f,
                "selector(<{}>)",
                if *valid { "válido" } else { "inválido" }
            ),
            SupportsCondition::Unknown => f.write_str("(<unknown>)"),
        }
    }
}

impl SupportsCondition {
    pub fn evaluate(&self, is_supported: SupportsFn<'_>) -> bool {
        match self {
            SupportsCondition::Not(c) => !c.evaluate(is_supported),
            SupportsCondition::And(cs) => cs.iter().all(|c| c.evaluate(is_supported)),
            SupportsCondition::Or(cs) => cs.iter().any(|c| c.evaluate(is_supported)),
            SupportsCondition::Declaration { name, value } => is_supported(name, value),
            SupportsCondition::Selector(valid) => *valid,
            SupportsCondition::Unknown => false,
        }
    }
}

impl Stylesheet {
    /// Parsea una hoja de estilos completa.
    pub fn parse(input: &str, origin: Origin) -> Stylesheet {
        let raw = syntax::parse_stylesheet(input);
        let mut converter = Converter {
            ns: NamespaceMap::default(),
            anonymous_layers: 0,
            layer_order: Vec::new(),
        };
        let rules = converter.top_level(raw);
        Stylesheet {
            rules,
            origin,
            layer_order: converter.layer_order,
        }
    }

    /// Recalcula `layer_order` recorriendo las reglas en orden, como lo hace el parser. Se usa
    /// después de insertar el contenido de hojas importadas.
    pub fn rebuild_layer_order(&mut self) {
        fn register(order: &mut Vec<LayerName>, name: &LayerName) {
            for len in 1..=name.0.len() {
                let prefix = LayerName(name.0[..len].to_vec());
                if !order.contains(&prefix) {
                    order.push(prefix);
                }
            }
        }
        fn walk(rules: &[Rule], order: &mut Vec<LayerName>) {
            for rule in rules {
                match rule {
                    Rule::Layer(LayerRule::Statement(names)) => {
                        for n in names {
                            register(order, n);
                        }
                    }
                    Rule::Layer(LayerRule::Block { name, rules }) => {
                        register(order, name);
                        walk(rules, order);
                    }
                    Rule::Import(i) => {
                        if let Some(l) = &i.layer {
                            register(order, l);
                        }
                    }
                    Rule::Media(m) => walk(&m.rules, order),
                    Rule::Supports(s) => walk(&s.rules, order),
                    Rule::Style(s) => walk(&s.nested, order),
                    _ => {}
                }
            }
        }
        let mut order = Vec::new();
        walk(&self.rules, &mut order);
        self.layer_order = order;
    }

    /// Reemplaza cada `<url>` de las declaraciones (salvo custom properties) por `f(url)`, si
    /// devuelve algo. Sirve para resolver URLs relativas contra la hoja que las contiene.
    pub fn rewrite_urls(&mut self, f: &mut dyn FnMut(&str) -> Option<String>) {
        fn values(vs: &mut [ComponentValue], f: &mut dyn FnMut(&str) -> Option<String>) {
            for v in vs {
                match v {
                    ComponentValue::Token(t) => {
                        if let TokenKind::Url(u) = &mut t.kind
                            && let Some(new) = f(u)
                        {
                            *u = new;
                        }
                    }
                    ComponentValue::Function(func) if func.name.eq_ignore_ascii_case("url") => {
                        for arg in &mut func.value {
                            if let ComponentValue::Token(t) = arg
                                && let TokenKind::String(u) = &mut t.kind
                                && let Some(new) = f(u)
                            {
                                *u = new;
                            }
                        }
                    }
                    ComponentValue::Function(func) => values(&mut func.value, f),
                    ComponentValue::Block(b) => values(&mut b.value, f),
                }
            }
        }
        fn declarations(ds: &mut [Declaration], f: &mut dyn FnMut(&str) -> Option<String>) {
            for d in ds.iter_mut().filter(|d| !d.name.starts_with("--")) {
                values(&mut d.value, f);
            }
        }
        fn rules(rs: &mut [Rule], f: &mut dyn FnMut(&str) -> Option<String>) {
            for r in rs {
                match r {
                    Rule::Style(s) => {
                        declarations(&mut s.declarations, f);
                        rules(&mut s.nested, f);
                    }
                    Rule::Media(m) => rules(&mut m.rules, f),
                    Rule::Supports(s) => rules(&mut s.rules, f),
                    Rule::Layer(LayerRule::Block { rules: inner, .. }) => rules(inner, f),
                    Rule::FontFace(ds) => declarations(ds, f),
                    Rule::Keyframes(k) => {
                        for kf in &mut k.keyframes {
                            declarations(&mut kf.declarations, f);
                        }
                    }
                    Rule::Page(p) => declarations(&mut p.declarations, f),
                    Rule::Import(i) => {
                        if let Some(new) = f(&i.url) {
                            i.url = new;
                        }
                    }
                    _ => {}
                }
            }
        }
        rules(&mut self.rules, f);
    }
}

// --- Conversión de reglas ---

#[derive(Clone, Copy)]
struct Ctx<'a> {
    /// Selector del style rule padre (ya des-anidado), si hay.
    parent: Option<&'a SelectorList>,
    layer: &'a LayerName,
}

struct Converter {
    ns: NamespaceMap,
    anonymous_layers: usize,
    layer_order: Vec<LayerName>,
}

fn non_ws(values: &[ComponentValue]) -> Vec<&ComponentValue> {
    values.iter().filter(|v| !v.is_whitespace()).collect()
}

fn ident_of(v: &ComponentValue) -> Option<&str> {
    match v.token() {
        Some(TokenKind::Ident(s)) => Some(s),
        _ => None,
    }
}

/// `<string>` o `<url>` (incluido `url("...")`).
fn url_of(v: &ComponentValue) -> Option<String> {
    match v {
        ComponentValue::Token(t) => match &t.kind {
            TokenKind::String(s) | TokenKind::Url(s) => Some(s.clone()),
            _ => None,
        },
        ComponentValue::Function(f) if f.name.eq_ignore_ascii_case("url") => {
            match non_ws(&f.value).as_slice() {
                [arg] => match arg.token() {
                    Some(TokenKind::String(s)) => Some(s.clone()),
                    _ => None,
                },
                _ => None,
            }
        }
        _ => None,
    }
}

/// `<layer-name>` = `<ident> [ '.' <ident> ]*`, sin whitespace.
fn parse_layer_name(values: &[ComponentValue]) -> Option<Vec<String>> {
    let values: Vec<&ComponentValue> = {
        let start = values.iter().position(|v| !v.is_whitespace())?;
        let end = values.iter().rposition(|v| !v.is_whitespace())?;
        values[start..=end].iter().collect()
    };
    let mut segments = Vec::new();
    let mut expect_ident = true;
    for v in values {
        match (expect_ident, v.token()) {
            (true, Some(TokenKind::Ident(s))) => segments.push(s.clone()),
            (false, Some(TokenKind::Delim('.'))) => {}
            _ => return None,
        }
        expect_ident = !expect_ident;
    }
    (!expect_ident).then_some(segments)
}

impl Converter {
    fn register_layer(&mut self, name: &LayerName) {
        for len in 1..=name.0.len() {
            let prefix = LayerName(name.0[..len].to_vec());
            if !self.layer_order.contains(&prefix) {
                self.layer_order.push(prefix);
            }
        }
    }

    fn anonymous_layer(&mut self) -> String {
        self.anonymous_layers += 1;
        format!("\0anonymous-{}", self.anonymous_layers)
    }

    fn top_level(&mut self, raw: Vec<syntax::Rule>) -> Vec<Rule> {
        let root = LayerName(Vec::new());
        let ctx = Ctx {
            parent: None,
            layer: &root,
        };
        let mut rules = Vec::new();
        let (mut imports_allowed, mut namespaces_allowed) = (true, true);
        for rule in raw {
            let name = match &rule {
                syntax::Rule::At(at) => at.name.to_ascii_lowercase(),
                syntax::Rule::Qualified(_) => String::new(),
            };
            let is_layer_statement =
                matches!(&rule, syntax::Rule::At(at) if name == "layer" && at.block.is_none());
            match (name.as_str(), &rule) {
                ("charset", _) => continue,
                ("import", syntax::Rule::At(at)) => {
                    if imports_allowed && let Some(import) = self.import(at) {
                        rules.push(Rule::Import(import));
                    }
                    continue;
                }
                ("namespace", syntax::Rule::At(at)) => {
                    imports_allowed = false;
                    if namespaces_allowed && let Some(ns) = self.namespace(at) {
                        rules.push(Rule::Namespace(ns));
                    }
                    continue;
                }
                _ => {}
            }
            if !is_layer_statement {
                imports_allowed = false;
                namespaces_allowed = false;
            }
            if let Some(converted) = self.rule(&rule, ctx) {
                rules.push(converted);
            }
        }
        rules
    }

    fn rule(&mut self, rule: &syntax::Rule, ctx: Ctx<'_>) -> Option<Rule> {
        match rule {
            syntax::Rule::Qualified(q) => {
                let selectors = match ctx.parent {
                    None => parse_selector_list_from(&q.prelude, &self.ns).ok()?,
                    Some(parent) => {
                        let mut list =
                            parse_relative_selector_list_from(&q.prelude, &self.ns).ok()?;
                        resolve_nested(&mut list, parent);
                        list
                    }
                };
                Some(Rule::Style(self.style_rule(selectors, &q.block, ctx)))
            }
            syntax::Rule::At(at) => self.at_rule(at, ctx),
        }
    }

    fn style_rule(&mut self, selectors: SelectorList, block: &Block, ctx: Ctx<'_>) -> StyleRule {
        let mut nested = Vec::new();
        {
            let inner = Ctx {
                parent: Some(&selectors),
                layer: ctx.layer,
            };
            for item in &block.contents.rules {
                match item {
                    NestedItem::Rule(r) => nested.extend(self.rule(r, inner)),
                    // Nested declarations rule: mismos elementos que el padre.
                    NestedItem::Declarations(decls) => nested.push(Rule::Style(StyleRule {
                        selectors: selectors.clone(),
                        declarations: decls.clone(),
                        nested: Vec::new(),
                    })),
                }
            }
        }
        StyleRule {
            declarations: block.contents.declarations.clone(),
            selectors,
            nested,
        }
    }

    /// Contenido de una group rule (`@media`, `@supports`, `@layer`). Dentro de un style rule
    /// las declaraciones se aplican al selector padre; a nivel superior son inválidas.
    fn group_contents(&mut self, block: &Block, ctx: Ctx<'_>) -> Vec<Rule> {
        let mut rules = Vec::new();
        let decls_rule = |decls: &[Declaration]| {
            ctx.parent.map(|parent| {
                Rule::Style(StyleRule {
                    selectors: parent.clone(),
                    declarations: decls.to_vec(),
                    nested: Vec::new(),
                })
            })
        };
        if !block.contents.declarations.is_empty() {
            rules.extend(decls_rule(&block.contents.declarations));
        }
        for item in &block.contents.rules {
            match item {
                NestedItem::Rule(r) => rules.extend(self.rule(r, ctx)),
                NestedItem::Declarations(decls) => rules.extend(decls_rule(decls)),
            }
        }
        rules
    }

    fn at_rule(&mut self, at: &AtRule, ctx: Ctx<'_>) -> Option<Rule> {
        let name = at.name.to_ascii_lowercase();
        match name.as_str() {
            "media" => {
                let block = at.block.as_ref()?;
                Some(Rule::Media(MediaRule {
                    queries: parse_media_query_list_from(&at.prelude),
                    rules: self.group_contents(block, ctx),
                }))
            }
            "supports" => {
                let block = at.block.as_ref()?;
                Some(Rule::Supports(SupportsRule {
                    condition: self.supports_condition(&at.prelude)?,
                    rules: self.group_contents(block, ctx),
                }))
            }
            "layer" => match &at.block {
                None => {
                    let names = split_comma_separated(&at.prelude)
                        .iter()
                        .map(|group| parse_layer_name(group).map(|n| ctx.layer.child(&n)))
                        .collect::<Option<Vec<_>>>()?;
                    for n in &names {
                        self.register_layer(n);
                    }
                    Some(Rule::Layer(LayerRule::Statement(names)))
                }
                Some(block) => {
                    let segments = if non_ws(&at.prelude).is_empty() {
                        vec![self.anonymous_layer()]
                    } else {
                        parse_layer_name(&at.prelude)?
                    };
                    let full = ctx.layer.child(&segments);
                    self.register_layer(&full);
                    let inner = Ctx {
                        parent: ctx.parent,
                        layer: &full,
                    };
                    let rules = self.group_contents(block, inner);
                    Some(Rule::Layer(LayerRule::Block { name: full, rules }))
                }
            },
            "font-face" => {
                let block = at.block.as_ref()?;
                Some(Rule::FontFace(all_declarations(block)))
            }
            "keyframes" | "-webkit-keyframes" => {
                let block = at.block.as_ref()?;
                let name = match non_ws(&at.prelude).as_slice() {
                    [v] => match v.token()? {
                        TokenKind::Ident(s) | TokenKind::String(s) => s.clone(),
                        _ => return None,
                    },
                    _ => return None,
                };
                let keyframes = block
                    .contents
                    .rules
                    .iter()
                    .filter_map(|item| match item {
                        NestedItem::Rule(syntax::Rule::Qualified(q)) => keyframe(q),
                        _ => None,
                    })
                    .collect();
                Some(Rule::Keyframes(KeyframesRule { name, keyframes }))
            }
            "page" => {
                let block = at.block.as_ref()?;
                let margins = block
                    .contents
                    .rules
                    .iter()
                    .filter_map(|item| match item {
                        NestedItem::Rule(syntax::Rule::At(m)) => m
                            .block
                            .as_ref()
                            .map(|b| (m.name.to_ascii_lowercase(), all_declarations(b))),
                        _ => None,
                    })
                    .collect();
                let mut declarations = block.contents.declarations.clone();
                for item in &block.contents.rules {
                    if let NestedItem::Declarations(d) = item {
                        declarations.extend(d.iter().cloned());
                    }
                }
                Some(Rule::Page(PageRule {
                    selectors: at.prelude.clone(),
                    declarations,
                    margins,
                }))
            }
            // Solo válidas al principio de la hoja (ver `top_level`).
            "import" | "namespace" | "charset" => None,
            _ => Some(Rule::Unknown(at.clone())),
        }
    }

    fn import(&mut self, at: &AtRule) -> Option<ImportRule> {
        if at.block.is_some() {
            return None;
        }
        let start = at.prelude.iter().position(|v| !v.is_whitespace())?;
        let url = url_of(&at.prelude[start])?;
        let mut rest = &at.prelude[start + 1..];
        let skip_ws = |r: &mut &[ComponentValue]| {
            while r.first().is_some_and(ComponentValue::is_whitespace) {
                *r = &r[1..];
            }
        };
        skip_ws(&mut rest);
        let mut layer = None;
        match rest.first() {
            Some(v) if ident_of(v).is_some_and(|s| s.eq_ignore_ascii_case("layer")) => {
                layer = Some(LayerName(vec![self.anonymous_layer()]));
                rest = &rest[1..];
            }
            Some(ComponentValue::Function(f)) if f.name.eq_ignore_ascii_case("layer") => {
                layer = Some(LayerName(parse_layer_name(&f.value)?));
                rest = &rest[1..];
            }
            _ => {}
        }
        if let Some(l) = &layer {
            self.register_layer(l);
        }
        skip_ws(&mut rest);
        let mut supports = None;
        if let Some(ComponentValue::Function(f)) = rest.first()
            && f.name.eq_ignore_ascii_case("supports")
        {
            // `supports( <supports-condition> | <declaration> )`.
            supports = Some(self.supports_condition(&f.value).or_else(|| {
                let decl = parse_declaration_from(&f.value).ok()?;
                Some(SupportsCondition::Declaration {
                    name: decl.name,
                    value: decl.value,
                })
            })?);
            rest = &rest[1..];
        }
        Some(ImportRule {
            url,
            layer,
            supports,
            media: parse_media_query_list_from(rest),
        })
    }

    fn namespace(&mut self, at: &AtRule) -> Option<NamespaceRule> {
        if at.block.is_some() {
            return None;
        }
        let rule = match non_ws(&at.prelude).as_slice() {
            [url] => NamespaceRule {
                prefix: None,
                url: url_of(url)?,
            },
            [prefix, url] => NamespaceRule {
                prefix: Some(ident_of(prefix)?.to_owned()),
                url: url_of(url)?,
            },
            _ => return None,
        };
        match &rule.prefix {
            None => self.ns.default = Some(rule.url.clone()),
            Some(p) => {
                self.ns.prefixes.insert(p.clone(), rule.url.clone());
            }
        }
        Some(rule)
    }

    /// `<supports-condition>`.
    fn supports_condition(&self, values: &[ComponentValue]) -> Option<SupportsCondition> {
        let items = non_ws(values);
        let keyword =
            |v: &ComponentValue, k: &str| ident_of(v).is_some_and(|s| s.eq_ignore_ascii_case(k));
        let first = *items.first()?;
        if keyword(first, "not") {
            return match items.as_slice() {
                [_, operand] => Some(SupportsCondition::Not(Box::new(
                    self.supports_in_parens(operand)?,
                ))),
                _ => None,
            };
        }
        let mut conditions = vec![self.supports_in_parens(first)?];
        let mut op: Option<String> = None;
        let mut rest = &items[1..];
        while let [kw, operand, tail @ ..] = rest {
            let kw = ident_of(kw)?.to_ascii_lowercase();
            if !matches!(kw.as_str(), "and" | "or") || op.as_ref().is_some_and(|o| *o != kw) {
                return None;
            }
            op = Some(kw);
            conditions.push(self.supports_in_parens(operand)?);
            rest = tail;
        }
        if !rest.is_empty() {
            return None;
        }
        Some(match op.as_deref() {
            None => conditions.pop().expect("una condición"),
            Some("and") => SupportsCondition::And(conditions),
            Some(_) => SupportsCondition::Or(conditions),
        })
    }

    /// `<supports-in-parens>`.
    fn supports_in_parens(&self, value: &ComponentValue) -> Option<SupportsCondition> {
        match value {
            ComponentValue::Block(b) if b.kind == BlockKind::Paren => {
                if let Some(c) = self.supports_condition(&b.value) {
                    return Some(c);
                }
                Some(match parse_declaration_from(&b.value) {
                    Ok(decl) => SupportsCondition::Declaration {
                        name: decl.name,
                        value: decl.value,
                    },
                    Err(_) => SupportsCondition::Unknown,
                })
            }
            ComponentValue::Function(f) if f.name.eq_ignore_ascii_case("selector") => {
                Some(SupportsCondition::Selector(
                    parse_selector_list_from(&f.value, &self.ns).is_ok_and(|l| l.0.len() == 1),
                ))
            }
            ComponentValue::Function(_) => Some(SupportsCondition::Unknown),
            _ => None,
        }
    }
}

/// Todas las declaraciones de un bloque (para reglas que solo admiten declaraciones).
fn all_declarations(block: &Block) -> Vec<Declaration> {
    let mut decls = block.contents.declarations.clone();
    for item in &block.contents.rules {
        if let NestedItem::Declarations(d) = item {
            decls.extend(d.iter().cloned());
        }
    }
    decls
}

fn keyframe(q: &syntax::QualifiedRule) -> Option<Keyframe> {
    let offsets = split_comma_separated(&q.prelude)
        .iter()
        .map(|group| match non_ws(group).as_slice() {
            [v] => match v.token()? {
                TokenKind::Ident(s) if s.eq_ignore_ascii_case("from") => Some(0.0),
                TokenKind::Ident(s) if s.eq_ignore_ascii_case("to") => Some(1.0),
                TokenKind::Percentage(p) if (0.0..=100.0).contains(&p.value) => {
                    Some(p.value / 100.0)
                }
                _ => None,
            },
            _ => None,
        })
        .collect::<Option<Vec<f64>>>()?;
    Some(Keyframe {
        offsets,
        declarations: all_declarations(&q.block),
    })
}

/// Des-anida un selector relativo: antepone `:is(<padre>)` a los relativos y reemplaza `&`
/// por `:is(<padre>)` en el resto.
fn resolve_nested(list: &mut SelectorList, parent: &SelectorList) {
    let parent_is = Simple::PseudoClass(PseudoClass::Is(SelectorList(
        parent
            .0
            .iter()
            .filter(|s| s.pseudo_element.is_none())
            .cloned()
            .collect(),
    )));
    for selector in &mut list.0 {
        // Un `&` explícito (también junto a un combinator inicial) representa al padre.
        selector.replace_nesting(parent);
        if let Some(combinator) = selector.leading.take() {
            selector.compounds.insert(0, vec![parent_is.clone()]);
            selector.combinators.insert(0, combinator);
        }
    }
}

// --- Recolección de declaraciones ---

/// Una declaración que aplica al elemento, con todo lo necesario para ordenarla en la cascada.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchedDeclaration<'a> {
    pub origin: Origin,
    /// Posición de la capa en el orden de capas del origen (de menor a mayor precedencia
    /// para declaraciones normales). Las declaraciones sin capa tienen el índice más alto.
    pub layer_index: usize,
    pub specificity: Specificity,
    /// Orden de aparición entre todas las hojas.
    pub source_order: usize,
    pub important: bool,
    pub declaration: &'a Declaration,
}

/// Recolecta las declaraciones de `sheets` (en orden) que aplican a `node` (o a su
/// pseudo-elemento `pseudo`), resolviendo `@media`, `@supports` y `@layer`. `@import` solo se
/// registra: la carga de hojas importadas es de otro paso.
pub fn matching_declarations<'a>(
    doc: &Document,
    node: NodeId,
    pseudo: Option<PseudoElement>,
    sheets: &[&'a Stylesheet],
    media: &MediaContext,
    supports: SupportsFn<'_>,
    ctx: &MatchContext,
) -> Vec<MatchedDeclaration<'a>> {
    let layer_indexes = layer_indexes(sheets);
    let mut collector = Collector {
        doc,
        node,
        pseudo,
        media,
        supports,
        ctx,
        out: Vec::new(),
        source_order: 0,
    };
    for sheet in sheets {
        let indexes = &layer_indexes[&sheet.origin];
        collector.rules(&sheet.rules, sheet.origin, &LayerName(Vec::new()), indexes);
    }
    collector.out
}

/// Orden de capas por origen: post-orden del árbol de capas (subcapas antes que los estilos
/// directos de su capa padre); la raíz (estilos sin capa) queda última.
fn layer_indexes(sheets: &[&Stylesheet]) -> HashMap<Origin, HashMap<LayerName, usize>> {
    let mut per_origin: HashMap<Origin, Vec<LayerName>> = HashMap::new();
    for sheet in sheets {
        let order = per_origin.entry(sheet.origin).or_default();
        for name in &sheet.layer_order {
            if !order.contains(name) {
                order.push(name.clone());
            }
        }
    }
    let mut result = HashMap::new();
    for sheet in sheets {
        let order = per_origin.get(&sheet.origin).cloned().unwrap_or_default();
        result.entry(sheet.origin).or_insert_with(|| {
            let mut flat = Vec::new();
            post_order(&LayerName(Vec::new()), &order, &mut flat);
            flat.into_iter().enumerate().map(|(i, n)| (n, i)).collect()
        });
    }
    result
}

fn post_order(node: &LayerName, order: &[LayerName], out: &mut Vec<LayerName>) {
    for child in order
        .iter()
        .filter(|n| n.0.len() == node.0.len() + 1 && n.0.starts_with(&node.0))
    {
        post_order(child, order, out);
    }
    out.push(node.clone());
}

struct Collector<'a, 'd> {
    doc: &'d Document,
    node: NodeId,
    pseudo: Option<PseudoElement>,
    media: &'d MediaContext,
    supports: SupportsFn<'d>,
    ctx: &'d MatchContext,
    out: Vec<MatchedDeclaration<'a>>,
    source_order: usize,
}

impl<'a> Collector<'a, '_> {
    fn rules(
        &mut self,
        rules: &'a [Rule],
        origin: Origin,
        layer: &LayerName,
        indexes: &HashMap<LayerName, usize>,
    ) {
        for rule in rules {
            match rule {
                Rule::Style(style) => {
                    if let Some(specificity) = matching_specificity(
                        &style.selectors,
                        self.doc,
                        self.node,
                        self.pseudo,
                        self.ctx,
                    ) {
                        let layer_index = indexes.get(layer).copied().unwrap_or(usize::MAX);
                        for declaration in &style.declarations {
                            self.out.push(MatchedDeclaration {
                                origin,
                                layer_index,
                                specificity,
                                source_order: self.source_order,
                                important: declaration.important,
                                declaration,
                            });
                            self.source_order += 1;
                        }
                    } else {
                        self.source_order += style.declarations.len();
                    }
                    self.rules(&style.nested, origin, layer, indexes);
                }
                Rule::Media(m) => {
                    if m.queries.evaluate(self.media) {
                        self.rules(&m.rules, origin, layer, indexes);
                    }
                }
                Rule::Supports(s) => {
                    if s.condition.evaluate(self.supports) {
                        self.rules(&s.rules, origin, layer, indexes);
                    }
                }
                Rule::Layer(LayerRule::Block { name, rules }) => {
                    self.rules(rules, origin, name, indexes);
                }
                _ => {}
            }
        }
    }
}

// --- Volcado legible (bf-shell dump-css) ---

/// Serialización aproximada de component values (para volcados).
pub fn component_values_to_string(values: &[ComponentValue]) -> String {
    let mut out = String::new();
    for v in values {
        match v {
            ComponentValue::Token(t) => {
                let _ = write!(out, "{}", t.kind);
            }
            ComponentValue::Function(f) => {
                let _ = write!(out, "{}({})", f.name, component_values_to_string(&f.value));
            }
            ComponentValue::Block(b) => {
                let (open, close) = match b.kind {
                    BlockKind::Curly => ('{', '}'),
                    BlockKind::Square => ('[', ']'),
                    BlockKind::Paren => ('(', ')'),
                };
                let _ = write!(out, "{open}{}{close}", component_values_to_string(&b.value));
            }
        }
    }
    out
}

impl Stylesheet {
    /// Árbol de reglas legible: selectores con su especificidad, at-rules y declaraciones.
    pub fn dump(&self) -> String {
        let mut out = String::new();
        if !self.layer_order.is_empty() {
            let names: Vec<String> = self.layer_order.iter().map(ToString::to_string).collect();
            let _ = writeln!(out, "layer order: {}", names.join(", "));
        }
        dump_rules(&self.rules, 0, &mut out);
        out
    }
}

fn dump_declarations(decls: &[Declaration], depth: usize, out: &mut String) {
    for d in decls {
        let _ = writeln!(
            out,
            "{:indent$}{}: {}{}",
            "",
            d.name,
            component_values_to_string(&d.value),
            if d.important { " !important" } else { "" },
            indent = depth * 2
        );
    }
}

fn dump_rules(rules: &[Rule], depth: usize, out: &mut String) {
    let pad = "  ".repeat(depth);
    for rule in rules {
        match rule {
            Rule::Style(s) => {
                let specs: Vec<String> = s
                    .selectors
                    .0
                    .iter()
                    .map(|c| c.specificity().to_string())
                    .collect();
                let _ = writeln!(out, "{pad}style {}  {}", s.selectors, specs.join(" "));
                dump_declarations(&s.declarations, depth + 1, out);
                dump_rules(&s.nested, depth + 1, out);
            }
            Rule::Media(m) => {
                let _ = writeln!(out, "{pad}@media {}", m.queries);
                dump_rules(&m.rules, depth + 1, out);
            }
            Rule::Supports(s) => {
                let _ = writeln!(out, "{pad}@supports {}", s.condition);
                dump_rules(&s.rules, depth + 1, out);
            }
            Rule::Import(i) => {
                let _ = writeln!(
                    out,
                    "{pad}@import {:?} layer={} supports={} media={}",
                    i.url,
                    i.layer
                        .as_ref()
                        .map_or_else(|| "-".to_owned(), ToString::to_string),
                    i.supports
                        .as_ref()
                        .map_or_else(|| "-".to_owned(), ToString::to_string),
                    i.media
                );
            }
            Rule::FontFace(decls) => {
                let _ = writeln!(out, "{pad}@font-face");
                dump_declarations(decls, depth + 1, out);
            }
            Rule::Layer(LayerRule::Statement(names)) => {
                let names: Vec<String> = names.iter().map(ToString::to_string).collect();
                let _ = writeln!(out, "{pad}@layer {};", names.join(", "));
            }
            Rule::Layer(LayerRule::Block { name, rules }) => {
                let _ = writeln!(out, "{pad}@layer {name}");
                dump_rules(rules, depth + 1, out);
            }
            Rule::Keyframes(k) => {
                let _ = writeln!(out, "{pad}@keyframes {}", k.name);
                for frame in &k.keyframes {
                    let offsets: Vec<String> = frame
                        .offsets
                        .iter()
                        .map(|o| format!("{}%", o * 100.0))
                        .collect();
                    let _ = writeln!(out, "{pad}  {}", offsets.join(", "));
                    dump_declarations(&frame.declarations, depth + 2, out);
                }
            }
            Rule::Page(p) => {
                let _ = writeln!(
                    out,
                    "{pad}@page {}",
                    component_values_to_string(&p.selectors).trim()
                );
                dump_declarations(&p.declarations, depth + 1, out);
                for (name, decls) in &p.margins {
                    let _ = writeln!(out, "{pad}  @{name}");
                    dump_declarations(decls, depth + 2, out);
                }
            }
            Rule::Namespace(n) => {
                let _ = writeln!(
                    out,
                    "{pad}@namespace {} {:?}",
                    n.prefix.as_deref().unwrap_or("(default)"),
                    n.url
                );
            }
            Rule::Unknown(at) => {
                let _ = writeln!(
                    out,
                    "{pad}@{} {} (desconocida{})",
                    at.name,
                    component_values_to_string(&at.prelude).trim(),
                    if at.block.is_some() {
                        ", con bloque"
                    } else {
                        ""
                    }
                );
            }
        }
    }
}
