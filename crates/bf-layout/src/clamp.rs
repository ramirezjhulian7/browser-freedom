//! E12: `line-clamp` (CSS Overflow 4 §5) sobre los fragmentos de un block container ya
//! maquetado.
//!
//! El *clamp point* (§5.2) va tras la línea N (contando las de los descendientes de bloque en
//! flujo que no establecen un formatting context independiente) o, con `auto`, tras la última
//! línea con la que el contenido cabe en la altura (o `max-height`) de la caja. Lo que sigue
//! se descarta (`continue: collapse`: no se pinta ni cuenta para la altura automática, §5.3),
//! las cajas que contienen el punto terminan en él (con su padding, borde y margen del final)
//! y la última línea recibe el `block-ellipsis` (§5.4), recortando su contenido si no cabe.

use std::sync::Arc;

use bf_style::ComputedStyle;
use bf_style::properties::clamp::{BlockEllipsis, MaxLines};
use bf_style::properties::types::{Display, Float, Overflow, Position, Size};

use crate::block::Layout;
use crate::fragment::{Fragment, FragmentKind, Rect};

/// El `line-clamp` efectivo de una caja: el propio o el heredado de WebKit (`display:
/// -webkit-box` + `-webkit-box-orient: vertical` + `-webkit-line-clamp`, §5.5).
pub(crate) fn effective(style: &ComputedStyle) -> Option<(MaxLines, BlockEllipsis)> {
    if style.line_clamp.lines != MaxLines::None {
        return Some((style.line_clamp.lines, style.line_clamp.ellipsis.clone()));
    }
    let n = style.webkit_line_clamp.0?;
    (style.webkit_box_orient.is_vertical()
        && matches!(style.display, Display::FlowRoot | Display::InlineBlock))
    .then_some((MaxLines::Count(n), BlockEllipsis::Auto))
}

fn in_flow(f: &Fragment) -> bool {
    f.style.float == Float::None
        && !matches!(f.style.position, Position::Absolute | Position::Fixed)
}

/// ¿Un fragmento de bloque establece un formatting context independiente? Sus líneas no
/// cuentan para el clamp del contenedor.
fn independent(f: &Fragment) -> bool {
    let s = &f.style;
    !matches!(f.kind, FragmentKind::Block)
        || !matches!(s.display, Display::Block | Display::ListItem)
        || s.overflow_x != Overflow::Visible
        || s.overflow_y != Overflow::Visible
        || effective(s).is_some()
}

/// Los posibles clamp points: el camino de índices hasta cada line box.
fn clamp_points(children: &[Fragment], prefix: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
    for (i, c) in children.iter().enumerate() {
        match c.kind {
            FragmentKind::Line => {
                prefix.push(i);
                out.push(prefix.clone());
                prefix.pop();
            }
            FragmentKind::Block if in_flow(c) && !independent(c) => {
                prefix.push(i);
                clamp_points(&c.children, prefix, out);
                prefix.pop();
            }
            _ => {}
        }
    }
}

/// Dónde termina el contenido (margin box del final) si se corta en `path`.
fn clamped_bottom(children: &[Fragment], path: &[usize]) -> f32 {
    let f = &children[path[0]];
    if path.len() == 1 {
        return f.rect.bottom();
    }
    let inner = clamped_bottom(&f.children, &path[1..]);
    let end = inner + f.padding.bottom + f.border.bottom;
    let bottom = if f.style.height == Size::Auto {
        end
    } else {
        f.rect.bottom()
    };
    bottom + f.margin.bottom
}

/// ¿Hay algo que mostrar en `f`?
fn has_content(f: &Fragment) -> bool {
    f.rect.height > 0.0
        || f.rect.width > 0.0 && f.kind != FragmentKind::Block
        || f.children.iter().any(has_content)
}

/// Descarta lo que sigue a `path` y ajusta la altura de las cajas que lo contienen. Devuelve
/// dónde termina el contenido y si se descartó algo visible.
fn truncate(children: &mut Vec<Fragment>, path: &[usize]) -> (f32, bool) {
    let i = path[0];
    let removed = children.drain(i + 1..).any(|f| has_content(&f));
    let f = &mut children[i];
    if path.len() == 1 {
        return (f.rect.bottom(), removed);
    }
    let (inner, more) = truncate(&mut f.children, &path[1..]);
    if f.style.height == Size::Auto {
        let end = inner + f.padding.bottom + f.border.bottom;
        f.rect.height = (end - f.rect.y).max(0.0);
        if f.clip.is_some() {
            f.clip = Some(f.padding_rect());
        }
    }
    (f.rect.bottom() + f.margin.bottom, removed || more)
}

/// La línea de `path`.
fn line_mut<'f>(children: &'f mut [Fragment], path: &[usize]) -> &'f mut Fragment {
    let f = &mut children[path[0]];
    if path.len() == 1 {
        f
    } else {
        line_mut(&mut f.children, &path[1..])
    }
}

/// El camino hasta la última hoja de contenido de una línea (texto o caja atómica).
fn last_leaf(f: &Fragment, path: &mut Vec<usize>) -> bool {
    for (i, c) in f.children.iter().enumerate().rev() {
        match c.kind {
            FragmentKind::Text | FragmentKind::InlineBlock | FragmentKind::Replaced => {
                path.push(i);
                return true;
            }
            FragmentKind::Inline => {
                path.push(i);
                if last_leaf(c, path) {
                    return true;
                }
                path.pop();
            }
            _ => {}
        }
    }
    false
}

/// El extremo derecho del contenido de la línea: la última hoja o, si las cajas inline que la
/// contienen terminan en esta línea, su borde del final (con su padding y borde).
fn content_end(line: &Fragment) -> Option<f32> {
    let mut path = Vec::new();
    if !last_leaf(line, &mut path) {
        return None;
    }
    let mut f = line;
    let mut end = f32::NEG_INFINITY;
    for &i in &path {
        f = &f.children[i];
        end = end.max(f.rect.right());
    }
    Some(end)
}

/// ¿La línea termina en una oportunidad de corte suave (tras un espacio en texto que se
/// ajusta)? La elipsis va tras la última que la deja caber (§5.4).
fn ends_at_wrap(line: &Fragment) -> bool {
    let mut path = Vec::new();
    if !last_leaf(line, &mut path) {
        return true;
    }
    let mut f = line;
    for &i in &path {
        f = &f.children[i];
    }
    f.kind == FragmentKind::Text
        && f.style.white_space.wraps()
        && f.text
            .as_ref()
            .is_some_and(|(t, _)| t.ends_with([' ', '\t', '\u{3000}']))
}

/// Quita el último clúster (o la última caja atómica) de la línea. `false` si ya no queda.
fn pop_last(line: &mut Fragment) -> bool {
    let mut path = Vec::new();
    if !last_leaf(line, &mut path) {
        return false;
    }
    let (last, parents) = path.split_last().expect("camino no vacío");
    let mut parent = &mut *line;
    for &i in parents {
        parent = &mut parent.children[i];
    }
    let leaf = &mut parent.children[*last];
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
        parent.children.remove(*last);
    }
    true
}

impl Layout<'_> {
    /// Aplica el `line-clamp` de `style` a los hijos de su caja (con el content box desde
    /// `content_top`, altura especificada `spec_h` y máxima `max_h`). Devuelve la altura
    /// automática nueva del contenido si se cortó.
    pub(crate) fn line_clamp(
        &self,
        style: &Arc<ComputedStyle>,
        children: &mut Vec<Fragment>,
        content_top: f32,
        auto_h: f32,
        spec_h: Option<f32>,
        max_h: Option<f32>,
    ) -> Option<f32> {
        let (lines, ellipsis) = effective(style)?;
        let mut points = Vec::new();
        clamp_points(children, &mut Vec::new(), &mut points);
        let point = match lines {
            MaxLines::None => return None,
            MaxLines::Count(n) => {
                // Sin más líneas que el máximo solo se corta si después hay otro contenido.
                points.get(n as usize - 1).cloned()?
            }
            MaxLines::Auto => {
                let room = spec_h.or(max_h)?;
                if auto_h <= room + 0.01 {
                    return None;
                }
                let limit = content_top + room + 0.01;
                match points
                    .iter()
                    .rev()
                    .find(|p| clamped_bottom(children, p) <= limit)
                {
                    Some(p) => p.clone(),
                    None => {
                        // Ni la primera línea cabe: no queda contenido en flujo.
                        children.retain(|c| !in_flow(c));
                        return Some(0.0);
                    }
                }
            }
        };
        let (bottom, removed) = truncate(children, &point);
        if !removed {
            return None;
        }
        if let Some(text) = ellipsis.text() {
            // Con el estilo de la root inline box de la línea: el block container que la
            // contiene (no necesariamente el del clamp).
            let line = line_mut(children, &point);
            let root_inline = line.style.clone();
            self.block_ellipsis(line, text, &root_inline);
        }
        Some((bottom - content_top).max(0.0))
    }

    /// Pone `text` al final de la línea con el estilo `style` de su block container (§5.4: va
    /// en una caja inline anónima hija suya), quitando contenido del final hasta que quepa.
    fn block_ellipsis(&self, line: &mut Fragment, text: &str, style: &Arc<ComputedStyle>) {
        let glyphs = self.shape(text, style);
        let width: f32 = glyphs.iter().map(|g| g.advance).sum();
        let limit = line.rect.right();
        let fits =
            |line: &Fragment| content_end(line).is_none_or(|end| end + width <= limit + 0.01);
        if !fits(line) {
            // Se quita contenido hasta la última oportunidad de corte con la que cabe; si no
            // hay ninguna, la línea queda vacía.
            while !(fits(line) && ends_at_wrap(line)) {
                if !pop_last(line) {
                    break;
                }
            }
        }
        let m = self.metrics(style);
        let x = content_end(line).unwrap_or(line.rect.x);
        let baseline = line.baseline.unwrap_or(line.rect.y + m.ascent);
        let mut f = Fragment::new(
            FragmentKind::Text,
            Rect::new(x, baseline - m.ascent, width, m.ascent + m.descent),
            style.clone(),
        );
        f.anonymous = true;
        f.text = Some((text.to_owned(), glyphs));
        f.baseline = Some(baseline);
        line.children.push(f);
    }
}
