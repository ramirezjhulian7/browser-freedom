//! E3: caché de código compilado de los scripts clásicos.
//!
//! El código compilado ([`FunctionCode`]) no tiene valores del heap ni depende del realm: el
//! bytecode, las constantes y los nombres son los mismos en cualquier ventana. Lo único que
//! depende del agente son las inline caches (`FunctionCode::ics`, con objetos y shapes de su
//! heap) y los ids de los sitios de los templates (`TemplateSite::id`, la clave del
//! [[TemplateMap]] del realm, ECMA-262 §13.2.8.4 GetTemplateObject: cada parseo es otro Parse
//! Node). Por eso nunca se entrega dos veces el mismo `FunctionCode`: cada uso recibe una
//! copia con las caches vacías y sitios nuevos ([`fresh_copy`]).
//!
//! Dos niveles:
//! - **Del hilo**: el código ya compilado en este hilo, listo para copiar. Un hilo de pestaña
//!   que navega otra vez no recompila el JavaScript propio de `bf-web` ni los scripts de la
//!   página que no cambiaron.
//! - **Del proceso**: una forma portátil del código (`Send + Sync`, sin `Rc` ni átomos del
//!   hilo), que produce [`warm`] desde cualquier hilo (la precarga, mientras se espera a la
//!   red) y que el hilo que ejecuta convierte a su forma la primera vez.
//!
//! La clave es la URL del script (el nombre que va en el código: los errores y el
//! `Function.prototype.toString` lo usan) y un hash de su texto con su longitud: el texto
//! decide el resultado, así que el validador HTTP no hace falta para acertar. [`warm`] lo
//! recibe igual y lo usa para descartar lo guardado de esa URL con otro validador.
//!
//! Un error de sintaxis de [`warm`] se guarda como "no compila", y al ejecutar el script se
//! vuelve a parsear en el agente que lo ejecuta: el SyntaxError sale entonces, en
//! ScriptEvaluation (ECMA-262 §16.1.5 ParseScript, HTML "create a classic script" con su
//! *error to rethrow*), con las opciones de ese agente, y no al precargar.
//!
//! Solo scripts clásicos: un módulo es un Module Record del realm (§16.2.1.6) y no pasa por
//! aquí.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};

use crate::ast::Span;
use crate::compiler::bytecode::{
    CodeKind, Constant, Declarations, FunctionCode, Op, Source, TemplateSite, next_template_site,
};
use crate::parser::ParseOptions;
use crate::string::{Atom, JsString};
use crate::value::PropertyKey;
use crate::vm::env::{BindingKind, ScopeLayout};

/// Los scripts más chicos no se guardan: compilarlos cuesta menos que buscarlos.
pub const MIN_SOURCE_LEN: usize = 1024;

/// Bytes de fuente que guarda cada nivel antes de descartar lo más viejo.
const LIMIT: usize = 16 << 20;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct Key {
    url: String,
    len: usize,
    hash: u64,
}

impl Key {
    fn new(url: &str, source: &str) -> Key {
        let mut h = std::hash::DefaultHasher::new();
        source.hash(&mut h);
        Key {
            url: url.to_owned(),
            len: source.len(),
            hash: h.finish(),
        }
    }
}

/// Lo que se hizo en este hilo (para las trazas de `BF_TRACE`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Scripts compilados en el hilo (no estaban en ninguna caché).
    pub compiled: usize,
    /// Bytes de fuente de esos scripts.
    pub compiled_bytes: usize,
    /// Copias del código ya compilado en el hilo.
    pub thread_hits: usize,
    /// Scripts tomados de la caché del proceso (precompilados con [`warm`]).
    pub process_hits: usize,
}

// --- Nivel del hilo ---

struct Local {
    map: HashMap<Key, Rc<FunctionCode>>,
    order: VecDeque<Key>,
    bytes: usize,
    stats: Stats,
}

thread_local! {
    static LOCAL: RefCell<Local> = RefCell::new(Local {
        map: HashMap::new(),
        order: VecDeque::new(),
        bytes: 0,
        stats: Stats::default(),
    });
}

/// Lo que se hizo en este hilo hasta ahora.
pub fn stats() -> Stats {
    LOCAL.with(|l| l.borrow().stats)
}

/// El código de `source` (con nombre `url`) si está en alguna caché, como copia propia para
/// ejecutarlo. `None` si hay que compilarlo (o si [`warm`] encontró un error de sintaxis:
/// lo informa el parseo de quien lo ejecuta).
pub(crate) fn lookup(url: &str, source: &str) -> Option<Rc<FunctionCode>> {
    if source.len() < MIN_SOURCE_LEN {
        return None;
    }
    let key = Key::new(url, source);
    let local = LOCAL.with(|l| l.borrow().map.get(&key).cloned());
    if let Some(code) = local {
        LOCAL.with(|l| l.borrow_mut().stats.thread_hits += 1);
        return Some(fresh_copy(&code));
    }
    let entry = global().lock().ok()?.map.get(&key).cloned()?;
    let portable = entry.code.as_ref()?;
    let code = materialize(portable);
    LOCAL.with(|l| {
        let mut l = l.borrow_mut();
        l.stats.process_hits += 1;
        insert_local(&mut l, key, code.clone(), source.len());
    });
    Some(fresh_copy(&code))
}

/// Guarda en el hilo el código recién compilado de `source`; devuelve el que hay que
/// ejecutar (el guardado queda como molde, sin usar).
pub(crate) fn remember(url: &str, source: &str, code: Rc<FunctionCode>) -> Rc<FunctionCode> {
    LOCAL.with(|l| {
        let mut l = l.borrow_mut();
        l.stats.compiled += 1;
        l.stats.compiled_bytes += source.len();
        if source.len() >= MIN_SOURCE_LEN {
            insert_local(
                &mut l,
                Key::new(url, source),
                fresh_copy(&code),
                source.len(),
            );
        }
    });
    code
}

fn insert_local(l: &mut Local, key: Key, code: Rc<FunctionCode>, len: usize) {
    if l.map.insert(key.clone(), code).is_none() {
        l.order.push_back(key);
        l.bytes += len;
    }
    while l.bytes > LIMIT {
        let Some(old) = l.order.pop_front() else {
            break;
        };
        if l.map.remove(&old).is_some() {
            l.bytes -= old.len;
        }
    }
}

// --- Nivel del proceso ---

struct Entry {
    validator: Option<String>,
    /// `None`: no compila (error de sintaxis o anidamiento excesivo).
    code: Option<PortableScript>,
}

struct Global {
    map: HashMap<Key, Arc<Entry>>,
    order: VecDeque<Key>,
    bytes: usize,
}

fn global() -> &'static Mutex<Global> {
    static GLOBAL: OnceLock<Mutex<Global>> = OnceLock::new();
    GLOBAL.get_or_init(|| {
        Mutex::new(Global {
            map: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
        })
    })
}

/// Parsea y compila el script clásico `source` (de `url`, con el validador HTTP de su
/// respuesta si lo hay) y lo deja en la caché del proceso, para que el hilo que lo ejecute no
/// tenga que compilarlo. Se puede llamar desde cualquier hilo y no tiene efectos visibles:
/// un error de sintaxis no se informa aquí sino al ejecutar el script.
pub fn warm(url: &str, validator: Option<&str>, source: &str) {
    if source.len() < MIN_SOURCE_LEN {
        return;
    }
    let key = Key::new(url, source);
    if let Ok(g) = global().lock()
        && g.map
            .get(&key)
            .is_some_and(|e| e.validator.as_deref() == validator)
    {
        return;
    }
    // Sin el cerrojo: compilar puede tardar decenas de ms.
    let code = crate::parse_script(source, &ParseOptions::default())
        .ok()
        .map(|program| {
            let code =
                crate::compiler::compile_script(&program, Rc::new(Source::new(url, source)), false);
            portable_script(&code)
        });
    let Ok(mut g) = global().lock() else {
        return;
    };
    // Lo guardado de esta URL con otro validador ya no vale.
    if validator.is_some() {
        let stale: Vec<Key> = g
            .map
            .iter()
            .filter(|(k, e)| k.url == url && e.validator.as_deref() != validator)
            .map(|(k, _)| k.clone())
            .collect();
        for k in stale {
            g.map.remove(&k);
            g.bytes -= k.len;
        }
    }
    let entry = Arc::new(Entry {
        validator: validator.map(str::to_owned),
        code,
    });
    if g.map.insert(key.clone(), entry).is_none() {
        g.bytes += key.len;
        g.order.push_back(key);
    }
    while g.bytes > LIMIT {
        let Some(old) = g.order.pop_front() else {
            break;
        };
        if g.map.remove(&old).is_some() {
            g.bytes -= old.len;
        }
    }
}

/// ¿Está `source` (de `url`) en la caché del proceso, compilado o con su error? (Tests.)
pub fn is_warm(url: &str, source: &str) -> bool {
    let key = Key::new(url, source);
    global().lock().is_ok_and(|g| g.map.contains_key(&key))
}

// --- La copia propia de cada uso ---

/// Una copia de `code` para un uso nuevo: comparte las instrucciones y las constantes
/// inmutables, con inline caches vacías y sitios de template nuevos (en ella y en sus
/// funciones anidadas).
pub fn fresh_copy(code: &FunctionCode) -> Rc<FunctionCode> {
    let consts = code
        .consts
        .iter()
        .map(|c| match c {
            Constant::Function(f) => Constant::Function(fresh_copy(f)),
            Constant::Template(t) => Constant::Template(Rc::new(TemplateSite {
                id: next_template_site(),
                cooked: t.cooked.clone(),
                raw: t.raw.clone(),
            })),
            other => other.clone(),
        })
        .collect();
    Rc::new(FunctionCode {
        name: code.name.clone(),
        kind: code.kind,
        strict: code.strict,
        is_generator: code.is_generator,
        is_async: code.is_async,
        ops: code.ops.clone(),
        consts,
        num_locals: code.num_locals,
        length: code.length,
        source: code.source.clone(),
        span: code.span,
        positions: code.positions.clone(),
        has_prototype: code.has_prototype,
        decls: code.decls.clone(),
        dynamic: code.dynamic,
        direct_args: code.direct_args,
        ics: Default::default(),
    })
}

// --- La forma portátil (Send + Sync) ---

/// Un script compilado sin nada del hilo: la fuente y el código de su función de script.
struct PortableScript {
    name: String,
    units: Vec<u16>,
    line_starts: Vec<u32>,
    code: PortableCode,
}

struct PortableCode {
    name: Vec<u16>,
    kind: CodeKind,
    strict: bool,
    is_generator: bool,
    is_async: bool,
    ops: Vec<Op>,
    consts: Vec<PortableConst>,
    num_locals: u32,
    length: u32,
    span: Span,
    positions: Vec<(u32, u32)>,
    has_prototype: bool,
    decls: Option<Box<PortableDecls>>,
    dynamic: bool,
    direct_args: Option<u32>,
}

enum PortableConst {
    Number(f64),
    String(Vec<u16>),
    Name(String),
    BigInt(String),
    Function(Box<PortableCode>),
    Layout(PortableLayout),
    Template {
        cooked: Vec<Option<Vec<u16>>>,
        raw: Vec<Vec<u16>>,
    },
    RegExp {
        pattern: Vec<u16>,
        flags: Vec<u16>,
    },
    FinallyTable(Vec<(u32, u32)>),
    ParamMap(Vec<Option<u32>>),
}

struct PortableLayout {
    names: Vec<String>,
    kinds: Vec<BindingKind>,
    initialized: Vec<bool>,
    simple_catch: bool,
}

struct PortableDecls {
    var_names: Vec<String>,
    function_names: Vec<String>,
    lex_names: Vec<(String, bool)>,
    annex_b: Vec<String>,
    strict: bool,
    top_layout: Option<PortableLayout>,
}

fn portable_script(code: &FunctionCode) -> PortableScript {
    PortableScript {
        name: code.source.name.clone(),
        units: code.source.units.clone(),
        line_starts: code.source.line_starts.clone(),
        code: portable_code(code),
    }
}

fn names(atoms: &[Atom]) -> Vec<String> {
    atoms.iter().map(|a| a.as_str().to_owned()).collect()
}

fn atoms(names: &[String]) -> Vec<Atom> {
    names.iter().map(|n| Atom::new(n)).collect()
}

fn portable_layout(l: &ScopeLayout) -> PortableLayout {
    PortableLayout {
        names: names(&l.names),
        kinds: l.kinds.clone(),
        initialized: l.initialized.clone(),
        simple_catch: l.simple_catch,
    }
}

fn portable_code(code: &FunctionCode) -> PortableCode {
    let consts = code
        .consts
        .iter()
        .map(|c| match c {
            Constant::Number(n) => PortableConst::Number(*n),
            Constant::String(s) => PortableConst::String(s.units().to_vec()),
            Constant::Name(a, _) => PortableConst::Name(a.as_str().to_owned()),
            Constant::BigInt(b) => PortableConst::BigInt(b.clone()),
            Constant::Function(f) => PortableConst::Function(Box::new(portable_code(f))),
            Constant::Layout(l) => PortableConst::Layout(portable_layout(l)),
            Constant::Template(t) => PortableConst::Template {
                cooked: t
                    .cooked
                    .iter()
                    .map(|c| c.as_ref().map(|s| s.units().to_vec()))
                    .collect(),
                raw: t.raw.iter().map(|s| s.units().to_vec()).collect(),
            },
            Constant::RegExp { pattern, flags } => PortableConst::RegExp {
                pattern: pattern.units().to_vec(),
                flags: flags.units().to_vec(),
            },
            Constant::FinallyTable(t) => PortableConst::FinallyTable(t.as_ref().clone()),
            Constant::ParamMap(m) => PortableConst::ParamMap(m.as_ref().clone()),
        })
        .collect();
    PortableCode {
        name: code.name.units().to_vec(),
        kind: code.kind,
        strict: code.strict,
        is_generator: code.is_generator,
        is_async: code.is_async,
        ops: code.ops.clone(),
        consts,
        num_locals: code.num_locals,
        length: code.length,
        span: code.span,
        positions: code.positions.clone(),
        has_prototype: code.has_prototype,
        decls: code.decls.as_ref().map(|d| {
            Box::new(PortableDecls {
                var_names: names(&d.var_names),
                function_names: names(&d.function_names),
                lex_names: d
                    .lex_names
                    .iter()
                    .map(|(a, c)| (a.as_str().to_owned(), *c))
                    .collect(),
                annex_b: names(&d.annex_b),
                strict: d.strict,
                top_layout: d.top_layout.as_deref().map(portable_layout),
            })
        }),
        dynamic: code.dynamic,
        direct_args: code.direct_args,
    }
}

fn materialize(p: &PortableScript) -> Rc<FunctionCode> {
    let source = Rc::new(Source {
        name: p.name.clone(),
        units: p.units.clone(),
        line_starts: p.line_starts.clone(),
        module: None,
        url: None,
    });
    materialize_code(&p.code, &source)
}

fn materialize_layout(l: &PortableLayout) -> Rc<ScopeLayout> {
    Rc::new(ScopeLayout {
        names: atoms(&l.names),
        kinds: l.kinds.clone(),
        initialized: l.initialized.clone(),
        simple_catch: l.simple_catch,
    })
}

fn materialize_code(p: &PortableCode, source: &Rc<Source>) -> Rc<FunctionCode> {
    let consts = p
        .consts
        .iter()
        .map(|c| match c {
            PortableConst::Number(n) => Constant::Number(*n),
            PortableConst::String(s) => Constant::String(JsString::from_units(s.clone())),
            PortableConst::Name(n) => {
                let atom = Atom::new(n);
                let key = PropertyKey::from_atom(&atom);
                Constant::Name(atom, key)
            }
            PortableConst::BigInt(b) => Constant::BigInt(b.clone()),
            PortableConst::Function(f) => Constant::Function(materialize_code(f, source)),
            PortableConst::Layout(l) => Constant::Layout(materialize_layout(l)),
            PortableConst::Template { cooked, raw } => Constant::Template(Rc::new(TemplateSite {
                id: next_template_site(),
                cooked: cooked
                    .iter()
                    .map(|c| c.as_ref().map(|s| JsString::from_units(s.clone())))
                    .collect(),
                raw: raw
                    .iter()
                    .map(|s| JsString::from_units(s.clone()))
                    .collect(),
            })),
            PortableConst::RegExp { pattern, flags } => Constant::RegExp {
                pattern: JsString::from_units(pattern.clone()),
                flags: JsString::from_units(flags.clone()),
            },
            PortableConst::FinallyTable(t) => Constant::FinallyTable(Rc::new(t.clone())),
            PortableConst::ParamMap(m) => Constant::ParamMap(Rc::new(m.clone())),
        })
        .collect();
    Rc::new(FunctionCode {
        name: JsString::from_units(p.name.clone()),
        kind: p.kind,
        strict: p.strict,
        is_generator: p.is_generator,
        is_async: p.is_async,
        ops: p.ops.clone(),
        consts,
        num_locals: p.num_locals,
        length: p.length,
        source: source.clone(),
        span: p.span,
        positions: p.positions.clone(),
        has_prototype: p.has_prototype,
        decls: p.decls.as_ref().map(|d| {
            Box::new(Declarations {
                var_names: atoms(&d.var_names),
                function_names: atoms(&d.function_names),
                lex_names: d
                    .lex_names
                    .iter()
                    .map(|(n, c)| (Atom::new(n), *c))
                    .collect(),
                annex_b: atoms(&d.annex_b),
                strict: d.strict,
                top_layout: d.top_layout.as_ref().map(materialize_layout),
            })
        }),
        dynamic: p.dynamic,
        direct_args: p.direct_args,
        ics: Default::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn portable_code_is_send_and_sync() {
        assert_send_sync::<PortableScript>();
        assert_send_sync::<Entry>();
    }
}
