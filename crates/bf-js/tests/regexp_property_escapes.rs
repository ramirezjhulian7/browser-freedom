//! Los tests generados de `\p{…}` de test262 (`built-ins/RegExp/property-escapes/generated`,
//! incluidos los de propiedades de strings) ejecutados directamente sobre el motor de RegExp:
//! los strings de `buildString` se arman en Rust y cada `testPropertyEscapes` /
//! `testPropertyOfStrings` se comprueba con [`bf_js::regexp::Regex`]. Es la misma verificación
//! que hacen los tests en JavaScript (`harness/regExpUtils.js`), sin el costo de construir
//! strings de un millón de code points en el intérprete. Sin `tests/test262` (ver
//! `tools/fetch-test262`), el test no hace nada.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use bf_js::regexp::Regex;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/test262/test/built-ins/RegExp/property-escapes/generated")
}

/// Un parser mínimo del subconjunto de JavaScript que usan los tests generados.
struct Src<'a> {
    s: &'a [u8],
    i: usize,
}

impl Src<'_> {
    fn skip_ws(&mut self) {
        while self.i < self.s.len() && (self.s[self.i] as char).is_whitespace() {
            self.i += 1;
        }
    }

    fn find(&mut self, pat: &str) -> bool {
        let p = pat.as_bytes();
        while self.i + p.len() <= self.s.len() {
            if &self.s[self.i..self.i + p.len()] == p {
                self.i += p.len();
                return true;
            }
            self.i += 1;
        }
        false
    }

    fn eat(&mut self, c: u8) -> bool {
        self.skip_ws();
        if self.s.get(self.i) == Some(&c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn number(&mut self) -> u32 {
        self.skip_ws();
        let start = self.i;
        while self.i < self.s.len() && (self.s[self.i] as char).is_ascii_alphanumeric() {
            self.i += 1;
        }
        let t = std::str::from_utf8(&self.s[start..self.i]).unwrap();
        match t.strip_prefix("0x") {
            Some(h) => u32::from_str_radix(h, 16).unwrap(),
            None => t.parse().unwrap(),
        }
    }

    fn ident(&mut self) -> String {
        self.skip_ws();
        let start = self.i;
        while self.i < self.s.len()
            && ((self.s[self.i] as char).is_ascii_alphanumeric() || self.s[self.i] == b'_')
        {
            self.i += 1;
        }
        String::from_utf8(self.s[start..self.i].to_vec()).unwrap()
    }

    /// Un literal de regex `/…/flags`: (patrón, flags).
    fn regex(&mut self) -> (String, String) {
        self.skip_ws();
        assert_eq!(self.s[self.i], b'/');
        self.i += 1;
        let start = self.i;
        let mut in_class = false;
        while self.s[self.i] != b'/' || in_class {
            match self.s[self.i] {
                b'\\' => self.i += 1,
                b'[' => in_class = true,
                b']' => in_class = false,
                _ => {}
            }
            self.i += 1;
        }
        let pattern = String::from_utf8(self.s[start..self.i].to_vec()).unwrap();
        self.i += 1;
        let fstart = self.i;
        while (self.s[self.i] as char).is_ascii_alphabetic() {
            self.i += 1;
        }
        let flags = String::from_utf8(self.s[fstart..self.i].to_vec()).unwrap();
        (pattern, flags)
    }

    /// Un literal de string con escapes `\uXXXX`, `\xXX`, `\\` y `\"`: code units.
    fn string(&mut self) -> Vec<u16> {
        self.skip_ws();
        let q = self.s[self.i];
        assert!(q == b'"' || q == b'\'');
        self.i += 1;
        let mut out: Vec<u16> = Vec::new();
        let rest = std::str::from_utf8(&self.s[self.i..]).unwrap();
        let mut chars = rest.char_indices();
        while let Some((k, c)) = chars.next() {
            if c as u32 == u32::from(q) {
                self.i += k + 1;
                return out;
            }
            if c != '\\' {
                let mut buf = [0u16; 2];
                out.extend_from_slice(c.encode_utf16(&mut buf));
                continue;
            }
            let (_, e) = chars.next().unwrap();
            match e {
                'u' => {
                    let mut h = String::new();
                    let (_, first) = chars.next().unwrap();
                    if first == '{' {
                        for (_, c) in chars.by_ref() {
                            if c == '}' {
                                break;
                            }
                            h.push(c);
                        }
                        push_cp(&mut out, u32::from_str_radix(&h, 16).unwrap());
                    } else {
                        h.push(first);
                        h.extend((0..3).map(|_| chars.next().unwrap().1));
                        out.push(u16::from_str_radix(&h, 16).unwrap());
                    }
                }
                'x' => {
                    let h: String = (0..2).map(|_| chars.next().unwrap().1).collect();
                    out.push(u16::from_str_radix(&h, 16).unwrap());
                }
                other => {
                    let mut buf = [0u16; 2];
                    out.extend_from_slice(other.encode_utf16(&mut buf));
                }
            }
        }
        panic!("string sin terminar");
    }

    /// `[ elem, elem, … ]` con `f` para cada elemento.
    fn list<T>(&mut self, mut f: impl FnMut(&mut Self) -> T) -> Vec<T> {
        assert!(self.eat(b'['));
        let mut out = Vec::new();
        loop {
            if self.eat(b']') {
                return out;
            }
            out.push(f(self));
            self.eat(b',');
        }
    }
}

fn push_cp(out: &mut Vec<u16>, cp: u32) {
    match char::from_u32(cp) {
        Some(c) => {
            let mut buf = [0u16; 2];
            out.extend_from_slice(c.encode_utf16(&mut buf));
        }
        None => out.push(cp as u16),
    }
}

/// `buildString({ loneCodePoints: [...], ranges: [[a, b], ...] })` (después de `({`).
fn build_string(src: &mut Src<'_>) -> Vec<u16> {
    assert!(src.find("loneCodePoints:"));
    let lone = src.list(Src::number);
    assert!(src.find("ranges:"));
    let ranges = src.list(|s| {
        let v = s.list(Src::number);
        (v[0], v[1])
    });
    let mut out = Vec::new();
    for cp in lone {
        push_cp(&mut out, cp);
    }
    for (a, b) in ranges {
        for cp in a..=b {
            push_cp(&mut out, cp);
        }
    }
    out
}

fn test_regex(r: &Regex, s: &[u16]) -> bool {
    r.match_at(s, 0, None)
        .expect("límite del matcher")
        .is_some()
}

/// Ejecuta un archivo; devuelve los errores.
fn run_file(path: &std::path::Path) -> Vec<String> {
    let text = fs::read_to_string(path).unwrap();
    let body = &text[text.find("---*/").unwrap()..];
    let mut src = Src {
        s: body.as_bytes(),
        i: 0,
    };
    let mut vars: HashMap<String, Vec<u16>> = HashMap::new();
    let mut errors = Vec::new();
    loop {
        let save = src.i;
        let next_const = src.find("const ").then_some(src.i);
        src.i = save;
        let next_test = src.find("testPropertyEscapes(").then_some(src.i);
        src.i = save;
        let next_strings = src.find("testPropertyOfStrings(").then_some(src.i);
        src.i = save;
        let next = [next_const, next_test, next_strings]
            .into_iter()
            .flatten()
            .min();
        let Some(at) = next else { break };
        src.i = at;
        if Some(at) == next_const {
            let name = src.ident();
            assert!(src.find("buildString("));
            vars.insert(name, build_string(&mut src));
        } else if Some(at) == next_test {
            let (p, f) = src.regex();
            src.eat(b',');
            let var = src.ident();
            let r = match Regex::new(&p.encode_utf16().collect::<Vec<_>>(), &f) {
                Ok(r) => r,
                Err(e) => {
                    errors.push(format!("/{p}/{f}: {e}"));
                    continue;
                }
            };
            if !test_regex(&r, &vars[&var]) {
                errors.push(format!("/{p}/{f} no acepta {var}"));
            }
        } else {
            assert!(src.find("regExp:"));
            let (p, f) = src.regex();
            let r = match Regex::new(&p.encode_utf16().collect::<Vec<_>>(), &f) {
                Ok(r) => r,
                Err(e) => {
                    errors.push(format!("/{p}/{f}: {e}"));
                    continue;
                }
            };
            assert!(src.find("matchStrings:"));
            let matches = src.list(Src::string);
            let all: Vec<u16> = matches.concat();
            if !test_regex(&r, &all) {
                for m in &matches {
                    if !test_regex(&r, m) {
                        errors.push(format!(
                            "/{p}/{f} no acepta {:?}",
                            String::from_utf16_lossy(m)
                        ));
                    }
                }
            }
            let save = src.i;
            let end = if src.find("});") { src.i } else { src.s.len() };
            src.i = save;
            if src.find("nonMatchStrings:") && src.i < end {
                // Como regExpUtils.js: solo si el string concatenado coincide se revisa
                // cada uno.
                let non = src.list(Src::string);
                if !test_regex(&r, &non.concat()) {
                    continue;
                }
                for m in non {
                    if test_regex(&r, &m) {
                        errors.push(format!(
                            "/{p}/{f} acepta {:?}",
                            String::from_utf16_lossy(&m)
                        ));
                    }
                }
            } else {
                src.i = save;
            }
        }
    }
    errors
}

#[test]
fn test262_property_escapes() {
    let dir = root();
    if !dir.exists() {
        eprintln!("sin {}: se omite", dir.display());
        return;
    }
    let mut files: Vec<PathBuf> = Vec::new();
    for sub in [dir.clone(), dir.join("strings")] {
        for e in fs::read_dir(&sub).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x == "js") {
                files.push(p);
            }
        }
    }
    files.sort();
    // Los archivos se reparten entre hilos (cada uno arma strings de un millón de code
    // points).
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let next = std::sync::atomic::AtomicUsize::new(0);
    let failed = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(f) = files.get(k) else { break };
                    let errs = run_file(f);
                    if !errs.is_empty() {
                        failed.lock().unwrap().push(format!(
                            "{}: {}",
                            f.file_name().unwrap().to_string_lossy(),
                            errs.join("; ")
                        ));
                    }
                }
            });
        }
    });
    let mut failed = failed.into_inner().unwrap();
    failed.sort();
    eprintln!(
        "property-escapes/generated: {} de {} archivos pasan",
        files.len() - failed.len(),
        files.len()
    );
    assert!(failed.is_empty(), "fallan:\n{}", failed.join("\n"));
}
