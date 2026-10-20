//! Cascada (CSS Cascade 5) y cómputo de valores.
//!
//! Fuentes de declaraciones, en orden: hoja UA (`ua/html.css`, más `ua/quirks.css` y las
//! reglas de §15.3.9 en quirks mode), presentational hints (`ua/hints.css` y `hints.rs`, origen
//! author con especificidad cero), las hojas `<style>` del documento (respetando `media`), las
//! hojas adicionales del llamador y el atributo `style`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use bf_css::media::MediaContext;
use bf_css::selectors::{MatchContext, PseudoElement, Specificity};
use bf_css::stylesheet::{MatchedDeclaration, Origin, Stylesheet, matching_declarations};
use bf_css::syntax::{
    ComponentValue, Declaration, Function, NestedItem, SimpleBlock, parse_block_contents,
    parse_declaration,
};
use bf_dom::{Document, NodeData, NodeId, QuirksMode};

use crate::hints::presentational_hints;
use crate::properties::shorthands::{ShorthandDef, shorthand_by_name};
use crate::properties::types::{Content, Display, Float, Position, TextAlign};
use crate::properties::{
    ComputeContext, ComputedStyle, CssWide, CustomProperties, LonghandId, PROPERTIES, Value,
    initial_values, longhand_by_name,
};
use crate::values::{LengthContext, NORMAL_LINE_HEIGHT, trim};

/// Estilos computados por (elemento, pseudo-elemento).
pub type StyleMap = HashMap<(NodeId, Option<PseudoElement>), Arc<ComputedStyle>>;

struct UaSheets {
    html: Stylesheet,
    quirks: Stylesheet,
    hints: Stylesheet,
}

fn ua_sheets() -> &'static UaSheets {
    static SHEETS: OnceLock<UaSheets> = OnceLock::new();
    SHEETS.get_or_init(|| UaSheets {
        html: Stylesheet::parse(include_str!("../ua/html.css"), Origin::UserAgent),
        quirks: Stylesheet::parse(include_str!("../ua/quirks.css"), Origin::UserAgent),
        hints: Stylesheet::parse(include_str!("../ua/hints.css"), Origin::Author),
    })
}

/// La hoja UA parseada (para volcados y tests).
pub fn user_agent_stylesheet() -> &'static Stylesheet {
    &ua_sheets().html
}

/// ¿Contiene `var()` (en cualquier nivel)?
pub fn contains_var(values: &[ComponentValue]) -> bool {
    values.iter().any(|v| match v {
        ComponentValue::Function(f) => f.name.eq_ignore_ascii_case("var") || contains_var(&f.value),
        ComponentValue::Block(b) => contains_var(&b.value),
        ComponentValue::Token(_) => false,
    })
}

/// Callback de `@supports`: ¿`name: value` es una declaración válida?
pub fn is_supported(name: &str, value: &[ComponentValue]) -> bool {
    if name.starts_with("--") {
        return true;
    }
    let value = trim(value);
    let longhand = longhand_by_name(name);
    let shorthand = shorthand_by_name(name);
    if longhand.is_none() && shorthand.is_none() {
        return false;
    }
    if contains_var(value) || CssWide::parse(value).is_some() {
        return true;
    }
    match (longhand, shorthand) {
        (Some(id), _) => (id.def().parse)(value).is_some(),
        (None, Some(sh)) => (sh.expand)(value).is_some(),
        _ => false,
    }
}

// --- Orden de la cascada ---

const UNLAYERED: usize = usize::MAX;

/// Clave de precedencia (de menor a mayor), en el orden de criterios de CSS Cascade 5 §6:
/// origen e importancia, element-attached styles, capas, especificidad y orden de aparición.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Precedence {
    band: u8,
    attached: bool,
    layer: usize,
    specificity: Specificity,
    order: u64,
}

fn band(origin: Origin, important: bool) -> u8 {
    match (origin, important) {
        (Origin::UserAgent, false) => 0,
        (Origin::User, false) => 1,
        (Origin::Author, false) => 2,
        (Origin::Author, true) => 3,
        (Origin::User, true) => 4,
        (Origin::UserAgent, true) => 5,
    }
}

/// Una declaración aplicable, antes de expandir shorthands.
struct Source<'a> {
    origin: Origin,
    important: bool,
    attached: bool,
    /// `UNLAYERED` o el índice de la capa (menor = menos precedencia para normales).
    layer: usize,
    specificity: Specificity,
    order: u64,
    decl: &'a Declaration,
}

impl Source<'_> {
    fn precedence(&self) -> Precedence {
        let layer = if self.important {
            // Para `!important` el orden de capas se invierte; sin capa queda debajo de todo.
            if self.layer == UNLAYERED {
                0
            } else {
                UNLAYERED - 1 - self.layer
            }
        } else {
            self.layer
        };
        Precedence {
            band: band(self.origin, self.important),
            attached: self.attached,
            layer,
            specificity: self.specificity,
            order: self.order,
        }
    }
}

#[derive(Debug, Clone)]
enum DeclValue {
    Value(Value),
    Wide(CssWide),
    /// Contiene `var()`: se sustituye en computed-value time.
    Pending {
        shorthand: Option<&'static ShorthandDef>,
        values: Arc<Vec<ComponentValue>>,
    },
}

#[derive(Debug, Clone)]
struct Entry {
    prec: Precedence,
    origin: Origin,
    /// Capa sin invertir (para `revert-layer`).
    layer: usize,
    value: DeclValue,
}

#[derive(Debug, Clone)]
enum CustomValue {
    Raw(Arc<Vec<ComponentValue>>),
    Wide(CssWide),
}

#[derive(Debug, Clone)]
struct CustomEntry {
    prec: Precedence,
    origin: Origin,
    layer: usize,
    name: String,
    value: CustomValue,
}

/// Expande una declaración en entradas por longhand. Las inválidas o desconocidas se
/// descartan.
fn expand(
    source: &Source<'_>,
    longhands: &mut Vec<(LonghandId, Entry)>,
    customs: &mut Vec<CustomEntry>,
) {
    let decl = source.decl;
    let value = trim(&decl.value);
    let prec = source.precedence();
    let entry = |value: DeclValue| Entry {
        prec,
        origin: source.origin,
        layer: source.layer,
        value,
    };
    if decl.name.starts_with("--") {
        let value = match CssWide::parse(value) {
            Some(w) => CustomValue::Wide(w),
            None => CustomValue::Raw(Arc::new(decl.value.clone())),
        };
        customs.push(CustomEntry {
            prec,
            origin: source.origin,
            layer: source.layer,
            name: decl.name.clone(),
            value,
        });
        return;
    }
    let wide = CssWide::parse(value);
    let pending = contains_var(value).then(|| Arc::new(value.to_vec()));
    if let Some(id) = longhand_by_name(&decl.name) {
        let v = match (wide, pending) {
            (Some(w), _) => DeclValue::Wide(w),
            (None, Some(values)) => DeclValue::Pending {
                shorthand: None,
                values,
            },
            (None, None) => match (id.def().parse)(value) {
                Some(v) => DeclValue::Value(v),
                None => return,
            },
        };
        longhands.push((id, entry(v)));
    } else if let Some(sh) = shorthand_by_name(&decl.name) {
        match (wide, pending) {
            (Some(w), _) => {
                for &id in sh.longhands {
                    longhands.push((id, entry(DeclValue::Wide(w))));
                }
            }
            (None, Some(values)) => {
                for &id in sh.longhands {
                    longhands.push((
                        id,
                        entry(DeclValue::Pending {
                            shorthand: Some(sh),
                            values: values.clone(),
                        }),
                    ));
                }
            }
            (None, None) => {
                if let Some(values) = (sh.expand)(value) {
                    for v in values {
                        longhands.push((v.id(), entry(DeclValue::Value(v))));
                    }
                }
            }
        }
    }
}

/// Regla de salto activa tras un `revert` o `revert-layer`.
enum Skip {
    Origin(Origin),
    Layer {
        band: u8,
        attached: bool,
        layer: usize,
    },
}

impl Skip {
    fn skips(&self, prec: &Precedence, origin: Origin, layer: usize) -> bool {
        match self {
            Skip::Origin(o) => *o == origin,
            Skip::Layer {
                band,
                attached,
                layer: l,
            } => prec.band == *band && prec.attached == *attached && layer == *l,
        }
    }
}

/// Valor en cascada de una lista ordenada (ascendente) de entradas, resolviendo `revert` y
/// `revert-layer`. `None` = sin valor en cascada (se comporta como `unset`).
fn cascaded<T>(
    list: &[T],
    key: impl Fn(&T) -> (&Precedence, Origin, usize),
    as_wide: impl Fn(&T) -> Option<CssWide>,
) -> Option<&T> {
    let mut skip: Option<Skip> = None;
    for item in list.iter().rev() {
        let (prec, origin, layer) = key(item);
        if skip.as_ref().is_some_and(|s| s.skips(prec, origin, layer)) {
            continue;
        }
        match as_wide(item) {
            Some(CssWide::Revert) => skip = Some(Skip::Origin(origin)),
            Some(CssWide::RevertLayer) => {
                skip = Some(Skip::Layer {
                    band: prec.band,
                    attached: prec.attached,
                    layer,
                });
            }
            _ => return Some(item),
        }
    }
    None
}

// --- var() ---

/// Sustituye las referencias `var()` de `values`. `None` si alguna referencia es inválida y
/// no tiene fallback válido.
fn substitute(
    values: &[ComponentValue],
    lookup: &mut dyn FnMut(&str) -> Option<Arc<Vec<ComponentValue>>>,
) -> Option<Vec<ComponentValue>> {
    let mut out = Vec::with_capacity(values.len());
    for v in values {
        match v {
            ComponentValue::Function(f) if f.name.eq_ignore_ascii_case("var") => {
                let comma = f
                    .value
                    .iter()
                    .position(|v| v.token() == Some(&bf_css::syntax::TokenKind::Comma));
                let (name_part, fallback) = match comma {
                    Some(i) => (&f.value[..i], Some(&f.value[i + 1..])),
                    None => (&f.value[..], None),
                };
                let name = match trim(name_part) {
                    [n] => match n.token() {
                        Some(bf_css::syntax::TokenKind::Ident(s)) if s.starts_with("--") => {
                            s.clone()
                        }
                        _ => return None,
                    },
                    _ => return None,
                };
                match lookup(&name) {
                    Some(value) => out.extend(value.iter().cloned()),
                    None => out.extend(substitute(fallback?, lookup)?),
                }
            }
            ComponentValue::Function(f) => out.push(ComponentValue::Function(Function {
                value: substitute(&f.value, lookup)?,
                ..f.clone()
            })),
            ComponentValue::Block(b) => out.push(ComponentValue::Block(SimpleBlock {
                value: substitute(&b.value, lookup)?,
                ..b.clone()
            })),
            other => out.push(other.clone()),
        }
    }
    Some(out)
}

/// Resuelve las custom properties del elemento: aplica la cascada sobre las heredadas y
/// sustituye `var()` con detección de ciclos (las propiedades de un ciclo quedan inválidas).
fn compute_custom_properties(
    entries: &mut [CustomEntry],
    parent: Option<&ComputedStyle>,
) -> Arc<CustomProperties> {
    let inherited = parent.map(|p| p.custom.clone()).unwrap_or_default();
    if entries.is_empty() {
        return inherited;
    }
    entries.sort_by_key(|e| e.prec);
    let mut by_name: HashMap<&str, Vec<&CustomEntry>> = HashMap::new();
    for e in entries.iter() {
        by_name.entry(&e.name).or_default().push(e);
    }
    let mut raw: HashMap<String, Arc<Vec<ComponentValue>>> = (*inherited).clone();
    for (name, list) in by_name {
        let winner = cascaded(
            &list,
            |e| (&e.prec, e.origin, e.layer),
            |e| match e.value {
                CustomValue::Wide(w) => Some(w),
                CustomValue::Raw(_) => None,
            },
        );
        match winner.map(|e| &e.value) {
            Some(CustomValue::Raw(v)) => {
                raw.insert(name.to_owned(), v.clone());
            }
            Some(CustomValue::Wide(CssWide::Initial)) => {
                raw.remove(name);
            }
            // `inherit`, `unset` y sin valor en cascada: el heredado (las custom properties
            // se heredan).
            _ => match inherited.get(name) {
                Some(v) => {
                    raw.insert(name.to_owned(), v.clone());
                }
                None => {
                    raw.remove(name);
                }
            },
        }
    }
    // Sustitución con detección de ciclos (DFS).
    #[derive(Clone)]
    enum State {
        InProgress,
        Done(Option<Arc<Vec<ComponentValue>>>),
    }
    fn resolve(
        name: &str,
        raw: &HashMap<String, Arc<Vec<ComponentValue>>>,
        state: &mut HashMap<String, State>,
        stack: &mut Vec<String>,
        cyclic: &mut HashSet<String>,
    ) -> Option<Arc<Vec<ComponentValue>>> {
        match state.get(name) {
            Some(State::Done(v)) => return v.clone(),
            Some(State::InProgress) => {
                // Ciclo: todas las propiedades desde `name` hasta el tope de la pila.
                if let Some(pos) = stack.iter().position(|n| n == name) {
                    cyclic.extend(stack[pos..].iter().cloned());
                }
                return None;
            }
            None => {}
        }
        let value = raw.get(name)?.clone();
        if !contains_var(&value) {
            state.insert(name.to_owned(), State::Done(Some(value.clone())));
            return Some(value);
        }
        state.insert(name.to_owned(), State::InProgress);
        stack.push(name.to_owned());
        let result =
            substitute(&value, &mut |n| resolve(n, raw, state, stack, cyclic)).map(Arc::new);
        stack.pop();
        let result = if cyclic.contains(name) { None } else { result };
        state.insert(name.to_owned(), State::Done(result.clone()));
        result
    }
    let mut state = HashMap::new();
    let mut cyclic = HashSet::new();
    let names: Vec<String> = raw.keys().cloned().collect();
    let mut out = CustomProperties::new();
    for name in names {
        if let Some(v) = resolve(&name, &raw, &mut state, &mut Vec::new(), &mut cyclic) {
            out.insert(name, v);
        }
    }
    Arc::new(out)
}

// --- Recolección de declaraciones ---

/// Elementos "with default margins" de §15.3.9.
const DEFAULT_MARGINS: &[&str] = &[
    "blockquote",
    "dir",
    "dl",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "listing",
    "menu",
    "ol",
    "p",
    "plaintext",
    "pre",
    "ul",
    "xmp",
];

fn is_substantial(doc: &Document, node: NodeId) -> bool {
    match &doc.get(node).data {
        NodeData::Element(_) => true,
        NodeData::Text(t) => !t
            .chars()
            .all(|c| matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' ')),
        _ => false,
    }
}

/// Reglas de §15.3.9 (margin collapsing quirks), como declaraciones de nivel UA.
fn quirks_margin_declarations(doc: &Document, node: NodeId) -> Vec<Declaration> {
    let mut out = Vec::new();
    let Some(el) = doc.element(node) else {
        return out;
    };
    if !DEFAULT_MARGINS.iter().any(|n| el.is_html(n)) {
        return out;
    }
    let Some(parent) = doc
        .parent(node)
        .and_then(|p| doc.element(p).map(|e| (p, e)))
    else {
        return out;
    };
    let in_body = parent.1.is_html("body");
    let in_cell = parent.1.is_html("td") || parent.1.is_html("th");
    if !in_body && !in_cell {
        return out;
    }
    let siblings: Vec<NodeId> = doc.children(parent.0).collect();
    let index = siblings.iter().position(|&s| s == node).unwrap_or(0);
    let no_prev = !siblings[..index].iter().any(|&s| is_substantial(doc, s));
    let no_next = !siblings[index + 1..]
        .iter()
        .any(|&s| is_substantial(doc, s));
    let blank = !doc.children(node).any(|c| is_substantial(doc, c));
    let mut push = |text: &str| {
        if let Ok(d) = parse_declaration(text) {
            out.push(d);
        }
    };
    if no_prev {
        push("margin-top: 0");
        if blank {
            push("margin-bottom: 0");
        }
    }
    if in_cell && no_next && blank {
        push("margin-top: 0");
    }
    if in_cell && no_next && el.is_html("p") {
        push("margin-bottom: 0");
    }
    out
}

struct Styler<'a> {
    doc: &'a Document,
    ua: Vec<&'a Stylesheet>,
    author: Vec<&'a Stylesheet>,
    user: Vec<&'a Stylesheet>,
    media: &'a MediaContext,
    ctx: &'a MatchContext,
    root_font_size: f32,
    root_line_height: f32,
    fonts: Option<&'a dyn FontMetricsSource>,
}

/// Métricas de fuente para las unidades `ex` y `ch`. El layout las provee con sus fuentes;
/// sin ellas se usa el fallback de 0.5em.
pub trait FontMetricsSource {
    /// (x-height, avance de "0") en px para la fuente de `style`.
    fn ex_ch(&self, style: &ComputedStyle) -> (f32, f32);
}

/// Índice que `matching_declarations` da a los estilos sin capa en un grupo de hojas del
/// mismo origen: el número de capas distintas.
fn unlayered_index(sheets: &[&Stylesheet]) -> usize {
    let names: HashSet<_> = sheets.iter().flat_map(|s| s.layer_order.iter()).collect();
    names.len()
}

impl<'a> Styler<'a> {
    fn matched(
        &self,
        node: NodeId,
        pseudo: Option<PseudoElement>,
        sheets: &[&'a Stylesheet],
    ) -> Vec<MatchedDeclaration<'a>> {
        if sheets.is_empty() {
            return Vec::new();
        }
        matching_declarations(
            self.doc,
            node,
            pseudo,
            sheets,
            self.media,
            &is_supported,
            self.ctx,
        )
    }

    /// Todas las declaraciones aplicables a `node`/`pseudo` y las generadas para él.
    fn sources<'d>(
        &self,
        owned: &'d [(Origin, bool, Declaration)],
        matched: &'d [(u64, usize, MatchedDeclaration<'a>)],
    ) -> Vec<Source<'d>>
    where
        'a: 'd,
    {
        let mut sources: Vec<Source<'d>> = Vec::new();
        for (group, unlayered, m) in matched {
            sources.push(Source {
                origin: m.origin,
                important: m.important,
                attached: false,
                layer: if m.layer_index >= *unlayered {
                    UNLAYERED
                } else {
                    m.layer_index
                },
                // Los presentational hints (grupo 2) tienen especificidad cero.
                specificity: if *group == 2 {
                    Specificity::default()
                } else {
                    m.specificity
                },
                order: (group << 40) | m.source_order as u64,
                decl: m.declaration,
            });
        }
        for (i, (origin, attached, decl)) in owned.iter().enumerate() {
            // Generadas: quirks UA (grupo 1), hints (3) y atributo style (6).
            let group: u64 = match (origin, attached) {
                (Origin::UserAgent, _) => 1,
                (_, false) => 3,
                (_, true) => 6,
            };
            // Las reglas de §15.3.9 deben ganar a las de la hoja UA que ponen los márgenes
            // por defecto; los hints y el atributo style tienen especificidad cero.
            // TODO(cascade): sustituir por un origen "UA quirks" explícito
            let specificity = if group == 1 {
                Specificity(u32::MAX, 0, 0)
            } else {
                Specificity::default()
            };
            sources.push(Source {
                origin: *origin,
                important: decl.important,
                attached: *attached,
                layer: UNLAYERED,
                specificity,
                order: (group << 40) | i as u64,
                decl,
            });
        }
        sources
    }

    fn style_for(
        &self,
        node: NodeId,
        pseudo: Option<PseudoElement>,
        parent: Option<&ComputedStyle>,
        is_root: bool,
    ) -> ComputedStyle {
        let sheets = ua_sheets();
        let mut matched: Vec<(u64, usize, MatchedDeclaration<'a>)> = Vec::new();
        let groups: [(u64, &[&'a Stylesheet]); 3] =
            [(0, &self.ua), (4, &self.author), (5, &self.user)];
        for (group, list) in groups {
            let unlayered = unlayered_index(list);
            for m in self.matched(node, pseudo, list) {
                matched.push((group, unlayered, m));
            }
        }
        let hints_sheet: [&'a Stylesheet; 1] = [&sheets.hints];
        for m in self.matched(node, pseudo, &hints_sheet) {
            matched.push((2, usize::MAX, m));
        }
        let mut owned: Vec<(Origin, bool, Declaration)> = Vec::new();
        if pseudo.is_none() {
            if self.doc.mode == QuirksMode::Quirks {
                for d in quirks_margin_declarations(self.doc, node) {
                    owned.push((Origin::UserAgent, false, d));
                }
            }
            // E4: HTML §15.3.8: un `th` cuyo padre tiene el valor inicial de `text-align` se
            // centra (regla de la hoja UA que depende de un valor computado).
            if self.doc.element(node).is_some_and(|e| e.is_html("th"))
                && parent.is_some_and(|p| p.text_align == TextAlign::Start)
                && let Ok(d) = parse_declaration("text-align: center")
            {
                owned.push((Origin::UserAgent, false, d));
            }
            for d in presentational_hints(self.doc, node) {
                owned.push((Origin::Author, false, d));
            }
            if let Some(style) = self.doc.element(node).and_then(|e| e.attr("style")) {
                let contents = parse_block_contents(style);
                let mut decls = contents.declarations;
                for item in contents.rules {
                    if let NestedItem::Declarations(d) = item {
                        decls.extend(d);
                    }
                }
                for d in decls {
                    owned.push((Origin::Author, true, d));
                }
            }
        }
        let sources = self.sources(&owned, &matched);
        self.compute(&sources, parent, is_root)
    }

    fn compute(
        &self,
        sources: &[Source<'_>],
        parent: Option<&ComputedStyle>,
        is_root: bool,
    ) -> ComputedStyle {
        let mut longhands: Vec<(LonghandId, Entry)> = Vec::new();
        let mut customs: Vec<CustomEntry> = Vec::new();
        for s in sources {
            expand(s, &mut longhands, &mut customs);
        }
        longhands.sort_by_key(|e| e.1.prec);
        let mut per_property: Vec<Vec<Entry>> = vec![Vec::new(); PROPERTIES.len()];
        for (id, e) in longhands {
            per_property[id as usize].push(e);
        }
        let mut style = parent
            .cloned()
            .unwrap_or_else(|| ComputedStyle::initial().clone());
        style.custom = compute_custom_properties(&mut customs, parent);
        let custom = style.custom.clone();

        let root_font_size = if is_root {
            crate::properties::types::MEDIUM_FONT_SIZE
        } else {
            self.root_font_size
        };
        let parent_font = parent.map_or(
            crate::properties::types::MEDIUM_FONT_SIZE,
            ComputedStyle::font_size_px,
        );
        let ratios = |s: &ComputedStyle| {
            let fs = s.font_size_px();
            match self.fonts {
                Some(f) if fs > 0.0 => {
                    let (ex, ch) = f.ex_ch(s);
                    (ex / fs, ch / fs)
                }
                _ => (0.5, 0.5),
            }
        };
        let (ex_ratio, ch_ratio) = parent.map_or((0.5, 0.5), ratios);
        let mut len = LengthContext {
            font_size: parent_font,
            root_font_size,
            line_height: self.root_line_height,
            viewport_width: self.media.width as f32,
            viewport_height: self.media.height as f32,
            ex_ratio,
            ch_ratio,
        };
        let ctx = |len: LengthContext| ComputeContext { len, parent };
        // font-size primero (sus `em` y `ex` son relativos a la fuente del padre), luego el
        // resto de la fuente (para `ex`/`ch` del elemento) y line-height (para `lh`).
        let fs = self.cascaded_value(
            LonghandId::FontSize,
            &per_property,
            parent,
            &ctx(len),
            &custom,
        );
        style.set(fs);
        len.font_size = style.font_size_px();
        for id in [
            LonghandId::FontFamily,
            LonghandId::FontWeight,
            LonghandId::FontStyle,
        ] {
            let v = self.cascaded_value(id, &per_property, parent, &ctx(len), &custom);
            style.set(v);
        }
        (len.ex_ratio, len.ch_ratio) = ratios(&style);
        let lh = self.cascaded_value(
            LonghandId::LineHeight,
            &per_property,
            parent,
            &ctx(len),
            &custom,
        );
        style.set(lh);
        len.line_height = style.line_height.resolve(len.font_size, NORMAL_LINE_HEIGHT);
        let full = ctx(len);
        for def in PROPERTIES {
            if matches!(
                def.id,
                LonghandId::FontSize
                    | LonghandId::LineHeight
                    | LonghandId::FontFamily
                    | LonghandId::FontWeight
                    | LonghandId::FontStyle
            ) {
                continue;
            }
            let value = self.cascaded_value(def.id, &per_property, parent, &full, &custom);
            style.set(value);
        }
        // CSS 2.2 §9.7 y CSS Display 3 §2.7.
        style.original_display = style.display;
        if matches!(style.position, Position::Absolute | Position::Fixed) {
            style.float = Float::None;
        }
        // Los hijos de un contenedor flex o grid se blockifican (CSS Display 3 §2.7).
        let flex_item = parent.is_some_and(|p| {
            matches!(
                p.display,
                Display::Flex | Display::InlineFlex | Display::Grid | Display::InlineGrid
            )
        });
        if is_root
            || flex_item
            || style.float != Float::None
            || matches!(style.position, Position::Absolute | Position::Fixed)
        {
            style.display = match style.display {
                Display::Contents if is_root => Display::Block,
                d => d.blockify(),
            };
        }
        style
    }

    /// Valor computado de un longhand según su valor en cascada.
    fn cascaded_value(
        &self,
        id: LonghandId,
        per_property: &[Vec<Entry>],
        parent: Option<&ComputedStyle>,
        ctx: &ComputeContext<'_>,
        custom: &CustomProperties,
    ) -> Value {
        let def = id.def();
        let initial = || initial_values()[id as usize].clone();
        let inherit = || parent.map_or_else(initial, |p| p.get(id));
        let unset = || if def.inherited { inherit() } else { initial() };
        let winner = cascaded(
            &per_property[id as usize],
            |e| (&e.prec, e.origin, e.layer),
            |e| match e.value {
                DeclValue::Wide(w) => Some(w),
                _ => None,
            },
        );
        let wide = |w: CssWide| match w {
            CssWide::Initial => initial(),
            CssWide::Inherit => inherit(),
            _ => unset(),
        };
        match winner.map(|e| &e.value) {
            None => unset(),
            Some(DeclValue::Wide(w)) => wide(*w),
            Some(DeclValue::Value(v)) => (def.compute)(v, ctx),
            Some(DeclValue::Pending { shorthand, values }) => {
                let mut lookup = |name: &str| custom.get(name).cloned();
                let Some(substituted) = substitute(values, &mut lookup) else {
                    // Inválido en computed-value time.
                    return unset();
                };
                let substituted = trim(&substituted);
                if let Some(w) = CssWide::parse(substituted) {
                    return wide(w);
                }
                let parsed = match shorthand {
                    Some(sh) => (sh.expand)(substituted)
                        .and_then(|vals| vals.into_iter().find(|v| v.id() == id)),
                    None => (def.parse)(substituted),
                };
                match parsed {
                    Some(v) => (def.compute)(&v, ctx),
                    None => unset(),
                }
            }
        }
    }
}

/// Calcula los estilos de todos los elementos del documento (y de sus `::before`, `::after` y
/// `::marker`). `sheets` son hojas adicionales (de usuario o de autor, después de las del
/// documento).
pub fn style_document(
    doc: &Document,
    sheets: &[&Stylesheet],
    media: &MediaContext,
    ctx: &MatchContext,
) -> StyleMap {
    let document_sheets = crate::loader::document_stylesheets(doc, media, None);
    style_document_with_sheets(doc, &document_sheets, sheets, media, ctx, None)
}

/// Como [`style_document`], con las hojas del documento ya recolectadas (por ejemplo con
/// [`crate::loader::document_stylesheets`] y un cargador de recursos) y, opcionalmente,
/// métricas de fuente para `ex` y `ch`.
pub fn style_document_with_sheets(
    doc: &Document,
    document_sheets: &[Stylesheet],
    sheets: &[&Stylesheet],
    media: &MediaContext,
    ctx: &MatchContext,
    fonts: Option<&dyn FontMetricsSource>,
) -> StyleMap {
    let ua = ua_sheets();
    let mut ua_list: Vec<&Stylesheet> = vec![&ua.html];
    if doc.mode == QuirksMode::Quirks {
        ua_list.push(&ua.quirks);
    }
    let mut author: Vec<&Stylesheet> = document_sheets.iter().collect();
    let mut user = Vec::new();
    for s in sheets {
        match s.origin {
            Origin::Author => author.push(s),
            Origin::User => user.push(*s),
            Origin::UserAgent => ua_list.push(s),
        }
    }
    let mut styler = Styler {
        doc,
        ua: ua_list,
        author,
        user,
        media,
        ctx,
        root_font_size: crate::properties::types::MEDIUM_FONT_SIZE,
        root_line_height: crate::properties::types::MEDIUM_FONT_SIZE * NORMAL_LINE_HEIGHT,
        fonts,
    };
    let mut map = StyleMap::new();
    let mut stack: Vec<(NodeId, Option<Arc<ComputedStyle>>)> = doc
        .children(doc.root())
        .filter(|&c| doc.element(c).is_some())
        .map(|c| (c, None))
        .collect();
    stack.reverse();
    while let Some((node, parent)) = stack.pop() {
        let is_root = parent.is_none() && doc.parent(node) == Some(doc.root());
        let style = Arc::new(styler.style_for(node, None, parent.as_deref(), is_root));
        if is_root {
            styler.root_font_size = style.font_size_px();
            styler.root_line_height = style
                .line_height
                .resolve(style.font_size_px(), NORMAL_LINE_HEIGHT);
        }
        for pseudo in [PseudoElement::Before, PseudoElement::After] {
            let ps = styler.style_for(node, Some(pseudo), Some(&style), false);
            if !matches!(ps.content, Content::None | Content::Normal) && ps.display != Display::None
            {
                map.insert((node, Some(pseudo)), Arc::new(ps));
            }
        }
        if style.display == Display::ListItem {
            let marker = styler.style_for(node, Some(PseudoElement::Marker), Some(&style), false);
            map.insert((node, Some(PseudoElement::Marker)), Arc::new(marker));
        }
        let children: Vec<NodeId> = doc
            .children(node)
            .filter(|&c| doc.element(c).is_some())
            .collect();
        for child in children.into_iter().rev() {
            stack.push((child, Some(style.clone())));
        }
        map.insert((node, None), style);
    }
    map
}
