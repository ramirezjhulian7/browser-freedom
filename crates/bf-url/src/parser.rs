//! Basic URL parser (URL Standard §4.4): la máquina de estados completa, con `state override`
//! para los setters.

use crate::host::{Host, parse_host};
use crate::percent::{self, EncodeSet};
use crate::{Path, Url, default_port, is_special_scheme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    SchemeStart,
    Scheme,
    NoScheme,
    SpecialRelativeOrAuthority,
    PathOrAuthority,
    Relative,
    RelativeSlash,
    SpecialAuthoritySlashes,
    SpecialAuthorityIgnoreSlashes,
    Authority,
    Host,
    Hostname,
    Port,
    File,
    FileSlash,
    FileHost,
    PathStart,
    Path,
    OpaquePath,
    Query,
    Fragment,
}

/// Codificación del query con el encoding del documento: los bytes de un code point, o
/// `None` si no es representable (se emite `%26%23N%3B`).
pub type QueryEncoder<'a> = &'a dyn Fn(char) -> Option<Vec<u8>>;

fn is_windows_drive_letter(s: &[char]) -> bool {
    s.len() == 2 && s[0].is_ascii_alphabetic() && matches!(s[1], ':' | '|')
}

fn is_normalized_windows_drive_letter(s: &str) -> bool {
    let c: Vec<char> = s.chars().collect();
    is_windows_drive_letter(&c) && c[1] == ':'
}

/// "Starts with a Windows drive letter" (§4.2).
fn starts_with_windows_drive_letter(s: &[char]) -> bool {
    s.len() >= 2
        && is_windows_drive_letter(&s[..2])
        && (s.len() == 2 || matches!(s[2], '/' | '\\' | '?' | '#'))
}

fn is_single_dot(s: &str) -> bool {
    s == "." || s.eq_ignore_ascii_case("%2e")
}

fn is_double_dot(s: &str) -> bool {
    matches!(
        s.to_ascii_lowercase().as_str(),
        ".." | ".%2e" | "%2e." | "%2e%2e"
    )
}

impl Url {
    pub(crate) fn empty() -> Url {
        Url {
            scheme: String::new(),
            username: String::new(),
            password: String::new(),
            host: None,
            port: None,
            path: Path::List(Vec::new()),
            query: None,
            fragment: None,
        }
    }

    fn special(&self) -> bool {
        is_special_scheme(&self.scheme)
    }

    pub(crate) fn includes_credentials(&self) -> bool {
        !self.username.is_empty() || !self.password.is_empty()
    }

    /// Shorten a URL's path (§4.1).
    fn shorten_path(&mut self) {
        let file = self.scheme == "file";
        if let Path::List(segments) = &mut self.path {
            if file && segments.len() == 1 && is_normalized_windows_drive_letter(&segments[0]) {
                return;
            }
            segments.pop();
        }
    }

    fn push_segment(&mut self, s: String) {
        if let Path::List(segments) = &mut self.path {
            segments.push(s);
        }
    }

    fn path_is_empty(&self) -> bool {
        matches!(&self.path, Path::List(s) if s.is_empty())
    }
}

/// Basic URL parser. Con `url` y `state_override` modifica `url` (setters); devuelve `Err`
/// en caso de falla.
pub(crate) fn basic_parse(
    input: &str,
    base: Option<&Url>,
    url: Option<&mut Url>,
    state_override: Option<State>,
    encoder: Option<QueryEncoder<'_>>,
) -> Result<Option<Url>, ()> {
    let mut owned;
    let given = url.is_some();
    let url: &mut Url = match url {
        Some(u) => u,
        None => {
            owned = Url::empty();
            &mut owned
        }
    };
    let mut input: &str = input;
    let trimmed;
    if !given {
        trimmed = input.trim_matches(|c: char| (c as u32) <= 0x20);
        input = trimmed;
    }
    let chars: Vec<char> = input
        .chars()
        .filter(|&c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    let mut state = state_override.unwrap_or(State::SchemeStart);
    let mut buffer = String::new();
    let (mut at_sign_seen, mut inside_brackets, mut password_token_seen) = (false, false, false);
    let mut pointer: isize = 0;
    let len = chars.len() as isize;
    let at = |p: isize| -> Option<char> {
        if p >= 0 && p < len {
            Some(chars[p as usize])
        } else {
            None
        }
    };
    let remaining = |p: isize| -> &[char] {
        let start = (p + 1).clamp(0, len) as usize;
        &chars[start..]
    };
    let from = |p: isize| -> &[char] {
        let start = p.clamp(0, len) as usize;
        &chars[start..]
    };

    loop {
        let c = at(pointer);
        match state {
            State::SchemeStart => {
                if let Some(ch) = c.filter(char::is_ascii_alphabetic) {
                    buffer.push(ch.to_ascii_lowercase());
                    state = State::Scheme;
                } else if state_override.is_none() {
                    state = State::NoScheme;
                    pointer -= 1;
                } else {
                    return Err(());
                }
            }
            State::Scheme => {
                if let Some(ch) =
                    c.filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
                {
                    buffer.push(ch.to_ascii_lowercase());
                } else if c == Some(':') {
                    if state_override.is_some() {
                        if url.special() != is_special_scheme(&buffer) {
                            return Ok(None);
                        }
                        if (url.includes_credentials() || url.port.is_some()) && buffer == "file" {
                            return Ok(None);
                        }
                        if url.scheme == "file" && url.host == Some(Host::Empty) {
                            return Ok(None);
                        }
                    }
                    url.scheme = std::mem::take(&mut buffer);
                    if state_override.is_some() {
                        if url.port.is_some() && url.port == default_port(&url.scheme) {
                            url.port = None;
                        }
                        return Ok(None);
                    }
                    if url.scheme == "file" {
                        state = State::File;
                    } else if url.special() && base.is_some_and(|b| b.scheme == url.scheme) {
                        state = State::SpecialRelativeOrAuthority;
                    } else if url.special() {
                        state = State::SpecialAuthoritySlashes;
                    } else if remaining(pointer).first() == Some(&'/') {
                        state = State::PathOrAuthority;
                        pointer += 1;
                    } else {
                        url.path = Path::Opaque(String::new());
                        state = State::OpaquePath;
                    }
                } else if state_override.is_none() {
                    buffer.clear();
                    state = State::NoScheme;
                    pointer = -1;
                } else {
                    return Err(());
                }
            }
            State::NoScheme => {
                let Some(b) = base else {
                    return Err(());
                };
                let opaque = matches!(b.path, Path::Opaque(_));
                if opaque && c != Some('#') {
                    return Err(());
                } else if opaque && c == Some('#') {
                    url.scheme = b.scheme.clone();
                    url.path = b.path.clone();
                    url.query = b.query.clone();
                    url.fragment = Some(String::new());
                    state = State::Fragment;
                } else if b.scheme != "file" {
                    state = State::Relative;
                    pointer -= 1;
                } else {
                    state = State::File;
                    pointer -= 1;
                }
            }
            State::SpecialRelativeOrAuthority => {
                if c == Some('/') && remaining(pointer).first() == Some(&'/') {
                    state = State::SpecialAuthorityIgnoreSlashes;
                    pointer += 1;
                } else {
                    state = State::Relative;
                    pointer -= 1;
                }
            }
            State::PathOrAuthority => {
                if c == Some('/') {
                    state = State::Authority;
                } else {
                    state = State::Path;
                    pointer -= 1;
                }
            }
            State::Relative => {
                let b = base.ok_or(())?;
                url.scheme = b.scheme.clone();
                if c == Some('/') || (url.special() && c == Some('\\')) {
                    state = State::RelativeSlash;
                } else {
                    url.username = b.username.clone();
                    url.password = b.password.clone();
                    url.host = b.host.clone();
                    url.port = b.port;
                    url.path = b.path.clone();
                    url.query = b.query.clone();
                    if c == Some('?') {
                        url.query = Some(String::new());
                        state = State::Query;
                    } else if c == Some('#') {
                        url.fragment = Some(String::new());
                        state = State::Fragment;
                    } else if c.is_some() {
                        url.query = None;
                        url.shorten_path();
                        state = State::Path;
                        pointer -= 1;
                    }
                }
            }
            State::RelativeSlash => {
                if url.special() && matches!(c, Some('/' | '\\')) {
                    state = State::SpecialAuthorityIgnoreSlashes;
                } else if c == Some('/') {
                    state = State::Authority;
                } else {
                    let b = base.ok_or(())?;
                    url.username = b.username.clone();
                    url.password = b.password.clone();
                    url.host = b.host.clone();
                    url.port = b.port;
                    state = State::Path;
                    pointer -= 1;
                }
            }
            State::SpecialAuthoritySlashes => {
                if c == Some('/') && remaining(pointer).first() == Some(&'/') {
                    state = State::SpecialAuthorityIgnoreSlashes;
                    pointer += 1;
                } else {
                    state = State::SpecialAuthorityIgnoreSlashes;
                    pointer -= 1;
                }
            }
            State::SpecialAuthorityIgnoreSlashes => {
                if !matches!(c, Some('/' | '\\')) {
                    state = State::Authority;
                    pointer -= 1;
                }
            }
            State::Authority => {
                if c == Some('@') {
                    if at_sign_seen {
                        buffer.insert_str(0, "%40");
                    }
                    at_sign_seen = true;
                    for cp in buffer.chars() {
                        if cp == ':' && !password_token_seen {
                            password_token_seen = true;
                            continue;
                        }
                        let target = if password_token_seen {
                            &mut url.password
                        } else {
                            &mut url.username
                        };
                        percent::utf8_encode_char(target, cp, EncodeSet::Userinfo);
                    }
                    buffer.clear();
                } else if matches!(c, None | Some('/' | '?' | '#'))
                    || (url.special() && c == Some('\\'))
                {
                    if at_sign_seen && buffer.is_empty() {
                        return Err(());
                    }
                    pointer -= buffer.chars().count() as isize + 1;
                    buffer.clear();
                    state = State::Host;
                } else if let Some(ch) = c {
                    buffer.push(ch);
                }
            }
            State::Host | State::Hostname => {
                if state_override.is_some() && url.scheme == "file" {
                    pointer -= 1;
                    state = State::FileHost;
                } else if c == Some(':') && !inside_brackets {
                    if buffer.is_empty() {
                        return Err(());
                    }
                    if state_override == Some(State::Hostname) {
                        return Err(());
                    }
                    let host = parse_host(&buffer, !url.special()).ok_or(())?;
                    url.host = Some(host);
                    buffer.clear();
                    state = State::Port;
                } else if matches!(c, None | Some('/' | '?' | '#'))
                    || (url.special() && c == Some('\\'))
                {
                    pointer -= 1;
                    let credentials = url.includes_credentials() || url.port.is_some();
                    if buffer.is_empty()
                        && (url.special() || (state_override.is_some() && credentials))
                    {
                        return Err(());
                    }
                    let host = parse_host(&buffer, !url.special()).ok_or(())?;
                    url.host = Some(host);
                    buffer.clear();
                    state = State::PathStart;
                    if state_override.is_some() {
                        return Ok(None);
                    }
                } else if let Some(ch) = c {
                    if ch == '[' {
                        inside_brackets = true;
                    }
                    if ch == ']' {
                        inside_brackets = false;
                    }
                    buffer.push(ch);
                }
            }
            State::Port => {
                if let Some(d) = c.filter(char::is_ascii_digit) {
                    buffer.push(d);
                } else if matches!(c, None | Some('/' | '?' | '#'))
                    || (url.special() && c == Some('\\'))
                    || state_override.is_some()
                {
                    if !buffer.is_empty() {
                        let digits = buffer.trim_start_matches('0');
                        let port: u32 = if digits.is_empty() {
                            0
                        } else if digits.len() > 5 {
                            return Err(());
                        } else {
                            digits.parse().map_err(|_| ())?
                        };
                        if port > 65535 {
                            return Err(());
                        }
                        let port = port as u16;
                        url.port = if Some(port) == default_port(&url.scheme) {
                            None
                        } else {
                            Some(port)
                        };
                        buffer.clear();
                        if state_override.is_some() {
                            return Ok(None);
                        }
                    }
                    if state_override.is_some() {
                        return Err(());
                    }
                    state = State::PathStart;
                    pointer -= 1;
                } else {
                    return Err(());
                }
            }
            State::File => {
                url.scheme = "file".into();
                url.host = Some(Host::Empty);
                if matches!(c, Some('/' | '\\')) {
                    state = State::FileSlash;
                } else if let Some(b) = base.filter(|b| b.scheme == "file") {
                    url.host = b.host.clone();
                    url.path = b.path.clone();
                    url.query = b.query.clone();
                    if c == Some('?') {
                        url.query = Some(String::new());
                        state = State::Query;
                    } else if c == Some('#') {
                        url.fragment = Some(String::new());
                        state = State::Fragment;
                    } else if c.is_some() {
                        url.query = None;
                        if !starts_with_windows_drive_letter(from(pointer)) {
                            url.shorten_path();
                        } else {
                            url.path = Path::List(Vec::new());
                        }
                        state = State::Path;
                        pointer -= 1;
                    }
                } else {
                    state = State::Path;
                    pointer -= 1;
                }
            }
            State::FileSlash => {
                if matches!(c, Some('/' | '\\')) {
                    state = State::FileHost;
                } else {
                    if let Some(b) = base.filter(|b| b.scheme == "file") {
                        url.host = b.host.clone();
                        if !starts_with_windows_drive_letter(from(pointer))
                            && let Path::List(segments) = &b.path
                            && let Some(first) = segments.first()
                            && is_normalized_windows_drive_letter(first)
                        {
                            url.push_segment(first.clone());
                        }
                    }
                    state = State::Path;
                    pointer -= 1;
                }
            }
            State::FileHost => {
                if matches!(c, None | Some('/' | '\\' | '?' | '#')) {
                    pointer -= 1;
                    let buf: Vec<char> = buffer.chars().collect();
                    if state_override.is_none() && is_windows_drive_letter(&buf) {
                        state = State::Path;
                    } else if buffer.is_empty() {
                        url.host = Some(Host::Empty);
                        if state_override.is_some() {
                            return Ok(None);
                        }
                        state = State::PathStart;
                    } else {
                        let mut host = parse_host(&buffer, !url.special()).ok_or(())?;
                        if host == Host::Domain("localhost".into()) {
                            host = Host::Empty;
                        }
                        url.host = Some(host);
                        if state_override.is_some() {
                            return Ok(None);
                        }
                        buffer.clear();
                        state = State::PathStart;
                    }
                } else if let Some(ch) = c {
                    buffer.push(ch);
                }
            }
            State::PathStart => {
                if url.special() {
                    state = State::Path;
                    if !matches!(c, Some('/' | '\\')) {
                        pointer -= 1;
                    }
                } else if state_override.is_none() && c == Some('?') {
                    url.query = Some(String::new());
                    state = State::Query;
                } else if state_override.is_none() && c == Some('#') {
                    url.fragment = Some(String::new());
                    state = State::Fragment;
                } else if c.is_some() {
                    state = State::Path;
                    if c != Some('/') {
                        pointer -= 1;
                    }
                } else if state_override.is_some() && url.host.is_none() {
                    url.push_segment(String::new());
                }
            }
            State::Path => {
                let slash = c == Some('/') || (url.special() && c == Some('\\'));
                if c.is_none()
                    || slash
                    || (state_override.is_none() && matches!(c, Some('?' | '#')))
                {
                    if is_double_dot(&buffer) {
                        url.shorten_path();
                        if !slash {
                            url.push_segment(String::new());
                        }
                    } else if is_single_dot(&buffer) && !slash {
                        url.push_segment(String::new());
                    } else if !is_single_dot(&buffer) {
                        if url.scheme == "file" && url.path_is_empty() {
                            let b: Vec<char> = buffer.chars().collect();
                            if is_windows_drive_letter(&b) {
                                buffer = format!("{}:", b[0]);
                            }
                        }
                        url.push_segment(std::mem::take(&mut buffer));
                    }
                    buffer.clear();
                    if c == Some('?') {
                        url.query = Some(String::new());
                        state = State::Query;
                    }
                    if c == Some('#') {
                        url.fragment = Some(String::new());
                        state = State::Fragment;
                    }
                } else if let Some(ch) = c {
                    percent::utf8_encode_char(&mut buffer, ch, EncodeSet::Path);
                }
            }
            State::OpaquePath => {
                if c == Some('?') {
                    url.query = Some(String::new());
                    state = State::Query;
                } else if c == Some('#') {
                    url.fragment = Some(String::new());
                    state = State::Fragment;
                } else if let Some(ch) = c
                    && let Path::Opaque(p) = &mut url.path
                {
                    if ch == ' ' && matches!(remaining(pointer).first(), Some('?' | '#')) {
                        p.push_str("%20");
                    } else {
                        percent::utf8_encode_char(p, ch, EncodeSet::C0Control);
                    }
                }
            }
            State::Query => {
                if (state_override.is_none() && c == Some('#')) || c.is_none() {
                    let set = if url.special() {
                        EncodeSet::SpecialQuery
                    } else {
                        EncodeSet::Query
                    };
                    // El encoding del documento solo aplica a esquemas especiales salvo ws/wss.
                    let utf8 = !url.special() || matches!(url.scheme.as_str(), "ws" | "wss");
                    // "Percent-encode after encoding" (§1.3).
                    let mut encoded = String::new();
                    match encoder {
                        Some(e) if !utf8 => {
                            for ch in buffer.chars() {
                                match e(ch) {
                                    Some(bytes) => {
                                        encoded
                                            .push_str(&percent::encode_bytes(&bytes, set, false));
                                    }
                                    None => encoded.push_str(&format!("%26%23{}%3B", ch as u32)),
                                }
                            }
                        }
                        _ => encoded = percent::encode_bytes(buffer.as_bytes(), set, false),
                    }
                    url.query.get_or_insert_with(String::new).push_str(&encoded);
                    buffer.clear();
                    if c == Some('#') {
                        url.fragment = Some(String::new());
                        state = State::Fragment;
                    }
                } else if let Some(ch) = c {
                    buffer.push(ch);
                }
            }
            State::Fragment => {
                if let Some(ch) = c {
                    let f = url.fragment.get_or_insert_with(String::new);
                    percent::utf8_encode_char(f, ch, EncodeSet::Fragment);
                }
            }
        }
        // "Si después de una pasada el puntero apunta a EOF, terminar; si no, avanzar."
        if pointer >= len {
            break;
        }
        pointer += 1;
    }
    if given {
        Ok(None)
    } else {
        Ok(Some(url.clone()))
    }
}
