//! E1: SHA-1, SHA-256, SHA-384 y SHA-512 (FIPS 180-4) con estado incremental, para que HMAC
//! (FIPS 198-1) pueda guardar el estado tras el bloque `K ^ ipad` y `K ^ opad` y no volver a
//! comprimirlo en cada iteración de PBKDF2 (RFC 8018 §5.2), que en la web se usa con 100 000
//! iteraciones o más.

use super::Hash;

const K256: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const K512: [u64; 80] = [
    0x428a2f98d728ae22,
    0x7137449123ef65cd,
    0xb5c0fbcfec4d3b2f,
    0xe9b5dba58189dbbc,
    0x3956c25bf348b538,
    0x59f111f1b605d019,
    0x923f82a4af194f9b,
    0xab1c5ed5da6d8118,
    0xd807aa98a3030242,
    0x12835b0145706fbe,
    0x243185be4ee4b28c,
    0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f,
    0x80deb1fe3b1696b1,
    0x9bdc06a725c71235,
    0xc19bf174cf692694,
    0xe49b69c19ef14ad2,
    0xefbe4786384f25e3,
    0x0fc19dc68b8cd5b5,
    0x240ca1cc77ac9c65,
    0x2de92c6f592b0275,
    0x4a7484aa6ea6e483,
    0x5cb0a9dcbd41fbd4,
    0x76f988da831153b5,
    0x983e5152ee66dfab,
    0xa831c66d2db43210,
    0xb00327c898fb213f,
    0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2,
    0xd5a79147930aa725,
    0x06ca6351e003826f,
    0x142929670a0e6e70,
    0x27b70a8546d22ffc,
    0x2e1b21385c26c926,
    0x4d2c6dfc5ac42aed,
    0x53380d139d95b3df,
    0x650a73548baf63de,
    0x766a0abb3c77b2a8,
    0x81c2c92e47edaee6,
    0x92722c851482353b,
    0xa2bfe8a14cf10364,
    0xa81a664bbc423001,
    0xc24b8b70d0f89791,
    0xc76c51a30654be30,
    0xd192e819d6ef5218,
    0xd69906245565a910,
    0xf40e35855771202a,
    0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8,
    0x1e376c085141ab53,
    0x2748774cdf8eeb99,
    0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63,
    0x4ed8aa4ae3418acb,
    0x5b9cca4f7763e373,
    0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc,
    0x78a5636f43172f60,
    0x84c87814a1f0ab72,
    0x8cc702081a6439ec,
    0x90befffa23631e28,
    0xa4506cebde82bde9,
    0xbef9a3f7b2c67915,
    0xc67178f2e372532b,
    0xca273eceea26619c,
    0xd186b8c721c0c207,
    0xeada7dd6cde0eb1e,
    0xf57d4f7fee6ed178,
    0x06f067aa72176fba,
    0x0a637dc5a2c898a6,
    0x113f9804bef90dae,
    0x1b710b35131c471b,
    0x28db77f523047d84,
    0x32caab7b40c72493,
    0x3c9ebe0a15c9bebc,
    0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6,
    0x597f299cfc657e2a,
    0x5fcb6fab3ad6faec,
    0x6c44198c4a475817,
];

#[derive(Clone)]
enum State {
    S1([u32; 5]),
    S256([u32; 8]),
    S512([u64; 8]),
}

/// Un hash incremental: `update` y después `finish`. Se clona para guardar un estado intermedio.
#[derive(Clone)]
pub struct Hasher {
    hash: Hash,
    state: State,
    buf: [u8; 128],
    pos: usize,
    len: u64,
}

impl Hasher {
    pub fn new(hash: Hash) -> Hasher {
        let state = match hash {
            Hash::Sha1 => State::S1([0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0]),
            Hash::Sha256 => State::S256([
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ]),
            Hash::Sha384 => State::S512([
                0xcbbb9d5dc1059ed8,
                0x629a292a367cd507,
                0x9159015a3070dd17,
                0x152fecd8f70e5939,
                0x67332667ffc00b31,
                0x8eb44a8768581511,
                0xdb0c2e0d64f98fa7,
                0x47b5481dbefa4fa4,
            ]),
            Hash::Sha512 => State::S512([
                0x6a09e667f3bcc908,
                0xbb67ae8584caa73b,
                0x3c6ef372fe94f82b,
                0xa54ff53a5f1d36f1,
                0x510e527fade682d1,
                0x9b05688c2b3e6c1f,
                0x1f83d9abfb41bd6b,
                0x5be0cd19137e2179,
            ]),
        };
        Hasher {
            hash,
            state,
            buf: [0; 128],
            pos: 0,
            len: 0,
        }
    }

    fn block_len(&self) -> usize {
        match self.state {
            State::S512(_) => 128,
            _ => 64,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        let bl = self.block_len();
        self.len = self.len.wrapping_add(data.len() as u64);
        while !data.is_empty() {
            let n = (bl - self.pos).min(data.len());
            self.buf[self.pos..self.pos + n].copy_from_slice(&data[..n]);
            self.pos += n;
            data = &data[n..];
            if self.pos == bl {
                let block = self.buf;
                self.compress(&block[..bl]);
                self.pos = 0;
            }
        }
    }

    /// El relleno de §5.1 (un 1, ceros y la longitud en bits) y la salida truncada de §6.
    pub fn finish(mut self) -> Vec<u8> {
        let bl = self.block_len();
        let len_bytes = if bl == 128 { 16 } else { 8 };
        let bits = u128::from(self.len) * 8;
        let mut pad = vec![0x80u8];
        let used = (self.pos + 1) % bl;
        let zeros = (bl + bl - len_bytes - used) % bl;
        pad.resize(1 + zeros, 0);
        pad.extend_from_slice(&bits.to_be_bytes()[16 - len_bytes..]);
        let len = self.len;
        self.update(&pad);
        self.len = len;
        let mut out = Vec::with_capacity(64);
        match self.state {
            State::S1(h) => h
                .iter()
                .for_each(|w| out.extend_from_slice(&w.to_be_bytes())),
            State::S256(h) => h
                .iter()
                .for_each(|w| out.extend_from_slice(&w.to_be_bytes())),
            State::S512(h) => h
                .iter()
                .for_each(|w| out.extend_from_slice(&w.to_be_bytes())),
        }
        out.truncate(match self.hash {
            Hash::Sha1 => 20,
            Hash::Sha256 => 32,
            Hash::Sha384 => 48,
            Hash::Sha512 => 64,
        });
        out
    }

    fn compress(&mut self, block: &[u8]) {
        match &mut self.state {
            State::S1(h) => compress1(h, block),
            State::S256(h) => compress256(h, block),
            State::S512(h) => compress512(h, block),
        }
    }
}

/// FIPS 180-4 §6.1.2.
fn compress1(h: &mut [u32; 5], block: &[u8]) {
    let mut w = [0u32; 80];
    for (t, c) in block.as_chunks::<4>().0.iter().enumerate() {
        w[t] = u32::from_be_bytes(*c);
    }
    for t in 16..80 {
        w[t] = (w[t - 3] ^ w[t - 8] ^ w[t - 14] ^ w[t - 16]).rotate_left(1);
    }
    let [mut a, mut b, mut c, mut d, mut e] = *h;
    for (t, wt) in w.iter().enumerate() {
        let (f, k) = match t {
            0..=19 => ((b & c) | (!b & d), 0x5a827999),
            20..=39 => (b ^ c ^ d, 0x6ed9eba1),
            40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdc),
            _ => (b ^ c ^ d, 0xca62c1d6),
        };
        let tmp = a
            .rotate_left(5)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(*wt);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = tmp;
    }
    for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
        *x = x.wrapping_add(y);
    }
}

/// FIPS 180-4 §6.2.2.
fn compress256(h: &mut [u32; 8], block: &[u8]) {
    let mut w = [0u32; 64];
    for (t, c) in block.as_chunks::<4>().0.iter().enumerate() {
        w[t] = u32::from_be_bytes(*c);
    }
    for t in 16..64 {
        let s0 = w[t - 15].rotate_right(7) ^ w[t - 15].rotate_right(18) ^ (w[t - 15] >> 3);
        let s1 = w[t - 2].rotate_right(17) ^ w[t - 2].rotate_right(19) ^ (w[t - 2] >> 10);
        w[t] = w[t - 16]
            .wrapping_add(s0)
            .wrapping_add(w[t - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
    for (k, wt) in K256.iter().zip(w) {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = hh
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(*k)
            .wrapping_add(wt);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
        *x = x.wrapping_add(y);
    }
}

/// FIPS 180-4 §6.4.2 (SHA-384 es lo mismo con otro valor inicial).
fn compress512(h: &mut [u64; 8], block: &[u8]) {
    let mut w = [0u64; 80];
    for (t, c) in block.as_chunks::<8>().0.iter().enumerate() {
        w[t] = u64::from_be_bytes(*c);
    }
    for t in 16..80 {
        let s0 = w[t - 15].rotate_right(1) ^ w[t - 15].rotate_right(8) ^ (w[t - 15] >> 7);
        let s1 = w[t - 2].rotate_right(19) ^ w[t - 2].rotate_right(61) ^ (w[t - 2] >> 6);
        w[t] = w[t - 16]
            .wrapping_add(s0)
            .wrapping_add(w[t - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
    for (k, wt) in K512.iter().zip(w) {
        let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
        let ch = (e & f) ^ (!e & g);
        let t1 = hh
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(*k)
            .wrapping_add(wt);
        let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
        *x = x.wrapping_add(y);
    }
}

/// HMAC (FIPS 198-1) con los estados tras `K' ^ ipad` y `K' ^ opad` ya calculados.
#[derive(Clone)]
pub struct Hmac {
    inner: Hasher,
    outer: Hasher,
}

impl Hmac {
    pub fn new(hash: Hash, key: &[u8]) -> Hmac {
        let block = hash.block_len();
        let mut k = if key.len() > block {
            let mut h = Hasher::new(hash);
            h.update(key);
            h.finish()
        } else {
            key.to_vec()
        };
        k.resize(block, 0);
        let mut inner = Hasher::new(hash);
        inner.update(&k.iter().map(|b| b ^ 0x36).collect::<Vec<u8>>());
        let mut outer = Hasher::new(hash);
        outer.update(&k.iter().map(|b| b ^ 0x5c).collect::<Vec<u8>>());
        Hmac { inner, outer }
    }

    pub fn mac(&self, data: &[u8]) -> Vec<u8> {
        let mut i = self.inner.clone();
        i.update(data);
        let mut o = self.outer.clone();
        o.update(&i.finish());
        o.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El mismo resultado que los hash de `bf-net` (que ya tienen los vectores de FIPS 180-4)
    /// para longitudes que cruzan los bordes del relleno.
    #[test]
    fn matches_reference_digests() {
        for h in [Hash::Sha1, Hash::Sha256, Hash::Sha384, Hash::Sha512] {
            for n in [0usize, 1, 55, 56, 63, 64, 65, 111, 112, 127, 128, 129, 300] {
                let data: Vec<u8> = (0..n).map(|i| (i * 7 + 3) as u8).collect();
                let mut s = Hasher::new(h);
                let (a, b) = data.split_at(n / 3);
                s.update(a);
                s.update(b);
                assert_eq!(s.finish(), super::super::digest(h, &data), "{h:?} {n}");
            }
        }
    }
}
