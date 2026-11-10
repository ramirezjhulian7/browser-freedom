//! Decodificadores y encoders del WHATWG Encoding Standard como máquinas de estado que procesan
//! una I/O queue (§3, "process a queue"/"process an item"): cada handler recibe un byte (o
//! code point) o end-of-queue y devuelve *continue*, *finished*, uno o dos code points (bytes)
//! o *error*. "Prepend" vuelve a meter bytes delante de la cola: aquí van a una pila
//! (`pending`) que se vacía antes de leer más entrada.
//!
//! Los decodificadores conservan su estado entre llamadas, así que sirven para el modo
//! `stream` de `TextDecoder` y para decodificar por trozos.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::encoding::Encoding;
use crate::encoding_mb_tables::{
    BIG5, EUC_KR, GB18030, GB18030_RANGES, ISO_2022_JP_KATAKANA, JIS0208, JIS0212,
};
use crate::encoding_tables::SINGLE_BYTE;

/// Resultado de un handler de decodificador.
enum Step {
    Continue,
    Finished,
    Cp(u32),
    /// Big5 devuelve dos code points para cuatro pointers (§11.1.1, paso 3.4).
    Two(u32, u32),
    Error,
}

/// Los estados del decodificador ISO-2022-JP (§12.2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Iso2022JpState {
    Ascii,
    Roman,
    Katakana,
    LeadByte,
    TrailByte,
    EscapeStart,
    Escape,
}

#[derive(Debug, Clone)]
enum State {
    /// UTF-8 decoder (§8.1.1).
    Utf8 {
        code_point: u32,
        seen: u8,
        needed: u8,
        lower: u8,
        upper: u8,
    },
    /// Shared UTF-16 decoder (§14.2.1).
    Utf16 {
        le: bool,
        lead_byte: Option<u8>,
        lead_surrogate: Option<u16>,
    },
    /// Single-byte decoder (§9.1).
    SingleByte(usize),
    /// x-user-defined decoder (§14.3.1).
    XUserDefined,
    /// replacement decoder (§14.1.1).
    Replacement { error_returned: bool },
    /// gb18030 decoder (§10.2.1); GBK usa el mismo (§10.1.1).
    Gb18030 { first: u8, second: u8, third: u8 },
    /// Big5 decoder (§11.1.1).
    Big5 { lead: u8 },
    /// EUC-JP decoder (§12.1.1).
    EucJp { jis0212: bool, lead: u8 },
    /// ISO-2022-JP decoder (§12.2.1).
    Iso2022Jp {
        state: Iso2022JpState,
        output_state: Iso2022JpState,
        lead: u8,
        output: bool,
    },
    /// Shift_JIS decoder (§12.3.1).
    ShiftJis { lead: u8 },
    /// EUC-KR decoder (§13.1.1).
    EucKr { lead: u8 },
}

/// Un decodificador con su estado (una "decoder instance").
#[derive(Debug, Clone)]
pub struct Decoder {
    encoding: Encoding,
    state: State,
    /// Bytes devueltos a la cola ("prepend"); el último es el siguiente.
    pending: Vec<u8>,
    finished: bool,
}

/// Un error de decodificación en modo `fatal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeError;

fn is_ascii(b: u8) -> bool {
    b < 0x80
}

impl Decoder {
    pub fn new(encoding: Encoding) -> Decoder {
        let state = match encoding {
            Encoding::Utf8 => State::Utf8 {
                code_point: 0,
                seen: 0,
                needed: 0,
                lower: 0x80,
                upper: 0xBF,
            },
            Encoding::Utf16Le | Encoding::Utf16Be => State::Utf16 {
                le: encoding == Encoding::Utf16Le,
                lead_byte: None,
                lead_surrogate: None,
            },
            Encoding::SingleByte(i) => State::SingleByte(i),
            Encoding::XUserDefined => State::XUserDefined,
            Encoding::Replacement => State::Replacement {
                error_returned: false,
            },
            Encoding::Gbk | Encoding::Gb18030 => State::Gb18030 {
                first: 0,
                second: 0,
                third: 0,
            },
            Encoding::Big5 => State::Big5 { lead: 0 },
            Encoding::EucJp => State::EucJp {
                jis0212: false,
                lead: 0,
            },
            Encoding::Iso2022Jp => State::Iso2022Jp {
                state: Iso2022JpState::Ascii,
                output_state: Iso2022JpState::Ascii,
                lead: 0,
                output: false,
            },
            Encoding::ShiftJis => State::ShiftJis { lead: 0 },
            Encoding::EucKr => State::EucKr { lead: 0 },
        };
        Decoder {
            encoding,
            state,
            pending: Vec::new(),
            finished: false,
        }
    }

    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    /// Procesa `input`; con `last`, también el end-of-queue (y el decodificador queda listo
    /// para otra pasada). Los code points van a `out`; los errores, como U+FFFD, salvo con
    /// `fatal`, que termina en `Err` en el primero ("process a queue", §3).
    pub fn decode(
        &mut self,
        input: &[u8],
        last: bool,
        fatal: bool,
        out: &mut String,
    ) -> Result<(), DecodeError> {
        let mut i = 0;
        let result = loop {
            if self.finished {
                break Ok(());
            }
            let item = if let Some(b) = self.pending.pop() {
                Some(b)
            } else if i < input.len() {
                i += 1;
                Some(input[i - 1])
            } else if last {
                None
            } else {
                return Ok(());
            };
            match self.step(item) {
                Step::Continue => {}
                Step::Finished => {
                    // Solo el replacement decoder termina antes del end-of-queue: el resto de
                    // la entrada se ignora.
                    self.finished = item.is_some();
                    break Ok(());
                }
                Step::Cp(cp) => out.push(char::from_u32(cp).unwrap_or('\u{FFFD}')),
                Step::Two(a, b) => {
                    out.push(char::from_u32(a).unwrap_or('\u{FFFD}'));
                    out.push(char::from_u32(b).unwrap_or('\u{FFFD}'));
                }
                Step::Error => {
                    if fatal {
                        break Err(DecodeError);
                    }
                    out.push('\u{FFFD}');
                }
            }
        };
        if last || result.is_err() {
            *self = Decoder::new(self.encoding);
        }
        result
    }

    fn step(&mut self, item: Option<u8>) -> Step {
        let pending = &mut self.pending;
        match &mut self.state {
            State::Utf8 {
                code_point,
                seen,
                needed,
                lower,
                upper,
            } => {
                let Some(byte) = item else {
                    if *needed != 0 {
                        *needed = 0;
                        return Step::Error;
                    }
                    return Step::Finished;
                };
                if *needed == 0 {
                    return match byte {
                        0x00..=0x7F => Step::Cp(u32::from(byte)),
                        0xC2..=0xDF => {
                            *needed = 1;
                            *code_point = u32::from(byte & 0x1F);
                            Step::Continue
                        }
                        0xE0..=0xEF => {
                            if byte == 0xE0 {
                                *lower = 0xA0;
                            }
                            if byte == 0xED {
                                *upper = 0x9F;
                            }
                            *needed = 2;
                            *code_point = u32::from(byte & 0x0F);
                            Step::Continue
                        }
                        0xF0..=0xF4 => {
                            if byte == 0xF0 {
                                *lower = 0x90;
                            }
                            if byte == 0xF4 {
                                *upper = 0x8F;
                            }
                            *needed = 3;
                            *code_point = u32::from(byte & 0x07);
                            Step::Continue
                        }
                        _ => Step::Error,
                    };
                }
                if !(*lower..=*upper).contains(&byte) {
                    *code_point = 0;
                    *needed = 0;
                    *seen = 0;
                    *lower = 0x80;
                    *upper = 0xBF;
                    pending.push(byte);
                    return Step::Error;
                }
                *lower = 0x80;
                *upper = 0xBF;
                *code_point = (*code_point << 6) | u32::from(byte & 0x3F);
                *seen += 1;
                if *seen != *needed {
                    return Step::Continue;
                }
                let cp = *code_point;
                *code_point = 0;
                *seen = 0;
                *needed = 0;
                Step::Cp(cp)
            }
            State::Utf16 {
                le,
                lead_byte,
                lead_surrogate,
            } => {
                let Some(byte) = item else {
                    if lead_byte.is_some() || lead_surrogate.is_some() {
                        *lead_byte = None;
                        *lead_surrogate = None;
                        return Step::Error;
                    }
                    return Step::Finished;
                };
                let Some(lead) = lead_byte.take() else {
                    *lead_byte = Some(byte);
                    return Step::Continue;
                };
                let cu = if *le {
                    u16::from(byte) << 8 | u16::from(lead)
                } else {
                    u16::from(lead) << 8 | u16::from(byte)
                };
                if let Some(ls) = lead_surrogate.take() {
                    if (0xDC00..0xE000).contains(&cu) {
                        return Step::Cp(
                            0x10000 + ((u32::from(ls) - 0xD800) << 10) + (u32::from(cu) - 0xDC00),
                        );
                    }
                    // "Prepend the result of converting code unit to bytes using is UTF-16BE
                    // decoder".
                    let [hi, lo] = cu.to_be_bytes();
                    if *le {
                        pending.push(hi);
                        pending.push(lo);
                    } else {
                        pending.push(lo);
                        pending.push(hi);
                    }
                    return Step::Error;
                }
                if (0xD800..0xDC00).contains(&cu) {
                    *lead_surrogate = Some(cu);
                    return Step::Continue;
                }
                if (0xDC00..0xE000).contains(&cu) {
                    return Step::Error;
                }
                Step::Cp(u32::from(cu))
            }
            State::SingleByte(i) => match item {
                None => Step::Finished,
                Some(b) if is_ascii(b) => Step::Cp(u32::from(b)),
                Some(b) => match SINGLE_BYTE[*i].1[usize::from(b - 0x80)] {
                    0 => Step::Error,
                    cp => Step::Cp(u32::from(cp)),
                },
            },
            State::XUserDefined => match item {
                None => Step::Finished,
                Some(b) if is_ascii(b) => Step::Cp(u32::from(b)),
                Some(b) => Step::Cp(0xF780 + u32::from(b) - 0x80),
            },
            State::Replacement { error_returned } => match item {
                None => Step::Finished,
                Some(_) if !*error_returned => {
                    *error_returned = true;
                    Step::Error
                }
                Some(_) => Step::Finished,
            },
            State::Gb18030 {
                first,
                second,
                third,
            } => {
                let Some(byte) = item else {
                    if *first == 0 && *second == 0 && *third == 0 {
                        return Step::Finished;
                    }
                    (*first, *second, *third) = (0, 0, 0);
                    return Step::Error;
                };
                if *third != 0 {
                    if !(0x30..=0x39).contains(&byte) {
                        pending.push(byte);
                        pending.push(*third);
                        pending.push(*second);
                        (*first, *second, *third) = (0, 0, 0);
                        return Step::Error;
                    }
                    let pointer = (u32::from(*first) - 0x81) * (10 * 126 * 10)
                        + (u32::from(*second) - 0x30) * (10 * 126)
                        + (u32::from(*third) - 0x81) * 10
                        + u32::from(byte)
                        - 0x30;
                    (*first, *second, *third) = (0, 0, 0);
                    return match gb18030_ranges_code_point(pointer) {
                        Some(cp) => Step::Cp(cp),
                        None => Step::Error,
                    };
                }
                if *second != 0 {
                    if (0x81..=0xFE).contains(&byte) {
                        *third = byte;
                        return Step::Continue;
                    }
                    pending.push(byte);
                    pending.push(*second);
                    (*first, *second) = (0, 0);
                    return Step::Error;
                }
                if *first != 0 {
                    if (0x30..=0x39).contains(&byte) {
                        *second = byte;
                        return Step::Continue;
                    }
                    let lead = *first;
                    *first = 0;
                    let offset = if byte < 0x7F { 0x40 } else { 0x41 };
                    let cp = if (0x40..=0x7E).contains(&byte) || (0x80..=0xFE).contains(&byte) {
                        let pointer =
                            (usize::from(lead) - 0x81) * 190 + (usize::from(byte) - offset);
                        index_u16(&GB18030, pointer)
                    } else {
                        None
                    };
                    if let Some(cp) = cp {
                        return Step::Cp(cp);
                    }
                    if is_ascii(byte) {
                        pending.push(byte);
                    }
                    return Step::Error;
                }
                match byte {
                    0x00..=0x7F => Step::Cp(u32::from(byte)),
                    0x80 => Step::Cp(0x20AC),
                    0x81..=0xFE => {
                        *first = byte;
                        Step::Continue
                    }
                    _ => Step::Error,
                }
            }
            State::Big5 { lead } => {
                let Some(byte) = item else {
                    if *lead != 0 {
                        *lead = 0;
                        return Step::Error;
                    }
                    return Step::Finished;
                };
                if *lead != 0 {
                    let l = *lead;
                    *lead = 0;
                    let offset = if byte < 0x7F { 0x40 } else { 0x62 };
                    let pointer = if (0x40..=0x7E).contains(&byte) || (0xA1..=0xFE).contains(&byte)
                    {
                        Some((usize::from(l) - 0x81) * 157 + (usize::from(byte) - offset))
                    } else {
                        None
                    };
                    match pointer {
                        Some(1133) => return Step::Two(0x00CA, 0x0304),
                        Some(1135) => return Step::Two(0x00CA, 0x030C),
                        Some(1164) => return Step::Two(0x00EA, 0x0304),
                        Some(1166) => return Step::Two(0x00EA, 0x030C),
                        _ => {}
                    }
                    let cp = pointer.and_then(|p| match BIG5.get(p) {
                        Some(&cp) if cp != 0 => Some(cp),
                        _ => None,
                    });
                    if let Some(cp) = cp {
                        return Step::Cp(cp);
                    }
                    if is_ascii(byte) {
                        pending.push(byte);
                    }
                    return Step::Error;
                }
                match byte {
                    0x00..=0x7F => Step::Cp(u32::from(byte)),
                    0x81..=0xFE => {
                        *lead = byte;
                        Step::Continue
                    }
                    _ => Step::Error,
                }
            }
            State::EucJp { jis0212, lead } => {
                let Some(byte) = item else {
                    if *lead != 0 {
                        *lead = 0;
                        return Step::Error;
                    }
                    return Step::Finished;
                };
                if *lead == 0x8E && (0xA1..=0xDF).contains(&byte) {
                    *lead = 0;
                    return Step::Cp(0xFF61 - 0xA1 + u32::from(byte));
                }
                if *lead == 0x8F && (0xA1..=0xFE).contains(&byte) {
                    *jis0212 = true;
                    *lead = byte;
                    return Step::Continue;
                }
                if *lead != 0 {
                    let l = *lead;
                    *lead = 0;
                    let mut cp = None;
                    if (0xA1..=0xFE).contains(&l) && (0xA1..=0xFE).contains(&byte) {
                        let pointer = (usize::from(l) - 0xA1) * 94 + usize::from(byte) - 0xA1;
                        cp = if *jis0212 {
                            index_u16(&JIS0212, pointer)
                        } else {
                            index_u16(&JIS0208, pointer)
                        };
                    }
                    *jis0212 = false;
                    if let Some(cp) = cp {
                        return Step::Cp(cp);
                    }
                    if is_ascii(byte) {
                        pending.push(byte);
                    }
                    return Step::Error;
                }
                match byte {
                    0x00..=0x7F => Step::Cp(u32::from(byte)),
                    0x8E | 0x8F | 0xA1..=0xFE => {
                        *lead = byte;
                        Step::Continue
                    }
                    _ => Step::Error,
                }
            }
            State::Iso2022Jp {
                state,
                output_state,
                lead,
                output,
            } => {
                use Iso2022JpState as S;
                match *state {
                    S::Ascii => match item {
                        Some(0x1B) => {
                            *state = S::EscapeStart;
                            Step::Continue
                        }
                        Some(b @ 0x00..=0x7F) if b != 0x0E && b != 0x0F => {
                            *output = false;
                            Step::Cp(u32::from(b))
                        }
                        None => Step::Finished,
                        Some(_) => {
                            *output = false;
                            Step::Error
                        }
                    },
                    S::Roman => match item {
                        Some(0x1B) => {
                            *state = S::EscapeStart;
                            Step::Continue
                        }
                        Some(0x5C) => {
                            *output = false;
                            Step::Cp(0xA5)
                        }
                        Some(0x7E) => {
                            *output = false;
                            Step::Cp(0x203E)
                        }
                        Some(b @ 0x00..=0x7F) if b != 0x0E && b != 0x0F => {
                            *output = false;
                            Step::Cp(u32::from(b))
                        }
                        None => Step::Finished,
                        Some(_) => {
                            *output = false;
                            Step::Error
                        }
                    },
                    S::Katakana => match item {
                        Some(0x1B) => {
                            *state = S::EscapeStart;
                            Step::Continue
                        }
                        Some(b @ 0x21..=0x5F) => {
                            *output = false;
                            Step::Cp(0xFF61 - 0x21 + u32::from(b))
                        }
                        None => Step::Finished,
                        Some(_) => {
                            *output = false;
                            Step::Error
                        }
                    },
                    S::LeadByte => match item {
                        Some(0x1B) => {
                            *state = S::EscapeStart;
                            Step::Continue
                        }
                        Some(b @ 0x21..=0x7E) => {
                            *output = false;
                            *lead = b;
                            *state = S::TrailByte;
                            Step::Continue
                        }
                        None => Step::Finished,
                        Some(_) => {
                            *output = false;
                            Step::Error
                        }
                    },
                    S::TrailByte => match item {
                        Some(0x1B) => {
                            *state = S::EscapeStart;
                            Step::Error
                        }
                        Some(b @ 0x21..=0x7E) => {
                            *state = S::LeadByte;
                            let pointer = (usize::from(*lead) - 0x21) * 94 + usize::from(b) - 0x21;
                            match index_u16(&JIS0208, pointer) {
                                Some(cp) => Step::Cp(cp),
                                None => Step::Error,
                            }
                        }
                        // End-of-queue: "prepend byte" (se vuelve a procesar el
                        // end-of-queue en el estado lead byte).
                        None => {
                            *state = S::LeadByte;
                            Step::Error
                        }
                        Some(_) => {
                            *state = S::LeadByte;
                            Step::Error
                        }
                    },
                    S::EscapeStart => {
                        if let Some(b @ (0x24 | 0x28)) = item {
                            *lead = b;
                            *state = S::Escape;
                            return Step::Continue;
                        }
                        if let Some(b) = item {
                            pending.push(b);
                        }
                        *output = false;
                        *state = *output_state;
                        Step::Error
                    }
                    S::Escape => {
                        let l = *lead;
                        *lead = 0;
                        let next = match (l, item) {
                            (0x28, Some(0x42)) => Some(S::Ascii),
                            (0x28, Some(0x4A)) => Some(S::Roman),
                            (0x28, Some(0x49)) => Some(S::Katakana),
                            (0x24, Some(0x40 | 0x42)) => Some(S::LeadByte),
                            _ => None,
                        };
                        if let Some(next) = next {
                            *state = next;
                            *output_state = next;
                            let was_output = *output;
                            *output = true;
                            return if was_output {
                                Step::Error
                            } else {
                                Step::Continue
                            };
                        }
                        if let Some(b) = item {
                            pending.push(b);
                        }
                        pending.push(l);
                        *output = false;
                        *state = *output_state;
                        Step::Error
                    }
                }
            }
            State::ShiftJis { lead } => {
                let Some(byte) = item else {
                    if *lead != 0 {
                        *lead = 0;
                        return Step::Error;
                    }
                    return Step::Finished;
                };
                if *lead != 0 {
                    let l = *lead;
                    *lead = 0;
                    let offset = if byte < 0x7F { 0x40 } else { 0x41 };
                    let lead_offset = if l < 0xA0 { 0x81 } else { 0xC1 };
                    let pointer = if (0x40..=0x7E).contains(&byte) || (0x80..=0xFC).contains(&byte)
                    {
                        Some((usize::from(l) - lead_offset) * 188 + usize::from(byte) - offset)
                    } else {
                        None
                    };
                    if let Some(p @ 8836..=10715) = pointer {
                        return Step::Cp(0xE000 - 8836 + p as u32);
                    }
                    if let Some(cp) = pointer.and_then(|p| index_u16(&JIS0208, p)) {
                        return Step::Cp(cp);
                    }
                    if is_ascii(byte) {
                        pending.push(byte);
                    }
                    return Step::Error;
                }
                match byte {
                    0x00..=0x80 => Step::Cp(u32::from(byte)),
                    0xA1..=0xDF => Step::Cp(0xFF61 - 0xA1 + u32::from(byte)),
                    0x81..=0x9F | 0xE0..=0xFC => {
                        *lead = byte;
                        Step::Continue
                    }
                    _ => Step::Error,
                }
            }
            State::EucKr { lead } => {
                let Some(byte) = item else {
                    if *lead != 0 {
                        *lead = 0;
                        return Step::Error;
                    }
                    return Step::Finished;
                };
                if *lead != 0 {
                    let l = *lead;
                    *lead = 0;
                    let cp = if (0x41..=0xFE).contains(&byte) {
                        let pointer = (usize::from(l) - 0x81) * 190 + (usize::from(byte) - 0x41);
                        index_u16(&EUC_KR, pointer)
                    } else {
                        None
                    };
                    if let Some(cp) = cp {
                        return Step::Cp(cp);
                    }
                    if is_ascii(byte) {
                        pending.push(byte);
                    }
                    return Step::Error;
                }
                match byte {
                    0x00..=0x7F => Step::Cp(u32::from(byte)),
                    0x81..=0xFE => {
                        *lead = byte;
                        Step::Continue
                    }
                    _ => Step::Error,
                }
            }
        }
    }
}

/// "Index code point" (§5) de un índice de `u16` (0 = null).
fn index_u16(index: &[u16], pointer: usize) -> Option<u32> {
    match index.get(pointer) {
        Some(&cp) if cp != 0 => Some(u32::from(cp)),
        _ => None,
    }
}

/// "Index gb18030 ranges code point" (§5).
fn gb18030_ranges_code_point(pointer: u32) -> Option<u32> {
    if (pointer > 39419 && pointer < 189000) || pointer > 1237575 {
        return None;
    }
    if pointer == 7457 {
        return Some(0xE7C7);
    }
    let i = GB18030_RANGES.partition_point(|&(p, _)| p <= pointer) - 1;
    let (offset, cp_offset) = GB18030_RANGES[i];
    Some(cp_offset + pointer - offset)
}

/// "Index gb18030 ranges pointer" (§5).
fn gb18030_ranges_pointer(cp: u32) -> u32 {
    if cp == 0xE7C7 {
        return 7457;
    }
    let i = GB18030_RANGES.partition_point(|&(_, c)| c <= cp) - 1;
    let (pointer_offset, offset) = GB18030_RANGES[i];
    pointer_offset + cp - offset
}

/// "Index pointer" (§5): el primer pointer de cada code point, saltando los pointers de
/// `skip`.
fn reverse_u16(index: &[u16], skip: impl Fn(usize) -> bool) -> HashMap<u32, u32> {
    let mut map = HashMap::with_capacity(index.len());
    for (p, &cp) in index.iter().enumerate() {
        if cp != 0 && !skip(p) {
            map.entry(u32::from(cp)).or_insert(p as u32);
        }
    }
    map
}

fn jis0208_pointer(cp: u32) -> Option<u32> {
    static MAP: OnceLock<HashMap<u32, u32>> = OnceLock::new();
    MAP.get_or_init(|| reverse_u16(&JIS0208, |_| false))
        .get(&cp)
        .copied()
}

/// "Index Shift_JIS pointer" (§5): index jis0208 sin los pointers 8272..=8835.
fn shift_jis_pointer(cp: u32) -> Option<u32> {
    static MAP: OnceLock<HashMap<u32, u32>> = OnceLock::new();
    MAP.get_or_init(|| reverse_u16(&JIS0208, |p| (8272..=8835).contains(&p)))
        .get(&cp)
        .copied()
}

fn euc_kr_pointer(cp: u32) -> Option<u32> {
    static MAP: OnceLock<HashMap<u32, u32>> = OnceLock::new();
    MAP.get_or_init(|| reverse_u16(&EUC_KR, |_| false))
        .get(&cp)
        .copied()
}

fn gb18030_pointer(cp: u32) -> Option<u32> {
    static MAP: OnceLock<HashMap<u32, u32>> = OnceLock::new();
    MAP.get_or_init(|| reverse_u16(&GB18030, |_| false))
        .get(&cp)
        .copied()
}

/// "Index Big5 pointer" (§5): sin los pointers de HKSCS anteriores a (0xA1 − 0x81) × 157, y
/// el último pointer para U+2550, U+255E, U+2561, U+256A, U+5341 y U+5345.
fn big5_pointer(cp: u32) -> Option<u32> {
    static MAP: OnceLock<HashMap<u32, u32>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut map = HashMap::with_capacity(BIG5.len());
        let last = [0x2550, 0x255E, 0x2561, 0x256A, 0x5341, 0x5345];
        for (p, &c) in BIG5.iter().enumerate().skip((0xA1 - 0x81) * 157) {
            if c == 0 {
                continue;
            }
            if last.contains(&c) {
                map.insert(c, p as u32);
            } else {
                map.entry(c).or_insert(p as u32);
            }
        }
        map
    })
    .get(&cp)
    .copied()
}

/// Los estados del encoder ISO-2022-JP (§12.2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Iso2022JpEncoderState {
    Ascii,
    Roman,
    Jis0208,
}

/// Resultado de un handler de encoder.
enum EncStep {
    /// El code point quedó codificado (o no producía bytes).
    Done,
    /// "Prepend code point": volver a procesarlo.
    Again,
    Finished,
    /// Error con el code point dado.
    Error(u32),
}

/// Un encoder con su estado (una "encoder instance"). Solo existen para los encodings de
/// salida (§4.3).
#[derive(Debug, Clone)]
pub struct Encoder {
    encoding: Encoding,
    iso_state: Iso2022JpEncoderState,
}

impl Encoder {
    /// El encoder del output encoding de `encoding` (UTF-16 y replacement codifican como
    /// UTF-8).
    pub fn new(encoding: Encoding) -> Encoder {
        Encoder {
            encoding: encoding.output_encoding(),
            iso_state: Iso2022JpEncoderState::Ascii,
        }
    }

    /// "Encode or fail" (§4.6 del spec, sobre una cola de entrada): codifica code points de
    /// `input` hasta el primero no representable (que devuelve, ya consumido) o hasta el
    /// final, donde procesa el end-of-queue y devuelve `None`.
    pub fn encode_or_fail(
        &mut self,
        input: &mut impl Iterator<Item = char>,
        out: &mut Vec<u8>,
    ) -> Option<u32> {
        self.encode_items(input, true, out)
    }

    /// Como [`Encoder::encode_or_fail`]; sin `last`, al acabarse `input` vuelve sin procesar
    /// el end-of-queue (el estado queda para la siguiente llamada).
    pub fn encode_items(
        &mut self,
        input: &mut impl Iterator<Item = char>,
        last: bool,
        out: &mut Vec<u8>,
    ) -> Option<u32> {
        let mut again: Option<char> = None;
        loop {
            let item = again.take().or_else(|| input.next());
            if item.is_none() && !last {
                return None;
            }
            match self.step(item, out) {
                EncStep::Done => {}
                EncStep::Again => again = item,
                EncStep::Finished => return None,
                EncStep::Error(cp) => return Some(cp),
            }
        }
    }

    fn step(&mut self, item: Option<char>, out: &mut Vec<u8>) -> EncStep {
        let Some(c) = item else {
            // End-of-queue: solo ISO-2022-JP vuelve al estado ASCII.
            if self.encoding == Encoding::Iso2022Jp
                && self.iso_state != Iso2022JpEncoderState::Ascii
            {
                self.iso_state = Iso2022JpEncoderState::Ascii;
                out.extend_from_slice(&[0x1B, 0x28, 0x42]);
                return EncStep::Again;
            }
            return EncStep::Finished;
        };
        let mut cp = c as u32;
        let ascii = cp < 0x80;
        match self.encoding {
            // UTF-8 encoder (§8.2.1).
            Encoding::Utf8 | Encoding::Utf16Le | Encoding::Utf16Be | Encoding::Replacement => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                EncStep::Done
            }
            // Single-byte encoder (§9.2).
            Encoding::SingleByte(i) => {
                if ascii {
                    out.push(cp as u8);
                    return EncStep::Done;
                }
                match SINGLE_BYTE[i]
                    .1
                    .iter()
                    .position(|&t| t != 0 && u32::from(t) == cp)
                {
                    Some(p) => {
                        out.push(0x80 + p as u8);
                        EncStep::Done
                    }
                    None => EncStep::Error(cp),
                }
            }
            // x-user-defined encoder (§14.3.2).
            Encoding::XUserDefined => {
                if ascii {
                    out.push(cp as u8);
                } else if (0xF780..=0xF7FF).contains(&cp) {
                    out.push((cp - 0xF780 + 0x80) as u8);
                } else {
                    return EncStep::Error(cp);
                }
                EncStep::Done
            }
            // gb18030 encoder (§10.2.2); GBK es el mismo con "is GBK" (§10.1.2).
            Encoding::Gbk | Encoding::Gb18030 => {
                let gbk = self.encoding == Encoding::Gbk;
                if ascii {
                    out.push(cp as u8);
                    return EncStep::Done;
                }
                if cp == 0xE5E5 {
                    return EncStep::Error(cp);
                }
                if gbk && cp == 0x20AC {
                    out.push(0x80);
                    return EncStep::Done;
                }
                if let Some(&(_, bytes)) = GB18030_PUA.iter().find(|(c, _)| *c == cp) {
                    out.extend_from_slice(&bytes);
                    return EncStep::Done;
                }
                if let Some(pointer) = gb18030_pointer(cp) {
                    let lead = pointer / 190 + 0x81;
                    let trail = pointer % 190;
                    let offset = if trail < 0x3F { 0x40 } else { 0x41 };
                    out.extend_from_slice(&[lead as u8, (trail + offset) as u8]);
                    return EncStep::Done;
                }
                if gbk {
                    return EncStep::Error(cp);
                }
                let mut pointer = gb18030_ranges_pointer(cp);
                let b1 = pointer / (10 * 126 * 10);
                pointer %= 10 * 126 * 10;
                let b2 = pointer / (10 * 126);
                pointer %= 10 * 126;
                let b3 = pointer / 10;
                let b4 = pointer % 10;
                out.extend_from_slice(&[
                    (b1 + 0x81) as u8,
                    (b2 + 0x30) as u8,
                    (b3 + 0x81) as u8,
                    (b4 + 0x30) as u8,
                ]);
                EncStep::Done
            }
            // Big5 encoder (§11.1.2).
            Encoding::Big5 => {
                if ascii {
                    out.push(cp as u8);
                    return EncStep::Done;
                }
                let Some(pointer) = big5_pointer(cp) else {
                    return EncStep::Error(cp);
                };
                let lead = pointer / 157 + 0x81;
                let trail = pointer % 157;
                let offset = if trail < 0x3F { 0x40 } else { 0x62 };
                out.extend_from_slice(&[lead as u8, (trail + offset) as u8]);
                EncStep::Done
            }
            // EUC-JP encoder (§12.1.2).
            Encoding::EucJp => {
                if ascii {
                    out.push(cp as u8);
                    return EncStep::Done;
                }
                match cp {
                    0xA5 => out.push(0x5C),
                    0x203E => out.push(0x7E),
                    0xFF61..=0xFF9F => out.extend_from_slice(&[0x8E, (cp - 0xFF61 + 0xA1) as u8]),
                    _ => {
                        if cp == 0x2212 {
                            cp = 0xFF0D;
                        }
                        let Some(pointer) = jis0208_pointer(cp) else {
                            return EncStep::Error(cp);
                        };
                        out.extend_from_slice(&[
                            (pointer / 94 + 0xA1) as u8,
                            (pointer % 94 + 0xA1) as u8,
                        ]);
                    }
                }
                EncStep::Done
            }
            // ISO-2022-JP encoder (§12.2.2).
            Encoding::Iso2022Jp => {
                use Iso2022JpEncoderState as S;
                let state = self.iso_state;
                if matches!(state, S::Ascii | S::Roman) && matches!(cp, 0x0E | 0x0F | 0x1B) {
                    return EncStep::Error(0xFFFD);
                }
                if state == S::Ascii && ascii {
                    out.push(cp as u8);
                    return EncStep::Done;
                }
                if state == S::Roman
                    && ((ascii && cp != 0x5C && cp != 0x7E) || cp == 0xA5 || cp == 0x203E)
                {
                    out.push(match cp {
                        0xA5 => 0x5C,
                        0x203E => 0x7E,
                        _ => cp as u8,
                    });
                    return EncStep::Done;
                }
                if ascii && state != S::Ascii {
                    self.iso_state = S::Ascii;
                    out.extend_from_slice(&[0x1B, 0x28, 0x42]);
                    return EncStep::Again;
                }
                if (cp == 0xA5 || cp == 0x203E) && state != S::Roman {
                    self.iso_state = S::Roman;
                    out.extend_from_slice(&[0x1B, 0x28, 0x4A]);
                    return EncStep::Again;
                }
                if cp == 0x2212 {
                    cp = 0xFF0D;
                }
                if (0xFF61..=0xFF9F).contains(&cp) {
                    cp = u32::from(ISO_2022_JP_KATAKANA[(cp - 0xFF61) as usize]);
                }
                let Some(pointer) = jis0208_pointer(cp) else {
                    if state == S::Jis0208 {
                        self.iso_state = S::Ascii;
                        out.extend_from_slice(&[0x1B, 0x28, 0x42]);
                        return EncStep::Again;
                    }
                    return EncStep::Error(cp);
                };
                if state != S::Jis0208 {
                    self.iso_state = S::Jis0208;
                    out.extend_from_slice(&[0x1B, 0x24, 0x42]);
                    return EncStep::Again;
                }
                out.extend_from_slice(&[(pointer / 94 + 0x21) as u8, (pointer % 94 + 0x21) as u8]);
                EncStep::Done
            }
            // Shift_JIS encoder (§12.3.2).
            Encoding::ShiftJis => {
                if ascii || cp == 0x80 {
                    out.push(cp as u8);
                    return EncStep::Done;
                }
                match cp {
                    0xA5 => out.push(0x5C),
                    0x203E => out.push(0x7E),
                    0xFF61..=0xFF9F => out.push((cp - 0xFF61 + 0xA1) as u8),
                    _ => {
                        if cp == 0x2212 {
                            cp = 0xFF0D;
                        }
                        let Some(pointer) = shift_jis_pointer(cp) else {
                            return EncStep::Error(cp);
                        };
                        let lead = pointer / 188;
                        let lead_offset = if lead < 0x1F { 0x81 } else { 0xC1 };
                        let trail = pointer % 188;
                        let offset = if trail < 0x3F { 0x40 } else { 0x41 };
                        out.extend_from_slice(&[
                            (lead + lead_offset) as u8,
                            (trail + offset) as u8,
                        ]);
                    }
                }
                EncStep::Done
            }
            // EUC-KR encoder (§13.1.2).
            Encoding::EucKr => {
                if ascii {
                    out.push(cp as u8);
                    return EncStep::Done;
                }
                let Some(pointer) = euc_kr_pointer(cp) else {
                    return EncStep::Error(cp);
                };
                out.extend_from_slice(&[
                    (pointer / 190 + 0x81) as u8,
                    (pointer % 190 + 0x41) as u8,
                ]);
                EncStep::Done
            }
        }
    }
}

/// gb18030 encoder (§10.2.2, paso 5): los code points de uso privado que GB18030-2005 ponía en
/// estos bytes y que GB18030-2022 mapea a otros code points siguen codificándose aquí.
const GB18030_PUA: [(u32, [u8; 2]); 18] = [
    (0xE78D, [0xA6, 0xD9]),
    (0xE78E, [0xA6, 0xDA]),
    (0xE78F, [0xA6, 0xDB]),
    (0xE790, [0xA6, 0xDC]),
    (0xE791, [0xA6, 0xDD]),
    (0xE792, [0xA6, 0xDE]),
    (0xE793, [0xA6, 0xDF]),
    (0xE794, [0xA6, 0xEC]),
    (0xE795, [0xA6, 0xED]),
    (0xE796, [0xA6, 0xF3]),
    (0xE81E, [0xFE, 0x59]),
    (0xE826, [0xFE, 0x61]),
    (0xE82B, [0xFE, 0x66]),
    (0xE82C, [0xFE, 0x67]),
    (0xE832, [0xFE, 0x6D]),
    (0xE843, [0xFE, 0x7E]),
    (0xE854, [0xFE, 0x90]),
    (0xE864, [0xFE, 0xA0]),
];
