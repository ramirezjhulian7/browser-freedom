//! Texto (SVG 2 §11): `text`, `tspan`, `a` y `textPath` a *text chunks* ([`Chunk`]) con
//! fragmentos de un solo estilo ([`Span`]).
//!
//! Sigue el algoritmo de layout de §11.8.2 hasta donde no hacen falta las métricas de la
//! fuente: se recogen los caracteres direccionables (con los espacios ya colapsados), se
//! resuelven las listas `x`, `y`, `dx`, `dy` y `rotate` de cada elemento sobre ellos
//! (*resolve character positioning*: los descendientes mandan sobre los ancestros y el último
//! `rotate` se repite) y cada carácter con `x` o `y` absoluto abre un *chunk* (*anchored
//! chunk*), igual que el principio de cada `textPath`. El avance, `text-anchor` y la
//! colocación sobre el camino (§11.8.3) son de quien pinta, que tiene las fuentes.

use super::{
    Axis, Builder, Len, LineHeight, PaintSpec, St, Viewport, conditions_pass, ident, parse_len,
};
use bf_css::syntax::TokenKind;

use crate::geom;
use crate::{Baseline, Chunk, Node, Span, Text, TextAnchor, TextPath};

/// Palabra clave de `dominant-baseline` / `alignment-baseline` (SVG 2 §11.10.2).
pub(super) fn baseline_keyword(k: &str) -> Option<Baseline> {
    Some(match k {
        "alphabetic" => Baseline::Alphabetic,
        "hanging" => Baseline::Hanging,
        "middle" => Baseline::Middle,
        "central" => Baseline::Central,
        "text-before-edge" | "text-top" | "before-edge" => Baseline::TextBeforeEdge,
        "text-after-edge" | "text-bottom" | "after-edge" => Baseline::TextAfterEdge,
        "ideographic" => Baseline::Ideographic,
        "mathematical" => Baseline::Mathematical,
        _ => return None,
    })
}

/// Texto de un elemento, con los espacios ya colapsados.
struct Piece {
    text: String,
    /// Plantilla del fragmento: estilo sin texto ni posiciones.
    span: Span,
    anchor: TextAnchor,
    /// Índice del `textPath` que lo contiene.
    path: Option<usize>,
}

/// Listas de posición de un elemento (§11.6) y los trozos que cubre (él y sus descendientes).
#[derive(Default)]
struct Lists {
    pieces: (usize, usize),
    x: Vec<f32>,
    y: Vec<f32>,
    dx: Vec<f32>,
    dy: Vec<f32>,
    rotate: Vec<f32>,
}

#[derive(Default)]
struct Walk {
    pieces: Vec<Piece>,
    /// En orden de documento (los ancestros antes que sus descendientes).
    lists: Vec<Lists>,
    paths: Vec<TextPath>,
}

/// Controles bidi explícitos (UAX #9 §2): no se pintan. Aproximación sin algoritmo bidi: solo
/// `RLO` (U+202E) invierte el orden visual de lo que sigue hasta `PDF` (U+202C), dentro de
/// cada trozo; el resto de controles se quitan sin más.
fn bidi_controls(text: &str, rlo: &mut bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    for c in text.chars() {
        match c {
            '\u{202e}' => *rlo = true,
            '\u{202c}' => {
                out.extend(run.drain(..).rev());
                *rlo = false;
            }
            '\u{200e}' | '\u{200f}' | '\u{202a}' | '\u{202b}' | '\u{202d}' => {}
            '\u{2066}'..='\u{2069}' => {}
            c if *rlo => run.push(c),
            c => out.push(c),
        }
    }
    out.extend(run.chars().rev());
    out
}

/// Elemento `text`: `None` si no tiene caracteres.
pub(super) fn build(b: &mut Builder, i: usize, st: &St, vp: Viewport) -> Option<Node> {
    let mut w = Walk::default();
    walk(b, i, st, vp, &mut w, None, 0.0);
    // Espacios: los del principio y el final del elemento `text` no cuentan (§11.5).
    if let Some(first) = w.pieces.first_mut() {
        first.text = first.text.trim_start().to_owned();
    }
    if let Some(last) = w.pieces.last_mut() {
        last.text = last.text.trim_end().to_owned();
    }
    let mut rlo = false;
    for p in &mut w.pieces {
        p.text = bidi_controls(&p.text, &mut rlo);
    }
    // Índice global del primer carácter de cada trozo (y el total al final).
    let mut offs = Vec::with_capacity(w.pieces.len() + 1);
    let mut total = 0;
    for p in &w.pieces {
        offs.push(total);
        total += p.text.chars().count();
    }
    offs.push(total);
    if total == 0 {
        return None;
    }

    // *Resolve character positioning* (§11.8.2, paso 3).
    let mut rx: Vec<Option<f32>> = vec![None; total];
    let mut ry: Vec<Option<f32>> = vec![None; total];
    let mut rdx = vec![0.0f32; total];
    let mut rdy = vec![0.0f32; total];
    let mut rrot = vec![0.0f32; total];
    for l in &w.lists {
        let (c0, c1) = (offs[l.pieces.0], offs[l.pieces.1]);
        for (j, g) in (c0..c1).enumerate() {
            if let Some(&v) = l.x.get(j) {
                rx[g] = Some(v);
            }
            if let Some(&v) = l.y.get(j) {
                ry[g] = Some(v);
            }
            if let Some(&v) = l.dx.get(j) {
                rdx[g] = v;
            }
            if let Some(&v) = l.dy.get(j) {
                rdy[g] = v;
            }
            // El último valor de `rotate` sigue para el resto de caracteres del elemento.
            if let Some(&v) = l.rotate.get(j).or(l.rotate.last()) {
                rrot[g] = v;
            }
        }
    }

    // `inline-size` (§11.4.1): texto ajustado en un área; solo cuentan la primera `x` e `y`
    // (las demás posiciones absolutas se ignoran) y no hay `textPath`.
    let inline_size = inline_size(b, i, st, vp).filter(|_| w.paths.is_empty());
    if inline_size.is_some() {
        for v in rx.iter_mut().skip(1).chain(ry.iter_mut().skip(1)) {
            *v = None;
        }
    }

    // Los *chunks*: uno nuevo en cada posición absoluta y al entrar o salir de un `textPath`.
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut g = 0;
    for p in &w.pieces {
        let mut fresh_span = true;
        for c in p.text.chars() {
            // En un camino horizontal, `y` absoluto no cuenta (§11.8.3).
            let anchored = rx[g].is_some() || (p.path.is_none() && ry[g].is_some());
            let new_chunk = match chunks.last() {
                None => true,
                Some(last) => anchored || last.path != p.path,
            };
            if new_chunk {
                let first = chunks.is_empty();
                chunks.push(Chunk {
                    // El primer chunk del `text` empieza en (0, 0) si no dice otra cosa; uno
                    // de camino, en su principio.
                    x: rx[g].or((first || p.path.is_some()).then_some(0.0)),
                    y: if p.path.is_some() {
                        None
                    } else {
                        ry[g].or(first.then_some(0.0))
                    },
                    anchor: p.anchor,
                    spans: Vec::new(),
                    path: p.path,
                });
                fresh_span = true;
            }
            let Some(chunk) = chunks.last_mut() else {
                break;
            };
            if fresh_span {
                chunk.spans.push(p.span.clone());
                fresh_span = false;
            }
            if let Some(s) = chunk.spans.last_mut() {
                s.text.push(c);
                s.dx.push(rdx[g]);
                s.dy.push(rdy[g]);
                s.rotate.push(rrot[g]);
            }
            g += 1;
        }
    }
    for s in chunks.iter_mut().flat_map(|c| c.spans.iter_mut()) {
        for v in [&mut s.dx, &mut s.dy, &mut s.rotate] {
            if v.iter().all(|d| *d == 0.0) {
                v.clear();
            }
        }
    }
    Some(Node::Text(Text {
        transform: geom::Transform::IDENTITY,
        chunks,
        paths: w.paths,
        inline_size,
    }))
}

/// `inline-size` del elemento `text` (SVG 2 §11.4.1): `None` si es `0` o `auto`; los
/// porcentajes, del ancho del viewport (texto horizontal).
fn inline_size(b: &Builder, i: usize, st: &St, vp: Viewport) -> Option<f32> {
    let v = b.node(i).props.get("inline-size")?;
    let px = match super::one(v).and_then(super::cv_len)? {
        Len::Pct(p) => vp.w * p / 100.0,
        l => vp.resolve(l, Axis::X, st.size),
    };
    (px > 0.0 && px.is_finite()).then_some(px)
}

/// Lista de `<length-percentage>` de un atributo (separados por espacios y/o comas); vacía si
/// falta o tiene algún error.
fn length_list(b: &Builder, i: usize, name: &str, axis: Axis, vp: Viewport, st: &St) -> Vec<f32> {
    let Some(v) = b.node(i).attr(name) else {
        return Vec::new();
    };
    v.split(|c: char| c == ',' || c.is_ascii_whitespace())
        .filter(|s| !s.is_empty())
        .map(|s| parse_len(s).map(|l| vp.resolve(l, axis, st.size)))
        .collect::<Option<Vec<f32>>>()
        .unwrap_or_default()
}

/// `baseline-shift` (SVG 2 §11.10.2.3) de un elemento, hacia arriba en unidades de usuario:
/// `sub` y `super` como los navegadores (1/5 y 1/3 del tamaño de la fuente) y los
/// porcentajes, del tamaño de la fuente.
fn baseline_shift(b: &Builder, i: usize, vp: Viewport, st: &St) -> f32 {
    let Some(v) = b.node(i).props.get("baseline-shift") else {
        return 0.0;
    };
    match ident(v).as_deref() {
        Some("sub") => -st.size / 5.0,
        Some("super") => st.size / 3.0,
        Some(_) => 0.0,
        None => match super::one(v).and_then(super::cv_len) {
            Some(Len::Pct(p)) => st.size * p / 100.0,
            Some(l) => vp.resolve(l, Axis::Y, st.size),
            None => 0.0,
        },
    }
}

/// Recorre `text`/`tspan`/`a`/`textPath`: el texto se normaliza como `white-space: normal`
/// (espacios colapsados también entre elementos) y cada elemento deja sus listas.
fn walk(
    b: &mut Builder,
    i: usize,
    st: &St,
    vp: Viewport,
    w: &mut Walk,
    path: Option<usize>,
    shift: f32,
) {
    let n = b.node(i);
    let li = w.lists.len();
    let start = w.pieces.len();
    let mut lists = Lists {
        x: length_list(b, i, "x", Axis::X, vp, st),
        y: length_list(b, i, "y", Axis::Y, vp, st),
        dx: length_list(b, i, "dx", Axis::X, vp, st),
        dy: length_list(b, i, "dy", Axis::Y, vp, st),
        rotate: n
            .attr("rotate")
            .and_then(geom::parse_numbers)
            .unwrap_or_default(),
        ..Lists::default()
    };
    if n.name == "textPath" {
        // El principio de un `textPath` empieza un chunk al principio del camino (§11.8.2);
        // sus `x`/`y`/`dx`/`dy`/`rotate` no existen.
        lists = Lists {
            x: vec![0.0],
            ..Lists::default()
        };
    }
    w.lists.push(lists);
    // `alignment-baseline` (no heredada) manda sobre `dominant-baseline` en el propio texto.
    let baseline = n
        .props
        .get("alignment-baseline")
        .and_then(|v| ident(v))
        .and_then(|k| baseline_keyword(&k))
        .unwrap_or(st.dominant_baseline);
    let shift = if n.name == "text" {
        shift
    } else {
        shift + baseline_shift(b, i, vp, st)
    };
    let mut template: Option<Span> = None;
    let kids = n.children.clone();
    for k in kids {
        let kn = b.node(k);
        if kn.is_text() {
            let text: String = kn
                .text
                .chars()
                .map(|c| if c.is_whitespace() { ' ' } else { c })
                .collect();
            // Colapsa con el espacio final del trozo anterior.
            let mut last_space = w.pieces.last().is_none_or(|p| p.text.ends_with(' '));
            let mut collapsed = String::with_capacity(text.len());
            for c in text.chars() {
                if c == ' ' {
                    if !last_space {
                        collapsed.push(' ');
                    }
                    last_space = true;
                } else {
                    collapsed.push(c);
                    last_space = false;
                }
            }
            if collapsed.is_empty() {
                continue;
            }
            let span = match &template {
                Some(s) => s.clone(),
                None => {
                    // `visibility: hidden`: los caracteres ocupan su sitio pero no se pintan.
                    let (fill, stroke) = if st.visible {
                        (b.fill(st, None, vp), b.stroke(st, None, vp))
                    } else {
                        (None, None)
                    };
                    let s = Span {
                        text: String::new(),
                        family: st.family.clone(),
                        size: st.size,
                        weight: st.weight,
                        italic: st.italic,
                        fill,
                        stroke,
                        dx: Vec::new(),
                        dy: Vec::new(),
                        rotate: Vec::new(),
                        letter_spacing: st.letter_spacing,
                        word_spacing: st.word_spacing,
                        baseline,
                        baseline_shift: shift,
                        stroke_first: st.paint_order.iter().position(|&k| k == 1)
                            < st.paint_order.iter().position(|&k| k == 0),
                        line_height: match st.line_height {
                            LineHeight::Normal => None,
                            LineHeight::Factor(f) => Some(f * st.size),
                            LineHeight::Px(px) => Some(px),
                        },
                    };
                    template = Some(s.clone());
                    s
                }
            };
            w.pieces.push(Piece {
                text: collapsed,
                span,
                anchor: st.anchor,
                path,
            });
            continue;
        }
        if Builder::display_none(kn) || !conditions_pass(kn) {
            continue;
        }
        match kn.name.as_str() {
            "tspan" | "a" => {
                let kst = b.computed(kn, st, vp);
                walk(b, k, &kst, vp, w, path, shift);
            }
            // Un `textPath` dentro de otro no se pinta; uno sin camino válido, tampoco.
            "textPath" if path.is_none() => {
                let kst = b.computed(kn, st, vp);
                if let Some(tp) = text_path(b, k, &kst, vp) {
                    w.paths.push(tp);
                    let p = w.paths.len() - 1;
                    walk(b, k, &kst, vp, w, Some(p), shift);
                }
            }
            _ => {}
        }
    }
    w.lists[li].pieces = (start, w.pieces.len());
}

/// El camino de un `textPath` (SVG 2 §11.8.1): el atributo `path` si tiene datos válidos y
/// si no el elemento de `href` (con su `transform`, en el sistema del `text`), invertido con
/// `side="right"`, y `startOffset` resuelto (los porcentajes, de la longitud medida; las
/// longitudes, escaladas por el `pathLength` del elemento referenciado).
fn text_path(b: &mut Builder, i: usize, st: &St, vp: Viewport) -> Option<TextPath> {
    let n = b.node(i);
    let inline = n
        .attr("path")
        .map(geom::parse_path)
        .filter(|s| !s.is_empty());
    let (mut segs, scale) = match inline {
        // El `pathLength` no se aplica a los datos en línea.
        Some(segs) => (segs, None),
        None => {
            let r = b.by_id(n.attr("href")?)?;
            let rn = b.node(r);
            if !matches!(
                rn.name.as_str(),
                "path" | "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon"
            ) {
                return None;
            }
            // Solo la geometría: sin pintura, marcadores ni `visibility`.
            let mut rst = b.computed(rn, &b.resource_st(), vp);
            rst.fill = PaintSpec::None;
            rst.stroke = PaintSpec::None;
            rst.visible = true;
            rst.markers = [None, None, None];
            rst.paint_order = [0, 1, 2];
            let node = b.content(r, &rst, vp)?;
            let Node::Path(p) = &node else {
                return None;
            };
            let rn = b.node(r);
            let t = b.element_transform(rn, &node, vp, &rst);
            let segs = geom::transform_segs(&p.segs, &t);
            // La propiedad `path-length` (tentativa, como en Chrome) manda sobre el atributo.
            let css = rn
                .props
                .get("path-length")
                .and_then(|v| match super::one(v)?.token()? {
                    TokenKind::Number(n) | TokenKind::Dimension(n, _) => Some(n.value as f32),
                    _ => None,
                });
            let scale = css
                .or_else(|| {
                    rn.attr("pathLength")
                        .and_then(|v| v.trim().parse::<f32>().ok())
                })
                .filter(|v| *v >= 0.0 && v.is_finite())
                .map(|pl| geom::length(&segs) / pl);
            (segs, scale)
        }
    };
    let n = b.node(i);
    if n.attr("side").is_some_and(|s| s.trim() == "right") {
        segs = geom::reverse(&segs);
    }
    let start_offset = match n.attr("startOffset").and_then(parse_len) {
        Some(Len::Pct(p)) => geom::length(&segs) * p / 100.0,
        Some(l) => {
            let v = vp.resolve(l, Axis::X, st.size);
            match scale {
                // `pathLength="0"` escala a infinito (§9.5.1); cero sigue siendo cero.
                Some(k) if v != 0.0 => v * k,
                _ => v,
            }
        }
        None => 0.0,
    };
    Some(TextPath {
        segs,
        start_offset: if start_offset.is_nan() {
            0.0
        } else {
            start_offset
        },
    })
}

#[cfg(test)]
mod tests {
    use crate::geom::{Seg, parse_path};
    use crate::{Baseline, Group, Node, Svg, Text};

    fn find(g: &Group) -> Option<&Text> {
        g.children.iter().find_map(|c| match c {
            Node::Text(t) => Some(t),
            Node::Group(g) => find(g),
            _ => None,
        })
    }

    fn text(body: &str) -> Text {
        let src = format!("<svg xmlns='http://www.w3.org/2000/svg'>{body}</svg>");
        let tree = Svg::parse(&src).unwrap().tree(100.0, 100.0);
        find(&tree.root).expect("un text").clone()
    }

    #[test]
    fn absolute_lists_start_chunks() {
        let t = text("<text x='10 20 30' y='5'>ABCD</text>");
        let pos: Vec<_> = t.chunks.iter().map(|c| (c.x, c.y)).collect();
        assert_eq!(
            pos,
            vec![
                (Some(10.0), Some(5.0)),
                (Some(20.0), None),
                (Some(30.0), None)
            ]
        );
        assert_eq!(t.chunks[2].spans[0].text, "CD");
        // Un `tspan` con su propia lista manda sobre la del `text`.
        let t = text("<text x='0 1 2'>a<tspan x='50'>bc</tspan></text>");
        let pos: Vec<_> = t.chunks.iter().map(|c| c.x).collect();
        assert_eq!(pos, vec![Some(0.0), Some(50.0), Some(2.0)]);
    }

    #[test]
    fn relative_lists_and_rotate() {
        let t = text("<text dx='1 2' rotate='10 20'>abc<tspan dx='5' dy='3'>d</tspan></text>");
        assert_eq!(t.chunks.len(), 1);
        let s = &t.chunks[0].spans;
        assert_eq!((s[0].text.as_str(), s[1].text.as_str()), ("abc", "d"));
        assert_eq!(s[0].dx, vec![1.0, 2.0, 0.0]);
        assert!(s[0].dy.is_empty());
        // El último `rotate` sigue para el resto, también en los descendientes.
        assert_eq!(s[0].rotate, vec![10.0, 20.0, 20.0]);
        assert_eq!((s[1].dx.clone(), s[1].dy.clone()), (vec![5.0], vec![3.0]));
        assert_eq!(s[1].rotate, vec![20.0]);
    }

    #[test]
    fn text_path_reference() {
        let t = text(
            "<path id='p' d='M0,0 h100' transform='translate(0,10)'/>\
             <text>A<textPath href='#p' startOffset='50%' side='right'>BC</textPath>D</text>",
        );
        assert_eq!(t.paths.len(), 1);
        assert_eq!(
            t.paths[0].segs,
            vec![Seg::MoveTo(100.0, 10.0), Seg::LineTo(0.0, 10.0)]
        );
        assert_eq!(t.paths[0].start_offset, 50.0);
        let chunks: Vec<_> = t
            .chunks
            .iter()
            .map(|c| (c.spans[0].text.as_str(), c.path, c.x, c.y))
            .collect();
        assert_eq!(
            chunks,
            vec![
                ("A", None, Some(0.0), Some(0.0)),
                ("BC", Some(0), Some(0.0), None),
                ("D", None, None, None),
            ]
        );
        // `pathLength` escala las longitudes de `startOffset`.
        let t = text(
            "<path id='p' d='M0,0 h200' pathLength='100'/>\
             <text><textPath href='#p' startOffset='10'>x</textPath></text>",
        );
        assert!((t.paths[0].start_offset - 20.0).abs() < 1e-3);
        // `path` manda sobre `href` si es válido; si no, se usa `href`.
        let t = text(
            "<path id='p' d='M0,0 h200'/>\
             <text><textPath href='#p' path='M0,5 h10'>x</textPath></text>",
        );
        assert_eq!(t.paths[0].segs, parse_path("M0,5 h10"));
        let t = text(
            "<path id='p' d='M0,0 h200'/>\
             <text><textPath href='#p' path='nada'>x</textPath></text>",
        );
        assert_eq!(t.paths[0].segs, parse_path("M0,0 h200"));
        // Sin camino válido, el contenido del `textPath` no se pinta.
        let t = text("<text>a<textPath href='#nada'>x</textPath></text>");
        assert!(t.paths.is_empty());
        assert_eq!(t.chunks.len(), 1);
    }

    #[test]
    fn bidi_override_and_css_path_length() {
        // `RLO` invierte hasta `PDF`; los controles no quedan en el texto.
        let t = text("<text>a\u{202e}bcd\u{202c}e\u{200f}</text>");
        assert_eq!(t.chunks[0].spans[0].text, "adcbe");
        let t = text(
            "<path id='p' d='M0,0 h200' pathLength='1' style='path-length: 10px'/>\
             <text><textPath href='#p' startOffset='5'>x</textPath></text>",
        );
        assert!((t.paths[0].start_offset - 100.0).abs() < 1e-3);
    }

    #[test]
    fn spacing_baselines_and_font_shorthand() {
        let t = text(
            "<text dominant-baseline='hanging' letter-spacing='2' word-spacing='10%' \
             style='font: italic bold 20px/2 monospace'>a<tspan alignment-baseline='central' \
             baseline-shift='super'>b</tspan></text>",
        );
        let s = &t.chunks[0].spans;
        assert_eq!(s[0].baseline, Baseline::Hanging);
        assert_eq!((s[0].letter_spacing, s[0].word_spacing), (2.0, 2.0));
        assert_eq!((s[0].size, s[0].weight, s[0].italic), (20.0, 700.0, true));
        assert_eq!(s[0].family, vec!["monospace".to_owned()]);
        assert_eq!(s[1].baseline, Baseline::Central);
        assert!((s[1].baseline_shift - 20.0 / 3.0).abs() < 1e-4);
    }

    #[test]
    fn inline_size_keeps_one_chunk() {
        // §11.4.1: con `inline-size` solo cuentan la primera `x` e `y`; los porcentajes son
        // del ancho del viewport.
        let t = text(
            "<text x='10' y='20' style='inline-size:50%;line-height:1.5;font-size:10px'>a \
             <tspan x='70' y='5'>b</tspan></text>",
        );
        assert_eq!(t.inline_size, Some(50.0));
        assert_eq!(t.chunks.len(), 1);
        assert_eq!((t.chunks[0].x, t.chunks[0].y), (Some(10.0), Some(20.0)));
        assert_eq!(t.chunks[0].spans[0].line_height, Some(15.0));
        let t = text("<text style='inline-size:0'>a</text>");
        assert_eq!(t.inline_size, None);
        assert_eq!(t.chunks[0].spans[0].line_height, None);
    }
}
