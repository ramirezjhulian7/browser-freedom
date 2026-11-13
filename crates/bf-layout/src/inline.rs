//! Inline formatting context (CSS 2.2 §9.4.2 y §10.8, CSS Text 3): segmentación del contenido
//! inline, corte de líneas, fase 2 de white space, alineación horizontal (`text-align`,
//! `text-indent`) y vertical (`vertical-align`), cajas inline fragmentadas en varias líneas,
//! inline-level atómicas y floats dentro del contenido inline.
//!
//! E9: texto internacional.
//! - Niveles bidi del párrafo con UAX #9 ([`crate::bidi`]) y los controles que CSS Writing
//!   Modes 3 §2.4.2 asocia a `unicode-bidi` y `direction`; orden visual de cada línea con
//!   L1–L2 y reconstrucción de los fragmentos de las cajas inline en ese orden.
//! - Oportunidades de corte con UAX #14 ([`crate::linebreak`]) sobre el texto de todo el
//!   IFC, adaptadas por `word-break` (CSS Text 3 §5.2), `white-space` (§5.1: la oportunidad
//!   entre cajas distintas la controla su ancestro común más cercano) y `hyphens` (§5.4).
//! - Shaping por run de un solo sentido ([`crate::FontProvider::shape_dir`]).
//! - `text-align` `start`/`end` según la dirección del párrafo y `text-justify` (§7.3–§7.4).

use bf_style::ComputedStyle;
use bf_style::properties::types::{
    Direction, Hyphens, Overflow, OverflowWrap, TextAlign, TextAlignLast, TextJustify,
    TextLineBreak, TextTransform, UnicodeBidi, VerticalAlign, VerticalAlignKeyword, WhiteSpace,
    WordBreak,
};

use crate::bidi;
use crate::block::{Cb, Layout, border_edges, margin_edges, padding_edges};
use crate::box_tree::{BoxKind, LayoutBox};
use crate::float::FloatContext;
use crate::font::{FontMetrics, Glyph};
use crate::fragment::{Fragment, FragmentKind, Rect};
use crate::linebreak::{self, Break, LineBreak};

/// U+00AD SOFT HYPHEN.
const SHY: char = '\u{AD}';

/// Un texto del IFC con sus glifos.
struct TextItem<'a> {
    b: &'a LayoutBox,
    text: String,
    glyphs: Vec<Glyph>,
    /// Caja inline que lo contiene (`None` = la raíz).
    parent: Option<usize>,
    /// Nivel bidi de cada byte del texto (el del carácter que lo contiene).
    levels: Vec<u8>,
    /// `opp[i]`: hay una oportunidad de corte en el offset `i` (después del carácter que
    /// termina ahí).
    opp: Vec<bool>,
    /// Glifos del guion que se muestra al cortar en un U+00AD, y su ancho.
    hyphen: Option<(Vec<Glyph>, f32)>,
}

/// Una caja inline del IFC.
struct InlineBox<'a> {
    b: &'a LayoutBox,
    parent: Option<usize>,
    /// Margen + borde + padding de los lados start / end (según la `direction` de la caja).
    start: f32,
    end: f32,
    /// Los márgenes de esos lados.
    start_margin: f32,
    end_margin: f32,
    /// Nivel de incrustación explícito (UAX #9 X1–X8 con los controles de `unicode-bidi`).
    level: u8,
    /// Cajas aislantes: niveles resueltos de su iniciador (LRI/RLI/FSI) y de su PDI. Son
    /// caracteres del párrafo para UAX #9 (X9 no los quita) y separan en L2 lo de dentro de lo
    /// de fuera.
    iso: Option<(u8, u8)>,
}

/// Una inline-level atómica ya dimensionada (fragmento con su margin box en el origen).
struct Atomic {
    fragment: Fragment,
    /// Distancia del borde superior del margin box a la baseline.
    baseline: f32,
    width: f32,
    height: f32,
    parent: Option<usize>,
    /// Nivel bidi (el de su U+FFFC en el párrafo).
    level: u8,
    /// Oportunidad de corte después de la atómica.
    break_after: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SegKind {
    Word {
        item: usize,
        start: usize,
        end: usize,
    },
    Space {
        item: usize,
        start: usize,
        end: usize,
        collapsible: bool,
    },
    Start(usize),
    End(usize),
    Atomic(usize),
    Break,
    Opportunity,
    Float(usize),
    Abs(usize),
}

#[derive(Debug, Clone, Copy)]
struct Seg {
    kind: SegKind,
    width: f32,
    /// Hay una oportunidad de corte después del segmento.
    break_after: bool,
    /// La palabra termina en un U+00AD: si la línea se corta ahí, se muestra un guion.
    hyphen: bool,
}

impl Seg {
    fn new(kind: SegKind, width: f32, break_after: bool) -> Seg {
        Seg {
            kind,
            width,
            break_after,
            hyphen: false,
        }
    }
}

/// Contenido del IFC en orden lógico antes de segmentar los textos.
enum Piece {
    Text(usize),
    Seg(Seg),
}

struct Ifc<'a> {
    texts: Vec<TextItem<'a>>,
    boxes: Vec<InlineBox<'a>>,
    atomics: Vec<Atomic>,
    oof: Vec<&'a LayoutBox>,
    pieces: Vec<Piece>,
    segs: Vec<Seg>,
    /// Nivel bidi del párrafo de la línea en curso.
    para_level: u8,
    /// Nivel de cada párrafo bidi (separados por los saltos forzados), en orden.
    para_levels: Vec<u8>,
}

fn transform(text: &str, t: TextTransform) -> String {
    match t {
        TextTransform::Uppercase => text.to_uppercase(),
        TextTransform::Lowercase => text.to_lowercase(),
        TextTransform::Capitalize => {
            let mut out = String::with_capacity(text.len());
            let mut start = true;
            for c in text.chars() {
                if start && c.is_alphanumeric() {
                    out.extend(c.to_uppercase());
                    start = false;
                } else {
                    out.push(c);
                    if c.is_whitespace() {
                        start = true;
                    }
                }
            }
            out
        }
        TextTransform::None | TextTransform::FullWidth => text.to_owned(),
    }
}

fn glyph_width(glyphs: &[Glyph], start: usize, end: usize) -> f32 {
    glyphs
        .iter()
        .filter(|g| g.cluster >= start && g.cluster < end)
        .map(|g| g.advance)
        .sum()
}

/// Altura de media leading (CSS 2.2 §10.8.1).
fn half_leading(m: &FontMetrics, line_height: f32) -> f32 {
    (line_height - (m.ascent + m.descent)) / 2.0
}

/// Separadores de palabras (CSS Text 3 §4.2).
fn is_word_separator(c: char) -> bool {
    matches!(
        c,
        ' ' | '\u{A0}' | '\u{1361}' | '\u{10100}' | '\u{10101}' | '\u{1039F}' | '\u{1091F}'
    )
}

/// `letter-spacing` después de cada carácter tipográfico (el último glifo del clúster) y
/// `word-spacing` en los separadores de palabras (CSS Text 3 §8.2 y §4.2). `text` es el texto
/// al que indexan los clústeres.
fn apply_spacing(glyphs: &mut [Glyph], text: &str, style: &ComputedStyle) {
    let letter = style.letter_spacing.px();
    let word = style.word_spacing.px();
    if letter == 0.0 && word == 0.0 {
        return;
    }
    for i in 0..glyphs.len() {
        let cluster = glyphs[i].cluster;
        let last_of_cluster = glyphs.get(i + 1).is_none_or(|n| n.cluster != cluster);
        if last_of_cluster {
            glyphs[i].advance += letter;
            if text[cluster..].starts_with(is_word_separator) {
                glyphs[i].advance += word;
            }
        }
    }
}

type Controls = (&'static [char], &'static [char]);

/// Los controles bidi que CSS Writing Modes 3 §2.4.2 (tabla de `unicode-bidi`) inserta al
/// principio y al final del contenido de una caja inline.
fn bidi_controls(style: &ComputedStyle) -> Controls {
    let rtl = style.direction == Direction::Rtl;
    match (style.unicode_bidi, rtl) {
        (UnicodeBidi::Normal, _) => (&[], &[]),
        (UnicodeBidi::Embed, false) => (&['\u{202A}'], &['\u{202C}']),
        (UnicodeBidi::Embed, true) => (&['\u{202B}'], &['\u{202C}']),
        (UnicodeBidi::Isolate, false) => (&['\u{2066}'], &['\u{2069}']),
        (UnicodeBidi::Isolate, true) => (&['\u{2067}'], &['\u{2069}']),
        (UnicodeBidi::BidiOverride, false) => (&['\u{202D}'], &['\u{202C}']),
        (UnicodeBidi::BidiOverride, true) => (&['\u{202E}'], &['\u{202C}']),
        (UnicodeBidi::IsolateOverride, false) => {
            (&['\u{2066}', '\u{202D}'], &['\u{202C}', '\u{2069}'])
        }
        (UnicodeBidi::IsolateOverride, true) => {
            (&['\u{2067}', '\u{202E}'], &['\u{202C}', '\u{2069}'])
        }
        (UnicodeBidi::Plaintext, _) => (&['\u{2068}'], &['\u{2069}']),
    }
}

/// Clase de corte de un carácter con `word-break` (CSS Text 3 §5.2): `break-all` trata las
/// letras y números como ideogramas; `keep-all` suprime las oportunidades entre ideogramas
/// (los trata como letras).
fn adapted_class(c: char, word_break: WordBreak, line_break: TextLineBreak) -> LineBreak {
    let k = match (linebreak::resolve_class(c), line_break) {
        // CSS Text 3 §5.3: con `strict` no se corta antes de las kana pequeñas (CJ como NS);
        // con `normal` y `loose`, sí (CJ como ID).
        (_, TextLineBreak::Strict) if linebreak::line_break_class(c) == LineBreak::CJ => {
            LineBreak::NS
        }
        _ if linebreak::line_break_class(c) == LineBreak::CJ => LineBreak::ID,
        // `loose`: se puede cortar antes de los guiones CJK 〜 y ゠, de las marcas de
        // iteración, de la puntuación centrada y entre los caracteres inseparables. (Antes de
        // ‐ y – solo tras un ID: ver `break_opportunities`.)
        (_, TextLineBreak::Loose)
            if matches!(
                c,
                '\u{301C}'
                    | '\u{30A0}'
                    | '\u{3005}'
                    | '\u{303B}'
                    | '\u{309D}'
                    | '\u{309E}'
                    | '\u{30FD}'
                    | '\u{30FE}'
                    | '\u{30FB}'
                    | '\u{FF1A}'
                    | '\u{FF1B}'
                    | '\u{FF65}'
                    | '\u{2024}'
                    | '\u{2025}'
                    | '\u{2026}'
                    | '\u{22EF}'
                    | '\u{FE19}'
            ) =>
        {
            LineBreak::ID
        }
        (k, _) => k,
    };
    match word_break {
        WordBreak::BreakAll if matches!(k, LineBreak::AL | LineBreak::HL | LineBreak::NU) => {
            LineBreak::ID
        }
        WordBreak::KeepAll
            if matches!(
                k,
                LineBreak::ID
                    | LineBreak::H2
                    | LineBreak::H3
                    | LineBreak::JL
                    | LineBreak::JV
                    | LineBreak::JT
            ) =>
        {
            LineBreak::AL
        }
        _ => k,
    }
}

/// ¿El carácter es una oportunidad de justificación (CSS Text 3 §7.3) con `text-justify`?
fn justification_slot(c: char, mode: TextJustify) -> bool {
    match mode {
        TextJustify::None => false,
        TextJustify::InterCharacter => c != '\t',
        TextJustify::InterWord => is_word_separator(c),
        // `auto`: entre palabras y, en la escritura CJK (sin separadores), entre caracteres.
        TextJustify::Auto => {
            is_word_separator(c)
                || matches!(
                    linebreak::resolve_class(c),
                    LineBreak::ID
                        | LineBreak::H2
                        | LineBreak::H3
                        | LineBreak::JL
                        | LineBreak::JV
                        | LineBreak::JT
                        | LineBreak::NS
                )
        }
    }
}

impl<'a> Layout<'a> {
    /// Glifos de un run bidi de un solo sentido, con el espaciado de CSS aplicado.
    fn shape_run(&self, text: &str, style: &ComputedStyle, rtl: bool) -> Vec<Glyph> {
        let mut glyphs = self
            .fonts
            .shape_dir(text, &crate::font::font_request(style), rtl);
        apply_spacing(&mut glyphs, text, style);
        glyphs
    }

    fn text_item(&self, ifc: &mut Ifc<'a>, b: &'a LayoutBox, raw: &str, parent: Option<usize>) {
        let text = transform(raw, b.style.text_transform);
        let n = text.len();
        ifc.pieces.push(Piece::Text(ifc.texts.len()));
        ifc.texts.push(TextItem {
            b,
            text,
            glyphs: Vec::new(),
            parent,
            levels: vec![0; n],
            opp: vec![false; n + 1],
            hyphen: None,
        });
    }

    fn collect(
        &mut self,
        ifc: &mut Ifc<'a>,
        children: &'a [LayoutBox],
        parent: Option<usize>,
        cb: Cb,
    ) {
        for c in children {
            match &c.kind {
                BoxKind::Text(t) => self.text_item(ifc, c, t, parent),
                BoxKind::Marker {
                    text,
                    outside: false,
                } => self.text_item(ifc, c, text, parent),
                BoxKind::Marker { outside: true, .. } => {}
                _ if c.is_out_of_flow() => {
                    let idx = ifc.oof.len();
                    ifc.oof.push(c);
                    let kind = if c.is_float() {
                        SegKind::Float(idx)
                    } else {
                        SegKind::Abs(idx)
                    };
                    ifc.pieces.push(Piece::Seg(Seg::new(kind, 0.0, false)));
                }
                BoxKind::Inline => {
                    let s = &c.style;
                    let m = margin_edges(s, cb.width);
                    let b = border_edges(s);
                    let p = padding_edges(s, cb.width);
                    let left = m.left + b.left + p.left;
                    let right = m.right + b.right + p.right;
                    let rtl = s.direction == Direction::Rtl;
                    let idx = ifc.boxes.len();
                    ifc.boxes.push(InlineBox {
                        b: c,
                        parent,
                        start: if rtl { right } else { left },
                        end: if rtl { left } else { right },
                        start_margin: if rtl { m.right } else { m.left },
                        end_margin: if rtl { m.left } else { m.right },
                        level: 0,
                        iso: None,
                    });
                    let (start, end) = (ifc.boxes[idx].start, ifc.boxes[idx].end);
                    ifc.pieces
                        .push(Piece::Seg(Seg::new(SegKind::Start(idx), start, false)));
                    self.collect(ifc, &c.children, Some(idx), cb);
                    ifc.pieces
                        .push(Piece::Seg(Seg::new(SegKind::End(idx), end, false)));
                }
                BoxKind::LineBreak => {
                    ifc.pieces
                        .push(Piece::Seg(Seg::new(SegKind::Break, 0.0, true)));
                }
                BoxKind::BreakOpportunity => {
                    ifc.pieces
                        .push(Piece::Seg(Seg::new(SegKind::Opportunity, 0.0, true)));
                }
                // E4: las cajas de tabla nunca quedan en un IFC (van dentro de su envoltorio).
                BoxKind::Table | BoxKind::TablePart => {}
                BoxKind::InlineBlock
                | BoxKind::Replaced { .. }
                | BoxKind::Block
                | BoxKind::AnonymousBlock => {
                    let atomic = self.atomic(c, cb, parent);
                    let idx = ifc.atomics.len();
                    ifc.pieces.push(Piece::Seg(Seg::new(
                        SegKind::Atomic(idx),
                        atomic.width,
                        false,
                    )));
                    ifc.atomics.push(atomic);
                }
            }
        }
    }

    /// UAX #9 sobre el contenido del IFC: arma el texto del párrafo con los controles de
    /// `unicode-bidi` de cada caja (CSS Writing Modes 3 §2.4.2), las atómicas como U+FFFC y
    /// los saltos forzados como separadores de párrafo, y reparte los niveles resueltos entre
    /// los textos, las atómicas y las cajas.
    fn bidi_levels(&self, ifc: &mut Ifc<'a>, root: &ComputedStyle) {
        #[derive(Clone, Copy)]
        enum Origin {
            Text(usize, usize),
            Atomic(usize),
            /// El iniciador de aislamiento / el PDI de una caja.
            IsoStart(usize),
            IsoEnd(usize),
            Other,
        }
        let is_isolate = |c: char| ('\u{2066}'..='\u{2068}').contains(&c);
        fn push_other(chars: &mut Vec<char>, origin: &mut Vec<Origin>, cs: &[char]) {
            for &c in cs {
                chars.push(c);
                origin.push(Origin::Other);
            }
        }
        // X8 termina todas las incrustaciones al final del párrafo: en cada separador se
        // cierran los controles abiertos y se reabren después.
        fn paragraph_break(
            chars: &mut Vec<char>,
            origin: &mut Vec<Origin>,
            open: &[Controls],
            b: Origin,
        ) {
            for (_, close) in open.iter().rev() {
                push_other(chars, origin, close);
            }
            chars.push('\u{2029}');
            origin.push(b);
            for (o, _) in open {
                push_other(chars, origin, o);
            }
        }
        let mut chars: Vec<char> = Vec::new();
        let mut origin: Vec<Origin> = Vec::new();
        let mut open: Vec<Controls> = Vec::new();
        // La caja contenedora: `bidi-override`/`isolate-override` fuerzan la dirección de todo
        // su contenido; `plaintext` calcula el nivel de cada párrafo con P2–P3.
        let root_rtl = root.direction == Direction::Rtl;
        let base = match root.unicode_bidi {
            UnicodeBidi::Plaintext => None,
            _ => Some(u8::from(root_rtl)),
        };
        if matches!(
            root.unicode_bidi,
            UnicodeBidi::BidiOverride | UnicodeBidi::IsolateOverride
        ) {
            let o: &'static [char] = if root_rtl {
                &['\u{202E}']
            } else {
                &['\u{202D}']
            };
            push_other(&mut chars, &mut origin, o);
            open.push((o, &['\u{202C}']));
        }
        // Niveles explícitos de las cajas (para los bordes de sus fragmentos).
        let mut level_stack: Vec<u8> = vec![base.unwrap_or(0)];
        for p in &ifc.pieces {
            match p {
                Piece::Text(it) => {
                    for (i, c) in ifc.texts[*it].text.char_indices() {
                        if c == '\n' {
                            paragraph_break(&mut chars, &mut origin, &open, Origin::Text(*it, i));
                        } else {
                            chars.push(c);
                            origin.push(Origin::Text(*it, i));
                        }
                    }
                }
                Piece::Seg(s) => match s.kind {
                    SegKind::Start(b) => {
                        let style = &ifc.boxes[b].b.style;
                        let controls = bidi_controls(style);
                        match controls.0.split_first() {
                            Some((&c, rest)) if is_isolate(c) => {
                                chars.push(c);
                                origin.push(Origin::IsoStart(b));
                                push_other(&mut chars, &mut origin, rest);
                            }
                            _ => push_other(&mut chars, &mut origin, controls.0),
                        }
                        open.push(controls);
                        let parent = level_stack.last().copied().unwrap_or(0);
                        let level = if style.unicode_bidi == UnicodeBidi::Normal {
                            parent
                        } else {
                            let rtl = style.direction == Direction::Rtl;
                            let next = if rtl == (parent % 2 == 1) {
                                parent + 2
                            } else {
                                parent + 1
                            };
                            if next <= bidi::MAX_DEPTH {
                                next
                            } else {
                                parent
                            }
                        };
                        level_stack.push(level);
                        ifc.boxes[b].level = level;
                    }
                    SegKind::End(b) => {
                        if let Some((_, c)) = open.pop() {
                            match c.split_last() {
                                Some((&'\u{2069}', rest)) => {
                                    push_other(&mut chars, &mut origin, rest);
                                    chars.push('\u{2069}');
                                    origin.push(Origin::IsoEnd(b));
                                }
                                _ => push_other(&mut chars, &mut origin, c),
                            }
                        }
                        level_stack.pop();
                    }
                    SegKind::Atomic(a) => {
                        chars.push('\u{FFFC}');
                        origin.push(Origin::Atomic(a));
                    }
                    SegKind::Break => {
                        paragraph_break(&mut chars, &mut origin, &open, Origin::Other);
                    }
                    SegKind::Opportunity => push_other(&mut chars, &mut origin, &['\u{200B}']),
                    _ => {}
                },
            }
        }
        // Un nivel por párrafo (los separa cada salto forzado); con `plaintext`, el de P2–P3
        // o, sin caracteres fuertes, el de `direction` (CSS Writing Modes 3 §2.4.2).
        let (levels, paragraphs) =
            bidi::text_levels_with_fallback(&chars, base, u8::from(root_rtl));
        ifc.para_level = paragraphs.first().copied().unwrap_or(u8::from(root_rtl));
        ifc.para_levels = paragraphs;
        for (k, o) in origin.iter().enumerate() {
            match *o {
                Origin::Text(it, i) => {
                    let t = &mut ifc.texts[it];
                    let len = t.text[i..].chars().next().map_or(1, char::len_utf8);
                    for l in &mut t.levels[i..i + len] {
                        *l = levels[k];
                    }
                }
                Origin::Atomic(a) => ifc.atomics[a].level = levels[k],
                Origin::IsoStart(b) => {
                    let iso = ifc.boxes[b].iso.get_or_insert((levels[k], levels[k]));
                    iso.0 = levels[k];
                }
                Origin::IsoEnd(b) => {
                    let iso = ifc.boxes[b].iso.get_or_insert((levels[k], levels[k]));
                    iso.1 = levels[k];
                }
                Origin::Other => {}
            }
        }
    }

    /// Da forma a cada texto por runs de un mismo sentido (nivel par / impar).
    fn shape_texts(&self, ifc: &mut Ifc<'a>) {
        for t in &mut ifc.texts {
            let style = &t.b.style;
            let mut glyphs = Vec::with_capacity(t.text.len());
            let mut start = 0;
            while start < t.text.len() {
                let rtl = t.levels[start] % 2 == 1;
                let mut end = start;
                for (i, c) in t.text[start..].char_indices() {
                    if (t.levels[start + i] % 2 == 1) != rtl {
                        break;
                    }
                    end = start + i + c.len_utf8();
                }
                let run = &t.text[start..end];
                glyphs.extend(self.shape_run(run, style, rtl).into_iter().map(|mut g| {
                    g.cluster += start;
                    g
                }));
                start = end;
            }
            t.glyphs = glyphs;
            if t.text.contains(SHY) && style.hyphens != Hyphens::None {
                // CSS Text 3 §5.4: el UA elige el guion; U+2010 si la fuente lo tiene.
                let mut g = self.shape_run("\u{2010}", style, false);
                if g.iter().any(|g| g.id == 0) {
                    g = self.shape_run("-", style, false);
                }
                let w = g.iter().map(|g| g.advance).sum();
                t.hyphen = Some((g, w));
            }
        }
    }

    /// UAX #14 sobre el texto de todo el IFC (las atómicas como U+FFFC, los `<wbr>` como
    /// U+200B y los `<br>` como saltos), con las adaptaciones de CSS Text 3 §5.
    fn break_opportunities(&self, ifc: &mut Ifc<'a>, root: &ComputedStyle) {
        #[derive(Clone, Copy)]
        enum Origin {
            Text(usize, usize, usize),
            Atomic(usize),
            Other,
        }
        let mut chars: Vec<char> = Vec::new();
        let mut origin: Vec<Origin> = Vec::new();
        for p in &ifc.pieces {
            match p {
                Piece::Text(it) => {
                    for (i, c) in ifc.texts[*it].text.char_indices() {
                        chars.push(c);
                        origin.push(Origin::Text(*it, i, c.len_utf8()));
                    }
                }
                Piece::Seg(s) => match s.kind {
                    SegKind::Atomic(a) => {
                        chars.push('\u{FFFC}');
                        origin.push(Origin::Atomic(a));
                    }
                    SegKind::Break => {
                        chars.push('\n');
                        origin.push(Origin::Other);
                    }
                    SegKind::Opportunity => {
                        chars.push('\u{200B}');
                        origin.push(Origin::Other);
                    }
                    _ => {}
                },
            }
        }
        if chars.is_empty() {
            return;
        }
        let texts = &ifc.texts;
        let breaks = linebreak::line_breaks_with(&chars, |k, c| match origin[k] {
            Origin::Text(it, ..) => {
                let s = &texts[it].b.style;
                adapted_class(c, s.word_break, s.line_break)
            }
            _ => linebreak::resolve_class(c),
        });
        // Dueño de cada posición: (texto, caja padre), para la regla del ancestro común.
        let owner = |o: Origin| match o {
            Origin::Text(it, ..) => Some((Some(it), ifc.texts[it].parent)),
            Origin::Atomic(a) => Some((None, ifc.atomics[a].parent)),
            Origin::Other => None,
        };
        let chain = |mut p: Option<usize>| {
            let mut out = vec![p];
            while let Some(i) = p {
                p = ifc.boxes[i].parent;
                out.push(p);
            }
            out
        };
        let box_style = |p: Option<usize>| p.map_or(root, |c| &*ifc.boxes[c].b.style);
        let mut text_opps: Vec<(usize, usize)> = Vec::new();
        let mut atomic_opps: Vec<usize> = Vec::new();
        for k in 1..chars.len() {
            let before = origin[k - 1];
            // `break-spaces`: hay una oportunidad después de cada espacio preservado (§3).
            let preserved_space = match before {
                Origin::Text(it, i, _) => {
                    let t = &ifc.texts[it];
                    t.b.style.white_space == WhiteSpace::BreakSpaces
                        && t.text[i..].starts_with([' ', '\t'])
                }
                _ => false,
            };
            // CSS Text 3 §5.1: por compatibilidad web hay una oportunidad antes y después de
            // cada atómica, aunque el carácter vecino la suprima (U+00A0, `:`…). Los caracteres
            // de pegado (GL salvo U+00A0, WJ, ZWJ y las marcas combinantes) la siguen
            // suprimiendo, como en los demás navegadores (WPT line-breaking-atomic-*).
            // Después de una atómica, los espacios y saltos siguen sus reglas (LB4–LB7: no se
            // corta antes de ellos); antes de una atómica, un espacio sí deja cortar (aunque lo
            // preceda `(`, LB14).
            let glue = |c: char, after_atomic: bool| {
                let class = linebreak::resolve_class(c);
                c != '\u{A0}'
                    && (matches!(
                        class,
                        LineBreak::GL | LineBreak::WJ | LineBreak::ZWJ | LineBreak::CM
                    ) || after_atomic
                        && matches!(
                            class,
                            LineBreak::SP
                                | LineBreak::BK
                                | LineBreak::CR
                                | LineBreak::LF
                                | LineBreak::NL
                        ))
            };
            let atomic_edge = match (before, origin[k]) {
                (Origin::Atomic(_), Origin::Atomic(_)) => true,
                (Origin::Atomic(_), _) => !glue(chars[k], true),
                (_, Origin::Atomic(_)) => !glue(chars[k - 1], false),
                _ => false,
            };
            // ¿Qué `white-space` y `line-break` controlan la posición?
            let style: &ComputedStyle = match (owner(before), owner(origin[k])) {
                (Some((Some(a), _)), Some((Some(b), _))) if a == b => &ifc.texts[a].b.style,
                (Some((_, pa)), Some((_, pb))) => {
                    let ca = chain(pa);
                    let cb = chain(pb);
                    box_style(ca.iter().find(|p| cb.contains(p)).copied().flatten())
                }
                (Some((Some(a), _)), None) => &ifc.texts[a].b.style,
                (Some((None, pa)), None) => box_style(pa),
                _ => continue,
            };
            // `line-break: anywhere` (CSS Text 3 §5.3): oportunidad alrededor de cada unidad
            // tipográfica, también junto a la puntuación y los espacios; no dentro de un
            // grafema (marcas combinantes, ZWJ).
            let anywhere = style.line_break == TextLineBreak::Anywhere
                && !matches!(
                    linebreak::resolve_class(chars[k]),
                    LineBreak::CM | LineBreak::ZWJ
                )
                && linebreak::resolve_class(chars[k - 1]) != LineBreak::ZWJ;
            // `loose`: antes de ‐ y – si los precede un carácter de clase ID (CSS Text 3 §5.3).
            let loose_hyphen = style.line_break == TextLineBreak::Loose
                && matches!(chars[k], '\u{2010}' | '\u{2013}')
                && match origin[k - 1] {
                    Origin::Text(it, ..) => {
                        let s = &ifc.texts[it].b.style;
                        adapted_class(chars[k - 1], s.word_break, s.line_break) == LineBreak::ID
                    }
                    _ => false,
                };
            if breaks[k] == Break::Prohibited
                && !preserved_space
                && !atomic_edge
                && !anywhere
                && !loose_hyphen
            {
                continue;
            }
            if !style.white_space.wraps() {
                continue;
            }
            match before {
                Origin::Text(it, i, len) => {
                    let t = &ifc.texts[it];
                    // `hyphens: none`: el guion opcional no es una oportunidad (§5.4).
                    if t.text[i..].starts_with(SHY) && t.b.style.hyphens == Hyphens::None {
                        continue;
                    }
                    text_opps.push((it, i + len));
                }
                Origin::Atomic(a) => atomic_opps.push(a),
                Origin::Other => {}
            }
        }
        for (it, e) in text_opps {
            ifc.texts[it].opp[e] = true;
        }
        for a in atomic_opps {
            ifc.atomics[a].break_after = true;
        }
    }

    /// Segmenta los textos (palabras irrompibles, espacios y saltos) y arma la lista final de
    /// segmentos del IFC.
    fn segment(&self, ifc: &mut Ifc<'a>) {
        let pieces = std::mem::take(&mut ifc.pieces);
        let mut segs: Vec<Seg> = Vec::with_capacity(pieces.len() * 4);
        for p in &pieces {
            match p {
                Piece::Seg(s) => {
                    let mut s = *s;
                    if let SegKind::Atomic(a) = s.kind {
                        s.break_after = ifc.atomics[a].break_after;
                    }
                    segs.push(s);
                }
                Piece::Text(it) => {
                    let item = *it;
                    let t = &ifc.texts[item];
                    let collapsible = t.b.style.white_space.collapses_spaces();
                    let hyphens = t.hyphen.is_some();
                    let mut word_start: Option<usize> = None;
                    let push_word = |segs: &mut Vec<Seg>, start: usize, end: usize| {
                        segs.push(Seg {
                            kind: SegKind::Word { item, start, end },
                            width: glyph_width(&t.glyphs, start, end),
                            break_after: t.opp[end],
                            hyphen: hyphens && t.opp[end] && t.text[..end].ends_with(SHY),
                        });
                    };
                    for (i, c) in t.text.char_indices() {
                        let end = i + c.len_utf8();
                        match c {
                            '\n' => {
                                if let Some(s) = word_start.take() {
                                    push_word(&mut segs, s, i);
                                }
                                segs.push(Seg::new(SegKind::Break, 0.0, true));
                            }
                            ' ' | '\t' => {
                                if let Some(s) = word_start.take() {
                                    push_word(&mut segs, s, i);
                                }
                                let mut width = glyph_width(&t.glyphs, i, end);
                                if c == '\t' {
                                    // Aproximación inicial: 8 espacios; la línea lo lleva a
                                    // la siguiente parada.
                                    width *= 8.0;
                                }
                                segs.push(Seg::new(
                                    SegKind::Space {
                                        item,
                                        start: i,
                                        end,
                                        collapsible,
                                    },
                                    width,
                                    t.opp[end],
                                ));
                            }
                            _ => {
                                let s = *word_start.get_or_insert(i);
                                if t.opp[end] {
                                    push_word(&mut segs, s, end);
                                    word_start = None;
                                }
                            }
                        }
                    }
                    if let Some(s) = word_start {
                        push_word(&mut segs, s, t.text.len());
                    }
                }
            }
        }
        // Una oportunidad justo antes del cierre de cajas inline va después de los cierres: el
        // borde final de la caja se queda en la línea.
        for i in 0..segs.len() {
            if segs[i].break_after
                && matches!(
                    segs[i].kind,
                    SegKind::Word { .. } | SegKind::Space { .. } | SegKind::Atomic(_)
                )
            {
                let mut j = i;
                while j + 1 < segs.len() && matches!(segs[j + 1].kind, SegKind::End(_)) {
                    j += 1;
                }
                if j != i {
                    segs[i].break_after = false;
                    segs[j].break_after = true;
                }
            }
        }
        ifc.segs = segs;
    }

    /// Dimensiona una inline-level atómica con su margin box en el origen.
    fn atomic(&mut self, b: &'a LayoutBox, cb: Cb, parent: Option<usize>) -> Atomic {
        let margins = margin_edges(&b.style, cb.width);
        let content_w = self.fit_width(b, cb, margins.horizontal());
        let mut scratch = FloatContext::default();
        // E9: una inline-block de otro modo de escritura es la raíz de un flujo ortogonal.
        let fragment = if self.is_orthogonal(b) {
            self.orthogonal_fragment(b, cb)
        } else {
            self.box_contents(
                b,
                cb,
                margins.left,
                margins.top,
                content_w,
                margins,
                &mut scratch,
                false,
            )
            .0
        };
        let outer = fragment.margin_rect();
        let visible =
            b.style.overflow_x == Overflow::Visible && b.style.overflow_y == Overflow::Visible;
        // E9: en un flujo vertical con baseline central, la baseline sintetizada de una caja
        // sin baseline propia es la central: la mitad de su margin box (CSS Inline 3 §4.1).
        let frame = self.frame;
        let synth_of = |h: f32| crate::writing_mode::synthesized_baseline(frame, &b.style, h);
        let synth = synth_of(outer.height);
        // La baseline de un inline-block es la de su última line box; si no tiene o su
        // overflow no es visible, el borde inferior del margin box (§10.8.1).
        let baseline = match (&b.kind, visible) {
            // Controles de una línea: la baseline de su texto (aunque recorten su contenido).
            _ if b
                .control
                .as_ref()
                .is_some_and(crate::controls::Control::single_line) =>
            {
                fragment.baseline.map_or(synth, |y| y - outer.y)
            }
            // El resto de los controles (casillas, barras, list box, textarea): el borde inferior
            // del margin box, como un contenedor de scroll.
            _ if b.control.is_some() => synth,
            // inline-flex: la baseline del contenedor es la de su primer ítem (Flexbox §8.5).
            (BoxKind::InlineBlock, _) if crate::flex::is_flex_container(b) => {
                crate::flex::container_baseline(&fragment, &synth_of).map_or(synth, |y| y - outer.y)
            }
            // E4: inline-grid (CSS Grid 2 §9.1) e inline-table (CSS 2.2 §17.5.3: la baseline de
            // la primera fila).
            (BoxKind::InlineBlock, _) if crate::grid::is_grid_container(b) => {
                crate::grid::container_baseline(&fragment).map_or(synth, |y| y - outer.y)
            }
            (BoxKind::InlineBlock, _) if crate::table::is_table_wrapper(b) => {
                fragment.baseline.map_or(synth, |y| y - outer.y)
            }
            (BoxKind::InlineBlock, true) => last_line_baseline(&fragment.children).unwrap_or(synth),
            _ => synth,
        };
        Atomic {
            fragment,
            baseline,
            width: outer.width,
            height: outer.height,
            parent,
            level: 0,
            break_after: false,
        }
    }

    /// Layout del contenido inline de `b`: devuelve las line boxes (y los floats que contiene) y
    /// la altura total.
    pub(crate) fn inline_content(
        &mut self,
        b: &'a LayoutBox,
        cb: Cb,
        floats: &mut FloatContext,
    ) -> (Vec<Fragment>, f32) {
        let mut ifc = Ifc {
            texts: Vec::new(),
            boxes: Vec::new(),
            atomics: Vec::new(),
            oof: Vec::new(),
            pieces: Vec::new(),
            segs: Vec::new(),
            para_level: 0,
            para_levels: Vec::new(),
        };
        let root = &b.style;
        let root_rtl = root.direction == Direction::Rtl;
        self.collect(&mut ifc, &b.children, None, cb);
        self.bidi_levels(&mut ifc, root);
        self.shape_texts(&mut ifc);
        self.break_opportunities(&mut ifc, root);
        self.segment(&mut ifc);
        let root_lh = self.line_height(root);
        let indent = root.text_indent.resolve(cb.width);
        let mut out: Vec<Fragment> = Vec::new();
        let mut y = cb.y;
        let mut i = 0;
        let mut open: Vec<usize> = Vec::new();
        let mut first = true;
        // Párrafo bidi de la línea: cada salto forzado empieza uno.
        let mut paragraph = 0;
        while i < ifc.segs.len() {
            let line_start = i;
            if let Some(&level) = ifc.para_levels.get(paragraph).or(ifc.para_levels.last()) {
                ifc.para_level = level;
            }
            let open_at_start = open.clone();
            let mut guess = root_lh;
            let mut attempts = 0;
            // CSS 2.2 §9.5: la line box esquiva los floats en toda su altura (si resulta más alta
            // que lo estimado, se rehace con la banda de esa altura) y, si junto a ellos no cabe
            // nada, baja hasta el siguiente borde inferior de un float. Solo se reintenta si la
            // línea no colocó floats ni cajas posicionadas (efectos laterales).
            'attempt: loop {
                i = line_start;
                open.clone_from(&open_at_start);
                let (mut l, mut r) = floats.band(y, guess, cb.x, cb.x + cb.width);
                let narrowed = |l: f32, r: f32| l > cb.x + 0.01 || r < cb.x + cb.width - 0.01;
                let line_indent = if first { indent } else { 0.0 };
                let mut line: Vec<usize> = Vec::new();
                let mut width = line_indent;
                let mut last_break: Option<usize> = None;
                let mut has_content = false;
                let mut forced = false;
                let mut side_effects = false;
                // Cajas absolutas de la línea: se registran al confirmarla, cuando se conoce su
                // altura (índice, x de la pluma, ¿había contenido antes?).
                let mut abs_here: Vec<(usize, f32, bool)> = Vec::new();
                while i < ifc.segs.len() {
                    // Un tab preservado avanza hasta la siguiente parada: múltiplos de 8
                    // espacios desde el inicio de la línea (CSS Text 3 §4.1.3, tab-size: 8).
                    if let SegKind::Space {
                        item,
                        start,
                        end,
                        collapsible: false,
                    } = ifc.segs[i].kind
                        && ifc.texts[item].text[start..].starts_with('\t')
                    {
                        let interval = glyph_width(&ifc.texts[item].glyphs, start, end) * 8.0;
                        if interval > 0.0 {
                            let next = ((width + 0.01) / interval).floor() * interval + interval;
                            ifc.segs[i].width = next - width;
                        }
                    }
                    let seg = ifc.segs[i];
                    match seg.kind {
                        SegKind::Space {
                            collapsible: true, ..
                        } if !has_content => {
                            i += 1;
                            continue;
                        }
                        SegKind::Break => {
                            line.push(i);
                            i += 1;
                            forced = true;
                            break;
                        }
                        SegKind::Float(idx) => {
                            let f = self.float_box(ifc.oof[idx], cb, y, floats);
                            out.push(f);
                            (l, r) = floats.band(y, guess, cb.x, cb.x + cb.width);
                            side_effects = true;
                            i += 1;
                            continue;
                        }
                        SegKind::Abs(idx) => {
                            // E9: la pluma en un contenedor `rtl` avanza desde la derecha.
                            let x = if root_rtl { r - width } else { l + width };
                            abs_here.push((idx, x, has_content));
                            i += 1;
                            continue;
                        }
                        _ => {}
                    }
                    let available = r - l;
                    let is_space = matches!(seg.kind, SegKind::Space { .. });
                    if !has_content
                        && !side_effects
                        && matches!(seg.kind, SegKind::Word { .. } | SegKind::Atomic(_))
                        && width + seg.width > available + 0.01
                        && narrowed(l, r)
                        && let Some(next) = floats.next_bottom(y, guess)
                    {
                        y = next;
                        continue 'attempt;
                    }
                    // El cierre de una caja no deja de colgar los espacios que lo preceden (la
                    // fase 2 los quita igual): no cuentan para ver si el cierre cabe.
                    let hanging = if matches!(seg.kind, SegKind::End(_)) {
                        line.iter()
                            .rev()
                            .filter(|&&s| !matches!(ifc.segs[s].kind, SegKind::End(_)))
                            .map_while(|&s| match ifc.segs[s].kind {
                                SegKind::Space {
                                    item, collapsible, ..
                                } if collapsible || ifc.texts[item].b.style.white_space.wraps() => {
                                    Some(ifc.segs[s].width)
                                }
                                _ => None,
                            })
                            .sum::<f32>()
                    } else {
                        0.0
                    };
                    if has_content
                        && !is_space
                        && width - hanging + seg.width > available + 0.01
                        && let Some(pos) = last_break
                    {
                        i = line[pos] + 1;
                        line.truncate(pos + 1);
                        break;
                    }
                    // overflow-wrap: una palabra que no cabe sola en la línea se parte.
                    if let SegKind::Word { item, start, end } = seg.kind
                        && width + seg.width > available + 0.01
                        && last_break.is_none()
                        && ifc.texts[item].b.style.overflow_wrap != OverflowWrap::Normal
                        && end - start
                            > ifc.texts[item].text[start..end]
                                .chars()
                                .next()
                                .map_or(1, char::len_utf8)
                    {
                        let text = &ifc.texts[item];
                        let room = (available - width).max(0.0);
                        let mut split = start;
                        let mut acc = 0.0;
                        for (off, ch) in text.text[start..end].char_indices() {
                            let cs = start + off;
                            let ce = cs + ch.len_utf8();
                            let w = glyph_width(&text.glyphs, cs, ce);
                            if acc + w > room + 0.01 && split > start {
                                break;
                            }
                            acc += w;
                            split = ce;
                        }
                        if split < end {
                            let first_w = glyph_width(&text.glyphs, start, split);
                            let rest_w = glyph_width(&text.glyphs, split, end);
                            ifc.segs[i] = Seg::new(
                                SegKind::Word {
                                    item,
                                    start,
                                    end: split,
                                },
                                first_w,
                                true,
                            );
                            ifc.segs.insert(
                                i + 1,
                                Seg {
                                    kind: SegKind::Word {
                                        item,
                                        start: split,
                                        end,
                                    },
                                    width: rest_w,
                                    break_after: seg.break_after,
                                    hyphen: seg.hyphen,
                                },
                            );
                            if has_content {
                                // Cortar antes de la palabra si la línea ya tiene contenido.
                                break;
                            }
                        }
                    }
                    let seg = ifc.segs[i];
                    line.push(i);
                    width += seg.width;
                    if matches!(seg.kind, SegKind::Word { .. } | SegKind::Atomic(_)) {
                        has_content = true;
                    }
                    // Un corte en un guion opcional solo vale si el guion también cabe.
                    let hyphen_fits = |item: usize| {
                        ifc.texts[item]
                            .hyphen
                            .as_ref()
                            .is_none_or(|(_, w)| width + w <= available + 0.01)
                    };
                    let fits = match seg.kind {
                        SegKind::Word { item, .. } if seg.hyphen => hyphen_fits(item),
                        _ => true,
                    };
                    if seg.break_after && has_content && fits {
                        last_break = Some(line.len() - 1);
                    }
                    i += 1;
                }
                // Las cajas abiertas al final de la línea, para la siguiente.
                for &s in &line {
                    match ifc.segs[s].kind {
                        SegKind::Start(b) => open.push(b),
                        SegKind::End(b) => {
                            if let Some(p) = open.iter().rposition(|&x| x == b) {
                                open.remove(p);
                            }
                        }
                        _ => {}
                    }
                }
                // Fase 2: los espacios colapsables al final de la línea se quitan.
                let mut trimmed = line.clone();
                while let Some(pos) = trimmed
                    .iter()
                    .rposition(|&s| !matches!(ifc.segs[s].kind, SegKind::End(_) | SegKind::Break))
                {
                    match ifc.segs[trimmed[pos]].kind {
                        // Se quitan los colapsables; los preservados que pueden partirse
                        // (pre-wrap) cuelgan y no cuentan; los de `pre` se quedan.
                        SegKind::Space {
                            item, collapsible, ..
                        } if collapsible || ifc.texts[item].b.style.white_space.wraps() => {
                            trimmed.remove(pos);
                        }
                        _ => break,
                    }
                }
                let empty = !forced
                    && !trimmed.iter().any(|&s| match ifc.segs[s].kind {
                        SegKind::Word { .. } | SegKind::Atomic(_) | SegKind::Space { .. } => true,
                        SegKind::Start(_) | SegKind::End(_) => ifc.segs[s].width != 0.0,
                        _ => false,
                    });
                if empty {
                    self.place_line_abs(&ifc, &abs_here, cb, y, 0.0, root_rtl);
                    if line.is_empty() && i < ifc.segs.len() {
                        // Nada cupo: forzar avance para no ciclar.
                        i += 1;
                    }
                    break 'attempt;
                }
                let last = i >= ifc.segs.len();
                let (mut fragment, height) = self.build_line(
                    &ifc,
                    b,
                    &trimmed,
                    &open_at_start,
                    l,
                    r - l,
                    y,
                    line_indent,
                    last || forced,
                    first,
                );
                if !side_effects && attempts < 4 && height > guess + 0.01 {
                    let (l2, r2) = floats.band(y, height, cb.x, cb.x + cb.width);
                    if l2 > l + 0.01 || r2 < r - 0.01 {
                        guess = height;
                        attempts += 1;
                        continue 'attempt;
                    }
                }
                if forced {
                    paragraph += 1;
                }
                self.offset_relative_inlines(&mut fragment, cb);
                self.place_line_abs(&ifc, &abs_here, cb, y, height, root_rtl);
                out.push(fragment);
                y += height;
                first = false;
                break 'attempt;
            }
        }
        (out, y - cb.y)
    }

    /// Posición estática de las cajas absolutas de una línea (CSS Position 3 §4.3): si su
    /// `display` original era inline-level, donde estaba la pluma; si no, como un bloque: al
    /// inicio del containing block, debajo de la línea si antes había contenido.
    fn place_line_abs(
        &mut self,
        ifc: &Ifc<'a>,
        abs: &[(usize, f32, bool)],
        cb: Cb,
        y: f32,
        height: f32,
        rtl: bool,
    ) {
        for &(idx, x, had_content) in abs {
            let b = ifc.oof[idx];
            if b.style.original_display.is_inline_level() {
                self.add_abs(b, x, y, rtl);
            } else {
                let x = if rtl { cb.x + cb.width } else { cb.x };
                self.add_abs(b, x, if had_content { y + height } else { y }, rtl);
            }
        }
    }

    /// `position: relative` en cajas inline: desplaza cada fragmento con sus descendientes,
    /// sin afectar al resto de la línea (CSS 2.2 §9.4.3). Los atómicos ya se desplazan al
    /// hacer su layout.
    fn offset_relative_inlines(&self, f: &mut Fragment, cb: Cb) {
        for c in &mut f.children {
            if c.kind == FragmentKind::Inline
                && c.style.position == bf_style::properties::types::Position::Relative
            {
                let (dx, dy) = self.relative_offset(&c.style, cb);
                c.translate(dx, dy);
            }
            if matches!(c.kind, FragmentKind::Inline | FragmentKind::Line) {
                self.offset_relative_inlines(c, cb);
            }
        }
    }

    /// Desplazamiento de la baseline de una caja respecto de la de su padre (positivo = abajo).
    fn baseline_shift(
        &self,
        va: &VerticalAlign,
        parent: &ComputedStyle,
        own: &ComputedStyle,
        top_from_baseline: f32,
        bottom_from_baseline: f32,
    ) -> f32 {
        let pm = self.metrics(parent);
        let pfs = parent.font_size_px();
        match va {
            VerticalAlign::Keyword(k) => match k {
                VerticalAlignKeyword::Baseline
                | VerticalAlignKeyword::Top
                | VerticalAlignKeyword::Bottom => 0.0,
                VerticalAlignKeyword::Sub => pfs / 5.0,
                VerticalAlignKeyword::Super => -pfs / 3.0,
                VerticalAlignKeyword::TextTop => -pm.ascent + top_from_baseline,
                VerticalAlignKeyword::TextBottom => pm.descent - bottom_from_baseline,
                VerticalAlignKeyword::Middle => {
                    -pm.x_height / 2.0 - (bottom_from_baseline - top_from_baseline) / 2.0
                }
            },
            VerticalAlign::LengthPercentage(lp) => -lp.resolve(self.line_height(own)),
        }
    }

    /// Construye la line box: orden visual (UAX #9 L1–L2), posiciones, alineación y fragmentos.
    #[allow(clippy::too_many_arguments)]
    fn build_line(
        &mut self,
        ifc: &Ifc<'a>,
        container: &'a LayoutBox,
        line: &[usize],
        open_at_start: &[usize],
        x0: f32,
        available: f32,
        y: f32,
        indent: f32,
        last: bool,
        first_line: bool,
    ) -> (Fragment, f32) {
        #[derive(Clone, Copy)]
        enum LeafKind {
            Text {
                item: usize,
                start: usize,
                end: usize,
                space: bool,
                hyphen: bool,
            },
            Atomic(usize),
            /// Borde start o end de una caja inline, con el margen de ese lado.
            Edge {
                b: usize,
                margin: f32,
            },
        }
        #[derive(Clone, Copy)]
        struct Leaf {
            kind: LeafKind,
            width: f32,
            level: u8,
            /// Caja inline más interna que lo contiene.
            owner: Option<usize>,
            /// Oportunidades de justificación.
            slots: usize,
            x: f32,
        }
        let root = &container.style;
        let para_level = ifc.para_level;
        let rtl = para_level % 2 == 1;

        // ¿La línea termina en un guion opcional?
        let hyphen_seg = line
            .iter()
            .rev()
            .find(|&&s| matches!(ifc.segs[s].kind, SegKind::Word { .. } | SegKind::Atomic(_)))
            .copied()
            .filter(|&s| ifc.segs[s].hyphen && !last);

        // Hojas en orden lógico: los textos se parten donde cambia el nivel bidi.
        let mut leaves: Vec<Leaf> = Vec::with_capacity(line.len());
        for &s in line {
            let seg = ifc.segs[s];
            match seg.kind {
                SegKind::Word { item, start, end } => {
                    let t = &ifc.texts[item];
                    let mut a = start;
                    while a < end {
                        let level = t.levels[a];
                        let mut b = a;
                        for (i, c) in t.text[a..end].char_indices() {
                            if t.levels[a + i] != level {
                                break;
                            }
                            b = a + i + c.len_utf8();
                        }
                        let hyphen = hyphen_seg == Some(s) && b == end;
                        let mut width = glyph_width(&t.glyphs, a, b);
                        if hyphen && let Some((_, w)) = &t.hyphen {
                            width += w;
                        }
                        leaves.push(Leaf {
                            kind: LeafKind::Text {
                                item,
                                start: a,
                                end: b,
                                space: false,
                                hyphen,
                            },
                            width,
                            level,
                            owner: t.parent,
                            slots: 0,
                            x: 0.0,
                        });
                        a = b;
                    }
                }
                SegKind::Space {
                    item, start, end, ..
                } => {
                    let t = &ifc.texts[item];
                    leaves.push(Leaf {
                        kind: LeafKind::Text {
                            item,
                            start,
                            end,
                            space: true,
                            hyphen: false,
                        },
                        width: seg.width,
                        level: t.levels[start],
                        owner: t.parent,
                        slots: 0,
                        x: 0.0,
                    });
                }
                SegKind::Start(b) | SegKind::End(b) => {
                    let bx = &ifc.boxes[b];
                    let margin = if matches!(seg.kind, SegKind::Start(_)) {
                        bx.start_margin
                    } else {
                        bx.end_margin
                    };
                    leaves.push(Leaf {
                        kind: LeafKind::Edge { b, margin },
                        width: seg.width,
                        level: bx.level,
                        owner: Some(b),
                        slots: 0,
                        x: 0.0,
                    });
                }
                SegKind::Atomic(a) => {
                    let at = &ifc.atomics[a];
                    leaves.push(Leaf {
                        kind: LeafKind::Atomic(a),
                        width: seg.width,
                        level: at.level,
                        owner: at.parent,
                        slots: 0,
                        x: 0.0,
                    });
                }
                _ => {}
            }
        }

        // L1: los espacios (y tabs) del final de la línea y los tabs con los espacios que los
        // preceden vuelven al nivel del párrafo.
        let is_tab = |l: &Leaf| match l.kind {
            LeafKind::Text {
                item,
                start,
                space: true,
                ..
            } => ifc.texts[item].text[start..].starts_with('\t'),
            _ => false,
        };
        let mut trailing = true;
        for k in (0..leaves.len()).rev() {
            match leaves[k].kind {
                LeafKind::Edge { .. } => {}
                LeafKind::Text { space: true, .. } if trailing || is_tab(&leaves[k]) => {
                    leaves[k].level = para_level;
                    trailing = true;
                }
                _ => trailing = false,
            }
        }
        // Bordes de las cajas inline (CSS Writing Modes 3 §2.4.3): no son caracteres para
        // UAX #9. Los de una caja con contenido en la línea se colocan después de reordenar: el
        // borde start en el lado start (según el `direction` de la caja) del fragmento que
        // tiene su primer contenido lógico, y el end en el lado end del que tiene el último.
        // Los de una caja vacía se reordenan con el resto, con el menor nivel de sus vecinos
        // (como un neutro entre ellos, N1–N2) y nunca menos que el suyo.
        let inside = |owner: Option<usize>, b: usize| {
            let mut p = owner;
            while let Some(i) = p {
                if i == b {
                    return true;
                }
                p = ifc.boxes[i].parent;
            }
            false
        };
        let is_edge = |l: &Leaf| matches!(l.kind, LeafKind::Edge { .. });
        // Por borde con contenido: ¿es el start? (hay contenido de la caja después de él).
        let floating: Vec<Option<bool>> = (0..leaves.len())
            .map(|k| {
                let LeafKind::Edge { b, .. } = leaves[k].kind else {
                    return None;
                };
                let mut own = leaves
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| !is_edge(l) && inside(l.owner, b))
                    .map(|(j, _)| j);
                own.next().map(|first| first > k)
            })
            .collect();
        for k in 0..leaves.len() {
            let LeafKind::Edge { b, .. } = leaves[k].kind else {
                continue;
            };
            if floating[k].is_some() {
                continue;
            }
            let content = |j: &usize| !is_edge(&leaves[*j]);
            let next = (k + 1..leaves.len()).find(content);
            let prev = (0..k).rev().find(content);
            let level = match (prev, next) {
                (Some(p), Some(n)) => leaves[p].level.min(leaves[n].level),
                (Some(j), None) | (None, Some(j)) => leaves[j].level,
                (None, None) => leaves[k].level,
            };
            leaves[k].level = level.max(ifc.boxes[b].level);
        }
        // Los iniciadores de aislamiento y los PDI de las cajas con contenido en la línea entran
        // en L2 como barreras sin ancho (con su nivel resuelto; L1 los devuelve al nivel del
        // párrafo al final de la línea) y luego se descartan.
        let last_content = (0..leaves.len()).rev().find(|&k| {
            !is_edge(&leaves[k]) && !matches!(leaves[k].kind, LeafKind::Text { space: true, .. })
        });
        let fixed: Vec<(Option<usize>, u8)> = (0..leaves.len())
            .filter_map(|k| match (leaves[k].kind, floating[k]) {
                (_, None) => Some((Some(k), leaves[k].level)),
                (LeafKind::Edge { b, .. }, Some(start)) => {
                    let (open, close) = ifc.boxes[b].iso?;
                    let level = if last_content.is_none_or(|c| k > c) {
                        para_level
                    } else if start {
                        open
                    } else {
                        close
                    };
                    Some((None, level))
                }
                _ => None,
            })
            .collect();
        let levels: Vec<u8> = fixed.iter().map(|&(_, l)| l).collect();
        let mut order: Vec<usize> = bidi::reorder(&levels)
            .into_iter()
            .filter_map(|i| fixed[i].0)
            .collect();
        // Los bordes de las cajas más internas primero: el fragmento de la caja exterior
        // incluye ya los bordes de las interiores.
        let depth = |b: usize| {
            let mut d = 0;
            let mut p = ifc.boxes[b].parent;
            while let Some(i) = p {
                d += 1;
                p = ifc.boxes[i].parent;
            }
            d
        };
        let mut edges: Vec<(usize, usize, bool)> = (0..leaves.len())
            .filter_map(|k| match (leaves[k].kind, floating[k]) {
                (LeafKind::Edge { b, .. }, Some(start)) => Some((k, b, start)),
                _ => None,
            })
            .collect();
        edges.sort_by_key(|&(k, b, _)| (std::cmp::Reverse(depth(b)), k));
        for (k, b, start) in edges {
            let within: Vec<usize> = (0..order.len())
                .filter(|&p| inside(leaves[order[p]].owner, b))
                .collect();
            let anchor = if start {
                within.iter().copied().min_by_key(|&p| order[p])
            } else {
                within.iter().copied().max_by_key(|&p| order[p])
            };
            let Some(anchor) = anchor else {
                continue;
            };
            let mut lo = anchor;
            while lo > 0 && inside(leaves[order[lo - 1]].owner, b) {
                lo -= 1;
            }
            let mut hi = anchor;
            while hi + 1 < order.len() && inside(leaves[order[hi + 1]].owner, b) {
                hi += 1;
            }
            let rtl_box = ifc.boxes[b].b.style.direction == Direction::Rtl;
            // Start de una caja ltr o end de una rtl: a la izquierda del fragmento.
            let pos = if start != rtl_box { lo } else { hi + 1 };
            order.insert(pos, k);
        }

        // Justificación (CSS Text 3 §7.3): oportunidades por hoja; la del último carácter de
        // la línea no cuenta.
        let used: f32 = leaves.iter().map(|l| l.width).sum::<f32>() + indent;
        let free = available - used;
        // CSS Text 3 §6.3: la última línea (y la que acaba en un salto forzado) se alinea con
        // `text-align-last`; `auto` es `text-align`, salvo `justify`, que alinea al inicio.
        let align = if last {
            match root.text_align_last {
                TextAlignLast::Auto if root.text_align == TextAlign::Justify => TextAlign::Start,
                TextAlignLast::Auto | TextAlignLast::MatchParent => root.text_align,
                TextAlignLast::Start => TextAlign::Start,
                TextAlignLast::End => TextAlign::End,
                TextAlignLast::Left => TextAlign::Left,
                TextAlignLast::Right => TextAlign::Right,
                TextAlignLast::Center => TextAlign::Center,
                TextAlignLast::Justify => TextAlign::Justify,
            }
        } else {
            root.text_align
        };
        let justify =
            align == TextAlign::Justify && root.text_justify != TextJustify::None && free > 0.0;
        let mut last_text_char: Option<(usize, usize)> = None;
        if justify {
            for l in &mut leaves {
                if let LeafKind::Text {
                    item, start, end, ..
                } = l.kind
                {
                    let t = &ifc.texts[item];
                    let mode = t.b.style.text_justify;
                    l.slots = t.text[start..end]
                        .chars()
                        .filter(|&c| justification_slot(c, mode))
                        .count();
                    if let Some((i, c)) = t.text[start..end].char_indices().next_back() {
                        last_text_char = Some((item, start + i));
                        let _ = c;
                    }
                }
            }
            // El último carácter de la línea no recibe espacio después.
            if let Some((item, byte)) = last_text_char {
                let t = &ifc.texts[item];
                let c = t.text[byte..].chars().next().unwrap_or(' ');
                if justification_slot(c, t.b.style.text_justify)
                    && let Some(l) = leaves.iter_mut().rev().find(|l| {
                        matches!(l.kind, LeafKind::Text { item: i, start, end, .. }
                            if i == item && start <= byte && byte < end)
                    })
                {
                    l.slots = l.slots.saturating_sub(1);
                }
            }
        }
        let total_slots: usize = leaves.iter().map(|l| l.slots).sum();
        let extra = if justify && total_slots > 0 {
            free / total_slots as f32
        } else {
            0.0
        };
        // CSS Text 3 §6.1: `start`/`end` según la dirección; si el contenido no cabe, se
        // alinea al inicio y desborda por el final.
        let start_shift = if rtl { free } else { 0.0 };
        let end_shift = if rtl { 0.0 } else { free };
        let shift = if free < 0.0 {
            if rtl { free } else { 0.0 }
        } else {
            match align {
                TextAlign::Left => 0.0,
                TextAlign::Right => free,
                TextAlign::Center => free / 2.0,
                TextAlign::End => end_shift,
                TextAlign::Justify if extra > 0.0 => 0.0,
                _ => start_shift,
            }
        };
        let mut x = x0 + shift + if rtl { 0.0 } else { indent };
        for &k in &order {
            let l = &mut leaves[k];
            l.width += extra * l.slots as f32;
            l.x = x;
            x += l.width;
        }

        // Alineación vertical: desplazamientos de baseline de cada caja respecto de la raíz.
        let root_m = self.metrics(root);
        let root_lh = self.line_height(root);
        let root_hl = half_leading(&root_m, root_lh);
        let mut box_offset: Vec<Option<f32>> = vec![None; ifc.boxes.len()];
        let offset_of = |this: &Self, idx: Option<usize>, cache: &mut Vec<Option<f32>>| -> f32 {
            fn go<'b>(
                this: &Layout<'b>,
                ifc: &Ifc<'b>,
                root: &ComputedStyle,
                idx: Option<usize>,
                cache: &mut Vec<Option<f32>>,
            ) -> f32 {
                let Some(i) = idx else { return 0.0 };
                if let Some(v) = cache[i] {
                    return v;
                }
                let bx = &ifc.boxes[i];
                let parent_style = bx.parent.map_or(root, |p| &ifc.boxes[p].b.style);
                let s = &bx.b.style;
                let m = this.metrics(s);
                let hl = half_leading(&m, this.line_height(s));
                let shift = this.baseline_shift(
                    &s.vertical_align,
                    parent_style,
                    s,
                    m.ascent + hl,
                    m.descent + hl,
                );
                let v = go(this, ifc, root, bx.parent, cache) + shift;
                cache[i] = Some(v);
                v
            }
            go(this, ifc, root, idx, cache)
        };
        let mut top = -(root_m.ascent + root_hl);
        let mut bottom = root_m.descent + root_hl;
        let mut deferred: Vec<(bool, f32)> = Vec::new();
        let mut present: Vec<usize> = open_at_start.to_vec();
        for &s in line {
            if let SegKind::Start(b) = ifc.segs[s].kind {
                present.push(b);
            }
        }
        for &bi in &present {
            let s = &ifc.boxes[bi].b.style;
            let m = self.metrics(s);
            let hl = half_leading(&m, self.line_height(s));
            let off = offset_of(self, Some(bi), &mut box_offset);
            if matches!(
                s.vertical_align,
                VerticalAlign::Keyword(VerticalAlignKeyword::Top | VerticalAlignKeyword::Bottom)
            ) {
                deferred.push((
                    matches!(
                        s.vertical_align,
                        VerticalAlign::Keyword(VerticalAlignKeyword::Top)
                    ),
                    m.ascent + m.descent + 2.0 * hl,
                ));
                continue;
            }
            top = top.min(off - (m.ascent + hl));
            bottom = bottom.max(off + m.descent + hl);
        }
        let mut atomic_offsets: Vec<(usize, f32)> = Vec::new();
        for &s in line {
            if let SegKind::Atomic(a) = ifc.segs[s].kind {
                let at = &ifc.atomics[a];
                let parent_style = at.parent.map_or(root, |p| &ifc.boxes[p].b.style);
                let st = &at.fragment.style;
                let base = offset_of(self, at.parent, &mut box_offset);
                let off = base
                    + self.baseline_shift(
                        &st.vertical_align,
                        parent_style,
                        st,
                        at.baseline,
                        at.height - at.baseline,
                    );
                atomic_offsets.push((a, off));
                if matches!(
                    st.vertical_align,
                    VerticalAlign::Keyword(
                        VerticalAlignKeyword::Top | VerticalAlignKeyword::Bottom
                    )
                ) {
                    // `top`/`bottom` se alinean con la line box una vez conocida su altura.
                    deferred.push((
                        matches!(
                            st.vertical_align,
                            VerticalAlign::Keyword(VerticalAlignKeyword::Top)
                        ),
                        at.height,
                    ));
                    continue;
                }
                top = top.min(off - at.baseline);
                bottom = bottom.max(off - at.baseline + at.height);
            }
        }
        for &(_, h) in &deferred {
            if bottom - top < h {
                bottom = top + h;
            }
        }
        let height = bottom - top;
        let baseline = y - top;

        // Fragmentos, recorriendo las hojas en orden visual. Cada caja inline genera un
        // fragmento por tramo visual contiguo de sus hojas.
        let mut line_frag = Fragment::new(
            FragmentKind::Line,
            Rect::new(x0, y, available, height),
            container.style.clone(),
        );
        line_frag.baseline = Some(baseline);
        struct Open {
            idx: usize,
            fragment: Fragment,
            empty: bool,
            lo: f32,
            hi: f32,
            margin_lo: f32,
            margin_hi: f32,
        }
        struct PendingText {
            item: usize,
            start: usize,
            end: usize,
            x: f32,
            width: f32,
            glyphs: Vec<Glyph>,
        }
        let chain_of = |owner: Option<usize>| {
            let mut out = Vec::new();
            let mut p = owner;
            while let Some(i) = p {
                out.push(i);
                p = ifc.boxes[i].parent;
            }
            out.reverse();
            out
        };
        let open_box = |this: &Self, idx: usize, cache: &mut Vec<Option<f32>>| -> Open {
            let bx = &ifc.boxes[idx];
            let s = &bx.b.style;
            let m = this.metrics(s);
            let border = border_edges(s);
            let padding = padding_edges(s, 0.0);
            let off = offset_of(this, Some(idx), cache);
            let mut f = Fragment::new(
                FragmentKind::Inline,
                Rect::new(
                    0.0,
                    baseline + off - m.ascent - padding.top - border.top,
                    0.0,
                    m.ascent + m.descent + padding.vertical() + border.vertical(),
                ),
                s.clone(),
            );
            f.node = bx.b.node;
            f.pseudo = bx.b.pseudo;
            f.border = border;
            f.padding = padding;
            f.baseline = Some(baseline + off);
            Open {
                idx,
                fragment: f,
                empty: true,
                lo: 0.0,
                hi: 0.0,
                margin_lo: 0.0,
                margin_hi: 0.0,
            }
        };
        fn push_child(stack: &mut [Open], line_frag: &mut Fragment, f: Fragment) {
            match stack.last_mut() {
                Some(o) => o.fragment.children.push(f),
                None => line_frag.children.push(f),
            }
        }
        fn close(stack: &mut Vec<Open>, line_frag: &mut Fragment) {
            let mut o = stack.pop().expect("caja abierta");
            o.fragment.rect.x = o.lo;
            o.fragment.rect.width = (o.hi - o.lo).max(0.0);
            o.fragment.margin.left = o.margin_lo;
            o.fragment.margin.right = o.margin_hi;
            push_child(stack, line_frag, o.fragment);
        }
        let flush = |this: &Self,
                     p: Option<PendingText>,
                     stack: &mut Vec<Open>,
                     line_frag: &mut Fragment,
                     cache: &mut Vec<Option<f32>>| {
            let Some(mut p) = p else { return };
            let text = &ifc.texts[p.item];
            let s = &text.b.style;
            let m = this.metrics(s);
            let off = offset_of(this, text.parent, cache);
            for g in &mut p.glyphs {
                g.cluster -= p.start;
            }
            let mut f = Fragment::new(
                FragmentKind::Text,
                Rect::new(
                    p.x,
                    baseline + off - m.ascent,
                    p.width,
                    m.ascent + m.descent,
                ),
                s.clone(),
            );
            f.node = text.b.node;
            f.pseudo = text.b.pseudo;
            f.text = Some((text.text[p.start..p.end].to_owned(), p.glyphs));
            f.baseline = Some(baseline + off);
            push_child(stack, line_frag, f);
        };
        let mut stack: Vec<Open> = Vec::new();
        let mut pending: Option<PendingText> = None;
        for &k in &order {
            let leaf = leaves[k];
            // ¿Se une al texto pendiente? (mismo texto y bytes contiguos)
            let merges = match (&pending, leaf.kind) {
                (
                    Some(p),
                    LeafKind::Text {
                        item, start, end, ..
                    },
                ) => p.item == item && (start == p.end || end == p.start),
                _ => false,
            };
            if !merges {
                flush(
                    self,
                    pending.take(),
                    &mut stack,
                    &mut line_frag,
                    &mut box_offset,
                );
            }
            // Cajas abiertas: el prefijo común con la cadena de la hoja.
            let chain = chain_of(leaf.owner);
            let common = stack
                .iter()
                .zip(&chain)
                .take_while(|(o, c)| o.idx == **c)
                .count();
            while stack.len() > common {
                close(&mut stack, &mut line_frag);
            }
            for &bi in &chain[common..] {
                let o = open_box(self, bi, &mut box_offset);
                stack.push(o);
            }
            // Extensión de las cajas abiertas; sus propios bordes excluyen el margen.
            for o in &mut stack {
                let own = match leaf.kind {
                    LeafKind::Edge { b, margin } if b == o.idx => margin,
                    _ => 0.0,
                };
                if o.empty {
                    o.empty = false;
                    o.lo = leaf.x + own;
                    o.margin_lo = own;
                }
                o.hi = leaf.x + leaf.width - own;
                o.margin_hi = own;
            }
            match leaf.kind {
                LeafKind::Text {
                    item,
                    start,
                    end,
                    space,
                    hyphen,
                } => {
                    let t = &ifc.texts[item];
                    let mode = t.b.style.text_justify;
                    let mut glyphs: Vec<Glyph> = Vec::new();
                    let n_chars = t.text[start..end].chars().count().max(1);
                    for (gi, g) in t.glyphs.iter().enumerate() {
                        if g.cluster < start || g.cluster >= end {
                            continue;
                        }
                        let mut g = *g;
                        if space {
                            g.advance = (leaf.width - extra * leaf.slots as f32) / n_chars as f32;
                        }
                        let last_of_cluster =
                            t.glyphs.get(gi + 1).is_none_or(|n| n.cluster != g.cluster);
                        if extra > 0.0
                            && last_of_cluster
                            && last_text_char != Some((item, g.cluster))
                            && t.text[g.cluster..]
                                .chars()
                                .next()
                                .is_some_and(|c| justification_slot(c, mode))
                        {
                            g.advance += extra;
                        }
                        glyphs.push(g);
                    }
                    if hyphen && let Some((hg, _)) = &t.hyphen {
                        // El guion va al final lógico: a la derecha en LTR, a la izquierda
                        // en RTL. Su clúster es el del U+00AD.
                        let shy = end - SHY.len_utf8();
                        let hg = hg.iter().map(|g| Glyph { cluster: shy, ..*g });
                        if leaf.level % 2 == 1 {
                            glyphs.splice(0..0, hg);
                        } else {
                            glyphs.extend(hg);
                        }
                    }
                    match &mut pending {
                        Some(p) if merges => {
                            p.start = p.start.min(start);
                            p.end = p.end.max(end);
                            p.width += leaf.width;
                            p.glyphs.extend(glyphs);
                        }
                        _ => {
                            pending = Some(PendingText {
                                item,
                                start,
                                end,
                                x: leaf.x,
                                width: leaf.width,
                                glyphs,
                            });
                        }
                    }
                }
                LeafKind::Atomic(a) => {
                    let off = atomic_offsets
                        .iter()
                        .find(|(i, _)| *i == a)
                        .map_or(0.0, |(_, o)| *o);
                    let at = &ifc.atomics[a];
                    let mut f = at.fragment.clone();
                    let margin_top = match f.style.vertical_align {
                        VerticalAlign::Keyword(VerticalAlignKeyword::Top) => y,
                        VerticalAlign::Keyword(VerticalAlignKeyword::Bottom) => {
                            y + height - at.height
                        }
                        _ => baseline + off - at.baseline,
                    };
                    f.translate(leaf.x, margin_top);
                    push_child(&mut stack, &mut line_frag, f);
                }
                LeafKind::Edge { .. } => {}
            }
        }
        flush(
            self,
            pending.take(),
            &mut stack,
            &mut line_frag,
            &mut box_offset,
        );
        while !stack.is_empty() {
            close(&mut stack, &mut line_frag);
        }
        // Las cajas abiertas al inicio de la línea sin hojas en ella (p. ej. una línea que solo
        // tiene un salto forzado) no generan fragmento.
        let _ = open_at_start;
        // Marcador exterior: a la izquierda de la primera línea del list-item.
        if let Some(marker) = container
            .children
            .iter()
            .find(|c| matches!(c.kind, BoxKind::Marker { outside: true, .. }))
            && first_line
        {
            let f = self.marker_fragment(marker, Some((x0, baseline)), x0, y);
            line_frag.children.insert(0, f);
        }
        (line_frag, height)
    }

    /// Anchos intrínsecos del contenido inline de `b` (CSS Sizing 3 §5).
    pub(crate) fn inline_intrinsic_widths(&mut self, b: &'a LayoutBox) -> (f32, f32) {
        #[derive(Default)]
        struct Acc {
            line: f32,
            max: f32,
            word: f32,
            min: f32,
            /// Espacios colapsables todavía no sumados: se quitan al principio y al final de
            /// la línea (fase 2 del procesamiento de espacios).
            pending: f32,
            /// E5: la parte de `pending` que no es oportunidad de corte (espacios colapsables
            /// de un texto que no se parte, `nowrap`): si sigue contenido, es de la palabra.
            word_pending: f32,
            has_content: bool,
            /// E9: lo último fue una atómica (hay oportunidad después, CSS Text 3 §5.1).
            after_atomic: bool,
            /// E9: lo último fue un carácter de pegado (GL salvo U+00A0, WJ, ZWJ, CM).
            glue: bool,
        }
        impl Acc {
            fn content(&mut self) {
                if self.has_content {
                    self.line += self.pending;
                    self.word += self.word_pending;
                }
                self.pending = 0.0;
                self.word_pending = 0.0;
                self.has_content = true;
            }
            fn end_line(&mut self) {
                self.max = self.max.max(self.line);
                self.line = 0.0;
                self.min = self.min.max(self.word);
                self.word = 0.0;
                self.pending = 0.0;
                self.word_pending = 0.0;
                self.has_content = false;
            }
        }
        fn walk<'b>(this: &mut Layout<'b>, children: &'b [LayoutBox], acc: &mut Acc) {
            for c in children {
                match &c.kind {
                    BoxKind::Text(t)
                    | BoxKind::Marker {
                        text: t,
                        outside: false,
                    } => {
                        let s = &c.style;
                        let text = transform(t, s.text_transform);
                        let glyphs = this.shape(&text, s);
                        let wraps = s.white_space.wraps();
                        // E9: las oportunidades dentro del texto, con UAX #14.
                        let chars: Vec<char> = text.chars().collect();
                        let breaks = if wraps {
                            linebreak::line_breaks_with(&chars, |_, ch| {
                                adapted_class(ch, s.word_break, s.line_break)
                            })
                        } else {
                            Vec::new()
                        };
                        for (k, (i, ch)) in text.char_indices().enumerate() {
                            let w = glyph_width(&glyphs, i, i + ch.len_utf8());
                            let class = linebreak::resolve_class(ch);
                            let glue = ch != '\u{A0}'
                                && matches!(
                                    class,
                                    LineBreak::GL | LineBreak::WJ | LineBreak::ZWJ | LineBreak::CM
                                );
                            if std::mem::take(&mut acc.after_atomic) && wraps && !glue {
                                acc.min = acc.min.max(acc.word);
                                acc.word = 0.0;
                            }
                            acc.glue = glue;
                            match ch {
                                '\n' => acc.end_line(),
                                ' ' | '\t' if s.white_space.collapses_spaces() => {
                                    acc.pending += w;
                                    // E5: sin `wraps` (`nowrap`) el espacio no es oportunidad de
                                    // corte (CSS Text 3 §3): el min-content es la línea entera.
                                    if wraps {
                                        acc.min = acc.min.max(acc.word);
                                        acc.word = 0.0;
                                    } else {
                                        acc.word_pending += w;
                                    }
                                }
                                ' ' | '\t' if wraps => {
                                    acc.content();
                                    acc.line += w;
                                    acc.min = acc.min.max(acc.word);
                                    acc.word = 0.0;
                                }
                                _ => {
                                    acc.content();
                                    acc.line += w;
                                    acc.word += w;
                                    // Al final del texto solo se sabe de ZW (LB8); lo que
                                    // sigue decide el resto.
                                    let anywhere = s.line_break == TextLineBreak::Anywhere
                                        && chars.get(k + 1).is_some_and(|&n| {
                                            !matches!(
                                                linebreak::resolve_class(n),
                                                LineBreak::CM | LineBreak::ZWJ
                                            )
                                        });
                                    let opportunity = wraps
                                        && if k + 1 < chars.len() {
                                            breaks[k + 1] != Break::Prohibited || anywhere
                                        } else {
                                            class == LineBreak::ZW
                                        }
                                        && !(ch == SHY && s.hyphens == Hyphens::None);
                                    if opportunity {
                                        acc.min = acc.min.max(acc.word);
                                        acc.word = 0.0;
                                    }
                                }
                            }
                        }
                    }
                    BoxKind::Inline => {
                        let s = &c.style;
                        let start = margin_edges(s, 0.0).left
                            + border_edges(s).left
                            + padding_edges(s, 0.0).left;
                        let end = margin_edges(s, 0.0).right
                            + border_edges(s).right
                            + padding_edges(s, 0.0).right;
                        if start != 0.0 {
                            acc.content();
                        }
                        acc.line += start;
                        acc.word += start;
                        walk(this, &c.children, acc);
                        if end != 0.0 {
                            acc.content();
                        }
                        acc.line += end;
                        acc.word += end;
                    }
                    BoxKind::LineBreak => acc.end_line(),
                    BoxKind::Marker { outside: true, .. } | BoxKind::BreakOpportunity => {}
                    _ if c.is_absolutely_positioned() => {}
                    _ => {
                        // Sin oportunidad de corte alrededor de un atómico: se suma a la
                        // palabra en curso.
                        let (amin, amax) = this.outer_intrinsic_widths(c);
                        if !c.is_float() {
                            // E9: oportunidad antes y después de la atómica (CSS Text 3 §5.1),
                            // salvo junto a un carácter de pegado.
                            let wraps = c.style.white_space.wraps();
                            if wraps && acc.has_content && !acc.glue {
                                acc.min = acc.min.max(acc.word);
                                acc.word = 0.0;
                            }
                            acc.content();
                            acc.after_atomic = wraps;
                            acc.glue = false;
                        }
                        acc.word += amin;
                        acc.line += amax;
                    }
                }
            }
        }
        // text-indent cuenta en la primera línea para max-content y en su primer segmento
        // irrompible para min-content (CSS Text 3 §8.1).
        let indent = b.style.text_indent.resolve(0.0);
        let mut acc = Acc {
            line: indent,
            word: indent,
            ..Acc::default()
        };
        walk(self, &b.children, &mut acc);
        acc.max = acc.max.max(acc.line);
        acc.min = acc.min.max(acc.word);
        (acc.min, acc.max)
    }
}

/// Baseline de la última line box entre los fragmentos (relativa a las coordenadas de éstos).
fn last_line_baseline(frags: &[Fragment]) -> Option<f32> {
    for f in frags.iter().rev() {
        if f.kind == FragmentKind::Line {
            return f.baseline;
        }
        if (f.kind == FragmentKind::Block || crate::grid::is_item_fragment(f)) // E4 grid
            && let Some(b) = last_line_baseline(&f.children)
        {
            return Some(b);
        }
    }
    None
}
