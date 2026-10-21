//! Selección de texto en la página: arrastrar, doble clic (palabra), triple clic (párrafo) y
//! `Ctrl+A`, el resaltado y el texto que se copia. Trabaja sobre los fragmentos de texto en
//! orden de árbol, que en el flujo normal es el orden de lectura.

use bf_dom::Document;
use bf_layout::fragment::{Fragment, FragmentKind, Rect};
use bf_style::properties::types::{Position, Visibility};

/// Un fragmento de texto: dónde está, su texto y el borde x de cada carácter.
struct Run {
    rect: Rect,
    chars: Vec<char>,
    /// x absoluta del borde izquierdo de cada carácter, más el final (`chars.len() + 1`).
    edges: Vec<f32>,
    /// La line box y el bloque que la contiene: distinguen un salto de línea suave (mismo
    /// bloque, se copia como espacio) de uno entre bloques (se copia como `\n`).
    line: usize,
    block: usize,
    /// Dentro de una caja `position: fixed`: en coordenadas del viewport.
    fixed: bool,
}

/// Los fragmentos de texto de una página, en orden.
pub struct TextIndex {
    runs: Vec<Run>,
}

/// Un extremo de la selección: el fragmento y el carácter dentro de él.
pub type Point = (usize, usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Point,
    pub focus: Point,
}

impl Selection {
    pub fn collapsed(at: Point) -> Selection {
        Selection {
            anchor: at,
            focus: at,
        }
    }

    /// Los extremos en orden.
    pub fn range(&self) -> (Point, Point) {
        (self.anchor.min(self.focus), self.anchor.max(self.focus))
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.focus
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl TextIndex {
    /// Los fragmentos de texto de `root`, sin el contenido de los campos editables
    /// (`input`, `textarea`, `select`), que tienen su propia selección.
    pub fn build(doc: &Document, root: Option<&Fragment>) -> TextIndex {
        struct Walk<'a> {
            doc: &'a Document,
            runs: Vec<Run>,
            lines: usize,
            blocks: usize,
        }
        fn visit(w: &mut Walk, f: &Fragment, line: usize, block: usize, fixed: bool) {
            if f.kind != FragmentKind::Text
                && let Some(el) = f.node.and_then(|n| w.doc.element(n))
                && (el.is_html("input") || el.is_html("textarea") || el.is_html("select"))
            {
                return;
            }
            let (mut line, mut block, mut fixed) = (line, block, fixed);
            match f.kind {
                FragmentKind::Line => {
                    w.lines += 1;
                    line = w.lines;
                }
                FragmentKind::Block | FragmentKind::InlineBlock => {
                    w.blocks += 1;
                    block = w.blocks;
                    fixed = fixed || f.style.position == Position::Fixed;
                }
                FragmentKind::Text => {
                    if f.style.visibility == Visibility::Visible
                        && let Some((text, glyphs)) = &f.text
                        && !text.is_empty()
                    {
                        w.runs.push(Run {
                            rect: f.rect,
                            chars: text.chars().collect(),
                            edges: edges(f.rect.x, text, glyphs),
                            line,
                            block,
                            fixed,
                        });
                    }
                    return;
                }
                _ => {}
            }
            for c in &f.children {
                visit(w, c, line, block, fixed);
            }
        }
        let mut w = Walk {
            doc,
            runs: Vec::new(),
            lines: 0,
            blocks: 0,
        };
        if let Some(root) = root {
            visit(&mut w, root, 0, 0, false);
        }
        TextIndex { runs: w.runs }
    }

    /// El extremo más cercano al punto `(x, y)` del viewport con la página desplazada
    /// `scroll`: el fragmento de la línea bajo el punto (o el de la línea más cercana) y el
    /// borde de carácter más cercano a `x`.
    pub fn point_at(&self, x: f32, y: f32, scroll: (f32, f32)) -> Option<Point> {
        let mut best: Option<((f32, f32), usize)> = None;
        for (i, r) in self.runs.iter().enumerate() {
            let (px, py) = if r.fixed {
                (x, y)
            } else {
                (x + scroll.0, y + scroll.1)
            };
            let dy = if py < r.rect.y {
                r.rect.y - py
            } else if py >= r.rect.bottom() {
                py - r.rect.bottom() + 0.001
            } else {
                0.0
            };
            let dx = if px < r.rect.x {
                r.rect.x - px
            } else if px > r.rect.right() {
                px - r.rect.right()
            } else {
                0.0
            };
            if best.is_none_or(|(d, _)| (dy, dx) < d) {
                best = Some(((dy, dx), i));
            }
        }
        let (_, i) = best?;
        let r = &self.runs[i];
        let px = if r.fixed { x } else { x + scroll.0 };
        let c = r
            .edges
            .iter()
            .enumerate()
            .min_by(|a, b| (a.1 - px).abs().total_cmp(&(b.1 - px).abs()))
            .map_or(0, |(c, _)| c);
        Some((i, c))
    }

    /// La palabra en `p` (o el carácter, si no es de palabra).
    pub fn word_at(&self, p: Point) -> Selection {
        let chars = &self.runs[p.0].chars;
        let at = p.1.min(chars.len().saturating_sub(1));
        if chars.is_empty() || !is_word(chars[at]) {
            return Selection {
                anchor: (p.0, at),
                focus: (p.0, (at + 1).min(chars.len())),
            };
        }
        let start = (0..at).rev().take_while(|&i| is_word(chars[i])).last();
        let end = (at..chars.len()).take_while(|&i| is_word(chars[i])).last();
        Selection {
            anchor: (p.0, start.unwrap_or(at)),
            focus: (p.0, end.map_or(at, |e| e + 1)),
        }
    }

    /// Todo el texto del bloque que contiene `p` (el párrafo).
    pub fn block_at(&self, p: Point) -> Selection {
        let block = self.runs[p.0].block;
        let first = (0..=p.0)
            .rev()
            .take_while(|&i| self.runs[i].block == block)
            .last()
            .unwrap_or(p.0);
        let last = (p.0..self.runs.len())
            .take_while(|&i| self.runs[i].block == block)
            .last()
            .unwrap_or(p.0);
        Selection {
            anchor: (first, 0),
            focus: (last, self.runs[last].chars.len()),
        }
    }

    /// Todo el texto de la página.
    pub fn all(&self) -> Option<Selection> {
        let last = self.runs.len().checked_sub(1)?;
        Some(Selection {
            anchor: (0, 0),
            focus: (last, self.runs[last].chars.len()),
        })
    }

    /// Los tramos de cada fragmento que cubre `sel`, en coordenadas de la página (los fijos
    /// llevan sumado `scroll`, para pintarse con el resto).
    pub fn highlight(&self, sel: &Selection, scroll: (f32, f32)) -> Vec<Rect> {
        let ((s, sc), (e, ec)) = sel.range();
        let mut out = Vec::new();
        for i in s..=e.min(self.runs.len().saturating_sub(1)) {
            let r = &self.runs[i];
            let a = if i == s { sc } else { 0 }.min(r.chars.len());
            let b = if i == e { ec } else { r.chars.len() }.min(r.chars.len());
            if a >= b {
                continue;
            }
            let mut rect = Rect::new(r.edges[a], r.rect.y, r.edges[b] - r.edges[a], r.rect.height);
            if r.fixed {
                rect.translate(scroll.0, scroll.1);
            }
            out.push(rect);
        }
        out
    }

    /// El texto de `sel`: los saltos de línea suaves como espacios, los cambios de bloque
    /// como `\n`.
    pub fn text(&self, sel: &Selection) -> String {
        let ((s, sc), (e, ec)) = sel.range();
        let mut out = String::new();
        let mut prev: Option<&Run> = None;
        for i in s..=e.min(self.runs.len().saturating_sub(1)) {
            let r = &self.runs[i];
            let a = if i == s { sc } else { 0 }.min(r.chars.len());
            let b = if i == e { ec } else { r.chars.len() }.min(r.chars.len());
            if let Some(p) = prev
                && p.line != r.line
                && !out.is_empty()
            {
                if p.block == r.block {
                    if !out.ends_with(char::is_whitespace) {
                        out.push(' ');
                    }
                } else {
                    let trimmed = out.trim_end_matches([' ', '\t']).len();
                    out.truncate(trimmed);
                    out.push('\n');
                }
            }
            out.extend(&r.chars[a..b]);
            prev = Some(r);
        }
        out.trim_end().to_owned()
    }
}

/// El borde izquierdo (x absoluta) de cada carácter de `text` y el final, a partir de los
/// avances de los glifos y el offset de su clúster.
fn edges(x0: f32, text: &str, glyphs: &[bf_layout::font::Glyph]) -> Vec<f32> {
    let mut clusters: Vec<(usize, f32)> = glyphs.iter().map(|g| (g.cluster, g.advance)).collect();
    clusters.sort_by_key(|&(c, _)| c);
    let mut out = Vec::with_capacity(text.chars().count() + 1);
    let (mut x, mut g) = (x0, 0);
    for (byte, _) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        while g < clusters.len() && clusters[g].0 < byte {
            x += clusters[g].1;
            g += 1;
        }
        out.push(x);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(runs: &[(&str, usize, usize)]) -> TextIndex {
        // Cada fragmento en su propia fila de 20 px, con caracteres de 10 px.
        TextIndex {
            runs: runs
                .iter()
                .enumerate()
                .map(|(i, &(t, line, block))| Run {
                    rect: Rect::new(0.0, i as f32 * 20.0, t.chars().count() as f32 * 10.0, 20.0),
                    chars: t.chars().collect(),
                    edges: (0..=t.chars().count()).map(|c| c as f32 * 10.0).collect(),
                    line,
                    block,
                    fixed: false,
                })
                .collect(),
        }
    }

    #[test]
    fn text_joins_soft_wraps_and_blocks() {
        let idx = index(&[("Hola ", 1, 1), ("mundo", 2, 1), ("Otro párrafo", 3, 2)]);
        let all = idx.all().unwrap();
        assert_eq!(idx.text(&all), "Hola mundo\nOtro párrafo");
        let part = Selection {
            anchor: (1, 2),
            focus: (2, 4),
        };
        assert_eq!(idx.text(&part), "ndo\nOtro");
    }

    #[test]
    fn point_word_and_block() {
        let idx = index(&[("uno dos", 1, 1), ("tres", 2, 1), ("cuatro", 3, 2)]);
        // Fila 0, x = 52: entre "dos" (borde 50) y el siguiente carácter.
        let p = idx.point_at(52.0, 5.0, (0.0, 0.0)).unwrap();
        assert_eq!(p, (0, 5));
        assert_eq!(idx.text(&idx.word_at(p)), "dos");
        assert_eq!(idx.text(&idx.block_at(p)), "uno dos tres");
        // Debajo de todo: el final del último fragmento.
        assert_eq!(idx.point_at(500.0, 900.0, (0.0, 0.0)), Some((2, 6)));
        let r = idx.highlight(
            &Selection {
                anchor: (0, 4),
                focus: (0, 7),
            },
            (0.0, 0.0),
        );
        assert_eq!(r, vec![Rect::new(40.0, 0.0, 30.0, 20.0)]);
    }
}
