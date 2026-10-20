//! NumericLiteral (ECMA-262 §12.9.3) y su valor matemático redondeado a Number (§12.9.3.1,
//! "RoundMVResult"): decimales (con `f64::from_str`, que redondea correctamente), enteros en
//! base 2, 8 y 16 de cualquier longitud (redondeo propio al par más cercano), octales legados
//! y BigInt.

use super::Tok;

type NResult = Result<Tok, (usize, &'static str)>;

/// Dígitos de `radix` con separadores `_` (§12.9.3, NumericLiteralSeparator: solo entre dos
/// dígitos). Devuelve los dígitos sin separadores.
fn digits(
    chars: &[char],
    pos: &mut usize,
    radix: u32,
    separators: bool,
) -> Result<String, (usize, &'static str)> {
    let mut out = String::new();
    let mut prev_sep = false;
    while let Some(&c) = chars.get(*pos) {
        if c == '_' && separators {
            if out.is_empty() || prev_sep {
                return Err((*pos, "Numeric separators are not allowed here"));
            }
            prev_sep = true;
            *pos += 1;
            continue;
        }
        if c == '_' {
            return Err((*pos, "Numeric separators are not allowed here"));
        }
        if !c.is_digit(radix) {
            break;
        }
        prev_sep = false;
        out.push(c);
        *pos += 1;
    }
    if prev_sep {
        return Err((
            *pos - 1,
            "Numeric separators are not allowed at the end of numeric literals",
        ));
    }
    Ok(out)
}

/// Valor redondeado al Number más cercano (empates al par) de un entero en base 2^`bits`.
pub fn radix_to_f64(digits: &str, radix: u32) -> f64 {
    let bits_per = match radix {
        2 => 1,
        8 => 3,
        16 => 4,
        _ => unreachable!("base potencia de 2"),
    };
    // Bits del entero, del más significativo al menos.
    let mut bits: Vec<bool> = Vec::with_capacity(digits.len() * bits_per);
    for c in digits.chars() {
        let d = c.to_digit(radix).unwrap_or(0);
        for i in (0..bits_per).rev() {
            bits.push(d >> i & 1 == 1);
        }
    }
    let first = bits.iter().position(|&b| b);
    let Some(first) = first else {
        return 0.0;
    };
    let bits = &bits[first..];
    if bits.len() <= 64 {
        let mut v: u64 = 0;
        for &b in bits {
            v = v << 1 | u64::from(b);
        }
        // `as` redondea al más cercano con empates al par.
        return v as f64;
    }
    // 53 bits de mantisa, el de redondeo y el resto ("sticky").
    let mut mantissa: u64 = 0;
    for &b in &bits[..53] {
        mantissa = mantissa << 1 | u64::from(b);
    }
    let round = bits[53];
    let sticky = bits[54..].iter().any(|&b| b);
    let mut exp = (bits.len() - 53) as i32;
    if round && (sticky || mantissa & 1 == 1) {
        mantissa += 1;
        if mantissa == 1 << 53 {
            mantissa >>= 1;
            exp += 1;
        }
    }
    if exp > 1023 - 52 {
        return f64::INFINITY;
    }
    mantissa as f64 * 2f64.powi(exp)
}

/// Escanea un NumericLiteral desde `pos`.
pub fn scan(chars: &[char], pos: &mut usize) -> NResult {
    let start = *pos;
    let c = chars[*pos];
    let next = chars.get(*pos + 1).copied();
    if c == '0' {
        let radix = match next {
            Some('x' | 'X') => Some(16),
            Some('o' | 'O') => Some(8),
            Some('b' | 'B') => Some(2),
            _ => None,
        };
        if let Some(radix) = radix {
            *pos += 2;
            let d = digits(chars, pos, radix, true)?;
            if d.is_empty() {
                return Err((start, "Invalid or unexpected token"));
            }
            if chars.get(*pos) == Some(&'n') {
                *pos += 1;
                let raw: String = chars[start..*pos - 1]
                    .iter()
                    .filter(|&&c| c != '_')
                    .collect();
                return Ok(Tok::BigInt(raw));
            }
            return Ok(Tok::Number {
                value: radix_to_f64(&d, radix),
                legacy_octal: false,
            });
        }
        if next.is_some_and(|d| d.is_ascii_digit()) {
            // LegacyOctalIntegerLiteral o NonOctalDecimalIntegerLiteral (B.1.1 / §12.9.3):
            // sin separadores ni BigInt.
            *pos += 1;
            let mut all = String::new();
            while let Some(&d) = chars.get(*pos) {
                if d == '_' {
                    return Err((*pos, "Numeric separators are not allowed here"));
                }
                if !d.is_ascii_digit() {
                    break;
                }
                all.push(d);
                *pos += 1;
            }
            if chars.get(*pos) == Some(&'n') {
                return Err((start, "Invalid or unexpected token"));
            }
            if all.chars().all(|d| d < '8') {
                let mut v = 0f64;
                for d in all.chars() {
                    v = v * 8.0 + f64::from(d.to_digit(8).unwrap_or(0));
                }
                return Ok(Tok::Number {
                    value: v,
                    legacy_octal: true,
                });
            }
            // NonOctalDecimalIntegerLiteral: puede seguir con fracción y exponente.
            let mut text = String::from("0");
            text.push_str(&all);
            decimal_tail(chars, pos, &mut text, false)?;
            return Ok(Tok::Number {
                value: text.parse().unwrap_or(f64::NAN),
                legacy_octal: true,
            });
        }
    }
    // `0_1`: un cero inicial no admite separadores (§12.9.3, DecimalIntegerLiteral).
    if c == '0' && next == Some('_') {
        return Err((
            start + 1,
            "Numeric separator can not be used after leading 0",
        ));
    }
    let mut text = String::new();
    let mut integer = true;
    if c != '.' {
        text.push_str(&digits(chars, pos, 10, true)?);
    }
    // BigInt: solo enteros decimales sin ceros a la izquierda.
    if chars.get(*pos) == Some(&'n') {
        *pos += 1;
        let raw: String = chars[start..*pos - 1]
            .iter()
            .filter(|&&c| c != '_')
            .collect();
        return Ok(Tok::BigInt(raw));
    }
    if chars.get(*pos) == Some(&'.') || matches!(chars.get(*pos), Some('e' | 'E')) {
        integer = false;
    }
    decimal_tail(chars, pos, &mut text, true)?;
    if !integer && chars.get(*pos) == Some(&'n') {
        return Err((start, "Invalid or unexpected token"));
    }
    if text.starts_with('.') {
        text.insert(0, '0');
    }
    Ok(Tok::Number {
        value: text.parse().unwrap_or(f64::NAN),
        legacy_octal: false,
    })
}

/// La fracción y el exponente de un decimal.
fn decimal_tail(
    chars: &[char],
    pos: &mut usize,
    text: &mut String,
    separators: bool,
) -> Result<(), (usize, &'static str)> {
    if chars.get(*pos) == Some(&'.') {
        *pos += 1;
        if chars.get(*pos) == Some(&'_') {
            return Err((*pos, "Numeric separators are not allowed here"));
        }
        text.push('.');
        text.push_str(&digits(chars, pos, 10, separators)?);
    }
    if matches!(chars.get(*pos), Some('e' | 'E')) {
        let at = *pos;
        *pos += 1;
        text.push('e');
        if let Some(&s) = chars.get(*pos)
            && (s == '+' || s == '-')
        {
            text.push(s);
            *pos += 1;
        }
        if chars.get(*pos) == Some(&'_') {
            return Err((*pos, "Numeric separators are not allowed here"));
        }
        let exp = digits(chars, pos, 10, separators)?;
        if exp.is_empty() {
            return Err((at, "Invalid or unexpected token"));
        }
        text.push_str(&exp);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(src: &str) -> f64 {
        let chars: Vec<char> = src.chars().collect();
        let mut pos = 0;
        match scan(&chars, &mut pos).unwrap() {
            Tok::Number { value, .. } => value,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn values() {
        assert_eq!(value("1_000"), 1000.0);
        assert_eq!(value("0x1F"), 31.0);
        assert_eq!(value("0b101"), 5.0);
        assert_eq!(value("0o17"), 15.0);
        assert_eq!(value("017"), 15.0);
        assert_eq!(value("019"), 19.0);
        assert_eq!(value("019.5"), 19.5);
        assert_eq!(value(".5e1"), 5.0);
        assert_eq!(value("1e-7"), 1e-7);
        // Redondeo exacto de un hexadecimal de más de 64 bits (empate al par).
        assert_eq!(value("0x20000000000001"), 9007199254740992.0);
        assert_eq!(value("0x20000000000003"), 9007199254740996.0);
        assert_eq!(value("0x1fffffffffffff800"), 36893488147419103232.0);
        assert_eq!(value("0x1fffffffffffff801"), 36893488147419103232.0);
        assert_eq!(value("0x1fffffffffffff7ff"), 36893488147419099136.0);
    }

    #[test]
    fn separator_errors() {
        for bad in ["1__0", "1_", "0_1", "1._5", "1e_5", "0x_1", "08_1"] {
            let chars: Vec<char> = bad.chars().collect();
            let mut pos = 0;
            assert!(scan(&chars, &mut pos).is_err(), "{bad}");
        }
    }
}
