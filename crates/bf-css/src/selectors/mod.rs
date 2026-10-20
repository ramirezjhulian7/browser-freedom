//! Selectors Level 4: AST, parser ([`parser`]), especificidad y matching ([`matching`]).

mod matching;
mod parser;

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;

pub use matching::{MatchContext, matches, matches_list, matching_specificity};
pub use parser::{
    SelectorError, parse_relative_selector_list_from, parse_selector_list, parse_selector_list_from,
};

/// Declaraciones `@namespace` vigentes al parsear selectores.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NamespaceMap {
    /// Default namespace (URL).
    pub default: Option<String>,
    /// Prefijo → URL.
    pub prefixes: HashMap<String, String>,
}

/// Restricción de namespace de un type/universal selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NsConstraint {
    /// Sin prefijo y sin default namespace, o `*|`.
    Any,
    /// `|E`: elementos sin namespace.
    None,
    /// Un namespace concreto (URL).
    Url(String),
}

/// Restricción de namespace de un atributo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttrNs {
    /// Sin prefijo o `|attr`: atributos sin namespace.
    None,
    /// `*|attr`.
    Any,
    Url(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttrOp {
    /// `=`
    Equals,
    /// `~=`
    Includes,
    /// `|=`
    DashMatch,
    /// `^=`
    Prefix,
    /// `$=`
    Suffix,
    /// `*=`
    Substring,
}

/// Modificador `i` / `s` de un attribute selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseFlag {
    /// Sin modificador: lo decide el lenguaje del documento.
    Default,
    Insensitive,
    Sensitive,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AttrSelector {
    pub ns: AttrNs,
    pub name: String,
    pub matcher: Option<(AttrOp, String, CaseFlag)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Ltr,
    Rtl,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PseudoClass {
    Not(SelectorList),
    Is(SelectorList),
    Where(SelectorList),
    Has(SelectorList),
    NthChild {
        a: i32,
        b: i32,
        of: Option<SelectorList>,
    },
    NthLastChild {
        a: i32,
        b: i32,
        of: Option<SelectorList>,
    },
    NthOfType {
        a: i32,
        b: i32,
    },
    NthLastOfType {
        a: i32,
        b: i32,
    },
    FirstChild,
    LastChild,
    OnlyChild,
    FirstOfType,
    LastOfType,
    OnlyOfType,
    Root,
    Empty,
    Link,
    AnyLink,
    Visited,
    Hover,
    Active,
    Focus,
    FocusVisible,
    FocusWithin,
    Target,
    Checked,
    Disabled,
    Enabled,
    Required,
    Optional,
    ReadOnly,
    ReadWrite,
    PlaceholderShown,
    Lang(Vec<String>),
    Dir(Dir),
    Scope,
    Defined,
    /// `:heading` (vacío) o `:heading(<integer>#)` de HTML: elementos `h1`–`h6` con esos
    /// niveles.
    Heading(Vec<i32>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PseudoElement {
    Before,
    After,
    FirstLine,
    FirstLetter,
    Marker,
    Placeholder,
    Selection,
}

impl PseudoElement {
    pub fn name(self) -> &'static str {
        match self {
            PseudoElement::Before => "before",
            PseudoElement::After => "after",
            PseudoElement::FirstLine => "first-line",
            PseudoElement::FirstLetter => "first-letter",
            PseudoElement::Marker => "marker",
            PseudoElement::Placeholder => "placeholder",
            PseudoElement::Selection => "selection",
        }
    }
}

/// Simple selector.
#[derive(Debug, Clone, PartialEq)]
pub enum Simple {
    /// Type selector. `name` tal como se escribió y `lower` en minúsculas ASCII (para elementos
    /// HTML en documentos HTML).
    Type {
        ns: NsConstraint,
        name: String,
        lower: String,
    },
    Universal(NsConstraint),
    Id(String),
    Class(String),
    Attribute(AttrSelector),
    PseudoClass(PseudoClass),
    /// `&`. En el modelo de hoja de estilos se reemplaza por `:is(<selector padre>)`; si queda
    /// sin resolver equivale a `:scope`.
    Nesting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combinator {
    /// Whitespace.
    Descendant,
    /// `>`
    Child,
    /// `+`
    NextSibling,
    /// `~`
    SubsequentSibling,
}

pub type Compound = Vec<Simple>;

/// Complex selector (o relative selector, si `leading` no es `None`).
#[derive(Debug, Clone, PartialEq)]
pub struct ComplexSelector {
    /// Combinator inicial de un relative selector (`:has(> a)`, reglas anidadas).
    pub leading: Option<Combinator>,
    /// Compound selectors en orden de fuente.
    pub compounds: Vec<Compound>,
    /// `combinators[i]` une `compounds[i]` con `compounds[i + 1]`.
    pub combinators: Vec<Combinator>,
    /// Pseudo-elemento al final del último compound.
    pub pseudo_element: Option<PseudoElement>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SelectorList(pub Vec<ComplexSelector>);

/// Especificidad `(a, b, c)`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Specificity(pub u32, pub u32, pub u32);

impl Ord for Specificity {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.0, self.1, self.2).cmp(&(other.0, other.1, other.2))
    }
}

impl PartialOrd for Specificity {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl std::ops::Add for Specificity {
    type Output = Specificity;
    fn add(self, o: Specificity) -> Specificity {
        Specificity(
            self.0.saturating_add(o.0),
            self.1.saturating_add(o.1),
            self.2.saturating_add(o.2),
        )
    }
}

impl fmt::Display for Specificity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {}, {})", self.0, self.1, self.2)
    }
}

impl SelectorList {
    /// La mayor especificidad entre sus complex selectors (la de `:is()`), o cero si está vacía.
    pub fn max_specificity(&self) -> Specificity {
        self.0
            .iter()
            .map(ComplexSelector::specificity)
            .max()
            .unwrap_or_default()
    }

    /// ¿Algún selector usa `&`?
    pub fn contains_nesting(&self) -> bool {
        self.0.iter().any(ComplexSelector::contains_nesting)
    }

    /// Reemplaza `&` por `:is(parent)` en todos los selectores.
    pub fn replace_nesting(&mut self, parent: &SelectorList) {
        for selector in &mut self.0 {
            selector.replace_nesting(parent);
        }
    }
}

impl ComplexSelector {
    /// Especificidad (§15 de Selectors 4).
    pub fn specificity(&self) -> Specificity {
        let mut s = Specificity::default();
        for simple in self.compounds.iter().flatten() {
            s = s + simple_specificity(simple);
        }
        if self.pseudo_element.is_some() {
            s = s + Specificity(0, 0, 1);
        }
        s
    }

    pub fn contains_nesting(&self) -> bool {
        self.compounds.iter().flatten().any(|simple| match simple {
            Simple::Nesting => true,
            Simple::PseudoClass(pc) => pseudo_class_lists(pc).any(SelectorList::contains_nesting),
            _ => false,
        })
    }

    pub fn replace_nesting(&mut self, parent: &SelectorList) {
        for simple in self.compounds.iter_mut().flatten() {
            match simple {
                Simple::Nesting => {
                    // El nesting selector no representa pseudo-elementos (como `:is()`).
                    let list = SelectorList(
                        parent
                            .0
                            .iter()
                            .filter(|s| s.pseudo_element.is_none())
                            .cloned()
                            .collect(),
                    );
                    *simple = Simple::PseudoClass(PseudoClass::Is(list));
                }
                Simple::PseudoClass(pc) => {
                    for list in pseudo_class_lists_mut(pc) {
                        list.replace_nesting(parent);
                    }
                }
                _ => {}
            }
        }
    }
}

fn simple_specificity(simple: &Simple) -> Specificity {
    match simple {
        Simple::Id(_) => Specificity(1, 0, 0),
        Simple::Class(_) | Simple::Attribute(_) => Specificity(0, 1, 0),
        Simple::Type { .. } => Specificity(0, 0, 1),
        Simple::Universal(_) => Specificity::default(),
        // `&` sin resolver: "zero if no such selector list exists".
        Simple::Nesting => Specificity::default(),
        Simple::PseudoClass(pc) => match pc {
            PseudoClass::Is(list) | PseudoClass::Not(list) | PseudoClass::Has(list) => {
                list.max_specificity()
            }
            PseudoClass::Where(_) => Specificity::default(),
            PseudoClass::NthChild { of, .. } | PseudoClass::NthLastChild { of, .. } => {
                Specificity(0, 1, 0)
                    + of.as_ref()
                        .map(SelectorList::max_specificity)
                        .unwrap_or_default()
            }
            _ => Specificity(0, 1, 0),
        },
    }
}

fn pseudo_class_lists(pc: &PseudoClass) -> impl Iterator<Item = &SelectorList> {
    let list = match pc {
        PseudoClass::Not(l) | PseudoClass::Is(l) | PseudoClass::Where(l) | PseudoClass::Has(l) => {
            Some(l)
        }
        PseudoClass::NthChild { of, .. } | PseudoClass::NthLastChild { of, .. } => of.as_ref(),
        _ => None,
    };
    list.into_iter()
}

fn pseudo_class_lists_mut(pc: &mut PseudoClass) -> impl Iterator<Item = &mut SelectorList> {
    let list = match pc {
        PseudoClass::Not(l) | PseudoClass::Is(l) | PseudoClass::Where(l) | PseudoClass::Has(l) => {
            Some(l)
        }
        PseudoClass::NthChild { of, .. } | PseudoClass::NthLastChild { of, .. } => of.as_mut(),
        _ => None,
    };
    list.into_iter()
}

// --- Serialización (aproximada a CSSOM "serialize a selector") ---

impl fmt::Display for SelectorList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, s) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{s}")?;
        }
        Ok(())
    }
}

impl fmt::Display for Combinator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Combinator::Descendant => " ",
            Combinator::Child => " > ",
            Combinator::NextSibling => " + ",
            Combinator::SubsequentSibling => " ~ ",
        })
    }
}

impl fmt::Display for ComplexSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(c) = self.leading
            && c != Combinator::Descendant
        {
            write!(f, "{}", c.to_string().trim_start())?;
        }
        for (i, compound) in self.compounds.iter().enumerate() {
            if i > 0 {
                write!(f, "{}", self.combinators[i - 1])?;
            }
            if compound.is_empty() {
                f.write_str("*")?;
            }
            for simple in compound {
                write!(f, "{simple}")?;
            }
        }
        if let Some(pe) = self.pseudo_element {
            write!(f, "::{}", pe.name())?;
        }
        Ok(())
    }
}

fn write_ns(f: &mut fmt::Formatter<'_>, ns: &NsConstraint) -> fmt::Result {
    match ns {
        NsConstraint::Any => Ok(()),
        NsConstraint::None => f.write_str("|"),
        NsConstraint::Url(url) => write!(f, "{{{url}}}|"),
    }
}

impl fmt::Display for Simple {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Simple::Type { ns, name, .. } => {
                write_ns(f, ns)?;
                f.write_str(name)
            }
            Simple::Universal(ns) => {
                write_ns(f, ns)?;
                f.write_str("*")
            }
            Simple::Id(id) => write!(f, "#{id}"),
            Simple::Class(c) => write!(f, ".{c}"),
            Simple::Attribute(a) => {
                f.write_str("[")?;
                match &a.ns {
                    AttrNs::None => {}
                    AttrNs::Any => f.write_str("*|")?,
                    AttrNs::Url(url) => write!(f, "{{{url}}}|")?,
                }
                f.write_str(&a.name)?;
                if let Some((op, value, flag)) = &a.matcher {
                    let op = match op {
                        AttrOp::Equals => "=",
                        AttrOp::Includes => "~=",
                        AttrOp::DashMatch => "|=",
                        AttrOp::Prefix => "^=",
                        AttrOp::Suffix => "$=",
                        AttrOp::Substring => "*=",
                    };
                    write!(f, "{op}{value:?}")?;
                    match flag {
                        CaseFlag::Default => {}
                        CaseFlag::Insensitive => f.write_str(" i")?,
                        CaseFlag::Sensitive => f.write_str(" s")?,
                    }
                }
                f.write_str("]")
            }
            Simple::Nesting => f.write_str("&"),
            Simple::PseudoClass(pc) => write_pseudo_class(f, pc),
        }
    }
}

fn write_anb(f: &mut fmt::Formatter<'_>, a: i32, b: i32) -> fmt::Result {
    match (a, b) {
        (0, b) => write!(f, "{b}"),
        (a, 0) => write!(f, "{a}n"),
        (a, b) if b > 0 => write!(f, "{a}n+{b}"),
        (a, b) => write!(f, "{a}n{b}"),
    }
}

fn write_pseudo_class(f: &mut fmt::Formatter<'_>, pc: &PseudoClass) -> fmt::Result {
    let simple = match pc {
        PseudoClass::Not(l) => return write!(f, ":not({l})"),
        PseudoClass::Is(l) => return write!(f, ":is({l})"),
        PseudoClass::Where(l) => return write!(f, ":where({l})"),
        PseudoClass::Has(l) => return write!(f, ":has({l})"),
        PseudoClass::NthChild { a, b, of } | PseudoClass::NthLastChild { a, b, of } => {
            let name = if matches!(pc, PseudoClass::NthChild { .. }) {
                "nth-child"
            } else {
                "nth-last-child"
            };
            write!(f, ":{name}(")?;
            write_anb(f, *a, *b)?;
            if let Some(of) = of {
                write!(f, " of {of}")?;
            }
            return f.write_str(")");
        }
        PseudoClass::NthOfType { a, b } | PseudoClass::NthLastOfType { a, b } => {
            let name = if matches!(pc, PseudoClass::NthOfType { .. }) {
                "nth-of-type"
            } else {
                "nth-last-of-type"
            };
            write!(f, ":{name}(")?;
            write_anb(f, *a, *b)?;
            return f.write_str(")");
        }
        PseudoClass::Lang(ranges) => {
            f.write_str(":lang(")?;
            for (i, r) in ranges.iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{r:?}")?;
            }
            return f.write_str(")");
        }
        PseudoClass::Heading(levels) => {
            if levels.is_empty() {
                return f.write_str(":heading");
            }
            let levels: Vec<String> = levels.iter().map(ToString::to_string).collect();
            return write!(f, ":heading({})", levels.join(", "));
        }
        PseudoClass::Dir(d) => {
            return f.write_str(match d {
                Dir::Ltr => ":dir(ltr)",
                Dir::Rtl => ":dir(rtl)",
            });
        }
        PseudoClass::FirstChild => "first-child",
        PseudoClass::LastChild => "last-child",
        PseudoClass::OnlyChild => "only-child",
        PseudoClass::FirstOfType => "first-of-type",
        PseudoClass::LastOfType => "last-of-type",
        PseudoClass::OnlyOfType => "only-of-type",
        PseudoClass::Root => "root",
        PseudoClass::Empty => "empty",
        PseudoClass::Link => "link",
        PseudoClass::AnyLink => "any-link",
        PseudoClass::Visited => "visited",
        PseudoClass::Hover => "hover",
        PseudoClass::Active => "active",
        PseudoClass::Focus => "focus",
        PseudoClass::FocusVisible => "focus-visible",
        PseudoClass::FocusWithin => "focus-within",
        PseudoClass::Target => "target",
        PseudoClass::Checked => "checked",
        PseudoClass::Disabled => "disabled",
        PseudoClass::Enabled => "enabled",
        PseudoClass::Required => "required",
        PseudoClass::Optional => "optional",
        PseudoClass::ReadOnly => "read-only",
        PseudoClass::ReadWrite => "read-write",
        PseudoClass::PlaceholderShown => "placeholder-shown",
        PseudoClass::Scope => "scope",
        PseudoClass::Defined => "defined",
    };
    write!(f, ":{simple}")
}
