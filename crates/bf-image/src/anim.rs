//! Imágenes animadas (GIF89a §23–§25, APNG): los fotogramas ya compuestos sobre el lienzo, con
//! su duración y cómo se disponen, y cuál se ve en cada instante ([`ImageFrames::frame_at`]).

use std::time::Duration;

use crate::{Image, premultiply};

/// Qué pasa con la región del fotograma antes de pintar el siguiente (GIF *disposal method*,
/// APNG `dispose_op`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dispose {
    /// Se deja como está.
    #[default]
    None,
    /// La región vuelve a transparente.
    Background,
    /// La región vuelve a como estaba antes del fotograma.
    Previous,
}

/// Cómo se pintan los píxeles del fotograma sobre el lienzo (APNG `blend_op`; los GIF siempre
/// componen con [`Blend::Over`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Blend {
    /// Reemplazan a los del lienzo (alfa incluido).
    Source,
    /// Se componen encima (*source over*).
    #[default]
    Over,
}

/// Un fotograma: el lienzo completo tal como se ve mientras dura.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub image: Image,
    pub delay: Duration,
    /// Región que pintó el fotograma: x, y, ancho y alto en el lienzo.
    pub region: (u32, u32, u32, u32),
    pub dispose: Dispose,
    pub blend: Blend,
}

/// Los fotogramas de una imagen, compuestos sobre un lienzo de `width`×`height`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageFrames {
    pub width: u32,
    pub height: u32,
    pub frames: Vec<Frame>,
    /// Veces que se reproduce la animación entera; `None` = sin fin.
    pub plays: Option<u32>,
}

/// Duración mínima de un fotograma: como los navegadores, los retardos de 10 ms o menos se
/// muestran a 100 ms (muchos GIF viejos dicen 0).
const MIN_DELAY: Duration = Duration::from_millis(10);
const DEFAULT_DELAY: Duration = Duration::from_millis(100);

/// Tope de memoria de los fotogramas compuestos: los que no entran se descartan.
const MAX_BYTES: usize = 256 << 20;

impl ImageFrames {
    /// Una imagen fija: un solo fotograma.
    pub fn still(image: Image) -> ImageFrames {
        ImageFrames {
            width: image.width,
            height: image.height,
            frames: vec![Frame {
                region: (0, 0, image.width, image.height),
                image,
                delay: Duration::ZERO,
                dispose: Dispose::None,
                blend: Blend::Source,
            }],
            plays: Some(1),
        }
    }

    /// ¿Hay más de un fotograma?
    pub fn is_animated(&self) -> bool {
        self.frames.len() > 1
    }

    /// Duración de una pasada de la animación.
    pub fn cycle(&self) -> Duration {
        self.frames.iter().map(|f| f.delay).sum()
    }

    /// Índice del fotograma visible `t` después de empezar. Terminadas las pasadas queda el
    /// último.
    pub fn index_at(&self, t: Duration) -> usize {
        let cycle = self.cycle();
        if !self.is_animated() || cycle.is_zero() {
            return 0;
        }
        let pass = t.as_nanos() / cycle.as_nanos();
        if self.plays.is_some_and(|p| pass >= u128::from(p.max(1))) {
            return self.frames.len() - 1;
        }
        let mut rest = Duration::from_nanos((t.as_nanos() % cycle.as_nanos()) as u64);
        for (i, f) in self.frames.iter().enumerate() {
            if rest < f.delay {
                return i;
            }
            rest -= f.delay;
        }
        self.frames.len() - 1
    }

    /// El fotograma visible `t` después de empezar.
    pub fn frame_at(&self, t: Duration) -> &Image {
        &self.frames[self.index_at(t)].image
    }

    /// Cuánto falta, desde `t`, para que cambie el fotograma visible; `None` si ya no cambia.
    /// Sirve para programar el próximo cuadro sin consumir CPU entre medio.
    pub fn next_change(&self, t: Duration) -> Option<Duration> {
        let cycle = self.cycle();
        if !self.is_animated() || cycle.is_zero() {
            return None;
        }
        if let Some(p) = self.plays {
            // En la última pasada, el último fotograma queda fijo.
            let last = self.frames[self.frames.len() - 1].delay;
            if t >= cycle * p.max(1) - last {
                return None;
            }
        }
        let mut rest = Duration::from_nanos((t.as_nanos() % cycle.as_nanos()) as u64);
        for f in &self.frames {
            if rest < f.delay {
                return Some(f.delay - rest);
            }
            rest -= f.delay;
        }
        None
    }

    /// Normaliza un retardo leído del archivo.
    pub(crate) fn delay(d: Duration) -> Duration {
        if d <= MIN_DELAY { DEFAULT_DELAY } else { d }
    }
}

/// Compone fotogramas parciales (RGBA sin premultiplicar) sobre el lienzo.
pub(crate) struct Compositor {
    width: u32,
    height: u32,
    /// Lienzo premultiplicado.
    canvas: Vec<u8>,
    /// Disposición pendiente del fotograma anterior.
    pending: Option<(Dispose, (u32, u32, u32, u32), Option<Vec<u8>>)>,
    pub frames: Vec<Frame>,
    bytes: usize,
}

impl Compositor {
    pub fn new(width: u32, height: u32) -> Compositor {
        Compositor {
            width,
            height,
            canvas: vec![0; (width * height * 4) as usize],
            pending: None,
            frames: Vec::new(),
            bytes: 0,
        }
    }

    /// Añade un fotograma de `w`×`h` en (`x`, `y`). Devuelve `false` si ya no entran más.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        (x, y, w, h): (u32, u32, u32, u32),
        mut rgba: Vec<u8>,
        delay: Duration,
        mut dispose: Dispose,
        blend: Blend,
    ) -> bool {
        let size = self.canvas.len();
        if self.bytes + size > MAX_BYTES && !self.frames.is_empty() {
            return false;
        }
        // Disposición del anterior.
        if let Some((d, (px, py, pw, ph), saved)) = self.pending.take() {
            match d {
                Dispose::None => {}
                Dispose::Background => self.fill_region((px, py, pw, ph), None),
                Dispose::Previous => {
                    if let Some(saved) = saved {
                        self.canvas = saved;
                    }
                }
            }
        }
        if self.frames.is_empty() && dispose == Dispose::Previous {
            // APNG: en el primer fotograma, `PREVIOUS` equivale a `BACKGROUND`.
            dispose = Dispose::Background;
        }
        let saved = (dispose == Dispose::Previous).then(|| self.canvas.clone());
        premultiply(&mut rgba);
        for row in 0..h {
            let cy = y + row;
            if cy >= self.height {
                break;
            }
            for col in 0..w {
                let cx = x + col;
                if cx >= self.width {
                    break;
                }
                let s = ((row * w + col) * 4) as usize;
                let Some(src) = rgba.get(s..s + 4) else {
                    break;
                };
                let d = ((cy * self.width + cx) * 4) as usize;
                let dst = &mut self.canvas[d..d + 4];
                match blend {
                    Blend::Source => dst.copy_from_slice(src),
                    Blend::Over => {
                        let inv = 255 - u16::from(src[3]);
                        for k in 0..4 {
                            dst[k] = (u16::from(src[k]) + (u16::from(dst[k]) * inv + 127) / 255)
                                .min(255) as u8;
                        }
                    }
                }
            }
        }
        self.frames.push(Frame {
            image: Image {
                width: self.width,
                height: self.height,
                rgba: self.canvas.clone(),
            },
            delay: ImageFrames::delay(delay),
            region: (x, y, w, h),
            dispose,
            blend,
        });
        self.bytes += size;
        self.pending = Some((dispose, (x, y, w, h), saved));
        true
    }

    fn fill_region(&mut self, (x, y, w, h): (u32, u32, u32, u32), color: Option<[u8; 4]>) {
        let c = color.unwrap_or([0; 4]);
        for cy in y..(y + h).min(self.height) {
            for cx in x..(x + w).min(self.width) {
                let d = ((cy * self.width + cx) * 4) as usize;
                self.canvas[d..d + 4].copy_from_slice(&c);
            }
        }
    }

    pub fn finish(self, plays: Option<u32>) -> ImageFrames {
        ImageFrames {
            width: self.width,
            height: self.height,
            frames: self.frames,
            plays,
        }
    }
}
