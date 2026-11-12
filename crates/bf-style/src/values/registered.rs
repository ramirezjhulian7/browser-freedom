//! Custom properties registradas (CSS Properties and Values API 1): la regla `@property`
//! (§3) y lo que `CSS.registerProperty()` (§2) le pasa a la cascada.
//!
//! Una propiedad registrada tiene una sintaxis (§5), hereda o no según `inherits` y tiene un
//! valor inicial. En la cascada ([`crate::cascade`]) su valor se valida contra la sintaxis
//! después de sustituir `var()` (si no la cumple es "inválida en computed-value time" y se
//! comporta como `unset`) y se computa (§2.4: longitudes en px, colores resueltos…).
//!
//! bf-css deja `@property` como [`Rule::Unknown`] (añadir una variante a `Rule` cambiaría los
//! `match` del CSSOM de bf-web); aquí se interpreta su bloque.

use std::collections::HashMap;
use std::sync::Arc;

use bf_css::media::MediaContext;
use bf_css::stylesheet::{LayerRule, Rule, Stylesheet};
use bf_css::syntax::{
    AtRule, ComponentValue, Declaration, NestedItem, TokenKind, parse_component_value_list,
    split_comma_separated,
};

use super::color::{Color, parse_color};
use super::{
    LengthContext, LengthPercentage, LengthUnit, components, fmt_number, ident, parse_angle,
    parse_length_percentage, parse_number, parse_time, parse_url, trim,
};

/// Un tipo de `<syntax-component-name>` (§5.1) o un ident literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaxKind {
    Length,
    Number,
    Percentage,
    LengthPercentage,
    Color,
    Image,
    Url,
    Integer,
    Angle,
    Time,
    Resolution,
    TransformFunction,
    CustomIdent,
    TransformList,
    String,
    Ident(String),
}

/// Multiplicador de un componente: `+` (lista separada por espacios) o `#` (por comas).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Multiplier {
    Space,
    Comma,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxComponent {
    pub kind: SyntaxKind,
    pub multiplier: Option<Multiplier>,
}

/// La sintaxis de una propiedad registrada: `*` o alternativas `|`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Syntax {
    Universal,
    Alternatives(Vec<SyntaxComponent>),
}

impl Syntax {
    /// "Consume a syntax definition" (§5.4). `None` si la cadena no es válida.
    pub fn parse(text: &str) -> Option<Syntax> {
        let text = text.trim_matches(|c: char| c.is_ascii_whitespace());
        if text.is_empty() {
            return None;
        }
        if text == "*" {
            return Some(Syntax::Universal);
        }
        let mut out = Vec::new();
        for part in text.split('|') {
            let part = part.trim_matches(|c: char| c.is_ascii_whitespace());
            let (body, multiplier) = match part.as_bytes().last() {
                Some(b'+') => (&part[..part.len() - 1], Some(Multiplier::Space)),
                Some(b'#') => (&part[..part.len() - 1], Some(Multiplier::Comma)),
                _ => (part, None),
            };
            let kind = if let Some(name) = body.strip_prefix('<').and_then(|b| b.strip_suffix('>'))
            {
                match name {
                    "length" => SyntaxKind::Length,
                    "number" => SyntaxKind::Number,
                    "percentage" => SyntaxKind::Percentage,
                    "length-percentage" => SyntaxKind::LengthPercentage,
                    "color" => SyntaxKind::Color,
                    "image" => SyntaxKind::Image,
                    "url" => SyntaxKind::Url,
                    "integer" => SyntaxKind::Integer,
                    "angle" => SyntaxKind::Angle,
                    "time" => SyntaxKind::Time,
                    "resolution" => SyntaxKind::Resolution,
                    "transform-function" => SyntaxKind::TransformFunction,
                    "custom-ident" => SyntaxKind::CustomIdent,
                    "string" => SyntaxKind::String,
                    // §5.1: `<transform-list>` no admite multiplicador.
                    "transform-list" if multiplier.is_none() => SyntaxKind::TransformList,
                    _ => return None,
                }
            } else {
                // Un ident literal: no puede ser una palabra clave CSS-wide ni `default`.
                let valid = !body.is_empty()
                    && body
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || !c.is_ascii())
                    && !body.starts_with(|c: char| c.is_ascii_digit())
                    && !is_reserved_ident(body);
                if !valid {
                    return None;
                }
                SyntaxKind::Ident(body.to_owned())
            };
            out.push(SyntaxComponent { kind, multiplier });
        }
        Some(Syntax::Alternatives(out))
    }

    /// El primer componente que acepta `value` (sin `var()` ya sustituido).
    fn matching(&self, value: &[ComponentValue]) -> Option<&SyntaxComponent> {
        match self {
            Syntax::Universal => None,
            Syntax::Alternatives(list) => list.iter().find(|c| c.matches(value)),
        }
    }

    /// ¿`value` cumple la sintaxis?
    pub fn accepts(&self, value: &[ComponentValue]) -> bool {
        matches!(self, Syntax::Universal) || self.matching(value).is_some()
    }
}

fn is_reserved_ident(s: &str) -> bool {
    [
        "initial",
        "inherit",
        "unset",
        "revert",
        "revert-layer",
        "default",
    ]
    .iter()
    .any(|k| s.eq_ignore_ascii_case(k))
}

impl SyntaxComponent {
    /// Los elementos de `value` según el multiplicador (`None` si no tiene la forma).
    fn items(&self, value: &[ComponentValue]) -> Option<Vec<Vec<ComponentValue>>> {
        let value = trim(value);
        if value.is_empty() {
            return None;
        }
        match self.multiplier {
            None => Some(vec![value.to_vec()]),
            Some(Multiplier::Space) => Some(
                components(value)
                    .into_iter()
                    .map(|v| vec![v.clone()])
                    .collect(),
            ),
            Some(Multiplier::Comma) => {
                let groups = split_comma_separated(value);
                groups
                    .iter()
                    .all(|g| !trim(g).is_empty())
                    .then(|| groups.iter().map(|g| trim(g).to_vec()).collect())
            }
        }
    }

    fn matches(&self, value: &[ComponentValue]) -> bool {
        self.items(value)
            .is_some_and(|items| items.iter().all(|i| self.kind.matches(i)))
    }
}

impl SyntaxKind {
    fn matches(&self, item: &[ComponentValue]) -> bool {
        if let SyntaxKind::TransformList = self {
            return crate::properties::longhand_by_name("transform")
                .is_some_and(|id| (id.def().parse)(item).is_some())
                && !single_ident_is(item, "none");
        }
        let [v] = trim(item) else {
            return false;
        };
        match self {
            SyntaxKind::Length => parse_length_percentage(v).is_some_and(|lp| !lp.has_percentage()),
            SyntaxKind::Number => parse_number(v).is_some(),
            SyntaxKind::Integer => {
                parse_number(v).is_some_and(|n| n.fract() == 0.0)
                    && !matches!(v.token(), Some(TokenKind::Number(n)) if n.as_integer().is_none())
            }
            SyntaxKind::Percentage => {
                matches!(v.token(), Some(TokenKind::Percentage(_)))
                    || parse_length_percentage(v).is_some_and(|lp| {
                        matches!(lp, LengthPercentage::Calc(_)) && percentage_only(&lp)
                    })
            }
            SyntaxKind::LengthPercentage => parse_length_percentage(v).is_some(),
            SyntaxKind::Color => parse_color(v).is_some(),
            SyntaxKind::Image => {
                parse_url(v).is_some() || crate::properties::gradient::Gradient::parse(v).is_some()
            }
            SyntaxKind::Url => parse_url(v).is_some(),
            SyntaxKind::Angle => {
                parse_angle(v).is_some() && !matches!(v.token(), Some(TokenKind::Number(_)))
            }
            SyntaxKind::Time => parse_time(v).is_some(),
            SyntaxKind::Resolution => matches!(
                v.token(),
                Some(TokenKind::Dimension(_, u))
                    if ["dpi", "dpcm", "dppx", "x"].iter().any(|k| u.eq_ignore_ascii_case(k))
            ),
            SyntaxKind::TransformFunction => {
                matches!(v, ComponentValue::Function(_))
                    && crate::properties::longhand_by_name("transform")
                        .is_some_and(|id| (id.def().parse)(item).is_some())
            }
            SyntaxKind::CustomIdent => ident(v).is_some_and(|s| !is_reserved_ident(s)),
            SyntaxKind::String => matches!(v.token(), Some(TokenKind::String(_))),
            SyntaxKind::Ident(name) => ident(v) == Some(name.as_str()),
            SyntaxKind::TransformList => false,
        }
    }

    /// El valor computado de un elemento (§2.4), serializado.
    fn compute(&self, item: &[ComponentValue], ctx: &LengthContext) -> Option<String> {
        let [v] = trim(item) else {
            return None;
        };
        Some(match self {
            SyntaxKind::Length | SyntaxKind::LengthPercentage | SyntaxKind::Percentage => {
                parse_length_percentage(v)?.compute(ctx).to_string()
            }
            SyntaxKind::Number | SyntaxKind::Integer => fmt_number(parse_number(v)?),
            SyntaxKind::Color => match parse_color(v)? {
                Color::Rgba(c) => c.to_string(),
                Color::CurrentColor => "currentcolor".to_owned(),
            },
            SyntaxKind::Angle => format!("{}deg", fmt_number(parse_angle(v)?)),
            SyntaxKind::Time => format!("{}s", fmt_number(parse_time(v)?)),
            _ => return None,
        })
    }
}

fn single_ident_is(item: &[ComponentValue], name: &str) -> bool {
    matches!(trim(item), [v] if ident(v).is_some_and(|s| s.eq_ignore_ascii_case(name)))
}

fn percentage_only(lp: &LengthPercentage) -> bool {
    // Un `calc()` solo de porcentajes: computado con una referencia 0 y otra 100 da valores
    // proporcionales (sin parte de longitud).
    let ctx = LengthContext::default();
    let c = lp.compute(&ctx);
    c.resolve(0.0) == 0.0 && c.has_percentage()
}

/// Una propiedad registrada.
#[derive(Debug, Clone, PartialEq)]
pub struct Registration {
    pub name: String,
    pub syntax: Syntax,
    pub inherits: bool,
    /// `None` solo con la sintaxis universal sin `initial-value` (el valor inicial es el
    /// guaranteed-invalid value).
    pub initial: Option<Arc<Vec<ComponentValue>>>,
}

impl Registration {
    /// Valida una registración (§3.1 para `@property`, §2.1 paso 3–6 para
    /// `CSS.registerProperty`): nombre de custom property, sintaxis válida y, salvo con `*`,
    /// un valor inicial que cumpla la sintaxis y sea computacionalmente independiente.
    pub fn new(
        name: &str,
        syntax: &str,
        inherits: bool,
        initial: Option<&[ComponentValue]>,
    ) -> Option<Registration> {
        if !name.starts_with("--") {
            return None;
        }
        let syntax = Syntax::parse(syntax)?;
        let initial = match (initial.map(trim), &syntax) {
            (None, Syntax::Universal) => None,
            (None, _) => return None,
            (Some(v), Syntax::Universal) => Some(Arc::new(v.to_vec())),
            (Some(v), s) => {
                if !s.accepts(v) || !computationally_independent(v) {
                    return None;
                }
                Some(Arc::new(v.to_vec()))
            }
        };
        Some(Registration {
            name: name.to_owned(),
            syntax,
            inherits,
            initial,
        })
    }

    /// `@property <custom-property-name> { syntax; inherits; initial-value }` (§3).
    pub fn from_at_rule(at: &AtRule) -> Option<Registration> {
        let prelude = components(&at.prelude);
        let [name] = prelude.as_slice() else {
            return None;
        };
        let name = ident(name)?;
        let block = at.block.as_ref()?;
        let mut decls: Vec<&Declaration> = block.contents.declarations.iter().collect();
        for item in &block.contents.rules {
            if let NestedItem::Declarations(d) = item {
                decls.extend(d.iter());
            }
        }
        let mut syntax = None;
        let mut inherits = None;
        let mut initial = None;
        // La última declaración válida de cada descriptor gana.
        for d in decls {
            match d.name.to_ascii_lowercase().as_str() {
                "syntax" => {
                    if let [v] = components(&d.value).as_slice()
                        && let Some(TokenKind::String(s)) = v.token()
                    {
                        syntax = Some(s.clone());
                    }
                }
                "inherits" => match components(&d.value).as_slice() {
                    [v] if ident(v).is_some_and(|s| s.eq_ignore_ascii_case("true")) => {
                        inherits = Some(true);
                    }
                    [v] if ident(v).is_some_and(|s| s.eq_ignore_ascii_case("false")) => {
                        inherits = Some(false);
                    }
                    _ => {}
                },
                "initial-value" => initial = Some(d.value.clone()),
                _ => {}
            }
        }
        Registration::new(name, &syntax?, inherits?, initial.as_deref())
    }

    /// ¿`value` (ya sustituido) es válido para la propiedad?
    pub fn accepts(&self, value: &[ComponentValue]) -> bool {
        self.syntax.accepts(value)
    }

    /// El valor computado (§2.4) de un valor válido: las longitudes en px, los números y los
    /// colores resueltos; el resto se queda como está.
    pub fn compute(
        &self,
        value: &[ComponentValue],
        ctx: &LengthContext,
    ) -> Option<Vec<ComponentValue>> {
        let component = self.syntax.matching(value)?;
        let items = component.items(value)?;
        let mut parts = Vec::with_capacity(items.len());
        for item in &items {
            parts.push(component.kind.compute(item, ctx)?);
        }
        let sep = match component.multiplier {
            Some(Multiplier::Comma) => ", ",
            _ => " ",
        };
        Some(parse_component_value_list(&parts.join(sep)))
    }
}

/// ¿Se puede computar sin otras propiedades (§3.1)? Sin `var()`/`env()` ni unidades
/// relativas a la fuente o al viewport.
fn computationally_independent(values: &[ComponentValue]) -> bool {
    values.iter().all(|v| match v {
        ComponentValue::Function(f) => {
            !f.name.eq_ignore_ascii_case("var")
                && !f.name.eq_ignore_ascii_case("env")
                && computationally_independent(&f.value)
        }
        ComponentValue::Block(b) => computationally_independent(&b.value),
        ComponentValue::Token(t) => match &t.kind {
            TokenKind::Dimension(_, unit) => !LengthUnit::from_unit(unit).is_some_and(|u| {
                !matches!(
                    u,
                    LengthUnit::Px
                        | LengthUnit::Cm
                        | LengthUnit::Mm
                        | LengthUnit::Q
                        | LengthUnit::In
                        | LengthUnit::Pt
                        | LengthUnit::Pc
                )
            }),
            _ => true,
        },
    })
}

/// Las propiedades registradas de un documento, por nombre.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    map: HashMap<String, Arc<Registration>>,
}

impl Registry {
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&Arc<Registration>> {
        self.map.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<Registration>> {
        self.map.values()
    }

    /// Registra (la última registración de un nombre gana).
    pub fn insert(&mut self, r: Registration) {
        self.map.insert(r.name.clone(), Arc::new(r));
    }

    /// Las reglas `@property` de `sheets`, en orden (también dentro de `@media` que se
    /// cumplen, `@supports` y `@layer`). `CSS.registerProperty` (bf-web) gana sobre ellas
    /// (§3: "If a property is registered by both… the registerProperty() one wins"): basta
    /// con pasar sus registraciones en una hoja que vaya después de las del documento, o
    /// llamar a [`Registry::insert`] después.
    pub fn from_sheets(sheets: &[&Stylesheet], media: &MediaContext) -> Registry {
        let mut reg = Registry::default();
        for s in sheets {
            reg.collect(&s.rules, media);
        }
        reg
    }

    fn collect(&mut self, rules: &[Rule], media: &MediaContext) {
        for rule in rules {
            match rule {
                Rule::Unknown(at) if at.name.eq_ignore_ascii_case("property") => {
                    if let Some(r) = Registration::from_at_rule(at) {
                        self.insert(r);
                    }
                }
                Rule::Media(m) if m.queries.evaluate(media) => self.collect(&m.rules, media),
                Rule::Supports(s) if s.condition.evaluate(&crate::cascade::is_supported) => {
                    self.collect(&s.rules, media);
                }
                Rule::Layer(LayerRule::Block { rules, .. }) => self.collect(rules, media),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bf_css::stylesheet::component_values_to_string as to_s;

    fn v(s: &str) -> Vec<ComponentValue> {
        parse_component_value_list(s)
    }

    #[test]
    fn syntax_strings() {
        assert_eq!(Syntax::parse("*"), Some(Syntax::Universal));
        assert!(Syntax::parse("<length>").is_some());
        assert!(Syntax::parse("<length> | <percentage>+ | auto").is_some());
        assert!(Syntax::parse("<color>#").is_some());
        assert!(Syntax::parse("<transform-list>").is_some());
        assert_eq!(Syntax::parse("<transform-list>+"), None);
        assert_eq!(Syntax::parse("<nope>"), None);
        assert_eq!(Syntax::parse(""), None);
        assert_eq!(Syntax::parse("inherit"), None);
        assert_eq!(Syntax::parse("<length> |"), None);
    }

    #[test]
    fn accepts_values() {
        let s = Syntax::parse("<length> | auto").unwrap();
        assert!(s.accepts(&v("10px")));
        assert!(s.accepts(&v("calc(1em + 2px)")));
        assert!(s.accepts(&v("auto")));
        assert!(!s.accepts(&v("10%")));
        assert!(!s.accepts(&v("red")));
        let list = Syntax::parse("<color>#").unwrap();
        assert!(list.accepts(&v("red, blue")));
        assert!(!list.accepts(&v("red blue")));
        let int = Syntax::parse("<integer>").unwrap();
        assert!(int.accepts(&v("3")));
        assert!(!int.accepts(&v("3.5")));
        let angle = Syntax::parse("<angle>").unwrap();
        assert!(angle.accepts(&v("45deg")));
        assert!(!angle.accepts(&v("0")));
    }

    #[test]
    fn registrations() {
        assert!(Registration::new("--a", "<length>", true, Some(&v("0px"))).is_some());
        // Sin valor inicial solo con `*`.
        assert!(Registration::new("--a", "<length>", true, None).is_none());
        assert!(Registration::new("--a", "*", true, None).is_some());
        // No computacionalmente independiente.
        assert!(Registration::new("--a", "<length>", true, Some(&v("1em"))).is_none());
        assert!(Registration::new("a", "*", true, None).is_none());
        let r = Registration::new("--c", "<color>", false, Some(&v("red"))).unwrap();
        let ctx = LengthContext {
            font_size: 10.0,
            ..LengthContext::default()
        };
        let len = Registration::new("--l", "<length>+", true, Some(&v("1px"))).unwrap();
        assert_eq!(
            to_s(&len.compute(&v("2em 1in"), &ctx).unwrap()),
            "20px 96px"
        );
        assert_eq!(
            to_s(&r.compute(&v("hsl(120 100% 50%)"), &ctx).unwrap()),
            "rgb(0, 255, 0)"
        );
    }
}
