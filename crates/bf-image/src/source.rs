//! Imágenes adaptables: qué recurso carga un `<img>` según `srcset`, `sizes` y los `<source>`
//! de su `<picture>` (HTML §4.8.4.3, "select an image source").
//!
//! - [`parse_srcset`]: "parse a srcset attribute" (§4.8.4.3.10), con descriptores `x`, `w` y
//!   `h` (este último solo se valida).
//! - [`parse_sizes`]: "parse a sizes attribute" (§4.8.4.3.11), con condiciones de medios y
//!   longitudes (también `calc()`, `min()`, `max()` y `clamp()`).
//! - [`source_set`]: "update the source set" (§4.8.4.3.12), que recorre los `<source>` del
//!   `<picture>` saltando los que no cumplen su `media` o tienen un `type` sin soporte.
//! - [`select_image_source`]: elige un candidato para la densidad de la pantalla.
//!
//! La URL elegida se devuelve sin resolver: la resuelve quien la carga (contra la base del
//! documento). La densidad elegida sirve para el tamaño natural corregido (§4.8.4.4,
//! "density-corrected natural width and height"): `píxeles / densidad`.

use bf_css::media::{MediaContext, length_px, parse_media_query_list, parse_media_query_list_from};
use bf_css::syntax::{BlockKind, ComponentValue, TokenKind, parse_comma_separated_list};
use bf_dom::{Document, NodeId};

/// El descriptor de un candidato de `srcset`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Descriptor {
    /// Sin descriptor: equivale a `1x`.
    None,
    /// `<n>x`: densidad de píxeles.
    Density(f64),
    /// `<n>w`: ancho en píxeles del recurso; la densidad sale de `sizes`.
    Width(u32),
}

/// Un candidato de `srcset`, con la URL tal como está escrita.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub url: String,
    pub descriptor: Descriptor,
}

/// Una fuente de imagen del *source set*, ya normalizada: URL sin resolver y densidad.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageSource {
    pub url: String,
    pub density: f64,
}

/// ASCII whitespace (Infra): TAB, LF, FF, CR y espacio (no VT).
fn is_ws(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' ')
}

/// "Parse a srcset attribute" (HTML §4.8.4.3.10). Los candidatos con descriptores inválidos
/// se descartan (error de parseo).
pub fn parse_srcset(input: &str) -> Vec<Candidate> {
    let s: Vec<char> = input.chars().collect();
    let mut pos = 0;
    let mut out = Vec::new();
    loop {
        // Splitting loop.
        while pos < s.len() && (is_ws(s[pos]) || s[pos] == ',') {
            pos += 1;
        }
        if pos >= s.len() {
            return out;
        }
        let start = pos;
        while pos < s.len() && !is_ws(s[pos]) {
            pos += 1;
        }
        let mut url: String = s[start..pos].iter().collect();
        let mut descriptors = Vec::new();
        if url.ends_with(',') {
            // Las comas finales cierran el candidato: no tiene descriptores.
            let len = url.trim_end_matches(',').len();
            url.truncate(len);
        } else {
            tokenize_descriptors(&s, &mut pos, &mut descriptors);
        }
        if let Some(descriptor) = parse_descriptors(&descriptors) {
            out.push(Candidate { url, descriptor });
        }
    }
}

/// El "descriptor tokenizer" de §4.8.4.3.10: separa por espacios hasta la coma que cierra el
/// candidato (las comas dentro de paréntesis no cuentan).
fn tokenize_descriptors(s: &[char], pos: &mut usize, out: &mut Vec<String>) {
    enum State {
        InDescriptor,
        InParens,
        AfterDescriptor,
    }
    while *pos < s.len() && is_ws(s[*pos]) {
        *pos += 1;
    }
    let mut current = String::new();
    let mut state = State::InDescriptor;
    loop {
        let c = s.get(*pos).copied();
        match state {
            State::InDescriptor => match c {
                Some(c) if is_ws(c) => {
                    if !current.is_empty() {
                        out.push(std::mem::take(&mut current));
                    }
                    state = State::AfterDescriptor;
                }
                Some(',') => {
                    *pos += 1;
                    if !current.is_empty() {
                        out.push(current);
                    }
                    return;
                }
                Some('(') => {
                    current.push('(');
                    state = State::InParens;
                }
                None => {
                    if !current.is_empty() {
                        out.push(current);
                    }
                    return;
                }
                Some(c) => current.push(c),
            },
            State::InParens => match c {
                Some(')') => {
                    current.push(')');
                    state = State::InDescriptor;
                }
                None => {
                    out.push(current);
                    return;
                }
                Some(c) => current.push(c),
            },
            State::AfterDescriptor => match c {
                Some(c) if is_ws(c) => {}
                None => return,
                Some(_) => {
                    // Se vuelve a consumir el carácter en el estado "in descriptor".
                    state = State::InDescriptor;
                    continue;
                }
            },
        }
        *pos += 1;
    }
}

/// "Valid non-negative integer" seguido de su valor (saturado a `u32`).
fn non_negative_integer(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(
        s.parse::<u64>()
            .map_or(u32::MAX, |v| v.min(u64::from(u32::MAX)) as u32),
    )
}

/// "Valid floating-point number" (HTML §2.3.4.3) seguido de su valor: `-`?, dígitos y/o `.`
/// con dígitos, y exponente opcional. Sin `+` inicial, sin `Infinity` ni `NaN`.
fn floating_point(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let mut i = usize::from(b.first() == Some(&b'-'));
    let digits = |i: &mut usize| {
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i - start
    };
    let int = digits(&mut i);
    let mut frac = 0;
    if b.get(i) == Some(&b'.') {
        i += 1;
        frac = digits(&mut i);
        if frac == 0 {
            return None;
        }
    }
    if int == 0 && frac == 0 {
        return None;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'-' | b'+')) {
            i += 1;
        }
        if digits(&mut i) == 0 {
            return None;
        }
    }
    if i != b.len() {
        return None;
    }
    s.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// El "descriptor parser" de §4.8.4.3.10. `None` si hay error.
fn parse_descriptors(descriptors: &[String]) -> Option<Descriptor> {
    let (mut width, mut density, mut height) = (None, None, None);
    for d in descriptors {
        let last = d.chars().next_back()?;
        let value = &d[..d.len() - last.len_utf8()];
        match last {
            'w' => {
                if width.is_some() || density.is_some() {
                    return None;
                }
                width = Some(non_negative_integer(value).filter(|&v| v != 0)?);
            }
            'x' => {
                if width.is_some() || density.is_some() || height.is_some() {
                    return None;
                }
                // `-0x` es válido: solo se rechazan los negativos de verdad.
                density = Some(floating_point(value).filter(|&v| v >= 0.0)?);
            }
            'h' => {
                if height.is_some() || density.is_some() {
                    return None;
                }
                height = Some(non_negative_integer(value).filter(|&v| v != 0)?);
            }
            _ => return None,
        }
    }
    // `h` es solo para compatibilidad futura y necesita un `w`.
    if height.is_some() && width.is_none() {
        return None;
    }
    Some(match (width, density) {
        (Some(w), _) => Descriptor::Width(w),
        (None, Some(d)) => Descriptor::Density(d),
        (None, None) => Descriptor::None,
    })
}

/// "Parse a sizes attribute" (HTML §4.8.4.3.11): el tamaño de fuente, en px CSS, de la primera
/// entrada cuya condición de medios se cumple; `100vw` si ninguna sirve. Las entradas con
/// longitud o condición inválidas se saltan. `auto` (que solo vale con `loading=lazy`, sin
/// soporte aquí) se trata como inválido, y gana la entrada siguiente.
pub fn parse_sizes(input: &str, ctx: &MediaContext) -> f64 {
    let ws = |v: &ComponentValue| v.is_whitespace();
    for mut unparsed in parse_comma_separated_list(input) {
        while unparsed.last().is_some_and(ws) {
            unparsed.pop();
        }
        let Some(last) = unparsed.pop() else {
            continue;
        };
        let Some(size) = source_size_value(&last, ctx) else {
            continue;
        };
        while unparsed.last().is_some_and(ws) {
            unparsed.pop();
        }
        if unparsed.is_empty() || media_condition_matches(&unparsed, ctx) == Some(true) {
            return size;
        }
    }
    ctx.width
}

/// Evalúa una `<media-condition>` (sin tipo de medio). `None` si no es una condición válida.
fn media_condition_matches(values: &[ComponentValue], ctx: &MediaContext) -> Option<bool> {
    let list = parse_media_query_list_from(values);
    match list.0.as_slice() {
        [q] if q.media_type.is_none() && q.condition.is_some() => Some(q.evaluate(ctx)),
        _ => None,
    }
}

/// Un `<source-size-value>` no negativo, en px: una longitud (sin porcentajes; `0` sin
/// unidad vale) o una función matemática, que se recorta a 0 si da negativa.
fn source_size_value(v: &ComponentValue, ctx: &MediaContext) -> Option<f64> {
    match v {
        ComponentValue::Token(t) => match &t.kind {
            TokenKind::Dimension(n, unit) => {
                length_px(n.value, unit, ctx).filter(|&px| px >= 0.0 && px.is_finite())
            }
            TokenKind::Number(n) if n.value == 0.0 => Some(0.0),
            _ => None,
        },
        ComponentValue::Function(f) if is_math_function(&f.name) => match math(v, ctx)? {
            Calc::Length(px) if px.is_finite() => Some(px.max(0.0)),
            _ => None,
        },
        _ => None,
    }
}

fn is_math_function(name: &str) -> bool {
    ["calc", "min", "max", "clamp"]
        .iter()
        .any(|n| name.eq_ignore_ascii_case(n))
}

/// Resultado parcial de una expresión matemática (CSS Values 4 §10).
#[derive(Clone, Copy)]
enum Calc {
    Number(f64),
    Length(f64),
}

/// Evalúa un valor de una expresión: número, longitud, paréntesis o función matemática.
fn math(v: &ComponentValue, ctx: &MediaContext) -> Option<Calc> {
    match v {
        ComponentValue::Token(t) => match &t.kind {
            TokenKind::Number(n) => Some(Calc::Number(n.value)),
            TokenKind::Dimension(n, unit) => length_px(n.value, unit, ctx).map(Calc::Length),
            _ => None,
        },
        ComponentValue::Block(b) if b.kind == BlockKind::Paren => sum(&b.value, ctx),
        ComponentValue::Function(f) => {
            let name = f.name.to_ascii_lowercase();
            if name == "calc" {
                return sum(&f.value, ctx);
            }
            let args = bf_css::syntax::split_comma_separated(&f.value)
                .iter()
                .map(|a| sum(a, ctx))
                .collect::<Option<Vec<_>>>()?;
            let lengths = args
                .iter()
                .map(|a| match a {
                    Calc::Length(px) => Some(*px),
                    Calc::Number(_) => None,
                })
                .collect::<Option<Vec<_>>>()?;
            match (name.as_str(), lengths.as_slice()) {
                ("min", [_, ..]) => Some(Calc::Length(
                    lengths.iter().copied().fold(f64::INFINITY, f64::min),
                )),
                ("max", [_, ..]) => Some(Calc::Length(
                    lengths.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                )),
                ("clamp", [lo, v, hi]) => Some(Calc::Length(v.min(*hi).max(*lo))),
                _ => None,
            }
        }
        _ => None,
    }
}

/// `<calc-sum>`: términos separados por `+` y `-` (con espacios alrededor, que el tokenizador
/// ya distingue del signo de un número).
fn sum(values: &[ComponentValue], ctx: &MediaContext) -> Option<Calc> {
    let items: Vec<&ComponentValue> = values.iter().filter(|v| !v.is_whitespace()).collect();
    let mut parts = items.split(|v| matches!(v.token(), Some(TokenKind::Delim('+' | '-'))));
    let ops = items
        .iter()
        .filter_map(|v| match v.token() {
            Some(TokenKind::Delim(c @ ('+' | '-'))) => Some(*c),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut acc = product(parts.next()?, ctx)?;
    for (op, part) in ops.into_iter().zip(parts) {
        let rhs = product(part, ctx)?;
        let sign = if op == '-' { -1.0 } else { 1.0 };
        acc = match (acc, rhs) {
            (Calc::Number(a), Calc::Number(b)) => Calc::Number(a + sign * b),
            (Calc::Length(a), Calc::Length(b)) => Calc::Length(a + sign * b),
            _ => return None,
        };
    }
    Some(acc)
}

/// `<calc-product>`: valores separados por `*` y `/`.
fn product(items: &[&ComponentValue], ctx: &MediaContext) -> Option<Calc> {
    let mut it = items.iter();
    let mut acc = math(it.next()?, ctx)?;
    while let Some(op) = it.next() {
        let rhs = math(it.next()?, ctx)?;
        acc = match (op.token(), acc, rhs) {
            (Some(TokenKind::Delim('*')), Calc::Number(a), Calc::Number(b)) => Calc::Number(a * b),
            (Some(TokenKind::Delim('*')), Calc::Number(a), Calc::Length(b))
            | (Some(TokenKind::Delim('*')), Calc::Length(b), Calc::Number(a)) => {
                Calc::Length(a * b)
            }
            (Some(TokenKind::Delim('/')), Calc::Number(a), Calc::Number(b)) if b != 0.0 => {
                Calc::Number(a / b)
            }
            (Some(TokenKind::Delim('/')), Calc::Length(a), Calc::Number(b)) if b != 0.0 => {
                Calc::Length(a / b)
            }
            _ => return None,
        };
    }
    Some(acc)
}

/// ¿Es `mime` un tipo de imagen que `bf-image` (o `bf-svg`) sabe mostrar? Para el `type` de
/// `<source>` (HTML §4.8.4.3.12: un tipo desconocido o sin soporte salta el `<source>`). Los
/// parámetros (`;…`) no cuentan.
pub fn supports_type(mime: &str) -> bool {
    let essence = mime.split(';').next().unwrap_or("").trim();
    [
        "image/png",
        "image/apng",
        "image/jpeg",
        "image/jpg",
        "image/pjpeg",
        "image/gif",
        "image/webp",
        "image/bmp",
        "image/x-bmp",
        "image/x-ms-bmp",
        "image/x-icon",
        "image/vnd.microsoft.icon",
        "image/svg+xml",
    ]
    .iter()
    .any(|t| essence.eq_ignore_ascii_case(t))
}

/// "Update the source set" (HTML §4.8.4.3.12) de un `<img>`, ya normalizado (§4.8.4.3.13): si
/// está en un `<picture>`, el primer `<source>` anterior con `srcset` no vacío, `media` que se
/// cumple y `type` con soporte; si no, su propio `srcset` más su `src` (como `1x`, si no hay
/// ya un `1x` ni descriptores `w`).
pub fn source_set(doc: &Document, img: NodeId, ctx: &MediaContext) -> Vec<ImageSource> {
    let Some(el) = doc.element(img) else {
        return Vec::new();
    };
    let picture = doc
        .parent(img)
        .filter(|&p| doc.element(p).is_some_and(|e| e.is_html("picture")));
    if let Some(picture) = picture {
        for child in doc.children(picture) {
            if child == img {
                break;
            }
            let Some(source) = doc.element(child).filter(|e| e.is_html("source")) else {
                continue;
            };
            let Some(srcset) = source.attr("srcset") else {
                continue;
            };
            let candidates = parse_srcset(srcset);
            if candidates.is_empty() {
                continue;
            }
            if source
                .attr("media")
                .is_some_and(|m| !parse_media_query_list(m).evaluate(ctx))
            {
                continue;
            }
            if source.attr("type").is_some_and(|t| !supports_type(t)) {
                continue;
            }
            let sizes = parse_sizes(source.attr("sizes").unwrap_or(""), ctx);
            return normalize(candidates, sizes);
        }
    }
    let mut candidates = el.attr("srcset").map(parse_srcset).unwrap_or_default();
    let sizes = parse_sizes(el.attr("sizes").unwrap_or(""), ctx);
    if let Some(src) = el.attr("src").filter(|s| !s.is_empty()) {
        let has_1x_or_w = candidates.iter().any(|c| match c.descriptor {
            Descriptor::None | Descriptor::Width(_) => true,
            Descriptor::Density(d) => d == 1.0,
        });
        if !has_1x_or_w {
            candidates.push(Candidate {
                url: src.to_owned(),
                descriptor: Descriptor::None,
            });
        }
    }
    normalize(candidates, sizes)
}

/// "Normalize the source densities" (§4.8.4.3.13): un `w` da `ancho / sizes`; sin descriptor,
/// `1x`.
fn normalize(candidates: Vec<Candidate>, sizes: f64) -> Vec<ImageSource> {
    candidates
        .into_iter()
        .map(|c| ImageSource {
            density: match c.descriptor {
                Descriptor::None => 1.0,
                Descriptor::Density(d) => d,
                Descriptor::Width(w) => f64::from(w) / sizes,
            },
            url: c.url,
        })
        .collect()
}

/// "Select an image source" (§4.8.4.3.12): del *source set* sin densidades repetidas (gana la
/// primera), la de menor densidad que alcanza `ctx.device_pixel_ratio`; si ninguna llega, la
/// de más densidad. `None` si no hay candidatos (ni `srcset` ni `src`).
pub fn select_image_source(doc: &Document, img: NodeId, ctx: &MediaContext) -> Option<ImageSource> {
    choose(source_set(doc, img, ctx), ctx.device_pixel_ratio)
}

/// La elección de [`select_image_source`] sobre un *source set* ya hecho.
pub fn choose(set: Vec<ImageSource>, device_pixel_ratio: f64) -> Option<ImageSource> {
    let mut unique: Vec<ImageSource> = Vec::with_capacity(set.len());
    for s in set {
        if !unique.iter().any(|u| u.density == s.density) {
            unique.push(s);
        }
    }
    let enough = unique
        .iter()
        .filter(|s| s.density >= device_pixel_ratio)
        .min_by(|a, b| a.density.total_cmp(&b.density));
    let best = enough.or_else(|| unique.iter().max_by(|a, b| a.density.total_cmp(&b.density)));
    best.cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(s: &str) -> Vec<(String, Descriptor)> {
        parse_srcset(s)
            .into_iter()
            .map(|c| (c.url, c.descriptor))
            .collect()
    }

    #[test]
    fn srcset_descriptors() {
        assert_eq!(
            urls("a.png 1x, b.png 2x"),
            [
                ("a.png".into(), Descriptor::Density(1.0)),
                ("b.png".into(), Descriptor::Density(2.0))
            ]
        );
        assert_eq!(
            urls("a 100w,b 200w 50h"),
            [
                ("a".into(), Descriptor::Width(100)),
                ("b".into(), Descriptor::Width(200))
            ]
        );
        assert_eq!(urls("a,,,"), [("a".into(), Descriptor::None)]);
        // Las comas dentro de paréntesis no cierran el candidato (el descriptor es inválido).
        assert_eq!(urls("a ( , b 1x, ), c"), [("c".into(), Descriptor::None)]);
        for bad in [
            "a 1x 1x",
            "a 0w",
            "a 1w 1x",
            "a +1x",
            "a 1.x",
            "a Infinityx",
            "a 1h",
        ] {
            assert!(urls(bad).is_empty(), "{bad}");
        }
        assert_eq!(urls("a -0x"), [("a".into(), Descriptor::Density(-0.0))]);
        assert_eq!(urls("a .5x"), [("a".into(), Descriptor::Density(0.5))]);
        assert_eq!(urls("a 1e-1x"), [("a".into(), Descriptor::Density(0.1))]);
    }

    #[test]
    fn sizes_lists() {
        let ctx = MediaContext {
            width: 800.0,
            height: 600.0,
            ..MediaContext::default()
        };
        assert_eq!(parse_sizes("", &ctx), 800.0);
        assert_eq!(parse_sizes("50vw", &ctx), 400.0);
        assert_eq!(parse_sizes("(max-width: 500px) 100px, 300px", &ctx), 300.0);
        assert_eq!(parse_sizes("(min-width: 500px) 100px, 300px", &ctx), 100.0);
        assert_eq!(parse_sizes("calc(50vw - 2em)", &ctx), 368.0);
        assert_eq!(parse_sizes("min(10px, 5em)", &ctx), 10.0);
        // Inválidas: porcentaje, negativo, tipo de medio; se pasa a la siguiente.
        assert_eq!(parse_sizes("50%, -1px, screen 3px, 7px", &ctx), 7.0);
        assert_eq!(parse_sizes("auto, 20px", &ctx), 20.0);
        assert_eq!(parse_sizes("0", &ctx), 0.0);
    }

    #[test]
    fn choice_by_density() {
        let set = |ds: &[f64]| {
            ds.iter()
                .enumerate()
                .map(|(i, &d)| ImageSource {
                    url: i.to_string(),
                    density: d,
                })
                .collect::<Vec<_>>()
        };
        let pick = |ds: &[f64], dpr| choose(set(ds), dpr).map(|s| s.url);
        assert_eq!(pick(&[1.0, 2.0], 1.0).as_deref(), Some("0"));
        assert_eq!(pick(&[1.0, 2.0], 2.0).as_deref(), Some("1"));
        assert_eq!(pick(&[1.0, 2.0], 3.0).as_deref(), Some("1"));
        assert_eq!(pick(&[2.0, 1.0, 1.5], 1.25).as_deref(), Some("2"));
        // Densidades repetidas: gana la primera.
        assert_eq!(pick(&[1.0, 1.0], 1.0).as_deref(), Some("0"));
        assert_eq!(pick(&[], 1.0), None);
    }

    #[test]
    fn mime_types() {
        assert!(supports_type("image/webp"));
        assert!(supports_type("IMAGE/PNG; charset=x"));
        assert!(!supports_type("image/avif"));
        assert!(!supports_type("image/jxl"));
        assert!(!supports_type(""));
    }
}
