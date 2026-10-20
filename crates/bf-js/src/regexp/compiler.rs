//! Compilación del árbol del patrón a un programa de backtracking (ECMA-262 §22.2.2,
//! CompilePattern / CompileSubpattern / CompileAtom). Cada Matcher del spec se traduce a
//! instrucciones que ejecuta [`super::matcher`]; la dirección (`forward`/`backward`) de los
//! lookbehind queda fija en cada instrucción, y las Alternatives en dirección backward se
//! emiten de derecha a izquierda (§22.2.2.3).

use std::collections::HashMap;

use super::parser::{Node, Pattern};
use super::unicode::{CharSet, ClassSet, canonical_closure, canonicalize, word_characters};

/// Una instrucción. `back` indica la dirección backward (dentro de un lookbehind).
#[derive(Debug, Clone)]
pub enum Inst {
    /// Un carácter: `c` ya pasado por Canonicalize si `ic`.
    Char {
        c: u32,
        ic: bool,
        back: bool,
    },
    /// CharacterSetMatcher: `set` indexa [`Program::sets`] (la clausura canónica si `ic`).
    Set {
        set: u32,
        invert: bool,
        ic: bool,
        back: bool,
    },
    /// `.` (AllCharacters menos los LineTerminator si no hay DotAll).
    Any {
        dot_all: bool,
        back: bool,
    },
    /// Una clase del modo `v` con strings: indexa [`Program::strings`].
    Strings {
        id: u32,
        back: bool,
    },
    /// `^`
    Start {
        multiline: bool,
    },
    /// `$`
    End {
        multiline: bool,
    },
    /// `\b` / `\B`.
    WordBoundary {
        negate: bool,
        ic: bool,
    },
    /// Prueba `first`; si falla, `second`.
    Split {
        first: u32,
        second: u32,
    },
    Jmp(u32),
    /// Guarda el extremo inicial del grupo `n` (el final si es backward).
    GroupStart(u32),
    /// Fija la captura del grupo `n`.
    GroupEnd {
        n: u32,
        back: bool,
    },
    /// BackreferenceMatcher: indexa [`Program::backrefs`].
    BackRef {
        id: u32,
        ic: bool,
        back: bool,
    },
    /// Inicia un cuantificador: su contador a 0.
    RepInit {
        reg: u32,
    },
    /// RepeatMatcher: decide si iterar (el cuerpo empieza en la instrucción siguiente) o
    /// seguir en `exit`.
    RepLoop {
        reg: u32,
        min: u64,
        max: u64,
        greedy: bool,
        exit: u32,
        first_group: u32,
        group_count: u32,
    },
    /// Fin de una iteración: la condición de iteración vacía y vuelta a `RepLoop`.
    RepEnd {
        reg: u32,
        min: u64,
        head: u32,
    },
    /// Un cuantificador sobre un único carácter sin grupos (la instrucción siguiente es el
    /// matcher del carácter y `next` la continuación).
    RepSimple {
        min: u64,
        max: u64,
        greedy: bool,
        next: u32,
    },
    /// Lookahead / lookbehind: el subprograma va de la instrucción siguiente hasta `end`
    /// (exclusivo, termina en `Match`).
    Look {
        negate: bool,
        end: u32,
    },
    Match,
}

/// Las strings de una clase del modo `v`, en el orden de §22.2.2.7 (CompileAtom): de mayor a
/// menor longitud, después los caracteres sueltos y al final el string vacío.
#[derive(Debug, Clone)]
pub struct StringClass {
    /// Strings de más de un carácter (canónicas si `ic`), de mayor a menor longitud.
    pub strings: Vec<Vec<u32>>,
    /// Índices de `strings` por su primer carácter (forward) y por el último (backward).
    pub by_first: HashMap<u32, Vec<u32>>,
    pub by_last: HashMap<u32, Vec<u32>>,
    pub singles: CharSet,
    pub empty: bool,
    pub ic: bool,
}

#[derive(Debug, Clone)]
pub struct Program {
    pub insts: Vec<Inst>,
    pub sets: Vec<CharSet>,
    pub strings: Vec<StringClass>,
    pub backrefs: Vec<Vec<u32>>,
    /// Cantidad de grupos (sin el 0).
    pub group_count: u32,
    pub regs: u32,
    /// HasEitherUnicodeFlag: el input son code points.
    pub unicode: bool,
    /// WordCharacters sin y con IgnoreCase.
    pub word: CharSet,
    pub word_ic: CharSet,
    /// Los caracteres con los que puede empezar un match (si se pueden acotar): las
    /// posiciones cuyo carácter no está aquí no pueden dar un match.
    pub first: Option<CharSet>,
    /// El patrón empieza con `^` sin Multiline: solo puede haber match en el índice 0.
    pub anchored: bool,
}

/// CompilePattern (§22.2.2.2).
pub fn compile(pattern: &Pattern, unicode: bool) -> Program {
    let mut c = Compiler {
        prog: Program {
            insts: Vec::new(),
            sets: Vec::new(),
            strings: Vec::new(),
            backrefs: Vec::new(),
            group_count: pattern.group_count,
            regs: 0,
            unicode,
            word: word_characters(unicode, false),
            word_ic: word_characters(unicode, true),
            first: None,
            anchored: false,
        },
        unicode,
    };
    c.node(&pattern.node, false);
    c.emit(Inst::Match);
    c.prog.first = match first_chars(&pattern.node, unicode) {
        Some((set, false)) => Some(set),
        _ => None,
    };
    c.prog.anchored = starts_anchored(&pattern.node);
    c.prog
}

/// Los caracteres que puede consumir primero `node` (dirección forward) y si puede no
/// consumir nada; `None` si no se acota (IgnoreCase, `.`, backreferences, strings).
fn first_chars(node: &Node, unicode: bool) -> Option<(CharSet, bool)> {
    let max = if unicode { 0x10FFFF } else { 0xFFFF };
    match node {
        Node::Empty
        | Node::Start { .. }
        | Node::End { .. }
        | Node::WordBoundary { .. }
        | Node::Look { .. } => Some((CharSet::new(), true)),
        Node::Char { c, ic: false } => Some((CharSet::single(*c), false)),
        Node::Set {
            set,
            invert,
            ic: false,
        } if set.strings.is_empty() => Some((
            if *invert {
                set.chars.complement(max)
            } else {
                set.chars.clone()
            },
            false,
        )),
        Node::Seq(v) => {
            let mut acc = CharSet::new();
            for n in v {
                let (s, empty) = first_chars(n, unicode)?;
                acc.add_set(&s);
                if !empty {
                    return Some((acc, false));
                }
            }
            Some((acc, true))
        }
        Node::Alt(v) => {
            let mut acc = CharSet::new();
            let mut any_empty = false;
            for n in v {
                let (s, empty) = first_chars(n, unicode)?;
                acc.add_set(&s);
                any_empty |= empty;
            }
            Some((acc, any_empty))
        }
        Node::Capture { body, .. } => first_chars(body, unicode),
        Node::Repeat { body, min, max, .. } => {
            if *max == Some(0) {
                return Some((CharSet::new(), true));
            }
            let (s, empty) = first_chars(body, unicode)?;
            Some((s, empty || *min == 0))
        }
        _ => None,
    }
}

/// ¿Todo match empieza con `^` sin Multiline?
fn starts_anchored(node: &Node) -> bool {
    match node {
        Node::Start { multiline: false } => true,
        Node::Seq(v) => v.first().is_some_and(starts_anchored),
        Node::Alt(v) => v.iter().all(starts_anchored),
        Node::Capture { body, .. } => starts_anchored(body),
        _ => false,
    }
}

struct Compiler {
    prog: Program,
    unicode: bool,
}

impl Compiler {
    fn pc(&self) -> u32 {
        self.prog.insts.len() as u32
    }

    fn emit(&mut self, i: Inst) -> u32 {
        self.prog.insts.push(i);
        self.pc() - 1
    }

    fn patch(&mut self, at: u32, target: u32) {
        match &mut self.prog.insts[at as usize] {
            Inst::Split { second, .. } => *second = target,
            Inst::Jmp(t) => *t = target,
            Inst::RepLoop { exit, .. } => *exit = target,
            Inst::RepSimple { next, .. } => *next = target,
            Inst::Look { end, .. } => *end = target,
            _ => unreachable!("patch"),
        }
    }

    fn add_set(&mut self, set: &CharSet, ic: bool) -> u32 {
        let set = if ic {
            canonical_closure(set, self.unicode)
        } else {
            set.clone()
        };
        self.prog.sets.push(set);
        self.prog.sets.len() as u32 - 1
    }

    /// ¿El nodo es un único CharacterSetMatcher (para [`Inst::RepSimple`])?
    fn single_char(node: &Node) -> bool {
        match node {
            Node::Char { .. } | Node::Dot { .. } => true,
            Node::Set { set, .. } => set.strings.is_empty(),
            _ => false,
        }
    }

    fn node(&mut self, node: &Node, back: bool) {
        match node {
            Node::Empty => {}
            Node::Char { c, ic } => {
                let c = canonicalize(*c, self.unicode, *ic);
                self.emit(Inst::Char { c, ic: *ic, back });
            }
            Node::Dot { dot_all } => {
                self.emit(Inst::Any {
                    dot_all: *dot_all,
                    back,
                });
            }
            Node::Set { set, invert, ic } => self.set(set, *invert, *ic, back),
            Node::Seq(v) => {
                if back {
                    for n in v.iter().rev() {
                        self.node(n, back);
                    }
                } else {
                    for n in v {
                        self.node(n, back);
                    }
                }
            }
            Node::Alt(v) => {
                // Split(alt1, siguiente) … con un salto al final de cada alternativa.
                let mut jumps = Vec::new();
                for (k, alt) in v.iter().enumerate() {
                    if k + 1 < v.len() {
                        let split = self.emit(Inst::Split {
                            first: self.pc() + 1,
                            second: 0,
                        });
                        self.node(alt, back);
                        jumps.push(self.emit(Inst::Jmp(0)));
                        let next = self.pc();
                        self.patch(split, next);
                    } else {
                        self.node(alt, back);
                    }
                }
                let end = self.pc();
                for j in jumps {
                    self.patch(j, end);
                }
            }
            Node::Capture { index, body } => {
                self.emit(Inst::GroupStart(*index));
                self.node(body, back);
                self.emit(Inst::GroupEnd { n: *index, back });
            }
            Node::Start { multiline } => {
                self.emit(Inst::Start {
                    multiline: *multiline,
                });
            }
            Node::End { multiline } => {
                self.emit(Inst::End {
                    multiline: *multiline,
                });
            }
            Node::WordBoundary { negate, ic } => {
                self.emit(Inst::WordBoundary {
                    negate: *negate,
                    ic: *ic,
                });
            }
            Node::Look {
                behind,
                negate,
                body,
            } => {
                let look = self.emit(Inst::Look {
                    negate: *negate,
                    end: 0,
                });
                self.node(body, *behind);
                self.emit(Inst::Match);
                let end = self.pc();
                self.patch(look, end);
            }
            Node::BackRef { groups, ic } => {
                self.prog.backrefs.push(groups.clone());
                let id = self.prog.backrefs.len() as u32 - 1;
                self.emit(Inst::BackRef { id, ic: *ic, back });
            }
            Node::Repeat {
                body,
                min,
                max,
                greedy,
                first_group,
                group_count,
            } => {
                let max = max.unwrap_or(u64::MAX);
                if max == 0 {
                    // RepeatMatcher paso 1: max = 0 no prueba el átomo.
                    return;
                }
                if *group_count == 0 && Self::single_char(body) {
                    let rep = self.emit(Inst::RepSimple {
                        min: *min,
                        max,
                        greedy: *greedy,
                        next: 0,
                    });
                    self.node(body, back);
                    let next = self.pc();
                    self.patch(rep, next);
                    return;
                }
                let reg = self.prog.regs;
                self.prog.regs += 1;
                self.emit(Inst::RepInit { reg });
                let head = self.emit(Inst::RepLoop {
                    reg,
                    min: *min,
                    max,
                    greedy: *greedy,
                    exit: 0,
                    first_group: *first_group,
                    group_count: *group_count,
                });
                self.node(body, back);
                self.emit(Inst::RepEnd {
                    reg,
                    min: *min,
                    head,
                });
                let exit = self.pc();
                self.patch(head, exit);
            }
        }
    }

    fn set(&mut self, set: &ClassSet, invert: bool, ic: bool, back: bool) {
        if set.strings.is_empty() {
            let id = self.add_set(&set.chars, ic);
            self.emit(Inst::Set {
                set: id,
                invert,
                ic,
                back,
            });
            return;
        }
        let canon = |s: &Vec<u32>| -> Vec<u32> {
            s.iter()
                .map(|&c| canonicalize(c, self.unicode, ic))
                .collect()
        };
        let mut strings: Vec<Vec<u32>> = set
            .strings
            .iter()
            .filter(|s| !s.is_empty())
            .map(canon)
            .collect();
        strings.sort_by_key(|s| std::cmp::Reverse(s.len()));
        let mut by_first: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut by_last: HashMap<u32, Vec<u32>> = HashMap::new();
        for (i, s) in strings.iter().enumerate() {
            by_first.entry(s[0]).or_default().push(i as u32);
            by_last.entry(s[s.len() - 1]).or_default().push(i as u32);
        }
        let singles = if ic {
            canonical_closure(&set.chars, self.unicode)
        } else {
            set.chars.clone()
        };
        self.prog.strings.push(StringClass {
            strings,
            by_first,
            by_last,
            singles,
            empty: set.strings.contains(&Vec::new()),
            ic,
        });
        let id = self.prog.strings.len() as u32 - 1;
        self.emit(Inst::Strings { id, back });
    }
}
