//! Ejecución de un [`Program`]: un intérprete de backtracking con pila explícita que
//! reproduce las semánticas de Matcher/MatcherContinuation de ECMA-262 §22.2.2.
//!
//! El estado del spec (MatchState: endIndex y captures) es inmutable y se copia en cada
//! paso; aquí es mutable y cada cambio deja en la pila de backtracking una entrada que lo
//! deshace (undo log). Fallar es desapilar hasta el último punto de elección, deshaciendo
//! los cambios hechos después de él. Los lookaround se ejecutan como una sub-ejecución sobre
//! la misma pila: al tener éxito se descartan sus puntos de elección (son atómicos, §22.2.2.4
//! pasos de `(?=`) pero se conservan sus entradas de undo, para que fallar después restaure
//! las capturas.

use std::time::Instant;

use super::compiler::{Inst, Program};
use super::unicode::{canonicalize, is_line_terminator};

/// Un fallo que no es "no hay match": se agotó un límite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchError {
    /// Demasiados pasos o demasiada memoria de backtracking.
    Limit(&'static str),
    /// Se pasó el límite de tiempo del agente.
    Timeout,
}

/// Una captura: (inicio, fin) en code units.
pub type Capture = Option<(usize, usize)>;

/// Límites del matcher.
const MAX_STACK: usize = 4_000_000;
const MAX_LOOK_DEPTH: u32 = 400;
const MAX_STEPS: u64 = 2_000_000_000;

#[derive(Debug, Clone, Copy)]
enum Bt {
    /// Punto de elección: seguir en `pc` con `pos`.
    Choice {
        pc: u32,
        pos: usize,
    },
    /// Punto de elección de un cuantificador no greedy: iterar el `RepLoop` de `pc`.
    Iterate {
        pc: u32,
        pos: usize,
    },
    /// `RepSimple` greedy: devolver un carácter (sin bajar de `min_pos`).
    GreedyGive {
        next: u32,
        pos: usize,
        min_pos: usize,
        back: bool,
    },
    /// `RepSimple` lazy: tomar un carácter más.
    LazyTake {
        pc: u32,
        pos: usize,
        count: u64,
    },
    Capture {
        n: u32,
        old: Capture,
    },
    Pending {
        n: u32,
        old: usize,
    },
    Reg {
        r: u32,
        count: u64,
        start: usize,
    },
}

impl Bt {
    fn is_choice(&self) -> bool {
        matches!(
            self,
            Bt::Choice { .. } | Bt::Iterate { .. } | Bt::GreedyGive { .. } | Bt::LazyTake { .. }
        )
    }
}

pub struct Matcher<'a> {
    prog: &'a Program,
    input: &'a [u16],
    caps: Vec<Capture>,
    pending: Vec<usize>,
    regs: Vec<(u64, usize)>,
    stack: Vec<Bt>,
    steps: u64,
    depth: u32,
    deadline: Option<Instant>,
}

fn is_lead(u: u16) -> bool {
    (0xD800..0xDC00).contains(&u)
}

fn is_trail(u: u16) -> bool {
    (0xDC00..0xE000).contains(&u)
}

impl<'a> Matcher<'a> {
    pub fn new(prog: &'a Program, input: &'a [u16], deadline: Option<Instant>) -> Matcher<'a> {
        let n = prog.group_count as usize + 1;
        Matcher {
            prog,
            input,
            caps: vec![None; n],
            pending: vec![0; n],
            regs: vec![(0, 0); prog.regs as usize],
            stack: Vec::new(),
            steps: 0,
            depth: 0,
            deadline,
        }
    }

    /// El matcher en el índice `start` (code units): las capturas (la 0 es el match) o
    /// `None`. Con `u`/`v`, un `start` en medio de un par de surrogates empieza en el par
    /// (§22.2.7.2 RegExpBuiltinExec, "the character that was obtained from element
    /// lastIndex").
    pub fn match_at(&mut self, start: usize) -> Result<Option<Vec<Capture>>, MatchError> {
        let mut start = start;
        if self.prog.unicode
            && start > 0
            && start < self.input.len()
            && is_trail(self.input[start])
            && is_lead(self.input[start - 1])
        {
            start -= 1;
        }
        self.caps.fill(None);
        self.stack.clear();
        self.depth = 0;
        match self.run(0, start)? {
            Some(end) => {
                let mut caps = self.caps.clone();
                caps[0] = Some((start, end));
                Ok(Some(caps))
            }
            None => Ok(None),
        }
    }

    // --- Lectura del input ---

    /// El carácter que empieza en `pos` y la posición siguiente.
    fn next_char(&self, pos: usize) -> Option<(u32, usize)> {
        let u = *self.input.get(pos)?;
        if self.prog.unicode
            && is_lead(u)
            && let Some(&t) = self.input.get(pos + 1)
            && is_trail(t)
        {
            let c = 0x10000 + ((u32::from(u) - 0xD800) << 10) + (u32::from(t) - 0xDC00);
            return Some((c, pos + 2));
        }
        Some((u32::from(u), pos + 1))
    }

    /// El carácter que termina en `pos` y la posición anterior.
    fn prev_char(&self, pos: usize) -> Option<(u32, usize)> {
        if pos == 0 {
            return None;
        }
        let u = self.input[pos - 1];
        if self.prog.unicode && is_trail(u) && pos >= 2 && is_lead(self.input[pos - 2]) {
            let l = self.input[pos - 2];
            let c = 0x10000 + ((u32::from(l) - 0xD800) << 10) + (u32::from(u) - 0xDC00);
            return Some((c, pos - 2));
        }
        Some((u32::from(u), pos - 1))
    }

    fn read(&self, pos: usize, back: bool) -> Option<(u32, usize)> {
        if back {
            self.prev_char(pos)
        } else {
            self.next_char(pos)
        }
    }

    fn canon(&self, c: u32, ic: bool) -> u32 {
        canonicalize(c, self.prog.unicode, ic)
    }

    /// ¿La instrucción de un carácter (`Char`, `Set`, `Any`) acepta el input en `pos`? La
    /// nueva posición.
    fn char_step(&self, inst: &Inst, pos: usize) -> Option<usize> {
        match *inst {
            Inst::Char { c, ic, back } => {
                let (x, np) = self.read(pos, back)?;
                (self.canon(x, ic) == c).then_some(np)
            }
            Inst::Set {
                set,
                invert,
                ic,
                back,
            } => {
                let (x, np) = self.read(pos, back)?;
                let found = self.prog.sets[set as usize].contains(self.canon(x, ic));
                (found != invert).then_some(np)
            }
            Inst::Any { dot_all, back } => {
                let (x, np) = self.read(pos, back)?;
                (dot_all || !is_line_terminator(x)).then_some(np)
            }
            _ => None,
        }
    }

    fn is_word_char(&self, pos: usize, before: bool, ic: bool) -> bool {
        let c = if before {
            self.prev_char(pos)
        } else {
            self.next_char(pos)
        };
        let set = if ic {
            &self.prog.word_ic
        } else {
            &self.prog.word
        };
        c.is_some_and(|(c, _)| set.contains(c))
    }

    // --- Estado ---

    fn push(&mut self, b: Bt) -> Result<(), MatchError> {
        if self.stack.len() >= MAX_STACK {
            return Err(MatchError::Limit("pila de backtracking agotada"));
        }
        self.stack.push(b);
        Ok(())
    }

    fn set_capture(&mut self, n: u32, v: Capture) -> Result<(), MatchError> {
        let old = self.caps[n as usize];
        self.push(Bt::Capture { n, old })?;
        self.caps[n as usize] = v;
        Ok(())
    }

    fn set_reg(&mut self, r: u32, count: u64, start: usize) -> Result<(), MatchError> {
        let (oc, os) = self.regs[r as usize];
        self.push(Bt::Reg {
            r,
            count: oc,
            start: os,
        })?;
        self.regs[r as usize] = (count, start);
        Ok(())
    }

    /// Desapila hasta el último punto de elección por encima de `base` y devuelve dónde
    /// seguir; `None` si se llegó a `base`.
    fn backtrack(&mut self, base: usize) -> Result<Option<(u32, usize)>, MatchError> {
        while self.stack.len() > base {
            let Some(b) = self.stack.pop() else { break };
            match b {
                Bt::Capture { n, old } => self.caps[n as usize] = old,
                Bt::Pending { n, old } => self.pending[n as usize] = old,
                Bt::Reg { r, count, start } => self.regs[r as usize] = (count, start),
                Bt::Choice { pc, pos } => return Ok(Some((pc, pos))),
                Bt::Iterate { pc, pos } => {
                    // El RepLoop de `pc` decide iterar.
                    let Inst::RepLoop {
                        reg,
                        first_group,
                        group_count,
                        ..
                    } = self.prog.insts[pc as usize]
                    else {
                        unreachable!("Iterate sin RepLoop")
                    };
                    self.iterate(reg, pos, first_group, group_count)?;
                    return Ok(Some((pc + 1, pos)));
                }
                Bt::GreedyGive {
                    next,
                    pos,
                    min_pos,
                    back,
                } => {
                    // Devolver un carácter: retroceder (o avanzar si es backward) uno.
                    let np = if back {
                        self.next_char(pos).map(|(_, p)| p)
                    } else {
                        self.prev_char(pos).map(|(_, p)| p)
                    };
                    let Some(np) = np else { continue };
                    if np != min_pos {
                        self.push(Bt::GreedyGive {
                            next,
                            pos: np,
                            min_pos,
                            back,
                        })?;
                    }
                    return Ok(Some((next, np)));
                }
                Bt::LazyTake { pc, pos, count } => {
                    let Inst::RepSimple { max, next, .. } = self.prog.insts[pc as usize] else {
                        unreachable!("LazyTake sin RepSimple")
                    };
                    if count >= max {
                        continue;
                    }
                    let Some(np) = self.char_step(&self.prog.insts[pc as usize + 1], pos) else {
                        continue;
                    };
                    self.push(Bt::LazyTake {
                        pc,
                        pos: np,
                        count: count + 1,
                    })?;
                    return Ok(Some((next, np)));
                }
            }
        }
        Ok(None)
    }

    /// Prepara una iteración de RepeatMatcher (pasos 3–4): resetea las capturas del átomo y
    /// recuerda dónde empezó la iteración (para la condición de iteración vacía).
    fn iterate(
        &mut self,
        reg: u32,
        pos: usize,
        first_group: u32,
        group_count: u32,
    ) -> Result<(), MatchError> {
        let (count, _) = self.regs[reg as usize];
        self.set_reg(reg, count + 1, pos)?;
        for k in first_group + 1..=first_group + group_count {
            if self.caps[k as usize].is_some() {
                self.set_capture(k, None)?;
            }
        }
        Ok(())
    }

    /// Ejecuta desde `pc` en `pos` hasta un `Match` (devuelve la posición final) o hasta
    /// agotar los puntos de elección propios (los de encima de la altura actual de la pila).
    fn run(&mut self, pc: u32, pos: usize) -> Result<Option<usize>, MatchError> {
        let base = self.stack.len();
        let mut pc = pc as usize;
        let mut pos = pos;
        loop {
            self.steps += 1;
            if self.steps & 0xFFFF == 0 {
                if self.steps > MAX_STEPS {
                    return Err(MatchError::Limit("demasiados pasos de backtracking"));
                }
                if let Some(d) = self.deadline
                    && Instant::now() > d
                {
                    return Err(MatchError::Timeout);
                }
            }
            let ok: bool = match self.prog.insts[pc] {
                Inst::Char { .. } | Inst::Set { .. } | Inst::Any { .. } => {
                    match self.char_step(&self.prog.insts[pc], pos) {
                        Some(np) => {
                            pos = np;
                            pc += 1;
                            true
                        }
                        None => false,
                    }
                }
                Inst::Strings { id, back } => self.strings(id, back, &mut pc, &mut pos)?,
                Inst::Start { multiline } => {
                    let ok = pos == 0
                        || (multiline && is_line_terminator(u32::from(self.input[pos - 1])));
                    pc += 1;
                    ok
                }
                Inst::End { multiline } => {
                    let ok = pos == self.input.len()
                        || (multiline && is_line_terminator(u32::from(self.input[pos])));
                    pc += 1;
                    ok
                }
                Inst::WordBoundary { negate, ic } => {
                    let a = self.is_word_char(pos, true, ic);
                    let b = self.is_word_char(pos, false, ic);
                    pc += 1;
                    (a != b) != negate
                }
                Inst::Split { first, second } => {
                    self.push(Bt::Choice { pc: second, pos })?;
                    pc = first as usize;
                    true
                }
                Inst::Jmp(t) => {
                    pc = t as usize;
                    true
                }
                Inst::GroupStart(n) => {
                    let old = self.pending[n as usize];
                    self.push(Bt::Pending { n, old })?;
                    self.pending[n as usize] = pos;
                    pc += 1;
                    true
                }
                Inst::GroupEnd { n, back } => {
                    let p = self.pending[n as usize];
                    let v = if back { (pos, p) } else { (p, pos) };
                    self.set_capture(n, Some(v))?;
                    pc += 1;
                    true
                }
                Inst::BackRef { id, ic, back } => match self.backref(id, ic, back, pos) {
                    Some(np) => {
                        pos = np;
                        pc += 1;
                        true
                    }
                    None => false,
                },
                Inst::RepInit { reg } => {
                    self.set_reg(reg, 0, pos)?;
                    pc += 1;
                    true
                }
                Inst::RepLoop {
                    reg,
                    min,
                    max,
                    greedy,
                    exit,
                    first_group,
                    group_count,
                } => {
                    let (count, _) = self.regs[reg as usize];
                    if count >= max {
                        pc = exit as usize;
                    } else if count < min {
                        self.iterate(reg, pos, first_group, group_count)?;
                        pc += 1;
                    } else if greedy {
                        self.push(Bt::Choice { pc: exit, pos })?;
                        self.iterate(reg, pos, first_group, group_count)?;
                        pc += 1;
                    } else {
                        self.push(Bt::Iterate { pc: pc as u32, pos })?;
                        pc = exit as usize;
                    }
                    true
                }
                Inst::RepEnd { reg, min, head } => {
                    // RepeatMatcher paso 2.b: si la iteración empezó con min = 0 y no
                    // consumió nada, falla.
                    let (count, start) = self.regs[reg as usize];
                    if count > min && pos == start {
                        false
                    } else {
                        pc = head as usize;
                        true
                    }
                }
                Inst::RepSimple {
                    min,
                    max,
                    greedy,
                    next,
                } => {
                    let ok = self.rep_simple(pc, min, max, greedy, next, &mut pos)?;
                    if ok {
                        pc = next as usize;
                    }
                    ok
                }
                Inst::Look { negate, end } => {
                    if self.depth >= MAX_LOOK_DEPTH {
                        return Err(MatchError::Limit("lookaround demasiado anidados"));
                    }
                    let sub_base = self.stack.len();
                    self.depth += 1;
                    let r = self.run(pc as u32 + 1, pos);
                    self.depth -= 1;
                    let matched = r?.is_some();
                    if matched {
                        if negate {
                            // `(?!…)` con match: deshacer todo lo de la sub-ejecución.
                            self.unwind(sub_base);
                        } else {
                            // `(?=…)`: atómico; se conservan las capturas (y su undo).
                            self.cut(sub_base);
                        }
                    }
                    pc = end as usize;
                    matched != negate
                }
                Inst::Match => return Ok(Some(pos)),
            };
            if !ok {
                match self.backtrack(base)? {
                    Some((npc, npos)) => {
                        pc = npc as usize;
                        pos = npos;
                    }
                    None => return Ok(None),
                }
            }
        }
    }

    /// Descarta los puntos de elección por encima de `base` (conserva el undo).
    fn cut(&mut self, base: usize) {
        let mut w = base;
        for r in base..self.stack.len() {
            let b = self.stack[r];
            if !b.is_choice() {
                self.stack[w] = b;
                w += 1;
            }
        }
        self.stack.truncate(w);
    }

    /// Deshace todo lo de encima de `base`.
    fn unwind(&mut self, base: usize) {
        while self.stack.len() > base {
            match self.stack.pop() {
                Some(Bt::Capture { n, old }) => self.caps[n as usize] = old,
                Some(Bt::Pending { n, old }) => self.pending[n as usize] = old,
                Some(Bt::Reg { r, count, start }) => self.regs[r as usize] = (count, start),
                _ => {}
            }
        }
    }

    /// Cuantificador sobre un carácter: consume lo que pueda (greedy) o lo mínimo (lazy) y
    /// deja un único punto de elección que devuelve / toma de a un carácter.
    #[allow(clippy::too_many_arguments)]
    fn rep_simple(
        &mut self,
        at: usize,
        min: u64,
        max: u64,
        greedy: bool,
        next: u32,
        pos: &mut usize,
    ) -> Result<bool, MatchError> {
        let inst = &self.prog.insts[at + 1];
        let back = matches!(
            *inst,
            Inst::Char { back: true, .. }
                | Inst::Set { back: true, .. }
                | Inst::Any { back: true, .. }
        );
        let mut p = *pos;
        let mut count = 0u64;
        while count < min {
            match self.char_step(inst, p) {
                Some(np) => {
                    p = np;
                    count += 1;
                }
                None => return Ok(false),
            }
            self.steps += 1;
        }
        let min_pos = p;
        if greedy {
            while count < max {
                match self.char_step(inst, p) {
                    Some(np) => {
                        p = np;
                        count += 1;
                    }
                    None => break,
                }
                self.steps += 1;
            }
            if p != min_pos {
                self.push(Bt::GreedyGive {
                    next,
                    pos: p,
                    min_pos,
                    back,
                })?;
            }
        } else if count < max {
            self.push(Bt::LazyTake {
                pc: at as u32,
                pos: p,
                count,
            })?;
        }
        *pos = p;
        Ok(true)
    }

    /// Una clase del modo `v` con strings: prueba las alternativas en orden (strings de
    /// mayor a menor longitud, caracteres sueltos, string vacío).
    fn strings(
        &mut self,
        id: u32,
        back: bool,
        pc: &mut usize,
        pos: &mut usize,
    ) -> Result<bool, MatchError> {
        let sc = &self.prog.strings[id as usize];
        let mut ends: Vec<usize> = Vec::new();
        if let Some((first, first_end)) = self.read(*pos, back) {
            let key = self.canon(first, sc.ic);
            let idx = if back {
                sc.by_last.get(&key)
            } else {
                sc.by_first.get(&key)
            };
            if let Some(idx) = idx {
                for &i in idx {
                    let s = &sc.strings[i as usize];
                    let mut p = *pos;
                    let mut ok = true;
                    for k in 0..s.len() {
                        let c = if back { s[s.len() - 1 - k] } else { s[k] };
                        match self.read(p, back) {
                            Some((x, np)) if self.canon(x, sc.ic) == c => p = np,
                            _ => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok && !ends.contains(&p) {
                        ends.push(p);
                    }
                }
            }
            if sc.singles.contains(key) && !ends.contains(&first_end) {
                ends.push(first_end);
            }
        }
        if sc.empty && !ends.contains(pos) {
            ends.push(*pos);
        }
        if ends.is_empty() {
            return Ok(false);
        }
        let next = *pc as u32 + 1;
        for &e in ends[1..].iter().rev() {
            self.push(Bt::Choice { pc: next, pos: e })?;
        }
        *pos = ends[0];
        *pc += 1;
        Ok(true)
    }

    /// BackreferenceMatcher (§22.2.2.7.2): el grupo de `groups` que participó (si ninguno,
    /// la referencia es vacía) comparado carácter a carácter con Canonicalize.
    fn backref(&self, id: u32, ic: bool, back: bool, pos: usize) -> Option<usize> {
        let groups = &self.prog.backrefs[id as usize];
        let Some((s, e)) = groups.iter().find_map(|&n| self.caps[n as usize]) else {
            return Some(pos);
        };
        // Los caracteres de la captura, en orden de lectura.
        let mut cap = Vec::new();
        let mut i = s;
        while i < e {
            let u = self.input[i];
            if self.prog.unicode && is_lead(u) && i + 1 < e && is_trail(self.input[i + 1]) {
                cap.push(
                    0x10000
                        + ((u32::from(u) - 0xD800) << 10)
                        + (u32::from(self.input[i + 1]) - 0xDC00),
                );
                i += 2;
            } else {
                cap.push(u32::from(u));
                i += 1;
            }
        }
        let mut p = pos;
        if back {
            for &c in cap.iter().rev() {
                let (x, np) = self.prev_char(p)?;
                if self.canon(x, ic) != self.canon(c, ic) {
                    return None;
                }
                p = np;
            }
        } else {
            for &c in &cap {
                let (x, np) = self.next_char(p)?;
                if self.canon(x, ic) != self.canon(c, ic) {
                    return None;
                }
                p = np;
            }
        }
        Some(p)
    }
}
