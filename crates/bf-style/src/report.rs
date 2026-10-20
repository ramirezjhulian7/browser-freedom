//! Qué del CSS de un documento ignora el motor: propiedades desconocidas, valores que no se
//! aceptan (con el mismo criterio que `@supports`) y at-rules sin efecto. Se calcula sobre las
//! hojas de autor y los atributos `style`; la hoja UA no cuenta.

use std::collections::BTreeMap;

use bf_css::stylesheet::{LayerRule, Rule, Stylesheet};
use bf_css::syntax::{Declaration, parse_declaration_list};
use bf_dom::Document;

use crate::cascade::is_supported;
use crate::properties::longhand_by_name;
use crate::properties::shorthands::shorthand_by_name;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StyleReport {
    /// Hojas de autor (`<style>`, `<link>` y sus `@import`).
    pub sheets: usize,
    /// Reglas de estilo (incluidas las anidadas y las de `@media`/`@supports`/`@layer`).
    pub rules: usize,
    /// Declaraciones de autor analizadas (hojas y atributos `style`).
    pub declarations: usize,
    /// Propiedad desconocida → apariciones.
    pub unknown_properties: BTreeMap<String, usize>,
    /// Propiedad conocida cuyo valor el motor no acepta → apariciones.
    pub unsupported_values: BTreeMap<String, usize>,
    /// At-rule sin efecto (`@container`, `@keyframes` sin animaciones, ...) → apariciones.
    pub unsupported_at_rules: BTreeMap<String, usize>,
}

impl StyleReport {
    fn declaration(&mut self, d: &Declaration) {
        self.declarations += 1;
        let name = d.name.to_ascii_lowercase();
        if name.starts_with("--") {
            return;
        }
        if longhand_by_name(&name).is_none() && shorthand_by_name(&name).is_none() {
            *self.unknown_properties.entry(name).or_default() += 1;
        } else if !is_supported(&name, &d.value) {
            *self.unsupported_values.entry(name).or_default() += 1;
        }
    }

    fn at_rule(&mut self, name: &str) {
        *self
            .unsupported_at_rules
            .entry(format!("@{}", name.to_ascii_lowercase()))
            .or_default() += 1;
    }

    fn rules(&mut self, rules: &[Rule]) {
        for rule in rules {
            match rule {
                Rule::Style(r) => {
                    self.rules += 1;
                    for d in &r.declarations {
                        self.declaration(d);
                    }
                    self.rules(&r.nested);
                }
                Rule::Media(m) => self.rules(&m.rules),
                Rule::Supports(s) => self.rules(&s.rules),
                Rule::Layer(LayerRule::Block { rules, .. }) => self.rules(rules),
                Rule::Layer(LayerRule::Statement(_))
                | Rule::Import(_)
                | Rule::FontFace(_)
                | Rule::Namespace(_) => {}
                // Se parsean pero todavía no tienen efecto.
                Rule::Keyframes(_) => self.at_rule("keyframes"),
                Rule::Page(_) => self.at_rule("page"),
                Rule::Unknown(a) => self.at_rule(&a.name),
            }
        }
    }

    /// Los totales de lo ignorado, para un indicador rápido.
    pub fn ignored(&self) -> usize {
        self.unknown_properties.values().sum::<usize>()
            + self.unsupported_values.values().sum::<usize>()
            + self.unsupported_at_rules.values().sum::<usize>()
    }
}

/// El reporte de las hojas de autor `sheets` y los atributos `style` de `doc`.
pub fn style_report(doc: &Document, sheets: &[Stylesheet]) -> StyleReport {
    let mut report = StyleReport {
        sheets: sheets.len(),
        ..StyleReport::default()
    };
    for sheet in sheets {
        report.rules(&sheet.rules);
    }
    for node in doc.inclusive_descendants(doc.root()) {
        if let Some(style) = doc.element(node).and_then(|e| e.attr("style")) {
            for d in parse_declaration_list(style) {
                report.declaration(&d);
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use bf_css::stylesheet::Origin;

    #[test]
    fn counts_what_is_ignored() {
        let sheet = Stylesheet::parse(
            "a { color: red; -webkit-tap-highlight-color: x; zoom: 2; display: flex }
             @media screen { b { color: nope; mask-type: alpha } }
             @container (min-width: 1px) { c { color: red } }
             @keyframes k { from { color: red } }",
            Origin::Author,
        );
        let (doc, _) = bf_html::parse_document(
            "<p style='color: red; foo: bar'>",
            &bf_html::ParseOptions::default(),
        );
        let r = style_report(&doc, &[sheet]);
        assert_eq!(r.sheets, 1);
        assert_eq!(r.rules, 2);
        assert_eq!(
            r.unknown_properties.get("-webkit-tap-highlight-color"),
            Some(&1)
        );
        assert_eq!(r.unknown_properties.get("zoom"), Some(&1));
        assert_eq!(r.unknown_properties.get("mask-type"), Some(&1));
        assert_eq!(r.unknown_properties.get("foo"), Some(&1));
        assert_eq!(r.unsupported_values.get("color"), Some(&1));
        assert_eq!(r.unsupported_at_rules.get("@container"), Some(&1));
        assert_eq!(r.unsupported_at_rules.get("@keyframes"), Some(&1));
        assert_eq!(r.declarations, 8);
    }
}
