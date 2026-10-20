//! El tipo `<an-plus-b>` (CSS Syntax Level 3 §6.2).

use super::parser::ComponentValue;
use super::tokenizer::{Numeric, TokenKind};

/// Parsea `values` como `<an-plus-b>` y devuelve `(A, B)`. Whitespace permitido entre tokens,
/// salvo entre un `+` y el ident que empieza con "n" que lo sigue.
pub fn parse_anb(values: &[ComponentValue]) -> Option<(i32, i32)> {
    // Tokens no-whitespace, recordando si cada uno vino precedido de whitespace.
    let mut tokens: Vec<(&TokenKind, bool)> = Vec::new();
    let mut saw_ws = false;
    for value in values {
        match value.token() {
            Some(TokenKind::Whitespace) => saw_ws = true,
            Some(kind) => {
                tokens.push((kind, saw_ws));
                saw_ws = false;
            }
            None => return None,
        }
    }
    let (first, rest) = tokens.split_first()?;
    match first.0 {
        TokenKind::Ident(v) if v.eq_ignore_ascii_case("odd") && rest.is_empty() => Some((2, 1)),
        TokenKind::Ident(v) if v.eq_ignore_ascii_case("even") && rest.is_empty() => Some((2, 0)),
        TokenKind::Number(n) if rest.is_empty() => Some((0, n.as_integer()?)),
        TokenKind::Dimension(n, unit) => n_form(n.as_integer()?, unit, rest),
        TokenKind::Delim('+') => {
            let ((TokenKind::Ident(ident), false), rest) = rest.split_first()? else {
                return None;
            };
            // '+'? solo precede a las formas que empiezan con "n".
            if ident.starts_with('-') {
                return None;
            }
            n_form(1, ident, rest)
        }
        TokenKind::Ident(ident) => {
            let lower = ident.to_ascii_lowercase();
            match lower.strip_prefix('-') {
                Some(after) => n_form(-1, after, rest),
                None => n_form(1, &lower, rest),
            }
        }
        _ => None,
    }
}

/// Las formas con `n`: `a` es el coeficiente; `ident` empieza en la "n" (insensible a
/// mayúsculas) y `rest` son los tokens siguientes.
fn n_form(a: i32, ident: &str, rest: &[(&TokenKind, bool)]) -> Option<(i32, i32)> {
    let lower = ident.to_ascii_lowercase();
    let after_n = lower.strip_prefix('n')?;
    match after_n {
        // <n-dimension> | n | -n, opcionalmente seguidos de B.
        "" => match rest {
            [] => Some((a, 0)),
            [(TokenKind::Number(b), _)] if b.sign.is_some() => Some((a, b.as_integer()?)),
            [
                (TokenKind::Delim(sign @ ('+' | '-')), _),
                (TokenKind::Number(b), _),
            ] if is_signless_integer(b) => {
                let b = b.as_integer()?;
                Some((a, if *sign == '-' { b.checked_neg()? } else { b }))
            }
            _ => None,
        },
        // <ndash-dimension> | n- | -n- seguidos de un entero sin signo.
        "-" => match rest {
            [(TokenKind::Number(b), _)] if is_signless_integer(b) => {
                Some((a, b.as_integer()?.checked_neg()?))
            }
            _ => None,
        },
        // <ndashdigit-*>: n-<dígitos>.
        _ => {
            let digits = after_n.strip_prefix('-')?;
            if !rest.is_empty() || digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit())
            {
                return None;
            }
            let b: i64 = digits.parse().unwrap_or(i64::MAX);
            Some((a, (-b).max(i64::from(i32::MIN)) as i32))
        }
    }
}

fn is_signless_integer(n: &Numeric) -> bool {
    n.integer && n.sign.is_none()
}
