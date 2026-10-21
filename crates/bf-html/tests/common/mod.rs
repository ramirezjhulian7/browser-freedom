//! Host de scripts para los tests del parser: en lugar de un motor de JavaScript, entiende un
//! subconjunto *literal*: sentencias `document.write(...)`, `document.writeln(...)`,
//! `document.open()`, `document.close()` y `log(...)`, cuyos argumentos son literales de string
//! (con `'`/`"` y escapes `\n`, `\\`, `\'`, `\"`, `\uXXXX`) concatenados con `+`. Cualquier otra
//! cosa queda registrada como `unsupported` (necesita JS de verdad).

#![allow(dead_code)]

use std::collections::HashMap;

use bf_dom::events::{Event, EventTarget};
use bf_dom::{Document, DomString, NodeId};
use bf_html::{Parser, ScriptDisposition, ScriptHost};

/// Una sentencia del subconjunto.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Write(DomString),
    Writeln(DomString),
    Open,
    Close,
    Log(String),
}

/// Parsea un script del subconjunto; `None` si tiene algo fuera de él.
pub fn parse_script(src: &str) -> Option<Vec<Stmt>> {
    let cps: Vec<char> = src.chars().collect();
    let mut p = Cursor { cps: &cps, i: 0 };
    let mut out = Vec::new();
    loop {
        p.skip_space();
        while p.eat(';') {
            p.skip_space();
        }
        if p.i == cps.len() {
            return Some(out);
        }
        let name = p.name()?;
        p.skip_space();
        if !p.eat('(') {
            return None;
        }
        let mut args = Vec::new();
        p.skip_space();
        if !p.eat(')') {
            loop {
                args.push(p.concat()?);
                p.skip_space();
                if p.eat(')') {
                    break;
                }
                if !p.eat(',') {
                    return None;
                }
            }
        }
        let joined = || {
            let mut s = DomString::new();
            for a in &args {
                s.push_dom_str(a);
            }
            s
        };
        out.push(match name.as_str() {
            "document.write" => Stmt::Write(joined()),
            "document.writeln" => Stmt::Writeln(joined()),
            "document.open" if args.is_empty() => Stmt::Open,
            "document.close" if args.is_empty() => Stmt::Close,
            "log" => Stmt::Log(joined().as_lossy_str().to_owned()),
            _ => return None,
        });
    }
}

struct Cursor<'a> {
    cps: &'a [char],
    i: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<char> {
        self.cps.get(self.i).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.i += 1;
        }
    }

    fn name(&mut self) -> Option<String> {
        let start = self.i;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
        {
            self.i += 1;
        }
        (self.i > start).then(|| self.cps[start..self.i].iter().collect())
    }

    /// `"a" + 'b' + ...`
    fn concat(&mut self) -> Option<DomString> {
        let mut out = Vec::new();
        loop {
            self.skip_space();
            out.extend(self.string()?);
            self.skip_space();
            if !self.eat('+') {
                return Some(DomString::from_code_points(out));
            }
        }
    }

    fn string(&mut self) -> Option<Vec<u32>> {
        let quote = self.peek().filter(|&c| c == '"' || c == '\'')?;
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let c = self.peek()?;
            self.i += 1;
            match c {
                c if c == quote => return Some(out),
                '\\' => {
                    let e = self.peek()?;
                    self.i += 1;
                    match e {
                        'n' => out.push(0x0A),
                        'r' => out.push(0x0D),
                        't' => out.push(0x09),
                        'u' => {
                            let hex: String = self.cps.get(self.i..self.i + 4)?.iter().collect();
                            out.push(u32::from_str_radix(&hex, 16).ok()?);
                            self.i += 4;
                        }
                        other => out.push(u32::from(other)),
                    }
                }
                c => out.push(u32::from(c)),
            }
        }
    }
}

/// Un script externo conocido por el host.
#[derive(Debug, Clone)]
pub struct External {
    pub source: String,
    /// Qué responde `prepare`: `Ready` (ya descargado), `Pending` (lo completa el test con
    /// `script_fetched`) o `Failed`.
    pub disposition: ScriptDisposition,
}

/// Host de prueba: registra el orden de preparación, ejecución y eventos.
#[derive(Default)]
pub struct TestHost {
    pub log: Vec<String>,
    /// `src` → script externo.
    pub external: HashMap<String, External>,
    /// Nombre de cada listener (handle) para el log.
    pub listeners: HashMap<u64, String>,
    /// Hubo scripts fuera del subconjunto.
    pub unsupported: bool,
}

impl TestHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_external(
        mut self,
        src: &str,
        source: &str,
        disposition: ScriptDisposition,
    ) -> Self {
        self.external.insert(
            src.to_owned(),
            External {
                source: source.to_owned(),
                disposition,
            },
        );
        self
    }

    /// Nombre de un `script` para el log: su `id`, o su `src`, o su texto.
    pub fn label(doc: &Document, script: NodeId) -> String {
        let el = doc.element(script).expect("script");
        if let Some(id) = el.attr("id") {
            return id.to_owned();
        }
        if let Some(src) = el.attr("src") {
            return src.to_owned();
        }
        doc.child_text_content(script).as_lossy_str().to_owned()
    }

    fn source(&self, doc: &Document, script: NodeId) -> String {
        let info = doc.script_info(script).expect("script preparado");
        match &info.src {
            Some(src) => self
                .external
                .get(src.as_lossy_str())
                .map(|e| e.source.clone())
                .unwrap_or_default(),
            None => info.source_text.as_lossy_str().to_owned(),
        }
    }

    /// Corre un script del subconjunto sobre el parser.
    pub fn run(&mut self, parser: &mut Parser, source: &str) {
        let Some(stmts) = parse_script(source) else {
            self.unsupported = true;
            self.log.push(format!("unsupported:{source}"));
            return;
        };
        for stmt in stmts {
            let r = match stmt {
                Stmt::Write(s) => parser.write(self, &s),
                Stmt::Writeln(s) => parser.writeln(self, &s),
                Stmt::Open => parser.open(self),
                Stmt::Close => parser.close(self),
                Stmt::Log(s) => {
                    self.log.push(s);
                    Ok(())
                }
            };
            if let Err(e) = r {
                self.log.push(format!("exception:{e:?}"));
            }
        }
    }
}

impl bf_dom::events::ScriptHost for TestHost {
    fn call_listener(&mut self, doc: &mut Document, handle: u64, event: &mut Event) -> bool {
        let name = self.listeners.get(&handle).cloned().unwrap_or_default();
        let target = match event.target {
            Some(EventTarget::Node(n)) if n == doc.root() => "document".to_owned(),
            Some(EventTarget::Node(n)) if doc.element(n).is_some() => Self::label(doc, n),
            Some(EventTarget::Window) => "window".to_owned(),
            _ => "?".to_owned(),
        };
        let ready = doc.ready_state().as_str();
        self.log.push(format!(
            "{name}:{}@{target}({ready})",
            event.type_.as_lossy_str()
        ));
        false
    }
}

impl ScriptHost for TestHost {
    fn prepare(&mut self, parser: &mut Parser, script: NodeId) -> ScriptDisposition {
        let doc = parser.document();
        self.log
            .push(format!("prepare:{}", Self::label(doc, script)));
        let info = doc.script_info(script).expect("script preparado");
        match &info.src {
            Some(src) => self
                .external
                .get(src.as_lossy_str())
                .map_or(ScriptDisposition::Failed, |e| e.disposition),
            None => ScriptDisposition::Ready,
        }
    }

    fn execute(&mut self, parser: &mut Parser, script: NodeId) {
        let doc = parser.document();
        self.log
            .push(format!("execute:{}", Self::label(doc, script)));
        let source = self.source(doc, script);
        self.run(parser, &source);
    }
}
