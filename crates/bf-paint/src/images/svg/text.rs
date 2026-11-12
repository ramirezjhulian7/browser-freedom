//! Layout y contornos del texto SVG (SVG 2 §11.8): el avance con las fuentes de `bf-font`,
//! `letter-spacing` y `word-spacing` (CSS Text 3 §8), `dx`/`dy`/`rotate` por carácter,
//! `text-anchor` por *chunk* (§11.8.2), líneas base (§11.10.2) y texto sobre un camino
//! (`textPath`, *text on a path layout rules*, §11.8.3) y texto ajustado en líneas con
//! `inline-size` (§11.4.1).

use bf_layout::{FontProvider, FontRequest, Glyph, PathCommand};
use bf_style::properties::types::{FamilyName, FontFamily};
use bf_svg::geom::PathMeasure;
use bf_svg::{Baseline, Chunk, Fill, Span, Stroke, Text, TextAnchor};
use tiny_skia::{Path, PathBuilder};

/// Tamaño al que se piden las métricas para escalarlas después (las de `bf-font` vienen
/// redondeadas a px, que con fuentes pequeñas no sirven para las líneas base).
const METRICS_SIZE: f32 = 1000.0;

/// Un glifo colocado en la línea de su chunk.
struct Placed {
    glyph: Glyph,
    /// Distancia en la dirección del texto desde el principio del chunk (con los `dx`).
    s: f32,
    /// `dy` acumulado desde el principio del chunk.
    dy: f32,
    /// `rotate` en grados.
    rotate: f32,
    /// El clúster es un espacio (oportunidad de corte de línea).
    space: bool,
}

/// Un fragmento ya medido.
struct Laid<'a> {
    span: &'a Span,
    glyphs: Vec<Placed>,
    /// Desplazamiento del glifo hacia abajo respecto de la posición del texto: su línea base
    /// de alineación y `baseline-shift`.
    base: f32,
}

/// Familia de CSS a partir de los nombres de `font-family`.
fn family(names: &[String]) -> FontFamily {
    const GENERIC: &[&str] = &[
        "serif",
        "sans-serif",
        "monospace",
        "cursive",
        "fantasy",
        "system-ui",
    ];
    FontFamily(
        names
            .iter()
            .map(|n| {
                if GENERIC.contains(&n.to_ascii_lowercase().as_str()) {
                    FamilyName::Generic(n.to_ascii_lowercase())
                } else {
                    FamilyName::Named(n.clone())
                }
            })
            .collect(),
    )
}

/// Distancia (hacia abajo) de la línea base de alineación a la alfabética (SVG 2 §11.10.2.1
/// y CSS Inline 3 §4.1), con las métricas de la fuente: `hanging` a 0,8 del ascent y
/// `mathematical` a la mitad, como los navegadores cuando la fuente no trae tabla `BASE`.
fn baseline_offset(fonts: &dyn FontProvider, req: &FontRequest<'_>, b: Baseline) -> f32 {
    if b == Baseline::Alphabetic {
        return 0.0;
    }
    let m = fonts.metrics(&FontRequest {
        size: METRICS_SIZE,
        ..*req
    });
    let k = req.size / METRICS_SIZE;
    let (ascent, descent) = (m.ascent * k, m.descent * k);
    match b {
        Baseline::Alphabetic => 0.0,
        Baseline::Hanging => 0.8 * ascent,
        Baseline::Middle => m.x_height * k / 2.0,
        Baseline::Central => (ascent - descent) / 2.0,
        Baseline::TextBeforeEdge => ascent,
        Baseline::TextAfterEdge | Baseline::Ideographic => -descent,
        Baseline::Mathematical => ascent / 2.0,
    }
}

/// Mide un chunk: sus fragmentos con los glifos colocados, su anchura (la distancia de la
/// pluma al final, que es lo que alinea `text-anchor`) y el `dy` acumulado.
fn lay_chunk<'a>(fonts: &dyn FontProvider, chunk: &'a Chunk) -> (Vec<Laid<'a>>, f32, f32) {
    let (mut s, mut dy) = (0.0f32, 0.0f32);
    let mut out = Vec::with_capacity(chunk.spans.len());
    for span in &chunk.spans {
        let fam = family(&span.family);
        let req = FontRequest {
            family: &fam,
            size: span.size,
            weight: span.weight,
            italic: span.italic,
        };
        let glyphs = fonts.shape(&span.text, &req);
        let base = baseline_offset(fonts, &req, span.baseline) - span.baseline_shift;
        // Índice de carácter de cada clúster (los `cluster` son offsets en bytes).
        let starts: Vec<usize> = span.text.char_indices().map(|(b, _)| b).collect();
        let chars: Vec<char> = span.text.chars().collect();
        let char_of = |byte: usize| starts.partition_point(|&b| b < byte);
        let mut clusters: Vec<usize> = glyphs.iter().map(|g| char_of(g.cluster)).collect();
        clusters.sort_unstable();
        clusters.dedup();
        // Los caracteres del clúster que empieza en `c`.
        let range = |c: usize| {
            let end = clusters
                .iter()
                .copied()
                .find(|&n| n > c)
                .unwrap_or(chars.len());
            c..end
        };
        let at = |v: &[f32], c: usize| v.get(c).copied().unwrap_or(0.0);
        let mut placed = Vec::with_capacity(glyphs.len());
        let mut rotate = 0.0;
        for (i, g) in glyphs.iter().enumerate() {
            let c = char_of(g.cluster);
            let first = i == 0 || glyphs[i - 1].cluster != g.cluster;
            let last = glyphs.get(i + 1).is_none_or(|n| n.cluster != g.cluster);
            if first {
                for k in range(c) {
                    s += at(&span.dx, k);
                    dy += at(&span.dy, k);
                }
                rotate = at(&span.rotate, c);
            }
            placed.push(Placed {
                glyph: *g,
                s,
                dy,
                rotate,
                space: range(c).all(|k| matches!(chars.get(k), Some(' ' | '\u{a0}'))),
            });
            s += g.advance;
            if last {
                // El espaciado va detrás de cada carácter tipográfico (CSS Text 3 §8.2) y
                // `word-spacing`, detrás de cada separador de palabras (§8.1).
                let spaces = range(c)
                    .filter(|&k| matches!(chars.get(k), Some(' ' | '\u{a0}')))
                    .count();
                s += span.letter_spacing + span.word_spacing * spaces as f32;
            }
        }
        out.push(Laid {
            span,
            glyphs: placed,
            base,
        });
    }
    (out, s, dy)
}

/// Añade el contorno de `g` a `pb`, con su origen en `origin`, la dirección de la línea base
/// `dir` (vector unitario) y `base` hacia abajo de ella.
fn outline(
    fonts: &dyn FontProvider,
    pb: &mut PathBuilder,
    g: &Glyph,
    size: f32,
    origin: (f32, f32),
    dir: (f32, f32),
    base: f32,
) {
    let Some(outline) = fonts.outline(g.font, g.id) else {
        return;
    };
    let k = size / outline.units_per_em;
    let (ox, oy) = (origin.0 + g.x_offset, origin.1 + base - g.y_offset);
    let (c, s) = dir;
    let map = |x: f32, y: f32| {
        if (c, s) == (1.0, 0.0) {
            (ox + x * k, oy - y * k)
        } else {
            // Girado alrededor del origen del glifo.
            let (lx, ly) = (g.x_offset + x * k, base - g.y_offset - y * k);
            (origin.0 + lx * c - ly * s, origin.1 + lx * s + ly * c)
        }
    };
    for cmd in &outline.commands {
        match *cmd {
            PathCommand::MoveTo(x, y) => {
                let (x, y) = map(x, y);
                pb.move_to(x, y);
            }
            PathCommand::LineTo(x, y) => {
                let (x, y) = map(x, y);
                pb.line_to(x, y);
            }
            PathCommand::QuadTo(x1, y1, x, y) => {
                let (x1, y1) = map(x1, y1);
                let (x, y) = map(x, y);
                pb.quad_to(x1, y1, x, y);
            }
            PathCommand::CurveTo(x1, y1, x2, y2, x, y) => {
                let (x1, y1) = map(x1, y1);
                let (x2, y2) = map(x2, y2);
                let (x, y) = map(x, y);
                pb.cubic_to(x1, y1, x2, y2, x, y);
            }
            PathCommand::Close => pb.close(),
        }
    }
}

/// `dir` girado `deg` grados (el `rotate` del carácter sobre la dirección de la línea).
fn turn(dir: (f32, f32), deg: f32) -> (f32, f32) {
    if deg == 0.0 {
        return dir;
    }
    let (s, c) = deg.to_radians().sin_cos();
    (dir.0 * c - dir.1 * s, dir.1 * c + dir.0 * s)
}

/// Los glifos de cada fragmento como paths en coordenadas de usuario del `text`, con su
/// relleno, su trazo y si el trazo va primero.
pub(super) fn paths(
    fonts: &dyn FontProvider,
    t: &Text,
) -> Vec<(Path, Option<Fill>, Option<Stroke>, bool)> {
    let mut out = Vec::new();
    let measures: Vec<PathMeasure> = t.paths.iter().map(|p| PathMeasure::new(&p.segs)).collect();
    let (mut pen_x, mut pen_y) = (0.0f32, 0.0f32);
    for chunk in &t.chunks {
        let (laid, width, dy_total) = lay_chunk(fonts, chunk);
        let shift = match chunk.anchor {
            TextAnchor::Start => 0.0,
            TextAnchor::Middle => width / 2.0,
            TextAnchor::End => width,
        };
        let on_path = chunk
            .path
            .and_then(|i| Some((measures.get(i)?, t.paths.get(i)?.start_offset)));
        match on_path {
            None if t.inline_size.is_some() => {
                let x = chunk.x.unwrap_or(pen_x);
                let y = chunk.y.unwrap_or(pen_y);
                let size = t.inline_size.unwrap_or(f32::INFINITY);
                wrapped(fonts, &laid, chunk.anchor, (x, y), size, &mut out);
            }
            None => {
                let x0 = chunk.x.unwrap_or(pen_x) - shift;
                let y0 = chunk.y.unwrap_or(pen_y);
                for l in &laid {
                    let mut pb = PathBuilder::new();
                    for p in &l.glyphs {
                        let origin = (x0 + p.s, y0 + p.dy);
                        let dir = turn((1.0, 0.0), p.rotate);
                        outline(fonts, &mut pb, &p.glyph, l.span.size, origin, dir, l.base);
                    }
                    if let Some(path) = pb.finish() {
                        out.push(paint(l.span, path));
                    }
                }
                pen_x = x0 + width;
                pen_y = y0 + dy_total;
            }
            Some((m, start_offset)) => {
                // §11.8.3: cada glifo va en el punto del camino a la distancia de su punto
                // medio de avance, girado con la tangente allí; los que caen fuera del camino
                // no se pintan. `dy` desplaza en perpendicular.
                let s0 = start_offset + chunk.x.unwrap_or(0.0) - shift;
                for l in &laid {
                    let mut pb = PathBuilder::new();
                    for p in &l.glyphs {
                        let s = s0 + p.s;
                        let mid = s + p.glyph.advance / 2.0;
                        let Some(piece) = m.piece_at(mid) else {
                            continue;
                        };
                        // El origen, sobre la recta del tramo del punto medio (en un tramo
                        // recto, el punto del camino a la distancia `s`).
                        let (px, py) = piece.point(s);
                        let origin = (px - piece.uy * p.dy, py + piece.ux * p.dy);
                        let dir = turn((piece.ux, piece.uy), p.rotate);
                        outline(fonts, &mut pb, &p.glyph, l.span.size, origin, dir, l.base);
                    }
                    if let Some(path) = pb.finish() {
                        out.push(paint(l.span, path));
                    }
                }
                // Lo que sigue al `textPath` empieza al final del camino (§11.8.2).
                if let Some((x, y)) = m.end() {
                    (pen_x, pen_y) = (x, y);
                }
            }
        }
    }
    out
}

/// Una línea del texto ajustado.
struct Line {
    /// Primer glifo (índices de fragmento y de glifo).
    start: (usize, usize),
    /// Distancia desde el principio del chunk a la que empieza.
    s0: f32,
    /// Ancho sin los espacios del final (lo que alinea `text-anchor`).
    width: f32,
}

/// Texto con `inline-size` (SVG 2 §11.4.1): los glifos del chunk cortados en líneas de
/// `size` de ancho como máximo, por los espacios (sin oportunidad de corte, la palabra
/// desborda); cada línea se alinea con `text-anchor` en `x` y la primera línea base va en `y`.
/// Las líneas se apilan como en CSS Inline 3 §5: el `line-height` de cada fragmento se
/// reparte (*half-leading*) alrededor de su ascent y su descent.
fn wrapped(
    fonts: &dyn FontProvider,
    laid: &[Laid<'_>],
    anchor: TextAnchor,
    (x, y): (f32, f32),
    size: f32,
    out: &mut Vec<(Path, Option<Fill>, Option<Stroke>, bool)>,
) {
    // Por encima y por debajo de la línea base, de cada fragmento.
    let extents: Vec<(f32, f32)> = laid
        .iter()
        .map(|l| {
            let fam = family(&l.span.family);
            let req = FontRequest {
                family: &fam,
                size: METRICS_SIZE,
                weight: l.span.weight,
                italic: l.span.italic,
            };
            let m = fonts.metrics(&req);
            let k = l.span.size / METRICS_SIZE;
            let (a, d) = (m.ascent * k, m.descent * k);
            let lh = l
                .span
                .line_height
                .unwrap_or_else(|| m.normal_line_height() * k);
            let half = (lh - (a + d)) / 2.0;
            (a + half, d + half)
        })
        .collect();
    let new_line = |start: (usize, usize), s0: f32| Line {
        start,
        s0,
        width: 0.0,
    };
    let mut lines: Vec<Line> = Vec::new();
    let mut line = new_line((0, 0), 0.0);
    // Inicio de la última palabra de la línea (detrás de un espacio), si la hay, y el ancho
    // de la línea sin ella.
    let mut word: Option<((usize, usize), f32, f32)> = None;
    let (mut after_space, mut ink) = (false, false);
    for (li, l) in laid.iter().enumerate() {
        for (gi, p) in l.glyphs.iter().enumerate() {
            if p.space {
                after_space = true;
                continue;
            }
            if after_space && ink {
                word = Some(((li, gi), p.s, line.width));
            }
            after_space = false;
            if p.s + p.glyph.advance - line.s0 > size
                && let Some((start, s0, width)) = word.take()
            {
                line.width = width;
                lines.push(std::mem::replace(&mut line, new_line(start, s0)));
            }
            ink = true;
            line.width = line.width.max(p.s + p.glyph.advance - line.s0);
        }
    }
    lines.push(line);
    let line_of = |li: usize, gi: usize| {
        lines
            .partition_point(|ln| ln.start <= (li, gi))
            .saturating_sub(1)
    };
    // Alto de cada línea: el de los fragmentos que tienen glifos en ella.
    let mut heights = vec![(0.0f32, 0.0f32); lines.len()];
    for (li, l) in laid.iter().enumerate() {
        for gi in 0..l.glyphs.len() {
            let h = &mut heights[line_of(li, gi)];
            h.0 = h.0.max(extents[li].0);
            h.1 = h.1.max(extents[li].1);
        }
    }
    let mut baselines = Vec::with_capacity(lines.len());
    let mut base = y;
    for k in 0..lines.len() {
        if k > 0 {
            base += heights[k - 1].1 + heights[k].0;
        }
        baselines.push(base);
    }
    for (li, l) in laid.iter().enumerate() {
        let mut pb = PathBuilder::new();
        for (gi, p) in l.glyphs.iter().enumerate() {
            let k = line_of(li, gi);
            let ln = &lines[k];
            let shift = match anchor {
                TextAnchor::Start => 0.0,
                TextAnchor::Middle => ln.width / 2.0,
                TextAnchor::End => ln.width,
            };
            let origin = (x - shift + p.s - ln.s0, baselines[k] + p.dy);
            let dir = turn((1.0, 0.0), p.rotate);
            outline(fonts, &mut pb, &p.glyph, l.span.size, origin, dir, l.base);
        }
        if let Some(path) = pb.finish() {
            out.push(paint(l.span, path));
        }
    }
}

fn paint(span: &Span, path: Path) -> (Path, Option<Fill>, Option<Stroke>, bool) {
    (
        path,
        span.fill.clone(),
        span.stroke.clone(),
        span.stroke_first,
    )
}
