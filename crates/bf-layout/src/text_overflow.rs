//! E12: `text-overflow` (CSS Overflow 3 §5, CSS Overflow 4 §5.1) sobre las line boxes de un
//! block container ya maquetado.
//!
//! Solo actúa si el block container recorta en el eje inline (`overflow` usado distinto de
//! `visible`). En cada línea cuyo contenido inline desborda el borde del final de la line box
//! se ocultan caracteres (clústeres) y cajas atómicas del final hasta que cabe la elipsis, que
//! se coloca justo tras lo que queda con el estilo del block container (§5: "the ellipsis is
//! styled and baseline-aligned according to the block"). El primer carácter o caja atómica de
//! la línea se recorta en lugar de ocultarse. Las cajas inline cortadas terminan donde termina
//! su contenido visible.
//!
//! Limitaciones: sin bidi (E9), solo el extremo del final en `ltr`; en `rtl` el contenido no
//! se ordena de derecha a izquierda y la elipsis iría a la izquierda, así que no se aplica. El
//! extremo line-left de la forma de dos valores (contenido que desborda por el inicio) tampoco.
//! El área de desborde desplazable se mide sobre el contenido ya recortado.

use std::sync::Arc;

use bf_style::ComputedStyle;
use bf_style::properties::types::{Direction, Overflow};

use crate::block::Layout;
use crate::clamp::{content_end, last_leaf};
use crate::fragment::{Fragment, FragmentKind, Rect};

/// ¿Quedan en la línea al menos dos unidades (clústeres o cajas atómicas)? La primera no se
/// oculta nunca.
fn more_than_one_unit(line: &Fragment) -> bool {
    fn count(f: &Fragment, n: &mut usize) {
        for c in &f.children {
            if *n > 1 {
                return;
            }
            match (&c.kind, &c.text) {
                (FragmentKind::Text, Some((_, glyphs))) => {
                    let mut clusters = glyphs.iter().map(|g| g.cluster).collect::<Vec<_>>();
                    clusters.dedup();
                    *n += clusters.len();
                }
                (FragmentKind::InlineBlock | FragmentKind::Replaced, _) => *n += 1,
                (FragmentKind::Inline, _) => count(c, n),
                _ => {}
            }
        }
    }
    let mut n = 0;
    count(line, &mut n);
    n > 1
}

/// Oculta el último clúster (o la última caja atómica) de la línea y acorta las cajas inline
/// que lo contenían hasta su contenido restante. `false` si ya no queda nada.
fn hide_last(line: &mut Fragment) -> bool {
    let mut path = Vec::new();
    if !last_leaf(line, &mut path) {
        return false;
    }
    hide_in(line, &path);
    true
}

fn hide_in(parent: &mut Fragment, path: &[usize]) {
    let i = path[0];
    if path.len() > 1 {
        let child = &mut parent.children[i];
        hide_in(child, &path[1..]);
        // La caja inline cortada termina en su contenido visible (sin su lado del final).
        if child.kind == FragmentKind::Inline {
            let start = child.rect.x + child.border.left + child.padding.left;
            let end = child
                .children
                .iter()
                .map(|c| c.rect.right())
                .fold(start, f32::max);
            child.rect.width = (end - child.rect.x).max(0.0);
            child.padding.right = 0.0;
            child.border.right = 0.0;
            if child.children.is_empty() && child.rect.width <= 0.0 {
                parent.children.remove(i);
            }
        }
        return;
    }
    let leaf = &mut parent.children[i];
    let emptied = match (&leaf.kind, &mut leaf.text) {
        (FragmentKind::Text, Some((text, glyphs))) => {
            let cluster = glyphs.last().map_or(0, |g| g.cluster);
            let mut width = 0.0;
            while glyphs.last().is_some_and(|g| g.cluster == cluster) {
                width += glyphs.pop().map_or(0.0, |g| g.advance);
            }
            text.truncate(cluster);
            leaf.rect.width = (leaf.rect.width - width).max(0.0);
            glyphs.is_empty()
        }
        _ => true,
    };
    if emptied {
        parent.children.remove(i);
    }
}

/// ¿Carácter de anchura East Asian F, W o H (UAX #11), aproximado por bloques? Los emoji no
/// cuentan.
fn east_asian_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7FF
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFFDC
        | 0xFFE0..=0xFFE6
        | 0x20000..=0x3FFFD)
}

fn hangul(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F | 0xAC00..=0xD7FF | 0xFFA0..=0xFFDC)
}

/// El texto de la elipsis tras el procesado de espacios de `white-space: normal` (CSS Text 3
/// §4.1.1 y §4.1.3): cada tramo de espacios y saltos de segmento queda en un espacio, salvo
/// un tramo de solo saltos entre dos caracteres anchos de Asia oriental que no sean hangul,
/// que se quita. U+2028/U+2029 cuentan como saltos.
fn collapse(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let is_break = |c: char| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}');
    let is_space = |c: char| matches!(c, ' ' | '\t' | '\x0C') || is_break(c);
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if !is_space(chars[i]) {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && is_space(chars[i]) {
            i += 1;
        }
        let only_breaks = chars[start..i].iter().all(|&c| is_break(c));
        let before = start.checked_sub(1).map(|j| chars[j]);
        let after = chars.get(i).copied();
        let removed = only_breaks
            && before.zip(after).is_some_and(|(a, b)| {
                east_asian_wide(a) && east_asian_wide(b) && !hangul(a) && !hangul(b)
            });
        if !removed {
            out.push(' ');
        }
    }
    out
}

/// ¿Aplica `text-overflow` a las líneas de un block container con este estilo?
fn applies(style: &ComputedStyle) -> bool {
    !style.text_overflow.is_clip()
        && crate::scroll::used_overflow(style).0 != Overflow::Visible
        && style.direction == Direction::Ltr
}

impl Layout<'_> {
    /// Aplica el `text-overflow` de `style` a las line boxes de `children` (y a las de sus
    /// bloques anónimos, que son del mismo block container).
    pub(crate) fn text_overflow(&self, style: &ComputedStyle, children: &mut [Fragment]) {
        if !applies(style) {
            return;
        }
        let Some(text) = style.text_overflow.sides(false).1.text() else {
            return;
        };
        self.ellipsize_lines(children, &collapse(text));
    }

    fn ellipsize_lines(&self, children: &mut [Fragment], text: &str) {
        for c in children {
            match c.kind {
                FragmentKind::Line => self.ellipsize(c, text),
                FragmentKind::Block if c.anonymous && c.node.is_none() => {
                    self.ellipsize_lines(&mut c.children, text)
                }
                _ => {}
            }
        }
    }

    /// Si el contenido de la línea desborda su final, oculta lo necesario y pone `text`.
    fn ellipsize(&self, line: &mut Fragment, text: &str) {
        let limit = line.rect.right();
        if content_end(line).is_none_or(|end| end <= limit + 0.01) {
            return;
        }
        let style: Arc<ComputedStyle> = line.style.clone();
        let glyphs = self.shape(text, &style);
        let width: f32 = glyphs.iter().map(|g| g.advance).sum();
        while content_end(line).is_some_and(|end| end + width > limit + 0.01)
            && more_than_one_unit(line)
        {
            if !hide_last(line) {
                break;
            }
        }
        let m = self.metrics(&style);
        let x = content_end(line).unwrap_or(line.rect.x);
        let baseline = line.baseline.unwrap_or(line.rect.y + m.ascent);
        let mut f = Fragment::new(
            FragmentKind::Text,
            Rect::new(x, baseline - m.ascent, width, m.ascent + m.descent),
            style,
        );
        f.anonymous = true;
        f.text = Some((text.to_owned(), glyphs));
        f.baseline = Some(baseline);
        line.children.push(f);
    }
}

#[cfg(test)]
mod tests {
    use super::collapse;

    #[test]
    fn ellipsis_string_white_space() {
        assert_eq!(collapse("Hi\n\n\nHi"), "Hi Hi");
        assert_eq!(collapse("Hi\u{2028}Hi"), "Hi Hi");
        assert_eq!(collapse("안녕\n你好"), "안녕 你好");
        assert_eq!(collapse("你好\n你好"), "你好你好");
        assert_eq!(collapse("こん\nにちは"), "こんにちは");
        assert_eq!(collapse("ＦＵ\nＬＬ"), "ＦＵＬＬ");
        assert_eq!(collapse("你好\n🟢"), "你好 🟢");
        assert_eq!(collapse("a \t b"), "a b");
    }
}
