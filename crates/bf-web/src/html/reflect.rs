//! Reflexión de atributos de contenido en atributos IDL (HTML §2.6.1, "Reflecting content
//! attributes in IDL attributes") y los microsintaxis que usa (§2.3.4: enteros, enteros no
//! negativos y números de punto flotante).
//!
//! Todo es puro: el getter recibe el valor actual del atributo de contenido (`None` si no
//! está) y el setter devuelve qué hacer con él ([`SetAction`]). La conversión WebIDL del
//! valor que llega desde JavaScript la hace quien llama.

/// Cómo se refleja un atributo de contenido (las clases de §2.6.1).
#[derive(Debug, Clone, Copy)]
pub enum Reflect {
    /// `DOMString`: el valor, o el string vacío.
    Str,
    /// `USVString` que refleja una URL: el valor parseado contra la base del documento.
    Url,
    /// `boolean`: presente o ausente.
    Bool,
    /// `long`, con su valor por defecto.
    Long(i32),
    /// `long` limitado a no negativos: el setter lanza IndexSizeError con negativos.
    LimitedLong(i32),
    /// `unsigned long`.
    ULong(u32),
    /// `unsigned long` limitado a positivos (`fallback`: el 0 del setter usa el default en
    /// vez de lanzar).
    LimitedULong { default: u32, fallback: bool },
    /// `unsigned long` acotado a `[min, max]`.
    ClampedULong { default: u32, min: u32, max: u32 },
    /// `double`.
    Double(f64),
    /// `double` limitado a positivos: el setter ignora los valores no positivos.
    LimitedDouble(f64),
    /// Un atributo enumerado limitado a los valores conocidos.
    Enum(&'static EnumSpec),
}

/// Un atributo enumerado (§2.3.3) reflejado "limited to only known values".
#[derive(Debug)]
pub struct EnumSpec {
    /// Las palabras clave en su forma canónica (en minúsculas).
    pub keywords: &'static [&'static str],
    /// Valores no canónicos que corresponden a una palabra clave (p. ej. `""` →
    /// `anonymous` en CORS settings).
    pub aliases: &'static [(&'static str, &'static str)],
    /// *Missing value default*: `None` = el string vacío.
    pub missing: Option<&'static str>,
    /// *Invalid value default*: `None` = el string vacío.
    pub invalid: Option<&'static str>,
    /// *Empty value default*, si el estado del valor vacío difiere del inválido.
    pub empty: Option<&'static str>,
    /// IDL `DOMString?`: sin atributo (y sin *missing value default*) el getter da `null`.
    pub nullable: bool,
}

/// Lo que devuelve un getter.
#[derive(Debug, Clone, PartialEq)]
pub enum Got {
    Str(String),
    Null,
    Bool(bool),
    Num(f64),
}

/// Lo que el setter hace con el atributo de contenido.
#[derive(Debug, Clone, PartialEq)]
pub enum SetAction {
    Set(String),
    Remove,
    Nothing,
    /// `IndexSizeError` DOMException.
    IndexSizeError,
}

/// El valor que llega al setter, ya convertido por WebIDL según el tipo IDL.
#[derive(Debug, Clone)]
pub enum Input {
    Str(String),
    Null,
    Bool(bool),
    Num(f64),
}

const MAX_LONG: i64 = 2147483647;

fn is_ascii_ws(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' ')
}

/// Las reglas para parsear enteros (§2.3.4.1).
pub fn parse_integer(input: &str) -> Option<i64> {
    let s = input.trim_start_matches(is_ascii_ws);
    let (sign, rest) = match s.as_bytes().first()? {
        b'-' => (-1i64, &s[1..]),
        b'+' => (1, &s[1..]),
        _ => (1, s),
    };
    let digits: &str = &rest[..rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len())];
    if digits.is_empty() {
        return None;
    }
    // Más dígitos de los que caben en un i64 quedan fuera de cualquier rango que se usa.
    let mut v: i64 = 0;
    for b in digits.bytes() {
        v = v.saturating_mul(10).saturating_add(i64::from(b - b'0'));
    }
    Some(sign * v)
}

/// Las reglas para parsear enteros no negativos (§2.3.4.2).
pub fn parse_non_negative_integer(input: &str) -> Option<i64> {
    parse_integer(input).filter(|&v| v >= 0)
}

/// Las reglas para parsear números de punto flotante (§2.3.4.3). El valor exacto se
/// redondea al double más cercano (el `-0` queda en `0`); fuera de rango es un error.
pub fn parse_float(input: &str) -> Option<f64> {
    let s = input.trim_start_matches(is_ascii_ws);
    let b = s.as_bytes();
    let mut i = 0;
    let mut text = String::new();
    match b.first()? {
        b'-' => {
            text.push('-');
            i += 1;
        }
        b'+' => i += 1,
        _ => {}
    }
    if i >= b.len() {
        return None;
    }
    // Paso 6: ".5" empieza directamente por la fracción.
    let mut int_digits = String::new();
    if b[i] == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit) {
        int_digits.push('0');
    } else {
        if !b[i].is_ascii_digit() {
            return None;
        }
        while i < b.len() && b[i].is_ascii_digit() {
            int_digits.push(b[i] as char);
            i += 1;
        }
    }
    text.push_str(&int_digits);
    // Fracción (paso 9): después del punto, dígitos o directamente el exponente (`1.e2`);
    // cualquier otra cosa termina el número.
    if i < b.len() && b[i] == b'.' {
        match b.get(i + 1) {
            Some(c) if c.is_ascii_digit() => {
                text.push('.');
                i += 1;
                while i < b.len() && b[i].is_ascii_digit() {
                    text.push(b[i] as char);
                    i += 1;
                }
            }
            Some(b'e' | b'E') => i += 1,
            _ => {}
        }
    }
    // Exponente: solo si después de `e` (y el signo) hay dígitos.
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        let mut exp = String::from("e");
        if j < b.len() && (b[j] == b'-' || b[j] == b'+') {
            exp.push(b[j] as char);
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            while j < b.len() && b[j].is_ascii_digit() {
                exp.push(b[j] as char);
                j += 1;
            }
            text.push_str(&exp);
        }
    }
    let v: f64 = text.parse().ok()?;
    if !v.is_finite() {
        return None;
    }
    Some(if v == 0.0 { 0.0 } else { v })
}

/// "The best representation of the number n as a floating-point number" (§2.3.4.3):
/// Number::toString.
pub fn float_to_string(n: f64) -> String {
    bf_js::abstract_ops::number::number_to_std_string(n)
}

/// El getter de §2.6.1. `base` resuelve las URL (devuelve `None` si no parsea).
pub fn get(kind: Reflect, value: Option<&str>, base: &dyn Fn(&str) -> Option<String>) -> Got {
    match kind {
        Reflect::Str => Got::Str(value.unwrap_or("").to_owned()),
        Reflect::Url => match value {
            None => Got::Str(String::new()),
            Some(v) => Got::Str(base(v).unwrap_or_else(|| v.to_owned())),
        },
        Reflect::Bool => Got::Bool(value.is_some()),
        Reflect::Long(d) => {
            let v = value
                .and_then(parse_integer)
                .filter(|v| (-MAX_LONG - 1..=MAX_LONG).contains(v));
            Got::Num(v.map_or(f64::from(d), |v| v as f64))
        }
        Reflect::LimitedLong(d) => {
            let v = value
                .and_then(parse_non_negative_integer)
                .filter(|v| *v <= MAX_LONG);
            Got::Num(v.map_or(f64::from(d), |v| v as f64))
        }
        Reflect::ULong(d) => {
            let v = value
                .and_then(parse_non_negative_integer)
                .filter(|v| *v <= MAX_LONG);
            Got::Num(v.map_or(f64::from(d), |v| v as f64))
        }
        Reflect::LimitedULong { default, .. } => {
            let v = value
                .and_then(parse_non_negative_integer)
                .filter(|v| (1..=MAX_LONG).contains(v));
            Got::Num(v.map_or(f64::from(default), |v| v as f64))
        }
        Reflect::ClampedULong { default, min, max } => {
            let v = match value.and_then(parse_non_negative_integer) {
                None => i64::from(default),
                Some(v) => v.clamp(i64::from(min), i64::from(max)),
            };
            Got::Num(v as f64)
        }
        Reflect::Double(d) => Got::Num(value.and_then(parse_float).unwrap_or(d)),
        Reflect::LimitedDouble(d) => Got::Num(
            value
                .and_then(parse_float)
                .filter(|v| *v > 0.0)
                .unwrap_or(d),
        ),
        Reflect::Enum(spec) => get_enum(spec, value),
    }
}

/// El estado de un atributo enumerado (§2.3.3) como su palabra clave canónica.
fn get_enum(spec: &EnumSpec, value: Option<&str>) -> Got {
    let missing = || match spec.missing {
        Some(k) => Got::Str(k.to_owned()),
        None if spec.nullable => Got::Null,
        None => Got::Str(String::new()),
    };
    let Some(v) = value else {
        return missing();
    };
    if v.is_empty()
        && let Some(e) = spec.empty
    {
        return Got::Str(e.to_owned());
    }
    if let Some(k) = spec.keywords.iter().find(|k| k.eq_ignore_ascii_case(v)) {
        return Got::Str((*k).to_owned());
    }
    if let Some((_, k)) = spec.aliases.iter().find(|(a, _)| a.eq_ignore_ascii_case(v)) {
        return Got::Str((*k).to_owned());
    }
    match spec.invalid {
        Some(k) => Got::Str(k.to_owned()),
        // Sin *invalid value default*, el estado es el del valor ausente.
        None if spec.missing.is_some() => missing(),
        None if spec.nullable => Got::Null,
        None => Got::Str(String::new()),
    }
}

/// El setter de §2.6.1.
pub fn set(kind: Reflect, input: Input) -> SetAction {
    let num = |i: &Input| match i {
        Input::Num(n) => *n,
        _ => 0.0,
    };
    match kind {
        Reflect::Str | Reflect::Url | Reflect::Enum(_) => match input {
            Input::Str(s) => SetAction::Set(s),
            Input::Null => SetAction::Remove,
            Input::Bool(b) => SetAction::Set(b.to_string()),
            Input::Num(n) => SetAction::Set(float_to_string(n)),
        },
        Reflect::Bool => match input {
            Input::Bool(true) => SetAction::Set(String::new()),
            _ => SetAction::Remove,
        },
        Reflect::Long(_) => SetAction::Set((num(&input) as i64).to_string()),
        Reflect::LimitedLong(_) => {
            let n = num(&input) as i64;
            if n < 0 {
                SetAction::IndexSizeError
            } else {
                SetAction::Set(n.to_string())
            }
        }
        Reflect::ULong(d) | Reflect::ClampedULong { default: d, .. } => {
            let n = num(&input) as i64;
            let n = if n <= MAX_LONG { n } else { i64::from(d) };
            SetAction::Set(n.to_string())
        }
        Reflect::LimitedULong { default, fallback } => {
            let n = num(&input) as i64;
            if n == 0 {
                if fallback {
                    SetAction::Set(default.to_string())
                } else {
                    SetAction::IndexSizeError
                }
            } else if n <= MAX_LONG {
                SetAction::Set(n.to_string())
            } else {
                SetAction::Set(default.to_string())
            }
        }
        Reflect::Double(_) => SetAction::Set(float_to_string(num(&input))),
        Reflect::LimitedDouble(_) => {
            let n = num(&input);
            if n > 0.0 {
                SetAction::Set(float_to_string(n))
            } else {
                SetAction::Nothing
            }
        }
    }
}
