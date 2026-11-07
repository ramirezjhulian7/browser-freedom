//! UAX #14, Unicode Line Breaking Algorithm (revisión 55, Unicode 17.0.0): oportunidades de
//! corte de línea con las reglas por pares de §6.1 (no adaptables) y §6.2 (adaptables por
//! defecto), LB1–LB31.
//!
//! Validado con `LineBreakTest.txt` de la UCD 17.0.0 (`tests/data/`, test
//! `linebreak_conformance`).

use crate::unicode_tables::{LB_FLAGS, LINE_BREAK, LINE_BREAK_CLASSES};

/// Line_Break (UAX #14 §5.1, tabla 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineBreak {
    AI,
    AK,
    AL,
    AP,
    AS,
    B2,
    BA,
    BB,
    BK,
    CB,
    CJ,
    CL,
    CM,
    CP,
    CR,
    EB,
    EM,
    EX,
    GL,
    H2,
    H3,
    HH,
    HL,
    HY,
    ID,
    IN,
    IS,
    JL,
    JT,
    JV,
    LF,
    NL,
    NS,
    NU,
    OP,
    PO,
    PR,
    QU,
    RI,
    SA,
    SG,
    SP,
    SY,
    VF,
    VI,
    WJ,
    XX,
    ZW,
    ZWJ,
}

const _: () = assert!(LINE_BREAK_CLASSES == 49);

use LineBreak::*;

fn lookup<T: Copy>(table: &[(u32, u32, T)], c: char, default: T) -> T {
    let cp = c as u32;
    match table.binary_search_by(|&(a, b, _)| {
        if cp < a {
            std::cmp::Ordering::Greater
        } else if cp > b {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Equal
        }
    }) {
        Ok(i) => table[i].2,
        Err(_) => default,
    }
}

/// Line_Break de un carácter (sin resolver: puede ser AI, SA, CJ…).
pub fn line_break_class(c: char) -> LineBreak {
    lookup(LINE_BREAK, c, XX)
}

const EAST_ASIAN: u8 = 1;
const EXT_PICT: u8 = 2;
const PI: u8 = 4;
const PF: u8 = 8;
const CN: u8 = 16;
const MARK: u8 = 32;

fn flags(c: char) -> u8 {
    lookup(LB_FLAGS, c, 0)
}

/// LB1: resolución de AI, SG, XX, SA y CJ sin información externa.
pub fn resolve_class(c: char) -> LineBreak {
    match line_break_class(c) {
        AI | SG | XX => AL,
        SA if flags(c) & MARK != 0 => CM,
        SA => AL,
        CJ => NS,
        k => k,
    }
}

/// Qué hay en una posición entre dos caracteres.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Break {
    /// No se puede cortar.
    Prohibited,
    /// Oportunidad de corte.
    Allowed,
    /// Corte obligatorio (después de BK, CR, LF, NL).
    Mandatory,
}

/// Un carácter con su clase resuelta y sus propiedades auxiliares.
#[derive(Debug, Clone, Copy)]
struct Unit {
    class: LineBreak,
    flags: u8,
    dotted_circle: bool,
}

/// Oportunidades de corte de `chars` con las clases de LB1 por defecto. El resultado tiene
/// `chars.len() + 1` entradas: la `i` es la posición antes del carácter `i` (la 0 es sot,
/// siempre `Prohibited` por LB2, y la última eot, `Mandatory` por LB3).
pub fn line_breaks(chars: &[char]) -> Vec<Break> {
    line_breaks_with(chars, |_, c| resolve_class(c))
}

/// Como [`line_breaks`], con la clase resuelta de cada carácter dada por `class(i, c)` (para
/// adaptaciones como `word-break` de CSS Text 3).
pub fn line_breaks_with(chars: &[char], class: impl Fn(usize, char) -> LineBreak) -> Vec<Break> {
    let n = chars.len();
    let mut out = vec![Break::Prohibited; n + 1];
    if n == 0 {
        out[0] = Break::Mandatory;
        return out;
    }
    out[n] = Break::Mandatory;
    let raw: Vec<LineBreak> = (0..n).map(|i| class(i, chars[i])).collect();

    // LB9/LB10: las unidades (base + CM/ZWJ absorbidos). `units[k]` es el índice del carácter
    // base; `absorbed[i]` marca los CM/ZWJ unidos a la base anterior.
    let mut absorbed = vec![false; n];
    let mut units: Vec<usize> = Vec::with_capacity(n);
    let mut info: Vec<Unit> = Vec::with_capacity(n);
    for i in 0..n {
        let k = raw[i];
        if matches!(k, CM | ZWJ)
            && let Some(base) = info.last()
            && !matches!(base.class, BK | CR | LF | NL | SP | ZW)
        {
            absorbed[i] = true;
            continue;
        }
        units.push(i);
        let unit = if matches!(k, CM | ZWJ) {
            // LB10: como U+0041.
            Unit {
                class: AL,
                flags: 0,
                dotted_circle: false,
            }
        } else {
            Unit {
                class: k,
                flags: flags(chars[i]),
                dotted_circle: chars[i] == '\u{25CC}',
            }
        };
        info.push(unit);
    }

    // Posiciones dentro de una unidad (antes de un CM/ZWJ absorbido).
    for i in 1..n {
        if absorbed[i] {
            // LB4–LB8a se evalúan antes que LB9 con el carácter anterior real.
            out[i] = early(&raw, i).unwrap_or(Break::Prohibited);
        }
    }

    let m = units.len();
    let cls = |k: usize| info[k].class;
    let ea = |k: usize| info[k].flags & EAST_ASIAN != 0;
    for j in 1..m {
        let pos = units[j];
        if let Some(b) = early(&raw, pos) {
            out[pos] = b;
            continue;
        }
        let a = cls(j - 1);
        let b = cls(j);
        let prev = |k: usize| if k == 0 { None } else { Some(cls(k - 1)) };
        let next = |k: usize| if k + 1 < m { Some(cls(k + 1)) } else { None };
        // Unidad anterior a `k` saltando espacios (para las reglas con SP*).
        let skip_sp_back = |k: usize| -> Option<usize> {
            let mut p = k;
            loop {
                if cls(p) != SP {
                    return Some(p);
                }
                if p == 0 {
                    return None;
                }
                p -= 1;
            }
        };
        // Retrocede desde `k` sobre (SY | IS)* y devuelve la primera otra unidad.
        let skip_num_punct_back = |k: usize| -> Option<usize> {
            let mut p = k;
            loop {
                if !matches!(cls(p), SY | IS) {
                    return Some(p);
                }
                if p == 0 {
                    return None;
                }
                p -= 1;
            }
        };
        let akish = |k: usize| matches!(cls(k), AK | AS) || info[k].dotted_circle;

        let r = 'rules: {
            // LB11.
            if b == WJ || a == WJ {
                break 'rules Break::Prohibited;
            }
            // LB12.
            if a == GL {
                break 'rules Break::Prohibited;
            }
            // LB12a.
            if b == GL && !matches!(a, SP | BA | HY | HH) {
                break 'rules Break::Prohibited;
            }
            // LB13.
            if matches!(b, CL | CP | EX | SY) {
                break 'rules Break::Prohibited;
            }
            // LB14: OP SP* ×
            if let Some(p) = skip_sp_back(j - 1)
                && cls(p) == OP
            {
                break 'rules Break::Prohibited;
            }
            // LB15a: (sot | BK | CR | LF | NL | OP | QU | GL | SP | ZW) [\p{Pi}&QU] SP* ×
            if let Some(p) = skip_sp_back(j - 1)
                && cls(p) == QU
                && info[p].flags & PI != 0
                && prev(p).is_none_or(|c| matches!(c, BK | CR | LF | NL | OP | QU | GL | SP | ZW))
            {
                break 'rules Break::Prohibited;
            }
            // LB15b: × [\p{Pf}&QU] ( SP | GL | WJ | CL | QU | CP | EX | IS | SY | BK | CR | LF
            // | NL | ZW | eot)
            if b == QU
                && info[j].flags & PF != 0
                && next(j).is_none_or(|c| {
                    matches!(
                        c,
                        SP | GL | WJ | CL | QU | CP | EX | IS | SY | BK | CR | LF | NL | ZW
                    )
                })
            {
                break 'rules Break::Prohibited;
            }
            // LB15c: SP ÷ IS NU
            if a == SP && b == IS && next(j) == Some(NU) {
                break 'rules Break::Allowed;
            }
            // LB15d.
            if b == IS {
                break 'rules Break::Prohibited;
            }
            // LB16: (CL | CP) SP* × NS
            if b == NS
                && let Some(p) = skip_sp_back(j - 1)
                && matches!(cls(p), CL | CP)
            {
                break 'rules Break::Prohibited;
            }
            // LB17: B2 SP* × B2
            if b == B2
                && let Some(p) = skip_sp_back(j - 1)
                && cls(p) == B2
            {
                break 'rules Break::Prohibited;
            }
            // LB18.
            if a == SP {
                break 'rules Break::Allowed;
            }
            // LB19.
            if b == QU && info[j].flags & PI == 0 {
                break 'rules Break::Prohibited;
            }
            if a == QU && info[j - 1].flags & PF == 0 {
                break 'rules Break::Prohibited;
            }
            // LB19a.
            if b == QU && (!ea(j - 1) || j + 1 >= m || !ea(j + 1)) {
                break 'rules Break::Prohibited;
            }
            if a == QU && (!ea(j) || j < 2 || !ea(j - 2)) {
                break 'rules Break::Prohibited;
            }
            // LB20.
            if b == CB || a == CB {
                break 'rules Break::Allowed;
            }
            // LB20a: ( sot | BK | CR | LF | NL | SP | ZW | CB | GL ) ( HY | HH ) × ( AL | HL )
            if matches!(a, HY | HH)
                && matches!(b, AL | HL)
                && prev(j - 1).is_none_or(|c| matches!(c, BK | CR | LF | NL | SP | ZW | CB | GL))
            {
                break 'rules Break::Prohibited;
            }
            // LB21.
            if matches!(b, BA | HH | HY | NS) || a == BB {
                break 'rules Break::Prohibited;
            }
            // LB21a: HL (HY | HH) × [^HL]
            if matches!(a, HY | HH) && prev(j - 1) == Some(HL) && b != HL {
                break 'rules Break::Prohibited;
            }
            // LB21b.
            if a == SY && b == HL {
                break 'rules Break::Prohibited;
            }
            // LB22.
            if b == IN {
                break 'rules Break::Prohibited;
            }
            // LB23.
            if (matches!(a, AL | HL) && b == NU) || (a == NU && matches!(b, AL | HL)) {
                break 'rules Break::Prohibited;
            }
            // LB23a.
            if (a == PR && matches!(b, ID | EB | EM)) || (matches!(a, ID | EB | EM) && b == PO) {
                break 'rules Break::Prohibited;
            }
            // LB24.
            if (matches!(a, PR | PO) && matches!(b, AL | HL))
                || (matches!(a, AL | HL) && matches!(b, PR | PO))
            {
                break 'rules Break::Prohibited;
            }
            // LB25.
            if matches!(b, PO | PR) {
                // NU (SY | IS)* (CL | CP) × (PO | PR)
                if matches!(a, CL | CP)
                    && j >= 2
                    && skip_num_punct_back(j - 2).is_some_and(|p| cls(p) == NU)
                {
                    break 'rules Break::Prohibited;
                }
                // NU (SY | IS)* × (PO | PR)
                if skip_num_punct_back(j - 1).is_some_and(|p| cls(p) == NU) {
                    break 'rules Break::Prohibited;
                }
            }
            if matches!(a, PO | PR) {
                if b == NU {
                    break 'rules Break::Prohibited;
                }
                if b == OP
                    && (next(j) == Some(NU)
                        || (next(j) == Some(IS) && j + 2 < m && cls(j + 2) == NU))
                {
                    break 'rules Break::Prohibited;
                }
            }
            if matches!(a, HY | IS) && b == NU {
                break 'rules Break::Prohibited;
            }
            if b == NU && skip_num_punct_back(j - 1).is_some_and(|p| cls(p) == NU) {
                break 'rules Break::Prohibited;
            }
            // LB26.
            if (a == JL && matches!(b, JL | JV | H2 | H3))
                || (matches!(a, JV | H2) && matches!(b, JV | JT))
                || (matches!(a, JT | H3) && b == JT)
            {
                break 'rules Break::Prohibited;
            }
            // LB27.
            if (matches!(a, JL | JV | JT | H2 | H3) && b == PO)
                || (a == PR && matches!(b, JL | JV | JT | H2 | H3))
            {
                break 'rules Break::Prohibited;
            }
            // LB28.
            if matches!(a, AL | HL) && matches!(b, AL | HL) {
                break 'rules Break::Prohibited;
            }
            // LB28a.
            if a == AP && akish(j) {
                break 'rules Break::Prohibited;
            }
            if akish(j - 1) && matches!(b, VF | VI) {
                break 'rules Break::Prohibited;
            }
            if a == VI && j >= 2 && akish(j - 2) && (b == AK || info[j].dotted_circle) {
                break 'rules Break::Prohibited;
            }
            if akish(j - 1) && akish(j) && next(j) == Some(VF) {
                break 'rules Break::Prohibited;
            }
            // LB29.
            if a == IS && matches!(b, AL | HL) {
                break 'rules Break::Prohibited;
            }
            // LB30.
            if matches!(a, AL | HL | NU) && b == OP && !ea(j) {
                break 'rules Break::Prohibited;
            }
            if a == CP && !ea(j - 1) && matches!(b, AL | HL | NU) {
                break 'rules Break::Prohibited;
            }
            // LB30a: un número impar de RI antes de la posición.
            if a == RI && b == RI {
                let mut count = 0;
                let mut p = j;
                while p > 0 && cls(p - 1) == RI {
                    count += 1;
                    p -= 1;
                }
                if count % 2 == 1 {
                    break 'rules Break::Prohibited;
                }
            }
            // LB30b.
            if b == EM
                && (a == EB
                    || (info[j - 1].flags & EXT_PICT != 0 && info[j - 1].flags & CN != 0))
            {
                break 'rules Break::Prohibited;
            }
            // LB31.
            Break::Allowed
        };
        out[pos] = r;
    }
    out
}

/// LB4–LB8a: reglas que miran los caracteres reales alrededor de la posición `i` (antes de
/// LB9). `None` si ninguna decide.
fn early(raw: &[LineBreak], i: usize) -> Option<Break> {
    let a = raw[i - 1];
    let b = raw[i];
    // LB4.
    if a == BK {
        return Some(Break::Mandatory);
    }
    // LB5.
    if a == CR && b == LF {
        return Some(Break::Prohibited);
    }
    if matches!(a, CR | LF | NL) {
        return Some(Break::Mandatory);
    }
    // LB6.
    if matches!(b, BK | CR | LF | NL) {
        return Some(Break::Prohibited);
    }
    // LB7.
    if matches!(b, SP | ZW) {
        return Some(Break::Prohibited);
    }
    // LB8: ZW SP* ÷
    let mut p = i;
    while p > 0 && raw[p - 1] == SP {
        p -= 1;
    }
    if p > 0 && raw[p - 1] == ZW {
        return Some(Break::Allowed);
    }
    // LB8a.
    if a == ZWJ {
        return Some(Break::Prohibited);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opportunities(s: &str) -> Vec<usize> {
        let chars: Vec<char> = s.chars().collect();
        line_breaks(&chars)
            .iter()
            .enumerate()
            .filter(|(_, b)| **b != Break::Prohibited)
            .map(|(i, _)| i)
            .collect()
    }

    #[test]
    fn spaces_and_words() {
        assert_eq!(opportunities("ab cd"), [3, 5]);
        assert_eq!(opportunities("ab  cd"), [4, 6]);
    }

    #[test]
    fn hyphen_and_punctuation() {
        // Después del guion se puede cortar; antes de la coma, no.
        assert_eq!(opportunities("well-known, x"), [5, 12, 13]);
    }

    #[test]
    fn ideographs_break_between() {
        assert_eq!(opportunities("日本語"), [1, 2, 3]);
        // Ni antes de 。(CL).
        assert_eq!(opportunities("日本。語"), [1, 3, 4]);
    }

    #[test]
    fn soft_hyphen_is_opportunity() {
        assert_eq!(opportunities("hy\u{AD}phen"), [3, 7]);
    }
}
