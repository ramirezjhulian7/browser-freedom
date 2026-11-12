//! At-rules que bf-css no interpreta pero conserva para otros crates: `@property` (lo
//! interpreta bf-style, `values::registered`) llega como `Rule::Unknown` con su bloque de
//! declaraciones, en el nivel superior y dentro de `@media`/`@supports`/`@layer`.

use bf_css::stylesheet::{LayerRule, Origin, Rule, Stylesheet};
use bf_css::syntax::TokenKind;

fn property_rule(rule: &Rule) -> Option<(String, Vec<String>)> {
    let Rule::Unknown(at) = rule else {
        return None;
    };
    if !at.name.eq_ignore_ascii_case("property") {
        return None;
    }
    let name = at.prelude.iter().find_map(|v| match v.token() {
        Some(TokenKind::Ident(s)) => Some(s.clone()),
        _ => None,
    })?;
    let decls = at
        .block
        .as_ref()?
        .contents
        .declarations
        .iter()
        .map(|d| d.name.clone())
        .collect();
    Some((name, decls))
}

#[test]
fn property_is_kept_with_its_descriptors() {
    let sheet = Stylesheet::parse(
        "@property --x { syntax: '<length>'; inherits: false; initial-value: 0px }",
        Origin::Author,
    );
    let [rule] = sheet.rules.as_slice() else {
        panic!("una regla: {:?}", sheet.rules);
    };
    assert_eq!(
        property_rule(rule),
        Some((
            "--x".to_owned(),
            vec![
                "syntax".to_owned(),
                "inherits".to_owned(),
                "initial-value".to_owned()
            ]
        ))
    );
}

#[test]
fn property_inside_group_rules() {
    let sheet = Stylesheet::parse(
        "@media all { @property --a { syntax: '*'; inherits: true } }
         @supports (color: red) { @property --b { syntax: '*'; inherits: true } }
         @layer l { @property --c { syntax: '*'; inherits: true } }",
        Origin::Author,
    );
    let mut names = Vec::new();
    for rule in &sheet.rules {
        let inner = match rule {
            Rule::Media(m) => &m.rules,
            Rule::Supports(s) => &s.rules,
            Rule::Layer(LayerRule::Block { rules, .. }) => rules,
            other => panic!("regla inesperada {other:?}"),
        };
        names.extend(inner.iter().filter_map(property_rule).map(|(n, _)| n));
    }
    assert_eq!(names, ["--a", "--b", "--c"]);
}
