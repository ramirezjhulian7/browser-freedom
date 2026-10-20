//! Tipos de las propiedades de CSS Grid Layout 2 (§7 y §8): listas de pistas, áreas con nombre,
//! `grid-auto-flow` y las líneas de colocación. `subgrid` y `masonry` no se admiten (roadmap).

use bf_css::syntax::{ComponentValue, TokenKind};

use super::{ComputeContext, PropValue};
use crate::values::{
    LengthPercentage, components, ident, integer, is_ident, parse_length_percentage, trim,
};

/// `<track-breadth>` (o el `<inflexible-breadth>` de `minmax()`).
#[derive(Debug, Clone, PartialEq)]
pub enum TrackBreadth {
    Length(LengthPercentage),
    /// `<flex>` en `fr`.
    Flex(f32),
    MinContent,
    MaxContent,
    Auto,
}

/// `<track-size>`.
#[derive(Debug, Clone, PartialEq)]
pub enum TrackSize {
    Breadth(TrackBreadth),
    MinMax(TrackBreadth, TrackBreadth),
    FitContent(LengthPercentage),
}

impl TrackSize {
    pub const AUTO: TrackSize = TrackSize::Breadth(TrackBreadth::Auto);

    /// Función de dimensionado mínima (§11.1 / §12.1).
    pub fn min(&self) -> &TrackBreadth {
        match self {
            TrackSize::Breadth(b) => match b {
                // Un `<flex>` como mínimo se trata como `auto`.
                TrackBreadth::Flex(_) => &TrackBreadth::Auto,
                b => b,
            },
            TrackSize::MinMax(a, _) => a,
            TrackSize::FitContent(_) => &TrackBreadth::Auto,
        }
    }

    /// Función de dimensionado máxima.
    pub fn max(&self) -> &TrackBreadth {
        match self {
            TrackSize::Breadth(b) | TrackSize::MinMax(_, b) => b,
            TrackSize::FitContent(_) => &TrackBreadth::MaxContent,
        }
    }

    fn compute(&self, ctx: &ComputeContext<'_>) -> TrackSize {
        let b = |b: &TrackBreadth| match b {
            TrackBreadth::Length(l) => TrackBreadth::Length(l.compute(&ctx.len)),
            other => other.clone(),
        };
        match self {
            TrackSize::Breadth(x) => TrackSize::Breadth(b(x)),
            TrackSize::MinMax(x, y) => TrackSize::MinMax(b(x), b(y)),
            TrackSize::FitContent(l) => TrackSize::FitContent(l.compute(&ctx.len)),
        }
    }

    fn to_css(&self) -> String {
        match self {
            TrackSize::Breadth(b) => breadth_css(b),
            TrackSize::MinMax(a, b) => format!("minmax({}, {})", breadth_css(a), breadth_css(b)),
            TrackSize::FitContent(l) => format!("fit-content({l})"),
        }
    }
}

fn breadth_css(b: &TrackBreadth) -> String {
    match b {
        TrackBreadth::Length(l) => l.to_string(),
        TrackBreadth::Flex(f) => format!("{}fr", crate::values::fmt_number(*f)),
        TrackBreadth::MinContent => "min-content".into(),
        TrackBreadth::MaxContent => "max-content".into(),
        TrackBreadth::Auto => "auto".into(),
    }
}

fn parse_breadth(v: &ComponentValue) -> Option<TrackBreadth> {
    if let Some(TokenKind::Dimension(n, unit)) = v.token()
        && unit.eq_ignore_ascii_case("fr")
    {
        return (n.value >= 0.0).then_some(TrackBreadth::Flex(n.value as f32));
    }
    if let Some(name) = ident(v) {
        return match name.to_ascii_lowercase().as_str() {
            "min-content" => Some(TrackBreadth::MinContent),
            "max-content" => Some(TrackBreadth::MaxContent),
            "auto" => Some(TrackBreadth::Auto),
            _ => None,
        };
    }
    let lp = parse_length_percentage(v)?;
    let negative = match &lp {
        LengthPercentage::Length(l) => l.value < 0.0,
        LengthPercentage::Percentage(p) => *p < 0.0,
        LengthPercentage::Calc(_) => false,
    };
    (!negative).then_some(TrackBreadth::Length(lp))
}

/// `<track-size>`.
pub fn parse_track_size(v: &ComponentValue) -> Option<TrackSize> {
    if let ComponentValue::Function(f) = v {
        let args = bf_css::syntax::split_comma_separated(&f.value);
        let one = |a: &[ComponentValue]| match components(a).as_slice() {
            [x] => Some((*x).clone()),
            _ => None,
        };
        return match f.name.to_ascii_lowercase().as_str() {
            "minmax" => {
                let [a, b] = args.as_slice() else {
                    return None;
                };
                let min = parse_breadth(&one(a)?)?;
                let max = parse_breadth(&one(b)?)?;
                // El mínimo no puede ser `<flex>`.
                if matches!(min, TrackBreadth::Flex(_)) {
                    return None;
                }
                Some(TrackSize::MinMax(min, max))
            }
            "fit-content" => {
                let [a] = args.as_slice() else {
                    return None;
                };
                match parse_breadth(&one(a)?)? {
                    TrackBreadth::Length(l) => Some(TrackSize::FitContent(l)),
                    _ => None,
                }
            }
            _ => parse_breadth(v).map(TrackSize::Breadth),
        };
    }
    parse_breadth(v).map(TrackSize::Breadth)
}

/// Repeticiones de `repeat()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatCount {
    Count(u32),
    AutoFill,
    AutoFit,
}

/// Un elemento de una lista de pistas.
#[derive(Debug, Clone, PartialEq)]
pub enum TrackListItem {
    /// `[nombres de línea]`.
    Names(Vec<String>),
    Track(TrackSize),
    /// `repeat()`: sus elementos son solo `Names` y `Track`.
    Repeat(RepeatCount, Vec<TrackListItem>),
}

/// `[ <custom-ident>* ]`.
fn parse_line_names(v: &ComponentValue) -> Option<Vec<String>> {
    let ComponentValue::Block(b) = v else {
        return None;
    };
    if b.kind != bf_css::syntax::BlockKind::Square {
        return None;
    }
    components(&b.value)
        .iter()
        .map(|n| {
            let name = ident(n)?;
            (!name.eq_ignore_ascii_case("span") && !name.eq_ignore_ascii_case("auto"))
                .then(|| name.to_owned())
        })
        .collect()
}

fn parse_repeat(v: &ComponentValue) -> Option<TrackListItem> {
    let ComponentValue::Function(f) = v else {
        return None;
    };
    if !f.name.eq_ignore_ascii_case("repeat") {
        return None;
    }
    let args = bf_css::syntax::split_comma_separated(&f.value);
    let [count, list] = args.as_slice() else {
        return None;
    };
    let count = match components(count).as_slice() {
        [c] if is_ident(c, "auto-fill") => RepeatCount::AutoFill,
        [c] if is_ident(c, "auto-fit") => RepeatCount::AutoFit,
        [c] => RepeatCount::Count(u32::try_from(integer(c)?).ok().filter(|&n| n >= 1)?),
        _ => return None,
    };
    let mut items = Vec::new();
    for v in components(list) {
        if let Some(n) = parse_line_names(v) {
            items.push(TrackListItem::Names(n));
        } else {
            items.push(TrackListItem::Track(parse_track_size(v)?));
        }
    }
    if !items.iter().any(|i| matches!(i, TrackListItem::Track(_))) {
        return None;
    }
    Some(TrackListItem::Repeat(count, items))
}

/// `<track-list> | <auto-track-list>`.
pub fn parse_track_list(input: &[ComponentValue]) -> Option<Vec<TrackListItem>> {
    let mut items = Vec::new();
    let mut autos = 0;
    for v in components(input) {
        if let Some(n) = parse_line_names(v) {
            // Dos grupos de nombres seguidos se fusionan.
            if let Some(TrackListItem::Names(prev)) = items.last_mut() {
                prev.extend(n);
            } else {
                items.push(TrackListItem::Names(n));
            }
        } else if let Some(r) = parse_repeat(v) {
            if let TrackListItem::Repeat(RepeatCount::AutoFill | RepeatCount::AutoFit, inner) = &r {
                autos += 1;
                // Dentro de un repeat automático solo caben tamaños fijos (§7.2.3.2).
                if inner.iter().any(|i| match i {
                    TrackListItem::Track(t) => !is_fixed(t),
                    _ => false,
                }) {
                    return None;
                }
            }
            items.push(r);
        } else {
            items.push(TrackListItem::Track(parse_track_size(v)?));
        }
    }
    if autos > 1 {
        return None;
    }
    // Con un repeat automático, el resto de las pistas también deben ser fijas.
    if autos == 1
        && items.iter().any(|i| match i {
            TrackListItem::Track(t) => !is_fixed(t),
            TrackListItem::Repeat(RepeatCount::Count(_), inner) => inner
                .iter()
                .any(|i| matches!(i, TrackListItem::Track(t) if !is_fixed(t))),
            _ => false,
        })
    {
        return None;
    }
    if !items
        .iter()
        .any(|i| matches!(i, TrackListItem::Track(_) | TrackListItem::Repeat(..)))
    {
        return None;
    }
    Some(items)
}

/// `<fixed-size>`: al menos una de las funciones es una longitud fija.
fn is_fixed(t: &TrackSize) -> bool {
    let fixed = |b: &TrackBreadth| matches!(b, TrackBreadth::Length(_));
    match t {
        TrackSize::Breadth(b) => fixed(b),
        TrackSize::MinMax(a, b) => fixed(a) || (fixed(b) && !matches!(a, TrackBreadth::Flex(_))),
        TrackSize::FitContent(_) => false,
    }
}

fn items_compute(items: &[TrackListItem], ctx: &ComputeContext<'_>) -> Vec<TrackListItem> {
    items
        .iter()
        .map(|i| match i {
            TrackListItem::Track(t) => TrackListItem::Track(t.compute(ctx)),
            TrackListItem::Repeat(c, inner) => TrackListItem::Repeat(*c, items_compute(inner, ctx)),
            other => other.clone(),
        })
        .collect()
}

fn items_css(items: &[TrackListItem]) -> String {
    items
        .iter()
        .map(|i| match i {
            TrackListItem::Names(n) => format!("[{}]", n.join(" ")),
            TrackListItem::Track(t) => t.to_css(),
            TrackListItem::Repeat(c, inner) => {
                let c = match c {
                    RepeatCount::Count(n) => n.to_string(),
                    RepeatCount::AutoFill => "auto-fill".into(),
                    RepeatCount::AutoFit => "auto-fit".into(),
                };
                format!("repeat({c}, {})", items_css(inner))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `grid-template-columns` / `grid-template-rows`: `none | <track-list> | <auto-track-list>`.
#[derive(Debug, Clone, PartialEq)]
pub enum GridTemplate {
    None,
    List(Vec<TrackListItem>),
}

impl PropValue for GridTemplate {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        if let [v] = trim(input)
            && is_ident(v, "none")
        {
            return Some(GridTemplate::None);
        }
        parse_track_list(input).map(GridTemplate::List)
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        match self {
            GridTemplate::None => GridTemplate::None,
            GridTemplate::List(items) => GridTemplate::List(items_compute(items, ctx)),
        }
    }
    fn to_css(&self) -> String {
        match self {
            GridTemplate::None => "none".into(),
            GridTemplate::List(items) => items_css(items),
        }
    }
}

/// `grid-auto-columns` / `grid-auto-rows`: `<track-size>+`.
#[derive(Debug, Clone, PartialEq)]
pub struct GridAutoTracks(pub Vec<TrackSize>);

impl PropValue for GridAutoTracks {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let v: Vec<TrackSize> = components(input)
            .into_iter()
            .map(parse_track_size)
            .collect::<Option<_>>()?;
        (!v.is_empty()).then_some(GridAutoTracks(v))
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        GridAutoTracks(self.0.iter().map(|t| t.compute(ctx)).collect())
    }
    fn to_css(&self) -> String {
        self.0
            .iter()
            .map(TrackSize::to_css)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Un área con nombre de `grid-template-areas`: líneas (base 1) de inicio y fin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedArea {
    pub name: String,
    pub row_start: u32,
    pub row_end: u32,
    pub column_start: u32,
    pub column_end: u32,
}

/// `grid-template-areas: none | <string>+` (§7.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GridTemplateAreas {
    None,
    Areas {
        rows: u32,
        columns: u32,
        areas: Vec<NamedArea>,
        strings: Vec<String>,
    },
}

/// Tokens de una fila de `grid-template-areas` (§7.3.1): nombres, `.`+ (celda nula) y basura.
fn area_tokens(row: &str) -> Option<Vec<Option<String>>> {
    let mut out = Vec::new();
    let chars: Vec<char> = row.chars().collect();
    let mut i = 0;
    let is_name = |c: char| c.is_alphanumeric() || c == '-' || c == '_' || !c.is_ascii();
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '.' {
            while i < chars.len() && chars[i] == '.' {
                i += 1;
            }
            out.push(None);
        } else if is_name(c) {
            let start = i;
            while i < chars.len() && is_name(chars[i]) && chars[i] != '.' {
                i += 1;
            }
            out.push(Some(chars[start..i].iter().collect()));
        } else {
            // Trash token: la declaración es inválida.
            return None;
        }
    }
    Some(out)
}

impl GridTemplateAreas {
    /// Construye las áreas a partir de las filas; `None` si no son rectángulos o las filas
    /// tienen distinta cantidad de columnas.
    pub fn from_strings(strings: Vec<String>) -> Option<GridTemplateAreas> {
        let rows: Vec<Vec<Option<String>>> = strings
            .iter()
            .map(|s| area_tokens(s))
            .collect::<Option<_>>()?;
        let columns = rows.first()?.len();
        if columns == 0 || rows.iter().any(|r| r.len() != columns) {
            return None;
        }
        let mut areas: Vec<NamedArea> = Vec::new();
        for (r, row) in rows.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                let Some(name) = cell else {
                    continue;
                };
                let (r, c) = (r as u32 + 1, c as u32 + 1);
                if let Some(a) = areas.iter_mut().find(|a| &a.name == name) {
                    a.row_start = a.row_start.min(r);
                    a.row_end = a.row_end.max(r + 1);
                    a.column_start = a.column_start.min(c);
                    a.column_end = a.column_end.max(c + 1);
                } else {
                    areas.push(NamedArea {
                        name: name.clone(),
                        row_start: r,
                        row_end: r + 1,
                        column_start: c,
                        column_end: c + 1,
                    });
                }
            }
        }
        // Cada área debe ser un rectángulo lleno solo con su nombre.
        for a in &areas {
            for r in a.row_start..a.row_end {
                for c in a.column_start..a.column_end {
                    if rows[r as usize - 1][c as usize - 1].as_deref() != Some(a.name.as_str()) {
                        return None;
                    }
                }
            }
        }
        Some(GridTemplateAreas::Areas {
            rows: rows.len() as u32,
            columns: columns as u32,
            areas,
            strings,
        })
    }
}

impl PropValue for GridTemplateAreas {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let items = components(input);
        if let [v] = items.as_slice()
            && is_ident(v, "none")
        {
            return Some(GridTemplateAreas::None);
        }
        let strings = items
            .iter()
            .map(|v| match v.token() {
                Some(TokenKind::String(s)) => Some(s.clone()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?;
        GridTemplateAreas::from_strings(strings)
    }
    fn to_css(&self) -> String {
        match self {
            GridTemplateAreas::None => "none".into(),
            GridTemplateAreas::Areas { strings, .. } => strings
                .iter()
                .map(|s| format!("{s:?}"))
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}

/// `grid-auto-flow: [ row | column ] || dense`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridAutoFlow {
    pub column: bool,
    pub dense: bool,
}

impl PropValue for GridAutoFlow {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let mut dir = None;
        let mut dense = false;
        let items = components(input);
        if items.is_empty() {
            return None;
        }
        for v in items {
            if dir.is_none() && is_ident(v, "row") {
                dir = Some(false);
            } else if dir.is_none() && is_ident(v, "column") {
                dir = Some(true);
            } else if !dense && is_ident(v, "dense") {
                dense = true;
            } else {
                return None;
            }
        }
        Some(GridAutoFlow {
            column: dir.unwrap_or(false),
            dense,
        })
    }
    fn to_css(&self) -> String {
        let mut s = String::from(if self.column { "column" } else { "row" });
        if self.dense {
            s.push_str(" dense");
        }
        s
    }
}

/// `<grid-line>` (§8.3): `auto | <custom-ident> | [ <integer> && <custom-ident>? ] |
/// [ span && [ <integer> || <custom-ident> ] ]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GridLine {
    Auto,
    /// Línea `integer` (0 = sin número: la primera con ese nombre o, sin nombre, el área
    /// `<name>-start`/`-end`) con nombre opcional.
    Line {
        integer: i32,
        name: Option<String>,
    },
    Span {
        integer: u32,
        name: Option<String>,
    },
}

impl GridLine {
    pub fn is_auto(&self) -> bool {
        matches!(self, GridLine::Auto)
    }

    /// El `<custom-ident>` solo (para los shorthands que lo copian al otro extremo).
    pub fn lone_ident(&self) -> Option<&str> {
        match self {
            GridLine::Line { integer: 0, name } => name.as_deref(),
            _ => None,
        }
    }
}

impl PropValue for GridLine {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let items = components(input);
        if let [v] = items.as_slice()
            && is_ident(v, "auto")
        {
            return Some(GridLine::Auto);
        }
        let mut span = false;
        let mut int = None;
        let mut name = None;
        for v in &items {
            if is_ident(v, "span") {
                if span {
                    return None;
                }
                span = true;
            } else if let Some(n) = integer(v) {
                if int.is_some() {
                    return None;
                }
                int = Some(n);
            } else {
                let i = ident(v)?;
                let reserved = ["span", "auto", "inherit", "initial", "unset", "default"];
                if name.is_some() || reserved.iter().any(|r| i.eq_ignore_ascii_case(r)) {
                    return None;
                }
                name = Some(i.to_owned());
            }
        }
        // `span` debe ir al principio o al final.
        if span {
            let pos = items.iter().position(|v| is_ident(v, "span"))?;
            if pos != 0 && pos != items.len() - 1 {
                return None;
            }
            let n = int.unwrap_or(1);
            if n <= 0 || (int.is_none() && name.is_none()) {
                return None;
            }
            return Some(GridLine::Span {
                integer: n as u32,
                name,
            });
        }
        match (int, name) {
            (Some(0), _) => None,
            (None, None) => None,
            (i, n) => Some(GridLine::Line {
                integer: i.unwrap_or(0),
                name: n,
            }),
        }
    }
    fn to_css(&self) -> String {
        match self {
            GridLine::Auto => "auto".into(),
            GridLine::Line { integer, name } => match (integer, name) {
                (0, Some(n)) => n.clone(),
                (i, Some(n)) => format!("{i} {n}"),
                (i, None) => i.to_string(),
            },
            GridLine::Span { integer, name } => match name {
                Some(n) if *integer == 1 => format!("span {n}"),
                Some(n) => format!("span {integer} {n}"),
                None => format!("span {integer}"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(s: &str) -> Vec<ComponentValue> {
        bf_css::syntax::parse_component_value_list(s)
    }

    #[test]
    fn track_lists() {
        let t = GridTemplate::parse(trim(&tokens("[a] 100px [b c] 1fr minmax(10px, auto)")))
            .expect("válido");
        let GridTemplate::List(items) = t else {
            panic!()
        };
        assert_eq!(items.len(), 5);
        assert!(GridTemplate::parse(trim(&tokens("repeat(auto-fill, 100px)"))).is_some());
        assert!(GridTemplate::parse(trim(&tokens("repeat(auto-fill, 1fr)"))).is_none());
        assert!(
            GridTemplate::parse(trim(&tokens("repeat(2, 1fr 20px) fit-content(40%)"))).is_some()
        );
        assert!(GridTemplate::parse(trim(&tokens("minmax(1fr, 10px)"))).is_none());
    }

    #[test]
    fn areas_and_lines() {
        let a = GridTemplateAreas::parse(trim(&tokens("\"a a b\" \"a a .\""))).expect("válido");
        let GridTemplateAreas::Areas { areas, .. } = a else {
            panic!()
        };
        assert_eq!(areas[0].column_end, 3);
        assert_eq!(areas[0].row_end, 3);
        assert!(GridTemplateAreas::parse(trim(&tokens("\"a b a\""))).is_none());
        assert_eq!(
            GridLine::parse(trim(&tokens("span 2 foo"))),
            Some(GridLine::Span {
                integer: 2,
                name: Some("foo".into())
            })
        );
        assert_eq!(
            GridLine::parse(trim(&tokens("-1"))),
            Some(GridLine::Line {
                integer: -1,
                name: None
            })
        );
        assert!(GridLine::parse(trim(&tokens("0"))).is_none());
        assert!(GridLine::parse(trim(&tokens("2 span 3"))).is_none());
    }
}
