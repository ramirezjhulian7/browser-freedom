//! El tipo Number (§6.1.6.1): Number::toString y los operadores con todos sus casos de NaN,
//! ±0 e ±Infinity.

use crate::string::JsString;

/// Number::toString(x, 10) (§6.1.6.1.20): los dígitos más cortos que vuelven a `x` (los da
/// el formateo de `f64` de Rust) con las reglas de exponente del spec.
pub fn number_to_string(x: f64) -> JsString {
    JsString::from(number_to_std_string(x).as_str())
}

pub fn number_to_std_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".to_owned();
    }
    if x == 0.0 {
        return "0".to_owned();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    if x < 0.0 {
        return format!("-{}", number_to_std_string(-x));
    }
    // `{:e}`: "d.ddddde±x" con los dígitos más cortos.
    let e = format!("{x:e}");
    let (mantissa, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let digits: String = mantissa.chars().filter(|c| c.is_ascii_digit()).collect();
    let exp: i32 = exp.parse().unwrap_or(0);
    let k = digits.len() as i32;
    let n = exp + 1;
    if k <= n && n <= 21 {
        let mut s = digits;
        for _ in 0..(n - k) {
            s.push('0');
        }
        s
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{}", "0".repeat((-n) as usize), digits)
    } else {
        let sign = if n > 0 { '+' } else { '-' };
        let e = (n - 1).abs();
        if k == 1 {
            format!("{digits}e{sign}{e}")
        } else {
            format!("{}.{}e{sign}{e}", &digits[..1], &digits[1..])
        }
    }
}

/// El siguiente Number representable hacia arriba (para el algoritmo de radix).
fn next_up(x: f64) -> f64 {
    if x.is_nan() || x == f64::INFINITY {
        return x;
    }
    if x == 0.0 {
        return f64::from_bits(1);
    }
    let b = x.to_bits();
    if x > 0.0 {
        f64::from_bits(b + 1)
    } else {
        f64::from_bits(b - 1)
    }
}

/// Number::toString(x, radix) para radix ≠ 10: dígitos suficientes para distinguir el
/// valor (el spec deja la aproximación a la implementación; este es el algoritmo de V8).
pub fn number_to_string_radix(value: f64, radix: u32) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value == 0.0 {
        return "0".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    let chars: Vec<char> = "0123456789abcdefghijklmnopqrstuvwxyz".chars().collect();
    let negative = value < 0.0;
    let value = value.abs();
    let mut integer = value.floor();
    let mut fraction = value - integer;
    let mut delta = 0.5 * (next_up(value) - value);
    delta = delta.max(next_up(0.0));
    let mut frac_digits: Vec<char> = Vec::new();
    let r = f64::from(radix);
    if fraction >= delta {
        loop {
            fraction *= r;
            delta *= r;
            let digit = fraction as u32;
            frac_digits.push(chars[digit as usize]);
            fraction -= f64::from(digit);
            if (fraction > 0.5 || (fraction == 0.5 && digit & 1 == 1)) && fraction + delta > 1.0 {
                // Acarreo hacia atrás.
                loop {
                    match frac_digits.pop() {
                        None => {
                            integer += 1.0;
                            break;
                        }
                        Some(c) => {
                            let d = c.to_digit(36).unwrap_or(0);
                            if d + 1 < radix {
                                frac_digits.push(chars[(d + 1) as usize]);
                                break;
                            }
                        }
                    }
                }
                break;
            }
            if fraction < delta {
                break;
            }
        }
    }
    // La parte entera; los dígitos no representables son ceros.
    let mut int_digits: Vec<char> = Vec::new();
    let two53 = 9007199254740992.0;
    while integer / r >= two53 {
        integer /= r;
        int_digits.push('0');
    }
    loop {
        let rem = integer % r;
        int_digits.push(chars[rem as usize]);
        integer = (integer - rem) / r;
        if integer <= 0.0 {
            break;
        }
    }
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    out.extend(int_digits.iter().rev());
    if !frac_digits.is_empty() {
        out.push('.');
        out.extend(frac_digits.iter());
    }
    out
}

/// Number::exponentiate (§6.1.6.1.3).
pub fn exponentiate(base: f64, exponent: f64) -> f64 {
    if exponent.is_nan() {
        return f64::NAN;
    }
    if exponent == 0.0 {
        return 1.0;
    }
    if base.is_nan() {
        return f64::NAN;
    }
    if (base == 1.0 || base == -1.0) && exponent.is_infinite() {
        return f64::NAN;
    }
    base.powf(exponent)
}

/// Number::remainder (§6.1.6.1.6): el resto con el signo del dividendo (`fmod`).
pub fn remainder(n: f64, d: f64) -> f64 {
    if n.is_nan() || d.is_nan() || n.is_infinite() || d == 0.0 {
        return f64::NAN;
    }
    if d.is_infinite() || n == 0.0 {
        return n;
    }
    let r = n % d;
    if r == 0.0 && n < 0.0 { -0.0 } else { r }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_string() {
        let cases = [
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (123.456, "123.456"),
            (1e21, "1e+21"),
            (1e20, "100000000000000000000"),
            (1e-7, "1e-7"),
            (0.000001, "0.000001"),
            (1.5e-7, "1.5e-7"),
            (-1.5, "-1.5"),
            (0.1 + 0.2, "0.30000000000000004"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
            (123e25, "1.23e+27"),
        ];
        for (x, s) in cases {
            assert_eq!(number_to_std_string(x), s, "{x}");
        }
    }

    #[test]
    fn radix() {
        assert_eq!(number_to_string_radix(255.0, 16), "ff");
        assert_eq!(number_to_string_radix(-255.0, 2), "-11111111");
        assert_eq!(number_to_string_radix(0.5, 2), "0.1");
    }
}
