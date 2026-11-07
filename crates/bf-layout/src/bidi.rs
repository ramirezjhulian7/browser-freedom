//! UAX #9, Unicode Bidirectional Algorithm (revisión de Unicode 17.0.0).
//!
//! - §3.3.1 P1–P3: párrafos y nivel de párrafo.
//! - §3.3.2 X1–X8: niveles explícitos (incrustaciones, overrides y aislados) con la pila de
//!   estados direccionales de profundidad 125 (BD2).
//! - §3.3.3 X9: se quitan las incrustaciones, overrides, PDF y BN.
//! - §3.3.4 X10: secuencias de runs aislantes (BD13) con sus sos/eos.
//! - §3.3.4 W1–W7, §3.3.5 N0–N2 (con pares de corchetes, BD14–BD16) e §3.3.6 I1–I2.
//! - §3.4 L1 (niveles al final de la línea) y L2 (reordenado visual).
//!
//! Validado con `BidiTest.txt` y `BidiCharacterTest.txt` de la UCD 17.0.0
//! (`tests/data/`, test `bidi_conformance`).

use crate::unicode_tables::{BIDI_BRACKETS, BIDI_CLASS};

/// Bidi_Class (UAX #9 §3.2, tabla 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BidiClass {
    L,
    R,
    AL,
    EN,
    ES,
    ET,
    AN,
    CS,
    NSM,
    BN,
    B,
    S,
    WS,
    ON,
    LRE,
    LRO,
    RLE,
    RLO,
    PDF,
    LRI,
    RLI,
    FSI,
    PDI,
}

use BidiClass::*;

impl BidiClass {
    /// Nombre corto de la UCD (`"L"`, `"NSM"`…).
    pub fn from_name(s: &str) -> Option<BidiClass> {
        Some(match s {
            "L" => L,
            "R" => R,
            "AL" => AL,
            "EN" => EN,
            "ES" => ES,
            "ET" => ET,
            "AN" => AN,
            "CS" => CS,
            "NSM" => NSM,
            "BN" => BN,
            "B" => B,
            "S" => S,
            "WS" => WS,
            "ON" => ON,
            "LRE" => LRE,
            "LRO" => LRO,
            "RLE" => RLE,
            "RLO" => RLO,
            "PDF" => PDF,
            "LRI" => LRI,
            "RLI" => RLI,
            "FSI" => FSI,
            "PDI" => PDI,
            _ => return None,
        })
    }

    fn is_isolate_initiator(self) -> bool {
        matches!(self, LRI | RLI | FSI)
    }

    /// X9: las que el algoritmo quita.
    fn removed_by_x9(self) -> bool {
        matches!(self, RLE | LRE | RLO | LRO | PDF | BN)
    }

    /// NI de BD1 (neutros y aislados), después de W1–W7.
    fn is_ni(self) -> bool {
        matches!(self, B | S | WS | ON | LRI | RLI | FSI | PDI)
    }

    /// Dirección fuerte para N0–N2: EN y AN cuentan como R.
    fn strong_for_n(self) -> Option<BidiClass> {
        match self {
            L => Some(L),
            R | AL | EN | AN => Some(R),
            _ => None,
        }
    }
}

/// Bidi_Class de un carácter.
pub fn bidi_class(c: char) -> BidiClass {
    let cp = c as u32;
    match BIDI_CLASS.binary_search_by(|&(a, b, _)| {
        if cp < a {
            std::cmp::Ordering::Greater
        } else if cp > b {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Equal
        }
    }) {
        Ok(i) => BIDI_CLASS[i].2,
        Err(_) => L,
    }
}

/// Bidi_Paired_Bracket y si es de apertura (BidiBrackets.txt).
fn paired_bracket(c: u32) -> Option<(u32, bool)> {
    BIDI_BRACKETS
        .binary_search_by_key(&c, |&(cp, _, _)| cp)
        .ok()
        .map(|i| (BIDI_BRACKETS[i].1, BIDI_BRACKETS[i].2))
}

/// BD16: U+2329/U+232A son canónicamente equivalentes a U+3008/U+3009.
fn canonical_bracket(c: u32) -> u32 {
    match c {
        0x2329 => 0x3008,
        0x232A => 0x3009,
        c => c,
    }
}

/// BD2: profundidad máxima de incrustación.
pub const MAX_DEPTH: u8 = 125;

/// P2–P3: dirección del primer carácter fuerte de `classes` saltando el contenido de los
/// aislados (hasta su PDI). `stop_at_pdi`: para FSI, se detiene en el PDI que cierra el
/// aislado (o al final). Devuelve 1 si es R/AL, 0 si es L, `None` si no hay.
fn first_strong(classes: &[BidiClass], stop_at_pdi: bool) -> Option<u8> {
    let mut depth = 0usize;
    for &c in classes {
        match c {
            L if depth == 0 => return Some(0),
            R | AL if depth == 0 => return Some(1),
            LRI | RLI | FSI => depth += 1,
            PDI => {
                if depth > 0 {
                    depth -= 1;
                } else if stop_at_pdi {
                    return None;
                }
            }
            B => return None,
            _ => {}
        }
    }
    None
}

/// P2–P3: nivel de párrafo por defecto (0 si no hay carácter fuerte).
pub fn paragraph_level(classes: &[BidiClass]) -> u8 {
    first_strong(classes, false).unwrap_or(0)
}

/// Resultado de resolver un párrafo: el nivel del párrafo y los niveles de cada carácter
/// (`None` para los que quita X9). Los niveles son los de I1–I2, antes de L1.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub para_level: u8,
    pub levels: Vec<Option<u8>>,
}

/// Resuelve los niveles de un párrafo (P2–P3, X1–X10, W1–W7, N0–N2, I1–I2). `classes` no debe
/// contener B salvo, como mucho, el último carácter. `chars` (opcional, del mismo largo) da los
/// code points para los pares de corchetes de N0; sin él, N0 no encuentra pares.
/// `base`: el nivel del párrafo (0 o 1) o `None` para P2–P3.
pub fn resolve_paragraph(
    classes: &[BidiClass],
    chars: Option<&[u32]>,
    base: Option<u8>,
) -> Resolved {
    let n = classes.len();
    let para_level = base.unwrap_or_else(|| paragraph_level(classes));
    let mut types: Vec<BidiClass> = classes.to_vec();
    let mut levels: Vec<u8> = vec![para_level; n];

    // BD9: el PDI que corresponde a cada iniciador de aislado (y al revés).
    let mut matching_pdi: Vec<Option<usize>> = vec![None; n];
    let mut matched_initiator: Vec<bool> = vec![false; n];
    {
        let mut stack: Vec<usize> = Vec::new();
        for i in 0..n {
            match classes[i] {
                LRI | RLI | FSI => stack.push(i),
                PDI => {
                    if let Some(o) = stack.pop() {
                        matching_pdi[o] = Some(i);
                        matched_initiator[i] = true;
                    }
                }
                _ => {}
            }
        }
    }

    // X1–X8.
    #[derive(Clone, Copy)]
    struct Status {
        level: u8,
        override_: Option<BidiClass>,
        isolate: bool,
    }
    let mut stack: Vec<Status> = Vec::with_capacity(MAX_DEPTH as usize + 2);
    stack.push(Status {
        level: para_level,
        override_: None,
        isolate: false,
    });
    let mut overflow_isolates = 0usize;
    let mut overflow_embeddings = 0usize;
    let mut valid_isolates = 0usize;
    let next_odd = |l: u8| if l % 2 == 0 { l + 1 } else { l + 2 };
    let next_even = |l: u8| if l % 2 == 0 { l + 2 } else { l + 1 };
    for i in 0..n {
        let top = *stack.last().expect("pila no vacía");
        match classes[i] {
            // X2–X5.
            RLE | LRE | RLO | LRO => {
                let rtl = matches!(classes[i], RLE | RLO);
                let level = if rtl {
                    next_odd(top.level)
                } else {
                    next_even(top.level)
                };
                levels[i] = top.level;
                if level <= MAX_DEPTH && overflow_isolates == 0 && overflow_embeddings == 0 {
                    stack.push(Status {
                        level,
                        override_: match classes[i] {
                            RLO => Some(R),
                            LRO => Some(L),
                            _ => None,
                        },
                        isolate: false,
                    });
                } else if overflow_isolates == 0 {
                    overflow_embeddings += 1;
                }
            }
            // X5a–X5c.
            RLI | LRI | FSI => {
                levels[i] = top.level;
                if let Some(o) = top.override_ {
                    types[i] = o;
                }
                let rtl = match classes[i] {
                    RLI => true,
                    LRI => false,
                    _ => {
                        let end = matching_pdi[i].unwrap_or(n);
                        first_strong(&classes[i + 1..end], true) == Some(1)
                    }
                };
                let level = if rtl {
                    next_odd(top.level)
                } else {
                    next_even(top.level)
                };
                if level <= MAX_DEPTH && overflow_isolates == 0 && overflow_embeddings == 0 {
                    valid_isolates += 1;
                    stack.push(Status {
                        level,
                        override_: None,
                        isolate: true,
                    });
                } else {
                    overflow_isolates += 1;
                }
            }
            // X6a.
            PDI => {
                if overflow_isolates > 0 {
                    overflow_isolates -= 1;
                } else if valid_isolates > 0 {
                    overflow_embeddings = 0;
                    while stack.last().is_some_and(|s| !s.isolate) {
                        stack.pop();
                    }
                    stack.pop();
                    valid_isolates -= 1;
                }
                let top = *stack.last().expect("pila no vacía");
                levels[i] = top.level;
                if let Some(o) = top.override_ {
                    types[i] = o;
                }
            }
            // X7.
            PDF => {
                levels[i] = top.level;
                if overflow_isolates > 0 {
                } else if overflow_embeddings > 0 {
                    overflow_embeddings -= 1;
                } else if !top.isolate && stack.len() >= 2 {
                    stack.pop();
                }
            }
            // X8.
            B => levels[i] = para_level,
            BN => levels[i] = top.level,
            // X6.
            _ => {
                levels[i] = top.level;
                if let Some(o) = top.override_ {
                    types[i] = o;
                }
            }
        }
    }

    // X9: índices que siguen en el algoritmo.
    let kept: Vec<usize> = (0..n).filter(|&i| !classes[i].removed_by_x9()).collect();

    // X10 / BD13: runs de nivel y secuencias de runs aislantes.
    let mut runs: Vec<Vec<usize>> = Vec::new();
    for &i in &kept {
        match runs.last_mut() {
            Some(r) if levels[*r.last().expect("run no vacío")] == levels[i] => r.push(i),
            _ => runs.push(vec![i]),
        }
    }
    let mut run_of: Vec<usize> = vec![usize::MAX; n];
    for (k, r) in runs.iter().enumerate() {
        for &i in r {
            run_of[i] = k;
        }
    }
    let mut sequences: Vec<Vec<usize>> = Vec::new();
    for r in &runs {
        let first = r[0];
        if classes[first] == PDI && matched_initiator[first] {
            continue;
        }
        let mut seq = r.clone();
        loop {
            let last = *seq.last().expect("secuencia no vacía");
            if classes[last].is_isolate_initiator()
                && let Some(pdi) = matching_pdi[last]
                && run_of[pdi] != usize::MAX
            {
                seq.extend_from_slice(&runs[run_of[pdi]]);
            } else {
                break;
            }
        }
        sequences.push(seq);
    }

    // Posición en `kept` para buscar vecinos ignorando lo que quitó X9.
    let mut kept_pos: Vec<usize> = vec![usize::MAX; n];
    for (p, &i) in kept.iter().enumerate() {
        kept_pos[i] = p;
    }

    for seq in &sequences {
        let level = levels[seq[0]];
        let first = seq[0];
        let last = *seq.last().expect("secuencia no vacía");
        let before = if kept_pos[first] > 0 {
            levels[kept[kept_pos[first] - 1]]
        } else {
            para_level
        };
        let after = if classes[last].is_isolate_initiator() {
            para_level
        } else if kept_pos[last] + 1 < kept.len() {
            levels[kept[kept_pos[last] + 1]]
        } else {
            para_level
        };
        let dir = |l: u8| if l % 2 == 0 { L } else { R };
        let sos = dir(level.max(before));
        let eos = dir(level.max(after));
        resolve_sequence(seq, &mut types, classes, chars, level, sos, eos);
    }

    // I1–I2.
    for &i in &kept {
        let l = levels[i];
        levels[i] = if l % 2 == 0 {
            match types[i] {
                R => l + 1,
                AN | EN => l + 2,
                _ => l,
            }
        } else {
            match types[i] {
                L | EN | AN => l + 1,
                _ => l,
            }
        };
    }

    let mut out = vec![None; n];
    for &i in &kept {
        out[i] = Some(levels[i]);
    }
    Resolved {
        para_level,
        levels: out,
    }
}

/// W1–W7, N0–N2 sobre una secuencia de runs aislantes.
fn resolve_sequence(
    seq: &[usize],
    types: &mut [BidiClass],
    original: &[BidiClass],
    chars: Option<&[u32]>,
    level: u8,
    sos: BidiClass,
    eos: BidiClass,
) {
    let len = seq.len();
    let t = |types: &[BidiClass], k: usize| types[seq[k]];

    // W1.
    for k in 0..len {
        if t(types, k) == NSM {
            types[seq[k]] = if k == 0 {
                sos
            } else {
                match t(types, k - 1) {
                    LRI | RLI | FSI | PDI => ON,
                    p => p,
                }
            };
        }
    }
    // W2 (y W3 a la vez: el último fuerte se recuerda antes de cambiar AL).
    let mut last_strong = sos;
    for k in 0..len {
        match t(types, k) {
            c @ (L | R | AL) => last_strong = c,
            EN if last_strong == AL => types[seq[k]] = AN,
            _ => {}
        }
    }
    // W3.
    for k in 0..len {
        if t(types, k) == AL {
            types[seq[k]] = R;
        }
    }
    // W4.
    for k in 1..len.saturating_sub(1) {
        let (p, c, nx) = (t(types, k - 1), t(types, k), t(types, k + 1));
        if c == ES && p == EN && nx == EN {
            types[seq[k]] = EN;
        } else if c == CS && p == nx && matches!(p, EN | AN) {
            types[seq[k]] = p;
        }
    }
    // W5.
    let mut k = 0;
    while k < len {
        if t(types, k) == ET {
            let start = k;
            while k < len && t(types, k) == ET {
                k += 1;
            }
            let touches =
                (start > 0 && t(types, start - 1) == EN) || (k < len && t(types, k) == EN);
            if touches {
                for j in start..k {
                    types[seq[j]] = EN;
                }
            }
        } else {
            k += 1;
        }
    }
    // W6.
    for k in 0..len {
        if matches!(t(types, k), ES | ET | CS) {
            types[seq[k]] = ON;
        }
    }
    // W7.
    let mut last_strong = sos;
    for k in 0..len {
        match t(types, k) {
            c @ (L | R) => last_strong = c,
            EN if last_strong == L => types[seq[k]] = L,
            _ => {}
        }
    }

    let e = if level % 2 == 0 { L } else { R };

    // N0: pares de corchetes (BD16).
    if let Some(chars) = chars {
        let mut pairs: Vec<(usize, usize)> = Vec::new();
        let mut stack: Vec<(u32, usize)> = Vec::new();
        'scan: for k in 0..len {
            if t(types, k) != ON {
                continue;
            }
            let c = chars[seq[k]];
            let Some((pair, open)) = paired_bracket(c) else {
                continue;
            };
            if open {
                if stack.len() == 63 {
                    break 'scan;
                }
                stack.push((canonical_bracket(pair), k));
            } else {
                let me = canonical_bracket(c);
                if let Some(pos) = stack.iter().rposition(|&(p, _)| p == me) {
                    pairs.push((stack[pos].1, k));
                    stack.truncate(pos);
                }
            }
        }
        pairs.sort_unstable();
        for (o, c) in pairs {
            let mut found_e = false;
            let mut found_o = false;
            for k in o + 1..c {
                match t(types, k).strong_for_n() {
                    Some(d) if d == e => found_e = true,
                    Some(_) => found_o = true,
                    None => {}
                }
            }
            let new = if found_e {
                Some(e)
            } else if found_o {
                let mut ctx = sos;
                for k in (0..o).rev() {
                    if let Some(d) = t(types, k).strong_for_n() {
                        ctx = d;
                        break;
                    }
                }
                Some(if ctx != e { ctx } else { e })
            } else {
                None
            };
            if let Some(d) = new {
                for b in [o, c] {
                    types[seq[b]] = d;
                    let mut j = b + 1;
                    while j < len && original[seq[j]] == NSM {
                        types[seq[j]] = d;
                        j += 1;
                    }
                }
            }
        }
    }

    // N1–N2.
    let mut k = 0;
    while k < len {
        if !t(types, k).is_ni() {
            k += 1;
            continue;
        }
        let start = k;
        while k < len && t(types, k).is_ni() {
            k += 1;
        }
        let prev = if start == 0 {
            sos
        } else {
            t(types, start - 1).strong_for_n().unwrap_or(e)
        };
        let next = if k == len {
            eos
        } else {
            t(types, k).strong_for_n().unwrap_or(e)
        };
        let d = if prev == next { prev } else { e };
        for j in start..k {
            types[seq[j]] = d;
        }
    }
}

/// L1 sobre una línea (`classes` y `levels` de los caracteres de la línea, en orden lógico):
/// los separadores de segmento y de párrafo, los espacios y aislados que los preceden y los
/// del final de la línea vuelven al nivel del párrafo. Los que quitó X9 (`None`) son
/// transparentes.
pub fn apply_l1(classes: &[BidiClass], levels: &mut [Option<u8>], para_level: u8) {
    let ws_like = |c: BidiClass| matches!(c, WS | FSI | LRI | RLI | PDI) || c.removed_by_x9();
    // ¿Lo que sigue (hasta un S/B o el final de la línea) son solo espacios y aislados?
    let mut trailing = true;
    for i in (0..classes.len()).rev() {
        match classes[i] {
            S | B => {
                levels[i] = levels[i].map(|_| para_level);
                trailing = true;
            }
            c if ws_like(c) && trailing => levels[i] = levels[i].map(|_| para_level),
            _ => trailing = false,
        }
    }
}

/// L2: orden visual de una línea. `levels` son los niveles de sus elementos en orden lógico;
/// devuelve los índices lógicos en orden visual (de izquierda a derecha).
pub fn reorder(levels: &[u8]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..levels.len()).collect();
    let Some(&max) = levels.iter().max() else {
        return order;
    };
    let Some(min_odd) = levels.iter().copied().filter(|l| l % 2 == 1).min() else {
        return order;
    };
    let mut level = max;
    while level >= min_odd {
        let mut k = 0;
        while k < order.len() {
            if levels[order[k]] >= level {
                let start = k;
                while k < order.len() && levels[order[k]] >= level {
                    k += 1;
                }
                order[start..k].reverse();
            } else {
                k += 1;
            }
        }
        if level == 0 {
            break;
        }
        level -= 1;
    }
    order
}

/// Niveles de cada carácter de `text` (indexados por carácter), con P1: el texto se parte en
/// párrafos detrás de cada B. `base`: nivel de todos los párrafos o `None` para P2–P3 en cada
/// uno (`unicode-bidi: plaintext`). Los caracteres que quita X9 toman el nivel del carácter
/// anterior (o el del párrafo). No aplica L1.
pub fn text_levels(text: &[char], base: Option<u8>) -> Vec<u8> {
    text_levels_with_fallback(text, base, 0).0
}

/// Como [`text_levels`], pero con `base: None` un párrafo sin caracteres fuertes toma el nivel
/// del párrafo anterior, y el primero `fallback`, en lugar de 0 (UAX #9 HL1; CSS Writing Modes 3
/// §2.4.2, `unicode-bidi: plaintext`: `fallback` es el de `direction`, como en los navegadores,
/// WPT css-text/bidi/bidi-lines-002). Devuelve también el nivel de cada párrafo, en orden.
pub fn text_levels_with_fallback(
    text: &[char],
    base: Option<u8>,
    fallback: u8,
) -> (Vec<u8>, Vec<u8>) {
    let mut out = Vec::with_capacity(text.len());
    let mut paragraphs = Vec::new();
    let classes: Vec<BidiClass> = text.iter().map(|&c| bidi_class(c)).collect();
    let cps: Vec<u32> = text.iter().map(|&c| c as u32).collect();
    let mut start = 0;
    while start < text.len() {
        let end = classes[start..]
            .iter()
            .position(|&c| c == B)
            .map_or(text.len(), |p| start + p + 1);
        let level = base.unwrap_or_else(|| {
            first_strong(&classes[start..end], false)
                .unwrap_or_else(|| paragraphs.last().copied().unwrap_or(fallback))
        });
        let r = resolve_paragraph(&classes[start..end], Some(&cps[start..end]), Some(level));
        paragraphs.push(r.para_level);
        let mut prev = r.para_level;
        for l in r.levels {
            let l = l.unwrap_or(prev);
            out.push(l);
            prev = l;
        }
        start = end;
    }
    (out, paragraphs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn levels_of(s: &str, base: Option<u8>) -> Vec<u8> {
        let chars: Vec<char> = s.chars().collect();
        text_levels(&chars, base)
    }

    #[test]
    fn hebrew_in_latin() {
        // "ab אב cd": las letras hebreas a nivel 1 en un párrafo LTR.
        assert_eq!(
            levels_of("ab \u{5D0}\u{5D1} cd", Some(0)),
            [0, 0, 0, 1, 1, 0, 0, 0]
        );
    }

    #[test]
    fn numbers_in_rtl() {
        // En un párrafo RTL, los números europeos van a nivel 2.
        assert_eq!(levels_of("\u{5D0} 12", None), [1, 1, 2, 2]);
    }

    #[test]
    fn reorder_reverses_odd_runs() {
        assert_eq!(reorder(&[0, 1, 1, 0]), [0, 2, 1, 3]);
        assert_eq!(reorder(&[1, 2, 2, 1]), [3, 1, 2, 0]);
    }

    #[test]
    fn brackets_n0() {
        // BidiCharacterTest: "a(b)" con un hebreo dentro de los corchetes en un párrafo RTL.
        let l = levels_of("\u{5D0}(a)", Some(1));
        assert_eq!(l, [1, 1, 2, 1]);
    }
}
