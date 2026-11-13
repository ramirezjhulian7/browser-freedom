//! E5: desglose del tiempo de maqueta para medir la primera pintura (`BF_TRACE`).
//!
//! Con `BF_TRACE` en el entorno, cada maqueta completa imprime en stderr una línea
//! `perf maqueta: …` con el tiempo **propio** de cada parte (lo que no se fue a una parte
//! anidada: el shaping de las líneas de un ítem flex cuenta en *shaping*, no en *flex*), el
//! tiempo **total** desde la entrada más externa de la parte y el número de llamadas. Sin
//! `BF_TRACE` cada marca es una comprobación de un `bool`.

use std::cell::RefCell;
use std::time::{Duration, Instant};

/// Las partes de la maqueta que se miden por separado.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Part {
    /// `block_in`: dimensionar y colocar cajas de bloque (y lo que no es de otra parte).
    Block,
    /// `inline_content`: recoger, bidi, cortar y colocar líneas (sin el shaping).
    Inline,
    /// Shaping y métricas (`FontProvider`).
    Shape,
    /// `intrinsic_widths`: anchos min-content y max-content.
    Intrinsic,
    Table,
    Flex,
    Grid,
    /// `float_box`.
    Float,
    /// Cajas absolutas y fijas (`absolute`).
    Positioned,
}

const PARTS: [(Part, &str); 9] = [
    (Part::Block, "bloques"),
    (Part::Inline, "líneas"),
    (Part::Shape, "shaping"),
    (Part::Intrinsic, "intrínsecos"),
    (Part::Table, "tablas"),
    (Part::Flex, "flex"),
    (Part::Grid, "grid"),
    (Part::Float, "flotantes"),
    (Part::Positioned, "posicionados"),
];

fn index(p: Part) -> usize {
    PARTS.iter().position(|(q, _)| *q == p).unwrap_or(0)
}

#[derive(Default)]
struct State {
    /// Partes abiertas, con el instante en que se abrió cada una.
    stack: Vec<(Part, Instant)>,
    last: Option<Instant>,
    own: [Duration; PARTS.len()],
    total: [Duration; PARTS.len()],
    calls: [u32; PARTS.len()],
    /// Cajas distintas que entraron en cada parte (para ver cuánto se repite).
    boxes: [std::collections::HashSet<usize>; PARTS.len()],
}

/// Contador de tiempos por parte; inactivo (sin coste) si no se pidió la traza.
pub(crate) struct Perf {
    on: bool,
    state: RefCell<State>,
}

impl Perf {
    pub(crate) fn new() -> Self {
        Perf {
            on: std::env::var_os("BF_TRACE").is_some(),
            state: RefCell::default(),
        }
    }

    /// Abre `p` (hasta el [`Self::exit`] correspondiente).
    #[inline]
    pub(crate) fn enter(&self, p: Part) {
        if self.on {
            self.enter_slow(p, None);
        }
    }

    /// Como [`Self::enter`], anotando la caja `b` para contar las cajas distintas.
    #[inline]
    pub(crate) fn enter_box<T>(&self, p: Part, b: &T) {
        if self.on {
            self.enter_slow(p, Some(b as *const T as usize));
        }
    }

    /// Cierra la última parte abierta.
    #[inline]
    pub(crate) fn exit(&self) {
        if self.on {
            self.exit_slow();
        }
    }

    fn enter_slow(&self, p: Part, b: Option<usize>) {
        let now = Instant::now();
        let mut s = self.state.borrow_mut();
        if let Some(b) = b {
            s.boxes[index(p)].insert(b);
        }
        if let (Some(&(top, _)), Some(last)) = (s.stack.last(), s.last) {
            s.own[index(top)] += now - last;
        }
        s.calls[index(p)] += 1;
        s.stack.push((p, now));
        s.last = Some(now);
    }

    fn exit_slow(&self) {
        let now = Instant::now();
        let mut s = self.state.borrow_mut();
        let Some((p, start)) = s.stack.pop() else {
            return;
        };
        if let Some(last) = s.last {
            s.own[index(p)] += now - last;
        }
        // El total cuenta solo la entrada más externa de cada parte.
        if !s.stack.iter().any(|(q, _)| *q == p) {
            s.total[index(p)] += now - start;
        }
        s.last = Some(now);
    }

    /// La línea `perf maqueta` (si la traza está activa): `tree` es el tiempo de construir el
    /// árbol de cajas y `all` el de la maqueta entera.
    pub(crate) fn report(&self, tree: Duration, all: Duration, boxes: usize) {
        if !self.on {
            return;
        }
        let s = self.state.borrow();
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let parts: Vec<String> = PARTS
            .iter()
            .enumerate()
            .filter(|(i, _)| s.calls[*i] > 0)
            .map(|(i, (_, name))| {
                let distinct = match s.boxes[i].len() {
                    0 => String::new(),
                    n => format!(" en {n} cajas"),
                };
                format!(
                    "{name} {:.1} ms (total {:.1}, {} llamadas{distinct})",
                    ms(s.own[i]),
                    ms(s.total[i]),
                    s.calls[i]
                )
            })
            .collect();
        eprintln!(
            "perf maqueta: {:.1} ms; árbol de cajas {:.1} ms ({boxes} cajas) · {}",
            ms(all),
            ms(tree),
            parts.join(" · ")
        );
    }
}
