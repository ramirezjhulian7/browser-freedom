//! Media Queries Level 4: parser desde component values y evaluación contra un
//! [`MediaContext`] con lógica de tres valores (true / false / unknown).

use std::fmt;

use crate::syntax::{
    BlockKind, ComponentValue, TokenKind, parse_component_value_list, split_comma_separated,
};

/// Entorno contra el que se evalúan las media queries.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaContext {
    /// Media type del dispositivo: "screen" o "print".
    pub media_type: String,
    /// Tamaño del viewport en px CSS.
    pub width: f64,
    pub height: f64,
    pub device_pixel_ratio: f64,
    pub color_scheme: ColorScheme,
    pub reduced_motion: bool,
    pub hover: bool,
    pub any_hover: bool,
    pub pointer: Pointer,
    pub any_pointer: Pointer,
    /// Bits por componente de color (0 si el dispositivo no es a color).
    pub color_bits: u32,
    pub monochrome_bits: u32,
    /// Tamaño de fuente inicial, para `em`/`rem` (16px por ahora).
    pub font_size: f64,
}

impl Default for MediaContext {
    fn default() -> Self {
        MediaContext {
            media_type: "screen".into(),
            width: 1280.0,
            height: 720.0,
            device_pixel_ratio: 1.0,
            color_scheme: ColorScheme::Light,
            reduced_motion: false,
            hover: true,
            any_hover: true,
            pointer: Pointer::Fine,
            any_pointer: Pointer::Fine,
            color_bits: 8,
            monochrome_bits: 0,
            font_size: 16.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorScheme {
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pointer {
    None,
    Coarse,
    Fine,
}

/// `<media-query-list>`. Una lista vacía coincide con todo.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MediaQueryList(pub Vec<MediaQuery>);

#[derive(Debug, Clone, PartialEq)]
pub struct MediaQuery {
    pub qualifier: Option<Qualifier>,
    /// `None` cuando la query es solo una `<media-condition>` (equivale a `all`).
    pub media_type: Option<String>,
    pub condition: Option<MediaCondition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Qualifier {
    Not,
    Only,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MediaCondition {
    Not(Box<MediaCondition>),
    And(Vec<MediaCondition>),
    Or(Vec<MediaCondition>),
    Feature(MediaFeature),
    /// `<general-enclosed>` o una media feature desconocida: evalúa a "unknown".
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaFeature {
    /// Nombre en minúsculas, sin prefijo `min-`/`max-`.
    pub name: String,
    pub test: FeatureTest,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FeatureTest {
    /// `(name)`.
    Boolean,
    /// `(name: value)`.
    Equals(MfValue),
    /// Formas de rango (incluidas `min-`/`max-`): `feature op value` para cada par.
    Range(Vec<(Cmp, MfValue)>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MfValue {
    Number(f64),
    Dimension(f64, String),
    Ident(String),
    Ratio(f64, f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kleene {
    True,
    False,
    Unknown,
}

impl Kleene {
    fn from_bool(b: bool) -> Kleene {
        if b { Kleene::True } else { Kleene::False }
    }
    fn not(self) -> Kleene {
        match self {
            Kleene::True => Kleene::False,
            Kleene::False => Kleene::True,
            Kleene::Unknown => Kleene::Unknown,
        }
    }
}

impl MediaQuery {
    /// La query que resulta de una query inválida: `not all`.
    pub fn not_all() -> Self {
        MediaQuery {
            qualifier: Some(Qualifier::Not),
            media_type: Some("all".into()),
            condition: None,
        }
    }

    pub fn evaluate(&self, ctx: &MediaContext) -> bool {
        let type_matches = match self.media_type.as_deref() {
            None | Some("all") => true,
            Some(t) => t == ctx.media_type,
        };
        let condition = self
            .condition
            .as_ref()
            .map_or(Kleene::True, |c| c.evaluate(ctx));
        let result = if type_matches {
            condition
        } else {
            Kleene::False
        };
        // "unknown" se trata como false antes de aplicar `not`.
        let result = result == Kleene::True;
        if self.qualifier == Some(Qualifier::Not) {
            !result
        } else {
            result
        }
    }
}

impl MediaQueryList {
    pub fn evaluate(&self, ctx: &MediaContext) -> bool {
        self.0.is_empty() || self.0.iter().any(|q| q.evaluate(ctx))
    }
}

impl MediaCondition {
    fn evaluate(&self, ctx: &MediaContext) -> Kleene {
        match self {
            MediaCondition::Not(c) => c.evaluate(ctx).not(),
            MediaCondition::And(cs) => {
                cs.iter()
                    .fold(Kleene::True, |acc, c| match (acc, c.evaluate(ctx)) {
                        (Kleene::False, _) | (_, Kleene::False) => Kleene::False,
                        (Kleene::Unknown, _) | (_, Kleene::Unknown) => Kleene::Unknown,
                        _ => Kleene::True,
                    })
            }
            MediaCondition::Or(cs) => {
                cs.iter()
                    .fold(Kleene::False, |acc, c| match (acc, c.evaluate(ctx)) {
                        (Kleene::True, _) | (_, Kleene::True) => Kleene::True,
                        (Kleene::Unknown, _) | (_, Kleene::Unknown) => Kleene::Unknown,
                        _ => Kleene::False,
                    })
            }
            MediaCondition::Feature(f) => f.evaluate(ctx),
            MediaCondition::Unknown => Kleene::Unknown,
        }
    }
}

// --- Evaluación de features ---

#[derive(Debug, Clone, Copy, PartialEq)]
enum Actual {
    Number(f64),
    Ident(&'static str),
}

impl MediaFeature {
    fn evaluate(&self, ctx: &MediaContext) -> Kleene {
        let Some((actual, range)) = actual_value(&self.name, ctx) else {
            return Kleene::Unknown;
        };
        match &self.test {
            FeatureTest::Boolean => Kleene::from_bool(match actual {
                Actual::Number(n) => n != 0.0,
                Actual::Ident(i) => i != "none" && i != "no-preference",
            }),
            FeatureTest::Equals(value) => match (actual, value) {
                (Actual::Ident(a), MfValue::Ident(v)) => {
                    Kleene::from_bool(v.eq_ignore_ascii_case(a))
                }
                (Actual::Number(a), v) if range => match resolve(&self.name, v, ctx) {
                    Some(v) => Kleene::from_bool((a - v).abs() < 1e-9),
                    None => Kleene::Unknown,
                },
                _ => Kleene::Unknown,
            },
            FeatureTest::Range(tests) => {
                let Actual::Number(a) = actual else {
                    return Kleene::Unknown;
                };
                if !range {
                    return Kleene::Unknown;
                }
                let mut result = true;
                for (cmp, value) in tests {
                    let Some(v) = resolve(&self.name, value, ctx) else {
                        return Kleene::Unknown;
                    };
                    result &= match cmp {
                        Cmp::Lt => a < v,
                        Cmp::Le => a <= v,
                        Cmp::Gt => a > v,
                        Cmp::Ge => a >= v,
                        Cmp::Eq => (a - v).abs() < 1e-9,
                    };
                }
                Kleene::from_bool(result)
            }
        }
    }
}

/// Valor del entorno para la feature `name` y si es una feature de rango.
fn actual_value(name: &str, ctx: &MediaContext) -> Option<(Actual, bool)> {
    let pointer = |p: Pointer| {
        Actual::Ident(match p {
            Pointer::None => "none",
            Pointer::Coarse => "coarse",
            Pointer::Fine => "fine",
        })
    };
    Some(match name {
        "width" => (Actual::Number(ctx.width), true),
        "height" => (Actual::Number(ctx.height), true),
        "aspect-ratio" => (Actual::Number(ctx.width / ctx.height), true),
        "resolution" => (Actual::Number(ctx.device_pixel_ratio), true),
        "color" => (Actual::Number(f64::from(ctx.color_bits)), true),
        "monochrome" => (Actual::Number(f64::from(ctx.monochrome_bits)), true),
        "grid" => (Actual::Number(0.0), false),
        "orientation" => (
            Actual::Ident(if ctx.height >= ctx.width {
                "portrait"
            } else {
                "landscape"
            }),
            false,
        ),
        "prefers-color-scheme" => (
            Actual::Ident(match ctx.color_scheme {
                ColorScheme::Light => "light",
                ColorScheme::Dark => "dark",
            }),
            false,
        ),
        "prefers-reduced-motion" => (
            Actual::Ident(if ctx.reduced_motion {
                "reduce"
            } else {
                "no-preference"
            }),
            false,
        ),
        "hover" => (
            Actual::Ident(if ctx.hover { "hover" } else { "none" }),
            false,
        ),
        "any-hover" => (
            Actual::Ident(if ctx.any_hover { "hover" } else { "none" }),
            false,
        ),
        "pointer" => (pointer(ctx.pointer), false),
        "any-pointer" => (pointer(ctx.any_pointer), false),
        _ => return None,
    })
}

/// Convierte un `<mf-value>` a las unidades canónicas de la feature (px, dppx o ratio).
fn resolve(feature: &str, value: &MfValue, ctx: &MediaContext) -> Option<f64> {
    match feature {
        "width" | "height" => match value {
            MfValue::Number(n) if *n == 0.0 => Some(0.0),
            MfValue::Dimension(n, unit) => length_px(*n, unit, ctx),
            _ => None,
        },
        "aspect-ratio" => match value {
            MfValue::Ratio(a, b) => Some(a / b),
            MfValue::Number(n) => Some(*n),
            _ => None,
        },
        "resolution" => match value {
            MfValue::Dimension(n, unit) => match unit.to_ascii_lowercase().as_str() {
                "dppx" | "x" => Some(*n),
                "dpi" => Some(n / 96.0),
                "dpcm" => Some(n * 2.54 / 96.0),
                _ => None,
            },
            _ => None,
        },
        "color" | "monochrome" => match value {
            MfValue::Number(n) => Some(*n),
            _ => None,
        },
        _ => None,
    }
}

/// Longitud en px CSS. `em`/`rem` usan el tamaño de fuente inicial del contexto.
pub fn length_px(n: f64, unit: &str, ctx: &MediaContext) -> Option<f64> {
    Some(match unit.to_ascii_lowercase().as_str() {
        "px" => n,
        "em" | "rem" => n * ctx.font_size,
        "ex" | "ch" => n * ctx.font_size / 2.0,
        "in" => n * 96.0,
        "cm" => n * 96.0 / 2.54,
        "mm" => n * 96.0 / 25.4,
        "q" => n * 96.0 / 101.6,
        "pt" => n * 96.0 / 72.0,
        "pc" => n * 16.0,
        "vw" => n * ctx.width / 100.0,
        "vh" => n * ctx.height / 100.0,
        "vmin" => n * ctx.width.min(ctx.height) / 100.0,
        "vmax" => n * ctx.width.max(ctx.height) / 100.0,
        _ => return None,
    })
}

/// Features que aceptan los prefijos `min-`/`max-` y la sintaxis de rango.
fn is_range_feature(name: &str) -> bool {
    matches!(
        name,
        "width" | "height" | "aspect-ratio" | "resolution" | "color" | "monochrome"
    )
}

// --- Parser ---

/// Parsea texto como `<media-query-list>`.
pub fn parse_media_query_list(input: &str) -> MediaQueryList {
    parse_media_query_list_from(&parse_component_value_list(input))
}

/// Parsea component values como `<media-query-list>`. Cada query inválida se convierte en
/// `not all`.
pub fn parse_media_query_list_from(values: &[ComponentValue]) -> MediaQueryList {
    if values.iter().all(ComponentValue::is_whitespace) {
        return MediaQueryList::default();
    }
    MediaQueryList(
        split_comma_separated(values)
            .iter()
            .map(|group| parse_media_query(group).unwrap_or_else(MediaQuery::not_all))
            .collect(),
    )
}

fn ident(v: &ComponentValue) -> Option<&str> {
    match v.token() {
        Some(TokenKind::Ident(s)) => Some(s),
        _ => None,
    }
}

fn is_keyword(v: &ComponentValue, kw: &str) -> bool {
    ident(v).is_some_and(|s| s.eq_ignore_ascii_case(kw))
}

fn is_in_parens(v: &ComponentValue) -> bool {
    matches!(v, ComponentValue::Function(_))
        || matches!(v, ComponentValue::Block(b) if b.kind == BlockKind::Paren)
}

fn parse_media_query(values: &[ComponentValue]) -> Option<MediaQuery> {
    let items: Vec<&ComponentValue> = values.iter().filter(|v| !v.is_whitespace()).collect();
    let first = *items.first()?;
    // <media-condition>
    if is_in_parens(first)
        || (is_keyword(first, "not") && items.get(1).is_some_and(|v| is_in_parens(v)))
    {
        let condition = parse_condition(&items, true)?;
        return Some(MediaQuery {
            qualifier: None,
            media_type: None,
            condition: Some(condition),
        });
    }
    // [ not | only ]? <media-type> [ and <media-condition-without-or> ]?
    let mut i = 0;
    let qualifier = if is_keyword(first, "not") {
        i += 1;
        Some(Qualifier::Not)
    } else if is_keyword(first, "only") {
        i += 1;
        Some(Qualifier::Only)
    } else {
        None
    };
    let media_type = ident(items.get(i)?)?.to_ascii_lowercase();
    if matches!(media_type.as_str(), "only" | "not" | "and" | "or" | "layer") {
        return None;
    }
    i += 1;
    let condition = if i < items.len() {
        if !is_keyword(items[i], "and") {
            return None;
        }
        Some(parse_condition(&items[i + 1..], false)?)
    } else {
        None
    };
    Some(MediaQuery {
        qualifier,
        media_type: Some(media_type),
        condition,
    })
}

/// `<media-condition>` (o `<media-condition-without-or>` si `!allow_or`).
fn parse_condition(items: &[&ComponentValue], allow_or: bool) -> Option<MediaCondition> {
    let first = *items.first()?;
    if is_keyword(first, "not") {
        if items.len() != 2 {
            return None;
        }
        return Some(MediaCondition::Not(Box::new(parse_in_parens(items[1])?)));
    }
    let mut conditions = vec![parse_in_parens(first)?];
    let mut op: Option<&str> = None;
    let mut rest = &items[1..];
    while let [keyword, operand, tail @ ..] = rest {
        let kw = ident(keyword)?.to_ascii_lowercase();
        if !matches!(kw.as_str(), "and" | "or") || (kw == "or" && !allow_or) {
            return None;
        }
        if op.is_some_and(|o| o != kw) {
            return None;
        }
        op = Some(if kw == "and" { "and" } else { "or" });
        conditions.push(parse_in_parens(operand)?);
        rest = tail;
    }
    if !rest.is_empty() {
        return None;
    }
    Some(match op {
        None => conditions.pop().expect("una condición"),
        Some("and") => MediaCondition::And(conditions),
        Some(_) => MediaCondition::Or(conditions),
    })
}

/// `<media-in-parens>`: lo que no es una condición ni una feature válida es
/// `<general-enclosed>` (unknown).
fn parse_in_parens(value: &ComponentValue) -> Option<MediaCondition> {
    let ComponentValue::Block(block) = value else {
        // Función: <general-enclosed>.
        return is_in_parens(value).then_some(MediaCondition::Unknown);
    };
    if block.kind != BlockKind::Paren {
        return None;
    }
    let inner: Vec<&ComponentValue> = block.value.iter().filter(|v| !v.is_whitespace()).collect();
    if let Some(condition) = parse_condition(&inner, true) {
        return Some(condition);
    }
    Some(parse_feature(&block.value).map_or(MediaCondition::Unknown, MediaCondition::Feature))
}

/// `<media-feature>` desde el contenido de un `()`-block.
fn parse_feature(values: &[ComponentValue]) -> Option<MediaFeature> {
    // Tokens no-whitespace; los operadores de dos caracteres (`<=`, `>=`) deben ir juntos.
    let mut items: Vec<&ComponentValue> = Vec::new();
    let mut ops_adjacent: Vec<bool> = Vec::new();
    let mut prev_ws = false;
    for v in values {
        if v.is_whitespace() {
            prev_ws = true;
            continue;
        }
        ops_adjacent.push(!prev_ws);
        items.push(v);
        prev_ws = false;
    }
    match items.as_slice() {
        [name] => {
            let name = ident(name)?.to_ascii_lowercase();
            Some(MediaFeature {
                name,
                test: FeatureTest::Boolean,
            })
        }
        [name, colon, value @ ..] if colon.token() == Some(&TokenKind::Colon) => {
            let raw = ident(name)?.to_ascii_lowercase();
            let value = parse_mf_value(value)?;
            let (name, test) = if let Some(n) = raw.strip_prefix("min-") {
                (n.to_owned(), FeatureTest::Range(vec![(Cmp::Ge, value)]))
            } else if let Some(n) = raw.strip_prefix("max-") {
                (n.to_owned(), FeatureTest::Range(vec![(Cmp::Le, value)]))
            } else {
                (raw, FeatureTest::Equals(value))
            };
            if matches!(test, FeatureTest::Range(_)) && !is_range_feature(&name) {
                return None;
            }
            Some(MediaFeature { name, test })
        }
        _ => parse_range(&items, &ops_adjacent),
    }
}

/// Lee un operador de comparación en `items[i..]`; devuelve el operador y cuántos tokens usa.
fn comparison(items: &[&ComponentValue], adjacent: &[bool], i: usize) -> Option<(Cmp, usize)> {
    let delim = |j: usize| match items.get(j).and_then(|v| v.token()) {
        Some(TokenKind::Delim(c)) => Some(*c),
        _ => None,
    };
    let eq_follows = delim(i + 1) == Some('=') && adjacent.get(i + 1) == Some(&true);
    match delim(i)? {
        '<' if eq_follows => Some((Cmp::Le, 2)),
        '>' if eq_follows => Some((Cmp::Ge, 2)),
        '<' => Some((Cmp::Lt, 1)),
        '>' => Some((Cmp::Gt, 1)),
        '=' => Some((Cmp::Eq, 1)),
        _ => None,
    }
}

fn flip(c: Cmp) -> Cmp {
    match c {
        Cmp::Lt => Cmp::Gt,
        Cmp::Le => Cmp::Ge,
        Cmp::Gt => Cmp::Lt,
        Cmp::Ge => Cmp::Le,
        Cmp::Eq => Cmp::Eq,
    }
}

/// `<mf-range>`.
fn parse_range(items: &[&ComponentValue], adjacent: &[bool]) -> Option<MediaFeature> {
    // Separa en operandos y operadores.
    let mut operands: Vec<Vec<&ComponentValue>> = vec![Vec::new()];
    let mut ops = Vec::new();
    let mut i = 0;
    while i < items.len() {
        if let Some((cmp, len)) = comparison(items, adjacent, i) {
            ops.push(cmp);
            operands.push(Vec::new());
            i += len;
        } else {
            operands.last_mut().expect("operando").push(items[i]);
            i += 1;
        }
    }
    let as_name = |o: &[&ComponentValue]| match o {
        [v] => ident(v).map(str::to_ascii_lowercase),
        _ => None,
    };
    let owned = |o: &[&ComponentValue]| o.iter().map(|v| (*v).clone()).collect::<Vec<_>>();
    match (operands.as_slice(), ops.as_slice()) {
        ([a, b], [op]) => {
            if let Some(name) = as_name(a)
                && is_range_feature(&name)
            {
                let value = parse_mf_value_refs(&owned(b))?;
                return Some(MediaFeature {
                    name,
                    test: FeatureTest::Range(vec![(*op, value)]),
                });
            }
            let name = as_name(b)?;
            let value = parse_mf_value_refs(&owned(a))?;
            is_range_feature(&name).then(|| MediaFeature {
                name,
                test: FeatureTest::Range(vec![(flip(*op), value)]),
            })
        }
        ([a, n, b], [op1, op2]) => {
            let same_direction = matches!((op1, op2), (Cmp::Lt | Cmp::Le, Cmp::Lt | Cmp::Le))
                || matches!((op1, op2), (Cmp::Gt | Cmp::Ge, Cmp::Gt | Cmp::Ge));
            if !same_direction {
                return None;
            }
            let name = as_name(n)?;
            let low = parse_mf_value_refs(&owned(a))?;
            let high = parse_mf_value_refs(&owned(b))?;
            is_range_feature(&name).then(|| MediaFeature {
                name,
                test: FeatureTest::Range(vec![(flip(*op1), low), (*op2, high)]),
            })
        }
        _ => None,
    }
}

fn parse_mf_value_refs(values: &[ComponentValue]) -> Option<MfValue> {
    let refs: Vec<&ComponentValue> = values.iter().collect();
    parse_mf_value(&refs)
}

/// `<mf-value>`: `<number>`, `<dimension>`, `<ident>` o `<ratio>`.
fn parse_mf_value(items: &[&ComponentValue]) -> Option<MfValue> {
    let items: Vec<&&ComponentValue> = items.iter().filter(|v| !v.is_whitespace()).collect();
    let number = |v: &ComponentValue| match v.token() {
        Some(TokenKind::Number(n)) => Some(n.value),
        _ => None,
    };
    match items.as_slice() {
        [v] => match v.token()? {
            TokenKind::Number(n) => Some(MfValue::Number(n.value)),
            TokenKind::Dimension(n, unit) => Some(MfValue::Dimension(n.value, unit.clone())),
            TokenKind::Ident(i) => Some(MfValue::Ident(i.to_ascii_lowercase())),
            _ => None,
        },
        [a, slash, b] if slash.token() == Some(&TokenKind::Delim('/')) => {
            let (a, b) = (number(a)?, number(b)?);
            (a >= 0.0 && b >= 0.0).then_some(MfValue::Ratio(a, b))
        }
        _ => None,
    }
}

// --- Serialización (para volcados) ---

impl fmt::Display for MediaQueryList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, q) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{q}")?;
        }
        Ok(())
    }
}

impl fmt::Display for MediaQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.qualifier {
            Some(Qualifier::Not) => f.write_str("not ")?,
            Some(Qualifier::Only) => f.write_str("only ")?,
            None => {}
        }
        if let Some(t) = &self.media_type {
            f.write_str(t)?;
            if self.condition.is_some() {
                f.write_str(" and ")?;
            }
        }
        if let Some(c) = &self.condition {
            write!(f, "{c}")?;
        }
        Ok(())
    }
}

impl fmt::Display for MediaCondition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let join = |f: &mut fmt::Formatter<'_>, cs: &[MediaCondition], op: &str| {
            for (i, c) in cs.iter().enumerate() {
                if i > 0 {
                    write!(f, " {op} ")?;
                }
                write!(f, "{c}")?;
            }
            Ok(())
        };
        match self {
            MediaCondition::Not(c) => write!(f, "not {c}"),
            MediaCondition::And(cs) => join(f, cs, "and"),
            MediaCondition::Or(cs) => join(f, cs, "or"),
            MediaCondition::Feature(feat) => {
                write!(f, "({}", feat.name)?;
                match &feat.test {
                    FeatureTest::Boolean => {}
                    FeatureTest::Equals(v) => write!(f, ": {v}")?,
                    FeatureTest::Range(tests) => {
                        for (i, (cmp, v)) in tests.iter().enumerate() {
                            let op = match cmp {
                                Cmp::Lt => "<",
                                Cmp::Le => "<=",
                                Cmp::Gt => ">",
                                Cmp::Ge => ">=",
                                Cmp::Eq => "=",
                            };
                            if i > 0 {
                                write!(f, ") and ({}", feat.name)?;
                            }
                            write!(f, " {op} {v}")?;
                        }
                    }
                }
                f.write_str(")")
            }
            MediaCondition::Unknown => f.write_str("(<unknown>)"),
        }
    }
}

impl fmt::Display for MfValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MfValue::Number(n) => write!(f, "{n}"),
            MfValue::Dimension(n, unit) => write!(f, "{n}{unit}"),
            MfValue::Ident(i) => f.write_str(i),
            MfValue::Ratio(a, b) => write!(f, "{a}/{b}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(query: &str, ctx: &MediaContext) -> bool {
        parse_media_query_list(query).evaluate(ctx)
    }

    #[test]
    fn media_queries() {
        let ctx = MediaContext {
            width: 600.0,
            height: 800.0,
            ..MediaContext::default()
        };
        let cases: &[(&str, bool)] = &[
            ("", true),
            ("all", true),
            ("screen", true),
            ("print", false),
            ("not print", true),
            ("only screen and (min-width: 500px)", true),
            ("screen and (max-width: 500px)", false),
            ("(400px <= width <= 800px)", true),
            ("(800px < width)", false),
            ("(width > 30em)", true),
            ("(width >= 600px) and (height = 800px)", true),
            ("(orientation: portrait)", true),
            ("(orientation: landscape)", false),
            ("(prefers-color-scheme: dark)", false),
            ("(prefers-color-scheme: light)", true),
            ("(hover: hover) and (pointer: fine)", true),
            ("(pointer: coarse) or (hover)", true),
            ("not (color)", false),
            ("(aspect-ratio: 3/4)", true),
            ("(min-aspect-ratio: 1/1)", false),
            ("(resolution: 1dppx)", true),
            ("(min-resolution: 2x)", false),
            ("(unknown-feature)", false),
            ("not (unknown-feature)", false),
            ("(color) and (unknown-feature)", false),
            ("(color) or (unknown-feature)", true),
            ("screen and (width > 100px) or (height > 1px)", false),
            ("print, (min-width: 100px)", true),
            ("garbage here", false),
            ("not all", false),
            ("tv", false),
            ("(min-orientation: portrait)", false),
        ];
        for (query, expected) in cases {
            assert_eq!(eval(query, &ctx), *expected, "{query}");
        }
    }
}
