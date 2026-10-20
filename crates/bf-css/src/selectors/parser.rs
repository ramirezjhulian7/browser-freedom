//! Parser de selectores (Selectors 4 §16) sobre component values de CSS Syntax.

use super::{
    AttrNs, AttrOp, AttrSelector, CaseFlag, Combinator, ComplexSelector, Compound, Dir,
    NamespaceMap, NsConstraint, PseudoClass, PseudoElement, SelectorList, Simple,
};
use crate::syntax::{
    BlockKind, ComponentValue, TokenKind, parse_anb, parse_component_value_list,
    split_comma_separated,
};

/// El selector (o la lista) es inválido.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectorError;

type Result<T> = std::result::Result<T, SelectorError>;

#[derive(Debug, Clone, Copy)]
struct Ctx {
    /// ¿Se permiten pseudo-elementos? (no dentro de `:is()`, `:not()`, `:has()`, etc.)
    pseudo_elements: bool,
    /// ¿Estamos dentro de `:has()`? (`:has()` no se puede anidar.)
    in_has: bool,
    /// Parsear como relative selector.
    relative: Relative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Relative {
    No,
    /// Argumento de `:has()`: siempre relativo (descendiente por defecto).
    Has,
    /// Prelude de una regla anidada: relativo salvo que contenga `&` sin combinator inicial.
    Nested,
}

const TOP: Ctx = Ctx {
    pseudo_elements: true,
    in_has: false,
    relative: Relative::No,
};

/// Parsea `input` como `<selector-list>` (no forgiving: un selector inválido invalida todo).
pub fn parse_selector_list(input: &str, ns: &NamespaceMap) -> Result<SelectorList> {
    parse_selector_list_from(&parse_component_value_list(input), ns)
}

/// Como [`parse_selector_list`], desde component values (p. ej. el prelude de una regla).
pub fn parse_selector_list_from(
    values: &[ComponentValue],
    ns: &NamespaceMap,
) -> Result<SelectorList> {
    parse_list(values, ns, TOP)
}

/// Parsea el prelude de una regla anidada como `<relative-selector-list>` (CSS Nesting §3.1).
pub fn parse_relative_selector_list_from(
    values: &[ComponentValue],
    ns: &NamespaceMap,
) -> Result<SelectorList> {
    parse_list(
        values,
        ns,
        Ctx {
            relative: Relative::Nested,
            ..TOP
        },
    )
}

fn parse_list(values: &[ComponentValue], ns: &NamespaceMap, ctx: Ctx) -> Result<SelectorList> {
    // Una lista vacía no es un selector válido (salvo en las forgiving lists).
    if values.iter().all(ComponentValue::is_whitespace) {
        return Err(SelectorError);
    }
    split_comma_separated(values)
        .iter()
        .map(|group| parse_complex(group, ns, ctx))
        .collect::<Result<Vec<_>>>()
        .map(SelectorList)
}

/// `<forgiving-selector-list>`: descarta los selectores inválidos.
fn parse_forgiving(values: &[ComponentValue], ns: &NamespaceMap, ctx: Ctx) -> SelectorList {
    SelectorList(
        split_comma_separated(values)
            .iter()
            .filter_map(|group| parse_complex(group, ns, ctx).ok())
            .collect(),
    )
}

fn parse_complex(items: &[ComponentValue], ns: &NamespaceMap, ctx: Ctx) -> Result<ComplexSelector> {
    let mut p = Parser {
        items,
        pos: 0,
        ns,
        ctx,
    };
    p.skip_whitespace();
    if p.at_end() {
        return Err(SelectorError);
    }
    let leading = if ctx.relative == Relative::No {
        None
    } else if let Some(c) = p.combinator() {
        p.skip_whitespace();
        Some(c)
    } else {
        None
    };
    let mut selector = ComplexSelector {
        leading,
        compounds: Vec::new(),
        combinators: Vec::new(),
        pseudo_element: None,
    };
    loop {
        let (compound, pseudo_element) = p.compound()?;
        selector.compounds.push(compound);
        let had_whitespace = p.skip_whitespace();
        if let Some(pe) = pseudo_element {
            if !p.at_end() {
                return Err(SelectorError);
            }
            selector.pseudo_element = Some(pe);
            break;
        }
        if p.at_end() {
            break;
        }
        if let Some(c) = p.combinator() {
            p.skip_whitespace();
            selector.combinators.push(c);
        } else if had_whitespace {
            selector.combinators.push(Combinator::Descendant);
        } else {
            return Err(SelectorError);
        }
        if p.at_end() {
            return Err(SelectorError);
        }
    }
    match ctx.relative {
        Relative::No => {}
        Relative::Has => {
            selector.leading.get_or_insert(Combinator::Descendant);
        }
        Relative::Nested => {
            if selector.leading.is_none() && !selector.contains_nesting() {
                selector.leading = Some(Combinator::Descendant);
            }
        }
    }
    Ok(selector)
}

struct Parser<'a> {
    items: &'a [ComponentValue],
    pos: usize,
    ns: &'a NamespaceMap,
    ctx: Ctx,
}

impl<'a> Parser<'a> {
    fn at_end(&self) -> bool {
        self.pos >= self.items.len()
    }

    fn peek(&self, n: usize) -> Option<&'a ComponentValue> {
        self.items.get(self.pos + n)
    }

    fn peek_token(&self, n: usize) -> Option<&'a TokenKind> {
        self.peek(n).and_then(ComponentValue::token)
    }

    fn skip_whitespace(&mut self) -> bool {
        let start = self.pos;
        while self.peek(0).is_some_and(ComponentValue::is_whitespace) {
            self.pos += 1;
        }
        self.pos > start
    }

    fn combinator(&mut self) -> Option<Combinator> {
        let c = match self.peek_token(0)? {
            TokenKind::Delim('>') => Combinator::Child,
            TokenKind::Delim('+') => Combinator::NextSibling,
            TokenKind::Delim('~') => Combinator::SubsequentSibling,
            _ => return None,
        };
        self.pos += 1;
        Some(c)
    }

    /// `[ <type-selector>? <subclass-selector>* ]!`, más un pseudo-elemento final opcional.
    fn compound(&mut self) -> Result<(Compound, Option<PseudoElement>)> {
        let mut compound = Vec::new();
        if let Some(t) = self.type_selector()? {
            compound.push(t);
        }
        while let Some(item) = self.peek(0) {
            match item {
                ComponentValue::Block(block) if block.kind == BlockKind::Square => {
                    compound.push(Simple::Attribute(self.attribute(&block.value)?));
                    self.pos += 1;
                }
                ComponentValue::Token(t) => match &t.kind {
                    TokenKind::Hash { value, id } => {
                        if !id {
                            return Err(SelectorError);
                        }
                        compound.push(Simple::Id(value.clone()));
                        self.pos += 1;
                    }
                    TokenKind::Delim('.') => {
                        let Some(TokenKind::Ident(class)) = self.peek_token(1) else {
                            return Err(SelectorError);
                        };
                        compound.push(Simple::Class(class.clone()));
                        self.pos += 2;
                    }
                    TokenKind::Delim('&') => {
                        compound.push(Simple::Nesting);
                        self.pos += 1;
                    }
                    TokenKind::Colon => {
                        self.pos += 1;
                        match self.pseudo()? {
                            Pseudo::Class(pc) => compound.push(Simple::PseudoClass(pc)),
                            Pseudo::Element(pe) => {
                                if !self.ctx.pseudo_elements {
                                    return Err(SelectorError);
                                }
                                return Ok((compound, Some(pe)));
                            }
                        }
                    }
                    _ => break,
                },
                _ => break,
            }
        }
        if compound.is_empty() {
            return Err(SelectorError);
        }
        Ok((compound, None))
    }

    /// Resuelve un prefijo de namespace (`None` = sin prefijo explícito).
    fn element_ns(&self, prefix: Option<&TokenKind>) -> Result<NsConstraint> {
        Ok(match prefix {
            None => match &self.ns.default {
                Some(url) => NsConstraint::Url(url.clone()),
                None => NsConstraint::Any,
            },
            Some(TokenKind::Delim('*')) => NsConstraint::Any,
            Some(TokenKind::Delim('|')) => NsConstraint::None,
            Some(TokenKind::Ident(p)) => {
                NsConstraint::Url(self.ns.prefixes.get(p).ok_or(SelectorError)?.clone())
            }
            Some(_) => unreachable!(),
        })
    }

    /// `<type-selector>` (`E`, `*`, `ns|E`, `*|E`, `|E`, ...), si lo hay.
    fn type_selector(&mut self) -> Result<Option<Simple>> {
        let name_like =
            |t: Option<&TokenKind>| matches!(t, Some(TokenKind::Ident(_) | TokenKind::Delim('*')));
        let (prefix, name) = if name_like(self.peek_token(0))
            && self.peek_token(1) == Some(&TokenKind::Delim('|'))
            && name_like(self.peek_token(2))
        {
            let prefix = self.peek_token(0);
            self.pos += 3;
            (
                Some(prefix.expect("prefijo")),
                self.items[self.pos - 1].token(),
            )
        } else if self.peek_token(0) == Some(&TokenKind::Delim('|'))
            && name_like(self.peek_token(1))
        {
            self.pos += 2;
            (
                Some(&TokenKind::Delim('|')),
                self.items[self.pos - 1].token(),
            )
        } else if name_like(self.peek_token(0)) {
            self.pos += 1;
            (None, self.items[self.pos - 1].token())
        } else {
            return Ok(None);
        };
        let ns = self.element_ns(prefix)?;
        Ok(Some(match name {
            Some(TokenKind::Ident(n)) => Simple::Type {
                ns,
                lower: n.to_ascii_lowercase(),
                name: n.clone(),
            },
            _ => Simple::Universal(ns),
        }))
    }

    /// `<attribute-selector>` a partir del contenido del `[]`-block.
    fn attribute(&self, values: &'a [ComponentValue]) -> Result<AttrSelector> {
        let mut p = Parser {
            items: values,
            pos: 0,
            ns: self.ns,
            ctx: self.ctx,
        };
        p.skip_whitespace();
        let (ns, name) = match (p.peek_token(0), p.peek_token(1), p.peek_token(2)) {
            (
                Some(TokenKind::Ident(prefix)),
                Some(TokenKind::Delim('|')),
                Some(TokenKind::Ident(n)),
            ) => {
                p.pos += 3;
                let url = self.ns.prefixes.get(prefix).ok_or(SelectorError)?;
                (AttrNs::Url(url.clone()), n.clone())
            }
            (
                Some(TokenKind::Delim('*')),
                Some(TokenKind::Delim('|')),
                Some(TokenKind::Ident(n)),
            ) => {
                p.pos += 3;
                (AttrNs::Any, n.clone())
            }
            (Some(TokenKind::Delim('|')), Some(TokenKind::Ident(n)), _) => {
                p.pos += 2;
                (AttrNs::None, n.clone())
            }
            (Some(TokenKind::Ident(n)), _, _) => {
                p.pos += 1;
                (AttrNs::None, n.clone())
            }
            _ => return Err(SelectorError),
        };
        p.skip_whitespace();
        if p.at_end() {
            return Ok(AttrSelector {
                ns,
                name,
                matcher: None,
            });
        }
        let op = match (p.peek_token(0), p.peek_token(1)) {
            (Some(TokenKind::Delim('=')), _) => {
                p.pos += 1;
                AttrOp::Equals
            }
            (Some(TokenKind::Delim(c)), Some(TokenKind::Delim('='))) => {
                let op = match c {
                    '~' => AttrOp::Includes,
                    '|' => AttrOp::DashMatch,
                    '^' => AttrOp::Prefix,
                    '$' => AttrOp::Suffix,
                    '*' => AttrOp::Substring,
                    _ => return Err(SelectorError),
                };
                p.pos += 2;
                op
            }
            _ => return Err(SelectorError),
        };
        p.skip_whitespace();
        let value = match p.peek_token(0) {
            Some(TokenKind::Ident(v) | TokenKind::String(v)) => v.clone(),
            _ => return Err(SelectorError),
        };
        p.pos += 1;
        p.skip_whitespace();
        let mut flag = CaseFlag::Default;
        if let Some(TokenKind::Ident(f)) = p.peek_token(0) {
            flag = if f.eq_ignore_ascii_case("i") {
                CaseFlag::Insensitive
            } else if f.eq_ignore_ascii_case("s") {
                CaseFlag::Sensitive
            } else {
                return Err(SelectorError);
            };
            p.pos += 1;
            p.skip_whitespace();
        }
        if !p.at_end() {
            return Err(SelectorError);
        }
        Ok(AttrSelector {
            ns,
            name,
            matcher: Some((op, value, flag)),
        })
    }

    /// Lo que sigue a un `:` (ya consumido).
    fn pseudo(&mut self) -> Result<Pseudo> {
        let item = self.peek(0).ok_or(SelectorError)?;
        self.pos += 1;
        match item {
            ComponentValue::Token(t) => match &t.kind {
                TokenKind::Colon => {
                    let Some(TokenKind::Ident(name)) = self.peek_token(0) else {
                        return Err(SelectorError);
                    };
                    self.pos += 1;
                    pseudo_element(name)
                        .map(Pseudo::Element)
                        .ok_or(SelectorError)
                }
                TokenKind::Ident(name) => {
                    let lower = name.to_ascii_lowercase();
                    // Pseudo-elementos legacy con un solo ':'.
                    if matches!(
                        lower.as_str(),
                        "before" | "after" | "first-line" | "first-letter"
                    ) {
                        return Ok(Pseudo::Element(pseudo_element(&lower).expect("legacy")));
                    }
                    pseudo_class(&lower).map(Pseudo::Class).ok_or(SelectorError)
                }
                _ => Err(SelectorError),
            },
            ComponentValue::Function(f) => self
                .functional_pseudo_class(&f.name.to_ascii_lowercase(), &f.value)
                .map(Pseudo::Class),
            ComponentValue::Block(_) => Err(SelectorError),
        }
    }

    fn functional_pseudo_class(&self, name: &str, args: &[ComponentValue]) -> Result<PseudoClass> {
        let inner = Ctx {
            pseudo_elements: false,
            in_has: self.ctx.in_has,
            relative: Relative::No,
        };
        Ok(match name {
            "not" => PseudoClass::Not(parse_list(args, self.ns, inner)?),
            "is" => PseudoClass::Is(parse_forgiving(args, self.ns, inner)),
            "where" => PseudoClass::Where(parse_forgiving(args, self.ns, inner)),
            "has" => {
                if self.ctx.in_has {
                    return Err(SelectorError);
                }
                PseudoClass::Has(parse_list(
                    args,
                    self.ns,
                    Ctx {
                        pseudo_elements: false,
                        in_has: true,
                        relative: Relative::Has,
                    },
                )?)
            }
            "nth-child" | "nth-last-child" => {
                let of_index = args.iter().position(
                    |v| matches!(v.token(), Some(TokenKind::Ident(i)) if i.eq_ignore_ascii_case("of")),
                );
                let (anb, of) = match of_index {
                    Some(i) => {
                        // "of" debe ir separado por whitespace del An+B.
                        if !args[..i].last().is_some_and(ComponentValue::is_whitespace) {
                            return Err(SelectorError);
                        }
                        (
                            &args[..i],
                            Some(parse_list(&args[i + 1..], self.ns, inner)?),
                        )
                    }
                    None => (args, None),
                };
                let (a, b) = parse_anb(anb).ok_or(SelectorError)?;
                if name == "nth-child" {
                    PseudoClass::NthChild { a, b, of }
                } else {
                    PseudoClass::NthLastChild { a, b, of }
                }
            }
            "nth-of-type" | "nth-last-of-type" => {
                let (a, b) = parse_anb(args).ok_or(SelectorError)?;
                if name == "nth-of-type" {
                    PseudoClass::NthOfType { a, b }
                } else {
                    PseudoClass::NthLastOfType { a, b }
                }
            }
            "lang" => {
                let mut ranges = Vec::new();
                for group in split_comma_separated(args) {
                    let mut values = group.iter().filter(|v| !v.is_whitespace());
                    match (values.next().and_then(ComponentValue::token), values.next()) {
                        (Some(TokenKind::Ident(v) | TokenKind::String(v)), None) => {
                            ranges.push(v.clone());
                        }
                        _ => return Err(SelectorError),
                    }
                }
                if ranges.is_empty() {
                    return Err(SelectorError);
                }
                PseudoClass::Lang(ranges)
            }
            "heading" => {
                let mut levels = Vec::new();
                for group in split_comma_separated(args) {
                    let mut values = group.iter().filter(|v| !v.is_whitespace());
                    match (values.next().and_then(ComponentValue::token), values.next()) {
                        (Some(TokenKind::Number(n)), None) => {
                            levels.push(n.as_integer().ok_or(SelectorError)?);
                        }
                        _ => return Err(SelectorError),
                    }
                }
                if levels.is_empty() {
                    return Err(SelectorError);
                }
                PseudoClass::Heading(levels)
            }
            "dir" => {
                let mut values = args.iter().filter(|v| !v.is_whitespace());
                match (values.next().and_then(ComponentValue::token), values.next()) {
                    (Some(TokenKind::Ident(v)), None) if v.eq_ignore_ascii_case("ltr") => {
                        PseudoClass::Dir(Dir::Ltr)
                    }
                    (Some(TokenKind::Ident(v)), None) if v.eq_ignore_ascii_case("rtl") => {
                        PseudoClass::Dir(Dir::Rtl)
                    }
                    _ => return Err(SelectorError),
                }
            }
            _ => return Err(SelectorError),
        })
    }
}

enum Pseudo {
    Class(PseudoClass),
    Element(PseudoElement),
}

fn pseudo_element(name: &str) -> Option<PseudoElement> {
    Some(match name.to_ascii_lowercase().as_str() {
        "before" => PseudoElement::Before,
        "after" => PseudoElement::After,
        "first-line" => PseudoElement::FirstLine,
        "first-letter" => PseudoElement::FirstLetter,
        "marker" => PseudoElement::Marker,
        "placeholder" => PseudoElement::Placeholder,
        "selection" => PseudoElement::Selection,
        _ => return None,
    })
}

fn pseudo_class(lower: &str) -> Option<PseudoClass> {
    Some(match lower {
        "first-child" => PseudoClass::FirstChild,
        "last-child" => PseudoClass::LastChild,
        "only-child" => PseudoClass::OnlyChild,
        "first-of-type" => PseudoClass::FirstOfType,
        "last-of-type" => PseudoClass::LastOfType,
        "only-of-type" => PseudoClass::OnlyOfType,
        "root" => PseudoClass::Root,
        "empty" => PseudoClass::Empty,
        "link" => PseudoClass::Link,
        "any-link" => PseudoClass::AnyLink,
        "visited" => PseudoClass::Visited,
        "hover" => PseudoClass::Hover,
        "active" => PseudoClass::Active,
        "focus" => PseudoClass::Focus,
        "focus-visible" => PseudoClass::FocusVisible,
        "focus-within" => PseudoClass::FocusWithin,
        "target" => PseudoClass::Target,
        "checked" => PseudoClass::Checked,
        "disabled" => PseudoClass::Disabled,
        "enabled" => PseudoClass::Enabled,
        "required" => PseudoClass::Required,
        "optional" => PseudoClass::Optional,
        "read-only" => PseudoClass::ReadOnly,
        "read-write" => PseudoClass::ReadWrite,
        "placeholder-shown" => PseudoClass::PlaceholderShown,
        "scope" => PseudoClass::Scope,
        "defined" => PseudoClass::Defined,
        "heading" => PseudoClass::Heading(Vec::new()),
        _ => return None,
    })
}
