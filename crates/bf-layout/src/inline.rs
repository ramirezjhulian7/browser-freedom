//! Inline formatting context (CSS 2.2 §9.4.2 y §10.8, CSS Text 3): segmentación del contenido
//! inline, corte de líneas, fase 2 de white space, alineación horizontal (`text-align`,
//! `text-indent`) y vertical (`vertical-align`), cajas inline fragmentadas en varias líneas,
//! inline-level atómicas y floats dentro del contenido inline.

use bf_style::ComputedStyle;
use bf_style::properties::types::{
    Overflow, OverflowWrap, TextAlign, TextTransform, VerticalAlign, VerticalAlignKeyword,
    WordBreak,
};

use crate::block::{Cb, Layout, border_edges, margin_edges, padding_edges};
use crate::box_tree::{BoxKind, LayoutBox};
use crate::float::FloatContext;
use crate::font::{FontMetrics, Glyph};
use crate::fragment::{Fragment, FragmentKind, Rect};

/// Un texto del IFC con sus glifos.
struct TextItem<'a> {
    b: &'a LayoutBox,
    text: String,
    glyphs: Vec<Glyph>,
    /// Caja inline que lo contiene (`None` = la raíz).
    parent: Option<usize>,
}

/// Una caja inline del IFC.
struct InlineBox<'a> {
    b: &'a LayoutBox,
    parent: Option<usize>,
    /// Margen + borde + padding izquierdo / derecho.
    start: f32,
    end: f32,
}

/// Una inline-level atómica ya dimensionada (fragmento con su margin box en el origen).
struct Atomic {
    fragment: Fragment,
    /// Distancia del borde superior del margin box a la baseline.
    baseline: f32,
    width: f32,
    height: f32,
    parent: Option<usize>,
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
}

struct Ifc<'a> {
    texts: Vec<TextItem<'a>>,
    boxes: Vec<InlineBox<'a>>,
    atomics: Vec<Atomic>,
    oof: Vec<&'a LayoutBox>,
    segs: Vec<Seg>,
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

/// CSS Text 3 §5.1: una oportunidad de corte en el límite entre contenido de cajas distintas
/// la controla el `white-space` de su ancestro común más cercano. Un espacio al final de un
/// `nowrap` seguido de texto de otra caja que sí parte ofrece corte.
fn boundary_opportunities(ifc: &mut Ifc<'_>, root: &ComputedStyle) {
    let owner = |ifc: &Ifc<'_>, k: SegKind| match k {
        SegKind::Word { item, .. } | SegKind::Space { item, .. } => Some(ifc.texts[item].parent),
        SegKind::Atomic(a) => Some(ifc.atomics[a].parent),
        _ => None,
    };
    let chain = |ifc: &Ifc<'_>, mut p: Option<usize>| {
        let mut out = vec![p];
        while let Some(i) = p {
            p = ifc.boxes[i].parent;
            out.push(p);
        }
        out
    };
    for i in 0..ifc.segs.len() {
        let SegKind::Space { item, .. } = ifc.segs[i].kind else {
            continue;
        };
        if ifc.segs[i].break_after || ifc.texts[item].b.style.white_space.wraps() {
            continue;
        }
        let Some(next) = ifc.segs[i + 1..]
            .iter()
            .find_map(|s| owner(ifc, s.kind).map(|o| (o, s.kind)))
        else {
            continue;
        };
        let here = ifc.texts[item].parent;
        if next.0 == here {
            continue;
        }
        let a = chain(ifc, here);
        let b = chain(ifc, next.0);
        let common = a.iter().find(|p| b.contains(p)).copied().flatten();
        let style = common.map_or(root, |c| &*ifc.boxes[c].b.style);
        if style.white_space.wraps() {
            ifc.segs[i].break_after = true;
        }
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

impl<'a> Layout<'a> {
    fn text_item(&self, ifc: &mut Ifc<'a>, b: &'a LayoutBox, raw: &str, parent: Option<usize>) {
        let style = &b.style;
        let text = transform(raw, style.text_transform);
        let glyphs = self.shape(&text, style);
        let item = ifc.texts.len();
        let ws = style.white_space;
        let wraps = ws.wraps();
        let break_all = style.word_break == WordBreak::BreakAll;
        let mut word_start: Option<usize> = None;
        let push_word = |ifc: &mut Ifc<'a>, start: usize, end: usize, glyphs: &[Glyph]| {
            ifc.segs.push(Seg {
                kind: SegKind::Word { item, start, end },
                width: glyph_width(glyphs, start, end),
                break_after: false,
            });
        };
        for (i, c) in text.char_indices() {
            let end = i + c.len_utf8();
            match c {
                '\n' => {
                    if let Some(s) = word_start.take() {
                        push_word(ifc, s, i, &glyphs);
                    }
                    ifc.segs.push(Seg {
                        kind: SegKind::Break,
                        width: 0.0,
                        break_after: true,
                    });
                }
                ' ' | '\t' => {
                    if let Some(s) = word_start.take() {
                        push_word(ifc, s, i, &glyphs);
                    }
                    let mut width = glyph_width(&glyphs, i, end);
                    if c == '\t' {
                        // Aproximación: un tab vale 8 espacios (sin tab stops todavía).
                        width *= 8.0;
                    }
                    ifc.segs.push(Seg {
                        kind: SegKind::Space {
                            item,
                            start: i,
                            end,
                            collapsible: ws.collapses_spaces(),
                        },
                        width,
                        break_after: wraps,
                    });
                }
                _ if break_all => {
                    if let Some(s) = word_start.take() {
                        push_word(ifc, s, i, &glyphs);
                    }
                    push_word(ifc, i, end, &glyphs);
                    if let Some(last) = ifc.segs.last_mut() {
                        last.break_after = wraps;
                    }
                }
                // U+200B ZERO WIDTH SPACE: oportunidad de corte sin ancho.
                '\u{200B}' if wraps => {
                    let s = word_start.take().unwrap_or(i);
                    push_word(ifc, s, end, &glyphs);
                    if let Some(last) = ifc.segs.last_mut() {
                        last.break_after = true;
                    }
                }
                _ => {
                    word_start.get_or_insert(i);
                }
            }
        }
        if let Some(s) = word_start {
            push_word(ifc, s, text.len(), &glyphs);
        }
        ifc.texts.push(TextItem {
            b,
            text,
            glyphs,
            parent,
        });
    }

    fn collect(
        &mut self,
        ifc: &mut Ifc<'a>,
        children: &'a [LayoutBox],
        parent: Option<usize>,
        cb: Cb,
        wraps: bool,
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
                    ifc.segs.push(Seg {
                        kind: if c.is_float() {
                            SegKind::Float(idx)
                        } else {
                            SegKind::Abs(idx)
                        },
                        width: 0.0,
                        break_after: false,
                    });
                }
                BoxKind::Inline => {
                    let s = &c.style;
                    let m = margin_edges(s, cb.width);
                    let b = border_edges(s);
                    let p = padding_edges(s, cb.width);
                    let idx = ifc.boxes.len();
                    ifc.boxes.push(InlineBox {
                        b: c,
                        parent,
                        start: m.left + b.left + p.left,
                        end: m.right + b.right + p.right,
                    });
                    ifc.segs.push(Seg {
                        kind: SegKind::Start(idx),
                        width: ifc.boxes[idx].start,
                        break_after: false,
                    });
                    self.collect(ifc, &c.children, Some(idx), cb, wraps);
                    ifc.segs.push(Seg {
                        kind: SegKind::End(idx),
                        width: ifc.boxes[idx].end,
                        break_after: false,
                    });
                }
                BoxKind::LineBreak => ifc.segs.push(Seg {
                    kind: SegKind::Break,
                    width: 0.0,
                    break_after: true,
                }),
                BoxKind::BreakOpportunity => ifc.segs.push(Seg {
                    kind: SegKind::Opportunity,
                    width: 0.0,
                    break_after: true,
                }),
                BoxKind::InlineBlock
                | BoxKind::Replaced { .. }
                | BoxKind::Block
                | BoxKind::AnonymousBlock => {
                    let atomic = self.atomic(c, cb, parent);
                    let idx = ifc.atomics.len();
                    // Hay oportunidades de corte antes y después de una atómica.
                    if let Some(last) = ifc.segs.last_mut() {
                        last.break_after |= wraps;
                    }
                    ifc.segs.push(Seg {
                        kind: SegKind::Atomic(idx),
                        width: atomic.width,
                        break_after: wraps,
                    });
                    ifc.atomics.push(atomic);
                }
            }
        }
    }

    /// Dimensiona una inline-level atómica con su margin box en el origen.
    fn atomic(&mut self, b: &'a LayoutBox, cb: Cb, parent: Option<usize>) -> Atomic {
        let margins = margin_edges(&b.style, cb.width);
        let content_w = self.fit_width(b, cb, margins.horizontal());
        let mut scratch = FloatContext::default();
        let (fragment, _) = self.box_contents(
            b,
            cb,
            margins.left,
            margins.top,
            content_w,
            margins,
            &mut scratch,
            false,
        );
        let outer = fragment.margin_rect();
        let visible =
            b.style.overflow_x == Overflow::Visible && b.style.overflow_y == Overflow::Visible;
        // La baseline de un inline-block es la de su última line box; si no tiene o su
        // overflow no es visible, el borde inferior del margin box (§10.8.1).
        let baseline = match (&b.kind, visible) {
            // Controles de una línea: la baseline de su texto (aunque recorten su contenido).
            _ if b
                .control
                .as_ref()
                .is_some_and(crate::controls::Control::single_line) =>
            {
                fragment.baseline.map_or(outer.height, |y| y - outer.y)
            }
            // El resto de los controles (casillas, barras, list box, textarea): el borde inferior
            // del margin box, como un contenedor de scroll.
            _ if b.control.is_some() => outer.height,
            // inline-flex: la baseline del contenedor es la de su primer ítem (Flexbox §8.5).
            (BoxKind::InlineBlock, _) if crate::flex::is_flex_container(b) => {
                crate::flex::container_baseline(&fragment).map_or(outer.height, |y| y - outer.y)
            }
            (BoxKind::InlineBlock, true) => {
                last_line_baseline(&fragment.children).unwrap_or(outer.height)
            }
            _ => outer.height,
        };
        Atomic {
            fragment,
            baseline,
            width: outer.width,
            height: outer.height,
            parent,
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
            segs: Vec::new(),
        };
        let wraps = b.style.white_space.wraps();
        self.collect(&mut ifc, &b.children, None, cb, wraps);
        boundary_opportunities(&mut ifc, &b.style);
        let root = &b.style;
        let root_lh = self.line_height(root);
        let indent = root.text_indent.resolve(cb.width);
        let mut out: Vec<Fragment> = Vec::new();
        let mut y = cb.y;
        let mut i = 0;
        let mut open: Vec<usize> = Vec::new();
        let mut first = true;
        while i < ifc.segs.len() {
            let line_start = i;
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
                            abs_here.push((idx, l + width, has_content));
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
                    if has_content
                        && !is_space
                        && width + seg.width > available + 0.01
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
                            ifc.segs[i] = Seg {
                                kind: SegKind::Word {
                                    item,
                                    start,
                                    end: split,
                                },
                                width: first_w,
                                break_after: true,
                            };
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
                    if seg.break_after && has_content {
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
                    self.place_line_abs(&ifc, &abs_here, cb, y, 0.0);
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
                self.offset_relative_inlines(&mut fragment, cb);
                self.place_line_abs(&ifc, &abs_here, cb, y, height);
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
    ) {
        for &(idx, x, had_content) in abs {
            let b = ifc.oof[idx];
            if b.style.original_display.is_inline_level() {
                self.add_abs(b, x, y);
            } else {
                self.add_abs(b, cb.x, if had_content { y + height } else { y });
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

    /// Construye la line box: posiciones, alineación y fragmentos.
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
        let root = &container.style;
        // Posiciones horizontales.
        let mut xs = Vec::with_capacity(line.len());
        let mut x = x0 + indent;
        for &s in line {
            xs.push(x);
            x += ifc.segs[s].width;
        }
        let used = x - x0;
        let free = (available - used).max(0.0);
        let spaces = line
            .iter()
            .filter(|&&s| matches!(ifc.segs[s].kind, SegKind::Space { .. }))
            .count();
        let (shift, extra_per_space) = match root.text_align {
            TextAlign::Right | TextAlign::End => (free, 0.0),
            TextAlign::Center => (free / 2.0, 0.0),
            TextAlign::Justify if !last && spaces > 0 => (0.0, free / spaces as f32),
            _ => (0.0, 0.0),
        };
        let mut extra = 0.0;
        for (k, &s) in line.iter().enumerate() {
            xs[k] += shift + extra;
            if matches!(ifc.segs[s].kind, SegKind::Space { .. }) {
                extra += extra_per_space;
            }
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

        // Fragmentos.
        let mut line_frag = Fragment::new(
            FragmentKind::Line,
            Rect::new(x0, y, available, height),
            container.style.clone(),
        );
        line_frag.baseline = Some(baseline);
        struct Open {
            idx: usize,
            fragment: Fragment,
            start_x: f32,
            has_start: bool,
        }
        let mut stack: Vec<Open> = Vec::new();
        let make_box = |this: &Self,
                        idx: usize,
                        start_x: f32,
                        has_start: bool,
                        cache: &mut Vec<Option<f32>>|
         -> Open {
            let bx = &ifc.boxes[idx];
            let s = &bx.b.style;
            let m = this.metrics(s);
            let border = border_edges(s);
            let padding = padding_edges(s, 0.0);
            let off = offset_of(this, Some(idx), cache);
            let mut f = Fragment::new(
                FragmentKind::Inline,
                Rect::new(
                    start_x,
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
                start_x,
                has_start,
            }
        };
        let first_x = xs.first().copied().unwrap_or(x0 + indent + shift);
        for &bi in open_at_start {
            let o = make_box(self, bi, first_x, false, &mut box_offset);
            stack.push(o);
        }
        let push_child =
            |stack: &mut Vec<Open>, line_frag: &mut Fragment, f: Fragment| match stack.last_mut() {
                Some(o) => o.fragment.children.push(f),
                None => line_frag.children.push(f),
            };
        let close = |stack: &mut Vec<Open>, line_frag: &mut Fragment, end_x: f32, has_end: bool| {
            let mut o = stack.pop().expect("caja abierta");
            let bx = &ifc.boxes[o.idx];
            let m = margin_edges(&bx.b.style, 0.0);
            let left = if o.has_start { m.left } else { 0.0 };
            o.fragment.rect.x = o.start_x + left;
            let right_edge = if has_end { end_x - m.right } else { end_x };
            o.fragment.rect.width = (right_edge - o.fragment.rect.x).max(0.0);
            if o.has_start {
                o.fragment.margin.left = m.left;
            }
            if has_end {
                o.fragment.margin.right = m.right;
            }
            match stack.last_mut() {
                Some(p) => p.fragment.children.push(o.fragment),
                None => line_frag.children.push(o.fragment),
            }
        };
        let mut k = 0;
        let mut atomic_iter = atomic_offsets.iter();
        let mut end_x = first_x;
        while k < line.len() {
            let seg = ifc.segs[line[k]];
            let sx = xs[k];
            match seg.kind {
                SegKind::Start(bi) => {
                    let o = make_box(self, bi, sx, true, &mut box_offset);
                    stack.push(o);
                    end_x = sx + seg.width;
                    k += 1;
                }
                SegKind::End(_) => {
                    let e = sx + seg.width;
                    close(&mut stack, &mut line_frag, e, true);
                    end_x = e;
                    k += 1;
                }
                SegKind::Word { item, .. } | SegKind::Space { item, .. } => {
                    // Un fragmento de texto por tramo contiguo del mismo item.
                    let start_x = sx;
                    let text = &ifc.texts[item];
                    let mut s_byte = usize::MAX;
                    let mut e_byte = 0;
                    let mut width = 0.0;
                    let mut glyphs = Vec::new();
                    while k < line.len() {
                        let sg = ifc.segs[line[k]];
                        let (it, a, z, is_space) = match sg.kind {
                            SegKind::Word { item, start, end } => (item, start, end, false),
                            SegKind::Space {
                                item, start, end, ..
                            } => (item, start, end, true),
                            _ => break,
                        };
                        if it != item {
                            break;
                        }
                        s_byte = s_byte.min(a);
                        e_byte = e_byte.max(z);
                        for g in text
                            .glyphs
                            .iter()
                            .filter(|g| g.cluster >= a && g.cluster < z)
                        {
                            let mut g = *g;
                            if is_space {
                                g.advance = sg.width / ((z - a).max(1) as f32) + extra_per_space;
                            }
                            glyphs.push(g);
                        }
                        width += sg.width + if is_space { extra_per_space } else { 0.0 };
                        k += 1;
                    }
                    let s = &text.b.style;
                    let m = self.metrics(s);
                    let off = offset_of(self, text.parent, &mut box_offset);
                    let substring = text.text[s_byte..e_byte].to_owned();
                    for g in &mut glyphs {
                        g.cluster -= s_byte;
                    }
                    let mut f = Fragment::new(
                        FragmentKind::Text,
                        Rect::new(
                            start_x,
                            baseline + off - m.ascent,
                            width,
                            m.ascent + m.descent,
                        ),
                        s.clone(),
                    );
                    f.node = text.b.node;
                    f.pseudo = text.b.pseudo;
                    f.text = Some((substring, glyphs));
                    f.baseline = Some(baseline + off);
                    end_x = start_x + width;
                    push_child(&mut stack, &mut line_frag, f);
                }
                SegKind::Atomic(a) => {
                    let (_, off) = *atomic_iter.next().expect("offset de atómica");
                    let at = &ifc.atomics[a];
                    let mut f = at.fragment.clone();
                    let st = &f.style;
                    let margin_top = match st.vertical_align {
                        VerticalAlign::Keyword(VerticalAlignKeyword::Top) => y,
                        VerticalAlign::Keyword(VerticalAlignKeyword::Bottom) => {
                            y + height - at.height
                        }
                        _ => baseline + off - at.baseline,
                    };
                    f.translate(sx, margin_top);
                    end_x = sx + at.width;
                    push_child(&mut stack, &mut line_frag, f);
                    k += 1;
                }
                _ => k += 1,
            }
        }
        while !stack.is_empty() {
            close(&mut stack, &mut line_frag, end_x, false);
        }
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
            has_content: bool,
        }
        impl Acc {
            fn content(&mut self) {
                if self.has_content {
                    self.line += self.pending;
                }
                self.pending = 0.0;
                self.has_content = true;
            }
            fn end_line(&mut self) {
                self.max = self.max.max(self.line);
                self.line = 0.0;
                self.min = self.min.max(self.word);
                self.word = 0.0;
                self.pending = 0.0;
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
                        for (i, ch) in text.char_indices() {
                            let w = glyph_width(&glyphs, i, i + ch.len_utf8());
                            match ch {
                                '\n' => acc.end_line(),
                                ' ' | '\t' if s.white_space.collapses_spaces() => {
                                    acc.pending += w;
                                    acc.min = acc.min.max(acc.word);
                                    acc.word = 0.0;
                                }
                                ' ' | '\t' | '\u{200B}' if wraps => {
                                    acc.content();
                                    acc.line += w;
                                    acc.min = acc.min.max(acc.word);
                                    acc.word = 0.0;
                                }
                                _ => {
                                    acc.content();
                                    acc.line += w;
                                    acc.word += w;
                                    if s.word_break == WordBreak::BreakAll && wraps {
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
                            acc.content();
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
        if f.kind == FragmentKind::Block
            && let Some(b) = last_line_baseline(&f.children)
        {
            return Some(b);
        }
    }
    None
}
