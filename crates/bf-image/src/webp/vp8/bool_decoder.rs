//! Decodificador booleano (aritmético binario) de VP8 (RFC 6386 §7).
//!
//! `value` guarda la ventana de 8 bits alineada con `range` seguida de `bits` bits de
//! adelanto. Al agotarse los datos se rellena una vez con ceros y se marca `eof`, como hace
//! libwebp: el llamador lo comprueba tras cada macrobloque.

pub(in crate::webp) struct BoolDecoder<'a> {
    data: &'a [u8],
    pos: usize,
    value: u64,
    bits: i32,
    range: u32,
    eof: bool,
    /// Bits leídos con su probabilidad (para recodificar flujos en las pruebas).
    #[cfg(test)]
    pub(in crate::webp) log: Vec<(u8, bool)>,
}

impl<'a> BoolDecoder<'a> {
    pub(in crate::webp) fn new(data: &'a [u8]) -> BoolDecoder<'a> {
        BoolDecoder {
            data,
            pos: 0,
            value: 0,
            bits: -8,
            range: 255,
            eof: false,
            #[cfg(test)]
            log: Vec::new(),
        }
    }

    /// ¿Se intentó leer más allá del final?
    pub(in crate::webp) fn eof(&self) -> bool {
        self.eof
    }

    fn load(&mut self) {
        if let Some(chunk) = self.data.get(self.pos..self.pos + 6) {
            for &b in chunk {
                self.value = (self.value << 8) | u64::from(b);
            }
            self.pos += 6;
            self.bits += 48;
        } else if let Some(&b) = self.data.get(self.pos) {
            self.value = (self.value << 8) | u64::from(b);
            self.pos += 1;
            self.bits += 8;
        } else if !self.eof {
            self.value <<= 8;
            self.bits += 8;
            self.eof = true;
        } else {
            self.bits = 0;
        }
    }

    /// Lee un bit cuya probabilidad de ser 0 es `prob / 256` (§7.3).
    pub(in crate::webp) fn get(&mut self, prob: u8) -> bool {
        if self.bits < 0 {
            self.load();
        }
        let split = 1 + (((self.range - 1) * u32::from(prob)) >> 8);
        let bit = (self.value >> self.bits) >= u64::from(split);
        if bit {
            self.range -= split;
            self.value -= u64::from(split) << self.bits;
        } else {
            self.range = split;
        }
        let shift = self.range.leading_zeros().saturating_sub(24);
        self.range <<= shift;
        self.bits -= shift as i32;
        #[cfg(test)]
        self.log.push((prob, bit));
        bit
    }

    /// Entero sin signo de `n` bits, el más significativo primero (`L(n)`, §19.1).
    pub(in crate::webp) fn literal(&mut self, n: u32) -> u32 {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | u32::from(self.get(128));
        }
        v
    }

    /// Magnitud de `n` bits seguida del signo.
    pub(in crate::webp) fn signed(&mut self, n: u32) -> i32 {
        let v = self.literal(n) as i32;
        if self.get(128) { -v } else { v }
    }

    /// `L(1)`.
    pub(in crate::webp) fn flag(&mut self) -> bool {
        self.get(128)
    }
}

/// Codificador booleano para construir flujos de prueba.
#[cfg(test)]
pub(in crate::webp) mod encoder {
    /// Codificador booleano de referencia (RFC 6386 §7.3) para comprobar el decodificador.
    pub(in crate::webp) struct Encoder {
        out: Vec<u8>,
        range: u32,
        bottom: u32,
        bit_count: i32,
    }

    impl Encoder {
        pub(in crate::webp) fn new() -> Encoder {
            Encoder {
                out: Vec::new(),
                range: 255,
                bottom: 0,
                bit_count: 24,
            }
        }

        fn add_one(&mut self) {
            let mut i = self.out.len();
            while i > 0 {
                i -= 1;
                if self.out[i] == 255 {
                    self.out[i] = 0;
                } else {
                    self.out[i] += 1;
                    break;
                }
            }
        }

        pub(in crate::webp) fn put(&mut self, prob: u8, bit: bool) {
            let split = 1 + (((self.range - 1) * u32::from(prob)) >> 8);
            if bit {
                self.bottom = self.bottom.wrapping_add(split);
                self.range -= split;
            } else {
                self.range = split;
            }
            while self.range < 128 {
                self.range <<= 1;
                if self.bottom & (1 << 31) != 0 {
                    self.add_one();
                }
                self.bottom <<= 1;
                self.bit_count -= 1;
                if self.bit_count == 0 {
                    self.out.push((self.bottom >> 24) as u8);
                    self.bottom &= (1 << 24) - 1;
                    self.bit_count = 8;
                }
            }
        }

        pub(in crate::webp) fn finish(mut self) -> Vec<u8> {
            for _ in 0..32 {
                self.put(128, false);
            }
            self.out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::encoder::Encoder;
    use super::*;

    #[test]
    fn roundtrip() {
        let mut seed = 12345u32;
        let mut bits = Vec::new();
        let mut enc = Encoder::new();
        for _ in 0..5000 {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let prob = ((seed >> 16) % 255 + 1) as u8;
            let bit = (seed >> 8) % 256 >= u32::from(prob);
            enc.put(prob, bit);
            bits.push((prob, bit));
        }
        let data = enc.finish();
        let mut dec = BoolDecoder::new(&data);
        for (prob, bit) in bits {
            assert_eq!(dec.get(prob), bit);
        }
        assert!(!dec.eof());
    }
}
