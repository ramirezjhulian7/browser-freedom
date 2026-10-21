//! El modelo de un bloque de declaraciones CSS (CSSOM §6.7): la lista ordenada de
//! declaraciones de longhands y custom properties, con los algoritmos de `setProperty`,
//! `removeProperty`, `getPropertyValue` y la serialización ("serialize a CSS declaration
//! block", con la compresión en shorthands).

use std::collections::HashSet;

use bf_css::syntax::{
    ComponentValue, Declaration, TokenKind, parse_block_contents, parse_component_value_list,
};
use bf_style::is_supported;
use bf_style::properties::shorthands::{SHORTHANDS, ShorthandDef};
use bf_style::properties::{CssWide, LonghandId};
use bf_style::values::trim;

use super::value::{Property, is_alias_shorthand, property, serialize_raw, serialize_value};

/// Una declaración: el nombre (longhand en minúsculas o custom property), el valor
/// serializado y la importancia.
#[derive(Debug, Clone, PartialEq)]
pub struct Decl {
    pub name: String,
    pub value: String,
    pub important: bool,
}

/// Un bloque de declaraciones CSS (CSSOM §6.7, "CSS declaration block").
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeclBlock {
    pub decls: Vec<Decl>,
}

/// El texto del valor inicial especificado de un longhand (para omitirlo al serializar
/// shorthands).
fn initial_text(id: LonghandId) -> String {
    let def = id.def();
    let tokens = parse_component_value_list(def.initial);
    match (def.parse)(trim(&tokens)) {
        Some(v) => (def.to_css)(&v),
        None => def.initial.to_owned(),
    }
}

/// ¿Es `values` un valor aceptable para una declaración (CSSOM "parse a CSS value")?
/// Rechaza `;`, `!important`, tokens malos y cierres sueltos.
fn well_formed(values: &[ComponentValue]) -> bool {
    values.iter().all(|v| {
        !matches!(
            v.token(),
            Some(
                TokenKind::Semicolon
                    | TokenKind::BadString
                    | TokenKind::BadUrl
                    | TokenKind::RightBrace
                    | TokenKind::RightParen
                    | TokenKind::RightBracket
                    | TokenKind::Delim('!')
            )
        )
    })
}

/// Las declaraciones de longhands en que se expande `name: values` (vacío si es inválida).
pub fn expand(
    name: &str,
    values: &[ComponentValue],
    original: Option<&str>,
) -> Vec<(String, String)> {
    let values = trim(values);
    if values.is_empty() && !super::value::is_custom(name) {
        return Vec::new();
    }
    match property(name) {
        Some(Property::Custom) => {
            let text = match original {
                Some(t) => t.trim().to_owned(),
                None => serialize_raw(values),
            };
            vec![(name.to_owned(), text)]
        }
        Some(Property::Longhand(id)) => {
            if !is_supported(id.name(), values) {
                return Vec::new();
            }
            vec![(id.name().to_owned(), serialize_value(id.name(), values))]
        }
        Some(Property::Shorthand(sh)) => expand_shorthand(sh, values),
        None => Vec::new(),
    }
}

fn expand_shorthand(sh: &ShorthandDef, values: &[ComponentValue]) -> Vec<(String, String)> {
    if CssWide::parse(values).is_some() || bf_style::cascade::contains_var(values) {
        if !is_supported(sh.name, values) {
            return Vec::new();
        }
        let text = serialize_value(sh.name, values);
        return sh
            .longhands
            .iter()
            .map(|l| (l.name().to_owned(), text.clone()))
            .collect();
    }
    let Some(expanded) = (sh.expand)(values) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = sh
        .longhands
        .iter()
        .map(|l| (l.name().to_owned(), initial_text(*l)))
        .collect();
    for v in expanded {
        let id = v.id();
        let text = (id.def().to_css)(&v);
        match out.iter_mut().find(|(n, _)| n == id.name()) {
            Some(slot) => slot.1 = text,
            None => out.push((id.name().to_owned(), text)),
        }
    }
    out
}

impl DeclBlock {
    /// "Parse a CSS declaration block" (CSSOM §6.7.2) de un atributo `style` o `cssText`.
    pub fn parse(text: &str) -> DeclBlock {
        let contents = parse_block_contents(text);
        let mut block = DeclBlock::default();
        let mut all: Vec<&Declaration> = contents.declarations.iter().collect();
        for item in &contents.rules {
            if let bf_css::syntax::NestedItem::Declarations(d) = item {
                all.extend(d.iter());
            }
        }
        for d in all {
            block.add_declaration(d);
        }
        block
    }

    /// Un bloque desde declaraciones ya parseadas (las de una regla de estilo).
    pub fn from_declarations(decls: &[Declaration]) -> DeclBlock {
        let mut block = DeclBlock::default();
        for d in decls {
            block.add_declaration(d);
        }
        block
    }

    /// Un bloque de descriptores (`@font-face`, `@page`): sin validar contra las propiedades.
    pub fn from_descriptors(decls: &[Declaration]) -> DeclBlock {
        let mut block = DeclBlock::default();
        for d in decls {
            let name = normalize_name(&d.name);
            let value = serialize_value(&name, &d.value);
            block.decls.retain(|x| x.name != name);
            block.decls.push(Decl {
                name,
                value,
                important: d.important,
            });
        }
        block
    }

    fn add_declaration(&mut self, d: &Declaration) {
        let name = if super::value::is_custom(&d.name) {
            d.name.clone()
        } else {
            d.name.to_ascii_lowercase()
        };
        for (n, v) in expand(&name, &d.value, d.original_text.as_deref()) {
            // Al parsear, la última declaración gana pero una importante no la pisa una
            // normal (CSS Syntax: las declaraciones duplicadas se quedan con la última).
            if let Some(i) = self.decls.iter().position(|x| x.name == n) {
                if self.decls[i].important && !d.important {
                    continue;
                }
                self.decls.remove(i);
            }
            self.decls.push(Decl {
                name: n,
                value: v,
                important: d.important,
            });
        }
    }

    fn find(&self, name: &str) -> Option<&Decl> {
        self.decls.iter().find(|d| d.name == name)
    }

    /// "Set a CSS declaration" (CSSOM §6.7.2): `true` si cambió algo.
    pub fn set_declaration(&mut self, name: &str, value: String, important: bool) -> bool {
        if let Some(d) = self.decls.iter_mut().find(|d| d.name == name) {
            if d.value == value && d.important == important {
                return false;
            }
            d.value = value;
            d.important = important;
            return true;
        }
        self.decls.push(Decl {
            name: name.to_owned(),
            value,
            important,
        });
        true
    }

    /// `setProperty(property, value, priority)` (CSSOM §6.7.3), sin el chequeo de solo
    /// lectura: `true` si el bloque cambió.
    pub fn set_property(&mut self, property_name: &str, value: &str, priority: &str) -> bool {
        let name = normalize_name(property_name);
        if property(&name).is_none() {
            return false;
        }
        if value.is_empty() {
            return self.remove_property(&name).1;
        }
        let important = if priority.is_empty() {
            false
        } else if priority.eq_ignore_ascii_case("important") {
            true
        } else {
            return false;
        };
        let tokens = parse_component_value_list(value);
        if !well_formed(&tokens) {
            return false;
        }
        let original = super::value::is_custom(&name).then_some(value);
        let decls = expand(&name, &tokens, original);
        if decls.is_empty() {
            return false;
        }
        let mut updated = false;
        for (n, v) in decls {
            updated |= self.set_declaration(&n, v, important);
        }
        updated
    }

    /// `removeProperty(property)` (CSSOM §6.7.3): el valor anterior y si cambió.
    pub fn remove_property(&mut self, property_name: &str) -> (String, bool) {
        let name = normalize_name(property_name);
        let value = self.get_property_value(&name);
        let before = self.decls.len();
        match property(&name) {
            Some(Property::Shorthand(sh)) => {
                self.decls
                    .retain(|d| !sh.longhands.iter().any(|l| l.name() == d.name));
            }
            _ => self.decls.retain(|d| d.name != name),
        }
        (value, self.decls.len() != before)
    }

    /// `getPropertyValue(property)` (CSSOM §6.7.3).
    pub fn get_property_value(&self, property_name: &str) -> String {
        let name = normalize_name(property_name);
        match property(&name) {
            Some(Property::Shorthand(sh)) => {
                let Some(decls) = self.longhands_of(sh) else {
                    return String::new();
                };
                if decls.iter().any(|d| d.important != decls[0].important) {
                    return String::new();
                }
                shorthand_value(sh, &decls)
            }
            _ => self
                .find(&name)
                .map(|d| d.value.clone())
                .unwrap_or_default(),
        }
    }

    /// `getPropertyPriority(property)` (CSSOM §6.7.3).
    pub fn get_property_priority(&self, property_name: &str) -> &'static str {
        let name = normalize_name(property_name);
        let important = match property(&name) {
            Some(Property::Shorthand(sh)) => self
                .longhands_of(sh)
                .is_some_and(|ds| ds.iter().all(|d| d.important)),
            _ => self.find(&name).is_some_and(|d| d.important),
        };
        if important { "important" } else { "" }
    }

    /// Las declaraciones de todos los longhands de `sh`, si están todas.
    fn longhands_of(&self, sh: &ShorthandDef) -> Option<Vec<&Decl>> {
        sh.longhands.iter().map(|l| self.find(l.name())).collect()
    }

    /// "Serialize a CSS declaration block" (CSSOM §6.7.2).
    pub fn css_text(&self) -> String {
        let mut shorthands: Vec<&ShorthandDef> = SHORTHANDS
            .iter()
            .filter(|s| s.longhands.len() > 1 && !is_alias_shorthand(s.name))
            .collect();
        shorthands.sort_by_key(|s| std::cmp::Reverse(s.longhands.len()));
        let mut list: Vec<String> = Vec::new();
        let mut done: HashSet<&str> = HashSet::new();
        'decls: for d in &self.decls {
            if done.contains(d.name.as_str()) {
                continue;
            }
            for sh in shorthands
                .iter()
                .filter(|s| s.longhands.iter().any(|l| l.name() == d.name))
            {
                let Some(decls) = self.longhands_of(sh) else {
                    continue;
                };
                if decls
                    .iter()
                    .any(|x| done.contains(x.name.as_str()) || x.important != d.important)
                {
                    continue;
                }
                let value = shorthand_value(sh, &decls);
                if value.is_empty() {
                    continue;
                }
                list.push(declaration_text(sh.name, &value, d.important));
                for x in decls {
                    done.insert(x.name.as_str());
                }
                continue 'decls;
            }
            list.push(declaration_text(&d.name, &d.value, d.important));
            done.insert(d.name.as_str());
        }
        list.join(" ")
    }

    /// Los nombres de las declaraciones (para `item()` y `length`).
    pub fn names(&self) -> Vec<String> {
        self.decls.iter().map(|d| d.name.clone()).collect()
    }
}

/// "Serialize a CSS declaration" (CSSOM §6.7.2).
fn declaration_text(name: &str, value: &str, important: bool) -> String {
    let mut s = format!("{name}: {value}");
    if important {
        s.push_str(" !important");
    }
    s.push(';');
    s
}

/// El nombre de propiedad normalizado: en minúsculas salvo las custom properties.
pub fn normalize_name(name: &str) -> String {
    if super::value::is_custom(name) {
        name.to_owned()
    } else {
        name.to_ascii_lowercase()
    }
}

/// Cuatro valores de caja en su forma más corta (`1px`, `1px 2px`, …).
fn box4(v: [&str; 4]) -> String {
    let [t, r, b, l] = v;
    if r == l {
        if t == b {
            if t == r {
                return t.to_owned();
            }
            return format!("{t} {r}");
        }
        return format!("{t} {r} {b}");
    }
    format!("{t} {r} {b} {l}")
}

/// El valor de un shorthand a partir de sus longhands (todos presentes, misma importancia):
/// vacío si no se puede representar.
fn shorthand_value(sh: &ShorthandDef, decls: &[&Decl]) -> String {
    let vals: Vec<&str> = decls.iter().map(|d| d.value.as_str()).collect();
    let is_wide = |v: &str| {
        matches!(
            v,
            "initial" | "inherit" | "unset" | "revert" | "revert-layer"
        )
    };
    if vals.iter().any(|v| is_wide(v)) {
        return if vals.iter().all(|v| *v == vals[0]) {
            vals[0].to_owned()
        } else {
            String::new()
        };
    }
    // Un shorthand con var(): todos los longhands guardan el mismo texto.
    if vals.iter().all(|v| *v == vals[0]) && vals[0].contains("var(") {
        return vals[0].to_owned();
    }
    if vals.iter().any(|v| v.contains("var(")) {
        return String::new();
    }
    let get = |name: &str| -> &str {
        decls
            .iter()
            .find(|d| d.name == name)
            .map_or("", |d| d.value.as_str())
    };
    let non_initial = || -> Vec<&str> {
        decls
            .iter()
            .zip(sh.longhands.iter())
            .filter(|(d, l)| d.value != initial_text(**l))
            .map(|(d, _)| d.value.as_str())
            .collect()
    };
    match sh.name {
        "margin" | "padding" | "inset" | "border-width" | "border-style" | "border-color" => {
            if vals.len() == 4 {
                box4([vals[0], vals[1], vals[2], vals[3]])
            } else {
                String::new()
            }
        }
        "border-radius" => {
            if vals.len() != 4 {
                return String::new();
            }
            let split = |v: &str| -> (String, String) {
                let mut it = v.split(' ');
                let h = it.next().unwrap_or("").to_owned();
                let w = it.next().map_or_else(|| h.clone(), str::to_owned);
                (h, w)
            };
            let parts: Vec<(String, String)> = vals.iter().map(|v| split(v)).collect();
            let h = box4([&parts[0].0, &parts[1].0, &parts[2].0, &parts[3].0]);
            let v = box4([&parts[0].1, &parts[1].1, &parts[2].1, &parts[3].1]);
            if h == v { h } else { format!("{h} / {v}") }
        }
        "overflow" | "gap" | "place-content" | "place-items" | "place-self" => {
            if vals.len() == 2 && vals[0] == vals[1] {
                vals[0].to_owned()
            } else {
                vals.join(" ")
            }
        }
        "border" => {
            // Solo si los cuatro lados son iguales.
            let sides = ["top", "right", "bottom", "left"];
            let side = |s: &str| {
                (
                    get(&format!("border-{s}-width")).to_owned(),
                    get(&format!("border-{s}-style")).to_owned(),
                    get(&format!("border-{s}-color")).to_owned(),
                )
            };
            let first = side("top");
            if sides.iter().any(|s| side(s) != first) {
                return String::new();
            }
            let defs: Vec<(&str, String)> = vec![
                (first.0.as_str(), initial_text(LonghandId::BorderTopWidth)),
                (first.1.as_str(), initial_text(LonghandId::BorderTopStyle)),
                (first.2.as_str(), initial_text(LonghandId::BorderTopColor)),
            ];
            let parts: Vec<&str> = defs
                .iter()
                .filter(|(v, i)| v != i)
                .map(|(v, _)| *v)
                .collect();
            if parts.is_empty() {
                first.1.clone()
            } else {
                parts.join(" ")
            }
        }
        "flex" => format!(
            "{} {} {}",
            get("flex-grow"),
            get("flex-shrink"),
            get("flex-basis")
        ),
        "font" => {
            let mut parts = Vec::new();
            for n in ["font-style", "font-weight"] {
                let v = get(n);
                if v != "normal" && !v.is_empty() {
                    parts.push(v.to_owned());
                }
            }
            let lh = get("line-height");
            if lh == "normal" || lh.is_empty() {
                parts.push(get("font-size").to_owned());
            } else {
                parts.push(format!("{}/{lh}", get("font-size")));
            }
            parts.push(get("font-family").to_owned());
            parts.join(" ")
        }
        "background" => {
            let mut parts: Vec<String> = Vec::new();
            let init = |n: &str| match bf_style::properties::longhand_by_name(n) {
                Some(id) => initial_text(id),
                None => String::new(),
            };
            let push = |parts: &mut Vec<String>, n: &str| {
                let v = get(n);
                if v != init(n) {
                    parts.push(v.to_owned());
                }
            };
            push(&mut parts, "background-image");
            let pos = get("background-position");
            let size = get("background-size");
            if size != init("background-size") {
                parts.push(format!("{pos} / {size}"));
            } else if pos != init("background-position") {
                parts.push(pos.to_owned());
            }
            push(&mut parts, "background-repeat");
            push(&mut parts, "background-attachment");
            push(&mut parts, "background-origin");
            push(&mut parts, "background-clip");
            push(&mut parts, "background-color");
            if parts.is_empty() {
                "none".to_owned()
            } else {
                parts.join(" ")
            }
        }
        "grid-row" | "grid-column" => {
            if vals.len() == 2 {
                let auto_or_same = vals[1] == "auto" && !vals[0].starts_with("span");
                if auto_or_same {
                    vals[0].to_owned()
                } else {
                    format!("{} / {}", vals[0], vals[1])
                }
            } else {
                String::new()
            }
        }
        name if is_alias_shorthand(name) => String::new(),
        _ => {
            let parts = non_initial();
            if parts.is_empty() {
                vals.first().map_or_else(String::new, |v| (*v).to_owned())
            } else {
                parts.join(" ")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_serialize() {
        let mut b = DeclBlock::parse("color: RED; margin: 1px 2px; --x: a  b");
        assert_eq!(b.get_property_value("color"), "red");
        assert_eq!(b.get_property_value("margin-left"), "2px");
        assert_eq!(b.get_property_value("margin"), "1px 2px");
        assert_eq!(b.get_property_value("--x"), "a  b");
        assert_eq!(b.css_text(), "color: red; margin: 1px 2px; --x: a  b;");
        assert!(b.set_property("margin-top", "5px", ""));
        assert_eq!(b.get_property_value("margin"), "5px 2px 1px");
        assert!(!b.set_property("color", "nonsense", ""));
        assert!(!b.set_property("color", "red; x: y", ""));
        assert!(b.set_property("color", "blue", "important"));
        assert_eq!(b.get_property_priority("color"), "important");
        assert_eq!(b.remove_property("margin"), ("5px 2px 1px".into(), true));
        assert_eq!(b.css_text(), "color: blue !important; --x: a  b;");
        assert!(b.set_property("border", "1px solid", ""));
        assert_eq!(b.get_property_value("border-top-style"), "solid");
        assert_eq!(b.get_property_value("border"), "1px solid");
    }

    #[test]
    fn important_wins_when_parsing() {
        let b = DeclBlock::parse("color: red !important; color: blue");
        assert_eq!(b.get_property_value("color"), "red");
        assert_eq!(b.get_property_priority("color"), "important");
    }
}
