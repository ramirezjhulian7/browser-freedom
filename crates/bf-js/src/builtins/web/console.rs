//! El namespace `console` (Console Standard §1): `log`, `info`, `warn`, `error`, `debug`,
//! `trace`, `dir`, `dirxml`, `table`, `assert`, `clear`, `count`/`countReset`,
//! `group`/`groupCollapsed`/`groupEnd` y `time`/`timeLog`/`timeEnd`, con el Formatter de §2.2
//! (`%s %d %i %f %o %O %c %%`). Los mensajes van a un [`HostConsole`] que pone el navegador
//! (el panel del motor); sin uno, a `print` del agente.

use std::collections::HashMap;
use std::time::Instant;

use crate::builtins::web::{WebData, data_of_mut, registry_get, registry_set};
use crate::heap::Gc;
use crate::object::{Object, ObjectKind, PropValue};
use crate::string::JsString;
use crate::value::{PropertyKey, Value};
use crate::vm::function::{CallArgs, NativeFn};
use crate::vm::{Agent, JsResult, RealmId};

/// El nivel de un mensaje (Console Standard §2.1, Logger: logLevel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Log,
    Info,
    Warn,
    Error,
    Debug,
}

/// Lo que el host hace con los mensajes de `console` (Printer, §2.3).
pub trait HostConsole {
    /// Un mensaje ya formateado, con la profundidad de grupo actual.
    fn message(&mut self, level: LogLevel, group_depth: usize, text: &str);
    /// `console.clear()`.
    fn clear(&mut self) {}
}

/// El estado de `console`: el sink, los contadores, los timers y el grupo.
#[derive(Default)]
pub struct ConsoleState {
    sink: Option<Box<dyn HostConsole>>,
    counts: HashMap<String, u64>,
    timers: HashMap<String, Instant>,
    depth: usize,
}

/// Conecta el [`HostConsole`] del realm.
pub fn set_host_console(agent: &mut Agent, realm: RealmId, sink: Box<dyn HostConsole>) {
    if let Some(c) = registry_get(agent, realm, "console")
        && let Some(WebData::Console(s)) = data_of_mut(agent, c)
    {
        s.sink = Some(sink);
    }
}

pub fn init(agent: &mut Agent, realm: RealmId) {
    // El namespace object (§1): su prototipo es un objeto vacío creado como con
    // OrdinaryObjectCreate(%Object.prototype%).
    let op = agent.realms[realm].intrinsics.object_prototype;
    let proto = agent.new_object(Some(op));
    let c = agent.alloc(Object::new(
        Some(proto),
        ObjectKind::Web(Box::new(WebData::Console(Box::default()))),
    ));
    crate::builtins::to_string_tag(agent, c, "console");
    const OPS: &[(&str, u32, NativeFn)] = &[
        ("assert", 0, assert),
        ("clear", 0, clear),
        ("debug", 0, debug),
        ("error", 0, error),
        ("info", 0, info),
        ("log", 0, log),
        ("table", 0, table),
        ("trace", 0, trace),
        ("warn", 0, warn),
        ("dir", 0, dir),
        ("dirxml", 0, log),
        ("count", 0, count),
        ("countReset", 0, count_reset),
        ("group", 0, group),
        ("groupCollapsed", 0, group),
        ("groupEnd", 0, group_end),
        ("time", 0, time),
        ("timeLog", 0, time_log),
        ("timeEnd", 0, time_end),
    ];
    for &(name, len, f) in OPS {
        crate::builtins::web::op(agent, c, name, len, f);
    }
    registry_set(agent, realm, "console", c);
    crate::builtins::register_global(agent, realm, "console", c);
}

/// Logger (§2.1): el mensaje al sink del realm actual.
fn emit(agent: &mut Agent, level: LogLevel, text: &str) {
    let realm = agent.current_realm;
    let Some(c) = registry_get(agent, realm, "console") else {
        return;
    };
    let mut fallback: Option<(usize, String)> = None;
    if let Some(WebData::Console(s)) = data_of_mut(agent, c) {
        let depth = s.depth;
        match &mut s.sink {
            Some(sink) => sink.message(level, depth, text),
            None => fallback = Some((depth, text.to_owned())),
        }
    }
    if let Some((depth, text)) = fallback {
        let indent = "  ".repeat(depth);
        let line: String = text
            .lines()
            .map(|l| format!("{indent}{l}"))
            .collect::<Vec<_>>()
            .join("\n");
        agent.print(&line);
    }
}

fn with_state<T>(agent: &mut Agent, f: impl FnOnce(&mut ConsoleState) -> T) -> Option<T> {
    let realm = agent.current_realm;
    let c = registry_get(agent, realm, "console")?;
    match data_of_mut(agent, c) {
        Some(WebData::Console(s)) => Some(f(s)),
        _ => None,
    }
}

// --- Formatter (§2.2) ---

/// Formatter: aplica los especificadores del primer argumento y une el resto con espacios.
pub fn format_args(agent: &mut Agent, args: &[Value]) -> JsResult<String> {
    let mut out = String::new();
    let mut rest = 0;
    if let Some(Value::String(first)) = args.first() {
        rest = 1;
        let f = first.to_string_lossy();
        let mut chars = f.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            let Some(&spec) = chars.peek() else {
                out.push('%');
                break;
            };
            match spec {
                '%' => {
                    chars.next();
                    out.push('%');
                }
                's' | 'd' | 'i' | 'f' | 'o' | 'O' | 'c' if rest < args.len() => {
                    chars.next();
                    let v = args[rest].clone();
                    rest += 1;
                    match spec {
                        's' => match &v {
                            Value::String(s) => out.push_str(&s.to_string_lossy()),
                            Value::Symbol(_) | Value::Object(_) => {
                                out.push_str(&inspect(agent, &v, 1))
                            }
                            _ => out.push_str(&agent.to_string(&v)?.to_string_lossy()),
                        },
                        'd' | 'i' => {
                            let n = match v {
                                Value::Symbol(_) | Value::Object(_) => f64::NAN,
                                _ => agent.to_number(&v)?.trunc(),
                            };
                            out.push_str(&crate::abstract_ops::number::number_to_std_string(n));
                        }
                        'f' => {
                            let n = match v {
                                Value::Symbol(_) | Value::Object(_) => f64::NAN,
                                _ => agent.to_number(&v)?,
                            };
                            out.push_str(&crate::abstract_ops::number::number_to_std_string(n));
                        }
                        'o' | 'O' => out.push_str(&inspect(agent, &v, 2)),
                        // %c: estilos CSS, que la salida de texto ignora.
                        _ => {}
                    }
                }
                _ => out.push('%'),
            }
        }
    }
    for v in &args[rest..] {
        if !out.is_empty() || rest > 0 {
            out.push(' ');
        }
        match v {
            Value::String(s) => out.push_str(&s.to_string_lossy()),
            other => out.push_str(&inspect(agent, other, 2)),
        }
    }
    Ok(out)
}

/// Una representación legible de un valor (el "generic JavaScript object formatting" de
/// §2.2.1) que no ejecuta código del usuario: solo propiedades de datos propias.
pub fn inspect(agent: &mut Agent, v: &Value, depth: usize) -> String {
    let mut seen = Vec::new();
    inspect_inner(agent, v, depth, &mut seen, false)
}

fn quote(s: &str) -> String {
    format!(
        "'{}'",
        s.replace('\\', "\\\\")
            .replace('\'', "\\'")
            .replace('\n', "\\n")
    )
}

fn inspect_inner(
    agent: &mut Agent,
    v: &Value,
    depth: usize,
    seen: &mut Vec<Gc<Object>>,
    nested: bool,
) -> String {
    let o = match v {
        Value::Undefined => return "undefined".into(),
        Value::Null => return "null".into(),
        Value::Boolean(b) => return b.to_string(),
        Value::Number(n) => {
            if *n == 0.0 && n.is_sign_negative() {
                return "-0".into();
            }
            return crate::abstract_ops::number::number_to_std_string(*n);
        }
        Value::BigInt(b) => return format!("{}n", agent.heap[*b].to_string_radix(10)),
        Value::String(s) => {
            let s = s.to_string_lossy();
            return if nested { quote(&s) } else { s };
        }
        Value::Symbol(s) => {
            let d = agent.heap[*s]
                .description
                .as_ref()
                .map(JsString::to_string_lossy)
                .unwrap_or_default();
            return format!("Symbol({d})");
        }
        Value::Object(o) => *o,
    };
    if seen.contains(&o) {
        return "[Circular]".into();
    }
    let name_of = |agent: &Agent, f: Gc<Object>| -> String {
        match agent.heap[f].props.get(&agent.keys.name).map(|p| &p.value) {
            Some(PropValue::Data {
                value: Value::String(s),
                ..
            }) => s.to_string_lossy(),
            _ => String::new(),
        }
    };
    {
        let obj = &agent.heap[o];
        if obj.is_callable() {
            let n = name_of(agent, o);
            let kind = if matches!(&obj.kind, ObjectKind::Function(c) if c.class.is_some()) {
                "class"
            } else {
                "Function"
            };
            return if n.is_empty() {
                format!("[{kind} (anonymous)]")
            } else {
                format!("[{kind}: {n}]")
            };
        }
        match &obj.kind {
            ObjectKind::Date(t) => {
                return if t.is_nan() {
                    "Invalid Date".into()
                } else {
                    let d = crate::builtins::date::create_date(agent, *t);
                    match agent.invoke(&Value::Object(d), &PropertyKey::from("toISOString"), &[]) {
                        Ok(Value::String(s)) => s.to_string_lossy(),
                        _ => "Date".into(),
                    }
                };
            }
            ObjectKind::RegExp(r) => {
                return format!(
                    "/{}/{}",
                    r.source.to_string_lossy(),
                    r.flags.to_string_lossy()
                );
            }
            ObjectKind::Error => {
                let get = |k: &str| match agent.heap[o]
                    .props
                    .get(&PropertyKey::from(k))
                    .map(|p| &p.value)
                {
                    Some(PropValue::Data {
                        value: Value::String(s),
                        ..
                    }) => Some(s.to_string_lossy()),
                    _ => None,
                };
                if let Some(stack) = get("stack").filter(|s| s.contains(" at ")) {
                    let msg = get("message").unwrap_or_default();
                    return format!("Error: {msg}\n{}", stack.trim_end());
                }
                let msg = get("message").unwrap_or_default();
                return if msg.is_empty() {
                    "Error".into()
                } else {
                    format!("Error: {msg}")
                };
            }
            ObjectKind::Proxy(_) => return "Proxy {}".into(),
            _ => {}
        }
    }
    if depth == 0 {
        return if agent.heap[o].is_array() {
            "[Array]".into()
        } else {
            "[Object]".into()
        };
    }
    seen.push(o);
    let mut parts: Vec<String> = Vec::new();
    let is_array = agent.heap[o].is_array();
    let mut prefix = String::new();
    match &agent.heap[o].kind {
        ObjectKind::Map(m) | ObjectKind::Set(m) => {
            let is_set = matches!(agent.heap[o].kind, ObjectKind::Set(_));
            let entries: Vec<(Value, Value)> = m.entries.iter().flatten().cloned().collect();
            prefix = format!("{}({}) ", if is_set { "Set" } else { "Map" }, entries.len());
            for (k, val) in entries.iter().take(100) {
                let ks = inspect_inner(agent, k, depth - 1, seen, true);
                if is_set {
                    parts.push(ks);
                } else {
                    let vs = inspect_inner(agent, val, depth - 1, seen, true);
                    parts.push(format!("{ks} => {vs}"));
                }
            }
        }
        ObjectKind::Array(a) => {
            let items: Vec<Option<Value>> = a.dense.clone();
            let len = a.length as usize;
            for i in 0..len.min(100) {
                match items.get(i).cloned().flatten() {
                    Some(x) => parts.push(inspect_inner(agent, &x, depth - 1, seen, true)),
                    None => parts.push("<empty>".into()),
                }
            }
            if len > 100 {
                parts.push(format!("... {} more items", len - 100));
            }
        }
        _ => {}
    }
    // Las propiedades propias con clave string (en un array, las no índice).
    let props: Vec<(PropertyKey, PropValue, bool)> = agent.heap[o]
        .props
        .keys()
        .into_iter()
        .filter_map(|k| {
            let p = agent.heap[o].props.get(&k)?;
            Some((k.clone(), p.value.clone(), p.enumerable))
        })
        .collect();
    for (k, pv, enumerable) in props {
        if !enumerable || k.is_symbol() || (is_array && k.as_index().is_some()) {
            continue;
        }
        let ks = k.to_string();
        let key = if !ks.is_empty()
            && ks
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
            && !ks.starts_with(|c: char| c.is_ascii_digit())
        {
            ks
        } else {
            quote(&ks)
        };
        let vs = match pv {
            PropValue::Data { value, .. } => inspect_inner(agent, &value, depth - 1, seen, true),
            PropValue::Accessor { get, set } => match (get.is_some(), set.is_some()) {
                (true, true) => "[Getter/Setter]".into(),
                (true, false) => "[Getter]".into(),
                _ => "[Setter]".into(),
            },
        };
        parts.push(format!("{key}: {vs}"));
    }
    seen.pop();
    let (open, close) = if is_array { ("[", "]") } else { ("{", "}") };
    if parts.is_empty() {
        return format!("{prefix}{open}{close}");
    }
    format!("{prefix}{open} {} {close}", parts.join(", "))
}

// --- Métodos (§1.1) ---

fn logger(agent: &mut Agent, level: LogLevel, args: &[Value]) -> JsResult<Value> {
    let text = format_args(agent, args)?;
    emit(agent, level, &text);
    Ok(Value::Undefined)
}

fn log(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    logger(agent, LogLevel::Log, a.args)
}

fn info(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    logger(agent, LogLevel::Info, a.args)
}

fn warn(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    logger(agent, LogLevel::Warn, a.args)
}

fn error(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    logger(agent, LogLevel::Error, a.args)
}

fn debug(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    logger(agent, LogLevel::Debug, a.args)
}

fn dir(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let text = inspect(agent, &a.arg(0), 2);
    emit(agent, LogLevel::Log, &text);
    Ok(Value::Undefined)
}

/// `table(data)`: sin tabla en la salida de texto, como `log` (permitido por §1.1.6).
fn table(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    logger(agent, LogLevel::Log, a.args)
}

/// `trace(...data)`: el mensaje y la pila de llamadas.
fn trace(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let mut text = format_args(agent, a.args)?;
    text = if text.is_empty() {
        "Trace".into()
    } else {
        format!("Trace: {text}")
    };
    let e = agent.new_plain_object();
    agent.attach_stack(e);
    if let Ok(Value::String(s)) = agent.get_str(e, "stack") {
        let s = s.to_string_lossy();
        if !s.is_empty() {
            text.push('\n');
            text.push_str(s.trim_end());
        }
    }
    emit(agent, LogLevel::Debug, &text);
    Ok(Value::Undefined)
}

/// `assert(condition, ...data)` (§1.1.1).
fn assert(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    if crate::abstract_ops::conversion::to_boolean(agent, &a.arg(0)) {
        return Ok(Value::Undefined);
    }
    let mut data: Vec<Value> = a.args.iter().skip(1).cloned().collect();
    let message = "Assertion failed";
    match data.first() {
        None => data.push(Value::str(message)),
        Some(Value::String(s)) => {
            let joined = format!("{message}: {}", s.to_string_lossy());
            data[0] = Value::str(&joined);
        }
        Some(_) => data.insert(0, Value::str(message)),
    }
    logger(agent, LogLevel::Error, &data)
}

fn clear(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    with_state(agent, |s| {
        s.depth = 0;
        if let Some(sink) = &mut s.sink {
            sink.clear();
        }
    });
    Ok(Value::Undefined)
}

fn label(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<String> {
    let v = a.arg(0);
    if v.is_undefined() {
        Ok("default".into())
    } else {
        Ok(agent.to_string(&v)?.to_string_lossy())
    }
}

/// `count(label)` (§1.2.1).
fn count(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let l = label(agent, a)?;
    let n = with_state(agent, |s| {
        let c = s.counts.entry(l.clone()).or_insert(0);
        *c += 1;
        *c
    })
    .unwrap_or(1);
    emit(agent, LogLevel::Info, &format!("{l}: {n}"));
    Ok(Value::Undefined)
}

/// `countReset(label)` (§1.2.2).
fn count_reset(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let l = label(agent, a)?;
    let ok = with_state(agent, |s| match s.counts.get_mut(&l) {
        Some(c) => {
            *c = 0;
            true
        }
        None => false,
    })
    .unwrap_or(false);
    if !ok {
        emit(
            agent,
            LogLevel::Warn,
            &format!("Count for '{l}' does not exist"),
        );
    }
    Ok(Value::Undefined)
}

/// `group(...data)` y `groupCollapsed(...data)` (§1.3.1).
fn group(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    if !a.args.is_empty() {
        logger(agent, LogLevel::Log, a.args)?;
    }
    with_state(agent, |s| s.depth += 1);
    Ok(Value::Undefined)
}

fn group_end(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    with_state(agent, |s| s.depth = s.depth.saturating_sub(1));
    Ok(Value::Undefined)
}

/// `time(label)` (§1.4.1).
fn time(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let l = label(agent, a)?;
    let exists = with_state(agent, |s| {
        if s.timers.contains_key(&l) {
            true
        } else {
            s.timers.insert(l.clone(), Instant::now());
            false
        }
    })
    .unwrap_or(false);
    if exists {
        emit(
            agent,
            LogLevel::Warn,
            &format!("Timer '{l}' already exists"),
        );
    }
    Ok(Value::Undefined)
}

fn elapsed(agent: &mut Agent, l: &str, remove: bool) -> Option<f64> {
    with_state(agent, |s| {
        let t = if remove {
            s.timers.remove(l)
        } else {
            s.timers.get(l).copied()
        };
        t.map(|t| t.elapsed().as_secs_f64() * 1000.0)
    })
    .flatten()
}

/// `timeLog(label, ...data)` (§1.4.2).
fn time_log(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let l = label(agent, a)?;
    match elapsed(agent, &l, false) {
        Some(ms) => {
            let mut text = format!("{l}: {ms:.3} ms");
            let rest = format_args(agent, a.args.get(1..).unwrap_or(&[]))?;
            if !rest.is_empty() {
                text.push(' ');
                text.push_str(&rest);
            }
            emit(agent, LogLevel::Log, &text);
        }
        None => emit(
            agent,
            LogLevel::Warn,
            &format!("Timer '{l}' does not exist"),
        ),
    }
    Ok(Value::Undefined)
}

/// `timeEnd(label)` (§1.4.3).
fn time_end(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let l = label(agent, a)?;
    match elapsed(agent, &l, true) {
        Some(ms) => emit(agent, LogLevel::Info, &format!("{l}: {ms:.3} ms")),
        None => emit(
            agent,
            LogLevel::Warn,
            &format!("Timer '{l}' does not exist"),
        ),
    }
    Ok(Value::Undefined)
}
