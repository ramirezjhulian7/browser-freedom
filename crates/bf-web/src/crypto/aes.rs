//! E1: AES (FIPS 197) y los modos CBC, CTR (NIST SP 800-38A) y GCM (NIST SP 800-38D), para
//! `crypto.subtle` (Web Cryptography §28). Código propio y sin `unsafe`.
//!
//! Las tablas (S-box y su inversa) se calculan una vez a partir de la aritmética de GF(2⁸)
//! (FIPS 197 §4.2 y §5.1.1) en vez de copiarlas a mano. Las búsquedas en tablas no son de tiempo
//! constante: vale para la compatibilidad web, no para usos donde importen los canales laterales.

use std::sync::OnceLock;

/// La multiplicación en GF(2⁸) con el polinomio de AES (FIPS 197 §4.2).
fn gmul(mut a: u8, mut b: u8) -> u8 {
    let mut p = 0u8;
    while b != 0 {
        if b & 1 != 0 {
            p ^= a;
        }
        let hi = a & 0x80;
        a <<= 1;
        if hi != 0 {
            a ^= 0x1b;
        }
        b >>= 1;
    }
    p
}

struct Tables {
    sbox: [u8; 256],
    inv: [u8; 256],
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let mut sbox = [0u8; 256];
        let mut inv = [0u8; 256];
        for (i, slot) in sbox.iter_mut().enumerate() {
            // El inverso multiplicativo (0 para 0) y la transformación afín (§5.1.1).
            let b = if i == 0 {
                0
            } else {
                (1..=255u8).find(|&x| gmul(i as u8, x) == 1).unwrap_or(0)
            };
            let v = b
                ^ b.rotate_left(1)
                ^ b.rotate_left(2)
                ^ b.rotate_left(3)
                ^ b.rotate_left(4)
                ^ 0x63;
            *slot = v;
            inv[v as usize] = i as u8;
        }
        Tables { sbox, inv }
    })
}

/// Una clave AES expandida (FIPS 197 §5.2).
pub struct Aes {
    /// Las claves de ronda, `Nr + 1` bloques de 16 bytes.
    round_keys: Vec<[u8; 16]>,
}

impl Aes {
    /// `None` si la clave no es de 16, 24 o 32 bytes.
    pub fn new(key: &[u8]) -> Option<Aes> {
        let nk = match key.len() {
            16 | 24 | 32 => key.len() / 4,
            _ => return None,
        };
        let nr = nk + 6;
        let t = tables();
        let mut w: Vec<[u8; 4]> = key.chunks(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
        let mut rcon = 1u8;
        for i in nk..4 * (nr + 1) {
            let mut temp = w[i - 1];
            if i % nk == 0 {
                temp = [temp[1], temp[2], temp[3], temp[0]];
                for b in &mut temp {
                    *b = t.sbox[*b as usize];
                }
                temp[0] ^= rcon;
                rcon = gmul(rcon, 2);
            } else if nk > 6 && i % nk == 4 {
                for b in &mut temp {
                    *b = t.sbox[*b as usize];
                }
            }
            let prev = w[i - nk];
            w.push([
                prev[0] ^ temp[0],
                prev[1] ^ temp[1],
                prev[2] ^ temp[2],
                prev[3] ^ temp[3],
            ]);
        }
        let round_keys = w
            .chunks(4)
            .map(|c| {
                let mut k = [0u8; 16];
                for (j, word) in c.iter().enumerate() {
                    k[4 * j..4 * j + 4].copy_from_slice(word);
                }
                k
            })
            .collect();
        Some(Aes { round_keys })
    }

    /// Cifra un bloque (FIPS 197 §5.1, Cipher).
    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        let t = tables();
        let nr = self.round_keys.len() - 1;
        xor(block, &self.round_keys[0]);
        for (round, rk) in self.round_keys.iter().enumerate().skip(1) {
            for b in block.iter_mut() {
                *b = t.sbox[*b as usize];
            }
            shift_rows(block);
            if round != nr {
                mix_columns(block);
            }
            xor(block, rk);
        }
    }

    /// Descifra un bloque (FIPS 197 §5.3, InvCipher).
    pub fn decrypt_block(&self, block: &mut [u8; 16]) {
        let t = tables();
        let nr = self.round_keys.len() - 1;
        xor(block, &self.round_keys[nr]);
        for round in (0..nr).rev() {
            inv_shift_rows(block);
            for b in block.iter_mut() {
                *b = t.inv[*b as usize];
            }
            xor(block, &self.round_keys[round]);
            if round != 0 {
                inv_mix_columns(block);
            }
        }
    }
}

fn xor(a: &mut [u8; 16], b: &[u8; 16]) {
    for (x, y) in a.iter_mut().zip(b) {
        *x ^= y;
    }
}

/// El estado va por columnas: el byte de la fila `r` y la columna `c` es `s[4c + r]`.
fn shift_rows(s: &mut [u8; 16]) {
    let o = *s;
    for r in 1..4 {
        for c in 0..4 {
            s[4 * c + r] = o[4 * ((c + r) % 4) + r];
        }
    }
}

fn inv_shift_rows(s: &mut [u8; 16]) {
    let o = *s;
    for r in 1..4 {
        for c in 0..4 {
            s[4 * ((c + r) % 4) + r] = o[4 * c + r];
        }
    }
}

fn mix_columns(s: &mut [u8; 16]) {
    for c in 0..4 {
        let a = [s[4 * c], s[4 * c + 1], s[4 * c + 2], s[4 * c + 3]];
        s[4 * c] = gmul(a[0], 2) ^ gmul(a[1], 3) ^ a[2] ^ a[3];
        s[4 * c + 1] = a[0] ^ gmul(a[1], 2) ^ gmul(a[2], 3) ^ a[3];
        s[4 * c + 2] = a[0] ^ a[1] ^ gmul(a[2], 2) ^ gmul(a[3], 3);
        s[4 * c + 3] = gmul(a[0], 3) ^ a[1] ^ a[2] ^ gmul(a[3], 2);
    }
}

fn inv_mix_columns(s: &mut [u8; 16]) {
    for c in 0..4 {
        let a = [s[4 * c], s[4 * c + 1], s[4 * c + 2], s[4 * c + 3]];
        s[4 * c] = gmul(a[0], 14) ^ gmul(a[1], 11) ^ gmul(a[2], 13) ^ gmul(a[3], 9);
        s[4 * c + 1] = gmul(a[0], 9) ^ gmul(a[1], 14) ^ gmul(a[2], 11) ^ gmul(a[3], 13);
        s[4 * c + 2] = gmul(a[0], 13) ^ gmul(a[1], 9) ^ gmul(a[2], 14) ^ gmul(a[3], 11);
        s[4 * c + 3] = gmul(a[0], 11) ^ gmul(a[1], 13) ^ gmul(a[2], 9) ^ gmul(a[3], 14);
    }
}

fn block(b: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..b.len()].copy_from_slice(b);
    out
}

// --- CBC (SP 800-38A §6.2; Web Crypto §28.3, con relleno PKCS #7, RFC 2315 §10.3) ---

/// `None` si `iv` no tiene 16 bytes.
pub fn cbc_encrypt(aes: &Aes, iv: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    if iv.len() != 16 {
        return None;
    }
    let pad = 16 - data.len() % 16;
    let mut padded = data.to_vec();
    padded.extend(std::iter::repeat_n(pad as u8, pad));
    let mut prev = block(iv);
    let mut out = Vec::with_capacity(padded.len());
    for chunk in padded.chunks(16) {
        let mut b = block(chunk);
        xor(&mut b, &prev);
        aes.encrypt_block(&mut b);
        out.extend_from_slice(&b);
        prev = b;
    }
    Some(out)
}

/// `None` si `iv` no tiene 16 bytes, el texto no es un múltiplo de 16 o el relleno no es válido.
pub fn cbc_decrypt(aes: &Aes, iv: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    if iv.len() != 16 || data.is_empty() || !data.len().is_multiple_of(16) {
        return None;
    }
    let mut prev = block(iv);
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.chunks(16) {
        let c = block(chunk);
        let mut b = c;
        aes.decrypt_block(&mut b);
        xor(&mut b, &prev);
        out.extend_from_slice(&b);
        prev = c;
    }
    let pad = *out.last()? as usize;
    if pad == 0 || pad > 16 || out[out.len() - pad..].iter().any(|&b| b as usize != pad) {
        return None;
    }
    out.truncate(out.len() - pad);
    Some(out)
}

// --- CTR (SP 800-38A §6.5; Web Crypto §28.2) ---

/// Cifra o descifra con el bloque contador `counter`, cuyos `length` bits de la derecha son el
/// contador (y el resto, el nonce). `None` si `counter` no tiene 16 bytes, `length` no está en
/// 1..=128 o el contador daría la vuelta (se repetirían bloques de la secuencia).
pub fn ctr(aes: &Aes, counter: &[u8], length: u32, data: &[u8]) -> Option<Vec<u8>> {
    if counter.len() != 16 || length == 0 || length > 128 {
        return None;
    }
    let blocks = data.len().div_ceil(16) as u128;
    if length < 128 && blocks > (1u128 << length) {
        return None;
    }
    let start = u128::from_be_bytes(block(counter));
    let mask = if length == 128 {
        u128::MAX
    } else {
        (1u128 << length) - 1
    };
    let mut out = Vec::with_capacity(data.len());
    for (i, chunk) in data.chunks(16).enumerate() {
        let ctr = (start & !mask) | ((start & mask).wrapping_add(i as u128) & mask);
        let mut ks = ctr.to_be_bytes();
        aes.encrypt_block(&mut ks);
        out.extend(chunk.iter().zip(ks).map(|(a, b)| a ^ b));
    }
    Some(out)
}

// --- GCM (SP 800-38D; Web Crypto §28.4) ---

/// La multiplicación en GF(2¹²⁸) de GCM (SP 800-38D §6.3, Algoritmo 1).
fn gf_mul(x: u128, y: u128) -> u128 {
    const R: u128 = 0xE1 << 120;
    let mut z = 0u128;
    let mut v = y;
    for i in 0..128 {
        if (x >> (127 - i)) & 1 == 1 {
            z ^= v;
        }
        v = if v & 1 == 1 { (v >> 1) ^ R } else { v >> 1 };
    }
    z
}

/// GHASH (§6.4) de los bloques de `data` (rellenos con ceros hasta un múltiplo de 16).
fn ghash(h: u128, y: u128, data: &[u8]) -> u128 {
    let mut y = y;
    for chunk in data.chunks(16) {
        y = gf_mul(y ^ u128::from_be_bytes(block(chunk)), h);
    }
    y
}

/// GCTR (§6.5) con el bloque contador inicial `icb` (se incrementan los 32 bits de la derecha).
fn gctr(aes: &Aes, icb: u128, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut cb = icb;
    for chunk in data.chunks(16) {
        let mut ks = cb.to_be_bytes();
        aes.encrypt_block(&mut ks);
        out.extend(chunk.iter().zip(ks).map(|(a, b)| a ^ b));
        cb = (cb & !0xffff_ffffu128) | ((cb as u32).wrapping_add(1) as u128);
    }
    out
}

/// `H` y `J0` (§7.1, pasos 1 y 2).
fn gcm_setup(aes: &Aes, iv: &[u8]) -> (u128, u128) {
    let mut h = [0u8; 16];
    aes.encrypt_block(&mut h);
    let h = u128::from_be_bytes(h);
    let j0 = if iv.len() == 12 {
        let mut b = [0u8; 16];
        b[..12].copy_from_slice(iv);
        b[15] = 1;
        u128::from_be_bytes(b)
    } else {
        let y = ghash(h, 0, iv);
        gf_mul(y ^ ((iv.len() as u128) * 8), h)
    };
    (h, j0)
}

/// La etiqueta de autenticación (§7.1, pasos 5 y 6), de `tag_len` bytes.
fn gcm_tag(aes: &Aes, h: u128, j0: u128, aad: &[u8], c: &[u8], tag_len: usize) -> Vec<u8> {
    let mut s = ghash(h, 0, aad);
    s = ghash(h, s, c);
    s = gf_mul(
        s ^ ((((aad.len() as u128) * 8) << 64) | ((c.len() as u128) * 8)),
        h,
    );
    let t = gctr(aes, j0, &s.to_be_bytes());
    t[..tag_len].to_vec()
}

fn inc32(x: u128) -> u128 {
    (x & !0xffff_ffffu128) | ((x as u32).wrapping_add(1) as u128)
}

/// GCM-AE (§7.1): el texto cifrado seguido de la etiqueta de `tag_len` bytes.
pub fn gcm_encrypt(aes: &Aes, iv: &[u8], aad: &[u8], data: &[u8], tag_len: usize) -> Vec<u8> {
    let (h, j0) = gcm_setup(aes, iv);
    let mut c = gctr(aes, inc32(j0), data);
    let tag = gcm_tag(aes, h, j0, aad, &c, tag_len);
    c.extend_from_slice(&tag);
    c
}

/// GCM-AD (§7.2): `None` si el texto es más corto que la etiqueta o la etiqueta no coincide.
pub fn gcm_decrypt(
    aes: &Aes,
    iv: &[u8],
    aad: &[u8],
    data: &[u8],
    tag_len: usize,
) -> Option<Vec<u8>> {
    if data.len() < tag_len {
        return None;
    }
    let (c, tag) = data.split_at(data.len() - tag_len);
    let (h, j0) = gcm_setup(aes, iv);
    let expected = gcm_tag(aes, h, j0, aad, c, tag_len);
    if expected
        .iter()
        .zip(tag)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        != 0
    {
        return None;
    }
    Some(gctr(aes, inc32(j0), c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        let s: String = s.split_whitespace().collect();
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn sbox_known_values() {
        let t = tables();
        // FIPS 197 Figura 7: S(0x00) = 0x63, S(0x53) = 0xed.
        assert_eq!(t.sbox[0x00], 0x63);
        assert_eq!(t.sbox[0x53], 0xed);
        assert_eq!(t.inv[0x63], 0x00);
    }

    /// FIPS 197, apéndice C (C.1, C.2 y C.3): el mismo texto con claves de 128, 192 y 256 bits.
    #[test]
    fn fips_197_appendix_c() {
        let plain = unhex("00112233445566778899aabbccddeeff");
        let cases = [
            (
                "000102030405060708090a0b0c0d0e0f",
                "69c4e0d86a7b0430d8cdb78070b4c55a",
            ),
            (
                "000102030405060708090a0b0c0d0e0f1011121314151617",
                "dda97ca4864cdfe06eaf70a0ec0d7191",
            ),
            (
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
                "8ea2b7ca516745bfeafc49904b496089",
            ),
        ];
        for (key, cipher) in cases {
            let aes = Aes::new(&unhex(key)).unwrap();
            let mut b = block(&plain);
            aes.encrypt_block(&mut b);
            assert_eq!(hex(&b), cipher);
            aes.decrypt_block(&mut b);
            assert_eq!(b.to_vec(), plain);
        }
        assert!(Aes::new(&[0; 15]).is_none());
    }

    /// SP 800-38A F.2.1 (CBC-AES128.Encrypt), los dos primeros bloques; y el relleno PKCS #7.
    #[test]
    fn sp800_38a_cbc() {
        let aes = Aes::new(&unhex("2b7e151628aed2a6abf7158809cf4f3c")).unwrap();
        let iv = unhex("000102030405060708090a0b0c0d0e0f");
        let plain = unhex("6bc1bee22e409f96e93d7e117393172a ae2d8a571e03ac9c9eb76fac45af8e51");
        let out = cbc_encrypt(&aes, &iv, &plain).unwrap();
        assert_eq!(
            hex(&out[..32]),
            "7649abac8119b246cee98e9b12e9197d5086cb9b507219ee95db113a917678b2"
        );
        // El bloque de relleno (16 bytes de 0x10) va al final.
        assert_eq!(out.len(), 48);
        assert_eq!(cbc_decrypt(&aes, &iv, &out).unwrap(), plain);
        assert!(cbc_decrypt(&aes, &iv, &out[..31]).is_none());
        assert!(cbc_encrypt(&aes, &iv[..8], &plain).is_none());
    }

    /// SP 800-38A F.5.1 (CTR-AES128.Encrypt), los dos primeros bloques.
    #[test]
    fn sp800_38a_ctr() {
        let aes = Aes::new(&unhex("2b7e151628aed2a6abf7158809cf4f3c")).unwrap();
        let counter = unhex("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff");
        let plain = unhex("6bc1bee22e409f96e93d7e117393172a ae2d8a571e03ac9c9eb76fac45af8e51");
        let out = ctr(&aes, &counter, 128, &plain).unwrap();
        assert_eq!(
            hex(&out),
            "874d6191b620e3261bef6864990db6ce9806f66b7970fdff8617187bb9fffdff"
        );
        assert_eq!(ctr(&aes, &counter, 128, &out).unwrap(), plain);
        // Un contador de 1 bit no alcanza para tres bloques.
        assert!(ctr(&aes, &counter, 1, &[0; 33]).is_none());
    }

    /// Los casos de prueba de GCM de McGrew y Viega ("The Galois/Counter Mode of Operation",
    /// los que usa NIST): 2, 3, 4 (con AAD) y 6 (IV de 60 bytes).
    #[test]
    fn gcm_test_cases() {
        let k = unhex("feffe9928665731c6d6a8f9467308308");
        let iv = unhex("cafebabefacedbaddecaf888");
        let p = unhex(
            "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a72\
             1c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b39",
        );
        let aad = unhex("feedfacedeadbeeffeedfacedeadbeefabaddad2");
        // Caso 2: clave cero, un bloque cero.
        let zero = Aes::new(&[0; 16]).unwrap();
        assert_eq!(
            hex(&gcm_encrypt(&zero, &[0; 12], &[], &[0; 16], 16)),
            "0388dace60b6a392f328c2b971b2fe78ab6e47d42cec13bdf53a67b21257bddf"
        );
        let aes = Aes::new(&k).unwrap();
        // Caso 4: con AAD y un texto de 60 bytes.
        let out = gcm_encrypt(&aes, &iv, &aad, &p, 16);
        assert_eq!(
            hex(&out),
            "42831ec2217774244b7221b784d0d49ce3aa212f2c02a4e035c17e2329aca12e\
             21d514b25466931c7d8f6a5aac84aa051ba30b396a0aac973d58e0915bc94fbc\
             3221a5db94fae95ae7121a47"
        );
        assert_eq!(gcm_decrypt(&aes, &iv, &aad, &out, 16).unwrap(), p);
        let mut bad = out.clone();
        bad[0] ^= 1;
        assert!(gcm_decrypt(&aes, &iv, &aad, &bad, 16).is_none());
        // Caso 6: IV de 60 bytes (J0 por GHASH).
        let iv6 = unhex(
            "9313225df88406e555909c5aff5269aa6a7a9538534f7da1e4c303d2a318a728\
             c3c0c95156809539fcf0e2429a6b525416aedbf5a0de6a57a637b39b",
        );
        assert_eq!(
            hex(&gcm_encrypt(&aes, &iv6, &aad, &p, 16)[60..]),
            "619cc5aefffe0bfa462af43c1699d050"
        );
    }
}
